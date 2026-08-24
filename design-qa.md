# SkillYard Open Design 交互稿视觉验收

## 对照目标

- 视觉真值：Open Design 项目 `f4e8cf8a-62d8-4ccd-82ab-5004b140b199` 中的
  `skillyard-1to1.html`。
- 实现范围：Layers、Ledger、设置页、Agent 抽屉与全局顶栏。
- Archive 不再属于主题契约；安装本地归档文件的业务能力不在本次视觉删除范围内。

## 视口与方法

- 参考稿与本地实现都使用 `1180 × 840 CSS px`。
- Layers、Ledger、设置页、Agent 抽屉均在相同视口与对应状态下截图。
- 每个页面都把参考稿和本地实现放入同一张横向对照图后再检查，没有只凭单张截图判断。
- 临时浏览器数据只用于复现完整清单与挂载状态，已在交付前从源码中删除。

## 视觉结论

- 顶栏保留参考稿的暖灰背景、细分隔线、衬线标识、橄榄色当前项和右侧搜索/添加操作。
- 页面没有绘制 macOS 红黄绿窗口按钮；系统窗口装饰仍由 macOS 自己负责。
- Layers 的当前 Bundle 是展开纸张，其余 Bundle 才作为书脊出现；书脊上下端均为圆角。
- Ledger 使用高密度主从清单，并保留 Monogram、Skill 数量和挂载状态层级。
- 设置页沿用同一顶栏和页面坐标系；主题、Provider、模型与语言下拉框使用统一圆角、边框和箭头资产。
- Agent 抽屉四角均为圆角；关闭只收起抽屉，结束会话才清空内容并取消进行中的请求。
- Agent 入口使用从新应用图标抽取的新芽与纸张语义，不直接缩放应用图标，也不使用被否决的粉色图标。
- Tauri 应用图标已使用用户提供的新图标，旧 `T.` SVG 标识不再参与应用打包。

## 交互检查

- Bundle 鼠标选择与键盘切换可用。
- Layers 在选中书脊变成纸张后，会把键盘焦点移交给新详情，避免焦点丢失。
- Layers 与 Ledger 切换时保留当前 Bundle、搜索条件和 Agent 会话。
- Agent 抽屉的关闭与结束会话是两个独立动作。
- Provider 高级设置可以展开，既有 API Key、连接测试和启用状态能力仍可操作。
- 刷新本机与检查更新继续使用既有产品入口，没有合并生命周期行为。

## 验证结果

- 浏览器控制台错误：`0`。
- Frontend：`206 passed`，`pnpm typecheck` 通过，`pnpm build` 通过。
- Rust：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`
  与 `cargo test --workspace` 通过。
- `ThemePreset` 领域解析继续拒绝 `archive`；仅对 schema 30 开发版数据库中的旧偏好执行一次显式 forward migration，将其归一化为默认 `Ledger`。
- macOS：`pnpm tauri build --bundles app` 与 `codesign --verify --deep --strict` 通过。

## Open Questions

无阻塞问题。

Final result: passed
