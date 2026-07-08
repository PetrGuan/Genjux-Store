//! Pluggable release-source abstraction.
//!
//! Genjux-Store fetches releases from GitHub today, but is designed from
//! Phase 0 to support other source hosts (Gitee, GitLab, Codeberg/Forgejo,
//! GitCode, AtomGit, ...) without touching the classification, caching, or
//! download layers. Everything above this module operates purely in terms
//! of [`RepoRef`], [`Release`], and [`ReleaseAsset`] — never against a
//! provider-specific type.
//!
//! See `.copilot-workflow/PLAN.md` section 3 and
//! <https://github.com/PetrGuan/Genjux-Store/issues/28> for the design
//! rationale.

use async_trait::async_trait;
use std::fmt;

/// A reference to a repository on some source provider.
///
/// `provider` is a short identifier (e.g. `"github"`) rather than an enum so
/// that new providers can be added without changing this type — the set of
/// valid values is owned by whichever [`SourceProvider`] registry is in use,
/// not by this struct.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RepoRef {
    pub provider: String,
    pub owner: String,
    pub repo: String,
}

impl RepoRef {
    pub fn new(
        provider: impl Into<String>,
        owner: impl Into<String>,
        repo: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            owner: owner.into(),
            repo: repo.into(),
        }
    }
}

impl fmt::Display for RepoRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}/{}", self.provider, self.owner, self.repo)
    }
}

/// A single downloadable asset attached to a release.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub size_bytes: u64,
    pub download_url: String,
    pub content_type: Option<String>,
}

/// A single release (e.g. a GitHub "release" / git tag with attached assets).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Release {
    pub tag: String,
    pub assets: Vec<ReleaseAsset>,
}

/// Errors a [`SourceProvider`] implementation can return.
///
/// Kept provider-agnostic on purpose: callers should be able to handle these
/// without knowing which concrete provider produced them.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("network error talking to {provider}: {message}")]
    Network {
        provider: &'static str,
        message: String,
    },

    #[error("rate limited by {provider}, retry after {retry_after_secs:?} seconds")]
    RateLimited {
        provider: &'static str,
        retry_after_secs: Option<u64>,
    },

    #[error("repo not found: {0}")]
    NotFound(RepoRef),

    #[error("{provider} returned an unexpected response: {message}")]
    Provider {
        provider: &'static str,
        message: String,
    },
}

/// A source of releases for a given [`RepoRef`].
///
/// Implementations should not leak provider-specific types through this
/// trait's signatures — everything must be expressible in [`RepoRef`],
/// [`Release`], [`ReleaseAsset`], and [`SourceError`].
#[async_trait]
pub trait SourceProvider: Send + Sync {
    /// Short identifier for this provider, e.g. `"github"`. Must match the
    /// `provider` field on the [`RepoRef`]s this implementation accepts.
    fn provider_id(&self) -> &'static str;

    /// List releases for a repo, most recent first.
    async fn list_releases(&self, repo: &RepoRef) -> Result<Vec<Release>, SourceError>;

    /// Get the latest release for a repo, if any exist.
    ///
    /// Default implementation just takes the head of [`Self::list_releases`];
    /// providers may override this with a more efficient direct call.
    async fn latest_release(&self, repo: &RepoRef) -> Result<Option<Release>, SourceError> {
        Ok(self.list_releases(repo).await?.into_iter().next())
    }
}

#[cfg(test)]
pub(crate) mod mock {
    //! A minimal in-memory [`SourceProvider`] used only to prove the trait
    //! boundary isn't leaky (see #28 acceptance criteria): code written
    //! against `SourceProvider` should work unmodified against any
    //! implementation, not just a real GitHub one.
    use super::*;
    use std::collections::HashMap;

    pub struct MockProvider {
        releases: HashMap<RepoRef, Vec<Release>>,
    }

    impl MockProvider {
        pub fn new() -> Self {
            Self {
                releases: HashMap::new(),
            }
        }

        pub fn with_releases(mut self, repo: RepoRef, releases: Vec<Release>) -> Self {
            self.releases.insert(repo, releases);
            self
        }
    }

    #[async_trait]
    impl SourceProvider for MockProvider {
        fn provider_id(&self) -> &'static str {
            "mock"
        }

        async fn list_releases(&self, repo: &RepoRef) -> Result<Vec<Release>, SourceError> {
            self.releases
                .get(repo)
                .cloned()
                .ok_or_else(|| SourceError::NotFound(repo.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockProvider;
    use super::*;

    fn sample_release(tag: &str) -> Release {
        Release {
            tag: tag.to_string(),
            assets: vec![ReleaseAsset {
                name: format!("app-{tag}-x86_64-unknown-linux-gnu.tar.gz"),
                size_bytes: 1024,
                download_url: format!("https://example.invalid/{tag}/app.tar.gz"),
                content_type: None,
            }],
        }
    }

    /// Generic helper written purely against `SourceProvider` — if this
    /// compiles and works against `MockProvider`, the abstraction isn't
    /// leaky, satisfying #28's acceptance criteria ahead of the real GitHub
    /// implementation landing in #3.
    async fn total_assets<P: SourceProvider>(provider: &P, repo: &RepoRef) -> usize {
        provider
            .list_releases(repo)
            .await
            .map(|releases| releases.iter().map(|r| r.assets.len()).sum())
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn latest_release_default_impl_returns_head_of_list() {
        let repo = RepoRef::new("mock", "acme", "widget");
        let provider = MockProvider::new().with_releases(
            repo.clone(),
            vec![sample_release("v2.0.0"), sample_release("v1.0.0")],
        );

        let latest = provider.latest_release(&repo).await.unwrap();
        assert_eq!(latest.unwrap().tag, "v2.0.0");
    }

    #[tokio::test]
    async fn unknown_repo_returns_not_found() {
        let provider = MockProvider::new();
        let repo = RepoRef::new("mock", "nobody", "nothing");

        let err = provider.list_releases(&repo).await.unwrap_err();
        assert!(matches!(err, SourceError::NotFound(r) if r == repo));
    }

    #[tokio::test]
    async fn generic_code_over_the_trait_works_against_the_mock() {
        let repo = RepoRef::new("mock", "acme", "widget");
        let provider =
            MockProvider::new().with_releases(repo.clone(), vec![sample_release("v1.0.0")]);

        assert_eq!(total_assets(&provider, &repo).await, 1);
    }

    #[test]
    fn repo_ref_display_format() {
        let repo = RepoRef::new("github", "acme", "widget");
        assert_eq!(repo.to_string(), "github:acme/widget");
    }
}
