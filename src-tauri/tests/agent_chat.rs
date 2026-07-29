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
    AgentConversationMessage, AgentMessageRole, AgentPageContext, AgentProviderEndpoints,
    AiProvider, ApplicationPaths, PlatformInfo, SecretStore, SecretStoreError,
    SkillYardApplication, UiIntent, UiOutcome,
};
use tempfile::tempdir;

#[test]
fn skill_detail_agent_resolves_stable_id_and_filters_local_sensitive_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/explain-me");
    fs::create_dir_all(&skill_root).expect("应创建 Skill fixture");
    let original = format!(
        "---\nname: explain-me\ndescription: Explain repository code\n---\n# Explain Me\nUse this Skill to explain code.\nContact: person@example.com\napi_key: fixture-secret\nLocal note: {}/private-note\n",
        home.display()
    );
    fs::write(skill_root.join("SKILL.md"), &original).expect("应写入 Skill fixture");
    fs::write(skill_root.join(".env"), "TOKEN=must-not-leave").expect("应写入被禁止的敏感 fixture");

    let secrets = Arc::new(FixtureSecretStore::default());
    let (endpoint, requests) = spawn_openai_agent_server();
    let application = SkillYardApplication::new_with_agent_dependencies(
        ApplicationPaths::for_home(data_root, home.clone()),
        PlatformInfo::supported_for_test(),
        secrets,
        AgentProviderEndpoints::for_test(endpoint),
    );
    application
        .handle(UiIntent::SetAiConfiguration {
            enabled: true,
            disclosure_accepted: true,
            provider: AiProvider::OpenAi,
            model: "gpt-5.6-terra".to_owned(),
        })
        .expect("应保存 AI 配置");
    application
        .handle(UiIntent::SaveAiApiKey {
            api_key: "skillyard-fixture-agent-key".to_owned(),
        })
        .expect("应保存 fixture Key");
    application
        .handle(UiIntent::TestAiConnection)
        .expect("应先验证当前 Provider");

    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::StartInitialScan)
        .expect("应扫描 Skill fixture")
    else {
        panic!("扫描后应返回 Inventory");
    };
    let inventory_id = entries
        .iter()
        .find(|entry| entry.skill_name == "explain-me")
        .expect("应发现目标 Skill")
        .id
        .clone();

    assert_eq!(
        application
            .handle(UiIntent::AskAgent {
                context: AgentPageContext::Skill {
                    inventory_id: inventory_id.clone(),
                },
                messages: vec![AgentConversationMessage {
                    role: AgentMessageRole::User,
                    content: "这个 Skill 是做什么的？".to_owned(),
                }],
            })
            .expect("Agent 应解释当前 Skill"),
        UiOutcome::AgentReply {
            reply: "这是一个用于解释代码的 Skill。".to_owned(),
        }
    );

    let recorded = requests.recv().expect("Fake Server 应返回请求记录");
    assert_eq!(recorded.len(), 3);
    let agent_request = &recorded[2];
    assert_eq!(agent_request["model"], "gpt-5.6-terra");
    assert!(
        agent_request.get("tools").is_none(),
        "解释 Skill 不能启用联网搜索"
    );
    let serialized = agent_request.to_string();
    assert!(serialized.contains("# Explain Me"));
    assert!(!serialized.contains("person@example.com"));
    assert!(!serialized.contains("fixture-secret"));
    assert!(!serialized.contains("must-not-leave"));
    assert!(!serialized.contains(home.to_string_lossy().as_ref()));
    assert_eq!(
        fs::read_to_string(skill_root.join("SKILL.md")).expect("应重新读取原文件"),
        original,
        "只读 Agent 不能修改 Skill"
    );

    let error = application
        .handle(UiIntent::AskAgent {
            context: AgentPageContext::Skill {
                inventory_id: "unknown-inventory-id".to_owned(),
            },
            messages: vec![AgentConversationMessage {
                role: AgentMessageRole::User,
                content: "解释它".to_owned(),
            }],
        })
        .expect_err("前端不能用不存在的稳定 ID 读取路径");
    assert_eq!(error.to_string(), "当前页面对应的 Skill 已不存在");
}

#[test]
fn glm_and_deepseek_use_the_same_read_only_conversation_contract() {
    assert_provider_chat(
        AiProvider::Glm,
        "glm-4.7",
        AgentProviderEndpoints::for_glm_test,
        "/api/paas/v4",
        [
            r#"{"choices":[{"message":{"content":"{\"status\":\"ok\"}"}}]}"#,
            r#"{"choices":[{"message":{"content":"SkillYard"}}],"web_search":[{"title":"SkillYard","link":"https://github.com/ReyYang/SkillYard"}]}"#,
            r#"{"choices":[{"message":{"content":"fixture answer"}}]}"#,
        ],
    );
    assert_provider_chat(
        AiProvider::DeepSeek,
        "deepseek-v4-flash",
        AgentProviderEndpoints::for_deepseek_test,
        "/anthropic",
        [
            r#"{"content":[{"type":"tool_use","id":"tool_1","name":"skillyard_connection_test","input":{"status":"ok"}}]}"#,
            r#"{"content":[{"type":"web_search_tool_result","tool_use_id":"srv_1","content":[{"type":"web_search_result","url":"https://github.com/ReyYang/SkillYard","title":"SkillYard"}]}]}"#,
            r#"{"content":[{"type":"text","text":"fixture answer"}]}"#,
        ],
    );
}

fn assert_provider_chat(
    provider: AiProvider,
    model: &str,
    endpoints: fn(String) -> AgentProviderEndpoints,
    base_path: &str,
    responses: [&'static str; 3],
) {
    let sandbox = tempdir().expect("应创建 Provider 隔离目录");
    let home = sandbox.path().join("home");
    let skill_root = home.join(".codex/skills/provider-fixture");
    fs::create_dir_all(&skill_root).expect("应创建 Provider Skill fixture");
    fs::write(
        skill_root.join("SKILL.md"),
        "---\nname: provider-fixture\ndescription: fixture\n---\n# Provider Fixture\n",
    )
    .expect("应写入 Provider Skill fixture");
    let (endpoint, requests) = spawn_agent_server(base_path, responses);
    let application = SkillYardApplication::new_with_agent_dependencies(
        ApplicationPaths::for_home(sandbox.path().join("application-support/SkillYard"), home),
        PlatformInfo::supported_for_test(),
        Arc::new(FixtureSecretStore::default()),
        endpoints(endpoint),
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
            api_key: "skillyard-fixture-provider-key".to_owned(),
        })
        .expect("应保存 Provider fixture Key");
    application
        .handle(UiIntent::TestAiConnection)
        .expect("应验证 Provider");
    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::StartInitialScan)
        .expect("应扫描 Provider Skill")
    else {
        panic!("扫描后应返回 Inventory");
    };
    let inventory_id = entries.first().expect("应发现 Provider Skill").id.clone();

    assert_eq!(
        application
            .handle(UiIntent::AskAgent {
                context: AgentPageContext::Skill { inventory_id },
                messages: vec![AgentConversationMessage {
                    role: AgentMessageRole::User,
                    content: "explain".to_owned(),
                }],
            })
            .expect("当前 Provider 应返回对话回答"),
        UiOutcome::AgentReply {
            reply: "fixture answer".to_owned(),
        }
    );
    let recorded = requests.recv().expect("应读取 Provider 请求");
    assert_eq!(recorded.len(), 3);
    assert!(
        recorded[2].get("tools").is_none(),
        "普通解释路径不能启用 Provider Web Search"
    );
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

fn spawn_openai_agent_server() -> (String, mpsc::Receiver<Vec<Value>>) {
    spawn_agent_server(
        "/v1",
        [
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"status\":\"ok\"}"}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"SkillYard","annotations":[{"type":"url_citation","url":"https://github.com/ReyYang/SkillYard"}]}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"这是一个用于解释代码的 Skill。"}]}]}"#,
        ],
    )
}

fn spawn_agent_server(
    base_path: &str,
    responses: [&'static str; 3],
) -> (String, mpsc::Receiver<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("应启动 Agent Fake Server");
    let address = listener.local_addr().expect("应读取 Fake Server 地址");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut recorded = Vec::new();
        for response in responses {
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
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            stream
                .write_all(reply.as_bytes())
                .expect("应写入 Provider fixture 响应");
        }
        sender.send(recorded).expect("应返回 Provider 请求记录");
    });
    (format!("http://{address}{base_path}"), receiver)
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
