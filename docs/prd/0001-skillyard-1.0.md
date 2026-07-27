# SkillYard 1.0 PRD

> 状态：已确认。本文只描述已经确认的 SkillYard 1.0 产品边界；已从工作区删除的旧 Python 原型、已撤回设计和后续版本设想不构成 1.0 需求。

## Problem Statement

使用 AI Agent Skill 的用户通常从 GitHub、`skills.sh`、命令行工具、ZIP、本地目录、推特、知乎或论坛获得 Skill。安装完成后，这些 Skill 会散落在不同 Agent 应用的全局目录和项目目录中。用户很难回答几个基本问题：本机到底有哪些 Skill、它们来自哪里、属于哪个完整来源、哪些 Agent 或项目正在使用、上游是否已经变化，以及删除某个安装会影响什么。

现有安装工具通常只负责把内容放到某个目录，不负责长期维护本地使用关系。一个来源还可能包含多个 Skill：用户曾经只安装其中一部分，但上游更新面对的是整个来源。用户如果继续依靠手工复制和多个工具分别更新，容易产生重复副本、内容不一致、挂载失效、来源遗失，以及删除后又被其他工具重新写回等问题。

用户需要一个真正可以日常使用的本地管理器，而不只是清单或更新提醒。这个管理器需要把用户明确交给它的 Skill 统一保存到一个持久的中央目录，再通过软链接供不同 Agent 应用和项目使用；同时尊重 Codex 插件、Agent 内置 Skill 和项目仓库维护内容的原管理权，不能为了“统一”而接管一切。

安装、接管、更新和删除会同时修改 SQLite、多个目录和多个软链接。普通数据库事务无法覆盖这些文件系统操作。如果应用崩溃、系统终止或外部路径被修改，产品仍必须知道操作是在生效前还是生效后，并恢复到可解释的一致状态，避免丢失用户唯一主副本或留下半完成的挂载。

SkillYard 1.0 因此必须在保持产品简单的同时完成完整主流程：发现、安装、接管、挂载、检查更新、更新整个 Bundle、移除挂载、删除 Source、删除 Bundle，以及中断恢复。它不能通过自建公共 Skill registry、执行任意外部命令、内置 LLM、本地版本历史或复杂回滚来掩盖主流程缺口。

## Solution

SkillYard 1.0 是只在产品所有者个人 Apple Silicon Mac 上使用的本地桌面应用，目标系统为 macOS 14 及以上。它使用 Tauri 2、TypeScript 界面和封闭的 Rust Lifecycle Core，在这台 Mac 上本地构建并运行 `SkillYard.app`。1.0 不承担公开分发、第三方安装体验或 Apple 开发者身份验证。

首次启动时，应用先说明扫描范围、只读性质和不会自动接管，再由用户点击“开始扫描”。扫描完成后，主界面第一层以 Bundle 为列表；进入 Bundle 后查看 Skill 成员，再进入单个 Skill 查看来源、路径和 Mount。其他内容明确区分为“待接管”“Agent 应用管理”和“项目仓库管理”的只读分组。

SkillYard 使用以下模型管理生命周期：

- Source 是完整远端或本地来源，例如一个 GitHub 仓库、ZIP、确定性 URL 或 Editable Local Source；
- Bundle 是用户已经安装到本机的一个组，只在安装或接管后创建；
- Skill Member 是 Bundle 中可独立挂载的成员；
- Mount 是从 Supported App 的全局或项目目录指向中央主副本的软链接；
- Project 是用户明确登记、可以承载 project Mount 的本地项目。

Source 与 Bundle 可以分别独立存在，并最多建立一对一关联。删除 Source 只会让关联 Bundle 失去更新来源；删除 Bundle 才会移除该组全部 Mount 和本地受管内容，并保留 Source 供以后重新安装。

直接安装把内容先获取到临时区，发现并验证其中的 Skill，默认选择全部成员但允许用户在确认前取消。安装成功后内容进入固定 Central Store，默认保持“已安装、未挂载”；用户再主动选择 Codex、Claude Code、GitHub Copilot 以及 global 或 project scope 创建 Mount。

接管已有安装采用“扫描 → 接管计划 → 用户确认 → 临时恢复内容 → 按需搬迁 → 建立或校正 Mount → 验证”的事务流程。已经位于正确中央结构的内容只登记和校正链接，散落内容在确认后搬迁。扫描绝不静默移动、覆盖或删除文件。

检查更新只在用户点击时查询已有 Source。GitHub Source 通过 Tracked Ref 的 commit SHA 判断是否变化；ZIP 和直接文件 Source 使用手动导入替换。真正执行更新时，SkillYard 获取并验证完整 Source 内容，安装该 Source 当前的全部有效 Skill，并通过 Bundle 唯一的 `current` 软链接一次切换整个 Bundle。更新不提供成员排除、本地版本历史或回滚。

安装、接管、更新、Mount 变更、Bundle 归并和 Cascading Delete 等会改变受管内容或受控路径的高保证生命周期操作，必须先生成绑定当前状态的 Plan，展示影响并要求确认。确认后不可取消，应用同一时间只执行一个此类写事务。SQLite 保存领域状态，Filesystem Transaction Journal 保存跨文件系统操作的阶段和生效点；应用重启时自动完成或撤销中断操作，只有无法安全判断时才进入人工恢复。首次扫描、Local Refresh、Update Check 和 Source Catalog Reload 仍是用户主动触发的只读盘点或查询，不进入危险操作确认流程。

SkillYard 不执行 `npx skills`、`gh skill`、Lark CLI 或 Skill 中携带的代码。这些外部工具只作为 Installation Chain 和来源证据。产品不内置 LLM、不收集遥测、不自动检查上游，也不建设公共 registry。

## User Stories

### 构建应用与首次使用

1. 作为 SkillYard 1.0 的唯一用户，我希望在自己的 Apple Silicon Mac 上本地构建并运行 `SkillYard.app`，从而不需要购买 Apple Developer Program。
2. 作为唯一用户，我希望构建或启动时明确检查 macOS 14 和 `arm64` 环境，从而不会在未支持的环境中进入不可依赖的运行状态。
3. 作为首次用户，我希望先看到扫描范围、只读性质和不会自动接管的说明，从而知道应用准备读取什么。
4. 作为首次用户，我希望由自己点击“开始扫描”，从而在明确同意前应用不会读取本机 Skill 目录。
5. 作为首次用户，我希望首次扫描不访问 GitHub、`skills.sh` 或其他上游，从而本地盘点不会产生意外网络请求。
6. 作为没有安装任何 Skill 的用户，我希望空扫描也被记录为已完成，从而下次启动不会重复首次介绍。
7. 作为返回用户，我希望应用先恢复未完成事务并检查已有 Mount，再直接展示原有状态，从而无需重复配置。
8. 作为返回用户，我希望启动时不自动刷新完整本机清单或检查上游更新，从而所有耗时和联网动作仍由我发起。

### 本机清单与主界面

9. 作为用户，我希望主清单只列出 Bundle 或只读管理分组，从而不会被大量 Skill 卡片淹没。
10. 作为用户，我希望明确区分“由 SkillYard 管理”“待接管”“Agent 应用管理”和“项目仓库管理”，从而不会误以为所有扫描结果都能被 SkillYard 修改。
11. 作为用户，我希望 Bundle 卡片展示来源名称、成员数量、更新状态和批量操作，从而看清本地安装组和删除范围。
12. 作为用户，我希望进入 Bundle 后查看成员列表，并进入单个 Skill 查看来源、路径、Metadata、安装收据和 Mount，从而保留成员差异。
13. 作为用户，我希望一个 Skill Identity 只在所属 Bundle 详情中出现一次，从而多个 Mount 不会制造重复 Skill 条目。
14. 作为用户，我希望主列表只展示本机已有或已安装 Skill，从而 Source 中尚未安装的远端成员不会混入清单。
15. 作为用户，我希望按管理状态筛选和搜索，从而快速找到需要接管、更新或处理的内容。
16. 作为用户，我希望筛选只改变界面显示，从而不会意外改变安装、管理权或挂载关系。
17. 作为用户，我希望 SkillYard 的展示标签、Skill Name 和各 Agent 应用的展示标签彼此分开，从而 Agent 的 UI 前缀不会被误认为重命名。
18. 作为用户，我希望在生命周期事务运行时仍能只读浏览列表和详情，从而长操作不会锁死整个应用。
19. 作为用户，我希望所有页面持续显示当前操作及返回进度页的入口，并明确暂停“刷新本机”“检查更新”和 Source Reload 等会写入状态的动作，从而不会在事务期间引入竞争状态。
20. 作为用户，我希望 Agent 应用管理或项目仓库管理的 Skill 提供原管理方和跳转方向，从而在正确的位置处理它们。
21. 作为用户，我希望点击“刷新本机”后发现应用外新增、移除或改变的 Skill，从而继续使用现有安装工具也不会失去盘点能力。
22. 作为用户，我希望“刷新本机”只读取本地、不访问上游、不自动接管或修复，从而刷新行为可预测。

### Source 发现与管理

23. 作为用户，我希望“安装 Skill”提供仓库、`skills.sh`、URL、ZIP、本地目录和接管已有安装等入口，从而不需要先理解内部 Adapter。
24. 作为用户，我希望仓库视图初始包含已经确认的四个 cc-switch GitHub Source，从而第一次使用就有可浏览内容。
25. 作为用户，我希望这些初始 Source 与普通 Source 使用相同的维护和删除规则，从而不存在隐藏的特殊状态。
26. 作为用户，我希望通过 `owner/repo`、GitHub 仓库 URL 或包含 branch 和子目录的成员 URL 添加 Source，从而常见分享形式都能被识别。
27. 作为用户，我希望成员 URL 只定位和高亮 Skill，而完整仓库仍作为一个 Source，从而不会把同一仓库拆成多个来源。
28. 作为用户，我希望未提供 ref 时读取并保存仓库真实的 default branch，从而 SkillYard 不猜测 `main` 或 `master`。
29. 作为用户，我希望明确提供的 ref 必须先验证可访问，从而无效跟踪目标不会进入本地状态。
30. 作为用户，我希望同一个 canonical upstream 无论从推荐列表、`skills.sh`、URL 还是已有安装证据到达都复用同一个 Source，从而不会出现重复 Bundle。
31. 作为用户，我希望切换 GitHub Tracked Ref 前看到当前值和候选值并明确确认，从而切换分支不会被当成普通文本修改。
32. 作为用户，我希望切换 Tracked Ref 只影响后续发现和更新，从而当前受管内容和 Mount 不会立即改变。
33. 作为用户，我希望 `skills.sh` 搜索结果先还原成可验证 Source，并按 Source 分组，从而发现服务不会成为新的生命周期管理方。
34. 作为用户，我希望普通网页、推特、知乎和论坛帖子不能被直接当成安装源，从而 SkillYard 不会抓取页面并猜测下载内容。
35. 作为用户，我希望 GitHub Source 1.0 只处理公开仓库，从而应用不要求登录、读取或保存访问令牌。
36. 作为已经取得私有内容的用户，我希望通过 ZIP 或本地目录导入，从而私有 GitHub 认证缺失不阻止本地管理。
37. 作为用户，我希望进入发现页或主动重新加载时才访问 Source，从而应用启动和首次扫描不产生预取。
38. 作为用户，我希望 Source 目录完整获取和验证成功后才替换旧目录，从而超时、空响应或错误不会被解释成上游删除。
39. 作为用户，我希望重新加载失败时仍能查看上次成功目录和时间，从而保留调查线索。
40. 作为用户，我希望 Stale Source Catalog 不能用于新安装或 Bundle Update，从而过期目录不会驱动破坏性操作。
41. 作为用户，我希望 Source 可以在没有本地 Bundle 时独立保存，从而删除本地安装后仍能随时重新安装。
42. 作为用户，我希望 Source 已有关联 Bundle 时看到已安装成员和 Mount，从而不会重复安装已有内容。
43. 作为用户，我希望删除 Source 前看到哪些 Bundle 将失去更新能力，从而清楚这个操作不会删除本地内容。

### 直接安装

44. 作为用户，我希望选择 Source 后看到其中所有由有效 `SKILL.md` 定义的成员，从而明确将安装什么。
45. 作为用户，我希望全新安装默认选择全部有效成员，从而常见的完整 Bundle 安装不需要逐项勾选。
46. 作为用户，我希望在最终确认前取消不需要的成员，从而首次安装仍允许保留一个较小集合。
47. 作为选择部分成员的用户，我希望看到 SkillYard 不检查跨 Skill 依赖的提示，从而自行决定是否承担风险。
48. 作为用户，我希望没有发现 `SKILL.md` 的 Source 显示“未发现 Skill”且不能创建空 Bundle，从而失败不会留下无意义记录。
49. 作为用户，我希望无效 YAML、缺失字段、非法名称、目录名不匹配和重复名称得到明确错误，从而可以修复真实来源问题。
50. 作为用户，我希望 Nested Skill Conflict 显示重叠路径，从而知道为什么某个成员不能安装。
51. 作为用户，我希望安装和验证永不执行脚本、二进制或 hook，从而候选内容不会在进入 Agent 前运行。
52. 作为用户，我希望内容中的脚本或可执行文件只产生简短风险提示，从而不会被误解成恶意代码扫描结果。
53. 作为用户，我希望包含 symlink、hard link、FIFO、socket、device node 或非法归档路径的内容被拒绝，从而 Central Store 只包含边界明确的普通文件和目录。
54. 作为用户，我希望过大的下载或归档达到固定资源上限时立即停止，从而避免失控下载和压缩炸弹。
55. 作为用户，我希望整个候选 Bundle 在生效前完成验证，从而安装失败不会留下半个 Bundle。
56. 作为用户，我希望安装完成后默认保持“已安装、未挂载”，从而 Skill 不会自动出现在任何 Agent 应用中。
57. 作为用户，我希望之后主动为每个 Skill 选择 Supported App 和 global 或 project scope，从而使用位置由我决定。
58. 作为已有部分安装的用户，我希望补装尚未安装成员不会覆盖现有成员，从而内容替换只能通过正式 Bundle Update 发生。

### 接管已有安装

59. 作为用户，我希望扫描只生成 Takeover Candidate，从而未确认前不会移动、覆盖或删除任何文件。
60. 作为用户，我希望 Takeover Plan 展示 Source、Installation Chain、Bundle 边界、成员、路径、Mount、临时恢复内容和删除影响，从而确认的是一份具体计划。
61. 作为用户，我希望计划状态变化后必须重新确认，从而旧计划不能操作已经改变的文件系统。
62. 作为用户，我希望已经符合 Central Store 规则的内容只登记并校正链接，从而不会发生无意义搬迁。
63. 作为用户，我希望散落内容在可恢复事务中进入 Central Store，并在原 Host 位置重建 Mount，从而接管后现有使用方式继续工作。
64. 作为用户，我希望接管时默认保留已有 Host 使用关系，从而不需要重新选择已经存在的 Mount。
65. 作为用户，我希望可以在确认前取消某个已有使用位置，从而接管不强迫保留所有 Mount。
66. 作为用户，我希望内容相同的多个副本合并成一个主副本，同时保留全部使用位置，从而消除重复内容。
67. 作为用户，我希望内容不同的多个副本由我选择唯一主副本，从而所有 Host 最终使用同一份内容。
68. 作为用户，我希望未选中的副本只在事务完成前作为临时恢复内容存在，从而不会形成隐含版本历史。
69. 作为用户，我希望名称相同但身份不确定的副本不会自动合并，从而名称相似不会造成内容丢失。
70. 作为用户，我希望可以明确确认若干副本是否属于同一个 Skill，从而在证据不足时仍能完成受控接管。
71. 作为用户，我希望多个不同的来源未知 Skill 只有在确定性证据证明同一安装组时才进入同一 Bundle，从而父目录相同不会被误当作来源相同。
72. 作为用户，我希望来源未知也能被接管，从而本地主副本和 Mount 可以先统一管理。
73. 作为用户，我希望来源未知的受管 Skill 同时显示“由 SkillYard 管理”和“没有更新来源”，从而管理权与更新能力不会混淆。
74. 作为用户，我希望接管前同一 Skill 同时存在 global 和 project 安装时由我选择保留一种 scope，从而最终拓扑符合 1.0 规则。
75. 作为共享 `.agents/skills` 用户，我希望接管时选择目标 Supported App，并在应用专属目录建立 Mount，从而共享目录不会继续控制使用关系。
76. 作为用户，我希望共享入口只在全部新 Mount 验证成功后移除，从而失败时原有 Agent 仍能发现 Skill。
77. 作为用户，我希望无效或不安全 Skill 不能生成可执行接管计划，从而原安装目录保持不变。
78. 作为用户，我希望 SkillYard 只替换明确选择的单个 Skill 根目录，从而不会把 Agent 的整个 Skill 根目录变成软链接。
79. 作为外部 CLI 用户，我希望运行安装工具后通过“刷新本机”发现结果，再由 SkillYard 接管，从而保留现有生态但不把命令执行权交给 SkillYard。

### Project 与 Mount 管理

80. 作为用户，我希望只通过主动选择或接管确认把项目加入 Project 列表，从而 SkillYard 不扫描整块磁盘寻找仓库。
81. 作为用户，我希望添加 Project 后只读扫描三个 Supported App 的项目 Skill 目录，从而先看清项目里已有内容。
82. 作为用户，我希望添加 Project 不自动接管、搬迁或挂载任何 Skill，从而登记项目不等于授权修改。
83. 作为用户，我希望 project Mount 只能选择已经登记的 Project，从而目标范围始终明确。
84. 作为用户，我希望 Supported App 固定为 Codex、Claude Code 和 GitHub Copilot，从而 1.0 不会根据目录猜测新的应用。
85. 作为用户，我希望应用检测只显示“已检测到／未检测到”，从而检测结果不会修改 Supported App 列表。
86. 作为用户，我希望选择 Supported App 前看到实际目标和可能共同扫描该路径的其他应用，从而“选择应用”不会被误解成独占可见。
87. 作为用户，我希望同一 Skill 在同一 Supported App 中只能使用一个 global Mount 或多个不同 Project 的 project Mount，从而 scope 不会重叠。
88. 作为用户，我希望普通 scope 切换必须先移除旧 Mount，再创建新 Mount，从而不会隐式合并两个有风险的操作。
89. 作为用户，我希望不同 Supported App 可以分别选择 scope，从而 Codex、Claude Code 和 Copilot 的使用关系互不绑死。
90. 作为用户，我希望 Mount 叶子目录直接使用 Skill Name，从而 Bundle 标签和 Agent UI 前缀不会污染实际路径。
91. 作为用户，我希望 Mount 只使用软链接且不回退到复制，从而所有使用位置始终跟随中央主副本。
92. 作为用户，我希望目标路径已被未知内容占用时进入 Mount Conflict，从而 SkillYard 不会自动覆盖、改名或一键替换。
93. 作为用户，我希望目标已经是正确软链接时只校验并校正记录，从而重复操作不会创建第二个 Mount。
94. 作为用户，我希望 Bundle 分组提供批量挂载入口，从而可以一次选择多个 Skill 和目标。
95. 作为用户，我希望批量挂载在确认前列出每个 Skill、App、scope 和冲突，从而可以排除冲突成员。
96. 作为用户，我希望最终确认的批量挂载全成或全退，从而不会出现界面显示成功但只创建部分链接。
97. 作为用户，我希望移除 Mount 只停止对应 Agent 或 Project 使用，从而 Skill 回到“已安装、未挂载”而不是被删除。
98. 作为用户，我希望启动和“刷新本机”能够发现 Mount Drift，从而外部删除或改写软链接不会被忽略。
99. 作为用户，我希望 Mount Drift 不被自动修复，从而 SkillYard 不会覆盖外部新内容。
100. 作为用户，我希望 Drift 目标为空时可以确认修复，目标被占用时进入 Mount Conflict，从而修复仍遵守路径安全边界。
101. 作为用户，我希望正式移除异常 Mount 记录时不删除无法证明属于该 Mount 的外部内容，从而清理记录不会清理用户文件。
102. 作为用户，我希望移除仍有 managed Mount 的 Project 前看到全部影响，并在确认后事务性移除这些 Mount，从而项目记录与文件系统保持一致。

### 补充来源与 Bundle 归并

103. 作为来源未知 Bundle 的用户，我希望找到真实 Source 后补充关联，从而恢复上游发现和更新能力。
104. 作为用户，我希望 GitHub 仓库始终登记为完整 Source，从而补充来源不会把仓库拆成若干单成员来源。
105. 作为用户，我希望只为每个本地 Skill 选择“对应”某个 Source Member 或“不对应”，从而不需要解释不对应原因。
106. 作为用户，我希望来源关联保持当前内容和 Mount 不变，从而“补充来源”不会静默执行首次更新。
107. 作为用户，我希望 Source 中其他成员在关联时只显示为可用，从而不会立即进入本地 Bundle。
108. 作为用户，我希望 Source 已经关联另一个 Bundle 时看到完整归并计划，从而不会创建第二个 Source-backed Bundle。
109. 作为用户，我希望归并计划列出成员、Mount、重复身份、内容选择和路径冲突，从而所有歧义在执行前解决。
110. 作为用户，我希望归并成功后才清理已经为空的原 Bundle，从而归并不会被误当成内容删除。
111. 作为用户，我希望关联 GitHub Source 后当前内容先显示“可更新”，并在第一次完整更新后才建立已采用基线，从而不会猜测本地内容对应的历史 commit。
112. 作为用户，我希望更换 canonical Source 时先删除旧 Source，再添加和关联新 Source，从而当前内容不会被隐式替换。

### 检查和执行 Bundle Update

113. 作为用户，我希望上游检查只在点击全局“检查更新”时执行，从而启动、后台和定时任务不会联网。
114. 作为 GitHub Source 用户，我希望 Tracked Ref 的 commit SHA 变化即显示“可更新”，从而检查逻辑简单、确定且可解释。
115. 作为用户，我希望即使新 commit 只修改 README 或未安装成员也显示“可更新”，从而状态准确表达上游引用已经变化。
116. 作为用户，我希望 Update Check 只记录“可更新”“已是最新”或“无法检查”及对应上游标识，不下载候选内容或修改 Mount；检查失败时保留当前受管内容和上次可解释状态，从而检查和更新保持两个动作。
117. 作为 ZIP 或直接文件 Source 用户，我希望看到“手动更新”并通过“导入新内容”替换，从而没有可靠远端标识时不伪造自动检查。
118. 作为 Source-less Bundle 用户，我希望看到“没有更新来源”，从而不会被误标为“已是最新”或“手动更新”。
119. 作为用户，我希望单个 Bundle 可以独立更新，并在确认页看到将安装的全部 Skill、新增成员、已有 Mount 和上游链接，从而影响清晰但不过度复杂。
120. 作为用户，我希望更新确认不展示文件 diff、文件数量、自动 changelog 或行为摘要，从而 1.0 保持关注管理影响。
121. 作为用户，我希望 Bundle Update 固定安装 Source 的全部当前有效 Skill，从而不会形成成员级更新状态。
122. 作为用户，我希望更新时不能排除某个成员，从而一个 Bundle 只有一份完整当前内容。
123. 作为用户，我希望上游新增或此前未安装成员随更新进入 Bundle，但保持未挂载，从而内容完整但使用位置不被扩大。
124. 作为用户，我希望上游已移除或明确“不对应”的现有成员继续保留，从而更新不会静默删除本地受管内容。
125. 作为用户，我希望候选中任一成员验证失败时整个 Bundle Update 停止，从而当前内容、Member Selection 和 Mount 保持不变。
126. 作为用户，我希望完整候选通过后只替换一个 Bundle `current`，从而全部成员和现有 Mount 同时采用新内容。
127. 作为用户，我希望存在多个可更新 Bundle 时使用“全部更新”，并在确认前增减参与的 Bundle，从而批量入口仍可控制。
128. 作为用户，我希望 Batch Update 顺序执行各 Bundle 的独立事务，一个失败不回滚其他成功 Bundle，从而结果范围清晰。
129. 作为用户，我希望更新成功后不生成本地版本、Revision 或回滚点，从而 1.0 不引入复杂的版本管理。
130. 作为用户，我希望 1.0 不推断 Skill 重命名或路径迁移，从而名称、路径、描述或内容相似都不会触发身份合并；旧成员继续保留，新成员独立安装。

### Editable Local Source

131. 作为个人 Skill 作者，我希望把个人目录或本地 Git clone 明确登记为 Editable Local Source，从而继续在原目录编辑。
132. 作为作者，我希望 Host 使用 Central Store 中经过确认的副本，而不是直接扫描持续变化的编辑目录，从而使用状态可控。
133. 作为作者，我希望主动检查本地来源变化并看到完整 Bundle 影响，从而修改不会自动传播。
134. 作为作者，我希望确认后一次采用 Editable Local Source 的全部当前成员，从而更新仍遵守完整 Bundle 和原子切换规则。
135. 作为作者，我希望新成员采用后保持未挂载，从而本地编辑不会自动扩大 Agent 使用范围。
136. 作为作者，我希望来源目录不可访问时现有受管内容和 Mount 继续工作，从而编辑目录移动不会中断 Agent。
137. 作为作者，我希望重新指定来源路径前经过内容识别和确认，从而同名目录不会被静默关联。
138. 作为用户，我希望普通本地目录安装默认为一次性快照，只有明确选择 Editable Local Source 才持续跟踪，从而两个入口含义简单明确。

### 移除与删除

139. 作为用户，我希望 1.0 不出现成员级“卸载 Skill”或“删除 Skill”入口，从而停止使用和删除本地内容不会混淆。
140. 作为用户，我希望移除最后一个 Mount 后仍保留 Skill 内容，从而以后可以重新挂载。
141. 作为用户，我希望删除 Bundle 前看到全部 Skill、Mount、项目和将永久删除的受管内容，从而理解完整影响。
142. 作为用户，我希望 Cascading Delete 需要影响确认和第二次危险确认，从而不会误删 Central Store 中的唯一主副本。
143. 作为用户，我希望删除 Bundle 移除全部成员、Mount、Member Selection 和当前受管内容，但保留 Source，从而之后可以重新安装。
144. 作为用户，我希望删除 Source 只删除 SkillYard 保存的 Source、Catalog、检查结果和更新关联，从而上游、本地来源目录、Bundle、Skill 和 Mount 都不会被删除。
145. 作为 Editable Local Source 用户，我希望删除 Source 或 Bundle 都不会删除原编辑目录，从而 SkillYard 不越过用户内容所有权边界。
146. 作为用户，我希望删除 Source 只需普通确认，而删除 Bundle 使用危险确认，从而界面准确表达两者风险差异。

### 事务、中断恢复与持久数据

147. 作为用户，我希望每次会改变受管内容或受控路径的高保证生命周期操作先生成精确 Plan 和影响预览，从而最终确认前可以修改或放弃。
148. 作为用户，我希望最终确认后操作不允许取消或强制停止，从而高保证事务不会因为人为中止留下未知状态。
149. 作为用户，我希望应用同一时间最多运行一个写事务，从而多个操作不会竞争同一批文件和软链接。
150. 作为用户，我希望进行中的事务不会把文件系统中间状态显示成最终结果，从而界面始终可解释。
151. 作为用户，我希望进程退出、系统终止或崩溃后，下次启动自动读取 Journal 恢复，从而无需自己选择旧内容还是新内容。
152. 作为用户，我希望 `current` 生效前中断时保留旧内容并清理候选，从而 Agent 继续使用原状态。
153. 作为用户，我希望 `current` 生效后中断时保留新内容并继续完成记录、Mount 和清理，从而已经生效的内容不会被错误撤销。
154. 作为用户，我希望普通恢复只提示“已恢复上次中断的操作”，从而事务恢复不会包装成版本选择。
155. 作为用户，我希望只有内容缺失、外部路径被改写、权限异常或证据不足时进入人工恢复，从而可自动判断的情况不打扰我。
156. 作为用户，我希望人工恢复只阻塞相关对象的写操作，从而其他 Skill 和只读功能仍可使用。
157. 作为用户，我希望 Central Store 固定在本机 Application Support 并可从 Finder 打开，从而知道受管主副本在哪里。
158. 作为用户，我希望 Central Store 中持续存在 `SKILLYARD-INFO.md`，说明这里不是缓存并列出已知 Source 和 Mount，从而手工查看时不容易误删。
159. 作为用户，我希望删除或移动 `SkillYard.app` 后 Central Store、SQLite、`current` 和 Mount 全部保留，从而 Agent 中已经挂载的 Skill 继续可用。
160. 作为重新构建或重新放置应用的用户，我希望应用检测既有状态并恢复管理能力，从而无需重新接管全部 Skill。
161. 作为用户，我希望在设置页打开 Central Store，或使用只清除偏好、窗口状态和缓存的“重置应用”，从而主界面只保留日常管理入口，并且不会删除任何受管内容或使用关系。

### 个人使用与隐私

162. 作为唯一用户，我希望 1.0 只服务当前这台个人 Mac，从而不需要为公开发布建设安装、签名、公证和兼容支持流程。
163. 作为唯一用户，我希望需要新版本时重新本地构建并手动替换 `SkillYard.app`，从而 1.0 不需要应用自更新系统。
164. 作为唯一用户，我希望手动替换或删除应用本体不影响 Central Store、SQLite、Journal 和 Mount，从而已经挂载的 Skill 继续工作。
165. 作为唯一用户，我希望所有管理数据、路径、Skill 名称和崩溃信息留在本机，从而不需要遥测同意或隐私开关。
166. 作为唯一用户，我希望只有主动触发 Source 获取、搜索、Update Check 或 Bundle Update 时才发生必要网络请求，从而联网行为与我的动作一致。
167. 作为唯一用户，我希望 SkillYard 无法以当前用户权限访问目标路径时在首次变更前失败，从而应用不会提权、绕过系统保护或留下半完成状态。

## Implementation Decisions

### 产品与运行边界

- 1.0 只支持 Apple Silicon `arm64` 和 macOS 14 Sonoma 及以上系统，不构建 Intel 或 Universal 2 产物。
- `SkillYard.app` 是唯一用户入口；不提供 CLI、headless 模式、daemon、localhost API、公开 Rust API 或独立恢复工具。
- 使用 Tauri 2 和 TypeScript 构建桌面界面，前端静态资源打包进应用并由 WKWebView 加载。
- 生产应用不启动 localhost Server，不捆绑 Chromium、Python runtime 或 Python sidecar。
- 1.0 在产品所有者的个人 Mac 上本地构建和运行，不制作面向公众的 DMG、ZIP、GitHub Release 或其他安装包。
- 1.0 不加入 Apple Developer Program，不承诺 Developer ID、Hardened Runtime、notarization、stapling 或无警告的 Gatekeeper 安装体验；本地构建可以使用 macOS／Tauri 所需的 ad-hoc signing，但它不代表 Apple 验证身份。
- 1.0 不进入 Mac App Store，也不启用 App Sandbox。
- 所有扫描和生命周期操作只使用当前登录用户权限；TCC、POSIX permission、ACL、System Integrity Protection 或只读文件系统拒绝访问时，必须在首次变更前失败。SkillYard 不提权，也不绕过系统保护。

### 应用架构与内部命令

- TypeScript 只负责界面渲染、临时表单状态、用户选择和确认呈现。
- Rust Lifecycle Core 是 SQLite、网络获取、本地扫描、候选验证、Bundle、Mount、事务和恢复的唯一执行边界。
- 前端不能获得通用文件系统、SQL 或 shell 能力，也不能直接操作 Central Store、Supported App 目录、Project 或 Journal。
- Tauri 只暴露任务级类型化命令，例如读取 Inventory、开始扫描、生成安装或接管 Plan、确认 Plan、读取进度和恢复状态。
- 主要业务入口是内部 `SkillYardApplication` 接收封闭的 `UiIntent` 并返回类型化 `UiOutcome`；这不是公开 JSON dispatcher 或第三方 API。
- Plan 在 Rust 侧持久或可靠缓存，并绑定输入、候选内容、文件系统前置条件和影响范围。确认命令只能执行已经签发且仍有效的 `plan_id`。
- 未签发、过期或前置状态已经变化的 Plan 必须拒绝执行并要求重新生成，不能接受 TypeScript 回传任意文件操作。

### 领域模型

- `Source` 表示完整上游或本地来源；不同发现方式解析到同一个 canonical upstream 时必须复用同一 Source。
- `Source Catalog Member` 表示 Source 当前可发现但不一定已安装的成员。
- `Bundle` 表示本地已安装组和 Cascading Delete 范围，只在直接安装或 Takeover 确认后创建。
- `Skill Member` 属于一个 Bundle，是展示和 Mount 的实际成员；Source 中未安装成员不属于 Bundle。
- `Skill Identity` 只标识已经建立的本地 Bundle Member；1.0 不根据上游名称、路径、描述或内容变化推断成员重命名或路径迁移。
- `Member Selection` 保存当前 Bundle 中已经安装的成员集合。它可以因补装、接管或完整 Bundle Update 增加，但不能通过成员级删除缩减。
- `Project` 是用户显式登记的项目根目录，可以承载 project Mount。
- `Mount` 关联一个 Skill Member、Supported App 和 scope；project Mount 还关联一个 Project。
- `Managed Bundle Directory` 包含一个 Bundle 的完整当前受管内容和唯一 `current` 软链接。
- `Lifecycle Transaction` 关联具体操作、影响对象、持久化阶段和 Filesystem Transaction Journal。
- Source 与 Bundle 是可选一对一：任一方都可以独立存在，但一方最多关联另一方一个实例。
- 一个 Skill 可以没有 Mount、拥有多个不同 Supported App 的 Mount，或拥有同一 Supported App 下多个不同 Project 的 project Mount。
- 一个 Skill 与同一 Supported App 不能同时拥有 global 和 project Mount。
- 管理状态、来源状态、更新状态、使用状态、Mount 健康状态和事务状态是相互独立的状态轴，不能压成一个模糊标签。

### 名称与展示

- Skill Name 来自 Skill 内容，不由 SkillYard 或 Supported App 的 UI 前缀改写。
- Host Presentation Label 只读展示，由对应 Agent 应用决定，不参与 Skill Identity、Source、Bundle 或 Mount 判断。
- SkillYard Presentation Label 使用 `Bundle Display Name: Skill Name`。
- GitHub Bundle Display Name 固定使用 canonical `owner/repository`；ZIP、`.skill` 和普通本地目录使用去除扩展名的 basename。
- Bundle Display Name 在创建时固定，删除 Source 后继续保留；1.0 不允许自定义名称。
- Mount 叶子目录固定使用 Skill Name，不加入 Bundle 前缀、自动后缀、alias 或 Host Presentation Label。

### Supported App 与 Project

- 1.0 的 Supported App 固定为 Codex、Claude Code 和 GitHub Copilot。
- 每个 Supported App 由内建路径配置描述显示名称、global Mount 根、project Mount 根、只读兼容目录、已知路径重叠和可选安装检测方式。

| Supported App | global Mount 根 | project Mount 根 | 1.0 依据 |
| --- | --- | --- | --- |
| Codex | `~/.codex/skills` | `<project>/.codex/skills` | 当前 Codex loader 仍兼容，但已将这组路径标记为 deprecated |
| Claude Code | `~/.claude/skills` | `<project>/.claude/skills` | Claude Code 官方 Skill 路径 |
| GitHub Copilot | `~/.copilot/skills` | `<project>/.github/skills` | GitHub Copilot 官方 Skill 路径 |

- Codex 的当前官方推荐路径是用户级和项目级 `.agents/skills`；SkillYard 使用仍受支持的 `.codex/skills` 兼容路径，是为了保留按应用选择 Mount 的能力。它必须针对当前个人电脑安装的 Codex 版本做兼容性测试，而不能被描述为长期稳定的官方路径。
- 用户级和项目级 `.agents/skills` 会被 Codex 与 GitHub Copilot 扫描；Claude Code 官方文档没有声明直接扫描这些目录。
- 项目级 `.claude/skills` 会被 Claude Code 和 GitHub Copilot 共同扫描。创建 Claude Code project Mount 前必须提示这项交叉可见性。
- `.agents/skills` 等多应用共享目录只用于只读扫描、Management Evidence 和 Takeover，不能作为 Central Store 或新 Mount 目标。
- 安装检测只返回“已检测到／未检测到”，不能生成新的 Supported App。
- “Host 可以发现 Skill”的 1.0 验收定义是：目标路径正确、Mount 指向预期稳定成员路径、成员包含有效 `SKILL.md`，以及应用配置声明会扫描该路径；自动化测试不启动或操控 Agent 应用本体。
- Host 内置或 Plugin Skill 只有在 Host、Plugin manifest、安装记录或受支持目录提供确定性证据时才只读展示并标注原管理方；1.0 不承诺仅靠文件扫描完整枚举 Claude Code 或 GitHub Copilot 的全部 bundled skills。
- Project 只能由用户选择或 Takeover 时确认加入，不能通过全盘扫描自动发现。
- 1.0 假设 Project 位于当前 Mac 的本地、持续可访问目录，不建立外置磁盘和离线 Project 状态机。

### 首次扫描、Local Refresh 与所有权分类

- 首次扫描完成状态保存在 SQLite；用户点击“开始扫描”前不读取 Skill 目录。
- 首次扫描和 Local Refresh 都是本地只读操作，不访问上游、不接管、不修复、不安装或删除。
- 启动顺序固定为：恢复未完成事务、检查已登记 Mount 健康、加载已有状态。
- Local Refresh 扫描 Supported App 的固定 global、已登记 project 和只读证据目录，并更新 Inventory、Management Evidence 和 Mount 健康。
- 所有权分类只依赖内建规则、确定性本地证据和用户确认，不使用名称相似、目录邻近或 AI 推断。
- 能够用确定性证据识别的 Codex 插件、Agent 内置 Skill 和明确由项目仓库维护的 Skill 只读展示，生命周期操作交回原管理方。
- 1.0 只自动读取已经核验的 lock v3 作为 Installation Chain；保存其中的 Source URL／路径和上游 Skill 路径等事实，但不猜测具体执行工具，也不把它直接登记为可更新 Source。Lark 专属状态、GitHub frontmatter 和其他 receipt／manifest 格式留到后续版本。

### Source Adapter 与发现

- 1.0 支持公开 GitHub、`skills.sh`、确定性可下载 URL、ZIP、`.skill`、普通本地目录和 Editable Local Source。
- GitHub Source 的 canonical identity 是仓库，不因 URL 形式、ref、成员子目录或发现入口不同而改变。
- GitHub Source 保存一个已验证的 Tracked Ref；未提供 ref 时读取并保存实际 default branch 名称。
- `skills.sh` 只负责发现，搜索结果必须解析为受支持 Source；后续安装和更新使用对应 Source Adapter。
- 仓库视图初始内建 `anthropics/skills@main`、`ComposioHQ/awesome-claude-skills@master`、`cexll/myclaude@master` 和 `JimLiu/baoyu-skills@main`。它们是可删除的普通本地 Source 配置，不是动态 registry。
- 普通网页、社交媒体或论坛页面不是可安装 Source，应用不抓取页面或猜测下载链接。
- 公开 GitHub 是 1.0 唯一直接支持的 GitHub 权限范围；不实现登录、token 存储或私有仓库 API。
- 普通本地目录安装是一次性快照。只有用户明确选择“作为 Editable Local Source”时，原目录才成为持续维护的 Source。
- Editable Local Source 的变化只在用户从 Source 详情主动执行“检查本地改动”时读取，不使用文件监听或后台轮询。
- ZIP、`.skill` 和直接文件 URL 不依赖 HTTP metadata 做自动更新；关联 Bundle 固定显示“手动更新”。
- 非 GitHub Adapter 的输入格式和 canonical identity 必须在实施计划中形成封闭清单；不允许实现阶段自行扩张为任意网页或任意 manifest。

### Skill 发现与内容验证

- Skill 以目录中的 `SKILL.md` 为成员边界，并严格解析 YAML frontmatter。`name` 必填、长度 1–64，只允许小写 ASCII 字母、数字和连字符，不能以连字符开头或结尾、不能包含连续连字符，且必须与根目录名一致；`description` 必填且长度 1–1024。未知可选字段原样保留，Markdown 正文不做质量或效果评价。
- 同一 Bundle 中 Skill Name 必须唯一；不同 Bundle 可以同名，但挂载到同一个 Supported App 和 scope 时会形成 Mount Conflict。
- Nested Skill Conflict 必须在计划前报告具体重叠路径；父子两个成员都验证失败，不能通过只选其中一个、自动排除或重划根目录绕过。
- 任何候选内容中的脚本、二进制和 hook 都不能在扫描、安装、接管、验证或更新时执行。
- Skill Member 内容只允许普通目录和普通文件。symlink、hard link、FIFO、socket、device node 和其他特殊条目一律拒绝。
- 归档路径规范化后必须仍位于本次临时区；绝对路径和目录逃逸输入在展开前失败。
- Source Resource Limit 固定为：累计接收 100 MiB、归档 20,000 条目、展开普通文件总量 512 MiB、单个普通文件 100 MiB，其中 `1 MiB = 1,048,576 bytes`。
- 限制必须在流式读取和写出过程中累计，不能仅依赖 `Content-Length` 或展开后的磁盘统计。
- 资源上限是 1.0 常量，不提供设置或强制继续。
- 超限错误必须显示触发的限制、固定上限和实际已检测值，并清理本次临时内容。
- 直接安装可以排除失败成员后继续选择其他有效成员；完整 Bundle Update 和手动替换中任一候选成员失败则整个操作失败。

### Central Store、安装与 Takeover

- Central Store 固定在 `~/Library/Application Support/SkillYard/`，是用户 Skill 的持久唯一主副本，不是缓存。
- Central Store 包含 SQLite、按 Bundle 保存的完整当前内容、唯一 `current`、临时区、事务恢复临时内容和 Filesystem Transaction Journal 的受控子区域。
- 根目录持续生成 `SKILLYARD-INFO.md`，说明内容性质并列出已知 Source 与 Host／project Mount。
- 直接安装在临时区获取和验证内容，按用户最终选择构建完整候选 Bundle，再原子建立 `current`。
- 全新安装默认选择全部成员，但用户可以在最终确认前取消；部分安装必须提示依赖风险，风险由用户承担。
- 直接安装默认不创建 Mount；安装完成后成员处于“已安装、未挂载”。
- 已有关联 Bundle 时，安装入口只能补充尚未安装的成员，不能覆盖现有成员。
- GitHub Source 创建新 Bundle 的首次直接安装保存本次实际获取内容的 commit 作为已采用基线。补充成员会生成包含现有内容和新增成员的完整候选并原子切换，但不会推进既有 Bundle 的已采用 commit；后续只有成功的完整 Bundle Update 才推进该基线。
- Takeover 必须经过扫描、Plan、确认、事务恢复临时内容、按需搬迁、Mount 重建或校正和验证。
- Takeover 只处理用户选择的单个 Skill 根目录，绝不替换 Supported App 的整个 Skill 根目录。
- 内容相同的副本可以合并关系；内容不同的副本必须由用户选择唯一内容，所有 Mount 最终统一指向它。
- 来源未知不阻止 Takeover，但没有 Source 的 Bundle 不能检查或执行更新。

### Mount 与 Project 事务

- 所有 Mount 都是指向 Bundle `current` 下稳定成员路径的软链接，不提供 copy fallback。
- 创建 Mount 前必须检查目标路径；未知或其他内容占用时进入 Mount Conflict，不能自动覆盖、改名或替换。
- 正确软链接只需验证和校正 SQLite 记录，不重复创建。
- 普通 scope 变更由“移除原 Mount”和“创建新 Mount”两个独立用户操作完成。
- Batch Mount 在确认前检查全部路径，用户可以排除冲突项；确认后的集合在一个可恢复事务中全成或全退。
- Mount Drift 只在启动和 Local Refresh 时检查，不使用文件监听或后台轮询。
- 修复 Drift 时，目标为空才可重建；目标被占用时进入 Mount Conflict。
- 正式移除异常 Mount 记录时，只删除能够验证为该 Mount 的软链接，不能删除身份不明的占用内容。
- 移除 Project 先事务性移除其中全部 SkillYard-managed project Mount，再删除 Project 记录；不删除 Bundle 内容或其他 Mount。

### Source 关联与更新

- Source 关联只建立更新关系和可选成员对应，不替换 Current Content、不修改 Mount、不自动执行更新。
- Source 已关联另一个 Bundle 时必须执行 Bundle 归并，不能建立第二条 Source-to-Bundle 关系。
- 归并以已关联 Source 的 Bundle 为保留目标，先准备完整候选并验证 Mount，成功后才清理空的原 Bundle。
- GitHub Source 与来源未知 Bundle 首次关联后直接显示“可更新”；不通过哈希或相似度猜测历史 commit。
- GitHub Update Check 只比较同一 Tracked Ref 的已采用 commit SHA 与当前 commit SHA，不下载 archive。
- 新 commit 无论修改了哪个文件都表示上游引用已变化。
- 上游查询失败时记录“无法检查”、检查时间和错误摘要，保留上次成功标识、Current Content、Member Selection 和 Mount，不把失败解释为“已是最新”或成员删除。
- ZIP 和直接文件 Source 使用“导入新内容”；没有 Source 的 Bundle 不显示手动更新入口。
- 完整 Bundle Update 获取 Source 的全部当前内容，并安装全部通过验证的当前成员，不提供成员排除。
- 更新候选同时保留上游已移除和明确“不对应”的现有成员，避免更新隐式删除受管内容。
- 新增和此前未安装成员进入 Bundle 后保持未挂载。
- 一个 Bundle 只有一份完整 Current Content；所有 Mount 通过唯一 `current` 同时采用它。
- Batch Update 是顺序协调的独立 Bundle 事务，不是跨 Bundle 原子事务；普通失败不回滚其他成功 Bundle，并继续后续 Bundle。若某个事务进入阻塞人工恢复，应用级写入门仍被占用，剩余 Bundle 标记为未执行。
- GitHub 新 Bundle 的首次直接安装建立已采用 commit；此后只有成功的完整 Bundle Update 才更新 GitHub 已采用 commit 或相应 Source 基线。
- 1.0 不创建本地版本号、Revision、历史内容或回滚点；成功后旧内容仅按事务清理规则处理。

### 删除语义

- 移除 Mount 只解除一个使用位置，不删除 Skill Member、Member Selection 或 Current Content。
- 1.0 不提供成员级卸载、删除或清空 Bundle 成员的入口。
- 删除 Bundle 是 Cascading Delete：移除全部 managed Mount、Member Selection、Managed Bundle Directory、Current Content 和 Bundle 记录，保留 Source。
- 删除 Bundle 必须经过完整影响预览和第二次危险确认，成功后没有回滚入口。
- 删除 Source 只删除 SkillYard 保存的 Source 记录、Source Catalog metadata、检查结果、更新标识和 Source-to-Bundle 关联，保留上游或用户输入目录以及本地 Bundle、Skill、Mount 和内容。
- 删除 Source 只需普通确认，并列出将失去更新来源的 Bundle。
- 删除 Source 或 Bundle 都不能删除用户拥有的 Editable Local Source 原目录、Agent-managed 内容或 Project-managed 内容。

### SQLite、Journal 与恢复

- SQLite 保存首次扫描状态；最近一次 Local Refresh 的时间、结果和 Inventory；Management Evidence；canonical Source、Tracked Ref、Source Catalog、上次成功获取时间与最近失败结果；Bundle、Skill Identity、Member Selection 和成员映射；Current Content 引用、预期 `current` 目标和稳定成员路径；Project、Mount、健康状态；已采用和最近检查的上游标识；以及生命周期事务与 Journal 引用。
- SQLite 不保存 Skill 文件内容、本地版本历史或回滚点。
- 每个会修改 Central Store、Mount、Project 受控路径或删除受管内容的生命周期操作，在首次文件系统变更前同时建立 SQLite 事务记录和持久化 Filesystem Transaction Journal。
- 首次扫描、Local Refresh、Update Check、Source Catalog Reload 和仅修改 Source metadata 的操作使用 SQLite 原子提交，不创建虚假的文件系统 Journal；需要确认的 Source 维护动作仍使用各自已经定义的普通确认。
- Journal 保存 Plan、受影响路径、旧目标、预期新目标、临时恢复内容、持久化阶段和已经完成的幂等步骤。
- Bundle `current` 的原子替换是安装和更新的唯一内容生效点。
- 启动恢复核对 Journal 与实际 `current`：指向旧目标表示生效前，指向新目标表示生效后，指向第三个目标则进入人工恢复。
- 生效前中断保留旧内容并清理候选；生效后中断保留新内容并完成记录、Mount、验证和清理。
- Takeover、Cascading Delete、Project Remove 和 Bundle Merge 等多路径操作必须持久化阶段，并允许安全重复每一步。
- 应用级写入门保证同一时间最多一个生命周期写事务；Batch Update 也只能顺序启动独立事务。
- 生命周期写事务运行时，Local Refresh、Update Check、Source Catalog Reload 和其他会提交 Inventory 或 Source 状态的动作保持不可执行；纯浏览继续可用。
- 最终确认后不允许取消或强制停止。进程退出属于中断，由下次启动恢复。
- 人工恢复仅在内容缺失、路径被外部改写、权限异常或状态无法安全判断时出现，并只阻塞相关对象。

### 应用本体、数据生命周期与隐私

- 1.0 不提供 Application Update、更新源、下载安装器或 GitHub Releases 发布流程；“检查更新”和 Bundle Update 只指 Skill Source 内容。
- 需要更新应用本体时，由产品所有者在同一台 Mac 上重新构建并手动替换 `SkillYard.app`。
- 手动替换应用本体不能修改 Central Store、SQLite、Current Content、Journal 或 Mount；新构建启动后仍先执行普通事务恢复。
- 删除或移动应用本体不会删除托管数据，也不会把 Mount 转换成普通文件。
- 重新构建或重新放置应用后，使用固定 Central Store 恢复原管理状态。
- App Reset 只能删除偏好、窗口状态和缓存，不能删除生命周期数据。
- SkillYard 采用 Zero Telemetry，不上传分析事件、设备标识、崩溃信息、Skill 名称、Source URL、Project 路径或本机管理状态。
- 网络请求只在用户主动触发 Source 获取、搜索、Update Check 或 Bundle Update 时发起，并只携带对应协议所需信息。

## Testing Decisions

### 主要业务测试边界

- 1.0 只设一个主要业务验收 seam：测试直接向内部 `SkillYardApplication` 发送类型化 `UiIntent`，并验证返回的 `UiOutcome`、持久化状态和真实文件系统结果。
- 该 seam 位于 TypeScript UI 之下、Rust Lifecycle Core 之上，覆盖与真实 App 相同的 Plan、确认、事务和恢复路径。
- 测试不通过旧 CLI、localhost Server、HTTP endpoint 或通用 JSON dispatcher 驱动业务，也不把测试入口发布给用户或第三方。
- 好的业务测试只断言用户可观察结果：列表和详情状态、Plan 影响、错误类型、SQLite 重开后的关系、实际文件内容、`current` 目标、Mount 目标、网络请求和重启恢复终态。
- 测试不以内部 helper 调用次数、函数顺序或私有数据结构为主要断言。

### 测试环境

- 每个业务测试使用独立临时目录、真实文件型 SQLite 和正式 migrations，不使用内存数据库或 mock repository。
- 使用真实文件、目录、软链接和硬链接构建 Central Store、Supported App 根目录、Project 根目录和不安全输入。
- 测试可以注入临时根目录，但 Central Store 位置不能因此成为面向用户的可配置项。
- 只替换最外层 `SourceTransport` 网络边界；GitHub metadata、ref、commit、archive、`skills.sh` 响应、超时、断流和超限输入使用真实协议格式。
- Source Adapter 的 canonicalization、Catalog、成员发现、归档展开、资源限制和 Skill 验证必须运行生产代码。
- 测试可以注入确定性时钟、ID 和事务 failpoint，用于稳定复现过期 Plan 和各持久化阶段中断。

### 必须覆盖的业务行为

- 首次扫描点击前零读取、点击后只读盘点、空结果也保存完成状态，以及返回用户启动不访问上游。
- 四种管理状态、Skill 按 Bundle 分组、未安装 Source Member 不进入主列表，以及 Installed-but-Unmounted 状态。
- Source canonicalization、Tracked Ref、Stale Source Catalog、GitHub 公开仓库边界和不同发现入口去重。
- Update Check 的“无法检查”状态、上次成功结果保留，以及查询失败不改变任何受管内容。
- 直接安装默认全选、部分安装警告、补装不覆盖现有成员、无 Mount 默认状态和完整候选原子生效。
- Safe Skill Content、YAML、名称、Nested Skill Conflict、归档逃逸和四项 Source Resource Limit。
- Takeover 在确认前零写入、已有 Mount 保留、重复副本选择、Unknown Provenance、多应用共享目录迁移和整个 Host 根目录保护。
- Project 登记、scope 互斥、Mount Conflict、Batch Mount 全成全退、Mount Drift 检测和修复边界。
- Source 关联不修改内容、Source-to-Bundle 唯一性、Bundle Merge 和首次 GitHub 更新基线。
- GitHub SHA 检查、手动替换、完整 Bundle Update、新成员未挂载、上游已移除成员保留和 Batch Update 独立结果。
- 移除 Mount、删除 Source、Cascading Delete 两次确认，以及 Editable Local Source 原目录保护。
- 当前用户权限不足、TCC／ACL／SIP 或只读文件系统拒绝时在首次变更前失败，并且不尝试提权或绕过保护。
- 生命周期事务期间禁用 Local Refresh、Update Check 和 Source Catalog Reload，事务结束后再读取最终状态。
- 零遥测、用户触发网络，以及手动替换应用本体不接触托管数据。

### 中断恢复测试

- 测试先生成并确认真实 Plan，在每个持久化阶段触发 failpoint，然后用同一 SQLite 和文件系统重新构造应用并执行正常启动恢复。
- `current` 切换前中断必须保留旧内容和 Mount，并清理候选。
- `current` 切换后中断必须保留新内容并继续完成 SQLite、Mount、验证和清理。
- `current` 指向旧目标和新目标之外的路径时必须进入人工恢复，不能猜测。
- Cascading Delete 必须分别覆盖破坏性生效点前后中断。
- Takeover、Project Remove 和 Bundle Merge 必须验证阶段重放不会创建重复内容、重复 Mount 或额外删除。
- 恢复期间第二个写事务必须被拒绝；恢复完成后只展示最终状态。
- 至少保留少量子进程强制终止测试，证明正确性不依赖析构函数或正常退出回调。

### UI、IPC 与本机应用验证

- TypeScript component tests 只验证用户动作产生正确类型化 intent，以及 Plan、危险二次确认、处理中、失败和人工恢复界面的呈现。
- IPC contract tests 验证 TypeScript 与 Rust 的命令名称、序列化类型和错误映射一致，不重复业务规则。
- Tauri smoke test 验证 WKWebView 加载打包资源、关键命令可调用、生产应用不启动 localhost。
- Supported App 的自动化验收通过固定路径、有效 `SKILL.md` 和软链接目标完成，不把启动真实 Agent 应用作为测试依赖。
- Codex 专属 `.codex/skills` global 和 project 路径必须对当前个人电脑安装的 Codex 版本执行兼容性 contract test；任一路径不再被扫描时，Codex Mount 支持不能仅凭旧文档判定通过。
- 本机构建 smoke test 验证 `.app` 为 `arm64`、最低 macOS 14，并能在产品所有者当前 Mac 上直接启动。
- 应用本体替换测试验证重新构建并手动替换 `.app` 后，原 Central Store、SQLite、Journal 和 Mount 仍被识别且保持不变。

### 从旧原型提炼的测试先例

- 旧原型验证过的“真实临时 SQLite、文件系统和 symlink”测试方式可以在 Rust 中重建。
- “Plan 或预览不修改状态”“未签发或过期 Plan 不可执行”“确认绑定已预览候选”等不变量可以迁移。
- “Mount Conflict 在首次写入前阻止操作”“外部替换软链接后报告 Drift”“同一内容服务多个应用和项目”等场景可以迁移。
- 旧 CLI、HTTP、HTML、旧领域模型、copy fallback、自动改名、Cursor、执行外部命令、AI Assist、启动自动扫描、本地版本和日志行为不能迁移为 1.0 需求。

## Out of Scope

- Intel、Universal 2、macOS 13 及更早系统。
- 面向其他用户的公开分发、安装文档、DMG／ZIP 发布包和 GitHub Releases。
- Apple Developer Program、Developer ID、notarization、stapling，以及无警告 Gatekeeper 安装体验。
- Mac App Store 和 App Sandbox 分发。
- 独立 CLI、headless 模式、daemon、localhost Server、公开 API 和第三方自动化接口。
- 公共 Skill registry、SkillYard 账号体系、云端数据库或市场运营。
- 私有 GitHub 登录、token 管理和私有仓库 API。
- 抓取普通网页、社交媒体或论坛帖子并猜测安装来源。
- 执行 `npx skills`、`gh skill`、Lark CLI、任意 shell 命令或官方安装命令 Adapter。
- 执行候选 Skill 携带的脚本、二进制或 lifecycle hook。
- 本地或云端 LLM、AI 来源推断、AI Skill 解释和置信度评分。
- Codex、Claude Code、GitHub Copilot 之外的 Mount 目标。
- 自动发现 Supported App、Host Family、Runtime Surface 或品牌层级模型。
- 把共享 `.agents/skills` 作为 Central Store 或新 Mount 目标。
- 把整个 Supported App Skill 根目录替换为软链接。
- 扫描整块磁盘自动发现 Project。
- Project 离线、外置磁盘重连和跨 Mac 路径恢复。
- 文件系统监听、后台轮询、自动 Mount 修复和自动上游检查。
- 自动覆盖、自动改名、一键替换、Mount alias 和 copy fallback。
- 同一 Skill 在同一 Supported App 同时拥有 global 和 project Mount。
- 不同 Host 或 Mount 使用不同版本的受管内容。
- 成员级删除、成员级卸载、保留空 Bundle 和成员级更新排除。
- Source enable／disable 和一步直接更换 canonical Source。
- 本地版本号、Revision、历史版本、旧版选择、Revision 清理和回滚。
- 文件 diff、逐文件变化列表、自动 changelog、行为摘要和依赖图。
- 对 Managed Bundle Directory 外部直接修改的检测、修复或转存。
- Central Store 自定义位置、迁移到其他目录或外置磁盘。
- 备份、恢复、导出、导入、云同步和跨设备迁移。
- “彻底清除 SkillYard”或一键移除全部托管内容与 Mount。
- 活动页、操作历史、日志查看器、日志导出和日志保留设置。
- 使用分析、设备标识、自动崩溃上传和任何遥测设置页。
- `SkillYard.app` 的自动或手动在线更新检查、下载、签名验证和安装。
- 自定义 Bundle Display Name、SkillYard Presentation Label 或 Mount 目录名。
- 完整恶意代码扫描、Skill 安全评级或 Agent 执行行为保证。

## Further Notes

### 术语表

| 术语 | 1.0 含义 |
| --- | --- |
| Source | 完整上游或本地来源，负责发现和更新能力 |
| Source Catalog | 最近一次成功获取的 Source 可用成员目录 |
| Bundle | Central Store 中的本地已安装组，也是完整更新和级联删除范围 |
| Skill Member | Bundle 中可以独立展示和 Mount 的成员 |
| Current Content | Bundle 当前唯一有效的完整受管内容树 |
| `current` | 指向 Current Content 的 Bundle 级软链接，也是安装和更新生效点 |
| Mount | Supported App 或 Project Skill 目录中指向稳定成员路径的软链接 |
| Project | 用户显式登记、可以承载 project Mount 的本地项目 |
| Takeover | 把已有本地安装交给 SkillYard 统一管理的确认事务 |
| Management Evidence | receipt、lock、manifest、已核验 Adapter 结果等确定性本地证据 |
| Installation Chain | 内容如何到达本机的安装履历，不代表当前管理权 |
| Unknown Provenance | 无法确认 Source，但仍可以 Takeover 的来源状态 |
| Mount Conflict | 目标路径被其他或身份不明内容占用，因而禁止写入 |
| Mount Drift | 已登记 Mount 被外部删除、替换或改指其他内容 |
| Cascading Delete | 删除整个 Bundle 及其全部受管成员和 Mount 的危险操作 |
| Filesystem Transaction Journal | 保存文件系统阶段和生效点、用于中断恢复的持久记录 |

### 已收敛的细节

- GitHub Bundle Display Name 在本 PRD 中收敛为 canonical `owner/repository`，例如 `mattpocock/skills`。它只用于 SkillYard 展示，不改变 Skill Name 或 Mount 目录；现有产品契约中的 `repository slug` 表述应按这个明确值同步。
- GitHub 新 Bundle 的首次直接安装建立已采用 commit；向已有 Bundle 补装成员不会推进既有基线，只有完整 Bundle Update 成功后才推进，因此补装后仍可能显示整个 Bundle 可更新。现有细化文档中笼统的“安装或更新时保存 commit”应按这三个场景同步拆开。
- 普通本地目录是一次性导入；Editable Local Source 必须由用户明确选择，并通过独立的主动检查动作读取变化。
- Supported App 的“可发现”采用文件系统契约验收，不把启动真实 Agent 应用纳入自动化业务测试。
- 三个 Supported App 的只读 Plugin、built-in 和安装检测证据必须在实施计划中列成封闭表；不能识别的内容不猜测归属，也不承诺完整枚举。
- `.skill`、`skills.sh` 数据映射以及其他官方 index／manifest 的输入契约必须在实施计划中列出 1.0 封闭支持清单；未列出的格式不属于 1.0。

### Supported App 当前事实依据

- Codex 官方文档将用户级和项目级 `.agents/skills` 作为推荐发现路径；当前开源 loader 仍兼容 `.codex/skills`，但明确标为 deprecated。因此 Codex 专属 Mount 需要对当前个人电脑实际安装的 Codex 版本做兼容性测试。[Codex Skills 文档](https://learn.chatgpt.com/docs/build-skills)；[Codex loader 源码](https://github.com/openai/codex/blob/main/codex-rs/core-skills/src/loader.rs)
- Claude Code 官方确认用户级和项目级 `.claude/skills` 以及 Plugin Skill，并支持 Skill 目录软链接；没有官方证据表明它直接扫描 `.agents/skills`。[Claude Code Skills 文档](https://code.claude.com/docs/en/skills)
- GitHub Copilot 官方确认用户级 `.copilot/skills`、`.agents/skills`，以及项目级 `.github/skills`、`.agents/skills`、`.claude/skills`。这构成 Codex／Copilot 共享 `.agents/skills` 和 Claude Code／Copilot 共享项目 `.claude/skills` 的交叉可见性。[GitHub Copilot Agent Skills 文档](https://docs.github.com/en/copilot/concepts/agents/about-agent-skills)
- Codex、Claude Code 和 GitHub Copilot 都存在可由 Plugin manifest 或安装记录确认归属的 Plugin Skill，但 Claude Code 与 GitHub Copilot 的 bundled skills 没有已证实的稳定逐成员文件清单；1.0 只展示能够以确定性证据发现的内容。

### 交付解释

- Python CLI、Local Server 和旧测试已经从当前工作区删除。1.0 实施只根据本文记录重建有效场景和不变量，不兼容旧 API、旧数据库或旧运行方式；历史实现由 Git 保存。
- Central Store 是持久用户内容。删除应用、重置设置和手动替换本地构建都不能把它当作缓存清理。
- Filesystem Transaction Journal 和事务临时恢复内容只保证当前操作安全，不构成用户可见的备份、版本或回滚功能。
- 1.0 的目标不是功能演示，而是在产品所有者的一台个人 Mac、三个 Supported App 和本地单用户环境中，把核心 Skill 生命周期做到可预测、可恢复和日常可用。公开分发和 Application Update 在后续确实需要其他用户安装时再单独设计。
