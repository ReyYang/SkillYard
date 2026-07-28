export type SupportedAppId = "codex" | "claudeCode" | "gitHubCopilot";

export interface SupportedAppSummary {
  id: SupportedAppId;
  displayName: string;
  detected: boolean | null;
}

export type InventoryLocationKind =
  | "appGlobal"
  | "appProject"
  | "sharedReadOnly"
  | "managedStore";

export type ScanRootKey =
  | "codexGlobal"
  | "codexOfficialPlugins"
  | "claudeCodeGlobal"
  | "gitHubCopilotGlobal"
  | "sharedAgents"
  | "codexProject"
  | "claudeCodeProject"
  | "gitHubCopilotProject"
  | "sharedAgentsProject";

export interface ManagementEvidence {
  kind: "gitHeadTracked";
  authorityRoot: string;
  snapshotCommitOid: string;
  subjectPath: string;
}

export interface InstallationChain {
  kind: "lockV3";
  recordPath: string;
  source: string;
  sourceType: string;
  sourceLocator: string;
  skillPath: string | null;
  trackedRef: string | null;
  contentMarker: string;
  installedAt: string;
  updatedAt: string;
}

export interface InventoryObservation {
  id: string;
  skillName: string;
  declaredName: string | null;
  skillRoot: string;
  skillFile: string;
  locationKind: InventoryLocationKind;
  metadataStatus: "valid" | "invalid" | "unreadable";
  observedBy: SupportedAppId[];
  observedFingerprint: string;
  rootKey: ScanRootKey | null;
  // Project 扫描观察携带稳定 ID；global、共享根和受管条目保持为空。
  projectId: string | null;
  stale: boolean;
  managementKind:
    | "skillYardManaged"
    | "takeoverCandidate"
    | "agentManaged"
    | "projectManaged";
  // 当前仅由已登记 Project 的 Git HEAD 确定性证据产生。
  managementEvidence?: ManagementEvidence | null;
  // lock v3 只证明这份安装记录，不证明具体由哪个外部 CLI 执行。
  installationChain: InstallationChain | null;
  // 后端只有在存在确定性来源证据时才提供分组；null 仍表示独立待接管候选。
  takeoverGroupId: string | null;
  takeoverGroupDisplayName: string | null;
  // 只读 Skill 可按原管理方的真实容器分组，例如 Codex 官方插件名。
  externalGroupDisplayName?: string | null;
  // 受管安装由 Rust 投影真实 Bundle；扫描观察保持这些关系为空。
  bundleId?: string | null;
  bundleDisplayName?: string | null;
  sourceDisplayName?: string | null;
  projectDisplayName?: string | null;
  memberId?: string | null;
}

export interface LocalRefreshSummary {
  completedAt: number;
  added: number;
  changed: number;
  removed: number;
}

export interface ScanIssue {
  // 同一个扫描根可以包含多个问题，界面身份必须使用独立 issue id。
  id: string;
  rootId: string;
  rootKey: ScanRootKey;
  projectId: string | null;
  path: string;
  code:
    | "inspectPath"
    | "inspectManagementEvidence"
    | "rootNotDirectory"
    | "readRoot"
    | "readSkillContent";
  message: string;
}

export interface RecoveryIssue {
  id: string;
  bundleDisplayName: string;
  message: string;
}

export type BundleUpdateStatus =
  | "noSource"
  | "notChecked"
  | "available"
  | "upToDate"
  | "unableToCheck"
  | "manual"
  | "sourceUnavailable";

export type BundleUpdateAction =
  | "update"
  | "importReplacement"
  | "checkEditableLocal"
  | null;

export interface BundleUpdateSummary {
  bundleId: string;
  status: BundleUpdateStatus;
  action: BundleUpdateAction;
  checkedAt: number | null;
  message: string;
  upstreamUrl: string | null;
}

export type MountScope = "global" | "project";
export type MountHealth = "healthy" | "missing" | "conflict";

export interface ProjectSummary {
  id: string;
  displayName: string;
  rootPath: string;
}

export interface ProjectSelection {
  displayName: string;
  rootPath: string;
}

export interface MountSummary {
  id: string;
  memberId: string;
  skillName: string;
  appId: SupportedAppId;
  scope: MountScope;
  projectId: string | null;
  projectDisplayName: string | null;
  targetPath: string;
  expectedTarget: string;
  health: MountHealth;
}

export interface MountPlan {
  id: string;
  operation: "create" | "remove";
  purpose: "create" | "repair" | "remove";
  mountId: string;
  memberId: string;
  skillName: string;
  appId: SupportedAppId;
  scope: MountScope;
  projectId: string | null;
  projectDisplayName: string | null;
  targetPath: string;
  expectedTarget: string;
  targetHealth: MountHealth;
  createdAt: number;
  expiresAt: number;
}

export interface BatchMountRequest {
  memberId: string;
  appId: SupportedAppId;
  scope: MountScope;
  projectId: string | null;
}

export type BatchMountDisposition =
  | "ready"
  | "pathConflict"
  | "scopeConflict"
  | "alreadyMounted";

export interface BatchMountPlanItem {
  id: string;
  memberId: string;
  skillName: string;
  appId: SupportedAppId;
  scope: MountScope;
  projectId: string | null;
  projectDisplayName: string | null;
  targetPath: string;
  expectedTarget: string;
  disposition: BatchMountDisposition;
  selectable: boolean;
  defaultSelected: boolean;
  conflictReason: string | null;
  targetHealth: MountHealth;
}

export interface BatchMountPlan {
  id: string;
  bundleId: string;
  bundleDisplayName: string;
  items: BatchMountPlanItem[];
  createdAt: number;
  expiresAt: number;
}

export interface InstallPlan {
  id: string;
  inputKind:
    | "localFolder"
    | "github"
    | "archive"
    | "directUrl"
    | "editableLocal";
  mode: "create" | "supplement" | "update";
  inputPath: string;
  bundleDisplayName: string;
  candidates: InstallCandidate[];
  // Update 继续复用安装 Plan；这里仅补充整组更新的只读影响信息。
  updateImpact: {
    newCandidateIds: string[];
    existingMounts: MountSummary[];
    upstreamUrl: string | null;
  } | null;
  warnings: string[];
  willMount: boolean;
  createdAt: number;
  expiresAt: number;
}

export interface InstallCandidate {
  candidateId: string;
  sourceRelativePath: string;
  skillName: string | null;
  description: string | null;
  targetDirectory: string | null;
  selectable: boolean;
  validationErrors: string[];
  warnings: string[];
  defaultSelected: boolean;
}

export type BundleUpdateBatchDisposition = "ready" | "preparationFailed";

export interface BundleUpdateBatchPlanItem {
  id: string;
  bundleId: string;
  bundleDisplayName: string;
  disposition: BundleUpdateBatchDisposition;
  // Ready 项复用单 Bundle 的唯一更新 Plan；准备失败时保持为空。
  installPlan: InstallPlan | null;
  errorSummary: string | null;
}

export interface BundleUpdateBatchPlan {
  id: string;
  items: BundleUpdateBatchPlanItem[];
  createdAt: number;
  expiresAt: number;
}

export type BundleUpdateBatchItemStatus =
  | "succeeded"
  | "failed"
  | "blocked"
  | "notExecuted";

export interface BundleUpdateBatchResultItem {
  id: string;
  bundleId: string;
  bundleDisplayName: string;
  status: BundleUpdateBatchItemStatus;
  errorSummary: string | null;
}

export interface BundleUpdateBatchResult {
  id: string;
  status: "completed" | "blocked";
  items: BundleUpdateBatchResultItem[];
  confirmedAt: number;
  updatedAt: number;
}

export type SourceCatalogStatus = "unloaded" | "fresh" | "stale";
export type SourceKind =
  | "github"
  | "archive"
  | "directUrl"
  | "editableLocal";

export interface SourceCatalogMemberSummary {
  id: string;
  relativePath: string;
  skillName: string | null;
  description: string | null;
  selectable: boolean;
  validationErrors: string[];
  warnings: string[];
  // 只有已经安装的 Catalog Member 才关联稳定 Member ID。
  installedMemberId: string | null;
}

export interface SourceSummary {
  id: string;
  kind: SourceKind;
  canonicalIdentity: string;
  displayName: string;
  locator: string;
  trackedRef: string | null;
  memberPathHint: string | null;
  catalogStatus: SourceCatalogStatus;
  // marker 仅说明本地采用的来源基线，不是用户可见版本。
  catalogMarker: string | null;
  catalogFetchedAt: number | null;
  lastReloadAt: number | null;
  lastReloadError: string | null;
  bundleId: string | null;
  adoptedMarker: string | null;
  members: SourceCatalogMemberSummary[];
}

export type RemovalKind =
  | "project"
  | "source"
  | "bundle"
  | "bundleMounts";

export interface RemovalMemberSummary {
  id: string;
  skillName: string;
}

export interface RemovalBundleSummary {
  id: string;
  displayName: string;
}

export interface RemovalPreservedSource {
  id: string;
  displayName: string;
  kind: SourceKind;
  locator: string;
}

// 四类移除共用一份只读影响模型，前端不能按入口重新推导删除范围。
export interface RemovalPlan {
  id: string;
  kind: RemovalKind;
  targetId: string;
  targetDisplayName: string;
  members: RemovalMemberSummary[];
  mounts: MountSummary[];
  affectedBundles: RemovalBundleSummary[];
  preservedSource: RemovalPreservedSource | null;
  managedDirectory: string | null;
  preservedExternalPaths: string[];
  warnings: string[];
  createdAt: number;
  expiresAt: number;
}

export type SourceAssociationMode = "link" | "merge";

// null 是用户明确选择“不对应”，不能被前端改写为名称或路径猜测。
export interface SourceMemberMappingChoice {
  memberId: string;
  sourceRelativePath: string | null;
}

export interface SourceAssociationMemberChoice
  extends SourceMemberMappingChoice {
  skillName: string;
}

export interface SourceAssociationContentChoice {
  conflictId: string;
  memberId: string;
}

export interface SourceAssociationPlanMember {
  memberId: string;
  bundleId: string;
  bundleDisplayName: string;
  skillName: string;
  contentFingerprint: string;
}

export interface SourceAssociationConflict {
  id: string;
  label: string;
  candidateMemberIds: string[];
}

// link 与 merge 共用同一份确认模型，避免前端产生第二套归并状态机。
export interface SourceAssociationPlan {
  id: string;
  mode: SourceAssociationMode;
  sourceId: string;
  sourceDisplayName: string;
  targetBundleId: string;
  targetBundleDisplayName: string;
  retiringBundleId: string | null;
  retiringBundleDisplayName: string | null;
  memberChoices: SourceAssociationMemberChoice[];
  members: SourceAssociationPlanMember[];
  mounts: MountSummary[];
  conflicts: SourceAssociationConflict[];
  blockingIssues: string[];
  createdAt: number;
  expiresAt: number;
}

// Tracked Ref 变更 Plan 只冻结确认信息，不代表安装或文件系统事务。
export interface SourceRefChangePlan {
  id: string;
  sourceId: string;
  sourceDisplayName: string;
  currentRef: string;
  candidateRef: string;
  candidateCommitSha: string;
  memberPathHint: string | null;
  createdAt: number;
  expiresAt: number;
}

export interface EditableLocalRelinkMember {
  relativePath: string;
  skillName: string | null;
  description: string | null;
  selectable: boolean;
  validationErrors: string[];
  warnings: string[];
}

// 重新关联只改变 Source 路径，候选内容仍需后续单独检查和更新。
export interface EditableLocalRelinkPlan {
  id: string;
  sourceId: string;
  sourceDisplayName: string;
  currentPath: string;
  candidatePath: string;
  candidateDisplayName: string;
  bundleDisplayName: string | null;
  members: EditableLocalRelinkMember[];
  createdAt: number;
  expiresAt: number;
}

export interface SkillsShSearchMember {
  skillId: string;
  name: string;
  installs: number;
}

export interface SkillsShSearchSource {
  sourceInput: string;
  supported: boolean;
  members: SkillsShSearchMember[];
}

// Takeover 的全部选择在创建 Plan 时冻结；最终确认只再提交 Plan ID。
export interface TakeoverPlanRequest {
  members: TakeoverMemberRequest[];
  sharedTargets: TakeoverSharedTargetRequest[];
}

export interface TakeoverMemberRequest {
  observationIds: string[];
  selectedObservationId: string;
  preservedObservationIds: string[];
}

export interface TakeoverSharedTargetRequest {
  sharedObservationId: string;
  appId: SupportedAppId;
}

export type TakeoverIdentityBasis = "singleOrigin" | "userConfirmed";
export type TakeoverOriginDisposition = "mount" | "remove";

export interface TakeoverPlanOrigin {
  memberId: string;
  observationId: string;
  originalPath: string;
  appId: SupportedAppId | null;
  scope: MountScope | null;
  projectId: string | null;
  projectDisplayName: string | null;
  contentFingerprint: string;
  warnings: string[];
  finalDisposition: TakeoverOriginDisposition;
}

export interface TakeoverPlanTarget {
  memberId: string;
  mountId: string;
  appId: SupportedAppId;
  scope: MountScope;
  projectId: string | null;
  projectDisplayName: string | null;
  targetPath: string;
  expectedTarget: string;
}

export interface TakeoverPlan {
  id: string;
  bundleId: string;
  contentId: string;
  bundleDisplayName: string;
  sourceDisplayName: string | null;
  managedDirectory: string;
  contentDirectory: string;
  retainedMembers: TakeoverPlanRetainedMember[];
  members: TakeoverPlanMember[];
  origins: TakeoverPlanOrigin[];
  targets: TakeoverPlanTarget[];
  warnings: string[];
  createdAt: number;
  expiresAt: number;
}

export interface TakeoverPlanRetainedMember {
  memberId: string;
  skillName: string;
  skillDescription: string;
  installationChain: InstallationChain | null;
  expectedTarget: string;
  mounts: MountSummary[];
}

export interface TakeoverPlanMember {
  memberId: string;
  identityBasis: TakeoverIdentityBasis;
  selectedObservationId: string;
  skillName: string;
  skillDescription: string;
  installationChain: InstallationChain | null;
  expectedTarget: string;
  warnings: string[];
}

export type UiOutcome =
  | {
      type: "unsupportedPlatform";
      actualOs: string;
      actualArchitecture: string;
      actualMajorVersion: number;
      requiredArchitecture: string;
      minimumMajorVersion: number;
    }
  | {
      type: "onboardingRequired";
      supportedApps: SupportedAppSummary[];
    }
  | {
      type: "inventory";
      scanCompletedAt: number;
      entries: InventoryObservation[];
      supportedApps: SupportedAppSummary[];
      lastLocalRefresh: LocalRefreshSummary | null;
      scanIssues: ScanIssue[];
      recoveryIssues: RecoveryIssue[];
      // 只表示本次启动已自动完成普通恢复，不持久化为历史通知。
      recoveredInterruptedOperation: boolean;
      projects: ProjectSummary[];
      mounts: MountSummary[];
      // 更新状态属于 Bundle read model，前端不能根据 Source marker 自行推断。
      bundleUpdates: BundleUpdateSummary[];
    }
  | {
      type: "sourceDiscovery";
      sources: SourceSummary[];
      highlightedSourceId: string | null;
      highlightedMemberPath: string | null;
    }
  | {
      type: "sourceRefChangePlan";
      plan: SourceRefChangePlan;
    }
  | {
      type: "editableLocalRelinkPlan";
      plan: EditableLocalRelinkPlan;
    }
  | {
      type: "sourceAssociationPlan";
      plan: SourceAssociationPlan;
    }
  | {
      type: "sourceAssociationPlanDiscarded";
    }
  | {
      type: "skillsShSearch";
      query: string;
      sources: SkillsShSearchSource[];
    }
  | {
      type: "installPlan";
      plan: InstallPlan;
    }
  | {
      type: "bundleUpdateBatchPlan";
      plan: BundleUpdateBatchPlan;
    }
  | {
      type: "bundleUpdateBatchResult";
      result: BundleUpdateBatchResult;
    }
  | {
      type: "removalPlan";
      plan: RemovalPlan;
    }
  | {
      type: "installPlanDiscarded";
    }
  | {
      type: "mountPlan";
      plan: MountPlan;
    }
  | {
      type: "batchMountPlan";
      plan: BatchMountPlan;
    }
  | {
      type: "takeoverPlan";
      plan: TakeoverPlan;
    };
