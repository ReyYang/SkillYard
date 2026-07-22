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
    content::{ContentValidationError, validate_single_skill_folder},
    domain::{
        InventoryItem, ManagementKind, MountScope, SkillMetadataStatus, TakeoverIdentityBasis,
        TakeoverOriginDisposition, TakeoverPlan, TakeoverPlanOrigin, TakeoverPlanRequest,
        TakeoverPlanTarget, UiOutcome,
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

/// Plan 构建只读取已保存 Inventory 与真实文件系统，并把完整合同写入 SQLite。
pub(crate) fn create_takeover_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    request: TakeoverPlanRequest,
    now: i64,
) -> Result<TakeoverPlan, TakeoverError> {
    validate_single_origin_request(&request)?;
    let observation = read_selected_observation(storage, &request.observation_ids[0])?;
    validate_observation(&observation)?;
    let root_key = observation
        .root_key
        .ok_or_else(|| TakeoverError::InvalidRequest("扫描观察缺少受支持路径信息".to_owned()))?;
    let app = paths
        .supported_apps()
        .into_iter()
        .find(|app| app.root_key == root_key)
        .ok_or_else(|| {
            TakeoverError::InvalidRequest("第一个切片只接受应用专属 global Skill".to_owned())
        })?;
    let original_root = PathBuf::from(&observation.skill_root);
    let validated = validate_single_skill_folder(&original_root)?;
    if validated.name != observation.skill_name {
        return Err(TakeoverError::InvalidRequest(
            "扫描结果与当前 Skill 内容不一致，请刷新本机后重试".to_owned(),
        ));
    }
    let expected_original = app.global_root.join(&validated.name);
    if expected_original != original_root {
        return Err(TakeoverError::InvalidRequest(
            "Skill 不位于受支持应用的固定 global 路径".to_owned(),
        ));
    }
    let preserve_mount = request.preserved_observation_ids.contains(&observation.id);
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
        .join(&validated.name);
    let expected_target_text = path_text(&expected_target)?;
    let original_path = path_text(&original_root)?;
    let parent = original_root
        .parent()
        .ok_or_else(|| TakeoverError::InvalidRequest("Skill 根目录缺少父目录".to_owned()))?;
    let root_metadata = metadata(&original_root, "读取原 Skill 身份")?;
    let parent_metadata = metadata(parent, "读取原 Skill 父目录身份")?;
    let origin_snapshot = TakeoverOriginSnapshot {
        observation_id: observation.id.clone(),
        root_device: root_metadata.dev(),
        root_inode: root_metadata.ino(),
        root_mode: root_metadata.mode(),
        parent_device: parent_metadata.dev(),
        parent_inode: parent_metadata.ino(),
        parent_mode: parent_metadata.mode(),
    };
    let origin = TakeoverPlanOrigin {
        observation_id: observation.id.clone(),
        original_path: original_path.clone(),
        app_id: Some(app.id),
        scope: Some(MountScope::Global),
        project_id: None,
        project_display_name: None,
        content_fingerprint: validated.fingerprint.clone(),
        warnings: validated.warnings.clone(),
        final_disposition: if preserve_mount {
            TakeoverOriginDisposition::Mount
        } else {
            TakeoverOriginDisposition::Remove
        },
    };
    let (targets, target_snapshots) = if preserve_mount {
        (
            vec![TakeoverPlanTarget {
                mount_id: Uuid::new_v4().to_string(),
                app_id: app.id,
                scope: MountScope::Global,
                project_id: None,
                project_display_name: None,
                target_path: original_path.clone(),
                expected_target: expected_target_text.clone(),
            }],
            vec![TakeoverTargetSnapshot {
                target_path: original_path,
                parent_device: parent_metadata.dev(),
                parent_inode: parent_metadata.ino(),
                parent_mode: parent_metadata.mode(),
                occupied_by_observation_id: Some(observation.id.clone()),
            }],
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let expires_at = now.saturating_add(TAKEOVER_PLAN_TTL_MILLIS);
    let plan = TakeoverPlan {
        id: plan_id.clone(),
        identity_basis: TakeoverIdentityBasis::SingleOrigin,
        selected_observation_id: observation.id,
        bundle_id,
        member_id,
        content_id,
        bundle_display_name: validated.name.clone(),
        skill_name: validated.name,
        skill_description: validated.description,
        source_display_name: None,
        managed_directory: path_text(&managed_directory)?,
        content_directory: path_text(&content_directory)?,
        expected_target: expected_target_text,
        origins: vec![origin],
        targets,
        warnings: validated.warnings,
        created_at: now,
        expires_at,
    };
    let contract = TakeoverPlanContract {
        plan: plan.clone(),
        origin_snapshots: vec![origin_snapshot],
        target_snapshots,
    };
    persist_and_reopen_contract(storage, &contract)?;
    Ok(plan)
}

fn validate_single_origin_request(request: &TakeoverPlanRequest) -> Result<(), TakeoverError> {
    let observation_ids = request.observation_ids.iter().collect::<BTreeSet<_>>();
    let preserved_ids = request
        .preserved_observation_ids
        .iter()
        .collect::<BTreeSet<_>>();
    if request.observation_ids.len() != 1
        || observation_ids.len() != request.observation_ids.len()
        || request.selected_observation_id != request.observation_ids[0]
        || preserved_ids.len() != request.preserved_observation_ids.len()
        || !preserved_ids.is_subset(&observation_ids)
        || !request.shared_targets.is_empty()
    {
        return Err(TakeoverError::InvalidRequest(
            "第一个纵向切片只接受一个应用专属 Skill 副本".to_owned(),
        ));
    }
    Ok(())
}

fn read_selected_observation(
    storage: &Storage,
    observation_id: &str,
) -> Result<InventoryItem, TakeoverError> {
    let Some(UiOutcome::Inventory { entries, .. }) = storage.read_initial_scan()? else {
        return Err(TakeoverError::InvalidRequest(
            "完成首次扫描后才能生成 Takeover Plan".to_owned(),
        ));
    };
    entries
        .into_iter()
        .find(|entry| entry.id == observation_id)
        .ok_or_else(|| TakeoverError::InvalidRequest("所选扫描观察已经不存在".to_owned()))
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
