import type {
  SupportedAppId,
  TakeoverPlan,
  TakeoverPlanOrigin,
  TakeoverPlanTarget,
} from "../domain";

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
  const selectedOrigin = plan.origins.find(
    (origin) => origin.observationId === plan.selectedObservationId,
  );

  return (
    <main className="mount-shell">
      <p className="eyebrow">SKILLYARD · CONFIRM TAKEOVER</p>
      <h1>{`确认接管 ${plan.skillName}`}</h1>
      <p className="lead">
        下面是 Rust 根据当前文件状态封存的完整影响。确认开始后不能取消；如果应用意外退出，
        下次启动会继续恢复到一致状态。
      </p>

      <section className="install-plan" aria-label="接管影响预览">
        <PlanRow label="Bundle" value={plan.bundleDisplayName} />
        <PlanRow label="Skill Member" value={plan.skillName} />
        <PlanRow
          label="更新来源"
          value={plan.sourceDisplayName ?? "没有更新来源"}
        />
        <PlanRow
          label="安装线索"
          // 1.0 只展示后端已确认的来源；没有证据时明确未知，不能猜测外部 CLI。
          value={
            plan.sourceDisplayName
              ? `已确认来源：${plan.sourceDisplayName}`
              : "未发现可靠的安装命令或来源记录"
          }
        />
        <PlanRow
          label="采用内容"
          value={selectedOrigin?.originalPath ?? plan.selectedObservationId}
          code
        />
        <PlanRow label="Central Store" value={plan.managedDirectory} code />

        <div className="batch-plan" aria-label="原有位置处理">
          <p className="section-eyebrow">EXISTING LOCATIONS</p>
          <ul className="batch-plan-list">
            {plan.origins.map((origin) => (
              <li key={origin.observationId} className="is-ready">
                <span className="batch-plan-copy">
                  <span className="batch-plan-heading">
                    <strong>{originLabel(origin)}</strong>
                    <small className="batch-disposition ready">
                      {origin.finalDisposition === "mount"
                        ? "替换为 Mount"
                        : "移除原位置"}
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

        <div className="batch-plan" aria-label="最终挂载位置">
          <p className="section-eyebrow">FINAL MOUNTS</p>
          {plan.targets.length > 0 ? (
            <ul className="batch-plan-list">
              {plan.targets.map((target) => (
                <li key={target.mountId} className="is-ready">
                  <span className="batch-plan-copy">
                    <span className="batch-plan-heading">
                      <strong>{targetLabel(target)}</strong>
                      <small className="batch-disposition ready">创建 Mount</small>
                    </span>
                    <code title={target.targetPath}>{target.targetPath}</code>
                    <code title={target.expectedTarget}>
                      → {target.expectedTarget}
                    </code>
                    {target.appId === "claudeCode" &&
                    target.scope === "project" ? (
                      <em>
                        GitHub Copilot 也可能读取这个 Claude Code 项目目录。
                      </em>
                    ) : null}
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="mount-project-hint">
              接管后保持已安装、未挂载；所有原使用位置都会移除。
            </p>
          )}
        </div>

        <div className="install-mount-note">
          <strong>临时恢复内容不是版本历史</strong>
          <span>
            SkillYard 只在事务期间保留恢复所需内容；验证成功后会清理，未选副本不会成为回滚版本。
          </span>
        </div>
        {plan.warnings.length > 0 ? (
          <ul className="install-warnings" aria-label="接管提示">
            {plan.warnings.map((warning) => (
              <li key={warning}>{warning}</li>
            ))}
          </ul>
        ) : null}
      </section>

      <p className="mount-confirm-warning">
        确认开始后不能取消，也不会接受部分接管结果。
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
          {isConfirming ? "正在安全接管…" : "确认接管"}
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

function originLabel(origin: TakeoverPlanOrigin): string {
  if (!origin.appId || !origin.scope) return "共享目录";
  return origin.scope === "global"
    ? `${supportedAppLabel(origin.appId)} · 全局`
    : `${supportedAppLabel(origin.appId)} · ${origin.projectDisplayName ?? "已登记项目"}`;
}

function targetLabel(target: TakeoverPlanTarget): string {
  return target.scope === "global"
    ? `${supportedAppLabel(target.appId)} · 全局`
    : `${supportedAppLabel(target.appId)} · ${target.projectDisplayName ?? "已登记项目"}`;
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}
