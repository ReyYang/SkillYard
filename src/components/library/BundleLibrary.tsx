import { useRef, type KeyboardEvent, type ReactNode } from "react";

import type { ThemePreset } from "../../domain";
import { useI18n } from "../../i18n";

export interface BundleLibraryItem {
  id: string;
  title: string;
  eyebrow: string;
  skillCount: number;
  summary: string | null;
  category: string | null;
  status: string;
  statusTone: "accent" | "warning" | "muted";
}

interface BundleLibraryProps {
  theme: ThemePreset;
  items: BundleLibraryItem[];
  selectedId: string | null;
  onSelect(id: string): void;
  renderDetails(id: string): ReactNode;
  emptyState: ReactNode;
}

/** 三个 renderer 只消费同一份只读模型；选择状态和生命周期能力仍由 InventoryPage 持有。 */
export function BundleLibrary({
  theme,
  items,
  selectedId,
  onSelect,
  renderDetails,
  emptyState,
}: BundleLibraryProps) {
  const activeId =
    items.some((item) => item.id === selectedId)
      ? selectedId
      : (items[0]?.id ?? null);

  if (items.length === 0 || !activeId) {
    const renderedTheme =
      theme === "archive" || theme === "layers" ? theme : "ledger";
    return (
      <section
        className={`bundle-library ${renderedTheme}-library`}
        role="region"
        aria-label={`${capitalize(renderedTheme)} Bundle Library`}
        data-theme-preset={renderedTheme}
        data-library-scroll-region
      >
        {emptyState}
      </section>
    );
  }

  if (theme === "archive") {
    return (
      <ArchiveLibrary
        items={items}
        activeId={activeId}
        onSelect={onSelect}
        renderDetails={renderDetails}
      />
    );
  }

  if (theme === "layers") {
    return (
      <LayersLibrary
        items={items}
        activeId={activeId}
        onSelect={onSelect}
        renderDetails={renderDetails}
      />
    );
  }

  return (
    <LedgerLibrary
      items={items}
      activeId={activeId}
      onSelect={onSelect}
      renderDetails={renderDetails}
    />
  );
}

function LedgerLibrary({
  items,
  activeId,
  onSelect,
  renderDetails,
}: {
  items: BundleLibraryItem[];
  activeId: string;
  onSelect(id: string): void;
  renderDetails(id: string): ReactNode;
}) {
  const { t } = useI18n();
  const activeItem = items.find((item) => item.id === activeId) ?? items[0]!;
  return (
    <section
      className="bundle-library ledger-library"
      role="region"
      aria-label="Ledger Bundle Library"
      data-theme-preset="ledger"
      data-library-scroll-region
    >
      <header className="ledger-library-heading">
        <span>{t("全部 Bundle")}</span>
        <span>{items.length}</span>
      </header>
      <nav
        className="ledger-library-list"
        aria-label="Bundle"
        onKeyDown={(event) =>
          moveKeyboardSelection(event, items, activeId, onSelect)
        }
      >
        {items.map((item, index) => (
          <button
            className="ledger-library-item"
            type="button"
            key={item.id}
            data-library-select
            aria-label={item.title}
            aria-pressed={item.id === activeId}
            onClick={() => onSelect(item.id)}
          >
            <span className="ledger-library-index">
              {String(index + 1).padStart(2, "0")}
            </span>
            <span className="ledger-library-item-copy">
              <strong>{item.title}</strong>
              <small>{item.skillCount} Skill</small>
              <em data-tone={item.statusTone}>{item.status}</em>
            </span>
            <span className="ledger-library-disclosure" aria-hidden="true">
              {t("打开")}
            </span>
          </button>
        ))}
      </nav>
      <article className="ledger-library-detail">
        <span className="ledger-library-monogram" aria-hidden="true">
          {bundleInitial(activeItem.title)}
        </span>
        {renderDetails(activeId)}
      </article>
    </section>
  );
}

function ArchiveLibrary({
  items,
  activeId,
  onSelect,
  renderDetails,
}: {
  items: BundleLibraryItem[];
  activeId: string;
  onSelect(id: string): void;
  renderDetails(id: string): ReactNode;
}) {
  const { t } = useI18n();
  const shelfRef = useRef<HTMLElement>(null);
  const activeItem = items.find((item) => item.id === activeId) ?? items[0]!;
  const moveShelf = (direction: -1 | 1) => {
    // 横向书架只改变纯展示位置，不改变当前 Bundle 或业务状态。
    shelfRef.current?.scrollBy({
      left: direction * Math.max(240, shelfRef.current.clientWidth * 0.72),
      behavior: "smooth",
    });
  };

  return (
    <section
      className="bundle-library archive-library"
      role="region"
      aria-label="Archive Bundle Library"
      data-theme-preset="archive"
      data-library-scroll-region
    >
      <div className="archive-library-stage">
        <div className="archive-library-cover">
          <div className="archive-library-cover-texture" aria-hidden="true" />
          <span className="archive-library-cover-count">
            {activeItem.skillCount}
            <small>Skill</small>
          </span>
          <span className="archive-library-cover-mark" aria-hidden="true">
            {bundleInitial(activeItem.title)}.
          </span>
          <span className="archive-library-cover-label">
            <strong>{activeItem.title}</strong>
            <small>SkillYard Bundle Archive</small>
          </span>
        </div>
        <article className="archive-library-detail">
          {renderDetails(activeId)}
        </article>
      </div>

      <div className="archive-library-shelf-viewport">
        <button
          className="archive-library-shelf-control archive-library-shelf-control--previous"
          type="button"
          onClick={() => moveShelf(-1)}
        >
          {t("上一组")}
        </button>
        <nav
          ref={shelfRef}
          className="archive-library-shelf"
          aria-label="Bundle"
          onKeyDown={(event) =>
            moveKeyboardSelection(event, items, activeId, onSelect)
          }
        >
          {items.map((item, index) => (
            <button
              className="archive-library-book"
              type="button"
              key={item.id}
              data-library-select
              data-book-tone={String(index % 5)}
              aria-label={item.title}
              aria-pressed={item.id === activeId}
              onClick={() => onSelect(item.id)}
            >
              <span className="archive-library-book-mark">
                {bundleInitial(item.title)}.
              </span>
              <span className="archive-library-book-copy">
                <strong>{item.title}</strong>
                <small>{item.skillCount} Skill</small>
                <em data-tone={item.statusTone}>{item.status}</em>
              </span>
            </button>
          ))}
        </nav>
        <button
          className="archive-library-shelf-control archive-library-shelf-control--next"
          type="button"
          onClick={() => moveShelf(1)}
        >
          {t("下一组")}
        </button>
      </div>
    </section>
  );
}

function LayersLibrary({
  items,
  activeId,
  onSelect,
  renderDetails,
}: {
  items: BundleLibraryItem[];
  activeId: string;
  onSelect(id: string): void;
  renderDetails(id: string): ReactNode;
}) {
  const activeItem = items.find((item) => item.id === activeId) ?? items[0]!;
  return (
    <section
      className="bundle-library layers-library"
      role="region"
      aria-label="Layers Bundle Library"
      data-theme-preset="layers"
      data-library-scroll-region
    >
      <div className="layers-library-stack-viewport">
        <nav
          className="layers-library-stack"
          aria-label="Bundle"
          onKeyDown={(event) =>
            moveKeyboardSelection(event, items, activeId, onSelect)
          }
        >
          {items.map((item, index) => (
            <button
              className="layers-library-card"
              type="button"
              key={item.id}
              data-library-select
              data-layer-tone={String(index % 4)}
              aria-label={item.title}
              aria-pressed={item.id === activeId}
              onClick={() => onSelect(item.id)}
            >
              <span className="layers-library-card-index">
                {bundleInitial(item.title)}.
              </span>
              <span className="layers-library-card-copy">
                <strong>{item.title}</strong>
              </span>
            </button>
          ))}
        </nav>
      </div>
      <article className="layers-library-sheet">
        <span className="layers-library-monogram" aria-hidden="true">
          {bundleInitial(activeItem.title)}.
        </span>
        <div className="layers-library-detail">{renderDetails(activeId)}</div>
      </article>
    </section>
  );
}

function moveKeyboardSelection(
  event: KeyboardEvent<HTMLElement>,
  items: BundleLibraryItem[],
  activeId: string,
  onSelect: (id: string) => void,
) {
  const keys = [
    "ArrowDown",
    "ArrowUp",
    "ArrowRight",
    "ArrowLeft",
    "Home",
    "End",
  ];
  if (!keys.includes(event.key)) return;
  event.preventDefault();

  const currentIndex = Math.max(
    0,
    items.findIndex((item) => item.id === activeId),
  );
  const nextIndex =
    event.key === "Home"
      ? 0
      : event.key === "End"
        ? items.length - 1
        : event.key === "ArrowDown" || event.key === "ArrowRight"
          ? (currentIndex + 1) % items.length
          : (currentIndex - 1 + items.length) % items.length;
  onSelect(items[nextIndex]!.id);
  const buttons = event.currentTarget.querySelectorAll<HTMLButtonElement>(
    "[data-library-select]",
  );
  buttons[nextIndex]?.focus();
}

function bundleInitial(title: string): string {
  const first = Array.from(title.trim())[0];
  return first ? first.toLocaleUpperCase() : "S";
}

function capitalize(value: string): string {
  return `${value.charAt(0).toUpperCase()}${value.slice(1)}`;
}
