use std::{env, process::Command};

const BACKTRACE_WORKER: &str = "engineering_contract::limited_debug_backtrace_worker";
const BACKTRACE_WORKER_GATE: &str = "SKILLYARD_LIMITED_DEBUG_BACKTRACE_WORKER";
const BACKTRACE_SENTINEL: &str = "SKILLYARD_LIMITED_DEBUG_BACKTRACE_SENTINEL";
const MANIFEST: &str = include_str!("../../../Cargo.toml");

fn assignment<'a>(manifest: &'a str, table: &str, key: &str) -> Option<&'a str> {
    let mut current_table = None;

    for raw_line in manifest.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') && line.ends_with(']') {
            current_table = Some(&line[1..line.len() - 1]);
            continue;
        }
        if current_table != Some(table) {
            continue;
        }
        let Some((candidate_key, value)) = line.split_once('=') else {
            continue;
        };
        if candidate_key.trim() == key {
            return Some(value.trim());
        }
    }

    None
}

fn table_count(manifest: &str, table: &str) -> usize {
    manifest
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| *line == format!("[{table}]").as_str())
        .count()
}

fn manifest_assigns_value(manifest: &str, key: &str, expected_value: &str) -> bool {
    manifest.lines().any(|raw_line| {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        let Some((candidate_key, value)) = line.split_once('=') else {
            return false;
        };
        candidate_key.trim() == key && value.trim() == expected_value
    })
}

fn backtrace_has_positive_source_line(backtrace: &str) -> bool {
    let source_path = "tests/all/engineering_contract.rs:";
    backtrace.match_indices(source_path).any(|(index, _)| {
        let suffix = &backtrace[index + source_path.len()..];
        let digits = suffix
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect::<String>();
        digits
            .parse::<u32>()
            .is_ok_and(|line_number| line_number > 0)
    })
}

#[inline(never)]
fn limited_debug_backtrace_fixture() -> ! {
    panic!("{BACKTRACE_SENTINEL}");
}

#[test]
fn workspace_profiles_use_limited_debug_without_disabling_incremental() {
    for profile in ["profile.dev", "profile.test"] {
        assert_eq!(
            table_count(MANIFEST, profile),
            1,
            "workspace manifest 必须且只能声明一次 [{profile}]"
        );
        assert_eq!(
            assignment(MANIFEST, profile, "debug"),
            Some("\"limited\""),
            "[{profile}] 必须使用 limited debug"
        );
        assert_eq!(
            assignment(MANIFEST, profile, "incremental"),
            None,
            "[{profile}] 必须保留 Cargo 的本地 incremental 默认值"
        );
    }

    assert_eq!(
        table_count(MANIFEST, "profile.release"),
        0,
        "A2 不得改变 release profile"
    );
    assert!(
        !manifest_assigns_value(MANIFEST, "debug", "\"full\""),
        "完整 debug 只能通过一次性环境覆盖启用"
    );
    assert!(
        !manifest_assigns_value(MANIFEST, "incremental", "false"),
        "A2 不得持久禁用 incremental"
    );
}

#[test]
fn limited_debug_backtrace_reports_fixture_function_file_and_line() {
    let output = Command::new(env::current_exe().expect("应解析当前 integration test executable"))
        .args([BACKTRACE_WORKER, "--exact", "--nocapture"])
        .env(BACKTRACE_WORKER_GATE, "1")
        .env("RUST_BACKTRACE", "1")
        .env_remove("RUST_LIB_BACKTRACE")
        .output()
        .expect("应启动 limited debug backtrace worker");

    assert!(
        !output.status.success(),
        "backtrace worker 必须因 fixture panic 失败"
    );
    let backtrace = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        backtrace.contains(BACKTRACE_SENTINEL),
        "panic 输出必须包含唯一 sentinel：{backtrace}"
    );
    assert!(
        backtrace.contains("engineering_contract::limited_debug_backtrace_fixture"),
        "limited debug backtrace 必须保留 fixture 函数名：{backtrace}"
    );
    assert!(
        backtrace_has_positive_source_line(&backtrace),
        "limited debug backtrace 必须包含 tests/all/engineering_contract.rs:<正整数行号>：{backtrace}"
    );
}

#[test]
fn limited_debug_backtrace_worker() {
    if env::var_os(BACKTRACE_WORKER_GATE).is_none() {
        return;
    }
    limited_debug_backtrace_fixture();
}
