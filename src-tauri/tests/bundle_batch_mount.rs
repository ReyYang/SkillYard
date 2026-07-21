use std::{
    collections::BTreeMap,
    env, fs,
    io::ErrorKind,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};

use rusqlite::{Connection, params};
use skillyard_lib::{
    ApplicationPaths, BatchMountDisposition, BatchMountPlan, BatchMountRequest, LifecycleFailpoint,
    ManagementKind, MountHealth, MountScope, MountSummary, PlatformInfo, RecoveryIssue,
    SkillYardApplication, SupportedAppId, UiIntent, UiOutcome,
};
use tempfile::tempdir;

const HARD_EXIT_WORKER: &str = "SKILLYARD_BATCH_MOUNT_HARD_EXIT_WORKER";
const HARD_EXIT_DATA_ROOT: &str = "SKILLYARD_BATCH_MOUNT_HARD_EXIT_DATA_ROOT";
const HARD_EXIT_HOME: &str = "SKILLYARD_BATCH_MOUNT_HARD_EXIT_HOME";
const HARD_EXIT_PLAN_ID: &str = "SKILLYARD_BATCH_MOUNT_HARD_EXIT_PLAN_ID";
const HARD_EXIT_SELECTED_ITEM_IDS: &str = "SKILLYARD_BATCH_MOUNT_HARD_EXIT_SELECTED_ITEM_IDS";
const HARD_EXIT_POINT: &str = "SKILLYARD_BATCH_MOUNT_HARD_EXIT_POINT";

#[test]
fn one_bundle_mounts_multiple_members_across_all_supported_apps_in_one_batch() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let bundle = install_bundle(&harness, "multi-app-bundle", &["brainstorming", "tdd"]);
    let requests = vec![
        global_request(&bundle, "brainstorming", SupportedAppId::Codex),
        global_request(&bundle, "brainstorming", SupportedAppId::ClaudeCode),
        global_request(&bundle, "tdd", SupportedAppId::GitHubCopilot),
    ];

    let plan = create_batch_plan(&harness.application, &bundle.id, requests);

    assert_eq!(plan.bundle_id, bundle.id);
    assert_eq!(plan.items.len(), 3);
    assert!(plan.items.iter().all(|item| {
        item.disposition == BatchMountDisposition::Ready
            && item.selectable
            && item.default_selected
            && item.target_health == MountHealth::Missing
    }));
    for item in &plan.items {
        let expected_root = match item.app_id {
            SupportedAppId::Codex => ".codex/skills",
            SupportedAppId::ClaudeCode => ".claude/skills",
            SupportedAppId::GitHubCopilot => ".copilot/skills",
        };
        assert!(
            Path::new(&item.target_path).ends_with(Path::new(expected_root).join(&item.skill_name))
        );
        assert_missing(Path::new(&item.target_path));
    }

    let mounted = confirm_batch_plan(&harness.application, &plan, selectable_item_ids(&plan));

    assert_eq!(inventory_mounts(&mounted).len(), 3);
    for item in &plan.items {
        assert_link(
            Path::new(&item.target_path),
            Path::new(&item.expected_target),
        );
    }
    assert_eq!(mount_row_count(&harness.data_root), 3);
}

#[test]
fn batch_plan_does_not_create_mount_state_and_confirmation_applies_only_selected_ready_items() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let bundle = install_bundle(&harness, "selection-bundle", &["alpha", "beta"]);
    let plan = create_batch_plan(
        &harness.application,
        &bundle.id,
        vec![
            global_request(&bundle, "alpha", SupportedAppId::Codex),
            global_request(&bundle, "beta", SupportedAppId::Codex),
        ],
    );
    let alpha = plan_item(&plan, "alpha", SupportedAppId::Codex);
    let beta = plan_item(&plan, "beta", SupportedAppId::Codex);

    // Plan 自身需要持久化以绑定前置状态，但确认前不能出现 Mount 关系或写事务。
    assert_missing(Path::new(&alpha.target_path));
    assert_missing(Path::new(&beta.target_path));
    assert_eq!(mount_row_count(&harness.data_root), 0);
    assert_eq!(row_count(&harness.data_root, "batch_mount_transactions"), 0);
    assert_eq!(inventory_mounts(&startup_state(&harness)).len(), 0);

    let mounted = confirm_batch_plan(&harness.application, &plan, vec![alpha.id.clone()]);

    assert_link(
        Path::new(&alpha.target_path),
        Path::new(&alpha.expected_target),
    );
    assert_missing(Path::new(&beta.target_path));
    assert_eq!(inventory_mounts(&mounted).len(), 1);
    assert_eq!(inventory_mounts(&mounted)[0].member_id, alpha.member_id);
}

#[test]
fn path_conflicts_are_previewed_rejected_when_selected_and_can_be_excluded() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let bundle = install_bundle(&harness, "path-conflict-bundle", &["ready", "occupied"]);
    let occupied_target = harness.home.join(".codex/skills/occupied");
    fs::create_dir_all(occupied_target.parent().unwrap()).expect("应创建测试 Host 目录");
    fs::write(&occupied_target, "external owner").expect("应创建未知占用内容");

    let plan = create_batch_plan(
        &harness.application,
        &bundle.id,
        vec![
            global_request(&bundle, "ready", SupportedAppId::Codex),
            global_request(&bundle, "occupied", SupportedAppId::Codex),
        ],
    );
    let ready = plan_item(&plan, "ready", SupportedAppId::Codex);
    let conflict = plan_item(&plan, "occupied", SupportedAppId::Codex);

    assert_eq!(ready.disposition, BatchMountDisposition::Ready);
    assert!(ready.selectable);
    assert_eq!(conflict.disposition, BatchMountDisposition::PathConflict);
    assert_eq!(conflict.target_health, MountHealth::Conflict);
    assert!(!conflict.selectable);
    assert!(!conflict.default_selected);
    assert!(conflict.conflict_reason.is_some());

    let error = harness
        .application
        .handle(UiIntent::ConfirmBatchMountPlan {
            plan_id: plan.id.clone(),
            selected_item_ids: vec![ready.id.clone(), conflict.id.clone()],
        })
        .expect_err("冲突项不能混入已确认集合");
    assert!(error.to_string().contains("Plan") || error.to_string().contains("选择"));
    assert_missing(Path::new(&ready.target_path));
    assert_eq!(
        fs::read_to_string(&occupied_target).unwrap(),
        "external owner"
    );
    assert_eq!(mount_row_count(&harness.data_root), 0);

    let mounted = confirm_batch_plan(&harness.application, &plan, vec![ready.id.clone()]);
    assert_link(
        Path::new(&ready.target_path),
        Path::new(&ready.expected_target),
    );
    assert_eq!(
        fs::read_to_string(&occupied_target).unwrap(),
        "external owner"
    );
    assert_eq!(inventory_mounts(&mounted).len(), 1);
}

#[test]
fn global_and_project_requests_for_the_same_member_and_app_are_scope_conflicts() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let bundle = install_bundle(&harness, "scope-conflict-bundle", &["shared-skill"]);
    let project = sandbox.path().join("project");
    fs::create_dir(&project).expect("应创建测试 Project");
    let registered = harness
        .application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记测试 Project");
    let project_id = inventory_project_id(&registered);
    let member_id = bundle.member("shared-skill").id.clone();

    let plan = create_batch_plan(
        &harness.application,
        &bundle.id,
        vec![
            BatchMountRequest {
                member_id: member_id.clone(),
                app_id: SupportedAppId::Codex,
                scope: MountScope::Global,
                project_id: None,
            },
            BatchMountRequest {
                member_id,
                app_id: SupportedAppId::Codex,
                scope: MountScope::Project,
                project_id: Some(project_id),
            },
        ],
    );

    assert_eq!(plan.items.len(), 2);
    assert!(plan.items.iter().all(|item| {
        item.disposition == BatchMountDisposition::ScopeConflict
            && !item.selectable
            && !item.default_selected
            && item.conflict_reason.is_some()
    }));
    let error = harness
        .application
        .handle(UiIntent::ConfirmBatchMountPlan {
            plan_id: plan.id.clone(),
            selected_item_ids: plan.items.iter().map(|item| item.id.clone()).collect(),
        })
        .expect_err("同一 Skill 与应用不能同时确认 global 和 project Mount");
    assert!(error.to_string().contains("Plan") || error.to_string().contains("选择"));
    for item in &plan.items {
        assert_missing(Path::new(&item.target_path));
    }
    assert_eq!(mount_row_count(&harness.data_root), 0);
}

#[test]
fn the_same_member_can_batch_mount_into_multiple_projects_for_one_app() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let bundle = install_bundle(&harness, "multi-project-bundle", &["shared-skill"]);
    let mut project_ids = Vec::new();
    for name in ["project-a", "project-b"] {
        let project = sandbox.path().join(name);
        fs::create_dir(&project).expect("应创建测试 Project");
        let outcome = harness
            .application
            .handle(UiIntent::RegisterProject {
                root_path: project.to_string_lossy().into_owned(),
            })
            .expect("应登记测试 Project");
        let UiOutcome::Inventory { projects, .. } = outcome else {
            panic!("登记后应返回 Inventory");
        };
        project_ids.push(
            projects
                .iter()
                .find(|candidate| candidate.display_name == name)
                .expect("应找到刚登记的 Project")
                .id
                .clone(),
        );
    }
    let member_id = bundle.member("shared-skill").id.clone();
    let plan = create_batch_plan(
        &harness.application,
        &bundle.id,
        project_ids
            .into_iter()
            .map(|project_id| BatchMountRequest {
                member_id: member_id.clone(),
                app_id: SupportedAppId::Codex,
                scope: MountScope::Project,
                project_id: Some(project_id),
            })
            .collect(),
    );

    assert_eq!(plan.items.len(), 2);
    assert!(
        plan.items
            .iter()
            .all(|item| item.disposition == BatchMountDisposition::Ready && item.selectable)
    );
    let mounted = confirm_batch_plan(&harness.application, &plan, selectable_item_ids(&plan));

    assert_eq!(inventory_mounts(&mounted).len(), 2);
    assert_eq!(mount_row_count(&harness.data_root), 2);
    for item in &plan.items {
        assert_link(
            Path::new(&item.target_path),
            Path::new(&item.expected_target),
        );
    }
}

#[test]
fn an_already_registered_drifted_mount_remains_a_nonselectable_preview_item() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let bundle = install_bundle(&harness, "drift-preview-bundle", &["drifted", "new-skill"]);
    let UiOutcome::MountPlan { plan: single_plan } = harness
        .application
        .handle(UiIntent::CreateMountPlan {
            member_id: bundle.member("drifted").id.clone(),
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect("应创建单项 Mount Plan")
    else {
        panic!("应返回单项 Mount Plan");
    };
    harness
        .application
        .handle(UiIntent::ConfirmMountPlan {
            plan_id: single_plan.id,
        })
        .expect("应先登记 Mount");
    fs::remove_file(&single_plan.target_path).expect("应模拟已登记 Mount 漂移为缺失");

    let batch = create_batch_plan(
        &harness.application,
        &bundle.id,
        vec![
            global_request(&bundle, "drifted", SupportedAppId::Codex),
            global_request(&bundle, "new-skill", SupportedAppId::Codex),
        ],
    );
    let drifted = plan_item(&batch, "drifted", SupportedAppId::Codex);
    let ready = plan_item(&batch, "new-skill", SupportedAppId::Codex);
    assert_eq!(drifted.disposition, BatchMountDisposition::AlreadyMounted);
    assert_eq!(drifted.target_health, MountHealth::Missing);
    assert!(!drifted.selectable);
    assert_eq!(ready.disposition, BatchMountDisposition::Ready);

    let mounted = confirm_batch_plan(&harness.application, &batch, vec![ready.id.clone()]);
    assert_eq!(inventory_mounts(&mounted).len(), 2);
    assert_missing(Path::new(&drifted.target_path));
    assert_link(
        Path::new(&ready.target_path),
        Path::new(&ready.expected_target),
    );
}

#[test]
fn existing_correct_symlink_is_ready_and_confirmation_only_records_it() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let bundle = install_bundle(&harness, "existing-link-bundle", &["existing-link"]);
    let member = bundle.member("existing-link");
    let target = harness.home.join(".codex/skills/existing-link");
    fs::create_dir_all(target.parent().unwrap()).expect("应创建测试 Host 目录");
    symlink(&member.expected_target, &target).expect("应预置正确软链接");
    let original_inode = fs::symlink_metadata(&target).unwrap().ino();

    let plan = create_batch_plan(
        &harness.application,
        &bundle.id,
        vec![global_request(
            &bundle,
            "existing-link",
            SupportedAppId::Codex,
        )],
    );
    let item = &plan.items[0];
    assert_eq!(item.disposition, BatchMountDisposition::Ready);
    assert_eq!(item.target_health, MountHealth::Healthy);
    assert!(item.selectable);

    let mounted = confirm_batch_plan(&harness.application, &plan, vec![item.id.clone()]);

    assert_eq!(fs::symlink_metadata(&target).unwrap().ino(), original_inode);
    assert_link(&target, &member.expected_target);
    assert_eq!(inventory_mounts(&mounted).len(), 1);
    assert_eq!(inventory_mounts(&mounted)[0].target_path, item.target_path);
}

#[test]
fn a_late_target_write_failure_rolls_back_every_link_created_by_the_batch() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let bundle = install_bundle(&harness, "target-failure-bundle", &["first", "second"]);
    let codex_root = harness.home.join(".codex/skills");
    fs::create_dir_all(&codex_root).expect("应创建第二个目标根目录");
    let plan = create_batch_plan(
        &harness.application,
        &bundle.id,
        vec![
            global_request(&bundle, "first", SupportedAppId::ClaudeCode),
            global_request(&bundle, "second", SupportedAppId::Codex),
        ],
    );
    let first = plan_item(&plan, "first", SupportedAppId::ClaudeCode);
    let second = plan_item(&plan, "second", SupportedAppId::Codex);

    // 目标仍可只读预检，但第二次 symlink 写入会在首项目标生效后正常失败。
    fs::set_permissions(&codex_root, fs::Permissions::from_mode(0o500))
        .expect("应收紧第二个目标根目录权限");
    let result = harness.application.handle(UiIntent::ConfirmBatchMountPlan {
        plan_id: plan.id.clone(),
        selected_item_ids: selectable_item_ids(&plan),
    });
    fs::set_permissions(&codex_root, fs::Permissions::from_mode(0o700))
        .expect("应恢复测试目录权限");

    result.expect_err("第二个目标写入失败必须让整个批次失败");
    assert_missing(Path::new(&first.target_path));
    assert_missing(Path::new(&second.target_path));
    assert_eq!(mount_row_count(&harness.data_root), 0);
    assert_eq!(inventory_mounts(&startup_state(&harness)).len(), 0);
    assert_directory_empty(&harness.data_root.join("journals"));
}

#[test]
fn normal_commit_failure_rolls_back_new_links_preserves_existing_links_and_commits_no_mounts() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let bundle = install_bundle(&harness, "rollback-bundle", &["existing", "new-a", "new-b"]);
    let existing = bundle.member("existing");
    let existing_target = harness.home.join(".codex/skills/existing");
    fs::create_dir_all(existing_target.parent().unwrap()).expect("应创建测试 Host 目录");
    symlink(&existing.expected_target, &existing_target).expect("应预置正确软链接");
    let existing_inode = fs::symlink_metadata(&existing_target).unwrap().ino();
    let plan = create_batch_plan(
        &harness.application,
        &bundle.id,
        vec![
            global_request(&bundle, "existing", SupportedAppId::Codex),
            global_request(&bundle, "new-a", SupportedAppId::Codex),
            global_request(&bundle, "new-b", SupportedAppId::Codex),
        ],
    );
    assert!(plan.items.iter().all(|item| item.selectable));

    // 在最后一个 Mount 记录写入时制造普通 SQLite 失败，文件效果此时必须整体撤回。
    let database = database_path(&harness.data_root);
    let connection = Connection::open(&database).expect("应打开测试 SQLite");
    let failing_member = &bundle.member("new-b").id;
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER fail_batch_mount_insert \
             BEFORE INSERT ON mounts WHEN NEW.member_id = '{failing_member}' \
             BEGIN SELECT RAISE(ABORT, 'test batch mount failure'); END;"
        ))
        .expect("应安装定向失败 trigger");

    let error = harness
        .application
        .handle(UiIntent::ConfirmBatchMountPlan {
            plan_id: plan.id.clone(),
            selected_item_ids: selectable_item_ids(&plan),
        })
        .expect_err("Mount 状态写入失败必须让整个批次失败");
    assert!(error.to_string().contains("test batch mount failure"));
    connection
        .execute_batch("DROP TRIGGER fail_batch_mount_insert;")
        .expect("应移除测试 trigger");

    assert_eq!(
        fs::symlink_metadata(&existing_target).unwrap().ino(),
        existing_inode
    );
    assert_link(&existing_target, &existing.expected_target);
    assert_missing(&harness.home.join(".codex/skills/new-a"));
    assert_missing(&harness.home.join(".codex/skills/new-b"));
    assert_eq!(mount_row_count(&harness.data_root), 0);
    assert_eq!(inventory_mounts(&startup_state(&harness)).len(), 0);
    assert_directory_empty(&harness.data_root.join("journals"));
}

#[test]
fn hard_exit_during_rollback_resumes_to_zero_links_and_zero_mounts() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (harness, plan) = prepared_recovery_batch(sandbox.path(), "rollback-hard-exit");
    let targets = plan_targets(&plan);
    let connection =
        Connection::open(database_path(&harness.data_root)).expect("应打开测试 SQLite");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_rollback_hard_exit_mount_insert \
             BEFORE INSERT ON mounts \
             BEGIN SELECT RAISE(ABORT, 'test rollback hard-exit failure'); END;",
        )
        .expect("应安装 finalize 失败 trigger");

    // finalize 失败后开始反向撤销，并在删除首个事务链接后模拟进程崩溃。
    run_hard_exit_worker(&harness, &plan, "after-first-rollback-before-progress");

    assert_eq!(symlink_count(&targets), 1, "中断点必须位于首个链接回滚后");
    assert_eq!(mount_row_count(&harness.data_root), 0);
    let recovered = startup_state(&harness);
    assert_eq!(symlink_count(&targets), 0);
    assert_eq!(mount_row_count(&harness.data_root), 0);
    assert!(inventory_mounts(&recovered).is_empty());
    assert!(inventory_recovery_issues(&recovered).is_empty());
    assert_eq!(row_count(&harness.data_root, "batch_mount_transactions"), 0);
    assert_directory_empty(&harness.data_root.join("journals"));
}

#[test]
fn hard_exit_with_owned_link_in_rollback_quarantine_finishes_cleanup() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (harness, plan) = prepared_recovery_batch(sandbox.path(), "rollback-quarantine");
    let targets = plan_targets(&plan);
    let connection =
        Connection::open(database_path(&harness.data_root)).expect("应打开测试 SQLite");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_rollback_quarantine_mount_insert \
             BEFORE INSERT ON mounts \
             BEGIN SELECT RAISE(ABORT, 'test rollback quarantine failure'); END;",
        )
        .expect("应安装 finalize 失败 trigger");

    // 首个 owned link 已离开最终名但尚未转入受管 discard，恢复必须识别 Host quarantine 的归属证据。
    run_hard_exit_worker(&harness, &plan, "after-first-rollback-quarantine");

    assert_eq!(symlink_count(&targets), 1, "中断时首个最终链接应已被隔离");
    assert_eq!(batch_rollback_entry_count(&harness.home), 1);
    assert_eq!(mount_row_count(&harness.data_root), 0);
    let recovered = startup_state(&harness);
    assert_eq!(symlink_count(&targets), 0);
    assert_eq!(batch_rollback_entry_count(&harness.home), 0);
    assert_eq!(mount_row_count(&harness.data_root), 0);
    assert!(inventory_mounts(&recovered).is_empty());
    assert!(inventory_recovery_issues(&recovered).is_empty());
    assert_eq!(row_count(&harness.data_root, "batch_mount_transactions"), 0);
    assert_directory_empty(&harness.data_root.join("journals"));
}

#[test]
fn hard_exit_with_owned_link_in_managed_discard_finishes_cleanup() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (harness, plan) = prepared_recovery_batch(sandbox.path(), "rollback-discard");
    let targets = plan_targets(&plan);
    let connection =
        Connection::open(database_path(&harness.data_root)).expect("应打开测试 SQLite");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_rollback_discard_mount_insert \
             BEFORE INSERT ON mounts \
             BEGIN SELECT RAISE(ABORT, 'test rollback discard failure'); END;",
        )
        .expect("应安装 finalize 失败 trigger");

    // 链接已经离开 Host 并进入私有 discard，但还没有被删除。
    run_hard_exit_worker(&harness, &plan, "after-first-rollback-discard");

    assert_eq!(symlink_count(&targets), 1);
    assert_eq!(batch_rollback_entry_count(&harness.home), 0);
    assert_eq!(batch_managed_discard_entry_count(&harness.data_root), 1);
    assert_eq!(mount_row_count(&harness.data_root), 0);
    let recovered = startup_state(&harness);
    assert_eq!(symlink_count(&targets), 0);
    assert_eq!(batch_managed_discard_entry_count(&harness.data_root), 0);
    assert_eq!(mount_row_count(&harness.data_root), 0);
    assert!(inventory_mounts(&recovered).is_empty());
    assert!(inventory_recovery_issues(&recovered).is_empty());
    assert_eq!(row_count(&harness.data_root, "batch_mount_transactions"), 0);
    assert_directory_empty(&harness.data_root.join("journals"));
}

#[test]
fn unknown_content_in_managed_discard_is_blocked_and_preserved() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (harness, plan) = prepared_recovery_batch(sandbox.path(), "unknown-discard");
    let connection =
        Connection::open(database_path(&harness.data_root)).expect("应打开测试 SQLite");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_unknown_discard_mount_insert \
             BEFORE INSERT ON mounts \
             BEGIN SELECT RAISE(ABORT, 'test unknown discard failure'); END;",
        )
        .expect("应安装 finalize 失败 trigger");
    run_hard_exit_worker(&harness, &plan, "after-first-rollback-discard");

    let discard_entries = batch_managed_discard_entries(&harness.data_root);
    assert_eq!(discard_entries.len(), 1);
    fs::remove_file(&discard_entries[0]).expect("应模拟外部替换 owned link");
    fs::write(&discard_entries[0], b"external-content").expect("应写入未知内容");

    let blocked = startup_state(&harness);
    assert_eq!(inventory_recovery_issues(&blocked).len(), 1);
    assert_eq!(
        fs::read(&discard_entries[0]).expect("未知内容不能被递归清理"),
        b"external-content"
    );
    assert_eq!(row_count(&harness.data_root, "batch_mount_transactions"), 1);
}

#[test]
fn quarantine_replacement_race_restores_and_preserves_unknown_host_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (harness, plan) = prepared_recovery_batch(sandbox.path(), "rollback-race");
    let connection =
        Connection::open(database_path(&harness.data_root)).expect("应打开测试 SQLite");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_rollback_race_mount_insert \
             BEFORE INSERT ON mounts \
             BEGIN SELECT RAISE(ABORT, 'test rollback race failure'); END;",
        )
        .expect("应安装 finalize 失败 trigger");
    let application = SkillYardApplication::new_with_lifecycle_failpoint(
        harness.paths.clone(),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::ReplaceFirstBatchMountQuarantineWithUnknownBeforeDiscard,
    );

    let error = application
        .handle(UiIntent::ConfirmBatchMountPlan {
            plan_id: plan.id.clone(),
            selected_item_ids: selectable_item_ids(&plan),
        })
        .expect_err("Host quarantine 校验后的替换竞态必须阻塞事务");
    assert!(error.to_string().contains("前置状态"));

    let quarantine_entries = batch_temporary_entries(&harness.home, ".skillyard-batch-rollback-");
    assert_eq!(quarantine_entries.len(), 1);
    let metadata = fs::symlink_metadata(&quarantine_entries[0]).expect("未知内容必须被保留");
    assert!(metadata.is_file(), "竞态写入的普通文件不能被删除");
    assert_eq!(batch_managed_discard_entry_count(&harness.data_root), 0);
    assert_eq!(mount_row_count(&harness.data_root), 0);

    let blocked = startup_state(&harness);
    assert_eq!(inventory_recovery_issues(&blocked).len(), 1);
    assert!(
        fs::symlink_metadata(&quarantine_entries[0])
            .expect("启动恢复也不能删除未知内容")
            .is_file()
    );
    assert_eq!(row_count(&harness.data_root, "batch_mount_transactions"), 1);
}

#[test]
fn hard_exit_after_stage_evidence_recovers_by_publishing_the_owned_link() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (harness, plan) = prepared_recovery_batch(sandbox.path(), "after-stage-evidence");
    let targets = plan_targets(&plan);

    run_hard_exit_worker(&harness, &plan, "after-first-stage-evidence");

    assert_eq!(
        symlink_count(&targets),
        0,
        "中断时暂存链接还没有向 Host 发布"
    );
    assert_eq!(batch_stage_entry_count(&harness.home), 1);
    assert_eq!(mount_row_count(&harness.data_root), 0);
    let recovered = startup_state(&harness);
    assert_eq!(symlink_count(&targets), 2);
    assert_eq!(batch_stage_entry_count(&harness.home), 0);
    assert_eq!(inventory_mounts(&recovered).len(), 2);
    assert_eq!(mount_row_count(&harness.data_root), 2);
    assert_directory_empty(&harness.data_root.join("journals"));
}

#[test]
fn hard_exit_after_first_target_recovers_forward_as_one_complete_batch() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (harness, plan) = prepared_recovery_batch(sandbox.path(), "after-first");
    let targets = plan_targets(&plan);

    run_hard_exit_worker(&harness, &plan, "after-first-target");

    assert_eq!(symlink_count(&targets), 1, "中断点必须位于首个目标生效后");
    assert_eq!(mount_row_count(&harness.data_root), 0);
    let recovered = startup_state(&harness);
    assert_eq!(symlink_count(&targets), 2);
    assert_eq!(inventory_mounts(&recovered).len(), 2);
    assert_eq!(mount_row_count(&harness.data_root), 2);
    assert_directory_empty(&harness.data_root.join("journals"));
}

#[test]
fn double_block_during_recovery_is_idempotent_and_preserves_unknown_host_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (harness, plan) = prepared_recovery_batch(sandbox.path(), "double-block");
    let targets = plan_targets(&plan);

    run_hard_exit_worker(&harness, &plan, "after-first-target");
    let published = targets
        .iter()
        .find(|target| {
            fs::symlink_metadata(target)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
        })
        .expect("中断后应只有一条已发布软链接")
        .clone();
    assert_eq!(symlink_count(&targets), 1);

    // 外部内容没有事务归属证据：forward 与 rollback 都必须拒绝触碰，重复 block 仍应正常返回启动状态。
    fs::remove_file(&published).expect("应移除事务创建的测试软链接");
    let unknown_content = b"external owner must remain";
    fs::write(&published, unknown_content).expect("应以未知普通文件占用已发布目标");

    let recovered = startup_state(&harness);

    assert_eq!(inventory_recovery_issues(&recovered).len(), 1);
    assert!(inventory_mounts(&recovered).is_empty());
    assert_eq!(mount_row_count(&harness.data_root), 0);
    assert_eq!(row_count(&harness.data_root, "batch_mount_transactions"), 1);
    assert_eq!(
        fs::read(&published).expect("未知文件必须保留"),
        unknown_content
    );
    assert!(
        fs::metadata(&published)
            .expect("未知文件必须仍存在")
            .is_file()
    );
    assert_eq!(symlink_count(&targets), 0);
}

#[test]
fn hard_exit_after_all_targets_recovers_forward_as_one_complete_batch() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (harness, plan) = prepared_recovery_batch(sandbox.path(), "after-all");
    let targets = plan_targets(&plan);

    run_hard_exit_worker(&harness, &plan, "after-all-targets");

    assert_eq!(symlink_count(&targets), 2);
    assert_eq!(mount_row_count(&harness.data_root), 0);
    let recovered = startup_state(&harness);
    assert_eq!(symlink_count(&targets), 2);
    assert_eq!(inventory_mounts(&recovered).len(), 2);
    assert_eq!(mount_row_count(&harness.data_root), 2);
    assert_directory_empty(&harness.data_root.join("journals"));
}

#[test]
fn recovery_finalize_failure_rolls_back_all_targets_instead_of_staying_blocked() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (harness, plan) = prepared_recovery_batch(sandbox.path(), "recovery-finalize-failure");
    let targets = plan_targets(&plan);

    run_hard_exit_worker(&harness, &plan, "after-all-targets");
    assert_eq!(symlink_count(&targets), 2);
    assert_eq!(mount_row_count(&harness.data_root), 0);

    // 恢复已经确认全部文件效果后，让 SQLite 最终提交失败；恢复方向必须切换为完整回滚。
    let connection =
        Connection::open(database_path(&harness.data_root)).expect("应打开测试 SQLite");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_recovery_mount_insert \
             BEFORE INSERT ON mounts \
             BEGIN SELECT RAISE(ABORT, 'test recovery finalize failure'); END;",
        )
        .expect("应安装恢复阶段 finalize 失败 trigger");

    let recovered = startup_state(&harness);

    assert_eq!(symlink_count(&targets), 0);
    assert_eq!(mount_row_count(&harness.data_root), 0);
    assert!(inventory_mounts(&recovered).is_empty());
    assert!(inventory_recovery_issues(&recovered).is_empty());
    assert_eq!(row_count(&harness.data_root, "batch_mount_transactions"), 0);
    assert_directory_empty(&harness.data_root.join("journals"));
}

#[test]
fn blocked_batch_reserves_related_members_and_targets_but_allows_unrelated_mounts() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let blocked_bundle = install_bundle(
        &harness,
        "blocked-reservation-bundle",
        &["first-applied", "reserved-skill", "free-skill"],
    );
    let colliding_bundle = install_bundle(&harness, "target-collision-bundle", &["reserved-skill"]);
    let plan = create_batch_plan(
        &harness.application,
        &blocked_bundle.id,
        vec![
            global_request(&blocked_bundle, "first-applied", SupportedAppId::ClaudeCode),
            global_request(&blocked_bundle, "reserved-skill", SupportedAppId::Codex),
        ],
    );
    let first =
        PathBuf::from(&plan_item(&plan, "first-applied", SupportedAppId::ClaudeCode).target_path);
    let reserved =
        PathBuf::from(&plan_item(&plan, "reserved-skill", SupportedAppId::Codex).target_path);

    run_hard_exit_worker(&harness, &plan, "after-first-target");
    assert_link(
        &first,
        &blocked_bundle.member("first-applied").expected_target,
    );
    assert_missing(&reserved);

    // 篡改 seal 只用于把仍有明确对象范围的事务推进到 blocked，不改变被预留的 member/target。
    let connection =
        Connection::open(database_path(&harness.data_root)).expect("应打开测试 SQLite");
    let changed = connection
        .execute(
            "UPDATE batch_mount_plan_items SET expected_target = ?1 WHERE id = ?2",
            params![
                sandbox.path().join("tampered-target").to_string_lossy(),
                plan.items[0].id
            ],
        )
        .expect("应模拟 Batch Plan seal 损坏");
    assert_eq!(changed, 1);
    let blocked = startup_state(&harness);
    assert_eq!(inventory_recovery_issues(&blocked).len(), 1);

    harness
        .application
        .handle(UiIntent::CreateMountPlan {
            member_id: blocked_bundle.member("reserved-skill").id.clone(),
            app_id: SupportedAppId::GitHubCopilot,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect_err("blocked Batch 中的同一 member 不能建立新的单项 Mount Plan");

    let same_target = harness.application.handle(UiIntent::CreateBatchMountPlan {
        bundle_id: colliding_bundle.id.clone(),
        requests: vec![global_request(
            &colliding_bundle,
            "reserved-skill",
            SupportedAppId::Codex,
        )],
    });
    match same_target {
        Err(_) => {}
        Ok(UiOutcome::BatchMountPlan { plan }) => {
            assert_eq!(plan.items.len(), 1);
            assert!(
                !plan.items[0].selectable,
                "blocked Batch 预留的缺失目标不能被另一 member 重新选择"
            );
        }
        Ok(other) => panic!("应拒绝同目标 Batch Mount 或返回不可选预览，实际为 {other:?}"),
    }

    let UiOutcome::MountPlan { plan: free_plan } = harness
        .application
        .handle(UiIntent::CreateMountPlan {
            member_id: blocked_bundle.member("free-skill").id.clone(),
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect("blocked Batch 不能冻结无关 member")
    else {
        panic!("无关 member 应返回单项 Mount Plan");
    };
    let mounted = harness
        .application
        .handle(UiIntent::ConfirmMountPlan {
            plan_id: free_plan.id.clone(),
        })
        .expect("无关 member 应可完成 Mount");

    assert_link(
        Path::new(&free_plan.target_path),
        &blocked_bundle.member("free-skill").expected_target,
    );
    assert_eq!(inventory_mounts(&mounted).len(), 1);
    assert_eq!(mount_row_count(&harness.data_root), 1);
    assert_link(
        &first,
        &blocked_bundle.member("first-applied").expected_target,
    );
    assert_missing(&reserved);
}

#[test]
fn hard_exit_after_database_commit_keeps_the_complete_batch_and_finishes_cleanup() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (harness, plan) = prepared_recovery_batch(sandbox.path(), "after-state");
    let targets = plan_targets(&plan);

    run_hard_exit_worker(&harness, &plan, "after-state-commit");

    assert_eq!(symlink_count(&targets), 2);
    assert_eq!(mount_row_count(&harness.data_root), 2);
    let recovered = startup_state(&harness);
    assert_eq!(symlink_count(&targets), 2);
    assert_eq!(inventory_mounts(&recovered).len(), 2);
    assert_eq!(mount_row_count(&harness.data_root), 2);
    assert_directory_empty(&harness.data_root.join("journals"));
}

#[test]
fn duplicate_requests_selections_and_plan_replay_are_rejected_without_duplicate_mounts() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let bundle = install_bundle(&harness, "duplicate-bundle", &["only-skill"]);
    let request = global_request(&bundle, "only-skill", SupportedAppId::Codex);

    harness
        .application
        .handle(UiIntent::CreateBatchMountPlan {
            bundle_id: bundle.id.clone(),
            requests: vec![request.clone(), request.clone()],
        })
        .expect_err("完全重复的目标请求必须被拒绝");

    let plan = create_batch_plan(&harness.application, &bundle.id, vec![request.clone()]);
    let item = &plan.items[0];
    harness
        .application
        .handle(UiIntent::ConfirmBatchMountPlan {
            plan_id: plan.id.clone(),
            selected_item_ids: vec![item.id.clone(), item.id.clone()],
        })
        .expect_err("同一 Plan Item 不能重复选择");
    assert_missing(Path::new(&item.target_path));
    assert_eq!(mount_row_count(&harness.data_root), 0);

    let mounted = confirm_batch_plan(&harness.application, &plan, vec![item.id.clone()]);
    assert_eq!(inventory_mounts(&mounted).len(), 1);
    let original_inode = fs::symlink_metadata(&item.target_path).unwrap().ino();
    harness
        .application
        .handle(UiIntent::ConfirmBatchMountPlan {
            plan_id: plan.id.clone(),
            selected_item_ids: vec![item.id.clone()],
        })
        .expect_err("已消费的 Batch Mount Plan 不能重放");

    let already_mounted = create_batch_plan(&harness.application, &bundle.id, vec![request]);
    assert_eq!(
        already_mounted.items[0].disposition,
        BatchMountDisposition::AlreadyMounted
    );
    assert!(!already_mounted.items[0].selectable);
    assert!(!already_mounted.items[0].default_selected);
    harness
        .application
        .handle(UiIntent::ConfirmBatchMountPlan {
            plan_id: already_mounted.id,
            selected_item_ids: vec![already_mounted.items[0].id.clone()],
        })
        .expect_err("已登记 Mount 不能再次选择");
    assert_eq!(
        fs::symlink_metadata(&item.target_path).unwrap().ino(),
        original_inode
    );
    assert_eq!(mount_row_count(&harness.data_root), 1);
}

#[test]
fn cross_bundle_members_and_foreign_plan_items_are_rejected() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let first = install_bundle(&harness, "first-bundle", &["first-skill"]);
    let second = install_bundle(&harness, "second-bundle", &["second-skill"]);

    harness
        .application
        .handle(UiIntent::CreateBatchMountPlan {
            bundle_id: first.id.clone(),
            requests: vec![global_request(
                &second,
                "second-skill",
                SupportedAppId::Codex,
            )],
        })
        .expect_err("Bundle 不能接收另一个 Bundle 的 Member");

    let first_plan = create_batch_plan(
        &harness.application,
        &first.id,
        vec![global_request(&first, "first-skill", SupportedAppId::Codex)],
    );
    let second_plan = create_batch_plan(
        &harness.application,
        &second.id,
        vec![global_request(
            &second,
            "second-skill",
            SupportedAppId::ClaudeCode,
        )],
    );
    harness
        .application
        .handle(UiIntent::ConfirmBatchMountPlan {
            plan_id: first_plan.id.clone(),
            selected_item_ids: vec![second_plan.items[0].id.clone()],
        })
        .expect_err("确认不能借用另一份 Plan 的 Item ID");

    for item in first_plan.items.iter().chain(&second_plan.items) {
        assert_missing(Path::new(&item.target_path));
    }
    assert_eq!(mount_row_count(&harness.data_root), 0);
}

#[test]
fn tampered_persisted_plan_is_rejected_before_any_filesystem_or_mount_change() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let bundle = install_bundle(&harness, "tamper-bundle", &["tamper-a", "tamper-b"]);
    let plan = create_batch_plan(
        &harness.application,
        &bundle.id,
        vec![
            global_request(&bundle, "tamper-a", SupportedAppId::Codex),
            global_request(&bundle, "tamper-b", SupportedAppId::ClaudeCode),
        ],
    );
    let tampered_target = sandbox.path().join("outside/tampered-target");
    let connection =
        Connection::open(database_path(&harness.data_root)).expect("应打开测试 SQLite");
    let changed = connection
        .execute(
            "UPDATE batch_mount_plan_items SET expected_target = ?1 WHERE id = ?2",
            params![tampered_target.to_string_lossy(), plan.items[0].id],
        )
        .expect("应模拟持久化 Plan 被篡改");
    assert_eq!(changed, 1);

    let error = harness
        .application
        .handle(UiIntent::ConfirmBatchMountPlan {
            plan_id: plan.id.clone(),
            selected_item_ids: selectable_item_ids(&plan),
        })
        .expect_err("被篡改的持久化 Plan 必须在生效前拒绝");
    assert!(error.to_string().contains("Plan") || error.to_string().contains("篡改"));
    for item in &plan.items {
        assert_missing(Path::new(&item.target_path));
    }
    assert_missing(&tampered_target);
    assert_eq!(mount_row_count(&harness.data_root), 0);
    assert_eq!(row_count(&harness.data_root, "batch_mount_transactions"), 0);
}

#[test]
fn coordinated_target_rewrite_cannot_change_the_app_after_preview() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let harness = ready_harness(sandbox.path());
    let bundle = install_bundle(&harness, "coordinated-tamper-bundle", &["sealed-skill"]);
    let plan = create_batch_plan(
        &harness.application,
        &bundle.id,
        vec![global_request(
            &bundle,
            "sealed-skill",
            SupportedAppId::Codex,
        )],
    );
    let original_target = PathBuf::from(&plan.items[0].target_path);
    let rewritten_target = harness.home.join(".claude/skills/sealed-skill");
    let connection =
        Connection::open(database_path(&harness.data_root)).expect("应打开测试 SQLite");
    let changed = connection
        .execute(
            "UPDATE batch_mount_plan_items
             SET app_id = 'claude_code', target_path = ?1
             WHERE id = ?2",
            params![rewritten_target.to_string_lossy(), plan.items[0].id],
        )
        .expect("应模拟多个字段被协调改写");
    assert_eq!(changed, 1);

    let error = harness
        .application
        .handle(UiIntent::ConfirmBatchMountPlan {
            plan_id: plan.id.clone(),
            selected_item_ids: selectable_item_ids(&plan),
        })
        .expect_err("确认必须仍绑定用户看到的 Codex 预览");
    assert!(error.to_string().contains("Plan"));
    assert_missing(&original_target);
    assert_missing(&rewritten_target);
    assert_eq!(mount_row_count(&harness.data_root), 0);
    assert_eq!(row_count(&harness.data_root, "batch_mount_transactions"), 0);
}

#[test]
fn hard_exit_batch_mount_worker() {
    if env::var_os(HARD_EXIT_WORKER).is_none() {
        return;
    }
    let data_root =
        PathBuf::from(env::var_os(HARD_EXIT_DATA_ROOT).expect("子进程必须收到数据目录"));
    let home = PathBuf::from(env::var_os(HARD_EXIT_HOME).expect("子进程必须收到 home"));
    let plan_id = env::var(HARD_EXIT_PLAN_ID).expect("子进程必须收到 Batch Mount Plan ID");
    let selected_item_ids = env::var(HARD_EXIT_SELECTED_ITEM_IDS)
        .expect("子进程必须收到选择项")
        .split(',')
        .map(str::to_owned)
        .collect();
    let failpoint = match env::var(HARD_EXIT_POINT).as_deref() {
        Ok("after-first-stage-evidence") => {
            LifecycleFailpoint::HardExitAfterFirstBatchMountStageJournalBeforePublish
        }
        Ok("after-first-target") => {
            LifecycleFailpoint::HardExitAfterFirstBatchMountTargetAppliedBeforePhase
        }
        Ok("after-all-targets") => {
            LifecycleFailpoint::HardExitAfterAllBatchMountTargetsAppliedBeforeState
        }
        Ok("after-first-rollback-before-progress") => {
            LifecycleFailpoint::HardExitAfterFirstBatchMountRollbackBeforeProgress
        }
        Ok("after-first-rollback-quarantine") => {
            LifecycleFailpoint::HardExitAfterFirstBatchMountQuarantineBeforeUnlink
        }
        Ok("after-first-rollback-discard") => {
            LifecycleFailpoint::HardExitAfterFirstBatchMountDiscardBeforeUnlink
        }
        Ok("after-state-commit") => {
            LifecycleFailpoint::HardExitAfterBatchMountStateCommittedBeforeJournal
        }
        other => panic!("未知 Batch Mount hard-exit 点：{other:?}"),
    };
    let application = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
        failpoint,
    );
    application
        .handle(UiIntent::ConfirmBatchMountPlan {
            plan_id,
            selected_item_ids,
        })
        .expect("hard-exit failpoint 必须在返回前终止进程");
}

struct TestHarness {
    root: PathBuf,
    data_root: PathBuf,
    home: PathBuf,
    paths: ApplicationPaths,
    application: SkillYardApplication,
}

struct InstalledMember {
    id: String,
    expected_target: PathBuf,
}

struct InstalledBundle {
    id: String,
    members: BTreeMap<String, InstalledMember>,
}

impl InstalledBundle {
    fn member(&self, skill_name: &str) -> &InstalledMember {
        self.members
            .get(skill_name)
            .expect("测试 Bundle 应包含成员")
    }
}

fn ready_harness(root: &Path) -> TestHarness {
    let home = root.join("home");
    let data_root = root.join("data");
    fs::create_dir(&home).expect("应创建测试 home");
    let paths = ApplicationPaths::for_home(data_root.clone(), home.clone());
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应完成首次扫描");
    TestHarness {
        root: root.to_path_buf(),
        data_root,
        home,
        paths,
        application,
    }
}

fn install_bundle(harness: &TestHarness, label: &str, skill_names: &[&str]) -> InstalledBundle {
    let input = harness.root.join("sources").join(label);
    for skill_name in skill_names {
        let skill_root = input.join("skills").join(skill_name);
        fs::create_dir_all(&skill_root).expect("应创建测试 Skill 目录");
        fs::write(
            skill_root.join("SKILL.md"),
            format!(
                "---\nname: {skill_name}\ndescription: Batch Mount 测试 Skill\n---\n# {skill_name}\n"
            ),
        )
        .expect("应写入测试 Skill");
    }
    let UiOutcome::FolderInstallPlan { plan } = harness
        .application
        .handle(UiIntent::CreateFolderInstallPlan {
            input_path: input.to_string_lossy().into_owned(),
        })
        .expect("应创建 Bundle 安装 Plan")
    else {
        panic!("应返回 Bundle 安装 Plan");
    };
    let selected_candidate_ids = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.selectable)
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    let installed = harness
        .application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids,
        })
        .expect("应安装测试 Bundle");
    let UiOutcome::Inventory { entries, .. } = installed else {
        panic!("安装完成后应返回 Inventory");
    };
    let matching = entries
        .into_iter()
        .filter(|entry| {
            entry.management_kind == ManagementKind::SkillYardManaged
                && entry.bundle_display_name.as_deref() == Some(label)
                && skill_names.contains(&entry.skill_name.as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), skill_names.len());
    let bundle_id = matching[0]
        .bundle_id
        .clone()
        .expect("受管成员应公开 Bundle ID");
    assert!(
        matching
            .iter()
            .all(|entry| entry.bundle_id.as_deref() == Some(&bundle_id))
    );
    let members = matching
        .into_iter()
        .map(|entry| {
            (
                entry.skill_name,
                InstalledMember {
                    id: entry.member_id.expect("受管成员应公开 Member ID"),
                    expected_target: PathBuf::from(entry.skill_root),
                },
            )
        })
        .collect();
    InstalledBundle {
        id: bundle_id,
        members,
    }
}

fn global_request(
    bundle: &InstalledBundle,
    skill_name: &str,
    app_id: SupportedAppId,
) -> BatchMountRequest {
    BatchMountRequest {
        member_id: bundle.member(skill_name).id.clone(),
        app_id,
        scope: MountScope::Global,
        project_id: None,
    }
}

fn create_batch_plan(
    application: &SkillYardApplication,
    bundle_id: &str,
    requests: Vec<BatchMountRequest>,
) -> BatchMountPlan {
    let UiOutcome::BatchMountPlan { plan } = application
        .handle(UiIntent::CreateBatchMountPlan {
            bundle_id: bundle_id.to_owned(),
            requests,
        })
        .expect("应创建 Batch Mount Plan")
    else {
        panic!("应返回 Batch Mount Plan");
    };
    plan
}

fn confirm_batch_plan(
    application: &SkillYardApplication,
    plan: &BatchMountPlan,
    selected_item_ids: Vec<String>,
) -> UiOutcome {
    application
        .handle(UiIntent::ConfirmBatchMountPlan {
            plan_id: plan.id.clone(),
            selected_item_ids,
        })
        .expect("应确认 Batch Mount Plan")
}

fn selectable_item_ids(plan: &BatchMountPlan) -> Vec<String> {
    plan.items
        .iter()
        .filter(|item| item.selectable)
        .map(|item| item.id.clone())
        .collect()
}

fn plan_item<'a>(
    plan: &'a BatchMountPlan,
    skill_name: &str,
    app_id: SupportedAppId,
) -> &'a skillyard_lib::BatchMountPlanItem {
    plan.items
        .iter()
        .find(|item| item.skill_name == skill_name && item.app_id == app_id)
        .expect("Plan 应包含指定目标")
}

fn inventory_mounts(outcome: &UiOutcome) -> &[MountSummary] {
    let UiOutcome::Inventory { mounts, .. } = outcome else {
        panic!("应返回 Inventory");
    };
    mounts
}

fn inventory_recovery_issues(outcome: &UiOutcome) -> &[RecoveryIssue] {
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = outcome
    else {
        panic!("应返回 Inventory");
    };
    recovery_issues
}

fn inventory_project_id(outcome: &UiOutcome) -> String {
    let UiOutcome::Inventory { projects, .. } = outcome else {
        panic!("应返回 Inventory");
    };
    projects[0].id.clone()
}

fn startup_state(harness: &TestHarness) -> UiOutcome {
    SkillYardApplication::new(harness.paths.clone(), PlatformInfo::supported_for_test())
        .handle(UiIntent::GetStartupState)
        .expect("启动时应完成 Batch Mount 恢复")
}

fn prepared_recovery_batch(root: &Path, label: &str) -> (TestHarness, BatchMountPlan) {
    let harness = ready_harness(root);
    let bundle = install_bundle(&harness, label, &["first", "second"]);
    let plan = create_batch_plan(
        &harness.application,
        &bundle.id,
        vec![
            global_request(&bundle, "first", SupportedAppId::Codex),
            global_request(&bundle, "second", SupportedAppId::ClaudeCode),
        ],
    );
    (harness, plan)
}

fn run_hard_exit_worker(harness: &TestHarness, plan: &BatchMountPlan, point: &str) {
    let selected_item_ids = selectable_item_ids(plan).join(",");
    let status = Command::new(env::current_exe().expect("应定位当前测试二进制"))
        .args(["--exact", "hard_exit_batch_mount_worker", "--nocapture"])
        .env(HARD_EXIT_WORKER, "1")
        .env(HARD_EXIT_DATA_ROOT, &harness.data_root)
        .env(HARD_EXIT_HOME, &harness.home)
        .env(HARD_EXIT_PLAN_ID, &plan.id)
        .env(HARD_EXIT_SELECTED_ITEM_IDS, selected_item_ids)
        .env(HARD_EXIT_POINT, point)
        .status()
        .expect("应启动 Batch Mount hard-exit 子进程");
    assert_eq!(status.code(), Some(92), "子进程必须在指定阶段强制终止");
}

fn plan_targets(plan: &BatchMountPlan) -> Vec<PathBuf> {
    plan.items
        .iter()
        .map(|item| PathBuf::from(&item.target_path))
        .collect()
}

fn symlink_count(paths: &[PathBuf]) -> usize {
    paths
        .iter()
        .filter(|path| {
            fs::symlink_metadata(path)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
        })
        .count()
}

fn batch_stage_entry_count(home: &Path) -> usize {
    batch_temporary_entry_count(home, ".skillyard-batch-stage-")
}

fn batch_rollback_entry_count(home: &Path) -> usize {
    batch_temporary_entry_count(home, ".skillyard-batch-rollback-")
}

fn batch_temporary_entry_count(home: &Path, prefix: &str) -> usize {
    batch_temporary_entries(home, prefix).len()
}

fn batch_temporary_entries(home: &Path, prefix: &str) -> Vec<PathBuf> {
    // Batch Mount 只会在三个固定的可写 Host 根目录中留下暂存或隔离项。
    [".codex/skills", ".claude/skills", ".copilot/skills"]
        .iter()
        .filter_map(|relative| fs::read_dir(home.join(relative)).ok())
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        .map(|entry| entry.path())
        .collect()
}

fn batch_managed_discard_entry_count(data_root: &Path) -> usize {
    batch_managed_discard_entries(data_root).len()
}

fn batch_managed_discard_entries(data_root: &Path) -> Vec<PathBuf> {
    // 每个 Batch 事务只能在自己的固定 discard 目录留下预期条目。
    fs::read_dir(data_root.join("staging"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|transaction| fs::read_dir(transaction.path().join("discard")).ok())
        .flat_map(|entries| entries.filter_map(Result::ok))
        .map(|entry| entry.path())
        .collect()
}

fn assert_link(path: &Path, expected_target: &Path) {
    assert_eq!(
        fs::read_link(path).expect("Mount 必须是软链接"),
        expected_target
    );
}

fn assert_missing(path: &Path) {
    let error = fs::symlink_metadata(path).expect_err("路径应保持不存在");
    assert_eq!(error.kind(), ErrorKind::NotFound);
}

fn database_path(data_root: &Path) -> PathBuf {
    data_root.join("skillyard.sqlite3")
}

fn mount_row_count(data_root: &Path) -> i64 {
    row_count(data_root, "mounts")
}

fn row_count(data_root: &Path, table: &str) -> i64 {
    let connection = Connection::open(database_path(data_root)).expect("应打开测试 SQLite");
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("应读取测试表行数")
}

fn assert_directory_empty(path: &Path) {
    let has_entries = fs::read_dir(path)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    assert!(
        !has_entries,
        "事务结束后不应残留 Journal：{}",
        path.display()
    );
}
