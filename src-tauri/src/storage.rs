use std::{fs, path::Path};

use rusqlite::{Connection, params};
use thiserror::Error;

use crate::domain::{
    InventoryLocationKind, InventoryObservation, SkillMetadataStatus, SupportedAppId,
    SupportedAppSummary, UiOutcome,
};

const INITIAL_MIGRATION: &str = include_str!("../migrations/0001_initial.sql");

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("无法创建 SkillYard 数据目录：{0}")]
    CreateDataRoot(#[source] std::io::Error),
    #[error("无法打开 SkillYard SQLite：{0}")]
    OpenDatabase(#[source] rusqlite::Error),
    #[error("无法执行 SkillYard SQLite migration：{0}")]
    Migration(#[source] rusqlite::Error),
    #[error("无法读取首次扫描状态：{0}")]
    ReadStartupState(#[source] rusqlite::Error),
    #[error("无法保存首次扫描结果：{0}")]
    SaveInitialScan(#[source] rusqlite::Error),
    #[error("SQLite 中包含未知 Supported App：{0}")]
    UnknownSupportedApp(String),
    #[error("SQLite 中包含未知 Inventory location：{0}")]
    UnknownInventoryLocation(String),
    #[error("SQLite 中包含未知 Skill metadata 状态：{0}")]
    UnknownMetadataStatus(String),
}

pub struct Storage {
    connection: Connection,
}

impl Storage {
    pub fn open(data_root: &Path, database: &Path) -> Result<Self, StorageError> {
        fs::create_dir_all(data_root).map_err(StorageError::CreateDataRoot)?;
        let connection = Connection::open(database).map_err(StorageError::OpenDatabase)?;
        let storage = Self { connection };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&self) -> Result<(), StorageError> {
        self.connection
            .execute_batch(INITIAL_MIGRATION)
            .map_err(StorageError::Migration)
    }

    pub fn read_initial_scan(&self) -> Result<Option<UiOutcome>, StorageError> {
        let completed_at: Option<i64> = self
            .connection
            .query_row(
                "SELECT initial_scan_completed_at FROM app_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(StorageError::ReadStartupState)?;
        let Some(scan_completed_at) = completed_at else {
            return Ok(None);
        };

        let mut app_statement = self
            .connection
            .prepare(
                "SELECT app_id, display_name, detected FROM supported_app_status ORDER BY sort_order",
            )
            .map_err(StorageError::ReadStartupState)?;
        let app_rows = app_statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })
            .map_err(StorageError::ReadStartupState)?;
        let mut supported_apps = Vec::new();
        for row in app_rows {
            let (id, display_name, detected) = row.map_err(StorageError::ReadStartupState)?;
            supported_apps.push(SupportedAppSummary {
                id: SupportedAppId::from_str(&id)
                    .ok_or_else(|| StorageError::UnknownSupportedApp(id.clone()))?,
                display_name,
                detected: Some(detected),
            });
        }

        let entries = self.read_inventory_entries()?;
        Ok(Some(UiOutcome::Inventory {
            scan_completed_at,
            entries,
            supported_apps,
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
        transaction
            .execute("DELETE FROM inventory_observation_apps", [])
            .map_err(StorageError::SaveInitialScan)?;
        transaction
            .execute("DELETE FROM inventory_observations", [])
            .map_err(StorageError::SaveInitialScan)?;
        transaction
            .execute("DELETE FROM supported_app_status", [])
            .map_err(StorageError::SaveInitialScan)?;

        for (sort_order, app) in supported_apps.iter().enumerate() {
            transaction
                .execute(
                    "INSERT INTO supported_app_status (app_id, display_name, detected, sort_order) VALUES (?1, ?2, ?3, ?4)",
                    params![app.id.as_str(), app.display_name, app.detected.unwrap_or(false), sort_order as i64],
                )
                .map_err(StorageError::SaveInitialScan)?;
        }

        for entry in entries {
            transaction
                .execute(
                    "INSERT INTO inventory_observations (id, skill_name, declared_name, skill_root, skill_file, location_kind, metadata_status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![entry.id, entry.skill_name, entry.declared_name, entry.skill_root, entry.skill_file, entry.location_kind.as_str(), entry.metadata_status.as_str()],
                )
                .map_err(StorageError::SaveInitialScan)?;
            for app in &entry.observed_by {
                transaction
                    .execute(
                        "INSERT INTO inventory_observation_apps (observation_id, app_id) VALUES (?1, ?2)",
                        params![entry.id, app.as_str()],
                    )
                    .map_err(StorageError::SaveInitialScan)?;
            }
        }

        transaction
            .execute(
                "UPDATE app_state SET initial_scan_completed_at = ?1 WHERE singleton = 1",
                [scan_completed_at],
            )
            .map_err(StorageError::SaveInitialScan)?;
        transaction.commit().map_err(StorageError::SaveInitialScan)
    }

    fn read_inventory_entries(&self) -> Result<Vec<InventoryObservation>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, skill_name, declared_name, skill_root, skill_file, location_kind, metadata_status FROM inventory_observations ORDER BY skill_root",
            )
            .map_err(StorageError::ReadStartupState)?;
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
                ))
            })
            .map_err(StorageError::ReadStartupState)?;
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
            ) = row.map_err(StorageError::ReadStartupState)?;
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
            });
        }

        for entry in &mut entries {
            let mut app_statement = self
                .connection
                .prepare(
                    "SELECT app_id FROM inventory_observation_apps WHERE observation_id = ?1 ORDER BY app_id",
                )
                .map_err(StorageError::ReadStartupState)?;
            let app_rows = app_statement
                .query_map([&entry.id], |row| row.get::<_, String>(0))
                .map_err(StorageError::ReadStartupState)?;
            for row in app_rows {
                let id = row.map_err(StorageError::ReadStartupState)?;
                entry.observed_by.push(
                    SupportedAppId::from_str(&id)
                        .ok_or_else(|| StorageError::UnknownSupportedApp(id.clone()))?,
                );
            }
        }
        Ok(entries)
    }
}
