interface CurrentOperationBannerProps {
  title: string;
  detail: string;
  canBrowse: boolean;
  isBrowsing: boolean;
  onBrowse(): void;
  onReturn(): void;
}

export function CurrentOperationBanner({
  title,
  detail,
  canBrowse,
  isBrowsing,
  onBrowse,
  onReturn,
}: CurrentOperationBannerProps) {
  return (
    <aside className="current-operation" aria-label="当前操作">
      <div>
        <p className="section-eyebrow">CURRENT OPERATION</p>
        <strong>{title}</strong>
        <span>{detail}</span>
      </div>
      {isBrowsing ? (
        <button className="primary-action" type="button" onClick={onReturn}>
          返回当前操作
        </button>
      ) : canBrowse ? (
        <button className="secondary-action" type="button" onClick={onBrowse}>
          浏览已提交清单
        </button>
      ) : (
        <span className="current-operation-note">当前没有可浏览的已提交清单</span>
      )}
    </aside>
  );
}
