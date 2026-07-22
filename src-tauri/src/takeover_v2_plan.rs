use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

use thiserror::Error;
use uuid::Uuid;

use crate::{
    content::{ContentValidationError, ValidatedSingleSkill, validate_single_skill_folder},
    domain::{
        InventoryLocationKind, InventoryObservation, ManagementKind, MountScope, ScanRootKey,
        SkillMetadataStatus, SupportedAppId, TakeoverIdentityBasis, TakeoverOriginDisposition,
        TakeoverTargetInitialState, TakeoverV2Origin, TakeoverV2Plan, TakeoverV2PlanRequest,
        TakeoverV2PlanStatus, TakeoverV2SharedTargetRequest, TakeoverV2Target,
    },
    git_management_evidence::{ManagementEvidenceInspection, inspect_git_head_management},
    paths::{ApplicationPaths, SupportedAppPathConfig},
    scanner::{ScanError, fingerprint_skill_root},
    storage::{Storage, StorageError, StoredProject, takeover_v2_plan_seal},
};

const TAKEOVER_V2_PLAN_TTL_MILLIS: i64 = 30 * 60 * 1_000;

#[derive(Debug, Error)]
pub enum TakeoverV2PlanError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Content(#[from] ContentValidationError),
    #[error("v2 接管请求无效：{0}")]
    InvalidRequest(&'static str),
    #[error("该 Inventory observation 不能接管：{0}")]
    Ineligible(&'static str),
    #[error("Inventory observation 已变化，请刷新后重试：{0}")]
    ObservationChanged(String),
    #[error("接管路径不安全：{0}")]
    UnsafeLocation(String),
    #[error("已登记 Project 已变化：{0}")]
    ProjectChanged(String),
    #[error("Project 管理证据无法确认：{0}")]
    ProjectManagementIndeterminate(String),
    #[error("Target 冲突：{0}")]
    TargetConflict(String),
    #[error("无法检查路径 {path}：{source}")]
    InspectPath {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("路径不能无损保存为 UTF-8：{0}")]
    NonUnicodePath(String),
}

#[derive(Clone)]
struct OriginLocation {
    app_id: Option<SupportedAppId>,
    scope: Option<MountScope>,
    project: Option<StoredProject>,
    base: PathBuf,
    parent: PathBuf,
}

struct OriginSnapshot {
    origin: TakeoverV2Origin,
    location: OriginLocation,
}

struct TargetLocation {
    app_id: SupportedAppId,
    scope: MountScope,
    project: Option<StoredProject>,
    base: PathBuf,
    parent: PathBuf,
}

/// 创建阶段只保存已封印的 pending Plan；所有文件效果留给 confirm 生命周期。
#[allow(dead_code)]
pub fn create_takeover_v2_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    request: TakeoverV2PlanRequest,
    now: i64,
) -> Result<TakeoverV2Plan, TakeoverV2PlanError> {
    let request_shape = validate_request_shape(&request)?;
    let mut snapshots = Vec::with_capacity(request.observation_ids.len());
    let mut skill_name: Option<String> = None;

    for observation_id in &request.observation_ids {
        let observation = storage.read_inventory_observation(observation_id)?;
        if skill_name
            .as_ref()
            .is_some_and(|name| name != &observation.skill_name)
        {
            return Err(TakeoverV2PlanError::InvalidRequest(
                "observationIds 必须属于同名 Skill",
            ));
        }
        skill_name.get_or_insert_with(|| observation.skill_name.clone());
        let preserved = request_shape.preserved.contains(observation_id.as_str());
        snapshots.push(snapshot_origin(paths, storage, observation, preserved)?);
    }

    validate_classified_choices(&request, &request_shape, &snapshots)?;
    let selected_origin_id = snapshots
        .iter()
        .find(|snapshot| snapshot.origin.observation_id == request.selected_observation_id)
        .map(|snapshot| snapshot.origin.id.clone())
        .ok_or(TakeoverV2PlanError::InvalidRequest(
            "selectedObservationId 不在 observationIds 中",
        ))?;
    let skill_name = skill_name.ok_or(TakeoverV2PlanError::InvalidRequest(
        "observationIds 不能为空",
    ))?;

    let bundle_id = Uuid::new_v4().to_string();
    let member_id = Uuid::new_v4().to_string();
    let content_id = Uuid::new_v4().to_string();
    let managed_directory = paths.bundle_directory(&bundle_id);
    let content_directory = managed_directory.join("contents").join(&content_id);
    let expected_target = managed_directory.join("current/members").join(&skill_name);
    let expected_target_string = path_to_string(&expected_target)?;

    let mut targets = build_preserved_targets(&snapshots, &expected_target_string)?;
    add_shared_targets(
        paths,
        &request.shared_targets,
        &snapshots,
        &expected_target_string,
        &mut targets,
    )?;
    validate_target_scopes(&targets)?;

    // 所有可能失败的目标占用检查完成后，再统一重验 Origin 和 Target 快照。
    for snapshot in &snapshots {
        verify_origin_snapshot(snapshot)?;
    }
    for target in &targets {
        verify_target_snapshot(paths, &snapshots, target, &skill_name)?;
    }

    let identity_basis = if snapshots.len() == 1 {
        TakeoverIdentityBasis::SingleOrigin
    } else {
        TakeoverIdentityBasis::UserConfirmed
    };
    let mut plan = TakeoverV2Plan {
        id: Uuid::new_v4().to_string(),
        identity_basis,
        selected_origin_id,
        bundle_id,
        member_id,
        content_id,
        bundle_display_name: skill_name.clone(),
        skill_name,
        managed_directory: path_to_string(&managed_directory)?,
        content_directory: path_to_string(&content_directory)?,
        expected_target: expected_target_string,
        origins: snapshots
            .into_iter()
            .map(|snapshot| snapshot.origin)
            .collect(),
        targets,
        created_at: now,
        expires_at: now.saturating_add(TAKEOVER_V2_PLAN_TTL_MILLIS),
        status: TakeoverV2PlanStatus::Pending,
        seal: String::new(),
    };
    plan.seal = takeover_v2_plan_seal(&plan);
    storage.save_takeover_v2_plan(&plan).map_err(Into::into)
}

struct RequestShape<'a> {
    observations: BTreeSet<&'a str>,
    preserved: BTreeSet<&'a str>,
}

fn validate_request_shape(
    request: &TakeoverV2PlanRequest,
) -> Result<RequestShape<'_>, TakeoverV2PlanError> {
    if request.observation_ids.is_empty() {
        return Err(TakeoverV2PlanError::InvalidRequest(
            "observationIds 不能为空",
        ));
    }
    let observations = request
        .observation_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if observations.len() != request.observation_ids.len() {
        return Err(TakeoverV2PlanError::InvalidRequest(
            "observationIds 不能重复",
        ));
    }
    if !observations.contains(request.selected_observation_id.as_str()) {
        return Err(TakeoverV2PlanError::InvalidRequest(
            "selectedObservationId 不在 observationIds 中",
        ));
    }
    let preserved = request
        .preserved_observation_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if preserved.len() != request.preserved_observation_ids.len()
        || !preserved.iter().all(|id| observations.contains(id))
    {
        return Err(TakeoverV2PlanError::InvalidRequest(
            "preservedObservationIds 必须唯一且包含在 observationIds 中",
        ));
    }
    let shared_keys = request
        .shared_targets
        .iter()
        .map(|target| {
            (
                target.shared_observation_id.as_str(),
                target.app_id.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();
    if shared_keys.len() != request.shared_targets.len() {
        return Err(TakeoverV2PlanError::InvalidRequest(
            "sharedTargets 不能重复",
        ));
    }
    Ok(RequestShape {
        observations,
        preserved,
    })
}

fn validate_classified_choices(
    request: &TakeoverV2PlanRequest,
    request_shape: &RequestShape<'_>,
    snapshots: &[OriginSnapshot],
) -> Result<(), TakeoverV2PlanError> {
    for id in &request.preserved_observation_ids {
        let snapshot = snapshot_by_observation(snapshots, id)?;
        if is_shared_root(snapshot.origin.root_key) {
            return Err(TakeoverV2PlanError::InvalidRequest(
                "Shared observation 不能保留原位置",
            ));
        }
    }
    for target in &request.shared_targets {
        let shared = snapshot_by_observation(snapshots, &target.shared_observation_id)?;
        if !request_shape
            .observations
            .contains(target.shared_observation_id.as_str())
            || !is_shared_root(shared.origin.root_key)
        {
            return Err(TakeoverV2PlanError::InvalidRequest(
                "sharedTargets 只能引用已包含的 Shared observation",
            ));
        }
        if !shared
            .origin
            .observation_observed_by
            .contains(&target.app_id)
        {
            return Err(TakeoverV2PlanError::InvalidRequest(
                "sharedTargets 只能投影到实际读取该 Shared 路径的应用",
            ));
        }
    }
    Ok(())
}

fn snapshot_origin(
    paths: &ApplicationPaths,
    storage: &Storage,
    observation: InventoryObservation,
    preserved: bool,
) -> Result<OriginSnapshot, TakeoverV2PlanError> {
    validate_inventory_eligibility(&observation)?;
    ensure_single_component(&observation.skill_name)?;
    let location = derive_origin_location(paths, storage, &observation)?;
    if preserved && location.app_id.is_none() {
        return Err(TakeoverV2PlanError::InvalidRequest(
            "Shared observation 不能保留原位置",
        ));
    }
    let expected_root = location.parent.join(&observation.skill_name);
    if observation.skill_root != path_to_string(&expected_root)?
        || observation.skill_file != path_to_string(&expected_root.join("SKILL.md"))?
    {
        return Err(TakeoverV2PlanError::UnsafeLocation(
            observation.skill_root.clone(),
        ));
    }
    validate_project_management(location.project.as_ref(), &observation.skill_file)?;

    let parent_before = inspect_directory_chain(&location.base, &location.parent)?;
    let root_before = inspect_real_directory(&expected_root)?;
    let observed = fingerprint_skill_root(&expected_root)
        .map_err(|error| observation_scan_error(&observation.skill_root, error))?;
    if observed != observation.observed_fingerprint {
        return Err(TakeoverV2PlanError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }
    let validated = validate_single_skill_folder(&expected_root)?;
    if validated.name != observation.skill_name {
        return Err(TakeoverV2PlanError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }
    let parent_after = inspect_directory_chain(&location.base, &location.parent)?;
    let root_after = inspect_real_directory(&expected_root)?;
    if metadata_identity(&parent_before) != metadata_identity(&parent_after)
        || metadata_identity(&root_before) != metadata_identity(&root_after)
    {
        return Err(TakeoverV2PlanError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }

    let project = location.project.as_ref();
    let origin = TakeoverV2Origin {
        id: Uuid::new_v4().to_string(),
        observation_id: observation.id,
        observation_skill_name: observation.skill_name,
        observation_declared_name: observation.declared_name,
        observation_skill_file: observation.skill_file,
        observation_location_kind: observation.location_kind,
        observation_metadata_status: observation.metadata_status,
        observation_observed_by: observation.observed_by,
        observation_fingerprint: observation.observed_fingerprint,
        root_key: observation.root_key,
        observation_stale: observation.stale,
        observation_management_kind: observation.management_kind,
        observation_management_evidence: observation.management_evidence,
        app_id: location.app_id,
        scope: location.scope,
        project_id: project.map(|value| value.id.clone()),
        project_display_name: project.map(|value| value.display_name.clone()),
        project_root_path: project.map(|value| value.root_path.clone()),
        project_root_device: project.map(|value| value.root_device),
        project_root_inode: project.map(|value| value.root_inode),
        original_path: path_to_string(&expected_root)?,
        parent_device: parent_after.dev(),
        parent_inode: parent_after.ino(),
        parent_mode: parent_after.mode(),
        original_device: root_after.dev(),
        original_inode: root_after.ino(),
        original_mode: root_after.mode(),
        content_fingerprint: validated.fingerprint,
        skill_description: validated.description,
        warnings: validated.warnings,
        final_disposition: if preserved {
            TakeoverOriginDisposition::Mount
        } else {
            TakeoverOriginDisposition::Remove
        },
    };
    Ok(OriginSnapshot { origin, location })
}

fn validate_inventory_eligibility(
    observation: &InventoryObservation,
) -> Result<(), TakeoverV2PlanError> {
    if observation.stale {
        return Err(TakeoverV2PlanError::Ineligible("观察已经过期"));
    }
    if observation.metadata_status != SkillMetadataStatus::Valid {
        return Err(TakeoverV2PlanError::Ineligible("Skill metadata 无效"));
    }
    if observation.management_kind != ManagementKind::TakeoverCandidate
        || observation.management_evidence.is_some()
    {
        return Err(TakeoverV2PlanError::Ineligible("该 Skill 已有管理证据"));
    }
    Ok(())
}

fn derive_origin_location(
    paths: &ApplicationPaths,
    storage: &Storage,
    observation: &InventoryObservation,
) -> Result<OriginLocation, TakeoverV2PlanError> {
    let (location, expected_kind, expected_observers) = match observation.root_key {
        ScanRootKey::CodexGlobal
        | ScanRootKey::ClaudeCodeGlobal
        | ScanRootKey::GitHubCopilotGlobal => {
            let config = app_config_for_root(paths, observation.root_key)?;
            (
                OriginLocation {
                    app_id: Some(config.id),
                    scope: Some(MountScope::Global),
                    project: None,
                    base: paths.home().to_path_buf(),
                    parent: config.global_root,
                },
                InventoryLocationKind::AppGlobal,
                vec![config.id],
            )
        }
        ScanRootKey::CodexProject
        | ScanRootKey::ClaudeCodeProject
        | ScanRootKey::GitHubCopilotProject => {
            let config = app_config_for_root(paths, observation.root_key)?;
            let project = read_observation_project(storage, observation)?;
            let observers = if config.id == SupportedAppId::ClaudeCode {
                vec![SupportedAppId::ClaudeCode, SupportedAppId::GitHubCopilot]
            } else {
                vec![config.id]
            };
            (
                OriginLocation {
                    app_id: Some(config.id),
                    scope: Some(MountScope::Project),
                    base: PathBuf::from(&project.root_path),
                    parent: Path::new(&project.root_path).join(config.project_relative_root),
                    project: Some(project),
                },
                InventoryLocationKind::AppProject,
                observers,
            )
        }
        ScanRootKey::SharedAgents => (
            OriginLocation {
                app_id: None,
                scope: None,
                project: None,
                base: paths.home().to_path_buf(),
                parent: paths.shared_read_only_root(),
            },
            InventoryLocationKind::SharedReadOnly,
            vec![SupportedAppId::Codex, SupportedAppId::GitHubCopilot],
        ),
        ScanRootKey::SharedAgentsProject => {
            let project = read_observation_project(storage, observation)?;
            (
                OriginLocation {
                    app_id: None,
                    scope: None,
                    base: PathBuf::from(&project.root_path),
                    parent: Path::new(&project.root_path).join(".agents/skills"),
                    project: Some(project),
                },
                InventoryLocationKind::SharedReadOnly,
                vec![SupportedAppId::Codex, SupportedAppId::GitHubCopilot],
            )
        }
    };
    let project_shape_valid = if observation.root_key.is_project() {
        observation.project_id.is_some()
    } else {
        observation.project_id.is_none()
    };
    if observation.location_kind != expected_kind
        || observation.observed_by != expected_observers
        || !project_shape_valid
    {
        return Err(TakeoverV2PlanError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }
    Ok(location)
}

fn read_observation_project(
    storage: &Storage,
    observation: &InventoryObservation,
) -> Result<StoredProject, TakeoverV2PlanError> {
    let project_id = observation
        .project_id
        .as_deref()
        .ok_or(TakeoverV2PlanError::Ineligible(
            "project observation 缺少 Project",
        ))?;
    let project = storage.read_project(project_id)?;
    validate_project_identity(&project)?;
    Ok(project)
}

fn build_preserved_targets(
    snapshots: &[OriginSnapshot],
    expected_target: &str,
) -> Result<Vec<TakeoverV2Target>, TakeoverV2PlanError> {
    snapshots
        .iter()
        .filter(|snapshot| snapshot.origin.final_disposition == TakeoverOriginDisposition::Mount)
        .map(|snapshot| {
            let origin = &snapshot.origin;
            Ok(TakeoverV2Target {
                id: Uuid::new_v4().to_string(),
                mount_id: Uuid::new_v4().to_string(),
                app_id: origin.app_id.ok_or(TakeoverV2PlanError::InvalidRequest(
                    "Shared Origin 不能保留为 Mount",
                ))?,
                scope: origin.scope.ok_or(TakeoverV2PlanError::InvalidRequest(
                    "Shared Origin 不能保留为 Mount",
                ))?,
                project_id: origin.project_id.clone(),
                project_display_name: origin.project_display_name.clone(),
                project_root_path: origin.project_root_path.clone(),
                project_root_device: origin.project_root_device,
                project_root_inode: origin.project_root_inode,
                target_path: origin.original_path.clone(),
                expected_target: expected_target.to_owned(),
                parent_device: origin.parent_device,
                parent_inode: origin.parent_inode,
                parent_mode: origin.parent_mode,
                initial_state: TakeoverTargetInitialState::OccupiedByOrigin {
                    origin_id: origin.id.clone(),
                },
            })
        })
        .collect()
}

fn add_shared_targets(
    paths: &ApplicationPaths,
    requests: &[TakeoverV2SharedTargetRequest],
    snapshots: &[OriginSnapshot],
    expected_target: &str,
    targets: &mut Vec<TakeoverV2Target>,
) -> Result<(), TakeoverV2PlanError> {
    for request in requests {
        let shared = snapshot_by_observation(snapshots, &request.shared_observation_id)?;
        let location = derive_shared_target_location(paths, shared, request.app_id)?;
        let target_path = location.parent.join(&shared.origin.observation_skill_name);
        let target_path_string = path_to_string(&target_path)?;

        if let Some(existing) = targets
            .iter()
            .find(|target| target.target_path == target_path_string)
        {
            if existing.app_id == location.app_id
                && existing.scope == location.scope
                && existing.project_id
                    == location.project.as_ref().map(|project| project.id.clone())
                && matches!(
                    existing.initial_state,
                    TakeoverTargetInitialState::OccupiedByOrigin { .. }
                )
            {
                // Shared 请求指向已保留 Origin 时复用同一 Target，避免重复 Mount。
                continue;
            }
            return Err(TakeoverV2PlanError::TargetConflict(target_path_string));
        }
        if snapshots
            .iter()
            .any(|snapshot| snapshot.origin.original_path == target_path_string)
        {
            return Err(TakeoverV2PlanError::TargetConflict(target_path_string));
        }
        let parent = inspect_absent_target(&location.base, &location.parent, &target_path)?;
        let project = location.project.as_ref();
        targets.push(TakeoverV2Target {
            id: Uuid::new_v4().to_string(),
            mount_id: Uuid::new_v4().to_string(),
            app_id: location.app_id,
            scope: location.scope,
            project_id: project.map(|value| value.id.clone()),
            project_display_name: project.map(|value| value.display_name.clone()),
            project_root_path: project.map(|value| value.root_path.clone()),
            project_root_device: project.map(|value| value.root_device),
            project_root_inode: project.map(|value| value.root_inode),
            target_path: target_path_string,
            expected_target: expected_target.to_owned(),
            parent_device: parent.dev(),
            parent_inode: parent.ino(),
            parent_mode: parent.mode(),
            initial_state: TakeoverTargetInitialState::Absent,
        });
    }
    Ok(())
}

fn derive_shared_target_location(
    paths: &ApplicationPaths,
    shared: &OriginSnapshot,
    app_id: SupportedAppId,
) -> Result<TargetLocation, TakeoverV2PlanError> {
    if !is_shared_root(shared.origin.root_key) {
        return Err(TakeoverV2PlanError::InvalidRequest(
            "sharedTargets 只能引用 Shared observation",
        ));
    }
    let config = app_config_for_id(paths, app_id)?;
    match shared.origin.root_key {
        ScanRootKey::SharedAgents => Ok(TargetLocation {
            app_id,
            scope: MountScope::Global,
            project: None,
            base: paths.home().to_path_buf(),
            parent: config.global_root,
        }),
        ScanRootKey::SharedAgentsProject => {
            let project =
                shared
                    .location
                    .project
                    .clone()
                    .ok_or(TakeoverV2PlanError::InvalidRequest(
                        "Shared Project 快照不完整",
                    ))?;
            validate_project_identity(&project)?;
            Ok(TargetLocation {
                app_id,
                scope: MountScope::Project,
                base: PathBuf::from(&project.root_path),
                parent: Path::new(&project.root_path).join(config.project_relative_root),
                project: Some(project),
            })
        }
        _ => Err(TakeoverV2PlanError::InvalidRequest(
            "sharedTargets 只能引用 Shared observation",
        )),
    }
}

fn validate_target_scopes(targets: &[TakeoverV2Target]) -> Result<(), TakeoverV2PlanError> {
    let mut scopes = BTreeMap::new();
    let mut paths = BTreeSet::new();
    for target in targets {
        if scopes
            .insert(target.app_id.as_str(), target.scope)
            .is_some_and(|scope| scope != target.scope)
        {
            return Err(TakeoverV2PlanError::TargetConflict(format!(
                "{} 不能同时使用 global 与 project Target",
                target.app_id.as_str()
            )));
        }
        if !paths.insert(target.target_path.as_str()) {
            return Err(TakeoverV2PlanError::TargetConflict(
                target.target_path.clone(),
            ));
        }
    }
    Ok(())
}

fn verify_origin_snapshot(snapshot: &OriginSnapshot) -> Result<(), TakeoverV2PlanError> {
    if let Some(project) = snapshot.location.project.as_ref() {
        validate_project_identity(project)?;
    }
    validate_project_management(
        snapshot.location.project.as_ref(),
        &snapshot.origin.observation_skill_file,
    )?;
    let parent_before =
        inspect_directory_chain(&snapshot.location.base, &snapshot.location.parent)?;
    let root_before = inspect_real_directory(Path::new(&snapshot.origin.original_path))?;
    let observed = fingerprint_skill_root(Path::new(&snapshot.origin.original_path))
        .map_err(|error| observation_scan_error(&snapshot.origin.original_path, error))?;
    let validated = validate_single_skill_folder(Path::new(&snapshot.origin.original_path))?;
    let parent_after = inspect_directory_chain(&snapshot.location.base, &snapshot.location.parent)?;
    let root_after = inspect_real_directory(Path::new(&snapshot.origin.original_path))?;
    if metadata_identity(&parent_before) != metadata_identity(&parent_after)
        || metadata_identity(&root_before) != metadata_identity(&root_after)
        || metadata_identity(&parent_after)
            != (
                snapshot.origin.parent_device,
                snapshot.origin.parent_inode,
                snapshot.origin.parent_mode,
            )
        || metadata_identity(&root_after)
            != (
                snapshot.origin.original_device,
                snapshot.origin.original_inode,
                snapshot.origin.original_mode,
            )
        || observed != snapshot.origin.observation_fingerprint
        || !validated_matches_origin(&validated, &snapshot.origin)
    {
        return Err(TakeoverV2PlanError::ObservationChanged(
            snapshot.origin.original_path.clone(),
        ));
    }
    Ok(())
}

fn verify_target_snapshot(
    paths: &ApplicationPaths,
    snapshots: &[OriginSnapshot],
    target: &TakeoverV2Target,
    skill_name: &str,
) -> Result<(), TakeoverV2PlanError> {
    match &target.initial_state {
        TakeoverTargetInitialState::OccupiedByOrigin { origin_id } => {
            let origin = snapshots
                .iter()
                .find(|snapshot| snapshot.origin.id == *origin_id)
                .ok_or(TakeoverV2PlanError::InvalidRequest(
                    "Target 缺少对应 Origin",
                ))?;
            let parent = inspect_directory_chain(&origin.location.base, &origin.location.parent)?;
            if origin.origin.original_path != target.target_path
                || metadata_identity(&parent)
                    != (
                        target.parent_device,
                        target.parent_inode,
                        target.parent_mode,
                    )
                || metadata_identity(&inspect_real_directory(Path::new(&target.target_path))?)
                    != (
                        origin.origin.original_device,
                        origin.origin.original_inode,
                        origin.origin.original_mode,
                    )
            {
                return Err(TakeoverV2PlanError::TargetConflict(
                    target.target_path.clone(),
                ));
            }
        }
        TakeoverTargetInitialState::Absent => {
            let location = target_location_from_plan(paths, target)?;
            if location.parent.join(skill_name) != Path::new(&target.target_path) {
                return Err(TakeoverV2PlanError::TargetConflict(
                    target.target_path.clone(),
                ));
            }
            let parent = inspect_absent_target(
                &location.base,
                &location.parent,
                Path::new(&target.target_path),
            )?;
            if metadata_identity(&parent)
                != (
                    target.parent_device,
                    target.parent_inode,
                    target.parent_mode,
                )
            {
                return Err(TakeoverV2PlanError::TargetConflict(
                    target.target_path.clone(),
                ));
            }
        }
    }
    Ok(())
}

fn target_location_from_plan(
    paths: &ApplicationPaths,
    target: &TakeoverV2Target,
) -> Result<TargetLocation, TakeoverV2PlanError> {
    let config = app_config_for_id(paths, target.app_id)?;
    match target.scope {
        MountScope::Global => Ok(TargetLocation {
            app_id: target.app_id,
            scope: target.scope,
            project: None,
            base: paths.home().to_path_buf(),
            parent: config.global_root,
        }),
        MountScope::Project => {
            let root =
                target
                    .project_root_path
                    .as_deref()
                    .ok_or(TakeoverV2PlanError::InvalidRequest(
                        "Project Target 快照不完整",
                    ))?;
            let metadata = inspect_real_directory(Path::new(root))?;
            if Some(metadata.dev()) != target.project_root_device
                || Some(metadata.ino()) != target.project_root_inode
            {
                return Err(TakeoverV2PlanError::ProjectChanged(root.to_owned()));
            }
            Ok(TargetLocation {
                app_id: target.app_id,
                scope: target.scope,
                project: None,
                base: PathBuf::from(root),
                parent: Path::new(root).join(config.project_relative_root),
            })
        }
    }
}

fn inspect_absent_target(
    base: &Path,
    parent: &Path,
    target: &Path,
) -> Result<fs::Metadata, TakeoverV2PlanError> {
    let before = inspect_directory_chain(base, parent)?;
    match fs::symlink_metadata(target) {
        Ok(_) => {
            return Err(TakeoverV2PlanError::TargetConflict(
                target.display().to_string(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(inspect_error(target, source)),
    }
    let after = inspect_directory_chain(base, parent)?;
    if metadata_identity(&before) != metadata_identity(&after) {
        return Err(TakeoverV2PlanError::TargetConflict(
            target.display().to_string(),
        ));
    }
    Ok(after)
}

fn validate_project_management(
    project: Option<&StoredProject>,
    skill_file: &str,
) -> Result<(), TakeoverV2PlanError> {
    let Some(project) = project else {
        return Ok(());
    };
    match inspect_git_head_management(Path::new(&project.root_path), Path::new(skill_file)) {
        ManagementEvidenceInspection::Absent => Ok(()),
        ManagementEvidenceInspection::Confirmed(_) => Err(TakeoverV2PlanError::Ineligible(
            "该 Skill 已由 Project 维护",
        )),
        ManagementEvidenceInspection::Indeterminate(error) => Err(
            TakeoverV2PlanError::ProjectManagementIndeterminate(error.to_string()),
        ),
    }
}

fn validate_project_identity(project: &StoredProject) -> Result<(), TakeoverV2PlanError> {
    let root = Path::new(&project.root_path);
    let metadata = fs::symlink_metadata(root).map_err(|source| inspect_error(root, source))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.dev() != project.root_device
        || metadata.ino() != project.root_inode
    {
        return Err(TakeoverV2PlanError::ProjectChanged(
            project.root_path.clone(),
        ));
    }
    Ok(())
}

fn inspect_directory_chain(
    base: &Path,
    target: &Path,
) -> Result<fs::Metadata, TakeoverV2PlanError> {
    let mut metadata = inspect_real_directory(base)?;
    let relative = target
        .strip_prefix(base)
        .map_err(|_| TakeoverV2PlanError::UnsafeLocation(target.display().to_string()))?;
    let mut current = base.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(TakeoverV2PlanError::UnsafeLocation(
                target.display().to_string(),
            ));
        };
        current.push(name);
        metadata = inspect_real_directory(&current)?;
    }
    Ok(metadata)
}

fn inspect_real_directory(path: &Path) -> Result<fs::Metadata, TakeoverV2PlanError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| inspect_error(path, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(TakeoverV2PlanError::UnsafeLocation(
            path.display().to_string(),
        ));
    }
    Ok(metadata)
}

fn metadata_identity(metadata: &fs::Metadata) -> (u64, u64, u32) {
    (metadata.dev(), metadata.ino(), metadata.mode())
}

fn validated_matches_origin(validated: &ValidatedSingleSkill, origin: &TakeoverV2Origin) -> bool {
    validated.name == origin.observation_skill_name
        && validated.description == origin.skill_description
        && validated.fingerprint == origin.content_fingerprint
        && validated.warnings == origin.warnings
}

fn snapshot_by_observation<'a>(
    snapshots: &'a [OriginSnapshot],
    observation_id: &str,
) -> Result<&'a OriginSnapshot, TakeoverV2PlanError> {
    snapshots
        .iter()
        .find(|snapshot| snapshot.origin.observation_id == observation_id)
        .ok_or(TakeoverV2PlanError::InvalidRequest(
            "请求引用了未包含的 observation",
        ))
}

fn app_config_for_root(
    paths: &ApplicationPaths,
    root_key: ScanRootKey,
) -> Result<SupportedAppPathConfig, TakeoverV2PlanError> {
    paths
        .supported_apps()
        .into_iter()
        .find(|config| config.root_key == root_key || config.project_root_key == root_key)
        .ok_or(TakeoverV2PlanError::InvalidRequest(
            "rootKey 不属于 Supported App",
        ))
}

fn app_config_for_id(
    paths: &ApplicationPaths,
    app_id: SupportedAppId,
) -> Result<SupportedAppPathConfig, TakeoverV2PlanError> {
    paths
        .supported_apps()
        .into_iter()
        .find(|config| config.id == app_id)
        .ok_or(TakeoverV2PlanError::InvalidRequest(
            "appId 不属于 Supported App",
        ))
}

fn is_shared_root(root_key: ScanRootKey) -> bool {
    matches!(
        root_key,
        ScanRootKey::SharedAgents | ScanRootKey::SharedAgentsProject
    )
}

fn ensure_single_component(value: &str) -> Result<(), TakeoverV2PlanError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err(TakeoverV2PlanError::Ineligible(
            "Skill Name 不能作为安全目录名",
        ));
    }
    Ok(())
}

fn path_to_string(path: &Path) -> Result<String, TakeoverV2PlanError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| TakeoverV2PlanError::NonUnicodePath(path.display().to_string()))
}

fn inspect_error(path: &Path, source: std::io::Error) -> TakeoverV2PlanError {
    TakeoverV2PlanError::InspectPath {
        path: path.display().to_string(),
        source,
    }
}

fn observation_scan_error(path: &str, error: ScanError) -> TakeoverV2PlanError {
    TakeoverV2PlanError::ObservationChanged(format!("{path}：{error}"))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, os::unix::fs::symlink};

    use rusqlite::Connection;
    use tempfile::{TempDir, tempdir};

    use super::*;
    use crate::{
        scanner::{scan, scan_with_projects},
        storage::NewProject,
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
            fs::create_dir_all(&data_root).expect("应创建测试 data root");
            let paths = ApplicationPaths::for_home(data_root.clone(), home);
            let storage = Storage::open(&data_root, &paths.database()).expect("应打开 SQLite");
            Self {
                _temp: temp,
                paths,
                storage,
            }
        }

        fn save_inventory(&mut self) -> Vec<InventoryObservation> {
            let projects = self
                .storage
                .read_stored_projects()
                .expect("应读取已登记 Project");
            let result = if projects.is_empty() {
                scan(&self.paths)
            } else {
                scan_with_projects(&self.paths, &projects, &BTreeSet::new())
            };
            assert!(result.issues.is_empty(), "fixture 扫描不应产生 issue");
            self.storage
                .save_initial_scan(50, &result.entries, &result.supported_apps)
                .expect("应保存 Inventory");
            result.entries
        }

        fn register_project(&mut self, name: &str) -> StoredProject {
            let root = self._temp.path().join(name);
            fs::create_dir_all(&root).expect("应创建 Project 根");
            let metadata = fs::symlink_metadata(&root).expect("应读取 Project 根");
            self.storage
                .register_project(NewProject {
                    id: name,
                    display_name: name,
                    root_path: root.to_str().expect("测试路径应为 UTF-8"),
                    root_device: metadata.dev(),
                    root_inode: metadata.ino(),
                    created_at: 10,
                })
                .expect("应登记 Project")
        }

        fn pending_plan_count(&self) -> i64 {
            let connection = Connection::open(self.paths.database()).expect("应只读检查 SQLite");
            connection
                .query_row("SELECT COUNT(*) FROM takeover_v2_plans", [], |row| {
                    row.get(0)
                })
                .expect("应统计 v2 Plan")
        }
    }

    #[test]
    fn request_uses_the_camel_case_boundary_contract() {
        let request: TakeoverV2PlanRequest = serde_json::from_value(serde_json::json!({
            "observationIds": ["shared-one"],
            "selectedObservationId": "shared-one",
            "preservedObservationIds": [],
            "sharedTargets": [{
                "sharedObservationId": "shared-one",
                "appId": "codex"
            }]
        }))
        .expect("camelCase request 应可解析");

        assert_eq!(request.observation_ids, ["shared-one"]);
        assert_eq!(request.shared_targets[0].app_id, SupportedAppId::Codex);
    }

    #[test]
    fn single_origin_plan_can_be_installed_unmounted_without_file_effect() {
        let mut harness = Harness::new();
        let root = harness.paths.home().join(".codex/skills/alpha");
        write_skill(&root, "alpha", "单 Origin", None);
        let observation = only_observation(harness.save_inventory());
        let original = fs::read(root.join("SKILL.md")).expect("应读取原内容");

        let plan = create_takeover_v2_plan(
            &harness.paths,
            &mut harness.storage,
            request(
                vec![observation.id.clone()],
                &observation.id,
                Vec::new(),
                Vec::new(),
            ),
            100,
        )
        .expect("应创建单 Origin Plan");

        assert_eq!(plan.identity_basis, TakeoverIdentityBasis::SingleOrigin);
        assert_eq!(plan.status, TakeoverV2PlanStatus::Pending);
        assert_eq!(plan.seal, takeover_v2_plan_seal(&plan));
        assert_eq!(plan.origins.len(), 1);
        assert_eq!(
            plan.origins[0].final_disposition,
            TakeoverOriginDisposition::Remove
        );
        assert!(
            plan.targets.is_empty(),
            "installed-unmounted 不需要 Mount Target"
        );
        assert_eq!(
            harness
                .storage
                .read_takeover_v2_plan(&plan.id)
                .expect("应读回 pending Plan"),
            plan
        );
        assert_eq!(fs::read(root.join("SKILL.md")).unwrap(), original);
        assert!(!harness.paths.bundles_root().exists());
    }

    #[test]
    fn explicit_multi_origin_selects_one_content_and_never_expands_by_name() {
        let mut harness = Harness::new();
        write_skill(
            &harness.paths.home().join(".codex/skills/alpha"),
            "alpha",
            "相同内容",
            None,
        );
        write_skill(
            &harness.paths.home().join(".claude/skills/alpha"),
            "alpha",
            "相同内容",
            None,
        );
        write_skill(
            &harness.paths.home().join(".copilot/skills/alpha"),
            "alpha",
            "被选择的不同内容",
            Some("selected"),
        );
        // 同名 Shared observation 只作为未列入请求的旁证，不能被 builder 自动合并。
        write_skill(
            &harness.paths.home().join(".agents/skills/alpha"),
            "alpha",
            "未列入请求",
            None,
        );
        let observations = harness.save_inventory();
        let codex = observation_for_root(&observations, ScanRootKey::CodexGlobal);
        let claude = observation_for_root(&observations, ScanRootKey::ClaudeCodeGlobal);
        let copilot = observation_for_root(&observations, ScanRootKey::GitHubCopilotGlobal);

        let plan = create_takeover_v2_plan(
            &harness.paths,
            &mut harness.storage,
            request(
                vec![codex.id.clone(), claude.id.clone(), copilot.id.clone()],
                &copilot.id,
                vec![claude.id.clone()],
                Vec::new(),
            ),
            100,
        )
        .expect("应创建显式多 Origin Plan");

        assert_eq!(plan.identity_basis, TakeoverIdentityBasis::UserConfirmed);
        assert_eq!(plan.origins.len(), 3);
        assert!(!plan.origins.iter().any(|origin| {
            origin.observation_id
                == observation_for_root(&observations, ScanRootKey::SharedAgents).id
        }));
        let selected = plan
            .origins
            .iter()
            .find(|origin| origin.id == plan.selected_origin_id)
            .expect("应保留 selected Origin");
        assert_eq!(selected.observation_id, copilot.id);
        assert_eq!(selected.skill_description, "被选择的不同内容");
        let codex_origin = origin_for_observation(&plan, &codex.id);
        let claude_origin = origin_for_observation(&plan, &claude.id);
        assert_eq!(
            codex_origin.content_fingerprint,
            claude_origin.content_fingerprint
        );
        assert_ne!(
            selected.content_fingerprint,
            codex_origin.content_fingerprint
        );
        assert_eq!(
            claude_origin.final_disposition,
            TakeoverOriginDisposition::Mount
        );
        assert_eq!(
            codex_origin.final_disposition,
            TakeoverOriginDisposition::Remove
        );
        assert_eq!(plan.targets.len(), 1);
        assert!(!harness.paths.bundles_root().exists());
    }

    #[test]
    fn shared_targets_reuse_preserved_origin_and_add_only_absent_target() {
        let mut harness = Harness::new();
        write_skill(
            &harness.paths.home().join(".agents/skills/alpha"),
            "alpha",
            "Shared 内容",
            None,
        );
        write_skill(
            &harness.paths.home().join(".codex/skills/alpha"),
            "alpha",
            "Codex 内容",
            None,
        );
        fs::create_dir_all(harness.paths.home().join(".copilot/skills"))
            .expect("应创建空的 Copilot Target 父目录");
        let observations = harness.save_inventory();
        let shared = observation_for_root(&observations, ScanRootKey::SharedAgents);
        let codex = observation_for_root(&observations, ScanRootKey::CodexGlobal);

        let plan = create_takeover_v2_plan(
            &harness.paths,
            &mut harness.storage,
            request(
                vec![shared.id.clone(), codex.id.clone()],
                &shared.id,
                vec![codex.id.clone()],
                vec![
                    TakeoverV2SharedTargetRequest {
                        shared_observation_id: shared.id.clone(),
                        app_id: SupportedAppId::Codex,
                    },
                    TakeoverV2SharedTargetRequest {
                        shared_observation_id: shared.id.clone(),
                        app_id: SupportedAppId::GitHubCopilot,
                    },
                ],
            ),
            100,
        )
        .expect("应创建 Shared Target Plan");

        assert_eq!(
            origin_for_observation(&plan, &shared.id).final_disposition,
            TakeoverOriginDisposition::Remove
        );
        assert_eq!(plan.targets.len(), 2, "Codex Target 必须复用而非重复");
        assert_eq!(
            plan.targets
                .iter()
                .filter(|target| target.app_id == SupportedAppId::Codex)
                .count(),
            1
        );
        let copilot_target = plan
            .targets
            .iter()
            .find(|target| target.app_id == SupportedAppId::GitHubCopilot)
            .expect("应生成 Copilot Target");
        assert_eq!(
            copilot_target.initial_state,
            TakeoverTargetInitialState::Absent
        );
        assert!(!Path::new(&copilot_target.target_path).exists());
        assert!(!harness.paths.bundles_root().exists());

        let rejected = create_takeover_v2_plan(
            &harness.paths,
            &mut harness.storage,
            request(
                vec![shared.id.clone()],
                &shared.id,
                vec![],
                vec![TakeoverV2SharedTargetRequest {
                    shared_observation_id: shared.id.clone(),
                    app_id: SupportedAppId::ClaudeCode,
                }],
            ),
            101,
        )
        .expect_err("Shared Target 不能投影到不读取该目录的应用");
        assert!(matches!(rejected, TakeoverV2PlanError::InvalidRequest(_)));
    }

    #[test]
    fn global_and_project_targets_for_same_app_are_rejected_before_save() {
        let mut harness = Harness::new();
        write_skill(
            &harness.paths.home().join(".codex/skills/alpha"),
            "alpha",
            "Global",
            None,
        );
        let project = harness.register_project("project-one");
        let project_skill = Path::new(&project.root_path).join(".codex/skills/alpha");
        write_skill(&project_skill, "alpha", "Project", None);
        let observations = harness.save_inventory();
        let global = observation_for_root(&observations, ScanRootKey::CodexGlobal);
        let project_observation = observation_for_root(&observations, ScanRootKey::CodexProject);

        let error = create_takeover_v2_plan(
            &harness.paths,
            &mut harness.storage,
            request(
                vec![global.id.clone(), project_observation.id.clone()],
                &global.id,
                vec![global.id.clone(), project_observation.id.clone()],
                Vec::new(),
            ),
            100,
        )
        .expect_err("同一 app 的 global/project Target 必须冲突");

        assert!(matches!(error, TakeoverV2PlanError::TargetConflict(_)));
        assert_eq!(harness.pending_plan_count(), 0);
        assert!(project_skill.join("SKILL.md").is_file());
        assert!(!harness.paths.bundles_root().exists());
    }

    #[test]
    fn stale_changed_and_unsafe_origins_are_rejected_without_file_effects() {
        // stale 只使用已保存的 Inventory 状态，不应触碰原 Skill。
        {
            let mut harness = Harness::new();
            let root = harness.paths.home().join(".codex/skills/alpha");
            write_skill(&root, "alpha", "Stale", None);
            let mut observation = only_observation(harness.save_inventory());
            observation.stale = true;
            harness
                .storage
                .save_initial_scan(51, &[observation.clone()], &[])
                .expect("应保存 stale Inventory");
            assert!(matches!(
                create_takeover_v2_plan(
                    &harness.paths,
                    &mut harness.storage,
                    request(
                        vec![observation.id.clone()],
                        &observation.id,
                        Vec::new(),
                        Vec::new()
                    ),
                    100,
                ),
                Err(TakeoverV2PlanError::Ineligible(_))
            ));
            assert!(root.join("SKILL.md").is_file());
            assert_eq!(harness.pending_plan_count(), 0);
            assert!(!harness.paths.bundles_root().exists());
        }

        // Scanner 快照之后的内容变化必须在强内容校验前被识别。
        {
            let mut harness = Harness::new();
            let root = harness.paths.home().join(".codex/skills/alpha");
            write_skill(&root, "alpha", "Changed", None);
            let observation = only_observation(harness.save_inventory());
            fs::write(root.join("changed.txt"), "external change").expect("应模拟外部内容变化");
            assert!(matches!(
                create_takeover_v2_plan(
                    &harness.paths,
                    &mut harness.storage,
                    request(
                        vec![observation.id.clone()],
                        &observation.id,
                        Vec::new(),
                        Vec::new()
                    ),
                    100,
                ),
                Err(TakeoverV2PlanError::ObservationChanged(_))
            ));
            assert_eq!(
                fs::read_to_string(root.join("changed.txt")).unwrap(),
                "external change"
            );
            assert_eq!(harness.pending_plan_count(), 0);
            assert!(!harness.paths.bundles_root().exists());
        }

        // Scanner 可展示可达 symlink；Plan builder 必须拒绝把它当成可移动 Origin。
        {
            let mut harness = Harness::new();
            let real = harness._temp.path().join("outside-alpha");
            write_skill(&real, "alpha", "Unsafe", None);
            let link = harness.paths.home().join(".codex/skills/alpha");
            fs::create_dir_all(link.parent().expect("应有 symlink 父目录"))
                .expect("应创建 symlink 父目录");
            symlink(&real, &link).expect("应创建测试 symlink");
            let observation = only_observation(harness.save_inventory());
            assert!(matches!(
                create_takeover_v2_plan(
                    &harness.paths,
                    &mut harness.storage,
                    request(
                        vec![observation.id.clone()],
                        &observation.id,
                        Vec::new(),
                        Vec::new()
                    ),
                    100,
                ),
                Err(TakeoverV2PlanError::UnsafeLocation(_))
            ));
            assert!(
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert!(real.join("SKILL.md").is_file());
            assert_eq!(harness.pending_plan_count(), 0);
            assert!(!harness.paths.bundles_root().exists());
        }
    }

    fn request(
        observation_ids: Vec<String>,
        selected_observation_id: &str,
        preserved_observation_ids: Vec<String>,
        shared_targets: Vec<TakeoverV2SharedTargetRequest>,
    ) -> TakeoverV2PlanRequest {
        TakeoverV2PlanRequest {
            observation_ids,
            selected_observation_id: selected_observation_id.to_owned(),
            preserved_observation_ids,
            shared_targets,
        }
    }

    fn write_skill(root: &Path, name: &str, description: &str, helper: Option<&str>) {
        fs::create_dir_all(root).expect("应创建 Skill 根");
        fs::write(
            root.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
        )
        .expect("应写入有效 SKILL.md");
        if let Some(helper) = helper {
            fs::write(root.join("helper.txt"), helper).expect("应写入区分内容");
        }
    }

    fn only_observation(entries: Vec<InventoryObservation>) -> InventoryObservation {
        assert_eq!(entries.len(), 1, "fixture 应只有一个 observation");
        entries.into_iter().next().expect("应有 observation")
    }

    fn observation_for_root(
        entries: &[InventoryObservation],
        root_key: ScanRootKey,
    ) -> &InventoryObservation {
        entries
            .iter()
            .find(|entry| entry.root_key == root_key)
            .expect("应找到指定 rootKey observation")
    }

    fn origin_for_observation<'a>(
        plan: &'a TakeoverV2Plan,
        observation_id: &str,
    ) -> &'a TakeoverV2Origin {
        plan.origins
            .iter()
            .find(|origin| origin.observation_id == observation_id)
            .expect("Plan 应包含指定 Origin")
    }
}
