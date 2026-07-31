use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use serde_json::Value;
use skillyard_lib::{
    AgentProviderEndpoints, AiProvider, ApplicationPaths, DiscoverWebResultKind, PlatformInfo,
    SecretStore, SecretStoreError, SkillYardApplication, UiIntent, UiOutcome,
};
use tempfile::tempdir;

#[test]
fn discover_submit_searches_openai_even_when_a_local_skill_matches_without_writing_state() {
    let sandbox = tempdir().expect("应创建发现搜索隔离目录");
    let home = sandbox.path().join("home");
    write_skill(&home.join(".codex/skills/tdd"), "tdd", "测试驱动开发工作流");
    let responses = openai_responses();
    let (endpoint, requests) = spawn_provider_server(responses);
    let application = configured_application(
        sandbox.path(),
        home,
        AiProvider::OpenAi,
        "gpt-5.6-terra",
        AgentProviderEndpoints::for_test(endpoint),
    );
    let before = application
        .handle(UiIntent::GetStartupState)
        .expect("应读取搜索前状态");

    let UiOutcome::DiscoverWebSearch { query, results } = application
        .handle(UiIntent::SearchDiscoverWeb {
            query: "测试驱动开发".to_owned(),
        })
        .expect("主动提交必须执行独立的全网发现")
    else {
        panic!("应返回结构化全网发现结果");
    };

    assert_eq!(query, "测试驱动开发");
    assert_eq!(results.len(), 3, "同一 GitHub 仓库的多个引用只能保留一个");
    assert_eq!(
        results[0].canonical_identity.as_deref(),
        Some("github:acme/review-skills")
    );
    assert_eq!(results[0].kind, DiscoverWebResultKind::Github);
    assert_eq!(results[1].kind, DiscoverWebResultKind::DirectUrl);
    assert_eq!(
        results[1].canonical_identity.as_deref(),
        Some("direct-url:https://downloads.example.com/review-skills.zip")
    );
    assert_eq!(results[2].kind, DiscoverWebResultKind::Reference);
    assert_eq!(
        application
            .handle(UiIntent::GetStartupState)
            .expect("应读取搜索后状态"),
        before,
        "发现搜索不能写入 Agent Session、Inventory 或生命周期状态"
    );

    let recorded = requests.recv().expect("应读取 OpenAI 请求");
    assert_eq!(recorded.len(), 3);
    assert_eq!(recorded[2]["input"], "测试驱动开发");
    assert_eq!(recorded[2]["tools"][0]["type"], "web_search");
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

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].kind, DiscoverWebResultKind::Github);
    assert_eq!(results[1].kind, DiscoverWebResultKind::DirectUrl);
    assert_eq!(results[2].kind, DiscoverWebResultKind::Reference);
    assert!(
        results
            .iter()
            .all(|result| result.url.starts_with("https://")),
        "全网发现只能展示 Provider 返回的可核验 URL"
    );
    let recorded = requests.recv().expect("应读取 Provider 请求");
    assert_eq!(recorded.len(), 3);
    assert!(
        recorded[2].to_string().contains("web_search"),
        "发现提交必须使用当前 Provider 的原生 Web Search"
    );
}

fn configured_application(
    sandbox: &std::path::Path,
    home: std::path::PathBuf,
    provider: AiProvider,
    model: &str,
    endpoints: AgentProviderEndpoints,
) -> SkillYardApplication {
    let application = SkillYardApplication::new_with_agent_dependencies(
        ApplicationPaths::for_home(sandbox.join("application-support/SkillYard"), home),
        PlatformInfo::supported_for_test(),
        Arc::new(FixtureSecretStore::default()),
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
            api_key: "skillyard-fixture-discover-key".to_owned(),
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

fn write_skill(path: &std::path::Path, name: &str, description: &str) {
    fs::create_dir_all(path).expect("应创建 Skill fixture");
    fs::write(
        path.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
    )
    .expect("应写入 Skill fixture");
}

#[derive(Default)]
struct FixtureSecretStore {
    values: Mutex<BTreeMap<String, String>>,
}

impl SecretStore for FixtureSecretStore {
    fn read(&self, account: &str) -> Result<Option<String>, SecretStoreError> {
        Ok(self
            .values
            .lock()
            .expect("fixture Key store 不应中毒")
            .get(account)
            .cloned())
    }

    fn write(&self, account: &str, value: &str) -> Result<(), SecretStoreError> {
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
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"找到公开结果。\",\"annotations\":[{\"type\":\"url_citation\",\"url\":\"https://github.com/acme/review-skills\",\"title\":\"Review Skills\"},{\"type\":\"url_citation\",\"url\":\"https://github.com/acme/review-skills/tree/main/skills/tdd\",\"title\":\"Review Skills TDD\"},{\"type\":\"url_citation\",\"url\":\"https://downloads.example.com/review-skills.zip\",\"title\":\"Review Skills ZIP\"},{\"type\":\"url_citation\",\"url\":\"https://forum.example.com/review-skills\",\"title\":\"Forum\"}]}]}]}}\n\n",
                "data: [DONE]\n\n"
            )
            .to_owned(),
        ),
    ]
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
                "data: {\"choices\":[{\"delta\":{\"content\":\"找到公开结果。\"}}],\"web_search\":[{\"title\":\"Review Skills\",\"link\":\"https://github.com/acme/review-skills\"},{\"title\":\"Review Skills TDD\",\"link\":\"https://github.com/acme/review-skills/tree/main/skills/tdd\"},{\"title\":\"Review Skills ZIP\",\"link\":\"https://downloads.example.com/review-skills.zip\"},{\"title\":\"Forum\",\"link\":\"https://forum.example.com/review-skills\"}]}\n\n",
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
                "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"web_search_tool_result\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://github.com/acme/review-skills\",\"title\":\"Review Skills\"},{\"type\":\"web_search_result\",\"url\":\"https://github.com/acme/review-skills/tree/main/skills/tdd\",\"title\":\"Review Skills TDD\"},{\"type\":\"web_search_result\",\"url\":\"https://downloads.example.com/review-skills.zip\",\"title\":\"Review Skills ZIP\"},{\"type\":\"web_search_result\",\"url\":\"https://forum.example.com/review-skills\",\"title\":\"Forum\"}]}}\n\n",
                "data: {\"type\":\"message_stop\"}\n\n",
                "data: [DONE]\n\n"
            )
            .to_owned(),
        ),
    ]
}

fn spawn_provider_server(
    responses: Vec<(&'static str, String)>,
) -> (String, mpsc::Receiver<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("应启动 Provider Fake Server");
    let address = listener.local_addr().expect("应读取 Fake Server 地址");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut recorded = Vec::new();
        for (content_type, response) in responses {
            let (mut stream, _) = listener.accept().expect("应接收 Provider 请求");
            let request = read_http_request(&mut stream);
            let body_start = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
                .expect("请求应包含 HTTP body 分隔符");
            recorded.push(
                serde_json::from_slice(&request[body_start..]).expect("请求应包含 JSON body"),
            );
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
