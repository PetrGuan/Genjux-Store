import Cocoa

/// Root content view controller for the main window: swaps between the
/// Home screen (#60), Search results (#61), App detail (#62), Install
/// progress (#63), and Installed/updates (#64) as its single child
/// content view controller. Built entirely in code, no Storyboards/XIBs.
final class RootViewController: NSViewController {
    private var currentChild: NSViewController?

    override func loadView() {
        view = NSView(frame: NSRect(x: 0, y: 0, width: 960, height: 640))
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        showHome()
    }

    func showHome() {
        let home = HomeViewController()
        home.onAppSelected = { [weak self] app in self?.showDetail(for: app) }
        home.onInstallRequested = { [weak self] app in self?.showInstall(owner: app.owner, repo: app.repo) }
        setContent(home)
    }

    /// Parses `query` as `owner/repo` and shows the Search screen (#61)
    /// for it. Falls back to Home if `query` is empty (e.g. the user
    /// cleared the toolbar search field) — matches the CLI's own
    /// `owner/repo` parsing convention (cli/src/main.rs's `parse_repo`)
    /// closely enough for a first pass; a full validation error UI can
    /// improve on the bare fallback later if it proves confusing in
    /// practice.
    func search(query: String) {
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            showHome()
            return
        }

        let parts = trimmed.split(separator: "/", maxSplits: 1, omittingEmptySubsequences: true)
        guard parts.count == 2 else {
            showHome()
            return
        }

        let search = SearchViewController(owner: String(parts[0]), repo: String(parts[1]))
        search.onBackTapped = { [weak self] in self?.showHome() }
        setContent(search)
    }

    /// Shows the Installed/updates screen (#64) — reached via the window
    /// toolbar's "Installed" button (see `AppDelegate`).
    func showInstalled() {
        let installedScreen = InstalledViewController()
        installedScreen.onBackTapped = { [weak self] in self?.showHome() }
        setContent(installedScreen)
    }

    /// Shows the App detail screen (#62) for a Home-screen card.
    private func showDetail(for app: RecommendedApp) {
        let detail = AppDetailViewController(app: app)
        detail.onBackTapped = { [weak self] in self?.showHome() }
        detail.onInstallTapped = { [weak self] app in self?.showInstall(owner: app.owner, repo: app.repo) }
        setContent(detail)
    }

    /// Shows the Install progress screen (#63) for `owner/repo`.
    private func showInstall(owner: String, repo: String) {
        let install = InstallProgressViewController(owner: owner, repo: repo)
        install.onBackTapped = { [weak self] in self?.showHome() }
        setContent(install)
    }

    private func setContent(_ child: NSViewController) {
        if let current = currentChild {
            current.view.removeFromSuperview()
            current.removeFromParent()
        }
        addChild(child)
        child.view.frame = view.bounds
        child.view.autoresizingMask = [.width, .height]
        view.addSubview(child.view)
        currentChild = child
    }
}
