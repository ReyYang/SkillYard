import type {
  InstallationChain,
  SupportedAppId,
  TakeoverPlan,
  TakeoverPlanOrigin,
  TakeoverPlanTarget,
} from "../domain";
import { useI18n } from "../i18n";
import { PageBackButton } from "./PageBackButton";

interface TakeoverPlanPageProps {
  plan: TakeoverPlan;
  isConfirming: boolean;
  onBack(): void;
  onConfirm(): void;
}

export function TakeoverPlanPage({
  plan,
  isConfirming,
  onBack,
  onConfirm,
}: TakeoverPlanPageProps) {
  const { t } = useI18n();
  const retainedMounts = plan.retainedMembers.flatMap((member) =>
    member.mounts.map((mount) => ({ member, mount })),
  );

  return (
    <main className="mount-shell">
      <PageBackButton disabled={isConfirming} onClick={onBack} />
      <p className="eyebrow">SKILLYARD · CONFIRM TAKEOVER</p>
      <h1>
        {t("确认接管 Bundle：{bundle}", {
          bundle: plan.bundleDisplayName,
        })}
      </h1>
      <p className="lead">
        {t(
          "下面是 Rust 根据当前文件状态封存的完整影响。确认开始后不能取消；如果应用意外退出，下次启动会继续恢复到一致状态。",
        )}
      </p>

      <section className="install-plan" aria-label={t("接管影响预览")}>
        <PlanRow label="Bundle" value={plan.bundleDisplayName} />
        <PlanRow
          label={t("更新来源")}
          value={plan.sourceDisplayName ?? t("没有更新来源")}
        />
        <PlanRow label="Central Store" value={plan.managedDirectory} code />

        <section className="batch-plan" aria-label={t("Bundle 成员预览")}>
          <p className="section-eyebrow">BUNDLE MEMBERS</p>
          <ul className="batch-plan-list">
            {plan.retainedMembers.map((member) => (
              <li key={member.memberId} className="is-ready">
                <span className="batch-plan-copy">
                  <span className="batch-plan-heading">
                    <strong>{member.skillName}</strong>
                    <small className="batch-disposition ready">
                      {t("保留现有 Skill")}
                    </small>
                  </span>
                  <span>
                    {t("Installation Chain：{chain}", {
                      chain: installationChainLabel(
                        member.installationChain,
                        t,
                      ),
                    })}
                  </span>
                  <code title={member.expectedTarget}>
                    {t("继续使用：{path}", {
                      path: member.expectedTarget,
                    })}
                  </code>
                </span>
              </li>
            ))}
            {plan.members.map((member) => {
              const selectedOrigin = plan.origins.find(
                (origin) =>
                  origin.memberId === member.memberId &&
                  origin.observationId === member.selectedObservationId,
              );
              return (
                <li key={member.memberId} className="is-ready">
                  <span className="batch-plan-copy">
                    <span className="batch-plan-heading">
                      <strong>{member.skillName}</strong>
                      <small className="batch-disposition ready">
                        Skill Member
                      </small>
                    </span>
                    <span>
                      {t("Installation Chain：{chain}", {
                        chain: installationChainLabel(
                          member.installationChain,
                          t,
                        ),
                      })}
                    </span>
                    <code
                      title={
                        selectedOrigin?.originalPath ??
                        member.selectedObservationId
                      }
                    >
                      {t("采用内容：{path}", {
                        path:
                          selectedOrigin?.originalPath ??
                          member.selectedObservationId,
                      })}
                    </code>
                    <code title={member.expectedTarget}>
                      {t("受管目标：{path}", {
                        path: member.expectedTarget,
                      })}
                    </code>
                    {member.warnings.map((warning) => (
                      <em key={warning}>{warning}</em>
                    ))}
                  </span>
                </li>
              );
            })}
          </ul>
        </section>

        <div className="batch-plan" aria-label={t("原有位置处理")}>
          <p className="section-eyebrow">EXISTING LOCATIONS</p>
          <ul className="batch-plan-list">
            {plan.origins.map((origin) => (
              <li
                key={`${origin.memberId}:${origin.observationId}`}
                className="is-ready"
              >
                <span className="batch-plan-copy">
                  <span className="batch-plan-heading">
                    <strong>
                      {`${memberName(plan, origin.memberId)} · ${originLabel(
                        origin,
                        t,
                      )}`}
                    </strong>
                    <small className="batch-disposition ready">
                      {origin.finalDisposition === "mount"
                        ? t("替换为 Mount")
                        : t("移除原位置")}
                    </small>
                  </span>
                  <code title={origin.originalPath}>{origin.originalPath}</code>
                  {origin.warnings.map((warning) => (
                    <em key={warning}>{warning}</em>
                  ))}
                </span>
              </li>
            ))}
          </ul>
        </div>

        <div className="batch-plan" aria-label={t("最终挂载位置")}>
          <p className="section-eyebrow">FINAL MOUNTS</p>
          {retainedMounts.length > 0 || plan.targets.length > 0 ? (
            <ul className="batch-plan-list">
              {retainedMounts.map(({ member, mount }) => (
                <li key={mount.id} className="is-ready">
                  <span className="batch-plan-copy">
                    <span className="batch-plan-heading">
                      <strong>
                        {`${member.skillName} · ${targetLabel(mount, t)}`}
                      </strong>
                      <small className="batch-disposition ready">
                        {t("保留 Mount")}
                      </small>
                    </span>
                    <code title={mount.targetPath}>{mount.targetPath}</code>
                    <code title={mount.expectedTarget}>
                      → {mount.expectedTarget}
                    </code>
                  </span>
                </li>
              ))}
              {plan.targets.map((target) => (
                <li
                  key={`${target.memberId}:${target.mountId}`}
                  className="is-ready"
                >
                  <span className="batch-plan-copy">
                    <span className="batch-plan-heading">
                      <strong>
                        {`${memberName(plan, target.memberId)} · ${targetLabel(
                          target,
                          t,
                        )}`}
                      </strong>
                      <small className="batch-disposition ready">
                        {t("创建 Mount")}
                      </small>
                    </span>
                    <code title={target.targetPath}>{target.targetPath}</code>
                    <code title={target.expectedTarget}>
                      → {target.expectedTarget}
                    </code>
                    {target.appId === "claudeCode" &&
                    target.scope === "project" ? (
                      <em>
                        {t(
                          "GitHub Copilot 也可能读取这个 Claude Code 项目目录。",
                        )}
                      </em>
                    ) : null}
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="mount-project-hint">
              {t("接管后保持已安装、未挂载；所有原使用位置都会移除。")}
            </p>
          )}
        </div>

        <div className="install-mount-note">
          <strong>{t("临时恢复内容不是版本历史")}</strong>
          <span>
            {t(
              "SkillYard 只在事务期间保留恢复所需内容；验证成功后会清理，未选副本不会成为回滚版本。",
            )}
          </span>
        </div>
        {plan.warnings.length > 0 ? (
          <ul className="install-warnings" aria-label={t("接管提示")}>
            {plan.warnings.map((warning) => (
              <li key={warning}>{warning}</li>
            ))}
          </ul>
        ) : null}
      </section>

      <p className="mount-confirm-warning">
        {t("确认开始后不能取消，也不会接受部分接管结果。")}
      </p>
      <div className="install-actions">
        <button
          className="primary-action"
          type="button"
          disabled={isConfirming}
          onClick={onConfirm}
        >
          {isConfirming ? t("正在安全接管…") : t("确认接管")}
        </button>
      </div>
    </main>
  );
}

function memberName(plan: TakeoverPlan, memberId: string): string {
  return (
    plan.members.find((member) => member.memberId === memberId)?.skillName ??
    plan.retainedMembers.find((member) => member.memberId === memberId)
      ?.skillName ??
    "Skill"
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

function installationChainLabel(
  chain: InstallationChain | null,
  t: ReturnType<typeof useI18n>["t"],
): string {
  if (!chain) return t("未发现可核验的安装记录");
  const memberPath = chain.skillPath ? ` · ${chain.skillPath}` : "";
  return `lock v3 · ${chain.sourceLocator}${memberPath}`;
}

function originLabel(
  origin: TakeoverPlanOrigin,
  t: ReturnType<typeof useI18n>["t"],
): string {
  if (!origin.appId || !origin.scope) return t("共享目录");
  return origin.scope === "global"
    ? t("{app} · 全局", { app: supportedAppLabel(origin.appId) })
    : t("{app} · {project}", {
        app: supportedAppLabel(origin.appId),
        project: origin.projectDisplayName ?? t("已登记项目"),
      });
}

function targetLabel(
  target: Pick<
    TakeoverPlanTarget,
    "appId" | "scope" | "projectDisplayName"
  >,
  t: ReturnType<typeof useI18n>["t"],
): string {
  return target.scope === "global"
    ? t("{app} · 全局", { app: supportedAppLabel(target.appId) })
    : t("{app} · {project}", {
        app: supportedAppLabel(target.appId),
        project: target.projectDisplayName ?? t("已登记项目"),
      });
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}
