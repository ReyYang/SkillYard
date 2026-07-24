use std::{
    fs,
    path::{Path, PathBuf},
};

use skillyard_lib::{
    ApplicationPaths, BundleUpdateStatus, LifecycleFailpoint, MountScope, PlatformInfo,
    RemovalKind, SkillYardApplication, SourceKind, SourceSummary, SupportedAppId, UiIntent,
    UiOutcome,
};
use tempfile::tempdir;

#[test]
fn project_removal_removes_all_project_mounts_but_preserves_other_mounts_and_bundles() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _home) = ready_application(sandbox.path());
    let first = install_editable(&application, sandbox.path(), "project-first");
    let second = install_editable(&application, sandbox.path(), "project-second");
    let project = register_project(&application, sandbox.path().join("workspace/project"));
    mount(
        &application,
        &first.member_id,
        SupportedAppId::Codex,
        MountScope::Project,
        Some(&project.id),
    );
    mount(
        &application,
        &second.member_id,
        SupportedAppId::ClaudeCode,
        MountScope::Project,
        Some(&project.id),
    );
    mount(
        &application,
        &first.member_id,
        SupportedAppId::GitHubCopilot,
        MountScope::Global,
        None,
    );
    let global_target = sandbox.path().join("home/.copilot/skills/project-first");
    assert!(global_target.is_symlink());

    let UiOutcome::RemovalPlan { plan } = application
        .handle(UiIntent::CreateProjectRemovalPlan {
            project_id: project.id.clone(),
        })
        .expect("应生成 Project Removal Plan")
    else {
        panic!("应返回 RemovalPlan");
    };
    assert_eq!(plan.kind, RemovalKind::Project);
    assert_eq!(plan.mounts.len(), 2);
    assert_eq!(plan.affected_bundles.len(), 2);

    let UiOutcome::Inventory {
        projects, mounts, ..
    } = application
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect("应事务性移除 Project")
    else {
        panic!("Project 删除后应返回 Inventory");
    };
    assert!(projects.iter().all(|item| item.id != project.id));
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].app_id, SupportedAppId::GitHubCopilot);
    assert!(global_target.is_symlink());
    assert!(!project.root.join(".codex/skills/project-first").exists());
    assert!(!project.root.join(".claude/skills/project-second").exists());
    assert_eq!(
        managed_payload(&data_root, &first.bundle_id, "project-first"),
        "old"
    );
    assert_eq!(
        managed_payload(&data_root, &second.bundle_id, "project-second"),
        "old"
    );
}

#[test]
fn source_removal_discards_pending_plan_and_preserves_all_local_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let installed = install_editable(&application, sandbox.path(), "source-delete");
    mount(
        &application,
        &installed.member_id,
        SupportedAppId::Codex,
        MountScope::Global,
        None,
    );
    let original_source =
        fs::read_to_string(installed.path.join("skills/source-delete/payload.txt"))
            .expect("应读取 Editable Local 原目录");
    fs::write(
        installed.path.join("skills/source-delete/payload.txt"),
        "changed",
    )
    .expect("应制造可更新内容");
    application
        .handle(UiIntent::CheckEditableLocalBundle {
            bundle_id: installed.bundle_id.clone(),
        })
        .expect("应显式检查 Editable Local");
    let UiOutcome::InstallPlan { .. } = application
        .handle(UiIntent::CreateBundleUpdatePlan {
            bundle_id: installed.bundle_id.clone(),
        })
        .expect("应留下真实 pending InstallPlan/FK")
    else {
        panic!("应返回 InstallPlan");
    };

    let UiOutcome::RemovalPlan { plan } = application
        .handle(UiIntent::CreateSourceRemovalPlan {
            source_id: installed.source.id.clone(),
        })
        .expect("应生成 Source Removal Plan")
    else {
        panic!("应返回 RemovalPlan");
    };
    assert_eq!(plan.kind, RemovalKind::Source);
    assert_eq!(plan.affected_bundles.len(), 1);
    assert_eq!(
        plan.preserved_external_paths,
        vec![
            fs::canonicalize(&installed.path)
                .expect("应规范化 Editable Local 原目录")
                .to_string_lossy()
                .into_owned()
        ]
    );

    let UiOutcome::SourceDiscovery { sources, .. } = application
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect("应删除 Source metadata 并处理 pending Plan")
    else {
        panic!("Source 删除后应返回 SourceDiscovery");
    };
    assert!(
        sources
            .iter()
            .all(|source| source.id != installed.source.id)
    );
    let UiOutcome::Inventory {
        mounts,
        bundle_updates,
        ..
    } = application
        .handle(UiIntent::GetStartupState)
        .expect("删除 Source 后应读取本机受管状态")
    else {
        panic!("应返回 Inventory");
    };
    assert_eq!(mounts.len(), 1);
    assert!(
        home.join(".codex/skills/source-delete").is_symlink(),
        "Source 删除不能改变 Mount"
    );
    assert_eq!(
        managed_payload(&data_root, &installed.bundle_id, "source-delete"),
        "old",
        "未确认的更新快照不能改变 Current Content"
    );
    assert_eq!(
        fs::read_to_string(installed.path.join("skills/source-delete/payload.txt"))
            .expect("Editable Local 原目录必须保留"),
        "changed"
    );
    assert_ne!(original_source, "changed");
    assert_eq!(
        bundle_updates
            .iter()
            .find(|summary| summary.bundle_id == installed.bundle_id)
            .expect("Bundle 必须继续存在")
            .status,
        BundleUpdateStatus::NoSource
    );
}

#[test]
fn recommended_source_uses_the_same_delete_flow_and_stays_deleted_after_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let source = open_sources(&application)
        .into_iter()
        .find(|source| source.id == "source-anthropics-skills")
        .expect("首次扫描后应存在推荐 Source");
    let UiOutcome::RemovalPlan { plan } = application
        .handle(UiIntent::CreateSourceRemovalPlan {
            source_id: source.id.clone(),
        })
        .expect("推荐 Source 必须使用普通 Source 删除流程")
    else {
        panic!("应返回 RemovalPlan");
    };
    let UiOutcome::SourceDiscovery { sources, .. } = application
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect("应删除推荐 Source")
    else {
        panic!("应返回 SourceDiscovery");
    };
    assert!(sources.iter().all(|item| item.id != source.id));
    drop(application);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    assert!(
        open_sources(&restarted)
            .iter()
            .all(|item| item.id != source.id),
        "重启不能重新播种用户已经删除的推荐 Source"
    );
}

#[test]
fn bundle_removal_has_read_only_preview_and_preserves_its_source_and_editable_directory() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let installed = install_editable(&application, sandbox.path(), "bundle-delete");
    mount(
        &application,
        &installed.member_id,
        SupportedAppId::Codex,
        MountScope::Global,
        None,
    );
    let current_before = fs::read_link(
        data_root
            .join("bundles")
            .join(&installed.bundle_id)
            .join("current"),
    )
    .expect("应读取删除前 current");
    let mount_before =
        fs::read_link(home.join(".codex/skills/bundle-delete")).expect("应读取删除前 Mount");

    let UiOutcome::RemovalPlan { plan } = application
        .handle(UiIntent::CreateBundleRemovalPlan {
            bundle_id: installed.bundle_id.clone(),
        })
        .expect("应生成 Bundle Cascading Delete Plan")
    else {
        panic!("应返回 RemovalPlan");
    };
    assert_eq!(plan.kind, RemovalKind::Bundle);
    assert_eq!(plan.members.len(), 1);
    assert_eq!(plan.mounts.len(), 1);
    assert_eq!(
        plan.preserved_source
            .as_ref()
            .expect("关联 Source 必须明确保留")
            .id,
        installed.source.id
    );
    assert_eq!(
        fs::read_link(
            data_root
                .join("bundles")
                .join(&installed.bundle_id)
                .join("current")
        )
        .expect("Plan 不能修改 current"),
        current_before
    );
    assert_eq!(
        fs::read_link(home.join(".codex/skills/bundle-delete")).expect("Plan 不能修改 Mount"),
        mount_before
    );

    let UiOutcome::Inventory {
        entries, mounts, ..
    } = application
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect("最终 opaque planId 确认应执行 Cascading Delete")
    else {
        panic!("Bundle 删除后应返回 Inventory");
    };
    assert!(
        entries
            .iter()
            .all(|entry| entry.member_id.as_deref() != Some(&installed.member_id))
    );
    assert!(mounts.is_empty());
    assert!(!home.join(".codex/skills/bundle-delete").exists());
    assert!(
        !data_root
            .join("bundles")
            .join(&installed.bundle_id)
            .exists()
    );
    assert!(
        installed.path.exists(),
        "Bundle 删除不能删除 Editable Local 原目录"
    );
    let sources = open_sources(&application);
    let preserved = sources
        .iter()
        .find(|source| source.id == installed.source.id)
        .expect("Bundle 删除后 Source 必须保留");
    assert!(preserved.bundle_id.is_none());
    let notice = fs::read_to_string(data_root.join("SKILLYARD-INFO.md"))
        .expect("应读取删除后的 Central Store notice");
    assert!(
        !notice.contains(
            &data_root
                .join("bundles")
                .join(&installed.bundle_id)
                .display()
                .to_string()
        ),
        "Notice 不再列出已经删除的受管 Bundle"
    );
    assert!(notice.contains(&installed.source.display_name));
}

#[test]
fn project_without_mounts_can_be_removed_without_touching_its_directory() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _data_root, _home) = ready_application(sandbox.path());
    let project = register_project(&application, sandbox.path().join("workspace/empty-project"));
    fs::write(project.root.join("README.md"), "external project content")
        .expect("应写入 Project 自有内容");

    let plan = project_removal_plan(&application, &project.id);
    assert!(plan.mounts.is_empty());
    let UiOutcome::Inventory { projects, .. } = application
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect("没有 Mount 的 Project 也应可移除")
    else {
        panic!("应返回 Inventory");
    };

    assert!(projects.iter().all(|item| item.id != project.id));
    assert_eq!(
        fs::read_to_string(project.root.join("README.md")).expect("Project 内容必须保留"),
        "external project content"
    );
}

#[test]
fn project_removal_continues_after_all_project_mounts_are_isolated() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let installed = install_editable(&application, sandbox.path(), "project-recovery");
    let project = register_project(
        &application,
        sandbox.path().join("workspace/project-recovery"),
    );
    mount(
        &application,
        &installed.member_id,
        SupportedAppId::Codex,
        MountScope::Project,
        Some(&project.id),
    );
    let plan = project_removal_plan(&application, &project.id);
    drop(application);

    let interrupted = application_with_failpoint(
        &data_root,
        &home,
        LifecycleFailpoint::AfterRemovalMountsIsolated,
    );
    interrupted
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect_err("应在全部 Project Mount 隔离后模拟中断");
    drop(interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        projects,
        entries,
        mounts,
        recovery_issues,
        ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应继续完成 Project 删除")
    else {
        panic!("应返回 Inventory");
    };
    assert!(projects.iter().all(|item| item.id != project.id));
    assert!(mounts.is_empty());
    assert!(recovery_issues.is_empty());
    assert!(
        entries
            .iter()
            .any(|entry| entry.member_id.as_deref() == Some(&installed.member_id)),
        "移除 Project 不能删除 Bundle Member"
    );
    assert_eq!(
        managed_payload(&data_root, &installed.bundle_id, "project-recovery"),
        "old"
    );
}

#[test]
fn project_removal_continues_when_journal_is_ahead_of_sqlite_phase() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let installed = install_editable(&application, sandbox.path(), "project-journal-ahead");
    let project = register_project(
        &application,
        sandbox.path().join("workspace/project-journal-ahead"),
    );
    mount(
        &application,
        &installed.member_id,
        SupportedAppId::Codex,
        MountScope::Project,
        Some(&project.id),
    );
    let plan = project_removal_plan(&application, &project.id);
    drop(application);

    let interrupted = application_with_failpoint(
        &data_root,
        &home,
        LifecycleFailpoint::AfterRemovalMountsJournalWrittenBeforePhase,
    );
    interrupted
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect_err("应模拟 Journal 已持久化、SQLite 尚未推进的退出窗口");
    drop(interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        projects,
        mounts,
        recovery_issues,
        ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("确认后的 Project Removal 应从持久 Journal 继续")
    else {
        panic!("应返回 Inventory");
    };
    assert!(projects.iter().all(|item| item.id != project.id));
    assert!(mounts.is_empty());
    assert!(recovery_issues.is_empty());
}

#[test]
fn cascading_removal_rejects_an_unavailable_project_mount_parent() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _data_root, _home) = ready_application(sandbox.path());
    let installed = install_editable(&application, sandbox.path(), "unavailable-parent");
    let project = register_project(
        &application,
        sandbox.path().join("workspace/unavailable-parent"),
    );
    mount(
        &application,
        &installed.member_id,
        SupportedAppId::Codex,
        MountScope::Project,
        Some(&project.id),
    );
    let moved_project = sandbox.path().join("workspace/unavailable-parent-moved");
    fs::rename(&project.root, &moved_project).expect("应模拟 Project 根目录在预览前被移动");

    application
        .handle(UiIntent::CreateProjectRemovalPlan {
            project_id: project.id.clone(),
        })
        .expect_err("无法安全检查 Mount 父目录时不能签发级联删除 Plan");

    fs::rename(&moved_project, &project.root).expect("应恢复 Project 根目录");
    let UiOutcome::Inventory {
        projects, mounts, ..
    } = application
        .handle(UiIntent::GetStartupState)
        .expect("拒绝预览后原有登记必须完整保留")
    else {
        panic!("应返回 Inventory");
    };
    assert!(projects.iter().any(|item| item.id == project.id));
    assert_eq!(mounts.len(), 1);
}

#[test]
fn bundle_removal_rolls_back_when_interrupted_before_the_destructive_boundary() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let installed = install_editable(&application, sandbox.path(), "bundle-rollback");
    mount(
        &application,
        &installed.member_id,
        SupportedAppId::Codex,
        MountScope::Global,
        None,
    );
    let mount_path = home.join(".codex/skills/bundle-rollback");
    let plan = bundle_removal_plan(&application, &installed.bundle_id);
    drop(application);

    let interrupted = application_with_failpoint(
        &data_root,
        &home,
        LifecycleFailpoint::AfterRemovalMountsIsolated,
    );
    interrupted
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect_err("应在 Bundle 进入 Trash 前模拟中断");
    assert!(!mount_path.exists(), "中断点的 Mount 已暂时隔离");
    drop(interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        entries,
        mounts,
        recovery_issues,
        ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应回滚尚未越过生效点的 Bundle 删除")
    else {
        panic!("应返回 Inventory");
    };
    assert!(recovery_issues.is_empty());
    assert_eq!(mounts.len(), 1);
    assert!(mount_path.is_symlink());
    assert!(
        entries
            .iter()
            .any(|entry| entry.member_id.as_deref() == Some(&installed.member_id))
    );
    assert_eq!(
        managed_payload(&data_root, &installed.bundle_id, "bundle-rollback"),
        "old"
    );
}

#[test]
fn blocked_bundle_removal_prevents_related_writes_but_not_unrelated_bundles() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let installed = install_editable(&application, sandbox.path(), "blocked-removal");
    let unrelated = install_editable(&application, sandbox.path(), "unrelated-removal");
    mount(
        &application,
        &installed.member_id,
        SupportedAppId::Codex,
        MountScope::Global,
        None,
    );
    let plan = bundle_removal_plan(&application, &installed.bundle_id);
    let mount_id = plan.mounts[0].id.clone();
    drop(application);

    let interrupted = application_with_failpoint(
        &data_root,
        &home,
        LifecycleFailpoint::AfterRemovalMountsIsolated,
    );
    interrupted
        .handle(UiIntent::ConfirmRemovalPlan {
            plan_id: plan.id.clone(),
        })
        .expect_err("应在 Bundle 生效点前模拟中断");
    drop(interrupted);

    let isolated_mount = home
        .join(".codex/skills")
        .join(format!(".skillyard-removal-{}-{mount_id}", plan.id));
    fs::remove_file(&isolated_mount).expect("应移除事务自有隔离链接");
    fs::write(&isolated_mount, "unknown").expect("应制造无法自动回滚的未知占用");

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("无法判断的 Removal 应进入人工恢复")
    else {
        panic!("应返回 Inventory");
    };
    assert_eq!(recovery_issues.len(), 1);

    fs::write(
        installed.path.join("skills/blocked-removal/payload.txt"),
        "changed",
    )
    .expect("应制造相关 Bundle 的本地变化");
    restarted
        .handle(UiIntent::CheckEditableLocalBundle {
            bundle_id: installed.bundle_id.clone(),
        })
        .expect_err("blocked Removal 必须阻止相关 Bundle 更新");
    restarted
        .handle(UiIntent::CreateMountPlan {
            member_id: installed.member_id,
            app_id: SupportedAppId::ClaudeCode,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect_err("blocked Removal 必须阻止相关 Bundle 新增 Mount");

    fs::write(
        unrelated.path.join("skills/unrelated-removal/payload.txt"),
        "changed",
    )
    .expect("应制造无关 Bundle 的本地变化");
    restarted
        .handle(UiIntent::CheckEditableLocalBundle {
            bundle_id: unrelated.bundle_id,
        })
        .expect("blocked Removal 不能阻止无关 Bundle");
}

#[test]
fn blocked_removals_reject_overlapping_bundle_and_project_deletions() {
    let first = tempdir().expect("应创建第一个隔离测试目录");
    let (application, data_root, home) = ready_application(first.path());
    let installed = install_editable(&application, first.path(), "blocked-bundle-overlap");
    let project = register_project(
        &application,
        first.path().join("workspace/blocked-bundle-overlap"),
    );
    mount(
        &application,
        &installed.member_id,
        SupportedAppId::Codex,
        MountScope::Project,
        Some(&project.id),
    );
    let plan = bundle_removal_plan(&application, &installed.bundle_id);
    drop(application);
    block_removal_before_bundle_boundary(&data_root, &home, &plan);
    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    restarted
        .handle(UiIntent::GetStartupState)
        .expect("Bundle Removal 应进入人工恢复");
    restarted
        .handle(UiIntent::CreateProjectRemovalPlan {
            project_id: project.id,
        })
        .expect_err("blocked Bundle Removal 必须阻止共享 Mount 的 Project Removal");

    let second = tempdir().expect("应创建第二个隔离测试目录");
    let (application, data_root, home) = ready_application(second.path());
    let installed = install_editable(&application, second.path(), "blocked-project-overlap");
    let project = register_project(
        &application,
        second.path().join("workspace/blocked-project-overlap"),
    );
    mount(
        &application,
        &installed.member_id,
        SupportedAppId::Codex,
        MountScope::Project,
        Some(&project.id),
    );
    let plan = project_removal_plan(&application, &project.id);
    drop(application);
    block_removal_before_bundle_boundary(&data_root, &home, &plan);
    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    restarted
        .handle(UiIntent::GetStartupState)
        .expect("Project Removal 应进入人工恢复");
    restarted
        .handle(UiIntent::CreateBundleRemovalPlan {
            bundle_id: installed.bundle_id,
        })
        .expect_err("blocked Project Removal 必须阻止共享 Mount 的 Bundle Removal");
}

#[test]
fn bundle_removal_finishes_when_interrupted_after_the_destructive_boundary() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let installed = install_editable(&application, sandbox.path(), "bundle-complete");
    mount(
        &application,
        &installed.member_id,
        SupportedAppId::Codex,
        MountScope::Global,
        None,
    );
    let mount_path = home.join(".codex/skills/bundle-complete");
    let plan = bundle_removal_plan(&application, &installed.bundle_id);
    drop(application);

    let interrupted = application_with_failpoint(
        &data_root,
        &home,
        LifecycleFailpoint::AfterRemovalBundleIsolated,
    );
    interrupted
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect_err("应在 Bundle 越过破坏性边界后模拟中断");
    assert!(
        !data_root
            .join("bundles")
            .join(&installed.bundle_id)
            .exists()
    );
    drop(interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        entries,
        mounts,
        recovery_issues,
        ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应继续完成已经生效的 Bundle 删除")
    else {
        panic!("应返回 Inventory");
    };
    assert!(recovery_issues.is_empty());
    assert!(mounts.is_empty());
    assert!(!mount_path.exists());
    assert!(
        entries
            .iter()
            .all(|entry| entry.member_id.as_deref() != Some(&installed.member_id))
    );
    assert!(
        !data_root
            .join("bundles")
            .join(&installed.bundle_id)
            .exists()
    );
    assert!(
        fs::read_dir(data_root.join("trash"))
            .expect("应读取受控 Trash")
            .next()
            .is_none(),
        "完成恢复后必须清理受控 Trash"
    );
}

#[test]
fn deep_bundle_removal_journal_recovers_after_the_destructive_boundary() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let source_skill = sandbox
        .path()
        .join("sources/deep-bundle-removal/skills/deep-bundle-removal");
    write_skill(&source_skill, "deep-bundle-removal", "deep removal payload");
    let mut nested = source_skill;
    // 深度超过 serde_json 默认递归预算，证明重启恢复不依赖递归清单。
    for _ in 0..96 {
        nested.push("d");
        fs::create_dir(&nested).expect("应创建深层 Skill 目录");
    }
    fs::write(nested.join("deep.txt"), "deep content").expect("应写入深层 Skill 内容");

    let installed = install_editable(&application, sandbox.path(), "deep-bundle-removal");
    let plan = bundle_removal_plan(&application, &installed.bundle_id);
    drop(application);

    let interrupted = application_with_failpoint(
        &data_root,
        &home,
        LifecycleFailpoint::AfterRemovalBundleRenamedBeforeJournal,
    );
    interrupted
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect_err("应在深层 Bundle rename 后、Journal 阶段推进前模拟中断");
    drop(interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        entries,
        recovery_issues,
        ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应从扁平 Journal 完成深层 Bundle 删除")
    else {
        panic!("应返回 Inventory");
    };
    assert!(recovery_issues.is_empty());
    assert!(
        entries
            .iter()
            .all(|entry| entry.member_id.as_deref() != Some(&installed.member_id))
    );
    assert!(
        !data_root
            .join("bundles")
            .join(&installed.bundle_id)
            .exists()
    );
    assert!(
        fs::read_dir(data_root.join("trash"))
            .expect("应读取受控 Trash")
            .next()
            .is_none(),
        "深层 Bundle 恢复后必须清空受控 Trash"
    );
}

#[test]
fn bundle_recovery_preserves_unknown_content_added_after_the_trash_rename() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let installed = install_editable(&application, sandbox.path(), "trash-child-change");
    let plan = bundle_removal_plan(&application, &installed.bundle_id);
    drop(application);

    let interrupted = application_with_failpoint(
        &data_root,
        &home,
        LifecycleFailpoint::AfterRemovalBundleRenamedBeforeJournal,
    );
    interrupted
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect_err("应在 Bundle rename 后、Journal 阶段推进前模拟中断");
    drop(interrupted);

    let trash_directory = fs::read_dir(data_root.join("trash"))
        .expect("应读取受控 Trash")
        .next()
        .expect("应存在待恢复的 Bundle")
        .expect("应读取 Trash 条目")
        .path();
    fs::write(trash_directory.join("unknown.txt"), "external content")
        .expect("应在同一 Trash 根目录内加入未知内容");

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("未知 Trash 内容应阻塞清理")
    else {
        panic!("应返回 Inventory");
    };
    assert_eq!(recovery_issues.len(), 1);
    assert_eq!(
        fs::read_to_string(trash_directory.join("unknown.txt")).expect("未知内容必须保留"),
        "external content"
    );
}

#[test]
fn missing_journal_before_any_filesystem_change_aborts_without_deleting_the_project() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let project = register_project(&application, sandbox.path().join("workspace/journal-gap"));
    let plan = project_removal_plan(&application, &project.id);
    drop(application);

    let interrupted = application_with_failpoint(
        &data_root,
        &home,
        LifecycleFailpoint::AfterRemovalTransactionRecord,
    );
    interrupted
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect_err("应在 SQLite 行写入而 Journal 尚未创建时模拟中断");
    drop(interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        projects,
        recovery_issues,
        ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("Journal 尚未创建时应安全放弃 Removal")
    else {
        panic!("应返回 Inventory");
    };
    assert!(projects.iter().any(|item| item.id == project.id));
    assert!(recovery_issues.is_empty());
    assert!(
        fs::read_dir(data_root.join("journals"))
            .expect("应读取 Journal 目录")
            .next()
            .is_none()
    );
}

#[test]
fn changed_trash_identity_after_commit_is_blocked_without_deleting_unknown_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let installed = install_editable(&application, sandbox.path(), "bundle-blocked-cleanup");
    let unrelated = install_editable(&application, sandbox.path(), "bundle-unrelated");
    let plan = bundle_removal_plan(&application, &installed.bundle_id);
    drop(application);

    let interrupted = application_with_failpoint(
        &data_root,
        &home,
        LifecycleFailpoint::AfterRemovalStateCommitted,
    );
    interrupted
        .handle(UiIntent::ConfirmRemovalPlan { plan_id: plan.id })
        .expect_err("应在领域状态提交后、清理前模拟中断");
    drop(interrupted);

    let trash_root = data_root.join("trash");
    let original_trash = fs::read_dir(&trash_root)
        .expect("应读取受控 Trash")
        .next()
        .expect("应留下待清理 Bundle")
        .expect("应读取 Trash 条目")
        .path();
    let preserved_original = sandbox.path().join("preserved-original-trash");
    fs::rename(&original_trash, &preserved_original).expect("应移走原 Trash 内容");
    fs::create_dir(&original_trash).expect("应创建同名未知目录");
    fs::write(original_trash.join("unknown.txt"), "external content").expect("应写入未知内容");

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("身份不明的 Trash 必须进入人工恢复，而不是删除")
    else {
        panic!("应返回 Inventory");
    };
    assert_eq!(recovery_issues.len(), 1);
    assert_eq!(
        fs::read_to_string(original_trash.join("unknown.txt")).expect("未知内容必须保留"),
        "external content"
    );
    assert!(preserved_original.exists());

    let unrelated_plan = bundle_removal_plan(&restarted, &unrelated.bundle_id);
    assert_eq!(
        unrelated_plan.target_id, unrelated.bundle_id,
        "blocked 事务不能锁死无关 Bundle"
    );
}

#[test]
fn bundle_removal_rejects_a_stale_preview_before_changing_new_mounts() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let installed = install_editable(&application, sandbox.path(), "bundle-stale-preview");
    let plan = bundle_removal_plan(&application, &installed.bundle_id);
    mount(
        &application,
        &installed.member_id,
        SupportedAppId::Codex,
        MountScope::Global,
        None,
    );
    let mount_path = home.join(".codex/skills/bundle-stale-preview");

    application
        .handle(UiIntent::ConfirmRemovalPlan {
            plan_id: plan.id.clone(),
        })
        .expect_err("预览后新增 Mount 必须让旧 Removal Plan 失效");

    assert!(mount_path.is_symlink(), "过期预览不能先隔离新增 Mount");
    assert_eq!(
        managed_payload(&data_root, &installed.bundle_id, "bundle-stale-preview"),
        "old"
    );
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = application
        .handle(UiIntent::DiscardRemovalPlan { plan_id: plan.id })
        .expect("放弃过期预览后应返回完整旧状态")
    else {
        panic!("应返回 Inventory");
    };
    assert_eq!(mounts.len(), 1);
    assert!(
        entries
            .iter()
            .any(|entry| entry.member_id.as_deref() == Some(&installed.member_id))
    );
}

struct InstalledEditable {
    path: PathBuf,
    source: SourceSummary,
    bundle_id: String,
    member_id: String,
}

struct RegisteredProject {
    id: String,
    root: PathBuf,
}

fn ready_application(root: &Path) -> (SkillYardApplication, PathBuf, PathBuf) {
    let data_root = root.join("application-support/SkillYard");
    let home = root.join("home");
    fs::create_dir_all(&home).expect("应创建隔离 home");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应完成首次扫描");
    (application, data_root, home)
}

fn application_with_failpoint(
    data_root: &Path,
    home: &Path,
    failpoint: LifecycleFailpoint,
) -> SkillYardApplication {
    SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.to_path_buf(), home.to_path_buf()),
        PlatformInfo::supported_for_test(),
        failpoint,
    )
}

fn block_removal_before_bundle_boundary(
    data_root: &Path,
    home: &Path,
    plan: &skillyard_lib::RemovalPlan,
) {
    let failpoint = if plan.kind == RemovalKind::Project {
        LifecycleFailpoint::AfterRemovalMountsJournalWrittenBeforePhase
    } else {
        LifecycleFailpoint::AfterRemovalMountsIsolated
    };
    let interrupted = application_with_failpoint(data_root, home, failpoint);
    interrupted
        .handle(UiIntent::ConfirmRemovalPlan {
            plan_id: plan.id.clone(),
        })
        .expect_err("应在 Bundle 生效点前模拟中断");
    drop(interrupted);
    let isolated_mount = Path::new(&plan.mounts[0].target_path).with_file_name(format!(
        ".skillyard-removal-{}-{}",
        plan.id, plan.mounts[0].id
    ));
    fs::remove_file(&isolated_mount).expect("应移除事务自有隔离链接");
    fs::write(isolated_mount, "unknown").expect("应制造无法自动回滚的未知占用");
}

fn project_removal_plan(
    application: &SkillYardApplication,
    project_id: &str,
) -> skillyard_lib::RemovalPlan {
    let UiOutcome::RemovalPlan { plan } = application
        .handle(UiIntent::CreateProjectRemovalPlan {
            project_id: project_id.to_owned(),
        })
        .expect("应生成 Project Removal Plan")
    else {
        panic!("应返回 RemovalPlan");
    };
    plan
}

fn bundle_removal_plan(
    application: &SkillYardApplication,
    bundle_id: &str,
) -> skillyard_lib::RemovalPlan {
    let UiOutcome::RemovalPlan { plan } = application
        .handle(UiIntent::CreateBundleRemovalPlan {
            bundle_id: bundle_id.to_owned(),
        })
        .expect("应生成 Bundle Removal Plan")
    else {
        panic!("应返回 RemovalPlan");
    };
    plan
}

fn install_editable(
    application: &SkillYardApplication,
    root: &Path,
    name: &str,
) -> InstalledEditable {
    let path = root.join("sources").join(name);
    write_skill(&path.join("skills").join(name), name, "old");
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateEditableLocalInstallPlan {
            input_path: path.to_string_lossy().into_owned(),
        })
        .expect("Editable Local 应生成安装 Plan")
    else {
        panic!("应返回 InstallPlan");
    };
    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: plan
                .candidates
                .iter()
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("应安装 Editable Local Source")
    else {
        panic!("应返回 Inventory");
    };
    let member_id = entries
        .iter()
        .find(|entry| entry.skill_name == name)
        .and_then(|entry| entry.member_id.clone())
        .expect("应找到已安装 Member");
    let canonical = fs::canonicalize(&path)
        .expect("应规范化 Editable Local")
        .to_string_lossy()
        .into_owned();
    let source = open_sources(application)
        .into_iter()
        .find(|source| source.kind == SourceKind::EditableLocal && source.locator == canonical)
        .expect("应找到刚安装的 Source");
    InstalledEditable {
        path,
        bundle_id: source.bundle_id.clone().expect("Source 应关联 Bundle"),
        source,
        member_id,
    }
}

fn register_project(application: &SkillYardApplication, root: PathBuf) -> RegisteredProject {
    fs::create_dir_all(&root).expect("应创建 Project");
    let canonical = fs::canonicalize(&root)
        .expect("应规范化 Project")
        .to_string_lossy()
        .into_owned();
    let UiOutcome::Inventory { projects, .. } = application
        .handle(UiIntent::RegisterProject {
            root_path: canonical.clone(),
        })
        .expect("应登记 Project")
    else {
        panic!("应返回 Inventory");
    };
    let project = projects
        .into_iter()
        .find(|project| project.root_path == canonical)
        .expect("应找到刚登记的 Project");
    RegisteredProject {
        id: project.id,
        root,
    }
}

fn mount(
    application: &SkillYardApplication,
    member_id: &str,
    app_id: SupportedAppId,
    scope: MountScope,
    project_id: Option<&str>,
) {
    let UiOutcome::MountPlan { plan } = application
        .handle(UiIntent::CreateMountPlan {
            member_id: member_id.to_owned(),
            app_id,
            scope,
            project_id: project_id.map(str::to_owned),
        })
        .expect("应生成 Mount Plan")
    else {
        panic!("应返回 MountPlan");
    };
    application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect("应确认 Mount");
}

fn open_sources(application: &SkillYardApplication) -> Vec<SourceSummary> {
    let UiOutcome::SourceDiscovery { sources, .. } = application
        .handle(UiIntent::OpenSourceDiscovery)
        .expect("应读取 Source 列表")
    else {
        panic!("应返回 SourceDiscovery");
    };
    sources
}

fn managed_payload(data_root: &Path, bundle_id: &str, skill_name: &str) -> String {
    fs::read_to_string(
        data_root
            .join("bundles")
            .join(bundle_id)
            .join("current/members")
            .join(skill_name)
            .join("payload.txt"),
    )
    .expect("应读取受管 Skill 内容")
}

fn write_skill(path: &Path, name: &str, payload: &str) {
    fs::create_dir_all(path).expect("应创建 Skill 目录");
    fs::write(
        path.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} fixture\n---\n"),
    )
    .expect("应写入 SKILL.md");
    fs::write(path.join("payload.txt"), payload).expect("应写入 payload");
}
