import Foundation

/// Swift mirrors of the core service's classification types
/// (core/src/classify.rs, core/src/package.rs) and the discovery
/// pipeline's output (core/src/discovery.rs, #54/#55). These are decoded
/// directly from the real JSON the HTTP API returns — enum raw values
/// match Rust's plain (un-renamed) `#[derive(Serialize)]` variant names
/// exactly (e.g. `"MacOS"`, `"X86_64"`), and struct properties are named
/// so `JSONDecoder`'s `.convertFromSnakeCase` (set in
/// `CoreServiceClient.get`) maps the wire's snake_case keys onto them
/// (e.g. `download_url` -> `downloadUrl`).

enum Platform: String, Codable, Equatable {
    case macOS = "MacOS"
    case windows = "Windows"
    case linux = "Linux"
    case android = "Android"
}

enum Arch: String, Codable, Equatable {
    case x86_64 = "X86_64"
    case arm64 = "Arm64"
}

enum AssetKind: String, Codable, Equatable {
    case dmg = "Dmg"
    case pkg = "Pkg"
    case macAppZip = "MacAppZip"
    case exe = "Exe"
    case msi = "Msi"
    case appx = "Appx"
    case appImage = "AppImage"
    case deb = "Deb"
    case rpm = "Rpm"
    case apk = "Apk"
    case archive = "Archive"
}

/// Mirrors `genjux_core::classify::Classification`.
struct Classification: Codable, Equatable {
    let platform: Platform?
    let arch: Arch?
    let kind: AssetKind?
}

/// Mirrors `genjux_core::package::InstallablePackage`.
struct InstallablePackage: Codable, Equatable {
    let assetName: String
    let downloadUrl: String
    let sizeBytes: UInt64
    let classification: Classification
    let sha256: String?
    let minOsVersion: String?
    let silentInstallArgs: String?
}

/// Mirrors `genjux_core::discovery::RecommendedApp` (#54/#55) — one entry
/// in the Home screen's recommended-apps feed (#60).
struct RecommendedApp: Codable, Equatable, Identifiable {
    let owner: String
    let repo: String
    let stars: UInt64
    let description: String?
    let releaseTag: String
    let package: InstallablePackage

    var id: String { "\(owner)/\(repo)" }
}

/// Mirrors `genjux_core::source::github::RepoMetadata` (#57) — README
/// excerpt/star count/last-release date for the App detail screen (#62).
struct RepoMetadata: Codable, Equatable {
    let stars: UInt64
    let description: String?
    let lastReleaseAt: String?
    let readmeExcerpt: String?
}

/// Mirrors `genjux_core::orchestrate::InstallStage` (#11) — one stage of
/// an in-progress (or finished) install, polled by the Install progress
/// screen (#63). The Rust enum is `#[serde(tag = "stage")]`
/// (internally-tagged: `{"stage": "Resolving"}` for unit variants,
/// `{"stage": "Downloading", "bytes_downloaded": ..., ...}` for variants
/// with fields) — Swift's auto-synthesized `Codable` for enums with
/// associated values doesn't produce that shape, so this has a hand-written
/// `init(from:)` instead of relying on synthesis.
enum InstallStage: Equatable {
    case resolving
    case downloading(bytesDownloaded: UInt64, totalBytes: UInt64?)
    /// `matchedPublishedChecksum` is `false` when the core only had a
    /// self-computed hash to show the user (no official checksum existed
    /// to compare against) — per the trust model in PLAN.md section 5,
    /// that's a fact to disclose, not something to gloss over as success.
    case verified(sha256: String, matchedPublishedChecksum: Bool)
    case installing
    case succeeded
    case failed(reason: String)

    var isTerminal: Bool {
        switch self {
        case .succeeded, .failed:
            return true
        case .resolving, .downloading, .verified, .installing:
            return false
        }
    }
}

extension InstallStage: Decodable {
    private enum CodingKeys: String, CodingKey {
        case stage
        case bytesDownloaded = "bytes_downloaded"
        case totalBytes = "total_bytes"
        case sha256
        case matchedPublishedChecksum = "matched_published_checksum"
        case reason
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let stage = try container.decode(String.self, forKey: .stage)
        switch stage {
        case "Resolving":
            self = .resolving
        case "Downloading":
            self = .downloading(
                bytesDownloaded: try container.decode(UInt64.self, forKey: .bytesDownloaded),
                totalBytes: try container.decodeIfPresent(UInt64.self, forKey: .totalBytes)
            )
        case "Verified":
            self = .verified(
                sha256: try container.decode(String.self, forKey: .sha256),
                matchedPublishedChecksum: try container.decode(Bool.self, forKey: .matchedPublishedChecksum)
            )
        case "Installing":
            self = .installing
        case "Succeeded":
            self = .succeeded
        case "Failed":
            self = .failed(reason: try container.decode(String.self, forKey: .reason))
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .stage,
                in: container,
                debugDescription: "unrecognized install stage: \(stage)"
            )
        }
    }
}

/// Mirrors `core::api::InstallStarted` (core/src/api.rs).
struct InstallStarted: Decodable {
    let installId: String
}

/// Mirrors `genjux_core::source::RepoRef`.
struct RepoRef: Codable, Equatable {
    let provider: String
    let owner: String
    let repo: String
}

/// Mirrors `genjux_core::registry::InstalledEntry` (#15) — one row in the
/// Installed/updates screen (#64).
struct InstalledEntry: Codable, Equatable, Identifiable {
    let repo: RepoRef
    let installedTag: String
    /// Unix epoch seconds, as the Rust side stores it (no date/time
    /// crate pulled in there either — see registry.rs's own comment).
    let installedAtUnix: UInt64
    let sourceUrl: String

    var id: String { "\(repo.provider)/\(repo.owner)/\(repo.repo)" }
}

/// Mirrors `genjux_core::registry::UpdateCheckResult` (#15).
struct UpdateCheckResult: Codable, Equatable {
    let repo: RepoRef
    let installedTag: String
    let latestTag: String
    let updateAvailable: Bool
}
