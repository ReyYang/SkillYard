use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read},
    path::Path,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    content::MAX_ENTRIES,
    domain::{
        MountSummary, RemovalBundleSummary, RemovalKind, RemovalMemberSummary, RemovalPlan,
        RemovalPreservedSource, SourceKind, SourceSummary,
    },
    lifecycle::{
        LifecycleError, LifecycleFailpoint, SealedTreeCleanupManifest, acquire_lifecycle_lock,
        capture_sealed_tree_cleanup_manifest, discard_pending_install_plans, entry_metadata_at,
        open_managed_directory_from_root, open_regular_file_at,
        remove_sealed_tree_at_with_manifest, rename_at_no_replace, unlink_at, write_atomic_at,
        write_notice_from_storage,
    },
    mount_lifecycle::{
        ManagedMountRemovalSnapshot, ManagedMountRemovalState, MountLifecycleError,
        finalize_managed_mount_removal, inspect_managed_mount_removal,
        isolate_managed_mount_removal, restore_managed_mount_removal, seal_managed_mount_removal,
    },
    paths::ApplicationPaths,
    storage::{
        NewRemovalPlan, Storage, StorageError, StoredMount, StoredProject, StoredRemovalPlan,
        StoredRemovalTransaction, StoredSourceAssociationBundle,
    },
    takeover::{TakeoverError, blocked_takeover_references_project},
};

const REMOVAL_PLAN_TTL_MILLIS: i64 = 30 * 60 * 1_000;
const REMOVAL_JOURNAL_VERSION: u32 = 1;
// 每个合法 Bundle 最多 20,000 个条目；完整清理清单按最坏 255 字节文件名预留空间。
const MAX_REMOVAL_JOURNAL_BYTES: usize = MAX_ENTRIES * 768 + 1024 * 1024;

#[derive(Debug, Error)]
pub enum RemovalError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error(transparent)]
    Mount(#[from] MountLifecycleError),
    #[error(transparent)]
    Takeover(#[from] TakeoverError),
    #[error("Removal Plan 数据无法解析：{0}")]
    InvalidPlanJson(#[source] serde_json::Error),
    #[error("Removal Journal 数据无法解析：{0}")]
    InvalidJournalJson(#[source] serde_json::Error),
    #[error("Removal Plan 的不可变前置状态已经变化，请重新预览")]
    PlanPreconditionChanged,
    #[error("这个 Bundle 当前没有 Mount")]
    NoBundleMounts,
    #[error("Removal 事务需要人工恢复：{0}")]
    RecoveryBlocked(String),
    #[error("无法{action} {path}：{source}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("测试中断点：{0}")]
    SimulatedInterruption(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SealedRemovalPlan {
    version: u32,
    plan: RemovalPlan,
    project: Option<ProjectSnapshot>,
    source: Option<SourceSnapshot>,
    bundle: Option<BundleSnapshot>,
    mount_snapshots: Vec<ManagedMountRemovalSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectSnapshot {
    id: String,
    display_name: String,
    root_path: String,
    root_device: u64,
    root_inode: u64,
    created_at: i64,
}

impl From<&StoredProject> for ProjectSnapshot {
    fn from(project: &StoredProject) -> Self {
        Self {
            id: project.id.clone(),
            display_name: project.display_name.clone(),
            root_path: project.root_path.clone(),
            root_device: project.root_device,
            root_inode: project.root_inode,
            created_at: project.created_at,
        }
    }
}

impl From<&ProjectSnapshot> for StoredProject {
    fn from(project: &ProjectSnapshot) -> Self {
        Self {
            id: project.id.clone(),
            display_name: project.display_name.clone(),
            root_path: project.root_path.clone(),
            root_device: project.root_device,
            root_inode: project.root_inode,
            created_at: project.created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceSnapshot {
    id: String,
    canonical_identity: String,
    display_name: String,
    kind: SourceKind,
    locator: String,
    bundle_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BundleSnapshot {
    id: String,
    display_name: String,
    managed_directory: String,
    current_target: String,
    source_id: Option<String>,
    device: u64,
    inode: u64,
    member_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum RemovalJournalPhase {
    JournalReady,
    MountsIsolated,
    BundleIsolated,
    StateCommitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemovalJournal {
    version: u32,
    transaction_id: String,
    plan_id: String,
    plan_sha256: String,
    kind: RemovalKind,
    target_id: String,
    phase: RemovalJournalPhase,
    bundle_trash_name: Option<String>,
    bundle_cleanup_manifest: Option<SealedTreeCleanupManifest>,
}

pub(crate) fn create_project_removal_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    project_id: &str,
    now: i64,
) -> Result<RemovalPlan, RemovalError> {
    ensure_project_target_available(paths, storage, project_id)?;
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    let project = storage.read_project(project_id)?;
    let mounts = storage
        .read_mounts()?
        .into_iter()
        .filter(|mount| mount.project_id.as_deref() == Some(project_id))
        .collect::<Vec<_>>();
    let plan_id = Uuid::new_v4().to_string();
    let mount_snapshots = seal_mounts(paths, storage, &plan_id, &mounts)?;
    let affected_bundles = affected_bundles(storage, &mounts)?;
    let created_at = now;
    let expires_at = now.saturating_add(REMOVAL_PLAN_TTL_MILLIS);
    let plan = RemovalPlan {
        id: plan_id,
        kind: RemovalKind::Project,
        target_id: project.id.clone(),
        target_display_name: project.display_name.clone(),
        members: Vec::new(),
        mounts: mounts.iter().map(mount_summary).collect(),
        affected_bundles,
        preserved_source: None,
        managed_directory: None,
        preserved_external_paths: vec![project.root_path.clone()],
        warnings: vec![
            "只移除这个 Project 的 managed Mount 与登记记录，不删除 Project 目录".to_owned(),
        ],
        created_at,
        expires_at,
    };
    let sealed = SealedRemovalPlan {
        version: REMOVAL_JOURNAL_VERSION,
        plan: plan.clone(),
        project: Some(ProjectSnapshot::from(&project)),
        source: None,
        bundle: None,
        mount_snapshots,
    };
    save_sealed_plan(storage, &sealed)?;
    lifecycle_lock.recheck(paths)?;
    Ok(plan)
}

pub(crate) fn create_source_removal_plan(
    storage: &mut Storage,
    source_id: &str,
    now: i64,
) -> Result<RemovalPlan, RemovalError> {
    ensure_target_available(storage, "source", source_id)?;
    let source = storage
        .read_source_summaries()?
        .into_iter()
        .find(|source| source.id == source_id)
        .ok_or(StorageError::SourceNotFound)?;
    let affected_bundles = match source.bundle_id.as_deref() {
        Some(bundle_id) => {
            let bundle = storage.read_source_association_bundle(bundle_id)?;
            vec![RemovalBundleSummary {
                id: bundle.id,
                display_name: bundle.display_name,
            }]
        }
        None => Vec::new(),
    };
    let preserved_external_paths = if source.kind == SourceKind::EditableLocal {
        vec![source.locator.clone()]
    } else {
        Vec::new()
    };
    let created_at = now;
    let expires_at = now.saturating_add(REMOVAL_PLAN_TTL_MILLIS);
    let plan = RemovalPlan {
        id: Uuid::new_v4().to_string(),
        kind: RemovalKind::Source,
        target_id: source.id.clone(),
        target_display_name: source.display_name.clone(),
        members: Vec::new(),
        mounts: Vec::new(),
        affected_bundles,
        preserved_source: None,
        managed_directory: None,
        preserved_external_paths,
        warnings: vec![
            "只删除 Source、Catalog 与关联 metadata；Bundle、Current Content 和 Mount 保持不变"
                .to_owned(),
        ],
        created_at,
        expires_at,
    };
    let sealed = SealedRemovalPlan {
        version: REMOVAL_JOURNAL_VERSION,
        plan: plan.clone(),
        project: None,
        source: Some(source_snapshot(&source)),
        bundle: None,
        mount_snapshots: Vec::new(),
    };
    save_sealed_plan(storage, &sealed)?;
    Ok(plan)
}

pub(crate) fn create_bundle_removal_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    bundle_id: &str,
    now: i64,
) -> Result<RemovalPlan, RemovalError> {
    ensure_target_available(storage, "bundle", bundle_id)?;
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    let bundle = storage.read_source_association_bundle(bundle_id)?;
    let plan_id = Uuid::new_v4().to_string();
    let mounts = bundle
        .members
        .iter()
        .flat_map(|member| member.mounts.iter().cloned())
        .collect::<Vec<_>>();
    let mount_snapshots = seal_mounts(paths, storage, &plan_id, &mounts)?;
    // 即使 Bundle 没有 Mount，也必须核对每个 Member 的 current 与受管目录身份。
    for member in &bundle.members {
        storage.read_managed_member(&member.id)?;
    }
    let (device, inode) = read_bundle_identity(paths, lifecycle_lock.root(), bundle_id)?;
    let source = source_for_bundle(storage, bundle.source_id.as_deref())?;
    let preserved_external_paths = source
        .iter()
        .filter(|source| source.kind == SourceKind::EditableLocal)
        .map(|source| source.locator.clone())
        .collect::<Vec<_>>();
    let created_at = now;
    let expires_at = now.saturating_add(REMOVAL_PLAN_TTL_MILLIS);
    let plan = RemovalPlan {
        id: plan_id,
        kind: RemovalKind::Bundle,
        target_id: bundle.id.clone(),
        target_display_name: bundle.display_name.clone(),
        members: bundle
            .members
            .iter()
            .map(|member| RemovalMemberSummary {
                id: member.id.clone(),
                skill_name: member.skill_name.clone(),
            })
            .collect(),
        mounts: mounts.iter().map(mount_summary).collect(),
        affected_bundles: vec![RemovalBundleSummary {
            id: bundle.id.clone(),
            display_name: bundle.display_name.clone(),
        }],
        preserved_source: source.as_ref().map(|source| RemovalPreservedSource {
            id: source.id.clone(),
            display_name: source.display_name.clone(),
            kind: source.kind,
            locator: source.locator.clone(),
        }),
        managed_directory: Some(paths.bundle_directory(bundle_id).display().to_string()),
        preserved_external_paths,
        warnings: vec![
            "确认后会删除这个 managed Bundle 及其 managed Mount；关联 Source 和外部目录保持不变"
                .to_owned(),
        ],
        created_at,
        expires_at,
    };
    let mut member_ids = bundle
        .members
        .iter()
        .map(|member| member.id.clone())
        .collect::<Vec<_>>();
    member_ids.sort();
    let sealed = SealedRemovalPlan {
        version: REMOVAL_JOURNAL_VERSION,
        plan: plan.clone(),
        project: None,
        source: source.as_ref().map(source_snapshot),
        bundle: Some(BundleSnapshot {
            id: bundle.id,
            display_name: bundle.display_name,
            managed_directory: bundle.managed_directory,
            current_target: bundle.current_target,
            source_id: bundle.source_id,
            device,
            inode,
            member_ids,
        }),
        mount_snapshots,
    };
    save_sealed_plan(storage, &sealed)?;
    lifecycle_lock.recheck(paths)?;
    Ok(plan)
}

pub(crate) fn create_bundle_mount_removal_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    bundle_id: &str,
    now: i64,
) -> Result<RemovalPlan, RemovalError> {
    ensure_target_available(storage, "bundle", bundle_id)?;
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    let bundle = storage.read_source_association_bundle(bundle_id)?;
    let mounts = bundle
        .members
        .iter()
        .flat_map(|member| member.mounts.iter().cloned())
        .collect::<Vec<_>>();
    if mounts.is_empty() {
        return Err(RemovalError::NoBundleMounts);
    }
    let plan_id = Uuid::new_v4().to_string();
    let mount_snapshots = seal_mounts(paths, storage, &plan_id, &mounts)?;
    for member in &bundle.members {
        storage.read_managed_member(&member.id)?;
    }
    let (device, inode) = read_bundle_identity(paths, lifecycle_lock.root(), bundle_id)?;
    let source = source_for_bundle(storage, bundle.source_id.as_deref())?;
    let created_at = now;
    let plan = RemovalPlan {
        id: plan_id,
        kind: RemovalKind::BundleMounts,
        target_id: bundle.id.clone(),
        target_display_name: bundle.display_name.clone(),
        members: bundle
            .members
            .iter()
            .map(|member| RemovalMemberSummary {
                id: member.id.clone(),
                skill_name: member.skill_name.clone(),
            })
            .collect(),
        mounts: mounts.iter().map(mount_summary).collect(),
        affected_bundles: vec![RemovalBundleSummary {
            id: bundle.id.clone(),
            display_name: bundle.display_name.clone(),
        }],
        preserved_source: source.as_ref().map(|source| RemovalPreservedSource {
            id: source.id.clone(),
            display_name: source.display_name.clone(),
            kind: source.kind,
            locator: source.locator.clone(),
        }),
        managed_directory: None,
        preserved_external_paths: Vec::new(),
        warnings: vec![
            "只解除这个 Bundle 的全部 Mount；Bundle、Skill、Source 和当前受管内容保持不变"
                .to_owned(),
        ],
        created_at,
        expires_at: now.saturating_add(REMOVAL_PLAN_TTL_MILLIS),
    };
    let mut member_ids = bundle
        .members
        .iter()
        .map(|member| member.id.clone())
        .collect::<Vec<_>>();
    member_ids.sort();
    let sealed = SealedRemovalPlan {
        version: REMOVAL_JOURNAL_VERSION,
        plan: plan.clone(),
        project: None,
        source: source.as_ref().map(source_snapshot),
        bundle: Some(BundleSnapshot {
            id: bundle.id,
            display_name: bundle.display_name,
            managed_directory: bundle.managed_directory,
            current_target: bundle.current_target,
            source_id: bundle.source_id,
            device,
            inode,
            member_ids,
        }),
        mount_snapshots,
    };
    save_sealed_plan(storage, &sealed)?;
    lifecycle_lock.recheck(paths)?;
    Ok(plan)
}

pub(crate) fn read_open_removal_plan(
    storage: &Storage,
) -> Result<Option<RemovalPlan>, RemovalError> {
    storage
        .read_pending_removal_plan()?
        .map(|stored| read_sealed_plan(&stored).map(|sealed| sealed.plan))
        .transpose()
}

pub(crate) fn discard_removal_plan(
    storage: &mut Storage,
    plan_id: &str,
) -> Result<RemovalKind, RemovalError> {
    let stored = storage.read_removal_plan(plan_id)?;
    let sealed = read_sealed_plan(&stored)?;
    if stored.status != "pending" {
        return Err(StorageError::RemovalPlanNotFound.into());
    }
    storage.discard_removal_plan(plan_id)?;
    Ok(sealed.plan.kind)
}

pub(crate) fn confirm_removal_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<RemovalKind, RemovalError> {
    let stored = storage.read_removal_plan(plan_id)?;
    let sealed = read_sealed_plan(&stored)?;
    if stored.status != "pending" {
        return Err(StorageError::RemovalPlanNotFound.into());
    }
    if stored.expires_at <= now {
        return Err(StorageError::RemovalPlanExpired.into());
    }
    match sealed.plan.kind {
        RemovalKind::Source => {
            confirm_source_removal(paths, storage, &stored, &sealed)?;
        }
        RemovalKind::Project | RemovalKind::Bundle | RemovalKind::BundleMounts => {
            let lifecycle_lock = acquire_lifecycle_lock(paths)?;
            lifecycle_lock.recheck(paths)?;
            validate_live_plan(paths, lifecycle_lock.root(), storage, &sealed)?;
            if sealed.plan.kind == RemovalKind::Bundle {
                let pending =
                    storage.read_pending_install_plans_for_bundle(&sealed.plan.target_id)?;
                discard_pending_install_plans(paths, &lifecycle_lock, storage, &pending)?;
                // 清理 pending snapshot 后再核对一次，确保确认仍绑定同一 Bundle。
                validate_live_plan(paths, lifecycle_lock.root(), storage, &sealed)?;
            }
            let transaction_id = Uuid::new_v4().to_string();
            let journal_relative = format!("journals/{transaction_id}.json");
            let consumed = storage.begin_removal_transaction(
                plan_id,
                &transaction_id,
                &journal_relative,
                now,
            )?;
            let mut journal = RemovalJournal {
                version: REMOVAL_JOURNAL_VERSION,
                transaction_id,
                plan_id: plan_id.to_owned(),
                plan_sha256: consumed.payload_sha256,
                kind: sealed.plan.kind,
                target_id: sealed.plan.target_id.clone(),
                phase: RemovalJournalPhase::JournalReady,
                bundle_trash_name: (sealed.plan.kind == RemovalKind::Bundle)
                    .then(|| Uuid::new_v4().to_string()),
                bundle_cleanup_manifest: None,
            };
            interrupt(
                failpoint,
                LifecycleFailpoint::AfterRemovalTransactionRecord,
                "Removal 事务行已写入",
            )?;
            write_journal(paths, lifecycle_lock.root(), &journal)?;
            storage.update_removal_phase(
                &journal.transaction_id,
                "journal_pending",
                "journal_ready",
                now,
            )?;
            execute_filesystem_removal(
                paths,
                lifecycle_lock.root(),
                storage,
                &sealed,
                &mut journal,
                now,
                failpoint,
            )?;
            lifecycle_lock.recheck(paths)?;
        }
    }
    Ok(sealed.plan.kind)
}

pub(crate) fn recover_pending_removals(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), RemovalError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    for transaction in storage.recoverable_removal_transactions()? {
        if transaction.status == "blocked" {
            continue;
        }
        let result = recover_transaction(
            paths,
            lifecycle_lock.root(),
            storage,
            &transaction,
            now,
            failpoint,
        );
        if let Err(error) = result {
            storage.block_removal_transaction(&transaction.id, &error.to_string(), now)?;
        }
        lifecycle_lock.recheck(paths)?;
    }
    Ok(())
}

fn confirm_source_removal(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    stored: &StoredRemovalPlan,
    sealed: &SealedRemovalPlan,
) -> Result<(), RemovalError> {
    let source = sealed
        .source
        .as_ref()
        .ok_or(RemovalError::PlanPreconditionChanged)?;
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    let current = storage
        .read_source_summaries()?
        .into_iter()
        .find(|candidate| candidate.id == source.id)
        .ok_or(RemovalError::PlanPreconditionChanged)?;
    if source_snapshot(&current) != *source {
        return Err(RemovalError::PlanPreconditionChanged);
    }
    let pending = storage.read_pending_source_install_plans_for_source(&source.id)?;
    discard_pending_install_plans(paths, &lifecycle_lock, storage, &pending)?;
    let current = storage
        .read_source_summaries()?
        .into_iter()
        .find(|candidate| candidate.id == source.id)
        .ok_or(RemovalError::PlanPreconditionChanged)?;
    if source_snapshot(&current) != *source {
        return Err(RemovalError::PlanPreconditionChanged);
    }
    storage.finalize_source_removal(&stored.id, &source.id, &source.canonical_identity)?;
    write_notice_from_storage(paths, lifecycle_lock.root(), storage)?;
    Ok(())
}

fn execute_filesystem_removal(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &mut Storage,
    sealed: &SealedRemovalPlan,
    journal: &mut RemovalJournal,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), RemovalError> {
    for mount in &sealed.mount_snapshots {
        isolate_managed_mount_removal(paths, mount)?;
    }
    journal.phase = RemovalJournalPhase::MountsIsolated;
    write_journal(paths, managed_root, journal)?;
    interrupt(
        failpoint,
        LifecycleFailpoint::AfterRemovalMountsJournalWrittenBeforePhase,
        "Removal Journal 已记录全部 Mount 隔离，SQLite 阶段尚未推进",
    )?;
    storage.update_removal_phase(
        &journal.transaction_id,
        "journal_ready",
        "mounts_isolated",
        now,
    )?;
    interrupt(
        failpoint,
        LifecycleFailpoint::AfterRemovalMountsIsolated,
        "Removal Mount 已隔离",
    )?;

    if sealed.plan.kind == RemovalKind::Bundle {
        isolate_bundle(paths, managed_root, sealed, journal)?;
        interrupt(
            failpoint,
            LifecycleFailpoint::AfterRemovalBundleRenamedBeforeJournal,
            "Bundle 已进入受控 Trash，Journal 阶段尚未推进",
        )?;
        journal.phase = RemovalJournalPhase::BundleIsolated;
        write_journal(paths, managed_root, journal)?;
        storage.update_removal_phase(
            &journal.transaction_id,
            "mounts_isolated",
            "bundle_isolated",
            now,
        )?;
        interrupt(
            failpoint,
            LifecycleFailpoint::AfterRemovalBundleIsolated,
            "Bundle 已进入受控 Trash",
        )?;
    }

    finalize_domain_state(storage, sealed, journal, now)?;
    journal.phase = RemovalJournalPhase::StateCommitted;
    write_journal(paths, managed_root, journal)?;
    interrupt(
        failpoint,
        LifecycleFailpoint::AfterRemovalStateCommitted,
        "Removal 领域状态已提交",
    )?;
    finish_committed_removal(paths, managed_root, storage, sealed, journal)
}

fn finalize_domain_state(
    storage: &mut Storage,
    sealed: &SealedRemovalPlan,
    journal: &RemovalJournal,
    now: i64,
) -> Result<(), RemovalError> {
    let mount_ids = sealed
        .mount_snapshots
        .iter()
        .map(|mount| mount.mount_id.clone())
        .collect::<Vec<_>>();
    match sealed.plan.kind {
        RemovalKind::Project => {
            let project = sealed
                .project
                .as_ref()
                .ok_or(RemovalError::PlanPreconditionChanged)?;
            storage.finalize_project_removal(
                &journal.transaction_id,
                &StoredProject::from(project),
                &mount_ids,
                now,
            )?;
        }
        RemovalKind::Bundle => {
            let bundle = sealed
                .bundle
                .as_ref()
                .ok_or(RemovalError::PlanPreconditionChanged)?;
            storage.finalize_bundle_removal(
                &journal.transaction_id,
                &bundle.id,
                &bundle.display_name,
                &bundle.managed_directory,
                &bundle.current_target,
                &bundle.member_ids,
                &mount_ids,
                now,
            )?;
        }
        RemovalKind::BundleMounts => {
            storage.finalize_bundle_mount_removal(
                &journal.transaction_id,
                &sealed.plan.target_id,
                &mount_ids,
                now,
            )?;
        }
        RemovalKind::Source => return Err(RemovalError::PlanPreconditionChanged),
    }
    Ok(())
}

fn finish_committed_removal(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &mut Storage,
    sealed: &SealedRemovalPlan,
    journal: &RemovalJournal,
) -> Result<(), RemovalError> {
    write_notice_from_storage(paths, managed_root, storage)?;
    for mount in &sealed.mount_snapshots {
        finalize_managed_mount_removal(paths, mount)?;
    }
    if sealed.plan.kind == RemovalKind::Bundle {
        cleanup_trash(paths, managed_root, journal)?;
    }
    remove_journal(paths, managed_root, journal)?;
    storage.forget_terminal_removal_transaction(&journal.transaction_id)?;
    Ok(())
}

fn recover_transaction(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &mut Storage,
    transaction: &StoredRemovalTransaction,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), RemovalError> {
    let stored = storage.read_removal_plan(&transaction.plan_id)?;
    let sealed = read_sealed_plan(&stored)?;
    validate_transaction_contract(transaction, &stored, &sealed)?;
    let journal_name = OsString::from(format!("{}.json", transaction.id));
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let journal_path = paths.journals_root().join(&journal_name);
    let exists = entry_metadata_at(&journals, &journal_name)
        .map_err(|source| removal_io("检查 Removal Journal", &journal_path, source))?
        .is_some();
    if !exists {
        if transaction.phase == "journal_pending" && transaction.status == "in_progress" {
            storage.abort_removal_transaction(&transaction.id, now)?;
            storage.forget_terminal_removal_transaction(&transaction.id)?;
            return Ok(());
        }
        if matches!(transaction.status.as_str(), "completed" | "aborted") {
            // Journal 只会在文件清理或回滚完成后删除；此处仅剩幂等清理 SQLite 终态。
            storage.forget_terminal_removal_transaction(&transaction.id)?;
            return Ok(());
        }
        return Err(RemovalError::RecoveryBlocked(
            "Removal 事务行存在，但持久 Journal 缺失".to_owned(),
        ));
    }
    let mut journal = read_journal_at(&journals, &journal_name, &journal_path)?;
    validate_journal_contract(&journal, transaction, &stored, &sealed)?;

    if transaction.status == "completed" || transaction.phase == "state_committed" {
        journal.phase = RemovalJournalPhase::StateCommitted;
        return finish_committed_removal(paths, managed_root, storage, &sealed, &journal);
    }
    if transaction.status == "aborted" {
        rollback_before_boundary(paths, storage, &sealed, &journal, now)?;
        remove_journal(paths, managed_root, &journal)?;
        storage.forget_terminal_removal_transaction(&transaction.id)?;
        return Ok(());
    }

    match sealed.plan.kind {
        RemovalKind::Project | RemovalKind::BundleMounts => {
            if transaction.phase == "journal_ready" {
                match journal.phase {
                    RemovalJournalPhase::JournalReady => {
                        rollback_before_boundary(paths, storage, &sealed, &journal, now)?;
                        remove_journal(paths, managed_root, &journal)?;
                        storage.forget_terminal_removal_transaction(&transaction.id)?;
                        return Ok(());
                    }
                    RemovalJournalPhase::MountsIsolated => {
                        ensure_all_mounts_isolated(paths, &sealed)?;
                        storage.update_removal_phase(
                            &transaction.id,
                            "journal_ready",
                            "mounts_isolated",
                            now,
                        )?;
                    }
                    RemovalJournalPhase::BundleIsolated | RemovalJournalPhase::StateCommitted => {
                        return Err(RemovalError::RecoveryBlocked(
                            "Mount Removal 阶段与 Journal 不一致".to_owned(),
                        ));
                    }
                }
            } else if transaction.phase != "mounts_isolated" {
                return Err(RemovalError::RecoveryBlocked(
                    "Mount Removal 阶段与 Journal 不一致".to_owned(),
                ));
            }
            finalize_domain_state(storage, &sealed, &journal, now)?;
        }
        RemovalKind::Bundle => {
            let crossed = inspect_bundle_boundary(paths, managed_root, &sealed, &journal)?;
            if !crossed {
                rollback_before_boundary(paths, storage, &sealed, &journal, now)?;
                remove_journal(paths, managed_root, &journal)?;
                storage.forget_terminal_removal_transaction(&transaction.id)?;
                return Ok(());
            }
            if journal.bundle_cleanup_manifest.is_none() {
                return Err(RemovalError::RecoveryBlocked(
                    "Bundle 已进入 Trash，但持久清理清单缺失".to_owned(),
                ));
            }
            journal.phase = RemovalJournalPhase::BundleIsolated;
            write_journal(paths, managed_root, &journal)?;
            if transaction.phase == "mounts_isolated" {
                storage.update_removal_phase(
                    &transaction.id,
                    "mounts_isolated",
                    "bundle_isolated",
                    now,
                )?;
            }
            finalize_domain_state(storage, &sealed, &journal, now)?;
        }
        RemovalKind::Source => return Err(RemovalError::PlanPreconditionChanged),
    }
    journal.phase = RemovalJournalPhase::StateCommitted;
    write_journal(paths, managed_root, &journal)?;
    interrupt(
        failpoint,
        LifecycleFailpoint::AfterRemovalStateCommitted,
        "恢复期间 Removal 领域状态已提交",
    )?;
    finish_committed_removal(paths, managed_root, storage, &sealed, &journal)
}

fn rollback_before_boundary(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    sealed: &SealedRemovalPlan,
    journal: &RemovalJournal,
    now: i64,
) -> Result<(), RemovalError> {
    for mount in sealed.mount_snapshots.iter().rev() {
        match inspect_managed_mount_removal(paths, mount)? {
            ManagedMountRemovalState::Original => {}
            ManagedMountRemovalState::Isolated => restore_managed_mount_removal(paths, mount)?,
            ManagedMountRemovalState::Ambiguous(message) => {
                return Err(RemovalError::RecoveryBlocked(message));
            }
        }
    }
    storage.abort_removal_transaction(&journal.transaction_id, now)?;
    Ok(())
}

fn ensure_all_mounts_isolated(
    paths: &ApplicationPaths,
    sealed: &SealedRemovalPlan,
) -> Result<(), RemovalError> {
    for mount in &sealed.mount_snapshots {
        match inspect_managed_mount_removal(paths, mount)? {
            ManagedMountRemovalState::Isolated => {}
            ManagedMountRemovalState::Original => {
                return Err(RemovalError::RecoveryBlocked(
                    "Removal Journal 已记录全部 Mount 隔离，但仍发现原 Mount".to_owned(),
                ));
            }
            ManagedMountRemovalState::Ambiguous(message) => {
                return Err(RemovalError::RecoveryBlocked(message));
            }
        }
    }
    Ok(())
}

fn isolate_bundle(
    paths: &ApplicationPaths,
    managed_root: &File,
    sealed: &SealedRemovalPlan,
    journal: &mut RemovalJournal,
) -> Result<(), RemovalError> {
    let bundle = sealed
        .bundle
        .as_ref()
        .ok_or(RemovalError::PlanPreconditionChanged)?;
    let trash_name = journal
        .bundle_trash_name
        .as_deref()
        .ok_or(RemovalError::PlanPreconditionChanged)?;
    let bundles = open_managed_directory_from_root(paths, managed_root, &paths.bundles_root())?;
    let trash = open_managed_directory_from_root(paths, managed_root, &paths.trash_root())?;
    ensure_exact_directory_entry(
        &bundles,
        OsStr::new(&bundle.id),
        bundle.device,
        bundle.inode,
        &paths.bundle_directory(&bundle.id),
    )?;
    if entry_metadata_at(&trash, OsStr::new(trash_name))
        .map_err(|source| removal_io("检查 Bundle Trash 目标", &paths.trash_root(), source))?
        .is_some()
    {
        return Err(RemovalError::RecoveryBlocked(
            "Bundle Trash 目标已被占用".to_owned(),
        ));
    }
    journal.bundle_cleanup_manifest = Some(capture_sealed_tree_cleanup_manifest(
        &bundles,
        OsStr::new(&bundle.id),
        &paths.bundle_directory(&bundle.id),
    )?);
    // 清理清单必须先持久化；rename 后崩溃时不能从可能已被改写的 Trash 重新推断归属。
    write_journal(paths, managed_root, journal)?;
    rename_at_no_replace(
        &bundles,
        OsStr::new(&bundle.id),
        &trash,
        OsStr::new(trash_name),
    )
    .map_err(|source| {
        removal_io(
            "隔离 managed Bundle",
            &paths.bundle_directory(&bundle.id),
            source,
        )
    })?;
    bundles
        .sync_all()
        .map_err(|source| removal_io("同步 bundles 目录", &paths.bundles_root(), source))?;
    trash
        .sync_all()
        .map_err(|source| removal_io("同步 trash 目录", &paths.trash_root(), source))?;
    Ok(())
}

fn inspect_bundle_boundary(
    paths: &ApplicationPaths,
    managed_root: &File,
    sealed: &SealedRemovalPlan,
    journal: &RemovalJournal,
) -> Result<bool, RemovalError> {
    let bundle = sealed
        .bundle
        .as_ref()
        .ok_or(RemovalError::PlanPreconditionChanged)?;
    let trash_name = journal
        .bundle_trash_name
        .as_deref()
        .ok_or(RemovalError::PlanPreconditionChanged)?;
    let bundles = open_managed_directory_from_root(paths, managed_root, &paths.bundles_root())?;
    let trash = open_managed_directory_from_root(paths, managed_root, &paths.trash_root())?;
    let source = entry_metadata_at(&bundles, OsStr::new(&bundle.id)).map_err(|source| {
        removal_io(
            "检查 Bundle 原路径",
            &paths.bundle_directory(&bundle.id),
            source,
        )
    })?;
    let destination = entry_metadata_at(&trash, OsStr::new(trash_name))
        .map_err(|source| removal_io("检查 Bundle Trash 路径", &paths.trash_root(), source))?;
    match (
        matches_identity(source.as_ref(), bundle),
        matches_identity(destination.as_ref(), bundle),
    ) {
        (true, false) if destination.is_none() => Ok(false),
        (false, true) if source.is_none() => Ok(true),
        _ => Err(RemovalError::RecoveryBlocked(
            "Bundle 原路径或 Trash 路径身份不明确".to_owned(),
        )),
    }
}

fn cleanup_trash(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &RemovalJournal,
) -> Result<(), RemovalError> {
    let trash_name = journal
        .bundle_trash_name
        .as_deref()
        .ok_or(RemovalError::PlanPreconditionChanged)?;
    let manifest = journal
        .bundle_cleanup_manifest
        .as_ref()
        .ok_or(RemovalError::PlanPreconditionChanged)?;
    let trash = open_managed_directory_from_root(paths, managed_root, &paths.trash_root())?;
    remove_sealed_tree_at_with_manifest(
        &trash,
        OsStr::new(trash_name),
        &paths.trash_root().join(trash_name),
        manifest,
    )?;
    Ok(())
}

fn validate_live_plan(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &mut Storage,
    sealed: &SealedRemovalPlan,
) -> Result<(), RemovalError> {
    if sealed.plan.kind == RemovalKind::Project {
        ensure_project_target_available(paths, storage, &sealed.plan.target_id)?;
    } else {
        let target_kind = if sealed.plan.kind == RemovalKind::BundleMounts {
            "bundle"
        } else {
            sealed.plan.kind.storage_kind()
        };
        ensure_target_available(storage, target_kind, &sealed.plan.target_id)?;
    }
    match sealed.plan.kind {
        RemovalKind::Project => {
            let expected = sealed
                .project
                .as_ref()
                .ok_or(RemovalError::PlanPreconditionChanged)?;
            if ProjectSnapshot::from(&storage.read_project(&expected.id)?) != *expected {
                return Err(RemovalError::PlanPreconditionChanged);
            }
            let current_mount_ids = storage
                .read_mounts()?
                .into_iter()
                .filter(|mount| mount.project_id.as_deref() == Some(expected.id.as_str()))
                .map(|mount| mount.id)
                .collect::<BTreeSet<_>>();
            let expected_mount_ids = sealed
                .mount_snapshots
                .iter()
                .map(|mount| mount.mount_id.clone())
                .collect::<BTreeSet<_>>();
            if current_mount_ids != expected_mount_ids {
                return Err(RemovalError::PlanPreconditionChanged);
            }
        }
        RemovalKind::Bundle | RemovalKind::BundleMounts => {
            let expected = sealed
                .bundle
                .as_ref()
                .ok_or(RemovalError::PlanPreconditionChanged)?;
            let current = storage.read_source_association_bundle(&expected.id)?;
            validate_bundle_snapshot(&current, expected)?;
            let current_mount_ids = current
                .members
                .iter()
                .flat_map(|member| member.mounts.iter().map(|mount| mount.id.clone()))
                .collect::<BTreeSet<_>>();
            let expected_mount_ids = sealed
                .mount_snapshots
                .iter()
                .map(|mount| mount.mount_id.clone())
                .collect::<BTreeSet<_>>();
            if current_mount_ids != expected_mount_ids {
                return Err(RemovalError::PlanPreconditionChanged);
            }
            let (device, inode) = read_bundle_identity(paths, managed_root, &expected.id)?;
            if (device, inode) != (expected.device, expected.inode) {
                return Err(RemovalError::PlanPreconditionChanged);
            }
        }
        RemovalKind::Source => return Err(RemovalError::PlanPreconditionChanged),
    }
    let current_mounts = storage.read_mounts()?;
    for snapshot in &sealed.mount_snapshots {
        let mount = current_mounts
            .iter()
            .find(|mount| mount.id == snapshot.mount_id)
            .ok_or(RemovalError::PlanPreconditionChanged)?;
        if seal_managed_mount_removal(paths, storage, mount, snapshot.temporary_name.clone())?
            != *snapshot
        {
            return Err(RemovalError::PlanPreconditionChanged);
        }
    }
    Ok(())
}

fn validate_bundle_snapshot(
    current: &StoredSourceAssociationBundle,
    expected: &BundleSnapshot,
) -> Result<(), RemovalError> {
    let mut member_ids = current
        .members
        .iter()
        .map(|member| member.id.clone())
        .collect::<Vec<_>>();
    member_ids.sort();
    if current.id != expected.id
        || current.display_name != expected.display_name
        || current.managed_directory != expected.managed_directory
        || current.current_target != expected.current_target
        || current.source_id != expected.source_id
        || member_ids != expected.member_ids
    {
        Err(RemovalError::PlanPreconditionChanged)
    } else {
        Ok(())
    }
}

fn validate_transaction_contract(
    transaction: &StoredRemovalTransaction,
    stored: &StoredRemovalPlan,
    sealed: &SealedRemovalPlan,
) -> Result<(), RemovalError> {
    if transaction.plan_id != stored.id
        || transaction.kind != stored.kind
        || transaction.target_id != stored.target_id
        || sealed.plan.id != stored.id
        || sealed.plan.target_id != stored.target_id
        || transaction.journal_path != format!("journals/{}.json", transaction.id)
    {
        Err(RemovalError::RecoveryBlocked(
            "Removal SQLite 事务与不可变 Plan 不一致".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn validate_journal_contract(
    journal: &RemovalJournal,
    transaction: &StoredRemovalTransaction,
    stored: &StoredRemovalPlan,
    sealed: &SealedRemovalPlan,
) -> Result<(), RemovalError> {
    if journal.version != REMOVAL_JOURNAL_VERSION
        || journal.transaction_id != transaction.id
        || journal.plan_id != transaction.plan_id
        || journal.plan_sha256 != stored.payload_sha256
        || journal.kind != sealed.plan.kind
        || journal.target_id != sealed.plan.target_id
    {
        Err(RemovalError::RecoveryBlocked(
            "Removal Journal 与 SQLite 事务不一致".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn save_sealed_plan(storage: &mut Storage, sealed: &SealedRemovalPlan) -> Result<(), RemovalError> {
    let payload = serde_json::to_string(sealed).map_err(RemovalError::InvalidPlanJson)?;
    let payload_sha256 = sha256_hex(payload.as_bytes());
    storage.save_removal_plan(NewRemovalPlan {
        id: &sealed.plan.id,
        kind: sealed.plan.kind.storage_kind(),
        target_id: &sealed.plan.target_id,
        payload_json: &payload,
        payload_sha256: &payload_sha256,
        created_at: sealed.plan.created_at,
        expires_at: sealed.plan.expires_at,
    })?;
    Ok(())
}

fn read_sealed_plan(stored: &StoredRemovalPlan) -> Result<SealedRemovalPlan, RemovalError> {
    if sha256_hex(stored.payload_json.as_bytes()) != stored.payload_sha256 {
        return Err(RemovalError::PlanPreconditionChanged);
    }
    let sealed: SealedRemovalPlan =
        serde_json::from_str(&stored.payload_json).map_err(RemovalError::InvalidPlanJson)?;
    if sealed.version != REMOVAL_JOURNAL_VERSION
        || sealed.plan.id != stored.id
        || sealed.plan.kind.storage_kind() != stored.kind
        || sealed.plan.target_id != stored.target_id
        || sealed.plan.created_at != stored.created_at
        || sealed.plan.expires_at != stored.expires_at
    {
        return Err(RemovalError::PlanPreconditionChanged);
    }
    Ok(sealed)
}

fn write_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &RemovalJournal,
) -> Result<(), RemovalError> {
    let bytes = serde_json::to_vec(journal).map_err(RemovalError::InvalidJournalJson)?;
    if bytes.len() > MAX_REMOVAL_JOURNAL_BYTES {
        return Err(RemovalError::RecoveryBlocked(
            "Removal Journal 超过安全大小限制".to_owned(),
        ));
    }
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let name = OsString::from(format!("{}.json", journal.transaction_id));
    write_atomic_at(&journals, &name, &paths.journals_root().join(&name), &bytes)?;
    Ok(())
}

fn read_journal_at(
    journals: &File,
    name: &OsStr,
    path: &Path,
) -> Result<RemovalJournal, RemovalError> {
    let mut file = open_regular_file_at(journals, name, path, false)?;
    let metadata = file
        .metadata()
        .map_err(|source| removal_io("检查 Removal Journal", path, source))?;
    if metadata.len() > MAX_REMOVAL_JOURNAL_BYTES as u64 {
        return Err(RemovalError::RecoveryBlocked(
            "Removal Journal 超过安全大小限制".to_owned(),
        ));
    }
    let mut bytes = Vec::with_capacity((metadata.len() + 1) as usize);
    Read::by_ref(&mut file)
        .take(MAX_REMOVAL_JOURNAL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| removal_io("读取 Removal Journal", path, source))?;
    if bytes.len() > MAX_REMOVAL_JOURNAL_BYTES {
        return Err(RemovalError::RecoveryBlocked(
            "Removal Journal 超过安全大小限制".to_owned(),
        ));
    }
    serde_json::from_slice(&bytes).map_err(RemovalError::InvalidJournalJson)
}

fn remove_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &RemovalJournal,
) -> Result<(), RemovalError> {
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let name = OsString::from(format!("{}.json", journal.transaction_id));
    let path = paths.journals_root().join(&name);
    if entry_metadata_at(&journals, &name)
        .map_err(|source| removal_io("检查 Removal Journal", &path, source))?
        .is_some()
    {
        unlink_at(&journals, &name, false)
            .map_err(|source| removal_io("删除 Removal Journal", &path, source))?;
    }
    journals
        .sync_all()
        .map_err(|source| removal_io("同步 journals 目录", &paths.journals_root(), source))
}

fn seal_mounts(
    paths: &ApplicationPaths,
    storage: &Storage,
    plan_id: &str,
    mounts: &[StoredMount],
) -> Result<Vec<ManagedMountRemovalSnapshot>, RemovalError> {
    let mut mounts = mounts.to_vec();
    mounts.sort_by(|left, right| {
        left.target_path
            .cmp(&right.target_path)
            .then_with(|| left.id.cmp(&right.id))
    });
    let snapshots = mounts
        .iter()
        .map(|mount| {
            seal_managed_mount_removal(
                paths,
                storage,
                mount,
                format!(".skillyard-removal-{plan_id}-{}", mount.id),
            )
            .map_err(Into::into)
        })
        .collect::<Result<Vec<_>, RemovalError>>()?;
    if snapshots.iter().any(|snapshot| {
        snapshot.target_observation.starts_with("unsafe_parent:")
            || snapshot
                .target_observation
                .starts_with("unavailable_parent:")
    }) {
        return Err(RemovalError::RecoveryBlocked(
            "无法安全检查级联删除中的 Mount 父目录".to_owned(),
        ));
    }
    Ok(snapshots)
}

fn affected_bundles(
    storage: &mut Storage,
    mounts: &[StoredMount],
) -> Result<Vec<RemovalBundleSummary>, RemovalError> {
    let bundle_ids = mounts
        .iter()
        .map(|mount| mount.bundle_id.clone())
        .collect::<BTreeSet<_>>();
    bundle_ids
        .into_iter()
        .map(|bundle_id| {
            let bundle = storage.read_source_association_bundle(&bundle_id)?;
            Ok(RemovalBundleSummary {
                id: bundle.id,
                display_name: bundle.display_name,
            })
        })
        .collect()
}

fn source_for_bundle(
    storage: &mut Storage,
    source_id: Option<&str>,
) -> Result<Option<SourceSummary>, RemovalError> {
    let Some(source_id) = source_id else {
        return Ok(None);
    };
    storage
        .read_source_summaries()?
        .into_iter()
        .find(|source| source.id == source_id)
        .ok_or_else(|| StorageError::SourceBundleStateConflict.into())
        .map(Some)
}

fn source_snapshot(source: &SourceSummary) -> SourceSnapshot {
    SourceSnapshot {
        id: source.id.clone(),
        canonical_identity: source.canonical_identity.clone(),
        display_name: source.display_name.clone(),
        kind: source.kind,
        locator: source.locator.clone(),
        bundle_id: source.bundle_id.clone(),
    }
}

fn mount_summary(mount: &StoredMount) -> MountSummary {
    MountSummary {
        id: mount.id.clone(),
        member_id: mount.member_id.clone(),
        skill_name: mount.skill_name.clone(),
        app_id: mount.app_id,
        scope: mount.scope,
        project_id: mount.project_id.clone(),
        project_display_name: mount.project_display_name.clone(),
        target_path: mount.target_path.clone(),
        expected_target: mount.expected_target.clone(),
        health: mount.health,
    }
}

fn ensure_target_available(
    storage: &Storage,
    kind: &str,
    target_id: &str,
) -> Result<(), RemovalError> {
    if storage.removal_object_is_blocked(kind, target_id)? {
        Err(RemovalError::RecoveryBlocked(
            "目标对象正等待人工恢复，不能同时删除".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn ensure_project_target_available(
    paths: &ApplicationPaths,
    storage: &Storage,
    project_id: &str,
) -> Result<(), RemovalError> {
    ensure_target_available(storage, "project", project_id)?;
    if blocked_takeover_references_project(paths, storage, project_id)? {
        Err(RemovalError::RecoveryBlocked(
            "目标对象正等待人工恢复，不能同时删除".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn read_bundle_identity(
    paths: &ApplicationPaths,
    managed_root: &File,
    bundle_id: &str,
) -> Result<(u64, u64), RemovalError> {
    let bundles = open_managed_directory_from_root(paths, managed_root, &paths.bundles_root())?;
    let path = paths.bundle_directory(bundle_id);
    let metadata = entry_metadata_at(&bundles, OsStr::new(bundle_id))
        .map_err(|source| removal_io("检查 managed Bundle", &path, source))?
        .ok_or(RemovalError::PlanPreconditionChanged)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(RemovalError::RecoveryBlocked(
            "managed Bundle 路径不是普通目录".to_owned(),
        ));
    }
    Ok((metadata.st_dev as u64, metadata.st_ino as u64))
}

fn ensure_exact_directory_entry(
    parent: &File,
    name: &OsStr,
    device: u64,
    inode: u64,
    path: &Path,
) -> Result<(), RemovalError> {
    let metadata = entry_metadata_at(parent, name)
        .map_err(|source| removal_io("检查受管目录身份", path, source))?
        .ok_or(RemovalError::PlanPreconditionChanged)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
        || metadata.st_dev as u64 != device
        || metadata.st_ino as u64 != inode
    {
        Err(RemovalError::PlanPreconditionChanged)
    } else {
        Ok(())
    }
}

fn matches_identity(metadata: Option<&libc::stat>, bundle: &BundleSnapshot) -> bool {
    metadata.is_some_and(|metadata| {
        metadata.st_mode & libc::S_IFMT == libc::S_IFDIR
            && metadata.st_dev as u64 == bundle.device
            && metadata.st_ino == bundle.inode
    })
}

fn interrupt(
    actual: LifecycleFailpoint,
    expected: LifecycleFailpoint,
    message: &'static str,
) -> Result<(), RemovalError> {
    if actual == expected {
        Err(RemovalError::SimulatedInterruption(message))
    } else {
        Ok(())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn removal_io(action: &'static str, path: &Path, source: io::Error) -> RemovalError {
    RemovalError::Io {
        action,
        path: path.display().to_string(),
        source,
    }
}

trait RemovalKindStorage {
    fn storage_kind(self) -> &'static str;
}

impl RemovalKindStorage for RemovalKind {
    fn storage_kind(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Source => "source",
            Self::Bundle => "bundle",
            Self::BundleMounts => "bundle_mounts",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn maximum_supported_tree_manifest_fits_the_removal_journal_limit() {
        let sandbox = tempdir().expect("应创建隔离目录");
        let bundle = sandbox.path().join("bundle");
        fs::create_dir(&bundle).expect("应创建模拟 Bundle");
        let suffix = "x".repeat(249);
        for index in 0..MAX_ENTRIES {
            let name = format!("{index:05}-{suffix}");
            fs::write(bundle.join(name), []).expect("应创建最大合法条目清单");
        }
        let parent = File::open(sandbox.path()).expect("应打开 Bundle 父目录");
        let manifest = capture_sealed_tree_cleanup_manifest(&parent, OsStr::new("bundle"), &bundle)
            .expect("应封存最大合法 Bundle");
        let journal = RemovalJournal {
            version: REMOVAL_JOURNAL_VERSION,
            transaction_id: Uuid::new_v4().to_string(),
            plan_id: Uuid::new_v4().to_string(),
            plan_sha256: "0".repeat(64),
            kind: RemovalKind::Bundle,
            target_id: Uuid::new_v4().to_string(),
            phase: RemovalJournalPhase::MountsIsolated,
            bundle_trash_name: Some(Uuid::new_v4().to_string()),
            bundle_cleanup_manifest: Some(manifest),
        };

        let bytes = serde_json::to_vec(&journal).expect("应序列化 Removal Journal");
        assert!(
            bytes.len() <= MAX_REMOVAL_JOURNAL_BYTES,
            "最大合法 Bundle 的清理清单必须可持久化：{} > {}",
            bytes.len(),
            MAX_REMOVAL_JOURNAL_BYTES
        );
    }
}
