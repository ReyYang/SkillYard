# SkillYard

SkillYard is a Mac-first local Skill Library Manager for people who use multiple AI coding agents.

It keeps a unified local Library of skill Source Trees, then exposes selected Library Entries to Hosts such as Codex, Claude Code, Cursor, and GitHub Copilot. The first product shape is CLI plus a localhost HTML View.

## Product Shape

- CLI commands: `init`, `import`, `expose`, `doctor`, `update`, `serve`
- Local Server for the HTML View
- SQLite State File
- Symlink-first Exposures with snapshot fallback
- One Source Tree per upstream repository or managed source
- Update Impact preview before Source Tree updates
- Simplified Chinese confirmation for conflict and write decisions

## Docs

- [Domain glossary](./CONTEXT.md)
- [PRD](./docs/prd/0001-skillyard.md)
- [Architecture decisions](./docs/adr/)

## Current Status

This repository currently contains the product definition and architectural decisions for the first version.
