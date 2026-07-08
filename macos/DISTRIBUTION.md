# macOS Distribution: signing, notarization, packaging (#66)

Genjux-Store does **not** ship through the Mac App Store — direct download +
notarized distribution was a deliberate, discussed decision (see
[`../.copilot-workflow/PLAN.md`](../.copilot-workflow/PLAN.md)): the app
downloads and executes third-party installers, which App Review explicitly
disallows. This document is the local, no-CI (see [#49](https://github.com/PetrGuan/Genjux-Store/issues/49))
build → sign → notarize → staple → package pipeline for whoever holds the
Apple Developer credentials.

## One command

```bash
macos/scripts/build-release.sh
```

[`build-release.sh`](scripts/build-release.sh) does the whole pipeline:

1. Cross-compiles `genjuxd` for both `aarch64-apple-darwin` and
   `x86_64-apple-darwin`, and `lipo`s them into a single universal binary
   (the app itself is a universal binary too — a single-architecture
   `genjuxd` would silently fail to launch on the other architecture).
2. Regenerates the Xcode project (`xcodegen generate`) and archives
   `GenjuxStore` in the `Release` configuration.
3. Embeds the universal `genjuxd` into
   `GenjuxStore.app/Contents/MacOS/genjuxd` — `ServiceLifecycle.swift`'s
   `locateGenjuxd()` already looks there first.
4. Code-signs `genjuxd`, then re-signs the whole app `--deep` (embedding a
   file after the initial archive-time signature invalidates it, so both
   steps are required, in that order).
5. Notarizes and staples, if credentials are available.
6. Packages a `.dmg` (with an `/Applications` symlink, the standard
   drag-to-install layout) at `macos/build/GenjuxStore.dmg`.

Without any credentials, the script still runs end-to-end using a local
**ad-hoc signature** (`codesign --sign -`) — this is enough to verify the
whole pipeline (universal-binary genjuxd, embedding, packaging) but the
result will **not** pass Gatekeeper on another Mac. This has been verified
in this repo: a full ad-hoc run produces a working `GenjuxStore.app` whose
embedded `genjuxd` launches and serves `/health` correctly, and a valid,
`hdiutil verify`-passing `.dmg`.

## Prerequisites for a real, distributable build

1. **An Apple Developer Program membership** (individual or organization).
2. **A "Developer ID Application" certificate** installed in your login
   keychain (Xcode → Settings → Accounts → Manage Certificates → **+** →
   *Developer ID Application*, or via the
   [developer.apple.com certificate portal](https://developer.apple.com/account/resources/certificates/list)).
   Confirm it's visible to the command line:
   ```bash
   security find-identity -v -p codesigning
   ```
3. **A notarization keychain profile** (one-time setup), using an
   [app-specific password](https://support.apple.com/en-us/102654) (not
   your real Apple ID password):
   ```bash
   xcrun notarytool store-credentials "genjux-notary" \
     --apple-id "you@example.com" \
     --team-id "ABCDE12345" \
     --password "xxxx-xxxx-xxxx-xxxx"
   ```

## Running a real signed + notarized build

```bash
GENJUX_DEVELOPMENT_TEAM=ABCDE12345 \
GENJUX_NOTARIZE_PROFILE=genjux-notary \
macos/scripts/build-release.sh
```

Environment variables the script reads:

| Variable                     | Required for            | Default                          |
|-------------------------------|--------------------------|-----------------------------------|
| `GENJUX_DEVELOPMENT_TEAM`     | real signing             | *(unset → ad-hoc signing)*        |
| `GENJUX_CODE_SIGN_IDENTITY`   | real signing (optional)  | `Developer ID Application`        |
| `GENJUX_NOTARIZE_PROFILE`     | notarization (optional)  | *(unset → notarization skipped)*  |

When `GENJUX_DEVELOPMENT_TEAM` is set, the script:
- Signs with `--options runtime` (Hardened Runtime — a hard requirement for
  notarization) and `--timestamp` (a secure timestamp — also required).
- Runs `codesign --verify --deep --strict` and `spctl --assess` after
  signing (the `spctl` check is *expected* to still fail at this point,
  since notarization hasn't happened yet — see below).
- If `GENJUX_NOTARIZE_PROFILE` is also set: zips the app, submits it to
  `notarytool` and waits synchronously for a result (this step alone
  typically takes a few minutes), then staples the notarization ticket to
  the `.app` with `xcrun stapler staple` and validates the staple.

After a real signed + notarized run, `spctl --assess --type execute
GenjuxStore.app` should report `accepted`, and the app will launch on any
Mac without a Gatekeeper warning.

## Why no entitlements file

GenjuxStore does not run inside the App Sandbox (direct distribution has no
sandboxing requirement, unlike the Mac App Store) and needs no Hardened
Runtime exceptions: it makes ordinary outbound network calls (GitHub API,
localhost `genjuxd`) and spawns `genjuxd` as a plain, separately-signed
subprocess via `Process()` — none of which require
`com.apple.security.*` entitlement keys. Hardened Runtime itself is enabled
purely via the `--options runtime` codesign flag, no entitlements plist
needed.

## Known limitation: genjuxd auto-update

The bundled `genjuxd` is embedded once, at build time. There's no
auto-update mechanism yet (for either the app or the bundled core
service) — that's a separate, not-yet-filed concern for a later phase, once
there's a real release channel to update from.
