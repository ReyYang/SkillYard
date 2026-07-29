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
