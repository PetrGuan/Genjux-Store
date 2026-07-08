//! Install orchestration state machine (issue #11).
//!
//! Wires together resolve -> download -> verify -> platform install, per
//! `.copilot-workflow/PLAN.md` section 4. Platform-specific install
//! execution is delegated to a [`PlatformAdapter`] implementation (issues
//! #12/#13/#14 for macOS/Windows/Linux) — this module is purely the
//! orchestration glue and doesn't know how to actually run an installer.
//!
//! Checksum-manifest *discovery* (finding a `checksums.txt`/`SHA256SUMS`/
//! `<asset>.sha256` sibling asset in the same release and fetching it)
//! lives here rather than in #10, since it's part of the "resolve" stage
//! of the state machine described in PLAN.md section 4 ("有官方 checksum
//! 则比对"): #10 only implements the pure parsing/comparison logic this
//! module calls.

use crate::classify::current_platform;
use crate::download::{download_resumable, DownloadError};
use crate::package::InstallablePackage;
use crate::source::{Release, RepoRef, SourceError, SourceProvider};
use crate::verify::{
    parse_checksum_manifest, parse_single_checksum_file, sha256_file, verify_sha256, VerifyError,
};
use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// One stage of an in-progress (or finished) install. Emitted via the
/// `on_stage` callback passed to [`run_install`] so a caller (the HTTP/MCP
/// API layer, in later issues) can stream progress to clients.
#[derive(Debug, Clone, PartialEq)]
pub enum InstallStage {
    Resolving,
    Downloading {
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
    },
    /// Verification finished (not necessarily successfully — a checksum
    /// mismatch surfaces as an `Err` from [`run_install`], not through
    /// this stage). `matched_published_checksum` is `false` when we only
    /// had a self-computed hash to show the user, per the trust model in
    /// PLAN.md section 5.
    Verified {
        sha256: String,
        matched_published_checksum: bool,
    },
    Installing,
    Succeeded,
    Failed {
        reason: String,
    },
}

/// Executes the platform-specific install step for an already-downloaded,
/// already-verified file. Implementations live in later issues
/// (#12 macOS / #13 Windows / #14 Linux); this trait is just the seam
/// orchestration depends on.
#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    async fn install(&self, downloaded_file: &Path) -> Result<(), String>;
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error(transparent)]
    Verify(#[from] VerifyError),
    #[error("platform install failed: {0}")]
    PlatformInstall(String),
    #[error("no installable package found in the latest release for this platform")]
    NoMatchingPackage,
}

/// Filenames commonly used for a release-wide checksum manifest, checked
/// case-insensitively.
const CHECKSUM_MANIFEST_NAMES: &[&str] = &[
    "checksums.txt",
    "sha256sums",
    "sha256sums.txt",
    "checksums.sha256",
];

/// Looks for a checksum manifest or per-asset `.sha256` file among a
/// release's other assets, fetches it, and tries to find the expected
/// digest for `asset_name`. Returns `None` (not an error) if nothing
/// matches or nothing can be parsed — an unverifiable checksum isn't a
/// failure, it just means we fall back to showing a self-computed hash.
async fn find_published_checksum(
    client: &reqwest::Client,
    release: &Release,
    asset_name: &str,
) -> Option<String> {
    for candidate in &release.assets {
        let is_manifest = CHECKSUM_MANIFEST_NAMES
            .iter()
            .any(|n| candidate.name.eq_ignore_ascii_case(n));
        let is_per_file_sibling = candidate
            .name
            .eq_ignore_ascii_case(&format!("{asset_name}.sha256"));
        if !is_manifest && !is_per_file_sibling {
            continue;
        }

        let Ok(response) = client.get(&candidate.download_url).send().await else {
            continue;
        };
        let Ok(text) = response.text().await else {
            continue;
        };

        let found = if is_per_file_sibling {
            parse_single_checksum_file(&text, asset_name)
        } else {
            parse_checksum_manifest(&text, asset_name)
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Runs the full resolve -> download -> verify -> install pipeline for the
/// latest release of `repo`, picking whichever `InstallablePackage`
/// matches [`current_platform`]. Always emits a terminal
/// `InstallStage::Succeeded` or `InstallStage::Failed` via `on_stage`
/// before returning, in addition to returning the `Result` itself — so a
/// caller that's only watching the stage stream (e.g. over the future
/// HTTP/MCP API) still observes completion.
pub async fn run_install<P, A, F>(
    provider: &P,
    repo: &RepoRef,
    dest_dir: &Path,
    adapter: &A,
    mut on_stage: F,
) -> Result<(), InstallError>
where
    P: SourceProvider,
    A: PlatformAdapter,
    F: FnMut(InstallStage),
{
    match run_install_inner(provider, repo, dest_dir, adapter, &mut on_stage).await {
        Ok(()) => {
            on_stage(InstallStage::Succeeded);
            Ok(())
        }
        Err(err) => {
            on_stage(InstallStage::Failed {
                reason: err.to_string(),
            });
            Err(err)
        }
    }
}

async fn run_install_inner<P, A, F>(
    provider: &P,
    repo: &RepoRef,
    dest_dir: &Path,
    adapter: &A,
    on_stage: &mut F,
) -> Result<(), InstallError>
where
    P: SourceProvider,
    A: PlatformAdapter,
    F: FnMut(InstallStage),
{
    on_stage(InstallStage::Resolving);
    let release = provider
        .latest_release(repo)
        .await?
        .ok_or(InstallError::NoMatchingPackage)?;

    let target_platform = current_platform();
    let package: InstallablePackage = crate::package::classify_release(&release)
        .into_iter()
        .find(|p| target_platform.is_some() && p.classification.platform == target_platform)
        .ok_or(InstallError::NoMatchingPackage)?;

    let client = reqwest::Client::new();
    let published_checksum = find_published_checksum(&client, &release, &package.asset_name).await;

    let dest_path: PathBuf = dest_dir.join(&package.asset_name);

    on_stage(InstallStage::Downloading {
        bytes_downloaded: 0,
        total_bytes: None,
    });
    download_resumable(&client, &package.download_url, &dest_path, |progress| {
        on_stage(InstallStage::Downloading {
            bytes_downloaded: progress.bytes_downloaded,
            total_bytes: progress.total_bytes,
        });
    })
    .await?;

    let (sha256, matched_published_checksum) = match &published_checksum {
        Some(expected) => {
            verify_sha256(&dest_path, expected).await?;
            (expected.clone(), true)
        }
        None => (sha256_file(&dest_path).await?, false),
    };
    on_stage(InstallStage::Verified {
        sha256,
        matched_published_checksum,
    });

    on_stage(InstallStage::Installing);
    adapter
        .install(&dest_path)
        .await
        .map_err(InstallError::PlatformInstall)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::Platform;
    use crate::source::mock::MockProvider;
    use crate::source::ReleaseAsset;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ASSET_CONTENTS: &[u8] = b"fake binary contents";

    /// Filename that will classify (tier 1) to whatever platform this test
    /// actually runs on, so the same test works unmodified on both the
    /// macOS and Linux CI runners.
    fn asset_name_for_current_platform() -> &'static str {
        match current_platform().expect("test must run on a supported platform") {
            Platform::MacOS => "app-current.dmg",
            Platform::Windows => "app-current.exe",
            Platform::Linux => "app-current.AppImage",
            Platform::Android => "app-current.apk",
        }
    }

    struct FakeAdapter {
        should_fail: bool,
    }

    #[async_trait]
    impl PlatformAdapter for FakeAdapter {
        async fn install(&self, _downloaded_file: &Path) -> Result<(), String> {
            if self.should_fail {
                Err("simulated install failure".to_string())
            } else {
                Ok(())
            }
        }
    }

    async fn mount_asset(server: &MockServer, asset_name: &str) {
        Mock::given(method("GET"))
            .and(path(format!("/{asset_name}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(ASSET_CONTENTS))
            .mount(server)
            .await;
    }

    fn release_with_assets(assets: Vec<ReleaseAsset>) -> Release {
        Release {
            tag: "v1.0.0".to_string(),
            assets,
        }
    }

    fn asset(name: &str, download_url: String) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            size_bytes: ASSET_CONTENTS.len() as u64,
            download_url,
            content_type: None,
        }
    }

    #[tokio::test]
    async fn full_run_with_no_checksum_manifest_resolves_downloads_and_installs() {
        let server = MockServer::start().await;
        let asset_name = asset_name_for_current_platform();
        mount_asset(&server, asset_name).await;

        let repo = RepoRef::new("mock", "acme", "widget");
        let provider = MockProvider::new().with_releases(
            repo.clone(),
            vec![release_with_assets(vec![asset(
                asset_name,
                format!("{}/{asset_name}", server.uri()),
            )])],
        );
        let adapter = FakeAdapter { should_fail: false };
        let dest_dir = tempfile::tempdir().unwrap();

        let mut stages = Vec::new();
        let result = run_install(&provider, &repo, dest_dir.path(), &adapter, |stage| {
            stages.push(stage);
        })
        .await;

        assert!(result.is_ok(), "expected success, got {result:?}");
        assert_eq!(stages.first(), Some(&InstallStage::Resolving));
        assert_eq!(stages.last(), Some(&InstallStage::Succeeded));
        assert!(stages.iter().any(|s| matches!(
            s,
            InstallStage::Verified {
                matched_published_checksum: false,
                ..
            }
        )));
        assert!(stages.contains(&InstallStage::Installing));
    }

    #[tokio::test]
    async fn full_run_with_matching_checksum_manifest_verifies_against_it() {
        let server = MockServer::start().await;
        let asset_name = asset_name_for_current_platform();
        mount_asset(&server, asset_name).await;

        let expected_hex = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(ASSET_CONTENTS);
            format!("{:x}", hasher.finalize())
        };
        let manifest_body = format!("{expected_hex}  {asset_name}\n");
        Mock::given(method("GET"))
            .and(path("/checksums.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&manifest_body))
            .mount(&server)
            .await;

        let repo = RepoRef::new("mock", "acme", "widget");
        let provider = MockProvider::new().with_releases(
            repo.clone(),
            vec![release_with_assets(vec![
                asset(asset_name, format!("{}/{asset_name}", server.uri())),
                asset("checksums.txt", format!("{}/checksums.txt", server.uri())),
            ])],
        );
        let adapter = FakeAdapter { should_fail: false };
        let dest_dir = tempfile::tempdir().unwrap();

        let mut stages = Vec::new();
        let result = run_install(&provider, &repo, dest_dir.path(), &adapter, |stage| {
            stages.push(stage);
        })
        .await;

        assert!(result.is_ok(), "expected success, got {result:?}");
        assert!(stages.iter().any(|s| matches!(
            s,
            InstallStage::Verified {
                matched_published_checksum: true,
                sha256
            } if *sha256 == expected_hex
        )));
    }

    #[tokio::test]
    async fn checksum_mismatch_against_a_real_manifest_fails_before_install() {
        let server = MockServer::start().await;
        let asset_name = asset_name_for_current_platform();
        mount_asset(&server, asset_name).await;

        // A manifest entry that does NOT match the actual asset bytes.
        let wrong_hex = "0".repeat(64);
        let manifest_body = format!("{wrong_hex}  {asset_name}\n");
        Mock::given(method("GET"))
            .and(path("/checksums.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&manifest_body))
            .mount(&server)
            .await;

        let repo = RepoRef::new("mock", "acme", "widget");
        let provider = MockProvider::new().with_releases(
            repo.clone(),
            vec![release_with_assets(vec![
                asset(asset_name, format!("{}/{asset_name}", server.uri())),
                asset("checksums.txt", format!("{}/checksums.txt", server.uri())),
            ])],
        );
        let adapter = FakeAdapter { should_fail: false };
        let dest_dir = tempfile::tempdir().unwrap();

        let mut stages = Vec::new();
        let result = run_install(&provider, &repo, dest_dir.path(), &adapter, |stage| {
            stages.push(stage);
        })
        .await;

        assert!(matches!(
            result,
            Err(InstallError::Verify(VerifyError::Mismatch { .. }))
        ));
        // Must fail before ever reaching the Installing stage.
        assert!(!stages.contains(&InstallStage::Installing));
        assert!(matches!(stages.last(), Some(InstallStage::Failed { .. })));
    }

    #[tokio::test]
    async fn platform_install_failure_surfaces_as_a_terminal_failed_stage_and_err() {
        let server = MockServer::start().await;
        let asset_name = asset_name_for_current_platform();
        mount_asset(&server, asset_name).await;

        let repo = RepoRef::new("mock", "acme", "widget");
        let provider = MockProvider::new().with_releases(
            repo.clone(),
            vec![release_with_assets(vec![asset(
                asset_name,
                format!("{}/{asset_name}", server.uri()),
            )])],
        );
        let adapter = FakeAdapter { should_fail: true };
        let dest_dir = tempfile::tempdir().unwrap();

        let mut stages = Vec::new();
        let result = run_install(&provider, &repo, dest_dir.path(), &adapter, |stage| {
            stages.push(stage);
        })
        .await;

        assert!(matches!(result, Err(InstallError::PlatformInstall(_))));
        match stages.last() {
            Some(InstallStage::Failed { reason }) => {
                assert!(reason.contains("simulated install failure"));
            }
            other => panic!("expected a terminal Failed stage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_release_at_all_fails_with_source_error_and_a_failed_stage() {
        let provider = MockProvider::new(); // no releases registered for any repo
        let repo = RepoRef::new("mock", "acme", "widget");
        let adapter = FakeAdapter { should_fail: false };
        let dest_dir = tempfile::tempdir().unwrap();

        let mut stages = Vec::new();
        let result = run_install(&provider, &repo, dest_dir.path(), &adapter, |stage| {
            stages.push(stage);
        })
        .await;

        assert!(matches!(result, Err(InstallError::Source(_))));
        assert!(matches!(stages.last(), Some(InstallStage::Failed { .. })));
    }
}
