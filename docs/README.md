# SkillYard 文档

本目录只保留会继续参与 SkillYard 当前版本设计、实施和验收的文档。讨论过程中的逐题 ADR 已删除；Git 历史负责保存历史，不再让已经被取代的决定与当前规则同时出现在工作区。

## 权威顺序

1. [1.0 产品契约](1.0-product-contract.md)定义已经发布的 Skill 生命周期承诺，1.1.0 不改变这些边界。
2. [1.1.0 产品规格](prd/0002-skillyard-1.1.0-agent.md)定义下一版本新增的可选 Agent、AI 整理、全网发现、语言能力和三套主题。
3. [1.0 PRD](prd/0001-skillyard-1.0.md)把 1.0 边界整理为完整用户故事、实施决策、测试决策和非目标。
4. [1.0 实施计划](plans/0001-skillyard-1.0-implementation.md)定义 1.0 已经采用的纵向实施顺序、阶段范围和验收门槛。
5. [1.0 接管与管理权设计](1.0-management-authority.md)定义生命周期承诺如何落到本地数据、文件、事务和界面行为。
6. [1.0 User Story 验收证据](acceptance/1.0-user-story-evidence.md)把 1.0 PRD User Story 映射到自动化、本机契约或最终人工体验。
7. [领域术语表](../CONTEXT.md)只统一 1.0 领域词义，不能单独增加产品能力。
8. `research/` 只保存已经核验的外部事实和选型依据，不能替代产品规格。

如果文档之间出现冲突，应先修正低层文档，不能通过继续增加补丁式决策文件来维持两套说法。

## 当前版本与下一版本

当前已发布版本是 SkillYard 1.0.1，面向 macOS 14 及以上的 Apple Silicon Mac，支持 Codex、Claude Code 和 GitHub Copilot。应用使用 ad-hoc signing，未经过 Apple notarization，也不提供应用自动更新。

可重复的自动化、macOS 应用契约和人工流程验收仍记录在 [1.0 User Story 验收证据](acceptance/1.0-user-story-evidence.md)，这些证据只验证现有产品承诺，不增加新的产品能力。

下一版本为 1.1.0。它在不改变现有 Skill 生命周期的前提下增加可选 Agent、AI 整理、全网 Skill 发现、简体中文／English 切换，以及 `Archive`、`Layers`、`Ledger` 三套主题；详细边界以 [1.1.0 产品规格](prd/0002-skillyard-1.1.0-agent.md)为准。

## 研究资料

- [cc-switch Skill 管理机制](research/cc-switch-skill-management.md)：可复用的界面与本地管理事实。
- [Skill Provider CLI 证据](research/skill-provider-cli-evidence.md)：`skills`、`gh skill` 和 Lark CLI 的真实能力与证据边界。
- [桌面技术选型](research/desktop-technology-options.md)：Tauri、Electron、Wails 与 Python UI 的对比依据。
- [Source 资源限制依据](research/source-resource-limits.md)：1.0 下载与展开上限的实测基础。
- [Agent API Provider 选型](research/agent-api-provider-selection.md)：1.1.0 三个固定 Provider 与薄 Adapter 的事实依据。
- [Provider 原生全网搜索](research/provider-native-web-search.md)：本地优先、Provider 服务端搜索和 `skills.sh` 边界。
- [Agent 静态支持模型目录](research/agent-supported-model-catalog.md)：首批模型、证据边界和用户侧能力验证要求。
- [Agent Markdown 渲染器选型](research/agent-markdown-renderer-open-source-survey.md)：Streamdown、流式 Markdown 和安全渲染边界。
- [多主题与多布局架构](research/ui-theme-architecture-market-survey.md)：Theme Preset、Library renderer 与公共业务状态的边界。
