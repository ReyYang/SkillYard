use crate::error::{FabricError, FabricResult};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct ProcessOutcome {
    pub exit_status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub output_exceeded: bool,
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

pub fn find_executable(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 {
        return executable_file(candidate).then(|| candidate.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if executable_file(&candidate) {
            // rustup 等多调用名代理依赖 argv[0]；保留 PATH 中的入口名，不能解析成底层目标。
            return Some(candidate);
        }
    }
    None
}

fn drain<R: Read + Send + 'static>(
    mut reader: R,
    total: Arc<AtomicUsize>,
    exceeded: Arc<AtomicBool>,
    limit: usize,
) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let previous = total.fetch_add(count, Ordering::Relaxed);
                    if previous.saturating_add(count) > limit {
                        exceeded.store(true, Ordering::Relaxed);
                    }
                    if previous < limit {
                        let available = limit - previous;
                        kept.extend_from_slice(&buffer[..count.min(available)]);
                    }
                }
            }
        }
        kept
    })
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // 独立进程组允许超时或输出超限时终止 CLI 的子进程树。
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_process_group(pid: u32) {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    const SIGTERM: i32 = 15;
    const SIGKILL: i32 = 9;
    unsafe {
        let _ = kill(-(pid as i32), SIGTERM);
    }
    thread::sleep(Duration::from_millis(100));
    unsafe {
        let _ = kill(-(pid as i32), SIGKILL);
    }
}

#[cfg(not(unix))]
fn terminate_process_group(_pid: u32) {}

/// 固定 argv、无 shell、受限环境、总输出上限和进程组超时。
pub fn run_fixed_process(
    executable: &Path,
    argv: &[String],
    root: &Path,
    stdin_bytes: &[u8],
    timeout: Duration,
    output_limit: usize,
) -> FabricResult<ProcessOutcome> {
    let mut command = Command::new(executable);
    command
        .args(argv)
        .current_dir(root)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in [
        "HOME",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "PATH",
        "TERM",
        "TMPDIR",
        "XDG_CONFIG_HOME",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.env("NO_COLOR", "1");
    configure_process_group(&mut command);
    let mut child = command.spawn().map_err(|error| {
        FabricError::new(
            "process_spawn_failed",
            format!("无法启动 {}：{error}", executable.display()),
        )
    })?;
    let pid = child.id();
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| FabricError::new("process_spawn_failed", "无法打开进程 stdin。"))?;
    let input = stdin_bytes.to_vec();
    let writer = thread::spawn(move || {
        let _ = stdin.write_all(&input);
        let _ = stdin.flush();
    });
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| FabricError::new("process_spawn_failed", "无法打开进程 stdout。"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| FabricError::new("process_spawn_failed", "无法打开进程 stderr。"))?;
    let total = Arc::new(AtomicUsize::new(0));
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout_reader = drain(
        stdout,
        Arc::clone(&total),
        Arc::clone(&exceeded),
        output_limit,
    );
    let stderr_reader = drain(
        stderr,
        Arc::clone(&total),
        Arc::clone(&exceeded),
        output_limit,
    );

    let started = Instant::now();
    let mut timed_out = false;
    let exit_status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            terminate_process_group(pid);
            let _ = child.kill();
            break child.wait()?;
        }
        if exceeded.load(Ordering::Relaxed) {
            terminate_process_group(pid);
            let _ = child.kill();
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let _ = writer.join();
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(ProcessOutcome {
        exit_status,
        stdout,
        stderr,
        timed_out,
        output_exceeded: exceeded.load(Ordering::Relaxed),
    })
}

pub fn output_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}
