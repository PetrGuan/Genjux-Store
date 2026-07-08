//! Local audit log (issue #19).
//!
//! Records installation activity — source, verification result, and
//! install outcome — for user-side troubleshooting and accountability, per
//! `.copilot-workflow/PLAN.md` section 5 ("record installation source and
//! time to help users investigate problems later").
//!
//! Persisted as append-only JSONL (one JSON object per line): unlike
//! [`crate::registry`], which needs random-access overwrite semantics
//! (reinstalling a repo replaces its entry), an audit log is purely
//! sequential and never edits past entries, so append-only is both the
//! simpler and the more correct choice here.

use crate::source::RepoRef;
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuditEntry {
    /// Unix epoch seconds (see [`crate::registry::InstalledEntry`] for why
    /// this crate doesn't pull in a date/time dependency for Phase 0).
    pub timestamp_unix: u64,
    pub repo: RepoRef,
    pub asset_name: String,
    pub source_url: String,
    pub verification: VerificationOutcome,
    pub install_outcome: InstallOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum VerificationOutcome {
    ChecksumMatched {
        sha256: String,
    },
    ChecksumMismatch {
        expected: String,
        computed: String,
    },
    /// No published checksum was available; this is the computed hash for
    /// the user to look up elsewhere if they want to, per the trust model
    /// in PLAN.md section 5 — it is not itself an assertion of safety.
    NoChecksumAvailable {
        computed_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum InstallOutcome {
    Success,
    Failed { reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to (de)serialize audit entry: {0}")]
    Serde(#[from] serde_json::Error),
}

#[async_trait]
pub trait AuditLog: Send + Sync {
    async fn append(&self, entry: AuditEntry) -> Result<(), AuditError>;
    /// Returns up to `limit` most recent entries, oldest first within that
    /// window (i.e. the same order they'd read top-to-bottom in the file).
    async fn recent(&self, limit: usize) -> Result<Vec<AuditEntry>, AuditError>;
}

/// An [`AuditLog`] backed by an append-only JSONL file.
pub struct JsonlAuditLog {
    path: PathBuf,
    /// Serializes concurrent appends within this process. A `tokio::sync`
    /// mutex (not `std::sync`) specifically so the guard can be held
    /// across the `.await` points of the file write without blocking the
    /// async runtime's worker thread.
    write_lock: Mutex<()>,
}

impl JsonlAuditLog {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            write_lock: Mutex::new(()),
        }
    }
}

#[async_trait]
impl AuditLog for JsonlAuditLog {
    async fn append(&self, entry: AuditEntry) -> Result<(), AuditError> {
        let _guard = self.write_lock.lock().await;

        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;

        let mut line = serde_json::to_string(&entry)?;
        line.push('\n');
        file.write_all(line.as_bytes()).await?;
        Ok(())
    }

    async fn recent(&self, limit: usize) -> Result<Vec<AuditEntry>, AuditError> {
        if !tokio::fs::try_exists(&self.path).await.unwrap_or(false) {
            return Ok(Vec::new());
        }
        let contents = tokio::fs::read_to_string(&self.path).await?;
        let mut entries = Vec::new();
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            entries.push(serde_json::from_str(line)?);
        }
        let start = entries.len().saturating_sub(limit);
        Ok(entries.split_off(start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(asset_name: &str) -> AuditEntry {
        AuditEntry {
            timestamp_unix: 1_700_000_000,
            repo: RepoRef::new("github", "acme", "widget"),
            asset_name: asset_name.to_string(),
            source_url: format!("https://example.invalid/{asset_name}"),
            verification: VerificationOutcome::ChecksumMatched {
                sha256: "abc123".to_string(),
            },
            install_outcome: InstallOutcome::Success,
        }
    }

    #[tokio::test]
    async fn append_then_recent_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let log = JsonlAuditLog::new(tmp.path().join("audit.jsonl"));

        log.append(sample_entry("widget-v1.dmg")).await.unwrap();

        let recent = log.recent(10).await.unwrap();
        assert_eq!(recent, vec![sample_entry("widget-v1.dmg")]);
    }

    #[tokio::test]
    async fn recent_respects_limit_and_keeps_the_newest_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let log = JsonlAuditLog::new(tmp.path().join("audit.jsonl"));

        for i in 0..5 {
            log.append(sample_entry(&format!("asset-{i}")))
                .await
                .unwrap();
        }

        let recent = log.recent(2).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].asset_name, "asset-3");
        assert_eq!(recent[1].asset_name, "asset-4");
    }

    #[tokio::test]
    async fn records_both_successful_and_failed_installs() {
        let tmp = tempfile::tempdir().unwrap();
        let log = JsonlAuditLog::new(tmp.path().join("audit.jsonl"));

        let mut failed = sample_entry("widget-v2.dmg");
        failed.install_outcome = InstallOutcome::Failed {
            reason: "checksum mismatch".to_string(),
        };
        failed.verification = VerificationOutcome::ChecksumMismatch {
            expected: "aaa".to_string(),
            computed: "bbb".to_string(),
        };
        log.append(failed.clone()).await.unwrap();
        log.append(sample_entry("widget-v3.dmg")).await.unwrap();

        let recent = log.recent(10).await.unwrap();
        assert_eq!(recent[0], failed);
        assert_eq!(recent[1].install_outcome, InstallOutcome::Success);
    }

    #[tokio::test]
    async fn recent_on_a_log_with_no_entries_yet_returns_empty_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let log = JsonlAuditLog::new(tmp.path().join("does-not-exist.jsonl"));
        assert_eq!(log.recent(10).await.unwrap(), Vec::new());
    }

    #[tokio::test]
    async fn log_survives_a_restart_by_reopening_the_same_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("audit.jsonl");

        {
            let log = JsonlAuditLog::new(path.clone());
            log.append(sample_entry("widget-v1.dmg")).await.unwrap();
        } // dropped here, simulating a restart

        let reopened = JsonlAuditLog::new(path);
        assert_eq!(reopened.recent(10).await.unwrap().len(), 1);
    }
}
