use std::{fs, path::Path};

use serde::Deserialize;

use thiserror::Error;

use crate::{
    domain::{
        InventoryLocationKind, InventoryObservation, SkillMetadataStatus, SupportedAppId,
        SupportedAppSummary,
    },
    paths::ApplicationPaths,
};

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("无法检查扫描路径 {path}：{source}")]
    InspectPath {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("扫描根目录不是文件夹：{0}")]
    RootIsNotDirectory(String),
    #[error("无法读取扫描根目录 {path}：{source}")]
    ReadRoot {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

pub struct ScanResult {
    pub entries: Vec<InventoryObservation>,
    pub supported_apps: Vec<SupportedAppSummary>,
}

/// 首次扫描只读取已确认的用户级目录，不创建任何 Host 路径。
pub fn scan(paths: &ApplicationPaths) -> Result<ScanResult, ScanError> {
    let mut entries = Vec::new();
    let mut supported_apps = Vec::new();

    for app in paths.supported_apps() {
        let detected = path_exists(&app.detection_root)?;
        entries.extend(scan_optional_root(
            &app.global_root,
            InventoryLocationKind::AppGlobal,
            vec![app.id],
        )?);
        supported_apps.push(SupportedAppSummary {
            id: app.id,
            display_name: app.display_name.to_owned(),
            detected: Some(detected),
        });
    }

    entries.extend(scan_optional_root(
        &paths.shared_read_only_root(),
        InventoryLocationKind::SharedReadOnly,
        vec![SupportedAppId::Codex, SupportedAppId::GitHubCopilot],
    )?);
    entries.sort_by(|left, right| left.skill_root.cmp(&right.skill_root));

    Ok(ScanResult {
        entries,
        supported_apps,
    })
}

fn scan_optional_root(
    path: &Path,
    location_kind: InventoryLocationKind,
    observed_by: Vec<SupportedAppId>,
) -> Result<Vec<InventoryObservation>, ScanError> {
    if !path_exists(path)? {
        return Ok(Vec::new());
    }
    if !path.is_dir() {
        return Err(ScanError::RootIsNotDirectory(path.display().to_string()));
    }

    let directory = fs::read_dir(path).map_err(|source| ScanError::ReadRoot {
        path: path.display().to_string(),
        source,
    })?;
    let mut children =
        directory
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| ScanError::ReadRoot {
                path: path.display().to_string(),
                source,
            })?;
    children.sort_by_key(|entry| entry.path());

    let mut observations = Vec::new();
    for child in children {
        let skill_root = child.path();
        let file_type = child.file_type().map_err(|source| ScanError::ReadRoot {
            path: path.display().to_string(),
            source,
        })?;
        if !file_type.is_dir() && !file_type.is_symlink() {
            continue;
        }

        let skill_file = skill_root.join("SKILL.md");
        if !skill_file.is_file() {
            continue;
        }

        let fallback_name = child.file_name().to_string_lossy().into_owned();
        let (declared_name, metadata_status) = read_skill_metadata(&skill_file, &fallback_name);
        // 无效声明只能作为诊断证据展示，不能冒充可信 Skill Name。
        let skill_name = if metadata_status == SkillMetadataStatus::Valid {
            declared_name
                .clone()
                .unwrap_or_else(|| fallback_name.clone())
        } else {
            fallback_name
        };
        let root_string = skill_root.to_string_lossy().into_owned();
        observations.push(InventoryObservation {
            id: format!("{}:{root_string}", location_kind.as_str()),
            skill_name,
            declared_name,
            skill_root: root_string,
            skill_file: skill_file.to_string_lossy().into_owned(),
            location_kind,
            metadata_status,
            observed_by: observed_by.clone(),
        });
    }

    Ok(observations)
}

#[derive(Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
}

fn read_skill_metadata(path: &Path, directory_name: &str) -> (Option<String>, SkillMetadataStatus) {
    let Ok(contents) = fs::read_to_string(path) else {
        return (None, SkillMetadataStatus::Unreadable);
    };
    let Some(frontmatter) = extract_frontmatter(&contents) else {
        return (None, SkillMetadataStatus::Invalid);
    };
    let Ok(metadata) = serde_yaml_ng::from_str::<SkillFrontmatter>(frontmatter) else {
        return (None, SkillMetadataStatus::Invalid);
    };
    let SkillFrontmatter { name, description } = metadata;
    let valid_name = name
        .as_deref()
        .is_some_and(|value| is_valid_skill_name(value, directory_name));
    let valid_description = description.as_deref().is_some_and(|value| {
        let length = value.trim().chars().count();
        (1..=1024).contains(&length)
    });
    let status = if valid_name && valid_description {
        SkillMetadataStatus::Valid
    } else {
        SkillMetadataStatus::Invalid
    };
    (name, status)
}

fn is_valid_skill_name(name: &str, directory_name: &str) -> bool {
    let length = name.len();
    (1..=64).contains(&length)
        && name == directory_name
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name.bytes().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == b'-'
        })
}

fn path_exists(path: &Path) -> Result<bool, ScanError> {
    path.try_exists().map_err(|source| ScanError::InspectPath {
        path: path.display().to_string(),
        source,
    })
}

fn extract_frontmatter(contents: &str) -> Option<&str> {
    let body = contents.strip_prefix("---\n")?;
    let end = body.find("\n---")?;
    Some(&body[..end])
}
