import type { FolderInstallPlan } from "../domain";

interface InstallFolderPageProps {
  plan: FolderInstallPlan;
  isInstalling: boolean;
  error: string | null;
  onCancel(): void;
  onConfirm(): void;
}

export function InstallFolderPage({
  plan,
  isInstalling,
  error,
  onCancel,
  onConfirm,
}: InstallFolderPageProps) {
  return (
    <main className="install-shell">
      <p className="eyebrow">SKILLYARD · INSTALL PLAN</p>
      <h1>确认安装这个 Skill</h1>
      <p className="lead">
        确认后，SkillYard 会把所选文件夹复制到自己的 Central Store。原文件夹不会被移动或修改。
        安装开始后不能取消；如果应用意外退出，下次启动会自动恢复。
      </p>

      <section className="install-plan" aria-label="安装影响预览">
        <PlanRow label="Bundle" value={plan.bundleDisplayName} />
        <PlanRow label="Skill" value={plan.skillName} />
        <PlanRow label="原文件夹" value={plan.inputPath} code />
        <PlanRow label="安装位置" value={plan.targetDirectory} code />
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
          disabled={isInstalling}
          onClick={onCancel}
        >
          返回
        </button>
        <button
          className="primary-action"
          type="button"
          disabled={isInstalling}
          onClick={onConfirm}
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
