# cc-switch 3.17.0 Skill 管理机制核验

> 本文是基于固定源码快照的事实记录，不是 SkillYard 产品规范。
> SkillYard 1.0 的现行边界只以
> [产品契约](../1.0-product-contract.md)和
> [接管与管理权设计](../1.0-management-authority.md)为准。

## 研究范围

- 研究对象：[`farion1231/cc-switch`](https://github.com/farion1231/cc-switch)
- 固定源码快照：[`f6e37ed99443890a865669e28bf1caf5e85d466d`](https://github.com/farion1231/cc-switch/tree/f6e37ed99443890a865669e28bf1caf5e85d466d)
- 对应应用版本：[`3.17.0`](https://github.com/farion1231/cc-switch/releases/tag/v3.17.0)
- 核验日期：2026-07-15；安装入口复核日期：2026-07-16
- 证据范围：项目自己的 README、用户手册、发布说明和源码

## 结论

cc-switch 已经证明一套轻量的本机 Skill 管理流程可以工作：

```text
来源与搜索
  → 安装或扫描已有 Skill
  → 保存一份本地主副本
  → 投影到多个 Agent 目录
  → 用本地 SQLite 记录来源与启用状态
  → 检查更新或卸载
```

它不需要自建公共 Skill registry。已安装状态、来源列表和 Agent 启用关系都保存在本机；`skills.sh` 只是可选搜索服务。

但 cc-switch 的本地对象是彼此独立的单个 Skill。它没有 SkillYard 当前定义的 Source 与本地 Bundle 关系、项目级 Mount、Host 管理内容只读边界，也没有同等强度的文件系统事务恢复。因此它适合作为界面、发现流程和“本地主副本 + 多 Agent 投影”的实证，不应被视为 SkillYard 数据模型或事务实现。

## 1. 安装与发现入口

### 1.1 用户入口

用户先选择当前 Agent，再进入 **Skills**。Skills 主面板同时提供三个入口：

1. **发现技能**；
2. **从 ZIP 安装**；
3. **导入已有**。

入口位置见 [App.tsx L912-L930](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src/App.tsx#L912-L930) 和 [App.tsx L1329-L1370](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src/App.tsx#L1329-L1370)。

发现页有两个在线视图：

- **仓库**：浏览已添加仓库，并按仓库、安装状态和关键词筛选；
- **skills.sh**：用户提交关键词后搜索。

界面实现见 [SkillsPage.tsx L369-L470](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src/components/skills/SkillsPage.tsx#L369-L470) 和 [SkillsPage.tsx L494-L526](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src/components/skills/SkillsPage.tsx#L494-L526)。

### 1.2 来源管理

cc-switch 的来源进入方式如下：

| 入口 | 实际行为 |
| --- | --- |
| 内置仓库 | 初始化时向本地数据库补入四个默认 GitHub 仓库 |
| 自定义仓库 | 保存用户输入的 `owner/name` 或 GitHub URL，以及可选 branch |
| `skills.sh` | 搜索外部服务，将可识别结果转换为 GitHub 仓库 |
| 本地 ZIP 或 `.skill` | 解压并安装，不创建远端仓库来源 |

四个默认仓库和初始化逻辑见 [skill.rs L133-L163](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L133-L163)。仓库管理界面只接受 GitHub 仓库级输入，没有独立的 subdirectory 输入；添加后递归发现仓库中的 `SKILL.md`：[RepoManagerPanel.tsx L39-L132](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src/components/skills/RepoManagerPanel.tsx#L39-L132)、[skill.rs L1936-L1981](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L1936-L1981)。

`ccswitch://` 深链也只是把仓库和 branch 写入来源表，不会立即安装 URL 中的目录：[用户手册](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/docs/user-manual/zh/5-faq/5.3-deeplink.md#L119-L123)、[deeplink/skill.rs L10-L50](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/deeplink/skill.rs#L10-L50)。

### 1.3 真实获取方式

在线安装最终都归一到 GitHub archive：下载指定 branch 的 ZIP，递归寻找 `SKILL.md`，再把发现结果展示为独立 Skill。下载和发现见 [skill.rs L2193-L2229](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L2193-L2229) 与 [skill.rs L1936-L2008](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L1936-L2008)。

`skills.sh` 只负责搜索。cc-switch 过滤不能表示为 GitHub `owner/repo` 的结果，再复用 GitHub 安装流程：[skill.rs L2785-L2844](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L2785-L2844)。

GitHub 和 `skills.sh` 结果按单个 Skill 安装；本地 ZIP 会安装其中全部识别到的 Skill；导入已有界面允许多选。相关实现见 [SkillCard.tsx L133-L163](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src/components/skills/SkillCard.tsx#L133-L163)、[skill.rs L637-L695](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L637-L695) 和 [UnifiedSkillsPanel.tsx L729-L862](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src/components/skills/UnifiedSkillsPanel.tsx#L729-L862)。

版本 3.17.0 没有通用 URL 下载器、本地目录安装器，也不在应用内执行 `npx`、npm、Homebrew 或其他发布者命令。它只能扫描这些工具最终写入的文件。

## 2. 本地主副本与 Agent 投影

### 2.1 文件与 SQLite 共同组成状态

默认主副本目录是 `~/.cc-switch/skills/`，SQLite 位于 `~/.cc-switch/cc-switch.db`。用户也可以把主副本目录切换为 `~/.agents/skills/`：[README_ZH L299-L306](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/README_ZH.md#L299-L306)、[skill.rs L472-L485](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L472-L485)。

`InstalledSkill` 是单个 Skill 记录，主要保存：

- GitHub 或本地 ID；
- `name`、`description` 和主副本目录名；
- GitHub owner、repo、branch 与文档 URL；
- 各 Agent 的启用布尔值；
- 安装、更新时间和内容哈希。

结构见 [app_config.rs L166-L201](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/app_config.rs#L166-L201)。SQLite 的 `skills` 表与它近似一一映射；`skill_repos` 只保存仓库、branch 和启用状态：[schema.rs L82-L114](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/database/schema.rs#L82-L114)。

仓库内可以有多级路径，但主副本目录只采用最后一段目录名。相同目录名来自同一仓库时被视为同一个 Skill；来自不同仓库时直接报冲突：[skill.rs L569-L633](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L569-L633)。

### 2.2 投影到 Agent

直接安装完成后，cc-switch 会立即把 Skill 启用到进入页面前选中的当前 Agent：[SkillsPage.tsx L112-L113](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src/components/skills/SkillsPage.tsx#L112-L113)、[skill.rs L736-L765](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L736-L765)。

启用会把主副本同步到对应 Agent 的用户级全局目录；禁用会删除该目录中的对应入口，再更新数据库布尔值：[skill.rs L1340-L1365](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L1340-L1365)。

cc-switch 实际支持 `Auto`、`Symlink` 和 `Copy` 三种同步模式。`Auto` 优先软链接，失败时复制；复制模式需要后续再次同步才能反映主副本变化。模式和实现见 [skill.rs L23-L47](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L23-L47) 与 [skill.rs L1562-L1715](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L1562-L1715)。这是 cc-switch 的兼容行为，不是 SkillYard 当前的 Mount 规则。

## 3. 扫描与导入已有 Skill

### 3.1 扫描范围

`scan_unmanaged()` 检查受支持 Agent 的固定全局目录、存在时的 `~/.agents/skills/` 和当前主副本目录。它只读取这些根目录的直接子目录，要求其中存在 `SKILL.md`，再按目录名聚合发现位置：[skill.rs L1368-L1429](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L1368-L1429)。

固定目录包括 Claude、Codex、Gemini、OpenCode 和 Hermes 的用户级目录，并允许配置 override：[skill.rs L494-L545](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L494-L545)。进入 Skills 页面时会静默扫描一次，结果在短时间内复用，并用提示点表示存在未管理项：[UnifiedSkillsPanel.tsx L83-L103](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src/components/skills/UnifiedSkillsPanel.tsx#L83-L103)。

### 3.2 来源恢复

cc-switch 会读取 `~/.agents/.skill-lock.json`，但只接受 `sourceType = "github"` 的记录，并提取仓库、Skill 路径、branch 和来源 URL 信息：[skill.rs L291-L431](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L291-L431)。

匹配到 lock 时，导入项获得 GitHub 来源；匹配不到时记录为 `local:<directory>`，不能使用 GitHub 更新检查。识别出的仓库同时写入本地来源表：[skill.rs L2849-L2924](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L2849-L2924)。

它不读取 npm lock、Homebrew receipt、任意 Git remote、shell history 或安装器日志。因此它恢复的是有限的结果证据，不是完整安装命令履历。

### 3.3 导入行为

导入时，后端找到首个同名候选目录，复制到主副本目录，解析 `SKILL.md`，尝试恢复 GitHub 来源，计算内容哈希并写入数据库：[skill.rs L1431-L1534](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L1431-L1534)。

这条流程不会删除原实体目录、把原位置统一替换成软链接、比较多个同名副本的内容，也没有多步骤失败后的完整恢复。因此它是“复制并登记”，不是 SkillYard 当前定义的接管事务。

cc-switch 还有一次性 schema 迁移流程，会把各 Agent 的 Skill 复制到主副本并重建数据库。它只在迁移标志存在时运行，不是每次启动都接管外部内容：[skill.rs L2927-L3042](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L2927-L3042)、[lib.rs L509-L548](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/lib.rs#L509-L548)。

## 4. 更新、禁用与卸载

### 4.1 更新检查

cc-switch 对 Skill 目录中的非隐藏文件计算稳定 SHA-256：按相对路径排序后，把路径和文件内容共同写入 hasher：[skill.rs L812-L859](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L812-L859)。

检查时会跳过本地来源，按 `owner + repo + branch` 合并下载请求，下载最新 archive，再按安装目录名定位远端 Skill 并比较内容哈希：[skill.rs L861-L975](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L861-L975)。它检查的是本地与远端内容是否不同，不是 SemVer，也不能区分上游变化和本地修改。

### 4.2 执行更新

更新一个 Skill 时，cc-switch 下载 archive，尝试备份旧内容，删除当前主副本目录，复制新目录，更新数据库和内容哈希，再同步到已启用 Agent：[skill.rs L978-L1104](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L978-L1104)。

前端“全部更新”按列表逐项调用更新；某项失败不会撤销已经成功的项，也不阻止后续项：[UnifiedSkillsPanel.tsx L258-L277](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src/components/skills/UnifiedSkillsPanel.tsx#L258-L277)。

### 4.3 禁用与卸载

禁用只删除某个 Agent 目录中的入口，保留主副本和其他 Agent 状态。

卸载一个 Skill 会尝试备份当前内容，从全部 Agent 全局目录移除同名入口，删除主副本，最后删除数据库记录：[skill.rs L770-L810](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L770-L810)。备份保存文件和 `InstalledSkill` metadata，整个备份目录最多保留 20 份：[skill.rs L2364-L2462](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L2364-L2462)。

cc-switch 的卸载对象仍是单个 Skill；它没有一个代表完整来源安装组的本地删除对象。

## 5. 崩溃恢复边界

本次固定源码中，更新和卸载都是依次执行文件备份、删除、复制、Agent 同步和数据库写入。更新实现会忽略某些备份错误，删除旧目录后的复制失败也没有统一的持久化阶段记录；卸载同样跨越多个文件系统与数据库步骤。[更新实现](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L978-L1104)、[卸载实现](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L770-L810)。

主副本目录迁移也逐项移动内容，最后更新设置并重建 Agent 入口：[skill.rs L1137-L1214](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L1137-L1214)。

在该版本中，没有发现与 SkillYard 当前设计相当的持久化文件系统事务阶段、单一生效点和启动幂等重放机制。cc-switch 的备份降低了误删后的恢复成本，但不能等同于中断后自动恢复一致状态。

## 6. 其他明确边界

### 6.1 没有真实项目级安装关系

Agent 投影目标都是用户级全局目录，`get_app_skills_dir()` 不接收项目路径，`skills` 表也没有 project/workspace 字段：[skill.rs L494-L545](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L494-L545)、[schema.rs L82-L101](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/database/schema.rs#L82-L101)。

3.17.0 中名为“项目”的能力是全局配置快照，不绑定某个本地项目目录：[profile.rs L1-L14](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/profile.rs#L1-L14)、[发布说明 L64-L79](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/docs/release-notes/v3.17.0-zh.md#L64-L79)。

### 6.2 没有 Host 管理内容只读状态

`InstalledSkill` 没有 `managed_by`、`host_owned`、`read_only` 或操作能力字段。扫描只看固定 Agent 目录、`~/.agents/skills/` 和主副本目录，不枚举 Host 私有插件清单：[app_config.rs L166-L201](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/app_config.rs#L166-L201)、[skill.rs L1378-L1390](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L1378-L1390)。

如果 Host 自带内容出现在普通扫描目录中，它可能被当作未管理 Skill；导入后，卸载流程没有额外的 Host 所有权检查：[skill.rs L776-L809](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/services/skill.rs#L776-L809)。

### 6.3 本地管理不依赖公共 registry

仓库列表、安装记录和 Agent 启用状态都存于本地 SQLite，Skill 文件保存在本地主副本目录。默认仓库只是应用写入本地表的初始值；`skills.sh` 搜索失败不会使已经安装的 Skill 无法使用：[schema.rs L82-L114](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/database/schema.rs#L82-L114)、[dao/skills.rs L235-L261](https://github.com/farion1231/cc-switch/blob/f6e37ed99443890a865669e28bf1caf5e85d466d/src-tauri/src/database/dao/skills.rs#L235-L261)。

因此，cc-switch 展示的是市场式发现体验，而不是它自己运营的公共 Skill registry。

## 7. 对 SkillYard 的有限参考价值

当前 SkillYard 文档已经吸收三项经验证的交互事实：

1. 安装页可以同时提供已添加仓库、`skills.sh` 搜索、本地文件和导入已有等明确入口；
2. 一份本地主副本可以通过软链接服务多个 Agent；
3. 已有安装可以先只读扫描，再由用户确认是否接管。

除此之外，SkillYard 的 Source、Bundle、Mount、Supported App、安装后未挂载状态、接管事务、完整 Bundle 更新、删除边界和 Host 只读内容都由当前
[产品契约](../1.0-product-contract.md)与
[接管与管理权设计](../1.0-management-authority.md)独立定义。

cc-switch 的单 Skill 数据表、安装后立即启用、用户级全局目录、有限 lock 解析、成员级内容哈希更新、逐项“全部更新”、顺序卸载和有限崩溃恢复，只用于说明参考产品的真实边界，不构成 SkillYard 1.0 的实现要求。
