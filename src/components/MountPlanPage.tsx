import type { MountPlan, SupportedAppId } from "../domain";
import { useI18n } from "../i18n";
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
  const { t } = useI18n();
  const isCreate = plan.operation === "create";
  const action =
    plan.purpose === "repair"
      ? t("修复")
      : isCreate
        ? t("创建")
        : t("移除");
  const appName = supportedAppLabel(plan.appId);

  return (
    <main className="mount-shell">
      <PageBackButton disabled={isConfirming} onClick={onBack} />
      <p className="eyebrow">SKILLYARD · CONFIRM MOUNT</p>
      <h1>{t("确认{action} {app} 挂载", { action, app: appName })}</h1>
      <section className="install-plan" aria-label={t("挂载影响预览")}>
        <PlanRow label="Skill" value={plan.skillName} />
        <PlanRow label={t("应用")} value={appName} />
        <PlanRow label={t("位置")} value={mountDestinationLabel(plan, t)} />
        <PlanRow label={t("Mount 路径")} value={plan.targetPath} isCode />
        <PlanRow label={t("指向")} value={plan.expectedTarget} isCode />
        <div className="install-mount-note">
          <strong>
            {isCreate
              ? createImpactCopy(plan, t)
              : removeImpactCopy(plan, t)}
          </strong>
          <span>
            {isCreate
              ? plan.purpose === "repair"
                ? t("修复只重建正确软链接，不会修改 Skill 或 Bundle。")
                : plan.targetHealth === "healthy"
                  ? t("现有软链接不会被改写；SkillYard 只补充这条使用关系。")
                  : t(
                      "Skill 内容仍只有一份，Bundle 更新后这里会继续使用最新 Current Content。",
                    )
              : t("移除挂载不会删除 Skill 或 Bundle，也不会影响其他使用位置。")}
          </span>
        </div>
        {plan.appId === "claudeCode" && plan.scope === "project" ? (
          <p className="mount-project-hint">
            {t(
              "这个位置位于 .claude/skills，GitHub Copilot 也可能读取这里的 Skill。",
            )}
          </p>
        ) : null}
      </section>

      <p className="mount-confirm-warning">
        {t("确认开始后不能取消。SkillYard 会完成或自动恢复这次高保证操作。")}
      </p>
      <div className="install-actions">
        <button
          className="primary-action"
          type="button"
          disabled={isConfirming}
          onClick={onConfirm}
        >
          {isConfirming
            ? t("正在安全{action}…", { action })
            : t("确认{action}", { action })}
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

function mountDestinationLabel(
  plan: MountPlan,
  t: ReturnType<typeof useI18n>["t"],
): string {
  const appName = supportedAppLabel(plan.appId);
  return plan.scope === "global"
    ? t("{app} 全局", { app: appName })
    : t("{app} 项目 {project}", {
        app: appName,
        project: plan.projectDisplayName ?? t("已登记项目"),
      });
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}

function createImpactCopy(
  plan: MountPlan,
  t: ReturnType<typeof useI18n>["t"],
): string {
  if (plan.purpose === "repair") {
    return plan.targetHealth === "healthy"
      ? t("软链接已经恢复，将只校正 Mount 状态")
      : t("将重新创建指向中央主副本的软链接");
  }
  return plan.targetHealth === "healthy"
    ? t("软链接已经正确存在，将只登记为 SkillYard Mount")
    : t("将创建一个指向中央主副本的软链接");
}

function removeImpactCopy(
  plan: MountPlan,
  t: ReturnType<typeof useI18n>["t"],
): string {
  if (plan.targetHealth === "missing") {
    return t("Mount 已缺失，将只清理 SkillYard 记录");
  }
  if (plan.targetHealth === "conflict") {
    return t("目标已被其他内容占用，将保留该内容并只清理记录");
  }
  return t("将移除这个由 SkillYard 管理的软链接");
}
