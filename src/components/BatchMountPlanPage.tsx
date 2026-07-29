import { useState } from "react";

import type {
  BatchMountDisposition,
  BatchMountPlan,
  BatchMountPlanItem,
  SupportedAppId,
} from "../domain";
import { useI18n } from "../i18n";
import { PageBackButton } from "./PageBackButton";

interface BatchMountPlanPageProps {
  plan: BatchMountPlan;
  isConfirming: boolean;
  onBack(): void;
  onConfirm(selectedItemIds: string[]): void;
}

export function BatchMountPlanPage({
  plan,
  isConfirming,
  onBack,
  onConfirm,
}: BatchMountPlanPageProps) {
  const { localize, t } = useI18n();
  const [selectedItemIds, setSelectedItemIds] = useState<string[]>(() =>
    // 默认集合由后端 Plan 决定；前端只能继续排除，不能扩大默认影响范围。
    plan.items
      .filter(
        (item) =>
          item.selectable &&
          item.defaultSelected &&
          item.disposition === "ready",
      )
      .map((item) => item.id),
  );

  const toggleItem = (itemId: string, checked: boolean) => {
    setSelectedItemIds((current) =>
      checked
        ? [...new Set([...current, itemId])]
        : current.filter((id) => id !== itemId),
    );
  };

  return (
    <main className="mount-shell">
      <PageBackButton disabled={isConfirming} onClick={onBack} />
      <p className="eyebrow">SKILLYARD · CONFIRM BATCH MOUNT</p>
      <h1>
        {t("确认 {bundle} 批量挂载", {
          bundle: plan.bundleDisplayName,
        })}
      </h1>
      <p className="lead">
        {t("每项仍是一个独立的 Skill Mount，不会创建 Bundle 级软链接。")}
      </p>

      <section className="batch-plan" aria-label={t("批量挂载影响预览")}>
        <div className="install-mount-note">
          <strong>{t("确认后的所选项会全部完成或全部撤销")}</strong>
          <span>
            {t("冲突和已经挂载的项目不会进入事务；你也可以排除其他 Ready 项。")}
          </span>
        </div>
        <ul className="batch-plan-list">
          {plan.items.map((item) => {
            const ready = item.selectable && item.disposition === "ready";
            return (
              <li
                key={item.id}
                className={ready ? "is-ready" : "is-blocked"}
              >
                <label>
                  <input
                    type="checkbox"
                    aria-label={itemLabel(item, t)}
                    checked={ready && selectedItemIds.includes(item.id)}
                    disabled={isConfirming || !ready}
                    onChange={(event) =>
                      toggleItem(item.id, event.target.checked)
                    }
                  />
                  <span className="batch-plan-copy">
                    <span className="batch-plan-heading">
                      <strong>{item.skillName}</strong>
                      <small className={`batch-disposition ${item.disposition}`}>
                        {dispositionLabel(item.disposition, t)}
                      </small>
                    </span>
                    <span>{`${supportedAppLabel(item.appId)} · ${destinationLabel(
                      item,
                      t,
                    )}`}</span>
                    <code title={item.targetPath}>{item.targetPath}</code>
                    {!ready ? (
                      <em>
                        {item.conflictReason
                          ? localize(
                              item.conflictReason,
                              "请在继续前检查此提示。",
                            )
                          : dispositionReason(item.disposition, t)}
                      </em>
                    ) : null}
                  </span>
                </label>
              </li>
            );
          })}
        </ul>
      </section>

      {selectedItemIds.length === 0 ? (
        <p className="install-selection-empty">
          {t("至少保留一个可挂载项")}
        </p>
      ) : null}
      <p className="mount-confirm-warning">
        {t("确认开始后不能取消，也不能接受部分结果。")}
      </p>
      <div className="install-actions">
        <button
          className="primary-action"
          type="button"
          disabled={isConfirming || selectedItemIds.length === 0}
          onClick={() => onConfirm(selectedItemIds)}
        >
          {isConfirming ? t("正在安全挂载…") : t("确认批量挂载")}
        </button>
      </div>
    </main>
  );
}

function itemLabel(
  item: BatchMountPlanItem,
  t: ReturnType<typeof useI18n>["t"],
): string {
  return `${item.skillName} · ${supportedAppLabel(item.appId)} · ${destinationLabel(
    item,
    t,
  )}`;
}

function destinationLabel(
  item: BatchMountPlanItem,
  t: ReturnType<typeof useI18n>["t"],
): string {
  return item.scope === "global"
    ? t("全局")
    : t("项目 {project}", {
        project: item.projectDisplayName ?? t("已登记项目"),
      });
}

function dispositionLabel(
  disposition: BatchMountDisposition,
  t: ReturnType<typeof useI18n>["t"],
): string {
  return {
    ready: t("可挂载"),
    pathConflict: t("路径冲突"),
    scopeConflict: t("Scope 冲突"),
    alreadyMounted: t("已挂载"),
  }[disposition];
}

function dispositionReason(
  disposition: BatchMountDisposition,
  t: ReturnType<typeof useI18n>["t"],
): string {
  return {
    ready: t("可安全创建"),
    pathConflict: t("目标路径已被其他内容占用"),
    scopeConflict: t("同一应用的 global 与 project scope 不能重叠"),
    alreadyMounted: t("已经挂载，无需重复创建"),
  }[disposition];
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}
