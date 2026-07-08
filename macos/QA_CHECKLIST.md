# macOS GUI manual QA checklist (#67)

Automated coverage for Genjux-Store's macOS GUI is split across three layers:

1. **Rust core, real data** — [`core/tests/e2e_real_repos.rs`](../core/tests/e2e_real_repos.rs) (#21): classification pipeline correctness against curated real repos.
2. **Swift networking/decoding, real data** — [`scripts/e2e-validate.sh`](scripts/e2e-validate.sh) (#67): the exact Swift code every GUI screen uses (`CoreServiceClient`, `Models`, `ServiceLifecycle`), run as a standalone executable against the same curated repos, through a real lazily-started `genjuxd`. Not an XCTest target — see the comment atop [`scripts/e2e-validate.swift`](scripts/e2e-validate.swift) for why.
3. **Unit tests** — `GenjuxStoreTests` (`InstallStageTests`, plus `CoreServiceClientTests` where the local environment allows XCTest to spawn `genjuxd`).

None of the above drives the actual UI — clicking, scrolling, reading rendered text. That requires either a human at a real screen, or working screen-capture/accessibility automation (unavailable in the environment this project was originally built in — see the "Known environment issues" note in [`README.md`](README.md)). This checklist is the durable, repeatable list for whoever next has working interactive tooling (or is testing by hand) to run through once per release, or after any GUI-affecting change.

## Setup

```bash
cd macos
xcodegen generate
open GenjuxStore.xcodeproj   # Run the GenjuxStore scheme (Cmd+R)
```

A fresh checkout has no installed apps and an empty registry — useful for exercising every "empty state" below exactly once, then re-run after installing something for the "populated state" checks.

## Home screen (#60)

- [ ] On first launch, the Home screen shows a loading state, then a grid of recommended macOS apps (not an empty screen, not stuck loading forever — a cold cache take up to ~3 minutes on first run; subsequent launches are fast, cached).
- [ ] Each card shows: app name, owner/repo, star count, a short description (or a sensible fallback if the repo has none).
- [ ] Cards render in a responsive grid — resizing the window reflows the grid without clipping or overlapping cards.
- [ ] Clicking a card navigates to the App detail screen (#62) for that repo.
- [ ] The toolbar's search field and "Installed" button are both reachable from this screen.

## Search screen (#61)

- [ ] Typing `owner/repo` for a real repo with releases (e.g. `BurntSushi/ripgrep`) and submitting shows a table of classified installable assets (not raw asset names — human-readable platform/arch/kind).
- [ ] Typing `owner/repo` for a repo with **no releases** shows a clear "nothing installable here" state, not a raw error or a blank screen.
- [ ] Typing a **nonexistent** `owner/repo` shows a clear "not found" state, not a raw error or a crash.
- [ ] Selecting a row navigates to the Install progress screen (#63) or App detail screen, consistent with the Home screen's card-click behavior.
- [ ] The back/dismiss action returns cleanly to the Home screen (no navigation stack corruption, no duplicate toolbar items).

## App detail screen (#62)

- [ ] Shows the repo's real README excerpt, star count, and last-release date (not placeholder text) for a repo known to have all three.
- [ ] Shows a sensible fallback for a repo missing one of those fields (e.g. no description) rather than a blank area or a decoding crash.
- [ ] The Install button starts a real install and navigates to (or presents) the Install progress screen (#63).

## Install progress screen (#63)

- [ ] Installing a small, real macOS package end-to-end shows the stage sequence progressing (Resolving → Downloading → Verified → Installing → Succeeded), with a progress indicator during Downloading that reflects real byte progress (not stuck at 0% or jumping straight to 100%).
- [ ] The Verified stage discloses whether the checksum matched a **published** checksum or was self-computed only (per the trust-model requirement in `PLAN.md` section 5) — this should be visibly different in the UI, not silently treated the same.
- [ ] Installing a nonexistent repo reaches a clear Failed state with a legible reason, not a raw stack trace or a stuck spinner.
- [ ] After a real Succeeded install, the Installed screen (#64) reflects the new entry without requiring an app restart.

## Installed / updates screen (#64)

- [ ] On a fresh registry (nothing installed yet), shows a clear empty state, not a blank screen.
- [ ] After at least one real install, shows the installed app's name/repo and installed version/tag.
- [ ] An update check (whatever triggers `GET /updates` in the UI — a button, automatic on open, etc.) correctly distinguishes "up to date" vs. "update available" for at least one installed entry with a newer release upstream.

## Cross-cutting

- [ ] Quitting and relaunching the app reuses the already-running `genjuxd` instance rather than spawning a duplicate (only meaningful to check if `genjuxd` was still running from a prior launch — the CLI's `genjux status` or checking `ps` for a second `genjuxd` process is one way to confirm this from outside the GUI).
- [ ] No screen shows a raw/untranslated error type (e.g. a Swift `CoreServiceError` description) as the *only* feedback for a common failure case (404, offline, rate-limited) — each should read like a real product message.
