import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  DotsThreeIcon,
  FunnelIcon,
  GearIcon,
  LinkSimpleIcon,
  PlugsIcon,
  PlugsConnectedIcon,
} from "@phosphor-icons/react";

import type {
  AiConfigurationInput,
  AiPreferences,
  BundleUpdateAction,
  BundleUpdateSummary,
  BundleUpdateStatus,
  InventoryObservation,
  InterfaceLanguage,
  MountSummary,
  SkillCategory,
  SupportedAppId,
  ThemePreset,
  UiOutcome,
} from "../domain";
import { useI18n, type TranslationKey } from "../i18n";
import {
  BundleLibrary,
  type BundleLibraryItem,
} from "./library/BundleLibrary";
import { PageBackButton } from "./PageBackButton";

type InventoryOutcome = Extract<UiOutcome, { type: "inventory" }>;
export type ManagementFilter = "all" | "managed" | "takeover" | "other";
type CategoryFilter = "all" | SkillCategory;
type BundleSortMode = "management" | "nameAsc";

const AI_MODELS: Record<AiPreferences["provider"], readonly string[]> = {
  openAi: [
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.4-mini",
    "gpt-5.5",
  ],
  glm: [
    "glm-5.2",
    "glm-5.1",
    "glm-4.7",
    "glm-4.7-flashx",
    "glm-4.7-flash",
  ],
  deepSeek: ["deepseek-v4-flash", "deepseek-v4-pro"],
};

const DEFAULT_AI_MODELS: Record<AiPreferences["provider"], string> = {
  openAi: "gpt-5.6-terra",
  glm: "glm-4.7",
  deepSeek: "deepseek-v4-flash",
};

interface InventoryPageProps {
  outcome: InventoryOutcome;
  screen: InventoryScreen;
  onScreenChange(screen: InventoryScreen): void;
  pendingManagementFilter?: ManagementFilter | null;
  onPendingManagementFilterHandled?(): void;
  language: InterfaceLanguage;
  theme: ThemePreset;
  aiPreferences: AiPreferences;
  isSavingLanguage: boolean;
  languageError: string | null;
  isSavingTheme: boolean;
  themeError: string | null;
  aiOperation:
    | "savingConfiguration"
    | "savingKey"
    | "deletingKey"
    | "testing"
    | null;
  aiError: string | null;
  isWriteBlocked: boolean;
  allowReadOnlyDetails: boolean;
  isRefreshing: boolean;
  isCheckingUpdates: boolean;
  preparingBundleUpdateId: string | null;
  checkingEditableBundleId: string | null;
  isPreparingBundleUpdateBatch: boolean;
  removingBundleId: string | null;
  unmountingBundleId: string | null;
  removingProjectId: string | null;
  isOpeningDiscover: boolean;
  isOpeningInstaller: boolean;
  isAddingProject: boolean;
  isOpeningCentralStore: boolean;
  isResettingApplication: boolean;
  refreshError: string | null;
  updateError: string | null;
  discoverError: string | null;
  installError: string | null;
  projectError: string | null;
  removalError: string | null;
  mountError: string | null;
  takeoverError: string | null;
  sourceAssociationError: string | null;
  centralStoreError: string | null;
  resetError: string | null;
  generatingSkillExplanationId: string | null;
  skillExplanationError: string | null;
  isAiOrganizationRunning: boolean;
  aiOrganizationFeedback: string | null;
  aiOrganizationError: string | null;
  onRefresh(): void;
  onCheckUpdates(): void;
  onDismissUpdateError(): void;
  onUpdateBundle(bundleId: string): void;
  onChooseBundleReplacement(bundleId: string): void;
  onCheckEditableLocalBundle(bundleId: string): void;
  onUpdateAll(): void;
  onRemoveBundle(bundleId: string): void;
  onUnmountBundle(bundleId: string): void;
  onRemoveProject(projectId: string): void;
  onDiscover(): void;
  onInstall(): void;
  onAddProject(): void;
  onOpenCentralStore(): void;
  onResetApplication(): void;
  onLanguageChange(language: InterfaceLanguage): void;
  onThemeChange(theme: ThemePreset): void;
  onAiConfigurationChange(configuration: AiConfigurationInput): Promise<void>;
  onSaveAiApiKey(apiKey: string): Promise<void>;
  onDeleteAiApiKey(): Promise<void>;
  onTestAiConnection(): Promise<void>;
  onAssociateSource(bundleId: string): void;
  onOpenRecovery(issueId: string): void;
  onTakeover(observationId: string): void;
  onManageMount(memberId: string): void;
  onBatchMount(bundleId: string): void;
  onGenerateSkillExplanation(inventoryId: string): void;
  onOrganizeSkillExplanations(): void;
}

const FILTERS: Array<{ id: ManagementFilter; label: TranslationKey }> = [
  { id: "all", label: "全部" },
  { id: "managed", label: "由 SkillYard 管理" },
  { id: "takeover", label: "待接管" },
  { id: "other", label: "其他管理方" },
];

const BUNDLE_SORT_MODES: Array<{
  id: BundleSortMode;
  label: TranslationKey;
}> = [
  { id: "management", label: "管理状态优先" },
  { id: "nameAsc", label: "名称 A–Z" },
];

// 固定顺序属于 SkillYard 自己的 Taxonomy；界面只取当前清单实际出现的项。
const SKILL_CATEGORIES: Array<{
  id: SkillCategory;
  label: TranslationKey;
}> = [
  { id: "developmentEngineering", label: "开发与工程" },
  { id: "systemOperations", label: "系统与运维" },
  { id: "productivityAutomation", label: "效率与自动化" },
  { id: "dataAnalytics", label: "数据与分析" },
  { id: "productBusiness", label: "产品与业务" },
  { id: "researchLearning", label: "研究与学习" },
  { id: "writingCommunication", label: "写作与沟通" },
  { id: "designCreative", label: "设计与创意" },
  { id: "securityCompliance", label: "安全与合规" },
  { id: "other", label: "其他" },
];

type InventoryGroupKind =
  | "managedBundle"
  | "takeoverBundle"
  | "agentManaged"
  | "projectManaged";

interface InventoryGroupView {
  id: string;
  title: string;
  kind: InventoryGroupKind;
  entries: InventoryObservation[];
  bundleId: string | null;
  hasSource: boolean;
}

export type InventoryScreen =
  | { type: "list" }
  | { type: "settings" }
  | { type: "group"; groupId: string }
  | { type: "skill"; groupId: string; entryId: string };

export function InventoryPage({
  outcome,
  screen,
  onScreenChange,
  pendingManagementFilter = null,
  onPendingManagementFilterHandled,
  language,
  theme,
  aiPreferences,
  isSavingLanguage,
  languageError,
  isSavingTheme,
  themeError,
  aiOperation,
  aiError,
  isWriteBlocked,
  allowReadOnlyDetails,
  isRefreshing,
  isCheckingUpdates,
  preparingBundleUpdateId,
  checkingEditableBundleId,
  isPreparingBundleUpdateBatch,
  removingBundleId,
  unmountingBundleId,
  removingProjectId,
  isOpeningDiscover,
  isOpeningInstaller,
  isAddingProject,
  isOpeningCentralStore,
  isResettingApplication,
  refreshError,
  updateError,
  discoverError,
  installError,
  projectError,
  removalError,
  mountError,
  takeoverError,
  sourceAssociationError,
  centralStoreError,
  resetError,
  generatingSkillExplanationId,
  skillExplanationError,
  isAiOrganizationRunning,
  aiOrganizationFeedback,
  aiOrganizationError,
  onRefresh,
  onCheckUpdates,
  onDismissUpdateError,
  onUpdateBundle,
  onChooseBundleReplacement,
  onCheckEditableLocalBundle,
  onUpdateAll,
  onRemoveBundle,
  onUnmountBundle,
  onRemoveProject,
  onDiscover,
  onInstall,
  onAddProject,
  onOpenCentralStore,
  onResetApplication,
  onLanguageChange,
  onThemeChange,
  onAiConfigurationChange,
  onSaveAiApiKey,
  onDeleteAiApiKey,
  onTestAiConnection,
  onAssociateSource,
  onOpenRecovery,
  onTakeover,
  onManageMount,
  onBatchMount,
  onGenerateSkillExplanation,
  onOrganizeSkillExplanations,
}: InventoryPageProps) {
  const { localize, t } = useI18n();
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<ManagementFilter>("all");
  const [categoryFilter, setCategoryFilter] =
    useState<CategoryFilter>("all");
  const [sortMode, setSortMode] =
    useState<BundleSortMode>("management");
  const projectMenuRef = useRef<HTMLDetailsElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const [openProjectMenuAfterReturn, setOpenProjectMenuAfterReturn] =
    useState(false);
  const [focusSearchAfterReturn, setFocusSearchAfterReturn] = useState(false);
  // 选择状态属于 InventoryPage；renderer 切换时只改变构图，不重建领域状态。
  const [selectedLibraryGroupId, setSelectedLibraryGroupId] = useState<
    string | null
  >(null);

  const groups = useMemo(
    () => groupInventoryEntries(outcome.entries, t, language),
    [language, outcome.entries, t],
  );
  const availableCategories = useMemo(() => {
    const present = new Set(
      outcome.entries.flatMap((entry) =>
        entry.aiExplanation ? [entry.aiExplanation.category] : [],
      ),
    );
    return SKILL_CATEGORIES.filter((category) => present.has(category.id));
  }, [outcome.entries]);
  useEffect(() => {
    if (
      categoryFilter !== "all" &&
      !availableCategories.some((category) => category.id === categoryFilter)
    ) {
      setCategoryFilter("all");
    }
  }, [availableCategories, categoryFilter]);
  useLayoutEffect(() => {
    if (screen.type !== "list" || !openProjectMenuAfterReturn) return;
    const projectMenu = projectMenuRef.current;
    if (!projectMenu) return;
    projectMenu.open = true;
    projectMenu.querySelector<HTMLElement>("summary")?.focus();
    setOpenProjectMenuAfterReturn(false);
  }, [openProjectMenuAfterReturn, screen.type]);
  useLayoutEffect(() => {
    if (screen.type !== "list" || !focusSearchAfterReturn) return;
    searchInputRef.current?.focus();
    setFocusSearchAfterReturn(false);
  }, [focusSearchAfterReturn, screen.type]);
  useLayoutEffect(() => {
    if (screen.type !== "list" || pendingManagementFilter === null) return;
    setFilter(pendingManagementFilter);
    onPendingManagementFilterHandled?.();
  }, [
    onPendingManagementFilterHandled,
    pendingManagementFilter,
    screen.type,
  ]);
  // 搜索命中成员时只显示其所属分组，主清单仍不展开 Skill。
  const visibleGroups = useMemo(() => {
    const normalizedQuery = query
      .trim()
      .toLocaleLowerCase(language === "zhCn" ? "zh-CN" : "en");
    const filteredGroups = groups.filter((group) =>
      group.entries.some(
        (entry) =>
          matchesFilter(entry, filter) &&
          matchesQuery(entry, normalizedQuery) &&
          matchesCategory(entry, categoryFilter),
      ),
    );
    if (sortMode === "management") return filteredGroups;
    const locale = language === "zhCn" ? "zh-CN" : "en";
    return filteredGroups.slice().sort(
      (left, right) =>
        left.title.localeCompare(right.title, locale) ||
        left.id.localeCompare(right.id),
    );
  }, [categoryFilter, filter, groups, language, query, sortMode]);
  const managementFilterLabel =
    filter === "all"
      ? null
      : t(FILTERS.find((item) => item.id === filter)!.label);
  const categoryFilterLabel =
    categoryFilter === "all"
      ? null
      : t(
          SKILL_CATEGORIES.find((category) => category.id === categoryFilter)!
            .label,
        );
  const filterSummaryLabel =
    managementFilterLabel && categoryFilterLabel
      ? `${managementFilterLabel} · ${categoryFilterLabel}`
      : (managementFilterLabel ?? categoryFilterLabel ?? t("全部 Bundle"));
  const sortModeLabel = t(
    BUNDLE_SORT_MODES.find((item) => item.id === sortMode)!.label,
  );
  const hasActiveFilter = filter !== "all" || categoryFilter !== "all";
  const hasCustomizedLibraryView =
    hasActiveFilter || sortMode !== "management";
  const libraryControlsVisibleLabel =
    sortMode === "management"
      ? filterSummaryLabel
      : `${filterSummaryLabel} · ${sortModeLabel}`;
  const libraryControlsCompactLabel =
    sortMode === "nameAsc" ? "A–Z" : language === "zhCn" ? "筛" : "F";
  const hasActiveSearchOrFilter = query.trim().length > 0 || hasActiveFilter;
  const changeSortMode = (nextSortMode: BundleSortMode) => {
    // 排序时固化当前可见 Bundle；空结果不能清掉暂时被筛选隐藏的稳定选择。
    setSelectedLibraryGroupId((current) =>
      current && visibleGroups.some((group) => group.id === current)
        ? current
        : (visibleGroups[0]?.id ?? current),
    );
    setSortMode(nextSortMode);
  };
  const libraryItems = useMemo<BundleLibraryItem[]>(
    () =>
      visibleGroups.map((group) => {
        const groupMounts = outcome.mounts.filter((mount) =>
          group.entries.some((entry) => entry.memberId === mount.memberId),
        );
        const bundleUpdate =
          group.bundleId !== null &&
          group.kind === "managedBundle"
            ? outcome.bundleUpdates.find(
                (update) => update.bundleId === group.bundleId,
              ) ?? null
            : null;
        const abnormalMountCount = groupMounts.filter(
          (mount) => mount.health !== "healthy",
        ).length;
        const mountStatus =
          abnormalMountCount > 0
            ? t("挂载异常 {count} 处", { count: abnormalMountCount })
            : groupMounts.length > 0
              ? t("已挂载")
              : t("未挂载");
        const updateStatus = bundleUpdate
          ? bundleUpdateStatusLabel(bundleUpdate.status, t)
          : null;
        const status =
          group.kind === "takeoverBundle"
            ? t("待接管")
            : group.kind === "agentManaged" ||
                group.kind === "projectManaged"
              ? t("只读")
              : [mountStatus, updateStatus].filter(Boolean).join(" · ");
        const hasWarning =
          abnormalMountCount > 0 ||
          bundleUpdate?.status === "available" ||
          bundleUpdate?.status === "unableToCheck" ||
          bundleUpdate?.status === "sourceUnavailable";
        return {
          id: group.id,
          title: group.title,
          eyebrow: t(groupEyebrow(group.kind)),
          skillCount: group.entries.length,
          status,
          statusTone: hasWarning
            ? "warning"
            : group.kind === "managedBundle" ||
                group.kind === "takeoverBundle"
              ? "accent"
              : "muted",
        };
      }),
    [outcome.bundleUpdates, outcome.mounts, t, visibleGroups],
  );
  // 主清单中的每张分组卡都是用户看到的 Bundle，包括只读的插件与项目分组。
  const bundleCount = groups.length;
  const updatableBundleCount = useMemo(
    () =>
      new Set(
        outcome.bundleUpdates
          .filter((update) => update.action === "update")
          .map((update) => update.bundleId),
      ).size,
    [outcome.bundleUpdates],
  );
  const hasPendingAiExplanation = outcome.entries.some((entry) =>
    isSkillExplanationPending(entry, language),
  );
  const canOrganizeWithAi =
    aiPreferences.enabled &&
    aiPreferences.disclosureAccepted &&
    aiPreferences.hasApiKey &&
    aiPreferences.verified;
  const maintenanceActions = (
    <>
      <button
        className="secondary-action"
        type="button"
        disabled={
          isWriteBlocked ||
          isAiOrganizationRunning ||
          !canOrganizeWithAi ||
          !hasPendingAiExplanation
        }
        onClick={onOrganizeSkillExplanations}
      >
        {t("AI 整理")}
      </button>
      <button
        className="secondary-action"
        type="button"
        disabled={isWriteBlocked || isCheckingUpdates}
        onClick={onCheckUpdates}
      >
        {isCheckingUpdates ? t("正在检查更新…") : t("检查更新")}
      </button>
      {updatableBundleCount >= 2 ? (
        <button
          className="secondary-action"
          type="button"
          aria-label={t("全部更新")}
          disabled={isWriteBlocked || isPreparingBundleUpdateBatch}
          onClick={onUpdateAll}
        >
          {isPreparingBundleUpdateBatch
            ? t("正在准备全部更新…")
            : t("全部更新")}
        </button>
      ) : null}
      <button
        className="secondary-action"
        type="button"
        disabled={isWriteBlocked || isRefreshing}
        onClick={onRefresh}
      >
        {isRefreshing ? t("正在刷新本机…") : t("刷新本机")}
      </button>
      <p className="refresh-summary" aria-label={t("最近刷新结果")}>
        {outcome.lastLocalRefresh
          ? t("最近刷新：新增 {added} · 变化 {changed} · 移除 {removed}", {
              added: outcome.lastLocalRefresh.added,
              changed: outcome.lastLocalRefresh.changed,
              removed: outcome.lastLocalRefresh.removed,
            })
          : t("尚未执行本机刷新")}
        {outcome.lastLocalRefresh ? (
          <span>
            {formatTimestamp(outcome.lastLocalRefresh.completedAt, language)}
          </span>
        ) : null}
      </p>
    </>
  );
  const selectedGroup =
    screen.type === "group" || screen.type === "skill"
      ? groups.find((group) => group.id === screen.groupId) ?? null
      : null;
  const selectedEntry =
    screen.type === "skill"
      ? selectedGroup?.entries.find((entry) => entry.id === screen.entryId) ??
        null
      : null;
  const selectedGroupEntries = selectedGroup
    ? selectedGroup.entries.filter((entry) =>
        matchesCategory(entry, categoryFilter),
      )
    : [];

  if (screen.type === "settings") {
    return (
      <main
        className={`inventory-library-shell inventory-library-shell--${theme} inventory-settings-shell`}
      >
        <header
          className="library-topbar"
          data-library-chrome={theme}
          data-tauri-drag-region
        >
          <span className="library-brand" aria-label="SkillYard">
            <span className="library-brand-mark" aria-hidden="true" />
          </span>
          <nav className="library-primary-navigation" aria-label={t("主要导航")}>
            <button
              className="library-navigation-action"
              type="button"
              onClick={() => onScreenChange({ type: "list" })}
            >
              {t("技能库")}
            </button>
            <button
              className="library-navigation-action"
              type="button"
              disabled={isOpeningDiscover}
              onClick={onDiscover}
            >
              {isOpeningDiscover ? t("正在打开…") : t("发现")}
            </button>
            <button
              className="library-navigation-action"
              type="button"
              onClick={() => {
                setOpenProjectMenuAfterReturn(true);
                onScreenChange({ type: "list" });
              }}
            >
              {t("项目")}
            </button>
          </nav>
          <button
            className="library-search-field library-search-return"
            type="button"
            aria-label={t("返回技能库并搜索 Bundle 或 Skill")}
            onClick={() => {
              setFocusSearchAfterReturn(true);
              onScreenChange({ type: "list" });
            }}
          >
            {t("搜索 Bundle 或 Skill")}
          </button>
          <button
            className="library-install-action"
            type="button"
            aria-label={t("添加 Bundle")}
            disabled={isWriteBlocked || isOpeningInstaller}
            onClick={onInstall}
          >
            {isOpeningInstaller ? t("正在打开…") : t("添加 Bundle")}
          </button>
          <span className="library-settings-profile" aria-hidden="true">
            <span className="library-profile-mark" />
          </span>
        </header>
        <InventorySettingsPage
          language={language}
          theme={theme}
          aiPreferences={aiPreferences}
          isSavingLanguage={isSavingLanguage}
          languageError={languageError}
          isSavingTheme={isSavingTheme}
          themeError={themeError}
          aiOperation={aiOperation}
          aiError={aiError}
          isWriteBlocked={isWriteBlocked}
          isRefreshing={isRefreshing}
          isCheckingUpdates={isCheckingUpdates}
          isOpeningCentralStore={isOpeningCentralStore}
          isResettingApplication={isResettingApplication}
          centralStoreError={centralStoreError}
          resetError={resetError}
          onRefresh={onRefresh}
          onCheckUpdates={onCheckUpdates}
          onOpenCentralStore={onOpenCentralStore}
          onResetApplication={onResetApplication}
          onLanguageChange={onLanguageChange}
          onThemeChange={onThemeChange}
          onAiConfigurationChange={onAiConfigurationChange}
          onSaveAiApiKey={onSaveAiApiKey}
          onDeleteAiApiKey={onDeleteAiApiKey}
          onTestAiConnection={onTestAiConnection}
        />
      </main>
    );
  }

  if (screen.type === "skill" && selectedGroup && selectedEntry) {
    return (
      <SkillDetailsPage
        group={selectedGroup}
        entry={selectedEntry}
        mounts={outcome.mounts.filter(
          (mount) => mount.memberId === selectedEntry.memberId,
        )}
        actionsDisabled={isWriteBlocked}
        allowReadOnlyDetails={allowReadOnlyDetails}
        mountError={mountError}
        canGenerateExplanation={
          !allowReadOnlyDetails &&
          aiPreferences.enabled &&
          aiPreferences.disclosureAccepted &&
          aiPreferences.hasApiKey &&
          aiPreferences.verified
        }
        isGeneratingExplanation={
          generatingSkillExplanationId === selectedEntry.id
        }
        explanationError={skillExplanationError}
        explanationStale={isSkillExplanationPending(selectedEntry, language)}
        onGenerateExplanation={() =>
          onGenerateSkillExplanation(selectedEntry.id)
        }
        onManageMount={onManageMount}
        onBack={() =>
          onScreenChange(
            selectedGroup.entries.length === 1
              ? { type: "list" }
              : { type: "group", groupId: selectedGroup.id },
          )
        }
      />
    );
  }

  if (screen.type === "group" && selectedGroup) {
    return (
      <InventoryGroupDetails
        group={selectedGroup}
        entries={selectedGroupEntries}
        categoryFilter={categoryFilter}
        language={language}
        mounts={outcome.mounts}
        onOpenSkill={(entryId) =>
          onScreenChange({
            type: "skill",
            groupId: selectedGroup.id,
            entryId,
          })
        }
        onBack={() => onScreenChange({ type: "list" })}
      />
    );
  }

  return (
    <main
      className={`inventory-library-shell inventory-library-shell--${theme}`}
    >
      <h1 className="sr-only">{t("Bundle 清单")}</h1>
      <header
        className="library-topbar"
        data-library-chrome={theme}
        data-tauri-drag-region
      >
        <span className="library-brand" aria-label="SkillYard">
          <span className="library-brand-mark" aria-hidden="true" />
        </span>
        <nav className="library-primary-navigation" aria-label={t("主要导航")}>
          <span aria-current="page">{t("技能库")}</span>
          <button
            className="library-navigation-action"
            type="button"
            disabled={isOpeningDiscover}
            onClick={onDiscover}
          >
            {isOpeningDiscover ? t("正在打开…") : t("发现")}
          </button>
          <details ref={projectMenuRef} className="library-project-menu">
            <summary>{t("项目")}</summary>
            <div className="library-popover">
              {outcome.projects.length > 0 ? (
                <section
                  className="registered-projects"
                  aria-label={t("已登记项目")}
                >
                  <header>
                    <div>
                      <p className="section-eyebrow">REGISTERED PROJECTS</p>
                      <h2>{t("已登记项目")}</h2>
                    </div>
                    <span>{outcome.projects.length}</span>
                  </header>
                  <p>
                    {t(
                      "移除项目会先清理其中全部 SkillYard-managed project Mount，再删除登记记录。",
                    )}
                  </p>
                  <ul>
                    {outcome.projects.map((project) => (
                      <li key={project.id}>
                        <div>
                          <strong>{project.displayName}</strong>
                          <code title={project.rootPath}>
                            {project.rootPath}
                          </code>
                        </div>
                        <button
                          className="danger-outline-action"
                          type="button"
                          aria-label={t("移除项目 {project}", {
                            project: project.displayName,
                          })}
                          disabled={isWriteBlocked}
                          onClick={() => onRemoveProject(project.id)}
                        >
                          {removingProjectId === project.id
                            ? t("正在准备移除…")
                            : t("移除项目")}
                        </button>
                      </li>
                    ))}
                  </ul>
                </section>
              ) : null}
              <button
                className="secondary-action"
                type="button"
                disabled={isWriteBlocked || isAddingProject}
                onClick={onAddProject}
              >
                {isAddingProject ? t("正在选择项目…") : t("添加项目")}
              </button>
            </div>
          </details>
        </nav>
        <label className="library-search-field">
          <span className="sr-only">{t("搜索 Bundle 或 Skill")}</span>
          <input
            ref={searchInputRef}
            type="search"
            value={query}
            placeholder={t("搜索 Bundle 或 Skill")}
            aria-label={t("搜索 Bundle 或 Skill")}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <details
          className="library-filter-menu"
          data-filter-active={hasCustomizedLibraryView ? "true" : "false"}
        >
          <summary
            aria-label={`${t("筛选与排序")}：${filterSummaryLabel} · ${sortModeLabel}`}
          >
            {theme === "ledger" ? (
              <span>{libraryControlsVisibleLabel}</span>
            ) : (
              <>
                <FunnelIcon size={18} weight="regular" aria-hidden />
                {hasCustomizedLibraryView ? (
                  <>
                    <span className="library-filter-active-label">
                      {libraryControlsVisibleLabel}
                    </span>
                    <span
                      className="library-filter-compact-badge"
                      aria-hidden="true"
                    >
                      {libraryControlsCompactLabel}
                    </span>
                  </>
                ) : null}
              </>
            )}
          </summary>
          <div className="library-popover library-filter-panel">
            <label className="category-filter">
              <span>{t("分类")}</span>
              <select
                aria-label={t("分类")}
                value={categoryFilter}
                onChange={(event) =>
                  setCategoryFilter(event.target.value as CategoryFilter)
                }
              >
                <option value="all">{t("全部分类")}</option>
                {availableCategories.map((category) => (
                  <option key={category.id} value={category.id}>
                    {t(category.label)}
                  </option>
                ))}
              </select>
            </label>
            <div className="filter-group" aria-label={t("管理状态")}>
              {FILTERS.map((item) => (
                <button
                  key={item.id}
                  type="button"
                  aria-pressed={filter === item.id}
                  onClick={() => setFilter(item.id)}
                >
                  {t(item.label)}
                </button>
              ))}
            </div>
            <label className="sort-filter">
              <span>{t("排序")}</span>
              <select
                aria-label={t("排序")}
                value={sortMode}
                onChange={(event) =>
                  changeSortMode(event.target.value as BundleSortMode)
                }
              >
                {BUNDLE_SORT_MODES.map((item) => (
                  <option key={item.id} value={item.id}>
                    {t(item.label)}
                  </option>
                ))}
              </select>
            </label>
          </div>
        </details>
        <button
          className="library-install-action"
          type="button"
          aria-label={t("添加 Bundle")}
          disabled={isWriteBlocked || isOpeningInstaller}
          onClick={onInstall}
        >
          {isOpeningInstaller ? t("正在打开…") : t("添加 Bundle")}
        </button>
        <details className="library-maintenance-menu">
          <summary aria-label={t("更多操作")}>
            {theme === "ledger" ? (
              <>
                <span>{t("更多")}</span>
                <span className="library-profile-mark" aria-hidden="true" />
              </>
            ) : (
              <DotsThreeIcon size={20} weight="bold" aria-hidden />
            )}
          </summary>
          <div className="library-popover library-maintenance-actions">
            {maintenanceActions}
          </div>
        </details>
      </header>
      <button
        className="library-settings-action"
        type="button"
        onClick={() => onScreenChange({ type: "settings" })}
      >
        <GearIcon
          className="library-settings-icon"
          size={20}
          weight="regular"
          aria-hidden
        />
        <span>{t("设置")}</span>
      </button>
      <p className="sr-only">
        {outcome.entries.length === 0
          ? t("本机暂未发现 Bundle")
          : t("本机已有 {bundleCount} 个 Bundle · {skillCount} 个 Skill", {
              bundleCount,
              skillCount: outcome.entries.length,
            })}
      </p>

      <section className="library-notice-stack" aria-label={t("状态提示")}>
      {aiOrganizationFeedback ? (
        <p
          className="recovery-notice"
          role="status"
          aria-label={aiOrganizationFeedback}
        >
          {aiOrganizationFeedback}
        </p>
      ) : null}
      {aiOrganizationError ? (
        <div className="inline-error" role="alert">
          {t("AI 整理未能开始")}：{aiOrganizationError}
        </div>
      ) : null}

      {outcome.recoveredInterruptedOperation ? (
        <p className="recovery-notice" role="status">
          {t("已恢复上次中断的操作")}
        </p>
      ) : null}

      {outcome.recoveryIssues.length > 0 ? (
        <section className="recovery-warning" aria-label={t("需要人工恢复")}>
          <p className="section-eyebrow">FILESYSTEM RECOVERY</p>
          <h2>{t("需要人工恢复")}</h2>
          <p>
            {t(
              "SkillYard 无法安全判断下面操作的最终状态，因此只停止修改相关 Bundle。其他 Skill 和只读清单仍可正常使用。",
            )}
          </p>
          <ul>
            {outcome.recoveryIssues.map((issue) => (
              <li key={issue.id}>
                <strong>{issue.bundleDisplayName}</strong>
                <span>{localize(issue.message, "这个操作需要人工恢复。")}</span>
                <button
                  className="compact-action"
                  type="button"
                  aria-label={t("查看 {bundle} 的恢复说明", {
                    bundle: issue.bundleDisplayName,
                  })}
                  disabled={isWriteBlocked}
                  onClick={() => onOpenRecovery(issue.id)}
                >
                  {t("查看说明")}
                </button>
              </li>
            ))}
          </ul>
          <p>{t("请保留 Central Store 中的现有内容，不要手动删除相关目录。")}</p>
        </section>
      ) : null}

      {refreshError ? (
        <div className="inline-error" role="alert">
          <strong>{t("刷新未完成")}</strong>
          <span>{refreshError}</span>
        </div>
      ) : null}

      {updateError ? (
        <div className="inline-error" role="alert">
          <strong>{t("更新未完成")}</strong>
          <span>{updateError}</span>
          <button
            className="inline-error-dismiss"
            type="button"
            aria-label={t("关闭更新提示")}
            onClick={onDismissUpdateError}
          >
            {t("关闭")}
          </button>
        </div>
      ) : null}

      {installError ? (
        <div className="inline-error" role="alert">
          <strong>{t("无法准备安装")}</strong>
          <span>{installError}</span>
        </div>
      ) : null}

      {discoverError ? (
        <div className="inline-error" role="alert">
          <strong>{t("无法打开发现页")}</strong>
          <span>{discoverError}</span>
        </div>
      ) : null}

      {projectError ? (
        <div className="inline-error" role="alert">
          <strong>{t("无法添加项目")}</strong>
          <span>{projectError}</span>
        </div>
      ) : null}

      {removalError ? (
        <div className="inline-error" role="alert">
          <strong>{t("移除操作未完成")}</strong>
          <span>{removalError}</span>
        </div>
      ) : null}

      {mountError ? (
        <div className="inline-error" role="alert">
          <strong>{t("挂载操作未完成")}</strong>
          <span>{mountError}</span>
        </div>
      ) : null}

      {takeoverError ? (
        <div className="inline-error" role="alert">
          <strong>{t("接管未完成")}</strong>
          <span>{takeoverError}</span>
        </div>
      ) : null}

      {sourceAssociationError ? (
        <div className="inline-error" role="alert">
          <strong>{t("来源操作未完成")}</strong>
          <span>{sourceAssociationError}</span>
        </div>
      ) : null}

      {outcome.scanIssues.length > 0 ? (
        <section className="scan-warning" aria-label={t("扫描告警")}>
          <strong>{t("部分 Skill 或目录暂时无法读取")}</strong>
          <p>
            {t(
              "SkillYard 已继续扫描其他内容，并保留已有记录；不会自动修改这些路径。",
            )}
          </p>
          <ul>
            {outcome.scanIssues.map((issue) => (
              <li key={issue.id}>
                <code>{issue.path}</code>
                <span>{localize(issue.message, "无法读取这个路径。")}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      </section>

      <section className="library-workspace">
        <BundleLibrary
          theme={theme}
          items={libraryItems}
          selectedId={selectedLibraryGroupId}
          onSelect={setSelectedLibraryGroupId}
          renderDetails={(groupId) => {
            const group =
              visibleGroups.find((candidate) => candidate.id === groupId) ??
              null;
            if (!group) return null;
            const groupMounts = outcome.mounts.filter((mount) =>
              group.entries.some((entry) => entry.memberId === mount.memberId),
            );
            const libraryItem = libraryItems.find(
              (item) => item.id === group.id,
            );
            if (!libraryItem) return null;
            return (
              <InventorySection
                theme={theme}
                groupKind={group.kind}
                title={group.title}
                eyebrow={t(groupEyebrow(group.kind))}
                status={libraryItem.status}
                statusTone={libraryItem.statusTone}
                entries={group.entries}
                language={language}
                actionsDisabled={isWriteBlocked}
                batchMountBundleId={group.bundleId}
                onBatchMount={onBatchMount}
                canAssociateSource={
                  group.kind === "managedBundle" && !group.hasSource
                }
                onAssociateSource={onAssociateSource}
                bundleUpdate={
                  group.kind === "managedBundle" && group.bundleId
                    ? outcome.bundleUpdates.find(
                        (update) => update.bundleId === group.bundleId,
                      ) ?? null
                    : null
                }
                preparingBundleUpdateId={preparingBundleUpdateId}
                checkingEditableBundleId={checkingEditableBundleId}
                onUpdateBundle={onUpdateBundle}
                onChooseBundleReplacement={onChooseBundleReplacement}
                onCheckEditableLocalBundle={onCheckEditableLocalBundle}
                removingBundleId={
                  group.kind === "managedBundle" ? removingBundleId : null
                }
                onRemoveBundle={onRemoveBundle}
                mounts={group.kind === "managedBundle" ? groupMounts : []}
                unmountingBundleId={
                  group.kind === "managedBundle" ? unmountingBundleId : null
                }
                onUnmountBundle={onUnmountBundle}
                onTakeover={
                  group.kind === "takeoverBundle" ? onTakeover : undefined
                }
                onOpen={() => {
                  const singleEntry =
                    group.entries.length === 1 ? group.entries[0] : null;
                  onScreenChange(
                    singleEntry
                      ? {
                          type: "skill",
                          groupId: group.id,
                          entryId: singleEntry.id,
                        }
                      : { type: "group", groupId: group.id },
                  );
                }}
                openLabel={
                  group.entries.length === 1
                    ? t("查看 Skill {skill}", {
                        skill: group.entries[0]!.skillName,
                      })
                    : group.kind === "managedBundle" ||
                        group.kind === "takeoverBundle"
                      ? t("查看 Bundle {bundle}", { bundle: group.title })
                      : t("查看分组 {group}", { group: group.title })
                }
                openText={
                  group.entries.length === 1
                    ? t("查看 Skill")
                    : group.kind === "managedBundle" ||
                        group.kind === "takeoverBundle"
                      ? t("查看 Bundle")
                      : t("查看分组")
                }
              />
            );
          }}
          emptyState={
            <section className="empty-inventory">
              <h2>
                {outcome.entries.length === 0
                  ? t("未发现 Skill")
                  : t("没有匹配结果")}
              </h2>
              <p>
                {outcome.entries.length === 0
                  ? t("你可以继续使用现有安装方式，再主动刷新本机。")
                  : t("换一个关键词或管理状态看看。")}
              </p>
              {outcome.entries.length > 0 && hasActiveSearchOrFilter ? (
                <button
                  className="secondary-action"
                  type="button"
                  onClick={() => {
                    setQuery("");
                    setFilter("all");
                    setCategoryFilter("all");
                  }}
                >
                  {t("清除筛选")}
                </button>
              ) : null}
            </section>
          }
        />
      </section>
    </main>
  );
}

function InventorySection({
  theme,
  groupKind,
  title,
  eyebrow,
  status,
  statusTone,
  entries,
  language,
  actionsDisabled = false,
  batchMountBundleId,
  onBatchMount,
  canAssociateSource = false,
  onAssociateSource,
  bundleUpdate = null,
  preparingBundleUpdateId = null,
  checkingEditableBundleId = null,
  onUpdateBundle,
  onChooseBundleReplacement,
  onCheckEditableLocalBundle,
  removingBundleId = null,
  onRemoveBundle,
  mounts = [],
  unmountingBundleId = null,
  onUnmountBundle,
  onTakeover,
  onOpen,
  openLabel,
  openText,
}: {
  theme: ThemePreset;
  groupKind: InventoryGroupKind;
  title: string;
  eyebrow: string;
  status: string;
  statusTone: BundleLibraryItem["statusTone"];
  entries: InventoryObservation[];
  language: InterfaceLanguage;
  actionsDisabled?: boolean;
  batchMountBundleId?: string | null;
  onBatchMount?(bundleId: string): void;
  canAssociateSource?: boolean;
  onAssociateSource?(bundleId: string): void;
  bundleUpdate?: BundleUpdateSummary | null;
  preparingBundleUpdateId?: string | null;
  checkingEditableBundleId?: string | null;
  onUpdateBundle?(bundleId: string): void;
  onChooseBundleReplacement?(bundleId: string): void;
  onCheckEditableLocalBundle?(bundleId: string): void;
  removingBundleId?: string | null;
  onRemoveBundle?(bundleId: string): void;
  mounts?: MountSummary[];
  unmountingBundleId?: string | null;
  onUnmountBundle?(bundleId: string): void;
  onTakeover?(observationId: string): void;
  onOpen(): void;
  openLabel: string;
  openText: string;
}) {
  const { t } = useI18n();
  if (entries.length === 0) return null;
  const sourceNames = [
    ...new Set(
      entries.flatMap((entry) => {
        const sourceName =
          entry.sourceDisplayName ?? entry.installationChain?.source;
        return sourceName ? [sourceName] : [];
      }),
    ),
  ];
  const sourceLabel = sourceNames.join("、") || t("来源未知");
  const abnormalMountCount = mounts.filter(
    (mount) => mount.health !== "healthy",
  ).length;
  const mountLabel =
    mounts.length > 0
      ? `${t("{count} 个 Mount", { count: mounts.length })} · ${[
          ...new Set(mounts.map((mount) => supportedAppLabel(mount.appId))),
        ].join("、")}${
          abnormalMountCount > 0
            ? ` · ${t("挂载异常 {count} 处", {
                count: abnormalMountCount,
              })}`
            : ""
        }`
      : t("未挂载");
  // 总数已在标题中完整呈现；预览保留五个真实成员，不再用汇总卡挤掉第五个内容。
  const previewEntries = entries.slice(0, 5);
  const previewMountDestinations = representativeMountDestinations(mounts, 2);
  const hasLifecycleActions = Boolean(batchMountBundleId || onTakeover);
  return (
    <section className="inventory-section" aria-label={title}>
      <header>
        <div>
          <p className="section-eyebrow">{eyebrow}</p>
          <h2 title={title}>{title}</h2>
          <div className="bundle-library-status-line">
            <span>{t("{count} 个 Skill", { count: entries.length })}</span>
            <em data-tone={statusTone}>· {status}</em>
            {entries.some((entry) => entry.stale) ? (
              <span className="stale-badge">{t("上次结果")}</span>
            ) : null}
            {bundleUpdate ? (
              <BundleUpdateStatusView
                update={bundleUpdate}
                bundleDisplayName={title}
                isPreparing={preparingBundleUpdateId === batchMountBundleId}
                isChecking={checkingEditableBundleId === batchMountBundleId}
                actionsDisabled={actionsDisabled}
                showStatus={false}
                onUpdate={
                  batchMountBundleId
                    ? () => onUpdateBundle?.(batchMountBundleId)
                    : undefined
                }
                onImportReplacement={
                  batchMountBundleId
                    ? () => onChooseBundleReplacement?.(batchMountBundleId)
                    : undefined
                }
                onCheckEditableLocal={
                  batchMountBundleId
                    ? () => onCheckEditableLocalBundle?.(batchMountBundleId)
                    : undefined
                }
              />
            ) : null}
          </div>
        </div>
      </header>
      {entries.length > 1 ? (
        <p className="bundle-library-summary">
          {t("集中管理 {count} 个 Skill，并保留各自的来源与挂载状态。", {
            count: entries.length,
          })}
        </p>
      ) : null}
      <dl className="bundle-library-metadata">
        <div>
          <dt>{t("来源")}</dt>
          <dd>
            <LinkSimpleIcon
              className="bundle-source-mark"
              size={18}
              weight="regular"
              aria-hidden
            />
            <span className="bundle-metadata-value" title={sourceLabel}>
              {sourceLabel}
            </span>
          </dd>
        </div>
        {groupKind === "managedBundle" ? (
          <div>
            <dt>{t("当前挂载")}</dt>
            <dd>
              {mounts.length > 0 ? (
                <PlugsConnectedIcon
                  className="bundle-mount-mark"
                  data-mount-state="connected"
                  size={18}
                  weight="regular"
                  aria-hidden
                />
              ) : (
                <PlugsIcon
                  className="bundle-mount-mark"
                  data-mount-state="empty"
                  size={18}
                  weight="regular"
                  aria-hidden
                />
              )}
              <span className="bundle-metadata-value" title={mountLabel}>
                {mountLabel}
              </span>
            </dd>
            {theme === "layers" && mounts.length > 0 ? (
              <ul className="bundle-mount-destinations" aria-label={t("当前挂载")}>
                {previewMountDestinations.map((mount) => (
                  <li key={mount.id}>
                    <strong>{mountLabelForDisplay(mount, t)}</strong>
                    <code title={mount.targetPath}>{mount.targetPath}</code>
                  </li>
                ))}
                {mounts.length > previewMountDestinations.length ? (
                  <li className="bundle-mount-destinations-more">
                    {t("还有 {count} 处 Mount", {
                      count: mounts.length - previewMountDestinations.length,
                    })}
                  </li>
                ) : null}
              </ul>
            ) : null}
          </div>
        ) : null}
      </dl>
      {entries.length > 1 ? (
        theme === "ledger" ? (
          <table
            className="bundle-library-member-table"
            aria-label={t("{group} 的 Skill", { group: title })}
          >
            <caption>{t("精选成员")}</caption>
            <thead>
              <tr>
                <th scope="col">{t("名称")}</th>
                <th scope="col">{t("类型")}</th>
                <th scope="col">{t("描述")}</th>
              </tr>
            </thead>
            <tbody>
              {previewEntries.map((entry) => (
                <tr key={entry.id}>
                  <th scope="row">{entry.skillName}</th>
                  <td>Skill</td>
                  <td>
                    {entry.aiExplanation?.summary || entry.description || "—"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <>
            <p className="bundle-library-members-label">{t("精选成员")}</p>
            <ul
              className="bundle-library-members"
              aria-label={t("{group} 的 Skill", { group: title })}
            >
              {previewEntries.map((entry) => {
                const description =
                  entry.aiExplanation?.summary || entry.description;
                return (
                  <li key={entry.id}>
                    <span className="bundle-member-initial" aria-hidden="true">
                      {skillInitials(entry.skillName)}
                    </span>
                    <span className="bundle-member-copy">
                      <strong>{entry.skillName}</strong>
                      {description ? <small>{description}</small> : null}
                    </span>
                  </li>
                );
              })}
            </ul>
          </>
        )
      ) : null}
      {entries.length === 1 ? (
        <div className="bundle-library-single-member">
          <strong className="bundle-library-single-member-name">
            {entries[0]!.skillName}
          </strong>
          <SkillAiPresentation
            entry={entries[0]!}
            language={language}
            detailed={false}
          />
        </div>
      ) : null}
      <footer className="inventory-section-footer">
        <button
          className="bundle-open-action"
          type="button"
          aria-label={openLabel}
          onClick={onOpen}
        >
          <span>{openText}</span>
          <span className="bundle-open-arrow" aria-hidden="true" />
        </button>
        {hasLifecycleActions ? (
          <details className="inventory-section-menu">
            <summary aria-label={t("更多 Bundle 操作")}>{t("更多")}</summary>
            <div className="inventory-section-actions">
              {batchMountBundleId && canAssociateSource ? (
                <button
                  className="compact-action"
                  type="button"
                  disabled={actionsDisabled}
                  onClick={() => onAssociateSource?.(batchMountBundleId)}
                >
                  {t("补充来源")}
                </button>
              ) : null}
              {batchMountBundleId ? (
                <button
                  className="compact-action"
                  type="button"
                  disabled={actionsDisabled}
                  onClick={() => onBatchMount?.(batchMountBundleId)}
                >
                  {t("批量挂载")}
                </button>
              ) : null}
              {batchMountBundleId && mounts.length > 0 ? (
                <button
                  className="compact-action"
                  type="button"
                  aria-label={t(
                    unmountingBundleId === batchMountBundleId
                      ? "正在准备解除…：Bundle {bundle}"
                      : "解除全部挂载：Bundle {bundle}",
                    { bundle: title },
                  )}
                  disabled={actionsDisabled}
                  onClick={() => onUnmountBundle?.(batchMountBundleId)}
                >
                  {unmountingBundleId === batchMountBundleId
                    ? t("正在准备解除…")
                    : t("解除全部挂载")}
                </button>
              ) : null}
              {batchMountBundleId ? (
                <button
                  className="danger-outline-action"
                  type="button"
                  aria-label={t("删除 Bundle {bundle}", { bundle: title })}
                  disabled={actionsDisabled}
                  onClick={() => onRemoveBundle?.(batchMountBundleId)}
                >
                  {removingBundleId === batchMountBundleId
                    ? t("正在准备删除…")
                    : t("删除 Bundle")}
                </button>
              ) : null}
              {onTakeover ? (
                <button
                  className="compact-action"
                  type="button"
                  disabled={actionsDisabled}
                  aria-label={t("接管 Bundle {bundle}", { bundle: title })}
                  onClick={() => onTakeover(entries[0]!.id)}
                >
                  {t("接管 Bundle")}
                </button>
              ) : null}
            </div>
          </details>
        ) : null}
      </footer>
      {entries.length === 1 && !batchMountBundleId ? (
        <small className="bundle-context">
          {entries[0]!.observedBy.map(supportedAppLabel).join("、") ||
            t("本地安装")}
          {" · "}
          {entries[0]!.skillRoot}
        </small>
      ) : null}
    </section>
  );
}

function skillInitials(name: string): string {
  const initials = name
    .split(/[\s/_-]+/u)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => Array.from(part)[0])
    .join("");
  return initials.toLocaleUpperCase() || "SY";
}

function InventorySettingsPage({
  language,
  theme,
  aiPreferences,
  isSavingLanguage,
  languageError,
  isSavingTheme,
  themeError,
  aiOperation,
  aiError,
  isWriteBlocked,
  isRefreshing,
  isCheckingUpdates,
  isOpeningCentralStore,
  isResettingApplication,
  centralStoreError,
  resetError,
  onOpenCentralStore,
  onResetApplication,
  onRefresh,
  onCheckUpdates,
  onLanguageChange,
  onThemeChange,
  onAiConfigurationChange,
  onSaveAiApiKey,
  onDeleteAiApiKey,
  onTestAiConnection,
}: {
  language: InterfaceLanguage;
  theme: ThemePreset;
  aiPreferences: AiPreferences;
  isSavingLanguage: boolean;
  languageError: string | null;
  isSavingTheme: boolean;
  themeError: string | null;
  aiOperation:
    | "savingConfiguration"
    | "savingKey"
    | "deletingKey"
    | "testing"
    | null;
  aiError: string | null;
  isWriteBlocked: boolean;
  isRefreshing: boolean;
  isCheckingUpdates: boolean;
  isOpeningCentralStore: boolean;
  isResettingApplication: boolean;
  centralStoreError: string | null;
  resetError: string | null;
  onOpenCentralStore(): void;
  onResetApplication(): void;
  onRefresh(): void;
  onCheckUpdates(): void;
  onLanguageChange(language: InterfaceLanguage): void;
  onThemeChange(theme: ThemePreset): void;
  onAiConfigurationChange(configuration: AiConfigurationInput): Promise<void>;
  onSaveAiApiKey(apiKey: string): Promise<void>;
  onDeleteAiApiKey(): Promise<void>;
  onTestAiConnection(): Promise<void>;
}) {
  const { t } = useI18n();
  const [apiKey, setApiKey] = useState("");
  // 只显示用户当前输入的值；已保存的 Key 仍然不会从 Keychain 读回前端。
  const [isApiKeyVisible, setIsApiKeyVisible] = useState(false);
  const pendingThemeFocusRef = useRef<ThemePreset | null>(null);
  const themeInputRefs = useRef<Record<ThemePreset, HTMLInputElement | null>>({
    layers: null,
    ledger: null,
  });
  const aiBusy = aiOperation !== null;
  const connectionTestDisabledReason = !aiPreferences.hasApiKey
    ? t("保存 API Key 后可测试连接")
    : !aiPreferences.disclosureAccepted
      ? t("同意向 Provider 发送测试请求后可测试连接")
      : null;
  const connectionStatus =
    aiOperation === "testing"
      ? t("正在测试连接…")
      : aiPreferences.verified
        ? t("连接测试成功")
        : (connectionTestDisabledReason ?? t("连接尚未测试"));

  useLayoutEffect(() => {
    const pendingTheme = pendingThemeFocusRef.current;
    if (isSavingTheme || !pendingTheme) return;

    const activeElement = document.activeElement;
    if (
      activeElement === document.body ||
      !activeElement ||
      !activeElement.isConnected
    ) {
      themeInputRefs.current[pendingTheme]?.focus();
    }
    pendingThemeFocusRef.current = null;
  }, [isSavingTheme, theme]);
  const updateAi = (
    changes: Partial<Pick<AiPreferences, "enabled" | "disclosureAccepted" | "provider" | "model">>,
  ) =>
    onAiConfigurationChange({
      enabled: changes.enabled ?? aiPreferences.enabled,
      disclosureAccepted:
        changes.disclosureAccepted ?? aiPreferences.disclosureAccepted,
      provider: changes.provider ?? aiPreferences.provider,
      model: changes.model ?? aiPreferences.model,
    });
  return (
    <section className="inventory-settings-page">
      <header className="settings-page-heading">
        <div>
          <p className="section-eyebrow">{t("设置")}</p>
          <h1>{t("设置")}</h1>
          <p className="settings-page-lead">
            {t("主题只改变技能库的视觉表达；当前路由、搜索、筛选与 Agent 会话均会保留。")}
          </p>
        </div>
      </header>

      <section className="settings-card settings-language-card">
        <div>
          <p className="section-eyebrow">LANGUAGE</p>
          <h2>{t("语言")}</h2>
          <p>{t("切换后立即更新，并在下次启动时保留。")}</p>
        </div>
        <label className="settings-select">
          <span>{t("界面语言")}</span>
          <select
            aria-label={t("界面语言")}
            value={language}
            disabled={isSavingLanguage}
            onChange={(event) =>
              onLanguageChange(event.target.value as InterfaceLanguage)
            }
          >
            {/* 语言名称是自身标识，不能跟随当前界面语言翻译。 */}
            <option value="en">English</option>
            <option value="zhCn">简体中文</option>
          </select>
        </label>
      </section>

      <section className="settings-card settings-card-stack settings-theme-card">
        <div>
          <p className="section-eyebrow">APPEARANCE</p>
          <h2>{t("技能库主题")}</h2>
          <p>{t("两套主题共享同一产品状态与交互入口。")}</p>
        </div>
        <fieldset className="theme-preset-options" disabled={isSavingTheme}>
          <legend className="visually-hidden">{t("主题")}</legend>
          {(["layers", "ledger"] as const).map((preset) => {
            const label = preset === "ledger" ? "Ledger" : "Layers";
            return (
              <label className="theme-preset-option" key={preset}>
                <input
                  ref={(element) => {
                    themeInputRefs.current[preset] = element;
                  }}
                  type="radio"
                  name="theme-preset"
                  value={preset}
                  aria-label={label}
                  checked={theme === preset}
                  onChange={() => {
                    pendingThemeFocusRef.current = preset;
                    onThemeChange(preset);
                  }}
                />
                <span>
                  {/* Preset 名称是稳定的产品标识，不跟随界面语言翻译。 */}
                  <strong>{label}</strong>
                  <small>
                    {preset === "ledger"
                      ? t("高密度主从清单")
                      : t("纸面、书脊与分层结构")}
                  </small>
                </span>
              </label>
            );
          })}
        </fieldset>
        {themeError ? (
          <p className="inline-error" role="alert">
            <strong>{t("主题未保存")}</strong>
            <span>{themeError}</span>
          </p>
        ) : null}
      </section>

      <section className="settings-card settings-card-stack settings-provider-card">
        <div>
          <p className="section-eyebrow">AGENT PROVIDER</p>
          <h2>Agent Provider</h2>
          <p>
            {t("Provider、模型与 Key 由用户配置；SkillYard 不会自动测试连接。")}
          </p>
        </div>

        <div className="settings-ai-controls">
          <div className="settings-ai-grid">
            <label className="settings-select">
              <span>{t("模型供应商")}</span>
              <select
                aria-label={t("模型供应商")}
                value={aiPreferences.provider}
                disabled={aiBusy}
                onChange={(event) => {
                  const provider = event.target
                    .value as AiPreferences["provider"];
                  updateAi({
                    provider,
                    model: DEFAULT_AI_MODELS[provider],
                  });
                }}
              >
                <option value="openAi">OpenAI</option>
                <option value="glm">GLM</option>
                <option value="deepSeek">DeepSeek</option>
              </select>
            </label>
            <label className="settings-select">
              <span>{t("模型")}</span>
              <select
                aria-label={t("模型")}
                value={aiPreferences.model}
                disabled={aiBusy}
                onChange={(event) => updateAi({ model: event.target.value })}
              >
                {AI_MODELS[aiPreferences.provider].map((model) => (
                  <option key={model} value={model}>
                    {model}
                  </option>
                ))}
              </select>
            </label>
          </div>

          <details className="settings-provider-advanced">
            <summary
              aria-label={`${
                aiPreferences.hasApiKey
                  ? t("API Key 已保存在 macOS Keychain")
                  : t("尚未保存 API Key")
              } · ${connectionStatus} · ${t("管理 Agent Provider")}`}
            >
              <span>
                {aiPreferences.hasApiKey
                  ? t("API Key 已保存在 macOS Keychain")
                  : t("尚未保存 API Key")}
              </span>
              <span role="status" aria-live="polite">
                {connectionStatus}
              </span>
            </summary>
            <div className="settings-provider-advanced-content">
              <label className="settings-check">
                <input
                  type="checkbox"
                  checked={aiPreferences.enabled}
                  disabled={aiBusy}
                  onChange={(event) =>
                    updateAi({ enabled: event.target.checked })
                  }
                />
                <span>{t("启用 AI")}</span>
              </label>
              <label className="settings-check">
                <input
                  type="checkbox"
                  checked={aiPreferences.disclosureAccepted}
                  disabled={aiBusy}
                  onChange={(event) =>
                    updateAi({ disclosureAccepted: event.target.checked })
                  }
                />
                <span>
                  {t("同意将非敏感 Skill 内容发送给所选 Provider")}
                </span>
              </label>

              <div className="settings-key-row">
                <div className="settings-key-field">
                  <span>API Key</span>
                  <div className="settings-key-input">
                    <input
                      aria-label="API Key"
                      type={isApiKeyVisible ? "text" : "password"}
                      autoComplete="off"
                      value={apiKey}
                      disabled={aiBusy}
                      onChange={(event) => setApiKey(event.target.value)}
                    />
                    <button
                      className="secondary-action compact-action"
                      type="button"
                      aria-pressed={isApiKeyVisible}
                      disabled={aiBusy || apiKey.length === 0}
                      onClick={() =>
                        setIsApiKeyVisible((visible) => !visible)
                      }
                    >
                      {isApiKeyVisible
                        ? t("隐藏 API Key")
                        : t("显示 API Key")}
                    </button>
                  </div>
                </div>
                <button
                  className="secondary-action"
                  type="button"
                  disabled={aiBusy || apiKey.trim().length === 0}
                  onClick={async () => {
                    await onSaveAiApiKey(apiKey);
                    setApiKey("");
                    setIsApiKeyVisible(false);
                  }}
                >
                  {aiOperation === "savingKey"
                    ? t("正在保存…")
                    : t("保存 API Key")}
                </button>
                {aiPreferences.hasApiKey ? (
                  <button
                    className="secondary-action danger-muted"
                    type="button"
                    disabled={aiBusy}
                    onClick={onDeleteAiApiKey}
                  >
                    {aiOperation === "deletingKey"
                      ? t("正在删除…")
                      : t("删除 API Key")}
                  </button>
                ) : null}
              </div>

              <div className="settings-ai-status">
                <button
                  className="primary-action compact-action"
                  type="button"
                  disabled={aiBusy || connectionTestDisabledReason !== null}
                  onClick={onTestAiConnection}
                >
                  {aiOperation === "testing"
                    ? t("正在测试…")
                    : t("测试连接")}
                </button>
              </div>
              {aiError ? (
                <p className="inline-error settings-ai-feedback" role="alert">
                  <strong>{t("AI 操作失败")}</strong>
                  <span>{aiError}</span>
                </p>
              ) : null}
            </div>
          </details>
        </div>
      </section>

      <section className="settings-card settings-maintenance-card">
        <div>
          <p className="section-eyebrow">MAINTENANCE</p>
          <h2>{t("维护")}</h2>
          <p>
            {t("刷新本机是只读盘点；检查更新只访问已登记 Source，两者不会合并。")}
          </p>
        </div>
        <div className="settings-maintenance-actions">
          <button
            className="secondary-action"
            type="button"
            disabled={isWriteBlocked || isRefreshing}
            onClick={onRefresh}
          >
            {isRefreshing ? t("正在刷新本机…") : t("刷新本机")}
          </button>
          <button
            className="secondary-action"
            type="button"
            disabled={isWriteBlocked || isCheckingUpdates}
            onClick={onCheckUpdates}
          >
            {isCheckingUpdates ? t("正在检查更新…") : t("检查更新")}
          </button>
        </div>
      </section>

      <section className="settings-card settings-central-store-card">
        <div>
          <p className="section-eyebrow">CENTRAL STORE</p>
          <h2>{t("受管内容目录")}</h2>
          <p>
            {t(
              "这里保存 SkillYard 管理的实际主副本，不是可以随意清理的缓存。",
            )}
          </p>
        </div>
        <button
          className="secondary-action"
          type="button"
          disabled={isOpeningCentralStore}
          onClick={onOpenCentralStore}
        >
          {isOpeningCentralStore
            ? t("正在打开…")
            : t("打开 Central Store")}
        </button>
      </section>

      <section className="settings-card settings-reset-card">
        <div>
          <p className="section-eyebrow">APPLICATION</p>
          <h2>{t("重置界面状态")}</h2>
          <p>
            {t("只清除偏好、窗口状态和缓存，不删除 Bundle、Skill 或 Mount。")}
          </p>
        </div>
        <button
          className="secondary-action"
          type="button"
          disabled={isWriteBlocked || isResettingApplication}
          onClick={onResetApplication}
        >
          {isResettingApplication ? t("正在重置…") : t("重置应用")}
        </button>
      </section>

      {resetError ? (
        <div className="inline-error" role="alert">
          <strong>{t("重置未完成")}</strong>
          <span>{resetError}</span>
        </div>
      ) : null}
      {centralStoreError ? (
        <div className="inline-error" role="alert">
          <strong>{t("无法打开 Central Store")}</strong>
          <span>{centralStoreError}</span>
        </div>
      ) : null}
      {languageError ? (
        <div className="inline-error" role="alert">
          <strong>{t("语言")}</strong>
          <span>{languageError}</span>
        </div>
      ) : null}
    </section>
  );
}

function InventoryGroupDetails({
  group,
  entries,
  categoryFilter,
  language,
  mounts,
  onOpenSkill,
  onBack,
}: {
  group: InventoryGroupView;
  entries: InventoryObservation[];
  categoryFilter: CategoryFilter;
  language: InterfaceLanguage;
  mounts: MountSummary[];
  onOpenSkill(entryId: string): void;
  onBack(): void;
}) {
  const { t } = useI18n();
  return (
    <main className="inventory-shell inventory-subpage">
      <PageBackButton onClick={onBack} />
      <header className="detail-header">
        <div>
          <p className="eyebrow">{t(groupEyebrow(group.kind))}</p>
          <h1>{group.title}</h1>
          <p className="inventory-summary">
            {categoryFilter === "all"
              ? t("{count} 个 Skill", { count: entries.length })
              : t("{visible} / {total} 个 Skill", {
                  visible: entries.length,
                  total: group.entries.length,
                })}
          </p>
        </div>
      </header>

      <ul
        className="skill-member-list"
        aria-label={t("{group} 的 Skill", { group: group.title })}
      >
        {entries.map((entry) => {
          const memberMounts = mounts.filter(
            (mount) => mount.memberId === entry.memberId,
          );
          return (
            <li key={entry.id} className="skill-member-row">
              <div className="skill-member-copy">
                <div className="skill-member-heading">
                  <strong>{entry.skillName}</strong>
                  <span className={`management-badge ${entry.managementKind}`}>
                    {managementLabel(entry.managementKind, t)}
                  </span>
                  <small>
                    {memberMounts.length > 0
                      ? t("{count} 个 Mount", { count: memberMounts.length })
                      : t("未挂载")}
                  </small>
                </div>
                <SkillAiPresentation
                  entry={entry}
                  language={language}
                  detailed
                />
              </div>
              <button
                className="compact-action"
                type="button"
                aria-label={t("查看 Skill {skill}", {
                  skill: entry.skillName,
                })}
                onClick={() => onOpenSkill(entry.id)}
              >
                {t("查看详情")}
              </button>
            </li>
          );
        })}
      </ul>
    </main>
  );
}

function SkillAiPresentation({
  entry,
  language,
  detailed,
}: {
  entry: InventoryObservation;
  language: InterfaceLanguage;
  detailed: boolean;
}) {
  const { t } = useI18n();
  const explanation = entry.aiExplanation;
  if (!explanation) {
    return (
      <div className="skill-ai-presentation is-empty">
        {entry.description ? (
          <p className="skill-source-description">{entry.description}</p>
        ) : null}
        <span className="skill-ai-state">{t("未整理")}</span>
      </div>
    );
  }
  const stale = isSkillExplanationPending(entry, language);
  return (
    <div className="skill-ai-presentation">
      <div className="skill-ai-presentation-heading">
        <span className="skill-category">
          {t(skillCategoryLabel(explanation.category))}
        </span>
        {stale ? (
          <span className="skill-ai-state is-stale">
            {t("待重新整理")}
          </span>
        ) : null}
      </div>
      <p className="skill-ai-presentation-summary">{explanation.summary}</p>
      {detailed ? (
        <div className="skill-ai-presentation-details">
          <div>
            <strong>{t("适用场景")}</strong>
            <ul>
              {explanation.useCases.map((useCase) => (
                <li key={useCase}>{useCase}</li>
              ))}
            </ul>
          </div>
          <div>
            <strong>{t("使用说明")}</strong>
            <p>{explanation.instructions}</p>
          </div>
        </div>
      ) : null}
    </div>
  );
}

function SkillDetailsPage({
  group,
  entry,
  mounts,
  actionsDisabled,
  allowReadOnlyDetails,
  mountError,
  canGenerateExplanation,
  isGeneratingExplanation,
  explanationError,
  explanationStale,
  onGenerateExplanation,
  onManageMount,
  onBack,
}: {
  group: InventoryGroupView;
  entry: InventoryObservation;
  mounts: MountSummary[];
  actionsDisabled: boolean;
  allowReadOnlyDetails: boolean;
  mountError: string | null;
  canGenerateExplanation: boolean;
  isGeneratingExplanation: boolean;
  explanationError: string | null;
  explanationStale: boolean;
  onGenerateExplanation(): void;
  onManageMount(memberId: string): void;
  onBack(): void;
}) {
  const { t } = useI18n();
  const sourceName =
    entry.sourceDisplayName ??
    entry.installationChain?.source ??
    t("来源未知");
  return (
    <main className="inventory-shell inventory-subpage">
      <PageBackButton onClick={onBack} />
      <header className="detail-header">
        <div>
          <p className="eyebrow">{group.title}</p>
          <h1>{entry.skillName}</h1>
          <span className={`management-badge ${entry.managementKind}`}>
            {managementLabel(entry.managementKind, t)}
          </span>
        </div>
      </header>

      <section className="skill-detail-card" aria-label={t("Skill 详情")}>
        <dl>
          <div>
            <dt>{t("所属分组")}</dt>
            <dd>{group.title}</dd>
          </div>
          <div>
            <dt>{t("来源")}</dt>
            <dd>{sourceName}</dd>
          </div>
          {entry.description ? (
            <div>
              <dt>{t("SKILL.md 描述")}</dt>
              <dd>{entry.description}</dd>
            </div>
          ) : null}
          <div>
            <dt>{t("本地目录")}</dt>
            <dd>
              <code>{entry.skillRoot}</code>
            </dd>
          </div>
          <div>
            <dt>{t("定义文件")}</dt>
            <dd>
              <code>{entry.skillFile}</code>
            </dd>
          </div>
          <div>
            <dt>Metadata</dt>
            <dd>
              {entry.metadataStatus === "valid" ? t("有效") : t("需要检查")}
            </dd>
          </div>
        </dl>
      </section>

      <section className="skill-detail-card" aria-label={t("AI 说明")}>
        <div className="skill-ai-header">
          <div>
            <p className="section-eyebrow">AI ORGANIZATION</p>
            <h2>{t("AI 说明")}</h2>
          </div>
          <button
            className="compact-action"
            type="button"
            disabled={!canGenerateExplanation || isGeneratingExplanation}
            onClick={onGenerateExplanation}
          >
            {isGeneratingExplanation
              ? t("正在整理…")
              : entry.aiExplanation
                ? t("重新整理")
                : t("AI 整理")}
          </button>
        </div>
        {entry.aiExplanation ? (
          <div className="skill-ai-content">
            <span className="skill-category">
              {t(skillCategoryLabel(entry.aiExplanation.category))}
            </span>
            {explanationStale ? (
              <p className="skill-ai-stale">{t("待重新整理")}</p>
            ) : null}
            <p className="skill-ai-summary">{entry.aiExplanation.summary}</p>
            <div>
              <h3>{t("适用场景")}</h3>
              <ul>
                {entry.aiExplanation.useCases.map((useCase) => (
                  <li key={useCase}>{useCase}</li>
                ))}
              </ul>
            </div>
            <div>
              <h3>{t("使用说明")}</h3>
              <p>{entry.aiExplanation.instructions}</p>
            </div>
          </div>
        ) : (
          <p className="skill-ai-empty">{t("尚未生成 AI 说明")}</p>
        )}
        {!canGenerateExplanation && !allowReadOnlyDetails ? (
          <p className="skill-ai-empty">
            {t("请先在设置中启用 AI 并完成连接测试。")}
          </p>
        ) : null}
        {explanationError ? (
          <div className="inline-error" role="alert">
            {explanationError}
          </div>
        ) : null}
      </section>

      {entry.installationChain ? (
        <section className="skill-detail-card" aria-label={t("安装来源记录")}>
          <p className="section-eyebrow">INSTALLATION RECEIPT</p>
          <h2>{t("安装来源记录")}</h2>
          <dl>
            <div>
              <dt>{t("来源名称")}</dt>
              <dd>{entry.installationChain.source}</dd>
            </div>
            <div>
              <dt>{t("来源地址")}</dt>
              <dd>
                <code>{entry.installationChain.sourceLocator}</code>
              </dd>
            </div>
            {entry.installationChain.skillPath ? (
              <div>
                <dt>{t("仓库内路径")}</dt>
                <dd>
                  <code>{entry.installationChain.skillPath}</code>
                </dd>
              </div>
            ) : null}
          </dl>
        </section>
      ) : null}

      <section className="skill-detail-card" aria-label={t("当前挂载")}>
        <p className="section-eyebrow">MOUNTS</p>
        <h2>{t("当前挂载")}</h2>
        <div className="mount-badges">
          {mounts.length > 0 ? (
            mounts.map((mount) => (
              <span key={mount.id} className={`mount-badge ${mount.health}`}>
                {mountLabel(mount, t)}
                {mount.health === "healthy"
                  ? ""
                  : ` · ${mountHealthLabel(mount.health, t)}`}
              </span>
            ))
          ) : (
            <span className="mount-empty">{t("未挂载")}</span>
          )}
        </div>
        {entry.managementKind === "skillYardManaged" && entry.memberId ? (
          <button
            className="compact-action"
            type="button"
            disabled={actionsDisabled && !allowReadOnlyDetails}
            onClick={() => onManageMount(entry.memberId!)}
          >
            {t("管理挂载")}
          </button>
        ) : null}
      </section>

      {mountError ? (
        <div className="inline-error" role="alert">
          <strong>{t("挂载操作未完成")}</strong>
          <span>{mountError}</span>
        </div>
      ) : null}

      {managementDirection(entry, t) ? (
        <p className="management-direction">
          {managementDirection(entry, t)}
        </p>
      ) : null}
    </main>
  );
}

function skillCategoryLabel(category: SkillCategory): TranslationKey {
  return {
    developmentEngineering: "开发与工程",
    systemOperations: "系统与运维",
    productivityAutomation: "效率与自动化",
    dataAnalytics: "数据与分析",
    productBusiness: "产品与业务",
    researchLearning: "研究与学习",
    writingCommunication: "写作与沟通",
    designCreative: "设计与创意",
    securityCompliance: "安全与合规",
    other: "其他",
  }[category] as TranslationKey;
}

function isSkillExplanationPending(
  entry: InventoryObservation,
  language: InterfaceLanguage,
): boolean {
  const explanation = entry.aiExplanation;
  return (
    !explanation ||
    explanation.stale ||
    explanation.language !== language ||
    explanation.contentFingerprint !== entry.observedFingerprint
  );
}

function BundleUpdateStatusView({
  update,
  bundleDisplayName,
  isPreparing,
  isChecking,
  actionsDisabled,
  showStatus = true,
  onUpdate,
  onImportReplacement,
  onCheckEditableLocal,
}: {
  update: BundleUpdateSummary;
  bundleDisplayName: string;
  isPreparing: boolean;
  isChecking: boolean;
  actionsDisabled: boolean;
  showStatus?: boolean;
  onUpdate?: () => void;
  onImportReplacement?: () => void;
  onCheckEditableLocal?: () => void;
}) {
  const { language, localize, t } = useI18n();
  const actionLabel = bundleUpdateActionLabel(
    update.status,
    update.action,
    t,
  );
  const actionHandler =
    update.action === "update"
      ? onUpdate
      : update.action === "importReplacement"
        ? onImportReplacement
        : update.action === "checkEditableLocal"
          ? onCheckEditableLocal
          : undefined;
  const actionBusy =
    update.action === "checkEditableLocal" ? isChecking : isPreparing;
  const checkedAt = update.checkedAt
    ? t("检查于 {time}", {
        time: formatTimestamp(update.checkedAt, language),
      })
    : null;
  const detail = [
    checkedAt,
    update.message
      ? localize(update.message, "无法读取最新更新状态。")
      : null,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <div
      className="bundle-update-summary"
      aria-label={t("Bundle 更新状态：{status}", {
        status: bundleUpdateStatusLabel(update.status, t),
      })}
    >
      {showStatus ? (
        <span className={`bundle-update-status is-${update.status}`}>
          {bundleUpdateStatusLabel(update.status, t)}
        </span>
      ) : null}
      {actionLabel && actionHandler ? (
        <button
          className="bundle-update-action"
          type="button"
          aria-label={`${actionLabel} ${bundleDisplayName}`}
          disabled={actionsDisabled}
          onClick={actionHandler}
        >
          {actionBusy ? bundleUpdateBusyLabel(update.action, t) : actionLabel}
        </button>
      ) : actionLabel ? (
        <span className="bundle-update-action-label">{actionLabel}</span>
      ) : null}
      {detail ? (
        <small title={detail}>{detail}</small>
      ) : null}
    </div>
  );
}

function bundleUpdateStatusLabel(
  status: BundleUpdateStatus,
  t: ReturnType<typeof useI18n>["t"],
): string {
  return {
    noSource: t("没有更新来源"),
    notChecked: t("尚未检查"),
    available: t("可更新"),
    upToDate: t("已是最新"),
    unableToCheck: t("无法检查"),
    manual: t("手动更新"),
    sourceUnavailable: t("来源不可用"),
  }[status];
}

function bundleUpdateActionLabel(
  status: BundleUpdateStatus,
  action: BundleUpdateAction,
  t: ReturnType<typeof useI18n>["t"],
): string | null {
  if (action === "update") return t("更新");
  if (action === "importReplacement") return t("导入新内容");
  if (action === "checkEditableLocal") {
    if (status === "upToDate") return t("再次检查");
    if (status === "sourceUnavailable" || status === "unableToCheck") {
      return t("重新检查");
    }
    return t("检查本地改动");
  }
  return null;
}

function bundleUpdateBusyLabel(
  action: BundleUpdateAction,
  t: ReturnType<typeof useI18n>["t"],
): string {
  if (action === "importReplacement") return t("正在选择新内容…");
  if (action === "checkEditableLocal") return t("正在检查本地改动…");
  return t("正在准备…");
}

function mountLabel(
  mount: MountSummary,
  t: ReturnType<typeof useI18n>["t"],
): string {
  const appName = supportedAppLabel(mount.appId);
  return mount.scope === "global"
    ? t("{app} · 全局", { app: appName })
    : t("{app} · {project}", {
        app: appName,
        project: mount.projectDisplayName ?? t("已登记项目"),
      });
}

function mountLabelForDisplay(
  mount: MountSummary,
  t: ReturnType<typeof useI18n>["t"],
): string {
  const label = mountLabel(mount, t);
  return mount.health === "healthy"
    ? label
    : `${label} · ${mountHealthLabel(mount.health, t)}`;
}

function representativeMountDestinations(
  mounts: readonly MountSummary[],
  limit: number,
): MountSummary[] {
  if (limit <= 0) return [];
  const result: MountSummary[] = [];
  const selectedIds = new Set<string>();
  const destinationKinds = new Set<string>();

  for (const mount of mounts) {
    const destinationKind = [
      mount.appId,
      mount.scope,
      mount.projectId ?? "",
    ].join("\u0000");
    if (destinationKinds.has(destinationKind)) continue;
    destinationKinds.add(destinationKind);
    selectedIds.add(mount.id);
    result.push(mount);
    if (result.length === limit) return result;
  }

  for (const mount of mounts) {
    if (selectedIds.has(mount.id)) continue;
    result.push(mount);
    if (result.length === limit) break;
  }
  return result;
}

function mountHealthLabel(
  health: MountSummary["health"],
  t: ReturnType<typeof useI18n>["t"],
): string {
  return {
    healthy: t("正常"),
    missing: t("已缺失"),
    conflict: t("路径冲突"),
  }[health];
}

function groupManagedEntries(
  entries: InventoryObservation[],
  t: ReturnType<typeof useI18n>["t"],
  locale: string,
): Array<{
  id: string;
  bundleId: string | null;
  title: string;
  hasSource: boolean;
  entries: InventoryObservation[];
}> {
  const groups = new Map<
    string,
    {
      id: string;
      bundleId: string | null;
      title: string;
      hasSource: boolean;
      entries: InventoryObservation[];
    }
  >();
  for (const entry of entries) {
    if (entry.managementKind !== "skillYardManaged") continue;
    // Bundle 名只是展示文字；生命周期分组必须使用稳定 ID，缺失时也不能误合并。
    const id = entry.bundleId ?? `unassigned:${entry.id}`;
    const existing = groups.get(id);
    groups.set(id, {
      id,
      // fallback 分组仅用于显示，绝不能把合成 ID 交给生命周期命令。
      bundleId: entry.bundleId ?? null,
      title: entry.bundleDisplayName ?? t("本地 Bundle"),
      hasSource:
        (existing?.hasSource ?? false) || Boolean(entry.sourceDisplayName),
      entries: [...(existing?.entries ?? []), entry],
    });
  }
  return [...groups.values()].sort((left, right) =>
    left.title.localeCompare(right.title, locale) ||
    left.id.localeCompare(right.id),
  );
}

function groupTakeoverEntries(
  entries: InventoryObservation[],
  t: ReturnType<typeof useI18n>["t"],
  locale: string,
): {
  groups: Array<{
    id: string;
    title: string;
    entries: InventoryObservation[];
  }>;
  ungrouped: InventoryObservation[];
} {
  const grouped = new Map<
    string,
    {
      id: string;
      title: string;
      entries: InventoryObservation[];
    }
  >();
  const ungrouped: InventoryObservation[] = [];
  for (const entry of entries) {
    // 缺少确定性分组证据时保持独立，不能用 skillName 或展示名冒充分组 ID。
    if (!entry.takeoverGroupId) {
      ungrouped.push(entry);
      continue;
    }
    const current = grouped.get(entry.takeoverGroupId);
    grouped.set(entry.takeoverGroupId, {
      id: entry.takeoverGroupId,
      title:
        entry.takeoverGroupDisplayName ??
        current?.title ??
        t("本地 Bundle"),
      entries: [...(current?.entries ?? []), entry],
    });
  }
  return {
    groups: [...grouped.values()].sort(
      (left, right) =>
        left.title.localeCompare(right.title, locale) ||
        left.id.localeCompare(right.id),
    ),
    ungrouped,
  };
}

function groupInventoryEntries(
  entries: InventoryObservation[],
  t: ReturnType<typeof useI18n>["t"],
  language: InterfaceLanguage,
): InventoryGroupView[] {
  const locale = language === "zhCn" ? "zh-CN" : "en";
  const managed = groupManagedEntries(entries, t, locale).map((group) => ({
    id: `managed:${group.id}`,
    title: group.title,
    kind: "managedBundle" as const,
    entries: group.entries,
    bundleId: group.bundleId,
    hasSource: group.hasSource,
  }));
  const takeover = groupTakeoverEntries(
    entries.filter(
      (entry) => entry.managementKind === "takeoverCandidate",
    ),
    t,
    locale,
  );
  const takeoverGroups: InventoryGroupView[] = [
    ...takeover.groups.map((group) => ({
      id: `takeover:${group.id}`,
      title: group.title,
      kind: "takeoverBundle" as const,
      entries: group.entries,
      bundleId: null,
      hasSource: false,
    })),
    ...takeover.ungrouped.map((entry) => ({
      id: `takeover-entry:${entry.id}`,
      title: entry.skillName,
      kind: "takeoverBundle" as const,
      entries: [entry],
      bundleId: null,
      hasSource: false,
    })),
  ];

  const external = new Map<string, InventoryGroupView>();
  for (const entry of entries) {
    if (
      entry.managementKind !== "agentManaged" &&
      entry.managementKind !== "projectManaged"
    ) {
      continue;
    }
    const isAgentManaged = entry.managementKind === "agentManaged";
    const appKey = entry.observedBy.slice().sort().join(":");
    const id = isAgentManaged
      ? `agent:${entry.externalGroupDisplayName || appKey || entry.id}`
      : `project:${entry.projectId ?? entry.id}`;
    const appNames = entry.observedBy
      .map(supportedAppLabel)
      .join(language === "zhCn" ? "、" : ", ");
    const title = isAgentManaged
      ? entry.externalGroupDisplayName ??
        (appNames
          ? t("{apps} 管理", { apps: appNames })
          : t("Agent 应用管理"))
      : entry.projectDisplayName ?? t("项目仓库管理");
    const current = external.get(id);
    external.set(id, {
      id,
      title: current?.title ?? title,
      kind: isAgentManaged ? "agentManaged" : "projectManaged",
      entries: [...(current?.entries ?? []), entry],
      bundleId: null,
      hasSource: false,
    });
  }

  const order: Record<InventoryGroupKind, number> = {
    managedBundle: 0,
    takeoverBundle: 1,
    agentManaged: 2,
    projectManaged: 3,
  };
  return [...managed, ...takeoverGroups, ...external.values()]
    .map((group) => ({
      ...group,
      entries: group.entries.slice().sort((left, right) =>
        left.skillName.localeCompare(right.skillName, locale),
      ),
    }))
    .sort(
      (left, right) =>
        order[left.kind] - order[right.kind] ||
        left.title.localeCompare(right.title, locale) ||
        left.id.localeCompare(right.id),
    );
}

function groupEyebrow(kind: InventoryGroupKind): TranslationKey {
  const labels: Record<InventoryGroupKind, TranslationKey> = {
    managedBundle: "由 SkillYard 管理 · BUNDLE",
    takeoverBundle: "待接管 · BUNDLE",
    agentManaged: "Agent 应用管理 · 只读",
    projectManaged: "项目仓库管理 · 只读",
  };
  return labels[kind];
}

function matchesFilter(
  entry: InventoryObservation,
  filter: ManagementFilter,
): boolean {
  if (filter === "all") return true;
  if (filter === "managed") return entry.managementKind === "skillYardManaged";
  if (filter === "takeover") return entry.managementKind === "takeoverCandidate";
  return (
    entry.managementKind === "agentManaged" ||
    entry.managementKind === "projectManaged"
  );
}

function matchesCategory(
  entry: InventoryObservation,
  category: CategoryFilter,
): boolean {
  return category === "all" || entry.aiExplanation?.category === category;
}

function matchesQuery(entry: InventoryObservation, query: string): boolean {
  if (!query) return true;
  return [
    entry.skillName,
    entry.declaredName,
    entry.bundleDisplayName,
    entry.takeoverGroupDisplayName,
    entry.externalGroupDisplayName,
    entry.sourceDisplayName,
    entry.installationChain?.source,
    entry.installationChain?.sourceLocator,
    entry.installationChain?.skillPath,
    entry.projectDisplayName,
    ...entry.observedBy.map(supportedAppLabel),
  ]
    .filter((value): value is string => Boolean(value))
    .some((value) => value.toLocaleLowerCase("zh-CN").includes(query));
}

function managementLabel(
  kind: InventoryObservation["managementKind"],
  t: ReturnType<typeof useI18n>["t"],
): string {
  return {
    skillYardManaged: t("由 SkillYard 管理"),
    takeoverCandidate: t("待接管"),
    agentManaged: t("Agent 应用管理"),
    projectManaged: t("项目仓库管理"),
  }[kind];
}

function managementDirection(
  entry: InventoryObservation,
  t: ReturnType<typeof useI18n>["t"],
): string | null {
  if (entry.managementKind === "agentManaged") {
    const apps = entry.observedBy.map(supportedAppLabel).join(", ");
    return t("请前往 {apps} 管理此 Skill。", {
      apps: apps || t("对应 Agent 应用"),
    });
  }
  if (entry.managementKind === "projectManaged") {
    return t("请在 {project} 中管理此 Skill。", {
      project: entry.projectDisplayName ?? t("对应项目仓库"),
    });
  }
  return null;
}

function supportedAppLabel(app: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[app];
}

function formatTimestamp(
  timestamp: number,
  language: InterfaceLanguage,
): string {
  return new Intl.DateTimeFormat(language === "zhCn" ? "zh-CN" : "en", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(timestamp));
}
