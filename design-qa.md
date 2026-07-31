# SkillYard Library 三主题视觉验收

## 对照目标

- Source visual truth:
  - `docs/assets/theme-ledger-reference.png`
  - `docs/assets/theme-layers-reference.png`
  - `docs/assets/theme-archive-reference.png`
- Implementation full-view evidence:
  - Ledger、Layers、Archive 的 Pass 6 本地 VM 全屏对照。
- Latest VM evidence:
  - Ledger、Layers、Archive 的 Pass 7 本地 VM 截图。
  - Pass 6 半宽窗口本地 VM 截图。
- Focused icon comparison:
  - Ledger、Layers、Archive 的 Pass 7 设置入口局部对照。

## 视口与归一化

- 三张参考图均为 `1487 × 1058 px`。
- 实现使用 `1180 × 840 CSS px` 的统一设计坐标系。
- Pass 6 的 VM 应用内容截图为 `904 × 644 px`，按相同宽高比归一化为
  `1487 × 1058 px` 后，与参考图左右并排比较。
- Pass 7 的 VM 全屏截图为 `1224 × 768 px`；应用内部设计画布在当前窗口中按
  `min(windowWidth / 1180, windowHeight / 840)` 等比显示。Pass 7 只复核最后修改
  的设置图标，不用它替代无系统通知遮挡的 Pass 6 全屏对照。
- VM 截图为 `1x` 桌面截图；归一化只用于对齐可见尺寸，不作为图像相似度分数。

## 验收状态

- 环境：Apple Silicon macOS Tart VM，安装 production `.app`。
- 语言：简体中文。
- 页面：Bundle Library 首页，选中第一个可见 Bundle。
- 三个主题：`Ledger`、`Layers`、`Archive`。
- VM 中使用的真实本地 Bundle 与参考图 fixture 不同，因此 Bundle 名称、成员数、
  说明和挂载状态不参与像素级判断；导航、构图、静态文案、组件尺寸、间距、颜色、
  资产和交互位置仍按参考图判断。

## Findings

没有剩余可执行的 P0、P1 或 P2 视觉问题。

- 字体与排版：UI 字体、衬线标题、Monogram、导航与小字号信息已形成与参考图一致
  的层级；真实 VM 的字体栅格化比原始设计图略软，属于截图环境差异。
- 间距与构图：三主题的顶栏、主分栏、书脊/书封、详情起点、成员区和底部操作已按
  `1180 × 840` 坐标系对齐。窗口改变时整张画布等比缩放，不再重排或生成多重滚动条。
- 颜色与 Token：Ledger 的暖白与橄榄绿、Layers 的纸张与浅色书脊、Archive 的深紫
  画布与荧光绿操作色均与参考方向一致。
- 图片与资产：Archive 使用真实书封纹理；应用标识沿用项目资产；设置入口已改用
  `@phosphor-icons/react` 的真实 `GearIcon`，未使用字符、CSS 图形或手写 SVG。
- 文案：固定导航、搜索、添加、来源、挂载、成员和设置文案一致；Bundle 内容来自
  VM 真实数据，差异是预期状态而非 UI 漂移。
- 图标：三个主题的设置入口已与参考图统一为齿轮语义；导航与应用标识保持同一图标
  家族和光学尺寸。
- 响应式：半宽窗口截图证明页面、浮层和 Agent 入口使用同一缩放比例，主体保持完整，
  剩余空间由主题背景填充；没有横向滚动条、嵌套滚动条或持久控件裁切。
- 交互与可访问性：主题切换、Bundle 选择、设置入口和 Agent 入口在 VM 中可操作；
  设置按钮保留可访问名称，图标为 `aria-hidden`，键盘语义仍由原生 `button` 提供。

## Focused region evidence

- Ledger：底部设置入口的图标、文字间距和左侧基线已与参考图对齐。
- Layers：设置入口采用相同的轻量齿轮图标和文字布局；书脊与纸张仍保留既有层级。
- Archive：圆形设置按钮内部已从应用 Logo 改为齿轮图标，尺寸、颜色和圆形容器与
  参考图一致。
- 顶栏、Ledger 分栏与详情、Layers 书脊与纸张、Archive 书封与书架已在三张 Pass 6
  full-view comparison 中以相同像素尺寸比较，因此不再重复制作同一区域的局部图。

## Comparison history

1. 初始实现存在旧首页与新主题叠加、多个可见滚动条、导航与主体比例失真等 P1 问题。
   修复为单一 Library 外壳，并分别实现三套主题构图。
2. Pass 1–4 发现顶栏高度、左右分栏、书封/纸张位置、成员区和主操作间距存在 P2
   偏差。根据参考图坐标建立 `1180 × 840` UI 规范，并逐项校正主题 CSS。
3. Pass 5 在精确窗口中复核后，继续修正 Ledger 详情顶部、Layers 纸张内边距与
   Archive 主舞台/书架位置。修复后证据为三张 Pass 6 exact comparison。
4. Pass 6 发现设置入口仍使用应用 Logo 或纯文字，属于 P2 图标漂移。Pass 7 引入
   Phosphor `GearIcon` 并在同一 VM 重装 production `.app`；三张 focused comparison
   证明该问题已消除。
5. 用户补充窗口缩放要求后，删除会改变结构的响应式重排，改为完整设计画布统一比例
   缩放。Pass 6 半宽窗口 VM 验收证明缩小窗口后仍保持完整构图。

## Open Questions

无阻塞问题。当前对照使用真实 VM 数据而不是参考图专用 fixture，因此不声明逐文本的
像素相似度；这不影响静态 UI 与布局验收。

## Implementation Checklist

- [x] 三主题各自使用独立构图，共享同一产品状态和回调。
- [x] 使用固定设计坐标系加窗口等比缩放。
- [x] 移除首页多层滚动与结构性响应式重排。
- [x] 使用真实纹理和图标资产。
- [x] Frontend 单元测试、类型检查、Rust fmt/clippy 与 production build 通过。
- [x] 最新 ZIP 在 Tart macOS VM 重装并复核三主题。

## Follow-up Polish

- P3：参考图中的 Agent 装饰线比当前已确认的浮动入口更具装饰性；当前实现保留用户
  已确认的 Agent 图标和交互，不作为本次阻塞项。
- P3：VM 截图的字体抗锯齿和参考渲染不同，可在未来固定同一显示缩放后再做更细的
  光学字重微调。

final result: passed
