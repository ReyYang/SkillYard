import { useState } from "react";

import type {
  SourceAssociationContentChoice,
  SourceAssociationPlan,
  SupportedAppId,
} from "../domain";
import { PageBackButton } from "./PageBackButton";

interface SourceAssociationPlanPageProps {
  plan: SourceAssociationPlan;
  isConfirming: boolean;
  isDiscarding: boolean;
  error: string | null;
  onBack(): void;
  onConfirm(choices: SourceAssociationContentChoice[]): void;
}

export function SourceAssociationPlanPage({
  plan,
  isConfirming,
  isDiscarding,
  error,
  onBack,
  onConfirm,
}: SourceAssociationPlanPageProps) {
  const [choiceByConflict, setChoiceByConflict] = useState<
    Record<string, string>
  >({});
  const isBusy = isConfirming || isDiscarding;
  const choicesComplete = plan.conflicts.every(
    (conflict) => choiceByConflict[conflict.id],
  );
  const canConfirm =
    !isBusy && choicesComplete && plan.blockingIssues.length === 0;

  const confirm = () => {
    const choices = plan.conflicts.map((conflict) => ({
      conflictId: conflict.id,
      memberId: choiceByConflict[conflict.id],
    }));
    onConfirm(choices);
  };

  return (
    <main className="association-shell">
      <PageBackButton
        disabled={isBusy}
        label={isDiscarding ? "正在返回…" : "返回"}
        onClick={onBack}
      />
      <header className="association-header">
        <div>
          <p className="eyebrow">SKILLYARD · ASSOCIATION PLAN</p>
          <h1>
            {plan.mode === "link" ? "确认补充来源" : "确认归并 Bundle"}
          </h1>
          <p className="lead">
            {plan.targetBundleDisplayName} → {plan.sourceDisplayName}
          </p>
        </div>
      </header>

      {plan.mode === "link" ? (
        <section className="association-notice" aria-label="关联影响">
          <h2>只建立来源关系</h2>
          <p>
            这次操作不会修改当前内容或 Mount，也不会自动采用 Source
            中的其他 Skill。
          </p>
        </section>
      ) : (
        <section className="association-notice" aria-label="归并影响">
          <h2>两个 Bundle 将归并为一个</h2>
          <p>
            {plan.retiringBundleDisplayName} 将归入{" "}
            {plan.targetBundleDisplayName}，全部 Mount
            最终使用下面选择的唯一内容。
          </p>
        </section>
      )}

      <section className="association-plan-section" aria-label="成员关系">
        <h2>本地 Skill</h2>
        <ul>
          {plan.members.map((member) => {
            const mapping = plan.memberChoices.find(
              (choice) => choice.memberId === member.memberId,
            );
            return (
              <li key={member.memberId}>
                <div>
                  <strong>
                    {member.bundleDisplayName} · {member.skillName}
                  </strong>
                  <span>
                    {mapping?.sourceRelativePath === null || !mapping
                      ? "不对应"
                      : `对应 ${mapping.sourceRelativePath || "来源根目录"}`}
                  </span>
                </div>
              </li>
            );
          })}
        </ul>
      </section>

      {plan.mounts.length > 0 ? (
        <section className="association-plan-section" aria-label="受影响 Mount">
          <h2>Mount</h2>
          <ul>
            {plan.mounts.map((mount) => (
              <li key={mount.id}>
                <div>
                  <strong>{mount.skillName}</strong>
                  <span>
                    {supportedAppLabel(mount.appId)} ·{" "}
                    {mount.scope === "global"
                      ? "全局"
                      : mount.projectDisplayName ?? "已登记项目"}
                  </span>
                </div>
                <code>{mount.targetPath}</code>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      {plan.conflicts.length > 0 ? (
        <section className="association-conflicts" aria-label="内容冲突">
          <h2>选择唯一内容</h2>
          {plan.conflicts.map((conflict) => (
            <fieldset key={conflict.id}>
              <legend>{conflict.label}</legend>
              {conflict.candidateMemberIds.map((memberId) => {
                const member = plan.members.find(
                  (candidate) => candidate.memberId === memberId,
                );
                if (!member) return null;
                return (
                  <label key={memberId}>
                    <input
                      type="radio"
                      name={conflict.id}
                      value={memberId}
                      checked={choiceByConflict[conflict.id] === memberId}
                      disabled={isBusy}
                      onChange={() =>
                        setChoiceByConflict((current) => ({
                          ...current,
                          [conflict.id]: memberId,
                        }))
                      }
                    />
                    <span>
                      {conflictMemberRole(plan, member)} ·{" "}
                      {member.bundleDisplayName} · {member.skillName} · 内容{" "}
                      <code>{shortFingerprint(member.contentFingerprint)}</code>
                    </span>
                  </label>
                );
              })}
            </fieldset>
          ))}
        </section>
      ) : null}

      {plan.blockingIssues.length > 0 ? (
        <section className="association-blocked" role="alert">
          <h2>需要先处理冲突</h2>
          <ul>
            {plan.blockingIssues.map((issue) => (
              <li key={issue}>{issue}</li>
            ))}
          </ul>
        </section>
      ) : null}

      {error ? (
        <div className="inline-error" role="alert">
          <strong>来源操作未完成</strong>
          <span>{error}</span>
        </div>
      ) : null}

      <footer className="plan-actions">
        <button
          className="primary-action"
          type="button"
          disabled={!canConfirm}
          onClick={confirm}
        >
          {isConfirming
            ? "正在安全处理…"
            : plan.mode === "link"
              ? "确认关联"
              : "确认归并"}
        </button>
      </footer>
    </main>
  );
}

function supportedAppLabel(app: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[app];
}

function conflictMemberRole(
  plan: SourceAssociationPlan,
  member: SourceAssociationPlan["members"][number],
): string {
  // 角色来自后端封存的 Bundle 身份，不能依赖可能重名的展示名称。
  return member.bundleId === plan.targetBundleId
    ? "保留已关联 Bundle"
    : "使用待归入 Bundle";
}

function shortFingerprint(fingerprint: string): string {
  // 短指纹只用于区分当前内容，不把内部哈希包装成版本号。
  return fingerprint.slice(0, 8);
}
