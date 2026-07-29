import { useI18n } from "../i18n";

interface OnboardingPageProps {
  isScanning: boolean;
  onStartScan(): void;
}

export function OnboardingPage({
  isScanning,
  onStartScan,
}: OnboardingPageProps) {
  const { t } = useI18n();
  return (
    <main className="onboarding-shell">
      <section className="onboarding-copy">
        <div className="brand-mark" aria-hidden="true">
          SY
        </div>
        <p className="eyebrow">SKILLYARD · LOCAL SKILL LIBRARY</p>
        <h1>{t("管理本机 Skill，从一次只读扫描开始")}</h1>
        <p className="lead">
          {t(
            "SkillYard 将读取 Codex、Claude Code 和 GitHub Copilot 已确认的本地 Skill 目录。",
          )}
        </p>
        <div className="safety-note">
          <span aria-hidden="true">✓</span>
          <p>{t("扫描不会自动接管、移动、覆盖或删除任何 Skill。")}</p>
        </div>
        <button
          className="primary-action"
          type="button"
          disabled={isScanning}
          onClick={onStartScan}
        >
          {isScanning ? t("正在扫描…") : t("开始扫描")}
        </button>
      </section>
      <aside className="scope-card" aria-label={t("扫描范围")}>
        <p className="scope-label">{t("本次读取范围")}</p>
        <ul>
          <li>
            <span>Codex</span>
            <code>~/.codex/skills</code>
          </li>
          <li>
            <span>Claude Code</span>
            <code>~/.claude/skills</code>
          </li>
          <li>
            <span>GitHub Copilot</span>
            <code>~/.copilot/skills</code>
          </li>
          <li>
            <span>{t("共享只读目录")}</span>
            <code>~/.agents/skills</code>
          </li>
        </ul>
        <p className="local-only">{t("全部数据只保存在这台 Mac。")}</p>
      </aside>
    </main>
  );
}
