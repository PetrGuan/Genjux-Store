import XCTest
@testable import GenjuxStore

/// Pure unit tests for `InstallStage`'s hand-written `Decodable`
/// implementation (Models.swift) against JSON shaped exactly like the
/// real `genjux_core::orchestrate::InstallStage`'s
/// `#[serde(tag = "stage")]` (internally-tagged) serialization — no
/// network or `genjuxd` dependency, so these run as part of the default
/// test suite and stay fast/deterministic.
///
/// Deliberately does *not* test the happy path (Downloading -> Verified
/// -> Installing -> Succeeded) via a real install: the real macOS
/// adapter's `.pkg` path runs the actual system `installer` command
/// (core/src/platform/macos.rs), which is a real, if scoped, system
/// action not worth risking in an automated test. See
/// `CoreServiceClientTests.testStartInstallForANonexistentRepoReachesFailedStage`
/// for the real-network round-trip test, which only ever reaches the
/// safe `Resolving -> Failed` path.
final class InstallStageTests: XCTestCase {
    private func decode(_ json: String) throws -> InstallStage {
        try JSONDecoder().decode(InstallStage.self, from: Data(json.utf8))
    }

    func testDecodesResolving() throws {
        XCTAssertEqual(try decode(#"{"stage":"Resolving"}"#), .resolving)
    }

    func testDecodesDownloadingWithKnownTotal() throws {
        let stage = try decode(#"{"stage":"Downloading","bytes_downloaded":512,"total_bytes":1024}"#)
        XCTAssertEqual(stage, .downloading(bytesDownloaded: 512, totalBytes: 1024))
    }

    func testDecodesDownloadingWithUnknownTotal() throws {
        // total_bytes is an Option<u64> on the Rust side and can be
        // absent (e.g. no Content-Length header from the source).
        let stage = try decode(#"{"stage":"Downloading","bytes_downloaded":512,"total_bytes":null}"#)
        XCTAssertEqual(stage, .downloading(bytesDownloaded: 512, totalBytes: nil))
    }

    func testDecodesVerifiedMatchingPublishedChecksum() throws {
        let stage = try decode(
            #"{"stage":"Verified","sha256":"abc123","matched_published_checksum":true}"#
        )
        XCTAssertEqual(stage, .verified(sha256: "abc123", matchedPublishedChecksum: true))
    }

    func testDecodesVerifiedWithNoPublishedChecksumToCompare() throws {
        let stage = try decode(
            #"{"stage":"Verified","sha256":"abc123","matched_published_checksum":false}"#
        )
        XCTAssertEqual(stage, .verified(sha256: "abc123", matchedPublishedChecksum: false))
    }

    func testDecodesInstalling() throws {
        XCTAssertEqual(try decode(#"{"stage":"Installing"}"#), .installing)
    }

    func testDecodesSucceeded() throws {
        XCTAssertEqual(try decode(#"{"stage":"Succeeded"}"#), .succeeded)
        XCTAssertTrue(InstallStage.succeeded.isTerminal)
    }

    func testDecodesFailedWithReason() throws {
        let stage = try decode(#"{"stage":"Failed","reason":"checksum mismatch"}"#)
        XCTAssertEqual(stage, .failed(reason: "checksum mismatch"))
        XCTAssertTrue(stage.isTerminal)
    }

    func testNonTerminalStagesAreNotTerminal() {
        XCTAssertFalse(InstallStage.resolving.isTerminal)
        XCTAssertFalse(InstallStage.downloading(bytesDownloaded: 0, totalBytes: nil).isTerminal)
        XCTAssertFalse(InstallStage.verified(sha256: "x", matchedPublishedChecksum: true).isTerminal)
        XCTAssertFalse(InstallStage.installing.isTerminal)
    }

    func testUnrecognizedStageThrowsRatherThanSilentlyMisdecoding() {
        XCTAssertThrowsError(try decode(#"{"stage":"SomeFutureStage"}"#))
    }
}
