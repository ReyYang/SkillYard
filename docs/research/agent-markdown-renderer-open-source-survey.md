# SkillYard Agent Markdown 开源实现调研

> 调研日期：2026-07-30
> 范围：为 SkillYard 1.1.0 的 Agent 流式回答选择可复用的 React Markdown 阅读组件，不改变现有对话状态、Provider 或 Session 模型。

## 结论

SkillYard 1.1.0 采用 Streamdown 作为 Agent Markdown 渲染基础。

Streamdown 专门处理 AI 流式回答中暂时未闭合的 Markdown，并已经提供 GFM、安全加固和可选代码高亮能力。SkillYard 只保留一层很薄的 `AgentMarkdown` 包装：

- 禁止 Raw HTML；
- 过滤链接协议，外部链接通过 Tauri 的受控方式打开；
- 默认不加载远程图片；
- 将组件的语义 CSS variables 映射到 SkillYard 三套主题；
- 保留 Agent 搜索结果、安装预览等结构化卡片，不把它们转换为 Markdown。

Streamdown 官方集成依赖 Tailwind utilities。SkillYard 只为该渲染组件增加所需构建支持，不把现有页面迁移到 Tailwind，也不因此引入 AI Elements、shadcn 或新的聊天 Runtime。最终颜色、圆角、间距、字体与代码块材质仍由 SkillYard 的主题 token 决定。

## 当前项目约束

- 前端使用 React 19、Vite 和 Tauri 2。
- 当前没有 Tailwind、shadcn 或第三方 Markdown 依赖。
- 已确认 1.1.0 的全局对话必须流式显示回答。
- `AgentOverlay` 已拥有自己的 Session、页面上下文、消息列表和结构化搜索结果。

因此，候选组件必须正确处理流式过程中暂时未闭合的 Markdown，但不能引入第二套聊天 Runtime 或状态模型。

## 候选比较

| 候选 | 可直接复用的能力 | 与 SkillYard 的额外成本 | 判断 |
| --- | --- | --- | --- |
| Streamdown | 流式未闭合 Markdown、GFM、安全加固、可选代码高亮与复制 | 增加受限的 Tailwind 构建支持、链接策略、图片策略和主题 token 映射 | **采用** |
| `@uiw/react-markdown-preview/common` | GFM、GitHub 风格基础排版、常用语言代码高亮、复制按钮、明暗模式、CSS variables | 不解决流式过程中暂时未闭合的 Markdown | 不采用 |
| `react-markdown` + `remark-gfm` | 安全默认值、CommonMark、GFM、组件替换 | 仍需自行完成整套排版、代码块、复制按钮和高亮 | 适合作为底层，不符合本次“不自己写阅读组件”的目标 |
| `@assistant-ui/react-markdown` | 面向 Agent 消息的 Markdown primitive 和样式示例 | 与 `@assistant-ui/react` Runtime、shadcn/Tailwind 体系耦合 | 不采用，避免与现有 Agent 状态模型重叠 |
| TanStack Markdown | 安全默认值、React 适配、可选流式扩展 | 仍处于 1.0 前，排版、高亮和复制仍需自行完成 | 不采用 |
| Markstream React | 流式 Markdown 与独立样式 | 维护成熟度和 React 生态采用度不如 Streamdown | 不采用 |

## 采用方案的边界

建议的数据与组件关系：

```text
Provider SSE
└── Rust Provider Adapter
    └── 统一 AgentStreamEvent
        └── Tauri Channel
            └── AgentOverlay
                ├── AgentMarkdown
                │   ├── Streamdown
                │   ├── Tauri 外部链接策略
                │   └── SkillYard 主题 token 映射
                └── AgentSearchResults
                    └── 完成后展示结构化结果与安装预览操作
```

Rust 只需要把三家 Provider 的流式协议归一为正文增量、完成和失败三类事件，不建立通用 Agent Harness。`AgentMarkdown` 只渲染不断增长的自然语言回答。联网搜索结果、来源、安装预览按钮和后续产品操作在完成事件中以结构化数据交付，继续使用有类型的 React 组件，不能让模型通过 Markdown 生成可执行操作。

## 安全要求

AI 返回内容属于不可信输入。即使 Streamdown 内建安全加固，仍应明确限制：

- Raw HTML 保持禁用；
- 链接只接受明确允许的协议，不能让链接直接控制 Tauri WebView 导航；
- 默认禁止远程图片、`data:` 图片、iframe、script 和内联事件；
- 代码块只展示与复制，不执行；
- 标题不生成可改变应用路由的锚点行为。

## 对三套主题的影响

三套主题共用同一个 Streamdown 语义结构，不复制三份 renderer。每套主题只提供对应 token：

- 正文、次级文字和链接颜色；
- 引用、分隔线和表格边框；
- inline code 与 code block 的背景、前景和语法色；
- 标题字号层级、段落间距、圆角与阴影。

这样 Markdown 在 Archive、Layers、Ledger 中会保持内容结构一致，同时呈现各自的视觉材质，不会出现 Agent 对话像嵌入了另一款产品的割裂感。

## 发现页结果结构

发现页采用三个明确并列的结果区，而不是把结果混在一个列表中：

1. **本机已有**：已安装、已接管或只读展示的 Skill。
2. **已添加来源**：Source 最近一次成功加载并保存在本地的成员目录，包括尚未安装的成员。
3. **全网发现**：Provider 联网搜索得到的公开结果。

三者是同一次搜索的并列结果，不是“本机命中后停止”的串行步骤：

- 本机与已添加来源的结果先显示；
- 用户提交搜索后，全网发现并行加载；
- 同一 canonical 来源或仓库的结果合并，不重复展示；
- 添加了 Source 但未安装任何 Skill 时，仍可从本地保存的成员目录命中“已添加来源”，但不会因此创建 Bundle 或 Mount；
- Agent 的 Markdown 回答可以解释搜索结果，实际结果与安装入口仍由结构化卡片展示。

## 第一方资料

- [Streamdown 官方仓库](https://github.com/vercel/streamdown)
- [Tauri Channel 官方说明](https://v2.tauri.app/develop/calling-rust/#channels)
- [`@uiw/react-markdown-preview` 官方仓库](https://github.com/uiwjs/react-markdown-preview)
- [`react-markdown` 官方仓库与安全说明](https://github.com/remarkjs/react-markdown)
- [assistant-ui Markdown 官方文档](https://www.assistant-ui.com/docs/ui/markdown)
- [TanStack Markdown 官方文档](https://tanstack.com/markdown/latest/docs/)
- [Markstream React 官方文档](https://markstream.simonhe.me/guide/react-quick-start)
