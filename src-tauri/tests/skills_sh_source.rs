use std::{
    io::{self, Cursor, Read},
    sync::{Arc, Mutex},
};

use skillyard_lib::{
    ApplicationPaths, PlatformInfo, SkillYardApplication, SourceRequest, SourceResponse,
    SourceTransport, SourceTransportError, UiIntent, UiOutcome,
};
use tempfile::tempdir;
use url::Url;

#[test]
fn skills_sh_search_groups_github_members_without_creating_a_second_source_kind() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let transport = Arc::new(FixtureTransport::with_json(
        r#"{
            "query": "react",
            "searchType": "fuzzy",
            "skills": [
                {
                    "id": "vercel-labs/agent-skills/react-best-practices",
                    "skillId": "react-best-practices",
                    "name": "React Best Practices",
                    "installs": 20,
                    "source": "vercel-labs/agent-skills"
                },
                {
                    "id": "Vercel-Labs/Agent-Skills/react-native",
                    "skillId": "react-native",
                    "name": "React Native",
                    "installs": 10,
                    "source": "https://github.com/Vercel-Labs/Agent-Skills.git"
                },
                {
                    "id": "vercel-labs/agent-skills/react-best-practices",
                    "skillId": "react-best-practices",
                    "name": "React Best Practices",
                    "installs": 40,
                    "source": "https://github.com/vercel-labs/agent-skills.git"
                },
                {
                    "id": "react.dev/react",
                    "skillId": "react",
                    "name": "React",
                    "installs": 30,
                    "source": "react.dev"
                }
            ],
            "count": 4,
            "duration_ms": 4
        }"#,
    ));
    let data_root = sandbox.path().join("data");
    let home = sandbox.path().join("home");
    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let application = SkillYardApplication::new_with_source_transport(
        paths.clone(),
        PlatformInfo::supported_for_test(),
        transport.clone(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应先完成首次扫描");
    let baseline = database_counts(&data_root);
    assert_eq!(baseline, (4, 0, 0), "首次扫描只应创建推荐 Source");

    let UiOutcome::SkillsShSearch { query, sources } = application
        .handle(UiIntent::SearchSkillsSh {
            query: " react ".to_owned(),
        })
        .expect("公开 skills.sh 搜索结果应能还原成 GitHub Source")
    else {
        panic!("应返回 skills.sh 搜索结果");
    };

    assert_eq!(query, "react");
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0].source_input, "vercel-labs/agent-skills");
    assert!(sources[0].supported);
    assert_eq!(
        sources[0]
            .members
            .iter()
            .map(|member| (member.skill_id.as_str(), member.installs))
            .collect::<Vec<_>>(),
        [("react-best-practices", 40), ("react-native", 10)]
    );
    assert_eq!(sources[1].source_input, "react.dev");
    assert!(!sources[1].supported);
    assert_eq!(
        transport.requests(),
        vec!["https://skills.sh/api/search?q=react"]
    );

    assert_eq!(
        database_counts(&data_root),
        baseline,
        "搜索只能发现候选，不能把 skills.sh 变成生命周期 Source"
    );
    assert_restart_inventory(paths);
}

#[test]
fn skills_sh_response_failures_do_not_mutate_persistent_state() {
    let oversized = vec![b'x'; 2 * 1024 * 1024 + 1];
    let cases = [
        (
            "HTTP 状态",
            FixtureResponse::bytes(
                503,
                "https://skills.sh/api/search?q=react",
                br#"{"skills":[]}"#.to_vec(),
            ),
            "HTTP 状态",
        ),
        (
            "意外 final URL",
            FixtureResponse::bytes(
                200,
                "https://example.com/api/search?q=react",
                br#"{"skills":[]}"#.to_vec(),
            ),
            "意外的跳转地址",
        ),
        (
            "空响应",
            FixtureResponse::bytes(200, "https://skills.sh/api/search?q=react", Vec::new()),
            "无效响应",
        ),
        (
            "无效 JSON",
            FixtureResponse::bytes(
                200,
                "https://skills.sh/api/search?q=react",
                b"not-json".to_vec(),
            ),
            "无效响应",
        ),
        (
            "读取中断",
            FixtureResponse::interrupted(200, "https://skills.sh/api/search?q=react"),
            "无效响应",
        ),
        (
            "响应超限",
            FixtureResponse::bytes(200, "https://skills.sh/api/search?q=react", oversized),
            "2097153 bytes",
        ),
    ];

    for (name, response, expected_error) in cases {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        let paths = ApplicationPaths::for_home(data_root.clone(), sandbox.path().join("home"));
        let application = SkillYardApplication::new_with_source_transport(
            paths.clone(),
            PlatformInfo::supported_for_test(),
            Arc::new(FixtureTransport::new(response)),
        );
        application
            .handle(UiIntent::StartInitialScan)
            .expect("应先完成首次扫描");
        let baseline = database_counts(&data_root);
        assert_eq!(baseline, (4, 0, 0), "{name} fixture 的初始状态应稳定");

        let error = application
            .handle(UiIntent::SearchSkillsSh {
                query: "react".to_owned(),
            })
            .expect_err("异常响应必须作为搜索失败返回");
        assert!(
            error.to_string().contains(expected_error),
            "{name} 应返回对应错误，实际为：{error}"
        );
        assert_eq!(
            database_counts(&data_root),
            baseline,
            "{name} 不能新增 Source、Install Plan 或 Bundle"
        );

        drop(application);
        assert_restart_inventory(paths);
        assert_eq!(
            database_counts(&data_root),
            baseline,
            "{name} 在重启恢复后仍不能污染持久状态"
        );
    }
}

#[test]
fn skills_sh_zero_results_is_a_success_without_persistent_changes() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let data_root = sandbox.path().join("data");
    let paths = ApplicationPaths::for_home(data_root.clone(), sandbox.path().join("home"));
    let application = SkillYardApplication::new_with_source_transport(
        paths.clone(),
        PlatformInfo::supported_for_test(),
        Arc::new(FixtureTransport::with_json(r#"{"skills":[]}"#)),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应先完成首次扫描");
    let baseline = database_counts(&data_root);

    let outcome = application
        .handle(UiIntent::SearchSkillsSh {
            query: "nothing".to_owned(),
        })
        .expect("零结果仍是一次成功搜索");
    assert_eq!(
        outcome,
        UiOutcome::SkillsShSearch {
            query: "nothing".to_owned(),
            sources: Vec::new(),
        }
    );
    assert_eq!(database_counts(&data_root), baseline);

    drop(application);
    assert_restart_inventory(paths);
}

fn database_counts(data_root: &std::path::Path) -> (i64, i64, i64) {
    let connection =
        rusqlite::Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开 SQLite");
    connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM sources),
                (SELECT COUNT(*) FROM install_plans),
                (SELECT COUNT(*) FROM bundles)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("应读取生命周期对象数量")
}

fn assert_restart_inventory(paths: ApplicationPaths) {
    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    assert!(
        matches!(
            reopened
                .handle(UiIntent::GetStartupState)
                .expect("重启后应能读取持久清单"),
            UiOutcome::Inventory { .. }
        ),
        "搜索结果或失败不应破坏已保存 Inventory"
    );
}

#[derive(Clone)]
enum FixtureBody {
    Bytes(Vec<u8>),
    Interrupted,
}

#[derive(Clone)]
struct FixtureResponse {
    status: u16,
    final_url: Url,
    body: FixtureBody,
}

impl FixtureResponse {
    fn bytes(status: u16, final_url: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            final_url: Url::parse(final_url).expect("fixture URL 应合法"),
            body: FixtureBody::Bytes(body),
        }
    }

    fn interrupted(status: u16, final_url: &str) -> Self {
        Self {
            status,
            final_url: Url::parse(final_url).expect("fixture URL 应合法"),
            body: FixtureBody::Interrupted,
        }
    }
}

struct FixtureTransport {
    response: FixtureResponse,
    requests: Mutex<Vec<String>>,
}

impl FixtureTransport {
    fn with_json(json: &str) -> Self {
        Self::new(FixtureResponse::bytes(
            200,
            "https://skills.sh/api/search?q=react",
            json.as_bytes().to_vec(),
        ))
    }

    fn new(response: FixtureResponse) -> Self {
        Self {
            response,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("请求锁不应损坏").clone()
    }
}

impl SourceTransport for FixtureTransport {
    fn get(&self, request: SourceRequest) -> Result<SourceResponse, SourceTransportError> {
        self.requests
            .lock()
            .expect("请求锁不应损坏")
            .push(request.url.as_str().to_owned());
        Ok(SourceResponse {
            status: self.response.status,
            final_url: self.response.final_url.clone(),
            body: match &self.response.body {
                FixtureBody::Bytes(bytes) => Box::new(Cursor::new(bytes.clone())),
                FixtureBody::Interrupted => Box::new(InterruptedReader),
            },
        })
    }
}

struct InterruptedReader;

impl Read for InterruptedReader {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("fixture read interrupted"))
    }
}
