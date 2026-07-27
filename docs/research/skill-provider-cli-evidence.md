# Skill CLI 与安装链证据

核验基线：2026-07-15。

本文只回答四个事实问题：相关 CLI 是否真实存在、它们实际提供什么能力、会留下什么本地证据，以及这些证据不能推出什么结论。本文不是 SkillYard 的产品状态定义，也不授权 SkillYard 执行任何外部安装命令。

## 核验结论

| 对象 | 已证明的事实 | 不能据此推出的结论 |
| --- | --- | --- |
| npm package `skills` / `npx skills` | 存在真实发布包，提供安装、列表、更新、移除，并实现 canonical store、Host 投影和 lock v3 | 不能推出所有 `npx` 命令或所有 Skill 都采用这套协议 |
| GitHub CLI `gh skill` | 是 GitHub CLI 的官方 Public Preview 命令，提供安装、列表、搜索、预览、更新和发布 | 不能把它视为 `npx skills` 的别名；核验快照中没有移除命令 |
| `@larksuite/cli` / `lark-cli` | 官方安装和更新流程会委托 `skills` CLI 同步 Lark Skills | 不能把 Lark CLI、`npx` 和 `skills` CLI 合并成同一个执行者 |
| npm / `npx` | 负责解析 npm package 并执行其 `bin` | npm 和 `npx` 本身不是通用 Skill 生命周期协议 |
| Homebrew | 负责 formula、cask 和第三方命令的分发 | 没有证据表明 Homebrew 内置通用 Agent Skill 协议 |

得到证明的是三种具体实现，不是“每个 Skill 发布者都有自己的 CLI”。没有相同一手证据的 Git clone、ZIP、本地目录、项目内容、Host 内置内容和其他安装器，必须分别判断，不能从这些例子类推。

## 证据范围

结论只使用以下一手来源：

- 官方 GitHub 仓库和固定源码快照；
- npm 官方 registry 与 npm CLI 文档；
- GitHub CLI 官方 manual 与 release；
- Homebrew 官方 manual。

固定源码快照：

| 项目 | 快照 |
| --- | --- |
| `vercel-labs/skills` | [`5527c09adc367612b0bffd9c80e3bc28a6b01b6d`](https://github.com/vercel-labs/skills/tree/5527c09adc367612b0bffd9c80e3bc28a6b01b6d) |
| `larksuite/cli` | [`8897196dee2882efe410ecd7183d40446ed9d3d7`](https://github.com/larksuite/cli/tree/8897196dee2882efe410ecd7183d40446ed9d3d7) |
| `cli/cli` | [`v2.96.0`](https://github.com/cli/cli/tree/b300f2ec7ec9dc9addc39b2ad88c54097ded7ca0) |

版本号和能力只描述这些核验快照。Preview 命令或 package 后续可能变化，不能把本文当作永久 API 保证。

## 1. `skills` / `npx skills`

### 1.1 真实发布包与命令

npm registry 中存在名为 `skills` 的 package，并将 `skills` 与 `add-skill` 映射到同一个 CLI 入口。可通过 [npm registry](https://registry.npmjs.org/skills/latest) 和官方 [`package.json`](https://github.com/vercel-labs/skills/blob/5527c09adc367612b0bffd9c80e3bc28a6b01b6d/package.json#L2-L9) 复核。

官方实现提供以下主要操作：

- `skills add <source>`；
- `skills list` / `skills ls`；
- `skills update [skills...]`；
- `skills remove [skills...]` / `skills rm`。

命令证据见官方 [README 的安装与列表说明](https://github.com/vercel-labs/skills/blob/5527c09adc367612b0bffd9c80e3bc28a6b01b6d/README.md#L106-L172)、[更新与移除说明](https://github.com/vercel-labs/skills/blob/5527c09adc367612b0bffd9c80e3bc28a6b01b6d/README.md#L184-L223) 和 [CLI 注册代码](https://github.com/vercel-labs/skills/blob/5527c09adc367612b0bffd9c80e3bc28a6b01b6d/src/cli.ts#L105-L168)。

[`npx` 官方文档](https://docs.npmjs.com/cli/v11/commands/npx/)说明，`npx` 从本地依赖或 registry 解析 package，再执行 package 的 `bin`。因此 `npx skills` 中理解 Skill、Agent 路径和安装收据的是 `skills` package，而不是 `npx` 本身。

### 1.2 canonical store 与 Host 投影

核验快照中的 canonical directory 为：

```text
global   ~/.agents/skills
project  <cwd>/.agents/skills
```

路径定义见 [`installer.ts`](https://github.com/vercel-labs/skills/blob/5527c09adc367612b0bffd9c80e3bc28a6b01b6d/src/installer.ts#L98-L101)。

默认流程先把内容写入 canonical directory，再从具体 Agent 目录建立投影。实现优先创建 symlink；失败时可以退回 copy；直接读取 canonical directory 的 Agent 不需要重复链接。证据见 [安装实现](https://github.com/vercel-labs/skills/blob/5527c09adc367612b0bffd9c80e3bc28a6b01b6d/src/installer.ts#L265-L412) 和 [官方模式说明](https://github.com/vercel-labs/skills/blob/5527c09adc367612b0bffd9c80e3bc28a6b01b6d/README.md#L97-L104)。

这是 `skills` package 的实现选择，不是 npm、`npx` 或整个 Skill 生态的统一目录标准。

### 1.3 lock v3 是来源证据，不是唯一执行者证明

`skills` 使用 `.skill-lock.json` 保存安装收据。全局默认位置为：

```text
$XDG_STATE_HOME/skills/.skill-lock.json
```

未设置 `XDG_STATE_HOME` 时回退到：

```text
~/.agents/.skill-lock.json
```

lock v3 可记录 Source 类型与 URL、ref、Skill 路径、目录 hash、安装时间和更新时间。格式和路径见 [`skill-lock.ts`](https://github.com/vercel-labs/skills/blob/5527c09adc367612b0bffd9c80e3bc28a6b01b6d/src/skill-lock.ts#L8-L73)。

它是有价值的 Management Evidence，但有明确限制：

- 写 lock 是 best-effort；缺少 lock 不能证明安装从未发生；
- `well-known` 来源不一定具有足够的路径与 hash 信息完成自动比较；
- lock 描述已知来源和安装状态，不保证目录未被其他工具或用户修改；
- GitHub CLI 也采用兼容的 lock v3，因此 lock 本身不能唯一证明写入者。

`well-known` 更新限制见 [`update.ts`](https://github.com/vercel-labs/skills/blob/5527c09adc367612b0bffd9c80e3bc28a6b01b6d/src/update.ts#L166-L220)。`remove` 还会根据其他 Agent 是否仍在使用 canonical copy，决定是否删除主副本并更新 lock，见 [`remove.ts`](https://github.com/vercel-labs/skills/blob/5527c09adc367612b0bffd9c80e3bc28a6b01b6d/src/remove.ts#L130-L247)。

## 2. GitHub CLI `gh skill`

### 2.1 官方命令真实存在

GitHub 在 [`gh` v2.90.0 release](https://github.com/cli/cli/releases/tag/v2.90.0) 中以 Public Preview 发布 `gh skill`。核验的 v2.96.0 源码注册了：

- `install`；
- `list`；
- `preview`；
- `publish`；
- `search`；
- `update`。

证据见 [`skills.go`](https://github.com/cli/cli/blob/b300f2ec7ec9dc9addc39b2ad88c54097ded7ca0/pkg/cmd/skills/skills.go#L16-L59) 和 [GitHub CLI 官方 manual](https://cli.github.com/manual/gh_skill)。`list` 在 [`v2.94.0`](https://github.com/cli/cli/releases/tag/v2.94.0) 加入，说明 Preview 期间的命令面会随版本变化。

核验快照没有 `remove` 或 `uninstall` 子命令。不能因为 `gh skill` 支持 update，就假设它也具有完整卸载能力。

### 2.2 它不采用 `skills` CLI 的统一拓扑

`gh skill install` 直接把内容写入所选 Agent 的 project 或 user Skill 目录，并向 `SKILL.md` frontmatter 写入 GitHub Source、ref、tree SHA、path 和 pin 信息。证据见 [installer 入口](https://github.com/cli/cli/blob/b300f2ec7ec9dc9addc39b2ad88c54097ded7ca0/internal/skills/installer/installer.go#L55-L82) 与 [安装和 metadata 写入](https://github.com/cli/cli/blob/b300f2ec7ec9dc9addc39b2ad88c54097ded7ca0/internal/skills/installer/installer.go#L251-L304)。Host 与 scope 的目标路径见 [`registry.go`](https://github.com/cli/cli/blob/b300f2ec7ec9dc9addc39b2ad88c54097ded7ca0/internal/skills/registry/registry.go#L399-L423)。

因此两套工具的已证明拓扑不同：

| 维度 | `skills` CLI | `gh skill` v2.96.0 |
| --- | --- | --- |
| 内容位置 | `.agents/skills` canonical store，再建立 Host 投影 | 直接写入目标 Host 目录 |
| 投影方式 | symlink 优先，允许 copy fallback | 没有通用 canonical store + symlink 承诺 |
| 来源证据 | 主要写入 lock v3 | frontmatter metadata 与 lock v3 |
| 来源范围 | Git、local、well-known 等多种来源 | GitHub repository 或 local directory |
| 移除能力 | 有 `remove` | 核验快照未提供 |

### 2.3 lock 互操作会产生来源歧义

GitHub CLI 会写兼容的 `~/.agents/.skill-lock.json`，源码明确要求其 schema 与 Vercel `skills` 保持一致。证据见 [`lockfile.go` 的版本与兼容说明](https://github.com/cli/cli/blob/b300f2ec7ec9dc9addc39b2ad88c54097ded7ca0/internal/skills/lockfile/lockfile.go#L16-L48) 和 [读写实现](https://github.com/cli/cli/blob/b300f2ec7ec9dc9addc39b2ad88c54097ded7ca0/internal/skills/lockfile/lockfile.go#L94-L136)。

所以 `.skill-lock.json` 是可互操作的收据协议，不是可靠的执行者标识。恢复 Installation Chain 时需要同时观察：

- lock 字段；
- `SKILL.md` 中的 GitHub metadata；
- canonical store、直接 Host 目录和 symlink/copy 拓扑；
- 已知工具版本与对应能力；
- 其他来源或厂商状态文件。

任何单项证据都不应被升级成唯一来源或唯一执行工具的结论。

## 3. Lark CLI 的委托链

### 3.1 官方安装入口真实存在

npm registry 中存在官方 package `@larksuite/cli`，其 `bin` 为 `lark-cli`。可通过 [npm registry](https://registry.npmjs.org/%40larksuite%2Fcli/latest) 和官方 [`package.json`](https://github.com/larksuite/cli/blob/8897196dee2882efe410ecd7183d40446ed9d3d7/package.json#L2-L27) 复核。

用户运行：

```bash
npx @larksuite/cli@latest install
```

官方源码显示的委托链为：

1. `npx` 解析 `@larksuite/cli@latest` 并执行 `lark-cli`；
2. `scripts/run.js` 把 `install` 路由到安装向导；
3. 向导安装 Lark CLI 本体；
4. 向导通过 `npx -y skills ls -g` 检查全局 Lark Skills；
5. 需要安装时调用 `npx -y skills add https://open.feishu.cn -y -g`，失败后再尝试 GitHub 来源。

入口路由见 [`run.js`](https://github.com/larksuite/cli/blob/8897196dee2882efe410ecd7183d40446ed9d3d7/scripts/run.js#L44-L49)，CLI 本体安装见 [`install-wizard.js`](https://github.com/larksuite/cli/blob/8897196dee2882efe410ecd7183d40446ed9d3d7/scripts/install-wizard.js#L237-L260)，Skill 委托见同文件的 [检查与安装逻辑](https://github.com/larksuite/cli/blob/8897196dee2882efe410ecd7183d40446ed9d3d7/scripts/install-wizard.js#L263-L295)。

准确角色是：

```text
npx
└── 解析并执行 @larksuite/cli
    └── Lark 安装向导编排配置与 Skill 同步
        └── skills CLI 写入 Skill、Host 投影和 lock v3
            └── open.feishu.cn / GitHub 提供内容
```

### 3.2 更新同样委托 `skills` CLI

`lark-cli update` 会先识别 Lark CLI 本体的安装方式。在需要同步官方 Skills 时，它读取官方 well-known index，再选择 `npx skills` 或 `pnpm dlx skills` 执行列表与增量安装。命令构造见 [`updater.go`](https://github.com/larksuite/cli/blob/8897196dee2882efe410ecd7183d40446ed9d3d7/internal/selfupdate/updater.go#L271-L366)，同步计划见 [`sync.go`](https://github.com/larksuite/cli/blob/8897196dee2882efe410ecd7183d40446ed9d3d7/internal/skillscheck/sync.go#L276-L347)。

Lark 还维护自己的 `skills-state.json`，记录目标 CLI version、官方 Skill 列表以及更新、添加和跳过结果，见 [`state.go`](https://github.com/larksuite/cli/blob/8897196dee2882efe410ecd7183d40446ed9d3d7/internal/skillscheck/state.go#L18-L35)。这份状态可以补充 Installation Chain，但不能替代真实目录、lock 和 Source 证据。

已证明的是“Lark 按官方 index 编排 Bundle 同步”，不是“Lark CLI 能精确判断每个 Skill 的独立远端版本”。`update --check` 与真正同步也不是同一个动作；相关条件见 [`update.go`](https://github.com/larksuite/cli/blob/8897196dee2882efe410ecd7183d40446ed9d3d7/cmd/update/update.go#L369-L383)。

## 4. npm 与 Homebrew 的边界

npm 与 `npx` 负责 package 分发和执行，不定义所有 package 产生的 Skill 拓扑。下面两条链不能被缩写成“npm 管理这些 Skill”：

```text
npx skills
└── skills package 实现 Skill 协议

npx @larksuite/cli install
└── Lark CLI 再委托 skills package
```

Homebrew 官方 [`brew install` manual](https://docs.brew.sh/Manpage#install-options-formulacask-)把核心对象定义为 formula 和 cask。Homebrew 允许第三方扩展命令，见 [External Commands](https://docs.brew.sh/External-Commands)，但这不证明存在 Homebrew 内置的通用 Agent Skill 协议。

一个 Skill 相关 CLI 由 npm 或 Homebrew 安装，只能说明该 CLI binary 的分发渠道。要恢复 Skill 的 Installation Chain，仍需检查该 CLI 自己的源码、receipt、metadata 和目录拓扑。

## 5. 对 SkillYard 1.0 的意义

SkillYard 当前产品边界以 [1.0 产品契约](../1.0-product-contract.md) 和 [接管与管理权设计](../1.0-management-authority.md) 为准。

外部工具在 SkillYard 中只有两种作用：

- **Installation Chain**：说明内容曾经经过哪些 package runner、CLI、Source 和同步步骤；
- **Management Evidence**：帮助识别 Source、Bundle 边界、Skill Identity、原始安装位置和目录拓扑。

SkillYard 可以读取这些确定性证据，展示能够确认的事实，并在接管计划中解释为何判断某些文件来自同一安装组。证据不足时保持 Unknown Provenance，不根据命令名称、目录相邻或单一 lock 猜测来源。

SkillYard 1.0 不执行 `npx skills`、`gh skill`、Lark CLI 或用户提供的任意 shell 命令来安装或更新。直接安装由 SkillYard 支持的 Source Adapter 获取内容；外部命令产生的已有安装，只能在用户运行“刷新本机”后成为 Inventory 或 Takeover Candidate。

用户确认 Takeover 后，外部工具不再控制正在使用的本地主副本。SkillYard 把内容纳入 Central Store，成为唯一 Local Lifecycle Authority，负责 Current Content、Mount、Bundle Update 和 Bundle 删除。原 CLI、lock、frontmatter 与厂商状态仍可作为历史来源证据，但不能直接覆盖、更新或删除受管主副本。

因此，以下推论都不成立：

- 有 lock，不等于能够唯一识别安装工具；
- 有 update 命令，不等于所有 Source 类型都可检查更新；
- 一个 CLI 能安装 Skill，不等于它能卸载 Skill；
- 内容位于 `~/.agents/skills`，不等于它必须继续由 `skills` CLI 控制；
- 命令以 `npx` 开头，不等于 npm 是 Skill 的管理者；
- CLI 由 Homebrew 安装，不等于 Homebrew 管理它产生的 Skill；
- Lark 使用 `skills` CLI，不等于其他发布者采用同一委托链；
- 外部工具曾经安装内容，不等于接管后仍拥有主副本。

## 最终判断

`skills` / `npx skills`、GitHub CLI `gh skill` 和 Lark CLI 的委托链都是真实存在、可以从官方源码证明的实现。它们的命令能力、目录拓扑和收据并不相同；lock v3 甚至可能由不同工具共同写入。

这些事实足以支持 SkillYard 的扫描、来源恢复和接管影响说明，但不足以建立一个长期的“Provider-managed Installation”产品状态，也不足以支持执行外部命令的通用 Adapter。SkillYard 只保存可验证的 Installation Chain 与 Management Evidence；用户确认接管后，SkillYard 独占本地生命周期管理权。
