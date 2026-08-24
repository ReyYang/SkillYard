# Rust 范围导航

根级 [Agent 指令](../AGENTS.md) 的安全、阶段边界、单一实现与架构变更门在本目录继续生效；本文件只补充 Rust 范围的导航，不重复根规则。

## 先路由上下文

- **用户可见行为或产品边界争议：**先读 [1.0 产品契约](../docs/1.0-product-contract.md)，以它裁决产品承诺。
- **Module owner、依赖方向、测试拓扑、Cargo 治理或 Train B ticket：**先读唯一 [Rust 工程规格](../docs/design/0002-rust-engineering-spec.md)，不要把目标架构复制到其他文档。
- **选择或运行验证：**先查唯一 [justfile](../justfile)，按改动风险选择下列 public recipe；本文件不缓存其内部命令。

## Canonical seam

`SkillYardApplication` 是最高且唯一的 application behavior seam。主行为验收从它或 typed Tauri client 进入，并使用真实临时文件系统、SQLite 与重启后的持久状态观察结果。私有 helper 测试只补算法边界，不能代替公开 seam。

## 变更目标

以下依赖方向只约束新代码和对应 ticket 内的重构，不表示 Train B 目录迁移已经完成；owner 尚未迁移时保留当前 owner。

| 范围 | 目标方向 |
| --- | --- |
| Domain | 只含 invariant、值对象与纯逻辑，不依赖 SQLite、Tauri、Provider 或 filesystem transaction。 |
| Persistence | 一个具体 `Storage` 只依赖 Domain；不拥有 network、Provider 或 Application dispatch。 |
| Lifecycle / Application | 协调既有且唯一的 Plan、Journal 与 recovery 语义；每种行为只扩展对应 ticket 指定的 canonical owner。 |
| Filesystem safety | 只提供低层安全效果，不反向依赖 Domain workflow、Persistence 或 Agent。 |
| Source / Agent | 外部能力通过最小 Adapter interface 进入；Agent 保持只读智能层。 |
| Tauri Adapter | 只校验输入、构造 canonical Intent、调用 Application、映射 Outcome，不复制状态机。 |

只有职责拥有独立 invariant、封装共享知识或具有多个真实调用者时才建立 Module，避免薄转发。约 `500` 行是可读性方向；超过约 `800` 行触发职责审查，二者都不是自动拆分线或验收失败线。

## 危险能力 owner

- SQLite connection 与 migration 继续由一个具体 `Storage` canonical owner 负责；released prefix 由 `migration` recipe 守护。
- 直接 filesystem 或 lifecycle mutation 必须扩展当前 ticket 指定的 canonical owner，不得建立旁路。对应 Train B ticket 尚未执行时，保留当前 owner，不提前建立目标结构。

## 验证触发

- Wire 或跨语言协议变更：`wire`；Tauri 跨语言行为还应由其中的 typed client 覆盖，再运行 `slice`。
- Migration 变更：`migration`。
- Lifecycle 或 unsafe filesystem 变更：先用 `rust-test` 验证相关公开 seam，再运行 `slice`；阶段完成运行 `stage`。
- 打包影响：`stage`。
- `mac-contract`、真实 Provider 与 `release` 门只在环境已准备且取得对应授权后执行。

## 停止门

若改动需要第二套 Plan、Journal 或 recovery 协议，带版本后缀的 Domain 类型，保留旧生产入口或新增公开 seam，只能通过私有步骤测试，或跨入下一阶段边界，立即停止并请求确认。根级架构门仍是更强约束。

## 完成条件

完成一个切片前，必须能解释 owner 与依赖方向，以公开 seam 完成 Red→Green，运行相关 public recipe，核对 diff scope，并确认没有第二套实现。阶段票还需通过 `stage` 与独立审查。
