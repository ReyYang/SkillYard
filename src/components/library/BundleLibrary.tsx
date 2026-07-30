import type { KeyboardEvent, ReactNode } from "react";

import type { ThemePreset } from "../../domain";

export interface BundleLibraryItem {
  id: string;
  title: string;
  eyebrow: string;
  skillCount: number;
  summary: string | null;
  category: string | null;
}

interface BundleLibraryProps {
  theme: ThemePreset;
  items: BundleLibraryItem[];
  selectedId: string | null;
  onSelect(id: string): void;
  renderDetails(id: string): ReactNode;
  emptyState: ReactNode;
}

/** 三种 Library renderer 只接收同一份只读条目和选择回调，不能拥有领域状态。 */
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
    <section
      className="bundle-library ledger-library"
      role="region"
      aria-label="Ledger Bundle Library"
      data-theme-preset="ledger"
    >
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
              <small>
                {item.eyebrow} · {item.skillCount} Skill
              </small>
              {item.category ? <em>{item.category}</em> : null}
            </span>
          </button>
        ))}
      </nav>
      <div className="ledger-library-detail">{renderDetails(activeId)}</div>
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
  const activeItem = items.find((item) => item.id === activeId) ?? items[0]!;
  return (
    <section
      className="bundle-library archive-library"
      role="region"
      aria-label="Archive Bundle Library"
      data-theme-preset="archive"
    >
      <div className="archive-library-stage">
        <div className="archive-library-cover" aria-hidden="true">
          <span>{bundleInitial(activeItem.title)}</span>
          <small>{activeItem.skillCount} Skill</small>
        </div>
        <div className="archive-library-detail">{renderDetails(activeId)}</div>
      </div>
      <nav
        className="archive-library-shelf"
        aria-label="Bundle"
        onKeyDown={(event) =>
          moveKeyboardSelection(event, items, activeId, onSelect)
        }
      >
        {items.map((item) => (
          <button
            className="archive-library-book"
            type="button"
            key={item.id}
            data-library-select
            aria-label={item.title}
            aria-pressed={item.id === activeId}
            onClick={() => onSelect(item.id)}
          >
            <span className="archive-library-book-mark">
              {bundleInitial(item.title)}
            </span>
            <span className="archive-library-book-copy">
              <strong>{item.title}</strong>
              <small>
                {item.skillCount} Skill · {item.eyebrow}
              </small>
              {item.category ? <em>{item.category}</em> : null}
            </span>
          </button>
        ))}
      </nav>
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
  return (
    <section
      className="bundle-library layers-library"
      role="region"
      aria-label="Layers Bundle Library"
      data-theme-preset="layers"
    >
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
            aria-label={item.title}
            aria-pressed={item.id === activeId}
            onClick={() => onSelect(item.id)}
          >
            <span className="layers-library-card-index">
              {String(index + 1).padStart(2, "0")}
            </span>
            <span className="layers-library-card-copy">
              <strong>{item.title}</strong>
              <small>
                {item.skillCount} Skill · {item.eyebrow}
              </small>
              {item.category ? <em>{item.category}</em> : null}
            </span>
          </button>
        ))}
      </nav>
      <div className="layers-library-detail">{renderDetails(activeId)}</div>
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
