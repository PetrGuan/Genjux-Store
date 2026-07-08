//! Platform-specific install adapters (issues #12/#13/#14), each
//! implementing [`crate::orchestrate::PlatformAdapter`].
//!
//! Every submodule is only compiled on its target OS, since it shells out
//! to OS-specific tools that don't exist elsewhere (hdiutil/installer/
//! spctl on macOS, msiexec on Windows, dpkg/rpm/AppImage tooling on
//! Linux).

#[cfg(target_os = "macos")]
pub mod macos;
