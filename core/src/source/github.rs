//! GitHub implementation of [`SourceProvider`](super::SourceProvider).
//!
//! This is the first (and, for Phase 0, only) concrete source provider. Its
//! [`SourceProvider`] **trait** implementation must not leak any
//! GitHub-specific types outside this module — everything it returns
//! through the trait is expressed in the provider-agnostic types from
//! [`super`]. See <https://github.com/PetrGuan/Genjux-Store/issues/28> for
//! the abstraction this implements and
//! <https://github.com/PetrGuan/Genjux-Store/issues/3> for this issue.
//!
//! [`GitHubProvider::fetch_metadata`] is a deliberate exception: it's an
//! *inherent* method (not part of [`SourceProvider`]), added for the
//! Phase 1 macOS GUI's app-detail screen (#57). README/star-count/last-
//! release-date shape varies enough across hosts that it isn't worth
//! generalizing into the trait before a second provider actually exists to
//! design the abstraction against — the same reasoning that kept
//! [`crate::discovery::GitHubSearchClient`] concrete rather than behind a
//! new trait.

use super::{Release, ReleaseAsset, RepoRef, SourceError, SourceProvider};
use async_trait::async_trait;
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "https://api.github.com";
const USER_AGENT: &str = concat!("genjux-store/", env!("CARGO_PKG_VERSION"));
/// Safety cap on pagination so a misbehaving server (or a bug in our Link
/// header parsing) can't cause an unbounded loop of requests.
const MAX_PAGES: usize = 20;
/// Character cap on the README excerpt returned by `fetch_metadata` — the
/// app-detail screen needs a preview, not the full document.
const README_EXCERPT_MAX_CHARS: usize = 500;

/// Fetches releases from the GitHub REST API.
pub struct GitHubProvider {
    client: reqwest::Client,
    base_url: String,
    token: Option<String>,
}

impl Default for GitHubProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            token: None,
        }
    }

    /// Builds a provider that reads an optional token from the
    /// `GENJUX_GITHUB_TOKEN` environment variable, to raise the (otherwise
    /// very low, shared-by-IP) unauthenticated rate limit.
    pub fn from_env() -> Self {
        let mut provider = Self::new();
        if let Ok(token) = std::env::var("GENJUX_GITHUB_TOKEN") {
            if !token.is_empty() {
                provider.token = Some(token);
            }
        }
        provider
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Overrides the API base URL. Only exposed under `#[cfg(test)]`
    /// (`pub(crate)` so other modules' tests, like `api.rs`'s, can point a
    /// `GitHubProvider` at a wiremock server too — not part of the public
    /// API).
    #[cfg(test)]
    pub(crate) fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn releases_url(&self, repo: &RepoRef) -> String {
        format!(
            "{}/repos/{}/{}/releases?per_page=100",
            self.base_url, repo.owner, repo.repo
        )
    }

    /// Parses the `Link` response header (RFC 5988) for a `rel="next"` URL.
    fn next_page_url(headers: &reqwest::header::HeaderMap) -> Option<String> {
        let link = headers.get(reqwest::header::LINK)?.to_str().ok()?;
        for part in link.split(',') {
            let mut segments = part.split(';');
            let url_part = segments.next()?.trim();
            let is_next = segments.any(|s| s.trim() == "rel=\"next\"");
            if is_next {
                return Some(
                    url_part
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_string(),
                );
            }
        }
        None
    }

    async fn get_page(&self, url: &str) -> Result<(Vec<GhRelease>, Option<String>), SourceError> {
        let mut request = self
            .client
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await.map_err(|e| SourceError::Network {
            provider: "github",
            message: e.to_string(),
        })?;

        let status = response.status();
        let headers = response.headers().clone();

        if status == reqwest::StatusCode::NOT_FOUND {
            // The specific RepoRef isn't known at this layer; callers
            // (list_releases) substitute the real one in.
            return Err(SourceError::NotFound(RepoRef::new("github", "", "")));
        }

        let rate_limited = matches!(
            status,
            reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::TOO_MANY_REQUESTS
        ) && headers
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            == Some("0");

        if rate_limited {
            let retry_after_secs = headers
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            return Err(SourceError::RateLimited {
                provider: "github",
                retry_after_secs,
            });
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SourceError::Provider {
                provider: "github",
                message: format!("HTTP {status}: {body}"),
            });
        }

        let next = Self::next_page_url(&headers);
        let releases: Vec<GhRelease> =
            response.json().await.map_err(|e| SourceError::Provider {
                provider: "github",
                message: format!("failed to parse response: {e}"),
            })?;

        Ok((releases, next))
    }
}

#[async_trait]
impl SourceProvider for GitHubProvider {
    fn provider_id(&self) -> &'static str {
        "github"
    }

    async fn list_releases(&self, repo: &RepoRef) -> Result<Vec<Release>, SourceError> {
        let mut url = self.releases_url(repo);
        let mut all = Vec::new();

        for _ in 0..MAX_PAGES {
            let (page, next) = self.get_page(&url).await.map_err(|err| match err {
                SourceError::NotFound(_) => SourceError::NotFound(repo.clone()),
                other => other,
            })?;
            let page_len = page.len();
            all.extend(page);
            match next {
                Some(next_url) if page_len > 0 => url = next_url,
                _ => break,
            }
        }

        Ok(all.into_iter().map(Release::from).collect())
    }
}

/// Repo-level metadata for the Phase 1 macOS GUI's app-detail screen
/// (#57) — deliberately separate from [`Release`]/[`ReleaseAsset`], since
/// this isn't release-specific and isn't part of the [`SourceProvider`]
/// trait (see this module's doc comment for why).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepoMetadata {
    pub stars: u64,
    pub description: Option<String>,
    /// ISO 8601 timestamp of the latest release, if the repo has any
    /// releases at all (a repo with none isn't an error here — the
    /// detail screen just shows nothing for this field).
    pub last_release_at: Option<String>,
    /// A short prefix of the repo's README, truncated to
    /// [`README_EXCERPT_MAX_CHARS`] — a preview, not the full document.
    /// `None` if the repo has no README.
    pub readme_excerpt: Option<String>,
}

impl GitHubProvider {
    /// Fetches [`RepoMetadata`] for `repo`: star count + description (from
    /// the repo info endpoint), the latest release's publish date (from
    /// the releases/latest convenience endpoint — absent, not an error, if
    /// there are no releases), and a truncated README excerpt (raw
    /// markdown, via the `Accept: application/vnd.github.raw` content
    /// negotiation, avoiding manual base64 decoding of the default JSON
    /// response) — absent, not an error, if the repo has no README.
    pub async fn fetch_metadata(&self, repo: &RepoRef) -> Result<RepoMetadata, SourceError> {
        let repo_info = self.fetch_repo_info(repo).await?;
        let last_release_at = self.fetch_latest_release_published_at(repo).await?;
        let readme_excerpt = self.fetch_readme_excerpt(repo).await?;

        Ok(RepoMetadata {
            stars: repo_info.stargazers_count,
            description: repo_info.description,
            last_release_at,
            readme_excerpt,
        })
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<Option<T>, SourceError> {
        let mut request = self
            .client
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await.map_err(|e| SourceError::Network {
            provider: "github",
            message: e.to_string(),
        })?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(SourceError::Provider {
                provider: "github",
                message: format!("HTTP {status}: {body}"),
            });
        }

        response
            .json()
            .await
            .map(Some)
            .map_err(|e| SourceError::Provider {
                provider: "github",
                message: format!("failed to parse response: {e}"),
            })
    }

    async fn fetch_repo_info(&self, repo: &RepoRef) -> Result<GhRepoInfo, SourceError> {
        let url = format!("{}/repos/{}/{}", self.base_url, repo.owner, repo.repo);
        self.get_json(&url)
            .await?
            .ok_or_else(|| SourceError::NotFound(repo.clone()))
    }

    async fn fetch_latest_release_published_at(
        &self,
        repo: &RepoRef,
    ) -> Result<Option<String>, SourceError> {
        let url = format!(
            "{}/repos/{}/{}/releases/latest",
            self.base_url, repo.owner, repo.repo
        );
        let release: Option<GhReleaseWithPublishedAt> = self.get_json(&url).await?;
        Ok(release.and_then(|r| r.published_at))
    }

    async fn fetch_readme_excerpt(&self, repo: &RepoRef) -> Result<Option<String>, SourceError> {
        let url = format!(
            "{}/repos/{}/{}/readme",
            self.base_url, repo.owner, repo.repo
        );
        let mut request = self
            .client
            .get(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github.raw");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await.map_err(|e| SourceError::Network {
            provider: "github",
            message: e.to_string(),
        })?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(SourceError::Provider {
                provider: "github",
                message: format!("HTTP {status}: {body}"),
            });
        }

        let text = response.text().await.map_err(|e| SourceError::Provider {
            provider: "github",
            message: format!("failed to read README body: {e}"),
        })?;

        let excerpt: String = text.chars().take(README_EXCERPT_MAX_CHARS).collect();
        Ok(Some(excerpt))
    }
}

#[derive(Debug, Deserialize)]
struct GhRepoInfo {
    stargazers_count: u64,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhReleaseWithPublishedAt {
    published_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
    content_type: Option<String>,
}

impl From<GhRelease> for Release {
    fn from(gh: GhRelease) -> Self {
        Release {
            tag: gh.tag_name,
            assets: gh.assets.into_iter().map(ReleaseAsset::from).collect(),
        }
    }
}

impl From<GhAsset> for ReleaseAsset {
    fn from(gh: GhAsset) -> Self {
        ReleaseAsset {
            name: gh.name,
            size_bytes: gh.size,
            download_url: gh.browser_download_url,
            content_type: gh.content_type,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn lists_releases_and_parses_assets() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widget/releases"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "tag_name": "v1.0.0",
                    "assets": [
                        {
                            "name": "widget-v1.0.0-macos-arm64.dmg",
                            "size": 12345,
                            "browser_download_url": "https://example.invalid/widget.dmg",
                            "content_type": "application/octet-stream"
                        }
                    ]
                }
            ])))
            .mount(&server)
            .await;

        let provider = GitHubProvider::new().with_base_url(server.uri());
        let repo = RepoRef::new("github", "acme", "widget");
        let releases = provider.list_releases(&repo).await.unwrap();

        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].tag, "v1.0.0");
        assert_eq!(releases[0].assets[0].name, "widget-v1.0.0-macos-arm64.dmg");
        assert_eq!(releases[0].assets[0].size_bytes, 12345);
    }

    #[tokio::test]
    async fn not_found_repo_returns_typed_error_with_the_original_repo_ref() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/nobody/nothing/releases"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let provider = GitHubProvider::new().with_base_url(server.uri());
        let repo = RepoRef::new("github", "nobody", "nothing");
        let err = provider.list_releases(&repo).await.unwrap_err();
        assert!(matches!(err, SourceError::NotFound(r) if r == repo));
    }

    #[tokio::test]
    async fn rate_limit_exhaustion_returns_typed_error_not_panic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widget/releases"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("x-ratelimit-remaining", "0")
                    .insert_header("retry-after", "30"),
            )
            .mount(&server)
            .await;

        let provider = GitHubProvider::new().with_base_url(server.uri());
        let repo = RepoRef::new("github", "acme", "widget");
        let err = provider.list_releases(&repo).await.unwrap_err();
        match err {
            SourceError::RateLimited {
                retry_after_secs, ..
            } => assert_eq!(retry_after_secs, Some(30)),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn follows_pagination_link_header() {
        let server = MockServer::start().await;
        // Point "next" at a completely distinct mock path, since
        // `get_page` simply follows whatever URL the Link header contains
        // (it doesn't reconstruct query params) — this also sidesteps any
        // ambiguity between overlapping query-param matchers on the same
        // mocked path.
        let page2_url = format!("{}/releases-page-2", server.uri());

        Mock::given(method("GET"))
            .and(path("/repos/acme/widget/releases"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{"tag_name": "v2.0.0", "assets": []}]))
                    .insert_header("link", format!("<{page2_url}>; rel=\"next\"")),
            )
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/releases-page-2"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{"tag_name": "v1.0.0", "assets": []}])),
            )
            .mount(&server)
            .await;

        let provider = GitHubProvider::new().with_base_url(server.uri());
        let repo = RepoRef::new("github", "acme", "widget");
        let releases = provider.list_releases(&repo).await.unwrap();

        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].tag, "v2.0.0");
        assert_eq!(releases[1].tag, "v1.0.0");
    }

    #[tokio::test]
    async fn provider_id_is_github() {
        let provider = GitHubProvider::new();
        assert_eq!(provider.provider_id(), "github");
    }

    #[tokio::test]
    async fn fetch_metadata_combines_repo_info_latest_release_and_readme() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widget"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "stargazers_count": 4200,
                "description": "A fine widget",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widget/releases/latest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tag_name": "v1.0.0",
                "published_at": "2025-01-01T00:00:00Z",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widget/readme"))
            .respond_with(ResponseTemplate::new(200).set_body_string("# Widget\n\nIt widgets."))
            .mount(&server)
            .await;

        let provider = GitHubProvider::new().with_base_url(server.uri());
        let repo = RepoRef::new("github", "acme", "widget");
        let metadata = provider.fetch_metadata(&repo).await.unwrap();

        assert_eq!(metadata.stars, 4200);
        assert_eq!(metadata.description.as_deref(), Some("A fine widget"));
        assert_eq!(
            metadata.last_release_at.as_deref(),
            Some("2025-01-01T00:00:00Z")
        );
        assert_eq!(
            metadata.readme_excerpt.as_deref(),
            Some("# Widget\n\nIt widgets.")
        );
    }

    #[tokio::test]
    async fn fetch_metadata_tolerates_a_repo_with_no_releases_and_no_readme() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widget"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "stargazers_count": 1,
                "description": null,
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widget/releases/latest"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widget/readme"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let provider = GitHubProvider::new().with_base_url(server.uri());
        let repo = RepoRef::new("github", "acme", "widget");
        let metadata = provider.fetch_metadata(&repo).await.unwrap();

        assert_eq!(metadata.stars, 1);
        assert_eq!(metadata.description, None);
        assert_eq!(metadata.last_release_at, None);
        assert_eq!(metadata.readme_excerpt, None);
    }

    #[tokio::test]
    async fn fetch_metadata_truncates_a_long_readme_to_the_excerpt_cap() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widget"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "stargazers_count": 1,
                "description": null,
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widget/releases/latest"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widget/readme"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(10_000)))
            .mount(&server)
            .await;

        let provider = GitHubProvider::new().with_base_url(server.uri());
        let repo = RepoRef::new("github", "acme", "widget");
        let metadata = provider.fetch_metadata(&repo).await.unwrap();

        assert_eq!(
            metadata.readme_excerpt.unwrap().len(),
            README_EXCERPT_MAX_CHARS
        );
    }

    #[tokio::test]
    async fn fetch_metadata_for_an_entirely_unknown_repo_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/nobody/nothing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let provider = GitHubProvider::new().with_base_url(server.uri());
        let repo = RepoRef::new("github", "nobody", "nothing");
        let err = provider.fetch_metadata(&repo).await.unwrap_err();
        assert!(matches!(err, SourceError::NotFound(r) if r == repo));
    }

    #[tokio::test]
    #[ignore = "hits the real GitHub REST API; run explicitly with \
                `cargo test -p genjux-core source::github::tests::real_fetch_metadata -- --ignored`"]
    async fn real_fetch_metadata_returns_sensible_data_for_a_well_known_repo() {
        let provider = GitHubProvider::from_env();
        let repo = RepoRef::new("github", "cli", "cli");
        let metadata = provider
            .fetch_metadata(&repo)
            .await
            .expect("real GitHub REST API request should succeed");

        assert!(metadata.stars > 1000, "expected cli/cli to have many stars");
        assert!(metadata.description.is_some());
        assert!(metadata.last_release_at.is_some());
        assert!(metadata.readme_excerpt.is_some());
        assert!(metadata.readme_excerpt.unwrap().len() <= README_EXCERPT_MAX_CHARS);
    }
}
