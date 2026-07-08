//! Integration test for cross-process singleton locking (issue #18).
//!
//! Lives here (not as a `#[cfg(test)] mod` inside `src/lifecycle.rs`)
//! specifically because `CARGO_BIN_EXE_genjuxd` — the env var Cargo uses
//! to tell a test where a sibling binary target got built — is only set
//! for integration tests (files under `tests/`), not for a lib's own
//! unit tests compiled as part of the same crate.

use genjux_core::lifecycle::{try_acquire_singleton_lock, AcquireOutcome};
use std::time::{Duration, Instant};

/// Spawns the real `genjuxd` binary as a genuinely separate OS process,
/// waits for it to publish its discovery info file, then confirms
/// `try_acquire_singleton_lock()` from *this* process correctly reports
/// it's already running instead of double-acquiring. Kills the child
/// afterward and confirms the lock becomes acquirable again.
///
/// This is deliberately a real second *process* (not just a second file
/// handle within one process): Windows' `LockFileEx` is documented to be
/// per-process, not per-handle, so a same-process multi-handle test
/// cannot reliably observe cross-process exclusion there (see the
/// comment in `core/src/lifecycle.rs`'s unit test for the full
/// explanation, verified via a real windows-latest CI failure while
/// developing this feature).
#[test]
fn a_second_process_finds_the_first_real_genjuxd_process_already_running() {
    let tmp = tempfile::tempdir().unwrap();

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_genjuxd"))
        .env("GENJUX_RUNTIME_DIR", tmp.path())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn the real genjuxd binary");

    let info_path = tmp.path().join("genjuxd.json");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !info_path.exists() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("genjuxd did not publish its info file within 15s");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Give the write a moment to fully land (file existence and a
    // complete write aren't observed atomically by a plain existence
    // check).
    std::thread::sleep(Duration::from_millis(100));

    // SAFETY: no other test in this binary reads/writes
    // GENJUX_RUNTIME_DIR concurrently (it's only consulted by
    // `runtime_dir()`, which this is the sole integration test exercising
    // via env var), so mutating it here for the duration of this
    // single-threaded check is sound in practice even though the
    // underlying libc setenv/getenv are not inherently thread-safe.
    unsafe {
        std::env::set_var("GENJUX_RUNTIME_DIR", tmp.path());
    }

    let result = match try_acquire_singleton_lock() {
        Ok(AcquireOutcome::AlreadyRunning(info)) => {
            assert!(info.port > 0, "published port should be non-zero");
            assert_eq!(
                info.token.len(),
                32,
                "published token should be a real generated token"
            );
            Ok(())
        }
        Ok(AcquireOutcome::Acquired(_)) => {
            Err("expected to find the already-running child, but acquired the lock ourselves")
        }
        Err(e) => {
            let _ = child.kill();
            panic!("try_acquire_singleton_lock errored unexpectedly: {e}");
        }
    };

    child
        .kill()
        .expect("failed to kill the child genjuxd process");
    child.wait().expect("failed to reap the child process");

    // Now that the real process is gone, we should be able to acquire
    // the lock ourselves.
    let outcome_after_exit =
        try_acquire_singleton_lock().expect("should succeed once the child has exited");
    assert!(matches!(outcome_after_exit, AcquireOutcome::Acquired(_)));

    unsafe {
        std::env::remove_var("GENJUX_RUNTIME_DIR");
    }

    result.unwrap();
}
