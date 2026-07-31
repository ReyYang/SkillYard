# SkillYard 1.1.0 Provider 原生全网搜索核验

> 修订日期：2026-07-29
>
> 范围：OpenAI、智谱 GLM 和 DeepSeek 的服务端搜索能力，以及 `skills.sh` / `npx skills find` 与当前产品方向的关系。本文只提供事实依据；搜索行为以 1.1.0 产品规格为准。

## 修订说明

本文原先提出由 SkillYard 本地 Harness 提供统一的 `search_public_skills`，并在本地无结果时直接查询 `skills.sh/api/search`。这个产品结论已经被后续讨论取代：

- 用户需要的是全网搜索，不是只搜索一个 Skill 目录；
- SkillYard 不建设复杂的本地 Agent Harness；
- 本地搜索仍由 SkillYard 基于实际 Skill 内容完成；
- 需要联网时，由用户当前选择的 Provider 执行服务端全网搜索；
- `skills.sh` 与 `npx skills find` 的事实调查保留为背景，但不成为 1.1.0 Agent 的搜索路径。

## 结论

SkillYard 1.1.0 的搜索路径是：

1. 先读取本机已知 Skill 的实际内容；
2. 找到适合用户需求的本地 Skill 时直接回答，不访问互联网；
3. 本地没有合适结果时，调用当前 Provider 的服务端全网搜索；
4. 用户明确要求在线、新选择或最新结果时，可以直接进入全网搜索；
5. 带可核验 URL 的结果用于说明和候选展示；
6. 能被现有 Source Adapter 解析的候选进入确定性安装预览，其他网页只作参考；
7. Agent 不直接安装，也不执行 CLI、Shell 或网页脚本。

SkillYard 不为搜索结果建立评分、热度、新鲜度或二次排序。展示顺序只是当前 Agent 回答的一部分，不成为持久产品数据。

## 必须区分的三个概念

### 模型能力

模型根据输入生成输出。普通聊天请求本身不会让模型自动访问互联网。

### Function Calling

模型可以提出工具名称和参数，但默认仍由调用方执行工具。只支持 Function Calling 不能证明 Provider 会替 SkillYard 搜索互联网。

### Provider 托管搜索

Provider 在自己的服务器执行搜索，并返回结果、引用或带来源的回答。这通常使用专有 Tool 类型、Endpoint、响应事件和计费规则。

因此：

- 消费端产品可以联网，不等于普通 API 请求自动联网；
- OpenAI-compatible Chat Completions 不定义跨厂商统一的托管搜索；
- SkillYard 必须为三家已选择 Provider 分别适配搜索协议；
- 这种适配仍然可以保持很薄，因为 SkillYard 不执行本地 Tool Loop 或网页抓取。

## 三个 Provider 的搜索协议

| Provider | 服务端搜索入口 | 主要返回证据 | 当前证据边界 |
| --- | --- | --- | --- |
| OpenAI | Responses API `web_search` | `web_search_call` 与 URL citations | 官方逐模型页面可以明确核验支持 |
| 智谱 GLM | Chat Completions 专有 `type: "web_search"` | 顶层搜索结果、URL、标题、摘要和引用标识 | 官方缺少现代模型逐项兼容矩阵，用户需要验证当前选择 |
| DeepSeek | 官方 Anthropic-compatible Endpoint 的 server-tool 路径 | `server_tool_use`、`web_search_tool_result` | 官方确认 Claude Code 集成可搜索，用户需要验证当前选择的独立请求 |

### OpenAI

新接入使用：

```text
POST /v1/responses
tools: [{ "type": "web_search" }]
```

模型可以根据当前问题决定是否搜索，响应包含 Web Search Call 和 URL citations。这个字段属于 OpenAI Responses API，不能原样发送给任意 OpenAI-compatible Provider。

官方依据：[OpenAI Web Search](https://developers.openai.com/api/docs/guides/tools-web-search)、[OpenAI 模型目录](https://developers.openai.com/api/docs/models)。

### 智谱 GLM

智谱 Chat Completions 在 `tools` 中支持专有 `type: "web_search"`，响应可以包含标题、链接、站点、发布日期、摘要和引用信息。它是真正的 Provider 托管搜索，不是让 SkillYard 执行一个普通函数。

当前文档的搜索示例仍使用较早模型，没有给出 1.1.0 五个候选与 Web Search in Chat 的逐模型矩阵。因此静态目录只能表达 SkillYard 对这些正式模型 ID 的支持；用户选择其中一个模型后，必须用自己的 Key 验证搜索和真实 URL。

官方依据：[智谱联网搜索](https://docs.bigmodel.cn/cn/guide/tools/web-search)、[对话补全 API](https://docs.bigmodel.cn/api-reference/%E6%A8%A1%E5%9E%8B-api/%E5%AF%B9%E8%AF%9D%E8%A1%A5%E5%85%A8)。

### DeepSeek

DeepSeek 普通 OpenAI-compatible Chat Completions 的 Tool 文档主要描述由调用方执行的 Function Calling。1.1.0 使用另一条官方路径：

```text
Base URL: https://api.deepseek.com/anthropic
API shape: Anthropic Messages
```

官方兼容表包含 `server_tool_use` 和 `web_search_tool_result`，Claude Code 集成文档明确搜索由 DeepSeek API 执行。公开资料没有给出一个完整的独立应用服务端搜索请求，因此 Adapter 使用已知协议编写离线合同测试，并由用户用自己的 Key 验证当前所选 DeepSeek 模型，不能只凭字段名称宣称真实可用。

官方依据：[DeepSeek Anthropic API](https://api-docs.deepseek.com/guides/anthropic_api/)、[DeepSeek Claude Code 集成](https://api-docs.deepseek.com/quick_start/agent_integrations/claude_code/)。

## 本地优先为什么仍由 SkillYard 判断

Provider 只能看到 SkillYard 明确发送的内容，不天然知道用户本机安装了什么。SkillYard 必须先从 Inventory 中选择当前可读的 Skill，并让所选模型根据真实内容判断是否满足需求。

本地优先不是一个持久搜索索引或排名系统：

- 不建立 Embedding 或向量数据库；
- 不维护相关度分数；
- 不按 Star、下载量或更新时间排序；
- 不把一次对话结果保存为搜索记录；
- 不依赖 Skill Name 精确匹配；
- Agent 可以读取实际文件，判断名称之外的能力。

如果本地已有合适结果，本次请求不提供 Provider Web Search。用户明确要求在线结果时，才允许绕过这个默认规则。

## 全网结果如何进入安装流程

Provider 搜索返回的是信息，不是安装授权。

- GitHub 仓库、可解析归档或其他已受支持 Source，可以转换为现有 Source／安装预览输入；
- 普通网页、论坛、社交媒体或无法解析的链接只显示为参考；
- Agent 不能从网页推断一个未验证的下载命令并执行；
- Agent 不能直接创建 Bundle、写 Central Store 或挂载到 Supported App；
- 用户仍要在现有页面查看候选 Skill、内容验证结果和影响预览，并主动确认。

这让 Agent 只负责“理解和发现”，Lifecycle Core 继续负责“真正改变本机状态”。

## `skills.sh` 与 `npx skills find` 的保留事实

`vercel-labs/skills` 的源码把搜索和 CLI 遥测写成两个独立 HTTP 请求：

- 搜索请求访问 `https://skills.sh/api/search`；
- CLI 的 `track()` 另外访问 `https://add-skill.vercel.sh/t`；
- 设置 `DISABLE_TELEMETRY` 或 `DO_NOT_TRACK` 可以阻止 CLI 的额外遥测请求。

所以，直接请求 `skills.sh/api/search` 不会执行 `npx skills find` 的 `track()` 代码，但查询词、来源 IP、Headers 和请求时间仍会发送给 `skills.sh`。不能把它描述为“零日志”或“零数据”。

代码依据：[find.ts](https://github.com/vercel-labs/skills/blob/main/src/find.ts)、[telemetry.ts](https://github.com/vercel-labs/skills/blob/main/src/telemetry.ts)、[CLI README](https://github.com/vercel-labs/skills/blob/main/README.md)。

1.1.0 不采用这条路径，原因不是 CLI 有问题，而是：

1. 用户已经明确要求全网搜索；
2. `skills.sh` 只覆盖自己的目录数据；
3. `npx` 还要求本机 Node.js/npm，并可能下载和执行包；
4. Provider 原生搜索已经是三个固定 Adapter 的共同能力契约；
5. 再加入 `skills.sh` 会产生第二条线上发现路径和额外降级规则。

如果未来产品明确需要“只浏览 skills.sh 目录”，应作为独立目录入口讨论，而不是混进全局 Agent。

## 隐私边界

- 本地找到合适 Skill 时不发起全网搜索。
- 全网搜索会把用户查询和完成回答所需的已过滤上下文发送给当前 Provider。
- 发送前移除本机敏感信息；不能把完整 Inventory、个人路径或凭据作为搜索上下文。
- Provider 自身可能记录、计费或保留请求，具体规则由用户所选 Provider 决定。
- SkillYard 不添加分析遥测，也不把搜索请求复制到自己的服务器。

## 错误与可用性

SkillYard 不负责诊断用户网络、Provider 账号或地区资格，也不在三家 Provider 之间自动切换。搜索失败时：

- 当前 Agent 请求显示 Provider 错误；
- 不伪造无来源答案；
- 不改变本地 Inventory、Source 或 Bundle；
- 非 AI 功能继续可用；
- 用户可以稍后重试或在设置中更换全局 Provider／模型。

## 离线合同与用户连接测试

普通离线 CI 使用 Fake Server 验证：

1. 本地有合适 Skill 时没有服务端搜索请求；
2. 本地无结果时构造对应 Provider 的服务端搜索请求；
3. 用户明确要求在线时可以直接进入搜索路径；
4. 三个 Adapter 能归一真实 URL 和引用关系；
5. 搜索失败、无权限、限流和超时不会生成伪来源；
6. 可解析结果只进入既有安装预览；
7. OpenAI citations、GLM 顶层搜索结果和 DeepSeek server-tool 内容能被正确展示。

真实接口由用户只验证当前选择的模型：一次固定 Schema 请求和一次必须返回真实 URL 的搜索请求。两项都通过后才能启用 AI；SkillYard 不使用用户的 Key 批量测试同一 Provider 的其他候选模型。

## 最终回答

“模型供应商自己的 Agent 能联网吗？”更准确的回答是：

> 多家消费产品和 API 都有联网能力，但底层模型不会仅凭普通聊天请求自动访问互联网。SkillYard 1.1.0 使用三个固定 Provider 各自的服务端搜索协议，而不是自建搜索 Harness。

“为什么不直接用 `find-skills` 或 `skills.sh`？”更准确的回答是：

> 它们适合查找特定目录中的 Skill，但用户已经明确需要全网搜索。1.1.0 先读取本机 Skill，必要时再让当前 Provider 搜索整个互联网；安装仍回到 SkillYard 的确定性预览和确认流程。
