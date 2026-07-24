import { invoke } from "@tauri-apps/api/core";

import type {
  BatchMountPlan,
  BatchMountRequest,
  InstallPlan,
  MountPlan,
  MountScope,
  SourceAssociationContentChoice,
  SourceAssociationPlan,
  SourceMemberMappingChoice,
  SupportedAppId,
  TakeoverPlan,
  TakeoverPlanRequest,
  UiOutcome,
} from "./domain";

type InventoryOutcome = Extract<UiOutcome, { type: "inventory" }>;
type BundleUpdateBatchPlanOutcome = Extract<
  UiOutcome,
  { type: "bundleUpdateBatchPlan" }
>;
type BundleUpdateBatchResultOutcome = Extract<
  UiOutcome,
  { type: "bundleUpdateBatchResult" }
>;
type RemovalPlanOutcome = Extract<UiOutcome, { type: "removalPlan" }>;

export interface SkillYardClient {
  getStartupState(): Promise<UiOutcome>;
  startInitialScan(): Promise<UiOutcome>;
  refreshLocalInventory(): Promise<UiOutcome>;
  checkBundleUpdates(): Promise<InventoryOutcome>;
  checkEditableLocalBundle(bundleId: string): Promise<InventoryOutcome>;
  createBundleUpdateBatchPlan(): Promise<BundleUpdateBatchPlanOutcome>;
  confirmBundleUpdateBatchPlan(
    planId: string,
    selectedItemIds: string[],
  ): Promise<BundleUpdateBatchResultOutcome>;
  discardBundleUpdateBatchPlan(planId: string): Promise<InventoryOutcome>;
  acknowledgeBundleUpdateBatchResult(
    batchId: string,
  ): Promise<InventoryOutcome>;
  createProjectRemovalPlan(projectId: string): Promise<RemovalPlanOutcome>;
  createSourceRemovalPlan(sourceId: string): Promise<RemovalPlanOutcome>;
  createBundleRemovalPlan(bundleId: string): Promise<RemovalPlanOutcome>;
  confirmRemovalPlan(planId: string): Promise<UiOutcome>;
  discardRemovalPlan(planId: string): Promise<UiOutcome>;
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
  createSourceAssociationPlan(
    bundleId: string,
    sourceId: string,
    memberChoices: SourceMemberMappingChoice[],
  ): Promise<SourceAssociationPlan>;
  confirmSourceAssociationPlan(
    planId: string,
    contentChoices: SourceAssociationContentChoice[],
  ): Promise<InventoryOutcome>;
  discardSourceAssociationPlan(planId: string): Promise<void>;
  createGithubInstallPlan(sourceId: string): Promise<InstallPlan>;
  createBundleUpdatePlan(bundleId: string): Promise<InstallPlan>;
  chooseBundleReplacementPlan(bundleId: string): Promise<InstallPlan | null>;
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
  checkBundleUpdates: () =>
    invoke<InventoryOutcome>("check_bundle_updates"),
  checkEditableLocalBundle: (bundleId) =>
    invoke<InventoryOutcome>("check_editable_local_bundle", { bundleId }),
  createBundleUpdateBatchPlan: () =>
    invoke<BundleUpdateBatchPlanOutcome>("create_bundle_update_batch_plan"),
  confirmBundleUpdateBatchPlan: (planId, selectedItemIds) =>
    invoke<BundleUpdateBatchResultOutcome>(
      "confirm_bundle_update_batch_plan",
      { planId, selectedItemIds },
    ),
  discardBundleUpdateBatchPlan: (planId) =>
    invoke<InventoryOutcome>("discard_bundle_update_batch_plan", { planId }),
  acknowledgeBundleUpdateBatchResult: (batchId) =>
    invoke<InventoryOutcome>("acknowledge_bundle_update_batch_result", {
      batchId,
    }),
  createProjectRemovalPlan: (projectId) =>
    invoke<RemovalPlanOutcome>("create_project_removal_plan", { projectId }),
  createSourceRemovalPlan: (sourceId) =>
    invoke<RemovalPlanOutcome>("create_source_removal_plan", { sourceId }),
  createBundleRemovalPlan: (bundleId) =>
    invoke<RemovalPlanOutcome>("create_bundle_removal_plan", { bundleId }),
  confirmRemovalPlan: (planId) =>
    invoke<UiOutcome>("confirm_removal_plan", { planId }),
  discardRemovalPlan: (planId) =>
    invoke<UiOutcome>("discard_removal_plan", { planId }),
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
  createSourceAssociationPlan: (bundleId, sourceId, memberChoices) =>
    invoke<SourceAssociationPlan>("create_source_association_plan", {
      bundleId,
      sourceId,
      memberChoices,
    }),
  confirmSourceAssociationPlan: (planId, contentChoices) =>
    invoke<InventoryOutcome>("confirm_source_association_plan", {
      planId,
      contentChoices,
    }),
  discardSourceAssociationPlan: (planId) =>
    invoke<void>("discard_source_association_plan", { planId }),
  createGithubInstallPlan: (sourceId) =>
    invoke<InstallPlan>("create_github_install_plan", { sourceId }),
  createBundleUpdatePlan: (bundleId) =>
    invoke<InstallPlan>("create_bundle_update_plan", { bundleId }),
  chooseBundleReplacementPlan: (bundleId) =>
    invoke<InstallPlan | null>("choose_bundle_replacement_plan", { bundleId }),
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
