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
});
