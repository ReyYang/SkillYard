import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  type ReactNode,
} from "react";

import type { InterfaceLanguage } from "./domain";

// 中文原文同时充当稳定 message key，迁移旧页面时不会再维护第二套 ID。
const EN_MESSAGES = {
  "正在读取本机状态…": "Reading local state…",
  "暂时无法继续": "Unable to continue",
  "SkillYard 收到了无法显示的应用状态。":
    "SkillYard received an application state it cannot display.",
  "当前 Mac 不受 SkillYard 1.0 支持":
    "This Mac is not supported by SkillYard 1.0",
  "无法读取 SkillYard 状态": "Unable to read SkillYard state",
  "正在更新所选 Bundle": "Updating selected Bundles",
  "已确认的 Bundle 会依次完成，当前操作不能取消。":
    "Confirmed Bundles will finish in order. This operation cannot be canceled.",
  "正在删除 Bundle": "Deleting Bundle",
  "正在解除 Bundle 挂载": "Unmounting Bundle",
  "正在移除项目": "Removing project",
  "正在删除 Source": "Deleting Source",
  "SkillYard 正在完成已确认的影响范围，当前操作不能取消。":
    "SkillYard is applying the confirmed impact. This operation cannot be canceled.",
  "正在保存来源关联": "Saving Source association",
  "关联或归并会作为一个完整操作完成，当前操作不能取消。":
    "The association or merge will finish as one operation and cannot be canceled.",
  "正在重新关联 Source 路径": "Relinking Source path",
  "只更新来源位置，不会替换正在使用的 Skill 内容。":
    "Only the Source location will change; active Skill content is not replaced.",
  "正在更改 Source 分支": "Changing Source branch",
  "只更新后续来源基线，不会替换正在使用的 Skill 内容。":
    "Only the future Source baseline changes; active Skill content is not replaced.",
  "正在接管 Skill": "Taking over Skill",
  "文件迁移、受管内容和 Mount 会作为一个完整操作完成。":
    "File migration, managed content, and Mounts will finish as one operation.",
  "正在更新 Bundle": "Updating Bundle",
  "正在安装 Bundle": "Installing Bundle",
  "确认后的文件系统操作不能取消，SkillYard 会自动完成或恢复。":
    "Confirmed filesystem operations cannot be canceled; SkillYard will finish or recover them.",
  "正在修改 Mount": "Changing Mount",
  "目标路径与登记状态会作为一个完整操作完成。":
    "The target path and recorded state will finish as one operation.",
  "正在批量挂载 Bundle": "Mounting Bundle",
  "所选 Mount 会作为一个完整操作完成，当前操作不能取消。":
    "Selected Mounts will finish as one operation and cannot be canceled.",
  "本地 Bundle": "Local Bundle",
  "设置": "Settings",
  "返回": "Back",
  "返回上一页": "Back",
  "界面语言": "Interface language",
  "简体中文": "Simplified Chinese",
  "English": "English",
  "语言": "Language",
  "切换后立即更新，并在下次启动时保留。":
    "Changes apply immediately and remain after restart.",
  "受管内容目录": "Managed content directory",
  "这里保存 SkillYard 管理的实际主副本，不是可以随意清理的缓存。":
    "This directory contains the actual master copies managed by SkillYard, not disposable cache.",
  "正在打开…": "Opening…",
  "打开 Central Store": "Open Central Store",
  "重置界面状态": "Reset interface state",
  "只清除偏好、窗口状态和缓存，不删除 Bundle、Skill 或 Mount。":
    "Clears preferences, window state, and cache without deleting Bundles, Skills, or Mounts.",
  "正在重置…": "Resetting…",
  "重置应用": "Reset app",
  "重置未完成": "Reset not completed",
  "无法打开 Central Store": "Unable to open Central Store",
  "Bundle 清单": "Bundle inventory",
  "本机暂未发现 Bundle": "No Bundles found on this Mac",
  "本机已有 {bundleCount} 个 Bundle · {skillCount} 个 Skill":
    "{bundleCount} Bundles · {skillCount} Skills on this Mac",
  "检查更新": "Check for updates",
  "正在检查更新…": "Checking for updates…",
  "全部更新": "Update all",
  "正在准备全部更新…": "Preparing all updates…",
  "安装 Skill": "Install Skill",
  "添加项目": "Add project",
  "正在选择项目…": "Choosing project…",
  "刷新本机": "Refresh local",
  "正在刷新本机…": "Refreshing local…",
  "已恢复上次中断的操作": "Recovered the interrupted operation",
  "已登记项目": "Registered projects",
  "移除项目会先清理其中全部 SkillYard-managed project Mount，再删除登记记录。":
    "Removing a project first clears its SkillYard-managed project Mounts, then removes the record.",
  "全部": "All",
  "由 SkillYard 管理": "Managed by SkillYard",
  "待接管": "Ready for takeover",
  "其他管理方": "Managed elsewhere",
  "搜索 Bundle 或 Skill": "Search Bundles or Skills",
  "从 GitHub、归档、直接 URL、个人编辑目录或本机已有安装开始。内容进入 SkillYard 后默认不会挂载到任何应用。":
    "Start from GitHub, an archive, a direct URL, an editable directory, or an existing local installation. Content is not mounted to any app by default after it enters SkillYard.",
  "接管已有安装": "Take over existing installation",
  "正在选择…": "Choosing…",
  "从本地文件夹安装": "Install from local folder",
  "从 ZIP / .skill 安装": "Install from ZIP / .skill",
  "从个人编辑目录安装": "Install from editable directory",
  "添加 GitHub Source": "Add GitHub Source",
  "GitHub 仓库": "GitHub repository",
  "owner/repository 或 GitHub URL": "owner/repository or GitHub URL",
  "Tracked Ref（可选）": "Tracked Ref (optional)",
  "默认使用仓库默认分支": "Use the repository default branch",
  "正在验证…": "Validating…",
  "添加 Source": "Add Source",
  "从直接 URL 安装": "Install from direct URL",
  "ZIP / .skill 直接 URL": "Direct ZIP / .skill URL",
  "正在下载…": "Downloading…",
  "准备安装": "Prepare installation",
  "搜索 skills.sh": "Search skills.sh",
  "例如 react、testing": "For example: react, testing",
  "正在搜索…": "Searching…",
  "skills.sh 搜索结果": "skills.sh search results",
  "“{query}”的搜索结果": "Search results for “{query}”",
  "Source 操作未完成": "Source operation not completed",
  "已登记 Source": "Registered Sources",
  "可添加为 GitHub Source": "Can be added as a GitHub Source",
  "当前不是受支持的 GitHub Source":
    "This is not a supported GitHub Source",
  "添加 {source} Source": "Add {source} Source",
  "{count} 次安装": "{count} installs",
  "目录已加载": "Catalog loaded",
  "上次目录已过期": "Last catalog is stale",
  "尚未加载": "Not loaded",
  "上次成功加载：{time}": "Last successful load: {time}",
  "正在重新加载…": "Reloading…",
  "重新加载来源": "Reload Source",
  "正在准备…": "Preparing…",
  "补装 Skill": "Install missing Skills",
  "安装 Bundle": "Install Bundle",
  "重新指定路径": "Relink path",
  "删除 Source {source}": "Delete Source {source}",
  "正在准备删除…": "Preparing removal…",
  "删除 Source": "Delete Source",
  "最近一次加载失败：{error}": "Last load failed: {error}",
  "收起 Skill": "Collapse Skills",
  "查看 {count} 个 Skill": "View {count} Skills",
  "当前没有发现可展示的 Skill。": "No Skills are available to display.",
  "不可安装": "Cannot install",
  "可安装": "Available to install",
  "等待重新加载": "Waiting for reload",
  "没有尚未安装的有效 Skill。": "No valid uninstalled Skills remain.",
  "已安装 · 未挂载": "Installed · Not mounted",
  "已安装 · 已挂载 {count} 处": "Installed · Mounted in {count} locations",
  "已安装 · 挂载异常 {count} 处":
    "Installed · {count} unhealthy mounts",
  "已安装 · 正常挂载 {healthy} 处 · 异常 {abnormal} 处":
    "Installed · {healthy} healthy mounts · {abnormal} unhealthy mounts",
  "正在返回…": "Going back…",
  "确认更新这个 Bundle": "Confirm Bundle update",
  "确认安装这个 Bundle": "Confirm Bundle installation",
  "确认后，SkillYard 会把来源当前的全部有效 Skill 一次性更新到这个 Bundle。更新开始后不能取消；如果应用意外退出，下次启动会自动恢复。":
    "After confirmation, SkillYard updates this Bundle with every currently valid Skill from the Source. The update cannot be canceled after it starts; if the app exits unexpectedly, it resumes automatically on the next launch.",
  "确认后只新增当前未安装的 Skill；已有 Skill 内容和 Mount 不会被覆盖。安装开始后不能取消；如果应用意外退出，下次启动会自动恢复。":
    "After confirmation, only currently uninstalled Skills are added; existing Skill content and Mounts are not overwritten. Installation cannot be canceled after it starts; if the app exits unexpectedly, it resumes automatically on the next launch.",
  "确认后，SkillYard 会采用刚刚验证的内容快照；原文件、目录或远端内容不会被移动或改写。安装开始后不能取消；如果应用意外退出，下次启动会自动恢复。":
    "After confirmation, SkillYard adopts the content snapshot that was just validated; original files, directories, and remote content are not moved or changed. Installation cannot be canceled after it starts; if the app exits unexpectedly, it resumes automatically on the next launch.",
  "确认后，SkillYard 会把所选文件夹复制到自己的 Central Store。原文件夹不会被移动或修改。安装开始后不能取消；如果应用意外退出，下次启动会自动恢复。":
    "After confirmation, SkillYard copies the selected folder into its Central Store. The original folder is not moved or changed. Installation cannot be canceled after it starts; if the app exits unexpectedly, it resumes automatically on the next launch.",
  "更新影响预览": "Update impact preview",
  "安装影响预览": "Installation impact preview",
  "原文件夹": "Original folder",
  "Bundle 中的 Skill": "Skills in Bundle",
  "无法识别的 Skill": "Unrecognized Skill",
  "新增安装": "New installation",
  "所选 Bundle 根目录": "Selected Bundle root",
  "部分 Skill 可能依赖同一 Bundle 中未选择的其他 Skill。SkillYard 1.0 不检查这种依赖。":
    "Some Skills may depend on other unselected Skills in the same Bundle. SkillYard 1.0 does not check these dependencies.",
  "至少选择一个有效 Skill 才能安装。":
    "Select at least one valid Skill to install.",
  "更新会一次性替换整个 Bundle 的当前内容；SkillYard 1.0 不保留旧版用于回滚。":
    "An update replaces the current content of the entire Bundle at once. SkillYard 1.0 does not keep an older version for rollback.",
  "现有挂载继续使用": "Existing Mounts remain active",
  "新增 Skill 保持未挂载，更新后可再选择 Agent 应用。":
    "New Skills remain unmounted; you can choose Agent apps after the update.",
  "安装后不会自动挂载": "Installation does not mount automatically",
  "稍后由你选择 Codex、Claude Code 或 GitHub Copilot。":
    "You can choose Codex, Claude Code, or GitHub Copilot later.",
  "安装提示": "Installation notices",
  "更新未完成": "Update not completed",
  "安装未完成": "Installation not completed",
  "正在安全更新…": "Updating safely…",
  "正在安全安装…": "Installing safely…",
  "确认更新": "Confirm update",
  "确认安装": "Confirm installation",
  "上游地址不可用": "Upstream URL unavailable",
  "查看上游发布页": "Open upstream release page",
  "现有挂载": "Existing Mounts",
  "全局": "Global",
  "项目 · {project}": "Project · {project}",
  "为 Bundle 补充来源": "Add a Source to Bundle",
  "为 {bundle} 选择一个已经登记的 Source，再明确每个本地 Skill 是否对应其中的成员。":
    "Choose a registered Source for {bundle}, then specify whether each local Skill corresponds to one of its members.",
  "没有可选择的 Source": "No Source available",
  "先在现有 Source 页面添加或重新加载来源，再回来补充关系。":
    "Add or reload a Source on the Source page, then return to complete the association.",
  "前往 Source 页面添加": "Go to Source page",
  "选择 Source": "Choose Source",
  "请选择": "Choose one",
  "Skill 对应关系": "Skill correspondence",
  "逐个确认对应关系": "Confirm each correspondence",
  "此 Source 已有关联 Bundle": "This Source is already linked to a Bundle",
  "可直接关联": "Can be linked directly",
  "找不到对应成员时保持“不对应”。SkillYard 不会根据名称或内容自动猜测。":
    "Keep “No correspondence” when no matching member exists. SkillYard does not guess from names or content.",
  "{skill} 的对应关系": "Correspondence for {skill}",
  "不对应": "No correspondence",
  "正在生成计划…": "Creating plan…",
  "生成关联计划": "Create association plan",
  "无法生成关联计划": "Unable to create association plan",
  "来源根目录": "Source root",
  "确认补充来源": "Confirm Source association",
  "确认归并 Bundle": "Confirm Bundle merge",
  "关联影响": "Association impact",
  "只建立来源关系": "Only add a Source association",
  "这次操作不会修改当前内容或 Mount，也不会自动采用 Source 中的其他 Skill。":
    "This operation does not change current content or Mounts, and it does not automatically adopt other Skills from the Source.",
  "归并影响": "Merge impact",
  "两个 Bundle 将归并为一个": "Two Bundles will be merged into one",
  "{retiring} 将归入 {target}，全部 Mount 最终使用下面选择的唯一内容。":
    "{retiring} will be merged into {target}. Every Mount will use the single content choice below.",
  "成员关系": "Member correspondence",
  "本地 Skill": "Local Skills",
  "对应 {path}": "Corresponds to {path}",
  "受影响 Mount": "Affected Mounts",
  "内容冲突": "Content conflicts",
  "选择唯一内容": "Choose the single content",
  "内容 {fingerprint}": "Content {fingerprint}",
  "需要先处理冲突": "Resolve conflicts first",
  "来源操作未完成": "Source operation not completed",
  "正在安全处理…": "Processing safely…",
  "确认关联": "Confirm association",
  "确认归并": "Confirm merge",
  "保留已关联 Bundle": "Keep the linked Bundle",
  "使用待归入 Bundle": "Use the Bundle being merged",
  "确认更改 Source 分支": "Confirm Source branch change",
  "同一个 GitHub 仓库只保存一个 Source。更改后，后续目录加载和安装都以新的 Tracked Ref 为准；现有 Bundle 内容和 Mount 不会改变。":
    "Only one Source is stored for a GitHub repository. After this change, future catalog loads and installations use the new Tracked Ref; existing Bundle content and Mounts remain unchanged.",
  "Tracked Ref 变更预览": "Tracked Ref change preview",
  "当前 Ref": "Current Ref",
  "新的 Ref": "New Ref",
  "已解析 Commit": "Resolved Commit",
  "无法更改 Tracked Ref": "Unable to change Tracked Ref",
  "正在确认…": "Confirming…",
  "确认更改": "Confirm change",
  "正在取消…": "Canceling…",
  "确认重新指定 Source 路径": "Confirm Source path relink",
  "SkillYard 已确认这是原来登记的同一个目录。确认只恢复后续检查和更新能力；当前受管内容、Skill 和所有 Mount 都不会改变。":
    "SkillYard confirmed that this is the same directory originally registered. Confirmation only restores future checks and updates; current managed content, Skills, and Mounts remain unchanged.",
  "Source 路径变更预览": "Source path change preview",
  "关联 Bundle": "Linked Bundle",
  "原路径": "Previous path",
  "新路径": "New path",
  "新路径中的 Skill": "Skills at the new path",
  "可识别": "Recognized",
  "需要修正": "Needs attention",
  "如果新路径中的内容已经变化，确认后请回到主界面点击“检查更新”；本次操作不会直接采用这些变化。":
    "If content at the new path has changed, return to the inventory and select “Check for updates” after confirmation. This operation does not adopt those changes.",
  "无法重新指定 Source 路径": "Unable to relink Source path",
  "确认新路径": "Confirm new path",
  "管理本机 Skill，从一次只读扫描开始":
    "Manage local Skills, starting with a read-only scan",
  "SkillYard 将读取 Codex、Claude Code 和 GitHub Copilot 已确认的本地 Skill 目录。":
    "SkillYard will read the confirmed local Skill directories for Codex, Claude Code, and GitHub Copilot.",
  "先让 SkillYard 读取已支持位置。扫描完成后，你可以决定哪些 Skill 需要接管；应用不会在启动时自行重复扫描。":
    "Let SkillYard read supported locations first. After the scan, you decide which Skills to take over; the app does not repeat the scan automatically on launch.",
  "扫描不会自动接管、移动、覆盖或删除任何 Skill。":
    "Scanning never takes over, moves, overwrites, or deletes a Skill automatically.",
  "正在扫描…": "Scanning…",
  "开始扫描": "Start scan",
  "扫描范围": "Scan scope",
  "本次读取范围": "Locations read in this scan",
  "全局 Skill 目录": "Global Skill directory",
  "项目 Skill 目录": "Project Skill directory",
  "共享只读目录": "Shared read-only directory",
  "全部数据只保存在这台 Mac。": "All data stays on this Mac.",
  "确认添加项目": "Confirm adding project",
  "SkillYard 将登记这个项目，并扫描其中受支持应用的 Skill 目录。":
    "SkillYard will register this project and scan its supported app Skill directories.",
  "无法添加项目": "Unable to add project",
  "取消": "Cancel",
  "确认": "Confirm",
  "正在添加…": "Adding…",
  "确认添加": "Confirm",
  "接管 Bundle {bundle}": "Take over Bundle {bundle}",
  "接管 Bundle": "Take over Bundle",
  "{count} 个 Skill": "{count} Skills",
  "查看 Bundle {bundle}": "View Bundle {bundle}",
  "查看成员": "View members",
  "查看分组 {group}": "View group {group}",
  "上次结果": "Previous result",
  "本地安装": "Local installation",
  "由 SkillYard 管理 · BUNDLE": "MANAGED BY SKILLYARD · BUNDLE",
  "待接管 · BUNDLE": "READY FOR TAKEOVER · BUNDLE",
  "Agent 应用管理 · 只读": "MANAGED BY AGENT APP · READ ONLY",
  "项目仓库管理 · 只读": "MANAGED BY PROJECT REPOSITORY · READ ONLY",
  "选择要接管的 Bundle：{bundle}": "Choose Bundle to take over: {bundle}",
  "选择要接管的 {skill}": "Choose {skill} to take over",
  "确定性来源证据已经把这些 Skill 识别为同一个 Bundle。接管前只生成整组影响预览。":
    "Deterministic source evidence identifies these Skills as one Bundle. SkillYard only creates a complete impact preview before takeover.",
  "接管前只生成影响预览。勾选其他同名位置，表示你确认它们是同一个 Skill；同名本身不会触发自动合并。":
    "SkillYard only creates an impact preview before takeover. Selecting other same-named locations confirms they are the same Skill; matching names alone never trigger an automatic merge.",
  "无法生成接管预览": "Unable to create takeover preview",
  "Bundle 成员": "Bundle members",
  "确认同一个 Skill": "Confirm the same Skill",
  "将一起接管的 Skill": "Skills taken over together",
  "确认属于同一个 Skill 的位置":
    "Confirm locations that contain the same Skill",
  "接管 Bundle 成员：{skill}": "Take over Bundle member: {skill}",
  "{count} 个已确认安装位置": "{count} confirmed installation locations",
  "Skill metadata 无效，本次不会接管":
    "Skill metadata is invalid and will not be taken over",
  "确认同一 Skill：{path}": "Confirm same Skill: {path}",
  "以下同名位置没有安装组证据，只有你明确确认后才会并入对应 Member。":
    "The same-named locations below have no installation-group evidence. They are merged into the corresponding Member only after explicit confirmation.",
  "选择 {skill} 的唯一内容": "Choose the single content for {skill}",
  "请选择 {skill} 的唯一一份内容": "Choose one content copy for {skill}",
  "请选择唯一一份内容": "Choose one content copy",
  "该成员的其他位置会统一使用这份内容，不会保留为可选旧版本。":
    "Every other location for this member will use this content; no selectable older copy is kept.",
  "使用 {path} 作为主副本": "Use {path} as the master copy",
  "保留现有使用位置": "Keep existing usage locations",
  "保留哪些现有使用位置": "Choose existing usage locations to keep",
  "取消后，该原位置会在接管成功时移除，不会建立 Mount。":
    "If unselected, the original location is removed after takeover and no Mount is created.",
  "保留使用位置：{path}": "Keep usage location: {path}",
  "共享目录目标 {path}": "Shared directory target {path}",
  "选择共享目录对应的应用": "Choose the app for this shared directory",
  "原共享入口会在全部应用专属 Mount 验证成功后移除；未选择的应用可能不再发现此 Skill。":
    "The original shared entry is removed after every app-specific Mount is verified. Apps that are not selected may no longer discover this Skill.",
  "将 {path} 挂载到 {app}": "Mount {path} to {app}",
  "SkillYard 将使用该应用的固定专属 Skill 目录":
    "SkillYard will use the app's dedicated Skill directory",
  "有 {count} 个无效成员未加入计划；其他有效成员仍可接管。":
    "{count} invalid members are excluded from the plan; other valid members can still be taken over.",
  "所选位置包含无效 Skill metadata，刷新或修复后才能接管。":
    "The selected locations contain invalid Skill metadata. Refresh or fix them before takeover.",
  "共享目录必须选择至少一个应用。":
    "Select at least one app for every shared directory.",
  "下一步由 Rust 重新检查路径并封存影响预览，此时仍不会修改文件。":
    "Next, Rust rechecks the paths and seals the impact preview. No files are changed yet.",
  "正在检查现有安装…": "Checking existing installation…",
  "生成影响预览": "Create impact preview",
  "Bundle 共享目录目标": "Bundle shared-directory targets",
  "一次选择 Bundle 的 Supported App":
    "Choose Supported Apps for the Bundle",
  "上方批量选择会应用到全部兼容成员，也可以在下方逐个调整。":
    "The selection above applies to all compatible members and can be adjusted for each member below.",
  "将 Bundle 中的共享目录挂载到 {app}":
    "Mount shared directories in the Bundle to {app}",
  "应用到 {count} 个兼容成员": "Applies to {count} compatible members",
  "将 {skill} 挂载到 {app}": "Mount {skill} to {app}",
  "{app} 同时保留了 global 和 project，请只保留一种 scope。":
    "{app} has both global and project locations selected. Keep only one scope.",
  "{app} 原本同时存在 global 和 project，必须保留其中一种 scope。":
    "{app} already exists in both global and project scopes. Keep one scope.",
  "共享目录 · {project}": "Shared directory · {project}",
  "共享目录": "Shared directory",
  "{app} · {project}": "{app} · {project}",
  "{app} · 全局": "{app} · Global",
  "确认接管 Bundle：{bundle}": "Confirm Bundle takeover: {bundle}",
  "接管影响预览": "Takeover impact preview",
  "下面是 Rust 根据当前文件状态封存的完整影响。确认开始后不能取消；如果应用意外退出，下次启动会继续恢复到一致状态。":
    "This is the complete impact sealed by Rust from the current filesystem state. Takeover cannot be canceled after confirmation; if the app exits unexpectedly, the next launch continues recovery to a consistent state.",
  "更新来源": "Update Source",
  "没有更新来源": "No update Source",
  "Bundle 成员预览": "Bundle member preview",
  "保留现有 Skill": "Keep existing Skill",
  "Installation Chain：{chain}": "Installation Chain: {chain}",
  "继续使用：{path}": "Continue using: {path}",
  "采用内容：{path}": "Adopt content: {path}",
  "受管目标：{path}": "Managed target: {path}",
  "原有位置处理": "Existing location handling",
  "替换为 Mount": "Replace with Mount",
  "移除原位置": "Remove original location",
  "最终挂载位置": "Final Mount locations",
  "保留 Mount": "Keep Mount",
  "创建 Mount": "Create Mount",
  "GitHub Copilot 也可能读取这个 Claude Code 项目目录。":
    "GitHub Copilot may also read this Claude Code project directory.",
  "接管后保持已安装、未挂载；所有原使用位置都会移除。":
    "After takeover, the Bundle remains installed and unmounted; every original usage location is removed.",
  "临时恢复内容不是版本历史":
    "Temporary recovery content is not version history",
  "SkillYard 只在事务期间保留恢复所需内容；验证成功后会清理，未选副本不会成为回滚版本。":
    "SkillYard keeps recovery content only during the transaction. It is removed after successful verification, and unselected copies do not become rollback versions.",
  "接管提示": "Takeover notices",
  "确认开始后不能取消，也不会接受部分接管结果。":
    "Takeover cannot be canceled after confirmation, and partial results are never accepted.",
  "正在安全接管…": "Taking over safely…",
  "确认接管": "Confirm takeover",
  "未发现可核验的安装记录": "No verifiable installation record found",
} as const;

export type TranslationKey = keyof typeof EN_MESSAGES;
type TranslationValues = Record<string, string | number>;

interface I18nValue {
  language: InterfaceLanguage;
  t(key: TranslationKey, values?: TranslationValues): string;
}

const I18nContext = createContext<I18nValue>({
  language: "zhCn",
  t: (key, values) => interpolate(key, values),
});

export function I18nProvider({
  language,
  children,
}: {
  language: InterfaceLanguage;
  children: ReactNode;
}) {
  useEffect(() => {
    document.documentElement.lang = language === "zhCn" ? "zh-CN" : "en";
  }, [language]);

  const value = useMemo<I18nValue>(
    () => ({
      language,
      t: (key, values) =>
        interpolate(language === "zhCn" ? key : EN_MESSAGES[key], values),
    }),
    [language],
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nValue {
  return useContext(I18nContext);
}

function interpolate(
  template: string,
  values: TranslationValues | undefined,
): string {
  if (!values) return template;
  return template.replace(/\{([a-zA-Z0-9_]+)\}/g, (_, key: string) =>
    String(values[key] ?? `{${key}}`),
  );
}
