use std::{
    collections::BTreeSet,
    env, fs,
    os::unix::fs::{MetadataExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, BundleUpdateStatus, InstallationChainKind, InventoryLocationKind,
    LifecycleFailpoint, ManagementKind, MountHealth, MountScope, PlatformInfo,
    SkillYardApplication, SupportedAppId, TakeoverIdentityBasis, TakeoverMemberRequest,
    TakeoverOriginDisposition, TakeoverPlanRequest, TakeoverSharedTargetRequest, UiIntent,
    UiOutcome,
};
use tempfile::tempdir;

const HARD_EXIT_TAKEOVER_WORKER: &str = "SKILLYARD_TAKEOVER_HARD_EXIT_WORKER";
const HARD_EXIT_TAKEOVER_DATA_ROOT: &str = "SKILLYARD_TAKEOVER_HARD_EXIT_DATA_ROOT";
const HARD_EXIT_TAKEOVER_HOME: &str = "SKILLYARD_TAKEOVER_HARD_EXIT_HOME";
const HARD_EXIT_TAKEOVER_PLAN_ID: &str = "SKILLYARD_TAKEOVER_HARD_EXIT_PLAN_ID";
const HARD_EXIT_TAKEOVER_POINT: &str = "SKILLYARD_TAKEOVER_HARD_EXIT_POINT";

/// 绝大多数测试仍描述单个 Member；宏只把它们投影到新的 Bundle 请求合同。
macro_rules! takeover_plan_request {
    (
        observation_ids: $observation_ids:expr,
        selected_observation_id: $selected_observation_id:expr,
        preserved_observation_ids: $preserved_observation_ids:expr,
        shared_targets: $shared_targets:expr $(,)?
    ) => {
        TakeoverPlanRequest {
            members: vec![TakeoverMemberRequest {
                observation_ids: $observation_ids,
                selected_observation_id: $selected_observation_id,
                preserved_observation_ids: $preserved_observation_ids,
            }],
            shared_targets: $shared_targets,
        }
    };
    (
        members: $members:expr,
        shared_targets: $shared_targets:expr $(,)?
    ) => {
        TakeoverPlanRequest {
            members: $members,
            shared_targets: $shared_targets,
        }
    };
}

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
            request: takeover_plan_request! {
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

    assert_eq!(plan.members[0].skill_name, "alpha");
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
fn verified_lock_v3_installation_chain_survives_takeover_and_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/alpha");
    let project_root = sandbox.path().join("project");
    let project_skill_root = project_root.join(".codex/skills/alpha");
    write_skill(&skill_root, "alpha", "接管安装履历");
    write_skill(
        &project_skill_root,
        "alpha",
        "同名项目 Skill 没有全局安装履历",
    );
    let project_skill_root = fs::canonicalize(&project_root)
        .expect("应解析 Project")
        .join(".codex/skills/alpha");

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
    let observation_id = entries
        .iter()
        .find(|entry| entry.skill_root == path_text(&skill_root))
        .expect("应发现待接管 Skill")
        .id
        .clone();
    assert!(
        entries
            .iter()
            .find(|entry| entry.id == observation_id)
            .expect("应保留扫描观察")
            .installation_chain
            .is_none(),
        "没有 lock 时不能猜测 Installation Chain"
    );
    let project_entries = match application
        .handle(UiIntent::RegisterProject {
            root_path: path_text(&project_root),
        })
        .expect("应登记包含同名 Skill 的 Project")
    {
        UiOutcome::Inventory { entries, .. } => entries,
        _ => panic!("登记 Project 后应返回 Inventory"),
    };
    let project_observation_id = observation_id_at(&project_entries, &project_skill_root);

    write_global_lock_v3(&home, "alpha");
    let entries = match application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("刷新本机应读取新增的 lock v3")
    {
        UiOutcome::Inventory {
            entries,
            last_local_refresh: Some(summary),
            ..
        } => {
            assert_eq!(summary.changed, 1, "新增安装履历属于本机观察变化");
            entries
        }
        _ => panic!("刷新本机应返回 Inventory 和刷新摘要"),
    };
    let observed = entries
        .iter()
        .find(|entry| entry.id == observation_id)
        .expect("刷新后应保留同一观察");
    let installation_chain = observed
        .installation_chain
        .as_ref()
        .expect("扫描观察应携带 lock v3 安装履历");
    assert_eq!(installation_chain.kind, InstallationChainKind::LockV3);
    assert_eq!(installation_chain.source, "owner/repository");
    assert_eq!(
        installation_chain.skill_path.as_deref(),
        Some("skills/alpha/SKILL.md")
    );
    assert_eq!(
        installation_chain.tracked_ref.as_deref(),
        Some("release-2026-07"),
        "GitHub CLI 的 pinnedRef 必须保存为同一 tracked ref"
    );
    assert!(
        entries
            .iter()
            .find(|entry| entry.id == project_observation_id)
            .expect("刷新后应保留项目观察")
            .installation_chain
            .is_none(),
        "全局 lock 不能附给项目目录中的同名 Skill"
    );

    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![observed.id.clone()],
                selected_observation_id: observed.id.clone(),
                preserved_observation_ids: vec![observed.id.clone()],
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成带安装履历的接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    assert_eq!(
        plan.members[0].installation_chain.as_deref(),
        Some(installation_chain),
        "接管计划必须封存扫描时的安装履历"
    );
    assert_eq!(
        plan.source_display_name.as_deref(),
        Some("owner/repository"),
        "接管预览应明确显示即将自动保存的 Source"
    );
    application
        .handle(UiIntent::ConfirmTakeoverPlan { plan_id: plan.id })
        .expect("接管应成功");
    fs::remove_file(home.join(".agents/.skill-lock.json")).expect("应删除外部 lock");
    drop(application);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let (entries, bundle_updates) = match restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应读取受管状态")
    {
        UiOutcome::Inventory {
            entries,
            bundle_updates,
            ..
        } => (entries, bundle_updates),
        _ => panic!("重启后应返回 Inventory"),
    };
    let managed = entries
        .iter()
        .find(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
        .expect("接管后应存在受管 Skill");
    assert_eq!(
        managed.installation_chain.as_ref(),
        Some(installation_chain),
        "外部 lock 后续不再控制已保存的安装履历"
    );
    assert_eq!(
        managed.source_display_name.as_deref(),
        Some("owner/repository"),
        "删除外部 lock 并重启后，Bundle 仍应保留已登记 Source"
    );
    assert_eq!(
        bundle_updates
            .iter()
            .find(|update| { Some(update.bundle_id.as_str()) == managed.bundle_id.as_deref() })
            .map(|update| update.status),
        Some(BundleUpdateStatus::Available),
        "没有 adopted commit 的自动 Source 应提示可执行首次完整更新"
    );
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开接管后的 SQLite");
    let saved_source: (String, String, String, String, Option<String>) = connection
        .query_row(
            "SELECT source.canonical_identity, source.display_name, source.locator,
                    source.tracked_ref, link.adopted_marker
             FROM sources AS source
             JOIN source_bundle_links AS link ON link.source_id = source.id
             WHERE link.bundle_id = ?1",
            [managed.bundle_id.as_deref().expect("受管成员应有 Bundle")],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .expect("接管确认应自动保存并关联 lock Source");
    assert_eq!(
        saved_source,
        (
            "github:owner/repository".to_owned(),
            "owner/repository".to_owned(),
            "https://github.com/owner/repository".to_owned(),
            "release-2026-07".to_owned(),
            None,
        )
    );
}

#[test]
fn takeover_reuses_an_unlinked_source_without_silently_changing_its_tracked_ref() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/alpha");
    write_skill(&skill_root, "alpha", "复用 Source 时保留已确认的 ref");

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应建立真实 SQLite");
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    connection
        .execute(
            "INSERT INTO sources (
                id, kind, canonical_identity, owner, repository,
                display_name, locator, tracked_ref, member_path_hint,
                sort_order, created_at, updated_at
             ) VALUES (
                'existing-source', 'github', 'github:owner/repository',
                'owner', 'repository', 'owner/repository',
                'https://github.com/owner/repository', 'main', NULL, 50, 1, 1
             )",
            [],
        )
        .expect("应准备尚未关联 Bundle 的既有 Source");
    drop(connection);

    write_global_lock_v3(&home, "alpha");
    let entries = match application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("刷新应读取 ref 不同的 lock v3")
    {
        UiOutcome::Inventory { entries, .. } => entries,
        _ => panic!("刷新后应返回 Inventory"),
    };
    let observation_id = entries
        .iter()
        .find(|entry| entry.skill_root == path_text(&skill_root))
        .expect("应发现待接管 Skill")
        .id
        .clone();
    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: vec![observation_id],
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成复用 Source 的接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    application
        .handle(UiIntent::ConfirmTakeoverPlan {
            plan_id: plan.id.clone(),
        })
        .expect("接管应复用 Source 并保留其当前 ref");

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应重新打开真实 SQLite");
    let saved: (String, String) = connection
        .query_row(
            "SELECT source.tracked_ref, link.bundle_id
             FROM sources AS source
             JOIN source_bundle_links AS link ON link.source_id = source.id
             WHERE source.id = 'existing-source'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("既有 Source 应自动关联到新 Bundle");
    assert_eq!(
        saved,
        ("main".to_owned(), plan.bundle_id),
        "lock 中不同的 ref 只能保留在安装履历，不能跳过用户确认改写 Source"
    );
}

#[test]
fn verified_lock_v3_bundle_can_plan_all_installed_members_together() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let shared_root = home.join(".agents/skills");
    write_skill(&shared_root.join("alpha"), "alpha", "Bundle 成员 alpha");
    write_skill(&shared_root.join("beta"), "beta", "Bundle 成员 beta");
    write_global_lock_v3_for(&home, &["alpha", "beta"]);

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
    let alpha = entries
        .iter()
        .find(|entry| entry.skill_name == "alpha")
        .expect("应发现 alpha");
    let beta = entries
        .iter()
        .find(|entry| entry.skill_name == "beta")
        .expect("应发现 beta");

    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                members: vec![
                    TakeoverMemberRequest {
                        observation_ids: vec![alpha.id.clone()],
                        selected_observation_id: alpha.id.clone(),
                        preserved_observation_ids: Vec::new(),
                    },
                    TakeoverMemberRequest {
                        observation_ids: vec![beta.id.clone()],
                        selected_observation_id: beta.id.clone(),
                        preserved_observation_ids: Vec::new(),
                    },
                ],
                shared_targets: vec![
                    TakeoverSharedTargetRequest {
                        shared_observation_id: alpha.id.clone(),
                        app_id: SupportedAppId::Codex,
                    },
                    TakeoverSharedTargetRequest {
                        shared_observation_id: beta.id.clone(),
                        app_id: SupportedAppId::Codex,
                    },
                ],
            },
        })
        .expect("同一 lock v3 来源中的已安装成员应生成一个 Bundle 接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("接管计划必须覆盖整个待接管 Bundle"),
    };
    assert_eq!(plan.bundle_display_name, "owner/repository");
    assert_eq!(plan.members.len(), 2);
    assert_eq!(
        plan.members
            .iter()
            .map(|member| member.skill_name.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["alpha", "beta"])
    );
    assert_eq!(plan.targets.len(), 2);
    assert_eq!(
        plan.source_display_name.as_deref(),
        Some("owner/repository")
    );

    let entries = match application
        .handle(UiIntent::ConfirmTakeoverPlan {
            plan_id: plan.id.clone(),
        })
        .expect("应原子接管整个 Bundle")
    {
        UiOutcome::Inventory {
            entries, mounts, ..
        } => {
            assert_eq!(mounts.len(), 2);
            assert!(mounts.iter().all(|mount| {
                mount.health == MountHealth::Healthy && mount.app_id == SupportedAppId::Codex
            }));
            entries
        }
        _ => panic!("确认后应返回 Inventory"),
    };
    let managed = entries
        .iter()
        .filter(|entry| entry.bundle_id.as_deref() == Some(plan.bundle_id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(managed.len(), 2);
    assert!(
        managed
            .iter()
            .all(|entry| entry.bundle_display_name.as_deref() == Some("owner/repository"))
    );
    assert_eq!(
        managed
            .iter()
            .map(|entry| (
                entry.skill_name.as_str(),
                entry
                    .description
                    .as_deref()
                    .expect("受管 Skill 应保留原始 description"),
            ))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([("alpha", "Bundle 成员 alpha"), ("beta", "Bundle 成员 beta"),])
    );
    for member in &plan.members {
        assert!(
            !shared_root.join(&member.skill_name).exists(),
            "共享目录原入口应在全部目标验证后移除"
        );
        assert_eq!(
            fs::read_link(home.join(".codex/skills").join(&member.skill_name))
                .expect("Codex 位置应成为 Mount"),
            Path::new(&member.expected_target)
        );
    }

    drop(application);
    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let restarted_entries = match restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应读取受管 Bundle")
    {
        UiOutcome::Inventory { entries, .. } => entries,
        _ => panic!("重启后应返回 Inventory"),
    };
    let restarted_managed = restarted_entries
        .iter()
        .filter(|entry| entry.bundle_id.as_deref() == Some(plan.bundle_id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(restarted_managed.len(), 2);
    assert!(
        restarted_managed
            .iter()
            .all(|entry| entry.description.as_deref().is_some()),
        "重启后仍应从受管 Member 读取原始 description"
    );
}

#[test]
fn lock_source_name_is_used_as_the_takeover_bundle_name() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".agents/skills/research");
    write_skill(&skill_root, "research", "调研一手资料");
    write_global_lock_v3_for_source(&home, &["research"], "Matt Pocock Skills");

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let entries = match application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    {
        UiOutcome::Inventory { entries, .. } => entries,
        _ => panic!("首次扫描应返回 Inventory"),
    };
    let observed = entries
        .iter()
        .find(|entry| entry.skill_name == "research")
        .expect("应发现 research");
    assert_eq!(
        observed.takeover_group_display_name.as_deref(),
        Some("Matt Pocock Skills"),
        "待接管 Bundle 必须展示 lock 保存的 source 名称"
    );

    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![observed.id.clone()],
                selected_observation_id: observed.id.clone(),
                preserved_observation_ids: Vec::new(),
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    assert_eq!(plan.bundle_display_name, "Matt Pocock Skills");
}

#[test]
fn bundle_evidence_is_independent_from_the_selected_member_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let shared_root = home.join(".agents/skills");
    let project_root = sandbox.path().join("project");
    let project_alpha = project_root.join(".codex/skills/alpha");
    write_skill(&shared_root.join("alpha"), "alpha", "带安装收据的 alpha");
    write_skill(&shared_root.join("beta"), "beta", "带安装收据的 beta");
    write_skill(&project_alpha, "alpha", "用户选择的本地 alpha");
    let project_alpha = fs::canonicalize(&project_alpha).expect("应解析 Project Skill 路径");
    write_global_lock_v3_for(&home, &["alpha", "beta"]);

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home.clone()),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    let entries = match application
        .handle(UiIntent::RegisterProject {
            root_path: path_text(&project_root),
        })
        .expect("应登记包含同名副本的 Project")
    {
        UiOutcome::Inventory { entries, .. } => entries,
        _ => panic!("登记 Project 后应返回 Inventory"),
    };
    let shared_alpha_id = observation_id_at(&entries, &shared_root.join("alpha"));
    let shared_beta_id = observation_id_at(&entries, &shared_root.join("beta"));
    let project_alpha_id = observation_id_at(&entries, &project_alpha);
    assert!(
        entries
            .iter()
            .find(|entry| entry.id == project_alpha_id)
            .expect("应发现 Project alpha")
            .installation_chain
            .is_none(),
        "Project 副本不能继承 global lock"
    );

    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                members: vec![
                    TakeoverMemberRequest {
                        observation_ids: vec![
                            shared_alpha_id.clone(),
                            project_alpha_id.clone(),
                        ],
                        selected_observation_id: project_alpha_id.clone(),
                        preserved_observation_ids: Vec::new(),
                    },
                    TakeoverMemberRequest {
                        observation_ids: vec![shared_beta_id.clone()],
                        selected_observation_id: shared_beta_id.clone(),
                        preserved_observation_ids: Vec::new(),
                    },
                ],
                shared_targets: vec![
                    TakeoverSharedTargetRequest {
                        shared_observation_id: shared_alpha_id,
                        app_id: SupportedAppId::Codex,
                    },
                    TakeoverSharedTargetRequest {
                        shared_observation_id: shared_beta_id,
                        app_id: SupportedAppId::Codex,
                    },
                ],
            },
        })
        .expect("Bundle 证据不应强迫用户选择带收据的内容副本")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    let alpha = plan
        .members
        .iter()
        .find(|member| member.skill_name == "alpha")
        .expect("Plan 应包含 alpha");
    assert_eq!(alpha.selected_observation_id, project_alpha_id);
    assert!(
        alpha.installation_chain.is_some(),
        "Member 应保留证明 Bundle 边界的安装履历"
    );
    assert_eq!(plan.bundle_display_name, "owner/repository");

    let UiOutcome::Inventory {
        entries, mounts, ..
    } = application
        .handle(UiIntent::ConfirmTakeoverPlan { plan_id: plan.id })
        .expect("应一次接管整个 Bundle")
    else {
        panic!("接管后应返回 Inventory");
    };
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
            .count(),
        2
    );
    assert_eq!(mounts.len(), 2);
    assert_eq!(
        read_skill_file(&home.join(".codex/skills/alpha")),
        "---\nname: alpha\ndescription: 用户选择的本地 alpha\n---\n# alpha\n".as_bytes()
    );
}

#[test]
fn later_valid_member_reuses_the_managed_bundle_for_the_same_installation_group() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let shared_root = home.join(".agents/skills");
    let alpha_root = shared_root.join("alpha");
    let beta_root = shared_root.join("beta");
    write_skill(&alpha_root, "alpha", "先接管的有效成员");
    fs::create_dir_all(&beta_root).expect("应创建无效 beta");
    fs::write(beta_root.join("SKILL.md"), "---\nname: beta\n---\n# beta\n")
        .expect("应写入缺少 description 的 beta");
    write_global_lock_v3_for(&home, &["alpha", "beta"]);

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
    let alpha_id = observation_id_at(&entries, &alpha_root);
    let first_plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![alpha_id.clone()],
                selected_observation_id: alpha_id.clone(),
                preserved_observation_ids: Vec::new(),
                shared_targets: vec![TakeoverSharedTargetRequest {
                    shared_observation_id: alpha_id,
                    app_id: SupportedAppId::Codex,
                }],
            },
        })
        .expect("有效 alpha 应能先接管")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    let bundle_id = first_plan.bundle_id.clone();
    application
        .handle(UiIntent::ConfirmTakeoverPlan {
            plan_id: first_plan.id,
        })
        .expect("应先接管 alpha");

    write_skill(&beta_root, "beta", "修复后补充进同一 Bundle");
    let entries = match application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("修复 beta 后应刷新本机")
    {
        UiOutcome::Inventory { entries, .. } => entries,
        _ => panic!("刷新应返回 Inventory"),
    };
    let beta_id = observation_id_at(&entries, &beta_root);
    let second_plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![beta_id.clone()],
                selected_observation_id: beta_id.clone(),
                preserved_observation_ids: Vec::new(),
                shared_targets: vec![TakeoverSharedTargetRequest {
                    shared_observation_id: beta_id,
                    app_id: SupportedAppId::Codex,
                }],
            },
        })
        .expect("修复后的 beta 应能补充接管")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    assert_eq!(
        second_plan.bundle_id, bundle_id,
        "同一确定性安装组必须复用既有 Bundle"
    );
    assert_eq!(
        second_plan.source_display_name.as_deref(),
        Some("owner/repository"),
        "补充接管预览必须保留既有 Bundle 的更新来源"
    );
    assert_eq!(
        second_plan
            .retained_members
            .iter()
            .map(|member| member.skill_name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha"],
        "补充接管的预览必须展示最终 Bundle 中保留的既有成员"
    );
    assert_eq!(second_plan.retained_members[0].mounts.len(), 1);

    let UiOutcome::Inventory {
        entries, mounts, ..
    } = application
        .handle(UiIntent::ConfirmTakeoverPlan {
            plan_id: second_plan.id,
        })
        .expect("补充接管应通过同一 Takeover 事务完成")
    else {
        panic!("补充接管后应返回 Inventory");
    };
    let managed = entries
        .iter()
        .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
        .collect::<Vec<_>>();
    assert_eq!(managed.len(), 2);
    assert!(
        managed
            .iter()
            .all(|entry| entry.bundle_id.as_deref() == Some(bundle_id.as_str()))
    );
    assert_eq!(mounts.len(), 2);
    assert_eq!(
        read_skill_file(&home.join(".codex/skills/alpha")),
        "---\nname: alpha\ndescription: 先接管的有效成员\n---\n# alpha\n".as_bytes()
    );
    assert_eq!(
        read_skill_file(&home.join(".codex/skills/beta")),
        "---\nname: beta\ndescription: 修复后补充进同一 Bundle\n---\n# beta\n".as_bytes()
    );
    let content_count = fs::read_dir(data_root.join("bundles").join(bundle_id).join("contents"))
        .expect("应读取 Bundle contents")
        .count();
    assert_eq!(content_count, 1, "成功补充后不保留可回滚旧内容");
}

#[test]
fn supplementing_existing_bundle_recovers_across_the_commit_point() {
    for (point, should_commit) in [
        ("transaction-only", false),
        ("current-before-phase", false),
        ("state-before-journal", true),
        ("previous-content-isolated", true),
        ("previous-content-removal", true),
    ] {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let home = sandbox.path().join("home");
        let data_root = sandbox.path().join("application-support/SkillYard");
        let shared_root = home.join(".agents/skills");
        let alpha_root = shared_root.join("alpha");
        let beta_root = shared_root.join("beta");
        write_skill(&alpha_root, "alpha", "先接管的 alpha");
        fs::create_dir_all(&beta_root).expect("应创建无效 beta");
        fs::write(beta_root.join("SKILL.md"), "---\nname: beta\n---\n# beta\n")
            .expect("应写入缺少 description 的 beta");
        write_global_lock_v3_for(&home, &["alpha", "beta"]);

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
        let alpha_id = observation_id_at(&entries, &alpha_root);
        let first_plan = match application
            .handle(UiIntent::CreateTakeoverPlan {
                request: takeover_plan_request! {
                    observation_ids: vec![alpha_id.clone()],
                    selected_observation_id: alpha_id.clone(),
                    preserved_observation_ids: Vec::new(),
                    shared_targets: vec![TakeoverSharedTargetRequest {
                        shared_observation_id: alpha_id,
                        app_id: SupportedAppId::Codex,
                    }],
                },
            })
            .expect("应生成 alpha 接管计划")
        {
            UiOutcome::TakeoverPlan { plan } => plan,
            _ => panic!("应返回 Takeover Plan"),
        };
        let bundle_id = first_plan.bundle_id.clone();
        application
            .handle(UiIntent::ConfirmTakeoverPlan {
                plan_id: first_plan.id,
            })
            .expect("应先接管 alpha");

        write_skill(&beta_root, "beta", "后补充的 beta");
        let entries = match application
            .handle(UiIntent::RefreshLocalInventory)
            .expect("修复 beta 后应刷新")
        {
            UiOutcome::Inventory { entries, .. } => entries,
            _ => panic!("刷新应返回 Inventory"),
        };
        let beta_id = observation_id_at(&entries, &beta_root);
        let second_plan = match application
            .handle(UiIntent::CreateTakeoverPlan {
                request: takeover_plan_request! {
                    observation_ids: vec![beta_id.clone()],
                    selected_observation_id: beta_id.clone(),
                    preserved_observation_ids: Vec::new(),
                    shared_targets: vec![TakeoverSharedTargetRequest {
                        shared_observation_id: beta_id,
                        app_id: SupportedAppId::Codex,
                    }],
                },
            })
            .expect("应生成 beta 补充接管计划")
        {
            UiOutcome::TakeoverPlan { plan } => plan,
            _ => panic!("应返回 Takeover Plan"),
        };
        assert_eq!(second_plan.bundle_id, bundle_id);
        let plan_id = second_plan.id;
        drop(application);

        run_hard_exit_takeover_worker(&data_root, &home, &plan_id, point);
        let reopened = SkillYardApplication::new(
            ApplicationPaths::for_home(data_root.clone(), home.clone()),
            PlatformInfo::supported_for_test(),
        );
        let UiOutcome::Inventory {
            entries,
            mounts,
            recovery_issues,
            ..
        } = reopened
            .handle(UiIntent::GetStartupState)
            .unwrap_or_else(|error| panic!("{point} 重启后应自动恢复：{error}"))
        else {
            panic!("恢复后应返回 Inventory");
        };
        assert!(
            recovery_issues.is_empty(),
            "{point} 不应需要人工恢复：{recovery_issues:?}"
        );
        let managed = entries
            .iter()
            .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
            .collect::<Vec<_>>();
        let expected_count = if should_commit { 2 } else { 1 };
        assert_eq!(managed.len(), expected_count, "{point} 成员提交方向错误");
        assert!(
            managed
                .iter()
                .all(|entry| entry.bundle_id.as_deref() == Some(bundle_id.as_str()))
        );
        assert_eq!(mounts.len(), expected_count, "{point} Mount 提交方向错误");
        assert_eq!(
            read_skill_file(&home.join(".codex/skills/alpha")),
            "---\nname: alpha\ndescription: 先接管的 alpha\n---\n# alpha\n".as_bytes()
        );
        if should_commit {
            assert_eq!(
                read_skill_file(&home.join(".codex/skills/beta")),
                "---\nname: beta\ndescription: 后补充的 beta\n---\n# beta\n".as_bytes()
            );
            assert!(!beta_root.exists());
        } else {
            assert_eq!(
                read_skill_file(&beta_root),
                "---\nname: beta\ndescription: 后补充的 beta\n---\n# beta\n".as_bytes()
            );
            assert!(!home.join(".codex/skills/beta").exists());
        }
        assert_eq!(
            fs::read_dir(data_root.join("bundles").join(&bundle_id).join("contents"),)
                .expect("应读取 Bundle contents")
                .count(),
            1,
            "{point} 恢复后只保留一份当前内容"
        );
        drop(reopened);

        let reopened_again = SkillYardApplication::new(
            ApplicationPaths::for_home(data_root.clone(), home.clone()),
            PlatformInfo::supported_for_test(),
        );
        let UiOutcome::Inventory {
            entries,
            mounts,
            recovery_issues,
            ..
        } = reopened_again
            .handle(UiIntent::GetStartupState)
            .unwrap_or_else(|error| panic!("{point} 第二次启动仍应保持已恢复状态：{error}"))
        else {
            panic!("第二次启动仍应返回 Inventory");
        };
        assert!(
            recovery_issues.is_empty(),
            "{point} 第二次启动不应重新产生恢复问题"
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
                .count(),
            expected_count,
            "{point} 第二次启动不能改变成员结果"
        );
        assert_eq!(
            mounts.len(),
            expected_count,
            "{point} 第二次启动不能改变 Mount 结果"
        );
        assert_eq!(
            fs::read_dir(data_root.join("bundles").join(&bundle_id).join("contents"))
                .expect("第二次启动后应读取 Bundle contents")
                .count(),
            1,
            "{point} 第二次启动仍只能保留一份当前内容"
        );
        drop(reopened_again);

        let connection =
            Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开恢复后的 SQLite");
        let transaction_count = connection
            .query_row("SELECT COUNT(*) FROM takeover_transactions", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("应读取 Takeover 事务数量");
        assert_eq!(
            transaction_count, 0,
            "{point} 第二次启动后不能残留已完成 Takeover 事务"
        );
    }
}

#[test]
fn multi_member_bundle_recovers_atomically_across_the_commit_point() {
    for (point, should_commit) in [
        ("origin-before-progress", false),
        ("state-before-journal", true),
    ] {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let home = sandbox.path().join("home");
        let data_root = sandbox.path().join("application-support/SkillYard");
        let shared_root = home.join(".agents/skills");
        let alpha_root = shared_root.join("alpha");
        let beta_root = shared_root.join("beta");
        write_skill(&alpha_root, "alpha", "Bundle 中断测试 alpha");
        write_skill(&beta_root, "beta", "Bundle 中断测试 beta");
        let alpha_content = read_skill_file(&alpha_root);
        let beta_content = read_skill_file(&beta_root);
        write_global_lock_v3_for(&home, &["alpha", "beta"]);

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
        let alpha_id = observation_id_at(&entries, &alpha_root);
        let beta_id = observation_id_at(&entries, &beta_root);
        let plan = match application
            .handle(UiIntent::CreateTakeoverPlan {
                request: takeover_plan_request! {
                    members: vec![
                        TakeoverMemberRequest {
                            observation_ids: vec![alpha_id.clone()],
                            selected_observation_id: alpha_id.clone(),
                            preserved_observation_ids: Vec::new(),
                        },
                        TakeoverMemberRequest {
                            observation_ids: vec![beta_id.clone()],
                            selected_observation_id: beta_id.clone(),
                            preserved_observation_ids: Vec::new(),
                        },
                    ],
                    shared_targets: vec![
                        TakeoverSharedTargetRequest {
                            shared_observation_id: alpha_id,
                            app_id: SupportedAppId::Codex,
                        },
                        TakeoverSharedTargetRequest {
                            shared_observation_id: beta_id,
                            app_id: SupportedAppId::Codex,
                        },
                    ],
                },
            })
            .expect("应生成 Bundle 接管计划")
        {
            UiOutcome::TakeoverPlan { plan } => plan,
            _ => panic!("应返回 Takeover Plan"),
        };
        let plan_id = plan.id.clone();
        let bundle_id = plan.bundle_id.clone();
        let expected_targets = plan
            .members
            .iter()
            .map(|member| (member.skill_name.clone(), member.expected_target.clone()))
            .collect::<Vec<_>>();
        drop(application);

        run_hard_exit_takeover_worker(&data_root, &home, &plan_id, point);

        let reopened = SkillYardApplication::new(
            ApplicationPaths::for_home(data_root.clone(), home.clone()),
            PlatformInfo::supported_for_test(),
        );
        let UiOutcome::Inventory {
            entries,
            mounts,
            recovery_issues,
            ..
        } = reopened
            .handle(UiIntent::GetStartupState)
            .unwrap_or_else(|error| panic!("{point} 重启后应自动恢复：{error}"))
        else {
            panic!("恢复后应返回 Inventory");
        };
        assert!(recovery_issues.is_empty(), "{point} 不应需要人工恢复");

        if should_commit {
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry.bundle_id.as_deref() == Some(bundle_id.as_str()))
                    .count(),
                2,
                "{point} 必须完整保留两个受管 Member"
            );
            assert_eq!(mounts.len(), 2, "{point} 必须完整保留两个 Mount");
            assert!(!alpha_root.exists());
            assert!(!beta_root.exists());
            for (skill_name, expected_target) in &expected_targets {
                assert_eq!(
                    fs::read_link(home.join(".codex/skills").join(skill_name))
                        .expect("提交后的 Host 位置应为 Mount"),
                    Path::new(expected_target)
                );
            }
            assert_takeover_database_counts(&data_root, (1, 2, 0, 0));
        } else {
            assert!(
                entries
                    .iter()
                    .all(|entry| entry.management_kind != ManagementKind::SkillYardManaged),
                "{point} 不能只接管 Bundle 的部分成员"
            );
            assert!(mounts.is_empty(), "{point} 不能留下部分 Mount");
            assert_eq!(read_skill_file(&alpha_root), alpha_content);
            assert_eq!(read_skill_file(&beta_root), beta_content);
            assert!(!contains_entries(&data_root.join("bundles")));
            assert_takeover_database_counts(&data_root, (0, 0, 0, 0));
        }
        assert_clean_takeover_artifacts(&data_root, &[&alpha_root, &beta_root]);
    }
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
            request: takeover_plan_request! {
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

    assert_eq!(
        plan.members[0].identity_basis,
        TakeoverIdentityBasis::UserConfirmed
    );
    assert_eq!(plan.members[0].selected_observation_id, codex_id);
    assert_eq!(plan.members[0].skill_description, "采用这份内容");
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
            .all(|target| target.expected_target == plan.members[0].expected_target)
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
fn blocked_takeover_reserves_only_its_original_skill_for_plan_and_confirmation() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let fixture = prepare_hard_exit_takeover(sandbox.path(), "alpha");
    let beta_root = fixture.home.join(".codex/skills/beta");
    write_skill(&beta_root, "beta", "不相关 Skill 仍可操作");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home.clone()),
        PlatformInfo::supported_for_test(),
    );
    let entries = match application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("本机刷新应发现新增的不相关 Skill")
    {
        UiOutcome::Inventory { entries, .. } => entries,
        _ => panic!("本机刷新应返回 Inventory"),
    };
    let alpha_id = observation_id_at(&entries, &fixture.skill_root);
    let beta_id = observation_id_at(&entries, &beta_root);
    let create_plan = |observation_id: &str| {
        takeover_plan_request! {
            observation_ids: vec![observation_id.to_owned()],
            selected_observation_id: observation_id.to_owned(),
            preserved_observation_ids: vec![observation_id.to_owned()],
            shared_targets: Vec::new(),
        }
    };
    let pending_same_origin = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: create_plan(&alpha_id),
        })
        .expect("blocked 发生前允许生成另一份只读 Plan")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    drop(application);

    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "current-before-phase",
    );
    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "candidate-cleanup",
    );
    assert_eq!(
        read_skill_file(&fixture.skill_root),
        fixture.original_content
    );
    let candidate = only_takeover_candidate_content(&fixture.data_root);
    fs::remove_dir_all(&candidate).expect("应替换待清理候选根目录");
    fs::create_dir(&candidate).expect("应创建同名未知候选根目录");
    fs::write(candidate.join("unknown.txt"), "等待人工处理").expect("应写入未知内容");

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = application
        .handle(UiIntent::GetStartupState)
        .expect("根目录身份变化应进入 blocked recovery")
    else {
        panic!("启动应返回 Inventory");
    };
    assert_eq!(recovery_issues.len(), 1);

    application
        .handle(UiIntent::CreateTakeoverPlan {
            request: create_plan(&alpha_id),
        })
        .expect_err("blocked Takeover 必须阻止同一原 Skill 再次生成 Plan");
    application
        .handle(UiIntent::ConfirmTakeoverPlan {
            plan_id: pending_same_origin.id,
        })
        .expect_err("Plan 生成后出现 blocked Takeover 时确认仍必须二次拒绝");

    let unrelated = application
        .handle(UiIntent::CreateTakeoverPlan {
            request: create_plan(&beta_id),
        })
        .expect("不相关 Skill 仍应生成 Takeover Plan");
    assert!(matches!(unrelated, UiOutcome::TakeoverPlan { .. }));
}

#[test]
fn blocked_takeover_prevents_removing_its_registered_project() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let project_root = sandbox.path().join("workspace/project");
    let skill_root = project_root.join(".codex/skills/alpha");
    write_skill(&skill_root, "alpha", "Project 接管恢复依赖原项目身份");
    let skill_root = fs::canonicalize(&skill_root).expect("应解析 Project Skill 路径");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    let (project_id, observation_id) = match application
        .handle(UiIntent::RegisterProject {
            root_path: path_text(&project_root),
        })
        .expect("应登记包含待接管 Skill 的 Project")
    {
        UiOutcome::Inventory {
            entries, projects, ..
        } => (
            projects.first().expect("应返回已登记 Project").id.clone(),
            observation_id_at(&entries, &skill_root),
        ),
        _ => panic!("登记 Project 后应返回 Inventory"),
    };
    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: vec![observation_id],
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成 Project Takeover Plan")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    let removal_plan_id = match application
        .handle(UiIntent::CreateProjectRemovalPlan {
            project_id: project_id.clone(),
        })
        .expect("Takeover 尚未阻塞时应允许生成 Project Removal Plan")
    {
        UiOutcome::RemovalPlan { plan } => plan.id,
        _ => panic!("应返回 Project Removal Plan"),
    };
    drop(application);

    run_hard_exit_takeover_worker(&data_root, &home, &plan.id, "current-before-phase");
    run_hard_exit_takeover_worker(&data_root, &home, &plan.id, "candidate-cleanup");
    let candidate = only_takeover_candidate_content(&data_root);
    fs::remove_dir_all(&candidate).expect("应替换待清理候选根目录");
    fs::create_dir(&candidate).expect("应创建同名未知候选根目录");
    fs::write(candidate.join("unknown.txt"), "等待人工处理").expect("应写入未知内容");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    reopened
        .handle(UiIntent::GetStartupState)
        .expect("候选目录身份变化应进入人工恢复");
    reopened
        .handle(UiIntent::CreateProjectRemovalPlan {
            project_id: project_id.clone(),
        })
        .expect_err("blocked Takeover 必须阻止删除恢复仍依赖的 Project");
    reopened
        .handle(UiIntent::ConfirmRemovalPlan {
            plan_id: removal_plan_id,
        })
        .expect_err("旧 Project Removal Plan 确认时也必须重新检查 blocked Takeover");
}

#[test]
fn precommit_blocked_takeover_reserves_its_host_path_from_other_members() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应完成首次扫描");

    let install_input = sandbox.path().join("managed-source/skills/alpha");
    write_skill(&install_input, "alpha", "另一个已受管成员");
    let install_plan = match application
        .handle(UiIntent::CreateFolderInstallPlan {
            input_path: path_text(&sandbox.path().join("managed-source")),
        })
        .expect("应生成本地安装 Plan")
    {
        UiOutcome::InstallPlan { plan } => plan,
        _ => panic!("应返回 Folder Install Plan"),
    };
    let selected = install_plan
        .candidates
        .iter()
        .filter(|candidate| candidate.selectable)
        .map(|candidate| candidate.candidate_id.clone())
        .collect::<Vec<_>>();
    let installed = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: install_plan.id,
            selected_candidate_ids: selected,
        })
        .expect("应安装另一个受管 alpha");
    let UiOutcome::Inventory { entries, .. } = installed else {
        panic!("安装后应返回 Inventory");
    };
    let managed_member_id = entries
        .iter()
        .find(|entry| {
            entry.management_kind == ManagementKind::SkillYardManaged && entry.skill_name == "alpha"
        })
        .and_then(|entry| entry.member_id.clone())
        .expect("应找到已受管 alpha member");

    let takeover_root = home.join(".codex/skills/alpha");
    write_skill(&takeover_root, "alpha", "等待接管的另一份内容");
    let entries = match application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("应发现待接管 alpha")
    {
        UiOutcome::Inventory { entries, .. } => entries,
        _ => panic!("刷新应返回 Inventory"),
    };
    let observation_id = observation_id_at(&entries, &takeover_root);
    let takeover_plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: vec![observation_id],
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成待阻塞 Takeover Plan")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    drop(application);

    run_hard_exit_takeover_worker(
        &data_root,
        &home,
        &takeover_plan.id,
        "origin-before-progress",
    );
    let recovery = only_takeover_artifact(takeover_root.parent().expect("应有 Host 根目录"));
    fs::remove_dir_all(&recovery).expect("应替换 recovery 根目录");
    write_skill(&recovery, "unknown", "等待人工处理");

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = application
        .handle(UiIntent::GetStartupState)
        .expect("未知 recovery 应形成 blocked Takeover")
    else {
        panic!("启动应返回 Inventory");
    };
    assert_eq!(recovery_issues.len(), 1);

    application
        .handle(UiIntent::CreateMountPlan {
            member_id: managed_member_id.clone(),
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect_err("blocked Takeover 的原始 Host 路径不能被另一个 member 占用");
    let unrelated = application
        .handle(UiIntent::CreateMountPlan {
            member_id: managed_member_id,
            app_id: SupportedAppId::ClaudeCode,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect("不相关 Host 路径仍应生成 Mount Plan");
    assert!(matches!(unrelated, UiOutcome::MountPlan { .. }));
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
            request: takeover_plan_request! {
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
        .find(|entry| entry.member_id.as_deref() == Some(&plan.members[0].member_id))
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
        Path::new(&plan.members[0].expected_target)
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
            request: takeover_plan_request! {
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
            .any(|entry| entry.member_id.as_deref() == Some(plan.members[0].member_id.as_str())),
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
            request: takeover_plan_request! {
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
            Path::new(&plan.members[0].expected_target)
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
            .all(|mount| mount.expected_target == plan.members[0].expected_target)
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.member_id.as_deref() == Some(plan.members[0].member_id.as_str()))
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
            request: takeover_plan_request! {
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
                request: takeover_plan_request! {
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
fn takeover_resolves_global_project_scope_conflicts_before_one_atomic_confirmation() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let global_root = home.join(".codex/skills/alpha");
    let claude_global_root = home.join(".claude/skills/alpha");
    let project_a = sandbox.path().join("project-a");
    let project_b = sandbox.path().join("project-b");
    let project_a_root = project_a.join(".codex/skills/alpha");
    let project_b_root = project_b.join(".codex/skills/alpha");
    let claude_project_root = project_a.join(".claude/skills/alpha");
    write_skill(&global_root, "alpha", "最终不保留 global scope");
    write_skill(&claude_global_root, "alpha", "Claude 最终保留 global scope");
    write_skill(&project_a_root, "alpha", "用户选择的 project 内容");
    write_skill(&project_b_root, "alpha", "第二个 project 会统一内容");
    write_skill(
        &claude_project_root,
        "alpha",
        "Claude project 最终被 global 取代",
    );
    let project_a_root = fs::canonicalize(&project_a)
        .expect("应解析第一个 Project")
        .join(".codex/skills/alpha");
    let project_b_root = fs::canonicalize(&project_b)
        .expect("应解析第二个 Project")
        .join(".codex/skills/alpha");
    let claude_project_root = fs::canonicalize(&project_a)
        .expect("应解析 Claude Project")
        .join(".claude/skills/alpha");
    let selected_content = read_skill_file(&project_a_root);
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    application
        .handle(UiIntent::RegisterProject {
            root_path: path_text(&project_a),
        })
        .expect("应登记第一个 Project");
    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::RegisterProject {
            root_path: path_text(&project_b),
        })
        .expect("应登记第二个 Project")
    else {
        panic!("登记 Project 后应返回 Inventory");
    };
    let global_id = observation_id_at(&entries, &global_root);
    let project_a_id = observation_id_at(&entries, &project_a_root);
    let project_b_id = observation_id_at(&entries, &project_b_root);
    let claude_global_id = observation_id_at(&entries, &claude_global_root);
    let claude_project_id = observation_id_at(&entries, &claude_project_root);

    application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![global_id.clone(), project_a_id.clone()],
                selected_observation_id: project_a_id.clone(),
                preserved_observation_ids: vec![global_id.clone(), project_a_id.clone()],
                shared_targets: Vec::new(),
            },
        })
        .expect_err("同一应用不能同时保留 global 与 project scope");
    application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![global_id.clone(), project_a_id.clone()],
                selected_observation_id: project_a_id.clone(),
                preserved_observation_ids: Vec::new(),
                shared_targets: Vec::new(),
            },
        })
        .expect_err("scope 冲突必须选择 global 或 project，不能两种都删除");
    assert!(global_root.is_dir());
    assert!(project_a_root.is_dir());
    assert!(!contains_entries(&data_root.join("bundles")));

    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![
                    global_id,
                    project_a_id.clone(),
                    project_b_id.clone(),
                    claude_global_id.clone(),
                    claude_project_id,
                ],
                selected_observation_id: project_a_id.clone(),
                preserved_observation_ids: vec![project_a_id, project_b_id, claude_global_id],
                shared_targets: Vec::new(),
            },
        })
        .expect("选择 project scope 后应生成统一接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    assert_eq!(plan.targets.len(), 3);
    assert!(
        plan.targets
            .iter()
            .filter(|target| target.app_id == SupportedAppId::Codex)
            .all(|target| target.scope == MountScope::Project && target.project_id.is_some())
    );
    let claude_target = plan
        .targets
        .iter()
        .find(|target| target.app_id == SupportedAppId::ClaudeCode)
        .expect("Claude Code 应选择 global scope");
    assert_eq!(claude_target.scope, MountScope::Global);
    assert!(claude_target.project_id.is_none());

    let UiOutcome::Inventory { mounts, .. } = application
        .handle(UiIntent::ConfirmTakeoverPlan {
            plan_id: plan.id.clone(),
        })
        .expect("两个 Project 位置应由同一个事务接管")
    else {
        panic!("确认后应返回 Inventory");
    };
    assert!(matches!(
        fs::symlink_metadata(&global_root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    ));
    for root in [&project_a_root, &project_b_root] {
        assert_eq!(
            fs::read_link(root).expect("保留的 project 位置应成为 Mount"),
            Path::new(&plan.members[0].expected_target)
        );
        assert_eq!(read_skill_file(root), selected_content);
    }
    assert_eq!(
        fs::read_link(&claude_global_root).expect("Claude global 位置应成为 Mount"),
        Path::new(&plan.members[0].expected_target)
    );
    assert!(matches!(
        fs::symlink_metadata(&claude_project_root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    ));
    assert_eq!(mounts.len(), 3);
    assert_eq!(
        mounts
            .iter()
            .filter(|mount| mount.app_id == SupportedAppId::Codex
                && mount.scope == MountScope::Project
                && mount.project_id.is_some()
                && mount.project_display_name.is_some())
            .count(),
        2
    );
    assert!(
        mounts
            .iter()
            .any(|mount| mount.app_id == SupportedAppId::ClaudeCode
                && mount.scope == MountScope::Global
                && mount.project_id.is_none())
    );
}

#[test]
fn project_root_key_owns_the_app_and_project_replacement_invalidates_the_plan() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    fs::create_dir(&home).expect("应创建测试 home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let project = sandbox.path().join("claude-project");
    let visible_skill_root = project.join(".claude/skills/alpha");
    write_skill(&visible_skill_root, "alpha", "原 Project 内容");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::RegisterProject {
            root_path: path_text(&project),
        })
        .expect("应登记并扫描 Project")
    else {
        panic!("登记 Project 后应返回 Inventory");
    };
    let canonical_project = fs::canonicalize(&project).expect("应解析登记 Project");
    let canonical_skill_root = canonical_project.join(".claude/skills/alpha");
    let observation_id = observation_id_at(&entries, &canonical_skill_root);
    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: vec![observation_id],
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成 Claude Code project 接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    assert_eq!(plan.targets[0].app_id, SupportedAppId::ClaudeCode);
    assert_eq!(plan.targets[0].scope, MountScope::Project);

    let original_project = sandbox.path().join("claude-project-original");
    fs::rename(&canonical_project, &original_project).expect("应移动原 Project");
    write_skill(
        &canonical_project.join(".claude/skills/alpha"),
        "alpha",
        "替代 Project 不能被写入",
    );
    let replacement_content = read_skill_file(&canonical_skill_root);

    application
        .handle(UiIntent::ConfirmTakeoverPlan {
            plan_id: plan.id.clone(),
        })
        .expect_err("Project 根身份变化后必须拒绝确认");

    assert_eq!(read_skill_file(&canonical_skill_root), replacement_content);
    assert!(original_project.join(".claude/skills/alpha").is_dir());
    assert!(!data_root.join("bundles").join(plan.bundle_id).exists());
}

#[test]
fn shared_global_takeover_creates_only_selected_compatible_app_mounts() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let shared_root = home.join(".agents/skills/alpha");
    let codex_target = home.join(".codex/skills/alpha");
    let copilot_target = home.join(".copilot/skills/alpha");
    write_skill(&shared_root, "alpha", "共享目录中的原始内容");
    let selected_content = read_skill_file(&shared_root);
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let observation_id = match application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    {
        UiOutcome::Inventory { entries, .. } => observation_id_at(&entries, &shared_root),
        _ => panic!("首次扫描应返回 Inventory"),
    };

    write_skill(&codex_target, "alpha", "未被本 Plan 认领的已有内容");
    let occupied_content = read_skill_file(&codex_target);
    application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: Vec::new(),
                shared_targets: vec![TakeoverSharedTargetRequest {
                    shared_observation_id: observation_id.clone(),
                    app_id: SupportedAppId::Codex,
                }],
            },
        })
        .expect_err("共享接管不能覆盖未被本 Plan 认领的已有目标");
    assert_eq!(read_skill_file(&codex_target), occupied_content);
    fs::remove_dir_all(&codex_target).expect("应清理测试中的外部占用");

    application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: Vec::new(),
                shared_targets: vec![TakeoverSharedTargetRequest {
                    shared_observation_id: observation_id.clone(),
                    app_id: SupportedAppId::ClaudeCode,
                }],
            },
        })
        .expect_err("共享目录只能选择实际兼容的 Supported App");
    assert!(shared_root.is_dir());
    assert!(!codex_target.exists());

    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: Vec::new(),
                shared_targets: vec![
                    TakeoverSharedTargetRequest {
                        shared_observation_id: observation_id.clone(),
                        app_id: SupportedAppId::Codex,
                    },
                    TakeoverSharedTargetRequest {
                        shared_observation_id: observation_id,
                        app_id: SupportedAppId::GitHubCopilot,
                    },
                ],
            },
        })
        .expect("应生成共享目录接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    assert_eq!(plan.origins[0].app_id, None);
    assert_eq!(plan.origins[0].scope, None);
    assert_eq!(
        plan.origins[0].final_disposition,
        TakeoverOriginDisposition::Remove
    );
    assert_eq!(plan.targets.len(), 2);
    assert!(!codex_target.exists(), "Plan 不能创建目标父目录或 Mount");
    assert!(!copilot_target.exists(), "Plan 不能创建目标父目录或 Mount");

    let UiOutcome::Inventory { mounts, .. } = application
        .handle(UiIntent::ConfirmTakeoverPlan {
            plan_id: plan.id.clone(),
        })
        .expect("共享目录应在全部新 Mount 就绪后接管")
    else {
        panic!("确认后应返回 Inventory");
    };
    assert!(matches!(
        fs::symlink_metadata(&shared_root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    ));
    for target in [&codex_target, &copilot_target] {
        assert_eq!(
            fs::read_link(target).expect("选中的应用位置应成为 Mount"),
            Path::new(&plan.members[0].expected_target)
        );
        assert_eq!(read_skill_file(target), selected_content);
    }
    assert_eq!(mounts.len(), 2);
    assert!(mounts.iter().all(|mount| mount.scope == MountScope::Global));
}

#[test]
fn shared_project_takeover_derives_target_from_the_registered_project() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    fs::create_dir(&home).expect("应创建测试 home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let project = sandbox.path().join("shared-project");
    let shared_root = project.join(".agents/skills/alpha");
    write_skill(&shared_root, "alpha", "Project 共享目录内容");
    let canonical_project = fs::canonicalize(&project).expect("应解析登记 Project");
    let shared_root = canonical_project.join(".agents/skills/alpha");
    let copilot_target = canonical_project.join(".github/skills/alpha");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    let UiOutcome::Inventory {
        entries, projects, ..
    } = application
        .handle(UiIntent::RegisterProject {
            root_path: path_text(&project),
        })
        .expect("应登记并扫描 Project")
    else {
        panic!("登记 Project 后应返回 Inventory");
    };
    let project_id = projects[0].id.clone();
    let observation_id = observation_id_at(&entries, &shared_root);
    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: Vec::new(),
                shared_targets: vec![TakeoverSharedTargetRequest {
                    shared_observation_id: observation_id,
                    app_id: SupportedAppId::GitHubCopilot,
                }],
            },
        })
        .expect("应生成 Project 共享目录接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(plan.targets[0].scope, MountScope::Project);
    assert_eq!(
        plan.targets[0].project_id.as_deref(),
        Some(project_id.as_str())
    );
    assert_eq!(plan.targets[0].target_path, path_text(&copilot_target));

    let UiOutcome::Inventory { mounts, .. } = application
        .handle(UiIntent::ConfirmTakeoverPlan { plan_id: plan.id })
        .expect("应确认 Project 共享目录接管")
    else {
        panic!("确认后应返回 Inventory");
    };
    assert!(matches!(
        fs::symlink_metadata(&shared_root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    ));
    assert!(
        fs::symlink_metadata(&copilot_target)
            .expect("Project app 目标应存在")
            .file_type()
            .is_symlink()
    );
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].project_id.as_deref(), Some(project_id.as_str()));
}

#[test]
fn shared_target_failure_keeps_the_shared_entry_and_removes_new_mounts() {
    for failpoint in [
        LifecycleFailpoint::AfterFirstTakeoverTargetApplied,
        LifecycleFailpoint::AfterTakeoverOriginMovedBeforeProgress,
    ] {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let home = sandbox.path().join("home");
        let data_root = sandbox.path().join("application-support/SkillYard");
        let shared_root = home.join(".agents/skills/alpha");
        let codex_target = home.join(".codex/skills/alpha");
        let copilot_target = home.join(".copilot/skills/alpha");
        write_skill(&shared_root, "alpha", "失败时不能丢失共享入口");
        let original_identity = file_identity(&shared_root);
        let original_content = read_skill_file(&shared_root);
        let application = SkillYardApplication::new_with_lifecycle_failpoint(
            ApplicationPaths::for_home(data_root.clone(), home),
            PlatformInfo::supported_for_test(),
            failpoint,
        );
        let observation_id = match application
            .handle(UiIntent::StartInitialScan)
            .expect("首次扫描应成功")
        {
            UiOutcome::Inventory { entries, .. } => observation_id_at(&entries, &shared_root),
            _ => panic!("首次扫描应返回 Inventory"),
        };
        let plan = match application
            .handle(UiIntent::CreateTakeoverPlan {
                request: takeover_plan_request! {
                    observation_ids: vec![observation_id.clone()],
                    selected_observation_id: observation_id.clone(),
                    preserved_observation_ids: Vec::new(),
                    shared_targets: vec![
                        TakeoverSharedTargetRequest {
                            shared_observation_id: observation_id.clone(),
                            app_id: SupportedAppId::Codex,
                        },
                        TakeoverSharedTargetRequest {
                            shared_observation_id: observation_id,
                            app_id: SupportedAppId::GitHubCopilot,
                        },
                    ],
                },
            })
            .expect("应生成共享目录接管计划")
        {
            UiOutcome::TakeoverPlan { plan } => plan,
            _ => panic!("应返回 Takeover Plan"),
        };

        application
            .handle(UiIntent::ConfirmTakeoverPlan { plan_id: plan.id })
            .expect_err("第一个新 Mount 生效后应模拟整个事务失败");

        assert_eq!(file_identity(&shared_root), original_identity);
        assert_eq!(read_skill_file(&shared_root), original_content);
        for target in [&codex_target, &copilot_target] {
            assert!(matches!(
                fs::symlink_metadata(target),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
            ));
        }
        assert!(!contains_entries(&data_root.join("bundles")));
        assert!(!contains_entries(&data_root.join("staging")));
        assert!(!contains_entries(&data_root.join("journals")));
    }
}

#[test]
fn journal_temp_synced_before_rename_is_discarded_and_takeover_rolls_back() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let fixture = prepare_hard_exit_takeover(sandbox.path(), "alpha");
    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "journal-temp-before-rename",
    );
    let temporary = only_takeover_journal(&fixture.data_root);
    assert!(
        temporary
            .file_name()
            .expect("临时 Journal 应有文件名")
            .to_string_lossy()
            .starts_with(".takeover-")
    );

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("未完成 rename 的 Journal 写入应自动撤销")
    else {
        panic!("恢复后应返回 Inventory");
    };
    assert!(recovery_issues.is_empty());
    assert_eq!(
        file_identity(&fixture.skill_root),
        fixture.original_identity
    );
    assert_eq!(
        read_skill_file(&fixture.skill_root),
        fixture.original_content
    );
    assert_clean_takeover_artifacts(&fixture.data_root, &[&fixture.skill_root]);
    assert_takeover_database_counts(&fixture.data_root, (0, 0, 0, 0));
}

#[test]
fn malformed_journal_temp_is_blocked_and_preserved() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let fixture = prepare_hard_exit_takeover(sandbox.path(), "alpha");
    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "journal-temp-before-rename",
    );
    let temporary = only_takeover_journal(&fixture.data_root);
    fs::write(&temporary, b"unknown journal content").expect("应模拟未知临时 Journal");
    let evidence = fs::read(&temporary).expect("应读取未知临时 Journal");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("未知临时 Journal 只能阻塞相关事务")
    else {
        panic!("阻塞恢复后应返回 Inventory");
    };
    assert_eq!(recovery_issues.len(), 1);
    assert_eq!(fs::read(&temporary).unwrap(), evidence);
    assert_eq!(
        file_identity(&fixture.skill_root),
        fixture.original_identity
    );
}

#[test]
fn real_process_exits_before_takeover_commit_restore_each_original_state() {
    for point in [
        "transaction-only",
        "journal-before-phase",
        "staging-before-publish",
        "candidate-before-phase",
        "temporary-current-before-switch",
        "current-before-phase",
        "origin-before-progress",
        "mount-stage-before-progress",
        "origins-applied-before-state",
    ] {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let fixture = prepare_hard_exit_takeover(sandbox.path(), "alpha");

        run_hard_exit_takeover_worker(&fixture.data_root, &fixture.home, &fixture.plan_id, point);

        let reopened = SkillYardApplication::new(
            ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home.clone()),
            PlatformInfo::supported_for_test(),
        );
        let UiOutcome::Inventory {
            entries,
            mounts,
            recovery_issues,
            ..
        } = reopened
            .handle(UiIntent::GetStartupState)
            .unwrap_or_else(|error| panic!("{point} 重启后应自动恢复：{error}"))
        else {
            panic!("恢复后应返回 Inventory");
        };

        assert!(recovery_issues.is_empty(), "{point} 不应需要人工恢复");
        assert!(mounts.is_empty(), "{point} 不能留下 Mount 记录");
        assert!(
            entries
                .iter()
                .all(|entry| entry.management_kind != ManagementKind::SkillYardManaged),
            "{point} 不能留下受管 Bundle"
        );
        assert_eq!(
            file_identity(&fixture.skill_root),
            fixture.original_identity,
            "{point} 必须恢复原目录身份"
        );
        assert_eq!(
            read_skill_file(&fixture.skill_root),
            fixture.original_content,
            "{point} 必须恢复原 Skill 内容"
        );
        assert!(!contains_entries(&fixture.data_root.join("bundles")));
        assert_clean_takeover_artifacts(&fixture.data_root, &[&fixture.skill_root]);
        assert_takeover_database_counts(&fixture.data_root, (0, 0, 0, 0));
    }
}

#[test]
fn takeover_rollback_remains_replayable_across_two_recovery_process_exits() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let fixture = prepare_hard_exit_takeover(sandbox.path(), "alpha");
    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "origins-applied-before-state",
    );
    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "rollback-mount-before-progress",
    );
    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "rollback-origin-before-progress",
    );

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        mounts,
        recovery_issues,
        ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("回滚恢复连续退出后仍应回到原状态")
    else {
        panic!("恢复后应返回 Inventory");
    };
    assert!(mounts.is_empty());
    assert!(
        recovery_issues.is_empty(),
        "恢复不应进入人工处理：{recovery_issues:?}"
    );
    assert_eq!(
        file_identity(&fixture.skill_root),
        fixture.original_identity
    );
    assert_eq!(
        read_skill_file(&fixture.skill_root),
        fixture.original_content
    );
    assert!(!contains_entries(&fixture.data_root.join("bundles")));
    assert_clean_takeover_artifacts(&fixture.data_root, &[&fixture.skill_root]);
    assert_takeover_database_counts(&fixture.data_root, (0, 0, 0, 0));
}

#[test]
fn takeover_candidate_cleanup_resumes_after_a_mid_tree_process_exit() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let fixture = prepare_hard_exit_takeover(sandbox.path(), "alpha");
    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "current-before-phase",
    );
    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "candidate-cleanup",
    );

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        mounts,
        recovery_issues,
        ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("候选递归清理中断后应继续回滚")
    else {
        panic!("恢复后应返回 Inventory");
    };
    assert!(mounts.is_empty());
    assert!(
        recovery_issues.is_empty(),
        "恢复不应进入人工处理：{recovery_issues:?}"
    );
    assert_eq!(
        file_identity(&fixture.skill_root),
        fixture.original_identity
    );
    assert_eq!(
        read_skill_file(&fixture.skill_root),
        fixture.original_content
    );
    assert!(!contains_entries(&fixture.data_root.join("bundles")));
    assert_clean_takeover_artifacts(&fixture.data_root, &[&fixture.skill_root]);
    assert_takeover_database_counts(&fixture.data_root, (0, 0, 0, 0));
}

#[test]
fn real_process_exit_after_shared_origin_move_restores_shared_entry_and_new_mounts() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let shared_root = home.join(".agents/skills/alpha");
    let codex_target = home.join(".codex/skills/alpha");
    let copilot_target = home.join(".copilot/skills/alpha");
    write_skill(&shared_root, "alpha", "共享入口必须可恢复");
    let original_identity = file_identity(&shared_root);
    let original_content = read_skill_file(&shared_root);
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    let observation_id = match application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    {
        UiOutcome::Inventory { entries, .. } => observation_id_at(&entries, &shared_root),
        _ => panic!("首次扫描应返回 Inventory"),
    };
    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: Vec::new(),
                shared_targets: vec![
                    TakeoverSharedTargetRequest {
                        shared_observation_id: observation_id.clone(),
                        app_id: SupportedAppId::Codex,
                    },
                    TakeoverSharedTargetRequest {
                        shared_observation_id: observation_id,
                        app_id: SupportedAppId::GitHubCopilot,
                    },
                ],
            },
        })
        .expect("应生成共享接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    drop(application);

    run_hard_exit_takeover_worker(&data_root, &home, &plan.id, "origin-before-progress");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        mounts,
        recovery_issues,
        ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("共享入口移动窗口应自动恢复")
    else {
        panic!("恢复后应返回 Inventory");
    };
    assert!(recovery_issues.is_empty());
    assert!(mounts.is_empty());
    assert_eq!(file_identity(&shared_root), original_identity);
    assert_eq!(read_skill_file(&shared_root), original_content);
    for target in [&codex_target, &copilot_target] {
        assert!(
            fs::symlink_metadata(target).is_err(),
            "共享接管失败后不能留下新 Mount"
        );
    }
    assert!(!contains_entries(&data_root.join("bundles")));
    assert_clean_takeover_artifacts(&data_root, &[&shared_root]);
    assert_takeover_database_counts(&data_root, (0, 0, 0, 0));
}

#[test]
fn real_process_exit_after_takeover_state_commit_finishes_forward_cleanup() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let fixture = prepare_hard_exit_takeover(sandbox.path(), "alpha");

    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "state-before-journal",
    );

    let paths = ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home.clone());
    let reopened = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let first = reopened
        .handle(UiIntent::GetStartupState)
        .expect("SQLite 已提交后应保留新状态并完成清理");
    assert_managed_takeover_state(&first, &fixture.skill_root, &fixture.expected_target);
    assert_eq!(
        read_skill_file(&fixture.skill_root),
        fixture.original_content
    );
    assert_clean_takeover_artifacts(&fixture.data_root, &[&fixture.skill_root]);
    assert_takeover_database_counts(&fixture.data_root, (1, 1, 0, 0));

    let reopened_again = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let second = reopened_again
        .handle(UiIntent::GetStartupState)
        .expect("重复启动恢复必须幂等");
    assert_managed_takeover_state(&second, &fixture.skill_root, &fixture.expected_target);
}

#[test]
fn real_process_exit_during_committed_cleanup_continues_remaining_recoveries() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let codex_root = home.join(".codex/skills/alpha");
    let claude_root = home.join(".claude/skills/alpha");
    write_skill(&codex_root, "alpha", "最终统一使用的内容");
    write_skill(&claude_root, "alpha", "清理中断时仍要保留");
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
    let plan = match application
        .handle(UiIntent::CreateTakeoverPlan {
            request: takeover_plan_request! {
                observation_ids: vec![codex_id.clone(), claude_id.clone()],
                selected_observation_id: codex_id.clone(),
                preserved_observation_ids: vec![codex_id, claude_id],
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成多副本接管计划")
    {
        UiOutcome::TakeoverPlan { plan } => plan,
        _ => panic!("应返回 Takeover Plan"),
    };
    drop(application);

    run_hard_exit_takeover_worker(&data_root, &home, &plan.id, "first-recovery-removed");
    assert!(
        [&codex_root, &claude_root]
            .iter()
            .any(|root| has_takeover_artifact(root.parent().expect("应有 Skill 根目录"))),
        "第一次清理后退出时应留下尚未处理的 recovery"
    );

    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let reopened = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let first = reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启应继续剩余清理");
    assert_managed_takeover_state(&first, &codex_root, &plan.members[0].expected_target);
    assert_managed_takeover_state(&first, &claude_root, &plan.members[0].expected_target);
    assert_eq!(read_skill_file(&codex_root), selected_content);
    assert_eq!(read_skill_file(&claude_root), selected_content);
    assert_clean_takeover_artifacts(&data_root, &[&codex_root, &claude_root]);
    assert_takeover_database_counts(&data_root, (1, 2, 0, 0));

    let reopened_again = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let second = reopened_again
        .handle(UiIntent::GetStartupState)
        .expect("清理完成后的再次启动必须幂等");
    assert_managed_takeover_state(&second, &codex_root, &plan.members[0].expected_target);
    assert_managed_takeover_state(&second, &claude_root, &plan.members[0].expected_target);
}

#[test]
fn committed_recovery_tree_cleanup_resumes_after_a_mid_tree_process_exit() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let fixture = prepare_hard_exit_takeover(sandbox.path(), "alpha");
    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "recovery-removal-mid-tree",
    );

    let paths = ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home);
    let reopened = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let first = reopened
        .handle(UiIntent::GetStartupState)
        .expect("旧副本递归清理中断后应继续向前完成");
    assert_managed_takeover_state(&first, &fixture.skill_root, &fixture.expected_target);
    assert_clean_takeover_artifacts(&fixture.data_root, &[&fixture.skill_root]);
    assert_takeover_database_counts(&fixture.data_root, (1, 1, 0, 0));

    let reopened_again = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let second = reopened_again
        .handle(UiIntent::GetStartupState)
        .expect("递归清理完成后的再次启动必须幂等");
    assert_managed_takeover_state(&second, &fixture.skill_root, &fixture.expected_target);
}

#[test]
fn real_process_exit_after_takeover_journal_removal_only_forgets_terminal_record() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let fixture = prepare_hard_exit_takeover(sandbox.path(), "alpha");

    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "journal-removed",
    );
    assert!(!contains_entries(&fixture.data_root.join("journals")));
    assert_takeover_database_counts(&fixture.data_root, (1, 1, 1, 1));

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    let outcome = reopened
        .handle(UiIntent::GetStartupState)
        .expect("Journal 已删除后只需忘记终态事务");
    assert_managed_takeover_state(&outcome, &fixture.skill_root, &fixture.expected_target);
    assert_eq!(
        read_skill_file(&fixture.skill_root),
        fixture.original_content
    );
    assert_takeover_database_counts(&fixture.data_root, (1, 1, 0, 0));
    assert_clean_takeover_artifacts(&fixture.data_root, &[&fixture.skill_root]);
}

#[test]
fn unknown_replacement_in_precommit_recovery_is_blocked_and_preserved() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let fixture = prepare_hard_exit_takeover(sandbox.path(), "alpha");
    run_hard_exit_takeover_worker(
        &fixture.data_root,
        &fixture.home,
        &fixture.plan_id,
        "origin-before-progress",
    );

    let host_parent = fixture.skill_root.parent().expect("应有 Host Skill 根目录");
    let recovery = only_takeover_artifact(host_parent);
    fs::remove_dir_all(&recovery).expect("应替换事务 recovery 以模拟外部修改");
    write_skill(&recovery, "intruder", "不能被自动删除的未知内容");
    let unknown_content = read_skill_file(&recovery);
    let journal_path = only_takeover_journal(&fixture.data_root);
    let journal_evidence = fs::read(&journal_path).expect("阻塞前应保留 Journal 证据");

    let reopened = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    let UiOutcome::Inventory {
        recovery_issues, ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("未知替换只能阻塞相关接管事务")
    else {
        panic!("阻塞恢复后应返回 Inventory");
    };

    assert_eq!(recovery_issues.len(), 1);
    assert_eq!(read_skill_file(&recovery), unknown_content);
    assert_eq!(
        fs::read(&journal_path).expect("阻塞后必须保留 Journal"),
        journal_evidence
    );
    assert!(
        fs::symlink_metadata(&fixture.skill_root).is_err(),
        "无法证明 recovery 归属时不能伪造恢复结果"
    );
    let connection =
        Connection::open(fixture.data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let status = connection
        .query_row("SELECT status FROM takeover_transactions", [], |row| {
            row.get::<_, String>(0)
        })
        .expect("应读取阻塞事务状态");
    assert_eq!(status, "blocked");
}

/// 父测试通过精确过滤启动本测试；`_exit` 跳过析构，模拟真实应用进程中断。
#[test]
fn hard_exit_takeover_worker() {
    if env::var_os(HARD_EXIT_TAKEOVER_WORKER).is_none() {
        return;
    }
    let data_root = env::var_os(HARD_EXIT_TAKEOVER_DATA_ROOT).expect("子进程必须收到数据目录");
    let home = env::var_os(HARD_EXIT_TAKEOVER_HOME).expect("子进程必须收到 home");
    let plan_id = env::var(HARD_EXIT_TAKEOVER_PLAN_ID).expect("子进程必须收到 Takeover Plan ID");
    let point = env::var(HARD_EXIT_TAKEOVER_POINT).expect("子进程必须收到 Takeover failpoint");
    let failpoint = match point.as_str() {
        "transaction-only" => LifecycleFailpoint::HardExitAfterTakeoverTransactionRecord,
        "journal-temp-before-rename" => {
            LifecycleFailpoint::HardExitAfterTakeoverJournalTempSyncedBeforeRename
        }
        "journal-before-phase" => {
            LifecycleFailpoint::HardExitAfterTakeoverJournalWrittenBeforePhase
        }
        "staging-before-publish" => {
            LifecycleFailpoint::HardExitAfterTakeoverCandidatePreparedBeforePublish
        }
        "candidate-before-phase" => {
            LifecycleFailpoint::HardExitAfterTakeoverCandidatePublishedBeforePhase
        }
        "temporary-current-before-switch" => {
            LifecycleFailpoint::HardExitAfterTakeoverTemporaryCurrentCreatedBeforeSwitch
        }
        "current-before-phase" => {
            LifecycleFailpoint::HardExitAfterTakeoverCurrentSwitchedBeforePhase
        }
        "origin-before-progress" => {
            LifecycleFailpoint::HardExitAfterTakeoverOriginMovedBeforeProgress
        }
        "mount-stage-before-progress" => {
            LifecycleFailpoint::HardExitAfterTakeoverMountStagedBeforeProgress
        }
        "origins-applied-before-state" => {
            LifecycleFailpoint::HardExitAfterTakeoverOriginsAppliedBeforeState
        }
        "state-before-journal" => {
            LifecycleFailpoint::HardExitAfterTakeoverStateCommittedBeforeJournal
        }
        "first-recovery-removed" => {
            LifecycleFailpoint::HardExitAfterFirstTakeoverRecoveryRemovedBeforeProgress
        }
        "previous-content-isolated" => {
            LifecycleFailpoint::HardExitAfterTakeoverPreviousContentIsolated
        }
        "previous-content-removal" => {
            LifecycleFailpoint::HardExitDuringTakeoverPreviousContentRemoval
        }
        "journal-removed" => LifecycleFailpoint::HardExitAfterTakeoverJournalRemovedBeforeForget,
        "rollback-mount-before-progress" => {
            LifecycleFailpoint::HardExitAfterFirstTakeoverRollbackMountRemovedBeforeProgress
        }
        "rollback-origin-before-progress" => {
            LifecycleFailpoint::HardExitAfterFirstTakeoverRollbackOriginRestoredBeforeProgress
        }
        "candidate-cleanup" => LifecycleFailpoint::HardExitDuringTakeoverCandidateCleanup,
        "recovery-removal-mid-tree" => {
            LifecycleFailpoint::HardExitDuringFirstTakeoverRecoveryRemoval
        }
        _ => panic!("子进程收到未知 Takeover failpoint"),
    };
    let application = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.into(), home.into()),
        PlatformInfo::supported_for_test(),
        failpoint,
    );
    if matches!(
        point.as_str(),
        "rollback-mount-before-progress" | "rollback-origin-before-progress" | "candidate-cleanup"
    ) {
        application
            .handle(UiIntent::GetStartupState)
            .expect("恢复 hard-exit failpoint 必须在返回前终止进程");
    } else {
        application
            .handle(UiIntent::ConfirmTakeoverPlan { plan_id })
            .expect("hard-exit failpoint 必须在返回前终止进程");
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
            request: takeover_plan_request! {
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

struct TakeoverCrashFixture {
    data_root: PathBuf,
    home: PathBuf,
    plan_id: String,
    expected_target: String,
    skill_root: PathBuf,
    original_content: Vec<u8>,
    original_identity: (u64, u64, u32),
}

fn prepare_hard_exit_takeover(base: &Path, skill_name: &str) -> TakeoverCrashFixture {
    let home = base.join("home");
    let data_root = base.join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills").join(skill_name);
    write_skill(&skill_root, skill_name, "进程中断恢复测试");
    let original_content = read_skill_file(&skill_root);
    let original_identity = file_identity(&skill_root);
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
            request: takeover_plan_request! {
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
    drop(application);
    TakeoverCrashFixture {
        data_root,
        home,
        plan_id: plan.id,
        expected_target: plan.members[0].expected_target.clone(),
        skill_root,
        original_content,
        original_identity,
    }
}

fn run_hard_exit_takeover_worker(data_root: &Path, home: &Path, plan_id: &str, point: &str) {
    let status = Command::new(env::current_exe().expect("应找到当前测试二进制"))
        .args(["--exact", "hard_exit_takeover_worker", "--nocapture"])
        .env(HARD_EXIT_TAKEOVER_WORKER, "1")
        .env(HARD_EXIT_TAKEOVER_DATA_ROOT, data_root)
        .env(HARD_EXIT_TAKEOVER_HOME, home)
        .env(HARD_EXIT_TAKEOVER_PLAN_ID, plan_id)
        .env(HARD_EXIT_TAKEOVER_POINT, point)
        .status()
        .expect("应启动 Takeover hard-exit 子进程");
    assert_eq!(status.code(), Some(93), "子进程必须在 failpoint 直接退出");
}

fn assert_managed_takeover_state(outcome: &UiOutcome, target: &Path, expected_target: &str) {
    let UiOutcome::Inventory {
        entries,
        mounts,
        recovery_issues,
        ..
    } = outcome
    else {
        panic!("应返回 Inventory");
    };
    assert!(
        recovery_issues.is_empty(),
        "恢复不应进入人工处理：{recovery_issues:?}"
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
            .count(),
        1
    );
    assert!(
        mounts
            .iter()
            .any(|mount| mount.target_path == path_text(target))
    );
    assert_eq!(
        fs::read_link(target).expect("接管完成后 Host 位置应为 Mount"),
        Path::new(expected_target)
    );
}

fn assert_takeover_database_counts(data_root: &Path, expected: (i64, i64, i64, i64)) {
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let counts = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM bundles),
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
                ))
            },
        )
        .expect("应读取 Takeover 领域记录数量");
    assert_eq!(counts, expected);
}

fn assert_clean_takeover_artifacts(data_root: &Path, skill_roots: &[&Path]) {
    assert!(!contains_entries(&data_root.join("staging")));
    assert!(!contains_entries(&data_root.join("journals")));
    for skill_root in skill_roots {
        assert_no_takeover_artifacts(skill_root.parent().expect("应有 Host Skill 根目录"));
    }
}

fn has_takeover_artifact(parent: &Path) -> bool {
    fs::read_dir(parent).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(".skillyard-takeover-")
        })
    })
}

fn only_takeover_artifact(parent: &Path) -> PathBuf {
    let matches = fs::read_dir(parent)
        .expect("应读取 Host Skill 根目录")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            (name.starts_with(".skillyard-takeover-")
                && !name.starts_with(".skillyard-takeover-mount-"))
            .then(|| entry.path())
        })
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "应只有一个待恢复的原 Skill");
    matches.into_iter().next().expect("应存在 recovery")
}

fn only_takeover_journal(data_root: &Path) -> PathBuf {
    let journals = fs::read_dir(data_root.join("journals"))
        .expect("应读取 Takeover Journal 目录")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert_eq!(journals.len(), 1, "应只有一个 Takeover Journal");
    journals
        .into_iter()
        .next()
        .expect("应存在 Takeover Journal")
}

fn only_takeover_candidate_content(data_root: &Path) -> PathBuf {
    let bundle = fs::read_dir(data_root.join("bundles"))
        .expect("应读取受管 Bundle 根目录")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_dir())
        .expect("应存在待回滚 Bundle");
    let candidates = fs::read_dir(bundle.join("contents"))
        .expect("应读取待回滚 contents")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    assert_eq!(candidates.len(), 1, "应只有一个待清理候选根目录");
    candidates.into_iter().next().expect("应存在候选内容")
}

fn write_skill(root: &Path, name: &str, description: &str) {
    fs::create_dir_all(root).expect("应创建 Skill 根目录");
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
    )
    .expect("应写入有效 Skill");
}

fn write_global_lock_v3(home: &Path, skill_name: &str) {
    write_global_lock_v3_for(home, &[skill_name]);
}

fn write_global_lock_v3_for(home: &Path, skill_names: &[&str]) {
    write_global_lock_v3_for_source(home, skill_names, "owner/repository");
}

fn write_global_lock_v3_for_source(home: &Path, skill_names: &[&str], source: &str) {
    let lock_directory = home.join(".agents");
    fs::create_dir_all(&lock_directory).expect("应创建 lock 目录");
    let skills = skill_names
        .iter()
        .map(|skill_name| {
            (
                (*skill_name).to_owned(),
                serde_json::json!({
                    "source": source,
                    "sourceType": "github",
                    "sourceUrl": "https://github.com/owner/repository.git",
                    "pinnedRef": "release-2026-07",
                    "skillPath": format!("skills/{skill_name}/SKILL.md"),
                    "skillFolderHash": "0123456789abcdef0123456789abcdef01234567",
                    "installedAt": "2026-07-01T00:00:00.000Z",
                    "updatedAt": "2026-07-02T00:00:00.000Z"
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    fs::write(
        lock_directory.join(".skill-lock.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 3,
            "skills": skills
        }))
        .expect("应序列化 lock v3"),
    )
    .expect("应写入 lock v3");
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
