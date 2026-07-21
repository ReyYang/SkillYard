use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::{
    FolderInstallPlan, SkillYardApplication, UiIntent, UiOutcome, application::ApplicationError,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiError {
    code: &'static str,
    message: String,
}

impl From<ApplicationError> for UiError {
    fn from(error: ApplicationError) -> Self {
        let code = match &error {
            ApplicationError::Storage(_) => "storageError",
            ApplicationError::Lifecycle(_) => "lifecycleError",
            ApplicationError::InitialScan(_) => "scanError",
            ApplicationError::InvalidState(_) => "invalidState",
            ApplicationError::OperationInProgress => "operationInProgress",
            ApplicationError::OperationGateUnavailable => "operationGateUnavailable",
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

/// 同步扫描由 Tauri 放入异步任务，避免阻塞 WKWebView 主线程。
#[tauri::command(async)]
pub fn get_startup_state(
    application: State<'_, SkillYardApplication>,
) -> Result<UiOutcome, UiError> {
    dispatch(&application, UiIntent::GetStartupState)
}

#[tauri::command(async)]
pub fn start_initial_scan(
    application: State<'_, SkillYardApplication>,
) -> Result<UiOutcome, UiError> {
    dispatch(&application, UiIntent::StartInitialScan)
}

#[tauri::command(async)]
pub fn refresh_local_inventory(
    application: State<'_, SkillYardApplication>,
) -> Result<UiOutcome, UiError> {
    dispatch(&application, UiIntent::RefreshLocalInventory)
}

/// 文件夹路径只由 Rust 原生选择器取得，前端不能提交任意本机路径。
#[tauri::command(async)]
pub fn choose_folder_install_plan(
    app: AppHandle,
    application: State<'_, SkillYardApplication>,
) -> Result<Option<FolderInstallPlan>, UiError> {
    let Some(folder) = app
        .dialog()
        .file()
        .set_title("选择包含 SKILL.md 的 Skill 文件夹")
        .blocking_pick_folder()
    else {
        return Ok(None);
    };
    let path = folder.into_path().map_err(|error| UiError {
        code: "dialogError",
        message: format!("无法读取所选文件夹：{error}"),
    })?;
    let input_path = path.to_str().ok_or_else(|| UiError {
        code: "invalidPath",
        message: "所选文件夹名称包含 SkillYard 1.0 无法保存的字符".to_owned(),
    })?;
    match dispatch(
        &application,
        UiIntent::CreateFolderInstallPlan {
            input_path: input_path.to_owned(),
        },
    )? {
        UiOutcome::FolderInstallPlan { plan } => Ok(Some(plan)),
        _ => Err(UiError {
            code: "invalidOutcome",
            message: "SkillYard 没有生成安装确认信息".to_owned(),
        }),
    }
}

#[tauri::command(async)]
pub fn confirm_install_plan(
    application: State<'_, SkillYardApplication>,
    plan_id: String,
) -> Result<UiOutcome, UiError> {
    dispatch(&application, UiIntent::ConfirmInstallPlan { plan_id })
}

fn dispatch(application: &SkillYardApplication, intent: UiIntent) -> Result<UiOutcome, UiError> {
    application.handle(intent).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::{ApplicationPaths, PlatformInfo};

    #[test]
    fn ipc_dispatch_returns_the_same_outcome_as_the_application_seam() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let paths =
            ApplicationPaths::for_home(sandbox.path().join("data"), sandbox.path().join("home"));
        let application = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());

        let expected = application
            .handle(UiIntent::GetStartupState)
            .expect("application seam 应返回首次使用状态");
        let actual = dispatch(&application, UiIntent::GetStartupState)
            .expect("IPC dispatch 应返回首次使用状态");

        assert_eq!(actual, expected);
    }

    #[test]
    fn ipc_dispatch_keeps_the_typed_application_error() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let home = sandbox.path().join("home");
        fs::create_dir_all(home.join(".codex")).expect("应创建测试父目录");
        fs::write(home.join(".codex/skills"), "not a directory").expect("应创建非法扫描根");
        let application = SkillYardApplication::new(
            ApplicationPaths::for_home(sandbox.path().join("data"), home),
            PlatformInfo::supported_for_test(),
        );

        let expected = application
            .handle(UiIntent::StartInitialScan)
            .expect_err("application seam 应拒绝非法扫描根")
            .to_string();
        let actual = dispatch(&application, UiIntent::StartInitialScan)
            .expect_err("IPC dispatch 应拒绝非法扫描根");

        assert_eq!(actual.code, "scanError");
        assert_eq!(actual.message, expected);
    }

    #[test]
    fn ipc_dispatch_maps_refresh_before_onboarding_to_invalid_state() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let application = SkillYardApplication::new(
            ApplicationPaths::for_home(sandbox.path().join("data"), sandbox.path().join("home")),
            PlatformInfo::supported_for_test(),
        );

        let error = dispatch(&application, UiIntent::RefreshLocalInventory)
            .expect_err("首次扫描前不能刷新本机");

        assert_eq!(error.code, "invalidState");
    }

    #[test]
    fn ipc_dispatch_keeps_lifecycle_errors_typed() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let application = SkillYardApplication::new(
            ApplicationPaths::for_home(sandbox.path().join("data"), sandbox.path().join("home")),
            PlatformInfo::supported_for_test(),
        );
        dispatch(&application, UiIntent::StartInitialScan).expect("应完成首次扫描");

        let error = dispatch(
            &application,
            UiIntent::ConfirmInstallPlan {
                plan_id: "unknown".to_owned(),
            },
        )
        .expect_err("未知 Plan 应保留生命周期错误类型");

        assert_eq!(error.code, "lifecycleError");
        assert!(error.message.contains("未签发"));
    }
}
