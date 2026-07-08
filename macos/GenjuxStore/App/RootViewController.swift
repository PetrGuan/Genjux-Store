import Cocoa

/// Root content view controller for the main window. Currently just a
/// placeholder label proving the programmatic-AppKit scaffold actually
/// builds and runs — later issues (#60 home screen, #61 search, #62 app
/// detail, #63 install progress, #64 installed/updates) will replace this
/// with real navigation between those screens (e.g. via
/// `NSSplitViewController` or simple content-view swapping), all built in
/// code, no Storyboards/XIBs.
final class RootViewController: NSViewController {
    override func loadView() {
        view = NSView(frame: NSRect(x: 0, y: 0, width: 960, height: 640))
    }

    override func viewDidLoad() {
        super.viewDidLoad()

        let label = NSTextField(labelWithString: "Genjux Store — coming soon")
        label.font = .systemFont(ofSize: 20, weight: .medium)
        label.textColor = .secondaryLabelColor
        label.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(label)

        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        ])
    }
}
