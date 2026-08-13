---
name: agent-fabric
description: 在当前项目中按需组织多 Agent 协作或配置协作者。
disable-model-invocation: true
---

# Agent Fabric

这是一个由用户调用的路由 Skill。只有用户明确调用 Agent Fabric 时才运行；项目中存在 `.agent-fabric/` 本身不构成调用。

## 选择分支

- 完成一项工作：完整读取并执行 [`references/collaborate.md`](references/collaborate.md)。
- 查看、发现、推荐或更换协作者：完整读取并执行 [`references/configure-collaborators.md`](references/configure-collaborators.md)。
- 两类请求同时出现：先配置协作者，再执行工作；两个 reference 都必须读取。

## 每个分支都遵守

- 用户正在直接使用的当前 Agent 承担 `orchestrator`，保留用户意图、权限、关键决策、结果复核和最终交付。
- 其他 Role 只描述职责，具体协作者、模型和调用方式以本机配置为准；不可用时明确停止该路由，不静默替换。
- 用户指令、批准和修改范围是硬边界。协作者只能在这些边界内行动，不能代替用户授权。
- 日常协作直接使用当前环境的真实能力，不调用 Rust、`agent-run` 或中央调度程序。
- 若当前宿主不能保证本 Skill 只由用户调用，先向用户说明这一限制。

当所选 reference 的完成条件全部满足，或已向用户说明无法继续的准确原因和影响时，本次调用才结束。
