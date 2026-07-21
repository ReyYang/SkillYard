use std::{
    ffi::OsStr,
    fs,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use thiserror::Error;

use crate::domain::{ManagementEvidence, ManagementEvidenceKind};

const GIT_PATH: &str = "/usr/bin/git";

/// Git 证据必须保留三态；命令失败不能被误写成“没有被项目管理”。
pub(crate) enum ManagementEvidenceInspection {
    Confirmed(ManagementEvidence),
    Absent,
    Indeterminate(ManagementEvidenceError),
}

#[derive(Debug, Error)]
pub(crate) enum ManagementEvidenceError {
    #[error("无法检查 Git 管理证据路径 {path}：{source}")]
    InspectPath {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Git 管理证据路径不在已登记 Project 内：{0}")]
    OutsideProject(String),
    #[error("无法执行 Git 管理证据命令 {operation}：{source}")]
    RunGit {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("Git 管理证据命令 {operation} 失败：{message}")]
    GitFailed {
        operation: &'static str,
        message: String,
    },
    #[error("Git 管理证据命令 {operation} 返回了无法验证的结果")]
    InvalidGitOutput { operation: &'static str },
    #[error("Git 管理证据路径不能无损保存为 UTF-8：{0}")]
    NonUtf8Path(String),
    #[error("Git 管理证据只接受普通 .git 目录或文件：{0}")]
    UnsafeGitMarker(String),
}

pub(crate) fn inspect_git_head_management(
    project_root: &Path,
    skill_file: &Path,
) -> ManagementEvidenceInspection {
    match inspect_git_head_management_inner(project_root, skill_file) {
        Ok(inspection) => inspection,
        Err(error) => ManagementEvidenceInspection::Indeterminate(error),
    }
}

fn inspect_git_head_management_inner(
    project_root: &Path,
    skill_file: &Path,
) -> Result<ManagementEvidenceInspection, ManagementEvidenceError> {
    if !is_path_inside_project_without_symlinks(project_root, skill_file)? {
        return Ok(ManagementEvidenceInspection::Absent);
    }

    // 管理权属于已登记 Project 所在的 worktree；Skill 内嵌仓库或 submodule 不能取代它。
    let Some(git_marker) = nearest_git_marker(project_root)? else {
        return Ok(ManagementEvidenceInspection::Absent);
    };
    let working_directory =
        git_marker
            .parent()
            .ok_or(ManagementEvidenceError::InvalidGitOutput {
                operation: "定位 Git worktree",
            })?;

    let top_level = successful_utf8_stdout(
        run_git(
            working_directory,
            &[OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
            "rev-parse --show-toplevel",
        )?,
        "rev-parse --show-toplevel",
    )?;
    let authority_root = fs::canonicalize(Path::new(&top_level)).map_err(|source| {
        ManagementEvidenceError::InspectPath {
            path: top_level.clone(),
            source,
        }
    })?;
    let subject = skill_file
        .strip_prefix(&authority_root)
        .map_err(|_| ManagementEvidenceError::OutsideProject(skill_file.display().to_string()))?;
    let subject_path = subject
        .to_str()
        .ok_or_else(|| ManagementEvidenceError::NonUtf8Path(subject.display().to_string()))?;
    if subject_path.is_empty() || Path::new(subject_path).is_absolute() {
        return Err(ManagementEvidenceError::InvalidGitOutput {
            operation: "计算 Git 相对路径",
        });
    }

    let head_output = run_git(
        &authority_root,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("HEAD^{commit}"),
        ],
        "rev-parse --verify HEAD^{commit}",
    )?;
    if !head_output.status.success() {
        // 有效但尚无首个 commit 的仓库没有 HEAD 证据；它不是 Git 损坏。
        if is_unborn_head(&authority_root)? {
            return Ok(ManagementEvidenceInspection::Absent);
        }
        return Err(command_failed(
            "rev-parse --verify HEAD^{commit}",
            &head_output,
        ));
    }
    let snapshot_commit_oid =
        parse_object_id(&head_output.stdout, "rev-parse --verify HEAD^{commit}")?;

    // 使用刚验证的 commit OID，避免扫描期间 HEAD 改变后把两次命令混成一份证据。
    let tree_output =
        run_git_with_literal_pathspec(&authority_root, &snapshot_commit_oid, subject.as_os_str())?;
    if !tree_output.status.success() {
        return Err(command_failed("ls-tree", &tree_output));
    }
    match parse_exact_tree_entry(&tree_output.stdout, subject.as_os_str())? {
        TreeEntry::RegularBlob => {
            // Git 查询结束后再检查一次，避免并发路径替换让证据绑定到已跳出的实际文件。
            if !is_path_inside_project_without_symlinks(project_root, skill_file)? {
                return Ok(ManagementEvidenceInspection::Absent);
            }
            Ok(ManagementEvidenceInspection::Confirmed(
                ManagementEvidence {
                    kind: ManagementEvidenceKind::GitHeadTracked,
                    authority_root: path_to_utf8(&authority_root)?,
                    snapshot_commit_oid,
                    subject_path: subject_path.to_owned(),
                },
            ))
        }
        TreeEntry::AbsentOrUnsupported => Ok(ManagementEvidenceInspection::Absent),
    }
}

/// Project 根本身已经通过文件系统身份校验；这里继续拒绝其下任意链接跳转。
fn is_path_inside_project_without_symlinks(
    project_root: &Path,
    subject: &Path,
) -> Result<bool, ManagementEvidenceError> {
    let relative = match subject.strip_prefix(project_root) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative,
        _ => return Ok(false),
    };
    let mut current = PathBuf::from(project_root);
    let component_count = relative.components().count();
    for (index, component) in relative.components().enumerate() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current).map_err(|source| {
            ManagementEvidenceError::InspectPath {
                path: current.display().to_string(),
                source,
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Ok(false);
        }
        let expected_type_matches = if index + 1 == component_count {
            metadata.is_file()
        } else {
            metadata.is_dir()
        };
        if !expected_type_matches {
            return Err(ManagementEvidenceError::InvalidGitOutput {
                operation: "验证 Project Skill 路径",
            });
        }
    }
    Ok(true)
}

fn nearest_git_marker(location: &Path) -> Result<Option<PathBuf>, ManagementEvidenceError> {
    let mut ancestor = Some(location);
    while let Some(directory) = ancestor {
        let marker = directory.join(".git");
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.is_dir() || metadata.is_file() => return Ok(Some(marker)),
            Ok(_) => {
                return Err(ManagementEvidenceError::UnsafeGitMarker(
                    marker.display().to_string(),
                ));
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ManagementEvidenceError::InspectPath {
                    path: marker.display().to_string(),
                    source,
                });
            }
        }
        ancestor = directory.parent();
    }
    Ok(None)
}

fn is_unborn_head(authority_root: &Path) -> Result<bool, ManagementEvidenceError> {
    let output = run_git(
        authority_root,
        &[
            OsStr::new("symbolic-ref"),
            OsStr::new("-q"),
            OsStr::new("HEAD"),
        ],
        "symbolic-ref -q HEAD",
    )?;
    if !output.status.success() {
        return Ok(false);
    }
    let reference = successful_utf8_stdout(output, "symbolic-ref -q HEAD")?;
    if !reference.starts_with("refs/heads/") || reference.len() == "refs/heads/".len() {
        return Ok(false);
    }
    let reference_output = run_git(
        authority_root,
        &[
            OsStr::new("show-ref"),
            OsStr::new("--verify"),
            OsStr::new("--quiet"),
            OsStr::new(&reference),
        ],
        "show-ref --verify HEAD ref",
    )?;
    match reference_output.status.code() {
        Some(0) => Ok(false),
        // `show-ref --verify --quiet` 的 1 明确表示 ref 尚不存在，即 unborn branch。
        Some(1) => Ok(true),
        _ => Err(command_failed(
            "show-ref --verify HEAD ref",
            &reference_output,
        )),
    }
}

fn run_git(
    working_directory: &Path,
    arguments: &[&OsStr],
    operation: &'static str,
) -> Result<Output, ManagementEvidenceError> {
    let mut command = Command::new(GIT_PATH);
    command
        .env_clear()
        .arg("-C")
        .arg(working_directory)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("PATH", "/usr/bin:/bin");
    // 清空外部环境，避免 GIT_NAMESPACE、GIT_CEILING_DIRECTORIES 等改变证据仓库。
    command
        .output()
        .map_err(|source| ManagementEvidenceError::RunGit { operation, source })
}

fn run_git_with_literal_pathspec(
    authority_root: &Path,
    snapshot_commit_oid: &str,
    subject: &OsStr,
) -> Result<Output, ManagementEvidenceError> {
    let arguments = [
        OsStr::new("--literal-pathspecs"),
        OsStr::new("ls-tree"),
        OsStr::new("-z"),
        OsStr::new(snapshot_commit_oid),
        OsStr::new("--"),
        subject,
    ];
    run_git(authority_root, &arguments, "ls-tree")
}

fn successful_utf8_stdout(
    output: Output,
    operation: &'static str,
) -> Result<String, ManagementEvidenceError> {
    if !output.status.success() {
        return Err(command_failed(operation, &output));
    }
    let value = std::str::from_utf8(&output.stdout)
        .map_err(|_| ManagementEvidenceError::InvalidGitOutput { operation })?
        .trim_end_matches(['\n', '\r']);
    if value.is_empty() || value.contains('\n') || value.contains('\r') {
        return Err(ManagementEvidenceError::InvalidGitOutput { operation });
    }
    Ok(value.to_owned())
}

fn parse_object_id(
    bytes: &[u8],
    operation: &'static str,
) -> Result<String, ManagementEvidenceError> {
    let value = std::str::from_utf8(bytes)
        .map_err(|_| ManagementEvidenceError::InvalidGitOutput { operation })?
        .trim_end_matches(['\n', '\r']);
    if !(40..=64).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ManagementEvidenceError::InvalidGitOutput { operation });
    }
    Ok(value.to_ascii_lowercase())
}

enum TreeEntry {
    RegularBlob,
    AbsentOrUnsupported,
}

fn parse_exact_tree_entry(
    bytes: &[u8],
    expected_path: &OsStr,
) -> Result<TreeEntry, ManagementEvidenceError> {
    if bytes.is_empty() {
        return Ok(TreeEntry::AbsentOrUnsupported);
    }
    if !bytes.ends_with(&[0]) || bytes[..bytes.len() - 1].contains(&0) {
        return Err(ManagementEvidenceError::InvalidGitOutput {
            operation: "ls-tree",
        });
    }
    let record = &bytes[..bytes.len() - 1];
    let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
        return Err(ManagementEvidenceError::InvalidGitOutput {
            operation: "ls-tree",
        });
    };
    let header = std::str::from_utf8(&record[..tab]).map_err(|_| {
        ManagementEvidenceError::InvalidGitOutput {
            operation: "ls-tree",
        }
    })?;
    let fields = header.split_ascii_whitespace().collect::<Vec<_>>();
    if fields.len() != 3
        || !(40..=64).contains(&fields[2].len())
        || !fields[2].bytes().all(|byte| byte.is_ascii_hexdigit())
        || record[tab + 1..] != *expected_path.as_bytes()
    {
        return Err(ManagementEvidenceError::InvalidGitOutput {
            operation: "ls-tree",
        });
    }
    if matches!(fields[0], "100644" | "100755") && fields[1] == "blob" {
        Ok(TreeEntry::RegularBlob)
    } else {
        Ok(TreeEntry::AbsentOrUnsupported)
    }
}

fn command_failed(operation: &'static str, output: &Output) -> ManagementEvidenceError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr.trim();
    ManagementEvidenceError::GitFailed {
        operation,
        message: if message.is_empty() {
            format!("退出状态 {}", output.status)
        } else {
            message.to_owned()
        },
    }
}

fn path_to_utf8(path: &Path) -> Result<String, ManagementEvidenceError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| ManagementEvidenceError::NonUtf8Path(path.display().to_string()))
}
