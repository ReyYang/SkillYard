use std::collections::BTreeSet;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, InventoryLocationKind, ManagementKind, PlatformInfo, ScanRootKey,
    SkillMetadataStatus, SkillYardApplication, SupportedAppId, UiIntent, UiOutcome,
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
    assert_eq!(codex.management_kind, ManagementKind::TakeoverCandidate);

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
fn same_name_in_different_roots_does_not_create_a_false_identity() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    for root in [".codex/skills/same-skill", ".claude/skills/same-skill"] {
        write_skill(&home.join(root), "same-skill", root);
    }
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(sandbox.path().join("data"), home),
        PlatformInfo::supported_for_test(),
    );

    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    else {
        panic!("扫描后应返回 Inventory");
    };
    let same_name = entries
        .iter()
        .filter(|entry| entry.skill_name == "same-skill")
        .collect::<Vec<_>>();
    assert_eq!(same_name.len(), 2);
    assert_ne!(same_name[0].id, same_name[1].id);
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

#[test]
fn local_refresh_reconciles_added_removed_and_changed_skills() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    for name in ["kept-skill", "changed-skill", "removed-skill"] {
        write_skill(&home.join(".codex/skills").join(name), name, "initial");
    }
    let paths = ApplicationPaths::for_home(data_root, home.clone());
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let initial = application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    fs::write(
        home.join(".codex/skills/changed-skill/script.txt"),
        "changed",
    )
    .expect("应修改 Skill 内普通文件");
    fs::remove_dir_all(home.join(".codex/skills/removed-skill")).expect("应移除一个 Skill");
    write_skill(
        &home.join(".codex/skills/added-skill"),
        "added-skill",
        "added",
    );

    let refreshed = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("用户主动刷新应成功");
    let UiOutcome::Inventory {
        entries,
        last_local_refresh: Some(summary),
        scan_issues,
        ..
    } = &refreshed
    else {
        panic!("刷新后应返回包含摘要的 Inventory");
    };
    assert_eq!((summary.added, summary.changed, summary.removed), (1, 1, 1));
    assert!(scan_issues.is_empty());
    assert!(
        entries
            .iter()
            .any(|entry| entry.skill_name == "added-skill")
    );
    assert!(entries.iter().any(|entry| entry.skill_name == "kept-skill"));
    assert!(
        !entries
            .iter()
            .any(|entry| entry.skill_name == "removed-skill")
    );

    let UiOutcome::Inventory {
        entries: initial_entries,
        ..
    } = initial
    else {
        panic!("首次扫描应返回 Inventory");
    };
    let old_fingerprint = initial_entries
        .iter()
        .find(|entry| entry.skill_name == "changed-skill")
        .expect("首次扫描应包含待修改 Skill")
        .observed_fingerprint
        .clone();
    let new_fingerprint = entries
        .iter()
        .find(|entry| entry.skill_name == "changed-skill")
        .expect("刷新后应保留已修改 Skill")
        .observed_fingerprint
        .clone();
    assert_ne!(new_fingerprint, old_fingerprint);

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    assert_eq!(
        reopened
            .handle(UiIntent::GetStartupState)
            .expect("重开后应读取刷新结果"),
        refreshed
    );
}

#[test]
fn local_refresh_preserves_the_existing_management_authority() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    write_skill(
        &home.join(".codex/skills/agent-owned"),
        "agent-owned",
        "initial",
    );
    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let application = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开测试 SQLite");
    connection
        .execute(
            "UPDATE inventory_observations SET management_kind = 'agent_managed' WHERE skill_name = 'agent-owned'",
            [],
        )
        .expect("应设置已有管理归属");
    drop(connection);

    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("本机刷新应成功")
    else {
        panic!("刷新后应返回 Inventory");
    };
    let entry = entries
        .iter()
        .find(|entry| entry.skill_name == "agent-owned")
        .expect("刷新后应保留 Skill");

    assert_eq!(entry.management_kind, ManagementKind::AgentManaged);
}

#[test]
fn local_refresh_is_rejected_before_initial_scan_without_reading_roots() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    fs::create_dir_all(home.join(".codex")).expect("应创建测试父目录");
    fs::write(home.join(".codex/skills"), "must not scan").expect("应创建不可扫描哨兵");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(sandbox.path().join("data"), home),
        PlatformInfo::supported_for_test(),
    );

    let error = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect_err("首次扫描前不能刷新本机");

    assert!(error.to_string().contains("完成首次扫描后才能刷新本机"));
}

#[test]
fn local_refresh_preserves_the_last_snapshot_for_a_failed_root() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    write_skill(
        &home.join(".codex/skills/codex-existing"),
        "codex-existing",
        "initial",
    );
    write_skill(
        &home.join(".claude/skills/claude-existing"),
        "claude-existing",
        "initial",
    );
    let paths = ApplicationPaths::for_home(data_root, home.clone());
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    fs::remove_dir_all(home.join(".codex/skills")).expect("应移除 Codex 扫描根");
    fs::write(home.join(".codex/skills"), "not a directory").expect("应创建失败扫描根");
    write_skill(
        &home.join(".claude/skills/claude-added"),
        "claude-added",
        "added",
    );

    let refreshed = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("部分失败应返回带告警的刷新结果");
    let UiOutcome::Inventory {
        entries,
        last_local_refresh: Some(summary),
        scan_issues,
        ..
    } = &refreshed
    else {
        panic!("刷新后应返回 Inventory");
    };

    let preserved = entries
        .iter()
        .find(|entry| entry.skill_name == "codex-existing")
        .expect("失败根应保留上次成功观察");
    assert!(preserved.stale);
    assert!(
        entries
            .iter()
            .any(|entry| entry.skill_name == "claude-added")
    );
    assert_eq!((summary.added, summary.changed, summary.removed), (1, 0, 0));
    assert_eq!(scan_issues.len(), 1);
    assert_eq!(scan_issues[0].root_key, ScanRootKey::CodexGlobal);

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    assert_eq!(
        reopened
            .handle(UiIntent::GetStartupState)
            .expect("重开后应读取带告警的刷新结果"),
        refreshed
    );
}

#[cfg(unix)]
#[test]
fn local_refresh_treats_a_dangling_root_symlink_as_a_scan_failure() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    write_skill(
        &home.join(".codex/skills/codex-existing"),
        "codex-existing",
        "initial",
    );
    let paths = ApplicationPaths::for_home(data_root, home.clone());
    let application = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    fs::remove_dir_all(home.join(".codex/skills")).expect("应移除原扫描根");
    // 断链不是空目录，必须保留上次观察，避免把暂时不可读误判成用户删除。
    symlink(
        home.join("missing-codex-skills"),
        home.join(".codex/skills"),
    )
    .expect("应创建断链扫描根");

    let UiOutcome::Inventory {
        entries,
        last_local_refresh: Some(summary),
        scan_issues,
        ..
    } = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("断链应作为局部扫描告警返回")
    else {
        panic!("刷新后应返回 Inventory");
    };

    let preserved = entries
        .iter()
        .find(|entry| entry.skill_name == "codex-existing")
        .expect("断链根应保留上次成功观察");
    assert!(preserved.stale);
    assert_eq!(summary.removed, 0);
    assert_eq!(scan_issues.len(), 1);
    assert_eq!(scan_issues[0].root_key, ScanRootKey::CodexGlobal);
}

#[cfg(unix)]
#[test]
fn local_refresh_preserves_a_skill_when_its_metadata_becomes_unreadable() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/codex-existing");
    write_skill(&skill_root, "codex-existing", "initial");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    fs::remove_file(skill_root.join("SKILL.md")).expect("应移除原 metadata");
    symlink("SKILL.md", skill_root.join("SKILL.md")).expect("应创建不可解析的 metadata 链接");

    let UiOutcome::Inventory {
        entries,
        last_local_refresh: Some(summary),
        scan_issues,
        ..
    } = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("不可读 metadata 应成为局部扫描告警")
    else {
        panic!("刷新后应返回 Inventory");
    };

    assert!(
        entries
            .iter()
            .any(|entry| { entry.skill_name == "codex-existing" && entry.stale })
    );
    assert_eq!(summary.removed, 0);
    assert_eq!(scan_issues.len(), 1);
    assert_eq!(scan_issues[0].root_key, ScanRootKey::CodexGlobal);
}

#[cfg(unix)]
#[test]
fn local_refresh_preserves_a_root_when_its_detection_directory_is_dangling() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    write_skill(
        &home.join(".codex/skills/codex-existing"),
        "codex-existing",
        "initial",
    );
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home.clone()),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    fs::remove_dir_all(home.join(".codex")).expect("应移除原应用目录");
    symlink(home.join("missing-codex"), home.join(".codex")).expect("应创建断链应用目录");

    let UiOutcome::Inventory {
        entries,
        last_local_refresh: Some(summary),
        scan_issues,
        ..
    } = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("断链应用目录应成为局部扫描告警")
    else {
        panic!("刷新后应返回 Inventory");
    };

    assert!(
        entries
            .iter()
            .any(|entry| { entry.skill_name == "codex-existing" && entry.stale })
    );
    assert_eq!(summary.removed, 0);
    assert_eq!(scan_issues.len(), 1);
    assert_eq!(scan_issues[0].root_key, ScanRootKey::CodexGlobal);
}

#[cfg(unix)]
#[test]
fn local_refresh_preserves_a_mounted_skill_when_its_target_disappears() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let source = sandbox.path().join("source/codex-existing");
    let mounted = home.join(".codex/skills/codex-existing");
    write_skill(&source, "codex-existing", "initial");
    fs::create_dir_all(mounted.parent().expect("Mount 应有父目录")).expect("应创建扫描根");
    symlink(&source, &mounted).expect("应创建已有 Skill Mount");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    fs::remove_dir_all(&source).expect("应模拟 Mount 目标暂时消失");

    let UiOutcome::Inventory {
        entries,
        last_local_refresh: Some(summary),
        scan_issues,
        ..
    } = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("断链 Skill Mount 应成为局部扫描告警")
    else {
        panic!("刷新后应返回 Inventory");
    };

    assert!(
        entries
            .iter()
            .any(|entry| { entry.skill_name == "codex-existing" && entry.stale })
    );
    assert_eq!(summary.removed, 0);
    assert_eq!(scan_issues.len(), 1);
    assert_eq!(scan_issues[0].root_key, ScanRootKey::CodexGlobal);
}

#[test]
fn local_refresh_database_failure_keeps_the_previous_snapshot() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    write_skill(
        &home.join(".copilot/skills/existing-skill"),
        "existing-skill",
        "initial",
    );
    let paths = ApplicationPaths::for_home(data_root.clone(), home.clone());
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let initial = application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    write_skill(
        &home.join(".copilot/skills/added-skill"),
        "added-skill",
        "added",
    );

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开测试 SQLite");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_local_refresh BEFORE UPDATE OF last_local_refresh_at ON app_state BEGIN SELECT RAISE(ABORT, 'test failpoint'); END;",
        )
        .expect("应创建刷新 failpoint");
    assert!(application.handle(UiIntent::RefreshLocalInventory).is_err());
    connection
        .execute_batch("DROP TRIGGER fail_local_refresh;")
        .expect("应移除刷新 failpoint");

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    assert_eq!(
        reopened
            .handle(UiIntent::GetStartupState)
            .expect("失败后应读取旧快照"),
        initial
    );
}

#[test]
fn registering_a_project_immediately_scans_every_existing_supported_project_root() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    fs::create_dir(&home).expect("应创建测试 home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应先完成首次扫描");

    let project = sandbox.path().join("sample-project");
    fs::create_dir(&project).expect("应创建 Project");
    write_skill(
        &project.join(".codex/skills/codex-project"),
        "codex-project",
        "codex",
    );
    write_skill(
        &project.join(".claude/skills/claude-project"),
        "claude-project",
        "claude",
    );
    write_skill(
        &project.join(".github/skills/copilot-project"),
        "copilot-project",
        "copilot",
    );
    write_skill(
        &project.join(".agents/skills/shared-project"),
        "shared-project",
        "shared",
    );

    let UiOutcome::Inventory {
        entries, projects, ..
    } = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("登记 Project 后应立即只读扫描")
    else {
        panic!("应返回 Inventory");
    };
    let project_id = &projects[0].id;
    let project_entries = entries
        .iter()
        .filter(|entry| entry.project_id.as_ref() == Some(project_id))
        .collect::<Vec<_>>();
    assert_eq!(project_entries.len(), 4);
    for entry in &project_entries {
        assert_eq!(
            entry.project_display_name.as_deref(),
            Some("sample-project")
        );
        assert_eq!(entry.management_kind, ManagementKind::TakeoverCandidate);
    }

    let codex = project_entries
        .iter()
        .find(|entry| entry.skill_name == "codex-project")
        .unwrap();
    assert_eq!(codex.location_kind, InventoryLocationKind::AppProject);
    assert_eq!(codex.root_key, Some(ScanRootKey::CodexProject));
    assert_eq!(codex.observed_by, vec![SupportedAppId::Codex]);
    let claude = project_entries
        .iter()
        .find(|entry| entry.skill_name == "claude-project")
        .unwrap();
    assert_eq!(claude.root_key, Some(ScanRootKey::ClaudeCodeProject));
    assert_eq!(
        claude.observed_by,
        vec![SupportedAppId::ClaudeCode, SupportedAppId::GitHubCopilot]
    );
    let copilot = project_entries
        .iter()
        .find(|entry| entry.skill_name == "copilot-project")
        .unwrap();
    assert_eq!(copilot.root_key, Some(ScanRootKey::GitHubCopilotProject));
    assert_eq!(copilot.observed_by, vec![SupportedAppId::GitHubCopilot]);
    let shared = project_entries
        .iter()
        .find(|entry| entry.skill_name == "shared-project")
        .unwrap();
    assert_eq!(shared.location_kind, InventoryLocationKind::SharedReadOnly);
    assert_eq!(shared.root_key, Some(ScanRootKey::SharedAgentsProject));
    assert_eq!(
        shared.observed_by,
        vec![SupportedAppId::Codex, SupportedAppId::GitHubCopilot]
    );
}

#[test]
fn project_registration_rolls_back_when_its_initial_inventory_cannot_be_saved() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    fs::create_dir(&home).expect("应创建测试 home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应先完成首次扫描");
    let project = sandbox.path().join("atomic-project");
    write_skill(&project.join(".codex/skills/alpha"), "alpha", "alpha");

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开测试 SQLite");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_project_inventory
             BEFORE INSERT ON inventory_observations
             WHEN NEW.project_id IS NOT NULL
             BEGIN SELECT RAISE(ABORT, 'test project scan failure'); END;",
        )
        .expect("应创建 Project 扫描失败点");

    assert!(
        application
            .handle(UiIntent::RegisterProject {
                root_path: project.to_string_lossy().into_owned(),
            })
            .is_err()
    );
    let project_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .expect("应读取 Project 数量");
    assert_eq!(project_count, 0, "扫描保存失败时 Project 记录也必须回退");
}

#[cfg(unix)]
#[test]
fn local_refresh_isolates_a_failed_root_to_the_correct_registered_project() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    fs::create_dir(&home).expect("应创建测试 home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应先完成首次扫描");

    let project_a = sandbox.path().join("project-a");
    let project_b = sandbox.path().join("project-b");
    write_skill(&project_a.join(".codex/skills/alpha"), "alpha", "alpha");
    write_skill(&project_b.join(".codex/skills/bravo"), "bravo", "bravo");
    let registered_a = application
        .handle(UiIntent::RegisterProject {
            root_path: project_a.to_string_lossy().into_owned(),
        })
        .expect("应登记 Project A");
    let project_a_id = match registered_a {
        UiOutcome::Inventory { projects, .. } => projects[0].id.clone(),
        _ => panic!("应返回 Inventory"),
    };
    let registered_b = application
        .handle(UiIntent::RegisterProject {
            root_path: project_b.to_string_lossy().into_owned(),
        })
        .expect("应登记 Project B");
    let project_b_id = match registered_b {
        UiOutcome::Inventory { projects, .. } => projects
            .iter()
            .find(|project| project.display_name == "project-b")
            .unwrap()
            .id
            .clone(),
        _ => panic!("应返回 Inventory"),
    };

    fs::remove_dir_all(project_a.join(".codex/skills")).expect("应移除 Project A 扫描根");
    symlink(
        project_a.join("missing-skills"),
        project_a.join(".codex/skills"),
    )
    .expect("应创建 Project A 失败根");
    fs::remove_dir_all(project_b.join(".codex/skills/bravo")).expect("应移除旧 Skill");
    write_skill(
        &project_b.join(".codex/skills/charlie"),
        "charlie",
        "charlie",
    );

    let UiOutcome::Inventory {
        entries,
        scan_issues,
        ..
    } = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("一个 Project 根失败不能阻止其他 Project 刷新")
    else {
        panic!("应返回 Inventory");
    };
    let alpha = entries
        .iter()
        .find(|entry| entry.skill_name == "alpha")
        .expect("失败根应保留上次结果");
    assert_eq!(alpha.project_id.as_deref(), Some(project_a_id.as_str()));
    assert!(alpha.stale);
    assert!(entries.iter().all(|entry| entry.skill_name != "bravo"));
    let charlie = entries
        .iter()
        .find(|entry| entry.skill_name == "charlie")
        .expect("其他 Project 应保存新结果");
    assert_eq!(charlie.project_id.as_deref(), Some(project_b_id.as_str()));
    assert_eq!(scan_issues.len(), 1);
    assert_eq!(scan_issues[0].root_key, ScanRootKey::CodexProject);
    assert_eq!(
        scan_issues[0].project_id.as_deref(),
        Some(project_a_id.as_str())
    );
}

#[test]
fn returning_user_reads_saved_project_inventory_until_refresh_is_requested() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    fs::create_dir(&home).expect("应创建测试 home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let paths = ApplicationPaths::for_home(data_root, home);
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应先完成首次扫描");
    let project = sandbox.path().join("persisted-project");
    write_skill(&project.join(".codex/skills/alpha"), "alpha", "alpha");
    application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记并扫描 Project");
    write_skill(&project.join(".codex/skills/beta"), "beta", "beta");

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let startup = reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启应读取已保存清单");
    let UiOutcome::Inventory {
        entries: startup_entries,
        ..
    } = startup
    else {
        panic!("启动应返回 Inventory");
    };
    assert!(
        startup_entries
            .iter()
            .any(|entry| entry.skill_name == "alpha")
    );
    assert!(
        startup_entries
            .iter()
            .all(|entry| entry.skill_name != "beta"),
        "启动不能擅自扫描 Project"
    );

    let refreshed = reopened
        .handle(UiIntent::RefreshLocalInventory)
        .expect("主动刷新后应读取 Project 变化");
    let UiOutcome::Inventory {
        entries: refreshed_entries,
        ..
    } = refreshed
    else {
        panic!("刷新应返回 Inventory");
    };
    assert!(
        refreshed_entries
            .iter()
            .any(|entry| entry.skill_name == "beta")
    );
}

#[cfg(unix)]
#[test]
fn scan_issues_for_the_same_root_are_preserved_for_each_project() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    fs::create_dir(&home).expect("应创建测试 home");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(sandbox.path().join("data"), home),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应先完成首次扫描");

    let mut latest = None;
    for name in ["broken-project-a", "broken-project-b"] {
        let project = sandbox.path().join(name);
        fs::create_dir_all(project.join(".codex")).expect("应创建 Project 父目录");
        symlink(
            project.join("missing-skills"),
            project.join(".codex/skills"),
        )
        .expect("应创建断链扫描根");
        latest = Some(
            application
                .handle(UiIntent::RegisterProject {
                    root_path: project.to_string_lossy().into_owned(),
                })
                .expect("扫描问题不能阻止 Project 登记"),
        );
    }

    let UiOutcome::Inventory { scan_issues, .. } = latest.expect("应有最终清单") else {
        panic!("应返回 Inventory");
    };
    let project_issues = scan_issues
        .iter()
        .filter(|issue| issue.root_key == ScanRootKey::CodexProject)
        .collect::<Vec<_>>();
    assert_eq!(project_issues.len(), 2);
    assert_ne!(project_issues[0].root_id, project_issues[1].root_id);
    assert_ne!(project_issues[0].project_id, project_issues[1].project_id);
}

#[test]
fn committed_project_skill_is_project_managed_and_evidence_survives_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, paths) = started_application(&sandbox);
    let project = sandbox.path().join("tracked-project");
    init_git_repository(&project);
    write_skill(&project.join(".codex/skills/alpha"), "alpha", "tracked");
    commit_all(&project, "track alpha");
    let head = git_stdout(&project, &["rev-parse", "HEAD"]);

    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("登记 Project 应成功");
    let alpha = inventory_entry(&registered, "alpha");
    assert_eq!(alpha.management_kind, ManagementKind::ProjectManaged);
    let evidence = alpha
        .management_evidence
        .as_ref()
        .expect("HEAD 中的普通 blob 应产生管理证据");
    assert_eq!(
        evidence.authority_root,
        fs::canonicalize(&project)
            .expect("应解析 Git authority root")
            .to_string_lossy()
    );
    assert_eq!(evidence.snapshot_commit_oid, head);
    assert_eq!(evidence.subject_path, ".codex/skills/alpha/SKILL.md");

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let restored = reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启应读取已保存证据");
    assert_eq!(
        inventory_entry(&restored, "alpha").management_evidence,
        alpha.management_evidence
    );
}

#[test]
fn untracked_and_staged_only_project_skills_remain_takeover_candidates() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _) = started_application(&sandbox);
    let project = sandbox.path().join("candidate-project");
    init_git_repository(&project);
    fs::write(project.join("README.md"), "fixture\n").expect("应写入初始 Git 文件");
    commit_all(&project, "create initial head");
    write_skill(
        &project.join(".codex/skills/untracked"),
        "untracked",
        "untracked",
    );
    write_skill(&project.join(".codex/skills/staged"), "staged", "staged");
    run_git(&project, &["add", ".codex/skills/staged/SKILL.md"]);

    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("登记 Project 应成功");
    for name in ["untracked", "staged"] {
        let entry = inventory_entry(&registered, name);
        assert_eq!(entry.management_kind, ManagementKind::TakeoverCandidate);
        assert!(entry.management_evidence.is_none());
    }
}

#[test]
fn dirty_tracked_skill_stays_project_managed_because_only_head_is_authoritative() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _) = started_application(&sandbox);
    let project = sandbox.path().join("dirty-project");
    init_git_repository(&project);
    write_skill(&project.join(".codex/skills/alpha"), "alpha", "committed");
    commit_all(&project, "track alpha");
    fs::write(
        project.join(".codex/skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: Dirty working tree\n---\n",
    )
    .expect("应修改工作区内容");

    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("登记 Project 应成功");
    assert_eq!(
        inventory_entry(&registered, "alpha").management_kind,
        ManagementKind::ProjectManaged
    );
}

#[test]
fn project_management_follows_head_when_skill_is_committed_or_removed() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _) = started_application(&sandbox);
    let project = sandbox.path().join("changing-project");
    init_git_repository(&project);
    write_skill(&project.join(".codex/skills/alpha"), "alpha", "initial");

    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("登记 Project 应成功");
    assert_eq!(
        inventory_entry(&registered, "alpha").management_kind,
        ManagementKind::TakeoverCandidate
    );

    commit_all(&project, "track alpha");
    let upgraded = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("提交后刷新应成功");
    assert_eq!(
        inventory_entry(&upgraded, "alpha").management_kind,
        ManagementKind::ProjectManaged
    );

    run_git(
        &project,
        &["rm", "--cached", ".codex/skills/alpha/SKILL.md"],
    );
    run_git(&project, &["commit", "-m", "stop tracking alpha"]);
    let downgraded = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("HEAD 删除后刷新应成功");
    let alpha = inventory_entry(&downgraded, "alpha");
    assert_eq!(alpha.management_kind, ManagementKind::TakeoverCandidate);
    assert!(alpha.management_evidence.is_none());
}

#[test]
fn replacement_refs_cannot_substitute_a_different_tree_for_head_evidence() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _) = started_application(&sandbox);
    let project = sandbox.path().join("replace-ref-project");
    init_git_repository(&project);
    write_skill(&project.join(".codex/skills/alpha"), "alpha", "tracked");
    commit_all(&project, "track alpha");
    let commit_with_skill = git_stdout(&project, &["rev-parse", "HEAD"]);
    run_git(
        &project,
        &["rm", "--cached", ".codex/skills/alpha/SKILL.md"],
    );
    run_git(&project, &["commit", "-m", "remove alpha from head"]);
    let original_head = git_stdout(&project, &["rev-parse", "HEAD"]);
    run_git(&project, &["replace", &original_head, &commit_with_skill]);
    assert!(
        !git_stdout(
            &project,
            &["ls-tree", "HEAD", ".codex/skills/alpha/SKILL.md"]
        )
        .is_empty(),
        "测试 fixture 必须证明默认 Git 会应用 replacement ref"
    );

    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("登记 Project 应成功");
    let alpha = inventory_entry(&registered, "alpha");
    assert_eq!(alpha.management_kind, ManagementKind::TakeoverCandidate);
    assert!(alpha.management_evidence.is_none());
}

#[cfg(unix)]
#[test]
fn tracked_symlink_skill_file_is_not_project_management_evidence() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _) = started_application(&sandbox);
    let project = sandbox.path().join("symlink-project");
    init_git_repository(&project);
    let skill_root = project.join(".codex/skills/linked");
    fs::create_dir_all(&skill_root).expect("应创建 Skill 目录");
    fs::write(
        skill_root.join("actual.md"),
        "---\nname: linked\ndescription: Linked fixture\n---\n",
    )
    .expect("应写入链接目标");
    symlink("actual.md", skill_root.join("SKILL.md")).expect("应创建 SKILL.md 软链接");
    commit_all(&project, "track symlink");

    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("登记 Project 应成功");
    let linked = inventory_entry(&registered, "linked");
    assert_eq!(linked.management_kind, ManagementKind::TakeoverCandidate);
    assert!(linked.management_evidence.is_none());
}

#[test]
fn tracked_submodule_skill_is_not_project_management_evidence() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _) = started_application(&sandbox);
    let project = sandbox.path().join("submodule-project");
    init_git_repository(&project);
    let skill_root = project.join(".codex/skills/nested");
    init_git_repository(&skill_root);
    write_skill(&skill_root, "nested", "nested");
    commit_all(&skill_root, "track nested skill");
    commit_all(&project, "track nested repository as gitlink");
    assert!(
        git_stdout(&project, &["ls-tree", "HEAD", ".codex/skills/nested"])
            .starts_with("160000 commit "),
        "测试 fixture 必须让外层 Project 只追踪 gitlink"
    );

    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("登记 Project 应成功");
    let nested = inventory_entry(&registered, "nested");
    assert_eq!(nested.management_kind, ManagementKind::TakeoverCandidate);
    assert!(nested.management_evidence.is_none());
}

#[test]
fn project_inside_monorepo_and_git_file_worktree_use_their_actual_authority_root() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _) = started_application(&sandbox);
    let monorepo = sandbox.path().join("monorepo");
    init_git_repository(&monorepo);
    let project = monorepo.join("packages/product");
    write_skill(&project.join(".codex/skills/alpha"), "alpha", "alpha");
    commit_all(&monorepo, "track monorepo skill");

    let monorepo_outcome = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记 monorepo 子目录");
    let alpha = inventory_entry(&monorepo_outcome, "alpha");
    assert_eq!(alpha.management_kind, ManagementKind::ProjectManaged);
    assert_eq!(
        alpha
            .management_evidence
            .as_ref()
            .expect("应保存 monorepo 证据")
            .authority_root,
        fs::canonicalize(&monorepo)
            .expect("应解析 monorepo authority root")
            .to_string_lossy()
    );

    let linked_worktree = sandbox.path().join("linked-worktree");
    run_git(
        &monorepo,
        &[
            "worktree",
            "add",
            "-b",
            "worktree-test",
            linked_worktree.to_str().expect("测试路径应为 UTF-8"),
        ],
    );
    write_skill(
        &linked_worktree.join(".codex/skills/bravo"),
        "bravo",
        "bravo",
    );
    commit_all(&linked_worktree, "track worktree skill");
    assert!(linked_worktree.join(".git").is_file());

    let worktree_outcome = application
        .handle(UiIntent::RegisterProject {
            root_path: linked_worktree.to_string_lossy().into_owned(),
        })
        .expect("应登记 .git file worktree");
    let bravo = inventory_entry(&worktree_outcome, "bravo");
    assert_eq!(bravo.management_kind, ManagementKind::ProjectManaged);
    assert_eq!(
        bravo
            .management_evidence
            .as_ref()
            .expect("应保存 worktree 证据")
            .authority_root,
        fs::canonicalize(&linked_worktree)
            .expect("应解析 worktree authority root")
            .to_string_lossy()
    );
}

#[test]
fn broken_git_evidence_keeps_the_last_project_snapshot_stale() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, paths) = started_application(&sandbox);
    let project = sandbox.path().join("broken-git-project");
    init_git_repository(&project);
    write_skill(&project.join(".codex/skills/alpha"), "alpha", "alpha");
    commit_all(&project, "track alpha");
    let initial = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("登记 Project 应成功");
    let old_evidence = inventory_entry(&initial, "alpha")
        .management_evidence
        .clone();

    fs::rename(project.join(".git"), project.join(".git-valid")).expect("应暂存有效 Git 元数据");
    fs::write(project.join(".git"), "gitdir: missing-git-directory\n")
        .expect("应写入损坏的 .git file");
    let refreshed = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("证据不确定时应保留旧快照而不是让整次刷新失败");
    let alpha = inventory_entry(&refreshed, "alpha");
    assert!(alpha.stale);
    assert_eq!(alpha.management_kind, ManagementKind::ProjectManaged);
    assert_eq!(alpha.management_evidence, old_evidence);
    let UiOutcome::Inventory { scan_issues, .. } = &refreshed else {
        panic!("刷新后应返回 Inventory");
    };
    assert!(scan_issues.iter().any(|issue| {
        issue.root_key == ScanRootKey::CodexProject
            && issue.code == skillyard_lib::ScanIssueCode::InspectManagementEvidence
    }));

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    assert_eq!(
        inventory_entry(
            &reopened
                .handle(UiIntent::GetStartupState)
                .expect("重启应读取保留的旧快照"),
            "alpha"
        )
        .management_evidence,
        old_evidence
    );
}

#[test]
fn broken_head_object_is_indeterminate_instead_of_an_unborn_repository() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _) = started_application(&sandbox);
    let project = sandbox.path().join("broken-head-project");
    init_git_repository(&project);
    write_skill(&project.join(".codex/skills/alpha"), "alpha", "alpha");
    commit_all(&project, "track alpha");
    let initial = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("登记 Project 应成功");
    let old_evidence = inventory_entry(&initial, "alpha")
        .management_evidence
        .clone();
    let head_reference = git_stdout(&project, &["symbolic-ref", "HEAD"]);
    fs::write(
        project.join(".git").join(head_reference),
        "0000000000000000000000000000000000000000\n",
    )
    .expect("应损坏 HEAD 指向的对象引用");

    let refreshed = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("损坏对象应成为局部扫描问题");
    let alpha = inventory_entry(&refreshed, "alpha");
    assert!(alpha.stale);
    assert_eq!(alpha.management_evidence, old_evidence);
    let UiOutcome::Inventory { scan_issues, .. } = refreshed else {
        panic!("刷新后应返回 Inventory");
    };
    assert!(scan_issues.iter().any(|issue| {
        issue.root_key == ScanRootKey::CodexProject
            && issue.code == skillyard_lib::ScanIssueCode::InspectManagementEvidence
    }));
}

#[test]
fn partial_clone_missing_tree_does_not_lazy_fetch_during_local_refresh() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _) = started_application(&sandbox);
    let origin = sandbox.path().join("partial-origin");
    init_git_repository(&origin);
    run_git(&origin, &["config", "uploadpack.allowFilter", "true"]);
    write_skill(&origin.join(".codex/skills/alpha"), "alpha", "alpha");
    commit_all(&origin, "track alpha");

    let project = sandbox.path().join("partial-project");
    clone_tree_filtered(&origin, &project);
    let initial_pack_files = git_pack_files(&project);
    assert!(!initial_pack_files.is_empty(), "过滤 clone 应产生初始 pack");
    run_git(&project, &["checkout", "--force", "HEAD"]);
    let hydrated_pack_files = git_pack_files(&project);
    assert!(
        hydrated_pack_files.len() > initial_pack_files.len(),
        "checkout 应通过 promisor remote 补齐工作区对象"
    );

    let initial = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("对象完整时应登记 Project");
    assert_eq!(
        inventory_entry(&initial, "alpha").management_kind,
        ManagementKind::ProjectManaged
    );

    // 只移除 checkout 后懒加载的 pack，保留含 HEAD commit 的初始 promisor pack。
    for path in hydrated_pack_files.difference(&initial_pack_files) {
        fs::remove_file(path).expect("应移除测试用的已补齐对象 pack");
    }
    let missing_object_pack_files = git_pack_files(&project);
    assert_eq!(missing_object_pack_files, initial_pack_files);
    assert!(
        !git_command_with_no_lazy_fetch(
            &project,
            &["ls-tree", "HEAD", ".codex/skills/alpha/SKILL.md"]
        )
        .status
        .success(),
        "fixture 必须在禁用 lazy fetch 时缺少 tree object"
    );

    let refreshed = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("缺失承诺对象应成为局部扫描问题");
    let alpha = inventory_entry(&refreshed, "alpha");
    assert!(alpha.stale);
    assert_eq!(alpha.management_kind, ManagementKind::ProjectManaged);
    let UiOutcome::Inventory { scan_issues, .. } = refreshed else {
        panic!("刷新后应返回 Inventory");
    };
    assert!(scan_issues.iter().any(|issue| {
        issue.root_key == ScanRootKey::CodexProject
            && issue.code == skillyard_lib::ScanIssueCode::InspectManagementEvidence
    }));
    assert_eq!(
        git_pack_files(&project),
        missing_object_pack_files,
        "Local Refresh 不能从 promisor remote 新增对象 pack"
    );
}

#[cfg(unix)]
#[test]
fn symlink_git_marker_is_indeterminate_and_does_not_create_management_evidence() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _) = started_application(&sandbox);
    let project = sandbox.path().join("linked-git-marker-project");
    init_git_repository(&project);
    write_skill(&project.join(".codex/skills/alpha"), "alpha", "alpha");
    commit_all(&project, "track alpha");
    fs::rename(project.join(".git"), project.join(".git-real")).expect("应移动 Git 元数据目录");
    symlink(".git-real", project.join(".git")).expect("应创建 .git 软链接");

    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("证据问题不应阻止登记 Project");
    let UiOutcome::Inventory {
        entries,
        scan_issues,
        ..
    } = registered
    else {
        panic!("登记后应返回 Inventory");
    };
    assert!(entries.iter().all(|entry| entry.skill_name != "alpha"));
    assert!(scan_issues.iter().any(|issue| {
        issue.root_key == ScanRootKey::CodexProject
            && issue.code == skillyard_lib::ScanIssueCode::InspectManagementEvidence
    }));
}

#[test]
fn global_skill_in_a_git_clone_is_not_classified_as_project_managed() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("global-repository");
    init_git_repository(&home);
    write_skill(&home.join(".codex/skills/alpha"), "alpha", "alpha");
    commit_all(&home, "track global skill");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(sandbox.path().join("data"), home),
        PlatformInfo::supported_for_test(),
    );

    let scanned = application
        .handle(UiIntent::StartInitialScan)
        .expect("全局扫描应成功");
    let alpha = inventory_entry(&scanned, "alpha");
    assert_eq!(alpha.management_kind, ManagementKind::TakeoverCandidate);
    assert!(alpha.management_evidence.is_none());
}

#[test]
fn version_one_database_migrates_without_losing_inventory() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let data_root = sandbox.path().join("application-support/SkillYard");
    fs::create_dir_all(&data_root).expect("应创建测试数据目录");
    let database = data_root.join("skillyard.sqlite3");
    let connection = Connection::open(&database).expect("应创建 v1 SQLite");
    connection
        .execute_batch(include_str!("../migrations/0001_initial.sql"))
        .expect("应执行 v1 migration");
    connection
        .execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (1, 1)",
            [],
        )
        .expect("应记录 v1 migration");
    connection
        .execute(
            "UPDATE app_state SET initial_scan_completed_at = 100 WHERE singleton = 1",
            [],
        )
        .expect("应写入 v1 首次扫描状态");
    connection
        .execute(
            "INSERT INTO supported_app_status (app_id, display_name, detected, sort_order) VALUES ('codex', 'Codex', 1, 0)",
            [],
        )
        .expect("应写入 v1 App 状态");
    connection
        .execute(
            "INSERT INTO inventory_observations (id, skill_name, declared_name, skill_root, skill_file, location_kind, metadata_status) VALUES ('old', 'old-skill', 'old-skill', '/tmp/.codex/skills/old-skill', '/tmp/.codex/skills/old-skill/SKILL.md', 'app_global', 'valid')",
            [],
        )
        .expect("应写入 v1 observation");
    connection
        .execute(
            "INSERT INTO inventory_observation_apps (observation_id, app_id) VALUES ('old', 'codex')",
            [],
        )
        .expect("应写入 v1 observation App");
    drop(connection);

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, sandbox.path().join("home")),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        entries,
        last_local_refresh,
        ..
    } = application
        .handle(UiIntent::GetStartupState)
        .expect("v1 数据库应自动迁移")
    else {
        panic!("迁移后应返回 Inventory");
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].skill_name, "old-skill");
    assert_eq!(entries[0].root_key, Some(ScanRootKey::CodexGlobal));
    assert_eq!(
        entries[0].management_kind,
        ManagementKind::TakeoverCandidate
    );
    assert!(last_local_refresh.is_none());

    let connection = Connection::open(database).expect("应重开迁移后的 SQLite");
    let versions: String = connection
        .query_row(
            "SELECT group_concat(version, ',') FROM schema_migrations ORDER BY version",
            [],
            |row| row.get(0),
        )
        .expect("应读取 migration 版本");
    assert_eq!(versions, "1,2,3,4,5,6,7,8,9,10,11,12,13,14");
}

fn started_application(sandbox: &tempfile::TempDir) -> (SkillYardApplication, ApplicationPaths) {
    let home = sandbox.path().join("home");
    fs::create_dir_all(&home).expect("应创建测试 home");
    let paths = ApplicationPaths::for_home(sandbox.path().join("data"), home);
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应先完成首次扫描");
    (application, paths)
}

fn inventory_entry<'a>(outcome: &'a UiOutcome, name: &str) -> &'a skillyard_lib::InventoryItem {
    let UiOutcome::Inventory { entries, .. } = outcome else {
        panic!("应返回 Inventory");
    };
    entries
        .iter()
        .find(|entry| entry.skill_name == name)
        .expect("应找到目标 Skill")
}

fn init_git_repository(root: &Path) {
    fs::create_dir_all(root).expect("应创建 Git fixture 目录");
    run_git(root, &["init"]);
    run_git(root, &["config", "user.name", "SkillYard Test"]);
    run_git(root, &["config", "user.email", "skillyard@example.invalid"]);
}

fn commit_all(root: &Path, message: &str) {
    run_git(root, &["add", "--all"]);
    run_git(root, &["commit", "-m", message]);
}

fn run_git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .output()
        .expect("应执行测试 Git 命令");
    assert!(
        output.status.success(),
        "Git fixture 命令失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .output()
        .expect("应执行测试 Git 命令");
    assert!(output.status.success(), "Git fixture 查询应成功");
    String::from_utf8(output.stdout)
        .expect("Git fixture 输出应为 UTF-8")
        .trim()
        .to_owned()
}

fn clone_tree_filtered(origin: &Path, destination: &Path) {
    let origin_url = format!(
        "file://{}",
        origin.to_str().expect("测试 origin 路径应为 UTF-8")
    );
    let output = Command::new("/usr/bin/git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--filter=tree:0",
            "--no-checkout",
            &origin_url,
        ])
        .arg(destination)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .output()
        .expect("应执行过滤 clone");
    assert!(
        output.status.success(),
        "过滤 clone 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_pack_files(repository: &Path) -> BTreeSet<PathBuf> {
    fs::read_dir(repository.join(".git/objects/pack"))
        .expect("应读取 Git pack 目录")
        .map(|entry| entry.expect("应读取 Git pack 条目").path())
        .collect()
}

fn git_command_with_no_lazy_fetch(root: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new("/usr/bin/git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("LC_ALL", "C")
        .output()
        .expect("应执行禁用 lazy fetch 的 Git 命令")
}

fn write_skill(root: &std::path::Path, name: &str, script_contents: &str) {
    fs::create_dir_all(root).expect("应创建 Skill fixture");
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Test fixture\n---\n"),
    )
    .expect("应写入 SKILL.md fixture");
    fs::write(root.join("script.txt"), script_contents).expect("应写入 Skill 内容 fixture");
}
