# SkillYard 桌面技术选型

> 研究基线：2026-07-21。下文把官方文档可确认的能力与 SkillYard 的工程判断分开陈述。
>
> 决策状态：Tauri 2 + TypeScript Web UI + Rust Lifecycle Core 已写入 [SkillYard 1.0 产品契约](../1.0-product-contract.md)。本文只保留技术选型依据，不产生新的产品要求。

## 已确认约束

SkillYard 的桌面端必须同时满足：

- 仅支持 macOS arm64 14+；
- 接受 Web 风格 UI；
- 本地数据使用 SQLite；
- 需要创建和维护 symlink；
- 安装、升级与恢复按 Bundle 执行可恢复事务；
- 应用使用 Developer ID 签名并完成 notarization；
- 应用更新由用户触发，更新包具有独立签名，用户确认后才下载；
- 生产版本不运行或暴露 localhost 服务；
- 零遥测。

## 方案对比

| 方案 | 与约束的匹配度 | 主要优势 | 主要代价或缺口 | 判断 |
| --- | --- | --- | --- | --- |
| Tauri 2 | 高 | Web UI；Rust core 可直接承担 SQLite、文件系统、symlink、事务恢复和更新编排；updater 支持签名校验，并可把检查、下载、安装拆开；macOS 签名与 notarization 有官方流程 | 需要同时维护 TypeScript 与 Rust；权限、命令边界和发布链路需要认真设计 | 首选，维护成本中等 |
| Electron | 中高 | TypeScript/JavaScript 单语言栈；生态成熟；开发速度通常最快；renderer/main process 边界清晰 | Chromium/Node runtime 带来更大包体和运行开销；Electron 升级及 native SQLite addon 的 ABI 重建有持续成本；内置 `autoUpdater` 的检查流程会自动开始下载，不能直接满足“确认后下载” | 最现实的备选 |
| Wails | 中低 | Web UI + Go backend；v2 架构稳定，原生调用模型直接 | v2 缺少 v3 所展示的 updater/signing 整套能力；截至 2026-07-21，v3 仍为 Alpha，不适合作为 1.0 桌面产品的发布基础 | 排除 |
| Python + pywebview / PySide6 | 低 | 可最大化复用 Python 原型与既有业务代码；pywebview 提供轻量 Web bridge，PySide6 提供完整 Qt UI | macOS 打包、Developer ID/notarization、独立签名更新与可靠恢复需要更多自建工作；线程、bridge 和原生打包问题会形成长期维护负担 | 排除 |

## 结论

推荐采用 **Tauri 2 + TypeScript Web UI + 小而封闭的 Rust lifecycle core**。

这里的 Rust core 不应演变为通用后端。它只负责高风险、强一致性的生命周期能力：SQLite 访问、Bundle 规划与可恢复事务、symlink 变更、文件校验、应用更新，以及向 UI 暴露少量类型明确的命令。普通界面状态与交互留在 TypeScript。这样既利用 Web UI 的开发效率，也把文件系统和安装状态的写权限收进一个可审计边界；这些是针对 SkillYard 约束得出的工程判断，并非 Tauri 官方对应用架构的强制要求。

Tauri 2 最匹配，但不是最低维护成本方案：团队必须同时维护 Rust、TypeScript、capability 配置和 macOS 发布链路。其关键优势在于，官方 updater API 能把更新检查与 `downloadAndInstall` 分离，并对更新产物执行独立签名校验，因此可以实现“用户点击检查 → 展示版本 → 用户确认 → 下载并安装”。生产 UI 由应用内 WebView 加载打包资源，不需要 localhost；零遥测则应由 SkillYard 自身的依赖选择、构建配置和网络访问策略保证。

若团队更看重单语言和最快开发速度，可选择 Electron。它仍能实现本地资源 UI、SQLite、文件事务、签名和 notarization，但包体/runtime 更重；native SQLite addon 需要跟随 Electron ABI 处理 rebuild。尤其是 Electron 内置 `autoUpdater` 在调用检查后即开始下载，若必须严格做到“用户确认后才下载”，就需要改用自定义下载/更新流程或额外组件，这会削弱其简单性优势。

Wails v2 虽然稳定，但不具备 v3 文档所呈现的 updater/signing 完整路径；v3 截至研究日期仍为 Alpha，因此排除。Python + pywebview/PySide6 适合继续复用或验证原型，但在 macOS 分发、签名更新、恢复机制以及线程/bridge 的长期维护上最弱，因此不作为 1.0 技术底座。

## 官方来源

### Tauri 2

- [Architecture](https://v2.tauri.app/concept/architecture/)
- [Updater plugin](https://v2.tauri.app/plugin/updater/)
- [macOS code signing and notarization](https://v2.tauri.app/distribute/sign/macos/)
- [SQL plugin](https://v2.tauri.app/plugin/sql/)

### macOS 分发与支持范围

- [Preparing your app for distribution](https://developer.apple.com/documentation/Xcode/preparing-your-app-for-distribution)
- [Distributing software on macOS](https://developer.apple.com/macos/distribution/)
- [Accessing files from the macOS App Sandbox](https://developer.apple.com/documentation/security/accessing-files-from-the-macos-app-sandbox)
- [Current macOS versions](https://support.apple.com/en-us/109033)
- [Xcode system requirements](https://developer.apple.com/xcode/system-requirements)

### Electron

- [Process model](https://www.electronjs.org/docs/latest/tutorial/process-model)
- [`autoUpdater`](https://www.electronjs.org/docs/latest/api/auto-updater/)
- [Code signing](https://www.electronjs.org/docs/latest/tutorial/code-signing)
- [Using native Node modules](https://www.electronjs.org/docs/latest/tutorial/using-native-node-modules/)

### Wails

- [Repository and release status](https://github.com/wailsapp/wails)
- [Wails v2 architecture](https://wails.io/docs/howdoesitwork/)
- [Wails v3 self-update tutorial](https://v3.wails.io/tutorials/04-self-update-a-wails-app/)

### Python UI

- [pywebview architecture](https://pywebview.flowrl.com/guide/architecture)
- [PySide6 deployment](https://doc.qt.io/qtforpython-6/deployment/deployment-pyside6-deploy.html)
