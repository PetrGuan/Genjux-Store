//! `genjuxd`: the Genjux-Store core service binary.
//!
//! Wires together everything the Phase 0 issues built into one running
//! process: a singleton lock + local auth token (#18), the HTTP API (#16)
//! and MCP server (#17) sharing one `AppState`, and an idle-shutdown
//! watcher (#18). See `.copilot-workflow/PLAN.md` sections 1-2 for the
//! overall architecture this implements.

use genjux_core::api::{self, AppState};
use genjux_core::audit::JsonlAuditLog;
use genjux_core::lifecycle::{self, AcquireOutcome, ActivityTracker, ServiceInfo};
use genjux_core::mcp::build_mcp_router;
use genjux_core::platform;
use genjux_core::registry::JsonFileRegistry;
use genjux_core::source::github::GitHubProvider;
use std::sync::Arc;
use std::time::Duration;

/// How long the service waits with no requests before shutting itself
/// down. A future issue may make this configurable; a fixed default is a
/// reasonable Phase 0 starting point.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Waits for SIGTERM on Unix; never resolves on other platforms (Windows
/// has no equivalent signal in the same sense — Ctrl-C, already handled
/// separately, is the primary interactive-stop mechanism there).
#[cfg(unix)]
async fn wait_for_sigterm() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    term.recv().await;
}

#[cfg(not(unix))]
async fn wait_for_sigterm() {
    std::future::pending::<()>().await;
}

#[tokio::main]
async fn main() {
    let mut lock = match lifecycle::try_acquire_singleton_lock() {
        Ok(AcquireOutcome::Acquired(lock)) => lock,
        Ok(AcquireOutcome::AlreadyRunning(info)) => {
            println!("genjuxd is already running on 127.0.0.1:{}", info.port);
            return;
        }
        Err(e) => {
            eprintln!("genjuxd: failed to acquire singleton lock: {e}");
            std::process::exit(1);
        }
    };

    let data_dir = lifecycle::runtime_dir();
    let registry = JsonFileRegistry::open(data_dir.join("registry.json"))
        .await
        .expect("genjuxd: failed to open the installed-app registry");
    let audit_log = JsonlAuditLog::new(data_dir.join("audit.jsonl"));
    let adapter = platform::current_adapter(data_dir.join("installed-apps"));
    let source = GitHubProvider::from_env();

    let state = Arc::new(AppState::new(
        Arc::new(source),
        Arc::new(registry),
        Arc::new(audit_log),
        adapter,
        data_dir.join("downloads"),
    ));

    let token = lifecycle::generate_token();
    let token_for_middleware = Arc::new(token.clone());
    let activity = ActivityTracker::new();

    let router = api::build_router(state.clone())
        .merge(build_mcp_router(state))
        .layer(axum::middleware::from_fn({
            let activity = activity.clone();
            move |req, next| {
                activity.touch();
                let token = token_for_middleware.clone();
                async move { lifecycle::auth_middleware(token, req, next).await }
            }
        }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("genjuxd: failed to bind a local port");
    let port = listener
        .local_addr()
        .expect("genjuxd: failed to read the bound address")
        .port();

    lock.publish(&ServiceInfo { port, token })
        .expect("genjuxd: failed to publish service info");

    let shutdown = tokio_util::sync::CancellationToken::new();
    let idle_watcher = tokio::spawn(lifecycle::run_idle_shutdown_watcher(
        activity,
        IDLE_TIMEOUT,
        IDLE_POLL_INTERVAL,
        shutdown.clone(),
    ));

    println!("genjuxd listening on 127.0.0.1:{port}");

    let shutdown_for_signal = shutdown.clone();
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = shutdown_for_signal.cancelled() => {}
                _ = tokio::signal::ctrl_c() => { shutdown_for_signal.cancel(); }
                // SIGTERM is the standard "please stop" signal a plain
                // `kill <pid>` sends on Unix (as opposed to `kill -9`,
                // which can never be caught). Without handling it
                // explicitly, Rust does not run destructors on a raw
                // SIGTERM, so the lock file would be left behind with
                // stale contents (the OS-level advisory lock itself is
                // still released correctly either way — verified
                // manually against a real SIGKILL — but leaving a stale
                // file around is an avoidable rough edge for the common
                // graceful-stop case).
                _ = wait_for_sigterm() => { shutdown_for_signal.cancel(); }
            }
        })
        .await
        .expect("genjuxd: server error");

    idle_watcher.abort();
    drop(lock); // releases the singleton lock and removes the lock file
}
