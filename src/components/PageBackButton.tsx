import { useI18n } from "../i18n";

interface PageBackButtonProps {
  disabled?: boolean;
  label?: string;
  onClick(): void;
}

// 次级页面统一把返回入口放在内容左上角，具体返回目标仍由页面状态决定。
export function PageBackButton({
  disabled = false,
  label,
  onClick,
}: PageBackButtonProps) {
  const { t } = useI18n();
  return (
    <button
      className="page-back-action"
      type="button"
      aria-label={t("返回上一页")}
      disabled={disabled}
      onClick={onClick}
    >
      <span aria-hidden="true">←</span>
      {label ?? t("返回")}
    </button>
  );
}
