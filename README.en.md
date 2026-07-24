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

All ten implementation stages have entered final acceptance. The repository now contains the complete Tauri 2 desktop application, Rust lifecycle core, SQLite state, persistent Central Store, and Mount management for all three Supported Apps. Automated regression, the local macOS application build and launch, Codex path behavior, and application replacement contracts have been verified.

All 167 User Stories are mapped to automated, Mac-contract, or human evidence. Version 1.0 is not marked complete yet: the product owner still needs to review the key copy and run one real Bundle through the final daily-use flow. See the [1.0 User Story acceptance evidence](./docs/acceptance/1.0-user-story-evidence.md) for the current execution status.

The former Python CLI, local HTTP server, HTML view, and tests have been removed from the current workspace. Applicable behavioral constraints are preserved in the PRD and implementation plan, while Git retains the historical prototype.

The canonical design documents are currently maintained in Chinese:

- [Documentation index](./docs/README.md)
- [1.0 product contract](./docs/1.0-product-contract.md)
- [1.0 PRD](./docs/prd/0001-skillyard-1.0.md)
- [1.0 implementation plan](./docs/plans/0001-skillyard-1.0-implementation.md)
- [1.0 management authority](./docs/1.0-management-authority.md)
- [1.0 User Story acceptance evidence](./docs/acceptance/1.0-user-story-evidence.md)
- [Domain glossary](./CONTEXT.md)
