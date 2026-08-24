use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, BundleUpdateAction, BundleUpdateStatus, InstallInputKind, InstallMode,
    LifecycleFailpoint, MountScope, PlatformInfo, SkillYardApplication, SupportedAppId, UiIntent,
    UiOutcome,
};
use tempfile::tempdir;

#[test]
fn changed_editable_local_is_checked_then_adopted_as_one_complete_bundle() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let editable = sandbox.path().join("author/editable-bundle");
    write_skill(&editable.join("skills/alpha"), "alpha", "alpha-old");
    write_skill(&editable.join("skills/beta"), "beta", "beta-old");
    let installed = install_editable(&application, &editable);
    let alpha_member_id = managed_member_id(&installed, "alpha");
    mount_codex_global(&application, &alpha_member_id);
    let before = editable_state(&data_root);
    assert_bundle_update(
        &installed,
        &before.bundle_id,
        BundleUpdateStatus::NotChecked,
        Some(BundleUpdateAction::CheckEditableLocal),
    );

    fs::write(editable.join("skills/alpha/payload.txt"), "alpha-updated").expect("应修改作者目录");
    fs::remove_dir_all(editable.join("skills/beta")).expect("应模拟来源移除 beta");
    write_skill(&editable.join("skills/gamma"), "gamma", "gamma-new");
    let checked = application
        .handle(UiIntent::CheckEditableLocalBundle {
            bundle_id: before.bundle_id.clone(),
        })
        .expect("应主动扫描 Editable Local");
    assert_bundle_update(
        &checked,
        &before.bundle_id,
        BundleUpdateStatus::Available,
        Some(BundleUpdateAction::Update),
    );
    let after_check = editable_state(&data_root);
    assert_eq!(after_check.current_target, before.current_target);
    assert_eq!(after_check.adopted_marker, before.adopted_marker);
    assert_ne!(after_check.catalog_marker, before.catalog_marker);
    assert_eq!(
        after_check.checked_marker,
        Some(after_check.catalog_marker.clone())
    );
    assert!(after_check.checked_at.is_some());
    assert_eq!(
        after_check.catalog_generation,
        before.catalog_generation + 1
    );
    assert_eq!(
        fs::read_to_string(home.join(".codex/skills/alpha/payload.txt"))
            .expect("确认前 Mount 应继续可读"),
        "alpha-old",
        "检查只能刷新 Source Catalog，不能改变 current"
    );

    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateBundleUpdatePlan {
            bundle_id: before.bundle_id.clone(),
        })
        .expect("Available Editable Local 应复用 Bundle Update Plan")
    else {
        panic!("应返回 InstallPlan");
    };
    assert_eq!(plan.mode, InstallMode::Update);
    assert_eq!(plan.input_kind, InstallInputKind::EditableLocal);
    assert_eq!(
        plan.update_impact
            .as_ref()
            .expect("应展示更新影响")
            .upstream_url,
        None
    );
    assert_eq!(
        candidate_names(&plan),
        vec!["alpha".to_owned(), "gamma".to_owned()]
    );
    let before_confirmation = editable_state(&data_root);
    assert_eq!(before_confirmation.current_target, before.current_target);
    assert_eq!(before_confirmation.adopted_marker, before.adopted_marker);

    let updated = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: plan
                .candidates
                .iter()
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("确认后应采用完整 Editable Local 快照");
    assert_bundle_update(
        &updated,
        &before.bundle_id,
        BundleUpdateStatus::UpToDate,
        Some(BundleUpdateAction::CheckEditableLocal),
    );
    let after = editable_state(&data_root);
    assert_ne!(after.current_target, before.current_target);
    assert_eq!(after.adopted_marker, after.catalog_marker);
    assert_eq!(after.checked_marker, Some(after.catalog_marker.clone()));
    assert_eq!(
        fs::read_to_string(home.join(".codex/skills/alpha/payload.txt"))
            .expect("既有 Mount 应跟随新 current"),
        "alpha-updated"
    );
    assert_eq!(
        fs::read_to_string(
            data_root
                .join("bundles")
                .join(&before.bundle_id)
                .join("current/members/beta/payload.txt"),
        )
        .expect("来源移除成员应保留"),
        "beta-old"
    );
    assert!(
        inventory_has_unmounted_member(&updated, "gamma"),
        "新增成员必须保持未挂载"
    );

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    let restarted_inventory = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应保留采用结果");
    assert_bundle_update(
        &restarted_inventory,
        &before.bundle_id,
        BundleUpdateStatus::UpToDate,
        Some(BundleUpdateAction::CheckEditableLocal),
    );
    assert!(inventory_has_unmounted_member(
        &restarted_inventory,
        "gamma"
    ));
}

#[test]
fn editable_adoption_recovers_after_current_switch_with_the_checked_catalog_marker() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let editable = sandbox.path().join("author/editable-bundle");
    write_skill(&editable.join("alpha"), "alpha", "old");
    install_editable(&application, &editable);
    let before = editable_state(&data_root);
    fs::write(editable.join("alpha/payload.txt"), "new").expect("应修改作者目录");

    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterCurrentActivated,
    );
    interrupted
        .handle(UiIntent::CheckEditableLocalBundle {
            bundle_id: before.bundle_id.clone(),
        })
        .expect("应先刷新 Editable Catalog");
    let after_check = editable_state(&data_root);
    let UiOutcome::InstallPlan { plan } = interrupted
        .handle(UiIntent::CreateBundleUpdatePlan {
            bundle_id: before.bundle_id.clone(),
        })
        .expect("应生成 Editable Update Plan")
    else {
        panic!("应返回 InstallPlan");
    };
    let error = interrupted
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: plan
                .candidates
                .iter()
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect_err("应在 current 切换后模拟中断");
    assert!(error.to_string().contains("领域状态尚未完成"));
    let interrupted_state = editable_state(&data_root);
    let filesystem_current = fs::read_link(
        data_root
            .join("bundles")
            .join(&before.bundle_id)
            .join("current"),
    )
    .expect("中断后应读取 current 软链接")
    .to_string_lossy()
    .into_owned();
    assert_ne!(filesystem_current, before.current_target);
    assert_eq!(
        interrupted_state.current_target, before.current_target,
        "领域提交前 SQLite 仍保留旧目标"
    );
    assert_eq!(interrupted_state.adopted_marker, before.adopted_marker);
    assert_eq!(
        interrupted_state.catalog_generation,
        after_check.catalog_generation
    );

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let inventory = restarted
        .handle(UiIntent::GetStartupState)
        .expect("启动恢复应完成同一 Update 事务");
    assert_bundle_update(
        &inventory,
        &before.bundle_id,
        BundleUpdateStatus::UpToDate,
        Some(BundleUpdateAction::CheckEditableLocal),
    );
    let recovered = editable_state(&data_root);
    assert_eq!(recovered.adopted_marker, recovered.catalog_marker);
    assert_eq!(
        recovered.catalog_generation,
        after_check.catalog_generation + 1,
        "采用提交应在检查 Catalog 上推进一次 generation"
    );
    assert_eq!(
        fs::read_to_string(
            data_root
                .join("bundles")
                .join(&before.bundle_id)
                .join("current/members/alpha/payload.txt"),
        )
        .expect("恢复后 current 应可读"),
        "new"
    );
}

#[test]
fn unchanged_editable_local_stays_up_to_date_and_does_not_create_an_update_plan() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _home) = ready_application(sandbox.path());
    let editable = sandbox.path().join("author/editable-bundle");
    write_skill(&editable.join("alpha"), "alpha", "unchanged");
    let installed = install_editable(&application, &editable);
    let bundle_id = editable_state(&data_root).bundle_id;
    assert_bundle_update(
        &installed,
        &bundle_id,
        BundleUpdateStatus::NotChecked,
        Some(BundleUpdateAction::CheckEditableLocal),
    );

    let checked = application
        .handle(UiIntent::CheckEditableLocalBundle {
            bundle_id: bundle_id.clone(),
        })
        .expect("未变化目录也应完成主动检查");
    assert_bundle_update(
        &checked,
        &bundle_id,
        BundleUpdateStatus::UpToDate,
        Some(BundleUpdateAction::CheckEditableLocal),
    );
    let error = application
        .handle(UiIntent::CreateBundleUpdatePlan {
            bundle_id: bundle_id.clone(),
        })
        .expect_err("UpToDate 不能创建采用 Plan");
    assert!(
        error.to_string().contains("前置状态"),
        "错误应要求重新检查状态：{error}"
    );
    let pending = Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开 SQLite")
        .query_row(
            "SELECT COUNT(*) FROM install_plans WHERE status = 'pending'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("应读取 pending Plan");
    assert_eq!(pending, 0);
}

#[test]
fn unavailable_editable_local_keeps_current_and_can_be_checked_again() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let editable = sandbox.path().join("author/editable-bundle");
    write_skill(&editable.join("alpha"), "alpha", "stable");
    let installed = install_editable(&application, &editable);
    let alpha_member_id = managed_member_id(&installed, "alpha");
    mount_codex_global(&application, &alpha_member_id);
    let before = editable_state(&data_root);

    let moved = sandbox.path().join("author/temporarily-moved");
    fs::rename(&editable, &moved).expect("应暂时移动作者目录");
    let unavailable = application
        .handle(UiIntent::CheckEditableLocalBundle {
            bundle_id: before.bundle_id.clone(),
        })
        .expect("来源不可用应返回可重试 Inventory");
    assert_bundle_update(
        &unavailable,
        &before.bundle_id,
        BundleUpdateStatus::SourceUnavailable,
        Some(BundleUpdateAction::CheckEditableLocal),
    );
    let unavailable_state = editable_state(&data_root);
    assert_eq!(unavailable_state.current_target, before.current_target);
    assert_eq!(unavailable_state.adopted_marker, before.adopted_marker);
    assert_eq!(unavailable_state.catalog_marker, before.catalog_marker);
    assert_eq!(
        fs::read_to_string(home.join(".codex/skills/alpha/payload.txt"))
            .expect("来源不可用时 Mount 仍应工作"),
        "stable"
    );

    fs::rename(&moved, &editable).expect("应恢复原路径和同一 inode");
    let retried = application
        .handle(UiIntent::CheckEditableLocalBundle {
            bundle_id: before.bundle_id.clone(),
        })
        .expect("恢复后应允许再次检查");
    assert_bundle_update(
        &retried,
        &before.bundle_id,
        BundleUpdateStatus::UpToDate,
        Some(BundleUpdateAction::CheckEditableLocal),
    );
    let retried_state = editable_state(&data_root);
    assert_eq!(retried_state.current_target, before.current_target);
    assert_eq!(retried_state.adopted_marker, before.adopted_marker);
}

#[test]
fn a_different_directory_at_the_same_locator_is_source_unavailable_not_a_relink() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _home) = ready_application(sandbox.path());
    let editable = sandbox.path().join("author/editable-bundle");
    write_skill(&editable.join("alpha"), "alpha", "original");
    install_editable(&application, &editable);
    let before = editable_state(&data_root);

    let original_inode = sandbox.path().join("author/original-inode");
    fs::rename(&editable, &original_inode).expect("应保留原登记目录");
    write_skill(&editable.join("alpha"), "alpha", "different-directory");
    let checked = application
        .handle(UiIntent::CheckEditableLocalBundle {
            bundle_id: before.bundle_id.clone(),
        })
        .expect("identity 不符应转换成 SourceUnavailable");
    assert_bundle_update(
        &checked,
        &before.bundle_id,
        BundleUpdateStatus::SourceUnavailable,
        Some(BundleUpdateAction::CheckEditableLocal),
    );
    let after = editable_state(&data_root);
    assert_eq!(after.current_target, before.current_target);
    assert_eq!(after.adopted_marker, before.adopted_marker);
    assert_eq!(after.catalog_marker, before.catalog_marker);
    assert_eq!(after.catalog_generation, before.catalog_generation);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditableState {
    bundle_id: String,
    current_target: String,
    catalog_generation: i64,
    catalog_marker: String,
    adopted_marker: String,
    checked_marker: Option<String>,
    checked_at: Option<i64>,
}

fn editable_state(data_root: &Path) -> EditableState {
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(
            "SELECT bundle.id, bundle.current_target, source.catalog_generation,
                    source.catalog_marker, link.adopted_marker,
                    link.update_checked_marker, link.update_checked_at
             FROM sources AS source
             JOIN source_bundle_links AS link ON link.source_id = source.id
             JOIN bundles AS bundle ON bundle.id = link.bundle_id
             WHERE source.kind = 'editable_local'",
            [],
            |row| {
                Ok(EditableState {
                    bundle_id: row.get(0)?,
                    current_target: row.get(1)?,
                    catalog_generation: row.get(2)?,
                    catalog_marker: row.get(3)?,
                    adopted_marker: row.get(4)?,
                    checked_marker: row.get(5)?,
                    checked_at: row.get(6)?,
                })
            },
        )
        .expect("应读取 Editable Local 持久状态")
}

fn install_editable(application: &SkillYardApplication, editable: &Path) -> UiOutcome {
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateEditableLocalInstallPlan {
            input_path: editable.to_string_lossy().into_owned(),
        })
        .expect("Editable Local 应生成安装 Plan")
    else {
        panic!("应返回 InstallPlan");
    };
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: plan
                .candidates
                .iter()
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("Editable Local 应安装成功")
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

fn write_skill(path: &Path, name: &str, payload: &str) {
    fs::create_dir_all(path).expect("应创建 Skill 目录");
    fs::write(
        path.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} fixture\n---\n"),
    )
    .expect("应写入 SKILL.md");
    fs::write(path.join("payload.txt"), payload).expect("应写入 payload");
}

fn mount_codex_global(application: &SkillYardApplication, member_id: &str) {
    let UiOutcome::MountPlan { plan } = application
        .handle(UiIntent::CreateMountPlan {
            member_id: member_id.to_owned(),
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect("应生成 Codex Mount Plan")
    else {
        panic!("应返回 MountPlan");
    };
    application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect("应确认 Codex Mount");
}

fn managed_member_id(outcome: &UiOutcome, skill_name: &str) -> String {
    let UiOutcome::Inventory { entries, .. } = outcome else {
        panic!("应返回 Inventory");
    };
    entries
        .iter()
        .find(|entry| entry.skill_name == skill_name && entry.member_id.is_some())
        .and_then(|entry| entry.member_id.clone())
        .unwrap_or_else(|| panic!("应找到受管成员 {skill_name}"))
}

fn inventory_has_unmounted_member(outcome: &UiOutcome, skill_name: &str) -> bool {
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = outcome
    else {
        return false;
    };
    entries
        .iter()
        .find(|entry| entry.skill_name == skill_name)
        .and_then(|entry| entry.member_id.as_deref())
        .is_some_and(|member_id| mounts.iter().all(|mount| mount.member_id != member_id))
}

fn assert_bundle_update(
    outcome: &UiOutcome,
    bundle_id: &str,
    status: BundleUpdateStatus,
    action: Option<BundleUpdateAction>,
) {
    let UiOutcome::Inventory { bundle_updates, .. } = outcome else {
        panic!("应返回 Inventory");
    };
    let summary = bundle_updates
        .iter()
        .find(|summary| summary.bundle_id == bundle_id)
        .expect("应包含目标 Bundle 更新摘要");
    assert_eq!(summary.status, status);
    assert_eq!(summary.action, action);
}

fn candidate_names(plan: &skillyard_lib::InstallPlan) -> Vec<String> {
    let mut names = plan
        .candidates
        .iter()
        .filter_map(|candidate| candidate.skill_name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names
}
