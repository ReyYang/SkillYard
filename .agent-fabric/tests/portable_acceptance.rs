use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TempProject {
    path: PathBuf,
}

impl TempProject {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agent-fabric-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        // 只清理本测试创建且带有唯一 nonce 的临时项目。
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn run(core: &Path, root: &Path, args: &[&str], path: Option<OsString>) -> Output {
    let mut command = Command::new(core);
    command.args(args).current_dir(root);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    command.output().unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn expect_success(output: Output, expected: &[&str]) -> String {
    let body = text(&output);
    assert!(output.status.success(), "命令失败：{body}");
    for value in expected {
        assert!(body.contains(value), "输出缺少 {value:?}：{body}");
    }
    body
}

fn expect_failure(output: Output, expected: &[&str]) -> String {
    let body = text(&output);
    assert!(!output.status.success(), "命令意外成功：{body}");
    for value in expected {
        assert!(body.contains(value), "输出缺少 {value:?}：{body}");
    }
    body
}

fn source() -> (PathBuf, PathBuf) {
    let root = PathBuf::from(std::env::var("AGENT_FABRIC_TEST_PROJECT_ROOT").unwrap());
    let core = root.join(".agent-fabric/bin/agent-fabric-core");
    assert!(core.is_file());
    (root, core)
}

fn copy_blueprint(source_root: &Path, target_root: &Path) {
    fs::copy(
        source_root.join("AGENT-FABRIC.md"),
        target_root.join("AGENT-FABRIC.md"),
    )
    .unwrap();
}

#[test]
fn blueprint_machine_data_only_restores_files() {
    let (source_root, _) = source();
    let blueprint = fs::read_to_string(source_root.join("AGENT-FABRIC.md")).unwrap();
    let start = blueprint
        .find("<!-- agent-fabric:machine:start -->")
        .unwrap();
    let end = blueprint
        .rfind("<!-- agent-fabric:machine:end -->")
        .unwrap();
    let machine = &blueprint[start..end];
    assert!(machine.contains("\"blueprint_schema\": 3"));
    assert!(!machine.contains("\"agents\":"));
    assert!(!machine.contains("\"routing\":"));
    assert!(!machine.contains("\"contract_schema\":"));
    assert!(blueprint.contains("只有用户明确调用 Agent Fabric Skill 时发生"));
    assert!(blueprint.contains("没有 `rustc`"));
    assert!(blueprint.contains("不是 Git 项目"));
}

#[cfg(unix)]
#[test]
fn init_without_rustc_restores_a_usable_framework() {
    let (source_root, source_core) = source();
    let fixture = TempProject::new("no-rust");
    let root = fixture.path.canonicalize().unwrap();
    copy_blueprint(&source_root, &root);
    let empty_path = root.join("empty-path");
    fs::create_dir(&empty_path).unwrap();

    // check 必须只读，且写入前所有项目都要求准确根确认。
    expect_success(
        run(
            &source_core,
            &root,
            &["fabric", "check", "--project-root", root.to_str().unwrap()],
            Some(empty_path.clone().into_os_string()),
        ),
        &["\"overall_status\": \"changes_planned\"", "\"ok\": true"],
    );
    assert!(!root.join(".agent-fabric").exists());
    expect_failure(
        run(
            &source_core,
            &root,
            &["fabric", "init", "--project-root", root.to_str().unwrap()],
            Some(empty_path.clone().into_os_string()),
        ),
        &["root_confirmation_required"],
    );
    assert!(!root.join(".agent-fabric").exists());

    expect_success(
        run(
            &source_core,
            &root,
            &[
                "fabric",
                "init",
                "--project-root",
                root.to_str().unwrap(),
                "--confirm-root",
                root.to_str().unwrap(),
            ],
            Some(empty_path.clone().into_os_string()),
        ),
        &[
            "\"framework_ready\": true",
            "\"status\": \"unavailable\"",
            "\"external_tools_executed\": false",
        ],
    );
    assert!(!root.join(".git").exists());
    assert!(!root.join(".agent-fabric/bin/agent-fabric-core").exists());
    assert!(!root
        .join(".agent-fabric/bin/.runtime-source.sha256")
        .exists());
    assert!(!root.join(".agent-fabric/.agent-fabric").exists());

    for relative in [
        ".agent-fabric/README.md",
        ".agent-fabric/skill/SKILL.md",
        ".agent-fabric/skill/references/collaborate.md",
        ".agent-fabric/skill/references/configure-collaborators.md",
        ".agent-fabric/roles/orchestrator.md",
        ".agent-fabric/roles/implementer.md",
        ".agent-fabric/roles/worker.md",
        ".agent-fabric/roles/researcher.md",
        ".agent-fabric/roles/reviewer.md",
        ".agent-fabric/contracts/CONTRACTS.md",
        ".agent-fabric/contracts/TASK.md",
        ".agent-fabric/contracts/RESULT.md",
        ".agent-fabric/contracts/REVIEW.md",
        ".agent-fabric/local/resolution.json",
        ".agent-fabric/state/managed.json",
    ] {
        assert!(root.join(relative).is_file(), "缺少 {relative}");
    }
    for obsolete in [
        ".agent-fabric/contracts/task.schema.json",
        ".agent-fabric/contracts/result.schema.json",
        ".agent-fabric/contracts/review.schema.json",
        ".agent-fabric/contracts/receipt.schema.json",
        ".agent-fabric/runs",
    ] {
        assert!(!root.join(obsolete).exists(), "不应存在 {obsolete}");
    }

    let skill = fs::read_to_string(root.join(".agent-fabric/skill/SKILL.md")).unwrap();
    assert!(skill.contains("只有用户明确调用"));
    assert!(skill.contains("references/collaborate.md"));
    assert!(skill.contains("references/configure-collaborators.md"));
    assert!(skill.contains("不调用 Rust、`agent-run` 或中央调度程序"));
    let contracts = fs::read_to_string(root.join(".agent-fabric/contracts/CONTRACTS.md")).unwrap();
    assert!(contracts.contains("不是机器强制协议"));
    assert!(contracts.contains("不要求固定标题、字段、顺序"));

    for role in [
        "orchestrator",
        "implementer",
        "worker",
        "researcher",
        "reviewer",
    ] {
        let role = fs::read_to_string(root.join(format!(".agent-fabric/roles/{role}.md"))).unwrap();
        for heading in [
            "## 我是谁",
            "## 我擅长什么",
            "## 何时使用",
            "## 如何参与",
            "## 通常提供什么",
            "## 不负责什么",
        ] {
            assert!(role.contains(heading), "Role 缺少 {heading}");
        }
    }
    let reviewer = fs::read_to_string(root.join(".agent-fabric/roles/reviewer.md")).unwrap();
    assert!(reviewer.contains("任何一项不满足，都只能称为“自审”"));

    expect_success(
        run(
            &source_core,
            &root,
            &["fabric", "verify", "--project-root", root.to_str().unwrap()],
            Some(empty_path.clone().into_os_string()),
        ),
        &[
            "\"framework_ready\": true",
            "\"maintenance_available\": false",
            "\"ok\": true",
        ],
    );
    expect_success(
        run(
            &source_core,
            &root,
            &[
                "fabric",
                "repair",
                "--project-root",
                root.to_str().unwrap(),
                "--confirm-root",
                root.to_str().unwrap(),
            ],
            Some(empty_path.into_os_string()),
        ),
        &["\"changed\": []", "\"ok\": true"],
    );
}

#[cfg(unix)]
#[test]
fn git_marker_does_not_replace_exact_root_confirmation() {
    let (source_root, source_core) = source();
    let fixture = TempProject::new("git-confirm");
    let root = fixture.path.canonicalize().unwrap();
    copy_blueprint(&source_root, &root);
    fs::create_dir(root.join(".git")).unwrap();
    expect_failure(
        run(
            &source_core,
            &root,
            &["fabric", "init", "--project-root", root.to_str().unwrap()],
            None,
        ),
        &["root_confirmation_required"],
    );
    assert!(!root.join(".agent-fabric").exists());
}

#[cfg(unix)]
#[test]
fn compatibility_probe_accepts_free_text_and_traces_failures() {
    let (source_root, source_core) = source();
    let fixture = TempProject::new("probe");
    let root = fixture.path.canonicalize().unwrap();
    copy_blueprint(&source_root, &root);
    expect_success(
        run(
            &source_core,
            &root,
            &[
                "fabric",
                "init",
                "--project-root",
                root.to_str().unwrap(),
                "--confirm-root",
                root.to_str().unwrap(),
            ],
            None,
        ),
        &["\"framework_ready\": true"],
    );

    let marker = root.join("candidate-executed.log");
    let executable = root.join(format!("local-collaborator-{}", std::process::id()));
    fs::write(
        &executable,
        concat!(
            "#!/bin/sh\n",
            "set -eu\n",
            "printf '%s\\n' \"${1:-none}\" >> candidate-executed.log\n",
            "case \"${1:-}\" in\n",
            "  inspect) printf '%s\\n' '说明文字' '```markdown' '能力可用' '```' ;;\n",
            "  fail) printf '%s\\n' '诊断输出'; printf '%s\\n' '探测失败' >&2; exit 23 ;;\n",
            "  *) exit 24 ;;\n",
            "esac\n"
        ),
    )
    .unwrap();
    make_executable(&executable);
    let adapter_dir = root.join(".agent-fabric/local/adapters");
    fs::create_dir_all(&adapter_dir).unwrap();
    let descriptor = adapter_dir.join("local-collaborator.json");
    fs::write(
        &descriptor,
        format!(
            "{{\n  \"command\": \"{}\",\n  \"name\": \"本机协作者\",\n  \"notes\": \"自由扩展字段\",\n  \"probe\": [\"inspect\"]\n}}\n",
            executable.display()
        ),
    )
    .unwrap();

    // 静态检查不运行本机命令。
    expect_success(
        run(
            &source_core,
            &root,
            &[
                "fabric",
                "connection-check",
                "--project-root",
                root.to_str().unwrap(),
                "--descriptor",
                descriptor.to_str().unwrap(),
            ],
            None,
        ),
        &["\"executed\": false", "ready_for_probe"],
    );
    assert!(!marker.exists());
    expect_failure(
        run(
            &source_core,
            &root,
            &[
                "agent-run",
                "probe",
                "--project-root",
                root.to_str().unwrap(),
                "--descriptor",
                descriptor.to_str().unwrap(),
                "--confirm-command",
                source_core.to_str().unwrap(),
            ],
            None,
        ),
        &["command_confirmation_mismatch"],
    );
    assert!(!marker.exists());

    let success = expect_success(
        run(
            &source_core,
            &root,
            &[
                "agent-run",
                "probe",
                "--project-root",
                root.to_str().unwrap(),
                "--descriptor",
                descriptor.to_str().unwrap(),
                "--confirm-command",
                executable.to_str().unwrap(),
            ],
            None,
        ),
        &[
            "probe_passed",
            "说明文字",
            "能力可用",
            "\"retained_path\": null",
        ],
    );
    assert!(!success.contains("parse_failure"));
    assert!(!root.join(".agent-fabric/local/traces").exists());

    fs::write(
        &descriptor,
        format!(
            "{{\n  \"command\": \"{}\",\n  \"name\": \"本机协作者\",\n  \"probe\": [\"fail\"]\n}}\n",
            executable.display()
        ),
    )
    .unwrap();
    expect_failure(
        run(
            &source_core,
            &root,
            &[
                "agent-run",
                "probe",
                "--project-root",
                root.to_str().unwrap(),
                "--descriptor",
                descriptor.to_str().unwrap(),
                "--confirm-command",
                executable.to_str().unwrap(),
            ],
            None,
        ),
        &["probe_failed", ".agent-fabric/local/traces/"],
    );
    let trace_root = root.join(".agent-fabric/local/traces");
    let traces: Vec<_> = fs::read_dir(&trace_root)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(traces.len(), 1);
    for file in ["request.md", "raw-output.txt", "stderr.txt", "receipt.json"] {
        assert!(traces[0].path().join(file).is_file(), "trace 缺少 {file}");
    }
    assert!(!root.join(".agent-fabric/runs").exists());

    // 日常 process 转发已从 Rust 接口删除。
    expect_failure(
        run(
            &source_core,
            &root,
            &[
                "agent-run",
                "process",
                "--project-root",
                root.to_str().unwrap(),
            ],
            None,
        ),
        &["不是日常协作入口"],
    );
}
