import Foundation

/// What a running `genjuxd` instance publishes so other local processes
/// can find it instead of starting a second instance — the Swift-side
/// mirror of `genjux_core::lifecycle::ServiceInfo` (core/src/lifecycle.rs).
/// Field names/types must stay in sync with that Rust struct: this is
/// decoded directly from the same `genjuxd.json` file/JSON shape.
struct ServiceInfo: Codable, Equatable {
    let port: UInt16
    let token: String
    let pid: Int32
}
