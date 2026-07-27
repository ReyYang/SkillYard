# SkillYard

SkillYard 是一个本地优先的 macOS 应用，用 Bundle 统一整理、安装、接管、挂载和更新 AI Agent Skills。

中文 | [English](./README.en.md)

## 下载与系统要求

[下载 SkillYard 1.0.0](https://github.com/ReyYang/SkillYard/releases/tag/v1.0.0)

SkillYard 1.0 支持：

- macOS 14 Sonoma 或更高版本；
- Apple Silicon（`arm64`）Mac；
- Codex、Claude Code 和 GitHub Copilot 的本地 Skill 目录。

在 Release 中下载 `SkillYard-1.0.0-macos-aarch64.zip` 和 `SHA256SUMS.txt`，然后校验安装包：

```bash
shasum -a 256 -c SHA256SUMS.txt
```

解压 ZIP，将 `SkillYard.app` 移入 `/Applications` 后启动。当前安装包使用 ad-hoc signing，尚未经过 Apple notarization；macOS 可能在首次打开时显示安全提示。请在 Finder 中按住 Control 点击应用并选择“打开”，或前往“系统设置 → 隐私与安全性”确认打开。仅从本仓库的 [GitHub Releases](https://github.com/ReyYang/SkillYard/releases) 下载正式安装包。

## 界面预览

![使用匿名测试数据展示的 SkillYard Bundle 清单](./docs/assets/skillyard-overview.png)

截图只包含匿名测试数据，不含真实账号、目录或 Source。

## 三条主路径

1. **扫描并接管**：首次使用时由你点击开始扫描。SkillYard 只读发现三个 Supported Apps 中已有的 Skill，并按 Bundle 展示；只有确认接管计划后，内容才会进入 Central Store，原使用位置会替换为软链接 Mount。
2. **安装并挂载**：从 GitHub Source、`skills.sh` 搜索结果、ZIP / `.skill`、直接归档 URL 或本地目录安装 Bundle。安装完成后默认保持“已安装、未挂载”，由你选择挂载到 Codex、Claude Code 或 GitHub Copilot。
3. **更新或删除 Bundle**：检查到 Source 变化后，由你确认更新整个 Bundle。你可以一次解除 Bundle 的全部 Mount，或通过二次确认删除 Bundle；删除 Source 只会移除远端更新关联，不会删除本地 Bundle。

## 安全与隐私边界

- **本地优先**：Central Store、SQLite 状态、Mount 和事务记录保存在本机，不依赖 SkillYard 账号、云端数据库或公共 registry。
- **显式确认**：扫描不会静默移动文件。接管、安装、挂载、更新和删除都从可见计划开始，高风险删除需要二次确认。
- **不执行外部安装命令**：SkillYard 不运行 `npx skills`、`gh skill`、Lark CLI 或用户提供的 shell 命令，也不执行 Skill 内的脚本、二进制文件或 lifecycle hook。
- **无遥测**：应用不收集使用分析，不上传崩溃报告，也不上传 Skill、Source、Project、路径或 SQLite 信息。只有你主动发起 Source 加载、搜索或更新时，应用才执行该操作所需的网络请求。
- **管理边界清晰**：Codex 官方插件、Host 内置 Skill 和明确由项目仓库维护的 Skill 只读展示，SkillYard 不修改它们。

## 1.0 未支持的内容

- Intel Mac、macOS 13 及更早版本、Windows、Linux 或其他平台；
- Apple Developer ID signing、notarization、Mac App Store 或无提示的首次启动体验；
- Homebrew Cask、DMG、应用自动更新或独立 CLI；
- 私有 GitHub 仓库认证、后台自动检查更新或公共 Skill registry；
- Skill 版本历史、用户可见回滚、备份恢复或跨设备同步。

## 从源码构建

需要 Apple Silicon Mac（macOS 14+）、Xcode Command Line Tools、Rust stable、Node.js 20.19+ 或 22.12+，以及 Corepack。仓库固定使用 `pnpm@10.33.2`。

```bash
xcode-select --install
corepack enable
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm tauri build --bundles app
```

应用生成在 `target/release/bundle/macos/SkillYard.app`。本地构建沿用 ad-hoc signing，不会获得 Apple notarization。

## 文档、贡献与反馈

- [文档索引](./docs/README.md)与 [1.0 产品契约](./docs/1.0-product-contract.md)
- [更新记录](./CHANGELOG.md)
- [贡献指南](./CONTRIBUTING.md)与 [行为准则](./CODE_OF_CONDUCT.md)
- [使用支持](./SUPPORT.md)
- [安全策略与私密漏洞报告](./SECURITY.md)
- [Bug 与功能建议](https://github.com/ReyYang/SkillYard/issues)
- [MIT License](./LICENSE)
