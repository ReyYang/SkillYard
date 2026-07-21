use std::{fs, fs::File, io::Read, path::Path};

use serde::Deserialize;
use thiserror::Error;

use crate::{
    domain::{
        InventoryLocationKind, InventoryObservation, ManagementKind, ScanIssue, ScanIssueCode,
        ScanRootKey, SkillMetadataStatus, SupportedAppId, SupportedAppSummary,
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
    #[error("无法读取 Skill 内容 {path}：{source}")]
    ReadSkillContent {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

pub struct ScanResult {
    pub entries: Vec<InventoryObservation>,
    pub supported_apps: Vec<SupportedAppSummary>,
    pub successful_roots: Vec<ScanRootKey>,
    pub issues: Vec<ScanIssue>,
}

/// 扫描每个固定根目录并分别记录结果，单个根失败不会伪装成空目录。
pub fn scan(paths: &ApplicationPaths) -> ScanResult {
    let mut entries = Vec::new();
    let mut supported_apps = Vec::new();
    let mut successful_roots = Vec::new();
    let mut issues = Vec::new();

    for app in paths.supported_apps() {
        let detected = match path_exists(&app.detection_root) {
            Ok(value) => value,
            Err(error) => {
                issues.push(error.into_issue(app.root_key));
                continue;
            }
        };
        supported_apps.push(SupportedAppSummary {
            id: app.id,
            display_name: app.display_name.to_owned(),
            detected: Some(detected),
        });

        match scan_optional_root(
            &app.global_root,
            app.root_key,
            InventoryLocationKind::AppGlobal,
            vec![app.id],
        ) {
            Ok(root_entries) => {
                entries.extend(root_entries);
                successful_roots.push(app.root_key);
            }
            Err(error) => issues.push(error.into_issue(app.root_key)),
        }
    }

    match scan_optional_root(
        &paths.shared_read_only_root(),
        ScanRootKey::SharedAgents,
        InventoryLocationKind::SharedReadOnly,
        vec![SupportedAppId::Codex, SupportedAppId::GitHubCopilot],
    ) {
        Ok(root_entries) => {
            entries.extend(root_entries);
            successful_roots.push(ScanRootKey::SharedAgents);
        }
        Err(error) => issues.push(error.into_issue(ScanRootKey::SharedAgents)),
    }
    entries.sort_by(|left, right| left.skill_root.cmp(&right.skill_root));

    ScanResult {
        entries,
        supported_apps,
        successful_roots,
        issues,
    }
}

fn scan_optional_root(
    path: &Path,
    root_key: ScanRootKey,
    location_kind: InventoryLocationKind,
    observed_by: Vec<SupportedAppId>,
) -> Result<Vec<InventoryObservation>, ScanError> {
    if !path_exists(path)? {
        return Ok(Vec::new());
    }
    let root_metadata = fs::metadata(path).map_err(|source| ScanError::ReadRoot {
        path: path.display().to_string(),
        source,
    })?;
    if !root_metadata.is_dir() {
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
        let observed_fingerprint = fingerprint_skill_root(&skill_root)?;
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
            observed_fingerprint,
            root_key,
            stale: false,
            management_kind: ManagementKind::TakeoverCandidate,
        });
    }

    Ok(observations)
}

impl ScanError {
    fn into_issue(self, root_key: ScanRootKey) -> ScanIssue {
        let message = self.to_string();
        let (path, code) = match &self {
            Self::InspectPath { path, .. } => (path.clone(), ScanIssueCode::InspectPath),
            Self::RootIsNotDirectory(path) => (path.clone(), ScanIssueCode::RootNotDirectory),
            Self::ReadRoot { path, .. } => (path.clone(), ScanIssueCode::ReadRoot),
            Self::ReadSkillContent { path, .. } => (path.clone(), ScanIssueCode::ReadSkillContent),
        };
        ScanIssue {
            root_key,
            path,
            code,
            message,
        }
    }
}

fn fingerprint_skill_root(root: &Path) -> Result<String, ScanError> {
    let mut fingerprint = StableFingerprint::new();
    if fs::symlink_metadata(root)
        .map_err(|source| read_skill_content_error(root, source))?
        .file_type()
        .is_symlink()
    {
        let target =
            fs::read_link(root).map_err(|source| read_skill_content_error(root, source))?;
        fingerprint.write_segment(b"root-symlink");
        fingerprint.write_segment(target.to_string_lossy().as_bytes());
    }
    fingerprint_directory(root, root, &mut fingerprint)?;
    Ok(format!("{:016x}", fingerprint.finish()))
}

fn fingerprint_directory(
    root: &Path,
    directory: &Path,
    fingerprint: &mut StableFingerprint,
) -> Result<(), ScanError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| read_skill_content_error(directory, source))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| read_skill_content_error(directory, source))?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        fingerprint.write_segment(relative.to_string_lossy().as_bytes());
        let file_type = entry
            .file_type()
            .map_err(|source| read_skill_content_error(&path, source))?;
        if file_type.is_dir() {
            fingerprint.write_segment(b"directory");
            fingerprint_directory(root, &path, fingerprint)?;
        } else if file_type.is_file() {
            fingerprint.write_segment(b"file");
            fingerprint_file(&path, fingerprint)?;
        } else if file_type.is_symlink() {
            fingerprint.write_segment(b"symlink");
            let target =
                fs::read_link(&path).map_err(|source| read_skill_content_error(&path, source))?;
            fingerprint.write_segment(target.to_string_lossy().as_bytes());
        } else {
            fingerprint.write_segment(b"special");
        }
    }
    Ok(())
}

fn fingerprint_file(path: &Path, fingerprint: &mut StableFingerprint) -> Result<(), ScanError> {
    let mut file = File::open(path).map_err(|source| read_skill_content_error(path, source))?;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let bytes_read = file
            .read(&mut buffer)
            .map_err(|source| read_skill_content_error(path, source))?;
        if bytes_read == 0 {
            break;
        }
        fingerprint.write(&buffer[..bytes_read]);
    }
    Ok(())
}

fn read_skill_content_error(path: &Path, source: std::io::Error) -> ScanError {
    ScanError::ReadSkillContent {
        path: path.display().to_string(),
        source,
    }
}

/// 固定 FNV-1a 只用于比较本地观察，不承担安全摘要或身份判断。
struct StableFingerprint(u64);

impl StableFingerprint {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn write_segment(&mut self, bytes: &[u8]) {
        self.write(&(bytes.len() as u64).to_le_bytes());
        self.write(bytes);
    }

    fn finish(self) -> u64 {
        self.0
    }
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
