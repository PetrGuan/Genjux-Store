//! `genjux`: the Genjux-Store command-line client (issue #20).
//!
//! A thin client of the core service's local HTTP API (#16) — all
//! business logic lives in `genjux-core`; this crate is just argument
//! parsing, lazily starting `genjuxd` if it isn't already running, and
//! formatting responses for a terminal.

use clap::{Parser, Subcommand};
use genjux_core::lifecycle::{self, AcquireOutcome, ServiceInfo};
use genjux_core::orchestrate::InstallStage;
use genjux_core::package::InstallablePackage;
use genjux_core::registry::{InstalledEntry, UpdateCheckResult};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "genjux",
    version,
    about = "Discover and install open-source software."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Look up release packages for a repo, e.g. `genjux search owner/repo`.
    Search { repo: String },
    /// Install the release asset matching this platform for a repo.
    Install { repo: String },
    /// List apps installed via Genjux-Store.
    List,
    /// Check installed apps against their latest releases.
    Update,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Validate arguments before triggering the (potentially expensive —
    // it may spawn a whole new process) lazy service start, so a typo'd
    // repo spec fails fast instead of paying that cost first.
    let repo_spec = match &cli.command {
        Command::Search { repo } | Command::Install { repo } => match parse_repo(repo) {
            Ok(parsed) => Some(parsed),
            Err(e) => {
                eprintln!("genjux: {e}");
                std::process::exit(1);
            }
        },
        Command::List | Command::Update => None,
    };

    let service = match ensure_service_running().await {
        Ok(info) => info,
        Err(e) => {
            eprintln!("genjux: failed to reach the core service: {e}");
            std::process::exit(1);
        }
    };

    let client = reqwest::Client::new();
    let base_url = format!("http://127.0.0.1:{}", service.port);

    let result = match cli.command {
        Command::Search { .. } => {
            let (owner, repo) = repo_spec.expect("validated above");
            run_search(&client, &base_url, &service.token, &owner, &repo).await
        }
        Command::Install { .. } => {
            let (owner, repo) = repo_spec.expect("validated above");
            run_install(&client, &base_url, &service.token, &owner, &repo).await
        }
        Command::List => run_list(&client, &base_url, &service.token).await,
        Command::Update => run_update(&client, &base_url, &service.token).await,
    };

    if let Err(e) = result {
        eprintln!("genjux: {e}");
        std::process::exit(1);
    }
}

fn parse_repo(spec: &str) -> Result<(String, String), String> {
    match spec.split_once('/') {
        Some((owner, repo)) if !owner.is_empty() && !repo.is_empty() => {
            Ok((owner.to_string(), repo.to_string()))
        }
        _ => Err(format!("expected \"owner/repo\", got {spec:?}")),
    }
}

/// Finds a `genjuxd` (or `genjuxd.exe`) binary next to this one — the
/// expected layout for how the two are built/shipped together.
fn locate_genjuxd() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("failed to locate this binary: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "this binary has no parent directory".to_string())?;
    let name = if cfg!(windows) {
        "genjuxd.exe"
    } else {
        "genjuxd"
    };
    let path = dir.join(name);
    if path.exists() {
        Ok(path)
    } else {
        Err(format!(
            "expected to find {name} alongside this binary at {}, but it doesn't exist",
            path.display()
        ))
    }
}

/// Ensures the core service is reachable: if one is already running
/// (discovered via the same singleton-lock/discovery-file mechanism
/// `genjuxd` itself uses, #18), reuses it; otherwise, spawns a fresh
/// `genjuxd` and waits for it to publish its info file.
///
/// **Known Phase 0 limitation**: the spawned `genjuxd` is not a fully
/// detached daemon (no `setsid`/double-fork). It will normally keep
/// running after this CLI process exits, but closing the terminal
/// session that launched it could still send it a SIGHUP on Unix.
/// Full daemonization is out of scope for this issue; revisit if this
/// proves to matter in practice.
async fn ensure_service_running() -> Result<ServiceInfo, String> {
    match lifecycle::try_acquire_singleton_lock() {
        Ok(AcquireOutcome::AlreadyRunning(info)) => return Ok(info),
        Ok(AcquireOutcome::Acquired(lock)) => {
            // Nobody was running. Release immediately so the genjuxd we're
            // about to spawn can acquire it itself.
            drop(lock);
        }
        Err(e) => return Err(format!("failed to check for a running core service: {e}")),
    }

    let genjuxd_path = locate_genjuxd()?;
    // Redirect stdio to null rather than inheriting this process's
    // pipes: genjuxd is meant to keep running as a background service
    // after this CLI process exits. If it inherited an *inherited* (not
    // a real terminal) stdout/stderr -- e.g. a pipe set up by whatever
    // spawned `genjux` itself, such as a test harness capturing output --
    // it would hold that pipe's write end open indefinitely, and
    // anything waiting for that pipe to reach EOF (like
    // `std::process::Command::output()`) would hang forever even after
    // this CLI process exits. Caught via a real hang in the end-to-end
    // test for this issue.
    std::process::Command::new(&genjuxd_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start {}: {e}", genjuxd_path.display()))?;

    let info_path = lifecycle::info_file_path();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(contents) = std::fs::read_to_string(&info_path) {
            if let Ok(info) = serde_json::from_str::<ServiceInfo>(&contents) {
                return Ok(info);
            }
        }
        if std::time::Instant::now() > deadline {
            return Err("timed out waiting for the core service to start".to_string());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn run_search(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    owner: &str,
    name: &str,
) -> Result<(), String> {
    let url = format!("{base_url}/repos/{owner}/{name}/packages");
    let response = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("search failed ({status}): {body}"));
    }

    let packages: Vec<InstallablePackage> = response.json().await.map_err(|e| e.to_string())?;
    if packages.is_empty() {
        println!("No installable packages found for {owner}/{name}.");
        return Ok(());
    }
    println!("Packages for {owner}/{name}:");
    for pkg in &packages {
        let platform = pkg
            .classification
            .platform
            .map(|p| format!("{p:?}"))
            .unwrap_or_else(|| "unclassified".to_string());
        println!("  {} [{platform}]", pkg.asset_name);
    }
    Ok(())
}

async fn run_install(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    owner: &str,
    name: &str,
) -> Result<(), String> {
    #[derive(serde::Serialize)]
    struct InstallRequest {
        owner: String,
        repo: String,
    }
    #[derive(serde::Deserialize)]
    struct InstallStarted {
        install_id: String,
    }

    let response = client
        .post(format!("{base_url}/install"))
        .bearer_auth(token)
        .json(&InstallRequest {
            owner: owner.to_string(),
            repo: name.to_string(),
        })
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("failed to start install ({status}): {body}"));
    }
    let started: InstallStarted = response.json().await.map_err(|e| e.to_string())?;

    let status_url = format!("{base_url}/installs/{}", started.install_id);
    loop {
        let response = client
            .get(&status_url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let stage: InstallStage = response.json().await.map_err(|e| e.to_string())?;
        match stage {
            InstallStage::Succeeded => {
                println!("Installed {owner}/{name}.");
                return Ok(());
            }
            InstallStage::Failed { reason } => {
                return Err(format!("install failed: {reason}"));
            }
            InstallStage::Downloading {
                bytes_downloaded,
                total_bytes: Some(total),
            } => {
                println!("Downloading... {bytes_downloaded}/{total} bytes");
            }
            other => {
                println!("{other:?}");
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn run_list(client: &reqwest::Client, base_url: &str, token: &str) -> Result<(), String> {
    let response = client
        .get(format!("{base_url}/installed"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let entries: Vec<InstalledEntry> = response.json().await.map_err(|e| e.to_string())?;

    if entries.is_empty() {
        println!("No apps installed via Genjux-Store yet.");
        return Ok(());
    }
    for entry in &entries {
        println!("{} ({})", entry.repo, entry.installed_tag);
    }
    Ok(())
}

async fn run_update(client: &reqwest::Client, base_url: &str, token: &str) -> Result<(), String> {
    let response = client
        .get(format!("{base_url}/updates"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let updates: Vec<UpdateCheckResult> = response.json().await.map_err(|e| e.to_string())?;

    let outdated: Vec<&UpdateCheckResult> = updates.iter().filter(|u| u.update_available).collect();
    if outdated.is_empty() {
        println!("Everything is up to date.");
        return Ok(());
    }
    for update in outdated {
        println!(
            "{}: {} -> {}",
            update.repo, update.installed_tag, update.latest_tag
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owner_slash_repo() {
        assert_eq!(
            parse_repo("acme/widget").unwrap(),
            ("acme".to_string(), "widget".to_string())
        );
    }

    #[test]
    fn rejects_missing_slash() {
        assert!(parse_repo("acme-widget").is_err());
    }

    #[test]
    fn rejects_empty_owner_or_repo() {
        assert!(parse_repo("/widget").is_err());
        assert!(parse_repo("acme/").is_err());
    }
}
