import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { AgentOverlay } from "./components/AgentOverlay";
import { BatchMountPlanPage } from "./components/BatchMountPlanPage";
import { BundleMountPage } from "./components/BundleMountPage";
import { BundleUpdateBatchPage } from "./components/BundleUpdateBatchPage";
import { CurrentOperationBanner } from "./components/CurrentOperationBanner";
import { EditableLocalRelinkPage } from "./components/EditableLocalRelinkPage";
import {
  InventoryPage,
  type InventoryScreen,
} from "./components/InventoryPage";
import { InstallPlanPage } from "./components/InstallPlanPage";
import { MountManagementPage } from "./components/MountManagementPage";
import { MountPlanPage } from "./components/MountPlanPage";
import { OnboardingPage } from "./components/OnboardingPage";
import { ProjectConfirmationDialog } from "./components/ProjectConfirmationDialog";
import { RecoveryPage } from "./components/RecoveryPage";
import { RemovalPlanPage } from "./components/RemovalPlanPage";
import { SourceAssociationPlanPage } from "./components/SourceAssociationPlanPage";
import { SourceAssociationSelectionPage } from "./components/SourceAssociationSelectionPage";
import { SourceCatalogPage } from "./components/SourceCatalogPage";
import { SourceRefChangePage } from "./components/SourceRefChangePage";
import { TakeoverPlanPage } from "./components/TakeoverPlanPage";
import { TakeoverSelectionPage } from "./components/TakeoverSelectionPage";
import type {
  AgentPageContext,
  AgentSearchResult,
  AiConfigurationInput,
  AiPreferences,
  BatchMountPlan,
  BatchMountRequest,
  EditableLocalRelinkPlan,
  InstallPlan,
  InterfaceLanguage,
  MountPlan,
  MountScope,
  ProjectSelection,
  RemovalKind,
  RemovalPlan,
  SourceAssociationContentChoice,
  SourceAssociationPlan,
  SourceMemberMappingChoice,
  SourceRefChangePlan,
  SupportedAppId,
  TakeoverPlan,
  TakeoverPlanRequest,
  UiOutcome,
} from "./domain";
import { I18nProvider, useI18n } from "./i18n";
import {
  tauriSkillYardClient,
  type SkillYardClient,
} from "./skillyardClient";

interface AppProps {
  client?: SkillYardClient;
}

interface AppCoreProps {
  client: SkillYardClient;
  language: InterfaceLanguage;
  aiPreferences: AiPreferences;
  isSavingLanguage: boolean;
  languageError: string | null;
  aiOperation: AiOperation;
  aiError: string | null;
  onLanguageChange(language: InterfaceLanguage): Promise<void>;
  onAiConfigurationChange(configuration: AiConfigurationInput): Promise<void>;
  onSaveAiApiKey(apiKey: string): Promise<void>;
  onDeleteAiApiKey(): Promise<void>;
  onTestAiConnection(): Promise<void>;
  onAgentContextChange(context: AgentPageContext): void;
  agentInstallPlan: InstallPlan | null;
  onAgentInstallPlanClear(): void;
}

type AiOperation =
  | "savingConfiguration"
  | "savingKey"
  | "deletingKey"
  | "testing"
  | null;

type ViewState =
  | { status: "loading" }
  | { status: "ready"; outcome: UiOutcome }
  | { status: "error"; message: string };

type SourceDiscoveryOutcome = Extract<UiOutcome, { type: "sourceDiscovery" }>;
type SkillsShSearchOutcome = Extract<UiOutcome, { type: "skillsShSearch" }>;
type InventoryOutcome = Extract<UiOutcome, { type: "inventory" }>;

const INVENTORY_LIST_SCREEN: InventoryScreen = { type: "list" };

interface CurrentOperationSummary {
  title: string;
  detail: string;
}

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
  | { type: "choosingRelink"; sourceId: string }
  | { type: "confirmingRef" }
  | { type: "confirmingRelink" }
  | { type: "discardingRelink" };

type RemovalOperation =
  | { type: "planning"; kind: RemovalKind; targetId: string }
  | { type: "confirming" | "discarding"; planId: string };

export function App({ client = tauriSkillYardClient }: AppProps) {
  const [language, setLanguage] = useState<InterfaceLanguage | null>(null);
  const [aiPreferences, setAiPreferences] = useState<AiPreferences | null>(null);
  const [preferenceError, setPreferenceError] = useState<string | null>(null);
  const [isSavingLanguage, setIsSavingLanguage] = useState(false);
  const [languageError, setLanguageError] = useState<string | null>(null);
  const [aiOperation, setAiOperation] = useState<AiOperation>(null);
  const [aiError, setAiError] = useState<string | null>(null);
  const [agentContext, setAgentContext] = useState<AgentPageContext>({
    type: "page",
    page: "inventory",
  });
  const [agentInstallPlan, setAgentInstallPlan] = useState<InstallPlan | null>(
    null,
  );

  useEffect(() => {
    let active = true;
    client.getPreferences().then(
      (preferences) => {
        if (active) {
          setLanguage(preferences.language);
          setAiPreferences(preferences.ai);
        }
      },
      (error: unknown) => {
        if (active) setPreferenceError(formatErrorMessage(error, "zhCn"));
      },
    );
    return () => {
      active = false;
    };
  }, [client]);

  const changeLanguage = async (nextLanguage: InterfaceLanguage) => {
    if (isSavingLanguage || nextLanguage === language) return;
    setIsSavingLanguage(true);
    setLanguageError(null);
    try {
      const preferences = await client.setInterfaceLanguage(nextLanguage);
      setLanguage(preferences.language);
      setAiPreferences(preferences.ai);
    } catch (error) {
      setLanguageError(formatErrorMessage(error, language ?? "zhCn"));
    } finally {
      setIsSavingLanguage(false);
    }
  };

  const runAiOperation = async (
    operation: Exclude<AiOperation, null>,
    action: () => Promise<{ language: InterfaceLanguage; ai: AiPreferences }>,
  ) => {
    if (aiOperation) return;
    setAiOperation(operation);
    setAiError(null);
    try {
      const preferences = await action();
      setLanguage(preferences.language);
      setAiPreferences(preferences.ai);
    } catch (error) {
      const message = formatErrorMessage(error, language ?? "zhCn");
      // 连接测试必须给出就地失败结论，不能让用户把远处的通用错误误认为按钮无响应。
      setAiError(
        operation === "testing"
          ? language === "en"
            ? `Connection test failed: ${message}`
            : `连接测试失败：${message}`
          : message,
      );
    } finally {
      setAiOperation(null);
    }
  };

  const previewAgentInstall = async (result: AgentSearchResult) => {
    if (result.kind === "reference") return;
    if (result.kind === "directUrl") {
      setAgentInstallPlan(await client.createUrlInstallPlan(result.url));
      return;
    }
    const source = await client.addGithubSource(result.url, null);
    if (source.type === "sourceRefChangePlan") {
      throw new Error(
        language === "en"
          ? "This Source needs a Tracked Ref confirmation on the installation page first."
          : "这个 Source 需要先在安装页确认 Tracked Ref。",
      );
    }
    if (!source.highlightedSourceId) {
      throw new Error(
        language === "en"
          ? "SkillYard could not identify the saved Source."
          : "SkillYard 无法识别刚刚保存的 Source。",
      );
    }
    // Agent 只负责交接来源；安装 Plan 仍必须复用 Source Catalog 的真实加载步骤。
    await client.reloadGithubSource(source.highlightedSourceId);
    setAgentInstallPlan(
      await client.createGithubInstallPlan(source.highlightedSourceId),
    );
  };

  if (preferenceError) {
    return (
      <main className="state-page" role="alert">
        <h1>暂时无法继续</h1>
        <p>{preferenceError}</p>
      </main>
    );
  }
  if (!language || !aiPreferences) {
    return (
      <main className="state-page" aria-label="SkillYard 正在启动">
        <span className="loading-mark" aria-hidden="true" />
        <p>正在读取本机状态…</p>
      </main>
    );
  }

  return (
    <I18nProvider language={language}>
      <>
        <AppCore
          client={client}
          language={language}
          aiPreferences={aiPreferences}
          isSavingLanguage={isSavingLanguage}
          languageError={languageError}
          aiOperation={aiOperation}
          aiError={aiError}
          onAgentContextChange={setAgentContext}
          agentInstallPlan={agentInstallPlan}
          onAgentInstallPlanClear={() => setAgentInstallPlan(null)}
          onLanguageChange={changeLanguage}
          onAiConfigurationChange={(configuration) =>
            runAiOperation("savingConfiguration", () =>
              client.setAiConfiguration(configuration),
            )
          }
          onSaveAiApiKey={(apiKey) =>
            runAiOperation("savingKey", () => client.saveAiApiKey(apiKey))
          }
          onDeleteAiApiKey={() =>
            runAiOperation("deletingKey", () => client.deleteAiApiKey())
          }
          onTestAiConnection={() =>
            runAiOperation("testing", () => client.testAiConnection())
          }
        />
        <AgentOverlay
          context={agentContext}
          aiPreferences={aiPreferences}
          onAsk={(context, messages) => client.askAgent(context, messages)}
          onPreviewInstall={previewAgentInstall}
        />
      </>
    </I18nProvider>
  );
}

function AppCore({
  client,
  language,
  aiPreferences,
  isSavingLanguage,
  languageError,
  aiOperation,
  aiError,
  onLanguageChange,
  onAiConfigurationChange,
  onSaveAiApiKey,
  onDeleteAiApiKey,
  onTestAiConnection,
  onAgentContextChange,
  agentInstallPlan,
  onAgentInstallPlanClear,
}: AppCoreProps) {
  const { t } = useI18n();
  const formatError = (error: unknown) => formatErrorMessage(error, language);
  const [viewState, setViewState] = useState<ViewState>({ status: "loading" });
  const [committedInventory, setCommittedInventory] =
    useState<InventoryOutcome | null>(null);
  const [isBrowsingCommittedInventory, setIsBrowsingCommittedInventory] =
    useState(false);
  const [readOnlyManagedMemberId, setReadOnlyManagedMemberId] = useState<
    string | null
  >(null);
  // 页面级挂载管理会暂时卸载 Inventory，提升 screen 后才能准确回到原 Skill 详情。
  const [inventoryScreen, setInventoryScreen] = useState<InventoryScreen>(
    INVENTORY_LIST_SCREEN,
  );
  const [readOnlyInventoryScreen, setReadOnlyInventoryScreen] =
    useState<InventoryScreen>(INVENTORY_LIST_SCREEN);
  const [isScanning, setIsScanning] = useState(false);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [isCheckingUpdates, setIsCheckingUpdates] = useState(false);
  const [preparingBundleUpdateId, setPreparingBundleUpdateId] = useState<
    string | null
  >(null);
  const [checkingEditableBundleId, setCheckingEditableBundleId] = useState<
    string | null
  >(null);
  const [isPreparingBundleUpdateBatch, setIsPreparingBundleUpdateBatch] =
    useState(false);
  const [isConfirmingBundleUpdateBatch, setIsConfirmingBundleUpdateBatch] =
    useState(false);
  const [isDiscardingBundleUpdateBatch, setIsDiscardingBundleUpdateBatch] =
    useState(false);
  const [
    isAcknowledgingBundleUpdateBatch,
    setIsAcknowledgingBundleUpdateBatch,
  ] = useState(false);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [updateError, setUpdateError] = useState<string | null>(null);
  const [bundleUpdateBatchError, setBundleUpdateBatchError] = useState<
    string | null
  >(null);
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
  const [pendingEditableLocalRelink, setPendingEditableLocalRelink] =
    useState<EditableLocalRelinkPlan | null>(null);
  const [sourceAssociationBundleId, setSourceAssociationBundleId] = useState<
    string | null
  >(null);
  const [pendingSourceAssociationPlan, setPendingSourceAssociationPlan] =
    useState<SourceAssociationPlan | null>(null);
  const [isPlanningSourceAssociation, setIsPlanningSourceAssociation] =
    useState(false);
  const [isConfirmingSourceAssociation, setIsConfirmingSourceAssociation] =
    useState(false);
  const [isDiscardingSourceAssociation, setIsDiscardingSourceAssociation] =
    useState(false);
  const [sourceAssociationError, setSourceAssociationError] = useState<
    string | null
  >(null);
  const [pendingInstallPlan, setPendingInstallPlan] =
    useState<InstallPlan | null>(null);
  const activeInstallPlan = pendingInstallPlan ?? agentInstallPlan;
  const clearActiveInstallPlan = () => {
    setPendingInstallPlan(null);
    onAgentInstallPlanClear();
  };
  const [isInstalling, setIsInstalling] = useState(false);
  const [isDiscardingInstallPlan, setIsDiscardingInstallPlan] = useState(false);
  const [installError, setInstallError] = useState<string | null>(null);
  const [isAddingProject, setIsAddingProject] = useState(false);
  const [pendingProjectSelection, setPendingProjectSelection] =
    useState<ProjectSelection | null>(null);
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
  const [pendingRemovalPlan, setPendingRemovalPlan] =
    useState<RemovalPlan | null>(null);
  const [removalOperation, setRemovalOperation] =
    useState<RemovalOperation | null>(null);
  const [removalError, setRemovalError] = useState<string | null>(null);
  const [selectedRecoveryIssueId, setSelectedRecoveryIssueId] = useState<
    string | null
  >(null);
  const [isOpeningCentralStore, setIsOpeningCentralStore] = useState(false);
  const [recoveryOpenError, setRecoveryOpenError] = useState<string | null>(
    null,
  );
  const [isResettingApplication, setIsResettingApplication] = useState(false);
  const [resetError, setResetError] = useState<string | null>(null);
  const [generatingSkillExplanationId, setGeneratingSkillExplanationId] =
    useState<string | null>(null);
  const [skillExplanationError, setSkillExplanationError] = useState<
    string | null
  >(null);
  const [isAiOrganizationRunning, setIsAiOrganizationRunning] =
    useState(false);
  const [aiOrganizationFeedback, setAiOrganizationFeedback] = useState<
    string | null
  >(null);
  const [aiOrganizationError, setAiOrganizationError] = useState<string | null>(
    null,
  );
  const aiOrganizationFeedbackTimer = useRef<number | null>(null);
  const [inventoryPresentationKey, setInventoryPresentationKey] = useState(0);

  const activeRemovalPlan =
    pendingRemovalPlan ??
    (viewState.status === "ready" &&
    viewState.outcome.type === "removalPlan"
      ? viewState.outcome.plan
      : null);
  const activeEditableLocalRelink =
    pendingEditableLocalRelink ??
    (viewState.status === "ready" &&
    viewState.outcome.type === "editableLocalRelinkPlan"
      ? viewState.outcome.plan
      : null);
  const currentOperation = (() => {
    if (isConfirmingBundleUpdateBatch) {
      return {
        title: t("正在更新所选 Bundle"),
        detail: t("已确认的 Bundle 会依次完成，当前操作不能取消。"),
      };
    }
    if (removalOperation?.type === "confirming") {
      const title =
        activeRemovalPlan?.kind === "bundle"
          ? t("正在删除 Bundle")
          : activeRemovalPlan?.kind === "bundleMounts"
            ? t("正在解除 Bundle 挂载")
          : activeRemovalPlan?.kind === "project"
            ? t("正在移除项目")
            : t("正在删除 Source");
      return {
        title,
        detail: t("SkillYard 正在完成已确认的影响范围，当前操作不能取消。"),
      };
    }
    if (isConfirmingSourceAssociation) {
      return {
        title: t("正在保存来源关联"),
        detail: t("关联或归并会作为一个完整操作完成，当前操作不能取消。"),
      };
    }
    if (sourceOperation?.type === "confirmingRelink") {
      return {
        title: t("正在重新关联 Source 路径"),
        detail: t("只更新来源位置，不会替换正在使用的 Skill 内容。"),
      };
    }
    if (sourceOperation?.type === "confirmingRef") {
      return {
        title: t("正在更改 Source 分支"),
        detail: t("只更新后续来源基线，不会替换正在使用的 Skill 内容。"),
      };
    }
    if (isConfirmingTakeover) {
      return {
        title: t("正在接管 Skill"),
        detail: t("文件迁移、受管内容和 Mount 会作为一个完整操作完成。"),
      };
    }
    if (isInstalling) {
      return {
        title:
          activeInstallPlan?.mode === "update"
            ? t("正在更新 Bundle")
            : t("正在安装 Bundle"),
        detail: t("确认后的文件系统操作不能取消，SkillYard 会自动完成或恢复。"),
      };
    }
    if (isConfirmingMount) {
      return {
        title: t("正在修改 Mount"),
        detail: t("目标路径与登记状态会作为一个完整操作完成。"),
      };
    }
    if (isConfirmingBatchMount) {
      return {
        title: t("正在批量挂载 Bundle"),
        detail: t("所选 Mount 会作为一个完整操作完成，当前操作不能取消。"),
      };
    }
    return null;
  })() satisfies CurrentOperationSummary | null;
  const hasCurrentOperation = currentOperation !== null;
  const isInventoryWriteBlocked =
    isResettingApplication ||
    isRefreshing ||
    isCheckingUpdates ||
    preparingBundleUpdateId !== null ||
    checkingEditableBundleId !== null ||
    isPreparingBundleUpdateBatch ||
    isAddingProject ||
    pendingProjectSelection !== null ||
    removalOperation !== null ||
    sourceOperation?.type === "opening";

  const currentAgentContext = useMemo<AgentPageContext>(() => {
    if (sourceDiscovery) {
      return { type: "page", page: "sourceDiscovery" };
    }
    if (viewState.status !== "ready") {
      return { type: "page", page: "inventory" };
    }
    if (viewState.outcome.type === "onboardingRequired") {
      return { type: "page", page: "onboarding" };
    }
    if (viewState.outcome.type === "unsupportedPlatform") {
      return { type: "page", page: "unsupportedPlatform" };
    }
    const displayedInventory =
      isBrowsingCommittedInventory && committedInventory
        ? committedInventory
        : viewState.outcome.type === "inventory"
          ? viewState.outcome
          : null;
    const displayedScreen = isBrowsingCommittedInventory
      ? readOnlyInventoryScreen
      : inventoryScreen;
    if (displayedInventory && displayedScreen.type === "skill") {
      const entry = displayedInventory.entries.find(
        (candidate) => candidate.id === displayedScreen.entryId,
      );
      if (entry) {
        return { type: "skill", inventoryId: entry.id };
      }
    }
    if (displayedInventory && displayedScreen.type === "settings") {
      return { type: "page", page: "settings" };
    }
    if (displayedInventory && !hasCurrentOperation) {
      return { type: "page", page: "inventory" };
    }
    return { type: "page", page: "operation" };
  }, [
    committedInventory,
    hasCurrentOperation,
    inventoryScreen,
    isBrowsingCommittedInventory,
    readOnlyInventoryScreen,
    sourceDiscovery,
    viewState,
  ]);

  useEffect(() => {
    onAgentContextChange(currentAgentContext);
  }, [currentAgentContext, onAgentContextChange]);

  const openCentralStore = async () => {
    if (isOpeningCentralStore) return;
    setRecoveryOpenError(null);
    setIsOpeningCentralStore(true);
    try {
      await client.openCentralStore();
    } catch (error) {
      setRecoveryOpenError(formatError(error));
    } finally {
      setIsOpeningCentralStore(false);
    }
  };

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

  useEffect(() => {
    if (viewState.status === "ready" && viewState.outcome.type === "inventory") {
      // 事务期间只展示最近一次完整提交的 read model，不能混入正在写入的中间状态。
      setCommittedInventory(viewState.outcome);
    }
  }, [viewState]);

  useEffect(() => {
    if (!hasCurrentOperation) {
      setIsBrowsingCommittedInventory(false);
      setReadOnlyManagedMemberId(null);
    }
  }, [hasCurrentOperation]);

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

  const checkBundleUpdates = async () => {
    if (
      isCheckingUpdates ||
      preparingBundleUpdateId ||
      checkingEditableBundleId
    ) {
      return;
    }
    setIsCheckingUpdates(true);
    setUpdateError(null);
    try {
      const outcome = await client.checkBundleUpdates();
      setViewState({ status: "ready", outcome });
    } catch (error) {
      // 检查失败不抹掉上次结果，也不能改变当前 Bundle 内容或 Mount。
      setUpdateError(formatError(error));
    } finally {
      setIsCheckingUpdates(false);
    }
  };

  const createBundleUpdatePlan = async (bundleId: string) => {
    if (
      isCheckingUpdates ||
      preparingBundleUpdateId ||
      checkingEditableBundleId
    ) {
      return;
    }
    setPreparingBundleUpdateId(bundleId);
    setUpdateError(null);
    // 上一次确认失败已经回到 Inventory，新的 Plan 不能继承旧确认页错误。
    setInstallError(null);
    try {
      setPendingInstallPlan(await client.createBundleUpdatePlan(bundleId));
    } catch (error) {
      // 准备 Plan 尚未切换 Current Content，失败后保留主清单和上次检查结果。
      setUpdateError(formatError(error));
    } finally {
      setPreparingBundleUpdateId(null);
    }
  };

  const chooseBundleReplacementPlan = async (bundleId: string) => {
    if (
      isCheckingUpdates ||
      preparingBundleUpdateId ||
      checkingEditableBundleId
    ) {
      return;
    }
    setPreparingBundleUpdateId(bundleId);
    setUpdateError(null);
    setInstallError(null);
    try {
      const plan = await client.chooseBundleReplacementPlan(bundleId);
      // 取消原生选择器返回 null；当前 Inventory 和 Bundle 状态保持不变。
      if (plan) setPendingInstallPlan(plan);
    } catch (error) {
      setUpdateError(formatError(error));
    } finally {
      setPreparingBundleUpdateId(null);
    }
  };

  const checkEditableLocalBundle = async (bundleId: string) => {
    if (
      isCheckingUpdates ||
      preparingBundleUpdateId ||
      checkingEditableBundleId
    ) {
      return;
    }
    setCheckingEditableBundleId(bundleId);
    setUpdateError(null);
    setInstallError(null);
    try {
      // Rust 返回完整 Inventory；前端不根据文件时间或内容自行推断更新状态。
      const outcome = await client.checkEditableLocalBundle(bundleId);
      setViewState({ status: "ready", outcome });
    } catch (error) {
      setUpdateError(formatError(error));
    } finally {
      setCheckingEditableBundleId(null);
    }
  };

  const createBundleUpdateBatchPlan = async () => {
    if (
      isPreparingBundleUpdateBatch ||
      isCheckingUpdates ||
      preparingBundleUpdateId ||
      checkingEditableBundleId
    ) {
      return;
    }
    setIsPreparingBundleUpdateBatch(true);
    setUpdateError(null);
    setBundleUpdateBatchError(null);
    try {
      const outcome = await client.createBundleUpdateBatchPlan();
      setViewState({ status: "ready", outcome });
    } catch (error) {
      // 准备失败没有进入批量确认页，保留当前清单和每个 Bundle 的检查结果。
      setUpdateError(formatError(error));
    } finally {
      setIsPreparingBundleUpdateBatch(false);
    }
  };

  const discardBundleUpdateBatchPlan = async (planId: string) => {
    if (
      viewState.status !== "ready" ||
      viewState.outcome.type !== "bundleUpdateBatchPlan" ||
      viewState.outcome.plan.id !== planId ||
      isConfirmingBundleUpdateBatch ||
      isDiscardingBundleUpdateBatch
    ) {
      return;
    }
    setIsDiscardingBundleUpdateBatch(true);
    setBundleUpdateBatchError(null);
    try {
      const outcome = await client.discardBundleUpdateBatchPlan(planId);
      setViewState({ status: "ready", outcome });
    } catch (error) {
      // 必须由 Rust 确认全部 child Plan 已清理后，页面才能返回 Inventory。
      setBundleUpdateBatchError(formatError(error));
    } finally {
      setIsDiscardingBundleUpdateBatch(false);
    }
  };

  const confirmBundleUpdateBatchPlan = async (
    planId: string,
    selectedItemIds: string[],
  ) => {
    if (
      viewState.status !== "ready" ||
      viewState.outcome.type !== "bundleUpdateBatchPlan" ||
      viewState.outcome.plan.id !== planId ||
      isConfirmingBundleUpdateBatch ||
      isDiscardingBundleUpdateBatch
    ) {
      return;
    }
    setIsConfirmingBundleUpdateBatch(true);
    setBundleUpdateBatchError(null);
    try {
      const outcome = await client.confirmBundleUpdateBatchPlan(
        planId,
        selectedItemIds,
      );
      setViewState({ status: "ready", outcome });
    } catch (error) {
      const message = formatError(error);
      try {
        // 确认可能已经消费 Plan；重读唯一持久状态，不能让旧页面自行决定是否可重试。
        const outcome = await client.getStartupState();
        setViewState({ status: "ready", outcome });
        if (outcome.type === "inventory") {
          setUpdateError(message);
        } else {
          setBundleUpdateBatchError(message);
        }
      } catch (recoveryError) {
        setViewState({
          status: "error",
          message: t("{message}；重新读取状态失败：{recoveryError}", {
            message,
            recoveryError: formatError(recoveryError),
          }),
        });
      }
    } finally {
      setIsConfirmingBundleUpdateBatch(false);
    }
  };

  const acknowledgeBundleUpdateBatchResult = async (batchId: string) => {
    if (
      viewState.status !== "ready" ||
      viewState.outcome.type !== "bundleUpdateBatchResult" ||
      viewState.outcome.result.id !== batchId ||
      viewState.outcome.result.status !== "completed" ||
      isAcknowledgingBundleUpdateBatch
    ) {
      return;
    }
    setIsAcknowledgingBundleUpdateBatch(true);
    setBundleUpdateBatchError(null);
    try {
      const outcome =
        await client.acknowledgeBundleUpdateBatchResult(batchId);
      setViewState({ status: "ready", outcome });
    } catch (error) {
      setBundleUpdateBatchError(formatError(error));
    } finally {
      setIsAcknowledgingBundleUpdateBatch(false);
    }
  };

  const createRemovalPlan = async (
    kind: RemovalKind,
    targetId: string,
  ) => {
    if (removalOperation || sourceOperation) return;
    setRemovalOperation({ type: "planning", kind, targetId });
    setRemovalError(null);
    if (kind === "source") setSourceError(null);
    try {
      const outcome =
        kind === "project"
          ? await client.createProjectRemovalPlan(targetId)
          : kind === "source"
            ? await client.createSourceRemovalPlan(targetId)
            : kind === "bundleMounts"
              ? await client.createBundleMountRemovalPlan(targetId)
              : await client.createBundleRemovalPlan(targetId);
      setPendingRemovalPlan(outcome.plan);
    } catch (error) {
      // Plan 创建仍是只读预览；失败后保留入口所在页面和已提交 read model。
      setRemovalError(formatError(error));
    } finally {
      setRemovalOperation(null);
    }
  };

  const applyRemovalOutcome = async (outcome: UiOutcome) => {
    setPendingRemovalPlan(null);
    setManagedMemberId(null);
    if (outcome.type === "sourceDiscovery") {
      setSkillsShSearch(null);
      setSourceDiscovery(outcome);
      try {
        // Source 删除会同步改变 Bundle 更新能力，返回清单前必须补读最新 Inventory。
        const startupOutcome = await client.getStartupState();
        setViewState({ status: "ready", outcome: startupOutcome });
      } catch (error) {
        setViewState({
          status: "error",
          message: t("Source 已处理，但重新读取本机清单失败：{error}", {
            error: formatError(error),
          }),
        });
      }
      return;
    }
    setSourceDiscovery(null);
    setViewState({ status: "ready", outcome });
  };

  const discardRemovalPlan = async (planId: string) => {
    if (
      !activeRemovalPlan ||
      activeRemovalPlan.id !== planId ||
      removalOperation
    ) {
      return;
    }
    setRemovalOperation({ type: "discarding", planId });
    setRemovalError(null);
    try {
      await applyRemovalOutcome(await client.discardRemovalPlan(planId));
    } catch (error) {
      // 只有 Rust 确认 Plan 与预览资源清理完成后，页面才允许返回。
      setRemovalError(formatError(error));
    } finally {
      setRemovalOperation(null);
    }
  };

  const confirmRemovalPlan = async (planId: string) => {
    if (
      !activeRemovalPlan ||
      activeRemovalPlan.id !== planId ||
      removalOperation
    ) {
      return;
    }
    setRemovalOperation({ type: "confirming", planId });
    setRemovalError(null);
    try {
      await applyRemovalOutcome(await client.confirmRemovalPlan(planId));
    } catch (error) {
      const message = formatError(error);
      // 确认后 Plan 可能已经消费；必须丢弃本地页面并读取唯一持久状态。
      setPendingRemovalPlan(null);
      try {
        const outcome = await client.getStartupState();
        setViewState({ status: "ready", outcome });
        setSourceDiscovery(
          outcome.type === "sourceDiscovery" ? outcome : null,
        );
        setRemovalError(message);
      } catch (recoveryError) {
        setViewState({
          status: "error",
          message: t("{message}；重新读取状态失败：{recoveryError}", {
            message,
            recoveryError: formatError(recoveryError),
          }),
        });
      }
    } finally {
      setRemovalOperation(null);
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

  const openSourceAssociation = async (bundleId: string) => {
    if (sourceOperation) return;
    setSourceOperation({ type: "opening" });
    setSourceAssociationBundleId(bundleId);
    setSourceAssociationError(null);
    setSourceError(null);
    try {
      setSourceDiscovery(await client.openSourceDiscovery());
    } catch (error) {
      // 来源页读取失败时留在清单，不让一个半打开的关联流程遮住现有 Bundle。
      setSourceAssociationBundleId(null);
      setSourceAssociationError(formatError(error));
    } finally {
      setSourceOperation(null);
    }
  };

  const createSourceAssociationPlan = async (
    bundleId: string,
    sourceId: string,
    memberChoices: SourceMemberMappingChoice[],
  ) => {
    if (isPlanningSourceAssociation) return;
    setIsPlanningSourceAssociation(true);
    setSourceAssociationError(null);
    try {
      setPendingSourceAssociationPlan(
        await client.createSourceAssociationPlan(
          bundleId,
          sourceId,
          memberChoices,
        ),
      );
    } catch (error) {
      // 创建 Plan 尚未改变持久化关系，保留映射页供用户调整。
      setSourceAssociationError(formatError(error));
    } finally {
      setIsPlanningSourceAssociation(false);
    }
  };

  const discardSourceAssociationPlan = async () => {
    if (
      !pendingSourceAssociationPlan ||
      isConfirmingSourceAssociation ||
      isDiscardingSourceAssociation
    ) {
      return;
    }
    setIsDiscardingSourceAssociation(true);
    setSourceAssociationError(null);
    try {
      await client.discardSourceAssociationPlan(
        pendingSourceAssociationPlan.id,
      );
      setPendingSourceAssociationPlan(null);
      setSourceAssociationBundleId(null);
      setSourceDiscovery(null);
    } catch (error) {
      // 只有 Rust 确认丢弃成功后才离开 Plan，避免把失败伪装成取消。
      setSourceAssociationError(formatError(error));
    } finally {
      setIsDiscardingSourceAssociation(false);
    }
  };

  const confirmSourceAssociationPlan = async (
    contentChoices: SourceAssociationContentChoice[],
  ) => {
    if (!pendingSourceAssociationPlan || isConfirmingSourceAssociation) return;
    setIsConfirmingSourceAssociation(true);
    setSourceAssociationError(null);
    try {
      const confirmedOutcome = await client.confirmSourceAssociationPlan(
        pendingSourceAssociationPlan.id,
        contentChoices,
      );
      try {
        // 确认后以重新读取的持久化清单为准，不使用 Plan 页面推断成功状态。
        const outcome = await client.getStartupState();
        setViewState({ status: "ready", outcome });
      } catch (error) {
        setViewState({ status: "ready", outcome: confirmedOutcome });
        setSourceAssociationError(
          t("来源关系已处理，但重新读取清单失败：{error}", {
            error: formatError(error),
          }),
        );
      }
      setPendingSourceAssociationPlan(null);
      setSourceAssociationBundleId(null);
      setSourceDiscovery(null);
    } catch (error) {
      const message = formatError(error);
      // 确认失败后旧 Plan 可能已经消费；离开确认页并读取唯一持久化结果。
      setPendingSourceAssociationPlan(null);
      setSourceAssociationBundleId(null);
      setSourceDiscovery(null);
      try {
        const outcome = await client.getStartupState();
        setViewState({ status: "ready", outcome });
        setSourceAssociationError(message);
      } catch (recoveryError) {
        setViewState({
          status: "error",
          message: t("{message}；重新读取状态失败：{recoveryError}", {
            message,
            recoveryError: formatError(recoveryError),
          }),
        });
      }
    } finally {
      setIsConfirmingSourceAssociation(false);
    }
  };

  const returnToInventory = () => {
    if (sourceOperation) return;
    setSourceError(null);
    setSkillsShSearch(null);
    setSourceDiscovery(null);
  };

  const resetApplication = async () => {
    if (isInventoryWriteBlocked) return;

    setIsResettingApplication(true);
    setResetError(null);
    // 1.0 没有持久化偏好；重置只丢弃当前窗口中的临时展示状态。
    setIsBrowsingCommittedInventory(false);
    setReadOnlyManagedMemberId(null);
    setInventoryScreen(INVENTORY_LIST_SCREEN);
    setReadOnlyInventoryScreen(INVENTORY_LIST_SCREEN);
    setSourceDiscovery(null);
    setSkillsShSearch(null);
    setPendingSourceRefChange(null);
    setPendingEditableLocalRelink(null);
    setSourceAssociationBundleId(null);
    setPendingSourceAssociationPlan(null);
    setPendingInstallPlan(null);
    setTakeoverObservationId(null);
    setPendingTakeoverPlan(null);
    setManagedMemberId(null);
    setPendingMountPlan(null);
    setBatchMountBundleId(null);
    setPendingBatchMountPlan(null);
    setPendingRemovalPlan(null);
    setSelectedRecoveryIssueId(null);
    setRefreshError(null);
    setUpdateError(null);
    setBundleUpdateBatchError(null);
    setSourceError(null);
    setSourceAssociationError(null);
    setInstallError(null);
    setProjectError(null);
    setTakeoverError(null);
    setMountError(null);
    setRemovalError(null);
    setRecoveryOpenError(null);
    setInventoryPresentationKey((current) => current + 1);

    try {
      // 重新读取唯一持久状态，证明重置不会伪造或删除托管数据。
      const outcome = await client.getStartupState();
      setViewState({ status: "ready", outcome });
    } catch (error) {
      // 读取失败时保留上一次清单，避免把临时错误误表现成数据被重置。
      setResetError(formatError(error));
    } finally {
      setIsResettingApplication(false);
    }
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
        t("{message}；重新读取 Source 状态失败：{recoveryError}", {
          message,
          recoveryError: formatError(recoveryError),
        }),
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
      try {
        // Ref 确认会同步改变 Bundle 更新状态，主清单必须读取同一份持久结果。
        const inventory = await client.getStartupState();
        setViewState({ status: "ready", outcome: inventory });
      } catch (error) {
        setSourceError(
          t("Tracked Ref 已更改，但重新读取清单失败：{error}", {
            error: formatError(error),
          }),
        );
      }
    } catch (error) {
      // 确认失败后丢弃旧页面并重读持久状态，避免用户重试结果不确定的 Plan。
      setPendingSourceRefChange(null);
      await rereadSourceAfterFailure(error);
      try {
        // 确认可能已提交但后续步骤报错，不能让主清单继续保留旧 marker。
        const inventory = await client.getStartupState();
        setViewState({ status: "ready", outcome: inventory });
      } catch (recoveryError) {
        setSourceError(
          t("{message}；重新读取清单失败：{recoveryError}", {
            message: formatError(error),
            recoveryError: formatError(recoveryError),
          }),
        );
      }
    } finally {
      setSourceOperation(null);
    }
  };

  const chooseEditableLocalRelink = async (sourceId: string) => {
    if (sourceOperation) return;
    setSourceOperation({ type: "choosingRelink", sourceId });
    setSourceError(null);
    try {
      const plan = await client.chooseEditableLocalRelinkPlan(sourceId);
      if (plan) setPendingEditableLocalRelink(plan);
    } catch (error) {
      setSourceError(formatError(error));
    } finally {
      setSourceOperation(null);
    }
  };

  const confirmEditableLocalRelink = async () => {
    if (!activeEditableLocalRelink || sourceOperation) return;
    setSourceOperation({ type: "confirmingRelink" });
    setSourceError(null);
    try {
      const outcome = await client.confirmEditableLocalRelinkPlan(
        activeEditableLocalRelink.id,
      );
      setPendingEditableLocalRelink(null);
      setSourceDiscovery(outcome);
      const inventory = await client.getStartupState();
      setViewState({ status: "ready", outcome: inventory });
    } catch (error) {
      setSourceError(formatError(error));
    } finally {
      setSourceOperation(null);
    }
  };

  const discardEditableLocalRelink = async () => {
    if (!activeEditableLocalRelink || sourceOperation) return;
    setSourceOperation({ type: "discardingRelink" });
    setSourceError(null);
    try {
      const outcome = await client.discardEditableLocalRelinkPlan(
        activeEditableLocalRelink.id,
      );
      setPendingEditableLocalRelink(null);
      setSourceDiscovery(outcome);
      const inventory = await client.getStartupState();
      setViewState({ status: "ready", outcome: inventory });
    } catch (error) {
      setSourceError(formatError(error));
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
    if (!activeInstallPlan || isInstalling || isDiscardingInstallPlan) return;
    setIsDiscardingInstallPlan(true);
    setInstallError(null);
    try {
      await client.discardInstallPlan(activeInstallPlan.id);
      clearActiveInstallPlan();
    } catch (error) {
      if (errorCode(error) === "installPlanConsumed") {
        const message = formatError(error);
        try {
          // 另一实例已经开始确认时，本页 Plan 永远不能再使用；回到持久化最终状态。
          const outcome = await client.getStartupState();
          clearActiveInstallPlan();
          setSourceDiscovery(null);
          setViewState({ status: "ready", outcome });
          setInstallError(message);
        } catch (recoveryError) {
          clearActiveInstallPlan();
          setSourceDiscovery(null);
          setViewState({
            status: "error",
            message: t("{message}；重新读取状态失败：{recoveryError}", {
              message,
              recoveryError: formatError(recoveryError),
            }),
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
    if (!activeInstallPlan || isInstalling) return;
    setIsInstalling(true);
    setInstallError(null);
    try {
      const outcome = await client.confirmInstallPlan(
        activeInstallPlan.id,
        selectedCandidateIds,
      );
      clearActiveInstallPlan();
      setSourceDiscovery(null);
      setViewState({ status: "ready", outcome });
    } catch (error) {
      const message = formatError(error);
      // Plan 一旦确认就可能已消费；失败后重新读取后端最终状态，不能让用户重试旧 Plan。
      try {
        const outcome = await client.getStartupState();
        clearActiveInstallPlan();
        setSourceDiscovery(null);
        setViewState({ status: "ready", outcome });
        setInstallError(message);
      } catch (recoveryError) {
        clearActiveInstallPlan();
        setSourceDiscovery(null);
        setViewState({
          status: "error",
          message: t("{message}；重新读取状态失败：{recoveryError}", {
            message,
            recoveryError: formatError(recoveryError),
          }),
        });
      }
    } finally {
      setIsInstalling(false);
    }
  };

  const generateSkillExplanation = async (inventoryId: string) => {
    if (generatingSkillExplanationId) return;
    setGeneratingSkillExplanationId(inventoryId);
    setSkillExplanationError(null);
    try {
      const outcome = await client.generateSkillAiExplanation(inventoryId);
      setViewState({ status: "ready", outcome });
      setCommittedInventory(outcome);
    } catch (error) {
      setSkillExplanationError(formatError(error));
    } finally {
      setGeneratingSkillExplanationId(null);
    }
  };

  const organizeSkillExplanations = () => {
    if (isAiOrganizationRunning) return;
    setIsAiOrganizationRunning(true);
    setAiOrganizationError(null);
    const feedback = t("AI 整理已在后台开始");
    setAiOrganizationFeedback(feedback);
    if (aiOrganizationFeedbackTimer.current !== null) {
      window.clearTimeout(aiOrganizationFeedbackTimer.current);
    }
    aiOrganizationFeedbackTimer.current = window.setTimeout(() => {
      setAiOrganizationFeedback(null);
      aiOrganizationFeedbackTimer.current = null;
    }, 4_000);
    void client.organizeSkillAiExplanations().then(
      (outcome) => {
        // 后台完成只刷新 Inventory read model，不能把用户从当前工作页面拉走。
        setCommittedInventory(outcome);
        setViewState((current) =>
          current.status === "ready" && current.outcome.type === "inventory"
            ? { status: "ready", outcome }
            : current,
        );
        setIsAiOrganizationRunning(false);
      },
      (error: unknown) => {
        if (aiOrganizationFeedbackTimer.current !== null) {
          window.clearTimeout(aiOrganizationFeedbackTimer.current);
          aiOrganizationFeedbackTimer.current = null;
        }
        setAiOrganizationFeedback(null);
        setAiOrganizationError(formatError(error));
        setIsAiOrganizationRunning(false);
      },
    );
  };

  useEffect(
    () => () => {
      if (aiOrganizationFeedbackTimer.current !== null) {
        window.clearTimeout(aiOrganizationFeedbackTimer.current);
      }
    },
    [],
  );

  const chooseProjectDirectory = async () => {
    if (isAddingProject) return;
    setIsAddingProject(true);
    setProjectError(null);
    try {
      const selection = await client.chooseProjectDirectory();
      // 取消原生选择器返回 null；只有明确选择后才显示应用内确认弹窗。
      if (selection) setPendingProjectSelection(selection);
    } catch (error) {
      setProjectError(formatError(error));
    } finally {
      setIsAddingProject(false);
    }
  };

  const confirmProjectRegistration = async () => {
    if (!pendingProjectSelection || isAddingProject) return;
    setIsAddingProject(true);
    setProjectError(null);
    try {
      const outcome = await client.registerProject(
        pendingProjectSelection.rootPath,
      );
      setPendingProjectSelection(null);
      setViewState({ status: "ready", outcome });
    } catch (error) {
      // 登记失败时保留用户选中的路径，允许用户直接重试或取消。
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
          message: t("{message}；重新读取状态失败：{recoveryError}", {
            message,
            recoveryError: formatError(recoveryError),
          }),
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
          t("{message}；重新检查挂载状态失败：{recoveryError}", {
            message,
            recoveryError: formatError(refreshFailure),
          }),
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
          message: t("{message}；重新读取状态失败：{recoveryError}", {
            message,
            recoveryError: formatError(recoveryError),
          }),
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
          message: t("{message}；重新读取状态失败：{recoveryError}", {
            message,
            recoveryError: formatError(recoveryError),
          }),
        });
      }
    } finally {
      setIsConfirmingBatchMount(false);
    }
  };

  const readOnlyInventory = committedInventory ? (
    <InventoryPage
      outcome={committedInventory}
      screen={readOnlyInventoryScreen}
      onScreenChange={setReadOnlyInventoryScreen}
      language={language}
      aiPreferences={aiPreferences}
      isSavingLanguage={isSavingLanguage}
      languageError={languageError}
      aiOperation={aiOperation}
      aiError={aiError}
      onLanguageChange={onLanguageChange}
      onAiConfigurationChange={onAiConfigurationChange}
      onSaveAiApiKey={onSaveAiApiKey}
      onDeleteAiApiKey={onDeleteAiApiKey}
      onTestAiConnection={onTestAiConnection}
      isWriteBlocked
      allowReadOnlyDetails
      isRefreshing={false}
      isCheckingUpdates={false}
      preparingBundleUpdateId={null}
      checkingEditableBundleId={null}
      isPreparingBundleUpdateBatch={false}
      removingBundleId={null}
      unmountingBundleId={null}
      removingProjectId={null}
      isOpeningInstaller={false}
      isAddingProject={false}
      isOpeningCentralStore={false}
      isResettingApplication={false}
      refreshError={null}
      updateError={null}
      installError={null}
      projectError={null}
      removalError={null}
      mountError={null}
      takeoverError={null}
      sourceAssociationError={null}
      centralStoreError={null}
      resetError={null}
      generatingSkillExplanationId={null}
      skillExplanationError={null}
      isAiOrganizationRunning={false}
      aiOrganizationFeedback={null}
      aiOrganizationError={null}
      onRefresh={() => undefined}
      onCheckUpdates={() => undefined}
      onDismissUpdateError={() => undefined}
      onUpdateBundle={() => undefined}
      onChooseBundleReplacement={() => undefined}
      onCheckEditableLocalBundle={() => undefined}
      onUpdateAll={() => undefined}
      onRemoveBundle={() => undefined}
      onUnmountBundle={() => undefined}
      onRemoveProject={() => undefined}
      onInstall={() => undefined}
      onAddProject={() => undefined}
      onOpenCentralStore={() => undefined}
      onResetApplication={() => undefined}
      onAssociateSource={() => undefined}
      onOpenRecovery={() => undefined}
      onTakeover={() => undefined}
      onManageMount={setReadOnlyManagedMemberId}
      onBatchMount={() => undefined}
      onGenerateSkillExplanation={() => undefined}
      onOrganizeSkillExplanations={() => undefined}
    />
  ) : null;

  const readOnlyManagedEntry = readOnlyManagedMemberId
    ? committedInventory?.entries.find(
        (entry) =>
          entry.managementKind === "skillYardManaged" &&
          entry.memberId === readOnlyManagedMemberId,
      ) ?? null
    : null;
  const readOnlyMountDetails =
    committedInventory && readOnlyManagedEntry ? (
      <MountManagementPage
        entry={readOnlyManagedEntry}
        supportedApps={committedInventory.supportedApps}
        projects={committedInventory.projects}
        mounts={committedInventory.mounts}
        readOnly
        isPlanning={false}
        error={null}
        onBack={() => setReadOnlyManagedMemberId(null)}
        onCreate={() => undefined}
        onRemove={() => undefined}
        onRepair={() => undefined}
      />
    ) : null;

  const renderOperationSurface = (content: ReactNode) => {
    const showReadOnlyContent =
      currentOperation !== null &&
      isBrowsingCommittedInventory &&
      readOnlyInventory !== null;
    return (
      <div className="current-operation-frame">
        {currentOperation ? (
          <CurrentOperationBanner
            title={currentOperation.title}
            detail={currentOperation.detail}
            canBrowse={readOnlyInventory !== null}
            isBrowsing={isBrowsingCommittedInventory}
            onBrowse={() => {
              setReadOnlyManagedMemberId(null);
              setReadOnlyInventoryScreen(INVENTORY_LIST_SCREEN);
              setIsBrowsingCommittedInventory(true);
            }}
            onReturn={() => {
              setReadOnlyManagedMemberId(null);
              setIsBrowsingCommittedInventory(false);
            }}
          />
        ) : null}
        <div className="current-operation-content">
          {showReadOnlyContent
            ? (readOnlyMountDetails ?? readOnlyInventory)
            : content}
        </div>
      </div>
    );
  };

  if (viewState.status === "loading") {
    return (
      <main className="state-page" aria-label={t("正在读取本机状态…")}>
        <span className="spinner" aria-hidden="true" />
        <p>{t("正在读取本机状态…")}</p>
      </main>
    );
  }
  if (viewState.status === "error") {
    return (
      <main className="state-page" role="alert">
        <p className="eyebrow">SKILLYARD · LOCAL ERROR</p>
        <h1>{t("暂时无法继续")}</h1>
        <p>{viewState.message}</p>
      </main>
    );
  }
  if (
    viewState.outcome.type === "bundleUpdateBatchPlan" ||
    viewState.outcome.type === "bundleUpdateBatchResult"
  ) {
    return renderOperationSurface(
      <BundleUpdateBatchPage
        outcome={viewState.outcome}
        isConfirming={isConfirmingBundleUpdateBatch}
        isDiscarding={isDiscardingBundleUpdateBatch}
        isAcknowledging={isAcknowledgingBundleUpdateBatch}
        error={bundleUpdateBatchError}
        onDiscard={discardBundleUpdateBatchPlan}
        onConfirm={confirmBundleUpdateBatchPlan}
        onAcknowledge={acknowledgeBundleUpdateBatchResult}
      />,
    );
  }
  if (activeRemovalPlan) {
    return renderOperationSurface(
      <RemovalPlanPage
        key={activeRemovalPlan.id}
        plan={activeRemovalPlan}
        isConfirming={removalOperation?.type === "confirming"}
        isDiscarding={removalOperation?.type === "discarding"}
        error={removalError}
        onDiscard={discardRemovalPlan}
        onConfirm={confirmRemovalPlan}
      />,
    );
  }
  if (pendingSourceAssociationPlan) {
    return renderOperationSurface(
      <SourceAssociationPlanPage
        plan={pendingSourceAssociationPlan}
        isConfirming={isConfirmingSourceAssociation}
        isDiscarding={isDiscardingSourceAssociation}
        error={sourceAssociationError}
        onBack={discardSourceAssociationPlan}
        onConfirm={confirmSourceAssociationPlan}
      />,
    );
  }
  if (activeEditableLocalRelink) {
    return renderOperationSurface(
      <EditableLocalRelinkPage
        plan={activeEditableLocalRelink}
        isConfirming={sourceOperation?.type === "confirmingRelink"}
        isDiscarding={sourceOperation?.type === "discardingRelink"}
        error={sourceError}
        onDiscard={discardEditableLocalRelink}
        onConfirm={confirmEditableLocalRelink}
      />,
    );
  }
  if (pendingSourceRefChange) {
    return renderOperationSurface(
      <SourceRefChangePage
        plan={pendingSourceRefChange}
        isConfirming={sourceOperation?.type === "confirmingRef"}
        error={sourceError}
        onBack={() => {
          setSourceError(null);
          setPendingSourceRefChange(null);
        }}
        onConfirm={confirmSourceRefChange}
      />,
    );
  }
  if (pendingTakeoverPlan) {
    return renderOperationSurface(
      <TakeoverPlanPage
        plan={pendingTakeoverPlan}
        isConfirming={isConfirmingTakeover}
        onBack={() => setPendingTakeoverPlan(null)}
        onConfirm={confirmTakeover}
      />,
    );
  }
  if (activeInstallPlan) {
    return renderOperationSurface(
      <InstallPlanPage
        plan={activeInstallPlan}
        isInstalling={isInstalling}
        isDiscarding={isDiscardingInstallPlan}
        error={installError}
        onCancel={discardInstallPlan}
        onConfirm={confirmInstall}
      />,
    );
  }
  if (pendingMountPlan) {
    return renderOperationSurface(
      <MountPlanPage
        plan={pendingMountPlan}
        isConfirming={isConfirmingMount}
        onBack={() => setPendingMountPlan(null)}
        onConfirm={confirmMount}
      />,
    );
  }
  if (pendingBatchMountPlan) {
    return renderOperationSurface(
      <BatchMountPlanPage
        plan={pendingBatchMountPlan}
        isConfirming={isConfirmingBatchMount}
        onBack={() => setPendingBatchMountPlan(null)}
        onConfirm={confirmBatchMount}
      />,
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
        <h1>{t("当前 Mac 不受 SkillYard 1.0 支持")}</h1>
        <p>
          {t("需要 macOS {version} 或更高版本的 Apple Silicon Mac。", {
            version: viewState.outcome.minimumMajorVersion,
          })}
        </p>
      </main>
    );
  }

  const selectedRecoveryIssue =
    selectedRecoveryIssueId && viewState.outcome.type === "inventory"
      ? viewState.outcome.recoveryIssues.find(
          (issue) => issue.id === selectedRecoveryIssueId,
        )
      : null;
  if (selectedRecoveryIssue) {
    return (
      <RecoveryPage
        issue={selectedRecoveryIssue}
        isOpeningCentralStore={isOpeningCentralStore}
        error={recoveryOpenError}
        onBack={() => {
          setRecoveryOpenError(null);
          setSelectedRecoveryIssueId(null);
        }}
        onOpenCentralStore={openCentralStore}
      />
    );
  }

  if (
    sourceAssociationBundleId &&
    sourceDiscovery &&
    viewState.outcome.type === "inventory"
  ) {
    const members = viewState.outcome.entries.filter(
      (entry) =>
        entry.managementKind === "skillYardManaged" &&
        entry.bundleId === sourceAssociationBundleId,
    );
    const bundleDisplayName =
      members[0]?.bundleDisplayName ?? t("本地 Bundle");
    if (members.length > 0) {
      return (
        <SourceAssociationSelectionPage
          bundleId={sourceAssociationBundleId}
          bundleDisplayName={bundleDisplayName}
          members={members}
          sources={sourceDiscovery.sources}
          isPlanning={isPlanningSourceAssociation}
          error={sourceAssociationError}
          onBack={() => {
            setSourceAssociationError(null);
            setSourceAssociationBundleId(null);
            setSourceDiscovery(null);
          }}
          onAddSource={() => {
            // 添加 Source 继续复用现有 Source 页面，不另建安装入口。
            setSourceAssociationError(null);
            setSourceAssociationBundleId(null);
          }}
          onCreatePlan={createSourceAssociationPlan}
        />
      );
    }
  }

  if (sourceDiscovery) {
    return (
      <SourceCatalogPage
        outcome={sourceDiscovery}
        skillsShSearch={skillsShSearch}
        mounts={
          viewState.outcome.type === "inventory" ? viewState.outcome.mounts : []
        }
        operation={
          removalOperation?.type === "planning" &&
          removalOperation.kind === "source"
            ? {
                type: "planningRemoval",
                sourceId: removalOperation.targetId,
              }
            : sourceOperation
        }
        error={sourceError ?? removalError}
        onBack={returnToInventory}
        onAddSource={addGithubSource}
        onSearchSkillsSh={searchSkillsSh}
        onChooseFolder={chooseFolderInstallPlan}
        onChooseArchive={chooseArchiveInstallPlan}
        onChooseEditable={chooseEditableLocalInstallPlan}
        onInstallUrl={createUrlInstallPlan}
        onReload={reloadGithubSource}
        onInstall={createGithubInstallPlan}
        onRelink={chooseEditableLocalRelink}
        onRemoveSource={(sourceId) =>
          createRemovalPlan("source", sourceId)
        }
      />
    );
  }

  if (viewState.outcome.type !== "inventory") {
    return (
      <main className="state-page" role="alert">
        <h1>{t("暂时无法继续")}</h1>
        <p>{t("SkillYard 收到了无法显示的应用状态。")}</p>
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
    const groupedCandidates = takeoverEntry.takeoverGroupId
      ? viewState.outcome.entries.filter(
          (entry) =>
            entry.managementKind === "takeoverCandidate" &&
            entry.takeoverGroupId === takeoverEntry.takeoverGroupId,
        )
      : [];
    const groupedSkillNames = new Set(
      groupedCandidates.map((entry) => entry.skillName),
    );
    const takeoverCandidates = takeoverEntry.takeoverGroupId
      ? viewState.outcome.entries.filter(
          (entry) =>
            entry.managementKind === "takeoverCandidate" &&
            (entry.takeoverGroupId === takeoverEntry.takeoverGroupId ||
              (!entry.takeoverGroupId &&
                groupedSkillNames.has(entry.skillName))),
        )
      : viewState.outcome.entries.filter(
          (entry) =>
            entry.managementKind === "takeoverCandidate" &&
            !entry.takeoverGroupId &&
            entry.skillName === takeoverEntry.skillName,
        );
    return (
      <TakeoverSelectionPage
        initialObservationId={takeoverEntry.id}
        candidates={takeoverCandidates}
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
        readOnly={false}
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
    <>
      <InventoryPage
      key={inventoryPresentationKey}
      outcome={viewState.outcome}
      screen={inventoryScreen}
      onScreenChange={setInventoryScreen}
      language={language}
      aiPreferences={aiPreferences}
      isSavingLanguage={isSavingLanguage}
      languageError={languageError}
      aiOperation={aiOperation}
      aiError={aiError}
      onLanguageChange={onLanguageChange}
      onAiConfigurationChange={onAiConfigurationChange}
      onSaveAiApiKey={onSaveAiApiKey}
      onDeleteAiApiKey={onDeleteAiApiKey}
      onTestAiConnection={onTestAiConnection}
      // 任一主界面写操作进行中时统一冻结其他写入口；搜索和筛选仍可使用。
      isWriteBlocked={isInventoryWriteBlocked}
      allowReadOnlyDetails={false}
      isRefreshing={isRefreshing}
      isCheckingUpdates={isCheckingUpdates}
      isOpeningInstaller={sourceOperation?.type === "opening"}
      isAddingProject={isAddingProject}
      isOpeningCentralStore={isOpeningCentralStore}
      isResettingApplication={isResettingApplication}
      refreshError={refreshError}
      updateError={updateError}
      installError={installError ?? sourceError}
      projectError={projectError}
      removalError={removalError}
      mountError={mountError}
      takeoverError={takeoverError}
      sourceAssociationError={sourceAssociationError}
      centralStoreError={recoveryOpenError}
      resetError={resetError}
      generatingSkillExplanationId={generatingSkillExplanationId}
      skillExplanationError={skillExplanationError}
      isAiOrganizationRunning={isAiOrganizationRunning}
      aiOrganizationFeedback={aiOrganizationFeedback}
      aiOrganizationError={aiOrganizationError}
      onRefresh={refreshLocalInventory}
      onCheckUpdates={checkBundleUpdates}
      onDismissUpdateError={() => setUpdateError(null)}
      preparingBundleUpdateId={preparingBundleUpdateId}
      checkingEditableBundleId={checkingEditableBundleId}
      isPreparingBundleUpdateBatch={isPreparingBundleUpdateBatch}
      removingBundleId={
        removalOperation?.type === "planning" &&
        removalOperation.kind === "bundle"
          ? removalOperation.targetId
          : null
      }
      unmountingBundleId={
        removalOperation?.type === "planning" &&
        removalOperation.kind === "bundleMounts"
          ? removalOperation.targetId
          : null
      }
      removingProjectId={
        removalOperation?.type === "planning" &&
        removalOperation.kind === "project"
          ? removalOperation.targetId
          : null
      }
      onUpdateBundle={createBundleUpdatePlan}
      onChooseBundleReplacement={chooseBundleReplacementPlan}
      onCheckEditableLocalBundle={checkEditableLocalBundle}
      onUpdateAll={createBundleUpdateBatchPlan}
      onRemoveBundle={(bundleId) =>
        createRemovalPlan("bundle", bundleId)
      }
      onUnmountBundle={(bundleId) =>
        createRemovalPlan("bundleMounts", bundleId)
      }
      onRemoveProject={(projectId) =>
        createRemovalPlan("project", projectId)
      }
      onInstall={openSourceDiscovery}
      onAddProject={chooseProjectDirectory}
      onOpenCentralStore={openCentralStore}
      onResetApplication={resetApplication}
      onAssociateSource={openSourceAssociation}
      onOpenRecovery={(issueId) => {
        setRecoveryOpenError(null);
        setSelectedRecoveryIssueId(issueId);
      }}
      onTakeover={openTakeover}
      onManageMount={openMountManager}
      onBatchMount={openBatchMount}
      onGenerateSkillExplanation={generateSkillExplanation}
      onOrganizeSkillExplanations={organizeSkillExplanations}
      />
      {pendingProjectSelection ? (
        <ProjectConfirmationDialog
          selection={pendingProjectSelection}
          isConfirming={isAddingProject}
          error={projectError}
          onCancel={() => {
            setProjectError(null);
            setPendingProjectSelection(null);
          }}
          onConfirm={confirmProjectRegistration}
        />
      ) : null}
    </>
  );
}

function formatErrorMessage(
  error: unknown,
  language: InterfaceLanguage = "zhCn",
): string {
  const message =
    error instanceof Error
      ? error.message
      : typeof error === "object" &&
          error !== null &&
          "message" in error &&
          typeof error.message === "string"
        ? error.message
        : null;
  if (message && (language === "zhCn" || !containsCjk(message))) {
    return message;
  }
  // Tauri command 会把 Rust 的结构化 UiError 作为普通对象传给前端。
  if (language === "en") {
    const code = errorCode(error);
    return {
      storageError: "SkillYard could not access its local data.",
      lifecycleError: "SkillYard could not complete the filesystem operation.",
      mountError: "SkillYard could not complete the Mount operation.",
      takeoverError: "SkillYard could not complete the takeover operation.",
      sourceError: "SkillYard could not complete the Source operation.",
      sourceAssociationError:
        "SkillYard could not complete the Source association.",
      bundleUpdateBatchError:
        "SkillYard could not complete the Bundle update operation.",
      removalError: "SkillYard could not complete the removal operation.",
      invalidState: "SkillYard cannot continue from the current state.",
      operationInProgress: "Another high-assurance operation is in progress.",
      operationGateUnavailable:
        "SkillYard cannot access the operation coordinator.",
      dialogError: "The selected item could not be read.",
      invalidPath: "The selected path cannot be stored by SkillYard.",
      openPathError: "The Central Store could not be opened in Finder.",
      invalidOutcome: "SkillYard returned an unexpected application state.",
      installPlanConsumed: "This installation preview is no longer available.",
    }[code ?? ""] ?? "SkillYard could not complete the operation.";
  }
  return "无法读取 SkillYard 状态";
}

function containsCjk(value: string): boolean {
  return /[\u3400-\u9fff]/u.test(value);
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
