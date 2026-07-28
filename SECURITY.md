# 安全策略

## 支持范围

| 版本 | 安全修复 |
| --- | --- |
| `1.0.x` | 支持 |
| `< 1.0` | 不支持 |

SkillYard 1.0 仅支持 macOS 14+ Apple Silicon。其他系统或架构上的问题不属于当前安全支持范围。

## 私密报告漏洞

请使用 GitHub 的 [Private vulnerability reporting](https://github.com/ReyYang/SkillYard/security/advisories/new) 提交安全问题。不要创建公开 Issue，也不要在 Pull Request、讨论、截图或公开复现仓库中披露尚未修复的漏洞。

报告中请尽量包含：

- 受影响的 SkillYard 版本、macOS 版本和 Apple Silicon 型号范围；
- 可重复的最小步骤、预期行为与实际行为；
- 可能影响的数据、文件系统范围和攻击前提；
- 已去除个人路径、Token、私有 Source 和真实 Skill 内容的最小证明；
- 如已知，建议的修复或缓解方式。

请勿上传真实数据库、Central Store、私钥、访问 Token、完整用户目录或包含第三方敏感内容的归档。维护者会在能力范围内确认、评估并协调披露，但当前社区项目不承诺固定响应时限。

## 值得报告的问题

包括但不限于：

- 归档路径穿越，或软链接、硬链接、特殊文件验证被绕过；
- 未经用户确认访问、覆盖或删除约定范围之外的文件；
- Mount、Central Store、SQLite 或事务恢复导致的权限或完整性问题；
- 应只读的 Codex 插件、Host 内置或项目仓库内容被修改；
- 应用意外执行外部安装命令、Skill 脚本、二进制文件或 lifecycle hook；
- 敏感本地信息被意外发送到网络或写入公开输出；
- 正式 Release 的资产与 `SHA256SUMS.txt` 不一致。

## 已知分发边界

正式安装包使用 ad-hoc signing，未使用 Apple Developer ID，也未经过 notarization。macOS 首次打开时出现正常 Gatekeeper 提示本身不是安全漏洞；请先核对文件来自本仓库的 GitHub Releases。

SkillYard 会在用户明确发起 Source 加载、搜索、检查或更新时访问对应网络来源。它不提供遥测，不上传崩溃报告，也不执行 Source 或 Skill 携带的代码。如果实际行为超出这些边界，请按安全问题私密报告。
