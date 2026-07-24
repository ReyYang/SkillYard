import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import type {
  BatchMountPlan,
  BundleUpdateBatchPlan,
  BundleUpdateBatchResult,
  EditableLocalRelinkPlan,
  InstallPlan,
  InventoryObservation,
  MountPlan,
  MountSummary,
  RemovalPlan,
  SourceAssociationPlan,
  SourceRefChangePlan,
  SourceSummary,
  TakeoverPlan,
  UiOutcome,
} from "./domain";
import type { SkillYardClient } from "./skillyardClient";

describe("首次使用", () => {
  it("先解释扫描边界，并且不会在页面挂载时自动扫描", async () => {
    const client = createClient({
      type: "onboardingRequired",
      supportedApps: [
        { id: "codex", displayName: "Codex", detected: null },
        { id: "claudeCode", displayName: "Claude Code", detected: null },
        {
          id: "gitHubCopilot",
          displayName: "GitHub Copilot",
          detected: null,
        },
      ],
    });

    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", {
        name: "管理本机 Skill，从一次只读扫描开始",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText(/不会自动接管、移动、覆盖或删除/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "开始扫描" })).toBeEnabled();
    expect(client.startInitialScan).not.toHaveBeenCalled();
    expect(client.refreshLocalInventory).not.toHaveBeenCalled();
  });

  it("只在用户点击后扫描，并阻止重复提交", async () => {
    const user = userEvent.setup();
    let finishScan: ((outcome: UiOutcome) => void) | undefined;
    const client = createClient({
      type: "onboardingRequired",
      supportedApps: [],
    });
    vi.mocked(client.startInitialScan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishScan = resolve;
        }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "开始扫描" }));

    expect(client.startInitialScan).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "正在扫描…" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "正在扫描…" }));
    expect(client.startInitialScan).toHaveBeenCalledTimes(1);

    await act(async () => {
      finishScan?.(inventoryOutcome([createEntry()]));
    });

    expect(
      screen.getByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
    expect(screen.getByText("example")).toBeInTheDocument();
  });

  it("扫描失败时显示 Rust 返回的结构化错误", async () => {
    const user = userEvent.setup();
    const client = createClient({
      type: "onboardingRequired",
      supportedApps: [],
    });
    vi.mocked(client.startInitialScan).mockRejectedValue({
      code: "scanError",
      message: "无法读取扫描根目录",
    });

    render(<App client={client} />);
    await user.click(await screen.findByRole("button", { name: "开始扫描" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "无法读取扫描根目录",
    );
  });
});

describe("本机清单", () => {
  it("直接显示已保存清单，不自动刷新", async () => {
    const client = createClient(
      inventoryOutcome([createEntry({ skillName: "saved" })]),
    );

    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
    expect(screen.getByText("saved")).toBeInTheDocument();
    expect(client.startInitialScan).not.toHaveBeenCalled();
    expect(client.refreshLocalInventory).not.toHaveBeenCalled();
  });

  it("重置应用只清除窗口状态和临时错误，并重新读取原托管状态", async () => {
    const user = userEvent.setup();
    const startup = inventoryOutcome([
      createEntry({ skillName: "saved-after-reset" }),
    ]);
    const client = createClient(startup);
    vi.mocked(client.refreshLocalInventory).mockRejectedValue(
      new Error("临时刷新失败"),
    );
    render(<App client={client} />);

    await screen.findByText("saved-after-reset");
    await user.click(screen.getByRole("button", { name: "刷新本机" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("临时刷新失败");

    await user.type(
      screen.getByRole("searchbox", { name: "搜索 Skill" }),
      "不存在",
    );
    expect(screen.queryByText("saved-after-reset")).toBeNull();

    await user.click(screen.getByRole("button", { name: "重置应用" }));

    expect(await screen.findByText("saved-after-reset")).toBeInTheDocument();
    expect(screen.getByRole("searchbox", { name: "搜索 Skill" })).toHaveValue(
      "",
    );
    expect(screen.queryByText("临时刷新失败")).toBeNull();
    expect(client.getStartupState).toHaveBeenCalledTimes(2);
    expect(client.confirmInstallPlan).not.toHaveBeenCalled();
    expect(client.confirmRemovalPlan).not.toHaveBeenCalled();
    expect(client.confirmMountPlan).not.toHaveBeenCalled();
  });

  it("只在用户点击后检查 Bundle 更新，并在检查期间冻结写入口", async () => {
    const user = userEvent.setup();
    let finishCheck:
      | ((
          outcome: Extract<UiOutcome, { type: "inventory" }>,
        ) => void)
      | undefined;
    const initial = inventoryOutcome(
      [createManagedEntry({ sourceDisplayName: "owner/repo" })],
      null,
      {
        bundleUpdates: [
          {
            bundleId: "bundle-1",
            status: "notChecked",
            action: null,
            checkedAt: null,
            message: "尚未检查更新",
            upstreamUrl: "https://github.com/owner/repo",
          },
        ],
      },
    );
    const client = createClient(initial);
    vi.mocked(client.checkBundleUpdates).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishCheck = resolve;
        }),
    );
    render(<App client={client} />);

    expect(
      await screen.findByLabelText("Bundle 更新状态：尚未检查"),
    ).toBeInTheDocument();
    expect(client.checkBundleUpdates).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "检查更新" }));

    expect(client.checkBundleUpdates).toHaveBeenCalledTimes(1);
    expect(
      screen.getByRole("button", { name: "正在检查更新…" }),
    ).toBeDisabled();
    expect(screen.getByRole("button", { name: "安装 Skill" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "添加项目" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "刷新本机" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "批量挂载" })).toBeDisabled();
    expect(
      screen.getByRole("button", {
        name: "删除 Bundle example-bundle",
      }),
    ).toBeDisabled();
    expect(screen.getByRole("searchbox", { name: "搜索 Skill" })).toBeEnabled();

    await act(async () => {
      finishCheck?.(
        inventoryOutcome(
          [createManagedEntry({ sourceDisplayName: "owner/repo" })],
          null,
          {
            bundleUpdates: [
              {
                bundleId: "bundle-1",
                status: "available",
                action: "update",
                checkedAt: 1_753_000_001_000,
                message: "发现新的上游 commit",
                upstreamUrl: "https://github.com/owner/repo",
              },
            ],
          },
        ),
      );
    });

    expect(
      screen.getByLabelText("Bundle 更新状态：可更新"),
    ).toBeInTheDocument();
    expect(screen.getByText("更新")).toBeInTheDocument();
  });

  it("按 Bundle 启动更新准备，并在准备期间冻结写入口", async () => {
    const user = userEvent.setup();
    let finishPlan: ((plan: InstallPlan) => void) | undefined;
    const client = createClient(
      inventoryOutcome(
        [
          createManagedEntry({
            bundleDisplayName: "superpowers",
            sourceDisplayName: "obra/superpowers",
          }),
        ],
        null,
        {
          bundleUpdates: [
            {
              bundleId: "bundle-1",
              status: "available",
              action: "update",
              checkedAt: 1_753_000_001_000,
              message: "发现新的上游 commit",
              upstreamUrl: "https://github.com/obra/superpowers",
            },
          ],
        },
      ),
    );
    vi.mocked(client.createBundleUpdatePlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishPlan = resolve;
        }),
    );
    render(<App client={client} />);

    const updateButton = await screen.findByRole("button", {
      name: "更新 superpowers",
    });
    await user.click(updateButton);

    expect(client.createBundleUpdatePlan).toHaveBeenCalledWith("bundle-1");
    expect(updateButton).toBeDisabled();
    expect(screen.getByRole("button", { name: "检查更新" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "安装 Skill" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "添加项目" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "刷新本机" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "批量挂载" })).toBeDisabled();
    expect(screen.getByRole("searchbox", { name: "搜索 Skill" })).toBeEnabled();

    await act(async () => {
      finishPlan?.(
        createInstallPlan({
          mode: "update",
          bundleDisplayName: "superpowers",
          updateImpact: {
            newCandidateIds: [],
            existingMounts: [],
            upstreamUrl: "https://github.com/obra/superpowers",
          },
        }),
      );
    });
    expect(
      screen.getByRole("heading", { name: "确认更新这个 Bundle" }),
    ).toBeInTheDocument();
  });

  it("更新预览只读展示全部成员、新增成员和现有 Mount", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome(
        [
          createManagedEntry({
            bundleDisplayName: "superpowers",
            sourceDisplayName: "obra/superpowers",
          }),
        ],
        null,
        {
          bundleUpdates: [
            {
              bundleId: "bundle-1",
              status: "available",
              action: "update",
              checkedAt: 1,
              message: "发现新的上游 commit",
              upstreamUrl: "https://github.com/obra/superpowers",
            },
          ],
        },
      ),
    );
    vi.mocked(client.createBundleUpdatePlan).mockResolvedValue(
      createInstallPlan({
        mode: "update",
        inputKind: "github",
        inputPath: "https://github.com/obra/superpowers",
        bundleDisplayName: "superpowers",
        candidates: [
          createInstallCandidate({
            candidateId: "candidate-brainstorming",
            sourceRelativePath: "skills/brainstorming",
            skillName: "brainstorming",
          }),
          createInstallCandidate({
            candidateId: "candidate-tdd",
            sourceRelativePath: "skills/tdd",
            skillName: "tdd",
          }),
        ],
        updateImpact: {
          newCandidateIds: ["candidate-tdd"],
          existingMounts: [
            createMount({
              id: "mount-global",
              memberId: "member-brainstorming",
              skillName: "brainstorming",
            }),
            createMount({
              id: "mount-project",
              memberId: "member-brainstorming",
              skillName: "brainstorming",
              appId: "claudeCode",
              scope: "project",
              projectId: "project-1",
              projectDisplayName: "SkillYard",
            }),
          ],
          upstreamUrl: "https://github.com/obra/superpowers",
        },
      }),
    );
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", { name: "更新 superpowers" }),
    );

    const preview = await screen.findByLabelText("更新影响预览");
    expect(within(preview).queryByRole("checkbox")).not.toBeInTheDocument();
    expect(within(preview).getAllByText("brainstorming").length).toBeGreaterThan(
      0,
    );
    expect(within(preview).getByText("tdd")).toBeInTheDocument();
    expect(within(preview).getByText("新增安装")).toBeInTheDocument();
    const mounts = within(preview).getByLabelText("现有挂载");
    expect(mounts).toHaveTextContent("brainstorming");
    expect(mounts).toHaveTextContent("Codex · 全局");
    expect(mounts).toHaveTextContent("Claude Code · 项目 · SkillYard");
    expect(preview).toHaveTextContent("现有挂载继续使用");
    expect(preview).toHaveTextContent("新增 Skill 保持未挂载");
    expect(
      within(preview).getByRole("link", { name: "查看上游发布页" }),
    ).toHaveAttribute("href", "https://github.com/obra/superpowers");
  });

  it("更新候选包含无效 Skill 时禁止确认整组更新", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([createManagedEntry()], null, {
        bundleUpdates: [
          {
            bundleId: "bundle-1",
            status: "available",
            action: "update",
            checkedAt: 1,
            message: "发现新的上游 commit",
            upstreamUrl: "https://github.com/anthropics/skills",
          },
        ],
      }),
    );
    vi.mocked(client.createBundleUpdatePlan).mockResolvedValue(
      createInstallPlan({
        mode: "update",
        inputKind: "github",
        candidates: [
          createInstallCandidate({
            selectable: false,
            validationErrors: ["SKILL.md 缺少有效 name"],
          }),
        ],
        updateImpact: {
          newCandidateIds: [],
          existingMounts: [],
          upstreamUrl: "https://github.com/anthropics/skills",
        },
      }),
    );
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", { name: "更新 example-bundle" }),
    );

    expect(screen.getByText("SKILL.md 缺少有效 name")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认更新" })).toBeDisabled();
    expect(client.confirmInstallPlan).not.toHaveBeenCalled();
  });

  it("确认更新提交全部成员，并在成功后返回 Inventory", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome(
        [
          createManagedEntry({
            bundleDisplayName: "superpowers",
            sourceDisplayName: "obra/superpowers",
          }),
        ],
        null,
        {
          bundleUpdates: [
            {
              bundleId: "bundle-1",
              status: "available",
              action: "update",
              checkedAt: 1,
              message: "发现新的上游 commit",
              upstreamUrl: "https://github.com/obra/superpowers",
            },
          ],
        },
      ),
    );
    vi.mocked(client.createBundleUpdatePlan).mockResolvedValue(
      createInstallPlan({
        mode: "update",
        inputKind: "github",
        bundleDisplayName: "superpowers",
        candidates: [
          createInstallCandidate({
            candidateId: "candidate-brainstorming",
            skillName: "brainstorming",
          }),
          createInstallCandidate({
            candidateId: "candidate-tdd",
            skillName: "tdd",
          }),
        ],
        updateImpact: {
          newCandidateIds: ["candidate-tdd"],
          existingMounts: [],
          upstreamUrl: "https://github.com/obra/superpowers",
        },
      }),
    );
    vi.mocked(client.confirmInstallPlan).mockResolvedValue(
      inventoryOutcome([
        createManagedEntry({
          bundleDisplayName: "superpowers",
          skillName: "brainstorming",
        }),
        createManagedEntry({
          id: "managed:member-tdd",
          memberId: "member-tdd",
          bundleDisplayName: "superpowers",
          skillName: "tdd",
        }),
      ]),
    );
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", { name: "更新 superpowers" }),
    );
    await user.click(
      await screen.findByRole("button", { name: "确认更新" }),
    );

    expect(client.confirmInstallPlan).toHaveBeenCalledWith("plan-1", [
      "candidate-brainstorming",
      "candidate-tdd",
    ]);
    expect(
      await screen.findByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
    expect(screen.getByText("superpowers: tdd")).toBeInTheDocument();
  });

  it("更新确认失败后可以用新 Plan 重试，不继承旧错误", async () => {
    const user = userEvent.setup();
    const initial = inventoryOutcome(
      [
        createManagedEntry({
          bundleDisplayName: "superpowers",
          sourceDisplayName: "obra/superpowers",
        }),
      ],
      null,
      {
        bundleUpdates: [
          {
            bundleId: "bundle-1",
            status: "available",
            action: "update",
            checkedAt: 1,
            message: "发现新的上游 commit",
            upstreamUrl: "https://github.com/obra/superpowers",
          },
        ],
      },
    );
    const client = createClient(initial);
    vi.mocked(client.createBundleUpdatePlan).mockResolvedValue(
      createInstallPlan({
        mode: "update",
        inputKind: "github",
        bundleDisplayName: "superpowers",
        updateImpact: {
          newCandidateIds: [],
          existingMounts: [],
          upstreamUrl: "https://github.com/obra/superpowers",
        },
      }),
    );
    vi.mocked(client.confirmInstallPlan).mockRejectedValueOnce({
      code: "lifecycleError",
      message: "更新中断，已自动恢复",
    });
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", { name: "更新 superpowers" }),
    );
    await user.click(screen.getByRole("button", { name: "确认更新" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "更新中断，已自动恢复",
    );

    await user.click(
      screen.getByRole("button", { name: "更新 superpowers" }),
    );
    expect(
      await screen.findByRole("button", { name: "确认更新" }),
    ).toBeEnabled();
    expect(client.createBundleUpdatePlan).toHaveBeenCalledTimes(2);
  });

  it("更新 Plan 准备失败时保留 Inventory 和可更新状态", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome(
        [
          createManagedEntry({
            bundleDisplayName: "superpowers",
            sourceDisplayName: "obra/superpowers",
          }),
        ],
        null,
        {
          bundleUpdates: [
            {
              bundleId: "bundle-1",
              status: "available",
              action: "update",
              checkedAt: 1,
              message: "发现新的上游 commit",
              upstreamUrl: "https://github.com/obra/superpowers",
            },
          ],
        },
      ),
    );
    vi.mocked(client.createBundleUpdatePlan).mockRejectedValue({
      code: "sourceUnavailable",
      message: "暂时无法获取这个 Bundle 的更新内容",
    });
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", { name: "更新 superpowers" }),
    );

    expect(
      await screen.findByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("更新未完成");
    expect(screen.getByRole("alert")).toHaveTextContent(
      "暂时无法获取这个 Bundle 的更新内容",
    );
    expect(
      screen.getByRole("button", { name: "更新 superpowers" }),
    ).toBeEnabled();
  });

  it("从更新预览返回时复用安装 Plan 的真实丢弃流程", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome(
        [
          createManagedEntry({
            bundleDisplayName: "superpowers",
            sourceDisplayName: "obra/superpowers",
          }),
        ],
        null,
        {
          bundleUpdates: [
            {
              bundleId: "bundle-1",
              status: "available",
              action: "update",
              checkedAt: 1,
              message: "发现新的上游 commit",
              upstreamUrl: "https://github.com/obra/superpowers",
            },
          ],
        },
      ),
    );
    vi.mocked(client.createBundleUpdatePlan).mockResolvedValue(
      createInstallPlan({
        id: "update-plan-1",
        mode: "update",
        inputKind: "github",
        bundleDisplayName: "superpowers",
        updateImpact: {
          newCandidateIds: [],
          existingMounts: [],
          upstreamUrl: "https://github.com/obra/superpowers",
        },
      }),
    );
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", { name: "更新 superpowers" }),
    );
    await user.click(await screen.findByRole("button", { name: "返回" }));

    expect(client.discardInstallPlan).toHaveBeenCalledWith("update-plan-1");
    expect(
      await screen.findByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
    expect(client.confirmInstallPlan).not.toHaveBeenCalled();
  });

  it("手动替换取消原生选择器时保留 Inventory", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome(
        [
          createManagedEntry({
            bundleId: "bundle-archive",
            bundleDisplayName: "archive-bundle",
            sourceDisplayName: "archive.zip",
          }),
        ],
        null,
        {
          bundleUpdates: [
            {
              bundleId: "bundle-archive",
              status: "manual",
              action: "importReplacement",
              checkedAt: null,
              message: "选择新的归档或文件来更新",
              upstreamUrl: null,
            },
          ],
        },
      ),
    );
    vi.mocked(client.chooseBundleReplacementPlan).mockResolvedValue(null);
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", {
        name: "导入新内容 archive-bundle",
      }),
    );

    expect(client.chooseBundleReplacementPlan).toHaveBeenCalledWith(
      "bundle-archive",
    );
    expect(
      screen.getByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "导入新内容 archive-bundle" }),
    ).toBeEnabled();
    expect(client.confirmInstallPlan).not.toHaveBeenCalled();
  });

  it("手动替换选择成功后冻结写入口并进入通用更新 Plan", async () => {
    const user = userEvent.setup();
    let finishSelection: ((plan: InstallPlan | null) => void) | undefined;
    const client = createClient(
      inventoryOutcome(
        [
          createManagedEntry({
            bundleId: "bundle-archive",
            bundleDisplayName: "archive-bundle",
            sourceDisplayName: "archive.zip",
          }),
        ],
        null,
        {
          bundleUpdates: [
            {
              bundleId: "bundle-archive",
              status: "manual",
              action: "importReplacement",
              checkedAt: null,
              message: "选择新的归档或文件来更新",
              upstreamUrl: null,
            },
          ],
        },
      ),
    );
    vi.mocked(client.chooseBundleReplacementPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishSelection = resolve;
        }),
    );
    render(<App client={client} />);

    const replacementButton = await screen.findByRole("button", {
      name: "导入新内容 archive-bundle",
    });
    await user.click(replacementButton);

    expect(client.chooseBundleReplacementPlan).toHaveBeenCalledWith(
      "bundle-archive",
    );
    expect(replacementButton).toBeDisabled();
    expect(screen.getByText("正在选择新内容…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "检查更新" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "安装 Skill" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "添加项目" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "刷新本机" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "批量挂载" })).toBeDisabled();
    expect(screen.getByRole("searchbox", { name: "搜索 Skill" })).toBeEnabled();

    await act(async () => {
      finishSelection?.(
        createInstallPlan({
          id: "replacement-plan-1",
          mode: "update",
          inputKind: "archive",
          inputPath: "/tmp/replacement.zip",
          bundleDisplayName: "archive-bundle",
          updateImpact: {
            newCandidateIds: [],
            existingMounts: [],
            upstreamUrl: null,
          },
        }),
      );
    });

    expect(
      screen.getByRole("heading", { name: "确认更新这个 Bundle" }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("更新影响预览")).toHaveTextContent(
      "/tmp/replacement.zip",
    );
    expect(
      screen.queryByRole("link", { name: "查看上游发布页" }),
    ).not.toBeInTheDocument();
  });

  it("手动替换选择失败时保留 Inventory 和重试入口", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome(
        [
          createManagedEntry({
            bundleId: "bundle-direct",
            bundleDisplayName: "direct-bundle",
            sourceDisplayName: "https://example.com/skill.md",
          }),
        ],
        null,
        {
          bundleUpdates: [
            {
              bundleId: "bundle-direct",
              status: "manual",
              action: "importReplacement",
              checkedAt: null,
              message: "选择新文件来更新",
              upstreamUrl: null,
            },
          ],
        },
      ),
    );
    vi.mocked(client.chooseBundleReplacementPlan).mockRejectedValue({
      code: "sourceUnavailable",
      message: "无法读取所选替换内容",
    });
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", {
        name: "导入新内容 direct-bundle",
      }),
    );

    expect(
      screen.getByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("无法读取所选替换内容");
    expect(
      screen.getByRole("button", { name: "导入新内容 direct-bundle" }),
    ).toBeEnabled();
  });

  it("Editable Local 检查到改动时冻结写入口并显示通用更新入口", async () => {
    const user = userEvent.setup();
    let finishCheck:
      | ((
          outcome: Extract<UiOutcome, { type: "inventory" }>,
        ) => void)
      | undefined;
    const editableEntry = createManagedEntry({
      id: "managed:editable",
      memberId: "member-editable",
      bundleId: "bundle-editable",
      bundleDisplayName: "editable-bundle",
      sourceDisplayName: "editable-folder",
    });
    const client = createClient(
      inventoryOutcome([editableEntry], null, {
        bundleUpdates: [
          {
            bundleId: "bundle-editable",
            status: "notChecked",
            action: "checkEditableLocal",
            checkedAt: null,
            message: "检查本地改动后可以采用全部内容",
            upstreamUrl: null,
          },
        ],
      }),
    );
    vi.mocked(client.checkEditableLocalBundle).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishCheck = resolve;
        }),
    );
    vi.mocked(client.createBundleUpdatePlan).mockResolvedValue(
      createInstallPlan({
        mode: "update",
        inputKind: "editableLocal",
        bundleDisplayName: "editable-bundle",
        updateImpact: {
          newCandidateIds: [],
          existingMounts: [],
          upstreamUrl: null,
        },
      }),
    );
    render(<App client={client} />);

    const checkButton = await screen.findByRole("button", {
      name: "检查本地改动 editable-bundle",
    });
    await user.click(checkButton);

    expect(client.checkEditableLocalBundle).toHaveBeenCalledWith(
      "bundle-editable",
    );
    expect(checkButton).toBeDisabled();
    expect(screen.getByText("正在检查本地改动…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "检查更新" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "安装 Skill" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "添加项目" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "刷新本机" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "批量挂载" })).toBeDisabled();
    expect(screen.getByRole("searchbox", { name: "搜索 Skill" })).toBeEnabled();

    await act(async () => {
      finishCheck?.(
        inventoryOutcome([editableEntry], null, {
          bundleUpdates: [
            {
              bundleId: "bundle-editable",
              status: "available",
              action: "update",
              checkedAt: 1_753_000_001_000,
              message: "检测到本地内容变化",
              upstreamUrl: null,
            },
          ],
        }),
      );
    });

    expect(
      screen.getByLabelText("Bundle 更新状态：可更新"),
    ).toBeInTheDocument();
    const updateButton = screen.getByRole("button", {
      name: "更新 editable-bundle",
    });
    expect(updateButton).toBeEnabled();
    await user.click(updateButton);
    expect(client.createBundleUpdatePlan).toHaveBeenCalledWith(
      "bundle-editable",
    );
    expect(
      screen.getByRole("heading", { name: "确认更新这个 Bundle" }),
    ).toBeInTheDocument();
  });

  it("Editable Local 没有改动时显示已是最新并保留再次检查", async () => {
    const user = userEvent.setup();
    const editableEntry = createManagedEntry({
      id: "managed:editable",
      memberId: "member-editable",
      bundleId: "bundle-editable",
      bundleDisplayName: "editable-bundle",
      sourceDisplayName: "editable-folder",
    });
    const client = createClient(
      inventoryOutcome([editableEntry], null, {
        bundleUpdates: [
          {
            bundleId: "bundle-editable",
            status: "notChecked",
            action: "checkEditableLocal",
            checkedAt: null,
            message: "尚未检查本地改动",
            upstreamUrl: null,
          },
        ],
      }),
    );
    vi.mocked(client.checkEditableLocalBundle).mockResolvedValue(
      inventoryOutcome([editableEntry], null, {
        bundleUpdates: [
          {
            bundleId: "bundle-editable",
            status: "upToDate",
            action: "checkEditableLocal",
            checkedAt: 1_753_000_001_000,
            message: "本地内容与当前版本一致",
            upstreamUrl: null,
          },
        ],
      }),
    );
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", {
        name: "检查本地改动 editable-bundle",
      }),
    );

    expect(
      screen.getByLabelText("Bundle 更新状态：已是最新"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "再次检查 editable-bundle" }),
    ).toBeEnabled();
  });

  it("Editable Local 来源不可用时保留重新检查并可重试", async () => {
    const user = userEvent.setup();
    const editableEntry = createManagedEntry({
      id: "managed:editable",
      memberId: "member-editable",
      bundleId: "bundle-editable",
      bundleDisplayName: "editable-bundle",
      sourceDisplayName: "editable-folder",
    });
    const initial = inventoryOutcome([editableEntry], null, {
      bundleUpdates: [
        {
          bundleId: "bundle-editable",
          status: "notChecked",
          action: "checkEditableLocal",
          checkedAt: null,
          message: "尚未检查本地改动",
          upstreamUrl: null,
        },
      ],
    });
    const client = createClient(initial);
    vi.mocked(client.checkEditableLocalBundle)
      .mockResolvedValueOnce(
        inventoryOutcome([editableEntry], null, {
          bundleUpdates: [
            {
              bundleId: "bundle-editable",
              status: "sourceUnavailable",
              action: "checkEditableLocal",
              checkedAt: 1_753_000_001_000,
              message: "原始本地目录暂时无法读取",
              upstreamUrl: null,
            },
          ],
        }),
      )
      .mockResolvedValueOnce(
        inventoryOutcome([editableEntry], null, {
          bundleUpdates: [
            {
              bundleId: "bundle-editable",
              status: "upToDate",
              action: "checkEditableLocal",
              checkedAt: 1_753_000_002_000,
              message: "本地内容与当前版本一致",
              upstreamUrl: null,
            },
          ],
        }),
      );
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", {
        name: "检查本地改动 editable-bundle",
      }),
    );

    expect(
      screen.getByLabelText("Bundle 更新状态：来源不可用"),
    ).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", {
        name: "重新检查 editable-bundle",
      }),
    );

    expect(client.checkEditableLocalBundle).toHaveBeenNthCalledWith(
      1,
      "bundle-editable",
    );
    expect(client.checkEditableLocalBundle).toHaveBeenNthCalledWith(
      2,
      "bundle-editable",
    );
    expect(
      screen.getByLabelText("Bundle 更新状态：已是最新"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "再次检查 editable-bundle" }),
    ).toBeEnabled();
  });

  it("Editable Local 检查命令失败时保留 Inventory 和检查入口", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome(
        [
          createManagedEntry({
            id: "managed:editable",
            memberId: "member-editable",
            bundleId: "bundle-editable",
            bundleDisplayName: "editable-bundle",
            sourceDisplayName: "editable-folder",
          }),
        ],
        null,
        {
          bundleUpdates: [
            {
              bundleId: "bundle-editable",
              status: "notChecked",
              action: "checkEditableLocal",
              checkedAt: null,
              message: "尚未检查本地改动",
              upstreamUrl: null,
            },
          ],
        },
      ),
    );
    vi.mocked(client.checkEditableLocalBundle).mockRejectedValue({
      code: "storageError",
      message: "暂时无法保存检查结果",
    });
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", {
        name: "检查本地改动 editable-bundle",
      }),
    );

    expect(
      screen.getByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("暂时无法保存检查结果");
    expect(
      screen.getByRole("button", {
        name: "检查本地改动 editable-bundle",
      }),
    ).toBeEnabled();
  });

  it("没有来源的 Bundle 只显示状态，不提供生命周期按钮", async () => {
    const client = createClient(
      inventoryOutcome(
        [
          createManagedEntry({
            id: "managed:local",
            memberId: "member-local",
            bundleId: "bundle-local",
            bundleDisplayName: "local-bundle",
            sourceDisplayName: null,
          }),
        ],
        null,
        {
          bundleUpdates: [
            {
              bundleId: "bundle-local",
              status: "noSource",
              action: null,
              checkedAt: null,
              message: "没有更新来源",
              upstreamUrl: null,
            },
          ],
        },
      ),
    );
    render(<App client={client} />);

    const local = await screen.findByRole("region", { name: "local-bundle" });
    expect(local).toHaveTextContent("没有更新来源");
    expect(
      within(local).queryByRole("button", { name: /更新|导入|检查/ }),
    ).not.toBeInTheDocument();
  });

  it("至少两个普通可更新 Bundle 时才显示全部更新，手动来源不计数", async () => {
    const eligibleClient = createClient(inventoryWithTwoAvailableUpdates());
    const { unmount } = render(<App client={eligibleClient} />);

    expect(
      await screen.findByRole("button", { name: "全部更新" }),
    ).toBeEnabled();
    unmount();

    const ineligibleClient = createClient(
      inventoryOutcome(
        [
          createManagedEntry({
            bundleId: "bundle-alpha",
            bundleDisplayName: "Alpha",
          }),
          createManagedEntry({
            id: "managed:manual",
            memberId: "member-manual",
            bundleId: "bundle-manual",
            bundleDisplayName: "Manual",
          }),
        ],
        null,
        {
          bundleUpdates: [
            {
              bundleId: "bundle-alpha",
              status: "available",
              action: "update",
              checkedAt: 1,
              message: "可更新",
              upstreamUrl: null,
            },
            {
              bundleId: "bundle-manual",
              status: "manual",
              action: "importReplacement",
              checkedAt: null,
              message: "请导入新内容",
              upstreamUrl: null,
            },
          ],
        },
      ),
    );
    render(<App client={ineligibleClient} />);

    await screen.findByRole("heading", { name: "Skill 清单" });
    expect(
      screen.queryByRole("button", { name: "全部更新" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "导入新内容 Manual" }),
    ).toBeEnabled();
  });

  it("准备全部更新期间冻结写入口并保留清单搜索", async () => {
    const user = userEvent.setup();
    let finishPlan:
      | ((
          outcome: Extract<UiOutcome, { type: "bundleUpdateBatchPlan" }>,
        ) => void)
      | undefined;
    const client = createClient(inventoryWithTwoAvailableUpdates());
    vi.mocked(client.createBundleUpdateBatchPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishPlan = resolve;
        }),
    );
    render(<App client={client} />);

    const updateAll = await screen.findByRole("button", {
      name: "全部更新",
    });
    await user.click(updateAll);

    expect(client.createBundleUpdateBatchPlan).toHaveBeenCalledTimes(1);
    expect(updateAll).toBeDisabled();
    expect(screen.getByText("正在准备全部更新…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "检查更新" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "安装 Skill" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "添加项目" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "刷新本机" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "更新 Alpha" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "更新 Beta" })).toBeDisabled();
    expect(
      screen
        .getAllByRole("button", { name: "批量挂载" })
        .every((button) => button.hasAttribute("disabled")),
    ).toBe(true);
    expect(screen.getByRole("searchbox", { name: "搜索 Skill" })).toBeEnabled();

    await act(async () => {
      finishPlan?.({
        type: "bundleUpdateBatchPlan",
        plan: createBundleUpdateBatchPlan(),
      });
    });

    expect(
      screen.getByRole("heading", { name: "确认全部更新" }),
    ).toBeInTheDocument();
  });

  it("批量预览只允许选择 Bundle，并按页面顺序确认 Ready 项", async () => {
    const user = userEvent.setup();
    let finishConfirm:
      | ((
          outcome: Extract<UiOutcome, { type: "bundleUpdateBatchResult" }>,
        ) => void)
      | undefined;
    const plan = createBundleUpdateBatchPlan({
      items: [
        createBundleUpdateBatchPlanItem({
          id: "item-beta",
          bundleId: "bundle-beta",
          bundleDisplayName: "Beta",
          installPlan: createInstallPlan({
            id: "child-beta",
            mode: "update",
            inputKind: "github",
            bundleDisplayName: "Beta",
            candidates: [
              createInstallCandidate({
                candidateId: "candidate-beta",
                skillName: "beta",
              }),
              createInstallCandidate({
                candidateId: "candidate-beta-new",
                skillName: "beta-new",
              }),
            ],
            updateImpact: {
              newCandidateIds: ["candidate-beta-new"],
              existingMounts: [
                createMount({
                  id: "mount-beta",
                  memberId: "member-beta",
                  skillName: "beta",
                }),
              ],
              upstreamUrl: "https://github.com/example/beta",
            },
          }),
        }),
        createBundleUpdateBatchPlanItem({
          id: "item-broken",
          bundleId: "bundle-broken",
          bundleDisplayName: "Broken",
          disposition: "preparationFailed",
          installPlan: null,
          errorSummary: "无法获取这个 Bundle 的当前内容",
        }),
        createBundleUpdateBatchPlanItem({
          id: "item-alpha",
          bundleId: "bundle-alpha",
          bundleDisplayName: "Alpha",
          installPlan: createInstallPlan({
            id: "child-alpha",
            mode: "update",
            inputKind: "github",
            bundleDisplayName: "Alpha",
          }),
        }),
      ],
    });
    const client = createClient({ type: "bundleUpdateBatchPlan", plan });
    vi.mocked(client.confirmBundleUpdateBatchPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishConfirm = resolve;
        }),
    );
    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", { name: "确认全部更新" }),
    ).toBeInTheDocument();
    expect(screen.getAllByRole("checkbox")).toHaveLength(3);
    const beta = screen.getByRole("checkbox", { name: "更新 Beta" });
    const broken = screen.getByRole("checkbox", { name: "更新 Broken" });
    const alpha = screen.getByRole("checkbox", { name: "更新 Alpha" });
    expect(beta).toBeChecked();
    expect(alpha).toBeChecked();
    expect(broken).toBeDisabled();
    expect(
      within(
        screen.getByRole("region", { name: "Bundle 更新预览：Beta" }),
      ).getAllByRole("checkbox"),
    ).toHaveLength(1);
    expect(screen.getByLabelText("Beta 全部 Skill")).toHaveTextContent(
      "beta-new",
    );
    expect(screen.getByText("新增安装")).toBeInTheDocument();
    expect(screen.getByLabelText("Beta 现有挂载")).toHaveTextContent(
      "Codex · 全局",
    );
    expect(screen.getByText("无法获取这个 Bundle 的当前内容")).toBeInTheDocument();

    await user.click(beta);
    await user.click(alpha);
    expect(
      screen.getByRole("button", { name: "确认全部更新" }),
    ).toBeDisabled();
    await user.click(alpha);
    await user.click(beta);

    const back = screen.getByRole("button", { name: "返回清单" });
    const confirm = screen.getByRole("button", { name: "确认全部更新" });
    await user.click(confirm);

    expect(client.confirmBundleUpdateBatchPlan).toHaveBeenCalledWith(
      "update-batch-plan-1",
      ["item-beta", "item-alpha"],
    );
    expect(confirm).toBeDisabled();
    expect(back).toBeDisabled();
    expect(beta).toBeDisabled();
    expect(alpha).toBeDisabled();
    await user.click(confirm);
    expect(client.confirmBundleUpdateBatchPlan).toHaveBeenCalledTimes(1);

    await act(async () => {
      finishConfirm?.({
        type: "bundleUpdateBatchResult",
        result: createBundleUpdateBatchResult({
          items: [
            createBundleUpdateBatchResultItem({
              id: "item-beta",
              bundleId: "bundle-beta",
              bundleDisplayName: "Beta",
              status: "failed",
              errorSummary: "更新失败，已保留原内容",
            }),
            createBundleUpdateBatchResultItem({
              id: "item-broken",
              bundleId: "bundle-broken",
              bundleDisplayName: "Broken",
              status: "notExecuted",
              errorSummary: "准备阶段未通过",
            }),
            createBundleUpdateBatchResultItem({
              id: "item-alpha",
              bundleId: "bundle-alpha",
              bundleDisplayName: "Alpha",
              status: "succeeded",
            }),
          ],
        }),
      });
    });

    const result = screen.getByRole("region", { name: "全部更新结果" });
    expect(result).toHaveTextContent("Beta");
    expect(result).toHaveTextContent("失败");
    expect(result).toHaveTextContent("Broken");
    expect(result).toHaveTextContent("未执行");
    expect(result).toHaveTextContent("Alpha");
    expect(result).toHaveTextContent("成功");
    expect(
      screen.getByRole("button", { name: "返回清单" }),
    ).toBeEnabled();
  });

  it("返回批量预览时调用真实 discard，并在清理期间冻结页面", async () => {
    const user = userEvent.setup();
    let finishDiscard:
      | ((outcome: Extract<UiOutcome, { type: "inventory" }>) => void)
      | undefined;
    const client = createClient({
      type: "bundleUpdateBatchPlan",
      plan: createBundleUpdateBatchPlan(),
    });
    vi.mocked(client.discardBundleUpdateBatchPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishDiscard = resolve;
        }),
    );
    render(<App client={client} />);

    const back = await screen.findByRole("button", { name: "返回清单" });
    const confirm = screen.getByRole("button", { name: "确认全部更新" });
    await user.click(back);

    expect(client.discardBundleUpdateBatchPlan).toHaveBeenCalledWith(
      "update-batch-plan-1",
    );
    expect(back).toBeDisabled();
    expect(confirm).toBeDisabled();
    expect(screen.getByText("正在清理更新预览…")).toBeInTheDocument();
    await user.click(back);
    expect(client.discardBundleUpdateBatchPlan).toHaveBeenCalledTimes(1);

    await act(async () => {
      finishDiscard?.(inventoryWithTwoAvailableUpdates());
    });
    expect(
      screen.getByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
  });

  it("completed 结果确认已读期间冻结返回，并用结果 ID acknowledge", async () => {
    const user = userEvent.setup();
    let finishAcknowledge:
      | ((outcome: Extract<UiOutcome, { type: "inventory" }>) => void)
      | undefined;
    const client = createClient({
      type: "bundleUpdateBatchResult",
      result: createBundleUpdateBatchResult(),
    });
    vi.mocked(client.acknowledgeBundleUpdateBatchResult).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishAcknowledge = resolve;
        }),
    );
    render(<App client={client} />);

    const back = await screen.findByRole("button", { name: "返回清单" });
    await user.click(back);

    expect(client.acknowledgeBundleUpdateBatchResult).toHaveBeenCalledWith(
      "update-batch-1",
    );
    expect(back).toBeDisabled();
    expect(screen.getByText("正在返回清单…")).toBeInTheDocument();
    await user.click(back);
    expect(client.acknowledgeBundleUpdateBatchResult).toHaveBeenCalledTimes(1);

    await act(async () => {
      finishAcknowledge?.(inventoryWithTwoAvailableUpdates());
    });
    expect(
      screen.getByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
  });

  it("blocked 结果只提示等待人工恢复，不能 acknowledge", async () => {
    const client = createClient({
      type: "bundleUpdateBatchResult",
      result: createBundleUpdateBatchResult({
        status: "blocked",
        items: [
          createBundleUpdateBatchResultItem({
            status: "blocked",
            errorSummary: "Bundle current 指向未知内容",
          }),
          createBundleUpdateBatchResultItem({
            id: "item-beta",
            bundleId: "bundle-beta",
            bundleDisplayName: "Beta",
            status: "notExecuted",
            errorSummary: "等待前一个 Bundle 完成人工恢复",
          }),
        ],
      }),
    });
    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", {
        name: "全部更新正在等待人工恢复",
      }),
    ).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "请在人工恢复页面处理",
    );
    expect(screen.getByRole("region", { name: "全部更新结果" })).toHaveTextContent(
      "等待人工恢复",
    );
    expect(
      screen.queryByRole("button", { name: "返回清单" }),
    ).not.toBeInTheDocument();
    expect(client.acknowledgeBundleUpdateBatchResult).not.toHaveBeenCalled();
  });

  it("选择文件夹后先显示影响预览，确认前不写入", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(createInstallPlan({
      inputPath: "/Users/test/Downloads/example",
      warnings: ["包含可执行文件，请确认来源可信"],
    }));
    render(<App client={client} />);

    await openLocalFolderPicker(user);

    expect(client.chooseFolderInstallPlan).toHaveBeenCalledTimes(1);
    expect(
      screen.getByRole("heading", { name: /确认安装.*Bundle/ }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("安装影响预览")).toHaveTextContent(
      "安装后不会自动挂载",
    );
    expect(screen.getByText(/安装开始后不能取消/)).toBeInTheDocument();
    expect(screen.getByText("包含可执行文件，请确认来源可信")).toBeInTheDocument();
    expect(client.confirmInstallPlan).not.toHaveBeenCalled();
  });

  it("多 Skill Bundle 默认全选，并把用户最终选择提交为候选 ID", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(
      createInstallPlan({
        bundleDisplayName: "superpowers",
        candidates: [
          createInstallCandidate({
            candidateId: "candidate-brainstorming",
            sourceRelativePath: "skills/brainstorming",
            skillName: "brainstorming",
          }),
          createInstallCandidate({
            candidateId: "candidate-tdd",
            sourceRelativePath: "skills/tdd",
            skillName: "tdd",
          }),
        ],
      }),
    );
    vi.mocked(client.confirmInstallPlan).mockResolvedValue(inventoryOutcome([]));
    render(<App client={client} />);

    await openLocalFolderPicker(user);
    const brainstorming = screen.getByRole("checkbox", { name: /brainstorming/ });
    const tdd = screen.getByRole("checkbox", { name: /tdd/ });
    expect(brainstorming).toBeChecked();
    expect(tdd).toBeChecked();

    await user.click(tdd);
    expect(tdd).not.toBeChecked();
    expect(screen.getByRole("alert")).toHaveTextContent(
      /部分 Skill 可能依赖.*未选择/,
    );
    await user.click(screen.getByRole("button", { name: "确认安装" }));

    expect(client.confirmInstallPlan).toHaveBeenCalledWith("plan-1", [
      "candidate-brainstorming",
    ]);
  });

  it("没有选择任何有效成员时不能确认安装", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(createInstallPlan());
    render(<App client={client} />);

    await openLocalFolderPicker(user);
    await user.click(screen.getByRole("checkbox", { name: /example/ }));

    expect(screen.getByText(/至少选择一个有效 Skill/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认安装" })).toBeDisabled();
    expect(client.confirmInstallPlan).not.toHaveBeenCalled();
  });

  it("无效候选展示具体错误且不可选择，有效候选仍可安装", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(
      createInstallPlan({
        candidates: [
          createInstallCandidate({
            candidateId: "candidate-valid",
            sourceRelativePath: "skills/valid",
            skillName: "valid",
          }),
          createInstallCandidate({
            candidateId: "candidate-broken",
            sourceRelativePath: "skills/broken",
            skillName: "broken",
            selectable: false,
            validationErrors: ["SKILL.md YAML frontmatter 无法解析"],
            defaultSelected: false,
            targetDirectory: null,
          }),
        ],
      }),
    );
    render(<App client={client} />);

    await openLocalFolderPicker(user);

    expect(screen.getByRole("checkbox", { name: /valid/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /broken/ })).toBeDisabled();
    expect(screen.getByText("SKILL.md YAML frontmatter 无法解析")).toBeInTheDocument();
  });

  it("确认期间不可取消，但可以只读浏览最近一次已提交清单", async () => {
    const user = userEvent.setup();
    let finishInstall: ((outcome: UiOutcome) => void) | undefined;
    const client = createClient(
      inventoryOutcome([
        createManagedEntry({
          id: "managed:saved",
          skillName: "saved",
          declaredName: "saved",
        }),
      ]),
    );
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(createInstallPlan());
    vi.mocked(client.confirmInstallPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishInstall = resolve;
        }),
    );
    render(<App client={client} />);
    await openLocalFolderPicker(user);
    await user.click(screen.getByRole("button", { name: "确认安装" }));

    expect(client.confirmInstallPlan).toHaveBeenCalledWith("plan-1", [
      "candidate-example",
    ]);
    expect(screen.getByRole("button", { name: "正在安全安装…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "返回" })).toBeDisabled();
    expect(screen.getByLabelText("当前操作")).toHaveTextContent(
      "正在安装 Bundle",
    );
    await user.click(screen.getByRole("button", { name: "正在安全安装…" }));
    expect(client.confirmInstallPlan).toHaveBeenCalledTimes(1);

    await user.click(
      screen.getByRole("button", { name: "浏览已提交清单" }),
    );

    expect(
      screen.getByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
    expect(screen.getByText("example-bundle: saved")).toBeInTheDocument();
    expect(screen.getByRole("searchbox", { name: "搜索 Skill" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "检查更新" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "安装 Skill" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "添加项目" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "刷新本机" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "批量挂载" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "管理挂载" })).toBeDisabled();

    await user.type(
      screen.getByRole("searchbox", { name: "搜索 Skill" }),
      "saved",
    );
    expect(screen.getByText("example-bundle: saved")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "返回当前操作" }),
    );
    expect(screen.getByLabelText("安装影响预览")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "返回" })).toBeDisabled();

    await act(async () => {
      finishInstall?.(
        inventoryOutcome([
          createEntry({
            managementKind: "skillYardManaged",
            bundleId: "bundle-1",
            bundleDisplayName: "example",
          }),
        ]),
      );
    });
    expect(screen.getByRole("region", { name: "example" })).toHaveTextContent(
      "example: example",
    );
  });

  it("确认失败后丢弃已消费 Plan 并重新读取最终状态", async () => {
    const user = userEvent.setup();
    const initial = inventoryOutcome([]);
    const recovered = inventoryOutcome([
      createEntry({
        managementKind: "skillYardManaged",
        bundleId: "bundle-1",
        bundleDisplayName: "example",
      }),
    ]);
    const client = createClient(initial);
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(createInstallPlan());
    vi.mocked(client.confirmInstallPlan).mockRejectedValue({
      code: "lifecycleError",
      message: "安装中断，已自动恢复",
    });
    vi.mocked(client.getStartupState)
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(recovered);
    render(<App client={client} />);

    await openLocalFolderPicker(user);
    await user.click(screen.getByRole("button", { name: "确认安装" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "安装中断，已自动恢复",
    );
    expect(client.getStartupState).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("button", { name: "确认安装" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "example" })).toBeInTheDocument();
  });

  it("取消原生选择器时保留 Source 页面", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([createEntry({ skillName: "preserved" })]),
    );
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(null);
    render(<App client={client} />);

    await openLocalFolderPicker(user);

    expect(
      screen.getByRole("heading", { name: "安装 Skill" }),
    ).toBeInTheDocument();
    expect(screen.getByText("anthropics/skills")).toBeInTheDocument();
    expect(client.confirmInstallPlan).not.toHaveBeenCalled();
  });

  it("取消 Project 原生选择器时保持当前清单", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([createEntry({ skillName: "preserved" })]),
    );
    vi.mocked(client.chooseAndRegisterProject).mockResolvedValue(null);
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "添加项目" }));

    expect(client.chooseAndRegisterProject).toHaveBeenCalledTimes(1);
    expect(screen.getByText("preserved")).toBeInTheDocument();
    expect(client.createMountPlan).not.toHaveBeenCalled();
  });

  it("受管 Skill 卡直接显示三个应用的真实 Mount", async () => {
    const client = createClient(
      inventoryOutcome(
        [createManagedEntry()],
        null,
        {
          mounts: [
            createMount({ id: "mount-global", scope: "global" }),
            createMount({
              id: "mount-claude-project",
              appId: "claudeCode",
              scope: "project",
              projectId: "project-1",
              projectDisplayName: "SkillYard",
              targetPath: "/tmp/SkillYard/.claude/skills/example",
            }),
            createMount({
              id: "mount-copilot-global",
              appId: "gitHubCopilot",
              targetPath: "/tmp/.copilot/skills/example",
            }),
          ],
        },
      ),
    );

    render(<App client={client} />);

    const bundle = await screen.findByRole("region", { name: "example-bundle" });
    expect(bundle).toHaveTextContent("Codex · 全局");
    expect(bundle).toHaveTextContent("Claude Code · SkillYard");
    expect(bundle).toHaveTextContent("GitHub Copilot · 全局");
    expect(within(bundle).getByRole("button", { name: "管理挂载" })).toBeEnabled();
  });

  it("挂载管理按三个固定应用分区，未检测到也允许用户选择", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome(
        [createManagedEntry()],
        null,
        {
          supportedApps: [
            { id: "codex", displayName: "Codex", detected: true },
            { id: "claudeCode", displayName: "Claude Code", detected: true },
            {
              id: "gitHubCopilot",
              displayName: "GitHub Copilot",
              detected: false,
            },
          ],
        },
      ),
    );
    vi.mocked(client.createMountPlan).mockResolvedValue(
      createMountPlan({
        appId: "gitHubCopilot",
        targetPath: "/tmp/.copilot/skills/example",
      }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "管理挂载" }));

    expect(screen.getByRole("region", { name: "Codex 挂载" })).toHaveTextContent(
      "已检测到",
    );
    expect(
      screen.getByRole("region", { name: "GitHub Copilot 挂载" }),
    ).toHaveTextContent("未检测到");
    const copilotGlobal = screen.getByRole("button", {
      name: "挂载到 GitHub Copilot 全局",
    });
    expect(copilotGlobal).toBeEnabled();
    await user.click(copilotGlobal);

    expect(client.createMountPlan).toHaveBeenCalledWith(
      "member-1",
      "gitHubCopilot",
      "global",
      null,
    );
    expect(
      screen.getByRole("heading", { name: "确认创建 GitHub Copilot 挂载" }),
    ).toBeInTheDocument();
  });

  it("Claude Code project Mount 明确提示 Copilot 交叉可见性", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome(
        [createManagedEntry()],
        null,
        {
          projects: [
            {
              id: "project-1",
              displayName: "SkillYard",
              rootPath: "/tmp/SkillYard",
            },
          ],
        },
      ),
    );
    vi.mocked(client.createMountPlan).mockResolvedValue(
      createMountPlan({
        appId: "claudeCode",
        scope: "project",
        projectId: "project-1",
        projectDisplayName: "SkillYard",
        targetPath: "/tmp/SkillYard/.claude/skills/example",
      }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "管理挂载" }));
    await user.click(
      screen.getByRole("button", {
        name: "挂载到 Claude Code 项目 SkillYard",
      }),
    );

    expect(client.createMountPlan).toHaveBeenCalledWith(
      "member-1",
      "claudeCode",
      "project",
      "project-1",
    );
    expect(screen.getByLabelText("挂载影响预览")).toHaveTextContent(
      "GitHub Copilot 也可能读取",
    );
  });

  it("创建 global Mount 前先生成并确认精确 Plan", async () => {
    const user = userEvent.setup();
    let finishMount: ((outcome: UiOutcome) => void) | undefined;
    const initial = inventoryOutcome([createManagedEntry()]);
    const mounted = inventoryOutcome(
      [createManagedEntry()],
      null,
      { mounts: [createMount()] },
    );
    const client = createClient(initial);
    vi.mocked(client.createMountPlan).mockResolvedValue(createMountPlan());
    vi.mocked(client.confirmMountPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishMount = resolve;
        }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "管理挂载" }));
    expect(screen.getByRole("button", { name: "返回添加项目" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "挂载到 Codex 全局" }));

    expect(client.createMountPlan).toHaveBeenCalledWith(
      "member-1",
      "codex",
      "global",
      null,
    );
    expect(
      screen.getByRole("heading", { name: "确认创建 Codex 挂载" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/确认开始后不能取消/)).toBeInTheDocument();
    expect(client.confirmMountPlan).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "确认创建" }));

    expect(client.confirmMountPlan).toHaveBeenCalledWith("mount-plan-1");
    expect(screen.getByRole("button", { name: "正在安全创建…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "返回" })).toBeDisabled();

    await act(async () => {
      finishMount?.(mounted);
    });

    const bundle = screen.getByRole("region", { name: "example-bundle" });
    expect(bundle).toHaveTextContent("Codex · 全局");
  });

  it("project Mount Plan 只接受已登记 Project 的稳定 ID", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome(
        [createManagedEntry()],
        null,
        {
          projects: [
            {
              id: "project-1",
              displayName: "SkillYard",
              rootPath: "/tmp/SkillYard",
            },
          ],
        },
      ),
    );
    vi.mocked(client.createMountPlan).mockResolvedValue(
      createMountPlan({
        scope: "project",
        projectId: "project-1",
        projectDisplayName: "SkillYard",
        targetPath: "/tmp/SkillYard/.codex/skills/example",
      }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "管理挂载" }));
    await user.click(
      screen.getByRole("button", { name: "挂载到 Codex 项目 SkillYard" }),
    );

    expect(client.createMountPlan).toHaveBeenCalledWith(
      "member-1",
      "codex",
      "project",
      "project-1",
    );
    expect(screen.getByLabelText("挂载影响预览")).toHaveTextContent(
      "/tmp/SkillYard/.codex/skills/example",
    );
  });

  it("正确软链接的创建 Plan 明确表示只登记关系", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([createManagedEntry()]));
    vi.mocked(client.createMountPlan).mockResolvedValue(
      createMountPlan({ targetHealth: "healthy" }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "管理挂载" }));
    await user.click(screen.getByRole("button", { name: "挂载到 Codex 全局" }));

    expect(screen.getByLabelText("挂载影响预览")).toHaveTextContent(
      "软链接已经正确存在，将只登记为 SkillYard Mount",
    );
    expect(screen.getByLabelText("挂载影响预览")).toHaveTextContent(
      "现有软链接不会被改写",
    );
  });

  it("Mount Plan 冲突不丢弃已加载清单", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([createManagedEntry()]));
    vi.mocked(client.createMountPlan).mockRejectedValue({
      code: "mountConflict",
      message: "目标路径已被其他内容占用",
    });
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "管理挂载" }));
    await user.click(screen.getByRole("button", { name: "挂载到 Codex 全局" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "目标路径已被其他内容占用",
    );
    expect(client.getStartupState).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "返回添加项目" }));
    expect(screen.getByText("example-bundle: example")).toBeInTheDocument();
  });

  it("移除 Mount 前显示确认页，确认后只提交 Plan ID", async () => {
    const user = userEvent.setup();
    const initial = inventoryOutcome(
      [createManagedEntry()],
      null,
      { mounts: [createMount()] },
    );
    const client = createClient(initial);
    vi.mocked(client.createRemoveMountPlan).mockResolvedValue(
      createMountPlan({ operation: "remove" }),
    );
    vi.mocked(client.confirmMountPlan).mockResolvedValue(
      inventoryOutcome([createManagedEntry()]),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "管理挂载" }));
    await user.click(
      screen.getByRole("button", { name: "移除 Codex 全局挂载" }),
    );

    expect(client.createRemoveMountPlan).toHaveBeenCalledWith("mount-1");
    expect(
      screen.getByRole("heading", { name: "确认移除 Codex 挂载" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/不会删除 Skill 或 Bundle/)).toBeInTheDocument();
    expect(client.confirmMountPlan).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "确认移除" }));

    expect(client.confirmMountPlan).toHaveBeenCalledWith("mount-plan-1");
    expect(screen.queryByText("Codex · 全局")).not.toBeInTheDocument();
  });

  it("缺失 Mount 可以生成独立修复 Plan，冲突 Mount 不提供修复", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome(
        [createManagedEntry()],
        null,
        {
          mounts: [
            createMount({ id: "mount-missing", health: "missing" }),
            createMount({
              id: "mount-conflict",
              appId: "claudeCode",
              targetPath: "/tmp/.claude/skills/example",
              health: "conflict",
            }),
          ],
        },
      ),
    );
    vi.mocked(client.createRepairMountPlan).mockResolvedValue(
      createMountPlan({
        purpose: "repair",
        mountId: "mount-missing",
        targetHealth: "missing",
      }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "管理挂载" }));
    expect(
      screen.queryByRole("button", {
        name: "修复 Claude Code 全局挂载",
      }),
    ).not.toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "修复 Codex 全局挂载" }),
    );

    expect(client.createRepairMountPlan).toHaveBeenCalledWith("mount-missing");
    expect(
      screen.getByRole("heading", { name: "确认修复 Codex 挂载" }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("挂载影响预览")).toHaveTextContent(
      "将重新创建指向中央主副本的软链接",
    );
  });

  it("修复 Plan 发现外部占用后刷新缓存状态并隐藏修复入口", async () => {
    const user = userEvent.setup();
    const missing = createMount({ id: "mount-missing", health: "missing" });
    const client = createClient(
      inventoryOutcome([createManagedEntry()], null, { mounts: [missing] }),
    );
    vi.mocked(client.createRepairMountPlan).mockRejectedValue({
      code: "mountConflict",
      message: "Mount 目标已经被其他内容占用",
    });
    vi.mocked(client.refreshLocalInventory).mockResolvedValue(
      inventoryOutcome([createManagedEntry()], null, {
        mounts: [{ ...missing, health: "conflict" }],
      }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "管理挂载" }));
    await user.click(
      screen.getByRole("button", { name: "修复 Codex 全局挂载" }),
    );

    expect(client.refreshLocalInventory).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Mount 目标已经被其他内容占用",
    );
    expect(screen.getByText(/目标路径无法安全确认/)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "修复 Codex 全局挂载" }),
    ).not.toBeInTheDocument();
  });

  it("Mount 确认失败后丢弃 Plan 并重新读取最终清单", async () => {
    const user = userEvent.setup();
    const initial = inventoryOutcome([createManagedEntry()]);
    const recovered = inventoryOutcome([createManagedEntry()]);
    const client = createClient(initial);
    vi.mocked(client.createMountPlan).mockResolvedValue(createMountPlan());
    vi.mocked(client.confirmMountPlan).mockRejectedValue({
      code: "mountConflict",
      message: "目标路径已被其他内容占用",
    });
    vi.mocked(client.getStartupState)
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(recovered);
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "管理挂载" }));
    await user.click(screen.getByRole("button", { name: "挂载到 Codex 全局" }));
    await user.click(screen.getByRole("button", { name: "确认创建" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "目标路径已被其他内容占用",
    );
    expect(client.getStartupState).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("button", { name: "确认创建" })).not.toBeInTheDocument();
    expect(screen.getByText("example-bundle: example")).toBeInTheDocument();
  });

  it("只给真实受管 Bundle 提供批量挂载入口", async () => {
    const client = createClient(
      inventoryOutcome([
        createManagedEntry({
          id: "managed:unassigned",
          memberId: "member-unassigned",
          bundleId: null,
          bundleDisplayName: null,
          skillName: "unassigned",
        }),
      ]),
    );
    render(<App client={client} />);

    const fallbackGroup = await screen.findByRole("region", {
      name: "本地 Bundle",
    });
    expect(
      within(fallbackGroup).queryByRole("button", { name: "批量挂载" }),
    ).not.toBeInTheDocument();
  });

  it("从完整 Inventory 取出 Bundle 全成员并生成全成员乘目标请求", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome(
        [
          createManagedEntry({
            id: "managed:alpha",
            memberId: "member-alpha",
            skillName: "alpha",
          }),
          createManagedEntry({
            id: "managed:beta",
            memberId: "member-beta",
            skillName: "beta",
          }),
        ],
        null,
        {
          supportedApps: [
            { id: "codex", displayName: "Codex", detected: true },
            {
              id: "claudeCode",
              displayName: "Claude Code",
              detected: false,
            },
            {
              id: "gitHubCopilot",
              displayName: "GitHub Copilot",
              detected: null,
            },
          ],
        },
      ),
    );
    vi.mocked(client.createBatchMountPlan).mockResolvedValue(
      createBatchPlan({
        items: [
          createBatchPlanItem({
            id: "batch-alpha-codex",
            memberId: "member-alpha",
            skillName: "alpha",
          }),
          createBatchPlanItem({
            id: "batch-beta-codex",
            memberId: "member-beta",
            skillName: "beta",
          }),
        ],
      }),
    );
    render(<App client={client} />);

    await screen.findByRole("heading", { name: "Skill 清单" });
    await user.type(
      screen.getByRole("searchbox", { name: "搜索 Skill" }),
      "alpha",
    );
    const visibleBundle = screen.getByRole("region", { name: "example-bundle" });
    expect(
      within(visibleBundle).queryByText("example-bundle: beta"),
    ).not.toBeInTheDocument();
    await user.click(
      within(visibleBundle).getByRole("button", { name: "批量挂载" }),
    );

    expect(
      screen.getByRole("heading", { name: "批量挂载 example-bundle" }),
    ).toBeInTheDocument();
    const members = screen.getByRole("region", { name: "Bundle 全部成员" });
    expect(members).toHaveTextContent("alpha");
    expect(members).toHaveTextContent("beta");
    expect(
      screen.getByText("本 Bundle 的 2 个 Skill 将全部参与"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("checkbox", { name: "Codex 全局" }),
    ).not.toBeChecked();
    expect(
      screen.getByRole("region", { name: "Codex 批量挂载目标" }),
    ).toHaveTextContent("已检测到");
    expect(
      screen.getByRole("region", { name: "Claude Code 批量挂载目标" }),
    ).toHaveTextContent("未检测到");
    expect(
      screen.getByRole("region", { name: "GitHub Copilot 批量挂载目标" }),
    ).toHaveTextContent("尚未检测");

    await user.click(screen.getByRole("checkbox", { name: "Codex 全局" }));
    await user.click(screen.getByRole("button", { name: "生成影响预览" }));

    expect(client.createBatchMountPlan).toHaveBeenCalledWith("bundle-1", [
      {
        memberId: "member-alpha",
        appId: "codex",
        scope: "global",
        projectId: null,
      },
      {
        memberId: "member-beta",
        appId: "codex",
        scope: "global",
        projectId: null,
      },
    ]);
  });

  it("同一应用的 global 与 project 目标互斥，同时允许多个 Project", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([createManagedEntry()], null, {
        projects: [
          { id: "project-1", displayName: "Alpha", rootPath: "/tmp/alpha" },
          { id: "project-2", displayName: "Beta", rootPath: "/tmp/beta" },
        ],
      }),
    );
    vi.mocked(client.createBatchMountPlan).mockResolvedValue(createBatchPlan());
    render(<App client={client} />);

    const bundle = await screen.findByRole("region", { name: "example-bundle" });
    await user.click(within(bundle).getByRole("button", { name: "批量挂载" }));
    const global = screen.getByRole("checkbox", { name: "Claude Code 全局" });
    const alpha = screen.getByRole("checkbox", {
      name: "Claude Code 项目 Alpha",
    });
    const beta = screen.getByRole("checkbox", {
      name: "Claude Code 项目 Beta",
    });

    await user.click(global);
    expect(global).toBeChecked();
    await user.click(alpha);
    await user.click(beta);
    expect(global).not.toBeChecked();
    expect(alpha).toBeChecked();
    expect(beta).toBeChecked();
    expect(screen.getByText(/GitHub Copilot 也可能读取/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "生成影响预览" }));
    expect(client.createBatchMountPlan).toHaveBeenCalledWith("bundle-1", [
      {
        memberId: "member-1",
        appId: "claudeCode",
        scope: "project",
        projectId: "project-1",
      },
      {
        memberId: "member-1",
        appId: "claudeCode",
        scope: "project",
        projectId: "project-2",
      },
    ]);
  });

  it("Batch Plan 遵循后端默认选择，并禁用冲突和已挂载项", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([createManagedEntry()]));
    vi.mocked(client.createBatchMountPlan).mockResolvedValue(
      createBatchPlan({
        items: [
          createBatchPlanItem(),
          createBatchPlanItem({
            id: "batch-ready-not-default",
            skillName: "optional",
            selectable: true,
            defaultSelected: false,
          }),
          createBatchPlanItem({
            id: "batch-path-conflict",
            appId: "claudeCode",
            disposition: "pathConflict",
            selectable: false,
            defaultSelected: false,
            conflictReason: "目标路径已被其他内容占用",
          }),
          createBatchPlanItem({
            id: "batch-scope-conflict",
            appId: "gitHubCopilot",
            disposition: "scopeConflict",
            selectable: false,
            defaultSelected: false,
            conflictReason: "该应用已经存在 global Mount",
          }),
          createBatchPlanItem({
            id: "batch-already-mounted",
            scope: "project",
            projectId: "project-1",
            projectDisplayName: "Alpha",
            disposition: "alreadyMounted",
            selectable: false,
            defaultSelected: false,
            conflictReason: null,
          }),
        ],
      }),
    );
    render(<App client={client} />);

    const bundle = await screen.findByRole("region", { name: "example-bundle" });
    await user.click(within(bundle).getByRole("button", { name: "批量挂载" }));
    await user.click(screen.getByRole("checkbox", { name: "Codex 全局" }));
    await user.click(screen.getByRole("button", { name: "生成影响预览" }));

    expect(
      screen.getByRole("heading", { name: "确认 example-bundle 批量挂载" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/全部完成或全部撤销/)).toBeInTheDocument();
    const ready = screen.getByRole("checkbox", {
      name: "example · Codex · 全局",
    });
    expect(ready).toBeChecked();
    const readyNotDefault = screen.getByRole("checkbox", {
      name: "optional · Codex · 全局",
    });
    expect(readyNotDefault).toBeEnabled();
    expect(readyNotDefault).not.toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: "example · Claude Code · 全局" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("checkbox", { name: "example · GitHub Copilot · 全局" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("checkbox", { name: "example · Codex · 项目 Alpha" }),
    ).toBeDisabled();
    expect(screen.getByText("目标路径已被其他内容占用")).toBeInTheDocument();
    expect(screen.getByText("该应用已经存在 global Mount")).toBeInTheDocument();
    expect(screen.getByText("已经挂载，无需重复创建")).toBeInTheDocument();

    await user.click(ready);
    expect(screen.getByText("至少保留一个可挂载项")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认批量挂载" })).toBeDisabled();
  });

  it("确认 Batch Mount 期间不能返回或重复，成功后回到清单", async () => {
    const user = userEvent.setup();
    let finishBatchMount: ((outcome: UiOutcome) => void) | undefined;
    const initial = inventoryOutcome([createManagedEntry()]);
    const mounted = inventoryOutcome([createManagedEntry()], null, {
      mounts: [createMount()],
    });
    const client = createClient(initial);
    vi.mocked(client.createBatchMountPlan).mockResolvedValue(createBatchPlan());
    vi.mocked(client.confirmBatchMountPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishBatchMount = resolve;
        }),
    );
    render(<App client={client} />);

    const bundle = await screen.findByRole("region", { name: "example-bundle" });
    await user.click(within(bundle).getByRole("button", { name: "批量挂载" }));
    await user.click(screen.getByRole("checkbox", { name: "Codex 全局" }));
    await user.click(screen.getByRole("button", { name: "生成影响预览" }));
    await user.click(screen.getByRole("button", { name: "确认批量挂载" }));

    expect(client.confirmBatchMountPlan).toHaveBeenCalledWith("batch-plan-1", [
      "batch-item-1",
    ]);
    expect(screen.getByRole("button", { name: "正在安全挂载…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "返回" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "正在安全挂载…" }));
    expect(client.confirmBatchMountPlan).toHaveBeenCalledTimes(1);

    await act(async () => {
      finishBatchMount?.(mounted);
    });
    expect(
      screen.getByRole("region", { name: "example-bundle" }),
    ).toHaveTextContent("Codex · 全局");
  });

  it("Batch Mount 确认失败后丢弃 Plan 并读取真实状态", async () => {
    const user = userEvent.setup();
    const initial = inventoryOutcome([createManagedEntry()]);
    const recovered = inventoryOutcome([createManagedEntry()]);
    const client = createClient(initial);
    vi.mocked(client.createBatchMountPlan).mockResolvedValue(createBatchPlan());
    vi.mocked(client.confirmBatchMountPlan).mockRejectedValue({
      code: "batchMountConflict",
      message: "批量挂载中断，已撤销全部改动",
    });
    vi.mocked(client.getStartupState)
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(recovered);
    render(<App client={client} />);

    const bundle = await screen.findByRole("region", { name: "example-bundle" });
    await user.click(within(bundle).getByRole("button", { name: "批量挂载" }));
    await user.click(screen.getByRole("checkbox", { name: "Codex 全局" }));
    await user.click(screen.getByRole("button", { name: "生成影响预览" }));
    await user.click(screen.getByRole("button", { name: "确认批量挂载" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "批量挂载中断，已撤销全部改动",
    );
    expect(client.getStartupState).toHaveBeenCalledTimes(2);
    expect(
      screen.queryByRole("button", { name: "确认批量挂载" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("region", { name: "example-bundle" }),
    ).toBeInTheDocument();
  });

  it("空清单仍然是完成状态", async () => {
    const client = createClient(inventoryOutcome([]));

    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", { name: "未发现 Skill" }),
    ).toBeInTheDocument();
    expect(screen.getByText("尚未执行本机刷新")).toBeInTheDocument();
  });

  it("准确展示四种管理状态，并只把受管 Skill 放入 Bundle 分组", async () => {
    const client = createClient(
      inventoryOutcome([
        createEntry({
          id: "managed-one",
          skillName: "qa",
          managementKind: "skillYardManaged",
          bundleId: "bundle-1",
          bundleDisplayName: "mattpocock/skills",
          sourceDisplayName: "github.com/mattpocock/skills",
        }),
        createEntry({
          id: "managed-two",
          skillName: "tdd",
          managementKind: "skillYardManaged",
          bundleId: "bundle-1",
          bundleDisplayName: "mattpocock/skills",
        }),
        createEntry({
          id: "managed-same-name",
          skillName: "research",
          managementKind: "skillYardManaged",
          bundleId: "bundle-2",
          bundleDisplayName: "mattpocock/skills",
        }),
        createEntry({ id: "takeover", skillName: "local-copy" }),
        createEntry({
          id: "agent",
          skillName: "plugin-skill",
          managementKind: "agentManaged",
          observedBy: ["codex"],
        }),
        createEntry({
          id: "project",
          skillName: "repo-skill",
          locationKind: "appProject",
          rootKey: "codexProject",
          projectId: "project-1",
          managementKind: "projectManaged",
          projectDisplayName: "SkillYard",
        }),
      ]),
    );

    render(<App client={client} />);

    const bundles = await screen.findAllByRole("region", {
      name: "mattpocock/skills",
    });
    expect(bundles).toHaveLength(2);
    expect(bundles.some((bundle) => within(bundle).queryByText("mattpocock/skills: qa")))
      .toBe(true);
    expect(bundles.some((bundle) => within(bundle).queryByText("mattpocock/skills: tdd")))
      .toBe(true);
    expect(
      bundles.some((bundle) =>
        within(bundle).queryByText("mattpocock/skills: research"),
      ),
    ).toBe(true);
    expect(screen.getByRole("region", { name: "待接管" })).toHaveTextContent(
      "local-copy",
    );
    expect(
      screen.getByRole("region", { name: "Agent 应用管理" }),
    ).toHaveTextContent("请前往 Codex 管理此 Skill");
    expect(
      screen.getByRole("region", { name: "项目仓库管理" }),
    ).toHaveTextContent("请在 SkillYard 中管理此 Skill");
  });

  it("搜索和管理状态筛选只改变当前显示，不调用任何生命周期命令", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([
        createEntry({ id: "takeover", skillName: "local-copy" }),
        createEntry({
          id: "agent",
          skillName: "agent-only",
          managementKind: "agentManaged",
        }),
      ]),
    );
    render(<App client={client} />);

    await screen.findByRole("heading", { name: "Skill 清单" });
    await user.type(screen.getByRole("searchbox", { name: "搜索 Skill" }), "agent");
    expect(screen.getByText("agent-only")).toBeInTheDocument();
    expect(screen.queryByText("local-copy")).not.toBeInTheDocument();

    await user.clear(screen.getByRole("searchbox", { name: "搜索 Skill" }));
    await user.click(screen.getByRole("button", { name: "待接管" }));
    expect(screen.getByText("local-copy")).toBeInTheDocument();
    expect(screen.queryByText("agent-only")).not.toBeInTheDocument();
    expect(client.startInitialScan).not.toHaveBeenCalled();
    expect(client.refreshLocalInventory).not.toHaveBeenCalled();
  });

  it("只在点击后刷新，进行中保留旧列表并阻止重复提交", async () => {
    const user = userEvent.setup();
    let finishRefresh: ((outcome: UiOutcome) => void) | undefined;
    const client = createClient(
      inventoryOutcome([createEntry({ skillName: "old-skill" })]),
    );
    vi.mocked(client.refreshLocalInventory).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishRefresh = resolve;
        }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "刷新本机" }));

    expect(client.refreshLocalInventory).toHaveBeenCalledTimes(1);
    expect(screen.getByText("old-skill")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "正在刷新本机…" }),
    ).toBeDisabled();
    expect(screen.getByRole("button", { name: "安装 Skill" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "添加项目" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "接管 old-skill" })).toBeDisabled();
    expect(screen.getByRole("searchbox", { name: "搜索 Skill" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "正在刷新本机…" }));
    expect(client.refreshLocalInventory).toHaveBeenCalledTimes(1);

    await act(async () => {
      finishRefresh?.(
        inventoryOutcome([createEntry({ skillName: "new-skill" })], {
          completedAt: 1_753_000_000_000,
          added: 1,
          changed: 0,
          removed: 1,
        }),
      );
    });
    expect(screen.getByText("new-skill")).toBeInTheDocument();
    expect(screen.queryByText("old-skill")).not.toBeInTheDocument();
    expect(screen.getByLabelText("最近刷新结果")).toHaveTextContent(
      "新增 1 · 变化 0 · 移除 1",
    );
  });

  it("刷新失败时保留旧清单并以内联错误解释", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([createEntry({ skillName: "preserved-skill" })]),
    );
    vi.mocked(client.refreshLocalInventory).mockRejectedValue({
      code: "storageError",
      message: "SQLite 暂时不可写",
    });
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "刷新本机" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("SQLite 暂时不可写");
    expect(screen.getByText("preserved-skill")).toBeInTheDocument();
  });

  it("部分目录失败时解释保留的是上次结果", async () => {
    const client = createClient({
      ...inventoryOutcome([
        createEntry({ skillName: "stale-skill", stale: true }),
      ]),
      scanIssues: [
        {
          rootId: "global:codex_global",
          rootKey: "codexGlobal",
          projectId: null,
          path: "/tmp/.codex/skills",
          code: "rootNotDirectory",
          message: "扫描根目录不是文件夹",
        },
      ],
    });
    render(<App client={client} />);

    expect(await screen.findByLabelText("刷新告警")).toHaveTextContent(
      "没有把它们当作已删除",
    );
    expect(screen.getByText("上次结果")).toBeInTheDocument();
  });

  it("同一种项目扫描根的问题按 rootId 同时显示", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    const client = createClient({
      ...inventoryOutcome([]),
      scanIssues: [
        {
          rootId: "project:project-1:codex_project",
          rootKey: "codexProject",
          projectId: "project-1",
          path: "/tmp/first/.codex/skills",
          code: "readRoot",
          message: "无法读取第一个项目扫描根",
        },
        {
          rootId: "project:project-2:codex_project",
          rootKey: "codexProject",
          projectId: "project-2",
          path: "/tmp/second/.codex/skills",
          code: "readRoot",
          message: "无法读取第二个项目扫描根",
        },
      ],
    });

    try {
      render(<App client={client} />);

      const warning = await screen.findByLabelText("刷新告警");
      expect(warning).toHaveTextContent("/tmp/first/.codex/skills");
      expect(warning).toHaveTextContent("/tmp/second/.codex/skills");
      // 两个 Project 共用 rootKey 时，也不能触发 React 重复 key 告警。
      expect(
        consoleError.mock.calls.some((call) =>
          call.some(
            (value) =>
              typeof value === "string" && value.includes("same key"),
          ),
        ),
      ).toBe(false);
    } finally {
      consoleError.mockRestore();
    }
  });

  it("人工恢复只提供说明和固定 Central Store 入口，同时保留其他清单", async () => {
    const user = userEvent.setup();
    const client = createClient({
      ...inventoryOutcome([createEntry({ skillName: "still-readable" })]),
      recoveryIssues: [
        {
          id: "transaction-1",
          bundleDisplayName: "damaged-bundle",
          message: "current 指向未知状态",
        },
      ],
    });
    render(<App client={client} />);

    const recovery = await screen.findByRole("region", { name: "需要人工恢复" });
    expect(recovery).toHaveTextContent("damaged-bundle");
    expect(recovery).toHaveTextContent("只停止修改相关 Bundle");
    expect(screen.getByText("still-readable")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "安装 Skill" })).toBeEnabled();

    await user.click(
      screen.getByRole("button", {
        name: "查看 damaged-bundle 的恢复说明",
      }),
    );

    expect(
      screen.getByRole("heading", { name: "需要人工检查文件" }),
    ).toBeInTheDocument();
    expect(screen.getByText("current 指向未知状态")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /强制|删除|解除/ })).toBeNull();

    await user.click(
      screen.getByRole("button", { name: "打开 Central Store" }),
    );
    expect(client.openCentralStore).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: "返回清单" }));
    expect(screen.getByText("still-readable")).toBeInTheDocument();
  });
});

describe("接管已有 Skill", () => {
  it("从待接管卡片进入选择页，并且不会仅凭同名自动合并副本", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([
        createEntry({ id: "origin-codex", skillRoot: "/tmp/codex/example" }),
        createEntry({
          id: "origin-claude",
          skillRoot: "/tmp/claude/example",
          observedBy: ["claudeCode"],
          rootKey: "claudeCodeGlobal",
        }),
      ]),
    );
    render(<App client={client} />);

    const buttons = await screen.findAllByRole("button", {
      name: "接管 example",
    });
    await user.click(buttons[0]);

    expect(
      screen.getByRole("heading", { name: "选择要接管的 example" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("checkbox", {
        name: "确认同一 Skill：/tmp/codex/example",
      }),
    ).toBeChecked();
    expect(
      screen.getByRole("checkbox", {
        name: "确认同一 Skill：/tmp/claude/example",
      }),
    ).not.toBeChecked();
    expect(client.createTakeoverPlan).not.toHaveBeenCalled();
  });

  it("完整收集唯一内容、scope 取舍和共享目录目标后才生成 Plan", async () => {
    const user = userEvent.setup();
    const origins = [
      createEntry({
        id: "origin-global",
        skillRoot: "/tmp/home/.codex/skills/example",
        observedFingerprint: "global-content",
      }),
      createEntry({
        id: "origin-project",
        skillRoot: "/tmp/project/.codex/skills/example",
        locationKind: "appProject",
        observedFingerprint: "project-content",
        rootKey: "codexProject",
        projectId: "project-1",
        projectDisplayName: "demo",
      }),
      createEntry({
        id: "origin-shared",
        skillRoot: "/tmp/home/.agents/skills/example",
        locationKind: "sharedReadOnly",
        observedBy: ["claudeCode"],
        observedFingerprint: "project-content",
        rootKey: "sharedAgents",
      }),
    ];
    const client = createClient(inventoryOutcome(origins));
    vi.mocked(client.createTakeoverPlan).mockResolvedValue(createTakeoverPlan());
    render(<App client={client} />);

    const buttons = await screen.findAllByRole("button", {
      name: "接管 example",
    });
    await user.click(buttons[0]);
    await user.click(
      screen.getByRole("checkbox", {
        name: "确认同一 Skill：/tmp/project/.codex/skills/example",
      }),
    );
    await user.click(
      screen.getByRole("checkbox", {
        name: "确认同一 Skill：/tmp/home/.agents/skills/example",
      }),
    );

    expect(screen.getByText(/请选择唯一一份内容/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "生成影响预览" })).toBeDisabled();
    await user.click(
      screen.getByRole("radio", {
        name: "使用 /tmp/project/.codex/skills/example 作为主副本",
      }),
    );
    await user.click(
      screen.getByRole("checkbox", {
        name: "保留使用位置：/tmp/home/.codex/skills/example",
      }),
    );
    expect(screen.getByText(/共享目录必须选择至少一个应用/)).toBeInTheDocument();
    await user.click(
      screen.getByRole("checkbox", {
        name: "将 /tmp/home/.agents/skills/example 挂载到 Claude Code",
      }),
    );
    await user.click(screen.getByRole("button", { name: "生成影响预览" }));

    expect(client.createTakeoverPlan).toHaveBeenCalledWith({
      observationIds: ["origin-global", "origin-project", "origin-shared"],
      selectedObservationId: "origin-project",
      preservedObservationIds: ["origin-project"],
      sharedTargets: [
        { sharedObservationId: "origin-shared", appId: "claudeCode" },
      ],
    });
  });

  it("共享目录选择已有应用位置时复用同一最终 Mount，不生成重复目标", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([
        createEntry({
          id: "origin-codex",
          skillRoot: "/tmp/home/.codex/skills/example",
        }),
        createEntry({
          id: "origin-shared",
          skillRoot: "/tmp/home/.agents/skills/example",
          locationKind: "sharedReadOnly",
          rootKey: "sharedAgents",
        }),
      ]),
    );
    vi.mocked(client.createTakeoverPlan).mockResolvedValue(createTakeoverPlan());
    render(<App client={client} />);

    const buttons = await screen.findAllByRole("button", {
      name: "接管 example",
    });
    await user.click(buttons[0]);
    await user.click(
      screen.getByRole("checkbox", {
        name: "确认同一 Skill：/tmp/home/.agents/skills/example",
      }),
    );
    await user.click(
      screen.getByRole("checkbox", {
        name: "将 /tmp/home/.agents/skills/example 挂载到 Codex",
      }),
    );
    await user.click(screen.getByRole("button", { name: "生成影响预览" }));

    expect(client.createTakeoverPlan).toHaveBeenCalledWith({
      observationIds: ["origin-codex", "origin-shared"],
      selectedObservationId: "origin-codex",
      preservedObservationIds: ["origin-codex"],
      sharedTargets: [],
    });
  });

  it("移除已选主副本后，剩余内容仍不同时必须重新明确选择", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([
        createEntry({
          id: "origin-a",
          skillRoot: "/tmp/a/example",
          observedFingerprint: "content-a",
        }),
        createEntry({
          id: "origin-b",
          skillRoot: "/tmp/b/example",
          observedFingerprint: "content-b",
          observedBy: ["claudeCode"],
          rootKey: "claudeCodeGlobal",
        }),
        createEntry({
          id: "origin-c",
          skillRoot: "/tmp/c/example",
          observedFingerprint: "content-c",
          observedBy: ["gitHubCopilot"],
          rootKey: "gitHubCopilotGlobal",
        }),
      ]),
    );
    render(<App client={client} />);

    const buttons = await screen.findAllByRole("button", {
      name: "接管 example",
    });
    await user.click(buttons[0]);
    await user.click(
      screen.getByRole("checkbox", {
        name: "确认同一 Skill：/tmp/b/example",
      }),
    );
    await user.click(
      screen.getByRole("checkbox", {
        name: "确认同一 Skill：/tmp/c/example",
      }),
    );
    await user.click(
      screen.getByRole("radio", {
        name: "使用 /tmp/b/example 作为主副本",
      }),
    );
    await user.click(
      screen.getByRole("checkbox", {
        name: "确认同一 Skill：/tmp/b/example",
      }),
    );

    expect(screen.getByText(/请选择唯一一份内容/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "生成影响预览" })).toBeDisabled();
    expect(client.createTakeoverPlan).not.toHaveBeenCalled();
  });

  it("展示后端封存的影响预览，确认期间不可取消，成功后进入受管 Bundle", async () => {
    const user = userEvent.setup();
    let finishTakeover: ((outcome: UiOutcome) => void) | undefined;
    const initial = inventoryOutcome([createEntry({ id: "origin-1" })]);
    const client = createClient(initial);
    vi.mocked(client.createTakeoverPlan).mockResolvedValue(createTakeoverPlan());
    vi.mocked(client.confirmTakeoverPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishTakeover = resolve;
        }),
    );
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", { name: "接管 example" }),
    );
    await user.click(screen.getByRole("button", { name: "生成影响预览" }));

    const preview = screen.getByRole("region", { name: "接管影响预览" });
    expect(preview).toHaveTextContent("example-bundle");
    expect(preview).toHaveTextContent("没有更新来源");
    expect(preview).toHaveTextContent("/tmp/example");
    expect(preview).toHaveTextContent("/tmp/.codex/skills/example");
    expect(client.confirmTakeoverPlan).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "确认接管" }));
    expect(client.confirmTakeoverPlan).toHaveBeenCalledWith("takeover-plan-1");
    expect(screen.getByRole("button", { name: "正在安全接管…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "返回" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "正在安全接管…" }));
    expect(client.confirmTakeoverPlan).toHaveBeenCalledTimes(1);

    await act(async () => {
      finishTakeover?.(
        inventoryOutcome([
          createManagedEntry({ bundleDisplayName: "example-bundle" }),
        ]),
      );
    });
    expect(
      screen.getByRole("region", { name: "example-bundle" }),
    ).toBeInTheDocument();
  });

  it("确认失败后丢弃旧 Plan，并从持久状态重新读取 Inventory", async () => {
    const user = userEvent.setup();
    const initial = inventoryOutcome([createEntry({ id: "origin-1" })]);
    const recovered = inventoryOutcome([createEntry({ id: "origin-1" })]);
    const client = createClient(initial);
    vi.mocked(client.createTakeoverPlan).mockResolvedValue(createTakeoverPlan());
    vi.mocked(client.confirmTakeoverPlan).mockRejectedValue({
      code: "takeoverError",
      message: "接管中断，已恢复原安装",
    });
    vi.mocked(client.getStartupState)
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(recovered);
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", { name: "接管 example" }),
    );
    await user.click(screen.getByRole("button", { name: "生成影响预览" }));
    await user.click(screen.getByRole("button", { name: "确认接管" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "接管中断，已恢复原安装",
    );
    expect(client.getStartupState).toHaveBeenCalledTimes(2);
    expect(
      screen.queryByRole("button", { name: "确认接管" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "接管 example" }),
    ).toBeInTheDocument();
  });
});

describe("GitHub Source 安装", () => {
  it("按完整来源分组展示 skills.sh 结果，不支持的来源不能进入安装", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([createEntry()]));
    vi.mocked(client.searchSkillsSh).mockResolvedValue({
      type: "skillsShSearch",
      query: "react",
      sources: [
        {
          sourceInput: "vercel-labs/agent-skills",
          supported: true,
          members: [
            {
              skillId: "react-best-practices",
              name: "React Best Practices",
              installs: 20,
            },
            {
              skillId: "react-native",
              name: "React Native",
              installs: 10,
            },
          ],
        },
        {
          sourceInput: "react.dev",
          supported: false,
          members: [{ skillId: "react", name: "React", installs: 30 }],
        },
      ],
    });
    render(<App client={client} />);

    await screen.findByRole("heading", { name: "Skill 清单" });
    await user.click(screen.getByRole("button", { name: "安装 Skill" }));
    await screen.findByRole("heading", { name: "安装 Skill" });
    await user.type(
      screen.getByRole("searchbox", { name: "搜索 skills.sh" }),
      "react",
    );
    await user.click(screen.getByRole("button", { name: "搜索 skills.sh" }));

    expect(client.searchSkillsSh).toHaveBeenCalledWith("react");
    expect(
      await screen.findByRole("heading", {
        name: "vercel-labs/agent-skills",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("React Best Practices")).toBeInTheDocument();
    expect(screen.getByText("React Native")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "react.dev" })).toBeInTheDocument();
    expect(screen.getByText("当前不是受支持的 GitHub Source")).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: "添加 vercel-labs/agent-skills Source",
      }),
    ).toBeEnabled();
    expect(
      screen.queryByRole("button", { name: "添加 react.dev Source" }),
    ).not.toBeInTheDocument();
  });

  it("新搜索失败时清空旧结果，只显示本次错误", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([createEntry()]));
    vi.mocked(client.searchSkillsSh)
      .mockResolvedValueOnce({
        type: "skillsShSearch",
        query: "react",
        sources: [
          {
            sourceInput: "vercel-labs/agent-skills",
            supported: true,
            members: [
              {
                skillId: "react-best-practices",
                name: "React Best Practices",
                installs: 20,
              },
            ],
          },
        ],
      })
      .mockRejectedValueOnce({
        code: "skillsShError",
        message: "skills.sh 暂时不可用",
      });
    render(<App client={client} />);

    await screen.findByRole("heading", { name: "Skill 清单" });
    await user.click(screen.getByRole("button", { name: "安装 Skill" }));
    await user.type(
      screen.getByRole("searchbox", { name: "搜索 skills.sh" }),
      "react",
    );
    await user.click(screen.getByRole("button", { name: "搜索 skills.sh" }));
    expect(
      await screen.findByRole("heading", {
        name: "vercel-labs/agent-skills",
      }),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "搜索 skills.sh" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "skills.sh 暂时不可用",
    );
    expect(client.searchSkillsSh).toHaveBeenCalledTimes(2);
    expect(
      screen.queryByRole("region", { name: "skills.sh 搜索结果" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("heading", {
        name: "vercel-labs/agent-skills",
      }),
    ).not.toBeInTheDocument();
  });

  it("只在用户点击安装后进入 Source Catalog，返回时保留原 Inventory", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([createEntry({ skillName: "saved" })]),
    );
    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
    expect(client.openSourceDiscovery).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "安装 Skill" }));

    expect(client.openSourceDiscovery).toHaveBeenCalledTimes(1);
    expect(
      await screen.findByRole("heading", { name: "安装 Skill" }),
    ).toBeInTheDocument();
    expect(client.chooseFolderInstallPlan).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "返回清单" }));

    expect(screen.getByText("saved")).toBeInTheDocument();
    expect(client.getStartupState).toHaveBeenCalledTimes(1);
  });

  it("首次加载 Source 期间保留清单浏览，但禁用全部写入口", async () => {
    const user = userEvent.setup();
    let finishOpening:
      | ((outcome: Extract<UiOutcome, { type: "sourceDiscovery" }>) => void)
      | undefined;
    const client = createClient(
      inventoryOutcome([createManagedEntry(), createEntry()]),
    );
    vi.mocked(client.openSourceDiscovery).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishOpening = resolve;
        }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));

    expect(screen.getByRole("button", { name: "正在加载来源…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "添加项目" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "刷新本机" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "批量挂载" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "管理挂载" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "接管 example" })).toBeDisabled();

    await act(async () => {
      finishOpening?.(sourceDiscoveryOutcome());
    });
    expect(
      screen.getByRole("heading", { name: "安装 Skill" }),
    ).toBeInTheDocument();
  });

  it("展示 Fresh、Stale 和首次加载失败，并且只允许 Fresh 安装", async () => {
    const user = userEvent.setup();
    const initial = sourceDiscoveryOutcome([
      createSource(),
      createSource({
        id: "source-stale",
        canonicalIdentity: "github:example/stale",
        displayName: "example/stale",
        locator: "https://github.com/example/stale",
        catalogStatus: "stale",
        lastReloadError: "GitHub 暂时不可用",
      }),
      createSource({
        id: "source-unloaded",
        canonicalIdentity: "github:example/unloaded",
        displayName: "example/unloaded",
        locator: "https://github.com/example/unloaded",
        catalogStatus: "unloaded",
        catalogMarker: null,
        catalogFetchedAt: null,
        lastReloadError: "尚未成功加载",
        members: [],
      }),
    ]);
    const reloaded = sourceDiscoveryOutcome([
      createSource(),
      createSource({
        id: "source-stale",
        canonicalIdentity: "github:example/stale",
        displayName: "example/stale",
        locator: "https://github.com/example/stale",
      }),
    ]);
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.openSourceDiscovery).mockResolvedValue(initial);
    vi.mocked(client.reloadGithubSource).mockResolvedValue(reloaded);
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));

    const freshCard = screen.getByRole("article", { name: "anthropics/skills" });
    const staleCard = screen.getByRole("article", { name: "example/stale" });
    const unloadedCard = screen.getByRole("article", { name: "example/unloaded" });
    expect(within(freshCard).getByText("目录已加载")).toBeInTheDocument();
    expect(within(freshCard).getByRole("button", { name: "安装 Bundle" })).toBeEnabled();
    expect(within(staleCard).getByText("上次目录已过期")).toBeInTheDocument();
    expect(within(staleCard).getByText(/上次成功加载/)).toBeInTheDocument();
    expect(within(staleCard).getByText("等待重新加载")).toBeInTheDocument();
    expect(within(staleCard).getByText(/GitHub 暂时不可用/)).toBeInTheDocument();
    expect(within(staleCard).getByRole("button", { name: "安装 Bundle" })).toBeDisabled();
    expect(within(unloadedCard).getByText("尚未加载")).toBeInTheDocument();
    expect(within(unloadedCard).getByText(/尚未成功加载/)).toBeInTheDocument();

    await user.click(
      within(staleCard).getByRole("button", { name: "重新加载来源" }),
    );

    expect(client.reloadGithubSource).toHaveBeenCalledWith("source-stale");
    expect(
      within(screen.getByRole("article", { name: "example/stale" })).getByRole(
        "button",
        { name: "安装 Bundle" },
      ),
    ).toBeEnabled();
  });

  it("本地 Source 只展示来源和已安装状态，不误显示 GitHub 操作", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.openSourceDiscovery).mockResolvedValue(
      sourceDiscoveryOutcome([
        createSource({
          id: "source-archive",
          kind: "archive",
          canonicalIdentity: "archive:/tmp/superpowers.skill",
          displayName: "superpowers",
          locator: "/tmp/superpowers.skill",
          trackedRef: null,
          bundleId: "bundle-superpowers",
          adoptedMarker: "archive-marker",
          members: [
            createSourceMember({ installedMemberId: "member-brainstorming" }),
          ],
        }),
      ]),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));

    const card = screen.getByRole("article", { name: "superpowers" });
    expect(within(card).getByText("/tmp/superpowers.skill")).toBeInTheDocument();
    expect(within(card).getByText("已安装 · 未挂载")).toBeInTheDocument();
    expect(
      within(card).queryByRole("button", { name: "重新加载来源" }),
    ).not.toBeInTheDocument();
    expect(
      within(card).queryByRole("button", { name: "补装 Skill" }),
    ).not.toBeInTheDocument();
  });

  it("Editable Local 重新关联只确认路径，不把候选内容描述成已更新", async () => {
    const user = userEvent.setup();
    const inventory = inventoryOutcome([createManagedEntry()]);
    const editable = createSource({
      kind: "editableLocal",
      canonicalIdentity: "editable-local:1:2",
      displayName: "original-skills",
      locator: "/tmp/author/original-skills",
      trackedRef: null,
      bundleId: "bundle-1",
      adoptedMarker: "old-marker",
      members: [
        createSourceMember({
          relativePath: "alpha",
          skillName: "alpha",
          installedMemberId: "member-1",
        }),
      ],
    });
    const client = createClient(inventory);
    vi.mocked(client.openSourceDiscovery).mockResolvedValue(
      sourceDiscoveryOutcome([editable]),
    );
    vi.mocked(client.chooseEditableLocalRelinkPlan).mockResolvedValue(
      createEditableLocalRelinkPlan(),
    );
    vi.mocked(client.confirmEditableLocalRelinkPlan).mockResolvedValue(
      sourceDiscoveryOutcome([
        createSource({
          ...editable,
          displayName: "moved-skills",
          locator: "/tmp/author/moved-skills",
        }),
      ]),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.click(screen.getByRole("button", { name: "重新指定路径" }));

    expect(client.chooseEditableLocalRelinkPlan).toHaveBeenCalledWith("source-1");
    expect(
      screen.getByRole("heading", { name: "确认重新指定 Source 路径" }),
    ).toBeInTheDocument();
    const preview = screen.getByLabelText("Source 路径变更预览");
    expect(preview).toHaveTextContent("/tmp/author/original-skills");
    expect(preview).toHaveTextContent("/tmp/author/moved-skills");
    expect(screen.getByText(/本次操作不会直接采用这些变化/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "确认新路径" }));

    expect(client.confirmEditableLocalRelinkPlan).toHaveBeenCalledWith(
      "relink-plan-1",
    );
    expect(
      await screen.findByText("/tmp/author/moved-skills"),
    ).toBeInTheDocument();
  });

  it("重启后继续显示未确认的重新关联，并在明确取消后回到原清单", async () => {
    const user = userEvent.setup();
    const plan = createEditableLocalRelinkPlan();
    const inventory = inventoryOutcome([createManagedEntry()]);
    const client = createClient({
      type: "editableLocalRelinkPlan",
      plan,
    });
    vi.mocked(client.getStartupState)
      .mockResolvedValueOnce({ type: "editableLocalRelinkPlan", plan })
      .mockResolvedValueOnce(inventory);
    vi.mocked(client.discardEditableLocalRelinkPlan).mockResolvedValue(
      sourceDiscoveryOutcome([
        createSource({
          kind: "editableLocal",
          canonicalIdentity: "editable-local:1:2",
          locator: plan.currentPath,
          trackedRef: null,
        }),
      ]),
    );
    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", {
        name: "确认重新指定 Source 路径",
      }),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "取消" }));

    expect(client.discardEditableLocalRelinkPlan).toHaveBeenCalledWith(
      "relink-plan-1",
    );
    expect(
      await screen.findByRole("heading", { name: "安装 Skill" }),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "返回清单" }));
    expect(
      screen.getByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
  });

  it("添加同一 Source 的不同 Ref 时先确认，确认后再显示新 Ref", async () => {
    const user = userEvent.setup();
    const initialInventory = inventoryOutcome(
      [createManagedEntry()],
      null,
      {
        bundleUpdates: [
          {
            bundleId: "bundle-1",
            status: "upToDate",
            action: null,
            checkedAt: 1_753_000_001_000,
            message: "已是最新",
            upstreamUrl: "https://github.com/anthropics/skills",
          },
        ],
      },
    );
    const refreshedInventory = inventoryOutcome(
      [createManagedEntry()],
      null,
      {
        bundleUpdates: [
          {
            bundleId: "bundle-1",
            status: "available",
            action: "update",
            checkedAt: 1_753_000_002_000,
            message: "发现新的上游 commit",
            upstreamUrl: "https://github.com/anthropics/skills",
          },
        ],
      },
    );
    const client = createClient(initialInventory);
    vi.mocked(client.getStartupState)
      .mockResolvedValueOnce(initialInventory)
      .mockResolvedValueOnce(refreshedInventory);
    vi.mocked(client.addGithubSource).mockResolvedValue({
      type: "sourceRefChangePlan",
      plan: createSourceRefChangePlan(),
    });
    vi.mocked(client.confirmSourceRefChange).mockResolvedValue(
      sourceDiscoveryOutcome([createSource({ trackedRef: "next" })]),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.type(
      screen.getByLabelText("GitHub 仓库"),
      "https://github.com/anthropics/skills/tree/next",
    );
    await user.type(screen.getByLabelText("Tracked Ref（可选）"), "next");
    await user.click(screen.getByRole("button", { name: "添加 Source" }));

    expect(client.addGithubSource).toHaveBeenCalledWith(
      "https://github.com/anthropics/skills/tree/next",
      "next",
    );
    expect(
      screen.getByRole("heading", { name: "确认更改 Source 分支" }),
    ).toBeInTheDocument();
    const refPreview = screen.getByLabelText("Tracked Ref 变更预览");
    expect(refPreview).toHaveTextContent("当前 Refmain");
    expect(refPreview).toHaveTextContent("新的 Refnext");
    expect(refPreview).toHaveTextContent("已解析 Commitcommit-next");

    await user.click(screen.getByRole("button", { name: "确认更改" }));

    expect(client.confirmSourceRefChange).toHaveBeenCalledWith(
      "source-ref-plan-1",
    );
    expect(client.getStartupState).toHaveBeenCalledTimes(2);
    expect(screen.getByText("Tracked Ref: next")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "返回清单" }));
    expect(
      screen.getByLabelText("Bundle 更新状态：可更新"),
    ).toBeInTheDocument();
  });

  it("Ref 确认失败后丢弃旧 Plan，并重新读取 Source 状态", async () => {
    const user = userEvent.setup();
    const initial = sourceDiscoveryOutcome();
    const recovered = sourceDiscoveryOutcome([
      createSource({ trackedRef: "next" }),
    ]);
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.openSourceDiscovery)
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(recovered);
    vi.mocked(client.addGithubSource).mockResolvedValue({
      type: "sourceRefChangePlan",
      plan: createSourceRefChangePlan(),
    });
    vi.mocked(client.confirmSourceRefChange).mockRejectedValue({
      code: "storageError",
      message: "Ref 状态不确定",
    });
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.type(screen.getByLabelText("GitHub 仓库"), "anthropics/skills");
    await user.type(screen.getByLabelText("Tracked Ref（可选）"), "next");
    await user.click(screen.getByRole("button", { name: "添加 Source" }));
    await user.click(screen.getByRole("button", { name: "确认更改" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Ref 状态不确定",
    );
    expect(client.openSourceDiscovery).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("button", { name: "确认更改" })).not.toBeInTheDocument();
    expect(screen.getByText("Tracked Ref: next")).toBeInTheDocument();
  });

  it("添加 Source 返回错误时重读最终 SQLite 状态", async () => {
    const user = userEvent.setup();
    const initial = sourceDiscoveryOutcome();
    const recovered = sourceDiscoveryOutcome([
      createSource(),
      createSource({
        id: "source-new",
        canonicalIdentity: "github:example/new-skills",
        displayName: "example/new-skills",
        locator: "https://github.com/example/new-skills",
      }),
    ]);
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.openSourceDiscovery)
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(recovered);
    vi.mocked(client.addGithubSource).mockRejectedValue({
      code: "noticeError",
      message: "Source 已保存，但说明文件暂时无法更新",
    });
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.type(screen.getByLabelText("GitHub 仓库"), "example/new-skills");
    await user.click(screen.getByRole("button", { name: "添加 Source" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Source 已保存，但说明文件暂时无法更新",
    );
    expect(client.openSourceDiscovery).toHaveBeenCalledTimes(2);
    expect(screen.getByText("example/new-skills")).toBeInTheDocument();
  });

  it("GitHub 安装使用通用确认页，返回时先真实放弃 Plan", async () => {
    const user = userEvent.setup();
    let finishDiscard: (() => void) | undefined;
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.createGithubInstallPlan).mockResolvedValue(
      createInstallPlan({
        inputKind: "github",
        inputPath: "https://github.com/anthropics/skills",
      }),
    );
    vi.mocked(client.discardInstallPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishDiscard = resolve;
        }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.click(screen.getByRole("button", { name: "安装 Bundle" }));

    expect(screen.getByLabelText("安装影响预览")).toHaveTextContent(
      "Sourcehttps://github.com/anthropics/skills",
    );
    await user.click(screen.getByRole("button", { name: "返回" }));

    expect(client.discardInstallPlan).toHaveBeenCalledWith("plan-1");
    expect(screen.getByRole("button", { name: "正在返回…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "确认安装" })).toBeDisabled();

    await act(async () => {
      finishDiscard?.();
    });

    expect(
      screen.getByRole("heading", { name: "安装 Skill" }),
    ).toBeInTheDocument();
  });

  it("ZIP、直接 URL 和个人编辑目录都进入同一张安装确认页", async () => {
    const user = userEvent.setup();
    const archiveClient = createClient(inventoryOutcome([]));
    vi.mocked(archiveClient.chooseArchiveInstallPlan).mockResolvedValue(
      createInstallPlan({
        inputKind: "archive",
        inputPath: "/tmp/superpowers.skill",
      }),
    );
    const archiveView = render(<App client={archiveClient} />);
    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.click(
      screen.getByRole("button", { name: "从 ZIP / .skill 安装" }),
    );
    expect(screen.getByLabelText("安装影响预览")).toHaveTextContent(
      "/tmp/superpowers.skill",
    );
    archiveView.unmount();

    const urlClient = createClient(inventoryOutcome([]));
    vi.mocked(urlClient.createUrlInstallPlan).mockResolvedValue(
      createInstallPlan({
        inputKind: "directUrl",
        inputPath: "https://example.com/skills.zip",
      }),
    );
    const urlView = render(<App client={urlClient} />);
    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.type(
      screen.getByLabelText("ZIP / .skill 直接 URL"),
      "https://example.com/skills.zip",
    );
    await user.click(screen.getByRole("button", { name: "准备安装" }));
    expect(urlClient.createUrlInstallPlan).toHaveBeenCalledWith(
      "https://example.com/skills.zip",
    );
    expect(screen.getByLabelText("安装影响预览")).toHaveTextContent(
      "https://example.com/skills.zip",
    );
    urlView.unmount();

    const editableClient = createClient(inventoryOutcome([]));
    vi.mocked(editableClient.chooseEditableLocalInstallPlan).mockResolvedValue(
      createInstallPlan({
        inputKind: "editableLocal",
        inputPath: "/tmp/my-skills",
      }),
    );
    render(<App client={editableClient} />);
    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.click(
      screen.getByRole("button", { name: "从个人编辑目录安装" }),
    );
    expect(screen.getByLabelText("安装影响预览")).toHaveTextContent(
      "/tmp/my-skills",
    );
  });

  it("Plan 放弃失败时保留确认页，不能把清理失败伪装成返回成功", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.createGithubInstallPlan).mockResolvedValue(
      createInstallPlan({
        inputKind: "github",
        inputPath: "https://github.com/anthropics/skills",
      }),
    );
    vi.mocked(client.discardInstallPlan).mockRejectedValue({
      code: "storageError",
      message: "无法删除安装快照",
    });
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.click(screen.getByRole("button", { name: "安装 Bundle" }));
    await user.click(screen.getByRole("button", { name: "返回" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "无法删除安装快照",
    );
    expect(screen.getByRole("button", { name: "确认安装" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "返回" })).toBeEnabled();
  });

  it("Plan 已在其他实例消费时退出旧确认页并重读最终状态", async () => {
    const user = userEvent.setup();
    const initial = inventoryOutcome([]);
    const recovered = inventoryOutcome([
      createManagedEntry({ bundleDisplayName: "installed-elsewhere" }),
    ]);
    const client = createClient(initial);
    vi.mocked(client.getStartupState)
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(recovered);
    vi.mocked(client.createGithubInstallPlan).mockResolvedValue(
      createInstallPlan({ inputKind: "github" }),
    );
    vi.mocked(client.discardInstallPlan).mockRejectedValue({
      code: "installPlanConsumed",
      message: "安装 Plan 已经使用，不能重复确认",
    });
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.click(screen.getByRole("button", { name: "安装 Bundle" }));
    await user.click(screen.getByRole("button", { name: "返回" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "安装 Plan 已经使用",
    );
    expect(client.getStartupState).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("button", { name: "确认安装" })).not.toBeInTheDocument();
    expect(screen.getByText("installed-elsewhere: example")).toBeInTheDocument();
  });

  it("补装只提交未安装成员，并明确不覆盖已有内容和 Mount", async () => {
    const user = userEvent.setup();
    const supplementSource = createSource({
      bundleId: "bundle-1",
      adoptedMarker: "commit-old",
      members: [
        createSourceMember({ installedMemberId: "member-existing" }),
        createSourceMember({
          id: "catalog-member-new",
          relativePath: "skills/new-skill",
          skillName: "new-skill",
        }),
      ],
    });
    const client = createClient(
      inventoryOutcome(
        [createManagedEntry({ memberId: "member-existing" })],
        null,
        { mounts: [createMount({ memberId: "member-existing" })] },
      ),
    );
    vi.mocked(client.openSourceDiscovery).mockResolvedValue(
      sourceDiscoveryOutcome([supplementSource]),
    );
    vi.mocked(client.createGithubInstallPlan).mockResolvedValue(
      createInstallPlan({
        inputKind: "github",
        mode: "supplement",
        inputPath: "https://github.com/anthropics/skills",
        candidates: [
          createInstallCandidate({
            candidateId: "candidate-new",
            sourceRelativePath: "skills/new-skill",
            skillName: "new-skill",
          }),
        ],
      }),
    );
    vi.mocked(client.confirmInstallPlan).mockResolvedValue(inventoryOutcome([]));
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    expect(screen.getByText("已安装 · 已挂载 1 处")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "补装 Skill" }));

    expect(screen.getByText(/已有 Skill 内容和 Mount 不会被覆盖/)).toBeInTheDocument();
    expect(screen.getAllByRole("checkbox")).toHaveLength(1);
    expect(screen.getByRole("checkbox", { name: /new-skill/ })).toBeChecked();

    await user.click(screen.getByRole("button", { name: "确认安装" }));

    expect(client.confirmInstallPlan).toHaveBeenCalledWith("plan-1", [
      "candidate-new",
    ]);
  });

  it("Source 成员把缺失或冲突的 Mount 显示为异常而不是正常挂载", async () => {
    const user = userEvent.setup();
    const source = createSource({
      bundleId: "bundle-1",
      members: [createSourceMember({ installedMemberId: "member-existing" })],
    });
    const client = createClient(
      inventoryOutcome(
        [createManagedEntry({ memberId: "member-existing" })],
        null,
        {
          mounts: [
            createMount({
              id: "mount-missing",
              memberId: "member-existing",
              health: "missing",
            }),
            createMount({
              id: "mount-conflict",
              memberId: "member-existing",
              health: "conflict",
            }),
          ],
        },
      ),
    );
    vi.mocked(client.openSourceDiscovery).mockResolvedValue(
      sourceDiscoveryOutcome([source]),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));

    expect(screen.getByText("已安装 · 挂载异常 2 处")).toBeInTheDocument();
    expect(screen.queryByText(/已安装 · 已挂载/)).not.toBeInTheDocument();
  });
});

describe("补充来源与 Bundle 归并", () => {
  it("只给没有 Source 的受管 Bundle 提供补充来源入口", async () => {
    const client = createClient(
      inventoryOutcome([
        createManagedEntry(),
        createManagedEntry({
          id: "managed:linked",
          memberId: "member-linked",
          bundleId: "bundle-linked",
          bundleDisplayName: "linked-bundle",
          sourceDisplayName: "anthropics/skills",
        }),
      ]),
    );

    render(<App client={client} />);

    const unlinked = await screen.findByRole("region", {
      name: "example-bundle",
    });
    const linked = screen.getByRole("region", { name: "linked-bundle" });
    expect(
      within(unlinked).getByRole("button", { name: "补充来源" }),
    ).toBeEnabled();
    expect(
      within(linked).queryByRole("button", { name: "补充来源" }),
    ).not.toBeInTheDocument();
  });

  it("只列出 Fresh Source，并为每个本地 Skill 提交明确的对应或不对应", async () => {
    const user = userEvent.setup();
    const entries = [
      createManagedEntry(),
      createManagedEntry({
        id: "managed:local",
        memberId: "member-local",
        skillName: "local-only",
      }),
    ];
    const client = createClient(inventoryOutcome(entries));
    vi.mocked(client.openSourceDiscovery).mockResolvedValue(
      sourceDiscoveryOutcome([
        createSource({
          members: [
            createSourceMember(),
            createSourceMember({
              id: "catalog-member-other",
              relativePath: "skills/other",
              skillName: "other",
            }),
          ],
        }),
        createSource({
          id: "source-stale",
          displayName: "stale/source",
          canonicalIdentity: "github:stale/source",
          locator: "https://github.com/stale/source",
          catalogStatus: "stale",
        }),
      ]),
    );
    vi.mocked(client.createSourceAssociationPlan).mockResolvedValue(
      createSourceAssociationPlan(),
    );
    render(<App client={client} />);

    await user.click(
      within(
        await screen.findByRole("region", { name: "example-bundle" }),
      ).getByRole("button", { name: "补充来源" }),
    );

    expect(
      await screen.findByRole("heading", { name: "为 Bundle 补充来源" }),
    ).toBeInTheDocument();
    const sourceSelect = screen.getByRole("combobox", { name: "选择 Source" });
    expect(
      within(sourceSelect).getByRole("option", { name: "anthropics/skills" }),
    ).toBeInTheDocument();
    expect(
      within(sourceSelect).queryByRole("option", { name: "stale/source" }),
    ).not.toBeInTheDocument();

    await user.selectOptions(sourceSelect, "source-1");
    const exampleMapping = screen.getByRole("combobox", {
      name: "example 的对应关系",
    });
    const localMapping = screen.getByRole("combobox", {
      name: "local-only 的对应关系",
    });
    await user.selectOptions(exampleMapping, "skills/example");

    expect(
      within(localMapping).getByRole("option", { name: /example/ }),
    ).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "生成关联计划" }));

    expect(client.createSourceAssociationPlan).toHaveBeenCalledWith(
      "bundle-1",
      "source-1",
      [
        {
          memberId: "member-1",
          sourceRelativePath: "skills/example",
        },
        { memberId: "member-local", sourceRelativePath: null },
      ],
    );
    expect(
      screen.getByRole("heading", { name: "确认补充来源" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/只建立来源关系/)).toBeInTheDocument();
    expect(screen.getByText(/不会修改当前内容或 Mount/)).toBeInTheDocument();
  });

  it("把 Source 根目录空路径与不对应区分提交", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([
        createManagedEntry(),
        createManagedEntry({
          id: "managed:local",
          memberId: "member-local",
          skillName: "local-only",
        }),
      ]),
    );
    vi.mocked(client.openSourceDiscovery).mockResolvedValue(
      sourceDiscoveryOutcome([
        createSource({
          members: [
            createSourceMember({
              id: "catalog-member-root",
              relativePath: "",
              skillName: "root-skill",
            }),
          ],
        }),
      ]),
    );
    vi.mocked(client.createSourceAssociationPlan).mockResolvedValue(
      createSourceAssociationPlan({
        memberChoices: [
          {
            memberId: "member-1",
            skillName: "example",
            sourceRelativePath: "",
          },
          {
            memberId: "member-local",
            skillName: "local-only",
            sourceRelativePath: null,
          },
        ],
      }),
    );
    render(<App client={client} />);

    await user.click(
      within(
        await screen.findByRole("region", { name: "example-bundle" }),
      ).getByRole("button", { name: "补充来源" }),
    );
    await user.selectOptions(
      screen.getByRole("combobox", { name: "选择 Source" }),
      "source-1",
    );
    await user.selectOptions(
      screen.getByRole("combobox", { name: "example 的对应关系" }),
      "",
    );
    await user.click(screen.getByRole("button", { name: "生成关联计划" }));

    expect(client.createSourceAssociationPlan).toHaveBeenCalledWith(
      "bundle-1",
      "source-1",
      [
        { memberId: "member-1", sourceRelativePath: "" },
        { memberId: "member-local", sourceRelativePath: null },
      ],
    );
    expect(screen.getByText("对应 来源根目录")).toBeInTheDocument();
  });

  it("没有可用 Source 时可以前往现有 Source 页面添加", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([createManagedEntry()]));
    vi.mocked(client.openSourceDiscovery).mockResolvedValue(
      sourceDiscoveryOutcome([
        createSource({ catalogStatus: "stale" }),
      ]),
    );
    render(<App client={client} />);

    await user.click(
      within(
        await screen.findByRole("region", { name: "example-bundle" }),
      ).getByRole("button", { name: "补充来源" }),
    );

    expect(await screen.findByText("没有可选择的 Source")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "前往 Source 页面添加" }),
    );
    expect(
      screen.getByRole("heading", { name: "安装 Skill" }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("添加 GitHub Source")).toBeInTheDocument();
  });

  it("放弃关联 Plan 后回到原 Bundle，不在前端伪装成功", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([createManagedEntry()]));
    vi.mocked(client.createSourceAssociationPlan).mockResolvedValue(
      createSourceAssociationPlan(),
    );
    vi.mocked(client.discardSourceAssociationPlan).mockResolvedValue(undefined);
    render(<App client={client} />);

    await openSourceAssociationPlan(user);
    await user.click(screen.getByRole("button", { name: "返回" }));

    expect(client.discardSourceAssociationPlan).toHaveBeenCalledWith(
      "association-plan-1",
    );
    expect(
      await screen.findByRole("region", { name: "example-bundle" }),
    ).toBeInTheDocument();
  });

  it("确认成功后重新读取 Inventory，再显示最终 Source 关系", async () => {
    const user = userEvent.setup();
    const initial = inventoryOutcome([createManagedEntry()]);
    const refreshed = inventoryOutcome([
      createManagedEntry({ sourceDisplayName: "anthropics/skills" }),
    ]);
    const client = createClient(initial);
    vi.mocked(client.getStartupState)
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(refreshed);
    vi.mocked(client.createSourceAssociationPlan).mockResolvedValue(
      createSourceAssociationPlan(),
    );
    vi.mocked(client.confirmSourceAssociationPlan).mockResolvedValue(refreshed);
    render(<App client={client} />);

    await openSourceAssociationPlan(user);
    await user.click(screen.getByRole("button", { name: "确认关联" }));

    expect(client.confirmSourceAssociationPlan).toHaveBeenCalledWith(
      "association-plan-1",
      [],
    );
    expect(client.getStartupState).toHaveBeenCalledTimes(2);
    expect(await screen.findByText("anthropics/skills")).toBeInTheDocument();
  });

  it("确认失败后重新读取 Inventory，并显示真实持久状态", async () => {
    const user = userEvent.setup();
    const initial = inventoryOutcome([createManagedEntry()]);
    const refreshed = inventoryOutcome([createManagedEntry()]);
    const client = createClient(initial);
    vi.mocked(client.getStartupState)
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(refreshed);
    vi.mocked(client.createSourceAssociationPlan).mockResolvedValue(
      createSourceAssociationPlan(),
    );
    vi.mocked(client.confirmSourceAssociationPlan).mockRejectedValue({
      code: "sourceAssociationError",
      message: "Source Catalog 已变化，请重新生成计划",
    });
    render(<App client={client} />);

    await openSourceAssociationPlan(user);
    await user.click(screen.getByRole("button", { name: "确认关联" }));

    expect(client.getStartupState).toHaveBeenCalledTimes(2);
    expect(
      await screen.findByRole("region", { name: "example-bundle" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Source Catalog 已变化，请重新生成计划",
    );
    expect(
      screen.queryByRole("heading", { name: "确认补充来源" }),
    ).not.toBeInTheDocument();
  });

  it("merge 复用同一确认页，并在全部内容冲突选择后才能确认", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([createManagedEntry()]));
    const mergePlan = createSourceAssociationPlan({
      mode: "merge",
      targetBundleId: "bundle-target",
      targetBundleDisplayName: "source-bundle",
      retiringBundleId: "bundle-1",
      retiringBundleDisplayName: "example-bundle",
      members: [
        {
          memberId: "member-target",
          bundleId: "bundle-target",
          bundleDisplayName: "same-bundle",
          skillName: "example",
          contentFingerprint: "11111111aaaaaaaa",
        },
        {
          memberId: "member-1",
          bundleId: "bundle-1",
          bundleDisplayName: "same-bundle",
          skillName: "example",
          contentFingerprint: "22222222bbbbbbbb",
        },
      ],
      mounts: [createMount()],
      conflicts: [
        {
          id: "conflict-1",
          label: "同一个 Source Member",
          candidateMemberIds: ["member-target", "member-1"],
        },
      ],
    });
    vi.mocked(client.createSourceAssociationPlan).mockResolvedValue(mergePlan);
    vi.mocked(client.confirmSourceAssociationPlan).mockResolvedValue(
      inventoryOutcome([]),
    );
    render(<App client={client} />);

    await openSourceAssociationPlan(user);

    expect(
      screen.getByRole("heading", { name: "确认归并 Bundle" }),
    ).toBeInTheDocument();
    expect(screen.getAllByText(/source-bundle/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/example-bundle/).length).toBeGreaterThan(0);
    expect(screen.getByText(/Codex · 全局/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认归并" })).toBeDisabled();
    expect(
      screen.getByRole("radio", {
        name: /保留已关联 Bundle.*11111111/,
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("radio", {
        name: /使用待归入 Bundle.*22222222/,
      }),
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("radio", {
        name: /使用待归入 Bundle.*22222222/,
      }),
    );
    expect(screen.getByRole("button", { name: "确认归并" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "确认归并" }));

    expect(client.confirmSourceAssociationPlan).toHaveBeenCalledWith(
      "association-plan-1",
      [{ conflictId: "conflict-1", memberId: "member-1" }],
    );
  });

  it("归并存在 blocker 时展示原因，并始终禁止确认", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([createManagedEntry()]));
    vi.mocked(client.createSourceAssociationPlan).mockResolvedValue(
      createSourceAssociationPlan({
        mode: "merge",
        targetBundleId: "bundle-target",
        targetBundleDisplayName: "source-bundle",
        retiringBundleId: "bundle-1",
        retiringBundleDisplayName: "example-bundle",
        members: [
          {
            memberId: "member-target",
            bundleId: "bundle-target",
            bundleDisplayName: "source-bundle",
            skillName: "example",
            contentFingerprint: "11111111aaaaaaaa",
          },
          {
            memberId: "member-1",
            bundleId: "bundle-1",
            bundleDisplayName: "example-bundle",
            skillName: "example",
            contentFingerprint: "22222222bbbbbbbb",
          },
        ],
        conflicts: [
          {
            id: "conflict-1",
            label: "同名 Skill：example",
            candidateMemberIds: ["member-target", "member-1"],
          },
        ],
        blockingIssues: ["需要先移除冲突 Mount，再重新生成计划"],
      }),
    );
    render(<App client={client} />);

    await openSourceAssociationPlan(user);

    expect(screen.getByRole("alert")).toHaveTextContent(
      "需要先移除冲突 Mount，再重新生成计划",
    );
    await user.click(
      screen.getByRole("radio", {
        name: /使用待归入 Bundle.*22222222/,
      }),
    );
    expect(screen.getByRole("button", { name: "确认归并" })).toBeDisabled();
    expect(client.confirmSourceAssociationPlan).not.toHaveBeenCalled();
  });

  it("创建 Plan 失败时保留映射页，并显示 Rust 错误", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([createManagedEntry()]));
    vi.mocked(client.createSourceAssociationPlan).mockRejectedValue({
      code: "sourceAssociationChanged",
      message: "Source Catalog 已变化，请重新选择",
    });
    render(<App client={client} />);

    await user.click(
      within(
        await screen.findByRole("region", { name: "example-bundle" }),
      ).getByRole("button", { name: "补充来源" }),
    );
    await user.selectOptions(
      screen.getByRole("combobox", { name: "选择 Source" }),
      "source-1",
    );
    await user.click(screen.getByRole("button", { name: "生成关联计划" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Source Catalog 已变化，请重新选择",
    );
    expect(
      screen.getByRole("heading", { name: "为 Bundle 补充来源" }),
    ).toBeInTheDocument();
  });

  it("Source Catalog 不根据 adopted marker 自行推断更新状态", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.openSourceDiscovery).mockResolvedValue(
      sourceDiscoveryOutcome([
        createSource({ bundleId: "bundle-1", adoptedMarker: null }),
      ]),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));

    expect(
      within(
        screen.getByRole("article", { name: "anthropics/skills" }),
      ).queryByText("可更新"),
    ).not.toBeInTheDocument();
  });
});

describe("移除与删除", () => {
  it("Bundle 删除完整展示影响，并且第二次点击前不会确认", async () => {
    const user = userEvent.setup();
    let finishConfirm: ((outcome: UiOutcome) => void) | undefined;
    const initial = inventoryOutcome(
      [
        createManagedEntry(),
        createManagedEntry({
          id: "managed:member-2",
          memberId: "member-2",
          skillName: "example-tools",
        }),
      ],
      null,
      {
        mounts: createRemovalPlan().mounts,
      },
    );
    const client = createClient(initial);
    vi.mocked(client.createBundleRemovalPlan).mockResolvedValue({
      type: "removalPlan",
      plan: createRemovalPlan(),
    });
    vi.mocked(client.confirmRemovalPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishConfirm = resolve;
        }),
    );
    render(<App client={client} />);

    const bundle = await screen.findByRole("region", {
      name: "example-bundle",
    });
    await user.click(
      within(bundle).getByRole("button", {
        name: "删除 Bundle example-bundle",
      }),
    );

    expect(client.createBundleRemovalPlan).toHaveBeenCalledWith("bundle-1");
    expect(
      screen.getByRole("heading", { name: "删除 Bundle example-bundle" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "将删除的 Skill" })).toHaveTextContent(
      "example-tools",
    );
    expect(screen.getByRole("region", { name: "将移除的 Mount" })).toHaveTextContent(
      "Claude Code · 项目 · SkillYard",
    );
    expect(screen.getByText("/tmp/central/bundles/bundle-1")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "将保留的内容" })).toHaveTextContent(
      "anthropics/skills",
    );
    expect(screen.getByRole("region", { name: "将保留的内容" })).toHaveTextContent(
      "/Users/test/editable/example",
    );
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /删除 Skill/ }),
    ).not.toBeInTheDocument();
    expect(client.confirmRemovalPlan).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "继续删除" }));

    expect(client.confirmRemovalPlan).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("永久删除");
    expect(screen.getByRole("alert")).toHaveTextContent("没有回滚");

    const confirm = screen.getByRole("button", { name: "确认永久删除" });
    const back = screen.getByRole("button", { name: "返回清单" });
    await user.click(confirm);

    expect(client.confirmRemovalPlan).toHaveBeenCalledWith("removal-plan-1");
    expect(confirm).toBeDisabled();
    expect(back).toBeDisabled();
    await user.click(confirm);
    expect(client.confirmRemovalPlan).toHaveBeenCalledTimes(1);

    await act(async () => {
      finishConfirm?.(inventoryOutcome([]));
    });
    expect(
      screen.getByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
  });

  it("删除 Source 只做普通确认，并明确本地内容和 Editable 原目录保留", async () => {
    const user = userEvent.setup();
    const initial = inventoryOutcome([createManagedEntry()]);
    const source = createSource({
      id: "source-editable",
      kind: "editableLocal",
      displayName: "editable-skills",
      locator: "/Users/test/editable/skills",
      bundleId: "bundle-1",
    });
    const client = createClient(initial);
    vi.mocked(client.getStartupState)
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(
        inventoryOutcome(
          [createManagedEntry({ sourceDisplayName: null })],
          null,
          {
            bundleUpdates: [
              {
                bundleId: "bundle-1",
                status: "noSource",
                action: null,
                checkedAt: null,
                message: "没有更新来源",
                upstreamUrl: null,
              },
            ],
          },
        ),
      );
    vi.mocked(client.openSourceDiscovery).mockResolvedValue(
      sourceDiscoveryOutcome([source]),
    );
    vi.mocked(client.createSourceRemovalPlan).mockResolvedValue({
      type: "removalPlan",
      plan: createRemovalPlan({
        kind: "source",
        targetId: "source-editable",
        targetDisplayName: "editable-skills",
        members: [],
        mounts: [],
        affectedBundles: [
          { id: "bundle-1", displayName: "example-bundle" },
        ],
        preservedSource: null,
        managedDirectory: null,
        preservedExternalPaths: ["/Users/test/editable/skills"],
        warnings: [],
      }),
    });
    vi.mocked(client.confirmRemovalPlan).mockResolvedValue(
      sourceDiscoveryOutcome([]),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    const sourceCard = await screen.findByRole("article", {
      name: "editable-skills",
    });
    await user.click(
      within(sourceCard).getByRole("button", {
        name: "删除 Source editable-skills",
      }),
    );

    expect(client.createSourceRemovalPlan).toHaveBeenCalledWith(
      "source-editable",
    );
    expect(
      screen.getByRole("heading", { name: "删除 Source editable-skills" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "失去更新来源的 Bundle" }))
      .toHaveTextContent("example-bundle");
    expect(screen.getByText(/本地 Bundle、current 内容和 Mount 都会保留/))
      .toBeInTheDocument();
    expect(screen.getByText(/Editable Local 原目录不会被删除/))
      .toBeInTheDocument();
    expect(screen.getByText("/Users/test/editable/skills")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "继续删除" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "确认永久删除" }),
    ).not.toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "确认删除 Source" }),
    );

    expect(client.confirmRemovalPlan).toHaveBeenCalledWith("removal-plan-1");
    expect(
      await screen.findByRole("heading", { name: "安装 Skill" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("article", { name: "editable-skills" }),
    ).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "返回清单" }));
    expect(
      screen.getByLabelText("Bundle 更新状态：没有更新来源"),
    ).toBeInTheDocument();
  });

  it("Source 删除预览返回时调用真实 discard 并回到 Source Catalog", async () => {
    const user = userEvent.setup();
    let finishDiscard: ((outcome: UiOutcome) => void) | undefined;
    const initial = inventoryOutcome([]);
    const source = createSource();
    const client = createClient(initial);
    vi.mocked(client.openSourceDiscovery).mockResolvedValue(
      sourceDiscoveryOutcome([source]),
    );
    vi.mocked(client.createSourceRemovalPlan).mockResolvedValue({
      type: "removalPlan",
      plan: createRemovalPlan({
        kind: "source",
        targetId: source.id,
        targetDisplayName: source.displayName,
        members: [],
        mounts: [],
        affectedBundles: [],
        preservedSource: null,
        managedDirectory: null,
        preservedExternalPaths: [],
      }),
    });
    vi.mocked(client.discardRemovalPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishDiscard = resolve;
        }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.click(
      within(
        await screen.findByRole("article", { name: source.displayName }),
      ).getByRole("button", {
        name: `删除 Source ${source.displayName}`,
      }),
    );
    const back = await screen.findByRole("button", {
      name: "返回 Source 列表",
    });
    const confirm = screen.getByRole("button", { name: "确认删除 Source" });
    await user.click(back);

    expect(client.discardRemovalPlan).toHaveBeenCalledWith("removal-plan-1");
    expect(back).toBeDisabled();
    expect(confirm).toBeDisabled();
    await user.click(back);
    expect(client.discardRemovalPlan).toHaveBeenCalledTimes(1);

    await act(async () => {
      finishDiscard?.(sourceDiscoveryOutcome([source]));
    });
    expect(
      screen.getByRole("article", { name: source.displayName }),
    ).toBeInTheDocument();
  });

  it("没有受管 Skill 时仍可移除 Project，并展示将移除的 project Mount", async () => {
    const user = userEvent.setup();
    const projectMount = createMount({
      id: "mount-project",
      scope: "project",
      projectId: "project-1",
      projectDisplayName: "SkillYard",
      targetPath: "/tmp/SkillYard/.codex/skills/example",
    });
    const initial = inventoryOutcome([], null, {
      projects: [
        {
          id: "project-1",
          displayName: "SkillYard",
          rootPath: "/tmp/SkillYard",
        },
      ],
      mounts: [projectMount],
    });
    const client = createClient(initial);
    vi.mocked(client.createProjectRemovalPlan).mockResolvedValue({
      type: "removalPlan",
      plan: createRemovalPlan({
        kind: "project",
        targetId: "project-1",
        targetDisplayName: "SkillYard",
        members: [],
        mounts: [projectMount],
        affectedBundles: [],
        preservedSource: null,
        managedDirectory: null,
        preservedExternalPaths: ["/tmp/SkillYard"],
        warnings: [],
      }),
    });
    vi.mocked(client.confirmRemovalPlan).mockResolvedValue(
      inventoryOutcome([createManagedEntry()]),
    );
    render(<App client={client} />);

    expect(
      await screen.findAllByRole("button", { name: "移除项目 SkillYard" }),
    ).toHaveLength(1);
    await user.click(
      screen.getByRole("button", { name: "移除项目 SkillYard" }),
    );

    expect(client.createProjectRemovalPlan).toHaveBeenCalledWith("project-1");
    expect(
      screen.getByRole("heading", { name: "移除项目 SkillYard" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "将移除的 Mount" }))
      .toHaveTextContent("Codex · 项目 · SkillYard");
    expect(screen.getByText(/不会删除 Bundle 或 Skill/)).toBeInTheDocument();
    expect(screen.getByText(/不会删除项目目录中的未知内容/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "确认移除项目" }));
    expect(client.confirmRemovalPlan).toHaveBeenCalledWith("removal-plan-1");
    expect(
      await screen.findByRole("heading", { name: "Skill 清单" }),
    ).toBeInTheDocument();
  });

  it("确认失败后重读唯一状态，并把 Bundle 危险确认重置到第一步", async () => {
    const user = userEvent.setup();
    const removalOutcome: Extract<UiOutcome, { type: "removalPlan" }> = {
      type: "removalPlan",
      plan: createRemovalPlan(),
    };
    const client = createClient(removalOutcome);
    vi.mocked(client.confirmRemovalPlan).mockRejectedValue({
      code: "lifecycleError",
      message: "删除状态需要重新读取",
    });
    render(<App client={client} />);

    await user.click(
      await screen.findByRole("button", { name: "继续删除" }),
    );
    await user.click(
      screen.getByRole("button", { name: "确认永久删除" }),
    );

    expect(client.confirmRemovalPlan).toHaveBeenCalledTimes(1);
    expect(client.getStartupState).toHaveBeenCalledTimes(2);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "删除状态需要重新读取",
    );
    expect(
      screen.getByRole("button", { name: "继续删除" }),
    ).toBeEnabled();
    expect(
      screen.queryByRole("button", { name: "确认永久删除" }),
    ).not.toBeInTheDocument();
  });
});

describe("平台检查", () => {
  it("在不支持的平台显示阻塞页", async () => {
    const client = createClient({
      type: "unsupportedPlatform",
      actualOs: "macos",
      actualArchitecture: "x86_64",
      actualMajorVersion: 13,
      requiredArchitecture: "aarch64",
      minimumMajorVersion: 14,
    });

    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", {
        name: "当前 Mac 不受 SkillYard 1.0 支持",
      }),
    ).toBeInTheDocument();
    expect(client.startInitialScan).not.toHaveBeenCalled();
    expect(client.refreshLocalInventory).not.toHaveBeenCalled();
  });
});

async function openLocalFolderPicker(
  user: ReturnType<typeof userEvent.setup>,
) {
  await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
  await user.click(
    await screen.findByRole("button", { name: "从本地文件夹安装" }),
  );
}

async function openSourceAssociationPlan(
  user: ReturnType<typeof userEvent.setup>,
) {
  await user.click(
    within(
      await screen.findByRole("region", { name: "example-bundle" }),
    ).getByRole("button", { name: "补充来源" }),
  );
  await user.selectOptions(
    await screen.findByRole("combobox", { name: "选择 Source" }),
    "source-1",
  );
  await user.click(screen.getByRole("button", { name: "生成关联计划" }));
}

function createSourceAssociationPlan(
  overrides: Partial<SourceAssociationPlan> = {},
): SourceAssociationPlan {
  return {
    id: "association-plan-1",
    mode: "link",
    sourceId: "source-1",
    sourceDisplayName: "anthropics/skills",
    targetBundleId: "bundle-1",
    targetBundleDisplayName: "example-bundle",
    retiringBundleId: null,
    retiringBundleDisplayName: null,
    memberChoices: [
      {
        memberId: "member-1",
        skillName: "example",
        sourceRelativePath: null,
      },
    ],
    members: [
      {
        memberId: "member-1",
        bundleId: "bundle-1",
        bundleDisplayName: "example-bundle",
        skillName: "example",
        contentFingerprint: "fingerprint-example",
      },
    ],
    mounts: [],
    conflicts: [],
    blockingIssues: [],
    createdAt: 1,
    expiresAt: 2,
    ...overrides,
  };
}

function createInstallPlan(
  overrides: Partial<InstallPlan> = {},
): InstallPlan {
  return {
    id: "plan-1",
    inputKind: "localFolder",
    mode: "create",
    inputPath: "/tmp/example",
    bundleDisplayName: "example",
    candidates: [createInstallCandidate()],
    updateImpact: null,
    warnings: [],
    willMount: false,
    createdAt: 1,
    expiresAt: 2,
    ...overrides,
  };
}

function createInstallCandidate(
  overrides: Partial<InstallPlan["candidates"][number]> = {},
): InstallPlan["candidates"][number] {
  return {
    candidateId: "candidate-example",
    sourceRelativePath: "",
    skillName: "example",
    description: "test skill",
    selectable: true,
    validationErrors: [],
    warnings: [],
    defaultSelected: true,
    targetDirectory:
      "/tmp/central/bundles/example/current/members/example",
    ...overrides,
  };
}

function createBundleUpdateBatchPlan(
  overrides: Partial<BundleUpdateBatchPlan> = {},
): BundleUpdateBatchPlan {
  return {
    id: "update-batch-plan-1",
    items: [
      createBundleUpdateBatchPlanItem(),
      createBundleUpdateBatchPlanItem({
        id: "item-beta",
        bundleId: "bundle-beta",
        bundleDisplayName: "Beta",
        installPlan: createInstallPlan({
          id: "child-beta",
          mode: "update",
          inputKind: "github",
          bundleDisplayName: "Beta",
          updateImpact: {
            newCandidateIds: [],
            existingMounts: [],
            upstreamUrl: "https://github.com/example/beta",
          },
        }),
      }),
    ],
    createdAt: 1,
    expiresAt: 2,
    ...overrides,
  };
}

function createBundleUpdateBatchPlanItem(
  overrides: Partial<BundleUpdateBatchPlan["items"][number]> = {},
): BundleUpdateBatchPlan["items"][number] {
  return {
    id: "item-alpha",
    bundleId: "bundle-alpha",
    bundleDisplayName: "Alpha",
    disposition: "ready",
    installPlan: createInstallPlan({
      id: "child-alpha",
      mode: "update",
      inputKind: "github",
      bundleDisplayName: "Alpha",
      updateImpact: {
        newCandidateIds: [],
        existingMounts: [],
        upstreamUrl: "https://github.com/example/alpha",
      },
    }),
    errorSummary: null,
    ...overrides,
  };
}

function createBundleUpdateBatchResult(
  overrides: Partial<BundleUpdateBatchResult> = {},
): BundleUpdateBatchResult {
  return {
    id: "update-batch-1",
    status: "completed",
    items: [
      createBundleUpdateBatchResultItem(),
      createBundleUpdateBatchResultItem({
        id: "item-beta",
        bundleId: "bundle-beta",
        bundleDisplayName: "Beta",
      }),
    ],
    confirmedAt: 1,
    updatedAt: 2,
    ...overrides,
  };
}

function createBundleUpdateBatchResultItem(
  overrides: Partial<BundleUpdateBatchResult["items"][number]> = {},
): BundleUpdateBatchResult["items"][number] {
  return {
    id: "item-alpha",
    bundleId: "bundle-alpha",
    bundleDisplayName: "Alpha",
    status: "succeeded",
    errorSummary: null,
    ...overrides,
  };
}

function createRemovalPlan(
  overrides: Partial<RemovalPlan> = {},
): RemovalPlan {
  return {
    id: "removal-plan-1",
    kind: "bundle",
    targetId: "bundle-1",
    targetDisplayName: "example-bundle",
    members: [
      { id: "member-1", skillName: "example" },
      { id: "member-2", skillName: "example-tools" },
    ],
    mounts: [
      createMount(),
      createMount({
        id: "mount-project",
        memberId: "member-2",
        skillName: "example-tools",
        appId: "claudeCode",
        scope: "project",
        projectId: "project-1",
        projectDisplayName: "SkillYard",
        targetPath: "/tmp/SkillYard/.claude/skills/example-tools",
      }),
    ],
    affectedBundles: [],
    preservedSource: {
      id: "source-1",
      displayName: "anthropics/skills",
      kind: "github",
      locator: "https://github.com/anthropics/skills",
    },
    managedDirectory: "/tmp/central/bundles/bundle-1",
    preservedExternalPaths: ["/Users/test/editable/example"],
    warnings: ["删除成功后没有回滚入口"],
    createdAt: 1,
    expiresAt: 2,
    ...overrides,
  };
}

function sourceDiscoveryOutcome(
  sources: SourceSummary[] = [createSource()],
  overrides: Partial<Extract<UiOutcome, { type: "sourceDiscovery" }>> = {},
): Extract<UiOutcome, { type: "sourceDiscovery" }> {
  return {
    type: "sourceDiscovery",
    sources,
    highlightedSourceId: null,
    highlightedMemberPath: null,
    ...overrides,
  };
}

function createSource(overrides: Partial<SourceSummary> = {}): SourceSummary {
  return {
    id: "source-1",
    kind: "github",
    canonicalIdentity: "github:anthropics/skills",
    displayName: "anthropics/skills",
    locator: "https://github.com/anthropics/skills",
    trackedRef: "main",
    memberPathHint: null,
    catalogStatus: "fresh",
    catalogMarker: "commit-1",
    catalogFetchedAt: 1,
    lastReloadAt: 1,
    lastReloadError: null,
    bundleId: null,
    adoptedMarker: null,
    members: [createSourceMember()],
    ...overrides,
  };
}

function createSourceMember(
  overrides: Partial<SourceSummary["members"][number]> = {},
): SourceSummary["members"][number] {
  return {
    id: "catalog-member-1",
    relativePath: "skills/example",
    skillName: "example",
    description: "test skill",
    selectable: true,
    validationErrors: [],
    warnings: [],
    installedMemberId: null,
    ...overrides,
  };
}

function createSourceRefChangePlan(
  overrides: Partial<SourceRefChangePlan> = {},
): SourceRefChangePlan {
  return {
    id: "source-ref-plan-1",
    sourceId: "source-1",
    sourceDisplayName: "anthropics/skills",
    currentRef: "main",
    candidateRef: "next",
    candidateCommitSha: "commit-next",
    memberPathHint: null,
    createdAt: 1,
    expiresAt: 2,
    ...overrides,
  };
}

function createEditableLocalRelinkPlan(
  overrides: Partial<EditableLocalRelinkPlan> = {},
): EditableLocalRelinkPlan {
  return {
    id: "relink-plan-1",
    sourceId: "source-1",
    sourceDisplayName: "original-skills",
    currentPath: "/tmp/author/original-skills",
    candidatePath: "/tmp/author/moved-skills",
    candidateDisplayName: "moved-skills",
    bundleDisplayName: "example",
    members: [
      {
        relativePath: "alpha",
        skillName: "alpha",
        description: "alpha description",
        selectable: true,
        validationErrors: [],
        warnings: [],
      },
    ],
    createdAt: 1,
    expiresAt: 2,
    ...overrides,
  };
}

function createClient(startup: UiOutcome): SkillYardClient {
  return {
    getStartupState: vi.fn().mockResolvedValue(startup),
    openCentralStore: vi.fn().mockResolvedValue(undefined),
    startInitialScan: vi.fn(),
    refreshLocalInventory: vi.fn(),
    checkBundleUpdates: vi.fn(),
    checkEditableLocalBundle: vi.fn(),
    createBundleUpdateBatchPlan: vi.fn(),
    confirmBundleUpdateBatchPlan: vi.fn(),
    discardBundleUpdateBatchPlan: vi.fn(),
    acknowledgeBundleUpdateBatchResult: vi.fn(),
    createProjectRemovalPlan: vi.fn(),
    createSourceRemovalPlan: vi.fn(),
    createBundleRemovalPlan: vi.fn(),
    confirmRemovalPlan: vi.fn(),
    discardRemovalPlan: vi.fn(),
    openSourceDiscovery: vi.fn().mockResolvedValue(sourceDiscoveryOutcome()),
    searchSkillsSh: vi.fn(),
    reloadGithubSource: vi.fn(),
    addGithubSource: vi.fn(),
    confirmSourceRefChange: vi.fn(),
    chooseEditableLocalRelinkPlan: vi.fn(),
    confirmEditableLocalRelinkPlan: vi.fn(),
    discardEditableLocalRelinkPlan: vi.fn(),
    createSourceAssociationPlan: vi.fn(),
    confirmSourceAssociationPlan: vi.fn(),
    discardSourceAssociationPlan: vi.fn(),
    createGithubInstallPlan: vi.fn(),
    createBundleUpdatePlan: vi.fn(),
    chooseBundleReplacementPlan: vi.fn(),
    createUrlInstallPlan: vi.fn(),
    discardInstallPlan: vi.fn().mockResolvedValue(undefined),
    chooseFolderInstallPlan: vi.fn(),
    chooseArchiveInstallPlan: vi.fn(),
    chooseEditableLocalInstallPlan: vi.fn(),
    confirmInstallPlan: vi.fn(),
    chooseAndRegisterProject: vi.fn(),
    createTakeoverPlan: vi.fn(),
    confirmTakeoverPlan: vi.fn(),
    createMountPlan: vi.fn(),
    createRemoveMountPlan: vi.fn(),
    createRepairMountPlan: vi.fn(),
    confirmMountPlan: vi.fn(),
    createBatchMountPlan: vi.fn(),
    confirmBatchMountPlan: vi.fn(),
  };
}

function inventoryWithTwoAvailableUpdates(): Extract<
  UiOutcome,
  { type: "inventory" }
> {
  return inventoryOutcome(
    [
      createManagedEntry({
        id: "managed:alpha",
        memberId: "member-alpha",
        bundleId: "bundle-alpha",
        bundleDisplayName: "Alpha",
        skillName: "alpha",
      }),
      createManagedEntry({
        id: "managed:beta",
        memberId: "member-beta",
        bundleId: "bundle-beta",
        bundleDisplayName: "Beta",
        skillName: "beta",
      }),
    ],
    null,
    {
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
    },
  );
}

function inventoryOutcome(
  entries: InventoryObservation[],
  lastLocalRefresh: Extract<UiOutcome, { type: "inventory" }>["lastLocalRefresh"] = null,
  overrides: Partial<Extract<UiOutcome, { type: "inventory" }>> = {},
): Extract<UiOutcome, { type: "inventory" }> {
  return {
    type: "inventory",
    scanCompletedAt: 1_753_000_000_000,
    entries,
    supportedApps: [],
    lastLocalRefresh,
    scanIssues: [],
    recoveryIssues: [],
    projects: [],
    mounts: [],
    bundleUpdates: [],
    ...overrides,
  };
}

function createManagedEntry(
  overrides: Partial<InventoryObservation> = {},
): InventoryObservation {
  return createEntry({
    id: "managed:member-1",
    memberId: "member-1",
    skillName: "example",
    managementKind: "skillYardManaged",
    bundleId: "bundle-1",
    bundleDisplayName: "example-bundle",
    locationKind: "managedStore",
    rootKey: null,
    observedBy: [],
    ...overrides,
  });
}

function createMount(overrides: Partial<MountSummary> = {}): MountSummary {
  return {
    id: "mount-1",
    memberId: "member-1",
    skillName: "example",
    appId: "codex",
    scope: "global",
    projectId: null,
    projectDisplayName: null,
    targetPath: "/tmp/.codex/skills/example",
    expectedTarget: "/tmp/central/bundles/bundle-1/current/members/example",
    health: "healthy",
    ...overrides,
  };
}

function createMountPlan(overrides: Partial<MountPlan> = {}): MountPlan {
  return {
    id: "mount-plan-1",
    operation: "create",
    purpose: "create",
    mountId: "mount-1",
    memberId: "member-1",
    skillName: "example",
    appId: "codex",
    scope: "global",
    projectId: null,
    projectDisplayName: null,
    targetPath: "/tmp/.codex/skills/example",
    expectedTarget: "/tmp/central/bundles/bundle-1/current/members/example",
    targetHealth: "missing",
    createdAt: 1,
    expiresAt: 2,
    ...overrides,
  };
}

function createBatchPlan(
  overrides: Partial<BatchMountPlan> = {},
): BatchMountPlan {
  return {
    id: "batch-plan-1",
    bundleId: "bundle-1",
    bundleDisplayName: "example-bundle",
    items: [createBatchPlanItem()],
    createdAt: 1,
    expiresAt: 2,
    ...overrides,
  };
}

function createBatchPlanItem(
  overrides: Partial<BatchMountPlan["items"][number]> = {},
): BatchMountPlan["items"][number] {
  return {
    id: "batch-item-1",
    memberId: "member-1",
    skillName: "example",
    appId: "codex",
    scope: "global",
    projectId: null,
    projectDisplayName: null,
    targetPath: "/tmp/.codex/skills/example",
    expectedTarget: "/tmp/central/bundles/bundle-1/current/members/example",
    disposition: "ready",
    selectable: true,
    defaultSelected: true,
    conflictReason: null,
    targetHealth: "missing",
    ...overrides,
  };
}

function createTakeoverPlan(
  overrides: Partial<TakeoverPlan> = {},
): TakeoverPlan {
  return {
    id: "takeover-plan-1",
    identityBasis: "singleOrigin",
    selectedObservationId: "origin-1",
    bundleId: "bundle-takeover",
    memberId: "member-takeover",
    contentId: "content-takeover",
    bundleDisplayName: "example-bundle",
    skillName: "example",
    skillDescription: "test skill",
    sourceDisplayName: null,
    managedDirectory: "/tmp/central/bundles/bundle-takeover",
    contentDirectory:
      "/tmp/central/bundles/bundle-takeover/contents/content-takeover",
    expectedTarget:
      "/tmp/central/bundles/bundle-takeover/current/members/example",
    origins: [
      {
        observationId: "origin-1",
        originalPath: "/tmp/example",
        appId: "codex",
        scope: "global",
        projectId: null,
        projectDisplayName: null,
        contentFingerprint: "fingerprint",
        warnings: [],
        finalDisposition: "mount",
      },
    ],
    targets: [
      {
        mountId: "mount-takeover",
        appId: "codex",
        scope: "global",
        projectId: null,
        projectDisplayName: null,
        targetPath: "/tmp/.codex/skills/example",
        expectedTarget:
          "/tmp/central/bundles/bundle-takeover/current/members/example",
      },
    ],
    warnings: ["包含脚本，SkillYard 不会执行它"],
    createdAt: 1,
    expiresAt: 2,
    ...overrides,
  };
}

function createEntry(
  overrides: Partial<InventoryObservation> = {},
): InventoryObservation {
  return {
    id: "app_global:/tmp/example",
    skillName: "example",
    declaredName: "example",
    skillRoot: "/tmp/example",
    skillFile: "/tmp/example/SKILL.md",
    locationKind: "appGlobal",
    metadataStatus: "valid",
    observedBy: ["codex"],
    observedFingerprint: "fingerprint",
    rootKey: "codexGlobal",
    projectId: null,
    stale: false,
    managementKind: "takeoverCandidate",
    ...overrides,
  };
}
