//! SkillYard 的内部 Rust Lifecycle Core。

mod application;
mod bundle_update_batch;
mod commands;
mod content;
mod domain;
mod git_management_evidence;
mod github_source;
mod installation_chain;
mod lifecycle;
mod mount_lifecycle;
mod paths;
mod removal;
mod scanner;
mod skills_sh;
mod source_archive;
mod source_association;
mod source_input;
mod storage;
mod takeover;

pub use application::SkillYardApplication;
pub use domain::{
    BatchMountDisposition, BatchMountPlan, BatchMountPlanItem, BatchMountRequest,
    BundleUpdateAction, BundleUpdateBatchPlan, BundleUpdateBatchPlanItem,
    BundleUpdateBatchPlanItemDisposition, BundleUpdateBatchResult, BundleUpdateBatchResultItem,
    BundleUpdateBatchResultItemStatus, BundleUpdateBatchResultStatus, BundleUpdateImpact,
    BundleUpdateStatus, BundleUpdateSummary, EditableLocalRelinkMember, EditableLocalRelinkPlan,
    InstallCandidate, InstallInputKind, InstallMode, InstallPlan, InstallationChain,
    InstallationChainKind, InventoryItem, InventoryLocationKind, InventoryObservation,
    LocalRefreshSummary, ManagementEvidence, ManagementEvidenceKind, ManagementKind,
    MergeContentChoice, MountHealth, MountOperation, MountPlan, MountPlanPurpose, MountScope,
    MountSummary, PlatformInfo, ProjectSummary, RecoveryIssue, RemovalBundleSummary, RemovalKind,
    RemovalMemberSummary, RemovalPlan, RemovalPreservedSource, ScanIssue, ScanIssueCode,
    ScanRootKey, SkillMetadataStatus, SkillsShSearchMember, SkillsShSearchSource,
    SourceAssociationConflict, SourceAssociationMember, SourceAssociationMemberChoice,
    SourceAssociationMode, SourceAssociationPlan, SourceCatalogMemberSummary, SourceCatalogStatus,
    SourceKind, SourceMemberMappingChoice, SourceRefChangePlan, SourceSummary, SupportedAppId,
    TakeoverIdentityBasis, TakeoverMemberRequest, TakeoverOriginDisposition, TakeoverPlan,
    TakeoverPlanMember, TakeoverPlanOrigin, TakeoverPlanRequest, TakeoverPlanTarget,
    TakeoverSharedTargetRequest, UiIntent, UiOutcome,
};
pub use github_source::{
    GithubSourceError, ReqwestSourceTransport, ResolvedGithubSource, SharedSourceTransport,
    SourceRequest, SourceResponse, SourceTransport, SourceTransportError, parse_github_source,
    resolve_github_source,
};
pub use lifecycle::LifecycleFailpoint;
pub use paths::ApplicationPaths;

/// 启动唯一的 SkillYard 桌面应用入口。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let home = dirs::home_dir().expect("当前用户必须有可访问的 home 目录");
    let data_root = home.join("Library/Application Support/SkillYard");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_current_user(data_root, home),
        PlatformInfo::current(),
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(application)
        .invoke_handler(tauri::generate_handler![
            commands::get_startup_state,
            commands::open_central_store,
            commands::start_initial_scan,
            commands::refresh_local_inventory,
            commands::check_bundle_updates,
            commands::check_editable_local_bundle,
            commands::open_source_discovery,
            commands::search_skills_sh,
            commands::reload_github_source,
            commands::add_github_source,
            commands::confirm_source_ref_change,
            commands::choose_editable_local_relink_plan,
            commands::confirm_editable_local_relink_plan,
            commands::discard_editable_local_relink_plan,
            commands::create_github_install_plan,
            commands::create_bundle_update_plan,
            commands::create_bundle_update_batch_plan,
            commands::confirm_bundle_update_batch_plan,
            commands::discard_bundle_update_batch_plan,
            commands::acknowledge_bundle_update_batch_result,
            commands::choose_bundle_replacement_plan,
            commands::choose_folder_install_plan,
            commands::choose_archive_install_plan,
            commands::create_url_install_plan,
            commands::choose_editable_local_install_plan,
            commands::confirm_install_plan,
            commands::discard_install_plan,
            commands::create_source_association_plan,
            commands::confirm_source_association_plan,
            commands::discard_source_association_plan,
            commands::choose_and_register_project,
            commands::create_takeover_plan,
            commands::confirm_takeover_plan,
            commands::create_mount_plan,
            commands::create_remove_mount_plan,
            commands::create_repair_mount_plan,
            commands::confirm_mount_plan,
            commands::create_batch_mount_plan,
            commands::confirm_batch_mount_plan,
            commands::create_project_removal_plan,
            commands::create_source_removal_plan,
            commands::create_bundle_removal_plan,
            commands::confirm_removal_plan,
            commands::discard_removal_plan
        ])
        .run(tauri::generate_context!())
        .expect("SkillYard.app 运行失败");
}
