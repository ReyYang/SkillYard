use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

use thiserror::Error;
use uuid::Uuid;

use crate::{
    content::{ContentValidationError, validate_single_skill_folder},
    domain::{
        InventoryLocationKind, ManagementKind, MountScope, ScanRootKey, SkillMetadataStatus,
        SupportedAppId, TakeoverPlan, TakeoverPlanPath,
    },
    git_management_evidence::{ManagementEvidenceInspection, inspect_git_head_management},
    paths::{ApplicationPaths, SupportedAppPathConfig},
    scanner::{ScanError, fingerprint_skill_root},
    storage::{Storage, StorageError, StoredProject, StoredTakeoverPlan, takeover_plan_seal},
};

const TAKEOVER_PLAN_TTL_MILLIS: i64 = 30 * 60 * 1_000;

#[derive(Debug, Error)]
pub enum TakeoverLifecycleError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Content(#[from] ContentValidationError),
    #[error("无法检查接管路径 {path}：{source}")]
    InspectPath {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Inventory observation 已变化，请先刷新本机清单：{0}")]
    ObservationChanged(String),
    #[error("该 Skill 不符合普通已有安装的接管条件：{0}")]
    Ineligible(&'static str),
    #[error("已有安装不在 Supported App 的固定 Skill 叶子中：{0}")]
    UnsafeLocation(String),
    #[error("已登记 Project 目录已经变化：{0}")]
    ProjectChanged(String),
    #[error("Project 的 Git 管理状态无法可靠确认：{0}")]
    ProjectManagementIndeterminate(String),
    #[error("接管路径不能无损保存为 UTF-8：{0}")]
    NonUnicodePath(String),
}

struct TakeoverLocation {
    app_id: SupportedAppId,
    scope: MountScope,
    project: Option<StoredProject>,
    base: PathBuf,
    parent: PathBuf,
}

/// 本片只签发只读 Plan；不会创建 Bundle、复制内容或替换 Host 路径。
pub fn create_takeover_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    observation_id: &str,
    now: i64,
) -> Result<TakeoverPlan, TakeoverLifecycleError> {
    let observation = storage.read_inventory_observation(observation_id)?;
    validate_inventory_eligibility(&observation)?;
    let location = derive_takeover_location(paths, storage, &observation)?;
    let expected_observers = expected_observers(location.app_id, location.scope);
    if observation.observed_by != expected_observers {
        return Err(TakeoverLifecycleError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }

    ensure_single_component(&observation.skill_name)?;
    let expected_original = location.parent.join(&observation.skill_name);
    let original = Path::new(&observation.skill_root);
    if original != expected_original
        || Path::new(&observation.skill_file) != expected_original.join("SKILL.md")
    {
        return Err(TakeoverLifecycleError::UnsafeLocation(
            observation.skill_root.clone(),
        ));
    }
    validate_project_management_snapshot(location.project.as_ref(), &observation.skill_file)?;
    let parent_before = inspect_directory_chain(&location.base, &location.parent)?;
    let original_before = inspect_real_directory(original)?;
    let current_observed_fingerprint = fingerprint_skill_root(original)
        .map_err(|error| map_scan_error(error, &observation.skill_root))?;
    if current_observed_fingerprint != observation.observed_fingerprint {
        return Err(TakeoverLifecycleError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }
    let validated = validate_single_skill_folder(original)?;
    if validated.name != observation.skill_name || validated.fingerprint.len() != 64 {
        return Err(TakeoverLifecycleError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }
    let parent_after = inspect_directory_chain(&location.base, &location.parent)?;
    let original_after = inspect_real_directory(original)?;
    if filesystem_identity(&parent_before) != filesystem_identity(&parent_after)
        || filesystem_identity(&original_before) != filesystem_identity(&original_after)
    {
        return Err(TakeoverLifecycleError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }

    let bundle_id = Uuid::new_v4().to_string();
    let content_id = Uuid::new_v4().to_string();
    let member_id = Uuid::new_v4().to_string();
    let managed_directory = paths.bundle_directory(&bundle_id);
    let content_directory = managed_directory.join("contents").join(&content_id);
    let expected_target = managed_directory
        .join("current/members")
        .join(&validated.name);
    let project = location.project.as_ref();
    let mut stored = StoredTakeoverPlan {
        plan: TakeoverPlan {
            id: String::new(),
            observation_id: observation.id.clone(),
            bundle_id,
            content_id,
            member_id,
            bundle_display_name: validated.name.clone(),
            source_display_name: None,
            source_notice: "来源未知；没有更新来源".to_owned(),
            skill_name: validated.name,
            skill_description: validated.description,
            content_fingerprint: validated.fingerprint,
            warnings: validated.warnings,
            managed_directory: path_to_string(&managed_directory)?,
            content_directory: path_to_string(&content_directory)?,
            expected_target: path_to_string(&expected_target)?,
            paths: vec![TakeoverPlanPath {
                id: Uuid::new_v4().to_string(),
                mount_id: Uuid::new_v4().to_string(),
                original_path: observation.skill_root.clone(),
                app_id: location.app_id,
                scope: location.scope,
                project_id: project.map(|value| value.id.clone()),
                project_display_name: project.map(|value| value.display_name.clone()),
                project_root_path: project.map(|value| value.root_path.clone()),
                project_root_device: project.map(|value| value.root_device),
                project_root_inode: project.map(|value| value.root_inode),
                parent_device: parent_after.dev(),
                parent_inode: parent_after.ino(),
                parent_mode: parent_after.mode(),
                original_device: original_after.dev(),
                original_inode: original_after.ino(),
                original_mode: original_after.mode(),
                default_preserve_mount: true,
            }],
            created_at: now,
            expires_at: now.saturating_add(TAKEOVER_PLAN_TTL_MILLIS),
        },
        observation,
        status: "pending".to_owned(),
    };
    verify_final_snapshot(
        &location,
        &stored.observation,
        &stored.plan,
        filesystem_identity(&parent_after),
        filesystem_identity(&original_after),
    )?;
    let seal = takeover_plan_seal(&stored);
    stored.plan.id = format!("takeover-{}-{seal}", Uuid::new_v4());
    Ok(storage.save_takeover_plan(&stored)?.plan)
}

fn verify_final_snapshot(
    location: &TakeoverLocation,
    observation: &crate::domain::InventoryObservation,
    plan: &TakeoverPlan,
    expected_parent_identity: (u64, u64, u32),
    expected_original_identity: (u64, u64, u32),
) -> Result<(), TakeoverLifecycleError> {
    validate_project_management_snapshot(location.project.as_ref(), &observation.skill_file)?;
    let parent = inspect_directory_chain(&location.base, &location.parent)?;
    let original = inspect_real_directory(Path::new(&observation.skill_root))?;
    let observed_fingerprint = fingerprint_skill_root(Path::new(&observation.skill_root))
        .map_err(|error| map_scan_error(error, &observation.skill_root))?;
    let validated = validate_single_skill_folder(Path::new(&observation.skill_root))?;
    if observed_fingerprint != observation.observed_fingerprint
        || validated.name != plan.skill_name
        || validated.description != plan.skill_description
        || validated.fingerprint != plan.content_fingerprint
        || validated.warnings != plan.warnings
        || filesystem_identity(&parent) != expected_parent_identity
        || filesystem_identity(&original) != expected_original_identity
    {
        return Err(TakeoverLifecycleError::ObservationChanged(
            observation.skill_root.clone(),
        ));
    }
    Ok(())
}

fn validate_inventory_eligibility(
    observation: &crate::domain::InventoryObservation,
) -> Result<(), TakeoverLifecycleError> {
    if observation.stale {
        return Err(TakeoverLifecycleError::Ineligible("观察已经过期"));
    }
    if observation.metadata_status != SkillMetadataStatus::Valid {
        return Err(TakeoverLifecycleError::Ineligible("Skill metadata 无效"));
    }
    if observation.management_kind != ManagementKind::TakeoverCandidate
        || observation.management_evidence.is_some()
    {
        return Err(TakeoverLifecycleError::Ineligible(
            "该 Skill 已由其他管理方负责",
        ));
    }
    if !matches!(
        observation.location_kind,
        InventoryLocationKind::AppGlobal | InventoryLocationKind::AppProject
    ) {
        return Err(TakeoverLifecycleError::Ineligible(
            "只接受应用专属普通 Skill 目录",
        ));
    }
    Ok(())
}

fn derive_takeover_location(
    paths: &ApplicationPaths,
    storage: &Storage,
    observation: &crate::domain::InventoryObservation,
) -> Result<TakeoverLocation, TakeoverLifecycleError> {
    let config = app_config_for_root(paths, observation.root_key).ok_or(
        TakeoverLifecycleError::Ineligible("共享或受管目录不能由本片接管"),
    )?;
    match observation.root_key {
        ScanRootKey::CodexGlobal
        | ScanRootKey::ClaudeCodeGlobal
        | ScanRootKey::GitHubCopilotGlobal => {
            if observation.location_kind != InventoryLocationKind::AppGlobal
                || observation.project_id.is_some()
            {
                return Err(TakeoverLifecycleError::Ineligible(
                    "global observation 的范围不一致",
                ));
            }
            Ok(TakeoverLocation {
                app_id: config.id,
                scope: MountScope::Global,
                project: None,
                base: paths.home().to_path_buf(),
                parent: config.global_root,
            })
        }
        ScanRootKey::CodexProject
        | ScanRootKey::ClaudeCodeProject
        | ScanRootKey::GitHubCopilotProject => {
            if observation.location_kind != InventoryLocationKind::AppProject {
                return Err(TakeoverLifecycleError::Ineligible(
                    "project observation 的范围不一致",
                ));
            }
            let project_id =
                observation
                    .project_id
                    .as_deref()
                    .ok_or(TakeoverLifecycleError::Ineligible(
                        "project observation 缺少 Project",
                    ))?;
            let project = storage.read_project(project_id)?;
            validate_project_identity(&project)?;
            let parent = Path::new(&project.root_path).join(&config.project_relative_root);
            Ok(TakeoverLocation {
                app_id: config.id,
                scope: MountScope::Project,
                base: PathBuf::from(&project.root_path),
                project: Some(project),
                parent,
            })
        }
        ScanRootKey::SharedAgents | ScanRootKey::SharedAgentsProject => {
            Err(TakeoverLifecycleError::Ineligible("共享只读目录不能接管"))
        }
    }
}

fn app_config_for_root(
    paths: &ApplicationPaths,
    root_key: ScanRootKey,
) -> Option<SupportedAppPathConfig> {
    paths
        .supported_apps()
        .into_iter()
        .find(|config| config.root_key == root_key || config.project_root_key == root_key)
}

fn expected_observers(app_id: SupportedAppId, scope: MountScope) -> Vec<SupportedAppId> {
    if app_id == SupportedAppId::ClaudeCode && scope == MountScope::Project {
        vec![SupportedAppId::ClaudeCode, SupportedAppId::GitHubCopilot]
    } else {
        vec![app_id]
    }
}

fn validate_project_identity(project: &StoredProject) -> Result<(), TakeoverLifecycleError> {
    let path = Path::new(&project.root_path);
    let metadata = fs::symlink_metadata(path).map_err(|source| inspect_error(path, source))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.dev() != project.root_device
        || metadata.ino() != project.root_inode
    {
        return Err(TakeoverLifecycleError::ProjectChanged(
            project.root_path.clone(),
        ));
    }
    Ok(())
}

fn validate_project_management_snapshot(
    project: Option<&StoredProject>,
    skill_file: &str,
) -> Result<(), TakeoverLifecycleError> {
    let Some(project) = project else {
        return Ok(());
    };
    match inspect_git_head_management(Path::new(&project.root_path), Path::new(skill_file)) {
        ManagementEvidenceInspection::Absent => Ok(()),
        ManagementEvidenceInspection::Confirmed(_) => Err(TakeoverLifecycleError::Ineligible(
            "该 Skill 已由 Project 仓库维护",
        )),
        ManagementEvidenceInspection::Indeterminate(error) => Err(
            TakeoverLifecycleError::ProjectManagementIndeterminate(error.to_string()),
        ),
    }
}

fn inspect_real_directory(path: &Path) -> Result<fs::Metadata, TakeoverLifecycleError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| inspect_error(path, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(TakeoverLifecycleError::UnsafeLocation(
            path.display().to_string(),
        ));
    }
    Ok(metadata)
}

fn inspect_directory_chain(
    base: &Path,
    target: &Path,
) -> Result<fs::Metadata, TakeoverLifecycleError> {
    inspect_real_directory(base)?;
    let relative = target
        .strip_prefix(base)
        .map_err(|_| TakeoverLifecycleError::UnsafeLocation(target.display().to_string()))?;
    let mut current = base.to_path_buf();
    let mut metadata = fs::symlink_metadata(base).map_err(|source| inspect_error(base, source))?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(TakeoverLifecycleError::UnsafeLocation(
                target.display().to_string(),
            ));
        };
        current.push(name);
        metadata = inspect_real_directory(&current)?;
    }
    Ok(metadata)
}

fn filesystem_identity(metadata: &fs::Metadata) -> (u64, u64, u32) {
    (metadata.dev(), metadata.ino(), metadata.mode())
}

fn ensure_single_component(value: &str) -> Result<(), TakeoverLifecycleError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err(TakeoverLifecycleError::Ineligible(
            "Skill Name 不能作为安全目录名",
        ));
    }
    Ok(())
}

fn path_to_string(path: &Path) -> Result<String, TakeoverLifecycleError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| TakeoverLifecycleError::NonUnicodePath(path.display().to_string()))
}

fn inspect_error(path: &Path, source: std::io::Error) -> TakeoverLifecycleError {
    TakeoverLifecycleError::InspectPath {
        path: path.display().to_string(),
        source,
    }
}

fn map_scan_error(error: ScanError, path: &str) -> TakeoverLifecycleError {
    TakeoverLifecycleError::ObservationChanged(format!("{path}：{error}"))
}
