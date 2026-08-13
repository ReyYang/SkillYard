import { Children, useMemo, useState, type FormEvent, type ReactNode } from "react";

import type {
  DiscoverLocalSkill,
  DiscoverWebResult,
  SourceCatalogMemberSummary,
  SourceSummary,
  UiOutcome,
} from "../domain";
import { useI18n } from "../i18n";
import { PageBackButton } from "./PageBackButton";

type DiscoverOutcome = Extract<UiOutcome, { type: "discover" }>;
type DiscoverWebSearchOutcome = Extract<
  UiOutcome,
  { type: "discoverWebSearch" }
>;

interface DiscoverPageProps {
  outcome: DiscoverOutcome;
  webSearch: DiscoverWebSearchOutcome | null;
  isSearchingWeb: boolean;
  webSearchError: string | null;
  error?: string | null;
  onBack(): void;
  onSearchWeb(query: string): void;
  onOpenExternalUrl(url: string): void;
  onPreviewInstall(result: DiscoverWebResult): Promise<void>;
  onOpenSourceManagement(
    sourceId: string,
    memberRelativePath: string | null,
  ): void;
}

interface SourceMemberMatch {
  source: SourceSummary;
  member: SourceCatalogMemberSummary;
}

interface DiscoverResultGroup {
  key: string;
  title: string;
  localSkills: DiscoverLocalSkill[];
  source: SourceSummary | null;
  sourceMembers: SourceCatalogMemberSummary[];
  webResults: DiscoverWebResult[];
}

export function DiscoverPage({
  outcome,
  webSearch,
  isSearchingWeb,
  webSearchError,
  error = null,
  onBack,
  onSearchWeb,
  onOpenExternalUrl,
  onPreviewInstall,
  onOpenSourceManagement,
}: DiscoverPageProps) {
  const { language, t } = useI18n();
  const [query, setQuery] = useState("");
  const [previewingUrl, setPreviewingUrl] = useState<string | null>(null);
  const normalizedQuery = normalizeQuery(query, language);
  const localMatches = useMemo(
    () =>
      normalizedQuery
        ? outcome.localSkills.filter((skill) =>
            matchesLocalSkill(skill, normalizedQuery, language),
          )
        : [],
    [language, normalizedQuery, outcome.localSkills],
  );
  const sourceMatches = useMemo(
    () =>
      normalizedQuery
        ? outcome.sources.flatMap((source) =>
            source.members
              .filter((member) =>
                matchesSourceMember(member, normalizedQuery, language),
              )
              .map((member) => ({ source, member })),
          )
        : [],
    [language, normalizedQuery, outcome.sources],
  );
  const groups = useMemo(
    () =>
      buildResultGroups(
        outcome,
        localMatches,
        sourceMatches,
        webSearch?.results ?? [],
      ),
    [localMatches, outcome, sourceMatches, webSearch],
  );
  const localGroups = groups.filter((group) => group.localSkills.length > 0);
  const sourceGroups = groups.filter(
    (group) => group.localSkills.length === 0 && group.source !== null,
  );
  const webGroups = groups.filter(
    (group) => group.localSkills.length === 0 && group.source === null,
  );
  const unloadedSources = outcome.sources.filter(
    (source) =>
      source.catalogStatus === "unloaded" &&
      !groups.some((group) => group.source?.id === source.id),
  );

  const submitSearch = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const submitted = query.trim();
    if (!submitted || isSearchingWeb) return;
    onSearchWeb(submitted);
  };

  const previewInstall = async (result: DiscoverWebResult) => {
    if (previewingUrl !== null) return;
    setPreviewingUrl(result.url);
    try {
      await onPreviewInstall(result);
    } finally {
      setPreviewingUrl(null);
    }
  };

  return (
    <main className="discover-shell">
      <PageBackButton onClick={onBack} />
      <header className="discover-header">
        <p className="eyebrow">SKILLYARD · DISCOVER</p>
        <h1>{t("发现 Skill")}</h1>
        <p>
          {t(
            "输入会立即筛选本机与已保存的 Source；只有主动提交才会使用当前 Provider 搜索全网。",
          )}
        </p>
      </header>

      <form className="discover-search" onSubmit={submitSearch}>
        <label>
          <span className="sr-only">{t("搜索 Skill")}</span>
          <input
            type="search"
            value={query}
            aria-label={t("搜索 Skill")}
            placeholder={t("描述你需要的 Skill")}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <button
          type="submit"
          disabled={!normalizedQuery || isSearchingWeb}
        >
          {isSearchingWeb ? t("正在搜索…") : t("搜索全网")}
        </button>
      </form>

      {error ? (
        <p className="inline-error" role="alert">
          {error}
        </p>
      ) : null}

      <div className="discover-regions">
        <DiscoverRegion
          title={t("本机已有")}
          count={localGroups.length}
          empty={
            normalizedQuery
              ? t("本机没有匹配的 Skill")
              : t("输入关键词以筛选本机 Skill")
          }
        >
          {localGroups.map((group) => (
            <DiscoverGroupCard
              group={group}
              key={group.key}
              previewingUrl={previewingUrl}
              onOpenExternalUrl={onOpenExternalUrl}
              onOpenSourceManagement={onOpenSourceManagement}
              onPreviewInstall={previewInstall}
            />
          ))}
        </DiscoverRegion>

        <DiscoverRegion
          title={t("已添加来源")}
          count={sourceGroups.length}
          empty={
            normalizedQuery
              ? t("已加载的 Source 中没有匹配成员")
              : t("输入关键词以筛选已保存的 Source 目录")
          }
        >
          {sourceGroups.map((group) => (
            <DiscoverGroupCard
              group={group}
              key={group.key}
              previewingUrl={previewingUrl}
              onOpenExternalUrl={onOpenExternalUrl}
              onOpenSourceManagement={onOpenSourceManagement}
              onPreviewInstall={previewInstall}
            />
          ))}

          {unloadedSources.map((source) => (
            <article
              className="discover-source-state"
              aria-label={source.displayName}
              key={source.id}
            >
              <div>
                <strong>{source.displayName}</strong>
                <span>{t("目录尚未加载")}</span>
              </div>
              <button
                className="compact-action"
                type="button"
                aria-label={`${t("前往 Source 管理")}：${source.displayName}`}
                onClick={() => onOpenSourceManagement(source.id, null)}
              >
                {t("前往 Source 管理")}
              </button>
            </article>
          ))}
        </DiscoverRegion>

        <DiscoverRegion
          title={t("全网发现")}
          count={webSearch ? webGroups.length : null}
          empty={webEmptyMessage(
            isSearchingWeb,
            webSearch,
            webSearchError,
            t,
          )}
          error={webSearchError}
        >
          {webGroups.map((group) => (
            <DiscoverGroupCard
              group={group}
              key={group.key}
              previewingUrl={previewingUrl}
              onOpenExternalUrl={onOpenExternalUrl}
              onOpenSourceManagement={onOpenSourceManagement}
              onPreviewInstall={previewInstall}
            />
          ))}
        </DiscoverRegion>
      </div>
    </main>
  );
}

function DiscoverGroupCard({
  group,
  previewingUrl,
  onOpenExternalUrl,
  onOpenSourceManagement,
  onPreviewInstall,
}: {
  group: DiscoverResultGroup;
  previewingUrl: string | null;
  onOpenExternalUrl(url: string): void;
  onOpenSourceManagement(
    sourceId: string,
    memberRelativePath: string | null,
  ): void;
  onPreviewInstall(result: DiscoverWebResult): void;
}) {
  const { t } = useI18n();
  const installable = group.webResults.find(
    (result) => result.kind !== "reference",
  );
  const description =
    group.localSkills.find((skill) => skill.aiSummary)?.aiSummary ??
    group.localSkills.find((skill) => skill.description)?.description ??
    group.sourceMembers.find((member) => member.description)?.description;

  return (
    <article
      className="discover-result-card discover-group-card"
      aria-label={group.title}
    >
      <header>
        <div>
          <h3>{group.title}</h3>
          <div className="discover-statuses">
            {group.localSkills.length > 0 ? <span>{t("已经安装")}</span> : null}
            {group.source ? <span>{t("已添加 Source")}</span> : null}
            {group.webResults.length > 0 ? <span>{t("包含全网引用")}</span> : null}
          </div>
        </div>
      </header>

      {description ? <p>{description}</p> : null}

      {group.localSkills.length > 0 ? (
        <ul className="discover-member-list" aria-label={t("本机 Skill")}>
          {group.localSkills.map((skill) => (
            <li key={skill.inventoryId}>
              <strong>{skill.skillName}</strong>
              <span>{t(managementLabel(skill.managementKind))}</span>
            </li>
          ))}
        </ul>
      ) : null}

      {group.sourceMembers.length > 0 ? (
        <ul className="discover-member-list" aria-label={t("Source 成员")}>
          {group.sourceMembers.map((member) => (
            <li key={member.id}>
              <strong>{member.skillName ?? member.relativePath}</strong>
              <span>
                {member.installedMemberId ? t("已经安装") : t("尚未安装")}
              </span>
            </li>
          ))}
        </ul>
      ) : null}

      <div className="discover-card-actions">
        {group.source ? (
          <button
            className="compact-action"
            type="button"
            onClick={() =>
              onOpenSourceManagement(
                group.source!.id,
                group.sourceMembers[0]?.relativePath ?? null,
              )
            }
          >
            {t("查看 Source")}
          </button>
        ) : null}
        {group.webResults.map((result) => (
          <button
            className="discover-reference-link"
            type="button"
            aria-label={t("打开 {title}", { title: result.title })}
            key={result.url}
            onClick={() => onOpenExternalUrl(result.url)}
          >
            {result.title}
          </button>
        ))}
        {installable ? (
          <button
            className="compact-action"
            type="button"
            disabled={previewingUrl !== null}
            aria-label={
              previewingUrl === installable.url
                ? t("正在准备…：{title}", {
                    title: installable.title,
                  })
                : t("查看安装预览：{title}", {
                    title: installable.title,
                  })
            }
            onClick={() => onPreviewInstall(installable)}
          >
            {previewingUrl === installable.url
              ? t("正在准备…")
              : t("查看安装预览")}
          </button>
        ) : null}
      </div>
    </article>
  );
}

function DiscoverRegion({
  title,
  count,
  empty,
  error = null,
  children,
}: {
  title: string;
  count: number | null;
  empty: string;
  error?: string | null;
  children?: ReactNode;
}) {
  const hasResults = Children.count(children) > 0;
  return (
    <section className="discover-region" aria-label={title}>
      <header>
        <h2>{title}</h2>
        {count !== null ? <span>{count}</span> : null}
      </header>
      <div className="discover-region-results">
        {hasResults ? children : (
          <p className={error ? "discover-empty inline-error" : "discover-empty"}>
            {empty}
          </p>
        )}
      </div>
    </section>
  );
}

function buildResultGroups(
  outcome: DiscoverOutcome,
  localMatches: DiscoverLocalSkill[],
  sourceMatches: SourceMemberMatch[],
  webResults: DiscoverWebResult[],
): DiscoverResultGroup[] {
  const groups = new Map<string, DiscoverResultGroup>();
  const ensureGroup = (
    key: string,
    title: string,
  ): DiscoverResultGroup => {
    const existing = groups.get(key);
    if (existing) return existing;
    const created: DiscoverResultGroup = {
      key,
      title,
      localSkills: [],
      source: null,
      sourceMembers: [],
      webResults: [],
    };
    groups.set(key, created);
    return created;
  };

  for (const skill of localMatches) {
    const key = localGroupKey(skill);
    const group = ensureGroup(
      key,
      skill.bundleDisplayName ?? skill.sourceDisplayName ?? skill.skillName,
    );
    addLocalSkill(group, skill);
    if (skill.sourceId) {
      group.source =
        outcome.sources.find((source) => source.id === skill.sourceId) ?? null;
    }
  }

  for (const { source, member } of sourceMatches) {
    const group = ensureGroup(
      sourceGroupKey(source.canonicalIdentity),
      source.displayName,
    );
    group.source = source;
    addSourceMember(group, member);
  }

  for (const result of webResults) {
    const source = result.canonicalIdentity
      ? outcome.sources.find(
          (candidate) =>
            candidate.canonicalIdentity === result.canonicalIdentity,
        ) ?? null
      : null;
    const key = result.canonicalIdentity
      ? sourceGroupKey(result.canonicalIdentity)
      : `web:${result.url}`;
    const group = ensureGroup(
      key,
      source?.displayName ?? result.title,
    );
    group.source = source;
    group.webResults.push(result);

    // Provider 命中已安装来源时，补入对应本机事实，即使自然语言没有逐字命中。
    if (result.canonicalIdentity) {
      for (const skill of outcome.localSkills.filter(
        (candidate) =>
          candidate.sourceCanonicalIdentity === result.canonicalIdentity,
      )) {
        addLocalSkill(group, skill);
      }
    }
  }

  return [...groups.values()];
}

function addLocalSkill(
  group: DiscoverResultGroup,
  skill: DiscoverLocalSkill,
) {
  if (
    !group.localSkills.some(
      (candidate) => candidate.inventoryId === skill.inventoryId,
    )
  ) {
    group.localSkills.push(skill);
  }
}

function addSourceMember(
  group: DiscoverResultGroup,
  member: SourceCatalogMemberSummary,
) {
  if (!group.sourceMembers.some((candidate) => candidate.id === member.id)) {
    group.sourceMembers.push(member);
  }
}

function localGroupKey(skill: DiscoverLocalSkill): string {
  if (skill.sourceCanonicalIdentity) {
    return sourceGroupKey(skill.sourceCanonicalIdentity);
  }
  if (skill.bundleId) return `bundle:${skill.bundleId}`;
  return `local:${skill.inventoryId}`;
}

function sourceGroupKey(canonicalIdentity: string): string {
  return `source:${canonicalIdentity}`;
}

function matchesLocalSkill(
  skill: DiscoverLocalSkill,
  query: string,
  language: "zhCn" | "en",
): boolean {
  return [
    skill.skillName,
    skill.description,
    skill.aiSummary,
    skill.bundleDisplayName,
    skill.sourceDisplayName,
  ].some((value) => value && normalizeQuery(value, language).includes(query));
}

function matchesSourceMember(
  member: SourceCatalogMemberSummary,
  query: string,
  language: "zhCn" | "en",
): boolean {
  return [member.skillName, member.description, member.relativePath].some(
    (value) => value && normalizeQuery(value, language).includes(query),
  );
}

function normalizeQuery(value: string, language: "zhCn" | "en"): string {
  return value
    .trim()
    .toLocaleLowerCase(language === "zhCn" ? "zh-CN" : "en");
}

function webEmptyMessage(
  isSearching: boolean,
  webSearch: DiscoverWebSearchOutcome | null,
  error: string | null,
  t: ReturnType<typeof useI18n>["t"],
): string {
  if (isSearching) return t("正在搜索公开互联网…");
  if (error) return error;
  if (!webSearch) return t("尚未提交全网搜索");
  if (webSearch.results.length === 0) return t("全网没有返回可核验结果");
  return t("线上结果已合并到本机或 Source");
}

function managementLabel(
  kind: DiscoverLocalSkill["managementKind"],
):
  | "由 SkillYard 管理"
  | "待接管"
  | "其他管理方"
  | "项目仓库管理" {
  switch (kind) {
    case "skillYardManaged":
      return "由 SkillYard 管理";
    case "takeoverCandidate":
      return "待接管";
    case "agentManaged":
      return "其他管理方";
    case "projectManaged":
      return "项目仓库管理";
  }
}
