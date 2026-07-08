#!/usr/bin/env bash
#
# Genjux-Store macOS release build (#66).
#
# Builds the release `genjuxd` binary, archives the GenjuxStore.app with
# XcodeGen + xcodebuild, embeds genjuxd into the app bundle, code-signs it
# (real Developer ID signing if credentials are supplied via environment
# variables, otherwise a local ad-hoc signature so the pipeline is still
# fully exercisable without real Apple credentials), optionally notarizes
# and staples, and finally packages a distributable .dmg.
#
# This project has no CI (#49) — this script is meant to be run locally by
# whoever holds the signing credentials, exactly like every other build/test
# step in this repo.
#
# Real Developer ID signing + notarization requires:
#   GENJUX_DEVELOPMENT_TEAM      Apple Developer Team ID (e.g. "ABCDE12345")
#   GENJUX_CODE_SIGN_IDENTITY    Full signing identity name, as it appears in
#                                `security find-identity -v -p codesigning`
#                                (defaults to "Developer ID Application")
# and, optionally, for notarization:
#   GENJUX_NOTARIZE_PROFILE      A keychain profile name created ahead of time
#                                via:
#                                  xcrun notarytool store-credentials \
#                                    "<profile-name>" \
#                                    --apple-id "<apple-id>" \
#                                    --team-id "<team-id>" \
#                                    --password "<app-specific-password>"
#
# If GENJUX_DEVELOPMENT_TEAM is unset, the script falls back to local ad-hoc
# signing (`codesign --sign -`) and skips notarization/stapling entirely —
# useful for verifying the rest of the pipeline (build, genjuxd embedding,
# DMG packaging) without needing real credentials.
#
# Usage:
#   macos/scripts/build-release.sh
#   GENJUX_DEVELOPMENT_TEAM=ABCDE12345 GENJUX_NOTARIZE_PROFILE=genjux-notary \
#     macos/scripts/build-release.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MACOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$MACOS_DIR/.." && pwd)"
BUILD_DIR="$MACOS_DIR/build"
ARCHIVE_PATH="$BUILD_DIR/GenjuxStore.xcarchive"
APP_NAME="GenjuxStore.app"
DIST_APP_PATH="$BUILD_DIR/$APP_NAME"
DMG_PATH="$BUILD_DIR/GenjuxStore.dmg"

SIGN_IDENTITY="${GENJUX_CODE_SIGN_IDENTITY:-Developer ID Application}"
TEAM_ID="${GENJUX_DEVELOPMENT_TEAM:-}"
NOTARIZE_PROFILE="${GENJUX_NOTARIZE_PROFILE:-}"

log() { printf '\033[1;34m==>\033[0m %s\n' "$1"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$1" >&2; }

rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR"

# --- 1. Build a universal (arm64 + x86_64) release genjuxd binary -------
# GenjuxStore.app itself is a universal binary (Xcode builds both slices
# by default); genjuxd must match, or it would fail to launch entirely on
# whichever architecture wasn't built natively on this machine.
log "Building genjuxd (release, arm64)"
(cd "$REPO_ROOT" && cargo build --release --bin genjuxd --target aarch64-apple-darwin)
GENJUXD_ARM64="$REPO_ROOT/target/aarch64-apple-darwin/release/genjuxd"

log "Building genjuxd (release, x86_64)"
if ! rustup target list --installed 2>/dev/null | grep -q '^x86_64-apple-darwin$'; then
  log "Installing rustup target x86_64-apple-darwin"
  rustup target add x86_64-apple-darwin
fi
(cd "$REPO_ROOT" && cargo build --release --bin genjuxd --target x86_64-apple-darwin)
GENJUXD_X86_64="$REPO_ROOT/target/x86_64-apple-darwin/release/genjuxd"

for f in "$GENJUXD_ARM64" "$GENJUXD_X86_64"; do
  if [[ ! -x "$f" ]]; then
    echo "error: $f not found after cargo build" >&2
    exit 1
  fi
done

GENJUXD_BIN="$BUILD_DIR/genjuxd-universal"
log "Combining into a universal genjuxd binary (lipo)"
lipo -create -output "$GENJUXD_BIN" "$GENJUXD_ARM64" "$GENJUXD_X86_64"
lipo -info "$GENJUXD_BIN"

# --- 2. Regenerate the Xcode project + archive the app -------------------
log "Regenerating Xcode project (xcodegen)"
(cd "$MACOS_DIR" && xcodegen generate)

log "Archiving GenjuxStore (Release configuration)"
if [[ -n "$TEAM_ID" ]]; then
  # Real Developer ID signing: let xcodebuild sign during the archive step.
  xcodebuild \
    -project "$MACOS_DIR/GenjuxStore.xcodeproj" \
    -scheme GenjuxStore \
    -configuration Release \
    -archivePath "$ARCHIVE_PATH" \
    CODE_SIGN_STYLE=Manual \
    CODE_SIGN_IDENTITY="$SIGN_IDENTITY" \
    DEVELOPMENT_TEAM="$TEAM_ID" \
    OTHER_CODE_SIGN_FLAGS="--timestamp" \
    ENABLE_HARDENED_RUNTIME=YES \
    archive
else
  # No credentials: skip Xcode's own signing entirely and rely solely on
  # the ad-hoc `codesign --sign -` pass below (step 4) — this is the path
  # that lets the rest of the pipeline (genjuxd embedding, DMG packaging)
  # be verified locally without any Apple Developer account.
  xcodebuild \
    -project "$MACOS_DIR/GenjuxStore.xcodeproj" \
    -scheme GenjuxStore \
    -configuration Release \
    -archivePath "$ARCHIVE_PATH" \
    CODE_SIGNING_ALLOWED=NO \
    CODE_SIGNING_REQUIRED=NO \
    archive
fi

ARCHIVED_APP="$ARCHIVE_PATH/Products/Applications/$APP_NAME"
if [[ ! -d "$ARCHIVED_APP" ]]; then
  echo "error: archived app not found at $ARCHIVED_APP" >&2
  exit 1
fi
cp -R "$ARCHIVED_APP" "$DIST_APP_PATH"

# --- 3. Embed genjuxd into the app bundle --------------------------------
# genjuxd ships as a second executable inside the bundle (Contents/MacOS/),
# not linked into the app binary — ServiceLifecycle.swift's
# locateGenjuxd() already checks this exact path. Embedding after archiving
# (rather than via an Xcode Run Script build phase) sidesteps
# ENABLE_USER_SCRIPT_SANDBOXING restrictions on running `cargo build` from
# inside a sandboxed Xcode build phase.
log "Embedding genjuxd into the app bundle"
cp "$GENJUXD_BIN" "$DIST_APP_PATH/Contents/MacOS/genjuxd"
chmod +x "$DIST_APP_PATH/Contents/MacOS/genjuxd"

# --- 4. Code-sign genjuxd, then re-sign the whole app -------------------
# Adding a file after the app was archived (and initially signed by
# xcodebuild) invalidates its seal, so genjuxd must be signed first and the
# app must be re-signed --deep afterwards to produce a valid, notarizable
# bundle.
if [[ -n "$TEAM_ID" ]]; then
  log "Code-signing genjuxd with Developer ID (team $TEAM_ID)"
  codesign --force --options runtime --timestamp \
    --sign "$SIGN_IDENTITY" \
    "$DIST_APP_PATH/Contents/MacOS/genjuxd"

  log "Re-signing GenjuxStore.app (--deep)"
  codesign --force --deep --options runtime --timestamp \
    --sign "$SIGN_IDENTITY" \
    "$DIST_APP_PATH"

  log "Verifying signature"
  codesign --verify --deep --strict --verbose=2 "$DIST_APP_PATH"
  spctl --assess --type execute --verbose "$DIST_APP_PATH" || \
    warn "spctl rejected the app — this is expected until it has been notarized and stapled."
else
  warn "GENJUX_DEVELOPMENT_TEAM not set — falling back to local ad-hoc signing."
  warn "The resulting .app/.dmg will NOT pass Gatekeeper on another Mac."
  codesign --force --deep --sign - "$DIST_APP_PATH/Contents/MacOS/genjuxd"
  codesign --force --deep --sign - "$DIST_APP_PATH"
  codesign --verify --deep --strict "$DIST_APP_PATH"
fi

# --- 5. Notarize + staple (real credentials only) ------------------------
if [[ -n "$TEAM_ID" && -n "$NOTARIZE_PROFILE" ]]; then
  log "Zipping app for notarization submission"
  NOTARIZE_ZIP="$BUILD_DIR/GenjuxStore-notarize.zip"
  ditto -c -k --keepParent "$DIST_APP_PATH" "$NOTARIZE_ZIP"

  log "Submitting to notarytool (profile: $NOTARIZE_PROFILE) — this can take several minutes"
  xcrun notarytool submit "$NOTARIZE_ZIP" \
    --keychain-profile "$NOTARIZE_PROFILE" \
    --wait

  log "Stapling notarization ticket"
  xcrun stapler staple "$DIST_APP_PATH"
  xcrun stapler validate "$DIST_APP_PATH"
elif [[ -n "$TEAM_ID" ]]; then
  warn "GENJUX_NOTARIZE_PROFILE not set — skipping notarization/stapling."
  warn "Gatekeeper will still block this build on other Macs until notarized."
else
  warn "Skipping notarization/stapling (no real signing identity)."
fi

# --- 6. Package a distributable .dmg -------------------------------------
log "Creating $DMG_PATH"
DMG_STAGING="$BUILD_DIR/dmg-staging"
mkdir -p "$DMG_STAGING"
cp -R "$DIST_APP_PATH" "$DMG_STAGING/"
ln -s /Applications "$DMG_STAGING/Applications"
hdiutil create -volname "Genjux-Store" -srcfolder "$DMG_STAGING" -ov -format UDZO "$DMG_PATH"
rm -rf "$DMG_STAGING"

log "Done: $DIST_APP_PATH"
log "Done: $DMG_PATH"
if [[ -z "$TEAM_ID" ]]; then
  warn "This was an ad-hoc-signed local build for pipeline verification only."
  warn "For real distribution, re-run with GENJUX_DEVELOPMENT_TEAM (and GENJUX_NOTARIZE_PROFILE) set."
fi
