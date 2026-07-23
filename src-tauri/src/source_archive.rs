//! ZIP 类 Source 的唯一安全预检与展开实现。

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions, Permissions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use thiserror::Error;
use zip::{CompressionMethod, ZipArchive};

use crate::content::{MAX_ENTRIES, MAX_SINGLE_FILE_BYTES, MAX_TOTAL_FILE_BYTES};

const STREAM_BUFFER_BYTES: usize = 8 * 1024;

/// GitHub archive 必须带共同目录；用户提供的 ZIP 则允许内容直接位于根部。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveWrapperPolicy {
    RequiredCommonWrapper,
    OptionalCommonWrapper,
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub(crate) enum SourceArchiveError {
    #[error("无效 ZIP archive")]
    InvalidArchive,
    #[error("ZIP archive 缺少要求的共同顶层目录")]
    InvalidArchiveRoot,
    #[error("ZIP archive 包含不安全路径：{path}")]
    UnsafeArchivePath { path: String },
    #[error("ZIP archive 包含重复规范化路径：{path}")]
    DuplicateArchivePath { path: String },
    #[error("ZIP archive 包含加密条目：{path}")]
    EncryptedArchiveEntry { path: String },
    #[error("ZIP archive 包含不支持的特殊条目：{path}")]
    UnsupportedArchiveEntry { path: String },
    #[error("ZIP archive 包含不支持的压缩格式：{path}")]
    UnsupportedArchiveCompression { path: String },
    #[error("ZIP archive 条目数超过固定上限 {limit}：已检测到 {actual}")]
    ArchiveEntryLimitExceeded { limit: usize, actual: usize },
    #[error("ZIP archive 普通文件总量超过固定上限 {limit} bytes：已检测到 {actual} bytes")]
    ArchiveTotalSizeLimitExceeded { limit: u64, actual: u64 },
    #[error("ZIP archive 普通文件超过固定单文件上限 {limit} bytes：{path} 为 {actual} bytes")]
    ArchiveFileSizeLimitExceeded {
        path: String,
        limit: u64,
        actual: u64,
    },
    #[error("ZIP archive 文件不可用")]
    ArchiveUnavailable,
    #[error("ZIP archive 展开目标不可用")]
    DestinationUnavailable,
    #[error("ZIP archive 展开失败且无法清理临时内容")]
    CleanupFailed,
}

#[derive(Clone, Copy, Debug)]
struct ArchiveLimits {
    max_entries: usize,
    max_total_file_bytes: u64,
    max_single_file_bytes: u64,
}

impl ArchiveLimits {
    const PRODUCTION: Self = Self {
        max_entries: MAX_ENTRIES,
        max_total_file_bytes: MAX_TOTAL_FILE_BYTES,
        max_single_file_bytes: MAX_SINGLE_FILE_BYTES,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArchiveEntryKind {
    Directory,
    File,
}

#[derive(Debug)]
struct ArchiveEntryPlan {
    index: usize,
    relative_path: PathBuf,
    kind: ArchiveEntryKind,
    declared_size: u64,
    permissions: u32,
}

#[derive(Debug)]
struct CentralDirectoryPlan {
    entry_count: usize,
    stripped_wrapper: Option<String>,
}

#[derive(Debug)]
struct ParsedCentralEntry {
    components: Vec<String>,
    name_is_directory: bool,
}

/// 将 ZIP 安全展开到一个尚不存在的目标目录；失败时不会留下部分内容。
pub(crate) fn extract_zip_archive(
    archive_path: &Path,
    destination_root: &Path,
    wrapper_policy: ArchiveWrapperPolicy,
) -> Result<(), SourceArchiveError> {
    extract_zip_archive_with_limits(
        archive_path,
        destination_root,
        wrapper_policy,
        ArchiveLimits::PRODUCTION,
    )
}

fn extract_zip_archive_with_limits(
    archive_path: &Path,
    destination_root: &Path,
    wrapper_policy: ArchiveWrapperPolicy,
    limits: ArchiveLimits,
) -> Result<(), SourceArchiveError> {
    let archive_file =
        File::open(archive_path).map_err(|_| SourceArchiveError::ArchiveUnavailable)?;
    let mut archive =
        ZipArchive::new(archive_file).map_err(|_| SourceArchiveError::InvalidArchive)?;
    let mut central_directory_file =
        File::open(archive_path).map_err(|_| SourceArchiveError::ArchiveUnavailable)?;
    let plan = preflight_archive(
        &mut archive,
        &mut central_directory_file,
        wrapper_policy,
        limits,
    )?;

    fs::create_dir(destination_root).map_err(|_| SourceArchiveError::DestinationUnavailable)?;
    if let Err(error) = write_archive(&mut archive, &plan, destination_root, limits) {
        return match fs::remove_dir_all(destination_root) {
            Ok(()) => Err(error),
            Err(_) => Err(SourceArchiveError::CleanupFailed),
        };
    }
    Ok(())
}

fn preflight_archive(
    archive: &mut ZipArchive<File>,
    central_directory_file: &mut File,
    wrapper_policy: ArchiveWrapperPolicy,
    limits: ArchiveLimits,
) -> Result<Vec<ArchiveEntryPlan>, SourceArchiveError> {
    let central = preflight_central_directory(
        central_directory_file,
        archive.central_directory_start(),
        wrapper_policy,
        limits.max_entries,
    )?;
    if archive.len() != central.entry_count {
        // zip crate 会折叠完全同名的 central entry；数量不一致只能来自重复项。
        return Err(SourceArchiveError::DuplicateArchivePath {
            path: "<duplicate-entry>".to_owned(),
        });
    }

    let mut declared_total = 0_u64;
    let mut kinds = BTreeMap::<PathBuf, ArchiveEntryKind>::new();
    let mut plan = Vec::with_capacity(central.entry_count);
    for index in 0..central.entry_count {
        // raw 模式不会开始解压，因此所有路径、类型和声明大小都会在首次写出前检查。
        let entry = archive
            .by_index_raw(index)
            .map_err(|_| SourceArchiveError::InvalidArchive)?;
        let entry_display = archive_entry_display(entry.name_raw());
        if entry.encrypted() {
            return Err(SourceArchiveError::EncryptedArchiveEntry {
                path: entry_display,
            });
        }
        if !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(SourceArchiveError::UnsupportedArchiveCompression {
                path: entry_display,
            });
        }
        let (components, name_is_directory) = strict_archive_components(entry.name_raw())?;
        let relative_path =
            relative_archive_path(&components, central.stripped_wrapper.as_deref())?;
        let kind = archive_entry_kind(entry.unix_mode(), name_is_directory, &entry_display)?;
        if relative_path.as_os_str().is_empty() && kind != ArchiveEntryKind::Directory {
            return Err(SourceArchiveError::InvalidArchiveRoot);
        }
        if kind == ArchiveEntryKind::Directory && entry.size() != 0 {
            return Err(SourceArchiveError::InvalidArchive);
        }
        if kind == ArchiveEntryKind::File {
            if entry.size() > limits.max_single_file_bytes {
                return Err(SourceArchiveError::ArchiveFileSizeLimitExceeded {
                    path: entry_display,
                    limit: limits.max_single_file_bytes,
                    actual: entry.size(),
                });
            }
            declared_total = declared_total.checked_add(entry.size()).ok_or(
                SourceArchiveError::ArchiveTotalSizeLimitExceeded {
                    limit: limits.max_total_file_bytes,
                    actual: u64::MAX,
                },
            )?;
            if declared_total > limits.max_total_file_bytes {
                return Err(SourceArchiveError::ArchiveTotalSizeLimitExceeded {
                    limit: limits.max_total_file_bytes,
                    actual: declared_total,
                });
            }
        }

        if kinds.insert(relative_path.clone(), kind).is_some() {
            return Err(SourceArchiveError::DuplicateArchivePath {
                path: archive_relative_display(&relative_path),
            });
        }
        plan.push(ArchiveEntryPlan {
            index,
            relative_path,
            kind,
            declared_size: entry.size(),
            permissions: archive_permissions(entry.unix_mode(), kind),
        });
    }

    // 文件不能同时充当后续条目的父目录；这类冲突也必须在创建目录前拒绝。
    for (path, kind) in &kinds {
        let mut ancestor = path.parent();
        while let Some(candidate) = ancestor {
            if kinds.get(candidate) == Some(&ArchiveEntryKind::File) {
                return Err(SourceArchiveError::DuplicateArchivePath {
                    path: archive_relative_display(path),
                });
            }
            if candidate.as_os_str().is_empty() {
                break;
            }
            ancestor = candidate.parent();
        }
        if path.as_os_str().is_empty() && *kind != ArchiveEntryKind::Directory {
            return Err(SourceArchiveError::InvalidArchiveRoot);
        }
    }
    Ok(plan)
}

/// `zip` 会折叠完全同名的 central entry；直接遍历原始 header 才能可靠计数。
fn preflight_central_directory(
    file: &mut File,
    central_directory_start: u64,
    wrapper_policy: ArchiveWrapperPolicy,
    max_entries: usize,
) -> Result<CentralDirectoryPlan, SourceArchiveError> {
    const CENTRAL_ENTRY_SIGNATURE: u32 = 0x0201_4b50;
    const CENTRAL_ENTRY_FIXED_BYTES_AFTER_SIGNATURE: usize = 42;
    const CENTRAL_END_SIGNATURES: [u32; 4] = [0x0605_4b50, 0x0606_4b50, 0x0706_4b50, 0x0505_4b50];

    file.seek(SeekFrom::Start(central_directory_start))
        .map_err(|_| SourceArchiveError::InvalidArchive)?;
    let mut entries = Vec::<ParsedCentralEntry>::new();
    loop {
        let mut signature_bytes = [0_u8; 4];
        file.read_exact(&mut signature_bytes)
            .map_err(|_| SourceArchiveError::InvalidArchive)?;
        let signature = u32::from_le_bytes(signature_bytes);
        if signature != CENTRAL_ENTRY_SIGNATURE {
            if entries.is_empty() || !CENTRAL_END_SIGNATURES.contains(&signature) {
                return Err(SourceArchiveError::InvalidArchive);
            }
            break;
        }

        let count =
            entries
                .len()
                .checked_add(1)
                .ok_or(SourceArchiveError::ArchiveEntryLimitExceeded {
                    limit: max_entries,
                    actual: usize::MAX,
                })?;
        if count > max_entries {
            return Err(SourceArchiveError::ArchiveEntryLimitExceeded {
                limit: max_entries,
                actual: count,
            });
        }
        let mut fixed = [0_u8; CENTRAL_ENTRY_FIXED_BYTES_AFTER_SIGNATURE];
        file.read_exact(&mut fixed)
            .map_err(|_| SourceArchiveError::InvalidArchive)?;
        let name_length = u16::from_le_bytes([fixed[24], fixed[25]]) as usize;
        let extra_length = u16::from_le_bytes([fixed[26], fixed[27]]) as u64;
        let comment_length = u16::from_le_bytes([fixed[28], fixed[29]]) as u64;
        let mut raw_name = vec![0_u8; name_length];
        file.read_exact(&mut raw_name)
            .map_err(|_| SourceArchiveError::InvalidArchive)?;
        file.seek(SeekFrom::Current(
            extra_length.saturating_add(comment_length) as i64,
        ))
        .map_err(|_| SourceArchiveError::InvalidArchive)?;
        let (components, name_is_directory) = strict_archive_components(&raw_name)?;
        entries.push(ParsedCentralEntry {
            components,
            name_is_directory,
        });
    }

    let stripped_wrapper = common_wrapper(&entries, wrapper_policy)?;
    let mut normalized_paths = BTreeMap::<PathBuf, ()>::new();
    for entry in &entries {
        let relative_path = relative_archive_path(&entry.components, stripped_wrapper.as_deref())?;
        if normalized_paths.insert(relative_path.clone(), ()).is_some() {
            return Err(SourceArchiveError::DuplicateArchivePath {
                path: archive_relative_display(&relative_path),
            });
        }
    }
    Ok(CentralDirectoryPlan {
        entry_count: entries.len(),
        stripped_wrapper,
    })
}

fn common_wrapper(
    entries: &[ParsedCentralEntry],
    wrapper_policy: ArchiveWrapperPolicy,
) -> Result<Option<String>, SourceArchiveError> {
    let first = entries
        .first()
        .and_then(|entry| entry.components.first())
        .ok_or(SourceArchiveError::InvalidArchiveRoot)?;
    let all_share_first = entries
        .iter()
        .all(|entry| entry.components.first() == Some(first));
    let all_live_below_or_are_directory = entries
        .iter()
        .all(|entry| entry.components.len() > 1 || entry.name_is_directory);
    match wrapper_policy {
        ArchiveWrapperPolicy::RequiredCommonWrapper
            if all_share_first && all_live_below_or_are_directory =>
        {
            Ok(Some(first.clone()))
        }
        ArchiveWrapperPolicy::RequiredCommonWrapper => Err(SourceArchiveError::InvalidArchiveRoot),
        ArchiveWrapperPolicy::OptionalCommonWrapper
            if all_share_first && all_live_below_or_are_directory =>
        {
            Ok(Some(first.clone()))
        }
        ArchiveWrapperPolicy::OptionalCommonWrapper => Ok(None),
    }
}

fn relative_archive_path(
    components: &[String],
    stripped_wrapper: Option<&str>,
) -> Result<PathBuf, SourceArchiveError> {
    let relative_components = match stripped_wrapper {
        Some(wrapper) if components.first().is_some_and(|first| first == wrapper) => {
            &components[1..]
        }
        Some(_) => return Err(SourceArchiveError::InvalidArchiveRoot),
        None => components,
    };
    Ok(relative_components.iter().collect())
}

fn strict_archive_components(raw_name: &[u8]) -> Result<(Vec<String>, bool), SourceArchiveError> {
    let display = archive_entry_display(raw_name);
    if raw_name.is_empty()
        || raw_name.contains(&b'\0')
        || raw_name.contains(&b'\\')
        || raw_name.first() == Some(&b'/')
    {
        return Err(SourceArchiveError::UnsafeArchivePath { path: display });
    }
    let name = std::str::from_utf8(raw_name)
        .map_err(|_| SourceArchiveError::UnsafeArchivePath { path: display })?;
    if name.chars().any(char::is_control) {
        return Err(SourceArchiveError::UnsafeArchivePath {
            path: archive_entry_display(raw_name),
        });
    }
    let name_is_directory = name.ends_with('/');
    let normalized_name = name.strip_suffix('/').unwrap_or(name);
    let components = normalized_name.split('/').collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| component.is_empty() || matches!(*component, "." | ".."))
        || components[0].as_bytes().get(1) == Some(&b':')
    {
        return Err(SourceArchiveError::UnsafeArchivePath {
            path: archive_entry_display(raw_name),
        });
    }
    Ok((
        components.into_iter().map(str::to_owned).collect(),
        name_is_directory,
    ))
}

fn archive_entry_kind(
    unix_mode: Option<u32>,
    name_is_directory: bool,
    display: &str,
) -> Result<ArchiveEntryKind, SourceArchiveError> {
    let declared_type = unix_mode.unwrap_or(0) & u32::from(libc::S_IFMT);
    if declared_type == u32::from(libc::S_IFLNK) {
        return Err(SourceArchiveError::UnsupportedArchiveEntry {
            path: display.to_owned(),
        });
    }
    let expected_type = if name_is_directory {
        u32::from(libc::S_IFDIR)
    } else {
        u32::from(libc::S_IFREG)
    };
    if declared_type != 0 && declared_type != expected_type {
        return Err(SourceArchiveError::UnsupportedArchiveEntry {
            path: display.to_owned(),
        });
    }
    Ok(if name_is_directory {
        ArchiveEntryKind::Directory
    } else {
        ArchiveEntryKind::File
    })
}

fn archive_permissions(unix_mode: Option<u32>, kind: ArchiveEntryKind) -> u32 {
    unix_mode.map(|mode| mode & 0o777).unwrap_or(match kind {
        ArchiveEntryKind::Directory => 0o755,
        ArchiveEntryKind::File => 0o644,
    })
}

fn write_archive(
    archive: &mut ZipArchive<File>,
    plan: &[ArchiveEntryPlan],
    destination_root: &Path,
    limits: ArchiveLimits,
) -> Result<(), SourceArchiveError> {
    let mut directory_permissions = Vec::<(PathBuf, u32)>::new();
    let mut actual_total = 0_u64;
    for planned in plan {
        let destination = destination_root.join(&planned.relative_path);
        match planned.kind {
            ArchiveEntryKind::Directory => {
                fs::create_dir_all(&destination)
                    .map_err(|_| SourceArchiveError::DestinationUnavailable)?;
                directory_permissions.push((destination, planned.permissions));
            }
            ArchiveEntryKind::File => {
                let parent = destination
                    .parent()
                    .ok_or(SourceArchiveError::InvalidArchive)?;
                fs::create_dir_all(parent)
                    .map_err(|_| SourceArchiveError::DestinationUnavailable)?;
                let mut destination_file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)
                    .map_err(|error| {
                        if error.kind() == std::io::ErrorKind::AlreadyExists {
                            SourceArchiveError::InvalidArchive
                        } else {
                            SourceArchiveError::DestinationUnavailable
                        }
                    })?;
                let mut source = archive
                    .by_index(planned.index)
                    .map_err(|_| SourceArchiveError::InvalidArchive)?;
                let mut actual_file = 0_u64;
                let mut buffer = [0_u8; STREAM_BUFFER_BYTES];
                loop {
                    let remaining_single = limits.max_single_file_bytes.saturating_sub(actual_file);
                    let remaining_total = limits.max_total_file_bytes.saturating_sub(actual_total);
                    let remaining_declared = planned.declared_size.saturating_sub(actual_file);
                    let remaining = remaining_single
                        .min(remaining_total)
                        .min(remaining_declared);
                    let probe = if remaining == 0 {
                        1
                    } else {
                        remaining.min(STREAM_BUFFER_BYTES as u64) as usize
                    };
                    let count = source
                        .read(&mut buffer[..probe])
                        .map_err(|_| SourceArchiveError::InvalidArchive)?;
                    if count == 0 {
                        break;
                    }
                    let next_file = actual_file.saturating_add(count as u64);
                    let next_total = actual_total.saturating_add(count as u64);
                    if next_file > limits.max_single_file_bytes {
                        return Err(SourceArchiveError::ArchiveFileSizeLimitExceeded {
                            path: archive_relative_display(&planned.relative_path),
                            limit: limits.max_single_file_bytes,
                            actual: next_file,
                        });
                    }
                    if next_total > limits.max_total_file_bytes {
                        return Err(SourceArchiveError::ArchiveTotalSizeLimitExceeded {
                            limit: limits.max_total_file_bytes,
                            actual: next_total,
                        });
                    }
                    if next_file > planned.declared_size {
                        return Err(SourceArchiveError::InvalidArchive);
                    }
                    destination_file
                        .write_all(&buffer[..count])
                        .map_err(|_| SourceArchiveError::DestinationUnavailable)?;
                    actual_file = next_file;
                    actual_total = next_total;
                }
                if actual_file != planned.declared_size {
                    return Err(SourceArchiveError::InvalidArchive);
                }
                destination_file
                    .flush()
                    .map_err(|_| SourceArchiveError::DestinationUnavailable)?;
                fs::set_permissions(&destination, Permissions::from_mode(planned.permissions))
                    .map_err(|_| SourceArchiveError::DestinationUnavailable)?;
            }
        }
    }

    // 最后从深到浅恢复目录权限，避免只读父目录阻断仍在进行的安全展开。
    directory_permissions.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (directory, permissions) in directory_permissions {
        fs::set_permissions(directory, Permissions::from_mode(permissions))
            .map_err(|_| SourceArchiveError::DestinationUnavailable)?;
    }
    Ok(())
}

fn archive_entry_display(raw_name: &[u8]) -> String {
    match std::str::from_utf8(raw_name) {
        Ok(name) => format!("{name:?}"),
        Err(_) => format!("{raw_name:?}"),
    }
}

fn archive_relative_display(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        "<archive-root>".to_owned()
    } else {
        path.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Cursor, Write},
    };

    use tempfile::{NamedTempFile, tempdir};
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use super::*;
    use crate::content::validate_skill_bundle_folder;

    #[test]
    fn optional_wrapper_strips_one_common_directory() {
        let sandbox = tempdir().expect("应创建隔离目录");
        let archive_path = sandbox.path().join("bundle.zip");
        fs::write(
            &archive_path,
            zip_fixture(&[
                (
                    "download-name/SKILL.md",
                    "---\nname: root\ndescription: root skill\n---\n",
                ),
                ("download-name/scripts/run.sh", "#!/bin/sh\n"),
            ]),
        )
        .expect("应写入 ZIP fixture");
        let destination = sandbox.path().join("root");

        extract_zip_archive(
            &archive_path,
            &destination,
            ArchiveWrapperPolicy::OptionalCommonWrapper,
        )
        .expect("唯一共同 wrapper 应被剥离");

        assert!(destination.join("SKILL.md").is_file());
        assert!(destination.join("scripts/run.sh").is_file());
        assert!(!destination.join("download-name").exists());
        let validated =
            validate_skill_bundle_folder(&destination).expect("剥离 wrapper 后应能验证根 Skill");
        assert_eq!(validated.candidates.len(), 1);
        assert!(validated.candidates[0].selectable());
    }

    #[test]
    fn optional_wrapper_preserves_and_validates_root_skill() {
        let sandbox = tempdir().expect("应创建隔离目录");
        let archive_path = sandbox.path().join("bundle.skill");
        fs::write(
            &archive_path,
            zip_fixture(&[
                (
                    "SKILL.md",
                    "---\nname: root\ndescription: root skill\n---\n",
                ),
                ("README.md", "# root bundle\n"),
            ]),
        )
        .expect("应写入 `.skill` ZIP fixture");
        let destination = sandbox.path().join("root");

        extract_zip_archive(
            &archive_path,
            &destination,
            ArchiveWrapperPolicy::OptionalCommonWrapper,
        )
        .expect("根文件应原样保留");

        assert!(destination.join("SKILL.md").is_file());
        assert!(destination.join("README.md").is_file());
        let validated = validate_skill_bundle_folder(&destination).expect("根级 SKILL.md 应能验证");
        assert_eq!(validated.candidates.len(), 1);
        assert!(validated.candidates[0].selectable());
    }

    #[test]
    fn optional_wrapper_preserves_and_validates_multiple_top_level_skills() {
        let sandbox = tempdir().expect("应创建隔离目录");
        let archive_path = sandbox.path().join("bundle.zip");
        fs::write(
            &archive_path,
            zip_fixture(&[
                (
                    "alpha/SKILL.md",
                    "---\nname: alpha\ndescription: alpha skill\n---\n",
                ),
                (
                    "beta/SKILL.md",
                    "---\nname: beta\ndescription: beta skill\n---\n",
                ),
            ]),
        )
        .expect("应写入多成员 ZIP fixture");
        let destination = sandbox.path().join("expanded");

        extract_zip_archive(
            &archive_path,
            &destination,
            ArchiveWrapperPolicy::OptionalCommonWrapper,
        )
        .expect("多个顶层 Skill 应保持各自目录");

        let validated =
            validate_skill_bundle_folder(&destination).expect("多个顶层 Skill 应能统一验证");
        assert_eq!(validated.candidates.len(), 2);
        assert!(
            validated
                .candidates
                .iter()
                .all(|candidate| candidate.selectable())
        );
    }

    #[test]
    fn required_wrapper_rejects_multiple_top_levels_without_writing() {
        let sandbox = tempdir().expect("应创建隔离目录");
        let archive_path = sandbox.path().join("github.zip");
        fs::write(
            &archive_path,
            zip_fixture(&[("one/SKILL.md", "one"), ("two/SKILL.md", "two")]),
        )
        .expect("应写入 ZIP fixture");
        let destination = sandbox.path().join("expanded");

        assert_eq!(
            extract_zip_archive(
                &archive_path,
                &destination,
                ArchiveWrapperPolicy::RequiredCommonWrapper,
            ),
            Err(SourceArchiveError::InvalidArchiveRoot)
        );
        assert!(!destination.exists());
    }

    #[test]
    fn extraction_failure_removes_partial_destination() {
        let sandbox = tempdir().expect("应创建隔离目录");
        let archive_path = sandbox.path().join("corrupted.zip");
        let mut archive_bytes = stored_zip_fixture("wrapper/file.txt", "contents");
        let name_length = u16::from_le_bytes([archive_bytes[26], archive_bytes[27]]) as usize;
        let extra_length = u16::from_le_bytes([archive_bytes[28], archive_bytes[29]]) as usize;
        let content_start = 30 + name_length + extra_length;
        archive_bytes[content_start] ^= 0xff;
        fs::write(&archive_path, archive_bytes).expect("应写入损坏 ZIP fixture");
        let destination = sandbox.path().join("expanded");

        assert_eq!(
            extract_zip_archive(
                &archive_path,
                &destination,
                ArchiveWrapperPolicy::RequiredCommonWrapper,
            ),
            Err(SourceArchiveError::InvalidArchive)
        );
        assert!(!destination.exists(), "失败展开不能留下部分内容");
    }

    #[test]
    fn strict_archive_paths_reject_every_forbidden_component_form() {
        for path in [
            b"/root/file".as_slice(),
            b"C:/root/file".as_slice(),
            b"root/../file".as_slice(),
            b"root/./file".as_slice(),
            b"root\\file".as_slice(),
            b"root/\0file".as_slice(),
        ] {
            assert!(matches!(
                strict_archive_components(path),
                Err(SourceArchiveError::UnsafeArchivePath { .. })
            ));
        }
    }

    #[test]
    fn preflight_rejects_normalized_duplicates_encryption_and_special_types() {
        let duplicate = normalized_duplicate_zip_fixture();
        let (mut archive, mut central_directory) = open_fixture_archive(&duplicate);
        assert!(matches!(
            preflight_archive(
                &mut archive,
                &mut central_directory,
                ArchiveWrapperPolicy::RequiredCommonWrapper,
                test_archive_limits(3, 10, 10),
            ),
            Err(SourceArchiveError::DuplicateArchivePath { .. })
        ));

        let encrypted = archive_with_central_mode_or_flag(Some(0x0001), None);
        let (mut archive, mut central_directory) = open_fixture_archive(&encrypted);
        assert!(matches!(
            preflight_archive(
                &mut archive,
                &mut central_directory,
                ArchiveWrapperPolicy::RequiredCommonWrapper,
                test_archive_limits(1, 10, 10),
            ),
            Err(SourceArchiveError::EncryptedArchiveEntry { .. })
        ));

        for special_type in [libc::S_IFLNK, libc::S_IFIFO] {
            let special =
                archive_with_central_mode_or_flag(None, Some(u32::from(special_type) | 0o777));
            let (mut archive, mut central_directory) = open_fixture_archive(&special);
            assert!(matches!(
                preflight_archive(
                    &mut archive,
                    &mut central_directory,
                    ArchiveWrapperPolicy::RequiredCommonWrapper,
                    test_archive_limits(1, 10, 10),
                ),
                Err(SourceArchiveError::UnsupportedArchiveEntry { .. })
            ));
        }
    }

    #[test]
    fn archive_entry_limit_accepts_exact_count_and_rejects_the_next_entry() {
        let exact = zip_fixture(&[("root/one.txt", "1"), ("root/two.txt", "2")]);
        let limits = test_archive_limits(2, 10, 10);
        let (mut archive, mut central_directory) = open_fixture_archive(&exact);
        assert_eq!(
            preflight_archive(
                &mut archive,
                &mut central_directory,
                ArchiveWrapperPolicy::RequiredCommonWrapper,
                limits,
            )
            .expect("恰好达到条目上限应成功")
            .len(),
            2
        );

        let over = zip_fixture(&[
            ("root/one.txt", "1"),
            ("root/two.txt", "2"),
            ("root/three.txt", "3"),
        ]);
        let (mut archive, mut central_directory) = open_fixture_archive(&over);
        assert_eq!(
            preflight_archive(
                &mut archive,
                &mut central_directory,
                ArchiveWrapperPolicy::RequiredCommonWrapper,
                limits,
            )
            .expect_err("下一条目必须被拒绝"),
            SourceArchiveError::ArchiveEntryLimitExceeded {
                limit: 2,
                actual: 3,
            }
        );
    }

    #[test]
    fn expanded_total_limit_accepts_exact_bytes_and_rejects_the_next_declared_byte() {
        let exact = zip_fixture(&[("root/one.txt", "123"), ("root/two.txt", "456")]);
        let strict = test_archive_limits(2, 6, 4);
        let sandbox = tempdir().expect("应创建隔离目录");
        let exact_path = sandbox.path().join("exact.zip");
        fs::write(&exact_path, exact).expect("应写入 ZIP fixture");
        extract_zip_archive_with_limits(
            &exact_path,
            &sandbox.path().join("exact"),
            ArchiveWrapperPolicy::RequiredCommonWrapper,
            strict,
        )
        .expect("恰好达到展开总量应成功");

        let over = zip_fixture(&[("root/one.txt", "123"), ("root/two.txt", "4567")]);
        let over_path = sandbox.path().join("over.zip");
        fs::write(&over_path, over).expect("应写入 ZIP fixture");
        assert_eq!(
            extract_zip_archive_with_limits(
                &over_path,
                &sandbox.path().join("over"),
                ArchiveWrapperPolicy::RequiredCommonWrapper,
                strict,
            ),
            Err(SourceArchiveError::ArchiveTotalSizeLimitExceeded {
                limit: 6,
                actual: 7,
            })
        );
        assert!(!sandbox.path().join("over").exists());
    }

    #[test]
    fn expanded_total_limit_rechecks_actual_written_bytes() {
        let archive_bytes = zip_fixture(&[("root/one.txt", "123"), ("root/two.txt", "4567")]);
        let relaxed = test_archive_limits(2, 7, 4);
        let strict = test_archive_limits(2, 6, 4);
        let (mut archive, mut central_directory) = open_fixture_archive(&archive_bytes);
        let plan = preflight_archive(
            &mut archive,
            &mut central_directory,
            ArchiveWrapperPolicy::RequiredCommonWrapper,
            relaxed,
        )
        .expect("宽松预检应建立测试计划");
        let sandbox = tempdir().expect("应创建隔离目录");
        let destination = sandbox.path().join("expanded");
        fs::create_dir(&destination).expect("应创建测试目标");

        assert_eq!(
            write_archive(&mut archive, &plan, &destination, strict)
                .expect_err("实际展开的下一字节必须被拒绝"),
            SourceArchiveError::ArchiveTotalSizeLimitExceeded {
                limit: 6,
                actual: 7,
            }
        );
    }

    #[test]
    fn single_file_limit_accepts_exact_bytes_and_rejects_the_next_declared_byte() {
        let sandbox = tempdir().expect("应创建隔离目录");
        let strict = test_archive_limits(1, 10, 5);
        let exact_path = sandbox.path().join("exact.zip");
        fs::write(&exact_path, zip_fixture(&[("root/file.txt", "12345")]))
            .expect("应写入 ZIP fixture");
        extract_zip_archive_with_limits(
            &exact_path,
            &sandbox.path().join("exact"),
            ArchiveWrapperPolicy::RequiredCommonWrapper,
            strict,
        )
        .expect("恰好达到单文件上限应成功");

        let over_path = sandbox.path().join("over.zip");
        fs::write(&over_path, zip_fixture(&[("root/file.txt", "123456")]))
            .expect("应写入 ZIP fixture");
        assert_eq!(
            extract_zip_archive_with_limits(
                &over_path,
                &sandbox.path().join("over"),
                ArchiveWrapperPolicy::RequiredCommonWrapper,
                strict,
            ),
            Err(SourceArchiveError::ArchiveFileSizeLimitExceeded {
                path: "\"root/file.txt\"".to_owned(),
                limit: 5,
                actual: 6,
            })
        );
        assert!(!sandbox.path().join("over").exists());
    }

    #[test]
    fn single_file_limit_rechecks_actual_written_bytes() {
        let archive_bytes = zip_fixture(&[("root/file.txt", "123456")]);
        let relaxed = test_archive_limits(1, 10, 6);
        let strict = test_archive_limits(1, 10, 5);
        let (mut archive, mut central_directory) = open_fixture_archive(&archive_bytes);
        let plan = preflight_archive(
            &mut archive,
            &mut central_directory,
            ArchiveWrapperPolicy::RequiredCommonWrapper,
            relaxed,
        )
        .expect("宽松预检应建立测试计划");
        let sandbox = tempdir().expect("应创建隔离目录");
        let destination = sandbox.path().join("expanded");
        fs::create_dir(&destination).expect("应创建测试目标");

        assert_eq!(
            write_archive(&mut archive, &plan, &destination, strict)
                .expect_err("实际单文件的下一字节必须被拒绝"),
            SourceArchiveError::ArchiveFileSizeLimitExceeded {
                path: "file.txt".to_owned(),
                limit: 5,
                actual: 6,
            }
        );
    }

    fn zip_fixture(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o644);
        for (path, contents) in entries {
            archive
                .start_file(*path, options)
                .expect("应创建 ZIP entry");
            archive
                .write_all(contents.as_bytes())
                .expect("应写入 ZIP 内容");
        }
        archive.finish().expect("应完成 ZIP fixture").into_inner()
    }

    fn stored_zip_fixture(path: &str, contents: &str) -> Vec<u8> {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        archive
            .start_file(path, options)
            .expect("应创建 stored ZIP entry");
        archive
            .write_all(contents.as_bytes())
            .expect("应写入 stored ZIP 内容");
        archive.finish().expect("应完成 ZIP fixture").into_inner()
    }

    fn normalized_duplicate_zip_fixture() -> Vec<u8> {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().unix_permissions(0o644);
        archive
            .start_file("root/path", options)
            .expect("应创建普通文件 entry");
        archive.write_all(b"x").expect("应写入普通文件");
        archive
            .add_directory("root/path", options)
            .expect("尾随斜杠形成不同原始名称");
        archive.finish().expect("应完成 ZIP fixture").into_inner()
    }

    fn archive_with_central_mode_or_flag(
        encrypted_flag: Option<u16>,
        unix_mode: Option<u32>,
    ) -> Vec<u8> {
        let mut bytes = zip_fixture(&[("root/file.txt", "x")]);
        let central = bytes
            .windows(4)
            .position(|window| window == [0x50, 0x4b, 0x01, 0x02])
            .expect("fixture 应包含 central entry");
        if let Some(flag) = encrypted_flag {
            bytes[6..8].copy_from_slice(&flag.to_le_bytes());
            bytes[central + 8..central + 10].copy_from_slice(&flag.to_le_bytes());
        }
        if let Some(mode) = unix_mode {
            bytes[central + 38..central + 42].copy_from_slice(&(mode << 16).to_le_bytes());
        }
        bytes
    }

    fn open_fixture_archive(bytes: &[u8]) -> (ZipArchive<File>, File) {
        let mut archive_file = NamedTempFile::new().expect("应创建临时 ZIP");
        archive_file.write_all(bytes).expect("应写入临时 ZIP");
        let archive_reader = archive_file.reopen().expect("应重新打开临时 ZIP");
        let central_directory_reader = archive_reader.try_clone().expect("应复制临时 ZIP 句柄");
        (
            ZipArchive::new(archive_reader).expect("fixture 必须是有效 ZIP"),
            central_directory_reader,
        )
    }

    fn test_archive_limits(
        max_entries: usize,
        max_total_file_bytes: u64,
        max_single_file_bytes: u64,
    ) -> ArchiveLimits {
        ArchiveLimits {
            max_entries,
            max_total_file_bytes,
            max_single_file_bytes,
        }
    }
}
