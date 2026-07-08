import Cocoa

/// App delegate for the Genjux-Store macOS app — no Storyboards/XIBs
/// (per the project's UI convention). The actual process entry point is
/// `main.swift`, not `@main` on this class: an explicit
/// `NSApplication.shared.run()` is more predictable than relying on
/// `@main`'s synthesized behavior for `NSApplicationDelegate` conformers.
///
/// This is the Phase 1 scaffold (issue #58): a real, buildable,
/// runnable window shell. The actual screens (recommended-apps grid,
/// search, app detail, install progress, installed/updates) land in
/// later issues (#60-#64), each as its own `NSViewController` swapped
/// into this window's content — see `RootViewController` for where that
/// will plug in.
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 960, height: 640),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Genjux Store"
        window.center()
        window.contentViewController = RootViewController()
        window.makeKeyAndOrderFront(nil)

        self.window = window
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}
