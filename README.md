# SkillYard

默认语言：中文 | [English](./README.en.md)

SkillYard 是一个 Mac-first 的本机 Skill Library Manager，面向同时使用多个 AI coding agent 的用户。它把 skill 的来源管理和不同 Host 的暴露入口分开：本机只维护一份可追踪、可更新的 Library，再按需把 Library Entry 暴露给 Codex、Claude Code、Cursor、GitHub Copilot 等 Host。

## 解决什么问题

多 agent 用户经常会遇到这些问题：

- skill 分散在 `.codex/skills`、`.claude/skills`、项目目录和各种工具目录里。
- 很多 skill 是通过 `npx`、`gh skill`、复制目录或历史脚本安装的，后来很难知道来源。
- 同一个 skill 被复制到多个 Host 后，远端更新无法自动同步。
- Host Entry 名称冲突、broken symlink、snapshot 漂移、Dirty Source Tree 等问题很难集中检查。
- 项目级 skill 和用户级 skill 混在一起，难以判断到底应该全局暴露还是只暴露给某个项目。

SkillYard 的核心思路是：把 skill 作为 Library Entry 管理，把 Host 目录里的入口作为 Exposure 管理。默认使用 symlink Exposure；当 symlink 不适合时再使用 snapshot fallback。

## 核心概念

- **Library**：本机统一的 skill source library。
- **Source Tree**：一个 Git repo、本地 personal 目录、npm package materialization 或 captured source。
- **Library Entry**：一个可管理的 skill，通常对应某个 `SKILL.md`。
- **Library Identity**：管理身份，格式是 `namespace/name`，例如 `mattpocock/review`。
- **Exposure**：把某个 Library Entry 暴露给某个 Host/scope 的关系。
- **Host Entry Name**：Host skill 目录里的真实目录名，默认等于 skill 自己声明的 `name`。
- **Plan / Apply**：所有写操作先生成 Plan；只有确认后才 Apply。

更多术语见 [CONTEXT.md](./CONTEXT.md)。

## 当前状态

这是 SkillYard 的首个可运行实现，包含：

- CLI：`init`、`import`、`expose`、`doctor`、`update`、`serve`
- SQLite State File
- Git / local / package / captured Source Tree
- symlink-first Exposure，snapshot fallback
- Codex、Claude Code、Cursor、GitHub Copilot Host Adapters
- Local Server + HTML View
- doctor 健康检查
- Update Impact preview
- Captured Install，用于捕获 `npx` 等外部 installer 的安装结果
- `gh skill` discovery
- 可选 AI Assist，用于解释 skill 和推断未知 provenance

## 安装

从源码运行：

```bash
git clone https://github.com/ReyYang/SkillYard.git
cd SkillYard
python3 -m skillyard --help
```

如果希望安装 `skillyard` 命令：

```bash
python3 -m pip install -e .
skillyard --help
```

默认 State File 位于：

```text
~/.skillyard/state.sqlite3
```

默认 Library 位于：

```text
~/.skillyard/library/
```

开发和测试时建议使用 `--home` 指向临时目录，避免影响真实本机 Library。

## 快速开始

初始化本机 Library：

```bash
python3 -m skillyard init
python3 -m skillyard init --yes
```

不带 `--yes` 时只输出 Plan，不会 Apply。带 `--yes` 才会写入 State File 和 Library 目录。

导入 Git repo：

```bash
python3 -m skillyard import https://github.com/example/agent-skills --namespace example
python3 -m skillyard import https://github.com/example/agent-skills --namespace example --yes
```

导入本地 personal skill 目录：

```bash
python3 -m skillyard import --local ~/GitHub/my-personal-skills --namespace personal --yes
```

暴露某个 skill 给 Codex：

```bash
python3 -m skillyard expose example/review --host codex --scope user
python3 -m skillyard expose example/review --host codex --scope user --yes
```

暴露给项目级 scope：

```bash
python3 -m skillyard expose example/review \
  --host codex \
  --scope project \
  --project-root /path/to/project \
  --yes
```

运行健康检查：

```bash
python3 -m skillyard doctor
```

预览 Source Tree 更新影响：

```bash
python3 -m skillyard update 1
python3 -m skillyard update 1 --apply --yes
```

启动本地 HTML View：

```bash
python3 -m skillyard serve --port 8765
```

Local Server 只允许绑定 localhost。

## Captured Install

如果用户仍想用原来的 installer，例如 `npx`，可以让 SkillYard 捕获安装前后的 Host 目录变化：

```bash
python3 -m skillyard import \
  --capture \
  --host codex \
  --host-dir /tmp/codex-skills \
  --yes \
  -- npx some-skill-installer
```

SkillYard 会：

1. 快照 Host skill 目录。
2. 执行真实 installer command。
3. 再次快照 Host skill 目录。
4. 识别 added / changed / deleted Host Entries。
5. 记录 Install Receipt。
6. 在证据足够时创建 Package Source Tree；证据不足时创建 Source Candidate。

## `gh skill` discovery

搜索公共 skill：

```bash
python3 -m skillyard import --gh-search review
```

从 discovery result 中拿到 repo URL 后，可以转为 SkillYard import plan：

```bash
python3 -m skillyard import --import-url https://github.com/example/agent-skills --namespace example
```

`gh skill` 只作为 Discovery Provider，不接管本地 Library state。

## AI Assist

AI Assist 是显式、可选能力，不参与核心写入流程。

解释一个 skill：

```bash
python3 -m skillyard doctor --explain-skill /path/to/skill
```

推断未知来源：

```bash
python3 -m skillyard doctor --infer-provenance /path/to/skill
```

推断结果是 Provenance Inference，不会自动写成 confirmed provenance。

## 开发

运行测试：

```bash
python3 -m unittest discover
```

编译检查：

```bash
python3 -m compileall skillyard tests
```

## 文档

- [Domain glossary](./CONTEXT.md)
- [PRD](./docs/prd/0001-skillyard.md)
- [Architecture decisions](./docs/adr/)
