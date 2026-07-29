import { useMemo, useState } from "react";

import type {
  InventoryObservation,
  SourceCatalogMemberSummary,
  SourceMemberMappingChoice,
  SourceSummary,
} from "../domain";
import { useI18n } from "../i18n";
import { PageBackButton } from "./PageBackButton";

const NO_SOURCE_MEMBER = "__skillyard_none__";

interface SourceAssociationSelectionPageProps {
  bundleDisplayName: string;
  bundleId: string;
  members: InventoryObservation[];
  sources: SourceSummary[];
  isPlanning: boolean;
  error: string | null;
  onBack(): void;
  onAddSource(): void;
  onCreatePlan(
    bundleId: string,
    sourceId: string,
    memberChoices: SourceMemberMappingChoice[],
  ): void;
}

export function SourceAssociationSelectionPage({
  bundleDisplayName,
  bundleId,
  members,
  sources,
  isPlanning,
  error,
  onBack,
  onAddSource,
  onCreatePlan,
}: SourceAssociationSelectionPageProps) {
  const { language, t } = useI18n();
  const freshSources = useMemo(
    () =>
      sources
        .filter((source) => source.catalogStatus === "fresh")
        .sort((left, right) =>
          left.displayName.localeCompare(
            right.displayName,
            language === "zhCn" ? "zh-CN" : "en",
          ),
        ),
    [language, sources],
  );
  const [sourceId, setSourceId] = useState("");
  const [mappingByMember, setMappingByMember] = useState<
    Record<string, string | null>
  >({});
  const source = freshSources.find((candidate) => candidate.id === sourceId);
  const selectableMembers = (source?.members ?? []).filter(
    (member) => member.selectable,
  );
  const localMembers = [...members].sort(
    (left, right) =>
      left.skillName.localeCompare(
        right.skillName,
        language === "zhCn" ? "zh-CN" : "en",
      ) ||
      (left.memberId ?? "").localeCompare(right.memberId ?? ""),
  );

  const changeSource = (nextSourceId: string) => {
    setSourceId(nextSourceId);
    // Source 改变后旧路径已经没有意义，不能把它带进新的关联请求。
    setMappingByMember({});
  };

  const changeMapping = (memberId: string, selection: string) => {
    setMappingByMember((current) => ({
      ...current,
      // 空字符串是 Source 根目录的合法路径，只有 sentinel 表示“不对应”。
      [memberId]: selection === NO_SOURCE_MEMBER ? null : selection,
    }));
  };

  const createPlan = () => {
    if (!source) return;
    const memberChoices = localMembers.flatMap((member) =>
      member.memberId
        ? [
            {
              memberId: member.memberId,
              sourceRelativePath: mappingByMember[member.memberId] ?? null,
            },
          ]
        : [],
    );
    onCreatePlan(bundleId, source.id, memberChoices);
  };

  return (
    <main className="association-shell">
      <PageBackButton disabled={isPlanning} onClick={onBack} />
      <header className="association-header">
        <div>
          <p className="eyebrow">SKILLYARD · SOURCE ASSOCIATION</p>
          <h1>{t("为 Bundle 补充来源")}</h1>
          <p className="lead">
            {t(
              "为 {bundle} 选择一个已经登记的 Source，再明确每个本地 Skill 是否对应其中的成员。",
              { bundle: bundleDisplayName },
            )}
          </p>
        </div>
      </header>

      {freshSources.length === 0 ? (
        <section className="association-empty">
          <h2>{t("没有可选择的 Source")}</h2>
          <p>
            {t("先在现有 Source 页面添加或重新加载来源，再回来补充关系。")}
          </p>
          <button
            className="primary-action"
            type="button"
            onClick={onAddSource}
          >
            {t("前往 Source 页面添加")}
          </button>
        </section>
      ) : (
        <>
          <label className="association-source-field">
            <span>{t("选择 Source")}</span>
            <select
              aria-label={t("选择 Source")}
              value={sourceId}
              disabled={isPlanning}
              onChange={(event) => changeSource(event.target.value)}
            >
              <option value="">{t("请选择")}</option>
              {freshSources.map((candidate) => (
                <option key={candidate.id} value={candidate.id}>
                  {candidate.displayName}
                </option>
              ))}
            </select>
          </label>

          {source ? (
            <section
              className="association-mapping"
              aria-label={t("Skill 对应关系")}
            >
              <header>
                <div>
                  <p className="section-eyebrow">MEMBER MAPPING</p>
                  <h2>{t("逐个确认对应关系")}</h2>
                </div>
                <span>
                  {source.bundleId
                    ? t("此 Source 已有关联 Bundle")
                    : t("可直接关联")}
                </span>
              </header>
              <p>
                {t(
                  "找不到对应成员时保持“不对应”。SkillYard 不会根据名称或内容自动猜测。",
                )}
              </p>
              <div className="association-mapping-list">
                {localMembers.map((member) => {
                  if (!member.memberId) return null;
                  const selectedElsewhere = new Set(
                    Object.entries(mappingByMember)
                      .filter(
                        ([otherMemberId, path]) =>
                          otherMemberId !== member.memberId && path !== null,
                      )
                      .map(([, path]) => path as string),
                  );
                  return (
                    <label key={member.memberId}>
                      <span>{member.skillName}</span>
                      <select
                        aria-label={t("{skill} 的对应关系", {
                          skill: member.skillName,
                        })}
                        value={
                          mappingByMember[member.memberId] ?? NO_SOURCE_MEMBER
                        }
                        disabled={isPlanning}
                        onChange={(event) =>
                          changeMapping(member.memberId!, event.target.value)
                        }
                      >
                        <option value={NO_SOURCE_MEMBER}>
                          {t("不对应")}
                        </option>
                        {selectableMembers.map((candidate) => (
                          <option
                            key={candidate.relativePath}
                            value={candidate.relativePath}
                            disabled={selectedElsewhere.has(
                              candidate.relativePath,
                            )}
                          >
                            {sourceMemberLabel(candidate, t)}
                          </option>
                        ))}
                      </select>
                    </label>
                  );
                })}
              </div>
              <button
                className="primary-action"
                type="button"
                disabled={isPlanning}
                onClick={createPlan}
              >
                {isPlanning ? t("正在生成计划…") : t("生成关联计划")}
              </button>
            </section>
          ) : null}
        </>
      )}

      {error ? (
        <div className="inline-error" role="alert">
          <strong>{t("无法生成关联计划")}</strong>
          <span>{error}</span>
        </div>
      ) : null}
    </main>
  );
}

function sourceMemberLabel(
  member: SourceCatalogMemberSummary,
  t: ReturnType<typeof useI18n>["t"],
): string {
  const pathLabel = member.relativePath || t("来源根目录");
  return member.skillName && member.skillName !== member.relativePath
    ? `${member.skillName} · ${pathLabel}`
    : pathLabel;
}
