use std::{
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
use thiserror::Error;
use uuid::Uuid;

use crate::{
    content::{ContentValidationError, validate_single_skill_folder},
    domain::{
        MountHealth, MountOperation, MountPlan, MountPlanPurpose, MountScope, SupportedAppId,
    },
    lifecycle::{
        LifecycleError, LifecycleFailpoint, LifecycleLock, acquire_lifecycle_lock,
        entry_metadata_at, mkdir_at, open_directory_at, open_managed_directory_from_root,
        open_regular_file_at, read_link_at, rename_at_no_replace, symlink_at, unlink_at,
        write_atomic_at, write_notice_from_storage,
    },
    paths::ApplicationPaths,
    storage::{
        NewMountPlan, NewProject, Storage, StorageError, StoredManagedMember, StoredMount,
        StoredMountPlan, StoredMountTransaction, StoredProject,
    },
};

const MOUNT_PLAN_TTL_MILLIS: i64 = 30 * 60 * 1_000;
const MOUNT_JOURNAL_VERSION: u32 = 1;
const MAX_MOUNT_JOURNAL_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum MountLifecycleError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Content(#[from] ContentValidationError),
    #[error(transparent)]
    SharedLifecycle(#[from] LifecycleError),
    #[error("SkillYard 不支持这个 Agent 应用")]
    UnsupportedApp,
    #[error("global Mount 不能选择 Project，project Mount 必须选择已登记 Project")]
    InvalidScope,
    #[error("Project 目录已经变化，请重新登记：{0}")]
    ProjectChanged(String),
    #[error("Mount 目标已经被其他内容占用：{0}")]
    MountConflict(String),
    #[error("Mount Plan 的前置状态已经变化，请重新确认")]
    PlanPreconditionChanged,
    #[error("Mount 路径不安全：{0}")]
    UnsafeMountPath(String),
    #[error("无法{action} {path}：{source}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("Mount Journal 无法解析：{0}")]
    InvalidJournal(#[from] serde_json::Error),
    #[error("Mount Journal 超过安全大小限制")]
    JournalTooLarge,
    #[error("Mount 事务恢复需要人工处理：{0}")]
    RecoveryBlocked(String),
    #[error("测试模拟 Mount 中断：{0}")]
    SimulatedInterruption(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum MountJournalPhase {
    JournalReady,
    TargetApplied,
    StateCommitted,
}

impl MountJournalPhase {
    fn as_storage_str(self) -> &'static str {
        match self {
            Self::JournalReady => "journal_ready",
            Self::TargetApplied => "target_applied",
            Self::StateCommitted => "state_committed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MountJournal {
    version: u32,
    transaction_id: String,
    plan_id: String,
    mount_id: String,
    operation: MountOperation,
    target_path: String,
    expected_target: String,
    target_observation: String,
    temporary_name: String,
    phase: MountJournalPhase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TargetKind {
    Absent,
    ExpectedLink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetSnapshot {
    kind: TargetKind,
    observation: String,
}

struct OpenMountParent {
    base: File,
    base_path: PathBuf,
    parent: File,
    parent_path: PathBuf,
}

enum ParentLookup {
    Missing,
    Open(OpenMountParent),
}

pub fn prepare_project_registration(
    _paths: &ApplicationPaths,
    storage: &Storage,
    root: &Path,
    now: i64,
) -> Result<StoredProject, MountLifecycleError> {
    let supplied =
        fs::symlink_metadata(root).map_err(|source| mount_io("检查 Project 目录", root, source))?;
    if supplied.file_type().is_symlink() || !supplied.is_dir() {
        return Err(MountLifecycleError::UnsafeMountPath(
            root.display().to_string(),
        ));
    }
    let canonical =
        fs::canonicalize(root).map_err(|source| mount_io("解析 Project 目录", root, source))?;
    let canonical_metadata = fs::symlink_metadata(&canonical)
        .map_err(|source| mount_io("重新检查 Project 目录", &canonical, source))?;
    if supplied.dev() != canonical_metadata.dev()
        || supplied.ino() != canonical_metadata.ino()
        || canonical_metadata.file_type().is_symlink()
        || !canonical_metadata.is_dir()
    {
        return Err(MountLifecycleError::ProjectChanged(
            root.display().to_string(),
        ));
    }
    let display_name = canonical
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| MountLifecycleError::UnsafeMountPath(canonical.display().to_string()))?;
    let root_path = path_to_string(&canonical)?;
    let stored = storage.prepare_project(NewProject {
        id: &Uuid::new_v4().to_string(),
        display_name,
        root_path: &root_path,
        root_device: canonical_metadata.dev(),
        root_inode: canonical_metadata.ino(),
        created_at: now,
    })?;
    Ok(stored)
}

pub fn create_mount_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    member_id: &str,
    app_id: SupportedAppId,
    scope: MountScope,
    project_id: Option<&str>,
    now: i64,
) -> Result<MountPlan, MountLifecycleError> {
    ensure_supported_scope(scope, project_id)?;
    let member = storage.read_managed_member(member_id)?;
    validate_member_content(&member)?;
    let project = read_scope_project(storage, scope, project_id)?;
    validate_project_identity(project.as_ref())?;
    let target_path = derive_target_path(paths, &member, app_id, scope, project.as_ref())?;
    let snapshot = observe_target(
        paths,
        app_id,
        scope,
        project.as_ref(),
        &member.skill_name,
        &member.expected_target,
    )?;
    if snapshot.kind == TargetKind::Other {
        return Err(MountLifecycleError::MountConflict(
            target_path.display().to_string(),
        ));
    }
    let plan_id = Uuid::new_v4().to_string();
    let mount_id = Uuid::new_v4().to_string();
    let target_path = path_to_string(&target_path)?;
    storage.save_mount_plan(NewMountPlan {
        id: &plan_id,
        operation: MountOperation::Create,
        purpose: MountPlanPurpose::Create,
        mount_id: &mount_id,
        member_id,
        app_id,
        scope,
        project_id,
        target_path: &target_path,
        expected_target: &member.expected_target,
        member_fingerprint: &member.content_fingerprint,
        target_observation: &snapshot.observation,
        created_at: now,
        expires_at: now.saturating_add(MOUNT_PLAN_TTL_MILLIS),
    })?;
    let stored = storage.read_mount_plan(&plan_id)?;
    stored_plan_to_ui(&stored, target_health(&snapshot))
}

pub fn create_remove_mount_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    mount_id: &str,
    now: i64,
) -> Result<MountPlan, MountLifecycleError> {
    let mount = storage.read_mount(mount_id)?;
    ensure_supported_scope(mount.scope, mount.project_id.as_deref())?;
    let member = storage.read_managed_member(&mount.member_id)?;
    if member.bundle_id != mount.bundle_id
        || member.skill_name != mount.skill_name
        || member.content_fingerprint != mount.member_fingerprint
        || member.expected_target != mount.expected_target
    {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    validate_member_content(&member)?;
    let project = read_scope_project(storage, mount.scope, mount.project_id.as_deref())?;
    let target = derive_target_path(paths, &member, mount.app_id, mount.scope, project.as_ref())?;
    if path_to_string(&target)? != mount.target_path {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    let snapshot = observe_removal_target(
        paths,
        mount.app_id,
        mount.scope,
        project.as_ref(),
        &member.skill_name,
        &member.expected_target,
    )?;
    let plan_id = Uuid::new_v4().to_string();
    storage.save_mount_plan(NewMountPlan {
        id: &plan_id,
        operation: MountOperation::Remove,
        purpose: MountPlanPurpose::Remove,
        mount_id,
        member_id: &member.id,
        app_id: mount.app_id,
        scope: mount.scope,
        project_id: mount.project_id.as_deref(),
        target_path: &mount.target_path,
        expected_target: &mount.expected_target,
        member_fingerprint: &mount.member_fingerprint,
        target_observation: &snapshot.observation,
        created_at: now,
        expires_at: now.saturating_add(MOUNT_PLAN_TTL_MILLIS),
    })?;
    let stored = storage.read_mount_plan(&plan_id)?;
    stored_plan_to_ui(&stored, target_health(&snapshot))
}

pub fn create_repair_mount_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    mount_id: &str,
    now: i64,
) -> Result<MountPlan, MountLifecycleError> {
    let mount = storage.read_mount(mount_id)?;
    ensure_supported_scope(mount.scope, mount.project_id.as_deref())?;
    let member = storage.read_managed_member(&mount.member_id)?;
    if member.bundle_id != mount.bundle_id
        || member.skill_name != mount.skill_name
        || member.content_fingerprint != mount.member_fingerprint
        || member.expected_target != mount.expected_target
    {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    validate_member_content(&member)?;
    let project = read_scope_project(storage, mount.scope, mount.project_id.as_deref())?;
    validate_project_identity(project.as_ref())?;
    let target = derive_target_path(paths, &member, mount.app_id, mount.scope, project.as_ref())?;
    if path_to_string(&target)? != mount.target_path {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    let snapshot = observe_target(
        paths,
        mount.app_id,
        mount.scope,
        project.as_ref(),
        &member.skill_name,
        &member.expected_target,
    )?;
    if snapshot.kind == TargetKind::Other {
        return Err(MountLifecycleError::MountConflict(
            mount.target_path.clone(),
        ));
    }

    let plan_id = Uuid::new_v4().to_string();
    storage.save_mount_plan(NewMountPlan {
        id: &plan_id,
        operation: MountOperation::Create,
        purpose: MountPlanPurpose::Repair,
        mount_id,
        member_id: &member.id,
        app_id: mount.app_id,
        scope: mount.scope,
        project_id: mount.project_id.as_deref(),
        target_path: &mount.target_path,
        expected_target: &mount.expected_target,
        member_fingerprint: &mount.member_fingerprint,
        target_observation: &snapshot.observation,
        created_at: now,
        expires_at: now.saturating_add(MOUNT_PLAN_TTL_MILLIS),
    })?;
    let stored = storage.read_mount_plan(&plan_id)?;
    stored_plan_to_ui(&stored, target_health(&snapshot))
}

/// 启动和主动刷新只更新 Mount 的观察状态，不创建、删除或改写 Host 内容。
pub fn refresh_mount_health(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    now: i64,
) -> Result<(), MountLifecycleError> {
    let mut updates = Vec::new();
    for mount in storage.read_mounts()? {
        let project = stored_mount_project(&mount)?;
        let snapshot = observe_removal_target(
            paths,
            mount.app_id,
            mount.scope,
            project.as_ref(),
            &mount.skill_name,
            &mount.expected_target,
        )?;
        let health = target_health(&snapshot);
        updates.push((mount.id, health));
    }
    storage.update_mount_health_batch(&updates, now)?;
    Ok(())
}

pub fn confirm_mount_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), MountLifecycleError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    let preview = storage.read_mount_plan(plan_id)?;
    if preview.status != "pending" {
        return Err(StorageError::MountPlanConsumed.into());
    }
    if preview.expires_at <= now {
        return Err(StorageError::MountPlanExpired.into());
    }
    validate_plan_contract(paths, storage, &preview)?;
    ensure_target_observation(paths, storage, &preview)?;

    let transaction_id = Uuid::new_v4().to_string();
    let journal_relative = format!("journals/mount-{transaction_id}.json");
    let mut journal = build_journal(&transaction_id, &preview);
    ensure_journal_fits(&journal)?;
    let plan = storage.begin_mount_transaction(plan_id, &transaction_id, &journal_relative, now)?;
    if plan != consumed_plan(&preview) {
        let error = MountLifecycleError::PlanPreconditionChanged;
        storage.abort_mount_transaction(&transaction_id, Some(&error.to_string()), now)?;
        storage.forget_terminal_mount_transaction(&transaction_id)?;
        return Err(error);
    }
    lifecycle_lock.recheck(paths)?;
    inject_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterMountTransactionRecord,
    );
    if failpoint == LifecycleFailpoint::AfterMountTransactionRecord {
        return Err(MountLifecycleError::SimulatedInterruption(
            "Mount 事务记录已提交，Journal 尚未写入",
        ));
    }

    let result = execute_mount(
        paths,
        &lifecycle_lock,
        storage,
        &plan,
        &mut journal,
        now,
        failpoint,
    );
    if let Err(error) = result {
        if matches!(error, MountLifecycleError::SimulatedInterruption(_)) {
            return Err(error);
        }
        handle_mount_error(
            paths,
            &lifecycle_lock,
            storage,
            &plan,
            &journal,
            now,
            &error,
        )?;
        return Err(error);
    }
    cleanup_completed_mount(paths, &lifecycle_lock, storage, &journal, failpoint)?;
    Ok(())
}

pub fn recover_pending_mount_transactions(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    now: i64,
) -> Result<(), MountLifecycleError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    for transaction in storage.recoverable_mount_transactions()? {
        if transaction.status == "blocked" {
            continue;
        }
        if let Err(error) =
            recover_mount_transaction(paths, &lifecycle_lock, storage, &transaction, now)
        {
            storage.block_mount_transaction(&transaction.id, &error.to_string(), now)?;
        }
        lifecycle_lock.recheck(paths)?;
    }
    Ok(())
}

fn execute_mount(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    plan: &StoredMountPlan,
    journal: &mut MountJournal,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), MountLifecycleError> {
    write_journal(paths, lifecycle_lock, journal)?;
    inject_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterMountJournalWrittenBeforePhase,
    );
    storage.update_mount_transaction_phase(
        &journal.transaction_id,
        MountJournalPhase::JournalReady.as_storage_str(),
        now,
    )?;
    lifecycle_lock.recheck(paths)?;

    apply_mount_effect(paths, plan, journal)?;
    lifecycle_lock.recheck(paths)?;
    match inspect_effect_state(paths, plan, journal)? {
        EffectState::Applied => {}
        EffectState::NotApplied => return Err(MountLifecycleError::PlanPreconditionChanged),
        EffectState::Ambiguous(message) => {
            return Err(MountLifecycleError::RecoveryBlocked(message));
        }
    }
    inject_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterMountTargetAppliedBeforePhase,
    );
    journal.phase = MountJournalPhase::TargetApplied;
    write_journal(paths, lifecycle_lock, journal)?;
    storage.update_mount_transaction_phase(
        &journal.transaction_id,
        journal.phase.as_storage_str(),
        now,
    )?;
    inject_interruption(
        failpoint,
        LifecycleFailpoint::AfterMountTargetApplied,
        LifecycleFailpoint::HardExitAfterMountTargetApplied,
        "Mount 文件系统效果已生效，SQLite 尚未完成",
    )?;

    finalize_mount_state(storage, &journal.transaction_id, plan, now)?;
    write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
    inject_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterMountStateCommittedBeforeJournal,
    );
    journal.phase = MountJournalPhase::StateCommitted;
    write_journal(paths, lifecycle_lock, journal)?;
    if failpoint == LifecycleFailpoint::AfterMountStateCommitted {
        return Err(MountLifecycleError::SimulatedInterruption(
            "Mount 状态已提交，清理尚未完成",
        ));
    }
    Ok(())
}

fn apply_mount_effect(
    paths: &ApplicationPaths,
    plan: &StoredMountPlan,
    journal: &MountJournal,
) -> Result<(), MountLifecycleError> {
    validate_plan_member_only(paths, plan)?;
    let project = stored_plan_project(plan)?;
    let parent = match open_mount_parent(
        paths,
        plan.app_id,
        plan.scope,
        project.as_ref(),
        plan.operation == MountOperation::Create,
    ) {
        Ok(parent) => parent,
        Err(error)
            if plan.operation == MountOperation::Remove
                && matches_unavailable_removal_observation(plan, &error) =>
        {
            // 父级无法安全检查时，移除只清理关系，不接触任何未知路径。
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let ParentLookup::Open(parent) = parent else {
        if plan.operation == MountOperation::Remove && plan.target_observation == "absent" {
            return Ok(());
        }
        return Err(MountLifecycleError::UnsafeMountPath(
            plan.target_path.clone(),
        ));
    };
    ensure_parent_matches_target(&parent, plan)?;
    let leaf = OsStr::new(&plan.skill_name);
    let current = snapshot_at(&parent.parent, leaf, &plan.expected_target)?;
    if current.observation != plan.target_observation {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    let temporary_name = OsStr::new(&journal.temporary_name);
    if plan.operation == MountOperation::Remove {
        ensure_temporary_absent(&parent.parent, temporary_name, &parent.parent_path)?;
    }

    match plan.operation {
        MountOperation::Create => {
            if current.kind == TargetKind::Absent {
                // `symlinkat` 在叶子不存在时一次创建完整链接；若竞态占位则只会失败，不覆盖。
                symlink_at(Path::new(&plan.expected_target), &parent.parent, leaf)
                    .map_err(|source| mount_io("原子创建 Mount", &parent.parent_path, source))?;
                parent
                    .parent
                    .sync_all()
                    .map_err(|source| mount_io("同步 Mount", &parent.parent_path, source))?;
            }
        }
        MountOperation::Remove => {
            if current.kind == TargetKind::ExpectedLink {
                quarantine_mount_for_removal(&parent, leaf, temporary_name, plan)?;
            }
            // missing 或 conflict 只移除 SkillYard 记录，绝不触碰未知占用内容。
        }
    }
    recheck_open_parent(&parent)?;
    Ok(())
}

fn quarantine_mount_for_removal(
    parent: &OpenMountParent,
    leaf: &OsStr,
    temporary_name: &OsStr,
    plan: &StoredMountPlan,
) -> Result<(), MountLifecycleError> {
    rename_at_no_replace(&parent.parent, leaf, &parent.parent, temporary_name)
        .map_err(|source| mount_io("隔离待移除 Mount", &parent.parent_path, source))?;
    let moved = snapshot_at(&parent.parent, temporary_name, &plan.expected_target)?;
    if moved.observation != plan.target_observation {
        // 验证与 rename 之间若被替换，优先把未知对象原样放回，绝不删除它。
        let restored = rename_at_no_replace(&parent.parent, temporary_name, &parent.parent, leaf);
        parent
            .parent
            .sync_all()
            .map_err(|source| mount_io("同步 Mount 竞态恢复", &parent.parent_path, source))?;
        return match restored {
            Ok(()) => Err(MountLifecycleError::PlanPreconditionChanged),
            Err(_) => Err(MountLifecycleError::RecoveryBlocked(
                "Mount 在移除时被外部替换，未知内容已保留在隔离路径".to_owned(),
            )),
        };
    }
    parent
        .parent
        .sync_all()
        .map_err(|source| mount_io("同步 Mount 移除", &parent.parent_path, source))?;
    Ok(())
}

fn handle_mount_error(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    plan: &StoredMountPlan,
    journal: &MountJournal,
    now: i64,
    error: &MountLifecycleError,
) -> Result<(), MountLifecycleError> {
    match inspect_effect_state(paths, plan, journal)? {
        EffectState::NotApplied => {
            cleanup_unapplied_temporary(paths, plan, journal)?;
            storage.abort_mount_transaction(
                &journal.transaction_id,
                Some(&error.to_string()),
                now,
            )?;
            remove_journal(paths, lifecycle_lock, journal)?;
            storage.forget_terminal_mount_transaction(&journal.transaction_id)?;
            Ok(())
        }
        EffectState::Applied => Ok(()),
        EffectState::Ambiguous(message) => {
            storage.block_mount_transaction(&journal.transaction_id, &message, now)?;
            Ok(())
        }
    }
}

fn recover_mount_transaction(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transaction: &StoredMountTransaction,
    now: i64,
) -> Result<(), MountLifecycleError> {
    ensure_single_component(&transaction.id)?;
    ensure_single_component(&transaction.plan_id)?;
    ensure_single_component(&transaction.mount_id)?;
    let expected_journal = format!("journals/mount-{}.json", transaction.id);
    if transaction.journal_path != expected_journal {
        return Err(MountLifecycleError::RecoveryBlocked(
            "SQLite 中的 Mount Journal 路径不符合固定布局".to_owned(),
        ));
    }
    let journal_name = OsString::from(format!("mount-{}.json", transaction.id));
    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    let journal_path = paths.journals_root().join(&journal_name);
    let journal_exists = entry_metadata_at(&journals, &journal_name)
        .map_err(|source| mount_io("检查 Mount Journal", &journal_path, source))?
        .is_some();
    if !journal_exists {
        if matches!(transaction.status.as_str(), "completed" | "aborted") {
            // 终态已经提交，Journal 删除后的中断只剩数据库清理。
            storage.forget_terminal_mount_transaction(&transaction.id)?;
            return Ok(());
        }
        if transaction.phase == "journal_pending" && transaction.status == "in_progress" {
            storage.abort_mount_transaction(&transaction.id, None, now)?;
            storage.forget_terminal_mount_transaction(&transaction.id)?;
            return Ok(());
        }
        return Err(MountLifecycleError::RecoveryBlocked(
            "Mount Journal 缺失但事务已经进入文件系统阶段".to_owned(),
        ));
    }
    let plan = storage.read_mount_plan(&transaction.plan_id)?;
    validate_plan_contract(paths, storage, &plan)?;
    if plan.mount_id != transaction.mount_id || plan.operation != transaction.operation {
        return Err(MountLifecycleError::RecoveryBlocked(
            "SQLite Mount 事务与 Plan 不一致".to_owned(),
        ));
    }
    let journal = read_journal(&journals, &journal_name, &journal_path)?;
    validate_journal(&journal, transaction, &plan)?;

    if transaction.status == "in_progress" && transaction.phase == "journal_pending" {
        // 正常顺序先持久化 journal_ready，再执行任何 Mount 效果；这里必定尚未开始。
        cleanup_unapplied_temporary(paths, &plan, &journal)?;
        storage.abort_mount_transaction(&transaction.id, None, now)?;
        remove_journal(paths, lifecycle_lock, &journal)?;
        storage.forget_terminal_mount_transaction(&transaction.id)?;
        return Ok(());
    }

    if transaction.status == "aborted" {
        cleanup_unapplied_temporary(paths, &plan, &journal)?;
        remove_journal(paths, lifecycle_lock, &journal)?;
        storage.forget_terminal_mount_transaction(&transaction.id)?;
        return Ok(());
    }

    match inspect_effect_state(paths, &plan, &journal)? {
        EffectState::NotApplied
            if matches!(
                transaction.phase.as_str(),
                "journal_pending" | "journal_ready"
            ) =>
        {
            cleanup_unapplied_temporary(paths, &plan, &journal)?;
            storage.abort_mount_transaction(&transaction.id, None, now)?;
            remove_journal(paths, lifecycle_lock, &journal)?;
            storage.forget_terminal_mount_transaction(&transaction.id)?;
            Ok(())
        }
        EffectState::Applied => {
            if transaction.status != "completed" {
                storage.update_mount_transaction_phase(
                    &transaction.id,
                    MountJournalPhase::TargetApplied.as_storage_str(),
                    now,
                )?;
                finalize_mount_state(storage, &transaction.id, &plan, now)?;
            }
            write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
            cleanup_applied_temporary(paths, &plan, &journal)?;
            remove_journal(paths, lifecycle_lock, &journal)?;
            storage.forget_terminal_mount_transaction(&transaction.id)?;
            Ok(())
        }
        EffectState::NotApplied => Err(MountLifecycleError::RecoveryBlocked(
            "Mount 事务记录为已生效，但目标仍是旧状态".to_owned(),
        )),
        EffectState::Ambiguous(message) => Err(MountLifecycleError::RecoveryBlocked(message)),
    }
}

fn finalize_mount_state(
    storage: &mut Storage,
    transaction_id: &str,
    plan: &StoredMountPlan,
    now: i64,
) -> Result<(), MountLifecycleError> {
    match plan.operation {
        MountOperation::Create => storage.finalize_mount_create(transaction_id, plan, now)?,
        MountOperation::Remove => storage.finalize_mount_remove(transaction_id, plan, now)?,
    }
    Ok(())
}

fn cleanup_completed_mount(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    journal: &MountJournal,
    failpoint: LifecycleFailpoint,
) -> Result<(), MountLifecycleError> {
    let plan = storage.read_mount_plan(&journal.plan_id)?;
    cleanup_applied_temporary(paths, &plan, journal)?;
    remove_journal(paths, lifecycle_lock, journal)?;
    inject_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterMountJournalRemovedBeforeForget,
    );
    storage.forget_terminal_mount_transaction(&journal.transaction_id)?;
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum EffectState {
    NotApplied,
    Applied,
    Ambiguous(String),
}

fn inspect_effect_state(
    paths: &ApplicationPaths,
    plan: &StoredMountPlan,
    journal: &MountJournal,
) -> Result<EffectState, MountLifecycleError> {
    let project = stored_plan_project(plan)?;
    let parent = match open_mount_parent(paths, plan.app_id, plan.scope, project.as_ref(), false) {
        Ok(parent) => parent,
        Err(error)
            if plan.operation == MountOperation::Remove
                && matches_unavailable_removal_observation(plan, &error) =>
        {
            return Ok(EffectState::Applied);
        }
        Err(error) => return Err(error),
    };
    let ParentLookup::Open(parent) = parent else {
        return Ok(match plan.operation {
            MountOperation::Create => EffectState::NotApplied,
            MountOperation::Remove if plan.target_observation == "absent" => EffectState::Applied,
            MountOperation::Remove => {
                EffectState::Ambiguous("Mount 父目录在移除事务中消失".to_owned())
            }
        });
    };
    ensure_parent_matches_target(&parent, plan)?;
    let target = snapshot_at(
        &parent.parent,
        OsStr::new(&plan.skill_name),
        &plan.expected_target,
    )?;
    let temporary = snapshot_at(
        &parent.parent,
        OsStr::new(&journal.temporary_name),
        &plan.expected_target,
    )?;
    match plan.operation {
        MountOperation::Create => match (&target.kind, &temporary.kind) {
            (TargetKind::ExpectedLink, TargetKind::Absent) => Ok(EffectState::Applied),
            (TargetKind::Absent, TargetKind::Absent | TargetKind::ExpectedLink) => {
                Ok(EffectState::NotApplied)
            }
            _ => Ok(EffectState::Ambiguous(
                "Mount 创建目标或临时链接被外部修改".to_owned(),
            )),
        },
        MountOperation::Remove => {
            let original_kind = observation_kind(&plan.target_observation);
            match original_kind {
                TargetKind::ExpectedLink => {
                    if target.observation == plan.target_observation
                        && temporary.kind == TargetKind::Absent
                    {
                        Ok(EffectState::NotApplied)
                    } else if target.kind == TargetKind::Absent
                        && ((temporary.kind == TargetKind::ExpectedLink
                            && temporary.observation == plan.target_observation)
                            || temporary.kind == TargetKind::Absent)
                    {
                        Ok(EffectState::Applied)
                    } else {
                        Ok(EffectState::Ambiguous(
                            "Mount 移除目标或隔离链接被外部修改".to_owned(),
                        ))
                    }
                }
                TargetKind::Absent => {
                    if target.kind == TargetKind::Absent && temporary.kind == TargetKind::Absent {
                        Ok(EffectState::Applied)
                    } else {
                        Ok(EffectState::Ambiguous(
                            "原本缺失的 Mount 路径出现了新内容".to_owned(),
                        ))
                    }
                }
                TargetKind::Other => {
                    if target.observation == plan.target_observation
                        && temporary.kind == TargetKind::Absent
                    {
                        Ok(EffectState::Applied)
                    } else {
                        Ok(EffectState::Ambiguous(
                            "冲突 Mount 路径在确认后再次变化".to_owned(),
                        ))
                    }
                }
            }
        }
    }
}

fn cleanup_unapplied_temporary(
    paths: &ApplicationPaths,
    plan: &StoredMountPlan,
    journal: &MountJournal,
) -> Result<(), MountLifecycleError> {
    cleanup_temporary(paths, plan, journal, false)
}

fn cleanup_applied_temporary(
    paths: &ApplicationPaths,
    plan: &StoredMountPlan,
    journal: &MountJournal,
) -> Result<(), MountLifecycleError> {
    cleanup_temporary(paths, plan, journal, true)
}

fn cleanup_temporary(
    paths: &ApplicationPaths,
    plan: &StoredMountPlan,
    journal: &MountJournal,
    applied: bool,
) -> Result<(), MountLifecycleError> {
    let project = stored_plan_project(plan)?;
    let parent = match open_mount_parent(paths, plan.app_id, plan.scope, project.as_ref(), false) {
        Ok(parent) => parent,
        Err(error)
            if plan.operation == MountOperation::Remove
                && matches_unavailable_removal_observation(plan, &error) =>
        {
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let ParentLookup::Open(parent) = parent else {
        return Ok(());
    };
    let name = OsStr::new(&journal.temporary_name);
    let temporary = snapshot_at(&parent.parent, name, &plan.expected_target)?;
    if temporary.kind == TargetKind::Absent {
        return Ok(());
    }
    if plan.operation == MountOperation::Create
        || temporary.kind != TargetKind::ExpectedLink
        || temporary.observation != plan.target_observation
    {
        return Err(MountLifecycleError::RecoveryBlocked(
            "Mount 临时路径被未知内容占用".to_owned(),
        ));
    }
    if plan.operation == MountOperation::Remove && !applied {
        return Err(MountLifecycleError::RecoveryBlocked(
            "待移除 Mount 已被隔离，不能按生效前状态清理".to_owned(),
        ));
    }
    unlink_at(&parent.parent, name, false)
        .map_err(|source| mount_io("清理 Mount 临时链接", &parent.parent_path, source))?;
    parent
        .parent
        .sync_all()
        .map_err(|source| mount_io("同步 Mount 临时清理", &parent.parent_path, source))?;
    Ok(())
}

fn validate_plan_contract(
    paths: &ApplicationPaths,
    storage: &Storage,
    plan: &StoredMountPlan,
) -> Result<(), MountLifecycleError> {
    ensure_single_component(&plan.id)?;
    ensure_single_component(&plan.mount_id)?;
    ensure_single_component(&plan.member_id)?;
    ensure_single_component(&plan.skill_name)?;
    ensure_supported_scope(plan.scope, plan.project_id.as_deref())?;
    validate_operation_observation(plan)?;
    let member = storage.read_managed_member(&plan.member_id)?;
    if member.bundle_id != plan.bundle_id
        || member.skill_name != plan.skill_name
        || member.content_fingerprint != plan.member_fingerprint
        || member.expected_target != plan.expected_target
    {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    validate_member_content(&member)?;
    let project = read_scope_project(storage, plan.scope, plan.project_id.as_deref())?;
    if !same_project_identity(project.as_ref(), stored_plan_project(plan)?.as_ref()) {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    if !is_unavailable_removal_plan(plan) {
        validate_project_identity(project.as_ref())?;
    }
    let expected_target_path =
        derive_target_path(paths, &member, plan.app_id, plan.scope, project.as_ref())?;
    if path_to_string(&expected_target_path)? != plan.target_path {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    Ok(())
}

fn validate_plan_member_only(
    _paths: &ApplicationPaths,
    plan: &StoredMountPlan,
) -> Result<(), MountLifecycleError> {
    let validated = validate_single_skill_folder(Path::new(&plan.expected_target))?;
    if validated.name != plan.skill_name || validated.fingerprint != plan.member_fingerprint {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    Ok(())
}

fn validate_member_content(member: &StoredManagedMember) -> Result<(), MountLifecycleError> {
    let validated = validate_single_skill_folder(Path::new(&member.expected_target))?;
    if validated.name != member.skill_name || validated.fingerprint != member.content_fingerprint {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    Ok(())
}

fn ensure_target_observation(
    paths: &ApplicationPaths,
    storage: &Storage,
    plan: &StoredMountPlan,
) -> Result<(), MountLifecycleError> {
    let project = read_scope_project(storage, plan.scope, plan.project_id.as_deref())?;
    let snapshot = match plan.operation {
        MountOperation::Create => observe_target(
            paths,
            plan.app_id,
            plan.scope,
            project.as_ref(),
            &plan.skill_name,
            &plan.expected_target,
        )?,
        MountOperation::Remove => observe_removal_target(
            paths,
            plan.app_id,
            plan.scope,
            project.as_ref(),
            &plan.skill_name,
            &plan.expected_target,
        )?,
    };
    if snapshot.observation == plan.target_observation {
        Ok(())
    } else {
        Err(MountLifecycleError::PlanPreconditionChanged)
    }
}

fn consumed_plan(plan: &StoredMountPlan) -> StoredMountPlan {
    let mut consumed = plan.clone();
    consumed.status = "consumed".to_owned();
    consumed
}

fn read_scope_project(
    storage: &Storage,
    scope: MountScope,
    project_id: Option<&str>,
) -> Result<Option<StoredProject>, MountLifecycleError> {
    match (scope, project_id) {
        (MountScope::Global, None) => Ok(None),
        (MountScope::Project, Some(project_id)) => Ok(Some(storage.read_project(project_id)?)),
        _ => Err(MountLifecycleError::InvalidScope),
    }
}

fn stored_plan_project(
    plan: &StoredMountPlan,
) -> Result<Option<StoredProject>, MountLifecycleError> {
    match plan.scope {
        MountScope::Global
            if plan.project_id.is_none()
                && plan.project_root_path.is_none()
                && plan.project_display_name.is_none()
                && plan.project_root_device.is_none()
                && plan.project_root_inode.is_none() =>
        {
            Ok(None)
        }
        MountScope::Project => Ok(Some(StoredProject {
            id: plan
                .project_id
                .clone()
                .ok_or(MountLifecycleError::InvalidScope)?,
            display_name: plan
                .project_display_name
                .clone()
                .ok_or(MountLifecycleError::InvalidScope)?,
            root_path: plan
                .project_root_path
                .clone()
                .ok_or(MountLifecycleError::InvalidScope)?,
            root_device: plan
                .project_root_device
                .ok_or(MountLifecycleError::InvalidScope)?,
            root_inode: plan
                .project_root_inode
                .ok_or(MountLifecycleError::InvalidScope)?,
            created_at: 0,
        })),
        _ => Err(MountLifecycleError::InvalidScope),
    }
}

fn stored_mount_project(mount: &StoredMount) -> Result<Option<StoredProject>, MountLifecycleError> {
    match mount.scope {
        MountScope::Global
            if mount.project_id.is_none()
                && mount.project_root_path.is_none()
                && mount.project_display_name.is_none()
                && mount.project_root_device.is_none()
                && mount.project_root_inode.is_none() =>
        {
            Ok(None)
        }
        MountScope::Project => Ok(Some(StoredProject {
            id: mount
                .project_id
                .clone()
                .ok_or(MountLifecycleError::InvalidScope)?,
            display_name: mount
                .project_display_name
                .clone()
                .ok_or(MountLifecycleError::InvalidScope)?,
            root_path: mount
                .project_root_path
                .clone()
                .ok_or(MountLifecycleError::InvalidScope)?,
            root_device: mount
                .project_root_device
                .ok_or(MountLifecycleError::InvalidScope)?,
            root_inode: mount
                .project_root_inode
                .ok_or(MountLifecycleError::InvalidScope)?,
            created_at: 0,
        })),
        _ => Err(MountLifecycleError::InvalidScope),
    }
}

fn validate_project_identity(project: Option<&StoredProject>) -> Result<(), MountLifecycleError> {
    let Some(project) = project else {
        return Ok(());
    };
    let path = Path::new(&project.root_path);
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| mount_io("检查已登记 Project", path, source))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.dev() != project.root_device
        || metadata.ino() != project.root_inode
    {
        return Err(MountLifecycleError::ProjectChanged(
            project.root_path.clone(),
        ));
    }
    Ok(())
}

fn derive_target_path(
    paths: &ApplicationPaths,
    member: &StoredManagedMember,
    app_id: SupportedAppId,
    scope: MountScope,
    project: Option<&StoredProject>,
) -> Result<PathBuf, MountLifecycleError> {
    let config = app_config(paths, app_id)?;
    let root = match (scope, project) {
        (MountScope::Global, None) => config.global_root,
        (MountScope::Project, Some(project)) => {
            Path::new(&project.root_path).join(config.project_relative_root)
        }
        _ => return Err(MountLifecycleError::InvalidScope),
    };
    Ok(root.join(&member.skill_name))
}

fn ensure_supported_scope(
    scope: MountScope,
    project_id: Option<&str>,
) -> Result<(), MountLifecycleError> {
    match (scope, project_id) {
        (MountScope::Global, None) | (MountScope::Project, Some(_)) => Ok(()),
        _ => Err(MountLifecycleError::InvalidScope),
    }
}

fn app_config(
    paths: &ApplicationPaths,
    app_id: SupportedAppId,
) -> Result<crate::paths::SupportedAppPathConfig, MountLifecycleError> {
    paths
        .supported_apps()
        .into_iter()
        .find(|config| config.id == app_id)
        .ok_or(MountLifecycleError::UnsupportedApp)
}

fn observe_target(
    paths: &ApplicationPaths,
    app_id: SupportedAppId,
    scope: MountScope,
    project: Option<&StoredProject>,
    skill_name: &str,
    expected_target: &str,
) -> Result<TargetSnapshot, MountLifecycleError> {
    match open_mount_parent(paths, app_id, scope, project, false)? {
        ParentLookup::Missing => Ok(TargetSnapshot {
            kind: TargetKind::Absent,
            observation: "absent".to_owned(),
        }),
        ParentLookup::Open(parent) => {
            let snapshot = snapshot_at(&parent.parent, OsStr::new(skill_name), expected_target)?;
            recheck_open_parent(&parent)?;
            Ok(snapshot)
        }
    }
}

fn observe_removal_target(
    paths: &ApplicationPaths,
    app_id: SupportedAppId,
    scope: MountScope,
    project: Option<&StoredProject>,
    skill_name: &str,
    expected_target: &str,
) -> Result<TargetSnapshot, MountLifecycleError> {
    match observe_target(paths, app_id, scope, project, skill_name, expected_target) {
        Ok(snapshot) => Ok(snapshot),
        Err(MountLifecycleError::UnsafeMountPath(path)) => Ok(TargetSnapshot {
            kind: TargetKind::Other,
            observation: unsafe_parent_observation(&path),
        }),
        Err(error @ (MountLifecycleError::ProjectChanged(_) | MountLifecycleError::Io { .. })) => {
            Ok(TargetSnapshot {
                kind: TargetKind::Other,
                observation: unavailable_parent_observation(&error),
            })
        }
        Err(error) => Err(error),
    }
}

fn open_mount_parent(
    paths: &ApplicationPaths,
    app_id: SupportedAppId,
    scope: MountScope,
    project: Option<&StoredProject>,
    create_missing: bool,
) -> Result<ParentLookup, MountLifecycleError> {
    let config = app_config(paths, app_id)?;
    let (base_path, relative) = match (scope, project) {
        (MountScope::Global, None) => {
            let relative = config.global_root.strip_prefix(paths.home()).map_err(|_| {
                MountLifecycleError::UnsafeMountPath(config.global_root.display().to_string())
            })?;
            (paths.home().to_path_buf(), relative.to_path_buf())
        }
        (MountScope::Project, Some(project)) => (
            PathBuf::from(&project.root_path),
            config.project_relative_root,
        ),
        _ => return Err(MountLifecycleError::InvalidScope),
    };
    let base = open_real_directory(&base_path)?;
    if let Some(project) = project {
        let metadata = base
            .metadata()
            .map_err(|source| mount_io("检查 Project 句柄", &base_path, source))?;
        if metadata.dev() != project.root_device || metadata.ino() != project.root_inode {
            return Err(MountLifecycleError::ProjectChanged(
                project.root_path.clone(),
            ));
        }
    }
    let mut parent = base
        .try_clone()
        .map_err(|source| mount_io("保留 Mount 根目录", &base_path, source))?;
    let mut parent_path = base_path.clone();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(MountLifecycleError::UnsafeMountPath(
                relative.display().to_string(),
            ));
        };
        let child_path = parent_path.join(name);
        match entry_metadata_at(&parent, name)
            .map_err(|source| mount_io("检查 Mount 父目录", &child_path, source))?
        {
            None if !create_missing => return Ok(ParentLookup::Missing),
            None => match mkdir_at(&parent, name, 0o700) {
                Ok(()) => parent
                    .sync_all()
                    .map_err(|source| mount_io("同步 Mount 父目录", &parent_path, source))?,
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(mount_io("创建 Mount 父目录", &child_path, source));
                }
            },
            Some(metadata) if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR => {}
            Some(_) => {
                return Err(MountLifecycleError::UnsafeMountPath(
                    child_path.display().to_string(),
                ));
            }
        }
        parent = open_directory_at(&parent, name)
            .map_err(|source| mount_io("安全打开 Mount 父目录", &child_path, source))?;
        parent_path = child_path;
    }
    Ok(ParentLookup::Open(OpenMountParent {
        base,
        base_path,
        parent,
        parent_path,
    }))
}

fn open_real_directory(path: &Path) -> Result<File, MountLifecycleError> {
    let visible =
        fs::symlink_metadata(path).map_err(|source| mount_io("检查 Mount 根目录", path, source))?;
    if visible.file_type().is_symlink() || !visible.is_dir() {
        return Err(MountLifecycleError::UnsafeMountPath(
            path.display().to_string(),
        ));
    }
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|source| mount_io("安全打开 Mount 根目录", path, source))?;
    let metadata = opened
        .metadata()
        .map_err(|source| mount_io("检查 Mount 根目录句柄", path, source))?;
    if visible.dev() != metadata.dev() || visible.ino() != metadata.ino() {
        return Err(MountLifecycleError::UnsafeMountPath(
            path.display().to_string(),
        ));
    }
    Ok(opened)
}

fn recheck_open_parent(parent: &OpenMountParent) -> Result<(), MountLifecycleError> {
    let visible = fs::symlink_metadata(&parent.base_path)
        .map_err(|source| mount_io("重新检查 Mount 根目录", &parent.base_path, source))?;
    let opened = parent
        .base
        .metadata()
        .map_err(|source| mount_io("重新检查 Mount 根目录句柄", &parent.base_path, source))?;
    if visible.file_type().is_symlink()
        || !visible.is_dir()
        || visible.dev() != opened.dev()
        || visible.ino() != opened.ino()
    {
        return Err(MountLifecycleError::UnsafeMountPath(
            parent.base_path.display().to_string(),
        ));
    }
    let visible_parent = fs::symlink_metadata(&parent.parent_path)
        .map_err(|source| mount_io("重新检查 Mount 父目录", &parent.parent_path, source))?;
    let opened_parent = parent
        .parent
        .metadata()
        .map_err(|source| mount_io("重新检查 Mount 父目录句柄", &parent.parent_path, source))?;
    if visible_parent.file_type().is_symlink()
        || !visible_parent.is_dir()
        || visible_parent.dev() != opened_parent.dev()
        || visible_parent.ino() != opened_parent.ino()
    {
        return Err(MountLifecycleError::UnsafeMountPath(
            parent.parent_path.display().to_string(),
        ));
    }
    Ok(())
}

fn ensure_parent_matches_target(
    parent: &OpenMountParent,
    plan: &StoredMountPlan,
) -> Result<(), MountLifecycleError> {
    if parent.parent_path.join(&plan.skill_name) == Path::new(&plan.target_path) {
        Ok(())
    } else {
        Err(MountLifecycleError::PlanPreconditionChanged)
    }
}

fn snapshot_at(
    parent: &File,
    name: &OsStr,
    expected_target: &str,
) -> Result<TargetSnapshot, MountLifecycleError> {
    let Some(metadata) = entry_metadata_at(parent, name)
        .map_err(|source| mount_io("检查 Mount 目标", Path::new(name), source))?
    else {
        return Ok(TargetSnapshot {
            kind: TargetKind::Absent,
            observation: "absent".to_owned(),
        });
    };
    let file_type = metadata.st_mode & libc::S_IFMT;
    if file_type == libc::S_IFLNK {
        let target = read_link_at(parent, name)
            .map_err(|source| mount_io("读取 Mount 软链接", Path::new(name), source))?;
        let encoded = hex_bytes(target.as_os_str().as_bytes());
        let kind = if target == Path::new(expected_target) {
            TargetKind::ExpectedLink
        } else {
            TargetKind::Other
        };
        let prefix = if kind == TargetKind::ExpectedLink {
            "expected_symlink"
        } else {
            "other_symlink"
        };
        return Ok(TargetSnapshot {
            kind,
            observation: format!(
                "{prefix}:{}:{}:{}:{encoded}",
                metadata.st_dev, metadata.st_ino, metadata.st_mode
            ),
        });
    }
    Ok(TargetSnapshot {
        kind: TargetKind::Other,
        observation: format!(
            "other:{}:{}:{}:{}:{}",
            metadata.st_dev, metadata.st_ino, metadata.st_mode, metadata.st_nlink, metadata.st_size
        ),
    })
}

fn target_health(snapshot: &TargetSnapshot) -> MountHealth {
    match snapshot.kind {
        TargetKind::ExpectedLink => MountHealth::Healthy,
        TargetKind::Absent => MountHealth::Missing,
        TargetKind::Other => MountHealth::Conflict,
    }
}

fn validate_operation_observation(plan: &StoredMountPlan) -> Result<(), MountLifecycleError> {
    let kind = parse_observation_kind(&plan.target_observation)
        .ok_or(MountLifecycleError::PlanPreconditionChanged)?;
    if plan.operation == MountOperation::Create && kind == TargetKind::Other {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    Ok(())
}

fn observation_kind(observation: &str) -> TargetKind {
    parse_observation_kind(observation).unwrap_or(TargetKind::Other)
}

fn parse_observation_kind(observation: &str) -> Option<TargetKind> {
    if observation == "absent" {
        return Some(TargetKind::Absent);
    }
    let parts = observation.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        [
            prefix @ ("expected_symlink" | "other_symlink"),
            dev,
            inode,
            mode,
            target,
        ] if [dev, inode, mode]
            .into_iter()
            .all(|value| value.parse::<u64>().is_ok())
            && is_nonempty_hex(target) =>
        {
            Some(if *prefix == "expected_symlink" {
                TargetKind::ExpectedLink
            } else {
                TargetKind::Other
            })
        }
        ["other", dev, inode, mode, links, size]
            if [dev, inode, mode, links, size]
                .into_iter()
                .all(|value| value.parse::<u64>().is_ok()) =>
        {
            Some(TargetKind::Other)
        }
        ["unsafe_parent" | "unavailable_parent", evidence] if is_nonempty_hex(evidence) => {
            Some(TargetKind::Other)
        }
        _ => None,
    }
}

fn is_nonempty_hex(value: &str) -> bool {
    !value.is_empty()
        && value.len().is_multiple_of(2)
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn unsafe_parent_observation(path: &str) -> String {
    format!("unsafe_parent:{}", hex_bytes(path.as_bytes()))
}

fn unavailable_parent_observation(error: &MountLifecycleError) -> String {
    format!(
        "unavailable_parent:{}",
        hex_bytes(error.to_string().as_bytes())
    )
}

fn matches_unavailable_removal_observation(
    plan: &StoredMountPlan,
    error: &MountLifecycleError,
) -> bool {
    match error {
        // 兼容 0005 中已经保存的 unsafe_parent Plan。
        MountLifecycleError::UnsafeMountPath(path)
            if plan.target_observation == unsafe_parent_observation(path) =>
        {
            true
        }
        MountLifecycleError::UnsafeMountPath(_)
        | MountLifecycleError::ProjectChanged(_)
        | MountLifecycleError::Io { .. } => {
            plan.target_observation == unavailable_parent_observation(error)
        }
        _ => false,
    }
}

fn is_unavailable_removal_plan(plan: &StoredMountPlan) -> bool {
    plan.purpose == MountPlanPurpose::Remove
        && (plan.target_observation.starts_with("unsafe_parent:")
            || plan.target_observation.starts_with("unavailable_parent:"))
}

fn same_project_identity(left: Option<&StoredProject>, right: Option<&StoredProject>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.id == right.id
                && left.display_name == right.display_name
                && left.root_path == right.root_path
                && left.root_device == right.root_device
                && left.root_inode == right.root_inode
        }
        _ => false,
    }
}

fn ensure_temporary_absent(
    parent: &File,
    name: &OsStr,
    path: &Path,
) -> Result<(), MountLifecycleError> {
    if entry_metadata_at(parent, name)
        .map_err(|source| mount_io("检查 Mount 临时路径", path, source))?
        .is_some()
    {
        Err(MountLifecycleError::UnsafeMountPath(
            path.join(name).display().to_string(),
        ))
    } else {
        Ok(())
    }
}

fn build_journal(transaction_id: &str, plan: &StoredMountPlan) -> MountJournal {
    let prefix = match plan.operation {
        MountOperation::Create => ".skillyard-create-",
        MountOperation::Remove => ".skillyard-remove-",
    };
    MountJournal {
        version: MOUNT_JOURNAL_VERSION,
        transaction_id: transaction_id.to_owned(),
        plan_id: plan.id.clone(),
        mount_id: plan.mount_id.clone(),
        operation: plan.operation,
        target_path: plan.target_path.clone(),
        expected_target: plan.expected_target.clone(),
        target_observation: plan.target_observation.clone(),
        temporary_name: format!("{prefix}{transaction_id}"),
        phase: MountJournalPhase::JournalReady,
    }
}

fn validate_journal(
    actual: &MountJournal,
    transaction: &StoredMountTransaction,
    plan: &StoredMountPlan,
) -> Result<(), MountLifecycleError> {
    let mut expected = build_journal(&transaction.id, plan);
    expected.phase = actual.phase;
    if actual.version == MOUNT_JOURNAL_VERSION
        && actual == &expected
        && transaction.plan_id == plan.id
        && transaction.mount_id == plan.mount_id
    {
        Ok(())
    } else {
        Err(MountLifecycleError::RecoveryBlocked(
            "SQLite、Mount Plan 与 Journal 不一致".to_owned(),
        ))
    }
}

fn ensure_journal_fits(journal: &MountJournal) -> Result<(), MountLifecycleError> {
    for phase in [
        MountJournalPhase::JournalReady,
        MountJournalPhase::TargetApplied,
        MountJournalPhase::StateCommitted,
    ] {
        let mut candidate = journal.clone();
        candidate.phase = phase;
        if serde_json::to_vec_pretty(&candidate)?.len() > MAX_MOUNT_JOURNAL_BYTES {
            return Err(MountLifecycleError::JournalTooLarge);
        }
    }
    Ok(())
}

fn write_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    journal: &MountJournal,
) -> Result<(), MountLifecycleError> {
    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    let name = OsString::from(format!("mount-{}.json", journal.transaction_id));
    let bytes = serde_json::to_vec_pretty(journal)?;
    if bytes.len() > MAX_MOUNT_JOURNAL_BYTES {
        return Err(MountLifecycleError::JournalTooLarge);
    }
    write_atomic_at(&journals, &name, &paths.journals_root().join(&name), &bytes)?;
    Ok(())
}

fn read_journal(
    journals: &File,
    name: &OsStr,
    path: &Path,
) -> Result<MountJournal, MountLifecycleError> {
    let mut file = open_regular_file_at(journals, name, path, false)?;
    let metadata = file
        .metadata()
        .map_err(|source| mount_io("检查 Mount Journal", path, source))?;
    if metadata.len() > MAX_MOUNT_JOURNAL_BYTES as u64 {
        return Err(MountLifecycleError::JournalTooLarge);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize + 1);
    Read::by_ref(&mut file)
        .take(MAX_MOUNT_JOURNAL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| mount_io("读取 Mount Journal", path, source))?;
    if bytes.len() > MAX_MOUNT_JOURNAL_BYTES {
        return Err(MountLifecycleError::JournalTooLarge);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn remove_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    journal: &MountJournal,
) -> Result<(), MountLifecycleError> {
    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    let name = OsString::from(format!("mount-{}.json", journal.transaction_id));
    match unlink_at(&journals, &name, false) {
        Ok(()) => journals
            .sync_all()
            .map_err(|source| mount_io("同步 Mount Journal 清理", &paths.journals_root(), source)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(mount_io(
            "清理 Mount Journal",
            &paths.journals_root().join(name),
            source,
        )),
    }
}

fn stored_plan_to_ui(
    plan: &StoredMountPlan,
    health: MountHealth,
) -> Result<MountPlan, MountLifecycleError> {
    Ok(MountPlan {
        id: plan.id.clone(),
        operation: plan.operation,
        purpose: plan.purpose,
        mount_id: plan.mount_id.clone(),
        member_id: plan.member_id.clone(),
        skill_name: plan.skill_name.clone(),
        app_id: plan.app_id,
        scope: plan.scope,
        project_id: plan.project_id.clone(),
        project_display_name: plan.project_display_name.clone(),
        target_path: plan.target_path.clone(),
        expected_target: plan.expected_target.clone(),
        target_health: health,
        created_at: plan.created_at,
        expires_at: plan.expires_at,
    })
}

fn ensure_single_component(value: &str) -> Result<(), MountLifecycleError> {
    let mut components = Path::new(value).components();
    let valid = matches!(components.next(), Some(Component::Normal(component)) if component == OsStr::new(value))
        && components.next().is_none();
    if valid {
        Ok(())
    } else {
        Err(MountLifecycleError::RecoveryBlocked(
            "SQLite 中的 Mount 标识不是安全单路径名称".to_owned(),
        ))
    }
}

fn path_to_string(path: &Path) -> Result<String, MountLifecycleError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| MountLifecycleError::UnsafeMountPath(path.display().to_string()))
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("写入 String 不会失败");
    }
    encoded
}

fn inject_interruption(
    actual: LifecycleFailpoint,
    simulated: LifecycleFailpoint,
    hard_exit: LifecycleFailpoint,
    message: &'static str,
) -> Result<(), MountLifecycleError> {
    if actual == simulated {
        return Err(MountLifecycleError::SimulatedInterruption(message));
    }
    if actual == hard_exit {
        unsafe { libc::_exit(92) }
    }
    Ok(())
}

fn inject_hard_exit(actual: LifecycleFailpoint, expected: LifecycleFailpoint) {
    if actual == expected {
        unsafe { libc::_exit(92) }
    }
}

fn mount_io(action: &'static str, path: &Path, source: io::Error) -> MountLifecycleError {
    MountLifecycleError::Io {
        action,
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn remove_leaf_swap_restores_unknown_content_instead_of_deleting_it() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let parent_path = sandbox.path().join("skills");
        let expected_target = sandbox.path().join("managed-skill");
        fs::create_dir(&parent_path).expect("应创建 Mount 父目录");
        fs::create_dir(&expected_target).expect("应创建受管目标");
        let leaf = OsStr::new("example-skill");
        symlink(&expected_target, parent_path.join(leaf)).expect("应创建原 Mount");
        let parent_file = open_real_directory(&parent_path).expect("应打开 Mount 父目录");
        let original = snapshot_at(
            &parent_file,
            leaf,
            expected_target.to_str().expect("测试路径应是 UTF-8"),
        )
        .expect("应记录原 Mount 身份");

        // 模拟 snapshot 与 rename 之间，外部把原软链接替换成普通文件。
        fs::remove_file(parent_path.join(leaf)).expect("应移除原 Mount");
        fs::write(parent_path.join(leaf), "external").expect("应写入外部文件");
        let parent = OpenMountParent {
            base: parent_file.try_clone().expect("应保留父目录句柄"),
            base_path: parent_path.clone(),
            parent: parent_file,
            parent_path: parent_path.clone(),
        };
        let plan = StoredMountPlan {
            id: "plan".to_owned(),
            operation: MountOperation::Remove,
            purpose: MountPlanPurpose::Remove,
            mount_id: "mount".to_owned(),
            member_id: "member".to_owned(),
            bundle_id: "bundle".to_owned(),
            skill_name: "example-skill".to_owned(),
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
            project_display_name: None,
            project_root_path: None,
            project_root_device: None,
            project_root_inode: None,
            target_path: parent_path.join(leaf).to_string_lossy().into_owned(),
            expected_target: expected_target.to_string_lossy().into_owned(),
            member_fingerprint: "sha256:test".to_owned(),
            target_observation: original.observation,
            created_at: 1,
            expires_at: 2,
            status: "pending".to_owned(),
        };
        let temporary_name = OsStr::new(".skillyard-remove-test");

        let error = quarantine_mount_for_removal(&parent, leaf, temporary_name, &plan)
            .expect_err("被替换的叶子不能作为受管 Mount 删除");
        assert!(matches!(
            error,
            MountLifecycleError::PlanPreconditionChanged
        ));
        assert_eq!(
            fs::read_to_string(parent_path.join(leaf)).unwrap(),
            "external"
        );
        assert!(!parent_path.join(temporary_name).exists());
    }
}
