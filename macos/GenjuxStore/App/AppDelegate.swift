import Cocoa

/// App delegate for the Genjux-Store macOS app — no Storyboards/XIBs
/// (per the project's UI convention). The actual process entry point is
/// `main.swift`, not `@main` on this class: an explicit
/// `NSApplication.shared.run()` is more predictable than relying on
/// `@main`'s synthesized behavior for `NSApplicationDelegate` conformers.
///
/// Owns the main window's toolbar (search field, #61, and an "Installed"
/// button, #64) and forwards their actions to `RootViewController`, which
/// owns the actual screen-swapping between Home (#60), Search (#61),
/// App detail (#62), Install progress (#63), and Installed/updates (#64)
/// content.
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?
    private let rootViewController = RootViewController()

    func applicationDidFinishLaunching(_ notification: Notification) {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 960, height: 640),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Genjux Store"
        window.center()
        window.contentViewController = rootViewController
        configureToolbar(for: window)
        window.makeKeyAndOrderFront(nil)

        self.window = window
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    private func configureToolbar(for window: NSWindow) {
        let toolbar = NSToolbar(identifier: "MainToolbar")
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        window.toolbar = toolbar
        window.toolbarStyle = .unified
    }

    @objc private func performSearch(_ sender: NSSearchField) {
        rootViewController.search(query: sender.stringValue)
    }

    @objc private func showInstalled(_ sender: Any?) {
        rootViewController.showInstalled()
    }
}

extension AppDelegate: NSToolbarDelegate {
    func toolbar(
        _ toolbar: NSToolbar,
        itemForItemIdentifier itemIdentifier: NSToolbarItem.Identifier,
        willBeInsertedIntoToolbar flag: Bool
    ) -> NSToolbarItem? {
        switch itemIdentifier {
        case .genjuxSearch:
            let searchItem = NSSearchToolbarItem(itemIdentifier: itemIdentifier)
            searchItem.searchField.placeholderString = "owner/repo"
            // Fires on Return (NSSearchField is an NSControl; target/action
            // is the "submitted" event, not every keystroke) — an empty
            // submission is how the user gets back to Home (see
            // RootViewController.search's empty-query fallback).
            searchItem.searchField.target = self
            searchItem.searchField.action = #selector(performSearch(_:))
            return searchItem
        case .genjuxInstalled:
            let item = NSToolbarItem(itemIdentifier: itemIdentifier)
            item.label = "Installed"
            item.target = self
            item.action = #selector(showInstalled(_:))
            item.image = NSImage(systemSymbolName: "square.stack.3d.up", accessibilityDescription: "Installed")
            return item
        default:
            return nil
        }
    }

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.genjuxInstalled, .flexibleSpace, .genjuxSearch]
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.genjuxInstalled, .genjuxSearch, .flexibleSpace]
    }
}

private extension NSToolbarItem.Identifier {
    static let genjuxSearch = NSToolbarItem.Identifier("com.petrguan.GenjuxStore.search")
    static let genjuxInstalled = NSToolbarItem.Identifier("com.petrguan.GenjuxStore.installed")
}
