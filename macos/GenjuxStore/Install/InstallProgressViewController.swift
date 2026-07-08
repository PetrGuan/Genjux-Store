import Cocoa

/// Install progress screen (#63): starts a real install via
/// `POST /install` and polls `GET /installs/:id` (#11/#16) until it
/// reaches a terminal stage, showing each stage as it happens. Built
/// entirely in code, no Storyboards/XIBs.
///
/// Deliberately does **not** try to suppress or work around OS security
/// prompts (Gatekeeper on macOS) that may appear during the real
/// `Installing` stage — per PLAN.md section 4/5, those are expected,
/// user-beneficial steps to show, not obstacles to hide.
final class InstallProgressViewController: NSViewController {
    private let owner: String
    private let repo: String

    /// Called when the user taps "Back to recommended" (available once
    /// the install reaches a terminal stage, or immediately on a
    /// start-install failure) — wired by `RootViewController`.
    var onBackTapped: (() -> Void)?

    private let titleLabel = NSTextField(labelWithString: "")
    private let stageLabel = NSTextField(labelWithString: "")
    private let progressIndicator = NSProgressIndicator()
    private let detailLabel = NSTextField(wrappingLabelWithString: "")
    private let doneButton = NSButton(title: "\u{2190} Recommended", target: nil, action: nil)

    private var pollTask: Task<Void, Never>?

    init(owner: String, repo: String) {
        self.owner = owner
        self.repo = repo
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
        Task { await startInstall() }
    }

    deinit {
        pollTask?.cancel()
    }

    private func setUpLayout() {
        titleLabel.stringValue = "Installing \(owner)/\(repo)"
        titleLabel.font = .boldSystemFont(ofSize: 18)

        stageLabel.font = .systemFont(ofSize: 14)
        stageLabel.stringValue = "Starting…"

        progressIndicator.style = .bar
        progressIndicator.isIndeterminate = true
        progressIndicator.startAnimation(nil)
        progressIndicator.widthAnchor.constraint(equalToConstant: 400).isActive = true

        detailLabel.font = .systemFont(ofSize: 12)
        detailLabel.textColor = .secondaryLabelColor
        detailLabel.maximumNumberOfLines = 0

        doneButton.bezelStyle = .rounded
        doneButton.target = self
        doneButton.action = #selector(backTapped)
        doneButton.isHidden = true

        let stack = NSStackView(views: [titleLabel, stageLabel, progressIndicator, detailLabel, doneButton])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.setCustomSpacing(20, after: titleLabel)
        stack.setCustomSpacing(20, after: progressIndicator)
        stack.setCustomSpacing(20, after: detailLabel)
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)

        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            stack.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: 40),
            stack.trailingAnchor.constraint(lessThanOrEqualTo: view.trailingAnchor, constant: -40),
        ])
    }

    @objc private func backTapped() {
        onBackTapped?()
    }

    private func startInstall() async {
        do {
            let installId = try await CoreServiceClient.shared.startInstall(owner: owner, repo: repo)
            pollTask = Task { await pollStatus(installId: installId) }
        } catch {
            showTerminal(stageText: "Failed to start", detail: error.localizedDescription)
        }
    }

    private func pollStatus(installId: String) async {
        while !Task.isCancelled {
            do {
                let stage = try await CoreServiceClient.shared.installStatus(id: installId)
                render(stage)
                if stage.isTerminal {
                    return
                }
            } catch {
                showTerminal(stageText: "Lost track of the install", detail: error.localizedDescription)
                return
            }
            try? await Task.sleep(nanoseconds: 300_000_000) // 300ms
        }
    }

    private func render(_ stage: InstallStage) {
        switch stage {
        case .resolving:
            stageLabel.stringValue = "Resolving release…"
            detailLabel.stringValue = ""
        case .downloading(let bytesDownloaded, let totalBytes):
            stageLabel.stringValue = "Downloading…"
            if let totalBytes, totalBytes > 0 {
                let percent = Int(Double(bytesDownloaded) / Double(totalBytes) * 100)
                detailLabel.stringValue = "\(Self.formatBytes(bytesDownloaded)) of \(Self.formatBytes(totalBytes)) (\(percent)%)"
            } else {
                detailLabel.stringValue = "\(Self.formatBytes(bytesDownloaded)) downloaded"
            }
        case .verified(let sha256, let matchedPublishedChecksum):
            stageLabel.stringValue = "Verified"
            // Trust model (PLAN.md section 5): disclose plainly whether
            // this matched an official checksum, or is just a
            // self-computed hash with nothing to compare against —
            // never present the latter as if it were a safety guarantee.
            let trustNote = matchedPublishedChecksum
                ? "Matches the checksum published with this release."
                : "No official checksum was published to compare against; this is only the file's own computed hash."
            detailLabel.stringValue = "SHA-256: \(sha256)\n\(trustNote)"
        case .installing:
            stageLabel.stringValue = "Installing…"
            detailLabel.stringValue = "If macOS shows a Gatekeeper security prompt, that's expected — review it before continuing."
        case .succeeded:
            showTerminal(stageText: "Installed \(owner)/\(repo)", detail: "")
        case .failed(let reason):
            showTerminal(stageText: "Install failed", detail: reason)
        }
    }

    private func showTerminal(stageText: String, detail: String) {
        stageLabel.stringValue = stageText
        detailLabel.stringValue = detail
        progressIndicator.stopAnimation(nil)
        progressIndicator.isHidden = true
        doneButton.isHidden = false
    }

    private static func formatBytes(_ bytes: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
    }
}
