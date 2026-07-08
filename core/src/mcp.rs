//! MCP server (issue #17): exposes the same capabilities as the HTTP API
//! (#16) to AI agents via the Model Context Protocol, sharing the same
//! [`AppState`] so both surfaces behave identically and see the same
//! install progress / registry state.
//!
//! **Phase 0 scope note**: `search_software` is a direct owner/repo
//! lookup, not a fuzzy/keyword search — there's no aggregated catalog of
//! repos to search across yet (that's a discovery/recommendation feature
//! for a later phase). The tool name matches PLAN.md's original naming so
//! it's ready to become real search once a catalog exists, without an API
//! rename.

use crate::api::AppState;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use std::sync::Arc;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchSoftwareArgs {
    /// Repo owner, e.g. "rust-lang".
    pub owner: String,
    /// Repo name, e.g. "rust".
    pub repo: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct InstallSoftwareArgs {
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetInstallStatusArgs {
    pub install_id: String,
}

#[derive(Clone)]
pub struct GenjuxMcpServer {
    state: Arc<AppState>,
    tool_router: ToolRouter<GenjuxMcpServer>,
}

#[tool_router]
impl GenjuxMcpServer {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Look up the release packages available for a repo owner/name, classified by platform/architecture/install-kind. This is a direct lookup (Phase 0 has no aggregated catalog to fuzzy-search across yet), not a keyword search."
    )]
    async fn search_software(
        &self,
        Parameters(args): Parameters<SearchSoftwareArgs>,
    ) -> Result<CallToolResult, McpError> {
        match self.state.get_packages(&args.owner, &args.repo).await {
            Ok(packages) => {
                let json = serde_json::to_string_pretty(&packages)
                    .unwrap_or_else(|e| format!("failed to serialize packages: {e}"));
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    #[tool(
        description = "Start installing the release asset matching the current platform for a repo owner/name. Returns an install_id; poll it with get_install_status."
    )]
    async fn install_software(
        &self,
        Parameters(args): Parameters<InstallSoftwareArgs>,
    ) -> Result<CallToolResult, McpError> {
        let install_id = self.state.start_install(args.owner, args.repo);
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::json!({ "install_id": install_id }).to_string(),
        )]))
    }

    #[tool(description = "List apps Genjux-Store has installed on this machine.")]
    async fn list_installed(&self) -> Result<CallToolResult, McpError> {
        match self.state.list_installed().await {
            Ok(entries) => {
                let json = serde_json::to_string_pretty(&entries).unwrap_or_default();
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    #[tool(
        description = "Poll the current stage of a previously started install by its install_id."
    )]
    async fn get_install_status(
        &self,
        Parameters(args): Parameters<GetInstallStatusArgs>,
    ) -> Result<CallToolResult, McpError> {
        match self.state.get_install_status(&args.install_id) {
            Some(stage) => {
                let json = serde_json::to_string_pretty(&stage).unwrap_or_default();
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            None => Ok(CallToolResult::error(vec![Content::text(format!(
                "unknown install_id: {}",
                args.install_id
            ))])),
        }
    }
}

#[tool_handler]
impl ServerHandler for GenjuxMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Genjux-Store: discover and install open-source software. Tools: \
                 search_software (owner/repo lookup), install_software (start an install), \
                 list_installed, get_install_status (poll progress)."
                    .to_string(),
            ),
        }
    }
}

/// Mounts the MCP server at `/mcp` on its own [`axum::Router`], suitable
/// for merging into the same router (and same listening port) as the HTTP
/// API's (#16) — matching PLAN.md's design of a single core service
/// process exposing both surfaces. Actually starting the combined server
/// (binding a port, idle shutdown, etc.) is service-lifecycle (#18)
/// territory; this function only does the routing wiring.
pub fn build_mcp_router(state: Arc<AppState>) -> axum::Router {
    let service = StreamableHttpService::new(
        move || Ok(GenjuxMcpServer::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    axum::Router::new().nest_service("/mcp", service)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::JsonlAuditLog;
    use crate::orchestrate::PlatformAdapter;
    use crate::registry::JsonFileRegistry;
    use crate::source::mock::MockProvider;
    use crate::source::{Release, ReleaseAsset, RepoRef};
    use async_trait::async_trait;
    use std::path::Path;

    struct NoopAdapter;

    #[async_trait]
    impl PlatformAdapter for NoopAdapter {
        async fn install(&self, _downloaded_file: &Path) -> Result<(), String> {
            Ok(())
        }
    }

    async fn test_server_with_release(
        repo: &RepoRef,
        release: Release,
    ) -> (GenjuxMcpServer, tempfile::TempDir) {
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
        (GenjuxMcpServer::new(state), tmp)
    }

    fn content_text(result: &CallToolResult) -> String {
        result
            .content
            .first()
            .and_then(|block| block.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn search_software_returns_classified_packages() {
        let repo = RepoRef::new("mock", "acme", "widget");
        let (server, _tmp) = test_server_with_release(
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

        let result = server
            .search_software(Parameters(SearchSoftwareArgs {
                owner: "acme".to_string(),
                repo: "widget".to_string(),
            }))
            .await
            .unwrap();

        assert_ne!(result.is_error, Some(true));
        assert!(content_text(&result).contains("widget-arm64.dmg"));
    }

    #[tokio::test]
    async fn search_software_for_unknown_repo_returns_an_error_result_not_a_transport_error() {
        let (server, _tmp) = test_server_with_release(
            &RepoRef::new("mock", "acme", "widget"),
            Release {
                tag: "v1.0.0".to_string(),
                assets: vec![],
            },
        )
        .await;

        let result = server
            .search_software(Parameters(SearchSoftwareArgs {
                owner: "nobody".to_string(),
                repo: "nothing".to_string(),
            }))
            .await
            .unwrap();

        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn install_software_returns_an_install_id_that_get_install_status_can_poll() {
        let repo = RepoRef::new("mock", "acme", "widget");
        let (server, _tmp) = test_server_with_release(
            &repo,
            Release {
                tag: "v1.0.0".to_string(),
                assets: vec![],
            },
        )
        .await;

        let start_result = server
            .install_software(Parameters(InstallSoftwareArgs {
                owner: "acme".to_string(),
                repo: "widget".to_string(),
            }))
            .await
            .unwrap();
        let started: serde_json::Value =
            serde_json::from_str(&content_text(&start_result)).unwrap();
        let install_id = started["install_id"].as_str().unwrap().to_string();

        let status_result = server
            .get_install_status(Parameters(GetInstallStatusArgs {
                install_id: install_id.clone(),
            }))
            .await
            .unwrap();
        assert_ne!(status_result.is_error, Some(true));
        assert!(content_text(&status_result).contains("stage"));
    }

    #[tokio::test]
    async fn get_install_status_for_unknown_id_returns_an_error_result() {
        let (server, _tmp) = test_server_with_release(
            &RepoRef::new("mock", "acme", "widget"),
            Release {
                tag: "v1.0.0".to_string(),
                assets: vec![],
            },
        )
        .await;

        let result = server
            .get_install_status(Parameters(GetInstallStatusArgs {
                install_id: "does-not-exist".to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn list_installed_starts_empty() {
        let (server, _tmp) = test_server_with_release(
            &RepoRef::new("mock", "acme", "widget"),
            Release {
                tag: "v1.0.0".to_string(),
                assets: vec![],
            },
        )
        .await;

        let result = server.list_installed().await.unwrap();
        assert_eq!(content_text(&result).trim(), "[]");
    }

    /// Genuine end-to-end test: binds `build_mcp_router` on a real
    /// ephemeral TCP port, connects a real `rmcp` streamable-HTTP client
    /// to it, and confirms the MCP protocol handshake completes and all
    /// four tools are actually registered and reachable over the wire —
    /// not just callable as plain Rust methods (the other tests above
    /// only exercise the tool methods directly, which proves the tool
    /// *logic* but not that they're correctly wired into the transport).
    #[tokio::test]
    async fn mcp_endpoint_is_reachable_over_real_http_and_exposes_its_tools() {
        use rmcp::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let repo = RepoRef::new("mock", "acme", "widget");
        let provider = MockProvider::new().with_releases(
            repo.clone(),
            vec![Release {
                tag: "v1.0.0".to_string(),
                assets: vec![],
            }],
        );
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

        let router = build_mcp_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let transport =
            rmcp::transport::streamable_http_client::StreamableHttpClientTransport::from_uri(
                format!("http://{addr}/mcp"),
            );
        let client = ()
            .serve(transport)
            .await
            .expect("client should connect and complete the MCP initialize handshake");

        let tools = client
            .list_all_tools()
            .await
            .expect("tools/list should succeed");
        let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();

        for expected in [
            "search_software",
            "install_software",
            "list_installed",
            "get_install_status",
        ] {
            assert!(
                tool_names.contains(&expected.to_string()),
                "expected tool {expected:?} in {tool_names:?}"
            );
        }

        client
            .cancel()
            .await
            .expect("client should shut down cleanly");
    }
}
