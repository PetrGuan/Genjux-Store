# Roadmap

Genjux-Store is developed in phases, tracked as [GitHub Milestones](https://github.com/PetrGuan/Genjux-Store/milestones), each with an `[Epic]` tracking issue containing a checklist of the concrete sub-issues for that phase. See [`.copilot-workflow/PLAN.md`](.copilot-workflow/PLAN.md) for the full architecture discussion behind these decisions.

| Phase | Milestone | Epic | Status |
|---|---|---|---|
| 0 | [Core Service & CLI](https://github.com/PetrGuan/Genjux-Store/milestone/1) | [#22](https://github.com/PetrGuan/Genjux-Store/issues/22) | In progress |
| 1 | [Flagship Desktop GUI](https://github.com/PetrGuan/Genjux-Store/milestone/2) (platform TBD) | [#23](https://github.com/PetrGuan/Genjux-Store/issues/23) | Blocked on Phase 0 |
| 2 | [Second Desktop Platform](https://github.com/PetrGuan/Genjux-Store/milestone/3) | [#24](https://github.com/PetrGuan/Genjux-Store/issues/24) | Not started |
| 3 | [Linux Desktop](https://github.com/PetrGuan/Genjux-Store/milestone/4) | [#25](https://github.com/PetrGuan/Genjux-Store/issues/25) | Not started |
| 4 | [Android](https://github.com/PetrGuan/Genjux-Store/milestone/5) | [#26](https://github.com/PetrGuan/Genjux-Store/issues/26) | Not started |

## Phase 0 — Core Service & CLI

A cross-platform Rust core service exposing a local HTTP/JSON API and an MCP server, plus a CLI client. No native GUI yet. Goal: prove the discovery → classification → download → verify → install pipeline end-to-end against real open-source GitHub repos.

Detailed sub-issues live in the [Phase 0 epic (#22)](https://github.com/PetrGuan/Genjux-Store/issues/22).

## Phase 1 — Flagship Desktop GUI

The first native desktop GUI (macOS/AppKit or Windows/WinUI3 — decision deferred until Phase 0 validates the core), talking to the core service over the local HTTP API. Distributed directly (notarized download / signed installer), not through an app store — see PLAN.md's distribution-channel decision.

## Phase 2 — Second Desktop Platform

The other of macOS/Windows, reusing the same core service.

## Phase 3 — Linux Desktop

GTK4 + libadwaita, expected to be the cheapest platform to add since the UI can live in the same Rust codebase as core (no FFI boundary needed).

## Phase 4 — Android

Kotlin + Jetpack Compose, with the core embedded in-process via UniFFI bindings (the one platform without a local-daemon pattern). Install trust model differs (sideload via `PackageInstaller` intent, no silent install).

## Contributing

Every change should reference the issue it addresses — see [CONTRIBUTING.md](CONTRIBUTING.md) for the PR/issue linking convention.
