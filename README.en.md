# SkillYard

Language: [中文](./README.md) | English

SkillYard is a Mac-first local Skill Library Manager for people who use multiple AI coding agents. It separates skill source management from host-specific exposure: you keep one traceable and updateable local Library, then expose selected Library Entries to Codex, Claude Code, Cursor, GitHub Copilot, and other Hosts.

## Problem

Multi-agent users often run into the same set of problems:

- Skills are scattered across `.codex/skills`, `.claude/skills`, project folders, and tool-specific directories.
- Many skills were installed through `npx`, `gh skill`, copied folders, or old scripts, making provenance hard to recover later.
- Once a skill is copied into several Host directories, upstream updates no longer propagate cleanly.
- Host Entry conflicts, broken symlinks, snapshot drift, and Dirty Source Trees are hard to inspect in one place.
- Project-level and user-level skills are easy to mix up.

SkillYard treats skills as Library Entries and Host-directory entries as Exposures. The default Exposure mode is symlink; snapshot is a visible fallback when symlink is not safe or supported.

## Core Concepts

- **Library**: the unified local skill source library.
- **Source Tree**: one Git repo, local personal directory, npm package materialization, or captured source.
- **Library Entry**: one manageable skill, usually represented by a `SKILL.md`.
- **Library Identity**: the stable management identity, formatted as `namespace/name`, such as `mattpocock/review`.
- **Exposure**: the relationship that makes a Library Entry available to a Host and scope.
- **Host Entry Name**: the real directory name inside the Host skill directory. It defaults to the skill's declared `name`.
- **Plan / Apply**: write operations produce a Plan first; only confirmed Plans are applied.

See [CONTEXT.md](./CONTEXT.md) for the full glossary.

## Current Status

This is the first runnable implementation of SkillYard. It includes:

- CLI: `init`, `import`, `expose`, `doctor`, `update`, `serve`
- SQLite State File
- Git / local / package / captured Source Trees
- symlink-first Exposures with snapshot fallback
- Host Adapters for Codex, Claude Code, Cursor, and GitHub Copilot
- Local Server + HTML View
- doctor health checks
- Update Impact preview
- Captured Install for external installers such as `npx`
- `gh skill` discovery
- Optional AI Assist for explaining skills and inferring unknown provenance

## Installation

Run from source:

```bash
git clone https://github.com/ReyYang/SkillYard.git
cd SkillYard
python3 -m skillyard --help
```

Install the `skillyard` command:

```bash
python3 -m pip install -e .
skillyard --help
```

The default State File location is:

```text
~/Library/Application Support/SkillYard/state.sqlite3
```

For development and testing, pass `--home` with a temporary directory so real local state is not touched.

## Quick Start

Initialize the local Library:

```bash
python3 -m skillyard init
python3 -m skillyard init --yes
```

Without `--yes`, SkillYard prints the Plan and does not Apply it. With `--yes`, it writes the State File and Library directories.

Import a Git repo:

```bash
python3 -m skillyard import https://github.com/example/agent-skills --namespace example
python3 -m skillyard import https://github.com/example/agent-skills --namespace example --yes
```

Import a local personal skill directory:

```bash
python3 -m skillyard import --local ~/GitHub/my-personal-skills --namespace personal --yes
```

Expose a skill to Codex:

```bash
python3 -m skillyard expose example/review --host codex --scope user
python3 -m skillyard expose example/review --host codex --scope user --yes
```

Expose a skill to a project scope:

```bash
python3 -m skillyard expose example/review \
  --host codex \
  --scope project \
  --project-root /path/to/project \
  --yes
```

Run health checks:

```bash
python3 -m skillyard doctor
```

Preview Source Tree update impact:

```bash
python3 -m skillyard update 1
python3 -m skillyard update 1 --apply --yes
```

Start the local HTML View:

```bash
python3 -m skillyard serve --port 8765
```

The Local Server is localhost-only.

## Captured Install

If you still want to use an existing installer such as `npx`, SkillYard can capture the Host directory changes around that command:

```bash
python3 -m skillyard import \
  --capture \
  --host codex \
  --host-dir /tmp/codex-skills \
  --yes \
  -- npx some-skill-installer
```

SkillYard will:

1. Snapshot the Host skill directory.
2. Run the real installer command.
3. Snapshot the Host skill directory again.
4. Detect added / changed / deleted Host Entries.
5. Record an Install Receipt.
6. Create a Package Source Tree when evidence is strong enough, or a Source Candidate when it is not.

## `gh skill` Discovery

Search for public skills:

```bash
python3 -m skillyard import --gh-search review
```

After getting a repo URL from discovery, convert it into a SkillYard import plan:

```bash
python3 -m skillyard import --import-url https://github.com/example/agent-skills --namespace example
```

`gh skill` is only a Discovery Provider. It does not manage local Library state.

## AI Assist

AI Assist is explicit and optional. It is not part of the core write path.

Explain a skill:

```bash
python3 -m skillyard doctor --explain-skill /path/to/skill
```

Infer unknown provenance:

```bash
python3 -m skillyard doctor --infer-provenance /path/to/skill
```

The result is a Provenance Inference. It is not automatically recorded as confirmed provenance.

## Development

Run tests:

```bash
python3 -m unittest discover
```

Run compile checks:

```bash
python3 -m compileall skillyard tests
```

## Docs

- [Domain glossary](./CONTEXT.md)
- [PRD](./docs/prd/0001-skillyard.md)
- [Architecture decisions](./docs/adr/)
