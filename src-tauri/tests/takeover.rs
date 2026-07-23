use std::{
    fs,
    os::unix::fs::{MetadataExt, symlink},
    path::Path,
};

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, InventoryLocationKind, LifecycleFailpoint, ManagementKind, MountHealth,
    MountScope, PlatformInfo, SkillYardApplication, SupportedAppId, TakeoverIdentityBasis,
    TakeoverOriginDisposition, TakeoverPlanRequest, UiIntent, UiOutcome,
};
use tempfile::tempdir;

#[test]
fn single_existing_skill_produces_a_read_only_takeover_plan() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/alpha");
    write_skill(&skill_root, "alpha", "接管测试");

    let original_metadata = fs::metadata(&skill_root).expect("应读取原 Skill 元数据");
    let original_content = fs::read(skill_root.join("SKILL.md")).expect("应读取原 Skill 内容");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    let observation_id = match application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    {
        UiOutcome::Inventory { entries, .. } => {
            entries
                .into_iter()
                .find(|entry| entry.skill_name == "alpha")
                .expect("应发现待接管 Skill")
                .id
        }
        _ => panic!("首次扫描应返回 Inventory"),
    };

    let outcome = application
        .handle(UiIntent::CreateTakeoverPlan {
            request: TakeoverPlanRequest {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: vec![observation_id],
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成只读接管计划");
    let UiOutcome::TakeoverPlan { plan } = outcome else {
        panic!("应返回 Takeover Plan");
    };

    assert_eq!(plan.skill_name, "alpha");
    assert_eq!(plan.origins.len(), 1);
    assert_eq!(
        plan.origins[0].final_disposition,
        TakeoverOriginDisposition::Mount
    );
    assert_eq!(plan.origins[0].original_path, path_text(&skill_root));
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(plan.targets[0].app_id, SupportedAppId::Codex);
    assert_eq!(plan.targets[0].scope, MountScope::Global);
    assert_eq!(plan.targets[0].target_path, path_text(&skill_root));
    assert!(plan.source_display_name.is_none());
    assert!(!data_root.join("bundles").join(&plan.bundle_id).exists());

    let after_metadata = fs::metadata(&skill_root).expect("Plan 后原 Skill 必须仍存在");
    assert_eq!(
        (
            after_metadata.dev(),
            after_metadata.ino(),
            after_metadata.mode()
        ),
        (
            original_metadata.dev(),
            original_metadata.ino(),
            original_metadata.mode()
        ),
        "生成 Plan 不能替换原 Skill 目录"
    );
    assert_eq!(
        fs::read(skill_root.join("SKILL.md")).expect("Plan 后应读取原 Skill 内容"),
        original_content,
        "生成 Plan 不能修改原 Skill 内容"
    );
}

#[test]
fn user_selected_origins_form_one_identity_with_one_selected_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let codex_root = home.join(".codex/skills/alpha");
    let claude_root = home.join(".claude/skills/alpha");
    let copilot_root = home.join(".copilot/skills/alpha");
    write_skill(&codex_root, "alpha", "采用这份内容");
    write_skill(&claude_root, "alpha", "会被统一替换");
    write_skill(&copilot_root, "alpha", "未被用户选择");
    let original_files = [
        read_skill_file(&codex_root),
        read_skill_file(&claude_root),
        read_skill_file(&copilot_root),
    ];
    let original_identities = [
        file_identity(&codex_root),
        file_identity(&claude_root),
        file_identity(&copilot_root),
    ];

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    let entries = match application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    {
        UiOutcome::Inventory { entries, .. } => entries,
        _ => panic!("首次扫描应返回 Inventory"),
    };
    let codex_id = observation_id_at(&entries, &codex_root);
    let claude_id = observation_id_at(&entries, &claude_root);
    let copilot_id = observation_id_at(&entries, &copilot_root);

    let outcome = application
        .handle(UiIntent::CreateTakeoverPlan {
            request: TakeoverPlanRequest {
                observation_ids: vec![codex_id.clone(), claude_id.clone()],
                selected_observation_id: codex_id.clone(),
                preserved_observation_ids: vec![codex_id.clone(), claude_id.clone()],
                shared_targets: Vec::new(),
            },
        })
        .expect("显式选择的同名副本应生成一份接管计划");
    let UiOutcome::TakeoverPlan { plan } = outcome else {
        panic!("应返回 Takeover Plan");
    };

    assert_eq!(plan.identity_basis, TakeoverIdentityBasis::UserConfirmed);
    assert_eq!(plan.selected_observation_id, codex_id);
    assert_eq!(plan.skill_description, "采用这份内容");
    assert_eq!(plan.origins.len(), 2);
    assert_eq!(plan.targets.len(), 2);
    assert!(
        plan.origins
            .iter()
            .all(|origin| origin.observation_id != copilot_id),
        "同名不能让未选择的观察自动进入计划"
    );
    assert!(
        plan.targets
            .iter()
            .any(|target| target.app_id == SupportedAppId::Codex)
    );
    assert!(
        plan.targets
            .iter()
            .any(|target| target.app_id == SupportedAppId::ClaudeCode)
    );
    assert!(
        plan.targets
            .iter()
            .all(|target| target.expected_target == plan.expected_target)
    );
    assert!(!data_root.join("bundles").join(&plan.bundle_id).exists());
    assert_eq!(read_skill_file(&codex_root), original_files[0]);
    assert_eq!(read_skill_file(&claude_root), original_files[1]);
    assert_eq!(read_skill_file(&copilot_root), original_files[2]);
    assert_eq!(file_identity(&codex_root), original_identities[0]);
    assert_eq!(file_identity(&claude_root), original_identities[1]);
    assert_eq!(file_identity(&copilot_root), original_identities[2]);
}

#[test]
fn confirming_one_origin_installs_one_bundle_and_preserves_its_mount() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/alpha");
    write_skill(&skill_root, "alpha", "接管后仍可使用");
    let original_content = read_skill_file(&skill_root);
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    let observation_id = match application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    {
        UiOutcome::Inventory { entries, .. } => observation_id_at(&entries, &skill_root),
        _ => panic!("首次扫描应返回 Inventory"),
    };
    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: TakeoverPlanRequest {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: vec![observation_id],
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成单副本计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };

    let outcome = application
        .handle(UiIntent::ConfirmTakeoverPlan {
            plan_id: plan.id.clone(),
        })
        .expect("确认应完成接管");
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = outcome
    else {
        panic!("接管完成后应返回 Inventory");
    };

    let managed = entries
        .iter()
        .find(|entry| entry.member_id.as_deref() == Some(&plan.member_id))
        .expect("Inventory 应展示接管后的受管成员");
    assert_eq!(managed.bundle_id.as_deref(), Some(plan.bundle_id.as_str()));
    assert_eq!(managed.management_kind, ManagementKind::SkillYardManaged);
    assert_eq!(managed.location_kind, InventoryLocationKind::ManagedStore);
    assert!(managed.source_display_name.is_none());
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].health, MountHealth::Healthy);
    assert_eq!(mounts[0].target_path, path_text(&skill_root));

    let root_metadata = fs::symlink_metadata(&skill_root).expect("原路径应成为 Mount");
    assert!(root_metadata.file_type().is_symlink());
    assert_eq!(
        fs::read_link(&skill_root).expect("应读取 Host Mount"),
        Path::new(&plan.expected_target)
    );
    assert!(skill_root.parent().expect("应有 Host 根目录").is_dir());
    assert_eq!(read_skill_file(&skill_root), original_content);

    let managed_directory = Path::new(&plan.managed_directory);
    assert_eq!(
        fs::read_link(managed_directory.join("current")).expect("Bundle 应有 current"),
        Path::new("contents").join(&plan.content_id)
    );
    assert_eq!(
        fs::read(managed_directory.join("current/members/alpha/SKILL.md"))
            .expect("current 应暴露唯一受管内容"),
        original_content
    );
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM bundles),
                (SELECT COUNT(*) FROM skill_members),
                (SELECT COUNT(*) FROM member_selections),
                (SELECT COUNT(*) FROM mounts),
                (SELECT COUNT(*) FROM takeover_plans),
                (SELECT COUNT(*) FROM takeover_transactions)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .expect("应读取接管后的唯一领域记录");
    assert_eq!(counts, (1, 1, 1, 1, 0, 0));

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应读取相同受管状态")
    else {
        panic!("重启后应返回 Inventory");
    };
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
            .count(),
        1
    );
    assert_eq!(mounts.len(), 1);
}

#[test]
fn confirming_one_origin_can_remove_its_existing_mount() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/alpha");
    write_skill(&skill_root, "alpha", "只保留中央主副本");
    let original_content = read_skill_file(&skill_root);
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    let observation_id = match application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    {
        UiOutcome::Inventory { entries, .. } => observation_id_at(&entries, &skill_root),
        _ => panic!("首次扫描应返回 Inventory"),
    };
    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: TakeoverPlanRequest {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id,
                preserved_observation_ids: Vec::new(),
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成排除原 Mount 的计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    assert!(plan.targets.is_empty());

    let UiOutcome::Inventory {
        entries, mounts, ..
    } = application
        .handle(UiIntent::ConfirmTakeoverPlan {
            plan_id: plan.id.clone(),
        })
        .expect("确认应完成接管")
    else {
        panic!("接管完成后应返回 Inventory");
    };

    assert!(
        matches!(fs::symlink_metadata(&skill_root), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
        "用户排除的 Host 位置不能残留断裂软链接"
    );
    assert!(mounts.is_empty());
    assert!(
        entries
            .iter()
            .any(|entry| entry.member_id.as_deref() == Some(plan.member_id.as_str())),
        "中央主副本仍应出现在 Inventory"
    );
    assert_eq!(
        fs::read(Path::new(&plan.managed_directory).join("current/members/alpha/SKILL.md"))
            .expect("应读取中央主副本"),
        original_content
    );
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));
    assert_no_takeover_artifacts(skill_root.parent().expect("应有 Host Skill 根目录"));
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let transaction_counts = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM takeover_plans),
                    (SELECT COUNT(*) FROM takeover_transactions)",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .expect("应读取接管事务清理状态");
    assert_eq!(transaction_counts, (0, 0));

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应读取已提交状态")
    else {
        panic!("重启后应返回 Inventory");
    };
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
            .count(),
        1
    );
    assert!(mounts.is_empty());
}

#[test]
fn confirming_multiple_origins_uses_one_selected_content_everywhere() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let codex_root = home.join(".codex/skills/alpha");
    let claude_root = home.join(".claude/skills/alpha");
    let copilot_root = home.join(".copilot/skills/alpha");
    write_skill(&codex_root, "alpha", "用户选择的唯一内容");
    write_skill(&claude_root, "alpha", "不会形成历史版本");
    write_skill(&copilot_root, "alpha", "用户决定不再挂载");
    let selected_content = read_skill_file(&codex_root);
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    let entries = match application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    {
        UiOutcome::Inventory { entries, .. } => entries,
        _ => panic!("首次扫描应返回 Inventory"),
    };
    let codex_id = observation_id_at(&entries, &codex_root);
    let claude_id = observation_id_at(&entries, &claude_root);
    let copilot_id = observation_id_at(&entries, &copilot_root);
    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: TakeoverPlanRequest {
                // 选中内容故意不放在第一项，确认流程必须按 ID 选择，不能依赖列表顺序。
                observation_ids: vec![claude_id.clone(), codex_id.clone(), copilot_id],
                selected_observation_id: codex_id.clone(),
                preserved_observation_ids: vec![codex_id, claude_id],
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成多副本统一接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };

    let UiOutcome::Inventory {
        entries, mounts, ..
    } = application
        .handle(UiIntent::ConfirmTakeoverPlan {
            plan_id: plan.id.clone(),
        })
        .expect("多副本确认应走同一接管事务")
    else {
        panic!("接管完成后应返回 Inventory");
    };

    for root in [&codex_root, &claude_root] {
        assert_eq!(
            fs::read_link(root).expect("保留位置应成为 Mount"),
            Path::new(&plan.expected_target)
        );
        assert_eq!(read_skill_file(root), selected_content);
        assert_no_takeover_artifacts(root.parent().expect("应有 Host Skill 根目录"));
    }
    assert!(
        matches!(fs::symlink_metadata(&copilot_root), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
        "未保留的位置应被移除，不能留下断裂 Mount"
    );
    assert_no_takeover_artifacts(copilot_root.parent().expect("应有 Host Skill 根目录"));
    assert_eq!(mounts.len(), 2);
    assert!(
        mounts
            .iter()
            .all(|mount| mount.expected_target == plan.expected_target)
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.member_id.as_deref() == Some(plan.member_id.as_str()))
            .count(),
        1
    );
    let contents = Path::new(&plan.managed_directory).join("contents");
    let content_names = fs::read_dir(&contents)
        .expect("应读取 Bundle contents")
        .map(|entry| {
            entry
                .expect("应读取内容目录项")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(content_names, vec![plan.content_id.clone()]);
    assert_eq!(
        fs::read(
            contents
                .join(&plan.content_id)
                .join("members/alpha/SKILL.md")
        )
        .expect("应读取唯一选中内容"),
        selected_content
    );
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应读取统一接管状态")
    else {
        panic!("重启后应返回 Inventory");
    };
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
            .count(),
        1
    );
    assert_eq!(mounts.len(), 2);
}

#[test]
fn interruption_after_first_origin_restores_every_origin_as_one_atomic_takeover() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let codex_root = home.join(".codex/skills/alpha");
    let claude_root = home.join(".claude/skills/alpha");
    write_skill(&codex_root, "alpha", "第一个副本的原始内容");
    write_skill(&claude_root, "alpha", "第二个副本的原始内容");
    let codex_content = read_skill_file(&codex_root);
    let claude_content = read_skill_file(&claude_root);
    let application = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterFirstTakeoverOriginApplied,
    );
    let entries = match application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    {
        UiOutcome::Inventory { entries, .. } => entries,
        _ => panic!("首次扫描应返回 Inventory"),
    };
    let codex_id = observation_id_at(&entries, &codex_root);
    let claude_id = observation_id_at(&entries, &claude_root);
    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: TakeoverPlanRequest {
                observation_ids: vec![codex_id.clone(), claude_id.clone()],
                selected_observation_id: codex_id.clone(),
                preserved_observation_ids: vec![codex_id, claude_id],
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成多副本统一接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };

    application
        .handle(UiIntent::ConfirmTakeoverPlan { plan_id: plan.id })
        .expect_err("第一个副本生效后的模拟中断必须让整个接管失败");

    for (root, content) in [(&codex_root, codex_content), (&claude_root, claude_content)] {
        assert!(
            fs::symlink_metadata(root)
                .expect("每个原始副本都必须恢复")
                .is_dir()
        );
        assert_eq!(read_skill_file(root), content);
        assert_no_takeover_artifacts(root.parent().expect("应有 Host Skill 根目录"));
    }
    assert!(!contains_entries(&data_root.join("bundles")));
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let counts = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM bundles),
                    (SELECT COUNT(*) FROM takeover_plans),
                    (SELECT COUNT(*) FROM takeover_transactions)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .expect("应读取多副本回滚后的数据库状态");
    assert_eq!(counts, (0, 0, 0));
}

#[test]
fn preprogress_interruptions_restore_the_original_and_remove_hidden_artifacts() {
    for failpoint in [
        LifecycleFailpoint::AfterTakeoverOriginMovedBeforeProgress,
        LifecycleFailpoint::AfterTakeoverMountStagedBeforeProgress,
    ] {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let home = sandbox.path().join("home");
        let data_root = sandbox.path().join("application-support/SkillYard");
        let skill_root = home.join(".codex/skills/alpha");
        write_skill(&skill_root, "alpha", "中断后必须完整恢复");
        let original_content = read_skill_file(&skill_root);
        let application = SkillYardApplication::new_with_lifecycle_failpoint(
            ApplicationPaths::for_home(data_root.clone(), home),
            PlatformInfo::supported_for_test(),
            failpoint,
        );
        let observation_id = match application
            .handle(UiIntent::StartInitialScan)
            .expect("首次扫描应成功")
        {
            UiOutcome::Inventory { entries, .. } => observation_id_at(&entries, &skill_root),
            _ => panic!("首次扫描应返回 Inventory"),
        };
        let plan = match application
            .handle(UiIntent::CreateTakeoverPlan {
                request: TakeoverPlanRequest {
                    observation_ids: vec![observation_id.clone()],
                    selected_observation_id: observation_id.clone(),
                    preserved_observation_ids: vec![observation_id],
                    shared_targets: Vec::new(),
                },
            })
            .expect("应生成接管计划")
        {
            UiOutcome::TakeoverPlan { plan } => plan,
            _ => panic!("应返回 Takeover Plan"),
        };

        application
            .handle(UiIntent::ConfirmTakeoverPlan { plan_id: plan.id })
            .expect_err("测试中断必须让确认返回错误");

        assert!(
            fs::symlink_metadata(&skill_root)
                .expect("原 Skill 必须恢复")
                .is_dir()
        );
        assert_eq!(read_skill_file(&skill_root), original_content);
        assert_no_takeover_artifacts(skill_root.parent().expect("应有 Host Skill 根目录"));
        assert!(!contains_entries(&data_root.join("bundles")));
        assert!(!contains_entries(&data_root.join("staging")));
        assert!(!contains_entries(&data_root.join("journals")));
        let connection =
            Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
        let counts = connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM bundles),
                        (SELECT COUNT(*) FROM takeover_plans),
                        (SELECT COUNT(*) FROM takeover_transactions)",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .expect("应读取回滚后的数据库状态");
        assert_eq!(counts, (0, 0, 0));
    }
}

#[test]
fn confirmation_rejects_a_host_ancestor_replaced_by_a_symlink() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/alpha");
    write_skill(&skill_root, "alpha", "祖先路径必须保持真实目录");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    let observation_id = match application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    {
        UiOutcome::Inventory { entries, .. } => observation_id_at(&entries, &skill_root),
        _ => panic!("首次扫描应返回 Inventory"),
    };
    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: TakeoverPlanRequest {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: vec![observation_id],
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };

    let real_codex = home.join(".codex-real");
    fs::rename(home.join(".codex"), &real_codex).expect("应移动真实 Codex 目录");
    symlink(&real_codex, home.join(".codex")).expect("应模拟中间祖先被软链接替换");

    application
        .handle(UiIntent::ConfirmTakeoverPlan {
            plan_id: plan.id.clone(),
        })
        .expect_err("Takeover 必须拒绝软链接祖先，不能沿路径写入");
    assert!(real_codex.join("skills/alpha").is_dir());
    assert!(!data_root.join("bundles").join(plan.bundle_id).exists());
}

fn write_skill(root: &Path, name: &str, description: &str) {
    fs::create_dir_all(root).expect("应创建 Skill 根目录");
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
    )
    .expect("应写入有效 Skill");
}

fn read_skill_file(root: &Path) -> Vec<u8> {
    fs::read(root.join("SKILL.md")).expect("应读取原 Skill 内容")
}

fn file_identity(root: &Path) -> (u64, u64, u32) {
    let metadata = fs::metadata(root).expect("应读取原 Skill 元数据");
    (metadata.dev(), metadata.ino(), metadata.mode())
}

fn contains_entries(path: &Path) -> bool {
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

fn assert_no_takeover_artifacts(parent: &Path) {
    let names = fs::read_dir(parent)
        .expect("应读取 Host Skill 根目录")
        .map(|entry| {
            entry
                .expect("应读取 Host 目录项")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert!(
        names
            .iter()
            .all(|name| !name.starts_with(".skillyard-takeover")),
        "回滚后不能遗留隐藏接管条目：{names:?}"
    );
}

fn observation_id_at(entries: &[skillyard_lib::InventoryItem], root: &Path) -> String {
    entries
        .iter()
        .find(|entry| entry.skill_root == path_text(root))
        .unwrap_or_else(|| panic!("应发现 {}", root.display()))
        .id
        .clone()
}

fn path_text(path: &Path) -> String {
    path.to_str().expect("测试路径应为 UTF-8").to_owned()
}
