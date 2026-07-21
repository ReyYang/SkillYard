import { invoke } from "@tauri-apps/api/core";

import type {
  FolderInstallPlan,
  MountPlan,
  MountScope,
  SupportedAppId,
  UiOutcome,
} from "./domain";

export interface SkillYardClient {
  getStartupState(): Promise<UiOutcome>;
  startInitialScan(): Promise<UiOutcome>;
  refreshLocalInventory(): Promise<UiOutcome>;
  chooseFolderInstallPlan(): Promise<FolderInstallPlan | null>;
  confirmInstallPlan(
    planId: string,
    selectedCandidateIds: string[],
  ): Promise<UiOutcome>;
  chooseAndRegisterProject(): Promise<UiOutcome | null>;
  createMountPlan(
    memberId: string,
    appId: SupportedAppId,
    scope: MountScope,
    projectId: string | null,
  ): Promise<MountPlan>;
  createRemoveMountPlan(mountId: string): Promise<MountPlan>;
  createRepairMountPlan(mountId: string): Promise<MountPlan>;
  confirmMountPlan(planId: string): Promise<UiOutcome>;
}

// 前端只知道任务级命令；文件夹路径由 Rust 原生选择器产生，不开放通用文件能力。
export const tauriSkillYardClient: SkillYardClient = {
  getStartupState: () => invoke<UiOutcome>("get_startup_state"),
  startInitialScan: () => invoke<UiOutcome>("start_initial_scan"),
  refreshLocalInventory: () => invoke<UiOutcome>("refresh_local_inventory"),
  chooseFolderInstallPlan: () =>
    invoke<FolderInstallPlan | null>("choose_folder_install_plan"),
  confirmInstallPlan: (planId, selectedCandidateIds) =>
    invoke<UiOutcome>("confirm_install_plan", {
      planId,
      selectedCandidateIds,
    }),
  chooseAndRegisterProject: () =>
    invoke<UiOutcome | null>("choose_and_register_project"),
  createMountPlan: (memberId, appId, scope, projectId) =>
    invoke<MountPlan>("create_mount_plan", {
      memberId,
      appId,
      scope,
      projectId,
    }),
  createRemoveMountPlan: (mountId) =>
    invoke<MountPlan>("create_remove_mount_plan", { mountId }),
  createRepairMountPlan: (mountId) =>
    invoke<MountPlan>("create_repair_mount_plan", { mountId }),
  confirmMountPlan: (planId) =>
    invoke<UiOutcome>("confirm_mount_plan", { planId }),
};
