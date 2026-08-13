import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import type {
  AiPreferences,
  InventoryObservation,
  MountSummary,
  ThemePreset,
  UiOutcome,
} from "./domain";
import type { SkillYardClient } from "./skillyardClient";

const themes = [
  ["ledger", "Ledger Bundle Library"],
  ["layers", "Layers Bundle Library"],
] as const;

const maintenanceOperations = [
  ["AI 整理", "organizeSkillAiExplanations"],
  ["检查更新", "checkBundleUpdates"],
  ["全部更新", "createBundleUpdateBatchPlan"],
  ["刷新本机", "refreshLocalInventory"],
] as const;

describe("主题功能等价", () => {
  it.each(themes)("%s 主题选择把键盘焦点画在可见卡片上", async (
    theme,
    libraryName,
  ) => {
    const user = userEvent.setup();
    render(<App client={createParityClient(theme)} />);

    await screen.findByRole("region", { name: libraryName });
    await user.click(screen.getByRole("button", { name: "设置" }));
    const radio = screen.getByRole("radio", {
      name: theme === "layers" ? "Layers" : "Ledger",
    });
    for (let step = 0; step < 24 && document.activeElement !== radio; step += 1) {
      await user.tab();
    }

    const option = radio.closest<HTMLElement>(".theme-preset-option");
    expect(radio).toHaveFocus();
    expect(option).not.toBeNull();
    expect(option?.matches(":has(input:focus-visible)")).toBe(true);
  });

  it.each(themes)("%s 通过共享文案展示 Bundle 的真实 Skill 数量", async (
    theme,
    libraryName,
  ) => {
    const inventory = parityInventory();
    inventory.entries = [
      createManagedEntry({
        id: "managed:alpha-one",
        memberId: "member-alpha-one",
        bundleId: "bundle-alpha",
        bundleDisplayName: "Alpha",
        skillName: "qa",
      }),
      createManagedEntry({
        id: "managed:alpha-two",
        memberId: "member-alpha-two",
        bundleId: "bundle-alpha",
        bundleDisplayName: "Alpha",
        skillName: "tdd",
      }),
      createManagedEntry({
        id: "managed:beta",
        memberId: "member-beta",
        bundleId: "bundle-beta",
        bundleDisplayName: "Beta",
        skillName: "research",
      }),
    ];
    inventory.mounts = [];
    inventory.bundleUpdates = [];
    render(<App client={createParityClient(theme, inventory)} />);

    const library = await screen.findByRole("region", { name: libraryName });
    const selectorName = theme === "layers" ? "Beta" : "Alpha";
    const expectedCount = theme === "layers" ? 1 : 2;
    const bundleSelector = within(library).getByRole("button", {
      name: selectorName,
    });
    expect(bundleSelector).toHaveAccessibleDescription(
      new RegExp(`${expectedCount} 个 Skill`),
    );
    expect(bundleSelector).not.toHaveAccessibleDescription(
      new RegExp(`${expectedCount} Skill(?:\\s|$)`),
    );
  });

  it.each(themes)(
    "%s 通过共享排序入口按名称浏览并保留当前 Bundle",
    async (theme, libraryName) => {
      const user = userEvent.setup();
      const inventory = parityInventory();
      inventory.entries = [
        createManagedEntry({
          id: "managed:zulu",
          memberId: "member-zulu",
          bundleId: "bundle-zulu",
          bundleDisplayName: "Zulu Managed",
          skillName: "zulu-skill",
        }),
        createManagedEntry({
          id: "project:alpha",
          memberId: null,
          bundleId: null,
          bundleDisplayName: null,
          skillName: "alpha-skill",
          locationKind: "appProject",
          rootKey: "codexProject",
          projectId: "project-alpha",
          managementKind: "projectManaged",
          projectDisplayName: "Alpha Project",
        }),
      ];
      inventory.mounts = [];
      inventory.bundleUpdates = [];
      render(<App client={createParityClient(theme, inventory)} />);

      const library = await screen.findByRole("region", { name: libraryName });
      expect(
        within(library).getByRole("region", { name: "Zulu Managed" }),
      ).toBeVisible();

      const controls = screen.getByLabelText(
        "筛选与排序：全部 Bundle · 管理状态优先",
        { selector: "summary" },
      );
      await user.click(controls);
      const sort = within(
        controls.closest("details") as HTMLElement,
      ).getByRole("combobox", { name: "排序" });
      expect(sort).toHaveValue("management");

      await user.selectOptions(sort, "nameAsc");
      expect(sort).toHaveValue("nameAsc");
      if (theme === "layers") {
        expect(
          controls.querySelector(".library-filter-compact-badge"),
        ).toHaveTextContent("A–Z");
      }
      expect(
        within(library).getByRole("region", { name: "Zulu Managed" }),
      ).toBeVisible();

      const keyboardTarget =
        theme === "ledger"
          ? within(library).getByRole("button", { name: "Zulu Managed" })
          : library.querySelector<HTMLElement>(".layers-library-sheet");
      expect(keyboardTarget).not.toBeNull();
      keyboardTarget?.focus();
      await user.keyboard("{Home}");

      expect(
        within(library).getByRole("region", { name: "Alpha Project" }),
      ).toBeVisible();
    },
  );

  it("空筛选结果中切换排序不会丢失此前选中的 Bundle", async () => {
    const user = userEvent.setup();
    render(<App client={createParityClient("ledger")} />);

    const library = await screen.findByRole("region", {
      name: "Ledger Bundle Library",
    });
    await user.click(within(library).getByRole("button", { name: "Beta" }));
    await user.type(
      screen.getByRole("searchbox", { name: "搜索 Bundle 或 Skill" }),
      "no-match",
    );
    expect(screen.getByRole("heading", { name: "没有匹配结果" })).toBeVisible();

    const controls = screen.getByLabelText(
      "筛选与排序：全部 Bundle · 管理状态优先",
      { selector: "summary" },
    );
    await user.click(controls);
    await user.selectOptions(
      within(controls.closest("details") as HTMLElement).getByRole(
        "combobox",
        { name: "排序" },
      ),
      "nameAsc",
    );
    await user.click(screen.getByRole("button", { name: "清除筛选" }));

    const restoredLibrary = screen.getByRole("region", {
      name: "Ledger Bundle Library",
    });
    expect(
      within(restoredLibrary).getByRole("region", { name: "Beta" }),
    ).toBeVisible();
  });

  it.each(themes)(
    "%s 可以从技能库打开同一组维护操作",
    async (theme, libraryName) => {
      const user = userEvent.setup();
      render(<App client={createParityClient(theme)} />);

      await screen.findByRole("region", { name: libraryName });
      const actionMenu = screen.getByLabelText("更多操作", {
        selector: "summary",
      });

      await user.click(actionMenu);
      const menu = actionMenu.closest("details");
      expect(menu).toHaveAttribute("open");
      expect(
        within(menu as HTMLElement).getByRole("button", { name: "AI 整理" }),
      ).toBeVisible();
      expect(
        within(menu as HTMLElement).getByRole("button", { name: "检查更新" }),
      ).toBeVisible();
      expect(
        within(menu as HTMLElement).getByRole("button", { name: "全部更新" }),
      ).toBeVisible();
      expect(
        within(menu as HTMLElement).getByRole("button", { name: "刷新本机" }),
      ).toBeVisible();
    },
  );

  describe.each(themes)("%s 维护入口", (theme, libraryName) => {
    it.each(maintenanceOperations)(
      "%s 复用正式 client 边界",
      async (actionName, clientMethod) => {
        const user = userEvent.setup();
        const client = createParityClient(theme);
        render(<App client={client} />);

        await screen.findByRole("region", { name: libraryName });
        const actionMenu = screen.getByLabelText("更多操作", {
          selector: "summary",
        });
        await user.click(actionMenu);
        await user.click(
          within(actionMenu.closest("details") as HTMLElement).getByRole(
            "button",
            { name: actionName },
          ),
        );

        expect(client[clientMethod]).toHaveBeenCalledTimes(1);
      },
    );
  });

  it.each(themes)(
    "%s 同时呈现更新检查与 Mount 健康真值",
    async (theme, libraryName) => {
      const user = userEvent.setup();
      render(<App client={createParityClient(theme, statusInventory())} />);

      const library = await screen.findByRole("region", { name: libraryName });
      const beta = within(library).getByRole("button", { name: "Beta" });
      expect(beta).toHaveTextContent("1 个 Skill");
      expect(beta).toHaveTextContent("挂载异常 1 处 · 来源不可用");
      expect(beta).toHaveAccessibleDescription(
        "1 个 Skill 挂载异常 1 处 · 来源不可用",
      );

      await user.click(beta);
      const detail = within(library).getByRole("region", { name: "Beta" });
      expect(detail).toHaveTextContent("挂载异常 1 处 · 来源不可用");
      expect(detail).toHaveTextContent("1 个 Mount · Codex · 挂载异常 1 处");
      expect(detail.querySelector(".bundle-mount-mark")).toHaveAttribute(
        "data-mount-state",
        "connected",
      );
      expect(detail).toHaveTextContent("检查于");
      expect(within(detail).getByRole("button", { name: "重新检查 Beta" }))
        .toBeEnabled();
    },
  );

  it.each(themes)(
    "%s 不把单个 Skill 的 AI 说明冒充成多成员 Bundle 摘要",
    async (theme, libraryName) => {
      const memberSummary = "只描述 alpha-skill 的用途";
      const inventory = parityInventory();
      inventory.entries = [
        createManagedEntry({
          id: "managed:alpha-one",
          memberId: "member-alpha-one",
          bundleId: "bundle-alpha",
          bundleDisplayName: "Alpha",
          skillName: "alpha-skill",
          aiExplanation: {
            category: "developmentEngineering",
            summary: memberSummary,
            useCases: ["只用于 Alpha"],
            instructions: "只处理 Alpha。",
            language: "zhCn",
            contentFingerprint: "fingerprint",
            stale: false,
          },
        }),
        createManagedEntry({
          id: "managed:alpha-two",
          memberId: "member-alpha-two",
          bundleId: "bundle-alpha",
          bundleDisplayName: "Alpha",
          skillName: "beta-skill",
        }),
      ];
      inventory.mounts = [];
      inventory.bundleUpdates = [];
      render(<App client={createParityClient(theme, inventory)} />);

      const library = await screen.findByRole("region", { name: libraryName });
      const detail = within(library).getByRole("region", { name: "Alpha" });
      const overview = detail.querySelector(".bundle-library-summary");
      expect(overview).toHaveTextContent(
        "集中管理 2 个 Skill，并保留各自的来源与挂载状态。",
      );
      expect(overview).not.toHaveTextContent(memberSummary);
      expect(within(detail).getByText(memberSummary)).toBeInTheDocument();
    },
  );

  it("ledger 用可扫描的语义表格呈现 Bundle 成员", async () => {
    const inventory = parityInventory();
    inventory.entries = [
      createManagedEntry({
        id: "managed:alpha-one",
        memberId: "member-alpha-one",
        bundleId: "bundle-alpha",
        bundleDisplayName: "Alpha",
        skillName: "qa",
        aiExplanation: {
          category: "developmentEngineering",
          summary: "测试与质量保障工作流",
          useCases: ["验证 Alpha"],
          instructions: "验证 Alpha。",
          language: "zhCn",
          contentFingerprint: "fingerprint",
          stale: false,
        },
      }),
      createManagedEntry({
        id: "managed:alpha-two",
        memberId: "member-alpha-two",
        bundleId: "bundle-alpha",
        bundleDisplayName: "Alpha",
        skillName: "tdd",
        description: "测试驱动开发实践",
      }),
    ];
    inventory.mounts = [];
    inventory.bundleUpdates = [];
    render(<App client={createParityClient("ledger", inventory)} />);

    const library = await screen.findByRole("region", {
      name: "Ledger Bundle Library",
    });
    const table = within(library).getByRole("table", {
      name: "Alpha 的 Skill",
    });
    expect(within(table).getAllByRole("columnheader")).toHaveLength(3);
    expect(within(table).getByRole("columnheader", { name: "名称" }))
      .toBeVisible();
    expect(within(table).getByRole("columnheader", { name: "类型" }))
      .toBeVisible();
    expect(within(table).getByRole("columnheader", { name: "描述" }))
      .toBeVisible();
    expect(within(table).getByRole("row", { name: /qa Skill 测试与质量保障工作流/ }))
      .toBeVisible();
    expect(within(table).getByRole("row", { name: /tdd Skill 测试驱动开发实践/ }))
      .toBeVisible();
    const detail = table.closest(".inventory-section");
    expect(detail?.querySelector(".bundle-source-mark")?.tagName).toBe("svg");
    expect(detail?.querySelector(".bundle-mount-mark")?.tagName).toBe("svg");
    expect(detail?.querySelector(".bundle-mount-mark")).toHaveAttribute(
      "data-mount-state",
      "empty",
    );
  });

  it("layers 在没有 AI 说明时仍展示 SKILL.md 的成员描述", async () => {
    const inventory = parityInventory();
    inventory.entries = [
      createManagedEntry({
        id: "managed:alpha-one",
        memberId: "member-alpha-one",
        bundleId: "bundle-alpha",
        bundleDisplayName: "Alpha",
        skillName: "qa",
        description: "测试与质量保障工作流",
      }),
      createManagedEntry({
        id: "managed:alpha-two",
        memberId: "member-alpha-two",
        bundleId: "bundle-alpha",
        bundleDisplayName: "Alpha",
        skillName: "tdd",
        description: "测试驱动开发实践",
      }),
    ];
    inventory.mounts = [];
    inventory.bundleUpdates = [];
    render(<App client={createParityClient("layers", inventory)} />);

    const library = await screen.findByRole("region", {
      name: "Layers Bundle Library",
    });
    const detail = within(library).getByRole("region", { name: "Alpha" });
    expect(within(detail).getByText("测试与质量保障工作流")).toBeVisible();
    expect(within(detail).getByText("测试驱动开发实践")).toBeVisible();
  });

  it.each(themes)(
    "%s 单成员在 AI 未整理时仍保留 SKILL.md 描述",
    async (theme, libraryName) => {
      const user = userEvent.setup();
      const inventory = parityInventory();
      inventory.entries = [
        createManagedEntry({
          id: "managed:alpha-one",
          memberId: "member-alpha-one",
          bundleId: "bundle-alpha",
          bundleDisplayName: "Alpha",
          skillName: "alpha-skill",
          description: "来自 SKILL.md 的原始能力说明",
        }),
      ];
      inventory.mounts = [];
      inventory.bundleUpdates = [];
      render(<App client={createParityClient(theme, inventory)} />);

      const library = await screen.findByRole("region", { name: libraryName });
      const detail = within(library).getByRole("region", { name: "Alpha" });
      expect(
        within(detail).getByText("来自 SKILL.md 的原始能力说明"),
      ).toBeVisible();
      expect(within(detail).getByText("未整理")).toBeVisible();

      await user.click(
        within(detail).getByRole("button", {
          name: "查看 Skill alpha-skill",
        }),
      );
      const skillDetails = await screen.findByRole("region", {
        name: "Skill 详情",
      });
      expect(within(skillDetails).getByText("SKILL.md 描述")).toBeVisible();
      expect(
        within(skillDetails).getByText("来自 SKILL.md 的原始能力说明"),
      ).toBeVisible();
    },
  );

  it("ledger 在五行预算内展示五个真实成员", async () => {
    const inventory = parityInventory();
    inventory.entries = Array.from({ length: 6 }, (_, index) =>
      createManagedEntry({
        id: `managed:alpha-${index}`,
        memberId: `member-alpha-${index}`,
        bundleId: "bundle-alpha",
        bundleDisplayName: "Alpha",
        skillName: `skill-${index + 1}`,
      }),
    );
    inventory.mounts = [];
    inventory.bundleUpdates = [];
    render(<App client={createParityClient("ledger", inventory)} />);

    const table = await screen.findByRole("table", {
      name: "Alpha 的 Skill",
    });
    expect(within(table).getAllByRole("row")).toHaveLength(6);
    expect(within(table).getByText("skill-5")).toBeVisible();
    expect(within(table).queryByText(/还有 \d+ 个 Skill/)).not.toBeInTheDocument();
  });

  it.each(themes)(
    "%s 不把只读外部分组冒充成未挂载 Bundle",
    async (theme, libraryName) => {
      const inventory = parityInventory();
      inventory.entries = [
        createManagedEntry({
          id: "agent:official-plugin",
          memberId: null,
          bundleId: null,
          bundleDisplayName: null,
          managementKind: "agentManaged",
          locationKind: "sharedReadOnly",
          observedBy: ["codex"],
          externalGroupDisplayName: "Codex 官方插件",
          skillName: "official-skill",
        }),
      ];
      inventory.mounts = [];
      inventory.bundleUpdates = [];
      render(<App client={createParityClient(theme, inventory)} />);

      const library = await screen.findByRole("region", { name: libraryName });
      const detail = within(library).getByRole("region", {
        name: "Codex 官方插件",
      });
      expect(within(detail).queryByText("当前挂载")).not.toBeInTheDocument();
      expect(within(detail).queryByText("未挂载")).not.toBeInTheDocument();
    },
  );

  it("layers 在切换纸张时保持每个 Bundle 的书脊色阶身份", async () => {
    const user = userEvent.setup();
    const inventory = parityInventory();
    inventory.entries = ["Alpha", "Beta", "Charlie", "Delta", "Echo"].map(
      (name) =>
        createManagedEntry({
          id: `managed:${name.toLowerCase()}`,
          memberId: `member-${name.toLowerCase()}`,
          bundleId: `bundle-${name.toLowerCase()}`,
          bundleDisplayName: name,
          skillName: `${name.toLowerCase()}-skill`,
        }),
    );
    inventory.mounts = [];
    inventory.bundleUpdates = [];
    render(<App client={createParityClient("layers", inventory)} />);

    const library = await screen.findByRole("region", {
      name: "Layers Bundle Library",
    });
    const initialCharlie = within(library).getByRole("button", {
      name: "Charlie",
    });
    const charlieTone = initialCharlie.getAttribute("data-layer-tone");

    await user.click(within(library).getByRole("button", { name: "Delta" }));

    expect(
      within(library).getByRole("button", { name: "Charlie" }),
    ).toHaveAttribute("data-layer-tone", charlieTone);
  });

  it("layers 把纸张作为唯一常规 Bundle tab stop", async () => {
    render(<App client={createParityClient("layers")} />);

    const library = await screen.findByRole("region", {
      name: "Layers Bundle Library",
    });
    expect(library.querySelector(".layers-library-sheet")).toHaveAttribute(
      "tabindex",
      "0",
    );
    for (const spine of within(library)
      .getByRole("navigation", { name: "Bundle" })
      .querySelectorAll(".layers-library-card")) {
      expect(spine).toHaveAttribute("tabindex", "-1");
    }
  });

  it("layers 多 Mount 展示真实 destination，并限制为两个预览", async () => {
    const inventory = parityInventory();
    inventory.mounts = [
      createMount({
        id: "mount-global",
        memberId: "member-alpha",
        targetPath: "/Users/test/.codex/skills/alpha",
      }),
      createMount({
        id: "mount-project",
        memberId: "member-alpha",
        appId: "claudeCode",
        scope: "project",
        projectId: "project-one",
        projectDisplayName: "Project One",
        targetPath: "/repo/.claude/skills/alpha",
      }),
      createMount({
        id: "mount-copilot",
        memberId: "member-alpha",
        appId: "gitHubCopilot",
        targetPath: "/Users/test/.copilot/skills/alpha",
      }),
    ];
    inventory.bundleUpdates = [];
    render(<App client={createParityClient("layers", inventory)} />);

    const library = await screen.findByRole("region", {
      name: "Layers Bundle Library",
    });
    const destinations = within(library).getByRole("list", {
      name: "当前挂载",
    });
    expect(destinations).toHaveTextContent("Codex · 全局");
    expect(destinations).toHaveTextContent("Claude Code · Project One");
    expect(destinations).toHaveTextContent("还有 1 处 Mount");
    expect(destinations).not.toHaveTextContent("GitHub Copilot");
  });

  it("layers 优先用两个不同挂载目标类型代表大量 Mount", async () => {
    const inventory = parityInventory();
    inventory.entries = [
      createManagedEntry({
        id: "managed:alpha-one",
        memberId: "member-alpha-one",
        bundleId: "bundle-alpha",
        bundleDisplayName: "Alpha",
        skillName: "alpha-one",
      }),
      createManagedEntry({
        id: "managed:alpha-two",
        memberId: "member-alpha-two",
        bundleId: "bundle-alpha",
        bundleDisplayName: "Alpha",
        skillName: "alpha-two",
      }),
    ];
    inventory.mounts = [
      createMount({
        id: "mount-claude-one",
        memberId: "member-alpha-one",
        appId: "claudeCode",
        targetPath: "/Users/test/.claude/skills/alpha-one",
      }),
      createMount({
        id: "mount-claude-two",
        memberId: "member-alpha-two",
        appId: "claudeCode",
        targetPath: "/Users/test/.claude/skills/alpha-two",
      }),
      createMount({
        id: "mount-codex-one",
        memberId: "member-alpha-one",
        targetPath: "/Users/test/.codex/skills/alpha-one",
      }),
    ];
    inventory.bundleUpdates = [];
    render(<App client={createParityClient("layers", inventory)} />);

    const destinations = await screen.findByRole("list", {
      name: "当前挂载",
    });
    const rows = within(destinations).getAllByRole("listitem");
    expect(rows[0]).toHaveTextContent("Claude Code · 全局");
    expect(rows[0]).toHaveTextContent("/Users/test/.claude/skills/alpha-one");
    expect(rows[1]).toHaveTextContent("Codex · 全局");
    expect(rows[1]).toHaveTextContent("/Users/test/.codex/skills/alpha-one");
    expect(rows[2]).toHaveTextContent("还有 1 处 Mount");
  });

});

function createParityClient(
  theme: ThemePreset,
  inventory = parityInventory(),
): SkillYardClient {
  const ai: AiPreferences = {
    enabled: true,
    disclosureAccepted: true,
    provider: "openAi",
    model: "gpt-5.6-terra",
    hasApiKey: true,
    verified: true,
  };
  return {
    getPreferences: vi.fn().mockResolvedValue({
      language: "zhCn",
      theme,
      ai,
    }),
    getStartupState: vi.fn().mockResolvedValue(inventory),
    organizeSkillAiExplanations: pendingOperation(),
    checkBundleUpdates: pendingOperation(),
    createBundleUpdateBatchPlan: pendingOperation(),
    refreshLocalInventory: pendingOperation(),
  } as unknown as SkillYardClient;
}

function statusInventory(): Extract<UiOutcome, { type: "inventory" }> {
  const inventory = parityInventory();
  inventory.mounts = [
    createMount({
      id: "mount-beta",
      memberId: "member-beta",
      skillName: "beta",
      health: "conflict",
    }),
  ];
  inventory.bundleUpdates = [
    {
      bundleId: "bundle-alpha",
      status: "upToDate",
      action: null,
      checkedAt: 1_753_000_000_000,
      message: "Alpha 已是最新",
      upstreamUrl: "https://github.com/example/alpha",
    },
    {
      bundleId: "bundle-beta",
      status: "sourceUnavailable",
      action: "checkEditableLocal",
      checkedAt: 1_753_000_000_000,
      message: "本地来源当前不可用",
      upstreamUrl: null,
    },
  ];
  return inventory;
}

function pendingOperation() {
  return vi.fn().mockReturnValue(new Promise<never>(() => {}));
}

function parityInventory(): Extract<UiOutcome, { type: "inventory" }> {
  const alpha = createManagedEntry({
    id: "managed:alpha",
    memberId: "member-alpha",
    bundleId: "bundle-alpha",
    bundleDisplayName: "Alpha",
    skillName: "alpha",
  });
  const beta = createManagedEntry({
    id: "managed:beta",
    memberId: "member-beta",
    bundleId: "bundle-beta",
    bundleDisplayName: "Beta",
    skillName: "beta",
  });
  return {
    type: "inventory",
    scanCompletedAt: 1_753_000_000_000,
    entries: [alpha, beta],
    supportedApps: [],
    lastLocalRefresh: null,
    scanIssues: [],
    recoveryIssues: [],
    recoveredInterruptedOperation: false,
    projects: [],
    mounts: [],
    bundleUpdates: [
      {
        bundleId: "bundle-alpha",
        status: "available",
        action: "update",
        checkedAt: 1,
        message: "Alpha 可更新",
        upstreamUrl: "https://github.com/example/alpha",
      },
      {
        bundleId: "bundle-beta",
        status: "available",
        action: "update",
        checkedAt: 1,
        message: "Beta 可更新",
        upstreamUrl: "https://github.com/example/beta",
      },
    ],
  };
}

function createManagedEntry(
  overrides: Partial<InventoryObservation>,
): InventoryObservation {
  return {
    id: "managed:member",
    memberId: "member",
    skillName: "example",
    declaredName: "example",
    skillRoot: "/tmp/example",
    skillFile: "/tmp/example/SKILL.md",
    locationKind: "managedStore",
    metadataStatus: "valid",
    observedBy: [],
    observedFingerprint: "fingerprint",
    rootKey: null,
    projectId: null,
    stale: false,
    managementKind: "skillYardManaged",
    installationChain: null,
    takeoverGroupId: null,
    takeoverGroupDisplayName: null,
    aiExplanation: null,
    bundleId: "bundle",
    bundleDisplayName: "Bundle",
    ...overrides,
  };
}

function createMount(overrides: Partial<MountSummary>): MountSummary {
  return {
    id: "mount",
    memberId: "member",
    skillName: "example",
    appId: "codex",
    scope: "global",
    projectId: null,
    projectDisplayName: null,
    targetPath: "/tmp/.codex/skills/example",
    expectedTarget: "/tmp/central/bundles/bundle/current/members/example",
    health: "healthy",
    ...overrides,
  };
}
