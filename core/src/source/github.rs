//! GitHub implementation of [`SourceProvider`](super::SourceProvider).
//!
//! This is the first (and, for Phase 0, only) concrete source provider. It
//! must not leak any GitHub-specific types outside this module — everything
//! it returns is expressed in the provider-agnostic types from
//! [`super`]. See <https://github.com/PetrGuan/Genjux-Store/issues/28> for
//! the abstraction this implements and
//! <https://github.com/PetrGuan/Genjux-Store/issues/3> for this issue.

use super::{Release, ReleaseAsset, RepoRef, SourceError, SourceProvider};
use async_trait::async_trait;
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "https://api.github.com";
const USER_AGENT: &str = concat!("genjux-store/", env!("CARGO_PKG_VERSION"));
/// Safety cap on pagination so a misbehaving server (or a bug in our Link
/// header parsing) can't cause an unbounded loop of requests.
const MAX_PAGES: usize = 20;

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

    /// Overrides the API base URL. Only exposed for tests, so they can point
    /// at a local mock server instead of the real network.
    #[cfg(test)]
    fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
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
}
