use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    content::{
        BundleCopyBudget, ContentValidationError, copy_single_skill_tree_into_open_directory,
        validate_single_skill_folder,
    },
    domain::{
        InventoryLocationKind, ManagementKind, MountScope, ScanRootKey, SkillMetadataStatus,
        SupportedAppId, TakeoverPlan, TakeoverPlanPath,
    },
    git_management_evidence::{ManagementEvidenceInspection, inspect_git_head_management},
    lifecycle::{
        LifecycleError, LifecycleFailpoint, LifecycleLock, acquire_lifecycle_lock,
        entry_metadata_at, mkdir_at, open_directory_at, open_managed_directory_from_root,
        open_regular_file_at, read_entry_names_os_from_handle, read_link_at, remove_owned_tree_at,
        rename_at_no_replace, rename_at_swap, symlink_at, unlink_at, write_atomic_at,
        write_notice_from_storage,
    },
    paths::{ApplicationPaths, SupportedAppPathConfig},
    scanner::{ScanError, fingerprint_skill_root},
    storage::{
        Storage, StorageError, StoredProject, StoredTakeoverPlan, StoredTakeoverTransaction,
        takeover_plan_seal,
    },
};

const TAKEOVER_PLAN_TTL_MILLIS: i64 = 30 * 60 * 1_000;
const TAKEOVER_JOURNAL_VERSION: u32 = 1;
const MAX_TAKEOVER_JOURNAL_BYTES: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum TakeoverLifecycleError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Content(#[from] ContentValidationError),
    #[error("无法检查接管路径 {path}：{source}")]
    InspectPath {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Inventory observation 已变化，请先刷新本机清单：{0}")]
    ObservationChanged(String),
    #[error("该 Skill 不符合普通已有安装的接管条件：{0}")]
    Ineligible(&'static str),
    #[error("已有安装不在 Supported App 的固定 Skill 叶子中：{0}")]
    UnsafeLocation(String),
    #[error("已登记 Project 目录已经变化：{0}")]
    ProjectChanged(String),
    #[error("Project 的 Git 管理状态无法可靠确认：{0}")]
    ProjectManagementIndeterminate(String),
    #[error("接管路径不能无损保存为 UTF-8：{0}")]
    NonUnicodePath(String),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error("接管路径与 Central Store 不在同一文件系统，1.0 不能安全接管")]
    CrossDevice,
    #[error("接管 Plan 的文件系统前置状态已经变化，请重新生成 Plan")]
    PlanPreconditionChanged,
    #[error("接管事务 Journal 无法解析：{0}")]
    InvalidJournal(#[from] serde_json::Error),
    #[error("接管事务 Journal 超过安全大小限制（{actual} 字节，限制 {limit} 字节）")]
    JournalTooLarge { actual: usize, limit: usize },
    #[error("接管事务恢复需要人工处理：{0}")]
    RecoveryBlocked(String),
    #[error("测试模拟接管中断：{0}")]
    SimulatedInterruption(&'static str),
}

struct TakeoverLocation {
    app_id: SupportedAppId,
    scope: MountScope,
    project: Option<StoredProject>,
    base: PathBuf,
    parent: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TakeoverJournalPhase {
    JournalReady,
    CandidateReady,
    ReplacementStaged,
    HostSwapped,
    StateCommitted,
    OriginalDiscarded,
}

impl TakeoverJournalPhase {
    fn as_storage_str(self) -> &'static str {
        match self {
            Self::JournalReady => "journal_ready",
            Self::CandidateReady => "candidate_ready",
            Self::ReplacementStaged => "replacement_staged",
            Self::HostSwapped => "host_swapped",
            Self::StateCommitted => "state_committed",
            Self::OriginalDiscarded => "original_discarded",
        }
    }
}

/// Journal 只记录由 SQLite Plan 重建后仍需核对的文件系统边界。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TakeoverJournal {
    version: u32,
    transaction_id: String,
    plan_id: String,
    bundle_id: String,
    content_id: String,
    member_id: String,
    path_id: String,
    skill_name: String,
    content_fingerprint: String,
    preserve_mount: bool,
    phase: TakeoverJournalPhase,
    staging_relative: String,
    bundle_relative: String,
    content_relative: String,
    current_target: String,
    host_parent: String,
    host_name: String,
    hidden_name: String,
    expected_target: String,
    parent_device: u64,
    parent_inode: u64,
    parent_mode: u32,
    original_device: u64,
    original_inode: u64,
    original_mode: u32,
    original_entries: Vec<TakeoverOriginalEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TakeoverOriginalEntryKind {
    Directory,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TakeoverOriginalEntry {
    /// 使用原始 Unix 路径字节的 hex，避免 Journal 因非 UTF-8 文件名失真。
    relative_path_hex: String,
    kind: TakeoverOriginalEntryKind,
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    content_sha256: Option<String>,
}

struct OpenTakeoverParent {
    handle: File,
    path: PathBuf,
}

/// 本片只签发只读 Plan；不会创建 Bundle、复制内容或替换 Host 路径。
pub fn create_takeover_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    observation_id: &str,
    now: i64,
) -> Result<TakeoverPlan, TakeoverLifecycleError> {
    let observation = storage.read_inventory_observation(observation_id)?;
    validate_inventory_eligibility(&observation)?;
    let location = derive_takeover_location(paths, storage, &observation)?;
    let expected_observers = expected_observers(location.app_id, location.scope);
    if observation.observed_by != expected_observers {
        return Err(TakeoverLifecycleError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }

    ensure_single_component(&observation.skill_name)?;
    let expected_original = location.parent.join(&observation.skill_name);
    let original = Path::new(&observation.skill_root);
    if original != expected_original
        || Path::new(&observation.skill_file) != expected_original.join("SKILL.md")
    {
        return Err(TakeoverLifecycleError::UnsafeLocation(
            observation.skill_root.clone(),
        ));
    }
    validate_project_management_snapshot(location.project.as_ref(), &observation.skill_file)?;
    let parent_before = inspect_directory_chain(&location.base, &location.parent)?;
    let original_before = inspect_real_directory(original)?;
    let current_observed_fingerprint = fingerprint_skill_root(original)
        .map_err(|error| map_scan_error(error, &observation.skill_root))?;
    if current_observed_fingerprint != observation.observed_fingerprint {
        return Err(TakeoverLifecycleError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }
    let validated = validate_single_skill_folder(original)?;
    if validated.name != observation.skill_name || validated.fingerprint.len() != 64 {
        return Err(TakeoverLifecycleError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }
    let parent_after = inspect_directory_chain(&location.base, &location.parent)?;
    let original_after = inspect_real_directory(original)?;
    if filesystem_identity(&parent_before) != filesystem_identity(&parent_after)
        || filesystem_identity(&original_before) != filesystem_identity(&original_after)
    {
        return Err(TakeoverLifecycleError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }

    let bundle_id = Uuid::new_v4().to_string();
    let content_id = Uuid::new_v4().to_string();
    let member_id = Uuid::new_v4().to_string();
    let managed_directory = paths.bundle_directory(&bundle_id);
    let content_directory = managed_directory.join("contents").join(&content_id);
    let expected_target = managed_directory
        .join("current/members")
        .join(&validated.name);
    let project = location.project.as_ref();
    let mut stored = StoredTakeoverPlan {
        plan: TakeoverPlan {
            id: String::new(),
            observation_id: observation.id.clone(),
            bundle_id,
            content_id,
            member_id,
            bundle_display_name: validated.name.clone(),
            source_display_name: None,
            source_notice: "来源未知；没有更新来源".to_owned(),
            skill_name: validated.name,
            skill_description: validated.description,
            content_fingerprint: validated.fingerprint,
            warnings: validated.warnings,
            managed_directory: path_to_string(&managed_directory)?,
            content_directory: path_to_string(&content_directory)?,
            expected_target: path_to_string(&expected_target)?,
            paths: vec![TakeoverPlanPath {
                id: Uuid::new_v4().to_string(),
                mount_id: Uuid::new_v4().to_string(),
                original_path: observation.skill_root.clone(),
                app_id: location.app_id,
                scope: location.scope,
                project_id: project.map(|value| value.id.clone()),
                project_display_name: project.map(|value| value.display_name.clone()),
                project_root_path: project.map(|value| value.root_path.clone()),
                project_root_device: project.map(|value| value.root_device),
                project_root_inode: project.map(|value| value.root_inode),
                parent_device: parent_after.dev(),
                parent_inode: parent_after.ino(),
                parent_mode: parent_after.mode(),
                original_device: original_after.dev(),
                original_inode: original_after.ino(),
                original_mode: original_after.mode(),
                default_preserve_mount: true,
            }],
            created_at: now,
            expires_at: now.saturating_add(TAKEOVER_PLAN_TTL_MILLIS),
        },
        observation,
        status: "pending".to_owned(),
    };
    verify_final_snapshot(
        &location,
        &stored.observation,
        &stored.plan,
        filesystem_identity(&parent_after),
        filesystem_identity(&original_after),
    )?;
    let seal = takeover_plan_seal(&stored);
    stored.plan.id = format!("takeover-{}-{seal}", Uuid::new_v4());
    Ok(storage.save_takeover_plan(&stored)?.plan)
}

/// 确认接管只暴露给后端 Application seam；Tauri command 由后续 UI 切片决定。
pub fn confirm_takeover_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
    preserved_path_ids: &[String],
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverLifecycleError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    let preview = storage.read_takeover_plan(plan_id)?;
    let transaction_id = Uuid::new_v4().to_string();
    let journal_relative = format!("journals/{transaction_id}.json");
    let preserve_mount = selected_preserve_mount(&preview, preserved_path_ids)?;
    let mut journal = build_takeover_journal(&transaction_id, &preview, preserve_mount)?;
    ensure_takeover_journal_fits(&journal)?;
    let journal_contract_sha256 = takeover_journal_contract_sha256(&journal)?;
    validate_takeover_execution_snapshot(paths, storage, &lifecycle_lock, &preview, &journal)?;

    let plan = storage.begin_takeover_transaction_with_journal_contract(
        plan_id,
        preserved_path_ids,
        &transaction_id,
        &journal_relative,
        &journal_contract_sha256,
        now,
    )?;
    let mut expected = preview.clone();
    expected.status = "consumed".to_owned();
    if plan != expected {
        storage.abort_takeover_transaction(
            &transaction_id,
            Some("SQLite 中的接管 Plan 与确认预览不一致"),
            now,
        )?;
        storage.forget_terminal_takeover_transaction(&transaction_id)?;
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    // begin 与首次文件写入之间仍要重验外部状态，失败时不会留下半份 Central 内容。
    if let Err(error) =
        validate_takeover_execution_snapshot(paths, storage, &lifecycle_lock, &plan, &journal)
    {
        storage.abort_takeover_transaction(&transaction_id, Some(&error.to_string()), now)?;
        storage.forget_terminal_takeover_transaction(&transaction_id)?;
        return Err(error);
    }
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverTransactionRecord,
    );
    if failpoint == LifecycleFailpoint::AfterTakeoverTransactionRecord {
        return Err(TakeoverLifecycleError::SimulatedInterruption(
            "事务记录已提交，Journal 尚未写入",
        ));
    }

    if let Err(error) = execute_takeover(
        paths,
        &lifecycle_lock,
        storage,
        &plan,
        &mut journal,
        now,
        failpoint,
    ) {
        if matches!(error, TakeoverLifecycleError::SimulatedInterruption(_)) {
            return Err(error);
        }
        // 恢复器接入前宁可阻塞相关事务，也不能凭错误类型猜测 Host 是否已经生效。
        storage.block_takeover_transaction(&transaction_id, &error.to_string(), now)?;
        return Err(error);
    }
    cleanup_completed_takeover(paths, &lifecycle_lock, storage, &journal)?;
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

/// 启动恢复以实际文件系统状态为准；单个异常事务只进入 blocked，不影响其他 Skill。
pub fn recover_pending_takeover_transactions(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    now: i64,
) -> Result<(), TakeoverLifecycleError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    for transaction in storage.recoverable_takeover_transactions()? {
        if transaction.status == "blocked" {
            continue;
        }
        if let Err(error) =
            recover_takeover_transaction(paths, &lifecycle_lock, storage, &transaction, now)
        {
            storage.block_takeover_transaction(&transaction.id, &error.to_string(), now)?;
        }
        lifecycle_lock.recheck(paths)?;
    }
    write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
    Ok(())
}

fn recover_takeover_transaction(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transaction: &StoredTakeoverTransaction,
    now: i64,
) -> Result<(), TakeoverLifecycleError> {
    lifecycle_lock.recheck(paths)?;
    let plan = storage.read_takeover_plan(&transaction.plan_id)?;
    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    let journal_name = OsString::from(format!("{}.json", transaction.id));
    let journal_path = paths.journals_root().join(&journal_name);
    let journal_metadata = entry_metadata_at(&journals, &journal_name)
        .map_err(|source| takeover_io("检查接管 Journal", &journal_path, source))?;

    let journal = if journal_metadata.is_some() {
        let journal = read_takeover_journal_at(&journals, &journal_name, &journal_path)?;
        validate_takeover_journal_contract(&journal, transaction, &plan)?;
        journal
    } else {
        return recover_takeover_without_journal(
            paths,
            lifecycle_lock,
            storage,
            transaction,
            &plan,
            now,
        );
    };

    if transaction.status == "aborted" {
        cleanup_before_takeover_effect(paths, lifecycle_lock, storage, &plan, &journal)?;
        remove_takeover_journal(paths, lifecycle_lock.root(), &journal)?;
        storage.forget_terminal_takeover_transaction(&transaction.id)?;
        return Ok(());
    }
    if transaction.status != "in_progress"
        || !matches!(
            transaction.phase.as_str(),
            "journal_pending" | "journal_ready" | "candidate_ready" | "replacement_staged"
        )
    {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "接管事务已越过 Host 生效点，当前恢复切片不能处理".to_owned(),
        ));
    }
    cleanup_before_takeover_effect(paths, lifecycle_lock, storage, &plan, &journal)?;
    storage.abort_takeover_transaction(&transaction.id, None, now)?;
    remove_takeover_journal(paths, lifecycle_lock.root(), &journal)?;
    storage.forget_terminal_takeover_transaction(&transaction.id)?;
    Ok(())
}

fn recover_takeover_without_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transaction: &StoredTakeoverTransaction,
    plan: &StoredTakeoverPlan,
    now: i64,
) -> Result<(), TakeoverLifecycleError> {
    if transaction.status != "in_progress" || transaction.phase != "journal_pending" {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "接管 Journal 缺失且事务已进入文件系统阶段".to_owned(),
        ));
    }
    // Journal 尚未落盘时，重新采集未触碰的原目录；只有 seal 完全一致才能安全终止。
    let journal = build_takeover_journal(&transaction.id, plan, transaction.preserve_mount)?;
    validate_takeover_journal_seal(&journal, transaction)?;
    validate_takeover_execution_snapshot(paths, storage, lifecycle_lock, plan, &journal)?;
    storage.abort_takeover_transaction(&transaction.id, None, now)?;
    storage.forget_terminal_takeover_transaction(&transaction.id)?;
    Ok(())
}

fn read_takeover_journal_at(
    journals: &File,
    name: &OsStr,
    path: &Path,
) -> Result<TakeoverJournal, TakeoverLifecycleError> {
    let mut file = open_regular_file_at(journals, name, path, false)?;
    let metadata = file
        .metadata()
        .map_err(|source| takeover_io("检查接管 Journal", path, source))?;
    if metadata.len() > MAX_TAKEOVER_JOURNAL_BYTES as u64 {
        return Err(TakeoverLifecycleError::JournalTooLarge {
            actual: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
            limit: MAX_TAKEOVER_JOURNAL_BYTES,
        });
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_TAKEOVER_JOURNAL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| takeover_io("读取接管 Journal", path, source))?;
    if bytes.len() > MAX_TAKEOVER_JOURNAL_BYTES {
        return Err(TakeoverLifecycleError::JournalTooLarge {
            actual: bytes.len(),
            limit: MAX_TAKEOVER_JOURNAL_BYTES,
        });
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn validate_takeover_journal_contract(
    actual: &TakeoverJournal,
    transaction: &StoredTakeoverTransaction,
    plan: &StoredTakeoverPlan,
) -> Result<(), TakeoverLifecycleError> {
    let mut expected = build_takeover_journal_with_entries(
        &transaction.id,
        plan,
        transaction.preserve_mount,
        actual.original_entries.clone(),
    )?;
    expected.phase = actual.phase;
    if actual != &expected {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "SQLite、Plan 与接管 Journal 的事务边界不一致".to_owned(),
        ));
    }
    validate_takeover_journal_seal(actual, transaction)
}

fn validate_takeover_journal_seal(
    journal: &TakeoverJournal,
    transaction: &StoredTakeoverTransaction,
) -> Result<(), TakeoverLifecycleError> {
    let expected = transaction
        .journal_contract_sha256
        .as_deref()
        .ok_or_else(|| {
            TakeoverLifecycleError::RecoveryBlocked("接管事务缺少 Journal contract seal".to_owned())
        })?;
    if takeover_journal_contract_sha256(journal)? != expected {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "接管 Journal 与 SQLite seal 不一致".to_owned(),
        ));
    }
    Ok(())
}

fn execute_takeover(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    plan: &StoredTakeoverPlan,
    journal: &mut TakeoverJournal,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverLifecycleError> {
    lifecycle_lock.recheck(paths)?;
    write_takeover_journal(paths, lifecycle_lock.root(), journal)?;
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverJournalWrittenBeforePhase,
    );
    storage.update_takeover_transaction_phase(
        &journal.transaction_id,
        journal.phase.as_storage_str(),
        now,
    )?;

    prepare_takeover_candidate(paths, lifecycle_lock, plan, journal)?;
    journal.phase = TakeoverJournalPhase::CandidateReady;
    write_takeover_journal(paths, lifecycle_lock.root(), journal)?;
    storage.update_takeover_transaction_phase(
        &journal.transaction_id,
        journal.phase.as_storage_str(),
        now,
    )?;
    inject_takeover_interruption(
        failpoint,
        LifecycleFailpoint::AfterTakeoverCandidatePrepared,
        "接管候选已准备，Host 尚未变化",
    )?;

    publish_takeover_candidate(paths, lifecycle_lock, plan, journal)?;
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverCandidatePublishedBeforePhase,
    );
    stage_host_replacement(paths, storage, lifecycle_lock, plan, journal)?;
    journal.phase = TakeoverJournalPhase::ReplacementStaged;
    write_takeover_journal(paths, lifecycle_lock.root(), journal)?;
    storage.update_takeover_transaction_phase(
        &journal.transaction_id,
        journal.phase.as_storage_str(),
        now,
    )?;
    inject_takeover_interruption(
        failpoint,
        LifecycleFailpoint::AfterTakeoverReplacementStaged,
        "接管替换项已准备，Host 尚未生效",
    )?;
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverReplacementStaged,
    );

    apply_host_takeover(paths, storage, lifecycle_lock, plan, journal)?;
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverHostSwappedBeforePhase,
    );
    journal.phase = TakeoverJournalPhase::HostSwapped;
    write_takeover_journal(paths, lifecycle_lock.root(), journal)?;
    storage.update_takeover_transaction_phase(
        &journal.transaction_id,
        journal.phase.as_storage_str(),
        now,
    )?;
    inject_takeover_interruption(
        failpoint,
        LifecycleFailpoint::AfterTakeoverHostSwapped,
        "Host 已切换，领域状态尚未完成",
    )?;

    validate_takeover_effect(paths, storage, lifecycle_lock, plan, journal)?;
    storage.finalize_takeover(&journal.transaction_id, plan, now)?;
    write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverStateCommittedBeforeJournal,
    );
    journal.phase = TakeoverJournalPhase::StateCommitted;
    write_takeover_journal(paths, lifecycle_lock.root(), journal)?;
    if failpoint == LifecycleFailpoint::AfterTakeoverStateCommitted {
        return Err(TakeoverLifecycleError::SimulatedInterruption(
            "接管领域状态已提交，原目录尚未清理",
        ));
    }

    discard_original_after_commit(paths, storage, lifecycle_lock, plan, journal, failpoint)?;
    storage.update_takeover_transaction_phase(
        &journal.transaction_id,
        TakeoverJournalPhase::OriginalDiscarded.as_storage_str(),
        now,
    )?;
    journal.phase = TakeoverJournalPhase::OriginalDiscarded;
    write_takeover_journal(paths, lifecycle_lock.root(), journal)?;
    Ok(())
}

fn selected_preserve_mount(
    plan: &StoredTakeoverPlan,
    preserved_path_ids: &[String],
) -> Result<bool, TakeoverLifecycleError> {
    let path = plan
        .plan
        .paths
        .first()
        .ok_or(StorageError::InvalidTakeoverPlan)?;
    match preserved_path_ids {
        [] => Ok(false),
        [selected] if selected == &path.id => Ok(true),
        _ => Err(StorageError::InvalidTakeoverSelection.into()),
    }
}

fn build_takeover_journal(
    transaction_id: &str,
    plan: &StoredTakeoverPlan,
    preserve_mount: bool,
) -> Result<TakeoverJournal, TakeoverLifecycleError> {
    let path = plan
        .plan
        .paths
        .first()
        .ok_or(StorageError::InvalidTakeoverPlan)?;
    let original_entries = collect_original_manifest_path(Path::new(&path.original_path))?;
    build_takeover_journal_with_entries(transaction_id, plan, preserve_mount, original_entries)
}

fn build_takeover_journal_with_entries(
    transaction_id: &str,
    plan: &StoredTakeoverPlan,
    preserve_mount: bool,
    original_entries: Vec<TakeoverOriginalEntry>,
) -> Result<TakeoverJournal, TakeoverLifecycleError> {
    let path = plan
        .plan
        .paths
        .first()
        .ok_or(StorageError::InvalidTakeoverPlan)?;
    let original = Path::new(&path.original_path);
    let parent = original
        .parent()
        .ok_or_else(|| TakeoverLifecycleError::UnsafeLocation(path.original_path.clone()))?;
    let name = original
        .file_name()
        .ok_or_else(|| TakeoverLifecycleError::UnsafeLocation(path.original_path.clone()))?;
    let host_name = name
        .to_str()
        .ok_or_else(|| TakeoverLifecycleError::NonUnicodePath(path.original_path.clone()))?;
    let bundle_relative = format!("bundles/{}", plan.plan.bundle_id);
    Ok(TakeoverJournal {
        version: TAKEOVER_JOURNAL_VERSION,
        transaction_id: transaction_id.to_owned(),
        plan_id: plan.plan.id.clone(),
        bundle_id: plan.plan.bundle_id.clone(),
        content_id: plan.plan.content_id.clone(),
        member_id: plan.plan.member_id.clone(),
        path_id: path.id.clone(),
        skill_name: plan.plan.skill_name.clone(),
        content_fingerprint: plan.plan.content_fingerprint.clone(),
        preserve_mount,
        phase: TakeoverJournalPhase::JournalReady,
        staging_relative: format!("staging/{transaction_id}"),
        bundle_relative: bundle_relative.clone(),
        content_relative: format!("{bundle_relative}/contents/{}", plan.plan.content_id),
        current_target: format!("contents/{}", plan.plan.content_id),
        host_parent: path_to_string(parent)?,
        host_name: host_name.to_owned(),
        hidden_name: format!(".skillyard-takeover-{transaction_id}-original"),
        expected_target: plan.plan.expected_target.clone(),
        parent_device: path.parent_device,
        parent_inode: path.parent_inode,
        parent_mode: path.parent_mode,
        original_device: path.original_device,
        original_inode: path.original_inode,
        original_mode: path.original_mode,
        original_entries,
    })
}

/// Phase 会随事务推进；seal 只覆盖其余不可变合同与原目录逐项删除授权。
fn takeover_journal_contract_sha256(
    journal: &TakeoverJournal,
) -> Result<String, TakeoverLifecycleError> {
    let mut contract = journal.clone();
    contract.phase = TakeoverJournalPhase::JournalReady;
    let bytes = serde_json::to_vec(&contract)?;
    Ok(hex_path_bytes(&Sha256::digest(bytes)))
}

fn ensure_takeover_journal_fits(journal: &TakeoverJournal) -> Result<(), TakeoverLifecycleError> {
    for phase in [
        TakeoverJournalPhase::JournalReady,
        TakeoverJournalPhase::CandidateReady,
        TakeoverJournalPhase::ReplacementStaged,
        TakeoverJournalPhase::HostSwapped,
        TakeoverJournalPhase::StateCommitted,
        TakeoverJournalPhase::OriginalDiscarded,
    ] {
        let mut candidate = journal.clone();
        candidate.phase = phase;
        let actual = serde_json::to_vec_pretty(&candidate)?.len();
        if actual > MAX_TAKEOVER_JOURNAL_BYTES {
            return Err(TakeoverLifecycleError::JournalTooLarge {
                actual,
                limit: MAX_TAKEOVER_JOURNAL_BYTES,
            });
        }
    }
    Ok(())
}

fn validate_takeover_execution_snapshot(
    paths: &ApplicationPaths,
    storage: &Storage,
    lifecycle_lock: &LifecycleLock,
    plan: &StoredTakeoverPlan,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    if plan.status != "pending" && plan.status != "consumed" {
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    let location = derive_takeover_location(paths, storage, &plan.observation)?;
    let path = plan
        .plan
        .paths
        .first()
        .ok_or(StorageError::InvalidTakeoverPlan)?;
    verify_final_snapshot(
        &location,
        &plan.observation,
        &plan.plan,
        (path.parent_device, path.parent_inode, path.parent_mode),
        (
            path.original_device,
            path.original_inode,
            path.original_mode,
        ),
    )?;
    let parent = open_takeover_parent(paths, storage, plan, journal)?;
    validate_original_entry(&parent, journal)?;
    if entry_metadata_at(&parent.handle, OsStr::new(&journal.hidden_name))
        .map_err(|source| takeover_io("检查 Host 隐藏路径", &parent.path, source))?
        .is_some()
    {
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    if fs::symlink_metadata(&plan.plan.managed_directory).is_ok() {
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    let staging =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let bundles =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?;
    let central_device = lifecycle_lock
        .root()
        .metadata()
        .map_err(|source| takeover_io("检查 Central Store", paths.data_root(), source))?
        .dev();
    let staging_device = staging
        .metadata()
        .map_err(|source| takeover_io("检查 staging", &paths.staging_root(), source))?
        .dev();
    let bundles_device = bundles
        .metadata()
        .map_err(|source| takeover_io("检查 bundles", &paths.bundles_root(), source))?
        .dev();
    ensure_same_takeover_device(
        journal.parent_device,
        journal.original_device,
        &[central_device, staging_device, bundles_device],
    )?;
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

fn ensure_same_takeover_device(
    parent_device: u64,
    original_device: u64,
    central_devices: &[u64],
) -> Result<(), TakeoverLifecycleError> {
    if original_device == parent_device
        && central_devices
            .iter()
            .all(|device| *device == parent_device)
    {
        Ok(())
    } else {
        Err(TakeoverLifecycleError::CrossDevice)
    }
}

fn prepare_takeover_candidate(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    plan: &StoredTakeoverPlan,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    mkdir_at(&staging_root, OsStr::new(&journal.transaction_id), 0o700)
        .map_err(|source| takeover_io("创建接管临时目录", &paths.staging_root(), source))?;
    let staging = open_directory_at(&staging_root, OsStr::new(&journal.transaction_id))
        .map_err(|source| takeover_io("打开接管临时目录", &paths.staging_root(), source))?;
    mkdir_at(&staging, OsStr::new("candidate"), 0o700)
        .map_err(|source| takeover_io("创建接管候选目录", &paths.staging_root(), source))?;
    let candidate = open_directory_at(&staging, OsStr::new("candidate"))
        .map_err(|source| takeover_io("打开接管候选目录", &paths.staging_root(), source))?;
    mkdir_at(&candidate, OsStr::new("members"), 0o700)
        .map_err(|source| takeover_io("创建接管成员目录", &paths.staging_root(), source))?;
    let members = open_directory_at(&candidate, OsStr::new("members"))
        .map_err(|source| takeover_io("打开接管成员目录", &paths.staging_root(), source))?;
    let members_path = paths
        .staging_root()
        .join(&journal.transaction_id)
        .join("candidate/members");
    let mut budget = BundleCopyBudget::production();
    copy_single_skill_tree_into_open_directory(
        Path::new(&plan.observation.skill_root),
        &members,
        &members_path,
        OsStr::new(&journal.skill_name),
        &journal.skill_name,
        &journal.content_fingerprint,
        &mut budget,
    )?;
    members
        .sync_all()
        .map_err(|source| takeover_io("同步接管成员目录", &members_path, source))?;
    candidate
        .sync_all()
        .map_err(|source| takeover_io("同步接管候选目录", &members_path, source))?;
    staging
        .sync_all()
        .map_err(|source| takeover_io("同步接管临时目录", &members_path, source))?;
    staging_root
        .sync_all()
        .map_err(|source| takeover_io("同步 staging", &paths.staging_root(), source))?;
    validate_candidate_container(
        &candidate,
        &members,
        &members_path,
        &journal.skill_name,
        &journal.content_fingerprint,
    )
}

fn validate_candidate_container(
    candidate: &File,
    members: &File,
    members_path: &Path,
    skill_name: &str,
    fingerprint: &str,
) -> Result<(), TakeoverLifecycleError> {
    if read_entry_names_os_from_handle(candidate)? != [OsString::from("members")]
        || read_entry_names_os_from_handle(members)? != [OsString::from(skill_name)]
    {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "接管候选的成员边界被外部修改".to_owned(),
        ));
    }
    let validated = validate_single_skill_folder(&members_path.join(skill_name))?;
    if validated.name != skill_name || validated.fingerprint != fingerprint {
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    Ok(())
}

fn publish_takeover_candidate(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    plan: &StoredTakeoverPlan,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let staging = open_directory_at(&staging_root, OsStr::new(&journal.transaction_id))
        .map_err(|source| takeover_io("打开接管临时目录", &paths.staging_root(), source))?;
    let bundles_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?;
    mkdir_at(&bundles_root, OsStr::new(&journal.bundle_id), 0o700)
        .map_err(|source| takeover_io("创建接管 Bundle", &paths.bundles_root(), source))?;
    let bundle = open_directory_at(&bundles_root, OsStr::new(&journal.bundle_id))
        .map_err(|source| takeover_io("打开接管 Bundle", &paths.bundles_root(), source))?;
    mkdir_at(&bundle, OsStr::new("contents"), 0o700)
        .map_err(|source| takeover_io("创建接管内容目录", &paths.bundles_root(), source))?;
    let contents = open_directory_at(&bundle, OsStr::new("contents"))
        .map_err(|source| takeover_io("打开接管内容目录", &paths.bundles_root(), source))?;
    rename_at_no_replace(
        &staging,
        OsStr::new("candidate"),
        &contents,
        OsStr::new(&journal.content_id),
    )
    .map_err(|source| {
        takeover_io(
            "发布接管候选",
            Path::new(&plan.plan.content_directory),
            source,
        )
    })?;
    staging
        .sync_all()
        .map_err(|source| takeover_io("同步接管临时目录", &paths.staging_root(), source))?;
    contents.sync_all().map_err(|source| {
        takeover_io(
            "同步接管内容目录",
            Path::new(&plan.plan.content_directory),
            source,
        )
    })?;

    let temporary_current = OsString::from(format!(".current-{}", journal.transaction_id));
    symlink_at(
        Path::new(&journal.current_target),
        &bundle,
        &temporary_current,
    )
    .map_err(|source| {
        takeover_io(
            "创建接管 current",
            Path::new(&plan.plan.managed_directory),
            source,
        )
    })?;
    bundle.sync_all().map_err(|source| {
        takeover_io(
            "同步接管 current",
            Path::new(&plan.plan.managed_directory),
            source,
        )
    })?;
    rename_at_no_replace(&bundle, &temporary_current, &bundle, OsStr::new("current")).map_err(
        |source| {
            takeover_io(
                "发布接管 current",
                Path::new(&plan.plan.managed_directory),
                source,
            )
        },
    )?;
    bundle.sync_all().map_err(|source| {
        takeover_io(
            "同步接管 Bundle",
            Path::new(&plan.plan.managed_directory),
            source,
        )
    })?;
    bundles_root
        .sync_all()
        .map_err(|source| takeover_io("同步 Bundle 根目录", &paths.bundles_root(), source))?;
    validate_takeover_managed_content(paths, lifecycle_lock, plan, journal)
}

fn stage_host_replacement(
    paths: &ApplicationPaths,
    storage: &Storage,
    lifecycle_lock: &LifecycleLock,
    plan: &StoredTakeoverPlan,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    validate_original_snapshot(paths, storage, plan, journal)?;
    validate_takeover_managed_content(paths, lifecycle_lock, plan, journal)?;
    let parent = open_takeover_parent(paths, storage, plan, journal)?;
    validate_original_entry(&parent, journal)?;
    if entry_metadata_at(&parent.handle, OsStr::new(&journal.hidden_name))
        .map_err(|source| takeover_io("检查 Host 隐藏路径", &parent.path, source))?
        .is_some()
    {
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    if journal.preserve_mount {
        symlink_at(
            Path::new(&journal.expected_target),
            &parent.handle,
            OsStr::new(&journal.hidden_name),
        )
        .map_err(|source| takeover_io("创建 Host 隐藏替换项", &parent.path, source))?;
        parent
            .handle
            .sync_all()
            .map_err(|source| takeover_io("同步 Host 隐藏替换项", &parent.path, source))?;
    }
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

fn apply_host_takeover(
    paths: &ApplicationPaths,
    storage: &Storage,
    lifecycle_lock: &LifecycleLock,
    plan: &StoredTakeoverPlan,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    validate_original_snapshot(paths, storage, plan, journal)?;
    validate_takeover_managed_content(paths, lifecycle_lock, plan, journal)?;
    let parent = open_takeover_parent(paths, storage, plan, journal)?;
    validate_original_entry(&parent, journal)?;
    if journal.preserve_mount {
        validate_hidden_link(&parent, journal)?;
        rename_at_swap(
            &parent.handle,
            OsStr::new(&journal.host_name),
            &parent.handle,
            OsStr::new(&journal.hidden_name),
        )
        .map_err(|source| takeover_io("原子切换 Host Skill", &parent.path, source))?;
    } else {
        if entry_metadata_at(&parent.handle, OsStr::new(&journal.hidden_name))
            .map_err(|source| takeover_io("检查 Host 隐藏路径", &parent.path, source))?
            .is_some()
        {
            return Err(TakeoverLifecycleError::PlanPreconditionChanged);
        }
        rename_at_no_replace(
            &parent.handle,
            OsStr::new(&journal.host_name),
            &parent.handle,
            OsStr::new(&journal.hidden_name),
        )
        .map_err(|source| takeover_io("移出 Host Skill", &parent.path, source))?;
    }
    parent
        .handle
        .sync_all()
        .map_err(|source| takeover_io("同步 Host Skill 父目录", &parent.path, source))?;
    validate_takeover_effect(paths, storage, lifecycle_lock, plan, journal)
}

fn validate_original_snapshot(
    paths: &ApplicationPaths,
    storage: &Storage,
    plan: &StoredTakeoverPlan,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    let location = derive_takeover_location(paths, storage, &plan.observation)?;
    validate_project_management_snapshot(location.project.as_ref(), &plan.observation.skill_file)?;
    let parent = open_takeover_parent(paths, storage, plan, journal)?;
    validate_original_entry(&parent, journal)?;
    let observed = fingerprint_skill_root(Path::new(&plan.observation.skill_root))
        .map_err(|error| map_scan_error(error, &plan.observation.skill_root))?;
    let validated = validate_single_skill_folder(Path::new(&plan.observation.skill_root))?;
    if observed != plan.observation.observed_fingerprint
        || validated.name != journal.skill_name
        || validated.description != plan.plan.skill_description
        || validated.fingerprint != journal.content_fingerprint
        || validated.warnings != plan.plan.warnings
    {
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    validate_original_entry(&parent, journal)
}

fn validate_original_entry(
    parent: &OpenTakeoverParent,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    let metadata = entry_metadata_at(&parent.handle, OsStr::new(&journal.host_name))
        .map_err(|source| takeover_io("检查 Host Skill", &parent.path, source))?
        .ok_or(TakeoverLifecycleError::PlanPreconditionChanged)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
        || metadata.st_dev as u64 != journal.original_device
        || metadata.st_ino != journal.original_inode
        || u32::from(metadata.st_mode) != journal.original_mode
    {
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    Ok(())
}

fn validate_hidden_link(
    parent: &OpenTakeoverParent,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    let metadata = entry_metadata_at(&parent.handle, OsStr::new(&journal.hidden_name))
        .map_err(|source| takeover_io("检查 Host 隐藏替换项", &parent.path, source))?
        .ok_or(TakeoverLifecycleError::PlanPreconditionChanged)?;
    let target = read_link_at(&parent.handle, OsStr::new(&journal.hidden_name))
        .map_err(|source| takeover_io("读取 Host 隐藏替换项", &parent.path, source))?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFLNK
        || target != Path::new(&journal.expected_target)
    {
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    Ok(())
}

fn validate_takeover_effect(
    paths: &ApplicationPaths,
    storage: &Storage,
    lifecycle_lock: &LifecycleLock,
    plan: &StoredTakeoverPlan,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    let parent = open_takeover_parent(paths, storage, plan, journal)?;
    let hidden = entry_metadata_at(&parent.handle, OsStr::new(&journal.hidden_name))
        .map_err(|source| takeover_io("检查 Host 隐藏原目录", &parent.path, source))?
        .ok_or_else(|| {
            TakeoverLifecycleError::RecoveryBlocked("Host 隐藏原目录已经缺失".to_owned())
        })?;
    if hidden.st_mode & libc::S_IFMT != libc::S_IFDIR
        || hidden.st_dev as u64 != journal.original_device
        || hidden.st_ino != journal.original_inode
        || u32::from(hidden.st_mode) != journal.original_mode
    {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "Host 隐藏原目录身份与接管 Plan 不一致".to_owned(),
        ));
    }
    let visible = entry_metadata_at(&parent.handle, OsStr::new(&journal.host_name))
        .map_err(|source| takeover_io("检查接管后的 Host 路径", &parent.path, source))?;
    if journal.preserve_mount {
        let visible = visible.ok_or_else(|| {
            TakeoverLifecycleError::RecoveryBlocked("接管后的 Host Mount 已缺失".to_owned())
        })?;
        let target = read_link_at(&parent.handle, OsStr::new(&journal.host_name))
            .map_err(|source| takeover_io("读取接管后的 Host Mount", &parent.path, source))?;
        if visible.st_mode & libc::S_IFMT != libc::S_IFLNK
            || target != Path::new(&journal.expected_target)
        {
            return Err(TakeoverLifecycleError::RecoveryBlocked(
                "接管后的 Host Mount 被外部修改".to_owned(),
            ));
        }
    } else if visible.is_some() {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "未保留挂载的 Host 路径被外部重新创建".to_owned(),
        ));
    }
    validate_takeover_managed_content(paths, lifecycle_lock, plan, journal)
}

fn validate_takeover_managed_content(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    plan: &StoredTakeoverPlan,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    let bundles_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?;
    let bundle =
        open_directory_at(&bundles_root, OsStr::new(&journal.bundle_id)).map_err(|source| {
            takeover_io(
                "打开接管 Bundle",
                Path::new(&plan.plan.managed_directory),
                source,
            )
        })?;
    let current = entry_metadata_at(&bundle, OsStr::new("current"))
        .map_err(|source| {
            takeover_io(
                "检查接管 current",
                Path::new(&plan.plan.managed_directory),
                source,
            )
        })?
        .ok_or_else(|| TakeoverLifecycleError::RecoveryBlocked("接管 current 缺失".to_owned()))?;
    let current_target = read_link_at(&bundle, OsStr::new("current")).map_err(|source| {
        takeover_io(
            "读取接管 current",
            Path::new(&plan.plan.managed_directory),
            source,
        )
    })?;
    if current.st_mode & libc::S_IFMT != libc::S_IFLNK
        || current_target != Path::new(&journal.current_target)
    {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "接管 current 指向未知内容".to_owned(),
        ));
    }
    let contents = open_directory_at(&bundle, OsStr::new("contents")).map_err(|source| {
        takeover_io(
            "打开接管 contents",
            Path::new(&plan.plan.managed_directory),
            source,
        )
    })?;
    let content =
        open_directory_at(&contents, OsStr::new(&journal.content_id)).map_err(|source| {
            takeover_io(
                "打开接管 Content",
                Path::new(&plan.plan.content_directory),
                source,
            )
        })?;
    let members = open_directory_at(&content, OsStr::new("members")).map_err(|source| {
        takeover_io(
            "打开接管成员目录",
            Path::new(&plan.plan.content_directory),
            source,
        )
    })?;
    validate_candidate_container(
        &content,
        &members,
        &Path::new(&plan.plan.content_directory).join("members"),
        &journal.skill_name,
        &journal.content_fingerprint,
    )?;
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

fn collect_original_manifest_path(
    root: &Path,
) -> Result<Vec<TakeoverOriginalEntry>, TakeoverLifecycleError> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)
        .map_err(|source| takeover_io("打开原 Skill 目录", root, source))?;
    let mut entries = Vec::new();
    collect_original_manifest(&directory, Vec::new(), root, &mut entries)?;
    entries.sort_by(|left, right| left.relative_path_hex.cmp(&right.relative_path_hex));
    Ok(entries)
}

fn collect_original_manifest(
    directory: &File,
    prefix: Vec<u8>,
    visible_path: &Path,
    entries: &mut Vec<TakeoverOriginalEntry>,
) -> Result<(), TakeoverLifecycleError> {
    for name in read_entry_names_os_from_handle(directory)? {
        let mut relative = prefix.clone();
        if !relative.is_empty() {
            relative.push(b'/');
        }
        relative.extend_from_slice(name.as_bytes());
        let child_path = visible_path.join(&name);
        let metadata = entry_metadata_at(directory, &name)
            .map_err(|source| takeover_io("检查原 Skill 条目", &child_path, source))?
            .ok_or(TakeoverLifecycleError::PlanPreconditionChanged)?;
        let kind = match metadata.st_mode & libc::S_IFMT {
            libc::S_IFDIR => TakeoverOriginalEntryKind::Directory,
            libc::S_IFREG if metadata.st_nlink == 1 => TakeoverOriginalEntryKind::File,
            _ => return Err(TakeoverLifecycleError::PlanPreconditionChanged),
        };
        let content_sha256 = if kind == TakeoverOriginalEntryKind::File {
            Some(hash_manifest_file(
                directory,
                &name,
                &child_path,
                &metadata,
            )?)
        } else {
            None
        };
        let after = entry_metadata_at(directory, &name)
            .map_err(|source| takeover_io("重新检查原 Skill 条目", &child_path, source))?
            .ok_or(TakeoverLifecycleError::PlanPreconditionChanged)?;
        if !same_manifest_stat(&metadata, &after) {
            return Err(TakeoverLifecycleError::PlanPreconditionChanged);
        }
        entries.push(takeover_original_entry(
            &relative,
            kind,
            &after,
            content_sha256,
        )?);
        if kind == TakeoverOriginalEntryKind::Directory {
            let child = open_directory_at(directory, &name)
                .map_err(|source| takeover_io("打开原 Skill 子目录", &child_path, source))?;
            collect_original_manifest(&child, relative, &child_path, entries)?;
        }
    }
    Ok(())
}

fn takeover_original_entry(
    relative: &[u8],
    kind: TakeoverOriginalEntryKind,
    metadata: &libc::stat,
    content_sha256: Option<String>,
) -> Result<TakeoverOriginalEntry, TakeoverLifecycleError> {
    Ok(TakeoverOriginalEntry {
        relative_path_hex: hex_path_bytes(relative),
        kind,
        device: metadata.st_dev as u64,
        inode: metadata.st_ino,
        mode: u32::from(metadata.st_mode),
        links: metadata.st_nlink as u64,
        size: u64::try_from(metadata.st_size)
            .map_err(|_| TakeoverLifecycleError::PlanPreconditionChanged)?,
        modified_seconds: stat_modified_seconds(metadata),
        modified_nanoseconds: stat_modified_nanoseconds(metadata),
        changed_seconds: stat_changed_seconds(metadata),
        changed_nanoseconds: stat_changed_nanoseconds(metadata),
        content_sha256,
    })
}

fn hash_manifest_file(
    parent: &File,
    name: &OsStr,
    path: &Path,
    expected: &libc::stat,
) -> Result<String, TakeoverLifecycleError> {
    let mut file = open_regular_file_at(parent, name, path, false)?;
    let opened = file
        .metadata()
        .map_err(|source| takeover_io("检查原 Skill 文件", path, source))?;
    if opened.dev() != expected.st_dev as u64
        || opened.ino() != expected.st_ino
        || opened.mode() != u32::from(expected.st_mode)
        || opened.nlink() != expected.st_nlink as u64
        || opened.size()
            != u64::try_from(expected.st_size)
                .map_err(|_| TakeoverLifecycleError::PlanPreconditionChanged)?
    {
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|source| takeover_io("读取原 Skill 文件", path, source))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let after = file
        .metadata()
        .map_err(|source| takeover_io("重新检查原 Skill 文件", path, source))?;
    if opened.dev() != after.dev()
        || opened.ino() != after.ino()
        || opened.mode() != after.mode()
        || opened.nlink() != after.nlink()
        || opened.size() != after.size()
        || opened.mtime() != after.mtime()
        || opened.mtime_nsec() != after.mtime_nsec()
        || opened.ctime() != after.ctime()
        || opened.ctime_nsec() != after.ctime_nsec()
    {
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    Ok(hex_path_bytes(&hasher.finalize()))
}

fn same_manifest_stat(left: &libc::stat, right: &libc::stat) -> bool {
    left.st_dev == right.st_dev
        && left.st_ino == right.st_ino
        && left.st_mode == right.st_mode
        && left.st_nlink == right.st_nlink
        && left.st_size == right.st_size
        && stat_modified_seconds(left) == stat_modified_seconds(right)
        && stat_modified_nanoseconds(left) == stat_modified_nanoseconds(right)
        && stat_changed_seconds(left) == stat_changed_seconds(right)
        && stat_changed_nanoseconds(left) == stat_changed_nanoseconds(right)
}

fn stat_modified_seconds(metadata: &libc::stat) -> i64 {
    metadata.st_mtime
}

fn stat_modified_nanoseconds(metadata: &libc::stat) -> i64 {
    metadata.st_mtime_nsec
}

fn stat_changed_seconds(metadata: &libc::stat) -> i64 {
    metadata.st_ctime
}

fn stat_changed_nanoseconds(metadata: &libc::stat) -> i64 {
    metadata.st_ctime_nsec
}

fn validate_original_manifest(
    root: &Path,
    expected: &[TakeoverOriginalEntry],
) -> Result<(), TakeoverLifecycleError> {
    if collect_original_manifest_path(root)? == expected {
        Ok(())
    } else {
        Err(TakeoverLifecycleError::RecoveryBlocked(
            "隔离原目录包含外部新增、移除或替换的条目".to_owned(),
        ))
    }
}

fn remove_manifest_tree_at(
    parent: &File,
    name: &OsStr,
    path: &Path,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    let root_metadata = entry_metadata_at(parent, name)
        .map_err(|source| takeover_io("检查 Central discard", path, source))?
        .ok_or_else(|| {
            TakeoverLifecycleError::RecoveryBlocked("Central discard 已经缺失".to_owned())
        })?;
    if root_metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
        || root_metadata.st_dev as u64 != journal.original_device
        || root_metadata.st_ino != journal.original_inode
        || u32::from(root_metadata.st_mode) != journal.original_mode
    {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "Central discard 根目录身份不一致".to_owned(),
        ));
    }
    let expected = journal
        .original_entries
        .iter()
        .map(|entry| (entry.relative_path_hex.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    let root = open_directory_at(parent, name)
        .map_err(|source| takeover_io("打开 Central discard", path, source))?;
    let opened_root = root
        .metadata()
        .map_err(|source| takeover_io("检查已打开 Central discard", path, source))?;
    if opened_root.dev() != root_metadata.st_dev as u64
        || opened_root.ino() != root_metadata.st_ino
        || opened_root.mode() != u32::from(root_metadata.st_mode)
    {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "打开的 Central discard 已被替换".to_owned(),
        ));
    }
    validate_remaining_manifest_entries(&root, path, &journal.original_entries)?;
    drop(root);
    // readdir 使用 duplicate fd 并共享目录 offset；第二遍删除必须重新打开根目录。
    let root = open_directory_at(parent, name)
        .map_err(|source| takeover_io("重新打开 Central discard", path, source))?;
    let reopened_root = root
        .metadata()
        .map_err(|source| takeover_io("重新检查已打开 Central discard", path, source))?;
    if reopened_root.dev() != root_metadata.st_dev as u64
        || reopened_root.ino() != root_metadata.st_ino
        || reopened_root.mode() != u32::from(root_metadata.st_mode)
    {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "删除前 Central discard 根目录已被替换".to_owned(),
        ));
    }
    remove_known_manifest_entries(&root, Vec::new(), path, &expected)?;
    drop(root);
    let before_root_unlink = entry_metadata_at(parent, name)
        .map_err(|source| takeover_io("删除前检查 Central discard 根目录", path, source))?
        .ok_or_else(|| {
            TakeoverLifecycleError::RecoveryBlocked("Central discard 根目录在删除前消失".to_owned())
        })?;
    if before_root_unlink.st_dev != root_metadata.st_dev
        || before_root_unlink.st_ino != root_metadata.st_ino
        || before_root_unlink.st_mode != root_metadata.st_mode
    {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "Central discard 根目录在删除前被替换".to_owned(),
        ));
    }
    unlink_at(parent, name, true)
        .map_err(|source| takeover_io("删除空 Central discard", path, source))?;
    parent
        .sync_all()
        .map_err(|source| takeover_io("同步 Central discard 父目录", path, source))?;
    Ok(())
}

fn validate_remaining_manifest_entries(
    directory: &File,
    visible_path: &Path,
    expected: &[TakeoverOriginalEntry],
) -> Result<(), TakeoverLifecycleError> {
    let mut actual = Vec::new();
    collect_original_manifest(directory, Vec::new(), visible_path, &mut actual)?;
    actual.sort_by(|left, right| left.relative_path_hex.cmp(&right.relative_path_hex));
    if actual != expected {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "Central discard 与接管时的完整文件快照不一致".to_owned(),
        ));
    }
    Ok(())
}

fn remove_known_manifest_entries(
    directory: &File,
    prefix: Vec<u8>,
    visible_path: &Path,
    expected: &BTreeMap<String, &TakeoverOriginalEntry>,
) -> Result<(), TakeoverLifecycleError> {
    for name in read_entry_names_os_from_handle(directory)? {
        let mut relative = prefix.clone();
        if !relative.is_empty() {
            relative.push(b'/');
        }
        relative.extend_from_slice(name.as_bytes());
        let key = hex_path_bytes(&relative);
        let child_path = visible_path.join(&name);
        let expected_entry = expected.get(&key).ok_or_else(|| {
            TakeoverLifecycleError::RecoveryBlocked(format!(
                "Central discard 出现未授权新增条目：{}",
                child_path.display()
            ))
        })?;
        let metadata = entry_metadata_at(directory, &name)
            .map_err(|source| takeover_io("检查 Central discard 条目", &child_path, source))?
            .ok_or_else(|| {
                TakeoverLifecycleError::RecoveryBlocked(format!(
                    "Central discard 条目在检查期间消失：{}",
                    child_path.display()
                ))
            })?;
        let actual_kind = match metadata.st_mode & libc::S_IFMT {
            libc::S_IFDIR => TakeoverOriginalEntryKind::Directory,
            libc::S_IFREG if metadata.st_nlink == 1 => TakeoverOriginalEntryKind::File,
            _ => {
                return Err(TakeoverLifecycleError::RecoveryBlocked(format!(
                    "Central discard 条目类型已变化：{}",
                    child_path.display()
                )));
            }
        };
        let content_sha256 = if actual_kind == TakeoverOriginalEntryKind::File {
            Some(hash_manifest_file(
                directory,
                &name,
                &child_path,
                &metadata,
            )?)
        } else {
            None
        };
        let after_hash = entry_metadata_at(directory, &name)
            .map_err(|source| takeover_io("重新检查 Central discard 条目", &child_path, source))?
            .ok_or_else(|| {
                TakeoverLifecycleError::RecoveryBlocked(format!(
                    "Central discard 条目在检查期间消失：{}",
                    child_path.display()
                ))
            })?;
        if !same_manifest_stat(&metadata, &after_hash)
            || takeover_original_entry(&relative, actual_kind, &after_hash, content_sha256)?
                != **expected_entry
        {
            return Err(TakeoverLifecycleError::RecoveryBlocked(format!(
                "Central discard 条目证据已变化：{}",
                child_path.display()
            )));
        }
        if actual_kind == TakeoverOriginalEntryKind::Directory {
            let child = open_directory_at(directory, &name).map_err(|source| {
                takeover_io("打开 Central discard 子目录", &child_path, source)
            })?;
            let opened_child = child.metadata().map_err(|source| {
                takeover_io("检查已打开 Central discard 子目录", &child_path, source)
            })?;
            if opened_child.dev() != expected_entry.device
                || opened_child.ino() != expected_entry.inode
                || opened_child.mode() != expected_entry.mode
            {
                return Err(TakeoverLifecycleError::RecoveryBlocked(format!(
                    "打开的 Central discard 子目录已被替换：{}",
                    child_path.display()
                )));
            }
            remove_known_manifest_entries(&child, relative, &child_path, expected)?;
            drop(child);
            let before_unlink = entry_metadata_at(directory, &name)
                .map_err(|source| {
                    takeover_io("删除前检查 Central discard 子目录", &child_path, source)
                })?
                .ok_or_else(|| {
                    TakeoverLifecycleError::RecoveryBlocked(format!(
                        "Central discard 子目录在删除前消失：{}",
                        child_path.display()
                    ))
                })?;
            if before_unlink.st_dev as u64 != expected_entry.device
                || before_unlink.st_ino != expected_entry.inode
                || u32::from(before_unlink.st_mode) != expected_entry.mode
            {
                return Err(TakeoverLifecycleError::RecoveryBlocked(format!(
                    "Central discard 子目录在删除前被替换：{}",
                    child_path.display()
                )));
            }
            unlink_at(directory, &name, true).map_err(|source| {
                takeover_io("删除 Central discard 子目录", &child_path, source)
            })?;
        } else {
            let before_unlink = entry_metadata_at(directory, &name)
                .map_err(|source| {
                    takeover_io("删除前检查 Central discard 文件", &child_path, source)
                })?
                .ok_or_else(|| {
                    TakeoverLifecycleError::RecoveryBlocked(format!(
                        "Central discard 文件在删除前消失：{}",
                        child_path.display()
                    ))
                })?;
            if !manifest_stat_matches_entry(&before_unlink, expected_entry) {
                return Err(TakeoverLifecycleError::RecoveryBlocked(format!(
                    "Central discard 文件在删除前被替换：{}",
                    child_path.display()
                )));
            }
            unlink_at(directory, &name, false)
                .map_err(|source| takeover_io("删除 Central discard 文件", &child_path, source))?;
        }
    }
    directory
        .sync_all()
        .map_err(|source| takeover_io("同步 Central discard 目录", visible_path, source))?;
    Ok(())
}

fn manifest_stat_matches_entry(metadata: &libc::stat, expected: &TakeoverOriginalEntry) -> bool {
    metadata.st_dev as u64 == expected.device
        && metadata.st_ino == expected.inode
        && u32::from(metadata.st_mode) == expected.mode
        && metadata.st_nlink as u64 == expected.links
        && u64::try_from(metadata.st_size).ok() == Some(expected.size)
        && stat_modified_seconds(metadata) == expected.modified_seconds
        && stat_modified_nanoseconds(metadata) == expected.modified_nanoseconds
        && stat_changed_seconds(metadata) == expected.changed_seconds
        && stat_changed_nanoseconds(metadata) == expected.changed_nanoseconds
}

fn hex_path_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn discard_original_after_commit(
    paths: &ApplicationPaths,
    storage: &Storage,
    lifecycle_lock: &LifecycleLock,
    plan: &StoredTakeoverPlan,
    journal: &TakeoverJournal,
    failpoint: LifecycleFailpoint,
) -> Result<(), TakeoverLifecycleError> {
    validate_takeover_effect(paths, storage, lifecycle_lock, plan, journal)?;
    let parent = open_takeover_parent(paths, storage, plan, journal)?;
    let hidden_path = parent.path.join(&journal.hidden_name);
    validate_original_manifest(&hidden_path, &journal.original_entries)?;
    let hidden_path_text = path_to_string(&hidden_path)?;
    let observed = fingerprint_skill_root(&hidden_path)
        .map_err(|error| map_scan_error(error, &hidden_path_text))?;
    if observed != plan.observation.observed_fingerprint {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "隔离原目录内容已被外部修改".to_owned(),
        ));
    }
    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let staging = open_directory_at(&staging_root, OsStr::new(&journal.transaction_id))
        .map_err(|source| takeover_io("打开接管临时目录", &paths.staging_root(), source))?;
    if entry_metadata_at(&staging, OsStr::new("discarding-original"))
        .map_err(|source| takeover_io("检查接管清理目录", &paths.staging_root(), source))?
        .is_some()
    {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "接管清理目录已经包含未知原目录".to_owned(),
        ));
    }
    rename_at_no_replace(
        &parent.handle,
        OsStr::new(&journal.hidden_name),
        &staging,
        OsStr::new("discarding-original"),
    )
    .map_err(|source| takeover_io("将原目录移入 Central discard", &parent.path, source))?;
    parent
        .handle
        .sync_all()
        .map_err(|source| takeover_io("同步 Host Skill 父目录", &parent.path, source))?;
    staging
        .sync_all()
        .map_err(|source| takeover_io("同步 Central discard", &paths.staging_root(), source))?;
    inject_takeover_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTakeoverOriginalMovedBeforeDiscard,
    );
    remove_manifest_tree_at(
        &staging,
        OsStr::new("discarding-original"),
        &paths
            .staging_root()
            .join(&journal.transaction_id)
            .join("discarding-original"),
        journal,
    )?;
    Ok(())
}

fn cleanup_before_takeover_effect(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &Storage,
    plan: &StoredTakeoverPlan,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    validate_original_snapshot(paths, storage, plan, journal)?;
    validate_original_manifest(
        Path::new(&plan.observation.skill_root),
        &journal.original_entries,
    )?;
    let parent = open_takeover_parent(paths, storage, plan, journal)?;
    let hidden = entry_metadata_at(&parent.handle, OsStr::new(&journal.hidden_name))
        .map_err(|source| takeover_io("检查 Host 隐藏替换项", &parent.path, source))?;
    if hidden.is_some() {
        if !journal.preserve_mount {
            return Err(TakeoverLifecycleError::RecoveryBlocked(
                "Host 生效前出现未知隐藏原目录".to_owned(),
            ));
        }
        validate_hidden_link(&parent, journal)?;
    }

    let bundles =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?;
    if entry_metadata_at(&bundles, OsStr::new(&journal.bundle_id))
        .map_err(|source| takeover_io("检查接管 Bundle", &paths.bundles_root(), source))?
        .is_some()
    {
        return Err(TakeoverLifecycleError::RecoveryBlocked(
            "Host 生效前出现已发布 Bundle，当前阶段无法安全清理".to_owned(),
        ));
    }

    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let staging_path = paths.staging_root().join(&journal.transaction_id);
    let staging = match open_directory_at(&staging_root, OsStr::new(&journal.transaction_id)) {
        Ok(staging) => Some(staging),
        Err(source) if source.kind() == io::ErrorKind::NotFound => None,
        Err(source) => return Err(takeover_io("打开接管临时目录", &staging_path, source)),
    };
    if let Some(staging) = staging {
        let entries = read_entry_names_os_from_handle(&staging)?;
        if entries == [OsString::from("candidate")] {
            let candidate_path = staging_path.join("candidate");
            let candidate = open_directory_at(&staging, OsStr::new("candidate"))
                .map_err(|source| takeover_io("打开接管候选目录", &candidate_path, source))?;
            let members = open_directory_at(&candidate, OsStr::new("members"))
                .map_err(|source| takeover_io("打开接管成员目录", &candidate_path, source))?;
            validate_candidate_container(
                &candidate,
                &members,
                &candidate_path.join("members"),
                &journal.skill_name,
                &journal.content_fingerprint,
            )?;
            drop(members);
            drop(candidate);
            remove_owned_tree_at(&staging, OsStr::new("candidate"), &candidate_path)?;
        } else if !entries.is_empty() {
            return Err(TakeoverLifecycleError::RecoveryBlocked(
                "接管临时目录包含未知内容".to_owned(),
            ));
        }
        drop(staging);
        unlink_at(&staging_root, OsStr::new(&journal.transaction_id), true)
            .map_err(|source| takeover_io("清理接管临时目录", &staging_path, source))?;
        staging_root
            .sync_all()
            .map_err(|source| takeover_io("同步 staging", &paths.staging_root(), source))?;
    }

    if hidden.is_some() {
        unlink_at(&parent.handle, OsStr::new(&journal.hidden_name), false)
            .map_err(|source| takeover_io("清理 Host 隐藏替换项", &parent.path, source))?;
        parent
            .handle
            .sync_all()
            .map_err(|source| takeover_io("同步 Host Skill 父目录", &parent.path, source))?;
    }
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

fn remove_takeover_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let name = OsString::from(format!("{}.json", journal.transaction_id));
    match unlink_at(&journals, &name, false) {
        Ok(()) => journals
            .sync_all()
            .map_err(|source| takeover_io("同步 Journal 目录", &paths.journals_root(), source)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(takeover_io(
            "删除接管 Journal",
            &paths.journals_root(),
            source,
        )),
    }
}

fn cleanup_completed_takeover(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    match unlink_at(
        &journals,
        OsStr::new(&format!("{}.json", journal.transaction_id)),
        false,
    ) {
        Ok(()) => journals
            .sync_all()
            .map_err(|source| takeover_io("同步 Journal 目录", &paths.journals_root(), source))?,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(takeover_io(
                "删除接管 Journal",
                &paths.journals_root(),
                source,
            ));
        }
    }
    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    match unlink_at(&staging_root, OsStr::new(&journal.transaction_id), true) {
        Ok(()) => staging_root
            .sync_all()
            .map_err(|source| takeover_io("同步 staging", &paths.staging_root(), source))?,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(takeover_io(
                "清理接管临时目录",
                &paths.staging_root(),
                source,
            ));
        }
    }
    storage.forget_terminal_takeover_transaction(&journal.transaction_id)?;
    Ok(())
}

fn write_takeover_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &TakeoverJournal,
) -> Result<(), TakeoverLifecycleError> {
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let bytes = serde_json::to_vec_pretty(journal)?;
    if bytes.len() > MAX_TAKEOVER_JOURNAL_BYTES {
        return Err(TakeoverLifecycleError::JournalTooLarge {
            actual: bytes.len(),
            limit: MAX_TAKEOVER_JOURNAL_BYTES,
        });
    }
    let name = OsString::from(format!("{}.json", journal.transaction_id));
    write_atomic_at(&journals, &name, &paths.journals_root().join(&name), &bytes)?;
    Ok(())
}

fn open_takeover_parent(
    paths: &ApplicationPaths,
    storage: &Storage,
    plan: &StoredTakeoverPlan,
    journal: &TakeoverJournal,
) -> Result<OpenTakeoverParent, TakeoverLifecycleError> {
    let location = derive_takeover_location(paths, storage, &plan.observation)?;
    if location.parent != Path::new(&journal.host_parent) {
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    let relative = location
        .parent
        .strip_prefix(&location.base)
        .map_err(|_| TakeoverLifecycleError::UnsafeLocation(journal.host_parent.clone()))?;
    let mut handle = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&location.base)
        .map_err(|source| takeover_io("打开 Host 根目录", &location.base, source))?;
    let mut visible = location.base.clone();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(TakeoverLifecycleError::UnsafeLocation(
                journal.host_parent.clone(),
            ));
        };
        visible.push(name);
        handle = open_directory_at(&handle, name)
            .map_err(|source| takeover_io("打开 Host Skill 父目录", &visible, source))?;
    }
    let opened = handle
        .metadata()
        .map_err(|source| takeover_io("检查 Host Skill 父目录", &location.parent, source))?;
    let shown = fs::symlink_metadata(&location.parent)
        .map_err(|source| takeover_io("重新检查 Host Skill 父目录", &location.parent, source))?;
    if !opened.is_dir()
        || shown.file_type().is_symlink()
        || !shown.is_dir()
        || opened.dev() != shown.dev()
        || opened.ino() != shown.ino()
        || opened.dev() != journal.parent_device
        || opened.ino() != journal.parent_inode
        || opened.mode() != journal.parent_mode
    {
        return Err(TakeoverLifecycleError::PlanPreconditionChanged);
    }
    Ok(OpenTakeoverParent {
        handle,
        path: location.parent,
    })
}

fn inject_takeover_interruption(
    actual: LifecycleFailpoint,
    expected: LifecycleFailpoint,
    message: &'static str,
) -> Result<(), TakeoverLifecycleError> {
    if actual == expected {
        Err(TakeoverLifecycleError::SimulatedInterruption(message))
    } else {
        Ok(())
    }
}

fn inject_takeover_hard_exit(actual: LifecycleFailpoint, expected: LifecycleFailpoint) {
    if actual == expected {
        // 子进程测试必须绕过析构，才能留下真实的持久化中断现场。
        unsafe { libc::_exit(91) }
    }
}

fn takeover_io(action: &'static str, path: &Path, source: io::Error) -> TakeoverLifecycleError {
    TakeoverLifecycleError::InspectPath {
        path: format!("{action}：{}", path.display()),
        source,
    }
}

fn verify_final_snapshot(
    location: &TakeoverLocation,
    observation: &crate::domain::InventoryObservation,
    plan: &TakeoverPlan,
    expected_parent_identity: (u64, u64, u32),
    expected_original_identity: (u64, u64, u32),
) -> Result<(), TakeoverLifecycleError> {
    validate_project_management_snapshot(location.project.as_ref(), &observation.skill_file)?;
    let parent = inspect_directory_chain(&location.base, &location.parent)?;
    let original = inspect_real_directory(Path::new(&observation.skill_root))?;
    let observed_fingerprint = fingerprint_skill_root(Path::new(&observation.skill_root))
        .map_err(|error| map_scan_error(error, &observation.skill_root))?;
    let validated = validate_single_skill_folder(Path::new(&observation.skill_root))?;
    if observed_fingerprint != observation.observed_fingerprint
        || validated.name != plan.skill_name
        || validated.description != plan.skill_description
        || validated.fingerprint != plan.content_fingerprint
        || validated.warnings != plan.warnings
        || filesystem_identity(&parent) != expected_parent_identity
        || filesystem_identity(&original) != expected_original_identity
    {
        return Err(TakeoverLifecycleError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }
    Ok(())
}

fn validate_inventory_eligibility(
    observation: &crate::domain::InventoryObservation,
) -> Result<(), TakeoverLifecycleError> {
    if observation.stale {
        return Err(TakeoverLifecycleError::Ineligible("观察已经过期"));
    }
    if observation.metadata_status != SkillMetadataStatus::Valid {
        return Err(TakeoverLifecycleError::Ineligible("Skill metadata 无效"));
    }
    if observation.management_kind != ManagementKind::TakeoverCandidate
        || observation.management_evidence.is_some()
    {
        return Err(TakeoverLifecycleError::Ineligible(
            "该 Skill 已由其他管理方负责",
        ));
    }
    if !matches!(
        observation.location_kind,
        InventoryLocationKind::AppGlobal | InventoryLocationKind::AppProject
    ) {
        return Err(TakeoverLifecycleError::Ineligible(
            "只接受应用专属普通 Skill 目录",
        ));
    }
    Ok(())
}

fn derive_takeover_location(
    paths: &ApplicationPaths,
    storage: &Storage,
    observation: &crate::domain::InventoryObservation,
) -> Result<TakeoverLocation, TakeoverLifecycleError> {
    let config = app_config_for_root(paths, observation.root_key).ok_or(
        TakeoverLifecycleError::Ineligible("共享或受管目录不能由本片接管"),
    )?;
    match observation.root_key {
        ScanRootKey::CodexGlobal
        | ScanRootKey::ClaudeCodeGlobal
        | ScanRootKey::GitHubCopilotGlobal => {
            if observation.location_kind != InventoryLocationKind::AppGlobal
                || observation.project_id.is_some()
            {
                return Err(TakeoverLifecycleError::Ineligible(
                    "global observation 的范围不一致",
                ));
            }
            Ok(TakeoverLocation {
                app_id: config.id,
                scope: MountScope::Global,
                project: None,
                base: paths.home().to_path_buf(),
                parent: config.global_root,
            })
        }
        ScanRootKey::CodexProject
        | ScanRootKey::ClaudeCodeProject
        | ScanRootKey::GitHubCopilotProject => {
            if observation.location_kind != InventoryLocationKind::AppProject {
                return Err(TakeoverLifecycleError::Ineligible(
                    "project observation 的范围不一致",
                ));
            }
            let project_id =
                observation
                    .project_id
                    .as_deref()
                    .ok_or(TakeoverLifecycleError::Ineligible(
                        "project observation 缺少 Project",
                    ))?;
            let project = storage.read_project(project_id)?;
            validate_project_identity(&project)?;
            let parent = Path::new(&project.root_path).join(&config.project_relative_root);
            Ok(TakeoverLocation {
                app_id: config.id,
                scope: MountScope::Project,
                base: PathBuf::from(&project.root_path),
                project: Some(project),
                parent,
            })
        }
        ScanRootKey::SharedAgents | ScanRootKey::SharedAgentsProject => {
            Err(TakeoverLifecycleError::Ineligible("共享只读目录不能接管"))
        }
    }
}

fn app_config_for_root(
    paths: &ApplicationPaths,
    root_key: ScanRootKey,
) -> Option<SupportedAppPathConfig> {
    paths
        .supported_apps()
        .into_iter()
        .find(|config| config.root_key == root_key || config.project_root_key == root_key)
}

fn expected_observers(app_id: SupportedAppId, scope: MountScope) -> Vec<SupportedAppId> {
    if app_id == SupportedAppId::ClaudeCode && scope == MountScope::Project {
        vec![SupportedAppId::ClaudeCode, SupportedAppId::GitHubCopilot]
    } else {
        vec![app_id]
    }
}

fn validate_project_identity(project: &StoredProject) -> Result<(), TakeoverLifecycleError> {
    let path = Path::new(&project.root_path);
    let metadata = fs::symlink_metadata(path).map_err(|source| inspect_error(path, source))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.dev() != project.root_device
        || metadata.ino() != project.root_inode
    {
        return Err(TakeoverLifecycleError::ProjectChanged(
            project.root_path.clone(),
        ));
    }
    Ok(())
}

fn validate_project_management_snapshot(
    project: Option<&StoredProject>,
    skill_file: &str,
) -> Result<(), TakeoverLifecycleError> {
    let Some(project) = project else {
        return Ok(());
    };
    match inspect_git_head_management(Path::new(&project.root_path), Path::new(skill_file)) {
        ManagementEvidenceInspection::Absent => Ok(()),
        ManagementEvidenceInspection::Confirmed(_) => Err(TakeoverLifecycleError::Ineligible(
            "该 Skill 已由 Project 仓库维护",
        )),
        ManagementEvidenceInspection::Indeterminate(error) => Err(
            TakeoverLifecycleError::ProjectManagementIndeterminate(error.to_string()),
        ),
    }
}

fn inspect_real_directory(path: &Path) -> Result<fs::Metadata, TakeoverLifecycleError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| inspect_error(path, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(TakeoverLifecycleError::UnsafeLocation(
            path.display().to_string(),
        ));
    }
    Ok(metadata)
}

fn inspect_directory_chain(
    base: &Path,
    target: &Path,
) -> Result<fs::Metadata, TakeoverLifecycleError> {
    inspect_real_directory(base)?;
    let relative = target
        .strip_prefix(base)
        .map_err(|_| TakeoverLifecycleError::UnsafeLocation(target.display().to_string()))?;
    let mut current = base.to_path_buf();
    let mut metadata = fs::symlink_metadata(base).map_err(|source| inspect_error(base, source))?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(TakeoverLifecycleError::UnsafeLocation(
                target.display().to_string(),
            ));
        };
        current.push(name);
        metadata = inspect_real_directory(&current)?;
    }
    Ok(metadata)
}

fn filesystem_identity(metadata: &fs::Metadata) -> (u64, u64, u32) {
    (metadata.dev(), metadata.ino(), metadata.mode())
}

fn ensure_single_component(value: &str) -> Result<(), TakeoverLifecycleError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err(TakeoverLifecycleError::Ineligible(
            "Skill Name 不能作为安全目录名",
        ));
    }
    Ok(())
}

fn path_to_string(path: &Path) -> Result<String, TakeoverLifecycleError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| TakeoverLifecycleError::NonUnicodePath(path.display().to_string()))
}

fn inspect_error(path: &Path, source: std::io::Error) -> TakeoverLifecycleError {
    TakeoverLifecycleError::InspectPath {
        path: path.display().to_string(),
        source,
    }
}

fn map_scan_error(error: ScanError, path: &str) -> TakeoverLifecycleError {
    TakeoverLifecycleError::ObservationChanged(format!("{path}：{error}"))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn cross_device_preflight_is_rejected() {
        assert!(ensure_same_takeover_device(7, 7, &[7, 7, 7]).is_ok());
        assert!(matches!(
            ensure_same_takeover_device(7, 7, &[7, 8, 7]),
            Err(TakeoverLifecycleError::CrossDevice)
        ));
        assert!(matches!(
            ensure_same_takeover_device(7, 8, &[7, 7, 7]),
            Err(TakeoverLifecycleError::CrossDevice)
        ));
    }

    #[test]
    fn discard_cleanup_rejects_an_external_added_entry() {
        assert_discard_mutation_is_blocked(|discard, _sandbox| {
            fs::write(discard.join("external.txt"), "must survive").expect("应模拟外部新增");
        });
    }

    #[test]
    fn discard_cleanup_rejects_a_missing_expected_entry_before_any_deletion() {
        assert_discard_mutation_is_blocked(|discard, _sandbox| {
            fs::remove_file(discard.join("SKILL.md")).expect("应模拟已知条目缺失");
        });
    }

    #[test]
    fn discard_cleanup_rejects_a_same_kind_inode_replacement_before_any_deletion() {
        assert_discard_mutation_is_blocked(|discard, sandbox| {
            fs::rename(discard.join("SKILL.md"), sandbox.join("old-skill.md"))
                .expect("应移开原 inode");
            fs::write(discard.join("SKILL.md"), "original").expect("应创建同名同类型替代文件");
        });
    }

    #[test]
    fn discard_cleanup_rejects_in_place_file_rewrite_before_any_deletion() {
        assert_discard_mutation_is_blocked(|discard, _sandbox| {
            // 保持相同长度，证明内容 hash 能覆盖粗粒度时间戳没有变化的情况。
            fs::write(discard.join("SKILL.md"), "changed!").expect("应原地改写已知文件");
        });
    }

    fn assert_discard_mutation_is_blocked(mutate: impl FnOnce(&Path, &Path)) {
        let sandbox = tempdir().expect("应创建隔离目录");
        let discard = sandbox.path().join("discarding-original");
        fs::create_dir(&discard).expect("应创建 discard");
        fs::write(discard.join("SKILL.md"), "original").expect("应写入原条目");
        fs::write(discard.join("keep.txt"), "must remain").expect("应写入删除保护哨兵");
        let expected = collect_original_manifest_path(&discard).expect("应记录原目录 manifest");
        let metadata = fs::symlink_metadata(&discard).expect("应读取 discard 身份");
        mutate(&discard, sandbox.path());
        let journal = discard_test_journal(&metadata, expected);
        let parent = File::open(sandbox.path()).expect("应打开 discard 父目录");

        remove_manifest_tree_at(
            &parent,
            OsStr::new("discarding-original"),
            &discard,
            &journal,
        )
        .expect_err("完整快照变化必须阻塞清理");

        assert_eq!(
            fs::read_to_string(discard.join("keep.txt")).expect("哨兵条目不得被删除"),
            "must remain"
        );
    }

    fn discard_test_journal(
        metadata: &fs::Metadata,
        original_entries: Vec<TakeoverOriginalEntry>,
    ) -> TakeoverJournal {
        TakeoverJournal {
            version: TAKEOVER_JOURNAL_VERSION,
            transaction_id: Uuid::new_v4().to_string(),
            plan_id: "plan".to_owned(),
            bundle_id: Uuid::new_v4().to_string(),
            content_id: Uuid::new_v4().to_string(),
            member_id: Uuid::new_v4().to_string(),
            path_id: Uuid::new_v4().to_string(),
            skill_name: "alpha".to_owned(),
            content_fingerprint: "0".repeat(64),
            preserve_mount: true,
            phase: TakeoverJournalPhase::StateCommitted,
            staging_relative: "staging/transaction".to_owned(),
            bundle_relative: "bundles/bundle".to_owned(),
            content_relative: "bundles/bundle/contents/content".to_owned(),
            current_target: "contents/content".to_owned(),
            host_parent: "/tmp".to_owned(),
            host_name: "alpha".to_owned(),
            hidden_name: ".hidden".to_owned(),
            expected_target: "/tmp/expected".to_owned(),
            parent_device: metadata.dev(),
            parent_inode: metadata.ino(),
            parent_mode: metadata.mode(),
            original_device: metadata.dev(),
            original_inode: metadata.ino(),
            original_mode: metadata.mode(),
            original_entries,
        }
    }
}
