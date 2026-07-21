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
  locationKind: "appGlobal" | "sharedReadOnly";
  metadataStatus: "valid" | "invalid" | "unreadable";
  observedBy: SupportedAppId[];
  observedFingerprint: string;
  rootKey:
    | "codexGlobal"
    | "claudeCodeGlobal"
    | "gitHubCopilotGlobal"
    | "sharedAgents";
  stale: boolean;
  managementKind:
    | "skillYardManaged"
    | "takeoverCandidate"
    | "agentManaged"
    | "projectManaged";
  // 后续领域 Issue 接入真实值；当前本机扫描不会伪造 Bundle、Source 或 Project。
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
    };
