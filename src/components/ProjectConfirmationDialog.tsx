import type { ProjectSelection } from "../domain";
import { useI18n } from "../i18n";

interface ProjectConfirmationDialogProps {
  selection: ProjectSelection;
  isConfirming: boolean;
  error: string | null;
  onCancel(): void;
  onConfirm(): void;
}

export function ProjectConfirmationDialog({
  selection,
  isConfirming,
  error,
  onCancel,
  onConfirm,
}: ProjectConfirmationDialogProps) {
  const { t } = useI18n();
  // 目录选择与登记明确分开；关闭弹窗时后端仍没有创建 Project 记录。
  return (
    <div className="dialog-backdrop">
      <section
        className="confirmation-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="project-confirmation-title"
      >
        <p className="eyebrow">PROJECT</p>
        <h2 id="project-confirmation-title">{t("确认添加项目")}</h2>
        <p>
          {t(
            "SkillYard 将登记这个项目，并扫描其中受支持应用的 Skill 目录。",
          )}
        </p>
        <code className="confirmation-dialog-path">{selection.rootPath}</code>
        {error ? (
          <div className="inline-error" role="alert">
            <strong>{t("无法添加项目")}</strong>
            <span>{error}</span>
          </div>
        ) : null}
        <div className="confirmation-dialog-actions">
          <button
            className="secondary-action"
            type="button"
            disabled={isConfirming}
            onClick={onCancel}
          >
            {t("取消")}
          </button>
          <button
            className="primary-action"
            type="button"
            disabled={isConfirming}
            onClick={onConfirm}
          >
            {isConfirming ? t("正在添加…") : t("确认添加")}
          </button>
        </div>
      </section>
    </div>
  );
}
