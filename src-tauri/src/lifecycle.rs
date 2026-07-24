use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString, OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    mem::MaybeUninit,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Component, Path, PathBuf},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    content::{
        BundleCopyBudget, ContentValidationError, ValidatedSkillBundle,
        copy_single_skill_tree_into_open_directory, validate_single_skill_folder,
        validate_skill_bundle_folder,
    },
    domain::{
        BundleUpdateImpact, InstallCandidate, InstallInputKind, InstallMode, InstallPlan,
        SourceKind,
    },
    github_source::{
        GithubCatalogTarget, GithubSourceError, SourceTransport, prepare_github_install_snapshot,
        resolve_github_source,
    },
    paths::ApplicationPaths,
    source_input::{
        PreparedSourceKind, PreparedSourceSnapshot, SourceFilesystemIdentity, SourceInputError,
        prepare_archive_source, prepare_direct_url_source, prepare_editable_local_source,
        prepare_registered_editable_local_source,
    },
    storage::{
        BundleUpdateBatchChildOwner, NewInstallCandidate, NewInstallPlan, NewManualSource,
        NewSourceCatalogMember, Storage, StorageError, StoredInstallCandidate, StoredInstallPlan,
        StoredLifecycleTransaction, StoredSourceInstallCatalogMember, StoredSourceInstallSource,
    },
};

const PLAN_TTL_MILLIS: i64 = 30 * 60 * 1_000;
const JOURNAL_VERSION: u32 = 1;
const MAX_JOURNAL_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OwnedTreeCleanupManifest {
    root_device: u64,
    root_inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SealedTreeCleanupManifest {
    root_device: u64,
    root_inode: u64,
    entries: Vec<SealedTreeCleanupEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SealedTreeCleanupEntry {
    // 文件名使用原始字节的十六进制，避免 Journal 无法表示合法的非 UTF-8 文件名。
    parent_index: Option<usize>,
    name_hex: String,
    device: u64,
    inode: u64,
    entry_type: u64,
    size: i64,
    modified_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleFailpoint {
    None,
    AfterTransactionRecord,
    AfterCandidatePrepared,
    AfterTemporaryCurrentCreated,
    AfterCurrentActivated,
    AfterDomainCommit,
    HardExitAfterTransactionRecord,
    HardExitAfterCandidatePrepared,
    HardExitAfterCurrentActivated,
    HardExitAfterCandidatePublishedBeforePhase,
    HardExitAfterCurrentSwitchedBeforePhase,
    HardExitAfterDomainCommittedBeforeJournal,
    AfterMountTransactionRecord,
    AfterMountTargetApplied,
    AfterMountStateCommitted,
    HardExitAfterMountTransactionRecord,
    HardExitAfterMountJournalWrittenBeforePhase,
    HardExitAfterMountTargetAppliedBeforePhase,
    HardExitAfterMountTargetApplied,
    HardExitAfterMountStateCommittedBeforeJournal,
    HardExitAfterMountJournalRemovedBeforeForget,
    HardExitAfterFirstBatchMountStageJournalBeforePublish,
    HardExitAfterFirstBatchMountTargetAppliedBeforePhase,
    HardExitAfterFirstBatchMountQuarantineBeforeUnlink,
    ReplaceFirstBatchMountQuarantineWithUnknownBeforeDiscard,
    HardExitAfterFirstBatchMountDiscardBeforeUnlink,
    HardExitAfterFirstBatchMountRollbackBeforeProgress,
    HardExitAfterAllBatchMountTargetsAppliedBeforeState,
    HardExitAfterBatchMountStateCommittedBeforeJournal,
    AfterTakeoverOriginMovedBeforeProgress,
    AfterTakeoverMountStagedBeforeProgress,
    AfterFirstTakeoverOriginApplied,
    AfterFirstTakeoverTargetApplied,
    HardExitAfterTakeoverTransactionRecord,
    HardExitAfterTakeoverJournalTempSyncedBeforeRename,
    HardExitAfterTakeoverJournalWrittenBeforePhase,
    HardExitAfterTakeoverCandidatePreparedBeforePublish,
    HardExitAfterTakeoverCandidatePublishedBeforePhase,
    HardExitAfterTakeoverTemporaryCurrentCreatedBeforeSwitch,
    HardExitAfterTakeoverCurrentSwitchedBeforePhase,
    HardExitAfterTakeoverOriginMovedBeforeProgress,
    HardExitAfterTakeoverMountStagedBeforeProgress,
    HardExitAfterTakeoverOriginsAppliedBeforeState,
    HardExitAfterTakeoverStateCommittedBeforeJournal,
    HardExitAfterFirstTakeoverRecoveryRemovedBeforeProgress,
    HardExitAfterTakeoverJournalRemovedBeforeForget,
    HardExitAfterFirstTakeoverRollbackMountRemovedBeforeProgress,
    HardExitAfterFirstTakeoverRollbackOriginRestoredBeforeProgress,
    HardExitDuringTakeoverCandidateCleanup,
    HardExitDuringFirstTakeoverRecoveryRemoval,
    AfterSourceAssociationCandidatePrepared,
    AfterSourceAssociationCandidateDirectoryCreatedBeforeManifest,
    AfterSourceAssociationCurrentActivated,
    AfterSourceAssociationMountsApplied,
    AfterSourceAssociationStateCommitted,
    AfterSourceAssociationRollbackJournalRemovedBeforeForget,
    AfterRemovalTransactionRecord,
    AfterRemovalMountsJournalWrittenBeforePhase,
    AfterRemovalMountsIsolated,
    AfterRemovalBundleRenamedBeforeJournal,
    AfterRemovalBundleIsolated,
    AfterRemovalStateCommitted,
}

#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Content(#[from] ContentValidationError),
    #[error(transparent)]
    GithubSource(#[from] GithubSourceError),
    #[error("{0}")]
    SourceInput(String),
    #[error("文件夹路径包含无法保存的字符：{0}")]
    NonUnicodePath(String),
    #[error("安装 Plan 的前置状态已经变化，请重新生成安装计划")]
    PlanPreconditionChanged,
    #[error("当前 Source 没有可安装的新 Skill")]
    NoInstallableSourceMembers,
    #[error("安装目标已经存在，不能覆盖：{0}")]
    TargetOccupied(String),
    #[error("Central Store 路径不安全：{0}")]
    UnsafeCentralStore(String),
    #[error("已有另一个 SkillYard 实例正在执行生命周期操作")]
    LifecycleBusy,
    #[error("生命周期目录没有写权限，尚未开始安装：{0}")]
    PermissionPreflight(String),
    #[error("无法{action} {path}：{source}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("事务 Journal 无法解析：{0}")]
    InvalidJournal(#[from] serde_json::Error),
    #[error("事务 Journal 超过安全大小限制（{actual} 字节，限制 {limit} 字节）")]
    JournalTooLarge { actual: usize, limit: usize },
    #[error("事务恢复需要人工处理：{0}")]
    RecoveryBlocked(String),
    #[error("测试模拟中断：{0}")]
    SimulatedInterruption(&'static str),
}

impl From<SourceInputError> for LifecycleError {
    fn from(error: SourceInputError) -> Self {
        // SourceInputError 保持模块私有，只把稳定的中文产品错误带到公开 seam。
        Self::SourceInput(error.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum JournalPhase {
    JournalReady,
    CandidateReady,
    Activated,
    StateCommitted,
}

impl JournalPhase {
    fn as_storage_str(self) -> &'static str {
        match self {
            Self::JournalReady => "journal_ready",
            Self::CandidateReady => "candidate_ready",
            Self::Activated => "activated",
            Self::StateCommitted => "state_committed",
        }
    }

    fn activation_was_recorded(self) -> bool {
        matches!(self, Self::Activated | Self::StateCommitted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InstallJournal {
    version: u32,
    transaction_id: String,
    plan_id: String,
    bundle_id: String,
    member_id: String,
    skill_name: String,
    input_fingerprint: String,
    phase: JournalPhase,
    staging_relative: String,
    bundle_relative: String,
    content_relative: String,
    current_relative: String,
    current_target: String,
    /// 补装前的完整内容目标；全新安装没有旧目标。
    previous_current_target: Option<String>,
    stable_member_relative: String,
    members: Vec<InstallJournalMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InstallJournalMember {
    candidate_id: String,
    /// 保留的“不对应”成员没有 Source 路径；新候选始终为 `Some`。
    source_relative_path: Option<String>,
    skill_name: String,
    content_fingerprint: String,
    stable_member_relative: String,
}

pub(crate) struct LifecycleLock {
    file: File,
    root: File,
}

impl LifecycleLock {
    pub(crate) fn recheck(&self, paths: &ApplicationPaths) -> Result<(), LifecycleError> {
        ensure_open_directory_matches_managed_path(paths, &self.root, paths.data_root())?;
        let lock_path = paths.data_root().join(".lifecycle.lock");
        let visible = entry_metadata_at(&self.root, OsStr::new(".lifecycle.lock"))
            .map_err(|source| io_error("重新检查生命周期锁", &lock_path, source))?
            .ok_or_else(|| LifecycleError::UnsafeCentralStore(lock_path.display().to_string()))?;
        let held = self
            .file
            .metadata()
            .map_err(|source| io_error("检查已持有生命周期锁", &lock_path, source))?;
        if !held.is_file()
            || held.nlink() != 1
            || visible.st_mode & libc::S_IFMT != libc::S_IFREG
            || visible.st_nlink != 1
            || visible.st_dev as u64 != held.dev()
            || visible.st_ino != held.ino()
        {
            return Err(LifecycleError::UnsafeCentralStore(
                lock_path.display().to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn root(&self) -> &File {
        &self.root
    }
}

impl Drop for LifecycleLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

enum CurrentState {
    Missing,
    Previous,
    Expected,
    Other(String),
}

struct InstallPlanCandidateDraft {
    candidate_id: String,
    source_relative_path: Option<String>,
    skill_name: Option<String>,
    skill_description: Option<String>,
    content_fingerprint: Option<String>,
    previous_content_fingerprint: Option<String>,
    selectable: bool,
    preserve_existing: bool,
    validation_errors: Vec<String>,
    warnings: Vec<String>,
    default_selected: bool,
}

struct ManagedContentGuard {
    directories: Vec<(File, PathBuf)>,
}

impl ManagedContentGuard {
    fn recheck(&self, paths: &ApplicationPaths) -> Result<(), LifecycleError> {
        for (handle, path) in &self.directories {
            ensure_open_directory_matches_managed_path(paths, handle, path)?;
        }
        Ok(())
    }
}

pub fn ensure_central_store_layout(paths: &ApplicationPaths) -> Result<(), LifecycleError> {
    ensure_real_directory(paths.data_root())?;
    let root = open_managed_directory(paths, paths.data_root())?;
    for (name, directory) in [
        (OsStr::new("bundles"), paths.bundles_root()),
        (OsStr::new("staging"), paths.staging_root()),
        (OsStr::new("journals"), paths.journals_root()),
        (OsStr::new("trash"), paths.trash_root()),
    ] {
        let child = match open_directory_at(&root, name) {
            Ok(child) => child,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                mkdir_at(&root, name, 0o700)
                    .map_err(|source| io_error("创建 Central Store 目录", &directory, source))?;
                root.sync_all()
                    .map_err(|source| io_error("同步 Central Store", paths.data_root(), source))?;
                open_expected_directory_at(&root, name, &directory)?
            }
            Err(source) => return Err(io_error("检查 Central Store 目录", &directory, source)),
        };
        ensure_open_directory_matches_managed_path(paths, &child, &directory)?;
    }
    ensure_regular_file_at(
        &root,
        OsStr::new(".lifecycle.lock"),
        &paths.data_root().join(".lifecycle.lock"),
    )?;
    let notice = paths.central_store_notice();
    match entry_metadata_at(&root, OsStr::new("SKILLYARD-INFO.md"))
        .map_err(|source| io_error("检查 Central Store 说明", &notice, source))?
    {
        None => write_atomic_at(
            &root,
            OsStr::new("SKILLYARD-INFO.md"),
            &notice,
            render_notice(paths, &[], &[]).as_bytes(),
        )?,
        Some(metadata)
            if metadata.st_mode & libc::S_IFMT == libc::S_IFREG && metadata.st_nlink == 1 => {}
        Some(_) => {
            return Err(LifecycleError::UnsafeCentralStore(
                notice.display().to_string(),
            ));
        }
    }
    root.sync_all()
        .map_err(|source| io_error("同步 Central Store", paths.data_root(), source))?;
    Ok(())
}

pub fn create_folder_install_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    input: &Path,
    now: i64,
) -> Result<InstallPlan, LifecycleError> {
    let validated = validate_skill_bundle_folder(input)?;
    let input_metadata = fs::symlink_metadata(&validated.canonical_root)
        .map_err(|source| io_error("读取输入目录", &validated.canonical_root, source))?;
    use std::os::unix::fs::MetadataExt;

    let input_path = path_to_string(&validated.canonical_root)?;
    let bundle_display_name = validated
        .canonical_root
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| LifecycleError::NonUnicodePath(input_path.clone()))?
        .to_owned();
    let plan_id = Uuid::new_v4().to_string();
    let bundle_id = Uuid::new_v4().to_string();
    ensure_absent(&paths.bundle_directory(&bundle_id))?;
    let expires_at = now.saturating_add(PLAN_TTL_MILLIS);

    let mut candidates = Vec::with_capacity(validated.candidates.len());
    for candidate in &validated.candidates {
        let source_relative_path = path_to_string(&candidate.relative_path)?;
        let target_directory = candidate.name.as_ref().map(|name| {
            path_to_string(
                &paths
                    .bundle_directory(&bundle_id)
                    .join("current/members")
                    .join(name),
            )
        });
        candidates.push(InstallCandidate {
            candidate_id: Uuid::new_v4().to_string(),
            source_relative_path,
            skill_name: candidate.name.clone(),
            description: candidate.description.clone(),
            target_directory: target_directory.transpose()?,
            selectable: candidate.selectable(),
            validation_errors: candidate.validation_errors.clone(),
            warnings: candidate.warnings.clone(),
            default_selected: candidate.selectable(),
        });
    }
    let mut warnings = candidates
        .iter()
        .flat_map(|candidate| candidate.warnings.iter().cloned())
        .collect::<Vec<_>>();
    warnings.sort();
    warnings.dedup();
    let new_candidates = candidates
        .iter()
        .zip(&validated.candidates)
        .map(|(candidate, discovered)| NewInstallCandidate {
            candidate_id: &candidate.candidate_id,
            source_relative_path: Some(candidate.source_relative_path.as_str()),
            skill_name: candidate.skill_name.as_deref(),
            skill_description: candidate.description.as_deref(),
            content_fingerprint: discovered.fingerprint.as_deref(),
            previous_content_fingerprint: None,
            selectable: candidate.selectable,
            preserve_existing: false,
            validation_errors: &candidate.validation_errors,
            warnings: &candidate.warnings,
            default_selected: candidate.default_selected,
        })
        .collect::<Vec<_>>();
    storage.save_install_plan(NewInstallPlan {
        id: &plan_id,
        kind: "folder_snapshot",
        install_mode: "create",
        input_path: Some(&input_path),
        input_device: input_metadata.dev(),
        input_inode: input_metadata.ino(),
        input_fingerprint: &validated.fingerprint,
        snapshot_relative_path: None,
        source_id: None,
        source_tracked_ref: None,
        source_catalog_generation: None,
        source_marker: None,
        expected_source_marker: None,
        expected_current_target: None,
        expected_adopted_marker: None,
        bundle_id: &bundle_id,
        bundle_display_name: &bundle_display_name,
        warnings: &warnings,
        candidates: &new_candidates,
        created_at: now,
        expires_at,
    })?;

    Ok(InstallPlan {
        id: plan_id,
        input_kind: InstallInputKind::LocalFolder,
        mode: InstallMode::Create,
        input_path,
        bundle_display_name,
        candidates,
        warnings,
        will_mount: false,
        update_impact: None,
        created_at: now,
        expires_at,
    })
}

pub fn create_github_install_plan(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transport: &dyn SourceTransport,
    source_id: &str,
    now: i64,
) -> Result<InstallPlan, LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    let source = storage.read_source_install_source(source_id)?;
    let owner = source
        .owner
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or(StorageError::SourceCatalogStateChanged)?;
    let repository = source
        .repository
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or(StorageError::SourceCatalogStateChanged)?;
    let tracked_ref = source
        .tracked_ref
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or(StorageError::SourceCatalogStateChanged)?;
    if source.kind != SourceKind::Github.as_str() {
        return Err(StorageError::SourceCatalogStateChanged.into());
    }
    let installed_paths = preflight_source_install(paths, lifecycle_lock, storage, &source)?;
    let plan_id = Uuid::new_v4().to_string();
    let canonical_identity = format!(
        "github:{}/{}",
        owner.to_ascii_lowercase(),
        repository.to_ascii_lowercase()
    );
    if source.canonical_identity != canonical_identity {
        return Err(StorageError::SourceCatalogStateChanged.into());
    }
    let snapshot = prepare_github_install_snapshot(
        transport,
        GithubCatalogTarget {
            owner,
            repository,
            canonical_identity: &canonical_identity,
            display_name: &source.display_name,
            tracked_ref,
        },
        &source.catalog_marker,
        &paths.staging_root(),
        &plan_id,
    )?;
    let validated = validate_skill_bundle_folder(snapshot.repository_root())?;
    let input_path = format!("https://github.com/{owner}/{repository}");
    let plan = create_source_install_plan_from_prepared_snapshot(
        paths,
        lifecycle_lock,
        storage,
        source,
        &plan_id,
        validated,
        snapshot.repository_root(),
        installed_paths,
        InstallInputKind::Github,
        input_path,
        now,
    )?;
    snapshot.persist();
    Ok(plan)
}

/// Bundle Update 按既有 Source kind 取得内容，但始终进入同一个 Update Plan。
pub fn create_bundle_update_plan(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transport: Option<&dyn SourceTransport>,
    bundle_id: &str,
    now: i64,
) -> Result<InstallPlan, LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    let source = storage.read_bundle_update_install_source(bundle_id)?;
    match SourceKind::from_str(&source.kind)
        .ok_or_else(|| StorageError::UnknownSourceKind(source.kind.clone()))?
    {
        SourceKind::Github => create_github_bundle_update_plan(
            paths,
            lifecycle_lock,
            storage,
            transport.ok_or(GithubSourceError::Network)?,
            source,
            now,
        ),
        SourceKind::EditableLocal => {
            create_editable_local_bundle_update_plan(paths, lifecycle_lock, storage, source, now)
        }
        SourceKind::Archive | SourceKind::DirectUrl => {
            Err(StorageError::SourceCatalogStateChanged.into())
        }
    }
}

fn create_github_bundle_update_plan(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transport: &dyn SourceTransport,
    source: StoredSourceInstallSource,
    now: i64,
) -> Result<InstallPlan, LifecycleError> {
    source
        .bundle
        .as_ref()
        .ok_or(StorageError::SourceBundleStateConflict)?;
    let owner = source
        .owner
        .clone()
        .filter(|value| !value.is_empty())
        .ok_or(StorageError::SourceCatalogStateChanged)?;
    let repository = source
        .repository
        .clone()
        .filter(|value| !value.is_empty())
        .ok_or(StorageError::SourceCatalogStateChanged)?;
    let tracked_ref = source
        .tracked_ref
        .clone()
        .filter(|value| !value.is_empty())
        .ok_or(StorageError::SourceCatalogStateChanged)?;
    if source.kind != SourceKind::Github.as_str() {
        return Err(StorageError::SourceCatalogStateChanged.into());
    }

    let resolved = resolve_github_source(transport, &source.locator, Some(&tracked_ref))?;
    if resolved.canonical_identity != source.canonical_identity
        || resolved.owner != owner
        || resolved.repository != repository
        || resolved.tracked_ref != tracked_ref
    {
        return Err(StorageError::SourceCatalogStateChanged.into());
    }

    let plan_id = Uuid::new_v4().to_string();
    let snapshot = prepare_github_install_snapshot(
        transport,
        GithubCatalogTarget {
            owner: &owner,
            repository: &repository,
            canonical_identity: &source.canonical_identity,
            display_name: &source.display_name,
            tracked_ref: &tracked_ref,
        },
        &resolved.commit,
        &paths.staging_root(),
        &plan_id,
    )?;
    let validated = validate_skill_bundle_folder(snapshot.repository_root())?;
    let repository_url = resolved.repository_url.to_string();
    let plan = create_bundle_update_plan_from_prepared_snapshot(
        paths,
        lifecycle_lock,
        storage,
        source,
        &plan_id,
        validated,
        snapshot.repository_root(),
        &resolved.commit,
        InstallInputKind::Github,
        repository_url.clone(),
        Some(repository_url),
        now,
    )?;
    snapshot.persist();
    Ok(plan)
}

/// 手动替换只把用户选择的归档当作本次快照，不重写原 Source 身份或 locator。
pub fn create_bundle_replacement_plan(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    bundle_id: &str,
    input: &Path,
    now: i64,
) -> Result<InstallPlan, LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    let source = storage.read_bundle_update_install_source(bundle_id)?;
    if !matches!(
        SourceKind::from_str(&source.kind),
        Some(SourceKind::Archive | SourceKind::DirectUrl)
    ) {
        return Err(StorageError::SourceCatalogStateChanged.into());
    }
    let plan_id = Uuid::new_v4().to_string();
    let prepared = prepare_archive_source(input, &paths.staging_root(), &plan_id)?;
    let validated = validate_skill_bundle_folder(prepared.content_root())?;
    let marker = prepared.marker().to_owned();
    let input_path = prepared.locator().to_owned();
    let plan = create_bundle_update_plan_from_prepared_snapshot(
        paths,
        lifecycle_lock,
        storage,
        source,
        &plan_id,
        validated,
        prepared.content_root(),
        &marker,
        InstallInputKind::Archive,
        input_path,
        None,
        now,
    )?;
    prepared.persist();
    Ok(plan)
}

/// Editable Local 只有刚检查出的 Catalog 仍对应同一目录快照时才能进入 Update Plan。
fn create_editable_local_bundle_update_plan(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    source: StoredSourceInstallSource,
    now: i64,
) -> Result<InstallPlan, LifecycleError> {
    let bundle = source
        .bundle
        .as_ref()
        .ok_or(StorageError::SourceBundleStateConflict)?;
    if bundle.update_check_status != crate::domain::BundleUpdateStatus::Available
        || bundle.update_checked_marker.as_deref() != Some(source.catalog_marker.as_str())
        || bundle.update_checked_at.is_none()
    {
        return Err(LifecycleError::PlanPreconditionChanged);
    }
    let identity = editable_local_identity(&source)?;
    let plan_id = Uuid::new_v4().to_string();
    let prepared = match prepare_registered_editable_local_source(
        Path::new(&source.locator),
        identity,
        &paths.staging_root(),
        &plan_id,
    ) {
        Ok(prepared) => prepared,
        Err(SourceInputError::EditableLocalUnavailable) => {
            storage.save_editable_local_check_unavailable(
                &source,
                now,
                &SourceInputError::EditableLocalUnavailable.to_string(),
            )?;
            return Err(SourceInputError::EditableLocalUnavailable.into());
        }
        Err(error) => return Err(error.into()),
    };
    let validated = validate_skill_bundle_folder(prepared.content_root())?;
    if prepared.marker() != source.catalog_marker
        || !source_catalog_matches_snapshot(&source.catalog_members, &validated.candidates)?
    {
        return Err(LifecycleError::PlanPreconditionChanged);
    }
    let marker = prepared.marker().to_owned();
    let input_path = source.locator.clone();
    let plan = create_bundle_update_plan_from_prepared_snapshot(
        paths,
        lifecycle_lock,
        storage,
        source,
        &plan_id,
        validated,
        prepared.content_root(),
        &marker,
        InstallInputKind::EditableLocal,
        input_path,
        None,
        now,
    )?;
    prepared.persist();
    Ok(plan)
}

/// 主动检查会刷新 Source Catalog；采用动作仍由随后创建的唯一 Update Plan 完成。
pub fn check_editable_local_bundle(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    bundle_id: &str,
    now: i64,
) -> Result<(), LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    let source = storage.read_bundle_update_install_source(bundle_id)?;
    let identity = editable_local_identity(&source)?;
    if storage.source_install_object_is_blocked(&source.id, Some(bundle_id))? {
        return Err(StorageError::ManagedObjectBlocked.into());
    }
    let scan_id = Uuid::new_v4().to_string();
    let prepared = match prepare_registered_editable_local_source(
        Path::new(&source.locator),
        identity,
        &paths.staging_root(),
        &scan_id,
    ) {
        Ok(prepared) => prepared,
        Err(SourceInputError::EditableLocalUnavailable) => {
            storage.save_editable_local_check_unavailable(
                &source,
                now,
                &SourceInputError::EditableLocalUnavailable.to_string(),
            )?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    lifecycle_lock.recheck(paths)?;
    let validated = validate_skill_bundle_folder(prepared.content_root())?;
    let member_ids = validated
        .candidates
        .iter()
        .map(|_| Uuid::new_v4().to_string())
        .collect::<Vec<_>>();
    let relative_paths = validated
        .candidates
        .iter()
        .map(|candidate| path_to_string(&candidate.relative_path))
        .collect::<Result<Vec<_>, _>>()?;
    let members = validated
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| NewSourceCatalogMember {
            id: &member_ids[index],
            relative_path: &relative_paths[index],
            skill_name: candidate.name.as_deref(),
            description: candidate.description.as_deref(),
            content_fingerprint: candidate.fingerprint.as_deref(),
            selectable: candidate.selectable(),
            validation_errors: &candidate.validation_errors,
            warnings: &candidate.warnings,
        })
        .collect::<Vec<_>>();
    storage.save_editable_local_check_success(&source, prepared.marker(), now, &members)?;
    lifecycle_lock.recheck(paths)
}

fn editable_local_identity(
    source: &StoredSourceInstallSource,
) -> Result<SourceFilesystemIdentity, LifecycleError> {
    let (Some(device), Some(inode)) = (source.filesystem_device, source.filesystem_inode) else {
        return Err(StorageError::SourceCatalogStateChanged.into());
    };
    if source.kind != SourceKind::EditableLocal.as_str()
        || source.canonical_identity != format!("editable-local:{device}:{inode}")
    {
        return Err(StorageError::SourceCatalogStateChanged.into());
    }
    Ok(SourceFilesystemIdentity { device, inode })
}

/// GitHub、手动替换和 Editable Local 共用成员匹配、保留与影响计算。
#[allow(clippy::too_many_arguments)]
fn create_bundle_update_plan_from_prepared_snapshot(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    source: StoredSourceInstallSource,
    plan_id: &str,
    validated: ValidatedSkillBundle,
    snapshot_input_root: &Path,
    source_marker: &str,
    input_kind: InstallInputKind,
    input_path: String,
    upstream_url: Option<String>,
    now: i64,
) -> Result<InstallPlan, LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    let bundle = source
        .bundle
        .as_ref()
        .ok_or(StorageError::SourceBundleStateConflict)?;
    if storage.source_install_object_is_blocked(&source.id, Some(&bundle.id))? {
        return Err(StorageError::ManagedObjectBlocked.into());
    }
    let superseded = storage.read_pending_source_install_plans_for_source(&source.id)?;
    discard_pending_install_plans(paths, lifecycle_lock, storage, &superseded)?;
    let input_metadata = fs::symlink_metadata(&validated.canonical_root)
        .map_err(|error| io_error("读取 Bundle Update 快照", &validated.canonical_root, error))?;
    let snapshot_relative_path = path_to_string(
        snapshot_input_root
            .strip_prefix(paths.data_root())
            .map_err(|_| {
                LifecycleError::UnsafeCentralStore(snapshot_input_root.display().to_string())
            })?,
    )?;

    let mut existing_by_source_path = BTreeMap::new();
    for member in &bundle.members {
        if let Some(path) = member.source_relative_path.as_deref() {
            existing_by_source_path.insert(path, member);
        }
    }
    let current_paths = validated
        .candidates
        .iter()
        .map(|candidate| path_to_string(&candidate.relative_path))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut matched_member_ids = BTreeSet::new();
    let mut drafts = Vec::with_capacity(validated.candidates.len() + bundle.members.len());
    for candidate in &validated.candidates {
        let relative_path = path_to_string(&candidate.relative_path)?;
        let matching = existing_by_source_path
            .get(relative_path.as_str())
            .copied()
            .filter(|member| candidate.name.as_deref() == Some(member.skill_name.as_str()));
        if let Some(member) = matching {
            matched_member_ids.insert(member.id.clone());
        }
        drafts.push(InstallPlanCandidateDraft {
            candidate_id: matching
                .map(|member| member.id.clone())
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            source_relative_path: Some(relative_path),
            skill_name: candidate.name.clone(),
            skill_description: candidate.description.clone(),
            content_fingerprint: candidate.fingerprint.clone(),
            previous_content_fingerprint: matching.map(|member| member.content_fingerprint.clone()),
            selectable: candidate.selectable(),
            preserve_existing: false,
            validation_errors: candidate.validation_errors.clone(),
            warnings: candidate.warnings.clone(),
            default_selected: candidate.selectable(),
        });
    }
    for member in &bundle.members {
        if matched_member_ids.contains(&member.id) {
            continue;
        }
        let source_relative_path = member.source_relative_path.clone().filter(|path| {
            // 同一路径出现不同名称时不猜重命名；旧成员转为不对应，新成员使用该路径。
            !current_paths.contains(path)
        });
        drafts.push(InstallPlanCandidateDraft {
            candidate_id: member.id.clone(),
            source_relative_path,
            skill_name: Some(member.skill_name.clone()),
            skill_description: Some(member.description.clone()),
            content_fingerprint: Some(member.content_fingerprint.clone()),
            previous_content_fingerprint: Some(member.content_fingerprint.clone()),
            selectable: false,
            preserve_existing: true,
            validation_errors: Vec::new(),
            warnings: Vec::new(),
            default_selected: true,
        });
    }

    let mut name_counts = BTreeMap::new();
    for candidate in &drafts {
        if let Some(name) = candidate.skill_name.as_deref() {
            *name_counts.entry(name.to_owned()).or_insert(0usize) += 1;
        }
    }
    for candidate in &mut drafts {
        if !candidate.preserve_existing
            && candidate
                .skill_name
                .as_ref()
                .is_some_and(|name| name_counts.get(name).copied().unwrap_or_default() > 1)
        {
            candidate.selectable = false;
            candidate.default_selected = false;
            candidate
                .validation_errors
                .push("更新后的 Bundle 包含重复 Skill Name".to_owned());
        }
    }
    if let Some(candidate) = drafts
        .iter()
        .find(|candidate| !candidate.preserve_existing && !candidate.selectable)
    {
        let detail = candidate
            .validation_errors
            .first()
            .cloned()
            .unwrap_or_else(|| "Skill metadata 或内容无效".to_owned());
        return Err(LifecycleError::SourceInput(format!(
            "Bundle 更新内容无法生成计划：{detail}"
        )));
    }

    let mut warnings = drafts
        .iter()
        .flat_map(|candidate| candidate.warnings.iter().cloned())
        .collect::<Vec<_>>();
    warnings.sort();
    warnings.dedup();
    let member_ids = bundle
        .members
        .iter()
        .map(|member| member.id.as_str())
        .collect::<BTreeSet<_>>();
    let existing_mounts = storage
        .read_mount_summaries()?
        .into_iter()
        .filter(|mount| member_ids.contains(mount.member_id.as_str()))
        .collect::<Vec<_>>();
    let new_candidates = drafts
        .iter()
        .map(|candidate| NewInstallCandidate {
            candidate_id: &candidate.candidate_id,
            source_relative_path: candidate.source_relative_path.as_deref(),
            skill_name: candidate.skill_name.as_deref(),
            skill_description: candidate.skill_description.as_deref(),
            content_fingerprint: candidate.content_fingerprint.as_deref(),
            previous_content_fingerprint: candidate.previous_content_fingerprint.as_deref(),
            selectable: candidate.selectable,
            preserve_existing: candidate.preserve_existing,
            validation_errors: &candidate.validation_errors,
            warnings: &candidate.warnings,
            default_selected: candidate.default_selected,
        })
        .collect::<Vec<_>>();
    storage.save_install_plan(NewInstallPlan {
        id: plan_id,
        kind: "source_snapshot",
        install_mode: "update",
        input_path: None,
        input_device: input_metadata.dev(),
        input_inode: input_metadata.ino(),
        input_fingerprint: &validated.fingerprint,
        snapshot_relative_path: Some(&snapshot_relative_path),
        source_id: Some(&source.id),
        source_tracked_ref: source.tracked_ref.as_deref(),
        source_catalog_generation: Some(source.catalog_generation),
        source_marker: Some(source_marker),
        expected_source_marker: Some(&source.catalog_marker),
        expected_current_target: Some(&bundle.current_target),
        expected_adopted_marker: bundle.adopted_marker.as_deref(),
        bundle_id: &bundle.id,
        bundle_display_name: &bundle.display_name,
        warnings: &warnings,
        candidates: &new_candidates,
        created_at: now,
        expires_at: now.saturating_add(PLAN_TTL_MILLIS),
    })?;

    let new_candidate_ids = drafts
        .iter()
        .filter(|candidate| {
            !candidate.preserve_existing && candidate.previous_content_fingerprint.is_none()
        })
        .map(|candidate| candidate.candidate_id.clone())
        .collect::<Vec<_>>();
    let candidates = drafts
        .into_iter()
        .filter(|candidate| !candidate.preserve_existing)
        .map(|candidate| InstallCandidate {
            target_directory: candidate.skill_name.as_ref().map(|name| {
                paths
                    .bundle_directory(&bundle.id)
                    .join("current/members")
                    .join(name)
                    .display()
                    .to_string()
            }),
            candidate_id: candidate.candidate_id,
            source_relative_path: candidate
                .source_relative_path
                .expect("当前 Source 候选必须包含来源路径"),
            skill_name: candidate.skill_name,
            description: candidate.skill_description,
            selectable: candidate.selectable,
            validation_errors: candidate.validation_errors,
            warnings: candidate.warnings,
            default_selected: candidate.default_selected,
        })
        .collect();
    Ok(InstallPlan {
        id: plan_id.to_owned(),
        input_kind,
        mode: InstallMode::Update,
        input_path,
        bundle_display_name: bundle.display_name.clone(),
        candidates,
        warnings,
        will_mount: false,
        update_impact: Some(BundleUpdateImpact {
            new_candidate_ids,
            existing_mounts,
            upstream_url,
        }),
        created_at: now,
        expires_at: now.saturating_add(PLAN_TTL_MILLIS),
    })
}

/// 本地归档只负责生成受控快照；Source、Plan 与确认都复用唯一生命周期。
pub fn create_archive_install_plan(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    input: &Path,
    now: i64,
) -> Result<InstallPlan, LifecycleError> {
    let plan_id = Uuid::new_v4().to_string();
    let prepared = prepare_archive_source(input, &paths.staging_root(), &plan_id)?;
    create_manual_source_install_plan(
        paths,
        lifecycle_lock,
        storage,
        prepared,
        plan_id,
        InstallInputKind::Archive,
        now,
    )
}

/// 直接 URL 与本地归档的差异只在快照准备阶段。
pub fn create_url_install_plan(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transport: &dyn SourceTransport,
    url: &str,
    now: i64,
) -> Result<InstallPlan, LifecycleError> {
    let plan_id = Uuid::new_v4().to_string();
    let prepared = prepare_direct_url_source(transport, url, &paths.staging_root(), &plan_id)?;
    create_manual_source_install_plan(
        paths,
        lifecycle_lock,
        storage,
        prepared,
        plan_id,
        InstallInputKind::DirectUrl,
        now,
    )
}

/// Editable Local 保留原目录，只把本次确认使用的完整内容复制到 Plan 快照。
pub fn create_editable_local_install_plan(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    input: &Path,
    now: i64,
) -> Result<InstallPlan, LifecycleError> {
    let plan_id = Uuid::new_v4().to_string();
    let prepared = prepare_editable_local_source(input, &paths.staging_root(), &plan_id)?;
    create_manual_source_install_plan(
        paths,
        lifecycle_lock,
        storage,
        prepared,
        plan_id,
        InstallInputKind::EditableLocal,
        now,
    )
}

fn create_manual_source_install_plan(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    prepared: PreparedSourceSnapshot,
    plan_id: String,
    input_kind: InstallInputKind,
    now: i64,
) -> Result<InstallPlan, LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    let validated = validate_skill_bundle_folder(prepared.content_root())?;
    let member_ids = validated
        .candidates
        .iter()
        .map(|_| Uuid::new_v4().to_string())
        .collect::<Vec<_>>();
    let relative_paths = validated
        .candidates
        .iter()
        .map(|candidate| path_to_string(&candidate.relative_path))
        .collect::<Result<Vec<_>, _>>()?;
    let members = validated
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| NewSourceCatalogMember {
            id: &member_ids[index],
            relative_path: &relative_paths[index],
            skill_name: candidate.name.as_deref(),
            description: candidate.description.as_deref(),
            content_fingerprint: candidate.fingerprint.as_deref(),
            selectable: candidate.selectable(),
            validation_errors: &candidate.validation_errors,
            warnings: &candidate.warnings,
        })
        .collect::<Vec<_>>();
    let filesystem_identity = prepared.filesystem_identity();
    let source_kind = match prepared.kind() {
        PreparedSourceKind::Archive => SourceKind::Archive,
        PreparedSourceKind::DirectUrl => SourceKind::DirectUrl,
        PreparedSourceKind::EditableLocal => SourceKind::EditableLocal,
    };
    let saved = storage.save_manual_source_with_catalog(NewManualSource {
        id: &Uuid::new_v4().to_string(),
        kind: source_kind.as_str(),
        canonical_identity: prepared.canonical_identity(),
        display_name: prepared.display_name(),
        locator: prepared.locator(),
        filesystem_device: filesystem_identity.map(|identity| identity.device),
        filesystem_inode: filesystem_identity.map(|identity| identity.inode),
        catalog_marker: prepared.marker(),
        members: &members,
        saved_at: now,
    })?;
    lifecycle_lock.recheck(paths)?;
    // Source 在确认页出现前已经是持久管理关系；即使用户放弃安装，也要立即同步说明文件。
    write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
    lifecycle_lock.recheck(paths)?;
    let source = storage.read_source_install_source(&saved.source_id)?;
    if source.catalog_generation != saved.catalog_generation {
        return Err(StorageError::SourceCatalogStateChanged.into());
    }
    let installed_paths = preflight_source_install(paths, lifecycle_lock, storage, &source)?;
    let input_path = prepared.locator().to_owned();
    let snapshot_input_root = prepared.content_root().to_path_buf();
    let plan = create_source_install_plan_from_prepared_snapshot(
        paths,
        lifecycle_lock,
        storage,
        source,
        &plan_id,
        validated,
        &snapshot_input_root,
        installed_paths,
        input_kind,
        input_path,
        now,
    )?;
    prepared.persist();
    Ok(plan)
}

/// 所有 Source 输入最终只在这里生成 source_snapshot Plan。
#[allow(clippy::too_many_arguments)]
fn create_source_install_plan_from_prepared_snapshot(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    source: StoredSourceInstallSource,
    plan_id: &str,
    validated: ValidatedSkillBundle,
    snapshot_input_root: &Path,
    installed_paths: BTreeSet<String>,
    input_kind: InstallInputKind,
    input_path: String,
    now: i64,
) -> Result<InstallPlan, LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    if !source_catalog_matches_snapshot(&source.catalog_members, &validated.candidates)? {
        return Err(LifecycleError::PlanPreconditionChanged);
    }
    let input_metadata = fs::symlink_metadata(&validated.canonical_root)
        .map_err(|source| io_error("读取 Source 安装快照", &validated.canonical_root, source))?;
    let snapshot_relative_path = path_to_string(
        snapshot_input_root
            .strip_prefix(paths.data_root())
            .map_err(|_| {
                LifecycleError::UnsafeCentralStore(snapshot_input_root.display().to_string())
            })?,
    )?;

    let mut drafts = Vec::new();
    if let Some(bundle) = &source.bundle {
        for member in &bundle.members {
            drafts.push(InstallPlanCandidateDraft {
                candidate_id: member.id.clone(),
                source_relative_path: member.source_relative_path.clone(),
                skill_name: Some(member.skill_name.clone()),
                skill_description: Some(member.description.clone()),
                content_fingerprint: Some(member.content_fingerprint.clone()),
                previous_content_fingerprint: Some(member.content_fingerprint.clone()),
                selectable: false,
                preserve_existing: true,
                validation_errors: Vec::new(),
                warnings: Vec::new(),
                default_selected: true,
            });
        }
    }
    for member in &source.catalog_members {
        if installed_paths.contains(member.relative_path.as_str()) {
            continue;
        }
        drafts.push(InstallPlanCandidateDraft {
            // Catalog ID 在确认后成为本地 Member ID，避免额外的映射层。
            candidate_id: member.id.clone(),
            source_relative_path: Some(member.relative_path.clone()),
            skill_name: member.skill_name.clone(),
            skill_description: member.description.clone(),
            content_fingerprint: member.content_fingerprint.clone(),
            previous_content_fingerprint: None,
            selectable: member.selectable,
            preserve_existing: false,
            validation_errors: member.validation_errors.clone(),
            warnings: member.warnings.clone(),
            default_selected: member.selectable,
        });
    }
    let bundle_id = source
        .bundle
        .as_ref()
        .map(|bundle| bundle.id.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let bundle_display_name = source
        .bundle
        .as_ref()
        .map(|bundle| bundle.display_name.clone())
        .unwrap_or_else(|| source.display_name.clone());
    if source.bundle.is_none() {
        ensure_absent(&paths.bundle_directory(&bundle_id))?;
    }
    let mut warnings = drafts
        .iter()
        .flat_map(|candidate| candidate.warnings.iter().cloned())
        .collect::<Vec<_>>();
    warnings.sort();
    warnings.dedup();
    let new_candidates = drafts
        .iter()
        .map(|candidate| NewInstallCandidate {
            candidate_id: &candidate.candidate_id,
            source_relative_path: candidate.source_relative_path.as_deref(),
            skill_name: candidate.skill_name.as_deref(),
            skill_description: candidate.skill_description.as_deref(),
            content_fingerprint: candidate.content_fingerprint.as_deref(),
            previous_content_fingerprint: candidate.previous_content_fingerprint.as_deref(),
            selectable: candidate.selectable,
            preserve_existing: candidate.preserve_existing,
            validation_errors: &candidate.validation_errors,
            warnings: &candidate.warnings,
            default_selected: candidate.default_selected,
        })
        .collect::<Vec<_>>();
    let mode = if source.bundle.is_some() {
        InstallMode::Supplement
    } else {
        InstallMode::Create
    };
    let install_mode = match mode {
        InstallMode::Create => "create",
        InstallMode::Supplement => "supplement",
        InstallMode::Update => unreachable!("普通 Source 安装不会生成 Update mode"),
    };
    storage.save_install_plan(NewInstallPlan {
        id: plan_id,
        kind: "source_snapshot",
        install_mode,
        input_path: None,
        input_device: input_metadata.dev(),
        input_inode: input_metadata.ino(),
        input_fingerprint: &validated.fingerprint,
        snapshot_relative_path: Some(&snapshot_relative_path),
        source_id: Some(&source.id),
        source_tracked_ref: source.tracked_ref.as_deref(),
        source_catalog_generation: Some(source.catalog_generation),
        source_marker: Some(&source.catalog_marker),
        expected_source_marker: None,
        expected_current_target: source
            .bundle
            .as_ref()
            .map(|bundle| bundle.current_target.as_str()),
        expected_adopted_marker: source
            .bundle
            .as_ref()
            .and_then(|bundle| bundle.adopted_marker.as_deref()),
        bundle_id: &bundle_id,
        bundle_display_name: &bundle_display_name,
        warnings: &warnings,
        candidates: &new_candidates,
        created_at: now,
        expires_at: now.saturating_add(PLAN_TTL_MILLIS),
    })?;

    let candidates = drafts
        .into_iter()
        .filter(|candidate| !candidate.preserve_existing)
        .map(|candidate| InstallCandidate {
            target_directory: candidate.skill_name.as_ref().map(|name| {
                paths
                    .bundle_directory(&bundle_id)
                    .join("current/members")
                    .join(name)
                    .display()
                    .to_string()
            }),
            candidate_id: candidate.candidate_id,
            source_relative_path: candidate
                .source_relative_path
                .expect("非保留 Source 候选必须包含来源路径"),
            skill_name: candidate.skill_name,
            description: candidate.skill_description,
            selectable: candidate.selectable,
            validation_errors: candidate.validation_errors,
            warnings: candidate.warnings,
            default_selected: candidate.default_selected,
        })
        .collect();
    Ok(InstallPlan {
        id: plan_id.to_owned(),
        input_kind,
        mode,
        input_path,
        bundle_display_name,
        candidates,
        warnings,
        will_mount: false,
        update_impact: None,
        created_at: now,
        expires_at: now.saturating_add(PLAN_TTL_MILLIS),
    })
}

fn preflight_source_install(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    source: &StoredSourceInstallSource,
) -> Result<BTreeSet<String>, LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    if storage.source_install_object_is_blocked(
        &source.id,
        source.bundle.as_ref().map(|bundle| bundle.id.as_str()),
    )? {
        return Err(StorageError::ManagedObjectBlocked.into());
    }
    let superseded = storage.read_pending_source_install_plans_for_source(&source.id)?;
    discard_pending_install_plans(paths, lifecycle_lock, storage, &superseded)?;
    let installed_paths = source
        .bundle
        .as_ref()
        .map(|bundle| {
            bundle
                .members
                .iter()
                .filter_map(|member| member.source_relative_path.clone())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    if !source
        .catalog_members
        .iter()
        .any(|member| member.selectable && !installed_paths.contains(member.relative_path.as_str()))
    {
        return Err(LifecycleError::NoInstallableSourceMembers);
    }
    Ok(installed_paths)
}

fn source_catalog_matches_snapshot(
    catalog: &[StoredSourceInstallCatalogMember],
    discovered: &[crate::content::DiscoveredBundleCandidate],
) -> Result<bool, LifecycleError> {
    if catalog.len() != discovered.len() {
        return Ok(false);
    }
    for catalog_member in catalog {
        let Some(candidate) = discovered.iter().find(|candidate| {
            candidate.relative_path.to_str() == Some(catalog_member.relative_path.as_str())
        }) else {
            return Ok(false);
        };
        if candidate.name != catalog_member.skill_name
            || candidate.description != catalog_member.description
            || candidate.fingerprint != catalog_member.content_fingerprint
            || candidate.selectable() != catalog_member.selectable
            || candidate.validation_errors != catalog_member.validation_errors
            || candidate.warnings != catalog_member.warnings
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn confirm_install(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
    selected_candidate_ids: &[String],
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), LifecycleError> {
    confirm_install_with_owner(
        paths,
        storage,
        plan_id,
        selected_candidate_ids,
        now,
        failpoint,
        None,
    )
}

pub(crate) fn confirm_bundle_update_batch_child_install(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    owner: BundleUpdateBatchChildOwner<'_>,
    plan_id: &str,
    selected_candidate_ids: &[String],
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), LifecycleError> {
    confirm_install_with_owner(
        paths,
        storage,
        plan_id,
        selected_candidate_ids,
        now,
        failpoint,
        Some(owner),
    )
}

fn confirm_install_with_owner(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
    selected_candidate_ids: &[String],
    now: i64,
    failpoint: LifecycleFailpoint,
    owner: Option<BundleUpdateBatchChildOwner<'_>>,
) -> Result<(), LifecycleError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    let preview = storage.read_install_plan(plan_id)?;
    validate_stored_plan_identifiers(&preview)?;
    if preview.status != "pending" {
        return Err(StorageError::InstallPlanConsumed.into());
    }
    if preview.expires_at <= now {
        discard_pending_install_plan(paths, &lifecycle_lock, storage, &preview, owner)?;
        return Err(StorageError::InstallPlanExpired.into());
    }
    let precondition = verify_plan_snapshot(paths, &preview)
        .and_then(|()| verify_install_target_precondition(paths, lifecycle_lock.root(), &preview));
    if let Err(error) = precondition {
        discard_pending_install_plan(paths, &lifecycle_lock, storage, &preview, owner)?;
        return Err(error);
    }
    preflight_lifecycle_directories(paths)?;

    let transaction_id = Uuid::new_v4().to_string();
    let journal_relative = format!("journals/{transaction_id}.json");
    // 在消费 Plan 前先固定最终选择，并证明生成的 Journal 一定能被恢复器完整读取。
    let expected_plan = prepare_selected_plan(&preview, selected_candidate_ids)?;
    let mut journal = build_journal(&transaction_id, &expected_plan)?;
    ensure_journal_fits(&journal)?;
    let begin = match owner {
        Some(owner) => storage.begin_bundle_update_batch_child_transaction_with_selection(
            plan_id,
            selected_candidate_ids,
            &transaction_id,
            &journal_relative,
            now,
            owner,
        ),
        None => storage.begin_install_transaction_with_selection(
            plan_id,
            selected_candidate_ids,
            &transaction_id,
            &journal_relative,
            now,
        ),
    };
    let plan = match begin {
        Ok(plan) => plan,
        Err(error) if install_plan_error_invalidates_snapshot(&error) => {
            discard_pending_install_plan(paths, &lifecycle_lock, storage, &preview, owner)?;
            return Err(error.into());
        }
        Err(error) => return Err(error.into()),
    };
    let transaction_contract = validate_stored_plan_identifiers(&plan).and_then(|()| {
        if plan == expected_plan {
            Ok(())
        } else {
            Err(LifecycleError::PlanPreconditionChanged)
        }
    });
    if let Err(error) = transaction_contract {
        // SQLite 重读不可信时尚未写入文件系统；立即撤销本次事务及已消费 Plan。
        storage.abort_lifecycle_transaction(&transaction_id, Some(&error.to_string()), now)?;
        storage.forget_terminal_transaction(&transaction_id)?;
        return Err(error);
    }
    lifecycle_lock.recheck(paths)?;
    inject_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterTransactionRecord,
    );
    if failpoint == LifecycleFailpoint::AfterTransactionRecord {
        return Err(LifecycleError::SimulatedInterruption(
            "事务记录已提交，Journal 尚未写入",
        ));
    }

    let execution = execute_install(
        paths,
        &lifecycle_lock,
        storage,
        &plan,
        &mut journal,
        now,
        failpoint,
    );
    if let Err(error) = execution {
        if matches!(error, LifecycleError::SimulatedInterruption(_)) {
            return Err(error);
        }
        handle_execution_error(
            paths,
            &lifecycle_lock,
            storage,
            &journal,
            &plan,
            now,
            &error,
        )?;
        return Err(error);
    }
    cleanup_completed(paths, &lifecycle_lock, storage, &journal, &plan)?;
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

/// 放弃只适用于尚未进入事务的 Plan；确认开始后由同一套恢复协议接管。
pub fn discard_install_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
) -> Result<(), LifecycleError> {
    discard_install_plan_with_owner(paths, storage, plan_id, None)
}

pub(crate) fn discard_bundle_update_batch_child_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    owner: BundleUpdateBatchChildOwner<'_>,
    plan_id: &str,
) -> Result<(), LifecycleError> {
    discard_install_plan_with_owner(paths, storage, plan_id, Some(owner))
}

fn discard_install_plan_with_owner(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
    owner: Option<BundleUpdateBatchChildOwner<'_>>,
) -> Result<(), LifecycleError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    let plan = match storage.read_install_plan(plan_id) {
        Ok(plan) => plan,
        Err(StorageError::InstallPlanNotFound)
            if !storage.install_plan_confirmation_has_started(plan_id)? =>
        {
            // 恢复器可能已经清理过期 Plan；重复放弃必须保持幂等。
            return Ok(());
        }
        Err(StorageError::InstallPlanNotFound) => {
            return Err(StorageError::InstallPlanConsumed.into());
        }
        Err(error) => return Err(error.into()),
    };
    validate_stored_plan_identifiers(&plan)?;
    if plan.status != "pending" {
        return Err(StorageError::InstallPlanConsumed.into());
    }
    discard_pending_install_plan(paths, &lifecycle_lock, storage, &plan, owner)
}

fn install_plan_error_invalidates_snapshot(error: &StorageError) -> bool {
    matches!(
        error,
        StorageError::InstallPlanExpired
            | StorageError::SourceCatalogStateChanged
            | StorageError::SourceBundleStateConflict
    )
}

fn discard_pending_install_plan(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    plan: &StoredInstallPlan,
    owner: Option<BundleUpdateBatchChildOwner<'_>>,
) -> Result<(), LifecycleError> {
    storage.ensure_install_plan_discard_owner(&plan.id, owner)?;
    cleanup_plan_snapshot(paths, lifecycle_lock.root(), plan)?;
    match owner {
        Some(owner) => {
            storage.discard_pending_bundle_update_batch_child_plan(&plan.id, owner)?;
        }
        None => storage.discard_pending_install_plan(&plan.id)?,
    }
    lifecycle_lock.recheck(paths)
}

pub(crate) fn discard_pending_install_plans(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    plans: &[StoredInstallPlan],
) -> Result<(), LifecycleError> {
    for plan in plans {
        validate_stored_plan_identifiers(plan)?;
        discard_pending_install_plan(paths, lifecycle_lock, storage, plan, None)?;
    }
    Ok(())
}

pub fn recover_pending_transactions(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    now: i64,
) -> Result<(), LifecycleError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    for transaction in storage.recoverable_lifecycle_transactions()? {
        // 被阻塞的对象保留证据等待人工处理，但不能阻止其他事务自动恢复或清单只读。
        if transaction.status == "blocked" {
            continue;
        }
        if let Err(error) = recover_transaction(paths, &lifecycle_lock, storage, &transaction, now)
        {
            storage.block_lifecycle_transaction(&transaction.id, &error.to_string(), now)?;
        }
        lifecycle_lock.recheck(paths)?;
    }
    let expired_plans = storage.read_expired_pending_install_plans(now)?;
    discard_pending_install_plans(paths, &lifecycle_lock, storage, &expired_plans)?;
    // 说明文件是 Central Store 的持久契约；每次启动都从 SQLite 幂等重建，避免遗失或陈旧。
    write_notice_from_storage(paths, lifecycle_lock.root(), storage)
}

fn recover_transaction(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transaction: &StoredLifecycleTransaction,
    now: i64,
) -> Result<(), LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    for (label, value) in [
        ("transaction id", transaction.id.as_str()),
        ("plan id", transaction.plan_id.as_str()),
        ("bundle id", transaction.bundle_id.as_str()),
        ("member id", transaction.member_id.as_str()),
    ] {
        ensure_single_path_component(label, value)?;
    }
    let expected_journal_relative = format!("journals/{}.json", transaction.id);
    if transaction.journal_path != expected_journal_relative {
        return Err(LifecycleError::RecoveryBlocked(
            "SQLite 中的 Journal 路径不符合事务边界".to_owned(),
        ));
    }
    let plan = storage.read_install_plan(&transaction.plan_id)?;
    validate_stored_plan_identifiers(&plan)?;
    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    let journal_name = OsString::from(format!("{}.json", transaction.id));
    let journal_path = paths.journals_root().join(&journal_name);
    if entry_metadata_at(&journals, &journal_name)
        .map_err(|source| io_error("检查 Journal", &journal_path, source))?
        .is_none()
    {
        return recover_without_journal(paths, lifecycle_lock, storage, transaction, &plan, now);
    }

    let journal = read_journal_at(&journals, &journal_name, &journal_path)?;
    validate_journal_contract(&journal, transaction, &plan)?;
    if transaction.status == "aborted" {
        let current = inspect_current(paths, lifecycle_lock.root(), &journal)?;
        if !current_is_before_activation(&current, &journal) {
            return Err(LifecycleError::RecoveryBlocked(
                "已终止事务的 current 不在原状态".to_owned(),
            ));
        }
        cleanup_before_activation(paths, lifecycle_lock.root(), &journal, &plan)?;
        remove_journal(paths, lifecycle_lock.root(), &journal)?;
        storage.forget_terminal_transaction(&transaction.id)?;
        return Ok(());
    }

    match inspect_current(paths, lifecycle_lock.root(), &journal)? {
        state
            if current_is_before_activation(&state, &journal)
                && !journal.phase.activation_was_recorded() =>
        {
            cleanup_before_activation(paths, lifecycle_lock.root(), &journal, &plan)?;
            storage.abort_lifecycle_transaction(&transaction.id, None, now)?;
            remove_journal(paths, lifecycle_lock.root(), &journal)?;
            storage.forget_terminal_transaction(&transaction.id)?;
        }
        CurrentState::Missing | CurrentState::Previous => {
            return Err(LifecycleError::RecoveryBlocked(
                "事务记录已经生效，但 current 仍在原状态".to_owned(),
            ));
        }
        CurrentState::Expected => {
            let activated_content =
                validate_activated_content(paths, lifecycle_lock.root(), &journal, &plan)?;
            activated_content.recheck(paths)?;
            storage.finalize_install(
                &transaction.id,
                &plan,
                &journal.bundle_relative,
                &journal.current_target,
                &journal.stable_member_relative,
                now,
            )?;
            write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
            cleanup_completed(paths, lifecycle_lock, storage, &journal, &plan)?;
        }
        CurrentState::Other(actual) => {
            return Err(LifecycleError::RecoveryBlocked(format!(
                "current 指向未知状态：{actual}"
            )));
        }
    }
    Ok(())
}

fn execute_install(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    plan: &StoredInstallPlan,
    journal: &mut InstallJournal,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    write_journal(paths, lifecycle_lock.root(), journal)?;
    storage.update_lifecycle_phase(
        &journal.transaction_id,
        JournalPhase::JournalReady.as_storage_str(),
        now,
    )?;
    lifecycle_lock.recheck(paths)?;

    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    mkdir_at(&staging_root, OsStr::new(&journal.transaction_id), 0o700)
        .map_err(|source| io_error("创建事务临时目录", &paths.staging_root(), source))?;
    let staging = open_directory_at(&staging_root, OsStr::new(&journal.transaction_id))
        .map_err(|source| io_error("打开事务临时目录", &paths.staging_root(), source))?;
    mkdir_at(&staging, OsStr::new("candidate"), 0o700)
        .map_err(|source| io_error("创建候选目录", &paths.staging_root(), source))?;
    let candidate = open_directory_at(&staging, OsStr::new("candidate"))
        .map_err(|source| io_error("打开候选目录", &paths.staging_root(), source))?;
    mkdir_at(&candidate, OsStr::new("members"), 0o700)
        .map_err(|source| io_error("创建候选成员目录", &paths.staging_root(), source))?;
    let members = open_directory_at(&candidate, OsStr::new("members"))
        .map_err(|source| io_error("打开候选成员目录", &paths.staging_root(), source))?;
    let members_path = paths
        .staging_root()
        .join(&journal.transaction_id)
        .join("candidate/members");
    let mut copy_budget = BundleCopyBudget::production();
    let plan_input_root = install_plan_input_root(paths, plan)?;
    for member in &journal.members {
        let plan_member = plan
            .candidates
            .iter()
            .find(|candidate| candidate.selected && candidate.candidate_id == member.candidate_id)
            .ok_or(StorageError::InvalidInstallSelection)?;
        let source = if plan_member.preserve_existing {
            let previous_target = journal
                .previous_current_target
                .as_deref()
                .ok_or(LifecycleError::PlanPreconditionChanged)?;
            paths
                .bundle_directory(&journal.bundle_id)
                .join(previous_target)
                .join("members")
                .join(&member.skill_name)
        } else {
            let source_relative_path = member
                .source_relative_path
                .as_deref()
                .ok_or(LifecycleError::PlanPreconditionChanged)?;
            plan_input_root.join(source_relative_path)
        };
        copy_single_skill_tree_into_open_directory(
            &source,
            &members,
            &members_path,
            OsStr::new(&member.skill_name),
            &member.skill_name,
            &member.content_fingerprint,
            &mut copy_budget,
        )?;
    }
    members
        .sync_all()
        .map_err(|source| io_error("同步候选成员目录", &members_path, source))?;
    candidate
        .sync_all()
        .map_err(|source| io_error("同步候选目录", &members_path, source))?;
    staging
        .sync_all()
        .map_err(|source| io_error("同步事务临时目录", &members_path, source))?;
    staging_root
        .sync_all()
        .map_err(|source| io_error("同步临时区", &paths.staging_root(), source))?;
    // 发布候选前再次核对完整来源和已复制内容，任何竞态变化都不能进入 current。
    verify_plan_snapshot(paths, plan)?;
    verify_install_target_precondition(paths, lifecycle_lock.root(), plan)?;
    validate_prepared_candidate(&candidate, &members, &members_path, journal, plan)?;
    lifecycle_lock.recheck(paths)?;

    let bundles_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?;
    if plan.install_mode == "create" {
        mkdir_at(&bundles_root, OsStr::new(&journal.bundle_id), 0o700)
            .map_err(|source| io_error("创建 Bundle 目录", &paths.bundles_root(), source))?;
    }
    let bundle = open_directory_at(&bundles_root, OsStr::new(&journal.bundle_id))
        .map_err(|source| io_error("打开 Bundle 目录", &paths.bundles_root(), source))?;
    if plan.install_mode == "create" {
        mkdir_at(&bundle, OsStr::new("contents"), 0o700)
            .map_err(|source| io_error("创建内容目录", &paths.bundles_root(), source))?;
    }
    let contents = open_directory_at(&bundle, OsStr::new("contents"))
        .map_err(|source| io_error("打开内容目录", &paths.bundles_root(), source))?;
    rename_at_no_replace(
        &staging,
        OsStr::new("candidate"),
        &contents,
        OsStr::new(&journal.transaction_id),
    )
    .map_err(|source| io_error("发布候选内容", &paths.bundles_root(), source))?;
    staging
        .sync_all()
        .map_err(|source| io_error("同步候选源目录", &paths.staging_root(), source))?;
    staging_root
        .sync_all()
        .map_err(|source| io_error("同步临时区", &paths.staging_root(), source))?;
    contents
        .sync_all()
        .map_err(|source| io_error("同步内容目录", &paths.bundles_root(), source))?;
    bundle
        .sync_all()
        .map_err(|source| io_error("同步 Bundle 目录", &paths.bundles_root(), source))?;
    bundles_root
        .sync_all()
        .map_err(|source| io_error("同步 Bundle 根目录", &paths.bundles_root(), source))?;
    lifecycle_lock.recheck(paths)?;
    inject_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterCandidatePublishedBeforePhase,
    );

    journal.phase = JournalPhase::CandidateReady;
    write_journal(paths, lifecycle_lock.root(), journal)?;
    storage.update_lifecycle_phase(&journal.transaction_id, journal.phase.as_storage_str(), now)?;
    inject_interruption(
        failpoint,
        LifecycleFailpoint::AfterCandidatePrepared,
        LifecycleFailpoint::HardExitAfterCandidatePrepared,
        "候选内容已准备，current 尚未生效",
    )?;

    let temporary_current_name = OsString::from(format!(".current-{}", journal.transaction_id));
    ensure_entry_absent_at(&bundle, &temporary_current_name)
        .map_err(|source| io_error("检查临时 current", &paths.bundles_root(), source))?;
    symlink_at(
        Path::new(&journal.current_target),
        &bundle,
        &temporary_current_name,
    )
    .map_err(|source| io_error("创建临时 current", &paths.bundles_root(), source))?;
    bundle
        .sync_all()
        .map_err(|source| io_error("同步临时 current", &paths.bundles_root(), source))?;
    if failpoint == LifecycleFailpoint::AfterTemporaryCurrentCreated {
        return Err(LifecycleError::SimulatedInterruption(
            "临时 current 已持久化，current 尚未生效",
        ));
    }
    if plan.install_mode == "create" {
        rename_at_no_replace(
            &bundle,
            &temporary_current_name,
            &bundle,
            OsStr::new("current"),
        )
        .map_err(|source| io_error("切换 current", &paths.bundles_root(), source))?;
    } else {
        if !matches!(
            inspect_current(paths, lifecycle_lock.root(), journal)?,
            CurrentState::Previous
        ) {
            return Err(LifecycleError::PlanPreconditionChanged);
        }
        // rename(2) 在同一目录内原子替换旧软链接；Host 只会看到旧或新完整 Bundle。
        rename_at_replace(
            &bundle,
            &temporary_current_name,
            &bundle,
            OsStr::new("current"),
        )
        .map_err(|source| io_error("切换 current", &paths.bundles_root(), source))?;
    }
    bundle
        .sync_all()
        .map_err(|source| io_error("同步 current", &paths.bundles_root(), source))?;
    lifecycle_lock.recheck(paths)?;
    inject_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterCurrentSwitchedBeforePhase,
    );

    journal.phase = JournalPhase::Activated;
    write_journal(paths, lifecycle_lock.root(), journal)?;
    storage.update_lifecycle_phase(&journal.transaction_id, journal.phase.as_storage_str(), now)?;
    inject_interruption(
        failpoint,
        LifecycleFailpoint::AfterCurrentActivated,
        LifecycleFailpoint::HardExitAfterCurrentActivated,
        "current 已生效，领域状态尚未完成",
    )?;

    let activated_content =
        validate_activated_content(paths, lifecycle_lock.root(), journal, plan)?;
    activated_content.recheck(paths)?;
    lifecycle_lock.recheck(paths)?;
    storage.finalize_install(
        &journal.transaction_id,
        plan,
        &journal.bundle_relative,
        &journal.current_target,
        &journal.stable_member_relative,
        now,
    )?;
    lifecycle_lock.recheck(paths)?;
    inject_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterDomainCommittedBeforeJournal,
    );
    journal.phase = JournalPhase::StateCommitted;
    write_journal(paths, lifecycle_lock.root(), journal)?;
    if failpoint == LifecycleFailpoint::AfterDomainCommit {
        return Err(LifecycleError::SimulatedInterruption(
            "领域状态已提交，清理尚未完成",
        ));
    }
    write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

fn inject_interruption(
    actual: LifecycleFailpoint,
    simulated: LifecycleFailpoint,
    hard_exit: LifecycleFailpoint,
    message: &'static str,
) -> Result<(), LifecycleError> {
    if actual == simulated {
        return Err(LifecycleError::SimulatedInterruption(message));
    }
    if actual == hard_exit {
        // 子进程测试必须跳过 Rust 析构，才能验证真正进程退出后的持久化恢复。
        unsafe { libc::_exit(91) }
    }
    Ok(())
}

fn inject_hard_exit(actual: LifecycleFailpoint, expected: LifecycleFailpoint) {
    if actual == expected {
        // 这个 failpoint 精确覆盖文件系统已持久化、事务阶段尚未记录的崩溃窗口。
        unsafe { libc::_exit(91) }
    }
}

fn verify_plan_snapshot(
    paths: &ApplicationPaths,
    plan: &StoredInstallPlan,
) -> Result<(), LifecycleError> {
    let input_root = install_plan_input_root(paths, plan)?;
    let validated = validate_skill_bundle_folder(&input_root)?;
    let metadata = fs::symlink_metadata(&validated.canonical_root)
        .map_err(|source| io_error("读取输入目录", &validated.canonical_root, source))?;
    use std::os::unix::fs::MetadataExt;
    let canonical = path_to_string(&validated.canonical_root)?;
    let expected_input = path_to_string(
        &fs::canonicalize(&input_root)
            .map_err(|source| io_error("解析安装 Plan 输入", &input_root, source))?,
    )?;
    let candidates_match = match plan.kind.as_str() {
        "folder_snapshot" => stored_candidates_match(&plan.candidates, &validated.candidates)?,
        "source_snapshot" => stored_source_plan_candidates_match(
            &plan.candidates,
            &validated.candidates,
            plan.install_mode == "update",
        )?,
        _ => false,
    };
    let unchanged = canonical == expected_input
        && metadata.dev() == plan.input_device
        && metadata.ino() == plan.input_inode
        && validated.fingerprint == plan.input_fingerprint
        && candidates_match;
    if unchanged {
        Ok(())
    } else {
        Err(LifecycleError::PlanPreconditionChanged)
    }
}

fn install_plan_input_root(
    paths: &ApplicationPaths,
    plan: &StoredInstallPlan,
) -> Result<PathBuf, LifecycleError> {
    match plan.kind.as_str() {
        "folder_snapshot" => plan
            .input_path
            .as_deref()
            .map(PathBuf::from)
            .ok_or(LifecycleError::PlanPreconditionChanged),
        "source_snapshot" => plan
            .snapshot_relative_path
            .as_deref()
            .ok_or(LifecycleError::PlanPreconditionChanged)
            .and_then(|relative| resolve_relative(paths.data_root(), relative)),
        _ => Err(LifecycleError::PlanPreconditionChanged),
    }
}

fn verify_install_target_precondition(
    paths: &ApplicationPaths,
    managed_root: &File,
    plan: &StoredInstallPlan,
) -> Result<(), LifecycleError> {
    match plan.install_mode.as_str() {
        "create" => ensure_absent(&paths.bundle_directory(&plan.bundle_id)),
        "supplement" | "update" => validate_previous_bundle_content(paths, managed_root, plan),
        _ => Err(LifecycleError::PlanPreconditionChanged),
    }
}

fn validate_previous_bundle_content(
    paths: &ApplicationPaths,
    managed_root: &File,
    plan: &StoredInstallPlan,
) -> Result<(), LifecycleError> {
    let expected_target = plan
        .expected_current_target
        .as_deref()
        .ok_or(LifecycleError::PlanPreconditionChanged)?;
    let bundles = open_managed_directory_from_root(paths, managed_root, &paths.bundles_root())?;
    let bundle_path = paths.bundle_directory(&plan.bundle_id);
    let bundle = open_expected_directory_at(&bundles, OsStr::new(&plan.bundle_id), &bundle_path)?;
    let current_path = bundle_path.join("current");
    let metadata = entry_metadata_at(&bundle, OsStr::new("current"))
        .map_err(|source| io_error("检查补装 current", &current_path, source))?
        .ok_or(LifecycleError::PlanPreconditionChanged)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFLNK {
        return Err(LifecycleError::PlanPreconditionChanged);
    }
    let actual_target = read_link_at(&bundle, OsStr::new("current"))
        .map_err(|source| io_error("读取补装 current", &current_path, source))?;
    if actual_target != Path::new(expected_target) {
        return Err(LifecycleError::PlanPreconditionChanged);
    }

    let previous = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.previous_content_fingerprint.is_some())
        .collect::<Vec<_>>();
    if previous.is_empty() {
        return Err(LifecycleError::PlanPreconditionChanged);
    }
    let member_names = previous
        .iter()
        .map(|candidate| {
            candidate
                .skill_name
                .clone()
                .ok_or(LifecycleError::PlanPreconditionChanged)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (guard, member_paths) = open_content_guard_for_target(
        paths,
        managed_root,
        &plan.bundle_id,
        expected_target,
        &member_names,
    )?;
    guard.recheck(paths)?;
    if read_entry_names_from_handle(&guard.directories[3].0)? != ["members"] {
        return Err(LifecycleError::PlanPreconditionChanged);
    }
    let mut expected_names = member_names.clone();
    expected_names.sort();
    if read_entry_names_from_handle(&guard.directories[4].0)? != expected_names {
        return Err(LifecycleError::PlanPreconditionChanged);
    }
    for (candidate, member_path) in previous.into_iter().zip(member_paths) {
        let validated = validate_single_skill_folder(&member_path)?;
        guard.recheck(paths)?;
        if candidate.skill_name.as_deref() != Some(validated.name.as_str())
            || candidate.previous_content_fingerprint.as_deref()
                != Some(validated.fingerprint.as_str())
        {
            return Err(LifecycleError::PlanPreconditionChanged);
        }
    }
    Ok(())
}

fn prepare_selected_plan(
    preview: &StoredInstallPlan,
    selected_candidate_ids: &[String],
) -> Result<StoredInstallPlan, LifecycleError> {
    let selected = selected_candidate_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if selected.is_empty() || selected.len() != selected_candidate_ids.len() {
        return Err(StorageError::InvalidInstallSelection.into());
    }
    if preview.install_mode == "update" {
        let required = preview
            .candidates
            .iter()
            .filter(|candidate| !candidate.preserve_existing)
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<BTreeSet<_>>();
        if selected != required
            || preview
                .candidates
                .iter()
                .filter(|candidate| !candidate.preserve_existing)
                .any(|candidate| !candidate.selectable)
        {
            return Err(StorageError::InvalidInstallSelection.into());
        }
    }
    if selected.iter().any(|candidate_id| {
        !preview
            .candidates
            .iter()
            .any(|candidate| candidate.selectable && candidate.candidate_id == *candidate_id)
    }) {
        return Err(StorageError::InvalidInstallSelection.into());
    }
    let mut selected_plan = preview.clone();
    selected_plan.status = "consumed".to_owned();
    for candidate in &mut selected_plan.candidates {
        // 补装的既有成员始终属于最终完整 Bundle，不由本次复选框控制。
        candidate.selected =
            candidate.preserve_existing || selected.contains(candidate.candidate_id.as_str());
    }
    Ok(selected_plan)
}

fn validate_stored_plan_identifiers(plan: &StoredInstallPlan) -> Result<(), LifecycleError> {
    for (label, value) in [
        ("plan id", plan.id.as_str()),
        ("bundle id", plan.bundle_id.as_str()),
    ] {
        ensure_single_path_component(label, value)?;
    }
    for candidate in &plan.candidates {
        ensure_single_path_component("candidate id", &candidate.candidate_id)?;
        if let Some(skill_name) = &candidate.skill_name {
            ensure_single_path_component("candidate skill name", skill_name)?;
        }
        if let Some(source_relative_path) = &candidate.source_relative_path {
            ensure_safe_relative_path("candidate source relative path", source_relative_path)?;
        } else if !candidate.preserve_existing {
            return Err(LifecycleError::RecoveryBlocked(
                "非保留安装候选缺少 Source 路径".to_owned(),
            ));
        }
    }
    match (plan.kind.as_str(), plan.install_mode.as_str()) {
        ("folder_snapshot", "create")
            if plan.input_path.is_some()
                && plan.snapshot_relative_path.is_none()
                && plan.source_id.is_none()
                && plan.expected_current_target.is_none() => {}
        ("source_snapshot", "create")
            if plan.input_path.is_none()
                && plan.snapshot_relative_path.is_some()
                && plan.source_id.is_some()
                && plan.expected_current_target.is_none() => {}
        ("source_snapshot", "supplement")
            if plan.input_path.is_none()
                && plan.snapshot_relative_path.is_some()
                && plan.source_id.is_some()
                && plan.expected_current_target.is_some() => {}
        ("source_snapshot", "update")
            if plan.input_path.is_none()
                && plan.snapshot_relative_path.is_some()
                && plan.source_id.is_some()
                && plan.expected_source_marker.is_some()
                && plan.expected_current_target.is_some() => {}
        _ => {
            return Err(LifecycleError::RecoveryBlocked(
                "SQLite 中的安装 Plan 类型组合无效".to_owned(),
            ));
        }
    }
    if let Some(source_id) = &plan.source_id {
        ensure_single_path_component("source id", source_id)?;
    }
    if let Some(snapshot_relative_path) = &plan.snapshot_relative_path {
        ensure_safe_relative_path("snapshot relative path", snapshot_relative_path)?;
    }
    if let Some(expected_current_target) = &plan.expected_current_target {
        ensure_safe_current_target("expected current target", expected_current_target)?;
    }
    Ok(())
}

fn stored_candidates_match(
    stored: &[StoredInstallCandidate],
    discovered: &[crate::content::DiscoveredBundleCandidate],
) -> Result<bool, LifecycleError> {
    if stored.len() != discovered.len() {
        return Ok(false);
    }
    for (stored, discovered) in stored.iter().zip(discovered) {
        if stored.source_relative_path.as_deref()
            != Some(path_to_string(&discovered.relative_path)?.as_str())
            || stored.skill_name != discovered.name
            || stored.skill_description != discovered.description
            || stored.content_fingerprint != discovered.fingerprint
            || stored.selectable != discovered.selectable()
            || stored.validation_errors != discovered.validation_errors
            || stored.warnings != discovered.warnings
            || stored.default_selected != discovered.selectable()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn stored_source_plan_candidates_match(
    stored: &[StoredInstallCandidate],
    discovered: &[crate::content::DiscoveredBundleCandidate],
    require_complete_source: bool,
) -> Result<bool, LifecycleError> {
    if require_complete_source
        && stored
            .iter()
            .filter(|candidate| !candidate.preserve_existing)
            .count()
            != discovered.len()
    {
        return Ok(false);
    }
    for stored in stored
        .iter()
        .filter(|candidate| !candidate.preserve_existing)
    {
        let Some(source_relative_path) = stored.source_relative_path.as_deref() else {
            return Ok(false);
        };
        let Some(discovered) = discovered
            .iter()
            .find(|candidate| candidate.relative_path.to_str() == Some(source_relative_path))
        else {
            return Ok(false);
        };
        if stored.skill_name != discovered.name
            || stored.skill_description != discovered.description
            || stored.content_fingerprint != discovered.fingerprint
            || stored.selectable != discovered.selectable()
            || stored.validation_errors != discovered.validation_errors
            || stored.warnings != discovered.warnings
            || stored.default_selected != discovered.selectable()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn ensure_safe_relative_path(label: &str, value: &str) -> Result<(), LifecycleError> {
    if value.is_empty()
        || Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        Ok(())
    } else {
        Err(LifecycleError::RecoveryBlocked(format!(
            "SQLite 中的 {label} 不是安全相对路径"
        )))
    }
}

fn ensure_safe_current_target(label: &str, value: &str) -> Result<(), LifecycleError> {
    let mut components = Path::new(value).components();
    let valid = matches!(components.next(), Some(Component::Normal(prefix)) if prefix == OsStr::new("contents"))
        && matches!(components.next(), Some(Component::Normal(content_id)) if !content_id.is_empty())
        && components.next().is_none();
    if valid {
        Ok(())
    } else {
        Err(LifecycleError::RecoveryBlocked(format!(
            "SQLite 中的 {label} 不是安全 current 目标"
        )))
    }
}

fn ensure_single_path_component(label: &str, value: &str) -> Result<(), LifecycleError> {
    let mut components = Path::new(value).components();
    let valid = matches!(components.next(), Some(Component::Normal(component)) if component == OsStr::new(value))
        && components.next().is_none();
    if valid {
        Ok(())
    } else {
        Err(LifecycleError::RecoveryBlocked(format!(
            "SQLite 中的 {label} 不是安全的单路径名称"
        )))
    }
}

fn build_journal(
    transaction_id: &str,
    plan: &StoredInstallPlan,
) -> Result<InstallJournal, LifecycleError> {
    let bundle_relative = format!("bundles/{}", plan.bundle_id);
    let content_id = transaction_id;
    let members = selected_candidates(plan)?
        .into_iter()
        .map(|candidate| {
            let skill_name = candidate.skill_name.clone().ok_or_else(|| {
                LifecycleError::RecoveryBlocked("已选成员缺少 Skill 名称".to_owned())
            })?;
            let content_fingerprint = candidate.content_fingerprint.clone().ok_or_else(|| {
                LifecycleError::RecoveryBlocked("已选成员缺少内容 fingerprint".to_owned())
            })?;
            Ok(InstallJournalMember {
                candidate_id: candidate.candidate_id.clone(),
                source_relative_path: candidate.source_relative_path.clone(),
                stable_member_relative: format!("members/{skill_name}"),
                skill_name,
                content_fingerprint,
            })
        })
        .collect::<Result<Vec<_>, LifecycleError>>()?;
    let anchor = members
        .first()
        .ok_or(StorageError::InvalidInstallSelection)?;
    Ok(InstallJournal {
        version: JOURNAL_VERSION,
        transaction_id: transaction_id.to_owned(),
        plan_id: plan.id.clone(),
        bundle_id: plan.bundle_id.clone(),
        member_id: anchor.candidate_id.clone(),
        skill_name: anchor.skill_name.clone(),
        input_fingerprint: plan.input_fingerprint.clone(),
        phase: JournalPhase::JournalReady,
        staging_relative: format!("staging/{transaction_id}"),
        bundle_relative: bundle_relative.clone(),
        content_relative: format!("{bundle_relative}/contents/{content_id}"),
        current_relative: format!("{bundle_relative}/current"),
        current_target: format!("contents/{content_id}"),
        previous_current_target: plan.expected_current_target.clone(),
        stable_member_relative: anchor.stable_member_relative.clone(),
        members,
    })
}

fn selected_candidates(
    plan: &StoredInstallPlan,
) -> Result<Vec<&StoredInstallCandidate>, LifecycleError> {
    let selected = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.selected)
        .collect::<Vec<_>>();
    if selected.is_empty()
        || selected.iter().any(|candidate| {
            (!candidate.selectable && !candidate.preserve_existing)
                || candidate.skill_name.is_none()
                || candidate.skill_description.is_none()
                || candidate.content_fingerprint.is_none()
        })
    {
        return Err(StorageError::InvalidInstallSelection.into());
    }
    Ok(selected)
}

fn validate_journal_contract(
    actual: &InstallJournal,
    transaction: &StoredLifecycleTransaction,
    plan: &StoredInstallPlan,
) -> Result<(), LifecycleError> {
    let mut expected = build_journal(&transaction.id, plan)?;
    // Journal phase 可以比 SQLite 领先一步；除此之外的路径和身份必须全部由可信状态重建。
    expected.phase = actual.phase;
    if actual == &expected
        && transaction.plan_id == plan.id
        && transaction.bundle_id == plan.bundle_id
        && transaction.member_id == expected.member_id
    {
        Ok(())
    } else {
        Err(LifecycleError::RecoveryBlocked(
            "SQLite、Plan 与 Journal 的事务边界不一致".to_owned(),
        ))
    }
}

fn handle_execution_error(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    journal: &InstallJournal,
    plan: &StoredInstallPlan,
    now: i64,
    error: &LifecycleError,
) -> Result<(), LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    match inspect_current(paths, lifecycle_lock.root(), journal)? {
        state
            if current_is_before_activation(&state, journal)
                && !journal.phase.activation_was_recorded() =>
        {
            if let Err(cleanup_error) =
                cleanup_before_activation(paths, lifecycle_lock.root(), journal, plan)
            {
                return block_recovery(
                    storage,
                    &journal.transaction_id,
                    &cleanup_error.to_string(),
                    now,
                );
            }
            storage.abort_lifecycle_transaction(
                &journal.transaction_id,
                Some(&error.to_string()),
                now,
            )?;
            if let Err(cleanup_error) = remove_journal(paths, lifecycle_lock.root(), journal) {
                return block_recovery(
                    storage,
                    &journal.transaction_id,
                    &cleanup_error.to_string(),
                    now,
                );
            }
            storage.forget_terminal_transaction(&journal.transaction_id)?;
            Ok(())
        }
        CurrentState::Expected => Ok(()),
        CurrentState::Missing | CurrentState::Previous => block_recovery(
            storage,
            &journal.transaction_id,
            "current 在记录生效后仍处于原状态",
            now,
        ),
        CurrentState::Other(actual) => block_recovery(
            storage,
            &journal.transaction_id,
            &format!("current 被外部修改：{actual}"),
            now,
        ),
    }
}

fn recover_without_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transaction: &StoredLifecycleTransaction,
    plan: &StoredInstallPlan,
    now: i64,
) -> Result<(), LifecycleError> {
    let journal = build_journal(&transaction.id, plan)?;
    if transaction.status == "completed" {
        match inspect_current(paths, lifecycle_lock.root(), &journal)? {
            CurrentState::Expected => {
                let activated_content =
                    validate_activated_content(paths, lifecycle_lock.root(), &journal, plan)?;
                activated_content.recheck(paths)?;
                write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
                cleanup_completed(paths, lifecycle_lock, storage, &journal, plan)?;
                return Ok(());
            }
            CurrentState::Missing | CurrentState::Previous => {
                return Err(LifecycleError::RecoveryBlocked(
                    "已完成事务的 current 仍在原状态".to_owned(),
                ));
            }
            CurrentState::Other(actual) => {
                return Err(LifecycleError::RecoveryBlocked(format!(
                    "已完成事务的 current 状态异常：{actual}"
                )));
            }
        }
    }
    if transaction.status == "aborted" {
        let current = inspect_current(paths, lifecycle_lock.root(), &journal)?;
        if !current_is_before_activation(&current, &journal) {
            return Err(LifecycleError::RecoveryBlocked(
                "已终止事务的 current 不在原状态".to_owned(),
            ));
        }
        cleanup_before_activation(paths, lifecycle_lock.root(), &journal, plan)?;
        storage.forget_terminal_transaction(&transaction.id)?;
        return Ok(());
    }
    if transaction.phase != "journal_pending" {
        return Err(LifecycleError::RecoveryBlocked(
            "Journal 缺失但事务已经进入文件系统阶段".to_owned(),
        ));
    }
    let bundle = paths.bundle_directory(&transaction.bundle_id);
    let staging = paths.staging_root().join(&transaction.id);
    let new_content = bundle.join("contents").join(&transaction.id);
    let temporary_current = bundle.join(format!(".current-{}", transaction.id));
    let current = inspect_current(paths, lifecycle_lock.root(), &journal)?;
    let unexpected_bundle = plan.install_mode == "create" && path_entry_exists(&bundle)?;
    if !current_is_before_activation(&current, &journal)
        || unexpected_bundle
        || path_entry_exists(&staging)?
        || path_entry_exists(&new_content)?
        || path_entry_exists(&temporary_current)?
    {
        return Err(LifecycleError::RecoveryBlocked(
            "Journal 写入前出现了未知业务内容".to_owned(),
        ));
    }
    if matches!(plan.install_mode.as_str(), "supplement" | "update") {
        validate_previous_bundle_content(paths, lifecycle_lock.root(), plan)?;
    }
    cleanup_plan_snapshot(paths, lifecycle_lock.root(), plan)?;
    storage.abort_lifecycle_transaction(&transaction.id, None, now)?;
    storage.forget_terminal_transaction(&transaction.id)?;
    Ok(())
}

fn validate_activated_content(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &InstallJournal,
    plan: &StoredInstallPlan,
) -> Result<ManagedContentGuard, LifecycleError> {
    let (guard, member_paths) = open_content_guard(paths, managed_root, journal)?;
    guard.recheck(paths)?;
    validate_content_container_open(
        &guard.directories[3].0,
        &guard.directories[4].0,
        &journal.members,
    )?;
    for (journal_member, member_path) in journal.members.iter().zip(member_paths) {
        let plan_member = plan
            .candidates
            .iter()
            .find(|candidate| {
                candidate.selected && candidate.candidate_id == journal_member.candidate_id
            })
            .ok_or_else(|| {
                LifecycleError::RecoveryBlocked("Journal 成员不属于安装 Plan 的最终选择".to_owned())
            })?;
        let validated = validate_single_skill_folder(&member_path)?;
        guard.recheck(paths)?;
        if validated.fingerprint != journal_member.content_fingerprint
            || plan_member.content_fingerprint.as_deref()
                != Some(journal_member.content_fingerprint.as_str())
            || validated.name != journal_member.skill_name
            || plan_member.skill_name.as_deref() != Some(journal_member.skill_name.as_str())
        {
            return Err(LifecycleError::RecoveryBlocked(
                "current 指向的内容与安装 Plan 不一致".to_owned(),
            ));
        }
    }
    Ok(guard)
}

fn validate_prepared_candidate(
    candidate: &File,
    members: &File,
    members_path: &Path,
    journal: &InstallJournal,
    plan: &StoredInstallPlan,
) -> Result<(), LifecycleError> {
    validate_content_container_open(candidate, members, &journal.members)?;
    for journal_member in &journal.members {
        let plan_member = plan
            .candidates
            .iter()
            .find(|candidate| {
                candidate.selected && candidate.candidate_id == journal_member.candidate_id
            })
            .ok_or(StorageError::InvalidInstallSelection)?;
        let member_path = members_path.join(&journal_member.skill_name);
        let validated = validate_single_skill_folder(&member_path)?;
        if validated.fingerprint != journal_member.content_fingerprint
            || plan_member.content_fingerprint.as_deref()
                != Some(journal_member.content_fingerprint.as_str())
            || validated.name != journal_member.skill_name
            || plan_member.skill_name.as_deref() != Some(journal_member.skill_name.as_str())
        {
            return Err(LifecycleError::PlanPreconditionChanged);
        }
    }
    Ok(())
}

fn open_content_guard(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &InstallJournal,
) -> Result<(ManagedContentGuard, Vec<PathBuf>), LifecycleError> {
    let member_names = journal
        .members
        .iter()
        .map(|member| member.skill_name.clone())
        .collect::<Vec<_>>();
    open_content_guard_for_target(
        paths,
        managed_root,
        &journal.bundle_id,
        &journal.current_target,
        &member_names,
    )
}

fn open_content_guard_for_target(
    paths: &ApplicationPaths,
    managed_root: &File,
    bundle_id: &str,
    current_target: &str,
    member_names: &[String],
) -> Result<(ManagedContentGuard, Vec<PathBuf>), LifecycleError> {
    ensure_single_path_component("bundle id", bundle_id)?;
    ensure_safe_current_target("current target", current_target)?;
    let content_id = Path::new(current_target)
        .components()
        .nth(1)
        .and_then(|component| match component {
            Component::Normal(value) => Some(value.to_owned()),
            _ => None,
        })
        .ok_or_else(|| LifecycleError::RecoveryBlocked("current 目标无效".to_owned()))?;
    let bundles_path = paths.bundles_root();
    let bundle_path = paths.bundle_directory(bundle_id);
    let contents_path = bundle_path.join("contents");
    let content_path = contents_path.join(&content_id);
    let members_path = content_path.join("members");

    let bundles = open_managed_directory_from_root(paths, managed_root, &bundles_path)?;
    let bundle = open_expected_directory_at(&bundles, OsStr::new(bundle_id), &bundle_path)?;
    let contents = open_expected_directory_at(&bundle, OsStr::new("contents"), &contents_path)?;
    let content = open_expected_directory_at(&contents, &content_id, &content_path)?;
    let members = open_expected_directory_at(&content, OsStr::new("members"), &members_path)?;
    let mut directories = vec![
        (bundles, bundles_path),
        (bundle, bundle_path),
        (contents, contents_path),
        (content, content_path),
        (members, members_path),
    ];
    let mut member_paths = Vec::with_capacity(member_names.len());
    for member_name in member_names {
        ensure_single_path_component("skill name", member_name)?;
        let member_path = directories[4].1.join(member_name);
        let member_handle =
            open_expected_directory_at(&directories[4].0, OsStr::new(member_name), &member_path)?;
        member_paths.push(member_path.clone());
        directories.push((member_handle, member_path));
    }
    Ok((ManagedContentGuard { directories }, member_paths))
}

fn validate_content_container_open(
    content: &File,
    members: &File,
    expected_members: &[InstallJournalMember],
) -> Result<(), LifecycleError> {
    if read_entry_names_from_handle(content)? != ["members"] {
        return Err(LifecycleError::RecoveryBlocked(
            "Current Content 包含未知顶层条目".to_owned(),
        ));
    }
    let mut expected_names = expected_members
        .iter()
        .map(|member| member.skill_name.clone())
        .collect::<Vec<_>>();
    expected_names.sort();
    if read_entry_names_from_handle(members)? != expected_names {
        return Err(LifecycleError::RecoveryBlocked(
            "Current Content 的成员边界不一致".to_owned(),
        ));
    }
    Ok(())
}

struct DirectoryStream(*mut libc::DIR);

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0);
        }
    }
}

pub(crate) fn read_entry_names_from_handle(
    directory: &File,
) -> Result<Vec<String>, LifecycleError> {
    read_entry_names_os_from_handle(directory)?
        .into_iter()
        .map(|name| {
            name.into_string().map_err(|_| {
                LifecycleError::RecoveryBlocked("受管内容包含无法识别的文件名".to_owned())
            })
        })
        .collect()
}

fn read_entry_names_os_from_handle(directory: &File) -> Result<Vec<OsString>, LifecycleError> {
    let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
    if duplicate < 0 {
        return Err(io_error(
            "保留受管目录句柄",
            Path::new("<dirfd>"),
            io::Error::last_os_error(),
        ));
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        unsafe {
            libc::close(duplicate);
        }
        return Err(io_error("读取受管目录", Path::new("<dirfd>"), error));
    }
    let stream = DirectoryStream(stream);
    let mut names = Vec::new();
    loop {
        clear_errno();
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            if error.raw_os_error().unwrap_or(0) == 0 {
                break;
            }
            return Err(io_error("读取受管目录", Path::new("<dirfd>"), error));
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        names.push(OsString::from_vec(name.to_vec()));
    }
    names.sort();
    Ok(names)
}

#[cfg(target_os = "macos")]
fn clear_errno() {
    unsafe {
        *libc::__error() = 0;
    }
}

#[cfg(target_os = "linux")]
fn clear_errno() {
    unsafe {
        *libc::__errno_location() = 0;
    }
}

fn inspect_current(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &InstallJournal,
) -> Result<CurrentState, LifecycleError> {
    let bundles = open_managed_directory_from_root(paths, managed_root, &paths.bundles_root())?;
    let bundle_path = paths.bundle_directory(&journal.bundle_id);
    let bundle = match open_directory_at(&bundles, OsStr::new(&journal.bundle_id)) {
        Ok(bundle) => bundle,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(CurrentState::Missing);
        }
        Err(source) => return Err(io_error("安全打开 Bundle", &bundle_path, source)),
    };
    ensure_open_directory_matches_managed_path(paths, &bundle, &bundle_path)?;
    match entry_metadata_at(&bundle, OsStr::new("current"))
        .map_err(|source| io_error("检查 current", &bundle_path.join("current"), source))?
    {
        Some(metadata) if metadata.st_mode & libc::S_IFMT == libc::S_IFLNK => {
            let target = read_link_at(&bundle, OsStr::new("current"))
                .map_err(|source| io_error("读取 current", &bundle_path.join("current"), source))?;
            ensure_open_directory_matches_managed_path(paths, &bundle, &bundle_path)?;
            if target == Path::new(&journal.current_target) {
                Ok(CurrentState::Expected)
            } else if journal
                .previous_current_target
                .as_deref()
                .is_some_and(|previous| target == Path::new(previous))
            {
                Ok(CurrentState::Previous)
            } else {
                Ok(CurrentState::Other(format!(
                    "软链接目标 {}",
                    target.display()
                )))
            }
        }
        Some(metadata) => Ok(CurrentState::Other(
            if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR {
                "普通目录".to_owned()
            } else {
                "普通文件或特殊条目".to_owned()
            },
        )),
        None => Ok(CurrentState::Missing),
    }
}

fn current_is_before_activation(state: &CurrentState, journal: &InstallJournal) -> bool {
    matches!(
        (state, journal.previous_current_target.as_ref()),
        (CurrentState::Missing, None) | (CurrentState::Previous, Some(_))
    )
}

fn cleanup_before_activation(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &InstallJournal,
    plan: &StoredInstallPlan,
) -> Result<(), LifecycleError> {
    cleanup_temporary_current(paths, managed_root, journal)?;
    stage_unactivated_content_for_cleanup(paths, managed_root, journal)?;
    cleanup_staging_before_activation(paths, managed_root, journal)?;
    cleanup_plan_snapshot(paths, managed_root, plan)?;
    // 只有 Create 可能留下由本事务新建的空 Bundle；Supplement 与 Update 都复用既有 Bundle。
    if plan.install_mode != "create" {
        return Ok(());
    }
    let bundles = open_managed_directory_from_root(paths, managed_root, &paths.bundles_root())?;
    let bundle_path = paths.bundle_directory(&journal.bundle_id);
    let bundle = match open_directory_at(&bundles, OsStr::new(&journal.bundle_id)) {
        Ok(bundle) => bundle,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error("安全打开 Bundle", &bundle_path, source)),
    };
    ensure_open_directory_matches_managed_path(paths, &bundle, &bundle_path)?;
    remove_empty_directory_at(
        &bundle,
        OsStr::new("contents"),
        &bundle_path.join("contents"),
    )?;
    drop(bundle);
    remove_empty_directory_at(&bundles, OsStr::new(&journal.bundle_id), &bundle_path)
}

fn stage_unactivated_content_for_cleanup(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &InstallJournal,
) -> Result<(), LifecycleError> {
    let content = resolve_relative(paths.data_root(), &journal.content_relative)?;
    if !path_entry_exists(&content)? {
        return Ok(());
    }

    let (content_guard, member_paths) = open_content_guard(paths, managed_root, journal)?;
    content_guard.recheck(paths)?;
    validate_content_container_open(
        &content_guard.directories[3].0,
        &content_guard.directories[4].0,
        &journal.members,
    )?;
    for (journal_member, member_path) in journal.members.iter().zip(member_paths) {
        let validated = validate_single_skill_folder(&member_path)?;
        content_guard.recheck(paths)?;
        if validated.fingerprint != journal_member.content_fingerprint
            || validated.name != journal_member.skill_name
        {
            return Err(LifecycleError::RecoveryBlocked(
                "生效前候选内容被外部修改".to_owned(),
            ));
        }
    }

    let staging = resolve_relative(paths.data_root(), &journal.staging_relative)?;
    let staging_root =
        open_managed_directory_from_root(paths, managed_root, &paths.staging_root())?;
    let staging_handle = match open_directory_at(&staging_root, OsStr::new(&journal.transaction_id))
    {
        Ok(staging_handle) => staging_handle,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            mkdir_at(&staging_root, OsStr::new(&journal.transaction_id), 0o700)
                .map_err(|source| io_error("创建事务清理目录", &staging, source))?;
            staging_root
                .sync_all()
                .map_err(|source| io_error("同步事务清理目录", &staging, source))?;
            open_expected_directory_at(
                &staging_root,
                OsStr::new(&journal.transaction_id),
                &staging,
            )?
        }
        Err(source) => return Err(io_error("安全打开事务清理目录", &staging, source)),
    };
    ensure_open_directory_matches_managed_path(paths, &staging_handle, &staging)?;
    if !read_entry_names_from_handle(&staging_handle)?.is_empty() {
        return Err(LifecycleError::RecoveryBlocked(
            "发布后的事务临时区包含未知内容".to_owned(),
        ));
    }
    let discard = staging.join("discarding-content");
    ensure_entry_absent_at(&staging_handle, OsStr::new("discarding-content"))
        .map_err(|source| io_error("检查清理隔离目录", &discard, source))?;
    rename_at_no_replace(
        &content_guard.directories[2].0,
        OsStr::new(&journal.transaction_id),
        &staging_handle,
        OsStr::new("discarding-content"),
    )
    .map_err(|source| io_error("隔离待清理候选内容", &discard, source))?;
    content_guard.directories[2]
        .0
        .sync_all()
        .map_err(|source| io_error("同步候选内容目录", &content, source))?;
    staging_handle
        .sync_all()
        .map_err(|source| io_error("同步事务清理目录", &staging, source))?;
    Ok(())
}

fn cleanup_completed(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    journal: &InstallJournal,
    plan: &StoredInstallPlan,
) -> Result<(), LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    cleanup_temporary_current(paths, lifecycle_lock.root(), journal)?;
    cleanup_previous_content(paths, lifecycle_lock.root(), journal, plan)?;
    cleanup_empty_staging(paths, lifecycle_lock.root(), journal)?;
    cleanup_plan_snapshot(paths, lifecycle_lock.root(), plan)?;
    remove_journal(paths, lifecycle_lock.root(), journal)?;
    storage.forget_terminal_transaction(&journal.transaction_id)?;
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

fn cleanup_previous_content(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &InstallJournal,
    plan: &StoredInstallPlan,
) -> Result<(), LifecycleError> {
    let Some(previous_target) = journal.previous_current_target.as_deref() else {
        return Ok(());
    };
    if previous_target == journal.current_target {
        return Err(LifecycleError::RecoveryBlocked(
            "补装的新旧 current 目标相同".to_owned(),
        ));
    }
    let previous = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.previous_content_fingerprint.is_some())
        .collect::<Vec<_>>();
    let member_names = previous
        .iter()
        .map(|candidate| {
            candidate
                .skill_name
                .clone()
                .ok_or_else(|| LifecycleError::RecoveryBlocked("旧成员缺少名称".to_owned()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let old_content_path = paths
        .bundle_directory(&journal.bundle_id)
        .join(previous_target);
    let staging_path = paths.staging_root().join(&journal.transaction_id);
    let staging_root =
        open_managed_directory_from_root(paths, managed_root, &paths.staging_root())?;
    let staging = match open_directory_at(&staging_root, OsStr::new(&journal.transaction_id)) {
        Ok(staging) => staging,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            mkdir_at(&staging_root, OsStr::new(&journal.transaction_id), 0o700)
                .map_err(|source| io_error("重建事务清理目录", &staging_path, source))?;
            staging_root
                .sync_all()
                .map_err(|source| io_error("同步事务清理目录", &staging_path, source))?;
            open_expected_directory_at(
                &staging_root,
                OsStr::new(&journal.transaction_id),
                &staging_path,
            )?
        }
        Err(source) => return Err(io_error("打开事务清理目录", &staging_path, source)),
    };
    let discard_name = OsStr::new("discarding-previous");
    let discard_path = staging_path.join(discard_name);
    let staging_entries = read_entry_names_from_handle(&staging)?;
    if staging_entries == ["discarding-previous"] {
        // 原子隔离后即使递归删除中断，剩余内容也只属于本事务，可以直接重试。
        return remove_owned_tree_at(&staging, discard_name, &discard_path);
    }
    if !staging_entries.is_empty() {
        return Err(LifecycleError::RecoveryBlocked(
            "已完成事务的临时区包含未知内容".to_owned(),
        ));
    }
    if !path_entry_exists(&old_content_path)? {
        // 上一次清理可能已经删除旧内容但尚未来得及移除空事务目录。
        return Ok(());
    }
    let (guard, member_paths) = open_content_guard_for_target(
        paths,
        managed_root,
        &journal.bundle_id,
        previous_target,
        &member_names,
    )?;
    if read_entry_names_from_handle(&guard.directories[3].0)? != ["members"] {
        return Err(LifecycleError::RecoveryBlocked(
            "待清理旧内容包含未知顶层条目".to_owned(),
        ));
    }
    let mut expected_names = member_names;
    expected_names.sort();
    if read_entry_names_from_handle(&guard.directories[4].0)? != expected_names {
        return Err(LifecycleError::RecoveryBlocked(
            "待清理旧内容的成员边界不一致".to_owned(),
        ));
    }
    for (candidate, member_path) in previous.into_iter().zip(member_paths) {
        let validated = validate_single_skill_folder(&member_path)?;
        guard.recheck(paths)?;
        if candidate.skill_name.as_deref() != Some(validated.name.as_str())
            || candidate.previous_content_fingerprint.as_deref()
                != Some(validated.fingerprint.as_str())
        {
            return Err(LifecycleError::RecoveryBlocked(
                "待清理旧内容已被外部修改".to_owned(),
            ));
        }
    }
    let content_id = Path::new(previous_target)
        .file_name()
        .ok_or_else(|| LifecycleError::RecoveryBlocked("旧 current 目标无效".to_owned()))?;
    let contents = &guard.directories[2].0;
    guard.recheck(paths)?;
    rename_at_no_replace(contents, content_id, &staging, discard_name)
        .map_err(|source| io_error("隔离待清理旧内容", &discard_path, source))?;
    contents
        .sync_all()
        .map_err(|source| io_error("同步旧内容目录", &old_content_path, source))?;
    staging
        .sync_all()
        .map_err(|source| io_error("同步事务清理目录", &staging_path, source))?;
    remove_owned_tree_at(&staging, discard_name, &discard_path)
}

fn cleanup_plan_snapshot(
    paths: &ApplicationPaths,
    managed_root: &File,
    plan: &StoredInstallPlan,
) -> Result<(), LifecycleError> {
    let Some(snapshot_relative_path) = plan.snapshot_relative_path.as_deref() else {
        return Ok(());
    };
    let components = Path::new(snapshot_relative_path)
        .components()
        .collect::<Vec<_>>();
    let expected_snapshot_name = OsString::from(format!(".install-plan-{}", plan.id));
    let valid = components.len() == 3
        && matches!(components[0], Component::Normal(value) if value == OsStr::new("staging"))
        && matches!(components[1], Component::Normal(value) if value == expected_snapshot_name)
        && matches!(components[2], Component::Normal(_));
    if !valid {
        return Err(LifecycleError::RecoveryBlocked(
            "安装 Plan 快照路径不属于本 Plan".to_owned(),
        ));
    }
    let staging = open_managed_directory_from_root(paths, managed_root, &paths.staging_root())?;
    let snapshot_root = paths.staging_root().join(&expected_snapshot_name);
    if entry_metadata_at(&staging, &expected_snapshot_name)
        .map_err(|source| io_error("检查安装 Plan 快照", &snapshot_root, source))?
        .is_none()
    {
        return Ok(());
    }
    remove_owned_tree_at(&staging, &expected_snapshot_name, &snapshot_root)
}

fn remove_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &InstallJournal,
) -> Result<(), LifecycleError> {
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let name = OsString::from(format!("{}.json", journal.transaction_id));
    match unlink_at(&journals, &name, false) {
        Ok(()) => journals
            .sync_all()
            .map_err(|source| io_error("同步 Journal 目录", &paths.journals_root(), source)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error(
            "清理事务 Journal",
            &journal_path(paths, journal),
            source,
        )),
    }
}

fn cleanup_staging_before_activation(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &InstallJournal,
) -> Result<(), LifecycleError> {
    let staging = resolve_relative(paths.data_root(), &journal.staging_relative)?;
    let staging_root =
        open_managed_directory_from_root(paths, managed_root, &paths.staging_root())?;
    let staging_handle = match open_directory_at(&staging_root, OsStr::new(&journal.transaction_id))
    {
        Ok(staging_handle) => staging_handle,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error("安全打开事务临时目录", &staging, source)),
    };
    ensure_open_directory_matches_managed_path(paths, &staging_handle, &staging)?;
    let staging_entries = read_entry_names_from_handle(&staging_handle)?;
    if staging_entries.is_empty() {
        drop(staging_handle);
        return remove_empty_directory_at(
            &staging_root,
            OsStr::new(&journal.transaction_id),
            &staging,
        );
    }
    if staging_entries == ["discarding-content"] {
        remove_owned_tree_at(
            &staging_handle,
            OsStr::new("discarding-content"),
            &staging.join("discarding-content"),
        )?;
        drop(staging_handle);
        return remove_empty_directory_at(
            &staging_root,
            OsStr::new(&journal.transaction_id),
            &staging,
        );
    }
    if staging_entries != ["candidate"] {
        return Err(LifecycleError::RecoveryBlocked(
            "事务临时区包含未知条目".to_owned(),
        ));
    }

    let candidate = staging.join("candidate");
    let candidate_handle =
        open_expected_directory_at(&staging_handle, OsStr::new("candidate"), &candidate)?;
    ensure_open_directory_matches_managed_path(paths, &candidate_handle, &candidate)?;
    if read_entry_names_from_handle(&candidate_handle)? != ["members"] {
        return Err(LifecycleError::RecoveryBlocked(
            "候选 Bundle 的目录边界异常".to_owned(),
        ));
    }
    let members = candidate.join("members");
    let members_handle =
        open_expected_directory_at(&candidate_handle, OsStr::new("members"), &members)?;
    ensure_open_directory_matches_managed_path(paths, &members_handle, &members)?;
    let member_entries = read_entry_names_from_handle(&members_handle)?;
    let expected = journal
        .members
        .iter()
        .map(|member| member.skill_name.as_str())
        .collect::<BTreeSet<_>>();
    if member_entries
        .iter()
        .all(|entry| expected.contains(entry.as_str()))
    {
        // JournalReady 阶段可能只复制了部分成员；仅删除 Journal 明确拥有的事务内容。
        for entry in member_entries {
            remove_owned_tree_at(&members_handle, OsStr::new(&entry), &members.join(&entry))?;
        }
    } else {
        return Err(LifecycleError::RecoveryBlocked(
            "候选 Bundle 包含未知成员".to_owned(),
        ));
    }
    drop(members_handle);
    remove_empty_directory_at(&candidate_handle, OsStr::new("members"), &members)?;
    drop(candidate_handle);
    remove_empty_directory_at(&staging_handle, OsStr::new("candidate"), &candidate)?;
    drop(staging_handle);
    remove_empty_directory_at(&staging_root, OsStr::new(&journal.transaction_id), &staging)
}

fn cleanup_empty_staging(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &InstallJournal,
) -> Result<(), LifecycleError> {
    let staging = resolve_relative(paths.data_root(), &journal.staging_relative)?;
    let staging_root =
        open_managed_directory_from_root(paths, managed_root, &paths.staging_root())?;
    match open_directory_at(&staging_root, OsStr::new(&journal.transaction_id)) {
        Ok(staging_handle) => {
            ensure_open_directory_matches_managed_path(paths, &staging_handle, &staging)?;
            if !read_entry_names_from_handle(&staging_handle)?.is_empty() {
                return Err(LifecycleError::RecoveryBlocked(
                    "已生效事务的临时区包含未知内容".to_owned(),
                ));
            }
            drop(staging_handle);
            remove_empty_directory_at(&staging_root, OsStr::new(&journal.transaction_id), &staging)
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error("安全打开事务临时目录", &staging, source)),
    }
}

fn cleanup_temporary_current(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &InstallJournal,
) -> Result<(), LifecycleError> {
    let bundles = open_managed_directory_from_root(paths, managed_root, &paths.bundles_root())?;
    let bundle_path = paths.bundle_directory(&journal.bundle_id);
    let bundle = match open_directory_at(&bundles, OsStr::new(&journal.bundle_id)) {
        Ok(bundle) => bundle,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error("安全打开 Bundle", &bundle_path, source)),
    };
    ensure_open_directory_matches_managed_path(paths, &bundle, &bundle_path)?;
    let temporary_name = OsString::from(format!(".current-{}", journal.transaction_id));
    let temporary = bundle_path.join(&temporary_name);
    match entry_metadata_at(&bundle, &temporary_name)
        .map_err(|source| io_error("检查临时 current", &temporary, source))?
    {
        Some(metadata) if metadata.st_mode & libc::S_IFMT == libc::S_IFLNK => {
            let target = read_link_at(&bundle, &temporary_name)
                .map_err(|source| io_error("读取临时 current", &temporary, source))?;
            if target != Path::new(&journal.current_target) {
                return Err(LifecycleError::RecoveryBlocked(
                    "临时 current 指向未知目标".to_owned(),
                ));
            }
            unlink_at(&bundle, &temporary_name, false)
                .map_err(|source| io_error("清理临时 current", &temporary, source))?;
            bundle
                .sync_all()
                .map_err(|source| io_error("同步 Bundle 目录", &bundle_path, source))
        }
        Some(_) => Err(LifecycleError::RecoveryBlocked(
            "临时 current 不是软链接".to_owned(),
        )),
        None => Ok(()),
    }
}

fn block_recovery(
    storage: &mut Storage,
    transaction_id: &str,
    message: &str,
    now: i64,
) -> Result<(), LifecycleError> {
    storage.block_lifecycle_transaction(transaction_id, message, now)?;
    Err(LifecycleError::RecoveryBlocked(message.to_owned()))
}

pub(crate) fn write_notice_from_storage(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &Storage,
) -> Result<(), LifecycleError> {
    let root = open_managed_directory_from_root(paths, managed_root, paths.data_root())?;
    let bundles = storage.managed_bundle_notice_rows()?;
    let sources = storage.source_notice_rows()?;
    write_atomic_at(
        &root,
        OsStr::new("SKILLYARD-INFO.md"),
        &paths.central_store_notice(),
        render_notice(paths, &bundles, &sources).as_bytes(),
    )
}

fn render_notice(
    paths: &ApplicationPaths,
    bundles: &[(String, String, Vec<String>)],
    sources: &[(String, String)],
) -> String {
    let mut notice = String::from(
        "# SkillYard Central Store\n\n这里保存的是用户 Skill 的实际主副本，不是缓存。请勿把整个目录作为临时数据删除。\n\n## 已安装 Bundle\n",
    );
    if bundles.is_empty() {
        notice.push_str("\n- 暂无\n");
    } else {
        for (display_name, relative, mount_targets) in bundles {
            notice.push_str(&format!(
                "\n- {display_name}: `{}`",
                paths.data_root().join(relative).display()
            ));
            if mount_targets.is_empty() {
                notice.push_str("（未挂载）\n");
            } else {
                notice.push('\n');
                for target in mount_targets {
                    notice.push_str(&format!("  - Mount: `{target}`\n"));
                }
            }
        }
    }
    notice.push_str("\n## Source\n");
    if sources.is_empty() {
        notice.push_str("\n- 当前没有已登记 Source。\n");
    } else {
        for (display_name, repository_url) in sources {
            notice.push_str(&format!("\n- {display_name}: `{repository_url}`\n"));
        }
    }
    notice
}

fn write_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &InstallJournal,
) -> Result<(), LifecycleError> {
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let name = OsString::from(format!("{}.json", journal.transaction_id));
    let bytes = serde_json::to_vec_pretty(journal)?;
    ensure_serialized_journal_fits(bytes.len())?;
    write_atomic_at(&journals, &name, &journal_path(paths, journal), &bytes)
}

fn ensure_journal_fits(journal: &InstallJournal) -> Result<(), LifecycleError> {
    // phase 文本长度不同；事务开始前必须证明后续每个持久化阶段都能被恢复器读取。
    for phase in [
        JournalPhase::JournalReady,
        JournalPhase::CandidateReady,
        JournalPhase::Activated,
        JournalPhase::StateCommitted,
    ] {
        let mut candidate = journal.clone();
        candidate.phase = phase;
        let bytes = serde_json::to_vec_pretty(&candidate)?;
        ensure_serialized_journal_fits(bytes.len())?;
    }
    Ok(())
}

fn ensure_serialized_journal_fits(actual: usize) -> Result<(), LifecycleError> {
    if actual > MAX_JOURNAL_BYTES {
        Err(LifecycleError::JournalTooLarge {
            actual,
            limit: MAX_JOURNAL_BYTES,
        })
    } else {
        Ok(())
    }
}

fn read_journal_at(
    journals: &File,
    name: &OsStr,
    path: &Path,
) -> Result<InstallJournal, LifecycleError> {
    let mut file = open_regular_file_at(journals, name, path, false)?;
    let metadata = file
        .metadata()
        .map_err(|source| io_error("检查 Journal", path, source))?;
    if metadata.len() > MAX_JOURNAL_BYTES as u64 {
        return Err(LifecycleError::RecoveryBlocked(
            "事务 Journal 超过安全大小限制".to_owned(),
        ));
    }
    let mut bytes = Vec::with_capacity((metadata.len() + 1) as usize);
    Read::by_ref(&mut file)
        .take(MAX_JOURNAL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| io_error("读取 Journal", path, source))?;
    if bytes.len() > MAX_JOURNAL_BYTES {
        return Err(LifecycleError::RecoveryBlocked(
            "事务 Journal 超过安全大小限制".to_owned(),
        ));
    }
    serde_json::from_slice(&bytes).map_err(Into::into)
}

fn journal_path(paths: &ApplicationPaths, journal: &InstallJournal) -> PathBuf {
    paths
        .journals_root()
        .join(format!("{}.json", journal.transaction_id))
}

fn ensure_real_directory(path: &Path) -> Result<(), LifecycleError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(LifecycleError::UnsafeCentralStore(
            path.display().to_string(),
        )),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)
                .map_err(|source| io_error("创建 Central Store 目录", path, source))?;
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
            Ok(())
        }
        Err(source) => Err(io_error("检查 Central Store 目录", path, source)),
    }
}

fn preflight_lifecycle_directories(paths: &ApplicationPaths) -> Result<(), LifecycleError> {
    for path in [
        paths.data_root().to_owned(),
        paths.bundles_root(),
        paths.staging_root(),
        paths.journals_root(),
    ] {
        ensure_real_directory(&path)?;
        ensure_managed_directory(paths, &path)?;
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| LifecycleError::UnsafeCentralStore(path.display().to_string()))?;
        // access(2) 会同时考虑 Unix mode 与 ACL；真正写入仍保留原子校验以防检查后变化。
        if unsafe { libc::access(c_path.as_ptr(), libc::W_OK) } != 0 {
            return Err(LifecycleError::PermissionPreflight(
                path.display().to_string(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn acquire_lifecycle_lock(
    paths: &ApplicationPaths,
) -> Result<LifecycleLock, LifecycleError> {
    let root = open_managed_directory(paths, paths.data_root())?;
    let path = paths.data_root().join(".lifecycle.lock");
    let file = open_regular_file_at(&root, OsStr::new(".lifecycle.lock"), &path, true)?;
    ensure_open_directory_matches_managed_path(paths, &root, paths.data_root())?;
    FileExt::try_lock_exclusive(&file).map_err(|source| {
        if source.kind() == io::ErrorKind::WouldBlock {
            LifecycleError::LifecycleBusy
        } else {
            io_error("获取生命周期锁", &path, source)
        }
    })?;
    ensure_open_directory_matches_managed_path(paths, &root, paths.data_root())?;
    Ok(LifecycleLock { file, root })
}

fn ensure_managed_directory(paths: &ApplicationPaths, path: &Path) -> Result<(), LifecycleError> {
    let relative = path.strip_prefix(paths.data_root()).map_err(|_| {
        LifecycleError::UnsafeCentralStore(format!("{} 不属于 Central Store", path.display()))
    })?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        && !relative.as_os_str().is_empty()
    {
        return Err(LifecycleError::UnsafeCentralStore(
            path.display().to_string(),
        ));
    }
    let canonical_root = fs::canonicalize(paths.data_root())
        .map_err(|source| io_error("解析 Central Store", paths.data_root(), source))?;
    let canonical_path =
        fs::canonicalize(path).map_err(|source| io_error("解析受管目录", path, source))?;
    let expected = canonical_root.join(relative);
    let metadata =
        fs::symlink_metadata(path).map_err(|source| io_error("检查受管目录", path, source))?;
    if canonical_path != expected || metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(LifecycleError::UnsafeCentralStore(
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn open_managed_directory(paths: &ApplicationPaths, path: &Path) -> Result<File, LifecycleError> {
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(paths.data_root())
        .map_err(|source| io_error("安全打开 Central Store", paths.data_root(), source))?;
    open_managed_directory_from_root(paths, &root, path)
}

pub(crate) fn open_managed_directory_from_root(
    paths: &ApplicationPaths,
    root: &File,
    path: &Path,
) -> Result<File, LifecycleError> {
    let relative = managed_relative_path(paths, path)?;
    ensure_open_directory_matches_managed_path(paths, root, paths.data_root())?;
    let mut handle = root
        .try_clone()
        .map_err(|source| io_error("保留 Central Store 根目录", paths.data_root(), source))?;
    let mut visible_path = paths.data_root().to_owned();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(LifecycleError::UnsafeCentralStore(
                path.display().to_string(),
            ));
        };
        visible_path.push(name);
        handle = open_directory_at(&handle, name)
            .map_err(|source| io_error("安全打开受管目录", &visible_path, source))?;
        ensure_open_directory_matches_managed_path(paths, &handle, &visible_path)?;
    }
    ensure_open_directory_matches_managed_path(paths, root, paths.data_root())?;
    Ok(handle)
}

pub(crate) fn open_expected_directory_at(
    parent: &File,
    name: &OsStr,
    path: &Path,
) -> Result<File, LifecycleError> {
    open_directory_at(parent, name).map_err(|source| io_error("安全打开受管目录", path, source))
}

pub(crate) fn ensure_open_directory_matches_managed_path(
    paths: &ApplicationPaths,
    handle: &File,
    path: &Path,
) -> Result<(), LifecycleError> {
    ensure_managed_directory(paths, path)?;
    let opened = handle
        .metadata()
        .map_err(|source| io_error("检查已打开受管目录", path, source))?;
    let visible =
        fs::symlink_metadata(path).map_err(|source| io_error("重新检查受管目录", path, source))?;
    if !opened.is_dir()
        || visible.file_type().is_symlink()
        || !visible.is_dir()
        || opened.dev() != visible.dev()
        || opened.ino() != visible.ino()
    {
        return Err(LifecycleError::UnsafeCentralStore(
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn managed_relative_path<'a>(
    paths: &ApplicationPaths,
    path: &'a Path,
) -> Result<&'a Path, LifecycleError> {
    let relative = path.strip_prefix(paths.data_root()).map_err(|_| {
        LifecycleError::UnsafeCentralStore(format!("{} 不属于 Central Store", path.display()))
    })?;
    if relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        || relative.as_os_str().is_empty()
    {
        Ok(relative)
    } else {
        Err(LifecycleError::UnsafeCentralStore(
            path.display().to_string(),
        ))
    }
}

pub(crate) fn open_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let name = c_string(name)?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: openat 返回一个由本 File 独占关闭的有效 descriptor。
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

pub(crate) fn mkdir_at(parent: &File, name: &OsStr, mode: u32) -> io::Result<()> {
    let name = c_string(name)?;
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), mode as libc::mode_t) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn symlink_at(target: &Path, parent: &File, name: &OsStr) -> io::Result<()> {
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "target contains NUL"))?;
    let name = c_string(name)?;
    if unsafe { libc::symlinkat(target.as_ptr(), parent.as_raw_fd(), name.as_ptr()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn open_regular_file_at(
    parent: &File,
    name: &OsStr,
    path: &Path,
    writable: bool,
) -> Result<File, LifecycleError> {
    let name = c_string(name).map_err(|source| io_error("解析受管文件名", path, source))?;
    let access = if writable {
        libc::O_RDWR
    } else {
        libc::O_RDONLY
    };
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            access | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if descriptor < 0 {
        return Err(io_error(
            "安全打开受管文件",
            path,
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: openat 成功返回的 descriptor 由 File 独占。
    let file = unsafe { File::from_raw_fd(descriptor) };
    let metadata = file
        .metadata()
        .map_err(|source| io_error("检查受管文件", path, source))?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(LifecycleError::UnsafeCentralStore(
            path.display().to_string(),
        ));
    }
    Ok(file)
}

fn create_new_file_at(parent: &File, name: &OsStr, path: &Path) -> Result<File, LifecycleError> {
    let name = c_string(name).map_err(|source| io_error("解析受管文件名", path, source))?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        Err(io_error("创建受管文件", path, io::Error::last_os_error()))
    } else {
        // SAFETY: openat 成功返回的 descriptor 由 File 独占。
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

fn ensure_regular_file_at(parent: &File, name: &OsStr, path: &Path) -> Result<(), LifecycleError> {
    match open_regular_file_at(parent, name, path, true) {
        Ok(_) => Ok(()),
        Err(LifecycleError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            create_new_file_at(parent, name, path)?
                .sync_all()
                .map_err(|source| io_error("同步受管文件", path, source))?;
            parent
                .sync_all()
                .map_err(|source| io_error("同步受管文件父目录", path, source))
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn rename_at_replace(
    source_parent: &File,
    source_name: &OsStr,
    destination_parent: &File,
    destination_name: &OsStr,
) -> io::Result<()> {
    let source_name = c_string(source_name)?;
    let destination_name = c_string(destination_name)?;
    if unsafe {
        libc::renameat(
            source_parent.as_raw_fd(),
            source_name.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_name.as_ptr(),
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn unlink_at(parent: &File, name: &OsStr, directory: bool) -> io::Result<()> {
    let name = c_string(name)?;
    let flags = if directory { libc::AT_REMOVEDIR } else { 0 };
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn remove_empty_directory_at(
    parent: &File,
    name: &OsStr,
    path: &Path,
) -> Result<(), LifecycleError> {
    let child = match open_directory_at(parent, name) {
        Ok(child) => child,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            // 上次进程可能停在 unlink 与父目录 fsync 之间；重启时补齐持久化。
            return parent
                .sync_all()
                .map_err(|source| io_error("同步已清理目录父级", path, source));
        }
        Err(source) => return Err(io_error("安全打开待清理目录", path, source)),
    };
    if !read_entry_names_os_from_handle(&child)?.is_empty() {
        return Err(LifecycleError::RecoveryBlocked(format!(
            "事务目录包含未知内容：{}",
            path.display()
        )));
    }
    drop(child);
    unlink_at(parent, name, true).map_err(|source| io_error("清理空事务目录", path, source))?;
    parent
        .sync_all()
        .map_err(|source| io_error("同步事务目录父级", path, source))
}

pub(crate) fn remove_owned_tree_at(
    parent: &File,
    name: &OsStr,
    path: &Path,
) -> Result<(), LifecycleError> {
    remove_owned_tree_at_with_hook(parent, name, path, &mut || {})
}

/// 测试钩子在每个目录项删除后运行，用来验证递归清理自身再次中断仍可重放。
pub(crate) fn remove_owned_tree_at_with_hook(
    parent: &File,
    name: &OsStr,
    path: &Path,
    after_entry_removed: &mut impl FnMut(),
) -> Result<(), LifecycleError> {
    let child = open_directory_at(parent, name)
        .map_err(|source| io_error("安全打开事务清理目标", path, source))?;
    remove_owned_tree_contents(&child, path, after_entry_removed)?;
    drop(child);
    unlink_at(parent, name, true).map_err(|source| io_error("清理事务目录", path, source))?;
    parent
        .sync_all()
        .map_err(|source| io_error("同步事务目录父级", path, source))
}

/// 删除意图持久化前封存事务根目录身份，防止恢复时删除同名替换目录。
pub(crate) fn capture_owned_tree_cleanup_manifest(
    parent: &File,
    name: &OsStr,
    path: &Path,
) -> Result<OwnedTreeCleanupManifest, LifecycleError> {
    let root = open_directory_at(parent, name)
        .map_err(|source| io_error("安全打开待封存清理目录", path, source))?;
    let root_metadata = root
        .metadata()
        .map_err(|source| io_error("检查待封存清理目录", path, source))?;
    if !root_metadata.is_dir() {
        return Err(LifecycleError::RecoveryBlocked(format!(
            "待清理根目录身份异常：{}",
            path.display()
        )));
    }
    Ok(OwnedTreeCleanupManifest {
        root_device: root_metadata.dev(),
        root_inode: root_metadata.ino(),
    })
}

/// 清理中断后继续删除同一事务根；调用方只在事务自有隐藏目录中使用这一轻量清单。
pub(crate) fn remove_owned_tree_at_with_manifest_and_hook(
    parent: &File,
    name: &OsStr,
    path: &Path,
    manifest: &OwnedTreeCleanupManifest,
    after_entry_removed: &mut impl FnMut(),
) -> Result<(), LifecycleError> {
    validate_owned_tree_cleanup_manifest(manifest)?;
    let root = match open_directory_at(parent, name) {
        Ok(root) => root,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            // 缺失可能来自上次已完成但未 fsync 的删除，先同步再确认清理完成。
            return parent
                .sync_all()
                .map_err(|source| io_error("同步已清理事务目录父级", path, source));
        }
        Err(source) => return Err(io_error("安全打开事务清理目标", path, source)),
    };
    let root_metadata = root
        .metadata()
        .map_err(|source| io_error("检查事务清理目标", path, source))?;
    if root_metadata.dev() != manifest.root_device
        || root_metadata.ino() != manifest.root_inode
        || !root_metadata.is_dir()
    {
        return Err(LifecycleError::RecoveryBlocked(format!(
            "事务清理根目录已被替换：{}",
            path.display()
        )));
    }
    remove_owned_tree_contents(&root, path, after_entry_removed)?;
    ensure_visible_directory_identity(
        parent,
        name,
        path,
        manifest.root_device,
        manifest.root_inode,
    )?;
    drop(root);
    unlink_at(parent, name, true).map_err(|source| io_error("清理事务目录", path, source))?;
    parent
        .sync_all()
        .map_err(|source| io_error("同步事务目录父级", path, source))
}

fn ensure_visible_directory_identity(
    parent: &File,
    name: &OsStr,
    path: &Path,
    expected_device: u64,
    expected_inode: u64,
) -> Result<(), LifecycleError> {
    let visible = entry_metadata_at(parent, name)
        .map_err(|source| io_error("重新检查事务清理目录", path, source))?
        .ok_or_else(|| cleanup_entry_disappeared(path))?;
    if visible.st_dev as u64 != expected_device
        || visible.st_ino != expected_inode
        || visible.st_mode & libc::S_IFMT != libc::S_IFDIR
    {
        return Err(cleanup_entry_changed(path));
    }
    Ok(())
}

pub(crate) fn validate_owned_tree_cleanup_manifest(
    manifest: &OwnedTreeCleanupManifest,
) -> Result<(), LifecycleError> {
    if manifest.root_device == 0 || manifest.root_inode == 0 {
        return Err(LifecycleError::RecoveryBlocked(
            "待清理事务根目录身份无效".to_owned(),
        ));
    }
    Ok(())
}

/// Removal 在 destructive rename 前封存完整树，避免恢复时删除后来出现的未知内容。
pub(crate) fn capture_sealed_tree_cleanup_manifest(
    parent: &File,
    name: &OsStr,
    path: &Path,
) -> Result<SealedTreeCleanupManifest, LifecycleError> {
    let root = open_directory_at(parent, name)
        .map_err(|source| io_error("安全打开待封存清理目录", path, source))?;
    let root_metadata = root
        .metadata()
        .map_err(|source| io_error("检查待封存清理目录", path, source))?;
    if !root_metadata.is_dir() {
        return Err(LifecycleError::RecoveryBlocked(format!(
            "待清理根目录身份异常：{}",
            path.display()
        )));
    }
    Ok(SealedTreeCleanupManifest {
        root_device: root_metadata.dev(),
        root_inode: root_metadata.ino(),
        entries: capture_sealed_tree_entries(&root, path)?,
    })
}

/// 只删除完整清单中的条目；已删除条目可重放，新增或替换内容会阻塞并保留。
pub(crate) fn remove_sealed_tree_at_with_manifest(
    parent: &File,
    name: &OsStr,
    path: &Path,
    manifest: &SealedTreeCleanupManifest,
) -> Result<(), LifecycleError> {
    validate_sealed_tree_cleanup_manifest(manifest)?;
    let root = match open_directory_at(parent, name) {
        Ok(root) => root,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return parent
                .sync_all()
                .map_err(|source| io_error("同步已清理事务目录父级", path, source));
        }
        Err(source) => return Err(io_error("安全打开事务清理目标", path, source)),
    };
    let root_metadata = root
        .metadata()
        .map_err(|source| io_error("检查事务清理目标", path, source))?;
    if root_metadata.dev() != manifest.root_device
        || root_metadata.ino() != manifest.root_inode
        || !root_metadata.is_dir()
    {
        return Err(LifecycleError::RecoveryBlocked(format!(
            "事务清理根目录已被替换：{}",
            path.display()
        )));
    }
    let current_indices = validate_sealed_tree_entries(&root, path, &manifest.entries)?;
    remove_sealed_manifest_entries(&root, path, &manifest.entries, current_indices)?;
    ensure_visible_directory_identity(
        parent,
        name,
        path,
        manifest.root_device,
        manifest.root_inode,
    )?;
    drop(root);
    unlink_at(parent, name, true).map_err(|source| io_error("清理事务目录", path, source))?;
    parent
        .sync_all()
        .map_err(|source| io_error("同步事务目录父级", path, source))
}

fn validate_sealed_tree_cleanup_manifest(
    manifest: &SealedTreeCleanupManifest,
) -> Result<(), LifecycleError> {
    if manifest.root_device == 0
        || manifest.root_inode == 0
        || !valid_sealed_tree_entries(&manifest.entries)
    {
        return Err(LifecycleError::RecoveryBlocked(
            "待清理事务目录清单无效".to_owned(),
        ));
    }
    Ok(())
}

fn capture_sealed_tree_entries(
    directory: &File,
    path: &Path,
) -> Result<Vec<SealedTreeCleanupEntry>, LifecycleError> {
    let root = directory
        .try_clone()
        .map_err(|source| io_error("保留待封存事务根目录", path, source))?;
    let mut entries = Vec::new();
    let mut pending = vec![(root, path.to_path_buf(), None)];
    while let Some((current, current_path, parent_index)) = pending.pop() {
        for name in read_entry_names_os_from_handle(&current)? {
            let child_path = current_path.join(&name);
            let metadata = entry_metadata_at(&current, &name)
                .map_err(|source| io_error("检查待封存事务条目", &child_path, source))?
                .ok_or_else(|| cleanup_entry_disappeared(&child_path))?;
            let entry_type = (metadata.st_mode & libc::S_IFMT) as u64;
            let entry_index = entries.len();
            entries.push(SealedTreeCleanupEntry {
                parent_index,
                name_hex: encode_manifest_name(&name),
                device: metadata.st_dev as u64,
                inode: metadata.st_ino,
                entry_type,
                size: metadata.st_size,
                modified_at: metadata.st_mtime,
            });
            if entry_type == libc::S_IFDIR as u64 {
                let child = open_directory_at(&current, &name)
                    .map_err(|source| io_error("安全打开待封存事务子目录", &child_path, source))?;
                ensure_open_entry_identity(&child, &metadata, &child_path)?;
                pending.push((child, child_path, Some(entry_index)));
            }
        }
    }
    Ok(entries)
}

fn validate_sealed_tree_entries(
    directory: &File,
    path: &Path,
    expected: &[SealedTreeCleanupEntry],
) -> Result<Vec<usize>, LifecycleError> {
    let expected_by_name = expected
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let name = decode_manifest_name_bytes(&entry.name_hex).ok_or_else(|| {
                LifecycleError::RecoveryBlocked("待清理事务目录清单无效".to_owned())
            })?;
            Ok(((entry.parent_index, name), (index, entry)))
        })
        .collect::<Result<BTreeMap<_, _>, LifecycleError>>()?;
    let root = directory
        .try_clone()
        .map_err(|source| io_error("保留待清理事务根目录", path, source))?;
    let mut current_indices = Vec::new();
    let mut pending = vec![(root, path.to_path_buf(), None)];
    while let Some((current, current_path, parent_index)) = pending.pop() {
        for name in read_entry_names_os_from_handle(&current)? {
            let child_path = current_path.join(&name);
            let (entry_index, entry) = expected_by_name
                .get(&(parent_index, name.as_bytes().to_vec()))
                .ok_or_else(|| cleanup_entry_changed(&child_path))?;
            let metadata = entry_metadata_at(&current, &name)
                .map_err(|source| io_error("检查待清理事务条目", &child_path, source))?
                .ok_or_else(|| cleanup_entry_disappeared(&child_path))?;
            ensure_manifest_entry_identity(entry, &metadata, &child_path)?;
            current_indices.push(*entry_index);
            if entry.entry_type == libc::S_IFDIR as u64 {
                let child = open_directory_at(&current, &name)
                    .map_err(|source| io_error("安全打开待清理事务子目录", &child_path, source))?;
                ensure_open_entry_identity(&child, &metadata, &child_path)?;
                pending.push((child, child_path, Some(*entry_index)));
            }
        }
    }
    Ok(current_indices)
}

fn remove_sealed_manifest_entries(
    directory: &File,
    path: &Path,
    expected: &[SealedTreeCleanupEntry],
    mut current_indices: Vec<usize>,
) -> Result<(), LifecycleError> {
    let depths = sealed_manifest_depths(expected);
    current_indices.sort_by(|left, right| {
        depths[*right]
            .cmp(&depths[*left])
            .then_with(|| {
                expected[*left]
                    .parent_index
                    .cmp(&expected[*right].parent_index)
            })
            .then_with(|| expected[*left].name_hex.cmp(&expected[*right].name_hex))
    });
    let mut start = 0;
    while start < current_indices.len() {
        let parent_index = expected[current_indices[start]].parent_index;
        let mut end = start + 1;
        while end < current_indices.len()
            && expected[current_indices[end]].parent_index == parent_index
        {
            end += 1;
        }
        let Some((parent, parent_path)) =
            open_sealed_manifest_parent(directory, path, expected, parent_index)?
        else {
            start = end;
            continue;
        };
        for entry_index in &current_indices[start..end] {
            let entry = &expected[*entry_index];
            let name = decode_manifest_name(&entry.name_hex)?;
            let child_path = parent_path.join(&name);
            let Some(metadata) = entry_metadata_at(&parent, &name)
                .map_err(|source| io_error("检查待清理事务条目", &child_path, source))?
            else {
                continue;
            };
            ensure_manifest_entry_identity(entry, &metadata, &child_path)?;
            unlink_at(&parent, &name, entry.entry_type == libc::S_IFDIR as u64)
                .map_err(|source| io_error("清理事务条目", &child_path, source))?;
        }
        parent
            .sync_all()
            .map_err(|source| io_error("同步事务清理目录", &parent_path, source))?;
        start = end;
    }
    Ok(())
}

fn open_sealed_manifest_parent(
    root: &File,
    root_path: &Path,
    entries: &[SealedTreeCleanupEntry],
    parent_index: Option<usize>,
) -> Result<Option<(File, PathBuf)>, LifecycleError> {
    let mut chain = Vec::new();
    let mut cursor = parent_index;
    while let Some(index) = cursor {
        chain.push(index);
        cursor = entries[index].parent_index;
    }
    chain.reverse();
    let mut current = root
        .try_clone()
        .map_err(|source| io_error("保留事务清理根目录", root_path, source))?;
    let mut current_path = root_path.to_path_buf();
    current
        .set_permissions(fs::Permissions::from_mode(0o700))
        .map_err(|source| io_error("准备清理事务目录", &current_path, source))?;
    for index in chain {
        let entry = &entries[index];
        let name = decode_manifest_name(&entry.name_hex)?;
        current_path.push(&name);
        let Some(metadata) = entry_metadata_at(&current, &name)
            .map_err(|source| io_error("检查事务清理父目录", &current_path, source))?
        else {
            return Ok(None);
        };
        ensure_manifest_entry_identity(entry, &metadata, &current_path)?;
        let child = open_directory_at(&current, &name)
            .map_err(|source| io_error("安全打开事务清理父目录", &current_path, source))?;
        ensure_open_entry_identity(&child, &metadata, &current_path)?;
        child
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|source| io_error("准备清理事务子目录", &current_path, source))?;
        current = child;
    }
    Ok(Some((current, current_path)))
}

fn sealed_manifest_depths(entries: &[SealedTreeCleanupEntry]) -> Vec<usize> {
    let mut depths = Vec::with_capacity(entries.len());
    for entry in entries {
        depths.push(
            entry
                .parent_index
                .map_or(1, |parent_index| depths[parent_index] + 1),
        );
    }
    depths
}

fn ensure_manifest_entry_identity(
    expected: &SealedTreeCleanupEntry,
    actual: &libc::stat,
    path: &Path,
) -> Result<(), LifecycleError> {
    let entry_type = (actual.st_mode & libc::S_IFMT) as u64;
    let file_metadata_changed = entry_type != libc::S_IFDIR as u64
        && (actual.st_size != expected.size || actual.st_mtime != expected.modified_at);
    if actual.st_dev as u64 != expected.device
        || actual.st_ino != expected.inode
        || entry_type != expected.entry_type
        || file_metadata_changed
    {
        return Err(cleanup_entry_changed(path));
    }
    Ok(())
}

fn ensure_open_entry_identity(
    opened: &File,
    expected: &libc::stat,
    path: &Path,
) -> Result<(), LifecycleError> {
    let actual = opened
        .metadata()
        .map_err(|source| io_error("检查已打开事务目录", path, source))?;
    if actual.dev() != expected.st_dev as u64 || actual.ino() != expected.st_ino || !actual.is_dir()
    {
        return Err(cleanup_entry_changed(path));
    }
    Ok(())
}

fn valid_sealed_tree_entries(entries: &[SealedTreeCleanupEntry]) -> bool {
    let mut names = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(name) = decode_manifest_name_bytes(&entry.name_hex) else {
            return false;
        };
        if name.is_empty()
            || name == b"."
            || name == b".."
            || name.contains(&b'/')
            || name.contains(&0)
            || entry.device == 0
            || entry.inode == 0
            || !names.insert((entry.parent_index, name))
            || entry.parent_index.is_some_and(|parent_index| {
                parent_index >= index || entries[parent_index].entry_type != libc::S_IFDIR as u64
            })
            || !matches!(
                entry.entry_type as libc::mode_t,
                libc::S_IFDIR
                    | libc::S_IFREG
                    | libc::S_IFLNK
                    | libc::S_IFIFO
                    | libc::S_IFSOCK
                    | libc::S_IFCHR
                    | libc::S_IFBLK
            )
        {
            return false;
        }
    }
    true
}

fn encode_manifest_name(name: &OsStr) -> String {
    let mut encoded = String::with_capacity(name.as_bytes().len() * 2);
    for byte in name.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn decode_manifest_name(encoded: &str) -> Result<OsString, LifecycleError> {
    decode_manifest_name_bytes(encoded)
        .map(OsString::from_vec)
        .ok_or_else(|| LifecycleError::RecoveryBlocked("待清理事务目录清单无效".to_owned()))
}

fn decode_manifest_name_bytes(encoded: &str) -> Option<Vec<u8>> {
    if !encoded.len().is_multiple_of(2) {
        return None;
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}

fn cleanup_entry_changed(path: &Path) -> LifecycleError {
    LifecycleError::RecoveryBlocked(format!(
        "待清理条目已被新增、替换或修改：{}",
        path.display()
    ))
}

fn cleanup_entry_disappeared(path: &Path) -> LifecycleError {
    LifecycleError::RecoveryBlocked(format!("事务清理条目在检查期间消失：{}", path.display()))
}

fn remove_owned_tree_contents(
    directory: &File,
    path: &Path,
    after_entry_removed: &mut impl FnMut(),
) -> Result<(), LifecycleError> {
    // 复制会保留只读目录权限；事务清理前只放宽本次自有目录，确保正常中断可自动恢复。
    directory
        .set_permissions(fs::Permissions::from_mode(0o700))
        .map_err(|source| io_error("准备清理事务目录", path, source))?;
    for name in read_entry_names_os_from_handle(directory)? {
        let child_path = path.join(&name);
        let metadata = entry_metadata_at(directory, &name)
            .map_err(|source| io_error("检查事务清理条目", &child_path, source))?
            .ok_or_else(|| {
                LifecycleError::RecoveryBlocked(format!(
                    "事务清理条目在检查期间消失：{}",
                    child_path.display()
                ))
            })?;
        if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR {
            let child = open_directory_at(directory, &name)
                .map_err(|source| io_error("安全打开事务子目录", &child_path, source))?;
            remove_owned_tree_contents(&child, &child_path, after_entry_removed)?;
            drop(child);
            unlink_at(directory, &name, true)
                .map_err(|source| io_error("清理事务子目录", &child_path, source))?;
        } else {
            // unlinkat 只删除当前 dirfd 下的目录项；软链接和特殊文件都不会被跟随。
            unlink_at(directory, &name, false)
                .map_err(|source| io_error("清理事务文件", &child_path, source))?;
        }
        after_entry_removed();
    }
    directory
        .sync_all()
        .map_err(|source| io_error("同步事务清理目录", path, source))
}

pub(crate) fn write_atomic_at(
    parent: &File,
    name: &OsStr,
    path: &Path,
    bytes: &[u8],
) -> Result<(), LifecycleError> {
    write_atomic_at_with_after_temp_sync(parent, name, path, bytes, &mut || {})
}

/// 测试钩子精确位于临时文件持久化与原子 rename 之间，用于验证真实进程退出恢复。
pub(crate) fn write_atomic_at_with_after_temp_sync(
    parent: &File,
    name: &OsStr,
    path: &Path,
    bytes: &[u8],
    after_temp_sync: &mut impl FnMut(),
) -> Result<(), LifecycleError> {
    match entry_metadata_at(parent, name)
        .map_err(|source| io_error("检查原子写入目标", path, source))?
    {
        Some(metadata)
            if metadata.st_mode & libc::S_IFMT == libc::S_IFREG && metadata.st_nlink == 1 => {}
        Some(_) => {
            return Err(LifecycleError::UnsafeCentralStore(
                path.display().to_string(),
            ));
        }
        None => {}
    }
    let temporary_name = OsString::from(format!(
        ".{}.tmp-{}",
        name.to_string_lossy(),
        Uuid::new_v4()
    ));
    let temporary_path = path.with_file_name(&temporary_name);
    let result = (|| {
        let mut file = create_new_file_at(parent, &temporary_name, &temporary_path)?;
        file.write_all(bytes)
            .map_err(|source| io_error("写入临时文件", &temporary_path, source))?;
        file.sync_all()
            .map_err(|source| io_error("同步临时文件", &temporary_path, source))?;
        after_temp_sync();
        rename_at_replace(parent, &temporary_name, parent, name)
            .map_err(|source| io_error("原子替换文件", path, source))?;
        parent
            .sync_all()
            .map_err(|source| io_error("同步原子写入父目录", path, source))
    })();
    if result.is_err() {
        let _ = unlink_at(parent, &temporary_name, false);
    }
    result
}

pub(crate) fn ensure_entry_absent_at(parent: &File, name: &OsStr) -> io::Result<()> {
    if entry_metadata_at(parent, name)?.is_some() {
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "target exists",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn entry_metadata_at(parent: &File, name: &OsStr) -> io::Result<Option<libc::stat>> {
    let name = c_string(name)?;
    let mut metadata = MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        // SAFETY: fstatat 成功后已经完整初始化 stat。
        Ok(Some(unsafe { metadata.assume_init() }))
    } else {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(error)
        }
    }
}

pub(crate) fn read_link_at(parent: &File, name: &OsStr) -> io::Result<PathBuf> {
    let name = c_string(name)?;
    let mut buffer = vec![0_u8; 256];
    loop {
        let length = unsafe {
            libc::readlinkat(
                parent.as_raw_fd(),
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        if length < 0 {
            return Err(io::Error::last_os_error());
        }
        let length = length as usize;
        if length < buffer.len() {
            buffer.truncate(length);
            return Ok(PathBuf::from(OsString::from_vec(buffer)));
        }
        if buffer.len() >= 16 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "symlink target is too long",
            ));
        }
        buffer.resize(buffer.len() * 2, 0);
    }
}

pub(crate) fn rename_at_no_replace(
    source_parent: &File,
    source_name: &OsStr,
    destination_parent: &File,
    destination_name: &OsStr,
) -> io::Result<()> {
    let source_name = c_string(source_name)?;
    let destination_name = c_string(destination_name)?;
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            source_parent.as_raw_fd(),
            source_name.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            source_parent.as_raw_fd(),
            source_name.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_name.as_ptr(),
            libc::RENAME_NOREPLACE,
        ) as libc::c_int
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let result = -1;
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn c_string(value: &OsStr) -> io::Result<CString> {
    CString::new(value.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

fn resolve_relative(root: &Path, relative: &str) -> Result<PathBuf, LifecycleError> {
    let relative = Path::new(relative);
    if relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Ok(root.join(relative))
    } else {
        Err(LifecycleError::UnsafeCentralStore(
            relative.display().to_string(),
        ))
    }
}

fn sync_directory(path: &Path) -> Result<(), LifecycleError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| io_error("同步目录", path, source))
}

fn ensure_absent(path: &Path) -> Result<(), LifecycleError> {
    if path_entry_exists(path)? {
        Err(LifecycleError::TargetOccupied(path.display().to_string()))
    } else {
        Ok(())
    }
}

fn path_entry_exists(path: &Path) -> Result<bool, LifecycleError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error("检查路径", path, source)),
    }
}

fn path_to_string(path: &Path) -> Result<String, LifecycleError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| LifecycleError::NonUnicodePath(path.display().to_string()))
}

fn io_error(action: &'static str, path: &Path, source: io::Error) -> LifecycleError {
    LifecycleError::Io {
        action,
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn stored_plan_with_candidates(candidates: Vec<StoredInstallCandidate>) -> StoredInstallPlan {
        StoredInstallPlan {
            id: "plan".to_owned(),
            kind: "folder_snapshot".to_owned(),
            install_mode: "create".to_owned(),
            input_path: Some("/tmp/input".to_owned()),
            input_device: 1,
            input_inode: 1,
            input_fingerprint: "bundle-fingerprint".to_owned(),
            snapshot_relative_path: None,
            source_id: None,
            source_tracked_ref: None,
            source_catalog_generation: None,
            source_marker: None,
            expected_source_marker: None,
            expected_current_target: None,
            expected_adopted_marker: None,
            bundle_id: "bundle".to_owned(),
            bundle_display_name: "Bundle".to_owned(),
            warnings: Vec::new(),
            created_at: 1,
            expires_at: i64::MAX,
            status: "consumed".to_owned(),
            candidates,
        }
    }

    fn stored_candidate(index: usize, fingerprint: &str) -> StoredInstallCandidate {
        StoredInstallCandidate {
            candidate_id: format!("candidate-{index}"),
            source_relative_path: Some(format!("skills/skill-{index}")),
            skill_name: Some(format!("skill-{index}")),
            skill_description: Some("测试 Skill".to_owned()),
            content_fingerprint: Some(fingerprint.to_owned()),
            previous_content_fingerprint: None,
            selectable: true,
            preserve_existing: false,
            validation_errors: Vec::new(),
            warnings: Vec::new(),
            default_selected: true,
            selected: true,
        }
    }

    #[test]
    fn oversized_journal_is_rejected_before_it_can_be_persisted() {
        // 真实 Bundle 上限允许大量成员；写入端必须使用与恢复端相同的 1 MiB 上限。
        let candidates = (0..8_000)
            .map(|index| stored_candidate(index, &format!("fingerprint-{index}")))
            .collect();
        let plan = stored_plan_with_candidates(candidates);
        let journal =
            build_journal("00000000-0000-4000-8000-000000000000", &plan).expect("应先构建 Journal");

        assert!(matches!(
            ensure_journal_fits(&journal),
            Err(LifecycleError::JournalTooLarge {
                limit: MAX_JOURNAL_BYTES,
                ..
            })
        ));
    }

    #[test]
    fn journal_preflight_uses_the_largest_lifecycle_phase() {
        let plan = stored_plan_with_candidates(vec![stored_candidate(0, "fingerprint")]);
        let mut journal =
            build_journal("00000000-0000-4000-8000-000000000000", &plan).expect("应构建 Journal");
        let ready_size = serde_json::to_vec_pretty(&journal)
            .expect("应序列化初始阶段")
            .len();
        journal.members[0]
            .source_relative_path
            .as_mut()
            .expect("测试成员应包含来源相对路径")
            .push_str(&"x".repeat(MAX_JOURNAL_BYTES - ready_size));
        assert_eq!(
            serde_json::to_vec_pretty(&journal)
                .expect("应序列化边界 Journal")
                .len(),
            MAX_JOURNAL_BYTES
        );

        assert!(matches!(
            ensure_journal_fits(&journal),
            Err(LifecycleError::JournalTooLarge { .. })
        ));
    }

    #[test]
    fn changed_candidate_is_rejected_before_current_can_be_created() {
        let temp = tempdir().expect("应创建临时目录");
        let candidate_path = temp.path().join("candidate");
        let members_path = candidate_path.join("members");
        let member_path = members_path.join("demo");
        fs::create_dir_all(&member_path).expect("应创建候选目录");
        fs::write(
            member_path.join("SKILL.md"),
            "---\nname: demo\ndescription: 测试\n---\n\n正文\n",
        )
        .expect("应写入 Skill");
        let validated = validate_single_skill_folder(&member_path).expect("候选应有效");
        let mut plan =
            stored_plan_with_candidates(vec![stored_candidate(0, &validated.fingerprint)]);
        plan.candidates[0].candidate_id = "candidate-demo".to_owned();
        plan.candidates[0].source_relative_path = Some("demo".to_owned());
        plan.candidates[0].skill_name = Some("demo".to_owned());
        let journal =
            build_journal("00000000-0000-4000-8000-000000000000", &plan).expect("应构建 Journal");
        let candidate = File::open(&candidate_path).expect("应打开候选目录");
        let members = File::open(&members_path).expect("应打开成员目录");
        fs::write(member_path.join("extra.txt"), "发生变化").expect("应模拟复制期间内容变化");
        let result =
            validate_prepared_candidate(&candidate, &members, &members_path, &journal, &plan);
        assert!(
            matches!(&result, Err(LifecycleError::PlanPreconditionChanged)),
            "候选变化应在发布前被拒绝：{result:?}"
        );
    }
}
