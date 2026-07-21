import { invoke } from "@tauri-apps/api/core";

import type { UiOutcome } from "./domain";

export interface SkillYardClient {
  getStartupState(): Promise<UiOutcome>;
  startInitialScan(): Promise<UiOutcome>;
  refreshLocalInventory(): Promise<UiOutcome>;
}

// 前端只知道两个任务级命令，不获得通用文件、SQL 或 shell 能力。
export const tauriSkillYardClient: SkillYardClient = {
  getStartupState: () => invoke<UiOutcome>("get_startup_state"),
  startInitialScan: () => invoke<UiOutcome>("start_initial_scan"),
  refreshLocalInventory: () => invoke<UiOutcome>("refresh_local_inventory"),
};
