import Cocoa

/// Home screen (#60): a scrollable grid of recommended macOS software,
/// backed by `GET /discover/macos` (#54-#56). Built entirely in code —
/// `NSCollectionView` + `NSCollectionViewFlowLayout`, no Storyboards/XIBs.
final class HomeViewController: NSViewController {
    private let itemIdentifier = NSUserInterfaceItemIdentifier("AppCardItem")

    private var apps: [RecommendedApp] = []
    private let collectionView = NSCollectionView()
    private let scrollView = NSScrollView()
    private let statusLabel = NSTextField(labelWithString: "")
    private let retryButton = NSButton(title: "Retry", target: nil, action: nil)

    /// Called when the user taps "Details" on a card — `RootViewController`
    /// wires this to open the App detail screen (#62) for that app.
    var onAppSelected: ((RecommendedApp) -> Void)?

    override func loadView() {
        view = NSView(frame: NSRect(x: 0, y: 0, width: 960, height: 640))
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        setUpCollectionView()
        setUpStatusOverlay()
        Task { await loadRecommendedApps() }
    }

    private func setUpCollectionView() {
        let layout = NSCollectionViewFlowLayout()
        layout.itemSize = NSSize(width: 240, height: 160)
        layout.minimumInteritemSpacing = 16
        layout.minimumLineSpacing = 16
        layout.sectionInset = NSEdgeInsets(top: 20, left: 20, bottom: 20, right: 20)

        collectionView.collectionViewLayout = layout
        collectionView.dataSource = self
        collectionView.delegate = self
        collectionView.register(AppCardItem.self, forItemWithIdentifier: itemIdentifier)
        collectionView.isSelectable = true
        collectionView.backgroundColors = [.clear]

        scrollView.documentView = collectionView
        scrollView.hasVerticalScroller = true
        scrollView.drawsBackground = false
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(scrollView)

        NSLayoutConstraint.activate([
            scrollView.topAnchor.constraint(equalTo: view.topAnchor),
            scrollView.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            scrollView.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
    }

    private func setUpStatusOverlay() {
        statusLabel.font = .systemFont(ofSize: 14)
        statusLabel.textColor = .secondaryLabelColor
        statusLabel.alignment = .center
        statusLabel.translatesAutoresizingMaskIntoConstraints = false
        statusLabel.stringValue = "Loading recommended apps…"

        retryButton.bezelStyle = .rounded
        retryButton.target = self
        retryButton.action = #selector(retryTapped)
        retryButton.isHidden = true
        retryButton.translatesAutoresizingMaskIntoConstraints = false

        view.addSubview(statusLabel)
        view.addSubview(retryButton)

        NSLayoutConstraint.activate([
            statusLabel.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            statusLabel.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            statusLabel.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: 40),
            statusLabel.trailingAnchor.constraint(lessThanOrEqualTo: view.trailingAnchor, constant: -40),

            retryButton.topAnchor.constraint(equalTo: statusLabel.bottomAnchor, constant: 12),
            retryButton.centerXAnchor.constraint(equalTo: view.centerXAnchor),
        ])
    }

    @objc private func retryTapped() {
        statusLabel.stringValue = "Loading recommended apps…"
        statusLabel.isHidden = false
        retryButton.isHidden = true
        Task { await loadRecommendedApps() }
    }

    private func loadRecommendedApps() async {
        do {
            let apps = try await CoreServiceClient.shared.discover(platform: "macos")
            self.apps = apps
            collectionView.reloadData()

            if apps.isEmpty {
                statusLabel.stringValue = "No recommended apps found yet."
                statusLabel.isHidden = false
            } else {
                statusLabel.isHidden = true
            }
            retryButton.isHidden = true
        } catch {
            // The first real discover() call can take a couple of
            // minutes (it fetches+classifies a release per candidate
            // repo — see #55/#56's real-network verification) so a
            // timeout here isn't necessarily a bug; surface it plainly
            // and let the user retry rather than silently retrying
            // forever or crashing.
            statusLabel.stringValue = "Couldn't load recommended apps: \(error.localizedDescription)"
            statusLabel.isHidden = false
            retryButton.isHidden = false
        }
    }
}

extension HomeViewController: NSCollectionViewDataSource {
    func collectionView(_ collectionView: NSCollectionView, numberOfItemsInSection section: Int) -> Int {
        apps.count
    }

    func collectionView(
        _ collectionView: NSCollectionView,
        itemForRepresentedObjectAt indexPath: IndexPath
    ) -> NSCollectionViewItem {
        let item = collectionView.makeItem(withIdentifier: itemIdentifier, for: indexPath)
        if let cardItem = item as? AppCardItem {
            cardItem.configure(with: apps[indexPath.item])
            cardItem.onDetailsTapped = { [weak self] app in
                self?.onAppSelected?(app)
            }
        }
        return item
    }
}

extension HomeViewController: NSCollectionViewDelegate {}
