import Cocoa

/// Installed apps / updates screen (#64): lists every app installed via
/// Genjux-Store (`GET /installed`, #15) with update-check status
/// (`GET /updates`, #15/#20) and a way to trigger a re-check. Built
/// entirely in code — `NSTableView`, no Storyboards/XIBs.
final class InstalledViewController: NSViewController {
    /// Called when the user taps "Back to recommended".
    var onBackTapped: (() -> Void)?

    private let backButton = NSButton(title: "\u{2190} Recommended", target: nil, action: nil)
    private let titleLabel = NSTextField(labelWithString: "Installed")
    private let refreshButton = NSButton(title: "Check for Updates", target: nil, action: nil)
    private let tableView = NSTableView()
    private let scrollView = NSScrollView()
    private let statusLabel = NSTextField(labelWithString: "")

    private var installed: [InstalledEntry] = []
    private var updatesByRepo: [String: UpdateCheckResult] = [:]

    override func loadView() {
        view = NSView(frame: NSRect(x: 0, y: 0, width: 960, height: 640))
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        setUpHeader()
        setUpTable()
        setUpStatusLabel()
        Task { await load() }
    }

    private func setUpHeader() {
        backButton.bezelStyle = .rounded
        backButton.target = self
        backButton.action = #selector(backTapped)
        backButton.translatesAutoresizingMaskIntoConstraints = false

        titleLabel.font = .boldSystemFont(ofSize: 16)
        titleLabel.translatesAutoresizingMaskIntoConstraints = false

        refreshButton.bezelStyle = .rounded
        refreshButton.target = self
        refreshButton.action = #selector(refreshTapped)
        refreshButton.translatesAutoresizingMaskIntoConstraints = false

        view.addSubview(backButton)
        view.addSubview(titleLabel)
        view.addSubview(refreshButton)

        NSLayoutConstraint.activate([
            backButton.topAnchor.constraint(equalTo: view.topAnchor, constant: 16),
            backButton.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 20),

            titleLabel.centerYAnchor.constraint(equalTo: backButton.centerYAnchor),
            titleLabel.leadingAnchor.constraint(equalTo: backButton.trailingAnchor, constant: 16),

            refreshButton.centerYAnchor.constraint(equalTo: backButton.centerYAnchor),
            refreshButton.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -20),
        ])
    }

    private func setUpTable() {
        let nameColumn = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("name"))
        nameColumn.title = "App"
        nameColumn.width = 320
        let versionColumn = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("version"))
        versionColumn.title = "Installed version"
        versionColumn.width = 200
        let statusColumn = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("status"))
        statusColumn.title = "Status"
        statusColumn.width = 380

        tableView.addTableColumn(nameColumn)
        tableView.addTableColumn(versionColumn)
        tableView.addTableColumn(statusColumn)
        tableView.dataSource = self
        tableView.delegate = self
        tableView.usesAlternatingRowBackgroundColors = true

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
        statusLabel.stringValue = "Loading installed apps…"
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

    @objc private func refreshTapped() {
        Task { await loadUpdatesOnly() }
    }

    private func load() async {
        do {
            let installed = try await CoreServiceClient.shared.installed()
            self.installed = installed
            tableView.reloadData()

            if installed.isEmpty {
                statusLabel.stringValue = "No apps installed yet."
                statusLabel.isHidden = false
            } else {
                statusLabel.isHidden = true
            }
        } catch {
            statusLabel.stringValue = "Couldn't load installed apps: \(error.localizedDescription)"
            statusLabel.isHidden = false
            return
        }

        await loadUpdatesOnly()
    }

    private func loadUpdatesOnly() async {
        do {
            let updates = try await CoreServiceClient.shared.updates()
            updatesByRepo = Dictionary(uniqueKeysWithValues: updates.map { (Self.key(for: $0.repo), $0) })
            tableView.reloadData()
        } catch {
            // Non-fatal: the installed list itself already loaded fine,
            // an update-check failure (e.g. transient network issue)
            // shouldn't blank out what we already know is installed.
        }
    }

    private static func key(for repo: RepoRef) -> String {
        "\(repo.provider)/\(repo.owner)/\(repo.repo)"
    }
}

extension InstalledViewController: NSTableViewDataSource {
    func numberOfRows(in tableView: NSTableView) -> Int {
        installed.count
    }
}

extension InstalledViewController: NSTableViewDelegate {
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let columnId = tableColumn?.identifier.rawValue else {
            return nil
        }
        let entry = installed[row]
        let text: String
        switch columnId {
        case "name":
            text = "\(entry.repo.owner)/\(entry.repo.repo)"
        case "version":
            text = entry.installedTag
        case "status":
            if let update = updatesByRepo[Self.key(for: entry.repo)] {
                text = update.updateAvailable
                    ? "Update available: \(update.latestTag)"
                    : "Up to date"
            } else {
                text = "Checking…"
            }
        default:
            text = ""
        }
        return Self.makeCell(in: tableView, text: text)
    }

    private static func makeCell(in tableView: NSTableView, text: String) -> NSTableCellView {
        let identifier = NSUserInterfaceItemIdentifier("InstalledRow")
        let cell: NSTableCellView
        if let reused = tableView.makeView(withIdentifier: identifier, owner: nil) as? NSTableCellView {
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
        cell.textField?.stringValue = text
        return cell
    }
}
