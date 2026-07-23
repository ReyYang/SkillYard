import { useState } from "react";

import type { InstallPlan } from "../domain";

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
  const [selectedCandidateIds, setSelectedCandidateIds] = useState<string[]>(
    plan.candidates
      .filter((candidate) => candidate.selectable && candidate.defaultSelected)
      .map((candidate) => candidate.candidateId),
  );
  const selectableCount = plan.candidates.filter(
    (candidate) => candidate.selectable,
  ).length;
  const hasPartialSelection =
    selectedCandidateIds.length > 0 &&
    selectedCandidateIds.length < selectableCount;
  const isBusy = isInstalling || isDiscarding;
  const isSourceBacked = plan.inputKind !== "localFolder";

  const toggleCandidate = (candidateId: string) => {
    setSelectedCandidateIds((current) =>
      current.includes(candidateId)
        ? current.filter((id) => id !== candidateId)
        : [...current, candidateId],
    );
  };

  return (
    <main className="install-shell">
      <p className="eyebrow">SKILLYARD · INSTALL PLAN</p>
      <h1>确认安装这个 Bundle</h1>
      <p className="lead">
        {plan.mode === "supplement"
          ? "确认后只新增当前未安装的 Skill；已有 Skill 内容和 Mount 不会被覆盖。"
          : isSourceBacked
            ? "确认后，SkillYard 会采用刚刚验证的内容快照；原文件、目录或远端内容不会被移动或改写。"
            : "确认后，SkillYard 会把所选文件夹复制到自己的 Central Store。原文件夹不会被移动或修改。"}
        安装开始后不能取消；如果应用意外退出，下次启动会自动恢复。
      </p>

      <section className="install-plan" aria-label="安装影响预览">
        <PlanRow label="Bundle" value={plan.bundleDisplayName} />
        <PlanRow
          label={isSourceBacked ? "Source" : "原文件夹"}
          value={plan.inputPath}
          code
        />
        <div className="install-candidates" aria-label="Bundle 中的 Skill">
          {plan.candidates.map((candidate) => {
            const pathParts = candidate.sourceRelativePath
              .split("/")
              .filter(Boolean);
            const displayName =
              candidate.skillName ??
              pathParts[pathParts.length - 1] ??
              "无法识别的 Skill";
            return (
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
                <span className="install-candidate-copy">
                  <strong>{displayName}</strong>
                  <code>
                    {candidate.sourceRelativePath || "所选 Bundle 根目录"}
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
              </label>
            );
          })}
        </div>
        {hasPartialSelection ? (
          <div className="install-selection-warning" role="alert">
            部分 Skill 可能依赖同一 Bundle 中未选择的其他 Skill。SkillYard 1.0
            不检查这种依赖。
          </div>
        ) : null}
        {selectedCandidateIds.length === 0 ? (
          <p className="install-selection-empty">至少选择一个有效 Skill 才能安装。</p>
        ) : null}
        <div className="install-mount-note">
          <strong>安装后不会自动挂载</strong>
          <span>稍后由你选择 Codex、Claude Code 或 GitHub Copilot。</span>
        </div>
        {plan.warnings.length > 0 ? (
          <ul className="install-warnings" aria-label="安装提示">
            {plan.warnings.map((warning) => (
              <li key={warning}>{warning}</li>
            ))}
          </ul>
        ) : null}
      </section>

      {error ? (
        <div className="inline-error" role="alert">
          <strong>安装未完成</strong>
          <span>{error}</span>
        </div>
      ) : null}

      <div className="install-actions">
        <button
          className="secondary-action"
          type="button"
          disabled={isBusy}
          onClick={onCancel}
        >
          {isDiscarding ? "正在返回…" : "返回"}
        </button>
        <button
          className="primary-action"
          type="button"
          disabled={isBusy || error !== null || selectedCandidateIds.length === 0}
          onClick={() => onConfirm(selectedCandidateIds)}
        >
          {isInstalling ? "正在安全安装…" : "确认安装"}
        </button>
      </div>
    </main>
  );
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
