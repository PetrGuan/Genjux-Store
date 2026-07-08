//! macOS platform adapter (issue #12): installs a downloaded `.dmg` or
//! `.pkg`, and separately surfaces Gatekeeper/notarization status as a
//! trust signal (never used to silently block or bypass — see
//! `.copilot-workflow/PLAN.md` sections 4-5).

use crate::orchestrate::PlatformAdapter;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Result of checking Gatekeeper/notarization status via `spctl`. This is
/// deliberately a standalone function rather than part of
/// [`PlatformAdapter::install`] — per PLAN.md, Genjux-Store shows this
/// signal to the user rather than acting on it itself (not blocking
/// install on rejection, not bypassing the OS's own warnings either).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatekeeperStatus {
    Accepted,
    Rejected {
        reason: String,
    },
    /// `spctl` itself couldn't be run (e.g. missing from `PATH`).
    Unknown,
}

/// Runs `spctl -a -vv <path>` and translates the result into a
/// [`GatekeeperStatus`].
pub async fn assess_gatekeeper(path: &Path) -> GatekeeperStatus {
    let output = match Command::new("spctl")
        .arg("-a")
        .arg("-vv")
        .arg(path)
        .output()
        .await
    {
        Ok(output) => output,
        Err(_) => return GatekeeperStatus::Unknown,
    };

    if output.status.success() {
        GatekeeperStatus::Accepted
    } else {
        GatekeeperStatus::Rejected {
            reason: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        }
    }
}

/// Builds the `installer` CLI arguments for a per-user install (no root
/// required). Extracted as a pure function so it's unit-testable without
/// actually invoking `installer`.
///
/// **Phase 0 scope note**: this only supports the per-user
/// (`CurrentUserHomeDirectory`) target. A true system-wide (`/`) install
/// needs root, which in turn needs either the OS's native admin-privilege
/// prompt (e.g. `osascript ... with administrator privileges`) or a GUI
/// framework's Authorization Services integration — neither of which is
/// implementable *or verifiable* in a headless environment. Deferred to
/// whichever future issue adds the real GUI (Phase 1) and can exercise an
/// actual interactive password prompt.
fn pkg_install_command_args(pkg_path: &Path) -> Vec<String> {
    vec![
        "-pkg".to_string(),
        pkg_path.to_string_lossy().to_string(),
        "-target".to_string(),
        "CurrentUserHomeDirectory".to_string(),
    ]
}

/// Extracts the string value following `<key>{key}</key>` in a `plist`
/// XML document. Hand-rolled rather than pulling in the `plist` crate,
/// since `hdiutil`'s plist output format for this one key is small,
/// well-known, and has been stable for many macOS releases; worth
/// revisiting if more complex plist parsing is ever needed elsewhere.
fn extract_plist_string(text: &str, key: &str) -> Option<String> {
    let key_tag = format!("<key>{key}</key>");
    let after_key = &text[text.find(&key_tag)? + key_tag.len()..];
    let start = after_key.find("<string>")? + "<string>".len();
    let end = start + after_key[start..].find("</string>")?;
    Some(after_key[start..end].to_string())
}

/// Finds the `(mount-point, dev-entry)` pair for the mountable volume in
/// an `hdiutil attach -plist` document.
fn find_mounted_volume(plist_xml: &str) -> Option<(String, String)> {
    for block in plist_xml.split("<dict>").skip(1) {
        let block = &block[..block.find("</dict>").unwrap_or(block.len())];
        if block.contains("<key>mount-point</key>") {
            let mount_point = extract_plist_string(block, "mount-point")?;
            let dev_entry = extract_plist_string(block, "dev-entry")?;
            return Some((mount_point, dev_entry));
        }
    }
    None
}

/// Installs `.dmg`/`.pkg` artifacts on macOS.
pub struct MacOsAdapter {
    /// Destination directory `.app` bundles from mounted `.dmg` images are
    /// copied into. Production code should pass `/Applications`; tests
    /// pass a temp directory so they don't need root and don't touch the
    /// real system Applications folder.
    apps_dir: PathBuf,
}

impl MacOsAdapter {
    pub fn new(apps_dir: PathBuf) -> Self {
        Self { apps_dir }
    }

    async fn install_dmg(&self, dmg_path: &Path) -> Result<(), String> {
        let output = Command::new("hdiutil")
            .arg("attach")
            .arg("-nobrowse")
            .arg("-readonly")
            .arg("-plist")
            .arg(dmg_path)
            .output()
            .await
            .map_err(|e| format!("failed to run hdiutil attach: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "hdiutil attach failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let plist_xml = String::from_utf8_lossy(&output.stdout);
        let (mount_point, dev_entry) = find_mounted_volume(&plist_xml)
            .ok_or_else(|| "could not determine mount point from hdiutil output".to_string())?;

        let install_result = self.copy_app_from_mount(Path::new(&mount_point)).await;

        // Always try to detach, even if copying failed, so we don't leave
        // a mounted volume behind. Detach by device identifier (more
        // reliable than by mount path, which can contain characters that
        // need extra quoting).
        let _ = Command::new("hdiutil")
            .arg("detach")
            .arg(&dev_entry)
            .arg("-quiet")
            .output()
            .await;

        install_result
    }

    async fn copy_app_from_mount(&self, mount_point: &Path) -> Result<(), String> {
        let mut entries = tokio::fs::read_dir(mount_point)
            .await
            .map_err(|e| format!("failed to read mounted volume: {e}"))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| format!("failed to read mounted volume entry: {e}"))?
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("app") {
                continue;
            }

            tokio::fs::create_dir_all(&self.apps_dir)
                .await
                .map_err(|e| format!("failed to create destination directory: {e}"))?;
            let dest = self
                .apps_dir
                .join(path.file_name().expect("path has a filename"));

            // `ditto` (not a hand-rolled recursive copy) preserves the
            // bundle's symlinks/resource forks/extended attributes, which
            // real .app bundles rely on.
            let status = Command::new("ditto")
                .arg(&path)
                .arg(&dest)
                .status()
                .await
                .map_err(|e| format!("failed to run ditto: {e}"))?;
            return if status.success() {
                Ok(())
            } else {
                Err(format!("ditto exited with status {status}"))
            };
        }

        Err("no .app bundle found inside the mounted volume".to_string())
    }

    async fn install_pkg(&self, pkg_path: &Path) -> Result<(), String> {
        let output = Command::new("installer")
            .args(pkg_install_command_args(pkg_path))
            .output()
            .await
            .map_err(|e| format!("failed to run installer: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "installer failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }
}

#[async_trait]
impl PlatformAdapter for MacOsAdapter {
    async fn install(&self, downloaded_file: &Path) -> Result<(), String> {
        match downloaded_file
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
        {
            Some(ext) if ext == "dmg" => self.install_dmg(downloaded_file).await,
            Some(ext) if ext == "pkg" => self.install_pkg(downloaded_file).await,
            other => Err(format!(
                "unsupported macOS install artifact extension: {other:?}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::process::Command as TokioCommand;

    /// Serializes the tests below that invoke `hdiutil attach`/`detach`.
    /// Running two disk-image attach operations concurrently in the same
    /// process was observed to intermittently fail with "Resource
    /// temporarily unavailable" — a real flakiness source in DiskArbitration
    /// contention, not a bug in the adapter itself. Holding this lock across
    /// each full test (attach through detach) trades a little test wall
    /// time for a CI signal that's actually trustworthy.
    static DMG_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn build_test_dmg(dmg_path: &Path, include_app: bool) {
        let staging = tempfile::tempdir().unwrap();
        if include_app {
            let app_dir = staging.path().join("GenjuxTestApp.app/Contents/MacOS");
            tokio::fs::create_dir_all(&app_dir).await.unwrap();
            tokio::fs::write(app_dir.join("GenjuxTestApp"), b"#!/bin/sh\necho hi\n")
                .await
                .unwrap();
        } else {
            tokio::fs::write(staging.path().join("README.txt"), b"no app in here")
                .await
                .unwrap();
        }

        let status = TokioCommand::new("hdiutil")
            .args(["create", "-volname", "GenjuxAdapterTest", "-srcfolder"])
            .arg(staging.path())
            .args(["-ov", "-format", "UDZO", "-quiet"])
            .arg(dmg_path)
            .status()
            .await
            .unwrap();
        assert!(status.success(), "hdiutil create failed");
    }

    #[tokio::test]
    async fn installs_app_bundle_from_a_real_dmg() {
        let _guard = DMG_TEST_LOCK.lock().await;
        let workdir = tempfile::tempdir().unwrap();
        let dmg_path = workdir.path().join("test.dmg");
        build_test_dmg(&dmg_path, true).await;

        let apps_dir = workdir.path().join("Applications");
        let adapter = MacOsAdapter::new(apps_dir.clone());

        adapter
            .install(&dmg_path)
            .await
            .expect("install should succeed");

        assert!(apps_dir.join("GenjuxTestApp.app").is_dir());
        assert!(apps_dir
            .join("GenjuxTestApp.app/Contents/MacOS/GenjuxTestApp")
            .is_file());
    }

    #[tokio::test]
    async fn dmg_with_no_app_bundle_fails_and_still_detaches() {
        let _guard = DMG_TEST_LOCK.lock().await;
        let workdir = tempfile::tempdir().unwrap();
        let dmg_path = workdir.path().join("test.dmg");
        build_test_dmg(&dmg_path, false).await;

        let apps_dir = workdir.path().join("Applications");
        let adapter = MacOsAdapter::new(apps_dir);

        let err = adapter.install(&dmg_path).await.unwrap_err();
        assert!(err.contains("no .app bundle found"));

        // Confirm hdiutil actually detached rather than leaking a mounted
        // volume: `hdiutil info` should no longer mention our volume name.
        let info = TokioCommand::new("hdiutil")
            .arg("info")
            .output()
            .await
            .unwrap();
        let info_text = String::from_utf8_lossy(&info.stdout);
        assert!(!info_text.contains("GenjuxAdapterTest"));
    }

    #[tokio::test]
    async fn unsupported_extension_is_rejected_without_touching_the_filesystem() {
        let adapter = MacOsAdapter::new(PathBuf::from("/tmp/genjux-unused"));
        let err = adapter
            .install(Path::new("/tmp/whatever.zip"))
            .await
            .unwrap_err();
        assert!(err.contains("unsupported macOS install artifact extension"));
    }

    #[test]
    fn pkg_install_args_target_current_user_home_directory() {
        let args = pkg_install_command_args(Path::new("/tmp/My App.pkg"));
        assert_eq!(
            args,
            vec![
                "-pkg",
                "/tmp/My App.pkg",
                "-target",
                "CurrentUserHomeDirectory"
            ]
        );
    }

    #[tokio::test]
    async fn gatekeeper_assessment_rejects_an_unsigned_ad_hoc_app() {
        let workdir = tempfile::tempdir().unwrap();
        let app_path = workdir.path().join("Unsigned.app");
        tokio::fs::create_dir_all(app_path.join("Contents/MacOS"))
            .await
            .unwrap();
        tokio::fs::write(app_path.join("Contents/MacOS/Unsigned"), b"#!/bin/sh\n")
            .await
            .unwrap();

        let status = assess_gatekeeper(&app_path).await;
        assert!(
            matches!(status, GatekeeperStatus::Rejected { .. }),
            "expected Rejected for an unsigned ad-hoc app, got {status:?}"
        );
    }

    #[test]
    fn extracts_mount_point_and_dev_entry_from_a_real_hdiutil_plist_shape() {
        // Trimmed fixture matching the actual shape of `hdiutil attach
        // -plist` output (verified by hand against a real invocation),
        // rather than a made-up structure.
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>system-entities</key>
	<array>
		<dict>
			<key>dev-entry</key>
			<string>/dev/disk21</string>
			<key>potentially-mountable</key>
			<false/>
		</dict>
		<dict>
			<key>dev-entry</key>
			<string>/dev/disk21s1</string>
			<key>mount-point</key>
			<string>/Volumes/GenjuxTest</string>
			<key>potentially-mountable</key>
			<true/>
		</dict>
	</array>
</dict>
</plist>"#;

        let (mount_point, dev_entry) =
            find_mounted_volume(plist).expect("should find the mounted entry");
        assert_eq!(mount_point, "/Volumes/GenjuxTest");
        assert_eq!(dev_entry, "/dev/disk21s1");
    }
}
