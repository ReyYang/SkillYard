use std::{
    collections::VecDeque,
    io::Cursor,
    sync::{Arc, Mutex},
};

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, PlatformInfo, SkillYardApplication, SourceCatalogStatus, SourceRequest,
    SourceResponse, SourceTransport, SourceTransportError, UiIntent, UiOutcome,
};
use tempfile::tempdir;

#[test]
fn recommended_github_sources_exist_without_creating_bundles_and_survive_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    let first = open_source_discovery(&application);
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
    let restarted = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    assert_eq!(open_source_discovery(&restarted), first);
}

#[test]
fn deleting_a_recommended_source_does_not_seed_it_again_on_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
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

    let restarted = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let sources = open_source_discovery(&restarted);
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

#[derive(Default)]
struct RecordingTransport {
    responses: Mutex<VecDeque<ScriptedResponse>>,
    requests: Mutex<Vec<SourceRequest>>,
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

    fn request_count(&self) -> usize {
        self.requests.lock().expect("应读取请求记录").len()
    }

    fn requests(&self) -> Vec<SourceRequest> {
        self.requests.lock().expect("应读取请求记录").clone()
    }
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
