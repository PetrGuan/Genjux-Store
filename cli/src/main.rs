//! Placeholder entry point for the `genjux` CLI binary.
//!
//! The real client (search/install/list/update commands talking to the
//! local core HTTP API) lands in a later Phase 0 issue:
//! https://github.com/PetrGuan/Genjux-Store/issues/20

fn main() {
    println!("genjux {}", env!("CARGO_PKG_VERSION"));
}
