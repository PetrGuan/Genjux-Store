import Foundation
#if canImport(Darwin)
import Darwin
#endif

/// Locates, lazily starts, and confirms readiness of the `genjuxd` core
/// service — the Swift-side counterpart to the CLI's
/// `ensure_service_running` (cli/src/main.rs, issue #20), reusing the
/// exact same on-disk singleton-lock/discovery-file protocol
/// (core/src/lifecycle.rs, issue #18) so a Swift GUI client and a Rust
/// CLI client correctly interoperate against the same running instance.
enum ServiceLifecycle {
    enum LifecycleError: Error, LocalizedError {
        case genjuxdNotFound(searched: [String])
        case failedToSpawn(String)
        case timedOutWaitingForReadiness

        var errorDescription: String? {
            switch self {
            case .genjuxdNotFound(let searched):
                return "could not find the genjuxd binary (looked in: \(searched.joined(separator: ", ")))"
            case .failedToSpawn(let reason):
                return "failed to start genjuxd: \(reason)"
            case .timedOutWaitingForReadiness:
                return "timed out waiting for the core service to start"
            }
        }
    }

    /// Where `genjuxd`'s lock/discovery file and other per-user runtime
    /// state lives. Mirrors `genjux_core::lifecycle::runtime_dir()`
    /// exactly: honors `GENJUX_RUNTIME_DIR` first (so tests can redirect
    /// it, matching the Rust side's own test convention), else
    /// `~/Library/Application Support/genjux` on macOS — empirically
    /// confirmed to match `dirs::data_local_dir()`'s real behavior on
    /// macOS by running the actual `genjuxd` binary and observing where
    /// it wrote its lock/info files, not just assumed from documentation.
    static func runtimeDirectory() -> URL {
        if let override = ProcessInfo.processInfo.environment["GENJUX_RUNTIME_DIR"], !override.isEmpty {
            return URL(fileURLWithPath: override, isDirectory: true)
        }
        let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return appSupport.appendingPathComponent("genjux", isDirectory: true)
    }

    static func lockFilePath() -> URL {
        runtimeDirectory().appendingPathComponent("genjuxd.lock")
    }

    static func infoFilePath() -> URL {
        runtimeDirectory().appendingPathComponent("genjuxd.json")
    }

    /// Ensures a `genjuxd` instance is running and reachable, returning
    /// its published [`ServiceInfo`]. Reuses an already-running instance
    /// (discovered via the same lock file `genjuxd` itself uses) rather
    /// than starting a duplicate; otherwise spawns a fresh one and waits
    /// for it to publish its info file.
    static func ensureServiceRunning() async throws -> ServiceInfo {
        if try acquireOrDetectRunning() {
            // We acquired the lock ourselves, meaning nobody was running.
            // Release it immediately (by simply not holding the fd open
            // past this function) so the genjuxd we're about to spawn can
            // acquire it itself.
            try spawnGenjuxd()
        }
        return try await waitForServiceInfo()
    }

    /// Attempts to exclusively lock `genjuxd.lock` (non-blocking).
    /// Returns `true` if *we* acquired it (meaning no `genjuxd` currently
    /// holds it), `false` if it's already held by a running instance.
    ///
    /// Uses the plain BSD `flock(2)` syscall — deliberately the same
    /// primitive `genjuxd`'s own lock uses (verified: the Rust side's
    /// `fs4` crate calls `flock()`, not `fcntl`/`F_SETLK`, on Unix — see
    /// `fs4-0.9.1/src/unix.rs`). `flock` and `fcntl` locks are separate,
    /// non-interacting lock domains on most Unix systems including
    /// macOS, so using anything other than `flock` here would silently
    /// fail to detect a real running instance.
    private static func acquireOrDetectRunning() throws -> Bool {
        let dir = runtimeDirectory()
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

        let path = lockFilePath().path
        let fd = open(path, O_RDWR | O_CREAT, 0o644)
        guard fd >= 0 else {
            throw LifecycleError.failedToSpawn("could not open lock file at \(path): errno \(errno)")
        }
        defer { close(fd) }

        let result = flock(fd, LOCK_EX | LOCK_NB)
        if result == 0 {
            // We hold the lock now; release it right away (closing fd
            // above does this too, but explicit unlock makes the intent
            // clear and doesn't depend on `defer` ordering).
            flock(fd, LOCK_UN)
            return true
        }
        // EWOULDBLOCK means another process already holds it.
        return false
    }

    /// Searches for the `genjuxd` binary. Checks (in order): an explicit
    /// `GENJUX_GENJUXD_PATH` override (for local dev/tests, mirroring how
    /// `CARGO_BIN_EXE_<name>` is used on the Rust side), then inside the
    /// app bundle itself (`Contents/MacOS/genjuxd` /
    /// `Contents/Resources/genjuxd`) for when it's actually embedded in a
    /// packaged build — packaging genjuxd into the .app bundle is a
    /// distribution-pipeline concern (#66), not solved here.
    private static func locateGenjuxd() throws -> URL {
        var searched: [String] = []

        if let override = ProcessInfo.processInfo.environment["GENJUX_GENJUXD_PATH"], !override.isEmpty {
            let url = URL(fileURLWithPath: override)
            if FileManager.default.fileExists(atPath: url.path) {
                return url
            }
            searched.append(url.path)
        }

        if let macOSDir = Bundle.main.executableURL?.deletingLastPathComponent() {
            let candidate = macOSDir.appendingPathComponent("genjuxd")
            if FileManager.default.fileExists(atPath: candidate.path) {
                return candidate
            }
            searched.append(candidate.path)
        }

        if let resourceURL = Bundle.main.url(forResource: "genjuxd", withExtension: nil) {
            if FileManager.default.fileExists(atPath: resourceURL.path) {
                return resourceURL
            }
            searched.append(resourceURL.path)
        }

        throw LifecycleError.genjuxdNotFound(searched: searched)
    }

    private static func spawnGenjuxd() throws {
        let genjuxdPath = try locateGenjuxd()

        let process = Process()
        process.executableURL = genjuxdPath
        // genjuxd is meant to keep running as a background service after
        // this process exits — do not inherit our stdio (the same
        // Stdio::null() fix the CLI needed in #20, for the same reason:
        // an inherited pipe held open by a long-lived background process
        // can hang anything waiting for that pipe's other end to reach
        // EOF).
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        // Process() otherwise inherits this process's *entire* environment
        // verbatim. When the spawning process is itself an Xcode-injected
        // XCTest host, that includes DYLD_*/__XPC_DYLD_* library-injection
        // hints and XCTest*/XPC_SERVICE_NAME bundle-context variables
        // pointing at libXCTestBundleInject.dylib etc. genjuxd is a plain
        // Rust binary with no Objective-C runtime for that injected dylib
        // to hook into, so there's no reason to pass any of it through --
        // stripping it is defensive hygiene regardless of the parent
        // process's context. (Note: a real launch-hang was independently
        // observed in this dev environment when spawning genjuxd from an
        // XCTest host specifically, isolated via `sample` to dyld's own
        // on-disk binary loading path -- but it reproduced identically
        // with or without these variables present, and did not reproduce
        // spawning the same binary from a plain, non-XCTest Swift process
        // or from a shell. That points to environment-specific process-launch
        // interference outside this codebase, not something this filter
        // fixes -- see the macOS README's "known environment issues" note.)
        process.environment = ProcessInfo.processInfo.environment.filter { key, _ in
            !key.contains("DYLD") && !key.hasPrefix("XCTest") && !key.hasPrefix("XPC_")
        }

        do {
            try process.run()
        } catch {
            throw LifecycleError.failedToSpawn(error.localizedDescription)
        }
    }

    private static func waitForServiceInfo() async throws -> ServiceInfo {
        let path = infoFilePath()
        let deadline = Date().addingTimeInterval(10)

        while Date() < deadline {
            if let data = try? Data(contentsOf: path),
               let info = try? JSONDecoder().decode(ServiceInfo.self, from: data) {
                return info
            }
            try await Task.sleep(nanoseconds: 50_000_000) // 50ms, matching the CLI's poll interval
        }
        throw LifecycleError.timedOutWaitingForReadiness
    }
}
