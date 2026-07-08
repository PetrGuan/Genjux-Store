import Cocoa

/// Search screen (#61): looks up an arbitrary `owner/repo` (typed into the
/// window toolbar's search field — see `AppDelegate`) and shows every
/// classified release asset for it via `GET /repos/:owner/:repo/packages`
/// — independent of the curated Home-screen feed (#60). This is the
/// "install anything on GitHub" escape hatch from the original product
/// pitch. Built entirely in code, no Storyboards/XIBs.
final class SearchViewController: NSViewController {
    private let owner: String
    private let repo: String

    /// Called when the user taps "Back to recommended" — wired by
    /// `RootViewController` to switch back to the Home screen.
    var onBackTapped: (() -> Void)?

    private let tableView = NSTableView()
    private let scrollView = NSScrollView()
    private let statusLabel = NSTextField(labelWithString: "")
    private let titleLabel = NSTextField(labelWithString: "")
    private let backButton = NSButton(title: "\u{2190} Recommended", target: nil, action: nil)

    private var packages: [InstallablePackage] = []

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
        setUpHeader()
        setUpTable()
        setUpStatusLabel()
        Task { await loadPackages() }
    }

    private func setUpHeader() {
        backButton.bezelStyle = .rounded
        backButton.target = self
        backButton.action = #selector(backTapped)
        backButton.translatesAutoresizingMaskIntoConstraints = false

        titleLabel.stringValue = "\(owner)/\(repo)"
        titleLabel.font = .boldSystemFont(ofSize: 16)
        titleLabel.translatesAutoresizingMaskIntoConstraints = false

        view.addSubview(backButton)
        view.addSubview(titleLabel)

        NSLayoutConstraint.activate([
            backButton.topAnchor.constraint(equalTo: view.topAnchor, constant: 16),
            backButton.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 20),

            titleLabel.centerYAnchor.constraint(equalTo: backButton.centerYAnchor),
            titleLabel.leadingAnchor.constraint(equalTo: backButton.trailingAnchor, constant: 16),
        ])
    }

    private func setUpTable() {
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("asset"))
        column.title = "Release asset"
        column.width = 900
        tableView.addTableColumn(column)
        tableView.headerView = NSTableHeaderView()
        tableView.dataSource = self
        tableView.delegate = self
        tableView.usesAlternatingRowBackgroundColors = true
        tableView.rowSizeStyle = .default

        scrollView.documentView = tableView
        scrollView.hasVerticalScroller = true
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(scrollView)

        NSLayoutConstraint.activate([
            scrollView.topAnchor.constraint(equalTo: backButton.bottomAnchor, constant: 16),
            scrollView.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            scrollView.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
    }

    private func setUpStatusLabel() {
        statusLabel.font = .systemFont(ofSize: 14)
        statusLabel.textColor = .secondaryLabelColor
        statusLabel.alignment = .center
        statusLabel.stringValue = "Searching \(owner)/\(repo)…"
        statusLabel.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(statusLabel)

        NSLayoutConstraint.activate([
            statusLabel.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            statusLabel.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            statusLabel.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: 40),
            statusLabel.trailingAnchor.constraint(lessThanOrEqualTo: view.trailingAnchor, constant: -40),
        ])
    }

    @objc private func backTapped() {
        onBackTapped?()
    }

    private func loadPackages() async {
        do {
            let packages = try await CoreServiceClient.shared.packages(owner: owner, repo: repo)
            self.packages = packages
            tableView.reloadData()

            if packages.isEmpty {
                statusLabel.stringValue = "\(owner)/\(repo) has no release assets."
                statusLabel.isHidden = false
            } else {
                statusLabel.isHidden = true
            }
        } catch let error as CoreServiceError where error.isNotFound {
            statusLabel.stringValue = "\(owner)/\(repo) has no releases (or doesn't exist)."
            statusLabel.isHidden = false
        } catch {
            statusLabel.stringValue = "Search failed: \(error.localizedDescription)"
            statusLabel.isHidden = false
        }
    }
}

extension SearchViewController: NSTableViewDataSource {
    func numberOfRows(in tableView: NSTableView) -> Int {
        packages.count
    }
}

extension SearchViewController: NSTableViewDelegate {
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let identifier = NSUserInterfaceItemIdentifier("PackageRow")
        let package = packages[row]

        let cell: NSTableCellView
        if let reused = tableView.makeView(withIdentifier: identifier, owner: self) as? NSTableCellView {
            cell = reused
        } else {
            cell = NSTableCellView()
            cell.identifier = identifier
            let textField = NSTextField(labelWithString: "")
            textField.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(textField)
            cell.textField = textField
            NSLayoutConstraint.activate([
                textField.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 8),
                textField.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -8),
                textField.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
        }

        cell.textField?.stringValue = Self.describe(package)
        return cell
    }

    private static func describe(_ package: InstallablePackage) -> String {
        let platform = package.classification.platform.map { "\($0.rawValue)" } ?? "unclassified"
        let kind = package.classification.kind.map { " (\($0.rawValue))" } ?? ""
        return "\(package.assetName) — \(platform)\(kind)"
    }
}
