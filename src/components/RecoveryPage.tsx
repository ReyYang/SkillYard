import type { RecoveryIssue } from "../domain";
import { useI18n } from "../i18n";
import { PageBackButton } from "./PageBackButton";

interface RecoveryPageProps {
  issue: RecoveryIssue;
  isOpeningCentralStore: boolean;
  error: string | null;
  onBack(): void;
  onOpenCentralStore(): void;
}

export function RecoveryPage({
  issue,
  isOpeningCentralStore,
  error,
  onBack,
  onOpenCentralStore,
}: RecoveryPageProps) {
  const { localize, t } = useI18n();
  return (
    <main className="source-shell source-ref-shell">
      <PageBackButton disabled={isOpeningCentralStore} onClick={onBack} />
      <p className="eyebrow">SKILLYARD · FILESYSTEM RECOVERY</p>
      <h1>{t("需要人工检查文件")}</h1>
      <p className="lead">
        {t(
          "SkillYard 无法安全判断这项操作的最终状态，因此已经停止修改相关 Bundle。其他 Skill 和只读清单不受影响。",
        )}
      </p>

      <section className="source-ref-plan" aria-label={t("需要检查的操作")}>
        <div>
          <span>{t("相关 Bundle")}</span>
          <strong>{issue.bundleDisplayName}</strong>
        </div>
        <div>
          <span>{t("停止原因")}</span>
          <strong>{localize(issue.message, "这个操作需要人工恢复。")}</strong>
        </div>
      </section>

      <p className="source-empty">
        {t(
          "你可以在 Finder 中查看 Central Store。请保留现有内容，不要手动删除相关目录；SkillYard 1.0 不提供强制继续或自动清理。",
        )}
      </p>

      {error ? (
        <div className="inline-error" role="alert">
          <strong>{t("无法打开 Central Store")}</strong>
          <span>{error}</span>
        </div>
      ) : null}

      <div className="install-actions">
        <button
          className="primary-action"
          type="button"
          disabled={isOpeningCentralStore}
          onClick={onOpenCentralStore}
        >
          {isOpeningCentralStore ? t("正在打开…") : t("打开 Central Store")}
        </button>
      </div>
    </main>
  );
}
