use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs::{self, File},
    io,
    path::Path,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    content::{
        BundleCopyBudget, copy_single_skill_tree_into_open_directory, validate_single_skill_folder,
    },
    domain::{
        MergeContentChoice, MountSummary, SourceAssociationConflict, SourceAssociationMember,
        SourceAssociationMemberChoice, SourceAssociationMode, SourceAssociationPlan,
        SourceMemberMappingChoice,
    },
    lifecycle::{
        LifecycleError, LifecycleFailpoint, OwnedTreeCleanupManifest, acquire_lifecycle_lock,
        capture_owned_tree_cleanup_manifest, ensure_entry_absent_at, entry_metadata_at, mkdir_at,
        open_directory_at, open_managed_directory_from_root, read_link_at,
        remove_empty_directory_at, remove_owned_tree_at_with_manifest_and_hook,
        rename_at_no_replace, rename_at_replace, symlink_at, unlink_at, write_atomic_at,
        write_notice_from_storage,
    },
    mount_lifecycle::{
        MountLifecycleError, ParentLookup, TargetKind, open_mount_parent, recheck_open_parent,
        snapshot_at,
    },
    paths::ApplicationPaths,
    storage::{
        DirectSourceAssociation, DirectSourceAssociationMember,
        DirectSourceAssociationMemberMapping, FinalSourceAssociationMember,
        FinalSourceAssociationMemberMapping, FinalSourceAssociationMerge,
        FinalSourceAssociationMountAssignment, Storage, StorageError, StoredMount, StoredProject,
        StoredSourceAssociationBundle, StoredSourceAssociationBundleMember,
        StoredSourceAssociationPlanRow, StoredSourceAssociationTransaction,
        StoredSourceInstallSource,
    },
};

const PLAN_TTL_MILLIS: i64 = 30 * 60 * 1_000;
const ASSOCIATION_JOURNAL_VERSION: u32 = 1;
const MAX_ASSOCIATION_JOURNAL_BYTES: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum SourceAssociationError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error(transparent)]
    MountLifecycle(#[from] MountLifecycleError),
    #[error("要补充来源的 Bundle 必须尚未关联 Source")]
    BundleAlreadyHasSource,
    #[error("每个本地 Skill 必须且只能选择一次“对应”或“不对应”")]
    InvalidMemberChoices,
    #[error("所选 Source Skill 不是当前可用成员：{0}")]
    SourceMemberUnavailable(String),
    #[error("同一个 Source Skill 不能对应多个本地 Skill：{0}")]
    DuplicateSourceMember(String),
    #[error("来源关联 Plan 已被修改或无法解析")]
    InvalidPlanContract,
    #[error("来源关联 Plan 已过期，请重新生成")]
    PlanExpired,
    #[error("直接关联不接受内容冲突选择")]
    UnexpectedContentChoices,
    #[error("计划包含尚未解决的阻塞问题，请处理后重新生成")]
    BlockingIssues,
    #[error("每个非阻塞内容冲突必须且只能选择一个候选内容")]
    InvalidContentChoices,
    #[error("来源关联 Journal 超过安全大小限制")]
    JournalTooLarge,
    #[error("来源关联 Journal 无法解析或与 SQLite 合同不一致")]
    InvalidJournalContract,
    #[error("Bundle 归并恢复需要人工处理：{0}")]
    RecoveryBlocked(String),
    #[error("测试模拟 Bundle 归并中断：{0}")]
    SimulatedInterruption(&'static str),
    #[error("无法{action} {path}：{source}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: io::Error,
    },
}

/// 公开 Plan 保持轻量；确认所需的 Source、Bundle、成员和 Mount 快照封存在内部合同中。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SealedSourceAssociationPlan {
    plan: SourceAssociationPlan,
    source_catalog_generation: i64,
    source_marker: String,
    target_bundle: SealedAssociationBundle,
    retiring_bundle: Option<SealedAssociationBundle>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SealedAssociationBundle {
    id: String,
    display_name: String,
    managed_directory: String,
    current_target: String,
    source_id: Option<String>,
    adopted_marker: Option<String>,
    members: Vec<SealedAssociationMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SealedAssociationMember {
    id: String,
    skill_name: String,
    description: String,
    stable_relative_path: String,
    content_fingerprint: String,
    source_relative_path: Option<String>,
    mounts: Vec<SealedAssociationMount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SealedAssociationMount {
    id: String,
    member_id: String,
    bundle_id: String,
    skill_name: String,
    member_fingerprint: String,
    app_id: crate::domain::SupportedAppId,
    scope: crate::domain::MountScope,
    project_id: Option<String>,
    project_display_name: Option<String>,
    project_root_path: Option<String>,
    project_root_device: Option<u64>,
    project_root_inode: Option<u64>,
    target_path: String,
    expected_target: String,
    health: crate::domain::MountHealth,
    target_observation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum AssociationJournalPhase {
    JournalReady,
    CandidateReady,
    CurrentActivated,
    MountsApplied,
    StateCommitted,
}

impl AssociationJournalPhase {
    fn as_storage_str(self) -> &'static str {
        match self {
            Self::JournalReady => "journal_ready",
            Self::CandidateReady => "candidate_ready",
            Self::CurrentActivated => "current_activated",
            Self::MountsApplied => "mounts_applied",
            Self::StateCommitted => "state_committed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AssociationJournal {
    version: u32,
    transaction_id: String,
    plan_id: String,
    source_id: String,
    phase: AssociationJournalPhase,
    target_bundle: SealedAssociationBundle,
    retiring_bundle: SealedAssociationBundle,
    final_current_target: String,
    final_members: Vec<JournalFinalMember>,
    source_mappings: Vec<JournalSourceMapping>,
    mount_assignments: Vec<JournalMountAssignment>,
    retiring_mounts: Vec<JournalRetiringMount>,
    candidate_create_intent: bool,
    candidate_cleanup: Option<OwnedTreeCleanupManifest>,
    target_old_content_cleanup: OwnedTreeCleanupManifest,
    retiring_bundle_cleanup: OwnedTreeCleanupManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct JournalFinalMember {
    member_id: String,
    source_bundle_id: String,
    source_current_target: String,
    skill_name: String,
    description: String,
    stable_relative_path: String,
    content_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct JournalSourceMapping {
    source_relative_path: String,
    member_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct JournalMountAssignment {
    mount_id: String,
    member_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct JournalRetiringMount {
    mount: SealedAssociationMount,
    final_member_id: String,
    final_expected_target: String,
    quarantine_name: String,
    prepared_name: String,
    prepared_create_intent: bool,
    quarantine_observation: Option<String>,
    prepared_observation: Option<String>,
    published_observation: Option<String>,
}

/// 创建时只读取一个 SQLite 快照并冻结用户选择；不会触碰 Bundle 内容或 Mount。
pub(crate) fn create_source_association_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    bundle_id: &str,
    source_id: &str,
    member_choices: Vec<SourceMemberMappingChoice>,
    now: i64,
) -> Result<SourceAssociationPlan, SourceAssociationError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;

    let source = storage.read_source_install_source(source_id)?;
    let selected_bundle = storage.read_source_association_bundle(bundle_id)?;
    if selected_bundle.source_id.is_some() {
        return Err(SourceAssociationError::BundleAlreadyHasSource);
    }
    let selected_plan_choices = validate_member_choices(&selected_bundle, &source, member_choices)?;

    let linked_bundle_id = source.bundle.as_ref().map(|bundle| bundle.id.as_str());
    let linked_bundle = linked_bundle_id
        .map(|id| storage.read_source_association_bundle(id))
        .transpose()?;
    if linked_bundle
        .as_ref()
        .is_some_and(|bundle| bundle.source_id.as_deref() != Some(source_id))
    {
        return Err(SourceAssociationError::InvalidPlanContract);
    }

    let (mode, target_bundle, retiring_bundle) = match linked_bundle {
        Some(target_bundle) => (
            SourceAssociationMode::Merge,
            target_bundle,
            Some(selected_bundle),
        ),
        None => (SourceAssociationMode::Link, selected_bundle, None),
    };
    let plan_id = Uuid::new_v4().to_string();
    let expires_at = now.saturating_add(PLAN_TTL_MILLIS);
    let mut plan_choices = if mode == SourceAssociationMode::Merge {
        target_bundle
            .members
            .iter()
            .map(|member| SourceAssociationMemberChoice {
                member_id: member.id.clone(),
                skill_name: member.skill_name.clone(),
                source_relative_path: member.source_relative_path.clone(),
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    plan_choices.extend(selected_plan_choices.iter().cloned());
    let (conflicts, mut blocking_issues) = build_merge_conflicts(
        mode,
        &source,
        &target_bundle,
        retiring_bundle.as_ref(),
        &selected_plan_choices,
    );
    let (sealed_target, target_mount_issues) = seal_bundle(paths, &target_bundle)?;
    blocking_issues.extend(target_mount_issues);
    let (sealed_retiring, retiring_mount_issues) = retiring_bundle
        .as_ref()
        .map(|bundle| seal_bundle(paths, bundle))
        .transpose()?
        .map_or((None, Vec::new()), |(bundle, issues)| {
            (Some(bundle), issues)
        });
    blocking_issues.extend(retiring_mount_issues);
    blocking_issues.sort();
    blocking_issues.dedup();
    plan_choices.sort_by(|left, right| left.member_id.cmp(&right.member_id));
    let mut members = public_members(&target_bundle);
    let mut mounts = public_mounts(&target_bundle);
    if let Some(bundle) = retiring_bundle.as_ref() {
        members.extend(public_members(bundle));
        mounts.extend(public_mounts(bundle));
    }
    members.sort_by(|left, right| left.member_id.cmp(&right.member_id));
    mounts.sort_by(|left, right| left.id.cmp(&right.id));
    let public_plan = SourceAssociationPlan {
        id: plan_id,
        mode,
        source_id: source.id.clone(),
        source_display_name: source.display_name.clone(),
        target_bundle_id: target_bundle.id.clone(),
        target_bundle_display_name: target_bundle.display_name.clone(),
        retiring_bundle_id: retiring_bundle.as_ref().map(|bundle| bundle.id.clone()),
        retiring_bundle_display_name: retiring_bundle
            .as_ref()
            .map(|bundle| bundle.display_name.clone()),
        member_choices: plan_choices,
        members,
        mounts,
        conflicts,
        blocking_issues,
        created_at: now,
        expires_at,
    };
    let sealed = SealedSourceAssociationPlan {
        plan: public_plan.clone(),
        source_catalog_generation: source.catalog_generation,
        source_marker: source.catalog_marker,
        target_bundle: sealed_target,
        retiring_bundle: sealed_retiring,
    };
    let payload_json =
        serde_json::to_string(&sealed).map_err(|_| SourceAssociationError::InvalidPlanContract)?;
    let payload_sha256 = sha256_hex(payload_json.as_bytes());
    storage.save_source_association_plan(&StoredSourceAssociationPlanRow {
        id: public_plan.id.clone(),
        payload_json,
        payload_sha256,
        status: "pending".to_owned(),
        created_at: public_plan.created_at,
        expires_at: public_plan.expires_at,
    })?;
    lifecycle_lock.recheck(paths)?;
    Ok(public_plan)
}

/// Link 是纯 SQLite 提交；Merge 在同一确认入口执行可恢复的文件系统事务。
pub(crate) fn confirm_source_association_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
    content_choices: Vec<MergeContentChoice>,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), SourceAssociationError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    let sealed = read_sealed_plan(storage, plan_id)?;
    if now >= sealed.plan.expires_at {
        return Err(SourceAssociationError::PlanExpired);
    }
    if !sealed.plan.blocking_issues.is_empty() {
        return Err(SourceAssociationError::BlockingIssues);
    }
    if sealed.plan.mode == SourceAssociationMode::Merge {
        return confirm_merge(
            paths,
            &lifecycle_lock,
            storage,
            sealed,
            content_choices,
            now,
            failpoint,
        );
    }
    if !content_choices.is_empty() || failpoint != LifecycleFailpoint::None {
        return Err(SourceAssociationError::UnexpectedContentChoices);
    }
    if sealed.retiring_bundle.is_some()
        || sealed.target_bundle.id != sealed.plan.target_bundle_id
        || sealed.target_bundle.source_id.is_some()
    {
        return Err(SourceAssociationError::InvalidPlanContract);
    }

    let expected_members = sealed
        .target_bundle
        .members
        .iter()
        .map(|member| DirectSourceAssociationMember {
            member_id: &member.id,
            content_fingerprint: &member.content_fingerprint,
        })
        .collect::<Vec<_>>();
    let member_mappings = sealed
        .plan
        .member_choices
        .iter()
        .filter_map(|choice| {
            choice.source_relative_path.as_deref().map(|path| {
                DirectSourceAssociationMemberMapping {
                    member_id: &choice.member_id,
                    source_relative_path: path,
                }
            })
        })
        .collect::<Vec<_>>();
    storage.finalize_direct_source_association(DirectSourceAssociation {
        plan_id: &sealed.plan.id,
        source_id: &sealed.plan.source_id,
        source_catalog_generation: sealed.source_catalog_generation,
        source_marker: &sealed.source_marker,
        bundle_id: &sealed.target_bundle.id,
        expected_current_target: &sealed.target_bundle.current_target,
        expected_members: &expected_members,
        member_mappings: &member_mappings,
        now,
    })?;
    lifecycle_lock.recheck(paths)?;
    // SQLite 已提交后关联事实和 Plan 消费状态已经生效；说明文件只是可由启动恢复重建的投影。
    let _notice_result = write_notice_from_storage(paths, lifecycle_lock.root(), storage);
    lifecycle_lock
        .recheck(paths)
        .map_err(SourceAssociationError::from)
}

fn confirm_merge(
    paths: &ApplicationPaths,
    lifecycle_lock: &crate::lifecycle::LifecycleLock,
    storage: &mut Storage,
    sealed: SealedSourceAssociationPlan,
    content_choices: Vec<MergeContentChoice>,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), SourceAssociationError> {
    let retiring = sealed
        .retiring_bundle
        .as_ref()
        .ok_or(SourceAssociationError::InvalidPlanContract)?;
    if sealed.target_bundle.source_id.as_deref() != Some(sealed.plan.source_id.as_str())
        || retiring.source_id.is_some()
        || sealed.plan.retiring_bundle_id.as_deref() != Some(retiring.id.as_str())
    {
        return Err(SourceAssociationError::InvalidPlanContract);
    }
    let choices = validate_content_choices(&sealed.plan, content_choices)?;
    preflight_merge_filesystem(paths, lifecycle_lock.root(), &sealed)?;
    let transaction_id = Uuid::new_v4().to_string();
    let journal_relative = format!("journals/{transaction_id}.json");
    let mut journal = build_merge_journal(
        paths,
        lifecycle_lock.root(),
        &sealed,
        &transaction_id,
        &choices,
    )?;
    ensure_journal_fits(&journal)?;
    let choices_json =
        serde_json::to_string(&choices).map_err(|_| SourceAssociationError::InvalidPlanContract)?;
    let expected_target = stored_bundle_from_sealed(&sealed.target_bundle);
    let expected_retiring = stored_bundle_from_sealed(retiring);
    let source_mappings = journal
        .source_mappings
        .iter()
        .map(|mapping| FinalSourceAssociationMemberMapping {
            source_relative_path: &mapping.source_relative_path,
            member_id: &mapping.member_id,
        })
        .collect::<Vec<_>>();
    storage.begin_source_association_merge(
        &sealed.plan.id,
        &transaction_id,
        &sealed.plan.source_id,
        sealed.source_catalog_generation,
        &sealed.source_marker,
        &expected_target,
        &expected_retiring,
        &choices_json,
        &source_mappings,
        &journal_relative,
        now,
    )?;
    write_merge_journal(paths, lifecycle_lock.root(), &journal)?;
    storage.update_source_association_transaction_phase(
        &transaction_id,
        journal.phase.as_storage_str(),
        now,
    )?;
    execute_merge_forward(paths, lifecycle_lock, storage, &mut journal, now, failpoint)
}

fn validate_content_choices(
    plan: &SourceAssociationPlan,
    choices: Vec<MergeContentChoice>,
) -> Result<Vec<MergeContentChoice>, SourceAssociationError> {
    if choices.len() != plan.conflicts.len() {
        return Err(SourceAssociationError::InvalidContentChoices);
    }
    let conflicts = plan
        .conflicts
        .iter()
        .map(|conflict| (conflict.id.as_str(), conflict))
        .collect::<BTreeMap<_, _>>();
    let mut selected = BTreeMap::<String, String>::new();
    for choice in choices {
        let conflict = conflicts
            .get(choice.conflict_id.as_str())
            .ok_or(SourceAssociationError::InvalidContentChoices)?;
        if !conflict.candidate_member_ids.contains(&choice.member_id)
            || selected
                .insert(choice.conflict_id, choice.member_id)
                .is_some()
        {
            return Err(SourceAssociationError::InvalidContentChoices);
        }
    }
    if selected.len() != conflicts.len() {
        return Err(SourceAssociationError::InvalidContentChoices);
    }
    Ok(selected
        .into_iter()
        .map(|(conflict_id, member_id)| MergeContentChoice {
            conflict_id,
            member_id,
        })
        .collect())
}

fn build_merge_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    sealed: &SealedSourceAssociationPlan,
    transaction_id: &str,
    choices: &[MergeContentChoice],
) -> Result<AssociationJournal, SourceAssociationError> {
    let retiring = sealed
        .retiring_bundle
        .as_ref()
        .ok_or(SourceAssociationError::InvalidPlanContract)?;
    ensure_bundle_contract(paths, &sealed.target_bundle)?;
    ensure_bundle_contract(paths, retiring)?;

    let all_members = sealed
        .target_bundle
        .members
        .iter()
        .map(|member| (member.id.as_str(), (&sealed.target_bundle, member)))
        .chain(
            retiring
                .members
                .iter()
                .map(|member| (member.id.as_str(), (retiring, member))),
        )
        .collect::<BTreeMap<_, _>>();
    if all_members.len() != sealed.target_bundle.members.len() + retiring.members.len() {
        return Err(SourceAssociationError::InvalidPlanContract);
    }
    let choice_by_conflict = choices
        .iter()
        .map(|choice| (choice.conflict_id.as_str(), choice.member_id.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut winner_by_member = all_members
        .keys()
        .map(|member_id| ((*member_id).to_owned(), (*member_id).to_owned()))
        .collect::<BTreeMap<_, _>>();
    for conflict in &sealed.plan.conflicts {
        let winner = choice_by_conflict
            .get(conflict.id.as_str())
            .ok_or(SourceAssociationError::InvalidContentChoices)?;
        for member_id in &conflict.candidate_member_ids {
            if !all_members.contains_key(member_id.as_str()) {
                return Err(SourceAssociationError::InvalidPlanContract);
            }
            winner_by_member.insert(member_id.clone(), (*winner).to_owned());
        }
    }
    let winner_ids = winner_by_member.values().cloned().collect::<BTreeSet<_>>();
    let mut final_members = Vec::with_capacity(winner_ids.len());
    for winner_id in winner_ids {
        let (source_bundle, member) = all_members
            .get(winner_id.as_str())
            .ok_or(SourceAssociationError::InvalidPlanContract)?;
        final_members.push(JournalFinalMember {
            member_id: member.id.clone(),
            source_bundle_id: source_bundle.id.clone(),
            source_current_target: source_bundle.current_target.clone(),
            skill_name: member.skill_name.clone(),
            description: member.description.clone(),
            stable_relative_path: member.stable_relative_path.clone(),
            content_fingerprint: member.content_fingerprint.clone(),
        });
    }
    final_members.sort_by(|left, right| left.member_id.cmp(&right.member_id));
    if final_members
        .iter()
        .map(|member| member.skill_name.as_str())
        .collect::<BTreeSet<_>>()
        .len()
        != final_members.len()
    {
        return Err(SourceAssociationError::InvalidPlanContract);
    }

    let plan_mapping_by_member = sealed
        .plan
        .member_choices
        .iter()
        .map(|choice| {
            (
                choice.member_id.as_str(),
                choice.source_relative_path.as_deref(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut mappings = BTreeMap::<String, String>::new();
    for (member_id, (_, member)) in &all_members {
        let mapping = member
            .source_relative_path
            .as_deref()
            .or_else(|| plan_mapping_by_member.get(member_id).copied().flatten());
        let Some(mapping) = mapping else {
            continue;
        };
        let winner = winner_by_member
            .get(*member_id)
            .ok_or(SourceAssociationError::InvalidPlanContract)?;
        if mappings
            .insert(mapping.to_owned(), winner.clone())
            .is_some_and(|previous| previous != *winner)
        {
            return Err(SourceAssociationError::InvalidPlanContract);
        }
    }
    let source_mappings = mappings
        .into_iter()
        .map(|(source_relative_path, member_id)| JournalSourceMapping {
            source_relative_path,
            member_id,
        })
        .collect::<Vec<_>>();

    let final_by_id = final_members
        .iter()
        .map(|member| (member.member_id.as_str(), member))
        .collect::<BTreeMap<_, _>>();
    let mut mount_assignments = Vec::new();
    let mut retiring_mounts = Vec::new();
    for (bundle, member) in sealed
        .target_bundle
        .members
        .iter()
        .map(|member| (&sealed.target_bundle, member))
        .chain(retiring.members.iter().map(|member| (retiring, member)))
    {
        let winner_id = winner_by_member
            .get(&member.id)
            .ok_or(SourceAssociationError::InvalidPlanContract)?;
        let winner = final_by_id
            .get(winner_id.as_str())
            .ok_or(SourceAssociationError::InvalidPlanContract)?;
        for mount in &member.mounts {
            mount_assignments.push(JournalMountAssignment {
                mount_id: mount.id.clone(),
                member_id: winner_id.clone(),
            });
            if bundle.id == retiring.id {
                if mount.skill_name != winner.skill_name {
                    return Err(SourceAssociationError::InvalidPlanContract);
                }
                retiring_mounts.push(JournalRetiringMount {
                    mount: mount.clone(),
                    final_member_id: winner_id.clone(),
                    final_expected_target: paths
                        .data_root()
                        .join(&sealed.target_bundle.managed_directory)
                        .join("current")
                        .join(&winner.stable_relative_path)
                        .to_string_lossy()
                        .into_owned(),
                    quarantine_name: association_mount_private_name(
                        transaction_id,
                        &mount.id,
                        "old",
                    )?,
                    prepared_name: association_mount_private_name(
                        transaction_id,
                        &mount.id,
                        "new",
                    )?,
                    prepared_create_intent: false,
                    quarantine_observation: None,
                    prepared_observation: None,
                    published_observation: None,
                });
            }
        }
    }
    mount_assignments.sort_by(|left, right| left.mount_id.cmp(&right.mount_id));
    retiring_mounts.sort_by(|left, right| left.mount.id.cmp(&right.mount.id));

    let bundles = open_managed_directory_from_root(paths, managed_root, &paths.bundles_root())?;
    let target = open_directory_at(&bundles, OsStr::new(&sealed.target_bundle.id))
        .map_err(|source| association_io("打开目标 Bundle", &paths.bundles_root(), source))?;
    let contents = open_directory_at(&target, OsStr::new("contents"))
        .map_err(|source| association_io("打开目标 contents", &paths.bundles_root(), source))?;
    let old_content_name = current_content_name(&sealed.target_bundle.current_target)?;
    let target_old_content_cleanup = capture_owned_tree_cleanup_manifest(
        &contents,
        &old_content_name,
        &paths
            .data_root()
            .join(&sealed.target_bundle.managed_directory)
            .join(&sealed.target_bundle.current_target),
    )?;
    let retiring_bundle_cleanup = capture_owned_tree_cleanup_manifest(
        &bundles,
        OsStr::new(&retiring.id),
        &paths.data_root().join(&retiring.managed_directory),
    )?;

    Ok(AssociationJournal {
        version: ASSOCIATION_JOURNAL_VERSION,
        transaction_id: transaction_id.to_owned(),
        plan_id: sealed.plan.id.clone(),
        source_id: sealed.plan.source_id.clone(),
        phase: AssociationJournalPhase::JournalReady,
        target_bundle: sealed.target_bundle.clone(),
        retiring_bundle: retiring.clone(),
        final_current_target: format!("contents/{transaction_id}"),
        final_members,
        source_mappings,
        mount_assignments,
        retiring_mounts,
        candidate_create_intent: false,
        candidate_cleanup: None,
        target_old_content_cleanup,
        retiring_bundle_cleanup,
    })
}

fn execute_merge_forward(
    paths: &ApplicationPaths,
    lifecycle_lock: &crate::lifecycle::LifecycleLock,
    storage: &mut Storage,
    journal: &mut AssociationJournal,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), SourceAssociationError> {
    if journal.phase == AssociationJournalPhase::JournalReady {
        prepare_merge_candidate(paths, lifecycle_lock, journal, failpoint)?;
        journal.phase = AssociationJournalPhase::CandidateReady;
        persist_merge_phase(paths, lifecycle_lock.root(), storage, journal, now)?;
        if failpoint == LifecycleFailpoint::AfterSourceAssociationCandidatePrepared {
            return Err(SourceAssociationError::SimulatedInterruption(
                "归并候选已准备，target current 尚未切换",
            ));
        }
    }
    if journal.phase == AssociationJournalPhase::CandidateReady {
        activate_merge_candidate(paths, lifecycle_lock, journal)?;
        journal.phase = AssociationJournalPhase::CurrentActivated;
        persist_merge_phase(paths, lifecycle_lock.root(), storage, journal, now)?;
        if failpoint == LifecycleFailpoint::AfterSourceAssociationCurrentActivated {
            return Err(SourceAssociationError::SimulatedInterruption(
                "target current 已切换，Mount 与 SQLite 尚未完成",
            ));
        }
    }
    validate_merge_candidate(paths, lifecycle_lock.root(), journal)?;
    if journal.phase == AssociationJournalPhase::CurrentActivated {
        apply_retiring_mounts(paths, lifecycle_lock, journal)?;
        verify_final_mounts(paths, journal)?;
        journal.phase = AssociationJournalPhase::MountsApplied;
        persist_merge_phase(paths, lifecycle_lock.root(), storage, journal, now)?;
        if failpoint == LifecycleFailpoint::AfterSourceAssociationMountsApplied {
            return Err(SourceAssociationError::SimulatedInterruption(
                "Mount 已全部生效，归并领域状态尚未提交",
            ));
        }
    }
    if journal.phase == AssociationJournalPhase::MountsApplied {
        verify_final_mounts(paths, journal)?;
        finalize_merge_storage(storage, journal, now)?;
        journal.phase = AssociationJournalPhase::StateCommitted;
        write_merge_journal(paths, lifecycle_lock.root(), journal)?;
        if failpoint == LifecycleFailpoint::AfterSourceAssociationStateCommitted {
            return Err(SourceAssociationError::SimulatedInterruption(
                "归并领域状态已提交，破坏性清理尚未开始",
            ));
        }
    }
    cleanup_completed_merge(paths, lifecycle_lock, storage, journal)
}

fn prepare_merge_candidate(
    paths: &ApplicationPaths,
    lifecycle_lock: &crate::lifecycle::LifecycleLock,
    journal: &mut AssociationJournal,
    failpoint: LifecycleFailpoint,
) -> Result<(), SourceAssociationError> {
    preflight_journal_filesystem(paths, lifecycle_lock.root(), journal)?;
    let bundles =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?;
    let target = open_directory_at(&bundles, OsStr::new(&journal.target_bundle.id))
        .map_err(|source| association_io("打开归并目标 Bundle", &paths.bundles_root(), source))?;
    let contents = open_directory_at(&target, OsStr::new("contents"))
        .map_err(|source| association_io("打开归并目标 contents", &paths.bundles_root(), source))?;
    ensure_entry_absent_at(&contents, OsStr::new(&journal.transaction_id))
        .map_err(|source| association_io("检查归并候选目录", &paths.bundles_root(), source))?;
    // mkdir 前先持久化单次创建意图，让恢复只清理由本事务命名且仍为空的候选。
    journal.candidate_create_intent = true;
    write_merge_journal(paths, lifecycle_lock.root(), journal)?;
    mkdir_at(&contents, OsStr::new(&journal.transaction_id), 0o700)
        .map_err(|source| association_io("创建归并候选目录", &paths.bundles_root(), source))?;
    sync(
        &contents,
        "同步归并候选目录创建",
        &paths
            .data_root()
            .join(&journal.target_bundle.managed_directory)
            .join("contents"),
    )?;
    if failpoint
        == LifecycleFailpoint::AfterSourceAssociationCandidateDirectoryCreatedBeforeManifest
    {
        return Err(SourceAssociationError::SimulatedInterruption(
            "归并候选目录已创建，目录身份尚未记录",
        ));
    }
    let candidate = open_directory_at(&contents, OsStr::new(&journal.transaction_id))
        .map_err(|source| association_io("打开归并候选目录", &paths.bundles_root(), source))?;
    journal.candidate_cleanup = Some(capture_owned_tree_cleanup_manifest(
        &contents,
        OsStr::new(&journal.transaction_id),
        &paths
            .data_root()
            .join(&journal.target_bundle.managed_directory)
            .join(&journal.final_current_target),
    )?);
    write_merge_journal(paths, lifecycle_lock.root(), journal)?;
    mkdir_at(&candidate, OsStr::new("members"), 0o700)
        .map_err(|source| association_io("创建归并成员目录", &paths.bundles_root(), source))?;
    let members = open_directory_at(&candidate, OsStr::new("members"))
        .map_err(|source| association_io("打开归并成员目录", &paths.bundles_root(), source))?;
    let members_path = paths
        .data_root()
        .join(&journal.target_bundle.managed_directory)
        .join(&journal.final_current_target)
        .join("members");
    let mut budget = BundleCopyBudget::production();
    for member in &journal.final_members {
        let source_bundle = journal_bundle(journal, &member.source_bundle_id)?;
        verify_bundle_current(paths, lifecycle_lock.root(), source_bundle)?;
        let source_path = paths
            .data_root()
            .join(&source_bundle.managed_directory)
            .join("current")
            .join(&member.stable_relative_path);
        copy_single_skill_tree_into_open_directory(
            &source_path,
            &members,
            &members_path,
            OsStr::new(&member.skill_name),
            &member.skill_name,
            &member.content_fingerprint,
            &mut budget,
        )
        .map_err(LifecycleError::from)?;
        verify_bundle_current(paths, lifecycle_lock.root(), source_bundle)?;
    }
    sync(&members, "同步归并成员目录", &members_path)?;
    sync(
        &candidate,
        "同步归并候选目录",
        &paths
            .data_root()
            .join(&journal.target_bundle.managed_directory)
            .join(&journal.final_current_target),
    )?;
    sync(
        &contents,
        "同步目标 contents",
        &paths
            .data_root()
            .join(&journal.target_bundle.managed_directory),
    )?;
    lifecycle_lock.recheck(paths)?;
    validate_merge_candidate(paths, lifecycle_lock.root(), journal)
}

fn activate_merge_candidate(
    paths: &ApplicationPaths,
    lifecycle_lock: &crate::lifecycle::LifecycleLock,
    journal: &AssociationJournal,
) -> Result<(), SourceAssociationError> {
    validate_merge_candidate(paths, lifecycle_lock.root(), journal)?;
    let bundles =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?;
    let target = open_directory_at(&bundles, OsStr::new(&journal.target_bundle.id))
        .map_err(|source| association_io("打开归并目标 Bundle", &paths.bundles_root(), source))?;
    let actual = read_link_at(&target, OsStr::new("current"))
        .map_err(|source| association_io("读取归并前 current", &paths.bundles_root(), source))?;
    if actual != Path::new(&journal.target_bundle.current_target) {
        return Err(SourceAssociationError::RecoveryBlocked(format!(
            "目标 current 已被外部修改：{}",
            actual.display()
        )));
    }
    let temporary = OsString::from(format!(".current-{}", journal.transaction_id));
    ensure_entry_absent_at(&target, &temporary)
        .map_err(|source| association_io("检查归并临时 current", &paths.bundles_root(), source))?;
    symlink_at(
        Path::new(&journal.final_current_target),
        &target,
        &temporary,
    )
    .map_err(|source| association_io("创建归并临时 current", &paths.bundles_root(), source))?;
    sync(&target, "同步归并临时 current", &paths.bundles_root())?;
    lifecycle_lock.recheck(paths)?;
    rename_at_replace(&target, &temporary, &target, OsStr::new("current"))
        .map_err(|source| association_io("切换归并 current", &paths.bundles_root(), source))?;
    sync(&target, "同步归并 current", &paths.bundles_root())?;
    let activated = read_link_at(&target, OsStr::new("current"))
        .map_err(|source| association_io("验证归并 current", &paths.bundles_root(), source))?;
    if activated != Path::new(&journal.final_current_target) {
        return Err(SourceAssociationError::RecoveryBlocked(
            "目标 current 切换后无法验证".to_owned(),
        ));
    }
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

fn validate_merge_candidate(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &AssociationJournal,
) -> Result<(), SourceAssociationError> {
    let bundles = open_managed_directory_from_root(paths, managed_root, &paths.bundles_root())?;
    let target = open_directory_at(&bundles, OsStr::new(&journal.target_bundle.id))
        .map_err(|source| association_io("打开归并目标 Bundle", &paths.bundles_root(), source))?;
    let contents = open_directory_at(&target, OsStr::new("contents"))
        .map_err(|source| association_io("打开归并目标 contents", &paths.bundles_root(), source))?;
    let candidate = open_directory_at(&contents, OsStr::new(&journal.transaction_id))
        .map_err(|source| association_io("打开归并候选内容", &paths.bundles_root(), source))?;
    let members = open_directory_at(&candidate, OsStr::new("members"))
        .map_err(|source| association_io("打开归并成员目录", &paths.bundles_root(), source))?;
    let entries = crate::lifecycle::read_entry_names_from_handle(&members)?;
    let expected_names = journal
        .final_members
        .iter()
        .map(|member| member.skill_name.as_str())
        .collect::<BTreeSet<_>>();
    if entries.iter().map(String::as_str).collect::<BTreeSet<_>>() != expected_names {
        return Err(SourceAssociationError::RecoveryBlocked(
            "归并候选成员集合与 Journal 不一致".to_owned(),
        ));
    }
    for member in &journal.final_members {
        let path = paths
            .data_root()
            .join(&journal.target_bundle.managed_directory)
            .join(&journal.final_current_target)
            .join(&member.stable_relative_path);
        let validated = validate_single_skill_folder(&path).map_err(LifecycleError::from)?;
        if validated.name != member.skill_name
            || validated.fingerprint != member.content_fingerprint
        {
            return Err(SourceAssociationError::RecoveryBlocked(format!(
                "归并候选成员与 Journal 不一致：{}",
                member.skill_name
            )));
        }
    }
    Ok(())
}

fn apply_retiring_mounts(
    paths: &ApplicationPaths,
    lifecycle_lock: &crate::lifecycle::LifecycleLock,
    journal: &mut AssociationJournal,
) -> Result<(), SourceAssociationError> {
    for index in 0..journal.retiring_mounts.len() {
        apply_retiring_mount(paths, lifecycle_lock, journal, index)?;
    }
    Ok(())
}

fn apply_retiring_mount(
    paths: &ApplicationPaths,
    lifecycle_lock: &crate::lifecycle::LifecycleLock,
    journal: &mut AssociationJournal,
    index: usize,
) -> Result<(), SourceAssociationError> {
    let mut progress = journal
        .retiring_mounts
        .get(index)
        .cloned()
        .ok_or(SourceAssociationError::InvalidJournalContract)?;
    let project = sealed_mount_project_from_sealed(&progress.mount);
    let parent = match open_mount_parent(
        paths,
        progress.mount.app_id,
        progress.mount.scope,
        project.as_ref(),
        false,
    )? {
        ParentLookup::Open(parent) => parent,
        ParentLookup::Missing => {
            return Err(SourceAssociationError::RecoveryBlocked(format!(
                "待迁移 Mount 的父目录已经消失：{}",
                progress.mount.target_path
            )));
        }
    };
    if parent.path().join(&progress.mount.skill_name) != Path::new(&progress.mount.target_path) {
        return Err(SourceAssociationError::InvalidJournalContract);
    }
    let leaf = OsStr::new(&progress.mount.skill_name);
    let quarantine = OsStr::new(&progress.quarantine_name);
    let prepared = OsStr::new(&progress.prepared_name);
    lifecycle_lock.recheck(paths)?;
    recheck_open_parent(&parent)?;

    let mut current = snapshot_at(parent.directory(), leaf, &progress.mount.expected_target)?;
    let mut old = snapshot_at(
        parent.directory(),
        quarantine,
        &progress.mount.expected_target,
    )?;
    if progress.quarantine_observation.is_none() {
        if current.observation() == progress.mount.target_observation
            && old.kind() == TargetKind::Absent
        {
            rename_at_no_replace(parent.directory(), leaf, parent.directory(), quarantine)
                .map_err(|source| association_io("隔离待迁移 Mount", parent.path(), source))?;
            sync(parent.directory(), "同步待迁移 Mount 隔离", parent.path())?;
            current = snapshot_at(parent.directory(), leaf, &progress.mount.expected_target)?;
            old = snapshot_at(
                parent.directory(),
                quarantine,
                &progress.mount.expected_target,
            )?;
        }
        if current.kind() != TargetKind::Absent
            || old.observation() != progress.mount.target_observation
        {
            return Err(SourceAssociationError::RecoveryBlocked(format!(
                "待迁移 Mount 已被未知内容替换：{}",
                progress.mount.target_path
            )));
        }
        progress.quarantine_observation = Some(old.observation().to_owned());
        journal.retiring_mounts[index] = progress.clone();
        write_merge_journal(paths, lifecycle_lock.root(), journal)?;
    } else if old.observation()
        != progress
            .quarantine_observation
            .as_deref()
            .unwrap_or_default()
    {
        return Err(SourceAssociationError::RecoveryBlocked(format!(
            "Mount 隔离副本已被未知内容替换：{}",
            progress.mount.target_path
        )));
    }

    let mut staged = snapshot_at(
        parent.directory(),
        prepared,
        &progress.final_expected_target,
    )?;
    let final_snapshot = snapshot_at(parent.directory(), leaf, &progress.final_expected_target)?;
    if progress.prepared_observation.is_none() {
        if final_snapshot.kind() == TargetKind::ExpectedLink
            && staged.kind() == TargetKind::Absent
            && progress.prepared_create_intent
        {
            progress.prepared_observation = Some(final_snapshot.observation().to_owned());
            progress.published_observation = Some(final_snapshot.observation().to_owned());
        } else {
            if staged.kind() == TargetKind::Absent {
                progress.prepared_create_intent = true;
                journal.retiring_mounts[index] = progress.clone();
                write_merge_journal(paths, lifecycle_lock.root(), journal)?;
                symlink_at(
                    Path::new(&progress.final_expected_target),
                    parent.directory(),
                    prepared,
                )
                .map_err(|source| {
                    association_io("创建归并 Mount 暂存链接", parent.path(), source)
                })?;
                sync(parent.directory(), "同步归并 Mount 暂存链接", parent.path())?;
                staged = snapshot_at(
                    parent.directory(),
                    prepared,
                    &progress.final_expected_target,
                )?;
            }
            if !progress.prepared_create_intent || staged.kind() != TargetKind::ExpectedLink {
                return Err(SourceAssociationError::RecoveryBlocked(format!(
                    "归并 Mount 暂存位置无法确认归属：{}",
                    progress.mount.target_path
                )));
            }
            progress.prepared_observation = Some(staged.observation().to_owned());
        }
        journal.retiring_mounts[index] = progress.clone();
        write_merge_journal(paths, lifecycle_lock.root(), journal)?;
    }

    if progress.published_observation.is_none() {
        let prepared_observation = progress
            .prepared_observation
            .as_deref()
            .ok_or(SourceAssociationError::InvalidJournalContract)?;
        current = snapshot_at(parent.directory(), leaf, &progress.final_expected_target)?;
        staged = snapshot_at(
            parent.directory(),
            prepared,
            &progress.final_expected_target,
        )?;
        if current.kind() == TargetKind::Absent && staged.observation() == prepared_observation {
            rename_at_no_replace(parent.directory(), prepared, parent.directory(), leaf)
                .map_err(|source| association_io("发布归并 Mount", parent.path(), source))?;
            sync(parent.directory(), "同步归并 Mount", parent.path())?;
            current = snapshot_at(parent.directory(), leaf, &progress.final_expected_target)?;
            staged = snapshot_at(
                parent.directory(),
                prepared,
                &progress.final_expected_target,
            )?;
        }
        if current.observation() != prepared_observation || staged.kind() != TargetKind::Absent {
            return Err(SourceAssociationError::RecoveryBlocked(format!(
                "归并 Mount 发布后无法确认归属：{}",
                progress.mount.target_path
            )));
        }
        progress.published_observation = Some(current.observation().to_owned());
        journal.retiring_mounts[index] = progress;
        write_merge_journal(paths, lifecycle_lock.root(), journal)?;
    }
    recheck_open_parent(&parent)?;
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

fn verify_final_mounts(
    paths: &ApplicationPaths,
    journal: &AssociationJournal,
) -> Result<(), SourceAssociationError> {
    for member in &journal.target_bundle.members {
        for mount in &member.mounts {
            verify_sealed_mount(
                paths,
                mount,
                &mount.expected_target,
                &mount.target_observation,
            )?;
        }
    }
    for progress in &journal.retiring_mounts {
        let observation = progress.published_observation.as_deref().ok_or_else(|| {
            SourceAssociationError::RecoveryBlocked("归并后的 Mount 缺少精确发布记录".to_owned())
        })?;
        verify_sealed_mount(
            paths,
            &progress.mount,
            &progress.final_expected_target,
            observation,
        )?;
    }
    Ok(())
}

fn finalize_merge_storage(
    storage: &mut Storage,
    journal: &AssociationJournal,
    now: i64,
) -> Result<(), SourceAssociationError> {
    let target = stored_bundle_from_sealed(&journal.target_bundle);
    let retiring = stored_bundle_from_sealed(&journal.retiring_bundle);
    let final_members = journal
        .final_members
        .iter()
        .map(|member| FinalSourceAssociationMember {
            member_id: &member.member_id,
            skill_name: &member.skill_name,
            description: &member.description,
            stable_relative_path: &member.stable_relative_path,
            content_fingerprint: &member.content_fingerprint,
        })
        .collect::<Vec<_>>();
    let mount_assignments = journal
        .mount_assignments
        .iter()
        .map(|assignment| FinalSourceAssociationMountAssignment {
            mount_id: &assignment.mount_id,
            member_id: &assignment.member_id,
        })
        .collect::<Vec<_>>();
    let source_mappings = journal
        .source_mappings
        .iter()
        .map(|mapping| FinalSourceAssociationMemberMapping {
            source_relative_path: &mapping.source_relative_path,
            member_id: &mapping.member_id,
        })
        .collect::<Vec<_>>();
    storage.finalize_source_association_merge(FinalSourceAssociationMerge {
        transaction_id: &journal.transaction_id,
        source_id: &journal.source_id,
        expected_target_bundle: &target,
        expected_retiring_bundle: &retiring,
        final_current_target: &journal.final_current_target,
        final_members: &final_members,
        mount_assignments: &mount_assignments,
        source_mappings: &source_mappings,
        now,
    })?;
    Ok(())
}

fn cleanup_completed_merge(
    paths: &ApplicationPaths,
    lifecycle_lock: &crate::lifecycle::LifecycleLock,
    storage: &mut Storage,
    journal: &AssociationJournal,
) -> Result<(), SourceAssociationError> {
    verify_final_current(paths, lifecycle_lock.root(), journal)?;
    verify_final_mounts(paths, journal)?;
    for progress in &journal.retiring_mounts {
        let project = sealed_mount_project_from_sealed(&progress.mount);
        let parent = match open_mount_parent(
            paths,
            progress.mount.app_id,
            progress.mount.scope,
            project.as_ref(),
            false,
        )? {
            ParentLookup::Open(parent) => parent,
            ParentLookup::Missing => {
                return Err(SourceAssociationError::RecoveryBlocked(format!(
                    "Mount 清理父目录已经消失：{}",
                    progress.mount.target_path
                )));
            }
        };
        let quarantine = OsStr::new(&progress.quarantine_name);
        let old = snapshot_at(
            parent.directory(),
            quarantine,
            &progress.mount.expected_target,
        )?;
        if old.kind() != TargetKind::Absent {
            if old.observation()
                != progress
                    .quarantine_observation
                    .as_deref()
                    .unwrap_or_default()
            {
                return Err(SourceAssociationError::RecoveryBlocked(format!(
                    "Mount 隔离副本已被未知内容替换：{}",
                    progress.mount.target_path
                )));
            }
            verify_final_current(paths, lifecycle_lock.root(), journal)?;
            unlink_at(parent.directory(), quarantine, false)
                .map_err(|source| association_io("清理旧 Mount", parent.path(), source))?;
            sync(parent.directory(), "同步旧 Mount 清理", parent.path())?;
        }
        if snapshot_at(
            parent.directory(),
            OsStr::new(&progress.prepared_name),
            &progress.final_expected_target,
        )?
        .kind()
            != TargetKind::Absent
        {
            return Err(SourceAssociationError::RecoveryBlocked(format!(
                "归并 Mount 暂存链接仍然存在：{}",
                progress.mount.target_path
            )));
        }
        recheck_open_parent(&parent)?;
    }

    let bundles =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?;
    let target = open_directory_at(&bundles, OsStr::new(&journal.target_bundle.id))
        .map_err(|source| association_io("打开归并目标 Bundle", &paths.bundles_root(), source))?;
    let contents = open_directory_at(&target, OsStr::new("contents"))
        .map_err(|source| association_io("打开归并目标 contents", &paths.bundles_root(), source))?;
    let old_content_name = current_content_name(&journal.target_bundle.current_target)?;
    verify_final_current(paths, lifecycle_lock.root(), journal)?;
    remove_owned_tree_at_with_manifest_and_hook(
        &contents,
        &old_content_name,
        &paths
            .data_root()
            .join(&journal.target_bundle.managed_directory)
            .join(&journal.target_bundle.current_target),
        &journal.target_old_content_cleanup,
        &mut || {},
    )?;
    verify_final_current(paths, lifecycle_lock.root(), journal)?;
    remove_owned_tree_at_with_manifest_and_hook(
        &bundles,
        OsStr::new(&journal.retiring_bundle.id),
        &paths
            .data_root()
            .join(&journal.retiring_bundle.managed_directory),
        &journal.retiring_bundle_cleanup,
        &mut || {},
    )?;
    lifecycle_lock.recheck(paths)?;
    remove_merge_journal(paths, lifecycle_lock.root(), &journal.transaction_id)?;
    storage.forget_terminal_source_association_transaction(&journal.transaction_id)?;
    // 说明文件是可重建投影，领域提交完成后不能因它失败而否定归并。
    let _ = write_notice_from_storage(paths, lifecycle_lock.root(), storage);
    Ok(())
}

/// 启动恢复按 target current 的实际方向决定撤销未生效候选或完成已生效归并。
pub(crate) fn recover_pending_source_association_transactions(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), SourceAssociationError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    for transaction in storage.recoverable_source_association_transactions()? {
        if transaction.status == "blocked" {
            continue;
        }
        let result = recover_source_association_transaction(
            paths,
            &lifecycle_lock,
            storage,
            &transaction,
            now,
            failpoint,
        );
        if let Err(error) = result {
            // 测试中断模拟进程立即退出，不能被误记为需要人工处理的 blocked。
            if matches!(&error, SourceAssociationError::SimulatedInterruption(_)) {
                return Err(error);
            }
            let message = error.to_string();
            storage.block_source_association_transaction(&transaction.id, &message, now)?;
        }
        lifecycle_lock.recheck(paths)?;
    }
    lifecycle_lock.recheck(paths)?;
    Ok(())
}

fn recover_source_association_transaction(
    paths: &ApplicationPaths,
    lifecycle_lock: &crate::lifecycle::LifecycleLock,
    storage: &mut Storage,
    transaction: &StoredSourceAssociationTransaction,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), SourceAssociationError> {
    if transaction.journal_path != format!("journals/{}.json", transaction.id) {
        return Err(SourceAssociationError::InvalidJournalContract);
    }
    let journal_path = paths
        .journals_root()
        .join(format!("{}.json", transaction.id));
    if fs_entry_missing(&journal_path)? {
        return recover_association_without_journal(
            paths,
            lifecycle_lock,
            storage,
            transaction,
            now,
        );
    }
    let mut journal = read_merge_journal(paths, lifecycle_lock.root(), &transaction.id)?;
    validate_journal_transaction(paths, lifecycle_lock.root(), storage, &journal, transaction)?;
    let current = inspect_target_current(paths, lifecycle_lock.root(), &journal)?;
    match current {
        TargetCurrentState::Old => {
            if transaction.status == "completed"
                || matches!(
                    transaction.phase.as_str(),
                    "current_activated" | "mounts_applied" | "state_committed"
                )
            {
                return Err(SourceAssociationError::RecoveryBlocked(
                    "SQLite 已记录归并生效，但 target current 仍是旧目标".to_owned(),
                ));
            }
            cleanup_unactivated_merge(paths, lifecycle_lock, &journal)?;
            if transaction.status == "in_progress" {
                storage.abort_source_association_transaction(
                    &transaction.id,
                    Some("启动恢复确认 target current 尚未生效"),
                    now,
                )?;
            }
            remove_merge_journal(paths, lifecycle_lock.root(), &transaction.id)?;
            if failpoint
                == LifecycleFailpoint::AfterSourceAssociationRollbackJournalRemovedBeforeForget
            {
                return Err(SourceAssociationError::SimulatedInterruption(
                    "回滚 Journal 已删除，终态事务尚未清理",
                ));
            }
            storage.forget_terminal_source_association_transaction(&transaction.id)?;
            Ok(())
        }
        TargetCurrentState::New => {
            if transaction.status == "aborted" {
                return Err(SourceAssociationError::RecoveryBlocked(
                    "事务已终止但 target current 已指向新候选".to_owned(),
                ));
            }
            if transaction.status == "completed" {
                journal.phase = AssociationJournalPhase::StateCommitted;
                return cleanup_completed_merge(paths, lifecycle_lock, storage, &journal);
            }
            journal.phase = reconcile_activated_recovery_phase(storage, transaction, now)?;
            write_merge_journal(paths, lifecycle_lock.root(), &journal)?;
            execute_merge_forward(
                paths,
                lifecycle_lock,
                storage,
                &mut journal,
                now,
                LifecycleFailpoint::None,
            )
        }
        TargetCurrentState::Missing => Err(SourceAssociationError::RecoveryBlocked(
            "target current 缺失，无法判断归并方向".to_owned(),
        )),
        TargetCurrentState::Other(actual) => Err(SourceAssociationError::RecoveryBlocked(format!(
            "target current 指向第三个目标：{actual}"
        ))),
    }
}

fn recover_association_without_journal(
    paths: &ApplicationPaths,
    lifecycle_lock: &crate::lifecycle::LifecycleLock,
    storage: &mut Storage,
    transaction: &StoredSourceAssociationTransaction,
    now: i64,
) -> Result<(), SourceAssociationError> {
    let final_target = format!("contents/{}", transaction.id);
    let bundle_path = paths.bundle_directory(&transaction.target_bundle_id);
    let bundle = open_managed_directory_from_root(paths, lifecycle_lock.root(), &bundle_path)?;
    let current = read_link_at(&bundle, OsStr::new("current"))
        .map_err(|source| association_io("读取无 Journal 的 current", &bundle_path, source))?;
    if transaction.status == "aborted" {
        if !matches!(
            transaction.phase.as_str(),
            "journal_pending" | "journal_ready" | "candidate_ready"
        ) {
            return Err(SourceAssociationError::RecoveryBlocked(
                "已终止事务的阶段不可能来自 current 生效前回滚".to_owned(),
            ));
        }
        let contents = open_directory_at(&bundle, OsStr::new("contents"))
            .map_err(|source| association_io("打开目标 contents", &bundle_path, source))?;
        let candidate = entry_metadata_at(&contents, OsStr::new(&transaction.id))
            .map_err(|source| association_io("检查无 Journal 候选", &bundle_path, source))?;
        let temporary =
            entry_metadata_at(&bundle, OsStr::new(&format!(".current-{}", transaction.id)))
                .map_err(|source| {
                    association_io("检查无 Journal 临时 current", &bundle_path, source)
                })?;
        if candidate.is_some() || temporary.is_some() {
            return Err(SourceAssociationError::RecoveryBlocked(
                "已终止事务缺少 Journal，但候选或临时 current 仍存在".to_owned(),
            ));
        }
        let sealed = read_consumed_sealed_plan_for_transaction(paths, storage, transaction)?;
        let retiring = sealed
            .retiring_bundle
            .as_ref()
            .ok_or(SourceAssociationError::InvalidJournalContract)?;
        verify_bundle_current(paths, lifecycle_lock.root(), &sealed.target_bundle)?;
        verify_bundle_current(paths, lifecycle_lock.root(), retiring)?;
        lifecycle_lock.recheck(paths)?;
        storage.forget_terminal_source_association_transaction(&transaction.id)?;
        return Ok(());
    }
    if transaction.status == "completed" && current == Path::new(&final_target) {
        let contents = open_directory_at(&bundle, OsStr::new("contents"))
            .map_err(|source| association_io("打开目标 contents", &bundle_path, source))?;
        let retiring_path = paths.bundle_directory(&transaction.retiring_bundle_id);
        let retiring_missing = entry_metadata_at(
            &open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?,
            OsStr::new(&transaction.retiring_bundle_id),
        )
        .map_err(|source| association_io("检查 retiring Bundle", &retiring_path, source))?
        .is_none();
        let old_contents = crate::lifecycle::read_entry_names_from_handle(&contents)?;
        if !retiring_missing || old_contents.iter().any(|name| name != &transaction.id) {
            return Err(SourceAssociationError::RecoveryBlocked(
                "Journal 缺失时归并清理状态不完整".to_owned(),
            ));
        }
        let target = storage.read_source_association_bundle(&transaction.target_bundle_id)?;
        let (_, issues) = seal_bundle(paths, &target)?;
        if !issues.is_empty() {
            return Err(SourceAssociationError::RecoveryBlocked(
                "Journal 缺失时最终 Mount 无法验证".to_owned(),
            ));
        }
        storage.forget_terminal_source_association_transaction(&transaction.id)?;
        return Ok(());
    }
    if transaction.status == "in_progress"
        && transaction.phase == "journal_pending"
        && current
            == Path::new(
                &storage
                    .read_source_association_bundle(&transaction.target_bundle_id)?
                    .current_target,
            )
    {
        let contents = open_directory_at(&bundle, OsStr::new("contents"))
            .map_err(|source| association_io("打开目标 contents", &bundle_path, source))?;
        let candidate = entry_metadata_at(&contents, OsStr::new(&transaction.id))
            .map_err(|source| association_io("检查无 Journal 候选", &bundle_path, source))?;
        let temporary =
            entry_metadata_at(&bundle, OsStr::new(&format!(".current-{}", transaction.id)))
                .map_err(|source| {
                    association_io("检查无 Journal 临时 current", &bundle_path, source)
                })?;
        if candidate.is_none() && temporary.is_none() {
            storage.abort_source_association_transaction(
                &transaction.id,
                Some("Journal 写入前中断"),
                now,
            )?;
            storage.forget_terminal_source_association_transaction(&transaction.id)?;
            return Ok(());
        }
    }
    Err(SourceAssociationError::RecoveryBlocked(
        "来源关联 Journal 缺失且文件系统状态不能安全解释".to_owned(),
    ))
}

/// `current` 已指向新候选时，恢复只能向前补齐 SQLite 阶段，不能把已应用的 Mount 回退。
fn reconcile_activated_recovery_phase(
    storage: &mut Storage,
    transaction: &StoredSourceAssociationTransaction,
    now: i64,
) -> Result<AssociationJournalPhase, SourceAssociationError> {
    let phases = [
        "journal_pending",
        "journal_ready",
        "candidate_ready",
        "current_activated",
    ];
    let position = phases
        .iter()
        .position(|phase| *phase == transaction.phase)
        .unwrap_or(phases.len());
    if position < phases.len() {
        for phase in phases.iter().skip(position + 1) {
            storage.update_source_association_transaction_phase(&transaction.id, phase, now)?;
        }
        return Ok(AssociationJournalPhase::CurrentActivated);
    }
    match transaction.phase.as_str() {
        "mounts_applied" => Ok(AssociationJournalPhase::MountsApplied),
        _ => Err(SourceAssociationError::InvalidJournalContract),
    }
}

fn cleanup_unactivated_merge(
    paths: &ApplicationPaths,
    lifecycle_lock: &crate::lifecycle::LifecycleLock,
    journal: &AssociationJournal,
) -> Result<(), SourceAssociationError> {
    let bundles =
        open_managed_directory_from_root(paths, lifecycle_lock.root(), &paths.bundles_root())?;
    let target = open_directory_at(&bundles, OsStr::new(&journal.target_bundle.id))
        .map_err(|source| association_io("打开归并目标 Bundle", &paths.bundles_root(), source))?;
    let temporary = OsString::from(format!(".current-{}", journal.transaction_id));
    if entry_metadata_at(&target, &temporary)
        .map_err(|source| association_io("检查归并临时 current", &paths.bundles_root(), source))?
        .is_some()
    {
        let target_value = read_link_at(&target, &temporary).map_err(|source| {
            association_io("读取归并临时 current", &paths.bundles_root(), source)
        })?;
        if target_value != Path::new(&journal.final_current_target) {
            return Err(SourceAssociationError::RecoveryBlocked(
                "归并临时 current 已被未知内容替换".to_owned(),
            ));
        }
        unlink_at(&target, &temporary, false).map_err(|source| {
            association_io("清理归并临时 current", &paths.bundles_root(), source)
        })?;
        sync(&target, "同步归并临时 current 清理", &paths.bundles_root())?;
    }
    let contents = open_directory_at(&target, OsStr::new("contents"))
        .map_err(|source| association_io("打开归并目标 contents", &paths.bundles_root(), source))?;
    let candidate_present = entry_metadata_at(&contents, OsStr::new(&journal.transaction_id))
        .map_err(|source| association_io("检查归并候选", &paths.bundles_root(), source))?
        .is_some();
    if candidate_present {
        let candidate_path = paths
            .data_root()
            .join(&journal.target_bundle.managed_directory)
            .join(&journal.final_current_target);
        if let Some(manifest) = journal.candidate_cleanup.as_ref() {
            remove_owned_tree_at_with_manifest_and_hook(
                &contents,
                OsStr::new(&journal.transaction_id),
                &candidate_path,
                manifest,
                &mut || {},
            )?;
        } else if journal.candidate_create_intent {
            // 没有目录身份时只能清理仍为空的确定性候选，任何内容都会转为 blocked。
            remove_empty_directory_at(
                &contents,
                OsStr::new(&journal.transaction_id),
                &candidate_path,
            )?;
        } else {
            return Err(SourceAssociationError::RecoveryBlocked(
                "归并候选存在但 Journal 缺少创建意图和目录身份".to_owned(),
            ));
        }
    }
    verify_bundle_current(paths, lifecycle_lock.root(), &journal.target_bundle)?;
    verify_bundle_current(paths, lifecycle_lock.root(), &journal.retiring_bundle)?;
    Ok(())
}

/// 放弃只消费未确认的封存 Plan，不改变 Source、Bundle、内容或 Mount。
pub(crate) fn discard_source_association_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
) -> Result<(), SourceAssociationError> {
    let lifecycle_lock = acquire_lifecycle_lock(paths)?;
    lifecycle_lock.recheck(paths)?;
    storage.discard_source_association_plan(plan_id)?;
    lifecycle_lock
        .recheck(paths)
        .map_err(SourceAssociationError::from)
}

fn read_sealed_plan(
    storage: &Storage,
    plan_id: &str,
) -> Result<SealedSourceAssociationPlan, SourceAssociationError> {
    let row = storage.read_source_association_plan(plan_id)?;
    if row.status != "pending"
        || row.id != plan_id
        || row.payload_sha256 != sha256_hex(row.payload_json.as_bytes())
    {
        return Err(SourceAssociationError::InvalidPlanContract);
    }
    let sealed = serde_json::from_str::<SealedSourceAssociationPlan>(&row.payload_json)
        .map_err(|_| SourceAssociationError::InvalidPlanContract)?;
    if sealed.plan.id != row.id
        || sealed.plan.created_at != row.created_at
        || sealed.plan.expires_at != row.expires_at
    {
        return Err(SourceAssociationError::InvalidPlanContract);
    }
    Ok(sealed)
}

fn validate_member_choices(
    bundle: &StoredSourceAssociationBundle,
    source: &StoredSourceInstallSource,
    choices: Vec<SourceMemberMappingChoice>,
) -> Result<Vec<SourceAssociationMemberChoice>, SourceAssociationError> {
    if choices.len() != bundle.members.len() {
        return Err(SourceAssociationError::InvalidMemberChoices);
    }
    let selectable_paths = source
        .catalog_members
        .iter()
        .filter(|member| member.selectable && member.validation_errors.is_empty())
        .map(|member| member.relative_path.as_str())
        .collect::<BTreeSet<_>>();
    let mut choices_by_member = BTreeMap::new();
    let mut used_source_paths = BTreeSet::new();
    for choice in choices {
        if choices_by_member
            .insert(
                choice.member_id.clone(),
                choice.source_relative_path.clone(),
            )
            .is_some()
        {
            return Err(SourceAssociationError::InvalidMemberChoices);
        }
        if let Some(path) = choice.source_relative_path.as_deref() {
            if !selectable_paths.contains(path) {
                return Err(SourceAssociationError::SourceMemberUnavailable(
                    path.to_owned(),
                ));
            }
            if !used_source_paths.insert(path.to_owned()) {
                return Err(SourceAssociationError::DuplicateSourceMember(
                    path.to_owned(),
                ));
            }
        }
    }
    if choices_by_member.len() != bundle.members.len()
        || bundle
            .members
            .iter()
            .any(|member| !choices_by_member.contains_key(&member.id))
    {
        return Err(SourceAssociationError::InvalidMemberChoices);
    }

    Ok(bundle
        .members
        .iter()
        .map(|member| SourceAssociationMemberChoice {
            member_id: member.id.clone(),
            skill_name: member.skill_name.clone(),
            source_relative_path: choices_by_member
                .remove(&member.id)
                .expect("成员集合已在上方完整核对"),
        })
        .collect())
}

fn public_members(bundle: &StoredSourceAssociationBundle) -> Vec<SourceAssociationMember> {
    bundle
        .members
        .iter()
        .map(|member| SourceAssociationMember {
            member_id: member.id.clone(),
            bundle_id: bundle.id.clone(),
            bundle_display_name: bundle.display_name.clone(),
            skill_name: member.skill_name.clone(),
            content_fingerprint: member.content_fingerprint.clone(),
        })
        .collect()
}

fn public_mounts(bundle: &StoredSourceAssociationBundle) -> Vec<MountSummary> {
    bundle
        .members
        .iter()
        .flat_map(|member| {
            member.mounts.iter().map(|mount| MountSummary {
                id: mount.id.clone(),
                member_id: member.id.clone(),
                skill_name: member.skill_name.clone(),
                app_id: mount.app_id,
                scope: mount.scope,
                project_id: mount.project_id.clone(),
                project_display_name: mount.project_display_name.clone(),
                target_path: mount.target_path.clone(),
                expected_target: mount.expected_target.clone(),
                health: mount.health,
            })
        })
        .collect()
}

fn seal_bundle(
    paths: &ApplicationPaths,
    bundle: &StoredSourceAssociationBundle,
) -> Result<(SealedAssociationBundle, Vec<String>), SourceAssociationError> {
    let mut blocking_issues = Vec::new();
    let mut members = Vec::with_capacity(bundle.members.len());
    for member in &bundle.members {
        let mut mounts = Vec::with_capacity(member.mounts.len());
        for mount in &member.mounts {
            let project = sealed_mount_project(mount);
            let observation =
                match open_mount_parent(paths, mount.app_id, mount.scope, project.as_ref(), false)?
                {
                    ParentLookup::Missing => {
                        blocking_issues.push(format!(
                            "Mount 父目录缺失，不能安全归并：{}",
                            mount.target_path
                        ));
                        "missing_parent".to_owned()
                    }
                    ParentLookup::Open(parent) => {
                        if parent.path().join(&member.skill_name) != Path::new(&mount.target_path) {
                            blocking_issues.push(format!(
                                "Mount 路径与成员名称不一致，不能安全归并：{}",
                                mount.target_path
                            ));
                        }
                        let snapshot = snapshot_at(
                            parent.directory(),
                            OsStr::new(&member.skill_name),
                            &mount.expected_target,
                        )?;
                        if snapshot.kind() != TargetKind::ExpectedLink {
                            blocking_issues.push(format!(
                                "Mount 当前不是已确认的受管链接，不能安全归并：{}",
                                mount.target_path
                            ));
                        }
                        recheck_open_parent(&parent)?;
                        snapshot.observation().to_owned()
                    }
                };
            mounts.push(SealedAssociationMount {
                id: mount.id.clone(),
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
                health: mount.health,
                target_observation: observation,
            });
        }
        members.push(SealedAssociationMember {
            id: member.id.clone(),
            skill_name: member.skill_name.clone(),
            description: member.description.clone(),
            stable_relative_path: member.stable_relative_path.clone(),
            content_fingerprint: member.content_fingerprint.clone(),
            source_relative_path: member.source_relative_path.clone(),
            mounts,
        });
    }
    Ok((
        SealedAssociationBundle {
            id: bundle.id.clone(),
            display_name: bundle.display_name.clone(),
            managed_directory: bundle.managed_directory.clone(),
            current_target: bundle.current_target.clone(),
            source_id: bundle.source_id.clone(),
            adopted_marker: bundle.adopted_marker.clone(),
            members,
        },
        blocking_issues,
    ))
}

fn build_merge_conflicts(
    mode: SourceAssociationMode,
    source: &StoredSourceInstallSource,
    target_bundle: &StoredSourceAssociationBundle,
    retiring_bundle: Option<&StoredSourceAssociationBundle>,
    member_choices: &[SourceAssociationMemberChoice],
) -> (Vec<SourceAssociationConflict>, Vec<String>) {
    if mode == SourceAssociationMode::Link {
        return (Vec::new(), Vec::new());
    }
    let Some(retiring_bundle) = retiring_bundle else {
        return (Vec::new(), vec!["归并计划缺少待归入 Bundle".to_owned()]);
    };

    let mut groups = Vec::<(String, Vec<String>)>::new();
    let mut by_name = BTreeMap::<String, Vec<String>>::new();
    for member in target_bundle
        .members
        .iter()
        .chain(retiring_bundle.members.iter())
    {
        by_name
            .entry(member.skill_name.clone())
            .or_default()
            .push(member.id.clone());
    }
    for (skill_name, mut member_ids) in by_name {
        member_ids.sort();
        member_ids.dedup();
        if member_ids.len() > 1 {
            groups.push((format!("同名 Skill：{skill_name}"), member_ids));
        }
    }

    let target_paths = source
        .bundle
        .as_ref()
        .map(|bundle| {
            bundle
                .members
                .iter()
                .filter_map(|member| {
                    member
                        .source_relative_path
                        .as_ref()
                        .map(|path| (path.clone(), member.id.clone()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    for choice in member_choices {
        let Some(path) = choice.source_relative_path.as_ref() else {
            continue;
        };
        if let Some(target_member_id) = target_paths.get(path) {
            let mut member_ids = vec![target_member_id.clone(), choice.member_id.clone()];
            member_ids.sort();
            member_ids.dedup();
            if member_ids.len() > 1 {
                groups.push((format!("对应同一 Source Skill：{path}"), member_ids));
            }
        }
    }

    let all_members = target_bundle
        .members
        .iter()
        .chain(retiring_bundle.members.iter())
        .map(|member| (member.id.as_str(), member))
        .collect::<BTreeMap<_, _>>();
    let mut blocking_issues = Vec::new();
    for (label, member_ids) in &groups {
        if !label.starts_with("对应同一 Source Skill：") {
            continue;
        }
        let names = member_ids
            .iter()
            .filter_map(|id| all_members.get(id.as_str()))
            .map(|member| member.skill_name.as_str())
            .collect::<BTreeSet<_>>();
        let has_mount = member_ids
            .iter()
            .filter_map(|id| all_members.get(id.as_str()))
            .any(|member| !member.mounts.is_empty());
        if names.len() > 1 && has_mount {
            blocking_issues.push(format!(
                "冲突组“{label}”包含不同 Skill Name 且已有 Mount，请先移除 Mount，归并后再重新挂载"
            ));
        }
    }

    // 同名和 Source 映射可能描述同一组候选；按成员集合归一化，避免把重复描述误判成交叉冲突。
    let mut groups_by_candidates = BTreeMap::<Vec<String>, String>::new();
    for (label, candidate_member_ids) in groups {
        groups_by_candidates
            .entry(candidate_member_ids)
            .and_modify(|current_label| {
                if label < *current_label {
                    *current_label = label.clone();
                }
            })
            .or_insert(label);
    }
    let groups = groups_by_candidates
        .into_iter()
        .map(|(candidate_member_ids, label)| (label, candidate_member_ids))
        .collect::<Vec<_>>();
    let source_path_by_member = target_bundle
        .members
        .iter()
        .filter_map(|member| {
            member
                .source_relative_path
                .as_ref()
                .map(|path| (member.id.as_str(), path.as_str()))
        })
        .chain(member_choices.iter().filter_map(|choice| {
            choice
                .source_relative_path
                .as_ref()
                .map(|path| (choice.member_id.as_str(), path.as_str()))
        }))
        .collect::<BTreeMap<_, _>>();
    for (label, member_ids) in &groups {
        let source_paths = member_ids
            .iter()
            .filter_map(|member_id| source_path_by_member.get(member_id.as_str()).copied())
            .collect::<BTreeSet<_>>();
        if source_paths.len() > 1 {
            // 一个最终 Skill 只能对应一个 Source Member；1.0 不增加第二层映射冲突选择器。
            blocking_issues.push(format!(
                "冲突组“{label}”包含多个 Source Skill 对应，请将待归入成员改为“不对应”后重新生成计划"
            ));
        }
    }
    let mut membership_count = BTreeMap::<String, usize>::new();
    for (_, member_ids) in &groups {
        for member_id in member_ids {
            *membership_count.entry(member_id.clone()).or_default() += 1;
        }
    }
    blocking_issues.extend(
        membership_count
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(member_id, _)| {
                format!("成员 {member_id} 同时卷入多组冲突，请先调整后重新生成计划")
            }),
    );
    let supported_apps = [
        crate::domain::SupportedAppId::Codex,
        crate::domain::SupportedAppId::ClaudeCode,
        crate::domain::SupportedAppId::GitHubCopilot,
    ];
    for (label, member_ids) in &groups {
        for app_id in supported_apps {
            let mut has_global = false;
            let mut has_project = false;
            for member in target_bundle
                .members
                .iter()
                .chain(retiring_bundle.members.iter())
                .filter(|member| member_ids.contains(&member.id))
            {
                for mount in member.mounts.iter().filter(|mount| mount.app_id == app_id) {
                    match mount.scope {
                        crate::domain::MountScope::Global => has_global = true,
                        crate::domain::MountScope::Project => has_project = true,
                    }
                }
            }
            if has_global && has_project {
                blocking_issues.push(format!(
                    "冲突组“{label}”在 {app_id:?} 同时包含 global 与 project Mount，请先统一 scope"
                ));
            }
        }
    }
    let conflicts = groups
        .into_iter()
        .map(|(label, candidate_member_ids)| SourceAssociationConflict {
            id: Uuid::new_v4().to_string(),
            label,
            candidate_member_ids,
        })
        .collect();
    (conflicts, blocking_issues)
}

fn sealed_mount_project(mount: &StoredMount) -> Option<StoredProject> {
    match (
        mount.project_id.as_ref(),
        mount.project_display_name.as_ref(),
        mount.project_root_path.as_ref(),
        mount.project_root_device,
        mount.project_root_inode,
    ) {
        (Some(id), Some(display_name), Some(root_path), Some(root_device), Some(root_inode)) => {
            Some(StoredProject {
                id: id.clone(),
                display_name: display_name.clone(),
                root_path: root_path.clone(),
                root_device,
                root_inode,
                created_at: 0,
            })
        }
        _ => None,
    }
}

fn sealed_mount_project_from_sealed(mount: &SealedAssociationMount) -> Option<StoredProject> {
    match (
        mount.project_id.as_ref(),
        mount.project_display_name.as_ref(),
        mount.project_root_path.as_ref(),
        mount.project_root_device,
        mount.project_root_inode,
    ) {
        (Some(id), Some(display_name), Some(root_path), Some(root_device), Some(root_inode)) => {
            Some(StoredProject {
                id: id.clone(),
                display_name: display_name.clone(),
                root_path: root_path.clone(),
                root_device,
                root_inode,
                created_at: 0,
            })
        }
        _ => None,
    }
}

fn verify_sealed_mount(
    paths: &ApplicationPaths,
    mount: &SealedAssociationMount,
    expected_target: &str,
    expected_observation: &str,
) -> Result<(), SourceAssociationError> {
    let project = sealed_mount_project_from_sealed(mount);
    let parent = match open_mount_parent(paths, mount.app_id, mount.scope, project.as_ref(), false)?
    {
        ParentLookup::Open(parent) => parent,
        ParentLookup::Missing => {
            return Err(SourceAssociationError::RecoveryBlocked(format!(
                "Mount 父目录已经消失：{}",
                mount.target_path
            )));
        }
    };
    if parent.path().join(&mount.skill_name) != Path::new(&mount.target_path) {
        return Err(SourceAssociationError::InvalidJournalContract);
    }
    let snapshot = snapshot_at(
        parent.directory(),
        OsStr::new(&mount.skill_name),
        expected_target,
    )?;
    if snapshot.observation() != expected_observation {
        return Err(SourceAssociationError::RecoveryBlocked(format!(
            "Mount 已被未知内容替换：{}",
            mount.target_path
        )));
    }
    recheck_open_parent(&parent)?;
    Ok(())
}

fn preflight_merge_filesystem(
    paths: &ApplicationPaths,
    managed_root: &File,
    sealed: &SealedSourceAssociationPlan,
) -> Result<(), SourceAssociationError> {
    let retiring = sealed
        .retiring_bundle
        .as_ref()
        .ok_or(SourceAssociationError::InvalidPlanContract)?;
    verify_bundle_current(paths, managed_root, &sealed.target_bundle)?;
    verify_bundle_current(paths, managed_root, retiring)?;
    for bundle in [&sealed.target_bundle, retiring] {
        for member in &bundle.members {
            for mount in &member.mounts {
                verify_sealed_mount(
                    paths,
                    mount,
                    &mount.expected_target,
                    &mount.target_observation,
                )?;
            }
        }
    }
    Ok(())
}

fn preflight_journal_filesystem(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &AssociationJournal,
) -> Result<(), SourceAssociationError> {
    verify_bundle_current(paths, managed_root, &journal.target_bundle)?;
    verify_bundle_current(paths, managed_root, &journal.retiring_bundle)?;
    for bundle in [&journal.target_bundle, &journal.retiring_bundle] {
        for member in &bundle.members {
            for mount in &member.mounts {
                verify_sealed_mount(
                    paths,
                    mount,
                    &mount.expected_target,
                    &mount.target_observation,
                )?;
            }
        }
    }
    Ok(())
}

fn stored_bundle_from_sealed(bundle: &SealedAssociationBundle) -> StoredSourceAssociationBundle {
    StoredSourceAssociationBundle {
        id: bundle.id.clone(),
        display_name: bundle.display_name.clone(),
        managed_directory: bundle.managed_directory.clone(),
        current_target: bundle.current_target.clone(),
        source_id: bundle.source_id.clone(),
        adopted_marker: bundle.adopted_marker.clone(),
        members: bundle
            .members
            .iter()
            .map(|member| StoredSourceAssociationBundleMember {
                id: member.id.clone(),
                skill_name: member.skill_name.clone(),
                description: member.description.clone(),
                stable_relative_path: member.stable_relative_path.clone(),
                content_fingerprint: member.content_fingerprint.clone(),
                source_relative_path: member.source_relative_path.clone(),
                mounts: member
                    .mounts
                    .iter()
                    .map(|mount| StoredMount {
                        id: mount.id.clone(),
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
                        health: mount.health,
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn ensure_bundle_contract(
    _paths: &ApplicationPaths,
    bundle: &SealedAssociationBundle,
) -> Result<(), SourceAssociationError> {
    if bundle.managed_directory != format!("bundles/{}", bundle.id)
        || current_content_name(&bundle.current_target).is_err()
        || bundle
            .members
            .iter()
            .any(|member| member.stable_relative_path != format!("members/{}", member.skill_name))
    {
        return Err(SourceAssociationError::InvalidPlanContract);
    }
    Ok(())
}

fn journal_bundle<'a>(
    journal: &'a AssociationJournal,
    bundle_id: &str,
) -> Result<&'a SealedAssociationBundle, SourceAssociationError> {
    if journal.target_bundle.id == bundle_id {
        Ok(&journal.target_bundle)
    } else if journal.retiring_bundle.id == bundle_id {
        Ok(&journal.retiring_bundle)
    } else {
        Err(SourceAssociationError::InvalidJournalContract)
    }
}

fn verify_bundle_current(
    paths: &ApplicationPaths,
    managed_root: &File,
    bundle: &SealedAssociationBundle,
) -> Result<(), SourceAssociationError> {
    let bundle_path = paths.data_root().join(&bundle.managed_directory);
    let directory = open_managed_directory_from_root(paths, managed_root, &bundle_path)?;
    let target = read_link_at(&directory, OsStr::new("current"))
        .map_err(|source| association_io("读取 Bundle current", &bundle_path, source))?;
    if target != Path::new(&bundle.current_target) {
        return Err(SourceAssociationError::RecoveryBlocked(format!(
            "Bundle current 已被外部修改：{}",
            bundle.managed_directory
        )));
    }
    Ok(())
}

fn current_content_name(target: &str) -> Result<OsString, SourceAssociationError> {
    let path = Path::new(target);
    let mut components = path.components();
    if components.next() != Some(std::path::Component::Normal(OsStr::new("contents"))) {
        return Err(SourceAssociationError::InvalidPlanContract);
    }
    let Some(std::path::Component::Normal(name)) = components.next() else {
        return Err(SourceAssociationError::InvalidPlanContract);
    };
    if components.next().is_some() || name.is_empty() {
        return Err(SourceAssociationError::InvalidPlanContract);
    }
    Ok(name.to_os_string())
}

fn association_mount_private_name(
    transaction_id: &str,
    mount_id: &str,
    suffix: &str,
) -> Result<String, SourceAssociationError> {
    let name = format!(".skillyard-association-{transaction_id}-{mount_id}-{suffix}");
    let mut components = Path::new(&name).components();
    if name.len() > 255
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(SourceAssociationError::InvalidJournalContract);
    }
    Ok(name)
}

fn ensure_journal_fits(journal: &AssociationJournal) -> Result<(), SourceAssociationError> {
    let bytes =
        serde_json::to_vec(journal).map_err(|_| SourceAssociationError::InvalidJournalContract)?;
    if bytes.len() > MAX_ASSOCIATION_JOURNAL_BYTES {
        Err(SourceAssociationError::JournalTooLarge)
    } else {
        Ok(())
    }
}

fn write_merge_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &AssociationJournal,
) -> Result<(), SourceAssociationError> {
    ensure_journal_fits(journal)?;
    let bytes =
        serde_json::to_vec(journal).map_err(|_| SourceAssociationError::InvalidJournalContract)?;
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    write_atomic_at(
        &journals,
        OsStr::new(&format!("{}.json", journal.transaction_id)),
        &paths
            .journals_root()
            .join(format!("{}.json", journal.transaction_id)),
        &bytes,
    )?;
    Ok(())
}

fn read_merge_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    transaction_id: &str,
) -> Result<AssociationJournal, SourceAssociationError> {
    let path = paths.journals_root().join(format!("{transaction_id}.json"));
    let metadata = fs::symlink_metadata(&path)
        .map_err(|source| association_io("检查来源关联 Journal", &path, source))?;
    if !metadata.is_file() || metadata.len() as usize > MAX_ASSOCIATION_JOURNAL_BYTES {
        return Err(SourceAssociationError::InvalidJournalContract);
    }
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let file = crate::lifecycle::open_regular_file_at(
        &journals,
        OsStr::new(&format!("{transaction_id}.json")),
        &path,
        false,
    )?;
    let journal = serde_json::from_reader::<_, AssociationJournal>(file)
        .map_err(|_| SourceAssociationError::InvalidJournalContract)?;
    ensure_journal_fits(&journal)?;
    Ok(journal)
}

fn remove_merge_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    transaction_id: &str,
) -> Result<(), SourceAssociationError> {
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())?;
    let name = OsString::from(format!("{transaction_id}.json"));
    match entry_metadata_at(&journals, &name)
        .map_err(|source| association_io("检查来源关联 Journal", &paths.journals_root(), source))?
    {
        None => sync(
            &journals,
            "同步来源关联 Journal 清理",
            &paths.journals_root(),
        ),
        Some(_) => {
            unlink_at(&journals, &name, false).map_err(|source| {
                association_io("清理来源关联 Journal", &paths.journals_root(), source)
            })?;
            sync(
                &journals,
                "同步来源关联 Journal 清理",
                &paths.journals_root(),
            )
        }
    }
}

fn persist_merge_phase(
    paths: &ApplicationPaths,
    managed_root: &File,
    storage: &mut Storage,
    journal: &AssociationJournal,
    now: i64,
) -> Result<(), SourceAssociationError> {
    write_merge_journal(paths, managed_root, journal)?;
    storage.update_source_association_transaction_phase(
        &journal.transaction_id,
        journal.phase.as_storage_str(),
        now,
    )?;
    Ok(())
}

fn validate_journal_transaction(
    paths: &ApplicationPaths,
    _managed_root: &File,
    storage: &Storage,
    journal: &AssociationJournal,
    transaction: &StoredSourceAssociationTransaction,
) -> Result<(), SourceAssociationError> {
    if journal.version != ASSOCIATION_JOURNAL_VERSION
        || journal.transaction_id != transaction.id
        || journal.plan_id != transaction.plan_id
        || journal.source_id != transaction.source_id
        || journal.target_bundle.id != transaction.target_bundle_id
        || journal.retiring_bundle.id != transaction.retiring_bundle_id
        || journal.final_current_target != format!("contents/{}", transaction.id)
        || serde_json::to_string(&journal.source_mappings)
            .map_err(|_| SourceAssociationError::InvalidJournalContract)?
            != transaction.source_mappings_json
    {
        return Err(SourceAssociationError::InvalidJournalContract);
    }
    let choices =
        serde_json::from_str::<Vec<MergeContentChoice>>(&transaction.content_choices_json)
            .map_err(|_| SourceAssociationError::InvalidJournalContract)?;
    let sealed = read_consumed_sealed_plan_for_transaction(paths, storage, transaction)?;
    let canonical_choices = validate_content_choices(&sealed.plan, choices)?;
    if serde_json::to_string(&canonical_choices)
        .map_err(|_| SourceAssociationError::InvalidJournalContract)?
        != transaction.content_choices_json
        || sealed.target_bundle != journal.target_bundle
        || sealed.retiring_bundle.as_ref() != Some(&journal.retiring_bundle)
    {
        return Err(SourceAssociationError::InvalidJournalContract);
    }
    validate_final_assignment(paths, &sealed, &canonical_choices, journal)?;

    Ok(())
}

fn read_consumed_sealed_plan_for_transaction(
    paths: &ApplicationPaths,
    storage: &Storage,
    transaction: &StoredSourceAssociationTransaction,
) -> Result<SealedSourceAssociationPlan, SourceAssociationError> {
    let row = storage.read_source_association_plan(&transaction.plan_id)?;
    if row.status != "consumed"
        || row.payload_sha256 != sha256_hex(row.payload_json.as_bytes())
        || row.id != transaction.plan_id
    {
        return Err(SourceAssociationError::InvalidJournalContract);
    }
    let sealed = serde_json::from_str::<SealedSourceAssociationPlan>(&row.payload_json)
        .map_err(|_| SourceAssociationError::InvalidJournalContract)?;
    let retiring = sealed
        .retiring_bundle
        .as_ref()
        .ok_or(SourceAssociationError::InvalidJournalContract)?;
    if sealed.plan.id != transaction.plan_id
        || sealed.plan.mode != SourceAssociationMode::Merge
        || sealed.plan.source_id != transaction.source_id
        || sealed.plan.target_bundle_id != transaction.target_bundle_id
        || sealed.plan.retiring_bundle_id.as_deref()
            != Some(transaction.retiring_bundle_id.as_str())
        || sealed.target_bundle.id != transaction.target_bundle_id
        || sealed.target_bundle.source_id.as_deref() != Some(transaction.source_id.as_str())
        || retiring.id != transaction.retiring_bundle_id
        || retiring.source_id.is_some()
    {
        return Err(SourceAssociationError::InvalidJournalContract);
    }
    ensure_bundle_contract(paths, &sealed.target_bundle)
        .map_err(|_| SourceAssociationError::InvalidJournalContract)?;
    ensure_bundle_contract(paths, retiring)
        .map_err(|_| SourceAssociationError::InvalidJournalContract)?;
    Ok(sealed)
}

fn validate_final_assignment(
    paths: &ApplicationPaths,
    sealed: &SealedSourceAssociationPlan,
    choices: &[MergeContentChoice],
    journal: &AssociationJournal,
) -> Result<(), SourceAssociationError> {
    let retiring = sealed
        .retiring_bundle
        .as_ref()
        .ok_or(SourceAssociationError::InvalidJournalContract)?;
    let all_members = sealed
        .target_bundle
        .members
        .iter()
        .map(|member| (member.id.as_str(), (&sealed.target_bundle, member)))
        .chain(
            retiring
                .members
                .iter()
                .map(|member| (member.id.as_str(), (retiring, member))),
        )
        .collect::<BTreeMap<_, _>>();
    let mut winners = all_members
        .keys()
        .map(|id| ((*id).to_owned(), (*id).to_owned()))
        .collect::<BTreeMap<_, _>>();
    let choice_by_conflict = choices
        .iter()
        .map(|choice| (choice.conflict_id.as_str(), choice.member_id.as_str()))
        .collect::<BTreeMap<_, _>>();
    for conflict in &sealed.plan.conflicts {
        let winner = choice_by_conflict
            .get(conflict.id.as_str())
            .ok_or(SourceAssociationError::InvalidJournalContract)?;
        for member_id in &conflict.candidate_member_ids {
            winners.insert(member_id.clone(), (*winner).to_owned());
        }
    }
    let expected_winners = winners.values().cloned().collect::<BTreeSet<_>>();
    let actual_winners = journal
        .final_members
        .iter()
        .map(|member| member.member_id.clone())
        .collect::<BTreeSet<_>>();
    if expected_winners != actual_winners || actual_winners.len() != journal.final_members.len() {
        return Err(SourceAssociationError::InvalidJournalContract);
    }
    for final_member in &journal.final_members {
        let (bundle, member) = all_members
            .get(final_member.member_id.as_str())
            .ok_or(SourceAssociationError::InvalidJournalContract)?;
        if final_member.source_bundle_id != bundle.id
            || final_member.source_current_target != bundle.current_target
            || final_member.skill_name != member.skill_name
            || final_member.description != member.description
            || final_member.stable_relative_path != member.stable_relative_path
            || final_member.content_fingerprint != member.content_fingerprint
        {
            return Err(SourceAssociationError::InvalidJournalContract);
        }
    }
    let expected_mounts = all_members
        .iter()
        .flat_map(|(member_id, (_, member))| {
            member.mounts.iter().map(|mount| {
                (
                    mount.id.clone(),
                    winners.get(*member_id).expect("成员已建立 winner").clone(),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let actual_mounts = journal
        .mount_assignments
        .iter()
        .map(|assignment| (assignment.mount_id.clone(), assignment.member_id.clone()))
        .collect::<BTreeMap<_, _>>();
    if expected_mounts != actual_mounts || actual_mounts.len() != journal.mount_assignments.len() {
        return Err(SourceAssociationError::InvalidJournalContract);
    }
    let retiring_mount_ids = retiring
        .members
        .iter()
        .flat_map(|member| member.mounts.iter().map(|mount| mount.id.as_str()))
        .collect::<BTreeSet<_>>();
    if journal
        .retiring_mounts
        .iter()
        .map(|progress| progress.mount.id.as_str())
        .collect::<BTreeSet<_>>()
        != retiring_mount_ids
    {
        return Err(SourceAssociationError::InvalidJournalContract);
    }

    let plan_mapping_by_member = sealed
        .plan
        .member_choices
        .iter()
        .map(|choice| {
            (
                choice.member_id.as_str(),
                choice.source_relative_path.as_deref(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut expected_mappings = BTreeMap::<String, String>::new();
    for (member_id, (_, member)) in &all_members {
        let mapping = member
            .source_relative_path
            .as_deref()
            .or_else(|| plan_mapping_by_member.get(member_id).copied().flatten());
        let Some(mapping) = mapping else {
            continue;
        };
        let winner = winners
            .get(*member_id)
            .ok_or(SourceAssociationError::InvalidJournalContract)?;
        if expected_mappings
            .insert(mapping.to_owned(), winner.clone())
            .is_some_and(|previous| previous != *winner)
        {
            return Err(SourceAssociationError::InvalidJournalContract);
        }
    }
    let expected_mappings = expected_mappings
        .into_iter()
        .map(|(source_relative_path, member_id)| JournalSourceMapping {
            source_relative_path,
            member_id,
        })
        .collect::<Vec<_>>();
    if journal.source_mappings != expected_mappings {
        return Err(SourceAssociationError::InvalidJournalContract);
    }

    let final_members = journal
        .final_members
        .iter()
        .map(|member| (member.member_id.as_str(), member))
        .collect::<BTreeMap<_, _>>();
    let progress_by_mount = journal
        .retiring_mounts
        .iter()
        .map(|progress| (progress.mount.id.as_str(), progress))
        .collect::<BTreeMap<_, _>>();
    if progress_by_mount.len() != journal.retiring_mounts.len() {
        return Err(SourceAssociationError::InvalidJournalContract);
    }
    for member in &retiring.members {
        let winner_id = winners
            .get(&member.id)
            .ok_or(SourceAssociationError::InvalidJournalContract)?;
        let winner = final_members
            .get(winner_id.as_str())
            .ok_or(SourceAssociationError::InvalidJournalContract)?;
        for mount in &member.mounts {
            let progress = progress_by_mount
                .get(mount.id.as_str())
                .ok_or(SourceAssociationError::InvalidJournalContract)?;
            let expected_target = paths
                .data_root()
                .join(&sealed.target_bundle.managed_directory)
                .join("current")
                .join(&winner.stable_relative_path);
            if progress.mount != *mount
                || progress.final_member_id != *winner_id
                || Path::new(&progress.final_expected_target) != expected_target
                || !Path::new(&progress.final_expected_target).is_absolute()
                || progress.quarantine_name
                    != association_mount_private_name(&journal.transaction_id, &mount.id, "old")?
                || progress.prepared_name
                    != association_mount_private_name(&journal.transaction_id, &mount.id, "new")?
            {
                return Err(SourceAssociationError::InvalidJournalContract);
            }
        }
    }
    Ok(())
}

enum TargetCurrentState {
    Old,
    New,
    Missing,
    Other(String),
}

fn verify_final_current(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &AssociationJournal,
) -> Result<(), SourceAssociationError> {
    match inspect_target_current(paths, managed_root, journal)? {
        TargetCurrentState::New => Ok(()),
        TargetCurrentState::Missing => Err(SourceAssociationError::RecoveryBlocked(
            "破坏性清理前 target current 已缺失".to_owned(),
        )),
        TargetCurrentState::Old => Err(SourceAssociationError::RecoveryBlocked(
            "破坏性清理前 target current 已退回旧目标".to_owned(),
        )),
        TargetCurrentState::Other(actual) => Err(SourceAssociationError::RecoveryBlocked(format!(
            "破坏性清理前 target current 已被外部修改：{actual}"
        ))),
    }
}

fn inspect_target_current(
    paths: &ApplicationPaths,
    managed_root: &File,
    journal: &AssociationJournal,
) -> Result<TargetCurrentState, SourceAssociationError> {
    let bundle_path = paths
        .data_root()
        .join(&journal.target_bundle.managed_directory);
    let bundle = open_managed_directory_from_root(paths, managed_root, &bundle_path)?;
    let metadata = entry_metadata_at(&bundle, OsStr::new("current"))
        .map_err(|source| association_io("检查 target current", &bundle_path, source))?;
    let Some(metadata) = metadata else {
        return Ok(TargetCurrentState::Missing);
    };
    if metadata.st_mode & libc::S_IFMT != libc::S_IFLNK {
        return Ok(TargetCurrentState::Other("非软链接".to_owned()));
    }
    let target = read_link_at(&bundle, OsStr::new("current"))
        .map_err(|source| association_io("读取 target current", &bundle_path, source))?;
    if target == Path::new(&journal.target_bundle.current_target) {
        Ok(TargetCurrentState::Old)
    } else if target == Path::new(&journal.final_current_target) {
        Ok(TargetCurrentState::New)
    } else {
        Ok(TargetCurrentState::Other(target.display().to_string()))
    }
}

fn fs_entry_missing(path: &Path) -> Result<bool, SourceAssociationError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(source) => Err(association_io("检查文件系统条目", path, source)),
    }
}

fn sync(file: &File, action: &'static str, path: &Path) -> Result<(), SourceAssociationError> {
    file.sync_all()
        .map_err(|source| association_io(action, path, source))
}

fn association_io(action: &'static str, path: &Path, source: io::Error) -> SourceAssociationError {
    SourceAssociationError::Io {
        action,
        path: path.display().to_string(),
        source,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
