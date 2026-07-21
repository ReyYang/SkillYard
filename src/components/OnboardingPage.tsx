interface OnboardingPageProps {
  isScanning: boolean;
  onStartScan(): void;
}

export function OnboardingPage({
  isScanning,
  onStartScan,
}: OnboardingPageProps) {
  return (
    <main className="onboarding-shell">
      <section className="onboarding-copy">
        <div className="brand-mark" aria-hidden="true">
          SY
        </div>
        <p className="eyebrow">SKILLYARD · LOCAL SKILL LIBRARY</p>
        <h1>管理本机 Skill，从一次只读扫描开始</h1>
        <p className="lead">
          SkillYard 将读取 Codex、Claude Code 和 GitHub Copilot
          已确认的本地 Skill 目录。
        </p>
        <div className="safety-note">
          <span aria-hidden="true">✓</span>
          <p>扫描不会自动接管、移动、覆盖或删除任何 Skill。</p>
        </div>
        <button
          className="primary-action"
          type="button"
          disabled={isScanning}
          onClick={onStartScan}
        >
          {isScanning ? "正在扫描…" : "开始扫描"}
        </button>
      </section>
      <aside className="scope-card" aria-label="扫描范围">
        <p className="scope-label">本次读取范围</p>
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
            <span>共享只读目录</span>
            <code>~/.agents/skills</code>
          </li>
        </ul>
        <p className="local-only">全部数据只保存在这台 Mac。</p>
      </aside>
    </main>
  );
}
