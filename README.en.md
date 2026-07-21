# SkillYard

[中文](./README.md) | English

SkillYard is a local macOS application for discovering, installing, taking over, mounting, updating, and removing AI Agent Skills under one explicit management model.

## Version 1.0 shape

- One product entry point: `SkillYard.app`.
- Version 1.0 is built and used only on the product owner's personal Apple silicon Mac, with no public installer, paid Apple signing, or application updater.
- Tauri 2 with a TypeScript Web UI and a narrow Rust Lifecycle Core.
- Apple Silicon on macOS 14 or later.
- Initial Supported Apps: Codex, Claude Code, and GitHub Copilot.
- A persistent Central Store, SQLite state, and symlink-only Mounts.
- User-confirmed scanning, Takeover, installation, whole-Bundle Update, and deletion.
- No public CLI, localhost service, version history, rollback UI, bundled LLM, telemetry, or crash upload.

## Current status

The 1.0 PRD and ten-stage implementation plan are approved, and 20 vertical implementation issues have been published. No 1.0 Tauri application is available yet; implementation starts with [#19](https://github.com/ReyYang/SkillYard/issues/19).

The former Python CLI, local HTTP server, HTML view, and tests have been removed from the current workspace. Applicable behavioral constraints are preserved in the PRD and implementation plan, while Git retains the historical prototype.

The canonical design documents are currently maintained in Chinese:

- [Documentation index](./docs/README.md)
- [1.0 product contract](./docs/1.0-product-contract.md)
- [1.0 PRD](./docs/prd/0001-skillyard-1.0.md)
- [1.0 implementation plan](./docs/plans/0001-skillyard-1.0-implementation.md)
- [1.0 management authority](./docs/1.0-management-authority.md)
- [Domain glossary](./CONTEXT.md)
