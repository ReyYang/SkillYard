import type { EditableLocalRelinkPlan } from "../domain";

interface EditableLocalRelinkPageProps {
  plan: EditableLocalRelinkPlan;
  isConfirming: boolean;
  isDiscarding: boolean;
  error: string | null;
  onDiscard(): void;
  onConfirm(): void;
}

export function EditableLocalRelinkPage({
  plan,
  isConfirming,
  isDiscarding,
  error,
  onDiscard,
  onConfirm,
}: EditableLocalRelinkPageProps) {
  const isBusy = isConfirming || isDiscarding;

  return (
    <main className="source-shell source-ref-shell">
      <p className="eyebrow">SKILLYARD · EDITABLE LOCAL</p>
      <h1>确认重新指定 Source 路径</h1>
      <p className="lead">
        SkillYard 已确认这是原来登记的同一个目录。确认只恢复后续检查和更新能力；
        当前受管内容、Skill 和所有 Mount 都不会改变。
      </p>

      <section className="source-ref-plan" aria-label="Source 路径变更预览">
        <div>
          <span>Source</span>
          <strong>{plan.sourceDisplayName}</strong>
        </div>
        {plan.bundleDisplayName ? (
          <div>
            <span>关联 Bundle</span>
            <strong>{plan.bundleDisplayName}</strong>
          </div>
        ) : null}
        <div>
          <span>原路径</span>
          <code>{plan.currentPath}</code>
        </div>
        <div>
          <span>新路径</span>
          <code>{plan.candidatePath}</code>
        </div>
      </section>

      <section className="source-list" aria-label="新路径中的 Skill">
        <h2>新路径中的 Skill</h2>
        <ul className="source-members">
          {plan.members.map((member) => (
            <li key={member.relativePath || member.skillName || "bundle-root"}>
              <div>
                <strong>{member.skillName ?? member.relativePath}</strong>
                <code>{member.relativePath || "."}</code>
              </div>
              <span>{member.selectable ? "可识别" : "需要修正"}</span>
              {member.validationErrors.map((message) => (
                <small className="candidate-error" key={message}>
                  {message}
                </small>
              ))}
              {member.warnings.map((message) => (
                <small key={message}>{message}</small>
              ))}
            </li>
          ))}
        </ul>
      </section>

      <p className="source-empty">
        如果新路径中的内容已经变化，确认后请回到主界面点击“检查更新”；本次操作不会直接采用这些变化。
      </p>

      {error ? (
        <div className="inline-error" role="alert">
          <strong>无法重新指定 Source 路径</strong>
          <span>{error}</span>
        </div>
      ) : null}

      <div className="install-actions">
        <button
          className="secondary-action"
          type="button"
          disabled={isBusy}
          onClick={onDiscard}
        >
          {isDiscarding ? "正在取消…" : "取消"}
        </button>
        <button
          className="primary-action"
          type="button"
          disabled={isBusy}
          onClick={onConfirm}
        >
          {isConfirming ? "正在确认…" : "确认新路径"}
        </button>
      </div>
    </main>
  );
}
