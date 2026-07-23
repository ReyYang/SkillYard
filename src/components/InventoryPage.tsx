import { useMemo, useState } from "react";

import type {
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
  isOpeningInstaller: boolean;
  isAddingProject: boolean;
  refreshError: string | null;
  installError: string | null;
  projectError: string | null;
  mountError: string | null;
  takeoverError: string | null;
  onRefresh(): void;
  onInstall(): void;
  onAddProject(): void;
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
  isOpeningInstaller,
  isAddingProject,
  refreshError,
  installError,
  projectError,
  mountError,
  takeoverError,
  onRefresh,
  onInstall,
  onAddProject,
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
        </div>
      </header>

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
  entries: InventoryObservation[];
}> {
  const groups = new Map<
    string,
    {
      id: string;
      bundleId: string | null;
      title: string;
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
