use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::{
    BatchMountPlan, BatchMountRequest, InstallPlan, MountPlan, MountScope, SkillYardApplication,
    SupportedAppId, TakeoverPlan, TakeoverPlanRequest, UiIntent, UiOutcome,
    application::ApplicationError,
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
            ApplicationError::MountLifecycle(_) => "mountError",
            ApplicationError::Takeover(_) => "takeoverError",
            ApplicationError::GithubSource(_) => "sourceError",
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

#[tauri::command(async)]
pub fn open_source_discovery(
    application: State<'_, SkillYardApplication>,
) -> Result<UiOutcome, UiError> {
    match dispatch(&application, UiIntent::OpenSourceDiscovery)? {
        outcome @ UiOutcome::SourceDiscovery { .. } => Ok(outcome),
        _ => Err(invalid_outcome("SkillYard 没有返回 Source 列表")),
    }
}

#[tauri::command(async)]
pub fn reload_github_source(
    application: State<'_, SkillYardApplication>,
    source_id: String,
) -> Result<UiOutcome, UiError> {
    match dispatch(&application, UiIntent::ReloadGitHubSource { source_id })? {
        outcome @ UiOutcome::SourceDiscovery { .. } => Ok(outcome),
        _ => Err(invalid_outcome(
            "SkillYard 没有返回重新加载后的 Source 列表",
        )),
    }
}

#[tauri::command(async)]
pub fn add_github_source(
    application: State<'_, SkillYardApplication>,
    input: String,
    tracked_ref: Option<String>,
) -> Result<UiOutcome, UiError> {
    match dispatch(
        &application,
        UiIntent::AddGitHubSource { input, tracked_ref },
    )? {
        outcome @ (UiOutcome::SourceDiscovery { .. } | UiOutcome::SourceRefChangePlan { .. }) => {
            Ok(outcome)
        }
        _ => Err(invalid_outcome(
            "SkillYard 没有返回 Source 或 Tracked Ref 变更确认",
        )),
    }
}

#[tauri::command(async)]
pub fn confirm_source_ref_change(
    application: State<'_, SkillYardApplication>,
    plan_id: String,
) -> Result<UiOutcome, UiError> {
    match dispatch(&application, UiIntent::ConfirmSourceRefChange { plan_id })? {
        outcome @ UiOutcome::SourceDiscovery { .. } => Ok(outcome),
        _ => Err(invalid_outcome("SkillYard 没有返回更新后的 Source 列表")),
    }
}

#[tauri::command(async)]
pub fn create_github_install_plan(
    application: State<'_, SkillYardApplication>,
    source_id: String,
) -> Result<InstallPlan, UiError> {
    match dispatch(
        &application,
        UiIntent::CreateGithubInstallPlan { source_id },
    )? {
        UiOutcome::InstallPlan { plan } => Ok(plan),
        _ => Err(invalid_outcome("SkillYard 没有生成 GitHub 安装确认信息")),
    }
}

/// 文件夹路径只由 Rust 原生选择器取得，前端不能提交任意本机路径。
#[tauri::command(async)]
pub fn choose_folder_install_plan(
    app: AppHandle,
    application: State<'_, SkillYardApplication>,
) -> Result<Option<InstallPlan>, UiError> {
    let Some(folder) = app
        .dialog()
        .file()
        .set_title("选择包含 Skill 的 Bundle 文件夹")
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
        UiOutcome::InstallPlan { plan } => Ok(Some(plan)),
        _ => Err(invalid_outcome("SkillYard 没有生成安装确认信息")),
    }
}

#[tauri::command(async)]
pub fn confirm_install_plan(
    application: State<'_, SkillYardApplication>,
    plan_id: String,
    selected_candidate_ids: Vec<String>,
) -> Result<UiOutcome, UiError> {
    dispatch(
        &application,
        UiIntent::ConfirmInstallPlan {
            plan_id,
            selected_candidate_ids,
        },
    )
}

#[tauri::command(async)]
pub fn discard_install_plan(
    application: State<'_, SkillYardApplication>,
    plan_id: String,
) -> Result<(), UiError> {
    let outcome = application
        .handle(UiIntent::DiscardInstallPlan { plan_id })
        .map_err(discard_install_plan_error)?;
    match outcome {
        UiOutcome::InstallPlanDiscarded => Ok(()),
        _ => Err(invalid_outcome("SkillYard 没有放弃安装 Plan")),
    }
}

fn discard_install_plan_error(error: ApplicationError) -> UiError {
    let plan_was_consumed = matches!(
        &error,
        ApplicationError::Storage(crate::storage::StorageError::InstallPlanConsumed)
            | ApplicationError::Lifecycle(crate::lifecycle::LifecycleError::Storage(
                crate::storage::StorageError::InstallPlanConsumed
            ))
    );
    if plan_was_consumed {
        // 前端需要离开已经永久失效的确认页；其他清理失败仍保留页面供用户重试。
        UiError {
            code: "installPlanConsumed",
            message: error.to_string(),
        }
    } else {
        error.into()
    }
}

/// Project 路径与安装文件夹一样，只能由 Rust 原生选择器签发。
#[tauri::command(async)]
pub fn choose_and_register_project(
    app: AppHandle,
    application: State<'_, SkillYardApplication>,
) -> Result<Option<UiOutcome>, UiError> {
    let Some(folder) = app
        .dialog()
        .file()
        .set_title("选择要交给 SkillYard 使用的 Project")
        .blocking_pick_folder()
    else {
        return Ok(None);
    };
    let path = folder.into_path().map_err(|error| UiError {
        code: "dialogError",
        message: format!("无法读取所选 Project：{error}"),
    })?;
    let root_path = path.to_str().ok_or_else(|| UiError {
        code: "invalidPath",
        message: "所选 Project 名称包含 SkillYard 1.0 无法保存的字符".to_owned(),
    })?;
    dispatch(
        &application,
        UiIntent::RegisterProject {
            root_path: root_path.to_owned(),
        },
    )
    .map(Some)
}

/// 创建 command 只拆出已封存 Plan，不暴露 Takeover 内部文件步骤。
#[tauri::command(async)]
pub fn create_takeover_plan(
    application: State<'_, SkillYardApplication>,
    request: TakeoverPlanRequest,
) -> Result<TakeoverPlan, UiError> {
    match dispatch(&application, UiIntent::CreateTakeoverPlan { request })? {
        UiOutcome::TakeoverPlan { plan } => Ok(plan),
        _ => Err(UiError {
            code: "invalidOutcome",
            message: "SkillYard 没有生成 Takeover 确认信息".to_owned(),
        }),
    }
}

/// 确认阶段只接收不透明 Plan ID，全部路径和用户选择沿用已封存 Plan。
#[tauri::command(async)]
pub fn confirm_takeover_plan(
    application: State<'_, SkillYardApplication>,
    plan_id: String,
) -> Result<UiOutcome, UiError> {
    dispatch(&application, UiIntent::ConfirmTakeoverPlan { plan_id })
}

#[tauri::command(async)]
pub fn create_mount_plan(
    application: State<'_, SkillYardApplication>,
    member_id: String,
    app_id: SupportedAppId,
    scope: MountScope,
    project_id: Option<String>,
) -> Result<MountPlan, UiError> {
    match dispatch(
        &application,
        UiIntent::CreateMountPlan {
            member_id,
            app_id,
            scope,
            project_id,
        },
    )? {
        UiOutcome::MountPlan { plan } => Ok(plan),
        _ => Err(UiError {
            code: "invalidOutcome",
            message: "SkillYard 没有生成 Mount 确认信息".to_owned(),
        }),
    }
}

#[tauri::command(async)]
pub fn create_remove_mount_plan(
    application: State<'_, SkillYardApplication>,
    mount_id: String,
) -> Result<MountPlan, UiError> {
    match dispatch(&application, UiIntent::CreateRemoveMountPlan { mount_id })? {
        UiOutcome::MountPlan { plan } => Ok(plan),
        _ => Err(UiError {
            code: "invalidOutcome",
            message: "SkillYard 没有生成移除 Mount 的确认信息".to_owned(),
        }),
    }
}

#[tauri::command(async)]
pub fn create_repair_mount_plan(
    application: State<'_, SkillYardApplication>,
    mount_id: String,
) -> Result<MountPlan, UiError> {
    match dispatch(&application, UiIntent::CreateRepairMountPlan { mount_id })? {
        UiOutcome::MountPlan { plan } => Ok(plan),
        _ => Err(UiError {
            code: "invalidOutcome",
            message: "SkillYard 没有生成修复 Mount 的确认信息".to_owned(),
        }),
    }
}

#[tauri::command(async)]
pub fn confirm_mount_plan(
    application: State<'_, SkillYardApplication>,
    plan_id: String,
) -> Result<UiOutcome, UiError> {
    dispatch(&application, UiIntent::ConfirmMountPlan { plan_id })
}

#[tauri::command(async)]
pub fn create_batch_mount_plan(
    application: State<'_, SkillYardApplication>,
    bundle_id: String,
    requests: Vec<BatchMountRequest>,
) -> Result<BatchMountPlan, UiError> {
    match dispatch(
        &application,
        UiIntent::CreateBatchMountPlan {
            bundle_id,
            requests,
        },
    )? {
        UiOutcome::BatchMountPlan { plan } => Ok(plan),
        _ => Err(UiError {
            code: "invalidOutcome",
            message: "SkillYard 没有生成批量 Mount 确认信息".to_owned(),
        }),
    }
}

#[tauri::command(async)]
pub fn confirm_batch_mount_plan(
    application: State<'_, SkillYardApplication>,
    plan_id: String,
    selected_item_ids: Vec<String>,
) -> Result<UiOutcome, UiError> {
    dispatch(
        &application,
        UiIntent::ConfirmBatchMountPlan {
            plan_id,
            selected_item_ids,
        },
    )
}

fn dispatch(application: &SkillYardApplication, intent: UiIntent) -> Result<UiOutcome, UiError> {
    application.handle(intent).map_err(Into::into)
}

fn invalid_outcome(message: &str) -> UiError {
    UiError {
        code: "invalidOutcome",
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

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
                selected_candidate_ids: vec!["unknown".to_owned()],
            },
        )
        .expect_err("未知 Plan 应保留生命周期错误类型");

        assert_eq!(error.code, "lifecycleError");
        assert!(error.message.contains("未签发"));
    }

    #[test]
    fn discard_maps_consumed_plan_to_a_stable_ui_state_code() {
        let direct = discard_install_plan_error(ApplicationError::Storage(
            crate::storage::StorageError::InstallPlanConsumed,
        ));
        let recovered = discard_install_plan_error(ApplicationError::Lifecycle(
            crate::lifecycle::LifecycleError::Storage(
                crate::storage::StorageError::InstallPlanConsumed,
            ),
        ));

        assert_eq!(direct.code, "installPlanConsumed");
        assert_eq!(recovered.code, "installPlanConsumed");
    }

    #[test]
    fn ipc_dispatch_runs_the_takeover_plan_and_confirmation_against_real_state() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let home = sandbox.path().join("home");
        let data_root = sandbox.path().join("data");
        let skill_root = home.join(".codex/skills/alpha");
        fs::create_dir_all(&skill_root).expect("应创建待接管 Skill");
        fs::write(
            skill_root.join("SKILL.md"),
            "---\nname: alpha\ndescription: IPC 接管验收\n---\n",
        )
        .expect("应写入 Skill metadata");
        let application = SkillYardApplication::new(
            ApplicationPaths::for_home(data_root, home),
            PlatformInfo::supported_for_test(),
        );

        let UiOutcome::Inventory { entries, .. } =
            dispatch(&application, UiIntent::StartInitialScan).expect("IPC 应完成首次扫描")
        else {
            panic!("首次扫描应返回 Inventory");
        };
        let observation_id = entries
            .into_iter()
            .find(|entry| entry.skill_root == skill_root.to_string_lossy())
            .expect("应发现待接管 Skill")
            .id;
        let UiOutcome::TakeoverPlan { plan } = dispatch(
            &application,
            UiIntent::CreateTakeoverPlan {
                request: TakeoverPlanRequest {
                    observation_ids: vec![observation_id.clone()],
                    selected_observation_id: observation_id.clone(),
                    preserved_observation_ids: vec![observation_id],
                    shared_targets: Vec::new(),
                },
            },
        )
        .expect("IPC 应生成 Takeover Plan") else {
            panic!("创建接管计划应返回 TakeoverPlan");
        };

        let UiOutcome::Inventory { entries, .. } = dispatch(
            &application,
            UiIntent::ConfirmTakeoverPlan {
                plan_id: plan.id.clone(),
            },
        )
        .expect("IPC 应确认接管") else {
            panic!("确认接管应返回 Inventory");
        };

        assert!(
            entries
                .iter()
                .any(|entry| entry.bundle_id.as_deref() == Some(plan.bundle_id.as_str()))
        );
        assert_eq!(
            fs::read_link(&skill_root).expect("原使用位置应成为 Mount"),
            Path::new(&plan.expected_target)
        );
    }
}
