//! Placeholder entry point for the `genjuxd` core service binary.
//!
//! The real local HTTP API + MCP server implementation lands in later
//! Phase 0 issues (see https://github.com/PetrGuan/Genjux-Store/issues/22).
//! For now this just proves the workspace builds and links against
//! `genjux_core`.

fn main() {
    println!("genjuxd {}", genjux_core::version());
}
