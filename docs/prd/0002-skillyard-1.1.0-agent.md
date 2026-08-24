# SkillYard 1.1.0 产品规格：Agent、Skill 发现、AI 整理与多主题体验

> 状态：已确认，待完成最终增量实施
>
> 版本：1.1.0
>
> 关联需求：GitHub Issue #43、#45
>
> 发布规格：[GitHub Issue #47](https://github.com/ReyYang/SkillYard/issues/47)
>
> 边界：本规格只增加可选的只读智能能力、发现体验、界面语言和视觉主题。Skill 的安装、接管、挂载、更新、解除挂载和删除继续遵守 SkillYard 1.0 产品契约。

## Problem Statement

SkillYard 1.0 已经能管理本机 Bundle、Skill、Source 和 Mount，但用户仍需要自己阅读大量 `SKILL.md` 和同目录文件，才能理解每个 Skill 能做什么、何时使用，以及本机是否已有适合当前需求的 Skill。

当用户想找新的 Skill 时，通常只能描述目标，例如“我想找一个可以审查 API 设计的 Skill”，而不知道准确名称、仓库或发布页。SkillYard 需要同时呈现本机已有内容、已添加 Source 中尚未安装的成员，以及互联网上的其他选择，不能因为本机已有一个近似结果就阻止用户比较线上方案。

已有的非流式对话会让用户在完整回答返回前一直等待，原始 Markdown 文本也不适合作为正式阅读体验。发现结果如果全部混在聊天文本中，用户难以分辨哪些已经安装、哪些来自已添加 Source、哪些只是互联网参考，更无法安全进入既有安装预览。

现有界面能够承载功能，但 Bundle 较多时缺少清晰、稳定且有辨识度的浏览方式。用户已经选定两种明显不同的视觉方向，希望它们成为真正可切换的产品主题，同时又不希望因此维护两套业务逻辑、两套危险操作流程或两套 Agent。

本地模型会显著增加安装包、硬件要求、模型分发和运行维护成本；完整的本地 Agent Harness 又会让 SkillYard 承担搜索引擎、网页抓取、Tool Loop 和任意 Provider 兼容层。两者都超出 1.1.0 所需范围。1.1.0 需要的是一个足够小、可选、可解释的 Agent 能力层，以及建立在同一产品状态之上的两种界面主题。

## Solution

SkillYard 1.1.0 增加一个可选的全局 Agent，并使用用户自己的 API Key 调用三个内建 Provider：OpenAI、智谱 GLM 和 DeepSeek。用户一次只选择一个 Provider 和一个 SkillYard 内置支持的模型，主动验证当前 Key 与模型后，所有 AI 能力共用这一个模型。

全局 Agent 通过唯一的浮动入口打开，并以流式、主题化 Markdown 逐步显示回答。它可以读取 SkillYard 已知且当前可读的 Skill 文件，解释、翻译、比较和查找 Skill，也可以在问题需要时调用当前 Provider 的服务端全网搜索。它不能执行安装、更新、挂载、解除挂载、删除或其他写操作。任何可执行结果都必须进入 SkillYard 已有的预览和确认流程。

SkillYard 增加独立的“发现”页面。用户输入时只筛选本机和已添加 Source 的本地目录；主动提交后立即展示本地结果，同时请求 Provider 搜索全网。结果固定分为“本机已有”“已添加来源”和“全网发现”三个区域。发现页搜索是一次无历史的结构化请求，不加入全局 Agent Session。

SkillYard 同时提供用户主动触发的“AI 整理”，为 Skill 生成固定分类、一句话概要、适用场景和简短使用说明。结果作为可重新生成的派生数据保存在本地 SQLite 中。扫描、安装和更新不会静默调用模型。

界面提供 `Layers` 和 `Ledger` 两个 Theme Preset。每个 Preset 包含一套全局视觉语言和一种 Bundle Library 浏览构图；两者共享路由、领域状态、搜索、筛选、Agent Session 和全部生命周期入口。`Ledger` 是首次使用时的默认主题。这里删除的是 Archive 视觉主题，不影响从 ZIP / `.skill` 归档文件安装的产品能力。

界面语言提供“简体中文”和“English”。AI 输出使用当前界面语言；原始 Skill 文件、Skill Name、Bundle Display Name、Source、路径和 Host Presentation Label 不会被翻译或修改。

## User Stories

### 启用、Provider 与设置

1. 作为 SkillYard 用户，我希望 AI 功能默认不影响现有管理能力，从而可以在不配置 Provider 的情况下继续使用全部非 AI 功能。
2. 作为 SkillYard 用户，我希望在设置中启用或停用 AI，从而明确控制 Skill 内容是否可能发送给模型 Provider。
3. 作为首次启用 AI 的用户，我希望看到一次清楚的数据说明，从而知道用户消息和完成任务所需的 Skill 内容会在本地过滤后发送给所选 Provider。
4. 作为 SkillYard 用户，我希望只选择 OpenAI、智谱 GLM 或 DeepSeek，从而使用经过产品适配的正式接入。
5. 作为 SkillYard 用户，我希望每个 Provider 提供一组依据官方资料维护的内置支持模型，从而可以按质量、速度和成本取舍。
6. 作为 SkillYard 用户，我希望一次只选择一个全局模型，从而不必分别配置聊天、翻译、分类和搜索。
7. 作为 SkillYard 用户，我希望全局模型自动用于全部 AI 能力，从而获得一致行为。
8. 作为 SkillYard 用户，我希望 API Key 保存在 macOS Keychain，而不是 SQLite 或普通配置文件中，从而降低凭据泄漏风险。
9. 作为 SkillYard 用户，我希望 API Key 输入框提供明确的显示与隐藏按钮，从而可以在保存前检查自己是否粘贴正确。
10. 作为 SkillYard 用户，我希望界面只在我主动点击时测试当前 Provider、模型和 Key，从而避免启动或切换设置时产生请求和费用。
11. 作为 SkillYard 用户，我希望“测试连接”完成后明确显示成功或失败，而不是只有短暂加载状态，从而知道 AI 是否已经可用。
12. 作为 SkillYard 用户，我希望连接测试失败时看到足够理解的 Provider 错误，从而可以修正 Key、模型或账号问题。
13. 作为 SkillYard 用户，我希望连接测试失败不影响非 AI 功能，从而仍能管理本机 Skill。
14. 作为 SkillYard 用户，我希望能够保存尚未通过测试的设置，但只有通过测试后才能启用 AI，从而不会把未验证配置误认为可用。
15. 作为 SkillYard 用户，我希望删除 API Key 或停用 AI 后立即停止后续模型调用，从而明确撤销外部数据发送。
16. 作为 SkillYard 用户，我希望更换 Provider、模型或 API Key 后配置回到未验证状态，从而避免沿用旧组合的测试结果。
17. 作为 SkillYard 用户，我希望更换 Provider 或模型不会使已有 AI 整理结果失效，从而不因设置调整产生无意义的重新整理。
18. 作为 SkillYard 用户，我希望 SkillYard 不记录 Token、搜索次数、余额或费用，从而不引入额外的计费管理产品。

### 全局 Agent 与流式回答

19. 作为 SkillYard 用户，我希望所有页面都能打开同一个简洁对话入口，从而随时询问当前看到的内容。
20. 作为 SkillYard 用户，我希望入口使用固定的抽象路径图标，而不是通用“Agent”文字，从而保持产品辨识度。
21. 作为 SkillYard 用户，我希望对话窗口足以阅读较长回答，同时不完全遮挡当前页面，从而可以边看页面边提问。
22. 作为 SkillYard 用户，我希望点击窗口外部、鼠标失焦或切换页面都不会关闭对话，从而不会意外丢失 Session。
23. 作为 SkillYard 用户，我希望收起抽屉时保留对话，只有明确点击“结束会话”才销毁 Session，从而清楚掌握 Session 生命周期。
24. 作为 SkillYard 用户，我希望 Agent 默认理解当前页面，从而不必反复说明正在查看哪个 Bundle、Skill、Source 或设置项。
25. 作为 SkillYard 用户，我希望在对话框保持打开时切换页面后继续同一段对话，从而让后续问题使用最新页面上下文。
26. 作为 SkillYard 用户，我希望同时只有一个对话 Session，从而不需要管理线程、历史记录或多个聊天窗口。
27. 作为 SkillYard 用户，我希望点击“结束会话”就销毁 Session，从而明确知道这段上下文已经被丢弃。
28. 作为 SkillYard 用户，我希望结束会话后再次打开对话框时得到空白 Session，从而不会意外带入旧问题。
29. 作为 SkillYard 用户，我希望应用重启后不恢复聊天，从而不在本地形成隐含的聊天历史。
30. 作为 SkillYard 用户，我希望回答在生成过程中逐步显示，从而无需等待完整响应返回。
31. 作为 SkillYard 用户，我希望流式回答中的标题、列表、引用、表格、链接和代码块得到适合当前主题的排版，从而不看到原始 Markdown。
32. 作为 SkillYard 用户，我希望未闭合的流式 Markdown 仍能稳定显示，从而不会在回答生成期间频繁出现破碎格式。
33. 作为 SkillYard 用户，我希望回答生成期间不能再次发送消息，从而避免同一 Session 中出现并发回答和顺序混乱。
34. 作为 SkillYard 用户，我希望页面导航不会中断当前回答，从而可以在等待时继续浏览。
35. 作为 SkillYard 用户，我希望“结束会话”会终止当前回答，而单纯收起抽屉不会打断生成，从而让中断语义明确且费用可预期。
36. 作为 SkillYard 用户，我希望流式请求失败时保留已经看到的部分并标记“回答未完成”，从而不会突然丢失可用内容。
37. 作为 SkillYard 用户，我希望未完成回答不进入下一轮上下文，从而不会把残缺内容当成可靠结论。
38. 作为 SkillYard 用户，我希望 SkillYard 不自动重试或切换 Provider，从而保持费用和行为可预期。
39. 作为 SkillYard 用户，我希望 Agent 可以解释当前 Skill 的用途，从而不必自己阅读全部文件。
40. 作为 SkillYard 用户，我希望 Agent 可以比较多个本地 Skill，从而判断哪个更适合当前任务。
41. 作为 SkillYard 用户，我希望 Agent 可以把 Skill 内容解释成当前界面语言，从而更容易理解外语 Skill。
42. 作为 SkillYard 用户，我希望 Agent 可以分析任何 SkillYard 能读取文件的 Skill，从而不因其属于受管 Bundle、待接管安装、项目仓库、Host 内置内容或官方插件而失去只读分析能力。
43. 作为 SkillYard 用户，我希望 Agent 把 Skill 文件当作不可信参考资料，从而不会执行其中要求修改规则、读取其他文件或发起网络请求的指令。
44. 作为 SkillYard 用户，我希望 Agent 只能回答、解释、搜索、比较或引导页面，从而不会在对话中直接改变本机状态。
45. 作为 SkillYard 用户，我希望回答中的引用和可安装候选使用结构化卡片展示，从而不会把模型生成的 Markdown 按钮误认为真实产品操作。
46. 作为 SkillYard 用户，我希望可执行建议进入已有页面，从而仍能查看真实影响预览并主动确认。
47. 作为 SkillYard 用户，我希望 Agent 无法绕过现有安装和生命周期确认，从而不会因自然语言误解造成文件变化。

### 发现页、本机结果、Source 与全网结果

48. 作为想找 Skill 的用户，我希望有一个独立的“发现”页面，从而可以专注搜索，而不是必须先进入某个 Bundle 或 Source。
49. 作为想找 Skill 的用户，我希望用自然语言描述需求，从而不必知道准确名称或仓库地址。
50. 作为想找 Skill 的用户，我希望只输入关键字时先筛选本机和已添加 Source 的本地目录，从而立即看到结果且不产生外部请求。
51. 作为想找 Skill 的用户，我希望主动提交后本机结果和全网搜索并行进行，从而即使本机有 TDD Skill，也可以比较线上其他选择。
52. 作为想找 Skill 的用户，我希望本机和已添加 Source 的结果先显示、全网结果随后补充，从而不必等待网络搜索才能开始浏览。
53. 作为想找 Skill 的用户，我希望结果明确分为“本机已有”“已添加来源”和“全网发现”，从而理解每个结果当前处于什么位置。
54. 作为想找 Skill 的用户，我希望没有结果的分区仍显示清楚的空状态，从而知道该范围确实完成了搜索。
55. 作为想找 Skill 的用户，我希望“本机已有”包含已安装、已接管和只读展示的 Skill，从而可以优先复用当前电脑上的能力。
56. 作为想找 Skill 的用户，我希望“已添加来源”搜索 Source 最近一次成功加载并保存在本地的成员目录，从而发现尚未安装的成员。
57. 作为想找 Skill 的用户，我希望只添加 Source、尚未安装任何成员时仍能看到其可用 Skill，从而不必先创建 Bundle。
58. 作为想找 Skill 的用户，我希望搜索 Source 目录不会自动创建 Bundle、Skill Member 或 Mount，从而保持发现与安装分离。
59. 作为想找 Skill 的用户，我希望 Source 从未成功加载时看到明确空状态或重新加载入口，从而不会把未知内容伪装成空目录。
60. 作为想找 Skill 的用户，我希望全网结果来自当前 Provider 的服务端搜索，从而获得不局限于单一 registry 的公开互联网结果。
61. 作为想找 Skill 的用户，我希望全网结果附带可打开的真实来源链接或引用，从而自行核验作者、仓库和发布页。
62. 作为想找 Skill 的用户，我希望同一 canonical Source 或仓库只显示一次，从而不会在三个分区看到重复卡片。
63. 作为想找 Skill 的用户，我希望重复结果合并本机状态、Source 状态和线上补充信息，从而在一个位置看到完整事实。
64. 作为想找 Skill 的用户，我希望 SkillYard 不显示自创的相关度、热度或新鲜度评分，从而不会把模型回答包装成确定性排名。
65. 作为想找 Skill 的用户，我希望可解析为现有 Source 类型的结果进入标准安装预览，从而复用已经验证的下载、内容检查和确认流程。
66. 作为想找 Skill 的用户，我希望普通网页、论坛帖子或暂时无法解析的结果只作为参考，从而不会被误当作可直接安装的包。
67. 作为想找 Skill 的用户，我希望 Agent 不能直接运行 `npx`、Shell、CLI 或网页脚本，从而不会扩大本机代码执行边界。
68. 作为想找 Skill 的用户，我希望发现页搜索不加入全局聊天 Session，从而不会让一次搜索改变后续对话语境。
69. 作为想找 Skill 的用户，我希望发现页和全局 Agent 共用当前 Provider、模型和隐私过滤，从而不需要配置第二套 AI。
70. 作为 SkillYard 用户，我希望 Bundle Library 的搜索框只过滤本机清单，从而不会因为整理现有内容而意外联网。
71. 作为 SkillYard 用户，我希望 Source 管理继续负责添加、维护、重新加载和删除远端来源，从而不会被“发现”页面取代。
72. 作为 SkillYard 用户，我希望一次全网搜索失败只影响本次结果，从而不改变 Inventory、Source、Bundle、Mount 或 Project。
73. 作为 SkillYard 用户，我希望 Provider 网络不可用时看到普通 Agent 错误，从而不需要 SkillYard 实现额外网络诊断或自动切换。

### AI 整理、概要与分类

74. 作为拥有很多 Skill 的用户，我希望主动点击“AI 整理”，从而批量补齐缺失或已过时的说明。
75. 作为 SkillYard 用户，我希望扫描、安装和更新不会自动调用模型，从而可以预期何时会产生外部请求和费用。
76. 作为 SkillYard 用户，我希望 AI 整理在后台进行，从而不必停留在当前页面等待。
77. 作为 SkillYard 用户，我希望 AI 整理不显示任务列表、进度条或取消入口，从而保持界面简单。
78. 作为 SkillYard 用户，我希望触发后只看到一次轻量反馈，从而知道请求已开始但不被后台细节打扰。
79. 作为 SkillYard 用户，我希望某个 Skill 整理失败时其他 Skill 仍能继续，从而不会因单项 Provider 错误阻塞整批结果。
80. 作为 SkillYard 用户，我希望失败或未完成的 Skill 保持“待重新整理”，从而可以稍后再次触发。
81. 作为 SkillYard 用户，我希望可以在 Skill 详情中单独重新生成，从而只刷新当前 Skill 的派生信息。
82. 作为 SkillYard 用户，我希望每个 Skill 只有一个固定主分类，从而可以稳定筛选，而不是得到每次不同的自由标签。
83. 作为 SkillYard 用户，我希望每个 Skill 有一句话概要，从而在 Bundle 清单中快速判断用途。
84. 作为 SkillYard 用户，我希望每个 Skill 有两到四个适用场景，从而知道何时应该使用它。
85. 作为 SkillYard 用户，我希望每个 Skill 有简短使用说明，从而知道如何触发或配合 Agent 使用。
86. 作为 SkillYard 用户，我希望原始文件没有提供的信息不会被补写成事实，从而避免 AI 生成虚构用法。
87. 作为 SkillYard 用户，我希望 AI 结果是只读派生数据，从而不需要维护手工编辑、锁定和模型输出之间的冲突。
88. 作为 SkillYard 用户，我希望重新生成时直接替换旧结果，从而不需要管理 AI 内容版本。
89. 作为 SkillYard 用户，我希望界面不显示置信度、推理过程、生成模型和自由标签，从而只看到完成任务需要的信息。
90. 作为 SkillYard 用户，我希望单成员 Bundle 直接显示该 Skill 的分类和概要，从而不必额外点击“查看成员”。
91. 作为 SkillYard 用户，我希望多成员 Bundle 仍按 Bundle 展示，并在查看成员时看到各 Skill 的分类和概要，从而保留现有分组模型。
92. 作为 SkillYard 用户，我希望按分类筛选时看到包含匹配 Skill 的 Bundle，从而不破坏 Bundle 作为主清单单位。
93. 作为 SkillYard 用户，我希望展开多成员 Bundle 后只突出或展示符合当前分类的 Skill，从而理解该 Bundle 为什么出现在结果中。
94. 作为 SkillYard 用户，我希望分类筛选不改变更新、挂载和删除的生命周期边界，从而不产生第二套管理模型。

### 两套主题与 Bundle Library

95. 作为 SkillYard 用户，我希望在设置中选择 `Layers` 或 `Ledger`，从而按自己偏好的方式浏览 Bundle。
96. 作为首次使用的用户，我希望默认看到 `Ledger`，从而在大量 Bundle、长名称和多状态下获得最稳定的浏览体验。
97. 作为 SkillYard 用户，我希望主题选择在重启后保留，从而不必反复设置。
98. 作为 SkillYard 用户，我希望切换主题立即生效，从而可以直接比较两种体验。
99. 作为 SkillYard 用户，我希望主题切换保留当前路由、选中 Bundle、搜索、筛选和排序，从而不会打断正在进行的浏览。
100. 作为 SkillYard 用户，我希望主题切换保留已打开的 Agent Session，从而不会因视觉变化丢失对话。
101. 作为 SkillYard 用户，我希望主题切换保留未完成表单和操作状态，从而不会意外丢弃输入。
102. 作为 SkillYard 用户，我希望设置只呈现已经定稿的 `Layers` 与 `Ledger`，从而不必在未采用的视觉方向之间选择。
103. 作为喜欢空间层级的用户，我希望 `Layers` 使用层叠卡片和当前 Bundle 构图，从而快速理解集合与当前选择。
104. 作为需要高效管理大量 Bundle 的用户，我希望 `Ledger` 使用主从清单和详情面板，从而快速比较名称、状态和内容。
105. 作为 SkillYard 用户，我希望两种 Library View 都能完成选择、搜索、筛选、打开 Bundle 和进入公共操作，从而不会因主题失去功能。
106. 作为 SkillYard 用户，我希望两种主题共用同一份 Bundle、Skill、Source 和 Mount 状态，从而不会形成不同步的数据副本。
107. 作为 SkillYard 用户，我希望安装、接管、Bundle 与 Skill 详情、设置、确认弹窗和 Agent 在两种主题中保持相同的信息层级与操作顺序，从而不必重新学习产品。
108. 作为 SkillYard 用户，我希望公共页面使用当前主题的颜色、字体、边框、阴影、图标和动效，从而不会出现视觉割裂。
109. 作为 SkillYard 用户，我希望主题只决定视觉和 Bundle Library 构图，从而不会改变生命周期行为或结果。
110. 作为键盘用户，我希望每种 Library View 都具有清晰焦点和可预测的选择顺序，从而不依赖鼠标操作。
111. 作为使用较窄窗口的用户，我希望两种 Library View 都保持可读并提供完整主操作，从而不会因为构图被裁切而失去能力。
112. 作为拥有长 Bundle 名称的用户，我希望两种主题都能清楚显示或合理截断名称，从而不会破坏布局。
113. 作为拥有很多 Bundle 的用户，我希望两种主题都能稳定浏览大量数据，从而不会因视觉效果造成性能或可访问性问题。
114. 作为 SkillYard 用户，我希望 Agent Markdown 在两种主题中使用同一语义结构并适配各自 token，从而既保持一致阅读能力又融入当前主题。

### 界面语言与派生数据生命周期

115. 作为 SkillYard 用户，我希望在设置中选择“简体中文”或“English”，从而使用自己熟悉的界面语言。
116. 作为 SkillYard 用户，我希望语言选项始终显示为“简体中文”和“English”，从而不会因为当前界面语言而改变自身名称。
117. 作为 SkillYard 用户，我希望切换语言后界面立即更新并在下次启动时保留，从而获得一致体验。
118. 作为 SkillYard 用户，我希望 AI 对话和新生成的整理结果使用当前界面语言，从而不需要单独设置模型语言。
119. 作为 Skill 作者，我希望语言切换不会修改、翻译或覆盖原始 `SKILL.md` 和其他文件，从而保留 Source 内容。
120. 作为 SkillYard 用户，我希望切换语言后旧 AI 结果仍可查看但标记为“待重新整理”，从而不会自动产生批量 API 调用。
121. 作为 SkillYard 用户，我希望下次主动整理时用当前语言替换旧结果，从而不保存两套平行译文。
122. 作为 SkillYard 用户，我希望 Skill 内容发生变化后旧 AI 结果标记为“待重新整理”，从而不会把旧说明误认为当前内容。
123. 作为 SkillYard 用户，我希望已完成的 AI 整理结果在重启后仍然存在，从而不必反复调用模型。
124. 作为 SkillYard 用户，我希望聊天消息和搜索临时总结不会在重启后保留，从而让持久数据只包含真正用于清单的派生信息。

## Implementation Decisions

### 版本与产品边界

- 版本号保持 `1.1.0`。当前已发布版本仍是 `1.0.1`；已有 1.1.0 代码只是未发布实施基线，最终版本在完成本规格全部增量后统一验收和发布。
- SkillYard 1.0 的 Bundle、Skill Member、Source、Takeover、Current Content、Mount 和删除语义保持不变。
- Agent 是只读智能层，不是新的 Local Lifecycle Authority。它不能获得安装、接管、更新、挂载、解除挂载、删除、文件写入或 SQLite 任意写入能力。
- 所有生命周期动作继续由现有 Lifecycle Core 和影响预览控制；Agent 与发现页只能导航或构造进入既有流程所需的非执行输入。
- AI 是可选能力。没有 Provider 配置、Key 无效或 Provider 暂时不可用时，非 AI 产品必须完整可用。
- 最新实现必须原地替换当前非流式 Agent 与串行搜索行为，不新增 `v2`、`legacy`、`next` 或第二套生产入口。

### Provider 与全局模型

- 1.1.0 只内建 OpenAI、智谱 GLM 和 DeepSeek，不提供任意 Provider、自定义 Base URL 或自定义模型 ID。
- 每个 Provider 使用一个薄 Adapter，负责其官方请求、服务端搜索、流式协议、结构化结果、引用和错误格式；不建立通用 Tool Loop 或 Provider 能力自动发现。
- OpenAI 使用 Responses API 和原生 `web_search`。
- 智谱 GLM 使用官方 Chat Completions 扩展和 `type: "web_search"`。
- DeepSeek 是唯一特殊兼容接入，使用官方 Anthropic-compatible Endpoint。这个实现不能扩张成“支持所有 Anthropic-compatible Provider”。
- 用户全局只选择一个 Provider 和一个模型。聊天、Skill 概要、分类、使用说明、语言输出、来源调查和全网 Skill 搜索全部使用这个模型。
- 不提供按功能选模型、自动 fallback、后台切换或多模型路由。
- 模型目录随 SkillYard 版本静态发布，并且只列出支持对话流式输出和对应 Provider 必需能力的模型。Provider `/models` 不能动态增加用户可见选项。
- OpenAI 的候选为 `gpt-5.6-sol`、`gpt-5.6-terra`、`gpt-5.6-luna`、`gpt-5.4-mini`、`gpt-5.5`，默认候选为 `gpt-5.6-terra`。
- 智谱 GLM 的候选为 `glm-5.2`、`glm-5.1`、`glm-4.7`、`glm-4.7-flashx`、`glm-4.7-flash`，默认候选为 `glm-4.7`。
- DeepSeek 的候选为 `deepseek-v4-flash`、`deepseek-v4-pro`，默认候选为 `deepseek-v4-flash`。
- 上述列表是随应用发布的内置支持目录，不表示维护者持有全部 Provider 凭据，也不表示每个用户账号天然拥有全部模型权限。
- 用户只验证当前选择的模型。SkillYard 不批量请求其他候选，也不展示候选、部分可用或维护者认证状态。

### 凭据、设置与错误

- API Key 只保存在 macOS Keychain。SQLite 保存 Provider、模型、当前组合是否已通过连接测试、AI 开关、首次披露状态、界面语言、Theme Preset 和其他非敏感偏好。
- API Key 输入框具有显式显示与隐藏控制；默认隐藏，显示状态不持久化。
- “测试连接”只由用户点击触发，并使用当前 Key 对当前模型发送两次可能计费的真实请求：一次固定 Schema 输出和一次必须返回真实 URL 的服务端搜索。
- 设置页在测试前说明会发起两次真实请求；测试结束后持久显示明确成功或失败状态，不依赖悬停、短暂动画或日志。
- 只有两项验证都通过，当前 Provider、模型和 Key 组合才能启用 AI。测试失败时不产生“部分可用”状态。
- 更换 Provider、模型或 API Key 后，当前配置立即回到未验证状态，但不会使已保存的 AI 整理结果失效。
- SkillYard 不在启动、后台或切换设置时自动验证，也不使用用户的 Key 批量测试其他模型。
- SkillYard 展示足够理解的 Provider 错误，但不实现网络质量诊断、离线队列、自动重试、账号余额管理或 Provider fallback。
- SkillYard 不记录 Token、搜索次数、余额或费用。

### 一个全局 Agent Session

- 应用同时最多存在一个 Agent Session。
- Session 只保存在当前应用内存中，不写入 SQLite，不恢复，不形成聊天历史或线程列表。
- 打开对话入口时创建 Session；收起抽屉保留 Session，只有明确点击“结束会话”才终止尚未完成的流式请求、结束 Session 并丢弃消息。
- 点击窗口外、鼠标失焦和页面导航不能关闭对话。
- 页面导航不会结束已打开的 Session。每次发送消息时使用当时最新的页面类型和稳定领域 ID 作为默认上下文。
- 前端不能向 Agent 提交任意本机路径。Lifecycle Core 根据稳定 ID 解析 Bundle、Skill、Source、Mount 或只读 Inventory 内容。
- 当前页面只是默认上下文，不限制 Agent 在用户明确询问时读取其他 SkillYard 已知 Skill。
- 发现页搜索不属于 Agent Session，不读取或写入对话历史；它只是共用当前 Provider、模型、过滤边界和搜索 Adapter。

### 流式回答与 Markdown

- 只有全局对话使用流式输出。连接测试、发现页结构化搜索和后台 AI 整理继续使用适合其任务的完成结果或分区状态。
- OpenAI、智谱 GLM 和 DeepSeek Adapter 各自解析官方流式协议，并归一为正文增量、完成和失败三类内部事件；Provider 私有事件不能进入 React。
- Lifecycle Core 通过 typed Tauri Channel 按顺序发送事件。React 不使用无类型全局事件总线，也不直接连接 Provider。
- 当前 `askAgent` 生产入口原地改为流式合同，不保留并行的非流式聊天入口。
- 一个回答生成期间不能发送第二条用户消息。页面导航和收起抽屉不打断生成；“结束会话”会终止生成并销毁 Session。
- 正文增量使用 Streamdown 渲染未完成 Markdown。SkillYard 只增加一层薄的 `AgentMarkdown` 包装，不引入第二套聊天 Runtime。
- Streamdown 所需 Tailwind 构建能力只服务于 Markdown renderer；现有页面不迁移到 Tailwind，也不引入 AI Elements、assistant-ui 或 shadcn 应用框架。
- Raw HTML 保持禁用；链接只允许明确协议并通过 Tauri 受控方式打开；默认不加载远程或 `data:` 图片；代码块只能展示和复制，不能执行。
- 全网引用、可安装候选和安装预览入口只在完成事件中作为 typed 数据展示，不能由模型通过 Markdown 生成可执行控件。
- 流式请求失败时保留已经显示的正文并标记“回答未完成”；这部分内容不加入下一轮上下文。SkillYard 不自动重试或切换 Provider。
- 两套主题共用同一个 Markdown 语义 renderer，只通过主题 token 改变排版材质。

### Skill 文件读取与隐私

- Agent 可以分析所有 SkillYard 已知且实际可读的 Skill，不以管理权决定是否允许只读分析。
- SkillYard 只从已确认 Skill 根读取完成当前任务所需的普通文本内容；不把二进制、凭据文件或任意外部路径交给模型。
- 发送前执行确定性过滤：阻止 `.env*`、私钥、证书、凭据、数据库等敏感文件，并从允许文本中移除 Token、Authorization Header、带凭据 URL、个人邮箱、用户名和个人绝对路径等本机敏感信息。
- 用户消息、页面上下文、本地搜索候选和 Skill 文件都经过同一过滤边界。无法安全构造请求时，本次 AI 操作停止。
- Skill 内容属于不可信数据。其文本不能改变 System 规则、授权新文件、触发生命周期动作或自行要求联网。
- AI 整理永远不启用 Web Search；全局对话只在问题需要时允许当前 Provider 使用服务端搜索。
- 首次启用 AI 时一次性说明数据会发送给所选 Provider；Provider 自身的保留、计费和地区条款由用户承担。之后不做逐次确认。
- Provider 请求是用户主动启用的功能请求，不是遥测。SkillYard 仍不上传分析事件、设备标识、Inventory 或崩溃报告。

### 发现页、分区搜索与安装交接

- “发现”是独立页面，不取代 Source 管理，也不复用 Bundle Library 的本机过滤框。
- 用户输入时只筛选本机 Inventory 和 Source 最近一次成功加载并保存在 SQLite 的成员目录，不产生 Provider 请求。
- 用户主动提交后，本机与已添加 Source 结果立即显示，同时启动当前 Provider 的服务端全网搜索；本机命中不会阻止全网搜索。
- 结果固定分为“本机已有”“已添加来源”和“全网发现”三个并列区域。无结果区域显示空状态，不能把三类内容混成一个列表。
- 添加了 Source 但未安装任何 Skill 时，其已保存成员仍可出现在“已添加来源”，但搜索本身不能创建 Bundle、Skill Member 或 Mount。
- Source 从未成功加载时没有可搜索成员；界面显示未加载状态并提供既有 Source 重新加载入口，不能静默联网补目录。
- 同一 canonical Source 或仓库只展示一个结果；本地状态、Source 状态和线上补充信息合并到同一结构化卡片。
- SkillYard 使用 Provider 的服务端全网搜索，不自建网页搜索、Crawler、Browser Tool 或 `skills.sh` 专用搜索 Harness。
- SkillYard 不维护搜索评分、热度、新鲜度或二次排序。Provider 返回顺序不成为持久产品数据。
- 在线结果必须保留 Provider 返回的可展示 URL 或引用。缺少可核验来源时不能描述为已确认来源。
- 能被现有 Source Adapter 解析的候选可以交给既有安装预览。无法解析的网页只显示为参考信息。
- Agent 与发现页都不能执行 `npx skills find`、其他 CLI、Shell 命令或网页脚本，也不能直接确认安装。
- 发现页搜索是无历史请求；结果和临时总结不写入 Agent Session 或 SQLite。

### AI 整理数据与执行

- AI 整理为每个 Skill 生成四项只读派生数据：一个固定分类、一句话概要、两到四个适用场景、简短使用说明。
- 分类只能从以下列表选择一项：
  1. 开发与工程
  2. 系统与运维
  3. 效率与自动化
  4. 数据与分析
  5. 产品与业务
  6. 研究与学习
  7. 写作与沟通
  8. 设计与创意
  9. 安全与合规
  10. 其他
- 分类是 SkillYard 自己的稳定 Taxonomy，只参考 Codex 插件目录的表达方式，不复用其 ID、Schema 或发布分类。
- AI 必须返回列表中的一个分类，不能创建自由分类、标签或多分类。
- SQLite 保存完成的派生结果、生成语言和对应 Skill 内容 fingerprint。API Key、完整 Prompt、完整 Response、对话和临时搜索总结不保存。
- Skill 内容 fingerprint 或界面语言变化时，旧结果继续显示但状态变为“待重新整理”。
- Provider 或模型变化不使现有结果过时。
- 用户不能编辑或锁定 AI 结果；重新生成直接替换旧结果，不保留 AI 内容版本。
- 原始 Skill 内容信息不足时，字段应保持简短或说明无法从内容确认，不能补造事实。
- 批量 AI 整理只由用户主动触发，处理当前缺失或已过时的 Skill。
- 执行发生在后台；界面不提供进度条、任务列表或取消操作，只给出一次轻量“已在后台开始”反馈。
- 每个 Skill 独立生成和保存。单项失败不阻塞其他项，失败项继续处于“待重新整理”。
- 扫描、安装、接管、更新、应用启动和语言切换都不能静默启动 AI 整理。
- 完成结果保存后自然出现在清单和详情中；未完成内容不能覆盖之前的可用结果。

### Bundle 清单、分类与两套 Theme Preset

- Bundle 继续是主清单单位，不把 Skill 平铺为顶层生命周期对象。
- 单成员 Bundle 直接展示该 Skill 的分类和一句话概要，点击后进入 Skill 详情。
- 多成员 Bundle 保持成员入口；成员列表和 Skill 详情展示 AI 派生信息。
- 分类筛选返回包含至少一个匹配 Skill 的 Bundle；展开后展示与当前分类匹配的成员。
- 分类筛选只改变只读展示，不改变 Bundle Update、Batch Mount、解除挂载或 Cascading Delete 的生命周期边界。
- 用户界面把 `Layers` 和 `Ledger` 作为两个完整 Theme Preset，不向用户暴露独立组合 `Appearance Theme` 与 `Library View` 的高级设置。
- 每个 Theme Preset 内部由两层组成：
  - `Appearance Theme` 作用于整个应用的背景、颜色、字体、边框、阴影、控件、弹窗、导航、图标和动效；
  - `Library View` 只负责 Bundle Library 的浏览构图。
- `Ledger` 是没有已保存偏好时的默认 Theme Preset。
- 预发布开发版本若已保存 `archive` 主题偏好，升级时一次性归一化为 `Ledger`；产品不保留 `Archive` 主题入口或运行时兼容分支，这不影响 ZIP / `.skill` 归档安装能力。
- `Layers` 使用层叠卡片和当前 Bundle 构图；`Ledger` 使用 Bundle 列表和详情面板构图。
- 两个 Library renderer 消费同一份领域数据、当前路由、选中 Bundle、搜索、筛选、排序和操作入口，不能各自保存 Bundle 状态。
- 切换 Theme Preset 时保留当前路由、选中 Bundle、搜索、筛选、排序、已打开 Agent Session，以及尚未确认的表单和操作状态。
- Library renderer 可以保留纯展示状态，例如 Layers 的当前纸张焦点；该状态不能进入 Bundle 领域模型或改变操作结果。
- 安装、接管、Bundle 与 Skill 详情、Source、Mount、设置、恢复、确认弹窗和 Agent 使用同一套信息架构与业务组件，只应用当前主题 token。
- 1.1.0 不建立全应用 Skin Engine，也不因主题复制路由、生命周期页面或 Agent。
- 两套主题必须提供相同主操作能力，并分别满足文字对比度、焦点可见性、键盘导航、窄窗口、长名称和大量 Bundle 的基本可用性。

### 界面语言

- 设置页只提供“简体中文”和“English”，这两个选项名称不随当前界面语言变化。
- 切换后所有 SkillYard 自有 UI 文案立即更新并持久化偏好。
- Agent 回答和新 AI 整理结果使用当前界面语言。
- 原始 `SKILL.md`、Skill Name、Bundle Display Name、Source、路径、Theme Preset 名称和 Host Presentation Label 不因语言切换而修改。
- 不保存中英双语派生副本。语言切换只把旧派生结果标记为待重新整理，直到用户主动触发。

## Testing Decisions

### 最高测试 seam

- 主要业务验收继续通过 `SkillYardApplication` 进入。这是现有 Lifecycle Core 的唯一公开业务 seam，typed Tauri command 只做薄适配。
- Agent Provider 的网络边界像现有 Source Transport 一样可替换，使应用级测试能够使用确定性 Fake Server，同时仍经过真实 Skill 文件读取、敏感过滤、SQLite 和重启后的持久状态。
- 不为 Agent、发现页或主题建立第二个可绕过 `SkillYardApplication` 的生产入口，也不以私有函数测试代替应用行为。

### 应用级行为

- 验证未配置 AI 时现有 1.0 主流程不受影响。
- 验证全局只有一个 Provider 和模型，并被聊天、AI 整理、翻译和搜索共同使用。
- 验证 API Key 通过 Keychain 边界保存，SQLite 和可观察错误中不出现明文 Key。
- 验证 API Key 可由用户临时显示检查，显示状态不持久化。
- 验证连接测试只在用户点击后执行，只请求当前模型，并且固定 Schema 与服务端搜索都通过后才启用 AI。
- 验证测试连接具有稳定、可回读的成功或失败反馈。
- 验证更换 Provider、模型或 API Key 后 AI 回到未验证状态，但已有 AI 整理结果不失效。
- 验证聊天 Session 在页面导航和收起抽屉后保留，在明确结束会话和重启后丢弃。
- 验证三个 Provider 的正文增量按顺序进入同一回答，完成前即可被用户看到，完成后才把该回答加入下一轮上下文。
- 验证页面导航和收起抽屉不打断流式回答，“结束会话”会终止请求；中途失败保留可见正文但不污染下一轮上下文。
- 验证页面只提交稳定 ID，Lifecycle Core 解析实际上下文，前端不能注入任意路径。
- 验证受管、待接管、项目维护、Host 内置和官方插件 Skill 在文件可读时都能被分析。
- 验证 Skill 中的 Prompt Injection 文本不能获得额外文件、网络或生命周期权限。
- 验证敏感文件被阻止，允许文本中的个人路径、邮箱、Token 和凭据 URL 被移除。
- 验证只输入搜索关键字时不调用 Provider，主动提交后即使本机命中也会执行 Provider Web Search。
- 验证“本机已有”“已添加来源”和“全网发现”分别展示，未安装 Source 成员可以命中，canonical 重复结果会合并。
- 验证发现页请求不进入 Agent Session，也不保存临时搜索总结。
- 验证搜索结果只能进入既有安装预览，Agent 无法直接产生本机写入。
- 验证 AI 整理不会在扫描、安装、更新、启动和语言切换时自动执行。
- 验证后台整理的单项失败不会阻塞其他项，完成结果可在重启后读取。
- 验证内容 fingerprint 或语言变化产生“待重新整理”，而 Provider 或模型变化不会。
- 验证固定分类只能返回十个允许值之一，概要、场景和使用说明遵守字段边界。
- 验证分类筛选仍以 Bundle 为主清单，并正确处理单成员与多成员 Bundle。
- 验证两套 Theme Preset 共用相同领域状态和生命周期入口。
- 验证切换主题保留路由、当前 Bundle、搜索、筛选、Agent Session 和未完成表单。
- 验证 schema 30 开发数据库中的 `archive` 偏好经正式 application 启动升级为 `Ledger`，持久值归一化且再次重启后稳定。
- 验证语言切换立即更新 UI、持久化设置、改变后续 AI 输出语言且不修改 Skill 文件。

### Provider 合同、typed IPC 与 UI

- 普通离线 CI 使用 Fake Server 覆盖三个 Adapter 的请求格式、SSE 分块与结束事件、固定 Schema 解析、搜索引用归一、错误映射和模型身份检查，不需要真实 API Key。
- typed client 测试验证每项新命令只提交完成任务所需的设置、稳定 ID、消息、无历史搜索请求和 typed Channel，不提交任意路径、Key 或前端推断的本机内容。
- React 行为测试覆盖设置、API Key 显示、连接反馈、一次性披露、全局对话开合、流式 Markdown、页面上下文、三个搜索分区、后台整理反馈、分类筛选、单成员 Bundle 展示、过时标记、两主题切换和中英文切换。
- UI 测试使用假的 typed client 观察用户可见状态；不得通过直接调用私有 Rust 步骤证明产品行为。
- Streamdown 包装测试覆盖 Raw HTML、危险协议、远程图片、代码块和未闭合 Markdown，不测试第三方库内部解析算法。
- 两个 Library renderer 分别验证空状态、少量 Bundle、大量 Bundle、长名称、窄窗口、键盘选择和公共操作入口。
- 窗口行为验证 `1180 × 840` reference 与 `760 × 560` compact 两种状态：前者保持参考坐标系，后者保持可读字号、完整主操作和单一正文滚动区；两种状态都复用同一 renderer 与产品状态。
- 公共安装、接管、更新、Mount 和 Cascading Delete 流程只需验证使用当前主题 token，不为每个主题重复 Rust 生命周期测试。

### 用户连接测试与最终应用验收

- 用户点击“测试连接”时，使用自己的 Key 验证当前选择的一个模型。OpenAI 验证 Responses `web_search`，GLM 验证 `type: "web_search"`，DeepSeek 验证 Anthropic Messages 服务端搜索。
- 测试必须同时得到符合固定 Schema 的结果和可打开的真实 URL；只支持部分能力、发生可识别的静默 fallback 或缺少来源时，当前配置不能启用 AI。
- 连接测试不遍历同一 Provider 的其他模型，也不把结果上传给 SkillYard。维护者不需要持有全部候选模型的访问权限。
- 真实 Key 不进入普通 CI、外部 PR、测试夹具、公开日志或截图。
- 自动化通过后先冻结生产构建与 SHA-256；需要真实 Provider 的独立产品验收继续验证 Keychain、用户主动连接测试、真实流式回答、发现页和 AI 整理，但不得与本轮两主题视觉迭代混成一个反复重建环境的门禁。
- 两主题视觉先由本地确定性五 Bundle 夹具覆盖 `Layers`、`Ledger`、排序、设置、Agent、`1180 × 840` 与 `760 × 560`；冻结候选后只运行至多一次 Apple Silicon macOS VM 冒烟，确认真实 WebView、原生窗口边界和主题状态保持，不为每套主题重复完整生命周期验收。
- 本地或 VM 冒烟命中 P0–P2 后，必须回到本地修复并生成新的候选；不能对旧候选连续申请相似 VM 操作来碰运气。
- VM 验收不得把真实 Key、完整本机路径、用户消息或 Skill 内容写入公开截图和日志。
- 1.0 的安装、接管、Mount、整 Bundle 解除挂载、Update 和 Cascading Delete 回归测试必须继续通过，证明 Agent、发现和主题没有扩张生命周期权限。

## Out of Scope

- 捆绑、下载或运行本地 LLM。
- Qwen 或其他本地模型运行时、模型量化、GPU/内存探测和硬件分档。
- 通用 Agent Harness、任意 Tool Loop、Browser、Crawler、代码执行或 Shell 执行。
- 除 OpenAI、智谱 GLM、DeepSeek 以外的 Provider。
- 任意 OpenAI-compatible 或 Anthropic-compatible Base URL。
- 自定义模型 ID、动态 `/models` 选择器或用户维护的模型目录。
- 为不同 AI 功能选择不同模型、自动 fallback 或自动 Provider 切换。
- Agent 直接安装、接管、挂载、更新、解除挂载、删除或修改 Skill。
- Agent 修改 SQLite、Central Store、Source、Bundle、Mount、Project 或 Host 文件。
- 聊天历史、多个 Session、线程列表、Session 恢复、导出或同步。
- 把发现页搜索变成第二个聊天 Session。
- 自动 AI 整理、定时整理、启动整理、扫描后整理、安装后整理或更新后整理。
- AI 整理进度条、任务中心、取消、暂停和恢复。
- 用户编辑、锁定或版本化 AI 生成结果。
- 自由标签、多分类、置信度、推理过程和模型归因展示。
- 本地向量数据库、Embedding 索引、Rerank、Skill 评分或搜索排序系统。
- SkillYard 自建全网搜索服务、公共 Skill registry 或 `skills.sh` 专用 Agent 搜索协议。
- 网络质量诊断、离线队列、Provider SLA 监控、费用或 Token 统计。
- 自动翻译或改写原始 Skill 文件。
- 简体中文和 English 以外的界面语言。
- 用户自定义主题、自动跟随系统主题、独立组合 Appearance Theme 与 Library View、第三方 Theme 或 Theme Marketplace。
- 为每个主题分别实现路由、生命周期页面、Agent、SQLite 领域状态或完整 Skin Engine。
- 改变 macOS 14+ Apple Silicon、Supported Apps、分发方式或 1.0 生命周期承诺。

## Further Notes

- `1.1.0` 是当前规划版本；已发布的 `1.0.1` 仍是稳定下载版本。仓库中已有的 1.1.0 实施基线不代表最终规格已经交付。
- Issue #43 的“概要和分类”与 Issue #45 的“模糊搜索安装新的 Skill”共享同一个 Agent、Provider 和模型配置，不形成两套 AI 系统。
- 现有已关闭阶段票据描述的是此前非流式、串行本地优先搜索和单一界面基线。最终增量需要新的实施票据，不能把旧票据重新解释成已经完成本规格。
- 研究文档中的 Provider、服务端搜索、候选模型、Streamdown 和主题架构只提供事实依据；本规格是 1.1.0 产品边界的唯一权威来源。
- “内置支持模型”表示 SkillYard 依据 Provider 官方资料维护相应 Adapter 和静态模型 ID，不表示维护者已经替所有用户账号完成真实请求。当前账号、Key 和模型组合是否可用，由用户主动连接测试决定。
- 1.1.0 追求完整用户主流程，不穷举所有理论边界。Provider 不可用、余额不足或普通网络失败作为本次 AI 请求错误呈现，不扩张成第二套网络管理产品。
