//! SkillYard 的内部 Rust Lifecycle Core。

mod application;
mod commands;
mod content;
mod domain;
mod lifecycle;
mod paths;
mod scanner;
mod storage;

pub use application::SkillYardApplication;
pub use domain::{
    FolderInstallPlan, InventoryItem, InventoryLocationKind, InventoryObservation,
    LocalRefreshSummary, ManagementKind, PlatformInfo, RecoveryIssue, ScanIssue, ScanIssueCode,
    ScanRootKey, SkillMetadataStatus, SupportedAppId, UiIntent, UiOutcome,
};
pub use lifecycle::LifecycleFailpoint;
pub use paths::ApplicationPaths;

/// 启动唯一的 SkillYard 桌面应用入口。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let home = dirs::home_dir().expect("当前用户必须有可访问的 home 目录");
    let data_root = home.join("Library/Application Support/SkillYard");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::current(),
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(application)
        .invoke_handler(tauri::generate_handler![
            commands::get_startup_state,
            commands::start_initial_scan,
            commands::refresh_local_inventory,
            commands::choose_folder_install_plan,
            commands::confirm_install_plan
        ])
        .run(tauri::generate_context!())
        .expect("SkillYard.app 运行失败");
}
