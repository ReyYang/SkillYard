use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::Read,
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
    content::ContentValidationError,
    domain::{
        MountScope, ScanRootKey, SupportedAppId, TakeoverTargetInitialState, TakeoverV2Origin,
        TakeoverV2Plan, TakeoverV2PlanStatus, TakeoverV2Target,
    },
    lifecycle::{
        LifecycleError, LifecycleLock, acquire_lifecycle_lock, entry_metadata_at,
        open_directory_at, open_managed_directory_from_root, open_regular_file_at,
        read_entry_names_os_from_handle, unlink_at, write_new_atomic_at,
    },
    paths::{ApplicationPaths, SupportedAppPathConfig},
    storage::{Storage, StorageError, StoredTakeoverV2Transaction},
    takeover_lifecycle::{
        TakeoverLifecycleError, TakeoverOriginalEntry, collect_original_manifest_at,
        validate_original_manifest_at,
    },
};

const TAKEOVER_V2_JOURNAL_VERSION: u32 = 1;
const MAX_TAKEOVER_V2_JOURNAL_BYTES: usize = 1024 * 1024;

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

/// Journal 只保存跨 SQLite 与文件系统边界恢复所需的不可变事实。
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
    phase: TakeoverV2JournalPhase,
    staging_relative: String,
    bundle_relative: String,
    content_relative: String,
    current_target: String,
    origins: Vec<TakeoverV2OriginManifest>,
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

/// 本批只把事务推进到 preparing；候选复制与任何可见路径切换由后续批次负责。
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
    lifecycle_lock.recheck(paths)?;
    Ok(transaction_id)
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
    if let Some(error) = &transaction.recovery_validation_error {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(error.clone()));
    }
    let plan = storage.read_takeover_v2_plan_for_transaction(transaction)?;
    if !matches!(transaction.phase.as_str(), "journal_pending" | "preparing") {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(format!(
            "事务已进入本批不能恢复的阶段：{}",
            transaction.phase
        )));
    }

    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    let temporary = inspect_takeover_v2_temporary_journal(paths, &journals, transaction, &plan)?;
    let name = OsString::from(journal_file_name(&transaction.id));
    let path = paths.journals_root().join(&name);
    let metadata = entry_metadata_at(&journals, &name)
        .map_err(|source| v2_io("检查正式 Journal", &path, source))?;

    if transaction.status == "aborted" {
        if metadata.is_some() {
            let (journal, owned) = read_takeover_v2_journal_at(&journals, &name, &path)?;
            validate_takeover_v2_journal_contract(&journal, transaction, &plan)?;
            validate_all_origin_manifests(paths, &plan, &journal.origins)?;
            validate_all_targets(paths, &plan)?;
            remove_owned_journal(paths, &journals, &owned)?;
        } else {
            validate_all_origins_without_journal(paths, &plan)?;
            validate_all_targets(paths, &plan)?;
        }
        if let Some(temporary) = temporary.as_ref() {
            remove_owned_journal(paths, &journals, temporary)?;
        }
        storage.forget_terminal_takeover_v2_transaction(&transaction.id)?;
        return Ok(());
    }
    if transaction.status != "in_progress" {
        return Err(TakeoverV2LifecycleError::RecoveryBlocked(format!(
            "事务状态不属于生效前恢复窗口：{}",
            transaction.status
        )));
    }

    match (transaction.phase.as_str(), metadata) {
        ("journal_pending", None) => {
            // 没有正式 Journal 意味着原子 rename 尚未成功；只重验 Origin，不删除任何业务路径。
            validate_all_origins_without_journal(paths, &plan)?;
            validate_all_targets(paths, &plan)?;
            storage.abort_takeover_v2_transaction(&transaction.id, None, now)?;
            if let Some(temporary) = temporary.as_ref() {
                remove_owned_journal(paths, &journals, temporary)?;
            }
            storage.forget_terminal_takeover_v2_transaction(&transaction.id)?;
        }
        ("journal_pending" | "preparing", Some(_)) => {
            let (journal, owned) = read_takeover_v2_journal_at(&journals, &name, &path)?;
            validate_takeover_v2_journal_contract(&journal, transaction, &plan)?;
            validate_all_origin_manifests(paths, &plan, &journal.origins)?;
            validate_all_targets(paths, &plan)?;
            // 先把已验证的安全结论写入 SQLite；后续清理中断时 aborted 仍可幂等续做。
            storage.abort_takeover_v2_transaction(&transaction.id, None, now)?;
            remove_owned_journal(paths, &journals, &owned)?;
            if let Some(temporary) = temporary.as_ref() {
                remove_owned_journal(paths, &journals, temporary)?;
            }
            storage.forget_terminal_takeover_v2_transaction(&transaction.id)?;
        }
        ("preparing", None) => {
            return Err(TakeoverV2LifecycleError::RecoveryBlocked(
                "preparing 事务缺少正式 Journal".to_owned(),
            ));
        }
        _ => unreachable!("阶段已在入口限制"),
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
        phase: TakeoverV2JournalPhase::Preparing,
        staging_relative: format!("staging/{transaction_id}"),
        bundle_relative: bundle_relative.clone(),
        content_relative: format!("{bundle_relative}/contents/{}", plan.content_id),
        current_target: format!("contents/{}", plan.content_id),
        origins,
    })
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

/// phase 会持续变化，因此 contract hash 把它规范化后再覆盖其余全部字段。
fn takeover_v2_journal_contract_sha256(
    journal: &TakeoverV2Journal,
) -> Result<String, TakeoverV2LifecycleError> {
    let mut contract = journal.clone();
    contract.phase = TakeoverV2JournalPhase::Preparing;
    let bytes = serde_json::to_vec(&contract)?;
    Ok(hex_sha256(&bytes))
}

fn validate_takeover_v2_journal_contract(
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
        && journal.staging_relative == format!("staging/{}", transaction.id)
        && journal.bundle_relative == bundle_relative
        && journal.content_relative
            == format!("bundles/{}/contents/{}", plan.bundle_id, plan.content_id)
        && journal.current_target == format!("contents/{}", plan.content_id)
        && journal.phase == TakeoverV2JournalPhase::Preparing;
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

fn write_takeover_v2_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    journal: &TakeoverV2Journal,
) -> Result<(), TakeoverV2LifecycleError> {
    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    validate_journal_size_for_all_phases(journal)?;
    let bytes = serde_json::to_vec_pretty(journal)?;
    let name = OsString::from(journal_file_name(&journal.transaction_id));
    write_new_atomic_at(&journals, &name, &paths.journals_root().join(&name), &bytes)?;
    Ok(())
}

fn validate_journal_size_for_all_phases(
    journal: &TakeoverV2Journal,
) -> Result<(), TakeoverV2LifecycleError> {
    let mut candidate = journal.clone();
    let mut maximum = 0;
    for phase in [
        TakeoverV2JournalPhase::Preparing,
        TakeoverV2JournalPhase::Prepared,
        TakeoverV2JournalPhase::EffectStarted,
        TakeoverV2JournalPhase::StateCommitted,
        TakeoverV2JournalPhase::CleanupCompleted,
    ] {
        candidate.phase = phase;
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

fn read_takeover_v2_journal_at(
    journals: &File,
    name: &OsStr,
    path: &Path,
) -> Result<(TakeoverV2Journal, OwnedJournalEntry), TakeoverV2LifecycleError> {
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
    let journal = serde_json::from_slice(&bytes)?;
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
        journal,
        OwnedJournalEntry {
            name: name.to_os_string(),
            snapshot: expected_snapshot,
        },
    ))
}

fn inspect_takeover_v2_temporary_journal(
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
    // 名称不是所有权证明；只有完整且匹配 SQLite 合同的 temp 才能自动清理。
    let (journal, owned) = read_takeover_v2_journal_at(journals, &name, &path)?;
    validate_takeover_v2_journal_contract(&journal, transaction, plan)?;
    Ok(Some(owned))
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
            let skill_root = self.paths.home().join(".codex/skills").join(name);
            write_skill(&skill_root, name);
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
            let mut plan = self.save_single_plan(name);
            let skill_root = self.paths.home().join(".claude/skills").join(name);
            write_skill(&skill_root, name);
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
            assert_eq!(
                plan.origins[0].content_fingerprint, second_origin.content_fingerprint,
                "测试合同要求两份 Origin 内容相同"
            );
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
    }

    #[test]
    fn prepare_writes_complete_journal_then_advances_to_preparing() {
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
    }

    #[test]
    fn two_origin_journal_blocks_on_either_replacement_without_deleting_content() {
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
        let (transaction_id, _) = harness.begin_without_journal(&plan);
        harness
            .storage
            .update_takeover_v2_transaction_phase(&transaction_id, "preparing", 201)
            .expect("应推进 preparing");
        harness
            .storage
            .update_takeover_v2_transaction_phase(&transaction_id, "prepared", 202)
            .expect("应推进 prepared");

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
    fn contract_hash_ignores_only_mutable_phase() {
        let mut harness = Harness::new();
        let plan = harness.save_single_plan("alpha");
        let journal = build_takeover_v2_journal(&harness.paths, &Uuid::new_v4().to_string(), &plan)
            .expect("应建立 Journal");
        let mut advanced = journal.clone();
        advanced.phase = TakeoverV2JournalPhase::Prepared;
        assert_eq!(
            takeover_v2_journal_contract_sha256(&journal).expect("应计算 seal"),
            takeover_v2_journal_contract_sha256(&advanced).expect("应计算 seal")
        );
        advanced.skill_name = "changed".to_owned();
        assert_ne!(
            takeover_v2_journal_contract_sha256(&journal).expect("应计算 seal"),
            takeover_v2_journal_contract_sha256(&advanced).expect("应计算 seal")
        );
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
