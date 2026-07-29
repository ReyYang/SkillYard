use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use rusqlite::Connection;
use serde_json::Value;
use skillyard_lib::{
    AgentProviderEndpoints, AiProvider, ApplicationPaths, InterfaceLanguage, PlatformInfo,
    SecretStore, SecretStoreError, SkillCategory, SkillYardApplication, UiIntent, UiOutcome,
};
use tempfile::tempdir;

#[test]
fn user_generated_skill_explanation_replaces_one_persisted_result_without_web_search() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/explain-me");
    fs::create_dir_all(&skill_root).expect("应创建 Skill fixture");
    let original = "---\nname: explain-me\ndescription: Review Rust code\n---\n# Explain Me\nReview Rust code and report risks.\n";
    fs::write(skill_root.join("SKILL.md"), original).expect("应写入 Skill fixture");

    let secrets = Arc::new(FixtureSecretStore::default());
    let (endpoint, requests) = spawn_openai_server([
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"status\":\"ok\"}"}]}]}"#,
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"SkillYard","annotations":[{"type":"url_citation","url":"https://github.com/ReyYang/SkillYard"}]}]}]}"#,
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"category\":\"developmentEngineering\",\"summary\":\"用于审查 Rust 代码并指出风险。\",\"useCases\":[\"提交前检查 Rust 改动\",\"定位实现中的潜在风险\"],\"instructions\":\"在 Skill 详情中提供需要审查的 Rust 代码上下文。\"}"}]}]}"#,
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"category\":\"securityCompliance\",\"summary\":\"用于从安全角度审查 Rust 代码。\",\"useCases\":[\"检查不安全代码\",\"审查敏感数据处理\"],\"instructions\":\"提供待审查代码；文件未说明自动修复能力。\"}"}]}]}"#,
    ]);
    let application = configured_application(
        &data_root,
        &home,
        secrets.clone(),
        AgentProviderEndpoints::for_test(endpoint.clone()),
    );
    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::StartInitialScan)
        .expect("应扫描 Skill fixture")
    else {
        panic!("应返回 Inventory");
    };
    let entry = entries
        .iter()
        .find(|entry| entry.skill_name == "explain-me")
        .expect("应发现目标 Skill");
    let inventory_id = entry.id.clone();
    let fingerprint = entry.observed_fingerprint.clone();

    let first = generate(&application, &inventory_id);
    assert_eq!(first.category, SkillCategory::DevelopmentEngineering);
    assert_eq!(first.language, InterfaceLanguage::ZhCn);
    assert_eq!(first.content_fingerprint, fingerprint);
    assert!(!first.stale);
    assert_eq!(first.use_cases.len(), 2);
    assert_eq!(
        fs::read_to_string(skill_root.join("SKILL.md")).expect("应重新读取原文件"),
        original,
        "AI 整理不能修改 Skill 内容"
    );

    let replacement = generate(&application, &inventory_id);
    assert_eq!(replacement.category, SkillCategory::SecurityCompliance);
    assert_eq!(
        replacement.summary, "用于从安全角度审查 Rust 代码。",
        "重新生成应直接替换旧结果"
    );
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let row_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM skill_ai_explanations", [], |row| {
            row.get(0)
        })
        .expect("应统计 AI 说明");
    assert_eq!(row_count, 1, "每个 Skill 只保存一个完成结果");
    let columns = connection
        .prepare("PRAGMA table_info(skill_ai_explanations)")
        .expect("应读取 AI 说明 schema")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("应查询 AI 说明字段")
        .collect::<Result<Vec<_>, _>>()
        .expect("应收集 AI 说明字段");
    assert!(!columns.iter().any(|column| {
        matches!(
            column.as_str(),
            "provider" | "model" | "prompt" | "response" | "api_key"
        )
    }));
    drop(connection);

    let reopened = SkillYardApplication::new_with_agent_dependencies(
        ApplicationPaths::for_home(data_root, home),
        PlatformInfo::supported_for_test(),
        secrets,
        AgentProviderEndpoints::for_test(endpoint),
    );
    let UiOutcome::Inventory { entries, .. } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启后应读取持久化说明")
    else {
        panic!("重启后应返回 Inventory");
    };
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.id == inventory_id)
            .and_then(|entry| entry.ai_explanation.as_ref())
            .map(|explanation| explanation.summary.as_str()),
        Some("用于从安全角度审查 Rust 代码。")
    );

    let recorded = requests.recv().expect("应读取 Provider 请求");
    assert_eq!(recorded.len(), 4);
    for request in &recorded[2..] {
        assert!(
            !request.to_string().contains("web_search"),
            "AI 整理不能启用 Web Search"
        );
        assert!(request.to_string().contains("developmentEngineering"));
    }
}

#[test]
fn glm_and_deepseek_generate_the_same_fixed_explanation_without_web_search() {
    assert_provider_explanation(
        AiProvider::Glm,
        "glm-4.7",
        AgentProviderEndpoints::for_glm_test,
        "/api/paas/v4",
        [
            r#"{"choices":[{"message":{"content":"{\"status\":\"ok\"}"}}]}"#,
            r#"{"choices":[{"message":{"content":"SkillYard"}}],"web_search":[{"title":"SkillYard","link":"https://github.com/ReyYang/SkillYard"}]}"#,
            r#"{"choices":[{"message":{"content":"{\"category\":\"researchLearning\",\"summary\":\"用于整理研究资料。\",\"useCases\":[\"归纳资料\",\"提炼结论\"],\"instructions\":\"提供需要整理的资料。\"}"}}]}"#,
        ],
    );
    assert_provider_explanation(
        AiProvider::DeepSeek,
        "deepseek-v4-flash",
        AgentProviderEndpoints::for_deepseek_test,
        "/anthropic",
        [
            r#"{"content":[{"type":"tool_use","id":"tool_1","name":"skillyard_connection_test","input":{"status":"ok"}}]}"#,
            r#"{"content":[{"type":"web_search_tool_result","tool_use_id":"srv_1","content":[{"type":"web_search_result","url":"https://github.com/ReyYang/SkillYard","title":"SkillYard"}]}]}"#,
            r#"{"content":[{"type":"tool_use","id":"tool_2","name":"skillyard_skill_explanation","input":{"category":"researchLearning","summary":"用于整理研究资料。","useCases":["归纳资料","提炼结论"],"instructions":"提供需要整理的资料。"}}]}"#,
        ],
    );
}

#[test]
fn user_started_organization_updates_only_missing_or_stale_skills_and_isolates_failures() {
    let sandbox = tempdir().expect("应创建批量整理隔离目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    for (name, description) in [
        ("a-failed", "This explanation will fail"),
        ("b-stale", "Organize stale research"),
        ("c-stable", "Keep the current explanation"),
    ] {
        let root = home.join(".codex/skills").join(name);
        fs::create_dir_all(&root).expect("应创建批量整理 Skill fixture");
        fs::write(
            root.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
        )
        .expect("应写入批量整理 Skill fixture");
    }
    let (endpoint, requests) = spawn_openai_server([
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"status\":\"ok\"}"}]}]}"#,
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"SkillYard","annotations":[{"type":"url_citation","url":"https://github.com/ReyYang/SkillYard"}]}]}]}"#,
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"category\":\"productivityAutomation\",\"summary\":\"保留当前说明。\",\"useCases\":[\"稳定场景一\",\"稳定场景二\"],\"instructions\":\"保持当前说明。\"}"}]}]}"#,
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"category\":\"researchLearning\",\"summary\":\"旧研究说明。\",\"useCases\":[\"旧场景一\",\"旧场景二\"],\"instructions\":\"使用旧内容。\"}"}]}]}"#,
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"status\":\"ok\"}"}]}]}"#,
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"SkillYard","annotations":[{"type":"url_citation","url":"https://github.com/ReyYang/SkillYard"}]}]}]}"#,
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"not valid structured data"}]}]}"#,
        r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"category\":\"researchLearning\",\"summary\":\"按新内容整理研究资料。\",\"useCases\":[\"归纳新资料\",\"提炼新结论\"],\"instructions\":\"提供当前研究资料。\"}"}]}]}"#,
    ]);
    let application = configured_application(
        &data_root,
        &home,
        Arc::new(FixtureSecretStore::default()),
        AgentProviderEndpoints::for_test(endpoint),
    );
    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::StartInitialScan)
        .expect("应扫描批量整理 fixtures")
    else {
        panic!("应返回 Inventory");
    };
    let id_for = |name: &str| {
        entries
            .iter()
            .find(|entry| entry.skill_name == name)
            .map(|entry| entry.id.clone())
            .expect("应找到批量整理 Skill")
    };
    let stable_id = id_for("c-stable");
    let stale_id = id_for("b-stale");
    let failed_id = id_for("a-failed");
    generate(&application, &stable_id);
    generate(&application, &stale_id);

    application
        .handle(UiIntent::SetAiConfiguration {
            enabled: true,
            disclosure_accepted: true,
            provider: AiProvider::OpenAi,
            model: "gpt-5.4-mini".to_owned(),
        })
        .expect("应切换全局模型");
    application
        .handle(UiIntent::SaveAiApiKey {
            api_key: "skillyard-fixture-replaced-key".to_owned(),
        })
        .expect("应替换 fixture Key");
    let before_language_change = inventory(&application, UiIntent::GetStartupState);
    assert!(
        !explanation(&before_language_change, &stable_id).stale
            && !explanation(&before_language_change, &stale_id).stale,
        "Provider、模型和 API Key 变化不能使既有说明过时"
    );

    application
        .handle(UiIntent::SetInterfaceLanguage {
            language: InterfaceLanguage::En,
        })
        .expect("应切换英文");
    let english = inventory(&application, UiIntent::GetStartupState);
    assert!(
        explanation(&english, &stable_id).stale && explanation(&english, &stale_id).stale,
        "界面语言变化后旧结果仍可读取但必须过时"
    );
    application
        .handle(UiIntent::SetInterfaceLanguage {
            language: InterfaceLanguage::ZhCn,
        })
        .expect("应切回中文");
    let stale_root = home.join(".codex/skills/b-stale");
    fs::write(
        stale_root.join("SKILL.md"),
        "---\nname: b-stale\ndescription: Updated research\n---\n# Updated Research\n",
    )
    .expect("应修改待重新整理的 Skill");
    let refreshed = inventory(&application, UiIntent::RefreshLocalInventory);
    assert!(!explanation(&refreshed, &stable_id).stale);
    assert!(
        explanation(&refreshed, &stale_id).stale,
        "内容 fingerprint 变化后旧结果必须过时"
    );

    application
        .handle(UiIntent::TestAiConnection)
        .expect("应验证切换后的模型");
    let organized = inventory(&application, UiIntent::OrganizeSkillAiExplanations);
    assert_eq!(
        explanation(&organized, &stable_id).summary,
        "保留当前说明。",
        "未过时的说明不能重复调用或覆盖"
    );
    assert_eq!(
        explanation(&organized, &stale_id).summary,
        "按新内容整理研究资料。",
        "过时说明应被当前结果替换"
    );
    assert!(
        organized
            .iter()
            .find(|entry| entry.id == failed_id)
            .and_then(|entry| entry.ai_explanation.as_ref())
            .is_none(),
        "单项失败应保持待整理且不能阻塞其他 Skill"
    );

    let recorded = requests.recv().expect("应读取批量整理 Provider 请求");
    assert_eq!(
        recorded.len(),
        8,
        "扫描、刷新、语言与 Provider 设置都不能静默调用模型"
    );
    for request in &recorded[6..] {
        assert!(
            !request.to_string().contains("web_search"),
            "后台 AI 整理不能启用 Web Search"
        );
    }
}

fn inventory(
    application: &SkillYardApplication,
    intent: UiIntent,
) -> Vec<skillyard_lib::InventoryItem> {
    let UiOutcome::Inventory { entries, .. } = application
        .handle(intent)
        .expect("应返回可读取的 Inventory")
    else {
        panic!("应返回 Inventory");
    };
    entries
}

fn explanation<'a>(
    entries: &'a [skillyard_lib::InventoryItem],
    inventory_id: &str,
) -> &'a skillyard_lib::SkillAiExplanation {
    entries
        .iter()
        .find(|entry| entry.id == inventory_id)
        .and_then(|entry| entry.ai_explanation.as_ref())
        .expect("应保留目标 Skill 的 AI 说明")
}

fn assert_provider_explanation(
    provider: AiProvider,
    model: &str,
    endpoints: fn(String) -> AgentProviderEndpoints,
    base_path: &str,
    responses: [&'static str; 3],
) {
    let sandbox = tempdir().expect("应创建 Provider 隔离目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/provider-explanation");
    fs::create_dir_all(&skill_root).expect("应创建 Provider Skill fixture");
    fs::write(
        skill_root.join("SKILL.md"),
        "---\nname: provider-explanation\ndescription: Organize research\n---\n# Research\n",
    )
    .expect("应写入 Provider Skill fixture");
    let (endpoint, requests) = spawn_provider_server(base_path, responses);
    let application = configured_provider_application(
        &data_root,
        &home,
        Arc::new(FixtureSecretStore::default()),
        endpoints(endpoint),
        provider,
        model,
    );
    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::StartInitialScan)
        .expect("应扫描 Provider Skill")
    else {
        panic!("应返回 Inventory");
    };
    let inventory_id = entries
        .iter()
        .find(|entry| entry.skill_name == "provider-explanation")
        .map(|entry| entry.id.clone())
        .expect("应发现 Provider Skill");

    let explanation = generate(&application, &inventory_id);
    assert_eq!(explanation.category, SkillCategory::ResearchLearning);
    let recorded = requests.recv().expect("应读取 Provider 请求");
    assert_eq!(recorded.len(), 3);
    assert!(
        !recorded[2].to_string().contains("web_search"),
        "单 Skill AI 整理不能启用 Provider Web Search"
    );
}

fn generate(
    application: &SkillYardApplication,
    inventory_id: &str,
) -> skillyard_lib::SkillAiExplanation {
    let UiOutcome::Inventory { entries, .. } = application
        .handle(UiIntent::GenerateSkillAiExplanation {
            inventory_id: inventory_id.to_owned(),
        })
        .expect("用户主动整理应成功")
    else {
        panic!("整理完成后应返回 Inventory");
    };
    entries
        .into_iter()
        .find(|entry| entry.id == inventory_id)
        .and_then(|entry| entry.ai_explanation)
        .expect("Inventory 应携带完成的 AI 说明")
}

fn configured_application(
    data_root: &std::path::Path,
    home: &std::path::Path,
    secrets: Arc<FixtureSecretStore>,
    endpoints: AgentProviderEndpoints,
) -> SkillYardApplication {
    configured_provider_application(
        data_root,
        home,
        secrets,
        endpoints,
        AiProvider::OpenAi,
        "gpt-5.6-terra",
    )
}

fn configured_provider_application(
    data_root: &std::path::Path,
    home: &std::path::Path,
    secrets: Arc<FixtureSecretStore>,
    endpoints: AgentProviderEndpoints,
    provider: AiProvider,
    model: &str,
) -> SkillYardApplication {
    let application = SkillYardApplication::new_with_agent_dependencies(
        ApplicationPaths::for_home(data_root.to_path_buf(), home.to_path_buf()),
        PlatformInfo::supported_for_test(),
        secrets,
        endpoints,
    );
    application
        .handle(UiIntent::SetAiConfiguration {
            enabled: true,
            disclosure_accepted: true,
            provider,
            model: model.to_owned(),
        })
        .expect("应保存 AI 配置");
    application
        .handle(UiIntent::SaveAiApiKey {
            api_key: "skillyard-fixture-explanation-key".to_owned(),
        })
        .expect("应保存 fixture Key");
    application
        .handle(UiIntent::TestAiConnection)
        .expect("应验证当前 Provider");
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

fn spawn_openai_server<const N: usize>(
    responses: [&'static str; N],
) -> (String, mpsc::Receiver<Vec<Value>>) {
    spawn_provider_server("/v1", responses)
}

fn spawn_provider_server<const N: usize>(
    base_path: &str,
    responses: [&'static str; N],
) -> (String, mpsc::Receiver<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("应启动 AI 说明 Fake Server");
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
        if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = header_end + 4;
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + content_length {
                break;
            }
        }
    }
    bytes
}
