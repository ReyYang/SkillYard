import type {
  InventoryObservation,
  MountScope,
  MountSummary,
  ProjectSummary,
  SupportedAppId,
  SupportedAppSummary,
} from "../domain";

interface MountManagementPageProps {
  entry: InventoryObservation;
  supportedApps: SupportedAppSummary[];
  projects: ProjectSummary[];
  mounts: MountSummary[];
  isPlanning: boolean;
  error: string | null;
  onBack(): void;
  onCreate(
    appId: SupportedAppId,
    scope: MountScope,
    projectId: string | null,
  ): void;
  onRemove(mountId: string): void;
  onRepair(mountId: string): void;
}

const SUPPORTED_APPS: Array<{
  id: SupportedAppId;
  displayName: string;
}> = [
  { id: "codex", displayName: "Codex" },
  { id: "claudeCode", displayName: "Claude Code" },
  { id: "gitHubCopilot", displayName: "GitHub Copilot" },
];

export function MountManagementPage({
  entry,
  supportedApps,
  projects,
  mounts,
  isPlanning,
  error,
  onBack,
  onCreate,
  onRemove,
  onRepair,
}: MountManagementPageProps) {
  return (
    <main className="mount-shell">
      <p className="eyebrow">SKILLYARD · SUPPORTED APP MOUNT</p>
      <h1>管理挂载</h1>
      <p className="lead">
        {entry.bundleDisplayName
          ? `${entry.bundleDisplayName}: ${entry.skillName}`
          : entry.skillName}
      </p>

      {error ? (
        <div className="inline-error" role="alert">
          <strong>无法准备挂载</strong>
          <span>{error}</span>
        </div>
      ) : null}

      {SUPPORTED_APPS.map((app) => {
        const appMounts = mounts.filter(
          (mount) => mount.memberId === entry.memberId && mount.appId === app.id,
        );
        const hasGlobalMount = appMounts.some(
          (mount) => mount.scope === "global",
        );
        const hasProjectMount = appMounts.some(
          (mount) => mount.scope === "project",
        );
        const detected = supportedApps.find(
          (summary) => summary.id === app.id,
        )?.detected;

        return (
          <section
            key={app.id}
            className="mount-panel"
            aria-label={`${app.displayName} 挂载`}
          >
            <div className="mount-app-heading">
              <div>
                <p className="section-eyebrow">SUPPORTED APP</p>
                <h2>{app.displayName}</h2>
              </div>
              <span>{detectionLabel(detected)}</span>
            </div>

            <h3>当前使用位置</h3>
            {appMounts.length === 0 ? (
              <p className="mount-empty-copy">
                {`这个 Skill 目前没有挂载到 ${app.displayName}。`}
              </p>
            ) : (
              <ul className="mount-list">
                {appMounts.map((mount) => (
                  <li key={mount.id}>
                    <div>
                      <strong>{mountDestinationLabel(mount)}</strong>
                      <span>{mountHealthCopy(mount)}</span>
                      <code title={mount.targetPath}>{mount.targetPath}</code>
                    </div>
                    <button
                      className="compact-action"
                      type="button"
                      hidden={mount.health !== "missing"}
                      disabled={isPlanning}
                      onClick={() => onRepair(mount.id)}
                    >
                      {`修复 ${mountDestinationLabel(mount)}挂载`}
                    </button>
                    <button
                      className="danger-outline-action"
                      type="button"
                      disabled={isPlanning}
                      onClick={() => onRemove(mount.id)}
                    >
                      {`移除 ${mountDestinationLabel(mount)}挂载`}
                    </button>
                  </li>
                ))}
              </ul>
            )}

            <h3>新增使用位置</h3>
            <div className="mount-target-list">
              <button
                className="mount-target"
                type="button"
                aria-label={`挂载到 ${app.displayName} 全局`}
                disabled={isPlanning || hasGlobalMount || hasProjectMount}
                onClick={() => onCreate(app.id, "global", null)}
              >
                <strong>{`挂载到 ${app.displayName} 全局`}</strong>
                <span>{`在这台 Mac 的所有 ${app.displayName} 项目中可用`}</span>
              </button>
              {projects.map((project) => {
                const alreadyMounted = appMounts.some(
                  (mount) =>
                    mount.scope === "project" && mount.projectId === project.id,
                );
                return (
                  <button
                    key={project.id}
                    className="mount-target"
                    type="button"
                    aria-label={`挂载到 ${app.displayName} 项目 ${project.displayName}`}
                    disabled={isPlanning || hasGlobalMount || alreadyMounted}
                    onClick={() => onCreate(app.id, "project", project.id)}
                  >
                    <strong>{`挂载到项目 ${project.displayName}`}</strong>
                    <code title={project.rootPath}>{project.rootPath}</code>
                  </button>
                );
              })}
            </div>
            {projects.length === 0 ? (
              <p className="mount-project-hint">
                还没有已登记项目。返回清单后可通过“添加项目”选择本地目录。
              </p>
            ) : null}
            {app.id === "claudeCode" && projects.length > 0 ? (
              <p className="mount-project-hint">
                Claude Code 的项目挂载位于 <code>.claude/skills</code>；GitHub
                Copilot 也可能读取这个位置。
              </p>
            ) : null}
            {hasGlobalMount ? (
              <p className="mount-project-hint">
                {`同一 Skill 不能同时使用 ${app.displayName} global 与 project Mount；请先移除全局挂载。`}
              </p>
            ) : null}
            {hasProjectMount && !hasGlobalMount ? (
              <p className="mount-project-hint">
                已有 project Mount 时不能再创建 global Mount，但可以继续添加其他项目。
              </p>
            ) : null}
          </section>
        );
      })}

      <div className="install-actions">
        <button
          className="secondary-action"
          type="button"
          disabled={isPlanning}
          onClick={onBack}
        >
          {projects.length === 0 ? "返回添加项目" : "返回清单"}
        </button>
      </div>
    </main>
  );
}

function detectionLabel(detected: boolean | null | undefined): string {
  if (detected === true) return "已检测到";
  if (detected === false) return "未检测到";
  return "尚未检测";
}

function mountDestinationLabel(mount: MountSummary): string {
  const appName = supportedAppLabel(mount.appId);
  return mount.scope === "global"
    ? `${appName} 全局`
    : `${appName} 项目 ${mount.projectDisplayName ?? "已登记项目"}`;
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}

function mountHealthCopy(mount: MountSummary): string {
  return {
    healthy: "软链接正常",
    missing: "软链接已缺失；移除时只清理 SkillYard 记录",
    conflict: "目标路径无法安全确认；移除只清理 SkillYard 记录",
  }[mount.health];
}
