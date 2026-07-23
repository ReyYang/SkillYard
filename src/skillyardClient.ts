import { invoke } from "@tauri-apps/api/core";

import type {
  BatchMountPlan,
  BatchMountRequest,
  InstallPlan,
  MountPlan,
  MountScope,
  SupportedAppId,
  TakeoverPlan,
  TakeoverPlanRequest,
  UiOutcome,
} from "./domain";

export interface SkillYardClient {
  getStartupState(): Promise<UiOutcome>;
  startInitialScan(): Promise<UiOutcome>;
  refreshLocalInventory(): Promise<UiOutcome>;
  openSourceDiscovery(): Promise<
    Extract<UiOutcome, { type: "sourceDiscovery" }>
  >;
  searchSkillsSh(
    query: string,
  ): Promise<Extract<UiOutcome, { type: "skillsShSearch" }>>;
  reloadGithubSource(
    sourceId: string,
  ): Promise<Extract<UiOutcome, { type: "sourceDiscovery" }>>;
  addGithubSource(
    input: string,
    trackedRef: string | null,
  ): Promise<
    Extract<
      UiOutcome,
      { type: "sourceDiscovery" | "sourceRefChangePlan" }
    >
  >;
  confirmSourceRefChange(
    planId: string,
  ): Promise<Extract<UiOutcome, { type: "sourceDiscovery" }>>;
  createGithubInstallPlan(sourceId: string): Promise<InstallPlan>;
  createUrlInstallPlan(url: string): Promise<InstallPlan>;
  discardInstallPlan(planId: string): Promise<void>;
  chooseFolderInstallPlan(): Promise<InstallPlan | null>;
  chooseArchiveInstallPlan(): Promise<InstallPlan | null>;
  chooseEditableLocalInstallPlan(): Promise<InstallPlan | null>;
  confirmInstallPlan(
    planId: string,
    selectedCandidateIds: string[],
  ): Promise<UiOutcome>;
  chooseAndRegisterProject(): Promise<UiOutcome | null>;
  createTakeoverPlan(request: TakeoverPlanRequest): Promise<TakeoverPlan>;
  confirmTakeoverPlan(planId: string): Promise<UiOutcome>;
  createMountPlan(
    memberId: string,
    appId: SupportedAppId,
    scope: MountScope,
    projectId: string | null,
  ): Promise<MountPlan>;
  createRemoveMountPlan(mountId: string): Promise<MountPlan>;
  createRepairMountPlan(mountId: string): Promise<MountPlan>;
  confirmMountPlan(planId: string): Promise<UiOutcome>;
  createBatchMountPlan(
    bundleId: string,
    requests: BatchMountRequest[],
  ): Promise<BatchMountPlan>;
  confirmBatchMountPlan(
    planId: string,
    selectedItemIds: string[],
  ): Promise<UiOutcome>;
}

// 前端只知道任务级命令；文件夹路径由 Rust 原生选择器产生，不开放通用文件能力。
export const tauriSkillYardClient: SkillYardClient = {
  getStartupState: () => invoke<UiOutcome>("get_startup_state"),
  startInitialScan: () => invoke<UiOutcome>("start_initial_scan"),
  refreshLocalInventory: () => invoke<UiOutcome>("refresh_local_inventory"),
  openSourceDiscovery: () =>
    invoke<Extract<UiOutcome, { type: "sourceDiscovery" }>>(
      "open_source_discovery",
    ),
  searchSkillsSh: (query) =>
    invoke<Extract<UiOutcome, { type: "skillsShSearch" }>>(
      "search_skills_sh",
      { query },
    ),
  reloadGithubSource: (sourceId) =>
    invoke<Extract<UiOutcome, { type: "sourceDiscovery" }>>(
      "reload_github_source",
      { sourceId },
    ),
  addGithubSource: (input, trackedRef) =>
    invoke<
      Extract<
        UiOutcome,
        { type: "sourceDiscovery" | "sourceRefChangePlan" }
      >
    >("add_github_source", { input, trackedRef }),
  confirmSourceRefChange: (planId) =>
    invoke<Extract<UiOutcome, { type: "sourceDiscovery" }>>(
      "confirm_source_ref_change",
      { planId },
    ),
  createGithubInstallPlan: (sourceId) =>
    invoke<InstallPlan>("create_github_install_plan", { sourceId }),
  createUrlInstallPlan: (url) =>
    invoke<InstallPlan>("create_url_install_plan", { url }),
  discardInstallPlan: (planId) =>
    invoke<void>("discard_install_plan", { planId }),
  chooseFolderInstallPlan: () =>
    invoke<InstallPlan | null>("choose_folder_install_plan"),
  chooseArchiveInstallPlan: () =>
    invoke<InstallPlan | null>("choose_archive_install_plan"),
  chooseEditableLocalInstallPlan: () =>
    invoke<InstallPlan | null>("choose_editable_local_install_plan"),
  confirmInstallPlan: (planId, selectedCandidateIds) =>
    invoke<UiOutcome>("confirm_install_plan", {
      planId,
      selectedCandidateIds,
    }),
  chooseAndRegisterProject: () =>
    invoke<UiOutcome | null>("choose_and_register_project"),
  createTakeoverPlan: (request) =>
    invoke<TakeoverPlan>("create_takeover_plan", { request }),
  confirmTakeoverPlan: (planId) =>
    invoke<UiOutcome>("confirm_takeover_plan", { planId }),
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
  createBatchMountPlan: (bundleId, requests) =>
    invoke<BatchMountPlan>("create_batch_mount_plan", {
      bundleId,
      requests,
    }),
  confirmBatchMountPlan: (planId, selectedItemIds) =>
    invoke<UiOutcome>("confirm_batch_mount_plan", {
      planId,
      selectedItemIds,
    }),
};
