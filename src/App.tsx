import { useEffect, useState } from "react";

import { BatchMountPlanPage } from "./components/BatchMountPlanPage";
import { BundleMountPage } from "./components/BundleMountPage";
import { InventoryPage } from "./components/InventoryPage";
import { InstallPlanPage } from "./components/InstallPlanPage";
import { MountManagementPage } from "./components/MountManagementPage";
import { MountPlanPage } from "./components/MountPlanPage";
import { OnboardingPage } from "./components/OnboardingPage";
import { SourceCatalogPage } from "./components/SourceCatalogPage";
import { SourceRefChangePage } from "./components/SourceRefChangePage";
import { TakeoverPlanPage } from "./components/TakeoverPlanPage";
import { TakeoverSelectionPage } from "./components/TakeoverSelectionPage";
import type {
  BatchMountPlan,
  BatchMountRequest,
  InstallPlan,
  MountPlan,
  MountScope,
  SourceRefChangePlan,
  SupportedAppId,
  TakeoverPlan,
  TakeoverPlanRequest,
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

type SourceDiscoveryOutcome = Extract<UiOutcome, { type: "sourceDiscovery" }>;
type SkillsShSearchOutcome = Extract<UiOutcome, { type: "skillsShSearch" }>;

type SourceOperation =
  | { type: "opening" }
  | { type: "adding" }
  | { type: "searchingSkillsSh" }
  | { type: "choosingFolder" }
  | { type: "choosingArchive" }
  | { type: "choosingEditable" }
  | { type: "planningUrl" }
  | { type: "reloading"; sourceId: string }
  | { type: "planningInstall"; sourceId: string }
  | { type: "confirmingRef" };

export function App({ client = tauriSkillYardClient }: AppProps) {
  const [viewState, setViewState] = useState<ViewState>({ status: "loading" });
  const [isScanning, setIsScanning] = useState(false);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  // Inventory 保留为主界面基座；Source 页面只是临时路由，不覆盖已加载清单。
  const [sourceDiscovery, setSourceDiscovery] =
    useState<SourceDiscoveryOutcome | null>(null);
  const [skillsShSearch, setSkillsShSearch] =
    useState<SkillsShSearchOutcome | null>(null);
  const [sourceOperation, setSourceOperation] =
    useState<SourceOperation | null>(null);
  const [sourceError, setSourceError] = useState<string | null>(null);
  const [pendingSourceRefChange, setPendingSourceRefChange] =
    useState<SourceRefChangePlan | null>(null);
  const [pendingInstallPlan, setPendingInstallPlan] =
    useState<InstallPlan | null>(null);
  const [isInstalling, setIsInstalling] = useState(false);
  const [isDiscardingInstallPlan, setIsDiscardingInstallPlan] = useState(false);
  const [installError, setInstallError] = useState<string | null>(null);
  const [isAddingProject, setIsAddingProject] = useState(false);
  const [projectError, setProjectError] = useState<string | null>(null);
  const [takeoverObservationId, setTakeoverObservationId] = useState<
    string | null
  >(null);
  const [pendingTakeoverPlan, setPendingTakeoverPlan] =
    useState<TakeoverPlan | null>(null);
  const [isPlanningTakeover, setIsPlanningTakeover] = useState(false);
  const [isConfirmingTakeover, setIsConfirmingTakeover] = useState(false);
  const [takeoverError, setTakeoverError] = useState<string | null>(null);
  const [managedMemberId, setManagedMemberId] = useState<string | null>(null);
  const [pendingMountPlan, setPendingMountPlan] = useState<MountPlan | null>(null);
  const [isPlanningMount, setIsPlanningMount] = useState(false);
  const [isConfirmingMount, setIsConfirmingMount] = useState(false);
  const [mountError, setMountError] = useState<string | null>(null);
  const [batchMountBundleId, setBatchMountBundleId] = useState<string | null>(
    null,
  );
  const [pendingBatchMountPlan, setPendingBatchMountPlan] =
    useState<BatchMountPlan | null>(null);
  const [isPlanningBatchMount, setIsPlanningBatchMount] = useState(false);
  const [isConfirmingBatchMount, setIsConfirmingBatchMount] = useState(false);

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

  const openSourceDiscovery = async () => {
    if (sourceOperation) return;
    setSourceOperation({ type: "opening" });
    setSourceError(null);
    setInstallError(null);
    try {
      const outcome = await client.openSourceDiscovery();
      setSourceDiscovery(outcome);
    } catch (error) {
      // Source 首次加载失败不抹掉当前 Inventory，用户仍可继续管理本机 Skill。
      setSourceError(formatError(error));
    } finally {
      setSourceOperation(null);
    }
  };

  const returnToInventory = () => {
    if (sourceOperation) return;
    setSourceError(null);
    setSkillsShSearch(null);
    setSourceDiscovery(null);
  };

  const searchSkillsSh = async (query: string) => {
    if (sourceOperation) return;
    setSourceOperation({ type: "searchingSkillsSh" });
    setSourceError(null);
    try {
      setSkillsShSearch(await client.searchSkillsSh(query));
    } catch (error) {
      // 搜索失败只影响发现结果，不改写 Source 或已安装内容。
      setSkillsShSearch(null);
      setSourceError(formatError(error));
    } finally {
      setSourceOperation(null);
    }
  };

  const rereadSourceAfterFailure = async (error: unknown) => {
    const message = formatError(error);
    try {
      // 当前会话已经打开过发现页，这次只读取 SQLite 的最终状态，不会隐式重新联网。
      const outcome = await client.openSourceDiscovery();
      setSourceDiscovery(outcome);
      setSourceError(message);
    } catch (recoveryError) {
      setSourceError(
        `${message}；重新读取 Source 状态失败：${formatError(recoveryError)}`,
      );
    }
  };

  const reloadGithubSource = async (sourceId: string) => {
    if (sourceOperation) return;
    setSourceOperation({ type: "reloading", sourceId });
    setSourceError(null);
    try {
      const outcome = await client.reloadGithubSource(sourceId);
      setSourceDiscovery(outcome);
    } catch (error) {
      await rereadSourceAfterFailure(error);
    } finally {
      setSourceOperation(null);
    }
  };

  const addGithubSource = async (input: string, trackedRef: string | null) => {
    if (sourceOperation) return;
    setSourceOperation({ type: "adding" });
    setSourceError(null);
    try {
      const outcome = await client.addGithubSource(input, trackedRef);
      if (outcome.type === "sourceRefChangePlan") {
        setPendingSourceRefChange(outcome.plan);
      } else {
        setSkillsShSearch(null);
        setSourceDiscovery(outcome);
      }
    } catch (error) {
      await rereadSourceAfterFailure(error);
    } finally {
      setSourceOperation(null);
    }
  };

  const confirmSourceRefChange = async () => {
    if (!pendingSourceRefChange || sourceOperation) return;
    setSourceOperation({ type: "confirmingRef" });
    setSourceError(null);
    try {
      const outcome = await client.confirmSourceRefChange(
        pendingSourceRefChange.id,
      );
      setPendingSourceRefChange(null);
      setSourceDiscovery(outcome);
    } catch (error) {
      // 确认失败后丢弃旧页面并重读持久状态，避免用户重试结果不确定的 Plan。
      setPendingSourceRefChange(null);
      await rereadSourceAfterFailure(error);
    } finally {
      setSourceOperation(null);
    }
  };

  const chooseFolderInstallPlan = async () => {
    if (sourceOperation) return;
    setSourceOperation({ type: "choosingFolder" });
    setSourceError(null);
    setInstallError(null);
    try {
      const plan = await client.chooseFolderInstallPlan();
      if (plan) setPendingInstallPlan(plan);
    } catch (error) {
      setSourceError(formatError(error));
    } finally {
      setSourceOperation(null);
    }
  };

  const chooseArchiveInstallPlan = async () => {
    if (sourceOperation) return;
    setSourceOperation({ type: "choosingArchive" });
    setSourceError(null);
    setInstallError(null);
    try {
      const plan = await client.chooseArchiveInstallPlan();
      if (plan) setPendingInstallPlan(plan);
    } catch (error) {
      setSourceError(formatError(error));
    } finally {
      setSourceOperation(null);
    }
  };

  const chooseEditableLocalInstallPlan = async () => {
    if (sourceOperation) return;
    setSourceOperation({ type: "choosingEditable" });
    setSourceError(null);
    setInstallError(null);
    try {
      const plan = await client.chooseEditableLocalInstallPlan();
      if (plan) setPendingInstallPlan(plan);
    } catch (error) {
      setSourceError(formatError(error));
    } finally {
      setSourceOperation(null);
    }
  };

  const createUrlInstallPlan = async (url: string) => {
    if (sourceOperation) return;
    setSourceOperation({ type: "planningUrl" });
    setSourceError(null);
    setInstallError(null);
    try {
      setPendingInstallPlan(await client.createUrlInstallPlan(url));
    } catch (error) {
      setSourceError(formatError(error));
    } finally {
      setSourceOperation(null);
    }
  };

  const createGithubInstallPlan = async (sourceId: string) => {
    if (sourceOperation) return;
    setSourceOperation({ type: "planningInstall", sourceId });
    setSourceError(null);
    setInstallError(null);
    try {
      const plan = await client.createGithubInstallPlan(sourceId);
      setPendingInstallPlan(plan);
    } catch (error) {
      setSourceError(formatError(error));
    } finally {
      setSourceOperation(null);
    }
  };

  const discardInstallPlan = async () => {
    if (!pendingInstallPlan || isInstalling || isDiscardingInstallPlan) return;
    setIsDiscardingInstallPlan(true);
    setInstallError(null);
    try {
      await client.discardInstallPlan(pendingInstallPlan.id);
      setPendingInstallPlan(null);
    } catch (error) {
      if (errorCode(error) === "installPlanConsumed") {
        const message = formatError(error);
        try {
          // 另一实例已经开始确认时，本页 Plan 永远不能再使用；回到持久化最终状态。
          const outcome = await client.getStartupState();
          setPendingInstallPlan(null);
          setSourceDiscovery(null);
          setViewState({ status: "ready", outcome });
          setInstallError(message);
        } catch (recoveryError) {
          setPendingInstallPlan(null);
          setSourceDiscovery(null);
          setViewState({
            status: "error",
            message: `${message}；重新读取状态失败：${formatError(recoveryError)}`,
          });
        }
        return;
      }
      // 返回按钮只有在后端确实删除 Plan 和快照后才离开确认页。
      setInstallError(formatError(error));
    } finally {
      setIsDiscardingInstallPlan(false);
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
      setSourceDiscovery(null);
      setViewState({ status: "ready", outcome });
    } catch (error) {
      const message = formatError(error);
      // Plan 一旦确认就可能已消费；失败后重新读取后端最终状态，不能让用户重试旧 Plan。
      try {
        const outcome = await client.getStartupState();
        setPendingInstallPlan(null);
        setSourceDiscovery(null);
        setViewState({ status: "ready", outcome });
        setInstallError(message);
      } catch (recoveryError) {
        setPendingInstallPlan(null);
        setSourceDiscovery(null);
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

  const openTakeover = (observationId: string) => {
    setTakeoverError(null);
    setTakeoverObservationId(observationId);
  };

  const createTakeoverPlan = async (request: TakeoverPlanRequest) => {
    if (isPlanningTakeover) return;
    setIsPlanningTakeover(true);
    setTakeoverError(null);
    try {
      const plan = await client.createTakeoverPlan(request);
      setPendingTakeoverPlan(plan);
    } catch (error) {
      // 创建 Plan 仍是只读操作，失败后保留用户选择供调整或返回。
      setTakeoverError(formatError(error));
    } finally {
      setIsPlanningTakeover(false);
    }
  };

  const confirmTakeover = async () => {
    if (!pendingTakeoverPlan || isConfirmingTakeover) return;
    setIsConfirmingTakeover(true);
    setTakeoverError(null);
    try {
      const outcome = await client.confirmTakeoverPlan(pendingTakeoverPlan.id);
      setPendingTakeoverPlan(null);
      setTakeoverObservationId(null);
      setViewState({ status: "ready", outcome });
    } catch (error) {
      const message = formatError(error);
      // 确认后的 Plan 不能重试；启动读取会先完成或回滚唯一接管事务。
      setPendingTakeoverPlan(null);
      setTakeoverObservationId(null);
      try {
        const outcome = await client.getStartupState();
        setViewState({ status: "ready", outcome });
        setTakeoverError(message);
      } catch (recoveryError) {
        setViewState({
          status: "error",
          message: `${message}；重新读取状态失败：${formatError(recoveryError)}`,
        });
      }
    } finally {
      setIsConfirmingTakeover(false);
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

  const createRepairMountPlan = async (mountId: string) => {
    if (isPlanningMount) return;
    setIsPlanningMount(true);
    setMountError(null);
    try {
      const plan = await client.createRepairMountPlan(mountId);
      setPendingMountPlan(plan);
    } catch (error) {
      const message = formatError(error);
      // 生成修复 Plan 时可能刚发现外部占用；立即重读，避免继续展示过时的“缺失”状态。
      try {
        const outcome = await client.refreshLocalInventory();
        setViewState({ status: "ready", outcome });
        setMountError(message);
      } catch (refreshFailure) {
        setMountError(
          `${message}；重新检查挂载状态失败：${formatError(refreshFailure)}`,
        );
      }
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

  const openBatchMount = (bundleId: string) => {
    setMountError(null);
    setBatchMountBundleId(bundleId);
  };

  const createBatchMountPlan = async (requests: BatchMountRequest[]) => {
    if (!batchMountBundleId || isPlanningBatchMount) return;
    setIsPlanningBatchMount(true);
    setMountError(null);
    try {
      const plan = await client.createBatchMountPlan(
        batchMountBundleId,
        requests,
      );
      setPendingBatchMountPlan(plan);
    } catch (error) {
      // 预览失败尚未开始生命周期事务，保留目标选择页供用户调整或返回。
      setMountError(formatError(error));
    } finally {
      setIsPlanningBatchMount(false);
    }
  };

  const confirmBatchMount = async (selectedItemIds: string[]) => {
    if (!pendingBatchMountPlan || isConfirmingBatchMount) return;
    setIsConfirmingBatchMount(true);
    setMountError(null);
    try {
      const outcome = await client.confirmBatchMountPlan(
        pendingBatchMountPlan.id,
        selectedItemIds,
      );
      setPendingBatchMountPlan(null);
      setBatchMountBundleId(null);
      setViewState({ status: "ready", outcome });
    } catch (error) {
      const message = formatError(error);
      // 确认后 Plan 可能已消费；丢弃两步页面并重新读取 Rust 的最终状态。
      setPendingBatchMountPlan(null);
      setBatchMountBundleId(null);
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
      setIsConfirmingBatchMount(false);
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
  if (pendingSourceRefChange) {
    return (
      <SourceRefChangePage
        plan={pendingSourceRefChange}
        isConfirming={sourceOperation?.type === "confirmingRef"}
        error={sourceError}
        onBack={() => {
          setSourceError(null);
          setPendingSourceRefChange(null);
        }}
        onConfirm={confirmSourceRefChange}
      />
    );
  }
  if (pendingTakeoverPlan) {
    return (
      <TakeoverPlanPage
        plan={pendingTakeoverPlan}
        isConfirming={isConfirmingTakeover}
        onBack={() => setPendingTakeoverPlan(null)}
        onConfirm={confirmTakeover}
      />
    );
  }
  if (pendingInstallPlan) {
    return (
      <InstallPlanPage
        plan={pendingInstallPlan}
        isInstalling={isInstalling}
        isDiscarding={isDiscardingInstallPlan}
        error={installError}
        onCancel={discardInstallPlan}
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
  if (pendingBatchMountPlan) {
    return (
      <BatchMountPlanPage
        plan={pendingBatchMountPlan}
        isConfirming={isConfirmingBatchMount}
        onBack={() => setPendingBatchMountPlan(null)}
        onConfirm={confirmBatchMount}
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

  if (sourceDiscovery) {
    return (
      <SourceCatalogPage
        outcome={sourceDiscovery}
        skillsShSearch={skillsShSearch}
        mounts={
          viewState.outcome.type === "inventory" ? viewState.outcome.mounts : []
        }
        operation={sourceOperation}
        error={sourceError}
        onBack={returnToInventory}
        onAddSource={addGithubSource}
        onSearchSkillsSh={searchSkillsSh}
        onChooseFolder={chooseFolderInstallPlan}
        onChooseArchive={chooseArchiveInstallPlan}
        onChooseEditable={chooseEditableLocalInstallPlan}
        onInstallUrl={createUrlInstallPlan}
        onReload={reloadGithubSource}
        onInstall={createGithubInstallPlan}
      />
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

  const takeoverEntry = takeoverObservationId
    ? viewState.outcome.entries.find(
        (entry) =>
          entry.id === takeoverObservationId &&
          entry.managementKind === "takeoverCandidate",
      )
    : null;
  if (takeoverEntry) {
    return (
      <TakeoverSelectionPage
        initialObservationId={takeoverEntry.id}
        candidates={viewState.outcome.entries.filter(
          (entry) =>
            entry.managementKind === "takeoverCandidate" &&
            entry.skillName === takeoverEntry.skillName,
        )}
        isPlanning={isPlanningTakeover}
        error={takeoverError}
        onBack={() => {
          setTakeoverError(null);
          setTakeoverObservationId(null);
        }}
        onCreatePlan={createTakeoverPlan}
      />
    );
  }

  const batchMountEntries = batchMountBundleId
    ? viewState.outcome.entries.flatMap((entry) =>
        entry.managementKind === "skillYardManaged" &&
        entry.bundleId === batchMountBundleId &&
        entry.memberId
          ? [{ memberId: entry.memberId, skillName: entry.skillName }]
          : [],
      )
    : [];
  const batchMountBundleName = batchMountBundleId
    ? viewState.outcome.entries.find(
        (entry) =>
          entry.managementKind === "skillYardManaged" &&
          entry.bundleId === batchMountBundleId,
      )?.bundleDisplayName
    : null;

  if (
    batchMountBundleId &&
    batchMountBundleName &&
    batchMountEntries.length > 0
  ) {
    return (
      <BundleMountPage
        bundleDisplayName={batchMountBundleName}
        members={batchMountEntries}
        supportedApps={viewState.outcome.supportedApps}
        projects={viewState.outcome.projects}
        isPlanning={isPlanningBatchMount}
        error={mountError}
        onBack={() => {
          setMountError(null);
          setBatchMountBundleId(null);
        }}
        onCreatePlan={createBatchMountPlan}
      />
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
        onRepair={createRepairMountPlan}
      />
    );
  }

  return (
    <InventoryPage
      outcome={viewState.outcome}
      // 任一主界面写操作进行中时统一冻结其他写入口；搜索和筛选仍可使用。
      isWriteBlocked={
        isRefreshing ||
        isAddingProject ||
        sourceOperation?.type === "opening"
      }
      isRefreshing={isRefreshing}
      isOpeningInstaller={sourceOperation?.type === "opening"}
      isAddingProject={isAddingProject}
      refreshError={refreshError}
      installError={installError ?? sourceError}
      projectError={projectError}
      mountError={mountError}
      takeoverError={takeoverError}
      onRefresh={refreshLocalInventory}
      onInstall={openSourceDiscovery}
      onAddProject={chooseAndRegisterProject}
      onTakeover={openTakeover}
      onManageMount={openMountManager}
      onBatchMount={openBatchMount}
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

function errorCode(error: unknown): string | null {
  if (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    typeof error.code === "string"
  ) {
    return error.code;
  }
  return null;
}
