import Cocoa

// Explicit process entry point (instead of `@main` on `AppDelegate`) —
// sets an activation policy and starts the standard AppKit run loop
// directly, which is more predictable than relying on `@main`'s
// synthesized behavior for `NSApplicationDelegate` conformers.
let app = NSApplication.shared
app.setActivationPolicy(.regular)

let delegate = AppDelegate()
app.delegate = delegate

app.run()
