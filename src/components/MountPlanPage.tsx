import type { MountPlan, SupportedAppId } from "../domain";
import { PageBackButton } from "./PageBackButton";

interface MountPlanPageProps {
  plan: MountPlan;
  isConfirming: boolean;
  onBack(): void;
  onConfirm(): void;
}

export function MountPlanPage({
  plan,
  isConfirming,
  onBack,
  onConfirm,
}: MountPlanPageProps) {
  const isCreate = plan.operation === "create";
  const action =
    plan.purpose === "repair" ? "修复" : isCreate ? "创建" : "移除";
  const appName = supportedAppLabel(plan.appId);

  return (
    <main className="mount-shell">
      <PageBackButton disabled={isConfirming} onClick={onBack} />
      <p className="eyebrow">SKILLYARD · CONFIRM MOUNT</p>
      <h1>{`确认${action} ${appName} 挂载`}</h1>
      <section className="install-plan" aria-label="挂载影响预览">
        <PlanRow label="Skill" value={plan.skillName} />
        <PlanRow label="应用" value={appName} />
        <PlanRow label="位置" value={mountDestinationLabel(plan)} />
        <PlanRow label="Mount 路径" value={plan.targetPath} isCode />
        <PlanRow label="指向" value={plan.expectedTarget} isCode />
        <div className="install-mount-note">
          <strong>
            {isCreate
              ? createImpactCopy(plan)
              : removeImpactCopy(plan)}
          </strong>
          <span>
            {isCreate
              ? plan.purpose === "repair"
                ? "修复只重建正确软链接，不会修改 Skill 或 Bundle。"
                : plan.targetHealth === "healthy"
                ? "现有软链接不会被改写；SkillYard 只补充这条使用关系。"
                : "Skill 内容仍只有一份，Bundle 更新后这里会继续使用最新 Current Content。"
              : "移除挂载不会删除 Skill 或 Bundle，也不会影响其他使用位置。"}
          </span>
        </div>
        {plan.appId === "claudeCode" && plan.scope === "project" ? (
          <p className="mount-project-hint">
            这个位置位于 <code>.claude/skills</code>，GitHub Copilot
            也可能读取这里的 Skill。
          </p>
        ) : null}
      </section>

      <p className="mount-confirm-warning">
        确认开始后不能取消。SkillYard 会完成或自动恢复这次高保证操作。
      </p>
      <div className="install-actions">
        <button
          className="primary-action"
          type="button"
          disabled={isConfirming}
          onClick={onConfirm}
        >
          {isConfirming ? `正在安全${action}…` : `确认${action}`}
        </button>
      </div>
    </main>
  );
}

function PlanRow({
  label,
  value,
  isCode = false,
}: {
  label: string;
  value: string;
  isCode?: boolean;
}) {
  return (
    <div className="install-plan-row">
      <span>{label}</span>
      {isCode ? <code title={value}>{value}</code> : <strong>{value}</strong>}
    </div>
  );
}

function mountDestinationLabel(plan: MountPlan): string {
  const appName = supportedAppLabel(plan.appId);
  return plan.scope === "global"
    ? `${appName} 全局`
    : `${appName} 项目 ${plan.projectDisplayName ?? "已登记项目"}`;
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}

function createImpactCopy(plan: MountPlan): string {
  if (plan.purpose === "repair") {
    return plan.targetHealth === "healthy"
      ? "软链接已经恢复，将只校正 Mount 状态"
      : "将重新创建指向中央主副本的软链接";
  }
  return plan.targetHealth === "healthy"
    ? "软链接已经正确存在，将只登记为 SkillYard Mount"
    : "将创建一个指向中央主副本的软链接";
}

function removeImpactCopy(plan: MountPlan): string {
  if (plan.targetHealth === "missing") {
    return "Mount 已缺失，将只清理 SkillYard 记录";
  }
  if (plan.targetHealth === "conflict") {
    return "目标已被其他内容占用，将保留该内容并只清理记录";
  }
  return "将移除这个由 SkillYard 管理的软链接";
}
