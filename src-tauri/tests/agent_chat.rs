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
    AgentSearchResultKind, AiProvider, ApplicationPaths, PlatformInfo, SecretStore,
    SecretStoreError, SkillYardApplication, UiIntent, UiOutcome,
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
            local_match_found: true,
            searched_public_web: false,
            search_results: Vec::new(),
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
fn local_search_reads_every_known_skill_kind_without_enabling_web_search() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    write_skill(
        &home.join(".codex/skills/takeover-review"),
        "takeover-review",
        "Review an existing local installation.",
    );
    write_skill(
        &home.join(".codex/plugins/cache/openai-bundled/review-plugin/1.0.0/skills/plugin-review"),
        "plugin-review",
        "Review code from an official plugin.",
    );
    let project = sandbox.path().join("sample-project");
    write_skill(
        &project.join(".codex/skills/project-review"),
        "project-review",
        "Review code from a registered project.",
    );
    let install_input = sandbox.path().join("downloads/managed-review");
    write_skill(
        &install_input,
        "managed-review",
        "Review code from SkillYard managed content.",
    );

    let secrets = Arc::new(FixtureSecretStore::default());
    let (endpoint, requests) = spawn_agent_server(
        "/v1",
        [
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"status\":\"ok\"}"}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"SkillYard","annotations":[{"type":"url_citation","url":"https://github.com/ReyYang/SkillYard"}]}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"reply\":\"本机已有四个可用于代码审查的 Skill。\",\"localMatchFound\":true,\"searchPublic\":false}"}]}]}"#,
        ],
    );
    let application = SkillYardApplication::new_with_agent_dependencies(
        ApplicationPaths::for_home(
            sandbox.path().join("application-support/SkillYard"),
            home.clone(),
        ),
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
            api_key: "skillyard-fixture-local-search-key".to_owned(),
        })
        .expect("应保存 fixture Key");
    application
        .handle(UiIntent::TestAiConnection)
        .expect("应先验证当前 Provider");
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应扫描全局与插件 Skill");
    application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记并扫描项目 Skill");
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateFolderInstallPlan {
            input_path: install_input.to_string_lossy().into_owned(),
        })
        .expect("应生成托管 Skill 安装预览")
    else {
        panic!("应返回安装 Plan");
    };
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: plan
                .candidates
                .into_iter()
                .filter(|candidate| candidate.default_selected)
                .map(|candidate| candidate.candidate_id)
                .collect(),
        })
        .expect("应通过生产入口安装托管 Skill");
    let inventory_before = application
        .handle(UiIntent::GetStartupState)
        .expect("应读取搜索前清单");

    assert_eq!(
        application
            .handle(UiIntent::AskAgent {
                context: AgentPageContext::Page {
                    page: skillyard_lib::AgentPageKind::Inventory,
                },
                messages: vec![AgentConversationMessage {
                    role: AgentMessageRole::User,
                    content: "我需要一个能做代码审查的 Skill".to_owned(),
                }],
            })
            .expect("本机 Skill 比较应返回匹配结果"),
        UiOutcome::AgentReply {
            reply: "本机已有四个可用于代码审查的 Skill。".to_owned(),
            local_match_found: true,
            searched_public_web: false,
            search_results: Vec::new(),
        }
    );

    let recorded = requests.recv().expect("Fake Server 应返回请求记录");
    assert_eq!(recorded.len(), 3);
    let request = &recorded[2];
    assert!(
        request.get("tools").is_none(),
        "有本机结果时不能启用 Provider Web Search"
    );
    let serialized = request.to_string();
    for skill_name in [
        "takeover-review",
        "plugin-review",
        "project-review",
        "managed-review",
    ] {
        assert!(
            serialized.contains(skill_name),
            "本机候选目录应包含 {skill_name}"
        );
    }
    assert!(!serialized.contains(home.to_string_lossy().as_ref()));
    assert!(!serialized.contains(project.to_string_lossy().as_ref()));
    assert_eq!(
        application
            .handle(UiIntent::GetStartupState)
            .expect("应读取搜索后清单"),
        inventory_before,
        "本机搜索不能修改 Inventory、Source、Bundle 或 Mount"
    );
}

#[test]
fn every_provider_uses_native_web_search_after_no_local_match() {
    assert_provider_web_search(
        AiProvider::OpenAi,
        "gpt-5.6-terra",
        AgentProviderEndpoints::for_test,
        "/v1",
        [
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"status\":\"ok\"}"}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"SkillYard","annotations":[{"type":"url_citation","url":"https://github.com/ReyYang/SkillYard"}]}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"reply\":\"本机没有匹配项，将搜索公开来源。\",\"localMatchFound\":false,\"searchPublic\":true}"}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"找到三个公开结果。","annotations":[{"type":"url_citation","url":"https://github.com/vercel-labs/skills","title":"Vercel Skills"},{"type":"url_citation","url":"https://downloads.example.com/review-skills.zip","title":"Review Skills ZIP"},{"type":"url_citation","url":"https://forum.example.com/review-skills","title":"Forum discussion"}]}]}]}"#,
        ],
    );
    assert_provider_web_search(
        AiProvider::Glm,
        "glm-4.7",
        AgentProviderEndpoints::for_glm_test,
        "/api/paas/v4",
        [
            r#"{"choices":[{"message":{"content":"{\"status\":\"ok\"}"}}]}"#,
            r#"{"choices":[{"message":{"content":"SkillYard"}}],"web_search":[{"title":"SkillYard","link":"https://github.com/ReyYang/SkillYard"}]}"#,
            r#"{"choices":[{"message":{"content":"{\"reply\":\"本机没有匹配项，将搜索公开来源。\",\"localMatchFound\":false,\"searchPublic\":true}"}}]}"#,
            r#"{"choices":[{"message":{"content":"找到三个公开结果。"}}],"web_search":[{"title":"Vercel Skills","link":"https://github.com/vercel-labs/skills"},{"title":"Review Skills ZIP","link":"https://downloads.example.com/review-skills.zip"},{"title":"Forum discussion","link":"https://forum.example.com/review-skills"}]}"#,
        ],
    );
    assert_provider_web_search(
        AiProvider::DeepSeek,
        "deepseek-v4-flash",
        AgentProviderEndpoints::for_deepseek_test,
        "/anthropic",
        [
            r#"{"content":[{"type":"tool_use","id":"tool_1","name":"skillyard_connection_test","input":{"status":"ok"}}]}"#,
            r#"{"content":[{"type":"web_search_tool_result","tool_use_id":"srv_1","content":[{"type":"web_search_result","url":"https://github.com/ReyYang/SkillYard","title":"SkillYard"}]}]}"#,
            r#"{"content":[{"type":"tool_use","id":"tool_2","name":"skillyard_agent_answer","input":{"reply":"本机没有匹配项，将搜索公开来源。","localMatchFound":false,"searchPublic":true}}]}"#,
            r#"{"content":[{"type":"text","text":"找到三个公开结果。"},{"type":"web_search_tool_result","tool_use_id":"srv_2","content":[{"type":"web_search_result","url":"https://github.com/vercel-labs/skills","title":"Vercel Skills"},{"type":"web_search_result","url":"https://downloads.example.com/review-skills.zip","title":"Review Skills ZIP"},{"type":"web_search_result","url":"https://forum.example.com/review-skills","title":"Forum discussion"}]}]}"#,
        ],
    );
}

#[test]
fn explicit_online_request_searches_even_when_a_local_skill_matches() {
    let sandbox = tempdir().expect("应创建显式联网隔离目录");
    let home = sandbox.path().join("home");
    write_skill(
        &home.join(".codex/skills/local-review"),
        "local-review",
        "Review local code.",
    );
    let (endpoint, requests) = spawn_agent_server(
        "/v1",
        [
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"status\":\"ok\"}"}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"SkillYard","annotations":[{"type":"url_citation","url":"https://github.com/ReyYang/SkillYard"}]}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"reply\":\"本机已有匹配项，但用户明确要求最新线上选择。\",\"localMatchFound\":true,\"searchPublic\":true}"}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"找到最新公开结果。","annotations":[{"type":"url_citation","url":"https://github.com/vercel-labs/skills","title":"Vercel Skills"}]}]}]}"#,
        ],
    );
    let application = configured_openai_application(sandbox.path(), home, endpoint);
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应扫描本机匹配项");

    let UiOutcome::AgentReply {
        local_match_found,
        searched_public_web,
        ..
    } = application
        .handle(UiIntent::AskAgent {
            context: AgentPageContext::Page {
                page: skillyard_lib::AgentPageKind::Inventory,
            },
            messages: vec![AgentConversationMessage {
                role: AgentMessageRole::User,
                content: "我想看看最新的线上代码审查 Skill".to_owned(),
            }],
        })
        .expect("显式线上请求应覆盖本地优先短路")
    else {
        panic!("应返回联网结果");
    };
    assert!(local_match_found);
    assert!(searched_public_web);
    let recorded = requests.recv().expect("应读取显式联网请求");
    assert_eq!(recorded.len(), 4);
    assert!(recorded[3].to_string().contains("web_search"));
}

#[test]
fn public_search_failure_does_not_mutate_local_state() {
    let sandbox = tempdir().expect("应创建联网失败隔离目录");
    let home = sandbox.path().join("home");
    fs::create_dir_all(&home).expect("应创建空 home");
    // Fake Server 在本机判断后关闭，第四次 Web Search 请求会得到连接失败。
    let (endpoint, requests) = spawn_agent_server(
        "/v1",
        [
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"status\":\"ok\"}"}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"SkillYard","annotations":[{"type":"url_citation","url":"https://github.com/ReyYang/SkillYard"}]}]}]}"#,
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"reply\":\"本机没有匹配项。\",\"localMatchFound\":false,\"searchPublic\":true}"}]}]}"#,
        ],
    );
    let application = configured_openai_application(sandbox.path(), home, endpoint);
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应完成空清单扫描");
    let before = application
        .handle(UiIntent::GetStartupState)
        .expect("应读取失败前状态");

    let error = application
        .handle(UiIntent::AskAgent {
            context: AgentPageContext::Page {
                page: skillyard_lib::AgentPageKind::Inventory,
            },
            messages: vec![AgentConversationMessage {
                role: AgentMessageRole::User,
                content: "找一个代码审查 Skill".to_owned(),
            }],
        })
        .expect_err("Provider Web Search 失败应只终止本次回答");
    assert!(error.to_string().contains("无法连接模型 Provider"));
    assert_eq!(
        application
            .handle(UiIntent::GetStartupState)
            .expect("应读取失败后状态"),
        before
    );
    assert_eq!(
        requests.recv().expect("应读取失败前请求").len(),
        3,
        "失败前只能完成两次验证和一次本机判断"
    );
}

fn assert_provider_web_search(
    provider: AiProvider,
    model: &str,
    endpoints: fn(String) -> AgentProviderEndpoints,
    base_path: &str,
    responses: [&'static str; 4],
) {
    let sandbox = tempdir().expect("应创建 Provider 搜索隔离目录");
    let home = sandbox.path().join("home");
    fs::create_dir_all(&home).expect("应创建空 home");
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
            api_key: "skillyard-fixture-public-search-key".to_owned(),
        })
        .expect("应保存 Provider fixture Key");
    application
        .handle(UiIntent::TestAiConnection)
        .expect("应验证 Provider");
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应完成空清单扫描");
    let state_before = application
        .handle(UiIntent::GetStartupState)
        .expect("应读取搜索前状态");

    let UiOutcome::AgentReply {
        reply,
        local_match_found,
        searched_public_web,
        search_results,
    } = application
        .handle(UiIntent::AskAgent {
            context: AgentPageContext::Page {
                page: skillyard_lib::AgentPageKind::Inventory,
            },
            messages: vec![AgentConversationMessage {
                role: AgentMessageRole::User,
                content: "我想找一个可以做代码审查的 Skill".to_owned(),
            }],
        })
        .expect("没有本机匹配时应搜索公开互联网")
    else {
        panic!("应返回 Agent 搜索结果");
    };
    assert_eq!(reply, "找到三个公开结果。");
    assert!(!local_match_found);
    assert!(searched_public_web);
    assert_eq!(search_results.len(), 3);
    assert_eq!(search_results[0].kind, AgentSearchResultKind::Github);
    assert_eq!(search_results[1].kind, AgentSearchResultKind::DirectUrl);
    assert_eq!(search_results[2].kind, AgentSearchResultKind::Reference);
    assert!(
        search_results
            .iter()
            .all(|result| result.url.starts_with("https://"))
    );

    let recorded = requests.recv().expect("应读取 Provider 搜索请求");
    assert_eq!(recorded.len(), 4);
    assert!(
        !recorded[2].to_string().contains("web_search"),
        "本机比较请求不能提前联网"
    );
    assert!(
        recorded[3].to_string().contains("web_search"),
        "公开搜索必须使用当前 Provider 的原生 Web Search"
    );
    assert_eq!(
        application
            .handle(UiIntent::GetStartupState)
            .expect("应读取搜索后状态"),
        state_before,
        "Agent 搜索不能创建或修改本机领域状态"
    );
}

fn configured_openai_application(
    sandbox: &std::path::Path,
    home: std::path::PathBuf,
    endpoint: String,
) -> SkillYardApplication {
    let application = SkillYardApplication::new_with_agent_dependencies(
        ApplicationPaths::for_home(sandbox.join("application-support/SkillYard"), home),
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
            api_key: "skillyard-fixture-search-key".to_owned(),
        })
        .expect("应保存 fixture Key");
    application
        .handle(UiIntent::TestAiConnection)
        .expect("应验证 OpenAI");
    application
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
            r#"{"choices":[{"message":{"content":"{\"reply\":\"fixture answer\",\"localMatchFound\":true,\"searchPublic\":false}"}}]}"#,
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
            r#"{"content":[{"type":"tool_use","id":"tool_2","name":"skillyard_agent_answer","input":{"reply":"fixture answer","localMatchFound":true,"searchPublic":false}}]}"#,
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
            local_match_found: true,
            searched_public_web: false,
            search_results: Vec::new(),
        }
    );
    let recorded = requests.recv().expect("应读取 Provider 请求");
    assert_eq!(recorded.len(), 3);
    assert!(
        !recorded[2].to_string().contains("web_search"),
        "普通解释路径不能启用 Provider Web Search；DeepSeek 只允许结构化回答工具"
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
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{\"reply\":\"这是一个用于解释代码的 Skill。\",\"localMatchFound\":true,\"searchPublic\":false}"}]}]}"#,
        ],
    )
}

fn write_skill(root: &std::path::Path, name: &str, description: &str) {
    fs::create_dir_all(root).expect("应创建 Skill fixture");
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n{description}\n"),
    )
    .expect("应写入 SKILL.md");
}

fn spawn_agent_server<const N: usize>(
    base_path: &str,
    responses: [&'static str; N],
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
