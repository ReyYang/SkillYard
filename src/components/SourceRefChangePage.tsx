import type { SourceRefChangePlan } from "../domain";

interface SourceRefChangePageProps {
  plan: SourceRefChangePlan;
  isConfirming: boolean;
  error: string | null;
  onBack(): void;
  onConfirm(): void;
}

export function SourceRefChangePage({
  plan,
  isConfirming,
  error,
  onBack,
  onConfirm,
}: SourceRefChangePageProps) {
  return (
    <main className="source-shell source-ref-shell">
      <p className="eyebrow">SKILLYARD · TRACKED REF</p>
      <h1>确认更改 Source 分支</h1>
      <p className="lead">
        同一个 GitHub 仓库只保存一个 Source。更改后，后续目录加载和安装都以新的
        Tracked Ref 为准；现有 Bundle 内容和 Mount 不会改变。
      </p>

      <section className="source-ref-plan" aria-label="Tracked Ref 变更预览">
        <div>
          <span>Source</span>
          <strong>{plan.sourceDisplayName}</strong>
        </div>
        <div>
          <span>当前 Ref</span>
          <code>{plan.currentRef}</code>
        </div>
        <div>
          <span>新的 Ref</span>
          <code>{plan.candidateRef}</code>
        </div>
        <div>
          <span>已解析 Commit</span>
          <code>{plan.candidateCommitSha}</code>
        </div>
      </section>

      {error ? (
        <div className="inline-error" role="alert">
          <strong>无法更改 Tracked Ref</strong>
          <span>{error}</span>
        </div>
      ) : null}

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
          {isConfirming ? "正在确认…" : "确认更改"}
        </button>
      </div>
    </main>
  );
}
