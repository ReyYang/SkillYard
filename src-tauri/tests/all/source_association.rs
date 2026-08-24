use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, InstallPlan, LifecycleFailpoint, ManagementKind, MergeContentChoice,
    MountScope, PlatformInfo, SkillYardApplication, SourceAssociationMode,
    SourceMemberMappingChoice, SourceRequest, SourceResponse, SourceTransport,
    SourceTransportError, SupportedAppId, UiIntent, UiOutcome,
};
use tempfile::tempdir;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

#[test]
fn direct_association_preserves_current_content_and_mount_while_saving_partial_mapping() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let local = install_folder_bundle(
        &application,
        sandbox.path(),
        "local-tools",
        &[("alpha", "local-alpha"), ("local-only", "local-only")],
    );
    let alpha_member_id = local.members["alpha"].clone();
    mount_global_codex(&application, &alpha_member_id);
    let archive_path = sandbox.path().join("downloads/upstream.skill");
    let source_id = create_idle_archive_source(
        &application,
        &data_root,
        &archive_path,
        &[("alpha", "upstream-alpha"), ("upstream-extra", "extra")],
    );

    let current_link = data_root
        .join("bundles")
        .join(&local.bundle_id)
        .join("current");
    let current_before = fs::read_link(&current_link).expect("应读取关联前 current");
    let payload_before = fs::read_to_string(
        current_link
            .join("members")
            .join("alpha")
            .join("payload.txt"),
    )
    .expect("应读取关联前内容");
    let mount_path = home.join(".codex/skills/alpha");
    let mount_before = fs::read_link(&mount_path).expect("应读取关联前 Mount");

    let UiOutcome::SourceAssociationPlan { plan } = application
        .handle(UiIntent::CreateSourceAssociationPlan {
            bundle_id: local.bundle_id.clone(),
            source_id: source_id.clone(),
            member_choices: vec![
                SourceMemberMappingChoice {
                    member_id: alpha_member_id,
                    source_relative_path: Some("skills/alpha".to_owned()),
                },
                SourceMemberMappingChoice {
                    member_id: local.members["local-only"].clone(),
                    source_relative_path: None,
                },
            ],
        })
        .expect("有效的对应与不对应选择应生成直接关联 Plan")
    else {
        panic!("应返回来源关联 Plan");
    };
    assert_eq!(plan.mode, SourceAssociationMode::Link);
    assert_eq!(plan.target_bundle_id, local.bundle_id);
    assert!(plan.retiring_bundle_id.is_none());
    assert!(plan.conflicts.is_empty());
    assert!(plan.blocking_issues.is_empty());
    assert_eq!(source_link_count(&data_root), 0, "创建 Plan 不能先写关联");
    assert_eq!(fs::read_link(&current_link).unwrap(), current_before);
    assert_eq!(fs::read_link(&mount_path).unwrap(), mount_before);
    assert_eq!(
        fs::read_to_string(
            current_link
                .join("members")
                .join("alpha")
                .join("payload.txt")
        )
        .unwrap(),
        payload_before
    );

    let UiOutcome::Inventory {
        entries, mounts, ..
    } = application
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: plan.id,
            content_choices: Vec::new(),
        })
        .expect("直接关联应通过一个 SQLite 事务确认")
    else {
        panic!("确认关联后应返回 Inventory");
    };
    assert_eq!(source_link_count(&data_root), 1);
    assert_eq!(source_member_link_count(&data_root), 1);
    assert_eq!(managed_member_count(&data_root), 2);
    assert!(
        entries
            .iter()
            .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
            .all(|entry| entry.skill_name != "upstream-extra"),
        "关联不能安装 Source 中其他成员"
    );
    assert_eq!(mounts.len(), 1);
    assert_eq!(fs::read_link(&current_link).unwrap(), current_before);
    assert_eq!(fs::read_link(&mount_path).unwrap(), mount_before);
    assert_eq!(
        fs::read_to_string(
            current_link
                .join("members")
                .join("alpha")
                .join("payload.txt")
        )
        .unwrap(),
        payload_before
    );

    // 部分“不对应”成员必须仍可被现有 Source 安装读取器读取。
    let UiOutcome::InstallPlan {
        plan: supplement_plan,
    } = application
        .handle(UiIntent::CreateArchiveInstallPlan {
            input_path: archive_path.to_string_lossy().into_owned(),
        })
        .expect("部分成员没有映射时仍应能读取 Source-backed Bundle")
    else {
        panic!("应返回补装 Plan");
    };
    application
        .handle(UiIntent::DiscardInstallPlan {
            plan_id: supplement_plan.id,
        })
        .expect("测试完成后应放弃补装 Plan");

    drop(application);
    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应读取已关联状态");
    assert_eq!(source_link_count(&data_root), 1);
    assert_eq!(source_member_link_count(&data_root), 1);
    assert_eq!(fs::read_link(&current_link).unwrap(), current_before);
    assert_eq!(fs::read_link(&mount_path).unwrap(), mount_before);
}

#[test]
fn all_not_corresponding_is_a_valid_source_association() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
    let local = install_folder_bundle(
        &application,
        sandbox.path(),
        "independent-local",
        &[("local-only", "local")],
    );
    let source_id = create_idle_archive_source(
        &application,
        &data_root,
        &sandbox.path().join("downloads/independent.skill"),
        &[("upstream-only", "upstream")],
    );
    let plan = create_association_plan(
        &application,
        &local.bundle_id,
        &source_id,
        vec![SourceMemberMappingChoice {
            member_id: local.members["local-only"].clone(),
            source_relative_path: None,
        }],
    );
    application
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: plan.id,
            content_choices: Vec::new(),
        })
        .expect("全部选择不对应仍应建立 Bundle 级 Source 关联");

    assert_eq!(source_link_count(&data_root), 1);
    assert_eq!(source_member_link_count(&data_root), 0);
    assert_eq!(managed_member_count(&data_root), 1);
}

#[test]
fn invalid_duplicate_mapping_and_changed_member_snapshot_leave_association_unchanged() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
    let local = install_folder_bundle(
        &application,
        sandbox.path(),
        "race-local",
        &[("alpha", "alpha"), ("beta", "beta")],
    );
    let source_id = create_idle_archive_source(
        &application,
        &data_root,
        &sandbox.path().join("downloads/race.skill"),
        &[("alpha", "upstream-alpha")],
    );

    application
        .handle(UiIntent::CreateSourceAssociationPlan {
            bundle_id: local.bundle_id.clone(),
            source_id: source_id.clone(),
            member_choices: vec![SourceMemberMappingChoice {
                member_id: local.members["alpha"].clone(),
                source_relative_path: Some("skills/alpha".to_owned()),
            }],
        })
        .expect_err("遗漏本地成员选择时不能生成 Plan");
    application
        .handle(UiIntent::CreateSourceAssociationPlan {
            bundle_id: local.bundle_id.clone(),
            source_id: source_id.clone(),
            member_choices: vec![
                SourceMemberMappingChoice {
                    member_id: local.members["alpha"].clone(),
                    source_relative_path: Some("skills/not-current".to_owned()),
                },
                SourceMemberMappingChoice {
                    member_id: local.members["beta"].clone(),
                    source_relative_path: None,
                },
            ],
        })
        .expect_err("非当前 Catalog 成员不能被选择为对应");
    let duplicate = application
        .handle(UiIntent::CreateSourceAssociationPlan {
            bundle_id: local.bundle_id.clone(),
            source_id: source_id.clone(),
            member_choices: vec![
                SourceMemberMappingChoice {
                    member_id: local.members["alpha"].clone(),
                    source_relative_path: Some("skills/alpha".to_owned()),
                },
                SourceMemberMappingChoice {
                    member_id: local.members["beta"].clone(),
                    source_relative_path: Some("skills/alpha".to_owned()),
                },
            ],
        })
        .expect_err("同一 Source Skill 不能对应两个本地成员");
    assert!(duplicate.to_string().contains("不能对应多个"));
    assert_eq!(source_association_plan_count(&data_root), 0);
    assert_eq!(source_link_count(&data_root), 0);

    let plan = create_association_plan(
        &application,
        &local.bundle_id,
        &source_id,
        vec![
            SourceMemberMappingChoice {
                member_id: local.members["alpha"].clone(),
                source_relative_path: Some("skills/alpha".to_owned()),
            },
            SourceMemberMappingChoice {
                member_id: local.members["beta"].clone(),
                source_relative_path: None,
            },
        ],
    );
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .execute(
            "UPDATE skill_members SET content_fingerprint = 'sha256:changed-after-plan'
             WHERE id = ?1",
            [&local.members["alpha"]],
        )
        .expect("应模拟 Plan 后成员快照变化");
    application
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: plan.id,
            content_choices: Vec::new(),
        })
        .expect_err("成员指纹变化后不能提交过期 Plan");
    assert_eq!(source_link_count(&data_root), 0);
    assert_eq!(source_member_link_count(&data_root), 0);
}

#[test]
fn github_direct_association_keeps_adopted_marker_empty_until_a_real_update() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(QueueTransport::default());
    let data_root = sandbox.path().join("application-support/SkillYard");
    let home = sandbox.path().join("home");
    let application = SkillYardApplication::new_with_source_transport(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
        transport.clone(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    let source_id = "source-anthropics-skills".to_owned();
    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        &github_catalog_archive(),
    );
    application
        .handle(UiIntent::ReloadGitHubSource {
            source_id: source_id.clone(),
        })
        .expect("应取得 GitHub Fresh Catalog");
    let local = install_folder_bundle(
        &application,
        sandbox.path(),
        "github-local",
        &[("alpha", "local-alpha")],
    );
    let plan = create_association_plan(
        &application,
        &local.bundle_id,
        &source_id,
        vec![SourceMemberMappingChoice {
            member_id: local.members["alpha"].clone(),
            source_relative_path: Some("skills/alpha".to_owned()),
        }],
    );
    application
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: plan.id,
            content_choices: Vec::new(),
        })
        .expect("GitHub Source 应能直接关联现有 Bundle");

    let adopted_marker = Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(
            "SELECT adopted_marker FROM source_bundle_links WHERE source_id = ?1",
            [&source_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .expect("应读取 GitHub 关联基线");
    assert!(
        adopted_marker.is_none(),
        "补充来源不能猜测本地内容采用过当前 commit"
    );
}

#[test]
fn merge_uses_only_two_local_currents_migrates_retiring_mount_and_survives_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let archive_path = sandbox.path().join("downloads/merge.skill");
    write_archive(
        &archive_path,
        &[
            ("alpha", "source-alpha"),
            ("beta", "source-beta"),
            ("source-only", "source-only"),
        ],
    );
    let source_install = create_archive_install_plan(&application, &archive_path);
    let source_id = source_id_for_locator(&data_root, &archive_path);
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: source_install.id,
            selected_candidate_ids: source_install
                .candidates
                .iter()
                .filter(|candidate| {
                    candidate.default_selected
                        && candidate.skill_name.as_deref() != Some("source-only")
                })
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("应先创建 Source 已关联的目标 Bundle");
    let target_bundle_id = source_bundle_id(&data_root, &source_id);
    let local = install_folder_bundle(
        &application,
        sandbox.path(),
        "retiring-local",
        &[
            ("alpha", "local-alpha"),
            ("beta", "local-beta"),
            ("local-only", "local-only"),
        ],
    );
    mount_global_codex(&application, &local.members["alpha"]);
    let mount_path = home.join(".codex/skills/alpha");
    let marker_before = adopted_marker(&data_root, &source_id);

    let plan = create_association_plan(
        &application,
        &local.bundle_id,
        &source_id,
        vec![
            SourceMemberMappingChoice {
                member_id: local.members["alpha"].clone(),
                source_relative_path: Some("skills/alpha".to_owned()),
            },
            SourceMemberMappingChoice {
                member_id: local.members["beta"].clone(),
                source_relative_path: Some("skills/beta".to_owned()),
            },
            SourceMemberMappingChoice {
                member_id: local.members["local-only"].clone(),
                source_relative_path: None,
            },
        ],
    );
    assert_eq!(plan.mode, SourceAssociationMode::Merge);
    assert_eq!(plan.target_bundle_id, target_bundle_id);
    assert_eq!(
        plan.retiring_bundle_id.as_deref(),
        Some(local.bundle_id.as_str())
    );
    assert_eq!(
        plan.conflicts.len(),
        2,
        "同名与同 Source 映射描述的同一候选集合只能生成一个冲突组"
    );
    assert!(
        plan.blocking_issues.is_empty(),
        "重复描述不能把两个独立的一对一冲突误判成交叉冲突"
    );
    assert!(
        plan.member_choices.iter().any(|choice| {
            choice.member_id == bundle_member_id(&data_root, &target_bundle_id, "alpha")
                && choice.source_relative_path.as_deref() == Some("skills/alpha")
        }),
        "merge 确认页必须同时展示 target Bundle 已有 Source mapping"
    );
    assert_eq!(source_link_count(&data_root), 1);
    assert_eq!(bundle_count(&data_root), 2);

    let content_choices = plan
        .conflicts
        .iter()
        .map(|conflict| {
            let member_id = conflict
                .candidate_member_ids
                .iter()
                .find(|member_id| {
                    plan.members.iter().any(|member| {
                        member.member_id == **member_id && member.bundle_id == target_bundle_id
                    })
                })
                .expect("每个冲突都应包含 target Bundle 候选")
                .clone();
            MergeContentChoice {
                conflict_id: conflict.id.clone(),
                member_id,
            }
        })
        .collect();
    fs::remove_file(&archive_path).expect("确认前删除 Source 文件，证明归并不重新获取 Source");
    application
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: plan.id,
            content_choices,
        })
        .expect("归并应通过同一个来源关联确认入口完成");
    assert_eq!(source_link_count(&data_root), 1);
    assert_eq!(bundle_count(&data_root), 1);
    assert_eq!(managed_member_count(&data_root), 3);
    assert_eq!(adopted_marker(&data_root, &source_id), marker_before);
    assert!(!data_root.join("bundles").join(&local.bundle_id).exists());
    assert_eq!(
        fs::read_link(&mount_path).unwrap(),
        data_root
            .join("bundles")
            .join(&target_bundle_id)
            .join("current/members/alpha")
    );
    assert!(
        fs::read_link(&mount_path).unwrap().is_absolute(),
        "迁移后的 Mount target 必须保持绝对路径"
    );
    assert_eq!(
        scalar_count(
            &data_root,
            "SELECT COUNT(*) FROM skill_members WHERE skill_name = 'source-only'"
        ),
        0,
        "归并不能安装 Catalog 额外成员"
    );

    drop(application);
    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    restarted
        .handle(UiIntent::GetStartupState)
        .expect("归并完成后重启应保持唯一 Bundle 和迁移后的 Mount");
    assert_eq!(bundle_count(&data_root), 1);
}

#[test]
fn merge_plan_blocks_mixed_global_and_project_mounts_within_one_conflict_group() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
    let archive_path = sandbox.path().join("downloads/mixed-scope.skill");
    write_archive(&archive_path, &[("alpha", "source-alpha")]);
    let source_install = create_archive_install_plan(&application, &archive_path);
    let source_id = source_id_for_locator(&data_root, &archive_path);
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: source_install.id,
            selected_candidate_ids: source_install
                .candidates
                .iter()
                .filter(|candidate| candidate.default_selected)
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("应先安装 Source 已关联的目标 Bundle");
    let target_bundle_id = source_bundle_id(&data_root, &source_id);
    let target_member_id = bundle_member_id(&data_root, &target_bundle_id, "alpha");
    mount_global_codex(&application, &target_member_id);

    let local = install_folder_bundle(
        &application,
        sandbox.path(),
        "mixed-scope-local",
        &[("alpha", "local-alpha")],
    );
    let project = sandbox.path().join("project");
    fs::create_dir(&project).expect("应创建测试 Project");
    let registered = application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记测试 Project");
    let UiOutcome::Inventory { projects, .. } = registered else {
        panic!("登记 Project 后应返回 Inventory");
    };
    mount_project_codex(&application, &local.members["alpha"], &projects[0].id);

    let plan = create_association_plan(
        &application,
        &local.bundle_id,
        &source_id,
        vec![SourceMemberMappingChoice {
            member_id: local.members["alpha"].clone(),
            source_relative_path: Some("skills/alpha".to_owned()),
        }],
    );
    assert_eq!(plan.conflicts.len(), 1);
    assert!(
        plan.blocking_issues
            .iter()
            .any(|issue| issue.contains("global") && issue.contains("project")),
        "同一最终冲突组在同一 Supported App 混用 scope 时必须阻塞归并"
    );
}

#[test]
fn merge_plan_blocks_one_final_skill_from_corresponding_to_two_source_members() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
    let archive_path = sandbox.path().join("downloads/source-path-conflict.skill");
    write_archive(
        &archive_path,
        &[("alpha", "source-alpha"), ("beta", "source-beta")],
    );
    let source_install = create_archive_install_plan(&application, &archive_path);
    let source_id = source_id_for_locator(&data_root, &archive_path);
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: source_install.id,
            selected_candidate_ids: source_install
                .candidates
                .iter()
                .filter(|candidate| candidate.skill_name.as_deref() == Some("alpha"))
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("应先创建只安装 alpha 的 Source-backed Bundle");
    let local = install_folder_bundle(
        &application,
        sandbox.path(),
        "local-source-path-conflict",
        &[("alpha", "local-alpha")],
    );

    let plan = create_association_plan(
        &application,
        &local.bundle_id,
        &source_id,
        vec![SourceMemberMappingChoice {
            member_id: local.members["alpha"].clone(),
            source_relative_path: Some("skills/beta".to_owned()),
        }],
    );

    assert_eq!(plan.conflicts.len(), 1, "同名内容仍应由用户选择唯一副本");
    assert!(
        plan.blocking_issues
            .iter()
            .any(|issue| issue.contains("包含多个 Source Skill 对应")),
        "计划阶段必须解释一个最终 Skill 不能同时对应两个 Source Member"
    );
    application
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: plan.id,
            content_choices: vec![MergeContentChoice {
                conflict_id: plan.conflicts[0].id.clone(),
                member_id: plan.conflicts[0].candidate_member_ids[0].clone(),
            }],
        })
        .expect_err("存在 Source 映射冲突时不能开始归并事务");
    assert_eq!(bundle_count(&data_root), 2);
    assert_eq!(source_association_transaction_count(&data_root), 0);
}

#[test]
fn merge_plan_tells_user_to_remove_and_recreate_mount_for_different_names() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
    let archive_path = sandbox.path().join("downloads/different-name.skill");
    write_archive(&archive_path, &[("alpha", "source-alpha")]);
    let source_install = create_archive_install_plan(&application, &archive_path);
    let source_id = source_id_for_locator(&data_root, &archive_path);
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: source_install.id,
            selected_candidate_ids: source_install
                .candidates
                .iter()
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("应安装 target Bundle");
    let local = install_folder_bundle(
        &application,
        sandbox.path(),
        "different-name-local",
        &[("renamed-alpha", "local-alpha")],
    );
    mount_global_codex(&application, &local.members["renamed-alpha"]);

    let plan = create_association_plan(
        &application,
        &local.bundle_id,
        &source_id,
        vec![SourceMemberMappingChoice {
            member_id: local.members["renamed-alpha"].clone(),
            source_relative_path: Some("skills/alpha".to_owned()),
        }],
    );
    assert!(
        plan.blocking_issues
            .iter()
            .any(|issue| issue.contains("移除 Mount") && issue.contains("重新挂载")),
        "不同名称映射到同一 Source Member 且已有 Mount 时应给出可执行处理方式"
    );
}

#[test]
fn merge_invalid_content_choices_have_no_domain_or_filesystem_side_effects() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, fixture) = prepare_merge_fixture(sandbox.path(), true);
    let target_before = fs::read_link(&fixture.target_current).expect("应读取 target current");
    let mount_before = fs::read_link(&fixture.mount_path).expect("应读取 retiring Mount");

    application
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: fixture.plan_id,
            content_choices: Vec::new(),
        })
        .expect_err("遗漏冲突选择必须在创建事务前失败");

    assert_eq!(bundle_count(&fixture.data_root), 2);
    assert_eq!(source_association_transaction_count(&fixture.data_root), 0);
    assert_eq!(journal_count(&fixture.data_root), 0);
    assert_eq!(
        fs::read_link(&fixture.target_current).unwrap(),
        target_before
    );
    assert_eq!(fs::read_link(&fixture.mount_path).unwrap(), mount_before);
}

#[test]
fn merge_interruption_before_current_switch_rolls_back_on_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, fixture) = prepare_merge_fixture(sandbox.path(), true);
    drop(application);
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home.clone()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterSourceAssociationCandidatePrepared,
    );
    interrupted
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: fixture.plan_id,
            content_choices: fixture.content_choices,
        })
        .expect_err("failpoint 应停在 target current 切换前");
    assert_eq!(
        fs::read_link(&fixture.target_current).unwrap(),
        fixture.old_target
    );
    assert_eq!(bundle_count(&fixture.data_root), 2);
    drop(interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启应清理未生效候选并保留两个 Bundle");
    assert_eq!(bundle_count(&fixture.data_root), 2);
    assert_eq!(source_association_transaction_count(&fixture.data_root), 0);
    assert_eq!(journal_count(&fixture.data_root), 0);
}

#[test]
fn merge_candidate_create_intent_recovers_directory_created_before_manifest() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, fixture) = prepare_merge_fixture(sandbox.path(), true);
    drop(application);
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home.clone()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterSourceAssociationCandidateDirectoryCreatedBeforeManifest,
    );
    interrupted
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: fixture.plan_id,
            content_choices: fixture.content_choices,
        })
        .expect_err("failpoint 应停在候选目录创建后、目录身份记录前");

    let transaction_id = source_association_transaction_id(&fixture.data_root);
    let candidate = fixture
        .data_root
        .join("bundles")
        .join(&fixture.target_bundle_id)
        .join("contents")
        .join(&transaction_id);
    assert!(candidate.is_dir(), "中断点应留下确定性的空候选目录");
    assert_eq!(
        fs::read_dir(&candidate)
            .expect("应读取中断后的候选目录")
            .count(),
        0
    );
    let journal: serde_json::Value = serde_json::from_slice(
        &fs::read(
            fixture
                .data_root
                .join("journals")
                .join(format!("{transaction_id}.json")),
        )
        .expect("应读取 create-intent Journal"),
    )
    .expect("create-intent Journal 应为 JSON");
    assert_eq!(journal["candidate_create_intent"], true);
    assert!(journal["candidate_cleanup"].is_null());
    drop(interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启应依据 create-intent 清理空候选并保留旧 Bundle");
    assert_eq!(
        fs::read_link(&fixture.target_current).unwrap(),
        fixture.old_target
    );
    assert!(!candidate.exists());
    assert_eq!(bundle_count(&fixture.data_root), 2);
    assert_eq!(source_association_transaction_count(&fixture.data_root), 0);
    assert_eq!(journal_count(&fixture.data_root), 0);
}

#[test]
fn aborted_merge_without_journal_is_forgotten_on_second_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, fixture) = prepare_merge_fixture(sandbox.path(), true);
    let retiring_current = fixture
        .data_root
        .join("bundles")
        .join(&fixture.retiring_bundle_id)
        .join("current");
    let retiring_old_target = fs::read_link(&retiring_current).expect("应读取 retiring 旧 current");
    drop(application);

    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home.clone()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterSourceAssociationCandidatePrepared,
    );
    interrupted
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: fixture.plan_id,
            content_choices: fixture.content_choices,
        })
        .expect_err("第一次中断应留下可回滚候选");
    let transaction_id = source_association_transaction_id(&fixture.data_root);
    drop(interrupted);

    let rollback_interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home.clone()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterSourceAssociationRollbackJournalRemovedBeforeForget,
    );
    rollback_interrupted
        .handle(UiIntent::GetStartupState)
        .expect_err("第二次中断应停在 Journal 删除后、终态事务清理前");
    assert_eq!(
        scalar_count(
            &fixture.data_root,
            "SELECT COUNT(*) FROM source_association_transactions WHERE status = 'aborted'"
        ),
        1
    );
    assert_eq!(journal_count(&fixture.data_root), 0);
    let target_bundle = fixture
        .data_root
        .join("bundles")
        .join(&fixture.target_bundle_id);
    assert!(
        !target_bundle
            .join("contents")
            .join(&transaction_id)
            .exists()
    );
    assert!(
        !target_bundle
            .join(format!(".current-{transaction_id}"))
            .exists()
    );
    assert_eq!(
        fs::read_link(&fixture.target_current).unwrap(),
        fixture.old_target
    );
    assert_eq!(
        fs::read_link(&retiring_current).unwrap(),
        retiring_old_target
    );
    drop(rollback_interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    restarted
        .handle(UiIntent::GetStartupState)
        .expect("无 Journal 的 aborted 事务应在核验旧状态后幂等清理");
    assert_eq!(source_association_transaction_count(&fixture.data_root), 0);
    assert_eq!(bundle_count(&fixture.data_root), 2);
    assert_eq!(
        fs::read_link(&fixture.target_current).unwrap(),
        fixture.old_target
    );
    assert_eq!(
        fs::read_link(&retiring_current).unwrap(),
        retiring_old_target
    );
}

#[test]
fn merge_interruption_after_current_switch_completes_forward_on_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, fixture) = prepare_merge_fixture(sandbox.path(), true);
    drop(application);
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home.clone()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterSourceAssociationCurrentActivated,
    );
    interrupted
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: fixture.plan_id,
            content_choices: fixture.content_choices,
        })
        .expect_err("failpoint 应停在 target current 已生效后");
    assert_ne!(
        fs::read_link(&fixture.target_current).unwrap(),
        fixture.old_target
    );
    assert_eq!(bundle_count(&fixture.data_root), 2);
    drop(interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启应沿已生效方向完成 Mount、SQLite 和清理");
    assert_eq!(bundle_count(&fixture.data_root), 1);
    assert_eq!(source_association_transaction_count(&fixture.data_root), 0);
    assert_eq!(
        fs::read_link(&fixture.mount_path).unwrap(),
        fixture
            .data_root
            .join("bundles")
            .join(&fixture.target_bundle_id)
            .join("current/members/alpha")
    );
}

#[test]
fn merge_interruption_after_mounts_applied_completes_domain_commit_on_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, fixture) = prepare_merge_fixture(sandbox.path(), true);
    drop(application);
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home.clone()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterSourceAssociationMountsApplied,
    );
    interrupted
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: fixture.plan_id,
            content_choices: fixture.content_choices,
        })
        .expect_err("failpoint 应停在 Mount 已生效、领域提交尚未完成时");
    let phase = Connection::open(fixture.data_root.join("skillyard.sqlite3"))
        .expect("应打开测试数据库")
        .query_row(
            "SELECT phase FROM source_association_transactions",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("应读取归并事务阶段");
    assert_eq!(phase, "mounts_applied");
    assert_eq!(bundle_count(&fixture.data_root), 2);
    drop(interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启应从 mounts_applied 继续提交领域状态和清理");
    assert_eq!(bundle_count(&fixture.data_root), 1);
    assert_eq!(source_association_transaction_count(&fixture.data_root), 0);
    assert_eq!(
        fs::read_link(&fixture.mount_path).unwrap(),
        fixture
            .data_root
            .join("bundles")
            .join(&fixture.target_bundle_id)
            .join("current/members/alpha")
    );
}

#[test]
fn merge_recovery_blocks_when_retiring_mount_is_replaced_after_activation() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, fixture) = prepare_merge_fixture(sandbox.path(), true);
    drop(application);
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home.clone()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterSourceAssociationCurrentActivated,
    );
    interrupted
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: fixture.plan_id,
            content_choices: fixture.content_choices,
        })
        .expect_err("failpoint 应停在 Mount 改写前");
    fs::remove_file(&fixture.mount_path).expect("应移除原 retiring Mount");
    fs::write(&fixture.mount_path, "unknown").expect("应模拟外部未知内容占用 Mount");
    drop(interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    let outcome = restarted
        .handle(UiIntent::GetStartupState)
        .expect("未知 Mount 应记录 blocked，但不能阻止只读 Inventory");
    assert!(
        matches!(outcome, UiOutcome::Inventory { .. }),
        "启动恢复后仍应返回 Inventory"
    );
    assert_eq!(bundle_count(&fixture.data_root), 2);
    assert_eq!(
        scalar_count(
            &fixture.data_root,
            "SELECT COUNT(*) FROM source_association_transactions WHERE status = 'blocked'"
        ),
        1
    );
    assert_eq!(
        fs::read_to_string(&fixture.mount_path).unwrap(),
        "unknown",
        "恢复阻塞不能覆盖外部内容"
    );
}

#[test]
fn merge_cleanup_blocks_if_target_current_changes_after_domain_commit() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, fixture) = prepare_merge_fixture(sandbox.path(), false);
    drop(application);
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home.clone()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterSourceAssociationStateCommitted,
    );
    interrupted
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: fixture.plan_id,
            content_choices: fixture.content_choices,
        })
        .expect_err("failpoint 应停在领域提交后、破坏性清理前");
    let target_bundle = fixture
        .data_root
        .join("bundles")
        .join(&fixture.target_bundle_id);
    fs::create_dir(target_bundle.join("contents/external")).expect("应创建第三方内容目录");
    let replacement = target_bundle.join(".external-current");
    std::os::unix::fs::symlink("contents/external", &replacement).expect("应创建外部 current");
    fs::rename(&replacement, &fixture.target_current).expect("应原子替换 target current");
    drop(interrupted);

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
        PlatformInfo::supported_for_test(),
    );
    let outcome = restarted
        .handle(UiIntent::GetStartupState)
        .expect("current 外改应记录 blocked，但 Inventory 仍可读");
    assert!(matches!(outcome, UiOutcome::Inventory { .. }));
    assert_eq!(
        fs::read_link(&fixture.target_current).unwrap(),
        PathBuf::from("contents/external")
    );
    assert!(
        fixture
            .data_root
            .join("bundles")
            .join(&fixture.target_bundle_id)
            .join(&fixture.old_target)
            .exists(),
        "current 外改后不能删除 target 旧内容"
    );
    assert!(
        fixture
            .data_root
            .join("bundles")
            .join(&fixture.retiring_bundle_id)
            .exists(),
        "current 外改后不能删除 retiring Managed Bundle Directory"
    );
    assert_eq!(
        scalar_count(
            &fixture.data_root,
            "SELECT COUNT(*) FROM source_association_transactions WHERE status = 'blocked'"
        ),
        1
    );
}

#[test]
fn merge_recovery_blocks_tampered_static_journal_contract_but_inventory_remains_readable() {
    for case in [
        "source-mapping",
        "retiring-mount",
        "relative-target",
        "unsafe-private-name",
    ] {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let base = sandbox.path().join(case);
        let (application, fixture) = prepare_merge_fixture(&base, true);
        drop(application);
        let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
            ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home.clone()),
            PlatformInfo::supported_for_test(),
            LifecycleFailpoint::AfterSourceAssociationCurrentActivated,
        );
        interrupted
            .handle(UiIntent::ConfirmSourceAssociationPlan {
                plan_id: fixture.plan_id,
                content_choices: fixture.content_choices,
            })
            .expect_err("failpoint 应保留待恢复 Journal");
        drop(interrupted);
        let journal_path = fs::read_dir(fixture.data_root.join("journals"))
            .expect("应读取 Journal 目录")
            .next()
            .expect("应存在来源关联 Journal")
            .expect("应读取 Journal entry")
            .path();
        let mut journal: serde_json::Value =
            serde_json::from_slice(&fs::read(&journal_path).expect("应读取来源关联 Journal"))
                .expect("Journal 应为 JSON");
        match case {
            "source-mapping" => {
                journal["source_mappings"][0]["member_id"] =
                    serde_json::Value::String("member-tampered".to_owned());
            }
            "retiring-mount" => {
                journal["retiring_mounts"][0]["mount"]["expectedTarget"] =
                    serde_json::Value::String("/tmp/tampered".to_owned());
            }
            "relative-target" => {
                journal["retiring_mounts"][0]["final_expected_target"] =
                    serde_json::Value::String("relative/current/members/alpha".to_owned());
            }
            "unsafe-private-name" => {
                journal["retiring_mounts"][0]["prepared_name"] =
                    serde_json::Value::String("../escape".to_owned());
            }
            _ => unreachable!("测试 case 已穷举"),
        }
        fs::write(
            &journal_path,
            serde_json::to_vec(&journal).expect("应重新编码 Journal"),
        )
        .expect("应写入篡改 Journal");

        let restarted = SkillYardApplication::new(
            ApplicationPaths::for_home(fixture.data_root.clone(), fixture.home),
            PlatformInfo::supported_for_test(),
        );
        let outcome = restarted
            .handle(UiIntent::GetStartupState)
            .expect("静态合同篡改应被 blocked，但 Inventory 仍可读");
        assert!(matches!(outcome, UiOutcome::Inventory { .. }));
        assert_eq!(
            scalar_count(
                &fixture.data_root,
                "SELECT COUNT(*) FROM source_association_transactions WHERE status = 'blocked'"
            ),
            1,
            "case {case} 应记录 blocked"
        );
    }
}

#[test]
fn merge_begin_preflight_rejects_stale_source_bundle_and_mapping_before_filesystem_writes() {
    for case in [
        "source-generation",
        "target-snapshot",
        "retiring-snapshot",
        "mapping-catalog",
        "filesystem-current",
        "mount-observation",
    ] {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let base = sandbox.path().join(case);
        let (application, fixture) = prepare_merge_fixture(&base, true);
        let connection = Connection::open(fixture.data_root.join("skillyard.sqlite3"))
            .expect("应打开真实 SQLite");
        match case {
            "source-generation" => {
                connection
                    .execute(
                        "UPDATE sources
                         SET catalog_generation = catalog_generation + 1,
                             catalog_marker = 'changed-after-plan'
                         WHERE id = ?1",
                        [&fixture.source_id],
                    )
                    .expect("应模拟 Source generation 变化");
            }
            "target-snapshot" => {
                connection
                    .execute(
                        "UPDATE bundles SET display_name = 'changed-target' WHERE id = ?1",
                        [&fixture.target_bundle_id],
                    )
                    .expect("应模拟 target Bundle 快照变化");
            }
            "retiring-snapshot" => {
                connection
                    .execute(
                        "UPDATE bundles SET display_name = 'changed-retiring' WHERE id = ?1",
                        [&fixture.retiring_bundle_id],
                    )
                    .expect("应模拟 retiring Bundle 快照变化");
            }
            "mapping-catalog" => {
                connection
                    .execute(
                        "UPDATE source_catalog_members
                         SET selectable = 0
                         WHERE source_id = ?1 AND relative_path = 'skills/alpha'",
                        [&fixture.source_id],
                    )
                    .expect("应模拟 mapping 对应的 Catalog 成员失效");
            }
            "filesystem-current" => {
                fs::remove_file(&fixture.target_current).expect("应移除原 target current");
                std::os::unix::fs::symlink("contents/external", &fixture.target_current)
                    .expect("应模拟 current 在 Plan 后外改");
            }
            "mount-observation" => {
                fs::remove_file(&fixture.mount_path).expect("应移除原 retiring Mount");
                fs::write(&fixture.mount_path, "unknown").expect("应模拟 Mount 在 Plan 后外改");
            }
            _ => unreachable!("测试 case 已穷举"),
        }
        let current_before = fs::read_link(&fixture.target_current).expect("应读取 target current");
        let mount_before = (case != "mount-observation")
            .then(|| fs::read_link(&fixture.mount_path).expect("应读取 retiring Mount"));
        application
            .handle(UiIntent::ConfirmSourceAssociationPlan {
                plan_id: fixture.plan_id,
                content_choices: fixture.content_choices,
            })
            .expect_err("stale Plan 必须在任何文件系统写入前失败");
        assert_eq!(
            source_association_transaction_count(&fixture.data_root),
            0,
            "case {case} 不能创建事务"
        );
        assert_eq!(journal_count(&fixture.data_root), 0);
        assert_eq!(
            fs::read_link(&fixture.target_current).unwrap(),
            current_before
        );
        if let Some(mount_before) = mount_before {
            assert_eq!(fs::read_link(&fixture.mount_path).unwrap(), mount_before);
        } else {
            assert_eq!(fs::read_to_string(&fixture.mount_path).unwrap(), "unknown");
        }
        assert_eq!(
            fs::read_dir(
                fixture
                    .data_root
                    .join("bundles")
                    .join(&fixture.target_bundle_id)
                    .join("contents")
            )
            .expect("应读取 target contents")
            .count(),
            1,
            "case {case} 不能写入候选目录"
        );
    }
}

struct MergeFixture {
    data_root: PathBuf,
    home: PathBuf,
    source_id: String,
    target_bundle_id: String,
    retiring_bundle_id: String,
    target_current: PathBuf,
    mount_path: PathBuf,
    plan_id: String,
    content_choices: Vec<MergeContentChoice>,
    old_target: PathBuf,
}

fn prepare_merge_fixture(
    base: &Path,
    with_retiring_mount: bool,
) -> (SkillYardApplication, MergeFixture) {
    let (application, data_root, home) = ready_application(base);
    let archive_path = base.join("downloads/recovery-merge.skill");
    write_archive(&archive_path, &[("alpha", "source-alpha")]);
    let source_install = create_archive_install_plan(&application, &archive_path);
    let source_id = source_id_for_locator(&data_root, &archive_path);
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: source_install.id,
            selected_candidate_ids: source_install
                .candidates
                .iter()
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("应安装关联 Source 的 target Bundle");
    let target_bundle_id = source_bundle_id(&data_root, &source_id);
    let local = install_folder_bundle(
        &application,
        base,
        "recovery-retiring",
        &[("alpha", "local-alpha"), ("local-only", "local-only")],
    );
    if with_retiring_mount {
        mount_global_codex(&application, &local.members["alpha"]);
    }
    let plan = create_association_plan(
        &application,
        &local.bundle_id,
        &source_id,
        vec![
            SourceMemberMappingChoice {
                member_id: local.members["alpha"].clone(),
                source_relative_path: Some("skills/alpha".to_owned()),
            },
            SourceMemberMappingChoice {
                member_id: local.members["local-only"].clone(),
                source_relative_path: None,
            },
        ],
    );
    assert_eq!(plan.conflicts.len(), 1);
    let target_alpha = bundle_member_id(&data_root, &target_bundle_id, "alpha");
    let content_choices = vec![MergeContentChoice {
        conflict_id: plan.conflicts[0].id.clone(),
        member_id: target_alpha,
    }];
    let target_current = data_root
        .join("bundles")
        .join(&target_bundle_id)
        .join("current");
    let old_target = fs::read_link(&target_current).expect("应读取原 target current");
    (
        application,
        MergeFixture {
            data_root,
            home: home.clone(),
            source_id,
            target_bundle_id,
            retiring_bundle_id: local.bundle_id,
            target_current,
            mount_path: home.join(".codex/skills/alpha"),
            plan_id: plan.id,
            content_choices,
            old_target,
        },
    )
}

#[derive(Debug)]
struct InstalledBundle {
    bundle_id: String,
    members: BTreeMap<String, String>,
}

fn ready_application(base: &Path) -> (SkillYardApplication, PathBuf, PathBuf) {
    let data_root = base.join("application-support/SkillYard");
    let home = base.join("home");
    fs::create_dir_all(&home).expect("应创建隔离 home");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    (application, data_root, home)
}

fn install_folder_bundle(
    application: &SkillYardApplication,
    base: &Path,
    directory_name: &str,
    skills: &[(&str, &str)],
) -> InstalledBundle {
    let input = base.join("downloads").join(directory_name);
    for (skill_name, payload) in skills {
        write_skill(&input.join(skill_name), skill_name, payload);
    }
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateFolderInstallPlan {
            input_path: input.to_string_lossy().into_owned(),
        })
        .expect("本地 Bundle 应生成安装 Plan")
    else {
        panic!("应返回安装 Plan");
    };
    let selected_candidate_ids = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.default_selected)
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids,
        })
        .expect("本地 Bundle 应安装成功")
    else {
        panic!("安装后应返回 Inventory");
    };
    let expected_names = skills.iter().map(|(name, _)| *name).collect::<Vec<_>>();
    let managed = entries
        .iter()
        .filter(|entry| {
            entry.management_kind == ManagementKind::SkillYardManaged
                && entry.source_display_name.is_none()
                && expected_names.contains(&entry.skill_name.as_str())
        })
        .collect::<Vec<_>>();
    let bundle_id = managed
        .first()
        .and_then(|entry| entry.bundle_id.clone())
        .expect("应找到新安装的本地 Bundle");
    let members = managed
        .into_iter()
        .filter(|entry| entry.bundle_id.as_deref() == Some(bundle_id.as_str()))
        .map(|entry| {
            (
                entry.skill_name.clone(),
                entry
                    .member_id
                    .clone()
                    .expect("受管 Skill 应公开 Member ID"),
            )
        })
        .collect();
    InstalledBundle { bundle_id, members }
}

fn create_idle_archive_source(
    application: &SkillYardApplication,
    data_root: &Path,
    archive_path: &Path,
    skills: &[(&str, &str)],
) -> String {
    write_archive(archive_path, skills);
    let plan = create_archive_install_plan(application, archive_path);
    application
        .handle(UiIntent::DiscardInstallPlan { plan_id: plan.id })
        .expect("只登记 Source 时应放弃安装 Plan");
    source_id_for_locator(data_root, archive_path)
}

fn create_archive_install_plan(
    application: &SkillYardApplication,
    archive_path: &Path,
) -> InstallPlan {
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateArchiveInstallPlan {
            input_path: archive_path.to_string_lossy().into_owned(),
        })
        .expect("有效归档应生成安装 Plan")
    else {
        panic!("应返回归档安装 Plan");
    };
    plan
}

fn create_association_plan(
    application: &SkillYardApplication,
    bundle_id: &str,
    source_id: &str,
    member_choices: Vec<SourceMemberMappingChoice>,
) -> skillyard_lib::SourceAssociationPlan {
    let UiOutcome::SourceAssociationPlan { plan } = application
        .handle(UiIntent::CreateSourceAssociationPlan {
            bundle_id: bundle_id.to_owned(),
            source_id: source_id.to_owned(),
            member_choices,
        })
        .expect("应生成唯一来源关联 Plan")
    else {
        panic!("应返回来源关联 Plan");
    };
    plan
}

fn mount_global_codex(application: &SkillYardApplication, member_id: &str) {
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

fn mount_project_codex(application: &SkillYardApplication, member_id: &str, project_id: &str) {
    let UiOutcome::MountPlan { plan } = application
        .handle(UiIntent::CreateMountPlan {
            member_id: member_id.to_owned(),
            app_id: SupportedAppId::Codex,
            scope: MountScope::Project,
            project_id: Some(project_id.to_owned()),
        })
        .expect("应生成 Codex project Mount Plan")
    else {
        panic!("应返回 Mount Plan");
    };
    application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect("应完成 Codex project Mount");
}

fn write_skill(root: &Path, name: &str, payload: &str) {
    fs::create_dir_all(root).expect("应创建 Skill 目录");
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} description\n---\n"),
    )
    .expect("应写入 SKILL.md");
    fs::write(root.join("payload.txt"), payload).expect("应写入测试内容");
}

fn write_archive(path: &Path, skills: &[(&str, &str)]) {
    fs::create_dir_all(path.parent().expect("归档应有父目录")).expect("应创建下载目录");
    fs::write(path, archive_bytes("repository", skills)).expect("应写入归档");
}

fn archive_bytes(wrapper: &str, skills: &[(&str, &str)]) -> Vec<u8> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o100644);
    for (name, payload) in skills {
        for (file_name, contents) in [
            (
                "SKILL.md",
                format!("---\nname: {name}\ndescription: {name} description\n---\n"),
            ),
            ("payload.txt", (*payload).to_owned()),
        ] {
            archive
                .start_file(format!("{wrapper}/skills/{name}/{file_name}"), options)
                .expect("应开始 ZIP entry");
            archive
                .write_all(contents.as_bytes())
                .expect("应写入 ZIP entry");
        }
    }
    archive.finish().expect("应完成 ZIP").into_inner()
}

fn github_catalog_archive() -> Vec<u8> {
    archive_bytes(
        "repository-sha",
        &[("alpha", "upstream-alpha"), ("beta", "upstream-beta")],
    )
}

fn source_id_for_locator(data_root: &Path, archive_path: &Path) -> String {
    let locator = fs::canonicalize(archive_path)
        .expect("归档应可规范化")
        .to_string_lossy()
        .into_owned();
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(
            "SELECT id FROM sources WHERE kind = 'archive' AND locator = ?1",
            [locator],
            |row| row.get(0),
        )
        .expect("应找到已登记的归档 Source")
}

fn source_bundle_id(data_root: &Path, source_id: &str) -> String {
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(
            "SELECT bundle_id FROM source_bundle_links WHERE source_id = ?1",
            [source_id],
            |row| row.get(0),
        )
        .expect("Source 应已关联 Bundle")
}

fn adopted_marker(data_root: &Path, source_id: &str) -> Option<String> {
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(
            "SELECT adopted_marker FROM source_bundle_links WHERE source_id = ?1",
            [source_id],
            |row| row.get(0),
        )
        .expect("应读取 Source adopted marker")
}

fn bundle_member_id(data_root: &Path, bundle_id: &str, skill_name: &str) -> String {
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(
            "SELECT id FROM skill_members WHERE bundle_id = ?1 AND skill_name = ?2",
            [bundle_id, skill_name],
            |row| row.get(0),
        )
        .expect("应找到 Bundle 成员")
}

fn scalar_count(data_root: &Path, sql: &str) -> i64 {
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(sql, [], |row| row.get(0))
        .expect("应读取 SQLite 计数")
}

fn source_link_count(data_root: &Path) -> i64 {
    scalar_count(data_root, "SELECT COUNT(*) FROM source_bundle_links")
}

fn source_member_link_count(data_root: &Path) -> i64 {
    scalar_count(data_root, "SELECT COUNT(*) FROM source_member_links")
}

fn managed_member_count(data_root: &Path) -> i64 {
    scalar_count(data_root, "SELECT COUNT(*) FROM skill_members")
}

fn source_association_plan_count(data_root: &Path) -> i64 {
    scalar_count(data_root, "SELECT COUNT(*) FROM source_association_plans")
}

fn source_association_transaction_count(data_root: &Path) -> i64 {
    scalar_count(
        data_root,
        "SELECT COUNT(*) FROM source_association_transactions",
    )
}

fn source_association_transaction_id(data_root: &Path) -> String {
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(
            "SELECT id FROM source_association_transactions",
            [],
            |row| row.get(0),
        )
        .expect("应读取来源关联事务 id")
}

fn journal_count(data_root: &Path) -> usize {
    fs::read_dir(data_root.join("journals"))
        .expect("应读取 Journal 目录")
        .count()
}

fn bundle_count(data_root: &Path) -> i64 {
    scalar_count(data_root, "SELECT COUNT(*) FROM bundles")
}

#[derive(Default)]
struct QueueTransport {
    responses: Mutex<VecDeque<(u16, Vec<u8>)>>,
}

impl QueueTransport {
    fn enqueue_catalog(&self, full_name: &str, tracked_ref: &str, sha: &str, archive: &[u8]) {
        self.enqueue(
            200,
            format!(
                r#"{{"full_name":"{full_name}","default_branch":"{tracked_ref}","private":false}}"#
            )
            .into_bytes(),
        );
        self.enqueue(200, format!(r#"{{"sha":"{sha}"}}"#).into_bytes());
        self.enqueue(200, archive.to_vec());
    }

    fn enqueue(&self, status: u16, body: Vec<u8>) {
        self.responses
            .lock()
            .expect("应写入响应队列")
            .push_back((status, body));
    }
}

impl SourceTransport for QueueTransport {
    fn get(&self, request: SourceRequest) -> Result<SourceResponse, SourceTransportError> {
        let (status, body) = self
            .responses
            .lock()
            .expect("应读取响应队列")
            .pop_front()
            .ok_or(SourceTransportError::Unavailable)?;
        Ok(SourceResponse {
            status,
            final_url: request.url,
            body: Box::new(Cursor::new(body)),
        })
    }
}
