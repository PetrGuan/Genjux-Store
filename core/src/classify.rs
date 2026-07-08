//! Release-asset classification pipeline.
//!
//! Given a release asset's filename (and, as a last resort, a few bytes of
//! its actual content), figures out which platform / architecture / package
//! kind it's for. Implements the tiered fallback pipeline from
//! `.copilot-workflow/PLAN.md` section 3:
//!
//! 1. [`classify_by_extension`] — extension-based mapping (issue #4).
//! 2. [`refine_by_keywords`] — filename keyword + arch-token fallback,
//!    only used when tier 1 left the platform unknown (issue #5).
//! 3. [`sniff`] / [`sniff_remote_asset_platform`] — magic-byte content
//!    sniffing, only used when tiers 1-2 still couldn't tell (issue #6).
//!
//! The pipeline never guesses past what the evidence supports: an asset
//! that can't be classified comes back with `platform: None` rather than a
//! wrong guess (callers should treat that as "unclassified", not an error).

use crate::source::ReleaseAsset;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    MacOS,
    Windows,
    Linux,
    Android,
}

impl Platform {
    /// Stable lowercase identifier for this platform (e.g. for use as an
    /// HTTP path segment or cache key) — the inverse of [`Platform::from_str`].
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::MacOS => "macos",
            Platform::Windows => "windows",
            Platform::Linux => "linux",
            Platform::Android => "android",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unrecognized platform: {0:?}")]
pub struct ParsePlatformError(pub String);

impl std::str::FromStr for Platform {
    type Err = ParsePlatformError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "macos" => Ok(Platform::MacOS),
            "windows" => Ok(Platform::Windows),
            "linux" => Ok(Platform::Linux),
            "android" => Ok(Platform::Android),
            _ => Err(ParsePlatformError(s.to_string())),
        }
    }
}

/// The platform this build of Genjux-Store is running on, if it's one of
/// the four we support. Used by install orchestration (#11) to pick which
/// `InstallablePackage` from a release matches the current machine.
pub fn current_platform() -> Option<Platform> {
    if cfg!(target_os = "macos") {
        Some(Platform::MacOS)
    } else if cfg!(target_os = "windows") {
        Some(Platform::Windows)
    } else if cfg!(target_os = "linux") {
        Some(Platform::Linux)
    } else if cfg!(target_os = "android") {
        Some(Platform::Android)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Arch {
    X86_64,
    Arm64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetKind {
    Dmg,
    Pkg,
    MacAppZip,
    Exe,
    Msi,
    Appx,
    AppImage,
    Deb,
    Rpm,
    Apk,
    /// A generic archive (zip/tar.gz) whose *platform* was resolved by
    /// tier 2 or 3, but whose install mechanism isn't otherwise known from
    /// the filename alone.
    Archive,
}

/// Result of running the classification pipeline over a single asset. Any
/// field may be `None` if the pipeline couldn't determine it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Classification {
    pub platform: Option<Platform>,
    pub arch: Option<Arch>,
    pub kind: Option<AssetKind>,
}

impl Classification {
    fn platform_known(&self) -> bool {
        self.platform.is_some()
    }
}

fn extract_arch(lower_filename: &str) -> Option<Arch> {
    if lower_filename.contains("x86_64")
        || lower_filename.contains("amd64")
        || lower_filename.contains("x64")
    {
        Some(Arch::X86_64)
    } else if lower_filename.contains("arm64") || lower_filename.contains("aarch64") {
        Some(Arch::Arm64)
    } else {
        None
    }
}

/// Tier 1: extension-based platform/kind mapping (issue #4).
pub fn classify_by_extension(filename: &str) -> Classification {
    let lower = filename.to_lowercase();

    // Compound suffix checked first: a zipped macOS .app bundle is still a
    // .zip by plain extension, but ".app.zip" is unambiguous.
    if lower.ends_with(".app.zip") {
        return Classification {
            platform: Some(Platform::MacOS),
            arch: extract_arch(&lower),
            kind: Some(AssetKind::MacAppZip),
        };
    }

    let (kind, platform) = if lower.ends_with(".dmg") {
        (AssetKind::Dmg, Platform::MacOS)
    } else if lower.ends_with(".pkg") {
        (AssetKind::Pkg, Platform::MacOS)
    } else if lower.ends_with(".msi") {
        (AssetKind::Msi, Platform::Windows)
    } else if lower.ends_with(".appx") || lower.ends_with(".msix") {
        (AssetKind::Appx, Platform::Windows)
    } else if lower.ends_with(".exe") {
        (AssetKind::Exe, Platform::Windows)
    } else if lower.ends_with(".appimage") {
        (AssetKind::AppImage, Platform::Linux)
    } else if lower.ends_with(".deb") {
        (AssetKind::Deb, Platform::Linux)
    } else if lower.ends_with(".rpm") {
        (AssetKind::Rpm, Platform::Linux)
    } else if lower.ends_with(".apk") {
        (AssetKind::Apk, Platform::Android)
    } else {
        return Classification::default();
    };

    Classification {
        platform: Some(platform),
        arch: extract_arch(&lower),
        kind: Some(kind),
    }
}

/// Tier 2: filename keyword + arch-token fallback (issue #5). Only fills in
/// what tier 1 left as `None` — never overrides a tier-1 result.
pub fn refine_by_keywords(filename: &str, mut current: Classification) -> Classification {
    if current.platform_known() {
        return current;
    }
    let lower = filename.to_lowercase();

    let platform = if lower.contains("darwin") || lower.contains("macos") || lower.contains("osx") {
        Some(Platform::MacOS)
    } else if lower.contains("windows") || lower.contains("win64") || lower.contains("win32") {
        Some(Platform::Windows)
    } else if lower.contains("android") {
        // Must be checked before the plain "linux" match below: real
        // Android target triples (aarch64-linux-android,
        // armv7-linux-androideabi, ...) routinely contain "linux" as a
        // substring, and would otherwise be misclassified as Linux (see
        // https://github.com/PetrGuan/Genjux-Store/issues/51, found via
        // the #21 real-repo validation harness against a real
        // ClementTsang/bottom release asset).
        Some(Platform::Android)
    } else if lower.contains("linux") {
        Some(Platform::Linux)
    } else {
        None
    };

    let Some(platform) = platform else {
        return current;
    };

    current.platform = Some(platform);
    current.arch = current.arch.or_else(|| extract_arch(&lower));
    if current.kind.is_none()
        && (lower.ends_with(".zip") || lower.ends_with(".tar.gz") || lower.ends_with(".tgz"))
    {
        current.kind = Some(AssetKind::Archive);
    }
    current
}

/// Runs tiers 1+2 (filename-only, no network) over a single asset.
pub fn classify_asset_by_filename(asset: &ReleaseAsset) -> Classification {
    let tier1 = classify_by_extension(&asset.name);
    refine_by_keywords(&asset.name, tier1)
}

pub mod sniff {
    //! Tier 3 core logic: content-sniffing via magic bytes (issue #6),
    //! reached only when tiers 1-2 couldn't determine the platform from the
    //! filename alone. Pure/synchronous by design so it's trivially
    //! testable without any network dependency; see
    //! [`super::sniff_remote_asset_platform`] for the network-integrated
    //! version that fetches just enough bytes to run this.
    use super::Platform;

    /// Mach-O / fat-binary magic numbers (32/64-bit, both endiannesses).
    const MACHO_MAGICS: [[u8; 4]; 6] = [
        [0xFE, 0xED, 0xFA, 0xCE], // MH_MAGIC
        [0xCE, 0xFA, 0xED, 0xFE], // MH_CIGAM
        [0xFE, 0xED, 0xFA, 0xCF], // MH_MAGIC_64
        [0xCF, 0xFA, 0xED, 0xFE], // MH_CIGAM_64
        [0xCA, 0xFE, 0xBA, 0xBE], // FAT_MAGIC (universal binary)
        [0xBE, 0xBA, 0xFE, 0xCA], // FAT_CIGAM
    ];

    /// Detects a platform from the first bytes of a file, using well-known
    /// executable-format magic numbers. Returns `None` (never guesses) if
    /// nothing recognizable matches.
    pub fn platform_from_magic_bytes(bytes: &[u8]) -> Option<Platform> {
        if bytes.len() >= 4 && MACHO_MAGICS.iter().any(|m| bytes[..4] == *m) {
            return Some(Platform::MacOS);
        }
        if bytes.len() >= 2 && &bytes[..2] == b"MZ" {
            return Some(Platform::Windows);
        }
        if bytes.len() >= 4 && &bytes[..4] == b"\x7FELF" {
            return Some(Platform::Linux);
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn detects_macho_64bit() {
            assert_eq!(
                platform_from_magic_bytes(&[0xFE, 0xED, 0xFA, 0xCF, 0, 0]),
                Some(Platform::MacOS)
            );
        }

        #[test]
        fn detects_macho_fat_universal_binary() {
            assert_eq!(
                platform_from_magic_bytes(&[0xCA, 0xFE, 0xBA, 0xBE, 0, 0]),
                Some(Platform::MacOS)
            );
        }

        #[test]
        fn detects_pe_exe() {
            assert_eq!(
                platform_from_magic_bytes(b"MZ\x90\x00"),
                Some(Platform::Windows)
            );
        }

        #[test]
        fn detects_elf() {
            assert_eq!(
                platform_from_magic_bytes(b"\x7FELF\x02\x01"),
                Some(Platform::Linux)
            );
        }

        #[test]
        fn unrecognized_bytes_return_none_rather_than_guessing() {
            // A plain zip's own magic bytes ("PK\x03\x04") aren't an
            // executable-format signature we recognize.
            assert_eq!(platform_from_magic_bytes(b"PK\x03\x04"), None);
        }

        #[test]
        fn too_short_to_tell_returns_none() {
            assert_eq!(platform_from_magic_bytes(&[0xFE]), None);
        }
    }
}

/// Tier 3, network-integrated half: fetches just enough bytes of a remote
/// asset (via an HTTP `Range` request) to run
/// [`sniff::platform_from_magic_bytes`], without downloading the whole
/// file.
pub async fn sniff_remote_asset_platform(
    client: &reqwest::Client,
    url: &str,
) -> Result<Option<Platform>, crate::source::SourceError> {
    // 512 bytes comfortably covers every magic number we check for.
    const PROBE_RANGE: &str = "bytes=0-511";

    let response = client
        .get(url)
        .header(reqwest::header::RANGE, PROBE_RANGE)
        .send()
        .await
        .map_err(|e| crate::source::SourceError::Network {
            provider: "http",
            message: e.to_string(),
        })?;

    let status = response.status();
    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(crate::source::SourceError::Provider {
            provider: "http",
            message: format!("unexpected status probing asset bytes: {status}"),
        });
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| crate::source::SourceError::Network {
            provider: "http",
            message: e.to_string(),
        })?;

    Ok(sniff::platform_from_magic_bytes(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            size_bytes: 0,
            download_url: String::new(),
            content_type: None,
        }
    }

    /// Table-driven coverage for tier 1 across real-world-shaped filenames
    /// for every platform (issue #4 acceptance criteria).
    #[test]
    fn tier1_classifies_known_extensions_across_platforms() {
        let cases: &[(&str, Platform, AssetKind, Option<Arch>)] = &[
            (
                "MyApp-1.2.0-arm64.dmg",
                Platform::MacOS,
                AssetKind::Dmg,
                Some(Arch::Arm64),
            ),
            (
                "MyApp-1.2.0-x86_64.dmg",
                Platform::MacOS,
                AssetKind::Dmg,
                Some(Arch::X86_64),
            ),
            ("MyApp-Installer.pkg", Platform::MacOS, AssetKind::Pkg, None),
            (
                "MyApp-macos.app.zip",
                Platform::MacOS,
                AssetKind::MacAppZip,
                None,
            ),
            (
                "myapp-setup-x64.exe",
                Platform::Windows,
                AssetKind::Exe,
                Some(Arch::X86_64),
            ),
            (
                "myapp-1.2.0-win64.msi",
                Platform::Windows,
                AssetKind::Msi,
                None,
            ),
            ("MyApp.appx", Platform::Windows, AssetKind::Appx, None),
            ("MyApp.msix", Platform::Windows, AssetKind::Appx, None),
            (
                "myapp-x86_64.AppImage",
                Platform::Linux,
                AssetKind::AppImage,
                Some(Arch::X86_64),
            ),
            (
                "myapp_1.2.0_amd64.deb",
                Platform::Linux,
                AssetKind::Deb,
                Some(Arch::X86_64),
            ),
            (
                "myapp-1.2.0.aarch64.rpm",
                Platform::Linux,
                AssetKind::Rpm,
                Some(Arch::Arm64),
            ),
            (
                "app-release-arm64-v8a.apk",
                Platform::Android,
                AssetKind::Apk,
                Some(Arch::Arm64),
            ),
        ];

        for (filename, expected_platform, expected_kind, expected_arch) in cases {
            let c = classify_by_extension(filename);
            assert_eq!(
                c.platform,
                Some(*expected_platform),
                "platform for {filename}"
            );
            assert_eq!(c.kind, Some(*expected_kind), "kind for {filename}");
            assert_eq!(c.arch, *expected_arch, "arch for {filename}");
        }
    }

    #[test]
    fn tier1_returns_unclassified_for_generic_archive_extensions() {
        for filename in ["release.zip", "release.tar.gz", "release.tgz"] {
            let c = classify_by_extension(filename);
            assert_eq!(c.platform, None, "{filename} should fall through to tier 2");
        }
    }

    #[test]
    fn tier2_resolves_ambiguous_zip_via_darwin_keyword_and_arch_token() {
        let c = classify_asset_by_filename(&asset("myapp_darwin_arm64.zip"));
        assert_eq!(c.platform, Some(Platform::MacOS));
        assert_eq!(c.arch, Some(Arch::Arm64));
        assert_eq!(c.kind, Some(AssetKind::Archive));
    }

    #[test]
    fn tier2_resolves_generic_linux_tarball_target_triple() {
        let c = classify_asset_by_filename(&asset("myapp-v1.2.0-x86_64-unknown-linux-gnu.tar.gz"));
        assert_eq!(c.platform, Some(Platform::Linux));
        assert_eq!(c.arch, Some(Arch::X86_64));
    }

    #[test]
    fn tier2_resolves_android_target_triples_containing_linux_as_android_not_linux() {
        // Regression test for #51: real Android target triples routinely
        // contain "linux" as a substring, and must not be misclassified
        // as Linux.
        for filename in [
            "bottom_aarch64-linux-android.tar.gz",
            "myapp-armv7-linux-androideabi.tar.gz",
            "myapp-x86_64-linux-android.tar.gz",
        ] {
            let c = classify_asset_by_filename(&asset(filename));
            assert_eq!(
                c.platform,
                Some(Platform::Android),
                "platform for {filename}"
            );
        }
    }

    #[test]
    fn tier1_result_is_never_overridden_by_tier2() {
        // No OS keyword in the filename at all — tier 1's extension match
        // must still be authoritative and untouched by tier 2.
        let c = classify_asset_by_filename(&asset("setup.exe"));
        assert_eq!(c.platform, Some(Platform::Windows));
        assert_eq!(c.kind, Some(AssetKind::Exe));
    }

    #[test]
    fn fully_ambiguous_filename_stays_unclassified_rather_than_guessing() {
        let c = classify_asset_by_filename(&asset("release-1.2.0.zip"));
        assert_eq!(c.platform, None);
    }

    #[test]
    fn current_platform_matches_the_os_running_this_test() {
        let detected = current_platform();
        assert_eq!(
            detected.is_some(),
            cfg!(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "linux",
                target_os = "android"
            ))
        );
    }

    #[test]
    fn platform_as_str_and_from_str_round_trip() {
        for platform in [
            Platform::MacOS,
            Platform::Windows,
            Platform::Linux,
            Platform::Android,
        ] {
            let parsed: Platform = platform.as_str().parse().unwrap();
            assert_eq!(parsed, platform);
        }
    }

    #[test]
    fn platform_from_str_is_case_insensitive() {
        assert_eq!("MacOS".parse::<Platform>().unwrap(), Platform::MacOS);
        assert_eq!("MACOS".parse::<Platform>().unwrap(), Platform::MacOS);
    }

    #[test]
    fn platform_from_str_rejects_unknown_values() {
        assert!("plan9".parse::<Platform>().is_err());
    }
}

#[cfg(test)]
mod remote_sniff_tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn sniffs_platform_via_range_request_without_downloading_whole_file() {
        let server = MockServer::start().await;
        // Requiring the Range header to match means this mock only
        // responds if `sniff_remote_asset_platform` actually sent a
        // partial-content request — proving it doesn't fetch the whole
        // asset, not just asserting the returned platform.
        Mock::given(method("GET"))
            .and(path("/myapp"))
            .and(header("range", "bytes=0-511"))
            .respond_with(
                ResponseTemplate::new(206)
                    .set_body_bytes([0x7F, b'E', b'L', b'F', 0x02, 0x01].as_slice()),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/myapp", server.uri());
        let platform = sniff_remote_asset_platform(&client, &url).await.unwrap();

        assert_eq!(platform, Some(Platform::Linux));
    }

    #[tokio::test]
    async fn unrecognized_remote_bytes_return_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/myapp"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(b"not an executable".as_slice()),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/myapp", server.uri());
        let platform = sniff_remote_asset_platform(&client, &url).await.unwrap();

        assert_eq!(platform, None);
    }
}
