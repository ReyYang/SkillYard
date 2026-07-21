use std::{env, fs, path::Path, process::Command};

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, LifecycleFailpoint, ManagementKind, PlatformInfo, SkillYardApplication,
    UiIntent, UiOutcome,
};
use tempfile::tempdir;

const HARD_EXIT_WORKER: &str = "SKILLYARD_HARD_EXIT_WORKER";
const HARD_EXIT_DATA_ROOT: &str = "SKILLYARD_HARD_EXIT_DATA_ROOT";
const HARD_EXIT_HOME: &str = "SKILLYARD_HARD_EXIT_HOME";
const HARD_EXIT_PLAN_ID: &str = "SKILLYARD_HARD_EXIT_PLAN_ID";
const HARD_EXIT_POINT: &str = "SKILLYARD_HARD_EXIT_POINT";

#[test]
fn creating_a_folder_install_plan_does_not_write_managed_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");

    let outcome = application
        .handle(UiIntent::CreateFolderInstallPlan {
            input_path: input.to_string_lossy().into_owned(),
        })
        .expect("有效单 Skill 文件夹应生成 Plan");
    let UiOutcome::FolderInstallPlan { plan } = outcome else {
        panic!("应返回文件夹安装 Plan");
    };

    assert_eq!(plan.skill_name, "example-skill");
    assert_eq!(plan.bundle_display_name, "example-skill");
    assert!(!plan.will_mount);
    assert!(plan.expires_at > plan.created_at);
    assert!(!contains_entries(&data_root.join("bundles")));
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));
    assert!(!home.join(".codex/skills/example-skill").exists());
    assert_eq!(
        fs::read_to_string(input.join("payload.txt")).expect("原输入应保持可读"),
        "original"
    );
}

#[test]
fn confirming_a_plan_installs_one_unmounted_managed_bundle() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);

    let outcome = application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect("确认 Plan 后应完成安装");
    let UiOutcome::Inventory { entries, .. } = outcome else {
        panic!("安装完成后应返回 Inventory");
    };
    let managed = entries
        .iter()
        .find(|entry| entry.skill_name == "example-skill")
        .expect("Inventory 应包含新安装成员");

    assert_eq!(managed.management_kind, ManagementKind::SkillYardManaged);
    assert_eq!(
        managed.bundle_display_name.as_deref(),
        Some("example-skill")
    );
    assert!(managed.bundle_id.is_some());
    assert!(Path::new(&managed.skill_root).join("SKILL.md").is_file());
    assert!(!home.join(".codex/skills/example-skill").exists());
    assert!(!home.join(".claude/skills/example-skill").exists());
    assert!(!home.join(".copilot/skills/example-skill").exists());
    assert_eq!(
        fs::read_to_string(input.join("payload.txt")).expect("原输入应保持可读"),
        "original"
    );
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));

    let notice = fs::read_to_string(data_root.join("SKILLYARD-INFO.md"))
        .expect("Central Store Notice 应存在");
    assert!(notice.contains("example-skill"));
    assert!(notice.contains("未挂载"));

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let counts = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM bundles), (SELECT COUNT(*) FROM skill_members), (SELECT COUNT(*) FROM member_selections)",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
        )
        .expect("应读取安装领域记录");
    assert_eq!(counts, (1, 1, 1));
}

#[test]
fn startup_rebuilds_a_missing_central_store_notice_from_sqlite() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);
    application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect("安装应成功");
    let notice = data_root.join("SKILLYARD-INFO.md");
    fs::remove_file(&notice).expect("应模拟说明文件被误删");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    reopened
        .handle(UiIntent::GetStartupState)
        .expect("启动应重建说明文件");

    let contents = fs::read_to_string(notice).expect("说明文件应恢复");
    assert!(contents.contains("example-skill"));
    assert!(!contents.contains("- 暂无"));
}

#[test]
fn confirmation_rejects_unknown_expired_and_changed_plans() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");

    let unknown = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: "not-issued".to_owned(),
        })
        .expect_err("未签发 Plan 必须被拒绝");
    assert!(unknown.to_string().contains("未签发"));

    let expired_plan = create_plan(&application, &input);
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    connection
        .execute(
            "UPDATE install_plans SET expires_at = 0 WHERE id = ?1",
            [&expired_plan],
        )
        .expect("应使 Plan 过期");
    drop(connection);
    let expired = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: expired_plan,
        })
        .expect_err("过期 Plan 必须被拒绝");
    assert!(expired.to_string().contains("已过期"));

    let changed_plan = create_plan(&application, &input);
    fs::write(input.join("payload.txt"), "changed").expect("应修改 Plan 前置内容");
    let changed = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: changed_plan,
        })
        .expect_err("输入变化后旧 Plan 必须被拒绝");
    assert!(changed.to_string().contains("前置状态已经变化"));
    assert!(!contains_entries(&data_root.join("bundles")));
}

#[test]
fn permission_failure_is_rejected_before_a_lifecycle_transaction_starts() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);
    let journals = data_root.join("journals");
    fs::set_permissions(&journals, fs::Permissions::from_mode(0o500))
        .expect("应模拟 Journal 目录不可写");

    let error = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan_id.clone(),
        })
        .expect_err("权限不足必须在事务开始前拒绝");
    fs::set_permissions(&journals, fs::Permissions::from_mode(0o700)).expect("应恢复测试目录权限");

    assert!(error.to_string().contains("没有写权限"));
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let (transactions, status) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM lifecycle_transactions), status FROM install_plans WHERE id = ?1",
            [&plan_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("应读取 Plan 与事务状态");
    assert_eq!(transactions, 0);
    assert_eq!(status, "pending");
    assert!(!contains_entries(&data_root.join("bundles")));
    assert_eq!(
        fs::read_to_string(input.join("payload.txt")).expect("原输入必须保持不变"),
        "original"
    );
}

#[cfg(unix)]
#[test]
fn unsafe_links_are_rejected_before_a_plan_is_issued() {
    use std::os::unix::fs::symlink;

    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    symlink("payload.txt", input.join("linked.txt")).expect("应创建不安全软链接");

    let error = application
        .handle(UiIntent::CreateFolderInstallPlan {
            input_path: input.to_string_lossy().into_owned(),
        })
        .expect_err("包含链接的输入不能签发 Plan");

    assert!(error.to_string().contains("软链接"));
    assert!(!contains_entries(&data_root.join("bundles")));
    assert!(!contains_entries(&data_root.join("journals")));
}

#[test]
fn interruption_before_current_activation_recovers_to_not_installed() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application_with_failpoint(
        sandbox.path(),
        LifecycleFailpoint::AfterCandidatePrepared,
    );
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);

    let error = application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect_err("failpoint 应模拟生效前中断");
    assert!(error.to_string().contains("模拟中断"));
    assert!(contains_entries(&data_root.join("journals")));

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory { entries, .. } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启应自动恢复生效前事务")
    else {
        panic!("恢复后应返回 Inventory");
    };

    assert!(
        !entries
            .iter()
            .any(|entry| entry.skill_name == "example-skill")
    );
    assert!(!contains_entries(&data_root.join("bundles")));
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));
    assert_eq!(
        fs::read_to_string(input.join("payload.txt")).expect("原输入应保持可读"),
        "original"
    );
}

#[cfg(unix)]
#[test]
fn interruption_cleans_a_candidate_that_preserves_read_only_directories() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application_with_failpoint(
        sandbox.path(),
        LifecycleFailpoint::AfterCandidatePrepared,
    );
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let read_only = input.join("read-only");
    fs::create_dir(&read_only).expect("应创建只读内容目录");
    fs::write(read_only.join("asset.txt"), "asset").expect("应写入只读内容");
    fs::set_permissions(&read_only, fs::Permissions::from_mode(0o500)).expect("应设置只读目录权限");
    fs::set_permissions(&input, fs::Permissions::from_mode(0o500))
        .expect("应设置只读 Skill 根权限");
    let plan_id = create_plan(&application, &input);
    application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect_err("failpoint 应留下保留只读权限的候选");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        entries,
        recovery_issues,
        ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("只读候选应自动清理")
    else {
        panic!("恢复后应返回 Inventory");
    };

    assert!(recovery_issues.is_empty());
    assert!(
        !entries
            .iter()
            .any(|entry| entry.skill_name == "example-skill")
    );
    assert!(!contains_entries(&data_root.join("bundles")));
    assert!(!contains_entries(&data_root.join("staging")));
    fs::set_permissions(&input, fs::Permissions::from_mode(0o700)).expect("应恢复测试输入权限");
    fs::set_permissions(&read_only, fs::Permissions::from_mode(0o700)).expect("应恢复测试内容权限");
}

#[test]
fn interruption_after_temporary_current_creation_is_cleaned_automatically() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application_with_failpoint(
        sandbox.path(),
        LifecycleFailpoint::AfterTemporaryCurrentCreated,
    );
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);

    application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect_err("failpoint 应停在临时 current 创建之后");
    assert!(find_entry_with_prefix(&data_root.join("bundles"), ".current-").is_some());

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启应清理尚未生效的临时 current");

    let UiOutcome::Inventory { entries, .. } = outcome else {
        panic!("恢复后应返回 Inventory");
    };
    assert!(
        !entries
            .iter()
            .any(|entry| entry.skill_name == "example-skill")
    );
    assert!(!contains_entries(&data_root.join("bundles")));
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));
}

#[test]
fn interruption_during_candidate_cleanup_retries_the_owned_discard_directory() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application_with_failpoint(
        sandbox.path(),
        LifecycleFailpoint::AfterCandidatePrepared,
    );
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);
    application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect_err("failpoint 应留下未生效候选");

    let journal_path = only_journal_path(&data_root);
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).expect("应读取 Journal"))
            .expect("Journal 应为 JSON");
    let content = data_root.join(
        journal["content_relative"]
            .as_str()
            .expect("Journal 应记录内容路径"),
    );
    let staging = data_root.join(
        journal["staging_relative"]
            .as_str()
            .expect("Journal 应记录临时路径"),
    );
    fs::rename(&content, staging.join("discarding-content"))
        .expect("应模拟清理已原子隔离但进程尚未删除内容");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启应重试事务自有清理目录");

    assert!(!contains_entries(&data_root.join("bundles")));
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));
}

#[test]
fn interruption_after_current_activation_recovers_to_installed_idempotently() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) =
        ready_application_with_failpoint(sandbox.path(), LifecycleFailpoint::AfterCurrentActivated);
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);

    let error = application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect_err("failpoint 应模拟生效后中断");
    assert!(error.to_string().contains("模拟中断"));
    assert!(contains_entries(&data_root.join("journals")));

    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let reopened = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let first = reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启应自动完成生效后事务");
    assert_single_managed_skill(&first, "example-skill");
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));

    let reopened_again = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let second = reopened_again
        .handle(UiIntent::GetStartupState)
        .expect("重复启动恢复必须幂等");
    assert_single_managed_skill(&second, "example-skill");

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let count = connection
        .query_row("SELECT COUNT(*) FROM bundles", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("应读取 Bundle 数量");
    assert_eq!(count, 1);
}

#[test]
fn a_real_process_exit_before_current_recovers_to_not_installed() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (data_root, home, input) = prepare_hard_exit_install(sandbox.path());

    run_hard_exit_worker(&data_root, &home, &input.1, "before-current");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("真实进程退出后应自动恢复");
    let UiOutcome::Inventory { entries, .. } = outcome else {
        panic!("恢复后应返回 Inventory");
    };
    assert!(
        !entries
            .iter()
            .any(|entry| entry.skill_name == "example-skill")
    );
    assert!(!contains_entries(&data_root.join("bundles")));
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));
    assert_eq!(
        fs::read_to_string(input.0.join("payload.txt")).expect("原输入必须保持不变"),
        "original"
    );
}

#[test]
fn a_real_process_exit_before_journal_recovers_to_not_installed() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (data_root, home, input) = prepare_hard_exit_install(sandbox.path());

    run_hard_exit_worker(&data_root, &home, &input.1, "before-journal");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory { entries, .. } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("Journal 写入前退出应自动撤销事务")
    else {
        panic!("恢复后应返回 Inventory");
    };
    assert!(
        !entries
            .iter()
            .any(|entry| entry.skill_name == "example-skill")
    );
    assert!(!contains_entries(&data_root.join("bundles")));
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));
    assert_eq!(
        fs::read_to_string(input.0.join("payload.txt")).expect("原输入必须保持不变"),
        "original"
    );
}

#[test]
fn a_real_process_exit_after_current_recovers_to_installed() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (data_root, home, input) = prepare_hard_exit_install(sandbox.path());

    run_hard_exit_worker(&data_root, &home, &input.1, "after-current");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("真实进程退出后应完成安装");
    assert_single_managed_skill(&outcome, "example-skill");
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));
    assert_eq!(
        fs::read_to_string(input.0.join("payload.txt")).expect("原输入必须保持不变"),
        "original"
    );
}

#[test]
fn a_real_process_exit_after_domain_commit_recovers_to_installed() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (data_root, home, input) = prepare_hard_exit_install(sandbox.path());

    run_hard_exit_worker(&data_root, &home, &input.1, "after-domain");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("领域状态提交后的进程退出应自动完成清理");
    assert_single_managed_skill(&outcome, "example-skill");
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));
    assert_eq!(
        fs::read_to_string(input.0.join("payload.txt")).expect("原输入必须保持不变"),
        "original"
    );
}

/// 父测试通过精确过滤启动本测试；`_exit` 跳过析构，模拟真正的应用进程中断。
#[test]
fn hard_exit_install_worker() {
    if env::var_os(HARD_EXIT_WORKER).is_none() {
        return;
    }
    let data_root = env::var_os(HARD_EXIT_DATA_ROOT).expect("子进程必须收到数据目录");
    let home = env::var_os(HARD_EXIT_HOME).expect("子进程必须收到 home");
    let plan_id = env::var(HARD_EXIT_PLAN_ID).expect("子进程必须收到 Plan ID");
    let failpoint = match env::var(HARD_EXIT_POINT).as_deref() {
        Ok("before-journal") => LifecycleFailpoint::HardExitAfterTransactionRecord,
        Ok("before-current") => LifecycleFailpoint::HardExitAfterCandidatePublishedBeforePhase,
        Ok("after-current") => LifecycleFailpoint::HardExitAfterCurrentSwitchedBeforePhase,
        Ok("after-domain") => LifecycleFailpoint::HardExitAfterDomainCommittedBeforeJournal,
        _ => panic!("子进程收到未知 failpoint"),
    };
    let application = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.into(), home.into()),
        PlatformInfo::supported_for_test(),
        failpoint,
    );

    application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect("hard-exit failpoint 必须在返回前终止进程");
}

#[test]
fn a_tampered_journal_is_blocked_without_touching_other_managed_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application_with_failpoint(
        sandbox.path(),
        LifecycleFailpoint::AfterCandidatePrepared,
    );
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);
    application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect_err("failpoint 应留下可恢复 Journal");

    let protected = data_root.join("bundles/protected-by-contract");
    fs::create_dir_all(&protected).expect("应创建不属于该事务的受保护目录");
    fs::write(protected.join("keep.txt"), "keep").expect("应写入受保护内容");
    let journal_path = only_journal_path(&data_root);
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).expect("应读取 Journal"))
            .expect("Journal 应为 JSON");
    journal["content_relative"] = serde_json::json!("bundles/protected-by-contract");
    fs::write(
        &journal_path,
        serde_json::to_vec_pretty(&journal).expect("应序列化篡改 Journal"),
    )
    .expect("应模拟外部篡改 Journal");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("损坏事务只阻塞自身，清单仍应可读");
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = outcome
    else {
        panic!("阻塞恢复后应返回可继续浏览的 Inventory");
    };

    assert_eq!(recovery_issues.len(), 1);
    assert_eq!(recovery_issues[0].bundle_display_name, "example-skill");
    assert!(
        recovery_issues[0].message.contains("人工处理"),
        "恢复提示应解释哪项文件系统操作无法自动完成"
    );

    assert_eq!(
        fs::read_to_string(protected.join("keep.txt")).expect("受保护内容必须仍存在"),
        "keep"
    );
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let status = connection
        .query_row("SELECT status FROM lifecycle_transactions", [], |row| {
            row.get::<_, String>(0)
        })
        .expect("应读取阻塞状态");
    assert_eq!(status, "blocked");
}

#[test]
fn a_journal_with_unknown_fields_is_rejected_as_an_incompatible_contract() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application_with_failpoint(
        sandbox.path(),
        LifecycleFailpoint::AfterCandidatePrepared,
    );
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);
    application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect_err("failpoint 应留下 Journal");
    let journal_path = only_journal_path(&data_root);
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).expect("应读取 Journal"))
            .expect("Journal 应为 JSON");
    journal["future_phase_payload"] = serde_json::json!({ "unsafe": true });
    fs::write(
        &journal_path,
        serde_json::to_vec_pretty(&journal).expect("应序列化未知字段"),
    )
    .expect("应模拟不兼容 Journal");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("未知 Journal 字段只阻塞相关事务")
    else {
        panic!("恢复后应返回 Inventory");
    };

    assert_eq!(recovery_issues.len(), 1);
    assert!(
        recovery_issues[0].message.contains("future_phase_payload"),
        "恢复提示应指出不兼容字段：{}",
        recovery_issues[0].message
    );
}

#[test]
fn an_oversized_journal_is_bounded_and_shown_as_blocked_recovery() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application_with_failpoint(
        sandbox.path(),
        LifecycleFailpoint::AfterCandidatePrepared,
    );
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);
    application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect_err("failpoint 应留下 Journal");
    fs::write(only_journal_path(&data_root), vec![b'x'; 1024 * 1024 + 1])
        .expect("应模拟超大 Journal");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("超大 Journal 只阻塞相关事务")
    else {
        panic!("恢复后应返回 Inventory");
    };

    assert_eq!(recovery_issues.len(), 1);
    assert!(recovery_issues[0].message.contains("超过安全大小"));
}

#[cfg(unix)]
#[test]
fn a_fifo_journal_is_rejected_without_blocking_startup() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application_with_failpoint(
        sandbox.path(),
        LifecycleFailpoint::AfterCandidatePrepared,
    );
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);
    application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect_err("failpoint 应留下 Journal");
    let journal = only_journal_path(&data_root);
    fs::remove_file(&journal).expect("应移除普通 Journal");
    let journal_c = CString::new(journal.as_os_str().as_bytes()).expect("测试路径不能含 NUL");
    assert_eq!(unsafe { libc::mkfifo(journal_c.as_ptr(), 0o600) }, 0);

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("FIFO Journal 必须立即进入阻塞恢复")
    else {
        panic!("恢复后应返回 Inventory");
    };

    assert_eq!(recovery_issues.len(), 1);
    assert!(recovery_issues[0].message.contains("路径不安全"));
}

#[cfg(unix)]
#[test]
fn recovery_rejects_a_bundle_ancestor_replaced_by_a_symlink() {
    use std::os::unix::fs::symlink;

    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) =
        ready_application_with_failpoint(sandbox.path(), LifecycleFailpoint::AfterCurrentActivated);
    let input = sandbox.path().join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);
    application
        .handle(UiIntent::ConfirmInstallPlan { plan_id })
        .expect_err("failpoint 应留下已切换 current 的事务");

    let bundle = fs::read_dir(data_root.join("bundles"))
        .expect("应读取 Bundle 根目录")
        .next()
        .expect("应存在待恢复 Bundle")
        .expect("应读取待恢复 Bundle")
        .path();
    let external = sandbox.path().join("external-bundle");
    fs::rename(&bundle, &external).expect("应模拟外部移动 Bundle");
    fs::write(external.join("external-marker.txt"), "keep").expect("应写入外部保护标记");
    symlink(&external, &bundle).expect("应模拟 Bundle 祖先被替换为软链接");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        entries,
        recovery_issues,
        ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("目录逃逸只应阻塞相关事务")
    else {
        panic!("恢复后应返回仍可浏览的 Inventory");
    };

    assert!(
        !entries
            .iter()
            .any(|entry| entry.skill_name == "example-skill")
    );
    assert_eq!(recovery_issues.len(), 1);
    assert_eq!(
        fs::read_to_string(external.join("external-marker.txt"))
            .expect("外部目录不能被恢复流程修改"),
        "keep"
    );
}

fn ready_application(
    base: &Path,
) -> (SkillYardApplication, std::path::PathBuf, std::path::PathBuf) {
    ready_application_with_failpoint(base, LifecycleFailpoint::None)
}

fn ready_application_with_failpoint(
    base: &Path,
    failpoint: LifecycleFailpoint,
) -> (SkillYardApplication, std::path::PathBuf, std::path::PathBuf) {
    let data_root = base.join("application-support/SkillYard");
    let home = base.join("home");
    fs::create_dir_all(&home).expect("应创建测试 home");
    let application = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
        failpoint,
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("测试前首次扫描应成功");
    (application, data_root, home)
}

fn prepare_hard_exit_install(
    base: &Path,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    (std::path::PathBuf, String),
) {
    let (application, data_root, home) = ready_application(base);
    let input = base.join("downloads/example-skill");
    write_skill(&input, "example-skill", "original");
    let plan_id = create_plan(&application, &input);
    drop(application);
    (data_root, home, (input, plan_id))
}

fn run_hard_exit_worker(data_root: &Path, home: &Path, plan_id: &str, point: &str) {
    let status = Command::new(env::current_exe().expect("应找到当前测试二进制"))
        .args(["--exact", "hard_exit_install_worker", "--nocapture"])
        .env(HARD_EXIT_WORKER, "1")
        .env(HARD_EXIT_DATA_ROOT, data_root)
        .env(HARD_EXIT_HOME, home)
        .env(HARD_EXIT_PLAN_ID, plan_id)
        .env(HARD_EXIT_POINT, point)
        .status()
        .expect("应启动 hard-exit 子进程");
    assert_eq!(status.code(), Some(91), "子进程必须在 failpoint 直接退出");
}

fn create_plan(application: &SkillYardApplication, input: &Path) -> String {
    let UiOutcome::FolderInstallPlan { plan } = application
        .handle(UiIntent::CreateFolderInstallPlan {
            input_path: input.to_string_lossy().into_owned(),
        })
        .expect("应生成安装 Plan")
    else {
        panic!("应返回文件夹安装 Plan");
    };
    plan.id
}

fn assert_single_managed_skill(outcome: &UiOutcome, name: &str) {
    let UiOutcome::Inventory { entries, .. } = outcome else {
        panic!("应返回 Inventory");
    };
    let matching = entries
        .iter()
        .filter(|entry| {
            entry.skill_name == name && entry.management_kind == ManagementKind::SkillYardManaged
        })
        .count();
    assert_eq!(matching, 1);
}

fn write_skill(root: &Path, name: &str, payload: &str) {
    fs::create_dir_all(root).expect("应创建 Skill 根目录");
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: test skill\n---\n# {name}\n"),
    )
    .expect("应写入 SKILL.md");
    fs::write(root.join("payload.txt"), payload).expect("应写入普通内容");
}

fn contains_entries(path: &Path) -> bool {
    path.is_dir()
        && fs::read_dir(path)
            .expect("应读取 Central Store 子目录")
            .next()
            .is_some()
}

fn find_entry_with_prefix(root: &Path, prefix: &str) -> Option<std::path::PathBuf> {
    if !root.is_dir() {
        return None;
    }
    for bundle in fs::read_dir(root).ok()?.flatten() {
        if !bundle.path().is_dir() {
            continue;
        }
        for entry in fs::read_dir(bundle.path()).ok()?.flatten() {
            if entry.file_name().to_string_lossy().starts_with(prefix) {
                return Some(entry.path());
            }
        }
    }
    None
}

fn only_journal_path(data_root: &Path) -> std::path::PathBuf {
    fs::read_dir(data_root.join("journals"))
        .expect("应读取 Journal 目录")
        .next()
        .expect("应存在 Journal")
        .expect("应读取 Journal 条目")
        .path()
}
