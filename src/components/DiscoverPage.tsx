import { Children, useMemo, useState, type ReactNode } from "react";

import type {
  DiscoverLocalSkill,
  SourceCatalogMemberSummary,
  UiOutcome,
} from "../domain";
import { useI18n } from "../i18n";
import { PageBackButton } from "./PageBackButton";

type DiscoverOutcome = Extract<UiOutcome, { type: "discover" }>;

interface DiscoverPageProps {
  outcome: DiscoverOutcome;
  error?: string | null;
  onBack(): void;
  onOpenSourceManagement(
    sourceId: string,
    memberRelativePath: string | null,
  ): void;
}

export function DiscoverPage({
  outcome,
  error = null,
  onBack,
  onOpenSourceManagement,
}: DiscoverPageProps) {
  const { language, t } = useI18n();
  const [query, setQuery] = useState("");
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
  const unloadedSources = outcome.sources.filter(
    (source) => source.catalogStatus === "unloaded",
  );

  return (
    <main className="discover-shell">
      <PageBackButton onClick={onBack} />
      <header className="discover-header">
        <p className="eyebrow">SKILLYARD · DISCOVER</p>
        <h1>{t("发现 Skill")}</h1>
        <p>
          {t(
            "输入时只筛选本机与已保存的 Source 目录，不会自动联网或修改任何内容。",
          )}
        </p>
      </header>

      <label className="discover-search">
        <span className="sr-only">{t("搜索 Skill")}</span>
        <input
          type="search"
          value={query}
          aria-label={t("搜索 Skill")}
          placeholder={t("描述你需要的 Skill")}
          onChange={(event) => setQuery(event.target.value)}
        />
      </label>

      {error ? (
        <p className="inline-error" role="alert">
          {error}
        </p>
      ) : null}

      <div className="discover-regions">
        <DiscoverRegion
          title={t("本机已有")}
          count={localMatches.length}
          empty={
            normalizedQuery
              ? t("本机没有匹配的 Skill")
              : t("输入关键词以筛选本机 Skill")
          }
        >
          {localMatches.map((skill) => (
            <article
              className="discover-result-card"
              aria-label={skill.skillName}
              key={skill.inventoryId}
            >
              <header>
                <div>
                  <h3>{skill.skillName}</h3>
                  <span>{t(managementLabel(skill.managementKind))}</span>
                </div>
                {skill.bundleDisplayName ? (
                  <small>{skill.bundleDisplayName}</small>
                ) : null}
              </header>
              {skill.description ? <p>{skill.description}</p> : null}
              {skill.aiSummary ? (
                <p className="discover-ai-summary">{skill.aiSummary}</p>
              ) : null}
            </article>
          ))}
        </DiscoverRegion>

        <DiscoverRegion
          title={t("已添加来源")}
          count={sourceMatches.length}
          empty={
            normalizedQuery
              ? t("已加载的 Source 中没有匹配成员")
              : t("输入关键词以筛选已保存的 Source 目录")
          }
        >
          {sourceMatches.map(({ source, member }) => (
            <article
              className="discover-result-card"
              aria-label={member.skillName ?? member.relativePath}
              key={`${source.id}:${member.id}`}
            >
              <header>
                <div>
                  <h3>{member.skillName ?? member.relativePath}</h3>
                  <span>
                    {member.installedMemberId
                      ? t("已经安装")
                      : t("尚未安装")}
                  </span>
                </div>
                <small>{source.displayName}</small>
              </header>
              {member.description ? <p>{member.description}</p> : null}
              <button
                className="compact-action"
                type="button"
                onClick={() =>
                  onOpenSourceManagement(source.id, member.relativePath)
                }
              >
                {t("查看 Source")}
              </button>
            </article>
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
                aria-label={t("管理 {source}", {
                  source: source.displayName,
                })}
                onClick={() => onOpenSourceManagement(source.id, null)}
              >
                {t("前往 Source 管理")}
              </button>
            </article>
          ))}
        </DiscoverRegion>

        <DiscoverRegion
          title={t("全网发现")}
          count={null}
          empty={t("尚未提交全网搜索")}
        />
      </div>
    </main>
  );
}

function DiscoverRegion({
  title,
  count,
  empty,
  children,
}: {
  title: string;
  count: number | null;
  empty: string;
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
        {hasResults ? children : <p className="discover-empty">{empty}</p>}
      </div>
    </section>
  );
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
