import type {
  InventoryObservation,
  MountSummary,
  ThemePreset,
  UiOutcome,
} from "./domain";

type InventoryOutcome = Extract<UiOutcome, { type: "inventory" }>;

const params = new URLSearchParams(window.location.search);
let currentTheme: ThemePreset =
  params.get("theme") === "layers" ? "layers" : "ledger";

const aiPreferences = {
  enabled: false,
  disclosureAccepted: false,
  provider: "openAi" as const,
  model: "gpt-5.6-terra",
  hasApiKey: false,
  verified: false,
};

const mattNames = [
  "qa",
  "tdd",
  "grill-me",
  "research",
  "refactor",
  ...Array.from({ length: 36 }, (_, index) =>
    `zz-matt-${String(index + 6).padStart(3, "0")}`,
  ),
];

const descriptions = new Map([
  ["qa", "测试与质量保障工作流"],
  ["tdd", "测试驱动开发实践"],
  ["grill-me", "代码审查与改进"],
  ["research", "研究与探索方法论"],
  ["refactor", "重构与优化实践"],
]);

function baseEntry(
  id: string,
  skillName: string,
  overrides: Partial<InventoryObservation>,
): InventoryObservation {
  return {
    id,
    skillName,
    description: null,
    declaredName: skillName,
    skillRoot: `/fixtures/${skillName}`,
    skillFile: `/fixtures/${skillName}/SKILL.md`,
    locationKind: "appGlobal",
    metadataStatus: "valid",
    observedBy: ["codex"],
    observedFingerprint: `fingerprint-${id}`,
    rootKey: "codexGlobal",
    projectId: null,
    stale: false,
    managementKind: "takeoverCandidate",
    installationChain: null,
    takeoverGroupId: null,
    takeoverGroupDisplayName: null,
    aiExplanation: null,
    ...overrides,
  };
}

function managedEntries(
  bundleId: string,
  bundleDisplayName: string,
  names: string[],
): InventoryObservation[] {
  return names.map((skillName, index) => {
    const memberId = `${bundleId}-member-${index + 1}`;
    return baseEntry(`managed:${memberId}`, skillName, {
      description:
        descriptions.get(skillName) ?? `${skillName} 的合成验收说明`,
      locationKind: "managedStore",
      managementKind: "skillYardManaged",
      observedBy: [],
      rootKey: null,
      memberId,
      bundleId,
      bundleDisplayName,
      sourceDisplayName: bundleDisplayName,
    });
  });
}

function takeoverEntries(): InventoryObservation[] {
  return Array.from({ length: 38 }, (_, index) => {
    const skillName = `lark-${String(index + 1).padStart(3, "0")}`;
    return baseEntry(`takeover:larkcli:${index + 1}`, skillName, {
      installationChain: {
        kind: "lockV3",
        recordPath: "/fixtures/.agents/.skill-lock.json",
        source: "larkcli",
        sourceType: "github",
        sourceLocator: "https://github.com/larksuite/cli",
        skillPath: `skills/${skillName}/SKILL.md`,
        trackedRef: "main",
        contentMarker: `marker-${index + 1}`,
        installedAt: "2026-08-01T00:00:00Z",
        updatedAt: "2026-08-01T00:00:00Z",
      },
      takeoverGroupId: "larkcli",
      takeoverGroupDisplayName: "larkcli",
    });
  });
}

function officialPluginEntries(): InventoryObservation[] {
  return Array.from({ length: 27 }, (_, index) => {
    const skillName = `official-${String(index + 1).padStart(3, "0")}`;
    return baseEntry(`agent:official:${index + 1}`, skillName, {
      locationKind: "sharedReadOnly",
      managementKind: "agentManaged",
      rootKey: "codexOfficialPlugins",
      externalGroupDisplayName: "Codex 官方插件",
    });
  });
}

const entries = [
  ...managedEntries("bundle-matt", "mattpocock/skills", mattNames),
  ...managedEntries(
    "bundle-anthropic",
    "anthropics/skills",
    Array.from({ length: 85 }, (_, index) =>
      `anthropic-${String(index + 1).padStart(3, "0")}`,
    ),
  ),
  ...managedEntries(
    "bundle-vercel",
    "vercel/skills",
    Array.from({ length: 9 }, (_, index) =>
      `vercel-${String(index + 1).padStart(3, "0")}`,
    ),
  ),
  ...takeoverEntries(),
  ...officialPluginEntries(),
];

function mountFor(
  entry: InventoryObservation,
  appId: MountSummary["appId"],
  index: number,
): MountSummary {
  return {
    id: `${appId}-${entry.memberId}-${index}`,
    memberId: entry.memberId!,
    skillName: entry.skillName,
    appId,
    scope: "global",
    projectId: null,
    projectDisplayName: null,
    targetPath: `/fixtures/.${appId === "codex" ? "codex" : "claude"}/skills/${entry.skillName}`,
    expectedTarget: `/fixtures/central/${entry.bundleId}/${entry.skillName}`,
    health: "healthy",
  };
}

const mattEntries = entries.filter(
  (entry) => entry.bundleId === "bundle-matt",
);
const vercelEntries = entries.filter(
  (entry) => entry.bundleId === "bundle-vercel",
);
// 模拟 storage 先返回同一 App 的连续路径，验证摘要仍优先展示不同目标应用。
const mounts: MountSummary[] = [
  ...mattEntries.map((entry, index) => mountFor(entry, "claudeCode", index)),
  ...mattEntries.map((entry, index) => mountFor(entry, "codex", index)),
  ...vercelEntries.map((entry, index) => mountFor(entry, "codex", index)),
];

export const visualFixtureInventory: InventoryOutcome = {
  type: "inventory",
  scanCompletedAt: 1_786_556_800_000,
  entries,
  supportedApps: [
    { id: "codex", displayName: "Codex", detected: true },
    { id: "claudeCode", displayName: "Claude Code", detected: true },
    { id: "gitHubCopilot", displayName: "GitHub Copilot", detected: false },
  ],
  lastLocalRefresh: {
    completedAt: 1_786_556_800_000,
    added: 0,
    changed: 0,
    removed: 0,
  },
  scanIssues: [],
  recoveryIssues: [],
  recoveredInterruptedOperation: false,
  projects: [],
  mounts,
  bundleUpdates: [
    {
      bundleId: "bundle-matt",
      status: "available",
      action: "update",
      checkedAt: 1_786_556_800_000,
      message: "上游已有新内容",
      upstreamUrl: "https://github.com/mattpocock/skills",
    },
    {
      bundleId: "bundle-anthropic",
      status: "available",
      action: "update",
      checkedAt: 1_786_556_800_000,
      message: "上游已有新内容",
      upstreamUrl: "https://github.com/anthropics/skills",
    },
    {
      bundleId: "bundle-vercel",
      status: "available",
      action: "update",
      checkedAt: 1_786_556_800_000,
      message: "上游已有新内容",
      upstreamUrl: "https://github.com/vercel/skills",
    },
  ],
};

export function assertVisualFixture(outcome: InventoryOutcome): void {
  const expectedGroups = new Map([
    ["mattpocock/skills", 41],
    ["anthropics/skills", 85],
    ["larkcli", 38],
    ["vercel/skills", 9],
    ["Codex 官方插件", 27],
  ]);
  const actualGroups = new Map<string, number>();
  for (const entry of outcome.entries) {
    const groupName =
      entry.bundleDisplayName ??
      entry.takeoverGroupDisplayName ??
      entry.externalGroupDisplayName;
    if (!groupName) {
      throw new Error(`视觉夹具分组缺失：${entry.id}`);
    }
    actualGroups.set(groupName, (actualGroups.get(groupName) ?? 0) + 1);
  }
  if (
    actualGroups.size !== expectedGroups.size ||
    [...expectedGroups].some(
      ([name, count]) => actualGroups.get(name) !== count,
    )
  ) {
    throw new Error("ticket8-five-bundles-v1 分组清单发生漂移");
  }

  assertUniqueIds(
    outcome.entries.map((entry) => entry.id),
    "Inventory",
  );
  const memberIds = outcome.entries.flatMap((entry) =>
    entry.memberId ? [entry.memberId] : [],
  );
  assertUniqueIds(memberIds, "Member");
  assertUniqueIds(
    outcome.mounts.map((mount) => mount.id),
    "Mount",
  );

  const memberIdSet = new Set(memberIds);
  const mountCounts = new Map<MountSummary["appId"], number>();
  for (const mount of outcome.mounts) {
    if (mount.health !== "healthy" || !memberIdSet.has(mount.memberId)) {
      throw new Error("视觉夹具 Mount 健康或成员关联发生漂移");
    }
    mountCounts.set(mount.appId, (mountCounts.get(mount.appId) ?? 0) + 1);
  }
  if (
    outcome.mounts.length !== 91 ||
    mountCounts.get("claudeCode") !== 41 ||
    mountCounts.get("codex") !== 50 ||
    (mountCounts.get("gitHubCopilot") ?? 0) !== 0
  ) {
    throw new Error("视觉夹具 Mount 应用分布发生漂移");
  }

  const updateBundleIds = outcome.bundleUpdates.map(
    (update) => update.bundleId,
  );
  assertUniqueIds(updateBundleIds, "更新 Bundle");
  const expectedUpdateBundleIds = new Set([
    "bundle-matt",
    "bundle-anthropic",
    "bundle-vercel",
  ]);
  if (
    updateBundleIds.length !== expectedUpdateBundleIds.size ||
    updateBundleIds.some((id) => !expectedUpdateBundleIds.has(id)) ||
    outcome.bundleUpdates.some(
      (update) => update.status !== "available" || update.action !== "update",
    )
  ) {
    throw new Error("视觉夹具更新清单发生漂移");
  }

  if (
    outcome.entries.length !== 200 ||
    outcome.entries.some(
      (entry) =>
        entry.managementKind === "skillYardManaged" && !entry.description,
    ) ||
    outcome.entries.some(
      (entry) =>
        entry.managementKind !== "skillYardManaged" &&
        entry.description != null,
    )
  ) {
    throw new Error("视觉夹具 Inventory 描述投影发生漂移");
  }
}

function assertUniqueIds(ids: string[], label: string): void {
  if (new Set(ids).size !== ids.length) {
    throw new Error(`视觉夹具 ${label} ID 发生重复`);
  }
}

assertVisualFixture(visualFixtureInventory);
const fixtureGroupNames = new Set(
  visualFixtureInventory.entries.map(
    (entry) =>
      entry.bundleDisplayName ??
      entry.takeoverGroupDisplayName ??
      entry.externalGroupDisplayName,
  ),
);
document.documentElement.dataset.visualFixture = "ticket8-five-bundles-v1";
document.documentElement.dataset.visualFixtureGroups = String(
  fixtureGroupNames.size,
);
document.documentElement.dataset.visualFixtureEntries = String(entries.length);
document.documentElement.dataset.visualFixtureMounts = String(mounts.length);

interface VisualTauriInternals {
  invoke(command: string, args?: Record<string, unknown>): Promise<unknown>;
}

const internals: VisualTauriInternals = {
  async invoke(command, args = {}) {
    if (command === "get_preferences") {
      return {
        type: "preferences",
        language: "zhCn",
        theme: currentTheme,
        ai: aiPreferences,
      };
    }
    if (command === "get_startup_state") return visualFixtureInventory;
    if (command === "set_theme_preset") {
      currentTheme = args.theme as ThemePreset;
      return {
        type: "preferences",
        language: "zhCn",
        theme: currentTheme,
        ai: aiPreferences,
      };
    }
    if (command === "set_interface_language") {
      return {
        type: "preferences",
        language: args.language,
        theme: currentTheme,
        ai: aiPreferences,
      };
    }
    if (command === "cancel_agent" || command === "open_external_url") {
      return undefined;
    }
    throw new Error(`Visual fixture 不支持命令：${command}`);
  },
};

(window as unknown as { __TAURI_INTERNALS__: VisualTauriInternals }).__TAURI_INTERNALS__ =
  internals;

await import("./main");
