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

## Building and testing

Once the Rust workspace is bootstrapped (see issue #1), the standard commands will be:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

This section will be expanded as the workspace and per-platform projects are added.

## Branch naming

Use `<type>/<short-description>`, e.g. `feat/release-classification`, `fix/download-resume`, `docs/roadmap`.

## Code of conduct

Be respectful and constructive. This is a small, early-stage open-source project — assume good faith.
