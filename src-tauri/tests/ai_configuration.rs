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
    AgentProviderEndpoints, AiPreferences, AiProvider, ApplicationPaths, InterfaceLanguage,
    PlatformInfo, SecretStore, SecretStoreError, SkillYardApplication, UiIntent, UiOutcome,
};
use tempfile::tempdir;

const FIXTURE_API_KEY: &str = "skillyard-fixture-openai-api-key";

#[test]
fn openai_configuration_uses_keychain_boundary_and_only_verifies_on_request() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let secrets = Arc::new(FixtureSecretStore::default());
    let (endpoint, requests) = spawn_openai_verification_server();
    let application = SkillYardApplication::new_with_agent_dependencies(
        paths.clone(),
        PlatformInfo::supported_for_test(),
        secrets.clone(),
        AgentProviderEndpoints::for_test(endpoint),
    );

    assert_eq!(
        application
            .handle(UiIntent::GetPreferences)
            .expect("首次读取偏好应成功"),
        preferences(AiPreferences {
            enabled: false,
            disclosure_accepted: false,
            provider: AiProvider::OpenAi,
            model: "gpt-5.6-terra".to_owned(),
            has_api_key: false,
            verified: false,
        })
    );

    assert_eq!(
        application
            .handle(UiIntent::SetAiConfiguration {
                enabled: true,
                disclosure_accepted: true,
                provider: AiProvider::OpenAi,
                model: "gpt-5.6-terra".to_owned(),
            })
            .expect("应保存 OpenAI 配置"),
        preferences(AiPreferences {
            enabled: true,
            disclosure_accepted: true,
            provider: AiProvider::OpenAi,
            model: "gpt-5.6-terra".to_owned(),
            has_api_key: false,
            verified: false,
        })
    );
    application
        .handle(UiIntent::SaveAiApiKey {
            api_key: FIXTURE_API_KEY.to_owned(),
        })
        .expect("应通过 SecretStore 保存 API Key");

    let database = fs::read(data_root.join("skillyard.sqlite3")).expect("应读取测试 SQLite");
    assert!(
        !database
            .windows(FIXTURE_API_KEY.len())
            .any(|window| window == FIXTURE_API_KEY.as_bytes()),
        "API Key 不能写入 SQLite"
    );
    assert!(
        requests.try_recv().is_err(),
        "保存配置或 API Key 时不能自动发起验证"
    );

    assert_eq!(
        application
            .handle(UiIntent::TestAiConnection)
            .expect("两项 OpenAI 能力都成功时应完成验证"),
        preferences(AiPreferences {
            enabled: true,
            disclosure_accepted: true,
            provider: AiProvider::OpenAi,
            model: "gpt-5.6-terra".to_owned(),
            has_api_key: true,
            verified: true,
        })
    );

    let recorded = requests.recv().expect("Fake Server 应返回记录");
    assert_eq!(recorded.len(), 2);
    assert_eq!(
        recorded[0].headers.get("authorization"),
        Some(&format!("Bearer {FIXTURE_API_KEY}"))
    );
    assert_eq!(recorded[0].body["model"], "gpt-5.6-terra");
    assert_eq!(recorded[0].body["text"]["format"]["type"], "json_schema");
    assert_eq!(recorded[1].body["tools"][0]["type"], "web_search");

    assert_eq!(
        application
            .handle(UiIntent::SaveAiApiKey {
                api_key: "skillyard-fixture-replaced-openai-key".to_owned(),
            })
            .expect("替换 API Key 应成功"),
        preferences(AiPreferences {
            enabled: true,
            disclosure_accepted: true,
            provider: AiProvider::OpenAi,
            model: "gpt-5.6-terra".to_owned(),
            has_api_key: true,
            verified: false,
        })
    );

    assert_eq!(
        application
            .handle(UiIntent::SetAiConfiguration {
                enabled: true,
                disclosure_accepted: true,
                provider: AiProvider::OpenAi,
                model: "gpt-5.4-mini".to_owned(),
            })
            .expect("切换到静态候选模型应成功"),
        preferences(AiPreferences {
            enabled: true,
            disclosure_accepted: true,
            provider: AiProvider::OpenAi,
            model: "gpt-5.4-mini".to_owned(),
            has_api_key: true,
            verified: false,
        })
    );

    let reopened = SkillYardApplication::new_with_agent_dependencies(
        paths,
        PlatformInfo::supported_for_test(),
        secrets,
        AgentProviderEndpoints::for_test("http://127.0.0.1:9/v1".to_owned()),
    );
    assert_eq!(
        reopened
            .handle(UiIntent::GetPreferences)
            .expect("重启后应恢复非敏感配置"),
        preferences(AiPreferences {
            enabled: true,
            disclosure_accepted: true,
            provider: AiProvider::OpenAi,
            model: "gpt-5.4-mini".to_owned(),
            has_api_key: true,
            verified: false,
        })
    );
}

fn preferences(ai: AiPreferences) -> UiOutcome {
    UiOutcome::Preferences {
        language: InterfaceLanguage::ZhCn,
        ai,
    }
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
            .expect("fixture secret lock 应可用")
            .get(account)
            .cloned())
    }

    fn write(&self, account: &str, value: &str) -> Result<(), SecretStoreError> {
        self.values
            .lock()
            .expect("fixture secret lock 应可用")
            .insert(account.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretStoreError> {
        self.values
            .lock()
            .expect("fixture secret lock 应可用")
            .remove(account);
        Ok(())
    }
}

#[derive(Debug)]
struct RecordedRequest {
    headers: BTreeMap<String, String>,
    body: Value,
}

fn spawn_openai_verification_server() -> (String, mpsc::Receiver<Vec<RecordedRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("应启动 Fake Server");
    let address = listener.local_addr().expect("应读取 Fake Server 地址");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let responses = [
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"status\":\"ok\"}"}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"SkillYard","annotations":[{"type":"url_citation","url":"https://github.com/ReyYang/SkillYard"}]}]}]}"#,
        ];
        let mut recorded = Vec::new();
        for response in responses {
            let (mut stream, _) = listener.accept().expect("应接收 Fake Server 请求");
            let raw = read_http_request(&mut stream);
            let (headers, body) = parse_http_request(&raw);
            recorded.push(RecordedRequest { headers, body });
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            stream
                .write_all(reply.as_bytes())
                .expect("应写入 Fake Server 响应");
        }
        sender.send(recorded).expect("应返回请求记录");
    });
    (format!("http://{address}/v1"), receiver)
}

fn read_http_request(stream: &mut impl Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).expect("应读取 HTTP 请求");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(header_end) = find_bytes(&bytes, b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                return bytes;
            }
        }
    }
}

fn parse_http_request(raw: &[u8]) -> (BTreeMap<String, String>, Value) {
    let header_end = find_bytes(raw, b"\r\n\r\n").expect("请求应包含 header");
    let headers = String::from_utf8_lossy(&raw[..header_end])
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let body = serde_json::from_slice(&raw[header_end + 4..]).expect("请求 body 应为 JSON");
    (headers, body)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
