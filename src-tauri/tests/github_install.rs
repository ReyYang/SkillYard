use std::{
    collections::VecDeque,
    env, fs,
    io::{self, Cursor, Read, Write},
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, LifecycleFailpoint, ManagementKind, MountScope, PlatformInfo,
    SkillYardApplication, SourceRequest, SourceResponse, SourceTransport, SourceTransportError,
    SupportedAppId, UiIntent, UiOutcome,
};
use tempfile::tempdir;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const HARD_EXIT_WORKER: &str = "SKILLYARD_GITHUB_HARD_EXIT_WORKER";
const HARD_EXIT_DATA_ROOT: &str = "SKILLYARD_GITHUB_HARD_EXIT_DATA_ROOT";
const HARD_EXIT_HOME: &str = "SKILLYARD_GITHUB_HARD_EXIT_HOME";
const HARD_EXIT_PLAN_ID: &str = "SKILLYARD_GITHUB_HARD_EXIT_PLAN_ID";
const HARD_EXIT_CANDIDATE_ID: &str = "SKILLYARD_GITHUB_HARD_EXIT_CANDIDATE_ID";
const HARD_EXIT_POINT: &str = "SKILLYARD_GITHUB_HARD_EXIT_POINT";

#[test]
fn fresh_github_catalog_installs_one_unmounted_source_backed_bundle() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, home) = ready_application(sandbox.path(), transport.clone());
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit))
        .expect("第一个 Source 应加载 Fresh Catalog")
        .id;

    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateGithubInstallPlan {
            source_id: source_id.clone(),
        })
        .expect("Fresh GitHub Catalog 应生成通用安装 Plan")
    else {
        panic!("应返回唯一安装 Plan");
    };
    let mut default_names = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.default_selected)
        .map(|candidate| candidate.skill_name.as_deref().expect("有效成员应有名称"))
        .collect::<Vec<_>>();
    default_names.sort_unstable();
    assert_eq!(default_names, ["alpha", "beta"]);
    assert!(!plan.will_mount);
    assert_eq!(
        transport
            .requests()
            .last()
            .expect("Plan 应重新取得固定内容")
            .url
            .path(),
        format!("/repos/anthropics/skills/zipball/{commit}")
    );

    let selected_candidate_ids = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.default_selected)
        .map(|candidate| candidate.candidate_id.clone())
        .collect::<Vec<_>>();
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids,
        })
        .expect("默认确认应安装全部有效 GitHub 成员")
    else {
        panic!("安装完成后应返回 Inventory");
    };
    let mut installed_names = entries
        .iter()
        .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
        .map(|entry| entry.skill_name.as_str())
        .collect::<Vec<_>>();
    installed_names.sort_unstable();
    assert_eq!(installed_names, ["alpha", "beta"]);
    assert!(
        entries
            .iter()
            .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
            .all(|entry| entry.source_display_name.as_deref() == Some("anthropics/skills")),
        "GitHub 安装后的 Inventory 应展示 Source 名称"
    );
    assert!(mounts.is_empty());
    assert!(!home.join(".codex/skills/alpha").exists());
    assert!(!home.join(".claude/skills/alpha").exists());
    assert!(!home.join(".copilot/skills/alpha").exists());
    assert_eq!(
        fs::read_dir(data_root.join("staging"))
            .expect("应读取 staging")
            .count(),
        0,
        "成功后 Plan 快照与事务临时内容都应清理"
    );
    let notice = fs::read_to_string(data_root.join("SKILLYARD-INFO.md"))
        .expect("安装后应同步更新 Central Store 说明");
    assert!(notice.contains("anthropics/skills"));
    assert!(notice.contains("https://github.com/anthropics/skills"));

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let state = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM bundles),
                (SELECT COUNT(*) FROM skill_members),
                (SELECT COUNT(*) FROM source_bundle_links
                    WHERE source_id = ?1 AND adopted_commit_sha = ?2),
                (SELECT COUNT(*) FROM source_member_links WHERE source_id = ?1),
                (SELECT COUNT(*) FROM mounts)",
            rusqlite::params![&source_id, commit],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .expect("应读取完整 Source-backed Bundle 状态");
    assert_eq!(state, (1, 2, 1, 2, 0));
}

#[test]
fn supplement_keeps_existing_content_mount_and_adopted_commit_while_adding_selected_members() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, home) = ready_application(sandbox.path(), transport.clone());
    let commit_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let commit_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit_a,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit_a))
        .expect("第一个 Source 应加载 commit A")
        .id;

    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let first_plan = create_github_plan(&application, &source_id);
    let alpha_candidate = candidate_id(&first_plan, "alpha");
    let installed = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: first_plan.id,
            selected_candidate_ids: vec![alpha_candidate],
        })
        .expect("首次部分安装应成功");
    let alpha_member_id = managed_member_id(&installed, "alpha");

    let UiOutcome::MountPlan { plan: mount_plan } = application
        .handle(UiIntent::CreateMountPlan {
            member_id: alpha_member_id,
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect("应生成 alpha 的 Codex Mount Plan")
    else {
        panic!("应返回 Mount Plan");
    };
    application
        .handle(UiIntent::ConfirmMountPlan {
            plan_id: mount_plan.id,
        })
        .expect("应挂载 alpha");

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let (bundle_id, old_current_target) = connection
        .query_row(
            "SELECT bundle.id, bundle.current_target
             FROM bundles AS bundle
             JOIN source_bundle_links AS link ON link.bundle_id = bundle.id
             WHERE link.source_id = ?1",
            [&source_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("应读取首次安装基线");
    drop(connection);

    transport.enqueue_catalog("anthropics/skills", "main", commit_b, &supplement_archive());
    application
        .handle(UiIntent::ReloadGitHubSource {
            source_id: source_id.clone(),
        })
        .expect("应把 Catalog 刷新到 commit B");
    transport.enqueue_bytes(200, &supplement_archive());
    let supplement_plan = create_github_plan(&application, &source_id);
    let mut selectable_names = supplement_plan
        .candidates
        .iter()
        .filter(|candidate| candidate.selectable)
        .map(|candidate| candidate.skill_name.as_deref().expect("可选成员应有名称"))
        .collect::<Vec<_>>();
    selectable_names.sort_unstable();
    assert_eq!(selectable_names, ["beta", "gamma"]);
    assert!(
        supplement_plan
            .candidates
            .iter()
            .all(|candidate| candidate.skill_name.as_deref() != Some("alpha")),
        "已安装成员不应再次暴露为用户选择"
    );

    let gamma_candidate = candidate_id(&supplement_plan, "gamma");
    let outcome = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: supplement_plan.id,
            selected_candidate_ids: vec![gamma_candidate],
        })
        .expect("补装 gamma 应原子完成");
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = outcome
    else {
        panic!("补装后应返回 Inventory");
    };
    let mut managed_names = entries
        .iter()
        .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
        .map(|entry| entry.skill_name.as_str())
        .collect::<Vec<_>>();
    managed_names.sort_unstable();
    assert_eq!(managed_names, ["alpha", "gamma"]);
    assert!(
        entries
            .iter()
            .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
            .all(|entry| entry.source_display_name.as_deref() == Some("anthropics/skills")),
        "GitHub 补装后的全部成员应保留同一个 Source 名称"
    );
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].skill_name, "alpha");
    assert_eq!(
        fs::read_to_string(home.join(".codex/skills/alpha/payload.txt"))
            .expect("原 Mount 应继续读取 alpha"),
        "alpha-original",
        "补装不能用上游新版覆盖既有成员"
    );
    assert!(!home.join(".codex/skills/gamma").exists());

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应重新打开真实 SQLite");
    let (new_current_target, adopted_commit, member_count, link_count, mount_count) = connection
        .query_row(
            "SELECT bundle.current_target, link.adopted_commit_sha,
                    (SELECT COUNT(*) FROM skill_members WHERE bundle_id = bundle.id),
                    (SELECT COUNT(*) FROM source_member_links WHERE source_id = link.source_id),
                    (SELECT COUNT(*) FROM mounts)
             FROM bundles AS bundle
             JOIN source_bundle_links AS link ON link.bundle_id = bundle.id
             WHERE bundle.id = ?1",
            [&bundle_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .expect("应读取补装后的完整领域状态");
    assert_ne!(new_current_target, old_current_target);
    assert_eq!(adopted_commit, commit_a, "补装不能推进更新基线");
    assert_eq!((member_count, link_count, mount_count), (2, 2, 1));
    assert!(
        !data_root
            .join("bundles")
            .join(bundle_id)
            .join(old_current_target)
            .exists(),
        "事务完成后旧内容不是回滚版本，应被清理"
    );
}

#[test]
fn catalog_failures_keep_the_old_catalog_and_installed_bundle_unchanged() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, _) = ready_application(sandbox.path(), transport.clone());
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit))
        .expect("应先建立 Fresh Catalog")
        .id;
    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let plan = create_github_plan(&application, &source_id);
    let installed = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: plan
                .candidates
                .iter()
                .filter(|candidate| candidate.default_selected)
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("应先建立已安装 Bundle 基线");
    let (bundle_id, member_id) = match installed {
        UiOutcome::Inventory { entries, .. } => {
            let entry = entries
                .into_iter()
                .find(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
                .expect("应找到已安装成员");
            (
                entry.bundle_id.expect("已安装成员应关联 Bundle"),
                entry.member_id.expect("已安装成员应有稳定 ID"),
            )
        }
        _ => panic!("安装后应返回 Inventory"),
    };
    mount_codex_global(&application, &member_id);
    let current_link = data_root.join("bundles").join(&bundle_id).join("current");
    let mount_path = sandbox.path().join("home/.codex/skills/alpha");
    let old_current_target = fs::read_link(&current_link).expect("Bundle current 应为软链接");
    let old_mount_target = fs::read_link(&mount_path).expect("Codex Mount 应为软链接");

    let assert_preserved = |sources: Vec<skillyard_lib::SourceSummary>| {
        let source = sources
            .iter()
            .find(|source| source.id == source_id)
            .expect("失败后 Source 应继续存在");
        assert_eq!(
            source.catalog_status,
            skillyard_lib::SourceCatalogStatus::Stale
        );
        assert_eq!(source.catalog_commit_sha.as_deref(), Some(commit));
        assert_eq!(source.bundle_id.as_deref(), Some(bundle_id.as_str()));
        assert_eq!(source.members.len(), 2);
        assert_eq!(
            fs::read_link(&current_link).expect("失败后 current 应继续存在"),
            old_current_target
        );
        assert_eq!(
            fs::read_link(&mount_path).expect("失败后 Mount 应继续存在"),
            old_mount_target
        );
    };

    // 没有响应代表 timeout/断网等 transport failure，必须只把旧 Catalog 标成 Stale。
    assert_preserved(reload_sources(&application, &source_id));

    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        &[],
    );
    assert_preserved(reload_sources(&application, &source_id));

    transport.enqueue_catalog_prefix(
        "anthropics/skills",
        "main",
        "cccccccccccccccccccccccccccccccccccccccc",
    );
    transport.enqueue_interrupted(200);
    assert_preserved(reload_sources(&application, &source_id));

    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        "dddddddddddddddddddddddddddddddddddddddd",
        &entry_limit_archive(),
    );
    assert_preserved(reload_sources(&application, &source_id));

    application
        .handle(UiIntent::OpenSourceDiscovery)
        .expect("失败后的旧 Catalog 仍应可浏览");
    let request_count = transport.requests().len();

    let error = application
        .handle(UiIntent::CreateGithubInstallPlan { source_id })
        .expect_err("Stale Catalog 不能签发安装 Plan");
    assert!(error.to_string().contains("状态已经变化"));
    assert_eq!(transport.requests().len(), request_count);
    assert_eq!(staging_entry_count(&data_root), 0);
}

#[test]
fn confirmed_tracked_ref_change_keeps_installed_content_and_mount_unchanged() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, home) = ready_application(sandbox.path(), transport.clone());
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit))
        .expect("应先建立 Fresh Catalog")
        .id;
    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let plan = create_github_plan(&application, &source_id);
    let alpha_candidate_id = candidate_id(&plan, "alpha");
    let installed = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: vec![alpha_candidate_id],
        })
        .expect("应先安装 alpha");
    let member_id = managed_member_id(&installed, "alpha");
    let bundle_id = match &installed {
        UiOutcome::Inventory { entries, .. } => entries
            .iter()
            .find(|entry| entry.member_id.as_deref() == Some(member_id.as_str()))
            .and_then(|entry| entry.bundle_id.clone())
            .expect("alpha 应关联 Bundle"),
        _ => panic!("安装后应返回 Inventory"),
    };

    mount_codex_global(&application, &member_id);

    let current_link = data_root.join("bundles").join(&bundle_id).join("current");
    let mount_path = home.join(".codex/skills/alpha");
    let old_current_target = fs::read_link(&current_link).expect("current 应为软链接");
    let old_mount_target = fs::read_link(&mount_path).expect("Codex Mount 应为软链接");
    let old_payload =
        fs::read_to_string(mount_path.join("payload.txt")).expect("应读取当前 alpha 内容");

    transport.enqueue_catalog_prefix(
        "anthropics/skills",
        "main",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    );
    let UiOutcome::SourceRefChangePlan { plan: ref_plan } = application
        .handle(UiIntent::AddGitHubSource {
            input: "https://github.com/anthropics/skills/tree/next".to_owned(),
            tracked_ref: None,
        })
        .expect("不同 Tracked Ref 应先生成确认 Plan")
    else {
        panic!("不同 Tracked Ref 不能静默切换");
    };
    let UiOutcome::SourceDiscovery { sources, .. } = application
        .handle(UiIntent::ConfirmSourceRefChange {
            plan_id: ref_plan.id,
        })
        .expect("用户确认后应切换 Tracked Ref")
    else {
        panic!("确认后应返回 Source 发现状态");
    };
    let source = sources
        .iter()
        .find(|source| source.id == source_id)
        .expect("切换后 Source 应继续存在");

    assert_eq!(source.tracked_ref, "next");
    assert_eq!(source.bundle_id.as_deref(), Some(bundle_id.as_str()));
    assert_eq!(fs::read_link(&current_link).unwrap(), old_current_target);
    assert_eq!(fs::read_link(&mount_path).unwrap(), old_mount_target);
    assert_eq!(
        fs::read_to_string(mount_path.join("payload.txt")).unwrap(),
        old_payload
    );
}

#[test]
fn a_source_with_every_valid_member_installed_rejects_an_empty_supplement_without_network() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, _) = ready_application(sandbox.path(), transport.clone());
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit))
        .expect("应建立 Fresh Catalog")
        .id;
    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let plan = create_github_plan(&application, &source_id);
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: plan
                .candidates
                .iter()
                .filter(|candidate| candidate.default_selected)
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("应先安装全部成员");
    let request_count = transport.requests().len();

    let error = application
        .handle(UiIntent::CreateGithubInstallPlan { source_id })
        .expect_err("没有未安装成员时不能生成空补装 Plan");
    assert!(error.to_string().contains("没有可补充安装"));
    assert_eq!(transport.requests().len(), request_count);
    assert_eq!(staging_entry_count(&data_root), 0);
}

#[test]
fn changing_the_catalog_after_plan_creation_rejects_confirmation_before_a_transaction() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, _) = ready_application(sandbox.path(), transport.clone());
    let commit_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let commit_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit_a,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit_a))
        .expect("应建立 commit A Catalog")
        .id;
    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let plan = create_github_plan(&application, &source_id);
    let alpha_candidate_id = candidate_id(&plan, "alpha");
    transport.enqueue_catalog("anthropics/skills", "main", commit_b, &supplement_archive());
    application
        .handle(UiIntent::ReloadGitHubSource { source_id })
        .expect("应把 Source 推进到 commit B");

    let error = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: vec![alpha_candidate_id],
        })
        .expect_err("旧 Catalog Plan 不能开始文件系统事务");
    assert!(error.to_string().contains("Source 状态已经变化"));
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM bundles),
                (SELECT COUNT(*) FROM lifecycle_transactions),
                (SELECT COUNT(*) FROM install_plans)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .expect("应读取事务边界");
    assert_eq!(counts, (0, 0, 0));
    assert_eq!(staging_entry_count(&data_root), 0);
}

#[test]
fn an_expired_github_plan_discards_its_snapshot_before_returning() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, _) = ready_application(sandbox.path(), transport.clone());
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit))
        .expect("应建立 Fresh Catalog")
        .id;
    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let plan = create_github_plan(&application, &source_id);
    let alpha_candidate_id = candidate_id(&plan, "alpha");
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    connection
        .execute(
            "UPDATE install_plans SET expires_at = 0 WHERE id = ?1",
            [&plan.id],
        )
        .expect("应使 GitHub Plan 过期");
    drop(connection);

    let error = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: vec![alpha_candidate_id],
        })
        .expect_err("过期 GitHub Plan 必须拒绝确认");
    // 应用入口会先恢复并清理过期的快照，因此确认阶段可能只看到 Plan 已不存在。
    assert!(
        error.to_string().contains("已过期") || error.to_string().contains("未签发"),
        "过期 Plan 必须以明确错误拒绝确认: {error}"
    );
    assert_eq!(staging_entry_count(&data_root), 0);
    let remaining = Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row("SELECT COUNT(*) FROM install_plans", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("应读取剩余 Plan");
    assert_eq!(remaining, 0);
}

#[test]
fn startup_discards_an_expired_github_plan_without_reopening_its_confirmation() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, home) = ready_application(sandbox.path(), transport.clone());
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit))
        .expect("应建立 Fresh Catalog")
        .id;
    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let plan = create_github_plan(&application, &source_id);
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    connection
        .execute(
            "UPDATE install_plans SET expires_at = 0 WHERE id = ?1",
            [&plan.id],
        )
        .expect("应使 Plan 过期");
    drop(connection);
    drop(application);

    let recovered = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    recovered
        .handle(UiIntent::GetStartupState)
        .expect("启动恢复应清理过期 Plan");
    let discard = recovered
        .handle(UiIntent::DiscardInstallPlan { plan_id: plan.id })
        .expect("恢复器已清理过期 Plan 时，用户返回仍应幂等成功");
    assert_eq!(discard, UiOutcome::InstallPlanDiscarded);
    assert_eq!(staging_entry_count(&data_root), 0);
    let remaining = Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row("SELECT COUNT(*) FROM install_plans", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("应读取剩余 Plan");
    assert_eq!(remaining, 0);
}

#[test]
fn creating_a_new_plan_for_the_same_source_replaces_the_old_pending_snapshot() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, _) = ready_application(sandbox.path(), transport.clone());
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit))
        .expect("应建立 Fresh Catalog")
        .id;
    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let first = create_github_plan(&application, &source_id);
    assert_eq!(staging_entry_count(&data_root), 1);

    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let second = create_github_plan(&application, &source_id);
    assert_ne!(second.id, first.id);
    assert_eq!(staging_entry_count(&data_root), 1);
    let remaining = Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row("SELECT COUNT(*) FROM install_plans", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("应读取剩余 Plan");
    assert_eq!(remaining, 1);
}

#[test]
fn user_can_discard_a_pending_github_plan_and_its_snapshot() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, _) = ready_application(sandbox.path(), transport.clone());
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit))
        .expect("应建立 Fresh Catalog")
        .id;
    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let plan = create_github_plan(&application, &source_id);
    let alpha_candidate_id = candidate_id(&plan, "alpha");
    assert_eq!(staging_entry_count(&data_root), 1);

    let outcome = application
        .handle(UiIntent::DiscardInstallPlan {
            plan_id: plan.id.clone(),
        })
        .expect("放弃 pending Plan 应成功");
    assert_eq!(outcome, UiOutcome::InstallPlanDiscarded);
    assert_eq!(staging_entry_count(&data_root), 0);
    let remaining = Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row("SELECT COUNT(*) FROM install_plans", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("应读取剩余 Plan");
    assert_eq!(remaining, 0);

    let error = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: vec![alpha_candidate_id],
        })
        .expect_err("已放弃 Plan 不能再次确认");
    assert!(error.to_string().contains("未签发"));
}

#[test]
fn github_create_hard_exit_before_journal_recovers_to_not_installed() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let prepared = prepare_github_create_hard_exit(sandbox.path());

    run_github_hard_exit_worker(&prepared, "before-journal");
    let outcome = reopen_after_hard_exit(&prepared);
    assert_managed_names(&outcome, &[]);
    assert_eq!(staging_entry_count(&prepared.data_root), 0);
    assert_eq!(bundle_count(&prepared.data_root), 0);
}

#[test]
fn github_create_hard_exit_after_current_recovers_the_complete_bundle() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let prepared = prepare_github_create_hard_exit(sandbox.path());

    run_github_hard_exit_worker(&prepared, "after-current");
    let outcome = reopen_after_hard_exit(&prepared);
    assert_managed_names(&outcome, &["alpha"]);
    assert_eq!(staging_entry_count(&prepared.data_root), 0);
    assert_eq!(bundle_count(&prepared.data_root), 1);
}

#[test]
fn github_supplement_hard_exit_before_journal_preserves_the_old_bundle() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (prepared, bundle_id, old_current_target) =
        prepare_github_supplement_hard_exit(sandbox.path());

    run_github_hard_exit_worker(&prepared, "before-journal");
    let outcome = reopen_after_hard_exit(&prepared);
    assert_managed_names(&outcome, &["alpha"]);
    assert_eq!(
        current_target(&prepared.data_root, &bundle_id),
        old_current_target
    );
    assert_eq!(staging_entry_count(&prepared.data_root), 0);
}

#[test]
fn github_supplement_hard_exit_after_current_finishes_the_new_bundle() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (prepared, bundle_id, old_current_target) =
        prepare_github_supplement_hard_exit(sandbox.path());

    run_github_hard_exit_worker(&prepared, "after-current");
    let outcome = reopen_after_hard_exit(&prepared);
    assert_managed_names(&outcome, &["alpha", "gamma"]);
    assert_ne!(
        current_target(&prepared.data_root, &bundle_id),
        old_current_target
    );
    assert!(
        !prepared
            .data_root
            .join("bundles")
            .join(bundle_id)
            .join(old_current_target)
            .exists(),
        "向前恢复完成后应清理旧内容"
    );
    assert_eq!(staging_entry_count(&prepared.data_root), 0);
}

#[test]
fn github_supplement_retries_an_interrupted_old_content_cleanup() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (prepared, bundle_id, old_current_target) =
        prepare_github_supplement_hard_exit(sandbox.path());
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(prepared.data_root.clone(), prepared.home.clone()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterDomainCommit,
    );
    interrupted
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: prepared.plan_id.clone(),
            selected_candidate_ids: vec![prepared.candidate_id.clone()],
        })
        .expect_err("应停在领域提交之后、旧内容清理之前");

    let journal_path = fs::read_dir(prepared.data_root.join("journals"))
        .expect("应读取 Journal 目录")
        .next()
        .expect("应留下 Journal")
        .expect("应读取 Journal 条目")
        .path();
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).expect("应读取 Journal"))
            .expect("Journal 应为 JSON");
    let transaction_id = journal["transaction_id"]
        .as_str()
        .expect("Journal 应记录 transaction_id");
    let old_content = prepared
        .data_root
        .join("bundles")
        .join(&bundle_id)
        .join(&old_current_target);
    let discard = prepared
        .data_root
        .join("staging")
        .join(transaction_id)
        .join("discarding-previous");
    fs::rename(&old_content, &discard).expect("应模拟旧内容已经原子隔离");
    fs::remove_file(discard.join("members/alpha/payload.txt")).expect("应模拟递归删除进行到一半");

    let outcome = reopen_after_hard_exit(&prepared);
    assert_managed_names(&outcome, &["alpha", "gamma"]);
    assert_eq!(staging_entry_count(&prepared.data_root), 0);
    assert!(!old_content.exists());
}

#[test]
fn blocked_github_supplement_isolates_its_source_and_bundle_but_not_independent_bundles() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, home) = ready_application(sandbox.path(), transport.clone());
    let commit_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let commit_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit_a,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit_a))
        .expect("应加载 commit A")
        .id;
    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let first_plan = create_github_plan(&application, &source_id);
    let alpha_candidate_id = candidate_id(&first_plan, "alpha");
    let installed = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: first_plan.id,
            selected_candidate_ids: vec![alpha_candidate_id],
        })
        .expect("应先建立 GitHub Bundle");
    let alpha_member_id = managed_member_id(&installed, "alpha");

    transport.enqueue_catalog("anthropics/skills", "main", commit_b, &supplement_archive());
    application
        .handle(UiIntent::ReloadGitHubSource {
            source_id: source_id.clone(),
        })
        .expect("应加载 commit B");
    transport.enqueue_bytes(200, &supplement_archive());
    let supplement_plan = create_github_plan(&application, &source_id);
    let gamma_candidate_id = candidate_id(&supplement_plan, "gamma");
    drop(application);

    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterCandidatePrepared,
    );
    interrupted
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: supplement_plan.id,
            selected_candidate_ids: vec![gamma_candidate_id],
        })
        .expect_err("failpoint 应留下 supplement Journal");
    drop(interrupted);

    let journal_path = fs::read_dir(data_root.join("journals"))
        .expect("应读取 Journal 目录")
        .next()
        .expect("应留下 supplement Journal")
        .expect("应读取 supplement Journal")
        .path();
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).expect("应读取 supplement Journal"))
            .expect("supplement Journal 应为 JSON");
    // 破坏 Journal 合同字段，使重启只能阻塞这次 supplement，不能触碰原 Bundle 内容。
    journal["content_relative"] = serde_json::json!("bundles/tampered-by-test");
    fs::write(
        &journal_path,
        serde_json::to_vec_pretty(&journal).expect("应序列化篡改后的 supplement Journal"),
    )
    .expect("应篡改 supplement Journal");

    let restarted = SkillYardApplication::new_with_source_transport(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
        transport.clone(),
    );
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("损坏的 supplement 只应阻塞自身")
    else {
        panic!("重启恢复后应返回 Inventory");
    };
    assert_eq!(recovery_issues.len(), 1);

    let request_count = transport.requests().len();
    let source_error = restarted
        .handle(UiIntent::CreateGithubInstallPlan {
            source_id: source_id.clone(),
        })
        .expect_err("blocked Source 不能创建新的 supplement Plan");
    assert!(source_error.to_string().contains("等待人工恢复"));
    assert_eq!(
        transport.requests().len(),
        request_count,
        "拒绝 blocked Source 必须发生在网络请求前"
    );

    let member_error = restarted
        .handle(UiIntent::CreateMountPlan {
            member_id: alpha_member_id,
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect_err("blocked Bundle 的既有 Member 不能创建 Mount Plan");
    assert!(member_error.to_string().contains("等待人工恢复"));

    let independent_input = sandbox.path().join("downloads/independent-skill");
    fs::create_dir_all(&independent_input).expect("应创建独立 folder Bundle");
    fs::write(
        independent_input.join("SKILL.md"),
        "---\nname: independent-skill\ndescription: independent skill\n---\n# independent-skill\n",
    )
    .expect("应写入独立 Skill metadata");
    fs::write(independent_input.join("payload.txt"), "independent").expect("应写入独立 Skill 内容");
    let UiOutcome::InstallPlan {
        plan: independent_plan,
    } = restarted
        .handle(UiIntent::CreateFolderInstallPlan {
            input_path: independent_input.to_string_lossy().into_owned(),
        })
        .expect("blocked supplement 不能冻结独立 folder Bundle")
    else {
        panic!("独立 folder Bundle 应返回安装 Plan");
    };
    let independent_candidate_id = candidate_id(&independent_plan, "independent-skill");
    let independent_installed = restarted
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: independent_plan.id,
            selected_candidate_ids: vec![independent_candidate_id],
        })
        .expect("独立 folder Bundle 应可安装");
    let independent_member_id = managed_member_id(&independent_installed, "independent-skill");

    let UiOutcome::MountPlan { .. } = restarted
        .handle(UiIntent::CreateMountPlan {
            member_id: independent_member_id,
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect("独立 folder Bundle 的 Member 仍应生成 Mount Plan")
    else {
        panic!("独立 folder Bundle 应返回 Mount Plan");
    };
}

/// 父测试精确启动本用例；`_exit` 跳过析构，验证真实进程退出后的持久化恢复。
#[test]
fn github_hard_exit_worker() {
    if env::var_os(HARD_EXIT_WORKER).is_none() {
        return;
    }
    let data_root = env::var_os(HARD_EXIT_DATA_ROOT).expect("子进程必须收到数据目录");
    let home = env::var_os(HARD_EXIT_HOME).expect("子进程必须收到 home");
    let plan_id = env::var(HARD_EXIT_PLAN_ID).expect("子进程必须收到 Plan ID");
    let candidate_id = env::var(HARD_EXIT_CANDIDATE_ID).expect("子进程必须收到候选成员 ID");
    let failpoint = match env::var(HARD_EXIT_POINT).as_deref() {
        Ok("before-journal") => LifecycleFailpoint::HardExitAfterTransactionRecord,
        Ok("after-current") => LifecycleFailpoint::HardExitAfterCurrentSwitchedBeforePhase,
        _ => panic!("子进程收到未知 failpoint"),
    };
    let application = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.into(), home.into()),
        PlatformInfo::supported_for_test(),
        failpoint,
    );
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id,
            selected_candidate_ids: vec![candidate_id],
        })
        .expect("hard-exit failpoint 必须在返回前终止进程");
}

struct PreparedHardExit {
    data_root: std::path::PathBuf,
    home: std::path::PathBuf,
    plan_id: String,
    candidate_id: String,
}

fn prepare_github_create_hard_exit(base: &Path) -> PreparedHardExit {
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, home) = ready_application(base, transport.clone());
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit))
        .expect("应加载 hard-exit Source")
        .id;
    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let plan = create_github_plan(&application, &source_id);
    let candidate_id = candidate_id(&plan, "alpha");
    drop(application);
    PreparedHardExit {
        data_root,
        home,
        plan_id: plan.id,
        candidate_id,
    }
}

fn prepare_github_supplement_hard_exit(base: &Path) -> (PreparedHardExit, String, String) {
    let transport = Arc::new(RecordingTransport::default());
    let (application, data_root, home) = ready_application(base, transport.clone());
    let commit_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let commit_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        commit_a,
        &bundle_archive("alpha-original", true),
    );
    let source_id = open_sources(&application)
        .into_iter()
        .find(|source| source.catalog_commit_sha.as_deref() == Some(commit_a))
        .expect("应加载 commit A")
        .id;
    transport.enqueue_bytes(200, &bundle_archive("alpha-original", true));
    let first_plan = create_github_plan(&application, &source_id);
    let alpha_candidate_id = candidate_id(&first_plan, "alpha");
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: first_plan.id,
            selected_candidate_ids: vec![alpha_candidate_id],
        })
        .expect("应准备补装基线");
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let (bundle_id, old_current_target) = connection
        .query_row(
            "SELECT bundle.id, bundle.current_target
             FROM bundles AS bundle
             JOIN source_bundle_links AS link ON link.bundle_id = bundle.id
             WHERE link.source_id = ?1",
            [&source_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("应读取旧 Bundle");
    drop(connection);

    transport.enqueue_catalog("anthropics/skills", "main", commit_b, &supplement_archive());
    application
        .handle(UiIntent::ReloadGitHubSource {
            source_id: source_id.clone(),
        })
        .expect("应加载 commit B");
    transport.enqueue_bytes(200, &supplement_archive());
    let plan = create_github_plan(&application, &source_id);
    let candidate_id = candidate_id(&plan, "gamma");
    drop(application);
    (
        PreparedHardExit {
            data_root,
            home,
            plan_id: plan.id,
            candidate_id,
        },
        bundle_id,
        old_current_target,
    )
}

fn run_github_hard_exit_worker(prepared: &PreparedHardExit, point: &str) {
    let status = Command::new(env::current_exe().expect("应找到当前测试二进制"))
        .args(["--exact", "github_hard_exit_worker", "--nocapture"])
        .env(HARD_EXIT_WORKER, "1")
        .env(HARD_EXIT_DATA_ROOT, &prepared.data_root)
        .env(HARD_EXIT_HOME, &prepared.home)
        .env(HARD_EXIT_PLAN_ID, &prepared.plan_id)
        .env(HARD_EXIT_CANDIDATE_ID, &prepared.candidate_id)
        .env(HARD_EXIT_POINT, point)
        .status()
        .expect("应启动 hard-exit 子进程");
    assert_eq!(status.code(), Some(91), "子进程必须在 failpoint 直接退出");
}

fn reopen_after_hard_exit(prepared: &PreparedHardExit) -> UiOutcome {
    SkillYardApplication::new(
        ApplicationPaths::for_home(prepared.data_root.clone(), prepared.home.clone()),
        PlatformInfo::supported_for_test(),
    )
    .handle(UiIntent::GetStartupState)
    .expect("真实进程退出后应自动恢复")
}

fn assert_managed_names(outcome: &UiOutcome, expected: &[&str]) {
    let UiOutcome::Inventory {
        entries,
        recovery_issues,
        ..
    } = outcome
    else {
        panic!("恢复后应返回 Inventory");
    };
    assert!(recovery_issues.is_empty());
    let mut names = entries
        .iter()
        .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
        .map(|entry| entry.skill_name.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, expected);
}

fn staging_entry_count(data_root: &Path) -> usize {
    fs::read_dir(data_root.join("staging"))
        .expect("应读取 staging")
        .count()
}

fn bundle_count(data_root: &Path) -> i64 {
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row("SELECT COUNT(*) FROM bundles", [], |row| row.get(0))
        .expect("应读取 Bundle 数量")
}

fn current_target(data_root: &Path, bundle_id: &str) -> String {
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(
            "SELECT current_target FROM bundles WHERE id = ?1",
            [bundle_id],
            |row| row.get(0),
        )
        .expect("应读取 Bundle current")
}

fn ready_application(
    root: &std::path::Path,
    transport: Arc<RecordingTransport>,
) -> (SkillYardApplication, std::path::PathBuf, std::path::PathBuf) {
    let data_root = root.join("application-support/SkillYard");
    let home = root.join("home");
    fs::create_dir_all(&home).expect("应创建隔离 home");
    let application = SkillYardApplication::new_with_source_transport(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
        transport,
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    (application, data_root, home)
}

fn open_sources(application: &SkillYardApplication) -> Vec<skillyard_lib::SourceSummary> {
    let UiOutcome::SourceDiscovery { sources, .. } = application
        .handle(UiIntent::OpenSourceDiscovery)
        .expect("应打开 Source 发现页")
    else {
        panic!("应返回 Source 发现状态");
    };
    sources
}

fn reload_sources(
    application: &SkillYardApplication,
    source_id: &str,
) -> Vec<skillyard_lib::SourceSummary> {
    let UiOutcome::SourceDiscovery { sources, .. } = application
        .handle(UiIntent::ReloadGitHubSource {
            source_id: source_id.to_owned(),
        })
        .expect("远端内容失败应保存为 Stale Source 状态")
    else {
        panic!("重新加载后应返回 Source 发现状态");
    };
    sources
}

fn create_github_plan(
    application: &SkillYardApplication,
    source_id: &str,
) -> skillyard_lib::InstallPlan {
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateGithubInstallPlan {
            source_id: source_id.to_owned(),
        })
        .expect("Fresh Catalog 应生成安装 Plan")
    else {
        panic!("应返回唯一安装 Plan");
    };
    plan
}

fn candidate_id(plan: &skillyard_lib::InstallPlan, skill_name: &str) -> String {
    plan.candidates
        .iter()
        .find(|candidate| candidate.skill_name.as_deref() == Some(skill_name))
        .unwrap_or_else(|| panic!("Plan 中应存在候选成员 {skill_name}"))
        .candidate_id
        .clone()
}

fn managed_member_id(outcome: &UiOutcome, skill_name: &str) -> String {
    let UiOutcome::Inventory { entries, .. } = outcome else {
        panic!("应返回 Inventory");
    };
    entries
        .iter()
        .find(|entry| {
            entry.management_kind == ManagementKind::SkillYardManaged
                && entry.skill_name == skill_name
        })
        .and_then(|entry| entry.member_id.clone())
        .unwrap_or_else(|| panic!("应找到受管成员 {skill_name}"))
}

fn mount_codex_global(application: &SkillYardApplication, member_id: &str) {
    let UiOutcome::MountPlan { plan } = application
        .handle(UiIntent::CreateMountPlan {
            member_id: member_id.to_owned(),
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect("应生成 Codex global Mount Plan")
    else {
        panic!("应返回 Mount Plan");
    };
    application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect("应完成 Codex global Mount");
}

#[derive(Default)]
struct RecordingTransport {
    responses: Mutex<VecDeque<ScriptedResponse>>,
    requests: Mutex<Vec<SourceRequest>>,
}

struct ScriptedResponse {
    status: u16,
    body: ScriptedBody,
}

enum ScriptedBody {
    Bytes(Vec<u8>),
    Interrupted,
}

struct InterruptedBody;

impl Read for InterruptedBody {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "测试连接在读取 body 时中断",
        ))
    }
}

impl RecordingTransport {
    fn enqueue_catalog(&self, full_name: &str, tracked_ref: &str, sha: &str, archive: &[u8]) {
        self.enqueue_catalog_prefix(full_name, tracked_ref, sha);
        self.enqueue_bytes(200, archive);
    }

    fn enqueue_catalog_prefix(&self, full_name: &str, tracked_ref: &str, sha: &str) {
        self.enqueue(
            200,
            format!(
                r#"{{"full_name":"{full_name}","default_branch":"{tracked_ref}","private":false}}"#
            )
            .into_bytes(),
        );
        self.enqueue(200, format!(r#"{{"sha":"{sha}"}}"#).into_bytes());
    }

    fn enqueue_bytes(&self, status: u16, body: &[u8]) {
        self.enqueue(status, body.to_vec());
    }

    fn enqueue(&self, status: u16, body: Vec<u8>) {
        self.responses
            .lock()
            .expect("应写入响应队列")
            .push_back(ScriptedResponse {
                status,
                body: ScriptedBody::Bytes(body),
            });
    }

    fn enqueue_interrupted(&self, status: u16) {
        self.responses
            .lock()
            .expect("应写入中断响应")
            .push_back(ScriptedResponse {
                status,
                body: ScriptedBody::Interrupted,
            });
    }

    fn requests(&self) -> Vec<SourceRequest> {
        self.requests.lock().expect("应读取请求记录").clone()
    }
}

impl SourceTransport for RecordingTransport {
    fn get(&self, request: SourceRequest) -> Result<SourceResponse, SourceTransportError> {
        self.requests
            .lock()
            .expect("应记录网络请求")
            .push(request.clone());
        let response = self
            .responses
            .lock()
            .expect("应读取响应队列")
            .pop_front()
            .ok_or(SourceTransportError::Unavailable)?;
        Ok(SourceResponse {
            status: response.status,
            final_url: request.url,
            body: match response.body {
                ScriptedBody::Bytes(body) => Box::new(Cursor::new(body)),
                ScriptedBody::Interrupted => Box::new(InterruptedBody),
            },
        })
    }
}

fn bundle_archive(alpha_payload: &str, include_beta: bool) -> Vec<u8> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o100644);
    write_archive_skill(
        &mut archive,
        options,
        "repository-sha/skills/alpha",
        "alpha",
        alpha_payload,
    );
    if include_beta {
        write_archive_skill(
            &mut archive,
            options,
            "repository-sha/skills/beta",
            "beta",
            "beta-original",
        );
    }
    archive.finish().expect("应完成 ZIP fixture").into_inner()
}

fn supplement_archive() -> Vec<u8> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o100644);
    for (root, name, payload) in [
        ("repository-sha/skills/alpha", "alpha", "alpha-upstream-new"),
        ("repository-sha/skills/beta", "beta", "beta-original"),
        ("repository-sha/skills/gamma", "gamma", "gamma-new"),
    ] {
        write_archive_skill(&mut archive, options, root, name, payload);
    }
    archive
        .finish()
        .expect("应完成补装 ZIP fixture")
        .into_inner()
}

fn entry_limit_archive() -> Vec<u8> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().unix_permissions(0o040755);
    for index in 0..=20_000 {
        archive
            .add_directory(format!("repository-sha/entry-{index}/"), options)
            .expect("应写入超限条目 fixture");
    }
    archive
        .finish()
        .expect("应完成超限条目 ZIP fixture")
        .into_inner()
}

fn write_archive_skill(
    archive: &mut ZipWriter<Cursor<Vec<u8>>>,
    options: SimpleFileOptions,
    root: &str,
    name: &str,
    payload: &str,
) {
    archive
        .start_file(format!("{root}/SKILL.md"), options)
        .expect("应写入 Skill metadata");
    write!(
        archive,
        "---\nname: {name}\ndescription: {name} description\n---\n# {name}\n"
    )
    .expect("应写入 Skill metadata 内容");
    archive
        .start_file(format!("{root}/payload.txt"), options)
        .expect("应写入 Skill payload");
    archive
        .write_all(payload.as_bytes())
        .expect("应写入 Skill payload 内容");
}
