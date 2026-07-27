# SkillYard 文档

本目录只保留会继续参与 SkillYard 1.0 设计、实施和验收的文档。讨论过程中的逐题 ADR 已删除；Git 历史负责保存历史，不再让已经被取代的决定与当前规则同时出现在工作区。

## 权威顺序

1. [1.0 产品契约](1.0-product-contract.md)定义产品向用户承诺什么，是当前最高层产品边界。
2. [1.0 PRD](prd/0001-skillyard-1.0.md)把已确认边界整理为完整用户故事、实施决策、测试决策和非目标。
3. [1.0 实施计划](plans/0001-skillyard-1.0-implementation.md)定义已经确认的纵向实施顺序、阶段范围和验收门槛。
4. [1.0 接管与管理权设计](1.0-management-authority.md)定义这些承诺如何落到本地数据、文件、事务和界面行为。
5. [1.0 User Story 验收证据](acceptance/1.0-user-story-evidence.md)把每条 PRD User Story 映射到自动化、本机契约或最终人工体验。
6. [领域术语表](../CONTEXT.md)只统一词义，不能单独增加产品能力。
7. `research/` 只保存已经核验的外部事实和选型依据，不能替代产品契约。

如果文档之间出现冲突，应先修正低层文档，不能通过继续增加补丁式决策文件来维持两套说法。

## 当前版本

SkillYard 1.0 面向 macOS 14 及以上的 Apple Silicon Mac，支持 Codex、Claude Code 和 GitHub Copilot。官方发布物是 GitHub Releases 中的 ZIP 与对应 SHA-256 校验文件；应用使用 ad-hoc signing，未经过 Apple notarization，也不提供应用自动更新。

可重复的自动化、macOS 应用契约和人工流程验收仍记录在 [1.0 User Story 验收证据](acceptance/1.0-user-story-evidence.md)，这些证据只验证现有产品承诺，不增加新的产品能力。

## 研究资料

- [cc-switch Skill 管理机制](research/cc-switch-skill-management.md)：可复用的界面与本地管理事实。
- [Skill Provider CLI 证据](research/skill-provider-cli-evidence.md)：`skills`、`gh skill` 和 Lark CLI 的真实能力与证据边界。
- [桌面技术选型](research/desktop-technology-options.md)：Tauri、Electron、Wails 与 Python UI 的对比依据。
- [Source 资源限制依据](research/source-resource-limits.md)：1.0 下载与展开上限的实测基础。
