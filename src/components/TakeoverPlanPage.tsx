import { useState } from "react";

import type {
  SupportedAppId,
  TakeoverPlan,
  TakeoverPlanPath,
} from "../domain";

interface TakeoverPlanPageProps {
  plan: TakeoverPlan;
  isConfirming: boolean;
  onBack(): void;
  onConfirm(preservedPathIds: string[]): void;
}

export function TakeoverPlanPage({
  plan,
  isConfirming,
  onBack,
  onConfirm,
}: TakeoverPlanPageProps) {
  const [preservedPathIds, setPreservedPathIds] = useState<string[]>(() =>
    // 默认选择完全由 Rust Plan 签发；前端只允许用户缩小或恢复这组 Mount。
    plan.paths
      .filter((path) => path.defaultPreserveMount)
      .map((path) => path.id),
  );

  const togglePath = (pathId: string, checked: boolean) => {
    setPreservedPathIds((current) =>
      checked
        ? [...new Set([...current, pathId])]
        : current.filter((id) => id !== pathId),
    );
  };

  return (
    <main className="install-shell">
      <p className="eyebrow">SKILLYARD · TAKEOVER PLAN</p>
      <h1>{`确认接管 ${plan.skillName}`}</h1>
      <p className="lead">
        SkillYard 会把这个 Skill 变成 Central Store 中的唯一主副本。确认前可以返回；确认开始后不能取消。
      </p>

      <section className="install-plan" aria-label="接管影响预览">
        <PlanRow label="Bundle" value={plan.bundleDisplayName} />
        <PlanRow label="Skill" value={plan.skillName} />
        <PlanRow
          label="来源"
          value={plan.sourceDisplayName ?? "未知来源"}
        />
        <PlanRow label="Central Store" value={plan.managedDirectory} code />
        <PlanRow label="生效内容" value={plan.expectedTarget} code />

        <div className="install-mount-note">
          <strong>{plan.sourceNotice}</strong>
          <span>
            接管只改变本机主副本与所选挂载，不会凭未知来源提供更新能力。
          </span>
        </div>

        <div className="install-candidates" aria-label="原路径与挂载选择">
          {plan.paths.map((path) => (
            <label className="install-candidate" key={path.id}>
              <input
                type="checkbox"
                checked={preservedPathIds.includes(path.id)}
                disabled={isConfirming}
                onChange={(event) => togglePath(path.id, event.target.checked)}
              />
              <span className="install-candidate-copy">
                <strong>{pathDestination(path)}</strong>
                <code title={path.originalPath}>{path.originalPath}</code>
                <span className="candidate-warning">
                  {preservedPathIds.includes(path.id)
                    ? "接管后用软链接继续在这里使用"
                    : "接管后从这里移除，不创建挂载"}
                </span>
              </span>
            </label>
          ))}
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
        确认后，SkillYard 会完成或自动恢复复制、替换软链接和保存管理状态的全过程。
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
          onClick={() => onConfirm(preservedPathIds)}
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

function pathDestination(path: TakeoverPlanPath): string {
  const app = supportedAppLabel(path.appId);
  return path.scope === "global"
    ? `${app} · 全局`
    : `${app} · 项目 ${path.projectDisplayName ?? "已登记项目"}`;
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}
