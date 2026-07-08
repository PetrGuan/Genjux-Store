//! The standardized, provider-agnostic package representation
//! ([`InstallablePackage`]) and a cache of classified releases keyed by
//! release tag, per `.copilot-workflow/PLAN.md` section 3 (issue #8).
//!
//! Everything downstream of classification (download, verification, install
//! orchestration) should operate on [`InstallablePackage`], never directly
//! on a [`crate::source::ReleaseAsset`] — this is what keeps those layers
//! provider-agnostic (see [`crate::source`]) and classification-pipeline-
//! agnostic (see [`crate::classify`]).

use crate::classify::Classification;
use crate::source::{Release, RepoRef};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

/// A single release asset, fully classified and enriched with whatever
/// extra metadata later pipeline stages attach (checksum, curator
/// overrides for OS-version requirements / silent-install flags, ...).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InstallablePackage {
    pub asset_name: String,
    pub download_url: String,
    pub size_bytes: u64,
    pub classification: Classification,
    /// Populated by the checksum-verification stage (#10) when the release
    /// publishes one, or after downloading if we computed it ourselves.
    pub sha256: Option<String>,
    /// Populated by the curator metadata overlay (#7) when known.
    pub min_os_version: Option<String>,
    /// Populated by the curator metadata overlay (#7) when known.
    pub silent_install_args: Option<String>,
}

impl InstallablePackage {
    /// Builds a package from a classified asset, with no enrichment yet
    /// (sha256/min_os_version/silent_install_args filled in by later
    /// pipeline stages).
    pub fn from_classification(
        asset: &crate::source::ReleaseAsset,
        classification: Classification,
    ) -> Self {
        Self {
            asset_name: asset.name.clone(),
            download_url: asset.download_url.clone(),
            size_bytes: asset.size_bytes,
            classification,
            sha256: None,
            min_os_version: None,
            silent_install_args: None,
        }
    }
}

/// Classifies every asset in a release into an [`InstallablePackage`],
/// using the filename-only tiers (1+2) of the classification pipeline.
/// Tier 3 (content sniffing) isn't run here since it needs network access
/// per-asset; callers that need it should run
/// [`crate::classify::sniff_remote_asset_platform`] themselves for assets
/// that come back unclassified.
pub fn classify_release(release: &Release) -> Vec<InstallablePackage> {
    release
        .assets
        .iter()
        .map(|asset| {
            let classification = crate::classify::classify_asset_by_filename(asset);
            InstallablePackage::from_classification(asset, classification)
        })
        .collect()
}

/// Caches classified releases keyed by `(repo, release tag)`, so repeated
/// requests for the same release don't re-fetch from the
/// [`crate::source::SourceProvider`] or re-run classification.
///
/// This is a trait (rather than a concrete struct) so the in-memory
/// implementation used in Phase 0 can later be swapped for a persistent
/// one (e.g. sled/sqlite) without changing callers.
#[async_trait]
pub trait PackageCache: Send + Sync {
    async fn get(&self, repo: &RepoRef, tag: &str) -> Option<Vec<InstallablePackage>>;
    async fn put(&self, repo: &RepoRef, tag: &str, packages: Vec<InstallablePackage>);
}

/// A simple process-local, in-memory [`PackageCache`]. Sufficient for
/// Phase 0; not persisted across restarts of the core service.
#[derive(Default)]
pub struct InMemoryPackageCache {
    entries: Mutex<HashMap<(RepoRef, String), Vec<InstallablePackage>>>,
}

impl InMemoryPackageCache {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PackageCache for InMemoryPackageCache {
    async fn get(&self, repo: &RepoRef, tag: &str) -> Option<Vec<InstallablePackage>> {
        let entries = self.entries.lock().expect("package cache lock poisoned");
        entries.get(&(repo.clone(), tag.to_string())).cloned()
    }

    async fn put(&self, repo: &RepoRef, tag: &str, packages: Vec<InstallablePackage>) {
        let mut entries = self.entries.lock().expect("package cache lock poisoned");
        entries.insert((repo.clone(), tag.to_string()), packages);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::ReleaseAsset;

    fn sample_release(tag: &str) -> Release {
        Release {
            tag: tag.to_string(),
            assets: vec![ReleaseAsset {
                name: "myapp-arm64.dmg".to_string(),
                size_bytes: 42,
                download_url: "https://example.invalid/myapp.dmg".to_string(),
                content_type: None,
            }],
        }
    }

    #[test]
    fn classify_release_produces_one_package_per_asset() {
        let release = sample_release("v1.0.0");
        let packages = classify_release(&release);

        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].asset_name, "myapp-arm64.dmg");
        assert_eq!(
            packages[0].classification.platform,
            Some(crate::classify::Platform::MacOS)
        );
        assert_eq!(packages[0].sha256, None);
    }

    #[tokio::test]
    async fn cache_hit_returns_previously_stored_packages() {
        let cache = InMemoryPackageCache::new();
        let repo = RepoRef::new("github", "acme", "widget");
        let packages = classify_release(&sample_release("v1.0.0"));

        cache.put(&repo, "v1.0.0", packages.clone()).await;

        let cached = cache.get(&repo, "v1.0.0").await;
        assert_eq!(cached, Some(packages));
    }

    #[tokio::test]
    async fn new_release_tag_is_a_cache_miss_even_when_an_older_tag_is_cached() {
        let cache = InMemoryPackageCache::new();
        let repo = RepoRef::new("github", "acme", "widget");
        cache
            .put(&repo, "v1.0.0", classify_release(&sample_release("v1.0.0")))
            .await;

        // A new tag we've never seen is correctly a miss, without needing
        // any explicit invalidation of the older cached entry.
        assert_eq!(cache.get(&repo, "v2.0.0").await, None);
        // The older entry is untouched.
        assert!(cache.get(&repo, "v1.0.0").await.is_some());
    }

    #[tokio::test]
    async fn different_repos_do_not_share_cache_entries() {
        let cache = InMemoryPackageCache::new();
        let repo_a = RepoRef::new("github", "acme", "widget");
        let repo_b = RepoRef::new("github", "acme", "gadget");
        cache
            .put(
                &repo_a,
                "v1.0.0",
                classify_release(&sample_release("v1.0.0")),
            )
            .await;

        assert_eq!(cache.get(&repo_b, "v1.0.0").await, None);
    }
}
