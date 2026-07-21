import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import type {
  BatchMountPlan,
  FolderInstallPlan,
  InventoryObservation,
  MountPlan,
  MountSummary,
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
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(createFolderInstallPlan({
      inputPath: "/Users/test/Downloads/example",
      warnings: ["包含可执行文件，请确认来源可信"],
    }));
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));

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
      createFolderInstallPlan({
        bundleDisplayName: "superpowers",
        candidates: [
          createFolderCandidate({
            candidateId: "candidate-brainstorming",
            sourceRelativePath: "skills/brainstorming",
            skillName: "brainstorming",
          }),
          createFolderCandidate({
            candidateId: "candidate-tdd",
            sourceRelativePath: "skills/tdd",
            skillName: "tdd",
          }),
        ],
      }),
    );
    vi.mocked(client.confirmInstallPlan).mockResolvedValue(inventoryOutcome([]));
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
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
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(createFolderInstallPlan());
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.click(screen.getByRole("checkbox", { name: /example/ }));

    expect(screen.getByText(/至少选择一个有效 Skill/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认安装" })).toBeDisabled();
    expect(client.confirmInstallPlan).not.toHaveBeenCalled();
  });

  it("无效候选展示具体错误且不可选择，有效候选仍可安装", async () => {
    const user = userEvent.setup();
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(
      createFolderInstallPlan({
        candidates: [
          createFolderCandidate({
            candidateId: "candidate-valid",
            sourceRelativePath: "skills/valid",
            skillName: "valid",
          }),
          createFolderCandidate({
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

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));

    expect(screen.getByRole("checkbox", { name: /valid/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /broken/ })).toBeDisabled();
    expect(screen.getByText("SKILL.md YAML frontmatter 无法解析")).toBeInTheDocument();
  });

  it("确认期间禁止取消或重复提交，成功后显示受管 Bundle", async () => {
    const user = userEvent.setup();
    let finishInstall: ((outcome: UiOutcome) => void) | undefined;
    const client = createClient(inventoryOutcome([]));
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(createFolderInstallPlan());
    vi.mocked(client.confirmInstallPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishInstall = resolve;
        }),
    );
    render(<App client={client} />);
    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
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
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(createFolderInstallPlan());
    vi.mocked(client.confirmInstallPlan).mockRejectedValue({
      code: "lifecycleError",
      message: "安装中断，已自动恢复",
    });
    vi.mocked(client.getStartupState)
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(recovered);
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));
    await user.click(screen.getByRole("button", { name: "确认安装" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "安装中断，已自动恢复",
    );
    expect(client.getStartupState).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("button", { name: "确认安装" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "example" })).toBeInTheDocument();
  });

  it("取消原生选择器时保持当前清单", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([createEntry({ skillName: "preserved" })]),
    );
    vi.mocked(client.chooseFolderInstallPlan).mockResolvedValue(null);
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "安装 Skill" }));

    expect(screen.getByText("preserved")).toBeInTheDocument();
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

  it("只有待接管条目显示接管入口，Plan 创建失败时保留清单", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([
        createEntry({ id: "candidate", skillName: "local-copy" }),
        createEntry({
          id: "agent",
          skillName: "plugin-copy",
          managementKind: "agentManaged",
        }),
        createEntry({
          id: "project",
          skillName: "project-copy",
          managementKind: "projectManaged",
        }),
        createManagedEntry({ id: "managed", skillName: "managed-copy" }),
      ]),
    );
    vi.mocked(client.createTakeoverPlan).mockRejectedValue({
      code: "takeoverError",
      message: "原路径已经变化，请刷新后重试",
    });
    render(<App client={client} />);

    const takeoverSection = await screen.findByRole("region", { name: "待接管" });
    expect(within(takeoverSection).getByRole("button", { name: "接管" })).toBeEnabled();
    expect(screen.getAllByRole("button", { name: "接管" })).toHaveLength(1);

    await user.click(within(takeoverSection).getByRole("button", { name: "接管" }));

    expect(client.createTakeoverPlan).toHaveBeenCalledWith("candidate");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "原路径已经变化，请刷新后重试",
    );
    expect(screen.getByText("local-copy")).toBeInTheDocument();
    expect(screen.getByText("plugin-copy")).toBeInTheDocument();
    expect(client.confirmTakeoverPlan).not.toHaveBeenCalled();
  });

  it("接管预览展示完整影响，按默认值选择并只提交 opaque 路径 ID", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([createEntry({ id: "candidate", skillName: "alpha" })]),
    );
    vi.mocked(client.createTakeoverPlan).mockResolvedValue(
      createTakeoverPlan({
        warnings: ["这个 Skill 包含可执行脚本，请确认来源可信"],
      }),
    );
    vi.mocked(client.confirmTakeoverPlan).mockResolvedValue(
      inventoryOutcome([
        createManagedEntry({
          skillName: "alpha",
          bundleDisplayName: "alpha-bundle",
        }),
      ]),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "接管" }));

    const preview = screen.getByLabelText("接管影响预览");
    expect(preview).toHaveTextContent("alpha-bundle");
    expect(preview).toHaveTextContent("alpha");
    expect(preview).toHaveTextContent("来源未知；没有更新来源");
    expect(preview).toHaveTextContent("/tmp/.codex/skills/alpha");
    expect(preview).toHaveTextContent("Codex · 全局");
    expect(preview).toHaveTextContent("/tmp/central/bundles/bundle-alpha");
    expect(preview).toHaveTextContent("这个 Skill 包含可执行脚本");
    expect(screen.getByRole("button", { name: "返回" })).toBeEnabled();

    const preserve = screen.getByRole("checkbox", { name: /Codex · 全局/ });
    expect(preserve).toBeChecked();
    await user.click(preserve);
    expect(preserve).not.toBeChecked();
    await user.click(screen.getByRole("button", { name: "确认接管" }));

    expect(client.confirmTakeoverPlan).toHaveBeenCalledWith(
      "takeover-plan-1",
      [],
    );
    expect(
      await screen.findByRole("region", { name: "alpha-bundle" }),
    ).toHaveTextContent("alpha-bundle: alpha");
  });

  it("不修改默认选择时保留并提交已有使用位置", async () => {
    const user = userEvent.setup();
    const client = createClient(
      inventoryOutcome([createEntry({ id: "candidate", skillName: "alpha" })]),
    );
    vi.mocked(client.createTakeoverPlan).mockResolvedValue(createTakeoverPlan());
    vi.mocked(client.confirmTakeoverPlan).mockResolvedValue(inventoryOutcome([]));
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "接管" }));
    expect(screen.getByRole("checkbox", { name: /Codex · 全局/ })).toBeChecked();
    await user.click(screen.getByRole("button", { name: "确认接管" }));

    expect(client.confirmTakeoverPlan).toHaveBeenCalledWith(
      "takeover-plan-1",
      ["takeover-path-1"],
    );
  });

  it("接管确认开始后禁用返回、选择和重复提交", async () => {
    const user = userEvent.setup();
    let finishTakeover: ((outcome: UiOutcome) => void) | undefined;
    const client = createClient(
      inventoryOutcome([createEntry({ id: "candidate", skillName: "alpha" })]),
    );
    vi.mocked(client.createTakeoverPlan).mockResolvedValue(createTakeoverPlan());
    vi.mocked(client.confirmTakeoverPlan).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishTakeover = resolve;
        }),
    );
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "接管" }));
    await user.click(screen.getByRole("button", { name: "确认接管" }));

    expect(screen.getByRole("checkbox", { name: /Codex · 全局/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: "返回" })).toBeDisabled();
    const confirming = screen.getByRole("button", { name: "正在安全接管…" });
    expect(confirming).toBeDisabled();
    await user.click(confirming);
    expect(client.confirmTakeoverPlan).toHaveBeenCalledTimes(1);

    await act(async () => {
      finishTakeover?.(inventoryOutcome([]));
    });
    expect(screen.getByRole("heading", { name: "未发现 Skill" })).toBeInTheDocument();
  });

  it("接管失败后丢弃旧 Plan，重读 blocked 状态并复用恢复提示", async () => {
    const user = userEvent.setup();
    const initial = inventoryOutcome([
      createEntry({ id: "candidate", skillName: "alpha" }),
    ]);
    const blocked = inventoryOutcome(
      [createEntry({ id: "candidate", skillName: "alpha" })],
      null,
      {
        recoveryIssues: [
          {
            id: "takeover-transaction-1",
            bundleDisplayName: "alpha-bundle",
            message: "原路径被外部修改，需要人工恢复",
          },
        ],
      },
    );
    const client = createClient(initial);
    vi.mocked(client.createTakeoverPlan).mockResolvedValue(createTakeoverPlan());
    vi.mocked(client.confirmTakeoverPlan).mockRejectedValue({
      code: "takeoverError",
      message: "接管中断，相关 Skill 已停止修改",
    });
    vi.mocked(client.getStartupState)
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(blocked);
    render(<App client={client} />);

    await user.click(await screen.findByRole("button", { name: "接管" }));
    await user.click(screen.getByRole("button", { name: "确认接管" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "接管中断，相关 Skill 已停止修改",
    );
    expect(client.getStartupState).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("button", { name: "确认接管" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("需要人工恢复")).toHaveTextContent(
      "原路径被外部修改，需要人工恢复",
    );
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

function createFolderInstallPlan(
  overrides: Partial<FolderInstallPlan> = {},
): FolderInstallPlan {
  return {
    id: "plan-1",
    inputPath: "/tmp/example",
    bundleDisplayName: "example",
    candidates: [createFolderCandidate()],
    warnings: [],
    willMount: false,
    createdAt: 1,
    expiresAt: 2,
    ...overrides,
  };
}

function createFolderCandidate(
  overrides: Partial<FolderInstallPlan["candidates"][number]> = {},
): FolderInstallPlan["candidates"][number] {
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

function createClient(startup: UiOutcome): SkillYardClient {
  return {
    getStartupState: vi.fn().mockResolvedValue(startup),
    startInitialScan: vi.fn(),
    refreshLocalInventory: vi.fn(),
    chooseFolderInstallPlan: vi.fn(),
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

function createTakeoverPlan(
  overrides: Partial<TakeoverPlan> = {},
): TakeoverPlan {
  return {
    id: "takeover-plan-1",
    observationId: "candidate",
    bundleId: "bundle-alpha",
    contentId: "content-alpha",
    memberId: "member-alpha",
    bundleDisplayName: "alpha-bundle",
    sourceDisplayName: null,
    sourceNotice: "来源未知；没有更新来源",
    skillName: "alpha",
    skillDescription: "alpha description",
    contentFingerprint: "fingerprint",
    warnings: [],
    managedDirectory: "/tmp/central/bundles/bundle-alpha",
    contentDirectory: "/tmp/central/contents/content-alpha",
    expectedTarget:
      "/tmp/central/bundles/bundle-alpha/current/members/alpha",
    paths: [
      {
        id: "takeover-path-1",
        mountId: "mount-alpha",
        originalPath: "/tmp/.codex/skills/alpha",
        appId: "codex",
        scope: "global",
        projectId: null,
        projectDisplayName: null,
        projectRootPath: null,
        projectRootDevice: null,
        projectRootInode: null,
        parentDevice: 1,
        parentInode: 2,
        parentMode: 16_877,
        originalDevice: 1,
        originalInode: 3,
        originalMode: 16_877,
        defaultPreserveMount: true,
      },
    ],
    createdAt: 1,
    expiresAt: 2,
    ...overrides,
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
