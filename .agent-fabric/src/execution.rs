use crate::contracts::safe_identifier;
use crate::discovery::{confirm_command, load_connection};
use crate::error::{FabricError, FabricResult};
use crate::fs_guard::atomic_write;
use crate::json::{canonical_json, Json};
use crate::process_guard::{output_text, run_fixed_process};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn insert(target: &mut Json, key: &str, value: Json) -> FabricResult<()> {
    target
        .insert(key, value)
        .map_err(|error| FabricError::new("internal_json_error", error))
}

fn new_run_id() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("probe-{}-{nonce}", std::process::id())
}

fn write_trace(
    root: &Path,
    run_id: &str,
    request: &str,
    stdout: &[u8],
    stderr: &[u8],
    receipt: &Json,
) -> FabricResult<String> {
    let run_id = safe_identifier(run_id, "run id")?;
    let base = format!(".agent-fabric/local/traces/{run_id}");
    atomic_write(
        root,
        &format!("{base}/request.md"),
        request.as_bytes(),
        0o600,
    )?;
    atomic_write(root, &format!("{base}/raw-output.txt"), stdout, 0o600)?;
    atomic_write(root, &format!("{base}/stderr.txt"), stderr, 0o600)?;
    atomic_write(
        root,
        &format!("{base}/receipt.json"),
        canonical_json(receipt).as_bytes(),
        0o600,
    )?;
    Ok(base)
}

/// 只用于初始化、重新配置或验收时确认本机命令可受控启动。
/// 日常 Agent 协作必须直接使用宿主真实能力，不能经过这个函数。
pub fn run_compatibility_probe(
    root: &Path,
    descriptor_path: &Path,
    command_confirmation: &str,
) -> FabricResult<Json> {
    let connection = load_connection(root, descriptor_path)?;
    let command = confirm_command(&connection, command_confirmation)?;
    let argv = connection.probe.clone().ok_or_else(|| {
        FabricError::new("probe_unavailable", "这个本机连接没有声明兼容性探测参数。")
    })?;
    let request = format!(
        "# 兼容性探测\n\n- 连接：{}\n- 命令：{}\n- 参数：{:?}\n",
        connection.name,
        command.display(),
        argv
    );
    let outcome = run_fixed_process(&command, &argv, root, &[], connection.timeout, 512 * 1024)?;
    let success = outcome.exit_status.success() && !outcome.timed_out && !outcome.output_exceeded;
    let run_id = new_run_id();
    let mut receipt = Json::object();
    insert(
        &mut receipt,
        "exit_code",
        Json::from(outcome.exit_status.code().unwrap_or(-1)),
    )?;
    insert(&mut receipt, "name", Json::from(connection.name.clone()))?;
    insert(
        &mut receipt,
        "output_exceeded",
        Json::Bool(outcome.output_exceeded),
    )?;
    insert(&mut receipt, "run_id", Json::from(run_id.clone()))?;
    insert(&mut receipt, "success", Json::Bool(success))?;
    insert(&mut receipt, "timed_out", Json::Bool(outcome.timed_out))?;

    let retained_path = if success {
        None
    } else {
        Some(write_trace(
            root,
            &run_id,
            &request,
            &outcome.stdout,
            &outcome.stderr,
            &receipt,
        )?)
    };
    let mut report = Json::object();
    insert(&mut report, "executed", Json::Bool(true))?;
    insert(&mut report, "name", Json::from(connection.name))?;
    insert(&mut report, "ok", Json::Bool(success))?;
    insert(
        &mut report,
        "retained_path",
        retained_path.map(Json::from).unwrap_or(Json::Null),
    )?;
    insert(
        &mut report,
        "status",
        Json::from(if success {
            "probe_passed"
        } else {
            "probe_failed"
        }),
    )?;
    // 输出保持自由文本；兼容性探测不要求候选返回 JSON。
    insert(
        &mut report,
        "stderr",
        Json::from(output_text(&outcome.stderr)),
    )?;
    insert(
        &mut report,
        "stdout",
        Json::from(output_text(&outcome.stdout)),
    )?;
    Ok(report)
}
