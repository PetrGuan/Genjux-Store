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
    ///
    /// `.convertFromSnakeCase` matches every response type's wire shape:
    /// the core service's JSON is produced by plain `#[derive(Serialize)]`
    /// Rust structs, whose field names are already snake_case (e.g.
    /// `download_url`), and un-renamed enum variants (e.g. `"MacOS"`) —
    /// see Models.swift's doc comment for how those map onto Swift.
    ///
    /// `timeout` defaults to `URLRequest`'s own default (60s), which is
    /// far too short for `discover()` on a cold cache — a real,
    /// uncached `GET /discover/macos` call fetches+classifies a release
    /// per candidate repo and took ~2m51s in testing (see #55/#56).
    /// Found via a real timeout while manually testing the Search screen
    /// (#61) against a fresh runtime dir, not a hypothetical.
    func get<T: Decodable>(_ path: String, as type: T.Type, timeout: TimeInterval = 60) async throws -> T {
        let info = try await serviceInfo()
        var request = URLRequest(url: baseURL(info).appendingPathComponent(path))
        request.setValue("Bearer \(info.token)", forHTTPHeaderField: "Authorization")
        request.timeoutInterval = timeout

        let (data, response) = try await session.data(for: request)
        try Self.checkResponse(response, data: data)

        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(T.self, from: data)
    }

    /// Calls `GET /health` — the simplest possible proof that lazy-start,
    /// discovery, and authenticated request/response round-tripping all
    /// actually work end-to-end.
    func health() async throws -> HealthResponse {
        try await get("/health", as: HealthResponse.self)
    }

    /// Calls `GET /discover/:platform` (#54-#56) — the Home screen's
    /// (#60) recommended-software feed. `platform` defaults to `"macos"`
    /// since that's the only flagship platform this GUI targets so far.
    /// Uses a generous 5-minute timeout: see `get`'s doc comment for why
    /// the default 60s isn't enough for a cold cache.
    func discover(platform: String = "macos") async throws -> [RecommendedApp] {
        try await get("/discover/\(platform)", as: [RecommendedApp].self, timeout: 300)
    }

    /// Calls `GET /repos/:owner/:repo/packages` (#4-#8) — every classified
    /// release asset for an arbitrary repo, not limited to the curated
    /// recommended feed. Backs the Search screen (#61)'s "install
    /// anything on GitHub" escape hatch from the original product pitch.
    func packages(owner: String, repo: String) async throws -> [InstallablePackage] {
        try await get("/repos/\(owner)/\(repo)/packages", as: [InstallablePackage].self)
    }

    /// Calls `GET /repos/:owner/:repo/metadata` (#57) — README excerpt,
    /// stars, and last-release date for the App detail screen (#62).
    func metadata(owner: String, repo: String) async throws -> RepoMetadata {
        try await get("/repos/\(owner)/\(repo)/metadata", as: RepoMetadata.self)
    }

    /// Calls `POST /install` (#11/#16) — starts a real install and
    /// returns its install id, which `installStatus(id:)` polls. Backs
    /// the Install progress screen (#63).
    func startInstall(owner: String, repo: String) async throws -> String {
        struct InstallRequest: Encodable {
            let owner: String
            let repo: String
        }

        let info = try await serviceInfo()
        var request = URLRequest(url: baseURL(info).appendingPathComponent("/install"))
        request.httpMethod = "POST"
        request.setValue("Bearer \(info.token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(InstallRequest(owner: owner, repo: repo))

        let (data, response) = try await session.data(for: request)
        try Self.checkResponse(response, data: data)

        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(InstallStarted.self, from: data).installId
    }

    /// Calls `GET /installs/:id` (#11/#16) — the latest known stage of a
    /// previously started install. Backs the Install progress screen
    /// (#63)'s polling loop.
    func installStatus(id: String) async throws -> InstallStage {
        try await get("/installs/\(id)", as: InstallStage.self)
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

    /// Whether this error represents a "not found" response (404) — the
    /// core service uses this for both "repo has no releases" and
    /// "repo doesn't exist", which the Search screen (#61) treats the
    /// same way: nothing installable here, not a real error.
    var isNotFound: Bool {
        if case .httpError(let status, _) = self, status == 404 {
            return true
        }
        return false
    }
}
