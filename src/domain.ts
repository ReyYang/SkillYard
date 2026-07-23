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
  // 同一种 project rootKey 可属于多个 Project，界面身份必须使用 rootId。
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

export type MountScope = "global" | "project";
export type MountHealth = "healthy" | "missing" | "conflict";

export interface ProjectSummary {
  id: string;
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
  inputKind: "localFolder" | "github";
  mode: "create" | "supplement";
  inputPath: string;
  bundleDisplayName: string;
  candidates: InstallCandidate[];
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

export type SourceCatalogStatus = "unloaded" | "fresh" | "stale";

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
  canonicalIdentity: string;
  displayName: string;
  repositoryUrl: string;
  trackedRef: string;
  memberPathHint: string | null;
  catalogStatus: SourceCatalogStatus;
  catalogCommitSha: string | null;
  catalogFetchedAt: number | null;
  lastReloadAt: number | null;
  lastReloadError: string | null;
  bundleId: string | null;
  adoptedCommitSha: string | null;
  members: SourceCatalogMemberSummary[];
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
  observationIds: string[];
  selectedObservationId: string;
  preservedObservationIds: string[];
  sharedTargets: TakeoverSharedTargetRequest[];
}

export interface TakeoverSharedTargetRequest {
  sharedObservationId: string;
  appId: SupportedAppId;
}

export type TakeoverIdentityBasis = "singleOrigin" | "userConfirmed";
export type TakeoverOriginDisposition = "mount" | "remove";

export interface TakeoverPlanOrigin {
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
  identityBasis: TakeoverIdentityBasis;
  selectedObservationId: string;
  bundleId: string;
  memberId: string;
  contentId: string;
  bundleDisplayName: string;
  skillName: string;
  skillDescription: string;
  sourceDisplayName: string | null;
  managedDirectory: string;
  contentDirectory: string;
  expectedTarget: string;
  origins: TakeoverPlanOrigin[];
  targets: TakeoverPlanTarget[];
  warnings: string[];
  createdAt: number;
  expiresAt: number;
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
      projects: ProjectSummary[];
      mounts: MountSummary[];
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
      type: "skillsShSearch";
      query: string;
      sources: SkillsShSearchSource[];
    }
  | {
      type: "installPlan";
      plan: InstallPlan;
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
