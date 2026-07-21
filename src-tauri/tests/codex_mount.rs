use std::{
    env, fs,
    os::unix::fs::{MetadataExt, symlink},
    path::Path,
    process::Command,
};

use rusqlite::{Connection, params};
use skillyard_lib::{
    ApplicationPaths, LifecycleFailpoint, MountHealth, MountOperation, MountScope, PlatformInfo,
    SkillYardApplication, SupportedAppId, UiIntent, UiOutcome,
};
use tempfile::tempdir;

const HARD_EXIT_WORKER: &str = "SKILLYARD_MOUNT_HARD_EXIT_WORKER";
const HARD_EXIT_DATA_ROOT: &str = "SKILLYARD_MOUNT_HARD_EXIT_DATA_ROOT";
const HARD_EXIT_HOME: &str = "SKILLYARD_MOUNT_HARD_EXIT_HOME";
const HARD_EXIT_PLAN_ID: &str = "SKILLYARD_MOUNT_HARD_EXIT_PLAN_ID";
const HARD_EXIT_POINT: &str = "SKILLYARD_MOUNT_HARD_EXIT_POINT";

#[test]
fn every_supported_app_uses_its_fixed_global_and_project_mount_paths() {
    let cases = [
        (SupportedAppId::Codex, ".codex/skills"),
        (SupportedAppId::ClaudeCode, ".claude/skills"),
        (SupportedAppId::GitHubCopilot, ".copilot/skills"),
    ];

    for (app_id, global_relative_root) in cases {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let (_paths, application, member_id) =
            installed_skill(sandbox.path(), "supported-app-skill");
        let global = mount_plan_for_app(&application, &member_id, app_id, MountScope::Global, None);
        let global_target = sandbox
            .path()
            .join("home")
            .join(global_relative_root)
            .join("supported-app-skill");
        assert_eq!(global.target_path, global_target.to_string_lossy());
        let mounted = application
            .handle(UiIntent::ConfirmMountPlan { plan_id: global.id })
            .expect("三个 Supported App 都应支持 global Mount");
        assert_eq!(
            fs::read_link(&global_target).expect("global Mount 必须是目录软链接"),
            Path::new(&global.expected_target)
        );

        let remove = remove_mount_plan(&application, &mount_id(&mounted));
        application
            .handle(UiIntent::ConfirmMountPlan { plan_id: remove.id })
            .expect("应先移除 global Mount，再验证 project scope");

        let project = sandbox.path().join("supported-project");
        fs::create_dir(&project).expect("应创建测试 Project");
        let registered = application
            .handle(UiIntent::RegisterProject {
                root_path: project.to_string_lossy().into_owned(),
            })
            .expect("应登记测试 Project");
        let project_id = inventory_projects(&registered)[0].id.clone();
        let project_mount = mount_plan_for_app(
            &application,
            &member_id,
            app_id,
            MountScope::Project,
            Some(project_id),
        );
        let project_relative_root = match app_id {
            SupportedAppId::Codex => ".codex/skills",
            SupportedAppId::ClaudeCode => ".claude/skills",
            SupportedAppId::GitHubCopilot => ".github/skills",
        };
        let project_target = fs::canonicalize(&project)
            .expect("应解析登记后的 Project 路径")
            .join(project_relative_root)
            .join("supported-app-skill");
        assert_eq!(project_mount.target_path, project_target.to_string_lossy());
        application
            .handle(UiIntent::ConfirmMountPlan {
                plan_id: project_mount.id,
            })
            .expect("三个 Supported App 都应支持 project Mount");
        assert_eq!(
            fs::read_link(&project_target).expect("project Mount 必须是目录软链接"),
            Path::new(&project_mount.expected_target)
        );
    }
}

#[test]
fn codex_global_mount_is_planned_created_and_removed_without_touching_bundle_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "global-skill");

    let plan = mount_plan(&application, &member_id, MountScope::Global, None);
    let target = sandbox.path().join("home/.codex/skills/global-skill");
    assert_eq!(plan.operation, MountOperation::Create);
    assert_eq!(plan.target_path, target.to_string_lossy());
    assert_eq!(plan.target_health, MountHealth::Missing);
    assert!(!target.exists(), "Plan 阶段不能创建 Mount 或父目录");

    let created = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect("应确认 global Mount");
    assert_eq!(
        fs::read_link(&target).expect("Mount 必须是软链接"),
        Path::new(&plan.expected_target)
    );
    assert_eq!(inventory_mount_count(&created), 1);
    let notice_path = sandbox.path().join("data/SKILLYARD-INFO.md");
    let notice = fs::read_to_string(&notice_path).expect("Mount 创建后应更新中央目录说明");
    assert!(notice.contains(target.to_str().unwrap()));

    let remove = remove_mount_plan(&application, &mount_id(&created));
    assert_eq!(remove.operation, MountOperation::Remove);
    application
        .handle(UiIntent::ConfirmMountPlan { plan_id: remove.id })
        .expect("应移除 global Mount");
    assert!(!target.exists());
    assert!(
        Path::new(&plan.expected_target).is_dir(),
        "移除 Mount 不能删除主副本"
    );
    let reopened = application
        .handle(UiIntent::GetStartupState)
        .expect("应重开清单");
    assert_eq!(inventory_mount_count(&reopened), 0);
    let notice = fs::read_to_string(notice_path).expect("Mount 移除后应更新中央目录说明");
    assert!(!notice.contains(target.to_str().unwrap()));
    assert!(notice.contains("未挂载"));
}

#[test]
fn local_refresh_does_not_duplicate_a_managed_mount_as_takeover_candidate() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "refresh-mounted");
    let plan = mount_plan(&application, &member_id, MountScope::Global, None);
    application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect("应创建 Mount");

    let refreshed = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("应主动刷新本机清单");
    let matching = inventory_entries(&refreshed)
        .iter()
        .filter(|entry| entry.skill_name == "refresh-mounted")
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 1);
    assert_eq!(
        matching[0].management_kind,
        skillyard_lib::ManagementKind::SkillYardManaged
    );
    let UiOutcome::Inventory {
        last_local_refresh: Some(summary),
        ..
    } = refreshed
    else {
        panic!("刷新后应返回摘要");
    };
    assert_eq!(summary.added, 0, "Mount 路径不能计为新增待接管 Skill");
}

#[test]
fn registered_project_is_read_only_until_project_mount_is_confirmed() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "project-skill");
    let project = sandbox.path().join("sample-project");
    fs::create_dir(&project).expect("应创建 Project");
    fs::write(project.join("README.md"), "keep").expect("应创建 Project 原内容");

    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记 Project");
    let project_id = inventory_projects(&registered)[0].id.clone();
    assert!(
        !project.join(".codex").exists(),
        "登记 Project 不能写入项目目录"
    );

    let plan = mount_plan(
        &application,
        &member_id,
        MountScope::Project,
        Some(project_id),
    );
    assert!(
        !project.join(".codex").exists(),
        "Mount Plan 不能创建父目录"
    );
    application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect("应创建 project Mount");

    let target = project.join(".codex/skills/project-skill");
    assert_eq!(
        fs::read_link(target).expect("project Mount 必须是软链接"),
        Path::new(&plan.expected_target)
    );
    assert_eq!(
        fs::read_to_string(project.join("README.md")).unwrap(),
        "keep"
    );
}

#[test]
fn mount_conflict_and_plan_race_never_overwrite_existing_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "conflict-skill");
    let target = sandbox.path().join("home/.codex/skills/conflict-skill");
    fs::create_dir_all(target.parent().unwrap()).expect("应创建 Codex 根目录");
    fs::write(&target, "external").expect("应创建外部占用文件");

    let error = application
        .handle(UiIntent::CreateMountPlan {
            member_id: member_id.clone(),
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect_err("普通文件占位必须形成 Mount Conflict");
    assert!(error.to_string().contains("占用"));
    assert_eq!(fs::read_to_string(&target).unwrap(), "external");

    fs::remove_file(&target).expect("应清理测试占用");
    let plan = mount_plan(&application, &member_id, MountScope::Global, None);
    fs::write(&target, "appeared-after-plan").expect("应模拟确认前竞态");
    let error = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect_err("目标变化后必须拒绝旧 Plan");
    assert!(error.to_string().contains("前置状态"));
    assert_eq!(fs::read_to_string(&target).unwrap(), "appeared-after-plan");
}

#[test]
fn removing_a_conflicted_mount_only_forgets_the_relation() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "drifted-skill");
    let create = mount_plan(&application, &member_id, MountScope::Global, None);
    let created = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: create.id })
        .expect("应创建 Mount");
    let target = sandbox.path().join("home/.codex/skills/drifted-skill");
    fs::remove_file(&target).expect("应模拟外部替换 Mount");
    fs::write(&target, "external replacement").expect("应创建外部替代文件");

    let remove = remove_mount_plan(&application, &mount_id(&created));
    assert_eq!(remove.target_health, MountHealth::Conflict);
    let outcome = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: remove.id })
        .expect("冲突路径应只移除 SkillYard 关系");

    assert_eq!(fs::read_to_string(&target).unwrap(), "external replacement");
    assert_eq!(inventory_mount_count(&outcome), 0);
    assert!(Path::new(&remove.expected_target).is_dir());
}

#[test]
fn removing_a_missing_mount_does_not_recreate_its_parent_directories() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "missing-parent");
    let create = mount_plan(&application, &member_id, MountScope::Global, None);
    let mounted = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: create.id })
        .expect("应创建 Mount");
    let codex_root = sandbox.path().join("home/.codex");
    fs::remove_file(codex_root.join("skills/missing-parent")).expect("应移除 Mount");
    fs::remove_dir(codex_root.join("skills")).expect("应移除空 skills 目录");
    fs::remove_dir(&codex_root).expect("应移除空 .codex 目录");

    let remove = remove_mount_plan(&application, &mount_id(&mounted));
    assert_eq!(remove.target_health, MountHealth::Missing);
    let removed = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: remove.id })
        .expect("缺失 Mount 的移除只应清理关系");
    assert_eq!(inventory_mount_count(&removed), 0);
    assert!(!codex_root.exists(), "移除不能重新创建缺失的父目录");
}

#[test]
fn removing_a_mount_with_an_unsafe_parent_preserves_the_external_parent() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "unsafe-remove");
    let create = mount_plan(&application, &member_id, MountScope::Global, None);
    let mounted = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: create.id })
        .expect("应创建 Mount");
    let codex_root = sandbox.path().join("home/.codex");
    fs::remove_file(codex_root.join("skills/unsafe-remove")).expect("应移除 Mount");
    fs::remove_dir(codex_root.join("skills")).expect("应移除空 skills 目录");
    fs::remove_dir(&codex_root).expect("应移除空 .codex 目录");
    fs::write(&codex_root, "external parent").expect("应模拟未知内容占用父路径");

    let remove = remove_mount_plan(&application, &mount_id(&mounted));
    assert_eq!(remove.target_health, MountHealth::Conflict);
    let removed = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: remove.id })
        .expect("危险父路径只能清理 Mount 关系");
    assert_eq!(inventory_mount_count(&removed), 0);
    assert_eq!(fs::read_to_string(codex_root).unwrap(), "external parent");
}

#[test]
fn global_and_project_scope_are_mutually_exclusive_for_the_same_app() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "scope-skill");
    let project = sandbox.path().join("scope-project");
    fs::create_dir(&project).expect("应创建 Project");
    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记 Project");
    let project_id = inventory_projects(&registered)[0].id.clone();
    let global = mount_plan(&application, &member_id, MountScope::Global, None);
    application
        .handle(UiIntent::ConfirmMountPlan { plan_id: global.id })
        .expect("应创建 global Mount");

    let error = application
        .handle(UiIntent::CreateMountPlan {
            member_id,
            app_id: SupportedAppId::Codex,
            scope: MountScope::Project,
            project_id: Some(project_id),
        })
        .expect_err("同一 App 不能同时使用 global 和 project");
    assert!(error.to_string().contains("scope") || error.to_string().contains("Mount"));
    assert!(!project.join(".codex").exists());
}

#[test]
fn the_same_skill_can_mount_into_multiple_registered_projects() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "multi-project");
    let mut project_ids = Vec::new();
    for name in ["project-one", "project-two"] {
        let project = sandbox.path().join(name);
        fs::create_dir(&project).expect("应创建 Project");
        let registered = application
            .handle(UiIntent::RegisterProject {
                root_path: project.to_string_lossy().into_owned(),
            })
            .expect("应登记 Project");
        project_ids.push(inventory_projects(&registered).last().unwrap().id.clone());
    }

    for (name, project_id) in ["project-one", "project-two"].into_iter().zip(project_ids) {
        let plan = mount_plan(
            &application,
            &member_id,
            MountScope::Project,
            Some(project_id),
        );
        application
            .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
            .expect("同一 Skill 应允许挂载到另一个 Project");
        assert_eq!(
            fs::read_link(
                sandbox
                    .path()
                    .join(name)
                    .join(".codex/skills/multi-project")
            )
            .expect("每个 Project 都应拥有独立 Mount"),
            Path::new(&plan.expected_target)
        );
    }

    let inventory = application
        .handle(UiIntent::GetStartupState)
        .expect("应读取最终 Mount 清单");
    assert_eq!(inventory_mount_count(&inventory), 2);
}

#[test]
fn an_existing_correct_link_is_adopted_and_a_missing_mount_can_be_forgotten() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "adopt-link");
    let first_plan = mount_plan(&application, &member_id, MountScope::Global, None);
    let target = sandbox.path().join("home/.codex/skills/adopt-link");
    fs::create_dir_all(target.parent().unwrap()).expect("应创建 Codex Skill 根目录");
    symlink(&first_plan.expected_target, &target).expect("应模拟已经正确存在的 Mount");
    let before_adopt = application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("登记前刷新应观察到现有软链接");
    assert_eq!(
        inventory_entries(&before_adopt)
            .iter()
            .filter(|entry| entry.skill_name == "adopt-link")
            .count(),
        2,
        "登记前应同时保留受管主副本和待接管观察"
    );

    let adopt = mount_plan(&application, &member_id, MountScope::Global, None);
    assert_eq!(adopt.target_health, MountHealth::Healthy);
    let mounted = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: adopt.id })
        .expect("正确软链接应只登记为 Mount");
    assert_eq!(inventory_mount_count(&mounted), 1);
    assert_eq!(
        inventory_entries(&mounted)
            .iter()
            .filter(|entry| entry.skill_name == "adopt-link")
            .count(),
        1,
        "登记后必须在同一事务清理重复观察"
    );

    fs::remove_file(&target).expect("应模拟 Mount 被外部删除");
    let remove = remove_mount_plan(&application, &mount_id(&mounted));
    assert_eq!(remove.target_health, MountHealth::Missing);
    let removed = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: remove.id })
        .expect("缺失的 Mount 应允许只清理关系");
    assert_eq!(inventory_mount_count(&removed), 0);
    assert!(Path::new(&remove.expected_target).is_dir());
}

#[test]
fn project_identity_change_after_plan_is_rejected_without_writing_to_replacement() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "project-race");
    let project = sandbox.path().join("replaceable-project");
    let moved_project = sandbox.path().join("original-project");
    fs::create_dir(&project).expect("应创建 Project");
    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记 Project");
    let project_id = inventory_projects(&registered)[0].id.clone();
    let plan = mount_plan(
        &application,
        &member_id,
        MountScope::Project,
        Some(project_id),
    );

    fs::rename(&project, &moved_project).expect("应移走原 Project");
    fs::create_dir(&project).expect("应在相同路径创建不同 Project");
    let error = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect_err("Project inode 改变后必须拒绝旧 Plan");
    assert!(error.to_string().contains("Project 目录已经变化"));
    assert!(!project.join(".codex").exists());
    assert!(!moved_project.join(".codex").exists());
}

#[test]
fn coordinated_project_row_tampering_cannot_retarget_an_existing_plan() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "project-tamper");
    let project = sandbox.path().join("planned-project");
    let replacement = sandbox.path().join("tampered-project");
    fs::create_dir(&project).expect("应创建原 Project");
    fs::create_dir(&replacement).expect("应创建替代 Project");
    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记原 Project");
    let project_id = inventory_projects(&registered)[0].id.clone();
    let plan = mount_plan(
        &application,
        &member_id,
        MountScope::Project,
        Some(project_id.clone()),
    );

    let metadata = fs::metadata(&replacement).expect("应读取替代 Project 身份");
    let connection =
        Connection::open(sandbox.path().join("data/skillyard.sqlite3")).expect("应打开测试 SQLite");
    connection
        .execute(
            "UPDATE projects SET root_path = ?1, root_device = ?2, root_inode = ?3 WHERE id = ?4",
            params![
                replacement.to_str().unwrap(),
                i64::try_from(metadata.dev()).unwrap(),
                i64::try_from(metadata.ino()).unwrap(),
                project_id,
            ],
        )
        .expect("应模拟 Project 行与路径一起被篡改");

    let error = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect_err("Plan 保存的 Project 身份必须拒绝重定向");
    assert!(error.to_string().contains("前置状态"));
    assert!(!project.join(".codex").exists());
    assert!(!replacement.join(".codex").exists());
}

#[test]
fn a_tampered_create_observation_cannot_register_an_external_file_as_a_mount() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "observation-tamper");
    let plan = mount_plan(&application, &member_id, MountScope::Global, None);
    let target = sandbox.path().join("home/.codex/skills/observation-tamper");
    fs::create_dir_all(target.parent().unwrap()).expect("应创建 Codex Skill 根目录");
    fs::write(&target, "external").expect("应创建外部占用文件");
    let metadata = fs::symlink_metadata(&target).expect("应读取外部文件身份");
    let observation = format!(
        "other:{}:{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.mode(),
        metadata.nlink(),
        metadata.size()
    );
    let connection =
        Connection::open(sandbox.path().join("data/skillyard.sqlite3")).expect("应打开测试 SQLite");
    connection
        .execute(
            "UPDATE mount_plans SET target_observation = ?1 WHERE id = ?2",
            params![observation, plan.id],
        )
        .expect("应模拟 Mount Plan observation 被篡改");

    let error = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect_err("create Plan 不能接受外部内容 observation");
    assert!(error.to_string().contains("前置状态"));
    assert_eq!(fs::read_to_string(&target).unwrap(), "external");
    let inventory = application
        .handle(UiIntent::GetStartupState)
        .expect("应读取未改变的清单");
    assert_eq!(inventory_mount_count(&inventory), 0);
}

#[test]
fn parent_symlink_introduced_after_plan_cannot_redirect_mount_creation() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "parent-race");
    let plan = mount_plan(&application, &member_id, MountScope::Global, None);
    let outside = sandbox.path().join("outside-after-plan");
    fs::create_dir(&outside).expect("应创建外部目录");
    symlink(&outside, sandbox.path().join("home/.codex")).expect("应替换 Mount 祖先");

    let error = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect_err("确认时必须重新拒绝祖先软链接");
    assert!(error.to_string().contains("不安全"));
    assert!(fs::read_dir(outside).unwrap().next().is_none());
}

#[test]
fn interrupted_mount_creation_and_removal_recover_to_the_confirmed_state() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (paths, normal, member_id) = installed_skill(sandbox.path(), "recovery-skill");
    let create = mount_plan(&normal, &member_id, MountScope::Global, None);
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        paths.clone(),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterMountTargetApplied,
    );
    interrupted
        .handle(UiIntent::ConfirmMountPlan { plan_id: create.id })
        .expect_err("应在 Mount effect point 后模拟中断");
    let recovered = normal
        .handle(UiIntent::GetStartupState)
        .expect("应完成创建恢复");
    let target = sandbox.path().join("home/.codex/skills/recovery-skill");
    assert!(
        fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(inventory_mount_count(&recovered), 1);

    let remove = remove_mount_plan(&normal, &mount_id(&recovered));
    let interrupted_remove = SkillYardApplication::new_with_lifecycle_failpoint(
        paths.clone(),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterMountTargetApplied,
    );
    interrupted_remove
        .handle(UiIntent::ConfirmMountPlan { plan_id: remove.id })
        .expect_err("应在移除 effect point 后模拟中断");
    let recovered = normal
        .handle(UiIntent::GetStartupState)
        .expect("应完成移除恢复");
    assert!(!target.exists());
    assert_eq!(inventory_mount_count(&recovered), 0);
}

#[test]
fn a_real_process_exit_before_mount_journal_recovers_to_not_mounted() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (paths, application, member_id) = installed_skill(sandbox.path(), "crash-before-journal");
    let plan = mount_plan(&application, &member_id, MountScope::Global, None);
    drop(application);

    run_hard_exit_mount_worker(
        &sandbox.path().join("data"),
        &sandbox.path().join("home"),
        &plan.id,
        "before-journal",
    );

    let reopened = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("Journal 写入前退出应撤销 Mount 事务");
    assert_eq!(inventory_mount_count(&outcome), 0);
    assert!(
        !sandbox
            .path()
            .join("home/.codex/skills/crash-before-journal")
            .exists()
    );
    assert!(!contains_entries(&sandbox.path().join("data/journals")));
}

#[test]
fn a_real_process_exit_after_mount_journal_recovers_to_not_mounted() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (paths, application, member_id) = installed_skill(sandbox.path(), "crash-after-journal");
    let plan = mount_plan(&application, &member_id, MountScope::Global, None);
    drop(application);

    run_hard_exit_mount_worker(
        &sandbox.path().join("data"),
        &sandbox.path().join("home"),
        &plan.id,
        "after-journal-before-phase",
    );

    let reopened = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("Journal 已写入但 phase 未更新时应自动撤销");
    assert_eq!(inventory_mount_count(&outcome), 0);
    assert!(
        !sandbox
            .path()
            .join("home/.codex/skills/crash-after-journal")
            .exists()
    );
    assert!(!contains_entries(&sandbox.path().join("data/journals")));
}

#[test]
fn journal_pending_does_not_adopt_a_preexisting_correct_link() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (paths, application, member_id) = installed_skill(sandbox.path(), "pending-adopt");
    let first = mount_plan(&application, &member_id, MountScope::Global, None);
    let target = sandbox.path().join("home/.codex/skills/pending-adopt");
    fs::create_dir_all(target.parent().unwrap()).expect("应创建 Codex Skill 根目录");
    symlink(&first.expected_target, &target).expect("应创建预先存在的正确链接");
    let adopt = mount_plan(&application, &member_id, MountScope::Global, None);
    drop(application);

    run_hard_exit_mount_worker(
        &sandbox.path().join("data"),
        &sandbox.path().join("home"),
        &adopt.id,
        "after-journal-before-phase",
    );

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("journal_pending 必须保持确认前关系状态");
    assert_eq!(inventory_mount_count(&outcome), 0);
    assert_eq!(
        fs::read_link(target).unwrap(),
        Path::new(&adopt.expected_target)
    );
}

#[test]
fn journal_pending_does_not_remove_an_already_missing_mount_relation() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (paths, application, member_id) = installed_skill(sandbox.path(), "pending-remove");
    let create = mount_plan(&application, &member_id, MountScope::Global, None);
    let mounted = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: create.id })
        .expect("应先创建 Mount");
    let target = sandbox.path().join("home/.codex/skills/pending-remove");
    fs::remove_file(&target).expect("应模拟已经缺失的 Mount");
    let remove = remove_mount_plan(&application, &mount_id(&mounted));
    drop(application);

    run_hard_exit_mount_worker(
        &sandbox.path().join("data"),
        &sandbox.path().join("home"),
        &remove.id,
        "after-journal-before-phase",
    );

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("journal_pending 必须保留确认前 Mount 记录");
    assert_eq!(inventory_mount_count(&outcome), 1);
    assert!(!target.exists());
}

#[test]
fn a_real_process_exit_after_mount_effect_recovers_to_mounted() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (paths, application, member_id) = installed_skill(sandbox.path(), "crash-after-effect");
    let plan = mount_plan(&application, &member_id, MountScope::Global, None);
    drop(application);

    run_hard_exit_mount_worker(
        &sandbox.path().join("data"),
        &sandbox.path().join("home"),
        &plan.id,
        "after-effect-before-phase",
    );

    let reopened = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let first = reopened
        .handle(UiIntent::GetStartupState)
        .expect("文件系统已生效后退出应完成 Mount 事务");
    assert_eq!(inventory_mount_count(&first), 1);
    assert_eq!(
        fs::read_link(sandbox.path().join("home/.codex/skills/crash-after-effect"))
            .expect("恢复后 Mount 必须是软链接"),
        Path::new(&plan.expected_target)
    );
    assert!(!contains_entries(&sandbox.path().join("data/journals")));
    let notice = fs::read_to_string(sandbox.path().join("data/SKILLYARD-INFO.md"))
        .expect("恢复后应同步中央目录说明");
    assert!(notice.contains("home/.codex/skills/crash-after-effect"));

    let reopened_again = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let second = reopened_again
        .handle(UiIntent::GetStartupState)
        .expect("重复恢复必须幂等");
    assert_eq!(inventory_mount_count(&second), 1);
}

#[test]
fn a_real_process_exit_after_mount_state_commit_finishes_cleanup() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (paths, application, member_id) = installed_skill(sandbox.path(), "crash-after-state");
    let plan = mount_plan(&application, &member_id, MountScope::Global, None);
    drop(application);

    run_hard_exit_mount_worker(
        &sandbox.path().join("data"),
        &sandbox.path().join("home"),
        &plan.id,
        "after-state-before-journal",
    );

    let reopened = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("SQLite 已提交后退出应完成清理");
    assert_eq!(inventory_mount_count(&outcome), 1);
    assert!(!contains_entries(&sandbox.path().join("data/journals")));
}

#[test]
fn a_real_process_exit_after_mount_journal_cleanup_only_forgets_terminal_record() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (paths, application, member_id) = installed_skill(sandbox.path(), "crash-after-cleanup");
    let plan = mount_plan(&application, &member_id, MountScope::Global, None);
    drop(application);

    run_hard_exit_mount_worker(
        &sandbox.path().join("data"),
        &sandbox.path().join("home"),
        &plan.id,
        "after-journal-removed",
    );

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("Journal 已清理后的终态事务应自动 forget");
    assert_eq!(inventory_mount_count(&outcome), 1);
    assert!(
        sandbox
            .path()
            .join("home/.codex/skills/crash-after-cleanup")
            .is_symlink()
    );
    assert!(!contains_entries(&sandbox.path().join("data/journals")));
}

#[test]
fn a_real_process_exit_after_remove_effect_recovers_to_unmounted() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (paths, application, member_id) = installed_skill(sandbox.path(), "crash-remove");
    let create = mount_plan(&application, &member_id, MountScope::Global, None);
    let mounted = application
        .handle(UiIntent::ConfirmMountPlan { plan_id: create.id })
        .expect("应创建待移除 Mount");
    let remove = remove_mount_plan(&application, &mount_id(&mounted));
    drop(application);

    run_hard_exit_mount_worker(
        &sandbox.path().join("data"),
        &sandbox.path().join("home"),
        &remove.id,
        "after-effect-before-phase",
    );

    let reopened = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("移除已生效后退出应完成移除事务");
    assert_eq!(inventory_mount_count(&outcome), 0);
    assert!(
        !sandbox
            .path()
            .join("home/.codex/skills/crash-remove")
            .exists()
    );
    assert!(Path::new(&remove.expected_target).is_dir());
    assert!(!contains_entries(&sandbox.path().join("data/journals")));
}

/// 父测试用精确过滤启动本测试；`_exit` 可模拟跳过析构的真实进程中断。
#[test]
fn hard_exit_mount_worker() {
    if env::var_os(HARD_EXIT_WORKER).is_none() {
        return;
    }
    let data_root = env::var_os(HARD_EXIT_DATA_ROOT).expect("子进程必须收到数据目录");
    let home = env::var_os(HARD_EXIT_HOME).expect("子进程必须收到 home");
    let plan_id = env::var(HARD_EXIT_PLAN_ID).expect("子进程必须收到 Mount Plan ID");
    let failpoint = match env::var(HARD_EXIT_POINT).as_deref() {
        Ok("before-journal") => LifecycleFailpoint::HardExitAfterMountTransactionRecord,
        Ok("after-journal-before-phase") => {
            LifecycleFailpoint::HardExitAfterMountJournalWrittenBeforePhase
        }
        Ok("after-effect-before-phase") => {
            LifecycleFailpoint::HardExitAfterMountTargetAppliedBeforePhase
        }
        Ok("after-state-before-journal") => {
            LifecycleFailpoint::HardExitAfterMountStateCommittedBeforeJournal
        }
        Ok("after-journal-removed") => {
            LifecycleFailpoint::HardExitAfterMountJournalRemovedBeforeForget
        }
        _ => panic!("子进程收到未知 Mount failpoint"),
    };
    let application = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.into(), home.into()),
        PlatformInfo::supported_for_test(),
        failpoint,
    );
    application
        .handle(UiIntent::ConfirmMountPlan { plan_id })
        .expect("hard-exit failpoint 必须在返回前终止进程");
}

#[test]
fn mount_parent_symlink_is_rejected_without_writing_outside_home() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (_paths, application, member_id) = installed_skill(sandbox.path(), "unsafe-parent");
    let outside = sandbox.path().join("outside");
    fs::create_dir(&outside).expect("应创建外部目录");
    symlink(&outside, sandbox.path().join("home/.codex")).expect("应模拟祖先软链接");

    let error = application
        .handle(UiIntent::CreateMountPlan {
            member_id,
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect_err("Mount 祖先软链接必须被拒绝");
    assert!(error.to_string().contains("不安全"));
    assert!(fs::read_dir(outside).unwrap().next().is_none());
}

fn installed_skill(
    root: &Path,
    skill_name: &str,
) -> (ApplicationPaths, SkillYardApplication, String) {
    let home = root.join("home");
    fs::create_dir(&home).expect("应创建测试 home");
    let paths = ApplicationPaths::for_home(root.join("data"), home);
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应完成首次扫描");
    let source = root.join("sources").join(skill_name);
    fs::create_dir_all(&source).expect("应创建 Skill 来源");
    fs::write(
        source.join("SKILL.md"),
        format!("---\nname: {skill_name}\ndescription: Mount 测试 Skill\n---\n"),
    )
    .expect("应写入 Skill");
    let UiOutcome::FolderInstallPlan { plan } = application
        .handle(UiIntent::CreateFolderInstallPlan {
            input_path: source.to_string_lossy().into_owned(),
        })
        .expect("应创建安装 Plan")
    else {
        panic!("应返回安装 Plan");
    };
    let candidate_id = plan.candidates[0].candidate_id.clone();
    let installed = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: vec![candidate_id],
        })
        .expect("应安装测试 Skill");
    let member_id = inventory_entries(&installed)[0]
        .member_id
        .clone()
        .expect("受管条目应公开 Member ID");
    (paths, application, member_id)
}

fn mount_plan(
    application: &SkillYardApplication,
    member_id: &str,
    scope: MountScope,
    project_id: Option<String>,
) -> skillyard_lib::MountPlan {
    mount_plan_for_app(
        application,
        member_id,
        SupportedAppId::Codex,
        scope,
        project_id,
    )
}

fn mount_plan_for_app(
    application: &SkillYardApplication,
    member_id: &str,
    app_id: SupportedAppId,
    scope: MountScope,
    project_id: Option<String>,
) -> skillyard_lib::MountPlan {
    let UiOutcome::MountPlan { plan } = application
        .handle(UiIntent::CreateMountPlan {
            member_id: member_id.to_owned(),
            app_id,
            scope,
            project_id,
        })
        .expect("应创建 Mount Plan")
    else {
        panic!("应返回 Mount Plan");
    };
    plan
}

fn remove_mount_plan(
    application: &SkillYardApplication,
    mount_id: &str,
) -> skillyard_lib::MountPlan {
    let UiOutcome::MountPlan { plan } = application
        .handle(UiIntent::CreateRemoveMountPlan {
            mount_id: mount_id.to_owned(),
        })
        .expect("应创建移除 Mount Plan")
    else {
        panic!("应返回移除 Mount Plan");
    };
    plan
}

fn inventory_entries(outcome: &UiOutcome) -> &[skillyard_lib::InventoryItem] {
    let UiOutcome::Inventory { entries, .. } = outcome else {
        panic!("应返回 Inventory");
    };
    entries
}

fn inventory_projects(outcome: &UiOutcome) -> &[skillyard_lib::ProjectSummary] {
    let UiOutcome::Inventory { projects, .. } = outcome else {
        panic!("应返回 Inventory");
    };
    projects
}

fn inventory_mount_count(outcome: &UiOutcome) -> usize {
    let UiOutcome::Inventory { mounts, .. } = outcome else {
        panic!("应返回 Inventory");
    };
    mounts.len()
}

fn mount_id(outcome: &UiOutcome) -> String {
    let UiOutcome::Inventory { mounts, .. } = outcome else {
        panic!("应返回 Inventory");
    };
    mounts[0].id.clone()
}

fn run_hard_exit_mount_worker(data_root: &Path, home: &Path, plan_id: &str, point: &str) {
    let status = Command::new(env::current_exe().expect("应找到当前测试二进制"))
        .args(["--exact", "hard_exit_mount_worker", "--nocapture"])
        .env(HARD_EXIT_WORKER, "1")
        .env(HARD_EXIT_DATA_ROOT, data_root)
        .env(HARD_EXIT_HOME, home)
        .env(HARD_EXIT_PLAN_ID, plan_id)
        .env(HARD_EXIT_POINT, point)
        .status()
        .expect("应启动 Mount hard-exit 子进程");
    assert_eq!(status.code(), Some(92), "子进程必须在 failpoint 直接退出");
}

fn contains_entries(path: &Path) -> bool {
    path.exists() && fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
}
