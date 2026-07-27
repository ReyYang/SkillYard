# SkillYard 1.0 实施计划

> 状态：已批准，实施 Issue 已发布。
>
> 产品依据：[SkillYard 1.0 PRD](../prd/0001-skillyard-1.0.md)。若本计划与 PRD 或产品契约冲突，以产品文档为准并修正本计划，不能在实现中自行改变产品行为。

## 目标

在产品所有者的一台 Apple Silicon Mac 上，完成一个可以日常使用的 `SkillYard.app`。应用使用 Tauri 2、TypeScript 和 Rust，统一管理用户明确交付的 Skill，并完整支持扫描、安装、Mount、Takeover、Source、Bundle 更新、删除和中断恢复。

本计划描述实施顺序，不把中间阶段当作可以降低 1.0 要求的 MVP。每个阶段都必须产生可运行、可验证的纵向结果；不能先分别建设完整前端、数据库或文件系统层，再到最后才贯通用户流程。

## 固定实施原则

- `SkillYard.app` 是唯一用户入口。新实现不提供 CLI、localhost Server、Python sidecar、公开 API 或通用 JSON dispatcher。
- TypeScript 只负责界面、临时表单状态和用户选择；SQLite、文件系统、Source 获取、Plan、事务与恢复只由 Rust Lifecycle Core 执行。
- 主要业务 seam 固定为 `SkillYardApplication::handle(UiIntent) -> UiOutcome`。Tauri command 只是这一入口的薄适配层。
- 前端不能获得通用 `fs`、`sql` 或 `shell` 权限，只能调用封闭的任务级命令。
- 每个阶段同时完成必要的 UI、Rust Core、SQLite migration、真实文件系统测试和最薄的 Tauri 验证。
- 每个会修改受管内容或受控路径的操作，必须在同一阶段完成 Plan、确认、Filesystem Transaction Journal、幂等恢复和失败测试，不能把正确性推迟到最终阶段。
- 主要业务测试使用真实临时目录、真实文件型 SQLite、正式 migrations、真实文件和软链接。只有最外层网络传输可以替换。
- 旧 Python 原型已经从当前工作区删除。1.0 只根据 PRD 和本计划重建仍然有效的测试场景与不变量，不兼容旧 API、旧数据库或旧领域模型。
- 不为公开分发、Developer ID、notarization、应用更新、备份恢复、日志页面或 CLI 预留实现。

## 自动化验收约定

计划中的“用户点击”“用户选择”“用户确认”和“手动导入”描述产品授权边界，不代表验收必须由真人操作。除非明确标记，所有阶段验收项默认属于 `[AUTO]`，由 AI 或本地自动化命令执行。

- `[AUTO]`：在隔离临时根目录中，通过 TypeScript component event 或类型化 `UiIntent` 驱动正式 `SkillYardApplication`，并断言 `UiOutcome`、真实 SQLite、文件、`current`、Mount 和网络请求结果。
- `[MAC-CONTRACT]`：仍由 AI 自动执行，但必须在产品所有者当前 Mac 上使用真实构建、当前文件系统或已安装 Agent 应用完成；它与普通隔离测试分开运行。
- `[HUMAN]`：只用于文案是否容易理解、关键页面是否符合真实使用习惯，以及最终日常使用体验等主观判断，不能代替业务正确性测试。
- 普通 `[AUTO]` 测试必须使用注入的临时 Central Store、Supported App 和 Project 根目录；一旦试图访问真实 `~/Library/Application Support/SkillYard/` 或真实 Agent Skill 目录，应立即失败。
- Source 测试只替换最外层 `SourceTransport`，使用真实协议格式 fixture；live 网络 smoke 属于单独的 `[MAC-CONTRACT]`，不能成为普通回归测试的稳定性前提。
- 事务测试使用确定性时钟、ID、failpoint 和子进程强制终止，自动覆盖生效点前后恢复与幂等重放。
- 仓库最终提供一个开发者验收入口，顺序运行 Rust、TypeScript、IPC、Tauri smoke 和可选本机 contract，并输出机器可读结果。它是开发工具，不是 SkillYard 面向用户的 CLI。
- 只有无法通过临时 Project 或机器可读接口验证真实 Host 时，才请求用户明确批准创建一个唯一命名的临时测试 Skill；AI 负责影响预览、执行和清理。

## 阶段总览

| 阶段 | 可观察结果 |
| --- | --- |
| 1. 首次启动与本机清单 | 用户主动扫描后看到已持久化的本机 Skill Inventory，重启不自动重扫 |
| 2. 从文件夹导入 Bundle | 一次性导入本地文件夹，得到“已安装、未挂载”的受管 Bundle |
| 3. Mount 与 Project | 已安装 Skill 可以挂载到三个 Supported App 的 global 或 project 位置 |
| 4. Takeover | 已有安装可以安全进入 Central Store，并保留原有使用关系 |
| 5. GitHub Source | 公共 GitHub 仓库可以被发现、登记并安装为关联 Bundle |
| 6. 其余安装入口 | `skills.sh`、URL、ZIP、`.skill` 和 Editable Local Source 复用统一安装流程 |
| 7. 补充来源与归并 | 无 Source Bundle 可以关联来源，必要时事务性归并两个 Bundle |
| 8. Bundle 更新 | 用户可以检查并原子更新整个 Bundle，也可以顺序批量更新 |
| 9. 移除与删除 | Mount、Project、Source 和 Bundle 使用各自明确且可恢复的删除语义 |
| 10. 1.0 完整验收 | 全部主流程、中断恢复、本机应用与数据保留契约通过最终验收 |

## 实施 Issue

父 Issue 为 [#18 SkillYard 1.0 PRD](https://github.com/ReyYang/SkillYard/issues/18)。以下 20 个纵向 Issue 已按依赖顺序发布；实现从唯一无前置依赖的 P1-01 开始。

| ID | GitHub Issue |
| --- | --- |
| P1-01 | [#19 启动应用并由用户授权首次只读扫描](https://github.com/ReyYang/SkillYard/issues/19) |
| P1-02 | [#20 浏览并主动刷新持久化的本机清单](https://github.com/ReyYang/SkillYard/issues/20) |
| P2-01 | [#21 原子导入单 Skill 文件夹并支持中断恢复](https://github.com/ReyYang/SkillYard/issues/21) |
| P2-02 | [#22 导入经选择的多 Skill Bundle 并拒绝不安全内容](https://github.com/ReyYang/SkillYard/issues/22) |
| P3-01 | [#23 在 Codex global/project 安全创建和移除 Mount](https://github.com/ReyYang/SkillYard/issues/23) |
| P3-02 | [#24 在三个 Supported App 批量 Mount 并处理 Drift](https://github.com/ReyYang/SkillYard/issues/24) |
| P4-01 | [#25 接管普通已有安装并保留原使用关系](https://github.com/ReyYang/SkillYard/issues/25) |
| P4-02 | [#26 归并冲突副本并安全迁出共享 Skill 目录](https://github.com/ReyYang/SkillYard/issues/26) |
| P5-01 | [#27 登记 canonical GitHub Source 并可靠维护 Catalog](https://github.com/ReyYang/SkillYard/issues/27) |
| P5-02 | [#28 从 GitHub Catalog 安装或补装未挂载 Bundle](https://github.com/ReyYang/SkillYard/issues/28) |
| P6-01 | [#29 通过 skills.sh 或确定性 URL 安装受支持 Source](https://github.com/ReyYang/SkillYard/issues/29) |
| P6-02 | [#30 从 ZIP、.skill 或目录安装并选择快照或 Editable Source](https://github.com/ReyYang/SkillYard/issues/30) |
| P7-01 | [#31 为现有 Bundle 关联 Source，必要时事务归并](https://github.com/ReyYang/SkillYard/issues/31) |
| P8-01 | [#32 用户主动检查 Bundle 更新状态](https://github.com/ReyYang/SkillYard/issues/32) |
| P8-02 | [#33 原子更新单个 Bundle](https://github.com/ReyYang/SkillYard/issues/33) |
| P8-03 | [#34 顺序批量更新多个 Bundle](https://github.com/ReyYang/SkillYard/issues/34) |
| P9-01 | [#35 非破坏性移除 Mount 与 Project](https://github.com/ReyYang/SkillYard/issues/35) |
| P9-02 | [#36 删除 Source 但保留本地 Bundle](https://github.com/ReyYang/SkillYard/issues/36) |
| P9-03 | [#37 双重确认 Cascading Delete Bundle](https://github.com/ReyYang/SkillYard/issues/37) |
| P10-01 | [#38 交付并验收本机 SkillYard 1.0](https://github.com/ReyYang/SkillYard/issues/38) |

## 阶段 1：首次启动与本机清单

### 目标

用第一条只读流程贯通正式应用架构，不在事务内核完成前修改用户 Skill。

### 范围

- 建立可启动的 Tauri 2、TypeScript 和 Rust 应用骨架。
- 建立封闭的 `UiIntent`、`UiOutcome` 与 `SkillYardApplication` 入口。
- 建立正式 SQLite migration 和可注入测试根目录的存储层。
- 内建 Codex、Claude Code 和 GitHub Copilot 的路径配置、共享扫描目录和只读目录。
- 首次启动展示扫描范围、只读性质和不会自动接管的说明。
- 只有用户点击“开始扫描”后才读取已支持目录。
- 扫描并持久化 Inventory、首次扫描状态和最近结果。
- 主界面第一层以 Bundle／只读管理分组展示结果；进入分组后看 Skill，进入 Skill 后看详情，并按证据区分四种管理状态。
- 返回用户启动时读取已保存状态，不自动执行完整 Local Refresh 或访问上游。

### 不包含

- Central Store 内容写入、安装、Mount、Takeover、Source 网络请求和 Filesystem Transaction Journal。

### 阶段验收

- 点击“开始扫描”前不读取 Supported App 或 Project Skill 目录。
- 空结果也保存为已完成，重启后不重复首次介绍。
- 扫描不联网，不移动、覆盖、删除或接管任何文件。
- SQLite 重开后 Inventory 与首次扫描状态保持一致。
- TypeScript 用户动作、Tauri typed command 和内部 application seam 返回一致结果。
- `[MAC-CONTRACT]` 本机 `arm64` `.app` 可以启动并加载打包资源，生产进程不启动 localhost，也不包含 Python sidecar。

## 阶段 2：从文件夹导入 Bundle

### 目标

通过最可控的写入流程建立 Central Store、Bundle `current` 和事务恢复基础。

### 范围

- 用户选择一个已经下载或解压到本机的完整文件夹。
- 普通文件夹导入是一次性快照，不持续读取原路径，也不自动登记为 Editable Local Source。
- 发现有效 `SKILL.md`，默认选择全部成员，并允许用户在最终确认前取消成员。
- 完成严格 YAML、名称、Nested Skill Conflict、特殊文件和内容边界验证。
- 生成绑定当前候选内容与前置状态的安装 Plan。
- 确认后不可取消；应用级写入门保证只有一个生命周期写事务。
- 在临时区准备完整候选 Bundle，经验证后写入 Managed Bundle Directory，并原子建立唯一 `current`。
- SQLite 保存 Bundle、Skill Member、Member Selection、Current Content 引用和事务状态。
- 原输入目录保持不变，安装完成后所有成员显示“已安装、未挂载”。
- 生成和维护 Central Store 根目录中的 `SKILLYARD-INFO.md`。

### 不包含

- Supported App Mount、Project、GitHub、ZIP、更新、Takeover 和删除。

### 阶段验收

- Plan 生成和用户确认前没有业务文件写入。
- symlink、hard link、FIFO、socket、device node、非法名称和嵌套成员被正确拒绝。
- `current` 切换前中断时不产生已安装 Bundle；切换后中断时恢复并完成新 Bundle。
- 重放恢复步骤不会创建重复内容或重复数据库记录。
- 原输入目录在成功、失败和中断场景下均不被修改。
- 安装后不存在任何自动 Mount。

## 阶段 3：Mount 与 Project

### 目标

让 Central Store 中的 Skill 通过受控软链接供三个 Supported App 使用。

### 范围

- 实施顺序为 Codex global Mount、Project 登记与 project Mount、Claude Code 和 GitHub Copilot。
- 用户可以主动添加 Project；添加时只读扫描已支持的项目 Skill 目录。
- 用户为每个 Skill 选择 Supported App，以及 global 或已登记 Project scope。
- Mount 叶子目录固定使用 Skill Name，并指向 Bundle `current` 下的稳定成员路径。
- 同一 Skill 可以挂载到多个 Supported App，也可以在同一应用的多个不同 Project 中使用 project Mount。
- 同一 Skill 在同一 Supported App 中不能同时存在 global 与 project Mount。
- 创建前检查全部目标路径；未知内容占用时进入 Mount Conflict。
- 正确软链接只校正记录，不重复创建。
- 支持移除单个 Mount，不删除 Skill 或 Bundle。
- 启动和 Local Refresh 检查 Mount Drift；目标为空时可以生成修复 Plan，目标被占用时保持冲突。
- 对共享扫描路径和跨应用可见性显示准确提示。

### 不包含

- Takeover、Source、Bundle 更新和 Bundle 删除。

### 阶段验收

- 三个 Supported App 的 global 和 project 路径均有文件系统 contract test。
- `[MAC-CONTRACT]` 当前个人电脑上的 Codex 版本必须实际验证 `.codex/skills` global 与 project 路径；验证失败时不能继续宣称支持该路径。
- Mount 始终是软链接，不自动改名、覆盖未知内容或回退为复制。
- Batch Mount 在确认集合内全成或全退。
- 移除 Mount 后 Bundle 内容和其他 Mount 保持不变。
- 外部删除、替换或改指 Mount 时能区分 Drift 与 Conflict。

## 阶段 4：Takeover

### 目标

把扫描发现的待接管 Skill 或确定性安装组安全迁入 Central Store，同时保持用户已有使用关系。

### 重做实施边界

Stage 4 在提交 `9abc7d0` 后从 Stage 3 已验收基线重新实现。此前同一阶段内先建立单路径 Takeover、再并行增加 `v2` 多路径协议，造成两套 Plan、Transaction、Journal 和恢复入口；该实现已经完整撤销，不能作为新实现的兼容前提或代码基础。

新实现必须遵守以下边界：

- 只有一套不带版本后缀的 `TakeoverPlan`、`TakeoverTransaction`、Filesystem Transaction Journal 和生产确认入口。
- 一个 Plan 表达一个最终 Bundle。Bundle 可以包含一个或多个 Skill Member；每个 Member 分别保存一个 Skill Identity、一个被用户选中的内容副本、多个原始位置和多个最终 Mount。单成员、多成员、重复副本、scope 冲突与共享目录都是同一模型的不同输入，不建立特殊的第二套事务。
- 创建 Plan 时冻结全部用户选择和后端派生路径；确认接口只接收 opaque `plan_id`，不能在确认时再次提交路径、Mount 或内容选择。
- Plan 和扫描完全只读。确认后才建立一个接管事务；SQLite 记录总体阶段，Journal 记录完整文件操作合同和逐路径进度。
- 领域状态提交前发生正常失败或中断时，恢复原始位置并移除本次候选与新增 Mount；领域状态提交后只向前完成验证和清理。未知占用、路径身份变化或权限异常进入 blocked recovery，不能猜测。
- 共享目录的应用专属 Mount 全部建立并验证后，才能最后移除共享入口；失败时撤销本次新增 Mount 并恢复共享入口。
- 不保留旧 Takeover 生产入口，不迁移已撤销的开发期 Stage 4 schema，也不建立 Stage 5 Source 模型。

公开验收 seam 固定为：

1. `SkillYardApplication::handle`：扫描、创建 Plan、确认和重新启动后的 Inventory。
2. 隔离的真实临时目录：原 Skill、Central Store `current` 和 Host Mount 的最终状态。
3. typed Tauri command/client 与 `App`：用户选择、只读影响预览、不可取消确认和失败后重读。
4. 独立子进程硬退出：确认各持久化阶段的重启恢复和幂等性。

### 完整技术实现图

Stage 4 的六个切片共同实现并验证下面这一条接管路径，不能各自建立独立流程：

```text
TakeoverPlan（只读、已封存）
  -> TakeoverTransaction + TakeoverJournal
  -> 在 staging 中准备完整 Bundle 内容
  -> 原子发布 contents/<content-id> 并建立 Bundle current
  -> 隔离全部原始 Skill 目录
  -> 在用户确认的 Supported App 位置建立全部 Mount
  -> 验证 current、内容和 Mount
  -> 一次 SQLite 事务提交 Bundle、全部 Member、Selection、Mount 并消费 Plan
  -> 清理隔离内容、Journal 和 Transaction
```

模型与职责固定如下：

- `TakeoverPlan`：保存一个 Bundle 的成员边界、扫描证据、每个成员的内容选择、全部原始位置、最终 Mount 和后端派生路径。创建后不可修改；确认只传 `plan_id`。
- `TakeoverTransaction`：SQLite 中唯一的接管总体状态，负责回答操作是否已经越过领域提交点。
- `TakeoverJournal`：文件系统事务合同，保存候选内容、原始位置、隔离位置、最终 Mount 和逐项进度；它不是第二套领域状态。
- `takeover.rs`：编排上述单一路径。单成员、多成员、重复副本、scope 冲突和共享目录只能改变 Plan 输入，不能改变事务协议。
- `storage.rs` 与 `0011_takeover_transactions.sql`：保存唯一接管事务，并在一个 SQLite 事务中提交完整受管领域状态；不引入 Source 或 Stage 5 schema。
- `application.rs`、typed Tauri command/client 与 UI：只暴露创建 Plan、确认 Plan、读取持久状态三个生产动作，不暴露内部文件步骤。

Stage 3 能力按以下方式直接复用：

- 继续使用全局 lifecycle writer lock，并在写入前调用 `LifecycleLock::recheck`，防止 Takeover 与 Install、Mount 同时修改 Central Store。
- 使用 `copy_single_skill_tree_into_open_directory` 和 `BundleCopyBudget` 复制并校验 Skill，沿用既有的路径、资源上限和内容指纹规则。
- 使用 `open_managed_directory_from_root`、`open_directory_at`、`mkdir_at`、`entry_metadata_at`、`rename_at_no_replace`、`symlink_at`、`unlink_at` 和 `write_atomic_at` 操作 Central Store；不新增基于裸绝对路径的弱化实现。
- 沿用 Stage 3 的原子发布方式：候选内容在 staging 完整准备后，原子移动到 `bundles/<bundle-id>/contents/<content-id>`，再通过临时链接和 no-replace rename 建立 `current`。
- 从现有 Mount 生命周期中复用 Host 父目录打开、重检和占用快照能力；Takeover 不能自己实现一套较弱的 Host 路径检查。
- 复用 `write_notice_from_storage` 更新 `SKILLYARD-INFO.md`，但不能串联调用 Install 与 Mount 的生产事务，因为那会产生两个提交点和可见的半完成状态。

恢复方向只有两个：

- `state_committed` 之前：删除本次新 Mount 和候选，按 Journal 把所有原始目录从隔离位置恢复，然后清除未提交领域状态。
- `state_committed` 之后：保留新领域状态，只继续完成 Mount 验证、隔离内容和事务记录清理；重复启动必须幂等。

如果原位置或 Mount 被未知内容占用、路径身份变化或权限状态无法判断，相关事务进入 blocked recovery。正常中断不能询问用户选择旧状态还是新状态。

1.0 封存并重检可见事务根、原位置和 Mount 的身份，但不为隐藏 recovery／candidate 目录中的每个文件建立内容清单，也不防御同一用户进程在检查与删除之间主动制造竞态。这个取舍不降低正常硬退出恢复、重复启动幂等和同名根目录替换保护。

各切片只增加同一条路径的输入与验收覆盖：

| 切片 | 新增产品输入或观察点 | 不变的生产路径 |
| --- | --- | --- |
| Plan | 单成员、确定性多成员组、重复副本和唯一内容选择 | 只生成封存 Plan，不写文件 |
| 单副本确认 | 一个原位置和保留／排除 Mount | 唯一 TakeoverTransaction 与 Journal |
| Bundle 确认 | 一个或多个 Member，各自包含唯一选中内容和原始位置 | 同一候选发布、隔离、Mount 和提交顺序 |
| scope／共享目录 | 不同最终 Mount 拓扑 | 同一事务，仅共享入口最后隔离 |
| 恢复 | 各持久化阶段的硬退出 | 同一 Journal 按提交点前后恢复 |
| UI | typed IPC 与用户影响预览 | 调用同一创建／确认生产入口 |

例如，同一 Skill 同时存在于 Codex global 与 Claude Code project 时，Plan 会为同一个 Member 保存两个 origin 和两个最终 Mount；同一 lock v3 来源下的多个不同 Skill 则会形成多个 Member。两种情况确认后都只创建一个候选 Bundle、一个 `current`、一个 TakeoverTransaction 和一个 Journal，不会按 Member 拆成多条生命周期事务。

实现只允许按以下纵向切片推进，每片先出现公开 seam 的失败测试，再写最少实现，并独立提交、推送：

1. 单一 Takeover Plan：单成员、确定性多成员组、重复副本显式身份确认、每个成员的唯一内容选择和零文件修改。
2. 单副本确认：进入一个新 Bundle，并保留或排除已有 Mount。
3. Bundle 确认：全部 Member 一次进入同一个 Bundle；每个 Member 的所有位置统一使用其唯一内容，未选内容不形成历史版本。
4. scope 冲突与共享目录：形成用户选择的最终 Mount 拓扑。
5. 硬中断恢复：生效前恢复、提交后向前完成、重复启动幂等和未知内容阻塞。
6. UI 与 typed IPC：完整用户主流程和失败后持久状态重读。

任何切片需要第二套模型、兼容层、带版本后缀的协议或超出本节范围时，必须停止并取得用户确认，不能自行继续。

### 范围

- 从 Inventory 中选择 Takeover Candidate，并展示 Source 证据、Bundle 边界、成员、现有路径、Mount 和影响。
- Takeover Plan 绑定当前文件系统状态；状态变化后必须重新生成。
- 已符合 Central Store 规则的内容只登记并校正链接。
- 散落内容通过临时恢复内容、按需搬迁、Mount 重建和验证进入受管状态。
- 默认保留已有 global 或 project 使用位置，用户可以在确认前排除不需要的 Mount。
- 相同副本合并为一个主副本；不同副本由用户选择唯一内容，所有 Mount 最终统一使用它。
- receipt、lock、安装 manifest 或 Adapter 结果能证明同一安装组时，多个不同 Skill 在一个 Plan 和一个事务中进入同一 Bundle。
- 处理同一 Skill 的 global／project scope 冲突。
- 处理 `.agents/skills` 等共享目录：用户选择目标 Supported App，新 Mount 全部验证后才移除原共享入口。
- 没有 Source 也可以完成 Takeover，并显示“没有更新来源”。
- Agent-managed、Plugin-managed 和 Project-managed 内容继续只读展示，不生成接管写入计划。

### 不包含

- GitHub 来源查找、Source 关联和上游更新。

### 阶段验收

- 扫描和 Plan 阶段不修改现有安装。
- 任一失败或中断都不会同时丢失原安装与 Central Store 候选。
- 不同副本选择完成后只有一个当前内容，未选内容不形成历史版本。
- 只替换用户明确选择的 Skill 根目录，绝不替换整个 Supported App Skill 根目录。
- Takeover 重启恢复不会产生重复 Mount、重复成员或额外删除。
- 多成员 Bundle 在提交点前完整回滚、提交点后完整向前恢复，不能出现只接管部分 Member。

## 阶段 5：GitHub Source

### 目标

用公共 GitHub 仓库建立第一条完整的远程发现与安装链路。

### 范围

- 支持 `owner/repo`、仓库根 URL、带 ref 的 URL 和成员 URL。
- GitHub canonical Source 固定为完整仓库；不同 URL、ref、子目录和发现入口不能创建重复 Source。
- 未提供 ref 时读取并保存真实 default branch；明确提供的 ref 必须先验证。
- 内建四个已经确认的初始 GitHub Source，并按普通 Source 维护。
- Source 可以在没有 Bundle 时独立保存。
- 只有用户进入发现页或主动重新加载时才访问网络。
- 完整获取和验证成功后才替换 Source Catalog；失败时保留上次成功结果并标记 Stale。
- 从 Source Catalog 选择成员时默认全选；安装复用阶段 2 的验证、Plan、Central Store 和未挂载结果。
- 新 Bundle 安装成功后保存实际采用的 GitHub commit 基线。
- 只支持公共仓库，不实现 GitHub 登录、token 或私有仓库 API。
- 网络接收、归档条目、展开总量和单文件大小执行 PRD 固定的四项资源限制。

### 不包含

- `skills.sh`、普通 URL、ZIP、Editable Local Source、Update Check 和 Bundle Update。

### 阶段验收

- 同一仓库从所有受支持入口到达时只有一个 Source。
- 默认分支和明确 ref 均经过真实协议格式测试。
- Stale Catalog 可以查看，但不能驱动新安装。
- 超时、断流、空响应和资源超限不会改写旧 Catalog 或已有 Bundle。
- GitHub 安装结束后不自动创建 Mount。

### 完整技术实现图

阶段 5 包含连续的两条纵向行为：先登记 canonical GitHub Source 并可靠维护 Catalog，再从 fresh Catalog 新装或补装 Bundle。两条行为共享同一套 Source、Bundle 和 Install Plan 领域模型，但文件系统职责不同。

#### 输入与 GitHub 协议

- canonical identity 固定为小写 `github:<owner>/<repository>`；展示名使用 GitHub metadata 返回的 canonical `owner/repository`。ref、成员路径、推荐入口和 URL 写法都不参与 identity。
- 1.0 接受 `owner/repository`、`https://github.com/owner/repository`、可选 `.git` 后缀、`/tree/<ref>/<path>` 和 `/blob/<ref>/<path>/SKILL.md`。只接受公开 GitHub 的 HTTPS 地址；SSH、Gist、GitHub Enterprise 和任意网页返回稳定的不支持错误。
- URL 中未提供 ref 时读取仓库 metadata 的真实 default branch。`tree` 或 `blob` URL 默认把 ref 解析为一个 URL segment；包含 `/` 的 branch 必须通过界面的独立 Tracked Ref 字段明确提供，Adapter 再用该 ref 前缀分离成员路径。这样覆盖正常使用，同时不通过多次远端试探猜测歧义 URL。
- 独立字段和 URL 同时提供 ref 时二者必须一致，否则拒绝输入。成员路径只作为高亮提示。
- GitHub Adapter 使用无认证 REST：仓库 metadata 验证公开性并取得 `full_name/default_branch`，commit endpoint 验证 Tracked Ref 并取得 SHA，archive 始终按固定 SHA 获取，不能按可能移动的 branch 下载。
- 生产网络只发送固定 `User-Agent` 和 GitHub `Accept`，不读取 token、credential helper 或本机 Git 配置；不调用 `git`、`gh`、`npx` 等外部命令。

#### 唯一持久化模型

- `sources` 保存唯一仓库、展示名、Tracked Ref、最近 Catalog 成功信息和最近 reload 结果。四个初始 Source 只由 migration 插入一次，删除后不会在启动时复活。
- `source_catalog_members` 保存最近一次成功发现的成员 metadata。成功 reload 在一个 SQLite 事务中整体替换；失败只写最近错误并把旧目录派生为 Stale，不清空旧成员和成功时间。
- `source_bundle_links` 对 Source 和 Bundle 双向唯一，并保存新 Bundle 首次安装建立的 adopted commit；补装不推进该值。
- `source_member_links` 保存 Catalog 成员路径与已安装 Skill Member 的关系。
- Source metadata 与 Catalog 不创建 Filesystem Journal。Source 可以没有 Bundle；Catalog 也不是可离线安装的内容缓存。
- 现有 `install_plans` 和 `lifecycle_transactions` 仍是唯一安装协议。它们原地增加本地或 GitHub 输入、全新或补装模式、Source/commit/Catalog generation、受管临时快照和预期旧 `current` 等字段；不得新增 GitHub 专用 Plan、Transaction 或 Journal。

#### 获取、验证与 Catalog

- `SourceTransport` 是唯一可替换的网络外边界，只返回真实 HTTP status、headers 和可读取 body。GitHub JSON、URL canonicalization、archive 处理、成员发现和校验全部使用生产代码。
- HTTP body 流式写入应用控制的临时区，下一字节超过 100 MiB 时立即停止。Archive 在写出前拒绝绝对路径、`..`、规范化重复路径、symlink 和其他特殊类型，并同时限制 20,000 个条目、512 MiB 实际展开总量和 100 MiB 实际单文件大小。
- Archive 必须具有一个共同顶层目录，剥离后再复用阶段 2 的严格 YAML、名称、重复名称、嵌套成员、特殊文件和脚本风险验证。
- 合法 archive 中没有 `SKILL.md` 是 fresh empty Catalog；零字节、断流、损坏 JSON/ZIP 或危险 archive 是 reload failure。失败清理临时内容且不改变 Bundle。
- 第一次进入发现页才自动 reload 当前 Source；同一应用会话之后再次进入只读取 SQLite，只有用户点击“重新加载来源”才再次联网。启动、首次扫描和 Local Refresh 始终为零 Source 请求。

#### 唯一安装事务与生效点

- 用户从 fresh Catalog 继续安装时，SkillYard 按 Catalog 固定 commit 再次获取并验证完整内容，生成受现有 Plan TTL 和前置条件约束的临时快照。Stale、无有效成员或全部成员已安装时不能签发空 Plan。
- 全新安装默认选择所有有效成员，允许最终确认前取消。部分选择显示“不检查跨 Skill 依赖”的风险提示。已安装成员不可再次选择。
- GitHub 与本地文件夹共用一张安装确认页、同一个确认 Intent、同一套候选复制、Journal、`current` 生效点和启动恢复逻辑。
- 全新安装准备所选成员的完整候选，确认前不创建 Bundle；领域提交一次保存 Bundle、Members、Member Selection、Source 关联、成员映射和 adopted commit。Mount 数保持为零。
- 补装把当前完整 Bundle 的已有成员与本次新增成员共同复制到一个候选。已有成员不从上游覆盖，已有 Mount 继续指向稳定成员路径；候选验证完成后只原子替换一次 `current`，再一次提交新增成员和映射，adopted commit 不变。
- Journal 同时冻结可选的旧 `current` 和新的目标。恢复时，`current` 仍为旧目标或全新安装中不存在表示尚未生效，清理候选并保留旧状态；指向新目标表示继续完成领域提交和清理；指向第三个目标才进入人工恢复。
- 成功提交后幂等删除旧内容和 Plan 临时快照，不把它们暴露为 Revision、历史版本或回滚点。

#### 文件职责与依赖

- `github_source.rs`：GitHub 输入解析、无认证协议、`SourceTransport`、archive 资源与路径安全、Catalog 获取。
- `domain.rs` / `storage.rs`：Source 读模型、Catalog 状态、Tracked Ref 确认 Plan、唯一安装 Plan 的新增字段和原子领域提交。
- `content.rs`：对 Catalog 复用成员发现与验证，并把四项固定资源限制提供给 archive 与复制路径。
- `lifecycle.rs`：原地支持全新与补装两种模式；不建立第二个安装执行器。
- `application.rs` / typed Tauri commands / TypeScript client：只暴露任务级 Source、Catalog 和安装 Intent，不暴露通用 HTTP、SQL 或文件操作。
- 前端使用一个 Source Catalog 页面和一张通用安装确认页；主界面“安装 Skill”先进入仓库发现，本地文件夹入口保留在发现页中。
- 直接依赖限定为 URL parser、同步 HTTPS client 和 ZIP/Deflate reader；不引入 Git client、GitHub SDK、认证框架、通用 Adapter registry 或异步运行时体系。

### 阶段 5 切片

1. **Source 基线与零启动网络**：migration 建立四个普通 Source；公开 seam 证明启动、扫描和刷新不联网，进入发现页才返回 Source 状态。
2. **canonical 输入与 Tracked Ref**：四种常用输入去重；default branch、显式 ref 和 ref 切换均使用真实 GitHub 协议 fixture，切换失败不改旧状态。
3. **Catalog 原子 reload**：真实 ZIP fixture 穿过生产 archive 和内容验证；成功整体替换，失败保留 Stale，资源和路径风险在写出边界被拒绝。
4. **GitHub 全新安装**：fresh Catalog 生成通用 Plan，确认后创建一个 Source-backed Bundle、adopted commit 和零 Mount，并覆盖生效前后中断恢复。
5. **已有 Bundle 补装**：完整候选只切换一次 `current`，保留已有成员和 Mount，不推进 adopted commit，并覆盖中断恢复。
6. **typed IPC 与界面**：仓库发现、添加 Source、ref 确认、Stale 展示、通用安装确认和失败后重读最终状态形成真实用户流程。

每个切片都从 `SkillYardApplication::handle` 的失败验收开始，使用文件型 SQLite、真实临时文件系统和真实协议格式 fixture；私有函数测试只补充 URL grammar、流式计数和 archive entry 等算法边界。普通回归不依赖 live GitHub。

“功能完整”在本阶段表示上述登记、Catalog、新装、补装、原子恢复和 UI 主流程全部完成。1.0 不为低概率歧义和对抗场景扩张实现：不自动猜测含 `/` 的 branch URL，不实现认证限流规避、重试体系、断点续传、ETag、LFS/submodule 展开、仓库改名历史或用户主动追逐内部临时路径；这些输入要么返回稳定错误，要么沿用普通 Stale/前置条件失败语义。

## 阶段 6：其余安装入口

### 目标

让 1.0 的其他受支持输入复用已经验证的 Source、内容验证和 Bundle 安装机制。

### 范围

- `skills.sh` 只负责搜索，结果解析为受支持的 canonical Source；GitHub 结果继续使用阶段 5 的 Adapter。
- 确定性下载 URL 只接受直接可获取内容，不抓取普通网页、社交媒体或论坛页面。
- 支持 ZIP、`.skill` 和普通本地目录输入。
- ZIP、`.skill` 和直接文件 URL 保存为需要手动提供新内容的 Source，不依赖 HTTP metadata 自动判断更新。
- 用户可以明确把个人目录或本地 Git clone 登记为 Editable Local Source。
- Editable Local Source 原目录继续由用户拥有，Host 仍只使用 Central Store 副本。
- 所有入口复用成员发现、Safe Skill Content、资源限制、Plan、事务和“已安装、未挂载”结果。
- 非 GitHub 输入格式和 canonical identity 形成封闭支持清单，未列出的格式不在实现中自动扩张。

### 1.0 封闭输入协议

- `skills.sh` 使用其当前公开页面调用的 `GET https://skills.sh/api/search?q=<query>` JSON 协议，只读取 `source`、`skillId`、`name` 和 `installs`。结果按 `source` 分组；只有能严格解析为 GitHub `owner/repository` 的分组可以继续进入阶段 5，域名型或其他结果只显示为不支持。它不创建 `skills_sh` Source，不调用 `npx skills`，端点失败也不改写本地 Source 或 Bundle。
- `.skill` 按 cc-switch 已核验行为定义为 ZIP 容器的扩展名别名，不是单文件 Markdown 或新的 package 格式。ZIP 与 `.skill` 都只支持 Stored 或 Deflate 条目，并复用同一个归档安全实现。
- 本地归档只接受扩展名为 `.zip` 或 `.skill` 的普通文件。canonical identity 使用规范化绝对路径；同一路径重复导入复用 Source，不依据内容相同自动合并不同路径。Source 另存最近采用的 artifact digest，因此后续手动替换内容不会改变 Source identity。
- 直接 URL 只接受无 user-info、无 fragment、路径以 `.zip` 或 `.skill` 结尾的 HTTPS URL。query 作为资源定位的一部分保留；最多跟随五次 HTTPS 重定向，非 GitHub 请求只能留在原 host。最终响应必须仍满足该边界并能被 ZIP parser 验证。canonical identity 使用规范化后的完整 URL。
- 普通本地目录继续使用阶段 2 已有的一次性快照入口，不创建 Source。只有用户明确选择 Editable Local Source 时才登记来源；其 identity 由登记时目录的 filesystem identity 产生，路径和最近采用的内容指纹独立保存，后续移动路径必须走同一 Source 的明确重新关联流程。
- 普通归档允许两种根布局：全部条目位于唯一 wrapper 目录时剥离 wrapper；否则保留归档根。因此根级 `SKILL.md`、一个带外层目录的 Skill，以及多个顶层 Skill 目录都能进入同一成员发现逻辑。GitHub archive 仍强制唯一 wrapper。
- 1.0 不再声明其他“官方 index／manifest”格式；只有上面的 `skills.sh` 映射属于已支持的远程发现协议。新增 manifest 必须在后续版本明确加入封闭清单，不能由运行时猜测。

### 文件职责与切片

- `skills_sh.rs` 只把搜索响应整理成 canonical GitHub Source 分组；它不保存 Source，也不参与安装事务。
- `source_archive.rs` 是 ZIP、`.skill`、GitHub archive 和直接 URL 下载内容唯一共用的安全展开实现；GitHub 强制 wrapper，其他归档允许可选 wrapper。
- `source_input.rs` 只把本地归档、直接 URL 和 Editable Local 目录转换为经过验证的临时内容快照与 canonical Source identity；它不保存 Bundle，也不执行生命周期事务。
- `domain.rs` / `storage.rs` 原地扩充现有 Source kind、locator、内容指纹和 Install Plan 输入，不建立第二套 Source 或安装表。
- `application.rs` 只把 typed intent 接入正式应用写入门；`lifecycle.rs` 负责把所有来源快照转换成同一套成员候选，并继续作为唯一的 Bundle `current`、Journal 和恢复执行器。
- typed Tauri commands / TypeScript client 分别提供搜索、原生归档选择器、直接 URL 和显式 Editable Local 入口；四者最终进入同一张安装确认页。

本阶段依次交付四个切片：先完成无持久化副作用的 `skills.sh` 搜索；再提取并回归唯一归档安全核心；随后以同一 Source/Plan/Journal 完成本地归档、直接 URL 和 Editable Local 安装；最后补齐 typed IPC、原生选择器和统一 UI。每个安装入口都必须从公开 seam 证明确认前零 Bundle 写入、确认后生成同构且未挂载的 Bundle，并在重启后保持一致。

### 不包含

- 给已接管 Bundle 补充 Source、Bundle 归并和真正的更新执行。

### 阶段验收

- `skills.sh` 结果不会形成第二套更新规则或生命周期管理方。
- 普通本地目录保持一次性快照，只有显式选择才成为 Editable Local Source。
- 归档逃逸、链接条目和资源超限在写入 Current Content 前失败。
- 各入口安装产生相同的 Bundle、Member Selection 和未挂载状态。

## 阶段 7：补充来源与 Bundle 归并

### 目标

让无 Source Bundle 获得可解释的更新来源，同时维持 Source 与 Bundle 的一对一关系。

### 范围

- 用户为已接管 Bundle 选择现有或新 Source。
- 对每个本地 Skill 只提供“对应”和“不对应”；选择“对应”时指定一个当前 Source Member。
- 关联本身不替换 Current Content、不改变 Mount，也不执行上游更新。
- Source 没有关联 Bundle 时直接建立唯一关联。
- Source 已关联另一个 Bundle 时必须展示 Bundle Merge Plan，不能建立第二条关系。
- 归并计划列出两个 Bundle 的全部成员、Mount、重复身份、内容选择和路径冲突。
- 内容冲突由用户选择唯一内容；所有 Mount 最终统一使用选择后的完整候选 Bundle。
- 成功后清理已经为空的原 Bundle；未选择内容不保存为版本或回滚点。
- GitHub Source 与来源未知 Bundle 关联后显示“可更新”，不猜测历史 commit。

### 完整技术实现图

Stage 7 只建立一套 `SourceAssociationPlan`。`link` 与 `merge` 是同一份封存计划根据 Source 当前关系得到的两种 mode，不能分别建立两套 Plan 或确认入口：

```text
选择无 Source Bundle + 当前 Fresh Source + 对应／不对应
  -> SourceAssociationPlan（封存 Source、Bundle、成员、Mount 与用户映射）
  -> link：一个 SQLite 事务建立 Source、Bundle 和可选成员映射
  -> merge：一个 SourceAssociationTransaction + Journal
       -> 从两个本地 current 准备完整候选
       -> 原子切换 Source 已关联 Bundle 的唯一 current
       -> 校正待归入 Bundle 的 Mount
       -> 一个 SQLite 事务提交最终成员、Mount、映射和 Bundle 关系
       -> 清理已经为空的原 Managed Bundle Directory
```

模型和边界固定如下：

- `SourceAssociationPlan` 同时覆盖直接关联和归并；创建时保存当前 Catalog generation／marker、两个 Bundle 的 `current`、全部成员、Mount、对应选择、内容冲突和阻塞冲突，确认只能在计划已列出的内容候选中选择。
- `source_member_links` 只保存“对应”。本地成员选择“不对应”时不写任何原因或替代状态；Source 安装读取模型必须允许 Bundle 成员没有 Source Member 映射。
- Source 必须拥有 Fresh Catalog，所选 Source Member 必须是当前可安装成员，同一个 Source Member 最多对应一个最终本地成员。不通过名称、路径、描述或内容相似度自动建立对应。
- Source 尚未关联 Bundle 时，直接关联不修改 `current`、内容或 Mount，也不创建文件系统事务；GitHub 的 adopted marker 保存为空，表示第一次完整更新尚未发生。
- Source 已关联另一个 Bundle 时，以该 Bundle 为保留目标。归并只组合两个本地 Bundle 的当前受管内容，不获取 Source、不安装 Catalog 中其他成员，也不推进已有 adopted marker。
- 同名成员或映射到同一 Source Member 的常见一对一冲突由用户选择唯一内容；所有 Mount 最终使用该成员。未选择内容不保留为 Revision。一个成员同时卷入多组交叉冲突，或归并后同一成员出现无法同时成立的 global／project Mount scope 时，计划列为阻塞冲突，1.0 要求用户先处理后重新生成，不增加冲突编辑器。
- Merge 的唯一文件系统生效点是目标 Bundle `current` 的原子替换。生效前中断清理候选并保留两个 Bundle；生效后中断继续完成 Mount、SQLite 和目录清理。未知 `current`、Mount 被外部替换、目录身份变化或权限异常进入 blocked recovery。

Stage 7 新增 `source_association.rs` 作为唯一关联与归并编排器；`domain.rs` 保存公开 Plan／选择 DTO，`storage.rs` 保存同一 Plan、可选归并事务和最终领域提交，`content.rs` 与 `mount_lifecycle.rs` 只提供现有安全文件和 Mount 原语，`application.rs` 与 typed IPC 继续作为薄入口。不能把归并塞进 Install Plan，也不能先调用安装事务再调用 Batch Mount 事务。

本阶段按三个纵向切片实施：先让“对应／不对应”的直接关联穿过公开应用 seam，并把 Source 安装读取器原地改为允许无映射成员；再用同一 Plan 完成归并、单一 Journal 和重启恢复；最后补齐 typed IPC、关联／冲突确认界面和失败后重读。每个切片保持可编译、可回归并独立提交。

### 不包含

- Update Check 和任何自动或隐式上游内容采用。

### 阶段验收

- Source 与 Bundle 的数据库约束和业务测试都保证可选一对一。
- “对应／不对应”选择不会修改当前文件或 Mount。
- Merge 在确认前零写入，成功后只剩一个受管 Bundle 和一组有效 Mount。
- Merge 在各持久化阶段重启恢复时保持幂等。

## 阶段 8：Bundle 更新

### 目标

提供用户主动触发、整个 Bundle 原子生效且不引入本地版本历史的更新能力。

### 范围

- Update Check 只在用户点击时运行，不后台轮询或自动检查。
- GitHub 只比较同一 Tracked Ref 的已采用 commit SHA 与当前 SHA；任意新 commit 都表示可更新。
- 查询失败记录“无法检查”、时间和错误摘要，保留上次成功结果和全部本地内容。
- 没有 Source 的 Bundle 不显示更新入口。
- 用户确认更新后获取 Source 的完整当前内容，并安装全部通过验证的当前成员，不提供成员排除。
- 即使 Bundle 此前只安装部分成员，完整更新也采用 Source 当前全部有效成员。
- 新增成员进入 Bundle 后保持未挂载。
- 上游已移除和明确“不对应”的既有成员继续保留。
- 在临时区准备完整候选 Bundle，验证后只原子切换一次 `current`；现有 Mount 不改写。
- 只有成功更新后才保存新的 commit 或内容基线。
- ZIP、`.skill` 和直接 URL 使用“导入新内容”；Editable Local Source 使用主动“检查本地改动”和确认采用。
- Batch Update 顺序执行每个 Bundle 的独立事务；普通失败不回滚其他成功 Bundle，并继续后续 Bundle。
- 更新结果保持简洁，不提供文件 diff、自动 changelog、本地版本或回滚。

### 完整技术实现图

Stage 8 沿用唯一的 `InstallPlan(mode=update)` 和 `lifecycle_transactions(kind=install_bundle)`。单个 Bundle 的 GitHub、手动归档和 Editable Local 更新只在候选内容如何进入临时区上不同；确认后都执行同一条 Bundle `current` 原子切换协议：

```text
用户点击检查
  -> GitHub：只解析 tracked ref 当前 commit
  -> Editable Local：只读取已登记目录并生成临时候选快照
  -> 保存检查状态，不修改 current 或 Mount

用户发起单 Bundle 更新
  -> 完整获取或选择替换内容
  -> InstallPlan(mode=update，封存 current、Source marker 和全部候选)
  -> lifecycle transaction + Install Journal
  -> 发布完整 Bundle 候选
  -> 原子替换一次 current
  -> 一个 SQLite 事务提交成员选择、Catalog／采用标识和更新状态
  -> 清理旧内容、临时区和 Journal
```

模型和边界固定如下：

- `0017_bundle_update_checks.sql` 只保存每个 Source-backed Bundle 的检查状态、检查 marker、时间和错误摘要。GitHub 查询失败保留上次成功 marker；没有 Source、手动替换来源和未主动检查的 Editable Local 不伪装成“已是最新”。
- `0018_bundle_update.sql` 原地扩展既有 Install Plan，增加 `update` mode、预期 Source marker 和旧内容 fingerprint；不建立第二套更新事务或本地版本。更新确认必须带入全部可更新候选，成员级排除在公开入口和领域校验中都被拒绝。
- GitHub Update 获取当前完整 Source；Archive／Direct URL Update 只使用用户本次选择的本地替换文件，不后台重新访问原 URL；Editable Local 先主动检查已登记目录，只有检测到变化并再次确认才采用。
- 候选中的新成员进入同一个完整 `current` 但不自动 Mount。上游已移除或明确“不对应”的既有成员从旧 `current` 保留；成功后现有 Mount 继续指向不变的成员路径。
- 生效点前中断保留旧 `current` 并清理候选；生效点后中断采用新 `current` 并继续提交成员、marker、notice 和清理。只有 `current`、路径身份或 Journal 证据无法安全判断时才把相关 Bundle 标记为 blocked。

“全部更新”只新增一个持久化的 `BundleUpdateBatch` 协调器：

```text
显式处于 Available 的 GitHub／Editable Local Bundle
  -> 逐个准备普通 InstallPlan(mode=update)
  -> 汇总页只选择 Bundle，并按页面顺序确认
  -> 逐个调用同一条单 Bundle 更新事务
  -> ordinary failure：记录失败并继续
  -> blocked child：停止，剩余项记为 NotExecuted
  -> completed／blocked 结果持久化，重启后继续展示
```

`0019_bundle_update_batches.sql` 只记录参与 Bundle、顺序、child Plan 和结果，不保存第二份候选内容，也不形成跨 Bundle Journal 或回滚。启动时先恢复普通 child transaction，再依据 blocked child、已采用 marker 或仍可执行 Plan 幂等归并批次状态；未执行 child 的临时 Plan 会被清理。

### 阶段验收

- Update Check 不下载 archive，不改变 Current Content。
- 完整候选任一成员验证失败时，整个 Bundle 保持原状。
- `current` 切换的并发读取只能观察到完整旧树或完整新树。
- 切换前中断保留旧内容；切换后中断保留新内容并完成记录与清理。
- Batch Update 的成功、失败、未执行和人工恢复阻塞范围准确。

## 阶段 9：移除与删除

### 目标

让停止使用、移除管理关系和永久删除内容具有互不混淆的操作语义。

### 范围

- 移除 Mount 只删除一个使用位置，保留 Bundle、Skill 和其他 Mount。
- 移除 Project 时先事务性移除其中全部 SkillYard-managed project Mount，再删除 Project 记录。
- 删除 Source 使用普通确认，删除 Catalog、检查结果、更新标识和 Source-to-Bundle 关联；本地 Bundle 继续显示“由 SkillYard 管理”和“没有更新来源”。
- 删除 Source 或 Bundle 都不能删除 Editable Local Source 原目录、Agent-managed 内容或 Project-managed 内容。
- 删除 Bundle 使用完整影响预览和第二次危险确认。
- Cascading Delete 删除全部 managed Mount、Member Selection、Current Content、Managed Bundle Directory 和 Bundle 记录，同时保留独立 Source。
- Cascading Delete 的 Journal 明确记录破坏性生效点；生效前恢复原状态，生效后持续完成删除意图。
- 1.0 不提供成员级卸载或删除，也不提供删除后的版本回滚。

### 完整技术实现图

Stage 9 只增加一份 `RemovalPlan`。Project、Source 与 Bundle 共用影响预览、过期校验和不可变摘要；确认后的实现按真实副作用分成两类，而不是建立三套删除模型：

```text
Project Remove
  -> RemovalPlan 封存 Project 与全部 managed project Mount
  -> removal transaction + Removal Journal
  -> 复用既有 Mount 隔离／恢复能力
  -> 一个 SQLite 事务删除 Mount 与 Project 记录
  -> 清理隔离项、Journal 和事务记录

Source Delete
  -> RemovalPlan 封存 Source 与受影响 Bundle
  -> 普通确认
  -> 一个 SQLite 事务删除 Source、Catalog、检查状态和更新关联
  -> Bundle、current、Mount 与 Editable Local 原目录保持不变

Bundle Cascading Delete
  -> RemovalPlan 封存全部 Member、Mount、Source 保留项和受管目录身份
  -> 前端两次明确确认，同一个 opaque planId
  -> removal transaction + Removal Journal
  -> 隔离全部 managed Mount
  -> 原子 rename：bundles/<bundleId> -> trash/<transaction-owned-name>
  -> 一个 SQLite 事务删除 Mount、Member Selection 与 Bundle 记录
  -> 安全清理 Trash、Mount 隔离项、Journal 和事务记录
```

模型、持久化和恢复方向固定如下：

- `0020_removals.sql` 保存一份带 SHA-256 摘要的 `removal_plans`，以及 Project／Bundle 共用的 `removal_transactions`。Source 没有跨文件系统破坏性状态，只使用同一 Plan 和一个 SQLite 事务，不伪造 Journal。
- `removal.rs` 是唯一协调器；`mount_lifecycle.rs` 只暴露既有 Mount 删除协议所需的封存、隔离、检查、恢复和清理能力；`storage.rs` 只负责 Plan、阶段和最终领域提交。
- SQLite 事务行先以 `journal_pending` 写入，Journal 持久化后才进入 `journal_ready`，避免数据库行与 Journal 之间的崩溃间隙被误判为已生效。
- Project 在 Journal 就绪前中断时保持原状态；全部 Mount 隔离后中断时继续完成 Project 移除。没有 Mount 的 Project 仍走同一协议并可正常删除登记记录。
- Bundle 的唯一破坏性生效点是受管目录原子进入 Central Store 的 `trash/`。该点之前中断会恢复 Mount 并保留 Bundle；该点之后中断会继续删除领域状态和受管内容，不向用户提供回滚选择。
- Trash 只按封存的目录身份和受管树清单清理，不使用路径递归删除未知内容。Mount、Bundle 或 Trash 身份被外部改写时，相关事务进入 blocked；已经提交后发生的清理异常同样保持可见，不影响其他 Bundle。
- 确认前会再次核对 Project、Source、Bundle、Source 关联和完整 Mount 集合。预览后新增或改变的 Mount 会让旧 Plan 失效，不能先执行部分删除。
- `RemovalPlanPage` 负责三种影响预览。Project 入口始终位于 Inventory；Source 使用普通确认；Bundle 的第一次点击只进入危险确认状态，第二次点击才调用后端。1.0 不提供成员删除入口。

### 阶段验收

- 四种操作的确认文案、影响范围和实际文件结果互不混淆。
- 移除最后一个 Mount 后 Skill 仍可重新挂载。
- 删除 Source 后全部本地内容和 Mount 保持可用。
- Cascading Delete 在破坏性生效点前后中断时都恢复到可解释终态。
- 权限异常或外部路径改写导致无法安全继续时进入人工恢复，不能报告部分成功。

## 阶段 10：1.0 完整验收

### 目标

证明前九个阶段共同构成一个可在当前个人 Mac 上长期使用的 SkillYard 1.0，而不是一组分别通过的功能演示。

### 范围

- 对安装、Mount、Takeover、Bundle Merge、Update、Project Remove 和 Cascading Delete 补齐完整 failpoint 与子进程强制终止矩阵。
- 完成人工恢复页面，只处理内容缺失、路径被外部改写、权限异常或证据不足等无法自动判断的状态。
- 验证应用级单写门、确认后不可取消、事务期间只读浏览，以及 Refresh、Update Check 和 Source Reload 暂停规则。
- 完成 Local Refresh、Mount 健康检查、冲突和 Drift 的跨流程验证。
- 验证 `SKILLYARD-INFO.md` 随 Source 与 Mount 关系变更保持同步。
- 验证 App Reset 只清除偏好、窗口状态和缓存，不删除生命周期数据。
- 在当前个人电脑上验证 Codex、Claude Code 和 GitHub Copilot 的实际路径与交叉可见性。
- 构建并启动仅面向 `arm64`、macOS 14 及以上的本地 `SkillYard.app`。
- 验证重新本地构建并手动替换 `.app` 后，原 Central Store、SQLite、Journal、`current` 和 Mount 仍被识别。
- 验证当前工作区没有重新引入旧 Python CLI、Local Server、HTML、旧 SQLite schema 或旧测试。
- 按 PRD 的全部 167 条 User Story 建立实现或验收映射，并完成最终回归。
- `[HUMAN]` 审阅关键危险确认、人工恢复页面和跨应用可见性提示是否容易理解。
- `[HUMAN]` 使用一份真实 Bundle 完成最终日常流程体验，不以这一步替代任何自动化正确性断言。

### 完整技术实现图

Stage 10 只补齐九个阶段组合使用后的产品缺口，不新增第二套生命周期状态机、进度协议或恢复协议。

```text
Editable Local 重新关联
  -> 原生目录选择器取得候选路径
  -> 只读识别同一 device/inode 与候选 Skill 内容
  -> 持久化 metadata-only Relink Plan
  -> 用户确认
  -> 一个 SQLite 事务更新 Source locator、展示名和检查状态
  -> current、Member、Mount 与受管内容保持不变

生命周期确认
  -> 继续调用前九阶段唯一的确认入口
  -> 前端从现有确认 Promise 推导“当前操作”
  -> 默认显示不可取消的操作页
  -> 用户可切换到最近一次已提交 Inventory 只读浏览
  -> 全局提示可返回当前操作；全部写入口和主动刷新保持不可执行

阻塞恢复
  -> Inventory 中的 RecoveryIssue 进入专用只读页面
  -> 展示相关对象、Rust 判定原因和 Central Store 保护提示
  -> 只提供返回清单与打开固定 Central Store
  -> 不提供未定义的强制采用、删除 Journal 或解除阻塞动作

Installation Chain
  -> Local Refresh 只读解析官方默认位置或 XDG state 位置的 lock v3
  -> 只接受完整的 v3 记录，并按协议中的 Skill Name 关联扫描观察
  -> 全局 lock 只关联全局或共享目录，不能附给项目目录中的同名 Skill
  -> Inventory 与 Takeover Plan 展示 lock、Source URL／路径和上游 Skill 路径
  -> 不根据 lock 猜测具体由 `skills`、`gh skill` 或 Lark CLI 执行
  -> 接管确认时随 Skill Member 原子保存，之后不受外部 lock 删除影响
```

模型和边界固定如下：

- Editable Local Relink 只支持同一台 Mac、同一文件系统内保留 device/inode 的移动或重命名。跨文件系统复制、重新创建目录以及仅名称或内容相似的路径在 1.0 中拒绝，不能自动认作同一 Source。
- Relink Plan 封存 Source ID、旧路径、候选规范路径、文件系统身份、候选内容 marker 和可展示成员；确认时重新检查同一事实。它只修改 Source metadata，并把关联 Bundle 标为“尚未检查”；采用候选内容仍需之后单独执行 Editable Local Check 与完整 Bundle Update。
- Relink 使用独立的持久化 Plan 表和应用级写入门，但不创建 Lifecycle Transaction 或 Filesystem Journal，因为确认不修改 Central Store 内容、Mount 或项目路径。
- “当前操作”不保存百分比、阶段或预计时间。应用只使用已有确认调用的开始与结束状态；后端单写门和现有 Journal 仍是唯一并发与恢复依据。
- 生命周期事务期间允许使用缓存的已提交 Inventory 进行搜索、筛选和查看 Mount／Source 详情；不能发起安装、接管、挂载、更新、删除、Local Refresh、Update Check 或 Source Reload。
- Finder 入口只调用固定的 `open_central_store` command；Rust 使用 Tauri Opener 打开 `ApplicationPaths::data_root()`，前端不能提供路径，也不获得通用文件系统或 shell 权限。
- 设置页收纳“重置应用”和“打开 Central Store”，主界面不常驻展示这两个低频入口。“重置应用”只清除当前前端导航、搜索筛选、临时错误和窗口内选择，并重新读取 Startup State。1.0 当前没有持久化偏好或窗口状态，因此不新增设置存储；SQLite、Journal、Source Catalog、Bundle、`current` 和 Mount 均不能被清理。
- Installation Chain 只有一套当前模型：扫描观察、Takeover Plan 和受管 Skill Member 使用同一份 lock v3 事实。Source 关联仍由现有 Source 模型负责，不能把 lock 中的来源字符串直接伪装成已经登记且可更新的 Source。
- lock v3 只是一份可核验的本地收据协议，不是执行者证明。1.0 保存记录位置、Source 类型、Source URL／路径、上游 Skill 路径、ref、内容 marker 与时间；不解析 Lark 专属状态、GitHub frontmatter 或其他 receipt 格式。
- Stage 10 建立 167 条 User Story 到自动化、`[MAC-CONTRACT]` 或 `[HUMAN]` 证据的映射。已有公开 seam 测试继续作为证据，只为真实缺口补测试，不重复实现前九阶段。

### 1.0 完成条件

- 十个阶段的阶段验收全部通过。
- 主要业务测试全部通过正式 `SkillYardApplication` seam，且使用真实 SQLite 与文件系统。
- Tauri UI、typed IPC 和 Rust Core 自动化验收全部通过。
- `[MAC-CONTRACT]` 本机 `.app` smoke、Supported App 路径、目标文件系统和应用替换 contract 全部通过。
- `[HUMAN]` 关键文案与最终日常使用体验已经由产品所有者确认。
- 当前工作区不再包含会让维护者误以为 Python 原型仍是产品组成的运行代码或测试。
- 没有把公开分发、应用更新、备份恢复、日志页面、版本历史、回滚或 CLI 偷带入 1.0。

## 必须尽早验证的技术契约

这些验证不是独立产品阶段，应分别在首次依赖它们的阶段完成：

| 最晚阶段 | 技术契约 |
| --- | --- |
| 阶段 1 | `[MAC-CONTRACT]` 最小 Tauri `.app` 在当前 Mac 以 `arm64` 启动，加载内嵌资源且无 localhost/Python sidecar |
| 阶段 1 | `SkillYardApplication` 与 Tauri typed command 的输入、输出和错误映射一致 |
| 阶段 1 | `[AUTO]` 三个 Supported App 的固定路径、共享只读目录和当前安装检测证据形成封闭配置 |
| 阶段 2 | `[AUTO]` Bundle `current` 在临时文件系统上可以原子替换，并发读取只看到完整旧树或新树；目标 APFS 另做 `[MAC-CONTRACT]` |
| 阶段 2 | SQLite、Journal、文件系统生效点和幂等启动恢复形成可重复的 failpoint 矩阵 |
| 阶段 2 | 应用级单写门和恢复优先启动顺序能够阻止竞争事务 |
| 阶段 10 | `[MAC-CONTRACT]` 自动替换两个本地 `.app` 构建后，持久用户内容保持不变 |

## 旧 Python 原型清理边界

旧 Python 实现已经在 1.0 开始实施前从当前工作区删除：

- Git 历史负责保留原型，不在工作区维持两套产品实现。
- 可以重建的只有 PRD 已记录的真实临时 SQLite／文件系统测试方式，以及 Plan 零写入、冲突前置拒绝、软链接 Drift、多应用使用关系等仍然有效的场景。
- AI Assist、Captured Install、外部命令执行、copy fallback、自动改名、Cursor、旧更新模型和日志行为不迁移。
- 实施过程中不能恢复旧代码作为 runtime、测试 oracle 或临时 sidecar；新的验收直接针对 Rust `SkillYardApplication` seam 编写。

## Issue 拆分规则

本计划审阅通过后再创建实施 Issue。拆分时遵守以下规则：

- Issue 以可观察的纵向结果为单位，不创建只有“数据库层”“文件系统层”或“前端层”的长期横向任务。
- 每个 Issue 明确对应阶段、用户行为、允许范围、非目标、依赖和可执行验证命令。
- 高保证写操作的 Plan、Journal、恢复和失败测试必须与该操作位于同一 Issue 或同一不可分割的依赖链中。
- 一个阶段可以拆为多个连续 Issue，但阶段验收没有通过前不能宣称该阶段完成。
- 不得为了复用旧测试而恢复 Python 运行面；所需验收证据以 PRD 和本计划记录为准。
- 实施 Issue 只来自本计划和 PRD，不从已删除 ADR 或旧 Python 行为反推新需求。

## 尚未锁定的实现选择

以下内容不改变产品行为，但应在首次使用前通过当前官方资料和本机验证确定：

- TypeScript UI 的具体 component framework、构建工具和测试工具；
- Rust SQLite library 与 migration 执行方式；
- TypeScript／Rust 类型共享或 contract generation 方式；
- SQLite 表、Journal 文件和内部错误类型的具体结构；
- Tauri 2 依赖的精确版本与本机构建命令。

这些选择应优先采用最小、直接、可测试的方案，不为未进入 1.0 的平台、分发方式或扩展能力建立抽象。
