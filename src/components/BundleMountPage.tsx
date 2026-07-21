import { useMemo, useState } from "react";

import type {
  BatchMountRequest,
  ProjectSummary,
  SupportedAppId,
  SupportedAppSummary,
} from "../domain";

interface BundleMountPageProps {
  bundleDisplayName: string;
  members: Array<{ memberId: string; skillName: string }>;
  supportedApps: SupportedAppSummary[];
  projects: ProjectSummary[];
  isPlanning: boolean;
  error: string | null;
  onBack(): void;
  onCreatePlan(requests: BatchMountRequest[]): void;
}

interface AppSelection {
  global: boolean;
  projectIds: string[];
}

const SUPPORTED_APPS: Array<{
  id: SupportedAppId;
  displayName: string;
}> = [
  { id: "codex", displayName: "Codex" },
  { id: "claudeCode", displayName: "Claude Code" },
  { id: "gitHubCopilot", displayName: "GitHub Copilot" },
];

const EMPTY_SELECTIONS: Record<SupportedAppId, AppSelection> = {
  codex: { global: false, projectIds: [] },
  claudeCode: { global: false, projectIds: [] },
  gitHubCopilot: { global: false, projectIds: [] },
};

export function BundleMountPage({
  bundleDisplayName,
  members,
  supportedApps,
  projects,
  isPlanning,
  error,
  onBack,
  onCreatePlan,
}: BundleMountPageProps) {
  const [selections, setSelections] =
    useState<Record<SupportedAppId, AppSelection>>(EMPTY_SELECTIONS);

  // Bundle 批量入口始终使用完整成员集合，不能继承清单页的搜索结果。
  const uniqueMembers = useMemo(
    () => [
      ...new Map(
        members.map((member) => [member.memberId, member]),
      ).values(),
    ],
    [members],
  );
  const requests = useMemo(
    () => buildRequests(uniqueMembers, projects, selections),
    [projects, selections, uniqueMembers],
  );

  const toggleGlobal = (appId: SupportedAppId, checked: boolean) => {
    setSelections((current) => ({
      ...current,
      // 选择 global 时清空 project，确保同一应用不会生成重叠 scope。
      [appId]: {
        global: checked,
        projectIds: checked ? [] : current[appId].projectIds,
      },
    }));
  };

  const toggleProject = (
    appId: SupportedAppId,
    projectId: string,
    checked: boolean,
  ) => {
    setSelections((current) => {
      const projectIds = checked
        ? [...new Set([...current[appId].projectIds, projectId])]
        : current[appId].projectIds.filter((id) => id !== projectId);
      return {
        ...current,
        // 任一 project 被选择后都取消 global；多个不同 Project 可以共存。
        [appId]: { global: false, projectIds },
      };
    });
  };

  return (
    <main className="mount-shell">
      <p className="eyebrow">SKILLYARD · BATCH MOUNT TARGETS</p>
      <h1>{`批量挂载 ${bundleDisplayName}`}</h1>
      <p className="lead">本 Bundle 的 {uniqueMembers.length} 个 Skill 将全部参与</p>
      <p className="batch-intro">
        先选择使用位置，再由 SkillYard 检查每个 Skill 的精确路径。此处不会立即创建挂载。
      </p>

      {error ? (
        <div className="inline-error" role="alert">
          <strong>无法生成批量挂载预览</strong>
          <span>{error}</span>
        </div>
      ) : null}

      <section className="batch-member-summary" aria-label="Bundle 全部成员">
        <p className="section-eyebrow">BUNDLE MEMBERS</p>
        <ul>
          {uniqueMembers.map((member) => (
            <li key={member.memberId}>{member.skillName}</li>
          ))}
        </ul>
      </section>

      {SUPPORTED_APPS.map((app) => {
        const detected = supportedApps.find(
          (summary) => summary.id === app.id,
        )?.detected;
        const selection = selections[app.id];
        return (
          <section
            key={app.id}
            className="mount-panel"
            aria-label={`${app.displayName} 批量挂载目标`}
          >
            <div className="mount-app-heading">
              <div>
                <p className="section-eyebrow">SUPPORTED APP</p>
                <h2>{app.displayName}</h2>
              </div>
              <span>{detectionLabel(detected)}</span>
            </div>
            <div className="batch-target-list">
              <label className="batch-target-option">
                <input
                  type="checkbox"
                  aria-label={`${app.displayName} 全局`}
                  checked={selection.global}
                  disabled={isPlanning}
                  onChange={(event) =>
                    toggleGlobal(app.id, event.target.checked)
                  }
                />
                <span>
                  <strong>{`${app.displayName} 全局`}</strong>
                  <small>{`在这台 Mac 的所有 ${app.displayName} 项目中可用`}</small>
                </span>
              </label>
              {projects.map((project) => (
                <label key={project.id} className="batch-target-option">
                  <input
                    type="checkbox"
                    aria-label={`${app.displayName} 项目 ${project.displayName}`}
                    checked={selection.projectIds.includes(project.id)}
                    disabled={isPlanning}
                    onChange={(event) =>
                      toggleProject(app.id, project.id, event.target.checked)
                    }
                  />
                  <span>
                    <strong>{`${app.displayName} 项目 ${project.displayName}`}</strong>
                    <code title={project.rootPath}>{project.rootPath}</code>
                  </span>
                </label>
              ))}
            </div>
            {projects.length === 0 ? (
              <p className="mount-project-hint">
                还没有已登记项目；可返回清单后通过“添加项目”选择本地目录。
              </p>
            ) : null}
            {app.id === "claudeCode" && projects.length > 0 ? (
              <p className="mount-project-hint">
                Claude Code 的项目挂载位于 <code>.claude/skills</code>；GitHub
                Copilot 也可能读取这个位置。
              </p>
            ) : null}
          </section>
        );
      })}

      {requests.length === 0 ? (
        <p className="install-selection-empty">至少选择一个应用或项目目标。</p>
      ) : null}
      <p className="mount-confirm-warning">
        下一步只生成影响预览；最终确认后，所选 Mount 会全部完成或全部撤销。
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
          disabled={isPlanning || requests.length === 0}
          onClick={() => onCreatePlan(requests)}
        >
          {isPlanning ? "正在检查目标…" : "生成影响预览"}
        </button>
      </div>
    </main>
  );
}

function buildRequests(
  members: Array<{ memberId: string; skillName: string }>,
  projects: ProjectSummary[],
  selections: Record<SupportedAppId, AppSelection>,
): BatchMountRequest[] {
  type BatchMountTarget = Omit<BatchMountRequest, "memberId">;
  const targets = SUPPORTED_APPS.flatMap<BatchMountTarget>((app) => {
    const selection = selections[app.id];
    if (selection.global) {
      return [
        {
          appId: app.id,
          scope: "global",
          projectId: null,
        },
      ];
    }
    return projects
      .filter((project) => selection.projectIds.includes(project.id))
      .map((project) => ({
        appId: app.id,
        scope: "project" as const,
        projectId: project.id,
      }));
  });

  const requests = members.flatMap((member) =>
    targets.map((target) => ({ memberId: member.memberId, ...target })),
  );
  // 即使后端 read model 意外重复投影 Project，也不能向 IPC 发送重复 Mount 请求。
  return [
    ...new Map(
      requests.map((request) => [
        `${request.memberId}\u0000${request.appId}\u0000${request.scope}\u0000${request.projectId ?? ""}`,
        request,
      ]),
    ).values(),
  ];
}

function detectionLabel(detected: boolean | null | undefined): string {
  if (detected === true) return "已检测到";
  if (detected === false) return "未检测到";
  return "尚未检测";
}
