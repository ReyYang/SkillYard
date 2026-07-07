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

This repository contains the product definition, architectural decisions, and a first runnable Python implementation.

## Development

Run the CLI from the repository root:

```bash
python3 -m skillyard --home /tmp/skillyard init --yes
```

Run tests:

```bash
python3 -m unittest discover
```

The CLI uses Plan -> Apply for write operations. Omit `--yes` to inspect the Plan without applying it.
