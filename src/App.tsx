import { useEffect, useState } from "react";

import { InventoryPage } from "./components/InventoryPage";
import { InstallFolderPage } from "./components/InstallFolderPage";
import { MountManagementPage } from "./components/MountManagementPage";
import { MountPlanPage } from "./components/MountPlanPage";
import { OnboardingPage } from "./components/OnboardingPage";
import type {
  FolderInstallPlan,
  MountPlan,
  MountScope,
  SupportedAppId,
  UiOutcome,
} from "./domain";
import {
  tauriSkillYardClient,
  type SkillYardClient,
} from "./skillyardClient";

interface AppProps {
  client?: SkillYardClient;
}

type ViewState =
  | { status: "loading" }
  | { status: "ready"; outcome: UiOutcome }
  | { status: "error"; message: string };

export function App({ client = tauriSkillYardClient }: AppProps) {
  const [viewState, setViewState] = useState<ViewState>({ status: "loading" });
  const [isScanning, setIsScanning] = useState(false);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [isChoosingFolder, setIsChoosingFolder] = useState(false);
  const [pendingInstallPlan, setPendingInstallPlan] =
    useState<FolderInstallPlan | null>(null);
  const [isInstalling, setIsInstalling] = useState(false);
  const [installError, setInstallError] = useState<string | null>(null);
  const [isAddingProject, setIsAddingProject] = useState(false);
  const [projectError, setProjectError] = useState<string | null>(null);
  const [managedMemberId, setManagedMemberId] = useState<string | null>(null);
  const [pendingMountPlan, setPendingMountPlan] = useState<MountPlan | null>(null);
  const [isPlanningMount, setIsPlanningMount] = useState(false);
  const [isConfirmingMount, setIsConfirmingMount] = useState(false);
  const [mountError, setMountError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    client.getStartupState().then(
      (outcome) => {
        if (active) setViewState({ status: "ready", outcome });
      },
      (error: unknown) => {
        if (active) setViewState({ status: "error", message: formatError(error) });
      },
    );
    return () => {
      active = false;
    };
  }, [client]);

  const startInitialScan = async () => {
    if (isScanning) return;
    setIsScanning(true);
    try {
      const outcome = await client.startInitialScan();
      setViewState({ status: "ready", outcome });
    } catch (error) {
      setViewState({ status: "error", message: formatError(error) });
    } finally {
      setIsScanning(false);
    }
  };

  const refreshLocalInventory = async () => {
    if (isRefreshing) return;
    setIsRefreshing(true);
    setRefreshError(null);
    try {
      const outcome = await client.refreshLocalInventory();
      setViewState({ status: "ready", outcome });
    } catch (error) {
      // 刷新失败不抹掉上次已提交清单，用户仍可继续搜索和浏览。
      setRefreshError(formatError(error));
    } finally {
      setIsRefreshing(false);
    }
  };

  const chooseFolderInstallPlan = async () => {
    if (isChoosingFolder) return;
    setIsChoosingFolder(true);
    setInstallError(null);
    try {
      const plan = await client.chooseFolderInstallPlan();
      if (plan) setPendingInstallPlan(plan);
    } catch (error) {
      setInstallError(formatError(error));
    } finally {
      setIsChoosingFolder(false);
    }
  };

  const confirmInstall = async (selectedCandidateIds: string[]) => {
    if (!pendingInstallPlan || isInstalling) return;
    setIsInstalling(true);
    setInstallError(null);
    try {
      const outcome = await client.confirmInstallPlan(
        pendingInstallPlan.id,
        selectedCandidateIds,
      );
      setPendingInstallPlan(null);
      setViewState({ status: "ready", outcome });
    } catch (error) {
      const message = formatError(error);
      // Plan 一旦确认就可能已消费；失败后重新读取后端最终状态，不能让用户重试旧 Plan。
      setPendingInstallPlan(null);
      try {
        const outcome = await client.getStartupState();
        setViewState({ status: "ready", outcome });
        setInstallError(message);
      } catch (recoveryError) {
        setViewState({
          status: "error",
          message: `${message}；重新读取状态失败：${formatError(recoveryError)}`,
        });
      }
    } finally {
      setIsInstalling(false);
    }
  };

  const chooseAndRegisterProject = async () => {
    if (isAddingProject) return;
    setIsAddingProject(true);
    setProjectError(null);
    try {
      const outcome = await client.chooseAndRegisterProject();
      // 取消原生选择器返回 null；现有清单和登记状态保持不变。
      if (outcome) setViewState({ status: "ready", outcome });
    } catch (error) {
      setProjectError(formatError(error));
    } finally {
      setIsAddingProject(false);
    }
  };

  const openMountManager = (memberId: string) => {
    setMountError(null);
    setManagedMemberId(memberId);
  };

  const createMountPlan = async (
    appId: SupportedAppId,
    scope: MountScope,
    projectId: string | null,
  ) => {
    if (!managedMemberId || isPlanningMount) return;
    setIsPlanningMount(true);
    setMountError(null);
    try {
      const plan = await client.createMountPlan(
        managedMemberId,
        appId,
        scope,
        projectId,
      );
      setPendingMountPlan(plan);
    } catch (error) {
      // Plan 生成失败没有生命周期写入，保留已加载清单和当前管理页。
      setMountError(formatError(error));
    } finally {
      setIsPlanningMount(false);
    }
  };

  const createRemoveMountPlan = async (mountId: string) => {
    if (isPlanningMount) return;
    setIsPlanningMount(true);
    setMountError(null);
    try {
      const plan = await client.createRemoveMountPlan(mountId);
      setPendingMountPlan(plan);
    } catch (error) {
      setMountError(formatError(error));
    } finally {
      setIsPlanningMount(false);
    }
  };

  const confirmMount = async () => {
    if (!pendingMountPlan || isConfirmingMount) return;
    setIsConfirmingMount(true);
    setMountError(null);
    try {
      const outcome = await client.confirmMountPlan(pendingMountPlan.id);
      setPendingMountPlan(null);
      setManagedMemberId(null);
      setViewState({ status: "ready", outcome });
    } catch (error) {
      const message = formatError(error);
      // 确认后 Plan 可能已经消费；必须重读 Rust 最终状态，不能重试旧 Plan。
      setPendingMountPlan(null);
      setManagedMemberId(null);
      try {
        const outcome = await client.getStartupState();
        setViewState({ status: "ready", outcome });
        setMountError(message);
      } catch (recoveryError) {
        setViewState({
          status: "error",
          message: `${message}；重新读取状态失败：${formatError(recoveryError)}`,
        });
      }
    } finally {
      setIsConfirmingMount(false);
    }
  };

  if (viewState.status === "loading") {
    return (
      <main className="state-page" aria-label="SkillYard 正在启动">
        <span className="spinner" aria-hidden="true" />
        <p>正在读取本机状态…</p>
      </main>
    );
  }
  if (viewState.status === "error") {
    return (
      <main className="state-page" role="alert">
        <p className="eyebrow">SKILLYARD · LOCAL ERROR</p>
        <h1>暂时无法继续</h1>
        <p>{viewState.message}</p>
      </main>
    );
  }
  if (pendingInstallPlan) {
    return (
      <InstallFolderPage
        plan={pendingInstallPlan}
        isInstalling={isInstalling}
        error={installError}
        onCancel={() => {
          setInstallError(null);
          setPendingInstallPlan(null);
        }}
        onConfirm={confirmInstall}
      />
    );
  }
  if (pendingMountPlan) {
    return (
      <MountPlanPage
        plan={pendingMountPlan}
        isConfirming={isConfirmingMount}
        onBack={() => setPendingMountPlan(null)}
        onConfirm={confirmMount}
      />
    );
  }
  if (viewState.outcome.type === "onboardingRequired") {
    return (
      <OnboardingPage
        isScanning={isScanning}
        onStartScan={startInitialScan}
      />
    );
  }

  if (viewState.outcome.type === "unsupportedPlatform") {
    return (
      <main className="state-page">
        <p className="eyebrow">SKILLYARD · PLATFORM CHECK</p>
        <h1>当前 Mac 不受 SkillYard 1.0 支持</h1>
        <p>
          需要 macOS {viewState.outcome.minimumMajorVersion} 或更高版本的 Apple
          Silicon Mac。
        </p>
      </main>
    );
  }

  if (viewState.outcome.type !== "inventory") {
    return (
      <main className="state-page" role="alert">
        <h1>暂时无法继续</h1>
        <p>SkillYard 收到了无法显示的应用状态。</p>
      </main>
    );
  }

  const managedEntry = managedMemberId
    ? viewState.outcome.entries.find(
        (entry) =>
          entry.managementKind === "skillYardManaged" &&
          entry.memberId === managedMemberId,
      )
    : null;

  if (managedEntry) {
    return (
      <MountManagementPage
        entry={managedEntry}
        supportedApps={viewState.outcome.supportedApps}
        projects={viewState.outcome.projects}
        mounts={viewState.outcome.mounts}
        isPlanning={isPlanningMount}
        error={mountError}
        onBack={() => {
          setMountError(null);
          setManagedMemberId(null);
        }}
        onCreate={createMountPlan}
        onRemove={createRemoveMountPlan}
      />
    );
  }

  return (
    <InventoryPage
      outcome={viewState.outcome}
      isRefreshing={isRefreshing}
      isChoosingFolder={isChoosingFolder}
      isAddingProject={isAddingProject}
      refreshError={refreshError}
      installError={installError}
      projectError={projectError}
      mountError={mountError}
      onRefresh={refreshLocalInventory}
      onInstall={chooseFolderInstallPlan}
      onAddProject={chooseAndRegisterProject}
      onManageMount={openMountManager}
    />
  );
}

function formatError(error: unknown): string {
  if (error instanceof Error) return error.message;
  // Tauri command 会把 Rust 的结构化 UiError 作为普通对象传给前端。
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string"
  ) {
    return error.message;
  }
  return "无法读取 SkillYard 状态";
}
