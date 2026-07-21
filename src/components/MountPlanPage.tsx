import type { MountPlan } from "../domain";

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
  const action = isCreate ? "创建" : "移除";

  return (
    <main className="mount-shell">
      <p className="eyebrow">SKILLYARD · CONFIRM MOUNT</p>
      <h1>{`确认${action} Codex 挂载`}</h1>
      <section className="install-plan" aria-label="挂载影响预览">
        <PlanRow label="Skill" value={plan.skillName} />
        <PlanRow label="应用" value="Codex" />
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
              ? plan.targetHealth === "healthy"
                ? "现有软链接不会被改写；SkillYard 只补充这条使用关系。"
                : "Skill 内容仍只有一份，Bundle 更新后这里会继续使用最新 Current Content。"
              : "移除挂载不会删除 Skill 或 Bundle，也不会影响其他使用位置。"}
          </span>
        </div>
      </section>

      <p className="mount-confirm-warning">
        确认开始后不能取消。SkillYard 会完成或自动恢复这次高保证操作。
      </p>
      <div className="install-actions">
        <button
          className="secondary-action"
          type="button"
          disabled={isConfirming}
          onClick={onBack}
        >
          返回
        </button>
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
  return plan.scope === "global"
    ? "Codex 全局"
    : `Codex 项目 ${plan.projectDisplayName ?? "已登记项目"}`;
}

function createImpactCopy(plan: MountPlan): string {
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
