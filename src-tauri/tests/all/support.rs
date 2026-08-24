use std::{env, fs, fs::OpenOptions, io::Write, path::Path, process::Command};

const MARKER_PATH_ENV: &str = "SKILLYARD_TEST_HARD_EXIT_MARKER_PATH";
const EXACT_NAME_ENV: &str = "SKILLYARD_TEST_HARD_EXIT_EXACT_NAME";

pub(crate) fn run_hard_exit_child<F>(worker_leaf_name: &str, expected_exit_code: i32, configure: F)
where
    F: FnOnce(&mut Command),
{
    let executable = env::current_exe().expect("应找到当前测试二进制");
    let exact_name = resolve_worker_name(&executable, worker_leaf_name);
    let marker_dir = tempfile::tempdir().expect("应为 hard-exit invocation 创建隔离 marker 目录");
    let marker_path = marker_dir.path().join("worker-entered");

    let mut child = Command::new(&executable);
    child.args(["--exact", &exact_name, "--nocapture"]);
    configure(&mut child);
    let status = child
        .env(MARKER_PATH_ENV, &marker_path)
        .env(EXACT_NAME_ENV, &exact_name)
        .status()
        .expect("应启动 hard-exit 子进程");

    assert_worker_marker(&marker_path, &exact_name);
    assert_eq!(
        status.code(),
        Some(expected_exit_code),
        "子进程必须以精确 hard-exit 状态码退出"
    );
}

pub(crate) fn mark_hard_exit_worker_entered(worker_leaf_name: &str) {
    let exact_name = env::var(EXACT_NAME_ENV).expect("hard-exit worker 必须收到精确测试名");
    assert_eq!(
        exact_name.rsplit("::").next(),
        Some(worker_leaf_name),
        "hard-exit worker 名必须与解析后的精确测试名一致"
    );
    assert!(
        exact_name.contains("::"),
        "hard-exit worker 必须使用 fully-qualified 测试名"
    );

    let marker_path = env::var_os(MARKER_PATH_ENV).expect("hard-exit worker 必须收到 marker 路径");
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker_path)
        .expect("hard-exit worker marker 必须是本 invocation 的新文件");
    marker
        .write_all(exact_name.as_bytes())
        .expect("应完整写入 hard-exit worker marker");
    marker
        .sync_all()
        .expect("hard-exit worker marker 必须在进入生产入口前持久化");
}

fn resolve_worker_name(executable: &Path, worker_leaf_name: &str) -> String {
    let output = Command::new(executable)
        .arg("--list")
        .output()
        .expect("应列出当前测试二进制中的测试");
    assert!(
        output.status.success(),
        "当前测试二进制 --list 必须成功: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let listed = String::from_utf8(output.stdout).expect("测试列表必须是 UTF-8");
    let mut matches = listed.lines().filter_map(|line| {
        let name = line.strip_suffix(": test")?;
        (name.rsplit("::").next() == Some(worker_leaf_name)).then(|| name.to_owned())
    });
    let exact_name = matches
        .next()
        .unwrap_or_else(|| panic!("未找到 hard-exit worker: {worker_leaf_name}"));
    assert!(
        matches.next().is_none(),
        "hard-exit worker 名不唯一: {worker_leaf_name}"
    );
    assert!(
        exact_name.contains("::"),
        "hard-exit worker 必须解析为 fully-qualified 测试名: {exact_name}"
    );
    exact_name
}

fn assert_worker_marker(marker_path: &Path, exact_name: &str) {
    let marker = fs::read_to_string(marker_path)
        .expect("hard-exit 子进程必须在进入生产 Application/failpoint 前写入 marker");
    assert_eq!(
        marker, exact_name,
        "hard-exit marker 必须记录实际启动的精确测试名"
    );
}
