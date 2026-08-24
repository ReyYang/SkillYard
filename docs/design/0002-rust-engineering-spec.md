# SkillYard Rust 工程规格：深模块、测试拓扑与 Cargo 产物治理

> 状态：已确认，待实施
>
> 适用范围：当前 1.1.0 收尾阶段及后续 Rust 工程演进
>
> 产品边界：本规格只改善 Rust 开发反馈、模块职责、跨语言协议治理和验证方式，不改变 SkillYard 1.0 产品契约或 1.1.0 产品能力。
>
> 实施顺序：先完成不改变产品语义的 Train A；完成 1.1.0 产品验收后，再按证据进入 Train B。

## Problem Statement

SkillYard 的最终 macOS 应用体积很小，但本地 Cargo `target` 已增长到约 `12 GB`。其中 `debug` 约占 `11 GB`、`debug/deps` 约占 `7.2 GB`、`debug/incremental` 约占 `3.3 GB`，Rust 测试可执行产物合计约占 `1.7 GB`。开发者需要承担明显高于最终产品规模的磁盘、编译和链接成本。

当前普通 Rust 集成测试由多个顶层测试文件组成。Cargo 会把每个顶层集成测试文件作为独立 crate 和测试可执行文件编译，导致同一套 Lifecycle Core、Tauri、SQLite、HTTP、压缩和序列化依赖被重复链接。完整调试信息、增量对象、debug 与 release 产物同时存在，进一步放大开发产物。

Rust 代码仍属于一个 canonical crate，这与 SkillYard 的单一领域模型和单一生命周期协议一致；问题不在于 crate 数量少，而在于 crate 内若干 Module 的 Interface 不够深、职责和依赖方向不够清晰。持久化、生命周期协调、底层安全文件系统操作、Source 解析、Agent Provider 和 Tauri Adapter 之间存在不必要的知识扩散，使局部修改更难局部理解、验证和审查。

Rust 与 TypeScript 之间的跨边界类型和命令存在手工镜像。新增或修改一个应用行为时，开发者需要同时维护应用 Intent、Tauri command 参数、Rust outcome 和 TypeScript 类型，编译器无法完整证明两端协议一致。

仓库已经拥有严格的 Product Contract、canonical Domain、Filesystem Transaction Journal 和公开行为验收，但开发命令、验证阶梯、迁移不可变规则和模块增长约束尚未全部成为机器可执行规则。开发者和编码 Agent 容易选择过大的验证范围，或者只靠清理 `target` 暂时释放空间，却没有改善下一次迭代的反馈成本。

SkillYard 需要借鉴成熟 Rust 项目的工程原则，但不能照搬其产品架构。任何治理都必须保护 Single Application Surface、Lifecycle Core、Current Content、Mount、Takeover、Local Lifecycle Authority 和已发布迁移，不能为编译优化建立第二套领域模型、测试实现或生命周期协议。

## Solution

SkillYard 保持一个 canonical Rust crate、一个 Domain、一个 Persistence 协议和每类产品行为唯一的 Plan、Filesystem Transaction Journal 与恢复方向。在这个边界内，把现有实现逐步整理为具有小而稳定 Interface、较深 Implementation 和明确依赖方向的私有 Module。

第一阶段先治理直接造成开发产物膨胀和验证低效的结构：将普通集成测试聚合为一个测试 target，将依赖真实本机 Codex CLI 的 macOS 契约测试保留为独立 ignored target；为开发和测试 profile 使用有限调试信息；保留本地 incremental；增加只读产物报告、统一命令入口、迁移不可变检查和分层验证。第一阶段不得改变产品语义、数据库 Schema、Tauri wire 或生命周期行为。

完成 1.1.0 产品验收后，再按真实职责和现有调用关系重构 Lifecycle Core。`SkillYardApplication` 继续作为最高应用行为 Seam；Persistence 继续使用一个具体 `Storage`；共享底层文件系统安全原语从 Install 协调中分离，但不建立通用 Journal；Agent 继续是只读智能层；Source Transport、Provider 和 Secret Store 只在真实外部系统边界建立薄 Adapter。

Rust 与 TypeScript 的跨边界协议使用生成的 canonical TypeScript 类型，并通过 committed artifact、协议 fixture 和漂移检查保持一致。Tauri 层继续提供 typed、task-specific commands，并将每个 command 保持为进入 `SkillYardApplication` 的薄 Adapter；不能用一个 general-purpose IPC command 取代现有产品边界。流式 Agent、原生文件选择和受控 opener 继续使用各自的专用 Adapter。任何 command contract 变化都必须在一个迁移切片内原地替换，不能长期保留平行入口。

所有实施按可独立验收和回退的 tranche 进行。行数只触发职责审查，不作为机械拆文件条件；多 crate、额外 profile、自动清理、动态 Provider Registry 和泛化 Repository 均不进入本规格。

## User Stories

### 开发反馈与 Cargo 产物

1. 作为 SkillYard 开发者，我希望普通 Rust 测试不再为每个测试文件重复链接完整依赖图，从而减少日常测试产生的磁盘和链接成本。
2. 作为 SkillYard 开发者，我希望 `target` 的主要组成可以被只读报告，从而知道空间来自测试可执行文件、增量对象、debug 还是 release 产物。
3. 作为 SkillYard 开发者，我希望日常开发保留增量编译，从而不会为了减少磁盘而牺牲每次修改后的反馈速度。
4. 作为 SkillYard 开发者，我希望开发和测试只保留足以定位源码行与 backtrace 的调试信息，从而避免为普通迭代保存全部局部变量级信息。
5. 作为需要深度调试的开发者，我希望可以显式临时启用完整调试信息，从而不需要维护第二套长期 Cargo profile。
6. 作为 SkillYard 开发者，我希望局部行为验证不触发 production App build，从而只在阶段或发布验收时承担 release 构建成本。
7. 作为 SkillYard 开发者，我希望普通验证不无条件编译所有 feature 组合，从而避免产生与当前行为无关的 artifact 变体。
8. 作为 SkillYard 维护者，我希望 `target` 治理以可重复基准为依据，从而不会把一次清理后的空目录误认为问题已经解决。
9. 作为 SkillYard 维护者，我希望 clean isolated build 与 warm targeted test 都有基线，从而能同时判断磁盘收益和反馈时间回归。
10. 作为磁盘空间有限的开发者，我希望工程工具不会自动删除或重建全部 Cargo cache，从而可以自行决定何时承担完整重编译成本。
11. 作为 SkillYard 维护者，我希望 CI 的增量策略与本地开发策略分离，从而让短生命周期 CI 不保存低价值的增量对象，同时保留本地快速迭代。
12. 作为 SkillYard 维护者，我希望未来引入构建缓存或新 profile 前先有 Cargo timing 证据，从而不增加未经证明的基础设施和 artifact 组合。

### 测试拓扑与行为验收

13. 作为 SkillYard 开发者，我希望所有普通集成测试通过一个聚合入口运行，从而只生成一个普通集成测试可执行文件。
14. 作为 SkillYard 开发者，我希望仍能按 suite、module 或测试名称运行单个行为，从而不会因为聚合而失去定向测试能力。
15. 作为 SkillYard 维护者，我希望测试聚合前后的测试清单可以逐项比较，从而不会静默丢失测试或改变 ignored 状态。
16. 作为 Lifecycle Core 维护者，我希望 hard-exit worker 在测试聚合后仍真实执行，从而中断恢复测试不会以“零测试成功”形成假阳性。
17. 作为 Lifecycle Core 维护者，我希望 hard-exit 子进程的测试名称来自真实测试清单或统一 helper，从而不依赖容易漂移的短名称。
18. 作为 SkillYard 维护者，我希望依赖已安装 Codex CLI 的 macOS 契约测试与普通离线测试分开，从而不会让日常测试隐式依赖本机外部工具。
19. 作为 SkillYard 维护者，我希望普通测试不需要真实 Provider、API Key 或外部网络，从而保持可重复、无凭据且无费用。
20. 作为 SkillYard 维护者，我希望主行为测试继续从 `SkillYardApplication` 进入，从而验证真实产品 Seam 而不是私有步骤。
21. 作为 SkillYard 维护者，我希望跨前后端行为继续通过 typed Tauri client 验证，从而发现 Rust 与 TypeScript 协议错误。
22. 作为 Lifecycle Core 维护者，我希望测试继续使用真实临时文件系统和临时 SQLite，从而验证 Current Content、Mount、Journal 和恢复行为，而不是测试 mock。
23. 作为 Lifecycle Core 维护者，我希望重启恢复测试重新创建应用并回读持久状态，从而证明事务恢复不依赖进程内对象。
24. 作为 SkillYard 维护者，我希望单一聚合测试 binary 出现明显链接或内存回归时有两个聚合器的证据化回退方案，从而不必退回一文件一 executable。

### Module、Interface 与依赖方向

25. 作为 SkillYard 开发者，我希望每个 Module 拥有明确 invariant 和较小 Interface，从而修改者不需要理解整个 Lifecycle Core。
26. 作为 SkillYard 审查者，我希望模块拆分由职责和知识所有权驱动，从而不会为了满足行数目标制造只转发参数的浅模块。
27. 作为 SkillYard 审查者，我希望生产 Module 超过增长审查线时说明新增职责，从而及时发现错误抽象和依赖扩散。
28. 作为 SkillYard 维护者，我希望 `SkillYardApplication` 保持唯一应用级 Interface，从而所有支持的产品行为继续经过同一授权与错误边界。
29. 作为 SkillYard 维护者，我希望每个新 Intent 显式选择读写 gate 和平台模式，从而编译器能发现未声明授权边界的新行为。
30. 作为 SkillYard 开发者，我希望 Application dispatch 与具体用例 Implementation 分离，从而新增行为不会继续扩大一个不可局部审查的 match。
31. 作为 Lifecycle Core 维护者，我希望 Install、Mount、Takeover、Association 和 Removal 各自保留唯一 Plan、Journal 和恢复方向，从而不会出现通用框架掩盖产品语义。
32. 作为 Lifecycle Core 维护者，我希望共享文件系统安全原语由一个低层 Module 所有，从而多个生命周期不必依赖 Install 的 Implementation。
33. 作为安全审查者，我希望底层文件系统 Module 不知道 Bundle transaction、SQLite 或 Agent，从而危险操作只能根据已确认的上层计划执行。
34. 作为 Lifecycle Core 维护者，我希望 Mount 共享效果与 Mount transaction 分开，从而 Takeover、Association 和 Removal 可以复用安全效果，但不能绕过自己的 Journal。
35. 作为 SkillYard 维护者，我希望批量更新继续组合 canonical Install transaction，从而不建立第二套安装协议。
36. 作为 SkillYard 维护者，我希望 Removal 不调用 Takeover 的生产事务实现，从而 sibling lifecycle 只通过 canonical recovery 或阻塞查询共享事实。
37. 作为 SkillYard 维护者，我希望 Persistence 只依赖 Domain，从而 SQLite 查询不会调用 GitHub Adapter、Agent 或应用协调逻辑。
38. 作为 SkillYard 开发者，我希望 Source identity 的纯解析属于 Domain 或 Source 解析 Module，从而 Persistence 只保存已经确定的 canonical identity。
39. 作为 SkillYard 开发者，我希望 Content 验证、复制、指纹和预算各自拥有明确职责，从而资源限制与生命周期编排可以独立验证。
40. 作为 SkillYard 维护者，我希望只有在 Interface 稳定且 Cargo timing 证明收益时才拆新 crate，从而不会为了目录整洁提前公开内部边界。

### Persistence、迁移与数据安全

41. 作为 SkillYard 维护者，我希望 Persistence 继续使用一个具体 `Storage` 和一个连接入口，从而不会产生多套 Repository 或事务语义。
42. 作为 SkillYard 开发者，我希望 `Storage` 的 Implementation 可以按领域职责分开，从而偏好、Inventory、Source、Project 和生命周期状态可以局部维护。
43. 作为 SkillYard 维护者，我希望测试使用真实 SQLite，而不是内存 Repository trait，从而迁移、约束和事务行为与生产一致。
44. 作为已安装 1.0.1 的用户，我希望工程重构后现有数据库仍可原地升级，从而不丢失 Bundle、Source、Mount、Project 或 Current Content 关联。
45. 作为 SkillYard 维护者，我希望已发布 migration 的文件名、顺序和内容由 checksum 锁定，从而历史 Schema 不能被无意改写。
46. 作为 SkillYard 维护者，我希望未发布 migration 在版本发布时追加到已发布清单，从而锁定动作与真实发布边界一致。
47. 作为 SkillYard 维护者，我希望新 Schema 变化只能追加 migration，从而不在历史编号中插入或修改旧步骤。
48. 作为 Lifecycle Core 维护者，我希望从 1.0.1 快照执行升级和重启恢复验收，从而证明模块搬迁没有改变迁移顺序或业务真值。
49. 作为 SkillYard 用户，我希望工程治理不修改 Central Store、Current Link、Current Content、Filesystem Transaction Journal 或 Mount 的物理语义，从而应用升级后现有安装继续可用。
50. 作为 SkillYard 维护者，我希望 App Reset、Bundle 删除和 Source 删除的边界保持不变，从而工程重构不会扩大任何删除行为。

### Tauri wire 与 Adapter

51. 作为前端开发者，我希望跨 Tauri 边界的 TypeScript 类型从 Rust canonical 协议生成，从而不必手工同步同一 Intent 和 Outcome。
52. 作为 Rust 开发者，我希望只有真正跨边界的类型参与生成，从而 React view model、SQLite row 和私有 Journal 不会变成公共协议。
53. 作为 SkillYard 审查者，我希望生成的 TypeScript 文件提交到仓库并由漂移检查验证，从而协议变化在 diff 中可见。
54. 作为 SkillYard 开发者，我希望普通测试只检查生成结果而不修改工作区，从而运行测试不会产生未预期文件变化。
55. 作为 SkillYard 开发者，我希望更新 wire artifact 必须显式触发，从而生成变化可以与对应 Rust 决策一起审查。
56. 作为 SkillYard 维护者，我希望 tagged enum、optional 和 null 表示有 JSON fixture，从而生成类型与真实 Serde payload 一致。
57. 作为前端开发者，我希望普通业务动作继续通过 typed、task-specific command 进入同一个应用 Interface，从而保持明确调用意图且不复制业务逻辑。
58. 作为 SkillYard 用户，我希望 native file picker 和 folder picker 仍由受控专用 command 提供，从而前端不能提交任意本机路径。
59. 作为 SkillYard 用户，我希望 Agent streaming 和 cancel 保留适合 Channel 的专用入口，从而不会为了统一 command 而降低流式语义。
60. 作为 SkillYard 用户，我希望外部链接和路径仍由受控 opener 打开，从而 Markdown 或前端不能绕过协议限制。
61. 作为 SkillYard 维护者，我希望 command contract 变化在同一 tranche 内更新 Rust、生成类型和 TypeScript client，从而不维护 `legacy`、`v2` 或双生产入口。
62. 作为 SkillYard 维护者，我希望修改 Tauri Interface 前核验真实调用者，从而不会误删已经承诺的外部兼容面。

### Agent、Source 与外部 Adapter

63. 作为 SkillYard 用户，我希望 Agent 继续是只读智能层，从而工程重构不会授予它安装、接管、更新、挂载或删除能力。
64. 作为 SkillYard 维护者，我希望 OpenAI、GLM 和 DeepSeek 仍由固定 `ProviderKind` 枚举选择，从而不会扩张成任意 Provider Registry。
65. 作为 Agent 开发者，我希望三个真实 Provider 通过一个最小私有 Interface 归一流式和验证行为，从而 Provider 私有协议不进入 Application 或 React。
66. 作为 Agent 开发者，我希望 Keychain 通过 `SecretStore` Adapter 与测试内存实现隔离，从而测试不接触用户真实凭据。
67. 作为 Source 开发者，我希望远端获取通过 `SourceTransport` Adapter 与测试 Transport 隔离，从而普通测试不需要网络。
68. 作为 SkillYard 维护者，我希望文件系统和 SQLite 不建立对应 mock trait，从而测试继续覆盖真实本地语义。
69. 作为 SkillYard 用户，我希望 Provider 或 Source 失败只返回其所属行为的错误，从而不会改变 Inventory、Current Content、Mount 或 Local Lifecycle Authority。
70. 作为安全审查者，我希望 Agent、Source 和 Content 中的不可信数据不能改变 System 规则或生命周期授权，从而工程模块化不削弱现有隐私与权限边界。

### 命令、CI、Lint 与 Agent 指令

71. 作为 SkillYard 开发者，我希望 Cargo、pnpm 和 Tauri 的常用动作通过一个统一命令入口表达，从而本地、文档和 CI 不再维护不同命令语义。
72. 作为 SkillYard 维护者，我希望 CI 调用与开发者相同的验证 recipe，从而本地通过后不会因为命令漂移在 CI 意外失败。
73. 作为 SkillYard 维护者，我希望统一命令工具在 CI 中固定版本，从而工具更新不会静默改变构建行为。
74. 作为 SkillYard 开发者，我希望验证分为 targeted、slice、stage 和 release 四个层级，从而根据改动风险选择最小可信证据。
75. 作为 SkillYard 审查者，我希望 wire、migration、lifecycle、unsafe filesystem 和 Tauri 改动各自触发明确验证，从而高风险边界不会遗漏专项检查。
76. 作为 SkillYard 维护者，我希望 workspace lint 先以低误报规则建立基线并逐步收紧，从而不通过大规模机械修复制造错误抽象。
77. 作为测试作者，我希望 `unwrap` 和 `expect` 不被未经审查地全局禁止，从而 fixture 保持清晰，同时生产错误处理由现有规则约束。
78. 作为安全审查者，我希望危险文件系统和数据库连接方法有 canonical owner，从而新增直接调用能够被 lint 或架构检查发现。
79. 作为编码 Agent，我希望根指令只保存稳定产品与安全边界，从而普通任务不会加载大量易过时实现细节。
80. 作为编码 Agent，我希望进入 Rust 范围时读取专门的 Rust 工程指令，从而能选择 owning Module、公开 Seam 和正确验证层级。
81. 作为 SkillYard 维护者，我希望详细架构只保存在一份工程设计文档中，从而不会形成相互冲突的目录图和命令副本。
82. 作为 SkillYard 审查者，我希望每个 tranche 都有明确变更边界、验收和回退点，从而大型重构不会以一个不可审查的 diff 交付。

### 发布与用户稳定性

83. 作为 SkillYard 用户，我希望 Rust 工程治理不改变任何可见产品流程，从而升级后不需要重新学习安装、接管、挂载、更新或删除。
84. 作为 SkillYard 用户，我希望最终 App 仍然只有一个用户入口，从而不会出现 CLI、Daemon、localhost API 或独立恢复工具。
85. 作为 SkillYard 用户，我希望现有 Managed Lifecycle 在工程重构后继续由 SkillYard 唯一负责，从而 Source 可用性不会改变 Local Lifecycle Authority。
86. 作为 SkillYard 发布维护者，我希望 Train A 可以在 1.1.0 收尾期间实施，从而先改善开发反馈而不重新打开产品边界。
87. 作为 SkillYard 发布维护者，我希望 Train B 等 1.1.0 产品验收后再开始，从而功能增量和大规模结构搬迁不会相互污染。
88. 作为 SkillYard 发布维护者，我希望如果 Train B 在发布候选前执行，就重新运行完整 1.1.0 验收，从而旧候选证据不会覆盖新结构。
89. 作为 SkillYard 维护者，我希望任何第二套 Plan、Journal、恢复协议或公开产品 Seam 都触发架构变更门，从而实施 Agent 不能自行扩大范围。
90. 作为 SkillYard 维护者，我希望本规格最终拆成可独立提交和验证的 tickets，从而 `ready-for-agent` 不会被解释为一次性完成全部重构。

## Implementation Decisions

### 产品与发布边界

- 本规格不改变 SkillYard 1.0 产品契约和 1.1.0 产品规格；所有产品行为、危险操作确认、隐私边界和 Supported Platform 保持不变。
- `SkillYardApplication` 继续是唯一应用级 Interface。Tauri Adapter、测试和内部用例都不能建立第二个产品入口。
- 保持一个 canonical Rust crate、一个 Domain、一个具体 Persistence 实现，以及每类生命周期唯一的 Plan、Filesystem Transaction Journal 和恢复方向。
- 不在当前阶段拆分 workspace crate。未来 crate 拆分必须同时满足 Interface 稳定、依赖足够小、不复制 Domain、Cargo timing 证明收益和不增加生产协议等条件。
- 实施分为 Train A 与 Train B。Train A 只改变开发、测试、配置和治理；Train B 涉及代码职责重构，必须在 1.1.0 产品验收后进入，或在发布候选前重新执行完整验收。

### Cargo profile 与产物治理

- workspace 的开发和测试 profile 使用 `debug = "limited"`。该配置必须保留 panic backtrace、函数名和源码行号；完整变量级调试只通过一次性显式覆盖启用。
- 本地继续启用 Cargo incremental。CI 默认关闭 incremental，但不得据此改变本地策略。
- release profile、Tauri crate type 和 production App packaging 在 Train A 中保持不变。
- 普通验证不得默认使用所有 feature 组合。未来出现 feature 时，每个验证层级必须声明其真实所需集合。
- 不增加自动清理 recipe。新增只读产物报告，展示 debug、release、deps、incremental、测试 executable 数量、体积和主要大文件。
- 第一轮不引入 `sccache`、新的持久 profile 或常规多 target directory。只有基准证明收益后才能提出后续选择。

### 集成测试拓扑

- 所有普通集成测试聚合到一个 Cargo integration test target；原有行为按 module 组织，仍支持名称过滤。
- 依赖本机 Codex CLI 的 macOS 契约测试保留为独立 ignored target。普通测试必须离线、无凭据且无真实 Provider 费用。
- hard-exit worker 统一通过 child-process test support 解析并传入完全限定测试名。子进程验收必须确认测试实际运行、退出状态符合预期，并验证 Journal 与重启恢复结果。
- 迁移前后保存并比较测试清单，包括 ignored 状态。测试聚合不得删除、重命名遗漏或静默跳过原有行为。
- 首选一个普通聚合器。只有 warm targeted test 中位时间回归超过 `20%`、链接峰值内存不可接受或单 binary 重链接成为主要瓶颈时，才回退为两个普通聚合器。
- 测试 support 只复用应用初始化、临时文件系统、临时 SQLite、child process 和 fake external transport；不得把产品行为隐藏在通用成功 helper 中。

### Application、Protocol 与 Tauri Adapter

- Application 外部 Interface 保持 `handle`、Agent stream 和 Agent cancel 三类行为。内部 dispatch、startup、preferences、discovery 和 Agent coordination 按职责分开。
- 每个 Intent 必须在 exhaustive policy 中声明 GateMode 与 PlatformMode。新增 Intent 未声明读写和平台约束时必须编译失败。
- Protocol 只拥有跨 Application/Tauri Seam 的 Intent、Outcome、wire error 和 Agent stream event，不包含 SQLite row、Journal、Provider payload 或前端 view model。
- 跨 Tauri 的 canonical TypeScript 类型由 `ts-rs` 生成。生成文件提交到仓库；普通验证只检查漂移，显式更新动作才允许重写该文件。
- Serde tagged enum、rename、optional 和 null 表示通过固定 JSON fixture 验证。
- 普通业务继续使用 typed、task-specific Tauri commands。每个 command 只负责输入校验、构造 canonical Intent、调用 `SkillYardApplication` 和检查对应 Outcome，不复制业务状态机。
- 不新增 general-purpose IPC backend。Agent stream/cancel、native dialog 和受控 opener 继续保留符合其能力边界的专用 Adapter。
- command contract 变化必须核验真实调用者，并在一个 tranche 内更新 Rust command、生成类型、TypeScript client、注册表和测试；不保留 `legacy`、`v2` 或双入口。

### Domain 与 Module 深度

- Domain 按 Inventory、Source、Bundle、Install、Mount、Takeover、Removal、Association、Agent 和 Preferences 等既有领域词汇组织。
- Domain 只包含值对象、invariant 和与外部系统无关的纯逻辑；不得包含 SQLite、Tauri、Provider 或文件系统事务步骤。
- Module 目标是小 Interface 与深 Implementation。生产 Module 以约 `500` 行作为可读性方向，超过约 `800` 行触发职责审查；这些数字不是自动拆分或验收失败线。
- 只有拥有独立 invariant、真实共享知识或多个调用者时才建立 Module。不得创建只转发参数、只重新命名类型或仅为降低行数的浅层包装。
- 不引入 `Manager`、泛型 `Repository`、通用 `LifecycleV2` 或万能 Journal framework 等没有真实产品语义的抽象。

### Lifecycle 与安全文件系统

- Install、Mount、Takeover、Association 和 Removal 分别拥有自己的 plan、execute/confirm、journal 与 recover Implementation，但不增加新的状态或协议。
- startup recovery 只负责确定既有恢复顺序和协调各 canonical lifecycle，不拥有第二套恢复状态机。
- 共享生命周期锁和底层安全文件系统原语从 Install 协调中分离，由低层安全 Module 所有。
- 低层安全 Module 只负责安全句柄、相对路径操作、identity 校验、atomic write/rename/link/unlink、owned tree 与 sealed tree 操作；它不依赖 Domain workflow、Persistence 或 Agent。
- Mount effects 可以作为内部共享 Interface 提供 inspect、seal、isolate、restore、finalize 和 snapshot 等既有能力，但不拥有独立 Mount transaction。
- bundle update batch 继续组合 canonical Install transaction。其他 lifecycle 不得调用 sibling lifecycle 的生产事务入口；共享事实必须提升到 Domain、Persistence 或 recovery query。

### Persistence 与 migration

- Persistence 继续使用一个具体 `Storage`、一个 connection owner、一个 migration runner 和真实 SQLite；不增加 Repository trait 或数据库 mock。
- `Storage` 的 Implementation 按 Preferences、Inventory、Sources、Projects、Install、Mount、Takeover、Association、Updates、Removal 和 Recovery 等职责分开，但共享同一事务语义。
- Persistence 只依赖 Domain。Source network parsing、Provider、Application dispatch 和 lifecycle grouping 必须在进入 Persistence 前完成。
- 已发布 migration 使用带文件名、顺序、发布版本和 SHA-256 的 append-only lock manifest 保护。
- 立即锁定 v1.0.1 已发布前缀；1.1.0 发布时追加锁定当前 1.1.0 migration。CI 拒绝修改、删除、改名、重排或在已发布前缀中插入 migration。
- Train B 的职责重构不得新增 migration。任何真实产品 Schema 变化必须作为独立产品切片处理。

### Source、Agent 与外部 Adapter

- Source identity 的纯解析与 canonicalization 从 Persistence 分离；网络获取通过最小 `SourceTransport` Interface 实现 production 与 test Adapter。
- Agent 按 material、catalog、secret、provider 和 stream 职责分开，但仍使用一个全局只读 Agent 产品模型。
- OpenAI、GLM 和 DeepSeek 继续由固定 Provider enum 选择。私有 Provider Interface 只归一当前三个真实 Adapter 所需的 verify、stream、search 和 error 行为，不提供动态注册。
- API Key 继续只由 Keychain production Adapter 保存；测试使用内存 SecretStore，不能读取用户真实 Keychain。
- 不为文件系统和 SQLite 建立 trait。真实临时目录和真实临时 SQLite 是 canonical 测试替代环境。

### 统一命令、CI 与 Lint

- 仓库根使用 `justfile` 作为 Cargo、pnpm 和 Tauri 开发动作的唯一命令入口。它提供格式、定向 Rust 测试、slice、stage、release、wire、migration、target report 和 macOS contract 等语义化 recipe。
- CI 固定 `just` 版本并调用相同 recipe。README、Contributor 文档和 Agent 指令只解释何时使用，不复制 recipe 内部命令。
- 验证分为 targeted、slice、stage 和 release。普通切片使用最小可信范围；阶段完成运行全量测试、Clippy、wire、migration 和 production build；发布层另外运行 tart、MAC-CONTRACT、人工路径和单独授权的真实 Provider 验收。
- workspace lint 先建立现状基线，再逐步启用低误报规则。第一阶段不全局禁止 `unwrap`、`expect`、`unsafe`、`too_many_arguments` 或大型 enum。
- 在完成调用审计后，可使用 disallowed-method 规则保护 SQLite connection owner 和危险文件系统原语 owner；canonical owner 的例外必须局部且说明理由。

### Agent 指令与工程文档

- 根 Agent 指令继续保存 Product Contract、单一实现原则、阶段边界、架构变更门、安全和完成标准。
- Rust 范围使用嵌套 Agent 指令，说明 canonical Seam、依赖方向、增长审查、改动触发的验证和完成标准。
- 详细目标架构、验证阶梯和 future crate decision gate 只保存在一份工程设计文档中。精确命令以 `justfile` 和 CI 配置为事实来源。
- 新增或修改第二套 Plan、Journal、恢复协议、带版本后缀领域类型、旧生产入口或公开产品 Seam 时，必须停止并进入现有架构变更门。

### 实施 tranche

- Train A0 固定 Git、测试清单、ignored 清单、test executable、hard-exit worker、target 分区、clean isolated build、warm targeted test 和 App 体积基线。
- Train A1 聚合普通集成测试并修复 hard-exit worker；以测试清单和恢复结果证明等价。
- Train A2 设置有限调试信息并比较 isolated target 体积与 backtrace 可用性。
- Train A3 建立统一命令、CI 调用、只读 target report 和验证阶梯。
- Train A4 锁定已发布 migration 前缀并启用 CI 检查。
- Train A5 增加 Rust 范围 Agent 指令和工程文档索引。
- 完成 Train A 后继续完成并验收 1.1.0 产品增量。
- Train B1 生成 canonical wire 类型；Train B2 在保留 task-specific commands 的前提下收口 Tauri Adapter 重复；Train B3 提取安全文件系统；Train B4 拆分 Persistence Implementation；Train B5 按产品行为拆分 lifecycle；Train B6 拆分 Application、Source、Content 和 Agent；Train B7 收紧 lint 并复测 Cargo 指标。
- 每个 tranche 单独审查、验证和提交；本规格不授权 push、发布、部署或一次性大规模改写。

### 量化完成标准

- 普通 integration test executable 从当前多个顶层 target 收敛为 `1`；包含 macOS contract 在内的显式 integration test target 不超过 `2`。
- 聚合前后的非 ignored 与 ignored 测试逐项对应；hard-exit worker 必须证明真实执行。
- 在同一 toolchain、同一 commit 内容和独立 target 条件下，`cargo test --no-run` 物理产物相对基线至少下降 `30%`。未达到时不得声明 Cargo 产物治理完成。
- warm targeted test 中位耗时不得相对基线退化超过 `20%`；超过时评估两个普通聚合器的回退方案。
- limited debug 必须保留可解析的 panic backtrace 与源码行号。
- 已发布 migration checksum、Central Store 语义、Current Content、Current Link、Mount、Filesystem Transaction Journal 和重启恢复结果保持不变。
- wire drift、migration history 修改、未声明 Intent policy 和禁止依赖必须由自动验证发现。
- Train B 若进入 1.1.0 发布候选，必须重新完成 1.1.0 全量自动化、production build、tart、MAC-CONTRACT 和人工产品验收。

## Testing Decisions

- 好测试只观察外部行为、持久状态和安全边界，不断言私有函数调用顺序、内部文件拆分或为了重构新增的薄包装。
- 最高主 Seam 是 `SkillYardApplication`。Install、Takeover、Mount、Association、Update、Removal、startup recovery、Preferences、Discovery 和 Agent 的行为测试优先从该 Interface 进入。
- 跨语言 Seam 是 typed Tauri client。协议、Intent、Outcome、Agent stream event、native dialog 参数和受控 opener 行为通过 Rust serialization fixture 与 TypeScript client test 共同验证。
- 状态 Seam 是真实临时文件系统、真实临时 SQLite 和重新创建应用后的回读。Current Content、Current Link、Mount、Filesystem Transaction Journal、migration 和恢复不得用 mock 代替。
- 已有 startup、folder install、GitHub install、bundle update、batch mount、Takeover、Source Association、Removal 和 Agent 集成测试是本规格的 prior art；迁移只改变测试组织方式，不重新定义预期结果。
- 测试聚合迁移前保存完整测试清单和 ignored 状态。迁移后按名称和数量比较，并抽查每类生命周期的公开 Seam 行为。
- hard-exit tests 必须额外断言子进程确实执行目标 worker，不能只接受进程成功退出。每组测试继续验证崩溃点、Journal 残留、应用重启和最终业务真值。
- `MAC-CONTRACT` 继续显式 ignored，并只在准备好的 macOS 环境中运行。普通 CI 不安装、不调用也不模拟本机 Codex CLI。
- Provider、GitHub Source 和 Keychain 使用真实外部 Adapter 对应的 fake transport 或 in-memory secret Adapter；普通测试不得联网、读取用户凭据或产生费用。
- 文件系统与 SQLite 继续使用真实 local substitute。不得新增 `FileSystem`、Repository mock 或 in-memory lifecycle 来缩短测试。
- wire 测试固定代表性 tagged enum、rename、optional、null、error 和 stream event JSON。生成的 TypeScript artifact 与临时生成结果必须逐字一致。
- migration 测试固定已发布 manifest，验证顺序和 checksum；使用 1.0.1 数据库快照执行升级、启动恢复和核心 Inventory 回读。
- Cargo 产物基准必须使用同一 Rust toolchain、同一源码、相同 feature 和独立 target 目录。记录物理大小、普通 integration executable 数量、clean `--no-run` 时间和 warm targeted median。
- limited debug 验证使用一个确定性 panic/backtrace fixture，确认函数和源码行可定位；不要求日常 profile 保留所有局部变量。
- Module 拆分不以私有 module path 断言为主要测试。需要的结构 guard 只检查禁止依赖、canonical owner、migration/wire 漂移和 target 数量等真实工程约束。
- 一个结构 tranche 必须同时具备公开 Seam characterization evidence 和新的结构 guard。不得为制造红灯而加入与用户行为无关的错误断言。
- targeted 验证只跑当前行为及直接依赖；slice 验证覆盖对应公开 Seam 和跨语言调用；stage 验证运行全部离线自动化、Clippy 和 production build；release 验证追加 tart、MAC-CONTRACT、人工路径和独立授权的真实 Provider 检查。
- Train A 不要求重新验收产品视觉和 Provider 行为，但必须运行与配置、测试入口和 production build 相关的完整阶段验证。
- Train B 每个职责集群独立运行相关公开 Seam 测试；所有集群完成后必须运行全量阶段验证。若位于发布候选范围内，旧候选证据立即失效并重新执行发布验收。
- 测试失败时优先修复生产行为或真实 test harness。不得通过扩大 sleep、忽略测试、删除断言、绕过 Journal 或调用生产入口不会调用的私有步骤获得通过。

## Out of Scope

- 不新增或修改 SkillYard 用户可见产品能力。
- 不改变 Skill、Bundle、Skill Member、Source、Current Content、Mount、Project、Takeover、Local Lifecycle Authority 或 Managed Lifecycle 的领域语义。
- 不改变 1.1.0 Agent、Discovery、AI 整理、语言和 Theme Preset 的产品范围。
- 不建立 CLI、Daemon、localhost server、公共 Rust API、headless mode 或独立恢复工具。
- 不以单一 general-purpose dispatch command 取代 typed、task-specific Tauri commands。
- 不立即拆分 workspace crate，也不创建 `skillyard-storage`、`skillyard-lifecycle`、`skillyard-agent` 或平行 Domain crate。
- 不建立 `v2`、`legacy`、`next` 或兼容未发布中间状态的生产入口。
- 不建立通用 Journal、通用 Transaction、通用 Plan 或第二套 recovery protocol。
- 不引入泛型 Repository、SQLite mock、`FileSystem` trait 或 in-memory lifecycle。
- 不引入任意 Provider、自定义 Base URL、动态 Provider Registry、Tool Loop 或 Provider 插件系统。
- 不改变 release optimization、Tauri crate type、App signing、notarization、发布渠道或 Supported Platform。
- 不引入自动 `target` 清理、定时清理、隐式删除或以清理后大小代替基准验收。
- 不在第一阶段引入 `sccache`、远程构建缓存、新的持久 Cargo profile 或日常多 target directory。
- 不无条件启用所有 Cargo feature 组合。
- 不因行数目标机械拆文件、改名、全仓格式化或清理与当前 tranche 无关的代码。
- 不修改已发布 migration；真实新 Schema 需求必须另行进入产品规格和 migration 流程。
- 不把真实 Provider、Keychain、外部网络或付费 API 放入普通自动化测试。
- 不授权 commit、push、GitHub Issue 发布、PR、release、deploy 或任何外部状态变化；这些动作继续遵守各自授权边界。

## Further Notes

- 当前约 `12 GB` 的 `target`、各子目录大小、Rust 总行数和测试文件数量是规格形成时的诊断基线，不是永久产品常量。Train A0 必须在实施 commit 上重新固定可比较基线。
- 最终 App 约 `24 MiB`，说明当前主要问题是开发 artifact 和反馈成本，而不是用户下载包或运行时体积。本规格不以缩小最终 App 为目标。
- 普通集成测试从多个 executable 聚合为一个，是第一优先级的直接治理；把单 crate 内的大文件拆成 Module 主要改善 Locality、Interface 深度和审查范围，不会单独把 Cargo compile unit 拆开。
- `500` 行方向和 `800` 行增长审查线只用于发现职责扩张。拥有单一 invariant 的复杂算法可以超过；没有独立知识所有权的薄 Module 即使很短也不应存在。
- `ready-for-agent` 表示本规格已经确认并可以继续拆分为有依赖关系的实施 tickets，不表示一个 Agent 可以在一个未分阶段的改动中完成 Train A 与 Train B。
- 推荐首先拆出 Train A0 至 A5 的 tickets。Train B tickets 应在 1.1.0 产品验收状态明确后创建或解除阻塞。
- 若实施证据表明一个普通聚合器造成明显反馈回归，允许使用两个普通聚合器；这是本规格预先定义的回退，不是架构边界变化。
- 若实施要求新增第二套 Plan、Journal、恢复协议、公开产品 Seam、带版本后缀的领域模型，或修改下一阶段产品边界，必须停止并按仓库架构变更门重新取得确认。
