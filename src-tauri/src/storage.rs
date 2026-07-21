use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use thiserror::Error;

use crate::domain::{
    InventoryItem, InventoryLocationKind, InventoryObservation, LocalRefreshSummary,
    ManagementKind, RecoveryIssue, ScanIssue, ScanIssueCode, ScanRootKey, SkillMetadataStatus,
    SupportedAppId, SupportedAppSummary, UiOutcome,
};

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_initial.sql")),
    (2, include_str!("../migrations/0002_local_inventory.sql")),
    (3, include_str!("../migrations/0003_folder_install.sql")),
    (
        4,
        include_str!("../migrations/0004_bundle_install_candidates.sql"),
    ),
];

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("无法创建 SkillYard 数据目录：{0}")]
    CreateDataRoot(#[source] std::io::Error),
    #[error("无法检查 SkillYard 数据目录：{0}")]
    InspectDataRoot(#[source] std::io::Error),
    #[error("SkillYard 数据目录不能是符号链接或其他文件类型：{0}")]
    UnsafeDataRoot(PathBuf),
    #[error("无法检查 SkillYard SQLite 路径：{0}")]
    InspectDatabase(#[source] std::io::Error),
    #[error("SkillYard SQLite 不能是符号链接或其他文件类型：{0}")]
    UnsafeDatabase(PathBuf),
    #[error("无法打开 SkillYard SQLite：{0}")]
    OpenDatabase(#[source] rusqlite::Error),
    #[error("无法执行 SkillYard SQLite migration：{0}")]
    Migration(#[source] rusqlite::Error),
    #[error("无法读取本机清单状态：{0}")]
    ReadInventory(#[source] rusqlite::Error),
    #[error("无法保存首次扫描结果：{0}")]
    SaveInitialScan(#[source] rusqlite::Error),
    #[error("无法保存本机刷新结果：{0}")]
    SaveLocalRefresh(#[source] rusqlite::Error),
    #[error("SQLite 中包含未知 Supported App：{0}")]
    UnknownSupportedApp(String),
    #[error("SQLite 中包含未知 Inventory location：{0}")]
    UnknownInventoryLocation(String),
    #[error("SQLite 中包含未知 Skill metadata 状态：{0}")]
    UnknownMetadataStatus(String),
    #[error("SQLite 中包含未知扫描根：{0}")]
    UnknownScanRoot(String),
    #[error("SQLite 中包含未知管理状态：{0}")]
    UnknownManagementKind(String),
    #[error("SQLite 中包含未知扫描问题类型：{0}")]
    UnknownScanIssueCode(String),
    #[error("SQLite 中包含非法刷新统计值：{0}")]
    InvalidRefreshCount(i64),
    #[error("安装 Plan 未签发或已经不存在")]
    InstallPlanNotFound,
    #[error("安装 Plan 已经使用，不能重复确认")]
    InstallPlanConsumed,
    #[error("安装 Plan 已过期，请重新选择文件夹")]
    InstallPlanExpired,
    #[error("安装 Plan 没有可保存的候选成员")]
    EmptyInstallPlanCandidates,
    #[error("确认的成员选择不属于当前安装 Plan")]
    InvalidInstallSelection,
    #[error("已有一项生命周期写事务正在执行")]
    ActiveLifecycleTransaction,
    #[error("无法保存安装 Plan：{0}")]
    SaveInstallPlan(#[source] rusqlite::Error),
    #[error("无法读取安装 Plan：{0}")]
    ReadInstallPlan(#[source] rusqlite::Error),
    #[error("安装 Plan 中的风险提示损坏：{0}")]
    InvalidPlanWarnings(#[source] serde_json::Error),
    #[error("安装 Plan 中的验证结果损坏：{0}")]
    InvalidPlanValidationErrors(#[source] serde_json::Error),
    #[error("安装 Plan 中包含非法布尔值：{0}")]
    InvalidPlanBoolean(i64),
    #[error("无法保存生命周期事务：{0}")]
    SaveLifecycleTransaction(#[source] rusqlite::Error),
    #[error("无法读取生命周期事务：{0}")]
    ReadLifecycleTransaction(#[source] rusqlite::Error),
    #[error("无法读取人工恢复状态：{0}")]
    ReadRecoveryIssues(#[source] rusqlite::Error),
    #[error("无法保存受管 Bundle：{0}")]
    SaveManagedBundle(#[source] rusqlite::Error),
    #[error("受管 Bundle 的持久化状态与事务计划不一致")]
    ManagedStateConflict,
    #[error("生命周期事务不存在或当前状态不允许该操作：{0}")]
    LifecycleStateConflict(String),
    #[error("SQLite 中包含未知生命周期阶段：{0}")]
    InvalidLifecyclePhase(String),
    #[error("受管内容路径不符合 Central Store 固定布局：{0}")]
    UnsafeManagedPath(String),
}

pub struct Storage {
    connection: Connection,
    data_root: PathBuf,
}

pub struct SavedLocalRefresh {
    pub entries: Vec<InventoryItem>,
    pub supported_apps: Vec<SupportedAppSummary>,
    pub summary: LocalRefreshSummary,
    pub recovery_issues: Vec<RecoveryIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredInstallPlan {
    pub id: String,
    pub input_path: String,
    pub input_device: u64,
    pub input_inode: u64,
    pub input_fingerprint: String,
    pub bundle_id: String,
    pub bundle_display_name: String,
    pub member_id: String,
    pub skill_name: String,
    pub _legacy_skill_description: String,
    pub expires_at: i64,
    pub status: String,
    pub candidates: Vec<StoredInstallCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredInstallCandidate {
    pub candidate_id: String,
    pub source_relative_path: String,
    pub skill_name: Option<String>,
    pub skill_description: Option<String>,
    pub content_fingerprint: Option<String>,
    pub selectable: bool,
    pub validation_errors: Vec<String>,
    pub warnings: Vec<String>,
    pub default_selected: bool,
    pub selected: bool,
}

pub struct NewInstallPlan<'a> {
    pub id: &'a str,
    pub input_path: &'a str,
    pub input_device: u64,
    pub input_inode: u64,
    pub input_fingerprint: &'a str,
    pub bundle_id: &'a str,
    pub bundle_display_name: &'a str,
    pub member_id: &'a str,
    pub skill_name: &'a str,
    pub skill_description: &'a str,
    pub warnings: &'a [String],
    pub candidates: &'a [NewInstallCandidate<'a>],
    pub created_at: i64,
    pub expires_at: i64,
}

pub struct NewInstallCandidate<'a> {
    pub candidate_id: &'a str,
    pub source_relative_path: &'a str,
    pub skill_name: Option<&'a str>,
    pub skill_description: Option<&'a str>,
    pub content_fingerprint: Option<&'a str>,
    pub selectable: bool,
    pub validation_errors: &'a [String],
    pub warnings: &'a [String],
    pub default_selected: bool,
}

#[derive(Debug, Clone)]
pub struct StoredLifecycleTransaction {
    pub id: String,
    pub plan_id: String,
    pub bundle_id: String,
    pub member_id: String,
    pub journal_path: String,
    pub phase: String,
    pub status: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LifecyclePhase {
    JournalPending,
    JournalReady,
    CandidateReady,
    Activated,
    StateCommitted,
}

impl LifecyclePhase {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "journal_pending" => Some(Self::JournalPending),
            "journal_ready" => Some(Self::JournalReady),
            "candidate_ready" => Some(Self::CandidateReady),
            "activated" => Some(Self::Activated),
            "state_committed" => Some(Self::StateCommitted),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::JournalPending => "journal_pending",
            Self::JournalReady => "journal_ready",
            Self::CandidateReady => "candidate_ready",
            Self::Activated => "activated",
            Self::StateCommitted => "state_committed",
        }
    }

    fn previous(self) -> Option<Self> {
        match self {
            Self::JournalPending => Some(Self::JournalPending),
            Self::JournalReady => Some(Self::JournalPending),
            Self::CandidateReady => Some(Self::JournalReady),
            Self::Activated => Some(Self::CandidateReady),
            Self::StateCommitted => None,
        }
    }
}

impl Storage {
    pub fn open(data_root: &Path, database: &Path) -> Result<Self, StorageError> {
        ensure_safe_data_root(data_root)?;
        let database_open_path = safe_database_open_path(data_root, database)?;
        // SQLite 自己在最终打开点执行 no-follow，补上检查与 open 之间的竞态防线。
        let connection = Connection::open_with_flags(
            database_open_path,
            OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(StorageError::OpenDatabase)?;
        let mut storage = Self {
            connection,
            data_root: data_root.to_owned(),
        };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&mut self) -> Result<(), StorageError> {
        // migration 目录自身必须先存在，后续版本才可以被真正跳过。
        self.connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA synchronous = FULL;
                 PRAGMA fullfsync = ON;
                 CREATE TABLE IF NOT EXISTS schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                 );",
            )
            .map_err(StorageError::Migration)?;

        for (version, migration) in MIGRATIONS {
            // IMMEDIATE 锁让多个同时启动的进程在检查版本前排队，避免重复执行 ALTER TABLE。
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(StorageError::Migration)?;
            let applied = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
                    [version],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(StorageError::Migration)?;
            if !applied {
                transaction
                    .execute_batch(migration)
                    .map_err(StorageError::Migration)?;
                transaction
                    .execute(
                        "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, unixepoch())",
                        [version],
                    )
                    .map_err(StorageError::Migration)?;
            }
            transaction.commit().map_err(StorageError::Migration)?;
        }
        Ok(())
    }

    pub fn read_initial_scan(&self) -> Result<Option<UiOutcome>, StorageError> {
        let (initial_scan_completed_at, refresh_at, added, changed, removed): (
            Option<i64>,
            Option<i64>,
            i64,
            i64,
            i64,
        ) = self
            .connection
            .query_row(
                "SELECT initial_scan_completed_at, last_local_refresh_at, last_local_refresh_added, last_local_refresh_changed, last_local_refresh_removed FROM app_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .map_err(StorageError::ReadInventory)?;
        let Some(scan_completed_at) = initial_scan_completed_at else {
            return Ok(None);
        };

        let supported_apps = self.read_supported_apps()?;
        let entries = self.with_managed_entries(self.read_inventory_entries()?)?;
        let scan_issues = self.read_scan_issues()?;
        let recovery_issues = self.read_recovery_issues()?;
        let last_local_refresh = refresh_at
            .map(|completed_at| {
                Ok(LocalRefreshSummary {
                    completed_at,
                    added: refresh_count(added)?,
                    changed: refresh_count(changed)?,
                    removed: refresh_count(removed)?,
                })
            })
            .transpose()?;

        Ok(Some(UiOutcome::Inventory {
            scan_completed_at,
            entries,
            supported_apps,
            last_local_refresh,
            scan_issues,
            recovery_issues,
        }))
    }

    pub fn save_initial_scan(
        &mut self,
        scan_completed_at: i64,
        entries: &[InventoryObservation],
        supported_apps: &[SupportedAppSummary],
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveInitialScan)?;
        replace_inventory_rows(&transaction, entries, supported_apps, &[])
            .map_err(StorageError::SaveInitialScan)?;
        transaction
            .execute(
                "UPDATE app_state
                 SET initial_scan_completed_at = ?1,
                     last_local_refresh_at = NULL,
                     last_local_refresh_added = 0,
                     last_local_refresh_changed = 0,
                     last_local_refresh_removed = 0
                 WHERE singleton = 1",
                [scan_completed_at],
            )
            .map_err(StorageError::SaveInitialScan)?;
        transaction.commit().map_err(StorageError::SaveInitialScan)
    }

    pub fn save_local_refresh(
        &mut self,
        completed_at: i64,
        scanned_entries: &[InventoryObservation],
        scanned_apps: &[SupportedAppSummary],
        successful_roots: &[ScanRootKey],
        scan_issues: &[ScanIssue],
    ) -> Result<SavedLocalRefresh, StorageError> {
        // 读取旧快照和写入新快照必须处于同一个写事务，不能让并发命令覆盖较新结果。
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveLocalRefresh)?;
        let previous_entries = read_inventory_entries_from(&transaction)?;
        let previous_apps = read_supported_apps_from(&transaction)?;
        let entries = reconcile_entries(
            &previous_entries,
            scanned_entries,
            successful_roots,
            scan_issues,
        );
        let supported_apps = reconcile_supported_apps(&previous_apps, scanned_apps);
        let summary = summarize_changes(completed_at, &previous_entries, &entries);
        replace_inventory_rows(&transaction, &entries, &supported_apps, scan_issues)
            .map_err(StorageError::SaveLocalRefresh)?;
        transaction
            .execute(
                "UPDATE app_state
                 SET last_local_refresh_at = ?1,
                     last_local_refresh_added = ?2,
                     last_local_refresh_changed = ?3,
                     last_local_refresh_removed = ?4
                 WHERE singleton = 1",
                params![
                    summary.completed_at,
                    summary.added as i64,
                    summary.changed as i64,
                    summary.removed as i64
                ],
            )
            .map_err(StorageError::SaveLocalRefresh)?;
        transaction
            .commit()
            .map_err(StorageError::SaveLocalRefresh)?;

        Ok(SavedLocalRefresh {
            entries: self.with_managed_entries(entries)?,
            supported_apps,
            summary,
            recovery_issues: self.read_recovery_issues()?,
        })
    }

    pub fn save_install_plan(&mut self, plan: NewInstallPlan<'_>) -> Result<(), StorageError> {
        if plan.candidates.is_empty() {
            return Err(StorageError::EmptyInstallPlanCandidates);
        }
        let warnings =
            serde_json::to_string(plan.warnings).map_err(StorageError::InvalidPlanWarnings)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveInstallPlan)?;
        transaction
            .execute(
                "INSERT INTO install_plans (id, kind, input_path, input_device, input_inode, input_fingerprint, bundle_id, bundle_display_name, member_id, skill_name, skill_description, warnings_json, created_at, expires_at, status)
                 VALUES (?1, 'folder_snapshot', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'pending')",
                params![
                    plan.id,
                    plan.input_path,
                    plan.input_device as i64,
                    plan.input_inode as i64,
                    plan.input_fingerprint,
                    plan.bundle_id,
                    plan.bundle_display_name,
                    plan.member_id,
                    plan.skill_name,
                    plan.skill_description,
                    warnings,
                    plan.created_at,
                    plan.expires_at
                ],
            )
            .map_err(StorageError::SaveInstallPlan)?;
        for (sort_order, candidate) in plan.candidates.iter().enumerate() {
            let validation_errors = serde_json::to_string(candidate.validation_errors)
                .map_err(StorageError::InvalidPlanValidationErrors)?;
            let candidate_warnings = serde_json::to_string(candidate.warnings)
                .map_err(StorageError::InvalidPlanWarnings)?;
            transaction
                .execute(
                    "INSERT INTO install_plan_candidates (
                        plan_id, candidate_id, source_relative_path, skill_name,
                        skill_description, content_fingerprint, selectable,
                        validation_errors_json, warnings_json, default_selected,
                        selected, sort_order
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11)",
                    params![
                        plan.id,
                        candidate.candidate_id,
                        candidate.source_relative_path,
                        candidate.skill_name,
                        candidate.skill_description,
                        candidate.content_fingerprint,
                        i64::from(candidate.selectable),
                        validation_errors,
                        candidate_warnings,
                        i64::from(candidate.default_selected),
                        sort_order as i64,
                    ],
                )
                .map_err(StorageError::SaveInstallPlan)?;
        }
        transaction.commit().map_err(StorageError::SaveInstallPlan)
    }

    pub fn read_install_plan(&self, plan_id: &str) -> Result<StoredInstallPlan, StorageError> {
        read_install_plan_from(&self.connection, plan_id)?.ok_or(StorageError::InstallPlanNotFound)
    }

    #[cfg(test)]
    pub fn begin_install_transaction(
        &mut self,
        plan_id: &str,
        transaction_id: &str,
        journal_path: &str,
        now: i64,
    ) -> Result<StoredInstallPlan, StorageError> {
        let selected_candidate_ids = self
            .read_install_plan(plan_id)?
            .candidates
            .into_iter()
            .filter(|candidate| candidate.selectable && candidate.default_selected)
            .map(|candidate| candidate.candidate_id)
            .collect::<Vec<_>>();
        self.begin_install_transaction_with_selection(
            plan_id,
            &selected_candidate_ids,
            transaction_id,
            journal_path,
            now,
        )
    }

    pub fn begin_install_transaction_with_selection(
        &mut self,
        plan_id: &str,
        selected_candidate_ids: &[String],
        transaction_id: &str,
        journal_path: &str,
        now: i64,
    ) -> Result<StoredInstallPlan, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveLifecycleTransaction)?;
        let mut plan = read_install_plan_from(&transaction, plan_id)?
            .ok_or(StorageError::InstallPlanNotFound)?;
        if plan.status != "pending" {
            return Err(StorageError::InstallPlanConsumed);
        }
        if plan.expires_at <= now {
            return Err(StorageError::InstallPlanExpired);
        }
        let selected = selected_candidate_ids.iter().collect::<BTreeSet<_>>();
        if selected.is_empty() || selected.len() != selected_candidate_ids.len() {
            return Err(StorageError::InvalidInstallSelection);
        }
        if selected.iter().any(|candidate_id| {
            !plan.candidates.iter().any(|candidate| {
                candidate.selectable && candidate.candidate_id.as_str() == candidate_id.as_str()
            })
        }) {
            return Err(StorageError::InvalidInstallSelection);
        }
        transaction
            .execute(
                "UPDATE install_plan_candidates SET selected = 0 WHERE plan_id = ?1",
                [&plan.id],
            )
            .map_err(StorageError::SaveLifecycleTransaction)?;
        for candidate_id in &selected {
            let changed = transaction
                .execute(
                    "UPDATE install_plan_candidates
                     SET selected = 1
                     WHERE plan_id = ?1 AND candidate_id = ?2 AND selectable = 1",
                    params![plan.id, candidate_id.as_str()],
                )
                .map_err(StorageError::SaveLifecycleTransaction)?;
            ensure_one_lifecycle_row(changed, transaction_id)?;
        }
        let anchor_member_id = plan
            .candidates
            .iter()
            .find(|candidate| selected.contains(&candidate.candidate_id))
            .expect("前面已拒绝空选择")
            .candidate_id
            .as_str();
        let inserted = transaction
            .execute(
                "INSERT INTO lifecycle_transactions (id, kind, plan_id, bundle_id, member_id, journal_path, phase, status, created_at, updated_at)
                 VALUES (?1, 'install_folder', ?2, ?3, ?4, ?5, 'journal_pending', 'in_progress', ?6, ?6)",
                params![
                    transaction_id,
                    plan.id,
                    plan.bundle_id,
                    anchor_member_id,
                    journal_path,
                    now
                ],
            )
            .map_err(map_lifecycle_insert_error)?;
        ensure_one_lifecycle_row(inserted, transaction_id)?;
        let consumed = transaction
            .execute(
                "UPDATE install_plans SET status = 'consumed' WHERE id = ?1 AND status = 'pending'",
                [&plan.id],
            )
            .map_err(StorageError::SaveLifecycleTransaction)?;
        ensure_one_lifecycle_row(consumed, transaction_id)?;
        // 返回这次 SQLite 事务实际确认的内存快照，避免提交后第三次读取引入新的竞态窗口。
        plan.status = "consumed".to_owned();
        for candidate in &mut plan.candidates {
            candidate.selected = selected.contains(&candidate.candidate_id);
        }
        transaction
            .commit()
            .map_err(StorageError::SaveLifecycleTransaction)?;
        Ok(plan)
    }

    pub fn update_lifecycle_phase(
        &mut self,
        transaction_id: &str,
        phase: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let next = LifecyclePhase::from_str(phase)
            .ok_or_else(|| StorageError::InvalidLifecyclePhase(phase.to_owned()))?;
        let Some(previous) = next.previous() else {
            return Err(StorageError::LifecycleStateConflict(
                transaction_id.to_owned(),
            ));
        };
        let changed = self
            .connection
            .execute(
                "UPDATE lifecycle_transactions
                 SET phase = ?2, updated_at = ?4
                 WHERE id = ?1
                   AND status = 'in_progress'
                   AND phase IN (?2, ?3)",
                params![transaction_id, next.as_str(), previous.as_str(), now],
            )
            .map_err(StorageError::SaveLifecycleTransaction)?;
        ensure_one_lifecycle_row(changed, transaction_id)
    }

    pub fn abort_lifecycle_transaction(
        &mut self,
        transaction_id: &str,
        error_message: Option<&str>,
        now: i64,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection
            .execute(
                "UPDATE lifecycle_transactions
                 SET status = 'aborted', error_message = ?2, updated_at = ?3
                 WHERE id = ?1
                   AND status = 'in_progress'
                   AND phase IN ('journal_pending', 'journal_ready', 'candidate_ready')",
                params![transaction_id, error_message, now],
            )
            .map_err(StorageError::SaveLifecycleTransaction)?;
        ensure_one_lifecycle_row(changed, transaction_id)
    }

    pub fn block_lifecycle_transaction(
        &mut self,
        transaction_id: &str,
        error_message: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection
            .execute(
                "UPDATE lifecycle_transactions SET status = 'blocked', error_message = ?2, updated_at = ?3 WHERE id = ?1 AND status IN ('in_progress', 'completed', 'aborted')",
                params![transaction_id, error_message, now],
            )
            .map_err(StorageError::SaveLifecycleTransaction)?;
        ensure_one_lifecycle_row(changed, transaction_id)
    }

    pub fn forget_terminal_transaction(
        &mut self,
        transaction_id: &str,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveLifecycleTransaction)?;
        let stored = transaction
            .query_row(
                "SELECT plan_id, status FROM lifecycle_transactions WHERE id = ?1",
                [transaction_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StorageError::SaveLifecycleTransaction)?;
        if let Some((plan_id, status)) = stored {
            if !matches!(status.as_str(), "completed" | "aborted") {
                return Err(StorageError::LifecycleStateConflict(
                    transaction_id.to_owned(),
                ));
            }
            let deleted_transaction = transaction
                .execute(
                    "DELETE FROM lifecycle_transactions WHERE id = ?1 AND status IN ('completed', 'aborted')",
                    [transaction_id],
                )
                .map_err(StorageError::SaveLifecycleTransaction)?;
            ensure_one_lifecycle_row(deleted_transaction, transaction_id)?;
            let deleted_plan = transaction
                .execute("DELETE FROM install_plans WHERE id = ?1", [plan_id])
                .map_err(StorageError::SaveLifecycleTransaction)?;
            ensure_one_lifecycle_row(deleted_plan, transaction_id)?;
        }
        transaction
            .commit()
            .map_err(StorageError::SaveLifecycleTransaction)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finalize_install(
        &mut self,
        transaction_id: &str,
        plan: &StoredInstallPlan,
        managed_directory: &str,
        current_target: &str,
        stable_relative_path: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let selected = selected_install_candidates(plan)?;
        let anchor = selected
            .first()
            .expect("selected_install_candidates 已拒绝空集合");
        validate_managed_install_paths(
            transaction_id,
            plan,
            &selected,
            managed_directory,
            current_target,
            stable_relative_path,
        )?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveManagedBundle)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO bundles (id, display_name, managed_directory, current_target, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    plan.bundle_id,
                    plan.bundle_display_name,
                    managed_directory,
                    current_target,
                    now
                ],
            )
            .map_err(StorageError::SaveManagedBundle)?;
        for candidate in &selected {
            let skill_name = candidate
                .skill_name
                .as_deref()
                .ok_or(StorageError::InvalidInstallSelection)?;
            let description = candidate
                .skill_description
                .as_deref()
                .ok_or(StorageError::InvalidInstallSelection)?;
            let fingerprint = candidate
                .content_fingerprint
                .as_deref()
                .ok_or(StorageError::InvalidInstallSelection)?;
            let stable_path = format!("members/{skill_name}");
            transaction
                .execute(
                    "INSERT OR IGNORE INTO skill_members (id, bundle_id, skill_name, description, stable_relative_path, content_fingerprint, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        candidate.candidate_id,
                        plan.bundle_id,
                        skill_name,
                        description,
                        stable_path,
                        fingerprint,
                        now
                    ],
                )
                .map_err(StorageError::SaveManagedBundle)?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO member_selections (bundle_id, member_id, selected_at) VALUES (?1, ?2, ?3)",
                    params![plan.bundle_id, candidate.candidate_id, now],
                )
                .map_err(StorageError::SaveManagedBundle)?;
        }
        ensure_managed_state_matches(
            &transaction,
            plan,
            &selected,
            managed_directory,
            current_target,
        )?;
        let changed = transaction
            .execute(
                "UPDATE lifecycle_transactions
                 SET phase = 'state_committed', status = 'completed', updated_at = ?5
                 WHERE id = ?1
                   AND plan_id = ?2
                   AND bundle_id = ?3
                   AND member_id = ?4
                   AND (
                       (status = 'in_progress' AND phase IN ('candidate_ready', 'activated'))
                       OR (status = 'completed' AND phase = 'state_committed')
                   )",
                params![
                    transaction_id,
                    plan.id,
                    plan.bundle_id,
                    anchor.candidate_id,
                    now
                ],
            )
            .map_err(StorageError::SaveManagedBundle)?;
        ensure_one_lifecycle_row(changed, transaction_id)?;
        transaction
            .commit()
            .map_err(StorageError::SaveManagedBundle)
    }

    pub fn recoverable_lifecycle_transactions(
        &self,
    ) -> Result<Vec<StoredLifecycleTransaction>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, plan_id, bundle_id, member_id, journal_path, phase, status FROM lifecycle_transactions WHERE status IN ('in_progress', 'completed', 'aborted', 'blocked') ORDER BY created_at",
            )
            .map_err(StorageError::ReadLifecycleTransaction)?;
        let rows = statement
            .query_map([], |row| {
                Ok(StoredLifecycleTransaction {
                    id: row.get(0)?,
                    plan_id: row.get(1)?,
                    bundle_id: row.get(2)?,
                    member_id: row.get(3)?,
                    journal_path: row.get(4)?,
                    phase: row.get(5)?,
                    status: row.get(6)?,
                })
            })
            .map_err(StorageError::ReadLifecycleTransaction)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadLifecycleTransaction)
    }

    pub fn managed_bundle_notice_rows(&self) -> Result<Vec<(String, String)>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, display_name, managed_directory, current_target FROM bundles ORDER BY display_name, id",
            )
            .map_err(StorageError::ReadInventory)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(StorageError::ReadInventory)?;
        let mut safe_rows = Vec::new();
        for row in rows {
            let (bundle_id, display_name, managed_directory, current_target) =
                row.map_err(StorageError::ReadInventory)?;
            if !is_single_path_component(&bundle_id)
                || managed_directory != format!("bundles/{bundle_id}")
                || !is_safe_current_target(&current_target)
            {
                return Err(StorageError::UnsafeManagedPath(managed_directory));
            }
            safe_rows.push((display_name, managed_directory));
        }
        Ok(safe_rows)
    }

    fn with_managed_entries(
        &self,
        entries: Vec<InventoryObservation>,
    ) -> Result<Vec<InventoryItem>, StorageError> {
        let mut entries = entries
            .into_iter()
            .map(inventory_item_from_observation)
            .collect::<Vec<_>>();
        entries.extend(read_managed_entries_from(
            &self.connection,
            &self.data_root,
        )?);
        entries.sort_by(|left, right| left.skill_root.cmp(&right.skill_root));
        Ok(entries)
    }

    fn read_supported_apps(&self) -> Result<Vec<SupportedAppSummary>, StorageError> {
        read_supported_apps_from(&self.connection)
    }

    fn read_inventory_entries(&self) -> Result<Vec<InventoryObservation>, StorageError> {
        read_inventory_entries_from(&self.connection)
    }

    fn read_scan_issues(&self) -> Result<Vec<ScanIssue>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT root_key, path, code, message FROM inventory_scan_issues ORDER BY root_key",
            )
            .map_err(StorageError::ReadInventory)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(StorageError::ReadInventory)?;
        let mut issues = Vec::new();
        for row in rows {
            let (root_key, path, code, message) = row.map_err(StorageError::ReadInventory)?;
            issues.push(ScanIssue {
                root_key: ScanRootKey::from_str(&root_key)
                    .ok_or_else(|| StorageError::UnknownScanRoot(root_key.clone()))?,
                path,
                code: ScanIssueCode::from_str(&code)
                    .ok_or_else(|| StorageError::UnknownScanIssueCode(code.clone()))?,
                message,
            });
        }
        Ok(issues)
    }

    fn read_recovery_issues(&self) -> Result<Vec<RecoveryIssue>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT lifecycle.id, COALESCE(plan.bundle_display_name, lifecycle.bundle_id), COALESCE(lifecycle.error_message, '事务状态无法自动判断')
                 FROM lifecycle_transactions AS lifecycle
                 LEFT JOIN install_plans AS plan ON plan.id = lifecycle.plan_id
                 WHERE lifecycle.status = 'blocked'
                 ORDER BY lifecycle.created_at, lifecycle.id",
            )
            .map_err(StorageError::ReadRecoveryIssues)?;
        let rows = statement
            .query_map([], |row| {
                Ok(RecoveryIssue {
                    id: row.get(0)?,
                    bundle_display_name: row.get(1)?,
                    message: row.get(2)?,
                })
            })
            .map_err(StorageError::ReadRecoveryIssues)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadRecoveryIssues)
    }
}

fn ensure_safe_data_root(data_root: &Path) -> Result<(), StorageError> {
    match fs::symlink_metadata(data_root) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(StorageError::UnsafeDataRoot(data_root.to_owned()));
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            fs::create_dir_all(data_root).map_err(StorageError::CreateDataRoot)?;
        }
        Err(error) => return Err(StorageError::InspectDataRoot(error)),
    }

    // 创建后再检查一次，避免把并发替换成符号链接的目录交给 SQLite。
    let metadata = fs::symlink_metadata(data_root).map_err(StorageError::InspectDataRoot)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StorageError::UnsafeDataRoot(data_root.to_owned()));
    }
    Ok(())
}

fn safe_database_open_path(data_root: &Path, database: &Path) -> Result<PathBuf, StorageError> {
    if database.parent() != Some(data_root) {
        return Err(StorageError::UnsafeDatabase(database.to_owned()));
    }
    match fs::symlink_metadata(database) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.nlink() != 1 {
                return Err(StorageError::UnsafeDatabase(database.to_owned()));
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(StorageError::InspectDatabase(error)),
    }
    let file_name = database.file_name().ok_or_else(|| {
        StorageError::InspectDatabase(std::io::Error::new(
            ErrorKind::InvalidInput,
            "SQLite 路径缺少文件名",
        ))
    })?;
    if !is_single_path_component(file_name.to_string_lossy().as_ref()) {
        return Err(StorageError::UnsafeDatabase(database.to_owned()));
    }
    // macOS 的 /var 本身是符号链接；只规范化已校验目录，最终数据库文件仍由 NOFOLLOW 保护。
    let canonical_root = fs::canonicalize(data_root).map_err(StorageError::InspectDatabase)?;
    Ok(canonical_root.join(file_name))
}

fn ensure_one_lifecycle_row(changed: usize, transaction_id: &str) -> Result<(), StorageError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(StorageError::LifecycleStateConflict(
            transaction_id.to_owned(),
        ))
    }
}

fn validate_managed_install_paths(
    transaction_id: &str,
    plan: &StoredInstallPlan,
    selected: &[&StoredInstallCandidate],
    managed_directory: &str,
    current_target: &str,
    stable_relative_path: &str,
) -> Result<(), StorageError> {
    let anchor_skill_name = selected
        .first()
        .and_then(|candidate| candidate.skill_name.as_deref())
        .ok_or(StorageError::InvalidInstallSelection)?;
    let members_are_safe = selected.iter().all(|candidate| {
        candidate.selectable
            && candidate
                .skill_name
                .as_deref()
                .is_some_and(is_single_path_component)
            && candidate.skill_description.is_some()
            && candidate.content_fingerprint.is_some()
            && is_single_path_component(&candidate.candidate_id)
    });
    if !is_single_path_component(transaction_id)
        || !is_single_path_component(&plan.bundle_id)
        || !members_are_safe
        || managed_directory != format!("bundles/{}", plan.bundle_id)
        || current_target != format!("contents/{transaction_id}")
        || stable_relative_path != format!("members/{anchor_skill_name}")
    {
        return Err(StorageError::UnsafeManagedPath(
            managed_directory.to_owned(),
        ));
    }
    Ok(())
}

fn selected_install_candidates(
    plan: &StoredInstallPlan,
) -> Result<Vec<&StoredInstallCandidate>, StorageError> {
    let selected = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.selected)
        .collect::<Vec<_>>();
    if selected.is_empty() || selected.iter().any(|candidate| !candidate.selectable) {
        Err(StorageError::InvalidInstallSelection)
    } else {
        Ok(selected)
    }
}

fn validate_stored_managed_paths(
    bundle_id: &str,
    skill_name: &str,
    managed_directory: &str,
    current_target: &str,
    stable_relative_path: &str,
) -> Result<(), StorageError> {
    if !is_single_path_component(bundle_id)
        || !is_single_path_component(skill_name)
        || managed_directory != format!("bundles/{bundle_id}")
        || stable_relative_path != format!("members/{skill_name}")
        || !is_safe_current_target(current_target)
    {
        return Err(StorageError::UnsafeManagedPath(
            managed_directory.to_owned(),
        ));
    }
    Ok(())
}

fn is_safe_current_target(value: &str) -> bool {
    let mut components = Path::new(value).components();
    let prefix_is_contents = matches!(
        components.next(),
        Some(std::path::Component::Normal(prefix))
            if prefix == std::ffi::OsStr::new("contents")
    );
    let content_id_is_safe = matches!(
        components.next(),
        Some(std::path::Component::Normal(content_id))
            if is_single_path_component(content_id.to_string_lossy().as_ref())
    );
    prefix_is_contents && content_id_is_safe && components.next().is_none()
}

fn is_single_path_component(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

fn read_supported_apps_from(
    connection: &Connection,
) -> Result<Vec<SupportedAppSummary>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT app_id, display_name, detected FROM supported_app_status ORDER BY sort_order",
        )
        .map_err(StorageError::ReadInventory)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })
        .map_err(StorageError::ReadInventory)?;
    let mut supported_apps = Vec::new();
    for row in rows {
        let (id, display_name, detected) = row.map_err(StorageError::ReadInventory)?;
        supported_apps.push(SupportedAppSummary {
            id: SupportedAppId::from_str(&id)
                .ok_or_else(|| StorageError::UnknownSupportedApp(id.clone()))?,
            display_name,
            detected: Some(detected),
        });
    }
    Ok(supported_apps)
}

fn read_inventory_entries_from(
    connection: &Connection,
) -> Result<Vec<InventoryObservation>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT id, skill_name, declared_name, skill_root, skill_file, location_kind, metadata_status, observed_fingerprint, root_key, stale, management_kind FROM inventory_observations ORDER BY skill_root",
        )
        .map_err(StorageError::ReadInventory)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, bool>(9)?,
                row.get::<_, String>(10)?,
            ))
        })
        .map_err(StorageError::ReadInventory)?;
    let mut entries = Vec::new();
    for row in rows {
        let (
            id,
            skill_name,
            declared_name,
            skill_root,
            skill_file,
            location_kind,
            metadata_status,
            observed_fingerprint,
            root_key,
            stale,
            management_kind,
        ) = row.map_err(StorageError::ReadInventory)?;
        entries.push(InventoryObservation {
            id,
            skill_name,
            declared_name,
            skill_root,
            skill_file,
            location_kind: InventoryLocationKind::from_str(&location_kind)
                .ok_or_else(|| StorageError::UnknownInventoryLocation(location_kind.clone()))?,
            metadata_status: SkillMetadataStatus::from_str(&metadata_status)
                .ok_or_else(|| StorageError::UnknownMetadataStatus(metadata_status.clone()))?,
            observed_by: Vec::new(),
            observed_fingerprint,
            root_key: ScanRootKey::from_str(&root_key)
                .ok_or_else(|| StorageError::UnknownScanRoot(root_key.clone()))?,
            stale,
            management_kind: ManagementKind::from_str(&management_kind)
                .ok_or_else(|| StorageError::UnknownManagementKind(management_kind.clone()))?,
        });
    }

    for entry in &mut entries {
        let mut app_statement = connection
            .prepare(
                "SELECT app_id FROM inventory_observation_apps WHERE observation_id = ?1 ORDER BY app_id",
            )
            .map_err(StorageError::ReadInventory)?;
        let app_rows = app_statement
            .query_map([&entry.id], |row| row.get::<_, String>(0))
            .map_err(StorageError::ReadInventory)?;
        for row in app_rows {
            let id = row.map_err(StorageError::ReadInventory)?;
            entry.observed_by.push(
                SupportedAppId::from_str(&id)
                    .ok_or_else(|| StorageError::UnknownSupportedApp(id.clone()))?,
            );
        }
    }
    Ok(entries)
}

fn read_install_plan_from(
    connection: &Connection,
    plan_id: &str,
) -> Result<Option<StoredInstallPlan>, StorageError> {
    let row = connection
        .query_row(
            "SELECT id, input_path, input_device, input_inode, input_fingerprint, bundle_id, bundle_display_name, member_id, skill_name, skill_description, expires_at, status FROM install_plans WHERE id = ?1",
            [plan_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, String>(11)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadInstallPlan)?;
    let Some((
        id,
        input_path,
        input_device,
        input_inode,
        input_fingerprint,
        bundle_id,
        bundle_display_name,
        member_id,
        skill_name,
        skill_description,
        expires_at,
        status,
    )) = row
    else {
        return Ok(None);
    };
    let candidates = read_install_candidates_from(connection, &id)?;
    Ok(Some(StoredInstallPlan {
        id,
        input_path,
        input_device: input_device as u64,
        input_inode: input_inode as u64,
        input_fingerprint,
        bundle_id,
        bundle_display_name,
        member_id,
        skill_name,
        _legacy_skill_description: skill_description,
        expires_at,
        status,
        candidates,
    }))
}

fn read_install_candidates_from(
    connection: &Connection,
    plan_id: &str,
) -> Result<Vec<StoredInstallCandidate>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT candidate_id, source_relative_path, skill_name, skill_description,
                    content_fingerprint, selectable, validation_errors_json, warnings_json,
                    default_selected, selected
             FROM install_plan_candidates
             WHERE plan_id = ?1
             ORDER BY sort_order",
        )
        .map_err(StorageError::ReadInstallPlan)?;
    let rows = statement
        .query_map([plan_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
            ))
        })
        .map_err(StorageError::ReadInstallPlan)?;
    let mut candidates = Vec::new();
    for row in rows {
        let (
            candidate_id,
            source_relative_path,
            skill_name,
            skill_description,
            content_fingerprint,
            selectable,
            validation_errors_json,
            warnings_json,
            default_selected,
            selected,
        ) = row.map_err(StorageError::ReadInstallPlan)?;
        candidates.push(StoredInstallCandidate {
            candidate_id,
            source_relative_path,
            skill_name,
            skill_description,
            content_fingerprint,
            selectable: sqlite_bool(selectable)?,
            validation_errors: serde_json::from_str(&validation_errors_json)
                .map_err(StorageError::InvalidPlanValidationErrors)?,
            warnings: serde_json::from_str(&warnings_json)
                .map_err(StorageError::InvalidPlanWarnings)?,
            default_selected: sqlite_bool(default_selected)?,
            selected: sqlite_bool(selected)?,
        });
    }
    Ok(candidates)
}

fn sqlite_bool(value: i64) -> Result<bool, StorageError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(StorageError::InvalidPlanBoolean(other)),
    }
}

fn inventory_item_from_observation(observation: InventoryObservation) -> InventoryItem {
    InventoryItem {
        id: observation.id,
        skill_name: observation.skill_name,
        declared_name: observation.declared_name,
        skill_root: observation.skill_root,
        skill_file: observation.skill_file,
        location_kind: observation.location_kind,
        metadata_status: observation.metadata_status,
        observed_by: observation.observed_by,
        observed_fingerprint: observation.observed_fingerprint,
        root_key: Some(observation.root_key),
        stale: observation.stale,
        management_kind: observation.management_kind,
        bundle_id: None,
        bundle_display_name: None,
        source_display_name: None,
        project_display_name: None,
    }
}

fn read_managed_entries_from(
    connection: &Connection,
    data_root: &Path,
) -> Result<Vec<InventoryItem>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT member.id, member.skill_name, member.description, member.stable_relative_path, member.content_fingerprint, bundle.id, bundle.display_name, bundle.managed_directory, bundle.current_target
             FROM skill_members member
             JOIN member_selections selection ON selection.member_id = member.id AND selection.bundle_id = member.bundle_id
             JOIN bundles bundle ON bundle.id = member.bundle_id
             ORDER BY bundle.display_name, member.skill_name, member.id",
        )
        .map_err(StorageError::ReadInventory)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .map_err(StorageError::ReadInventory)?;
    let mut entries = Vec::new();
    for row in rows {
        let (
            member_id,
            skill_name,
            _description,
            stable_relative_path,
            content_fingerprint,
            bundle_id,
            bundle_display_name,
            managed_directory,
            current_target,
        ) = row.map_err(StorageError::ReadInventory)?;
        validate_stored_managed_paths(
            &bundle_id,
            &skill_name,
            &managed_directory,
            &current_target,
            &stable_relative_path,
        )?;
        let skill_root = data_root
            .join(&managed_directory)
            .join("current")
            .join(&stable_relative_path);
        entries.push(InventoryItem {
            id: format!("managed:{member_id}"),
            declared_name: Some(skill_name.clone()),
            skill_name,
            skill_file: skill_root.join("SKILL.md").to_string_lossy().into_owned(),
            skill_root: skill_root.to_string_lossy().into_owned(),
            location_kind: InventoryLocationKind::ManagedStore,
            metadata_status: SkillMetadataStatus::Valid,
            observed_by: Vec::new(),
            observed_fingerprint: content_fingerprint,
            root_key: None,
            stale: false,
            management_kind: ManagementKind::SkillYardManaged,
            bundle_id: Some(bundle_id),
            bundle_display_name: Some(bundle_display_name),
            source_display_name: None,
            project_display_name: None,
        });
    }
    Ok(entries)
}

fn map_lifecycle_insert_error(error: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::SqliteFailure(code, Some(message)) = &error {
        // 只有单写者 partial unique index 冲突才表示已有活跃事务；主键等约束必须保留原因。
        if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
            && message.contains("lifecycle_single_active")
        {
            return StorageError::ActiveLifecycleTransaction;
        }
    }
    StorageError::SaveLifecycleTransaction(error)
}

fn ensure_managed_state_matches(
    transaction: &Transaction<'_>,
    plan: &StoredInstallPlan,
    selected: &[&StoredInstallCandidate],
    managed_directory: &str,
    current_target: &str,
) -> Result<(), StorageError> {
    let bundle = transaction
        .query_row(
            "SELECT display_name, managed_directory, current_target FROM bundles WHERE id = ?1",
            [&plan.bundle_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(StorageError::SaveManagedBundle)?;
    let bundle_matches = bundle
        == (
            plan.bundle_display_name.clone(),
            managed_directory.to_owned(),
            current_target.to_owned(),
        );
    if !bundle_matches {
        return Err(StorageError::ManagedStateConflict);
    }
    let actual_count = transaction
        .query_row(
            "SELECT COUNT(*) FROM skill_members WHERE bundle_id = ?1",
            [&plan.bundle_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(StorageError::SaveManagedBundle)?;
    if actual_count != selected.len() as i64 {
        return Err(StorageError::ManagedStateConflict);
    }
    for candidate in selected {
        let skill_name = candidate
            .skill_name
            .as_ref()
            .ok_or(StorageError::InvalidInstallSelection)?;
        let description = candidate
            .skill_description
            .as_ref()
            .ok_or(StorageError::InvalidInstallSelection)?;
        let fingerprint = candidate
            .content_fingerprint
            .as_ref()
            .ok_or(StorageError::InvalidInstallSelection)?;
        let member = transaction
            .query_row(
                "SELECT bundle_id, skill_name, description, stable_relative_path, content_fingerprint FROM skill_members WHERE id = ?1",
                [&candidate.candidate_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .map_err(StorageError::SaveManagedBundle)?;
        let selected_row = transaction
            .query_row(
                "SELECT COUNT(*) FROM member_selections WHERE bundle_id = ?1 AND member_id = ?2",
                params![plan.bundle_id, candidate.candidate_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StorageError::SaveManagedBundle)?;
        if member
            != (
                plan.bundle_id.clone(),
                skill_name.clone(),
                description.clone(),
                format!("members/{skill_name}"),
                fingerprint.clone(),
            )
            || selected_row != 1
        {
            return Err(StorageError::ManagedStateConflict);
        }
    }
    Ok(())
}

fn replace_inventory_rows(
    transaction: &Transaction<'_>,
    entries: &[InventoryObservation],
    supported_apps: &[SupportedAppSummary],
    scan_issues: &[ScanIssue],
) -> rusqlite::Result<()> {
    transaction.execute("DELETE FROM inventory_observation_apps", [])?;
    transaction.execute("DELETE FROM inventory_observations", [])?;
    transaction.execute("DELETE FROM supported_app_status", [])?;
    transaction.execute("DELETE FROM inventory_scan_issues", [])?;

    for (sort_order, app) in supported_apps.iter().enumerate() {
        transaction.execute(
            "INSERT INTO supported_app_status (app_id, display_name, detected, sort_order) VALUES (?1, ?2, ?3, ?4)",
            params![app.id.as_str(), app.display_name, app.detected.unwrap_or(false), sort_order as i64],
        )?;
    }

    for entry in entries {
        transaction.execute(
            "INSERT INTO inventory_observations (id, skill_name, declared_name, skill_root, skill_file, location_kind, metadata_status, observed_fingerprint, root_key, stale, management_kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                entry.id,
                entry.skill_name,
                entry.declared_name,
                entry.skill_root,
                entry.skill_file,
                entry.location_kind.as_str(),
                entry.metadata_status.as_str(),
                entry.observed_fingerprint,
                entry.root_key.as_str(),
                entry.stale,
                entry.management_kind.as_str()
            ],
        )?;
        for app in &entry.observed_by {
            transaction.execute(
                "INSERT INTO inventory_observation_apps (observation_id, app_id) VALUES (?1, ?2)",
                params![entry.id, app.as_str()],
            )?;
        }
    }

    for issue in scan_issues {
        transaction.execute(
            "INSERT INTO inventory_scan_issues (root_key, path, code, message) VALUES (?1, ?2, ?3, ?4)",
            params![
                issue.root_key.as_str(),
                issue.path,
                issue.code.as_str(),
                issue.message
            ],
        )?;
    }
    Ok(())
}

fn reconcile_entries(
    previous: &[InventoryObservation],
    scanned: &[InventoryObservation],
    successful_roots: &[ScanRootKey],
    scan_issues: &[ScanIssue],
) -> Vec<InventoryObservation> {
    let successful = successful_roots.iter().copied().collect::<BTreeSet<_>>();
    let failed = scan_issues
        .iter()
        .map(|issue| issue.root_key)
        .collect::<BTreeSet<_>>();
    let previous_by_id = previous
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut combined = previous
        .iter()
        .filter(|entry| !successful.contains(&entry.root_key))
        .cloned()
        .map(|mut entry| {
            if failed.contains(&entry.root_key) {
                entry.stale = true;
            }
            entry
        })
        .collect::<Vec<_>>();
    combined.extend(scanned.iter().cloned().map(|mut entry| {
        // 扫描只更新观察事实，不能覆盖由确定性证据或用户确认产生的管理归属。
        if let Some(previous_entry) = previous_by_id.get(entry.id.as_str()) {
            entry.management_kind = previous_entry.management_kind;
        }
        entry
    }));
    combined.sort_by(|left, right| left.skill_root.cmp(&right.skill_root));
    combined
}

fn reconcile_supported_apps(
    previous: &[SupportedAppSummary],
    scanned: &[SupportedAppSummary],
) -> Vec<SupportedAppSummary> {
    let mut combined = previous.to_vec();
    for app in scanned {
        if let Some(index) = combined.iter().position(|current| current.id == app.id) {
            combined[index] = app.clone();
        } else {
            combined.push(app.clone());
        }
    }
    combined.sort_by_key(|app| match app.id {
        SupportedAppId::Codex => 0,
        SupportedAppId::ClaudeCode => 1,
        SupportedAppId::GitHubCopilot => 2,
    });
    combined
}

fn summarize_changes(
    completed_at: i64,
    previous: &[InventoryObservation],
    current: &[InventoryObservation],
) -> LocalRefreshSummary {
    let previous_by_id = previous
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let current_by_id = current
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let added = current_by_id
        .keys()
        .filter(|id| !previous_by_id.contains_key(*id))
        .count();
    let removed = previous_by_id
        .keys()
        .filter(|id| !current_by_id.contains_key(*id))
        .count();
    let changed = current_by_id
        .iter()
        .filter(|(id, current_entry)| {
            previous_by_id
                .get(*id)
                .is_some_and(|previous_entry| observation_changed(previous_entry, current_entry))
        })
        .count();

    LocalRefreshSummary {
        completed_at,
        added,
        changed,
        removed,
    }
}

fn observation_changed(previous: &InventoryObservation, current: &InventoryObservation) -> bool {
    previous.skill_name != current.skill_name
        || previous.declared_name != current.declared_name
        || previous.skill_file != current.skill_file
        || previous.location_kind != current.location_kind
        || previous.metadata_status != current.metadata_status
        || previous.observed_by != current.observed_by
        || previous.observed_fingerprint != current.observed_fingerprint
        || previous.management_kind != current.management_kind
}

fn refresh_count(value: i64) -> Result<usize, StorageError> {
    usize::try_from(value).map_err(|_| StorageError::InvalidRefreshCount(value))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::symlink,
        path::Path,
        sync::{Arc, Barrier},
    };

    use tempfile::tempdir;

    use super::*;

    fn open_test_storage(root: &Path) -> Storage {
        let data_root = root.join("data");
        let database = data_root.join("skillyard.sqlite3");
        Storage::open(&data_root, &database).expect("应打开隔离 SQLite")
    }

    fn save_test_plan(storage: &mut Storage, plan_id: &str, bundle_id: &str, member_id: &str) {
        let candidates = [NewInstallCandidate {
            candidate_id: member_id,
            source_relative_path: "",
            skill_name: Some(member_id),
            skill_description: Some("测试 Skill"),
            content_fingerprint: Some("sha256:test"),
            selectable: true,
            validation_errors: &[],
            warnings: &[],
            default_selected: true,
        }];
        storage
            .save_install_plan(NewInstallPlan {
                id: plan_id,
                input_path: "/tmp/example-skill",
                input_device: 1,
                input_inode: 2,
                input_fingerprint: "sha256:test",
                bundle_id,
                bundle_display_name: bundle_id,
                member_id,
                skill_name: member_id,
                skill_description: "测试 Skill",
                warnings: &[],
                candidates: &candidates,
                created_at: 100,
                expires_at: 1_000,
            })
            .expect("应保存安装 Plan");
    }

    fn advance_to_candidate_ready(storage: &mut Storage, transaction_id: &str) {
        storage
            .update_lifecycle_phase(transaction_id, "journal_ready", 201)
            .expect("应记录 Journal 已就绪");
        storage
            .update_lifecycle_phase(transaction_id, "candidate_ready", 202)
            .expect("应记录候选内容已就绪");
    }

    #[test]
    fn open_rejects_a_data_root_symlink_or_file() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let actual = sandbox.path().join("actual");
        fs::create_dir(&actual).expect("应创建符号链接目标目录");
        let data_root = sandbox.path().join("data");
        symlink(&actual, &data_root).expect("应创建数据目录符号链接");

        let error = Storage::open(&data_root, &data_root.join("skillyard.sqlite3"))
            .err()
            .expect("数据根目录是符号链接时必须拒绝");

        assert!(matches!(error, StorageError::UnsafeDataRoot(path) if path == data_root));
        assert!(
            !actual.join("skillyard.sqlite3").exists(),
            "拒绝前不能在符号链接目标创建 SQLite"
        );

        fs::remove_file(&data_root).expect("应移除测试符号链接");
        fs::write(&data_root, []).expect("应创建同名普通文件");
        let file_error = Storage::open(&data_root, &data_root.join("skillyard.sqlite3"))
            .err()
            .expect("数据根目录是普通文件时必须拒绝");
        assert!(matches!(
            file_error,
            StorageError::UnsafeDataRoot(path) if path == data_root
        ));
    }

    #[test]
    fn open_rejects_a_database_symlink_or_directory() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        fs::create_dir(&data_root).expect("应创建数据目录");
        let target = data_root.join("actual.sqlite3");
        fs::write(&target, []).expect("应创建符号链接目标文件");
        let database = data_root.join("skillyard.sqlite3");
        symlink(&target, &database).expect("应创建数据库符号链接");

        let symlink_error = Storage::open(&data_root, &database)
            .err()
            .expect("数据库是符号链接时必须拒绝");
        assert!(matches!(
            symlink_error,
            StorageError::UnsafeDatabase(path) if path == database
        ));

        fs::remove_file(&database).expect("应移除测试符号链接");
        fs::create_dir(&database).expect("应创建同名目录");
        let directory_error = Storage::open(&data_root, &database)
            .err()
            .expect("数据库路径是目录时必须拒绝");
        assert!(matches!(
            directory_error,
            StorageError::UnsafeDatabase(path) if path == database
        ));
    }

    #[test]
    fn open_rejects_a_hard_linked_database() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        fs::create_dir(&data_root).expect("应创建数据目录");
        let outside = sandbox.path().join("shared.sqlite3");
        fs::write(&outside, []).expect("应创建外部文件");
        let database = data_root.join("skillyard.sqlite3");
        fs::hard_link(&outside, &database).expect("应创建数据库硬链接");

        let error = Storage::open(&data_root, &database)
            .err()
            .expect("数据库硬链接必须被拒绝");

        assert!(matches!(error, StorageError::UnsafeDatabase(path) if path == database));
        assert!(fs::read(&outside).expect("外部文件应保持可读").is_empty());
    }

    #[test]
    fn open_rejects_a_database_outside_the_data_root() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        let outside_database = sandbox.path().join("outside.sqlite3");

        let error = Storage::open(&data_root, &outside_database)
            .err()
            .expect("SQLite 必须属于 Central Store 根目录");

        assert!(matches!(
            error,
            StorageError::UnsafeDatabase(path) if path == outside_database
        ));
        assert!(!outside_database.exists(), "拒绝前不能创建外部 SQLite");
    }

    #[test]
    fn aborted_and_completed_transactions_remain_recoverable_until_cleanup() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan-abort", "bundle-abort", "member-abort");
        let aborted_plan = storage
            .begin_install_transaction("plan-abort", "txn-abort", "journals/abort.json", 200)
            .expect("应开始待中止事务");
        storage
            .abort_lifecycle_transaction("txn-abort", None, 201)
            .expect("应持久化中止状态");

        save_test_plan(
            &mut storage,
            "plan-complete",
            "bundle-complete",
            "member-complete",
        );
        let completed_plan = storage
            .begin_install_transaction(
                "plan-complete",
                "txn-complete",
                "journals/complete.json",
                202,
            )
            .expect("应开始待完成事务");
        advance_to_candidate_ready(&mut storage, "txn-complete");
        storage
            .finalize_install(
                "txn-complete",
                &completed_plan,
                "bundles/bundle-complete",
                "contents/txn-complete",
                "members/member-complete",
                203,
            )
            .expect("应提交受管状态");

        let statuses = storage
            .recoverable_lifecycle_transactions()
            .expect("应读取可恢复事务")
            .into_iter()
            .map(|transaction| (transaction.id, transaction.status))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            statuses.get("txn-abort").map(String::as_str),
            Some("aborted")
        );
        assert_eq!(
            statuses.get("txn-complete").map(String::as_str),
            Some("completed")
        );
        assert_eq!(aborted_plan.id, "plan-abort");
    }

    #[test]
    fn lifecycle_state_changes_reject_unknown_or_invalid_transactions() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());

        assert!(matches!(
            storage.update_lifecycle_phase("missing", "journal_ready", 200),
            Err(StorageError::LifecycleStateConflict(id)) if id == "missing"
        ));
        assert!(matches!(
            storage.abort_lifecycle_transaction("missing", None, 200),
            Err(StorageError::LifecycleStateConflict(id)) if id == "missing"
        ));
        assert!(matches!(
            storage.block_lifecycle_transaction("missing", "测试", 200),
            Err(StorageError::LifecycleStateConflict(id)) if id == "missing"
        ));

        save_test_plan(&mut storage, "plan", "bundle", "member");
        storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 201)
            .expect("应开始事务");
        assert!(matches!(
            storage.update_lifecycle_phase("txn", "candidate_ready", 202),
            Err(StorageError::LifecycleStateConflict(id)) if id == "txn"
        ));
        storage
            .update_lifecycle_phase("txn", "journal_ready", 203)
            .expect("应允许进入下一阶段");
        assert!(matches!(
            storage.update_lifecycle_phase("txn", "activated", 204),
            Err(StorageError::LifecycleStateConflict(id)) if id == "txn"
        ));
        storage
            .update_lifecycle_phase("txn", "candidate_ready", 205)
            .expect("应允许进入候选阶段");
        storage
            .update_lifecycle_phase("txn", "activated", 206)
            .expect("应允许进入生效阶段");
        assert!(matches!(
            storage.abort_lifecycle_transaction("txn", None, 207),
            Err(StorageError::LifecycleStateConflict(id)) if id == "txn"
        ));
        assert!(matches!(
            storage.update_lifecycle_phase("txn", "unknown", 208),
            Err(StorageError::InvalidLifecyclePhase(phase)) if phase == "unknown"
        ));
    }

    #[test]
    fn a_completed_transaction_can_be_blocked_when_recovery_finds_damage() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        let plan = storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        advance_to_candidate_ready(&mut storage, "txn");
        storage
            .finalize_install(
                "txn",
                &plan,
                "bundles/bundle",
                "contents/txn",
                "members/member",
                201,
            )
            .expect("应完成事务");

        storage
            .block_lifecycle_transaction("txn", "current 已损坏", 202)
            .expect("完成后的清理恢复仍应允许阻塞异常状态");

        let transaction = storage
            .recoverable_lifecycle_transactions()
            .expect("应读取阻塞事务")
            .into_iter()
            .find(|transaction| transaction.id == "txn")
            .expect("阻塞事务应保留");
        assert_eq!(transaction.status, "blocked");
    }

    #[test]
    fn an_aborted_transaction_can_be_blocked_when_cleanup_finds_damage() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        storage
            .abort_lifecycle_transaction("txn", None, 201)
            .expect("应中止事务");

        storage
            .block_lifecycle_transaction("txn", "清理时发现外部内容", 202)
            .expect("中止后的清理异常必须持久化为阻塞");

        let transaction = storage
            .recoverable_lifecycle_transactions()
            .expect("应读取阻塞事务")
            .into_iter()
            .find(|transaction| transaction.id == "txn")
            .expect("阻塞事务应保留");
        assert_eq!(transaction.status, "blocked");
    }

    #[test]
    fn blocked_transactions_are_visible_in_startup_and_refresh_inventory() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        storage
            .block_lifecycle_transaction("txn", "current 状态无法判断", 201)
            .expect("应保存阻塞恢复状态");
        storage
            .save_initial_scan(202, &[], &[])
            .expect("应保存初始清单");

        let Some(UiOutcome::Inventory {
            recovery_issues, ..
        }) = storage.read_initial_scan().expect("应读取启动清单")
        else {
            panic!("完成过扫描后应返回 Inventory");
        };
        assert_eq!(recovery_issues.len(), 1);
        assert_eq!(recovery_issues[0].id, "txn");
        assert_eq!(recovery_issues[0].bundle_display_name, "bundle");
        assert_eq!(recovery_issues[0].message, "current 状态无法判断");

        let refreshed = storage
            .save_local_refresh(203, &[], &[], &[], &[])
            .expect("刷新不应隐藏人工恢复状态");
        assert_eq!(refreshed.recovery_issues, recovery_issues);
    }

    #[test]
    fn blocked_transaction_remains_visible_when_its_plan_row_is_missing() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle-fallback", "member");
        storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        storage
            .block_lifecycle_transaction("txn", "Plan 已损坏", 201)
            .expect("应保存阻塞状态");
        storage
            .connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF; DELETE FROM install_plans; PRAGMA foreign_keys = ON;",
            )
            .expect("应模拟损坏数据库中缺失 Plan");

        let issues = storage.read_recovery_issues().expect("阻塞事务必须仍可见");

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].bundle_display_name, "bundle-fallback");
        assert_eq!(issues[0].message, "Plan 已损坏");
    }

    #[test]
    fn unrelated_unique_constraint_is_not_reported_as_an_active_transaction() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan-one", "bundle-one", "member-one");
        storage
            .begin_install_transaction("plan-one", "same-id", "journals/one.json", 200)
            .expect("应开始首个事务");
        storage
            .abort_lifecycle_transaction("same-id", None, 201)
            .expect("应中止首个事务");
        save_test_plan(&mut storage, "plan-two", "bundle-two", "member-two");

        let error = storage
            .begin_install_transaction("plan-two", "same-id", "journals/two.json", 202)
            .expect_err("重复事务 ID 应被 SQLite 拒绝");

        assert!(matches!(error, StorageError::SaveLifecycleTransaction(_)));
        assert_eq!(
            storage
                .read_install_plan("plan-two")
                .expect("失败事务不应消耗 Plan")
                .status,
            "pending"
        );
    }

    #[test]
    fn only_the_single_writer_index_is_reported_as_an_active_transaction() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan-one", "bundle-one", "member-one");
        storage
            .begin_install_transaction("plan-one", "txn-one", "journals/one.json", 200)
            .expect("应开始首个事务");
        save_test_plan(&mut storage, "plan-two", "bundle-two", "member-two");

        let error = storage
            .begin_install_transaction("plan-two", "txn-two", "journals/two.json", 201)
            .expect_err("单写者索引应拒绝第二个活跃事务");

        assert!(matches!(error, StorageError::ActiveLifecycleTransaction));
        assert_eq!(
            storage
                .read_install_plan("plan-two")
                .expect("失败事务不应消耗 Plan")
                .status,
            "pending"
        );
    }

    #[test]
    fn finalize_rejects_a_missing_transaction_and_rolls_back_managed_rows() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        let plan = storage.read_install_plan("plan").expect("应读取安装 Plan");

        let error = storage
            .finalize_install(
                "missing",
                &plan,
                "bundles/bundle",
                "contents/missing",
                "members/member",
                200,
            )
            .expect_err("缺少生命周期事务时不能提交受管状态");

        assert!(matches!(
            error,
            StorageError::LifecycleStateConflict(id) if id == "missing"
        ));
        assert!(
            storage
                .managed_bundle_notice_rows()
                .expect("应读取受管 Bundle")
                .is_empty(),
            "失败的 finalize 必须回滚已插入的 Bundle"
        );
    }

    #[test]
    fn finalize_requires_the_matching_plan_and_a_publishable_phase() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan-one", "bundle-one", "member-one");
        let plan_one = storage
            .begin_install_transaction("plan-one", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        save_test_plan(&mut storage, "plan-two", "bundle-two", "member-two");
        let plan_two = storage
            .read_install_plan("plan-two")
            .expect("应读取另一份 Plan");

        let early_error = storage
            .finalize_install(
                "txn",
                &plan_one,
                "bundles/bundle-one",
                "contents/txn",
                "members/member-one",
                201,
            )
            .expect_err("尚未发布候选内容时不能 finalize");
        assert!(matches!(
            early_error,
            StorageError::LifecycleStateConflict(id) if id == "txn"
        ));

        advance_to_candidate_ready(&mut storage, "txn");
        let wrong_plan_error = storage
            .finalize_install(
                "txn",
                &plan_two,
                "bundles/bundle-two",
                "contents/txn",
                "members/member-two",
                203,
            )
            .expect_err("事务不能提交另一份 Plan");
        assert!(matches!(
            wrong_plan_error,
            StorageError::LifecycleStateConflict(id) if id == "txn"
        ));
        assert!(
            storage
                .managed_bundle_notice_rows()
                .expect("应读取受管 Bundle")
                .is_empty(),
            "身份或阶段不匹配时必须回滚受管记录"
        );
    }

    #[test]
    fn finalize_rejects_paths_outside_the_fixed_bundle_layout() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        let plan = storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        advance_to_candidate_ready(&mut storage, "txn");

        for (managed_directory, current_target, stable_relative_path) in [
            ("../bundle", "contents/txn", "members/member"),
            ("bundles/bundle", "/tmp/txn", "members/member"),
            ("bundles/bundle", "contents/txn", "members/../member"),
        ] {
            assert!(matches!(
                storage.finalize_install(
                    "txn",
                    &plan,
                    managed_directory,
                    current_target,
                    stable_relative_path,
                    203,
                ),
                Err(StorageError::UnsafeManagedPath(_))
            ));
        }
        assert!(
            storage
                .managed_bundle_notice_rows()
                .expect("应读取受管 Bundle")
                .is_empty(),
            "非法路径不能留下受管记录"
        );
    }

    #[test]
    fn managed_inventory_rejects_paths_tampered_in_sqlite() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        let plan = storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        advance_to_candidate_ready(&mut storage, "txn");
        storage
            .finalize_install(
                "txn",
                &plan,
                "bundles/bundle",
                "contents/txn",
                "members/member",
                203,
            )
            .expect("应完成事务");
        storage
            .save_initial_scan(204, &[], &[])
            .expect("应建立可读取的清单状态");
        storage
            .connection
            .execute(
                "UPDATE bundles SET managed_directory = '../../outside' WHERE id = 'bundle'",
                [],
            )
            .expect("应模拟 SQLite 被外部篡改");

        assert!(matches!(
            storage.read_initial_scan(),
            Err(StorageError::UnsafeManagedPath(path)) if path == "../../outside"
        ));
        assert!(matches!(
            storage.managed_bundle_notice_rows(),
            Err(StorageError::UnsafeManagedPath(path)) if path == "../../outside"
        ));
    }

    #[test]
    fn cleanup_rejects_an_existing_non_terminal_transaction_but_is_idempotent_when_missing() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");

        assert!(matches!(
            storage.forget_terminal_transaction("txn"),
            Err(StorageError::LifecycleStateConflict(id)) if id == "txn"
        ));
        storage
            .forget_terminal_transaction("missing")
            .expect("已经清理的事务应保持幂等");
    }

    #[test]
    fn concurrent_open_applies_each_migration_only_once() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        let database = data_root.join("skillyard.sqlite3");
        let barrier = Arc::new(Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let data_root = data_root.clone();
                let database = database.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    Storage::open(&data_root, &database).map(|_| ())
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle
                .join()
                .expect("并发 migration 线程不应 panic")
                .expect("并发打开应成功");
        }
    }
}
