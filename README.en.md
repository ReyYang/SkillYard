# SkillYard

> How many AI Agent Skills are on your Mac right now? Who manages them? Is it safe to delete one?
>
> SkillYard answers all three questions. It organizes Skills scattered across Codex, Claude Code, and GitHub Copilot into **Bundles**—so you can see what you have, take control, and track every file's origin and destination.

[中文](./README.md) | English

---

## Download and requirements

[Download SkillYard 1.0.1](https://github.com/ReyYang/SkillYard/releases/tag/v1.0.1)

SkillYard 1.0 supports:

- macOS 14 Sonoma or later;
- Apple Silicon (`arm64`) Macs;
- the local Skill directories used by Codex, Claude Code, and GitHub Copilot.

Download `SkillYard-1.0.1-macos-aarch64.zip` from the Release. Unzip the archive, move `SkillYard.app` to `/Applications`, and launch it. The current build uses ad-hoc signing and is not notarized by Apple, so macOS may show a security prompt the first time it opens. Control-click the app in Finder and choose **Open**, or confirm the app under **System Settings → Privacy & Security**. Download official builds only from this repository's [GitHub Releases](https://github.com/ReyYang/SkillYard/releases).

## Preview

![SkillYard Bundle inventory populated with anonymous test data](./docs/assets/skillyard-overview.png)

*The screenshot contains anonymous test data only—no real accounts, paths, or Sources.*

## Why SkillYard

AI coding tools each maintain their own Skill directories. Skills arrive through different channels—official plugins, `npx skills`, `gh skill`, GitHub repositories, shared ZIP files—and before long, your local directories fill up with files you're not sure you can delete and whose maintenance status is unclear.

SkillYard does one thing well: **it discovers everything already on your machine**, so you can see what's where, where it came from, and who currently manages it. Then you decide—which Skills to hand over to SkillYard's unified management, and which to leave as they are.

| If you care about | SkillYard's approach |
|---|---|
| Where a Skill came from and what it belongs with | Managed as a **Bundle** (local install group) with Source provenance preserved |
| Whether a failed update corrupts your Skills | Atomic `current` symlink replacement—nothing takes effect until everything validates; crash recovery on next launch |
| Whether the tool silently modifies or deletes files | Every action—takeover, install, mount, update, delete—starts with a visible plan; destructive operations require double confirmation |
| Whether it messes with official plugin content | Official plugins, Host-bundled Skills, and project-maintained Skills are **read-only** and never modified |
| Whether installing a Skill exposes it to every Agent | Installed ≠ mounted. You can make a Skill visible only to Codex, or keep it unmounted entirely |
| Whether project-specific Skills get mixed up | **Global and project-level mounts**—project Skills only appear in their corresponding projects |

## Local-first, and you can verify it

SkillYard has no account system, no cloud database, no public registry. The Central Store, SQLite state, Mounts, and transaction records all stay on your machine.

- **No external command execution.** No `npx skills`, `gh skill`, Lark CLI, or any shell command. Scripts, binaries, and lifecycle hooks inside Skills are never executed.
- **Zero telemetry.** No usage analytics, no crash report uploads. Network requests only happen when you explicitly load a Source, search `skills.sh`, or check for updates.
- **The frontend can't touch your filesystem.** SkillYard is built with Tauri 2: the TypeScript UI handles display only; all persistent state and filesystem operations run through typed commands in the Rust Lifecycle Core. The frontend has no direct access to files, SQLite, or the shell.

## Three core workflows

### 1. Scan and take over

On first use, you click "Start Scan." SkillYard performs a read-only discovery of existing Skills in Codex, Claude Code, and GitHub Copilot, grouping them by source. **The scan moves nothing.**

After you approve the takeover plan, SkillYard migrates content into the Central Store (`~/Library/Application Support/SkillYard/`) and replaces the original locations with symlinks pointing to the managed copies. If content is already in a suitable location, it's registered and links are corrected without unnecessary copying.

### 2. Install and mount

Install a Bundle from a GitHub repository, `skills.sh` search result, ZIP / `.skill` archive, direct URL, or local directory—always as a complete Bundle, never as loose items.

After installation, the Bundle stays "installed but unmounted" by default. You choose whether to mount it to the global directory of Codex, Claude Code, or GitHub Copilot, or to a specific project directory. The Central Store is not scanned by any Agent—visibility is entirely under your control.

### 3. Update or remove

For Bundles with an upstream Source, you manually trigger "Check for Updates." When changes are found, you confirm the update—the entire Bundle is atomically replaced in one operation: after all candidate content passes validation, a single `current` symlink is swapped, and all Mounts follow automatically.

Removal has three levels: remove mounts (Skill returns to "unmounted" without data loss), remove Source (only severs the remote update relationship, local content untouched), delete Bundle (double-confirmation cascade removal of all members, mounts, and managed content—upstream remains unaffected).

## Architecture: one symlink decides everything

Each Bundle's current managed content takes effect through a single `current` symlink:

```
Bundle Directory
├── current → content-v2   # Atomically replace this link to complete an update
├── content-v1             # Cleaned up after validation
└── content-v2
    ├── example-skill/
    └── another-skill/
```

All Mounts (the Skill entries you see in Codex / Claude Code / Copilot) point to stable member paths under `current`. An update only swaps `current`'s target—no individual Mount rewriting needed.

Before any filesystem-modifying operation writes a single byte, a **Filesystem Transaction Journal** is established—recording the operation plan, affected paths, and completed steps—with every step designed for safe re-execution. If the app exits unexpectedly or the system crashes, the next launch prioritizes recovering unfinished transactions.

## Download and installation

[Download SkillYard 1.0.0](https://github.com/ReyYang/SkillYard/releases/tag/v1.0.0)

**System requirements:** macOS 14 Sonoma or later, Apple Silicon (`arm64`) Mac.

Download `SkillYard-1.0.0-macos-aarch64.zip`, unzip it, and move `SkillYard.app` to `/Applications` before launching. The current build uses ad-hoc signing and is not notarized by Apple, so macOS may show a security prompt on first launch. Control-click the app in Finder and choose **Open**, or confirm under **System Settings → Privacy & Security**.

> Only download official builds from [GitHub Releases](https://github.com/ReyYang/SkillYard/releases).

## Build from source

Requires an Apple Silicon Mac (macOS 14+), Xcode Command Line Tools, stable Rust, Node.js 20.19+ or 22.12+, and Corepack. The repository pins `pnpm@10.33.2`.

```bash
xcode-select --install
corepack enable
pnpm install --frozen-lockfile
pnpm typecheck && pnpm test
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm tauri build --bundles app
```

The application is generated at `target/release/bundle/macos/SkillYard.app`. Local builds keep the ad-hoc signing configuration and are not notarized by Apple.

## 1.0 scope

This is an intentional 1.0—not "feature-complete" in the absolute sense, but "core lifecycle is complete and boundaries are clear."

1.0 **includes:** Bundle management, first scan and takeover, installation and mounting (GitHub / skills.sh / ZIP / local), global and project-level mounts, Source association and update checking, atomic full-Bundle updates, Source deletion / Bundle deletion, Filesystem Transaction Journal with crash recovery.

1.0 **does not include:** Intel Macs, macOS 13 or earlier, Windows / Linux; Apple Developer ID signing / notarization / Mac App Store; Homebrew Cask, DMG packaging, automatic app updates, standalone CLI; private GitHub repository authentication, background update checks, public Skill registry; Skill version history, user-facing rollback, backup and restore, cross-device sync.

For the full product boundary, see the [1.0 Product Contract](./docs/1.0-product-contract.md).

## Documentation, contributing, and feedback

- [Product Contract](./docs/1.0-product-contract.md) and [Management Authority Design](./docs/1.0-management-authority.md)
- [Changelog](./CHANGELOG.md)
- [Contributing Guide](./CONTRIBUTING.md) · [Code of Conduct](./CODE_OF_CONDUCT.md) · [Support](./SUPPORT.md)
- [Security Policy & Private Vulnerability Reporting](./SECURITY.md)
- [Bug Reports & Feature Requests](https://github.com/ReyYang/SkillYard/issues)
- [MIT License](./LICENSE)
