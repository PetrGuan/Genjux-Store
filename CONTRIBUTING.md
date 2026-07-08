# Contributing to Genjux-Store

Thanks for your interest in Genjux-Store! This document covers the basics of how the project is organized and how to contribute.

## Project status

Genjux-Store is in early development (Phase 0 — see [ROADMAP.md](ROADMAP.md)). The architecture decisions behind the current design are recorded in [`.copilot-workflow/PLAN.md`](.copilot-workflow/PLAN.md); it's worth skimming before proposing architectural changes.

## Issue / PR workflow

All work is tracked as GitHub Issues, grouped into phase [Milestones](https://github.com/PetrGuan/Genjux-Store/milestones) with an `[Epic]` tracking issue per phase (a checklist of that phase's sub-issues).

- **Every PR must reference the issue it addresses**, using a closing keyword in the PR description, e.g. `Closes #12` or `Fixes #12`. Merging the PR then automatically closes the issue — this is how we keep the roadmap checklists in the Epic issues up to date without manual bookkeeping.
- If a PR only partially addresses an issue, reference it without a closing keyword (e.g. `Relates to #12`) and leave the issue open.
- Prefer smaller, single-purpose PRs that map to a single issue over large multi-issue PRs.
- If you want to work on an issue, leave a comment on it first to avoid duplicate work.

## No GitHub Actions / CI — verify locally before opening a PR

This project intentionally does **not** run GitHub Actions or any hosted CI. All verification is local, and it's the author's/reviewer's responsibility to run it before pushing or merging. Before opening (or merging) a PR, run:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

All four must pass on your machine. There is no automated safety net catching regressions after the fact, so treat this as a hard requirement, not a suggestion.

**Cross-platform code**: the core crate has platform-specific adapters (`core/src/platform/{macos,windows,linux}.rs`) and OS-specific behavior in `lifecycle.rs`. Without CI, code for a platform you're not running locally can only be compile-checked, not test-verified — call this out explicitly in your PR description (e.g. "Windows-specific changes, only compile-checked on macOS, needs a Windows run before merging") so reviewers know what's actually been exercised.

**macOS app** (`macos/`, Phase 1): regenerate and build before opening a PR that touches it —

```bash
cd macos
xcodegen generate
xcodebuild -project GenjuxStore.xcodeproj -scheme GenjuxStore -configuration Debug build
```

See [`macos/README.md`](macos/README.md) for more.

## Real-repo validation harness

`core/tests/e2e_real_repos.rs` runs the classification pipeline against a curated set of real public GitHub repos (clean per-platform naming, ambiguous names needing tier-2 keyword matching, missing/combined checksums, and the `genjux.yaml` override path). It hits the real GitHub API and CDN, so its tests are `#[ignore]`d and excluded from the default `cargo test --workspace`. Run it before cutting a Phase 0 milestone release:

```bash
cargo test -p genjux-core --test e2e_real_repos -- --ignored --test-threads=1
```

Any classification bug it finds against a curated repo should become its own tracked issue (see #51 for an example), not a special case bolted onto the harness or the classifier.

## Branch naming

Use `<type>/<short-description>`, e.g. `feat/release-classification`, `fix/download-resume`, `docs/roadmap`.

## Code of conduct

Be respectful and constructive. This is a small, early-stage open-source project — assume good faith.
