import { useState } from "react";

import type {
  BundleUpdateBatchItemStatus,
  BundleUpdateBatchPlan,
  BundleUpdateBatchPlanItem,
  BundleUpdateBatchResult,
  InstallCandidate,
  InstallPlan,
  MountSummary,
  SupportedAppId,
  UiOutcome,
} from "../domain";
import { PageBackButton } from "./PageBackButton";

type BundleUpdateBatchOutcome = Extract<
  UiOutcome,
  { type: "bundleUpdateBatchPlan" | "bundleUpdateBatchResult" }
>;

interface BundleUpdateBatchPageProps {
  outcome: BundleUpdateBatchOutcome;
  isConfirming: boolean;
  isDiscarding: boolean;
  isAcknowledging: boolean;
  error: string | null;
  onDiscard(planId: string): void;
  onConfirm(planId: string, selectedItemIds: string[]): void;
  onAcknowledge(batchId: string): void;
}

export function BundleUpdateBatchPage({
  outcome,
  isConfirming,
  isDiscarding,
  isAcknowledging,
  error,
  onDiscard,
  onConfirm,
  onAcknowledge,
}: BundleUpdateBatchPageProps) {
  if (outcome.type === "bundleUpdateBatchResult") {
    return (
      <BatchResult
        result={outcome.result}
        isAcknowledging={isAcknowledging}
        error={error}
        onAcknowledge={onAcknowledge}
      />
    );
  }
  return (
    <BatchPlan
      key={outcome.plan.id}
      plan={outcome.plan}
      isConfirming={isConfirming}
      isDiscarding={isDiscarding}
      error={error}
      onDiscard={onDiscard}
      onConfirm={onConfirm}
    />
  );
}

function BatchPlan({
  plan,
  isConfirming,
  isDiscarding,
  error,
  onDiscard,
  onConfirm,
}: {
  plan: BundleUpdateBatchPlan;
  isConfirming: boolean;
  isDiscarding: boolean;
  error: string | null;
  onDiscard(planId: string): void;
  onConfirm(planId: string, selectedItemIds: string[]): void;
}) {
  const [selectedItemIds, setSelectedItemIds] = useState<string[]>(() =>
    plan.items.filter(isReadyItem).map((item) => item.id),
  );
  const isBusy = isConfirming || isDiscarding;
  // 提交顺序永远来自后端 Plan；用户勾选的先后顺序不能改变执行顺序。
  const orderedSelectedItemIds = plan.items
    .filter((item) => isReadyItem(item) && selectedItemIds.includes(item.id))
    .map((item) => item.id);

  const toggleItem = (itemId: string, checked: boolean) => {
    setSelectedItemIds((current) =>
      checked
        ? [...new Set([...current, itemId])]
        : current.filter((id) => id !== itemId),
    );
  };

  return (
    <main className="batch-update-shell">
      <PageBackButton
        disabled={isBusy}
        label={isDiscarding ? "正在清理更新预览…" : "返回"}
        onClick={() => onDiscard(plan.id)}
      />
      <p className="eyebrow">SKILLYARD · ALL UPDATES</p>
      <h1>确认全部更新</h1>
      <p className="lead">
        每个 Bundle 仍使用自己的更新事务，并按下面的页面顺序逐个执行。一个普通失败不会撤销已经成功的
        Bundle，也不会阻止后续 Bundle。
      </p>

      <section
        className="bundle-update-batch-plan"
        aria-label="全部更新影响预览"
      >
        {plan.items.map((item) => {
          const ready = isReadyItem(item);
          return (
            <section
              className={`bundle-update-batch-item${ready ? "" : " is-failed"}`}
              aria-label={`Bundle 更新预览：${item.bundleDisplayName}`}
              key={item.id}
            >
              <label className="bundle-update-batch-choice">
                <input
                  type="checkbox"
                  aria-label={`更新 ${item.bundleDisplayName}`}
                  checked={ready && selectedItemIds.includes(item.id)}
                  disabled={isBusy || !ready}
                  onChange={(event) =>
                    toggleItem(item.id, event.target.checked)
                  }
                />
                <span>
                  <strong>{item.bundleDisplayName}</strong>
                  <small className={`batch-disposition ${item.disposition}`}>
                    {ready ? "已准备" : "准备失败"}
                  </small>
                </span>
              </label>
              {ready ? (
                <BundlePreview
                  bundleDisplayName={item.bundleDisplayName}
                  plan={item.installPlan}
                />
              ) : (
                <p className="bundle-update-batch-error">
                  {item.errorSummary ?? "无法准备这个 Bundle 的更新预览"}
                </p>
              )}
            </section>
          );
        })}
      </section>

      {orderedSelectedItemIds.length === 0 ? (
        <p className="install-selection-empty">至少选择一个已准备的 Bundle。</p>
      ) : null}
      {error ? (
        <div className="inline-error" role="alert">
          <strong>全部更新未开始</strong>
          <span>{error}</span>
        </div>
      ) : null}
      <p className="mount-confirm-warning">
        确认开始后不能取消或修改选择；应用会顺序完成各 Bundle。
      </p>
      <div className="install-actions">
        <button
          className="primary-action"
          type="button"
          disabled={isBusy || orderedSelectedItemIds.length === 0}
          onClick={() => onConfirm(plan.id, orderedSelectedItemIds)}
        >
          {isConfirming ? "正在顺序更新…" : "确认全部更新"}
        </button>
      </div>
    </main>
  );
}

function BundlePreview({
  bundleDisplayName,
  plan,
}: {
  bundleDisplayName: string;
  plan: InstallPlan;
}) {
  const impact = plan.updateImpact;
  return (
    <div className="bundle-update-batch-preview">
      <ul
        className="bundle-update-batch-skills"
        aria-label={`${bundleDisplayName} 全部 Skill`}
      >
        {plan.candidates.map((candidate) => (
          <CandidateSummary
            candidate={candidate}
            isNew={
              impact?.newCandidateIds.includes(candidate.candidateId) ?? false
            }
            key={candidate.candidateId}
          />
        ))}
      </ul>
      <MountSummaryList
        bundleDisplayName={bundleDisplayName}
        mounts={impact?.existingMounts ?? []}
      />
      {plan.warnings.length > 0 ? (
        <ul
          className="install-warnings"
          aria-label={`${bundleDisplayName} 更新提示`}
        >
          {plan.warnings.map((warning) => (
            <li key={warning}>{warning}</li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function CandidateSummary({
  candidate,
  isNew,
}: {
  candidate: InstallCandidate;
  isNew: boolean;
}) {
  const pathParts = candidate.sourceRelativePath.split("/").filter(Boolean);
  const displayName =
    candidate.skillName ??
    pathParts[pathParts.length - 1] ??
    "无法识别的 Skill";
  return (
    <li>
      <span className="install-candidate-heading">
        <strong>{displayName}</strong>
        {isNew ? <span className="candidate-new">新增安装</span> : null}
      </span>
      <code>{candidate.sourceRelativePath || "所选 Bundle 根目录"}</code>
      {candidate.validationErrors.map((message) => (
        <span className="candidate-error" key={message}>
          {message}
        </span>
      ))}
      {candidate.warnings.map((warning) => (
        <span className="candidate-warning" key={warning}>
          {warning}
        </span>
      ))}
    </li>
  );
}

function MountSummaryList({
  bundleDisplayName,
  mounts,
}: {
  bundleDisplayName: string;
  mounts: MountSummary[];
}) {
  return (
    <div
      className="bundle-update-batch-mounts"
      aria-label={`${bundleDisplayName} 现有挂载`}
    >
      <strong>现有挂载</strong>
      {mounts.length === 0 ? (
        <span>当前没有挂载</span>
      ) : (
        <ul>
          {mounts.map((mount) => (
            <li key={mount.id}>
              <span>{mount.skillName}</span>
              <span>{mountLabel(mount)}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function BatchResult({
  result,
  isAcknowledging,
  error,
  onAcknowledge,
}: {
  result: BundleUpdateBatchResult;
  isAcknowledging: boolean;
  error: string | null;
  onAcknowledge(batchId: string): void;
}) {
  const blocked = result.status === "blocked";
  return (
    <main className="batch-update-shell">
      {!blocked ? (
        <PageBackButton
          disabled={isAcknowledging}
          label={isAcknowledging ? "正在返回清单…" : "返回"}
          onClick={() => onAcknowledge(result.id)}
        />
      ) : null}
      <p className="eyebrow">SKILLYARD · ALL UPDATES RESULT</p>
      <h1>{blocked ? "全部更新正在等待人工恢复" : "全部更新已完成"}</h1>
      <p className="lead">
        {blocked
          ? "批量协调已停止，未执行的 Bundle 保持原内容。"
          : "每个 Bundle 都保留自己的独立结果；失败项没有撤销其他成功更新。"}
      </p>

      {blocked ? (
        <div className="recovery-warning" role="alert">
          <strong>请在人工恢复页面处理</strong>
          <p>
            当前结果不能确认已读；请保留 Central Store 和现有 Mount，不要手动改写相关目录。
          </p>
        </div>
      ) : null}

      <section className="bundle-update-batch-results" aria-label="全部更新结果">
        {result.items.map((item) => (
          <section
            className={`bundle-update-result-item is-${item.status}`}
            aria-label={`${item.bundleDisplayName} 更新结果`}
            key={item.id}
          >
            <strong>{item.bundleDisplayName}</strong>
            <span className={`batch-result-status is-${item.status}`}>
              {resultStatusLabel(item.status)}
            </span>
            {item.errorSummary ? <p>{item.errorSummary}</p> : null}
          </section>
        ))}
      </section>

      {error ? (
        <div className="inline-error" role="alert">
          <strong>无法返回清单</strong>
          <span>{error}</span>
        </div>
      ) : null}
    </main>
  );
}

function isReadyItem(
  item: BundleUpdateBatchPlanItem,
): item is BundleUpdateBatchPlanItem & { installPlan: InstallPlan } {
  return item.disposition === "ready" && item.installPlan !== null;
}

function resultStatusLabel(status: BundleUpdateBatchItemStatus): string {
  return {
    succeeded: "成功",
    failed: "失败",
    blocked: "等待人工恢复",
    notExecuted: "未执行",
  }[status];
}

function mountLabel(mount: MountSummary): string {
  const appName = supportedAppLabel(mount.appId);
  return mount.scope === "global"
    ? `${appName} · 全局`
    : `${appName} · 项目 · ${mount.projectDisplayName ?? "已登记项目"}`;
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}
