# Agent Fabric

Agent Fabric 是一套项目内、文件优先的协作框架。它把稳定的协作角色、使用方式和本机协作者配置分开，因此更换当前 Agent、外部工具或模型时，不需要重写整个框架。

它不会自动接管普通任务。只有用户明确要求使用 Agent Fabric 时，当前 Agent 才读取 [`skill/SKILL.md`](skill/SKILL.md) 并开始协作；当前 Agent 始终负责理解目标、保留权限、整合结果和向用户交付。

## 日常最简单用法

对当前 Agent 说：

> 请显式使用项目里的 Agent Fabric，帮我完成这个任务：……

如果当前宿主没有可识别的 Skill 投影，也可以说：

> 请读取 `.agent-fabric/skill/SKILL.md`，并按其中的协作流程完成这个任务：……

查看或更换协作者时说：

> 请使用 Agent Fabric 查看并重新配置本项目的协作者。

## 目录职责

- `skill/`：唯一的日常协作入口；正文保持精简，细节按需读取 `references/`。
- `roles/`：五个稳定 Role 的自我介绍，不绑定具体 Agent、模型或工具。
- `contracts/`：自由格式协作时可参考的内容建议，不是机器 Schema。
- `local/resolution.json`：当前机器的 Role、协作者、模型和调用方式；由初始化 Agent 或 Skill 维护。
- `local/adapters/`：按需保存本机连接说明，不是 Portable Catalog。
- `records/`：只保存已被当前 Agent 正式采纳或用户要求保留的整理后证据。
- `local/traces/`：只保存兼容性探测失败或排错材料，默认不进入 Git。
- `bin/`、`src/`、`tests/`：可选的 Rust 维护工具，不参与日常多 Agent 协作。

## 可选维护工具

如果本机有兼容的 `rustc`，可以构建并使用维护工具：

```bash
.agent-fabric/bin/build-runtime
.agent-fabric/bin/fabric check --project-root "$PWD"
.agent-fabric/bin/fabric verify --project-root "$PWD"
```

所有写入命令都必须逐字确认项目根：

```bash
.agent-fabric/bin/fabric repair --project-root "$PWD" --confirm-root "$PWD"
```

没有 `rustc` 时，Markdown、Role、Skill 和本机配置仍可正常使用；只是暂时不能构建或运行这些维护命令。`agent-run` 只用于初始化、重新配置或验收阶段的兼容性探测，不是日常协作入口。
