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
- 主界面以 Skill 为中心展示结果，并按能够确定的证据区分四种管理状态。
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

把扫描发现的待接管 Skill 安全迁入 Central Store，同时保持用户已有使用关系。

### 重做实施边界

Stage 4 在提交 `9abc7d0` 后从 Stage 3 已验收基线重新实现。此前同一阶段内先建立单路径 Takeover、再并行增加 `v2` 多路径协议，造成两套 Plan、Transaction、Journal 和恢复入口；该实现已经完整撤销，不能作为新实现的兼容前提或代码基础。

新实现必须遵守以下边界：

- 只有一套不带版本后缀的 `TakeoverPlan`、`TakeoverTransaction`、Filesystem Transaction Journal 和生产确认入口。
- 一个 Plan 表达一个待接管 Skill Identity、一个最终 Bundle Member、一个被用户选中的内容副本、多个原始位置和多个最终 Mount；单副本、重复副本、scope 冲突与共享目录都是同一模型的不同输入，不建立特殊的第二套事务。
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

实现只允许按以下纵向切片推进，每片先出现公开 seam 的失败测试，再写最少实现，并独立提交、推送：

1. 单一 Takeover Plan：单副本、多副本显式身份确认、唯一内容选择和零文件修改。
2. 单副本确认：进入一个新 Bundle，并保留或排除已有 Mount。
3. 多副本确认：所有位置统一使用唯一内容，未选内容不形成历史版本。
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
- 按 PRD 的全部 168 条 User Story 建立实现或验收映射，并完成最终回归。
- `[HUMAN]` 审阅关键危险确认、人工恢复页面和跨应用可见性提示是否容易理解。
- `[HUMAN]` 使用一份真实 Bundle 完成最终日常流程体验，不以这一步替代任何自动化正确性断言。

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
