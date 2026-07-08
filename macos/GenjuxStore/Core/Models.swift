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
