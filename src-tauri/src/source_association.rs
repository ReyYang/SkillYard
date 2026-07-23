use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    domain::{
        MergeContentChoice, MountSummary, SourceAssociationConflict, SourceAssociationMember,
        SourceAssociationMemberChoice, SourceAssociationMode, SourceAssociationPlan,
        SourceMemberMappingChoice,
    },
    lifecycle::{LifecycleError, acquire_lifecycle_lock, write_notice_from_storage},
    paths::ApplicationPaths,
    storage::{
        DirectSourceAssociation, DirectSourceAssociationMember,
        DirectSourceAssociationMemberMapping, Storage, StorageError, StoredSourceAssociationBundle,
        StoredSourceAssociationPlanRow, StoredSourceInstallSource,
    },
};

const PLAN_TTL_MILLIS: i64 = 30 * 60 * 1_000;

#[derive(Debug, Error)]
pub enum SourceAssociationError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
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
    #[error("当前 Plan 需要 Bundle 归并执行器")]
    MergeExecutorRequired,
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
    let plan_choices = validate_member_choices(&selected_bundle, &source, member_choices)?;

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
    let (conflicts, blocking_issues) = build_merge_conflicts(
        mode,
        &source,
        &target_bundle,
        retiring_bundle.as_ref(),
        &plan_choices,
    );
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
        target_bundle: sealed_bundle(&target_bundle),
        retiring_bundle: retiring_bundle.as_ref().map(sealed_bundle),
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

/// Link 的确认是一个纯 SQLite 领域提交；Merge 会在同一入口由下一切片接入事务执行器。
pub(crate) fn confirm_source_association_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    plan_id: &str,
    content_choices: Vec<MergeContentChoice>,
    now: i64,
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
        return Err(SourceAssociationError::MergeExecutorRequired);
    }
    if !content_choices.is_empty() {
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

fn sealed_bundle(bundle: &StoredSourceAssociationBundle) -> SealedAssociationBundle {
    SealedAssociationBundle {
        id: bundle.id.clone(),
        display_name: bundle.display_name.clone(),
        managed_directory: bundle.managed_directory.clone(),
        current_target: bundle.current_target.clone(),
        source_id: bundle.source_id.clone(),
        adopted_marker: bundle.adopted_marker.clone(),
        members: bundle
            .members
            .iter()
            .map(|member| SealedAssociationMember {
                id: member.id.clone(),
                skill_name: member.skill_name.clone(),
                description: member.description.clone(),
                stable_relative_path: member.stable_relative_path.clone(),
                content_fingerprint: member.content_fingerprint.clone(),
                mounts: member
                    .mounts
                    .iter()
                    .map(|mount| SealedAssociationMount {
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
    let mut membership_count = BTreeMap::<String, usize>::new();
    for (_, member_ids) in &groups {
        for member_id in member_ids {
            *membership_count.entry(member_id.clone()).or_default() += 1;
        }
    }
    let mut blocking_issues = membership_count
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(member_id, _)| format!("成员 {member_id} 同时卷入多组冲突，请先调整后重新生成计划"))
        .collect::<Vec<_>>();
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

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
