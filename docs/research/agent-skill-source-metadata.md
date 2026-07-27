# Agent Skill 来源元数据核验

核验日期：2026-07-27。

本文回答一个具体问题：Codex、Claude Code 和 GitHub Copilot 是否会为本地 Skill 保存类似 `~/.agents/.skill-lock.json` 的来源记录，以及 SkillYard 可以安全使用哪些证据为待接管 Skill 恢复 Bundle 名称和上游来源。

## 结论

| 对象 | 独立 Skill 是否有 Host 自己维护的上游来源记录 | 可用的插件级来源记录 | 是否写 `~/.agents/.skill-lock.json` |
| --- | --- | --- | --- |
| Codex | 没有发现 | 有，`codex plugin list --json` 可返回插件、市场、版本和来源 | Codex 本身不写 |
| Claude Code | 没有发现 | 有，`claude plugin list --json`、插件缓存和 marketplace 状态可恢复插件归属 | Claude Code 本身不写 |
| GitHub Copilot | 没有发现覆盖所有独立 Skill 的记录 | Copilot CLI 的插件状态包含插件名、marketplace、版本和 source | Copilot Host 本身不写 |
| `vercel-labs/skills`，即 `npx skills` 实际执行的 package | 有 | 不适用 | 写入，是该格式的原始实现 |
| GitHub CLI `gh skill` | 有，并同时向 `SKILL.md` 注入 GitHub metadata | 不适用 | 写入兼容格式 |

因此，`~/.agents/.skill-lock.json` **不能归因给 Codex、Claude Code 或 GitHub Copilot Host**。它可能由第三方 `vercel-labs/skills` CLI 写入，也可能由 GitHub 官方的独立安装器 `gh skill` 写入。文件存在只能证明“有兼容该协议的安装器记录过来源”，不能单独证明具体执行者。

三个 Host 的共同边界是：

- 普通 Skill 目录主要是文件系统发现机制。Host 知道 Skill 的名称、说明、本地路径和作用域，不等于知道它从哪个仓库下载。
- Host 自己安装的插件是另一类对象。Host 可以知道插件及其 marketplace/source，因而可以把插件内 Skill 归到该插件下；这类内容仍应由 Host 管理，SkillYard 只读展示。
- 只有外部安装器的 lock、`gh skill` 注入的 frontmatter、Git remote 等额外证据，才能为普通待接管 Skill 恢复上游来源。

## 1. `.skill-lock.json` 到底属于谁

### 1.1 `vercel-labs/skills` 定义并写入该文件

官方 `vercel-labs/skills` 源码定义：

- 全局 lock schema 当前为 v3；
- 设置 `XDG_STATE_HOME` 时，路径为 `$XDG_STATE_HOME/skills/.skill-lock.json`；
- 否则路径为 `~/.agents/.skill-lock.json`；
- 每个 Skill 记录 `source`、`sourceType`、`sourceUrl`、可选 `ref`、`skillPath`、目录 hash、安装与更新时间，以及可选 `pluginName`；
- `addSkillToLock()` 负责实际更新记录。

证据见固定快照 [`skill-lock.ts`](https://github.com/vercel-labs/skills/blob/e173b8c88f2581cfdaa1b6767c6519a08155790e/src/skill-lock.ts#L8-L72) 和[写入实现](https://github.com/vercel-labs/skills/blob/e173b8c88f2581cfdaa1b6767c6519a08155790e/src/skill-lock.ts#L204-L223)。

这里需要区分三个字段：

| 字段 | 含义 | SkillYard 用途 |
| --- | --- | --- |
| `sourceUrl` | 实际上游 URL | 规范化后作为同一 Source/Bundle 的首选身份 |
| `source` | 规范化的来源标识，例如 `mattpocock/skills` | `sourceUrl` 缺失时作为身份与展示回退 |
| `pluginName` | 安装器发现到的可选 plugin/group 名称 | 仅作为补充展示证据；SkillYard 1.0 不用它命名或拆分 Bundle |

`pluginName` 由发现结果写入 lock，源码也用它在安装界面分组，见固定快照 [`add.ts`](https://github.com/vercel-labs/skills/blob/e173b8c88f2581cfdaa1b6767c6519a08155790e/src/add.ts#L1840-L1848)。它不是必填字段，也不保证一个仓库只出现一个 `pluginName`，所以不能用它拆分 Bundle。

### 1.2 GitHub CLI `gh skill` 也写同一文件

GitHub CLI 官方源码明确写明其 lock version 要与 Vercel 的版本保持兼容，并使用同一个 `~/.agents/.skill-lock.json` 路径。它保存 `source`、`sourceType`、`sourceUrl`、`skillPath`、tree SHA、时间戳和 pinned ref，见 [`v2.92.0` lockfile 源码](https://github.com/cli/cli/blob/v2.92.0/internal/skills/lockfile/lockfile.go#L16-L48)及[`RecordInstall()`](https://github.com/cli/cli/blob/v2.92.0/internal/skills/lockfile/lockfile.go#L94-L136)。

安装成功后，`gh skill` 会调用 `lockfile.RecordInstall()`，见 [`v2.92.0` installer](https://github.com/cli/cli/blob/v2.92.0/internal/skills/installer/installer.go#L69-L82)。它还会向已安装 Skill 的 frontmatter 注入 GitHub repository、ref、tree SHA 和 path；官方命令手册对此有直接说明，见 [`gh skill install`](https://cli.github.com/manual/gh_skill_install)。

这带来两个限制：

1. 只看 `.skill-lock.json` 的路径和 v3 schema，无法区分记录来自 `npx skills` 还是 `gh skill`。
2. `gh skill` 是 GitHub CLI 的安装器，不是 GitHub Copilot Host runtime。即使目标参数选择了 Copilot、Codex 或 Claude Code，也不能把 lock 归因给该 Host。

### 1.3 本机只读证据

本机核验命令：

```bash
stat ~/.agents/.skill-lock.json
jq '.version, (.skills | length), .skills["ask-matt"]' ~/.agents/.skill-lock.json
```

截至核验日，本机文件为 v3，共 25 项；`ask-matt` 记录为：

```json
{
  "source": "mattpocock/skills",
  "sourceType": "github",
  "sourceUrl": "https://github.com/mattpocock/skills.git",
  "skillPath": "skills/engineering/ask-matt/SKILL.md",
  "pluginName": "mattpocock-skills"
}
```

这份证据足以得出：

- `ask-matt` 与其他相同 `sourceUrl` 的 Skill 属于同一个来源 Bundle；
- Bundle 应显示为 lock 保存的来源名称 `mattpocock/skills`；
- 不能仅凭该记录断言是 Codex、Claude Code、Copilot、`npx skills` 或 `gh skill` 中的哪一个执行了安装。

## 2. Codex

### 2.1 普通 Skill：有发现路径，没有上游来源收据

Codex 官方文档说明，它从 repository、user、admin 和 system 位置发现 Skill。repository 与 user 的通用目录分别包括 `.agents/skills` 和 `$HOME/.agents/skills`，并支持软链接目录，见 [Build skills](https://learn.chatgpt.com/docs/build-skills#where-codex-loads-local-skills)。

Codex 官方 `$skill-installer` 可以从 GitHub 获取内容，但当前实现最终只是把选中的目录 `copytree` 到 `$CODEX_HOME/skills/<skill-name>`，完成后打印目标路径。它没有写 lock、sidecar 或 per-Skill source registry，见固定快照[复制实现](https://github.com/openai/codex/blob/18f50c9e628af083a52d9240de09fc2db24d79ce/codex-rs/skills/src/assets/samples/skill-installer/scripts/install-skill-from-github.py#L164-L176)和[安装完成路径](https://github.com/openai/codex/blob/18f50c9e628af083a52d9240de09fc2db24d79ce/codex-rs/skills/src/assets/samples/skill-installer/scripts/install-skill-from-github.py#L275-L300)。

Codex runtime 的 `SkillMetadata` 保存名称、说明、本地 `SKILL.md` 路径、scope、`plugin_id` 和 `remote_plugin_id`，没有 repository URL 或 standalone Skill 的 source name，见固定快照 [`model.rs`](https://github.com/openai/codex/blob/18f50c9e628af083a52d9240de09fc2db24d79ce/codex-rs/skills/src/model.rs#L6-L19)。

结论：普通 Codex Skill 仅凭 Host 自身数据不能恢复上游来源。若目录旁没有外部 lock、Git metadata 或用户确认，应保持 Unknown Provenance。

### 2.2 Plugin Skill：可以恢复插件归属，但只读展示

Codex 插件由 Codex 自己安装和管理。官方插件文档说明插件可以包含多个 Skill，并由插件浏览器安装、启用和卸载，见 [Plugins](https://learn.chatgpt.com/docs/plugins)。

本机只读核验：

```bash
codex --version
codex plugin list --json
```

本机版本为 `codex-cli 0.145.0`。`codex plugin list --json` 的真实输出包含：

- `pluginId`、`name`、`marketplaceName`、`version`；
- `source`；
- 部分条目的 `marketplaceSource`；
- `installed` 与 `enabled`。

这份 Host API 足以把带 `plugin_id` 的 Skill 归到对应插件，并显示插件来源。但它只适用于 Codex 插件，不能为 `$skill-installer`、手动复制或 `npx skills` 产生的普通 Skill 补出来源。按照 SkillYard 现有边界，这些插件 Skill 应只读展示并跳转到 Codex 管理。

## 3. Claude Code

### 3.1 普通 Skill：目录就是配置，没有通用来源记录

Claude Code 官方支持：

- personal：`~/.claude/skills/<skill-name>/SKILL.md`；
- project：`.claude/skills/<skill-name>/SKILL.md`；
- plugin：`<plugin>/skills/<skill-name>/SKILL.md`。

官方创建流程也是直接创建目录和 `SKILL.md`，见 [Extend Claude with skills](https://code.claude.com/docs/en/skills#where-skills-live)。官方文档没有为 personal/project standalone Skill 定义类似 `.skill-lock.json` 的 source registry。

结论：Claude Code 可以发现一个 Skill 并观察其本地变化，但普通 Skill 的来源仍需要外部证据。

### 3.2 Plugin Skill：有 plugin/marketplace 级来源

Claude Code plugin manager 保存 marketplace、plugin source、版本和缓存。官方文档说明：

- marketplace entry 必须有 `name` 和 `source`；
- plugin 会复制到 `~/.claude/plugins/cache`；
- marketplace 状态保存在 `~/.claude/plugins/known_marketplaces.json`；
- `claude plugin list --json` 会列出版本、source marketplace 和 enable 状态。

证据见 [Plugin marketplaces](https://code.claude.com/docs/en/plugin-marketplaces#plugin-sources) 和 [Plugins reference](https://code.claude.com/docs/en/plugins-reference#plugin-list)。

本机只读核验：

```bash
claude --version
claude plugin list --json
jq '{version, plugin_count: (.plugins | length)}' ~/.claude/plugins/installed_plugins.json
```

本机版本为 `2.1.207`；当前 `claude plugin list --json` 返回空数组，`installed_plugins.json` 的 `plugins` 当前也为空。这证明了本机可用的检查入口，但不能用空状态推断其他机器没有插件。

结论：Claude Code 的 plugin metadata 可以为 plugin Skill 提供只读归属；它不能为 `~/.claude/skills` 或项目 `.claude/skills` 中的普通 Skill 提供通用来源。

## 4. GitHub Copilot

### 4.1 普通 Skill：发现和安装能力不等于完整 provenance

GitHub 官方文档列出 Copilot 支持的 Skill 位置：

- project：`.github/skills`、`.agents/skills`、`.claude/skills`；
- personal：`~/.copilot/skills`、`~/.agents/skills`；
- 还包括 plugin、built-in 和 organization/enterprise 来源。

证据见 [About agent skills](https://docs.github.com/en/copilot/concepts/agents/about-agent-skills) 和 [Copilot CLI command reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference#skills-reference)。

当前 Copilot CLI 还提供 `copilot plugins install --skill <FILE|URL|DIRECTORY>`。官方语义是：

- file 或 URL：复制到 personal/project Skill 目录；
- directory：注册为额外的自定义 Skill 目录。

见 [Installing a skill non-interactively](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference#installing-a-skill-non-interactively)。

但官方配置目录文档没有定义一个为所有独立 Skill 保存原始 URL、repository 和 source name 的 lock。它列出的持久化 Skill 设置是 `skills/` 内容、额外 `skillDirectories` 和 `disabledSkills`，而自动管理的 `config.json` 明确包含 authentication、installed plugin metadata 和其他内部状态，见 [Copilot CLI configuration directory](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)。

因此，不能假设一个出现在 `~/.copilot/skills` 或 `.github/skills` 的普通 Skill 一定能从 Copilot Host 状态恢复远端仓库。

### 4.2 Copilot CLI Plugin：有插件来源，但不是普通 Skill lock

Copilot CLI plugin 是 Host 自己管理的 Bundle。官方 plugin reference 定义了 marketplace、GitHub repository、Git URL 和本地路径等安装来源，并把安装内容放在 `~/.copilot/installed-plugins`，见 [GitHub Copilot CLI plugin reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference)。

本机只读核验：

```bash
copilot --version
copilot plugin list
```

本机安装的是 Homebrew Cask `copilot-cli 1.0.31`，`copilot plugin list` 显示已安装 `figma 2.1.3`。对 `~/.copilot/config.json` 只提取非敏感的 `installedPlugins` 字段后，记录包含：

```json
{
  "name": "figma",
  "source": {
    "source": "github",
    "repo": "figma/mcp-server-guide"
  },
  "version": "2.1.3",
  "enabled": true
}
```

这证明 Copilot CLI 的插件记录可以恢复 plugin source。但 `config.json` 还保存认证状态，SkillYard 不应读取或展示整个文件；如需支持，必须只解析 `installedPlugins` 的允许字段，或优先使用 Host 提供的只读清单命令。

这仍只覆盖 plugin Skill。当前本机 `config.json` 没有独立 Skill 来源条目，官方文档也没有承诺该文件是 standalone Skill provenance registry。

这份状态只代表 Copilot CLI，也不能被外推成 VS Code、JetBrains、GitHub Copilot App 或 cloud agent 的统一本地插件状态。

### 4.3 `gh skill` 不是 Copilot Host

GitHub 文档会推荐使用 `gh skill` 为 Copilot 安装 Skill，但执行者是独立的 GitHub CLI。它写入 `.skill-lock.json` 和 Skill frontmatter 的行为，应归入外部安装器证据，不能归入 Copilot Host metadata。

## 5. 对 SkillYard 的直接建议

### 5.1 Bundle 身份与名称

处理 `.skill-lock.json` 时采用以下最小规则：

1. 用规范化后的 `sourceUrl` 作为 Bundle identity；缺失时回退到 `source`。
2. 同一个 source 的所有 Skill 必须归入同一个 Bundle，不能因为安装目标 Host、安装命令或 `pluginName` 不同而拆分。
3. Bundle 显示名直接使用 lock 保存的 `source`；SkillYard 1.0 不再引入 `pluginName` 命名优先级。
4. `pluginName` 只保留为可核验的安装器附加信息，不改变 Bundle 边界或名称。
5. 用户确认接管后，能够规范化为 GitHub 仓库的 lock 来源应自动创建或复用 Source，并关联本地 Bundle；这项写入与接管领域状态使用同一次 SQLite 提交。

以本机 `mattpocock/skills` 为例：

```text
Bundle identity  = https://github.com/mattpocock/skills.git
Bundle name      = mattpocock/skills
Skill members    = ask-matt、code-review、grilling、……
```

### 5.2 来源恢复优先级

待接管扫描可以按以下证据强度恢复来源：

1. `.skill-lock.json` 中与本地 Skill name/path 匹配的 `sourceUrl`、`source`、`skillPath` 和 `pluginName`；
2. `gh skill` 注入的 GitHub frontmatter，用于交叉验证 repository、path 和 tree SHA；
3. 本地 Git checkout 的 remote 与 Skill 相对路径；
4. 用户确认；
5. 都不存在时保持 Unknown Provenance，不从 Host 名称或目录位置猜测来源。

`.skill-lock.json` 可能陈旧，记录也可能存在而本地文件已被改动或删除，所以它是来源证据，不是文件当前状态的唯一真相。扫描仍需验证本地目录、`SKILL.md` 和实际挂载。

### 5.3 Host-managed 内容

下面这些来源只用于只读展示，不进入接管计划：

- Codex plugin list 中带 `pluginId` 的 Skill；
- Claude Code plugin manager 中的 Skill；
- Copilot CLI installed plugin 中的 Skill；
- Host bundled、managed 或 remote organization Skill。

对应 UI 可以使用 Host/plugin 的名称进行分组并提供“在原应用中管理”的跳转。不要把这些插件状态合并到 `.skill-lock.json` Bundle，也不要因为它们包含 `SKILL.md` 就接管其主副本。

## 6. 证据范围与限制

- 官方网页文档会持续更新，本文描述截至 2026-07-27 可见的行为。
- 固定源码快照：
  - `vercel-labs/skills`：[`e173b8c88f2581cfdaa1b6767c6519a08155790e`](https://github.com/vercel-labs/skills/tree/e173b8c88f2581cfdaa1b6767c6519a08155790e)
  - `openai/codex`：[`18f50c9e628af083a52d9240de09fc2db24d79ce`](https://github.com/openai/codex/tree/18f50c9e628af083a52d9240de09fc2db24d79ce)
  - `cli/cli`：本机正式发布版 [`v2.92.0`](https://github.com/cli/cli/tree/v2.92.0)，并复核当前 HEAD [`592255318aa6a68944a534765bacbf4c52de5741`](https://github.com/cli/cli/tree/592255318aa6a68944a534765bacbf4c52de5741)
- 本机输出只证明核验日这台开发机的状态，不代表所有用户环境。产品实现应依能力检测和字段验证，不依赖本机当前恰好安装的插件数量。
