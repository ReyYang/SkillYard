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
    AgentConversationMessage, AgentMessageRole, AgentPageContext, AgentPageKind,
    AgentProviderEndpoints, AgentStreamEvent, AiProvider, ApplicationPaths, PlatformInfo,
    SecretStore, SecretStoreError, SkillYardApplication, UiIntent,
};
use tempfile::tempdir;

#[test]
fn openai_agent_streams_normalized_text_before_completion() {
    let sandbox = tempdir().expect("应创建 Agent 流式隔离目录");
    let home = sandbox.path().join("home");
    fs::create_dir_all(&home).expect("应创建空 home");
    let (endpoint, requests) = spawn_openai_stream_server();
    let application = SkillYardApplication::new_with_agent_dependencies(
        ApplicationPaths::for_home(sandbox.path().join("application-support/SkillYard"), home),
        PlatformInfo::supported_for_test(),
        Arc::new(FixtureSecretStore::default()),
        AgentProviderEndpoints::for_test(endpoint),
    );
    application
        .handle(UiIntent::SetAiConfiguration {
            enabled: true,
            disclosure_accepted: true,
            provider: AiProvider::OpenAi,
            model: "gpt-5.6-terra".to_owned(),
        })
        .expect("应保存 OpenAI 配置");
    application
        .handle(UiIntent::SaveAiApiKey {
            api_key: "skillyard-fixture-stream-key".to_owned(),
        })
        .expect("应保存 fixture Key");
    application
        .handle(UiIntent::TestAiConnection)
        .expect("应验证 OpenAI");
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应建立空 Inventory");

    let mut events = Vec::new();
    application
        .stream_agent(
            "request-1".to_owned(),
            AgentPageContext::Page {
                page: AgentPageKind::Inventory,
            },
            vec![AgentConversationMessage {
                role: AgentMessageRole::User,
                content: "解释一下本机 Skill".to_owned(),
            }],
            |event| {
                events.push(event);
                true
            },
        )
        .expect("流式回答应完成");

    assert_eq!(
        events,
        vec![
            AgentStreamEvent::Delta {
                text: "这是".to_owned(),
            },
            AgentStreamEvent::Delta {
                text: "流式回答。".to_owned(),
            },
            AgentStreamEvent::Completed {
                local_match_found: true,
                searched_public_web: false,
                search_results: Vec::new(),
            },
        ]
    );
    let requests = requests.recv().expect("应读取 Provider 请求");
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2]["stream"], true);
}

#[test]
fn glm_and_deepseek_stream_the_same_normalized_contract() {
    assert_provider_stream(
        AiProvider::Glm,
        "glm-4.7",
        AgentProviderEndpoints::for_glm_test,
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
                    "data: {\"choices\":[{\"delta\":{\"content\":\"{\\\"localMatchFound\\\":true,\\\"searchPublic\\\":false,\\\"reply\\\":\\\"GLM \"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"流式\\\"}\"},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: [DONE]\n\n"
                )
                .to_owned(),
            ),
        ],
        ["GLM ", "流式"],
    );
    assert_provider_stream(
        AiProvider::DeepSeek,
        "deepseek-v4-flash",
        AgentProviderEndpoints::for_deepseek_test,
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
                    "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"name\":\"skillyard_agent_answer\",\"input\":{}}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"localMatchFound\\\":true,\\\"searchPublic\\\":false,\\\"reply\\\":\\\"DeepSeek \"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"流式\\\"}\"}}\n\n",
                    "data: {\"type\":\"message_stop\"}\n\n",
                    "data: [DONE]\n\n"
                )
                .to_owned(),
            ),
        ],
        ["DeepSeek ", "流式"],
    );
}

#[test]
fn midstream_failure_preserves_delta_and_emits_failed_without_completion() {
    let sandbox = tempdir().expect("应创建 Agent 失败隔离目录");
    let home = sandbox.path().join("home");
    fs::create_dir_all(&home).expect("应创建空 home");
    let (endpoint, _) = spawn_stream_fixture_server(vec![
        (
            "application/json",
            r#"{"output":[{"content":[{"type":"output_text","text":"{\"status\":\"ok\"}"}]}]}"#
                .to_owned(),
        ),
        (
            "application/json",
            r#"{"output":[{"content":[{"type":"output_text","text":"SkillYard","annotations":[{"type":"url_citation","url":"https://github.com/ReyYang/SkillYard"}]}]}]}"#
                .to_owned(),
        ),
        (
            "text/event-stream",
            concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"localMatchFound\\\":true,\\\"searchPublic\\\":false,\\\"reply\\\":\\\"已经显示\"}\n\n",
                "data: invalid-json\n\n"
            )
            .to_owned(),
        ),
    ]);
    let application = configured_application(
        sandbox.path(),
        home,
        AiProvider::OpenAi,
        "gpt-5.6-terra",
        AgentProviderEndpoints::for_test(endpoint),
    );
    let mut events = Vec::new();

    application
        .stream_agent(
            "failure-request".to_owned(),
            AgentPageContext::Page {
                page: AgentPageKind::Inventory,
            },
            vec![AgentConversationMessage {
                role: AgentMessageRole::User,
                content: "触发中途失败".to_owned(),
            }],
            |event| {
                events.push(event);
                true
            },
        )
        .expect("Provider 失败应通过流事件返回");

    assert_eq!(
        events.first(),
        Some(&AgentStreamEvent::Delta {
            text: "已经显示".to_owned(),
        })
    );
    assert!(matches!(
        events.last(),
        Some(AgentStreamEvent::Failed { .. })
    ));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentStreamEvent::Completed { .. }))
    );
}

#[test]
fn closed_channel_stops_the_active_stream_without_a_failure_or_completion() {
    let sandbox = tempdir().expect("应创建 Agent 取消隔离目录");
    let home = sandbox.path().join("home");
    fs::create_dir_all(&home).expect("应创建空 home");
    let (endpoint, _) = spawn_openai_stream_server();
    let application = configured_application(
        sandbox.path(),
        home,
        AiProvider::OpenAi,
        "gpt-5.6-terra",
        AgentProviderEndpoints::for_test(endpoint),
    );
    let mut events = Vec::new();

    application
        .stream_agent(
            "cancel-request".to_owned(),
            AgentPageContext::Page {
                page: AgentPageKind::Inventory,
            },
            vec![AgentConversationMessage {
                role: AgentMessageRole::User,
                content: "关闭这个回答".to_owned(),
            }],
            |event| {
                events.push(event);
                false
            },
        )
        .expect("关闭 Channel 应作为正常取消结束");

    assert_eq!(
        events,
        vec![AgentStreamEvent::Delta {
            text: "这是".to_owned(),
        }]
    );
    application
        .cancel_agent("cancel-request")
        .expect("重复取消应保持幂等");
}

fn assert_provider_stream(
    provider: AiProvider,
    model: &str,
    endpoints: fn(String) -> AgentProviderEndpoints,
    responses: Vec<(&'static str, String)>,
    expected_deltas: [&str; 2],
) {
    let sandbox = tempdir().expect("应创建 Provider 流式隔离目录");
    let home = sandbox.path().join("home");
    fs::create_dir_all(&home).expect("应创建空 home");
    let (endpoint, requests) = spawn_stream_fixture_server(responses);
    let application =
        configured_application(sandbox.path(), home, provider, model, endpoints(endpoint));
    let mut events = Vec::new();

    application
        .stream_agent(
            "provider-stream".to_owned(),
            AgentPageContext::Page {
                page: AgentPageKind::Inventory,
            },
            vec![AgentConversationMessage {
                role: AgentMessageRole::User,
                content: "stream".to_owned(),
            }],
            |event| {
                events.push(event);
                true
            },
        )
        .expect("Provider 流式回答应完成");

    assert_eq!(
        events,
        vec![
            AgentStreamEvent::Delta {
                text: expected_deltas[0].to_owned(),
            },
            AgentStreamEvent::Delta {
                text: expected_deltas[1].to_owned(),
            },
            AgentStreamEvent::Completed {
                local_match_found: true,
                searched_public_web: false,
                search_results: Vec::new(),
            },
        ]
    );
    let requests = requests.recv().expect("应读取 Provider 请求");
    assert_eq!(requests[2]["stream"], true);
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
            api_key: "skillyard-fixture-stream-key".to_owned(),
        })
        .expect("应保存 fixture Key");
    application
        .handle(UiIntent::TestAiConnection)
        .expect("应验证 Provider");
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应建立空 Inventory");
    application
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

fn spawn_openai_stream_server() -> (String, mpsc::Receiver<Vec<Value>>) {
    spawn_stream_fixture_server(vec![
        (
            "application/json",
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"status\":\"ok\"}"}]}]}"#
                .to_owned(),
        ),
        (
            "application/json",
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"SkillYard","annotations":[{"type":"url_citation","url":"https://github.com/ReyYang/SkillYard"}]}]}]}"#
                .to_owned(),
        ),
        (
            "text/event-stream",
            concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"localMatchFound\\\":true,\\\"searchPublic\\\":false,\\\"reply\\\":\\\"这是\"}\n\n",
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"流式回答。\\\"}\"}\n\n",
                "data: {\"type\":\"response.completed\"}\n\n",
                "data: [DONE]\n\n"
            )
            .to_owned(),
        ),
    ])
}

fn spawn_stream_fixture_server(
    responses: Vec<(&'static str, String)>,
) -> (String, mpsc::Receiver<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("应启动 Agent Fake Server");
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
