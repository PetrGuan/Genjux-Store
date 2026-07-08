//! GitHub Search API-based discovery of candidate repos for the
//! recommended-software feed (issues #54/#55, Phase 1 macOS GUI — see
//! PLAN.md section 6.1).
//!
//! Two layers, kept independently testable:
//! 1. [`GitHubSearchClient`] (#54) is a thin, GitHub-specific search
//!    client: it finds *candidates* by topic, sorted by stars, and
//!    nothing more. It does **not** know anything about installability.
//! 2. [`discover_recommended`] (#55) layers the actual quality gate on
//!    top: it runs each candidate's latest release through the existing
//!    [`crate::package::classify_release`] pipeline and keeps only the
//!    ones with a real installable asset for the platform being asked
//!    about — turning "GitHub Search result quality is uncontrollable"
//!    into "at least guaranteed installable". [`DiscoveryCache`] wraps
//!    that (expensive, multi-repo-fetching) pipeline with a TTL so a
//!    recommended-feed request doesn't re-run it on every call.

use crate::classify::Platform;
use crate::package::classify_release;
use crate::source::{RepoRef, SourceError, SourceProvider};
use async_trait::async_trait;
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

    /// Overrides the search API base URL. Only exposed under `#[cfg(test)]`
    /// (`pub(crate)` so other modules' tests, like `api.rs`'s, can point
    /// this client at a wiremock server too — not part of the public API).
    #[cfg(test)]
    pub(crate) fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
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

/// A discovery candidate that has cleared the installability quality gate:
/// its latest release has at least one asset that classifies as installable
/// for the platform being asked about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecommendedApp {
    pub owner: String,
    pub repo: String,
    pub stars: u64,
    pub description: Option<String>,
    pub release_tag: String,
    pub package: crate::package::InstallablePackage,
}

/// Searches `topics` via `search_client`, then filters candidates down to
/// only those whose latest release (fetched via `source_provider`) has at
/// least one asset that [`classify_release`] resolves to `platform`. This
/// is the quality gate described in PLAN.md section 6.1: GitHub Search
/// result relevance/quality can't be controlled, but "does it actually
/// have something installable for this platform" can be verified for real
/// using the same classification pipeline the rest of Genjux-Store relies
/// on — so a recommended-software feed never points a user at a pure
/// library or source-only repo.
///
/// A candidate that fails to fetch (no releases, not found, transient
/// network error) is simply skipped rather than failing the whole batch —
/// one bad candidate in a list of dozens shouldn't take down the entire
/// feed. A [`SourceError::RateLimited`] is different: once the source is
/// rate limiting us, every subsequent per-candidate fetch would fail the
/// same way, so it's propagated immediately instead of being silently
/// swallowed candidate-by-candidate.
pub async fn discover_recommended<P: SourceProvider + ?Sized>(
    search_client: &GitHubSearchClient,
    source_provider: &P,
    topics: &[&str],
    platform: Platform,
    per_page_per_topic: u32,
) -> Result<Vec<RecommendedApp>, DiscoveryError> {
    let candidates = search_client
        .search_by_topics(topics, per_page_per_topic)
        .await?;

    let mut recommended = Vec::new();
    for candidate in candidates {
        let repo_ref = RepoRef::new(
            source_provider.provider_id(),
            candidate.owner.clone(),
            candidate.repo.clone(),
        );
        let release = match source_provider.latest_release(&repo_ref).await {
            Ok(Some(release)) => release,
            Ok(None) => continue,
            Err(SourceError::RateLimited {
                retry_after_secs, ..
            }) => {
                return Err(DiscoveryError::RateLimited { retry_after_secs });
            }
            Err(_) => continue,
        };

        let Some(package) = classify_release(&release)
            .into_iter()
            .find(|p| p.classification.platform == Some(platform))
        else {
            continue;
        };

        recommended.push(RecommendedApp {
            owner: candidate.owner,
            repo: candidate.repo,
            stars: candidate.stars,
            description: candidate.description,
            release_tag: release.tag,
            package,
        });
    }

    Ok(recommended)
}

/// Caches the (expensive — one search plus one release-fetch-and-classify
/// per candidate) output of [`discover_recommended`], keyed by whatever the
/// caller chooses (e.g. a platform name), with a time-to-live.
#[async_trait]
pub trait DiscoveryCache: Send + Sync {
    async fn get(&self, key: &str) -> Option<Vec<RecommendedApp>>;
    async fn put(&self, key: &str, apps: Vec<RecommendedApp>);
}

/// A simple process-local, in-memory [`DiscoveryCache`] with a fixed TTL.
/// Sufficient for Phase 1; not persisted across restarts of the core
/// service (a cold-started service just re-runs discovery once).
pub struct InMemoryDiscoveryCache {
    ttl: std::time::Duration,
    entries: std::sync::Mutex<
        std::collections::HashMap<String, (std::time::Instant, Vec<RecommendedApp>)>,
    >,
}

impl InMemoryDiscoveryCache {
    pub fn new(ttl: std::time::Duration) -> Self {
        Self {
            ttl,
            entries: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

#[async_trait]
impl DiscoveryCache for InMemoryDiscoveryCache {
    async fn get(&self, key: &str) -> Option<Vec<RecommendedApp>> {
        let entries = self.entries.lock().expect("discovery cache lock poisoned");
        entries.get(key).and_then(|(inserted_at, apps)| {
            if inserted_at.elapsed() < self.ttl {
                Some(apps.clone())
            } else {
                None
            }
        })
    }

    async fn put(&self, key: &str, apps: Vec<RecommendedApp>) {
        let mut entries = self.entries.lock().expect("discovery cache lock poisoned");
        entries.insert(key.to_string(), (std::time::Instant::now(), apps));
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

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use crate::source::mock::MockProvider;
    use crate::source::{Release, ReleaseAsset};
    use async_trait::async_trait;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn search_response_body(items: &[(&str, &str, u64)]) -> serde_json::Value {
        serde_json::json!({
            "total_count": items.len(),
            "incomplete_results": false,
            "items": items.iter().map(|(owner, repo, stars)| {
                serde_json::json!({
                    "name": repo,
                    "owner": { "login": owner },
                    "stargazers_count": stars,
                    "description": null,
                })
            }).collect::<Vec<_>>(),
        })
    }

    fn release_with_asset(tag: &str, asset_name: &str) -> Release {
        Release {
            tag: tag.to_string(),
            assets: vec![ReleaseAsset {
                name: asset_name.to_string(),
                size_bytes: 42,
                download_url: format!("https://example.invalid/{asset_name}"),
                content_type: None,
            }],
        }
    }

    async fn mock_search_server(items: &[(&str, &str, u64)]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(search_response_body(items)))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn discover_recommended_keeps_only_candidates_with_a_matching_platform_asset() {
        let server =
            mock_search_server(&[("alice", "mac-app", 500), ("bob", "linux-only", 100)]).await;
        let search_client = GitHubSearchClient::new().with_base_url(server.uri());

        let provider = MockProvider::new()
            .with_releases(
                RepoRef::new("mock", "alice", "mac-app"),
                vec![release_with_asset(
                    "v1.0.0",
                    "app-x86_64-apple-darwin.tar.gz",
                )],
            )
            .with_releases(
                RepoRef::new("mock", "bob", "linux-only"),
                vec![release_with_asset("v1.0.0", "app_amd64.deb")],
            );

        let recommended =
            discover_recommended(&search_client, &provider, &["macos"], Platform::MacOS, 50)
                .await
                .unwrap();

        assert_eq!(recommended.len(), 1);
        assert_eq!(recommended[0].owner, "alice");
        assert_eq!(recommended[0].repo, "mac-app");
        assert_eq!(
            recommended[0].package.classification.platform,
            Some(Platform::MacOS)
        );
    }

    #[tokio::test]
    async fn discover_recommended_skips_candidates_with_no_releases_without_failing_the_batch() {
        let server =
            mock_search_server(&[("alice", "no-releases", 500), ("bob", "mac-app", 100)]).await;
        let search_client = GitHubSearchClient::new().with_base_url(server.uri());

        // "alice/no-releases" is deliberately absent from the provider's
        // known repos, so latest_release() resolves to a NotFound error
        // for it — that must be skipped, not propagated.
        let provider = MockProvider::new().with_releases(
            RepoRef::new("mock", "bob", "mac-app"),
            vec![release_with_asset("v1.0.0", "app-arm64.dmg")],
        );

        let recommended =
            discover_recommended(&search_client, &provider, &["macos"], Platform::MacOS, 50)
                .await
                .unwrap();

        assert_eq!(recommended.len(), 1);
        assert_eq!(recommended[0].owner, "bob");
    }

    struct AlwaysRateLimitedProvider;

    #[async_trait]
    impl SourceProvider for AlwaysRateLimitedProvider {
        fn provider_id(&self) -> &'static str {
            "mock"
        }

        async fn list_releases(&self, _repo: &RepoRef) -> Result<Vec<Release>, SourceError> {
            Err(SourceError::RateLimited {
                provider: "mock",
                retry_after_secs: Some(60),
            })
        }
    }

    #[tokio::test]
    async fn discover_recommended_propagates_rate_limit_errors_instead_of_skipping() {
        let server = mock_search_server(&[("alice", "widget", 500)]).await;
        let search_client = GitHubSearchClient::new().with_base_url(server.uri());
        let provider = AlwaysRateLimitedProvider;

        let err = discover_recommended(&search_client, &provider, &["macos"], Platform::MacOS, 50)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            DiscoveryError::RateLimited {
                retry_after_secs: Some(60)
            }
        ));
    }

    fn sample_recommended(owner: &str) -> Vec<RecommendedApp> {
        vec![RecommendedApp {
            owner: owner.to_string(),
            repo: "widget".to_string(),
            stars: 100,
            description: None,
            release_tag: "v1.0.0".to_string(),
            package: crate::package::InstallablePackage {
                asset_name: "widget.dmg".to_string(),
                download_url: "https://example.invalid/widget.dmg".to_string(),
                size_bytes: 1,
                classification: crate::classify::Classification {
                    platform: Some(Platform::MacOS),
                    arch: None,
                    kind: Some(crate::classify::AssetKind::Dmg),
                },
                sha256: None,
                min_os_version: None,
                silent_install_args: None,
            },
        }]
    }

    #[tokio::test]
    async fn in_memory_discovery_cache_hits_within_ttl_and_expires_after_it() {
        let cache = InMemoryDiscoveryCache::new(std::time::Duration::from_millis(50));

        assert_eq!(cache.get("macos").await, None);

        cache.put("macos", sample_recommended("alice")).await;
        assert_eq!(cache.get("macos").await, Some(sample_recommended("alice")));

        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        assert_eq!(
            cache.get("macos").await,
            None,
            "entry should have expired after the TTL elapsed"
        );
    }

    #[tokio::test]
    async fn in_memory_discovery_cache_keys_are_independent() {
        let cache = InMemoryDiscoveryCache::new(std::time::Duration::from_secs(60));
        cache.put("macos", sample_recommended("alice")).await;

        assert_eq!(cache.get("windows").await, None);
        assert!(cache.get("macos").await.is_some());
    }

    #[tokio::test]
    #[ignore = "hits the real GitHub Search API and REST API; run explicitly with \
                `cargo test -p genjux-core discovery::pipeline_tests::real_discover_recommended -- --ignored`"]
    async fn real_discover_recommended_finds_at_least_one_genuinely_installable_macos_app() {
        use crate::source::github::GitHubProvider;

        let search_client = GitHubSearchClient::from_env();
        let source_provider = GitHubProvider::from_env();

        let recommended = discover_recommended(
            &search_client,
            &source_provider,
            &["macos-app"],
            Platform::MacOS,
            10,
        )
        .await
        .expect("real discovery pipeline should succeed");

        assert!(
            !recommended.is_empty(),
            "expected at least one real macos-app-tagged repo with a real installable macOS asset"
        );
        for app in &recommended {
            assert_eq!(app.package.classification.platform, Some(Platform::MacOS));
        }
    }
}
