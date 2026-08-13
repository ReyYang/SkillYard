use crate::error::{FabricError, FabricResult};
use crate::fs_guard::{atomic_write, read_regular, target_for};
use crate::json::{sha256_bytes, sha256_text, Json};
use crate::process_guard::{find_executable, output_text, run_fixed_process};
use std::fs;
use std::path::Path;
use std::time::Duration;

const SOURCE_FILES: [&str; 12] = [
    ".agent-fabric/src/blueprint.rs",
    ".agent-fabric/src/contracts.rs",
    ".agent-fabric/src/discovery.rs",
    ".agent-fabric/src/error.rs",
    ".agent-fabric/src/execution.rs",
    ".agent-fabric/src/fabric.rs",
    ".agent-fabric/src/fs_guard.rs",
    ".agent-fabric/src/json.rs",
    ".agent-fabric/src/main.rs",
    ".agent-fabric/src/materialize.rs",
    ".agent-fabric/src/process_guard.rs",
    ".agent-fabric/src/runtime_build.rs",
];

pub const RUNTIME_BINARY: &str = ".agent-fabric/bin/agent-fabric-core";
pub const RUNTIME_STAMP: &str = ".agent-fabric/bin/.runtime-source.sha256";

fn source_revision(root: &Path) -> FabricResult<String> {
    let mut material = String::new();
    for relative in SOURCE_FILES {
        let content = read_regular(root, relative)?;
        material.push_str(relative);
        material.push('\0');
        material.push_str(&sha256_bytes(&content));
        material.push('\n');
    }
    Ok(sha256_text(&material))
}

/// 仅供离线 bootstrap 编译脚本记录刚构建二进制对应的源码 revision。
pub fn stamp_current_runtime(root: &Path) -> FabricResult<Json> {
    let binary = target_for(root, RUNTIME_BINARY, true)?;
    if !binary.is_file() {
        return Err(FabricError::new(
            "runtime_missing",
            "运行时二进制不存在，不能写入源码 stamp。",
        ));
    }
    let desired = source_revision(root)?;
    atomic_write(
        root,
        RUNTIME_STAMP,
        format!("{desired}\n").as_bytes(),
        0o644,
    )?;
    runtime_status(root)
}

pub fn runtime_status(root: &Path) -> FabricResult<Json> {
    let desired = source_revision(root);
    let binary_exists = target_for(root, RUNTIME_BINARY, false)
        .map(|path| path.is_file())
        .unwrap_or(false);
    let observed_stamp = target_for(root, RUNTIME_STAMP, false)
        .ok()
        .filter(|path| path.is_file())
        .and_then(|_| read_regular(root, RUNTIME_STAMP).ok())
        .and_then(|value| String::from_utf8(value).ok())
        .map(|value| value.trim().to_string());
    let mut report = Json::object();
    match desired {
        Ok(desired) => {
            let rustc = find_executable("rustc");
            report
                .insert("binary_present", Json::Bool(binary_exists))
                .map_err(|error| FabricError::new("internal_json_error", error))?;
            report
                .insert("desired_source_sha256", Json::from(desired.clone()))
                .map_err(|error| FabricError::new("internal_json_error", error))?;
            report
                .insert(
                    "status",
                    Json::from(
                        if binary_exists && observed_stamp.as_deref() == Some(&desired) {
                            "ready"
                        } else if rustc.is_none() {
                            "unavailable"
                        } else {
                            "build_required"
                        },
                    ),
                )
                .map_err(|error| FabricError::new("internal_json_error", error))?;
        }
        Err(error) => {
            report
                .insert("binary_present", Json::Bool(binary_exists))
                .map_err(|json_error| FabricError::new("internal_json_error", json_error))?;
            report
                .insert("reason", Json::from(error.code))
                .map_err(|json_error| FabricError::new("internal_json_error", json_error))?;
            report
                .insert("status", Json::from("source_missing"))
                .map_err(|json_error| FabricError::new("internal_json_error", json_error))?;
        }
    }
    report
        .insert("framework_usable", Json::Bool(true))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    report
        .insert(
            "rustc",
            find_executable("rustc")
                .map(|path| Json::from(path.display().to_string()))
                .unwrap_or(Json::Null),
        )
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    Ok(report)
}

pub fn ensure_runtime(root: &Path) -> FabricResult<Json> {
    let desired = source_revision(root)?;
    let status = runtime_status(root)?;
    if status
        .get("status")
        .ok()
        .and_then(|value| value.as_str().ok())
        == Some("ready")
    {
        return Ok(status);
    }
    let Some(rustc) = find_executable("rustc") else {
        // Rust 是可选维护能力；缺失不能让已经恢复的 Markdown/Skill 框架失败。
        return runtime_status(root);
    };
    let main = root.join(".agent-fabric/src/main.rs");
    let temporary = root.join(format!(
        ".agent-fabric/bin/.agent-fabric-core-build-{}",
        std::process::id()
    ));
    let argv = vec![
        "--edition=2021".to_string(),
        "-C".to_string(),
        "opt-level=2".to_string(),
        "-C".to_string(),
        "debuginfo=0".to_string(),
        "-o".to_string(),
        temporary.display().to_string(),
        main.display().to_string(),
    ];
    let outcome = run_fixed_process(
        &rustc,
        &argv,
        root,
        &[],
        Duration::from_secs(180),
        1024 * 1024,
    )?;
    if outcome.timed_out || outcome.output_exceeded || !outcome.exit_status.success() {
        let _ = fs::remove_file(&temporary);
        return Err(FabricError::new(
            "runtime_build_failed",
            "Rust Core 编译失败；未替换现有运行时。",
        )
        .with_details(format!(
            "stdout={} stderr={}",
            output_text(&outcome.stdout),
            output_text(&outcome.stderr)
        )));
    }
    let binary = target_for(root, RUNTIME_BINARY, false)?;
    fs::rename(&temporary, &binary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    }
    atomic_write(
        root,
        RUNTIME_STAMP,
        format!("{desired}\n").as_bytes(),
        0o644,
    )?;
    runtime_status(root)
}
