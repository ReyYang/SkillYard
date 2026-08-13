use crate::error::{FabricError, FabricResult};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn validate_relative_path(value: &str) -> FabricResult<String> {
    if value.is_empty() || value.contains('\0') || value.contains('\\') {
        return Err(FabricError::new(
            "invalid_path",
            format!("非法相对路径：{value:?}"),
        ));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(FabricError::new(
            "absolute_path",
            format!("禁止绝对路径：{value}"),
        ));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(FabricError::new(
                    "path_traversal",
                    format!("路径必须是规范项目内相对路径：{value}"),
                ));
            }
        }
    }
    if value.starts_with('/') || value.ends_with('/') || value.contains("//") {
        return Err(FabricError::new(
            "invalid_path",
            format!("路径不是规范形式：{value}"),
        ));
    }
    Ok(value.to_string())
}

fn is_same_or_child(path: &Path, parent: &Path) -> bool {
    path == parent || path.starts_with(parent)
}

/// 用户授权根必须是普通项目目录，不能是系统根、主目录或通用凭据目录。
pub fn canonical_project_root(value: &Path) -> FabricResult<PathBuf> {
    let root = value.canonicalize().map_err(|error| {
        FabricError::new(
            "invalid_project_root",
            format!("项目根不存在或无法解析：{}（{error}）", value.display()),
        )
    })?;
    if !root.is_dir() {
        return Err(FabricError::new(
            "invalid_project_root",
            "项目根必须是现有目录。",
        ));
    }
    if root.parent().is_none() {
        return Err(FabricError::new(
            "unsafe_project_root",
            "禁止把文件系统根作为项目根。",
        ));
    }
    if let Some(home) = std::env::var_os("HOME") {
        if let Ok(home) = PathBuf::from(home).canonicalize() {
            if root == home {
                return Err(FabricError::new(
                    "unsafe_project_root",
                    "禁止把用户主目录作为项目根。",
                ));
            }
            for relative in [".config", ".ssh"] {
                let global = home.join(relative);
                if is_same_or_child(&root, &global) {
                    return Err(FabricError::new(
                        "unsafe_project_root",
                        format!("禁止在全局配置或凭据目录中初始化：{}", root.display()),
                    ));
                }
            }
        }
    }
    Ok(root)
}

pub fn has_git_marker(root: &Path) -> bool {
    root.join(".git").symlink_metadata().is_ok()
}

pub fn require_root_confirmation(root: &Path, confirmation: Option<&str>) -> FabricResult<()> {
    let Some(raw) = confirmation else {
        return Err(FabricError::new(
            "root_confirmation_required",
            format!("写入前必须传入准确的 --confirm-root {}", root.display()),
        ));
    };
    let confirmed = Path::new(raw).canonicalize().map_err(|error| {
        FabricError::new(
            "root_confirmation_mismatch",
            format!("无法解析确认根：{error}"),
        )
    })?;
    if confirmed != root {
        return Err(FabricError::new(
            "root_confirmation_mismatch",
            format!(
                "确认根 {} 与规范项目根 {} 不一致。",
                confirmed.display(),
                root.display()
            ),
        ));
    }
    Ok(())
}

/// 逐段检查现存组件，禁止任何受管读写穿过 symlink。
pub fn target_for(root: &Path, relative: &str, require_existing: bool) -> FabricResult<PathBuf> {
    validate_relative_path(relative)?;
    let mut current = root.to_path_buf();
    let components: Vec<_> = Path::new(relative).components().collect();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(FabricError::new(
                        "symlink_escape",
                        format!("受管路径不能经过 symlink：{relative}"),
                    ));
                }
                if index + 1 < components.len() && !metadata.is_dir() {
                    return Err(FabricError::new(
                        "invalid_parent",
                        format!("受管路径父级不是目录：{relative}"),
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if require_existing {
                    return Err(FabricError::new(
                        "target_missing",
                        format!("目标不存在：{relative}"),
                    ));
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(current)
}

pub fn read_regular(root: &Path, relative: &str) -> FabricResult<Vec<u8>> {
    let target = target_for(root, relative, true)?;
    let metadata = fs::symlink_metadata(&target)?;
    if !metadata.is_file() {
        return Err(FabricError::new(
            "not_regular_file",
            format!("只允许读取普通文件：{relative}"),
        ));
    }
    Ok(fs::read(target)?)
}

fn ensure_safe_parent(root: &Path, relative: &str) -> FabricResult<PathBuf> {
    validate_relative_path(relative)?;
    let target = root.join(relative);
    let parent = target
        .parent()
        .ok_or_else(|| FabricError::new("invalid_parent", "目标缺少父目录。"))?;
    let mut current = root.to_path_buf();
    let relative_parent = parent
        .strip_prefix(root)
        .map_err(|_| FabricError::new("path_escape", "目标父目录逃出项目根。"))?;
    for component in relative_parent.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(FabricError::new(
                        "unsafe_parent",
                        format!("受管路径父级不安全：{relative}"),
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(target)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> FabricResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> FabricResult<()> {
    Ok(())
}

/// 在目标同目录写完整临时文件并 fsync，再原子替换。
pub fn atomic_write(root: &Path, relative: &str, content: &[u8], mode: u32) -> FabricResult<()> {
    let target = ensure_safe_parent(root, relative)?;
    target_for(root, relative, false)?;
    let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_name = format!(".agent-fabric-tmp-{}-{nonce}", std::process::id());
    let temp = target
        .parent()
        .ok_or_else(|| FabricError::new("invalid_parent", "目标缺少父目录。"))?
        .join(temp_name);
    let result = (|| -> FabricResult<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(content)?;
        file.sync_all()?;
        set_mode(&temp, mode)?;
        target_for(root, relative, false)?;
        fs::rename(&temp, &target)?;
        if let Some(parent) = target.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if temp.exists() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn remove_managed_file(root: &Path, relative: &str) -> FabricResult<()> {
    let target = target_for(root, relative, true)?;
    let metadata = fs::symlink_metadata(&target)?;
    if !metadata.is_file() {
        return Err(FabricError::new(
            "not_regular_file",
            format!("拒绝删除非普通文件：{relative}"),
        ));
    }
    fs::remove_file(target)?;
    Ok(())
}

pub fn find_interrupted_temps(root: &Path) -> FabricResult<Vec<String>> {
    fn visit(root: &Path, directory: &Path, output: &mut Vec<String>) -> FabricResult<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            let name = entry.file_name().to_string_lossy().to_string();
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                if name != ".git" {
                    visit(root, &path, output)?;
                }
            } else if metadata.is_file() && name.starts_with(".agent-fabric-tmp-") {
                if let Ok(relative) = path.strip_prefix(root) {
                    output.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        Ok(())
    }

    let mut output = Vec::new();
    visit(root, root, &mut output)?;
    output.sort();
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_and_absolute_paths_are_rejected() {
        assert!(validate_relative_path("../outside").is_err());
        assert!(validate_relative_path("/tmp/outside").is_err());
        assert!(validate_relative_path("a//b").is_err());
        assert_eq!(validate_relative_path("a/b.txt").unwrap(), "a/b.txt");
    }
}
