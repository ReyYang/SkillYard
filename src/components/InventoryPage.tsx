import { useMemo, useState } from "react";

import type {
  BundleUpdateAction,
  BundleUpdateSummary,
  BundleUpdateStatus,
  InventoryObservation,
  MountSummary,
  SupportedAppId,
  UiOutcome,
} from "../domain";

type InventoryOutcome = Extract<UiOutcome, { type: "inventory" }>;
type ManagementFilter = "all" | "managed" | "takeover" | "other";

interface InventoryPageProps {
  outcome: InventoryOutcome;
  isWriteBlocked: boolean;
  isRefreshing: boolean;
  isCheckingUpdates: boolean;
  preparingBundleUpdateId: string | null;
  checkingEditableBundleId: string | null;
  isPreparingBundleUpdateBatch: boolean;
  removingBundleId: string | null;
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
  onUpdateBundle(bundleId: string): void;
  onChooseBundleReplacement(bundleId: string): void;
  onCheckEditableLocalBundle(bundleId: string): void;
  onUpdateAll(): void;
  onRemoveBundle(bundleId: string): void;
  onRemoveProject(projectId: string): void;
  onInstall(): void;
  onAddProject(): void;
  onOpenCentralStore(): void;
  onResetApplication(): void;
  onAssociateSource(bundleId: string): void;
  onOpenRecovery(issueId: string): void;
  onTakeover(observationId: string): void;
  onManageMount(memberId: string): void;
  onBatchMount(bundleId: string): void;
}

const FILTERS: Array<{ id: ManagementFilter; label: string }> = [
  { id: "all", label: "全部" },
  { id: "managed", label: "由 SkillYard 管理" },
  { id: "takeover", label: "待接管" },
  { id: "other", label: "其他管理方" },
];

export function InventoryPage({
  outcome,
  isWriteBlocked,
  isRefreshing,
  isCheckingUpdates,
  preparingBundleUpdateId,
  checkingEditableBundleId,
  isPreparingBundleUpdateBatch,
  removingBundleId,
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
  onUpdateBundle,
  onChooseBundleReplacement,
  onCheckEditableLocalBundle,
  onUpdateAll,
  onRemoveBundle,
  onRemoveProject,
  onInstall,
  onAddProject,
  onOpenCentralStore,
  onResetApplication,
  onAssociateSource,
  onOpenRecovery,
  onTakeover,
  onManageMount,
  onBatchMount,
}: InventoryPageProps) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<ManagementFilter>("all");

  // 搜索与筛选只操作已经加载的 read model，不能触发 IPC 或写回 SQLite。
  const visibleEntries = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase("zh-CN");
    return outcome.entries
      .filter((entry) => matchesFilter(entry, filter))
      .filter((entry) => matchesQuery(entry, normalizedQuery))
      .sort((left, right) =>
        presentationLabel(left).localeCompare(presentationLabel(right), "zh-CN"),
      );
  }, [filter, outcome.entries, query]);

  const managedGroups = useMemo(
    () => groupManagedEntries(visibleEntries),
    [visibleEntries],
  );
  const takeoverEntries = visibleEntries.filter(
    (entry) => entry.managementKind === "takeoverCandidate",
  );
  const agentEntries = visibleEntries.filter(
    (entry) => entry.managementKind === "agentManaged",
  );
  const projectEntries = visibleEntries.filter(
    (entry) => entry.managementKind === "projectManaged",
  );
  const hasVisibleEntries = visibleEntries.length > 0;
  const updatableBundleCount = useMemo(
    () =>
      new Set(
        outcome.bundleUpdates
          .filter((update) => update.action === "update")
          .map((update) => update.bundleId),
      ).size,
    [outcome.bundleUpdates],
  );

  return (
    <main className="inventory-shell">
      <header className="inventory-header">
        <div>
          <p className="eyebrow">SKILLYARD · LOCAL INVENTORY</p>
          <h1>Skill 清单</h1>
          <p className="inventory-summary">
            {outcome.entries.length === 0
              ? "本机暂未发现 Skill"
              : `本机已有 ${outcome.entries.length} 个 Skill`}
          </p>
        </div>
        <div className="inventory-actions">
          <button
            className="secondary-action"
            type="button"
            disabled={isWriteBlocked || isCheckingUpdates}
            onClick={onCheckUpdates}
          >
            {isCheckingUpdates ? "正在检查更新…" : "检查更新"}
          </button>
          {updatableBundleCount >= 2 ? (
            <button
              className="secondary-action"
              type="button"
              aria-label="全部更新"
              disabled={isWriteBlocked || isPreparingBundleUpdateBatch}
              onClick={onUpdateAll}
            >
              {isPreparingBundleUpdateBatch
                ? "正在准备全部更新…"
                : "全部更新"}
            </button>
          ) : null}
          <button
            className="primary-action"
            type="button"
            disabled={isWriteBlocked || isOpeningInstaller}
            onClick={onInstall}
          >
            {isOpeningInstaller ? "正在加载来源…" : "安装 Skill"}
          </button>
          <button
            className="secondary-action"
            type="button"
            disabled={isWriteBlocked || isAddingProject}
            onClick={onAddProject}
          >
            {isAddingProject ? "正在选择项目…" : "添加项目"}
          </button>
          <button
            className="secondary-action"
            type="button"
            disabled={isWriteBlocked || isRefreshing}
            onClick={onRefresh}
          >
            {isRefreshing ? "正在刷新本机…" : "刷新本机"}
          </button>
          <button
            className="secondary-action"
            type="button"
            disabled={isWriteBlocked || isResettingApplication}
            onClick={onResetApplication}
          >
            {isResettingApplication ? "正在重置…" : "重置应用"}
          </button>
          <button
            className="secondary-action"
            type="button"
            disabled={isOpeningCentralStore}
            onClick={onOpenCentralStore}
          >
            {isOpeningCentralStore ? "正在打开…" : "打开 Central Store"}
          </button>
        </div>
      </header>

      {outcome.projects.length > 0 ? (
        <section className="registered-projects" aria-label="已登记项目">
          <header>
            <div>
              <p className="section-eyebrow">REGISTERED PROJECTS</p>
              <h2>已登记项目</h2>
            </div>
            <span>{outcome.projects.length}</span>
          </header>
          <p>
            移除项目会先清理其中全部 SkillYard-managed project Mount，再删除登记记录。
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
                  aria-label={`移除项目 ${project.displayName}`}
                  disabled={isWriteBlocked}
                  onClick={() => onRemoveProject(project.id)}
                >
                  {removingProjectId === project.id
                    ? "正在准备移除…"
                    : "移除项目"}
                </button>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      {outcome.recoveryIssues.length > 0 ? (
        <section className="recovery-warning" aria-label="需要人工恢复">
          <p className="section-eyebrow">FILESYSTEM RECOVERY</p>
          <h2>需要人工恢复</h2>
          <p>
            SkillYard 无法安全判断下面操作的最终状态，因此只停止修改相关 Bundle。其他
            Skill 和只读清单仍可正常使用。
          </p>
          <ul>
            {outcome.recoveryIssues.map((issue) => (
              <li key={issue.id}>
                <strong>{issue.bundleDisplayName}</strong>
                <span>{issue.message}</span>
                <button
                  className="compact-action"
                  type="button"
                  aria-label={`查看 ${issue.bundleDisplayName} 的恢复说明`}
                  disabled={isWriteBlocked}
                  onClick={() => onOpenRecovery(issue.id)}
                >
                  查看说明
                </button>
              </li>
            ))}
          </ul>
          <p>请保留 Central Store 中的现有内容，不要手动删除相关目录。</p>
        </section>
      ) : null}

      <section className="inventory-controls" aria-label="清单筛选">
        <label className="search-field">
          <span className="sr-only">搜索 Skill</span>
          <input
            type="search"
            value={query}
            placeholder="搜索 Skill"
            aria-label="搜索 Skill"
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <div className="filter-group" aria-label="管理状态">
          {FILTERS.map((item) => (
            <button
              key={item.id}
              type="button"
              aria-pressed={filter === item.id}
              onClick={() => setFilter(item.id)}
            >
              {item.label}
            </button>
          ))}
        </div>
      </section>

      {refreshError ? (
        <div className="inline-error" role="alert">
          <strong>刷新未完成</strong>
          <span>{refreshError}</span>
        </div>
      ) : null}

      {updateError ? (
        <div className="inline-error" role="alert">
          <strong>更新未完成</strong>
          <span>{updateError}</span>
        </div>
      ) : null}

      {installError ? (
        <div className="inline-error" role="alert">
          <strong>无法准备安装</strong>
          <span>{installError}</span>
        </div>
      ) : null}

      {projectError ? (
        <div className="inline-error" role="alert">
          <strong>无法添加项目</strong>
          <span>{projectError}</span>
        </div>
      ) : null}

      {removalError ? (
        <div className="inline-error" role="alert">
          <strong>移除操作未完成</strong>
          <span>{removalError}</span>
        </div>
      ) : null}

      {mountError ? (
        <div className="inline-error" role="alert">
          <strong>挂载操作未完成</strong>
          <span>{mountError}</span>
        </div>
      ) : null}

      {takeoverError ? (
        <div className="inline-error" role="alert">
          <strong>接管未完成</strong>
          <span>{takeoverError}</span>
        </div>
      ) : null}

      {sourceAssociationError ? (
        <div className="inline-error" role="alert">
          <strong>来源操作未完成</strong>
          <span>{sourceAssociationError}</span>
        </div>
      ) : null}

      {resetError ? (
        <div className="inline-error" role="alert">
          <strong>重置未完成</strong>
          <span>{resetError}</span>
        </div>
      ) : null}

      {centralStoreError ? (
        <div className="inline-error" role="alert">
          <strong>无法打开 Central Store</strong>
          <span>{centralStoreError}</span>
        </div>
      ) : null}

      {outcome.scanIssues.length > 0 ? (
        <section className="scan-warning" aria-label="刷新告警">
          <strong>部分目录暂时无法读取</strong>
          <p>这些目录继续显示上次成功结果，SkillYard 没有把它们当作已删除。</p>
          <ul>
            {outcome.scanIssues.map((issue) => (
              <li key={issue.rootId}>
                <code>{issue.path}</code>
                <span>{issue.message}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      {outcome.lastLocalRefresh ? (
        <p className="refresh-summary" aria-label="最近刷新结果">
          最近刷新：新增 {outcome.lastLocalRefresh.added} · 变化{" "}
          {outcome.lastLocalRefresh.changed} · 移除 {outcome.lastLocalRefresh.removed}
          <span>{formatTimestamp(outcome.lastLocalRefresh.completedAt)}</span>
        </p>
      ) : (
        <p className="refresh-summary">尚未执行本机刷新</p>
      )}

      <div className="inventory-content">
        {managedGroups.map((group) => (
          <InventorySection
            key={group.id}
            title={group.title}
            eyebrow="由 SkillYard 管理 · BUNDLE"
            entries={group.entries}
            mounts={outcome.mounts}
            actionsDisabled={isWriteBlocked}
            onManageMount={onManageMount}
            batchMountBundleId={group.bundleId}
            onBatchMount={onBatchMount}
            canAssociateSource={!group.hasSource}
            onAssociateSource={onAssociateSource}
            bundleUpdate={
              group.bundleId
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
            removingBundleId={removingBundleId}
            onRemoveBundle={onRemoveBundle}
          />
        ))}
        <InventorySection
          title="待接管"
          eyebrow="本机已有 · 只读"
          entries={takeoverEntries}
          actionsDisabled={isWriteBlocked}
          onTakeover={onTakeover}
        />
        <InventorySection
          title="Agent 应用管理"
          eyebrow="交回原管理方"
          entries={agentEntries}
          actionsDisabled={isWriteBlocked}
        />
        <InventorySection
          title="项目仓库管理"
          eyebrow="交回项目仓库"
          entries={projectEntries}
          actionsDisabled={isWriteBlocked}
        />
        {!hasVisibleEntries ? (
          <section className="empty-inventory">
            <h2>{outcome.entries.length === 0 ? "未发现 Skill" : "没有匹配结果"}</h2>
            <p>
              {outcome.entries.length === 0
                ? "你可以继续使用现有安装方式，再主动刷新本机。"
                : "换一个关键词或管理状态看看。"}
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
  mounts = [],
  actionsDisabled = false,
  onManageMount,
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
  onTakeover,
}: {
  title: string;
  eyebrow: string;
  entries: InventoryObservation[];
  mounts?: MountSummary[];
  actionsDisabled?: boolean;
  onManageMount?(memberId: string): void;
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
  onTakeover?(observationId: string): void;
}) {
  if (entries.length === 0) return null;
  return (
    <section className="inventory-section" aria-label={title}>
      <header>
        <div>
          <p className="section-eyebrow">{eyebrow}</p>
          <h2>{title}</h2>
        </div>
        <div className="inventory-section-actions">
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
              补充来源
            </button>
          ) : null}
          {batchMountBundleId ? (
            <button
              className="compact-action"
              type="button"
              disabled={actionsDisabled}
              onClick={() => onBatchMount?.(batchMountBundleId)}
            >
              批量挂载
            </button>
          ) : null}
          {batchMountBundleId ? (
            <button
              className="danger-outline-action"
              type="button"
              aria-label={`删除 Bundle ${title}`}
              disabled={actionsDisabled}
              onClick={() => onRemoveBundle?.(batchMountBundleId)}
            >
              {removingBundleId === batchMountBundleId
                ? "正在准备删除…"
                : "删除 Bundle"}
            </button>
          ) : null}
          <span>{entries.length}</span>
        </div>
      </header>
      <ul className="inventory-list">
        {entries.map((entry) => (
          <SkillCard
            key={entry.id}
            entry={entry}
            mounts={mounts.filter(
              (mount) => mount.memberId === entry.memberId,
            )}
            actionsDisabled={actionsDisabled}
            onManageMount={onManageMount}
            onTakeover={onTakeover}
          />
        ))}
      </ul>
    </section>
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
  const actionLabel = bundleUpdateActionLabel(update.status, update.action);
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
    ? ` · ${formatTimestamp(update.checkedAt)}`
    : "";
  return (
    <div
      className="bundle-update-summary"
      aria-label={`Bundle 更新状态：${bundleUpdateStatusLabel(update.status)}`}
    >
      <span className={`bundle-update-status is-${update.status}`}>
        {bundleUpdateStatusLabel(update.status)}
      </span>
      {actionLabel && actionHandler ? (
        <button
          className="bundle-update-action"
          type="button"
          aria-label={`${actionLabel} ${bundleDisplayName}`}
          disabled={actionsDisabled}
          onClick={actionHandler}
        >
          {actionBusy ? bundleUpdateBusyLabel(update.action) : actionLabel}
        </button>
      ) : actionLabel ? (
        <span className="bundle-update-action">{actionLabel}</span>
      ) : null}
      {update.message ? (
        <small title={`${update.message}${checkedAt}`}>{update.message}</small>
      ) : null}
    </div>
  );
}

function bundleUpdateStatusLabel(status: BundleUpdateStatus): string {
  return {
    noSource: "没有更新来源",
    notChecked: "尚未检查",
    available: "可更新",
    upToDate: "已是最新",
    unableToCheck: "无法检查",
    manual: "手动更新",
    sourceUnavailable: "来源不可用",
  }[status];
}

function bundleUpdateActionLabel(
  status: BundleUpdateStatus,
  action: BundleUpdateAction,
): string | null {
  if (action === "update") return "更新";
  if (action === "importReplacement") return "导入新内容";
  if (action === "checkEditableLocal") {
    if (status === "upToDate") return "再次检查";
    if (status === "sourceUnavailable" || status === "unableToCheck") {
      return "重新检查";
    }
    return "检查本地改动";
  }
  return null;
}

function bundleUpdateBusyLabel(action: BundleUpdateAction): string {
  if (action === "importReplacement") return "正在选择新内容…";
  if (action === "checkEditableLocal") return "正在检查本地改动…";
  return "正在准备…";
}

function SkillCard({
  entry,
  mounts,
  actionsDisabled,
  onManageMount,
  onTakeover,
}: {
  entry: InventoryObservation;
  mounts: MountSummary[];
  actionsDisabled: boolean;
  onManageMount?(memberId: string): void;
  onTakeover?(observationId: string): void;
}) {
  return (
    <li className="skill-card">
      <div className="skill-card-heading">
        <div>
          <strong>{presentationLabel(entry)}</strong>
          <span className={`management-badge ${entry.managementKind}`}>
            {managementLabel(entry.managementKind)}
          </span>
        </div>
        {entry.stale ? <span className="stale-badge">上次结果</span> : null}
      </div>
      <code title={entry.skillRoot}>{entry.skillRoot}</code>
      <div className="skill-meta">
        <span>{entry.sourceDisplayName ?? "来源未知"}</span>
        {entry.observedBy.map((app) => (
          <span key={app}>{supportedAppLabel(app)}</span>
        ))}
        {entry.projectDisplayName ? <span>{entry.projectDisplayName}</span> : null}
        {entry.metadataStatus !== "valid" ? <span>Skill metadata 无效</span> : null}
      </div>
      {managementDirection(entry) ? (
        <p className="management-direction">{managementDirection(entry)}</p>
      ) : null}
      {entry.managementKind === "skillYardManaged" && entry.memberId ? (
        <div className="mount-card-controls">
          <div className="mount-badges" aria-label="当前挂载">
            {mounts.length > 0 ? (
              mounts.map((mount) => (
                <span key={mount.id} className={`mount-badge ${mount.health}`}>
                  {mountLabel(mount)}
                  {mount.health === "healthy"
                    ? ""
                    : ` · ${mountHealthLabel(mount.health)}`}
                </span>
              ))
            ) : (
              <span className="mount-empty">未挂载</span>
            )}
          </div>
          <button
            className="compact-action"
            type="button"
            disabled={actionsDisabled}
            onClick={() => onManageMount?.(entry.memberId!)}
          >
            管理挂载
          </button>
        </div>
      ) : null}
      {entry.managementKind === "takeoverCandidate" ? (
        <div className="mount-card-controls">
          <span className="mount-empty">确认后才会移动或替换文件</span>
          <button
            className="compact-action"
            type="button"
            disabled={actionsDisabled}
            aria-label={`接管 ${presentationLabel(entry)}`}
            onClick={() => onTakeover?.(entry.id)}
          >
            接管
          </button>
        </div>
      ) : null}
    </li>
  );
}

function mountLabel(mount: MountSummary): string {
  const appName = supportedAppLabel(mount.appId);
  return mount.scope === "global"
    ? `${appName} · 全局`
    : `${appName} · ${mount.projectDisplayName ?? "已登记项目"}`;
}

function mountHealthLabel(health: MountSummary["health"]): string {
  return {
    healthy: "正常",
    missing: "已缺失",
    conflict: "路径冲突",
  }[health];
}

function groupManagedEntries(
  entries: InventoryObservation[],
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
      title: entry.bundleDisplayName ?? "本地 Bundle",
      hasSource:
        (existing?.hasSource ?? false) || Boolean(entry.sourceDisplayName),
      entries: [...(existing?.entries ?? []), entry],
    });
  }
  return [...groups.values()].sort((left, right) =>
    left.title.localeCompare(right.title, "zh-CN") || left.id.localeCompare(right.id),
  );
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
    entry.sourceDisplayName,
    entry.projectDisplayName,
    ...entry.observedBy.map(supportedAppLabel),
  ]
    .filter((value): value is string => Boolean(value))
    .some((value) => value.toLocaleLowerCase("zh-CN").includes(query));
}

function presentationLabel(entry: InventoryObservation): string {
  return entry.managementKind === "skillYardManaged" && entry.bundleDisplayName
    ? `${entry.bundleDisplayName}: ${entry.skillName}`
    : entry.skillName;
}

function managementLabel(kind: InventoryObservation["managementKind"]): string {
  return {
    skillYardManaged: "由 SkillYard 管理",
    takeoverCandidate: "待接管",
    agentManaged: "Agent 应用管理",
    projectManaged: "项目仓库管理",
  }[kind];
}

function managementDirection(entry: InventoryObservation): string | null {
  if (entry.managementKind === "agentManaged") {
    const apps = entry.observedBy.map(supportedAppLabel).join("、");
    return `请前往 ${apps || "对应 Agent 应用"} 管理此 Skill。`;
  }
  if (entry.managementKind === "projectManaged") {
    return `请在 ${entry.projectDisplayName ?? "对应项目仓库"} 中管理此 Skill。`;
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

function formatTimestamp(timestamp: number): string {
  return new Intl.DateTimeFormat("zh-CN", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(timestamp));
}
