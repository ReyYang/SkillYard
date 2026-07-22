use std::{
    collections::BTreeSet,
    fs,
    io::ErrorKind,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    content::{ContentValidationError, ValidatedSingleSkill, validate_single_skill_folder},
    domain::{
        InventoryItem, ManagementKind, MountScope, SkillMetadataStatus, SupportedAppId,
        TakeoverIdentityBasis, TakeoverOriginDisposition, TakeoverPlan, TakeoverPlanOrigin,
        TakeoverPlanRequest, TakeoverPlanTarget, UiOutcome,
    },
    paths::ApplicationPaths,
    storage::{Storage, StorageError, StoredTakeoverPlanRow},
};

const TAKEOVER_PLAN_TTL_MILLIS: i64 = 15 * 60 * 1_000;

#[derive(Debug, Error)]
pub enum TakeoverError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Content(#[from] ContentValidationError),
    #[error("接管选择无效：{0}")]
    InvalidRequest(String),
    #[error("接管路径不是可表示的 UTF-8 路径：{0}")]
    NonUnicodePath(String),
    #[error("无法{action} {path}：{source}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Takeover Plan 的持久化合同不一致")]
    InvalidPlanContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TakeoverPlanContract {
    plan: TakeoverPlan,
    origin_snapshots: Vec<TakeoverOriginSnapshot>,
    target_snapshots: Vec<TakeoverTargetSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TakeoverOriginSnapshot {
    observation_id: String,
    root_device: u64,
    root_inode: u64,
    root_mode: u32,
    parent_device: u64,
    parent_inode: u64,
    parent_mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TakeoverTargetSnapshot {
    target_path: String,
    parent_device: u64,
    parent_inode: u64,
    parent_mode: u32,
    occupied_by_observation_id: Option<String>,
}

struct PreparedOrigin {
    observation: InventoryItem,
    app_id: SupportedAppId,
    original_root: PathBuf,
    validated: ValidatedSingleSkill,
    root_metadata: fs::Metadata,
    parent_metadata: fs::Metadata,
}

/// Plan 构建只读取已保存 Inventory 与真实文件系统，并把完整合同写入 SQLite。
pub(crate) fn create_takeover_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    request: TakeoverPlanRequest,
    now: i64,
) -> Result<TakeoverPlan, TakeoverError> {
    validate_plan_request(&request)?;
    let observations = read_observations(storage, &request.observation_ids)?;
    let app_configs = paths.supported_apps();
    let mut prepared_origins = Vec::with_capacity(observations.len());
    let mut original_paths = BTreeSet::new();
    for observation in observations {
        validate_observation(&observation)?;
        let root_key = observation.root_key.ok_or_else(|| {
            TakeoverError::InvalidRequest("扫描观察缺少受支持路径信息".to_owned())
        })?;
        let app = app_configs
            .iter()
            .find(|app| app.root_key == root_key)
            .ok_or_else(|| {
                TakeoverError::InvalidRequest("第一个切片只接受应用专属 global Skill".to_owned())
            })?;
        let original_root = PathBuf::from(&observation.skill_root);
        if !original_paths.insert(original_root.clone()) {
            return Err(TakeoverError::InvalidRequest(
                "接管选择包含重复的原始路径".to_owned(),
            ));
        }
        let validated = validate_single_skill_folder(&original_root)?;
        if validated.name != observation.skill_name {
            return Err(TakeoverError::InvalidRequest(
                "扫描结果与当前 Skill 内容不一致，请刷新本机后重试".to_owned(),
            ));
        }
        if app.global_root.join(&validated.name) != original_root {
            return Err(TakeoverError::InvalidRequest(
                "Skill 不位于受支持应用的固定 global 路径".to_owned(),
            ));
        }
        let parent = original_root
            .parent()
            .ok_or_else(|| TakeoverError::InvalidRequest("Skill 根目录缺少父目录".to_owned()))?;
        prepared_origins.push(PreparedOrigin {
            root_metadata: metadata(&original_root, "读取原 Skill 身份")?,
            parent_metadata: metadata(parent, "读取原 Skill 父目录身份")?,
            observation,
            app_id: app.id,
            original_root,
            validated,
        });
    }
    let selected = prepared_origins
        .iter()
        .find(|origin| origin.observation.id == request.selected_observation_id)
        .ok_or_else(|| TakeoverError::InvalidRequest("所选内容副本已经不存在".to_owned()))?;
    let skill_name = selected.validated.name.clone();
    let skill_description = selected.validated.description.clone();
    let selected_warnings = selected.validated.warnings.clone();
    if prepared_origins
        .iter()
        .any(|origin| origin.validated.name != skill_name)
    {
        return Err(TakeoverError::InvalidRequest(
            "只有同名副本可以确认成同一个本地 Skill Identity".to_owned(),
        ));
    }
    let bundle_id = Uuid::new_v4().to_string();
    let member_id = Uuid::new_v4().to_string();
    let content_id = Uuid::new_v4().to_string();
    let plan_id = Uuid::new_v4().to_string();
    let managed_directory = paths.bundle_directory(&bundle_id);
    ensure_absent(&managed_directory)?;
    let content_directory = managed_directory.join("contents").join(&content_id);
    let expected_target = managed_directory
        .join("current")
        .join("members")
        .join(&skill_name);
    let expected_target_text = path_text(&expected_target)?;
    let mut origins = Vec::with_capacity(prepared_origins.len());
    let mut origin_snapshots = Vec::with_capacity(prepared_origins.len());
    let mut targets = Vec::with_capacity(request.preserved_observation_ids.len());
    let mut target_snapshots = Vec::with_capacity(request.preserved_observation_ids.len());
    for prepared in prepared_origins {
        let preserve_mount = request
            .preserved_observation_ids
            .contains(&prepared.observation.id);
        let original_path = path_text(&prepared.original_root)?;
        origin_snapshots.push(TakeoverOriginSnapshot {
            observation_id: prepared.observation.id.clone(),
            root_device: prepared.root_metadata.dev(),
            root_inode: prepared.root_metadata.ino(),
            root_mode: prepared.root_metadata.mode(),
            parent_device: prepared.parent_metadata.dev(),
            parent_inode: prepared.parent_metadata.ino(),
            parent_mode: prepared.parent_metadata.mode(),
        });
        origins.push(TakeoverPlanOrigin {
            observation_id: prepared.observation.id.clone(),
            original_path: original_path.clone(),
            app_id: Some(prepared.app_id),
            scope: Some(MountScope::Global),
            project_id: None,
            project_display_name: None,
            content_fingerprint: prepared.validated.fingerprint,
            warnings: prepared.validated.warnings,
            final_disposition: if preserve_mount {
                TakeoverOriginDisposition::Mount
            } else {
                TakeoverOriginDisposition::Remove
            },
        });
        if preserve_mount {
            targets.push(TakeoverPlanTarget {
                mount_id: Uuid::new_v4().to_string(),
                app_id: prepared.app_id,
                scope: MountScope::Global,
                project_id: None,
                project_display_name: None,
                target_path: original_path.clone(),
                expected_target: expected_target_text.clone(),
            });
            target_snapshots.push(TakeoverTargetSnapshot {
                target_path: original_path,
                parent_device: prepared.parent_metadata.dev(),
                parent_inode: prepared.parent_metadata.ino(),
                parent_mode: prepared.parent_metadata.mode(),
                occupied_by_observation_id: Some(prepared.observation.id),
            });
        }
    }
    let expires_at = now.saturating_add(TAKEOVER_PLAN_TTL_MILLIS);
    let plan = TakeoverPlan {
        id: plan_id.clone(),
        identity_basis: if origins.len() == 1 {
            TakeoverIdentityBasis::SingleOrigin
        } else {
            TakeoverIdentityBasis::UserConfirmed
        },
        selected_observation_id: request.selected_observation_id,
        bundle_id,
        member_id,
        content_id,
        bundle_display_name: skill_name.clone(),
        skill_name,
        skill_description,
        source_display_name: None,
        managed_directory: path_text(&managed_directory)?,
        content_directory: path_text(&content_directory)?,
        expected_target: expected_target_text,
        origins,
        targets,
        warnings: selected_warnings,
        created_at: now,
        expires_at,
    };
    let contract = TakeoverPlanContract {
        plan: plan.clone(),
        origin_snapshots,
        target_snapshots,
    };
    persist_and_reopen_contract(storage, &contract)?;
    Ok(plan)
}

fn validate_plan_request(request: &TakeoverPlanRequest) -> Result<(), TakeoverError> {
    let observation_ids = request.observation_ids.iter().collect::<BTreeSet<_>>();
    let preserved_ids = request
        .preserved_observation_ids
        .iter()
        .collect::<BTreeSet<_>>();
    if request.observation_ids.is_empty()
        || observation_ids.len() != request.observation_ids.len()
        || !observation_ids.contains(&request.selected_observation_id)
        || preserved_ids.len() != request.preserved_observation_ids.len()
        || !preserved_ids.is_subset(&observation_ids)
        || !request.shared_targets.is_empty()
    {
        return Err(TakeoverError::InvalidRequest(
            "接管计划中的副本、内容选择或保留位置无效".to_owned(),
        ));
    }
    Ok(())
}

fn read_observations(
    storage: &Storage,
    observation_ids: &[String],
) -> Result<Vec<InventoryItem>, TakeoverError> {
    let Some(UiOutcome::Inventory { entries, .. }) = storage.read_initial_scan()? else {
        return Err(TakeoverError::InvalidRequest(
            "完成首次扫描后才能生成 Takeover Plan".to_owned(),
        ));
    };
    observation_ids
        .iter()
        .map(|observation_id| {
            entries
                .iter()
                .find(|entry| entry.id == *observation_id)
                .cloned()
                .ok_or_else(|| TakeoverError::InvalidRequest("所选扫描观察已经不存在".to_owned()))
        })
        .collect()
}

fn validate_observation(observation: &InventoryItem) -> Result<(), TakeoverError> {
    if observation.management_kind != ManagementKind::TakeoverCandidate
        || observation.metadata_status != SkillMetadataStatus::Valid
        || observation.stale
        || observation.declared_name.as_deref() != Some(observation.skill_name.as_str())
        || observation.bundle_id.is_some()
        || observation.member_id.is_some()
    {
        return Err(TakeoverError::InvalidRequest(
            "所选内容不是当前可执行的 Takeover Candidate".to_owned(),
        ));
    }
    Ok(())
}

fn persist_and_reopen_contract(
    storage: &mut Storage,
    contract: &TakeoverPlanContract,
) -> Result<(), TakeoverError> {
    let payload =
        serde_json::to_string(contract).map_err(|_| TakeoverError::InvalidPlanContract)?;
    let seal = sha256_hex(payload.as_bytes());
    storage.save_takeover_plan(&StoredTakeoverPlanRow {
        id: contract.plan.id.clone(),
        payload_json: payload,
        payload_sha256: seal,
        status: "pending".to_owned(),
        created_at: contract.plan.created_at,
        expires_at: contract.plan.expires_at,
    })?;
    let stored = storage.read_takeover_plan(&contract.plan.id)?;
    if stored.status != "pending"
        || stored.created_at != contract.plan.created_at
        || stored.expires_at != contract.plan.expires_at
        || sha256_hex(stored.payload_json.as_bytes()) != stored.payload_sha256
        || serde_json::from_str::<TakeoverPlanContract>(&stored.payload_json)
            .map_err(|_| TakeoverError::InvalidPlanContract)?
            != *contract
    {
        return Err(TakeoverError::InvalidPlanContract);
    }
    Ok(())
}

fn metadata(path: &Path, action: &'static str) -> Result<fs::Metadata, TakeoverError> {
    fs::symlink_metadata(path).map_err(|source| TakeoverError::Io {
        action,
        path: path.display().to_string(),
        source,
    })
}

fn ensure_absent(path: &Path) -> Result<(), TakeoverError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(TakeoverError::Io {
            action: "检查受管 Bundle 目标",
            path: path.display().to_string(),
            source,
        }),
        Ok(_) => Err(TakeoverError::InvalidRequest(
            "计划生成的 Bundle 目标已经存在".to_owned(),
        )),
    }
}

fn path_text(path: &Path) -> Result<String, TakeoverError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| TakeoverError::NonUnicodePath(path.display().to_string()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
