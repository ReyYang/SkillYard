use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    content::{ContentValidationError, validate_single_skill_folder},
    domain::{
        BatchMountDisposition, BatchMountPlan, BatchMountPlanItem, BatchMountRequest, MountHealth,
        MountOperation, MountPlan, MountPlanPurpose, MountScope, SupportedAppId,
    },
    lifecycle::{
        LifecycleError, LifecycleFailpoint, LifecycleLock, acquire_lifecycle_lock,
        entry_metadata_at, mkdir_at, open_directory_at, open_managed_directory_from_root,
        read_link_at, rename_at_no_replace, symlink_at, unlink_at, write_notice_from_storage,
    },
    paths::ApplicationPaths,
    storage::{
        NewBatchMountPlan, NewBatchMountPlanItem, NewMountPlan, NewProject, Storage, StorageError,
        StoredBatchMountPlan, StoredBatchMountPlanItem, StoredBatchMountTransaction,
        StoredManagedMember, StoredMount, StoredMountPlan, StoredMountTransaction, StoredProject,
    },
    transaction::{self, JournalIoError, journal_file_name},
};

const MOUNT_PLAN_TTL_MILLIS: i64 = 30 * 60 * 1_000;
const MOUNT_JOURNAL_VERSION: u32 = 1;
const MAX_MOUNT_JOURNAL_BYTES: usize = 64 * 1024;
const BATCH_MOUNT_JOURNAL_VERSION: u32 = 1;
const MAX_BATCH_MOUNT_JOURNAL_BYTES: usize = 1024 * 1024;

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
    #[error("Bundle 批量 Mount Plan 无效：{0}")]
    InvalidBatchMountPlan(String),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum BatchMountJournalPhase {
    JournalReady,
    Applying,
    TargetsApplied,
    RollingBack,
    StateCommitted,
}

impl BatchMountJournalPhase {
    fn as_storage_str(self) -> &'static str {
        match self {
            Self::JournalReady => "journal_ready",
            Self::Applying => "applying",
            Self::TargetsApplied => "targets_applied",
            Self::RollingBack => "rolling_back",
            Self::StateCommitted => "state_committed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BatchMountJournalItem {
    item_id: String,
    target_observation: String,
    /// 只有本事务确实创建且已经保存精确快照的链接，失败回滚时才允许删除。
    created_by_transaction: bool,
    applied_observation: Option<String>,
    /// 回滚方向下，只有文件系统已经恢复并同步后才持久化这个进度位。
    rolled_back: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BatchMountJournal {
    version: u32,
    transaction_id: String,
    plan_id: String,
    bundle_id: String,
    phase: BatchMountJournalPhase,
    items: Vec<BatchMountJournalItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetKind {
    Absent,
    ExpectedLink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetSnapshot {
    kind: TargetKind,
    observation: String,
}

impl TargetSnapshot {
    pub(crate) fn absent() -> Self {
        Self {
            kind: TargetKind::Absent,
            observation: "absent".to_owned(),
        }
    }

    pub(crate) fn kind(&self) -> TargetKind {
        self.kind
    }

    pub(crate) fn observation(&self) -> &str {
        &self.observation
    }
}

/// 多对象 Removal 复用单 Mount 的路径身份与隔离协议，但不创建第二套 Mount 事务。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedMountRemovalSnapshot {
    pub mount_id: String,
    pub member_id: String,
    pub bundle_id: String,
    pub skill_name: String,
    pub member_fingerprint: String,
    pub app_id: SupportedAppId,
    pub scope: MountScope,
    pub project_id: Option<String>,
    pub project_display_name: Option<String>,
    pub project_root_path: Option<String>,
    pub project_root_device: Option<u64>,
    pub project_root_inode: Option<u64>,
    pub target_path: String,
    pub expected_target: String,
    pub target_observation: String,
    pub temporary_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManagedMountRemovalState {
    Original,
    Isolated,
    Ambiguous(String),
}

struct PreparedBatchMountItem {
    id: String,
    mount_id: String,
    member: StoredManagedMember,
    app_id: SupportedAppId,
    scope: MountScope,
    project: Option<StoredProject>,
    target_path: String,
    snapshot: TargetSnapshot,
    disposition: BatchMountDisposition,
    conflict_reason: Option<String>,
}

/// Plan ID 携带预览内容摘要，确认时不只依赖可被协调改写的 SQLite 字段自洽。
#[derive(Serialize)]
struct BatchMountPlanSeal<'a> {
    bundle_id: &'a str,
    created_at: i64,
    expires_at: i64,
    items: Vec<BatchMountPlanSealItem<'a>>,
}

#[derive(Serialize)]
struct BatchMountPlanSealItem<'a> {
    id: &'a str,
    mount_id: &'a str,
    member_id: &'a str,
    bundle_id: &'a str,
    skill_name: &'a str,
    app_id: SupportedAppId,
    scope: MountScope,
    project_id: Option<&'a str>,
    project_display_name: Option<&'a str>,
    project_root_path: Option<&'a str>,
    project_root_device: Option<u64>,
    project_root_inode: Option<u64>,
    target_path: &'a str,
    expected_target: &'a str,
    member_fingerprint: &'a str,
    target_observation: &'a str,
    disposition: BatchMountDisposition,
    selectable: bool,
    default_selected: bool,
    conflict_reason: Option<&'a str>,
    target_health: MountHealth,
}

#[derive(Debug)]
pub(crate) struct OpenMountParent {
    base: File,
    base_path: PathBuf,
    parent: File,
    parent_path: PathBuf,
}

impl OpenMountParent {
    /// 文件操作必须继续通过这个已经逐组件验证的目录句柄完成。
    pub(crate) fn directory(&self) -> &File {
        &self.parent
    }

    /// 可见路径只用于错误信息和确认计划中的目标仍落在同一父目录。
    pub(crate) fn path(&self) -> &Path {
        &self.parent_path
    }
}

#[derive(Debug)]
pub(crate) enum ParentLookup {
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

pub(crate) fn seal_managed_mount_removal(
    paths: &ApplicationPaths,
    storage: &Storage,
    mount: &StoredMount,
    temporary_name: String,
) -> Result<ManagedMountRemovalSnapshot, MountLifecycleError> {
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
    let observed = observe_removal_target(
        paths,
        mount.app_id,
        mount.scope,
        project.as_ref(),
        &mount.skill_name,
        &mount.expected_target,
    )?;
    Ok(ManagedMountRemovalSnapshot {
        mount_id: mount.id.clone(),
        member_id: mount.member_id.clone(),
        bundle_id: mount.bundle_id.clone(),
        skill_name: mount.skill_name.clone(),
        member_fingerprint: mount.member_fingerprint.clone(),
        app_id: mount.app_id,
        scope: mount.scope,
        project_id: mount.project_id.clone(),
        project_display_name: mount.project_display_name.clone(),
        project_root_path: mount.project_root_path.clone(),
        project_root_device: mount.project_root_device,
        project_root_inode: mount.project_root_inode,
        target_path: mount.target_path.clone(),
        expected_target: mount.expected_target.clone(),
        target_observation: observed.observation,
        temporary_name,
    })
}

pub(crate) fn isolate_managed_mount_removal(
    paths: &ApplicationPaths,
    snapshot: &ManagedMountRemovalSnapshot,
) -> Result<(), MountLifecycleError> {
    let plan = removal_snapshot_plan(snapshot);
    let journal = removal_snapshot_journal(snapshot);
    apply_mount_effect(paths, &plan, &journal)?;
    match inspect_effect_state(paths, &plan, &journal)? {
        EffectState::Applied => Ok(()),
        EffectState::NotApplied => Err(MountLifecycleError::PlanPreconditionChanged),
        EffectState::Ambiguous(message) => Err(MountLifecycleError::RecoveryBlocked(message)),
    }
}

pub(crate) fn inspect_managed_mount_removal(
    paths: &ApplicationPaths,
    snapshot: &ManagedMountRemovalSnapshot,
) -> Result<ManagedMountRemovalState, MountLifecycleError> {
    let plan = removal_snapshot_plan(snapshot);
    let journal = removal_snapshot_journal(snapshot);
    Ok(match inspect_effect_state(paths, &plan, &journal)? {
        EffectState::NotApplied => ManagedMountRemovalState::Original,
        EffectState::Applied => ManagedMountRemovalState::Isolated,
        EffectState::Ambiguous(message) => ManagedMountRemovalState::Ambiguous(message),
    })
}

pub(crate) fn restore_managed_mount_removal(
    paths: &ApplicationPaths,
    snapshot: &ManagedMountRemovalSnapshot,
) -> Result<(), MountLifecycleError> {
    let plan = removal_snapshot_plan(snapshot);
    let project = stored_plan_project(&plan)?;
    let parent = match open_mount_parent(paths, plan.app_id, plan.scope, project.as_ref(), false) {
        Ok(parent) => parent,
        Err(error) if matches_unavailable_removal_observation(&plan, &error) => return Ok(()),
        Err(error) => return Err(error),
    };
    let ParentLookup::Open(parent) = parent else {
        return if plan.target_observation == "absent" {
            Ok(())
        } else {
            Err(MountLifecycleError::RecoveryBlocked(
                "Mount 父目录在 Removal 回滚时消失".to_owned(),
            ))
        };
    };
    ensure_parent_matches_target(&parent, &plan)?;
    let leaf = OsStr::new(&plan.skill_name);
    let temporary_name = OsStr::new(&snapshot.temporary_name);
    let target = snapshot_at(&parent.parent, leaf, &plan.expected_target)?;
    let temporary = snapshot_at(&parent.parent, temporary_name, &plan.expected_target)?;
    match observation_kind(&plan.target_observation) {
        TargetKind::ExpectedLink
            if target.observation == plan.target_observation
                && temporary.kind == TargetKind::Absent =>
        {
            Ok(())
        }
        TargetKind::ExpectedLink
            if target.kind == TargetKind::Absent
                && temporary.observation == plan.target_observation =>
        {
            rename_at_no_replace(&parent.parent, temporary_name, &parent.parent, leaf).map_err(
                |source| mount_io("恢复 Removal 隔离的 Mount", &parent.parent_path, source),
            )?;
            parent
                .parent
                .sync_all()
                .map_err(|source| mount_io("同步 Removal Mount 回滚", &parent.parent_path, source))
        }
        TargetKind::Absent | TargetKind::Other
            if target.observation == plan.target_observation
                && temporary.kind == TargetKind::Absent =>
        {
            Ok(())
        }
        _ => Err(MountLifecycleError::RecoveryBlocked(
            "Removal 回滚时 Mount 或隔离路径已被外部改写".to_owned(),
        )),
    }
}

pub(crate) fn finalize_managed_mount_removal(
    paths: &ApplicationPaths,
    snapshot: &ManagedMountRemovalSnapshot,
) -> Result<(), MountLifecycleError> {
    let plan = removal_snapshot_plan(snapshot);
    let journal = removal_snapshot_journal(snapshot);
    cleanup_applied_temporary(paths, &plan, &journal)
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

/// 只读取 Mount 目标并生成健康快照，调用方决定与哪一份状态原子提交。
pub fn observe_mount_health(
    paths: &ApplicationPaths,
    storage: &Storage,
) -> Result<Vec<(String, MountHealth)>, MountLifecycleError> {
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
    Ok(updates)
}

/// 启动检查可以单独保存；Local Refresh 会把同一快照交给清单事务一起提交。
pub fn refresh_mount_health(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    now: i64,
) -> Result<(), MountLifecycleError> {
    let updates = observe_mount_health(paths, storage)?;
    storage.update_mount_health_batch(&updates, now)?;
    Ok(())
}

pub fn create_batch_mount_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    bundle_id: &str,
    requests: &[BatchMountRequest],
    now: i64,
) -> Result<BatchMountPlan, MountLifecycleError> {
    ensure_single_component(bundle_id)?;
    if requests.is_empty() {
        return Err(MountLifecycleError::InvalidBatchMountPlan(
            "至少选择一个 Mount 目标".to_owned(),
        ));
    }

    let existing_mounts = storage.read_mounts()?;
    let mut request_keys = BTreeSet::new();
    let mut requested_scopes = BTreeMap::<(String, &'static str), (bool, bool)>::new();
    for request in requests {
        ensure_supported_scope(request.scope, request.project_id.as_deref())?;
        let key = format!(
            "{}\0{}\0{}\0{}",
            request.member_id,
            request.app_id.as_str(),
            request.scope.as_str(),
            request.project_id.as_deref().unwrap_or("")
        );
        if !request_keys.insert(key) {
            return Err(MountLifecycleError::InvalidBatchMountPlan(
                "同一个 Mount 目标被重复选择".to_owned(),
            ));
        }
        let scopes = requested_scopes
            .entry((request.member_id.clone(), request.app_id.as_str()))
            .or_default();
        match request.scope {
            MountScope::Global => scopes.0 = true,
            MountScope::Project => scopes.1 = true,
        }
    }

    let mut prepared = Vec::with_capacity(requests.len());
    for request in requests {
        let member = storage.read_managed_member(&request.member_id)?;
        if member.bundle_id != bundle_id {
            return Err(MountLifecycleError::InvalidBatchMountPlan(
                "一次批量 Mount 只能包含同一个 Bundle 的成员".to_owned(),
            ));
        }
        validate_member_content(&member)?;
        let project = read_scope_project(storage, request.scope, request.project_id.as_deref())?;
        validate_project_identity(project.as_ref())?;
        let target_path = derive_target_path(
            paths,
            &member,
            request.app_id,
            request.scope,
            project.as_ref(),
        )?;
        let snapshot = observe_target(
            paths,
            request.app_id,
            request.scope,
            project.as_ref(),
            &member.skill_name,
            &member.expected_target,
        )?;
        let target_path = path_to_string(&target_path)?;
        let blocked_object = storage.is_batch_mount_object_blocked(
            &member.id,
            &target_path,
            request.project_id.as_deref(),
        )?;

        let exact_mount = existing_mounts.iter().any(|mount| {
            mount.member_id == member.id
                && mount.app_id == request.app_id
                && mount.scope == request.scope
                && mount.project_id == request.project_id
                && mount.target_path == target_path
                && mount.expected_target == member.expected_target
        });
        let existing_scope_conflict = existing_mounts.iter().any(|mount| {
            mount.member_id == member.id
                && mount.app_id == request.app_id
                && mount.scope != request.scope
        });
        let requested_scope_conflict = requested_scopes
            .get(&(member.id.clone(), request.app_id.as_str()))
            .is_some_and(|(global, project)| *global && *project);
        let existing_path_conflict = existing_mounts.iter().any(|mount| {
            mount.target_path == target_path
                && !(mount.member_id == member.id
                    && mount.app_id == request.app_id
                    && mount.scope == request.scope
                    && mount.project_id == request.project_id)
        });

        let (disposition, conflict_reason) = if blocked_object {
            (
                BatchMountDisposition::PathConflict,
                Some("相关 Skill 或 Mount 路径正在等待人工恢复".to_owned()),
            )
        } else if exact_mount {
            (BatchMountDisposition::AlreadyMounted, None)
        } else if existing_scope_conflict || requested_scope_conflict {
            (
                BatchMountDisposition::ScopeConflict,
                Some("同一 Skill 在同一应用中不能同时使用 global 与 project".to_owned()),
            )
        } else if existing_path_conflict || snapshot.kind == TargetKind::Other {
            (
                BatchMountDisposition::PathConflict,
                Some("Mount 目标路径已被其他内容占用".to_owned()),
            )
        } else {
            (BatchMountDisposition::Ready, None)
        };
        prepared.push(PreparedBatchMountItem {
            id: Uuid::new_v4().to_string(),
            mount_id: Uuid::new_v4().to_string(),
            member,
            app_id: request.app_id,
            scope: request.scope,
            project,
            target_path,
            snapshot,
            disposition,
            conflict_reason,
        });
    }

    // 相同叶子即使来自异常数据库内容也不能让两个批量项争用同一路径。
    let mut target_counts = BTreeMap::<String, usize>::new();
    for item in &prepared {
        *target_counts.entry(item.target_path.clone()).or_default() += 1;
    }
    for item in &mut prepared {
        if target_counts.get(&item.target_path).copied().unwrap_or(0) > 1
            && item.disposition != BatchMountDisposition::AlreadyMounted
        {
            item.disposition = BatchMountDisposition::PathConflict;
            item.conflict_reason = Some("批量计划中的多个成员使用同一个 Mount 路径".to_owned());
        }
    }

    let created_at = now;
    let expires_at = now.saturating_add(MOUNT_PLAN_TTL_MILLIS);
    let seal = batch_plan_seal_for_prepared(bundle_id, created_at, expires_at, &prepared)?;
    let plan_id = format!("batch-{}-{seal}", Uuid::new_v4());
    let new_items = prepared
        .iter()
        .map(|item| NewBatchMountPlanItem {
            id: &item.id,
            mount_id: &item.mount_id,
            member_id: &item.member.id,
            app_id: item.app_id,
            scope: item.scope,
            project_id: item.project.as_ref().map(|project| project.id.as_str()),
            target_path: &item.target_path,
            expected_target: &item.member.expected_target,
            member_fingerprint: &item.member.content_fingerprint,
            target_observation: &item.snapshot.observation,
            disposition: item.disposition,
            selectable: item.disposition == BatchMountDisposition::Ready,
            default_selected: item.disposition == BatchMountDisposition::Ready,
            conflict_reason: item.conflict_reason.as_deref(),
            target_health: target_health(&item.snapshot),
        })
        .collect::<Vec<_>>();
    storage.save_batch_mount_plan(NewBatchMountPlan {
        id: &plan_id,
        bundle_id,
        items: &new_items,
        created_at,
        expires_at,
    })?;
    let stored = storage.read_batch_mount_plan(&plan_id)?;
    stored_batch_plan_to_ui(&stored)
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

pub fn confirm_batch_mount_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
    selected_item_ids: &[String],
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), MountLifecycleError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    let preview = storage.read_batch_mount_plan(plan_id)?;
    validate_batch_selection(&preview, selected_item_ids, now)?;
    validate_batch_plan_contract(paths, storage, &preview)?;
    ensure_batch_target_observations(paths, &preview, selected_item_ids)?;
    // Batch 可能跨多个 Host；在首个 Skill 路径写入前先证明回滚隔离可以使用原子 rename。
    preflight_batch_mount_devices(paths, &lifecycle_lock, storage, &preview, selected_item_ids)?;

    let transaction_id = Uuid::new_v4().to_string();
    let journal_relative = format!("journals/mount-batch-{transaction_id}.json");
    let plan = storage.begin_batch_mount_transaction(
        plan_id,
        selected_item_ids,
        &transaction_id,
        &journal_relative,
        now,
    )?;
    if let Err(error) = validate_consumed_batch_plan(&preview, &plan, selected_item_ids) {
        storage.abort_batch_mount_transaction(&transaction_id, Some(&error.to_string()), now)?;
        storage.forget_terminal_batch_mount_transaction(&transaction_id)?;
        return Err(error);
    }
    let mut journal = match build_batch_journal(&transaction_id, &plan) {
        Ok(journal) => journal,
        Err(error) => {
            storage.abort_batch_mount_transaction(
                &transaction_id,
                Some(&error.to_string()),
                now,
            )?;
            storage.forget_terminal_batch_mount_transaction(&transaction_id)?;
            return Err(error);
        }
    };
    if let Err(error) = ensure_batch_journal_fits(&journal, &plan) {
        storage.abort_batch_mount_transaction(&transaction_id, Some(&error.to_string()), now)?;
        storage.forget_terminal_batch_mount_transaction(&transaction_id)?;
        return Err(error);
    }
    lifecycle_lock.recheck(paths)?;

    let result = execute_batch_mount(
        paths,
        &lifecycle_lock,
        storage,
        &plan,
        &mut journal,
        now,
        failpoint,
    );
    if let Err(error) = result {
        handle_batch_mount_error(
            paths,
            &lifecycle_lock,
            storage,
            &plan,
            &mut journal,
            now,
            &error,
            failpoint,
        )?;
        return Err(error);
    }
    cleanup_completed_batch_mount(paths, &lifecycle_lock, storage, &journal)?;
    Ok(())
}

pub fn recover_pending_batch_mount_transactions(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    now: i64,
) -> Result<(), MountLifecycleError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    for transaction in storage.recoverable_batch_mount_transactions()? {
        if transaction.status == "blocked" {
            continue;
        }
        if let Err(error) =
            recover_batch_mount_transaction(paths, &lifecycle_lock, storage, &transaction, now)
        {
            storage.block_batch_mount_transaction(&transaction.id, &error.to_string(), now)?;
        }
        lifecycle_lock.recheck(paths)?;
    }
    Ok(())
}

fn validate_batch_selection(
    plan: &StoredBatchMountPlan,
    selected_item_ids: &[String],
    now: i64,
) -> Result<(), MountLifecycleError> {
    if plan.status != "pending" {
        return Err(StorageError::BatchMountPlanConsumed.into());
    }
    if plan.expires_at <= now {
        return Err(StorageError::BatchMountPlanExpired.into());
    }
    let selected = selected_item_ids.iter().collect::<BTreeSet<_>>();
    if selected.is_empty() || selected.len() != selected_item_ids.len() {
        return Err(MountLifecycleError::InvalidBatchMountPlan(
            "确认集合不能为空或包含重复项".to_owned(),
        ));
    }
    for item_id in selected {
        let Some(item) = plan.items.iter().find(|item| &item.id == item_id) else {
            return Err(MountLifecycleError::InvalidBatchMountPlan(
                "确认集合包含不属于当前 Plan 的项目".to_owned(),
            ));
        };
        if item.disposition != BatchMountDisposition::Ready || !item.selectable {
            return Err(MountLifecycleError::InvalidBatchMountPlan(
                "只有 Ready 项可以进入确认集合".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_batch_plan_contract(
    paths: &ApplicationPaths,
    storage: &Storage,
    plan: &StoredBatchMountPlan,
) -> Result<(), MountLifecycleError> {
    ensure_single_component(&plan.id)?;
    ensure_single_component(&plan.bundle_id)?;
    validate_batch_plan_seal(plan)?;
    if plan.items.is_empty() {
        return Err(MountLifecycleError::InvalidBatchMountPlan(
            "批量 Plan 不包含任何项目".to_owned(),
        ));
    }
    let mut item_ids = BTreeSet::new();
    let mut mount_ids = BTreeSet::new();
    for item in &plan.items {
        if !item_ids.insert(item.id.as_str()) || !mount_ids.insert(item.mount_id.as_str()) {
            return Err(MountLifecycleError::InvalidBatchMountPlan(
                "批量 Plan 包含重复身份".to_owned(),
            ));
        }
        let member = storage.read_managed_member(&item.member_id)?;
        if member.bundle_id != plan.bundle_id
            || item.bundle_id != plan.bundle_id
            || member.skill_name != item.skill_name
            || member.content_fingerprint != item.member_fingerprint
            || member.expected_target != item.expected_target
        {
            return Err(MountLifecycleError::PlanPreconditionChanged);
        }
        validate_member_content(&member)?;
        let project = batch_item_project(storage, item)?;
        validate_project_identity(project.as_ref())?;
        let expected_path =
            derive_target_path(paths, &member, item.app_id, item.scope, project.as_ref())?;
        if path_to_string(&expected_path)? != item.target_path
            || parse_observation_kind(&item.target_observation).is_none()
        {
            return Err(MountLifecycleError::PlanPreconditionChanged);
        }
    }
    Ok(())
}

fn batch_plan_seal_for_prepared(
    bundle_id: &str,
    created_at: i64,
    expires_at: i64,
    items: &[PreparedBatchMountItem],
) -> Result<String, MountLifecycleError> {
    let seal = BatchMountPlanSeal {
        bundle_id,
        created_at,
        expires_at,
        items: items
            .iter()
            .map(|item| BatchMountPlanSealItem {
                id: &item.id,
                mount_id: &item.mount_id,
                member_id: &item.member.id,
                bundle_id: &item.member.bundle_id,
                skill_name: &item.member.skill_name,
                app_id: item.app_id,
                scope: item.scope,
                project_id: item.project.as_ref().map(|project| project.id.as_str()),
                project_display_name: item
                    .project
                    .as_ref()
                    .map(|project| project.display_name.as_str()),
                project_root_path: item
                    .project
                    .as_ref()
                    .map(|project| project.root_path.as_str()),
                project_root_device: item.project.as_ref().map(|project| project.root_device),
                project_root_inode: item.project.as_ref().map(|project| project.root_inode),
                target_path: &item.target_path,
                expected_target: &item.member.expected_target,
                member_fingerprint: &item.member.content_fingerprint,
                target_observation: &item.snapshot.observation,
                disposition: item.disposition,
                selectable: item.disposition == BatchMountDisposition::Ready,
                default_selected: item.disposition == BatchMountDisposition::Ready,
                conflict_reason: item.conflict_reason.as_deref(),
                target_health: target_health(&item.snapshot),
            })
            .collect(),
    };
    hash_batch_plan_seal(&seal)
}

fn batch_plan_seal_for_stored(plan: &StoredBatchMountPlan) -> Result<String, MountLifecycleError> {
    let seal = BatchMountPlanSeal {
        bundle_id: &plan.bundle_id,
        created_at: plan.created_at,
        expires_at: plan.expires_at,
        items: plan
            .items
            .iter()
            .map(|item| BatchMountPlanSealItem {
                id: &item.id,
                mount_id: &item.mount_id,
                member_id: &item.member_id,
                bundle_id: &item.bundle_id,
                skill_name: &item.skill_name,
                app_id: item.app_id,
                scope: item.scope,
                project_id: item.project_id.as_deref(),
                project_display_name: item.project_display_name.as_deref(),
                project_root_path: item.project_root_path.as_deref(),
                project_root_device: item.project_root_device,
                project_root_inode: item.project_root_inode,
                target_path: &item.target_path,
                expected_target: &item.expected_target,
                member_fingerprint: &item.member_fingerprint,
                target_observation: &item.target_observation,
                disposition: item.disposition,
                selectable: item.selectable,
                default_selected: item.default_selected,
                conflict_reason: item.conflict_reason.as_deref(),
                target_health: item.target_health,
            })
            .collect(),
    };
    hash_batch_plan_seal(&seal)
}

fn hash_batch_plan_seal(seal: &BatchMountPlanSeal<'_>) -> Result<String, MountLifecycleError> {
    let encoded = serde_json::to_vec(seal)?;
    let digest = Sha256::digest(encoded);
    Ok(hex_bytes(&digest))
}

fn validate_batch_plan_seal(plan: &StoredBatchMountPlan) -> Result<(), MountLifecycleError> {
    let Some(encoded_id) = plan.id.strip_prefix("batch-") else {
        return Err(MountLifecycleError::InvalidBatchMountPlan(
            "批量 Plan 缺少预览摘要".to_owned(),
        ));
    };
    let Some((nonce, expected_seal)) = encoded_id.rsplit_once('-') else {
        return Err(MountLifecycleError::InvalidBatchMountPlan(
            "批量 Plan 缺少预览摘要".to_owned(),
        ));
    };
    if Uuid::parse_str(nonce).is_err()
        || expected_seal.len() != 64
        || !expected_seal
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(MountLifecycleError::InvalidBatchMountPlan(
            "批量 Plan 的预览摘要无效".to_owned(),
        ));
    }
    if batch_plan_seal_for_stored(plan)? != expected_seal {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    Ok(())
}

fn ensure_batch_target_observations(
    paths: &ApplicationPaths,
    plan: &StoredBatchMountPlan,
    selected_item_ids: &[String],
) -> Result<(), MountLifecycleError> {
    let selected = selected_item_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    for item in plan
        .items
        .iter()
        .filter(|item| selected.contains(item.id.as_str()))
    {
        let project = stored_batch_item_project(item)?;
        let snapshot = observe_target(
            paths,
            item.app_id,
            item.scope,
            project.as_ref(),
            &item.skill_name,
            &item.expected_target,
        )?;
        if snapshot.observation != item.target_observation {
            return Err(MountLifecycleError::PlanPreconditionChanged);
        }
    }
    Ok(())
}

fn preflight_batch_mount_devices(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &Storage,
    plan: &StoredBatchMountPlan,
    selected_item_ids: &[String],
) -> Result<(), MountLifecycleError> {
    let selected = selected_item_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let staging =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let staging_device = staging
        .metadata()
        .map_err(|source| mount_io("检查受管回滚临时区", &paths.staging_root(), source))?
        .dev();
    let mut checked = 0usize;
    for item in plan
        .items
        .iter()
        .filter(|item| selected.contains(item.id.as_str()))
    {
        let project = batch_item_project(storage, item)?;
        let target_device =
            preflight_mount_parent_device(paths, item.app_id, item.scope, project.as_ref())?;
        if target_device != staging_device {
            return Err(MountLifecycleError::InvalidBatchMountPlan(format!(
                "Mount 目标与 Central Store 不在同一文件系统，无法保证安全回滚：{}",
                item.target_path
            )));
        }
        lifecycle_lock.recheck(paths)?;
        checked += 1;
    }
    if checked != selected.len() {
        return Err(MountLifecycleError::InvalidBatchMountPlan(
            "批量事务包含未知确认项".to_owned(),
        ));
    }
    Ok(())
}

fn preflight_mount_parent_device(
    paths: &ApplicationPaths,
    app_id: SupportedAppId,
    scope: MountScope,
    project: Option<&StoredProject>,
) -> Result<u64, MountLifecycleError> {
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
            None => break,
            Some(metadata) if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR => {
                parent = open_directory_at(&parent, name)
                    .map_err(|source| mount_io("安全打开 Mount 父目录", &child_path, source))?;
                parent_path = child_path;
            }
            Some(_) => {
                return Err(MountLifecycleError::UnsafeMountPath(
                    child_path.display().to_string(),
                ));
            }
        }
    }
    let opened = OpenMountParent {
        base,
        base_path,
        parent,
        parent_path,
    };
    let device = opened
        .parent
        .metadata()
        .map_err(|source| mount_io("检查批量 Mount 文件系统", &opened.parent_path, source))?
        .dev();
    recheck_open_parent(&opened)?;
    Ok(device)
}

fn validate_consumed_batch_plan(
    preview: &StoredBatchMountPlan,
    consumed: &StoredBatchMountPlan,
    selected_item_ids: &[String],
) -> Result<(), MountLifecycleError> {
    let selected = selected_item_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut expected = preview.clone();
    expected.status = "consumed".to_owned();
    for item in &mut expected.items {
        item.selected = selected.contains(item.id.as_str());
    }
    if &expected == consumed {
        Ok(())
    } else {
        Err(MountLifecycleError::PlanPreconditionChanged)
    }
}

fn execute_batch_mount(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    plan: &StoredBatchMountPlan,
    journal: &mut BatchMountJournal,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), MountLifecycleError> {
    write_batch_journal(paths, lifecycle_lock, journal)?;
    storage.update_batch_mount_transaction_phase(
        &journal.transaction_id,
        BatchMountJournalPhase::JournalReady.as_storage_str(),
        now,
    )?;
    lifecycle_lock.recheck(paths)?;

    let mut selected = plan
        .items
        .iter()
        .filter(|item| item.selected)
        .collect::<Vec<_>>();
    selected.sort_by(|left, right| {
        left.target_path
            .cmp(&right.target_path)
            .then_with(|| left.id.cmp(&right.id))
    });
    for (index, item) in selected.into_iter().enumerate() {
        let item_failpoint = if index == 0 {
            failpoint
        } else {
            LifecycleFailpoint::None
        };
        apply_batch_mount_item(
            paths,
            lifecycle_lock,
            storage,
            item,
            journal,
            false,
            item_failpoint,
        )?;
        write_batch_journal(paths, lifecycle_lock, journal)?;
        lifecycle_lock.recheck(paths)?;
        if index == 0 {
            inject_hard_exit(
                failpoint,
                LifecycleFailpoint::HardExitAfterFirstBatchMountTargetAppliedBeforePhase,
            );
            journal.phase = BatchMountJournalPhase::Applying;
            write_batch_journal(paths, lifecycle_lock, journal)?;
            storage.update_batch_mount_transaction_phase(
                &journal.transaction_id,
                journal.phase.as_storage_str(),
                now,
            )?;
        }
    }

    journal.phase = BatchMountJournalPhase::TargetsApplied;
    write_batch_journal(paths, lifecycle_lock, journal)?;
    storage.update_batch_mount_transaction_phase(
        &journal.transaction_id,
        journal.phase.as_storage_str(),
        now,
    )?;
    inject_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterAllBatchMountTargetsAppliedBeforeState,
    );
    // 阶段信号持久化后再紧贴 SQLite 提交复验，外部不能利用可观察间隔写入虚假的 healthy 状态。
    verify_batch_mount_effects(paths, storage, plan, journal)?;
    storage.finalize_batch_mount_create(&journal.transaction_id, plan, now)?;
    verify_batch_mount_effects(paths, storage, plan, journal)?;
    write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
    inject_hard_exit(
        failpoint,
        LifecycleFailpoint::HardExitAfterBatchMountStateCommittedBeforeJournal,
    );
    journal.phase = BatchMountJournalPhase::StateCommitted;
    write_batch_journal(paths, lifecycle_lock, journal)?;
    Ok(())
}

fn apply_batch_mount_item(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &Storage,
    item: &StoredBatchMountPlanItem,
    journal: &mut BatchMountJournal,
    recovering: bool,
    failpoint: LifecycleFailpoint,
) -> Result<(), MountLifecycleError> {
    let journal_index = journal
        .items
        .iter()
        .position(|candidate| candidate.item_id == item.id)
        .ok_or_else(|| {
            MountLifecycleError::RecoveryBlocked("Batch Journal 缺少已确认项目".to_owned())
        })?;
    let member = storage.read_managed_member(&item.member_id)?;
    if member.bundle_id != item.bundle_id
        || member.skill_name != item.skill_name
        || member.content_fingerprint != item.member_fingerprint
        || member.expected_target != item.expected_target
    {
        return Err(MountLifecycleError::PlanPreconditionChanged);
    }
    validate_member_content(&member)?;
    let project = batch_item_project(storage, item)?;
    validate_project_identity(project.as_ref())?;
    let parent = match open_mount_parent(paths, item.app_id, item.scope, project.as_ref(), true)? {
        ParentLookup::Open(parent) => parent,
        ParentLookup::Missing => {
            return Err(MountLifecycleError::UnsafeMountPath(
                item.target_path.clone(),
            ));
        }
    };
    ensure_batch_parent_matches_target(&parent, item)?;
    let leaf = OsStr::new(&item.skill_name);
    let temporary_name = batch_mount_temporary_name(journal, item);
    let temporary_path = parent.parent_path.join(&temporary_name);
    let current = snapshot_at(&parent.parent, leaf, &item.expected_target)?;

    if let Some(applied) = journal.items[journal_index].applied_observation.clone() {
        if current.observation != applied {
            if recovering && current.observation == item.target_observation {
                let staged = snapshot_at(&parent.parent, &temporary_name, &item.expected_target)?;
                if staged.observation != applied {
                    return Err(MountLifecycleError::RecoveryBlocked(format!(
                        "批量 Mount 的暂存链接无法确认归属：{}",
                        temporary_path.display()
                    )));
                }
                recheck_open_parent(&parent)?;
                rename_at_no_replace(&parent.parent, &temporary_name, &parent.parent, leaf)
                    .map_err(|source| mount_io("发布批量 Mount", &parent.parent_path, source))?;
                let published = snapshot_at(&parent.parent, leaf, &item.expected_target)?;
                if published.observation != applied {
                    return Err(MountLifecycleError::RecoveryBlocked(format!(
                        "批量 Mount 发布后无法确认归属：{}",
                        item.target_path
                    )));
                }
                parent
                    .parent
                    .sync_all()
                    .map_err(|source| mount_io("同步批量 Mount", &parent.parent_path, source))?;
                recheck_open_parent(&parent)?;
                return Ok(());
            }
            return Err(MountLifecycleError::RecoveryBlocked(format!(
                "批量 Mount 目标在恢复时被外部修改：{}",
                item.target_path
            )));
        }
        let staged = snapshot_at(&parent.parent, &temporary_name, &item.expected_target)?;
        if staged.kind != TargetKind::Absent {
            return Err(MountLifecycleError::RecoveryBlocked(format!(
                "批量 Mount 已发布但暂存路径仍被占用：{}",
                temporary_path.display()
            )));
        }
        recheck_open_parent(&parent)?;
        return Ok(());
    }

    let staged = snapshot_at(&parent.parent, &temporary_name, &item.expected_target)?;
    if staged.kind != TargetKind::Absent {
        return Err(if recovering {
            MountLifecycleError::RecoveryBlocked(format!(
                "批量 Mount 暂存路径存在但缺少归属证据：{}",
                temporary_path.display()
            ))
        } else {
            MountLifecycleError::UnsafeMountPath(temporary_path.display().to_string())
        });
    }

    if current.observation != item.target_observation {
        return Err(if recovering {
            MountLifecycleError::RecoveryBlocked(format!(
                "批量 Mount 目标与已确认前置状态不一致：{}",
                item.target_path
            ))
        } else {
            MountLifecycleError::PlanPreconditionChanged
        });
    }

    match current.kind {
        TargetKind::Absent => {
            symlink_at(
                Path::new(&item.expected_target),
                &parent.parent,
                &temporary_name,
            )
            .map_err(|source| mount_io("创建批量 Mount 暂存链接", &parent.parent_path, source))?;
            journal.items[journal_index].created_by_transaction = true;
            let applied = snapshot_at(&parent.parent, &temporary_name, &item.expected_target)?;
            if applied.kind != TargetKind::ExpectedLink {
                return Err(MountLifecycleError::RecoveryBlocked(format!(
                    "批量 Mount 暂存链接创建后无法验证：{}",
                    temporary_path.display()
                )));
            }
            journal.items[journal_index].applied_observation = Some(applied.observation.clone());
            parent.parent.sync_all().map_err(|source| {
                mount_io("同步批量 Mount 暂存链接", &parent.parent_path, source)
            })?;
            // 先持久化暂存链接的 inode 证据，再向 Host 的最终名称发布，失败回滚不会误删后来出现的内容。
            write_batch_journal(paths, lifecycle_lock, journal)?;
            inject_hard_exit(
                failpoint,
                LifecycleFailpoint::HardExitAfterFirstBatchMountStageJournalBeforePublish,
            );
            lifecycle_lock.recheck(paths)?;
            recheck_open_parent(&parent)?;
            let staged = snapshot_at(&parent.parent, &temporary_name, &item.expected_target)?;
            if staged.observation != applied.observation {
                return Err(MountLifecycleError::RecoveryBlocked(format!(
                    "批量 Mount 暂存链接在发布前被外部修改：{}",
                    temporary_path.display()
                )));
            }
            rename_at_no_replace(&parent.parent, &temporary_name, &parent.parent, leaf)
                .map_err(|source| mount_io("发布批量 Mount", &parent.parent_path, source))?;
            let published = snapshot_at(&parent.parent, leaf, &item.expected_target)?;
            if published.observation != applied.observation {
                return Err(MountLifecycleError::RecoveryBlocked(format!(
                    "批量 Mount 发布后无法确认归属：{}",
                    item.target_path
                )));
            }
            parent
                .parent
                .sync_all()
                .map_err(|source| mount_io("同步批量 Mount", &parent.parent_path, source))?;
        }
        TargetKind::ExpectedLink => {
            // 已有正确链接只补齐数据库关系，任何失败回滚都不能删除它。
            journal.items[journal_index].created_by_transaction = false;
            journal.items[journal_index].applied_observation = Some(current.observation);
        }
        TargetKind::Other => return Err(MountLifecycleError::PlanPreconditionChanged),
    }
    recheck_open_parent(&parent)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_batch_mount_error(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    plan: &StoredBatchMountPlan,
    journal: &mut BatchMountJournal,
    now: i64,
    error: &MountLifecycleError,
    failpoint: LifecycleFailpoint,
) -> Result<(), MountLifecycleError> {
    let transaction = storage
        .recoverable_batch_mount_transactions()?
        .into_iter()
        .find(|transaction| transaction.id == journal.transaction_id)
        .ok_or_else(|| {
            MountLifecycleError::RecoveryBlocked(
                "Batch Mount 错误处理时事务记录已经不存在".to_owned(),
            )
        })?;
    if transaction.status == "completed" {
        // SQLite 已经提交后绝不反向删除；同时冻结相关对象，避免人工恢复前出现第二个所有者。
        storage.block_batch_mount_transaction(&journal.transaction_id, &error.to_string(), now)?;
        return Ok(());
    }
    if transaction.phase == "journal_pending" {
        storage.abort_batch_mount_transaction(
            &journal.transaction_id,
            Some(&error.to_string()),
            now,
        )?;
        remove_batch_journal(paths, lifecycle_lock, journal)?;
        storage.forget_terminal_batch_mount_transaction(&journal.transaction_id)?;
        return Ok(());
    }

    journal.phase = BatchMountJournalPhase::RollingBack;
    if let Err(rollback_start_error) = write_batch_journal(paths, lifecycle_lock, journal) {
        storage.block_batch_mount_transaction(
            &journal.transaction_id,
            &rollback_start_error.to_string(),
            now,
        )?;
        return Err(rollback_start_error);
    }
    storage.begin_batch_mount_rollback(&journal.transaction_id, now)?;
    if let Err(rollback_error) =
        rollback_batch_mount(paths, lifecycle_lock, storage, plan, journal, failpoint)
    {
        storage.block_batch_mount_transaction(
            &journal.transaction_id,
            &rollback_error.to_string(),
            now,
        )?;
        return Err(rollback_error);
    }
    storage.abort_batch_mount_transaction(
        &journal.transaction_id,
        Some(&error.to_string()),
        now,
    )?;
    remove_batch_journal(paths, lifecycle_lock, journal)?;
    storage.forget_terminal_batch_mount_transaction(&journal.transaction_id)?;
    Ok(())
}

struct BatchRollbackDiscard {
    staging_root: File,
    transaction: File,
    discard: File,
    transaction_name: OsString,
    transaction_path: PathBuf,
    discard_path: PathBuf,
}

fn open_batch_rollback_discard(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    journal: &BatchMountJournal,
) -> Result<BatchRollbackDiscard, MountLifecycleError> {
    ensure_single_component(&journal.transaction_id)?;
    let staging_root =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.staging_root())?;
    let transaction_name = OsString::from(&journal.transaction_id);
    let transaction_path = paths.staging_root().join(&transaction_name);
    let transaction =
        open_or_create_rollback_directory(&staging_root, &transaction_name, &transaction_path)?;
    ensure_only_expected_entries(
        &transaction,
        &BTreeSet::from([OsString::from("discard")]),
        &transaction_path,
    )?;
    let discard_path = transaction_path.join("discard");
    let discard =
        open_or_create_rollback_directory(&transaction, OsStr::new("discard"), &discard_path)?;
    let expected_entries = journal
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.created_by_transaction)
        .map(|(index, _)| batch_rollback_discard_name(index))
        .collect::<BTreeSet<_>>();
    ensure_only_expected_entries(&discard, &expected_entries, &discard_path)?;
    Ok(BatchRollbackDiscard {
        staging_root,
        transaction,
        discard,
        transaction_name,
        transaction_path,
        discard_path,
    })
}

fn batch_rollback_discard_name(index: usize) -> OsString {
    OsString::from(format!("item-{index}"))
}

fn open_or_create_rollback_directory(
    parent: &File,
    name: &OsStr,
    path: &Path,
) -> Result<File, MountLifecycleError> {
    match mkdir_at(parent, name, 0o700) {
        Ok(()) => parent
            .sync_all()
            .map_err(|source| mount_io("同步受管回滚目录", path, source))?,
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
        Err(source) => return Err(mount_io("创建受管回滚目录", path, source)),
    }
    let child = open_directory_at(parent, name)
        .map_err(|source| mount_io("安全打开受管回滚目录", path, source))?;
    recheck_child_directory(parent, name, &child, path)?;
    Ok(child)
}

fn recheck_child_directory(
    parent: &File,
    name: &OsStr,
    child: &File,
    path: &Path,
) -> Result<(), MountLifecycleError> {
    let visible = entry_metadata_at(parent, name)
        .map_err(|source| mount_io("重新检查受管回滚目录", path, source))?
        .ok_or_else(|| {
            MountLifecycleError::RecoveryBlocked(format!(
                "受管回滚目录在操作期间消失：{}",
                path.display()
            ))
        })?;
    let opened = child
        .metadata()
        .map_err(|source| mount_io("检查受管回滚目录句柄", path, source))?;
    if visible.st_mode & libc::S_IFMT != libc::S_IFDIR
        || visible.st_dev as u64 != opened.dev()
        || visible.st_ino as u64 != opened.ino()
    {
        return Err(MountLifecycleError::RecoveryBlocked(format!(
            "受管回滚目录在操作期间被替换：{}",
            path.display()
        )));
    }
    Ok(())
}

fn recheck_batch_rollback_discard(
    discard: &BatchRollbackDiscard,
) -> Result<(), MountLifecycleError> {
    let visible_staging =
        fs::symlink_metadata(discard.transaction_path.parent().ok_or_else(|| {
            MountLifecycleError::RecoveryBlocked("受管回滚目录缺少父路径".to_owned())
        })?)
        .map_err(|source| {
            mount_io(
                "重新检查受管 staging",
                discard
                    .transaction_path
                    .parent()
                    .unwrap_or(Path::new("<staging>")),
                source,
            )
        })?;
    let opened_staging = discard
        .staging_root
        .metadata()
        .map_err(|source| mount_io("检查受管 staging 句柄", &discard.transaction_path, source))?;
    if visible_staging.file_type().is_symlink()
        || !visible_staging.is_dir()
        || visible_staging.dev() != opened_staging.dev()
        || visible_staging.ino() != opened_staging.ino()
    {
        return Err(MountLifecycleError::RecoveryBlocked(
            "受管 staging 在回滚期间被替换".to_owned(),
        ));
    }
    recheck_child_directory(
        &discard.staging_root,
        &discard.transaction_name,
        &discard.transaction,
        &discard.transaction_path,
    )?;
    recheck_child_directory(
        &discard.transaction,
        OsStr::new("discard"),
        &discard.discard,
        &discard.discard_path,
    )
}

fn ensure_only_expected_entries(
    directory: &File,
    expected: &BTreeSet<OsString>,
    path: &Path,
) -> Result<(), MountLifecycleError> {
    let entries = read_mount_directory_entries(directory)?;
    if let Some(unknown) = entries.iter().find(|entry| !expected.contains(*entry)) {
        return Err(MountLifecycleError::RecoveryBlocked(format!(
            "受管回滚目录包含未知内容：{}",
            path.join(unknown).display()
        )));
    }
    Ok(())
}

struct MountDirectoryStream(*mut libc::DIR);

impl Drop for MountDirectoryStream {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0);
        }
    }
}

fn read_mount_directory_entries(directory: &File) -> Result<Vec<OsString>, MountLifecycleError> {
    let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
    if duplicate < 0 {
        return Err(mount_io(
            "保留受管回滚目录句柄",
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
        return Err(mount_io("读取受管回滚目录", Path::new("<dirfd>"), error));
    }
    let stream = MountDirectoryStream(stream);
    let mut names = Vec::new();
    loop {
        clear_mount_errno();
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            if error.raw_os_error().unwrap_or(0) == 0 {
                break;
            }
            return Err(mount_io("读取受管回滚目录", Path::new("<dirfd>"), error));
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            names.push(OsString::from_vec(name.to_vec()));
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(target_os = "macos")]
fn clear_mount_errno() {
    unsafe {
        *libc::__error() = 0;
    }
}

#[cfg(target_os = "linux")]
fn clear_mount_errno() {
    unsafe {
        *libc::__errno_location() = 0;
    }
}

fn inject_unknown_quarantine_replacement(
    failpoint: LifecycleFailpoint,
    parent: &OpenMountParent,
    quarantine_name: &OsStr,
) -> Result<(), MountLifecycleError> {
    if failpoint != LifecycleFailpoint::ReplaceFirstBatchMountQuarantineWithUnknownBeforeDiscard {
        return Ok(());
    }
    // 仅测试构造器可启用：模拟外部进程恰好在 Host 快照校验后替换同名条目。
    unlink_at(&parent.parent, quarantine_name, false)
        .map_err(|source| mount_io("模拟替换 Host 回滚隔离内容", &parent.parent_path, source))?;
    let unknown_path = parent.parent_path.join(quarantine_name);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&unknown_path)
        .map_err(|source| mount_io("模拟写入未知 Host 内容", &unknown_path, source))?;
    parent
        .parent
        .sync_all()
        .map_err(|source| mount_io("同步模拟 Host 竞态", &parent.parent_path, source))
}

fn cleanup_batch_rollback_discard(
    discard: BatchRollbackDiscard,
) -> Result<(), MountLifecycleError> {
    recheck_batch_rollback_discard(&discard)?;
    if !read_mount_directory_entries(&discard.discard)?.is_empty() {
        return Err(MountLifecycleError::RecoveryBlocked(format!(
            "受管回滚清理区仍包含内容：{}",
            discard.discard_path.display()
        )));
    }
    let BatchRollbackDiscard {
        staging_root,
        transaction,
        discard: discard_handle,
        transaction_name,
        transaction_path,
        discard_path,
    } = discard;
    drop(discard_handle);
    unlink_at(&transaction, OsStr::new("discard"), true)
        .map_err(|source| mount_io("清理空受管回滚目录", &discard_path, source))?;
    transaction
        .sync_all()
        .map_err(|source| mount_io("同步受管回滚事务目录", &transaction_path, source))?;
    if !read_mount_directory_entries(&transaction)?.is_empty() {
        return Err(MountLifecycleError::RecoveryBlocked(format!(
            "受管回滚事务目录包含未知内容：{}",
            transaction_path.display()
        )));
    }
    recheck_child_directory(
        &staging_root,
        &transaction_name,
        &transaction,
        &transaction_path,
    )?;
    drop(transaction);
    unlink_at(&staging_root, &transaction_name, true)
        .map_err(|source| mount_io("清理空受管回滚事务目录", &transaction_path, source))?;
    staging_root
        .sync_all()
        .map_err(|source| mount_io("同步 staging 清理", &transaction_path, source))
}

fn rollback_batch_mount(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &Storage,
    plan: &StoredBatchMountPlan,
    journal: &mut BatchMountJournal,
    failpoint: LifecycleFailpoint,
) -> Result<(), MountLifecycleError> {
    let discard = open_batch_rollback_discard(paths, lifecycle_lock, journal)?;
    for index in (0..journal.items.len()).rev() {
        let item_id = journal.items[index].item_id.clone();
        let created_by_transaction = journal.items[index].created_by_transaction;
        let item = plan
            .items
            .iter()
            .find(|item| item.id == item_id)
            .ok_or_else(|| {
                MountLifecycleError::RecoveryBlocked("Batch Plan 与 Journal 不一致".to_owned())
            })?;
        if !created_by_transaction {
            journal.items[index].rolled_back = true;
            write_batch_journal(paths, lifecycle_lock, journal)?;
            lifecycle_lock.recheck(paths)?;
            continue;
        }
        let applied = journal.items[index]
            .applied_observation
            .clone()
            .ok_or_else(|| {
                MountLifecycleError::RecoveryBlocked(
                    "Batch Journal 缺少事务创建链接的精确快照".to_owned(),
                )
            })?;
        let project = batch_item_project(storage, item)?;
        let parent =
            match open_mount_parent(paths, item.app_id, item.scope, project.as_ref(), false)? {
                ParentLookup::Open(parent) => parent,
                ParentLookup::Missing => {
                    return Err(MountLifecycleError::RecoveryBlocked(format!(
                        "批量 Mount 回滚时父目录消失：{}",
                        item.target_path
                    )));
                }
            };
        ensure_batch_parent_matches_target(&parent, item)?;
        let leaf = OsStr::new(&item.skill_name);
        let staged_name = batch_mount_temporary_name(journal, item);
        let quarantine_name = OsString::from(format!(
            ".skillyard-batch-rollback-{}-{index}",
            journal.transaction_id
        ));
        let discard_name = batch_rollback_discard_name(index);
        let current = snapshot_at(&parent.parent, leaf, &item.expected_target)?;
        let staged = snapshot_at(&parent.parent, &staged_name, &item.expected_target)?;
        let quarantine = snapshot_at(&parent.parent, &quarantine_name, &item.expected_target)?;
        let discarded = snapshot_at(&discard.discard, &discard_name, &item.expected_target)?;
        let (owned_name, mut quarantine_owned, mut discard_owned) = if quarantine.observation
            == applied
            && current.observation == item.target_observation
            && staged.kind == TargetKind::Absent
            && discarded.kind == TargetKind::Absent
        {
            (None, true, false)
        } else if quarantine.kind != TargetKind::Absent {
            return Err(MountLifecycleError::RecoveryBlocked(format!(
                "批量 Mount 回滚隔离路径被未知内容占用：{}",
                parent.parent_path.join(&quarantine_name).display()
            )));
        } else if discarded.observation == applied
            && current.observation == item.target_observation
            && staged.kind == TargetKind::Absent
        {
            (None, false, true)
        } else if discarded.kind != TargetKind::Absent {
            return Err(MountLifecycleError::RecoveryBlocked(format!(
                "批量 Mount 受管清理路径被未知内容占用：{}",
                discard.discard_path.join(&discard_name).display()
            )));
        } else if current.observation == applied && staged.kind == TargetKind::Absent {
            (Some(leaf), false, false)
        } else if current.observation == item.target_observation && staged.observation == applied {
            (Some(staged_name.as_os_str()), false, false)
        } else if item.target_observation == "absent"
            && current.kind == TargetKind::Absent
            && staged.kind == TargetKind::Absent
        {
            // 四个固定位置都缺失，才能把上次中断视为已经完成删除。
            (None, false, false)
        } else {
            return Err(MountLifecycleError::RecoveryBlocked(format!(
                "批量 Mount 回滚前无法确认事务链接的唯一归属：{}",
                item.target_path
            )));
        };
        if let Some(owned_name) = owned_name {
            rename_at_no_replace(&parent.parent, owned_name, &parent.parent, &quarantine_name)
                .map_err(|source| {
                    mount_io("隔离批量 Mount 回滚链接", &parent.parent_path, source)
                })?;
            let moved = snapshot_at(&parent.parent, &quarantine_name, &item.expected_target)?;
            if moved.observation != applied {
                // 隔离前选中了最终名还是暂存名，竞态恢复就必须回到同一个名字，不能把未知内容发布到 Host。
                let restored = rename_at_no_replace(
                    &parent.parent,
                    &quarantine_name,
                    &parent.parent,
                    owned_name,
                );
                parent.parent.sync_all().map_err(|source| {
                    mount_io("同步批量 Mount 竞态恢复", &parent.parent_path, source)
                })?;
                return Err(if restored.is_ok() {
                    MountLifecycleError::PlanPreconditionChanged
                } else {
                    MountLifecycleError::RecoveryBlocked(
                        "批量 Mount 回滚时未知内容已保留在隔离路径".to_owned(),
                    )
                });
            }
            parent.parent.sync_all().map_err(|source| {
                mount_io("同步批量 Mount 回滚隔离", &parent.parent_path, source)
            })?;
            inject_hard_exit(
                failpoint,
                LifecycleFailpoint::HardExitAfterFirstBatchMountQuarantineBeforeUnlink,
            );
            quarantine_owned = true;
        }

        if quarantine_owned {
            inject_unknown_quarantine_replacement(failpoint, &parent, &quarantine_name)?;
            recheck_open_parent(&parent)?;
            recheck_batch_rollback_discard(&discard)?;
            match rename_at_no_replace(
                &parent.parent,
                &quarantine_name,
                &discard.discard,
                &discard_name,
            ) {
                Ok(()) => {}
                Err(source) if source.raw_os_error() == Some(libc::EXDEV) => {
                    return Err(MountLifecycleError::RecoveryBlocked(format!(
                        "批量 Mount 回滚跨文件系统，事务内容保留在 Host 隔离路径：{}",
                        parent.parent_path.join(&quarantine_name).display()
                    )));
                }
                Err(source) => {
                    return Err(mount_io(
                        "转移批量 Mount 到受管清理区",
                        &discard.discard_path,
                        source,
                    ));
                }
            }
            let moved = snapshot_at(&discard.discard, &discard_name, &item.expected_target)?;
            if moved.observation != applied {
                // Host 校验后的竞态内容必须原路恢复；恢复失败也只能保留，绝不能删除。
                let restored = rename_at_no_replace(
                    &discard.discard,
                    &discard_name,
                    &parent.parent,
                    &quarantine_name,
                );
                let discard_synced = discard.discard.sync_all();
                let host_synced = parent.parent.sync_all();
                if restored.is_ok() && discard_synced.is_ok() && host_synced.is_ok() {
                    let restored_snapshot =
                        snapshot_at(&parent.parent, &quarantine_name, &item.expected_target)?;
                    if restored_snapshot.observation == moved.observation {
                        return Err(MountLifecycleError::PlanPreconditionChanged);
                    }
                }
                return Err(MountLifecycleError::RecoveryBlocked(format!(
                    "批量 Mount 回滚时未知内容已保留，需人工检查 {} 与 {}",
                    parent.parent_path.join(&quarantine_name).display(),
                    discard.discard_path.join(&discard_name).display()
                )));
            }
            parent
                .parent
                .sync_all()
                .map_err(|source| mount_io("同步 Host 回滚隔离", &parent.parent_path, source))?;
            discard
                .discard
                .sync_all()
                .map_err(|source| mount_io("同步受管回滚清理区", &discard.discard_path, source))?;
            inject_hard_exit(
                failpoint,
                LifecycleFailpoint::HardExitAfterFirstBatchMountDiscardBeforeUnlink,
            );
            discard_owned = true;
        }

        if discard_owned {
            // 恢复可能正好落在 rename 与 fsync 之间；再次同步两侧后才能删除受管条目。
            parent
                .parent
                .sync_all()
                .map_err(|source| mount_io("同步 Host 回滚隔离", &parent.parent_path, source))?;
            discard
                .discard
                .sync_all()
                .map_err(|source| mount_io("同步受管回滚清理区", &discard.discard_path, source))?;
            recheck_batch_rollback_discard(&discard)?;
            let before_unlink =
                snapshot_at(&discard.discard, &discard_name, &item.expected_target)?;
            if before_unlink.observation != applied {
                return Err(MountLifecycleError::RecoveryBlocked(format!(
                    "批量 Mount 受管清理内容在删除前被修改：{}",
                    discard.discard_path.join(&discard_name).display()
                )));
            }
            unlink_at(&discard.discard, &discard_name, false).map_err(|source| {
                mount_io("删除受管批量 Mount 回滚链接", &discard.discard_path, source)
            })?;
            discard.discard.sync_all().map_err(|source| {
                mount_io("同步受管批量 Mount 回滚", &discard.discard_path, source)
            })?;
            inject_hard_exit(
                failpoint,
                LifecycleFailpoint::HardExitAfterFirstBatchMountRollbackBeforeProgress,
            );
        }
        recheck_open_parent(&parent)?;
        journal.items[index].rolled_back = true;
        write_batch_journal(paths, lifecycle_lock, journal)?;
        lifecycle_lock.recheck(paths)?;
    }
    cleanup_batch_rollback_discard(discard)?;
    Ok(())
}

fn recover_batch_mount_transaction(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    transaction: &StoredBatchMountTransaction,
    now: i64,
) -> Result<(), MountLifecycleError> {
    ensure_single_component(&transaction.id)?;
    ensure_single_component(&transaction.plan_id)?;
    ensure_single_component(&transaction.bundle_id)?;
    let expected_journal = format!("journals/mount-batch-{}.json", transaction.id);
    if transaction.journal_path != expected_journal {
        return Err(MountLifecycleError::RecoveryBlocked(
            "SQLite 中的 Batch Mount Journal 路径不符合固定布局".to_owned(),
        ));
    }
    let journal_name = OsString::from(format!("mount-batch-{}.json", transaction.id));
    let journals =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.journals_root())?;
    let journal_path = paths.journals_root().join(&journal_name);
    let journal_exists = entry_metadata_at(&journals, &journal_name)
        .map_err(|source| mount_io("检查 Batch Mount Journal", &journal_path, source))?
        .is_some();
    if !journal_exists {
        if matches!(transaction.status.as_str(), "completed" | "aborted") {
            storage.forget_terminal_batch_mount_transaction(&transaction.id)?;
            return Ok(());
        }
        if transaction.phase == "journal_pending" && transaction.status == "in_progress" {
            storage.abort_batch_mount_transaction(&transaction.id, None, now)?;
            storage.forget_terminal_batch_mount_transaction(&transaction.id)?;
            return Ok(());
        }
        return Err(MountLifecycleError::RecoveryBlocked(
            "Batch Mount Journal 缺失但事务已经进入文件系统阶段".to_owned(),
        ));
    }

    let plan = storage.read_batch_mount_plan(&transaction.plan_id)?;
    validate_batch_plan_contract(paths, storage, &plan)?;
    let mut journal = read_batch_journal(&journals, &journal_name, &journal_path)?;
    validate_batch_journal(&journal, transaction, &plan)?;

    let rolling_back = transaction.status == "in_progress"
        && (journal.phase == BatchMountJournalPhase::RollingBack
            || transaction.phase == "rolling_back");
    if transaction.status == "aborted" {
        remove_batch_journal(paths, lifecycle_lock, &journal)?;
        storage.forget_terminal_batch_mount_transaction(&transaction.id)?;
        return Ok(());
    }
    if rolling_back {
        if journal.phase != BatchMountJournalPhase::RollingBack {
            return Err(MountLifecycleError::RecoveryBlocked(
                "SQLite 已进入回滚，但 Journal 没有保存回滚方向".to_owned(),
            ));
        }
        preflight_batch_mount_devices(
            paths,
            lifecycle_lock,
            storage,
            &plan,
            &transaction.selected_item_ids,
        )?;
        storage.begin_batch_mount_rollback(&transaction.id, now)?;
        rollback_batch_mount(
            paths,
            lifecycle_lock,
            storage,
            &plan,
            &mut journal,
            LifecycleFailpoint::None,
        )?;
        storage.abort_batch_mount_transaction(&transaction.id, None, now)?;
        remove_batch_journal(paths, lifecycle_lock, &journal)?;
        storage.forget_terminal_batch_mount_transaction(&transaction.id)?;
        return Ok(());
    }
    if transaction.status == "in_progress" && transaction.phase == "journal_pending" {
        if journal
            .items
            .iter()
            .any(|item| item.applied_observation.is_some())
        {
            return Err(MountLifecycleError::RecoveryBlocked(
                "Batch Mount 事务阶段落后于已记录文件系统效果".to_owned(),
            ));
        }
        storage.abort_batch_mount_transaction(&transaction.id, None, now)?;
        remove_batch_journal(paths, lifecycle_lock, &journal)?;
        storage.forget_terminal_batch_mount_transaction(&transaction.id)?;
        return Ok(());
    }

    if transaction.status == "in_progress" {
        preflight_batch_mount_devices(
            paths,
            lifecycle_lock,
            storage,
            &plan,
            &transaction.selected_item_ids,
        )?;
    }

    let forward_result = (|| -> Result<(), MountLifecycleError> {
        if transaction.status == "in_progress" {
            let mut selected = plan
                .items
                .iter()
                .filter(|item| item.selected)
                .collect::<Vec<_>>();
            selected.sort_by(|left, right| {
                left.target_path
                    .cmp(&right.target_path)
                    .then_with(|| left.id.cmp(&right.id))
            });
            for item in selected {
                apply_batch_mount_item(
                    paths,
                    lifecycle_lock,
                    storage,
                    item,
                    &mut journal,
                    true,
                    LifecycleFailpoint::None,
                )?;
                write_batch_journal(paths, lifecycle_lock, &journal)?;
                lifecycle_lock.recheck(paths)?;
            }
            if matches!(transaction.phase.as_str(), "journal_ready" | "applying") {
                journal.phase = BatchMountJournalPhase::Applying;
                write_batch_journal(paths, lifecycle_lock, &journal)?;
                storage.update_batch_mount_transaction_phase(
                    &transaction.id,
                    BatchMountJournalPhase::Applying.as_storage_str(),
                    now,
                )?;
            }
            journal.phase = BatchMountJournalPhase::TargetsApplied;
            write_batch_journal(paths, lifecycle_lock, &journal)?;
            storage.update_batch_mount_transaction_phase(
                &transaction.id,
                BatchMountJournalPhase::TargetsApplied.as_storage_str(),
                now,
            )?;
            verify_batch_mount_effects(paths, storage, &plan, &journal)?;
            storage.finalize_batch_mount_create(&transaction.id, &plan, now)?;
        }

        verify_batch_mount_effects(paths, storage, &plan, &journal)?;
        write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
        journal.phase = BatchMountJournalPhase::StateCommitted;
        write_batch_journal(paths, lifecycle_lock, &journal)?;
        Ok(())
    })();
    if let Err(error) = forward_result {
        handle_batch_mount_error(
            paths,
            lifecycle_lock,
            storage,
            &plan,
            &mut journal,
            now,
            &error,
            LifecycleFailpoint::None,
        )?;
        return Ok(());
    }
    remove_batch_journal(paths, lifecycle_lock, &journal)?;
    storage.forget_terminal_batch_mount_transaction(&transaction.id)?;
    Ok(())
}

fn verify_batch_mount_effects(
    paths: &ApplicationPaths,
    storage: &Storage,
    plan: &StoredBatchMountPlan,
    journal: &BatchMountJournal,
) -> Result<(), MountLifecycleError> {
    for item in plan.items.iter().filter(|item| item.selected) {
        let journal_item = journal
            .items
            .iter()
            .find(|candidate| candidate.item_id == item.id)
            .ok_or_else(|| {
                MountLifecycleError::RecoveryBlocked("Batch Journal 缺少确认项".to_owned())
            })?;
        let expected = journal_item.applied_observation.as_deref().ok_or_else(|| {
            MountLifecycleError::RecoveryBlocked("Batch Journal 缺少生效快照".to_owned())
        })?;
        let project = batch_item_project(storage, item)?;
        let parent =
            match open_mount_parent(paths, item.app_id, item.scope, project.as_ref(), false)? {
                ParentLookup::Open(parent) => parent,
                ParentLookup::Missing => {
                    return Err(MountLifecycleError::RecoveryBlocked(format!(
                        "已提交的批量 Mount 父目录缺失：{}",
                        item.target_path
                    )));
                }
            };
        ensure_batch_parent_matches_target(&parent, item)?;
        let current = snapshot_at(
            &parent.parent,
            OsStr::new(&item.skill_name),
            &item.expected_target,
        )?;
        if current.observation != expected {
            return Err(MountLifecycleError::RecoveryBlocked(format!(
                "已提交的批量 Mount 被外部修改：{}",
                item.target_path
            )));
        }
        recheck_open_parent(&parent)?;
    }
    Ok(())
}

fn cleanup_completed_batch_mount(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    storage: &mut Storage,
    journal: &BatchMountJournal,
) -> Result<(), MountLifecycleError> {
    remove_batch_journal(paths, lifecycle_lock, journal)?;
    storage.forget_terminal_batch_mount_transaction(&journal.transaction_id)?;
    lifecycle_lock.recheck(paths)?;
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

fn stored_batch_item_project(
    item: &StoredBatchMountPlanItem,
) -> Result<Option<StoredProject>, MountLifecycleError> {
    match item.scope {
        MountScope::Global
            if item.project_id.is_none()
                && item.project_root_path.is_none()
                && item.project_display_name.is_none()
                && item.project_root_device.is_none()
                && item.project_root_inode.is_none() =>
        {
            Ok(None)
        }
        MountScope::Project => Ok(Some(StoredProject {
            id: item
                .project_id
                .clone()
                .ok_or(MountLifecycleError::InvalidScope)?,
            display_name: item
                .project_display_name
                .clone()
                .ok_or(MountLifecycleError::InvalidScope)?,
            root_path: item
                .project_root_path
                .clone()
                .ok_or(MountLifecycleError::InvalidScope)?,
            root_device: item
                .project_root_device
                .ok_or(MountLifecycleError::InvalidScope)?,
            root_inode: item
                .project_root_inode
                .ok_or(MountLifecycleError::InvalidScope)?,
            created_at: 0,
        })),
        _ => Err(MountLifecycleError::InvalidScope),
    }
}

fn batch_item_project(
    storage: &Storage,
    item: &StoredBatchMountPlanItem,
) -> Result<Option<StoredProject>, MountLifecycleError> {
    let snapshot = stored_batch_item_project(item)?;
    let current = read_scope_project(storage, item.scope, item.project_id.as_deref())?;
    if same_project_identity(snapshot.as_ref(), current.as_ref()) {
        Ok(current)
    } else {
        Err(MountLifecycleError::PlanPreconditionChanged)
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

pub(crate) fn open_mount_parent(
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
    walk_relative_parent(base, base_path, &relative, create_missing)
}

/// 从一个真实根目录开始逐组件打开相对目录，任何祖先软链接都会被拒绝。
///
/// 这个入口只处理目录安全，不解释 Supported App 或 Mount Scope；调用方仍需自行派生
/// `base_path` 与 `relative`，避免在不同生命周期操作中复制一套更弱的路径遍历逻辑。
pub(crate) fn open_relative_parent(
    base_path: &Path,
    relative: &Path,
    create_missing: bool,
) -> Result<ParentLookup, MountLifecycleError> {
    let base = open_real_directory(base_path)?;
    walk_relative_parent(base, base_path.to_path_buf(), relative, create_missing)
}

/// 共享 Project 路径也必须绑定已登记 Project 的 inode，不能只按可见绝对路径打开。
pub(crate) fn open_project_relative_parent(
    project: &StoredProject,
    relative: &Path,
    create_missing: bool,
) -> Result<ParentLookup, MountLifecycleError> {
    let base_path = PathBuf::from(&project.root_path);
    let base = open_real_directory(&base_path)?;
    let metadata = base
        .metadata()
        .map_err(|source| mount_io("检查 Project 句柄", &base_path, source))?;
    if metadata.dev() != project.root_device || metadata.ino() != project.root_inode {
        return Err(MountLifecycleError::ProjectChanged(
            project.root_path.clone(),
        ));
    }
    walk_relative_parent(base, base_path, relative, create_missing)
}

fn walk_relative_parent(
    base: File,
    base_path: PathBuf,
    relative: &Path,
    create_missing: bool,
) -> Result<ParentLookup, MountLifecycleError> {
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

pub(crate) fn recheck_open_parent(parent: &OpenMountParent) -> Result<(), MountLifecycleError> {
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

fn ensure_batch_parent_matches_target(
    parent: &OpenMountParent,
    item: &StoredBatchMountPlanItem,
) -> Result<(), MountLifecycleError> {
    if parent.parent_path.join(&item.skill_name) == Path::new(&item.target_path) {
        Ok(())
    } else {
        Err(MountLifecycleError::PlanPreconditionChanged)
    }
}

pub(crate) fn snapshot_at(
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

fn removal_snapshot_plan(snapshot: &ManagedMountRemovalSnapshot) -> StoredMountPlan {
    StoredMountPlan {
        id: format!("removal-{}", snapshot.mount_id),
        operation: MountOperation::Remove,
        purpose: MountPlanPurpose::Remove,
        mount_id: snapshot.mount_id.clone(),
        member_id: snapshot.member_id.clone(),
        bundle_id: snapshot.bundle_id.clone(),
        skill_name: snapshot.skill_name.clone(),
        app_id: snapshot.app_id,
        scope: snapshot.scope,
        project_id: snapshot.project_id.clone(),
        project_display_name: snapshot.project_display_name.clone(),
        project_root_path: snapshot.project_root_path.clone(),
        project_root_device: snapshot.project_root_device,
        project_root_inode: snapshot.project_root_inode,
        target_path: snapshot.target_path.clone(),
        expected_target: snapshot.expected_target.clone(),
        member_fingerprint: snapshot.member_fingerprint.clone(),
        target_observation: snapshot.target_observation.clone(),
        created_at: 0,
        expires_at: i64::MAX,
        status: "consumed".to_owned(),
    }
}

fn removal_snapshot_journal(snapshot: &ManagedMountRemovalSnapshot) -> MountJournal {
    MountJournal {
        version: MOUNT_JOURNAL_VERSION,
        transaction_id: format!("removal-{}", snapshot.mount_id),
        plan_id: format!("removal-{}", snapshot.mount_id),
        mount_id: snapshot.mount_id.clone(),
        operation: MountOperation::Remove,
        target_path: snapshot.target_path.clone(),
        expected_target: snapshot.expected_target.clone(),
        target_observation: snapshot.target_observation.clone(),
        temporary_name: snapshot.temporary_name.clone(),
        phase: MountJournalPhase::JournalReady,
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

fn build_batch_journal(
    transaction_id: &str,
    plan: &StoredBatchMountPlan,
) -> Result<BatchMountJournal, MountLifecycleError> {
    let mut selected = plan
        .items
        .iter()
        .filter(|item| item.selected)
        .collect::<Vec<_>>();
    selected.sort_by(|left, right| {
        left.target_path
            .cmp(&right.target_path)
            .then_with(|| left.id.cmp(&right.id))
    });
    if selected.is_empty() {
        return Err(MountLifecycleError::InvalidBatchMountPlan(
            "Batch Mount 事务没有确认项".to_owned(),
        ));
    }
    Ok(BatchMountJournal {
        version: BATCH_MOUNT_JOURNAL_VERSION,
        transaction_id: transaction_id.to_owned(),
        plan_id: plan.id.clone(),
        bundle_id: plan.bundle_id.clone(),
        phase: BatchMountJournalPhase::JournalReady,
        items: selected
            .into_iter()
            .map(|item| BatchMountJournalItem {
                item_id: item.id.clone(),
                target_observation: item.target_observation.clone(),
                created_by_transaction: false,
                applied_observation: None,
                rolled_back: false,
            })
            .collect(),
    })
}

fn batch_mount_temporary_name(
    journal: &BatchMountJournal,
    item: &StoredBatchMountPlanItem,
) -> OsString {
    // 随机事务与 Plan item 身份让暂存名不会和 Host 的正常 Skill 名发生碰撞。
    OsString::from(format!(
        ".skillyard-batch-stage-{}-{}",
        journal.transaction_id, item.id
    ))
}

fn validate_batch_journal(
    actual: &BatchMountJournal,
    transaction: &StoredBatchMountTransaction,
    plan: &StoredBatchMountPlan,
) -> Result<(), MountLifecycleError> {
    let expected = build_batch_journal(&transaction.id, plan)?;
    let selected = transaction
        .selected_item_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let actual_ids = actual
        .items
        .iter()
        .map(|item| item.item_id.as_str())
        .collect::<BTreeSet<_>>();
    let static_fields_match = actual.version == BATCH_MOUNT_JOURNAL_VERSION
        && actual.transaction_id == expected.transaction_id
        && actual.plan_id == expected.plan_id
        && actual.bundle_id == expected.bundle_id
        && transaction.plan_id == plan.id
        && transaction.bundle_id == plan.bundle_id
        && selected == actual_ids
        && actual.items.len() == expected.items.len();
    if !static_fields_match {
        return Err(MountLifecycleError::RecoveryBlocked(
            "SQLite、Batch Mount Plan 与 Journal 不一致".to_owned(),
        ));
    }
    for actual_item in &actual.items {
        let Some(expected_item) = expected
            .items
            .iter()
            .find(|item| item.item_id == actual_item.item_id)
        else {
            return Err(MountLifecycleError::RecoveryBlocked(
                "Batch Mount Journal 包含未知项目".to_owned(),
            ));
        };
        if actual_item.target_observation != expected_item.target_observation {
            return Err(MountLifecycleError::RecoveryBlocked(
                "Batch Mount Journal 前置快照损坏".to_owned(),
            ));
        }
        if let Some(observation) = actual_item.applied_observation.as_deref()
            && parse_observation_kind(observation) != Some(TargetKind::ExpectedLink)
        {
            return Err(MountLifecycleError::RecoveryBlocked(
                "Batch Mount Journal 生效快照损坏".to_owned(),
            ));
        }
        if actual_item.created_by_transaction && actual_item.applied_observation.is_none() {
            return Err(MountLifecycleError::RecoveryBlocked(
                "Batch Mount Journal 缺少事务创建证据".to_owned(),
            ));
        }
        if actual_item.rolled_back && actual.phase != BatchMountJournalPhase::RollingBack {
            return Err(MountLifecycleError::RecoveryBlocked(
                "Batch Mount Journal 的回滚进度与方向不一致".to_owned(),
            ));
        }
    }
    Ok(())
}

fn ensure_batch_journal_fits(
    journal: &BatchMountJournal,
    plan: &StoredBatchMountPlan,
) -> Result<(), MountLifecycleError> {
    for phase in [
        BatchMountJournalPhase::JournalReady,
        BatchMountJournalPhase::Applying,
        BatchMountJournalPhase::TargetsApplied,
        BatchMountJournalPhase::RollingBack,
        BatchMountJournalPhase::StateCommitted,
    ] {
        let mut candidate = journal.clone();
        candidate.phase = phase;
        // 生效快照会在逐项创建后增长；必须在首次文件写入前按最大数字宽度预留空间。
        for journal_item in &mut candidate.items {
            let plan_item = plan
                .items
                .iter()
                .find(|item| item.id == journal_item.item_id)
                .ok_or_else(|| {
                    MountLifecycleError::InvalidBatchMountPlan(
                        "Batch Journal 包含未知确认项".to_owned(),
                    )
                })?;
            journal_item.applied_observation = Some(format!(
                "expected_symlink:{}:{}:{}:{}",
                u64::MAX,
                u64::MAX,
                u64::MAX,
                hex_bytes(plan_item.expected_target.as_bytes())
            ));
        }
        transaction::serialize_journal(&candidate, MAX_BATCH_MOUNT_JOURNAL_BYTES, true)
            .map_err(journal_io_error)?;
    }
    Ok(())
}

fn write_batch_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    journal: &BatchMountJournal,
) -> Result<(), MountLifecycleError> {
    let name = journal_file_name("mount-batch-", &journal.transaction_id);
    transaction::write_journal(
        paths,
        lifecycle_lock.root(),
        &name,
        journal,
        MAX_BATCH_MOUNT_JOURNAL_BYTES,
        true,
    )
    .map_err(journal_io_error)
}

fn read_batch_journal(
    journals: &File,
    name: &OsStr,
    path: &Path,
) -> Result<BatchMountJournal, MountLifecycleError> {
    transaction::read_journal(
        journals,
        name,
        path,
        MAX_BATCH_MOUNT_JOURNAL_BYTES,
        "检查 Batch Mount Journal",
        "读取 Batch Mount Journal",
    )
    .map_err(journal_io_error)
}

fn remove_batch_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    journal: &BatchMountJournal,
) -> Result<(), MountLifecycleError> {
    let name = journal_file_name("mount-batch-", &journal.transaction_id);
    transaction::remove_journal(
        paths,
        lifecycle_lock.root(),
        &name,
        "清理 Batch Mount Journal",
        "同步 Batch Mount Journal 清理",
        false,
    )
    .map_err(journal_io_error)
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
        transaction::serialize_journal(&candidate, MAX_MOUNT_JOURNAL_BYTES, true)
            .map_err(journal_io_error)?;
    }
    Ok(())
}

fn journal_io_error(error: JournalIoError) -> MountLifecycleError {
    match error {
        JournalIoError::TooLarge { .. } => MountLifecycleError::JournalTooLarge,
        JournalIoError::InvalidJson(source) => MountLifecycleError::InvalidJournal(source),
        JournalIoError::Lifecycle(error) => error.into(),
    }
}

fn write_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    journal: &MountJournal,
) -> Result<(), MountLifecycleError> {
    let name = journal_file_name("mount-", &journal.transaction_id);
    transaction::write_journal(
        paths,
        lifecycle_lock.root(),
        &name,
        journal,
        MAX_MOUNT_JOURNAL_BYTES,
        true,
    )
    .map_err(journal_io_error)
}

fn read_journal(
    journals: &File,
    name: &OsStr,
    path: &Path,
) -> Result<MountJournal, MountLifecycleError> {
    transaction::read_journal(
        journals,
        name,
        path,
        MAX_MOUNT_JOURNAL_BYTES,
        "检查 Mount Journal",
        "读取 Mount Journal",
    )
    .map_err(journal_io_error)
}

fn remove_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &LifecycleLock,
    journal: &MountJournal,
) -> Result<(), MountLifecycleError> {
    let name = journal_file_name("mount-", &journal.transaction_id);
    transaction::remove_journal(
        paths,
        lifecycle_lock.root(),
        &name,
        "清理 Mount Journal",
        "同步 Mount Journal 清理",
        false,
    )
    .map_err(journal_io_error)
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

fn stored_batch_plan_to_ui(
    plan: &StoredBatchMountPlan,
) -> Result<BatchMountPlan, MountLifecycleError> {
    Ok(BatchMountPlan {
        id: plan.id.clone(),
        bundle_id: plan.bundle_id.clone(),
        bundle_display_name: plan.bundle_display_name.clone(),
        items: plan
            .items
            .iter()
            .map(|item| BatchMountPlanItem {
                id: item.id.clone(),
                member_id: item.member_id.clone(),
                skill_name: item.skill_name.clone(),
                app_id: item.app_id,
                scope: item.scope,
                project_id: item.project_id.clone(),
                project_display_name: item.project_display_name.clone(),
                target_path: item.target_path.clone(),
                expected_target: item.expected_target.clone(),
                disposition: item.disposition,
                selectable: item.selectable,
                default_selected: item.default_selected,
                conflict_reason: item.conflict_reason.clone(),
                target_health: item.target_health,
            })
            .collect(),
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
    fn relative_parent_walker_rejects_an_intermediate_symlink() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let base = sandbox.path().join("home");
        let external = sandbox.path().join("external");
        fs::create_dir(&base).expect("应创建可信根目录");
        fs::create_dir(&external).expect("应创建外部目录");
        symlink(&external, base.join("redirected")).expect("应创建中间祖先软链接");

        let error = open_relative_parent(&base, Path::new("redirected/skills"), false)
            .expect_err("逐组件 walker 必须拒绝中间祖先软链接");

        assert!(
            matches!(error, MountLifecycleError::UnsafeMountPath(path) if path == base.join("redirected").display().to_string())
        );
    }

    #[test]
    fn relative_parent_walker_opens_real_components_for_snapshot_and_recheck() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let base = sandbox.path().join("home");
        let relative = Path::new(".agents/skills");
        let parent_path = base.join(relative);
        let expected_target = sandbox.path().join("managed/example-skill");
        fs::create_dir_all(&parent_path).expect("应创建普通目录链");
        fs::create_dir_all(&expected_target).expect("应创建受管目标");
        symlink(&expected_target, parent_path.join("example-skill")).expect("应创建 Mount");

        let ParentLookup::Open(parent) =
            open_relative_parent(&base, relative, false).expect("应逐组件打开普通目录链")
        else {
            panic!("已存在的普通目录链不应报告缺失");
        };
        let snapshot = snapshot_at(
            parent.directory(),
            OsStr::new("example-skill"),
            expected_target.to_str().expect("测试路径应是 UTF-8"),
        )
        .expect("应通过已打开目录读取叶子快照");

        assert_eq!(parent.path(), parent_path);
        assert_eq!(snapshot.kind(), TargetKind::ExpectedLink);
        assert!(snapshot.observation().starts_with("expected_symlink:"));
        recheck_open_parent(&parent).expect("可见路径仍指向同一目录句柄");
    }

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
