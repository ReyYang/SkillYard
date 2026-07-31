# SkillYard 1.1.0 Agent API Provider 选型

> 修订日期：2026-07-29
>
> 范围：为 SkillYard 1.1.0 的全局 Agent、AI 整理和全网 Skill 发现选择 API Provider。本文件提供选型依据；产品边界以 1.1.0 产品规格为准。

## 修订说明

本文原先建议只接入 Alibaba Cloud Model Studio `qwen3.7-plus`，并由 SkillYard 自建本地 Rust Agent Harness、搜索和网页读取工具。后续产品讨论已经明确撤回这两个结论：

- 1.1.0 首批固定支持 OpenAI、智谱 GLM 和 DeepSeek；
- 用户从每个 Provider 的静态支持模型列表中选择一个全局模型，并用自己的 Key 验证当前选择；
- 全网搜索由所选 Provider 的服务端搜索完成；
- SkillYard 只实现薄 Adapter、上下文组装、敏感过滤和结果归一，不建设通用 Tool Loop、搜索引擎或网页抓取 Harness。

Qwen 的本地运行与硬件要求研究仍可作为未来参考，但不属于 1.1.0 的实现方向。

## 结论

SkillYard 1.1.0 采用 API-first、BYOK、三 Provider 的方案：

| Provider | 普通请求协议 | 服务端全网搜索 | 1.1.0 接入结论 |
| --- | --- | --- | --- |
| OpenAI | Responses API | Responses `web_search` | 内建薄 Adapter |
| 智谱 GLM | Chat Completions | 专有 `type: "web_search"` 扩展 | 内建薄 Adapter |
| DeepSeek | 官方 Anthropic-compatible Messages | Anthropic server-tool 兼容路径 | 内建特殊 Adapter，只支持 DeepSeek 官方 Endpoint |

这三种协议不能被一个“任意 OpenAI-compatible Base URL”可靠覆盖。SkillYard 应共享产品语义和返回结构，但明确保留三个 Provider 的请求、搜索、引用与错误差异。

用户一次只选择一个 Provider 和一个模型。这个选择同时服务：

- 全局对话；
- Skill 说明、分类和使用场景；
- 中英文输出；
- 来源不明 Skill 的辅助调查；
- 本地优先后的全网 Skill 搜索。

不提供按功能选模型、自动 fallback 或后台模型路由。

## 产品需要的能力

Agent 只负责只读理解与发现：

| 场景 | Agent 需要完成 | SkillYard 继续负责 |
| --- | --- | --- |
| Skill 理解 | 根据实际可读文件生成说明、固定分类、适用场景和使用方法 | 解析 Skill 边界、过滤敏感内容、校验结构并保存派生数据 |
| 本地 Skill 查找 | 根据实际内容判断是否已有合适 Skill | 提供当前 Inventory 与允许读取的文本 |
| 全网 Skill 查找 | 使用 Provider 托管搜索返回带真实引用的候选 | 决定何时允许搜索，并把可解析候选交给现有安装预览 |
| 当前页面问答 | 解释 Bundle、Skill、Source、Mount 和只读状态 | 用稳定 ID 解析页面上下文，不开放任意本机路径 |

安装、接管、挂载、更新、解除挂载和删除继续由现有 Rust Lifecycle Core 执行。Agent 不能直接写 SQLite、Central Store 或 Host 路径，也不能自行确认影响预览。

## 为什么选择三个固定 Provider

### OpenAI

OpenAI Responses API 把普通推理、Structured Outputs 和托管 `web_search` 放在同一个正式接口中。逐模型页面能够明确说明 Function Calling、Structured Outputs 和 Web Search 支持，适合建立可核验的静态模型目录。

主要代价是用户必须自行满足 OpenAI 的账号、地区和计费条件。SkillYard 不代理账号，也不负责网络可用性。

官方依据：[Responses API](https://developers.openai.com/api/docs/guides/migrate-to-responses)、[Web Search](https://developers.openai.com/api/docs/guides/tools-web-search)、[模型目录](https://developers.openai.com/api/docs/models)。

### 智谱 GLM

智谱同时提供普通 Chat Completions、结构化输出和服务端 Web Search，适合中文说明、翻译和 Skill 发现。其搜索字段是智谱自己的协议扩展，不能通过通用 OpenAI-compatible Adapter 自动推断。

官方文档没有给出现代文本模型与 Web Search in Chat 的完整逐模型矩阵。因此 SkillYard 只能把它们列为依据官方资料维护的支持候选；用户仍需用自己的 Key 验证当前选择，不能把列表本身理解为当前账号一定可用。

官方依据：[模型概览](https://docs.bigmodel.cn/cn/guide/start/model-overview)、[对话补全 API](https://docs.bigmodel.cn/api-reference/%E6%A8%A1%E5%9E%8B-api/%E5%AF%B9%E8%AF%9D%E8%A1%A5%E5%85%A8)、[联网搜索](https://docs.bigmodel.cn/cn/guide/tools/web-search)。

### DeepSeek

DeepSeek 是用户明确要求的首批 Provider。1.1.0 使用其官方 Anthropic-compatible Endpoint：

```text
https://api.deepseek.com/anthropic
```

官方兼容表包含 `server_tool_use` 和 `web_search_tool_result`，Claude Code 集成文档也明确由 DeepSeek API 执行搜索。普通 OpenAI-compatible Chat Completions 不提供同等的托管搜索语义，因此 DeepSeek 必须使用独立 Adapter。

这项支持只针对 DeepSeek 官方 Endpoint，不能被描述为支持任意 Anthropic-compatible Provider。独立应用请求服务端搜索的完整字段由离线合同测试覆盖，并在用户验证当前 DeepSeek 模型时通过真实请求确认。

官方依据：[Anthropic API 兼容说明](https://api-docs.deepseek.com/guides/anthropic_api/)、[Claude Code 集成](https://api-docs.deepseek.com/quick_start/agent_integrations/claude_code/)、[模型与价格](https://api-docs.deepseek.com/quick_start/pricing/)。

## 薄 Adapter 的边界

SkillYard 需要的本地 AI 层仅承担：

1. 从 macOS Keychain 读取当前 Provider 的 API Key；
2. 从静态目录读取用户选择的模型；
3. 用稳定领域 ID 解析当前页面和本地 Skill 上下文；
4. 读取完成任务所需的普通文本；
5. 过滤本机敏感文件和敏感片段；
6. 构造该 Provider 的普通请求、结构化输出或服务端搜索请求；
7. 把文本、固定 Schema、引用和 Provider 错误归一为 SkillYard 可显示的结果；
8. 在任何写操作前退出 Agent 流程，返回现有确定性 UI。

1.1.0 不需要：

- 通用 Tool Loop；
- 本地搜索引擎或网页 Crawler；
- Browser 自动化；
- Shell 或代码执行；
- Provider 能力自动发现；
- 自定义 Base URL；
- 自定义模型 ID；
- 多模型 Router；
- Provider fallback；
- 云端 SkillYard 代理服务。

这里的“薄”不是指把不同 Provider 假装成完全相同，而是只适配 SkillYard 已确认的几个只读任务，不把它扩张成通用 Agent Runtime。

## 一个全局模型

每个 Provider 提供随应用维护的静态支持列表。详细 ID、证据边界和用户验证要求见 [Agent 首批静态支持模型目录](agent-supported-model-catalog.md)。

共同准入条件是：

1. 能完成普通中英文对话和 Skill 说明；
2. 能生成固定分类与概要 Schema；
3. 能用同一个模型完成 Provider 服务端全网搜索；
4. 能返回 SkillYard 可展示的真实 URL 或引用；
5. 不发生无法识别的静默模型替换；
6. 通过 SkillYard 的固定真实样本测试。

只有满足全部条件的模型才显示。不能把模型拆成“只可聊天”“只可分类”或“不可搜索”等部分功能状态，因为用户选择的是全局模型。

## 凭据与隐私

- API Key 只进入 macOS Keychain，不写 SQLite、日志或公开测试夹具。
- SQLite 只保存 Provider、模型、启用状态、语言和一次性披露状态等非敏感配置。
- SkillYard 在发送前阻止敏感文件，并移除 Token、Authorization Header、带凭据 URL、个人邮箱、用户名和个人绝对路径等本机敏感信息。
- Skill 内容是不可信数据，不能借助 Prompt 文本获得额外文件、网络或生命周期权限。
- 用户首次启用 AI 时看到一次数据披露；不做每次请求确认。
- Provider 的保留、计费和地区规则由 Provider 与用户之间的账号关系决定。SkillYard 不对这些外部条件提供代理或保证。
- Agent 请求属于用户启用的功能请求，不增加分析遥测、设备标识或崩溃上传。

## 网络与错误

SkillYard 不负责判断用户网络为什么无法连接 Provider，也不实现自动切换、离线队列或复杂重试。请求失败时展示足够理解的 Provider 错误，非 AI 功能继续工作。

设置中的“测试连接”只在用户主动点击后验证当前 Key 和所选模型。它发送一次固定 Schema 请求和一次必须返回真实 URL 的服务端搜索请求；两项都通过后，当前配置才能启用 AI。界面需要提前说明这两次真实请求可能由 Provider 计费。

## 合同测试与用户验证

普通 CI 使用 Fake Server 覆盖每个 Provider Adapter 的：

- 普通响应与固定 Schema 解析；
- 服务端搜索请求和 URL 引用归一；
- 无余额、限流、无权限和超时的错误语义；
- 请求与响应中的模型身份；
- Skill 内容中的 Prompt Injection 不扩大权限；
- 本地有合适 Skill 时不发起网络搜索。

真实接口是否可用于当前账号，由用户自己的 Key 验证当前选择。验证不遍历其他候选模型，不上传结果，也不形成用户可见的模型状态目录。更换 Provider、模型或 Key 后需要重新验证；失败时 AI 保持未启用，不降级成部分功能模式。

真实 Key 不能进入普通 CI、外部 PR、测试夹具、公开日志或截图。维护者不需要持有全部候选模型的访问权限。

## 未选择的方向

- **Qwen 单 Provider**：不是因为模型不可用，而是产品已经明确选择三家 Provider，并要求 Provider 托管全网搜索。
- **本地模型**：会增加模型分发、硬件兼容、推理性能和应用体积，不进入 1.1.0。
- **Anthropic、Gemini、Kimi 等其他 Provider**：本次没有产品需求，不进入首批实现；这不代表对其能力作负面判断。
- **任意兼容接口**：无法保证服务端搜索、结构化输出、引用和错误协议，也无法提供 SkillYard 内置 Adapter 的明确支持边界。
- **SkillYard 自建搜索 Harness**：用户需要全网搜索，而不是只查一个目录；让 Provider 执行搜索能避免本地 Tool Loop 和 Crawler 膨胀。

## 最终决策

SkillYard 1.1.0 使用 OpenAI、智谱 GLM 和 DeepSeek 三个固定 Provider 的薄 Adapter。用户选择一个全局模型，所有 AI 能力共用它；全网搜索使用该 Provider 的服务端能力。

这项方案保留了 Agent 带来的解释、分类、翻译和发现价值，同时不把 SkillYard 变成通用 Agent Framework、搜索引擎或新的生命周期执行器。
