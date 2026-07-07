---
status: ready-for-agent
title: SkillYard
---

# SkillYard PRD

## Problem Statement

多 agent 用户会同时使用 Codex、Claude Code、Cursor、GitHub Copilot 等 Host，但每个 Host 的 skill 目录、安装方式、命名规则和更新方式都不同。用户很难回答几个基本问题：这个 skill 从哪里来、是不是最新、暴露给了哪些 Host、一次更新会影响哪些 Host、当前目录里有没有 broken symlink、名称冲突或过时副本。

更复杂的是，很多 skill 并不是从清楚的 Git repo 导入，而是通过 `npx`、`gh skill`、本地复制、Codex App 或历史脚本直接写入 Host 目录。安装完成后，用户经常只剩一个目录，不知道包名、版本、上游仓库、是否项目级、是否还能更新。项目级 skill 和用户级 skill 也容易混在一起，导致用户不知道某个 Host Entry 应该被全局管理，还是只服务于某个项目。

现有工具更偏下载器或单 Host 安装器。它们可以把 skill 放到某个目录里，但不能把本机统一的 Library、Source Tree、Library Entry、Exposure、Update Impact 和 doctor 检查组织成一个清楚、可追溯、可预览再写入的本机系统。

## Solution

构建 SkillYard：一个 Mac-first 的 Skill Library Manager，产品形态是 CLI + Local Server + HTML View。CLI 负责初始化、导入、暴露、检查、更新预览和启动本地界面；Local Server 通过浏览器提供 Library 浏览、doctor findings、Update Impact 和冲突处理 UI。所有写操作都必须经过 Plan、中文确认、Apply，避免 UI 或脚本直接修改 Host 目录。

产品默认使用 symlink Exposure：一个 Source Tree 更新后，多个 Host Entry 可以同步看到最新 skill。对不适合 symlink 的环境支持 snapshot fallback，但 snapshot 必须在 doctor/status 中清楚标记，因为它可能和 Library 漂移。

Library 使用 SQLite State File 记录 Source Trees、Library Entries、Exposures、Host Adapters、Events 和用户选择。`gh skill` 只作为 Discovery Provider，用于搜索和导入公共 skill，不接管本地 Library 状态。

项目级 skill 通过 `Exposure Scope = project` 建模，并记录 `projectRoot`。它不是另一种 Library Entry。doctor 发现 Host 目录里已有但不受 SkillYard 管理的条目时，将其报告为 Unmanaged Host Entry，并给出保留未管理、导入 Library、或替换为 managed Exposure 的选择。

对 `npx` 等外部 installer，SkillYard 提供 Captured Install：保持用户原来的下载方式，由 SkillYard 运行真实命令，执行前后快照 Host skill 目录，记录 Install Receipt，并把新增或变更的 Host Entry 转为 Library 状态。npm 场景下，Install Receipt 记录 package name、version、repository URL、tarball URL、integrity 等证据；能匹配 Git repo 时升级为 Git Source Tree，不能可靠匹配时创建 Package Source Tree 或 Source Candidate。

AI Assist 是显式、可选的辅助能力，只用于解释 skill、总结发现、帮助推断未知来源。AI Assist 不参与核心写入流程，不直接修改 State File、Source Tree、Exposure 或 Host 目录。它给出的来源判断是 Provenance Inference，必须带证据和置信度，用户确认或修正后才会变成可管理的 provenance。

## User Stories

1. As a multi-agent user, I want one Library for all my skills, so that I can understand what skills I have without checking every Host directory.
2. As a Codex-first user, I want Codex to be the default Host, so that the main path matches my primary workflow.
3. As a Claude Code user, I want the same Library Entry exposed to Claude Code, so that I do not maintain a separate copy.
4. As a Cursor user, I want selected Library Entries exposed to Cursor, so that my editor agent can use the same skills.
5. As a GitHub Copilot user, I want supported skills exposed through a Host Adapter, so that common agent hosts share one management model.
6. As a user importing from GitHub, I want one Source Tree per repository, so that multiple skills from the same repo do not create duplicate clones.
7. As a user importing a repo with many skills, I want each `SKILL.md` discovered as a Library Entry, so that I can choose which entries to expose.
8. As a user importing a single skill path, I want the tool to preserve the source repo and skill path, so that update checks can still work.
9. As a user with personal skills, I want to add a local personal Source Tree, so that my private skills live beside GitHub skills in the Library.
10. As a user with bundle-style skills, I want bundle sources represented as Source Trees, so that grouped skills such as `lark/*` are managed together.
11. As a user, I want each skill to have a Library Identity like `namespace/name`, so that management identity is stable and collision-resistant.
12. As a user, I want Namespace inferred automatically, so that ordinary imports do not require naming decisions.
13. As a user, I want to override Namespace when needed, so that product identity like `lark/lark-doc` does not have to mirror upstream owner names.
14. As a user, I want Skill Name read from `SKILL.md`, so that the tool respects the skill's own short name.
15. As a user, I want Display Label generated for UI readability, so that the HTML View can show names like `Matt Pocock: Review`.
16. As a user, I want Host Entry Name to default to Skill Name, so that Codex and other Hosts see familiar names like `review`.
17. As a user, I want Host Entry Name overrides hidden unless there is a conflict, so that I do not have to learn an Alias concept.
18. As a user exposing a skill, I want conflicts detected before writing, so that an existing Host Entry is not silently overwritten.
19. As a Chinese-speaking user, I want conflict choices shown in Simplified Chinese, so that I can safely choose the next action.
20. As a user facing a conflict, I want to skip the new Exposure, so that I can avoid changing the Host directory.
21. As a user facing a conflict, I want to use a recommended Host Entry Name, so that both skills can coexist.
22. As a user facing a conflict, I want to replace the existing Host Entry, so that I can intentionally switch which Library Entry owns that name.
23. As a user facing a conflict, I want to choose a custom Host Entry Name, so that I can match my own naming style.
24. As a user, I want symlink Exposure by default, so that one Source Tree update updates multiple Host entries.
25. As a user, I want snapshot fallback when symlink is unsafe, so that unsupported environments still work.
26. As a user, I want snapshot Exposure clearly marked, so that I know it may drift from the Library.
27. As a user, I want `doctor` to detect broken symlinks, so that Host directories do not contain dead entries.
28. As a user, I want `doctor` to detect Dirty Source Trees, so that updates do not overwrite local work.
29. As a user, I want `doctor` to detect Host Entry conflicts, so that hidden conflicts are visible.
30. As a user, I want `doctor` to detect branch drift, so that I know when a Source Tree is not on its expected branch or tag.
31. As a user, I want `doctor` to detect missing Host support, so that I know why a Library Entry cannot be exposed.
32. As a user, I want `doctor` to detect snapshot drift, so that copied skills are not mistaken for live symlinks.
33. As a user, I want update to fetch before changing the working tree, so that I can preview what changed.
34. As a user, I want Update Impact before applying an update, so that I know which Library Entries and Exposures are affected.
35. As a user, I want Dirty Source Trees to block update by default, so that local edits are not hidden or overwritten.
36. As a user with a Dirty Source Tree, I want Chinese choices to view changes, back up then continue, or skip, so that I can recover confidently.
37. As a user, I do not want automatic stash, so that there is no hidden Git state I have to rediscover later.
38. As a user, I want minimal Events recorded, so that I can understand what changed on my machine.
39. As a user, I want Events to record import, expose, update, remove exposure, and conflict resolution, so that important changes are traceable.
40. As a user, I want `init` to create the State File and Library directories, so that the tool starts from a known local structure.
41. As a user, I want `import` to add Source Trees and Library Entries, so that new skills enter the Library before exposure.
42. As a user, I want `expose` to create Host entries from Library Entries, so that each Host only sees the skills I choose.
43. As a user, I want `doctor` to be read-only by default, so that checking health does not mutate my machine.
44. As a user, I want `update` to preview before apply, so that source changes are deliberate.
45. As a user, I want `serve` to open the Local Server, so that I can inspect and manage the Library visually.
46. As a user, I want the HTML View to browse Source Trees, so that repository-level organization is visible.
47. As a user, I want the HTML View to browse Library Entries, so that I can inspect skill identity, source, and exposure state.
48. As a user, I want the HTML View to browse Exposures by Host, so that I can see what Codex, Claude Code, Cursor, and GitHub Copilot currently receive.
49. As a user, I want the HTML View to show doctor findings, so that I can fix local problems without reading raw JSON.
50. As a user, I want the HTML View to show Update Impact, so that a repository update's blast radius is visually clear.
51. As a user, I want Local Server writes to show a Plan first, so that I can review filesystem and State File changes.
52. As a user, I want Local Server write confirmations in Simplified Chinese, so that risky actions are understandable.
53. As a user, I want CLI and Local Server to share one application layer, so that behavior is consistent across terminal and browser.
54. As a user, I want `gh skill` search available, so that I can discover public skills without leaving the tool.
55. As a user, I want `gh skill` to remain only a Discovery Provider, so that my local Source Tree and symlink model remain intact.
56. As a user, I want JSON export for debugging, so that I can inspect state without making JSON the source of truth.
57. As a user, I want the tool to avoid telemetry by default, so that my local skill setup stays private.
58. As a user, I want clear Chinese prompts around destructive operations, so that I do not accidentally delete or replace skills.
59. As a user, I want first-version scope to stay small, so that core Library and Exposure behavior is reliable before extra features arrive.
60. As a maintainer, I want Host Adapters to isolate host path logic, so that new Hosts can be added without changing Library semantics.
61. As a user, I want AI Assist to explain a skill's purpose and source clues, so that unknown skills are easier to evaluate.
62. As a user, I want AI Assist to stay optional, so that core skill management works without model calls.
63. As a user, I want Provenance Inference to include evidence and confidence, so that I can judge whether a guessed source is trustworthy.
64. As a user, I want inferred provenance to require my confirmation, so that guesses do not become false source truth.
65. As a user, I want project-level skills represented as project-scope Exposures, so that project skills use the same Library model as global skills.
66. As a user, I want project-scope Exposures to record projectRoot, so that the tool knows which project receives the Host Entry.
67. As a user, I want the same Library Entry exposed to user scope and project scope, so that I can reuse a skill without copying it.
68. As a user, I want doctor to report Unmanaged Host Entries, so that existing Host directory skills are visible even before import.
69. As a user, I want choices for Unmanaged Host Entries, so that I can keep them unmanaged, import them, or replace them with managed Exposures.
70. As a user who uses npx installers, I want SkillYard to capture my normal install flow, so that I do not have to learn a separate staging workflow.
71. As a user, I want Captured Install to snapshot Host directories before and after the external command, so that newly created or changed Host Entries can be identified.
72. As a user, I want an Install Receipt for captured installs, so that package identity, version, repository metadata, and created Host Entries remain traceable.
73. As a user, I want npm-based captured installs to create Package Source Trees when Git provenance is not reliable, so that package-installed skills can still be managed and checked.
74. As a user, I want Package Source Trees upgraded to Git Source Trees when repository evidence matches, so that source updates can use the better Git model.
75. As a user, I want unknown captured installs to remain Source Candidates or Provenance Inference, so that SkillYard does not invent namespaces or origins.

## Implementation Decisions

- The first product shape is CLI + Local Server + HTML View, not native Mac GUI and not static report.
- The CLI owns the first-version Command Surface: `init`, `import`, `expose`, `doctor`, `update`, and `serve`.
- The Local Server may execute writes, but every write must go through Plan, Simplified Chinese confirmation, and Apply.
- CLI commands and Local Server actions share one application layer. The HTML View must not mutate the State File, Source Trees, or Host directories directly.
- The State File is SQLite. JSON export is allowed for debugging and portability, but JSON is not the source of truth.
- The State File includes a minimal `events` table for user-visible changes. Events are traceability, not telemetry or a full audit system.
- The Library stores one Source Tree per upstream repository or managed source. A Source Tree can contain multiple Library Entries.
- A Library Entry has a Library Identity and points to a `SKILL.md` path inside a Source Tree.
- Library Identity is `namespace/name`. Namespace is inferred from source metadata by default and can be explicitly overridden.
- Skill Name is read from `SKILL.md` and is the default Host Entry Name.
- Display Label is UI-only and can be derived or customized without changing Library Identity or Host Entry Name.
- Alias is not a first-class product concept. The internal field is Host Entry Name Override, used only when the default Host Entry Name cannot be used.
- Host Entry conflicts are detected before write. The tool refuses to write and shows Simplified Chinese choices instead of silently renaming or overwriting.
- Exposure Mode is symlink-first. Snapshot is a fallback for unsupported Host or filesystem cases and must be visible in doctor/status.
- Built-in first-version Host Adapters are Codex, Claude Code, Cursor, and GitHub Copilot, with Codex as the default path.
- `gh skill` is integrated as a Discovery Provider for search/import, but it does not manage local Library state.
- AI Assist is explicit and optional. It can explain skills and help infer unknown provenance, but cannot mutate State File, Source Trees, Exposures, or Host directories.
- Provenance Inference stores evidence and confidence separately from confirmed provenance. A user must accept or correct an inference before it becomes source truth.
- Project-level skills are represented as project-scope Exposures with a recorded `projectRoot`, not as separate Library Entry types.
- Doctor reports Host directory entries that are not represented by SkillYard Exposures as Unmanaged Host Entries.
- Captured Install supports external installer workflows such as `npx` by snapshotting Host skill directories before and after the real installer command.
- Captured Install creates an Install Receipt containing command evidence, package/provider identity, resolved version, source metadata, and Host entries created or changed.
- npm-based Install Receipts can materialize a Package Source Tree from package registry metadata. When repository evidence reliably matches the skill source, the Package Source Tree can be upgraded to a Git Source Tree.
- If no reliable receipt or source evidence exists, the result remains a Source Candidate or Provenance Inference rather than a confirmed Library Identity.
- Source Tree update defaults to fetch plus Update Impact preview before changing the working tree.
- Dirty Source Trees block update by default. The tool shows Simplified Chinese choices and does not automatically stash.
- The Local Server is localhost-only. It is not a hosted service, registry database, or separate backend source of truth.

## Testing Decisions

- The highest-value test seam is the shared application layer that produces Plan objects and applies them. CLI and Local Server should both be tested through this seam instead of duplicating behavior tests per surface.
- Tests should validate external behavior: resulting State File rows, planned filesystem actions, created symlinks or snapshots, and user-visible conflict/update messages. They should not assert internal helper call order.
- State File tests should use temporary SQLite databases and verify Source Trees, Library Entries, Exposures, Host Entry Name overrides, and Events.
- Host Adapter tests should use temporary directories that simulate Codex, Claude Code, Cursor, and GitHub Copilot scopes.
- Exposure tests should verify symlink creation, snapshot fallback, existing-entry conflicts, replacement behavior, and custom Host Entry Name behavior.
- Conflict tests should verify that writes are refused before mutation and that Simplified Chinese choices include skip, recommended name, custom name, and replace.
- Update tests should use temporary Git repositories to verify fetch-before-update, Update Impact calculation, Dirty Source Tree blocking, branch/tag drift detection, and no automatic stash.
- Doctor tests should verify broken symlink detection, Dirty Source Tree detection, missing Host support, Host Entry conflict reporting, snapshot drift reporting, and branch drift reporting.
- Local Server tests should exercise HTTP endpoints through the same Plan/Apply application layer used by CLI commands.
- CLI tests should verify command outputs, exit codes, and write/no-write boundaries.
- Discovery Provider tests should mock or fixture `gh skill` output and verify that import creates Library state without handing state ownership to `gh skill`.
- AI Assist tests should verify that explanation and Provenance Inference produce suggestions without mutating State File, Source Trees, Exposures, or Host directories.
- Project-scope Exposure tests should use temporary project roots and verify that the same Library Entry can be exposed to both user and project scopes.
- Unmanaged Host Entry doctor tests should fixture existing Host directory entries and verify keep, import, and replace planning behavior.
- Captured Install tests should use a fake external installer command and temporary Host directories to verify before/after snapshot detection and Install Receipt creation.
- npm metadata tests should fixture package metadata and verify package name, version, repository URL, tarball URL, and integrity handling.
- Package Source Tree tests should verify both package-only materialization and upgrade to Git Source Tree when repository evidence reliably matches.
- Safety tests should verify that Apply is the only mutation layer and that preview/doctor commands remain read-only.

## Out of Scope

- Native Mac GUI.
- Hosted cloud service or public registry.
- Publishing skills.
- Full security scanner.
- Full audit log, telemetry, or analytics system.
- Supporting every possible Host in the first version.
- Automatic background updates.
- Automatic stash of Dirty Source Trees.
- Treating `gh skill` as the local state manager.
- Treating AI Assist as required infrastructure or an automatic decision-maker.
- Recording inferred provenance as confirmed provenance without user acceptance.
- Hidden shell history scraping to guess install origin.
- Making Alias a user-facing concept.
- Complex blocked-exposure workflow or background reconciliation queue.
- First-version remove, rename, export, security, or publish commands as standalone top-level commands.

## Further Notes

- This PRD assumes the project will be implemented from scratch or from a thin existing scaffold. Current repository state contains domain glossary and ADRs, not implementation modules.
- The intended first implementation seam is a small application core with explicit Plan and Apply APIs. CLI and Local Server should be thin adapters over that core.
- This PRD incorporates the later decisions that AI Assist is optional, project skills are project-scope Exposures, and external installers are handled through Captured Install plus Install Receipt.
- The issue tracker is not configured in this workspace, so this PRD is published as a local ready-for-agent document rather than an issue tracker item.
