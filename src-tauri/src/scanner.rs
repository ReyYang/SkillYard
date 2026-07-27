use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    fs::File,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::Deserialize;
use thiserror::Error;

use crate::{
    domain::{
        InstallationChain, InventoryLocationKind, InventoryObservation, ManagementKind, ScanIssue,
        ScanIssueCode, ScanRootIdentity, ScanRootKey, SkillMetadataStatus, SupportedAppId,
        SupportedAppSummary,
    },
    git_management_evidence::{
        ManagementEvidenceError, ManagementEvidenceInspection, inspect_git_head_management,
    },
    installation_chain::read_lock_v3_installation_chains,
    paths::ApplicationPaths,
    storage::StoredProject,
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
    #[error("无法检查 Project Skill 的管理证据 {path}：{source}")]
    InspectManagementEvidence {
        path: String,
        #[source]
        source: ManagementEvidenceError,
    },
    #[error("已登记 Project 目录已经变化：{0}")]
    ProjectChanged(String),
}

pub struct ScanResult {
    pub entries: Vec<InventoryObservation>,
    pub supported_apps: Vec<SupportedAppSummary>,
    pub successful_roots: Vec<ScanRootIdentity>,
    pub issues: Vec<ScanIssue>,
}

struct ScanContext<'a> {
    excluded_skill_roots: &'a BTreeSet<PathBuf>,
    installation_chains: &'a BTreeMap<String, InstallationChain>,
}

/// 扫描每个固定根目录并分别记录结果，单个根失败不会伪装成空目录。
pub fn scan(paths: &ApplicationPaths) -> ScanResult {
    scan_excluding(paths, &BTreeSet::new())
}

/// 已登记 Mount 由独立 health 检查读取，Inventory 扫描不能跟随它们指向的内容。
pub fn scan_excluding(
    paths: &ApplicationPaths,
    excluded_skill_roots: &BTreeSet<PathBuf>,
) -> ScanResult {
    let installation_chains = read_lock_v3_installation_chains(paths);
    let context = ScanContext {
        excluded_skill_roots,
        installation_chains: &installation_chains,
    };
    let mut entries = Vec::new();
    let mut supported_apps = Vec::new();
    let mut successful_roots = Vec::new();
    let mut issues = Vec::new();

    for app in paths.supported_apps() {
        let detected = match path_exists(&app.detection_root) {
            Ok(value) => value,
            Err(error) => {
                issues.push(error.as_issue(app.root_key, None));
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
            None,
            None,
            &context,
        ) {
            Ok(root_entries) => {
                entries.extend(root_entries);
                successful_roots.push(ScanRootIdentity::global(app.root_key));
            }
            Err(error) => issues.push(error.as_issue(app.root_key, None)),
        }
    }

    match scan_optional_root(
        &paths.shared_read_only_root(),
        ScanRootKey::SharedAgents,
        InventoryLocationKind::SharedReadOnly,
        vec![SupportedAppId::Codex, SupportedAppId::GitHubCopilot],
        None,
        None,
        &context,
    ) {
        Ok(root_entries) => {
            entries.extend(root_entries);
            successful_roots.push(ScanRootIdentity::global(ScanRootKey::SharedAgents));
        }
        Err(error) => issues.push(error.as_issue(ScanRootKey::SharedAgents, None)),
    }

    match scan_codex_official_plugins(paths, &context) {
        Ok(root_entries) => {
            entries.extend(root_entries);
            successful_roots.push(ScanRootIdentity::global(ScanRootKey::CodexOfficialPlugins));
        }
        Err(error) => issues.push(error.as_issue(ScanRootKey::CodexOfficialPlugins, None)),
    }
    entries.sort_by(|left, right| left.skill_root.cmp(&right.skill_root));

    ScanResult {
        entries,
        supported_apps,
        successful_roots,
        issues,
    }
}

/// Codex 官方插件由 Codex 自己维护；SkillYard 只读取当前缓存版本并按插件展示。
fn scan_codex_official_plugins(
    paths: &ApplicationPaths,
    context: &ScanContext<'_>,
) -> Result<Vec<InventoryObservation>, ScanError> {
    let mut observations = Vec::new();
    for marketplace_root in paths.codex_official_plugin_cache_roots() {
        for plugin_root in child_directories(&marketplace_root)? {
            let Some(version_root) = newest_child_directory(&plugin_root)? else {
                continue;
            };
            let mut plugin_observations = scan_optional_root(
                &version_root.join("skills"),
                ScanRootKey::CodexOfficialPlugins,
                InventoryLocationKind::SharedReadOnly,
                vec![SupportedAppId::Codex],
                None,
                None,
                context,
            )?;
            for observation in &mut plugin_observations {
                observation.management_kind = ManagementKind::AgentManaged;
            }
            observations.extend(plugin_observations);
        }
    }
    Ok(observations)
}

fn child_directories(path: &Path) -> Result<Vec<PathBuf>, ScanError> {
    if !path_exists(path)? {
        return Ok(Vec::new());
    }
    let metadata = fs::metadata(path).map_err(|source| ScanError::ReadRoot {
        path: path.display().to_string(),
        source,
    })?;
    if !metadata.is_dir() {
        return Err(ScanError::RootIsNotDirectory(path.display().to_string()));
    }

    let directory = fs::read_dir(path).map_err(|source| ScanError::ReadRoot {
        path: path.display().to_string(),
        source,
    })?;
    let mut children = Vec::new();
    for child in directory {
        let child = child.map_err(|source| ScanError::ReadRoot {
            path: path.display().to_string(),
            source,
        })?;
        let child_path = child.path();
        let child_metadata = fs::metadata(&child_path).map_err(|source| ScanError::ReadRoot {
            path: child_path.display().to_string(),
            source,
        })?;
        if child_metadata.is_dir() {
            children.push(child_path);
        }
    }
    children.sort();
    Ok(children)
}

fn newest_child_directory(path: &Path) -> Result<Option<PathBuf>, ScanError> {
    let mut versions = child_directories(path)?
        .into_iter()
        .map(|version_path| {
            let modified = fs::metadata(&version_path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH);
            (modified, version_path)
        })
        .collect::<Vec<_>>();
    versions.sort_by(|left, right| {
        left.0
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .cmp(&right.0.duration_since(UNIX_EPOCH).unwrap_or_default())
            .then_with(|| left.1.cmp(&right.1))
    });
    Ok(versions.pop().map(|(_, path)| path))
}

/// Local Refresh 在固定用户级根之外，只扫描用户明确登记且身份未变化的 Project。
pub fn scan_with_projects(
    paths: &ApplicationPaths,
    projects: &[StoredProject],
    excluded_skill_roots: &BTreeSet<PathBuf>,
) -> ScanResult {
    let mut result = scan_excluding(paths, excluded_skill_roots);
    let project_result = scan_projects(paths, projects, excluded_skill_roots);
    result.entries.extend(project_result.entries);
    result
        .successful_roots
        .extend(project_result.successful_roots);
    result.issues.extend(project_result.issues);
    result
        .entries
        .sort_by(|left, right| left.skill_root.cmp(&right.skill_root));
    result
}

/// Project 登记只扫描该 Project 的已存在目录，不触发用户级完整刷新。
pub fn scan_projects(
    paths: &ApplicationPaths,
    projects: &[StoredProject],
    excluded_skill_roots: &BTreeSet<PathBuf>,
) -> ScanResult {
    let installation_chains = read_lock_v3_installation_chains(paths);
    let context = ScanContext {
        excluded_skill_roots,
        installation_chains: &installation_chains,
    };
    let mut result = ScanResult {
        entries: Vec::new(),
        supported_apps: Vec::new(),
        successful_roots: Vec::new(),
        issues: Vec::new(),
    };
    for project in projects {
        let project_path = Path::new(&project.root_path);
        if let Err(error) = validate_project_root(project) {
            push_project_identity_issues(&mut result.issues, &error, &project.id);
            continue;
        }
        let entry_start = result.entries.len();
        let successful_start = result.successful_roots.len();
        let issue_start = result.issues.len();

        for app in paths.supported_apps() {
            let root = project_path.join(&app.project_relative_root);
            let identity = ScanRootIdentity::project(app.project_root_key, &project.id);
            match scan_optional_root(
                &root,
                app.project_root_key,
                InventoryLocationKind::AppProject,
                project_observers(app.id),
                Some(&project.id),
                Some(project_path),
                &context,
            ) {
                Ok(entries) => {
                    result.entries.extend(entries);
                    result.successful_roots.push(identity);
                }
                Err(error) => result
                    .issues
                    .push(error.as_issue(app.project_root_key, Some(&project.id))),
            }
        }

        let shared_root = project_path.join(".agents/skills");
        let shared_identity =
            ScanRootIdentity::project(ScanRootKey::SharedAgentsProject, &project.id);
        match scan_optional_root(
            &shared_root,
            ScanRootKey::SharedAgentsProject,
            InventoryLocationKind::SharedReadOnly,
            vec![SupportedAppId::Codex, SupportedAppId::GitHubCopilot],
            Some(&project.id),
            Some(project_path),
            &context,
        ) {
            Ok(entries) => {
                result.entries.extend(entries);
                result.successful_roots.push(shared_identity);
            }
            Err(error) => result
                .issues
                .push(error.as_issue(ScanRootKey::SharedAgentsProject, Some(&project.id))),
        }

        // 扫描期间若 Project 根被替换，丢弃本轮结果，不能把替代目录归到原 Project。
        if let Err(error) = validate_project_root(project) {
            result.entries.truncate(entry_start);
            result.successful_roots.truncate(successful_start);
            result.issues.truncate(issue_start);
            push_project_identity_issues(&mut result.issues, &error, &project.id);
        }
    }
    result
        .entries
        .sort_by(|left, right| left.skill_root.cmp(&right.skill_root));
    result
}

fn push_project_identity_issues(issues: &mut Vec<ScanIssue>, error: &ScanError, project_id: &str) {
    for root_key in project_scan_root_keys() {
        issues.push(error.as_issue(root_key, Some(project_id)));
    }
}

fn scan_optional_root(
    path: &Path,
    root_key: ScanRootKey,
    location_kind: InventoryLocationKind,
    observed_by: Vec<SupportedAppId>,
    project_id: Option<&str>,
    project_root: Option<&Path>,
    context: &ScanContext<'_>,
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
        if context.excluded_skill_roots.contains(&skill_root) {
            continue;
        }
        let file_type = child.file_type().map_err(|source| ScanError::ReadRoot {
            path: path.display().to_string(),
            source,
        })?;
        let is_reachable_directory = if file_type.is_symlink() {
            fs::metadata(&skill_root)
                .map_err(|source| read_skill_content_error(&skill_root, source))?
                .is_dir()
        } else {
            file_type.is_dir()
        };
        if !is_reachable_directory {
            continue;
        }

        let skill_file = skill_root.join("SKILL.md");
        if !is_regular_skill_file(&skill_file)? {
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
        let (management_kind, management_evidence) = match project_root {
            Some(project_root) => match inspect_git_head_management(project_root, &skill_file) {
                ManagementEvidenceInspection::Confirmed(evidence) => {
                    (ManagementKind::ProjectManaged, Some(evidence))
                }
                ManagementEvidenceInspection::Absent => (ManagementKind::TakeoverCandidate, None),
                ManagementEvidenceInspection::Indeterminate(source) => {
                    return Err(ScanError::InspectManagementEvidence {
                        path: skill_file.display().to_string(),
                        source,
                    });
                }
            },
            None => (ManagementKind::TakeoverCandidate, None),
        };
        let root_string = skill_root.to_string_lossy().into_owned();
        // 当前读取的是全局 lock；不能把同名证据附给项目仓库中的独立 Skill。
        let installation_chain = project_id
            .is_none()
            .then(|| context.installation_chains.get(&skill_name))
            .flatten()
            .cloned();
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
            project_id: project_id.map(str::to_owned),
            stale: false,
            management_kind,
            management_evidence,
            installation_chain,
        });
    }

    Ok(observations)
}

impl ScanError {
    fn as_issue(&self, root_key: ScanRootKey, project_id: Option<&str>) -> ScanIssue {
        let message = self.to_string();
        let (path, code) = match self {
            Self::InspectPath { path, .. } => (path.clone(), ScanIssueCode::InspectPath),
            Self::RootIsNotDirectory(path) => (path.clone(), ScanIssueCode::RootNotDirectory),
            Self::ReadRoot { path, .. } => (path.clone(), ScanIssueCode::ReadRoot),
            Self::ReadSkillContent { path, .. } => (path.clone(), ScanIssueCode::ReadSkillContent),
            Self::InspectManagementEvidence { path, .. } => {
                (path.clone(), ScanIssueCode::InspectManagementEvidence)
            }
            Self::ProjectChanged(path) => (path.clone(), ScanIssueCode::InspectPath),
        };
        let identity = match project_id {
            Some(project_id) => ScanRootIdentity::project(root_key, project_id),
            None => ScanRootIdentity::global(root_key),
        };
        ScanIssue {
            root_id: identity.stable_id(),
            root_key,
            project_id: project_id.map(str::to_owned),
            path,
            code,
            message,
        }
    }
}

fn validate_project_root(project: &StoredProject) -> Result<(), ScanError> {
    let path = Path::new(&project.root_path);
    let metadata = fs::symlink_metadata(path).map_err(|source| ScanError::InspectPath {
        path: project.root_path.clone(),
        source,
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.dev() != project.root_device
        || metadata.ino() != project.root_inode
    {
        return Err(ScanError::ProjectChanged(project.root_path.clone()));
    }
    Ok(())
}

fn project_scan_root_keys() -> [ScanRootKey; 4] {
    [
        ScanRootKey::CodexProject,
        ScanRootKey::ClaudeCodeProject,
        ScanRootKey::GitHubCopilotProject,
        ScanRootKey::SharedAgentsProject,
    ]
}

fn project_observers(app_id: SupportedAppId) -> Vec<SupportedAppId> {
    match app_id {
        SupportedAppId::ClaudeCode => {
            vec![SupportedAppId::ClaudeCode, SupportedAppId::GitHubCopilot]
        }
        other => vec![other],
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
    let mut content_fingerprint = StableFingerprint::new();
    let mut content_length = 0_u64;
    loop {
        let bytes_read = file
            .read(&mut buffer)
            .map_err(|source| read_skill_content_error(path, source))?;
        if bytes_read == 0 {
            break;
        }
        content_fingerprint.write(&buffer[..bytes_read]);
        content_length = content_length.wrapping_add(bytes_read as u64);
    }
    // 文件长度与独立内容摘要形成固定边界，不能被后续路径编码伪装。
    fingerprint.write_segment(&content_length.to_le_bytes());
    fingerprint.write_segment(&content_fingerprint.finish().to_le_bytes());
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
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            match fs::symlink_metadata(path) {
                // 路径条目存在但目标不可达时不能伪装成“尚未安装”。
                Ok(_) => Err(ScanError::InspectPath {
                    path: path.display().to_string(),
                    source,
                }),
                Err(link_error) if link_error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(link_error) => Err(ScanError::InspectPath {
                    path: path.display().to_string(),
                    source: link_error,
                }),
            }
        }
        Err(source) => Err(ScanError::InspectPath {
            path: path.display().to_string(),
            source,
        }),
    }
}

fn is_regular_skill_file(path: &Path) -> Result<bool, ScanError> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            match fs::symlink_metadata(path) {
                // 断链 SKILL.md 是不可读内容，不等同于目录里没有 SKILL.md。
                Ok(_) => Err(read_skill_content_error(path, source)),
                Err(link_error) if link_error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(link_error) => Err(read_skill_content_error(path, link_error)),
            }
        }
        Err(source) => Err(read_skill_content_error(path, source)),
    }
}

fn extract_frontmatter(contents: &str) -> Option<&str> {
    let body = contents.strip_prefix("---\n")?;
    let end = body.find("\n---")?;
    Some(&body[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn file_content_is_framed_separately_from_following_tree_entries() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let split_tree = sandbox.path().join("split");
        let joined_tree = sandbox.path().join("joined");
        fs::create_dir_all(&split_tree).expect("应创建分离文件树");
        fs::create_dir_all(&joined_tree).expect("应创建拼接文件树");
        fs::write(split_tree.join("a"), b"left").expect("应写入第一个文件");
        fs::write(split_tree.join("b"), b"right").expect("应写入第二个文件");

        // 这段内容在旧编码中会伪装成后续 b 文件的路径、类型和内容。
        let mut crafted = b"left".to_vec();
        crafted.extend_from_slice(&(1_u64).to_le_bytes());
        crafted.extend_from_slice(b"b");
        crafted.extend_from_slice(&(4_u64).to_le_bytes());
        crafted.extend_from_slice(b"file");
        crafted.extend_from_slice(b"right");
        fs::write(joined_tree.join("a"), crafted).expect("应写入边界碰撞样本");

        assert_ne!(
            fingerprint_skill_root(&split_tree).expect("应计算分离文件树指纹"),
            fingerprint_skill_root(&joined_tree).expect("应计算拼接文件树指纹")
        );
    }
}
