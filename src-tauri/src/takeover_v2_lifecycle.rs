use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    content::{
        BundleCopyBudget, ContentValidationError,
        copy_single_skill_tree_into_open_directory_preserving_partial,
    },
    domain::{
        MountScope, ScanRootKey, SupportedAppId, TakeoverOriginDisposition,
        TakeoverTargetInitialState, TakeoverV2Origin, TakeoverV2Plan, TakeoverV2PlanStatus,
        TakeoverV2Target,
    },
    lifecycle::{
        LifecycleError, LifecycleLock, acquire_lifecycle_lock, create_new_file_at,
        entry_metadata_at, mkdir_at, open_directory_at, open_managed_directory_from_root,
        open_regular_file_at, read_entry_names_os_from_handle, rename_at_no_replace,
        rename_at_swap, unlink_at, write_new_atomic_at,
    },
    paths::{ApplicationPaths, SupportedAppPathConfig},
    storage::{Storage, StorageError, StoredTakeoverV2Transaction},
    takeover_lifecycle::{
        TakeoverLifecycleError, TakeoverOriginalEntry, collect_original_manifest_at,
        validate_original_manifest_at,
    },
};

const TAKEOVER_V2_JOURNAL_VERSION: u32 = 2;
const TAKEOVER_V2_CANDIDATE_SHAPE_VERSION: u32 = 1;
const MAX_TAKEOVER_V2_JOURNAL_BYTES: usize = 1024 * 1024;
// 文件系统快照统一保存为 SHA-256 十六进制摘要，序列化长度因此固定且可预留。
const TAKEOVER_V2_EFFECT_OBSERVATION_BYTES: usize = 64;

#[derive(Debug, Error)]
pub(crate) enum TakeoverV2LifecycleError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error(transparent)]
    Content(#[from] ContentValidationError),
    #[error(transparent)]
    Takeover(#[from] TakeoverLifecycleError),
    #[error("无法检查 v2 接管路径 {path}：{source}")]
    InspectPath {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("v2 接管 Journal 无法解析：{0}")]
    InvalidJournal(#[from] serde_json::Error),
    #[error("v2 接管 Journal 超过安全大小限制（{actual} 字节，限制 {limit} 字节）")]
    JournalTooLarge { actual: usize, limit: usize },
    #[error("v2 接管恢复需要人工处理：{0}")]
    RecoveryBlocked(String),
    #[error("v2 接管的 Origin 快照已经变化：{0}")]
    OriginChanged(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TakeoverV2JournalPhase {
    Preparing,
    Prepared,
    EffectStarted,
    StateCommitted,
    CleanupCompleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TakeoverV2OriginManifest {
    origin_id: String,
    root_device: u64,
    root_inode: u64,
    root_mode: u32,
    entries: Vec<TakeoverOriginalEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TakeoverV2EffectOperation {
    CreateAbsentMount {
        target_id: String,
    },
    ReplaceOriginWithMount {
        target_id: String,
        origin_id: String,
    },
    RemoveOrigin {
        origin_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TakeoverV2EffectItem {
    operation: TakeoverV2EffectOperation,
    staged_observation: Option<String>,
    applied_observation: Option<String>,
    cleanup_completed: bool,
}

/// Journal 保存跨 SQLite 与文件系统边界恢复所需的合同与逐项进度。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TakeoverV2Journal {
    version: u32,
    transaction_id: String,
    plan_id: String,
    plan_seal: String,
    bundle_id: String,
    member_id: String,
    content_id: String,
    selected_origin_id: String,
    skill_name: String,
    content_fingerprint: String,
    candidate_shape_version: u32,
    phase: TakeoverV2JournalPhase,
    staging_relative: String,
    candidate_relative: String,
    phase_temp_relative: String,
    bundle_relative: String,
    content_relative: String,
    current_target: String,
    origins: Vec<TakeoverV2OriginManifest>,
    effect_items: Vec<TakeoverV2EffectItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnedJournalEntry {
    name: OsString,
    snapshot: JournalEntrySnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JournalEntrySnapshot {
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidateDirectoryAudit {
    name: OsString,
    path: PathBuf,
    snapshot: JournalEntrySnapshot,
    children: Vec<CandidateEntryAudit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CandidateEntryAudit {
    Directory(CandidateDirectoryAudit),
    File {
        name: OsString,
        path: PathBuf,
        snapshot: JournalEntrySnapshot,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidateAudit {
    staging: Option<CandidateDirectoryAudit>,
    complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreparedStagingKind {
    Missing,
    Empty,
    Candidate { complete: bool },
    CandidateWithEmptyBundle,
    CandidateWithContents,
    StagedBundle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedStagingAudit {
    kind: PreparedStagingKind,
    staging: Option<CandidateDirectoryAudit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StablePreparedAudit {
    formal: OwnedJournalEntry,
    formal_bytes: Vec<u8>,
    staging: PreparedStagingAudit,
    final_bundle: CandidateDirectoryAudit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BundleSkeletonKind {
    Empty,
    Contents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BundlePublishCheckpoint {
    B,
    C,
    D,
    BeforeAtomicPublish,
    AfterFreshStagedAudit,
    E,
    BeforePreparedCommit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CandidateSkillAuditState {
    content_complete: bool,
    all_directories_final: bool,
    has_distinguishable_final_directory: bool,
}

struct SelectedOriginRoot {
    handle: File,
    visible_path: PathBuf,
}

/// 本批准备完整候选，但仍停留在生效点之前，不发布 Bundle 或修改 Host。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn prepare_takeover_v2_journal(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
    now: i64,
) -> Result<String, TakeoverV2LifecycleError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    let preview = storage.read_takeover_v2_plan(plan_id)?;
    if preview.status != TakeoverV2PlanStatus::Pending {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "只有 pending Plan 可以开始接管".to_owned(),
        ));
    }
    let transaction_id = Uuid::new_v4().to_string();
    validate_all_targets(paths, &preview)?;
    let journal = build_takeover_v2_journal(paths, &transaction_id, &preview)?;
    validate_journal_size_for_all_phases(&journal)?;
    let journal_contract_sha256 = takeover_v2_journal_contract_sha256(&journal)?;
    let journal_relative = format!("journals/{}", journal_file_name(&transaction_id));
    let consumed = storage.begin_takeover_v2_transaction(
        plan_id,
        &transaction_id,
        &journal_relative,
        &journal_contract_sha256,
        now,
    )?;
    let mut expected = preview;
    expected.status = TakeoverV2PlanStatus::Consumed;
    if consumed != expected {
        abort_and_forget_before_effect(
            storage,
            &transaction_id,
            Some("SQLite 中的 v2 接管 Plan 与确认预览不一致"),
            now,
        )?;
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "SQLite 中的 v2 接管 Plan 与确认预览不一致".to_owned(),
        ));
    }

    // begin 会释放 SQLite 锁；首次写文件前必须再次证明所有外部 Origin 仍是同一快照。
    if let Err(error) = validate_all_origin_manifests(paths, &consumed, &journal.origins)
        .and_then(|()| validate_all_targets(paths, &consumed))
    {
        abort_and_forget_before_effect(storage, &transaction_id, Some(&error.to_string()), now)?;
        return Err(error);
    }
    if let Err(error) = write_takeover_v2_journal(paths, &lifecycle_lock, &journal) {
        // 已确认 temp 被替换时不能让重启恢复器再把它误认成事务自有文件。
        if matches!(
            &error,
            TakeoverV2LifecycleError::Lifecycle(LifecycleError::RecoveryBlocked(_))
        ) {
            storage.block_takeover_v2_transaction(&transaction_id, &error.to_string(), now)?;
        }
        return Err(error);
    }
    storage.update_takeover_v2_transaction_phase(&transaction_id, "preparing", now)?;
    prepare_takeover_v2_candidate(paths, &lifecycle_lock, &consumed, &journal)?;
    lifecycle_lock.recheck(paths)?;
    Ok(transaction_id)
}

fn prepare_takeover_v2_candidate(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    plan: &TakeoverV2Plan,
    journal: &TakeoverV2Journal,
) -> Result<(), TakeoverV2LifecycleError> {
    prepare_takeover_v2_candidate_with_hook(paths, lifecycle_lock, plan, journal, || {})
}

fn prepare_takeover_v2_candidate_with_hook(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    plan: &TakeoverV2Plan,
    journal: &TakeoverV2Journal,
    after_candidate_audited: impl FnOnce(),
) -> Result<(), TakeoverV2LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    validate_all_origin_manifests(paths, plan, &journal.origins)?;
    validate_all_targets(paths, plan)?;
    let (selected, _) = selected_origin_contract(plan, journal)?;

    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let staging_path = paths.staging_root().join(&journal.transaction_id);
    let staging = create_synced_candidate_directory(
        &staging_root,
        OsStr::new(&journal.transaction_id),
        &staging_path,
        "创建 v2 接管临时目录",
    )?;
    let candidate_path = staging_path.join("candidate");
    let candidate = create_synced_candidate_directory(
        &staging,
        OsStr::new("candidate"),
        &candidate_path,
        "创建 v2 接管候选目录",
    )?;
    let members_path = candidate_path.join("members");
    let members = create_synced_candidate_directory(
        &candidate,
        OsStr::new("members"),
        &members_path,
        "创建 v2 接管成员目录",
    )?;

    let mut budget = BundleCopyBudget::production();
    copy_single_skill_tree_into_open_directory_preserving_partial(
        Path::new(&selected.original_path),
        &members,
        &members_path,
        OsStr::new(&journal.skill_name),
        &journal.skill_name,
        &journal.content_fingerprint,
        &mut budget,
    )?;
    members
        .sync_all()
        .map_err(|source| v2_io("同步 v2 接管成员目录", &members_path, source))?;
    candidate
        .sync_all()
        .map_err(|source| v2_io("同步 v2 接管候选目录", &candidate_path, source))?;
    staging
        .sync_all()
        .map_err(|source| v2_io("同步 v2 接管临时目录", &staging_path, source))?;
    staging_root
        .sync_all()
        .map_err(|source| v2_io("同步 v2 staging", &paths.staging_root(), source))?;

    let audit = audit_takeover_v2_candidate(paths, lifecycle_lock, plan, journal)?;
    if !audit.complete {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 接管候选复制完成后仍不是完整合同内容".to_owned(),
        ));
    }
    after_candidate_audited();
    // 复制和候选验证都可能耗时；结束前必须交叉复核所有外部输入。
    validate_all_origin_manifests(paths, plan, &journal.origins)?;
    validate_all_targets(paths, plan)?;
    lifecycle_lock.recheck(paths)?;
    // 初次审计后 Candidate 仍可能被外部修改；成功返回前必须重新验证整棵合同内容。
    let final_audit = audit_takeover_v2_candidate(paths, lifecycle_lock, plan, journal)?;
    if !final_audit.complete {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 接管候选在最终审计时已不再完整".to_owned(),
        ));
    }
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

fn create_synced_candidate_directory(
    parent: &File,
    name: &OsStr,
    path: &Path,
    action: &'static str,
) -> Result<File, TakeoverV2LifecycleError> {
    mkdir_at(parent, name, 0o700).map_err(|source| v2_io(action, path, source))?;
    parent
        .sync_all()
        .map_err(|source| v2_io("同步 v2 接管候选父目录", path, source))?;
    open_directory_at(parent, name).map_err(|source| v2_io("打开 v2 接管候选目录", path, source))
}

fn selected_origin_contract<'a>(
    plan: &'a TakeoverV2Plan,
    journal: &'a TakeoverV2Journal,
) -> Result<(&'a TakeoverV2Origin, &'a TakeoverV2OriginManifest), TakeoverV2LifecycleError> {
    let selected = plan
        .origins
        .iter()
        .find(|origin| origin.id == journal.selected_origin_id)
        .ok_or_else(|| {
            TakeoverV2LifecycleError::RecoveryBlocked("Plan 缺少 selected Origin".to_owned())
        })?;
    let manifest = journal
        .origins
        .iter()
        .find(|manifest| manifest.origin_id == journal.selected_origin_id)
        .ok_or_else(|| {
            TakeoverV2LifecycleError::RecoveryBlocked(
                "Journal 缺少 selected Origin manifest".to_owned(),
            )
        })?;
    Ok((selected, manifest))
}

fn audit_takeover_v2_candidate(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    plan: &TakeoverV2Plan,
    journal: &TakeoverV2Journal,
) -> Result<CandidateAudit, TakeoverV2LifecycleError> {
    let (selected, manifest) = selected_origin_contract(plan, journal)?;
    let selected_root = open_selected_origin_root(paths, selected, manifest)?;
    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let staging_name = OsString::from(&journal.transaction_id);
    let staging_path = paths.staging_root().join(&staging_name);
    if entry_metadata_at(&staging_root, &staging_name)
        .map_err(|source| v2_io("检查 v2 接管临时目录", &staging_path, source))?
        .is_none()
    {
        return Ok(CandidateAudit {
            staging: None,
            complete: false,
        });
    }

    let (staging, staging_snapshot) =
        open_audited_candidate_directory(&staging_root, &staging_name, &staging_path, &[0o700])?;
    let staging_names = read_entry_names_os_from_handle(&staging)?;
    let mut staging_children = Vec::new();
    let complete = match staging_names.as_slice() {
        [] => false,
        [name] if name == OsStr::new("candidate") => {
            let (candidate, candidate_complete) =
                audit_candidate_container(&staging, &staging_path, plan, manifest, &selected_root)?;
            staging_children.push(CandidateEntryAudit::Directory(candidate));
            candidate_complete
        }
        _ => {
            return Err(candidate_blocked(
                "v2 接管临时目录包含未授权条目",
                &staging_path,
            ));
        }
    };
    let staging = finish_audited_candidate_directory(
        &staging_root,
        &staging_name,
        &staging_path,
        &staging,
        staging_snapshot,
        staging_children,
    )?;
    Ok(CandidateAudit {
        staging: Some(staging),
        complete,
    })
}

fn audit_takeover_v2_prepared_staging(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    plan: &TakeoverV2Plan,
    journal: &TakeoverV2Journal,
) -> Result<PreparedStagingAudit, TakeoverV2LifecycleError> {
    let (selected, manifest) = selected_origin_contract(plan, journal)?;
    let selected_root = open_selected_origin_root(paths, selected, manifest)?;
    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let staging_name = OsString::from(&journal.transaction_id);
    let staging_path = paths.staging_root().join(&staging_name);
    if entry_metadata_at(&staging_root, &staging_name)
        .map_err(|source| v2_io("检查 prepared staging", &staging_path, source))?
        .is_none()
    {
        return Ok(PreparedStagingAudit {
            kind: PreparedStagingKind::Missing,
            staging: None,
        });
    }

    let (staging, staging_snapshot) =
        open_audited_candidate_directory(&staging_root, &staging_name, &staging_path, &[0o700])?;
    let names = read_entry_names_os_from_handle(&staging)?;
    let mut children = Vec::new();
    let kind = match names.as_slice() {
        [] => PreparedStagingKind::Empty,
        [candidate_name] if candidate_name == OsStr::new("candidate") => {
            let (candidate, complete) =
                audit_candidate_container(&staging, &staging_path, plan, manifest, &selected_root)?;
            children.push(CandidateEntryAudit::Directory(candidate));
            PreparedStagingKind::Candidate { complete }
        }
        [bundle_name, candidate_name]
            if bundle_name == OsStr::new("bundle") && candidate_name == OsStr::new("candidate") =>
        {
            let (candidate, complete) =
                audit_candidate_container(&staging, &staging_path, plan, manifest, &selected_root)?;
            if !complete {
                return Err(candidate_blocked(
                    "B/C staging 中的 Candidate 必须完整",
                    &staging_path,
                ));
            }
            let (bundle, skeleton_kind) =
                audit_bundle_skeleton(&staging, &staging_path.join("bundle"))?;
            children.push(CandidateEntryAudit::Directory(bundle));
            children.push(CandidateEntryAudit::Directory(candidate));
            match skeleton_kind {
                BundleSkeletonKind::Empty => PreparedStagingKind::CandidateWithEmptyBundle,
                BundleSkeletonKind::Contents => PreparedStagingKind::CandidateWithContents,
            }
        }
        [bundle_name] if bundle_name == OsStr::new("bundle") => {
            let bundle = audit_complete_bundle_container(
                &staging,
                bundle_name,
                &staging_path.join(bundle_name),
                plan,
                manifest,
                &selected_root,
            )?;
            children.push(CandidateEntryAudit::Directory(bundle));
            PreparedStagingKind::StagedBundle
        }
        _ => {
            return Err(candidate_blocked(
                "prepared staging 包含未知、共存或非法条目",
                &staging_path,
            ));
        }
    };
    let staging = finish_audited_candidate_directory(
        &staging_root,
        &staging_name,
        &staging_path,
        &staging,
        staging_snapshot,
        children,
    )?;
    Ok(PreparedStagingAudit {
        kind,
        staging: Some(staging),
    })
}

fn audit_takeover_v2_exact_staged_bundle(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    plan: &TakeoverV2Plan,
    journal: &TakeoverV2Journal,
) -> Result<PreparedStagingAudit, TakeoverV2LifecycleError> {
    let audit = audit_takeover_v2_prepared_staging(paths, lifecycle_lock, plan, journal)?;
    if audit.kind != PreparedStagingKind::StagedBundle {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "原子发布只接受只含完整 Bundle 的 D shape staging".to_owned(),
        ));
    }
    Ok(audit)
}

fn audit_bundle_skeleton(
    staging: &File,
    path: &Path,
) -> Result<(CandidateDirectoryAudit, BundleSkeletonKind), TakeoverV2LifecycleError> {
    let name = OsStr::new("bundle");
    let (bundle, bundle_snapshot) =
        open_audited_candidate_directory(staging, name, path, &[0o700])?;
    let names = read_entry_names_os_from_handle(&bundle)?;
    let (kind, children) = match names.as_slice() {
        [] => (BundleSkeletonKind::Empty, Vec::new()),
        [contents_name] if contents_name == OsStr::new("contents") => {
            let contents_path = path.join(contents_name);
            let (contents, contents_snapshot) =
                open_audited_candidate_directory(&bundle, contents_name, &contents_path, &[0o700])?;
            if !read_entry_names_os_from_handle(&contents)?.is_empty() {
                return Err(candidate_blocked(
                    "Candidate 与 staged Bundle 内容不能共存",
                    &contents_path,
                ));
            }
            let contents = finish_audited_candidate_directory(
                &bundle,
                contents_name,
                &contents_path,
                &contents,
                contents_snapshot,
                Vec::new(),
            )?;
            (
                BundleSkeletonKind::Contents,
                vec![CandidateEntryAudit::Directory(contents)],
            )
        }
        _ => {
            return Err(candidate_blocked(
                "staged Bundle skeleton 包含未知条目",
                path,
            ));
        }
    };
    Ok((
        finish_audited_candidate_directory(
            staging,
            name,
            path,
            &bundle,
            bundle_snapshot,
            children,
        )?,
        kind,
    ))
}

fn audit_candidate_container(
    staging: &File,
    staging_path: &Path,
    plan: &TakeoverV2Plan,
    manifest: &TakeoverV2OriginManifest,
    selected_root: &SelectedOriginRoot,
) -> Result<(CandidateDirectoryAudit, bool), TakeoverV2LifecycleError> {
    let name = OsStr::new("candidate");
    let path = staging_path.join(name);
    audit_member_content_container(
        staging,
        name,
        &path,
        plan,
        manifest,
        selected_root,
        "v2 接管 candidate 包含未授权条目",
    )
}

fn audit_member_content_container(
    parent: &File,
    name: &OsStr,
    path: &Path,
    plan: &TakeoverV2Plan,
    manifest: &TakeoverV2OriginManifest,
    selected_root: &SelectedOriginRoot,
    invalid_entries_message: &'static str,
) -> Result<(CandidateDirectoryAudit, bool), TakeoverV2LifecycleError> {
    let (candidate, snapshot) = open_audited_candidate_directory(parent, name, path, &[0o700])?;
    let names = read_entry_names_os_from_handle(&candidate)?;
    let mut children = Vec::new();
    let complete = match names.as_slice() {
        [] => false,
        [member_name] if member_name == OsStr::new("members") => {
            let (members, members_complete) =
                audit_candidate_members(&candidate, path, plan, manifest, selected_root)?;
            children.push(CandidateEntryAudit::Directory(members));
            members_complete
        }
        _ => {
            return Err(candidate_blocked(invalid_entries_message, path));
        }
    };
    Ok((
        finish_audited_candidate_directory(parent, name, path, &candidate, snapshot, children)?,
        complete,
    ))
}

fn audit_candidate_members(
    candidate: &File,
    candidate_path: &Path,
    plan: &TakeoverV2Plan,
    manifest: &TakeoverV2OriginManifest,
    selected_root: &SelectedOriginRoot,
) -> Result<(CandidateDirectoryAudit, bool), TakeoverV2LifecycleError> {
    let name = OsStr::new("members");
    let path = candidate_path.join(name);
    let (members, snapshot) = open_audited_candidate_directory(candidate, name, &path, &[0o700])?;
    let names = read_entry_names_os_from_handle(&members)?;
    let mut children = Vec::new();
    let complete = match names.as_slice() {
        [] => false,
        [skill_name] if skill_name == OsStr::new(&plan.skill_name) => {
            let (skill, skill_complete) =
                audit_candidate_skill(&members, &path, skill_name, manifest, selected_root)?;
            children.push(CandidateEntryAudit::Directory(skill));
            skill_complete
        }
        _ => {
            return Err(candidate_blocked("v2 接管 members 包含未授权 Skill", &path));
        }
    };
    Ok((
        finish_audited_candidate_directory(candidate, name, &path, &members, snapshot, children)?,
        complete,
    ))
}

fn audit_complete_bundle_container(
    parent: &File,
    name: &OsStr,
    path: &Path,
    plan: &TakeoverV2Plan,
    manifest: &TakeoverV2OriginManifest,
    selected_root: &SelectedOriginRoot,
) -> Result<CandidateDirectoryAudit, TakeoverV2LifecycleError> {
    let (bundle, bundle_snapshot) = open_audited_candidate_directory(parent, name, path, &[0o700])?;
    if read_entry_names_os_from_handle(&bundle)? != [OsString::from("contents")] {
        return Err(candidate_blocked(
            "v2 接管 Bundle 包含 current 或其他未授权条目",
            path,
        ));
    }

    let contents_name = OsStr::new("contents");
    let contents_path = path.join(contents_name);
    let (contents, contents_snapshot) =
        open_audited_candidate_directory(&bundle, contents_name, &contents_path, &[0o700])?;
    let content_name = OsStr::new(&plan.content_id);
    if read_entry_names_os_from_handle(&contents)? != [content_name.to_os_string()] {
        return Err(candidate_blocked(
            "v2 接管 contents 不符合唯一 content_id 合同",
            &contents_path,
        ));
    }

    let content_path = contents_path.join(content_name);
    let (content, complete) = audit_member_content_container(
        &contents,
        content_name,
        &content_path,
        plan,
        manifest,
        selected_root,
        "v2 接管 content 包含未授权条目",
    )?;
    if !complete {
        return Err(candidate_blocked(
            "v2 接管 staged Bundle 不是完整合同内容",
            &content_path,
        ));
    }
    let contents = finish_audited_candidate_directory(
        &bundle,
        contents_name,
        &contents_path,
        &contents,
        contents_snapshot,
        vec![CandidateEntryAudit::Directory(content)],
    )?;
    finish_audited_candidate_directory(
        parent,
        name,
        path,
        &bundle,
        bundle_snapshot,
        vec![CandidateEntryAudit::Directory(contents)],
    )
}

fn audit_candidate_skill(
    members: &File,
    members_path: &Path,
    skill_name: &OsStr,
    manifest: &TakeoverV2OriginManifest,
    selected_root: &SelectedOriginRoot,
) -> Result<(CandidateDirectoryAudit, bool), TakeoverV2LifecycleError> {
    let path = members_path.join(skill_name);
    let final_permissions = manifest.root_mode & 0o7777;
    let (skill, snapshot) =
        open_audited_candidate_directory(members, skill_name, &path, &[0o700, final_permissions])?;
    let expected = manifest
        .entries
        .iter()
        .map(|entry| (entry.relative_path_hex().to_owned(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let (children, state) = audit_candidate_skill_entries(
        &skill,
        Vec::new(),
        &path,
        &expected,
        &mut seen,
        selected_root,
    )?;
    let root_has_final_permissions = permission_bits(snapshot.mode) == final_permissions;
    if state.has_distinguishable_final_directory && !state.content_complete {
        return Err(candidate_blocked(
            "v2 接管候选已有目录使用最终权限但整棵内容并不完整",
            &path,
        ));
    }
    if final_permissions != 0o700
        && root_has_final_permissions
        && (!state.content_complete || !state.all_directories_final)
    {
        return Err(candidate_blocked(
            "v2 接管候选根目录已使用最终权限但整棵目录尚未完成 chmod",
            &path,
        ));
    }
    let skill =
        finish_audited_candidate_directory(members, skill_name, &path, &skill, snapshot, children)?;
    Ok((
        skill,
        state.content_complete && state.all_directories_final && root_has_final_permissions,
    ))
}

fn audit_candidate_skill_entries(
    directory: &File,
    prefix: Vec<u8>,
    visible_path: &Path,
    expected: &BTreeMap<String, &TakeoverOriginalEntry>,
    seen: &mut BTreeSet<String>,
    selected_root: &SelectedOriginRoot,
) -> Result<(Vec<CandidateEntryAudit>, CandidateSkillAuditState), TakeoverV2LifecycleError> {
    let seen_before = seen.len();
    let expected_descendants = if prefix.is_empty() {
        expected.len()
    } else {
        let descendant_prefix = format!("{}2f", hex_bytes(&prefix));
        expected
            .keys()
            .filter(|path| path.starts_with(&descendant_prefix))
            .count()
    };
    let mut audited = Vec::new();
    let mut state = CandidateSkillAuditState {
        content_complete: true,
        all_directories_final: true,
        has_distinguishable_final_directory: false,
    };
    for name in read_entry_names_os_from_handle(directory)? {
        let relative = join_relative_bytes(&prefix, &name);
        let key = hex_bytes(&relative);
        let child_path = visible_path.join(&name);
        let expected_entry = expected
            .get(&key)
            .ok_or_else(|| candidate_blocked("v2 接管候选包含未授权条目", &child_path))?;
        if !seen.insert(key) {
            return Err(candidate_blocked(
                "v2 接管候选 manifest 出现重复路径",
                &child_path,
            ));
        }
        let metadata = entry_metadata_at(directory, &name)
            .map_err(|source| v2_io("检查 v2 接管候选条目", &child_path, source))?
            .ok_or_else(|| candidate_blocked("v2 接管候选条目在审计期间消失", &child_path))?;
        match metadata.st_mode & libc::S_IFMT {
            libc::S_IFDIR if expected_entry.is_directory() => {
                let final_permissions = expected_entry.mode() & 0o7777;
                let (child, snapshot) = open_audited_candidate_directory(
                    directory,
                    &name,
                    &child_path,
                    &[0o700, final_permissions],
                )?;
                let has_final_permissions = permission_bits(snapshot.mode) == final_permissions;
                let (children, child_state) = audit_candidate_skill_entries(
                    &child,
                    relative,
                    &child_path,
                    expected,
                    seen,
                    selected_root,
                )?;
                state.content_complete &= child_state.content_complete;
                state.all_directories_final &=
                    has_final_permissions && child_state.all_directories_final;
                state.has_distinguishable_final_directory |= child_state
                    .has_distinguishable_final_directory
                    || final_permissions != 0o700 && has_final_permissions;
                audited.push(CandidateEntryAudit::Directory(
                    finish_audited_candidate_directory(
                        directory,
                        &name,
                        &child_path,
                        &child,
                        snapshot,
                        children,
                    )?,
                ));
            }
            libc::S_IFREG if expected_entry.is_file() => {
                let (entry, complete) = audit_candidate_file(
                    directory,
                    &name,
                    &child_path,
                    &relative,
                    expected_entry,
                    selected_root,
                    &metadata,
                )?;
                if !complete {
                    state.content_complete = false;
                }
                audited.push(entry);
            }
            _ => {
                return Err(candidate_blocked(
                    "v2 接管候选包含错误类型、软链接、硬链接或特殊文件",
                    &child_path,
                ));
            }
        }
    }
    let seen_descendants = seen.len().saturating_sub(seen_before);
    state.content_complete &= seen_descendants == expected_descendants;
    Ok((audited, state))
}

fn audit_candidate_file(
    parent: &File,
    name: &OsStr,
    path: &Path,
    relative: &[u8],
    expected: &TakeoverOriginalEntry,
    selected_root: &SelectedOriginRoot,
    metadata: &libc::stat,
) -> Result<(CandidateEntryAudit, bool), TakeoverV2LifecycleError> {
    let final_permissions = expected.mode() & 0o7777;
    let actual_permissions = permission_bits(u32::from(metadata.st_mode));
    let actual_size = u64::try_from(metadata.st_size)
        .map_err(|_| candidate_blocked("v2 接管候选文件大小无效", path))?;
    // 只有完整文件可以进入最终权限；部分内容必须保持 copy primitive 的 0600 协议。
    let permissions_allowed = if actual_size < expected.size() {
        actual_permissions == 0o600
    } else {
        matches!(actual_permissions, 0o600) || actual_permissions == final_permissions
    };
    if metadata.st_nlink != 1 || !permissions_allowed || actual_size > expected.size() {
        return Err(candidate_blocked(
            "v2 接管候选文件的链接数、权限或大小不符合合同",
            path,
        ));
    }

    let snapshot = journal_snapshot_from_stat(metadata);
    let mut candidate = open_regular_file_at(parent, name, path, false)?;
    let opened = candidate
        .metadata()
        .map_err(|source| v2_io("检查已打开的 v2 接管候选文件", path, source))?;
    if journal_snapshot_from_metadata(&opened) != snapshot {
        return Err(candidate_blocked("v2 接管候选文件在打开期间被替换", path));
    }

    if actual_size == expected.size() {
        let expected_hash = expected
            .content_sha256()
            .ok_or_else(|| candidate_blocked("v2 接管文件 manifest 缺少内容摘要", path))?;
        if hash_open_file(&mut candidate, path)? != expected_hash {
            return Err(candidate_blocked(
                "v2 接管候选完整文件内容不符合 manifest",
                path,
            ));
        }
    } else {
        compare_candidate_prefix_with_selected_origin(
            &mut candidate,
            actual_size,
            relative,
            expected,
            selected_root,
            path,
        )?;
    }

    recheck_audited_file(parent, name, path, &candidate, snapshot)?;
    Ok((
        CandidateEntryAudit::File {
            name: name.to_os_string(),
            path: path.to_path_buf(),
            snapshot,
        },
        actual_size == expected.size() && actual_permissions == final_permissions,
    ))
}

fn hash_open_file(file: &mut File, path: &Path) -> Result<String, TakeoverV2LifecycleError> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|source| v2_io("读取 v2 接管候选文件", path, source))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex_bytes(&hasher.finalize()))
}

fn compare_candidate_prefix_with_selected_origin(
    candidate: &mut File,
    candidate_size: u64,
    relative: &[u8],
    expected: &TakeoverOriginalEntry,
    selected_root: &SelectedOriginRoot,
    candidate_path: &Path,
) -> Result<(), TakeoverV2LifecycleError> {
    let (source_parent, source_name, source_path) =
        open_selected_source_parent(selected_root, relative)?;
    let source_metadata = entry_metadata_at(&source_parent, &source_name)
        .map_err(|source| v2_io("检查 selected Origin 文件", &source_path, source))?
        .ok_or_else(|| {
            TakeoverV2LifecycleError::OriginChanged(source_path.display().to_string())
        })?;
    if source_metadata.st_mode & libc::S_IFMT != libc::S_IFREG
        || source_metadata.st_nlink != 1
        || !expected.matches_original_stat(&source_metadata)
    {
        return Err(TakeoverV2LifecycleError::OriginChanged(
            source_path.display().to_string(),
        ));
    }
    let mut source = open_regular_file_at(&source_parent, &source_name, &source_path, false)?;
    let source_opened = source
        .metadata()
        .map_err(|error| v2_io("检查已打开的 selected Origin 文件", &source_path, error))?;
    if !expected.matches_original_metadata(&source_opened) {
        return Err(TakeoverV2LifecycleError::OriginChanged(
            source_path.display().to_string(),
        ));
    }

    let mut remaining = candidate_size;
    let mut candidate_buffer = [0_u8; 64 * 1024];
    let mut source_buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let limit = usize::try_from(remaining.min(candidate_buffer.len() as u64))
            .expect("buffer 大小可转换为 usize");
        let candidate_count = candidate
            .read(&mut candidate_buffer[..limit])
            .map_err(|error| v2_io("读取部分 v2 接管候选文件", candidate_path, error))?;
        if candidate_count == 0 {
            return Err(candidate_blocked(
                "v2 接管候选文件大小在读取期间缩短",
                candidate_path,
            ));
        }
        source
            .read_exact(&mut source_buffer[..candidate_count])
            .map_err(|_| {
                TakeoverV2LifecycleError::OriginChanged(source_path.display().to_string())
            })?;
        if candidate_buffer[..candidate_count] != source_buffer[..candidate_count] {
            return Err(candidate_blocked(
                "v2 接管候选半写文件不是 selected Origin 的正确前缀",
                candidate_path,
            ));
        }
        remaining -= candidate_count as u64;
    }
    let mut extra = [0_u8; 1];
    if candidate
        .read(&mut extra)
        .map_err(|error| v2_io("确认部分 v2 接管候选文件边界", candidate_path, error))?
        != 0
    {
        return Err(candidate_blocked(
            "v2 接管候选文件大小在读取期间增长",
            candidate_path,
        ));
    }

    let source_after = source
        .metadata()
        .map_err(|error| v2_io("重新检查 selected Origin 文件", &source_path, error))?;
    let source_visible = entry_metadata_at(&source_parent, &source_name)
        .map_err(|error| v2_io("重新检查可见 selected Origin 文件", &source_path, error))?
        .ok_or_else(|| {
            TakeoverV2LifecycleError::OriginChanged(source_path.display().to_string())
        })?;
    if !expected.matches_original_metadata(&source_after)
        || !expected.matches_original_stat(&source_visible)
    {
        return Err(TakeoverV2LifecycleError::OriginChanged(
            source_path.display().to_string(),
        ));
    }
    Ok(())
}

fn open_selected_origin_root(
    paths: &ApplicationPaths,
    origin: &TakeoverV2Origin,
    manifest: &TakeoverV2OriginManifest,
) -> Result<SelectedOriginRoot, TakeoverV2LifecycleError> {
    let parent = open_verified_origin_parent(paths, origin)?;
    let visible_path = PathBuf::from(&origin.original_path);
    let metadata = entry_metadata_at(&parent.handle, &parent.leaf)
        .map_err(|source| v2_io("检查 selected Origin 根目录", &visible_path, source))?
        .ok_or_else(|| {
            TakeoverV2LifecycleError::OriginChanged(visible_path.display().to_string())
        })?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
        || metadata.st_dev as u64 != manifest.root_device
        || metadata.st_ino != manifest.root_inode
        || u32::from(metadata.st_mode) != manifest.root_mode
    {
        return Err(TakeoverV2LifecycleError::OriginChanged(
            visible_path.display().to_string(),
        ));
    }
    let handle = open_directory_at(&parent.handle, &parent.leaf)
        .map_err(|source| v2_io("打开 selected Origin 根目录", &visible_path, source))?;
    let opened = handle
        .metadata()
        .map_err(|source| v2_io("检查已打开的 selected Origin 根目录", &visible_path, source))?;
    if (opened.dev(), opened.ino(), opened.mode())
        != (
            manifest.root_device,
            manifest.root_inode,
            manifest.root_mode,
        )
    {
        return Err(TakeoverV2LifecycleError::OriginChanged(
            visible_path.display().to_string(),
        ));
    }
    parent.recheck(
        origin.parent_device,
        origin.parent_inode,
        origin.parent_mode,
    )?;
    Ok(SelectedOriginRoot {
        handle,
        visible_path,
    })
}

fn open_selected_source_parent(
    selected_root: &SelectedOriginRoot,
    relative: &[u8],
) -> Result<(File, OsString, PathBuf), TakeoverV2LifecycleError> {
    let components = relative.split(|byte| *byte == b'/').collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| component.is_empty() || matches!(*component, b"." | b".."))
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 接管 manifest 包含非法相对路径".to_owned(),
        ));
    }
    let mut parent = selected_root.handle.try_clone().map_err(|source| {
        v2_io(
            "保留 selected Origin 根目录",
            &selected_root.visible_path,
            source,
        )
    })?;
    let mut visible = selected_root.visible_path.clone();
    for component in &components[..components.len() - 1] {
        let name = OsString::from_vec(component.to_vec());
        visible.push(&name);
        parent = open_directory_at(&parent, &name)
            .map_err(|source| v2_io("打开 selected Origin 子目录", &visible, source))?;
    }
    let name = OsString::from_vec(
        components
            .last()
            .expect("非空 relative 至少有一个组件")
            .to_vec(),
    );
    visible.push(&name);
    Ok((parent, name, visible))
}

fn open_audited_candidate_directory(
    parent: &File,
    name: &OsStr,
    path: &Path,
    allowed_permissions: &[u32],
) -> Result<(File, JournalEntrySnapshot), TakeoverV2LifecycleError> {
    let metadata = entry_metadata_at(parent, name)
        .map_err(|source| v2_io("检查 v2 接管候选目录", path, source))?
        .ok_or_else(|| candidate_blocked("v2 接管候选目录在审计期间消失", path))?;
    let permissions = permission_bits(u32::from(metadata.st_mode));
    if metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
        || !allowed_permissions.contains(&permissions)
    {
        return Err(candidate_blocked(
            "v2 接管候选目录类型或权限不符合合同",
            path,
        ));
    }
    let snapshot = journal_snapshot_from_stat(&metadata);
    let directory = open_directory_at(parent, name)
        .map_err(|source| v2_io("打开 v2 接管候选目录", path, source))?;
    let opened = directory
        .metadata()
        .map_err(|source| v2_io("检查已打开的 v2 接管候选目录", path, source))?;
    if journal_snapshot_from_metadata(&opened) != snapshot {
        return Err(candidate_blocked("v2 接管候选目录在打开期间被替换", path));
    }
    Ok((directory, snapshot))
}

fn finish_audited_candidate_directory(
    parent: &File,
    name: &OsStr,
    path: &Path,
    directory: &File,
    snapshot: JournalEntrySnapshot,
    children: Vec<CandidateEntryAudit>,
) -> Result<CandidateDirectoryAudit, TakeoverV2LifecycleError> {
    let opened = directory
        .metadata()
        .map_err(|source| v2_io("重新检查已打开的 v2 接管候选目录", path, source))?;
    let visible = entry_metadata_at(parent, name)
        .map_err(|source| v2_io("重新检查可见 v2 接管候选目录", path, source))?
        .ok_or_else(|| candidate_blocked("v2 接管候选目录在审计期间消失", path))?;
    if journal_snapshot_from_metadata(&opened) != snapshot
        || journal_snapshot_from_stat(&visible) != snapshot
    {
        return Err(candidate_blocked(
            "v2 接管候选目录在只读审计期间发生变化",
            path,
        ));
    }
    Ok(CandidateDirectoryAudit {
        name: name.to_os_string(),
        path: path.to_path_buf(),
        snapshot,
        children,
    })
}

fn recheck_audited_file(
    parent: &File,
    name: &OsStr,
    path: &Path,
    file: &File,
    snapshot: JournalEntrySnapshot,
) -> Result<(), TakeoverV2LifecycleError> {
    let opened = file
        .metadata()
        .map_err(|source| v2_io("重新检查已打开的 v2 接管候选文件", path, source))?;
    let visible = entry_metadata_at(parent, name)
        .map_err(|source| v2_io("重新检查可见 v2 接管候选文件", path, source))?
        .ok_or_else(|| candidate_blocked("v2 接管候选文件在审计期间消失", path))?;
    if journal_snapshot_from_metadata(&opened) != snapshot
        || journal_snapshot_from_stat(&visible) != snapshot
    {
        return Err(candidate_blocked(
            "v2 接管候选文件在只读审计期间发生变化",
            path,
        ));
    }
    Ok(())
}

fn join_relative_bytes(prefix: &[u8], name: &OsStr) -> Vec<u8> {
    let mut relative =
        Vec::with_capacity(prefix.len() + usize::from(!prefix.is_empty()) + name.len());
    relative.extend_from_slice(prefix);
    if !relative.is_empty() {
        relative.push(b'/');
    }
    relative.extend_from_slice(name.as_bytes());
    relative
}

fn permission_bits(mode: u32) -> u32 {
    mode & 0o7777
}

fn candidate_blocked(message: &str, path: &Path) -> TakeoverV2LifecycleError {
    TakeoverV2LifecycleError::RecoveryBlocked(format!("{message}：{}", path.display()))
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn remove_audited_takeover_v2_candidate(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    audit: &CandidateAudit,
) -> Result<(), TakeoverV2LifecycleError> {
    let mut no_hook = |_: &Path| {};
    remove_audited_takeover_v2_candidate_with_hook(paths, lifecycle_lock, audit, &mut no_hook)
}

fn remove_audited_takeover_v2_candidate_with_hook(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    audit: &CandidateAudit,
    before_remove: &mut dyn FnMut(&Path),
) -> Result<(), TakeoverV2LifecycleError> {
    let Some(staging) = &audit.staging else {
        return Ok(());
    };
    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    remove_audited_candidate_directory(&staging_root, staging, before_remove)?;
    staging_root
        .sync_all()
        .map_err(|source| v2_io("同步已清理的 v2 staging", &paths.staging_root(), source))?;
    Ok(())
}

fn cleanup_prepared_staging_before_publish(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    plan: &TakeoverV2Plan,
    journal: &TakeoverV2Journal,
) -> Result<(), TakeoverV2LifecycleError> {
    loop {
        let audit = audit_takeover_v2_prepared_staging(paths, lifecycle_lock, plan, journal)?;
        match audit.kind {
            PreparedStagingKind::Missing => return Ok(()),
            PreparedStagingKind::Empty => {
                remove_audited_takeover_v2_candidate(
                    paths,
                    lifecycle_lock,
                    &CandidateAudit {
                        staging: audit.staging,
                        complete: false,
                    },
                )?;
            }
            PreparedStagingKind::Candidate { complete } => {
                remove_audited_takeover_v2_candidate(
                    paths,
                    lifecycle_lock,
                    &CandidateAudit {
                        staging: audit.staging,
                        complete,
                    },
                )?;
            }
            PreparedStagingKind::CandidateWithEmptyBundle => {
                remove_prepared_empty_bundle(paths, lifecycle_lock, &audit)?;
            }
            PreparedStagingKind::CandidateWithContents => {
                remove_prepared_empty_contents(paths, lifecycle_lock, &audit)?;
            }
            PreparedStagingKind::StagedBundle => {
                restore_staged_content_to_candidate(paths, lifecycle_lock, plan, &audit)?;
            }
        }
    }
}

fn restore_staged_content_to_candidate(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    plan: &TakeoverV2Plan,
    audit: &PreparedStagingAudit,
) -> Result<(), TakeoverV2LifecycleError> {
    let staging_audit = audit.staging.as_ref().ok_or_else(|| {
        TakeoverV2LifecycleError::RecoveryBlocked("D shape 缺少 staging audit".to_owned())
    })?;
    let bundle_audit = audited_child_directory(staging_audit, OsStr::new("bundle"))?;
    let contents_audit = audited_child_directory(bundle_audit, OsStr::new("contents"))?;
    let content_audit = audited_child_directory(contents_audit, OsStr::new(&plan.content_id))?;
    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let staging = open_matching_audited_directory(&staging_root, staging_audit)?;
    let bundle = open_matching_audited_directory(&staging, bundle_audit)?;
    let contents = open_matching_audited_directory(&bundle, contents_audit)?;
    if entry_metadata_at(&staging, OsStr::new("candidate"))
        .map_err(|source| v2_io("检查恢复 Candidate 目标", &staging_audit.path, source))?
        .is_some()
    {
        return Err(candidate_blocked(
            "D shape 恢复时 Candidate 目标已被占用",
            &staging_audit.path,
        ));
    }
    let visible_content = entry_metadata_at(&contents, &content_audit.name)
        .map_err(|source| v2_io("复核 staged content", &content_audit.path, source))?
        .ok_or_else(|| candidate_blocked("staged content 在恢复前消失", &content_audit.path))?;
    if journal_snapshot_from_stat(&visible_content) != content_audit.snapshot {
        return Err(candidate_blocked(
            "staged content 在恢复前被替换",
            &content_audit.path,
        ));
    }
    rename_at_no_replace(
        &contents,
        &content_audit.name,
        &staging,
        OsStr::new("candidate"),
    )
    .map_err(|source| {
        v2_io(
            "把 staged content 恢复为 Candidate",
            &content_audit.path,
            source,
        )
    })?;
    contents.sync_all().map_err(|source| {
        v2_io(
            "同步已移出 content 的 contents",
            &contents_audit.path,
            source,
        )
    })?;
    bundle
        .sync_all()
        .map_err(|source| v2_io("同步回退中的 staged Bundle", &bundle_audit.path, source))?;
    staging
        .sync_all()
        .map_err(|source| v2_io("同步恢复后的 Candidate", &staging_audit.path, source))?;
    staging_root
        .sync_all()
        .map_err(|source| v2_io("同步 v2 staging 根", &paths.staging_root(), source))?;
    Ok(())
}

fn remove_prepared_empty_contents(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    audit: &PreparedStagingAudit,
) -> Result<(), TakeoverV2LifecycleError> {
    let staging_audit = audit.staging.as_ref().ok_or_else(|| {
        TakeoverV2LifecycleError::RecoveryBlocked("C shape 缺少 staging audit".to_owned())
    })?;
    let bundle_audit = audited_child_directory(staging_audit, OsStr::new("bundle"))?;
    let contents_audit = audited_child_directory(bundle_audit, OsStr::new("contents"))?;
    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let staging = open_matching_audited_directory(&staging_root, staging_audit)?;
    let bundle = open_matching_audited_directory(&staging, bundle_audit)?;
    let contents = open_matching_audited_directory(&bundle, contents_audit)?;
    if !read_entry_names_os_from_handle(&contents)?.is_empty() {
        return Err(candidate_blocked(
            "C shape 的 contents 在清理前不再为空",
            &contents_audit.path,
        ));
    }
    drop(contents);
    unlink_at(&bundle, &contents_audit.name, true)
        .map_err(|source| v2_io("清理空 staged contents", &contents_audit.path, source))?;
    bundle
        .sync_all()
        .map_err(|source| v2_io("同步 staged Bundle", &bundle_audit.path, source))?;
    staging
        .sync_all()
        .map_err(|source| v2_io("同步 C→B staging", &staging_audit.path, source))?;
    staging_root
        .sync_all()
        .map_err(|source| v2_io("同步 v2 staging 根", &paths.staging_root(), source))?;
    Ok(())
}

fn remove_prepared_empty_bundle(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    audit: &PreparedStagingAudit,
) -> Result<(), TakeoverV2LifecycleError> {
    let staging_audit = audit.staging.as_ref().ok_or_else(|| {
        TakeoverV2LifecycleError::RecoveryBlocked("B shape 缺少 staging audit".to_owned())
    })?;
    let bundle_audit = audited_child_directory(staging_audit, OsStr::new("bundle"))?;
    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let staging = open_matching_audited_directory(&staging_root, staging_audit)?;
    let bundle = open_matching_audited_directory(&staging, bundle_audit)?;
    if !read_entry_names_os_from_handle(&bundle)?.is_empty() {
        return Err(candidate_blocked(
            "B shape 的 Bundle 在清理前不再为空",
            &bundle_audit.path,
        ));
    }
    drop(bundle);
    unlink_at(&staging, &bundle_audit.name, true)
        .map_err(|source| v2_io("清理空 staged Bundle", &bundle_audit.path, source))?;
    staging
        .sync_all()
        .map_err(|source| v2_io("同步 B→A staging", &staging_audit.path, source))?;
    staging_root
        .sync_all()
        .map_err(|source| v2_io("同步 v2 staging 根", &paths.staging_root(), source))?;
    Ok(())
}

fn audited_child_directory<'a>(
    parent: &'a CandidateDirectoryAudit,
    name: &OsStr,
) -> Result<&'a CandidateDirectoryAudit, TakeoverV2LifecycleError> {
    parent
        .children
        .iter()
        .find_map(|entry| match entry {
            CandidateEntryAudit::Directory(directory) if directory.name == name => Some(directory),
            _ => None,
        })
        .ok_or_else(|| {
            candidate_blocked(
                "prepared staging audit 缺少预期目录",
                &parent.path.join(name),
            )
        })
}

fn open_matching_audited_directory(
    parent: &File,
    audit: &CandidateDirectoryAudit,
) -> Result<File, TakeoverV2LifecycleError> {
    let (directory, snapshot) =
        open_audited_candidate_directory(parent, &audit.name, &audit.path, &[0o700])?;
    if snapshot != audit.snapshot {
        return Err(candidate_blocked(
            "prepared staging 在变更前发生变化",
            &audit.path,
        ));
    }
    Ok(directory)
}

fn remove_audited_candidate_directory(
    parent: &File,
    audit: &CandidateDirectoryAudit,
    before_remove: &mut dyn FnMut(&Path),
) -> Result<(), TakeoverV2LifecycleError> {
    before_remove(&audit.path);
    let visible = entry_metadata_at(parent, &audit.name)
        .map_err(|source| v2_io("删除前检查 v2 接管候选目录", &audit.path, source))?
        .ok_or_else(|| candidate_blocked("v2 接管候选目录在删除前消失", &audit.path))?;
    if journal_snapshot_from_stat(&visible) != audit.snapshot {
        return Err(candidate_blocked(
            "v2 接管候选目录在删除前被替换",
            &audit.path,
        ));
    }
    let directory = open_directory_at(parent, &audit.name)
        .map_err(|source| v2_io("打开待删除的 v2 接管候选目录", &audit.path, source))?;
    let opened = directory
        .metadata()
        .map_err(|source| v2_io("检查待删除的 v2 接管候选目录", &audit.path, source))?;
    if journal_snapshot_from_metadata(&opened) != audit.snapshot {
        return Err(candidate_blocked(
            "v2 接管候选目录在删除打开期间被替换",
            &audit.path,
        ));
    }

    let expected_names = audit
        .children
        .iter()
        .map(candidate_audit_entry_name)
        .collect::<BTreeSet<_>>();
    let current_names = read_entry_names_os_from_handle(&directory)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    if current_names != expected_names {
        return Err(candidate_blocked(
            "v2 接管候选目录在删除前出现未知或缺失条目",
            &audit.path,
        ));
    }

    let mut trusted_snapshot = audit.snapshot;
    // 整棵 Candidate 已完成只读审计，此后才允许放宽本事务目录权限。
    if permission_bits(opened.mode()) != 0o700 {
        directory
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|source| v2_io("准备删除 v2 接管候选目录", &audit.path, source))?;
        trusted_snapshot = refresh_candidate_directory_snapshot_after_owned_change(
            parent,
            audit,
            &directory,
            trusted_snapshot,
        )?;
    }
    for child in &audit.children {
        recheck_candidate_directory_snapshot(parent, audit, &directory, trusted_snapshot)?;
        match child {
            CandidateEntryAudit::Directory(child) => {
                remove_audited_candidate_directory(&directory, child, before_remove)?;
            }
            CandidateEntryAudit::File {
                name,
                path,
                snapshot,
            } => {
                before_remove(path);
                // 测试 hook 代表任意外部并发修改；删除子项前再次核对父目录完整快照。
                recheck_candidate_directory_snapshot(parent, audit, &directory, trusted_snapshot)?;
                let current = entry_metadata_at(&directory, name)
                    .map_err(|source| v2_io("删除前检查 v2 接管候选文件", path, source))?
                    .ok_or_else(|| candidate_blocked("v2 接管候选文件在删除前消失", path))?;
                if current.st_mode & libc::S_IFMT != libc::S_IFREG
                    || current.st_nlink != 1
                    || journal_snapshot_from_stat(&current) != *snapshot
                {
                    return Err(candidate_blocked("v2 接管候选文件在删除前被替换", path));
                }
                unlink_at(&directory, name, false)
                    .map_err(|source| v2_io("删除已验证的 v2 接管候选文件", path, source))?;
            }
        }
        // chmod 和删除子项是本事务唯一允许改变目录 metadata 的操作。
        trusted_snapshot = refresh_candidate_directory_snapshot_after_owned_change(
            parent,
            audit,
            &directory,
            trusted_snapshot,
        )?;
    }
    recheck_candidate_directory_snapshot(parent, audit, &directory, trusted_snapshot)?;
    if !read_entry_names_os_from_handle(&directory)?.is_empty() {
        return Err(candidate_blocked(
            "v2 接管候选目录在删除期间出现新内容",
            &audit.path,
        ));
    }
    directory
        .sync_all()
        .map_err(|source| v2_io("同步已清理的 v2 接管候选目录", &audit.path, source))?;
    recheck_candidate_directory_snapshot(parent, audit, &directory, trusted_snapshot)?;
    drop(directory);
    let before_unlink = entry_metadata_at(parent, &audit.name)
        .map_err(|source| v2_io("移除前检查 v2 接管候选目录", &audit.path, source))?
        .ok_or_else(|| candidate_blocked("v2 接管候选目录在移除前消失", &audit.path))?;
    if journal_snapshot_from_stat(&before_unlink) != trusted_snapshot {
        return Err(candidate_blocked(
            "v2 接管候选目录在最终移除前发生变化",
            &audit.path,
        ));
    }
    unlink_at(parent, &audit.name, true)
        .map_err(|source| v2_io("移除已验证的 v2 接管候选目录", &audit.path, source))?;
    parent
        .sync_all()
        .map_err(|source| v2_io("同步 v2 接管候选父目录", &audit.path, source))?;
    Ok(())
}

fn recheck_candidate_directory_snapshot(
    parent: &File,
    audit: &CandidateDirectoryAudit,
    directory: &File,
    expected: JournalEntrySnapshot,
) -> Result<(), TakeoverV2LifecycleError> {
    let opened = directory
        .metadata()
        .map_err(|source| v2_io("重新检查待删除的 v2 接管候选目录", &audit.path, source))?;
    let visible = entry_metadata_at(parent, &audit.name)
        .map_err(|source| v2_io("重新检查可见 v2 接管候选目录", &audit.path, source))?
        .ok_or_else(|| candidate_blocked("v2 接管候选目录在删除期间消失", &audit.path))?;
    if journal_snapshot_from_metadata(&opened) != expected
        || journal_snapshot_from_stat(&visible) != expected
    {
        return Err(candidate_blocked(
            "v2 接管候选目录在删除期间发生未授权变化",
            &audit.path,
        ));
    }
    Ok(())
}

fn refresh_candidate_directory_snapshot_after_owned_change(
    parent: &File,
    audit: &CandidateDirectoryAudit,
    directory: &File,
    previous: JournalEntrySnapshot,
) -> Result<JournalEntrySnapshot, TakeoverV2LifecycleError> {
    let opened = directory
        .metadata()
        .map_err(|source| v2_io("刷新待删除的 v2 接管候选目录", &audit.path, source))?;
    let visible = entry_metadata_at(parent, &audit.name)
        .map_err(|source| v2_io("刷新可见 v2 接管候选目录", &audit.path, source))?
        .ok_or_else(|| candidate_blocked("v2 接管候选目录在删除期间消失", &audit.path))?;
    let opened_snapshot = journal_snapshot_from_metadata(&opened);
    let visible_snapshot = journal_snapshot_from_stat(&visible);
    if opened_snapshot != visible_snapshot
        || opened_snapshot.device != previous.device
        || opened_snapshot.inode != previous.inode
        || opened_snapshot.mode & libc::S_IFMT as u32 != libc::S_IFDIR as u32
        || permission_bits(opened_snapshot.mode) != 0o700
    {
        return Err(candidate_blocked(
            "v2 接管候选目录在授权变更期间发生额外变化",
            &audit.path,
        ));
    }
    Ok(opened_snapshot)
}

fn candidate_audit_entry_name(entry: &CandidateEntryAudit) -> OsString {
    match entry {
        CandidateEntryAudit::Directory(directory) => directory.name.clone(),
        CandidateEntryAudit::File { name, .. } => name.clone(),
    }
}

fn ensure_takeover_v2_staging_absent_without_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    transaction_id: &str,
) -> Result<(), TakeoverV2LifecycleError> {
    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let name = OsStr::new(transaction_id);
    let path = paths.staging_root().join(name);
    if entry_metadata_at(&staging_root, name)
        .map_err(|source| v2_io("检查无 Journal 的 v2 staging", &path, source))?
        .is_some()
    {
        return Err(candidate_blocked(
            "正式 Journal 尚未建立，但事务 staging 已存在",
            &path,
        ));
    }
    Ok(())
}

/// 启动恢复只处理生效点之前的窗口；未知或后续阶段统一隔离，绝不猜测。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn recover_pre_effect_takeover_v2_transactions(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    now: i64,
) -> Result<(), TakeoverV2LifecycleError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    for transaction in storage.recoverable_takeover_v2_transactions()? {
        if transaction.status == "blocked" {
            continue;
        }
        if let Err(error) = recover_pre_effect_takeover_v2_transaction(
            paths,
            &lifecycle_lock,
            storage,
            &transaction,
            now,
        ) {
            storage.block_takeover_v2_transaction(&transaction.id, &error.to_string(), now)?;
        }
        lifecycle_lock.recheck(paths)?;
    }
    Ok(())
}

fn recover_pre_effect_takeover_v2_transaction(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transaction: &StoredTakeoverV2Transaction,
    now: i64,
) -> Result<(), TakeoverV2LifecycleError> {
    let mut no_phase_temp_hook = || Ok(());
    let mut no_prepared_hook = || Ok(());
    recover_pre_effect_takeover_v2_transaction_with_hook(
        paths,
        lifecycle_lock,
        storage,
        transaction,
        now,
        &mut no_phase_temp_hook,
        &mut no_prepared_hook,
    )
}

fn recover_pre_effect_takeover_v2_transaction_with_hook(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transaction: &StoredTakeoverV2Transaction,
    now: i64,
    after_phase_temp_removed: &mut dyn FnMut() -> Result<(), TakeoverV2LifecycleError>,
    before_prepared_commit: &mut dyn FnMut() -> Result<(), TakeoverV2LifecycleError>,
) -> Result<(), TakeoverV2LifecycleError> {
    if let Some(error) = &transaction.recovery_validation_error {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(error.clone()));
    }
    let plan = storage.read_takeover_v2_plan_for_transaction(transaction)?;
    if !matches!(
        transaction.phase.as_str(),
        "journal_pending" | "preparing" | "prepared"
    ) {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(format!(
            "事务已进入本批不能恢复的阶段：{}",
            transaction.phase
        )));
    }

    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    let initial_temporary =
        inspect_takeover_v2_initial_temporary_journal(paths, &journals, transaction, &plan)?;
    let name = OsString::from(journal_file_name(&transaction.id));
    let path = paths.journals_root().join(&name);
    let metadata = entry_metadata_at(&journals, &name)
        .map_err(|source| v2_io("检查正式 Journal", &path, source))?;

    if !matches!(transaction.status.as_str(), "in_progress" | "aborted") {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(format!(
            "事务状态不属于生效前恢复窗口：{}",
            transaction.status
        )));
    }

    match metadata {
        None if transaction.status == "aborted" && transaction.phase == "preparing" => {
            // 清理顺序是 staging → Journal → forget；两份文件证据都已消失时只剩幂等 forget。
            ensure_takeover_v2_phase_temp_absent(paths, &journals, &transaction.id)?;
            if initial_temporary.is_some() {
                return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                    "aborted preparing 缺少 formal 时不能认领残留首次 temp".to_owned(),
                ));
            }
            ensure_takeover_v2_staging_absent_without_journal(
                paths,
                lifecycle_lock,
                &transaction.id,
            )?;
            storage.forget_terminal_takeover_v2_transaction(&transaction.id)?;
        }
        None if transaction.phase == "journal_pending" => {
            // 正式 Journal 未建立时，协议还没有授权创建 staging；存在任何同名目录都必须保留。
            ensure_takeover_v2_phase_temp_absent(paths, &journals, &transaction.id)?;
            validate_all_origins_without_journal(paths, &plan)?;
            validate_all_targets(paths, &plan)?;
            ensure_takeover_v2_staging_absent_without_journal(
                paths,
                lifecycle_lock,
                &transaction.id,
            )?;
            if transaction.status == "in_progress" {
                storage.abort_takeover_v2_transaction(&transaction.id, None, now)?;
            }
            if let Some(temporary) = initial_temporary.as_ref() {
                remove_owned_journal(paths, &journals, temporary)?;
            }
            storage.forget_terminal_takeover_v2_transaction(&transaction.id)?;
        }
        Some(_) => {
            let (journal, owned, journal_bytes) =
                read_takeover_v2_journal_with_bytes_at(&journals, &name, &path)?;
            validate_takeover_v2_journal_contract(&journal, transaction, &plan)?;
            if journal.phase == TakeoverV2JournalPhase::Prepared
                && journal_bytes != serde_json::to_vec_pretty(&journal)?
            {
                return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                    "formal Prepared 不是 canonical pretty JSON".to_owned(),
                ));
            }
            if initial_temporary.is_some() {
                return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                    "formal Journal 已存在时不能认领首次 formal 的随机 temp".to_owned(),
                ));
            }
            let phase_temporary = inspect_takeover_v2_phase_temporary_journal(
                paths,
                &journals,
                transaction,
                &plan,
                &journal,
            )?;
            validate_all_origin_manifests(paths, &plan, &journal.origins)?;
            validate_all_targets(paths, &plan)?;
            if journal.phase == TakeoverV2JournalPhase::Prepared {
                let bundles_root = open_managed_directory_from_root(
                    paths,
                    lifecycle_lock.root(),
                    &paths.bundles_root(),
                )?;
                let final_name = OsString::from(&journal.bundle_id);
                let final_path = paths.bundles_root().join(&final_name);
                let final_exists = entry_metadata_at(&bundles_root, &final_name)
                    .map_err(|source| v2_io("检查恢复中的最终 Bundle", &final_path, source))?
                    .is_some();
                let audit =
                    audit_takeover_v2_prepared_staging(paths, lifecycle_lock, &plan, &journal)?;
                if final_exists {
                    if phase_temporary.is_some() {
                        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                            "最终 Bundle 与 phase temp 不能共存".to_owned(),
                        ));
                    }
                    if !matches!(
                        audit.kind,
                        PreparedStagingKind::Missing | PreparedStagingKind::Empty
                    ) {
                        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                            "staged 内容与最终 Bundle 不能共存".to_owned(),
                        ));
                    }
                    if transaction.status != "in_progress" {
                        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                            "已 aborted 的事务不能认领最终 Bundle".to_owned(),
                        ));
                    }
                    // E 是 atomic publish 后唯一合法形态；只前进 SQLite，不触碰 formal/final。
                    let stable_before = validate_stable_prepared_state(
                        paths,
                        lifecycle_lock,
                        transaction,
                        &plan,
                        &journal,
                    )?;
                    before_prepared_commit()?;
                    let stable_after_hook = validate_stable_prepared_state(
                        paths,
                        lifecycle_lock,
                        transaction,
                        &plan,
                        &journal,
                    )?;
                    if stable_after_hook != stable_before {
                        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                            "恢复提交 prepared 前文件系统状态发生变化".to_owned(),
                        ));
                    }
                    storage.update_takeover_v2_transaction_phase(
                        &transaction.id,
                        "prepared",
                        now,
                    )?;
                    let stable_after_commit = validate_stable_prepared_state(
                        paths,
                        lifecycle_lock,
                        transaction,
                        &plan,
                        &journal,
                    )?;
                    if stable_after_commit != stable_after_hook {
                        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                            "恢复提交 prepared 期间文件系统状态发生变化".to_owned(),
                        ));
                    }
                    return Ok(());
                }
                if transaction.phase == "prepared" {
                    return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                        "SQLite prepared 缺少完整最终 Bundle".to_owned(),
                    ));
                }
                let valid_before_abort = matches!(
                    audit.kind,
                    PreparedStagingKind::Candidate { complete: true }
                        | PreparedStagingKind::CandidateWithEmptyBundle
                        | PreparedStagingKind::CandidateWithContents
                        | PreparedStagingKind::StagedBundle
                );
                if transaction.status == "in_progress" && !valid_before_abort {
                    return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                        "in_progress formal Prepared 缺少合法 A/B/C/D staging".to_owned(),
                    ));
                }
                // A-D 尚未触及最终 Bundle；先持久化 aborted，再按 D→C→B→A 安全回退。
                validate_all_origin_manifests(paths, &plan, &journal.origins)?;
                validate_all_targets(paths, &plan)?;
                if transaction.status == "in_progress" {
                    storage.abort_takeover_v2_transaction(&transaction.id, None, now)?;
                }
                cleanup_prepared_staging_before_publish(paths, lifecycle_lock, &plan, &journal)?;
            } else {
                let audit = audit_takeover_v2_candidate(paths, lifecycle_lock, &plan, &journal)?;
                // 候选与外部输入必须通过同一轮交叉审计，避免用已变化的 Origin 授权删除。
                validate_all_origin_manifests(paths, &plan, &journal.origins)?;
                validate_all_targets(paths, &plan)?;
                // 先把已验证的安全结论写入 SQLite；后续清理中断时 aborted 仍可幂等续做。
                if transaction.status == "in_progress" {
                    storage.abort_takeover_v2_transaction(&transaction.id, None, now)?;
                }
                remove_audited_takeover_v2_candidate(paths, lifecycle_lock, &audit)?;
            }
            // phase temp 的 prefix 所有权依赖 formal；必须趁 formal 仍在时先删除并 fsync。
            if let Some(temporary) = phase_temporary.as_ref() {
                remove_owned_journal(paths, &journals, temporary)?;
                after_phase_temp_removed()?;
            }
            remove_owned_journal(paths, &journals, &owned)?;
            storage.forget_terminal_takeover_v2_transaction(&transaction.id)?;
        }
        None => {
            ensure_takeover_v2_phase_temp_absent(paths, &journals, &transaction.id)?;
            return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                "preparing 事务缺少正式 Journal".to_owned(),
            ));
        }
    }
    Ok(())
}

fn build_takeover_v2_journal(
    paths: &ApplicationPaths,
    transaction_id: &str,
    plan: &TakeoverV2Plan,
) -> Result<TakeoverV2Journal, TakeoverV2LifecycleError> {
    let selected = plan
        .origins
        .iter()
        .find(|origin| origin.id == plan.selected_origin_id)
        .ok_or_else(|| {
            TakeoverV2LifecycleError::RecoveryBlocked("Plan 缺少 selected Origin".to_owned())
        })?;
    let origins = plan
        .origins
        .iter()
        .map(|origin| capture_origin_manifest(paths, origin))
        .collect::<Result<Vec<_>, _>>()?;
    let effect_items = build_takeover_v2_effect_items(plan)?;
    let bundle_relative = format!("bundles/{}", plan.bundle_id);
    Ok(TakeoverV2Journal {
        version: TAKEOVER_V2_JOURNAL_VERSION,
        transaction_id: transaction_id.to_owned(),
        plan_id: plan.id.clone(),
        plan_seal: plan.seal.clone(),
        bundle_id: plan.bundle_id.clone(),
        member_id: plan.member_id.clone(),
        content_id: plan.content_id.clone(),
        selected_origin_id: plan.selected_origin_id.clone(),
        skill_name: plan.skill_name.clone(),
        content_fingerprint: selected.content_fingerprint.clone(),
        candidate_shape_version: TAKEOVER_V2_CANDIDATE_SHAPE_VERSION,
        phase: TakeoverV2JournalPhase::Preparing,
        staging_relative: format!("staging/{transaction_id}"),
        candidate_relative: format!(
            "staging/{transaction_id}/candidate/members/{}",
            plan.skill_name
        ),
        phase_temp_relative: format!("journals/{}", phase_temp_file_name(transaction_id)),
        bundle_relative: bundle_relative.clone(),
        content_relative: format!("{bundle_relative}/contents/{}", plan.content_id),
        current_target: format!("contents/{}", plan.content_id),
        origins,
        effect_items,
    })
}

fn build_takeover_v2_effect_items(
    plan: &TakeoverV2Plan,
) -> Result<Vec<TakeoverV2EffectItem>, TakeoverV2LifecycleError> {
    let origins_by_id = plan
        .origins
        .iter()
        .map(|origin| (origin.id.as_str(), origin))
        .collect::<BTreeMap<_, _>>();
    let mut absent_targets = Vec::new();
    let mut occupied_targets = Vec::new();
    let mut occupied_origins = BTreeSet::new();
    for target in &plan.targets {
        match &target.initial_state {
            TakeoverTargetInitialState::Absent => absent_targets.push(target),
            TakeoverTargetInitialState::OccupiedByOrigin { origin_id } => {
                let origin = origins_by_id.get(origin_id.as_str()).ok_or_else(|| {
                    TakeoverV2LifecycleError::RecoveryBlocked(
                        "Takeover v2 effect 缺少 Target 对应的 Origin".to_owned(),
                    )
                })?;
                if origin.final_disposition != TakeoverOriginDisposition::Mount
                    || origin.original_path != target.target_path
                    || !occupied_origins.insert(origin_id.as_str())
                {
                    return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                        "Takeover v2 effect 的 Target 与 Mount Origin 不一致".to_owned(),
                    ));
                }
                occupied_targets.push((target, *origin));
            }
        }
    }
    if plan.origins.iter().any(|origin| {
        origin.final_disposition == TakeoverOriginDisposition::Mount
            && !occupied_origins.contains(origin.id.as_str())
    }) {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "Takeover v2 effect 缺少 Mount Origin 对应的 Target".to_owned(),
        ));
    }
    absent_targets
        .sort_by(|left, right| (&left.target_path, &left.id).cmp(&(&right.target_path, &right.id)));
    occupied_targets.sort_by(|(left, _), (right, _)| {
        (&left.target_path, &left.id).cmp(&(&right.target_path, &right.id))
    });
    let mut removed_origins = plan
        .origins
        .iter()
        .filter(|origin| origin.final_disposition == TakeoverOriginDisposition::Remove)
        .collect::<Vec<_>>();
    removed_origins.sort_by(|left, right| {
        (&left.original_path, &left.id).cmp(&(&right.original_path, &right.id))
    });

    let mut effect_items = Vec::with_capacity(plan.targets.len() + removed_origins.len());
    effect_items.extend(
        absent_targets
            .into_iter()
            .map(|target| TakeoverV2EffectItem {
                operation: TakeoverV2EffectOperation::CreateAbsentMount {
                    target_id: target.id.clone(),
                },
                staged_observation: None,
                applied_observation: None,
                cleanup_completed: false,
            }),
    );
    effect_items.extend(occupied_targets.into_iter().map(|(target, origin)| {
        TakeoverV2EffectItem {
            operation: TakeoverV2EffectOperation::ReplaceOriginWithMount {
                target_id: target.id.clone(),
                origin_id: origin.id.clone(),
            },
            staged_observation: None,
            applied_observation: None,
            cleanup_completed: false,
        }
    }));
    effect_items.extend(
        removed_origins
            .into_iter()
            .map(|origin| TakeoverV2EffectItem {
                operation: TakeoverV2EffectOperation::RemoveOrigin {
                    origin_id: origin.id.clone(),
                },
                staged_observation: None,
                applied_observation: None,
                cleanup_completed: false,
            }),
    );

    Ok(effect_items)
}

#[cfg_attr(not(test), allow(dead_code))]
fn takeover_v2_effect_hidden_name(transaction_id: &str, index: usize) -> String {
    // 隐藏名只由已封印的事务与稳定排序位置决定，不重复写入 Journal 合同。
    format!(".skillyard-takeover-v2-{transaction_id}-{index:04}")
}

fn capture_origin_manifest(
    paths: &ApplicationPaths,
    origin: &TakeoverV2Origin,
) -> Result<TakeoverV2OriginManifest, TakeoverV2LifecycleError> {
    let parent = open_verified_origin_parent(paths, origin)?;
    let expected_root = (
        origin.original_device,
        origin.original_inode,
        origin.original_mode,
    );
    let entries = collect_original_manifest_at(
        &parent.handle,
        &parent.leaf,
        Path::new(&origin.original_path),
        expected_root,
    )?;
    parent.recheck(
        origin.parent_device,
        origin.parent_inode,
        origin.parent_mode,
    )?;
    Ok(TakeoverV2OriginManifest {
        origin_id: origin.id.clone(),
        root_device: origin.original_device,
        root_inode: origin.original_inode,
        root_mode: origin.original_mode,
        entries,
    })
}

fn validate_all_origins_without_journal(
    paths: &ApplicationPaths,
    plan: &TakeoverV2Plan,
) -> Result<(), TakeoverV2LifecycleError> {
    for origin in &plan.origins {
        let _ = capture_origin_manifest(paths, origin)?;
    }
    Ok(())
}

fn validate_all_origin_manifests(
    paths: &ApplicationPaths,
    plan: &TakeoverV2Plan,
    manifests: &[TakeoverV2OriginManifest],
) -> Result<(), TakeoverV2LifecycleError> {
    if plan.origins.len() != manifests.len() {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "Journal 的 Origin manifest 数量与 Plan 不一致".to_owned(),
        ));
    }
    for (origin, manifest) in plan.origins.iter().zip(manifests) {
        if manifest.origin_id != origin.id
            || manifest.root_device != origin.original_device
            || manifest.root_inode != origin.original_inode
            || manifest.root_mode != origin.original_mode
        {
            return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                "Journal 的 Origin 身份与 Plan 不一致".to_owned(),
            ));
        }
        let parent = open_verified_origin_parent(paths, origin)?;
        validate_original_manifest_at(
            &parent.handle,
            &parent.leaf,
            Path::new(&origin.original_path),
            (
                origin.original_device,
                origin.original_inode,
                origin.original_mode,
            ),
            &manifest.entries,
        )?;
        parent.recheck(
            origin.parent_device,
            origin.parent_inode,
            origin.parent_mode,
        )?;
    }
    Ok(())
}

struct OpenVerifiedParent {
    handle: File,
    visible_path: PathBuf,
    leaf: OsString,
}

impl OpenVerifiedParent {
    fn recheck(
        &self,
        expected_device: u64,
        expected_inode: u64,
        expected_mode: u32,
    ) -> Result<(), TakeoverV2LifecycleError> {
        let opened = self
            .handle
            .metadata()
            .map_err(|source| v2_io("检查已打开的父目录", &self.visible_path, source))?;
        let visible = fs::symlink_metadata(&self.visible_path)
            .map_err(|source| v2_io("重新检查父目录", &self.visible_path, source))?;
        if visible.file_type().is_symlink()
            || !visible.is_dir()
            || (opened.dev(), opened.ino(), opened.mode())
                != (expected_device, expected_inode, expected_mode)
            || (visible.dev(), visible.ino(), visible.mode())
                != (expected_device, expected_inode, expected_mode)
        {
            return Err(TakeoverV2LifecycleError::OriginChanged(
                self.visible_path.display().to_string(),
            ));
        }
        Ok(())
    }
}

fn open_verified_origin_parent(
    paths: &ApplicationPaths,
    origin: &TakeoverV2Origin,
) -> Result<OpenVerifiedParent, TakeoverV2LifecycleError> {
    let (base, parent, expected_base) = match origin.root_key {
        ScanRootKey::CodexGlobal
        | ScanRootKey::ClaudeCodeGlobal
        | ScanRootKey::GitHubCopilotGlobal => {
            let config = app_config_for_root(paths, origin.root_key)?;
            (paths.home().to_path_buf(), config.global_root, None)
        }
        ScanRootKey::SharedAgents => (
            paths.home().to_path_buf(),
            paths.shared_read_only_root(),
            None,
        ),
        ScanRootKey::CodexProject
        | ScanRootKey::ClaudeCodeProject
        | ScanRootKey::GitHubCopilotProject => {
            let config = app_config_for_root(paths, origin.root_key)?;
            let base = verified_project_base(
                origin.project_root_path.as_deref(),
                origin.project_root_device,
                origin.project_root_inode,
            )?;
            let parent = base.join(config.project_relative_root);
            let expected = Some((
                origin.project_root_device.expect("已由 helper 验证"),
                origin.project_root_inode.expect("已由 helper 验证"),
            ));
            (base, parent, expected)
        }
        ScanRootKey::SharedAgentsProject => {
            let base = verified_project_base(
                origin.project_root_path.as_deref(),
                origin.project_root_device,
                origin.project_root_inode,
            )?;
            let parent = base.join(".agents/skills");
            let expected = Some((
                origin.project_root_device.expect("已由 helper 验证"),
                origin.project_root_inode.expect("已由 helper 验证"),
            ));
            (base, parent, expected)
        }
    };
    if Path::new(&origin.original_path) != parent.join(&origin.observation_skill_name) {
        return Err(TakeoverV2LifecycleError::OriginChanged(
            origin.original_path.clone(),
        ));
    }
    let handle = open_verified_directory_chain(&base, &parent, expected_base)?;
    let opened = OpenVerifiedParent {
        handle,
        visible_path: parent,
        leaf: OsString::from(&origin.observation_skill_name),
    };
    opened.recheck(
        origin.parent_device,
        origin.parent_inode,
        origin.parent_mode,
    )?;
    Ok(opened)
}

fn validate_all_targets(
    paths: &ApplicationPaths,
    plan: &TakeoverV2Plan,
) -> Result<(), TakeoverV2LifecycleError> {
    for target in &plan.targets {
        validate_target(paths, plan, target)?;
    }
    Ok(())
}

fn validate_target(
    paths: &ApplicationPaths,
    plan: &TakeoverV2Plan,
    target: &TakeoverV2Target,
) -> Result<(), TakeoverV2LifecycleError> {
    let config = app_config_for_id(paths, target.app_id)?;
    let (base, parent, expected_base) = match target.scope {
        MountScope::Global => (paths.home().to_path_buf(), config.global_root, None),
        MountScope::Project => {
            let base = verified_project_base(
                target.project_root_path.as_deref(),
                target.project_root_device,
                target.project_root_inode,
            )?;
            let parent = base.join(config.project_relative_root);
            let expected = Some((
                target.project_root_device.expect("已由 helper 验证"),
                target.project_root_inode.expect("已由 helper 验证"),
            ));
            (base, parent, expected)
        }
    };
    if Path::new(&target.target_path) != parent.join(&plan.skill_name) {
        return Err(TakeoverV2LifecycleError::OriginChanged(
            target.target_path.clone(),
        ));
    }
    let handle = open_verified_directory_chain(&base, &parent, expected_base)?;
    let opened = OpenVerifiedParent {
        handle,
        visible_path: parent,
        leaf: OsString::from(&plan.skill_name),
    };
    opened.recheck(
        target.parent_device,
        target.parent_inode,
        target.parent_mode,
    )?;
    let before = entry_metadata_at(&opened.handle, &opened.leaf).map_err(|source| {
        v2_io(
            "检查 Target 初始状态",
            Path::new(&target.target_path),
            source,
        )
    })?;
    let valid = match (&target.initial_state, before.as_ref()) {
        (TakeoverTargetInitialState::Absent, None) => true,
        (TakeoverTargetInitialState::OccupiedByOrigin { origin_id }, Some(metadata)) => {
            plan.origins.iter().any(|origin| {
                origin.id == *origin_id
                    && origin.original_path == target.target_path
                    && metadata.st_mode & libc::S_IFMT == libc::S_IFDIR
                    && metadata.st_dev as u64 == origin.original_device
                    && metadata.st_ino == origin.original_inode
                    && u32::from(metadata.st_mode) == origin.original_mode
            })
        }
        _ => false,
    };
    if !valid {
        return Err(TakeoverV2LifecycleError::OriginChanged(
            target.target_path.clone(),
        ));
    }
    opened.recheck(
        target.parent_device,
        target.parent_inode,
        target.parent_mode,
    )?;
    let after = entry_metadata_at(&opened.handle, &opened.leaf).map_err(|source| {
        v2_io(
            "重新检查 Target 初始状态",
            Path::new(&target.target_path),
            source,
        )
    })?;
    if before.as_ref().map(entry_identity) != after.as_ref().map(entry_identity) {
        return Err(TakeoverV2LifecycleError::OriginChanged(
            target.target_path.clone(),
        ));
    }
    Ok(())
}

fn open_verified_directory_chain(
    base: &Path,
    target: &Path,
    expected_base: Option<(u64, u64)>,
) -> Result<File, TakeoverV2LifecycleError> {
    let relative = target
        .strip_prefix(base)
        .map_err(|_| TakeoverV2LifecycleError::OriginChanged(target.display().to_string()))?;
    let mut handle = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(base)
        .map_err(|source| v2_io("打开路径授权根", base, source))?;
    verify_open_directory_matches_visible(&handle, base)?;
    if expected_base.is_some_and(|expected| {
        handle
            .metadata()
            .map(|metadata| (metadata.dev(), metadata.ino()) != expected)
            .unwrap_or(true)
    }) {
        return Err(TakeoverV2LifecycleError::OriginChanged(
            base.display().to_string(),
        ));
    }
    let mut visible = base.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(TakeoverV2LifecycleError::OriginChanged(
                target.display().to_string(),
            ));
        };
        visible.push(name);
        handle = open_directory_at(&handle, name)
            .map_err(|source| v2_io("打开路径父目录", &visible, source))?;
        verify_open_directory_matches_visible(&handle, &visible)?;
    }
    Ok(handle)
}

fn verify_open_directory_matches_visible(
    handle: &File,
    visible_path: &Path,
) -> Result<(), TakeoverV2LifecycleError> {
    let opened = handle
        .metadata()
        .map_err(|source| v2_io("检查已打开目录", visible_path, source))?;
    let visible = fs::symlink_metadata(visible_path)
        .map_err(|source| v2_io("检查可见目录", visible_path, source))?;
    if visible.file_type().is_symlink()
        || !visible.is_dir()
        || (opened.dev(), opened.ino(), opened.mode())
            != (visible.dev(), visible.ino(), visible.mode())
    {
        return Err(TakeoverV2LifecycleError::OriginChanged(
            visible_path.display().to_string(),
        ));
    }
    Ok(())
}

fn verified_project_base(
    path: Option<&str>,
    device: Option<u64>,
    inode: Option<u64>,
) -> Result<PathBuf, TakeoverV2LifecycleError> {
    match (path, device, inode) {
        (Some(path), Some(_), Some(_)) => Ok(PathBuf::from(path)),
        _ => Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "Project 路径快照不完整".to_owned(),
        )),
    }
}

fn app_config_for_root(
    paths: &ApplicationPaths,
    root_key: ScanRootKey,
) -> Result<SupportedAppPathConfig, TakeoverV2LifecycleError> {
    paths
        .supported_apps()
        .into_iter()
        .find(|config| config.root_key == root_key || config.project_root_key == root_key)
        .ok_or_else(|| {
            TakeoverV2LifecycleError::RecoveryBlocked("未知 Supported App 路径".to_owned())
        })
}

fn app_config_for_id(
    paths: &ApplicationPaths,
    app_id: SupportedAppId,
) -> Result<SupportedAppPathConfig, TakeoverV2LifecycleError> {
    paths
        .supported_apps()
        .into_iter()
        .find(|config| config.id == app_id)
        .ok_or_else(|| {
            TakeoverV2LifecycleError::RecoveryBlocked("未知 Supported App 路径".to_owned())
        })
}

fn entry_identity(metadata: &libc::stat) -> (u64, u64, u32, u64) {
    (
        metadata.st_dev as u64,
        metadata.st_ino,
        u32::from(metadata.st_mode),
        metadata.st_nlink as u64,
    )
}

/// phase 与逐项进度会持续变化，合同哈希只覆盖恢复过程中不可变的事实。
fn takeover_v2_journal_contract_sha256(
    journal: &TakeoverV2Journal,
) -> Result<String, TakeoverV2LifecycleError> {
    let mut contract = journal.clone();
    contract.phase = TakeoverV2JournalPhase::Preparing;
    for item in &mut contract.effect_items {
        item.staged_observation = None;
        item.applied_observation = None;
        item.cleanup_completed = false;
    }
    let bytes = serde_json::to_vec(&contract)?;
    Ok(hex_sha256(&bytes))
}

fn validate_takeover_v2_journal_contract(
    journal: &TakeoverV2Journal,
    transaction: &StoredTakeoverV2Transaction,
    plan: &TakeoverV2Plan,
) -> Result<(), TakeoverV2LifecycleError> {
    validate_takeover_v2_journal_immutable_contract(journal, transaction, plan)?;
    validate_takeover_v2_effect_progress(journal)?;
    validate_takeover_v2_journal_phase_pairing(journal.phase, &transaction.phase)
}

fn validate_takeover_v2_effect_progress(
    journal: &TakeoverV2Journal,
) -> Result<(), TakeoverV2LifecycleError> {
    let mut saw_unapplied = false;
    for item in &journal.effect_items {
        let staged = item.staged_observation.as_deref();
        let applied = item.applied_observation.as_deref();
        let item_progress_valid = staged.is_none_or(is_takeover_v2_effect_observation)
            && applied.is_none_or(is_takeover_v2_effect_observation)
            && match &item.operation {
                TakeoverV2EffectOperation::CreateAbsentMount { .. }
                | TakeoverV2EffectOperation::ReplaceOriginWithMount { .. } => {
                    applied.is_none() || staged.is_some()
                }
                TakeoverV2EffectOperation::RemoveOrigin { .. } => staged.is_none(),
            }
            && (!item.cleanup_completed || applied.is_some());
        if !item_progress_valid {
            return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                "Takeover v2 Journal 包含不可能的 effect 进度".to_owned(),
            ));
        }
        if applied.is_none() {
            saw_unapplied = true;
        } else if saw_unapplied {
            // Remove 排在所有 Mount 之后；连续前缀保证共享入口不会先于新 Mount 消失。
            return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                "Takeover v2 Journal 的 effect 生效进度不连续".to_owned(),
            ));
        }
    }
    let phase_progress_valid = match journal.phase {
        TakeoverV2JournalPhase::Preparing => journal.effect_items.iter().all(|item| {
            item.staged_observation.is_none()
                && item.applied_observation.is_none()
                && !item.cleanup_completed
        }),
        TakeoverV2JournalPhase::Prepared => journal.effect_items.iter().all(|item| {
            item.applied_observation.is_none()
                && !item.cleanup_completed
                && match &item.operation {
                    TakeoverV2EffectOperation::CreateAbsentMount { .. }
                    | TakeoverV2EffectOperation::ReplaceOriginWithMount { .. } => true,
                    TakeoverV2EffectOperation::RemoveOrigin { .. } => {
                        item.staged_observation.is_none()
                    }
                }
        }),
        TakeoverV2JournalPhase::EffectStarted => journal
            .effect_items
            .iter()
            .all(|item| !item.cleanup_completed),
        TakeoverV2JournalPhase::StateCommitted => journal
            .effect_items
            .iter()
            .all(|item| item.applied_observation.is_some()),
        TakeoverV2JournalPhase::CleanupCompleted => journal
            .effect_items
            .iter()
            .all(|item| item.applied_observation.is_some() && item.cleanup_completed),
    };
    if phase_progress_valid {
        Ok(())
    } else {
        Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "Takeover v2 Journal phase 与 effect 进度不一致".to_owned(),
        ))
    }
}

fn is_takeover_v2_effect_observation(value: &str) -> bool {
    value.len() == TAKEOVER_V2_EFFECT_OBSERVATION_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_takeover_v2_journal_immutable_contract(
    journal: &TakeoverV2Journal,
    transaction: &StoredTakeoverV2Transaction,
    plan: &TakeoverV2Plan,
) -> Result<(), TakeoverV2LifecycleError> {
    let selected = plan
        .origins
        .iter()
        .find(|origin| origin.id == plan.selected_origin_id)
        .ok_or_else(|| {
            TakeoverV2LifecycleError::RecoveryBlocked("Plan 缺少 selected Origin".to_owned())
        })?;
    let expected_effect_items = build_takeover_v2_effect_items(plan)?;
    let mut normalized_effect_items = journal.effect_items.clone();
    for item in &mut normalized_effect_items {
        item.staged_observation = None;
        item.applied_observation = None;
        item.cleanup_completed = false;
    }
    let bundle_relative = format!("bundles/{}", plan.bundle_id);
    let immutable_matches = journal.version == TAKEOVER_V2_JOURNAL_VERSION
        && journal.transaction_id == transaction.id
        && journal.plan_id == plan.id
        && journal.plan_seal == plan.seal
        && journal.bundle_id == plan.bundle_id
        && journal.member_id == plan.member_id
        && journal.content_id == plan.content_id
        && journal.selected_origin_id == plan.selected_origin_id
        && journal.skill_name == plan.skill_name
        && journal.content_fingerprint == selected.content_fingerprint
        && journal.candidate_shape_version == TAKEOVER_V2_CANDIDATE_SHAPE_VERSION
        && journal.staging_relative == format!("staging/{}", transaction.id)
        && journal.candidate_relative
            == format!(
                "staging/{}/candidate/members/{}",
                transaction.id, plan.skill_name
            )
        && journal.phase_temp_relative
            == format!("journals/{}", phase_temp_file_name(&transaction.id))
        && journal.bundle_relative == bundle_relative
        && journal.content_relative
            == format!("bundles/{}/contents/{}", plan.bundle_id, plan.content_id)
        && journal.current_target == format!("contents/{}", plan.content_id)
        && normalized_effect_items == expected_effect_items;
    if !immutable_matches
        || takeover_v2_journal_contract_sha256(journal)? != transaction.journal_contract_sha256
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "SQLite、Plan 与 v2 接管 Journal 的事务合同不一致".to_owned(),
        ));
    }
    // 不重新采集来构造 expected；seal 已绑定原始 manifest，这里只核对其 Plan 身份关系。
    if journal.origins.len() != plan.origins.len()
        || journal
            .origins
            .iter()
            .zip(&plan.origins)
            .any(|(manifest, origin)| {
                manifest.origin_id != origin.id
                    || manifest.root_device != origin.original_device
                    || manifest.root_inode != origin.original_inode
                    || manifest.root_mode != origin.original_mode
            })
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 接管 Journal 的 Origin manifest 与 Plan 不一致".to_owned(),
        ));
    }
    Ok(())
}

fn validate_takeover_v2_journal_phase_pairing(
    journal_phase: TakeoverV2JournalPhase,
    transaction_phase: &str,
) -> Result<(), TakeoverV2LifecycleError> {
    let allowed = match transaction_phase {
        "journal_pending" => journal_phase == TakeoverV2JournalPhase::Preparing,
        "preparing" => matches!(
            journal_phase,
            TakeoverV2JournalPhase::Preparing | TakeoverV2JournalPhase::Prepared
        ),
        "prepared" => matches!(
            journal_phase,
            TakeoverV2JournalPhase::Prepared | TakeoverV2JournalPhase::EffectStarted
        ),
        "effect_started" => matches!(
            journal_phase,
            TakeoverV2JournalPhase::EffectStarted | TakeoverV2JournalPhase::StateCommitted
        ),
        "state_committed" => matches!(
            journal_phase,
            TakeoverV2JournalPhase::StateCommitted | TakeoverV2JournalPhase::CleanupCompleted
        ),
        "cleanup_completed" => journal_phase == TakeoverV2JournalPhase::CleanupCompleted,
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(TakeoverV2LifecycleError::RecoveryBlocked(format!(
            "SQLite phase {transaction_phase} 与 v2 Journal phase {journal_phase:?} 不构成合法配对"
        )))
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn update_takeover_v2_journal_to_prepared(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    transaction: &StoredTakeoverV2Transaction,
    plan: &TakeoverV2Plan,
) -> Result<(), TakeoverV2LifecycleError> {
    update_takeover_v2_journal_to_prepared_with_hooks(
        paths,
        lifecycle_lock,
        transaction,
        plan,
        |_, _| {},
        |_, _| {},
    )
}

fn update_takeover_v2_journal_to_prepared_with_hooks(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    transaction: &StoredTakeoverV2Transaction,
    plan: &TakeoverV2Plan,
    before_swap: impl FnOnce(&Path, &Path),
    after_swap: impl FnOnce(&Path, &Path),
) -> Result<(), TakeoverV2LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    if transaction.phase != "preparing" {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "只有 SQLite preparing 事务可以推进 v2 Journal phase".to_owned(),
        ));
    }
    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    let formal_name = OsString::from(journal_file_name(&transaction.id));
    let formal_path = paths.journals_root().join(&formal_name);
    let (preparing, preparing_owned, preparing_actual_bytes) =
        read_takeover_v2_journal_with_bytes_at(&journals, &formal_name, &formal_path)?;
    validate_takeover_v2_journal_immutable_contract(&preparing, transaction, plan)?;
    if preparing.phase != TakeoverV2JournalPhase::Preparing {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 Journal phase updater 只接受 formal Preparing".to_owned(),
        ));
    }
    let preparing_bytes = serde_json::to_vec_pretty(&preparing)?;
    if preparing_actual_bytes != preparing_bytes {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "formal Preparing Journal 不是 canonical pretty JSON".to_owned(),
        ));
    }
    if inspect_takeover_v2_initial_temporary_journal(paths, &journals, transaction, plan)?.is_some()
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "推进 v2 Journal phase 前仍存在首次 formal 的随机临时文件".to_owned(),
        ));
    }
    validate_complete_candidate_for_phase_update(paths, lifecycle_lock, plan, &preparing)?;

    let mut prepared = preparing.clone();
    prepared.phase = TakeoverV2JournalPhase::Prepared;
    validate_journal_size_for_all_phases(&prepared)?;
    let prepared_bytes = serde_json::to_vec_pretty(&prepared)?;
    let temporary_name = OsString::from(phase_temp_file_name(&transaction.id));
    let temporary_path = paths.journals_root().join(&temporary_name);
    let mut temporary = create_new_file_at(&journals, &temporary_name, &temporary_path)?;
    temporary
        .write_all(&prepared_bytes)
        .map_err(|source| v2_io("写入 Prepared 临时 Journal", &temporary_path, source))?;
    temporary
        .sync_all()
        .map_err(|source| v2_io("同步 Prepared 临时 Journal", &temporary_path, source))?;
    let temporary_snapshot = journal_snapshot_from_metadata(
        &temporary
            .metadata()
            .map_err(|source| v2_io("检查 Prepared 临时 Journal", &temporary_path, source))?,
    );
    journals
        .sync_all()
        .map_err(|source| v2_io("同步 Prepared 临时 Journal 父目录", &formal_path, source))?;
    before_swap(&formal_path, &temporary_path);
    let pre_swap_journals = rebind_unchanged_managed_directory(
        paths,
        lifecycle_lock,
        &journals,
        &paths.journals_root(),
    )?;

    let (preparing_rechecked, preparing_rechecked_owned, preparing_rechecked_bytes) =
        read_takeover_v2_journal_with_bytes_at(&pre_swap_journals, &formal_name, &formal_path)?;
    validate_takeover_v2_journal_immutable_contract(&preparing_rechecked, transaction, plan)?;
    if preparing_rechecked.phase != TakeoverV2JournalPhase::Preparing
        || preparing_rechecked_owned != preparing_owned
        || preparing_rechecked_bytes != preparing_bytes
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "formal Preparing Journal 在 swap 前发生变化".to_owned(),
        ));
    }
    let (prepared_rechecked, prepared_owned, prepared_rechecked_bytes) =
        read_takeover_v2_journal_with_bytes_at(
            &pre_swap_journals,
            &temporary_name,
            &temporary_path,
        )?;
    validate_takeover_v2_journal_immutable_contract(&prepared_rechecked, transaction, plan)?;
    if prepared_rechecked.phase != TakeoverV2JournalPhase::Prepared
        || prepared_owned.snapshot != temporary_snapshot
        || prepared_rechecked_bytes != prepared_bytes
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "Prepared 临时 Journal 在 swap 前发生变化".to_owned(),
        ));
    }
    validate_complete_candidate_for_phase_update(paths, lifecycle_lock, plan, &preparing)?;

    rename_at_swap(
        &pre_swap_journals,
        &temporary_name,
        &pre_swap_journals,
        &formal_name,
    )
    .map_err(|source| v2_io("原子交换 v2 Journal phase", &formal_path, source))?;
    pre_swap_journals
        .sync_all()
        .map_err(|source| v2_io("同步 v2 Journal phase 交换", &formal_path, source))?;

    // 交换会合理改变 ctime；先捕获交换后的完整基线，再开放测试 hook 模拟后续竞态。
    let (formal_after_swap, formal_after_swap_owned, formal_after_swap_bytes) =
        read_takeover_v2_journal_with_bytes_at(&pre_swap_journals, &formal_name, &formal_path)?;
    validate_takeover_v2_journal_immutable_contract(&formal_after_swap, transaction, plan)?;
    let (temporary_after_swap, temporary_after_swap_owned, temporary_after_swap_bytes) =
        read_takeover_v2_journal_with_bytes_at(
            &pre_swap_journals,
            &temporary_name,
            &temporary_path,
        )?;
    validate_takeover_v2_journal_immutable_contract(&temporary_after_swap, transaction, plan)?;
    if formal_after_swap.phase != TakeoverV2JournalPhase::Prepared
        || temporary_after_swap.phase != TakeoverV2JournalPhase::Preparing
        || formal_after_swap_bytes != prepared_bytes
        || temporary_after_swap_bytes != preparing_bytes
        || !same_journal_identity(formal_after_swap_owned.snapshot, prepared_owned.snapshot)
        || !same_journal_identity(
            temporary_after_swap_owned.snapshot,
            preparing_owned.snapshot,
        )
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 Journal phase swap 后的两侧身份或 phase 不一致".to_owned(),
        ));
    }
    after_swap(&formal_path, &temporary_path);
    let post_swap_journals = rebind_unchanged_managed_directory(
        paths,
        lifecycle_lock,
        &journals,
        &paths.journals_root(),
    )?;

    let (formal_after_hook, formal_after_hook_owned, formal_after_hook_bytes) =
        read_takeover_v2_journal_with_bytes_at(&post_swap_journals, &formal_name, &formal_path)?;
    validate_takeover_v2_journal_immutable_contract(&formal_after_hook, transaction, plan)?;
    let (temporary_after_hook, temporary_after_hook_owned, temporary_after_hook_bytes) =
        read_takeover_v2_journal_with_bytes_at(
            &post_swap_journals,
            &temporary_name,
            &temporary_path,
        )?;
    validate_takeover_v2_journal_immutable_contract(&temporary_after_hook, transaction, plan)?;
    if formal_after_hook.phase != TakeoverV2JournalPhase::Prepared
        || temporary_after_hook.phase != TakeoverV2JournalPhase::Preparing
        || formal_after_hook_bytes != formal_after_swap_bytes
        || temporary_after_hook_bytes != temporary_after_swap_bytes
        || formal_after_hook_owned != formal_after_swap_owned
        || temporary_after_hook_owned != temporary_after_swap_owned
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 Journal phase swap 后的两侧在复核期间发生变化".to_owned(),
        ));
    }
    validate_complete_candidate_for_phase_update(paths, lifecycle_lock, plan, &formal_after_hook)?;
    drop(temporary);
    remove_owned_journal(paths, &post_swap_journals, &temporary_after_hook_owned)?;
    let final_journals = rebind_unchanged_managed_directory(
        paths,
        lifecycle_lock,
        &journals,
        &paths.journals_root(),
    )?;
    let (formal_final, formal_final_owned, formal_final_bytes) =
        read_takeover_v2_journal_with_bytes_at(&final_journals, &formal_name, &formal_path)?;
    validate_takeover_v2_journal_immutable_contract(&formal_final, transaction, plan)?;
    if formal_final.phase != TakeoverV2JournalPhase::Prepared
        || formal_final_bytes != prepared_bytes
        || formal_final_owned != formal_after_hook_owned
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "清理旧 phase temp 后 formal Prepared 发生变化".to_owned(),
        ));
    }
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

/// 把完整 Candidate 包装成完整 Bundle，一次发布到最终目录后才推进 SQLite prepared。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn publish_takeover_v2_prepared_bundle(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    transaction_id: &str,
    now: i64,
) -> Result<(), TakeoverV2LifecycleError> {
    let mut no_hook = |_| Ok(());
    publish_takeover_v2_prepared_bundle_with_hook(paths, storage, transaction_id, now, &mut no_hook)
}

fn publish_takeover_v2_prepared_bundle_with_hook(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    transaction_id: &str,
    now: i64,
    after_checkpoint: &mut dyn FnMut(
        BundlePublishCheckpoint,
    ) -> Result<(), TakeoverV2LifecycleError>,
) -> Result<(), TakeoverV2LifecycleError> {
    let mut no_rename_boundary_hook = || Ok(());
    publish_takeover_v2_prepared_bundle_inner(
        paths,
        storage,
        transaction_id,
        now,
        after_checkpoint,
        &mut no_rename_boundary_hook,
    )
}

// 这个额外 seam 只用于把竞态精确放在最终校验与 renameat2 之间。
#[cfg(test)]
fn publish_takeover_v2_prepared_bundle_with_rename_boundary_hook(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    transaction_id: &str,
    now: i64,
    after_checkpoint: &mut dyn FnMut(
        BundlePublishCheckpoint,
    ) -> Result<(), TakeoverV2LifecycleError>,
    at_rename_boundary: &mut dyn FnMut() -> Result<(), TakeoverV2LifecycleError>,
) -> Result<(), TakeoverV2LifecycleError> {
    publish_takeover_v2_prepared_bundle_inner(
        paths,
        storage,
        transaction_id,
        now,
        after_checkpoint,
        at_rename_boundary,
    )
}

fn publish_takeover_v2_prepared_bundle_inner(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    transaction_id: &str,
    now: i64,
    after_checkpoint: &mut dyn FnMut(
        BundlePublishCheckpoint,
    ) -> Result<(), TakeoverV2LifecycleError>,
    at_rename_boundary: &mut dyn FnMut() -> Result<(), TakeoverV2LifecycleError>,
) -> Result<(), TakeoverV2LifecycleError> {
    let transaction = storage
        .recoverable_takeover_v2_transactions()?
        .into_iter()
        .find(|transaction| transaction.id == transaction_id)
        .ok_or_else(|| {
            TakeoverV2LifecycleError::RecoveryBlocked("待发布的 v2 接管事务不存在".to_owned())
        })?;
    if transaction.status != "in_progress" || transaction.phase != "preparing" {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "只有 in_progress/preparing 事务可以发布 v2 Bundle".to_owned(),
        ));
    }
    if let Some(error) = &transaction.recovery_validation_error {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(error.clone()));
    }
    let plan = storage.read_takeover_v2_plan_for_transaction(&transaction)?;
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;

    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    let formal_name = OsString::from(journal_file_name(transaction_id));
    let formal_path = paths.journals_root().join(&formal_name);
    let (journal, formal_owned, formal_bytes) =
        read_takeover_v2_journal_with_bytes_at(&journals, &formal_name, &formal_path)?;
    validate_takeover_v2_journal_contract(&journal, &transaction, &plan)?;
    if journal.phase != TakeoverV2JournalPhase::Prepared
        || formal_bytes != serde_json::to_vec_pretty(&journal)?
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "发布 v2 Bundle 只接受 canonical formal Prepared".to_owned(),
        ));
    }
    if inspect_takeover_v2_initial_temporary_journal(paths, &journals, &transaction, &plan)?
        .is_some()
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "发布 v2 Bundle 前仍存在首次 formal 的随机临时文件".to_owned(),
        ));
    }
    ensure_takeover_v2_phase_temp_absent(paths, &journals, transaction_id)?;
    validate_complete_candidate_for_phase_update(paths, &lifecycle_lock, &plan, &journal)?;

    let bundles_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?;
    let bundle_name = OsString::from(&plan.bundle_id);
    let final_bundle_path = paths.bundles_root().join(&bundle_name);
    if entry_metadata_at(&bundles_root, &bundle_name)
        .map_err(|source| v2_io("检查 v2 最终 Bundle", &final_bundle_path, source))?
        .is_some()
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 最终 Bundle 在发布前已经存在".to_owned(),
        ));
    }

    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let staging_name = OsString::from(transaction_id);
    let staging_path = paths.staging_root().join(&staging_name);
    let staging = open_directory_at(&staging_root, &staging_name)
        .map_err(|source| v2_io("打开 v2 接管临时目录", &staging_path, source))?;
    let staged_bundle_path = staging_path.join("bundle");
    let staged_bundle = create_synced_candidate_directory(
        &staging,
        OsStr::new("bundle"),
        &staged_bundle_path,
        "创建 staged Bundle",
    )?;
    staged_bundle
        .sync_all()
        .map_err(|source| v2_io("同步空 staged Bundle", &staged_bundle_path, source))?;
    after_checkpoint(BundlePublishCheckpoint::B)?;
    let staged_contents_path = staged_bundle_path.join("contents");
    let staged_contents = create_synced_candidate_directory(
        &staged_bundle,
        OsStr::new("contents"),
        &staged_contents_path,
        "创建 staged Bundle contents",
    )?;
    staged_contents
        .sync_all()
        .map_err(|source| v2_io("同步空 staged contents", &staged_contents_path, source))?;
    after_checkpoint(BundlePublishCheckpoint::C)?;
    rename_at_no_replace(
        &staging,
        OsStr::new("candidate"),
        &staged_contents,
        OsStr::new(&plan.content_id),
    )
    .map_err(|source| v2_io("组装 staged Bundle content", &staged_contents_path, source))?;
    staging
        .sync_all()
        .map_err(|source| v2_io("同步 v2 Candidate 移出", &staging_path, source))?;
    staged_contents
        .sync_all()
        .map_err(|source| v2_io("同步 staged Bundle content", &staged_contents_path, source))?;
    staged_bundle
        .sync_all()
        .map_err(|source| v2_io("同步 staged Bundle", &staged_bundle_path, source))?;
    staging_root
        .sync_all()
        .map_err(|source| v2_io("同步 v2 staging", &paths.staging_root(), source))?;
    after_checkpoint(BundlePublishCheckpoint::D)?;
    drop(staged_contents);
    drop(staged_bundle);

    let staged_before_publish =
        audit_takeover_v2_exact_staged_bundle(paths, &lifecycle_lock, &plan, &journal)?;
    validate_all_origin_manifests(paths, &plan, &journal.origins)?;
    validate_all_targets(paths, &plan)?;
    lifecycle_lock.recheck(paths)?;
    if entry_metadata_at(&bundles_root, &bundle_name)
        .map_err(|source| v2_io("重新检查 v2 最终 Bundle", &final_bundle_path, source))?
        .is_some()
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 最终 Bundle 在原子发布前被外部插入".to_owned(),
        ));
    }
    after_checkpoint(BundlePublishCheckpoint::BeforeAtomicPublish)?;

    validate_takeover_v2_prepublish_invariants(
        paths,
        &lifecycle_lock,
        &journals,
        &transaction,
        &plan,
        &journal,
        &formal_name,
        &formal_path,
        &formal_owned,
        &formal_bytes,
    )?;
    rebind_unchanged_managed_directory(paths, &lifecycle_lock, &staging, &staging_path)?;
    let staged_after_prepublish_hook =
        audit_takeover_v2_exact_staged_bundle(paths, &lifecycle_lock, &plan, &journal)?;
    if staged_after_prepublish_hook != staged_before_publish {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 staged Bundle 在原子发布前发生变化".to_owned(),
        ));
    }
    rebind_unchanged_managed_directory(paths, &lifecycle_lock, &staging, &staging_path)?;
    after_checkpoint(BundlePublishCheckpoint::AfterFreshStagedAudit)?;

    rebind_unchanged_managed_directory(paths, &lifecycle_lock, &staging, &staging_path)?;
    let staged_before_rename =
        audit_takeover_v2_exact_staged_bundle(paths, &lifecycle_lock, &plan, &journal)?;
    if staged_before_rename != staged_after_prepublish_hook {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 staged Bundle 在最终审计窗口发生变化".to_owned(),
        ));
    }
    rebind_unchanged_managed_directory(paths, &lifecycle_lock, &staging, &staging_path)?;
    validate_takeover_v2_prepublish_invariants(
        paths,
        &lifecycle_lock,
        &journals,
        &transaction,
        &plan,
        &journal,
        &formal_name,
        &formal_path,
        &formal_owned,
        &formal_bytes,
    )?;
    let publish_staging_root = rebind_unchanged_managed_directory(
        paths,
        &lifecycle_lock,
        &staging_root,
        &paths.staging_root(),
    )?;
    let publish_staging =
        rebind_unchanged_managed_directory(paths, &lifecycle_lock, &staging, &staging_path)?;
    let publish_bundles_root = rebind_unchanged_managed_directory(
        paths,
        &lifecycle_lock,
        &bundles_root,
        &paths.bundles_root(),
    )?;
    if entry_metadata_at(&publish_bundles_root, &bundle_name)
        .map_err(|source| v2_io("最终确认 v2 Bundle 发布目标", &final_bundle_path, source))?
        .is_some()
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 最终 Bundle 在完整复核期间被外部插入".to_owned(),
        ));
    }
    at_rename_boundary()?;

    rename_at_no_replace(
        &publish_staging,
        OsStr::new("bundle"),
        &publish_bundles_root,
        &bundle_name,
    )
    .map_err(|source| v2_io("原子发布完整 v2 Bundle", &final_bundle_path, source))?;
    publish_staging
        .sync_all()
        .map_err(|source| v2_io("同步已发布的 v2 staging", &staging_path, source))?;
    publish_staging_root
        .sync_all()
        .map_err(|source| v2_io("同步 v2 staging 根", &paths.staging_root(), source))?;
    publish_bundles_root
        .sync_all()
        .map_err(|source| v2_io("同步 v2 Bundle 根", &paths.bundles_root(), source))?;
    after_checkpoint(BundlePublishCheckpoint::E)?;

    let stable_before =
        validate_stable_prepared_state(paths, &lifecycle_lock, &transaction, &plan, &journal)?;
    if stable_before.formal != formal_owned || stable_before.formal_bytes != formal_bytes {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 Bundle 发布期间 formal Prepared 发生变化".to_owned(),
        ));
    }
    // rename 会改变根目录 ctime；其余身份字段必须证明 final 就是刚审计过的 staged source。
    let staged_root = audited_child_directory(
        staged_before_rename.staging.as_ref().ok_or_else(|| {
            TakeoverV2LifecycleError::RecoveryBlocked(
                "最终 D shape staging 缺少完整 audit".to_owned(),
            )
        })?,
        OsStr::new("bundle"),
    )?
    .snapshot;
    let final_root = stable_before.final_bundle.snapshot;
    if (
        final_root.device,
        final_root.inode,
        final_root.mode,
        final_root.links,
    ) != (
        staged_root.device,
        staged_root.inode,
        staged_root.mode,
        staged_root.links,
    ) {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 最终 Bundle 不是已审计的 staged source".to_owned(),
        ));
    }
    after_checkpoint(BundlePublishCheckpoint::BeforePreparedCommit)?;
    let stable_after_hook =
        validate_stable_prepared_state(paths, &lifecycle_lock, &transaction, &plan, &journal)?;
    if stable_after_hook != stable_before {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "SQLite prepared 前的稳定文件系统状态发生变化".to_owned(),
        ));
    }
    storage.update_takeover_v2_transaction_phase(transaction_id, "prepared", now)?;
    let stable_after_commit =
        validate_stable_prepared_state(paths, &lifecycle_lock, &transaction, &plan, &journal)?;
    if stable_after_commit != stable_after_hook {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "SQLite prepared 提交期间文件系统状态发生变化".to_owned(),
        ));
    }
    Ok(())
}

// hook 后必须从 lifecycle root 重新打开可见目录，并拒绝同名目录的 identity replacement。
fn rebind_unchanged_managed_directory(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    expected: &File,
    path: &Path,
) -> Result<File, TakeoverV2LifecycleError> {
    let fresh = open_managed_directory_from_root(paths, lifecycle_lock.root(), path)?;
    let expected_metadata = expected
        .metadata()
        .map_err(|source| v2_io("检查 hook 前受管目录", path, source))?;
    let fresh_metadata = fresh
        .metadata()
        .map_err(|source| v2_io("检查 fresh 受管目录", path, source))?;
    if (
        expected_metadata.dev(),
        expected_metadata.ino(),
        expected_metadata.mode(),
        expected_metadata.nlink(),
    ) != (
        fresh_metadata.dev(),
        fresh_metadata.ino(),
        fresh_metadata.mode(),
        fresh_metadata.nlink(),
    ) {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(format!(
            "v2 受管目录在发布窗口被替换：{}",
            path.display()
        )));
    }
    Ok(fresh)
}

// 两次复核必须使用同一组已封印对象，逐项传入可避免重新推导出不同合同。
#[allow(clippy::too_many_arguments)]
fn validate_takeover_v2_prepublish_invariants(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    expected_journals: &File,
    transaction: &StoredTakeoverV2Transaction,
    plan: &TakeoverV2Plan,
    expected_journal: &TakeoverV2Journal,
    formal_name: &OsStr,
    formal_path: &Path,
    expected_formal: &OwnedJournalEntry,
    expected_formal_bytes: &[u8],
) -> Result<(), TakeoverV2LifecycleError> {
    let journals = rebind_unchanged_managed_directory(
        paths,
        lifecycle_lock,
        expected_journals,
        &paths.journals_root(),
    )?;
    let (fresh_journal, fresh_formal, fresh_formal_bytes) =
        read_takeover_v2_journal_with_bytes_at(&journals, formal_name, formal_path)?;
    validate_takeover_v2_journal_contract(&fresh_journal, transaction, plan)?;
    if fresh_journal.phase != TakeoverV2JournalPhase::Prepared
        || fresh_formal_bytes != serde_json::to_vec_pretty(&fresh_journal)?
        || &fresh_journal != expected_journal
        || &fresh_formal != expected_formal
        || fresh_formal_bytes != expected_formal_bytes
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 formal Prepared 在原子发布前发生变化".to_owned(),
        ));
    }
    if inspect_takeover_v2_initial_temporary_journal(paths, &journals, transaction, plan)?.is_some()
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "原子发布前仍存在首次 formal 的随机临时文件".to_owned(),
        ));
    }
    ensure_takeover_v2_phase_temp_absent(paths, &journals, &transaction.id)?;
    validate_all_origin_manifests(paths, plan, &expected_journal.origins)?;
    validate_all_targets(paths, plan)?;
    lifecycle_lock.recheck(paths)?;
    rebind_unchanged_managed_directory(
        paths,
        lifecycle_lock,
        expected_journals,
        &paths.journals_root(),
    )?;
    Ok(())
}

fn validate_complete_candidate_for_phase_update(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    plan: &TakeoverV2Plan,
    journal: &TakeoverV2Journal,
) -> Result<(), TakeoverV2LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    validate_all_origin_manifests(paths, plan, &journal.origins)?;
    validate_all_targets(paths, plan)?;
    let audit = audit_takeover_v2_candidate(paths, lifecycle_lock, plan, journal)?;
    if !audit.complete {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "Candidate 尚未完整，不能推进 v2 Journal Prepared".to_owned(),
        ));
    }
    // Candidate 审计可能耗时，返回前交叉复核所有外部输入和生命周期根。
    validate_all_origin_manifests(paths, plan, &journal.origins)?;
    validate_all_targets(paths, plan)?;
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

fn validate_stable_prepared_state(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    transaction: &StoredTakeoverV2Transaction,
    plan: &TakeoverV2Plan,
    expected_journal: &TakeoverV2Journal,
) -> Result<StablePreparedAudit, TakeoverV2LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    let formal_name = OsString::from(journal_file_name(&transaction.id));
    let formal_path = paths.journals_root().join(&formal_name);
    let (formal, formal_owned, formal_bytes) =
        read_takeover_v2_journal_with_bytes_at(&journals, &formal_name, &formal_path)?;
    validate_takeover_v2_journal_contract(&formal, transaction, plan)?;
    if formal != *expected_journal
        || formal.phase != TakeoverV2JournalPhase::Prepared
        || formal_bytes != serde_json::to_vec_pretty(&formal)?
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "稳定 prepared 的 formal Journal 不符合 canonical 合同".to_owned(),
        ));
    }
    if inspect_takeover_v2_initial_temporary_journal(paths, &journals, transaction, plan)?.is_some()
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "稳定 prepared 不能残留首次 formal 临时文件".to_owned(),
        ));
    }
    ensure_takeover_v2_phase_temp_absent(paths, &journals, &transaction.id)?;

    validate_all_origin_manifests(paths, plan, &formal.origins)?;
    validate_all_targets(paths, plan)?;
    let staging = audit_takeover_v2_prepared_staging(paths, lifecycle_lock, plan, &formal)?;
    if !matches!(
        staging.kind,
        PreparedStagingKind::Missing | PreparedStagingKind::Empty
    ) {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "稳定 prepared 不能同时保留 staged 内容".to_owned(),
        ));
    }
    let bundles_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?;
    let bundle_name = OsString::from(&formal.bundle_id);
    let bundle_path = paths.bundles_root().join(&bundle_name);
    let (selected, manifest) = selected_origin_contract(plan, &formal)?;
    let selected_root = open_selected_origin_root(paths, selected, manifest)?;
    let final_bundle = audit_complete_bundle_container(
        &bundles_root,
        &bundle_name,
        &bundle_path,
        plan,
        manifest,
        &selected_root,
    )?;
    // 完整树和外部输入审计都可能耗时，返回前必须再做一次交叉复核。
    validate_all_origin_manifests(paths, plan, &formal.origins)?;
    validate_all_targets(paths, plan)?;
    lifecycle_lock.recheck(paths)?;
    Ok(StablePreparedAudit {
        formal: formal_owned,
        formal_bytes,
        staging,
        final_bundle,
    })
}

fn same_journal_identity(left: JournalEntrySnapshot, right: JournalEntrySnapshot) -> bool {
    (left.device, left.inode, left.mode, left.links)
        == (right.device, right.inode, right.mode, right.links)
}

fn write_takeover_v2_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    journal: &TakeoverV2Journal,
) -> Result<(), TakeoverV2LifecycleError> {
    write_takeover_v2_journal_with_hook(paths, lifecycle_lock, journal, |_| {})
}

fn write_takeover_v2_journal_with_hook(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    journal: &TakeoverV2Journal,
    after_phase_temp_absence_checked: impl FnOnce(&Path),
) -> Result<(), TakeoverV2LifecycleError> {
    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    validate_journal_size_for_all_phases(journal)?;
    let phase_temp_name = OsString::from(phase_temp_file_name(&journal.transaction_id));
    let phase_temp_path = paths.journals_root().join(&phase_temp_name);
    if entry_metadata_at(&journals, &phase_temp_name)
        .map_err(|source| v2_io("检查预封印 phase temp", &phase_temp_path, source))?
        .is_some()
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "首次写 formal Journal 前预封印 phase temp 必须为空".to_owned(),
        ));
    }
    after_phase_temp_absence_checked(&phase_temp_path);
    let fresh_journals = rebind_unchanged_managed_directory(
        paths,
        lifecycle_lock,
        &journals,
        &paths.journals_root(),
    )?;
    let bytes = serde_json::to_vec_pretty(journal)?;
    let name = OsString::from(journal_file_name(&journal.transaction_id));
    write_new_atomic_at(
        &fresh_journals,
        &name,
        &paths.journals_root().join(&name),
        &bytes,
    )?;
    let published_journals = rebind_unchanged_managed_directory(
        paths,
        lifecycle_lock,
        &journals,
        &paths.journals_root(),
    )?;
    // formal 发布是 phase temp 路径的所有权生效点；发布后复核可识别检查窗口内的外部插入。
    if entry_metadata_at(&published_journals, &phase_temp_name)
        .map_err(|source| v2_io("复核预封印 phase temp", &phase_temp_path, source))?
        .is_some()
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "首次 formal 发布期间 phase temp 被外部插入，已保留现场".to_owned(),
        ));
    }
    Ok(())
}

fn validate_journal_size_for_all_phases(
    journal: &TakeoverV2Journal,
) -> Result<(), TakeoverV2LifecycleError> {
    let mut maximum = 0;
    for phase in [
        TakeoverV2JournalPhase::Preparing,
        TakeoverV2JournalPhase::Prepared,
        TakeoverV2JournalPhase::EffectStarted,
        TakeoverV2JournalPhase::StateCommitted,
        TakeoverV2JournalPhase::CleanupCompleted,
    ] {
        let mut candidate = journal.clone();
        candidate.phase = phase;
        for item in &mut candidate.effect_items {
            item.staged_observation = match (phase, &item.operation) {
                (
                    TakeoverV2JournalPhase::Prepared
                    | TakeoverV2JournalPhase::EffectStarted
                    | TakeoverV2JournalPhase::StateCommitted
                    | TakeoverV2JournalPhase::CleanupCompleted,
                    TakeoverV2EffectOperation::CreateAbsentMount { .. }
                    | TakeoverV2EffectOperation::ReplaceOriginWithMount { .. },
                ) => Some("f".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES)),
                _ => None,
            };
            item.applied_observation = match phase {
                TakeoverV2JournalPhase::EffectStarted
                | TakeoverV2JournalPhase::StateCommitted
                | TakeoverV2JournalPhase::CleanupCompleted => {
                    Some("f".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES))
                }
                TakeoverV2JournalPhase::Preparing | TakeoverV2JournalPhase::Prepared => None,
            };
            // `false` 比 `true` 多一个 JSON 字节；StateCommitted 的合法最坏值必须预留 false。
            item.cleanup_completed = phase == TakeoverV2JournalPhase::CleanupCompleted;
        }
        maximum = maximum.max(serde_json::to_vec_pretty(&candidate)?.len());
    }
    if maximum > MAX_TAKEOVER_V2_JOURNAL_BYTES {
        Err(TakeoverV2LifecycleError::JournalTooLarge {
            actual: maximum,
            limit: MAX_TAKEOVER_V2_JOURNAL_BYTES,
        })
    } else {
        Ok(())
    }
}

// 生产路径需要同时保留 canonical bytes；这个简化包装只供测试断言使用。
#[cfg(test)]
fn read_takeover_v2_journal_at(
    journals: &File,
    name: &OsStr,
    path: &Path,
) -> Result<(TakeoverV2Journal, OwnedJournalEntry), TakeoverV2LifecycleError> {
    let (journal, owned, _) = read_takeover_v2_journal_with_bytes_at(journals, name, path)?;
    Ok((journal, owned))
}

fn read_takeover_v2_journal_with_bytes_at(
    journals: &File,
    name: &OsStr,
    path: &Path,
) -> Result<(TakeoverV2Journal, OwnedJournalEntry, Vec<u8>), TakeoverV2LifecycleError> {
    let (bytes, owned) = read_owned_journal_bytes_at(journals, name, path)?;
    let journal = serde_json::from_slice(&bytes)?;
    Ok((journal, owned, bytes))
}

fn read_owned_journal_bytes_at(
    journals: &File,
    name: &OsStr,
    path: &Path,
) -> Result<(Vec<u8>, OwnedJournalEntry), TakeoverV2LifecycleError> {
    let metadata = entry_metadata_at(journals, name)
        .map_err(|source| v2_io("检查 v2 接管 Journal", path, source))?
        .ok_or_else(|| {
            TakeoverV2LifecycleError::RecoveryBlocked("v2 接管 Journal 在检查期间消失".to_owned())
        })?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFREG || metadata.st_nlink != 1 {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 接管 Journal 不是独立普通文件".to_owned(),
        ));
    }
    if metadata.st_size < 0 || metadata.st_size as u64 > MAX_TAKEOVER_V2_JOURNAL_BYTES as u64 {
        return Err(TakeoverV2LifecycleError::JournalTooLarge {
            actual: usize::try_from(metadata.st_size).unwrap_or(usize::MAX),
            limit: MAX_TAKEOVER_V2_JOURNAL_BYTES,
        });
    }
    let mut file = open_regular_file_at(journals, name, path, false)?;
    let opened = file
        .metadata()
        .map_err(|source| v2_io("检查已打开的 v2 接管 Journal", path, source))?;
    let expected_snapshot = journal_snapshot_from_stat(&metadata);
    if journal_snapshot_from_metadata(&opened) != expected_snapshot {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 接管 Journal 在打开期间被外部替换".to_owned(),
        ));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_TAKEOVER_V2_JOURNAL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| v2_io("读取 v2 接管 Journal", path, source))?;
    if bytes.len() > MAX_TAKEOVER_V2_JOURNAL_BYTES {
        return Err(TakeoverV2LifecycleError::JournalTooLarge {
            actual: bytes.len(),
            limit: MAX_TAKEOVER_V2_JOURNAL_BYTES,
        });
    }
    let after_read = file
        .metadata()
        .map_err(|source| v2_io("重新检查已打开的 v2 接管 Journal", path, source))?;
    let visible = entry_metadata_at(journals, name)
        .map_err(|source| v2_io("重新检查可见的 v2 接管 Journal", path, source))?
        .ok_or_else(|| {
            TakeoverV2LifecycleError::RecoveryBlocked("v2 接管 Journal 在读取期间消失".to_owned())
        })?;
    if journal_snapshot_from_metadata(&after_read) != expected_snapshot
        || journal_snapshot_from_stat(&visible) != expected_snapshot
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 接管 Journal 在读取期间被外部修改".to_owned(),
        ));
    }
    Ok((
        bytes,
        OwnedJournalEntry {
            name: name.to_os_string(),
            snapshot: expected_snapshot,
        },
    ))
}

fn inspect_takeover_v2_initial_temporary_journal(
    paths: &ApplicationPaths,
    journals: &File,
    transaction: &StoredTakeoverV2Transaction,
    plan: &TakeoverV2Plan,
) -> Result<Option<OwnedJournalEntry>, TakeoverV2LifecycleError> {
    let prefix = format!(".{}.tmp-", journal_file_name(&transaction.id));
    let prefix = prefix.as_bytes();
    let mut matches = Vec::new();
    for name in read_entry_names_os_from_handle(journals)? {
        let bytes = name.as_bytes();
        let Some(suffix) = bytes.strip_prefix(prefix) else {
            continue;
        };
        let suffix = std::str::from_utf8(suffix).map_err(|_| {
            TakeoverV2LifecycleError::RecoveryBlocked(
                "v2 接管临时 Journal 名称不符合原子写入合同".to_owned(),
            )
        })?;
        let parsed = Uuid::parse_str(suffix).map_err(|_| {
            TakeoverV2LifecycleError::RecoveryBlocked(
                "v2 接管临时 Journal 名称不符合原子写入合同".to_owned(),
            )
        })?;
        if parsed.to_string() != suffix {
            return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                "v2 接管临时 Journal 名称不符合原子写入合同".to_owned(),
            ));
        }
        matches.push(name);
    }
    if matches.len() > 1 {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "同一 v2 接管事务存在多个临时 Journal".to_owned(),
        ));
    }
    let Some(name) = matches.pop() else {
        return Ok(None);
    };
    let path = paths.journals_root().join(&name);
    let (bytes, owned) = read_owned_journal_bytes_at(journals, &name, &path)?;
    let expected = build_takeover_v2_journal(paths, &transaction.id, plan)?;
    validate_takeover_v2_journal_immutable_contract(&expected, transaction, plan)?;
    validate_takeover_v2_journal_phase_pairing(expected.phase, &transaction.phase)?;
    let expected = serde_json::to_vec_pretty(&expected)?;
    // 随机 UUID 路径未被合同封印，只有完整 canonical Preparing 才能证明所有权。
    if bytes != expected {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "首次 formal 临时 Journal 不是完整 canonical Preparing".to_owned(),
        ));
    }
    Ok(Some(owned))
}

fn inspect_takeover_v2_phase_temporary_journal(
    paths: &ApplicationPaths,
    journals: &File,
    transaction: &StoredTakeoverV2Transaction,
    plan: &TakeoverV2Plan,
    formal: &TakeoverV2Journal,
) -> Result<Option<OwnedJournalEntry>, TakeoverV2LifecycleError> {
    let name = OsString::from(phase_temp_file_name(&transaction.id));
    let path = paths.journals_root().join(&name);
    if entry_metadata_at(journals, &name)
        .map_err(|source| v2_io("检查 v2 phase temp", &path, source))?
        .is_none()
    {
        return Ok(None);
    }
    validate_takeover_v2_journal_immutable_contract(formal, transaction, plan)?;
    let (expected_phase, prefix_allowed) = match (transaction.phase.as_str(), formal.phase) {
        ("preparing", TakeoverV2JournalPhase::Preparing) => {
            (TakeoverV2JournalPhase::Prepared, true)
        }
        ("preparing", TakeoverV2JournalPhase::Prepared) => {
            (TakeoverV2JournalPhase::Preparing, false)
        }
        _ => {
            return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                "当前 SQLite/formal 配对不允许存在 v2 phase temp".to_owned(),
            ));
        }
    };
    let mut expected = formal.clone();
    expected.phase = expected_phase;
    let expected = serde_json::to_vec_pretty(&expected)?;
    let (bytes, owned) = read_owned_journal_bytes_at(journals, &name, &path)?;
    let matches = if prefix_allowed {
        expected.starts_with(&bytes)
    } else {
        bytes == expected
    };
    if !matches {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "v2 phase temp 与 formal phase 的方向性合同不一致".to_owned(),
        ));
    }
    Ok(Some(owned))
}

fn ensure_takeover_v2_phase_temp_absent(
    paths: &ApplicationPaths,
    journals: &File,
    transaction_id: &str,
) -> Result<(), TakeoverV2LifecycleError> {
    let name = OsString::from(phase_temp_file_name(transaction_id));
    let path = paths.journals_root().join(&name);
    if entry_metadata_at(journals, &name)
        .map_err(|source| v2_io("检查 v2 phase temp", &path, source))?
        .is_some()
    {
        Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "formal Journal 缺失时不能认领残留 phase temp".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn remove_owned_journal(
    paths: &ApplicationPaths,
    journals: &File,
    entry: &OwnedJournalEntry,
) -> Result<(), TakeoverV2LifecycleError> {
    let path = paths.journals_root().join(&entry.name);
    let visible = entry_metadata_at(journals, &entry.name)
        .map_err(|source| v2_io("重新检查 v2 接管 Journal", &path, source))?
        .ok_or_else(|| {
            TakeoverV2LifecycleError::RecoveryBlocked(
                "待清理的 v2 接管 Journal 已被外部移除".to_owned(),
            )
        })?;
    if visible.st_mode & libc::S_IFMT != libc::S_IFREG
        || visible.st_nlink != 1
        || journal_snapshot_from_stat(&visible) != entry.snapshot
    {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(
            "待清理的 v2 接管 Journal 已被外部替换，已保留现场".to_owned(),
        ));
    }
    unlink_at(journals, &entry.name, false)
        .map_err(|source| v2_io("清理 v2 接管 Journal", &paths.journals_root(), source))?;
    journals
        .sync_all()
        .map_err(|source| v2_io("同步 Journal 目录", &paths.journals_root(), source))?;
    Ok(())
}

fn journal_snapshot_from_stat(metadata: &libc::stat) -> JournalEntrySnapshot {
    JournalEntrySnapshot {
        device: metadata.st_dev as u64,
        inode: metadata.st_ino,
        mode: u32::from(metadata.st_mode),
        links: metadata.st_nlink as u64,
        size: metadata.st_size.max(0) as u64,
        modified_seconds: metadata.st_mtime,
        modified_nanoseconds: metadata.st_mtime_nsec,
        changed_seconds: metadata.st_ctime,
        changed_nanoseconds: metadata.st_ctime_nsec,
    }
}

fn journal_snapshot_from_metadata(metadata: &fs::Metadata) -> JournalEntrySnapshot {
    JournalEntrySnapshot {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        links: metadata.nlink(),
        size: metadata.size(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    }
}

fn abort_and_forget_before_effect(
    storage: &mut Storage,
    transaction_id: &str,
    error_message: Option<&str>,
    now: i64,
) -> Result<(), TakeoverV2LifecycleError> {
    storage.abort_takeover_v2_transaction(transaction_id, error_message, now)?;
    storage.forget_terminal_takeover_v2_transaction(transaction_id)?;
    Ok(())
}

fn journal_file_name(transaction_id: &str) -> String {
    format!("takeover-v2-{transaction_id}.json")
}

fn phase_temp_file_name(transaction_id: &str) -> String {
    format!(".{}.phase-temp", journal_file_name(transaction_id))
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn v2_io(action: &'static str, path: &Path, source: std::io::Error) -> TakeoverV2LifecycleError {
    TakeoverV2LifecycleError::InspectPath {
        path: format!("{action}：{}", path.display()),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, io::Write, os::unix::fs::MetadataExt};

    use tempfile::{TempDir, tempdir};

    use super::*;
    use crate::{
        content::validate_single_skill_folder,
        domain::{
            InventoryLocationKind, InventoryObservation, ManagementKind, MountScope, ScanRootKey,
            SkillMetadataStatus, SupportedAppId, TakeoverIdentityBasis, TakeoverOriginDisposition,
            TakeoverTargetInitialState, TakeoverV2Target,
        },
        lifecycle::{ensure_central_store_layout, write_new_atomic_at_test_hook},
        scanner::fingerprint_skill_root,
        storage::{NewProject, takeover_v2_plan_seal},
    };

    struct Harness {
        _temp: TempDir,
        paths: ApplicationPaths,
        storage: Storage,
    }

    impl Harness {
        fn new() -> Self {
            let temp = tempdir().expect("应创建隔离目录");
            let home = temp.path().join("home");
            let data_root = temp.path().join("data");
            fs::create_dir_all(&home).expect("应创建测试 home");
            fs::create_dir_all(&data_root).expect("应创建 Central Store 根");
            let paths = ApplicationPaths::for_home(data_root.clone(), home);
            ensure_central_store_layout(&paths).expect("应初始化 Central Store");
            let storage = Storage::open(&data_root, &paths.database()).expect("应打开 SQLite");
            Self {
                _temp: temp,
                paths,
                storage,
            }
        }

        fn save_single_plan(&mut self, name: &str) -> TakeoverV2Plan {
            self.save_single_plan_with_setup(name, |_| {})
        }

        fn save_single_plan_with_setup(
            &mut self,
            name: &str,
            setup: impl FnOnce(&Path),
        ) -> TakeoverV2Plan {
            let skill_root = self.paths.home().join(".codex/skills").join(name);
            write_skill(&skill_root, name);
            // 测试可在快照生成前补充特定的合法目录结构。
            setup(&skill_root);
            let parent = fs::symlink_metadata(skill_root.parent().expect("应有父目录"))
                .expect("应读取父目录");
            let root = fs::symlink_metadata(&skill_root).expect("应读取 Skill 根");
            let validated = validate_single_skill_folder(&skill_root).expect("Skill 应有效");
            let origin_id = Uuid::new_v4().to_string();
            let observation_id = Uuid::new_v4().to_string();
            let bundle_id = Uuid::new_v4().to_string();
            let member_id = Uuid::new_v4().to_string();
            let content_id = Uuid::new_v4().to_string();
            let managed = self.paths.bundle_directory(&bundle_id);
            let expected_target = managed.join("current/members").join(name);
            let origin = TakeoverV2Origin {
                id: origin_id.clone(),
                observation_id: observation_id.clone(),
                observation_skill_name: name.to_owned(),
                observation_declared_name: Some(name.to_owned()),
                observation_skill_file: skill_root.join("SKILL.md").to_string_lossy().into_owned(),
                observation_location_kind: InventoryLocationKind::AppGlobal,
                observation_metadata_status: SkillMetadataStatus::Valid,
                observation_observed_by: vec![SupportedAppId::Codex],
                observation_fingerprint: fingerprint_skill_root(&skill_root)
                    .expect("应计算扫描指纹"),
                root_key: ScanRootKey::CodexGlobal,
                observation_stale: false,
                observation_management_kind: ManagementKind::TakeoverCandidate,
                observation_management_evidence: None,
                app_id: Some(SupportedAppId::Codex),
                scope: Some(MountScope::Global),
                project_id: None,
                project_display_name: None,
                project_root_path: None,
                project_root_device: None,
                project_root_inode: None,
                original_path: skill_root.to_string_lossy().into_owned(),
                parent_device: parent.dev(),
                parent_inode: parent.ino(),
                parent_mode: parent.mode(),
                original_device: root.dev(),
                original_inode: root.ino(),
                original_mode: root.mode(),
                content_fingerprint: validated.fingerprint,
                skill_description: validated.description,
                warnings: validated.warnings,
                final_disposition: TakeoverOriginDisposition::Mount,
            };
            let mut plan = TakeoverV2Plan {
                id: Uuid::new_v4().to_string(),
                identity_basis: TakeoverIdentityBasis::SingleOrigin,
                selected_origin_id: origin_id.clone(),
                bundle_id,
                member_id,
                content_id: content_id.clone(),
                bundle_display_name: name.to_owned(),
                skill_name: name.to_owned(),
                managed_directory: managed.to_string_lossy().into_owned(),
                content_directory: managed
                    .join("contents")
                    .join(content_id)
                    .to_string_lossy()
                    .into_owned(),
                expected_target: expected_target.to_string_lossy().into_owned(),
                origins: vec![origin.clone()],
                targets: vec![TakeoverV2Target {
                    id: Uuid::new_v4().to_string(),
                    mount_id: Uuid::new_v4().to_string(),
                    app_id: SupportedAppId::Codex,
                    scope: MountScope::Global,
                    project_id: None,
                    project_display_name: None,
                    project_root_path: None,
                    project_root_device: None,
                    project_root_inode: None,
                    target_path: origin.original_path.clone(),
                    expected_target: expected_target.to_string_lossy().into_owned(),
                    parent_device: parent.dev(),
                    parent_inode: parent.ino(),
                    parent_mode: parent.mode(),
                    initial_state: TakeoverTargetInitialState::OccupiedByOrigin { origin_id },
                }],
                created_at: 100,
                expires_at: 10_000,
                status: TakeoverV2PlanStatus::Pending,
                seal: String::new(),
            };
            plan.seal = takeover_v2_plan_seal(&plan);
            let observation = observation_from_origin(&origin);
            self.storage
                .save_initial_scan(150, &[observation], &[])
                .expect("应保存 Inventory");
            self.storage
                .save_takeover_v2_plan(&plan)
                .expect("应保存 v2 Plan")
        }

        fn save_two_origin_plan(&mut self, name: &str) -> TakeoverV2Plan {
            self.save_two_origin_plan_with_selected_content(name, None, false)
        }

        fn save_two_origin_plan_with_selected_content(
            &mut self,
            name: &str,
            second_helper: Option<&str>,
            select_second: bool,
        ) -> TakeoverV2Plan {
            let mut plan = self.save_single_plan(name);
            let skill_root = self.paths.home().join(".claude/skills").join(name);
            write_skill(&skill_root, name);
            if let Some(contents) = second_helper {
                fs::write(skill_root.join("helper.txt"), contents)
                    .expect("应写第二份 Origin 的区分内容");
            }
            let parent = fs::symlink_metadata(skill_root.parent().expect("应有父目录"))
                .expect("应读取 Claude 父目录");
            let root = fs::symlink_metadata(&skill_root).expect("应读取 Claude Skill 根");
            let validated = validate_single_skill_folder(&skill_root).expect("Skill 应有效");
            let second_origin = TakeoverV2Origin {
                id: Uuid::new_v4().to_string(),
                observation_id: Uuid::new_v4().to_string(),
                observation_skill_name: name.to_owned(),
                observation_declared_name: Some(name.to_owned()),
                observation_skill_file: skill_root.join("SKILL.md").to_string_lossy().into_owned(),
                observation_location_kind: InventoryLocationKind::AppGlobal,
                observation_metadata_status: SkillMetadataStatus::Valid,
                observation_observed_by: vec![SupportedAppId::ClaudeCode],
                observation_fingerprint: fingerprint_skill_root(&skill_root)
                    .expect("应计算 Claude 扫描指纹"),
                root_key: ScanRootKey::ClaudeCodeGlobal,
                observation_stale: false,
                observation_management_kind: ManagementKind::TakeoverCandidate,
                observation_management_evidence: None,
                app_id: Some(SupportedAppId::ClaudeCode),
                scope: Some(MountScope::Global),
                project_id: None,
                project_display_name: None,
                project_root_path: None,
                project_root_device: None,
                project_root_inode: None,
                original_path: skill_root.to_string_lossy().into_owned(),
                parent_device: parent.dev(),
                parent_inode: parent.ino(),
                parent_mode: parent.mode(),
                original_device: root.dev(),
                original_inode: root.ino(),
                original_mode: root.mode(),
                content_fingerprint: validated.fingerprint,
                skill_description: validated.description,
                warnings: validated.warnings,
                final_disposition: TakeoverOriginDisposition::Remove,
            };
            if second_helper.is_none() {
                assert_eq!(
                    plan.origins[0].content_fingerprint, second_origin.content_fingerprint,
                    "默认双 Origin fixture 应保持内容相同"
                );
            }
            if select_second {
                plan.selected_origin_id = second_origin.id.clone();
            }
            plan.identity_basis = TakeoverIdentityBasis::UserConfirmed;
            plan.origins.push(second_origin);
            plan.origins.sort_by(|left, right| {
                (&left.original_path, &left.id).cmp(&(&right.original_path, &right.id))
            });
            reset_plan_storage_identity(&self.paths, &mut plan);
            let observations = plan
                .origins
                .iter()
                .map(observation_from_origin)
                .collect::<Vec<_>>();
            self.storage
                .save_initial_scan(151, &observations, &[])
                .expect("应保存两个 Origin 的 Inventory");
            self.storage
                .save_takeover_v2_plan(&plan)
                .expect("应保存双 Origin Plan")
        }

        fn save_project_plan(&mut self, name: &str) -> TakeoverV2Plan {
            let mut plan = self.save_single_plan(name);
            let project_root = self._temp.path().join("project-one");
            let skill_root = project_root.join(".codex/skills").join(name);
            write_skill(&skill_root, name);
            let project_metadata = fs::symlink_metadata(&project_root).expect("应读取 Project 根");
            let project_id = Uuid::new_v4().to_string();
            self.storage
                .register_project(NewProject {
                    id: &project_id,
                    display_name: "测试 Project",
                    root_path: project_root.to_str().expect("测试路径应为 UTF-8"),
                    root_device: project_metadata.dev(),
                    root_inode: project_metadata.ino(),
                    created_at: 120,
                })
                .expect("应登记 Project");
            let parent = fs::symlink_metadata(skill_root.parent().expect("应有父目录"))
                .expect("应读取 Project Skill 父目录");
            let root = fs::symlink_metadata(&skill_root).expect("应读取 Project Skill 根");
            let validated = validate_single_skill_folder(&skill_root).expect("Skill 应有效");
            let origin = &mut plan.origins[0];
            origin.observation_id = Uuid::new_v4().to_string();
            origin.observation_skill_file =
                skill_root.join("SKILL.md").to_string_lossy().into_owned();
            origin.observation_location_kind = InventoryLocationKind::AppProject;
            origin.observation_fingerprint =
                fingerprint_skill_root(&skill_root).expect("应计算 Project 扫描指纹");
            origin.root_key = ScanRootKey::CodexProject;
            origin.scope = Some(MountScope::Project);
            origin.project_id = Some(project_id.clone());
            origin.project_display_name = Some("测试 Project".to_owned());
            origin.project_root_path = Some(project_root.to_string_lossy().into_owned());
            origin.project_root_device = Some(project_metadata.dev());
            origin.project_root_inode = Some(project_metadata.ino());
            origin.original_path = skill_root.to_string_lossy().into_owned();
            origin.parent_device = parent.dev();
            origin.parent_inode = parent.ino();
            origin.parent_mode = parent.mode();
            origin.original_device = root.dev();
            origin.original_inode = root.ino();
            origin.original_mode = root.mode();
            origin.content_fingerprint = validated.fingerprint;
            origin.skill_description = validated.description;
            origin.warnings = validated.warnings;
            let target = &mut plan.targets[0];
            target.scope = MountScope::Project;
            target.project_id = Some(project_id);
            target.project_display_name = Some("测试 Project".to_owned());
            target.project_root_path = Some(project_root.to_string_lossy().into_owned());
            target.project_root_device = Some(project_metadata.dev());
            target.project_root_inode = Some(project_metadata.ino());
            target.target_path = origin.original_path.clone();
            target.parent_device = parent.dev();
            target.parent_inode = parent.ino();
            target.parent_mode = parent.mode();
            reset_plan_storage_identity(&self.paths, &mut plan);
            let observation = observation_from_origin(&plan.origins[0]);
            self.storage
                .save_initial_scan(152, &[observation], &[])
                .expect("应保存 Project Inventory");
            self.storage
                .save_takeover_v2_plan(&plan)
                .expect("应保存 Project Plan")
        }

        fn begin_without_journal(&mut self, plan: &TakeoverV2Plan) -> (String, TakeoverV2Journal) {
            let transaction_id = Uuid::new_v4().to_string();
            let journal = build_takeover_v2_journal(&self.paths, &transaction_id, plan)
                .expect("应建立 Journal 合同");
            let seal = takeover_v2_journal_contract_sha256(&journal).expect("应计算合同 seal");
            self.storage
                .begin_takeover_v2_transaction(
                    &plan.id,
                    &transaction_id,
                    &format!("journals/{}", journal_file_name(&transaction_id)),
                    &seal,
                    200,
                )
                .expect("应开始 v2 事务");
            (transaction_id, journal)
        }

        fn transaction(&self, transaction_id: &str) -> Option<StoredTakeoverV2Transaction> {
            self.storage
                .recoverable_takeover_v2_transactions()
                .expect("应读取恢复事务")
                .into_iter()
                .find(|transaction| transaction.id == transaction_id)
        }

        fn begin_preparing_with_journal(
            &mut self,
            plan: &TakeoverV2Plan,
        ) -> (String, TakeoverV2Journal) {
            let (transaction_id, journal) = self.begin_without_journal(plan);
            let lock = acquire_lifecycle_lock(&self.paths).expect("应取得生命周期锁");
            write_takeover_v2_journal(&self.paths, &lock, &journal).expect("应写正式 Journal");
            drop(lock);
            self.storage
                .update_takeover_v2_transaction_phase(&transaction_id, "preparing", 201)
                .expect("应推进 preparing");
            (transaction_id, journal)
        }

        fn begin_with_prepared_candidate(
            &mut self,
            plan: &mut TakeoverV2Plan,
        ) -> (String, TakeoverV2Journal) {
            let (transaction_id, mut journal) = self.begin_preparing_with_journal(plan);
            let transaction = self.transaction(&transaction_id).expect("事务必须存在");
            let lock = acquire_lifecycle_lock(&self.paths).expect("应取得生命周期锁");
            plan.status = TakeoverV2PlanStatus::Consumed;
            prepare_takeover_v2_candidate(&self.paths, &lock, plan, &journal)
                .expect("应准备完整 Candidate");
            update_takeover_v2_journal_to_prepared(&self.paths, &lock, &transaction, plan)
                .expect("应推进 formal Prepared");
            journal.phase = TakeoverV2JournalPhase::Prepared;
            (transaction_id, journal)
        }
    }

    fn add_absent_copilot_target(harness: &Harness, plan: &mut TakeoverV2Plan) {
        let parent_path = harness.paths.home().join(".copilot/skills");
        fs::create_dir_all(&parent_path).expect("应创建 Copilot Skill 根");
        let parent = fs::symlink_metadata(&parent_path).expect("应读取 Copilot Skill 根");
        plan.targets.push(TakeoverV2Target {
            id: Uuid::new_v4().to_string(),
            mount_id: Uuid::new_v4().to_string(),
            app_id: SupportedAppId::GitHubCopilot,
            scope: MountScope::Global,
            project_id: None,
            project_display_name: None,
            project_root_path: None,
            project_root_device: None,
            project_root_inode: None,
            target_path: parent_path
                .join(&plan.skill_name)
                .to_string_lossy()
                .into_owned(),
            expected_target: plan.expected_target.clone(),
            parent_device: parent.dev(),
            parent_inode: parent.ino(),
            parent_mode: parent.mode(),
            initial_state: TakeoverTargetInitialState::Absent,
        });
        plan.seal = takeover_v2_plan_seal(plan);
    }

    #[test]
    fn prepare_writes_complete_journal_and_candidate_then_stays_preparing() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");

        let transaction_id =
            prepare_takeover_v2_journal(&harness.paths, &mut harness.storage, &plan.id, 200)
                .expect("应准备 v2 Journal");

        let transaction = harness.transaction(&transaction_id).expect("事务应保留");
        assert_eq!(transaction.phase, "preparing");
        let bytes = fs::read(
            harness
                .paths
                .journals_root()
                .join(journal_file_name(&transaction_id)),
        )
        .expect("Journal 应存在");
        let journal: TakeoverV2Journal = serde_json::from_slice(&bytes).expect("Journal 应可解析");
        assert_eq!(journal.origins.len(), 1);
        assert!(!journal.origins[0].entries.is_empty());
        assert_eq!(
            takeover_v2_journal_contract_sha256(&journal).expect("应计算 seal"),
            transaction.journal_contract_sha256
        );
        let candidate = harness
            .paths
            .staging_root()
            .join(&transaction_id)
            .join("candidate/members/alpha");
        assert_eq!(
            fs::read(candidate.join("helper.txt")).expect("候选应复制 selected Origin"),
            b"alpha helper"
        );
        assert!(candidate.join("SKILL.md").is_file());
    }

    #[test]
    fn multi_origin_preparation_copies_only_the_explicitly_selected_origin() {
        for select_second in [false, true] {
            let mut harness = Harness::new();
            let plan = harness.save_two_origin_plan_with_selected_content(
                "alpha",
                Some("claude selected helper"),
                select_second,
            );
            let selected = plan
                .origins
                .iter()
                .find(|origin| origin.id == plan.selected_origin_id)
                .expect("应找到 selected Origin");
            let expected = fs::read(Path::new(&selected.original_path).join("helper.txt"))
                .expect("应读取 selected Origin");

            let transaction_id =
                prepare_takeover_v2_journal(&harness.paths, &mut harness.storage, &plan.id, 200)
                    .expect("双 Origin 应准备候选");
            let candidate = harness
                .paths
                .staging_root()
                .join(&transaction_id)
                .join("candidate/members/alpha/helper.txt");
            assert_eq!(
                fs::read(candidate).expect("候选应存在"),
                expected,
                "select_second={select_second}"
            );
            let journal: TakeoverV2Journal = serde_json::from_slice(
                &fs::read(
                    harness
                        .paths
                        .journals_root()
                        .join(journal_file_name(&transaction_id)),
                )
                .expect("Journal 应存在"),
            )
            .expect("Journal 应可解析");
            assert_eq!(journal.origins.len(), 2);
            assert_eq!(journal.selected_origin_id, selected.id);
            assert_eq!(journal.content_fingerprint, selected.content_fingerprint);
        }
    }

    #[test]
    fn candidate_preparation_rejects_selected_origin_replaced_after_copy() {
        let mut harness = Harness::new();
        let mut plan = harness.save_single_plan("alpha");
        let (transaction_id, journal) = harness.begin_preparing_with_journal(&plan);
        plan.status = TakeoverV2PlanStatus::Consumed;
        let selected = PathBuf::from(&plan.origins[0].original_path);
        let backup = selected.with_file_name("alpha-after-copy-backup");
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");

        prepare_takeover_v2_candidate_with_hook(&harness.paths, &lock, &plan, &journal, || {
            fs::rename(&selected, &backup).expect("应在 copy 后替换 selected Origin");
            write_skill(&selected, "alpha");
        })
        .expect_err("copy 后的同内容 inode 替换必须被最终交叉检查拒绝");

        let candidate = harness
            .paths
            .staging_root()
            .join(&transaction_id)
            .join("candidate/members/alpha/helper.txt");
        assert!(candidate.exists(), "失败时完整候选必须留给恢复器");
        assert!(
            backup.join("SKILL.md").exists(),
            "原 selected Origin 必须保留"
        );
        assert!(selected.join("SKILL.md").exists(), "外部替换内容必须保留");
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务应保持 recoverable")
                .status,
            "in_progress"
        );
    }

    #[test]
    fn candidate_preparation_rejects_candidate_changed_after_initial_audit() {
        let mut harness = Harness::new();
        let mut plan = harness.save_single_plan("alpha");
        let (transaction_id, journal) = harness.begin_preparing_with_journal(&plan);
        plan.status = TakeoverV2PlanStatus::Consumed;
        let candidate = harness
            .paths
            .staging_root()
            .join(&transaction_id)
            .join("candidate/members/alpha/helper.txt");
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");

        let result =
            prepare_takeover_v2_candidate_with_hook(&harness.paths, &lock, &plan, &journal, || {
                // 模拟初次审计完成后，外部进程原地修改 Candidate。
                fs::write(&candidate, b"external candidate replacement")
                    .expect("应修改已审计 Candidate");
            });

        result.expect_err("返回成功前必须再次完整审计 Candidate");
        assert_eq!(
            fs::read(&candidate).expect("被修改的 Candidate 必须保留"),
            b"external candidate replacement"
        );
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务应保持 recoverable")
                .status,
            "in_progress"
        );
    }

    #[test]
    fn two_origin_candidate_blocks_on_any_origin_replacement_without_deleting_content() {
        for replaced_index in 0..2 {
            let mut harness = Harness::new();
            let plan = harness.save_two_origin_plan("alpha");
            let original_bytes = plan
                .origins
                .iter()
                .map(|origin| {
                    fs::read(Path::new(&origin.original_path).join("helper.txt"))
                        .expect("应读取 Origin 内容")
                })
                .collect::<Vec<_>>();
            let transaction_id =
                prepare_takeover_v2_journal(&harness.paths, &mut harness.storage, &plan.id, 200)
                    .expect("双 Origin 应准备 Journal");
            let journal_path = harness
                .paths
                .journals_root()
                .join(journal_file_name(&transaction_id));
            let journal: TakeoverV2Journal =
                serde_json::from_slice(&fs::read(&journal_path).expect("Journal 应存在"))
                    .expect("Journal 应可解析");
            assert_eq!(journal.origins.len(), 2);
            let candidate_file = harness
                .paths
                .staging_root()
                .join(&transaction_id)
                .join("candidate/members/alpha/helper.txt");
            let candidate_before = fs::read(&candidate_file).expect("候选应已准备");

            let replaced = Path::new(&plan.origins[replaced_index].original_path);
            let backup = replaced.with_file_name(format!("alpha-backup-{replaced_index}"));
            fs::rename(replaced, &backup).expect("应移走待替换 Origin");
            write_skill(replaced, "alpha");
            fs::write(replaced.join("helper.txt"), b"external replacement")
                .expect("应写外部替换内容");

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("替换只应阻塞当前事务");

            assert_eq!(
                harness
                    .transaction(&transaction_id)
                    .expect("事务应保留")
                    .status,
                "blocked"
            );
            assert!(journal_path.exists());
            assert_eq!(
                fs::read(&candidate_file).expect("Origin 变化时候选现场必须保留"),
                candidate_before
            );
            for (index, origin) in plan.origins.iter().enumerate() {
                let visible = fs::read(Path::new(&origin.original_path).join("helper.txt"))
                    .expect("两边可见内容都不得被删除");
                if index == replaced_index {
                    assert_eq!(visible, b"external replacement");
                    assert_eq!(
                        fs::read(backup.join("helper.txt")).expect("原目录备份必须保留"),
                        original_bytes[index]
                    );
                } else {
                    assert_eq!(visible, original_bytes[index]);
                }
            }
        }
    }

    #[test]
    fn project_path_derivation_rejects_a_replaced_project_root_identity() {
        let mut harness = Harness::new();
        let plan = harness.save_project_plan("alpha");
        validate_all_targets(&harness.paths, &plan).expect("真实 Project Target 应通过派生");
        let journal = build_takeover_v2_journal(&harness.paths, &Uuid::new_v4().to_string(), &plan)
            .expect("真实 Project Origin 应通过派生");
        assert_eq!(journal.origins.len(), 1);

        let project_root = PathBuf::from(
            plan.origins[0]
                .project_root_path
                .as_deref()
                .expect("应有 Project 根"),
        );
        let backup = project_root.with_file_name("project-one-backup");
        fs::rename(&project_root, &backup).expect("应替换 Project 根 inode");
        write_skill(&project_root.join(".codex/skills/alpha"), "alpha");

        prepare_takeover_v2_journal(&harness.paths, &mut harness.storage, &plan.id, 200)
            .expect_err("相同路径的新 Project 根必须被拒绝");
        assert!(
            harness
                .storage
                .recoverable_takeover_v2_transactions()
                .expect("应读取事务")
                .is_empty(),
            "begin 前的 Project 身份失败不能留下事务"
        );
        assert!(backup.join(".codex/skills/alpha/SKILL.md").exists());
        assert!(project_root.join(".codex/skills/alpha/SKILL.md").exists());
    }

    #[test]
    fn transaction_record_without_journal_aborts_without_touching_origin_and_is_idempotent() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let before = fs::read(Path::new(&plan.origins[0].original_path).join("SKILL.md"))
            .expect("应读取原内容");
        let (transaction_id, _) = harness.begin_without_journal(&plan);

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("应恢复 transaction record gap");
        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 301)
            .expect("重复恢复应幂等");

        assert!(harness.transaction(&transaction_id).is_none());
        assert_eq!(
            fs::read(Path::new(&plan.origins[0].original_path).join("SKILL.md"))
                .expect("原内容必须保留"),
            before
        );
    }

    #[test]
    fn transaction_without_a_formal_journal_never_claims_a_same_named_staging_directory() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, _) = harness.begin_without_journal(&plan);
        let staging = harness.paths.staging_root().join(&transaction_id);
        fs::create_dir(&staging).expect("应注入同名 staging");
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))
            .expect("应设置 staging 权限");

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("异常只应阻塞当前事务");

        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务必须保留")
                .status,
            "blocked"
        );
        assert!(staging.exists(), "无正式 Journal 时不能删除同名目录");
    }

    #[test]
    fn formal_journal_before_phase_update_is_cleaned_safely() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, journal) = harness.begin_without_journal(&plan);
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得锁");
        write_takeover_v2_journal(&harness.paths, &lock, &journal).expect("应写 Journal");
        drop(lock);

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("应恢复 Journal/phase gap");

        assert!(harness.transaction(&transaction_id).is_none());
        assert!(
            !harness
                .paths
                .journals_root()
                .join(journal_file_name(&transaction_id))
                .exists()
        );
    }

    #[test]
    fn journal_pending_with_a_formal_contract_cleans_a_semantic_partial_candidate() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, journal) = harness.begin_without_journal(&plan);
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得锁");
        write_takeover_v2_journal(&harness.paths, &lock, &journal).expect("应写正式 Journal");
        drop(lock);
        let levels = create_candidate_skeleton(&harness.paths, &transaction_id, "alpha", 4);
        let source = fs::read(Path::new(&plan.origins[0].original_path).join("helper.txt"))
            .expect("应读取源文件");
        let partial = levels[3].join("helper.txt");
        fs::write(&partial, &source[..1]).expect("应写合法部分候选");
        fs::set_permissions(&partial, fs::Permissions::from_mode(0o600)).expect("应设置构建中权限");

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("正式合同应授权清理 journal_pending 候选");

        assert!(harness.transaction(&transaction_id).is_none());
        assert!(!harness.paths.staging_root().join(transaction_id).exists());
    }

    #[test]
    fn preparing_recovery_removes_every_candidate_mkdir_window_idempotently() {
        for created_depth in 1..=4 {
            let mut harness = Harness::new();
            let plan = harness.save_single_plan("alpha");
            let origin_before =
                fs::read(Path::new(&plan.origins[0].original_path).join("SKILL.md"))
                    .expect("应读取 Origin");
            let (transaction_id, _) = harness.begin_preparing_with_journal(&plan);
            create_candidate_skeleton(&harness.paths, &transaction_id, "alpha", created_depth);

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("mkdir 中断窗口应自动恢复");
            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 301)
                .expect("重复恢复应幂等");

            assert!(harness.transaction(&transaction_id).is_none());
            assert!(!harness.paths.staging_root().join(&transaction_id).exists());
            assert!(
                !harness
                    .paths
                    .journals_root()
                    .join(journal_file_name(&transaction_id))
                    .exists()
            );
            assert_eq!(
                fs::read(Path::new(&plan.origins[0].original_path).join("SKILL.md"))
                    .expect("Origin 必须保留"),
                origin_before,
                "created_depth={created_depth}"
            );
        }
    }

    #[test]
    fn preparing_recovery_removes_zero_partial_and_complete_selected_prefixes() {
        for variant in ["zero", "partial", "complete_build_mode", "complete"] {
            let mut harness = Harness::new();
            let plan = harness.save_single_plan("alpha");
            let source = Path::new(&plan.origins[0].original_path).join("helper.txt");
            let source_bytes = fs::read(&source).expect("应读取 selected Origin 文件");
            let source_mode = fs::symlink_metadata(&source).expect("应读取源权限").mode() & 0o7777;
            let (transaction_id, _) = harness.begin_preparing_with_journal(&plan);
            let levels = create_candidate_skeleton(&harness.paths, &transaction_id, "alpha", 4);
            let destination = levels[3].join("helper.txt");
            let (bytes, mode) = match variant {
                "zero" => (&source_bytes[..0], 0o600),
                "partial" => (&source_bytes[..source_bytes.len() / 2], 0o600),
                "complete_build_mode" => (source_bytes.as_slice(), 0o600),
                "complete" => (source_bytes.as_slice(), source_mode),
                _ => unreachable!(),
            };
            fs::write(&destination, bytes).expect("应写候选文件窗口");
            fs::set_permissions(&destination, fs::Permissions::from_mode(mode))
                .expect("应设置候选文件协议权限");

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("合法文件前缀应自动恢复");
            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 301)
                .expect("重复恢复应幂等");

            assert!(
                harness.transaction(&transaction_id).is_none(),
                "variant={variant}"
            );
            assert!(
                !harness.paths.staging_root().join(&transaction_id).exists(),
                "variant={variant}"
            );
            assert_eq!(
                fs::read(&source).expect("selected Origin 必须保留"),
                source_bytes,
                "variant={variant}"
            );
        }
    }

    #[test]
    fn preparing_recovery_blocks_partial_file_with_final_permissions() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan_with_setup("alpha", |root| {
            fs::set_permissions(root.join("helper.txt"), fs::Permissions::from_mode(0o644))
                .expect("应固定源文件最终权限");
        });
        let source = Path::new(&plan.origins[0].original_path).join("helper.txt");
        let source_bytes = fs::read(&source).expect("应读取 selected Origin 文件");
        let final_mode = fs::symlink_metadata(&source).expect("应读取源权限").mode() & 0o7777;
        assert!(!source_bytes.is_empty(), "测试源文件必须非空");
        let (transaction_id, _) = harness.begin_preparing_with_journal(&plan);
        let levels = create_candidate_skeleton(&harness.paths, &transaction_id, "alpha", 4);
        let candidate_file = levels[3].join("helper.txt");
        fs::write(&candidate_file, &source_bytes[..source_bytes.len() - 1])
            .expect("应写未完成 Candidate 文件");
        fs::set_permissions(&candidate_file, fs::Permissions::from_mode(final_mode))
            .expect("应模拟提前使用最终权限");

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("不可信 Candidate 只应阻塞当前事务");

        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务必须保留")
                .status,
            "blocked"
        );
        assert!(candidate_file.exists(), "不可信 Candidate 必须原样保留");
    }

    #[test]
    fn preparing_recovery_blocks_final_mode_root_with_incomplete_subtree() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan_with_setup("alpha", |root| {
            fs::set_permissions(root, fs::Permissions::from_mode(0o755))
                .expect("应固定源根目录最终权限");
        });
        let source_root = Path::new(&plan.origins[0].original_path);
        let final_mode = fs::symlink_metadata(source_root)
            .expect("应读取源根目录权限")
            .mode()
            & 0o7777;
        let (transaction_id, _) = harness.begin_preparing_with_journal(&plan);
        let levels = create_candidate_skeleton(&harness.paths, &transaction_id, "alpha", 4);
        fs::set_permissions(&levels[3], fs::Permissions::from_mode(final_mode))
            .expect("应模拟根目录提前进入最终权限");

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("不可信 Candidate 只应阻塞当前事务");

        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务必须保留")
                .status,
            "blocked"
        );
        assert!(levels[3].exists(), "不完整的最终权限根目录必须保留");
    }

    #[test]
    fn preparing_recovery_blocks_final_mode_directory_with_incomplete_subtree() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan_with_setup("alpha", |root| {
            let scripts = root.join("scripts");
            fs::create_dir(&scripts).expect("应创建源子目录");
            fs::set_permissions(&scripts, fs::Permissions::from_mode(0o755))
                .expect("应固定源子目录最终权限");
            fs::write(scripts.join("run.sh"), b"#!/bin/sh\n").expect("应写源子文件");
        });
        let source_directory = Path::new(&plan.origins[0].original_path).join("scripts");
        let final_mode = fs::symlink_metadata(&source_directory)
            .expect("应读取源子目录权限")
            .mode()
            & 0o7777;
        let (transaction_id, _) = harness.begin_preparing_with_journal(&plan);
        let levels = create_candidate_skeleton(&harness.paths, &transaction_id, "alpha", 4);
        let candidate_directory = levels[3].join("scripts");
        fs::create_dir(&candidate_directory).expect("应创建未完成 Candidate 子目录");
        fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(final_mode))
            .expect("应模拟子目录提前进入最终权限");

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("不可信 Candidate 只应阻塞当前事务");

        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务必须保留")
                .status,
            "blocked"
        );
        assert!(
            candidate_directory.exists(),
            "不完整的最终权限子目录必须保留"
        );
    }

    #[test]
    fn preparing_recovery_blocks_final_directory_when_sibling_content_is_incomplete() {
        for variant in ["missing", "partial"] {
            let mut harness = Harness::new();
            let plan = save_sibling_plan(&mut harness);
            let source_root = Path::new(&plan.origins[0].original_path);
            let (transaction_id, _) = harness.begin_preparing_with_journal(&plan);
            let levels =
                create_sibling_candidate(&harness.paths, &transaction_id, source_root, variant);
            let candidate_a = levels[3].join("a");

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("不可信 Candidate 只应阻塞当前事务");

            assert_eq!(
                harness
                    .transaction(&transaction_id)
                    .expect("事务必须保留")
                    .status,
                "blocked",
                "variant={variant}"
            );
            assert!(candidate_a.exists(), "已 final 的目录必须保留");
        }
    }

    #[test]
    fn preparing_recovery_allows_partial_directory_chmod_but_keeps_root_last() {
        for root_is_final in [false, true] {
            let mut harness = Harness::new();
            let plan = save_sibling_plan(&mut harness);
            let source_root = Path::new(&plan.origins[0].original_path);
            let (transaction_id, _) = harness.begin_preparing_with_journal(&plan);
            let levels =
                create_sibling_candidate(&harness.paths, &transaction_id, source_root, "complete");
            if root_is_final {
                fs::set_permissions(&levels[3], fs::Permissions::from_mode(0o755))
                    .expect("应模拟 root 提前完成 chmod");
            }

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("目录 chmod 中断只应影响当前事务");

            if root_is_final {
                assert_eq!(
                    harness
                        .transaction(&transaction_id)
                        .expect("root 不是最后 chmod 时事务必须保留")
                        .status,
                    "blocked"
                );
                assert!(levels[3].exists());
            } else {
                assert!(
                    harness.transaction(&transaction_id).is_none(),
                    "文件已完整时 sibling 目录允许仍为 0700"
                );
                assert!(!harness.paths.staging_root().join(transaction_id).exists());
            }
        }
    }

    #[test]
    fn preparing_recovery_removes_nested_build_mode_partial_subtree() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan_with_setup("alpha", |root| {
            let scripts = root.join("scripts");
            fs::create_dir(&scripts).expect("应创建源子目录");
            fs::set_permissions(&scripts, fs::Permissions::from_mode(0o755))
                .expect("应固定源子目录最终权限");
            fs::write(scripts.join("run.sh"), b"#!/bin/sh\necho alpha\n").expect("应写源子文件");
        });
        let source_file = Path::new(&plan.origins[0].original_path).join("scripts/run.sh");
        let source_bytes = fs::read(&source_file).expect("应读取源子文件");
        let (transaction_id, _) = harness.begin_preparing_with_journal(&plan);
        let levels = create_candidate_skeleton(&harness.paths, &transaction_id, "alpha", 4);
        let candidate_directory = levels[3].join("scripts");
        fs::create_dir(&candidate_directory).expect("应创建构建中 Candidate 子目录");
        fs::set_permissions(&candidate_directory, fs::Permissions::from_mode(0o700))
            .expect("构建中子目录必须保持 0700");
        let candidate_file = candidate_directory.join("run.sh");
        fs::write(&candidate_file, &source_bytes[..source_bytes.len() / 2])
            .expect("应写合法嵌套前缀");
        fs::set_permissions(&candidate_file, fs::Permissions::from_mode(0o600))
            .expect("构建中文件必须保持 0600");

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("自然形成的嵌套 partial 应自动恢复");

        assert!(harness.transaction(&transaction_id).is_none());
        assert!(!harness.paths.staging_root().join(transaction_id).exists());
        assert_eq!(
            fs::read(source_file).expect("selected Origin 必须保留"),
            source_bytes
        );
    }

    #[test]
    fn complete_and_already_aborted_partial_candidates_recover_idempotently() {
        for variant in ["complete", "aborted_partial"] {
            let mut harness = Harness::new();
            let plan = harness.save_single_plan("alpha");
            let transaction_id = if variant == "complete" {
                prepare_takeover_v2_journal(&harness.paths, &mut harness.storage, &plan.id, 200)
                    .expect("应准备完整候选")
            } else {
                let (transaction_id, _) = harness.begin_preparing_with_journal(&plan);
                let levels = create_candidate_skeleton(&harness.paths, &transaction_id, "alpha", 4);
                let source = fs::read(Path::new(&plan.origins[0].original_path).join("helper.txt"))
                    .expect("应读取源文件");
                let partial = levels[3].join("helper.txt");
                fs::write(&partial, &source[..1]).expect("应写部分候选");
                fs::set_permissions(&partial, fs::Permissions::from_mode(0o600))
                    .expect("应设置构建中权限");
                harness
                    .storage
                    .abort_takeover_v2_transaction(&transaction_id, None, 250)
                    .expect("应模拟 abort 已提交、文件尚未清理");
                transaction_id
            };

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("生效前候选应恢复");
            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 301)
                .expect("重复恢复应幂等");

            assert!(
                harness.transaction(&transaction_id).is_none(),
                "variant={variant}"
            );
            assert!(
                !harness.paths.staging_root().join(&transaction_id).exists(),
                "variant={variant}"
            );
            assert!(Path::new(&plan.origins[0].original_path).exists());
        }
    }

    #[test]
    fn aborted_preparing_after_filesystem_cleanup_is_forgotten_without_a_journal() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, _) = harness.begin_preparing_with_journal(&plan);
        harness
            .storage
            .abort_takeover_v2_transaction(&transaction_id, None, 250)
            .expect("应记录 aborted");
        fs::remove_file(
            harness
                .paths
                .journals_root()
                .join(journal_file_name(&transaction_id)),
        )
        .expect("应模拟 Journal 已清理、尚未 forget 的崩溃窗口");

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("已清理的 aborted 事务应完成 forget");

        assert!(harness.transaction(&transaction_id).is_none());
        assert!(!harness.paths.staging_root().join(transaction_id).exists());
    }

    #[test]
    fn preparing_recovery_blocks_wrong_prefix_unknown_entries_links_modes_and_types() {
        for variant in [
            "wrong_prefix",
            "extra_top",
            "extra_descendant",
            "symlink",
            "hard_link",
            "special",
            "wrong_mode",
            "wrong_type",
        ] {
            let mut harness = Harness::new();
            let plan = harness.save_single_plan("alpha");
            let source = Path::new(&plan.origins[0].original_path).join("helper.txt");
            let source_before = fs::read(&source).expect("应读取 Origin 文件");
            let (transaction_id, _) = harness.begin_preparing_with_journal(&plan);
            let levels = create_candidate_skeleton(&harness.paths, &transaction_id, "alpha", 4);
            let skill = &levels[3];
            let candidate_file = skill.join("helper.txt");
            match variant {
                "wrong_prefix" => {
                    fs::write(&candidate_file, b"X").expect("应写错误前缀");
                    fs::set_permissions(&candidate_file, fs::Permissions::from_mode(0o600))
                        .expect("应设置构建中权限");
                }
                "extra_top" => {
                    fs::write(levels[1].join("external.txt"), b"external")
                        .expect("应注入 candidate sibling");
                }
                "extra_descendant" => {
                    fs::write(&candidate_file, &source_before[..1]).expect("应写合法前缀");
                    fs::set_permissions(&candidate_file, fs::Permissions::from_mode(0o600))
                        .expect("应设置构建中权限");
                    fs::write(skill.join("external.txt"), b"external")
                        .expect("应注入 Skill descendant");
                }
                "symlink" => {
                    std::os::unix::fs::symlink(&source, &candidate_file).expect("应注入软链接");
                }
                "hard_link" => {
                    let external = harness._temp.path().join("external-hard-link");
                    fs::write(&external, &source_before).expect("应创建外部文件");
                    fs::hard_link(&external, &candidate_file).expect("应注入硬链接");
                }
                "special" => {
                    let encoded = std::ffi::CString::new(candidate_file.as_os_str().as_bytes())
                        .expect("测试路径不能包含 NUL");
                    // SAFETY: CString 保证 NUL 终止，目标位于隔离测试目录。
                    let result = unsafe { libc::mkfifo(encoded.as_ptr(), 0o600) };
                    assert_eq!(result, 0, "应注入 FIFO 特殊文件");
                }
                "wrong_mode" => {
                    fs::write(&candidate_file, &source_before[..1]).expect("应写合法前缀");
                    fs::set_permissions(&candidate_file, fs::Permissions::from_mode(0o666))
                        .expect("应设置错误权限");
                }
                "wrong_type" => {
                    fs::create_dir(&candidate_file).expect("应以目录替换预期文件");
                    fs::set_permissions(&candidate_file, fs::Permissions::from_mode(0o700))
                        .expect("应设置目录权限");
                }
                _ => unreachable!(),
            }

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("不安全候选只应阻塞当前事务");

            let transaction = harness
                .transaction(&transaction_id)
                .expect("blocked 事务必须保留");
            assert_eq!(transaction.status, "blocked", "variant={variant}");
            assert!(
                harness.paths.staging_root().join(&transaction_id).exists(),
                "不安全现场必须保留，variant={variant}"
            );
            assert_eq!(
                fs::read(&source).expect("Origin 必须保留"),
                source_before,
                "variant={variant}"
            );
            if variant == "extra_descendant" {
                assert!(candidate_file.exists(), "审计失败前不得删除合法条目");
                assert!(skill.join("external.txt").exists());
            }
        }
    }

    #[test]
    fn candidate_cleanup_preserves_files_and_directories_replaced_after_read_only_audit() {
        for variant in ["file", "directory"] {
            let mut harness = Harness::new();
            let plan = harness.save_single_plan("alpha");
            let source_bytes =
                fs::read(Path::new(&plan.origins[0].original_path).join("helper.txt"))
                    .expect("应读取源文件");
            let (transaction_id, journal) = harness.begin_preparing_with_journal(&plan);
            let levels = create_candidate_skeleton(&harness.paths, &transaction_id, "alpha", 4);
            let candidate_file = levels[3].join("helper.txt");
            fs::write(&candidate_file, &source_bytes[..1]).expect("应写合法前缀");
            fs::set_permissions(&candidate_file, fs::Permissions::from_mode(0o600))
                .expect("应设置构建中权限");
            let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");
            let audit = audit_takeover_v2_candidate(&harness.paths, &lock, &plan, &journal)
                .expect("初始候选应通过只读审计");
            let mut replaced = false;
            let skill_path = levels[3].clone();
            let backup = levels[2].join("alpha-audited-backup");
            let result = remove_audited_takeover_v2_candidate_with_hook(
                &harness.paths,
                &lock,
                &audit,
                &mut |path| {
                    if replaced {
                        return;
                    }
                    if variant == "file" && path == candidate_file {
                        fs::remove_file(&candidate_file).expect("应替换候选文件");
                        fs::write(&candidate_file, b"external replacement")
                            .expect("应写外部替换文件");
                        fs::set_permissions(&candidate_file, fs::Permissions::from_mode(0o600))
                            .expect("应设置外部文件权限");
                        replaced = true;
                    } else if variant == "directory" && path == skill_path {
                        fs::rename(&skill_path, &backup).expect("应替换候选目录 inode");
                        fs::create_dir(&skill_path).expect("应创建外部替换目录");
                        fs::set_permissions(&skill_path, fs::Permissions::from_mode(0o700))
                            .expect("应设置替换目录权限");
                        fs::write(skill_path.join("external.txt"), b"external")
                            .expect("应写外部目录内容");
                        replaced = true;
                    }
                },
            );

            result.expect_err("审计后的替换必须阻止删除");
            assert!(replaced, "variant={variant}");
            if variant == "file" {
                assert_eq!(
                    fs::read(&candidate_file).expect("替换文件必须保留"),
                    b"external replacement"
                );
            } else {
                assert_eq!(
                    fs::read(skill_path.join("external.txt")).expect("替换目录必须保留"),
                    b"external"
                );
                assert!(backup.join("helper.txt").exists());
            }
        }
    }

    #[test]
    fn candidate_cleanup_blocks_directory_metadata_change_during_deletion() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let source_bytes = fs::read(Path::new(&plan.origins[0].original_path).join("helper.txt"))
            .expect("应读取源文件");
        let (transaction_id, journal) = harness.begin_preparing_with_journal(&plan);
        let levels = create_candidate_skeleton(&harness.paths, &transaction_id, "alpha", 4);
        let skill_path = levels[3].clone();
        let candidate_file = skill_path.join("helper.txt");
        fs::write(&candidate_file, &source_bytes[..1]).expect("应写合法前缀");
        fs::set_permissions(&candidate_file, fs::Permissions::from_mode(0o600))
            .expect("应设置构建中权限");
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");
        let audit = audit_takeover_v2_candidate(&harness.paths, &lock, &plan, &journal)
            .expect("初始 Candidate 应通过只读审计");
        let mut changed = false;

        let result = remove_audited_takeover_v2_candidate_with_hook(
            &harness.paths,
            &lock,
            &audit,
            &mut |path| {
                if !changed && path == candidate_file {
                    // 模拟删除期间仅修改父目录 metadata，不替换 inode 或增加条目。
                    fs::set_permissions(&skill_path, fs::Permissions::from_mode(0o711))
                        .expect("应修改已审计目录 metadata");
                    changed = true;
                }
            },
        );

        result.expect_err("目录 metadata 变化必须阻止删除");
        assert!(changed);
        assert!(skill_path.exists(), "发生变化的目录必须保留");
        assert!(candidate_file.exists(), "变化被发现前不得删除子文件");
        assert_eq!(
            fs::symlink_metadata(&skill_path)
                .expect("目录必须保留")
                .mode()
                & 0o7777,
            0o711
        );
    }

    #[test]
    fn aborted_transaction_is_not_forgotten_after_an_external_origin_replacement() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, _) = harness.begin_without_journal(&plan);
        harness
            .storage
            .abort_takeover_v2_transaction(&transaction_id, None, 201)
            .expect("应先记录 aborted");
        let original = Path::new(&plan.origins[0].original_path);
        fs::rename(original, original.with_file_name("alpha-external-backup"))
            .expect("应移走原目录");
        write_skill(original, "alpha");
        fs::write(original.join("helper.txt"), b"external replacement").expect("应写外部替换内容");

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("异常只应阻塞当前事务");

        let transaction = harness
            .transaction(&transaction_id)
            .expect("外部替换后不能忘记事务");
        assert_eq!(transaction.status, "blocked");
        assert_eq!(
            fs::read(original.join("helper.txt")).expect("外部内容必须保留"),
            b"external replacement"
        );
    }

    #[test]
    fn unique_valid_atomic_temp_is_removed() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, journal) = harness.begin_without_journal(&plan);
        let temporary = valid_temp_path(&harness.paths, &transaction_id);
        fs::write(
            &temporary,
            serde_json::to_vec_pretty(&journal).expect("应序列化完整 Journal"),
        )
        .expect("应创建完整临时 Journal");

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("应清理合法 temp");

        assert!(!temporary.exists());
        assert!(harness.transaction(&transaction_id).is_none());
    }

    #[test]
    fn journal_pending_only_recovers_complete_canonical_initial_temporary() {
        for variant in ["zero", "half", "full"] {
            let mut harness = Harness::new();
            let plan = harness.save_single_plan("alpha");
            let (transaction_id, journal) = harness.begin_without_journal(&plan);
            let expected = serde_json::to_vec_pretty(&journal).expect("应序列化 Preparing");
            let length = match variant {
                "zero" => 0,
                "half" => expected.len() / 2,
                "full" => expected.len(),
                _ => unreachable!(),
            };
            let temporary = valid_temp_path(&harness.paths, &transaction_id);
            fs::write(&temporary, &expected[..length]).expect("应写 canonical temp prefix");

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("initial temp 异常只应阻塞当前事务");

            if variant == "full" {
                assert!(harness.transaction(&transaction_id).is_none());
                assert!(!temporary.exists());
            } else {
                assert_eq!(
                    harness
                        .transaction(&transaction_id)
                        .expect("partial initial temp 必须保留事务")
                        .status,
                    "blocked",
                    "variant={variant}"
                );
                assert!(temporary.exists(), "variant={variant}");
            }
        }
    }

    #[test]
    fn preparing_recovery_cleans_pre_swap_prepared_phase_temp_prefixes() {
        for variant in ["zero", "half", "full"] {
            let mut harness = Harness::new();
            let plan = harness.save_single_plan("alpha");
            let (transaction_id, mut journal) = harness.begin_preparing_with_journal(&plan);
            journal.phase = TakeoverV2JournalPhase::Prepared;
            let expected = serde_json::to_vec_pretty(&journal).expect("应序列化 Prepared");
            let length = match variant {
                "zero" => 0,
                "half" => expected.len() / 2,
                "full" => expected.len(),
                _ => unreachable!(),
            };
            let phase_temp = harness
                .paths
                .journals_root()
                .join(phase_temp_file_name(&transaction_id));
            fs::write(&phase_temp, &expected[..length]).expect("应写 Prepared phase temp prefix");

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("swap 前合法 phase temp 应恢复");

            assert!(
                harness.transaction(&transaction_id).is_none(),
                "variant={variant}"
            );
            assert!(!phase_temp.exists(), "variant={variant}");
            assert!(
                !harness
                    .paths
                    .journals_root()
                    .join(journal_file_name(&transaction_id))
                    .exists(),
                "variant={variant}"
            );
        }
    }

    #[test]
    fn recovery_removes_phase_temp_before_formal_and_resumes_after_interruption() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, mut journal) = harness.begin_preparing_with_journal(&plan);
        journal.phase = TakeoverV2JournalPhase::Prepared;
        let expected = serde_json::to_vec_pretty(&journal).expect("应序列化 Prepared");
        let phase_temp = harness
            .paths
            .journals_root()
            .join(phase_temp_file_name(&transaction_id));
        fs::write(&phase_temp, &expected[..expected.len() / 2]).expect("应写合法 Prepared prefix");
        let formal = harness
            .paths
            .journals_root()
            .join(journal_file_name(&transaction_id));
        let transaction = harness.transaction(&transaction_id).expect("事务必须存在");
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");

        recover_pre_effect_takeover_v2_transaction_with_hook(
            &harness.paths,
            &lock,
            &mut harness.storage,
            &transaction,
            300,
            &mut || {
                Err(TakeoverV2LifecycleError::RecoveryBlocked(
                    "测试模拟 phase temp 删除后的硬中断".to_owned(),
                ))
            },
            &mut || Ok(()),
        )
        .expect_err("应模拟 formal 删除前中断");

        assert!(!phase_temp.exists(), "prefix 证明仍在时应先删除 phase temp");
        assert!(formal.exists(), "中断时 formal 必须仍在");
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("aborted 事务必须保留")
                .status,
            "aborted"
        );
        drop(lock);

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 301)
            .expect("第二次恢复应从 formal 继续");

        assert!(harness.transaction(&transaction_id).is_none());
        assert!(!formal.exists());
    }

    #[test]
    fn preparing_recovery_cleans_post_swap_and_stable_prepared_pairs() {
        for variant in ["post_swap", "stable_prepared"] {
            let mut harness = Harness::new();
            let mut plan = harness.save_single_plan("alpha");
            let (transaction_id, mut journal) = harness.begin_preparing_with_journal(&plan);
            let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");
            plan.status = TakeoverV2PlanStatus::Consumed;
            prepare_takeover_v2_candidate(&harness.paths, &lock, &plan, &journal)
                .expect("updater swap fixture 必须先有完整 Candidate");
            drop(lock);
            journal.phase = TakeoverV2JournalPhase::Prepared;
            let prepared = serde_json::to_vec_pretty(&journal).expect("应序列化 Prepared");
            let formal_name = OsString::from(journal_file_name(&transaction_id));
            let formal = harness.paths.journals_root().join(&formal_name);
            let phase_temp_name = OsString::from(phase_temp_file_name(&transaction_id));
            let phase_temp = harness.paths.journals_root().join(&phase_temp_name);
            if variant == "post_swap" {
                fs::write(&phase_temp, &prepared).expect("应写完整 Prepared phase temp");
                let journals =
                    File::open(harness.paths.journals_root()).expect("应打开 Journal 目录");
                rename_at_swap(&journals, &phase_temp_name, &journals, &formal_name)
                    .expect("应模拟 updater 已完成 swap");
                journals.sync_all().expect("应同步 swap");
            } else {
                fs::write(&formal, &prepared).expect("应模拟已清理 phase temp 的 Prepared");
            }

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("SQLite preparing 应清理 Prepared Journal");

            assert!(
                harness.transaction(&transaction_id).is_none(),
                "variant={variant}"
            );
            assert!(!formal.exists(), "variant={variant}");
            assert!(!phase_temp.exists(), "variant={variant}");
        }
    }

    #[test]
    fn directional_phase_temp_mismatches_block_and_preserve_all_entries() {
        for variant in [
            "wrong_direction",
            "post_swap_partial",
            "directory",
            "hard_link",
            "initial_plus_phase",
        ] {
            let mut harness = Harness::new();
            let plan = harness.save_single_plan("alpha");
            let (transaction_id, preparing) = harness.begin_preparing_with_journal(&plan);
            let formal = harness
                .paths
                .journals_root()
                .join(journal_file_name(&transaction_id));
            let phase_temp = harness
                .paths
                .journals_root()
                .join(phase_temp_file_name(&transaction_id));
            let preparing_bytes =
                serde_json::to_vec_pretty(&preparing).expect("应序列化 Preparing");
            let mut prepared = preparing.clone();
            prepared.phase = TakeoverV2JournalPhase::Prepared;
            let prepared_bytes = serde_json::to_vec_pretty(&prepared).expect("应序列化 Prepared");
            let mut initial_temp = None;
            match variant {
                "wrong_direction" => {
                    fs::write(&phase_temp, &preparing_bytes).expect("应写反向 phase temp");
                }
                "post_swap_partial" => {
                    fs::write(&formal, &prepared_bytes).expect("应写 Prepared formal");
                    fs::write(&phase_temp, &preparing_bytes[..preparing_bytes.len() / 2])
                        .expect("应写不完整 old Preparing");
                }
                "directory" => fs::create_dir(&phase_temp).expect("应创建目录 phase temp"),
                "hard_link" => {
                    fs::write(&phase_temp, &prepared_bytes).expect("应写 phase temp");
                    fs::hard_link(
                        &phase_temp,
                        harness._temp.path().join("phase-temp-external-link"),
                    )
                    .expect("应创建 phase temp hard link");
                }
                "initial_plus_phase" => {
                    fs::write(&phase_temp, &prepared_bytes).expect("应写合法 phase temp");
                    let path = valid_temp_path(&harness.paths, &transaction_id);
                    fs::write(&path, &preparing_bytes).expect("应写额外首次 temp");
                    initial_temp = Some(path);
                }
                _ => unreachable!(),
            }

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("不安全配对只应阻塞当前事务");

            assert_eq!(
                harness
                    .transaction(&transaction_id)
                    .expect("blocked 事务必须保留")
                    .status,
                "blocked",
                "variant={variant}"
            );
            assert!(formal.exists(), "variant={variant}");
            assert!(phase_temp.exists(), "variant={variant}");
            if let Some(initial_temp) = initial_temp {
                assert!(initial_temp.exists(), "额外 temp 必须保留");
            }
        }
    }

    #[test]
    fn journal_pending_rejects_prepared_formal_pairing() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, mut journal) = harness.begin_without_journal(&plan);
        journal.phase = TakeoverV2JournalPhase::Prepared;
        let formal = harness
            .paths
            .journals_root()
            .join(journal_file_name(&transaction_id));
        fs::write(
            &formal,
            serde_json::to_vec_pretty(&journal).expect("应序列化 Prepared"),
        )
        .expect("应写 Prepared formal");

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("非法 pairing 只应阻塞当前事务");

        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("blocked 事务必须保留")
                .status,
            "blocked"
        );
        assert!(formal.exists());
    }

    #[test]
    fn initial_formal_writer_preserves_a_preexisting_sealed_phase_temp() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, journal) = harness.begin_without_journal(&plan);
        let phase_temp = harness
            .paths
            .journals_root()
            .join(phase_temp_file_name(&transaction_id));
        fs::write(&phase_temp, b"external").expect("应预占 phase temp");
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");

        write_takeover_v2_journal(&harness.paths, &lock, &journal)
            .expect_err("首次 formal 不能覆盖预封印 phase temp");

        assert_eq!(
            fs::read(&phase_temp).expect("外部 phase temp 必须保留"),
            b"external"
        );
        assert!(
            !harness
                .paths
                .journals_root()
                .join(journal_file_name(&transaction_id))
                .exists()
        );
    }

    #[test]
    fn initial_formal_writer_preserves_phase_temp_inserted_during_publish_window() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, journal) = harness.begin_without_journal(&plan);
        let formal = harness
            .paths
            .journals_root()
            .join(journal_file_name(&transaction_id));
        let phase_temp = harness
            .paths
            .journals_root()
            .join(phase_temp_file_name(&transaction_id));
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");

        write_takeover_v2_journal_with_hook(&harness.paths, &lock, &journal, |path| {
            fs::write(path, b"external").expect("应在检查后插入 phase temp");
        })
        .expect_err("formal 发布后必须识别检查窗口内的 phase temp 插入");

        assert!(formal.exists(), "已发布 formal 必须保留现场");
        assert_eq!(
            fs::read(&phase_temp).expect("外部 phase temp 必须保留"),
            b"external"
        );
        drop(lock);

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("恢复只能阻塞该事务");

        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("竞态事务必须保留")
                .status,
            "blocked"
        );
        assert!(formal.exists());
        assert_eq!(
            fs::read(&phase_temp).expect("恢复不能认领外部 phase temp"),
            b"external"
        );
    }

    #[test]
    fn initial_formal_writer_rejects_a_replaced_journals_root() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, journal) = harness.begin_without_journal(&plan);
        let journals_root = harness.paths.journals_root();
        let journals_backup = harness._temp.path().join("initial-journals-backup");
        let formal_name = journal_file_name(&transaction_id);
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");

        write_takeover_v2_journal_with_hook(&harness.paths, &lock, &journal, |_| {
            fs::rename(&journals_root, &journals_backup).expect("应保留 hook 前 journals 根");
            fs::create_dir(&journals_root).expect("应重建可见 journals 根");
            fs::set_permissions(&journals_root, fs::Permissions::from_mode(0o700))
                .expect("可见 journals 根应使用受管权限");
        })
        .expect_err("journals 根被替换后不能继续发布 formal");

        assert!(
            !journals_root.join(&formal_name).exists(),
            "canonical formal 必须保持缺失"
        );
        assert!(
            !journals_backup.join(&formal_name).exists(),
            "formal 不能发布到 detached journals 根"
        );
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务必须保留")
                .phase,
            "journal_pending"
        );
    }

    #[test]
    fn aborted_missing_formal_never_claims_residual_temporary_journal() {
        for variant in ["initial", "phase"] {
            let mut harness = Harness::new();
            let plan = harness.save_single_plan("alpha");
            let (transaction_id, journal) = harness.begin_preparing_with_journal(&plan);
            harness
                .storage
                .abort_takeover_v2_transaction(&transaction_id, None, 250)
                .expect("应模拟 aborted");
            fs::remove_file(
                harness
                    .paths
                    .journals_root()
                    .join(journal_file_name(&transaction_id)),
            )
            .expect("应模拟 formal 已缺失");
            let temporary = if variant == "initial" {
                valid_temp_path(&harness.paths, &transaction_id)
            } else {
                harness
                    .paths
                    .journals_root()
                    .join(phase_temp_file_name(&transaction_id))
            };
            fs::write(
                &temporary,
                serde_json::to_vec_pretty(&journal).expect("应序列化 Preparing"),
            )
            .expect("应写残留 temp");

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("残留 temp 只应阻塞当前事务");

            assert_eq!(
                harness
                    .transaction(&transaction_id)
                    .expect("blocked 事务必须保留")
                    .status,
                "blocked",
                "variant={variant}"
            );
            assert!(temporary.exists(), "variant={variant}");
        }
    }

    #[test]
    fn unsafe_atomic_temp_variants_block_and_are_preserved() {
        for variant in ["partial", "multiple", "invalid", "directory", "hard_link"] {
            let mut harness = Harness::new();
            let plan = harness.save_single_plan("alpha");
            let (transaction_id, _) = harness.begin_without_journal(&plan);
            let first = valid_temp_path(&harness.paths, &transaction_id);
            match variant {
                "partial" => fs::write(&first, b"partial").expect("应创建不完整 temp"),
                "multiple" => {
                    fs::write(&first, b"one").expect("应创建第一个 temp");
                    fs::write(valid_temp_path(&harness.paths, &transaction_id), b"two")
                        .expect("应创建第二个 temp");
                }
                "invalid" => {
                    let name = format!(".{}.tmp-not-a-uuid", journal_file_name(&transaction_id));
                    fs::write(harness.paths.journals_root().join(name), b"invalid")
                        .expect("应创建非法 temp");
                }
                "directory" => fs::create_dir(&first).expect("应创建目录 temp"),
                "hard_link" => {
                    fs::write(&first, b"linked").expect("应创建 temp");
                    fs::hard_link(&first, harness.paths.journals_root().join("outside-link"))
                        .expect("应创建 hard link");
                }
                _ => unreachable!(),
            }

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("异常只应阻塞当前事务");

            let transaction = harness
                .transaction(&transaction_id)
                .expect("blocked 应保留");
            assert_eq!(transaction.status, "blocked", "variant={variant}");
            assert!(
                fs::read_dir(harness.paths.journals_root())
                    .expect("应读取 Journal 目录")
                    .next()
                    .is_some(),
                "variant={variant}"
            );
        }
    }

    #[test]
    fn preparing_without_or_with_tampered_journal_is_blocked() {
        for variant in ["missing", "tampered", "unknown_field", "oversized"] {
            let mut harness = Harness::new();
            let plan = harness.save_single_plan("alpha");
            let original_file = Path::new(&plan.origins[0].original_path).join("SKILL.md");
            let original_bytes = fs::read(&original_file).expect("应读取原内容");
            let (transaction_id, mut journal) = harness.begin_without_journal(&plan);
            harness
                .storage
                .update_takeover_v2_transaction_phase(&transaction_id, "preparing", 201)
                .expect("应推进 preparing");
            let journal_path = harness
                .paths
                .journals_root()
                .join(journal_file_name(&transaction_id));
            match variant {
                "missing" => {}
                "tampered" => {
                    journal.plan_seal = "f".repeat(64);
                    fs::write(
                        &journal_path,
                        serde_json::to_vec(&journal).expect("应序列化"),
                    )
                    .expect("应写篡改 Journal");
                }
                "unknown_field" => {
                    let mut value = serde_json::to_value(&journal).expect("应序列化");
                    value
                        .as_object_mut()
                        .expect("应是对象")
                        .insert("unexpected".to_owned(), serde_json::json!(true));
                    fs::write(&journal_path, serde_json::to_vec(&value).expect("应序列化"))
                        .expect("应写未知字段 Journal");
                }
                "oversized" => {
                    let mut file = File::create(&journal_path).expect("应创建超限 Journal");
                    file.write_all(&vec![b'x'; MAX_TAKEOVER_V2_JOURNAL_BYTES + 1])
                        .expect("应写超限 Journal");
                }
                _ => unreachable!(),
            }

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("异常只应阻塞当前事务");

            let transaction = harness
                .transaction(&transaction_id)
                .expect("blocked 应保留");
            assert_eq!(transaction.status, "blocked", "variant={variant}");
            if variant != "missing" {
                assert!(journal_path.exists(), "variant={variant}");
            }
            assert_eq!(
                fs::read(&original_file).expect("原内容必须保留"),
                original_bytes,
                "variant={variant}"
            );
        }
    }

    #[test]
    fn phases_after_preparing_are_blocked_without_touching_the_origin() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let original_file = Path::new(&plan.origins[0].original_path).join("SKILL.md");
        let original_bytes = fs::read(&original_file).expect("应读取原内容");
        let (transaction_id, mut journal) = harness.begin_without_journal(&plan);
        harness
            .storage
            .update_takeover_v2_transaction_phase(&transaction_id, "preparing", 201)
            .expect("应推进 preparing");
        harness
            .storage
            .update_takeover_v2_transaction_phase(&transaction_id, "prepared", 202)
            .expect("应推进 prepared");
        journal.phase = TakeoverV2JournalPhase::Prepared;
        let formal = harness
            .paths
            .journals_root()
            .join(journal_file_name(&transaction_id));
        fs::write(
            &formal,
            serde_json::to_vec_pretty(&journal).expect("应序列化 Prepared"),
        )
        .expect("应写 Prepared formal");

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("后续阶段只应阻塞当前事务");

        let transaction = harness
            .transaction(&transaction_id)
            .expect("prepared 事务应保留");
        assert_eq!(transaction.status, "blocked");
        assert_eq!(
            fs::read(&original_file).expect("原内容必须保留"),
            original_bytes
        );
        assert!(formal.exists(), "DB prepared 暂不进入本批恢复范围");
    }

    #[test]
    fn one_blocked_transaction_does_not_prevent_another_recovery() {
        let mut harness = Harness::new();
        let alpha = harness.save_single_plan("alpha");
        let (blocked_id, _) = harness.begin_without_journal(&alpha);
        harness
            .storage
            .update_takeover_v2_transaction_phase(&blocked_id, "preparing", 201)
            .expect("应推进 preparing");
        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
            .expect("应先隔离第一条");

        let beta = harness.save_single_plan("beta");
        let (healthy_id, _) = harness.begin_without_journal(&beta);
        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 400)
            .expect("第二条应独立恢复");

        assert_eq!(
            harness
                .transaction(&blocked_id)
                .expect("blocked 应保留")
                .status,
            "blocked"
        );
        assert!(harness.transaction(&healthy_id).is_none());
    }

    #[test]
    fn journal_effect_contract_orders_create_replace_then_remove() {
        let mut harness = Harness::new();
        let mut plan = harness.save_two_origin_plan("alpha");
        add_absent_copilot_target(&harness, &mut plan);
        let transaction_id = Uuid::new_v4().to_string();

        let journal = build_takeover_v2_journal(&harness.paths, &transaction_id, &plan)
            .expect("应建立 Journal");

        let occupied_origin_id = match &plan.targets[0].initial_state {
            TakeoverTargetInitialState::OccupiedByOrigin { origin_id } => origin_id,
            TakeoverTargetInitialState::Absent => panic!("fixture 应由 Origin 占用"),
        };
        let removed_origin_id = &plan
            .origins
            .iter()
            .find(|origin| origin.final_disposition == TakeoverOriginDisposition::Remove)
            .expect("fixture 应有待移除 Origin")
            .id;
        assert!(matches!(
            &journal.effect_items[0].operation,
            TakeoverV2EffectOperation::CreateAbsentMount { target_id }
                if target_id == &plan.targets[1].id
        ));
        assert!(matches!(
            &journal.effect_items[1].operation,
            TakeoverV2EffectOperation::ReplaceOriginWithMount {
                target_id,
                origin_id,
            } if target_id == &plan.targets[0].id && origin_id == occupied_origin_id
        ));
        assert!(matches!(
            &journal.effect_items[2].operation,
            TakeoverV2EffectOperation::RemoveOrigin { origin_id }
                if origin_id == removed_origin_id
        ));
        assert_eq!(
            journal
                .effect_items
                .iter()
                .enumerate()
                .map(|(index, _)| takeover_v2_effect_hidden_name(&transaction_id, index))
                .collect::<Vec<_>>(),
            vec![
                format!(".skillyard-takeover-v2-{transaction_id}-0000"),
                format!(".skillyard-takeover-v2-{transaction_id}-0001"),
                format!(".skillyard-takeover-v2-{transaction_id}-0002"),
            ]
        );
    }

    #[test]
    fn contract_hash_ignores_phase_and_effect_progress_but_covers_effect_identity() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let journal = build_takeover_v2_journal(&harness.paths, &Uuid::new_v4().to_string(), &plan)
            .expect("应建立 Journal");
        let mut advanced = journal.clone();
        advanced.phase = TakeoverV2JournalPhase::Prepared;
        advanced.effect_items[0].staged_observation =
            Some("a".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES));
        advanced.effect_items[0].applied_observation =
            Some("b".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES));
        advanced.effect_items[0].cleanup_completed = true;
        assert_eq!(
            takeover_v2_journal_contract_sha256(&journal).expect("应计算 seal"),
            takeover_v2_journal_contract_sha256(&advanced).expect("应计算 seal")
        );
        advanced.effect_items[0].operation = TakeoverV2EffectOperation::RemoveOrigin {
            origin_id: plan.origins[0].id.clone(),
        };
        assert_ne!(
            takeover_v2_journal_contract_sha256(&journal).expect("应计算 seal"),
            takeover_v2_journal_contract_sha256(&advanced).expect("应计算 seal")
        );
    }

    #[test]
    fn immutable_validator_rebuilds_effect_identity_from_plan() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, mut journal) = harness.begin_without_journal(&plan);
        let mut transaction = harness.transaction(&transaction_id).expect("事务必须存在");
        journal.effect_items[0].operation = TakeoverV2EffectOperation::RemoveOrigin {
            origin_id: plan.origins[0].id.clone(),
        };
        transaction.journal_contract_sha256 =
            takeover_v2_journal_contract_sha256(&journal).expect("应计算篡改后的合同 hash");

        let error = validate_takeover_v2_journal_immutable_contract(&journal, &transaction, &plan)
            .expect_err("即使协调篡改 SQLite hash，也必须按 Plan 拒绝 effect 身份");

        assert!(matches!(
            error,
            TakeoverV2LifecycleError::RecoveryBlocked(_)
        ));
    }

    #[test]
    fn phase_pairing_accepts_only_adjacent_persistent_crash_windows() {
        let allowed = [
            ("journal_pending", TakeoverV2JournalPhase::Preparing),
            ("preparing", TakeoverV2JournalPhase::Preparing),
            ("preparing", TakeoverV2JournalPhase::Prepared),
            ("prepared", TakeoverV2JournalPhase::Prepared),
            ("prepared", TakeoverV2JournalPhase::EffectStarted),
            ("effect_started", TakeoverV2JournalPhase::EffectStarted),
            ("effect_started", TakeoverV2JournalPhase::StateCommitted),
            ("state_committed", TakeoverV2JournalPhase::StateCommitted),
            ("state_committed", TakeoverV2JournalPhase::CleanupCompleted),
            (
                "cleanup_completed",
                TakeoverV2JournalPhase::CleanupCompleted,
            ),
        ];
        for (sqlite_phase, journal_phase) in allowed {
            validate_takeover_v2_journal_phase_pairing(journal_phase, sqlite_phase)
                .unwrap_or_else(|error| panic!("{sqlite_phase}/{journal_phase:?} 应合法：{error}"));
        }

        for (sqlite_phase, journal_phase) in [
            ("journal_pending", TakeoverV2JournalPhase::Prepared),
            ("prepared", TakeoverV2JournalPhase::StateCommitted),
            ("effect_started", TakeoverV2JournalPhase::Prepared),
            ("cleanup_completed", TakeoverV2JournalPhase::StateCommitted),
        ] {
            assert!(
                validate_takeover_v2_journal_phase_pairing(journal_phase, sqlite_phase).is_err(),
                "{sqlite_phase}/{journal_phase:?} 不应跨越恢复边界"
            );
        }
    }

    #[test]
    fn journal_contract_rejects_impossible_effect_progress() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, mut journal) = harness.begin_without_journal(&plan);
        let transaction = harness.transaction(&transaction_id).expect("事务必须存在");
        journal.effect_items[0].applied_observation =
            Some("b".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES));

        let error = validate_takeover_v2_journal_contract(&journal, &transaction, &plan)
            .expect_err("尚未暂存的 Mount 不可能已经生效");

        assert!(matches!(
            error,
            TakeoverV2LifecycleError::RecoveryBlocked(_)
        ));
    }

    #[test]
    fn effect_observation_accepts_only_fixed_lowercase_sha256() {
        assert!(is_takeover_v2_effect_observation(
            &"a".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES)
        ));
        assert!(!is_takeover_v2_effect_observation(
            &"x".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES)
        ));
        assert!(!is_takeover_v2_effect_observation(
            &"A".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES)
        ));
        assert!(!is_takeover_v2_effect_observation(
            &"a".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES - 1)
        ));
    }

    #[test]
    fn prepared_progress_allows_mount_staging_but_not_visible_effects() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let mut journal =
            build_takeover_v2_journal(&harness.paths, &Uuid::new_v4().to_string(), &plan)
                .expect("应建立 Journal");
        journal.phase = TakeoverV2JournalPhase::Prepared;
        journal.effect_items[0].staged_observation =
            Some("a".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES));

        validate_takeover_v2_effect_progress(&journal)
            .expect("Prepared 可以预暂存尚未对 Host 可见的 Mount 链接");

        journal.effect_items[0].applied_observation =
            Some("b".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES));
        assert!(
            validate_takeover_v2_effect_progress(&journal).is_err(),
            "Prepared 不能包含已经生效的 Host 路径"
        );
    }

    #[test]
    fn effect_started_progress_cannot_cleanup_originals_before_state_commit() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let mut journal =
            build_takeover_v2_journal(&harness.paths, &Uuid::new_v4().to_string(), &plan)
                .expect("应建立 Journal");
        journal.phase = TakeoverV2JournalPhase::EffectStarted;
        journal.effect_items[0].staged_observation =
            Some("a".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES));
        journal.effect_items[0].applied_observation =
            Some("b".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES));

        validate_takeover_v2_effect_progress(&journal).expect("生效阶段可以记录 Host 进度");

        journal.effect_items[0].cleanup_completed = true;
        assert!(
            validate_takeover_v2_effect_progress(&journal).is_err(),
            "领域状态提交前不能清理用于恢复的原目录"
        );
    }

    #[test]
    fn effect_started_progress_must_follow_effect_order() {
        let mut harness = Harness::new();
        let plan = harness.save_two_origin_plan("alpha");
        let mut journal =
            build_takeover_v2_journal(&harness.paths, &Uuid::new_v4().to_string(), &plan)
                .expect("应建立 Journal");
        journal.phase = TakeoverV2JournalPhase::EffectStarted;
        assert!(
            matches!(
                &journal.effect_items[1].operation,
                TakeoverV2EffectOperation::RemoveOrigin { .. }
            ),
            "fixture 的移除操作应位于 Mount 替换之后"
        );
        journal.effect_items[1].applied_observation =
            Some("c".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES));

        assert!(
            validate_takeover_v2_effect_progress(&journal).is_err(),
            "新 Mount 尚未生效时不能先移除后续 Origin"
        );
    }

    #[test]
    fn journal_size_preflight_reserves_all_effect_progress_fields() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let mut journal =
            build_takeover_v2_journal(&harness.paths, &Uuid::new_v4().to_string(), &plan)
                .expect("应建立 Journal");
        let current_maximum = [
            TakeoverV2JournalPhase::Preparing,
            TakeoverV2JournalPhase::Prepared,
            TakeoverV2JournalPhase::EffectStarted,
            TakeoverV2JournalPhase::StateCommitted,
            TakeoverV2JournalPhase::CleanupCompleted,
        ]
        .into_iter()
        .map(|phase| {
            let mut candidate = journal.clone();
            candidate.phase = phase;
            serde_json::to_vec_pretty(&candidate)
                .expect("应序列化 Journal")
                .len()
        })
        .max()
        .expect("应有 phase");
        journal
            .plan_seal
            .push_str(&"x".repeat(MAX_TAKEOVER_V2_JOURNAL_BYTES - current_maximum - 1));

        assert!(matches!(
            validate_journal_size_for_all_phases(&journal),
            Err(TakeoverV2LifecycleError::JournalTooLarge { .. })
        ));
    }

    #[test]
    fn journal_size_preflight_handles_state_committed_false_boundary() {
        let mut harness = Harness::new();
        let mut plan = harness.save_two_origin_plan("alpha");
        add_absent_copilot_target(&harness, &mut plan);
        let mut journal =
            build_takeover_v2_journal(&harness.paths, &Uuid::new_v4().to_string(), &plan)
                .expect("应建立三项 effect Journal");
        journal.phase = TakeoverV2JournalPhase::StateCommitted;
        for item in &mut journal.effect_items {
            if !matches!(
                &item.operation,
                TakeoverV2EffectOperation::RemoveOrigin { .. }
            ) {
                item.staged_observation = Some("a".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES));
            }
            item.applied_observation = Some("b".repeat(TAKEOVER_V2_EFFECT_OBSERVATION_BYTES));
            item.cleanup_completed = false;
        }
        let state_size = serde_json::to_vec_pretty(&journal)
            .expect("应序列化 StateCommitted Journal")
            .len();
        let mut cleanup = journal.clone();
        cleanup.phase = TakeoverV2JournalPhase::CleanupCompleted;
        for item in &mut cleanup.effect_items {
            item.cleanup_completed = true;
        }
        let cleanup_size = serde_json::to_vec_pretty(&cleanup)
            .expect("应序列化 CleanupCompleted Journal")
            .len();
        assert!(
            state_size > cleanup_size,
            "三个 false 必须覆盖更长 cleanup phase，才能复现精确边界"
        );
        journal
            .plan_seal
            .push_str(&"a".repeat(MAX_TAKEOVER_V2_JOURNAL_BYTES - state_size));
        assert_eq!(
            serde_json::to_vec_pretty(&journal)
                .expect("应序列化边界 Journal")
                .len(),
            MAX_TAKEOVER_V2_JOURNAL_BYTES
        );

        validate_journal_size_for_all_phases(&journal).expect("精确边界必须可写");
        journal.plan_seal.push('a');
        assert!(matches!(
            validate_journal_size_for_all_phases(&journal),
            Err(TakeoverV2LifecycleError::JournalTooLarge { .. })
        ));
    }

    #[test]
    fn journal_phase_updater_atomically_replaces_preparing_with_prepared() {
        let mut harness = Harness::new();
        let mut plan = harness.save_single_plan("alpha");
        let (transaction_id, journal) = harness.begin_preparing_with_journal(&plan);
        let transaction = harness.transaction(&transaction_id).expect("事务必须存在");
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");
        plan.status = TakeoverV2PlanStatus::Consumed;
        prepare_takeover_v2_candidate(&harness.paths, &lock, &plan, &journal)
            .expect("应先准备完整 Candidate");

        update_takeover_v2_journal_to_prepared(&harness.paths, &lock, &transaction, &plan)
            .expect("应原子推进 Journal phase");

        let journals = File::open(harness.paths.journals_root()).expect("应打开 Journal 目录");
        let name = OsString::from(journal_file_name(&transaction_id));
        let path = harness.paths.journals_root().join(&name);
        let (journal, _) =
            read_takeover_v2_journal_at(&journals, &name, &path).expect("正式 Journal 必须可读");
        assert_eq!(journal.phase, TakeoverV2JournalPhase::Prepared);
        assert!(
            !harness
                .paths
                .journals_root()
                .join(phase_temp_file_name(&transaction_id))
                .exists(),
            "成功后旧 Preparing temp 必须清理"
        );
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("SQLite 事务必须保留")
                .phase,
            "preparing",
            "本切片不能推进 SQLite prepared"
        );
    }

    #[test]
    fn bundle_publish_reaches_stable_prepared_without_activating_any_consumer() {
        let mut harness = Harness::new();
        let mut plan = harness.save_single_plan("alpha");
        let origin = PathBuf::from(&plan.origins[0].original_path);
        let origin_before = fs::symlink_metadata(&origin).expect("应读取 Origin 身份");
        let (transaction_id, journal) = harness.begin_preparing_with_journal(&plan);
        let transaction = harness.transaction(&transaction_id).expect("事务必须存在");
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");
        plan.status = TakeoverV2PlanStatus::Consumed;
        prepare_takeover_v2_candidate(&harness.paths, &lock, &plan, &journal)
            .expect("应准备完整 Candidate");
        update_takeover_v2_journal_to_prepared(&harness.paths, &lock, &transaction, &plan)
            .expect("应推进 formal Prepared");
        drop(lock);

        let staged_bundle = harness
            .paths
            .staging_root()
            .join(&transaction_id)
            .join("bundle");
        let mut staged_root_identity = None;
        publish_takeover_v2_prepared_bundle_with_hook(
            &harness.paths,
            &mut harness.storage,
            &transaction_id,
            202,
            &mut |checkpoint| {
                if checkpoint == BundlePublishCheckpoint::BeforeAtomicPublish {
                    let metadata = fs::symlink_metadata(&staged_bundle)
                        .expect("应读取发布前 staged root 身份");
                    staged_root_identity = Some((
                        metadata.dev(),
                        metadata.ino(),
                        metadata.mode(),
                        metadata.nlink(),
                    ));
                }
                Ok(())
            },
        )
        .expect("应发布完整 Bundle 并推进 SQLite prepared");

        let bundle = harness.paths.bundle_directory(&plan.bundle_id);
        let final_metadata = fs::symlink_metadata(&bundle).expect("应读取 final root 身份");
        assert_eq!(
            (
                final_metadata.dev(),
                final_metadata.ino(),
                final_metadata.mode(),
                final_metadata.nlink(),
            ),
            staged_root_identity.expect("发布前必须记录 staged root 身份"),
            "final root 必须是同一个 staged source"
        );
        let member = bundle
            .join("contents")
            .join(&plan.content_id)
            .join("members/alpha");
        assert!(member.join("SKILL.md").is_file());
        assert!(member.join("helper.txt").is_file());
        assert!(!bundle.join("current").exists(), "本阶段不能建立 current");
        let staging = harness.paths.staging_root().join(&transaction_id);
        assert!(
            !staging.exists()
                || fs::read_dir(&staging)
                    .expect("应读取空 staging")
                    .next()
                    .is_none(),
            "稳定 prepared 不能残留 staged Bundle"
        );
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("prepared 事务必须保留")
                .phase,
            "prepared"
        );
        let formal: TakeoverV2Journal = serde_json::from_slice(
            &fs::read(
                harness
                    .paths
                    .journals_root()
                    .join(journal_file_name(&transaction_id)),
            )
            .expect("formal 必须保留"),
        )
        .expect("formal 必须可解析");
        assert_eq!(formal.phase, TakeoverV2JournalPhase::Prepared);
        let origin_after = fs::symlink_metadata(&origin).expect("Origin 必须保持原状");
        assert_eq!(
            (origin_after.dev(), origin_after.ino(), origin_after.mode()),
            (
                origin_before.dev(),
                origin_before.ino(),
                origin_before.mode()
            )
        );
        assert!(
            harness
                .storage
                .read_mounts()
                .expect("应读取 Mount")
                .is_empty()
        );
        assert!(
            harness
                .storage
                .managed_bundle_notice_rows()
                .expect("应读取领域 Bundle")
                .is_empty(),
            "prepared 阶段不能提前写入领域 Bundle"
        );
    }

    #[test]
    fn prepublish_recovery_aborts_and_cleans_all_legal_staging_shapes() {
        for shape in ["a", "b", "c", "d"] {
            let mut harness = Harness::new();
            let mut plan = harness.save_single_plan("alpha");
            let origin = PathBuf::from(&plan.origins[0].original_path);
            let origin_before = fs::symlink_metadata(&origin).expect("应读取 Origin 身份");
            let (transaction_id, _) = harness.begin_with_prepared_candidate(&mut plan);
            let staging = harness.paths.staging_root().join(&transaction_id);
            let bundle = staging.join("bundle");
            let contents = bundle.join("contents");
            match shape {
                "a" => {}
                "b" => {
                    fs::create_dir(&bundle).expect("应建立 B shape");
                    fs::set_permissions(&bundle, fs::Permissions::from_mode(0o700))
                        .expect("B shape 必须使用 production mode");
                }
                "c" => {
                    fs::create_dir(&bundle).expect("应建立 B shape");
                    fs::set_permissions(&bundle, fs::Permissions::from_mode(0o700))
                        .expect("B shape 必须使用 production mode");
                    fs::create_dir(&contents).expect("应建立 C shape");
                    fs::set_permissions(&contents, fs::Permissions::from_mode(0o700))
                        .expect("C shape 必须使用 production mode");
                }
                "d" => {
                    fs::create_dir(&bundle).expect("应建立 B shape");
                    fs::set_permissions(&bundle, fs::Permissions::from_mode(0o700))
                        .expect("B shape 必须使用 production mode");
                    fs::create_dir(&contents).expect("应建立 C shape");
                    fs::set_permissions(&contents, fs::Permissions::from_mode(0o700))
                        .expect("C shape 必须使用 production mode");
                    fs::rename(staging.join("candidate"), contents.join(&plan.content_id))
                        .expect("应建立 D shape");
                }
                _ => unreachable!(),
            }

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("合法生效前 shape 应安全回退");

            assert!(
                harness.transaction(&transaction_id).is_none(),
                "shape={shape}"
            );
            assert!(!staging.exists(), "shape={shape}");
            assert!(
                !harness
                    .paths
                    .journals_root()
                    .join(journal_file_name(&transaction_id))
                    .exists(),
                "shape={shape}"
            );
            assert!(
                !harness.paths.bundle_directory(&plan.bundle_id).exists(),
                "shape={shape}"
            );
            let origin_after = fs::symlink_metadata(&origin).expect("Origin 必须保持原状");
            assert_eq!(
                (origin_after.dev(), origin_after.ino(), origin_after.mode()),
                (
                    origin_before.dev(),
                    origin_before.ino(),
                    origin_before.mode()
                ),
                "shape={shape}"
            );
        }
    }

    #[test]
    fn postpublish_recovery_advances_e_shape_to_sqlite_prepared() {
        for staging_state in ["empty", "absent"] {
            let mut harness = Harness::new();
            let mut plan = harness.save_single_plan("alpha");
            let (transaction_id, _) = harness.begin_with_prepared_candidate(&mut plan);
            let staging = harness.paths.staging_root().join(&transaction_id);
            let bundle = staging.join("bundle");
            let contents = bundle.join("contents");
            fs::create_dir(&bundle).expect("应建立 B shape");
            fs::set_permissions(&bundle, fs::Permissions::from_mode(0o700))
                .expect("B shape 必须使用 production mode");
            fs::create_dir(&contents).expect("应建立 C shape");
            fs::set_permissions(&contents, fs::Permissions::from_mode(0o700))
                .expect("C shape 必须使用 production mode");
            fs::rename(staging.join("candidate"), contents.join(&plan.content_id))
                .expect("应建立 D shape");
            let final_bundle = harness.paths.bundle_directory(&plan.bundle_id);
            fs::rename(&bundle, &final_bundle).expect("应模拟一次 atomic publish");
            if staging_state == "absent" {
                fs::remove_dir(&staging).expect("应模拟发布后清理空 staging");
            }
            let final_before = fs::symlink_metadata(&final_bundle).expect("应读取 final 身份");
            let formal_path = harness
                .paths
                .journals_root()
                .join(journal_file_name(&transaction_id));
            let formal_before = fs::read(&formal_path).expect("应读取 formal");
            let formal_identity_before =
                fs::symlink_metadata(&formal_path).expect("应读取 formal 身份");

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("E shape 必须自动恢复到 prepared");
            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 301)
                .expect("prepared E shape 第二次恢复必须幂等");

            let transaction = harness
                .transaction(&transaction_id)
                .expect("prepared 事务必须保留");
            assert_eq!(transaction.status, "in_progress", "state={staging_state}");
            assert_eq!(transaction.phase, "prepared", "state={staging_state}");
            assert!(
                final_bundle
                    .join("contents")
                    .join(&plan.content_id)
                    .join("members/alpha/SKILL.md")
                    .is_file(),
                "state={staging_state}"
            );
            assert!(!final_bundle.join("current").exists());
            let final_after = fs::symlink_metadata(&final_bundle).expect("final 必须保留");
            assert_eq!(
                (final_after.dev(), final_after.ino(), final_after.mode()),
                (final_before.dev(), final_before.ino(), final_before.mode()),
                "state={staging_state}"
            );
            assert_eq!(
                fs::read(&formal_path).expect("formal 必须保留"),
                formal_before,
                "state={staging_state}"
            );
            let formal_identity_after =
                fs::symlink_metadata(&formal_path).expect("formal 身份必须保留");
            assert_eq!(
                (
                    formal_identity_after.dev(),
                    formal_identity_after.ino(),
                    formal_identity_after.mode()
                ),
                (
                    formal_identity_before.dev(),
                    formal_identity_before.ino(),
                    formal_identity_before.mode()
                ),
                "state={staging_state}"
            );
            assert!(
                !staging.exists()
                    || fs::read_dir(&staging)
                        .expect("应读取空 staging")
                        .next()
                        .is_none(),
                "state={staging_state}"
            );
            assert!(
                harness
                    .storage
                    .read_mounts()
                    .expect("应读取 Mount")
                    .is_empty()
            );
            assert!(
                harness
                    .storage
                    .managed_bundle_notice_rows()
                    .expect("应读取领域 Bundle")
                    .is_empty()
            );
        }
    }

    #[test]
    fn bundle_publish_checkpoint_interruptions_recover_by_atomic_publish_boundary() {
        for interrupted_at in [
            BundlePublishCheckpoint::B,
            BundlePublishCheckpoint::C,
            BundlePublishCheckpoint::D,
            BundlePublishCheckpoint::E,
        ] {
            let mut harness = Harness::new();
            let mut plan = harness.save_single_plan("alpha");
            let (transaction_id, _) = harness.begin_with_prepared_candidate(&mut plan);

            publish_takeover_v2_prepared_bundle_with_hook(
                &harness.paths,
                &mut harness.storage,
                &transaction_id,
                202,
                &mut |checkpoint| {
                    if checkpoint == interrupted_at {
                        return Err(TakeoverV2LifecycleError::RecoveryBlocked(format!(
                            "测试模拟 {checkpoint:?} 持久化后的 hard exit"
                        )));
                    }
                    Ok(())
                },
            )
            .expect_err("checkpoint 必须在 SQLite prepared 前中断");
            assert_eq!(
                harness
                    .transaction(&transaction_id)
                    .expect("中断事务必须保留")
                    .phase,
                "preparing"
            );

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("持久化 checkpoint 必须可恢复");

            let final_bundle = harness.paths.bundle_directory(&plan.bundle_id);
            if interrupted_at == BundlePublishCheckpoint::E {
                let transaction = harness
                    .transaction(&transaction_id)
                    .expect("E 后必须前进到 prepared");
                assert_eq!(transaction.status, "in_progress");
                assert_eq!(transaction.phase, "prepared");
                assert!(final_bundle.exists());
                assert!(!final_bundle.join("current").exists());
            } else {
                assert!(harness.transaction(&transaction_id).is_none());
                assert!(!final_bundle.exists());
            }
        }
    }

    #[test]
    fn bundle_publish_revalidates_stable_state_immediately_before_sqlite_prepared() {
        for changed in ["formal", "phase_temp", "staging", "final"] {
            let mut harness = Harness::new();
            let mut plan = harness.save_single_plan("alpha");
            let (transaction_id, _) = harness.begin_with_prepared_candidate(&mut plan);
            let formal = harness
                .paths
                .journals_root()
                .join(journal_file_name(&transaction_id));
            let phase_temp = harness
                .paths
                .journals_root()
                .join(phase_temp_file_name(&transaction_id));
            let staging = harness.paths.staging_root().join(&transaction_id);
            let final_extra = harness
                .paths
                .bundle_directory(&plan.bundle_id)
                .join("external");

            let result = publish_takeover_v2_prepared_bundle_with_hook(
                &harness.paths,
                &mut harness.storage,
                &transaction_id,
                202,
                &mut |checkpoint| {
                    if checkpoint != BundlePublishCheckpoint::BeforePreparedCommit {
                        return Ok(());
                    }
                    match changed {
                        "formal" => OpenOptions::new()
                            .append(true)
                            .open(&formal)
                            .expect("应打开 formal")
                            .write_all(b"\n")
                            .expect("应修改 formal"),
                        "phase_temp" => {
                            fs::write(&phase_temp, b"external phase temp")
                                .expect("应插入 phase temp");
                        }
                        "staging" => {
                            fs::write(staging.join("external"), b"external staging")
                                .expect("应插入 staging 条目");
                        }
                        "final" => {
                            fs::write(&final_extra, b"external final")
                                .expect("应修改 final Bundle");
                        }
                        _ => unreachable!(),
                    }
                    Ok(())
                },
            );

            result.expect_err("DB prepared 前的外部变化必须阻止阶段推进");
            assert_eq!(
                harness
                    .transaction(&transaction_id)
                    .expect("受影响事务必须保留")
                    .phase,
                "preparing",
                "changed={changed}"
            );
            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("异常 E shape 只应阻塞当前事务");
            assert_eq!(
                harness
                    .transaction(&transaction_id)
                    .expect("异常现场必须保留")
                    .status,
                "blocked",
                "changed={changed}"
            );
            match changed {
                "formal" => assert!(
                    fs::read(&formal).expect("formal 必须保留").ends_with(b"\n"),
                    "changed={changed}"
                ),
                "phase_temp" => assert_eq!(
                    fs::read(&phase_temp).expect("phase temp 必须保留"),
                    b"external phase temp",
                    "changed={changed}"
                ),
                "staging" => assert_eq!(
                    fs::read(staging.join("external")).expect("staging 条目必须保留"),
                    b"external staging",
                    "changed={changed}"
                ),
                "final" => assert_eq!(
                    fs::read(&final_extra).expect("final 外部条目必须保留"),
                    b"external final",
                    "changed={changed}"
                ),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn e_recovery_revalidates_final_immediately_before_sqlite_prepared() {
        let mut harness = Harness::new();
        let mut plan = harness.save_single_plan("alpha");
        let (transaction_id, _) = harness.begin_with_prepared_candidate(&mut plan);
        publish_takeover_v2_prepared_bundle_with_hook(
            &harness.paths,
            &mut harness.storage,
            &transaction_id,
            202,
            &mut |checkpoint| {
                if checkpoint == BundlePublishCheckpoint::E {
                    return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                        "测试停在 E".to_owned(),
                    ));
                }
                Ok(())
            },
        )
        .expect_err("应停在 DB prepared 之前");
        let transaction = harness.transaction(&transaction_id).expect("事务必须存在");
        let final_extra = harness
            .paths
            .bundle_directory(&plan.bundle_id)
            .join("external-during-recovery");
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");

        recover_pre_effect_takeover_v2_transaction_with_hook(
            &harness.paths,
            &lock,
            &mut harness.storage,
            &transaction,
            300,
            &mut || Ok(()),
            &mut || {
                fs::write(&final_extra, b"external final").expect("应在恢复提交前修改 final");
                Ok(())
            },
        )
        .expect_err("E 恢复必须重新验证 final");
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务必须保留")
                .phase,
            "preparing"
        );
        assert_eq!(
            fs::read(&final_extra).expect("外部 final 条目必须保留"),
            b"external final"
        );
        drop(lock);

        recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 301)
            .expect("异常 E 只应阻塞当前事务");
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("异常 E 必须保留")
                .status,
            "blocked"
        );
        assert!(final_extra.exists());
    }

    #[test]
    fn invalid_prepared_filesystem_states_block_and_preserve_every_entry() {
        for variant in [
            "final_with_staged",
            "final_current",
            "final_unknown",
            "final_hardlink",
            "prepared_missing_final",
        ] {
            let mut harness = Harness::new();
            let mut plan = harness.save_single_plan("alpha");
            let (transaction_id, _) = harness.begin_with_prepared_candidate(&mut plan);
            let final_bundle = harness.paths.bundle_directory(&plan.bundle_id);
            let staging = harness.paths.staging_root().join(&transaction_id);
            let preserved_path: PathBuf;
            match variant {
                "final_with_staged" => {
                    fs::create_dir(&final_bundle).expect("应插入共存 final");
                    fs::set_permissions(&final_bundle, fs::Permissions::from_mode(0o700))
                        .expect("共存 final 应使用可审计 mode");
                    fs::write(final_bundle.join("external"), b"external final")
                        .expect("应写共存 final 内容");
                    preserved_path = final_bundle.join("external");
                }
                "final_current" | "final_unknown" | "final_hardlink" => {
                    publish_takeover_v2_prepared_bundle_with_hook(
                        &harness.paths,
                        &mut harness.storage,
                        &transaction_id,
                        202,
                        &mut |checkpoint| {
                            if checkpoint == BundlePublishCheckpoint::E {
                                return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                                    "测试停在 E".to_owned(),
                                ));
                            }
                            Ok(())
                        },
                    )
                    .expect_err("应停在 DB prepared 之前");
                    match variant {
                        "final_current" => {
                            preserved_path = final_bundle.join("current");
                            std::os::unix::fs::symlink(
                                format!("contents/{}", plan.content_id),
                                &preserved_path,
                            )
                            .expect("应插入 current");
                        }
                        "final_unknown" => {
                            preserved_path = final_bundle.join("unknown");
                            fs::write(&preserved_path, b"unknown").expect("应插入未知 final 条目");
                        }
                        "final_hardlink" => {
                            let member_file = final_bundle
                                .join("contents")
                                .join(&plan.content_id)
                                .join("members/alpha/helper.txt");
                            preserved_path = harness._temp.path().join("external-hardlink");
                            fs::hard_link(&member_file, &preserved_path)
                                .expect("应增加 final 文件链接数");
                        }
                        _ => unreachable!(),
                    }
                }
                "prepared_missing_final" => {
                    publish_takeover_v2_prepared_bundle(
                        &harness.paths,
                        &mut harness.storage,
                        &transaction_id,
                        202,
                    )
                    .expect("应先进入稳定 prepared");
                    preserved_path = harness._temp.path().join("removed-final-backup");
                    fs::rename(&final_bundle, &preserved_path)
                        .expect("应模拟 prepared final 被外部移走");
                }
                _ => unreachable!(),
            }

            recover_pre_effect_takeover_v2_transactions(&harness.paths, &mut harness.storage, 300)
                .expect("非法 prepared 状态只应阻塞当前事务");

            assert_eq!(
                harness
                    .transaction(&transaction_id)
                    .expect("非法状态事务必须保留")
                    .status,
                "blocked",
                "variant={variant}"
            );
            assert!(preserved_path.exists(), "variant={variant}");
            assert!(
                harness
                    .paths
                    .journals_root()
                    .join(journal_file_name(&transaction_id))
                    .exists(),
                "variant={variant}"
            );
            if variant == "final_with_staged" {
                assert!(staging.join("candidate").exists());
            }
        }
    }

    #[test]
    fn final_publish_no_replace_preserves_a_target_inserted_at_the_kernel_boundary() {
        let mut harness = Harness::new();
        let mut plan = harness.save_single_plan("alpha");
        let (transaction_id, _) = harness.begin_with_prepared_candidate(&mut plan);
        let final_bundle = harness.paths.bundle_directory(&plan.bundle_id);

        publish_takeover_v2_prepared_bundle_with_rename_boundary_hook(
            &harness.paths,
            &mut harness.storage,
            &transaction_id,
            202,
            &mut |_| Ok(()),
            &mut || {
                fs::create_dir(&final_bundle).expect("应在最终检查后插入目标");
                fs::write(final_bundle.join("external"), b"external final")
                    .expect("应写外部 final");
                Ok(())
            },
        )
        .expect_err("atomic no-replace 不能覆盖竞态目标");

        assert_eq!(
            fs::read(final_bundle.join("external")).expect("外部 final 必须保留"),
            b"external final"
        );
        assert!(
            harness
                .paths
                .staging_root()
                .join(&transaction_id)
                .join("bundle/contents")
                .join(&plan.content_id)
                .join("members/alpha/SKILL.md")
                .is_file(),
            "发布失败时完整 D shape 必须保留"
        );
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务必须保留")
                .phase,
            "preparing"
        );
    }

    #[test]
    fn final_publish_rejects_a_staged_source_replaced_after_fresh_audit() {
        let mut harness = Harness::new();
        let mut plan = harness.save_single_plan("alpha");
        let (transaction_id, _) = harness.begin_with_prepared_candidate(&mut plan);
        let staging = harness.paths.staging_root().join(&transaction_id);
        let staged_bundle = staging.join("bundle");
        let preserved_bundle = harness._temp.path().join("preserved-staged-bundle");
        let final_bundle = harness.paths.bundle_directory(&plan.bundle_id);

        publish_takeover_v2_prepared_bundle_with_hook(
            &harness.paths,
            &mut harness.storage,
            &transaction_id,
            202,
            &mut |checkpoint| {
                if checkpoint == BundlePublishCheckpoint::AfterFreshStagedAudit {
                    // 模拟同用户进程在 fresh audit 后替换 rename 的源目录。
                    fs::rename(&staged_bundle, &preserved_bundle)
                        .expect("应保留已审计通过的完整 D shape");
                    fs::create_dir(&staged_bundle).expect("应插入非法 staged source");
                    fs::write(staged_bundle.join("external"), b"external staged source")
                        .expect("应写入非法 staged source");
                }
                Ok(())
            },
        )
        .expect_err("原子发布前必须拒绝被替换的 staged source");

        assert!(
            preserved_bundle
                .join("contents")
                .join(&plan.content_id)
                .join("members/alpha/SKILL.md")
                .is_file(),
            "已审计通过的完整 D shape 必须原样保留"
        );
        assert!(
            !final_bundle.exists(),
            "非法 staged source 不能进入最终 Bundle"
        );
        assert_eq!(
            fs::read(staged_bundle.join("external")).expect("非法 source 现场必须保留"),
            b"external staged source"
        );
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务必须保留")
                .phase,
            "preparing"
        );
    }

    #[test]
    fn final_publish_rejects_phase_temp_inserted_during_fresh_staged_audit() {
        let mut harness = Harness::new();
        let mut plan = harness.save_single_plan("alpha");
        let (transaction_id, _) = harness.begin_with_prepared_candidate(&mut plan);
        let phase_temp = harness
            .paths
            .journals_root()
            .join(phase_temp_file_name(&transaction_id));
        let staged_bundle = harness
            .paths
            .staging_root()
            .join(&transaction_id)
            .join("bundle");
        let final_bundle = harness.paths.bundle_directory(&plan.bundle_id);

        publish_takeover_v2_prepared_bundle_with_hook(
            &harness.paths,
            &mut harness.storage,
            &transaction_id,
            202,
            &mut |checkpoint| {
                if checkpoint == BundlePublishCheckpoint::AfterFreshStagedAudit {
                    fs::write(&phase_temp, b"external phase temp")
                        .expect("应在 staged audit 期间插入 phase temp");
                }
                Ok(())
            },
        )
        .expect_err("staged audit 后的复核必须拒绝新插入的 phase temp");

        assert_eq!(
            fs::read(&phase_temp).expect("外部 phase temp 必须保留"),
            b"external phase temp"
        );
        assert!(
            !final_bundle.exists(),
            "phase temp 未通过复核时不能发布 final"
        );
        assert!(
            staged_bundle
                .join("contents")
                .join(&plan.content_id)
                .join("members/alpha/SKILL.md")
                .is_file(),
            "完整 D shape 必须保留"
        );
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务必须保留")
                .phase,
            "preparing"
        );
    }

    #[test]
    fn final_publish_rejects_an_unknown_staging_sibling_inserted_after_audit() {
        let mut harness = Harness::new();
        let mut plan = harness.save_single_plan("alpha");
        let (transaction_id, _) = harness.begin_with_prepared_candidate(&mut plan);
        let staging = harness.paths.staging_root().join(&transaction_id);
        let external = staging.join("external");
        let staged_member = staging
            .join("bundle/contents")
            .join(&plan.content_id)
            .join("members/alpha/SKILL.md");
        let final_bundle = harness.paths.bundle_directory(&plan.bundle_id);

        publish_takeover_v2_prepared_bundle_with_hook(
            &harness.paths,
            &mut harness.storage,
            &transaction_id,
            202,
            &mut |checkpoint| {
                if checkpoint == BundlePublishCheckpoint::AfterFreshStagedAudit {
                    fs::write(&external, b"external staging sibling")
                        .expect("应插入 staging 同级普通文件");
                }
                Ok(())
            },
        )
        .expect_err("完整 staging 审计必须拒绝未知同级条目");

        assert!(
            !final_bundle.exists(),
            "未知 staging 条目存在时不能发布 final"
        );
        assert_eq!(
            fs::read(&external).expect("外部 staging 条目必须保留"),
            b"external staging sibling"
        );
        assert!(staged_member.is_file(), "合法 D source 必须原样保留");
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务必须保留")
                .phase,
            "preparing"
        );
    }

    #[test]
    fn final_publish_rebinds_visible_managed_roots_after_prepublish_hook() {
        let mut harness = Harness::new();
        let mut plan = harness.save_single_plan("alpha");
        let (transaction_id, _) = harness.begin_with_prepared_candidate(&mut plan);
        let bundles_root = harness.paths.bundles_root();
        let bundles_backup = harness._temp.path().join("bundles-backup");
        let staged_member = harness
            .paths
            .staging_root()
            .join(&transaction_id)
            .join("bundle/contents")
            .join(&plan.content_id)
            .join("members/alpha/SKILL.md");
        let final_bundle = harness.paths.bundle_directory(&plan.bundle_id);

        publish_takeover_v2_prepared_bundle_with_hook(
            &harness.paths,
            &mut harness.storage,
            &transaction_id,
            202,
            &mut |checkpoint| {
                if checkpoint == BundlePublishCheckpoint::BeforeAtomicPublish {
                    // 模拟同用户进程把可见 bundles 根整体替换为新的空目录。
                    fs::rename(&bundles_root, &bundles_backup).expect("应保留 hook 前 bundles 根");
                    fs::create_dir(&bundles_root).expect("应重建可见 bundles 根");
                    fs::set_permissions(&bundles_root, fs::Permissions::from_mode(0o700))
                        .expect("可见 bundles 根应使用受管权限");
                }
                Ok(())
            },
        )
        .expect_err("根目录替换后必须拒绝发布");

        assert!(!final_bundle.exists(), "canonical final 必须保持缺失");
        assert!(
            !bundles_backup.join(&plan.bundle_id).exists(),
            "合法 D source 不能发布到 detached bundles 根"
        );
        assert!(staged_member.is_file(), "合法 D source 必须原样保留");
        assert_eq!(
            harness
                .transaction(&transaction_id)
                .expect("事务必须保留")
                .phase,
            "preparing"
        );
    }

    #[test]
    fn journal_phase_updater_rejects_missing_candidate() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, _) = harness.begin_preparing_with_journal(&plan);
        let transaction = harness.transaction(&transaction_id).expect("事务必须存在");
        let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");

        update_takeover_v2_journal_to_prepared(&harness.paths, &lock, &transaction, &plan)
            .expect_err("Candidate 不完整时不能推进 Prepared");

        let formal = harness
            .paths
            .journals_root()
            .join(journal_file_name(&transaction_id));
        let journal: TakeoverV2Journal =
            serde_json::from_slice(&fs::read(formal).expect("formal 必须保留"))
                .expect("formal 必须可解析");
        assert_eq!(journal.phase, TakeoverV2JournalPhase::Preparing);
        assert!(
            !harness
                .paths
                .journals_root()
                .join(phase_temp_file_name(&transaction_id))
                .exists()
        );
    }

    #[test]
    fn journal_phase_updater_preserves_both_sides_after_in_place_modification() {
        for point in ["before_swap", "after_swap"] {
            let mut harness = Harness::new();
            let mut plan = harness.save_single_plan("alpha");
            let (transaction_id, journal) = harness.begin_preparing_with_journal(&plan);
            let transaction = harness.transaction(&transaction_id).expect("事务必须存在");
            let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");
            plan.status = TakeoverV2PlanStatus::Consumed;
            prepare_takeover_v2_candidate(&harness.paths, &lock, &plan, &journal)
                .expect("应先准备完整 Candidate");

            let result = update_takeover_v2_journal_to_prepared_with_hooks(
                &harness.paths,
                &lock,
                &transaction,
                &plan,
                |formal, _| {
                    if point == "before_swap" {
                        OpenOptions::new()
                            .append(true)
                            .open(formal)
                            .expect("应打开 formal")
                            .write_all(b"\n")
                            .expect("应原地修改 formal");
                    }
                },
                |formal, _| {
                    if point == "after_swap" {
                        let canonical = fs::read(formal).expect("应读取 canonical formal");
                        // 内容和 inode 都不变时，完整 metadata 快照仍必须识别重写。
                        fs::write(formal, canonical).expect("应原地重写 canonical formal");
                    }
                },
            );

            result.expect_err("任一侧原地修改都必须阻塞 phase update");
            assert!(
                harness
                    .paths
                    .journals_root()
                    .join(journal_file_name(&transaction_id))
                    .exists(),
                "formal 必须保留，point={point}"
            );
            assert!(
                harness
                    .paths
                    .journals_root()
                    .join(phase_temp_file_name(&transaction_id))
                    .exists(),
                "phase temp 必须保留，point={point}"
            );
        }
    }

    #[test]
    fn journal_phase_updater_preserves_concurrently_replaced_entries() {
        for point in ["before_swap", "after_swap"] {
            let mut harness = Harness::new();
            let mut plan = harness.save_single_plan("alpha");
            let (transaction_id, journal) = harness.begin_preparing_with_journal(&plan);
            let transaction = harness.transaction(&transaction_id).expect("事务必须存在");
            let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");
            plan.status = TakeoverV2PlanStatus::Consumed;
            prepare_takeover_v2_candidate(&harness.paths, &lock, &plan, &journal)
                .expect("应先准备完整 Candidate");
            let external = b"external journal replacement";

            let result = update_takeover_v2_journal_to_prepared_with_hooks(
                &harness.paths,
                &lock,
                &transaction,
                &plan,
                |_, phase_temp| {
                    if point == "before_swap" {
                        fs::remove_file(phase_temp).expect("应移除原 phase temp");
                        fs::write(phase_temp, external).expect("应替换 phase temp inode");
                    }
                },
                |formal, _| {
                    if point == "after_swap" {
                        fs::remove_file(formal).expect("应移除 swap 后 formal");
                        fs::write(formal, external).expect("应替换 formal inode");
                    }
                },
            );

            result.expect_err("并发替换必须阻塞 phase update");
            let formal = harness
                .paths
                .journals_root()
                .join(journal_file_name(&transaction_id));
            let phase_temp = harness
                .paths
                .journals_root()
                .join(phase_temp_file_name(&transaction_id));
            if point == "before_swap" {
                assert_eq!(fs::read(&phase_temp).expect("替换 temp 必须保留"), external);
                assert!(formal.exists());
            } else {
                assert_eq!(fs::read(&formal).expect("替换 formal 必须保留"), external);
                assert!(phase_temp.exists());
            }
        }
    }

    #[test]
    fn journal_phase_updater_rejects_a_replaced_journals_root_at_each_hook() {
        for point in ["before_swap", "after_swap"] {
            let mut harness = Harness::new();
            let mut plan = harness.save_single_plan("alpha");
            let (transaction_id, journal) = harness.begin_preparing_with_journal(&plan);
            let transaction = harness.transaction(&transaction_id).expect("事务必须存在");
            let lock = acquire_lifecycle_lock(&harness.paths).expect("应取得生命周期锁");
            plan.status = TakeoverV2PlanStatus::Consumed;
            prepare_takeover_v2_candidate(&harness.paths, &lock, &plan, &journal)
                .expect("应先准备完整 Candidate");
            let journals_root = harness.paths.journals_root();
            let journals_backup = harness
                ._temp
                .path()
                .join(format!("phase-journals-backup-{point}"));

            let result = update_takeover_v2_journal_to_prepared_with_hooks(
                &harness.paths,
                &lock,
                &transaction,
                &plan,
                |_, _| {
                    if point == "before_swap" {
                        fs::rename(&journals_root, &journals_backup)
                            .expect("应保留 swap 前 journals 根");
                        fs::create_dir(&journals_root).expect("应重建可见 journals 根");
                        fs::set_permissions(&journals_root, fs::Permissions::from_mode(0o700))
                            .expect("可见 journals 根应使用受管权限");
                    }
                },
                |_, _| {
                    if point == "after_swap" {
                        fs::rename(&journals_root, &journals_backup)
                            .expect("应保留 swap 后 journals 根");
                        fs::create_dir(&journals_root).expect("应重建可见 journals 根");
                        fs::set_permissions(&journals_root, fs::Permissions::from_mode(0o700))
                            .expect("可见 journals 根应使用受管权限");
                    }
                },
            );

            result.expect_err("任一 hook 替换 journals 根都必须阻塞 phase update");
            assert!(
                fs::read_dir(&journals_root)
                    .expect("应读取 canonical journals 根")
                    .next()
                    .is_none(),
                "canonical journals 必须保持空，point={point}"
            );
            let formal: TakeoverV2Journal = serde_json::from_slice(
                &fs::read(journals_backup.join(journal_file_name(&transaction_id)))
                    .expect("detached formal 必须保留"),
            )
            .expect("detached formal 必须可解析");
            let temporary: TakeoverV2Journal = serde_json::from_slice(
                &fs::read(journals_backup.join(phase_temp_file_name(&transaction_id)))
                    .expect("detached phase temp 必须保留"),
            )
            .expect("detached phase temp 必须可解析");
            if point == "before_swap" {
                assert_eq!(formal.phase, TakeoverV2JournalPhase::Preparing);
                assert_eq!(temporary.phase, TakeoverV2JournalPhase::Prepared);
            } else {
                assert_eq!(formal.phase, TakeoverV2JournalPhase::Prepared);
                assert_eq!(temporary.phase, TakeoverV2JournalPhase::Preparing);
            }
            assert_eq!(
                harness
                    .transaction(&transaction_id)
                    .expect("事务必须保留")
                    .phase,
                "preparing",
                "point={point}"
            );
        }
    }

    #[test]
    fn no_replace_writer_preserves_a_competing_formal_target() {
        let harness = Harness::new();
        let parent = File::open(harness.paths.journals_root()).expect("应打开 Journal 目录");
        let name = OsStr::new("takeover-v2-race.json");
        let target = harness.paths.journals_root().join(name);

        write_new_atomic_at_test_hook(&parent, name, &target, b"ours", |_| {
            fs::write(&target, b"external").expect("应模拟竞态插入正式目标");
        })
        .expect_err("首次 Journal 不能覆盖竞态目标");

        assert_eq!(fs::read(&target).expect("竞态目标必须保留"), b"external");
        assert!(
            fs::read_dir(harness.paths.journals_root())
                .expect("应读取 Journal 目录")
                .all(|entry| !entry
                    .expect("应读取目录项")
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp-"))
        );
    }

    #[test]
    fn no_replace_writer_never_deletes_a_replaced_temporary_entry() {
        let harness = Harness::new();
        let parent = File::open(harness.paths.journals_root()).expect("应打开 Journal 目录");
        let name = OsStr::new("takeover-v2-temp-race.json");
        let target = harness.paths.journals_root().join(name);
        let replaced_path = RefCell::new(None::<PathBuf>);

        write_new_atomic_at_test_hook(&parent, name, &target, b"ours", |temporary_name| {
            let path = harness.paths.journals_root().join(temporary_name);
            fs::remove_file(&path).expect("应替换本次 temp");
            fs::write(&path, b"external-temp").expect("应插入外部 temp");
            *replaced_path.borrow_mut() = Some(path);
        })
        .expect_err("被替换的 temp 必须阻塞发布");

        let replaced_path = replaced_path.into_inner().expect("应记录替换路径");
        assert_eq!(
            fs::read(&replaced_path).expect("外部 temp 必须保留"),
            b"external-temp"
        );
        assert!(!target.exists());
    }

    #[test]
    fn journal_cleanup_preserves_the_same_inode_after_an_in_place_change() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let (transaction_id, journal) = harness.begin_without_journal(&plan);
        let name = OsString::from(journal_file_name(&transaction_id));
        let path = harness.paths.journals_root().join(&name);
        fs::write(
            &path,
            serde_json::to_vec_pretty(&journal).expect("应序列化 Journal"),
        )
        .expect("应写 Journal");
        let journals = File::open(harness.paths.journals_root()).expect("应打开 Journal 目录");
        let (_, owned) =
            read_takeover_v2_journal_at(&journals, &name, &path).expect("应读取并认领 Journal");
        OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("应打开同一 Journal")
            .write_all(b"\n")
            .expect("应原地修改 Journal");

        remove_owned_journal(&harness.paths, &journals, &owned)
            .expect_err("原地修改后的 Journal 必须保留");

        assert!(path.exists());
    }

    fn create_candidate_skeleton(
        paths: &ApplicationPaths,
        transaction_id: &str,
        skill_name: &str,
        created_depth: usize,
    ) -> Vec<PathBuf> {
        let levels = vec![
            paths.staging_root().join(transaction_id),
            paths.staging_root().join(transaction_id).join("candidate"),
            paths
                .staging_root()
                .join(transaction_id)
                .join("candidate/members"),
            paths
                .staging_root()
                .join(transaction_id)
                .join("candidate/members")
                .join(skill_name),
        ];
        for path in levels.iter().take(created_depth) {
            fs::create_dir(path).expect("应逐层创建候选骨架");
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .expect("候选骨架应使用协议权限");
        }
        levels
    }

    fn save_sibling_plan(harness: &mut Harness) -> TakeoverV2Plan {
        harness.save_single_plan_with_setup("alpha", |root| {
            fs::set_permissions(root, fs::Permissions::from_mode(0o755))
                .expect("应固定源 root 最终权限");
            for directory_name in ["a", "b"] {
                let directory = root.join(directory_name);
                fs::create_dir(&directory).expect("应创建源 sibling 目录");
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o755))
                    .expect("应固定源 sibling 目录最终权限");
                let file = directory.join(format!("{directory_name}.txt"));
                fs::write(&file, format!("{directory_name} content")).expect("应写源 sibling 文件");
                fs::set_permissions(file, fs::Permissions::from_mode(0o644))
                    .expect("应固定源 sibling 文件最终权限");
            }
        })
    }

    fn create_sibling_candidate(
        paths: &ApplicationPaths,
        transaction_id: &str,
        source_root: &Path,
        sibling_b_state: &str,
    ) -> Vec<PathBuf> {
        let levels = create_candidate_skeleton(paths, transaction_id, "alpha", 4);
        let candidate_root = &levels[3];
        for relative in ["SKILL.md", "helper.txt"] {
            copy_test_candidate_file(source_root, candidate_root, relative);
        }
        let candidate_a = candidate_root.join("a");
        fs::create_dir(&candidate_a).expect("应创建 Candidate A");
        copy_test_candidate_file(source_root, candidate_root, "a/a.txt");
        fs::set_permissions(&candidate_a, fs::Permissions::from_mode(0o755))
            .expect("Candidate A 应模拟已完成目录 chmod");

        if sibling_b_state != "missing" {
            let candidate_b = candidate_root.join("b");
            fs::create_dir(&candidate_b).expect("应创建 Candidate B");
            fs::set_permissions(&candidate_b, fs::Permissions::from_mode(0o700))
                .expect("Candidate B 应保持构建中权限");
            if sibling_b_state == "complete" {
                copy_test_candidate_file(source_root, candidate_root, "b/b.txt");
            } else {
                let source_b = fs::read(source_root.join("b/b.txt")).expect("应读取源 B 文件");
                let candidate_b_file = candidate_b.join("b.txt");
                fs::write(&candidate_b_file, &source_b[..source_b.len() / 2])
                    .expect("应写 Candidate B 的合法半文件");
                fs::set_permissions(&candidate_b_file, fs::Permissions::from_mode(0o600))
                    .expect("Candidate B 半文件应保持 0600");
            }
        }
        levels
    }

    fn copy_test_candidate_file(source_root: &Path, candidate_root: &Path, relative: &str) {
        let source = source_root.join(relative);
        let destination = candidate_root.join(relative);
        fs::write(&destination, fs::read(&source).expect("应读取测试源文件"))
            .expect("应复制测试 Candidate 文件");
        let final_mode = fs::symlink_metadata(source)
            .expect("应读取测试源权限")
            .mode()
            & 0o7777;
        fs::set_permissions(destination, fs::Permissions::from_mode(final_mode))
            .expect("测试 Candidate 文件应使用最终权限");
    }

    fn reset_plan_storage_identity(paths: &ApplicationPaths, plan: &mut TakeoverV2Plan) {
        plan.id = Uuid::new_v4().to_string();
        plan.bundle_id = Uuid::new_v4().to_string();
        plan.member_id = Uuid::new_v4().to_string();
        plan.content_id = Uuid::new_v4().to_string();
        let managed = paths.bundle_directory(&plan.bundle_id);
        plan.managed_directory = managed.to_string_lossy().into_owned();
        plan.content_directory = managed
            .join("contents")
            .join(&plan.content_id)
            .to_string_lossy()
            .into_owned();
        plan.expected_target = managed
            .join("current/members")
            .join(&plan.skill_name)
            .to_string_lossy()
            .into_owned();
        for target in &mut plan.targets {
            target.id = Uuid::new_v4().to_string();
            target.mount_id = Uuid::new_v4().to_string();
            target.expected_target = plan.expected_target.clone();
        }
        plan.seal = takeover_v2_plan_seal(plan);
    }

    fn write_skill(root: &Path, name: &str) {
        fs::create_dir_all(root).expect("应创建 Skill 目录");
        fs::write(
            root.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} description\n---\n# {name}\n"),
        )
        .expect("应写 Skill");
        fs::write(root.join("helper.txt"), format!("{name} helper")).expect("应写辅助文件");
    }

    fn observation_from_origin(origin: &TakeoverV2Origin) -> InventoryObservation {
        InventoryObservation {
            id: origin.observation_id.clone(),
            skill_name: origin.observation_skill_name.clone(),
            declared_name: origin.observation_declared_name.clone(),
            skill_root: origin.original_path.clone(),
            skill_file: origin.observation_skill_file.clone(),
            location_kind: origin.observation_location_kind,
            metadata_status: origin.observation_metadata_status,
            observed_by: origin.observation_observed_by.clone(),
            observed_fingerprint: origin.observation_fingerprint.clone(),
            root_key: origin.root_key,
            project_id: origin.project_id.clone(),
            stale: origin.observation_stale,
            management_kind: origin.observation_management_kind,
            management_evidence: origin.observation_management_evidence.clone(),
        }
    }

    fn valid_temp_path(paths: &ApplicationPaths, transaction_id: &str) -> PathBuf {
        paths.journals_root().join(format!(
            ".{}.tmp-{}",
            journal_file_name(transaction_id),
            Uuid::new_v4()
        ))
    }
}
