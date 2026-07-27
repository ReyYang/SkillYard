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
- 通过 GitHub Releases 发布 ad-hoc signed ZIP，不使用 Developer ID 或 notarization；
- 应用更新由用户下载新的官方 ZIP 后手动替换，不内置 updater；
- 生产版本不运行或暴露 localhost 服务；
- 零遥测。

## 方案对比

| 方案 | 与约束的匹配度 | 主要优势 | 主要代价或缺口 | 判断 |
| --- | --- | --- | --- | --- |
| Tauri 2 | 高 | Web UI；Rust core 可直接承担 SQLite、文件系统、symlink 与事务恢复；使用系统 WKWebView，发布包无需捆绑 Chromium | 需要同时维护 TypeScript 与 Rust；权限、命令边界和发布链路需要认真设计 | 首选，维护成本中等 |
| Electron | 中高 | TypeScript/JavaScript 单语言栈；生态成熟；开发速度通常最快；renderer/main process 边界清晰 | Chromium/Node runtime 带来更大包体和运行开销；Electron 升级及 native SQLite addon 的 ABI 重建有持续成本 | 最现实的备选 |
| Wails | 中低 | Web UI + Go backend；v2 架构稳定，原生调用模型直接 | 需要把生命周期核心改写为 Go；截至 2026-07-21，v3 仍为 Alpha，不适合作为 1.0 的稳定基础 | 排除 |
| Python + pywebview / PySide6 | 低 | Python 开发效率高；pywebview 提供轻量 Web bridge，PySide6 提供完整 Qt UI | 需要额外处理 Python runtime、线程、bridge、macOS 打包和可靠恢复，长期维护边界更宽 | 排除 |

## 结论

推荐采用 **Tauri 2 + TypeScript Web UI + 小而封闭的 Rust lifecycle core**。

这里的 Rust core 不应演变为通用后端。它只负责高风险、强一致性的生命周期能力：SQLite 访问、Bundle 规划与可恢复事务、symlink 变更、文件校验，以及向 UI 暴露少量类型明确的命令。普通界面状态与交互留在 TypeScript。这样既利用 Web UI 的开发效率，也把文件系统和安装状态的写权限收进一个可审计边界；这些是针对 SkillYard 约束得出的工程判断，并非 Tauri 官方对应用架构的强制要求。

Tauri 2 最匹配，但不是最低维护成本方案：团队必须同时维护 Rust、TypeScript、capability 配置和 macOS 发布链路。其关键优势在于生产 UI 由系统 WebView 加载打包资源，不需要 localhost 或捆绑 Chromium，同时 Rust 可以直接承载现有文件系统事务边界；零遥测则应由 SkillYard 自身的依赖选择、构建配置和网络访问策略保证。

若团队更看重单语言和最快开发速度，可选择 Electron。它仍能实现本地资源 UI、SQLite 和文件事务，但包体/runtime 更重；native SQLite addon 需要跟随 Electron ABI 处理 rebuild。

Wails v2 虽然稳定，但采用它需要把生命周期核心改写为 Go；v3 截至研究日期仍为 Alpha，因此排除。Python + pywebview/PySide6 在 runtime 打包、恢复机制以及线程/bridge 的长期维护上边界更宽，因此不作为 1.0 技术底座。

## 官方来源

### Tauri 2

- [Architecture](https://v2.tauri.app/concept/architecture/)
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
