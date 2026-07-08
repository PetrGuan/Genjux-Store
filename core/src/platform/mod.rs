//! Platform-specific install adapters (issues #12/#13/#14), each
//! implementing [`crate::orchestrate::PlatformAdapter`].
//!
//! Every submodule is only compiled on its target OS, since it shells out
//! to OS-specific tools that don't exist elsewhere (hdiutil/installer/
//! spctl on macOS, msiexec on Windows, dpkg/rpm/AppImage tooling on
//! Linux).

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

/// Builds the [`crate::orchestrate::PlatformAdapter`] for whichever OS
/// this binary is actually running on, so callers (`genjuxd`'s `main`)
/// don't need their own `cfg`-gated dispatch. `install_dir` is where
/// macOS `.app` bundles / Linux `.AppImage`s are placed; Windows ignores
/// it (its installers manage their own destination).
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub fn current_adapter(
    install_dir: std::path::PathBuf,
) -> std::sync::Arc<dyn crate::orchestrate::PlatformAdapter> {
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(macos::MacOsAdapter::new(install_dir))
    }
    #[cfg(target_os = "linux")]
    {
        std::sync::Arc::new(linux::LinuxAdapter::new(install_dir))
    }
    #[cfg(target_os = "windows")]
    {
        let _ = install_dir;
        std::sync::Arc::new(windows::WindowsAdapter::new())
    }
}
