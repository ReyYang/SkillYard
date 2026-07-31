# SkillYard Agent 首批静态支持模型目录

> 调研日期：2026-07-29
>
> 范围：SkillYard 已确认支持的 OpenAI、智谱 GLM、DeepSeek。只核验首批静态模型选择器，不增加 Provider，也不改变“一次只选择一个模型，所有 AI 能力共用该模型”的产品决定。

## 结论

首批目录按已确认的产品要求提供 OpenAI 5 个、智谱 GLM 5 个、DeepSeek 2 个候选模型：

| Provider | 建议模型 ID | 用户定位 | 建议默认 | 文档是否同时覆盖普通任务与服务端搜索 | 用户选择后的验证重点 |
| --- | --- | --- | --- | --- | --- |
| OpenAI | `gpt-5.6-sol` | 最高质量、复杂来源判断 | 否 | 是，模型页明确支持 Structured Outputs、Function Calling 和 Responses `web_search` | 当前账号权限、固定 Schema 和 URL 引用 |
| OpenAI | `gpt-5.6-terra` | 平衡质量与成本 | 是 | 是，模型页明确支持 Structured Outputs、Function Calling 和 Responses `web_search` | 当前账号权限、固定 Schema 和 URL 引用 |
| OpenAI | `gpt-5.6-luna` | 经济型、高频任务 | 否 | 是，模型页明确支持相同能力 | 当前账号权限、固定 Schema 和 URL 引用 |
| OpenAI | `gpt-5.4-mini` | 更低成本、较快响应 | 否 | 是，模型页明确支持 Structured Outputs、Function Calling 和 Responses `web_search` | 当前账号权限、固定 Schema 和 URL 引用 |
| OpenAI | `gpt-5.5` | 成熟的高质量专业任务模型 | 否 | 是，模型页明确支持 Structured Outputs、Function Calling 和 Responses `web_search` | 当前账号权限、固定 Schema 和 URL 引用 |
| 智谱 GLM | `glm-5.2` | 最高质量、超长内容 | 否 | 普通对话和结构化输出有官方依据；`web_search` 的逐模型兼容性没有独立矩阵 | 当前模型能否返回固定 Schema 和真实搜索 URL |
| 智谱 GLM | `glm-5.1` | 高质量复杂任务 | 否 | 普通对话和结构化输出有官方依据；`web_search` 的逐模型兼容性没有独立矩阵 | 当前模型能否返回固定 Schema 和真实搜索 URL |
| 智谱 GLM | `glm-4.7` | 高质量通用任务 | 是 | 普通对话、结构化输出和智能搜索有官方依据；`web_search` 的逐模型兼容性没有独立矩阵 | 当前模型能否返回固定 Schema 和真实搜索 URL |
| 智谱 GLM | `glm-4.7-flashx` | 轻量高速、低延迟 | 否 | GLM-4.7 系列普通任务与结构化输出有官方依据；`web_search` 的逐模型兼容性没有独立矩阵 | 当前模型能否返回固定 Schema 和真实搜索 URL |
| 智谱 GLM | `glm-4.7-flash` | 免费、轻量、高频任务 | 否 | 普通对话、翻译、长文本、Function Call 有官方依据；`web_search` 的逐模型兼容性没有独立矩阵 | 当前模型能否返回固定 Schema 和真实搜索 URL |
| DeepSeek | `deepseek-v4-flash` | 经济型、高并发任务 | 是 | 普通任务与 Anthropic Endpoint 均有官方依据；官方 Claude Code 集成把 Sonnet/Haiku 映射到它并支持服务端 Web Search | 独立 Messages 请求中的固定 Schema、搜索结果和真实 URL |
| DeepSeek | `deepseek-v4-pro` | 高质量任务 | 否 | 普通任务与 Anthropic Endpoint 均有官方依据；官方 Claude Code 集成把 Opus 映射到它并支持服务端 Web Search | 独立 Messages 请求中的固定 Schema、搜索结果和真实 URL |

这里的“建议模型”是依据 Provider 官方资料维护的**内置支持目录**，不是“维护者已经替所有用户账号完成真实请求”的认证名单。API Key 通常属于 Provider 账号而不是单个模型，但账号层级、地区、余额和权限仍可能限制具体模型。用户只使用自己的 Key 验证当前选择。

OpenAI 五个候选都有逐模型的原生搜索能力声明；GLM 五个候选虽然都是当前正式文本模型，但官方没有公布现代模型与 Web Search in Chat 的逐模型兼容矩阵；DeepSeek 的独立应用搜索请求也仍有文档空白。这些差异由 Adapter 的离线合同测试和用户对当前选择的真实连接测试共同处理，不要求维护者持有全部候选模型权限。

五个选项分别覆盖最高质量、平衡、经济、成熟高质量、低延迟或免费等用户能理解的差异。相邻型号没有清晰用途差异时，不继续加入。

## 目录准入条件

一个模型必须同时满足以下条件，才能显示在 SkillYard 的静态选择器中：

1. Provider 当前官方模型目录仍列出该模型；
2. 支持普通文本对话，能完成中文说明、摘要、分类与多轮问答；
3. 支持 SkillYard 所需的结构化输出或至少有效 JSON 输出；
4. 能通过该 Provider 已确认的服务端全网搜索协议完成搜索；
5. 搜索结果能提供 SkillYard 可展示的真实 URL 或引用；
6. 使用同一个模型 ID 完成普通任务与搜索任务，不在后台偷偷换模型；
7. 对应 Provider Adapter 的请求与响应格式已进入 SkillYard 离线合同测试；
8. 未处于已公告下线、实验或仅限特定合作伙伴的状态。

`GET /models` 只能证明账号看到了一个 ID，不能证明它满足以上产品契约。SkillYard 也不应把 `/models` 返回的所有内容动态展示给用户。

## OpenAI

### 建议纳入

#### `gpt-5.6-sol`

这是 OpenAI 首批的最高质量候选。

官方把它定位为 GPT-5.6 系列的 frontier model，适合复杂专业任务。模型页明确支持 Responses API、Function Calling、Structured Outputs 和 Web Search，价格为每百万 Token 输入 US$5、缓存输入 US$0.50、输出 US$30。

它适合来源证据冲突、多网页综合和复杂 Skill 说明，但不是默认模型：价格正好是 Terra 的两倍，而且 SkillYard 尚未证明日常任务需要持续支付这项差价。

官方依据：[GPT-5.6 Sol](https://developers.openai.com/api/docs/models/gpt-5.6-sol)、[OpenAI Model Guidance](https://developers.openai.com/api/docs/guides/latest-model)。

#### `gpt-5.6-terra`

这是 OpenAI 首批的默认候选。

官方把它定位为在 intelligence 与 cost 之间取得平衡的 GPT-5.6 模型。模型页明确列出：

- `v1/responses`；
- Function Calling；
- Structured Outputs；
- Responses API 的 Web Search；
- 1.05M context；
- 每百万 Token 输入 US$2.50、缓存输入 US$0.25、输出 US$15。

这些能力同时覆盖 SkillYard 的两类工作：

- 本地 Skill 的中文说明、摘要、分类与结构化结果；
- 由同一模型在 Responses API 中调用 OpenAI 托管的 `web_search`。

官方依据：[GPT-5.6 Terra](https://developers.openai.com/api/docs/models/gpt-5.6-terra)、[OpenAI Web Search](https://developers.openai.com/api/docs/guides/tools-web-search)。

#### `gpt-5.6-luna`

这是 OpenAI 的经济候选。

官方把它定位为 cost-sensitive、high-volume workload，近似早期 GPT-5 系列的 nano 档。模型页同样明确支持 Function Calling、Structured Outputs 和 Responses Web Search，价格为每百万 Token 输入 US$1、缓存输入 US$0.10、输出 US$6。

它在协议上满足目录条件，但“适合高频任务”不能保证所有 Skill 来源比较和长说明任务的实际质量。SkillYard 不建立自己的模型排名，用户可以根据效果和费用切换后重新验证。

官方依据：[GPT-5.6 Luna](https://developers.openai.com/api/docs/models/gpt-5.6-luna)。

#### `gpt-5.4-mini`

这是 OpenAI 的低成本、较快响应候选。

官方把它定位为面向 high-volume workload 的高效小模型。它支持普通文本生成、Function Calling、Structured Outputs 和 Responses Web Search，价格为每百万 Token 输入 US$0.75、缓存输入 US$0.075、输出 US$4.50。

它比 Luna 更便宜，但属于上一代模型。纳入它的用户价值不是“多一个旧模型”，而是给重视调用成本和响应速度、同时仍需要原生搜索的用户一个更低价位；具体输出质量由用户结合自己的任务判断。

官方依据：[GPT-5.4 mini](https://developers.openai.com/api/docs/models/gpt-5.4-mini)。

#### `gpt-5.5`

这是 OpenAI 的成熟高质量候选。

官方把它定位为面向复杂专业工作的 frontier model。它支持 Responses API、Function Calling、Structured Outputs 和 Responses Web Search，价格为每百万 Token 输入 US$5、缓存输入 US$0.50、输出 US$30。

它与 GPT-5.6 Sol 的价格相同，但官方提供了 `gpt-5.5-2026-04-23` dated snapshot，适合偏好固定行为的用户。SkillYard 说明这一差异，但不通过内部质量评分替用户决定是否选择。

官方依据：[GPT-5.5](https://developers.openai.com/api/docs/models/gpt-5.5)。

### 搜索协议限制

OpenAI Adapter 必须使用：

```text
POST /v1/responses
model: 上述五个白名单模型之一
tools: [{ "type": "web_search" }]
```

`web_search` 是当前非 preview 的推荐工具。模型可以自行判断是否搜索，响应会包含 Web Search Call 和 URL citations。

搜索有独立费用：当前官方价格为每 1,000 次 US$10，搜索内容 Token 另按所选模型输入价格计费。

官方依据：[OpenAI Web Search](https://developers.openai.com/api/docs/guides/tools-web-search)、[OpenAI API Pricing](https://developers.openai.com/api/docs/pricing)。

### 首批明确不纳入

| 模型 | 不纳入原因 |
| --- | --- |
| `gpt-5.6` | 只是路由到 `gpt-5.6-sol` 的别名；同时显示会让用户误以为是两个模型 |
| `gpt-5.5-pro`、`gpt-5.4`、`gpt-5.4-pro` | 当前仍正式可用，但分别与 Sol 或 Terra 形成能力、价格和用户定位高度重叠的中间项；Pro 还存在极高价格、较长延迟或 Structured Outputs 限制 |
| GPT-5.2、GPT-5.1、GPT-5、GPT-4.1 等更早通用模型 | 继续增加旧代不能形成首批选择器中新的用户定位，且会扩大回归测试矩阵 |
| `gpt-5-search-api` | 是保留 Chat Completions 搜索接入的专用模型路径，不适合作为所有普通对话、摘要与分类共同使用的唯一模型 |
| `gpt-4o-search-preview`、`gpt-4o-mini-search-preview` | 已于 2026-07-23 下线，且属于旧 preview 路径 |
| Realtime、audio、image、embedding、moderation、Codex 专用模型 | 不能同时履行普通文本 Agent 与服务端全网搜索的共同模型契约 |

官方依据：[OpenAI Deprecations](https://developers.openai.com/api/docs/deprecations)、[OpenAI Model Catalog](https://developers.openai.com/api/docs/models)。

### 文档仍未证明的内容

- GPT-5.6 三个模型当前页面没有提供不同的 dated snapshot；静态 ID 是否长期保持完全相同行为，不能由文档保证。
- Structured Outputs 与 `web_search` 在当前账号下的实际行为，需要用户选择模型后验证。
- `luna` 和 `gpt-5.4-mini` 在中文 Skill 分类、来源候选辨别和多网页证据综合上的实际质量，不能由文档保证。
- `gpt-5.5` 相对 Sol 与 Terra 是否仍有足够清晰的质量或稳定性价值，由用户结合自己的任务和费用取舍，不作为发布前横向评测门槛。
- 用户账号的 tier、地区与组织设置是否允许具体模型和 Web Search，只能用该用户自己的 Key 验证。

## 智谱 GLM

### 建议纳入

#### `glm-5.2`

这是 GLM 的最高质量、超长内容候选。

官方把它定位为面向长任务的旗舰基座模型，支持 1M context、普通文本对话、Function Call 和 JSON 等结构化输出。当前价格为每百万 Token 输入 ¥8、输出 ¥28。

它适合一次分析大量 Skill 内容或复杂来源证据，但这些场景不是 SkillYard 的日常默认路径，因此不作为默认模型。当前账号下是否支持 Web Search in Chat，由用户选择后验证。

官方依据：[GLM-5.2](https://docs.bigmodel.cn/cn/guide/models/text/glm-5.2)、[智谱价格页](https://bigmodel.cn/pricing)。

#### `glm-5.1`

这是 GLM 的高质量复杂任务候选。

官方将它定位为旗舰基座模型，并明确列出通用对话、复杂指令、多轮交流、Function Call 和 JSON 等结构化输出。它使用 200K context；短输入档当前价格为每百万 Token 输入 ¥6、输出 ¥24。

它位于 GLM-5.2 与 GLM-4.7 之间：适合想要更高质量、但不需要 1M context 的用户。SkillYard 只呈现官方定位，不对不同型号建立自己的质量排名。

官方依据：[GLM-5.1](https://docs.bigmodel.cn/cn/guide/models/text/glm-5.1)、[智谱价格页](https://bigmodel.cn/pricing)。

#### `glm-4.7`

这是 GLM 首批的默认候选。

官方模型页明确说明它具备：

- 高质量多轮对话与复杂问题协作；
- Function Call；
- JSON 等结构化输出；
- 200K context；
- 智能搜索与 Deep Research 场景能力。

官方价格页当前给出的常用短输入、短输出档价格为每百万 Token 输入 ¥2、输出 ¥8；输入或输出变长后进入更高阶梯。

相比 GLM-5.2，`glm-4.7` 更符合 SkillYard 的日常说明、翻译、分类和来源查找任务。智谱自己的 Coding Plan FAQ 也建议普通任务使用 GLM-4.7，复杂任务再使用 GLM-5.2。

官方依据：[GLM-4.7](https://docs.bigmodel.cn/cn/guide/models/text/glm-4.7)、[智谱价格页](https://bigmodel.cn/pricing)、[智谱 Coding Plan FAQ](https://docs.bigmodel.cn/cn/coding-plan/faq)。

#### `glm-4.7-flashx`

这是 GLM 的轻量高速候选。

智谱把 GLM-4.7-FlashX 列为 GLM-4.7 系列成员，定位为“小尺寸强能力”，用于中文写作、翻译、角色扮演等通用场景。GLM-4.7 系列页面列出 Function Call、结构化输出和 200K context；当前价格为每百万 Token 输入 ¥0.50、输出 ¥3。

它与免费 Flash 的实际延迟、限流和质量差异取决于用户账号与任务。SkillYard 只呈现官方定位和价格档位，不建立自己的模型排名。

官方依据：[GLM-4.7 系列](https://docs.bigmodel.cn/cn/guide/models/text/glm-4.7)、[智谱模型概览](https://docs.bigmodel.cn/cn/guide/start/model-overview)、[智谱价格页](https://bigmodel.cn/pricing)。

#### `glm-4.7-flash`

这是 GLM 的经济候选，目前官方价格页标为免费。

官方模型页明确把它用于中文写作、翻译、长文本和其他通用场景，并列出 Function Call、结构化输出、200K context。它比早期 `glm-4.5-flash` 更适合作为当前免费入口，因为后者已经公告下线并自动路由到 `glm-4.7-flash`。

免费不等于当前账号一定可用。用户仍需用自己的 Key 验证固定 Schema 和服务端搜索，验证失败时不能启用 AI。

官方依据：[GLM-4.7-Flash](https://docs.bigmodel.cn/cn/guide/models/free/glm-4.7-flash)、[智谱价格页](https://bigmodel.cn/pricing)、[智谱新品发布](https://docs.bigmodel.cn/cn/update/new-releases)。

### 搜索协议限制

GLM Adapter 使用官方 Chat Completions 扩展：

```text
POST https://open.bigmodel.cn/api/paas/v4/chat/completions
model: 上述五个白名单模型之一
tools: [{ "type": "web_search", "web_search": { ... } }]
```

响应顶层可包含 `web_search` 数组，其中有标题、链接、站点、发布日期、摘要和引用标识。

这里存在一项必须正面记录的文档空白：

- 当前 Chat Completions API Reference 把上述五个候选都列为有效文本模型；
- 同一接口的 `tools` 联合类型包含 `Web Search`；
- 但官方 Web Search 示例仍使用较早的 `glm-4-air`，没有给出一张“每个模型是否支持 Web Search in Chat”的逐模型矩阵。

所以静态目录不能被描述为“这五个模型已经替所有账号通过生产验证”。用户选择其中一个模型后，SkillYard 使用该用户自己的 Key 检查固定 Schema 和搜索结果；不会批量请求其他四个模型。

官方依据：[智谱对话补全 API](https://docs.bigmodel.cn/api-reference/%E6%A8%A1%E5%9E%8B-api/%E5%AF%B9%E8%AF%9D%E8%A1%A5%E5%85%A8)、[智谱联网搜索](https://docs.bigmodel.cn/cn/guide/tools/web-search)。

### 首批明确不纳入

| 模型 | 不纳入原因 |
| --- | --- |
| `glm-5` | 当前仍正式可用，但其高质量 Agent 定位夹在 GLM-5.1 与 GLM-4.7 之间；五个候选已经覆盖质量、平衡、速度和免费档 |
| `glm-5-turbo` | 官方定位为 OpenClaw 场景专项优化，不是 SkillYard 的通用默认模型 |
| `glm-4.6`、`glm-4.5-air*` 和更早正式模型 | 继续增加旧代和中间速度档，不形成新的首批用户定位 |
| `glm-4.5`、`glm-4.5-flash` | 官方已标为即将或已经下线，其中 `glm-4.5-flash` 请求会自动路由到 `glm-4.7-flash` |
| `glm-4-air` | 官方 Web Search 示例使用它，但它属于旧代；不能为了复用旧示例而把旧模型带入新静态目录 |
| 视觉、图像、音视频、Embedding、Rerank 模型 | 不满足同一模型同时承担普通文本 Agent 与服务端搜索的契约 |

### 文档仍未证明的内容

- 五个候选分别发送 `type: "web_search"` 时是否被账号与模型组合接受；
- 是否每次都在顶层 `web_search` 返回可展示 URL；
- `response_format: {"type":"json_object"}` 与 `web_search` 同时使用时，最终文本是否保持有效 JSON；
- FlashX 的真实延迟优势，以及免费 Flash 的实际并发、限流和高峰期可用性；
- 智谱没有在已核验页面中给出统一、长期的模型下线通知期。

## DeepSeek

### 建议纳入

#### `deepseek-v4-flash`

这是 DeepSeek 首批默认候选。

官方 Models & Pricing 当前只列出 `deepseek-v4-flash` 和 `deepseek-v4-pro` 两个主模型。两者都支持：

- thinking 与 non-thinking；
- JSON Output；
- Tool Calls；
- 1M context；
- OpenAI Chat Completions 和 Anthropic-compatible Endpoint。

`deepseek-v4-flash` 当前每百万 Token 价格为缓存命中 US$0.0028、缓存未命中 US$0.14、输出 US$0.28。官方 Claude Code 映射把 Claude Sonnet 与 Haiku 名称映射到它，说明它承担默认和轻量 Agent 路径。

官方依据：[DeepSeek Models & Pricing](https://api-docs.deepseek.com/quick_start/pricing/)、[DeepSeek Anthropic API](https://api-docs.deepseek.com/guides/anthropic_api/)、[DeepSeek Claude Code 集成](https://api-docs.deepseek.com/quick_start/agent_integrations/claude_code/)。

#### `deepseek-v4-pro`

这是 DeepSeek 的质量候选。

它具备与 Flash 相同的基础协议能力，当前价格为每百万 Token 缓存命中 US$0.003625、缓存未命中 US$0.435、输出 US$0.87。官方 Anthropic API 示例直接使用 `deepseek-v4-pro`，Claude Code 映射把 Opus 名称映射到它。

它是否在 SkillYard 的 Skill 说明和来源判断上明显优于 Flash 取决于用户任务。SkillYard 只说明官方定位和价格差异，不建立自己的质量排名。

官方依据：[DeepSeek Models & Pricing](https://api-docs.deepseek.com/quick_start/pricing/)、[DeepSeek Anthropic API](https://api-docs.deepseek.com/guides/anthropic_api/)。

### 搜索协议限制

DeepSeek 必须使用已经确认的特殊 Adapter：

```text
Base URL: https://api.deepseek.com/anthropic
API shape: Anthropic Messages
model: deepseek-v4-flash 或 deepseek-v4-pro
```

DeepSeek 官方兼容表明确支持响应内容块：

- `server_tool_use`；
- `web_search_tool_result`。

官方 Claude Code 集成进一步明确：当模型判断需要搜索时，会调用 Web Search，并通过 DeepSeek API 执行搜索。

但 DeepSeek 官方文档没有给出独立应用声明 Web Search Tool 的完整请求示例。Anthropic 当前的基础 GA 搜索 Tool 是：

```text
{ "type": "web_search_20250305", "name": "web_search" }
```

后续 `web_search_20260209` 及更新版本默认依赖 code execution 的动态过滤，而 DeepSeek 兼容表明确不支持 `code_execution_tool_result`。因此 1.1.0 Adapter 和用户连接测试从基础 `web_search_20250305` 开始，不能直接假定最新 Anthropic 搜索 Tool 版本兼容。

这项推导只决定测试顺序，不等于已经证明 DeepSeek 接受该请求字段。

官方依据：[DeepSeek Anthropic API](https://api-docs.deepseek.com/guides/anthropic_api/)、[DeepSeek Claude Code 集成](https://api-docs.deepseek.com/quick_start/agent_integrations/claude_code/)、[Anthropic Web Search Tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool)、[Anthropic Tool Reference](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-reference)。

### 首批明确不纳入

| 模型或名称 | 不纳入原因 |
| --- | --- |
| `deepseek-chat`、`deepseek-reasoner` | 官方已公告于 2026-07-24 停用；过渡期只分别指向 V4 Flash 的 non-thinking 与 thinking |
| DeepSeek V3.x、R1 与实验 Endpoint | 已被 V4 主模型替代，部分旧 Endpoint 没有 Tool Calls 或已经到期 |
| 任意 `claude-*` 名称 | DeepSeek 会把 Opus 映射到 Pro、Sonnet/Haiku 映射到 Flash；把这些别名放进 SkillYard 会让 UI 显示一个并未真正使用的模型 |
| 不受支持的任意模型名 | DeepSeek Anthropic API 会静默映射到 `deepseek-v4-flash`；SkillYard 必须只发送白名单中的真实 DeepSeek ID，不能把“请求成功”误判为用户所选模型存在 |
| `deepseek-v4-pro[1m]` | 官方 Claude Code 配置使用该形式，但 Models & Pricing 和直接 Anthropic API 示例使用 canonical `deepseek-v4-pro`；首批静态目录先使用 canonical ID，括号后缀必须另行验证 |

官方依据：[DeepSeek Change Log](https://api-docs.deepseek.com/updates/)、[DeepSeek Anthropic API](https://api-docs.deepseek.com/guides/anthropic_api/)。

### 文档仍未证明的内容

- 自定义应用向 DeepSeek Anthropic Endpoint 发送 `web_search_20250305` 是否被接受；
- Flash 与 Pro 是否都返回完整 `server_tool_use`、`web_search_tool_result` 和可展示 URL；
- DeepSeek 的兼容表把普通 `citations` 标为 ignored，服务端搜索结果中哪些引用字段会保留，需要实际读取响应；
- 搜索发生时的附加 Token 请求数量、延迟和错误语义；
- thinking 开关、JSON Output 与服务端搜索组合是否稳定；
- DeepSeek 没有在已核验页面中给出 V4 模型的长期下线通知期。

## 用户侧能力验证

静态目录只说明 SkillYard 为这些模型 ID 提供内建支持。真实可用性由用户在设置中选择一个模型、提供自己的 Key，并主动点击“测试连接”确认。

一次连接测试只验证当前选择：

1. **固定 Schema**：生成最小分类与摘要结果，并通过本地 Schema 校验；
2. **服务端搜索**：执行一个明确需要联网的问题，并返回至少一个可打开的真实 URL；
3. **模型身份**：Provider 返回模型身份时，确认没有发生可识别的静默 fallback。

三项要求不能拆成部分功能状态。任一要求失败，当前配置保持未启用；用户可以修改 Key、切换模型后再次测试。SkillYard 不遍历同一 Provider 的其他模型，不保存完整测试 Prompt 或 Response，也不上传验证结果。

Provider Adapter 的离线合同测试分别覆盖：

- OpenAI Responses 的 `web_search_call`、固定 Schema 和 URL citations；
- GLM Chat Completions 的 `type: "web_search"`、顶层搜索结果和引用关系；
- DeepSeek Anthropic Endpoint 的 `web_search_20250305`、`server_tool_use`、`web_search_tool_result` 和 `pause_turn`；
- 三家 Provider 的无权限、无余额、限流、超时、无搜索结果和错误响应。

离线合同测试证明 SkillYard 正确处理已知协议，不冒充真实账号测试。用户连接测试证明当前账号、Key 和所选模型在当时可用，也不代表 Provider 以后不会改变权限、价格或行为。

## 静态目录的维护规则

1. 模型 ID 由 SkillYard 随应用版本发布，不从 Provider `/models` 动态扩展 UI。
2. 目录依据官方资料维护，不在运行时建立候选、已验证、实验性或部分可用等用户可见模型状态。
3. 新模型具备官方普通任务、结构化输出和服务端搜索依据，并完成 Adapter 离线合同测试后，才能随应用版本加入目录。
4. Provider 公告下线时，应用提示用户更换模型，不能静默改变用户选择。
5. 不把官方明确会静默映射的别名放入目录；连接测试或实际请求发现可识别的 fallback 时，当前配置不能启用或继续使用。
6. 价格只用于帮助用户理解档位，不作为硬编码产品承诺；每次发布前重新读取官方价格页。

这不是动态 Provider 能力发现，也不是让 SkillYard 维护完整模型市场。它只是一个极小、依据官方资料维护并随应用发布的支持列表。

## 最终建议

首批 UI 显示：

- OpenAI：`GPT-5.6 Sol`、`GPT-5.6 Terra`、`GPT-5.6 Luna`、`GPT-5.4 mini`、`GPT-5.5`；
- 智谱 GLM：`GLM-5.2`、`GLM-5.1`、`GLM-4.7`、`GLM-4.7 FlashX`、`GLM-4.7 Flash`；
- DeepSeek：`DeepSeek V4 Flash`、`DeepSeek V4 Pro`。

默认分别为：

- OpenAI：`gpt-5.6-terra`；
- 智谱 GLM：`glm-4.7`；
- DeepSeek：`deepseek-v4-flash`。

用户选择其中一个模型后，必须用自己的 Key 完成连接测试，才能启用全部 Agent 能力。失败时不增加“仅部分 AI 功能可用”的例外；用户可以改选同一静态目录中的其他模型并重新测试。
