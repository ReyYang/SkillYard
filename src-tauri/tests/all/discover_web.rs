use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use serde_json::Value;
use skillyard_lib::{
    AgentConversationMessage, AgentMessageRole, AgentPageContext, AgentPageKind,
    AgentProviderEndpoints, AgentStreamEvent, AiProvider, ApplicationPaths, DiscoverWebResult,
    DiscoverWebResultKind, PlatformInfo, SecretStore, SecretStoreError, SkillYardApplication,
    UiIntent, UiOutcome,
};
use tempfile::tempdir;

const FIXTURE_API_KEY: &str = "skillyard-fixture-discover-key";
const OPENAI_SECRET_ACCOUNT: &str = "provider-openai";

#[test]
fn discover_reuses_saved_configuration_and_secret_boundary_without_persisting_or_reranking_results()
{
    let sandbox = tempdir().expect("应创建发现搜索隔离目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    write_skill(&home.join(".codex/skills/tdd"), "tdd", "测试驱动开发工作流");
    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let secrets = Arc::new(RecordingSecretStore::default());
    let responses = openai_responses_with_agent_answer();
    let (endpoint, requests) = spawn_provider_server(responses);
    let application = configured_application_with_secret_store(
        paths.clone(),
        AiProvider::OpenAi,
        "gpt-5.6-terra",
        secrets.clone(),
        AgentProviderEndpoints::for_test(endpoint),
    );
    let UiOutcome::Preferences { ai, .. } = application
        .handle(UiIntent::GetPreferences)
        .expect("应回读已保存的 AI 配置")
    else {
        panic!("应返回包含当前 Provider 与模型的偏好");
    };
    assert_eq!(ai.provider, AiProvider::OpenAi);
    assert_eq!(ai.model, "gpt-5.6-terra");

    let state_before = application
        .handle(UiIntent::GetStartupState)
        .expect("应读取搜索前状态");
    let persisted_before = persisted_state(&data_root);
    let setup_reads = secrets.take_read_accounts();
    assert!(
        !setup_reads.is_empty()
            && setup_reads
                .iter()
                .all(|account| account == OPENAI_SECRET_ACCOUNT),
        "保存、连接测试与偏好回读都必须只经过当前 Provider 的 SecretStore account"
    );

    let mut agent_events = Vec::new();
    application
        .stream_agent(
            "discover-shared-configuration".to_owned(),
            AgentPageContext::Page {
                page: AgentPageKind::Discover,
            },
            vec![AgentConversationMessage {
                role: AgentMessageRole::User,
                content: "解释当前发现页".to_owned(),
            }],
            |event| {
                agent_events.push(event);
                true
            },
        )
        .expect("全局 Agent 应使用已验证的当前配置回答");
    assert!(matches!(
        agent_events.last(),
        Some(AgentStreamEvent::Completed {
            searched_public_web: false,
            ..
        })
    ));

    let UiOutcome::DiscoverWebSearch { query, results } = application
        .handle(UiIntent::SearchDiscoverWeb {
            query: "测试驱动开发".to_owned(),
        })
        .expect("主动提交必须执行独立的全网发现")
    else {
        panic!("应返回结构化全网发现结果");
    };

    assert_eq!(query, "测试驱动开发");
    assert_canonical_provider_order(&results);
    assert_eq!(
        persisted_state(&data_root),
        persisted_before,
        "Discover 与 Agent 请求都不能写入可跨启动恢复的 SQLite 或本地状态"
    );
    assert_eq!(
        secrets.take_read_accounts(),
        vec![OPENAI_SECRET_ACCOUNT, OPENAI_SECRET_ACCOUNT],
        "全局 Agent 与 Discover 必须各自从同一已保存 SecretStore account 读取当前 Key"
    );

    drop(application);
    let restarted = SkillYardApplication::new_with_agent_dependencies(
        paths,
        PlatformInfo::supported_for_test(),
        secrets.clone(),
        AgentProviderEndpoints::for_test("http://127.0.0.1:9/v1".to_owned()),
    );
    assert_eq!(
        restarted
            .handle(UiIntent::GetStartupState)
            .expect("重启后应只恢复原有本地状态"),
        state_before,
        "Discover 结果和临时 Agent 对话不能在重启后变成应用状态"
    );

    let recorded = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("Fake Server 应在本地请求完成后返回记录");
    assert_eq!(
        recorded.len(),
        4,
        "连接测试固定两次、普通 Agent 一次、Discover 一次；不能 retry 或 fallback"
    );
    let expected_authorization = format!("Bearer {FIXTURE_API_KEY}");
    for request in &recorded {
        assert_eq!(request.body["model"], "gpt-5.6-terra");
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some(expected_authorization.as_str()),
            "连接测试、Agent 与 Discover 都必须使用同一已保存 Key"
        );
    }
    assert_eq!(recorded[0].body["text"]["format"]["type"], "json_schema");
    assert!(request_uses_web_search(&recorded[1].body));
    assert!(!request_uses_web_search(&recorded[2].body));
    assert_eq!(recorded[3].body["input"], "测试驱动开发");
    assert!(request_uses_web_search(&recorded[3].body));
    assert_eq!(secrets.write_accounts(), vec![OPENAI_SECRET_ACCOUNT]);
}

#[test]
fn glm_and_deepseek_return_the_same_stateless_discover_contract() {
    assert_provider_search(
        AiProvider::Glm,
        "glm-4.7",
        AgentProviderEndpoints::for_glm_test,
        glm_responses(),
    );
    assert_provider_search(
        AiProvider::DeepSeek,
        "deepseek-v4-flash",
        AgentProviderEndpoints::for_deepseek_test,
        deepseek_responses(),
    );
}

fn assert_provider_search(
    provider: AiProvider,
    model: &str,
    endpoints: fn(String) -> AgentProviderEndpoints,
    responses: Vec<(&'static str, String)>,
) {
    let sandbox = tempdir().expect("应创建 Provider 发现隔离目录");
    let home = sandbox.path().join("home");
    fs::create_dir_all(&home).expect("应创建空 home");
    let (endpoint, requests) = spawn_provider_server(responses);
    let application =
        configured_application(sandbox.path(), home, provider, model, endpoints(endpoint));

    let UiOutcome::DiscoverWebSearch { results, .. } = application
        .handle(UiIntent::SearchDiscoverWeb {
            query: "代码审查".to_owned(),
        })
        .expect("当前 Provider 应执行原生全网发现")
    else {
        panic!("应返回统一的发现结果");
    };

    assert_canonical_provider_order(&results);
    assert!(
        results
            .iter()
            .all(|result| result.url.starts_with("https://")),
        "全网发现只能展示 Provider 返回的可核验 URL"
    );
    let recorded = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("Fake Server 应在本地请求完成后返回记录");
    assert_eq!(recorded.len(), 3);
    assert!(request_uses_web_search(&recorded[2].body));
}

fn configured_application(
    sandbox: &std::path::Path,
    home: std::path::PathBuf,
    provider: AiProvider,
    model: &str,
    endpoints: AgentProviderEndpoints,
) -> SkillYardApplication {
    configured_application_with_secret_store(
        ApplicationPaths::for_home(sandbox.join("application-support/SkillYard"), home),
        provider,
        model,
        Arc::new(RecordingSecretStore::default()),
        endpoints,
    )
}

fn configured_application_with_secret_store(
    paths: ApplicationPaths,
    provider: AiProvider,
    model: &str,
    secret_store: Arc<dyn SecretStore>,
    endpoints: AgentProviderEndpoints,
) -> SkillYardApplication {
    let application = SkillYardApplication::new_with_agent_dependencies(
        paths,
        PlatformInfo::supported_for_test(),
        secret_store,
        endpoints,
    );
    application
        .handle(UiIntent::SetAiConfiguration {
            enabled: true,
            disclosure_accepted: true,
            provider,
            model: model.to_owned(),
        })
        .expect("应保存 Provider 配置");
    application
        .handle(UiIntent::SaveAiApiKey {
            api_key: FIXTURE_API_KEY.to_owned(),
        })
        .expect("应保存 fixture Key");
    application
        .handle(UiIntent::TestAiConnection)
        .expect("应先验证 Provider");
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应建立 Inventory");
    application
}

fn assert_canonical_provider_order(results: &[DiscoverWebResult]) {
    assert_eq!(results.len(), 3, "同一 GitHub 仓库的多个引用只能保留一个");
    assert_eq!(
        results
            .iter()
            .map(|result| result.url.as_str())
            .collect::<Vec<_>>(),
        vec![
            "https://forum.example.com/review-skills",
            "https://github.com/acme/review-skills/tree/main/skills/tdd",
            "https://downloads.example.com/review-skills.zip",
        ],
        "canonical dedupe 只能删除重复项，不能按本地规则重排 Provider 返回顺序"
    );
    assert_eq!(results[0].kind, DiscoverWebResultKind::Reference);
    assert_eq!(results[0].canonical_identity, None);
    assert_eq!(results[1].kind, DiscoverWebResultKind::Github);
    assert_eq!(
        results[1].canonical_identity.as_deref(),
        Some("github:acme/review-skills")
    );
    assert_eq!(results[2].kind, DiscoverWebResultKind::DirectUrl);
    assert_eq!(
        results[2].canonical_identity.as_deref(),
        Some("direct-url:https://downloads.example.com/review-skills.zip")
    );
}

fn request_uses_web_search(request: &Value) -> bool {
    match request {
        Value::String(value) => value == "web_search" || value == "web_search_20250305",
        Value::Array(values) => values.iter().any(request_uses_web_search),
        Value::Object(values) => values
            .iter()
            .any(|(key, value)| key == "web_search" || request_uses_web_search(value)),
        _ => false,
    }
}

fn persisted_state(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    collect_persisted_files(root, root, &mut files);
    files
}

fn collect_persisted_files(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(current).expect("应读取测试持久化目录") {
        let entry = entry.expect("持久化目录项应可读取");
        let path = entry.path();
        let file_type = entry.file_type().expect("应读取持久化目录项类型");
        if file_type.is_dir() {
            collect_persisted_files(root, &path, files);
        } else if file_type.is_file() {
            files.insert(
                path.strip_prefix(root)
                    .expect("持久化文件应位于应用目录内")
                    .to_path_buf(),
                fs::read(path).expect("应读取持久化文件"),
            );
        }
    }
}

fn write_skill(path: &std::path::Path, name: &str, description: &str) {
    fs::create_dir_all(path).expect("应创建 Skill fixture");
    fs::write(
        path.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
    )
    .expect("应写入 Skill fixture");
}

#[derive(Default)]
struct RecordingSecretStore {
    values: Mutex<BTreeMap<String, String>>,
    read_accounts: Mutex<Vec<String>>,
    write_accounts: Mutex<Vec<String>>,
}

impl RecordingSecretStore {
    fn take_read_accounts(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .read_accounts
                .lock()
                .expect("fixture Key store 不应中毒"),
        )
    }

    fn write_accounts(&self) -> Vec<String> {
        self.write_accounts
            .lock()
            .expect("fixture Key store 不应中毒")
            .clone()
    }
}

impl SecretStore for RecordingSecretStore {
    fn read(&self, account: &str) -> Result<Option<String>, SecretStoreError> {
        self.read_accounts
            .lock()
            .expect("fixture Key store 不应中毒")
            .push(account.to_owned());
        Ok(self
            .values
            .lock()
            .expect("fixture Key store 不应中毒")
            .get(account)
            .cloned())
    }

    fn write(&self, account: &str, value: &str) -> Result<(), SecretStoreError> {
        self.write_accounts
            .lock()
            .expect("fixture Key store 不应中毒")
            .push(account.to_owned());
        self.values
            .lock()
            .expect("fixture Key store 不应中毒")
            .insert(account.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretStoreError> {
        self.values
            .lock()
            .expect("fixture Key store 不应中毒")
            .remove(account);
        Ok(())
    }
}

fn openai_responses() -> Vec<(&'static str, String)> {
    // 前两次连接验证仍是 JSON；第三次发现请求必须按生产合同返回 SSE。
    vec![
        (
            "application/json",
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"status\":\"ok\"}"}]}]}"#.to_owned(),
        ),
        (
            "application/json",
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"SkillYard","annotations":[{"type":"url_citation","url":"https://github.com/ReyYang/SkillYard"}]}]}]}"#.to_owned(),
        ),
        (
            "text/event-stream",
            concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"找到公开结果。\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"找到公开结果。\",\"annotations\":[{\"type\":\"url_citation\",\"url\":\"https://forum.example.com/review-skills\",\"title\":\"Forum\"},{\"type\":\"url_citation\",\"url\":\"https://github.com/acme/review-skills/tree/main/skills/tdd\",\"title\":\"Review Skills TDD\"},{\"type\":\"url_citation\",\"url\":\"https://downloads.example.com/review-skills.zip\",\"title\":\"Review Skills ZIP\"},{\"type\":\"url_citation\",\"url\":\"https://github.com/acme/review-skills\",\"title\":\"Review Skills\"}]}]}]}}\n\n",
                "data: [DONE]\n\n"
            )
            .to_owned(),
        ),
    ]
}

fn openai_responses_with_agent_answer() -> Vec<(&'static str, String)> {
    let mut responses = openai_responses();
    let public_search = responses.pop().expect("fixture 应包含 Discover 响应");
    responses.push((
        "text/event-stream",
        concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"localMatchFound\\\":true,\\\"searchPublic\\\":false,\\\"reply\\\":\\\"Agent fixture\\\"}\"}\n\n",
            "data: [DONE]\n\n"
        )
        .to_owned(),
    ));
    responses.push(public_search);
    responses
}

fn glm_responses() -> Vec<(&'static str, String)> {
    vec![
        (
            "application/json",
            r#"{"choices":[{"message":{"content":"{\"status\":\"ok\"}"}}]}"#.to_owned(),
        ),
        (
            "application/json",
            r#"{"choices":[{"message":{"content":"SkillYard"}}],"web_search":[{"title":"SkillYard","link":"https://github.com/ReyYang/SkillYard"}]}"#.to_owned(),
        ),
        (
            "text/event-stream",
            concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"找到公开结果。\"}}],\"web_search\":[{\"title\":\"Forum\",\"link\":\"https://forum.example.com/review-skills\"},{\"title\":\"Review Skills TDD\",\"link\":\"https://github.com/acme/review-skills/tree/main/skills/tdd\"},{\"title\":\"Review Skills ZIP\",\"link\":\"https://downloads.example.com/review-skills.zip\"},{\"title\":\"Review Skills\",\"link\":\"https://github.com/acme/review-skills\"}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            )
            .to_owned(),
        ),
    ]
}

fn deepseek_responses() -> Vec<(&'static str, String)> {
    vec![
        (
            "application/json",
            r#"{"content":[{"type":"tool_use","name":"skillyard_connection_test","input":{"status":"ok"}}]}"#.to_owned(),
        ),
        (
            "application/json",
            r#"{"content":[{"type":"web_search_tool_result","content":[{"type":"web_search_result","url":"https://github.com/ReyYang/SkillYard","title":"SkillYard"}]}]}"#.to_owned(),
        ),
        (
            "text/event-stream",
            concat!(
                "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"找到公开结果。\"}}\n\n",
                "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"web_search_tool_result\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://forum.example.com/review-skills\",\"title\":\"Forum\"},{\"type\":\"web_search_result\",\"url\":\"https://github.com/acme/review-skills/tree/main/skills/tdd\",\"title\":\"Review Skills TDD\"},{\"type\":\"web_search_result\",\"url\":\"https://downloads.example.com/review-skills.zip\",\"title\":\"Review Skills ZIP\"},{\"type\":\"web_search_result\",\"url\":\"https://github.com/acme/review-skills\",\"title\":\"Review Skills\"}]}}\n\n",
                "data: {\"type\":\"message_stop\"}\n\n",
                "data: [DONE]\n\n"
            )
            .to_owned(),
        ),
    ]
}

#[derive(Debug)]
struct RecordedProviderRequest {
    headers: BTreeMap<String, String>,
    body: Value,
}

fn spawn_provider_server(
    responses: Vec<(&'static str, String)>,
) -> (String, mpsc::Receiver<Vec<RecordedProviderRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("应启动 Provider Fake Server");
    let address = listener.local_addr().expect("应读取 Fake Server 地址");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut recorded = Vec::new();
        for (content_type, response) in responses {
            let (mut stream, _) = listener.accept().expect("应接收 Provider 请求");
            let request = read_http_request(&mut stream);
            recorded.push(record_provider_request(&request));
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                response.len(),
            );
            stream
                .write_all(reply.as_bytes())
                .expect("应写入 Provider fixture 响应");
        }
        sender.send(recorded).expect("应返回 Provider 请求记录");
    });
    (format!("http://{address}/v1"), receiver)
}

fn record_provider_request(request: &[u8]) -> RecordedProviderRequest {
    let body_start = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .expect("请求应包含 HTTP body 分隔符");
    let headers = String::from_utf8_lossy(&request[..body_start - 4])
        .lines()
        .skip(1)
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect();
    RecordedProviderRequest {
        headers,
        body: serde_json::from_slice(&request[body_start..]).expect("请求应包含 JSON body"),
    }
}

fn read_http_request(stream: &mut impl Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).expect("应读取 HTTP 请求");
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(str::trim)
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap_or(0);
        if bytes.len() >= header_end + 4 + content_length {
            break;
        }
    }
    bytes
}
