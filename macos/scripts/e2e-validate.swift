import Foundation

// Standalone (non-XCTest) real-network end-to-end validation of the macOS
// GUI's backend contract (#67) -- exercises the exact same Swift
// networking/decoding code (CoreServiceClient.swift, Models.swift,
// ServiceLifecycle.swift) the app itself uses, against real curated repos
// (the same list core/tests/e2e_real_repos.rs, #21, validated at the Rust
// layer), but through the real local HTTP API and real Swift JSONDecoder
// conformance -- catching cross-language contract bugs (snake_case
// conversion, the InstallStage hand-written Decodable, etc.) that a
// Rust-only test can't.
//
// Deliberately NOT an XCTest target: in this specific dev environment,
// spawning genjuxd from an XCTest host process was observed to hang
// indefinitely (see the note in ServiceLifecycle.swift's spawnGenjuxd()
// and macos/README.md) -- a process-launch quirk unrelated to this code,
// but one that makes XCTest an unreliable vehicle for *this* validation
// pass specifically. Running as a plain compiled executable sidesteps it
// entirely (and was confirmed to do so).
//
// Usage:
//   swiftc -O \
//     GenjuxStore/Core/ServiceInfo.swift \
//     GenjuxStore/Core/ServiceLifecycle.swift \
//     GenjuxStore/Core/Models.swift \
//     GenjuxStore/Core/CoreServiceClient.swift \
//     scripts/e2e-validate.swift \
//     -o /tmp/genjux-e2e-validate
//   GENJUX_GENJUXD_PATH=../target/debug/genjuxd \
//   GENJUX_RUNTIME_DIR=/tmp/genjux-e2e-runtime \
//   GENJUX_GITHUB_TOKEN=... \
//     /tmp/genjux-e2e-validate

var failures: [String] = []
var checksRun = 0

func check(_ name: String, _ condition: @autoclosure () -> Bool, detail: String = "") {
    checksRun += 1
    if condition() {
        print("  \u{2713} \(name)")
    } else {
        let msg = "\(name)\(detail.isEmpty ? "" : " (\(detail))")"
        print("  \u{2717} \(msg)")
        failures.append(msg)
    }
}

func section(_ title: String) {
    print("\n== \(title) ==")
}

let client = CoreServiceClient.shared

func run() async {
    section("Health check")
    do {
        let health = try await client.health()
        check("status is ok", health.status == "ok")
        check("version is non-empty", !health.version.isEmpty)
    } catch {
        check("health() succeeded", false, detail: "\(error)")
        print("\nFATAL: cannot continue without a healthy service. Aborting.")
        print("\n\(checksRun) checks run, \(failures.count) failed.")
        cleanUpSpawnedGenjuxd()
        exit(1)
    }

    // Curated real repos, same set core/tests/e2e_real_repos.rs (#21)
    // validated at the Rust classification layer -- re-checked here
    // through the real HTTP API + real Swift JSON decoding.
    section("BurntSushi/ripgrep — clean per-platform naming")
    do {
        let packages = try await client.packages(owner: "BurntSushi", repo: "ripgrep")
        check("has at least one package", !packages.isEmpty)
        let macos = packages.first { $0.classification.platform == .macOS }
        check("has a classified macOS package", macos != nil)
        check("macOS package has a non-empty download URL", macos.map { !$0.downloadUrl.isEmpty } ?? false)
    } catch {
        check("packages(BurntSushi/ripgrep) succeeded", false, detail: "\(error)")
    }
    do {
        let metadata = try await client.metadata(owner: "BurntSushi", repo: "ripgrep")
        check("stars > 0", metadata.stars > 0)
        check("has a description", (metadata.description ?? "").isEmpty == false)
    } catch {
        check("metadata(BurntSushi/ripgrep) succeeded", false, detail: "\(error)")
    }

    section("restic/restic — ambiguous keyword-only naming")
    do {
        let packages = try await client.packages(owner: "restic", repo: "restic")
        let macos = packages.first { $0.classification.platform == .macOS }
        check("resolves a macOS package via keyword fallback", macos != nil)
    } catch {
        check("packages(restic/restic) succeeded", false, detail: "\(error)")
    }

    section("neovim/neovim — no published checksums")
    do {
        let packages = try await client.packages(owner: "neovim", repo: "neovim")
        let macos = packages.first { $0.classification.platform == .macOS }
        check("resolves a macOS package", macos != nil)
        check("macOS package has no sha256 (none published upstream)", macos?.sha256 == nil)
    } catch {
        check("packages(neovim/neovim) succeeded", false, detail: "\(error)")
    }

    section("helix-editor/helix — source archive must stay unclassified")
    do {
        let packages = try await client.packages(owner: "helix-editor", repo: "helix")
        let source = packages.first { $0.assetName.contains("source") }
        check(
            "source archive has no platform classification",
            source.map { $0.classification.platform == nil } ?? true,
            detail: source == nil ? "no source asset found in this release" : ""
        )
    } catch {
        check("packages(helix-editor/helix) succeeded", false, detail: "\(error)")
    }

    section("Install lifecycle — safe path only (nonexistent repo)")
    do {
        let installId = try await client.startInstall(
            owner: "genjux-store-e2e-validation",
            repo: "this-repo-does-not-exist-12345"
        )
        check("received an install id", !installId.isEmpty)

        var finalStage: InstallStage?
        for _ in 0..<50 {
            let stage = try await client.installStatus(id: installId)
            if stage.isTerminal {
                finalStage = stage
                break
            }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        if case .failed = finalStage {
            check("nonexistent repo install reaches Failed", true)
        } else {
            check("nonexistent repo install reaches Failed", false, detail: "got \(String(describing: finalStage))")
        }
    } catch {
        check("install lifecycle completed without a transport error", false, detail: "\(error)")
    }

    section("Installed / updates — fresh registry")
    do {
        let installed = try await client.installed()
        check("installed list is empty on a fresh registry", installed.isEmpty)
    } catch {
        check("installed() succeeded", false, detail: "\(error)")
    }
    do {
        let updates = try await client.updates()
        check("updates list is empty on a fresh registry", updates.isEmpty)
    } catch {
        check("updates() succeeded", false, detail: "\(error)")
    }

    print("\n\(checksRun) checks run, \(failures.count) failed.")

    // This script deliberately uses its own isolated GENJUX_RUNTIME_DIR
    // (see the usage comment above), so the genjuxd it lazily started only
    // affects that throwaway registry -- but it's still a real background
    // process left running otherwise (by design: genjuxd is meant to
    // persist across CLI/GUI sessions). Clean it up so repeat local runs
    // of this script don't accumulate orphaned instances, mirroring
    // CoreServiceClientTests.swift's tearDown.
    cleanUpSpawnedGenjuxd()

    if !failures.isEmpty {
        print("\nFailures:")
        for f in failures {
            print("  - \(f)")
        }
        exit(1)
    }
    exit(0)
}

func cleanUpSpawnedGenjuxd() {
    let infoPath = ServiceLifecycle.runtimeDirectory().appendingPathComponent("genjuxd.json")
    guard let data = try? Data(contentsOf: infoPath),
          let info = try? JSONDecoder().decode(ServiceInfo.self, from: data) else {
        return
    }
    kill(info.pid, SIGKILL)
}

@main
struct E2EValidate {
    static func main() async {
        await run()
    }
}
