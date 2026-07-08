//! Local HTTP/JSON API (issue #16).
//!
//! This is how the CLI (#20) and, later, native GUIs talk to the core
//! service — everything here is a thin wrapper over the business logic in
//! [`crate::source`], [`crate::package`], [`crate::registry`],
//! [`crate::audit`], and [`crate::orchestrate`]. AI agents get an
//! equivalent surface via the MCP server (#17), sharing the same
//! [`AppState`].
//!
//! **Phase 0 scope note**: only a single default [`crate::source::SourceProvider`]
//! is wired in (GitHub), matched against every request regardless of any
//! `provider` path segment — full multi-provider routing (per PLAN.md's
//! `SourceProvider`/`RepoRef` design, #28) is straightforward to add once
//! a second provider actually exists, but there's nothing to route
//! between yet. Auth/token-gating the local HTTP listener (see PLAN.md
//! open question about localhost API security) is also not done here —
//! that's service-lifecycle (#18) territory, since the token has to be
//! issued/shared by whatever process starts this server.

use crate::audit::AuditLog;
use crate::classify::Platform;
use crate::discovery::{
    DiscoveryCache, GitHubSearchClient, InMemoryDiscoveryCache, RecommendedApp,
};
use crate::orchestrate::{run_install, InstallStage, PlatformAdapter};
use crate::package::classify_release;
use crate::registry::InstalledAppRegistry;
use crate::source::{RepoRef, SourceProvider};
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a discovered recommended-software list stays valid before the
/// next request re-runs the search+classify pipeline (PLAN.md section
/// 6.1). 24 hours, matching the section's own suggested default.
const DISCOVERY_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Topics queried per platform for the recommended-software feed (#54/#55).
/// Deliberately small and easy to tune later — the quality gate that
/// actually matters (an asset must really classify as installable for the
/// platform) lives in [`crate::discovery::discover_recommended`], not here.
fn default_discovery_topics(platform: Platform) -> &'static [&'static str] {
    match platform {
        Platform::MacOS => &["macos", "macos-app", "menu-bar-app"],
        Platform::Windows => &["windows", "windows-app"],
        Platform::Linux => &["linux", "linux-app"],
        Platform::Android => &["android", "android-app"],
    }
}

/// Shared state behind every route, and reused as-is by the MCP server
/// (#17) so both surfaces see identical data.
pub struct AppState {
    pub source: Arc<dyn SourceProvider>,
    pub registry: Arc<dyn InstalledAppRegistry>,
    pub audit_log: Arc<dyn AuditLog>,
    pub adapter: Arc<dyn PlatformAdapter>,
    pub install_dir: PathBuf,
    /// In-progress/finished installs this process has started, keyed by a
    /// per-process install id. Deliberately in-memory only (unlike the
    /// registry/audit log): losing "what was in progress" on a restart is
    /// fine, since [`crate::registry`] is the durable record of what's
    /// actually installed.
    installs: Mutex<HashMap<String, InstallStage>>,
    next_install_id: AtomicU64,
    discovery_search_client: GitHubSearchClient,
    discovery_cache: Arc<dyn DiscoveryCache>,
}

impl AppState {
    pub fn new(
        source: Arc<dyn SourceProvider>,
        registry: Arc<dyn InstalledAppRegistry>,
        audit_log: Arc<dyn AuditLog>,
        adapter: Arc<dyn PlatformAdapter>,
        install_dir: PathBuf,
    ) -> Self {
        Self {
            source,
            registry,
            audit_log,
            adapter,
            install_dir,
            installs: Mutex::new(HashMap::new()),
            next_install_id: AtomicU64::new(1),
            discovery_search_client: GitHubSearchClient::from_env(),
            discovery_cache: Arc::new(InMemoryDiscoveryCache::new(DISCOVERY_CACHE_TTL)),
        }
    }

    /// Overrides the discovery search client and/or cache — used by tests
    /// to point the search step at a mock server and/or use a short-TTL
    /// cache, without changing every other `AppState::new` call site.
    #[cfg(test)]
    fn with_discovery(
        mut self,
        search_client: GitHubSearchClient,
        cache: Arc<dyn DiscoveryCache>,
    ) -> Self {
        self.discovery_search_client = search_client;
        self.discovery_cache = cache;
        self
    }

    fn allocate_install_id(&self) -> String {
        let n = self.next_install_id.fetch_add(1, Ordering::Relaxed);
        format!("install-{n}")
    }

    /// Looks up the release packages available for `owner/repo`,
    /// classified by platform/arch/kind. Shared by the HTTP API and the
    /// MCP server (#17) so both surfaces behave identically.
    pub async fn get_packages(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<crate::package::InstallablePackage>, GetPackagesError> {
        let repo_ref = RepoRef::new(self.source.provider_id(), owner, repo);
        let release = self
            .source
            .latest_release(&repo_ref)
            .await?
            .ok_or(GetPackagesError::NoReleases)?;
        Ok(classify_release(&release))
    }

    /// Returns the recommended-software feed for `platform` (#54/#55),
    /// serving from the TTL cache when available rather than re-running
    /// the (multi-repo-fetching) discovery pipeline on every call.
    pub async fn discover(
        &self,
        platform: Platform,
    ) -> Result<Vec<RecommendedApp>, crate::discovery::DiscoveryError> {
        let cache_key = platform.as_str();
        if let Some(cached) = self.discovery_cache.get(cache_key).await {
            return Ok(cached);
        }

        let topics = default_discovery_topics(platform);
        let recommended = crate::discovery::discover_recommended(
            &self.discovery_search_client,
            &*self.source,
            topics,
            platform,
            25,
        )
        .await?;

        self.discovery_cache
            .put(cache_key, recommended.clone())
            .await;
        Ok(recommended)
    }

    pub async fn list_installed(
        &self,
    ) -> Result<Vec<crate::registry::InstalledEntry>, crate::registry::RegistryError> {
        self.registry.list_installed().await
    }

    /// Compares every installed entry against its source's latest
    /// release, flagging outdated ones. Used by the CLI's `update`
    /// command (#20).
    pub async fn check_for_updates(
        &self,
    ) -> Result<Vec<crate::registry::UpdateCheckResult>, crate::registry::UpdateCheckError> {
        crate::registry::check_for_updates(&*self.registry, &*self.source).await
    }

    /// Starts a background install for `owner/repo`, returning an install
    /// id that [`Self::get_install_status`] can poll. Requires `Arc<Self>`
    /// since the background task holds its own clone of the state.
    pub fn start_install(self: &Arc<Self>, owner: String, repo: String) -> String {
        let install_id = self.allocate_install_id();
        let repo_ref = RepoRef::new(self.source.provider_id(), owner, repo);

        self.installs
            .lock()
            .expect("installs lock poisoned")
            .insert(install_id.clone(), InstallStage::Resolving);

        let state_for_task = self.clone();
        let id_for_task = install_id.clone();
        tokio::spawn(async move {
            // Ensure the destination directory exists before downloading
            // into it — a fresh machine won't have it yet, and
            // download_resumable doesn't create parent directories itself
            // (it just opens/creates the destination file).
            if let Err(e) = tokio::fs::create_dir_all(&state_for_task.install_dir).await {
                state_for_task
                    .installs
                    .lock()
                    .expect("installs lock poisoned")
                    .insert(
                        id_for_task.clone(),
                        InstallStage::Failed {
                            reason: format!("failed to create install directory: {e}"),
                        },
                    );
                return;
            }

            let installs_handle = &state_for_task.installs;
            let id_for_callback = id_for_task.clone();
            let result = run_install(
                &*state_for_task.source,
                &repo_ref,
                &state_for_task.install_dir,
                &*state_for_task.adapter,
                |stage| {
                    installs_handle
                        .lock()
                        .expect("installs lock poisoned")
                        .insert(id_for_callback.clone(), stage);
                },
            )
            .await;

            if result.is_ok() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let _ = state_for_task
                    .registry
                    .record_install(crate::registry::InstalledEntry {
                        repo: repo_ref,
                        installed_tag: "unknown".to_string(),
                        installed_at_unix: now,
                        source_url: String::new(),
                    })
                    .await;
            }
        });

        install_id
    }

    /// Returns the latest known stage of a previously started install, or
    /// `None` if `id` isn't recognized.
    pub fn get_install_status(&self, id: &str) -> Option<InstallStage> {
        self.installs
            .lock()
            .expect("installs lock poisoned")
            .get(id)
            .cloned()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GetPackagesError {
    #[error("no releases found for this repo")]
    NoReleases,
    #[error(transparent)]
    Source(#[from] crate::source::SourceError),
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/repos/:owner/:repo/packages", get(get_packages))
        .route("/discover/:platform", get(discover_platform))
        .route("/installed", get(list_installed))
        .route("/updates", get(get_updates))
        .route("/install", post(start_install))
        .route("/installs/:id", get(get_install_status))
        .with_state(state)
}

#[derive(serde::Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: crate::version(),
    })
}

async fn get_packages(
    State(state): State<Arc<AppState>>,
    AxumPath((owner, repo)): AxumPath<(String, String)>,
) -> Result<Json<Vec<crate::package::InstallablePackage>>, (StatusCode, String)> {
    state
        .get_packages(&owner, &repo)
        .await
        .map(Json)
        .map_err(|e| match &e {
            GetPackagesError::NoReleases => (StatusCode::NOT_FOUND, e.to_string()),
            GetPackagesError::Source(crate::source::SourceError::NotFound(_)) => {
                (StatusCode::NOT_FOUND, e.to_string())
            }
            _ => (StatusCode::BAD_GATEWAY, e.to_string()),
        })
}

async fn discover_platform(
    State(state): State<Arc<AppState>>,
    AxumPath(platform): AxumPath<String>,
) -> Result<Json<Vec<RecommendedApp>>, (StatusCode, String)> {
    let platform: Platform =
        platform
            .parse()
            .map_err(|e: crate::classify::ParsePlatformError| {
                (StatusCode::BAD_REQUEST, e.to_string())
            })?;

    state.discover(platform).await.map(Json).map_err(|e| {
        let status = match &e {
            crate::discovery::DiscoveryError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::BAD_GATEWAY,
        };
        (status, e.to_string())
    })
}

async fn list_installed(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<crate::registry::InstalledEntry>>, (StatusCode, String)> {
    state
        .list_installed()
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn get_updates(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<crate::registry::UpdateCheckResult>>, (StatusCode, String)> {
    state
        .check_for_updates()
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[derive(serde::Deserialize)]
struct InstallRequest {
    owner: String,
    repo: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct InstallStarted {
    install_id: String,
}

async fn start_install(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InstallRequest>,
) -> Json<InstallStarted> {
    let install_id = state.start_install(req.owner, req.repo);
    Json(InstallStarted { install_id })
}

async fn get_install_status(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<InstallStage>, StatusCode> {
    state
        .get_install_status(&id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::JsonlAuditLog;
    use crate::registry::JsonFileRegistry;
    use crate::source::mock::MockProvider;
    use crate::source::{Release, ReleaseAsset};
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use std::path::Path;
    use std::time::Duration;
    use tower::ServiceExt;

    struct NoopAdapter;

    #[async_trait]
    impl PlatformAdapter for NoopAdapter {
        async fn install(&self, _downloaded_file: &Path) -> Result<(), String> {
            Ok(())
        }
    }

    async fn test_state_with_release(
        repo: &RepoRef,
        release: Release,
    ) -> (Arc<AppState>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let provider = MockProvider::new().with_releases(repo.clone(), vec![release]);
        let registry = JsonFileRegistry::open(tmp.path().join("registry.json"))
            .await
            .unwrap();
        let audit_log = JsonlAuditLog::new(tmp.path().join("audit.jsonl"));

        let state = Arc::new(AppState::new(
            Arc::new(provider),
            Arc::new(registry),
            Arc::new(audit_log),
            Arc::new(NoopAdapter),
            tmp.path().join("installs"),
        ));
        (state, tmp)
    }

    #[tokio::test]
    async fn health_reports_ok_and_a_version() {
        let (state, _tmp) = test_state_with_release(
            &RepoRef::new("mock", "acme", "widget"),
            Release {
                tag: "v1.0.0".to_string(),
                assets: vec![],
            },
        )
        .await;
        let router = build_router(state);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert!(json["version"].is_string());
    }

    #[tokio::test]
    async fn get_packages_returns_classified_assets_for_a_known_repo() {
        let repo = RepoRef::new("mock", "acme", "widget");
        let (state, _tmp) = test_state_with_release(
            &repo,
            Release {
                tag: "v1.0.0".to_string(),
                assets: vec![ReleaseAsset {
                    name: "widget-arm64.dmg".to_string(),
                    size_bytes: 10,
                    download_url: "https://example.invalid/widget.dmg".to_string(),
                    content_type: None,
                }],
            },
        )
        .await;
        let router = build_router(state);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/repos/acme/widget/packages")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let packages: Vec<crate::package::InstallablePackage> =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].asset_name, "widget-arm64.dmg");
    }

    #[tokio::test]
    async fn get_packages_for_unknown_repo_returns_404() {
        let (state, _tmp) = test_state_with_release(
            &RepoRef::new("mock", "acme", "widget"),
            Release {
                tag: "v1.0.0".to_string(),
                assets: vec![],
            },
        )
        .await;
        let router = build_router(state);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/repos/nobody/nothing/packages")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn install_flow_reaches_succeeded_and_shows_up_in_installed_list() {
        let repo = RepoRef::new("mock", "acme", "widget");
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/widget.bin"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_bytes(b"contents".as_slice()),
            )
            .mount(&server)
            .await;

        let asset_name = match crate::classify::current_platform().unwrap() {
            crate::classify::Platform::MacOS => "widget.dmg",
            crate::classify::Platform::Windows => "widget.exe",
            crate::classify::Platform::Linux => "widget.AppImage",
            crate::classify::Platform::Android => "widget.apk",
        };
        // Re-mount under the platform-specific name so classification
        // picks it up regardless of which CI runner OS this test is on.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(format!("/{asset_name}")))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_bytes(b"contents".as_slice()),
            )
            .mount(&server)
            .await;

        let (state, _tmp) = test_state_with_release(
            &repo,
            Release {
                tag: "v1.0.0".to_string(),
                assets: vec![ReleaseAsset {
                    name: asset_name.to_string(),
                    size_bytes: 8,
                    download_url: format!("{}/{asset_name}", server.uri()),
                    content_type: None,
                }],
            },
        )
        .await;
        let router = build_router(state.clone());

        let start_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/install")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"owner":"acme","repo":"widget"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(start_response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(start_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let started: InstallStarted = serde_json::from_slice(&body).unwrap();

        // Poll until the background install reaches a terminal stage,
        // bounded so a real bug (e.g. a hang) fails the test instead of
        // blocking CI forever.
        let mut final_stage = None;
        for _ in 0..200 {
            let status_response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/installs/{}", started.install_id))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = axum::body::to_bytes(status_response.into_body(), usize::MAX)
                .await
                .unwrap();
            let stage: InstallStage = serde_json::from_slice(&body).unwrap();
            if matches!(stage, InstallStage::Succeeded | InstallStage::Failed { .. }) {
                final_stage = Some(stage);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(final_stage, Some(InstallStage::Succeeded));

        let installed_response = router
            .oneshot(
                Request::builder()
                    .uri("/installed")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(installed_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let installed: Vec<crate::registry::InstalledEntry> =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].repo, repo);
    }

    #[tokio::test]
    async fn unknown_install_id_returns_404() {
        let (state, _tmp) = test_state_with_release(
            &RepoRef::new("mock", "acme", "widget"),
            Release {
                tag: "v1.0.0".to_string(),
                assets: vec![],
            },
        )
        .await;
        let router = build_router(state);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/installs/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn updates_endpoint_flags_an_outdated_installed_entry() {
        let repo = RepoRef::new("mock", "acme", "widget");
        let (state, _tmp) = test_state_with_release(
            &repo,
            Release {
                tag: "v2.0.0".to_string(),
                assets: vec![],
            },
        )
        .await;
        state
            .registry
            .record_install(crate::registry::InstalledEntry {
                repo: repo.clone(),
                installed_tag: "v1.0.0".to_string(),
                installed_at_unix: 0,
                source_url: String::new(),
            })
            .await
            .unwrap();

        let router = build_router(state);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/updates")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let updates: Vec<crate::registry::UpdateCheckResult> =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(updates.len(), 1);
        assert!(updates[0].update_available);
        assert_eq!(updates[0].latest_tag, "v2.0.0");
    }

    fn discovery_search_response(items: &[(&str, &str, u64)]) -> serde_json::Value {
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

    async fn test_state_with_discovery(
        search_server_uri: String,
        provider: MockProvider,
    ) -> Arc<AppState> {
        let tmp = tempfile::tempdir().unwrap();
        let registry = JsonFileRegistry::open(tmp.path().join("registry.json"))
            .await
            .unwrap();
        let audit_log = JsonlAuditLog::new(tmp.path().join("audit.jsonl"));

        let state = AppState::new(
            Arc::new(provider),
            Arc::new(registry),
            Arc::new(audit_log),
            Arc::new(NoopAdapter),
            tmp.path().join("installs"),
        )
        .with_discovery(
            crate::discovery::GitHubSearchClient::new().with_base_url(search_server_uri),
            Arc::new(crate::discovery::InMemoryDiscoveryCache::new(
                Duration::from_secs(60),
            )),
        );
        // Keep the temp dir alive for the process lifetime of the test by
        // leaking it — these are short-lived unit test processes, and
        // `test_state_with_release` above already returns the TempDir to
        // its caller for the same reason; here it's simpler to just leak
        // since no test needs to inspect the directory afterward.
        std::mem::forget(tmp);
        Arc::new(state)
    }

    #[tokio::test]
    async fn discover_endpoint_returns_only_installable_candidates_for_the_platform() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let search_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(discovery_search_response(&[
                    ("alice", "mac-app", 500),
                    ("bob", "linux-only", 100),
                ])),
            )
            .mount(&search_server)
            .await;

        let provider = MockProvider::new()
            .with_releases(
                RepoRef::new("mock", "alice", "mac-app"),
                vec![Release {
                    tag: "v1.0.0".to_string(),
                    assets: vec![ReleaseAsset {
                        name: "app-x86_64-apple-darwin.tar.gz".to_string(),
                        size_bytes: 1,
                        download_url: "https://example.invalid/a".to_string(),
                        content_type: None,
                    }],
                }],
            )
            .with_releases(
                RepoRef::new("mock", "bob", "linux-only"),
                vec![Release {
                    tag: "v1.0.0".to_string(),
                    assets: vec![ReleaseAsset {
                        name: "app_amd64.deb".to_string(),
                        size_bytes: 1,
                        download_url: "https://example.invalid/b".to_string(),
                        content_type: None,
                    }],
                }],
            );

        let state = test_state_with_discovery(search_server.uri(), provider).await;
        let router = build_router(state);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/discover/macos")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let recommended: Vec<crate::discovery::RecommendedApp> =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(recommended.len(), 1);
        assert_eq!(recommended[0].owner, "alice");
    }

    #[tokio::test]
    async fn discover_endpoint_rejects_an_unknown_platform_with_bad_request() {
        let state = test_state_with_discovery(
            "http://127.0.0.1:1".to_string(), // never actually reached
            MockProvider::new(),
        )
        .await;
        let router = build_router(state);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/discover/plan9")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn discover_endpoint_serves_the_second_request_from_cache() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let search_server = MockServer::start().await;
        // macOS discovery queries 3 topics (macos/macos-app/menu-bar-app —
        // see default_discovery_topics), so a single discover() call makes
        // 3 search requests. `.expect(3)` fails the test (on drop) if the
        // search endpoint is hit more than 3 times total across both HTTP
        // calls below — proving the second /discover/macos request is
        // actually served from the TTL cache, not just documented as such.
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(discovery_search_response(&[("alice", "mac-app", 500)])),
            )
            .expect(3)
            .mount(&search_server)
            .await;

        let provider = MockProvider::new().with_releases(
            RepoRef::new("mock", "alice", "mac-app"),
            vec![Release {
                tag: "v1.0.0".to_string(),
                assets: vec![ReleaseAsset {
                    name: "app-arm64.dmg".to_string(),
                    size_bytes: 1,
                    download_url: "https://example.invalid/a".to_string(),
                    content_type: None,
                }],
            }],
        );

        let state = test_state_with_discovery(search_server.uri(), provider).await;
        let router = build_router(state);

        for _ in 0..2 {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/discover/macos")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        // Dropping the MockServer verifies the `.expect(3)` assertion.
        drop(search_server);
    }
}
