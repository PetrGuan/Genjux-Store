//! GitHub Search API-based discovery of candidate repos for the
//! recommended-software feed (issue #54, Phase 1 macOS GUI — see
//! PLAN.md section 6.1).
//!
//! This is intentionally a thin, GitHub-specific search client: it finds
//! *candidates* by topic, sorted by stars, and nothing more. It does
//! **not** filter candidates by "does this repo actually have an
//! installable asset for a given platform" — that quality gate is layered
//! on top of this in a later issue (#55), by running each candidate's
//! latest release through the existing [`crate::package::classify_release`]
//! pipeline. Keeping this module dumb-but-correct (just "what does GitHub
//! Search say for this topic") makes that layering straightforward to
//! test independently.

use serde::{Deserialize, Serialize};

const DEFAULT_BASE_URL: &str = "https://api.github.com/search/repositories";
const USER_AGENT: &str = concat!("genjux-store/", env!("CARGO_PKG_VERSION"));
/// GitHub caps search results at 100 per page; we never need more than one
/// page of candidates per topic for a recommended-software feed.
const MAX_PER_PAGE: u32 = 100;

/// A candidate repo surfaced by a topic search, before any installability
/// filtering has been applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryCandidate {
    pub owner: String,
    pub repo: String,
    pub stars: u64,
    pub description: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("network error searching GitHub: {0}")]
    Network(String),
    #[error("rate limited by GitHub search API, retry after {retry_after_secs:?} seconds")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("GitHub search API returned an unexpected response: {0}")]
    Provider(String),
}

/// Searches GitHub's `/search/repositories` endpoint by topic.
pub struct GitHubSearchClient {
    client: reqwest::Client,
    base_url: String,
    token: Option<String>,
}

impl Default for GitHubSearchClient {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubSearchClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            token: None,
        }
    }

    /// Builds a client that reads an optional token from the
    /// `GENJUX_GITHUB_TOKEN` environment variable (same convention as
    /// [`crate::source::github::GitHubProvider::from_env`]). Authenticating
    /// matters more here than for release-fetching: GitHub's Search API has
    /// a much stricter unauthenticated rate limit (10 requests/minute vs.
    /// 60/hour... note Search's limit is *per-minute*, not per-hour).
    pub fn from_env() -> Self {
        let mut client = Self::new();
        if let Ok(token) = std::env::var("GENJUX_GITHUB_TOKEN") {
            if !token.is_empty() {
                client.token = Some(token);
            }
        }
        client
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    #[cfg(test)]
    fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Searches for repos tagged with `topic`, sorted by star count
    /// descending, returning up to `per_page` results (capped at 100,
    /// GitHub's own per-page maximum).
    pub async fn search_by_topic(
        &self,
        topic: &str,
        per_page: u32,
    ) -> Result<Vec<DiscoveryCandidate>, DiscoveryError> {
        let per_page = per_page.clamp(1, MAX_PER_PAGE);
        let url = format!(
            "{}?q={}&sort=stars&order=desc&per_page={per_page}",
            self.base_url,
            format_args!("topic:{topic}"),
        );

        let mut request = self
            .client
            .get(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .await
            .map_err(|e| DiscoveryError::Network(e.to_string()))?;

        let status = response.status();
        let headers = response.headers().clone();

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
            return Err(DiscoveryError::RateLimited { retry_after_secs });
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(DiscoveryError::Provider(format!("HTTP {status}: {body}")));
        }

        let parsed: GhSearchResponse = response
            .json()
            .await
            .map_err(|e| DiscoveryError::Provider(format!("failed to parse response: {e}")))?;

        Ok(parsed
            .items
            .into_iter()
            .map(DiscoveryCandidate::from)
            .collect())
    }

    /// Searches across several topics and merges the results, deduplicated
    /// by `(owner, repo)` — the same repo is commonly tagged with more than
    /// one relevant topic (e.g. both `macos` and `menu-bar-app`). Keeps the
    /// overall list sorted by star count descending.
    pub async fn search_by_topics(
        &self,
        topics: &[&str],
        per_page_per_topic: u32,
    ) -> Result<Vec<DiscoveryCandidate>, DiscoveryError> {
        let mut seen = std::collections::HashSet::new();
        let mut merged = Vec::new();

        for topic in topics {
            let candidates = self.search_by_topic(topic, per_page_per_topic).await?;
            for candidate in candidates {
                if seen.insert((candidate.owner.clone(), candidate.repo.clone())) {
                    merged.push(candidate);
                }
            }
        }

        merged.sort_by(|a, b| b.stars.cmp(&a.stars));
        Ok(merged)
    }
}

#[derive(Debug, Deserialize)]
struct GhSearchResponse {
    items: Vec<GhSearchItem>,
}

#[derive(Debug, Deserialize)]
struct GhSearchItem {
    name: String,
    owner: GhSearchOwner,
    stargazers_count: u64,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhSearchOwner {
    login: String,
}

impl From<GhSearchItem> for DiscoveryCandidate {
    fn from(item: GhSearchItem) -> Self {
        DiscoveryCandidate {
            owner: item.owner.login,
            repo: item.name,
            stars: item.stargazers_count,
            description: item.description,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn search_response_body(items: &[(&str, &str, u64, Option<&str>)]) -> serde_json::Value {
        serde_json::json!({
            "total_count": items.len(),
            "incomplete_results": false,
            "items": items.iter().map(|(owner, repo, stars, description)| {
                serde_json::json!({
                    "name": repo,
                    "owner": { "login": owner },
                    "stargazers_count": stars,
                    "description": description,
                })
            }).collect::<Vec<_>>(),
        })
    }

    #[tokio::test]
    async fn search_by_topic_sends_the_expected_query_and_parses_results() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("q", "topic:macos"))
            .and(query_param("sort", "stars"))
            .and(query_param("order", "desc"))
            .and(query_param("per_page", "50"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(search_response_body(&[
                    ("alice", "widget", 500, Some("A nice widget")),
                    ("bob", "gadget", 100, None),
                ])),
            )
            .mount(&server)
            .await;

        let client = GitHubSearchClient::new().with_base_url(server.uri());
        let results = client.search_by_topic("macos", 50).await.unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].owner, "alice");
        assert_eq!(results[0].repo, "widget");
        assert_eq!(results[0].stars, 500);
        assert_eq!(results[0].description.as_deref(), Some("A nice widget"));
        assert_eq!(results[1].description, None);
    }

    #[tokio::test]
    async fn per_page_is_capped_at_the_github_maximum_of_100() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("per_page", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(search_response_body(&[])))
            .mount(&server)
            .await;

        let client = GitHubSearchClient::new().with_base_url(server.uri());
        // Ask for way more than GitHub allows; the mock only responds if
        // per_page was actually clamped to 100, proving the cap is applied
        // rather than just documented.
        client.search_by_topic("macos", 9_999).await.unwrap();
    }

    #[tokio::test]
    async fn search_by_topics_dedupes_repos_tagged_with_multiple_topics_and_sorts_by_stars() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("q", "topic:macos"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(search_response_body(&[
                    ("alice", "widget", 500, None),
                    ("carol", "thing", 50, None),
                ])),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("q", "topic:menu-bar-app"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(search_response_body(&[
                    ("alice", "widget", 500, None), // same repo, different topic
                    ("dave", "app", 800, None),
                ])),
            )
            .mount(&server)
            .await;

        let client = GitHubSearchClient::new().with_base_url(server.uri());
        let results = client
            .search_by_topics(&["macos", "menu-bar-app"], 50)
            .await
            .unwrap();

        // alice/widget must appear exactly once despite matching both
        // topics, and the merged list must be sorted by stars descending.
        let names: Vec<&str> = results.iter().map(|c| c.repo.as_str()).collect();
        assert_eq!(names, vec!["app", "widget", "thing"]);
    }

    #[tokio::test]
    async fn rate_limit_response_is_reported_distinctly() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("x-ratelimit-remaining", "0")
                    .insert_header("retry-after", "30")
                    .set_body_string("rate limited"),
            )
            .mount(&server)
            .await;

        let client = GitHubSearchClient::new().with_base_url(server.uri());
        let err = client.search_by_topic("macos", 50).await.unwrap_err();

        assert!(matches!(
            err,
            DiscoveryError::RateLimited {
                retry_after_secs: Some(30)
            }
        ));
    }

    #[tokio::test]
    async fn non_rate_limit_forbidden_is_reported_as_a_provider_error_not_rate_limited() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden, unrelated"))
            .mount(&server)
            .await;

        let client = GitHubSearchClient::new().with_base_url(server.uri());
        let err = client.search_by_topic("macos", 50).await.unwrap_err();

        assert!(matches!(err, DiscoveryError::Provider(_)));
    }

    #[tokio::test]
    #[ignore = "hits the real GitHub Search API; run explicitly with \
                `cargo test -p genjux-core discovery::tests::real_github_search -- --ignored`"]
    async fn real_github_search_api_returns_sensibly_shaped_results_for_a_known_active_topic() {
        let client = GitHubSearchClient::from_env();
        let results = client
            .search_by_topic("macos-app", 5)
            .await
            .expect("real GitHub Search API request should succeed");

        assert!(
            !results.is_empty(),
            "expected at least one real repo tagged \"macos-app\""
        );
        // Real results should be sorted by stars descending, per the
        // sort=stars&order=desc query params this client always sends.
        for pair in results.windows(2) {
            assert!(
                pair[0].stars >= pair[1].stars,
                "expected descending star order, got {} then {}",
                pair[0].stars,
                pair[1].stars
            );
        }
    }
}
