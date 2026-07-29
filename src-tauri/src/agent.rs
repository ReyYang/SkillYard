use std::{io::Read, sync::Arc, time::Duration};

use reqwest::blocking::Client;
use serde_json::{Value, json};
use thiserror::Error;
use url::Url;

use crate::domain::AiProvider;

const KEYCHAIN_SERVICE: &str = "com.reyyang.skillyard.ai";
const MAX_PROVIDER_RESPONSE_BYTES: u64 = 1_048_576;

#[derive(Debug, Error)]
pub enum SecretStoreError {
    #[error("macOS Keychain 当前不可用")]
    Unavailable,
    #[error("macOS Keychain 中的 API Key 不是有效文本")]
    InvalidValue,
}

/// 生产实现写入 macOS Keychain；测试替身也必须走同一个 Application seam。
pub trait SecretStore: Send + Sync {
    fn read(&self, account: &str) -> Result<Option<String>, SecretStoreError>;
    fn write(&self, account: &str, value: &str) -> Result<(), SecretStoreError>;
    fn delete(&self, account: &str) -> Result<(), SecretStoreError>;
}

pub type SharedSecretStore = Arc<dyn SecretStore>;

#[derive(Default)]
pub(crate) struct KeychainSecretStore;

#[cfg(target_os = "macos")]
impl SecretStore for KeychainSecretStore {
    fn read(&self, account: &str) -> Result<Option<String>, SecretStoreError> {
        use security_framework::passwords::get_generic_password;
        use security_framework_sys::base::errSecItemNotFound;

        match get_generic_password(KEYCHAIN_SERVICE, account) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| SecretStoreError::InvalidValue),
            Err(error) if error.code() == errSecItemNotFound => Ok(None),
            Err(_) => Err(SecretStoreError::Unavailable),
        }
    }

    fn write(&self, account: &str, value: &str) -> Result<(), SecretStoreError> {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            account,
            value.as_bytes(),
        )
        .map_err(|_| SecretStoreError::Unavailable)
    }

    fn delete(&self, account: &str) -> Result<(), SecretStoreError> {
        use security_framework_sys::base::errSecItemNotFound;

        match security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, account) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == errSecItemNotFound => Ok(()),
            Err(_) => Err(SecretStoreError::Unavailable),
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl SecretStore for KeychainSecretStore {
    fn read(&self, _account: &str) -> Result<Option<String>, SecretStoreError> {
        Err(SecretStoreError::Unavailable)
    }

    fn write(&self, _account: &str, _value: &str) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::Unavailable)
    }

    fn delete(&self, _account: &str) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::Unavailable)
    }
}

/// Provider 地址属于应用内建协议，测试只能整体替换，用户不能编辑。
#[derive(Debug, Clone)]
pub struct AgentProviderEndpoints {
    open_ai: String,
    glm: String,
}

impl AgentProviderEndpoints {
    pub(crate) fn production() -> Self {
        Self {
            open_ai: "https://api.openai.com/v1".to_owned(),
            glm: "https://open.bigmodel.cn/api/paas/v4".to_owned(),
        }
    }

    #[doc(hidden)]
    pub fn for_test(open_ai: String) -> Self {
        Self {
            open_ai,
            ..Self::production()
        }
    }

    #[doc(hidden)]
    pub fn for_glm_test(glm: String) -> Self {
        Self {
            glm,
            ..Self::production()
        }
    }

    fn open_ai_responses(&self) -> String {
        format!("{}/responses", self.open_ai.trim_end_matches('/'))
    }

    fn glm_chat_completions(&self) -> String {
        format!("{}/chat/completions", self.glm.trim_end_matches('/'))
    }
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
    #[error("请先阅读并同意 AI 数据发送说明")]
    DisclosureRequired,
    #[error("当前 Provider 还没有保存 API Key")]
    MissingApiKey,
    #[error("当前 Provider 不支持模型 {0}")]
    UnsupportedModel(String),
    #[error("当前 Provider 尚未由 SkillYard 支持")]
    UnsupportedProvider,
    #[error("无法连接模型 Provider")]
    ProviderUnavailable,
    #[error("模型 Provider 拒绝了请求（HTTP {0}）")]
    ProviderRejected(u16),
    #[error("模型 Provider 没有返回 SkillYard 需要的 {0} 能力")]
    InvalidCapability(&'static str),
}

pub(crate) fn verify_provider(
    endpoints: &AgentProviderEndpoints,
    provider: AiProvider,
    model: &str,
    api_key: &str,
) -> Result<(), AgentError> {
    if !provider.supports_model(model) {
        return Err(AgentError::UnsupportedModel(model.to_owned()));
    }
    match provider {
        AiProvider::OpenAi => verify_openai(endpoints, model, api_key),
        AiProvider::Glm => verify_glm(endpoints, model, api_key),
        AiProvider::DeepSeek => Err(AgentError::UnsupportedProvider),
    }
}

fn verify_glm(
    endpoints: &AgentProviderEndpoints,
    model: &str,
    api_key: &str,
) -> Result<(), AgentError> {
    let client = provider_client()?;
    let endpoint = endpoints.glm_chat_completions();

    let structured = send_json(
        &client,
        &endpoint,
        api_key,
        &json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": "Return a JSON object with exactly one field: {\"status\":\"ok\"}."
            }],
            "response_format": { "type": "json_object" },
            "stream": false
        }),
    )?;
    if !glm_message_contents(&structured).any(contains_json_status_ok) {
        return Err(AgentError::InvalidCapability("JSON Output"));
    }

    let searched = send_json(
        &client,
        &endpoint,
        api_key,
        &json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": "Find the official SkillYard GitHub repository and cite one public URL."
            }],
            "tools": [{
                "type": "web_search",
                "web_search": {
                    "enable": true,
                    "search_query": "SkillYard GitHub",
                    "search_result": true
                }
            }],
            "tool_choice": "auto",
            "stream": false
        }),
    )?;
    if !contains_glm_public_url(&searched) {
        return Err(AgentError::InvalidCapability("Web Search"));
    }
    Ok(())
}

fn verify_openai(
    endpoints: &AgentProviderEndpoints,
    model: &str,
    api_key: &str,
) -> Result<(), AgentError> {
    let client = provider_client()?;
    let endpoint = endpoints.open_ai_responses();

    let structured = send_json(
        &client,
        &endpoint,
        api_key,
        &json!({
            "model": model,
            "input": "Return the fixed connection-test result.",
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "skillyard_connection_test",
                    "strict": true,
                    "schema": {
                        "type": "object",
                        "properties": {
                            "status": { "type": "string", "enum": ["ok"] }
                        },
                        "required": ["status"],
                        "additionalProperties": false
                    }
                }
            },
            "store": false
        }),
    )?;
    if !contains_structured_ok(&structured) {
        return Err(AgentError::InvalidCapability("Structured Outputs"));
    }

    let searched = send_json(
        &client,
        &endpoint,
        api_key,
        &json!({
            "model": model,
            "input": "Find the official SkillYard GitHub repository and cite one public URL.",
            "tools": [{ "type": "web_search" }],
            "store": false
        }),
    )?;
    if !contains_public_url_citation(&searched) {
        return Err(AgentError::InvalidCapability("Web Search"));
    }
    Ok(())
}

fn provider_client() -> Result<Client, AgentError> {
    Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|_| AgentError::ProviderUnavailable)
}

fn send_json(
    client: &Client,
    endpoint: &str,
    api_key: &str,
    body: &Value,
) -> Result<Value, AgentError> {
    let mut response = client
        .post(endpoint)
        .bearer_auth(api_key)
        .json(body)
        .send()
        .map_err(|_| AgentError::ProviderUnavailable)?;
    let status = response.status();
    if !status.is_success() {
        return Err(AgentError::ProviderRejected(status.as_u16()));
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_PROVIDER_RESPONSE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|_| AgentError::ProviderUnavailable)?;
    serde_json::from_slice(&bytes).map_err(|_| AgentError::ProviderUnavailable)
}

fn contains_structured_ok(response: &Value) -> bool {
    output_texts(response).any(contains_json_status_ok)
}

fn contains_json_status_ok(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .as_deref()
        == Some("ok")
}

fn output_texts(response: &Value) -> impl Iterator<Item = &str> {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|content| {
            (content.get("type").and_then(Value::as_str) == Some("output_text"))
                .then(|| content.get("text").and_then(Value::as_str))
                .flatten()
        })
}

fn contains_public_url_citation(response: &Value) -> bool {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .flat_map(|content| {
            content
                .get("annotations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|annotation| annotation.get("type").and_then(Value::as_str) == Some("url_citation"))
        .filter_map(|annotation| annotation.get("url").and_then(Value::as_str))
        .any(|candidate| {
            Url::parse(candidate)
                .map(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
                .unwrap_or(false)
        })
}

fn glm_message_contents(response: &Value) -> impl Iterator<Item = &str> {
    response
        .get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|choice| {
            choice
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
        })
}

fn contains_glm_public_url(response: &Value) -> bool {
    response
        .get("web_search")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("link").and_then(Value::as_str))
        .any(is_public_url)
}

fn is_public_url(candidate: &str) -> bool {
    Url::parse(candidate)
        .map(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
        .unwrap_or(false)
}
