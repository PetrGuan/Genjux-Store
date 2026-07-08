import Cocoa

/// App detail screen (#62): README excerpt, star count, last-release
/// date, and a real install button — reached by tapping a card on the
/// Home screen (#60). Trust-signal presentation here is intentionally
/// scoped to what's knowable *before* download (stars, publisher
/// activity via last-release date): signature/notarization status and
/// checksum verification results (PLAN.md section 5) only become known
/// *during* an actual download+verify, which is #63's job, not this
/// screen's. Built entirely in code, no Storyboards/XIBs.
final class AppDetailViewController: NSViewController {
    private let app: RecommendedApp

    /// Called when the user taps "Back to recommended" — wired by
    /// `RootViewController` to switch back to the Home screen.
    var onBackTapped: (() -> Void)?

    private let backButton = NSButton(title: "\u{2190} Recommended", target: nil, action: nil)
    private let nameLabel = NSTextField(labelWithString: "")
    private let starsLabel = NSTextField(labelWithString: "")
    private let lastReleaseLabel = NSTextField(labelWithString: "")
    private let descriptionLabel = NSTextField(wrappingLabelWithString: "")
    private let readmeLabel = NSTextField(wrappingLabelWithString: "")
    private let packageLabel = NSTextField(labelWithString: "")
    private let installButton = NSButton(title: "Install", target: nil, action: nil)

    init(app: RecommendedApp) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used — this app builds all UI in code")
    }

    override func loadView() {
        view = NSView(frame: NSRect(x: 0, y: 0, width: 960, height: 640))
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        setUpLayout()
        populateKnownFields()
        Task { await loadMetadata() }
    }

    private func setUpLayout() {
        backButton.bezelStyle = .rounded
        backButton.target = self
        backButton.action = #selector(backTapped)

        nameLabel.font = .boldSystemFont(ofSize: 22)
        starsLabel.font = .systemFont(ofSize: 13)
        starsLabel.textColor = .secondaryLabelColor
        lastReleaseLabel.font = .systemFont(ofSize: 13)
        lastReleaseLabel.textColor = .secondaryLabelColor
        lastReleaseLabel.stringValue = "Last release: loading…"

        descriptionLabel.font = .systemFont(ofSize: 13)
        descriptionLabel.maximumNumberOfLines = 3

        let readmeHeading = NSTextField(labelWithString: "README")
        readmeHeading.font = .boldSystemFont(ofSize: 13)
        readmeLabel.font = .systemFont(ofSize: 12)
        readmeLabel.textColor = .secondaryLabelColor
        readmeLabel.maximumNumberOfLines = 0
        readmeLabel.stringValue = "Loading…"

        packageLabel.font = .systemFont(ofSize: 12)
        packageLabel.textColor = .secondaryLabelColor

        installButton.bezelStyle = .rounded
        installButton.controlSize = .large
        installButton.target = self
        installButton.action = #selector(installTapped)

        let stack = NSStackView(views: [
            backButton,
            nameLabel,
            starsLabel,
            lastReleaseLabel,
            descriptionLabel,
            readmeHeading,
            readmeLabel,
            packageLabel,
            installButton,
        ])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.setCustomSpacing(20, after: backButton)
        stack.setCustomSpacing(20, after: descriptionLabel)
        stack.setCustomSpacing(4, after: readmeHeading)
        stack.setCustomSpacing(20, after: readmeLabel)
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)

        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: view.topAnchor, constant: 20),
            stack.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(lessThanOrEqualTo: view.trailingAnchor, constant: -24),
            descriptionLabel.widthAnchor.constraint(equalTo: stack.widthAnchor),
            readmeLabel.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
    }

    private func populateKnownFields() {
        nameLabel.stringValue = "\(app.owner)/\(app.repo)"
        starsLabel.stringValue = "\u{2605} \(NumberFormatter.localizedString(from: NSNumber(value: app.stars), number: .decimal)) stars"
        descriptionLabel.stringValue = app.description ?? "No description available."

        let platform = app.package.classification.platform.map(\.rawValue) ?? "unclassified"
        let kind = app.package.classification.kind.map { " (\($0.rawValue))" } ?? ""
        packageLabel.stringValue = "\(app.releaseTag): \(app.package.assetName) — \(platform)\(kind)"
    }

    @objc private func backTapped() {
        onBackTapped?()
    }

    @objc private func installTapped() {
        // Full install orchestration UI lands in #63; for now, confirm
        // the button is wired to this app rather than shipping it as
        // dead/misleading UI (same placeholder convention as the Home
        // screen's card Install button, #60).
        let alert = NSAlert()
        alert.messageText = "Install \(app.owner)/\(app.repo)"
        alert.informativeText = "The install flow isn't implemented yet — see issue #63."
        alert.alertStyle = .informational
        alert.runModal()
    }

    private func loadMetadata() async {
        do {
            let metadata = try await CoreServiceClient.shared.metadata(owner: app.owner, repo: app.repo)
            readmeLabel.stringValue = metadata.readmeExcerpt ?? "No README available."
            lastReleaseLabel.stringValue = "Last release: \(Self.formatDate(metadata.lastReleaseAt))"
        } catch {
            readmeLabel.stringValue = "Couldn't load the README: \(error.localizedDescription)"
            lastReleaseLabel.stringValue = "Last release: unavailable"
        }
    }

    /// Formats the core service's ISO 8601 `last_release_at` timestamp
    /// (RFC 3339, as GitHub's API returns it) into something readable,
    /// falling back to the raw string if parsing fails rather than
    /// hiding a real value behind a formatting bug.
    private static func formatDate(_ iso8601: String?) -> String {
        guard let iso8601 else {
            return "unknown"
        }
        guard let date = ISO8601DateFormatter().date(from: iso8601) else {
            return iso8601
        }
        let formatter = DateFormatter()
        formatter.dateStyle = .medium
        formatter.timeStyle = .none
        return formatter.string(from: date)
    }
}
