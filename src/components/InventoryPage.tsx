import { useMemo, useState } from "react";

import type {
  BundleUpdateAction,
  BundleUpdateSummary,
  BundleUpdateStatus,
  InventoryObservation,
  InterfaceLanguage,
  MountSummary,
  SupportedAppId,
  UiOutcome,
} from "../domain";
import { useI18n, type TranslationKey } from "../i18n";
import { PageBackButton } from "./PageBackButton";

type InventoryOutcome = Extract<UiOutcome, { type: "inventory" }>;
type ManagementFilter = "all" | "managed" | "takeover" | "other";

interface InventoryPageProps {
  outcome: InventoryOutcome;
  screen: InventoryScreen;
  onScreenChange(screen: InventoryScreen): void;
  language: InterfaceLanguage;
  isSavingLanguage: boolean;
  languageError: string | null;
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
  isOpeningInstaller: boolean;
  isAddingProject: boolean;
  isOpeningCentralStore: boolean;
  isResettingApplication: boolean;
  refreshError: string | null;
  updateError: string | null;
  installError: string | null;
  projectError: string | null;
  removalError: string | null;
  mountError: string | null;
  takeoverError: string | null;
  sourceAssociationError: string | null;
  centralStoreError: string | null;
  resetError: string | null;
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
  onInstall(): void;
  onAddProject(): void;
  onOpenCentralStore(): void;
  onResetApplication(): void;
  onLanguageChange(language: InterfaceLanguage): void;
  onAssociateSource(bundleId: string): void;
  onOpenRecovery(issueId: string): void;
  onTakeover(observationId: string): void;
  onManageMount(memberId: string): void;
  onBatchMount(bundleId: string): void;
}

const FILTERS: Array<{ id: ManagementFilter; label: TranslationKey }> = [
  { id: "all", label: "全部" },
  { id: "managed", label: "由 SkillYard 管理" },
  { id: "takeover", label: "待接管" },
  { id: "other", label: "其他管理方" },
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
  language,
  isSavingLanguage,
  languageError,
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
  isOpeningInstaller,
  isAddingProject,
  isOpeningCentralStore,
  isResettingApplication,
  refreshError,
  updateError,
  installError,
  projectError,
  removalError,
  mountError,
  takeoverError,
  sourceAssociationError,
  centralStoreError,
  resetError,
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
  onInstall,
  onAddProject,
  onOpenCentralStore,
  onResetApplication,
  onLanguageChange,
  onAssociateSource,
  onOpenRecovery,
  onTakeover,
  onManageMount,
  onBatchMount,
}: InventoryPageProps) {
  const { localize, t } = useI18n();
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<ManagementFilter>("all");

  const groups = useMemo(
    () => groupInventoryEntries(outcome.entries, t, language),
    [language, outcome.entries, t],
  );
  // 搜索命中成员时只显示其所属分组，主清单仍不展开 Skill。
  const visibleGroups = useMemo(() => {
    const normalizedQuery = query
      .trim()
      .toLocaleLowerCase(language === "zhCn" ? "zh-CN" : "en");
    return groups.filter((group) =>
      group.entries.some(
        (entry) =>
          matchesFilter(entry, filter) &&
          matchesQuery(entry, normalizedQuery),
      ),
    );
  }, [filter, groups, language, query]);
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
  const selectedGroup =
    screen.type === "group" || screen.type === "skill"
      ? groups.find((group) => group.id === screen.groupId) ?? null
      : null;
  const selectedEntry =
    screen.type === "skill"
      ? selectedGroup?.entries.find((entry) => entry.id === screen.entryId) ??
        null
      : null;

  if (screen.type === "settings") {
    return (
      <InventorySettingsPage
        language={language}
        isSavingLanguage={isSavingLanguage}
        languageError={languageError}
        isWriteBlocked={isWriteBlocked}
        isOpeningCentralStore={isOpeningCentralStore}
        isResettingApplication={isResettingApplication}
        centralStoreError={centralStoreError}
        resetError={resetError}
        onOpenCentralStore={onOpenCentralStore}
        onResetApplication={onResetApplication}
        onLanguageChange={onLanguageChange}
        onBack={() => onScreenChange({ type: "list" })}
      />
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
        onManageMount={onManageMount}
        onBack={() =>
          onScreenChange({ type: "group", groupId: selectedGroup.id })
        }
      />
    );
  }

  if (screen.type === "group" && selectedGroup) {
    return (
      <InventoryGroupDetails
        group={selectedGroup}
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
    <main className="inventory-shell">
      <header className="inventory-header">
        <div>
          <p className="eyebrow">SKILLYARD · LOCAL INVENTORY</p>
          <h1>{t("Bundle 清单")}</h1>
          <p className="inventory-summary">
            {outcome.entries.length === 0
              ? t("本机暂未发现 Bundle")
              : t(
                  "本机已有 {bundleCount} 个 Bundle · {skillCount} 个 Skill",
                  {
                    bundleCount,
                    skillCount: outcome.entries.length,
                  },
                )}
          </p>
        </div>
        <div className="inventory-actions">
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
            className="primary-action"
            type="button"
            disabled={isWriteBlocked || isOpeningInstaller}
            onClick={onInstall}
          >
            {isOpeningInstaller ? t("正在打开…") : t("安装 Skill")}
          </button>
          <button
            className="secondary-action"
            type="button"
            disabled={isWriteBlocked || isAddingProject}
            onClick={onAddProject}
          >
            {isAddingProject ? t("正在选择项目…") : t("添加项目")}
          </button>
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
            onClick={() => onScreenChange({ type: "settings" })}
          >
            {t("设置")}
          </button>
        </div>
      </header>

      {outcome.recoveredInterruptedOperation ? (
        <p className="recovery-notice" role="status">
          {t("已恢复上次中断的操作")}
        </p>
      ) : null}

      {outcome.projects.length > 0 ? (
        <section className="registered-projects" aria-label={t("已登记项目")}>
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
                  <code title={project.rootPath}>{project.rootPath}</code>
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

      <section className="inventory-controls" aria-label={t("清单筛选")}>
        <label className="search-field">
          <span className="sr-only">{t("搜索 Skill")}</span>
          <input
            type="search"
            value={query}
            placeholder={t("搜索 Bundle 或 Skill")}
            aria-label={t("搜索 Skill")}
            onChange={(event) => setQuery(event.target.value)}
          />
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
      </section>

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

      {outcome.lastLocalRefresh ? (
        <p className="refresh-summary" aria-label={t("最近刷新结果")}>
          {t("最近刷新：新增 {added} · 变化 {changed} · 移除 {removed}", {
            added: outcome.lastLocalRefresh.added,
            changed: outcome.lastLocalRefresh.changed,
            removed: outcome.lastLocalRefresh.removed,
          })}
          <span>
            {formatTimestamp(outcome.lastLocalRefresh.completedAt, language)}
          </span>
        </p>
      ) : (
        <p className="refresh-summary">{t("尚未执行本机刷新")}</p>
      )}

      <div className="inventory-content">
        {visibleGroups.map((group) => (
          <InventorySection
            key={group.id}
            title={group.title}
            eyebrow={t(groupEyebrow(group.kind))}
            entries={group.entries}
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
            bundleMountCount={
              group.kind === "managedBundle"
                ? outcome.mounts.filter((mount) =>
                    group.entries.some(
                      (entry) => entry.memberId === mount.memberId,
                    ),
                  ).length
                : 0
            }
            unmountingBundleId={
              group.kind === "managedBundle" ? unmountingBundleId : null
            }
            onUnmountBundle={onUnmountBundle}
            onTakeover={
              group.kind === "takeoverBundle" ? onTakeover : undefined
            }
            onOpen={() =>
              onScreenChange({ type: "group", groupId: group.id })
            }
            openLabel={
              group.kind === "managedBundle" ||
              group.kind === "takeoverBundle"
                ? t("查看 Bundle {bundle}", { bundle: group.title })
                : t("查看分组 {group}", { group: group.title })
            }
          />
        ))}
        {visibleGroups.length === 0 ? (
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
          </section>
        ) : null}
      </div>
    </main>
  );
}

function InventorySection({
  title,
  eyebrow,
  entries,
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
  bundleMountCount = 0,
  unmountingBundleId = null,
  onUnmountBundle,
  onTakeover,
  onOpen,
  openLabel,
}: {
  title: string;
  eyebrow: string;
  entries: InventoryObservation[];
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
  bundleMountCount?: number;
  unmountingBundleId?: string | null;
  onUnmountBundle?(bundleId: string): void;
  onTakeover?(observationId: string): void;
  onOpen(): void;
  openLabel: string;
}) {
  const { t } = useI18n();
  if (entries.length === 0) return null;
  return (
    <section className="inventory-section" aria-label={title}>
      <header>
        <div>
          <p className="section-eyebrow">{eyebrow}</p>
          <h2>{title}</h2>
        </div>
        <div className="inventory-section-actions">
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
          {batchMountBundleId && bundleMountCount > 0 ? (
            <button
              className="compact-action"
              type="button"
              aria-label={t("解除 Bundle {bundle} 的全部挂载", {
                bundle: title,
              })}
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
          <span>{t("{count} 个 Skill", { count: entries.length })}</span>
        </div>
      </header>
      <button
        className="bundle-open-action"
        type="button"
        aria-label={openLabel}
        onClick={onOpen}
      >
        {t("查看成员")}
      </button>
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

function InventorySettingsPage({
  language,
  isSavingLanguage,
  languageError,
  isWriteBlocked,
  isOpeningCentralStore,
  isResettingApplication,
  centralStoreError,
  resetError,
  onOpenCentralStore,
  onResetApplication,
  onLanguageChange,
  onBack,
}: {
  language: InterfaceLanguage;
  isSavingLanguage: boolean;
  languageError: string | null;
  isWriteBlocked: boolean;
  isOpeningCentralStore: boolean;
  isResettingApplication: boolean;
  centralStoreError: string | null;
  resetError: string | null;
  onOpenCentralStore(): void;
  onResetApplication(): void;
  onLanguageChange(language: InterfaceLanguage): void;
  onBack(): void;
}) {
  const { t } = useI18n();
  return (
    <main className="inventory-shell inventory-subpage">
      <PageBackButton onClick={onBack} />
      <header className="detail-header">
        <div>
          <p className="eyebrow">SKILLYARD · SETTINGS</p>
          <h1>{t("设置")}</h1>
        </div>
      </header>

      <section className="settings-card">
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
            <option value="zhCn">{t("简体中文")}</option>
            <option value="en">{t("English")}</option>
          </select>
        </label>
      </section>

      <section className="settings-card">
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

      <section className="settings-card">
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
    </main>
  );
}

function InventoryGroupDetails({
  group,
  mounts,
  onOpenSkill,
  onBack,
}: {
  group: InventoryGroupView;
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
            {t("{count} 个 Skill", { count: group.entries.length })}
          </p>
        </div>
      </header>

      <ul
        className="skill-member-list"
        aria-label={t("{group} 的 Skill", { group: group.title })}
      >
        {group.entries.map((entry) => {
          const memberMounts = mounts.filter(
            (mount) => mount.memberId === entry.memberId,
          );
          return (
            <li key={entry.id} className="skill-member-row">
              <div>
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

function SkillDetailsPage({
  group,
  entry,
  mounts,
  actionsDisabled,
  allowReadOnlyDetails,
  mountError,
  onManageMount,
  onBack,
}: {
  group: InventoryGroupView;
  entry: InventoryObservation;
  mounts: MountSummary[];
  actionsDisabled: boolean;
  allowReadOnlyDetails: boolean;
  mountError: string | null;
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

function BundleUpdateStatusView({
  update,
  bundleDisplayName,
  isPreparing,
  isChecking,
  actionsDisabled,
  onUpdate,
  onImportReplacement,
  onCheckEditableLocal,
}: {
  update: BundleUpdateSummary;
  bundleDisplayName: string;
  isPreparing: boolean;
  isChecking: boolean;
  actionsDisabled: boolean;
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
    ? ` · ${formatTimestamp(update.checkedAt, language)}`
    : "";
  return (
    <div
      className="bundle-update-summary"
      aria-label={t("Bundle 更新状态：{status}", {
        status: bundleUpdateStatusLabel(update.status, t),
      })}
    >
      <span className={`bundle-update-status is-${update.status}`}>
        {bundleUpdateStatusLabel(update.status, t)}
      </span>
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
        <span className="bundle-update-action">{actionLabel}</span>
      ) : null}
      {update.message ? (
        <small
          title={`${localize(
            update.message,
            "无法读取最新更新状态。",
          )}${checkedAt}`}
        >
          {localize(update.message, "无法读取最新更新状态。")}
        </small>
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
