//! Local installed-app registry and update checking (issue #15).
//!
//! Tracks what Genjux-Store has installed on this machine, persisted to
//! disk (a JSON file) so the registry survives a core service restart —
//! unlike [`crate::package::PackageCache`], which is just a performance
//! cache and is fine to lose. The [`InstalledAppRegistry`] trait keeps the
//! door open to swapping the JSON-file implementation for something like
//! sqlite later without touching callers.

use crate::source::{RepoRef, SourceProvider};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// A record of one installed app.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InstalledEntry {
    pub repo: RepoRef,
    pub installed_tag: String,
    /// Unix epoch seconds. Kept as a plain integer rather than pulling in
    /// a date/time crate for Phase 0 — callers that need a human-readable
    /// timestamp can format it themselves.
    pub installed_at_unix: u64,
    pub source_url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to (de)serialize registry contents: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Tracks installed apps. See module docs for why this needs real
/// persistence rather than an in-memory cache.
#[async_trait]
pub trait InstalledAppRegistry: Send + Sync {
    async fn record_install(&self, entry: InstalledEntry) -> Result<(), RegistryError>;
    async fn list_installed(&self) -> Result<Vec<InstalledEntry>, RegistryError>;
    async fn get(&self, repo: &RepoRef) -> Result<Option<InstalledEntry>, RegistryError>;
}

/// A [`InstalledAppRegistry`] backed by a single JSON file on disk, with an
/// in-memory mirror for fast reads. Every write re-serializes the whole
/// file — fine at the scale of "apps a single user has installed via
/// Genjux-Store" (tens to low hundreds of entries, not millions).
pub struct JsonFileRegistry {
    path: PathBuf,
    entries: Mutex<HashMap<RepoRef, InstalledEntry>>,
}

impl JsonFileRegistry {
    /// Opens (or creates) the registry at `path`, loading any existing
    /// entries into memory.
    pub async fn open(path: PathBuf) -> Result<Self, RegistryError> {
        let entries = if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            let contents = tokio::fs::read_to_string(&path).await?;
            if contents.trim().is_empty() {
                HashMap::new()
            } else {
                let list: Vec<InstalledEntry> = serde_json::from_str(&contents)?;
                list.into_iter().map(|e| (e.repo.clone(), e)).collect()
            }
        } else {
            HashMap::new()
        };

        Ok(Self {
            path,
            entries: Mutex::new(entries),
        })
    }

    async fn persist(&self) -> Result<(), RegistryError> {
        let list: Vec<InstalledEntry> = {
            let entries = self.entries.lock().expect("registry lock poisoned");
            entries.values().cloned().collect()
        };
        let json = serde_json::to_string_pretty(&list)?;
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&self.path, json).await?;
        Ok(())
    }
}

#[async_trait]
impl InstalledAppRegistry for JsonFileRegistry {
    async fn record_install(&self, entry: InstalledEntry) -> Result<(), RegistryError> {
        {
            let mut entries = self.entries.lock().expect("registry lock poisoned");
            entries.insert(entry.repo.clone(), entry);
        }
        self.persist().await
    }

    async fn list_installed(&self) -> Result<Vec<InstalledEntry>, RegistryError> {
        let entries = self.entries.lock().expect("registry lock poisoned");
        Ok(entries.values().cloned().collect())
    }

    async fn get(&self, repo: &RepoRef) -> Result<Option<InstalledEntry>, RegistryError> {
        let entries = self.entries.lock().expect("registry lock poisoned");
        Ok(entries.get(repo).cloned())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateCheckError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Source(#[from] crate::source::SourceError),
}

/// The result of comparing one installed entry against the latest release
/// known to its source provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCheckResult {
    pub repo: RepoRef,
    pub installed_tag: String,
    pub latest_tag: String,
    pub update_available: bool,
}

/// Checks every registry entry owned by `provider` (matched by
/// `RepoRef.provider`) against that provider's latest release, flagging
/// entries whose installed tag no longer matches the latest one.
pub async fn check_for_updates<R, P>(
    registry: &R,
    provider: &P,
) -> Result<Vec<UpdateCheckResult>, UpdateCheckError>
where
    R: InstalledAppRegistry,
    P: SourceProvider,
{
    let mut results = Vec::new();
    for entry in registry.list_installed().await? {
        if entry.repo.provider != provider.provider_id() {
            continue;
        }
        if let Some(latest) = provider.latest_release(&entry.repo).await? {
            results.push(UpdateCheckResult {
                update_available: latest.tag != entry.installed_tag,
                repo: entry.repo,
                installed_tag: entry.installed_tag,
                latest_tag: latest.tag,
            });
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(repo: RepoRef, tag: &str) -> InstalledEntry {
        InstalledEntry {
            repo,
            installed_tag: tag.to_string(),
            installed_at_unix: 1_700_000_000,
            source_url: "https://example.invalid/download".to_string(),
        }
    }

    #[tokio::test]
    async fn record_and_list_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = JsonFileRegistry::open(tmp.path().join("registry.json"))
            .await
            .unwrap();
        let repo = RepoRef::new("github", "acme", "widget");

        registry
            .record_install(sample_entry(repo.clone(), "v1.0.0"))
            .await
            .unwrap();

        let listed = registry.list_installed().await.unwrap();
        assert_eq!(listed, vec![sample_entry(repo, "v1.0.0")]);
    }

    #[tokio::test]
    async fn registry_survives_a_restart_by_reopening_the_same_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("registry.json");
        let repo = RepoRef::new("github", "acme", "widget");

        {
            let registry = JsonFileRegistry::open(path.clone()).await.unwrap();
            registry
                .record_install(sample_entry(repo.clone(), "v1.0.0"))
                .await
                .unwrap();
        } // registry (and its in-memory state) dropped here, simulating a restart

        let reopened = JsonFileRegistry::open(path).await.unwrap();
        let listed = reopened.list_installed().await.unwrap();
        assert_eq!(listed, vec![sample_entry(repo, "v1.0.0")]);
    }

    #[tokio::test]
    async fn recording_the_same_repo_again_overwrites_the_previous_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = JsonFileRegistry::open(tmp.path().join("registry.json"))
            .await
            .unwrap();
        let repo = RepoRef::new("github", "acme", "widget");

        registry
            .record_install(sample_entry(repo.clone(), "v1.0.0"))
            .await
            .unwrap();
        registry
            .record_install(sample_entry(repo.clone(), "v2.0.0"))
            .await
            .unwrap();

        let listed = registry.list_installed().await.unwrap();
        assert_eq!(listed, vec![sample_entry(repo, "v2.0.0")]);
    }

    #[tokio::test]
    async fn opening_a_nonexistent_registry_file_starts_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = JsonFileRegistry::open(tmp.path().join("does-not-exist.json"))
            .await
            .unwrap();
        assert!(registry.list_installed().await.unwrap().is_empty());
    }

    mod update_checking {
        use super::*;
        use crate::source::mock::MockProvider;
        use crate::source::Release;

        #[tokio::test]
        async fn flags_an_outdated_entry_against_a_newer_fixture_release_tag() {
            let tmp = tempfile::tempdir().unwrap();
            let registry = JsonFileRegistry::open(tmp.path().join("registry.json"))
                .await
                .unwrap();
            let repo = RepoRef::new("mock", "acme", "widget");
            registry
                .record_install(sample_entry(repo.clone(), "v1.0.0"))
                .await
                .unwrap();

            let provider = MockProvider::new().with_releases(
                repo.clone(),
                vec![Release {
                    tag: "v2.0.0".to_string(),
                    assets: vec![],
                }],
            );

            let results = check_for_updates(&registry, &provider).await.unwrap();
            assert_eq!(results.len(), 1);
            assert!(results[0].update_available);
            assert_eq!(results[0].latest_tag, "v2.0.0");
        }

        #[tokio::test]
        async fn up_to_date_entry_is_not_flagged() {
            let tmp = tempfile::tempdir().unwrap();
            let registry = JsonFileRegistry::open(tmp.path().join("registry.json"))
                .await
                .unwrap();
            let repo = RepoRef::new("mock", "acme", "widget");
            registry
                .record_install(sample_entry(repo.clone(), "v1.0.0"))
                .await
                .unwrap();

            let provider = MockProvider::new().with_releases(
                repo.clone(),
                vec![Release {
                    tag: "v1.0.0".to_string(),
                    assets: vec![],
                }],
            );

            let results = check_for_updates(&registry, &provider).await.unwrap();
            assert_eq!(results.len(), 1);
            assert!(!results[0].update_available);
        }

        #[tokio::test]
        async fn entries_from_a_different_provider_are_skipped() {
            let tmp = tempfile::tempdir().unwrap();
            let registry = JsonFileRegistry::open(tmp.path().join("registry.json"))
                .await
                .unwrap();
            let github_repo = RepoRef::new("github", "acme", "widget");
            registry
                .record_install(sample_entry(github_repo, "v1.0.0"))
                .await
                .unwrap();

            // A "mock"-provider instance has nothing to say about a
            // "github"-provider entry; it should just be skipped, not
            // error.
            let provider = MockProvider::new();
            let results = check_for_updates(&registry, &provider).await.unwrap();
            assert!(results.is_empty());
        }
    }
}
