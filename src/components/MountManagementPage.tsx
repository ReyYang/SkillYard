import type {
  InventoryObservation,
  MountScope,
  MountSummary,
  ProjectSummary,
} from "../domain";

interface MountManagementPageProps {
  entry: InventoryObservation;
  projects: ProjectSummary[];
  mounts: MountSummary[];
  isPlanning: boolean;
  error: string | null;
  onBack(): void;
  onCreate(scope: MountScope, projectId: string | null): void;
  onRemove(mountId: string): void;
}

export function MountManagementPage({
  entry,
  projects,
  mounts,
  isPlanning,
  error,
  onBack,
  onCreate,
  onRemove,
}: MountManagementPageProps) {
  const codexMounts = mounts.filter(
    (mount) => mount.memberId === entry.memberId && mount.appId === "codex",
  );
  const hasGlobalMount = codexMounts.some((mount) => mount.scope === "global");
  const hasProjectMount = codexMounts.some((mount) => mount.scope === "project");

  return (
    <main className="mount-shell">
      <p className="eyebrow">SKILLYARD · CODEX MOUNT</p>
      <h1>管理 Codex 挂载</h1>
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

      <section className="mount-panel" aria-label="当前 Codex 挂载">
        <p className="section-eyebrow">CURRENT</p>
        <h2>当前使用位置</h2>
        {codexMounts.length === 0 ? (
          <p className="mount-empty-copy">这个 Skill 目前没有挂载到 Codex。</p>
        ) : (
          <ul className="mount-list">
            {codexMounts.map((mount) => (
              <li key={mount.id}>
                <div>
                  <strong>{mountDestinationLabel(mount)}</strong>
                  <span>{mountHealthCopy(mount)}</span>
                  <code title={mount.targetPath}>{mount.targetPath}</code>
                </div>
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
      </section>

      <section className="mount-panel" aria-label="新增 Codex 挂载">
        <p className="section-eyebrow">ADD</p>
        <h2>选择新的使用位置</h2>
        <div className="mount-target-list">
          <button
            className="mount-target"
            type="button"
            aria-label="挂载到 Codex 全局"
            disabled={isPlanning || hasGlobalMount || hasProjectMount}
            onClick={() => onCreate("global", null)}
          >
            <strong>挂载到 Codex 全局</strong>
            <span>在这台 Mac 的所有 Codex 项目中可用</span>
          </button>
          {projects.map((project) => {
            const alreadyMounted = codexMounts.some(
              (mount) =>
                mount.scope === "project" && mount.projectId === project.id,
            );
            return (
              <button
                key={project.id}
                className="mount-target"
                type="button"
                aria-label={`挂载到项目 ${project.displayName}`}
                disabled={isPlanning || hasGlobalMount || alreadyMounted}
                onClick={() => onCreate("project", project.id)}
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
        {hasGlobalMount ? (
          <p className="mount-project-hint">
            同一 Skill 不能同时使用 Codex global 与 project Mount；请先移除全局挂载。
          </p>
        ) : null}
        {hasProjectMount && !hasGlobalMount ? (
          <p className="mount-project-hint">
            已有 project Mount 时不能再创建 global Mount，但可以继续添加其他项目。
          </p>
        ) : null}
      </section>

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

function mountDestinationLabel(mount: MountSummary): string {
  return mount.scope === "global"
    ? "Codex 全局"
    : `Codex 项目 ${mount.projectDisplayName ?? "已登记项目"}`;
}

function mountHealthCopy(mount: MountSummary): string {
  return {
    healthy: "软链接正常",
    missing: "软链接已缺失；移除时只清理 SkillYard 记录",
    conflict: "目标路径已被其他内容占用；移除不会删除该内容",
  }[mount.health];
}
