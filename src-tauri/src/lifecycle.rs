use std::{
    collections::BTreeSet,
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
        BundleCopyBudget, ContentValidationError, copy_single_skill_tree_into_open_directory,
        validate_single_skill_folder, validate_skill_bundle_folder,
    },
    domain::{FolderInstallCandidate, FolderInstallPlan},
    paths::ApplicationPaths,
    storage::{
        NewInstallCandidate, NewInstallPlan, Storage, StorageError, StoredInstallCandidate,
        StoredInstallPlan, StoredLifecycleTransaction,
    },
};

const PLAN_TTL_MILLIS: i64 = 30 * 60 * 1_000;
const JOURNAL_VERSION: u32 = 1;
const MAX_JOURNAL_BYTES: usize = 1024 * 1024;

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
}

#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Content(#[from] ContentValidationError),
    #[error("文件夹路径包含无法保存的字符：{0}")]
    NonUnicodePath(String),
    #[error("安装 Plan 的前置状态已经变化，请重新选择文件夹")]
    PlanPreconditionChanged,
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
    stable_member_relative: String,
    #[serde(default)]
    members: Vec<InstallJournalMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InstallJournalMember {
    candidate_id: String,
    source_relative_path: String,
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
    Expected,
    Other(String),
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
            render_notice(paths, &[]).as_bytes(),
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
) -> Result<FolderInstallPlan, LifecycleError> {
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
        candidates.push(FolderInstallCandidate {
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
    let anchor_index = candidates
        .iter()
        .position(|candidate| candidate.selectable)
        .unwrap_or(0);
    let anchor = &candidates[anchor_index];
    let anchor_skill_name = anchor
        .skill_name
        .clone()
        .unwrap_or_else(|| anchor.candidate_id.clone());
    let anchor_description = anchor.description.clone().unwrap_or_default();
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
            source_relative_path: &candidate.source_relative_path,
            skill_name: candidate.skill_name.as_deref(),
            skill_description: candidate.description.as_deref(),
            content_fingerprint: discovered.fingerprint.as_deref(),
            selectable: candidate.selectable,
            validation_errors: &candidate.validation_errors,
            warnings: &candidate.warnings,
            default_selected: candidate.default_selected,
        })
        .collect::<Vec<_>>();
    storage.save_install_plan(NewInstallPlan {
        id: &plan_id,
        input_path: &input_path,
        input_device: input_metadata.dev(),
        input_inode: input_metadata.ino(),
        input_fingerprint: &validated.fingerprint,
        bundle_id: &bundle_id,
        bundle_display_name: &bundle_display_name,
        member_id: &anchor.candidate_id,
        skill_name: &anchor_skill_name,
        skill_description: &anchor_description,
        warnings: &warnings,
        candidates: &new_candidates,
        created_at: now,
        expires_at,
    })?;

    Ok(FolderInstallPlan {
        id: plan_id,
        input_path,
        bundle_display_name,
        candidates,
        warnings,
        will_mount: false,
        created_at: now,
        expires_at,
    })
}

pub fn confirm_folder_install(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
    selected_candidate_ids: &[String],
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), LifecycleError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    let preview = storage.read_install_plan(plan_id)?;
    validate_stored_plan_identifiers(&preview)?;
    verify_plan_input(&preview)?;
    ensure_absent(&paths.bundle_directory(&preview.bundle_id))?;
    preflight_lifecycle_directories(paths)?;

    let transaction_id = Uuid::new_v4().to_string();
    let journal_relative = format!("journals/{transaction_id}.json");
    // 在消费 Plan 前先固定最终选择，并证明生成的 Journal 一定能被恢复器完整读取。
    let expected_plan = prepare_selected_plan(&preview, selected_candidate_ids)?;
    let mut journal = build_journal(&transaction_id, &expected_plan)?;
    ensure_journal_fits(&journal)?;
    let plan = storage.begin_install_transaction_with_selection(
        plan_id,
        selected_candidate_ids,
        &transaction_id,
        &journal_relative,
        now,
    )?;
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
        handle_execution_error(paths, &lifecycle_lock, storage, &journal, now, &error)?;
        return Err(error);
    }
    cleanup_completed(paths, &lifecycle_lock, storage, &journal)?;
    lifecycle_lock.recheck(paths)?;
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

    let mut journal = read_journal_at(&journals, &journal_name, &journal_path)?;
    validate_journal_contract(&journal, transaction, &plan)?;
    if journal.members.is_empty() {
        journal.members = build_journal(&transaction.id, &plan)?.members;
    }
    if transaction.status == "aborted" {
        if !matches!(
            inspect_current(paths, lifecycle_lock.root(), &journal)?,
            CurrentState::Missing
        ) {
            return Err(LifecycleError::RecoveryBlocked(
                "已终止事务仍存在 current".to_owned(),
            ));
        }
        cleanup_before_activation(paths, lifecycle_lock.root(), &journal)?;
        remove_journal(paths, lifecycle_lock.root(), &journal)?;
        storage.forget_terminal_transaction(&transaction.id)?;
        return Ok(());
    }

    match inspect_current(paths, lifecycle_lock.root(), &journal)? {
        CurrentState::Missing if !journal.phase.activation_was_recorded() => {
            cleanup_before_activation(paths, lifecycle_lock.root(), &journal)?;
            storage.abort_lifecycle_transaction(&transaction.id, None, now)?;
            remove_journal(paths, lifecycle_lock.root(), &journal)?;
            storage.forget_terminal_transaction(&transaction.id)?;
        }
        CurrentState::Missing => {
            return Err(LifecycleError::RecoveryBlocked(
                "事务记录已经生效，但 current 已缺失".to_owned(),
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
            cleanup_completed(paths, lifecycle_lock, storage, &journal)?;
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
    for member in &journal.members {
        copy_single_skill_tree_into_open_directory(
            &Path::new(&plan.input_path).join(&member.source_relative_path),
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
    verify_plan_snapshot(plan)?;
    validate_prepared_candidate(&candidate, &members, &members_path, journal, plan)?;
    lifecycle_lock.recheck(paths)?;

    let bundles_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?;
    mkdir_at(&bundles_root, OsStr::new(&journal.bundle_id), 0o700)
        .map_err(|source| io_error("创建 Bundle 目录", &paths.bundles_root(), source))?;
    let bundle = open_directory_at(&bundles_root, OsStr::new(&journal.bundle_id))
        .map_err(|source| io_error("打开 Bundle 目录", &paths.bundles_root(), source))?;
    mkdir_at(&bundle, OsStr::new("contents"), 0o700)
        .map_err(|source| io_error("创建内容目录", &paths.bundles_root(), source))?;
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
    rename_at_no_replace(
        &bundle,
        &temporary_current_name,
        &bundle,
        OsStr::new("current"),
    )
    .map_err(|source| io_error("切换 current", &paths.bundles_root(), source))?;
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

fn verify_plan_input(plan: &StoredInstallPlan) -> Result<(), LifecycleError> {
    if plan.status != "pending" {
        return Err(StorageError::InstallPlanConsumed.into());
    }
    verify_plan_snapshot(plan)
}

fn verify_plan_snapshot(plan: &StoredInstallPlan) -> Result<(), LifecycleError> {
    let validated = validate_skill_bundle_folder(Path::new(&plan.input_path))?;
    let metadata = fs::symlink_metadata(&validated.canonical_root)
        .map_err(|source| io_error("读取输入目录", &validated.canonical_root, source))?;
    use std::os::unix::fs::MetadataExt;
    let canonical = path_to_string(&validated.canonical_root)?;
    let unchanged = canonical == plan.input_path
        && metadata.dev() == plan.input_device
        && metadata.ino() == plan.input_inode
        && validated.fingerprint == plan.input_fingerprint
        && stored_candidates_match(&plan.candidates, &validated.candidates)?;
    if unchanged {
        Ok(())
    } else {
        Err(LifecycleError::PlanPreconditionChanged)
    }
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
        candidate.selected = selected.contains(candidate.candidate_id.as_str());
    }
    Ok(selected_plan)
}

fn validate_stored_plan_identifiers(plan: &StoredInstallPlan) -> Result<(), LifecycleError> {
    for (label, value) in [
        ("plan id", plan.id.as_str()),
        ("bundle id", plan.bundle_id.as_str()),
        ("member id", plan.member_id.as_str()),
        ("skill name", plan.skill_name.as_str()),
    ] {
        ensure_single_path_component(label, value)?;
    }
    for candidate in &plan.candidates {
        ensure_single_path_component("candidate id", &candidate.candidate_id)?;
        if let Some(skill_name) = &candidate.skill_name {
            ensure_single_path_component("candidate skill name", skill_name)?;
        }
        ensure_safe_relative_path(
            "candidate source relative path",
            &candidate.source_relative_path,
        )?;
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
        if stored.source_relative_path != path_to_string(&discovered.relative_path)?
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
            !candidate.selectable
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
    let mut comparable = actual.clone();
    // 0003 的单成员 Journal 没有 members；升级后用已验证的唯一 Plan 成员补齐比较。
    if comparable.members.is_empty() && expected.members.len() == 1 {
        comparable.members = expected.members.clone();
    }
    if comparable == expected
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
    now: i64,
    error: &LifecycleError,
) -> Result<(), LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    match inspect_current(paths, lifecycle_lock.root(), journal)? {
        CurrentState::Missing if !journal.phase.activation_was_recorded() => {
            if let Err(cleanup_error) =
                cleanup_before_activation(paths, lifecycle_lock.root(), journal)
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
        CurrentState::Missing => block_recovery(
            storage,
            &journal.transaction_id,
            "current 在生效后消失",
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
                cleanup_empty_staging(paths, lifecycle_lock.root(), &journal)?;
                cleanup_temporary_current(paths, lifecycle_lock.root(), &journal)?;
                write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
                storage.forget_terminal_transaction(&transaction.id)?;
                return Ok(());
            }
            CurrentState::Missing => {
                return Err(LifecycleError::RecoveryBlocked(
                    "已完成事务的 current 缺失".to_owned(),
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
        if !matches!(
            inspect_current(paths, lifecycle_lock.root(), &journal)?,
            CurrentState::Missing
        ) {
            return Err(LifecycleError::RecoveryBlocked(
                "已终止事务仍存在 current".to_owned(),
            ));
        }
        cleanup_before_activation(paths, lifecycle_lock.root(), &journal)?;
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
    if path_entry_exists(&bundle)? || path_entry_exists(&staging)? {
        return Err(LifecycleError::RecoveryBlocked(
            "Journal 写入前出现了未知业务内容".to_owned(),
        ));
    }
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
    let bundles_path = paths.bundles_root();
    let bundle_path = paths.bundle_directory(&journal.bundle_id);
    let contents_path = bundle_path.join("contents");
    let content_path = contents_path.join(&journal.transaction_id);
    let members_path = content_path.join("members");

    let bundles = open_managed_directory_from_root(paths, managed_root, &bundles_path)?;
    let bundle =
        open_expected_directory_at(&bundles, OsStr::new(&journal.bundle_id), &bundle_path)?;
    let contents = open_expected_directory_at(&bundle, OsStr::new("contents"), &contents_path)?;
    let content = open_expected_directory_at(
        &contents,
        OsStr::new(&journal.transaction_id),
        &content_path,
    )?;
    let members = open_expected_directory_at(&content, OsStr::new("members"), &members_path)?;
    let mut directories = vec![
        (bundles, bundles_path),
        (bundle, bundle_path),
        (contents, contents_path),
        (content, content_path),
        (members, members_path),
    ];
    let mut member_paths = Vec::with_capacity(journal.members.len());
    for member in &journal.members {
        let member_path = directories[4].1.join(&member.skill_name);
        let member_handle = open_expected_directory_at(
            &directories[4].0,
            OsStr::new(&member.skill_name),
            &member_path,
        )?;
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

fn read_entry_names_from_handle(directory: &File) -> Result<Vec<String>, LifecycleError> {
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

fn cleanup_before_activation(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &InstallJournal,
) -> Result<(), LifecycleError> {
    cleanup_temporary_current(paths, managed_root, journal)?;
    stage_unactivated_content_for_cleanup(paths, managed_root, journal)?;
    cleanup_staging_before_activation(paths, managed_root, journal)?;
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
) -> Result<(), LifecycleError> {
    lifecycle_lock.recheck(paths)?;
    cleanup_empty_staging(paths, lifecycle_lock.root(), journal)?;
    cleanup_temporary_current(paths, lifecycle_lock.root(), journal)?;
    remove_journal(paths, lifecycle_lock.root(), journal)?;
    storage.forget_terminal_transaction(&journal.transaction_id)?;
    lifecycle_lock.recheck(paths)?;
    Ok(())
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
    let rows = storage.managed_bundle_notice_rows()?;
    write_atomic_at(
        &root,
        OsStr::new("SKILLYARD-INFO.md"),
        &paths.central_store_notice(),
        render_notice(paths, &rows).as_bytes(),
    )
}

fn render_notice(paths: &ApplicationPaths, bundles: &[(String, String, Vec<String>)]) -> String {
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
    notice.push_str("\n## Source\n\n- 当前没有已登记 Source。\n");
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

fn open_expected_directory_at(
    parent: &File,
    name: &OsStr,
    path: &Path,
) -> Result<File, LifecycleError> {
    open_directory_at(parent, name).map_err(|source| io_error("安全打开受管目录", path, source))
}

fn ensure_open_directory_matches_managed_path(
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

fn remove_empty_directory_at(
    parent: &File,
    name: &OsStr,
    path: &Path,
) -> Result<(), LifecycleError> {
    let child = match open_directory_at(parent, name) {
        Ok(child) => child,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
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

fn remove_owned_tree_at(parent: &File, name: &OsStr, path: &Path) -> Result<(), LifecycleError> {
    let child = open_directory_at(parent, name)
        .map_err(|source| io_error("安全打开事务清理目标", path, source))?;
    remove_owned_tree_contents(&child, path)?;
    drop(child);
    unlink_at(parent, name, true).map_err(|source| io_error("清理事务目录", path, source))?;
    parent
        .sync_all()
        .map_err(|source| io_error("同步事务目录父级", path, source))
}

fn remove_owned_tree_contents(directory: &File, path: &Path) -> Result<(), LifecycleError> {
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
            remove_owned_tree_contents(&child, &child_path)?;
            drop(child);
            unlink_at(directory, &name, true)
                .map_err(|source| io_error("清理事务子目录", &child_path, source))?;
        } else {
            // unlinkat 只删除当前 dirfd 下的目录项；软链接和特殊文件都不会被跟随。
            unlink_at(directory, &name, false)
                .map_err(|source| io_error("清理事务文件", &child_path, source))?;
        }
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

fn ensure_entry_absent_at(parent: &File, name: &OsStr) -> io::Result<()> {
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
            input_path: "/tmp/input".to_owned(),
            input_device: 1,
            input_inode: 1,
            input_fingerprint: "bundle-fingerprint".to_owned(),
            bundle_id: "bundle".to_owned(),
            bundle_display_name: "Bundle".to_owned(),
            member_id: candidates[0].candidate_id.clone(),
            skill_name: candidates[0].skill_name.clone().expect("测试候选应有名称"),
            _legacy_skill_description: "描述".to_owned(),
            expires_at: i64::MAX,
            status: "consumed".to_owned(),
            candidates,
        }
    }

    fn stored_candidate(index: usize, fingerprint: &str) -> StoredInstallCandidate {
        StoredInstallCandidate {
            candidate_id: format!("candidate-{index}"),
            source_relative_path: format!("skills/skill-{index}"),
            skill_name: Some(format!("skill-{index}")),
            skill_description: Some("测试 Skill".to_owned()),
            content_fingerprint: Some(fingerprint.to_owned()),
            selectable: true,
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
        plan.candidates[0].source_relative_path = "demo".to_owned();
        plan.candidates[0].skill_name = Some("demo".to_owned());
        plan.member_id = "candidate-demo".to_owned();
        plan.skill_name = "demo".to_owned();
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
