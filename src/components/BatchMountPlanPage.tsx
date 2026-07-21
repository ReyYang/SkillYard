import { useState } from "react";

import type {
  BatchMountDisposition,
  BatchMountPlan,
  BatchMountPlanItem,
  SupportedAppId,
} from "../domain";

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
      <p className="eyebrow">SKILLYARD · CONFIRM BATCH MOUNT</p>
      <h1>{`确认 ${plan.bundleDisplayName} 批量挂载`}</h1>
      <p className="lead">
        每项仍是一个独立的 Skill Mount，不会创建 Bundle 级软链接。
      </p>

      <section className="batch-plan" aria-label="批量挂载影响预览">
        <div className="install-mount-note">
          <strong>确认后的所选项会全部完成或全部撤销</strong>
          <span>
            冲突和已经挂载的项目不会进入事务；你也可以排除其他 Ready 项。
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
                    aria-label={itemLabel(item)}
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
                        {dispositionLabel(item.disposition)}
                      </small>
                    </span>
                    <span>{`${supportedAppLabel(item.appId)} · ${destinationLabel(item)}`}</span>
                    <code title={item.targetPath}>{item.targetPath}</code>
                    {!ready ? (
                      <em>{item.conflictReason ?? dispositionReason(item.disposition)}</em>
                    ) : null}
                  </span>
                </label>
              </li>
            );
          })}
        </ul>
      </section>

      {selectedItemIds.length === 0 ? (
        <p className="install-selection-empty">至少保留一个可挂载项</p>
      ) : null}
      <p className="mount-confirm-warning">
        确认开始后不能取消，也不能接受部分结果。
      </p>
      <div className="install-actions">
        <button
          className="secondary-action"
          type="button"
          disabled={isConfirming}
          onClick={onBack}
        >
          返回
        </button>
        <button
          className="primary-action"
          type="button"
          disabled={isConfirming || selectedItemIds.length === 0}
          onClick={() => onConfirm(selectedItemIds)}
        >
          {isConfirming ? "正在安全挂载…" : "确认批量挂载"}
        </button>
      </div>
    </main>
  );
}

function itemLabel(item: BatchMountPlanItem): string {
  return `${item.skillName} · ${supportedAppLabel(item.appId)} · ${destinationLabel(item)}`;
}

function destinationLabel(item: BatchMountPlanItem): string {
  return item.scope === "global"
    ? "全局"
    : `项目 ${item.projectDisplayName ?? "已登记项目"}`;
}

function dispositionLabel(disposition: BatchMountDisposition): string {
  return {
    ready: "可挂载",
    pathConflict: "路径冲突",
    scopeConflict: "Scope 冲突",
    alreadyMounted: "已挂载",
  }[disposition];
}

function dispositionReason(disposition: BatchMountDisposition): string {
  return {
    ready: "可安全创建",
    pathConflict: "目标路径已被其他内容占用",
    scopeConflict: "同一应用的 global 与 project scope 不能重叠",
    alreadyMounted: "已经挂载，无需重复创建",
  }[disposition];
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}
