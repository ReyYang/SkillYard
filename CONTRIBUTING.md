# 贡献指南

感谢你帮助改进 SkillYard。项目当前优先维护已经公开承诺的 1.0 主流程，提交应尽量小、可验证，并避免顺手扩张产品边界。

## 开始之前

- Bug 请使用 [Bug report](https://github.com/ReyYang/SkillYard/issues/new?template=bug_report.yml)。
- 功能建议请使用 [Feature request](https://github.com/ReyYang/SkillYard/issues/new?template=feature_request.yml)。
- 较大的行为变化请先创建 Issue，说明真实用户场景、期望结果和不包含的范围，再开始实现。
- 安全漏洞不要创建公开 Issue，请按照 [安全策略](./SECURITY.md) 私密报告。
- 不要在 Issue、截图、fixture、提交信息或测试输出中加入 Token、私钥、数据库、真实个人路径或其他敏感信息。

参与项目即表示同意遵守 [行为准则](./CODE_OF_CONDUCT.md)。

## 本地开发

支持的开发环境是 macOS 14+ Apple Silicon。需要 Xcode Command Line Tools、Rust stable、Node.js 20.19+ 或 22.12+、Corepack、`just 1.58.0`，以及仓库指定的 `pnpm@10.33.2`。

```bash
xcode-select --install
corepack enable
pnpm install --frozen-lockfile
```

启动开发应用：

```bash
pnpm tauri dev
```

## 修改原则

- 只修改解决当前 Issue 所需的内容，不增加未被请求的兼容层、配置项或抽象。
- 保持一个产品能力只有一套领域模型、持久化协议和生产入口。
- 涉及扫描、安装、接管、Mount、更新或删除时，通过公开 application seam 和真实临时文件系统验证行为。
- 新增或修改的代码应为非显然的业务规则、约束和安全边界添加简洁说明，避免注释重复语法。
- 不修改 Codex 官方插件、Host 内置 Skill 或项目仓库维护内容的只读边界。
- 不引入遥测，不执行外部安装命令，也不执行 Skill 携带的脚本或二进制文件。

## 验证边界

- 修改一个 Rust 行为时，用 `just rust-test <fully-qualified-name>` 运行 canonical integration target 中唯一匹配的测试；短名、零匹配和重复匹配都会失败。
- 普通切片完成后运行 `just slice`，覆盖格式、frontend、工程 guards、全部离线 Rust 测试、Clippy、wire 和 migration 当前检查，但不构建 App。
- 阶段完成或修改影响打包时运行 `just stage`；它在 slice 通过后构建 production App。
- `just release` 只用于发布准备交接。它先完成 stage，再以 exit 3 明确要求 tart、MAC-CONTRACT、人工产品路径和另行授权的真实 Provider 验收；它不会自动调用 Provider 或发布。

## 提交与 Pull Request

提交信息遵循 Conventional Commits：

```text
<type>(<scope>): <subject>
```

常用 `type` 包括 `feat`、`fix`、`docs`、`refactor`、`test`、`build`、`ci` 和 `chore`。一次提交只表达一个完整目的。

Pull Request 应包含：

- 关联 Issue 与用户可观察的问题；
- 实际变更及明确未变更的产品边界；
- 已运行的验证命令和结果；
- UI 变化的匿名截图；
- 尚未验证的条件或剩余风险。

请保持 PR 可审查，不要混入无关格式化、重命名或重构。维护者可能要求缩小范围或补充回归测试后再合并。
