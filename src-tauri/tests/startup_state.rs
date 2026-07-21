use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;

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
    assert_eq!(entries[0].root_key, ScanRootKey::CodexGlobal);
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
    assert_eq!(versions, "1,2");
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
