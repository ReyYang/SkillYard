# SkillYard

> 你的 Mac 上装了多少 AI Agent Skill？谁在管理它们？删掉会不会出问题？
>
> SkillYard 回答这三个问题——用 **Bundle** 统一管理散落在 Codex、Claude Code 和 GitHub Copilot 中的 Skill，让你看清、接管和掌控每一个文件的来源与去向。

中文 | [English](./README.en.md)

---

## 下载与系统要求

[下载 SkillYard 1.0.1](https://github.com/ReyYang/SkillYard/releases/tag/v1.0.1)

SkillYard 1.0 支持：

- macOS 14 Sonoma 或更高版本；
- Apple Silicon（`arm64`）Mac；
- Codex、Claude Code 和 GitHub Copilot 的本地 Skill 目录。

在 Release 中下载 `SkillYard-1.0.1-macos-aarch64.zip`。解压 ZIP，将 `SkillYard.app` 移入 `/Applications` 后启动。当前安装包使用 ad-hoc signing，尚未经过 Apple notarization；macOS 可能在首次打开时显示安全提示。请在 Finder 中按住 Control 点击应用并选择“打开”，或前往“系统设置 → 隐私与安全性”确认打开。仅从本仓库的 [GitHub Releases](https://github.com/ReyYang/SkillYard/releases) 下载正式安装包。

## 界面预览

![使用匿名测试数据展示的 SkillYard Bundle 清单](./docs/assets/skillyard-overview.png)

*截图只包含匿名测试数据，不含真实账号、路径或 Source。*

## 为什么需要 SkillYard

AI 编码工具各自维护了自己的 Skill 目录。安装来源五花八门——官方插件、`npx skills`、`gh skill`、GitHub 仓库、别人分享的 ZIP——时间一长，本地目录里堆满了你不确定能不能删、也不知道谁在负责维护的文件。

SkillYard 做的事很简单：**先把本机已有的全部发现出来**，让你看清楚什么在哪儿、从哪来的、当前由谁管理。然后由你决定——哪些交给 SkillYard 统一托管，哪些保持原样。

| 如果你在意 | SkillYard 的选择 |
|---|---|
| Skill 从哪里来、跟谁是一组 | 以 **Bundle**（本地安装组）为单位管理，保留 Source 追溯 |
| 更新失败后 Skill 会被搞乱吗 | 原子替换 `current` 软链接——验证不通过就不生效，中断后启动自动恢复 |
| 工具会不会静默修改或删除文件 | 接管、安装、挂载、更新、删除全部从可见计划开始，高风险操作二次确认 |
| 会不会把官方插件的内容弄坏 | 官方插件、Host 内置和项目仓库维护的 Skill **只读展示**，不会被修改 |
| 装了一个 Skill 就自动暴露给所有 Agent | 安装 ≠ 挂载。你可以让一个 Skill 只对 Codex 可见，或暂时不给任何 Agent 看 |
| 不同项目的 Skill 混在一起 | 支持 **global 和 project 两级挂载**，项目级 Skill 只出现在对应项目中 |

## 本地优先，信任有据

SkillYard 没有账号系统，没有云端数据库，没有公共 registry。Central Store、SQLite 状态、Mount 和事务记录全在本机。

- **不执行外部命令。** 不跑 `npx skills`、`gh skill`、Lark CLI 或任何 shell 命令，也不执行 Skill 内携带的脚本、二进制文件或 lifecycle hook。
- **零遥测。** 不收集使用分析，不上传崩溃报告。只有你主动加载 Source、搜索 `skills.sh` 或检查更新时，应用才发出该操作所需的网络请求。
- **前端没有文件系统权限。** SkillYard 使用 Tauri 2 构建：TypeScript UI 只负责展示，所有持久化状态和文件系统操作由 Rust Lifecycle Core 通过类型命令执行。前端不能直接访问文件系统、SQLite 或 shell。

## 三条路径

### 1. 扫描并接管

首次使用时你点击"开始扫描"。SkillYard 只读发现 Codex、Claude Code 和 GitHub Copilot 中已有的 Skill，按来源分组展示。**扫描不会移动任何文件。**

你确认接管计划后，SkillYard 才会把内容迁入 Central Store (`~/Library/Application Support/SkillYard/`)，并把原位置替换为指向受管副本的软链接。如果内容已经在合适位置，只做登记和链接校正，不做无意义搬运。

### 2. 安装并挂载

从 GitHub 仓库、`skills.sh` 搜索结果、ZIP / `.skill` 归档、直接 URL 或本地目录安装 Skill——总是一个完整的 Bundle，而不是散装条目。

安装完成后，Bundle 默认保持"已安装、未挂载"。由你选择挂载到 Codex、Claude Code 或 GitHub Copilot 的 global 目录，或者挂载到特定项目的 project 目录。Central Store 不被任何 Agent 扫描，所以你拥有完整的可见性控制。

### 3. 更新或删除

对有上游 Source 的 Bundle，由你主动触发"检查更新"。发现变化后，你确认执行更新——整个 Bundle 在一次原子操作中完成：候选内容全部验证通过后，替换一个 `current` 软链接，所有 Mount 自动跟随。

删除也分三层：移除挂载（Skill 回到"未挂载"但不丢内容）、删除 Source（只断开远端更新关联，不动本地）、删除 Bundle（二次确认后级联移除全部成员、挂载和受管内容，但不影响上游）。

## 架构：一个软链接决定一切

每个 Bundle 的当前受管内容通过一个名为 `current` 的软链接生效：

```
Bundle Directory
├── current → content-v2   # 原子替换此链接即完成更新
├── content-v1             # 验证通过后会被清理
└── content-v2
    ├── example-skill/
    └── another-skill/
```

所有 Mount（Codex / Claude Code / Copilot 里你看到的 Skill 入口）都指向 `current` 下的稳定成员路径。更新时只需替换 `current` 的目标，不用逐个 Mount 去改。

任何会修改文件系统的操作都在首次写入前建立 **Filesystem Transaction Journal**——记录操作计划、受影响路径、已完成步骤——并且每一步都设计为可以安全重复执行。应用意外退出或系统崩溃后，下次启动优先恢复未完成的事务。

## 下载与安装

[下载 SkillYard 1.0.0](https://github.com/ReyYang/SkillYard/releases/tag/v1.0.0)

**系统要求：** macOS 14 Sonoma 或更高版本，Apple Silicon（`arm64`）Mac。

下载 `SkillYard-1.0.0-macos-aarch64.zip`，解压后将 `SkillYard.app` 移入 `/Applications` 后启动。当前安装包使用 ad-hoc signing，尚未经过 Apple notarization，首次打开时 macOS 可能显示安全提示。请在 Finder 中按住 Control 点击应用并选择"打开"，或前往"系统设置 → 隐私与安全性"确认。

> 仅从 [GitHub Releases](https://github.com/ReyYang/SkillYard/releases) 下载正式安装包。

## 从源码构建

需要 Apple Silicon Mac（macOS 14+）、Xcode Command Line Tools、Rust stable、Node.js 20.19+ 或 22.12+，以及 Corepack。仓库固定使用 `pnpm@10.33.2`。

```bash
xcode-select --install
corepack enable
pnpm install --frozen-lockfile
pnpm typecheck && pnpm test
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm tauri build --bundles app
```

应用生成在 `target/release/bundle/macos/SkillYard.app`。本地构建沿用 ad-hoc signing，不会被 Apple notarize。

## 1.0 边界

这是一个明确的 1.0，不是"功能已完整"——是"核心生命周期已完整，边界很清楚"。

1.0 **包含：** Bundle 管理、首次扫描与接管、安装与挂载（GitHub / skills.sh / ZIP / 本地）、global 和 project 两级挂载、Source 关联与检查更新、完整 Bundle 原子更新、删除 Source / 删除 Bundle、Filesystem Transaction Journal 与启动恢复。

1.0 **不包含：** Intel Mac、macOS 13 及更早版本、Windows / Linux；Apple Developer ID signing / notarization / Mac App Store；Homebrew Cask、DMG、应用自动更新、独立 CLI；私有 GitHub 仓库认证、后台自动检查更新、公共 Skill registry；Skill 版本历史、用户可见回滚、备份恢复、跨设备同步。

更详细的产品边界见 [1.0 产品契约](./docs/1.0-product-contract.md)。

## 文档、贡献与反馈

- [产品契约](./docs/1.0-product-contract.md) 与 [管理权设计](./docs/1.0-management-authority.md)
- [更新记录](./CHANGELOG.md)
- [贡献指南](./CONTRIBUTING.md) · [行为准则](./CODE_OF_CONDUCT.md) · [使用支持](./SUPPORT.md)
- [安全策略与私密漏洞报告](./SECURITY.md)
- [Bug 与功能建议](https://github.com/ReyYang/SkillYard/issues)
- [MIT License](./LICENSE)
