import Cocoa

/// Root content view controller for the main window. Currently just
/// embeds `HomeViewController` (#60) as its single child — later issues
/// (#61 search, #62 app detail, #63 install progress, #64
/// installed/updates) will introduce real navigation between screens
/// (e.g. via `NSSplitViewController` or content-view swapping), all built
/// in code, no Storyboards/XIBs.
final class RootViewController: NSViewController {
    override func loadView() {
        view = NSView(frame: NSRect(x: 0, y: 0, width: 960, height: 640))
    }

    override func viewDidLoad() {
        super.viewDidLoad()

        let home = HomeViewController()
        addChild(home)
        home.view.frame = view.bounds
        home.view.autoresizingMask = [.width, .height]
        view.addSubview(home.view)
    }
}
