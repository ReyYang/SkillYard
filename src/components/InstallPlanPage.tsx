import { useState } from "react";

import type { InstallPlan, SupportedAppId } from "../domain";
import { useI18n } from "../i18n";
import { PageBackButton } from "./PageBackButton";

interface InstallPlanPageProps {
  plan: InstallPlan;
  isInstalling: boolean;
  isDiscarding: boolean;
  error: string | null;
  onCancel(): void;
  onConfirm(selectedCandidateIds: string[]): void;
}

export function InstallPlanPage({
  plan,
  isInstalling,
  isDiscarding,
  error,
  onCancel,
  onConfirm,
}: InstallPlanPageProps) {
  const { t } = useI18n();
  const isUpdate = plan.mode === "update";
  const [selectedCandidateIds, setSelectedCandidateIds] = useState<string[]>(
    plan.candidates
      .filter((candidate) => candidate.selectable && candidate.defaultSelected)
      .map((candidate) => candidate.candidateId),
  );
  // 更新没有成员选择：Source 当前目录中的全部成员必须一起提交给同一安装事务。
  const confirmedCandidateIds = isUpdate
    ? plan.candidates.map((candidate) => candidate.candidateId)
    : selectedCandidateIds;
  const selectableCount = plan.candidates.filter(
    (candidate) => candidate.selectable,
  ).length;
  const hasPartialSelection =
    !isUpdate &&
    selectedCandidateIds.length > 0 &&
    selectedCandidateIds.length < selectableCount;
  const isBusy = isInstalling || isDiscarding;
  const isSourceBacked = plan.inputKind !== "localFolder";
  const updateHasInvalidCandidate =
    isUpdate &&
    plan.candidates.some(
      (candidate) =>
        !candidate.selectable || candidate.validationErrors.length > 0,
    );

  const toggleCandidate = (candidateId: string) => {
    setSelectedCandidateIds((current) =>
      current.includes(candidateId)
        ? current.filter((id) => id !== candidateId)
        : [...current, candidateId],
    );
  };

  return (
    <main className="install-shell">
      <PageBackButton
        disabled={isBusy}
        label={isDiscarding ? t("正在返回…") : t("返回")}
        onClick={onCancel}
      />
      <p className="eyebrow">
        SKILLYARD · {isUpdate ? "UPDATE PLAN" : "INSTALL PLAN"}
      </p>
      <h1>
        {isUpdate ? t("确认更新这个 Bundle") : t("确认安装这个 Bundle")}
      </h1>
      <p className="lead">
        {isUpdate
          ? t(
              "确认后，SkillYard 会把来源当前的全部有效 Skill 一次性更新到这个 Bundle。更新开始后不能取消；如果应用意外退出，下次启动会自动恢复。",
            )
          : plan.mode === "supplement"
            ? t(
                "确认后只新增当前未安装的 Skill；已有 Skill 内容和 Mount 不会被覆盖。安装开始后不能取消；如果应用意外退出，下次启动会自动恢复。",
              )
          : isSourceBacked
            ? t(
                "确认后，SkillYard 会采用刚刚验证的内容快照；原文件、目录或远端内容不会被移动或改写。安装开始后不能取消；如果应用意外退出，下次启动会自动恢复。",
              )
            : t(
                "确认后，SkillYard 会把所选文件夹复制到自己的 Central Store。原文件夹不会被移动或修改。安装开始后不能取消；如果应用意外退出，下次启动会自动恢复。",
              )}
      </p>

      <section
        className="install-plan"
        aria-label={
          isUpdate ? t("更新影响预览") : t("安装影响预览")
        }
      >
        <PlanRow label="Bundle" value={plan.bundleDisplayName} />
        {isUpdate && plan.updateImpact?.upstreamUrl ? (
          <SafeUpstreamLink url={plan.updateImpact.upstreamUrl} />
        ) : (
          <PlanRow
            label={isSourceBacked ? "Source" : t("原文件夹")}
            value={plan.inputPath}
            code
          />
        )}
        <div
          className="install-candidates"
          aria-label={t("Bundle 中的 Skill")}
        >
          {plan.candidates.map((candidate) => {
            const pathParts = candidate.sourceRelativePath
              .split("/")
              .filter(Boolean);
            const displayName =
              candidate.skillName ??
              pathParts[pathParts.length - 1] ??
              t("无法识别的 Skill");
            const details = (
              <span className="install-candidate-copy">
                <span className="install-candidate-heading">
                  <strong>{displayName}</strong>
                  {isUpdate &&
                  plan.updateImpact?.newCandidateIds.includes(
                    candidate.candidateId,
                  ) ? (
                    <span className="candidate-new">{t("新增安装")}</span>
                  ) : null}
                </span>
                <code>
                  {candidate.sourceRelativePath || t("所选 Bundle 根目录")}
                </code>
                {candidate.targetDirectory ? (
                  <code title={candidate.targetDirectory}>
                    → {candidate.targetDirectory}
                  </code>
                ) : null}
                {candidate.validationErrors.map((message) => (
                  <span className="candidate-error" key={message}>
                    {message}
                  </span>
                ))}
                {candidate.warnings.map((warning) => (
                  <span className="candidate-warning" key={warning}>
                    {warning}
                  </span>
                ))}
              </span>
            );
            return isUpdate ? (
              <div
                className="install-candidate is-readonly"
                key={candidate.candidateId}
              >
                {details}
              </div>
            ) : (
              <label
                className={`install-candidate${candidate.selectable ? "" : " is-invalid"}`}
                key={candidate.candidateId}
              >
                <input
                  type="checkbox"
                  checked={selectedCandidateIds.includes(candidate.candidateId)}
                  disabled={isBusy || !candidate.selectable}
                  onChange={() => toggleCandidate(candidate.candidateId)}
                />
                {details}
              </label>
            );
          })}
        </div>
        {hasPartialSelection ? (
          <div className="install-selection-warning" role="alert">
            {t(
              "部分 Skill 可能依赖同一 Bundle 中未选择的其他 Skill。SkillYard 1.0 不检查这种依赖。",
            )}
          </div>
        ) : null}
        {!isUpdate && selectedCandidateIds.length === 0 ? (
          <p className="install-selection-empty">
            {t("至少选择一个有效 Skill 才能安装。")}
          </p>
        ) : null}
        {isUpdate ? (
          <>
            <div className="install-selection-warning">
              {t(
                "更新会一次性替换整个 Bundle 的当前内容；SkillYard 1.0 不保留旧版用于回滚。",
              )}
            </div>
            <ExistingMounts mounts={plan.updateImpact?.existingMounts ?? []} />
            <div className="install-mount-note">
              <strong>{t("现有挂载继续使用")}</strong>
              <span>
                {t("新增 Skill 保持未挂载，更新后可再选择 Agent 应用。")}
              </span>
            </div>
          </>
        ) : (
          <div className="install-mount-note">
            <strong>{t("安装后不会自动挂载")}</strong>
            <span>
              {t("稍后由你选择 Codex、Claude Code 或 GitHub Copilot。")}
            </span>
          </div>
        )}
        {plan.warnings.length > 0 ? (
          <ul className="install-warnings" aria-label={t("安装提示")}>
            {plan.warnings.map((warning) => (
              <li key={warning}>{warning}</li>
            ))}
          </ul>
        ) : null}
      </section>

      {error ? (
        <div className="inline-error" role="alert">
          <strong>
            {isUpdate ? t("更新未完成") : t("安装未完成")}
          </strong>
          <span>{error}</span>
        </div>
      ) : null}

      <div className="install-actions">
        <button
          className="primary-action"
          type="button"
          disabled={
            isBusy ||
            error !== null ||
            confirmedCandidateIds.length === 0 ||
            updateHasInvalidCandidate
          }
          onClick={() => onConfirm(confirmedCandidateIds)}
        >
          {isInstalling
            ? isUpdate
              ? t("正在安全更新…")
              : t("正在安全安装…")
            : isUpdate
              ? t("确认更新")
              : t("确认安装")}
        </button>
      </div>
    </main>
  );
}

function SafeUpstreamLink({ url }: { url: string }) {
  const { t } = useI18n();
  if (!isSafeHttpsUrl(url)) {
    return <PlanRow label="Source" value={t("上游地址不可用")} />;
  }
  return (
    <div className="install-plan-row">
      <span>Source</span>
      <a href={url} target="_blank" rel="noreferrer">
        {t("查看上游发布页")}
      </a>
    </div>
  );
}

function ExistingMounts({
  mounts,
}: {
  mounts: NonNullable<InstallPlan["updateImpact"]>["existingMounts"];
}) {
  const { t } = useI18n();
  if (mounts.length === 0) return null;
  return (
    <div className="update-mounts" aria-label={t("现有挂载")}>
      <strong>{t("现有挂载")}</strong>
      <ul>
        {mounts.map((mount) => (
          <li key={mount.id}>
            <span>{mount.skillName}</span>
            <span>
              {supportedAppLabel(mount.appId)} ·{" "}
              {mount.scope === "global"
                ? t("全局")
                : t("项目 · {project}", {
                    project:
                      mount.projectDisplayName ?? t("已登记项目"),
                  })}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}

function isSafeHttpsUrl(value: string): boolean {
  try {
    return new URL(value).protocol === "https:";
  } catch {
    return false;
  }
}

function PlanRow({
  label,
  value,
  code = false,
}: {
  label: string;
  value: string;
  code?: boolean;
}) {
  return (
    <div className="install-plan-row">
      <span>{label}</span>
      {code ? <code title={value}>{value}</code> : <strong>{value}</strong>}
    </div>
  );
}
