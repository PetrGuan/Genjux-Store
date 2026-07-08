import Foundation

/// Thin URLSession-based client for the core service's local HTTP API
/// (core/src/api.rs, issue #16) — the macOS GUI's equivalent of the CLI's
/// HTTP calls (#20). Lazily starts `genjuxd` on first use via
/// `ServiceLifecycle`, then talks to it exactly like the CLI/AI-agent
/// callers do: same endpoints, same bearer-token auth, so all three
/// surfaces share one core service and its state (per PLAN.md section 1).
///
/// An `actor` (not a plain class) so the cached `ServiceInfo` is safe to
/// access from concurrent screens without a manual lock — Swift
/// concurrency's structured equivalent of the `Mutex`-guarded state used
/// throughout the Rust core (e.g. `AppState.installs` in api.rs).
actor CoreServiceClient {
    static let shared = CoreServiceClient()

    private let session: URLSession
    private var cachedInfo: ServiceInfo?

    init(session: URLSession = .shared) {
        self.session = session
    }

    /// Only exposed for tests, so a test can point a fresh client at an
    /// isolated `GENJUX_RUNTIME_DIR` without disturbing `.shared`'s cached
    /// state across test runs.
    static func makeForTesting() -> CoreServiceClient {
        CoreServiceClient()
    }

    private func serviceInfo() async throws -> ServiceInfo {
        if let cached = cachedInfo {
            return cached
        }
        let info = try await ServiceLifecycle.ensureServiceRunning()
        cachedInfo = info
        return info
    }

    private func baseURL(_ info: ServiceInfo) -> URL {
        URL(string: "http://127.0.0.1:\(info.port)")!
    }

    /// Performs an authenticated GET against `path` (e.g. `/health`),
    /// decoding the JSON response as `T`. Lazily starts/discovers
    /// `genjuxd` first if this client hasn't already done so.
    func get<T: Decodable>(_ path: String, as type: T.Type) async throws -> T {
        let info = try await serviceInfo()
        var request = URLRequest(url: baseURL(info).appendingPathComponent(path))
        request.setValue("Bearer \(info.token)", forHTTPHeaderField: "Authorization")

        let (data, response) = try await session.data(for: request)
        try Self.checkResponse(response, data: data)
        return try JSONDecoder().decode(T.self, from: data)
    }

    /// Calls `GET /health` — the simplest possible proof that lazy-start,
    /// discovery, and authenticated request/response round-tripping all
    /// actually work end-to-end. Later issues (#60-#64) add typed methods
    /// for the product endpoints (`/discover`, `/repos/.../packages`,
    /// `/repos/.../metadata`, `/installed`, ...) as each screen needs them.
    func health() async throws -> HealthResponse {
        try await get("/health", as: HealthResponse.self)
    }

    private static func checkResponse(_ response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else {
            return
        }
        guard (200...299).contains(http.statusCode) else {
            let body = String(data: data, encoding: .utf8) ?? ""
            throw CoreServiceError.httpError(status: http.statusCode, body: body)
        }
    }
}

/// Mirrors `core::api::HealthResponse` (core/src/api.rs).
struct HealthResponse: Codable, Equatable {
    let status: String
    let version: String
}

enum CoreServiceError: Error, LocalizedError {
    case httpError(status: Int, body: String)

    var errorDescription: String? {
        switch self {
        case .httpError(let status, let body):
            return "HTTP \(status): \(body)"
        }
    }
}
