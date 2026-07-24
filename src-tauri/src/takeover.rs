use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{ErrorKind, Read},
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
        InventoryItem, ManagementKind, MountScope, ScanRootKey, SkillMetadataStatus,
        SupportedAppId, TakeoverIdentityBasis, TakeoverOriginDisposition, TakeoverPlan,
        TakeoverPlanOrigin, TakeoverPlanRequest, TakeoverPlanTarget, UiOutcome,
    },
    lifecycle::{
        LifecycleError, LifecycleFailpoint, LifecycleLock, OwnedTreeCleanupManifest,
        acquire_lifecycle_lock, capture_owned_tree_cleanup_manifest, ensure_entry_absent_at,
        ensure_open_directory_matches_managed_path, entry_metadata_at, mkdir_at, open_directory_at,
        open_expected_directory_at, open_managed_directory_from_root, open_regular_file_at,
        read_entry_names_from_handle, read_link_at, remove_empty_directory_at,
        remove_owned_tree_at_with_manifest_and_hook, rename_at_no_replace, symlink_at, unlink_at,
        validate_owned_tree_cleanup_manifest, write_atomic_at,
        write_atomic_at_with_after_temp_sync, write_notice_from_storage,
    },
    mount_lifecycle::{
        MountLifecycleError, OpenMountParent, ParentLookup, TargetKind, TargetSnapshot,
        open_mount_parent, open_project_relative_parent, open_relative_parent, recheck_open_parent,
        snapshot_at,
    },
    paths::ApplicationPaths,
    storage::{
        Storage, StorageError, StoredProject, StoredTakeoverPlanRow, StoredTakeoverTransaction,
        takeover_reserved_paths,
    },
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
    project: Option<TakeoverProjectSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TakeoverTargetSnapshot {
    mount_id: String,
    target_path: String,
    target_observation: String,
    occupied_by_observation_id: Option<String>,
    project: Option<TakeoverProjectSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TakeoverProjectSnapshot {
    id: String,
    display_name: String,
    root_path: String,
    root_device: u64,
    root_inode: u64,
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
    expected_fingerprint: String,
    original_observation: String,
    recovery_observation: Option<String>,
    // 根目录身份与删除意图一起持久化，避免重启后删除同名替换目录。
    cleanup_manifest: Option<OwnedTreeCleanupManifest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TakeoverCandidateCleanupLocation {
    PublishedContent,
    StagingCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TakeoverCandidateCleanupTree {
    location: TakeoverCandidateCleanupLocation,
    manifest: OwnedTreeCleanupManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TakeoverCandidateCleanupIntent {
    // None 表示意图持久化时没有递归候选树，只需清理外围链接和空目录。
    tree: Option<TakeoverCandidateCleanupTree>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TakeoverJournalTarget {
    mount_id: String,
    target_path: String,
    mount_staging_name: String,
    expected_target: String,
    target_observation: String,
    occupied_by_observation_id: Option<String>,
    // 创建意图必须先于 symlink 持久化，供崩溃恢复证明 UUID 临时名的归属。
    mount_create_intent: bool,
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
    // 回滚先持久化候选根目录身份，再开始可重入递归清理。
    candidate_cleanup: Option<TakeoverCandidateCleanupIntent>,
    origins: Vec<TakeoverJournalOrigin>,
    targets: Vec<TakeoverJournalTarget>,
}

struct PreparedOrigin {
    observation: InventoryItem,
    app_id: Option<SupportedAppId>,
    scope: Option<MountScope>,
    project: Option<StoredProject>,
    original_root: PathBuf,
    validated: ValidatedSingleSkill,
    root_device: u64,
    root_inode: u64,
    root_mode: u32,
    target_observation: String,
}

struct ResolvedOriginLocation {
    app_id: Option<SupportedAppId>,
    scope: Option<MountScope>,
    project: Option<StoredProject>,
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
    ensure_takeover_origins_are_writable(storage, &observations)?;
    let app_configs = paths.supported_apps();
    let mut prepared_origins = Vec::with_capacity(observations.len());
    let mut original_paths = BTreeSet::new();
    for observation in observations {
        validate_observation(&observation)?;
        let location = resolve_observation_location(storage, &app_configs, &observation)?;
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
        let parent = open_resolved_origin_parent(
            paths,
            location.app_id,
            location.scope,
            location.project.as_ref(),
            &original_root,
            &validated.name,
        )?;
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
            app_id: location.app_id,
            scope: location.scope,
            project: location.project,
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
    let mut targets =
        Vec::with_capacity(request.preserved_observation_ids.len() + request.shared_targets.len());
    let mut target_snapshots = Vec::with_capacity(targets.capacity());
    for prepared in &prepared_origins {
        let preserve_mount = request
            .preserved_observation_ids
            .contains(&prepared.observation.id);
        if preserve_mount && prepared.app_id.is_none() {
            return Err(TakeoverError::InvalidRequest(
                "共享目录不能继续作为 Mount，请选择应用专属目标".to_owned(),
            ));
        }
        let original_path = path_text(&prepared.original_root)?;
        origin_snapshots.push(TakeoverOriginSnapshot {
            observation_id: prepared.observation.id.clone(),
            root_device: prepared.root_device,
            root_inode: prepared.root_inode,
            root_mode: prepared.root_mode,
            target_observation: prepared.target_observation.clone(),
            project: prepared.project.as_ref().map(project_snapshot),
        });
        origins.push(TakeoverPlanOrigin {
            observation_id: prepared.observation.id.clone(),
            original_path: original_path.clone(),
            app_id: prepared.app_id,
            scope: prepared.scope,
            project_id: prepared.project.as_ref().map(|project| project.id.clone()),
            project_display_name: prepared
                .project
                .as_ref()
                .map(|project| project.display_name.clone()),
            content_fingerprint: prepared.validated.fingerprint.clone(),
            warnings: prepared.validated.warnings.clone(),
            final_disposition: if preserve_mount {
                TakeoverOriginDisposition::Mount
            } else {
                TakeoverOriginDisposition::Remove
            },
        });
        if preserve_mount {
            let app_id = prepared.app_id.ok_or(TakeoverError::InvalidPlanContract)?;
            let scope = prepared.scope.ok_or(TakeoverError::InvalidPlanContract)?;
            let mount_id = Uuid::new_v4().to_string();
            targets.push(TakeoverPlanTarget {
                mount_id: mount_id.clone(),
                app_id,
                scope,
                project_id: prepared.project.as_ref().map(|project| project.id.clone()),
                project_display_name: prepared
                    .project
                    .as_ref()
                    .map(|project| project.display_name.clone()),
                target_path: original_path.clone(),
                expected_target: expected_target_text.clone(),
            });
            target_snapshots.push(TakeoverTargetSnapshot {
                mount_id,
                target_path: original_path,
                target_observation: prepared.target_observation.clone(),
                occupied_by_observation_id: Some(prepared.observation.id.clone()),
                project: prepared.project.as_ref().map(project_snapshot),
            });
        }
    }
    for shared_target in &request.shared_targets {
        let prepared = prepared_origins
            .iter()
            .find(|origin| origin.observation.id == shared_target.shared_observation_id)
            .ok_or_else(|| {
                TakeoverError::InvalidRequest("共享目标没有对应的接管副本".to_owned())
            })?;
        if prepared.app_id.is_some()
            || prepared.scope.is_some()
            || !prepared
                .observation
                .observed_by
                .contains(&shared_target.app_id)
        {
            return Err(TakeoverError::InvalidRequest(
                "共享目标与扫描到的兼容应用不一致".to_owned(),
            ));
        }
        let (scope, project) = match prepared.project.as_ref() {
            Some(project) => (MountScope::Project, Some(project)),
            None => (MountScope::Global, None),
        };
        let app = app_configs
            .iter()
            .find(|app| app.id == shared_target.app_id)
            .ok_or(TakeoverError::InvalidPlanContract)?;
        let target_root = match project {
            Some(project) => Path::new(&project.root_path).join(&app.project_relative_root),
            None => app.global_root.clone(),
        };
        let target_path = target_root.join(&skill_name);
        let target_path_text = path_text(&target_path)?;
        if targets
            .iter()
            .any(|target| target.target_path == target_path_text)
        {
            return Err(TakeoverError::InvalidRequest(
                "共享目标与已有最终 Mount 重复".to_owned(),
            ));
        }
        let (target_kind, target_observation) = snapshot_new_target(
            paths,
            shared_target.app_id,
            scope,
            project,
            &skill_name,
            &target_path,
            &expected_target_text,
        )?;
        if target_kind != TargetKind::Absent {
            return Err(TakeoverError::InvalidRequest(format!(
                "共享目标已被其他内容占用：{}",
                target_path.display()
            )));
        }
        let mount_id = Uuid::new_v4().to_string();
        targets.push(TakeoverPlanTarget {
            mount_id: mount_id.clone(),
            app_id: shared_target.app_id,
            scope,
            project_id: project.map(|project| project.id.clone()),
            project_display_name: project.map(|project| project.display_name.clone()),
            target_path: target_path_text.clone(),
            expected_target: expected_target_text.clone(),
        });
        target_snapshots.push(TakeoverTargetSnapshot {
            mount_id,
            target_path: target_path_text,
            target_observation,
            occupied_by_observation_id: None,
            project: project.map(project_snapshot),
        });
    }
    validate_scope_resolution(&origins, &targets)?;
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
        installation_chain: selected
            .observation
            .installation_chain
            .clone()
            .map(Box::new),
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
    ensure_takeover_plan_origins_are_writable(storage, &contract.plan)?;
    validate_confirmation_contract(paths, storage, &stored, &contract, now)?;

    let transaction_id = Uuid::new_v4().to_string();
    let journal_relative = format!("journals/takeover-{transaction_id}.json");
    let reserved_paths = takeover_reserved_paths(&contract.plan)?;
    let mut journal = build_journal(&transaction_id, &contract)?;
    ensure_journal_fits(&journal)?;
    let consumed = storage.begin_takeover_transaction(
        plan_id,
        &transaction_id,
        &contract.plan.bundle_id,
        &contract.plan.member_id,
        &reserved_paths,
        &journal_relative,
        now,
    )?;
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
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverTransactionRecord,
    );

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
    if let Err(error) = verify_takeover_targets(paths, storage, &contract, &journal) {
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
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverStateCommittedBeforeJournal,
    );

    journal.phase = TakeoverJournalPhase::StateCommitted;
    write_journal(paths, lifecycle_lock.root(), &journal)?;
    write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
    cleanup_committed(
        paths,
        lifecycle_lock.root(),
        storage,
        &contract,
        &mut journal,
        failpoint,
    )?;
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

/// 启动恢复只重放同一份 Takeover Journal；SQLite 的提交点决定回滚或向前清理。
pub(crate) fn recover_pending_takeover_transactions(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    for transaction in storage.recoverable_takeover_transactions()? {
        if transaction.status == "blocked" {
            continue;
        }
        if let Err(error) = recover_takeover_transaction(
            paths,
            &lifecycle_lock,
            storage,
            &transaction,
            now,
            failpoint,
        ) {
            storage.block_takeover_transaction(&transaction.id, &error.to_string(), now)?;
        }
        lifecycle_lock.recheck(paths)?;
    }
    Ok(())
}

fn recover_takeover_transaction(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transaction: &StoredTakeoverTransaction,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverError> {
    validate_takeover_transaction_identity(transaction)?;
    let stored = storage.read_takeover_plan(&transaction.plan_id)?;
    let contract = decode_plan_contract(&stored)?;
    validate_recovery_contract(paths, storage, transaction, &stored, &contract)?;
    reconcile_takeover_journal_temp(paths, lifecycle_lock.root(), transaction, &contract)?;
    let Some(mut journal) = read_takeover_journal(paths, lifecycle_lock.root(), transaction)?
    else {
        return recover_takeover_without_journal(
            paths,
            lifecycle_lock,
            storage,
            transaction,
            &contract,
            now,
        );
    };
    validate_takeover_journal(transaction, &contract, &journal)?;

    if transaction.status == "completed" && transaction.phase == "state_committed" {
        // SQLite 已越过唯一提交点后只能向前，Journal 可能仍停在 origins_applied。
        storage.finalize_takeover(&transaction.id, &contract.plan, now)?;
        validate_committed_takeover_content(paths, lifecycle_lock.root(), &contract, &journal)?;
        verify_takeover_targets(paths, storage, &contract, &journal)?;
        journal.phase = TakeoverJournalPhase::StateCommitted;
        write_journal(paths, lifecycle_lock.root(), &journal)?;
        write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
        cleanup_committed(
            paths,
            lifecycle_lock.root(),
            storage,
            &contract,
            &mut journal,
            failpoint,
        )?;
        return Ok(());
    }

    if transaction.status != "in_progress" && transaction.status != "aborted" {
        return Err(TakeoverError::RecoveryBlocked(
            "Takeover 事务状态与提交点不一致".to_owned(),
        ));
    }
    rollback_before_commit(
        paths,
        lifecycle_lock.root(),
        storage,
        &contract,
        &mut journal,
        failpoint,
    )?;
    if transaction.status == "in_progress" {
        storage.abort_takeover_transaction(&transaction.id, None, now)?;
    }
    remove_journal_if_present(paths, lifecycle_lock.root(), &transaction.id)?;
    storage.forget_terminal_takeover_transaction(&transaction.id)?;
    Ok(())
}

fn recover_takeover_without_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transaction: &StoredTakeoverTransaction,
    contract: &TakeoverPlanContract,
    now: i64,
) -> Result<(), TakeoverError> {
    let journal = build_journal(&transaction.id, contract)?;
    if transaction.status == "completed" && transaction.phase == "state_committed" {
        storage.finalize_takeover(&transaction.id, &contract.plan, now)?;
        validate_committed_takeover_content(paths, lifecycle_lock.root(), contract, &journal)?;
        verify_expected_takeover_targets(paths, storage, contract, &journal)?;
        ensure_committed_cleanup_finished(
            paths,
            lifecycle_lock.root(),
            storage,
            contract,
            &journal,
        )?;
        write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
        storage.forget_terminal_takeover_transaction(&transaction.id)?;
        return Ok(());
    }
    if transaction.status != "in_progress" && transaction.status != "aborted" {
        return Err(TakeoverError::RecoveryBlocked(
            "缺少 Journal 的 Takeover 事务状态无法自动判断".to_owned(),
        ));
    }
    ensure_no_takeover_effects_without_journal(
        paths,
        lifecycle_lock.root(),
        storage,
        contract,
        &journal,
    )?;
    if transaction.status == "in_progress" {
        storage.abort_takeover_transaction(&transaction.id, None, now)?;
    }
    storage.forget_terminal_takeover_transaction(&transaction.id)?;
    Ok(())
}

fn validate_takeover_transaction_identity(
    transaction: &StoredTakeoverTransaction,
) -> Result<(), TakeoverError> {
    for value in [
        &transaction.id,
        &transaction.plan_id,
        &transaction.bundle_id,
        &transaction.member_id,
    ] {
        Uuid::parse_str(value).map_err(|_| TakeoverError::InvalidPlanContract)?;
    }
    let expected_journal = format!("journals/takeover-{}.json", transaction.id);
    if transaction.journal_path != expected_journal {
        return Err(TakeoverError::RecoveryBlocked(
            "SQLite 中的 Takeover Journal 路径不符合固定布局".to_owned(),
        ));
    }
    Ok(())
}

fn validate_recovery_contract(
    paths: &ApplicationPaths,
    storage: &Storage,
    transaction: &StoredTakeoverTransaction,
    stored: &StoredTakeoverPlanRow,
    contract: &TakeoverPlanContract,
) -> Result<(), TakeoverError> {
    let plan = &contract.plan;
    let origin_ids = plan
        .origins
        .iter()
        .map(|origin| origin.observation_id.as_str())
        .collect::<BTreeSet<_>>();
    let origin_paths = plan
        .origins
        .iter()
        .map(|origin| origin.original_path.as_str())
        .collect::<BTreeSet<_>>();
    let target_ids = plan
        .targets
        .iter()
        .map(|target| target.mount_id.as_str())
        .collect::<BTreeSet<_>>();
    let target_paths = plan
        .targets
        .iter()
        .map(|target| target.target_path.as_str())
        .collect::<BTreeSet<_>>();
    let selected_count = plan
        .origins
        .iter()
        .filter(|origin| origin.observation_id == plan.selected_observation_id)
        .count();
    let identity_basis_matches = matches!(
        (plan.origins.len(), plan.identity_basis),
        (1, TakeoverIdentityBasis::SingleOrigin)
    ) || (plan.origins.len() > 1
        && plan.identity_basis == TakeoverIdentityBasis::UserConfirmed);
    if stored.status != "consumed"
        || transaction.plan_id != plan.id
        || transaction.bundle_id != plan.bundle_id
        || transaction.member_id != plan.member_id
        || transaction.reserved_paths != takeover_reserved_paths(plan)?
        || plan.origins.is_empty()
        || contract.origin_snapshots.len() != plan.origins.len()
        || contract.target_snapshots.len() != plan.targets.len()
        || origin_ids.len() != plan.origins.len()
        || origin_paths.len() != plan.origins.len()
        || target_ids.len() != plan.targets.len()
        || target_paths.len() != plan.targets.len()
        || selected_count != 1
        || !identity_basis_matches
    {
        return Err(TakeoverError::InvalidPlanContract);
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
    validate_scope_resolution(&plan.origins, &plan.targets)
        .map_err(|_| TakeoverError::InvalidPlanContract)?;
    for (origin, snapshot) in plan.origins.iter().zip(&contract.origin_snapshots) {
        if snapshot.observation_id != origin.observation_id {
            return Err(TakeoverError::InvalidPlanContract);
        }
        validate_origin_project_contract(storage, origin, snapshot.project.as_ref())?;
    }
    for (target, snapshot) in plan.targets.iter().zip(&contract.target_snapshots) {
        Uuid::parse_str(&target.mount_id).map_err(|_| TakeoverError::InvalidPlanContract)?;
        if snapshot.mount_id != target.mount_id
            || snapshot.target_path != target.target_path
            || target.expected_target != plan.expected_target
        {
            return Err(TakeoverError::InvalidPlanContract);
        }
        validate_target_project_contract(storage, target, snapshot.project.as_ref())?;
    }
    Ok(())
}

fn read_takeover_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    transaction: &StoredTakeoverTransaction,
) -> Result<Option<TakeoverJournal>, TakeoverError> {
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let name = journal_file_name(&transaction.id);
    let path = paths.journals_root().join(&name);
    if entry_metadata_at(&journals, &name)
        .map_err(|source| takeover_io("检查 Takeover Journal", &path, source))?
        .is_none()
    {
        return Ok(None);
    }
    Ok(Some(read_takeover_journal_file(&journals, &name, &path)?))
}

fn read_takeover_journal_file(
    journals: &File,
    name: &OsStr,
    path: &Path,
) -> Result<TakeoverJournal, TakeoverError> {
    let file = open_regular_file_at(journals, name, path, false)?;
    let metadata = file
        .metadata()
        .map_err(|source| takeover_io("读取 Takeover Journal 元数据", path, source))?;
    if metadata.len() > MAX_TAKEOVER_JOURNAL_BYTES as u64 {
        return Err(TakeoverError::JournalTooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_TAKEOVER_JOURNAL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| takeover_io("读取 Takeover Journal", path, source))?;
    if bytes.len() > MAX_TAKEOVER_JOURNAL_BYTES {
        return Err(TakeoverError::JournalTooLarge);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn reconcile_takeover_journal_temp(
    paths: &ApplicationPaths,
    managed_root: &File,
    transaction: &StoredTakeoverTransaction,
    contract: &TakeoverPlanContract,
) -> Result<(), TakeoverError> {
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let prefix = format!(".takeover-{}.json.tmp-", transaction.id);
    let mut matches = Vec::new();
    for name in read_entry_names_from_handle(&journals)? {
        let Some(suffix) = name.strip_prefix(&prefix) else {
            continue;
        };
        if Uuid::parse_str(suffix).is_err() {
            return Err(TakeoverError::RecoveryBlocked(
                "Takeover Journal 临时文件名不符合固定布局".to_owned(),
            ));
        }
        matches.push(name);
    }
    if matches.len() > 1 {
        return Err(TakeoverError::RecoveryBlocked(
            "同一 Takeover 事务存在多个 Journal 临时文件".to_owned(),
        ));
    }
    let Some(name) = matches.pop() else {
        return Ok(());
    };
    let path = paths.journals_root().join(&name);
    let temporary = read_takeover_journal_file(&journals, OsStr::new(&name), &path)?;
    let canonical_name = journal_file_name(&transaction.id);
    let canonical_exists = entry_metadata_at(&journals, &canonical_name)
        .map_err(|source| takeover_io("检查正式 Takeover Journal", &path, source))?
        .is_some();
    if canonical_exists {
        validate_takeover_journal(transaction, contract, &temporary)?;
    } else if temporary != build_journal(&transaction.id, contract)? {
        return Err(TakeoverError::RecoveryBlocked(
            "首次 Takeover Journal 临时文件不符合初始合同".to_owned(),
        ));
    }
    // rename 才是原子写入生效点；合法临时文件只代表一次未提交的 Journal 写入。
    unlink_at(&journals, OsStr::new(&name), false)
        .map_err(|source| takeover_io("清理未提交 Takeover Journal", &path, source))?;
    sync(
        &journals,
        "同步 Journal 临时文件清理",
        &paths.journals_root(),
    )
}

fn validate_takeover_journal(
    transaction: &StoredTakeoverTransaction,
    contract: &TakeoverPlanContract,
    actual: &TakeoverJournal,
) -> Result<(), TakeoverError> {
    for manifest in actual
        .origins
        .iter()
        .filter_map(|origin| origin.cleanup_manifest.as_ref())
        .chain(
            actual
                .candidate_cleanup
                .as_ref()
                .and_then(|intent| intent.tree.as_ref())
                .map(|tree| &tree.manifest),
        )
    {
        validate_owned_tree_cleanup_manifest(manifest)?;
    }
    let mut expected = build_journal(&transaction.id, contract)?;
    if actual.origins.len() != expected.origins.len()
        || actual.targets.len() != expected.targets.len()
        || !journal_phase_matches_storage(actual.phase, &transaction.phase)
    {
        return Err(TakeoverError::RecoveryBlocked(
            "SQLite、Plan 与 Takeover Journal 的边界不一致".to_owned(),
        ));
    }
    let origin_progress_is_valid = actual.origins.iter().all(|origin| {
        (origin.recovery_observation.is_none()
            || origin.recovery_observation.as_deref() == Some(origin.original_observation.as_str()))
            && (origin.cleanup_manifest.is_none()
                || (actual.phase == TakeoverJournalPhase::StateCommitted
                    && origin.recovery_observation.is_some()))
    });
    let target_progress_is_valid = actual
        .targets
        .iter()
        .all(|target| target.mount_observation.is_none() || target.mount_create_intent);
    let phase_progress_is_valid = match actual.phase {
        TakeoverJournalPhase::JournalReady | TakeoverJournalPhase::CandidateReady => {
            actual
                .origins
                .iter()
                .all(|origin| origin.recovery_observation.is_none())
                && actual
                    .targets
                    .iter()
                    .all(|target| !target.mount_create_intent && target.mount_observation.is_none())
        }
        TakeoverJournalPhase::CurrentActivated => true,
        // 回滚会在同一 phase 下逐项清空进度；精确 observation 决定每项能否继续。
        TakeoverJournalPhase::OriginsApplied => true,
        TakeoverJournalPhase::StateCommitted => actual
            .targets
            .iter()
            .all(|target| target.mount_create_intent && target.mount_observation.is_some()),
    };
    let cleanup_progress_is_valid = actual
        .origins
        .iter()
        .filter(|origin| origin.cleanup_manifest.is_some())
        .count()
        <= 1
        && (actual.candidate_cleanup.is_none()
            || (actual.phase != TakeoverJournalPhase::StateCommitted
                && actual
                    .origins
                    .iter()
                    .all(|origin| origin.cleanup_manifest.is_none())));
    if !origin_progress_is_valid
        || !target_progress_is_valid
        || !phase_progress_is_valid
        || !cleanup_progress_is_valid
    {
        return Err(TakeoverError::RecoveryBlocked(
            "Takeover Journal 的逐路径进度不一致".to_owned(),
        ));
    }
    expected.phase = actual.phase;
    expected.candidate_cleanup = actual.candidate_cleanup.clone();
    for (expected, actual) in expected.origins.iter_mut().zip(&actual.origins) {
        expected.recovery_observation = actual.recovery_observation.clone();
        expected.cleanup_manifest = actual.cleanup_manifest.clone();
    }
    for (expected, actual) in expected.targets.iter_mut().zip(&actual.targets) {
        expected.mount_create_intent = actual.mount_create_intent;
        expected.mount_observation = actual.mount_observation.clone();
    }
    if &expected == actual {
        Ok(())
    } else {
        Err(TakeoverError::RecoveryBlocked(
            "SQLite、Plan 与 Takeover Journal 的边界不一致".to_owned(),
        ))
    }
}

fn journal_phase_matches_storage(journal: TakeoverJournalPhase, storage: &str) -> bool {
    matches!(
        (storage, journal),
        ("journal_pending", TakeoverJournalPhase::JournalReady)
            | ("journal_ready", TakeoverJournalPhase::JournalReady)
            | ("journal_ready", TakeoverJournalPhase::CandidateReady)
            | ("candidate_ready", TakeoverJournalPhase::CandidateReady)
            | ("candidate_ready", TakeoverJournalPhase::CurrentActivated)
            | ("current_activated", TakeoverJournalPhase::CurrentActivated)
            | ("current_activated", TakeoverJournalPhase::OriginsApplied)
            | ("origins_applied", TakeoverJournalPhase::OriginsApplied)
            | ("state_committed", TakeoverJournalPhase::OriginsApplied)
            | ("state_committed", TakeoverJournalPhase::StateCommitted)
    )
}

fn validate_committed_takeover_content(
    paths: &ApplicationPaths,
    managed_root: &File,
    contract: &TakeoverPlanContract,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverError> {
    let bundles = open_managed_directory_from_root(paths, managed_root, &paths.bundles_root())?;
    let bundle_path = paths.bundle_directory(&journal.bundle_id);
    let bundle =
        open_expected_directory_at(&bundles, OsStr::new(&journal.bundle_id), &bundle_path)?;
    ensure_open_directory_matches_managed_path(paths, &bundle, &bundle_path)?;
    ensure_only_entries(
        &read_entry_names_from_handle(&bundle)?,
        &["contents", "current"],
        "已提交 Bundle 包含未知条目",
    )?;
    if !validate_optional_current(
        &bundle,
        OsStr::new("current"),
        &Path::new("contents").join(&journal.content_id),
        &bundle_path.join("current"),
    )? {
        return Err(TakeoverError::RecoveryBlocked(
            "已提交 Bundle 的 current 已缺失".to_owned(),
        ));
    }
    let contents_path = bundle_path.join("contents");
    let contents = open_expected_directory_at(&bundle, OsStr::new("contents"), &contents_path)?;
    ensure_only_entries(
        &read_entry_names_from_handle(&contents)?,
        &[journal.content_id.as_str()],
        "已提交 Bundle 包含未知内容",
    )?;
    validate_published_content(
        paths,
        &contents,
        &contents_path.join(&journal.content_id),
        contract,
        journal,
    )
}

fn verify_expected_takeover_targets(
    paths: &ApplicationPaths,
    storage: &Storage,
    contract: &TakeoverPlanContract,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverError> {
    for (index, target) in contract.plan.targets.iter().enumerate() {
        let progress = journal
            .targets
            .get(index)
            .ok_or(TakeoverError::InvalidPlanContract)?;
        let parent = open_plan_target_parent(paths, storage, target, false)?;
        let leaf = target_leaf(target, &contract.plan.skill_name)?;
        let current = snapshot_at(parent.directory(), &leaf, &target.expected_target)?;
        let staged = snapshot_at(
            parent.directory(),
            OsStr::new(&progress.mount_staging_name),
            &target.expected_target,
        )?;
        if current.kind() != TargetKind::ExpectedLink || staged.kind() != TargetKind::Absent {
            return Err(TakeoverError::RecoveryBlocked(
                "已提交 Takeover 的最终 Mount 状态不一致".to_owned(),
            ));
        }
        recheck_open_parent(&parent)?;
    }
    Ok(())
}

fn ensure_committed_cleanup_finished(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &Storage,
    contract: &TakeoverPlanContract,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverError> {
    for (index, origin) in contract.plan.origins.iter().enumerate() {
        let parent = open_origin_parent(paths, storage, origin, &contract.plan.skill_name)?;
        let recovery_name = OsStr::new(&journal.origins[index].recovery_name);
        if snapshot_at(parent.directory(), recovery_name, "")?.kind() != TargetKind::Absent {
            return Err(TakeoverError::RecoveryBlocked(
                "Takeover Journal 已缺失，但旧副本清理尚未完成".to_owned(),
            ));
        }
        recheck_open_parent(&parent)?;
    }
    let staging_root =
        open_managed_directory_from_root(paths, managed_root, &paths.staging_root())?;
    if entry_metadata_at(&staging_root, OsStr::new(&journal.transaction_id))
        .map_err(|source| takeover_io("检查接管临时目录", &paths.staging_root(), source))?
        .is_some()
    {
        return Err(TakeoverError::RecoveryBlocked(
            "Takeover Journal 已缺失，但临时目录尚未清理".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_no_takeover_effects_without_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &Storage,
    contract: &TakeoverPlanContract,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverError> {
    match fs::symlink_metadata(&contract.plan.managed_directory) {
        Ok(_) => {
            return Err(TakeoverError::RecoveryBlocked(
                "Takeover Journal 缺失，但受管 Bundle 已经出现".to_owned(),
            ));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(source) => {
            return Err(takeover_io(
                "检查缺少 Journal 的 Bundle",
                Path::new(&contract.plan.managed_directory),
                source,
            ));
        }
    }
    let staging_root =
        open_managed_directory_from_root(paths, managed_root, &paths.staging_root())?;
    if entry_metadata_at(&staging_root, OsStr::new(&journal.transaction_id))
        .map_err(|source| takeover_io("检查接管临时目录", &paths.staging_root(), source))?
        .is_some()
    {
        return Err(TakeoverError::RecoveryBlocked(
            "Takeover Journal 缺失，但临时内容已经出现".to_owned(),
        ));
    }
    for (index, origin) in contract.plan.origins.iter().enumerate() {
        validate_live_origin(paths, storage, origin, &contract.origin_snapshots[index])?;
        let parent = open_origin_parent(paths, storage, origin, &contract.plan.skill_name)?;
        if snapshot_at(
            parent.directory(),
            OsStr::new(&journal.origins[index].recovery_name),
            "",
        )?
        .kind()
            != TargetKind::Absent
        {
            return Err(TakeoverError::RecoveryBlocked(
                "Takeover Journal 缺失，但原 Skill 已进入恢复位置".to_owned(),
            ));
        }
        recheck_open_parent(&parent)?;
    }
    for (index, target) in contract.plan.targets.iter().enumerate() {
        let parent = lookup_plan_target_parent(paths, storage, target, false)?;
        let ParentLookup::Open(parent) = parent else {
            continue;
        };
        if snapshot_at(
            parent.directory(),
            OsStr::new(&journal.targets[index].mount_staging_name),
            &target.expected_target,
        )?
        .kind()
            != TargetKind::Absent
        {
            return Err(TakeoverError::RecoveryBlocked(
                "Takeover Journal 缺失，但 Mount 暂存内容已经出现".to_owned(),
            ));
        }
        recheck_open_parent(&parent)?;
    }
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
    let origin_ids = plan
        .origins
        .iter()
        .map(|origin| origin.observation_id.as_str())
        .collect::<BTreeSet<_>>();
    let origin_paths = plan
        .origins
        .iter()
        .map(|origin| origin.original_path.as_str())
        .collect::<BTreeSet<_>>();
    let target_ids = plan
        .targets
        .iter()
        .map(|target| target.mount_id.as_str())
        .collect::<BTreeSet<_>>();
    let target_paths = plan
        .targets
        .iter()
        .map(|target| target.target_path.as_str())
        .collect::<BTreeSet<_>>();
    let selected_count = plan
        .origins
        .iter()
        .filter(|origin| origin.observation_id == plan.selected_observation_id)
        .count();
    let identity_basis_matches = matches!(
        (plan.origins.len(), plan.identity_basis),
        (1, TakeoverIdentityBasis::SingleOrigin)
    ) || (plan.origins.len() > 1
        && plan.identity_basis == TakeoverIdentityBasis::UserConfirmed);
    if plan.origins.is_empty()
        || contract.origin_snapshots.len() != plan.origins.len()
        || contract.target_snapshots.len() != plan.targets.len()
        || origin_ids.len() != plan.origins.len()
        || origin_paths.len() != plan.origins.len()
        || target_ids.len() != plan.targets.len()
        || target_paths.len() != plan.targets.len()
        || selected_count != 1
        || !identity_basis_matches
    {
        return Err(TakeoverError::InvalidPlanContract);
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
    validate_scope_resolution(&plan.origins, &plan.targets)
        .map_err(|_| TakeoverError::InvalidPlanContract)?;
    for target in &plan.targets {
        Uuid::parse_str(&target.mount_id).map_err(|_| TakeoverError::InvalidPlanContract)?;
    }
    for (origin, snapshot) in plan.origins.iter().zip(&contract.origin_snapshots) {
        if snapshot.observation_id != origin.observation_id {
            return Err(TakeoverError::InvalidPlanContract);
        }
        validate_origin_project_contract(storage, origin, snapshot.project.as_ref())?;
        let validated = validate_live_origin(paths, storage, origin, snapshot)?;
        if validated.name != plan.skill_name {
            return Err(TakeoverError::InvalidPlanContract);
        }
        let matching_targets = plan
            .targets
            .iter()
            .filter(|target| target.target_path == origin.original_path)
            .collect::<Vec<_>>();
        match (
            origin.app_id,
            origin.scope,
            origin.final_disposition,
            matching_targets.as_slice(),
        ) {
            (Some(_), Some(_), TakeoverOriginDisposition::Mount, [target])
                if target.expected_target == plan.expected_target
                    && Some(target.app_id) == origin.app_id
                    && Some(target.scope) == origin.scope
                    && target.project_id == origin.project_id
                    && target.project_display_name == origin.project_display_name => {}
            (Some(_), Some(_), TakeoverOriginDisposition::Remove, []) => {}
            (None, None, TakeoverOriginDisposition::Remove, []) => {}
            _ => return Err(TakeoverError::InvalidPlanContract),
        }
    }
    for (target, target_snapshot) in plan.targets.iter().zip(&contract.target_snapshots) {
        if target_snapshot.mount_id != target.mount_id
            || target_snapshot.target_path != target.target_path
            || target.expected_target != plan.expected_target
        {
            return Err(TakeoverError::InvalidPlanContract);
        }
        validate_target_project_contract(storage, target, target_snapshot.project.as_ref())?;
        let current = snapshot_plan_target(paths, storage, target)?;
        if current.observation() != target_snapshot.target_observation {
            return Err(TakeoverError::InvalidRequest(
                "Plan 生成后最终 Mount 位置已经变化".to_owned(),
            ));
        }
        match target_snapshot.occupied_by_observation_id.as_deref() {
            Some(observation_id) => {
                let occupied_origin = plan
                    .origins
                    .iter()
                    .position(|origin| origin.observation_id == observation_id)
                    .ok_or(TakeoverError::InvalidPlanContract)?;
                let origin = &plan.origins[occupied_origin];
                let origin_snapshot = &contract.origin_snapshots[occupied_origin];
                if target.target_path != origin.original_path
                    || origin.final_disposition != TakeoverOriginDisposition::Mount
                    || target_snapshot.target_observation != origin_snapshot.target_observation
                    || current.kind() != TargetKind::Other
                {
                    return Err(TakeoverError::InvalidPlanContract);
                }
            }
            None if current.kind() == TargetKind::Absent => {}
            None => return Err(TakeoverError::InvalidPlanContract),
        }
    }
    Ok(())
}

fn validate_origin_project_contract(
    storage: &Storage,
    origin: &TakeoverPlanOrigin,
    snapshot: Option<&TakeoverProjectSnapshot>,
) -> Result<(), TakeoverError> {
    match (
        origin.app_id,
        origin.scope,
        origin.project_id.as_deref(),
        snapshot,
    ) {
        (Some(_), Some(MountScope::Global), None, None)
            if origin.project_display_name.is_none() =>
        {
            Ok(())
        }
        (Some(_), Some(MountScope::Project), Some(project_id), Some(snapshot))
        | (None, None, Some(project_id), Some(snapshot))
            if origin.project_display_name.as_deref() == Some(snapshot.display_name.as_str())
                && project_id == snapshot.id =>
        {
            validate_project_snapshot(storage, snapshot)
        }
        (None, None, None, None) if origin.project_display_name.is_none() => Ok(()),
        _ => Err(TakeoverError::InvalidPlanContract),
    }
}

fn validate_target_project_contract(
    storage: &Storage,
    target: &TakeoverPlanTarget,
    snapshot: Option<&TakeoverProjectSnapshot>,
) -> Result<(), TakeoverError> {
    match (target.scope, target.project_id.as_deref(), snapshot) {
        (MountScope::Global, None, None) if target.project_display_name.is_none() => Ok(()),
        (MountScope::Project, Some(project_id), Some(snapshot))
            if target.project_display_name.as_deref() == Some(snapshot.display_name.as_str())
                && project_id == snapshot.id =>
        {
            validate_project_snapshot(storage, snapshot)
        }
        _ => Err(TakeoverError::InvalidPlanContract),
    }
}

fn validate_project_snapshot(
    storage: &Storage,
    snapshot: &TakeoverProjectSnapshot,
) -> Result<(), TakeoverError> {
    let current = storage.read_project(&snapshot.id)?;
    if project_snapshot(&current) == *snapshot {
        Ok(())
    } else {
        Err(TakeoverError::InvalidRequest(
            "Plan 生成后 Project 登记身份已经变化".to_owned(),
        ))
    }
}

fn snapshot_plan_target(
    paths: &ApplicationPaths,
    storage: &Storage,
    target: &TakeoverPlanTarget,
) -> Result<TargetSnapshot, TakeoverError> {
    match lookup_plan_target_parent(paths, storage, target, false)? {
        ParentLookup::Missing => Ok(TargetSnapshot::absent()),
        ParentLookup::Open(parent) => {
            let snapshot = snapshot_at(
                parent.directory(),
                Path::new(&target.target_path)
                    .file_name()
                    .ok_or(TakeoverError::InvalidPlanContract)?,
                &target.expected_target,
            )?;
            recheck_open_parent(&parent)?;
            Ok(snapshot)
        }
    }
}

fn lookup_plan_target_parent(
    paths: &ApplicationPaths,
    storage: &Storage,
    target: &TakeoverPlanTarget,
    create_missing: bool,
) -> Result<ParentLookup, TakeoverError> {
    let project = target
        .project_id
        .as_deref()
        .map(|project_id| storage.read_project(project_id))
        .transpose()?;
    let app = paths
        .supported_apps()
        .into_iter()
        .find(|app| app.id == target.app_id)
        .ok_or(TakeoverError::InvalidPlanContract)?;
    let target_root = match (target.scope, project.as_ref()) {
        (MountScope::Global, None) => app.global_root,
        (MountScope::Project, Some(project)) => {
            Path::new(&project.root_path).join(app.project_relative_root)
        }
        _ => return Err(TakeoverError::InvalidPlanContract),
    };
    let leaf = Path::new(&target.target_path)
        .file_name()
        .ok_or(TakeoverError::InvalidPlanContract)?;
    if target_root.join(leaf) != Path::new(&target.target_path) {
        return Err(TakeoverError::InvalidPlanContract);
    }
    let lookup = open_mount_parent(
        paths,
        target.app_id,
        target.scope,
        project.as_ref(),
        create_missing,
    )?;
    if let ParentLookup::Open(parent) = &lookup
        && parent.path().join(leaf) != Path::new(&target.target_path)
    {
        return Err(TakeoverError::InvalidPlanContract);
    }
    Ok(lookup)
}

fn open_plan_target_parent(
    paths: &ApplicationPaths,
    storage: &Storage,
    target: &TakeoverPlanTarget,
    create_missing: bool,
) -> Result<OpenMountParent, TakeoverError> {
    let ParentLookup::Open(parent) =
        lookup_plan_target_parent(paths, storage, target, create_missing)?
    else {
        return Err(TakeoverError::InvalidRequest(
            "最终 Mount 父目录尚不存在".to_owned(),
        ));
    };
    let leaf = Path::new(&target.target_path)
        .file_name()
        .ok_or(TakeoverError::InvalidPlanContract)?;
    if parent.path().join(leaf) != Path::new(&target.target_path) {
        return Err(TakeoverError::InvalidPlanContract);
    }
    Ok(parent)
}

fn target_leaf(
    target: &TakeoverPlanTarget,
    expected_name: &str,
) -> Result<OsString, TakeoverError> {
    let leaf = Path::new(&target.target_path)
        .file_name()
        .ok_or(TakeoverError::InvalidPlanContract)?;
    if leaf != OsStr::new(expected_name) {
        return Err(TakeoverError::InvalidPlanContract);
    }
    Ok(leaf.to_owned())
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
            Ok(TakeoverJournalOrigin {
                observation_id: origin.observation_id.clone(),
                original_path: origin.original_path.clone(),
                recovery_name: format!(".skillyard-takeover-{transaction_id}-{index}"),
                expected_fingerprint: origin.content_fingerprint.clone(),
                original_observation: snapshot.target_observation.clone(),
                recovery_observation: None,
                cleanup_manifest: None,
            })
        })
        .collect::<Result<Vec<_>, TakeoverError>>()?;
    let targets = plan
        .targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            let snapshot = contract
                .target_snapshots
                .get(index)
                .ok_or(TakeoverError::InvalidPlanContract)?;
            Ok(TakeoverJournalTarget {
                mount_id: target.mount_id.clone(),
                target_path: target.target_path.clone(),
                mount_staging_name: format!(".skillyard-takeover-mount-{transaction_id}-{index}"),
                expected_target: target.expected_target.clone(),
                target_observation: snapshot.target_observation.clone(),
                occupied_by_observation_id: snapshot.occupied_by_observation_id.clone(),
                mount_create_intent: false,
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
        candidate_cleanup: None,
        origins,
        targets,
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
    write_initial_journal(paths, managed_root, journal, failpoint)?;
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverJournalWrittenBeforePhase,
    );
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
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverCandidatePreparedBeforePublish,
    );

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
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverCandidatePublishedBeforePhase,
    );
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
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverTemporaryCurrentCreatedBeforeSwitch,
    );
    rename_at_no_replace(&bundle, &temporary_current, &bundle, OsStr::new("current"))
        .map_err(|source| takeover_io("切换 Bundle current", &paths.bundles_root(), source))?;
    sync(&bundle, "同步 Bundle current", &paths.bundles_root())?;
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverCurrentSwitchedBeforePhase,
    );
    journal.phase = TakeoverJournalPhase::CurrentActivated;
    persist_phase(paths, managed_root, storage, journal, now)?;

    // 应用专属原位置先腾空，全部最终 Mount 验证后才隔离共享入口。
    for index in contract
        .plan
        .origins
        .iter()
        .enumerate()
        .filter_map(|(index, origin)| origin.app_id.map(|_| index))
    {
        quarantine_takeover_origin(
            paths,
            lifecycle_lock,
            storage,
            contract,
            journal,
            index,
            failpoint,
        )?;
    }
    for index in 0..contract.plan.targets.len() {
        apply_takeover_target(
            paths,
            lifecycle_lock,
            storage,
            contract,
            journal,
            index,
            failpoint,
        )?;
        if index == 0
            && matches!(
                failpoint,
                LifecycleFailpoint::AfterFirstTakeoverOriginApplied
                    | LifecycleFailpoint::AfterFirstTakeoverTargetApplied
            )
        {
            return Err(
                LifecycleError::SimulatedInterruption("Takeover 第一个最终 Mount 已生效").into(),
            );
        }
    }
    verify_takeover_targets(paths, storage, contract, journal)?;
    for index in contract
        .plan
        .origins
        .iter()
        .enumerate()
        .filter_map(|(index, origin)| origin.app_id.is_none().then_some(index))
    {
        quarantine_takeover_origin(
            paths,
            lifecycle_lock,
            storage,
            contract,
            journal,
            index,
            failpoint,
        )?;
    }
    verify_takeover_targets(paths, storage, contract, journal)?;
    lifecycle_lock.recheck(paths)?;
    journal.phase = TakeoverJournalPhase::OriginsApplied;
    persist_phase(paths, managed_root, storage, journal, now)?;
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverOriginsAppliedBeforeState,
    );
    let activated = validate_single_skill_folder(Path::new(&contract.plan.expected_target))?;
    if activated.fingerprint != selected.content_fingerprint {
        return Err(TakeoverError::RecoveryBlocked(
            "Bundle current 内容与用户选择不一致".to_owned(),
        ));
    }
    Ok(())
}

fn quarantine_takeover_origin(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &Storage,
    contract: &TakeoverPlanContract,
    journal: &mut TakeoverJournal,
    index: usize,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverError> {
    let origin = contract
        .plan
        .origins
        .get(index)
        .ok_or(TakeoverError::InvalidPlanContract)?;
    let snapshot = contract
        .origin_snapshots
        .get(index)
        .ok_or(TakeoverError::InvalidPlanContract)?;
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
    sync(parent.directory(), "同步原 Skill 父目录", parent.path())?;
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverOriginMovedBeforeProgress,
    );
    if failpoint == LifecycleFailpoint::AfterTakeoverOriginMovedBeforeProgress {
        return Err(
            LifecycleError::SimulatedInterruption("Takeover 原目录已移动但进度尚未记录").into(),
        );
    }
    let recovery = snapshot_at(parent.directory(), &recovery_name, "")?;
    if recovery.observation() != journal.origins[index].original_observation {
        return Err(TakeoverError::RecoveryBlocked(
            "隔离后的原 Skill 不再是 Plan 中的同一目录".to_owned(),
        ));
    }
    journal.origins[index].recovery_observation = Some(recovery.observation().to_owned());
    recheck_open_parent(&parent)?;
    lifecycle_lock.recheck(paths)?;
    write_journal(paths, lifecycle_lock.root(), journal)
}

fn apply_takeover_target(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &Storage,
    contract: &TakeoverPlanContract,
    journal: &mut TakeoverJournal,
    index: usize,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverError> {
    let target = contract
        .plan
        .targets
        .get(index)
        .ok_or(TakeoverError::InvalidPlanContract)?;
    let progress = journal
        .targets
        .get(index)
        .ok_or(TakeoverError::InvalidPlanContract)?
        .clone();
    if progress.mount_id != target.mount_id || progress.target_path != target.target_path {
        return Err(TakeoverError::InvalidPlanContract);
    }
    if let Some(observation_id) = progress.occupied_by_observation_id.as_deref() {
        let origin_index = contract
            .plan
            .origins
            .iter()
            .position(|origin| origin.observation_id == observation_id)
            .ok_or(TakeoverError::InvalidPlanContract)?;
        if journal.origins[origin_index].recovery_observation.is_none() {
            return Err(TakeoverError::RecoveryBlocked(
                "最终 Mount 对应的原位置尚未安全隔离".to_owned(),
            ));
        }
    }
    let parent = open_plan_target_parent(paths, storage, target, true)?;
    let leaf = target_leaf(target, &contract.plan.skill_name)?;
    lifecycle_lock.recheck(paths)?;
    if snapshot_at(parent.directory(), &leaf, &target.expected_target)?.kind() != TargetKind::Absent
    {
        return Err(TakeoverError::InvalidRequest(
            "最终 Mount 位置在确认期间被其他内容占用".to_owned(),
        ));
    }
    let staging_name = OsString::from(&progress.mount_staging_name);
    if snapshot_at(parent.directory(), &staging_name, &target.expected_target)?.kind()
        != TargetKind::Absent
    {
        return Err(TakeoverError::RecoveryBlocked(
            "接管 Mount 暂存位置已被占用".to_owned(),
        ));
    }
    journal.targets[index].mount_create_intent = true;
    write_journal(paths, lifecycle_lock.root(), journal)?;
    symlink_at(
        Path::new(&target.expected_target),
        parent.directory(),
        &staging_name,
    )
    .map_err(|source| takeover_io("创建接管 Mount 暂存链接", parent.path(), source))?;
    sync(parent.directory(), "同步接管 Mount 暂存链接", parent.path())?;
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverMountStagedBeforeProgress,
    );
    let staged = snapshot_at(parent.directory(), &staging_name, &target.expected_target)?;
    if staged.kind() != TargetKind::ExpectedLink {
        return Err(TakeoverError::RecoveryBlocked(
            "新 Mount 暂存链接没有指向 Plan 固定的受管内容".to_owned(),
        ));
    }
    // 精确 observation 先进入内存，当前进程内的任何失败都只能撤销自己创建的链接。
    journal.targets[index].mount_observation = Some(staged.observation().to_owned());
    if failpoint == LifecycleFailpoint::AfterTakeoverMountStagedBeforeProgress {
        return Err(
            LifecycleError::SimulatedInterruption("Takeover Mount 已暂存但进度尚未记录").into(),
        );
    }
    write_journal(paths, lifecycle_lock.root(), journal)?;
    rename_at_no_replace(parent.directory(), &staging_name, parent.directory(), &leaf)
        .map_err(|source| takeover_io("发布接管 Mount", parent.path(), source))?;
    sync(parent.directory(), "同步接管 Mount", parent.path())?;
    let published = snapshot_at(parent.directory(), &leaf, &target.expected_target)?;
    if published.observation() != staged.observation() {
        return Err(TakeoverError::RecoveryBlocked(
            "发布后的 Mount 不再是本事务创建的链接".to_owned(),
        ));
    }
    recheck_open_parent(&parent)?;
    lifecycle_lock.recheck(paths)?;
    write_journal(paths, lifecycle_lock.root(), journal)
}

fn verify_takeover_targets(
    paths: &ApplicationPaths,
    storage: &Storage,
    contract: &TakeoverPlanContract,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverError> {
    for (index, target) in contract.plan.targets.iter().enumerate() {
        let progress = journal
            .targets
            .get(index)
            .ok_or(TakeoverError::InvalidPlanContract)?;
        let applied = progress.mount_observation.as_deref().ok_or_else(|| {
            TakeoverError::RecoveryBlocked("最终 Mount 缺少本事务的精确归属记录".to_owned())
        })?;
        let parent = open_plan_target_parent(paths, storage, target, false)?;
        let leaf = target_leaf(target, &contract.plan.skill_name)?;
        let current = snapshot_at(parent.directory(), &leaf, &target.expected_target)?;
        let staged = snapshot_at(
            parent.directory(),
            OsStr::new(&progress.mount_staging_name),
            &target.expected_target,
        )?;
        if current.observation() != applied || staged.kind() != TargetKind::Absent {
            return Err(TakeoverError::RecoveryBlocked(
                "最终 Mount 已被未知内容替换".to_owned(),
            ));
        }
        recheck_open_parent(&parent)?;
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
    if let Err(rollback) = rollback_before_commit(
        paths,
        managed_root,
        storage,
        contract,
        journal,
        LifecycleFailpoint::None,
    ) {
        let message = format!("原错误：{original}；恢复错误：{rollback}");
        storage.block_takeover_transaction(&journal.transaction_id, &message, now)?;
        return Err(TakeoverError::RecoveryBlocked(message));
    }
    storage.abort_takeover_transaction(
        &journal.transaction_id,
        Some(&original.to_string()),
        now,
    )?;
    if let Err(cleanup) = remove_journal_if_present(paths, managed_root, &journal.transaction_id) {
        storage.block_takeover_transaction(&journal.transaction_id, &cleanup.to_string(), now)?;
        return Err(cleanup);
    }
    storage.forget_terminal_takeover_transaction(&journal.transaction_id)?;
    Err(original)
}

fn rollback_before_commit(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &Storage,
    contract: &TakeoverPlanContract,
    journal: &mut TakeoverJournal,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverError> {
    let mut first_mount_removal_seen = false;
    for index in (0..contract.plan.targets.len()).rev() {
        let target = &contract.plan.targets[index];
        let progress = journal
            .targets
            .get(index)
            .ok_or(TakeoverError::InvalidPlanContract)?
            .clone();
        let parent = match lookup_plan_target_parent(paths, storage, target, false)? {
            ParentLookup::Missing if progress.mount_observation.is_none() => continue,
            ParentLookup::Missing => {
                return Err(TakeoverError::RecoveryBlocked(
                    "待撤销 Mount 的父目录已经消失".to_owned(),
                ));
            }
            ParentLookup::Open(parent) => parent,
        };
        let leaf = target_leaf(target, &contract.plan.skill_name)?;
        let staging_name = OsString::from(&progress.mount_staging_name);
        if progress.mount_observation.is_none() {
            let staged = snapshot_at(parent.directory(), &staging_name, &target.expected_target)?;
            let current = snapshot_at(parent.directory(), &leaf, &target.expected_target)?;
            if progress.mount_create_intent
                && staged.kind() == TargetKind::ExpectedLink
                && current.kind() == TargetKind::Absent
            {
                // Journal 中的预写创建意图与事务私有 UUID 名共同证明该链接属于本事务。
                journal.targets[index].mount_observation = Some(staged.observation().to_owned());
            } else if staged.kind() != TargetKind::Absent {
                return Err(TakeoverError::RecoveryBlocked(
                    "Mount 暂存链接与 Journal 的创建意图不一致".to_owned(),
                ));
            } else if current.kind() == TargetKind::ExpectedLink {
                return Err(TakeoverError::RecoveryBlocked(
                    "最终 Mount 存在，但 Journal 缺少可证明归属的精确 observation".to_owned(),
                ));
            } else {
                journal.targets[index].mount_create_intent = false;
            }
        }
        if let Some(applied_observation) = journal.targets[index].mount_observation.clone() {
            let current = snapshot_at(parent.directory(), &leaf, &target.expected_target)?;
            let staged = snapshot_at(parent.directory(), &staging_name, &target.expected_target)?;
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
                let moved =
                    snapshot_at(parent.directory(), &staging_name, &target.expected_target)?;
                if moved.observation() != applied_observation {
                    return Err(TakeoverError::RecoveryBlocked(
                        "待撤销 Mount 在隔离时发生变化".to_owned(),
                    ));
                }
                unlink_at(parent.directory(), &staging_name, false)
                    .map_err(|source| takeover_io("撤销接管 Mount", parent.path(), source))?;
                sync(parent.directory(), "同步 Mount 撤销", parent.path())?;
                if !first_mount_removal_seen {
                    first_mount_removal_seen = true;
                    inject_takeover_hard_exit(
                        failpoint,
                        LifecycleFailpoint::HardExitAfterFirstTakeoverRollbackMountRemovedBeforeProgress,
                    );
                }
            }
            journal.targets[index].mount_observation = None;
            journal.targets[index].mount_create_intent = false;
        }
        recheck_open_parent(&parent)?;
        write_journal(paths, managed_root, journal)?;
    }
    let mut first_origin_restore_seen = false;
    for (index, origin) in contract.plan.origins.iter().enumerate().rev() {
        let parent = open_origin_parent(paths, storage, origin, &contract.plan.skill_name)?;
        let leaf = origin_leaf(Path::new(&origin.original_path), &contract.plan.skill_name)?;
        let recovery_name = OsString::from(&journal.origins[index].recovery_name);
        let original_observation = journal.origins[index].original_observation.clone();
        if journal.origins[index].recovery_observation.is_none() {
            let current = snapshot_at(parent.directory(), &leaf, "")?;
            let recovery = snapshot_at(parent.directory(), &recovery_name, "")?;
            if current.kind() == TargetKind::Absent
                && recovery.observation() == original_observation
            {
                // 原目录已经完成 rename，但进度写入失败；原始精确快照足以确认其归属。
                journal.origins[index].recovery_observation =
                    Some(recovery.observation().to_owned());
            } else if current.observation() == original_observation
                && recovery.kind() == TargetKind::Absent
            {
                // rename 尚未发生，不需要恢复。
            } else {
                return Err(TakeoverError::RecoveryBlocked(
                    "无法确认原 Skill 在中断窗口中的唯一位置".to_owned(),
                ));
            }
        }
        if let Some(recovery_observation) = journal.origins[index].recovery_observation.clone() {
            let current = snapshot_at(parent.directory(), &leaf, "")?;
            let recovery = snapshot_at(parent.directory(), &recovery_name, "")?;
            if current.observation() == original_observation
                && recovery.kind() == TargetKind::Absent
            {
                // 上次恢复已经完成 rename，只缺最后一次 Journal 进度写入。
                journal.origins[index].recovery_observation = None;
            } else if recovery.observation() != recovery_observation
                || current.kind() != TargetKind::Absent
            {
                return Err(TakeoverError::RecoveryBlocked(
                    "原 Skill 恢复位置已被未知内容替换".to_owned(),
                ));
            } else {
                rename_at_no_replace(
                    parent.directory(),
                    &recovery_name,
                    parent.directory(),
                    &leaf,
                )
                .map_err(|source| takeover_io("恢复原 Skill", parent.path(), source))?;
                sync(parent.directory(), "同步原 Skill 恢复", parent.path())?;
                if !first_origin_restore_seen {
                    first_origin_restore_seen = true;
                    inject_takeover_hard_exit(
                        failpoint,
                        LifecycleFailpoint::HardExitAfterFirstTakeoverRollbackOriginRestoredBeforeProgress,
                    );
                }
                let restored = snapshot_at(parent.directory(), &leaf, "")?;
                if restored.observation() != original_observation {
                    return Err(TakeoverError::RecoveryBlocked(
                        "恢复后的原 Skill 不再是 Plan 中的目录".to_owned(),
                    ));
                }
                journal.origins[index].recovery_observation = None;
            }
        }
        journal.origins[index].cleanup_manifest = None;
        validate_live_origin(paths, storage, origin, &contract.origin_snapshots[index])?;
        recheck_open_parent(&parent)?;
        write_journal(paths, managed_root, journal)?;
    }
    prepare_uncommitted_cleanup(paths, managed_root, contract, journal)?;
    cleanup_uncommitted_takeover_content(paths, managed_root, journal, failpoint)?;
    journal.candidate_cleanup = None;
    write_journal(paths, managed_root, journal)?;
    Ok(())
}

fn cleanup_committed(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &mut Storage,
    contract: &TakeoverPlanContract,
    journal: &mut TakeoverJournal,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverError> {
    verify_takeover_targets(paths, storage, contract, journal)?;
    let mut first_recovery_entry_removed = false;
    for (index, origin) in contract.plan.origins.iter().enumerate() {
        let original_path = Path::new(&origin.original_path);
        let parent = open_origin_parent(paths, storage, origin, &contract.plan.skill_name)?;
        let leaf = origin_leaf(original_path, &contract.plan.skill_name)?;
        let has_final_target = contract
            .plan
            .targets
            .iter()
            .any(|target| target.target_path == origin.original_path);
        if !has_final_target
            && snapshot_at(parent.directory(), &leaf, "")?.kind() != TargetKind::Absent
        {
            return Err(TakeoverError::RecoveryBlocked(
                "最终 Host 位置与接管计划不一致".to_owned(),
            ));
        }
        let recovery_name = OsString::from(&journal.origins[index].recovery_name);
        let recovery_snapshot = snapshot_at(parent.directory(), &recovery_name, "")?;
        if recovery_snapshot.kind() == TargetKind::Absent {
            // state_committed 之后删除旧副本是既定方向；缺失表示该项清理已经完成。
            if journal.origins[index].cleanup_manifest.is_some() {
                sync(parent.directory(), "同步已清理旧副本父目录", parent.path())?;
            }
            journal.origins[index].recovery_observation = None;
            journal.origins[index].cleanup_manifest = None;
            write_journal(paths, managed_root, journal)?;
            recheck_open_parent(&parent)?;
            continue;
        }
        if journal.origins[index].recovery_observation.is_none()
            && recovery_snapshot.observation() == journal.origins[index].original_observation
        {
            journal.origins[index].recovery_observation =
                Some(recovery_snapshot.observation().to_owned());
        }
        if journal.origins[index].cleanup_manifest.is_none()
            && journal.origins[index].recovery_observation.as_deref()
                != Some(recovery_snapshot.observation())
        {
            return Err(TakeoverError::RecoveryBlocked(
                "原 Skill 隔离内容已被未知内容替换".to_owned(),
            ));
        }
        let recovery = parent.path().join(&recovery_name);
        if journal.origins[index].cleanup_manifest.is_none() {
            let validated = validate_single_skill_folder_as(&recovery, &contract.plan.skill_name)?;
            if validated.fingerprint != journal.origins[index].expected_fingerprint {
                return Err(TakeoverError::RecoveryBlocked(
                    "事务恢复内容与 Plan 不一致".to_owned(),
                ));
            }
            let manifest =
                capture_owned_tree_cleanup_manifest(parent.directory(), &recovery_name, &recovery)?;
            let revalidated =
                validate_single_skill_folder_as(&recovery, &contract.plan.skill_name)?;
            if revalidated.fingerprint != journal.origins[index].expected_fingerprint {
                return Err(TakeoverError::RecoveryBlocked(
                    "旧副本在封存清理边界时发生变化".to_owned(),
                ));
            }
            journal.origins[index].cleanup_manifest = Some(manifest);
            write_journal(paths, managed_root, journal)?;
        }
        let cleanup_manifest = journal.origins[index]
            .cleanup_manifest
            .clone()
            .ok_or(TakeoverError::InvalidPlanContract)?;
        let mut after_entry_removed = || {
            if !first_recovery_entry_removed {
                first_recovery_entry_removed = true;
                inject_takeover_hard_exit(
                    failpoint,
                    LifecycleFailpoint::HardExitDuringFirstTakeoverRecoveryRemoval,
                );
            }
        };
        remove_owned_tree_at_with_manifest_and_hook(
            parent.directory(),
            &recovery_name,
            &recovery,
            &cleanup_manifest,
            &mut after_entry_removed,
        )?;
        sync(parent.directory(), "同步已提交旧副本清理", parent.path())?;
        if index == 0 {
            inject_takeover_hard_exit(
                failpoint,
                LifecycleFailpoint::HardExitAfterFirstTakeoverRecoveryRemovedBeforeProgress,
            );
        }
        journal.origins[index].recovery_observation = None;
        journal.origins[index].cleanup_manifest = None;
        write_journal(paths, managed_root, journal)?;
        recheck_open_parent(&parent)?;
    }
    let staging_root =
        open_managed_directory_from_root(paths, managed_root, &paths.staging_root())?;
    if entry_metadata_at(&staging_root, OsStr::new(&journal.transaction_id))
        .map_err(|source| takeover_io("检查接管临时目录", &paths.staging_root(), source))?
        .is_some()
    {
        remove_empty_directory_at(
            &staging_root,
            OsStr::new(&journal.transaction_id),
            &paths.staging_root().join(&journal.transaction_id),
        )?;
    }
    remove_journal_if_present(paths, managed_root, &journal.transaction_id)?;
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverJournalRemovedBeforeForget,
    );
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
    let project = origin
        .project_id
        .as_deref()
        .map(|project_id| storage.read_project(project_id))
        .transpose()?;
    open_resolved_origin_parent(
        paths,
        origin.app_id,
        origin.scope,
        project.as_ref(),
        Path::new(&origin.original_path),
        skill_name,
    )
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
    let bytes = takeover_journal_bytes(journal)?;
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let name = journal_file_name(&journal.transaction_id);
    write_atomic_at(&journals, &name, &paths.journals_root().join(&name), &bytes)?;
    Ok(())
}

fn write_initial_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &TakeoverJournal,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverError> {
    let bytes = takeover_journal_bytes(journal)?;
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let name = journal_file_name(&journal.transaction_id);
    let mut after_temp_sync = || {
        inject_takeover_hard_exit(
            failpoint,
            LifecycleFailpoint::HardExitAfterTakeoverJournalTempSyncedBeforeRename,
        );
    };
    write_atomic_at_with_after_temp_sync(
        &journals,
        &name,
        &paths.journals_root().join(&name),
        &bytes,
        &mut after_temp_sync,
    )?;
    Ok(())
}

fn takeover_journal_bytes(journal: &TakeoverJournal) -> Result<Vec<u8>, TakeoverError> {
    let bytes = serde_json::to_vec(journal)?;
    if bytes.len() > MAX_TAKEOVER_JOURNAL_BYTES {
        return Err(TakeoverError::JournalTooLarge);
    }
    Ok(bytes)
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

fn prepare_uncommitted_cleanup(
    paths: &ApplicationPaths,
    managed_root: &File,
    contract: &TakeoverPlanContract,
    journal: &mut TakeoverJournal,
) -> Result<(), TakeoverError> {
    if journal.candidate_cleanup.is_some() {
        return Ok(());
    }
    let published = capture_published_candidate_cleanup(paths, managed_root, contract, journal)?;
    let staging = capture_staging_candidate_cleanup(paths, managed_root, contract, journal)?;
    if published.is_some() && staging.is_some() {
        return Err(TakeoverError::RecoveryBlocked(
            "待清理候选同时出现在发布区和临时区".to_owned(),
        ));
    }
    journal.candidate_cleanup = Some(TakeoverCandidateCleanupIntent {
        tree: published.or(staging),
    });
    write_journal(paths, managed_root, journal)
}

fn capture_published_candidate_cleanup(
    paths: &ApplicationPaths,
    managed_root: &File,
    contract: &TakeoverPlanContract,
    journal: &TakeoverJournal,
) -> Result<Option<TakeoverCandidateCleanupTree>, TakeoverError> {
    let bundles_path = paths.bundles_root();
    let bundles = open_managed_directory_from_root(paths, managed_root, &bundles_path)?;
    let bundle_path = paths.bundle_directory(&journal.bundle_id);
    let bundle = match open_directory_at(&bundles, OsStr::new(&journal.bundle_id)) {
        Ok(bundle) => bundle,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(takeover_io("安全打开待清理 Bundle", &bundle_path, source));
        }
    };
    ensure_open_directory_matches_managed_path(paths, &bundle, &bundle_path)?;
    let contents_path = bundle_path.join("contents");
    let contents = match open_directory_at(&bundle, OsStr::new("contents")) {
        Ok(contents) => contents,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(takeover_io(
                "安全打开待清理 contents",
                &contents_path,
                source,
            ));
        }
    };
    ensure_open_directory_matches_managed_path(paths, &contents, &contents_path)?;
    let content_path = contents_path.join(&journal.content_id);
    if entry_metadata_at(&contents, OsStr::new(&journal.content_id))
        .map_err(|source| takeover_io("检查待清理候选内容", &content_path, source))?
        .is_none()
    {
        return Ok(None);
    }
    // 已发布候选在进入可重入删除前必须仍是 Plan 选中的完整内容。
    validate_published_content(paths, &contents, &content_path, contract, journal)?;
    let manifest = capture_owned_tree_cleanup_manifest(
        &contents,
        OsStr::new(&journal.content_id),
        &content_path,
    )?;
    validate_published_content(paths, &contents, &content_path, contract, journal)?;
    Ok(Some(TakeoverCandidateCleanupTree {
        location: TakeoverCandidateCleanupLocation::PublishedContent,
        manifest,
    }))
}

fn capture_staging_candidate_cleanup(
    paths: &ApplicationPaths,
    managed_root: &File,
    contract: &TakeoverPlanContract,
    journal: &TakeoverJournal,
) -> Result<Option<TakeoverCandidateCleanupTree>, TakeoverError> {
    let staging_root_path = paths.staging_root();
    let staging_root = open_managed_directory_from_root(paths, managed_root, &staging_root_path)?;
    let staging_path = staging_root_path.join(&journal.transaction_id);
    let staging = match open_directory_at(&staging_root, OsStr::new(&journal.transaction_id)) {
        Ok(staging) => staging,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
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
        .is_none()
    {
        return Ok(None);
    }
    validate_staging_candidate_boundary(paths, &staging, &candidate_path, contract)?;
    Ok(Some(TakeoverCandidateCleanupTree {
        location: TakeoverCandidateCleanupLocation::StagingCandidate,
        manifest: capture_owned_tree_cleanup_manifest(
            &staging,
            OsStr::new("candidate"),
            &candidate_path,
        )?,
    }))
}

fn validate_staging_candidate_boundary(
    paths: &ApplicationPaths,
    staging: &File,
    candidate_path: &Path,
    contract: &TakeoverPlanContract,
) -> Result<(), TakeoverError> {
    let candidate = open_expected_directory_at(staging, OsStr::new("candidate"), candidate_path)?;
    ensure_open_directory_matches_managed_path(paths, &candidate, candidate_path)?;
    ensure_only_entries(
        &read_entry_names_from_handle(&candidate)?,
        &["members"],
        "待清理 candidate 包含未知条目",
    )?;
    let members_path = candidate_path.join("members");
    let members = match open_directory_at(&candidate, OsStr::new("members")) {
        Ok(members) => members,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(takeover_io(
                "安全打开待清理 candidate members",
                &members_path,
                source,
            ));
        }
    };
    ensure_open_directory_matches_managed_path(paths, &members, &members_path)?;
    ensure_only_entries(
        &read_entry_names_from_handle(&members)?,
        &[contract.plan.skill_name.as_str()],
        "待清理 candidate members 包含未知成员",
    )
}

fn cleanup_uncommitted_takeover_content(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &TakeoverJournal,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverError> {
    if journal.candidate_cleanup.is_none() {
        return Err(TakeoverError::InvalidPlanContract);
    }
    let mut first_entry_removed = false;
    let mut after_entry_removed = || {
        if !first_entry_removed {
            first_entry_removed = true;
            inject_takeover_hard_exit(
                failpoint,
                LifecycleFailpoint::HardExitDuringTakeoverCandidateCleanup,
            );
        }
    };
    cleanup_uncommitted_bundle(paths, managed_root, journal, &mut after_entry_removed)?;
    cleanup_uncommitted_staging(paths, managed_root, journal, &mut after_entry_removed)
}

fn cleanup_uncommitted_bundle(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &TakeoverJournal,
    after_entry_removed: &mut impl FnMut(),
) -> Result<(), TakeoverError> {
    let cleanup_tree = candidate_cleanup_tree(journal)?;
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
    let contents_exists = entry_metadata_at(&bundle, OsStr::new("contents"))
        .map_err(|source| takeover_io("检查待清理 contents", &contents_path, source))?
        .is_some();
    let _current_exists = validate_optional_current(
        &bundle,
        OsStr::new("current"),
        &Path::new("contents").join(&journal.content_id),
        &bundle_path.join("current"),
    )?;
    let _temporary_exists = validate_optional_current(
        &bundle,
        OsStr::new(&temporary_current),
        &Path::new("contents").join(&journal.content_id),
        &bundle_path.join(&temporary_current),
    )?;
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
    if contents_exists {
        let contents = open_expected_directory_at(&bundle, OsStr::new("contents"), &contents_path)?;
        ensure_open_directory_matches_managed_path(paths, &contents, &contents_path)?;
        ensure_only_entries(
            &read_entry_names_from_handle(&contents)?,
            &[journal.content_id.as_str()],
            "待清理 contents 包含未知内容",
        )?;
        let candidate_exists = entry_metadata_at(&contents, OsStr::new(&journal.content_id))
            .map_err(|source| takeover_io("检查待清理候选内容", &content_path, source))?
            .is_some();
        if candidate_exists {
            let Some(tree) = cleanup_tree
                .filter(|tree| tree.location == TakeoverCandidateCleanupLocation::PublishedContent)
            else {
                return Err(TakeoverError::RecoveryBlocked(
                    "发布区出现了未列入清理计划的候选内容".to_owned(),
                ));
            };
            remove_owned_tree_at_with_manifest_and_hook(
                &contents,
                OsStr::new(&journal.content_id),
                &content_path,
                &tree.manifest,
                after_entry_removed,
            )?;
        }
        drop(contents);
        remove_empty_directory_at(&bundle, OsStr::new("contents"), &contents_path)?;
    }
    drop(bundle);
    remove_empty_directory_at(&bundles, OsStr::new(&journal.bundle_id), &bundle_path)
        .map_err(Into::into)
}

fn candidate_cleanup_tree(
    journal: &TakeoverJournal,
) -> Result<Option<&TakeoverCandidateCleanupTree>, TakeoverError> {
    journal
        .candidate_cleanup
        .as_ref()
        .map(|intent| intent.tree.as_ref())
        .ok_or(TakeoverError::InvalidPlanContract)
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
    journal: &TakeoverJournal,
    after_entry_removed: &mut impl FnMut(),
) -> Result<(), TakeoverError> {
    let cleanup_tree = candidate_cleanup_tree(journal)?;
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
        let Some(tree) = cleanup_tree
            .filter(|tree| tree.location == TakeoverCandidateCleanupLocation::StagingCandidate)
        else {
            return Err(TakeoverError::RecoveryBlocked(
                "临时区出现了未列入清理计划的候选内容".to_owned(),
            ));
        };
        remove_owned_tree_at_with_manifest_and_hook(
            &staging,
            OsStr::new("candidate"),
            &candidate_path,
            &tree.manifest,
            after_entry_removed,
        )?;
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

fn inject_takeover_hard_exit(actual: LifecycleFailpoint, expected: LifecycleFailpoint) {
    if actual == expected {
        // 独立子进程必须跳过析构，才能覆盖真实应用退出后的持久化恢复。
        unsafe { libc::_exit(93) }
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
        || request
            .shared_targets
            .iter()
            .any(|target| !observation_ids.contains(&target.shared_observation_id))
        || request
            .shared_targets
            .iter()
            .enumerate()
            .any(|(index, target)| {
                request.shared_targets[..index].iter().any(|existing| {
                    existing.shared_observation_id == target.shared_observation_id
                        && existing.app_id == target.app_id
                })
            })
    {
        return Err(TakeoverError::InvalidRequest(
            "接管计划中的副本、内容选择或保留位置无效".to_owned(),
        ));
    }
    Ok(())
}

fn resolve_observation_location(
    storage: &Storage,
    app_configs: &[crate::paths::SupportedAppPathConfig],
    observation: &InventoryItem,
) -> Result<ResolvedOriginLocation, TakeoverError> {
    let root_key = observation
        .root_key
        .ok_or_else(|| TakeoverError::InvalidRequest("扫描观察缺少受支持路径信息".to_owned()))?;
    if let Some(app) = app_configs.iter().find(|app| app.root_key == root_key) {
        if observation.project_id.is_some() {
            return Err(TakeoverError::InvalidRequest(
                "global 扫描观察不能绑定 Project".to_owned(),
            ));
        }
        return Ok(ResolvedOriginLocation {
            app_id: Some(app.id),
            scope: Some(MountScope::Global),
            project: None,
        });
    }
    if let Some(app) = app_configs
        .iter()
        .find(|app| app.project_root_key == root_key)
    {
        let project = read_observation_project(storage, observation)?;
        return Ok(ResolvedOriginLocation {
            app_id: Some(app.id),
            scope: Some(MountScope::Project),
            project: Some(project),
        });
    }
    match root_key {
        ScanRootKey::SharedAgents if observation.project_id.is_none() => {
            Ok(ResolvedOriginLocation {
                app_id: None,
                scope: None,
                project: None,
            })
        }
        ScanRootKey::SharedAgentsProject => Ok(ResolvedOriginLocation {
            app_id: None,
            scope: None,
            project: Some(read_observation_project(storage, observation)?),
        }),
        _ => Err(TakeoverError::InvalidRequest(
            "扫描观察不属于受支持的接管路径".to_owned(),
        )),
    }
}

fn read_observation_project(
    storage: &Storage,
    observation: &InventoryItem,
) -> Result<StoredProject, TakeoverError> {
    let project_id = observation
        .project_id
        .as_deref()
        .ok_or_else(|| TakeoverError::InvalidRequest("Project 扫描观察缺少 Project".to_owned()))?;
    let project = storage.read_project(project_id)?;
    if observation.project_display_name.as_deref() != Some(project.display_name.as_str()) {
        return Err(TakeoverError::InvalidRequest(
            "扫描观察绑定的 Project 已经变化".to_owned(),
        ));
    }
    Ok(project)
}

fn open_resolved_origin_parent(
    paths: &ApplicationPaths,
    app_id: Option<SupportedAppId>,
    scope: Option<MountScope>,
    project: Option<&StoredProject>,
    original_root: &Path,
    skill_name: &str,
) -> Result<OpenMountParent, TakeoverError> {
    let lookup = match (app_id, scope, project) {
        (Some(app_id), Some(scope), project) => {
            open_mount_parent(paths, app_id, scope, project, false)?
        }
        (None, None, Some(project)) => {
            open_project_relative_parent(project, Path::new(".agents/skills"), false)?
        }
        (None, None, None) => {
            open_relative_parent(paths.home(), Path::new(".agents/skills"), false)?
        }
        _ => return Err(TakeoverError::InvalidPlanContract),
    };
    let ParentLookup::Open(parent) = lookup else {
        return Err(TakeoverError::InvalidRequest(
            "扫描到的 Host Skill 父目录已经不存在".to_owned(),
        ));
    };
    ensure_origin_parent_matches(&parent, original_root, skill_name)?;
    Ok(parent)
}

fn snapshot_new_target(
    paths: &ApplicationPaths,
    app_id: SupportedAppId,
    scope: MountScope,
    project: Option<&StoredProject>,
    skill_name: &str,
    target_path: &Path,
    expected_target: &str,
) -> Result<(TargetKind, String), TakeoverError> {
    match open_mount_parent(paths, app_id, scope, project, false)? {
        ParentLookup::Missing => Ok((TargetKind::Absent, "absent".to_owned())),
        ParentLookup::Open(parent) => {
            if parent.path().join(skill_name) != target_path {
                return Err(TakeoverError::InvalidPlanContract);
            }
            let snapshot =
                snapshot_at(parent.directory(), OsStr::new(skill_name), expected_target)?;
            recheck_open_parent(&parent)?;
            Ok((snapshot.kind(), snapshot.observation().to_owned()))
        }
    }
}

fn project_snapshot(project: &StoredProject) -> TakeoverProjectSnapshot {
    TakeoverProjectSnapshot {
        id: project.id.clone(),
        display_name: project.display_name.clone(),
        root_path: project.root_path.clone(),
        root_device: project.root_device,
        root_inode: project.root_inode,
    }
}

fn validate_scope_topology(targets: &[TakeoverPlanTarget]) -> Result<(), TakeoverError> {
    for app_id in [
        SupportedAppId::Codex,
        SupportedAppId::ClaudeCode,
        SupportedAppId::GitHubCopilot,
    ] {
        let has_global = targets
            .iter()
            .any(|target| target.app_id == app_id && target.scope == MountScope::Global);
        let has_project = targets
            .iter()
            .any(|target| target.app_id == app_id && target.scope == MountScope::Project);
        if has_global && has_project {
            return Err(TakeoverError::InvalidRequest(
                "同一 Skill 与 Supported App 不能同时保留 global 和 project Mount".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_scope_resolution(
    origins: &[TakeoverPlanOrigin],
    targets: &[TakeoverPlanTarget],
) -> Result<(), TakeoverError> {
    validate_scope_topology(targets)?;
    for app_id in [
        SupportedAppId::Codex,
        SupportedAppId::ClaudeCode,
        SupportedAppId::GitHubCopilot,
    ] {
        let has_global_origin = origins.iter().any(|origin| {
            origin.app_id == Some(app_id) && origin.scope == Some(MountScope::Global)
        });
        let has_project_origin = origins.iter().any(|origin| {
            origin.app_id == Some(app_id) && origin.scope == Some(MountScope::Project)
        });
        let keeps_scope = targets.iter().any(|target| target.app_id == app_id);
        if has_global_origin && has_project_origin && !keeps_scope {
            return Err(TakeoverError::InvalidRequest(
                "同一应用存在 global/project 冲突时必须保留其中一种 scope".to_owned(),
            ));
        }
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

fn ensure_takeover_origins_are_writable(
    storage: &Storage,
    observations: &[InventoryItem],
) -> Result<(), TakeoverError> {
    let origin_paths = observations
        .iter()
        .map(|observation| observation.skill_root.clone())
        .collect::<BTreeSet<_>>();
    ensure_takeover_origin_paths_are_writable(storage, &origin_paths)
}

fn ensure_takeover_plan_origins_are_writable(
    storage: &Storage,
    plan: &TakeoverPlan,
) -> Result<(), TakeoverError> {
    let origin_paths = plan
        .origins
        .iter()
        .map(|origin| origin.original_path.clone())
        .collect::<BTreeSet<_>>();
    ensure_takeover_origin_paths_are_writable(storage, &origin_paths)
}

fn ensure_takeover_origin_paths_are_writable(
    storage: &Storage,
    origin_paths: &BTreeSet<String>,
) -> Result<(), TakeoverError> {
    for transaction in storage
        .recoverable_takeover_transactions()?
        .into_iter()
        .filter(|transaction| transaction.status == "blocked")
    {
        let overlaps = transaction
            .reserved_paths
            .iter()
            .any(|path| origin_paths.contains(path));
        if overlaps {
            return Err(TakeoverError::RecoveryBlocked(
                "这个 Skill 还有一项未解决的接管恢复，暂时不能再次接管".to_owned(),
            ));
        }
    }
    Ok(())
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
