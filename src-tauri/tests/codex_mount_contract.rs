use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::symlink,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use serde_json::{Value, json};
use tempfile::tempdir;

/// 这个测试只读取当前安装的 Codex binary，并把全部配置、Project 和 Skill 放进临时目录。
#[test]
#[ignore = "MAC-CONTRACT：需要当前 Mac 已安装 codex-cli"]
fn current_codex_discovers_global_and_project_directory_symlinks() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let codex_home = sandbox.path().join("codex-home");
    let project = sandbox.path().join("project");
    let sources = sandbox.path().join("sources");
    fs::create_dir_all(codex_home.join("skills")).expect("应创建临时 CODEX_HOME");
    fs::create_dir_all(project.join(".codex/skills")).expect("应创建项目 Skill 根目录");
    fs::create_dir_all(&sources).expect("应创建临时主副本目录");
    let git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&project)
        .status()
        .expect("应能初始化临时 git repo");
    assert!(git.success(), "临时 git repo 初始化失败");

    let global_name = "skillyard-global-contract";
    let project_name = "skillyard-project-contract";
    let global_source = write_contract_skill(&sources, global_name);
    let project_source = write_contract_skill(&sources, project_name);
    symlink(&global_source, codex_home.join("skills").join(global_name))
        .expect("应创建 global 目录软链接");
    symlink(
        &project_source,
        project.join(".codex/skills").join(project_name),
    )
    .expect("应创建 project 目录软链接");

    let mut child = Command::new("codex")
        .args(["app-server", "--stdio", "-c", "analytics.enabled=false"])
        .env("CODEX_HOME", &codex_home)
        .current_dir(&project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("应启动当前 Codex app-server");
    let mut stdin = child.stdin.take().expect("应取得 app-server stdin");
    let stdout = child.stdout.take().expect("应取得 app-server stdout");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                let _ = sender.send(value);
            }
        }
    });

    send_message(
        &mut stdin,
        json!({
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": {"name": "skillyard-contract", "version": "1.0.0"},
                "capabilities": {"experimentalApi": true}
            }
        }),
    );
    receive_response(&receiver, 1);
    send_message(&mut stdin, json!({"method": "initialized"}));
    send_message(
        &mut stdin,
        json!({
            "id": 2,
            "method": "skills/list",
            "params": {"cwds": [project], "forceReload": true}
        }),
    );
    let response = receive_response(&receiver, 2);
    let entries = response["result"]["data"]
        .as_array()
        .expect("skills/list 应返回 data");
    let entry = entries
        .iter()
        .find(|entry| entry["cwd"] == project.to_string_lossy().as_ref())
        .expect("应返回临时 Project 的结果");
    assert_eq!(entry["errors"], json!([]));
    let skills = entry["skills"].as_array().expect("应返回 Skill 数组");
    assert_skill(
        skills,
        global_name,
        "user",
        &fs::canonicalize(global_source.join("SKILL.md")).expect("应解析 global 来源"),
    );
    assert_skill(
        skills,
        project_name,
        "repo",
        &fs::canonicalize(project_source.join("SKILL.md")).expect("应解析 project 来源"),
    );

    let _ = child.kill();
    let _ = child.wait();
}

fn write_contract_skill(parent: &std::path::Path, name: &str) -> std::path::PathBuf {
    let root = parent.join(name);
    fs::create_dir(&root).expect("应创建契约 Skill");
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: SkillYard Codex Mount contract\n---\n"),
    )
    .expect("应写入契约 Skill");
    root
}

fn send_message(stdin: &mut impl Write, message: Value) {
    serde_json::to_writer(&mut *stdin, &message).expect("应序列化 app-server 请求");
    stdin.write_all(b"\n").expect("应写入消息分隔符");
    stdin.flush().expect("应发送 app-server 请求");
}

fn receive_response(receiver: &mpsc::Receiver<Value>, id: i64) -> Value {
    loop {
        let message = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("Codex app-server 应在超时前响应");
        if message["id"] == id {
            assert!(
                message.get("error").is_none(),
                "app-server 返回错误：{message}"
            );
            return message;
        }
    }
}

fn assert_skill(skills: &[Value], name: &str, scope: &str, expected_path: &std::path::Path) {
    let skill = skills
        .iter()
        .find(|skill| skill["name"] == name)
        .unwrap_or_else(|| panic!("Codex 未发现 {name}"));
    assert_eq!(skill["scope"], scope);
    assert_eq!(skill["path"], expected_path.to_string_lossy().as_ref());
}
