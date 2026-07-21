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
    };
