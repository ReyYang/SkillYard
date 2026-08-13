---
name: agent-fabric
description: 使用当前项目的 Agent Fabric 按需组织多 Agent 协作或配置协作者。
disable-model-invocation: true
---

# Agent Fabric 项目投影

这是当前 Codex 宿主的可重建投影，不是第二份配置真值。只有用户明确调用 Agent Fabric 时才使用它。

开始任何配置或协作前，必须完整读取并执行项目内 canonical Skill：

[`../../../.agent-fabric/skill/SKILL.md`](../../../.agent-fabric/skill/SKILL.md)

Role、协作者、模型与调用方式以 `.agent-fabric/local/resolution.json` 为准；不得根据本投影自行补充绑定或 silent fallback。
