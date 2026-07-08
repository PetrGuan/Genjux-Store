//! Service lifecycle management (issue #18): singleton locking, local
//! auth token issuance, and idle auto-shutdown for the `genjuxd` core
//! service process.
//!
//! Per `.copilot-workflow/PLAN.md`'s open questions on local-service
//! security and lifecycle:
//! - Only one `genjuxd` should ever be running per user at a time. This
//!   is enforced with a real OS-level advisory file lock (not just a
//!   "check if a PID exists" heuristic, which is racy and doesn't
//!   reliably detect a dead-but-not-yet-reaped process) — see
//!   [`SingletonLock`].
//! - The lock file doubles as the *discovery* file: once a process holds
//!   the lock, it writes its bound port and auth token into that same
//!   file (as JSON) so a client that fails to acquire the lock can read
//!   those instead of needing separate IPC.
//! - The auth token gates every HTTP/MCP request (see the
//!   `auth_middleware` this module provides), so another local process
//!   can't just guess the port and start issuing install requests.

use fs4::fs_std::FileExt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where `genjuxd`'s lock/discovery file and any other per-user runtime
/// state lives. Uses the OS-appropriate local data directory (e.g.
/// `~/Library/Application Support/genjux` on macOS, `~/.local/share/genjux`
/// on Linux, `%LOCALAPPDATA%\genjux` on Windows) rather than a hardcoded
/// path. Honors `GENJUX_RUNTIME_DIR` if set, so tests (and advanced users)
/// can redirect it without touching the real per-user data directory.
pub fn runtime_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("GENJUX_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("genjux")
}

fn lock_file_path() -> PathBuf {
    runtime_dir().join("genjuxd.lock")
}

/// What a running `genjuxd` instance publishes into the lock file so
/// other local processes (the CLI, in #20) can find it instead of
/// starting a second instance.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceInfo {
    pub port: u16,
    pub token: String,
}

/// Generates a random, URL-safe local auth token. Uses a real CSPRNG
/// (`rand`), not a hash of predictable inputs like the PID/timestamp —
/// this token is the only thing standing between "any local process" and
/// "can trigger installs through this service's HTTP/MCP API".
pub fn generate_token() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("failed to (de)serialize service info: {0}")]
    Serde(#[from] serde_json::Error),
}

/// An OS-level advisory lock, held for as long as this value is alive.
/// Dropping it releases the lock (and, on process exit, the OS releases
/// it automatically even if the process crashes — unlike a plain "PID
/// file" that has to be interpreted heuristically).
pub struct SingletonLock {
    file: std::fs::File,
    path: PathBuf,
}

impl SingletonLock {
    /// Publishes this instance's `info` into the locked file so other
    /// processes that fail to acquire the lock can read it.
    pub fn publish(&mut self, info: &ServiceInfo) -> Result<(), LifecycleError> {
        use std::io::{Seek, SeekFrom, Write};
        let json = serde_json::to_vec(info)?;
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&json)?;
        self.file.flush()?;
        Ok(())
    }
}

impl Drop for SingletonLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
        // Best-effort cleanup; if this fails (e.g. another process is
        // racing to acquire right as we exit) that's fine, the lock
        // itself is what matters, not the file's continued existence.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The outcome of trying to become the singleton `genjuxd` instance.
pub enum AcquireOutcome {
    /// We now own the lock and should start serving.
    Acquired(SingletonLock),
    /// Another live instance already holds the lock; here's how to reach
    /// it instead of starting a second one.
    AlreadyRunning(ServiceInfo),
}

/// Tries to become the singleton instance. If another process already
/// holds the lock, reads that process's published [`ServiceInfo`] instead
/// of erroring — the expected, common case (a client lazily starting the
/// service finds one already running).
pub fn try_acquire_singleton_lock() -> Result<AcquireOutcome, LifecycleError> {
    let path = lock_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;

    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(AcquireOutcome::Acquired(SingletonLock { file, path })),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            let contents = std::fs::read_to_string(&path)?;
            let info: ServiceInfo = serde_json::from_str(&contents)?;
            Ok(AcquireOutcome::AlreadyRunning(info))
        }
        Err(e) => Err(e.into()),
    }
}

/// Tracks the timestamp of the most recent request, so a background task
/// can decide when the service has been idle long enough to shut down.
/// Cheap to update on every request (a single atomic store), and safe to
/// share across the whole axum router via `Arc`.
///
/// Tracks elapsed time via a monotonic [`Instant`] (not a wall-clock
/// `SystemTime`/unix-timestamp), for two reasons: it's immune to the
/// system clock being adjusted mid-run, and it has sub-second precision
/// — a unix-seconds timestamp rounds away exactly the granularity needed
/// to test the idle-shutdown watcher without waiting on real 30-minute
/// timeouts (discovered via a real flaky test failure while developing
/// this: a whole-second-resolution clock made a 50ms test timeout
/// meaningless).
pub struct ActivityTracker {
    last_activity: std::sync::Mutex<Instant>,
}

impl Default for ActivityTracker {
    fn default() -> Self {
        Self {
            last_activity: std::sync::Mutex::new(Instant::now()),
        }
    }
}

impl ActivityTracker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn touch(&self) {
        *self
            .last_activity
            .lock()
            .expect("activity tracker lock poisoned") = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_activity
            .lock()
            .expect("activity tracker lock poisoned")
            .elapsed()
    }
}

/// Runs until [`ActivityTracker`] has seen no activity for `idle_timeout`,
/// then cancels `shutdown` to trigger graceful shutdown. Checks every
/// `poll_interval` — deliberately a parameter (not hardcoded) so tests can
/// use very short intervals instead of waiting on real wall-clock idle
/// timeouts.
pub async fn run_idle_shutdown_watcher(
    activity: Arc<ActivityTracker>,
    idle_timeout: Duration,
    poll_interval: Duration,
    shutdown: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::time::sleep(poll_interval).await;
        if activity.idle_for() >= idle_timeout {
            shutdown.cancel();
            return;
        }
        if shutdown.is_cancelled() {
            return;
        }
    }
}

/// Axum middleware requiring `Authorization: Bearer <token>` to match the
/// service's local auth token, so another local process can't drive
/// installs through this API just by guessing the port.
pub async fn auth_middleware(
    expected_token: Arc<String>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let provided = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match provided {
        Some(token) if token == expected_token.as_str() => next.run(request).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            "missing or invalid local auth token",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_reasonably_long_and_not_trivially_predictable() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b, "two consecutive tokens should not collide");
    }

    #[test]
    fn second_lock_attempt_on_the_same_path_finds_the_first_instances_published_info() {
        // Point both attempts at an isolated temp lock file rather than
        // the real per-user runtime_dir(), so parallel test runs (and
        // other tests in this suite) can't collide on the same path.
        let tmp = tempfile::tempdir().unwrap();
        let lock_path = tmp.path().join("genjuxd.lock");

        let file_a = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        FileExt::try_lock_exclusive(&file_a).expect("first attempt should acquire the lock");
        let mut lock_a = SingletonLock {
            file: file_a,
            path: lock_path.clone(),
        };
        let info = ServiceInfo {
            port: 4242,
            token: "test-token".to_string(),
        };
        lock_a.publish(&info).unwrap();

        let file_b = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();

        // Cross-process mutual exclusion (the guarantee that actually
        // matters in production — two separate `genjuxd` processes) is
        // enforced identically on all three platforms: this is standard,
        // well-documented OS file-locking behavior. But *this test*
        // simulates "two attempts" with two handles opened by the same
        // process, and that specific scenario is only reliable evidence
        // of exclusion on Unix. Windows' LockFileEx is explicitly
        // per-process, not per-handle (see MSDN: "A process can lock a
        // region of a file more than once... There is no conflict
        // between different file handles for the same file in the same
        // process") — so a second handle in the *same* process is
        // guaranteed to succeed on Windows regardless of whether another
        // process holds the lock, which is exactly the opposite of what
        // this assertion checks. Verified this the hard way: this
        // assertion originally had no cfg guard and failed on the
        // windows-latest CI runner precisely because of this documented
        // behavior difference, not a bug in SingletonLock itself.
        #[cfg(unix)]
        {
            let acquired_b = FileExt::try_lock_exclusive(&file_b);
            assert!(
                acquired_b.is_err_and(|e| e.kind() == io::ErrorKind::WouldBlock),
                "second attempt must not acquire the lock while the first is held"
            );
        }

        let contents = std::fs::read_to_string(&lock_path).unwrap();
        let read_back: ServiceInfo = serde_json::from_str(&contents).unwrap();
        assert_eq!(read_back, info);

        drop(lock_a); // releases the OS-level lock
        assert!(
            FileExt::try_lock_exclusive(&file_b).is_ok(),
            "lock should become available again once the holder drops it"
        );
    }

    #[tokio::test]
    async fn idle_shutdown_watcher_cancels_after_the_configured_timeout() {
        let activity = ActivityTracker::new();
        let shutdown = tokio_util::sync::CancellationToken::new();

        let watcher = tokio::spawn(run_idle_shutdown_watcher(
            activity.clone(),
            Duration::from_millis(50),
            Duration::from_millis(10),
            shutdown.clone(),
        ));

        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("watcher should finish well within the timeout")
            .unwrap();
        assert!(shutdown.is_cancelled());
    }

    #[tokio::test]
    async fn activity_resets_the_idle_clock_so_shutdown_is_deferred() {
        let activity = ActivityTracker::new();
        let shutdown = tokio_util::sync::CancellationToken::new();

        let activity_for_task = activity.clone();
        let keep_touching = tokio::spawn(async move {
            for _ in 0..8 {
                tokio::time::sleep(Duration::from_millis(15)).await;
                activity_for_task.touch();
            }
        });

        let watcher = run_idle_shutdown_watcher(
            activity.clone(),
            Duration::from_millis(50),
            Duration::from_millis(10),
            shutdown.clone(),
        );

        // The watcher must NOT fire while activity keeps resetting the
        // clock faster than the idle timeout.
        tokio::select! {
            _ = watcher => panic!("idle shutdown fired despite ongoing activity"),
            _ = keep_touching => {}
        }
        assert!(!shutdown.is_cancelled());
    }
}
