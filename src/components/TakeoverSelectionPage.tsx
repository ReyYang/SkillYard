import { useMemo, useState } from "react";

import type {
  InventoryObservation,
  MountScope,
  SupportedAppId,
  TakeoverMemberRequest,
  TakeoverPlanRequest,
  TakeoverSharedTargetRequest,
} from "../domain";
import { useI18n } from "../i18n";
import { PageBackButton } from "./PageBackButton";

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
  const { t } = useI18n();
  const initial = candidates.find(
    (candidate) => candidate.id === initialObservationId,
  )!;
  const isBundle = Boolean(initial.takeoverGroupId);
  const bundleName = initial.takeoverGroupDisplayName ?? t("本地 Bundle");
  const allMemberGroups = useMemo(() => groupMembers(candidates), [candidates]);
  const initialIncludedIds = isBundle
    ? allMemberGroups
        .filter((member) =>
          isValidBundleMember(member, initial.takeoverGroupId),
        )
        .flatMap((member) =>
          bundleEvidenceObservations(
            member,
            initial.takeoverGroupId,
          ).map((candidate) => candidate.id),
        )
    : [initial.id];
  const [includedIds, setIncludedIds] = useState<string[]>(() =>
    initialIncludedIds,
  );
  const [selectedBySkill, setSelectedBySkill] = useState<
    Record<string, string | null>
  >(() =>
    createInitialSelections(
      candidates.filter((candidate) =>
        initialIncludedIds.includes(candidate.id),
      ),
    ),
  );
  const [preservedObservationIds, setPreservedObservationIds] = useState<
    string[]
  >(() =>
    candidates
      .filter((candidate) => initialIncludedIds.includes(candidate.id))
      .filter((candidate) => !isShared(candidate))
      .map((candidate) => candidate.id),
  );
  const [sharedTargets, setSharedTargets] = useState<
    TakeoverSharedTargetRequest[]
  >([]);

  const included = useMemo(
    () => candidates.filter((candidate) => includedIds.includes(candidate.id)),
    [candidates, includedIds],
  );
  const memberGroups = useMemo(() => groupMembers(included), [included]);
  const differentContentMembers = memberGroups.filter(
    (member) =>
      new Set(
        member.observations.map(
          (candidate) => candidate.observedFingerprint,
        ),
      ).size > 1,
  );
  const invalidMetadata = included.some(
    (candidate) => candidate.metadataStatus !== "valid",
  );
  const invalidBundleMembers = isBundle
    ? allMemberGroups.filter(
        (member) =>
          !isValidBundleMember(member, initial.takeoverGroupId),
      )
    : [];
  const sharedWithoutTarget = included.some(
    (candidate) =>
      isShared(candidate) &&
      !sharedTargets.some(
        (target) => target.sharedObservationId === candidate.id,
      ),
  );
  const scopeIssue =
    memberGroups
      .map((member) =>
        findScopeIssue(
          member.observations,
          preservedObservationIds,
          sharedTargets,
          t,
        ),
      )
      .find((issue) => issue !== null) ?? null;
  const canCreatePlan =
    memberGroups.length > 0 &&
    memberGroups.every(
      (member) => selectedBySkill[member.skillName] !== null,
    ) &&
    !invalidMetadata &&
    !sharedWithoutTarget &&
    scopeIssue === null;

  const toggleIdentity = (
    candidate: InventoryObservation,
    checked: boolean,
  ) => {
    if (
      candidate.id === initialObservationId ||
      (isBundle && candidate.takeoverGroupId)
    ) {
      return;
    }
    const nextIds = checked
      ? [...includedIds, candidate.id]
      : includedIds.filter((id) => id !== candidate.id);
    const remaining = candidates.filter((item) => nextIds.includes(item.id));
    setIncludedIds(nextIds);
    setPreservedObservationIds((current) =>
      checked && !isShared(candidate)
        ? [...new Set([...current, candidate.id])]
        : current.filter((id) => id !== candidate.id),
    );
    setSharedTargets((current) =>
      checked
        ? current
        : current.filter(
            (target) => target.sharedObservationId !== candidate.id,
          ),
    );
    // 未分组候选仍靠人工确认同名副本；内容变化后必须重新明确主副本。
    setSelectedBySkill((current) => ({
      ...current,
      [candidate.skillName]: checked
        ? defaultSelectionFor(
            remaining.filter(
              (item) => item.skillName === candidate.skillName,
            ),
            null,
          )
        : defaultSelectionFor(
            remaining.filter(
              (item) => item.skillName === candidate.skillName,
            ),
            current[candidate.skillName],
          ),
    }));
  };

  const toggleBundleMember = (
    member: ReturnType<typeof groupMembers>[number],
    checked: boolean,
  ) => {
    if (
      !isBundle ||
      !isValidBundleMember(member, initial.takeoverGroupId)
    ) {
      return;
    }
    const memberIds = member.observations.map((candidate) => candidate.id);
    const evidenceObservations = bundleEvidenceObservations(
      member,
      initial.takeoverGroupId,
    );
    const evidenceIds = evidenceObservations.map((candidate) => candidate.id);
    setIncludedIds((current) =>
      checked
        ? [...new Set([...current, ...evidenceIds])]
        : current.filter((id) => !memberIds.includes(id)),
    );
    setPreservedObservationIds((current) =>
      checked
        ? [
            ...new Set([
              ...current,
              ...evidenceObservations
                .filter((candidate) => !isShared(candidate))
                .map((candidate) => candidate.id),
            ]),
          ]
        : current.filter((id) => !memberIds.includes(id)),
    );
    setSharedTargets((current) =>
      checked
        ? current
        : current.filter(
            (target) => !memberIds.includes(target.sharedObservationId),
          ),
    );
    setSelectedBySkill((current) => ({
      ...current,
      [member.skillName]: checked
        ? defaultSelectionFor(
            evidenceObservations,
            current[member.skillName],
          )
        : null,
    }));
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
      updateSharedTargets(current, [{ sharedObservationId, appId }], checked),
    );
  };

  const toggleBundleSharedTarget = (
    appId: SupportedAppId,
    checked: boolean,
  ) => {
    const compatibleTargets = included
      .filter(
        (candidate) =>
          isShared(candidate) && candidate.observedBy.includes(appId),
      )
      .map((candidate) => ({
        sharedObservationId: candidate.id,
        appId,
      }));
    // Bundle 级选择展开成后端需要的精确 observation-app 对，避免逐成员重复勾选。
    setSharedTargets((current) =>
      updateSharedTargets(current, compatibleTargets, checked),
    );
  };

  const createPlan = () => {
    if (!canCreatePlan) return;
    const members: TakeoverMemberRequest[] = memberGroups.map((member) => ({
      observationIds: member.observations.map((candidate) => candidate.id),
      selectedObservationId: selectedBySkill[member.skillName]!,
      preservedObservationIds: member.observations
        .filter((candidate) =>
          preservedObservationIds.includes(candidate.id),
        )
        .map((candidate) => candidate.id),
    }));
    onCreatePlan({
      members,
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
      <PageBackButton disabled={isPlanning} onClick={onBack} />
      <p className="eyebrow">SKILLYARD · TAKEOVER</p>
      <h1>
        {isBundle
          ? t("选择要接管的 Bundle：{bundle}", { bundle: bundleName })
          : t("选择要接管的 {skill}", { skill: initial.skillName })}
      </h1>
      <p className="lead">
        {isBundle
          ? t(
              "确定性来源证据已经把这些 Skill 识别为同一个 Bundle。接管前只生成整组影响预览。",
            )
          : t(
              "接管前只生成影响预览。勾选其他同名位置，表示你确认它们是同一个 Skill；同名本身不会触发自动合并。",
            )}
      </p>

      {error ? (
        <div className="inline-error" role="alert">
          <strong>{t("无法生成接管预览")}</strong>
          <span>{error}</span>
        </div>
      ) : null}

      <section
        className="batch-member-summary"
        aria-label={
          isBundle ? t("Bundle 成员") : t("确认同一个 Skill")
        }
      >
        <p className="section-eyebrow">
          {isBundle ? "BUNDLE MEMBERS" : "LOCAL IDENTITY"}
        </p>
        <h2>
          {isBundle
            ? t("将一起接管的 Skill")
            : t("确认属于同一个 Skill 的位置")}
        </h2>
        <div className="batch-target-list">
          {isBundle
            ? allMemberGroups.map((member) => (
                <label className="batch-target-option" key={member.skillName}>
                  <input
                    type="checkbox"
                    aria-label={t("接管 Bundle 成员：{skill}", {
                      skill: member.skillName,
                    })}
                    checked={bundleEvidenceObservations(
                      member,
                      initial.takeoverGroupId,
                    ).every((candidate) =>
                      includedIds.includes(candidate.id),
                    )}
                    disabled={
                      isPlanning ||
                      !isValidBundleMember(
                        member,
                        initial.takeoverGroupId,
                      )
                    }
                    onChange={(event) =>
                      toggleBundleMember(member, event.target.checked)
                    }
                  />
                  <span>
                    <strong>{member.skillName}</strong>
                    <small>
                      {isValidBundleMember(
                        member,
                        initial.takeoverGroupId,
                      )
                        ? t("{count} 个已确认安装位置", {
                            count: bundleEvidenceObservations(
                              member,
                              initial.takeoverGroupId,
                            ).length,
                          })
                        : t("Skill metadata 无效，本次不会接管")}
                    </small>
                  </span>
                </label>
              ))
            : candidates.map((candidate) => (
                <label className="batch-target-option" key={candidate.id}>
                  <input
                    type="checkbox"
                    aria-label={t("确认同一 Skill：{path}", {
                      path: candidate.skillRoot,
                    })}
                    checked={includedIds.includes(candidate.id)}
                    disabled={
                      isPlanning || candidate.id === initialObservationId
                    }
                    onChange={(event) =>
                      toggleIdentity(candidate, event.target.checked)
                    }
                  />
                  <span>
                    <strong>{locationLabel(candidate, t)}</strong>
                    <code title={candidate.skillRoot}>
                      {candidate.skillRoot}
                    </code>
                  </span>
                </label>
              ))}
        </div>
        {isBundle &&
        allMemberGroups.some((member) =>
          member.observations.some(
            (candidate) => !candidate.takeoverGroupId,
          ),
        ) ? (
          <>
            <p>
              {t(
                "以下同名位置没有安装组证据，只有你明确确认后才会并入对应 Member。",
              )}
            </p>
            <div className="batch-target-list">
              {allMemberGroups.flatMap((member) =>
                member.observations
                  .filter((candidate) => !candidate.takeoverGroupId)
                  .map((candidate) => {
                    const memberIncluded = bundleEvidenceObservations(
                      member,
                      initial.takeoverGroupId,
                    ).every((observation) =>
                      includedIds.includes(observation.id),
                    );
                    return (
                      <label
                        className="batch-target-option"
                        key={candidate.id}
                      >
                        <input
                          type="checkbox"
                          aria-label={t("确认同一 Skill：{path}", {
                            path: candidate.skillRoot,
                          })}
                          checked={includedIds.includes(candidate.id)}
                          disabled={
                            isPlanning ||
                            !memberIncluded ||
                            candidate.metadataStatus !== "valid"
                          }
                          onChange={(event) =>
                            toggleIdentity(
                              candidate,
                              event.target.checked,
                            )
                          }
                        />
                        <span>
                          <strong>{`${candidate.skillName} · ${locationLabel(
                            candidate,
                            t,
                          )}`}</strong>
                          <code title={candidate.skillRoot}>
                            {candidate.skillRoot}
                          </code>
                        </span>
                      </label>
                    );
                  }),
              )}
            </div>
          </>
        ) : null}
      </section>

      {differentContentMembers.map((member) => (
        <section
          className="batch-member-summary"
          aria-label={t("选择 {skill} 的唯一内容", {
            skill: member.skillName,
          })}
          key={member.skillName}
        >
          <p className="section-eyebrow">PRIMARY CONTENT</p>
          <h2>
            {isBundle
              ? t("请选择 {skill} 的唯一一份内容", {
                  skill: member.skillName,
                })
              : t("请选择唯一一份内容")}
          </h2>
          <p>
            {t(
              "该成员的其他位置会统一使用这份内容，不会保留为可选旧版本。",
            )}
          </p>
          <div className="batch-target-list">
            {member.observations.map((candidate) => (
              <label className="batch-target-option" key={candidate.id}>
                <input
                  type="radio"
                  name={`takeover-primary-content-${member.skillName}`}
                  aria-label={t("使用 {path} 作为主副本", {
                    path: candidate.skillRoot,
                  })}
                  checked={
                    selectedBySkill[member.skillName] === candidate.id
                  }
                  disabled={isPlanning}
                  onChange={() =>
                    setSelectedBySkill((current) => ({
                      ...current,
                      [member.skillName]: candidate.id,
                    }))
                  }
                />
                <span>
                  <strong>{locationLabel(candidate, t)}</strong>
                  <code title={candidate.skillRoot}>
                    {candidate.skillRoot}
                  </code>
                </span>
              </label>
            ))}
          </div>
        </section>
      ))}

      {included.some((candidate) => !isShared(candidate)) ? (
        <section
          className="batch-member-summary"
          aria-label={t("保留现有使用位置")}
        >
          <p className="section-eyebrow">EXISTING MOUNTS</p>
          <h2>{t("保留哪些现有使用位置")}</h2>
          <p>
            {t("取消后，该原位置会在接管成功时移除，不会建立 Mount。")}
          </p>
          <div className="batch-target-list">
            {included
              .filter((candidate) => !isShared(candidate))
              .map((candidate) => (
                <label className="batch-target-option" key={candidate.id}>
                  <input
                    type="checkbox"
                    aria-label={t("保留使用位置：{path}", {
                      path: candidate.skillRoot,
                    })}
                    checked={preservedObservationIds.includes(candidate.id)}
                    disabled={isPlanning}
                    onChange={(event) =>
                      togglePreserved(candidate.id, event.target.checked)
                    }
                  />
                  <span>
                    <strong>{`${candidate.skillName} · ${locationLabel(
                      candidate,
                      t,
                    )}`}</strong>
                    <code title={candidate.skillRoot}>
                      {candidate.skillRoot}
                    </code>
                  </span>
                </label>
              ))}
          </div>
        </section>
      ) : null}

      {isBundle
        ? renderBundleSharedTargets(
            included,
            sharedTargets,
            isPlanning,
            toggleBundleSharedTarget,
            toggleSharedTarget,
            t,
          )
        : included.filter(isShared).map((candidate) => (
            <section
              className="batch-member-summary"
              aria-label={t("共享目录目标 {path}", {
                path: candidate.skillRoot,
              })}
              key={candidate.id}
            >
              <p className="section-eyebrow">SHARED DIRECTORY</p>
              <h2>{t("选择共享目录对应的应用")}</h2>
              <code title={candidate.skillRoot}>{candidate.skillRoot}</code>
              <p>
                {t(
                  "原共享入口会在全部应用专属 Mount 验证成功后移除；未选择的应用可能不再发现此 Skill。",
                )}
              </p>
              <div className="batch-target-list">
                {candidate.observedBy.map((appId) => (
                  <label className="batch-target-option" key={appId}>
                    <input
                      type="checkbox"
                      aria-label={t("将 {path} 挂载到 {app}", {
                        path: candidate.skillRoot,
                        app: supportedAppLabel(appId),
                      })}
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
                      <small>
                        {t("SkillYard 将使用该应用的固定专属 Skill 目录")}
                      </small>
                    </span>
                  </label>
                ))}
              </div>
            </section>
          ))}

      {invalidMetadata ? (
        <p className="install-selection-empty">
          {t("所选位置包含无效 Skill metadata，刷新或修复后才能接管。")}
        </p>
      ) : null}
      {invalidBundleMembers.length > 0 ? (
        <p className="install-selection-empty">
          {t("有 {count} 个无效成员未加入计划；其他有效成员仍可接管。", {
            count: invalidBundleMembers.length,
          })}
        </p>
      ) : null}
      {sharedWithoutTarget ? (
        <p className="install-selection-empty">
          {t("共享目录必须选择至少一个应用。")}
        </p>
      ) : null}
      {scopeIssue ? (
        <p className="install-selection-empty">{scopeIssue}</p>
      ) : null}
      <p className="mount-confirm-warning">
        {t("下一步由 Rust 重新检查路径并封存影响预览，此时仍不会修改文件。")}
      </p>
      <div className="install-actions">
        <button
          className="primary-action"
          type="button"
          disabled={isPlanning || !canCreatePlan}
          onClick={createPlan}
        >
          {isPlanning ? t("正在检查现有安装…") : t("生成影响预览")}
        </button>
      </div>
    </main>
  );
}

function createInitialSelections(
  candidates: InventoryObservation[],
): Record<string, string | null> {
  return Object.fromEntries(
    groupMembers(candidates).map((member) => [
      member.skillName,
      defaultSelectionFor(member.observations, null),
    ]),
  );
}

function defaultSelectionFor(
  observations: InventoryObservation[],
  current: string | null | undefined,
): string | null {
  if (
    current &&
    observations.some((observation) => observation.id === current)
  ) {
    return current;
  }
  return new Set(
    observations.map((observation) => observation.observedFingerprint),
  ).size === 1
    ? (observations[0]?.id ?? null)
    : null;
}

function groupMembers(candidates: InventoryObservation[]): Array<{
  skillName: string;
  observations: InventoryObservation[];
}> {
  const groups = new Map<string, InventoryObservation[]>();
  for (const candidate of candidates) {
    groups.set(candidate.skillName, [
      ...(groups.get(candidate.skillName) ?? []),
      candidate,
    ]);
  }
  return [...groups].map(([skillName, observations]) => ({
    skillName,
    observations,
  }));
}

function bundleEvidenceObservations(
  member: ReturnType<typeof groupMembers>[number],
  groupId: string | null,
): InventoryObservation[] {
  return member.observations.filter(
    (candidate) => candidate.takeoverGroupId === groupId,
  );
}

function isValidBundleMember(
  member: ReturnType<typeof groupMembers>[number],
  groupId: string | null,
): boolean {
  const observations = bundleEvidenceObservations(member, groupId);
  return observations.length > 0 && observations.every(
    (candidate) => candidate.metadataStatus === "valid",
  );
}

function updateSharedTargets(
  current: TakeoverSharedTargetRequest[],
  targets: TakeoverSharedTargetRequest[],
  checked: boolean,
): TakeoverSharedTargetRequest[] {
  if (checked) {
    const keys = new Set(
      current.map(
        (target) => `${target.sharedObservationId}:${target.appId}`,
      ),
    );
    return [
      ...current,
      ...targets.filter(
        (target) =>
          !keys.has(`${target.sharedObservationId}:${target.appId}`),
      ),
    ];
  }
  const removed = new Set(
    targets.map((target) => `${target.sharedObservationId}:${target.appId}`),
  );
  return current.filter(
    (target) =>
      !removed.has(`${target.sharedObservationId}:${target.appId}`),
  );
}

function renderBundleSharedTargets(
  included: InventoryObservation[],
  sharedTargets: TakeoverSharedTargetRequest[],
  isPlanning: boolean,
  onToggle: (appId: SupportedAppId, checked: boolean) => void,
  onToggleMember: (
    sharedObservationId: string,
    appId: SupportedAppId,
    checked: boolean,
  ) => void,
  t: ReturnType<typeof useI18n>["t"],
) {
  const shared = included.filter(isShared);
  if (shared.length === 0) return null;
  const compatibleApps = SUPPORTED_APPS.filter((appId) =>
    shared.some((candidate) => candidate.observedBy.includes(appId)),
  );
  return (
    <section
      className="batch-member-summary"
      aria-label={t("Bundle 共享目录目标")}
    >
      <p className="section-eyebrow">SHARED DIRECTORIES</p>
      <h2>{t("一次选择 Bundle 的 Supported App")}</h2>
      <p>{t("上方批量选择会应用到全部兼容成员，也可以在下方逐个调整。")}</p>
      <div className="batch-target-list">
        {compatibleApps.map((appId) => {
          const compatible = shared.filter((candidate) =>
            candidate.observedBy.includes(appId),
          );
          return (
            <label className="batch-target-option" key={appId}>
              <input
                type="checkbox"
                aria-label={t("将 Bundle 中的共享目录挂载到 {app}", {
                  app: supportedAppLabel(appId),
                })}
                checked={compatible.every((candidate) =>
                  sharedTargets.some(
                    (target) =>
                      target.sharedObservationId === candidate.id &&
                      target.appId === appId,
                  ),
                )}
                disabled={isPlanning}
                onChange={(event) => onToggle(appId, event.target.checked)}
              />
              <span>
                <strong>{supportedAppLabel(appId)}</strong>
                <small>
                  {t("应用到 {count} 个兼容成员", {
                    count: compatible.length,
                  })}
                </small>
              </span>
            </label>
          );
        })}
      </div>
      <div className="batch-target-list">
        {shared.map((candidate) => (
          <div
            className="batch-target-option takeover-member-target-option"
            key={candidate.id}
          >
            <span>
              <strong>{candidate.skillName}</strong>
              <small>{candidate.skillRoot}</small>
            </span>
            <div className="takeover-member-targets">
              {candidate.observedBy.map((appId) => (
                <label key={appId}>
                  <input
                    type="checkbox"
                    aria-label={t("将 {skill} 挂载到 {app}", {
                      skill: candidate.skillName,
                      app: supportedAppLabel(appId),
                    })}
                    checked={sharedTargets.some(
                      (target) =>
                        target.sharedObservationId === candidate.id &&
                        target.appId === appId,
                    )}
                    disabled={isPlanning}
                    onChange={(event) =>
                      onToggleMember(
                        candidate.id,
                        appId,
                        event.target.checked,
                      )
                    }
                  />
                  <span>{supportedAppLabel(appId)}</span>
                </label>
              ))}
            </div>
          </div>
        ))}
      </div>
    </section>
  );
}

function findScopeIssue(
  included: InventoryObservation[],
  preservedObservationIds: string[],
  sharedTargets: TakeoverSharedTargetRequest[],
  t: ReturnType<typeof useI18n>["t"],
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
      return t("{app} 同时保留了 global 和 project，请只保留一种 scope。", {
        app: supportedAppLabel(appId),
      });
    }
    if (originScopes.get(appId)!.size > 1 && finalScopes.size !== 1) {
      return t(
        "{app} 原本同时存在 global 和 project，必须保留其中一种 scope。",
        { app: supportedAppLabel(appId) },
      );
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
      candidate.skillName === shared.skillName &&
      observationApp(candidate) === appId &&
      observationScope(candidate) === sharedScope &&
      candidate.projectId === shared.projectId,
  );
}

function locationLabel(
  candidate: InventoryObservation,
  t: ReturnType<typeof useI18n>["t"],
): string {
  if (isShared(candidate)) {
    return candidate.projectDisplayName
      ? t("共享目录 · {project}", {
          project: candidate.projectDisplayName,
        })
      : t("共享目录");
  }
  const appId = observationApp(candidate);
  const scope = observationScope(candidate);
  const appName = appId ? supportedAppLabel(appId) : "Supported App";
  return scope === "project"
    ? t("{app} · {project}", {
        app: appName,
        project: candidate.projectDisplayName ?? t("已登记项目"),
      })
    : t("{app} · 全局", { app: appName });
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}
