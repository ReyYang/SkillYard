use serde::Serialize;
use tauri::State;

use crate::{SkillYardApplication, UiIntent, UiOutcome, application::ApplicationError};

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
            ApplicationError::InitialScan(_) => "scanError",
            ApplicationError::InvalidState(_) => "invalidState",
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
}
