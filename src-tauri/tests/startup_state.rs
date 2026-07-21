use std::fs;

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, InventoryLocationKind, PlatformInfo, SkillMetadataStatus,
    SkillYardApplication, SupportedAppId, UiIntent, UiOutcome,
};
use tempfile::tempdir;

#[test]
fn new_database_starts_on_onboarding_without_reading_skill_roots() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");

    // 用普通文件占据扫描根；启动若误做 read_dir，这个测试会立即失败。
    fs::create_dir_all(home.join(".codex")).expect("应创建测试父目录");
    fs::write(home.join(".codex/skills"), "scan must not happen")
        .expect("应创建不可扫描的哨兵文件");

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );

    let outcome = application
        .handle(UiIntent::GetStartupState)
        .expect("首次启动应成功");

    assert_eq!(outcome, UiOutcome::onboarding_required());
    assert!(data_root.join("skillyard.sqlite3").is_file());
}

#[test]
fn storage_initialization_failure_is_returned_through_the_application_seam() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let data_root = sandbox.path().join("blocked-data-root");
    fs::write(&data_root, "not a directory").expect("应创建不可用的数据根目录");

    // 构造应用本身不能 panic；存储错误应在 UI intent 中成为结构化失败。
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, sandbox.path().join("home")),
        PlatformInfo::supported_for_test(),
    );

    assert!(application.handle(UiIntent::GetStartupState).is_err());
}

#[test]
fn empty_scan_is_persisted_and_reopened_without_creating_host_directories() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    fs::create_dir_all(&home).expect("应创建测试 home");

    let paths = ApplicationPaths::for_home(data_root, home.clone());
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());

    let scanned = application
        .handle(UiIntent::StartInitialScan)
        .expect("空目录扫描也应成功");

    let UiOutcome::Inventory { entries, .. } = &scanned else {
        panic!("扫描后应进入 Inventory");
    };
    assert!(entries.is_empty());
    assert!(!home.join(".codex/skills").exists());
    assert!(!home.join(".claude/skills").exists());
    assert!(!home.join(".copilot/skills").exists());
    assert!(!home.join(".agents/skills").exists());

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let restored = reopened
        .handle(UiIntent::GetStartupState)
        .expect("返回用户启动应读取保存结果");

    assert_eq!(restored, scanned);
}

#[test]
fn scan_discovers_only_fixed_global_and_shared_roots_without_modifying_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let fixtures = [
        (".codex/skills/codex-one", "codex-one"),
        (".claude/skills/claude-one", "claude-one"),
        (".copilot/skills/copilot-one", "copilot-one"),
        (".agents/skills/shared-one", "shared-one"),
        (".cursor/skills/cursor-only", "cursor-only"),
    ];

    for (relative_root, name) in fixtures {
        let root = home.join(relative_root);
        fs::create_dir_all(&root).expect("应创建 Skill fixture");
        fs::write(
            root.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test fixture\n---\n# {name}\n"),
        )
        .expect("应写入 Skill fixture");
    }
    let shared_before =
        fs::read(home.join(".agents/skills/shared-one/SKILL.md")).expect("应读取扫描前内容");

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home.clone()),
        PlatformInfo::supported_for_test(),
    );
    let outcome = application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    let UiOutcome::Inventory {
        entries,
        supported_apps,
        ..
    } = outcome
    else {
        panic!("扫描后应进入 Inventory");
    };
    assert_eq!(entries.len(), 4);
    assert!(
        entries
            .iter()
            .all(|entry| entry.skill_name != "cursor-only")
    );

    let shared = entries
        .iter()
        .find(|entry| entry.skill_name == "shared-one")
        .expect("应发现共享 Skill");
    assert_eq!(shared.location_kind, InventoryLocationKind::SharedReadOnly);
    assert_eq!(shared.metadata_status, SkillMetadataStatus::Valid);
    assert_eq!(
        shared.observed_by,
        vec![SupportedAppId::Codex, SupportedAppId::GitHubCopilot]
    );

    let codex = entries
        .iter()
        .find(|entry| entry.skill_name == "codex-one")
        .expect("应发现 Codex Skill");
    assert_eq!(codex.location_kind, InventoryLocationKind::AppGlobal);
    assert_eq!(codex.observed_by, vec![SupportedAppId::Codex]);
    assert_eq!(codex.declared_name.as_deref(), Some("codex-one"));

    assert!(supported_apps.iter().all(|app| app.detected == Some(true)));
    assert_eq!(
        fs::read(home.join(".agents/skills/shared-one/SKILL.md")).expect("应读取扫描后内容"),
        shared_before
    );
}

#[test]
fn scan_marks_invalid_frontmatter_without_trusting_the_declared_name() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let fixtures = [
        (
            "valid-skill",
            "---\nname: valid-skill\ndescription: Valid fixture\n---\n",
            SkillMetadataStatus::Valid,
        ),
        (
            "missing-description",
            "---\nname: missing-description\n---\n",
            SkillMetadataStatus::Invalid,
        ),
        (
            "invalid-name",
            "---\nname: Invalid_Name\ndescription: Invalid fixture\n---\n",
            SkillMetadataStatus::Invalid,
        ),
        (
            "actual-directory",
            "---\nname: another-name\ndescription: Mismatched fixture\n---\n",
            SkillMetadataStatus::Invalid,
        ),
    ];
    for (directory, contents, _) in &fixtures {
        let skill_root = home.join(".codex/skills").join(directory);
        fs::create_dir_all(&skill_root).expect("应创建 Skill fixture");
        fs::write(skill_root.join("SKILL.md"), contents).expect("应写入 Skill fixture");
    }

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    else {
        panic!("扫描后应进入 Inventory");
    };

    for (directory, _, expected_status) in fixtures {
        let entry = entries
            .iter()
            .find(|entry| entry.skill_root.ends_with(directory))
            .expect("应保留无效 Skill candidate");
        assert_eq!(entry.metadata_status, expected_status);
        if expected_status == SkillMetadataStatus::Invalid {
            assert_eq!(entry.skill_name, directory);
        }
    }
}

#[test]
fn unsupported_platform_returns_a_typed_state_before_scanning() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    fs::create_dir_all(home.join(".codex")).expect("应创建测试父目录");
    fs::write(home.join(".codex/skills"), "scan must not happen")
        .expect("应创建不可扫描的哨兵文件");

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo {
            os: "macos".to_owned(),
            architecture: "x86_64".to_owned(),
            major_version: 13,
        },
    );

    let outcome = application
        .handle(UiIntent::StartInitialScan)
        .expect("平台不支持应作为可呈现状态返回");

    assert_eq!(
        outcome,
        UiOutcome::UnsupportedPlatform {
            actual_os: "macos".to_owned(),
            actual_architecture: "x86_64".to_owned(),
            actual_major_version: 13,
            required_architecture: "aarch64".to_owned(),
            minimum_major_version: 14,
        }
    );
}

#[test]
fn returning_user_reads_persisted_inventory_without_rescanning() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/persisted-skill");
    fs::create_dir_all(&skill_root).expect("应创建 Skill fixture");
    fs::write(
        skill_root.join("SKILL.md"),
        "---\nname: persisted-skill\n---\n",
    )
    .expect("应写入 Skill fixture");

    let paths = ApplicationPaths::for_home(data_root, home.clone());
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let scanned = application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    // 把原扫描根变成不可扫描文件，返回启动若重扫就会报错。
    fs::remove_dir_all(home.join(".codex/skills")).expect("应移除测试扫描根");
    fs::write(home.join(".codex/skills"), "must not rescan").expect("应创建不可扫描的哨兵文件");

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    assert_eq!(
        reopened
            .handle(UiIntent::GetStartupState)
            .expect("返回启动应只读取持久化状态"),
        scanned
    );
}

#[test]
fn failed_scan_does_not_mark_onboarding_complete() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    fs::create_dir_all(home.join(".claude")).expect("应创建测试父目录");
    fs::write(home.join(".claude/skills"), "not a directory").expect("应创建非法扫描根");

    let paths = ApplicationPaths::for_home(data_root, home);
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    assert!(application.handle(UiIntent::StartInitialScan).is_err());

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    assert_eq!(
        reopened
            .handle(UiIntent::GetStartupState)
            .expect("失败后仍应显示首次介绍"),
        UiOutcome::onboarding_required()
    );
}

#[test]
fn inventory_and_completion_marker_commit_atomically() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".copilot/skills/atomic-skill");
    fs::create_dir_all(&skill_root).expect("应创建 Skill fixture");
    fs::write(
        skill_root.join("SKILL.md"),
        "---\nname: atomic-skill\n---\n",
    )
    .expect("应写入 Skill fixture");

    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    application
        .handle(UiIntent::GetStartupState)
        .expect("读取启动状态应创建正式 SQLite");
    let database = data_root.join("skillyard.sqlite3");
    let connection = Connection::open(&database).expect("应打开测试 SQLite");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_completion BEFORE UPDATE OF initial_scan_completed_at ON app_state BEGIN SELECT RAISE(ABORT, 'test failpoint'); END;",
        )
        .expect("应创建数据库 failpoint");

    assert!(application.handle(UiIntent::StartInitialScan).is_err());
    connection
        .execute_batch("DROP TRIGGER fail_completion;")
        .expect("应移除数据库 failpoint");

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    assert_eq!(
        reopened
            .handle(UiIntent::GetStartupState)
            .expect("事务失败后仍应显示首次介绍"),
        UiOutcome::onboarding_required()
    );
    let UiOutcome::Inventory { entries, .. } = reopened
        .handle(UiIntent::StartInitialScan)
        .expect("移除 failpoint 后应可重新扫描")
    else {
        panic!("重试后应进入 Inventory");
    };
    assert_eq!(entries.len(), 1);
}
