use std::{
    collections::VecDeque,
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, BundleUpdateStatus, PlatformInfo, SkillYardApplication, SourceRequest,
    SourceResponse, SourceTransport, SourceTransportError, UiIntent, UiOutcome,
};
use tempfile::tempdir;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const BASELINE_COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const AVAILABLE_COMMIT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn github_update_check_reads_only_marker_and_survives_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, paths, data_root) = install_github_bundle(sandbox.path(), transport.clone());
    let (bundle_id, current_target, skill_root) = installed_state(&data_root);
    let original_content =
        fs::read_to_string(skill_root.join("payload.txt")).expect("应读取当前受管内容");
    let request_start = transport.requests().len();

    transport.enqueue_repository("Acme/Toolkit", "main");
    transport.enqueue_commit(AVAILABLE_COMMIT);
    let checked = application
        .handle(UiIntent::CheckBundleUpdates)
        .expect("用户主动检查应成功");
    let summary = bundle_update(&checked, &bundle_id);
    assert_eq!(summary.status, BundleUpdateStatus::Available);
    assert_eq!(
        checked_marker_for(&data_root, &bundle_id).as_deref(),
        Some(AVAILABLE_COMMIT)
    );
    let check_requests = &transport.requests()[request_start..];
    assert_eq!(check_requests.len(), 2, "检查只应读取 metadata 和 commit");
    assert!(
        check_requests
            .iter()
            .all(|request| !request.url.path().contains("/zipball/")),
        "Update Check 不能下载候选内容"
    );
    assert_eq!(
        current_target_for(&data_root, &bundle_id),
        current_target,
        "检查不能切换 Bundle current"
    );
    assert_eq!(
        fs::read_to_string(skill_root.join("payload.txt")).expect("检查后内容应仍可读"),
        original_content
    );
    assert_eq!(staging_entry_count(&data_root), 0);

    drop(application);
    let restarted_transport = Arc::new(RecordingTransport::default());
    let restarted = SkillYardApplication::new_with_source_transport(
        paths.clone(),
        PlatformInfo::supported_for_test(),
        restarted_transport.clone(),
    );
    let startup = restarted
        .handle(UiIntent::GetStartupState)
        .expect("重启后应读取已保存检查结果");
    assert_eq!(
        bundle_update(&startup, &bundle_id).status,
        BundleUpdateStatus::Available
    );
    assert!(
        restarted_transport.requests().is_empty(),
        "启动不能隐式检查上游"
    );
}

#[test]
fn failed_check_keeps_last_successful_marker_and_current_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, _paths, data_root) = install_github_bundle(sandbox.path(), transport.clone());
    let (bundle_id, current_target, skill_root) = installed_state(&data_root);
    let original_content =
        fs::read_to_string(skill_root.join("payload.txt")).expect("应读取当前受管内容");

    transport.enqueue_repository("Acme/Toolkit", "main");
    transport.enqueue_commit(AVAILABLE_COMMIT);
    application
        .handle(UiIntent::CheckBundleUpdates)
        .expect("第一次检查应保存成功 marker");
    let requests_before_failure = transport.requests().len();

    let failed = application
        .handle(UiIntent::CheckBundleUpdates)
        .expect("单个来源查询失败应作为可见状态返回");
    let summary = bundle_update(&failed, &bundle_id);
    assert_eq!(summary.status, BundleUpdateStatus::UnableToCheck);
    assert_eq!(
        checked_marker_for(&data_root, &bundle_id).as_deref(),
        Some(AVAILABLE_COMMIT),
        "失败不能抹掉上次成功上游标识"
    );
    assert_eq!(
        transport.requests().len(),
        requests_before_failure + 1,
        "transport 失败后不能继续伪造 commit 或 archive 请求"
    );
    assert_eq!(current_target_for(&data_root, &bundle_id), current_target);
    assert_eq!(
        fs::read_to_string(skill_root.join("payload.txt")).expect("失败后内容应仍可读"),
        original_content
    );
    assert_eq!(staging_entry_count(&data_root), 0);
}

#[test]
fn failed_check_is_visible_when_a_direct_association_has_no_adopted_marker() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, _paths, data_root) = install_github_bundle(sandbox.path(), transport);
    let (bundle_id, _, _) = installed_state(&data_root);

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    connection
        .execute(
            "UPDATE source_bundle_links
             SET adopted_marker = NULL,
                 update_check_status = 'not_checked',
                 update_checked_marker = NULL,
                 update_checked_at = NULL,
                 update_check_error = NULL
             WHERE bundle_id = ?1",
            [&bundle_id],
        )
        .expect("应模拟刚完成、尚无采用基线的直接来源关联");
    drop(connection);

    let failed = application
        .handle(UiIntent::CheckBundleUpdates)
        .expect("网络失败应作为可见检查结果返回");
    let summary = bundle_update(&failed, &bundle_id);
    assert_eq!(summary.status, BundleUpdateStatus::UnableToCheck);
    assert!(
        summary.action.is_none(),
        "检查失败后不能继续显示未经验证的更新动作"
    );
}

#[test]
fn confirmed_tracked_ref_change_uses_the_resolved_candidate_marker() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, _paths, data_root) = install_github_bundle(sandbox.path(), transport.clone());
    let (bundle_id, _, _) = installed_state(&data_root);

    transport.enqueue_repository("Acme/Toolkit", "main");
    transport.enqueue_commit(BASELINE_COMMIT);
    let checked = application
        .handle(UiIntent::CheckBundleUpdates)
        .expect("旧 ref 应先检查为最新");
    assert_eq!(
        bundle_update(&checked, &bundle_id).status,
        BundleUpdateStatus::UpToDate
    );

    transport.enqueue_repository("Acme/Toolkit", "main");
    transport.enqueue_commit(AVAILABLE_COMMIT);
    let proposed = application
        .handle(UiIntent::AddGitHubSource {
            input: "https://github.com/acme/toolkit/tree/next".to_owned(),
            tracked_ref: None,
        })
        .expect("不同 ref 应生成确认 Plan");
    let UiOutcome::SourceRefChangePlan { plan } = proposed else {
        panic!("不同 ref 不能静默覆盖");
    };
    application
        .handle(UiIntent::ConfirmSourceRefChange { plan_id: plan.id })
        .expect("用户确认后应切换 Tracked Ref");

    let startup = application
        .handle(UiIntent::GetStartupState)
        .expect("确认后应读取新的检查状态");
    assert_eq!(
        bundle_update(&startup, &bundle_id).status,
        BundleUpdateStatus::Available,
        "候选 commit 与既有采用基线不同，应立即显示可更新"
    );
    assert_eq!(
        checked_marker_for(&data_root, &bundle_id).as_deref(),
        Some(AVAILABLE_COMMIT)
    );
}

#[test]
fn blocked_bundle_update_check_preserves_state_without_network_access() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(RecordingTransport::default());
    let (application, _paths, data_root) = install_github_bundle(sandbox.path(), transport.clone());
    let (bundle_id, _, _) = installed_state(&data_root);

    transport.enqueue_repository("Acme/Toolkit", "main");
    transport.enqueue_commit(AVAILABLE_COMMIT);
    let available = application
        .handle(UiIntent::CheckBundleUpdates)
        .expect("阻塞前应保存可更新状态");
    assert_eq!(
        bundle_update(&available, &bundle_id).status,
        BundleUpdateStatus::Available
    );
    assert!(
        bundle_update(&available, &bundle_id).action.is_some(),
        "正常 Bundle 应提供更新入口"
    );

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let source_id = connection
        .query_row(
            "SELECT source_id FROM source_bundle_links WHERE bundle_id = ?1",
            [&bundle_id],
            |row| row.get::<_, String>(0),
        )
        .expect("应读取关联 Source");
    connection
        .execute_batch(&format!(
            "INSERT INTO source_association_plans (
                 id, payload_json, payload_sha256, status, created_at, expires_at
             ) VALUES (
                 'blocked-update-check-plan', '{{}}',
                 '0000000000000000000000000000000000000000000000000000000000000000',
                 'consumed', 1, 2
             );
             INSERT INTO source_association_transactions (
                 id, plan_id, source_id, target_bundle_id, retiring_bundle_id,
                 content_choices_json, source_mappings_json, journal_path,
                 phase, status, error_message, created_at, updated_at
             ) VALUES (
                 'blocked-update-check-transaction', 'blocked-update-check-plan',
                 '{source_id}', '{bundle_id}', 'retiring-test-bundle',
                 '[]', '[]', 'journals/blocked-update-check.json',
                 'journal_pending', 'blocked', '等待人工恢复', 1, 1
             );"
        ))
        .expect("应模拟已进入人工恢复的关联事务");
    drop(connection);

    let request_count = transport.requests().len();
    let checked = application
        .handle(UiIntent::CheckBundleUpdates)
        .expect("blocked Bundle 不应阻塞查看 Inventory");
    assert_eq!(
        transport.requests().len(),
        request_count,
        "blocked Source 必须在网络请求前跳过"
    );
    assert_eq!(
        bundle_update(&checked, &bundle_id).status,
        BundleUpdateStatus::Available,
        "人工恢复不能抹掉最近一次成功检查结果"
    );
    assert!(
        bundle_update(&checked, &bundle_id).action.is_none(),
        "blocked Bundle 不能继续暴露写操作入口"
    );
}

fn install_github_bundle(
    sandbox: &Path,
    transport: Arc<RecordingTransport>,
) -> (SkillYardApplication, ApplicationPaths, PathBuf) {
    let data_root = sandbox.join("application-support/SkillYard");
    let paths = ApplicationPaths::for_home(data_root.clone(), sandbox.join("home"));
    let application = SkillYardApplication::new_with_source_transport(
        paths.clone(),
        PlatformInfo::supported_for_test(),
        transport.clone(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    transport.enqueue_repository("Acme/Toolkit", "main");
    transport.enqueue_commit(BASELINE_COMMIT);
    let added = application
        .handle(UiIntent::AddGitHubSource {
            input: "acme/toolkit".to_owned(),
            tracked_ref: Some("main".to_owned()),
        })
        .expect("应登记测试 Source");
    let UiOutcome::SourceDiscovery { sources, .. } = added else {
        panic!("登记后应返回 Source 列表");
    };
    let source_id = sources
        .into_iter()
        .find(|source| source.canonical_identity == "github:acme/toolkit")
        .expect("应找到测试 Source")
        .id;

    transport.enqueue_repository("Acme/Toolkit", "main");
    transport.enqueue_commit(BASELINE_COMMIT);
    transport.enqueue_bytes(bundle_archive());
    application
        .handle(UiIntent::ReloadGitHubSource {
            source_id: source_id.clone(),
        })
        .expect("应加载测试 Source Catalog");

    transport.enqueue_bytes(bundle_archive());
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateGithubInstallPlan { source_id })
        .expect("应生成 GitHub 安装 Plan")
    else {
        panic!("应返回安装 Plan");
    };
    let selected_candidate_ids = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.default_selected)
        .map(|candidate| candidate.candidate_id.clone())
        .collect::<Vec<_>>();
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids,
        })
        .expect("应建立 GitHub Bundle 基线");
    (application, paths, data_root)
}

fn installed_state(data_root: &Path) -> (String, String, PathBuf) {
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let (bundle_id, current_target, managed_directory, stable_relative_path) = connection
        .query_row(
            "SELECT bundle.id, bundle.current_target, bundle.managed_directory,
                    member.stable_relative_path
             FROM bundles AS bundle
             JOIN skill_members AS member ON member.bundle_id = bundle.id
             LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .expect("应读取受管 Bundle");
    let skill_root = data_root
        .join(managed_directory)
        .join("current")
        .join(stable_relative_path);
    (bundle_id, current_target, skill_root)
}

fn current_target_for(data_root: &Path, bundle_id: &str) -> String {
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(
            "SELECT current_target FROM bundles WHERE id = ?1",
            [bundle_id],
            |row| row.get(0),
        )
        .expect("应读取 Bundle current")
}

fn checked_marker_for(data_root: &Path, bundle_id: &str) -> Option<String> {
    Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(
            "SELECT update_checked_marker
             FROM source_bundle_links
             WHERE bundle_id = ?1",
            [bundle_id],
            |row| row.get(0),
        )
        .expect("应读取最近成功上游标识")
}

fn bundle_update<'a>(
    outcome: &'a UiOutcome,
    bundle_id: &str,
) -> &'a skillyard_lib::BundleUpdateSummary {
    let UiOutcome::Inventory { bundle_updates, .. } = outcome else {
        panic!("检查应返回 Inventory");
    };
    bundle_updates
        .iter()
        .find(|summary| summary.bundle_id == bundle_id)
        .expect("Inventory 应包含 Bundle 更新摘要")
}

fn staging_entry_count(data_root: &Path) -> usize {
    fs::read_dir(data_root.join("staging"))
        .expect("应读取 staging")
        .count()
}

fn bundle_archive() -> Vec<u8> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o100644);
    archive
        .start_file("repository-sha/skills/alpha/SKILL.md", options)
        .expect("应写入 Skill metadata");
    archive
        .write_all(b"---\nname: alpha\ndescription: update check fixture\n---\n# Alpha\n")
        .expect("应写入 Skill metadata");
    archive
        .start_file("repository-sha/skills/alpha/payload.txt", options)
        .expect("应写入 Skill 内容");
    archive.write_all(b"baseline").expect("应写入 Skill 内容");
    archive.finish().expect("应完成 ZIP fixture").into_inner()
}

#[derive(Default)]
struct RecordingTransport {
    responses: Mutex<VecDeque<Vec<u8>>>,
    requests: Mutex<Vec<SourceRequest>>,
}

impl RecordingTransport {
    fn enqueue_repository(&self, full_name: &str, default_branch: &str) {
        self.enqueue_json(format!(
            r#"{{"full_name":"{full_name}","default_branch":"{default_branch}","private":false}}"#
        ));
    }

    fn enqueue_commit(&self, sha: &str) {
        self.enqueue_json(format!(r#"{{"sha":"{sha}"}}"#));
    }

    fn enqueue_json(&self, body: String) {
        self.responses
            .lock()
            .expect("应写入响应队列")
            .push_back(body.into_bytes());
    }

    fn enqueue_bytes(&self, body: Vec<u8>) {
        self.responses
            .lock()
            .expect("应写入响应队列")
            .push_back(body);
    }

    fn requests(&self) -> Vec<SourceRequest> {
        self.requests.lock().expect("应读取请求记录").clone()
    }
}

impl SourceTransport for RecordingTransport {
    fn get(&self, request: SourceRequest) -> Result<SourceResponse, SourceTransportError> {
        self.requests
            .lock()
            .expect("应记录请求")
            .push(request.clone());
        let body = self
            .responses
            .lock()
            .expect("应读取响应队列")
            .pop_front()
            .ok_or(SourceTransportError::Unavailable)?;
        Ok(SourceResponse {
            status: 200,
            final_url: request.url,
            body: Box::new(Cursor::new(body)),
        })
    }
}
