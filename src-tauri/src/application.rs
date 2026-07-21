use std::sync::{Mutex, TryLockError};

use thiserror::Error;

use crate::{
    domain::{PlatformInfo, UiIntent, UiOutcome},
    lifecycle::{
        LifecycleError, LifecycleFailpoint, acquire_lifecycle_lock, confirm_folder_install,
        create_folder_install_plan, ensure_central_store_layout, recover_pending_transactions,
    },
    paths::ApplicationPaths,
    scanner::scan,
    storage::{Storage, StorageError},
};

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error("首次扫描未完整完成：{0}")]
    InitialScan(String),
    #[error("当前状态不能执行这个操作：{0}")]
    InvalidState(&'static str),
    #[error("已有一项写操作正在执行，请等待完成")]
    OperationInProgress,
    #[error("写操作保护状态不可用，请重新启动 SkillYard")]
    OperationGateUnavailable,
}

/// 所有业务行为都从这个 seam 进入；Tauri command 只负责薄适配。
pub struct SkillYardApplication {
    paths: ApplicationPaths,
    platform: PlatformInfo,
    operation_gate: Mutex<()>,
    lifecycle_failpoint: LifecycleFailpoint,
}

impl SkillYardApplication {
    pub fn new(paths: ApplicationPaths, platform: PlatformInfo) -> Self {
        Self::new_with_lifecycle_failpoint(paths, platform, LifecycleFailpoint::None)
    }

    /// 仅供崩溃恢复测试在精确阶段注入中断，生产入口始终使用 `None`。
    #[doc(hidden)]
    pub fn new_with_lifecycle_failpoint(
        paths: ApplicationPaths,
        platform: PlatformInfo,
        lifecycle_failpoint: LifecycleFailpoint,
    ) -> Self {
        // SQLite 延迟到 intent 中打开，确保初始化失败能通过 UiError 呈现，而不是在窗口创建前 panic。
        Self {
            paths,
            platform,
            operation_gate: Mutex::new(()),
            lifecycle_failpoint,
        }
    }

    pub fn handle(&self, intent: UiIntent) -> Result<UiOutcome, ApplicationError> {
        if !self.platform.is_supported() {
            return Ok(UiOutcome::UnsupportedPlatform {
                actual_os: self.platform.os.clone(),
                actual_architecture: self.platform.architecture.clone(),
                actual_major_version: self.platform.major_version,
                required_architecture: "aarch64".to_owned(),
                minimum_major_version: 14,
            });
        }

        match intent {
            // 启动读取前可能需要修复上次中断的写事务，因此也必须取得单写门。
            UiIntent::GetStartupState => self.with_write_operation(|| self.get_startup_state()),
            UiIntent::StartInitialScan => self.with_write_operation(|| self.start_initial_scan()),
            UiIntent::RefreshLocalInventory => {
                self.with_write_operation(|| self.refresh_local_inventory())
            }
            UiIntent::CreateFolderInstallPlan { input_path } => {
                self.with_write_operation(|| self.create_folder_install_plan(input_path))
            }
            UiIntent::ConfirmInstallPlan {
                plan_id,
                selected_candidate_ids,
            } => self.with_write_operation(|| {
                self.confirm_install_plan(plan_id, selected_candidate_ids)
            }),
        }
    }

    /// 扫描结果会写入同一份 SQLite；拒绝并发写可避免旧快照覆盖新状态。
    fn with_write_operation(
        &self,
        operation: impl FnOnce() -> Result<UiOutcome, ApplicationError>,
    ) -> Result<UiOutcome, ApplicationError> {
        let _guard = match self.operation_gate.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Err(ApplicationError::OperationInProgress),
            Err(TryLockError::Poisoned(_)) => {
                return Err(ApplicationError::OperationGateUnavailable);
            }
        };
        operation()
    }

    fn get_startup_state(&self) -> Result<UiOutcome, ApplicationError> {
        let storage = self.open_recovered_storage()?;
        if let Some(outcome) = storage.read_initial_scan()? {
            return Ok(outcome);
        }

        Ok(UiOutcome::onboarding_required())
    }

    fn start_initial_scan(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        // 恢复完成后继续持有跨进程锁，避免另一实例在扫描与保存之间改写同一份清单。
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        if let Some(outcome) = storage.read_initial_scan()? {
            lifecycle_lock.recheck(&self.paths)?;
            return Ok(outcome);
        }

        let result = scan(&self.paths);
        if let Some(issue) = result.issues.first() {
            return Err(ApplicationError::InitialScan(issue.message.clone()));
        }
        let scan_completed_at = unix_timestamp_millis();
        storage.save_initial_scan(scan_completed_at, &result.entries, &result.supported_apps)?;
        lifecycle_lock.recheck(&self.paths)?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描结果没有保存成功"))
    }

    fn refresh_local_inventory(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        // 刷新必须把读取旧状态、扫描和保存视为一次完整写操作。
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let Some(UiOutcome::Inventory {
            scan_completed_at, ..
        }) = storage.read_initial_scan()?
        else {
            return Err(ApplicationError::InvalidState("完成首次扫描后才能刷新本机"));
        };

        let result = scan(&self.paths);
        let completed_at = unix_timestamp_millis();
        let saved = storage.save_local_refresh(
            completed_at,
            &result.entries,
            &result.supported_apps,
            &result.successful_roots,
            &result.issues,
        )?;
        lifecycle_lock.recheck(&self.paths)?;

        Ok(UiOutcome::Inventory {
            scan_completed_at,
            entries: saved.entries,
            supported_apps: saved.supported_apps,
            last_local_refresh: Some(saved.summary),
            scan_issues: result.issues,
            recovery_issues: saved.recovery_issues,
        })
    }

    fn create_folder_install_plan(
        &self,
        input_path: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        // Plan 会校验源目录并写入 SQLite，整个过程不能与另一实例的安装交错。
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        ensure_onboarding_completed(&storage)?;
        let plan = create_folder_install_plan(
            &self.paths,
            &mut storage,
            std::path::Path::new(&input_path),
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::FolderInstallPlan { plan })
    }

    fn confirm_install_plan(
        &self,
        plan_id: String,
        selected_candidate_ids: Vec<String>,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        confirm_folder_install(
            &self.paths,
            &mut storage,
            &plan_id,
            &selected_candidate_ids,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn open_recovered_storage(&self) -> Result<Storage, ApplicationError> {
        let mut storage = Storage::open(self.paths.data_root(), &self.paths.database())?;
        ensure_central_store_layout(&self.paths)?;
        recover_pending_transactions(&self.paths, &mut storage, unix_timestamp_millis())?;
        Ok(storage)
    }
}

fn ensure_onboarding_completed(storage: &Storage) -> Result<(), ApplicationError> {
    if storage.read_initial_scan()?.is_some() {
        Ok(())
    } else {
        Err(ApplicationError::InvalidState(
            "完成首次扫描后才能安装 Skill",
        ))
    }
}

fn unix_timestamp_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时间必须晚于 Unix epoch")
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn a_second_write_intent_is_rejected_while_the_operation_gate_is_held() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let application = SkillYardApplication::new(
            ApplicationPaths::for_home(sandbox.path().join("data"), sandbox.path().join("home")),
            PlatformInfo::supported_for_test(),
        );

        let _active_operation = application
            .operation_gate
            .lock()
            .expect("测试应取得写操作门");
        let error = application
            .handle(UiIntent::StartInitialScan)
            .expect_err("并发写操作必须被拒绝");

        assert!(matches!(error, ApplicationError::OperationInProgress));
    }

    #[test]
    fn an_external_lifecycle_lock_rejects_a_scan_before_it_writes_inventory() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let paths =
            ApplicationPaths::for_home(sandbox.path().join("data"), sandbox.path().join("home"));
        ensure_central_store_layout(&paths).expect("应准备 Central Store");
        let _external_lock = acquire_lifecycle_lock(&paths).expect("测试应取得跨进程锁");
        let application =
            SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());

        let error = application
            .handle(UiIntent::StartInitialScan)
            .expect_err("另一实例持锁时扫描必须被拒绝");

        assert!(matches!(
            error,
            ApplicationError::Lifecycle(LifecycleError::LifecycleBusy)
        ));
        let storage =
            Storage::open(paths.data_root(), &paths.database()).expect("应能只读核对测试数据库");
        assert!(
            storage
                .read_initial_scan()
                .expect("应能读取首次扫描状态")
                .is_none(),
            "跨进程锁冲突不能留下部分扫描结果"
        );
    }

    #[test]
    fn lifecycle_lock_detects_the_visible_central_store_being_replaced() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let paths =
            ApplicationPaths::for_home(sandbox.path().join("data"), sandbox.path().join("home"));
        ensure_central_store_layout(&paths).expect("应准备 Central Store");
        let lock = acquire_lifecycle_lock(&paths).expect("应取得生命周期锁");
        let moved = sandbox.path().join("moved-data");
        fs::rename(paths.data_root(), &moved).expect("应模拟移动 Central Store");
        fs::create_dir(paths.data_root()).expect("应模拟创建同名替代目录");

        let error = lock
            .recheck(&paths)
            .expect_err("锁必须绑定取得时的 Central Store inode");

        assert!(matches!(error, LifecycleError::UnsafeCentralStore(_)));
    }

    #[test]
    fn lifecycle_lock_detects_its_lock_file_being_replaced() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let paths =
            ApplicationPaths::for_home(sandbox.path().join("data"), sandbox.path().join("home"));
        ensure_central_store_layout(&paths).expect("应准备 Central Store");
        let lock = acquire_lifecycle_lock(&paths).expect("应取得生命周期锁");
        let lock_path = paths.data_root().join(".lifecycle.lock");
        fs::remove_file(&lock_path).expect("应模拟删除已持锁目录项");
        fs::write(&lock_path, []).expect("应模拟创建新锁文件");

        let error = lock
            .recheck(&paths)
            .expect_err("锁必须绑定取得时的文件 inode");

        assert!(matches!(error, LifecycleError::UnsafeCentralStore(_)));
    }
}
