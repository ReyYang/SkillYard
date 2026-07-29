import type {
  InventoryObservation,
  MountScope,
  MountSummary,
  ProjectSummary,
  SupportedAppId,
  SupportedAppSummary,
} from "../domain";
import { useI18n } from "../i18n";
import { PageBackButton } from "./PageBackButton";

interface MountManagementPageProps {
  entry: InventoryObservation;
  supportedApps: SupportedAppSummary[];
  projects: ProjectSummary[];
  mounts: MountSummary[];
  readOnly: boolean;
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
  readOnly,
  isPlanning,
  error,
  onBack,
  onCreate,
  onRemove,
  onRepair,
}: MountManagementPageProps) {
  const { t } = useI18n();
  return (
    <main className="mount-shell">
      <PageBackButton disabled={isPlanning} onClick={onBack} />
      <p className="eyebrow">SKILLYARD · SUPPORTED APP MOUNT</p>
      <h1>{t("管理挂载")}</h1>
      <p className="lead">
        {entry.bundleDisplayName
          ? `${entry.bundleDisplayName}: ${entry.skillName}`
          : entry.skillName}
      </p>
      {readOnly ? (
        <p className="recovery-notice" role="status">
          {t("当前操作进行中，只展示已提交的挂载状态。")}
        </p>
      ) : null}

      {error ? (
        <div className="inline-error" role="alert">
          <strong>{t("无法准备挂载")}</strong>
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
            aria-label={t("{app} 挂载", { app: app.displayName })}
          >
            <div className="mount-app-heading">
              <div>
                <p className="section-eyebrow">SUPPORTED APP</p>
                <h2>{app.displayName}</h2>
              </div>
              <span>{detectionLabel(detected, t)}</span>
            </div>

            <h3>{t("当前使用位置")}</h3>
            {appMounts.length === 0 ? (
              <p className="mount-empty-copy">
                {t("这个 Skill 目前没有挂载到 {app}。", {
                  app: app.displayName,
                })}
              </p>
            ) : (
              <ul className="mount-list">
                {appMounts.map((mount) => (
                  <li key={mount.id}>
                    <div>
                      <strong>{mountDestinationLabel(mount, t)}</strong>
                      <span>{mountHealthCopy(mount, t)}</span>
                      <code title={mount.targetPath}>{mount.targetPath}</code>
                    </div>
                    <button
                      className="compact-action"
                      type="button"
                      hidden={mount.health !== "missing"}
                      disabled={readOnly || isPlanning}
                      onClick={() => onRepair(mount.id)}
                    >
                      {t("修复 {destination}挂载", {
                        destination: mountDestinationLabel(mount, t),
                      })}
                    </button>
                    <button
                      className="danger-outline-action"
                      type="button"
                      disabled={readOnly || isPlanning}
                      onClick={() => onRemove(mount.id)}
                    >
                      {t("移除 {destination}挂载", {
                        destination: mountDestinationLabel(mount, t),
                      })}
                    </button>
                  </li>
                ))}
              </ul>
            )}

            <h3>{t("新增使用位置")}</h3>
            <div className="mount-target-list">
              <button
                className="mount-target"
                type="button"
                aria-label={t("挂载到 {app} 全局", {
                  app: app.displayName,
                })}
                disabled={
                  readOnly || isPlanning || hasGlobalMount || hasProjectMount
                }
                onClick={() => onCreate(app.id, "global", null)}
              >
                <strong>
                  {t("挂载到 {app} 全局", { app: app.displayName })}
                </strong>
                <span>
                  {t("在这台 Mac 的所有 {app} 项目中可用", {
                    app: app.displayName,
                  })}
                </span>
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
                    aria-label={t("挂载到 {app} 项目 {project}", {
                      app: app.displayName,
                      project: project.displayName,
                    })}
                    disabled={
                      readOnly || isPlanning || hasGlobalMount || alreadyMounted
                    }
                    onClick={() => onCreate(app.id, "project", project.id)}
                  >
                    <strong>
                      {t("挂载到项目 {project}", {
                        project: project.displayName,
                      })}
                    </strong>
                    <code title={project.rootPath}>{project.rootPath}</code>
                  </button>
                );
              })}
            </div>
            {projects.length === 0 ? (
              <p className="mount-project-hint">
                {t(
                  "还没有已登记项目。返回清单后可通过“添加项目”选择本地目录。",
                )}
              </p>
            ) : null}
            {app.id === "claudeCode" && projects.length > 0 ? (
              <p className="mount-project-hint">
                {t(
                  "Claude Code 的项目挂载位于 .claude/skills；GitHub Copilot 也可能读取这个位置。",
                )}
              </p>
            ) : null}
            {hasGlobalMount ? (
              <p className="mount-project-hint">
                {t(
                  "同一 Skill 不能同时使用 {app} global 与 project Mount；请先移除全局挂载。",
                  { app: app.displayName },
                )}
              </p>
            ) : null}
            {hasProjectMount && !hasGlobalMount ? (
              <p className="mount-project-hint">
                {t(
                  "已有 project Mount 时不能再创建 global Mount，但可以继续添加其他项目。",
                )}
              </p>
            ) : null}
          </section>
        );
      })}
    </main>
  );
}

function detectionLabel(
  detected: boolean | null | undefined,
  t: ReturnType<typeof useI18n>["t"],
): string {
  if (detected === true) return t("已检测到");
  if (detected === false) return t("未检测到");
  return t("尚未检测");
}

function mountDestinationLabel(
  mount: MountSummary,
  t: ReturnType<typeof useI18n>["t"],
): string {
  const appName = supportedAppLabel(mount.appId);
  return mount.scope === "global"
    ? t("{app} 全局", { app: appName })
    : t("{app} 项目 {project}", {
        app: appName,
        project: mount.projectDisplayName ?? t("已登记项目"),
      });
}

function supportedAppLabel(appId: SupportedAppId): string {
  return {
    codex: "Codex",
    claudeCode: "Claude Code",
    gitHubCopilot: "GitHub Copilot",
  }[appId];
}

function mountHealthCopy(
  mount: MountSummary,
  t: ReturnType<typeof useI18n>["t"],
): string {
  return {
    healthy: t("软链接正常"),
    missing: t("软链接已缺失；移除时只清理 SkillYard 记录"),
    conflict: t("目标路径无法安全确认；移除只清理 SkillYard 记录"),
  }[mount.health];
}
