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
    });

    await tauriSkillYardClient.startInitialScan();

    expect(mocks.invoke).toHaveBeenCalledWith("start_initial_scan");
  });
});
