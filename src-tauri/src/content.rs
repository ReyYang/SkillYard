use std::{
    collections::BTreeMap,
    ffi::{CString, OsStr, OsString},
    fmt::Write as _,
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    os::unix::{
        ffi::OsStrExt,
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

const MAX_ENTRIES: usize = 20_000;
const MAX_TOTAL_FILE_BYTES: u64 = 512 * 1_048_576;
const MAX_SINGLE_FILE_BYTES: u64 = 100 * 1_048_576;
const FINGERPRINT_VERSION: &[u8] = b"skillyard-single-skill-v1";
const EXECUTABLE_WARNING: &str = "内容包含可执行文件或 shebang，请在挂载前确认风险";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSingleSkill {
    pub canonical_root: PathBuf,
    pub name: String,
    pub description: String,
    pub fingerprint: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ContentValidationError {
    #[error("无法{action} {path}：{source}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Skill 输入根目录不能是软链接：{0}")]
    RootSymlink(String),
    #[error("Skill 输入根路径不是文件夹：{0}")]
    RootNotDirectory(String),
    #[error("Skill 根目录缺少普通文件 SKILL.md：{0}")]
    MissingSkillMetadata(String),
    #[error("Skill 根目录中的 SKILL.md 必须是普通文件：{0}")]
    SkillMetadataNotRegular(String),
    #[error("单 Skill 文件夹不支持嵌套 SKILL.md：{0}")]
    NestedSkillUnsupported(String),
    #[error("SKILL.md metadata 无效：{0}")]
    InvalidMetadata(String),
    #[error("Skill 内容包含不安全的{kind}：{path}")]
    UnsafeEntry { path: String, kind: &'static str },
    #[error("Skill 内容包含硬链接文件：{path}（链接数 {links}）")]
    HardLinkedFile { path: String, links: u64 },
    #[error("Skill 内容条目数超过固定上限 {limit}：已检测到 {actual}")]
    EntryLimitExceeded { limit: usize, actual: usize },
    #[error("普通文件总量超过固定上限 {limit} bytes：已检测到 {actual} bytes")]
    TotalSizeLimitExceeded { limit: u64, actual: u64 },
    #[error("普通文件超过固定单文件上限 {limit} bytes：{path} 为 {actual} bytes")]
    FileSizeLimitExceeded {
        path: String,
        limit: u64,
        actual: u64,
    },
    #[error("验证期间 Skill 内容发生变化，请重新生成计划：{0}")]
    SourceChanged(String),
    #[error("复制目标已经存在，不能覆盖：{0}")]
    DestinationExists(String),
    #[error("复制目标的父目录不可安全使用：{0}")]
    InvalidDestinationParent(String),
    #[error("复制目标不能位于输入 Skill 内容之内：{0}")]
    DestinationInsideSource(String),
    #[error("复制后的内容与已验证输入不一致：{0}")]
    CopyVerificationFailed(String),
    #[error("复制失败且无法完整清理本次目标 {path}：原错误：{original}；清理错误：{cleanup}")]
    CopyCleanupFailed {
        path: String,
        original: String,
        cleanup: String,
    },
}

#[derive(Debug, Clone, Copy)]
struct ContentLimits {
    max_entries: usize,
    max_total_file_bytes: u64,
    max_single_file_bytes: u64,
}

impl ContentLimits {
    const PRODUCTION: Self = Self {
        max_entries: MAX_ENTRIES,
        max_total_file_bytes: MAX_TOTAL_FILE_BYTES,
        max_single_file_bytes: MAX_SINGLE_FILE_BYTES,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
}

impl EntryKind {
    fn fingerprint_tag(self) -> &'static [u8] {
        match self {
            Self::Directory => b"directory",
            Self::File => b"file",
        }
    }
}

/// 这些字段共同绑定验证时看到的 inode；复制阶段任一字段变化都让旧快照失效。
#[derive(Debug, Clone, PartialEq, Eq)]
struct EntryIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl EntryIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            links: metadata.nlink(),
            length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeEntry {
    relative_path: PathBuf,
    kind: EntryKind,
    permissions: u32,
    length: u64,
    identity: EntryIdentity,
}

#[derive(Debug)]
struct InspectedTree {
    canonical_root: PathBuf,
    root_identity: EntryIdentity,
    root_permissions: u32,
    entries: Vec<TreeEntry>,
    skill_metadata_bytes: Vec<u8>,
    fingerprint: String,
    has_executable_risk: bool,
}

#[derive(Debug)]
struct ValidatedTree {
    skill: ValidatedSingleSkill,
    root_identity: EntryIdentity,
    root_permissions: u32,
    entries: Vec<TreeEntry>,
}

struct DestinationParent {
    handle: File,
    canonical_path: PathBuf,
    root_name: OsString,
    root_path: PathBuf,
}

struct CreatedDestination {
    root_handle: Option<File>,
    directories: BTreeMap<PathBuf, File>,
}

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
}

pub fn validate_single_skill_folder(
    root: &Path,
) -> Result<ValidatedSingleSkill, ContentValidationError> {
    validate_single_skill_folder_with_limits(root, ContentLimits::PRODUCTION)
}

#[cfg(test)]
pub fn copy_single_skill_tree(
    source: &Path,
    destination: &Path,
) -> Result<(), ContentValidationError> {
    copy_single_skill_tree_with_hooks(source, destination, || {}, || {})
}

/// 生命周期层已经安全打开目标父目录时，复制始终锚定该 dirfd，不再重新跟随可见祖先路径。
pub fn copy_single_skill_tree_into_open_directory(
    source: &Path,
    destination_parent: &File,
    destination_parent_path: &Path,
    destination_name: &OsStr,
) -> Result<(), ContentValidationError> {
    let validated = validate_tree(source, ContentLimits::PRODUCTION)?;
    let destination_parent = prepare_open_destination(
        &validated.skill.canonical_root,
        destination_parent,
        destination_parent_path,
        destination_name,
    )?;
    copy_validated_tree(validated, destination_parent, || {}, || {})
}

#[cfg(test)]
fn copy_single_skill_tree_with_hooks(
    source: &Path,
    destination: &Path,
    after_parent_opened: impl FnOnce(),
    after_entries_copied: impl FnOnce(),
) -> Result<(), ContentValidationError> {
    let validated = validate_tree(source, ContentLimits::PRODUCTION)?;
    let destination_parent = prepare_destination(&validated.skill.canonical_root, destination)?;
    copy_validated_tree(
        validated,
        destination_parent,
        after_parent_opened,
        after_entries_copied,
    )
}

fn copy_validated_tree(
    validated: ValidatedTree,
    destination_parent: DestinationParent,
    after_parent_opened: impl FnOnce(),
    after_entries_copied: impl FnOnce(),
) -> Result<(), ContentValidationError> {
    after_parent_opened();
    let mut created = create_destination_root(&destination_parent)?;

    let copy_result = (|| {
        copy_validated_entries(&validated, &destination_parent, &mut created)?;
        after_entries_copied();

        let source_after =
            validate_tree(&validated.skill.canonical_root, ContentLimits::PRODUCTION)?;
        if !same_source_snapshot(&validated, &source_after) {
            return Err(ContentValidationError::SourceChanged(display_path(
                &validated.skill.canonical_root,
            )));
        }

        ensure_destination_path_identity(&destination_parent, &created)?;
        let destination_tree =
            inspect_tree(&destination_parent.root_path, ContentLimits::PRODUCTION)?;
        ensure_destination_path_identity(&destination_parent, &created)?;
        if destination_tree.fingerprint != validated.skill.fingerprint {
            return Err(ContentValidationError::CopyVerificationFailed(
                display_path(&destination_parent.root_path),
            ));
        }

        Ok(())
    })();

    if let Err(original) = copy_result {
        if let Err(cleanup) =
            cleanup_created_destination(&destination_parent, &mut created, &validated.entries)
        {
            return Err(ContentValidationError::CopyCleanupFailed {
                path: display_path(&destination_parent.root_path),
                original: original.to_string(),
                cleanup: cleanup.to_string(),
            });
        }
        return Err(original);
    }

    Ok(())
}

fn validate_single_skill_folder_with_limits(
    root: &Path,
    limits: ContentLimits,
) -> Result<ValidatedSingleSkill, ContentValidationError> {
    Ok(validate_tree(root, limits)?.skill)
}

fn validate_tree(
    root: &Path,
    limits: ContentLimits,
) -> Result<ValidatedTree, ContentValidationError> {
    let inspected = inspect_tree(root, limits)?;
    let directory_name = inspected
        .canonical_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            ContentValidationError::InvalidMetadata(
                "Skill 根目录名必须是有效 UTF-8 名称".to_owned(),
            )
        })?;
    let (name, description) =
        parse_skill_metadata(&inspected.skill_metadata_bytes, directory_name)?;
    let warnings = if inspected.has_executable_risk {
        vec![EXECUTABLE_WARNING.to_owned()]
    } else {
        Vec::new()
    };

    Ok(ValidatedTree {
        skill: ValidatedSingleSkill {
            canonical_root: inspected.canonical_root,
            name,
            description,
            fingerprint: inspected.fingerprint,
            warnings,
        },
        root_identity: inspected.root_identity,
        root_permissions: inspected.root_permissions,
        entries: inspected.entries,
    })
}

fn inspect_tree(
    root: &Path,
    limits: ContentLimits,
) -> Result<InspectedTree, ContentValidationError> {
    let supplied_metadata = symlink_metadata(root, "检查 Skill 输入根目录")?;
    if supplied_metadata.file_type().is_symlink() {
        return Err(ContentValidationError::RootSymlink(display_path(root)));
    }
    if !supplied_metadata.is_dir() {
        return Err(ContentValidationError::RootNotDirectory(display_path(root)));
    }

    let canonical_root =
        fs::canonicalize(root).map_err(|source| io_error("解析 Skill 输入根目录", root, source))?;
    let root_metadata = symlink_metadata(&canonical_root, "重新检查 Skill 输入根目录")?;
    if root_metadata.file_type().is_symlink()
        || EntryIdentity::from_metadata(&supplied_metadata)
            != EntryIdentity::from_metadata(&root_metadata)
    {
        return Err(ContentValidationError::SourceChanged(display_path(root)));
    }

    let skill_metadata_path = canonical_root.join("SKILL.md");
    let skill_file_metadata = match fs::symlink_metadata(&skill_metadata_path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(ContentValidationError::MissingSkillMetadata(display_path(
                &skill_metadata_path,
            )));
        }
        Err(source) => {
            return Err(io_error("检查根部 SKILL.md", &skill_metadata_path, source));
        }
    };
    if !skill_file_metadata.file_type().is_file() {
        return Err(ContentValidationError::SkillMetadataNotRegular(
            display_path(&skill_metadata_path),
        ));
    }

    // 单 Skill 输入先完整寻找嵌套边界，避免较早排序的不安全条目掩盖更准确的产品错误。
    reject_nested_skill_metadata(&canonical_root, limits.max_entries)?;

    let root_identity = EntryIdentity::from_metadata(&root_metadata);
    let root_permissions = permission_bits(&root_metadata);
    let mut hasher = Sha256::new();
    write_frame(&mut hasher, FINGERPRINT_VERSION);
    hash_entry_metadata(
        &mut hasher,
        Path::new(""),
        EntryKind::Directory,
        root_permissions,
        0,
    );
    write_frame(&mut hasher, &[]);

    let mut entries = Vec::new();
    let mut stack = vec![(canonical_root.clone(), root_identity.clone())];
    let mut entry_count = 0_usize;
    let mut total_file_bytes = 0_u64;
    let mut skill_metadata_bytes = None;
    let mut has_executable_risk = false;

    while let Some((directory, expected_directory_identity)) = stack.pop() {
        ensure_canonical_path(&canonical_root, &directory)?;
        ensure_unchanged(&directory, &expected_directory_identity)?;
        let mut children = fs::read_dir(&directory)
            .map_err(|source| io_error("读取 Skill 目录", &directory, source))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| io_error("读取 Skill 目录项", &directory, source))?;
        children.sort_by_key(|entry| entry.file_name());

        let mut child_directories = Vec::new();
        for child in children {
            let path = child.path();
            let relative_path = path
                .strip_prefix(&canonical_root)
                .map_err(|_| ContentValidationError::SourceChanged(display_path(&path)))?
                .to_path_buf();

            if relative_path != Path::new("SKILL.md")
                && relative_path
                    .file_name()
                    .is_some_and(|name| name == "SKILL.md")
            {
                return Err(ContentValidationError::NestedSkillUnsupported(
                    display_path(&path),
                ));
            }

            entry_count =
                entry_count
                    .checked_add(1)
                    .ok_or(ContentValidationError::EntryLimitExceeded {
                        limit: limits.max_entries,
                        actual: usize::MAX,
                    })?;
            if entry_count > limits.max_entries {
                return Err(ContentValidationError::EntryLimitExceeded {
                    limit: limits.max_entries,
                    actual: entry_count,
                });
            }

            let metadata = symlink_metadata(&path, "检查 Skill 内容")?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(ContentValidationError::UnsafeEntry {
                    path: display_path(&path),
                    kind: "软链接",
                });
            }

            let identity = EntryIdentity::from_metadata(&metadata);
            let permissions = permission_bits(&metadata);
            if file_type.is_dir() {
                ensure_canonical_path(&canonical_root, &path)?;
                hash_entry_metadata(
                    &mut hasher,
                    &relative_path,
                    EntryKind::Directory,
                    permissions,
                    0,
                );
                write_frame(&mut hasher, &[]);
                entries.push(TreeEntry {
                    relative_path,
                    kind: EntryKind::Directory,
                    permissions,
                    length: 0,
                    identity: identity.clone(),
                });
                child_directories.push((path, identity));
                continue;
            }

            if !file_type.is_file() {
                return Err(ContentValidationError::UnsafeEntry {
                    path: display_path(&path),
                    kind: special_file_kind(&file_type),
                });
            }
            if metadata.nlink() > 1 {
                return Err(ContentValidationError::HardLinkedFile {
                    path: display_path(&path),
                    links: metadata.nlink(),
                });
            }
            if metadata.len() > limits.max_single_file_bytes {
                return Err(ContentValidationError::FileSizeLimitExceeded {
                    path: display_path(&path),
                    limit: limits.max_single_file_bytes,
                    actual: metadata.len(),
                });
            }
            total_file_bytes = total_file_bytes.checked_add(metadata.len()).ok_or(
                ContentValidationError::TotalSizeLimitExceeded {
                    limit: limits.max_total_file_bytes,
                    actual: u64::MAX,
                },
            )?;
            if total_file_bytes > limits.max_total_file_bytes {
                return Err(ContentValidationError::TotalSizeLimitExceeded {
                    limit: limits.max_total_file_bytes,
                    actual: total_file_bytes,
                });
            }

            ensure_canonical_path(&canonical_root, &path)?;
            hash_entry_metadata(
                &mut hasher,
                &relative_path,
                EntryKind::File,
                permissions,
                metadata.len(),
            );
            let capture_metadata = relative_path == Path::new("SKILL.md");
            let (captured, has_shebang) = hash_regular_file(
                &path,
                &identity,
                metadata.len(),
                capture_metadata,
                &mut hasher,
            )?;
            if capture_metadata {
                skill_metadata_bytes = Some(captured);
            }
            has_executable_risk |= permissions & 0o111 != 0 || has_shebang;
            entries.push(TreeEntry {
                relative_path,
                kind: EntryKind::File,
                permissions,
                length: metadata.len(),
                identity,
            });
        }

        ensure_unchanged(&directory, &expected_directory_identity)?;
        for child_directory in child_directories.into_iter().rev() {
            stack.push(child_directory);
        }
    }

    let root_after = symlink_metadata(&canonical_root, "完成 Skill 检查")?;
    if EntryIdentity::from_metadata(&root_after) != root_identity {
        return Err(ContentValidationError::SourceChanged(display_path(
            &canonical_root,
        )));
    }

    let skill_metadata_bytes = skill_metadata_bytes.ok_or_else(|| {
        ContentValidationError::MissingSkillMetadata(display_path(&skill_metadata_path))
    })?;
    let digest = hasher.finalize();
    let mut fingerprint = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut fingerprint, "{byte:02x}").expect("写入 String 不会失败");
    }

    Ok(InspectedTree {
        canonical_root,
        root_identity,
        root_permissions,
        entries,
        skill_metadata_bytes,
        fingerprint,
        has_executable_risk,
    })
}

fn reject_nested_skill_metadata(
    canonical_root: &Path,
    max_entries: usize,
) -> Result<(), ContentValidationError> {
    let mut stack = vec![canonical_root.to_path_buf()];
    let mut inspected_entries = 0_usize;

    while let Some(directory) = stack.pop() {
        let mut children = fs::read_dir(&directory)
            .map_err(|source| io_error("检查嵌套 Skill", &directory, source))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| io_error("读取嵌套 Skill 目录项", &directory, source))?;
        children.sort_by_key(|entry| entry.file_name());

        let mut child_directories = Vec::new();
        for child in children {
            let path = child.path();
            let relative = path
                .strip_prefix(canonical_root)
                .map_err(|_| ContentValidationError::SourceChanged(display_path(&path)))?;
            if relative != Path::new("SKILL.md")
                && relative.file_name().is_some_and(|name| name == "SKILL.md")
            {
                return Err(ContentValidationError::NestedSkillUnsupported(
                    display_path(&path),
                ));
            }

            inspected_entries = inspected_entries.checked_add(1).ok_or(
                ContentValidationError::EntryLimitExceeded {
                    limit: max_entries,
                    actual: usize::MAX,
                },
            )?;
            if inspected_entries > max_entries {
                return Err(ContentValidationError::EntryLimitExceeded {
                    limit: max_entries,
                    actual: inspected_entries,
                });
            }

            let metadata = symlink_metadata(&path, "检查嵌套 Skill 内容")?;
            if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                ensure_canonical_path(canonical_root, &path)?;
                child_directories.push(path);
            }
        }

        for child_directory in child_directories.into_iter().rev() {
            stack.push(child_directory);
        }
    }

    Ok(())
}

fn parse_skill_metadata(
    bytes: &[u8],
    directory_name: &str,
) -> Result<(String, String), ContentValidationError> {
    let contents = std::str::from_utf8(bytes).map_err(|_| {
        ContentValidationError::InvalidMetadata("SKILL.md 必须使用 UTF-8 编码".to_owned())
    })?;
    let frontmatter = extract_frontmatter(contents).ok_or_else(|| {
        ContentValidationError::InvalidMetadata("缺少以 --- 分隔的 YAML frontmatter".to_owned())
    })?;
    let metadata = serde_yaml_ng::from_str::<SkillFrontmatter>(frontmatter).map_err(|error| {
        ContentValidationError::InvalidMetadata(format!("无法解析 YAML：{error}"))
    })?;
    let name = metadata
        .name
        .ok_or_else(|| ContentValidationError::InvalidMetadata("缺少必填字段 name".to_owned()))?;
    let description = metadata.description.ok_or_else(|| {
        ContentValidationError::InvalidMetadata("缺少必填字段 description".to_owned())
    })?;

    if !is_valid_skill_name(&name, directory_name) {
        return Err(ContentValidationError::InvalidMetadata(format!(
            "name 必须与目录名 {directory_name} 一致，并使用 1-64 位小写字母、数字或单个连字符"
        )));
    }
    let description_length = description.chars().count();
    if !(1..=1024).contains(&description_length) || description.trim().is_empty() {
        return Err(ContentValidationError::InvalidMetadata(
            "description 原始字段必须为 1-1024 个字符且不能全为空白".to_owned(),
        ));
    }

    Ok((name, description))
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

fn extract_frontmatter(contents: &str) -> Option<&str> {
    let body = contents.strip_prefix("---\n")?;
    if let Some(end) = body.find("\n---\n") {
        return Some(&body[..end]);
    }
    body.strip_suffix("\n---")
}

fn hash_regular_file(
    path: &Path,
    expected_identity: &EntryIdentity,
    expected_length: u64,
    capture_contents: bool,
    hasher: &mut Sha256,
) -> Result<(Vec<u8>, bool), ContentValidationError> {
    let mut file = open_regular_file(path)?;
    let opened_metadata = file
        .metadata()
        .map_err(|source| io_error("检查已打开文件", path, source))?;
    if EntryIdentity::from_metadata(&opened_metadata) != *expected_identity {
        return Err(ContentValidationError::SourceChanged(display_path(path)));
    }

    write_frame_length(hasher, expected_length);
    let mut captured = if capture_contents {
        Vec::with_capacity(usize::try_from(expected_length).unwrap_or(0))
    } else {
        Vec::new()
    };
    let mut prefix = Vec::with_capacity(2);
    let mut actual_length = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let bytes_read = file
            .read(&mut buffer)
            .map_err(|source| io_error("读取普通文件", path, source))?;
        if bytes_read == 0 {
            break;
        }
        actual_length = actual_length
            .checked_add(bytes_read as u64)
            .ok_or_else(|| ContentValidationError::SourceChanged(display_path(path)))?;
        if actual_length > expected_length {
            return Err(ContentValidationError::SourceChanged(display_path(path)));
        }
        let chunk = &buffer[..bytes_read];
        hasher.update(chunk);
        if prefix.len() < 2 {
            let needed = 2 - prefix.len();
            prefix.extend_from_slice(&chunk[..chunk.len().min(needed)]);
        }
        if capture_contents {
            captured.extend_from_slice(chunk);
        }
    }
    if actual_length != expected_length {
        return Err(ContentValidationError::SourceChanged(display_path(path)));
    }

    let after_open_metadata = file
        .metadata()
        .map_err(|source| io_error("重新检查已打开文件", path, source))?;
    if EntryIdentity::from_metadata(&after_open_metadata) != *expected_identity {
        return Err(ContentValidationError::SourceChanged(display_path(path)));
    }
    ensure_unchanged(path, expected_identity)?;

    Ok((captured, prefix.as_slice() == b"#!"))
}

#[cfg(test)]
fn prepare_destination(
    canonical_source: &Path,
    destination: &Path,
) -> Result<DestinationParent, ContentValidationError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_metadata = symlink_metadata(parent, "检查复制目标父目录")?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(ContentValidationError::InvalidDestinationParent(
            display_path(parent),
        ));
    }
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|source| io_error("解析复制目标父目录", parent, source))?;
    let canonical_parent_metadata = symlink_metadata(&canonical_parent, "检查复制目标父目录")?;
    if EntryIdentity::from_metadata(&parent_metadata)
        != EntryIdentity::from_metadata(&canonical_parent_metadata)
    {
        return Err(ContentValidationError::InvalidDestinationParent(
            display_path(parent),
        ));
    }
    let parent_handle = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&canonical_parent)
        .map_err(|source| io_error("安全打开复制目标父目录", &canonical_parent, source))?;
    let opened_parent_metadata = parent_handle
        .metadata()
        .map_err(|source| io_error("检查已打开的目标父目录", &canonical_parent, source))?;
    if EntryIdentity::from_metadata(&opened_parent_metadata)
        != EntryIdentity::from_metadata(&canonical_parent_metadata)
    {
        return Err(ContentValidationError::InvalidDestinationParent(
            display_path(&canonical_parent),
        ));
    }

    let file_name = destination.file_name().ok_or_else(|| {
        ContentValidationError::InvalidDestinationParent(display_path(destination))
    })?;
    let root_name = file_name.to_os_string();
    let canonical_destination = canonical_parent.join(&root_name);
    if canonical_destination.starts_with(canonical_source)
        || canonical_source.starts_with(&canonical_destination)
    {
        return Err(ContentValidationError::DestinationInsideSource(
            display_path(&canonical_destination),
        ));
    }

    Ok(DestinationParent {
        handle: parent_handle,
        canonical_path: canonical_parent,
        root_name,
        root_path: canonical_destination,
    })
}

fn prepare_open_destination(
    canonical_source: &Path,
    parent_handle: &File,
    parent_path: &Path,
    root_name: &OsStr,
) -> Result<DestinationParent, ContentValidationError> {
    if Path::new(root_name).components().count() != 1
        || matches!(root_name.as_bytes(), b"." | b"..")
    {
        return Err(ContentValidationError::InvalidDestinationParent(
            display_path(parent_path),
        ));
    }
    let canonical_parent = fs::canonicalize(parent_path)
        .map_err(|source| io_error("解析复制目标父目录", parent_path, source))?;
    ensure_open_directory_matches_path(parent_handle, &canonical_parent)?;
    let handle = parent_handle
        .try_clone()
        .map_err(|source| io_error("保留复制目标父目录", &canonical_parent, source))?;
    let root_name = root_name.to_os_string();
    let root_path = canonical_parent.join(&root_name);
    if root_path.starts_with(canonical_source) || canonical_source.starts_with(&root_path) {
        return Err(ContentValidationError::DestinationInsideSource(
            display_path(&root_path),
        ));
    }
    Ok(DestinationParent {
        handle,
        canonical_path: canonical_parent,
        root_name,
        root_path,
    })
}

fn create_destination_root(
    destination: &DestinationParent,
) -> Result<CreatedDestination, ContentValidationError> {
    if let Err(source) = mkdir_at(&destination.handle, &destination.root_name, 0o700) {
        return if source.kind() == std::io::ErrorKind::AlreadyExists {
            Err(ContentValidationError::DestinationExists(display_path(
                &destination.root_path,
            )))
        } else {
            Err(io_error("创建复制目标", &destination.root_path, source))
        };
    }

    let root_handle = match open_directory_at(&destination.handle, &destination.root_name) {
        Ok(handle) => handle,
        Err(error) => {
            let _ = unlink_at(&destination.handle, &destination.root_name, true);
            return Err(error);
        }
    };
    let root_clone = match root_handle.try_clone() {
        Ok(handle) => handle,
        Err(source) => {
            drop(root_handle);
            let _ = unlink_at(&destination.handle, &destination.root_name, true);
            return Err(io_error(
                "保留目标根目录句柄",
                &destination.root_path,
                source,
            ));
        }
    };
    let mut directories = BTreeMap::new();
    directories.insert(PathBuf::new(), root_clone);
    Ok(CreatedDestination {
        root_handle: Some(root_handle),
        directories,
    })
}

fn copy_validated_entries(
    validated: &ValidatedTree,
    destination_parent: &DestinationParent,
    destination: &mut CreatedDestination,
) -> Result<(), ContentValidationError> {
    for entry in &validated.entries {
        if entry.kind != EntryKind::Directory {
            continue;
        }
        let (parent_relative, name) = relative_parent_and_name(&entry.relative_path)?;
        let parent_handle = destination
            .directories
            .get(parent_relative)
            .ok_or_else(|| {
                ContentValidationError::CopyVerificationFailed(display_path(
                    &destination_parent.root_path.join(&entry.relative_path),
                ))
            })?;
        mkdir_at(parent_handle, name, 0o700).map_err(|source| {
            io_error(
                "创建目标目录",
                &destination_parent.root_path.join(&entry.relative_path),
                source,
            )
        })?;
        let child_handle = open_directory_at(parent_handle, name)?;
        destination
            .directories
            .insert(entry.relative_path.clone(), child_handle);
    }

    for entry in &validated.entries {
        if entry.kind != EntryKind::File {
            continue;
        }
        let source = validated.skill.canonical_root.join(&entry.relative_path);
        let (parent_relative, name) = relative_parent_and_name(&entry.relative_path)?;
        let parent_handle = destination
            .directories
            .get(parent_relative)
            .ok_or_else(|| {
                ContentValidationError::CopyVerificationFailed(display_path(
                    &destination_parent.root_path.join(&entry.relative_path),
                ))
            })?;
        copy_regular_file(
            &validated.skill.canonical_root,
            &source,
            parent_handle,
            name,
            &destination_parent.root_path.join(&entry.relative_path),
            entry,
        )?;
    }

    // 目录权限最后应用，避免只读目录阻止其余普通文件安全写入。
    for entry in validated.entries.iter().rev() {
        if entry.kind == EntryKind::Directory {
            let handle = destination
                .directories
                .get(&entry.relative_path)
                .ok_or_else(|| {
                    ContentValidationError::CopyVerificationFailed(display_path(
                        &destination_parent.root_path.join(&entry.relative_path),
                    ))
                })?;
            set_file_permissions(
                handle,
                entry.permissions,
                &destination_parent.root_path.join(&entry.relative_path),
            )?;
        }
    }
    let root_handle = destination.root_handle.as_ref().ok_or_else(|| {
        ContentValidationError::CopyVerificationFailed(display_path(&destination_parent.root_path))
    })?;
    set_file_permissions(
        root_handle,
        validated.root_permissions,
        &destination_parent.root_path,
    )?;
    for handle in destination.directories.values().rev() {
        handle.sync_all().map_err(|source| {
            io_error("同步复制目标目录", &destination_parent.root_path, source)
        })?;
    }
    destination_parent.handle.sync_all().map_err(|source| {
        io_error(
            "同步复制目标父目录",
            &destination_parent.canonical_path,
            source,
        )
    })?;
    Ok(())
}

fn copy_regular_file(
    canonical_source_root: &Path,
    source: &Path,
    destination_parent: &File,
    destination_name: &OsStr,
    destination_path: &Path,
    entry: &TreeEntry,
) -> Result<(), ContentValidationError> {
    ensure_canonical_path(canonical_source_root, source)?;
    ensure_unchanged(source, &entry.identity)?;
    let mut input = open_regular_file(source)?;
    let opened_metadata = input
        .metadata()
        .map_err(|error| io_error("检查复制输入文件", source, error))?;
    if EntryIdentity::from_metadata(&opened_metadata) != entry.identity {
        return Err(ContentValidationError::SourceChanged(display_path(source)));
    }

    let mut output = create_regular_file_at(destination_parent, destination_name)
        .map_err(|error| io_error("创建目标普通文件", destination_path, error))?;
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let bytes_read = input
            .read(&mut buffer)
            .map_err(|error| io_error("读取复制输入文件", source, error))?;
        if bytes_read == 0 {
            break;
        }
        copied = copied
            .checked_add(bytes_read as u64)
            .ok_or_else(|| ContentValidationError::SourceChanged(display_path(source)))?;
        if copied > entry.length {
            return Err(ContentValidationError::SourceChanged(display_path(source)));
        }
        output
            .write_all(&buffer[..bytes_read])
            .map_err(|error| io_error("写入目标普通文件", destination_path, error))?;
    }
    if copied != entry.length {
        return Err(ContentValidationError::SourceChanged(display_path(source)));
    }
    output
        .flush()
        .map_err(|error| io_error("刷新目标普通文件", destination_path, error))?;
    set_file_permissions(&output, entry.permissions, destination_path)?;
    output
        .sync_all()
        .map_err(|error| io_error("同步目标普通文件", destination_path, error))?;

    let input_after = input
        .metadata()
        .map_err(|error| io_error("重新检查复制输入文件", source, error))?;
    if EntryIdentity::from_metadata(&input_after) != entry.identity {
        return Err(ContentValidationError::SourceChanged(display_path(source)));
    }
    ensure_unchanged(source, &entry.identity)?;
    Ok(())
}

fn ensure_destination_path_identity(
    destination: &DestinationParent,
    created: &CreatedDestination,
) -> Result<(), ContentValidationError> {
    ensure_open_directory_matches_path(&destination.handle, &destination.canonical_path)?;
    let root_handle = created.root_handle.as_ref().ok_or_else(|| {
        ContentValidationError::CopyVerificationFailed(display_path(&destination.root_path))
    })?;
    ensure_open_directory_matches_path(root_handle, &destination.root_path)
}

fn ensure_open_directory_matches_path(
    handle: &File,
    path: &Path,
) -> Result<(), ContentValidationError> {
    let opened = handle
        .metadata()
        .map_err(|source| io_error("检查已打开目录", path, source))?;
    let path_metadata = symlink_metadata(path, "检查目标目录路径")?;
    if path_metadata.file_type().is_symlink()
        || EntryIdentity::from_metadata(&opened) != EntryIdentity::from_metadata(&path_metadata)
    {
        return Err(ContentValidationError::CopyVerificationFailed(
            display_path(path),
        ));
    }
    Ok(())
}

fn cleanup_created_destination(
    destination: &DestinationParent,
    created: &mut CreatedDestination,
    entries: &[TreeEntry],
) -> Result<(), ContentValidationError> {
    // 清理只通过本次持有的目录句柄进行，路径被换成 symlink 时也不会越出新建目标。
    for (relative, handle) in &created.directories {
        set_file_permissions(handle, 0o700, &destination.root_path.join(relative))?;
    }

    let root_handle = created.root_handle.as_ref().ok_or_else(|| {
        ContentValidationError::CopyVerificationFailed(display_path(&destination.root_path))
    })?;
    remove_known_tree_entries(root_handle, &created.directories, entries)?;

    created.directories.clear();
    created.root_handle.take();
    match unlink_at(&destination.handle, &destination.root_name, true) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error("清理复制目标", &destination.root_path, source)),
    }
}

fn remove_known_tree_entries(
    root_handle: &File,
    directories: &BTreeMap<PathBuf, File>,
    entries: &[TreeEntry],
) -> Result<(), ContentValidationError> {
    // 反序移除已知清单，保证普通文件先于其父目录；unlinkat 永不跟随最终 symlink。
    for entry in entries.iter().rev() {
        let (parent_relative, name) = relative_parent_and_name(&entry.relative_path)?;
        let parent_handle = if parent_relative.as_os_str().is_empty() {
            root_handle
        } else {
            directories.get(parent_relative).ok_or_else(|| {
                ContentValidationError::CopyVerificationFailed("清理目标目录句柄缺失".to_owned())
            })?
        };
        match unlink_at(parent_handle, name, entry.kind == EntryKind::Directory) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(io_error("清理复制内容", &entry.relative_path, source));
            }
        }
    }
    Ok(())
}

fn same_source_snapshot(before: &ValidatedTree, after: &ValidatedTree) -> bool {
    before.skill.canonical_root == after.skill.canonical_root
        && before.skill.fingerprint == after.skill.fingerprint
        && before.root_identity == after.root_identity
        && before.entries == after.entries
}

fn relative_parent_and_name(
    relative_path: &Path,
) -> Result<(&Path, &OsStr), ContentValidationError> {
    let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
    let name = relative_path.file_name().ok_or_else(|| {
        ContentValidationError::CopyVerificationFailed(display_path(relative_path))
    })?;
    Ok((parent, name))
}

fn mkdir_at(parent: &File, name: &OsStr, mode: u32) -> std::io::Result<()> {
    let name = c_string(name)?;
    // dirfd 把创建范围固定在已打开目录；即使可见路径被换成 symlink，也不会越界写入。
    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), mode as libc::mode_t) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn open_directory_at(parent: &File, name: &OsStr) -> Result<File, ContentValidationError> {
    let encoded =
        c_string(name).map_err(|source| io_error("编码目标目录名", Path::new(name), source))?;
    // O_NOFOLLOW 同时约束最终组件；祖先目录由 parent dirfd 的 inode 固定。
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            encoded.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )
    };
    if descriptor < 0 {
        return Err(io_error(
            "安全打开目标目录",
            Path::new(name),
            std::io::Error::last_os_error(),
        ));
    }
    // descriptor 已由本函数独占，交给 File 后只会关闭一次。
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn create_regular_file_at(parent: &File, name: &OsStr) -> std::io::Result<File> {
    let encoded = c_string(name)?;
    // O_EXCL 与 dirfd 共同保证只创建本次目标中的新文件，不覆盖或跟随已有条目。
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            encoded.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600 as libc::c_uint,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // O_EXCL 已保证这是本次创建的新普通文件；File 接管唯一 descriptor 所有权。
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn unlink_at(parent: &File, name: &OsStr, directory: bool) -> std::io::Result<()> {
    let encoded = c_string(name)?;
    let flags = if directory { libc::AT_REMOVEDIR } else { 0 };
    // unlinkat 只删除 dirfd 下的最终组件，遇到 symlink 时删除链接本身而非其目标。
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), encoded.as_ptr(), flags) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn c_string(name: &OsStr) -> std::io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "文件名不能包含 NUL 字节")
    })
}

fn open_regular_file(path: &Path) -> Result<File, ContentValidationError> {
    OpenOptions::new()
        .read(true)
        // O_NONBLOCK 防止 lstat 后被替换成 FIFO 时卡住；调用方会立即 fstat 并核对普通文件身份。
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|source| io_error("打开普通文件", path, source))
}

fn ensure_unchanged(path: &Path, expected: &EntryIdentity) -> Result<(), ContentValidationError> {
    let actual = symlink_metadata(path, "重新检查 Skill 内容")?;
    if actual.file_type().is_symlink() || EntryIdentity::from_metadata(&actual) != *expected {
        return Err(ContentValidationError::SourceChanged(display_path(path)));
    }
    Ok(())
}

fn ensure_canonical_path(canonical_root: &Path, path: &Path) -> Result<(), ContentValidationError> {
    let canonical =
        fs::canonicalize(path).map_err(|source| io_error("确认 Skill 内容边界", path, source))?;
    if canonical != path || !canonical.starts_with(canonical_root) {
        return Err(ContentValidationError::SourceChanged(display_path(path)));
    }
    Ok(())
}

fn hash_entry_metadata(
    hasher: &mut Sha256,
    relative_path: &Path,
    kind: EntryKind,
    permissions: u32,
    length: u64,
) {
    // 遍历顺序固定且每个字段带长度帧，路径或内容都不能伪装成下一个字段或条目。
    write_frame(hasher, relative_path.as_os_str().as_bytes());
    write_frame(hasher, kind.fingerprint_tag());
    write_frame(hasher, &permissions.to_le_bytes());
    write_frame(hasher, &length.to_le_bytes());
}

fn write_frame(hasher: &mut Sha256, bytes: &[u8]) {
    write_frame_length(hasher, bytes.len() as u64);
    hasher.update(bytes);
}

fn write_frame_length(hasher: &mut Sha256, length: u64) {
    hasher.update(length.to_le_bytes());
}

fn permission_bits(metadata: &Metadata) -> u32 {
    metadata.permissions().mode() & 0o7777
}

fn set_file_permissions(
    file: &File,
    mode: u32,
    display_path: &Path,
) -> Result<(), ContentValidationError> {
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|source| io_error("保留文件权限", display_path, source))
}

fn special_file_kind(file_type: &fs::FileType) -> &'static str {
    if file_type.is_fifo() {
        "FIFO"
    } else if file_type.is_socket() {
        "套接字"
    } else if file_type.is_block_device() {
        "块设备"
    } else if file_type.is_char_device() {
        "字符设备"
    } else {
        "特殊文件"
    }
}

fn symlink_metadata(path: &Path, action: &'static str) -> Result<Metadata, ContentValidationError> {
    fs::symlink_metadata(path).map_err(|source| io_error(action, path, source))
}

fn io_error(action: &'static str, path: &Path, source: std::io::Error) -> ContentValidationError {
    ContentValidationError::Io {
        action,
        path: display_path(path),
        source,
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn validates_and_copies_a_safe_skill_without_executing_it() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let source = sandbox.path().join("example-skill");
        write_valid_skill(&source);
        let script = source.join("script.sh");
        fs::write(&script, "#!/bin/sh\nexit 99\n").expect("应写入不会被执行的脚本");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o740)).expect("应设置脚本权限");
        let payload = source.join("payload.txt");
        fs::write(&payload, "safe payload").expect("应写入普通内容");
        fs::set_permissions(&payload, fs::Permissions::from_mode(0o640))
            .expect("应设置普通文件权限");

        let validated = validate_single_skill_folder(&source).expect("有效 Skill 应通过验证");
        assert_eq!(validated.canonical_root, fs::canonicalize(&source).unwrap());
        assert_eq!(validated.name, "example-skill");
        assert_eq!(validated.description, "测试 Skill");
        assert_eq!(validated.fingerprint.len(), 64);
        assert_eq!(validated.warnings, vec![EXECUTABLE_WARNING]);

        let destination_parent = sandbox.path().join("copy");
        fs::create_dir(&destination_parent).expect("应创建复制父目录");
        let destination = destination_parent.join("example-skill");
        copy_single_skill_tree(&source, &destination).expect("安全 Skill 应完整复制");
        assert_eq!(
            fs::read_to_string(destination.join("payload.txt")).unwrap(),
            "safe payload"
        );
        assert_eq!(
            fs::metadata(destination.join("payload.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        assert_eq!(
            validate_single_skill_folder(&destination)
                .expect("复制结果仍应是有效 Skill")
                .fingerprint,
            validated.fingerprint
        );
    }

    #[test]
    fn rejects_invalid_or_mismatched_metadata() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let cases = [
            (
                "missing-description",
                "---\nname: missing-description\n---\n",
            ),
            (
                "invalid-name",
                "---\nname: Invalid_Name\ndescription: 测试\n---\n",
            ),
            (
                "mismatched-name",
                "---\nname: another-name\ndescription: 测试\n---\n",
            ),
            ("missing-frontmatter", "# 普通 Markdown\n"),
        ];

        for (directory_name, metadata) in cases {
            let root = sandbox.path().join(directory_name);
            fs::create_dir(&root).expect("应创建无效 metadata fixture");
            fs::write(root.join("SKILL.md"), metadata).expect("应写入无效 metadata");
            assert!(matches!(
                validate_single_skill_folder(&root),
                Err(ContentValidationError::InvalidMetadata(_))
            ));
        }
    }

    #[test]
    fn rejects_oversized_raw_description_and_non_line_frontmatter_delimiter() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let oversized = sandbox.path().join("oversized-description");
        fs::create_dir(&oversized).expect("应创建超长 description fixture");
        fs::write(
            oversized.join("SKILL.md"),
            format!(
                "---\nname: oversized-description\ndescription: '{}x'\n---\n",
                " ".repeat(1024)
            ),
        )
        .expect("应写入原始长度超限的 metadata");
        assert!(matches!(
            validate_single_skill_folder(&oversized),
            Err(ContentValidationError::InvalidMetadata(_))
        ));

        let bad_delimiter = sandbox.path().join("bad-delimiter");
        fs::create_dir(&bad_delimiter).expect("应创建非法分隔符 fixture");
        fs::write(
            bad_delimiter.join("SKILL.md"),
            "---\nname: bad-delimiter\ndescription: 测试\n---garbage\n",
        )
        .expect("应写入非法结束分隔符");
        assert!(matches!(
            validate_single_skill_folder(&bad_delimiter),
            Err(ContentValidationError::InvalidMetadata(_))
        ));
    }

    #[test]
    fn rejects_root_and_child_symlinks() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let source = sandbox.path().join("example-skill");
        write_valid_skill(&source);
        let linked_root = sandbox.path().join("linked-skill");
        symlink(&source, &linked_root).expect("应创建根目录软链接");
        assert!(matches!(
            validate_single_skill_folder(&linked_root),
            Err(ContentValidationError::RootSymlink(_))
        ));

        symlink("SKILL.md", source.join("linked.md")).expect("应创建内容软链接");
        assert!(matches!(
            validate_single_skill_folder(&source),
            Err(ContentValidationError::UnsafeEntry {
                kind: "软链接", ..
            })
        ));
    }

    #[test]
    fn rejects_hard_linked_regular_files() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let source = sandbox.path().join("example-skill");
        write_valid_skill(&source);
        let payload = source.join("payload.txt");
        fs::write(&payload, "shared inode").expect("应写入普通文件");
        fs::hard_link(&payload, source.join("alias.txt")).expect("应创建硬链接");

        assert!(matches!(
            validate_single_skill_folder(&source),
            Err(ContentValidationError::HardLinkedFile { .. })
        ));
    }

    #[test]
    fn nested_skill_is_reported_as_an_unsupported_single_skill_input() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let source = sandbox.path().join("example-skill");
        write_valid_skill(&source);
        symlink("SKILL.md", source.join("a-link")).expect("应创建排序更早的不安全条目");
        let nested = source.join("z-nested");
        fs::create_dir(&nested).expect("应创建嵌套目录");
        fs::write(
            nested.join("SKILL.md"),
            "---\nname: nested\ndescription: 嵌套 Skill\n---\n",
        )
        .expect("应写入嵌套 metadata");

        assert!(matches!(
            validate_single_skill_folder(&source),
            Err(ContentValidationError::NestedSkillUnsupported(_))
        ));
    }

    #[test]
    fn rejects_a_unix_socket_as_special_content() {
        use std::os::unix::net::UnixListener;

        let sandbox = tempdir().expect("应创建隔离测试目录");
        let source = sandbox.path().join("example-skill");
        write_valid_skill(&source);
        let socket_path = source.join("agent.socket");
        let _listener = UnixListener::bind(&socket_path).expect("应创建 socket fixture");

        assert!(matches!(
            validate_single_skill_folder(&source),
            Err(ContentValidationError::UnsafeEntry {
                kind: "套接字", ..
            })
        ));
    }

    #[test]
    fn enforces_entry_single_file_and_total_size_limits() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let source = sandbox.path().join("example-skill");
        write_valid_skill(&source);
        fs::write(source.join("payload.txt"), b"1234").expect("应写入大小限制 fixture");
        let metadata_length = fs::metadata(source.join("SKILL.md")).unwrap().len();

        let entry_error = validate_single_skill_folder_with_limits(
            &source,
            ContentLimits {
                max_entries: 1,
                max_total_file_bytes: u64::MAX,
                max_single_file_bytes: u64::MAX,
            },
        )
        .expect_err("超过条目限制应失败");
        assert!(matches!(
            entry_error,
            ContentValidationError::EntryLimitExceeded { .. }
        ));

        let single_file_error = validate_single_skill_folder_with_limits(
            &source,
            ContentLimits {
                max_entries: usize::MAX,
                max_total_file_bytes: u64::MAX,
                max_single_file_bytes: 3,
            },
        )
        .expect_err("超过单文件限制应失败");
        assert!(matches!(
            single_file_error,
            ContentValidationError::FileSizeLimitExceeded { .. }
        ));

        let total_error = validate_single_skill_folder_with_limits(
            &source,
            ContentLimits {
                max_entries: usize::MAX,
                max_total_file_bytes: metadata_length + 3,
                max_single_file_bytes: u64::MAX,
            },
        )
        .expect_err("超过普通文件总量限制应失败");
        assert!(matches!(
            total_error,
            ContentValidationError::TotalSizeLimitExceeded { .. }
        ));
    }

    #[test]
    fn refuses_to_overwrite_an_existing_copy_destination() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let source = sandbox.path().join("example-skill");
        write_valid_skill(&source);
        let destination = sandbox.path().join("existing");
        fs::create_dir(&destination).expect("应创建已存在目标");

        assert!(matches!(
            copy_single_skill_tree(&source, &destination),
            Err(ContentValidationError::DestinationExists(_))
        ));
    }

    #[test]
    fn parent_symlink_swap_cannot_redirect_destination_writes() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let source = sandbox.path().join("example-skill");
        write_valid_skill(&source);
        let parent = sandbox.path().join("staging");
        let moved_parent = sandbox.path().join("staging-original");
        let attacker = sandbox.path().join("outside");
        fs::create_dir(&parent).expect("应创建原目标父目录");
        fs::create_dir(&attacker).expect("应创建越界目标目录");
        let destination = parent.join("example-skill");

        let error = copy_single_skill_tree_with_hooks(
            &source,
            &destination,
            || {
                fs::rename(&parent, &moved_parent).expect("应替换目标父目录");
                symlink(&attacker, &parent).expect("应把可见父路径替换成软链接");
            },
            || {},
        )
        .expect_err("父路径变化必须使复制安全失败");

        assert!(matches!(
            error,
            ContentValidationError::Io { .. } | ContentValidationError::CopyVerificationFailed(_)
        ));
        assert!(!attacker.join("example-skill").exists());
        assert!(!moved_parent.join("example-skill").exists());
    }

    #[test]
    fn source_change_after_copy_removes_the_partial_destination() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let source = sandbox.path().join("example-skill");
        write_valid_skill(&source);
        let payload = source.join("payload.txt");
        fs::write(&payload, "before").expect("应写入复制前内容");
        let destination_parent = sandbox.path().join("copy");
        fs::create_dir(&destination_parent).expect("应创建复制父目录");
        let destination = destination_parent.join("example-skill");

        let error = copy_single_skill_tree_with_hooks(
            &source,
            &destination,
            || {},
            || fs::write(&payload, "after").expect("应模拟复制后来源变化"),
        )
        .expect_err("来源变化必须使复制失败");

        assert!(matches!(error, ContentValidationError::SourceChanged(_)));
        assert!(!destination.exists());
    }

    fn write_valid_skill(root: &Path) {
        fs::create_dir(root).expect("应创建 Skill 根目录");
        let name = root.file_name().unwrap().to_string_lossy();
        fs::write(
            root.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: 测试 Skill\n---\n# {name}\n"),
        )
        .expect("应写入有效 SKILL.md");
    }
}
