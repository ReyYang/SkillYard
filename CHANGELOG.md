# 更新记录

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 的结构，并使用 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [1.0.1] - 2026-07-28

### 修复

- 单个损坏或断开的 Skill 不再阻止首次扫描或同一目录中其他 Skill 的刷新。
- 扫描告警按具体 Skill 路径分别保存；修复某个 Skill 后，只清除对应告警。
- 条目级扫描失败只把受影响的 Skill 标记为 stale，不再冻结整个扫描目录。

## [1.0.0] - 2026-07-27

### 新增

- 以 Bundle 为中心展示本机 Skill，并查看成员、来源、路径和 Mount 关系。
- 扫描 Codex、Claude Code 与 GitHub Copilot 的本地 Skill 目录。
- 只读展示 Codex 官方插件 Skill，不接管其生命周期。
- 从 GitHub Source、`skills.sh` 搜索结果、ZIP / `.skill`、直接归档 URL 和本地目录安装 Bundle。
- 通过确认计划接管已有 Skill，并使用 Central Store 和软链接 Mount 统一管理。
- 挂载到 Supported App 的 global 或 project Skill 目录，并支持整 Bundle 解除挂载。
- 检查并更新整个 Bundle；独立删除 Source 或经二次确认删除本地 Bundle。
- 使用本地 SQLite、文件系统事务记录和启动恢复保护生命周期操作。
- 提供无遥测、不执行外部安装命令的本地优先运行边界。

### 发布

- 提供适用于 macOS 14+ Apple Silicon 的 ZIP 安装包。
- 安装包使用 ad-hoc signing，尚未经过 Apple notarization。

[1.0.1]: https://github.com/ReyYang/SkillYard/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/ReyYang/SkillYard/releases/tag/v1.0.0
