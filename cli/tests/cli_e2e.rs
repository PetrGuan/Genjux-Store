//! End-to-end integration test for the `genjux` CLI (issue #20).
//!
//! Spawns the real `genjux` binary (which itself lazily spawns the real
//! `genjuxd`, per #18) against an isolated `GENJUX_RUNTIME_DIR`, and
//! confirms the whole lazy-start -> HTTP round trip -> formatted output
//! pipeline actually works — not just that the individual pieces compile.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

#[cfg(unix)]
fn kill_process(pid: u32) {
    let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
}

#[cfg(windows)]
fn kill_process(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status();
}

/// Kills the `genjuxd` a test lazily started, on `Drop` — so cleanup
/// always runs, even if an `assert!` earlier in the test panics.
///
/// This matters specifically on Windows: GitHub Actions' Windows runners
/// track spawned processes via a Job Object and the whole CI step can
/// hang waiting for *every* descendant process to exit, not just the test
/// binary itself — confirmed against real (cancelled) CI runs while
/// developing this feature. A lazily-started `genjuxd` is *designed* to
/// keep running after the process that spawned it exits (that's the
/// point of "lazy start" — see #18), so it must be explicitly killed
/// before the test process exits, unconditionally, not just on the
/// happy path.
struct GenjuxdCleanup {
    info_path: PathBuf,
}

impl Drop for GenjuxdCleanup {
    fn drop(&mut self) {
        let Ok(contents) = std::fs::read_to_string(&self.info_path) else {
            return; // no genjuxd was ever started against this runtime dir
        };
        let Ok(info) = serde_json::from_str::<serde_json::Value>(&contents) else {
            return;
        };
        if let Some(pid) = info["pid"].as_u64() {
            kill_process(pid as u32);
            // Give the OS a moment to actually release the lock file
            // before the temp dir gets removed right after this.
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

#[test]
fn list_lazily_starts_the_core_service_and_prints_empty_state() {
    let tmp = tempfile::tempdir().unwrap();
    // Declared after `tmp` so it drops *before* `tmp` does (Rust drops
    // locals in reverse declaration order) — genjuxd needs to release
    // its lock file inside `tmp` before that directory gets removed.
    let _cleanup = GenjuxdCleanup {
        info_path: tmp.path().join("genjuxd.json"),
    };

    let output = Command::new(env!("CARGO_BIN_EXE_genjux"))
        .arg("list")
        .env("GENJUX_RUNTIME_DIR", tmp.path())
        .output()
        .expect("failed to run the real genjux binary");

    assert!(
        output.status.success(),
        "genjux list should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No apps installed"),
        "expected empty-state message, got: {stdout:?}"
    );

    // A second invocation should find the now-running service (via the
    // lock/discovery mechanism from #18) instead of starting a duplicate
    // genjuxd — exercising the "reuse an already-running instance" path,
    // not just "lazily start a fresh one".
    let second_output = Command::new(env!("CARGO_BIN_EXE_genjux"))
        .arg("list")
        .env("GENJUX_RUNTIME_DIR", tmp.path())
        .output()
        .expect("failed to run the real genjux binary a second time");
    assert!(second_output.status.success());
    assert!(String::from_utf8_lossy(&second_output.stdout).contains("No apps installed"));
}

#[test]
fn search_with_a_malformed_repo_spec_fails_with_a_clear_error() {
    let tmp = tempfile::tempdir().unwrap();
    let _cleanup = GenjuxdCleanup {
        info_path: tmp.path().join("genjuxd.json"),
    };

    let output = Command::new(env!("CARGO_BIN_EXE_genjux"))
        .args(["search", "not-a-valid-repo-spec"])
        .env("GENJUX_RUNTIME_DIR", tmp.path())
        .output()
        .expect("failed to run the real genjux binary");

    assert!(
        !output.status.success(),
        "expected a non-zero exit for a malformed repo spec"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("owner/repo"),
        "expected a helpful error message, got: {stderr:?}"
    );
    // Note: with argument validation happening before the lazy service
    // start (see main.rs), this invocation should never actually spawn a
    // genjuxd — the cleanup guard above is a defensive no-op here, kept
    // for consistency and in case that ordering ever changes.
}
