import Cocoa

/// Root content view controller for the main window: swaps between the
/// Home screen (#60) and Search results (#61) as its single child
/// content view controller. Later issues (#62 app detail, #63 install
/// progress, #64 installed/updates) plug into this same mechanism. Built
/// entirely in code, no Storyboards/XIBs.
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
        setContent(HomeViewController())
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
