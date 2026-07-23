use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, InstallPlan, ManagementKind, MountScope, PlatformInfo, SkillYardApplication,
    SourceAssociationMode, SourceMemberMappingChoice, SourceRequest, SourceResponse,
    SourceTransport, SourceTransportError, SupportedAppId, UiIntent, UiOutcome,
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
fn direct_association_succeeds_when_notice_projection_cannot_be_written() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let local = install_folder_bundle(
        &application,
        sandbox.path(),
        "notice-failure-local",
        &[("alpha", "local-alpha")],
    );
    let source_id = create_idle_archive_source(
        &application,
        &data_root,
        &sandbox.path().join("downloads/notice-failure.skill"),
        &[("alpha", "upstream-alpha")],
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
    let plan_id = plan.id.clone();
    let notice_path = data_root.join("SKILLYARD-INFO.md");
    let observer =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开观察用 SQLite");
    thread::scope(|scope| {
        let confirmation = scope.spawn(|| {
            application.handle(UiIntent::ConfirmSourceAssociationPlan {
                plan_id,
                content_choices: Vec::new(),
            })
        });
        loop {
            let consumed = observer
                .query_row(
                    "SELECT status = 'consumed' FROM source_association_plans LIMIT 1",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .expect("应观察 Plan 状态");
            if consumed {
                break;
            }
            thread::yield_now();
        }
        // 只在 SQLite 提交已经对另一连接可见后破坏目标，精确覆盖提交后的 projection 写入。
        fs::remove_file(&notice_path).expect("应移除已有说明文件");
        fs::create_dir(&notice_path).expect("应模拟说明文件目标无法写入");
        confirmation
            .join()
            .expect("确认线程不应 panic")
            .expect("说明投影失败不能否定已提交的直接关联");
    });
    assert!(notice_path.is_dir(), "故障目录应证明 notice 写入确实失败");
    assert_eq!(source_link_count(&data_root), 1);
    assert_eq!(
        scalar_count(
            &data_root,
            "SELECT COUNT(*) FROM source_association_plans WHERE status = 'consumed'"
        ),
        1,
        "SQLite 提交必须同时消费 Plan"
    );

    // 启动恢复会从 SQLite 重建可丢失的说明投影。
    fs::remove_dir(&notice_path).expect("应清除故障目录");
    drop(application);
    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应恢复说明投影");
    assert!(
        fs::read_to_string(notice_path)
            .expect("说明投影应被重建")
            .contains("notice-failure.skill")
    );
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
fn an_already_linked_source_returns_merge_mode_without_creating_a_second_relation() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
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
                .filter(|candidate| candidate.default_selected)
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
    assert_eq!(source_link_count(&data_root), 1);
    assert_eq!(bundle_count(&data_root), 2);

    let error = application
        .handle(UiIntent::ConfirmSourceAssociationPlan {
            plan_id: plan.id,
            content_choices: Vec::new(),
        })
        .expect_err("直接关联切片不能绕过同一 Plan 的归并执行器");
    assert!(error.to_string().contains("归并执行器"));
    assert_eq!(source_link_count(&data_root), 1);
    assert_eq!(bundle_count(&data_root), 2);
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
