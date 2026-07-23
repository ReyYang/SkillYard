import { useMemo, useState } from "react";

import type {
  InventoryObservation,
  MountScope,
  SupportedAppId,
  TakeoverPlanRequest,
  TakeoverSharedTargetRequest,
} from "../domain";

interface TakeoverSelectionPageProps {
  initialObservationId: string;
  candidates: InventoryObservation[];
  isPlanning: boolean;
  error: string | null;
  onBack(): void;
  onCreatePlan(request: TakeoverPlanRequest): void;
}

const SUPPORTED_APPS: SupportedAppId[] = [
  "codex",
  "claudeCode",
  "gitHubCopilot",
];

export function TakeoverSelectionPage({
  initialObservationId,
  candidates,
  isPlanning,
  error,
  onBack,
  onCreatePlan,
}: TakeoverSelectionPageProps) {
  const initial = candidates.find(
    (candidate) => candidate.id === initialObservationId,
  )!;
  const [includedIds, setIncludedIds] = useState<string[]>([
    initialObservationId,
  ]);
  const [selectedObservationId, setSelectedObservationId] = useState<
    string | null
  >(initialObservationId);
  const [preservedObservationIds, setPreservedObservationIds] = useState<
    string[]
  >(() => (isShared(initial) ? [] : [initialObservationId]));
  const [sharedTargets, setSharedTargets] = useState<
    TakeoverSharedTargetRequest[]
  >([]);

  const included = useMemo(
    () => candidates.filter((candidate) => includedIds.includes(candidate.id)),
    [candidates, includedIds],
  );
  const hasDifferentContent =
    new Set(included.map((candidate) => candidate.observedFingerprint)).size > 1;
  const invalidMetadata = included.some(
    (candidate) => candidate.metadataStatus !== "valid",
  );
  const sharedWithoutTarget = included.some(
    (candidate) =>
      isShared(candidate) &&
      !sharedTargets.some(
        (target) => target.sharedObservationId === candidate.id,
      ),
  );
  const scopeIssue = findScopeIssue(
    included,
    preservedObservationIds,
    sharedTargets,
  );
  const canCreatePlan =
    selectedObservationId !== null &&
    !invalidMetadata &&
    !sharedWithoutTarget &&
    scopeIssue === null;

  const toggleIdentity = (
    candidate: InventoryObservation,
    checked: boolean,
  ) => {
    if (candidate.id === initialObservationId) return;
    if (checked) {
      const nextIds = [...includedIds, candidate.id];
      setIncludedIds(nextIds);
      if (!isShared(candidate)) {
        setPreservedObservationIds((current) => [...current, candidate.id]);
      }
      const fingerprints = new Set(
        candidates
          .filter((item) => nextIds.includes(item.id))
          .map((item) => item.observedFingerprint),
      );
      // 内容不同时必须由用户显式选择，不能沿用点击入口作为隐式决定。
      if (fingerprints.size > 1) setSelectedObservationId(null);
      return;
    }

    const nextIds = includedIds.filter((id) => id !== candidate.id);
    setIncludedIds(nextIds);
    setPreservedObservationIds((current) =>
      current.filter((id) => id !== candidate.id),
    );
    setSharedTargets((current) =>
      current.filter((target) => target.sharedObservationId !== candidate.id),
    );
    const remaining = candidates.filter((item) => nextIds.includes(item.id));
    const remainingFingerprints = new Set(
      remaining.map((item) => item.observedFingerprint),
    );
    if (selectedObservationId === candidate.id) {
      setSelectedObservationId(
        remainingFingerprints.size === 1 ? (remaining[0]?.id ?? null) : null,
      );
    } else if (
      selectedObservationId === null &&
      remainingFingerprints.size === 1
    ) {
      setSelectedObservationId(nextIds[0] ?? null);
    }
  };

  const togglePreserved = (observationId: string, checked: boolean) => {
    setPreservedObservationIds((current) =>
      checked
        ? [...new Set([...current, observationId])]
        : current.filter((id) => id !== observationId),
    );
  };

  const toggleSharedTarget = (
    sharedObservationId: string,
    appId: SupportedAppId,
    checked: boolean,
  ) => {
    setSharedTargets((current) =>
      checked
        ? [...current, { sharedObservationId, appId }]
        : current.filter(
            (target) =>
              target.sharedObservationId !== sharedObservationId ||
              target.appId !== appId,
          ),
    );
  };

  const createPlan = () => {
    if (!canCreatePlan || selectedObservationId === null) return;
    const orderedIds = included.map((candidate) => candidate.id);
    onCreatePlan({
      observationIds: orderedIds,
      selectedObservationId,
      preservedObservationIds: included
        .filter((candidate) => preservedObservationIds.includes(candidate.id))
        .map((candidate) => candidate.id),
      sharedTargets: included.flatMap((candidate) =>
        sharedTargets.filter(
          (target) =>
            target.sharedObservationId === candidate.id &&
            !hasPreservedTargetForSharedSelection(
              candidate,
              target.appId,
              included,
              preservedObservationIds,
            ),
        ),
      ),
    });
  };

  return (
    <main className="mount-shell">
      <p className="eyebrow">SKILLYARD · TAKEOVER</p>
      <h1>{`选择要接管的 ${initial.skillName}`}</h1>
      <p className="lead">
        接管前只生成影响预览。勾选其他同名位置，表示你确认它们是同一个
        Skill；同名本身不会触发自动合并。
      </p>

      {error ? (
        <div className="inline-error" role="alert">
          <strong>无法生成接管预览</strong>
          <span>{error}</span>
        </div>
      ) : null}

      <section className="batch-member-summary" aria-label="确认同一个 Skill">
        <p className="section-eyebrow">LOCAL IDENTITY</p>
        <h2>确认属于同一个 Skill 的位置</h2>
        <div className="batch-target-list">
          {candidates.map((candidate) => (
            <label className="batch-target-option" key={candidate.id}>
              <input
                type="checkbox"
                aria-label={`确认同一 Skill：${candidate.skillRoot}`}
                checked={includedIds.includes(candidate.id)}
                disabled={isPlanning || candidate.id === initialObservationId}
                onChange={(event) =>
                  toggleIdentity(candidate, event.target.checked)
                }
              />
              <span>
                <strong>{locationLabel(candidate)}</strong>
                <code title={candidate.skillRoot}>{candidate.skillRoot}</code>
              </span>
            </label>
          ))}
        </div>
      </section>

      {hasDifferentContent ? (
        <section className="batch-member-summary" aria-label="选择唯一内容">
          <p className="section-eyebrow">PRIMARY CONTENT</p>
          <h2>请选择唯一一份内容</h2>
          <p>其他位置会统一使用这份内容，不会保留为可选旧版本。</p>
          <div className="batch-target-list">
            {included.map((candidate) => (
              <label className="batch-target-option" key={candidate.id}>
                <input
                  type="radio"
                  name="takeover-primary-content"
                  aria-label={`使用 ${candidate.skillRoot} 作为主副本`}
                  checked={selectedObservationId === candidate.id}
                  disabled={isPlanning}
                  onChange={() => setSelectedObservationId(candidate.id)}
                />
                <span>
                  <strong>{locationLabel(candidate)}</strong>
                  <code title={candidate.skillRoot}>{candidate.skillRoot}</code>
                </span>
              </label>
            ))}
          </div>
        </section>
      ) : null}

      {included.some((candidate) => !isShared(candidate)) ? (
        <section className="batch-member-summary" aria-label="保留现有使用位置">
          <p className="section-eyebrow">EXISTING MOUNTS</p>
          <h2>保留哪些现有使用位置</h2>
          <p>取消后，该原位置会在接管成功时移除，不会建立 Mount。</p>
          <div className="batch-target-list">
            {included
              .filter((candidate) => !isShared(candidate))
              .map((candidate) => (
                <label className="batch-target-option" key={candidate.id}>
                  <input
                    type="checkbox"
                    aria-label={`保留使用位置：${candidate.skillRoot}`}
                    checked={preservedObservationIds.includes(candidate.id)}
                    disabled={isPlanning}
                    onChange={(event) =>
                      togglePreserved(candidate.id, event.target.checked)
                    }
                  />
                  <span>
                    <strong>{locationLabel(candidate)}</strong>
                    <code title={candidate.skillRoot}>
                      {candidate.skillRoot}
                    </code>
                  </span>
                </label>
              ))}
          </div>
        </section>
      ) : null}

      {included.filter(isShared).map((candidate) => (
        <section
          className="batch-member-summary"
          aria-label={`共享目录目标 ${candidate.skillRoot}`}
          key={candidate.id}
        >
          <p className="section-eyebrow">SHARED DIRECTORY</p>
          <h2>选择共享目录对应的应用</h2>
          <code title={candidate.skillRoot}>{candidate.skillRoot}</code>
          <p>
            原共享入口会在全部应用专属 Mount 验证成功后移除；未选择的应用可能不再发现此
            Skill。
          </p>
          <div className="batch-target-list">
            {candidate.observedBy.map((appId) => (
              <label className="batch-target-option" key={appId}>
                <input
                  type="checkbox"
                  aria-label={`将 ${candidate.skillRoot} 挂载到 ${supportedAppLabel(appId)}`}
                  checked={sharedTargets.some(
                    (target) =>
                      target.sharedObservationId === candidate.id &&
                      target.appId === appId,
                  )}
                  disabled={isPlanning}
                  onChange={(event) =>
                    toggleSharedTarget(
                      candidate.id,
                      appId,
                      event.target.checked,
                    )
                  }
                />
                <span>
                  <strong>{supportedAppLabel(appId)}</strong>
                  <small>SkillYard 将使用该应用的固定专属 Skill 目录</small>
                </span>
              </label>
            ))}
          </div>
        </section>
      ))}

      {invalidMetadata ? (
        <p className="install-selection-empty">
          所选位置包含无效 Skill metadata，刷新或修复后才能接管。
        </p>
      ) : null}
      {sharedWithoutTarget ? (
        <p className="install-selection-empty">
          共享目录必须选择至少一个应用。
        </p>
      ) : null}
      {scopeIssue ? (
        <p className="install-selection-empty">{scopeIssue}</p>
      ) : null}
      <p className="mount-confirm-warning">
        下一步由 Rust 重新检查路径并封存影响预览，此时仍不会修改文件。
      </p>
      <div className="install-actions">
        <button
          className="secondary-action"
          type="button"
          disabled={isPlanning}
          onClick={onBack}
        >
          返回清单
        </button>
        <button
          className="primary-action"
          type="button"
          disabled={isPlanning || !canCreatePlan}
          onClick={createPlan}
        >
          {isPlanning ? "正在检查现有安装…" : "生成影响预览"}
        </button>
      </div>
    </main>
  );
}

function findScopeIssue(
  included: InventoryObservation[],
  preservedObservationIds: string[],
  sharedTargets: TakeoverSharedTargetRequest[],
): string | null {
  const originScopes = new Map<SupportedAppId, Set<MountScope>>();
  const targetScopes = new Map<SupportedAppId, Set<MountScope>>();
  for (const appId of SUPPORTED_APPS) {
    originScopes.set(appId, new Set());
    targetScopes.set(appId, new Set());
  }

  for (const candidate of included) {
    const appId = observationApp(candidate);
    const scope = observationScope(candidate);
    if (appId && scope) originScopes.get(appId)!.add(scope);
    if (appId && scope && preservedObservationIds.includes(candidate.id)) {
      targetScopes.get(appId)!.add(scope);
    }
  }
  for (const target of sharedTargets) {
    const candidate = included.find(
      (item) => item.id === target.sharedObservationId,
    );
    if (candidate) {
      targetScopes
        .get(target.appId)!
        .add(candidate.projectId ? "project" : "global");
    }
  }

  for (const appId of SUPPORTED_APPS) {
    const finalScopes = targetScopes.get(appId)!;
    if (finalScopes.size > 1) {
      return `${supportedAppLabel(appId)} 同时保留了 global 和 project，请只保留一种 scope。`;
    }
    if (originScopes.get(appId)!.size > 1 && finalScopes.size !== 1) {
      return `${supportedAppLabel(appId)} 原本同时存在 global 和 project，必须保留其中一种 scope。`;
    }
  }
  return null;
}

function isShared(candidate: InventoryObservation): boolean {
  return (
    candidate.locationKind === "sharedReadOnly" ||
    candidate.rootKey === "sharedAgents" ||
    candidate.rootKey === "sharedAgentsProject"
  );
}

function observationApp(
  candidate: InventoryObservation,
): SupportedAppId | null {
  if (
    candidate.rootKey === "codexGlobal" ||
    candidate.rootKey === "codexProject"
  ) {
    return "codex";
  }
  if (
    candidate.rootKey === "claudeCodeGlobal" ||
    candidate.rootKey === "claudeCodeProject"
  ) {
    return "claudeCode";
  }
  if (
    candidate.rootKey === "gitHubCopilotGlobal" ||
    candidate.rootKey === "gitHubCopilotProject"
  ) {
    return "gitHubCopilot";
  }
  return null;
}

function observationScope(candidate: InventoryObservation): MountScope | null {
  if (candidate.locationKind === "appGlobal") return "global";
  if (candidate.locationKind === "appProject") return "project";
  return null;
}

function hasPreservedTargetForSharedSelection(
  shared: InventoryObservation,
  appId: SupportedAppId,
  included: InventoryObservation[],
  preservedObservationIds: string[],
): boolean {
  const sharedScope: MountScope = shared.projectId ? "project" : "global";
  return included.some(
    (candidate) =>
      preservedObservationIds.includes(candidate.id) &&
      observationApp(candidate) === appId &&
      observationScope(candidate) === sharedScope &&
      candidate.projectId === shared.projectId,
  );
}

function locationLabel(candidate: InventoryObservation): string {
  if (isShared(candidate)) {
    return candidate.projectDisplayName
      ? `共享目录 · ${candidate.projectDisplayName}`
      : "共享目录";
  }
  const appId = observationApp(candidate);
  const scope = observationScope(candidate);
  const appName = appId ? supportedAppLabel(appId) : "Supported App";
  return scope === "project"
    ? `${appName} · ${candidate.projectDisplayName ?? "已登记项目"}`
    : `${appName} · 全局`;
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}
