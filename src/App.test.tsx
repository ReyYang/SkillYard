import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import type {
  BatchMountPlan,
  InstallPlan,
  InventoryObservation,
  MountPlan,
  MountSummary,
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

  it("确认期间禁止取消或重复提交，成功后显示受管 Bundle", async () => {
    const user = userEvent.setup();
    let finishInstall: ((outcome: UiOutcome) => void) | undefined;
    const client = createClient(inventoryOutcome([]));
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
    await user.click(screen.getByRole("button", { name: "正在安全安装…" }));
    expect(client.confirmInstallPlan).toHaveBeenCalledTimes(1);

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

  it("人工恢复只提示相关 Bundle，同时保留其他清单浏览", async () => {
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

  it("添加同一 Source 的不同 Ref 时先确认，确认后再显示新 Ref", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([]));
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
    expect(screen.getByText("Tracked Ref: next")).toBeInTheDocument();
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

function createClient(startup: UiOutcome): SkillYardClient {
  return {
    getStartupState: vi.fn().mockResolvedValue(startup),
    startInitialScan: vi.fn(),
    refreshLocalInventory: vi.fn(),
    openSourceDiscovery: vi.fn().mockResolvedValue(sourceDiscoveryOutcome()),
    searchSkillsSh: vi.fn(),
    reloadGithubSource: vi.fn(),
    addGithubSource: vi.fn(),
    confirmSourceRefChange: vi.fn(),
    createGithubInstallPlan: vi.fn(),
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
