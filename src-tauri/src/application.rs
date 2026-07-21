use thiserror::Error;

use crate::{
    domain::{PlatformInfo, UiIntent, UiOutcome},
    paths::ApplicationPaths,
    scanner::{ScanError, scan},
    storage::{Storage, StorageError},
};

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Scan(#[from] ScanError),
}

/// 所有业务行为都从这个 seam 进入；Tauri command 只负责薄适配。
pub struct SkillYardApplication {
    paths: ApplicationPaths,
    platform: PlatformInfo,
}

impl SkillYardApplication {
    pub fn new(paths: ApplicationPaths, platform: PlatformInfo) -> Result<Self, ApplicationError> {
        Storage::open(paths.data_root(), &paths.database())?;
        Ok(Self { paths, platform })
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
            UiIntent::StartInitialScan => self.start_initial_scan(),
        }
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

        let result = scan(&self.paths)?;
        let scan_completed_at = unix_timestamp_millis();
        storage.save_initial_scan(scan_completed_at, &result.entries, &result.supported_apps)?;

        Ok(UiOutcome::Inventory {
            scan_completed_at,
            entries: result.entries,
            supported_apps: result.supported_apps,
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
