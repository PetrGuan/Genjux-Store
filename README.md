# Genjux-Store

**Genjux-Store** is a cross-platform (macOS / Windows / Linux / Android) open-source software discovery & installation client. It aims to solve a common pain point: most projects hosted on open-source hubs like GitHub or Gitee lack a friendly, one-click install experience.

## Core idea

- Automatically surface recommended software for the platform you're currently running on, with one-click install/try.
- Never repackage or host third-party binaries — Genjux-Store only proxies downloads of the official GitHub Release artifacts and installs them locally. This minimizes legal/security liability and requires zero effort from upstream maintainers.
- The core business logic (discovery, release metadata parsing, download, verification, install orchestration) runs as a single cross-platform local service shared by three kinds of clients: native GUIs, a CLI, and AI agents (via MCP).

## Status

Early architecture-planning stage; implementation has not started yet. See [`.copilot-workflow/PLAN.md`](.copilot-workflow/PLAN.md) for the full architecture discussion and decisions made so far (layering, tech stack choices, security/trust model, monetization direction, roadmap, etc).

## License

MIT License — see [LICENSE](LICENSE).
