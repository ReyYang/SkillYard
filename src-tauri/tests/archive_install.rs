use std::{
    fs,
    io::{Cursor, Write},
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, BundleUpdateStatus, InstallInputKind, ManagementKind, MountScope,
    PlatformInfo, SkillYardApplication, SourceKind, SourceRequest, SourceResponse, SourceTransport,
    SourceTransportError, SupportedAppId, UiIntent, UiOutcome,
};
use tempfile::tempdir;
use url::Url;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

#[test]
fn local_skill_archive_installs_through_the_canonical_unmounted_bundle_flow() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let archive = sandbox.path().join("downloads/superpowers.skill");
    write_archive(
        &archive,
        &[
            (
                "superpowers/skills/brainstorming/SKILL.md",
                skill_document("brainstorming"),
            ),
            (
                "superpowers/skills/brainstorming/payload.txt",
                "first".to_owned(),
            ),
            ("superpowers/skills/tdd/SKILL.md", skill_document("tdd")),
            ("superpowers/skills/tdd/payload.txt", "second".to_owned()),
        ],
    );
    let original_archive = fs::read(&archive).expect("应读取安装前 archive");

    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateArchiveInstallPlan {
            input_path: archive.to_string_lossy().into_owned(),
        })
        .expect(".skill 应作为 ZIP 容器生成唯一安装 Plan")
    else {
        panic!("应返回安装 Plan");
    };
    assert_eq!(plan.input_kind, InstallInputKind::Archive);
    assert_eq!(plan.bundle_display_name, "superpowers");
    assert!(
        plan.candidates
            .iter()
            .all(|candidate| candidate.default_selected)
    );
    assert!(!plan.will_mount);
    let before_confirmation =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    assert_eq!(
        before_confirmation
            .query_row("SELECT COUNT(*) FROM bundles", [], |row| row
                .get::<_, i64>(0))
            .expect("确认前应能读取 Bundle 数量"),
        0,
        "生成 Plan 不能提前创建本地 Bundle"
    );

    let selected_candidate_ids = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.default_selected)
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids,
        })
        .expect("本地 archive 应复用正式安装事务")
    else {
        panic!("安装后应返回 Inventory");
    };
    let mut managed = entries
        .iter()
        .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
        .collect::<Vec<_>>();
    managed.sort_by(|left, right| left.skill_name.cmp(&right.skill_name));
    assert_eq!(
        managed
            .iter()
            .map(|entry| entry.skill_name.as_str())
            .collect::<Vec<_>>(),
        ["brainstorming", "tdd"]
    );
    assert!(managed.iter().all(|entry| {
        entry.bundle_display_name.as_deref() == Some("superpowers")
            && entry.source_display_name.as_deref() == Some("superpowers")
    }));
    assert!(mounts.is_empty());
    assert!(!home.join(".codex/skills/brainstorming").exists());
    assert!(!home.join(".claude/skills/tdd").exists());
    assert_eq!(
        fs::read(&archive).expect("原归档应保持可读"),
        original_archive,
        "原归档不应被移动或改写"
    );

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let state = connection
        .query_row(
            "SELECT source.kind, source.locator,
                    (SELECT COUNT(*) FROM source_bundle_links WHERE source_id = source.id),
                    (SELECT COUNT(*) FROM mounts)
             FROM sources source
             WHERE source.kind = 'archive'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .expect("archive 应成为可手动替换的 Source");
    assert_eq!(state.0, "archive");
    assert_eq!(
        Path::new(&state.1),
        fs::canonicalize(&archive)
            .expect("应取得 canonical archive")
            .as_path()
    );
    assert_eq!((state.2, state.3), (1, 0));

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        supported_platform(),
    );
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应从持久状态读取 Source-backed Bundle")
    else {
        panic!("重启后应返回 Inventory");
    };
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.management_kind == ManagementKind::SkillYardManaged)
            .count(),
        2
    );
    assert!(mounts.is_empty(), "直接安装在重启后仍不能自动挂载");
}

#[test]
fn direct_archive_url_uses_the_same_plan_and_source_lifecycle() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let body = archive_bytes(&[
        ("bundle/alpha/SKILL.md", skill_document("alpha")),
        ("bundle/alpha/payload.txt", "remote".to_owned()),
    ]);
    let transport = Arc::new(ArchiveTransport::new(
        "https://downloads.example/alpha.zip",
        body,
    ));
    let data_root = sandbox.path().join("data");
    let home = sandbox.path().join("home");
    let application = SkillYardApplication::new_with_source_transport(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        supported_platform(),
        transport.clone(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应先完成首次扫描");

    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateUrlInstallPlan {
            url: "https://downloads.example/alpha.zip".to_owned(),
        })
        .expect("确定性 ZIP URL 应生成安装 Plan")
    else {
        panic!("应返回安装 Plan");
    };
    assert_eq!(plan.input_kind, InstallInputKind::DirectUrl);
    let selected_candidate_ids = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.default_selected)
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids,
        })
        .expect("直接 URL 应复用正式安装事务");

    assert_eq!(
        transport.requests(),
        vec!["https://downloads.example/alpha.zip"]
    );
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let source = connection
        .query_row(
            "SELECT kind, locator,
                    (SELECT COUNT(*) FROM source_bundle_links WHERE source_id = sources.id)
             FROM sources WHERE kind = 'direct_url'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .expect("直接 URL 应保存为手动更新 Source");
    assert_eq!(
        source,
        (
            "direct_url".to_owned(),
            "https://downloads.example/alpha.zip".to_owned(),
            1
        )
    );

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        supported_platform(),
    );
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应保留直接 URL 安装结果")
    else {
        panic!("重启后应返回 Inventory");
    };
    assert!(entries.iter().any(|entry| {
        entry.skill_name == "alpha" && entry.management_kind == ManagementKind::SkillYardManaged
    }));
    assert!(mounts.is_empty(), "直接 URL 安装在重启后仍不能自动挂载");
}

#[test]
fn editable_local_source_keeps_the_author_directory_outside_current_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let editable = sandbox.path().join("author/my-skills");
    write_skill(&editable.join("skills/alpha"), "alpha", "author-original");

    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateEditableLocalInstallPlan {
            input_path: editable.to_string_lossy().into_owned(),
        })
        .expect("显式 Editable Local Source 应生成安装 Plan")
    else {
        panic!("应返回安装 Plan");
    };
    assert_eq!(plan.input_kind, InstallInputKind::EditableLocal);
    let selected_candidate_ids = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.default_selected)
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids,
        })
        .expect("Editable 首次采用应复制进 Central Store")
    else {
        panic!("安装后应返回 Inventory");
    };
    let managed = entries
        .iter()
        .find(|entry| entry.skill_name == "alpha")
        .expect("应找到受管 alpha");
    let managed_payload = Path::new(&managed.skill_root).join("payload.txt");
    assert_eq!(
        fs::read_to_string(&managed_payload).expect("应读取受管副本"),
        "author-original"
    );
    fs::write(editable.join("skills/alpha/payload.txt"), "author-edited")
        .expect("应能继续编辑原目录");
    assert_eq!(
        fs::read_to_string(&managed_payload).expect("正在使用的副本应继续可读"),
        "author-original",
        "原目录后续变化不能绕过确认直接传播"
    );
    assert!(mounts.is_empty());
    assert!(!home.join(".codex/skills/alpha").exists());

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let source = connection
        .query_row(
            "SELECT kind, locator FROM sources WHERE kind = 'editable_local'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("应保存 Editable Local Source");
    assert_eq!(source.0, "editable_local");
    assert_eq!(
        Path::new(&source.1),
        fs::canonicalize(&editable)
            .expect("应取得 canonical editable path")
            .as_path()
    );

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root, home),
        supported_platform(),
    );
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应保留 Editable Local 安装结果")
    else {
        panic!("重启后应返回 Inventory");
    };
    assert!(entries.iter().any(|entry| {
        entry.skill_name == "alpha" && entry.management_kind == ManagementKind::SkillYardManaged
    }));
    assert!(
        mounts.is_empty(),
        "Editable Local 安装在重启后仍不能自动挂载"
    );
}

#[test]
fn editable_local_source_rename_is_explicitly_relinked_without_adopting_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let editable = sandbox.path().join("author/original-skills");
    write_skill(&editable.join("alpha"), "alpha", "author-original");
    let original_locator = fs::canonicalize(&editable).expect("应取得原 Source 路径");

    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateEditableLocalInstallPlan {
            input_path: editable.to_string_lossy().into_owned(),
        })
        .expect("首次选择应登记 Editable Local Source")
    else {
        panic!("应返回安装 Plan");
    };
    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: plan
                .candidates
                .iter()
                .filter(|candidate| candidate.default_selected)
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("应安装 Editable Local Bundle")
    else {
        panic!("安装后应返回 Inventory");
    };
    let alpha_member_id = entries
        .iter()
        .find(|entry| entry.skill_name == "alpha")
        .and_then(|entry| entry.member_id.clone())
        .expect("应找到受管 alpha Member");
    fs::create_dir_all(&home).expect("应创建隔离 home");
    let UiOutcome::MountPlan { plan } = application
        .handle(UiIntent::CreateMountPlan {
            member_id: alpha_member_id,
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect("应生成 Codex Mount Plan")
    else {
        panic!("应返回 Mount Plan");
    };
    application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect("应挂载 alpha");
    let UiOutcome::SourceDiscovery { sources, .. } = application
        .handle(UiIntent::OpenSourceDiscovery)
        .expect("应读取 Source")
    else {
        panic!("应返回 SourceDiscovery");
    };
    let source = sources
        .into_iter()
        .find(|source| source.kind == SourceKind::EditableLocal)
        .expect("应找到 Editable Local Source");
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let before = connection
        .query_row(
            "SELECT source.catalog_generation, source.catalog_marker,
                    bundle.current_target, link.update_check_status
             FROM sources AS source
             JOIN source_bundle_links AS link ON link.source_id = source.id
             JOIN bundles AS bundle ON bundle.id = link.bundle_id
             WHERE source.id = ?1",
            [&source.id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .expect("应读取重新关联前状态");

    let moved = sandbox.path().join("author/moved-skills");
    fs::rename(&editable, &moved).expect("应在同一文件系统内移动 Source 目录");
    fs::write(moved.join("alpha/payload.txt"), "author-after-move")
        .expect("应模拟移动后继续编辑作者目录");
    let UiOutcome::EditableLocalRelinkPlan { plan } = application
        .handle(UiIntent::CreateEditableLocalRelinkPlan {
            source_id: source.id.clone(),
            candidate_path: moved.to_string_lossy().into_owned(),
        })
        .expect("同一 inode 的新路径应生成明确确认 Plan")
    else {
        panic!("应返回 EditableLocalRelinkPlan");
    };
    assert_eq!(plan.source_id, source.id);
    assert_eq!(Path::new(&plan.current_path), original_locator);
    assert_eq!(
        Path::new(&plan.candidate_path),
        fs::canonicalize(&moved).expect("应取得候选 canonical path")
    );
    assert_eq!(plan.bundle_display_name.as_deref(), Some("original-skills"));
    assert_eq!(plan.members.len(), 1);
    assert_eq!(plan.members[0].skill_name.as_deref(), Some("alpha"));

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        supported_platform(),
    );
    let UiOutcome::EditableLocalRelinkPlan {
        plan: restored_plan,
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应恢复未确认的重新关联 Plan")
    else {
        panic!("重启后应继续返回 EditableLocalRelinkPlan");
    };
    assert_eq!(restored_plan, plan);
    let UiOutcome::SourceDiscovery {
        sources,
        highlighted_source_id,
        ..
    } = restarted
        .handle(UiIntent::ConfirmEditableLocalRelinkPlan { plan_id: plan.id })
        .expect("确认后应只更新 Editable Local Source 路径")
    else {
        panic!("确认后应返回 SourceDiscovery");
    };
    let relinked = sources
        .iter()
        .find(|candidate| candidate.id == source.id)
        .expect("应返回重新关联后的 Source");
    assert_eq!(highlighted_source_id.as_deref(), Some(source.id.as_str()));
    assert_eq!(
        Path::new(&relinked.locator),
        fs::canonicalize(&moved).expect("应取得新路径").as_path()
    );

    let state = connection
        .query_row(
            "SELECT source.locator, source.catalog_generation, source.catalog_marker,
                    bundle.current_target, link.update_check_status,
                    (SELECT COUNT(*) FROM sources WHERE kind = 'editable_local')
             FROM sources AS source
             JOIN source_bundle_links AS link ON link.source_id = source.id
             JOIN bundles AS bundle ON bundle.id = link.bundle_id
             WHERE source.id = ?1",
            [&source.id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .expect("应读取重新关联后的状态");
    assert_eq!(Path::new(&state.0), fs::canonicalize(&moved).unwrap());
    assert_eq!(state.1, before.0, "重新关联不能刷新 Source Catalog");
    assert_eq!(state.2, before.1, "重新关联不能采用候选内容");
    assert_eq!(state.3, before.2, "重新关联不能切换 current");
    assert_eq!(state.4, "not_checked");
    assert_eq!(state.5, 1, "同一 Source 不能因路径变化被重复创建");
    assert_eq!(
        fs::read_to_string(home.join(".codex/skills/alpha/payload.txt"))
            .expect("既有 Mount 应继续指向受管内容"),
        "author-original",
        "重新关联不能把作者目录的新内容直接传播到 Host"
    );

    let UiOutcome::Inventory { bundle_updates, .. } = restarted
        .handle(UiIntent::CheckEditableLocalBundle {
            bundle_id: source.bundle_id.expect("已安装 Source 应关联 Bundle"),
        })
        .expect("重新关联后应能单独检查 Editable Local 更新")
    else {
        panic!("检查后应返回 Inventory");
    };
    assert!(
        bundle_updates
            .iter()
            .any(|update| update.status == BundleUpdateStatus::Available)
    );
}

#[test]
fn editable_local_relink_rejects_a_similar_but_different_directory() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
    let original = sandbox.path().join("author/original-skills");
    let replacement = sandbox.path().join("author/copied-skills");
    write_skill(&original.join("alpha"), "alpha", "same-content");
    write_skill(&replacement.join("alpha"), "alpha", "same-content");

    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateEditableLocalInstallPlan {
            input_path: original.to_string_lossy().into_owned(),
        })
        .expect("应登记 Editable Local Source")
    else {
        panic!("应返回安装 Plan");
    };
    application
        .handle(UiIntent::DiscardInstallPlan { plan_id: plan.id })
        .expect("应放弃安装但保留 Source");
    let UiOutcome::SourceDiscovery { sources, .. } = application
        .handle(UiIntent::OpenSourceDiscovery)
        .expect("应读取 Source")
    else {
        panic!("应返回 SourceDiscovery");
    };
    let source = sources
        .into_iter()
        .find(|source| source.kind == SourceKind::EditableLocal)
        .expect("应找到 Editable Local Source");

    let error = application
        .handle(UiIntent::CreateEditableLocalRelinkPlan {
            source_id: source.id,
            candidate_path: replacement.to_string_lossy().into_owned(),
        })
        .expect_err("名称和内容相同但 inode 不同的目录不能被重新关联");
    assert!(error.to_string().contains("已不是登记时的目录"));

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let state = connection
        .query_row(
            "SELECT source.locator,
                    (SELECT COUNT(*) FROM editable_local_relink_plans)
             FROM sources AS source
             WHERE source.kind = 'editable_local'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .expect("拒绝后应保留原 Source");
    assert_eq!(
        Path::new(&state.0),
        fs::canonicalize(&original).expect("应取得原路径").as_path()
    );
    assert_eq!(state.1, 0, "拒绝候选不能留下确认 Plan");
}

#[test]
fn unsafe_archive_fails_before_source_or_managed_content_is_created() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
    let archive = sandbox.path().join("downloads/unsafe.zip");
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "bundle/alpha",
            SimpleFileOptions::default().unix_permissions(0o120777),
        )
        .expect("应写入 symlink fixture");
    writer.write_all(b"outside").expect("应写入 fixture");
    fs::create_dir_all(archive.parent().expect("archive 应有父目录")).expect("应创建下载目录");
    let mut bytes = writer.finish().expect("应完成 archive").into_inner();
    let central = bytes
        .windows(4)
        .position(|window| window == [0x50, 0x4b, 0x01, 0x02])
        .expect("fixture 应包含 central entry");
    // zip writer 会规范化类型位；直接标记 central directory 才能构造真实 symlink entry。
    bytes[central + 38..central + 42].copy_from_slice(&((0o120777_u32) << 16).to_le_bytes());
    fs::write(&archive, bytes).expect("应保存 archive");

    let error = application
        .handle(UiIntent::CreateArchiveInstallPlan {
            input_path: archive.to_string_lossy().into_owned(),
        })
        .expect_err("带 symlink 的 archive 必须在写入 Current Content 前失败");
    assert!(
        error.to_string().contains("特殊"),
        "应明确说明特殊文件风险，实际错误：{error}"
    );

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM sources WHERE kind <> 'github'),
                (SELECT COUNT(*) FROM install_plans),
                (SELECT COUNT(*) FROM bundles)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .expect("应读取失败后的状态");
    assert_eq!(counts, (0, 0, 0));
}

#[test]
fn repeated_archive_identity_reuses_one_source_and_supersedes_the_old_plan() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
    let archive = sandbox.path().join("downloads/reusable.zip");
    write_archive(
        &archive,
        &[("bundle/alpha/SKILL.md", skill_document("alpha"))],
    );

    let UiOutcome::InstallPlan { plan: first } = application
        .handle(UiIntent::CreateArchiveInstallPlan {
            input_path: archive.to_string_lossy().into_owned(),
        })
        .expect("首次归档应生成 Plan")
    else {
        panic!("应返回安装 Plan");
    };
    let first_snapshot = data_root
        .join("staging")
        .join(format!(".install-plan-{}", first.id));
    assert!(first_snapshot.is_dir());

    let UiOutcome::InstallPlan { plan: second } = application
        .handle(UiIntent::CreateArchiveInstallPlan {
            input_path: archive.to_string_lossy().into_owned(),
        })
        .expect("同一归档再次进入时应替换未确认 Plan")
    else {
        panic!("应返回新的安装 Plan");
    };
    assert_ne!(first.id, second.id);
    assert!(!first_snapshot.exists(), "被替换 Plan 的快照必须清理");

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM sources WHERE kind = 'archive'),
                (SELECT COUNT(*) FROM install_plans WHERE status = 'pending')",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .expect("应读取 Source 和 Plan 数量");
    assert_eq!(
        counts,
        (1, 1),
        "canonical archive identity 只能对应一个 Source"
    );
}

#[test]
fn discarding_archive_plan_removes_snapshot_but_keeps_the_source() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _) = ready_application(sandbox.path());
    let archive = sandbox.path().join("downloads/discardable.zip");
    write_archive(
        &archive,
        &[("bundle/alpha/SKILL.md", skill_document("alpha"))],
    );
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateArchiveInstallPlan {
            input_path: archive.to_string_lossy().into_owned(),
        })
        .expect("归档应生成 Plan")
    else {
        panic!("应返回安装 Plan");
    };
    let snapshot = data_root
        .join("staging")
        .join(format!(".install-plan-{}", plan.id));
    assert!(snapshot.is_dir());
    let notice_path = data_root.join("SKILLYARD-INFO.md");
    let notice = fs::read_to_string(&notice_path).expect("生成确认页时应同步 Source 说明");
    assert!(notice.contains("discardable"));
    assert!(notice.contains(&archive.to_string_lossy().into_owned()));

    assert_eq!(
        application
            .handle(UiIntent::DiscardInstallPlan { plan_id: plan.id })
            .expect("放弃 Plan 应成功"),
        UiOutcome::InstallPlanDiscarded
    );
    assert!(!snapshot.exists(), "放弃 Plan 必须删除临时快照");
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM sources WHERE kind = 'archive'),
                (SELECT COUNT(*) FROM install_plans)",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .expect("应读取放弃后的 Source 和 Plan");
    assert_eq!(
        counts,
        (1, 0),
        "Source 是用户保存的来源，不能随确认页一起删除"
    );
    let notice = fs::read_to_string(notice_path).expect("放弃确认页后 Source 说明应继续存在");
    assert!(notice.contains("discardable"));
}

#[test]
fn interrupted_archive_confirmation_recovers_through_the_existing_journal() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let data_root = sandbox.path().join("data");
    let home = sandbox.path().join("home");
    let paths = ApplicationPaths::for_home(data_root.clone(), home.clone());
    let application = SkillYardApplication::new_with_lifecycle_failpoint(
        paths,
        supported_platform(),
        skillyard_lib::LifecycleFailpoint::AfterCurrentActivated,
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应先完成首次扫描");
    let archive = sandbox.path().join("downloads/recoverable.zip");
    write_archive(
        &archive,
        &[("bundle/alpha/SKILL.md", skill_document("alpha"))],
    );
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateArchiveInstallPlan {
            input_path: archive.to_string_lossy().into_owned(),
        })
        .expect("归档应生成 Plan")
    else {
        panic!("应返回安装 Plan");
    };
    let selected_candidate_ids = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.default_selected)
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    let error = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids,
        })
        .expect_err("failpoint 应模拟 current 生效后的中断");
    assert!(error.to_string().contains("current 已生效"));

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        supported_platform(),
    );
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应使用同一 Journal 自动完成 Source 安装")
    else {
        panic!("重启后应返回 Inventory");
    };
    assert!(
        entries.iter().any(|entry| {
            entry.management_kind == ManagementKind::SkillYardManaged && entry.skill_name == "alpha"
        }),
        "恢复后应保留已经原子切换的新 Bundle"
    );
    assert!(mounts.is_empty());
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let state = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM lifecycle_transactions),
                (SELECT COUNT(*) FROM source_bundle_links)",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .expect("应读取恢复后的事务和 Source link");
    assert_eq!(state, (0, 1), "恢复后必须完成记录并清理终态事务");
}

fn ready_application(
    base: &Path,
) -> (SkillYardApplication, std::path::PathBuf, std::path::PathBuf) {
    let data_root = base.join("data");
    let home = base.join("home");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        supported_platform(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应先完成首次扫描");
    (application, data_root, home)
}

fn supported_platform() -> PlatformInfo {
    PlatformInfo {
        os: "macos".to_owned(),
        architecture: "aarch64".to_owned(),
        major_version: 14,
    }
}

fn skill_document(name: &str) -> String {
    format!("---\nname: {name}\ndescription: {name} description\n---\n")
}

fn write_skill(root: &Path, name: &str, payload: &str) {
    fs::create_dir_all(root).expect("应创建 Skill 目录");
    fs::write(root.join("SKILL.md"), skill_document(name)).expect("应写入 SKILL.md");
    fs::write(root.join("payload.txt"), payload).expect("应写入 payload");
}

fn write_archive(path: &Path, entries: &[(&str, String)]) {
    let bytes = archive_bytes(entries);
    fs::create_dir_all(path.parent().expect("archive 应有父目录")).expect("应创建下载目录");
    fs::write(path, bytes).expect("应保存 archive");
}

fn archive_bytes(entries: &[(&str, String)]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o100644);
    for (path, contents) in entries {
        writer
            .start_file(path, options)
            .expect("应开始 archive 文件");
        writer
            .write_all(contents.as_bytes())
            .expect("应写入 archive 文件");
    }
    writer.finish().expect("应完成 archive").into_inner()
}

struct ArchiveTransport {
    final_url: Url,
    body: Vec<u8>,
    requests: Mutex<Vec<String>>,
}

impl ArchiveTransport {
    fn new(final_url: &str, body: Vec<u8>) -> Self {
        Self {
            final_url: Url::parse(final_url).expect("fixture URL 应合法"),
            body,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("请求锁不应损坏").clone()
    }
}

impl SourceTransport for ArchiveTransport {
    fn get(&self, request: SourceRequest) -> Result<SourceResponse, SourceTransportError> {
        self.requests
            .lock()
            .expect("请求锁不应损坏")
            .push(request.url.as_str().to_owned());
        Ok(SourceResponse {
            status: 200,
            final_url: self.final_url.clone(),
            body: Box::new(Cursor::new(self.body.clone())),
        })
    }
}
