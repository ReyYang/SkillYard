# 桌面应用多主题与多布局实现调研

> 调研基线：2026-07-30。
>
> 本文只使用产品官方文档、官方开发文档与官方仓库作为证据。市场事实与对 SkillYard 的工程判断分开陈述；本文不直接产生新的产品要求。

## 已确认的产品决策

SkillYard 内建 `Archive`、`Layers` 和 `Ledger` 三个用户可选的 Theme Preset。每个 Preset 由两层组成：

- `Appearance Theme` 作用于整个应用，统一背景、颜色、字体、边框、阴影、控件、弹窗、导航、图标和动效；
- `Library View` 只负责 Bundle Library 的浏览构图，分别呈现展台藏书、层叠卡片和主从清单。

三套 Theme Preset 共用路由、选择、搜索、筛选、Agent Session、领域数据和全部生命周期操作。Bundle 与 Skill 详情、安装、接管、挂载、更新、删除、设置、恢复和确认页面保持相同的信息层级、操作顺序和结果，但使用当前 Appearance Theme 的视觉语言呈现。

因此，“共用页面结构”不等于使用完全相同的外观；它表示三套主题不会分别实现或改变产品行为。

## 结论

市面上的成熟桌面应用没有把所有视觉变化都塞进同一种“Theme”机制，而是形成了三个层级：

1. **Theme**：替换颜色、字体、图标、纹理等视觉 token，不改变页面结构；
2. **View / Layout / Density Mode**：对同一批数据采用不同的浏览构图或密度，状态与业务行为仍由同一应用管理；
3. **Skin**：允许替换窗口、控件和导航，是接近“多套前端”的重型机制。

VS Code 与 Raycast 采用第一、二层分离；Finder 提供同一目录的多种 View；Obsidian 的 CSS 在技术上可以越过 token 改布局，但官方把依赖 DOM 结构的选择器视为兼容性风险；Kodi 则明确提供第三层的完整 Skin Engine。

SkillYard 选中的三张设计图不只是三套配色：

- **Archive** 是“主 Bundle 展台 + 横向藏书”的浏览构图；
- **Layers** 是“层叠卡片 + 当前 Bundle”的浏览构图；
- **Ledger** 是“Bundle 列表 + 详情面板”的浏览构图。

因此，它们不能实现成三份 CSS token，也不应实现成三套完整应用。较稳妥的市场同类做法是：

- 用户界面仍可把三套组合方案统称为“主题”；
- 代码中把 `Appearance Theme` 与 `Library View` 分层；
- 三套 `Library View` 消费同一份 Bundle 数据、选择状态、搜索与筛选状态，以及同一组操作；
- 安装、接管、挂载、更新、删除、设置和 Agent 不随主题产生不同产品行为。

## 市场实现对比

| 产品 | 官方名称 | 能否替换 token | 能否替换组件或布局 | 状态与业务行为关系 | 对 SkillYard 的价值 |
| --- | --- | --- | --- | --- | --- |
| VS Code | Color / Icon Theme | 是 | Theme 不可以；Layout 另行配置 | Theme 与 Workbench Layout 分离 | 最接近推荐的分层方式 |
| Raycast | Theme + Window Mode | 是 | Theme 不可以；Compact / Expanded 另行配置 | 同一功能结构上改变视觉和密度 | 证明“主题”和“密度”不必绑在一起 |
| Obsidian | CSS Theme / Snippet | 是 | CSS 技术上可以影响布局，但不是稳定的结构合同 | 功能由 App / Plugin 提供，Theme 主要负责外观 | 说明用 CSS 硬改结构的维护风险 |
| Finder | Icon / List / Column / Gallery View | 不以 Theme 为目标 | 是，同一目录有四种浏览结构 | 同一文件与目录状态，按 View 改变呈现 | 适合数据密集型 Bundle Library |
| Kodi | Theme + Skin | Theme 只换纹理；Skin 可全量替换 | Skin 可以改变控件、位置、导航和窗口 | 核心窗口与内容由 Kodi 提供，Skin 可重构交互层 | 说明重型 Skin 的能力和代价 |

## 1. VS Code：Theme 与 Layout 明确分层

VS Code 官方把主题分为三类映射：

- Color Theme：UI Component Identifier 与 Text Token Identifier 到颜色的映射；
- File Icon Theme：文件类型或文件名到图标的映射；
- Product Icon Theme：Workbench 内建图标集合。

这些扩展点没有提供替换组件树或页面导航的能力。[VS Code Theming](https://code.visualstudio.com/api/extension-capabilities/theming)

Workbench 的侧栏位置、Panel、View 拖放、Activity Bar、Editor Group 和网格布局属于独立的 Custom Layout。VS Code 还会跨 Session 记住 View 与 Panel 的位置。[VS Code Custom Layout](https://code.visualstudio.com/docs/configure/custom-layout)

官方同时提供 High Contrast Theme 和色觉辅助主题，说明每一套颜色方案都需要独立满足可读性，而不是只验证默认主题。[VS Code Accessibility](https://code.visualstudio.com/docs/configure/accessibility/accessibility)

**市场启示：**稳定主题 API 应依赖语义 token，而不是 DOM 位置；布局变化应使用独立状态和组件入口。Theme 与 Layout 可以同时持久化，但不应互相控制业务状态。

## 2. Raycast：Theme、尺寸与密度是三个设置

Raycast Theme Studio 允许调整背景或渐变、主要文字、主色和辅助色，并为 Light / Dark appearance 分别选择主题；官方没有提供通过 Theme 更换组件或导航结构的接口。[Raycast Themes](https://manual.raycast.com/themes)

Raycast 把以下能力作为 Theme 之外的 Appearance 设置：

- Interface Size：`Default`、`Large`、`Larger`；
- Window Mode：`Compact`、`Expanded`。

也就是说，即使只是让列表更密或窗口更大，Raycast 也没有把它伪装成颜色主题。[Raycast Settings](https://manual.raycast.com/settings#appearance)

Raycast Extension API 推荐使用会随 Theme 变化的语义 Color，并默认调整 Raw / Dynamic Color 的对比度。[Raycast Colors API](https://developers.raycast.com/api-reference/user-interface/colors)

**市场启示：**主题选择可以是一个面向用户的组合预设，但内部仍应把颜色、尺寸和浏览结构分开。这样后续调整某套配色时，不会意外改变信息密度或焦点顺序。

## 3. Obsidian：CSS 能越界，但官方不把结构选择器视为稳定合同

Obsidian 的 App UI 使用 CSS，并提供数百个 CSS variables。官方推荐 Theme 与 CSS snippet 通过覆盖这些变量完成外观适配。[Obsidian About styling](https://docs.obsidian.md/Reference/CSS%20variables/About%20styling)

CSS snippet 会叠加到当前 Theme 上，用于修改界面的部分外观。[Obsidian CSS snippets](https://obsidian.md/help/snippets)

因为底层是 CSS，开发者当然可以使用 `display`、`flex`、尺寸和位置等规则改变视觉布局。但官方 Theme Guidelines 明确指出：

- 优先使用 CSS variables；
- 使用低 specificity selector；
- App 的 class name 和 DOM nesting 可能变化，依赖它们的 selector 会失效；
- 避免用 `!important` 强行覆盖。

来源：[Obsidian Theme guidelines](https://github.com/obsidianmd/obsidian-developer-docs/blob/main/en/Themes/App%20themes/Theme%20guidelines.md)

**市场启示：**“用一份全局 CSS 把同一 DOM 扭成三种完全不同的页面”看起来代码少，实际会形成脆弱的选择器、隐藏元素和焦点顺序问题。SkillYard 自己控制源码，不需要复制这种社区 Theme 的兼容负担；结构差异应由明确的 React renderer 表达。

## 4. Finder：同一数据集的多种 View

Finder 可以把同一个文件夹显示为：

- Icon；
- List；
- Column；
- Gallery。

不同 View 强调的信息不同：List 使用多列，Column 展示层级与预览，Gallery 使用大预览和底部横向浏览。用户还可以为某个文件夹保存 View，并把同类 View 的设置设为默认。[Apple Finder User Guide](https://support.apple.com/en-mide/guide/mac-help/mchldaafb302/mac)

Finder 官方文档没有公开内部状态架构，但它的可观察产品合同很清楚：用户仍在同一个文件夹里操作同一批文件，只是浏览结构、信息密度和部分就近操作不同。这里的“状态与行为共用”是根据该产品合同得出的推论，不是对 Finder 私有源码的描述。

**市场启示：**SkillYard 的 Bundle Library 可以像 Finder View 一样具有多个 renderer。切换 renderer 时，Bundle 身份、当前选择、搜索结果和安装状态不变；每种 View 可以采用适合自己的键盘移动方式，但最终选择和操作必须回到同一应用状态。

## 5. Kodi：完整 Skin 是另一种重量级产品

Kodi 的 Skinning Manual 明确说明，Skin 可以改变：

- 图片、颜色、字体与文字；
- 控件尺寸和位置；
- 导航；
- 窗口；
- 甚至增加部分新功能。

每个标准窗口由一份 XML 描述，Skin 定义其中的 controls、布局和导航；部分窗口与必需控件仍由 Kodi 核心识别和填充内容。[Kodi Skinning Manual](https://kodi.wiki/view/Skinning_Manual)

Kodi 还专门区分了 Skin 内部的 Theme：Theme 只替换纹理包，布局保持不变。换言之，Kodi 自己也没有把“换颜色纹理”和“重做交互界面”混为一谈。

**市场启示：**Kodi 证明“多套完整布局”可以建立在同一核心数据与窗口协议上，但代价是为每个窗口维护 XML、控件 ID、焦点导航、条件可见性和兼容性。SkillYard 只有三套 Library 构图，不需要引入这种全应用 Skin Engine。

## 状态、路由与领域行为应该怎样共用

以下是基于上述市场证据对 SkillYard 的工程判断：

```text
同一份应用与领域状态
  ├─ 当前页面 / 返回历史
  ├─ selectedBundleId
  ├─ searchQuery / filter / sort
  ├─ Bundle、Skill、Source、Mount 数据
  └─ 安装、接管、挂载、更新、删除操作
              │
              ▼
      Library Presentation
        ├─ Archive renderer
        ├─ Layers renderer
        └─ Ledger renderer
              │
              ▼
        Appearance tokens
```

三种 renderer 只能决定“怎样浏览 Bundle”，不能各自保存一套 Bundle 状态，也不能各自实现安装、挂载或删除。

切换主题时至少应保持：

- 当前路由；
- 当前选中的 Bundle；
- 搜索、筛选和排序条件；
- 打开的 Agent Session；
- 未完成表单与操作状态。

不同 renderer 可以有自己的纯展示状态，例如 Archive 当前横向可见位置；这类状态不能进入 Bundle 领域模型，也不能改变操作结果。

## 维护、可访问性与测试成本

| 实现方式 | 维护成本 | 可访问性成本 | 测试成本 | 判断 |
| --- | --- | --- | --- | --- |
| 三套 token，共用全部组件 | 低 | 每套验证颜色、对比度、焦点可见性 | 一套流程测试 + 三套视觉检查 | 无法实现三张已选设计图 |
| 三套 Library renderer，共用状态与流程 | 中 | 每个 renderer 验证语义、键盘顺序和响应式；公共页面只验证 token | 公共流程一次；Library 交互与视觉按三套验证 | 推荐边界 |
| 三套完整 App / Kodi 式 Skin | 高 | 所有页面和弹窗重复验证 | 几乎是三套端到端产品矩阵 | 不符合 SkillYard 的实际需要 |
| 用全局 CSS 强扭一套 DOM | 初期低、长期高 | DOM 视觉顺序与键盘顺序容易分离 | 容易出现只在某套 Theme 复现的回归 | 不推荐 |

推荐方案并不是“只测试一套主题”。需要独立验证的内容包括：

- 三套主题的文字与状态色对比度；
- 三种 Library renderer 的空状态、少量 Bundle、大量 Bundle、长名称和窄窗口；
- 鼠标与键盘能完成选择、打开 Bundle 和触发公共操作；
- 切换主题前后，路由、选择、搜索和 Agent Session 不丢失；
- 三套 renderer 发出的操作都进入同一个安装、挂载、更新和删除入口。

不需要重复验证的内容包括：

- 每套主题各跑一遍 Rust 生命周期协议；
- 为每套主题复制安装或删除确认页；
- 为每套主题维护独立的 SQLite 字段或领域对象。

## 对三套候选主题的直接结论

### Archive

最具品牌感，但对大量 Bundle 和键盘浏览要求最高。实现时应把“书封、书脊、横向陈列”限制在 Library renderer；Bundle 详情和危险操作不能继续模仿书本隐喻，否则会把装饰语言扩张成业务交互。

### Layers

能保留收藏感，同时比 Archive 更接近普通详情页。层叠卡片应只代表可选择的 Bundle，不能把不可见的卡片变成无法访问的真实页面层级。

### Ledger

最适合大量 Bundle、长名称、状态比较和键盘操作，也最接近 Finder List / Column View。它适合作为默认主题或至少作为验收基准：另外两套 renderer 不能缺少 Ledger 能完成的主操作。

### 推荐讨论边界

下一步只需要确认一个产品问题：

> 三套“主题”是否只允许改变 **Bundle Library 的浏览构图 + 全局视觉 token**，而安装、接管、Bundle / Skill 详情、设置、确认弹窗和 Agent 使用同一套页面结构？

如果答案是肯定的，三套设计可以在保持产品一致性的前提下实现；如果每套主题还要改变所有功能页面的信息架构，那么它们就不再是 Theme，而是 Kodi 式 Skin，实施和验收范围会接近三套应用。
