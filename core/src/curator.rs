//! Curator metadata overlay: manual, per-repo overrides for assets the
//! automatic classification pipeline ([`crate::classify`]) can't handle (or
//! gets wrong), plus custom install metadata (min OS version, silent-install
//! flags). This is tier 4 of the classification pipeline described in
//! `.copilot-workflow/PLAN.md` section 3 (issue #7).
//!
//! Schema is documented in `core/docs/genjux-yaml.md`.

use crate::classify::{Arch, Platform};
use crate::package::InstallablePackage;
use serde::Deserialize;
use std::collections::HashMap;

/// A parsed `genjux.yaml` overlay for a single repo.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct CuratorOverlay {
    /// Overrides keyed by the *exact* release-asset filename. Exact match
    /// (rather than a glob/regex) is the simplest option that satisfies
    /// Phase 0's needs; pattern-based matching can be layered on later if
    /// exact match proves too rigid for a curated repo whose asset names
    /// change every release (e.g. embed the version number).
    #[serde(default)]
    pub assets: HashMap<String, AssetOverride>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct AssetOverride {
    /// One of "macos", "windows", "linux", "android". Unrecognized values
    /// are ignored (left as whatever the automatic pipeline produced)
    /// rather than treated as a parse error, so a typo in one override
    /// doesn't break loading the whole file.
    pub platform: Option<String>,
    /// One of "x86_64", "arm64".
    pub arch: Option<String>,
    pub min_os_version: Option<String>,
    pub silent_install_args: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    #[error("failed to parse genjux.yaml: {0}")]
    Parse(String),
}

/// Parses a `genjux.yaml` document's contents.
pub fn load_overlay(yaml: &str) -> Result<CuratorOverlay, OverlayError> {
    serde_yaml::from_str(yaml).map_err(|e| OverlayError::Parse(e.to_string()))
}

fn parse_platform(value: &str) -> Option<Platform> {
    match value.to_lowercase().as_str() {
        "macos" => Some(Platform::MacOS),
        "windows" => Some(Platform::Windows),
        "linux" => Some(Platform::Linux),
        "android" => Some(Platform::Android),
        _ => None,
    }
}

fn parse_arch(value: &str) -> Option<Arch> {
    match value.to_lowercase().as_str() {
        "x86_64" | "amd64" => Some(Arch::X86_64),
        "arm64" | "aarch64" => Some(Arch::Arm64),
        _ => None,
    }
}

/// Applies an overlay's overrides on top of already-classified packages,
/// matching by exact asset filename. Only fields explicitly present in the
/// overlay are changed; everything else about the package (including
/// whatever the automatic pipeline already determined) is left as-is.
pub fn apply_overlay(overlay: &CuratorOverlay, packages: &mut [InstallablePackage]) {
    for package in packages.iter_mut() {
        let Some(over) = overlay.assets.get(&package.asset_name) else {
            continue;
        };

        if let Some(platform) = over.platform.as_deref().and_then(parse_platform) {
            package.classification.platform = Some(platform);
        }
        if let Some(arch) = over.arch.as_deref().and_then(parse_arch) {
            package.classification.arch = Some(arch);
        }
        if over.min_os_version.is_some() {
            package.min_os_version = over.min_os_version.clone();
        }
        if over.silent_install_args.is_some() {
            package.silent_install_args = over.silent_install_args.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::Classification;

    const SAMPLE_YAML: &str = r#"
assets:
  "myapp-latest.zip":
    platform: macos
    arch: arm64
    min_os_version: "12.0"
    silent_install_args: "--silent --no-prompt"
"#;

    fn unclassified_package(name: &str) -> InstallablePackage {
        InstallablePackage {
            asset_name: name.to_string(),
            download_url: format!("https://example.invalid/{name}"),
            size_bytes: 0,
            classification: Classification::default(),
            sha256: None,
            min_os_version: None,
            silent_install_args: None,
        }
    }

    #[test]
    fn parses_sample_overlay() {
        let overlay = load_overlay(SAMPLE_YAML).unwrap();
        let over = overlay.assets.get("myapp-latest.zip").unwrap();
        assert_eq!(over.platform.as_deref(), Some("macos"));
        assert_eq!(over.arch.as_deref(), Some("arm64"));
        assert_eq!(over.min_os_version.as_deref(), Some("12.0"));
    }

    #[test]
    fn apply_overlay_fills_in_expected_overrides() {
        let overlay = load_overlay(SAMPLE_YAML).unwrap();
        let mut packages = vec![unclassified_package("myapp-latest.zip")];

        apply_overlay(&overlay, &mut packages);

        assert_eq!(packages[0].classification.platform, Some(Platform::MacOS));
        assert_eq!(packages[0].classification.arch, Some(Arch::Arm64));
        assert_eq!(packages[0].min_os_version.as_deref(), Some("12.0"));
        assert_eq!(
            packages[0].silent_install_args.as_deref(),
            Some("--silent --no-prompt")
        );
    }

    #[test]
    fn apply_overlay_leaves_unmatched_assets_untouched() {
        let overlay = load_overlay(SAMPLE_YAML).unwrap();
        let mut packages = vec![unclassified_package("some-other-asset.tar.gz")];

        apply_overlay(&overlay, &mut packages);

        assert_eq!(packages[0].classification.platform, None);
    }

    #[test]
    fn empty_overlay_parses_to_no_overrides() {
        let overlay = load_overlay("assets: {}").unwrap();
        assert!(overlay.assets.is_empty());
    }

    #[test]
    fn malformed_yaml_returns_typed_error_not_panic() {
        let err = load_overlay("assets: [this is not a map]").unwrap_err();
        assert!(matches!(err, OverlayError::Parse(_)));
    }

    #[test]
    fn unrecognized_platform_value_is_ignored_not_a_parse_error() {
        let yaml = r#"
assets:
  "weird.bin":
    platform: "some-future-os"
"#;
        let overlay = load_overlay(yaml).unwrap();
        let mut packages = vec![unclassified_package("weird.bin")];
        apply_overlay(&overlay, &mut packages);

        // Unrecognized platform string doesn't crash the loader and simply
        // doesn't set a platform, rather than corrupting the pipeline.
        assert_eq!(packages[0].classification.platform, None);
    }
}
