use std::sync::{Mutex, TryLockError};

use thiserror::Error;

use crate::{
    domain::{PlatformInfo, UiIntent, UiOutcome},
    paths::ApplicationPaths,
    scanner::scan,
    storage::{Storage, StorageError},
};

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error(transparent)]
    Storage(#[from] StorageError),
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
}

impl SkillYardApplication {
    pub fn new(paths: ApplicationPaths, platform: PlatformInfo) -> Self {
        // SQLite 延迟到 intent 中打开，确保初始化失败能通过 UiError 呈现，而不是在窗口创建前 panic。
        Self {
            paths,
            platform,
            operation_gate: Mutex::new(()),
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
            UiIntent::GetStartupState => self.get_startup_state(),
            UiIntent::StartInitialScan => self.with_write_operation(|| self.start_initial_scan()),
            UiIntent::RefreshLocalInventory => {
                self.with_write_operation(|| self.refresh_local_inventory())
            }
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
        let storage = Storage::open(self.paths.data_root(), &self.paths.database())?;
        if let Some(outcome) = storage.read_initial_scan()? {
            return Ok(outcome);
        }

        Ok(UiOutcome::onboarding_required())
    }

    fn start_initial_scan(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = Storage::open(self.paths.data_root(), &self.paths.database())?;
        if let Some(outcome) = storage.read_initial_scan()? {
            return Ok(outcome);
        }

        let result = scan(&self.paths);
        if let Some(issue) = result.issues.first() {
            return Err(ApplicationError::InitialScan(issue.message.clone()));
        }
        let scan_completed_at = unix_timestamp_millis();
        storage.save_initial_scan(scan_completed_at, &result.entries, &result.supported_apps)?;

        Ok(UiOutcome::Inventory {
            scan_completed_at,
            entries: result.entries,
            supported_apps: result.supported_apps,
            last_local_refresh: None,
            scan_issues: Vec::new(),
        })
    }

    fn refresh_local_inventory(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = Storage::open(self.paths.data_root(), &self.paths.database())?;
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

        Ok(UiOutcome::Inventory {
            scan_completed_at,
            entries: saved.entries,
            supported_apps: saved.supported_apps,
            last_local_refresh: Some(saved.summary),
            scan_issues: result.issues,
        })
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
}
