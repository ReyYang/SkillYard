use crate::blueprint::{object, string_array};
use crate::error::{FabricError, FabricResult};
use crate::fs_guard::{atomic_write, read_regular, target_for};
use crate::json::{canonical_json, parse_json, Json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const RESOLUTION_PATH: &str = ".agent-fabric/local/resolution.json";
const ADAPTER_DIRECTORY: &str = ".agent-fabric/local/adapters";

#[derive(Clone, Debug)]
pub struct LocalConnection {
    pub command: Option<PathBuf>,
    pub name: String,
    pub probe: Option<Vec<String>>,
    pub timeout: Duration,
}

fn insert(target: &mut Json, key: &str, value: Json) -> FabricResult<()> {
    target
        .insert(key, value)
        .map_err(|error| FabricError::new("internal_json_error", error))
}

#[cfg(unix)]
fn executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn executable_file(path: &Path) -> bool {
    path.is_file()
}

fn canonical_input(root: &Path, input: &Path) -> std::io::Result<PathBuf> {
    if input.is_absolute() {
        input.canonicalize()
    } else {
        root.join(input).canonicalize()
    }
}

fn descriptor_file(root: &Path, descriptor_path: &Path) -> FabricResult<PathBuf> {
    let canonical = canonical_input(root, descriptor_path).map_err(|error| {
        FabricError::new("invalid_connection", format!("本机连接说明不存在：{error}"))
    })?;
    let allowed = root.join(ADAPTER_DIRECTORY);
    let allowed = allowed
        .canonicalize()
        .map_err(|_| FabricError::new("invalid_connection", "本机连接说明目录尚未创建。"))?;
    if !canonical.starts_with(&allowed) {
        return Err(FabricError::new(
            "connection_outside_local_directory",
            "本机连接说明必须位于 .agent-fabric/local/adapters/。",
        ));
    }
    let metadata = fs::symlink_metadata(&canonical)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(FabricError::new(
            "invalid_connection",
            "本机连接说明必须是普通 JSON 文件。",
        ));
    }
    Ok(canonical)
}

/// Rust 只识别兼容性探测所需的少量字段；其他字段由当前 Agent 自由维护。
pub fn load_connection(root: &Path, descriptor_path: &Path) -> FabricResult<LocalConnection> {
    let canonical = descriptor_file(root, descriptor_path)?;
    let text = fs::read_to_string(&canonical)?;
    let data = parse_json(&text).map_err(|error| FabricError::new("invalid_connection", error))?;
    object(&data)
        .map_err(|_| FabricError::new("invalid_connection", "本机连接说明必须是 JSON object。"))?;
    let name = data
        .get_opt("name")
        .and_then(|value| value.as_str().ok())
        .map(ToString::to_string)
        .or_else(|| {
            canonical
                .file_stem()
                .map(|value| value.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "local connection".to_string());

    let command = match data.get_opt("command") {
        Some(value) => {
            let raw = value
                .as_str()
                .map_err(|_| FabricError::new("invalid_connection", "command 必须是字符串。"))?;
            let command = canonical_input(root, Path::new(raw)).map_err(|error| {
                FabricError::new(
                    "connection_command_missing",
                    format!("连接命令无法解析：{error}"),
                )
            })?;
            if !executable_file(&command) {
                return Err(FabricError::new(
                    "connection_command_missing",
                    "连接命令不存在或不可执行。",
                ));
            }
            Some(command)
        }
        None => None,
    };
    let probe = match data.get_opt("probe") {
        Some(value) => Some(
            string_array(value)
                .map_err(|_| FabricError::new("invalid_connection", "probe 必须是字符串数组。"))?,
        ),
        None => None,
    };
    if probe
        .as_ref()
        .is_some_and(|values| values.iter().any(|value| value.contains('\0')))
    {
        return Err(FabricError::new(
            "invalid_connection",
            "probe 参数不能包含 NUL。",
        ));
    }
    let timeout_seconds = data
        .get_opt("timeout_seconds")
        .and_then(|value| value.as_u64().ok())
        .unwrap_or(15);
    if timeout_seconds == 0 || timeout_seconds > 120 {
        return Err(FabricError::new(
            "invalid_connection",
            "兼容性探测 timeout 必须在安全范围内。",
        ));
    }
    Ok(LocalConnection {
        command,
        name,
        probe,
        timeout: Duration::from_secs(timeout_seconds),
    })
}

pub fn check_connection(root: &Path, descriptor_path: &Path) -> FabricResult<Json> {
    let connection = load_connection(root, descriptor_path)?;
    let probe_available = connection.command.is_some() && connection.probe.is_some();
    let mut report = Json::object();
    insert(&mut report, "executed", Json::Bool(false))?;
    insert(&mut report, "name", Json::from(connection.name))?;
    insert(&mut report, "ok", Json::Bool(true))?;
    insert(&mut report, "probe_available", Json::Bool(probe_available))?;
    insert(
        &mut report,
        "status",
        Json::from(if probe_available {
            "ready_for_probe"
        } else {
            "documented"
        }),
    )?;
    Ok(report)
}

pub fn confirm_command(connection: &LocalConnection, confirmation: &str) -> FabricResult<PathBuf> {
    let command = connection.command.as_ref().ok_or_else(|| {
        FabricError::new(
            "probe_unavailable",
            "这个本机连接没有声明可执行的兼容性探测命令。",
        )
    })?;
    let confirmed = Path::new(confirmation).canonicalize().map_err(|error| {
        FabricError::new(
            "command_confirmation_mismatch",
            format!("无法解析确认命令：{error}"),
        )
    })?;
    if &confirmed != command {
        return Err(FabricError::new(
            "command_confirmation_mismatch",
            "确认命令与连接说明中的命令不一致。",
        ));
    }
    Ok(command.clone())
}

fn initial_resolution() -> FabricResult<Json> {
    let mut orchestrator = Json::object();
    insert(
        &mut orchestrator,
        "collaborator",
        Json::from("current Agent"),
    )?;
    insert(
        &mut orchestrator,
        "invocation",
        Json::from("current session"),
    )?;
    insert(&mut orchestrator, "model", Json::from("current selection"))?;
    let mut resolution = Json::object();
    insert(&mut resolution, "orchestrator", orchestrator)?;
    insert(&mut resolution, "roles", Json::object())?;
    let mut projection = Json::object();
    insert(&mut projection, "status", Json::from("not_recorded"))?;
    insert(&mut resolution, "skill_projection", projection)?;
    Ok(resolution)
}

/// init 只创建最小本机真值；真实发现、推荐和模型选择由初始化 Agent 补充。
pub fn ensure_initial_resolution(root: &Path) -> FabricResult<Json> {
    let target = target_for(root, RESOLUTION_PATH, false)?;
    if !target.exists() {
        atomic_write(
            root,
            RESOLUTION_PATH,
            canonical_json(&initial_resolution()?).as_bytes(),
            0o600,
        )?;
    }
    resolution_status(root)
}

pub fn resolution_status(root: &Path) -> FabricResult<Json> {
    let target = target_for(root, RESOLUTION_PATH, false)?;
    if !target.exists() {
        let mut report = Json::object();
        insert(&mut report, "configured_roles", Json::array())?;
        insert(&mut report, "ok", Json::Bool(true))?;
        insert(&mut report, "status", Json::from("not_configured"))?;
        return Ok(report);
    }
    let raw = read_regular(root, RESOLUTION_PATH)?;
    let text = String::from_utf8(raw).map_err(|_| {
        FabricError::new("invalid_local_resolution", "本机协作者配置必须使用 UTF-8。")
    })?;
    let resolution =
        parse_json(&text).map_err(|error| FabricError::new("invalid_local_resolution", error))?;
    object(&resolution).map_err(|_| {
        FabricError::new(
            "invalid_local_resolution",
            "本机协作者配置必须是 JSON object。",
        )
    })?;
    let roles = match resolution.get_opt("roles") {
        Some(value) => object(value).map_err(|_| {
            FabricError::new(
                "invalid_local_resolution",
                "roles 必须是可以扩展的 JSON object。",
            )
        })?,
        None => {
            return Err(FabricError::new(
                "invalid_local_resolution",
                "本机协作者配置缺少 roles。",
            ))
        }
    };
    let configured_roles = roles.keys().cloned().map(Json::from).collect();
    let mut report = Json::object();
    insert(
        &mut report,
        "configured_roles",
        Json::Array(configured_roles),
    )?;
    insert(
        &mut report,
        "orchestrator_current",
        Json::Bool(resolution.get_opt("orchestrator").is_some()),
    )?;
    insert(&mut report, "ok", Json::Bool(true))?;
    insert(&mut report, "status", Json::from("configured"))?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::{field, string_field};

    #[test]
    fn initial_resolution_keeps_other_roles_optional() {
        let value = initial_resolution().unwrap();
        assert!(object(field(&value, "roles").unwrap()).unwrap().is_empty());
        assert_eq!(
            string_field(field(&value, "orchestrator").unwrap(), "collaborator").unwrap(),
            "current Agent"
        );
    }
}
