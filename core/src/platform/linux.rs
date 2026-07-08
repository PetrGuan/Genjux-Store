//! Linux platform adapter (issue #14): installs a downloaded `.deb`,
//! `.rpm`, or `.AppImage`.
//!
//! `.deb`/`.rpm` need root, handled via `pkexec` so the desktop's own
//! polkit authentication agent shows the graphical prompt (never bypassed,
//! per `.copilot-workflow/PLAN.md` sections 4-5). `.AppImage` needs no
//! privilege escalation at all — it's just made executable and placed
//! under a user-writable `bin` directory.

use crate::orchestrate::PlatformAdapter;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Builds the `pkexec dpkg -i <path>` argument list. Extracted as a pure
/// function so it's unit-testable without invoking `pkexec`, which needs a
/// live desktop polkit agent that doesn't exist in headless CI (see
/// module docs).
fn deb_install_command_args(deb_path: &Path) -> Vec<String> {
    vec![
        "dpkg".to_string(),
        "-i".to_string(),
        deb_path.to_string_lossy().to_string(),
    ]
}

/// Builds the `pkexec rpm -i <path>` argument list. Same rationale as
/// [`deb_install_command_args`].
fn rpm_install_command_args(rpm_path: &Path) -> Vec<String> {
    vec![
        "rpm".to_string(),
        "-i".to_string(),
        rpm_path.to_string_lossy().to_string(),
    ]
}

/// Installs `.deb`/`.rpm`/`.AppImage` artifacts on Linux.
pub struct LinuxAdapter {
    /// Directory `.AppImage` files are copied into and made executable.
    /// Production code should pass `~/.local/bin`; tests pass a temp
    /// directory.
    appimage_bin_dir: PathBuf,
}

impl LinuxAdapter {
    pub fn new(appimage_bin_dir: PathBuf) -> Self {
        Self { appimage_bin_dir }
    }

    async fn install_appimage(&self, appimage_path: &Path) -> Result<(), String> {
        tokio::fs::create_dir_all(&self.appimage_bin_dir)
            .await
            .map_err(|e| format!("failed to create bin directory: {e}"))?;

        let file_name = appimage_path
            .file_name()
            .ok_or_else(|| "AppImage path has no filename".to_string())?;
        let dest = self.appimage_bin_dir.join(file_name);

        tokio::fs::copy(appimage_path, &dest)
            .await
            .map_err(|e| format!("failed to copy AppImage: {e}"))?;

        use std::os::unix::fs::PermissionsExt;
        let mut perms = tokio::fs::metadata(&dest)
            .await
            .map_err(|e| format!("failed to read copied AppImage metadata: {e}"))?
            .permissions();
        perms.set_mode(perms.mode() | 0o111); // add execute bits for user/group/other
        tokio::fs::set_permissions(&dest, perms)
            .await
            .map_err(|e| format!("failed to make AppImage executable: {e}"))?;

        Ok(())
    }

    async fn install_deb(&self, deb_path: &Path) -> Result<(), String> {
        let status = Command::new("pkexec")
            .args(deb_install_command_args(deb_path))
            .status()
            .await
            .map_err(|e| format!("failed to run pkexec dpkg: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("dpkg exited with status {status}"))
        }
    }

    async fn install_rpm(&self, rpm_path: &Path) -> Result<(), String> {
        let status = Command::new("pkexec")
            .args(rpm_install_command_args(rpm_path))
            .status()
            .await
            .map_err(|e| format!("failed to run pkexec rpm: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("rpm exited with status {status}"))
        }
    }
}

#[async_trait]
impl PlatformAdapter for LinuxAdapter {
    async fn install(&self, downloaded_file: &Path) -> Result<(), String> {
        match downloaded_file
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
        {
            Some(ext) if ext == "appimage" => self.install_appimage(downloaded_file).await,
            Some(ext) if ext == "deb" => self.install_deb(downloaded_file).await,
            Some(ext) if ext == "rpm" => self.install_rpm(downloaded_file).await,
            other => Err(format!(
                "unsupported Linux install artifact extension: {other:?}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deb_install_args_are_correct() {
        assert_eq!(
            deb_install_command_args(Path::new("/tmp/app.deb")),
            vec![
                "dpkg".to_string(),
                "-i".to_string(),
                "/tmp/app.deb".to_string()
            ]
        );
    }

    #[test]
    fn rpm_install_args_are_correct() {
        assert_eq!(
            rpm_install_command_args(Path::new("/tmp/app.rpm")),
            vec![
                "rpm".to_string(),
                "-i".to_string(),
                "/tmp/app.rpm".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn appimage_is_copied_and_made_executable() {
        let workdir = tempfile::tempdir().unwrap();
        let src = workdir.path().join("MyApp.AppImage");
        tokio::fs::write(&src, b"fake appimage contents")
            .await
            .unwrap();

        let bin_dir = workdir.path().join("bin");
        let adapter = LinuxAdapter::new(bin_dir.clone());
        adapter.install(&src).await.expect("install should succeed");

        let dest = bin_dir.join("MyApp.AppImage");
        assert!(dest.is_file());
        assert_eq!(
            tokio::fs::read(&dest).await.unwrap(),
            b"fake appimage contents"
        );

        use std::os::unix::fs::PermissionsExt;
        let mode = tokio::fs::metadata(&dest)
            .await
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "expected all execute bits to be set");
    }

    #[tokio::test]
    async fn appimage_install_requires_no_privilege_escalation() {
        // Sanity check that install_appimage never shells out to pkexec —
        // if it did, this test would hang/fail in headless CI (no polkit
        // agent). Verified indirectly: the previous test already proves
        // the AppImage path completes successfully in exactly this kind
        // of headless environment.
        let workdir = tempfile::tempdir().unwrap();
        let src = workdir.path().join("Another.AppImage");
        tokio::fs::write(&src, b"x").await.unwrap();
        let adapter = LinuxAdapter::new(workdir.path().join("bin2"));
        assert!(adapter.install(&src).await.is_ok());
    }

    #[tokio::test]
    async fn unsupported_extension_is_rejected_without_touching_the_filesystem() {
        let adapter = LinuxAdapter::new(PathBuf::from("/tmp/genjux-unused"));
        let err = adapter
            .install(Path::new("/tmp/whatever.zip"))
            .await
            .unwrap_err();
        assert!(err.contains("unsupported Linux install artifact extension"));
    }
}
