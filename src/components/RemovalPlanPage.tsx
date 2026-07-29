import { useEffect, useState } from "react";

import type {
  MountSummary,
  RemovalPlan,
  SourceKind,
  SupportedAppId,
} from "../domain";
import { useI18n } from "../i18n";
import { PageBackButton } from "./PageBackButton";

interface RemovalPlanPageProps {
  plan: RemovalPlan;
  isConfirming: boolean;
  isDiscarding: boolean;
  error: string | null;
  onDiscard(planId: string): void;
  onConfirm(planId: string): void;
}

export function RemovalPlanPage({
  plan,
  isConfirming,
  isDiscarding,
  error,
  onDiscard,
  onConfirm,
}: RemovalPlanPageProps) {
  const { localize, t } = useI18n();
  const [isBundleDangerConfirmed, setIsBundleDangerConfirmed] = useState(false);
  const isBusy = isConfirming || isDiscarding;

  useEffect(() => {
    // 确认失败后重新读取 Plan 时必须回到第一步，不能保留旧页面的危险确认。
    if (!isConfirming) setIsBundleDangerConfirmed(false);
  }, [isConfirming]);

  return (
    <main className="removal-shell">
      <PageBackButton
        disabled={isBusy}
        label={isDiscarding ? t("正在清理预览…") : t("返回")}
        onClick={() => onDiscard(plan.id)}
      />
      <p className="eyebrow">SKILLYARD · REMOVAL PLAN</p>
      <h1>{removalHeading(plan, t)}</h1>
      <p className="lead">{removalLead(plan, t)}</p>

      {plan.kind === "bundle" && plan.members.length > 0 ? (
        <section className="removal-section" aria-label={t("将删除的 Skill")}>
          <h2>{t("将永久删除的 Skill")}</h2>
          <ul className="removal-list">
            {plan.members.map((member) => (
              <li key={member.id}>
                <strong>{member.skillName}</strong>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      {plan.mounts.length > 0 ? (
        <section className="removal-section" aria-label={t("将移除的 Mount")}>
          <h2>{t("将移除的 Mount")}</h2>
          <ul className="removal-list">
            {plan.mounts.map((mount) => (
              <MountImpact key={mount.id} mount={mount} />
            ))}
          </ul>
        </section>
      ) : null}

      {plan.kind === "source" ? (
        <section
          className="removal-section"
          aria-label={t("失去更新来源的 Bundle")}
        >
          <h2>{t("将失去更新来源的 Bundle")}</h2>
          {plan.affectedBundles.length === 0 ? (
            <p>{t("当前没有关联的本地 Bundle。")}</p>
          ) : (
            <ul className="removal-list">
              {plan.affectedBundles.map((bundle) => (
                <li key={bundle.id}>
                  <strong>{bundle.displayName}</strong>
                </li>
              ))}
            </ul>
          )}
        </section>
      ) : null}

      {plan.kind === "bundle" && plan.managedDirectory ? (
        <section className="removal-section is-destructive">
          <h2>{t("将永久删除的受管目录")}</h2>
          <code title={plan.managedDirectory}>{plan.managedDirectory}</code>
        </section>
      ) : null}

      <PreservedContent plan={plan} />

      {plan.warnings.length > 0 ? (
        <section className="removal-warnings" aria-label={t("删除警告")}>
          <strong>{t("操作提示")}</strong>
          <ul>
            {plan.warnings.map((warning) => (
              <li key={warning}>
                {localize(warning, "请在继续前检查此提示。")}
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      {plan.kind === "project" ? (
        <p className="removal-safety-copy">
          {t("不会删除 Bundle 或 Skill，也不会删除项目目录中的未知内容。")}
        </p>
      ) : null}
      {plan.kind === "bundleMounts" ? (
        <p className="removal-safety-copy">
          {t("Bundle、全部 Skill、Source 和 current 内容都会保留，之后仍可重新挂载。")}
        </p>
      ) : null}
      {plan.kind === "source" ? (
        <div className="removal-safety-copy">
          <p>{t("本地 Bundle、current 内容和 Mount 都会保留。")}</p>
          <p>{t("Editable Local 原目录不会被删除。")}</p>
        </div>
      ) : null}
      {plan.kind === "bundle" ? (
        <p className="removal-safety-copy">
          {t("保留的 Source 和外部路径不属于删除目标；1.0 不提供成员级删除。")}
        </p>
      ) : null}

      {plan.kind === "bundle" && isBundleDangerConfirmed ? (
        <div className="removal-danger-confirmation" role="alert">
          <strong>{t("这是永久删除")}</strong>
          <p>
            {t(
              "确认后将级联删除上面列出的受管内容和 Mount，成功后没有回滚入口。",
            )}
          </p>
        </div>
      ) : null}

      {error ? (
        <div className="inline-error" role="alert">
          <strong>{t("移除操作未完成")}</strong>
          <span>{error}</span>
        </div>
      ) : null}

      <div className="install-actions">
        {plan.kind === "bundle" && !isBundleDangerConfirmed ? (
          <button
            className="danger-action"
            type="button"
            disabled={isBusy}
            onClick={() => setIsBundleDangerConfirmed(true)}
          >
            {t("继续删除")}
          </button>
        ) : (
          <button
            className={
              plan.kind === "bundle" ? "danger-action" : "primary-action"
            }
            type="button"
            disabled={isBusy}
            onClick={() => onConfirm(plan.id)}
          >
            {isConfirming ? t("正在执行…") : removalConfirmLabel(plan, t)}
          </button>
        )}
      </div>
    </main>
  );
}

function PreservedContent({ plan }: { plan: RemovalPlan }) {
  const { t } = useI18n();
  const hasPreservedContent =
    plan.preservedSource !== null || plan.preservedExternalPaths.length > 0;
  if (!hasPreservedContent) return null;

  return (
    <section
      className="removal-section is-preserved"
      aria-label={t("将保留的内容")}
    >
      <h2>{t("将保留的内容")}</h2>
      {plan.preservedSource ? (
        <div className="removal-preserved-source">
          <strong>{plan.preservedSource.displayName}</strong>
          <span>{sourceKindLabel(plan.preservedSource.kind, t)} Source</span>
          <code title={plan.preservedSource.locator}>
            {plan.preservedSource.locator}
          </code>
        </div>
      ) : null}
      {plan.preservedExternalPaths.length > 0 ? (
        <ul className="removal-list">
          {plan.preservedExternalPaths.map((path) => (
            <li key={path}>
              <code title={path}>{path}</code>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}

function MountImpact({ mount }: { mount: MountSummary }) {
  const { t } = useI18n();
  return (
    <li>
      <strong>{mount.skillName}</strong>
      <span>{mountLabel(mount, t)}</span>
      <code title={mount.targetPath}>{mount.targetPath}</code>
    </li>
  );
}

function removalHeading(
  plan: RemovalPlan,
  t: ReturnType<typeof useI18n>["t"],
): string {
  if (plan.kind === "project") {
    return t("移除项目 {project}", { project: plan.targetDisplayName });
  }
  if (plan.kind === "source") {
    return t("删除 Source {source}", { source: plan.targetDisplayName });
  }
  if (plan.kind === "bundleMounts") {
    return t("解除 {bundle} 的全部挂载", {
      bundle: plan.targetDisplayName,
    });
  }
  return t("删除 Bundle {bundle}", { bundle: plan.targetDisplayName });
}

function removalLead(
  plan: RemovalPlan,
  t: ReturnType<typeof useI18n>["t"],
): string {
  if (plan.kind === "project") {
    return t("移除这个已登记项目及其中全部 SkillYard-managed project Mount。");
  }
  if (plan.kind === "source") {
    return t("删除 SkillYard 保存的 Source、目录状态、检查结果和更新关联。");
  }
  if (plan.kind === "bundleMounts") {
    return t("从所有 Agent 应用和项目中解除这个 Bundle 的 Mount。");
  }
  return t("删除整个本地受管 Bundle，而不是删除其中某一个 Skill。");
}

function removalConfirmLabel(
  plan: RemovalPlan,
  t: ReturnType<typeof useI18n>["t"],
): string {
  if (plan.kind === "project") return t("确认移除项目");
  if (plan.kind === "source") return t("确认删除 Source");
  if (plan.kind === "bundleMounts") return t("确认解除全部挂载");
  return t("确认永久删除");
}

function mountLabel(
  mount: MountSummary,
  t: ReturnType<typeof useI18n>["t"],
): string {
  const appName = supportedAppLabel(mount.appId);
  return mount.scope === "global"
    ? t("{app} · 全局", { app: appName })
    : t("{app} · 项目 · {project}", {
        app: appName,
        project: mount.projectDisplayName ?? t("已登记项目"),
      });
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}

function sourceKindLabel(
  kind: SourceKind,
  t: ReturnType<typeof useI18n>["t"],
): string {
  return {
    github: "GitHub",
    archive: t("归档"),
    directUrl: t("直接 URL"),
    editableLocal: "Editable Local",
  }[kind];
}
