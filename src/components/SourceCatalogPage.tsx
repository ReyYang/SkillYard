import { useState, type FormEvent } from "react";

import type {
  MountSummary,
  SkillsShSearchSource,
  SourceSummary,
  UiOutcome,
} from "../domain";

type SourceDiscoveryOutcome = Extract<UiOutcome, { type: "sourceDiscovery" }>;

interface SourceCatalogPageProps {
  outcome: SourceDiscoveryOutcome;
  skillsShSearch: Extract<UiOutcome, { type: "skillsShSearch" }> | null;
  mounts: MountSummary[];
  operation:
    | {
        type:
          | "opening"
          | "adding"
          | "choosingFolder"
          | "confirmingRef"
          | "searchingSkillsSh";
      }
    | { type: "reloading" | "planningInstall"; sourceId: string }
    | null;
  error: string | null;
  onBack(): void;
  onAddSource(input: string, trackedRef: string | null): void;
  onSearchSkillsSh(query: string): void;
  onChooseFolder(): void;
  onReload(sourceId: string): void;
  onInstall(sourceId: string): void;
}

export function SourceCatalogPage({
  outcome,
  skillsShSearch,
  mounts,
  operation,
  error,
  onBack,
  onAddSource,
  onSearchSkillsSh,
  onChooseFolder,
  onReload,
  onInstall,
}: SourceCatalogPageProps) {
  const [input, setInput] = useState("");
  const [trackedRef, setTrackedRef] = useState("");
  const [skillsShQuery, setSkillsShQuery] = useState("");
  const isBusy = operation !== null;
  const isAddingSource = operation?.type === "adding";
  const isChoosingFolder = operation?.type === "choosingFolder";
  const isSearchingSkillsSh = operation?.type === "searchingSkillsSh";

  const submitSource = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const normalizedInput = input.trim();
    if (!normalizedInput) return;
    onAddSource(normalizedInput, trackedRef.trim() || null);
  };

  const submitSkillsSh = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const query = skillsShQuery.trim();
    if (query.length < 2) return;
    onSearchSkillsSh(query);
  };

  return (
    <main className="source-shell">
      <header className="source-header">
        <div>
          <p className="eyebrow">SKILLYARD · SOURCE CATALOG</p>
          <h1>安装 Skill</h1>
          <p className="lead">
            Source 是远端仓库，安装后会在本机形成由 SkillYard 管理的 Bundle，默认不会挂载到任何应用。
          </p>
        </div>
        <div className="source-header-actions">
          <button
            className="secondary-action"
            type="button"
            disabled={isBusy}
            onClick={onBack}
          >
            返回清单
          </button>
          <button
            className="secondary-action"
            type="button"
            disabled={isBusy}
            onClick={onChooseFolder}
          >
            {isChoosingFolder ? "正在选择…" : "从本地文件夹安装"}
          </button>
        </div>
      </header>

      <form
        className="source-add-form"
        aria-label="添加 GitHub Source"
        onSubmit={submitSource}
      >
        <label>
          <span>GitHub 仓库</span>
          <input
            value={input}
            disabled={isBusy}
            placeholder="owner/repository 或 GitHub URL"
            onChange={(event) => setInput(event.target.value)}
          />
        </label>
        <label>
          <span>Tracked Ref（可选）</span>
          <input
            value={trackedRef}
            disabled={isBusy}
            placeholder="默认使用仓库默认分支"
            onChange={(event) => setTrackedRef(event.target.value)}
          />
        </label>
        <button
          className="primary-action"
          type="submit"
          disabled={isBusy || !input.trim()}
        >
          {isAddingSource ? "正在验证…" : "添加 Source"}
        </button>
      </form>

      <form
        className="source-add-form"
        aria-label="搜索 skills.sh"
        onSubmit={submitSkillsSh}
      >
        <label>
          <span>搜索 skills.sh</span>
          <input
            type="search"
            value={skillsShQuery}
            disabled={isBusy}
            placeholder="例如 react、testing"
            onChange={(event) => setSkillsShQuery(event.target.value)}
          />
        </label>
        <button
          className="secondary-action"
          type="submit"
          disabled={isBusy || skillsShQuery.trim().length < 2}
        >
          {isSearchingSkillsSh ? "正在搜索…" : "搜索 skills.sh"}
        </button>
      </form>

      {skillsShSearch ? (
        <section className="source-list" aria-label="skills.sh 搜索结果">
          <h2>“{skillsShSearch.query}”的搜索结果</h2>
          {skillsShSearch.sources.map((source) => (
            <SkillsShResultCard
              key={source.sourceInput}
              source={source}
              isBusy={isBusy}
              onAddSource={() => onAddSource(source.sourceInput, null)}
            />
          ))}
        </section>
      ) : null}

      {error ? (
        <div className="inline-error" role="alert">
          <strong>Source 操作未完成</strong>
          <span>{error}</span>
        </div>
      ) : null}

      <section className="source-list" aria-label="已登记 Source">
        {outcome.sources.map((source) => (
          <SourceCard
            key={source.id}
            source={source}
            mounts={mounts}
            highlightedMemberPath={
              outcome.highlightedSourceId === source.id
                ? outcome.highlightedMemberPath
                : null
            }
            isBusy={isBusy}
            isReloading={
              operation?.type === "reloading" &&
              operation.sourceId === source.id
            }
            isPlanning={
              operation?.type === "planningInstall" &&
              operation.sourceId === source.id
            }
            onReload={() => onReload(source.id)}
            onInstall={() => onInstall(source.id)}
          />
        ))}
      </section>
    </main>
  );
}

function SkillsShResultCard({
  source,
  isBusy,
  onAddSource,
}: {
  source: SkillsShSearchSource;
  isBusy: boolean;
  onAddSource(): void;
}) {
  return (
    <article className="source-card" aria-label={source.sourceInput}>
      <header>
        <div>
          <span className="source-status">
            {source.supported
              ? "可添加为 GitHub Source"
              : "当前不是受支持的 GitHub Source"}
          </span>
          <h2>{source.sourceInput}</h2>
        </div>
        {source.supported ? (
          <button
            className="primary-action"
            type="button"
            aria-label={`添加 ${source.sourceInput} Source`}
            disabled={isBusy}
            onClick={onAddSource}
          >
            添加 Source
          </button>
        ) : null}
      </header>
      <ul className="source-members">
        {source.members.map((member) => (
          <li key={member.skillId}>
            <div>
              <strong>{member.name}</strong>
              <code>{member.skillId}</code>
            </div>
            <span>{member.installs.toLocaleString("zh-CN")} 次安装</span>
          </li>
        ))}
      </ul>
    </article>
  );
}

function SourceCard({
  source,
  mounts,
  highlightedMemberPath,
  isBusy,
  isReloading,
  isPlanning,
  onReload,
  onInstall,
}: {
  source: SourceSummary;
  mounts: MountSummary[];
  highlightedMemberPath: string | null;
  isBusy: boolean;
  isReloading: boolean;
  isPlanning: boolean;
  onReload(): void;
  onInstall(): void;
}) {
  const available = source.members.filter(
    (member) => member.selectable && !member.installedMemberId,
  );
  const canInstall = source.catalogStatus === "fresh" && available.length > 0;
  const statusLabel =
    source.catalogStatus === "fresh"
      ? "目录已加载"
      : source.catalogStatus === "stale"
        ? "上次目录已过期"
        : "尚未加载";

  return (
    <article className="source-card" aria-label={source.displayName}>
      <header>
        <div>
          <span className={`source-status is-${source.catalogStatus}`}>
            {statusLabel}
          </span>
          <h2>{source.displayName}</h2>
          <code>{source.repositoryUrl}</code>
          <code>Tracked Ref: {source.trackedRef}</code>
          {source.catalogFetchedAt !== null ? (
            <small className="source-catalog-time">
              上次成功加载：{formatCatalogTime(source.catalogFetchedAt)}
            </small>
          ) : null}
        </div>
        <div className="source-card-actions">
          <button
            className="secondary-action"
            type="button"
            disabled={isBusy}
            onClick={onReload}
          >
            {isReloading ? "正在重新加载…" : "重新加载来源"}
          </button>
          <button
            className="primary-action"
            type="button"
            disabled={isBusy || !canInstall}
            onClick={onInstall}
          >
            {isPlanning
              ? "正在准备…"
              : source.bundleId
                ? "补装 Skill"
                : "安装 Bundle"}
          </button>
        </div>
      </header>

      {source.lastReloadError ? (
        <p className="source-reload-error">
          最近一次加载失败：{source.lastReloadError}
        </p>
      ) : null}

      {source.members.length === 0 ? (
        <p className="source-empty">当前没有发现可展示的 Skill。</p>
      ) : (
        <ul className="source-members">
          {source.members.map((member) => (
            <li
              key={member.id}
              className={
                member.relativePath === highlightedMemberPath
                  ? "is-highlighted"
                  : undefined
              }
            >
              <div>
                <strong>{member.skillName ?? member.relativePath}</strong>
                <code>{member.relativePath}</code>
              </div>
              <span>
                {member.installedMemberId
                  ? installedMemberStatus(mounts, member.installedMemberId)
                  : !member.selectable
                    ? "不可安装"
                    : source.catalogStatus === "fresh"
                      ? "可安装"
                      : "等待重新加载"}
              </span>
              {member.validationErrors.map((message) => (
                <small className="candidate-error" key={message}>
                  {message}
                </small>
              ))}
            </li>
          ))}
        </ul>
      )}
      {source.catalogStatus === "fresh" && available.length === 0 ? (
        <p className="source-empty">没有尚未安装的有效 Skill。</p>
      ) : null}
    </article>
  );
}

function installedMemberStatus(mounts: MountSummary[], memberId: string) {
  const memberMounts = mounts.filter(
    (mount) => mount.memberId === memberId,
  );
  if (memberMounts.length === 0) return "已安装 · 未挂载";

  // 缺失或冲突的记录仍需展示，但不能被误报为可正常使用的挂载。
  const abnormalCount = memberMounts.filter(
    (mount) => mount.health !== "healthy",
  ).length;
  if (abnormalCount === 0) {
    return `已安装 · 已挂载 ${memberMounts.length} 处`;
  }
  const healthyCount = memberMounts.length - abnormalCount;
  return healthyCount === 0
    ? `已安装 · 挂载异常 ${abnormalCount} 处`
    : `已安装 · 正常挂载 ${healthyCount} 处 · 异常 ${abnormalCount} 处`;
}

function formatCatalogTime(timestamp: number) {
  return new Intl.DateTimeFormat("zh-CN", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(timestamp));
}
