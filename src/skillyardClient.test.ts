import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import { tauriSkillYardClient } from "./skillyardClient";

describe("Tauri IPC contract", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
  });

  it("只通过任务级命令读取启动状态", async () => {
    mocks.invoke.mockResolvedValue({
      type: "onboardingRequired",
      supportedApps: [],
    });

    await tauriSkillYardClient.getStartupState();

    expect(mocks.invoke).toHaveBeenCalledWith("get_startup_state");
  });

  it("只通过任务级命令开始首次扫描", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
    });

    await tauriSkillYardClient.startInitialScan();

    expect(mocks.invoke).toHaveBeenCalledWith("start_initial_scan");
  });

  it("只通过任务级命令刷新本机清单", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
    });

    await tauriSkillYardClient.refreshLocalInventory();

    expect(mocks.invoke).toHaveBeenCalledWith("refresh_local_inventory");
  });

  it("只通过 Rust 任务命令打开文件夹选择器", async () => {
    mocks.invoke.mockResolvedValue(null);

    await tauriSkillYardClient.chooseFolderInstallPlan();

    expect(mocks.invoke).toHaveBeenCalledWith("choose_folder_install_plan");
  });

  it("确认时只把 opaque Plan 与候选 ID 交回 Rust", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
    });

    await tauriSkillYardClient.confirmInstallPlan("plan-1", ["candidate-1"]);

    expect(mocks.invoke).toHaveBeenCalledWith("confirm_install_plan", {
      planId: "plan-1",
      selectedCandidateIds: ["candidate-1"],
    });
  });

  it("只通过原生任务命令选择并登记 Project", async () => {
    mocks.invoke.mockResolvedValue(null);

    await tauriSkillYardClient.chooseAndRegisterProject();

    expect(mocks.invoke).toHaveBeenCalledWith("choose_and_register_project");
  });

  it("创建 Mount Plan 时提交明确的 member、应用和 scope", async () => {
    mocks.invoke.mockResolvedValue({ id: "mount-plan-1" });

    await tauriSkillYardClient.createMountPlan(
      "member-1",
      "codex",
      "project",
      "project-1",
    );

    expect(mocks.invoke).toHaveBeenCalledWith("create_mount_plan", {
      memberId: "member-1",
      appId: "codex",
      scope: "project",
      projectId: "project-1",
    });
  });

  it("移除 Mount 也必须先创建 opaque Plan", async () => {
    mocks.invoke.mockResolvedValue({ id: "remove-plan-1" });

    await tauriSkillYardClient.createRemoveMountPlan("mount-1");

    expect(mocks.invoke).toHaveBeenCalledWith("create_remove_mount_plan", {
      mountId: "mount-1",
    });
  });

  it("修复 Mount 也必须先创建 opaque Plan", async () => {
    mocks.invoke.mockResolvedValue({ id: "repair-plan-1" });

    await tauriSkillYardClient.createRepairMountPlan("mount-1");

    expect(mocks.invoke).toHaveBeenCalledWith("create_repair_mount_plan", {
      mountId: "mount-1",
    });
  });

  it("确认 Mount 事务时只提交 opaque Plan ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
      projects: [],
      mounts: [],
    });

    await tauriSkillYardClient.confirmMountPlan("mount-plan-1");

    expect(mocks.invoke).toHaveBeenCalledWith("confirm_mount_plan", {
      planId: "mount-plan-1",
    });
  });
});
