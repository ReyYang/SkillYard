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

export interface FolderInstallPlan {
  id: string;
  inputPath: string;
  bundleDisplayName: string;
  candidates: FolderInstallCandidate[];
  warnings: string[];
  willMount: boolean;
  createdAt: number;
  expiresAt: number;
}

export interface FolderInstallCandidate {
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
      type: "folderInstallPlan";
      plan: FolderInstallPlan;
    }
  | {
      type: "mountPlan";
      plan: MountPlan;
    }
  | {
      type: "batchMountPlan";
      plan: BatchMountPlan;
    };
