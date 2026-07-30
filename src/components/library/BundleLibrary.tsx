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

/**
 * 三种 Library renderer 只接收同一份只读条目和选择回调，不能拥有领域状态。
 * #61 先交付 Ledger；后续 renderer 继续复用这个合同。
 */
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
    return (
      <section
        className="bundle-library ledger-library"
        role="region"
        aria-label="Ledger Bundle Library"
        data-theme-preset="ledger"
      >
        {emptyState}
      </section>
    );
  }

  // #62、#63 开放主题选择前，未知的视觉分支仍安全退回已经完整交付的 Ledger。
  const renderedTheme = theme === "ledger" ? theme : "ledger";
  return (
    <section
      className={`bundle-library ${renderedTheme}-library`}
      role="region"
      aria-label="Ledger Bundle Library"
      data-theme-preset={renderedTheme}
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

function moveKeyboardSelection(
  event: KeyboardEvent<HTMLElement>,
  items: BundleLibraryItem[],
  activeId: string,
  onSelect: (id: string) => void,
) {
  const keys = ["ArrowDown", "ArrowUp", "Home", "End"];
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
        : event.key === "ArrowDown"
          ? (currentIndex + 1) % items.length
          : (currentIndex - 1 + items.length) % items.length;
  onSelect(items[nextIndex]!.id);
  const buttons = event.currentTarget.querySelectorAll<HTMLButtonElement>(
    "[data-library-select]",
  );
  buttons[nextIndex]?.focus();
}
