export type SupportedAppId = "codex" | "claudeCode" | "gitHubCopilot";

export interface SupportedAppSummary {
  id: SupportedAppId;
  displayName: string;
  detected: boolean | null;
}

export interface InventoryObservation {
  id: string;
  skillName: string;
  declaredName: string | null;
  skillRoot: string;
  skillFile: string;
  locationKind: "appGlobal" | "sharedReadOnly" | "managedStore";
  metadataStatus: "valid" | "invalid" | "unreadable";
  observedBy: SupportedAppId[];
  observedFingerprint: string;
  rootKey:
    | "codexGlobal"
    | "claudeCodeGlobal"
    | "gitHubCopilotGlobal"
    | "sharedAgents"
    | null;
  stale: boolean;
  managementKind:
    | "skillYardManaged"
    | "takeoverCandidate"
    | "agentManaged"
    | "projectManaged";
  // 受管安装由 Rust 投影真实 Bundle；扫描观察保持这些关系为空。
  bundleId?: string | null;
  bundleDisplayName?: string | null;
  sourceDisplayName?: string | null;
  projectDisplayName?: string | null;
}

export interface LocalRefreshSummary {
  completedAt: number;
  added: number;
  changed: number;
  removed: number;
}

export interface ScanIssue {
  rootKey: InventoryObservation["rootKey"];
  path: string;
  code:
    | "inspectPath"
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
    }
  | {
      type: "folderInstallPlan";
      plan: FolderInstallPlan;
    };
