use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    content::{
        BundleCopyBudget, ContentValidationError, ValidatedSingleSkill,
        copy_single_skill_tree_into_open_directory, validate_single_skill_folder,
        validate_single_skill_folder_as,
    },
    domain::{
        InventoryItem, ManagementKind, MountScope, SkillMetadataStatus, SupportedAppId,
        TakeoverIdentityBasis, TakeoverOriginDisposition, TakeoverPlan, TakeoverPlanOrigin,
        TakeoverPlanRequest, TakeoverPlanTarget, UiOutcome,
    },
    lifecycle::{
        LifecycleError, LifecycleFailpoint, LifecycleLock, acquire_lifecycle_lock,
        ensure_entry_absent_at, ensure_open_directory_matches_managed_path, entry_metadata_at,
        mkdir_at, open_directory_at, open_expected_directory_at, open_managed_directory_from_root,
        read_entry_names_from_handle, read_link_at, remove_empty_directory_at,
        remove_owned_tree_at, rename_at_no_replace, symlink_at, unlink_at, write_atomic_at,
        write_notice_from_storage,
    },
    mount_lifecycle::{
        MountLifecycleError, OpenMountParent, ParentLookup, TargetKind, open_mount_parent,
        open_relative_parent, recheck_open_parent, snapshot_at,
    },
    paths::ApplicationPaths,
    storage::{Storage, StorageError, StoredTakeoverPlanRow},
};

const TAKEOVER_PLAN_TTL_MILLIS: i64 = 15 * 60 * 1_000;
const MAX_TAKEOVER_JOURNAL_BYTES: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum TakeoverError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Content(#[from] ContentValidationError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error(transparent)]
    Mount(#[from] MountLifecycleError),
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
    #[error("Takeover Plan 已过期，请重新生成")]
    PlanExpired,
    #[error("当前接管切片只允许确认一个本地副本")]
    UnsupportedConfirmation,
    #[error("Takeover Journal 无法解析：{0}")]
    InvalidJournal(#[from] serde_json::Error),
    #[error("Takeover Journal 超过安全大小限制")]
    JournalTooLarge,
    #[error("接管事务恢复需要人工处理：{0}")]
    RecoveryBlocked(String),
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
    target_observation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TakeoverTargetSnapshot {
    target_path: String,
    target_observation: String,
    occupied_by_observation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TakeoverJournalPhase {
    JournalReady,
    CandidateReady,
    CurrentActivated,
    OriginsApplied,
    StateCommitted,
}

impl TakeoverJournalPhase {
    fn as_storage_str(self) -> &'static str {
        match self {
            Self::JournalReady => "journal_ready",
            Self::CandidateReady => "candidate_ready",
            Self::CurrentActivated => "current_activated",
            Self::OriginsApplied => "origins_applied",
            Self::StateCommitted => "state_committed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TakeoverJournalOrigin {
    observation_id: String,
    original_path: String,
    recovery_name: String,
    mount_staging_name: String,
    expected_fingerprint: String,
    expected_target: Option<String>,
    original_observation: String,
    recovery_observation: Option<String>,
    mount_observation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TakeoverJournal {
    transaction_id: String,
    plan_id: String,
    phase: TakeoverJournalPhase,
    bundle_id: String,
    content_id: String,
    selected_observation_id: String,
    origins: Vec<TakeoverJournalOrigin>,
}

struct PreparedOrigin {
    observation: InventoryItem,
    app_id: SupportedAppId,
    original_root: PathBuf,
    validated: ValidatedSingleSkill,
    root_device: u64,
    root_inode: u64,
    root_mode: u32,
    target_observation: String,
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
        let ParentLookup::Open(parent) =
            open_mount_parent(paths, app.id, MountScope::Global, None, false)?
        else {
            return Err(TakeoverError::InvalidRequest(
                "扫描到的 Host Skill 父目录已经不存在".to_owned(),
            ));
        };
        ensure_origin_parent_matches(&parent, &original_root, &validated.name)?;
        let leaf = OsStr::new(&validated.name);
        let root_metadata = entry_metadata_at(parent.directory(), leaf)
            .map_err(|source| takeover_io("检查原 Skill 身份", &original_root, source))?
            .ok_or_else(|| {
                TakeoverError::InvalidRequest("扫描到的原 Skill 已经不存在".to_owned())
            })?;
        let target_snapshot = snapshot_at(parent.directory(), leaf, "")?;
        if target_snapshot.kind() != TargetKind::Other {
            return Err(TakeoverError::InvalidRequest(
                "接管输入必须是普通 Skill 目录".to_owned(),
            ));
        }
        recheck_open_parent(&parent)?;
        prepared_origins.push(PreparedOrigin {
            observation,
            app_id: app.id,
            original_root,
            validated,
            root_device: root_metadata.st_dev as u64,
            root_inode: root_metadata.st_ino,
            root_mode: root_metadata.st_mode as u32,
            target_observation: target_snapshot.observation().to_owned(),
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
            root_device: prepared.root_device,
            root_inode: prepared.root_inode,
            root_mode: prepared.root_mode,
            target_observation: prepared.target_observation.clone(),
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
                target_observation: prepared.target_observation,
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

/// 确认阶段只接受 Plan ID；路径、内容选择和 Mount 集合全部来自已封存合同。
pub(crate) fn confirm_takeover_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    let stored = storage.read_takeover_plan(plan_id)?;
    let contract = decode_plan_contract(&stored)?;
    validate_confirmation_contract(paths, storage, &stored, &contract, now)?;

    let transaction_id = Uuid::new_v4().to_string();
    let journal_relative = format!("journals/takeover-{transaction_id}.json");
    let mut journal = build_journal(&transaction_id, &contract)?;
    ensure_journal_fits(&journal)?;
    let consumed =
        storage.begin_takeover_transaction(plan_id, &transaction_id, &journal_relative, now)?;
    if consumed.payload_json != stored.payload_json
        || consumed.payload_sha256 != stored.payload_sha256
        || consumed.status != "consumed"
    {
        storage.abort_takeover_transaction(
            &transaction_id,
            Some("Takeover Plan 在确认时发生变化"),
            now,
        )?;
        storage.forget_terminal_takeover_transaction(&transaction_id)?;
        return Err(TakeoverError::InvalidPlanContract);
    }

    if let Err(error) = execute_before_domain_commit(
        paths,
        &lifecycle_lock,
        storage,
        &contract,
        &mut journal,
        now,
        failpoint,
    ) {
        return handle_precommit_error(
            paths,
            lifecycle_lock.root(),
            storage,
            &contract,
            &mut journal,
            now,
            error,
        );
    }
    lifecycle_lock.recheck(paths)?;
    if let Err(error) = storage.finalize_takeover(&transaction_id, &contract.plan, now) {
        return handle_precommit_error(
            paths,
            lifecycle_lock.root(),
            storage,
            &contract,
            &mut journal,
            now,
            error.into(),
        );
    }

    journal.phase = TakeoverJournalPhase::StateCommitted;
    write_journal(paths, lifecycle_lock.root(), &journal)?;
    write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
    cleanup_committed(paths, lifecycle_lock.root(), storage, &contract, &journal)?;
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

fn decode_plan_contract(
    stored: &StoredTakeoverPlanRow,
) -> Result<TakeoverPlanContract, TakeoverError> {
    if sha256_hex(stored.payload_json.as_bytes()) != stored.payload_sha256 {
        return Err(TakeoverError::InvalidPlanContract);
    }
    let contract = serde_json::from_str::<TakeoverPlanContract>(&stored.payload_json)
        .map_err(|_| TakeoverError::InvalidPlanContract)?;
    if contract.plan.id != stored.id
        || contract.plan.created_at != stored.created_at
        || contract.plan.expires_at != stored.expires_at
    {
        return Err(TakeoverError::InvalidPlanContract);
    }
    Ok(contract)
}

fn validate_confirmation_contract(
    paths: &ApplicationPaths,
    storage: &Storage,
    stored: &StoredTakeoverPlanRow,
    contract: &TakeoverPlanContract,
    now: i64,
) -> Result<(), TakeoverError> {
    if stored.status != "pending" {
        return Err(TakeoverError::InvalidRequest(
            "Takeover Plan 已经使用".to_owned(),
        ));
    }
    if stored.expires_at <= now {
        return Err(TakeoverError::PlanExpired);
    }
    let plan = &contract.plan;
    if plan.origins.len() != 1
        || contract.origin_snapshots.len() != 1
        || plan.selected_observation_id != plan.origins[0].observation_id
        || plan.targets.len() > 1
        || contract.target_snapshots.len() != plan.targets.len()
    {
        return Err(TakeoverError::UnsupportedConfirmation);
    }
    for id in [&plan.id, &plan.bundle_id, &plan.member_id, &plan.content_id] {
        Uuid::parse_str(id).map_err(|_| TakeoverError::InvalidPlanContract)?;
    }
    let expected_managed = paths.bundle_directory(&plan.bundle_id);
    let expected_content = expected_managed.join("contents").join(&plan.content_id);
    let expected_target = expected_managed
        .join("current/members")
        .join(&plan.skill_name);
    if Path::new(&plan.managed_directory) != expected_managed
        || Path::new(&plan.content_directory) != expected_content
        || Path::new(&plan.expected_target) != expected_target
    {
        return Err(TakeoverError::InvalidPlanContract);
    }
    ensure_absent(&expected_managed)?;
    let origin = &plan.origins[0];
    let snapshot = &contract.origin_snapshots[0];
    if snapshot.observation_id != origin.observation_id {
        return Err(TakeoverError::InvalidPlanContract);
    }
    let validated = validate_live_origin(paths, storage, origin, snapshot)?;
    if validated.name != plan.skill_name {
        return Err(TakeoverError::InvalidPlanContract);
    }
    let matching_target = plan
        .targets
        .iter()
        .find(|target| target.target_path == origin.original_path);
    match (origin.final_disposition, matching_target) {
        (TakeoverOriginDisposition::Mount, Some(target))
            if target.expected_target == plan.expected_target => {}
        (TakeoverOriginDisposition::Remove, None) => {}
        _ => return Err(TakeoverError::InvalidPlanContract),
    }
    if let Some(target_snapshot) = contract.target_snapshots.first()
        && (target_snapshot.target_path != origin.original_path
            || target_snapshot.occupied_by_observation_id.as_deref()
                != Some(origin.observation_id.as_str())
            || target_snapshot.target_observation != snapshot.target_observation)
    {
        return Err(TakeoverError::InvalidPlanContract);
    }
    Ok(())
}

fn build_journal(
    transaction_id: &str,
    contract: &TakeoverPlanContract,
) -> Result<TakeoverJournal, TakeoverError> {
    let plan = &contract.plan;
    let origins = plan
        .origins
        .iter()
        .enumerate()
        .map(|(index, origin)| {
            let snapshot = contract
                .origin_snapshots
                .get(index)
                .ok_or(TakeoverError::InvalidPlanContract)?;
            let expected_target = plan
                .targets
                .iter()
                .find(|target| target.target_path == origin.original_path)
                .map(|target| target.expected_target.clone());
            Ok(TakeoverJournalOrigin {
                observation_id: origin.observation_id.clone(),
                original_path: origin.original_path.clone(),
                recovery_name: format!(".skillyard-takeover-{transaction_id}-{index}"),
                mount_staging_name: format!(".skillyard-takeover-mount-{transaction_id}-{index}"),
                expected_fingerprint: origin.content_fingerprint.clone(),
                expected_target,
                original_observation: snapshot.target_observation.clone(),
                recovery_observation: None,
                mount_observation: None,
            })
        })
        .collect::<Result<Vec<_>, TakeoverError>>()?;
    Ok(TakeoverJournal {
        transaction_id: transaction_id.to_owned(),
        plan_id: plan.id.clone(),
        phase: TakeoverJournalPhase::JournalReady,
        bundle_id: plan.bundle_id.clone(),
        content_id: plan.content_id.clone(),
        selected_observation_id: plan.selected_observation_id.clone(),
        origins,
    })
}

fn execute_before_domain_commit(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    contract: &TakeoverPlanContract,
    journal: &mut TakeoverJournal,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverError> {
    let managed_root = lifecycle_lock.root();
    write_journal(paths, managed_root, journal)?;
    storage.update_takeover_transaction_phase(
        &journal.transaction_id,
        journal.phase.as_storage_str(),
        now,
    )?;

    let staging_root =
        open_managed_directory_from_root(paths, managed_root, &paths.staging_root())?;
    let transaction_name = OsStr::new(&journal.transaction_id);
    mkdir_at(&staging_root, transaction_name, 0o700)
        .map_err(|source| takeover_io("创建接管临时目录", &paths.staging_root(), source))?;
    let staging = open_directory_at(&staging_root, transaction_name)
        .map_err(|source| takeover_io("打开接管临时目录", &paths.staging_root(), source))?;
    mkdir_at(&staging, OsStr::new("candidate"), 0o700)
        .map_err(|source| takeover_io("创建接管候选目录", &paths.staging_root(), source))?;
    let candidate = open_directory_at(&staging, OsStr::new("candidate"))
        .map_err(|source| takeover_io("打开接管候选目录", &paths.staging_root(), source))?;
    mkdir_at(&candidate, OsStr::new("members"), 0o700)
        .map_err(|source| takeover_io("创建接管成员目录", &paths.staging_root(), source))?;
    let members = open_directory_at(&candidate, OsStr::new("members"))
        .map_err(|source| takeover_io("打开接管成员目录", &paths.staging_root(), source))?;
    let selected = contract
        .plan
        .origins
        .iter()
        .find(|origin| origin.observation_id == contract.plan.selected_observation_id)
        .ok_or(TakeoverError::InvalidPlanContract)?;
    let members_path = paths
        .staging_root()
        .join(&journal.transaction_id)
        .join("candidate/members");
    copy_single_skill_tree_into_open_directory(
        Path::new(&selected.original_path),
        &members,
        &members_path,
        OsStr::new(&contract.plan.skill_name),
        &contract.plan.skill_name,
        &selected.content_fingerprint,
        &mut BundleCopyBudget::production(),
    )?;
    sync(&members, "同步接管成员目录", &members_path)?;
    sync(&candidate, "同步接管候选目录", &members_path)?;
    sync(&staging, "同步接管临时目录", &paths.staging_root())?;
    sync(&staging_root, "同步接管临时区", &paths.staging_root())?;
    lifecycle_lock.recheck(paths)?;

    let bundles_root =
        open_managed_directory_from_root(paths, managed_root, &paths.bundles_root())?;
    mkdir_at(&bundles_root, OsStr::new(&journal.bundle_id), 0o700)
        .map_err(|source| takeover_io("创建接管 Bundle", &paths.bundles_root(), source))?;
    let bundle = open_directory_at(&bundles_root, OsStr::new(&journal.bundle_id))
        .map_err(|source| takeover_io("打开接管 Bundle", &paths.bundles_root(), source))?;
    mkdir_at(&bundle, OsStr::new("contents"), 0o700)
        .map_err(|source| takeover_io("创建接管内容目录", &paths.bundles_root(), source))?;
    let contents = open_directory_at(&bundle, OsStr::new("contents"))
        .map_err(|source| takeover_io("打开接管内容目录", &paths.bundles_root(), source))?;
    lifecycle_lock.recheck(paths)?;
    rename_at_no_replace(
        &staging,
        OsStr::new("candidate"),
        &contents,
        OsStr::new(&journal.content_id),
    )
    .map_err(|source| takeover_io("发布接管候选内容", &paths.bundles_root(), source))?;
    sync(&staging, "同步候选源目录", &paths.staging_root())?;
    sync(&contents, "同步接管内容目录", &paths.bundles_root())?;
    sync(&bundle, "同步接管 Bundle", &paths.bundles_root())?;
    sync(&bundles_root, "同步 Bundle 根目录", &paths.bundles_root())?;
    journal.phase = TakeoverJournalPhase::CandidateReady;
    persist_phase(paths, managed_root, storage, journal, now)?;

    let temporary_current = OsString::from(format!(".current-{}", journal.transaction_id));
    lifecycle_lock.recheck(paths)?;
    ensure_entry_absent_at(&bundle, &temporary_current)
        .map_err(|source| takeover_io("检查临时 current", &paths.bundles_root(), source))?;
    symlink_at(
        &Path::new("contents").join(&journal.content_id),
        &bundle,
        &temporary_current,
    )
    .map_err(|source| takeover_io("创建临时 current", &paths.bundles_root(), source))?;
    sync(&bundle, "同步临时 current", &paths.bundles_root())?;
    rename_at_no_replace(&bundle, &temporary_current, &bundle, OsStr::new("current"))
        .map_err(|source| takeover_io("切换 Bundle current", &paths.bundles_root(), source))?;
    sync(&bundle, "同步 Bundle current", &paths.bundles_root())?;
    journal.phase = TakeoverJournalPhase::CurrentActivated;
    persist_phase(paths, managed_root, storage, journal, now)?;

    for (index, origin) in contract.plan.origins.iter().enumerate() {
        let snapshot = &contract.origin_snapshots[index];
        validate_live_origin(paths, storage, origin, snapshot)?;
        let parent = open_origin_parent(paths, storage, origin, &contract.plan.skill_name)?;
        let leaf = origin_leaf(Path::new(&origin.original_path), &contract.plan.skill_name)?;
        lifecycle_lock.recheck(paths)?;
        let original = snapshot_at(parent.directory(), &leaf, "")?;
        if original.observation() != journal.origins[index].original_observation {
            return Err(TakeoverError::InvalidRequest(
                "Plan 生成后原 Skill 身份已经变化".to_owned(),
            ));
        }
        let recovery_name = OsString::from(&journal.origins[index].recovery_name);
        if snapshot_at(parent.directory(), &recovery_name, "")?.kind() != TargetKind::Absent {
            return Err(TakeoverError::RecoveryBlocked(format!(
                "接管恢复位置已被占用：{}",
                parent.path().join(&recovery_name).display()
            )));
        }
        rename_at_no_replace(
            parent.directory(),
            &leaf,
            parent.directory(),
            &recovery_name,
        )
        .map_err(|source| takeover_io("隔离原 Skill", parent.path(), source))?;
        if failpoint == LifecycleFailpoint::AfterTakeoverOriginMovedBeforeProgress {
            return Err(LifecycleError::SimulatedInterruption(
                "Takeover 原目录已移动但进度尚未记录",
            )
            .into());
        }
        sync(parent.directory(), "同步原 Skill 父目录", parent.path())?;
        let recovery = snapshot_at(parent.directory(), &recovery_name, "")?;
        if recovery.observation() != journal.origins[index].original_observation {
            return Err(TakeoverError::RecoveryBlocked(
                "隔离后的原 Skill 不再是 Plan 中的同一目录".to_owned(),
            ));
        }
        journal.origins[index].recovery_observation = Some(recovery.observation().to_owned());
        write_journal(paths, managed_root, journal)?;
        if let Some(expected_target) = journal.origins[index].expected_target.clone() {
            let staging_name = OsString::from(&journal.origins[index].mount_staging_name);
            if snapshot_at(parent.directory(), &staging_name, &expected_target)?.kind()
                != TargetKind::Absent
            {
                return Err(TakeoverError::RecoveryBlocked(
                    "接管 Mount 暂存位置已被占用".to_owned(),
                ));
            }
            symlink_at(
                Path::new(&expected_target),
                parent.directory(),
                &staging_name,
            )
            .map_err(|source| takeover_io("创建接管 Mount 暂存链接", parent.path(), source))?;
            let staged = snapshot_at(parent.directory(), &staging_name, &expected_target)?;
            if staged.kind() != TargetKind::ExpectedLink {
                return Err(TakeoverError::RecoveryBlocked(
                    "新 Mount 暂存链接没有指向 Plan 固定的受管内容".to_owned(),
                ));
            }
            // 先把精确 inode observation 写入内存，再进入任何可能失败的同步或测试窗口。
            journal.origins[index].mount_observation = Some(staged.observation().to_owned());
            if failpoint == LifecycleFailpoint::AfterTakeoverMountStagedBeforeProgress {
                return Err(LifecycleError::SimulatedInterruption(
                    "Takeover Mount 已暂存但进度尚未记录",
                )
                .into());
            }
            sync(parent.directory(), "同步接管 Mount 暂存链接", parent.path())?;
            write_journal(paths, managed_root, journal)?;
            rename_at_no_replace(parent.directory(), &staging_name, parent.directory(), &leaf)
                .map_err(|source| takeover_io("发布接管 Mount", parent.path(), source))?;
            sync(parent.directory(), "同步接管 Mount", parent.path())?;
            let published = snapshot_at(parent.directory(), &leaf, &expected_target)?;
            if published.observation() != staged.observation() {
                return Err(TakeoverError::RecoveryBlocked(
                    "发布后的 Mount 不再是本事务创建的链接".to_owned(),
                ));
            }
        } else if snapshot_at(parent.directory(), &leaf, "")?.kind() != TargetKind::Absent {
            return Err(TakeoverError::RecoveryBlocked(
                "用户排除的 Host 位置仍被内容占用".to_owned(),
            ));
        }
        recheck_open_parent(&parent)?;
        lifecycle_lock.recheck(paths)?;
        write_journal(paths, managed_root, journal)?;
    }
    lifecycle_lock.recheck(paths)?;
    journal.phase = TakeoverJournalPhase::OriginsApplied;
    persist_phase(paths, managed_root, storage, journal, now)?;
    let activated = validate_single_skill_folder(Path::new(&contract.plan.expected_target))?;
    if activated.fingerprint != selected.content_fingerprint {
        return Err(TakeoverError::RecoveryBlocked(
            "Bundle current 内容与用户选择不一致".to_owned(),
        ));
    }
    Ok(())
}

fn handle_precommit_error(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &mut Storage,
    contract: &TakeoverPlanContract,
    journal: &mut TakeoverJournal,
    now: i64,
    original: TakeoverError,
) -> Result<(), TakeoverError> {
    if let Err(rollback) = rollback_before_commit(paths, managed_root, storage, contract, journal) {
        let message = format!("原错误：{original}；恢复错误：{rollback}");
        storage.block_takeover_transaction(&journal.transaction_id, &message, now)?;
        return Err(TakeoverError::RecoveryBlocked(message));
    }
    storage.abort_takeover_transaction(
        &journal.transaction_id,
        Some(&original.to_string()),
        now,
    )?;
    storage.forget_terminal_takeover_transaction(&journal.transaction_id)?;
    Err(original)
}

fn rollback_before_commit(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &Storage,
    contract: &TakeoverPlanContract,
    journal: &mut TakeoverJournal,
) -> Result<(), TakeoverError> {
    for (index, origin) in contract.plan.origins.iter().enumerate().rev() {
        let progress = &mut journal.origins[index];
        let parent = open_origin_parent(paths, storage, origin, &contract.plan.skill_name)?;
        let leaf = origin_leaf(Path::new(&origin.original_path), &contract.plan.skill_name)?;
        if progress.mount_observation.is_none()
            && let Some(expected) = progress.expected_target.as_deref()
        {
            let staging_name = OsString::from(&progress.mount_staging_name);
            let staged = snapshot_at(parent.directory(), &staging_name, expected)?;
            if staged.kind() != TargetKind::Absent {
                return Err(TakeoverError::RecoveryBlocked(
                    "Mount 暂存链接存在，但缺少可证明归属的精确 observation".to_owned(),
                ));
            }
        }
        if let Some(applied_observation) = progress.mount_observation.clone() {
            let expected = progress
                .expected_target
                .as_deref()
                .ok_or(TakeoverError::InvalidPlanContract)?;
            let staging_name = OsString::from(&progress.mount_staging_name);
            let current = snapshot_at(parent.directory(), &leaf, expected)?;
            let staged = snapshot_at(parent.directory(), &staging_name, expected)?;
            let owned_name = if current.observation() == applied_observation
                && staged.kind() == TargetKind::Absent
            {
                Some(leaf.as_os_str())
            } else if current.kind() == TargetKind::Absent
                && staged.observation() == applied_observation
            {
                Some(staging_name.as_os_str())
            } else if current.kind() == TargetKind::Absent && staged.kind() == TargetKind::Absent {
                None
            } else {
                return Err(TakeoverError::RecoveryBlocked(
                    "待撤销 Mount 已被未知内容替换".to_owned(),
                ));
            };
            if let Some(owned_name) = owned_name {
                if owned_name != staging_name.as_os_str() {
                    rename_at_no_replace(
                        parent.directory(),
                        owned_name,
                        parent.directory(),
                        &staging_name,
                    )
                    .map_err(|source| takeover_io("隔离待撤销 Mount", parent.path(), source))?;
                }
                let moved = snapshot_at(parent.directory(), &staging_name, expected)?;
                if moved.observation() != applied_observation {
                    return Err(TakeoverError::RecoveryBlocked(
                        "待撤销 Mount 在隔离时发生变化".to_owned(),
                    ));
                }
                unlink_at(parent.directory(), &staging_name, false)
                    .map_err(|source| takeover_io("撤销接管 Mount", parent.path(), source))?;
                sync(parent.directory(), "同步 Mount 撤销", parent.path())?;
            }
            progress.mount_observation = None;
        }
        if progress.recovery_observation.is_none() {
            let recovery_name = OsString::from(&progress.recovery_name);
            let current = snapshot_at(parent.directory(), &leaf, "")?;
            let recovery = snapshot_at(parent.directory(), &recovery_name, "")?;
            if current.kind() == TargetKind::Absent
                && recovery.observation() == progress.original_observation
            {
                // 原目录已经完成 rename，但进度写入失败；原始精确快照足以确认其归属。
                progress.recovery_observation = Some(recovery.observation().to_owned());
            } else if current.observation() == progress.original_observation
                && recovery.kind() == TargetKind::Absent
            {
                // rename 尚未发生，不需要恢复。
            } else {
                return Err(TakeoverError::RecoveryBlocked(
                    "无法确认原 Skill 在中断窗口中的唯一位置".to_owned(),
                ));
            }
        }
        if let Some(recovery_observation) = progress.recovery_observation.clone() {
            let recovery_name = OsString::from(&progress.recovery_name);
            let recovery = snapshot_at(parent.directory(), &recovery_name, "")?;
            if recovery.observation() != recovery_observation
                || snapshot_at(parent.directory(), &leaf, "")?.kind() != TargetKind::Absent
            {
                return Err(TakeoverError::RecoveryBlocked(
                    "原 Skill 恢复位置已被未知内容替换".to_owned(),
                ));
            }
            rename_at_no_replace(
                parent.directory(),
                &recovery_name,
                parent.directory(),
                &leaf,
            )
            .map_err(|source| takeover_io("恢复原 Skill", parent.path(), source))?;
            sync(parent.directory(), "同步原 Skill 恢复", parent.path())?;
            let restored = snapshot_at(parent.directory(), &leaf, "")?;
            if restored.observation() != progress.original_observation {
                return Err(TakeoverError::RecoveryBlocked(
                    "恢复后的原 Skill 不再是 Plan 中的目录".to_owned(),
                ));
            }
            progress.recovery_observation = None;
        }
        validate_live_origin(paths, storage, origin, &contract.origin_snapshots[index])?;
        recheck_open_parent(&parent)?;
        write_journal(paths, managed_root, journal)?;
    }
    cleanup_uncommitted_takeover_content(paths, managed_root, contract, journal)?;
    remove_journal_if_present(paths, managed_root, &journal.transaction_id)
}

fn cleanup_committed(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &mut Storage,
    contract: &TakeoverPlanContract,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverError> {
    for (index, origin) in contract.plan.origins.iter().enumerate() {
        let progress = &journal.origins[index];
        let original_path = Path::new(&origin.original_path);
        let parent = open_origin_parent(paths, storage, origin, &contract.plan.skill_name)?;
        let leaf = origin_leaf(original_path, &contract.plan.skill_name)?;
        match &progress.expected_target {
            Some(expected) => {
                let Some(applied_observation) = progress.mount_observation.as_deref() else {
                    return Err(TakeoverError::RecoveryBlocked(
                        "接管 Mount 缺少本事务的精确归属记录".to_owned(),
                    ));
                };
                let current = snapshot_at(parent.directory(), &leaf, expected)?;
                let staged = snapshot_at(
                    parent.directory(),
                    OsStr::new(&progress.mount_staging_name),
                    expected,
                )?;
                if current.observation() != applied_observation
                    || staged.kind() != TargetKind::Absent
                {
                    return Err(TakeoverError::RecoveryBlocked(
                        "最终 Mount 已被未知内容替换".to_owned(),
                    ));
                }
            }
            None if progress.mount_observation.is_none()
                && snapshot_at(parent.directory(), &leaf, "")?.kind() == TargetKind::Absent => {}
            _ => {
                return Err(TakeoverError::RecoveryBlocked(
                    "最终 Host 位置与接管计划不一致".to_owned(),
                ));
            }
        }
        let recovery_name = OsString::from(&progress.recovery_name);
        let recovery_snapshot = snapshot_at(parent.directory(), &recovery_name, "")?;
        if progress.recovery_observation.as_deref() != Some(recovery_snapshot.observation()) {
            return Err(TakeoverError::RecoveryBlocked(
                "原 Skill 隔离内容已被未知内容替换".to_owned(),
            ));
        }
        let recovery = parent.path().join(&recovery_name);
        let validated = validate_single_skill_folder_as(&recovery, &contract.plan.skill_name)?;
        if validated.fingerprint != progress.expected_fingerprint {
            return Err(TakeoverError::RecoveryBlocked(
                "事务恢复内容与 Plan 不一致".to_owned(),
            ));
        }
        remove_owned_tree_at(parent.directory(), &recovery_name, &recovery)?;
        recheck_open_parent(&parent)?;
    }
    let staging_root =
        open_managed_directory_from_root(paths, managed_root, &paths.staging_root())?;
    remove_empty_directory_at(
        &staging_root,
        OsStr::new(&journal.transaction_id),
        &paths.staging_root().join(&journal.transaction_id),
    )?;
    remove_journal_if_present(paths, managed_root, &journal.transaction_id)?;
    storage.forget_terminal_takeover_transaction(&journal.transaction_id)?;
    Ok(())
}

fn validate_live_origin(
    paths: &ApplicationPaths,
    storage: &Storage,
    origin: &TakeoverPlanOrigin,
    snapshot: &TakeoverOriginSnapshot,
) -> Result<ValidatedSingleSkill, TakeoverError> {
    let path = Path::new(&origin.original_path);
    let skill_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(TakeoverError::InvalidPlanContract)?;
    let parent = open_origin_parent(paths, storage, origin, skill_name)?;
    let leaf = origin_leaf(path, skill_name)?;
    let metadata = entry_metadata_at(parent.directory(), &leaf)
        .map_err(|source| takeover_io("检查原 Skill 身份", path, source))?
        .ok_or_else(|| TakeoverError::InvalidRequest("Plan 中的原 Skill 已经不存在".to_owned()))?;
    if metadata.st_dev as u64 != snapshot.root_device
        || metadata.st_ino != snapshot.root_inode
        || metadata.st_mode as u32 != snapshot.root_mode
    {
        return Err(TakeoverError::InvalidRequest(
            "Plan 生成后原 Skill 状态已经变化".to_owned(),
        ));
    }
    let current = snapshot_at(parent.directory(), &leaf, "")?;
    if current.observation() != snapshot.target_observation {
        return Err(TakeoverError::InvalidRequest(
            "Plan 生成后原 Skill 文件系统身份已经变化".to_owned(),
        ));
    }
    let validated = validate_single_skill_folder(path)?;
    if validated.fingerprint != origin.content_fingerprint {
        return Err(TakeoverError::InvalidRequest(
            "Plan 生成后原 Skill 内容已经变化".to_owned(),
        ));
    }
    recheck_open_parent(&parent)?;
    Ok(validated)
}

fn open_origin_parent(
    paths: &ApplicationPaths,
    storage: &Storage,
    origin: &TakeoverPlanOrigin,
    skill_name: &str,
) -> Result<OpenMountParent, TakeoverError> {
    let lookup = match (origin.app_id, origin.scope) {
        (Some(app_id), Some(scope)) => {
            let project = origin
                .project_id
                .as_deref()
                .map(|project_id| storage.read_project(project_id))
                .transpose()?;
            open_mount_parent(paths, app_id, scope, project.as_ref(), false)?
        }
        (None, None)
            if Path::new(&origin.original_path).parent()
                == Some(&paths.shared_read_only_root()) =>
        {
            let shared_root = paths.shared_read_only_root();
            let relative = shared_root
                .strip_prefix(paths.home())
                .map_err(|_| TakeoverError::InvalidPlanContract)?;
            open_relative_parent(paths.home(), relative, false)?
        }
        _ => return Err(TakeoverError::InvalidPlanContract),
    };
    let ParentLookup::Open(parent) = lookup else {
        return Err(TakeoverError::InvalidRequest(
            "Plan 中的 Host Skill 父目录已经不存在".to_owned(),
        ));
    };
    ensure_origin_parent_matches(&parent, Path::new(&origin.original_path), skill_name)?;
    Ok(parent)
}

fn ensure_origin_parent_matches(
    parent: &OpenMountParent,
    original: &Path,
    skill_name: &str,
) -> Result<(), TakeoverError> {
    if parent.path().join(skill_name) == original {
        Ok(())
    } else {
        Err(TakeoverError::InvalidPlanContract)
    }
}

fn origin_leaf(original: &Path, expected_name: &str) -> Result<OsString, TakeoverError> {
    let leaf = original
        .file_name()
        .ok_or(TakeoverError::InvalidPlanContract)?;
    if leaf != OsStr::new(expected_name) {
        return Err(TakeoverError::InvalidPlanContract);
    }
    Ok(leaf.to_owned())
}

fn persist_phase(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &mut Storage,
    journal: &TakeoverJournal,
    now: i64,
) -> Result<(), TakeoverError> {
    write_journal(paths, managed_root, journal)?;
    storage.update_takeover_transaction_phase(
        &journal.transaction_id,
        journal.phase.as_storage_str(),
        now,
    )?;
    Ok(())
}

fn write_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverError> {
    let bytes = serde_json::to_vec(journal)?;
    if bytes.len() > MAX_TAKEOVER_JOURNAL_BYTES {
        return Err(TakeoverError::JournalTooLarge);
    }
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let name = journal_file_name(&journal.transaction_id);
    write_atomic_at(&journals, &name, &paths.journals_root().join(&name), &bytes)?;
    Ok(())
}

fn ensure_journal_fits(journal: &TakeoverJournal) -> Result<(), TakeoverError> {
    if serde_json::to_vec(journal)?.len() > MAX_TAKEOVER_JOURNAL_BYTES {
        Err(TakeoverError::JournalTooLarge)
    } else {
        Ok(())
    }
}

fn remove_journal_if_present(
    paths: &ApplicationPaths,
    managed_root: &File,
    transaction_id: &str,
) -> Result<(), TakeoverError> {
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let name = journal_file_name(transaction_id);
    if entry_metadata_at(&journals, &name)
        .map_err(|source| takeover_io("检查 Takeover Journal", &paths.journals_root(), source))?
        .is_some()
    {
        unlink_at(&journals, &name, false).map_err(|source| {
            takeover_io("清理 Takeover Journal", &paths.journals_root(), source)
        })?;
        sync(&journals, "同步 Journal 目录", &paths.journals_root())?;
    }
    Ok(())
}

fn journal_file_name(transaction_id: &str) -> OsString {
    OsString::from(format!("takeover-{transaction_id}.json"))
}

fn cleanup_uncommitted_takeover_content(
    paths: &ApplicationPaths,
    managed_root: &File,
    contract: &TakeoverPlanContract,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverError> {
    cleanup_uncommitted_bundle(paths, managed_root, contract, journal)?;
    cleanup_uncommitted_staging(paths, managed_root, contract, journal)
}

fn cleanup_uncommitted_bundle(
    paths: &ApplicationPaths,
    managed_root: &File,
    contract: &TakeoverPlanContract,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverError> {
    let bundles_path = paths.bundles_root();
    let bundles = open_managed_directory_from_root(paths, managed_root, &bundles_path)?;
    let bundle_path = paths.bundle_directory(&journal.bundle_id);
    let bundle = match open_directory_at(&bundles, OsStr::new(&journal.bundle_id)) {
        Ok(bundle) => bundle,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(takeover_io("安全打开待清理 Bundle", &bundle_path, source)),
    };
    ensure_open_directory_matches_managed_path(paths, &bundle, &bundle_path)?;
    let temporary_current = format!(".current-{}", journal.transaction_id);
    ensure_only_entries(
        &read_entry_names_from_handle(&bundle)?,
        &["contents", "current", temporary_current.as_str()],
        "待清理 Bundle 包含未知条目",
    )?;
    let contents_path = bundle_path.join("contents");
    let content_path = contents_path.join(&journal.content_id);
    let content_exists = entry_metadata_at(&bundle, OsStr::new("contents"))
        .map_err(|source| takeover_io("检查待清理 contents", &contents_path, source))?
        .is_some();
    let current_exists = validate_optional_current(
        &bundle,
        OsStr::new("current"),
        &Path::new("contents").join(&journal.content_id),
        &bundle_path.join("current"),
    )?;
    let temporary_exists = validate_optional_current(
        &bundle,
        OsStr::new(&temporary_current),
        &Path::new("contents").join(&journal.content_id),
        &bundle_path.join(&temporary_current),
    )?;
    if (current_exists || temporary_exists) && !content_exists {
        return Err(TakeoverError::RecoveryBlocked(
            "待清理 current 指向的候选内容已经缺失".to_owned(),
        ));
    }
    for name in ["current", temporary_current.as_str()] {
        if entry_metadata_at(&bundle, OsStr::new(name))
            .map_err(|source| takeover_io("重新检查待清理 current", &bundle_path, source))?
            .is_some()
        {
            unlink_at(&bundle, OsStr::new(name), false)
                .map_err(|source| takeover_io("清理未提交 current", &bundle_path, source))?;
        }
    }
    sync(&bundle, "同步未提交 current 清理", &bundle_path)?;
    if content_exists {
        let contents = open_expected_directory_at(&bundle, OsStr::new("contents"), &contents_path)?;
        ensure_open_directory_matches_managed_path(paths, &contents, &contents_path)?;
        ensure_only_entries(
            &read_entry_names_from_handle(&contents)?,
            &[journal.content_id.as_str()],
            "待清理 contents 包含未知内容",
        )?;
        if entry_metadata_at(&contents, OsStr::new(&journal.content_id))
            .map_err(|source| takeover_io("检查待清理候选内容", &content_path, source))?
            .is_some()
        {
            validate_published_content(paths, &contents, &content_path, contract, journal)?;
            remove_owned_tree_at(&contents, OsStr::new(&journal.content_id), &content_path)?;
        }
        drop(contents);
        remove_empty_directory_at(&bundle, OsStr::new("contents"), &contents_path)?;
    }
    drop(bundle);
    remove_empty_directory_at(&bundles, OsStr::new(&journal.bundle_id), &bundle_path)
        .map_err(Into::into)
}

fn validate_published_content(
    paths: &ApplicationPaths,
    contents: &File,
    content_path: &Path,
    contract: &TakeoverPlanContract,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverError> {
    let content =
        open_expected_directory_at(contents, OsStr::new(&journal.content_id), content_path)?;
    ensure_open_directory_matches_managed_path(paths, &content, content_path)?;
    ensure_only_entries(
        &read_entry_names_from_handle(&content)?,
        &["members"],
        "待清理候选内容边界异常",
    )?;
    let members_path = content_path.join("members");
    let members = open_expected_directory_at(&content, OsStr::new("members"), &members_path)?;
    ensure_open_directory_matches_managed_path(paths, &members, &members_path)?;
    ensure_only_entries(
        &read_entry_names_from_handle(&members)?,
        &[contract.plan.skill_name.as_str()],
        "待清理候选成员边界异常",
    )?;
    let member_path = members_path.join(&contract.plan.skill_name);
    let validated = validate_single_skill_folder(&member_path)?;
    let selected = contract
        .plan
        .origins
        .iter()
        .find(|origin| origin.observation_id == contract.plan.selected_observation_id)
        .ok_or(TakeoverError::InvalidPlanContract)?;
    if validated.name != contract.plan.skill_name
        || validated.fingerprint != selected.content_fingerprint
    {
        return Err(TakeoverError::RecoveryBlocked(
            "待清理候选内容已被外部修改".to_owned(),
        ));
    }
    Ok(())
}

fn cleanup_uncommitted_staging(
    paths: &ApplicationPaths,
    managed_root: &File,
    contract: &TakeoverPlanContract,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverError> {
    let staging_root_path = paths.staging_root();
    let staging_root = open_managed_directory_from_root(paths, managed_root, &staging_root_path)?;
    let staging_path = staging_root_path.join(&journal.transaction_id);
    let staging = match open_directory_at(&staging_root, OsStr::new(&journal.transaction_id)) {
        Ok(staging) => staging,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(takeover_io("安全打开待清理 staging", &staging_path, source)),
    };
    ensure_open_directory_matches_managed_path(paths, &staging, &staging_path)?;
    ensure_only_entries(
        &read_entry_names_from_handle(&staging)?,
        &["candidate"],
        "待清理 staging 包含未知条目",
    )?;
    let candidate_path = staging_path.join("candidate");
    if entry_metadata_at(&staging, OsStr::new("candidate"))
        .map_err(|source| takeover_io("检查待清理 candidate", &candidate_path, source))?
        .is_some()
    {
        let candidate =
            open_expected_directory_at(&staging, OsStr::new("candidate"), &candidate_path)?;
        ensure_open_directory_matches_managed_path(paths, &candidate, &candidate_path)?;
        ensure_only_entries(
            &read_entry_names_from_handle(&candidate)?,
            &["members"],
            "待清理 candidate 包含未知条目",
        )?;
        let members_path = candidate_path.join("members");
        if entry_metadata_at(&candidate, OsStr::new("members"))
            .map_err(|source| takeover_io("检查待清理 candidate members", &members_path, source))?
            .is_some()
        {
            let members =
                open_expected_directory_at(&candidate, OsStr::new("members"), &members_path)?;
            ensure_open_directory_matches_managed_path(paths, &members, &members_path)?;
            ensure_only_entries(
                &read_entry_names_from_handle(&members)?,
                &[contract.plan.skill_name.as_str()],
                "待清理 candidate members 包含未知成员",
            )?;
            if entry_metadata_at(&members, OsStr::new(&contract.plan.skill_name))
                .map_err(|source| {
                    takeover_io("检查待清理 candidate member", &members_path, source)
                })?
                .is_some()
            {
                remove_owned_tree_at(
                    &members,
                    OsStr::new(&contract.plan.skill_name),
                    &members_path.join(&contract.plan.skill_name),
                )?;
            }
            drop(members);
            remove_empty_directory_at(&candidate, OsStr::new("members"), &members_path)?;
        }
        drop(candidate);
        remove_empty_directory_at(&staging, OsStr::new("candidate"), &candidate_path)?;
    }
    drop(staging);
    remove_empty_directory_at(
        &staging_root,
        OsStr::new(&journal.transaction_id),
        &staging_path,
    )
    .map_err(Into::into)
}

fn validate_optional_current(
    bundle: &File,
    name: &OsStr,
    expected_target: &Path,
    path: &Path,
) -> Result<bool, TakeoverError> {
    let Some(metadata) = entry_metadata_at(bundle, name)
        .map_err(|source| takeover_io("检查待清理 current", path, source))?
    else {
        return Ok(false);
    };
    if metadata.st_mode & libc::S_IFMT != libc::S_IFLNK
        || read_link_at(bundle, name)
            .map_err(|source| takeover_io("读取待清理 current", path, source))?
            != expected_target
    {
        return Err(TakeoverError::RecoveryBlocked(format!(
            "待清理 current 已被外部修改：{}",
            path.display()
        )));
    }
    Ok(true)
}

fn ensure_only_entries(
    entries: &[String],
    allowed: &[&str],
    message: &'static str,
) -> Result<(), TakeoverError> {
    if entries
        .iter()
        .all(|entry| allowed.contains(&entry.as_str()))
    {
        Ok(())
    } else {
        Err(TakeoverError::RecoveryBlocked(message.to_owned()))
    }
}

fn sync(file: &File, action: &'static str, path: &Path) -> Result<(), TakeoverError> {
    file.sync_all()
        .map_err(|source| takeover_io(action, path, source))
}

fn takeover_io(action: &'static str, path: &Path, source: std::io::Error) -> TakeoverError {
    TakeoverError::Io {
        action,
        path: path.display().to_string(),
        source,
    }
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
