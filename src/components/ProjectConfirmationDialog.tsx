import { useLayoutEffect, useRef, type KeyboardEvent } from "react";

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
  const dialogRef = useRef<HTMLElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const focusLifecycle = useRef(0);
  useLayoutEffect(() => {
    const lifecycle = ++focusLifecycle.current;
    // StrictMode 会重放 effect；只在这次弹窗生命周期首次进入时保存触发器。
    if (returnFocusRef.current === null) {
      returnFocusRef.current =
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
    }
    cancelRef.current?.focus();
    return () => {
      const returnTarget = returnFocusRef.current;
      queueMicrotask(() => {
        if (
          focusLifecycle.current === lifecycle &&
          returnTarget?.isConnected
        ) {
          returnTarget.focus();
        }
      });
    };
  }, []);
  useLayoutEffect(() => {
    if (isConfirming) {
      dialogRef.current?.focus();
    } else if (document.activeElement === dialogRef.current) {
      cancelRef.current?.focus();
    }
  }, [isConfirming]);

  const handleKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape" && !isConfirming) {
      event.preventDefault();
      onCancel();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = Array.from(
      event.currentTarget.querySelectorAll<HTMLElement>(
        "button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [href], [tabindex]:not([tabindex='-1'])",
      ),
    );
    if (focusable.length === 0) {
      event.preventDefault();
      dialogRef.current?.focus();
      return;
    }
    const first = focusable[0]!;
    const last = focusable[focusable.length - 1]!;
    if (!focusable.includes(document.activeElement as HTMLElement)) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    } else if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };
  // 目录选择与登记明确分开；关闭弹窗时后端仍没有创建 Project 记录。
  return (
    <div className="dialog-backdrop">
      <section
        ref={dialogRef}
        className="confirmation-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="project-confirmation-title"
        tabIndex={-1}
        onKeyDown={handleKeyDown}
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
            ref={cancelRef}
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
