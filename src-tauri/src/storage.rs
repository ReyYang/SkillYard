use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use rusqlite::{Connection, Transaction, params};
use thiserror::Error;

use crate::domain::{
    InventoryLocationKind, InventoryObservation, LocalRefreshSummary, ManagementKind, ScanIssue,
    ScanIssueCode, ScanRootKey, SkillMetadataStatus, SupportedAppId, SupportedAppSummary,
    UiOutcome,
};

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_initial.sql")),
    (2, include_str!("../migrations/0002_local_inventory.sql")),
];

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("无法创建 SkillYard 数据目录：{0}")]
    CreateDataRoot(#[source] std::io::Error),
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
}

pub struct Storage {
    connection: Connection,
}

pub struct SavedLocalRefresh {
    pub entries: Vec<InventoryObservation>,
    pub supported_apps: Vec<SupportedAppSummary>,
    pub summary: LocalRefreshSummary,
}

impl Storage {
    pub fn open(data_root: &Path, database: &Path) -> Result<Self, StorageError> {
        fs::create_dir_all(data_root).map_err(StorageError::CreateDataRoot)?;
        let connection = Connection::open(database).map_err(StorageError::OpenDatabase)?;
        let mut storage = Self { connection };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&mut self) -> Result<(), StorageError> {
        // migration 目录自身必须先存在，后续版本才可以被真正跳过。
        self.connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                 );",
            )
            .map_err(StorageError::Migration)?;

        for (version, migration) in MIGRATIONS {
            let applied = self
                .connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
                    [version],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(StorageError::Migration)?;
            if applied {
                continue;
            }

            let transaction = self
                .connection
                .transaction()
                .map_err(StorageError::Migration)?;
            transaction
                .execute_batch(migration)
                .map_err(StorageError::Migration)?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (?1, unixepoch())",
                    [version],
                )
                .map_err(StorageError::Migration)?;
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
        let entries = self.read_inventory_entries()?;
        let scan_issues = self.read_scan_issues()?;
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
            .transaction()
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
        let previous_entries = self.read_inventory_entries()?;
        let previous_apps = self.read_supported_apps()?;
        let entries = reconcile_entries(
            &previous_entries,
            scanned_entries,
            successful_roots,
            scan_issues,
        );
        let supported_apps = reconcile_supported_apps(&previous_apps, scanned_apps);
        let summary = summarize_changes(completed_at, &previous_entries, &entries);

        let transaction = self
            .connection
            .transaction()
            .map_err(StorageError::SaveLocalRefresh)?;
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
            entries,
            supported_apps,
            summary,
        })
    }

    fn read_supported_apps(&self) -> Result<Vec<SupportedAppSummary>, StorageError> {
        let mut statement = self
            .connection
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

    fn read_inventory_entries(&self) -> Result<Vec<InventoryObservation>, StorageError> {
        let mut statement = self
            .connection
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
            let mut app_statement = self
                .connection
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
    combined.extend(scanned.iter().cloned());
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
