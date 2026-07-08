//! Genjux-Store core service library.
//!
//! This crate will host the discovery/classification pipeline, download
//! manager, install orchestration, local HTTP API (axum), and MCP server
//! (rmcp) described in `.copilot-workflow/PLAN.md`. For now it exposes the
//! pluggable [`source`] abstraction, the release-asset [`classify`]
//! pipeline, the standardized [`package::InstallablePackage`] schema and
//! cache, and a version string; the rest of the business logic lands in
//! the Phase 0 issues tracked at
//! <https://github.com/PetrGuan/Genjux-Store/issues/22>.

pub mod api;
pub mod audit;
pub mod classify;
pub mod curator;
pub mod download;
pub mod mcp;
pub mod orchestrate;
pub mod package;
pub mod platform;
pub mod registry;
pub mod source;
pub mod verify;

/// Returns the core crate's version, as set by Cargo at build time.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_not_empty() {
        assert!(!version().is_empty());
    }
}
