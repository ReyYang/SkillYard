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
const BUNDLE_FINGERPRINT_VERSION: &[u8] = b"skillyard-folder-bundle-v1";
const EXECUTABLE_WARNING: &str = "内容包含脚本或可执行文件，请在挂载前确认风险";
const COMMON_SCRIPT_EXTENSIONS: &[&str] = &[
    "bash", "cjs", "command", "fish", "js", "jsx", "lua", "mjs", "php", "pl", "ps1", "py", "pyw",
    "rb", "sh", "ts", "tsx", "zsh",
];
const COMMON_SCRIPT_DIRECTORIES: &[&str] = &["bin", "hooks", "script", "scripts"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSingleSkill {
    pub canonical_root: PathBuf,
    pub name: String,
    pub description: String,
    pub fingerprint: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredBundleCandidate {
    /// 相对所选 Bundle 根目录的位置；根目录本身是 Skill 时为空路径。
    pub relative_path: PathBuf,
    pub name: Option<String>,
    pub description: Option<String>,
    pub fingerprint: Option<String>,
    pub warnings: Vec<String>,
    pub validation_errors: Vec<String>,
}

impl DiscoveredBundleCandidate {
    pub fn selectable(&self) -> bool {
        self.validation_errors.is_empty()
            && self.name.is_some()
            && self.description.is_some()
            && self.fingerprint.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSkillBundle {
    pub canonical_root: PathBuf,
    pub fingerprint: String,
    pub candidates: Vec<DiscoveredBundleCandidate>,
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
    #[error("所选文件夹中未发现有效的 SKILL.md")]
    NoSkillMetadataFound,
    #[error("Skill 目录不能互相嵌套：{ancestor} 与 {descendant}")]
    NestedSkillConflict {
        ancestor: String,
        descendant: String,
    },
    #[error("Bundle 中存在重复 Skill 名称 {name}：{paths}")]
    DuplicateSkillName { name: String, paths: String },
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

/// 一个 Bundle 内所有已选成员共享复制预算，避免逐成员上限被叠加放大。
#[derive(Debug)]
pub struct BundleCopyBudget {
    limits: ContentLimits,
    used_entries: usize,
    used_file_bytes: u64,
}

impl BundleCopyBudget {
    pub fn production() -> Self {
        Self {
            limits: ContentLimits::PRODUCTION,
            used_entries: 0,
            used_file_bytes: 0,
        }
    }

    fn reserve(&mut self, tree: &ValidatedTree) -> Result<(), ContentValidationError> {
        let entries = tree.entries.len();
        let file_bytes = tree
            .entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .try_fold(0_u64, |total, entry| total.checked_add(entry.length))
            .ok_or(ContentValidationError::TotalSizeLimitExceeded {
                limit: self.limits.max_total_file_bytes,
                actual: u64::MAX,
            })?;
        let next_entries = self.used_entries.checked_add(entries).ok_or(
            ContentValidationError::EntryLimitExceeded {
                limit: self.limits.max_entries,
                actual: usize::MAX,
            },
        )?;
        let next_file_bytes = self.used_file_bytes.checked_add(file_bytes).ok_or(
            ContentValidationError::TotalSizeLimitExceeded {
                limit: self.limits.max_total_file_bytes,
                actual: u64::MAX,
            },
        )?;
        if next_entries > self.limits.max_entries {
            return Err(ContentValidationError::EntryLimitExceeded {
                limit: self.limits.max_entries,
                actual: next_entries,
            });
        }
        if next_file_bytes > self.limits.max_total_file_bytes {
            return Err(ContentValidationError::TotalSizeLimitExceeded {
                limit: self.limits.max_total_file_bytes,
                actual: next_file_bytes,
            });
        }
        self.used_entries = next_entries;
        self.used_file_bytes = next_file_bytes;
        Ok(())
    }
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
struct InspectedDirectoryTree {
    canonical_root: PathBuf,
    root_identity: EntryIdentity,
    root_permissions: u32,
    entries: Vec<TreeEntry>,
    skill_metadata: BTreeMap<PathBuf, Vec<u8>>,
    fingerprint: String,
    has_executable_risk: bool,
}

#[derive(Debug)]
struct BundleDiscovery {
    canonical_root: PathBuf,
    candidate_roots: BTreeMap<PathBuf, ()>,
    skill_metadata: BTreeMap<PathBuf, Vec<u8>>,
    unsafe_entries: Vec<UnsafeDiscoveredEntry>,
    fingerprint: String,
}

#[derive(Debug)]
struct UnsafeDiscoveredEntry {
    relative_path: PathBuf,
    message: String,
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

/// 普通本地文件夹既可以是单 Skill，也可以是包含多个成员的完整 Bundle。
pub fn validate_skill_bundle_folder(
    root: &Path,
) -> Result<ValidatedSkillBundle, ContentValidationError> {
    let inspected = inspect_bundle_discovery(root, ContentLimits::PRODUCTION)?;
    if inspected.candidate_roots.is_empty() {
        return Err(ContentValidationError::NoSkillMetadataFound);
    }

    let mut member_roots = inspected
        .candidate_roots
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    member_roots.sort();
    let mut errors = BTreeMap::<PathBuf, Vec<String>>::new();
    for (index, ancestor) in member_roots.iter().enumerate() {
        for descendant in member_roots
            .iter()
            .skip(index + 1)
            .filter(|candidate| candidate.starts_with(ancestor) && *candidate != ancestor)
        {
            let message = ContentValidationError::NestedSkillConflict {
                ancestor: display_path(&inspected.canonical_root.join(ancestor)),
                descendant: display_path(&inspected.canonical_root.join(descendant)),
            }
            .to_string();
            errors
                .entry(ancestor.clone())
                .or_default()
                .push(message.clone());
            errors.entry(descendant.clone()).or_default().push(message);
        }
    }

    for unsafe_entry in &inspected.unsafe_entries {
        let owner = member_roots
            .iter()
            .filter(|root| unsafe_entry.relative_path.starts_with(root))
            .max_by_key(|root| root.components().count());
        if let Some(owner) = owner {
            errors
                .entry(owner.clone())
                .or_default()
                .push(unsafe_entry.message.clone());
        }
        // Bundle 根下的仓库文件不会进入任何 Member，因此不用它们否定可安装候选。
    }

    let mut candidates = Vec::with_capacity(member_roots.len());
    let mut names = BTreeMap::<String, Vec<usize>>::new();
    for relative_path in member_roots {
        let mut candidate = DiscoveredBundleCandidate {
            relative_path: relative_path.clone(),
            name: None,
            description: None,
            fingerprint: None,
            warnings: Vec::new(),
            validation_errors: errors.remove(&relative_path).unwrap_or_default(),
        };
        let member_root = inspected.canonical_root.join(&relative_path);
        if let Some(metadata) = inspected.skill_metadata.get(&relative_path) {
            let directory_name = member_root
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    ContentValidationError::InvalidMetadata(
                        "Skill 根目录名必须是有效 UTF-8 名称".to_owned(),
                    )
                })?;
            match parse_skill_metadata(metadata, directory_name) {
                Ok((name, description)) => {
                    candidate.name = Some(name);
                    candidate.description = Some(description);
                }
                Err(error) => candidate.validation_errors.push(error.to_string()),
            }
        }
        match validate_single_skill_folder_with_limits(&member_root, ContentLimits::PRODUCTION) {
            Ok(validated) => {
                candidate.fingerprint = Some(validated.fingerprint);
                candidate.warnings = validated.warnings;
            }
            Err(error) => candidate.validation_errors.push(error.to_string()),
        }
        if let Some(name) = &candidate.name {
            names
                .entry(name.clone())
                .or_default()
                .push(candidates.len());
        }
        candidate.validation_errors.sort();
        candidate.validation_errors.dedup();
        candidates.push(candidate);
    }
    for (name, indexes) in names.into_iter().filter(|(_, indexes)| indexes.len() > 1) {
        let paths = indexes
            .iter()
            .map(|index| {
                display_path(
                    &inspected
                        .canonical_root
                        .join(&candidates[*index].relative_path)
                        .join("SKILL.md"),
                )
            })
            .collect::<Vec<_>>()
            .join("、");
        let message = ContentValidationError::DuplicateSkillName { name, paths }.to_string();
        for index in indexes {
            candidates[index].validation_errors.push(message.clone());
        }
    }
    candidates.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

    Ok(ValidatedSkillBundle {
        canonical_root: inspected.canonical_root,
        fingerprint: inspected.fingerprint,
        candidates,
    })
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
    expected_name: &str,
    expected_fingerprint: &str,
    budget: &mut BundleCopyBudget,
) -> Result<(), ContentValidationError> {
    let validated = validate_tree(source, ContentLimits::PRODUCTION)?;
    if validated.skill.name != expected_name || validated.skill.fingerprint != expected_fingerprint
    {
        return Err(ContentValidationError::SourceChanged(display_path(source)));
    }
    // 在创建目标目录前占用共享预算；确认失败不会先把超限内容写入临时区。
    budget.reserve(&validated)?;
    let destination_parent = prepare_open_destination(
        &validated.skill.canonical_root,
        destination_parent,
        destination_parent_path,
        destination_name,
    )?;
    copy_validated_tree(validated, destination_parent, || {}, || {})
}

/// v2 接管把失败现场交给持久化恢复器审计，因此这里绝不主动删除已创建的部分目标。
pub fn copy_single_skill_tree_into_open_directory_preserving_partial(
    source: &Path,
    destination_parent: &File,
    destination_parent_path: &Path,
    destination_name: &OsStr,
    expected_name: &str,
    expected_fingerprint: &str,
    budget: &mut BundleCopyBudget,
) -> Result<(), ContentValidationError> {
    let validated = validate_tree(source, ContentLimits::PRODUCTION)?;
    if validated.skill.name != expected_name || validated.skill.fingerprint != expected_fingerprint
    {
        return Err(ContentValidationError::SourceChanged(display_path(source)));
    }
    budget.reserve(&validated)?;
    let destination_parent = prepare_open_destination(
        &validated.skill.canonical_root,
        destination_parent,
        destination_parent_path,
        destination_name,
    )?;
    copy_validated_tree_with_failure_policy(
        validated,
        destination_parent,
        || {},
        || {},
        FailureCleanup::PreservePartial,
    )
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

#[cfg(test)]
fn copy_single_skill_tree_preserving_partial_with_hooks(
    source: &Path,
    destination: &Path,
    after_entries_copied: impl FnOnce(),
) -> Result<(), ContentValidationError> {
    let validated = validate_tree(source, ContentLimits::PRODUCTION)?;
    let destination_parent = prepare_destination(&validated.skill.canonical_root, destination)?;
    copy_validated_tree_with_failure_policy(
        validated,
        destination_parent,
        || {},
        after_entries_copied,
        FailureCleanup::PreservePartial,
    )
}

fn copy_validated_tree(
    validated: ValidatedTree,
    destination_parent: DestinationParent,
    after_parent_opened: impl FnOnce(),
    after_entries_copied: impl FnOnce(),
) -> Result<(), ContentValidationError> {
    copy_validated_tree_with_failure_policy(
        validated,
        destination_parent,
        after_parent_opened,
        after_entries_copied,
        FailureCleanup::RemoveCreated,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureCleanup {
    RemoveCreated,
    PreservePartial,
}

fn copy_validated_tree_with_failure_policy(
    validated: ValidatedTree,
    destination_parent: DestinationParent,
    after_parent_opened: impl FnOnce(),
    after_entries_copied: impl FnOnce(),
    failure_cleanup: FailureCleanup,
) -> Result<(), ContentValidationError> {
    after_parent_opened();
    let mut created = match failure_cleanup {
        FailureCleanup::RemoveCreated => create_destination_root(&destination_parent)?,
        FailureCleanup::PreservePartial => {
            create_destination_root_preserving_partial(&destination_parent)?
        }
    };

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
        if failure_cleanup == FailureCleanup::PreservePartial {
            return Err(original);
        }
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

fn inspect_bundle_discovery(
    root: &Path,
    limits: ContentLimits,
) -> Result<BundleDiscovery, ContentValidationError> {
    let supplied_metadata = symlink_metadata(root, "检查 Bundle 输入根目录")?;
    if supplied_metadata.file_type().is_symlink() {
        return Err(ContentValidationError::RootSymlink(display_path(root)));
    }
    if !supplied_metadata.is_dir() {
        return Err(ContentValidationError::RootNotDirectory(display_path(root)));
    }
    let canonical_root = fs::canonicalize(root)
        .map_err(|source| io_error("解析 Bundle 输入根目录", root, source))?;
    let root_metadata = symlink_metadata(&canonical_root, "重新检查 Bundle 输入根目录")?;
    let root_identity = EntryIdentity::from_metadata(&root_metadata);
    if root_metadata.file_type().is_symlink()
        || root_identity != EntryIdentity::from_metadata(&supplied_metadata)
    {
        return Err(ContentValidationError::SourceChanged(display_path(root)));
    }

    let mut hasher = Sha256::new();
    write_frame(&mut hasher, BUNDLE_FINGERPRINT_VERSION);
    hash_entry_metadata(
        &mut hasher,
        Path::new(""),
        EntryKind::Directory,
        permission_bits(&root_metadata),
        0,
    );
    write_frame(&mut hasher, &[]);

    let mut stack = vec![(canonical_root.clone(), root_identity.clone())];
    let mut entry_count = 0_usize;
    let mut total_file_bytes = 0_u64;
    let mut candidate_roots = BTreeMap::new();
    let mut skill_metadata = BTreeMap::new();
    let mut unsafe_entries = Vec::new();

    while let Some((directory, expected_directory_identity)) = stack.pop() {
        ensure_canonical_path(&canonical_root, &directory)?;
        ensure_unchanged(&directory, &expected_directory_identity)?;
        let mut children = fs::read_dir(&directory)
            .map_err(|source| io_error("读取 Bundle 目录", &directory, source))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| io_error("读取 Bundle 目录项", &directory, source))?;
        children.sort_by_key(|entry| entry.file_name());

        let mut child_directories = Vec::new();
        for child in children {
            let path = child.path();
            let relative_path = path
                .strip_prefix(&canonical_root)
                .map_err(|_| ContentValidationError::SourceChanged(display_path(&path)))?
                .to_path_buf();
            if relative_path
                .file_name()
                .is_some_and(|name| name == "SKILL.md")
            {
                candidate_roots.insert(
                    relative_path
                        .parent()
                        .unwrap_or_else(|| Path::new(""))
                        .to_path_buf(),
                    (),
                );
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

            let metadata = symlink_metadata(&path, "检查 Bundle 内容")?;
            let file_type = metadata.file_type();
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
                child_directories.push((path, identity));
                continue;
            }

            if file_type.is_file() {
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
                let capture_metadata = relative_path
                    .file_name()
                    .is_some_and(|name| name == "SKILL.md");
                let (captured, _) = hash_regular_file(
                    &path,
                    &identity,
                    metadata.len(),
                    capture_metadata,
                    &mut hasher,
                )?;
                if capture_metadata {
                    let candidate_root = relative_path
                        .parent()
                        .unwrap_or_else(|| Path::new(""))
                        .to_path_buf();
                    skill_metadata.insert(candidate_root, captured);
                }
                if metadata.nlink() > 1 {
                    let error = ContentValidationError::HardLinkedFile {
                        path: display_path(&path),
                        links: metadata.nlink(),
                    };
                    unsafe_entries.push(UnsafeDiscoveredEntry {
                        relative_path,
                        message: error.to_string(),
                    });
                }
                continue;
            }

            let kind = if file_type.is_symlink() {
                "软链接"
            } else {
                special_file_kind(&file_type)
            };
            // 不打开或跟随特殊条目；只把 lstat 身份和链接文本纳入完整发现快照。
            write_frame(&mut hasher, relative_path.as_os_str().as_bytes());
            write_frame(&mut hasher, kind.as_bytes());
            write_frame(&mut hasher, &identity.mode.to_le_bytes());
            write_frame(&mut hasher, &identity.inode.to_le_bytes());
            write_frame(&mut hasher, &identity.changed_seconds.to_le_bytes());
            write_frame(&mut hasher, &identity.changed_nanoseconds.to_le_bytes());
            if file_type.is_symlink() {
                let target = fs::read_link(&path)
                    .map_err(|source| io_error("读取 Bundle 软链接", &path, source))?;
                write_frame(&mut hasher, target.as_os_str().as_bytes());
            } else {
                write_frame(&mut hasher, &[]);
            }
            let error = ContentValidationError::UnsafeEntry {
                path: display_path(&path),
                kind,
            };
            unsafe_entries.push(UnsafeDiscoveredEntry {
                relative_path,
                message: error.to_string(),
            });
        }

        ensure_unchanged(&directory, &expected_directory_identity)?;
        for child_directory in child_directories.into_iter().rev() {
            stack.push(child_directory);
        }
    }

    let root_after = symlink_metadata(&canonical_root, "完成 Bundle 检查")?;
    if EntryIdentity::from_metadata(&root_after) != root_identity {
        return Err(ContentValidationError::SourceChanged(display_path(
            &canonical_root,
        )));
    }
    let digest = hasher.finalize();
    let mut fingerprint = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut fingerprint, "{byte:02x}").expect("写入 String 不会失败");
    }
    Ok(BundleDiscovery {
        canonical_root,
        candidate_roots,
        skill_metadata,
        unsafe_entries,
        fingerprint,
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
    // 单 Skill 输入优先报告成员边界冲突，避免排序更早的其他条目掩盖真实原因。
    reject_nested_skill_metadata(&canonical_root, limits.max_entries)?;
    let inspected = inspect_directory_tree(root, limits, FINGERPRINT_VERSION)?;
    let skill_metadata_path = inspected.canonical_root.join("SKILL.md");
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
    if let Some(nested) = inspected
        .skill_metadata
        .keys()
        .find(|relative| relative.as_path() != Path::new("SKILL.md"))
    {
        return Err(ContentValidationError::NestedSkillUnsupported(
            display_path(&inspected.canonical_root.join(nested)),
        ));
    }
    let skill_metadata_bytes = inspected
        .skill_metadata
        .get(Path::new("SKILL.md"))
        .cloned()
        .ok_or_else(|| {
            ContentValidationError::MissingSkillMetadata(display_path(&skill_metadata_path))
        })?;

    Ok(InspectedTree {
        canonical_root: inspected.canonical_root,
        root_identity: inspected.root_identity,
        root_permissions: inspected.root_permissions,
        entries: inspected.entries,
        skill_metadata_bytes,
        fingerprint: inspected.fingerprint,
        has_executable_risk: inspected.has_executable_risk,
    })
}

fn inspect_directory_tree(
    root: &Path,
    limits: ContentLimits,
    fingerprint_version: &[u8],
) -> Result<InspectedDirectoryTree, ContentValidationError> {
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

    let root_identity = EntryIdentity::from_metadata(&root_metadata);
    let root_permissions = permission_bits(&root_metadata);
    let mut hasher = Sha256::new();
    write_frame(&mut hasher, fingerprint_version);
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
    let mut skill_metadata = BTreeMap::new();
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
            let capture_metadata = relative_path
                .file_name()
                .is_some_and(|name| name == "SKILL.md");
            let (captured, has_shebang) = hash_regular_file(
                &path,
                &identity,
                metadata.len(),
                capture_metadata,
                &mut hasher,
            )?;
            if capture_metadata {
                skill_metadata.insert(relative_path.clone(), captured);
            }
            has_executable_risk |=
                is_script_or_executable_risk(&relative_path, permissions, has_shebang);
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

    let digest = hasher.finalize();
    let mut fingerprint = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut fingerprint, "{byte:02x}").expect("写入 String 不会失败");
    }

    Ok(InspectedDirectoryTree {
        canonical_root,
        root_identity,
        root_permissions,
        entries,
        skill_metadata,
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

fn create_destination_root_preserving_partial(
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

    // 任一步失败都保留已创建目录；只有跨重启的语义审计有权决定它是否可删。
    let root_handle = open_directory_at(&destination.handle, &destination.root_name)?;
    let root_clone = root_handle
        .try_clone()
        .map_err(|source| io_error("保留目标根目录句柄", &destination.root_path, source))?;
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

fn is_script_or_executable_risk(relative_path: &Path, permissions: u32, has_shebang: bool) -> bool {
    if permissions & 0o111 != 0 || has_shebang {
        return true;
    }

    let has_script_extension = relative_path.extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy().to_ascii_lowercase();
        COMMON_SCRIPT_EXTENSIONS.contains(&extension.as_str())
    });
    if has_script_extension {
        return true;
    }

    // 只检查父目录，避免把恰好名为 scripts 的普通文件误当成目录信号。
    relative_path.parent().is_some_and(|parent| {
        parent.components().any(|component| {
            let component = component.as_os_str().to_string_lossy().to_ascii_lowercase();
            COMMON_SCRIPT_DIRECTORIES.contains(&component.as_str())
        })
    })
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
    use std::{
        ffi::CString,
        os::unix::{
            ffi::OsStrExt,
            fs::{PermissionsExt, symlink},
        },
    };

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

    #[test]
    fn v2_copy_failure_preserves_partial_destination_for_persisted_recovery() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let source = sandbox.path().join("example-skill");
        write_valid_skill(&source);
        let payload = source.join("payload.txt");
        fs::write(&payload, "before").expect("应写入复制前内容");
        let destination_parent = sandbox.path().join("copy");
        fs::create_dir(&destination_parent).expect("应创建复制父目录");
        let destination = destination_parent.join("example-skill");

        let error =
            copy_single_skill_tree_preserving_partial_with_hooks(&source, &destination, || {
                fs::write(&payload, "after").expect("应模拟复制后来源变化")
            })
            .expect_err("来源变化必须使 v2 复制失败");

        assert!(matches!(error, ContentValidationError::SourceChanged(_)));
        assert!(destination.exists(), "v2 必须把失败现场留给持久化恢复器");
        assert!(destination.join("SKILL.md").is_file());
    }

    #[test]
    fn duplicate_name_also_invalidates_a_safe_sibling_when_the_other_copy_is_unsafe() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let bundle = sandbox.path().join("bundle");
        let first = bundle.join("one/duplicate");
        let second = bundle.join("two/duplicate");
        fs::create_dir_all(&first).expect("应创建第一个成员");
        fs::create_dir_all(&second).expect("应创建第二个成员");
        let metadata = "---\nname: duplicate\ndescription: 重复成员\n---\n";
        fs::write(first.join("SKILL.md"), metadata).expect("应写入第一个 metadata");
        fs::write(second.join("SKILL.md"), metadata).expect("应写入第二个 metadata");
        symlink("SKILL.md", second.join("unsafe-link")).expect("应创建不安全内容");

        let discovered = validate_skill_bundle_folder(&bundle).expect("应返回可解释的候选列表");

        assert_eq!(discovered.candidates.len(), 2);
        assert!(discovered.candidates.iter().all(|candidate| {
            !candidate.selectable()
                && candidate
                    .validation_errors
                    .iter()
                    .any(|error| error.contains("重复 Skill 名称"))
        }));
        assert!(discovered.candidates.iter().any(|candidate| {
            candidate
                .validation_errors
                .iter()
                .any(|error| error.contains("软链接"))
        }));
    }

    #[test]
    fn unsafe_repository_entries_outside_members_do_not_block_valid_candidates() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let bundle = sandbox.path().join("bundle");
        let member = bundle.join("skills/valid-member");
        write_valid_skill(&member);

        fs::write(bundle.join("README.md"), "repository readme").expect("应写入仓库文件");
        symlink("README.md", bundle.join("README-link.md")).expect("应创建成员外软链接");
        let shared = bundle.join("shared.txt");
        fs::write(&shared, "shared inode").expect("应写入硬链接源文件");
        fs::hard_link(&shared, bundle.join("shared-alias.txt")).expect("应创建成员外硬链接");
        let fifo = bundle.join("events.pipe");
        let encoded = CString::new(fifo.as_os_str().as_bytes()).expect("测试路径不应包含 NUL");
        let result = unsafe { libc::mkfifo(encoded.as_ptr(), 0o600) };
        assert_eq!(result, 0, "应创建成员外 FIFO");

        let discovered = validate_skill_bundle_folder(&bundle).expect("成员外特殊条目不应阻止发现");

        assert_eq!(discovered.candidates.len(), 1);
        assert!(discovered.candidates[0].selectable());
        assert!(discovered.candidates[0].validation_errors.is_empty());
    }

    #[test]
    fn unsafe_entry_only_disables_the_member_that_contains_it() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let bundle = sandbox.path().join("bundle");
        let safe = bundle.join("skills/safe-member");
        let unsafe_member = bundle.join("skills/unsafe-member");
        write_valid_skill(&safe);
        write_valid_skill(&unsafe_member);
        symlink("SKILL.md", unsafe_member.join("linked.md")).expect("应创建成员内软链接");

        let discovered = validate_skill_bundle_folder(&bundle).expect("应保留其他可用成员");
        let safe_candidate = discovered
            .candidates
            .iter()
            .find(|candidate| candidate.name.as_deref() == Some("safe-member"))
            .expect("应发现安全成员");
        let unsafe_candidate = discovered
            .candidates
            .iter()
            .find(|candidate| candidate.name.as_deref() == Some("unsafe-member"))
            .expect("应发现不安全成员");

        assert!(safe_candidate.selectable());
        assert!(!unsafe_candidate.selectable());
        assert!(
            unsafe_candidate
                .validation_errors
                .iter()
                .any(|error| error.contains("软链接"))
        );
    }

    #[test]
    fn warns_for_each_deterministic_script_signal_but_not_plain_content() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let cases = [
            ("by-extension", "helper.py", "print('safe')\n", false),
            ("by-shebang", "helper", "#!/bin/sh\nexit 99\n", false),
            ("by-mode", "helper.txt", "not executed\n", true),
            (
                "by-directory",
                "scripts/helper.txt",
                "not executed\n",
                false,
            ),
        ];

        for (name, relative_path, contents, executable) in cases {
            let root = sandbox.path().join(name);
            write_valid_skill(&root);
            let path = root.join(relative_path);
            fs::create_dir_all(path.parent().expect("测试文件应有父目录")).expect("应创建脚本目录");
            fs::write(&path, contents).expect("应写入脚本风险 fixture");
            if executable {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o740))
                    .expect("应设置可执行位");
            }

            let validated = validate_single_skill_folder(&root).expect("脚本只应产生提示");
            assert_eq!(validated.warnings, vec![EXECUTABLE_WARNING], "{name}");
        }

        let plain = sandbox.path().join("plain-content");
        write_valid_skill(&plain);
        fs::write(plain.join("notes.txt"), "ordinary notes").expect("应写入普通文件");
        let validated = validate_single_skill_folder(&plain).expect("普通内容应通过验证");
        assert!(validated.warnings.is_empty());
    }

    #[test]
    fn bundle_copy_budget_is_shared_across_members() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let first = sandbox.path().join("first");
        let second = sandbox.path().join("second");
        write_valid_skill(&first);
        write_valid_skill(&second);
        let first_tree = validate_tree(&first, ContentLimits::PRODUCTION).expect("成员一应有效");
        let second_tree = validate_tree(&second, ContentLimits::PRODUCTION).expect("成员二应有效");
        let first_bytes = first_tree
            .entries
            .iter()
            .map(|entry| entry.length)
            .sum::<u64>();
        let second_bytes = second_tree
            .entries
            .iter()
            .map(|entry| entry.length)
            .sum::<u64>();
        let mut entry_budget = BundleCopyBudget {
            limits: ContentLimits {
                max_entries: first_tree.entries.len() + second_tree.entries.len() - 1,
                max_total_file_bytes: u64::MAX,
                max_single_file_bytes: u64::MAX,
            },
            used_entries: 0,
            used_file_bytes: 0,
        };
        entry_budget
            .reserve(&first_tree)
            .expect("首个成员应占用预算");
        assert!(matches!(
            entry_budget.reserve(&second_tree),
            Err(ContentValidationError::EntryLimitExceeded { .. })
        ));

        let mut byte_budget = BundleCopyBudget {
            limits: ContentLimits {
                max_entries: usize::MAX,
                max_total_file_bytes: first_bytes + second_bytes - 1,
                max_single_file_bytes: u64::MAX,
            },
            used_entries: 0,
            used_file_bytes: 0,
        };
        byte_budget
            .reserve(&first_tree)
            .expect("首个成员应占用字节预算");
        assert!(matches!(
            byte_budget.reserve(&second_tree),
            Err(ContentValidationError::TotalSizeLimitExceeded { .. })
        ));
    }

    fn write_valid_skill(root: &Path) {
        fs::create_dir_all(root).expect("应创建 Skill 根目录");
        let name = root.file_name().unwrap().to_string_lossy();
        fs::write(
            root.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: 测试 Skill\n---\n# {name}\n"),
        )
        .expect("应写入有效 SKILL.md");
    }
}
