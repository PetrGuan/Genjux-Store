import XCTest
@testable import GenjuxStore

/// Real end-to-end test against the actual `genjuxd` binary (built via
/// `cargo build --bin genjuxd` in the Rust workspace) — mirrors the
/// rigor of the CLI's own e2e test (cli/tests/cli_e2e.rs, issue #20):
/// spawn the real process, confirm the whole lazy-start -> discovery ->
/// authenticated HTTP round trip actually works, not just that the
/// individual pieces compile.
///
/// Skips (rather than fails) if the binary hasn't been built yet, so a
/// contributor who hasn't touched the Rust side isn't blocked — but runs
/// for real, against the real service, whenever it has been.
final class CoreServiceClientTests: XCTestCase {
    private var runtimeDir: URL!

    private static func repoRoot() -> URL {
        // #filePath is this source file's absolute path at compile time:
        // .../macos/GenjuxStoreTests/CoreServiceClientTests.swift
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // -> GenjuxStoreTests/
            .deletingLastPathComponent() // -> macos/
            .deletingLastPathComponent() // -> repo root
    }

    private static func genjuxdBinaryPath() -> String {
        repoRoot().appendingPathComponent("target/debug/genjuxd").path
    }

    override func setUpWithError() throws {
        let genjuxdPath = Self.genjuxdBinaryPath()
        guard FileManager.default.fileExists(atPath: genjuxdPath) else {
            throw XCTSkip(
                "genjuxd binary not found at \(genjuxdPath) — run `cargo build --bin genjuxd` in the repo root first"
            )
        }

        runtimeDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("genjux-swift-test-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: runtimeDir, withIntermediateDirectories: true)

        setenv("GENJUX_RUNTIME_DIR", runtimeDir.path, 1)
        setenv("GENJUX_GENJUXD_PATH", genjuxdPath, 1)
    }

    override func tearDownWithError() throws {
        defer {
            unsetenv("GENJUX_RUNTIME_DIR")
            unsetenv("GENJUX_GENJUXD_PATH")
            try? FileManager.default.removeItem(at: runtimeDir)
        }

        // Kill whatever genjuxd this test lazily started -- mirrors the
        // Rust CLI e2e test's manual cleanup. Unlike that Rust test (which
        // later added a Drop-guard after a real Windows CI hang), this
        // runs unconditionally in tearDown regardless of whether the test
        // body threw, which XCTest guarantees -- so there's no equivalent
        // "cleanup skipped because an assertion panicked first" gap here.
        let infoPath = runtimeDir.appendingPathComponent("genjuxd.json")
        if let data = try? Data(contentsOf: infoPath),
           let info = try? JSONDecoder().decode(ServiceInfo.self, from: data) {
            kill(info.pid, SIGKILL)
        }
    }

    func testEnsureServiceRunningLazilyStartsGenjuxdAndHealthCheckSucceeds() async throws {
        let client = CoreServiceClient.makeForTesting()

        let health = try await client.health()

        XCTAssertEqual(health.status, "ok")
        XCTAssertFalse(health.version.isEmpty)

        // The lazy-start should have actually published a real discovery
        // file with a real running pid.
        let infoPath = runtimeDir.appendingPathComponent("genjuxd.json")
        let info = try JSONDecoder().decode(ServiceInfo.self, from: Data(contentsOf: infoPath))
        XCTAssertGreaterThan(info.pid, 0)
        XCTAssertGreaterThan(info.port, 0)
    }

    func testSecondClientReusesTheAlreadyRunningInstanceRatherThanSpawningANewOne() async throws {
        let first = CoreServiceClient.makeForTesting()
        _ = try await first.health()

        let infoPath = runtimeDir.appendingPathComponent("genjuxd.json")
        let firstInfo = try JSONDecoder().decode(ServiceInfo.self, from: Data(contentsOf: infoPath))

        let second = CoreServiceClient.makeForTesting()
        _ = try await second.health()
        let secondInfo = try JSONDecoder().decode(ServiceInfo.self, from: Data(contentsOf: infoPath))

        XCTAssertEqual(
            firstInfo.pid, secondInfo.pid,
            "a second client should discover and reuse the same genjuxd process, not spawn a duplicate"
        )
    }

    func testUnauthenticatedRequestIsRejected() async throws {
        let client = CoreServiceClient.makeForTesting()
        _ = try await client.health() // lazily start genjuxd first

        let infoPath = runtimeDir.appendingPathComponent("genjuxd.json")
        let info = try JSONDecoder().decode(ServiceInfo.self, from: Data(contentsOf: infoPath))

        var request = URLRequest(url: URL(string: "http://127.0.0.1:\(info.port)/health")!)
        request.setValue("Bearer not-the-real-token", forHTTPHeaderField: "Authorization")
        let (_, response) = try await URLSession.shared.data(for: request)

        let httpResponse = try XCTUnwrap(response as? HTTPURLResponse)
        XCTAssertEqual(httpResponse.statusCode, 401)
    }

    func testPackagesLookupReturnsClassifiedAssetsForARealRepo() async throws {
        // Hits the real GitHub API (via the real genjuxd) for a
        // well-known repo with real macOS/Windows/Linux release assets —
        // same rigor as the Rust side's own real-repo verification
        // (core/tests/e2e_real_repos.rs, #21).
        let client = CoreServiceClient.makeForTesting()

        let packages = try await client.packages(owner: "cli", repo: "cli")

        XCTAssertFalse(packages.isEmpty, "expected cli/cli to have real release assets")
        XCTAssertTrue(
            packages.contains { $0.classification.platform == .macOS },
            "expected at least one real macOS asset among cli/cli's releases"
        )
    }

    func testPackagesLookupForARepoWithNoReleasesThrowsNotFound() async throws {
        let client = CoreServiceClient.makeForTesting()

        do {
            // A real, extremely unlikely-to-exist repo name -- GitHub
            // returns 404 for both "repo doesn't exist" and "repo has no
            // releases" from this endpoint, which is exactly the case
            // isNotFound is meant to capture.
            _ = try await client.packages(owner: "genjux-store-test-fixture", repo: "does-not-exist-12345")
            XCTFail("expected a not-found error for a repo that doesn't exist")
        } catch let error as CoreServiceError {
            // The core service's GET /repos/:owner/:repo/packages handler
            // (core/src/api.rs) maps a GitHub rate-limit error to the same
            // 502 as any other unexpected error (a real, pre-existing gap
            // from #16, not introduced here) -- and this specific
            // unauthenticated call can genuinely get rate limited after
            // heavy local test iteration in the same hour (observed for
            // real while developing this test). Treat that as
            // inconclusive rather than a false failure; only a real
            // "not found" or a genuinely unexpected error should fail
            // this test.
            if case .httpError(let status, let body) = error, status == 502, body.contains("rate limited") {
                throw XCTSkip("hit a real GitHub API rate limit -- inconclusive, not a real failure: \(body)")
            }
            XCTAssertTrue(error.isNotFound, "expected isNotFound to be true, got \(error)")
        }
    }
}
