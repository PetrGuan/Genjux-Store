import Cocoa

/// A single recommended-app card in the Home screen's grid (#60). Built
/// entirely in code — no Storyboards/XIBs, matching every other screen in
/// this app.
final class AppCardItem: NSCollectionViewItem {
    private let nameLabel = NSTextField(labelWithString: "")
    private let starsLabel = NSTextField(labelWithString: "")
    private let descriptionLabel = NSTextField(wrappingLabelWithString: "")
    private let installButton = NSButton(title: "Install", target: nil, action: nil)
    private let detailsButton = NSButton(title: "Details", target: nil, action: nil)

    private var owner: String?
    private var repo: String?
    private var app: RecommendedApp?

    /// Called when the user taps "Details" — `HomeViewController` wires
    /// this to open the App detail screen (#62) for this card's app.
    var onDetailsTapped: ((RecommendedApp) -> Void)?

    override func loadView() {
        let container = NSView()
        container.wantsLayer = true
        container.layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
        container.layer?.cornerRadius = 10
        view = container

        nameLabel.font = .boldSystemFont(ofSize: 14)
        nameLabel.lineBreakMode = .byTruncatingTail
        nameLabel.translatesAutoresizingMaskIntoConstraints = false

        starsLabel.font = .systemFont(ofSize: 12)
        starsLabel.textColor = .secondaryLabelColor
        starsLabel.translatesAutoresizingMaskIntoConstraints = false

        descriptionLabel.font = .systemFont(ofSize: 11)
        descriptionLabel.textColor = .secondaryLabelColor
        descriptionLabel.maximumNumberOfLines = 3
        descriptionLabel.translatesAutoresizingMaskIntoConstraints = false

        installButton.bezelStyle = .rounded
        installButton.target = self
        installButton.action = #selector(installTapped)
        installButton.translatesAutoresizingMaskIntoConstraints = false

        detailsButton.bezelStyle = .rounded
        detailsButton.target = self
        detailsButton.action = #selector(detailsTapped)
        detailsButton.translatesAutoresizingMaskIntoConstraints = false

        container.addSubview(nameLabel)
        container.addSubview(starsLabel)
        container.addSubview(descriptionLabel)
        container.addSubview(installButton)
        container.addSubview(detailsButton)

        NSLayoutConstraint.activate([
            nameLabel.topAnchor.constraint(equalTo: container.topAnchor, constant: 12),
            nameLabel.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 12),
            nameLabel.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -12),

            starsLabel.topAnchor.constraint(equalTo: nameLabel.bottomAnchor, constant: 4),
            starsLabel.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 12),
            starsLabel.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -12),

            descriptionLabel.topAnchor.constraint(equalTo: starsLabel.bottomAnchor, constant: 6),
            descriptionLabel.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 12),
            descriptionLabel.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -12),

            installButton.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 12),
            installButton.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -10),

            detailsButton.leadingAnchor.constraint(equalTo: installButton.trailingAnchor, constant: 8),
            detailsButton.centerYAnchor.constraint(equalTo: installButton.centerYAnchor),
        ])
    }

    /// Fills in this card's content for `app`. Called once per reused
    /// item by `HomeViewController`'s `NSCollectionViewDataSource`, same
    /// as `UICollectionViewCell` reuse on iOS.
    func configure(with app: RecommendedApp) {
        self.app = app
        owner = app.owner
        repo = app.repo
        nameLabel.stringValue = "\(app.owner)/\(app.repo)"
        starsLabel.stringValue = "\u{2605} \(formattedStarCount(app.stars))"
        descriptionLabel.stringValue = app.description ?? ""
    }

    private func formattedStarCount(_ stars: UInt64) -> String {
        NumberFormatter.localizedString(from: NSNumber(value: stars), number: .decimal)
    }

    @objc private func installTapped() {
        // The full install-orchestration UI lands in #63; for now, just
        // confirm the button is correctly wired to *this* card's app
        // rather than shipping it as inert, misleading UI.
        guard let owner, let repo else {
            return
        }
        let alert = NSAlert()
        alert.messageText = "Install \(owner)/\(repo)"
        alert.informativeText = "The install flow isn't implemented yet — see issue #63."
        alert.alertStyle = .informational
        alert.runModal()
    }

    @objc private func detailsTapped() {
        guard let app else {
            return
        }
        onDetailsTapped?(app)
    }
}
