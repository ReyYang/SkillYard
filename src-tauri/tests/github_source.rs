use std::{
    collections::VecDeque,
    fs,
    io::{Cursor, Write},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, PlatformInfo, SkillYardApplication, SourceCatalogStatus, SourceRequest,
    SourceResponse, SourceTransport, SourceTransportError, UiIntent, UiOutcome,
};
use tempfile::tempdir;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

#[test]
fn recommended_github_sources_exist_without_creating_bundles_and_survive_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let transport = Arc::new(RecordingTransport::default());
    let application = SkillYardApplication::new_with_source_transport(
        paths.clone(),
        PlatformInfo::supported_for_test(),
        transport.clone(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    assert_eq!(
        transport.request_count(),
        0,
        "启动和首次扫描不能访问 GitHub"
    );

    let first = open_source_discovery(&application);
    assert_eq!(
        transport.request_count(),
        4,
        "首次打开应逐个尝试加载当前四个 Source"
    );
    assert_eq!(
        first
            .iter()
            .map(|source| (source.display_name.as_str(), source.tracked_ref.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("anthropics/skills", "main"),
            ("ComposioHQ/awesome-claude-skills", "master"),
            ("cexll/myclaude", "master"),
            ("JimLiu/baoyu-skills", "main"),
        ]
    );
    assert!(first.iter().all(|source| {
        source.catalog_status == SourceCatalogStatus::Unloaded
            && source.bundle_id.is_none()
            && source.members.is_empty()
    }));
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let counts = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM sources), (SELECT COUNT(*) FROM bundles)",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .expect("应读取 Source 与 Bundle 数量");
    assert_eq!(counts, (4, 0));
    drop(connection);

    drop(application);
    let restarted_transport = Arc::new(RecordingTransport::default());
    let restarted = SkillYardApplication::new_with_source_transport(
        paths,
        PlatformInfo::supported_for_test(),
        restarted_transport.clone(),
    );
    let restarted_sources = open_source_discovery(&restarted);
    assert_eq!(restarted_transport.request_count(), 4);
    assert_eq!(
        restarted_sources
            .iter()
            .map(|source| (
                source.id.as_str(),
                source.display_name.as_str(),
                source.tracked_ref.as_str(),
            ))
            .collect::<Vec<_>>(),
        first
            .iter()
            .map(|source| (
                source.id.as_str(),
                source.display_name.as_str(),
                source.tracked_ref.as_str(),
            ))
            .collect::<Vec<_>>(),
        "重启后的网络失败不能改变已登记 Source 的稳定身份"
    );
}

#[test]
fn deleting_a_recommended_source_does_not_seed_it_again_on_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let application = SkillYardApplication::new_with_source_transport(
        paths.clone(),
        PlatformInfo::supported_for_test(),
        Arc::new(RecordingTransport::default()),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    drop(application);

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    connection
        .execute(
            "DELETE FROM sources WHERE id = 'source-anthropics-skills'",
            [],
        )
        .expect("应模拟后续 Source 删除功能的领域结果");
    drop(connection);

    let restarted_transport = Arc::new(RecordingTransport::default());
    let restarted = SkillYardApplication::new_with_source_transport(
        paths,
        PlatformInfo::supported_for_test(),
        restarted_transport.clone(),
    );
    let sources = open_source_discovery(&restarted);
    assert_eq!(restarted_transport.request_count(), 3);
    assert_eq!(sources.len(), 3);
    assert!(
        sources
            .iter()
            .all(|source| source.id != "source-anthropics-skills")
    );
}

#[test]
fn common_inputs_reuse_one_canonical_source_and_default_branch_comes_from_github() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let application = ready_application_with_transport(sandbox.path(), transport.clone());

    application
        .handle(UiIntent::GetStartupState)
        .expect("启动读取应成功");
    application
        .handle(UiIntent::RefreshLocalInventory)
        .expect("本机刷新应成功");
    assert_eq!(
        transport.request_count(),
        0,
        "启动和本机刷新不能访问 GitHub"
    );

    transport.enqueue_public_repository("Acme/Toolkit", "trunk");
    transport.enqueue_commit("1111111111111111111111111111111111111111");
    let first = add_source(&application, "  acme/toolkit\n", None);
    let added = first
        .iter()
        .find(|source| source.canonical_identity == "github:acme/toolkit")
        .expect("应保存 canonical Source");
    let source_id = added.id.clone();
    assert_eq!(added.display_name, "Acme/Toolkit");
    assert_eq!(added.tracked_ref, "trunk");
    assert!(added.bundle_id.is_none());
    let notice = fs::read_to_string(
        sandbox
            .path()
            .join("application-support/SkillYard/SKILLYARD-INFO.md"),
    )
    .expect("新增 Source 后应同步更新 Central Store 说明");
    assert!(notice.contains("Acme/Toolkit"));
    assert!(notice.contains("https://github.com/Acme/Toolkit"));

    transport.enqueue_public_repository("Acme/Toolkit", "next-default");
    transport.enqueue_commit("1111111111111111111111111111111111111111");
    let repeated = add_source(&application, "acme/toolkit", None);
    assert_eq!(
        repeated
            .iter()
            .find(|source| source.id == source_id)
            .expect("无 ref 的重复入口应复用 Source")
            .tracked_ref,
        "trunk",
        "仓库 default branch 变化不能建议漂移已有 Tracked Ref"
    );

    transport.enqueue_public_repository("Acme/Toolkit", "trunk");
    transport.enqueue_commit("1111111111111111111111111111111111111111");
    let root_url = add_source(
        &application,
        "https://github.com/ACME/Toolkit.git/",
        Some(" trunk "),
    );
    assert_eq!(
        root_url
            .iter()
            .filter(|source| source.canonical_identity == "github:acme/toolkit")
            .count(),
        1
    );
    assert_eq!(
        root_url
            .iter()
            .find(|source| source.canonical_identity == "github:acme/toolkit")
            .expect("应复用 Source")
            .id,
        source_id
    );

    transport.enqueue_public_repository("Acme/Toolkit", "trunk");
    transport.enqueue_commit("1111111111111111111111111111111111111111");
    let outcome = application
        .handle(UiIntent::AddGitHubSource {
            input: "https://github.com/acme/toolkit/tree/trunk/skills/my%20skill".to_owned(),
            tracked_ref: None,
        })
        .expect("成员 URL 应复用完整仓库 Source");
    let UiOutcome::SourceDiscovery {
        sources,
        highlighted_source_id,
        highlighted_member_path,
    } = outcome
    else {
        panic!("应返回 Source 发现状态");
    };
    assert_eq!(highlighted_source_id.as_deref(), Some(source_id.as_str()));
    assert_eq!(highlighted_member_path.as_deref(), Some("skills/my skill"));
    assert_eq!(
        sources
            .iter()
            .filter(|source| source.canonical_identity == "github:acme/toolkit")
            .count(),
        1
    );

    transport.enqueue_public_repository("Acme/Toolkit", "trunk");
    transport.enqueue_commit("1111111111111111111111111111111111111111");
    let outcome = application
        .handle(UiIntent::AddGitHubSource {
            input: "https://github.com/acme/toolkit/blob/trunk/skills/demo/SKILL.md".to_owned(),
            tracked_ref: None,
        })
        .expect("SKILL.md URL 应复用完整仓库 Source");
    let UiOutcome::SourceDiscovery {
        sources,
        highlighted_source_id,
        highlighted_member_path,
    } = outcome
    else {
        panic!("应返回 Source 发现状态");
    };
    assert_eq!(highlighted_source_id.as_deref(), Some(source_id.as_str()));
    assert_eq!(highlighted_member_path.as_deref(), Some("skills/demo"));
    assert_eq!(
        sources
            .iter()
            .filter(|source| source.canonical_identity == "github:acme/toolkit")
            .count(),
        1
    );

    let requests = transport.requests();
    assert_eq!(requests.len(), 10);
    assert!(requests[0].url.path().ends_with("/repos/acme/toolkit"));
    assert!(requests[1].url.path().ends_with("/commits/trunk"));
    assert!(requests[3].url.path().ends_with("/commits/trunk"));
}

#[test]
fn invalid_or_private_repository_does_not_create_a_source() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let application = ready_application_with_transport(sandbox.path(), transport.clone());

    transport.enqueue_public_repository("Acme/Missing", "main");
    transport.enqueue(404, "{}");
    let invalid = application
        .handle(UiIntent::AddGitHubSource {
            input: "acme/missing".to_owned(),
            tracked_ref: Some("does-not-exist".to_owned()),
        })
        .expect_err("无效 ref 不能登记 Source");
    assert!(invalid.to_string().contains("HTTP 状态"));

    transport.enqueue(
        200,
        r#"{"full_name":"Acme/Private","default_branch":"main","private":true}"#,
    );
    let private = application
        .handle(UiIntent::AddGitHubSource {
            input: "acme/private".to_owned(),
            tracked_ref: None,
        })
        .expect_err("私有仓库不能进入 1.0 Source");
    assert!(private.to_string().contains("不是公开仓库"));

    assert_eq!(open_source_discovery(&application).len(), 4);
}

#[test]
fn a_different_tracked_ref_requires_confirmation_before_source_changes() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let application = ready_application_with_transport(sandbox.path(), transport.clone());

    transport.enqueue_public_repository("anthropics/skills", "main");
    transport.enqueue(404, "{}");
    let invalid = application
        .handle(UiIntent::AddGitHubSource {
            input: "https://github.com/anthropics/skills/tree/missing/skills/example".to_owned(),
            tracked_ref: None,
        })
        .expect_err("无法访问的候选 ref 不能生成确认 Plan");
    assert!(invalid.to_string().contains("HTTP 状态"));
    let unchanged = open_source_discovery(&application);
    assert_eq!(
        unchanged
            .iter()
            .find(|source| source.canonical_identity == "github:anthropics/skills")
            .expect("原 Source 应继续存在")
            .tracked_ref,
        "main"
    );

    transport.enqueue_public_repository("anthropics/skills", "main");
    transport.enqueue_commit("2222222222222222222222222222222222222222");

    let outcome = application
        .handle(UiIntent::AddGitHubSource {
            input: "https://github.com/anthropics/skills/tree/next/skills/example".to_owned(),
            tracked_ref: None,
        })
        .expect("有效候选 ref 应生成确认 Plan");
    let UiOutcome::SourceRefChangePlan { plan } = outcome else {
        panic!("不同 ref 不能静默覆盖当前 Source");
    };
    assert_eq!(plan.current_ref, "main");
    assert_eq!(plan.candidate_ref, "next");
    assert_eq!(plan.member_path_hint.as_deref(), Some("skills/example"));
    let before = open_source_discovery(&application);
    assert_eq!(
        before
            .iter()
            .find(|source| source.id == plan.source_id)
            .expect("原 Source 应存在")
            .tracked_ref,
        "main"
    );

    let confirmed = application
        .handle(UiIntent::ConfirmSourceRefChange {
            plan_id: plan.id.clone(),
        })
        .expect("用户确认后应切换 Tracked Ref");
    let UiOutcome::SourceDiscovery { sources, .. } = confirmed else {
        panic!("确认后应返回 Source 发现状态");
    };
    let source = sources
        .iter()
        .find(|source| source.id == plan.source_id)
        .expect("切换后 Source 应继续存在");
    assert_eq!(source.tracked_ref, "next");
    assert_eq!(source.member_path_hint.as_deref(), Some("skills/example"));
    assert_eq!(source.catalog_status, SourceCatalogStatus::Unloaded);
    assert!(source.bundle_id.is_none());

    drop(application);
    let restarted = ready_application_with_transport(sandbox.path(), transport);
    let persisted = open_source_discovery(&restarted);
    let source = persisted
        .iter()
        .find(|source| source.id == plan.source_id)
        .expect("重启后应保留同一个 Source");
    assert_eq!(source.tracked_ref, "next");
    assert_eq!(source.member_path_hint.as_deref(), Some("skills/example"));

    let replay = restarted
        .handle(UiIntent::ConfirmSourceRefChange { plan_id: plan.id })
        .expect_err("同一确认 Plan 不能重复使用");
    assert!(replay.to_string().contains("已经使用"));
}

#[test]
fn first_discovery_reloads_every_catalog_once_and_restart_preserves_failed_catalogs() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let application = ready_application_with_transport(sandbox.path(), transport.clone());
    let archive = catalog_archive();
    transport.enqueue_seed_catalogs(&archive);

    let first = open_source_discovery(&application);
    assert_eq!(transport.request_count(), 12);
    assert!(first.iter().all(|source| {
        source.catalog_status == SourceCatalogStatus::Fresh
            && source.catalog_commit_sha.is_some()
            && source.members.len() == 2
            && source.members.iter().all(|member| member.selectable)
    }));
    assert_eq!(first[0].members[0].relative_path, "skills/alpha");
    assert_eq!(first[0].members[0].skill_name.as_deref(), Some("alpha"));

    let second = open_source_discovery(&application);
    assert_eq!(second, first);
    assert_eq!(transport.request_count(), 12, "同一会话不能再次自动联网");

    drop(application);
    let failing_transport = Arc::new(RecordingTransport::default());
    let restarted = ready_application_with_transport(sandbox.path(), failing_transport.clone());
    let stale = open_source_discovery(&restarted);
    assert_eq!(failing_transport.request_count(), 4);
    assert!(stale.iter().all(|source| {
        source.catalog_status == SourceCatalogStatus::Stale
            && source.last_reload_error.is_some()
            && source.members.len() == 2
    }));
    assert!(stale.iter().all(|source| source.bundle_id.is_none()));
    assert_eq!(
        open_source_discovery(&restarted),
        stale,
        "失败后同一会话也只能由用户主动重新加载"
    );
    assert_eq!(failing_transport.request_count(), 4);
}

#[test]
fn catalog_network_fetch_holds_the_cross_process_lifecycle_lock() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let paths = ApplicationPaths::for_home(
        sandbox.path().join("application-support/SkillYard"),
        sandbox.path().join("home"),
    );
    let transport = Arc::new(BlockingTransport::default());
    let loading_application = Arc::new(SkillYardApplication::new_with_source_transport(
        paths.clone(),
        PlatformInfo::supported_for_test(),
        transport.clone(),
    ));
    loading_application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    let loading_thread = {
        let application = loading_application.clone();
        thread::spawn(move || application.handle(UiIntent::OpenSourceDiscovery))
    };
    transport.wait_until_request();

    let competing_application = SkillYardApplication::new_with_source_transport(
        paths,
        PlatformInfo::supported_for_test(),
        Arc::new(RecordingTransport::default()),
    );
    let error = competing_application
        .handle(UiIntent::RefreshLocalInventory)
        .expect_err("Catalog 联网期间另一实例不能开始生命周期写操作");
    assert!(error.to_string().contains("已有另一个 SkillYard 实例"));

    transport.release();
    loading_thread
        .join()
        .expect("Catalog 加载线程不应 panic")
        .expect("网络失败应保存为 Source 状态而不是让加载失败");
}

#[test]
fn dangerous_reload_keeps_the_old_catalog_and_valid_empty_archive_replaces_it() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let application = ready_application_with_transport(sandbox.path(), transport.clone());
    transport.enqueue_seed_catalogs(&catalog_archive());
    let first = open_source_discovery(&application);
    let source_id = first[0].id.clone();
    let tracked_ref = first[0].tracked_ref.clone();
    let old_commit = first[0]
        .catalog_commit_sha
        .clone()
        .expect("首次 Catalog 应保存 commit");

    transport.enqueue_public_repository("anthropics/skills", &tracked_ref);
    transport.enqueue_commit("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
    transport.enqueue_bytes(200, &symlink_archive());
    let stale = reload_source(&application, &source_id);
    let source = stale
        .iter()
        .find(|source| source.id == source_id)
        .expect("失败后 Source 应继续存在");
    assert_eq!(source.catalog_status, SourceCatalogStatus::Stale);
    assert_eq!(
        source.catalog_commit_sha.as_deref(),
        Some(old_commit.as_str())
    );
    assert_eq!(source.members.len(), 2);
    assert!(source.last_reload_error.is_some());

    transport.enqueue_public_repository("anthropics/skills", &tracked_ref);
    transport.enqueue_commit("ffffffffffffffffffffffffffffffffffffffff");
    transport.enqueue_bytes(200, &empty_catalog_archive());
    let fresh = reload_source(&application, &source_id);
    let source = fresh
        .iter()
        .find(|source| source.id == source_id)
        .expect("空 Catalog 成功后 Source 应继续存在");
    assert_eq!(source.catalog_status, SourceCatalogStatus::Fresh);
    assert!(source.members.is_empty());
    assert_eq!(
        source.catalog_commit_sha.as_deref(),
        Some("ffffffffffffffffffffffffffffffffffffffff")
    );
    assert_eq!(
        std::fs::read_dir(sandbox.path().join("application-support/SkillYard/staging"))
            .expect("应读取 staging")
            .count(),
        0,
        "Catalog 成功或失败都不能遗留临时内容"
    );
}

#[test]
fn catalog_database_failure_rolls_back_the_complete_previous_snapshot() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let application = ready_application_with_transport(sandbox.path(), transport.clone());
    transport.enqueue_seed_catalogs(&catalog_archive());
    let first = open_source_discovery(&application);
    let source_id = first[0].id.clone();
    let old_commit = first[0]
        .catalog_commit_sha
        .clone()
        .expect("首次 Catalog 应保存 commit");

    let database = sandbox
        .path()
        .join("application-support/SkillYard/skillyard.sqlite3");
    let connection = Connection::open(&database).expect("应打开真实 SQLite");
    connection
        .execute_batch(
            "CREATE TRIGGER abort_catalog_insert
             BEFORE INSERT ON source_catalog_members
             BEGIN
                 SELECT RAISE(ABORT, 'catalog fixture abort');
             END;",
        )
        .expect("应安装 Catalog 事务失败 fixture");
    drop(connection);

    transport.enqueue_catalog(
        "anthropics/skills",
        "main",
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        &catalog_archive(),
    );
    let error = application
        .handle(UiIntent::ReloadGitHubSource {
            source_id: source_id.clone(),
        })
        .expect_err("SQLite 提交失败不能伪装成远端 Stale");
    assert!(error.to_string().contains("无法保存 Source Catalog"));

    let after = open_source_discovery(&application);
    let source = after
        .iter()
        .find(|source| source.id == source_id)
        .expect("回滚后原 Source 应存在");
    assert_eq!(source.catalog_status, SourceCatalogStatus::Fresh);
    assert_eq!(
        source.catalog_commit_sha.as_deref(),
        Some(old_commit.as_str())
    );
    assert_eq!(source.members.len(), 2);
}

fn open_source_discovery(application: &SkillYardApplication) -> Vec<skillyard_lib::SourceSummary> {
    match application
        .handle(UiIntent::OpenSourceDiscovery)
        .expect("已完成首次扫描后应能浏览 Source")
    {
        UiOutcome::SourceDiscovery { sources, .. } => sources,
        _ => panic!("应返回 Source 发现状态"),
    }
}

fn ready_application_with_transport(
    root: &std::path::Path,
    transport: Arc<RecordingTransport>,
) -> SkillYardApplication {
    let application = SkillYardApplication::new_with_source_transport(
        ApplicationPaths::for_home(
            root.join("application-support/SkillYard"),
            root.join("home"),
        ),
        PlatformInfo::supported_for_test(),
        transport,
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    application
}

fn add_source(
    application: &SkillYardApplication,
    input: &str,
    tracked_ref: Option<&str>,
) -> Vec<skillyard_lib::SourceSummary> {
    match application
        .handle(UiIntent::AddGitHubSource {
            input: input.to_owned(),
            tracked_ref: tracked_ref.map(str::to_owned),
        })
        .expect("有效 GitHub 输入应保存或复用 Source")
    {
        UiOutcome::SourceDiscovery { sources, .. } => sources,
        _ => panic!("同一 ref 应直接返回 Source 发现状态"),
    }
}

fn reload_source(
    application: &SkillYardApplication,
    source_id: &str,
) -> Vec<skillyard_lib::SourceSummary> {
    match application
        .handle(UiIntent::ReloadGitHubSource {
            source_id: source_id.to_owned(),
        })
        .expect("预期的远端失败应记录为 Source 状态")
    {
        UiOutcome::SourceDiscovery { sources, .. } => sources,
        _ => panic!("重新加载后应返回 Source 发现状态"),
    }
}

#[derive(Default)]
struct RecordingTransport {
    responses: Mutex<VecDeque<ScriptedResponse>>,
    requests: Mutex<Vec<SourceRequest>>,
}

#[derive(Default)]
struct BlockingTransport {
    state: Mutex<BlockingTransportState>,
    requested: Condvar,
    released: Condvar,
}

#[derive(Default)]
struct BlockingTransportState {
    requested: bool,
    released: bool,
}

impl BlockingTransport {
    fn wait_until_request(&self) {
        let state = self.state.lock().expect("应读取阻塞 transport 状态");
        let (state, timeout) = self
            .requested
            .wait_timeout_while(state, Duration::from_secs(5), |state| !state.requested)
            .expect("应等待 Catalog 网络请求");
        assert!(
            state.requested && !timeout.timed_out(),
            "Catalog 请求应在期限内开始"
        );
    }

    fn release(&self) {
        let mut state = self.state.lock().expect("应写入阻塞 transport 状态");
        state.released = true;
        self.released.notify_all();
    }
}

impl SourceTransport for BlockingTransport {
    fn get(&self, _request: SourceRequest) -> Result<SourceResponse, SourceTransportError> {
        let mut state = self.state.lock().expect("应写入阻塞 transport 状态");
        state.requested = true;
        self.requested.notify_all();
        while !state.released {
            state = self.released.wait(state).expect("应等待测试释放网络请求");
        }
        Err(SourceTransportError::Unavailable)
    }
}

struct ScriptedResponse {
    status: u16,
    body: Vec<u8>,
}

impl RecordingTransport {
    fn enqueue(&self, status: u16, body: &str) {
        self.responses
            .lock()
            .expect("应写入响应队列")
            .push_back(ScriptedResponse {
                status,
                body: body.as_bytes().to_vec(),
            });
    }

    fn enqueue_public_repository(&self, full_name: &str, default_branch: &str) {
        self.enqueue(
            200,
            &format!(
                r#"{{"full_name":"{full_name}","default_branch":"{default_branch}","private":false}}"#
            ),
        );
    }

    fn enqueue_commit(&self, sha: &str) {
        self.enqueue(200, &format!(r#"{{"sha":"{sha}"}}"#));
    }

    fn enqueue_catalog(&self, full_name: &str, tracked_ref: &str, sha: &str, archive: &[u8]) {
        self.enqueue_public_repository(full_name, tracked_ref);
        self.enqueue_commit(sha);
        self.enqueue_bytes(200, archive);
    }

    fn enqueue_seed_catalogs(&self, archive: &[u8]) {
        for (full_name, tracked_ref, sha) in [
            (
                "anthropics/skills",
                "main",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            (
                "ComposioHQ/awesome-claude-skills",
                "master",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
            (
                "cexll/myclaude",
                "master",
                "cccccccccccccccccccccccccccccccccccccccc",
            ),
            (
                "JimLiu/baoyu-skills",
                "main",
                "dddddddddddddddddddddddddddddddddddddddd",
            ),
        ] {
            self.enqueue_catalog(full_name, tracked_ref, sha, archive);
        }
    }

    fn enqueue_bytes(&self, status: u16, body: &[u8]) {
        self.responses
            .lock()
            .expect("应写入响应队列")
            .push_back(ScriptedResponse {
                status,
                body: body.to_vec(),
            });
    }

    fn request_count(&self) -> usize {
        self.requests.lock().expect("应读取请求记录").len()
    }

    fn requests(&self) -> Vec<SourceRequest> {
        self.requests.lock().expect("应读取请求记录").clone()
    }
}

fn catalog_archive() -> Vec<u8> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o100644);
    for (path, name) in [
        ("repository-sha/skills/alpha/SKILL.md", "alpha"),
        ("repository-sha/skills/beta/SKILL.md", "beta"),
    ] {
        archive.start_file(path, options).expect("应写入 ZIP entry");
        write!(
            archive,
            "---\nname: {name}\ndescription: {name} catalog skill\n---\n# {name}\n"
        )
        .expect("应写入 Skill metadata");
    }
    archive.finish().expect("应完成 ZIP fixture").into_inner()
}

fn symlink_archive() -> Vec<u8> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .add_symlink(
            "repository-sha/skills/alpha/link",
            "../../outside",
            SimpleFileOptions::default().unix_permissions(0o120777),
        )
        .expect("应写入 symlink fixture");
    archive
        .finish()
        .expect("应完成 symlink ZIP fixture")
        .into_inner()
}

fn empty_catalog_archive() -> Vec<u8> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .start_file(
            "repository-sha/README.md",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .expect("应写入空 Catalog fixture");
    archive
        .write_all(b"# Repository without skills\n")
        .expect("应写入 README");
    archive
        .finish()
        .expect("应完成空 Catalog ZIP fixture")
        .into_inner()
}

impl SourceTransport for RecordingTransport {
    fn get(&self, request: SourceRequest) -> Result<SourceResponse, SourceTransportError> {
        self.requests
            .lock()
            .expect("应记录 GitHub 请求")
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
            body: Box::new(Cursor::new(response.body)),
        })
    }
}
