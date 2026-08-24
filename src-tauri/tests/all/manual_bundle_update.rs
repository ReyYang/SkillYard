use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use skillyard_lib::{
    ApplicationPaths, InstallInputKind, InstallMode, MountScope, PlatformInfo,
    SkillYardApplication, SourceRequest, SourceResponse, SourceTransport, SourceTransportError,
    SupportedAppId, UiIntent, UiOutcome,
};
use tempfile::tempdir;
use url::Url;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

#[test]
fn archive_replacement_updates_the_whole_bundle_without_changing_source_identity_or_mounts() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let original = sandbox.path().join("downloads/original.skill");
    write_archive(
        &original,
        &[
            ("bundle/skills/alpha/SKILL.md", skill_document("alpha")),
            ("bundle/skills/alpha/payload.txt", "alpha-old".to_owned()),
            ("bundle/skills/beta/SKILL.md", skill_document("beta")),
            ("bundle/skills/beta/payload.txt", "beta-old".to_owned()),
        ],
    );
    let installed = install_archive(&application, &original);
    let alpha_member_id = managed_member_id(&installed, "alpha");
    mount_codex_global(&application, &alpha_member_id);
    let before = source_bundle_state(&data_root);

    let replacement = sandbox.path().join("incoming/replacement.zip");
    let replacement_bytes = write_archive(
        &replacement,
        &[
            ("bundle/skills/alpha/SKILL.md", skill_document("alpha")),
            (
                "bundle/skills/alpha/payload.txt",
                "alpha-updated".to_owned(),
            ),
            ("bundle/skills/gamma/SKILL.md", skill_document("gamma")),
            ("bundle/skills/gamma/payload.txt", "gamma-new".to_owned()),
        ],
    );
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateBundleReplacementPlan {
            bundle_id: before.bundle_id.clone(),
            input_path: replacement.to_string_lossy().into_owned(),
        })
        .expect("手动替换应生成 canonical Update Plan")
    else {
        panic!("应返回 InstallPlan");
    };
    assert_eq!(plan.mode, InstallMode::Update);
    assert_eq!(plan.input_kind, InstallInputKind::Archive);
    assert_eq!(
        plan.update_impact
            .as_ref()
            .expect("更新应展示影响")
            .upstream_url,
        None
    );
    assert_eq!(
        candidate_names(&plan),
        vec!["alpha".to_owned(), "gamma".to_owned()]
    );
    assert_eq!(
        source_bundle_state(&data_root),
        before,
        "生成替换 Plan 不能推进 Source 或 current"
    );
    assert_eq!(
        fs::read_to_string(home.join(".codex/skills/alpha/payload.txt"))
            .expect("确认前 Mount 应可读"),
        "alpha-old"
    );

    let updated = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: plan
                .candidates
                .iter()
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("确认后应通过唯一 install_bundle 事务更新");
    let after = source_bundle_state(&data_root);
    assert_eq!(after.source_id, before.source_id);
    assert_eq!(after.kind, before.kind);
    assert_eq!(after.canonical_identity, before.canonical_identity);
    assert_eq!(after.locator, before.locator);
    assert_ne!(after.current_target, before.current_target);
    assert_eq!(after.catalog_marker, sha256(&replacement_bytes));
    assert_eq!(after.adopted_marker, after.catalog_marker);
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
        .expect("替换文件移除的既有成员应保留"),
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
        .expect("重启后应读取替换后的持久状态");
    assert!(inventory_has_unmounted_member(
        &restarted_inventory,
        "gamma"
    ));
    assert_eq!(
        fs::read_to_string(home.join(".codex/skills/alpha/payload.txt"))
            .expect("重启后 Mount 应继续指向新内容"),
        "alpha-updated"
    );
}

#[test]
fn direct_url_replacement_uses_only_the_selected_local_archive() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let initial_bytes = archive_bytes(&[
        ("bundle/alpha/SKILL.md", skill_document("alpha")),
        ("bundle/alpha/payload.txt", "remote-old".to_owned()),
    ]);
    let transport = Arc::new(RecordingArchiveTransport::new(
        "https://downloads.example/bundle.zip",
        initial_bytes,
    ));
    let data_root = sandbox.path().join("data");
    let home = sandbox.path().join("home");
    let application = SkillYardApplication::new_with_source_transport(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
        transport.clone(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应完成首次扫描");
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateUrlInstallPlan {
            url: "https://downloads.example/bundle.zip".to_owned(),
        })
        .expect("直接 URL 应生成安装 Plan")
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
        .expect("应安装 URL Source");
    assert_eq!(transport.request_count(), 1);
    let before = source_bundle_state(&data_root);
    assert_eq!(before.kind, "direct_url");

    let replacement = sandbox.path().join("incoming/direct-replacement.skill");
    write_archive(
        &replacement,
        &[
            ("bundle/alpha/SKILL.md", skill_document("alpha")),
            ("bundle/alpha/payload.txt", "replacement-local".to_owned()),
        ],
    );
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateBundleReplacementPlan {
            bundle_id: before.bundle_id.clone(),
            input_path: replacement.to_string_lossy().into_owned(),
        })
        .expect("Direct URL Source 应接受本地替换文件")
    else {
        panic!("应返回 InstallPlan");
    };
    assert_eq!(transport.request_count(), 1, "替换阶段不能访问原 URL");
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: plan
                .candidates
                .iter()
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("应确认本地替换");
    assert_eq!(transport.request_count(), 1, "确认阶段也不能访问原 URL");
    let after = source_bundle_state(&data_root);
    assert_eq!(after.source_id, before.source_id);
    assert_eq!(after.kind, before.kind);
    assert_eq!(after.canonical_identity, before.canonical_identity);
    assert_eq!(after.locator, before.locator);
    assert_ne!(after.catalog_marker, before.catalog_marker);
}

#[test]
fn invalid_replacement_archive_fails_before_plan_and_preserves_current_state() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _home) = ready_application(sandbox.path());
    let original = sandbox.path().join("downloads/original.zip");
    write_archive(
        &original,
        &[
            ("bundle/alpha/SKILL.md", skill_document("alpha")),
            ("bundle/alpha/payload.txt", "old".to_owned()),
        ],
    );
    install_archive(&application, &original);
    let before = source_bundle_state(&data_root);

    let invalid = sandbox.path().join("incoming/invalid.zip");
    write_archive(
        &invalid,
        &[(
            "bundle/alpha/SKILL.md",
            "---\nname: alpha\n---\n".to_owned(),
        )],
    );
    let error = application
        .handle(UiIntent::CreateBundleReplacementPlan {
            bundle_id: before.bundle_id.clone(),
            input_path: invalid.to_string_lossy().into_owned(),
        })
        .expect_err("无效候选必须在 Plan 阶段拒绝");
    assert!(
        error.to_string().contains("无法生成计划"),
        "错误应说明替换内容无效：{error}"
    );
    assert_eq!(source_bundle_state(&data_root), before);
    let pending_plans = Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开 SQLite")
        .query_row(
            "SELECT COUNT(*) FROM install_plans WHERE status = 'pending'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("应读取 pending Plan 数量");
    assert_eq!(pending_plans, 0, "失败不能留下可确认的替换 Plan");
    let snapshot_leftovers = fs::read_dir(data_root.join("staging"))
        .expect("应读取 staging")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".install-plan-"))
        })
        .count();
    assert_eq!(snapshot_leftovers, 0, "失败的 prepared snapshot 必须清理");
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceBundleState {
    source_id: String,
    kind: String,
    canonical_identity: String,
    locator: String,
    catalog_marker: String,
    adopted_marker: String,
    bundle_id: String,
    current_target: String,
}

fn source_bundle_state(data_root: &Path) -> SourceBundleState {
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(
            "SELECT source.id, source.kind, source.canonical_identity, source.locator,
                    source.catalog_marker, link.adopted_marker, bundle.id,
                    bundle.current_target
             FROM sources AS source
             JOIN source_bundle_links AS link ON link.source_id = source.id
             JOIN bundles AS bundle ON bundle.id = link.bundle_id",
            [],
            |row| {
                Ok(SourceBundleState {
                    source_id: row.get(0)?,
                    kind: row.get(1)?,
                    canonical_identity: row.get(2)?,
                    locator: row.get(3)?,
                    catalog_marker: row.get(4)?,
                    adopted_marker: row.get(5)?,
                    bundle_id: row.get(6)?,
                    current_target: row.get(7)?,
                })
            },
        )
        .expect("应读取 Source 与 Bundle 状态")
}

fn install_archive(application: &SkillYardApplication, archive: &Path) -> UiOutcome {
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateArchiveInstallPlan {
            input_path: archive.to_string_lossy().into_owned(),
        })
        .expect("归档应生成安装 Plan")
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
        .expect("归档应安装成功")
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

fn candidate_names(plan: &skillyard_lib::InstallPlan) -> Vec<String> {
    let mut names = plan
        .candidates
        .iter()
        .filter_map(|candidate| candidate.skill_name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn skill_document(name: &str) -> String {
    format!("---\nname: {name}\ndescription: {name} fixture\n---\n")
}

fn write_archive(path: &Path, entries: &[(&str, String)]) -> Vec<u8> {
    let bytes = archive_bytes(entries);
    fs::create_dir_all(path.parent().expect("归档应有父目录")).expect("应创建归档父目录");
    fs::write(path, &bytes).expect("应写入归档");
    bytes
}

fn archive_bytes(entries: &[(&str, String)]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o100644);
    for (path, contents) in entries {
        writer.start_file(path, options).expect("应开始归档文件");
        writer
            .write_all(contents.as_bytes())
            .expect("应写入归档文件");
    }
    writer.finish().expect("应完成归档").into_inner()
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("写入 String 不应失败");
    }
    encoded
}

struct RecordingArchiveTransport {
    final_url: Url,
    body: Vec<u8>,
    requests: Mutex<Vec<String>>,
}

impl RecordingArchiveTransport {
    fn new(final_url: &str, body: Vec<u8>) -> Self {
        Self {
            final_url: Url::parse(final_url).expect("fixture URL 应合法"),
            body,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn request_count(&self) -> usize {
        self.requests.lock().expect("请求锁不应损坏").len()
    }
}

impl SourceTransport for RecordingArchiveTransport {
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
