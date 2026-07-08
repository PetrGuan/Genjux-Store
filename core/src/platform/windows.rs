//! Windows platform adapter (issue #13): installs a downloaded `.msi` or
//! `.exe`.
//!
//! Unlike macOS's Gatekeeper (checked via `spctl`, see
//! [`crate::platform::macos::assess_gatekeeper`]), Windows doesn't expose a
//! simple CLI equivalent for querying SmartScreen reputation ahead of
//! time — SmartScreen acts automatically, based on the downloaded file's
//! "Mark of the Web" (`Zone.Identifier` alternate data stream), when the
//! file is actually executed. The correct behavior here is simply to *not*
//! strip that marker (this crate's download manager doesn't touch it) and
//! let Windows show its own warning when `install()` runs the file — per
//! `.copilot-workflow/PLAN.md` sections 4-5, never suppressing it, not
//! trying to reimplement an "assess" step that the OS doesn't offer a
//! clean way to do ahead of time.

use crate::orchestrate::PlatformAdapter;
use async_trait::async_trait;
use std::path::Path;
use tokio::process::Command;

/// Builds the `msiexec` CLI arguments for an interactive (non-silent)
/// install. Extracted as a pure function so it's unit-testable without
/// invoking `msiexec`.
///
/// **Phase 0 scope note**: doesn't yet pass through
/// `InstallablePackage.silent_install_args` (populated by the curator
/// overlay, #7) — wiring that through requires extending orchestration
/// (#11) to carry the field to the adapter, which is straightforward but
/// kept out of this issue to keep its diff focused on the adapter
/// mechanism itself.
fn msi_install_command_args(msi_path: &Path) -> Vec<String> {
    vec!["/i".to_string(), msi_path.to_string_lossy().to_string()]
}

/// Installs `.msi`/`.exe` artifacts on Windows.
pub struct WindowsAdapter;

impl Default for WindowsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn install_msi(&self, msi_path: &Path) -> Result<(), String> {
        let status = Command::new("msiexec")
            .args(msi_install_command_args(msi_path))
            .status()
            .await
            .map_err(|e| format!("failed to run msiexec: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("msiexec exited with status {status}"))
        }
    }

    async fn install_exe(&self, exe_path: &Path) -> Result<(), String> {
        // Runs the installer directly, respecting whatever UAC elevation
        // prompt and installer UI it shows — never suppressed, per
        // PLAN.md sections 4-5.
        let status = Command::new(exe_path)
            .status()
            .await
            .map_err(|e| format!("failed to run installer: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("installer exited with status {status}"))
        }
    }
}

#[async_trait]
impl PlatformAdapter for WindowsAdapter {
    async fn install(&self, downloaded_file: &Path) -> Result<(), String> {
        match downloaded_file
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
        {
            Some(ext) if ext == "msi" => self.install_msi(downloaded_file).await,
            Some(ext) if ext == "exe" => self.install_exe(downloaded_file).await,
            other => Err(format!(
                "unsupported Windows install artifact extension: {other:?}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msi_install_args_run_a_plain_interactive_install() {
        let args = msi_install_command_args(Path::new(r"C:\Downloads\app.msi"));
        assert_eq!(
            args,
            vec!["/i".to_string(), r"C:\Downloads\app.msi".to_string()]
        );
    }

    #[tokio::test]
    async fn unsupported_extension_is_rejected_without_running_anything() {
        let adapter = WindowsAdapter::new();
        let err = adapter
            .install(Path::new(r"C:\Downloads\app.zip"))
            .await
            .unwrap_err();
        assert!(err.contains("unsupported Windows install artifact extension"));
    }
}
