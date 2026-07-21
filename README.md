# SkillYard

默认语言：中文 | [English](./README.en.md)

SkillYard 是一个面向 macOS 的本地 AI Agent Skill 管理应用。它让用户看清本机有哪些 Skill、来自哪里、被哪些项目和 Agent 应用使用，并把用户明确交付管理的内容统一安装、接管、挂载、更新和删除。

## SkillYard 1.0

1.0 的核心形态已经确定：

- 1.0 只在产品所有者的个人 Apple Silicon Mac 上本地构建和使用，`SkillYard.app` 是唯一入口；
- 不提供公开安装包、Apple 付费签名或应用自更新，也不提供 CLI、后台服务或 localhost API；
- 使用 Tauri 2、TypeScript Web UI 和小而封闭的 Rust Lifecycle Core；
- 1.0 仅支持 Apple Silicon 与 macOS 14 及以上系统；
- Supported Apps 固定为 Codex、Claude Code 和 GitHub Copilot；
- `Source` 表示远端来源，`Bundle` 表示本地已安装组，`Skill` 是 Bundle 成员，`Mount` 是到 Agent 应用目录的软链接；
- 扫描只产生 Inventory，不静默接管或移动文件；
- 直接安装后保持“已安装、未挂载”，由用户选择挂载目标；
- Bundle 更新采用完整候选内容验证和原子切换，不提供用户可见的版本历史或回滚；
- Central Store 与 SQLite 是持久用户内容，删除应用本体不会删除已托管 Skill；
- 不执行 `npx skills`、`gh skill`、Lark CLI 等外部安装命令，这些工具只作为 Installation Chain 和来源证据；
- 不收集遥测或上传崩溃报告。

## 当前状态

SkillYard 1.0 PRD 与十阶段实施计划已确认，20 个纵向实施 Issue 已发布。当前仓库已经包含可构建的 Tauri 2 应用骨架、首次使用介绍、用户授权后的只读扫描，以及持久化的基础 Inventory；实现按 [#19–#38](https://github.com/ReyYang/SkillYard/issues/18) 继续推进。

旧 Python CLI、HTML View、Local Server 和对应测试已经从当前工作区删除。仍然适用于 1.0 的行为约束已经写入 PRD 和实施计划，历史实现只通过 Git 记录保留。

## 本地开发

```bash
pnpm install
pnpm test
cargo test --workspace
pnpm tauri build --bundles app
```

生产构建生成在 `target/release/bundle/macos/SkillYard.app`。这是个人电脑上的本地构建，不包含公开分发、Developer ID 签名或 notarization。

## 文档

- [文档索引](./docs/README.md)
- [SkillYard 1.0 产品契约](./docs/1.0-product-contract.md)
- [SkillYard 1.0 PRD](./docs/prd/0001-skillyard-1.0.md)
- [SkillYard 1.0 实施计划](./docs/plans/0001-skillyard-1.0-implementation.md)
- [SkillYard 1.0 接管与管理权设计](./docs/1.0-management-authority.md)
- [领域术语表](./CONTEXT.md)

`docs/research/` 保存外部工具和技术选型的核验事实，不产生新的产品决策。历史讨论由 Git 记录，不再在当前工作区保留被取代的 ADR 文件。
