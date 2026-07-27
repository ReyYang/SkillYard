//! `skills.sh` 只提供发现结果；这里不会创建 Source 或执行安装。

use std::{
    collections::{BTreeMap, btree_map::Entry},
    io::Read,
};

use serde::Deserialize;
use thiserror::Error;
use url::Url;

use crate::{
    domain::{SkillsShSearchMember, SkillsShSearchSource},
    github_source::{SourceRequest, SourceTransport, parse_github_source},
};

const SEARCH_ENDPOINT: &str = "https://skills.sh/api/search";
const SEARCH_ACCEPT: &str = "application/json";
const MAX_QUERY_CHARS: usize = 100;
const MAX_RESULTS: usize = 200;
const MAX_SEARCH_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum SkillsShError {
    #[error("skills.sh 搜索词需要包含 2 到 100 个字符")]
    InvalidQuery,
    #[error("skills.sh 网络请求失败")]
    Network,
    #[error("skills.sh 返回了不支持的 HTTP 状态")]
    HttpStatus,
    #[error("skills.sh 返回了意外的跳转地址")]
    UnexpectedFinalUrl,
    #[error("skills.sh 响应超过固定上限 {limit} bytes：已检测到 {actual} bytes")]
    ResponseTooLarge { limit: u64, actual: u64 },
    #[error("skills.sh 返回了无效响应")]
    InvalidResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    skills: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    skill_id: String,
    name: String,
    installs: u64,
    source: String,
}

/// 公开端点的结果按完整来源分组；只有明确的 `owner/repo` 可继续进入 GitHub 流程。
pub fn search_skills_sh(
    transport: &dyn SourceTransport,
    query: &str,
) -> Result<(String, Vec<SkillsShSearchSource>), SkillsShError> {
    let query = query.trim();
    if !(2..=MAX_QUERY_CHARS).contains(&query.chars().count()) {
        return Err(SkillsShError::InvalidQuery);
    }
    let mut url = Url::parse(SEARCH_ENDPOINT).expect("固定 skills.sh URL 必须合法");
    url.query_pairs_mut().append_pair("q", query);
    let mut response = transport
        .get(SourceRequest {
            url,
            accept: SEARCH_ACCEPT.to_owned(),
        })
        .map_err(|_| SkillsShError::Network)?;
    if !(200..300).contains(&response.status) {
        return Err(SkillsShError::HttpStatus);
    }
    if response.final_url.scheme() != "https"
        || response.final_url.host_str() != Some("skills.sh")
        || response.final_url.path() != "/api/search"
    {
        return Err(SkillsShError::UnexpectedFinalUrl);
    }

    let mut bytes = Vec::new();
    response
        .body
        .by_ref()
        .take(MAX_SEARCH_RESPONSE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| SkillsShError::InvalidResponse)?;
    if bytes.len() as u64 > MAX_SEARCH_RESPONSE_BYTES {
        return Err(SkillsShError::ResponseTooLarge {
            limit: MAX_SEARCH_RESPONSE_BYTES,
            actual: bytes.len() as u64,
        });
    }
    let parsed: SearchResponse =
        serde_json::from_slice(&bytes).map_err(|_| SkillsShError::InvalidResponse)?;
    if parsed.skills.len() > MAX_RESULTS {
        return Err(SkillsShError::InvalidResponse);
    }

    let mut groups = BTreeMap::<String, SearchGroup>::new();
    for result in parsed.skills {
        if result.skill_id.trim().is_empty()
            || result.name.trim().is_empty()
            || result.source.trim().is_empty()
        {
            return Err(SkillsShError::InvalidResponse);
        }
        let raw_source = result.source.trim();
        let parsed_github = parse_github_source(raw_source, None).ok();
        let (group_key, source_input, supported) = match parsed_github {
            Some(parsed) => (
                format!(
                    "github:{}/{}",
                    parsed.owner.to_ascii_lowercase(),
                    parsed.repository.to_ascii_lowercase()
                ),
                format!("{}/{}", parsed.owner, parsed.repository),
                true,
            ),
            None => (
                format!("unsupported:{}", raw_source.to_ascii_lowercase()),
                raw_source.to_owned(),
                false,
            ),
        };
        let group = groups.entry(group_key).or_insert_with(|| SearchGroup {
            source_input,
            supported,
            members: BTreeMap::new(),
        });
        match group.members.entry(result.skill_id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(SkillsShSearchMember {
                    skill_id: result.skill_id,
                    name: result.name,
                    installs: result.installs,
                });
            }
            Entry::Occupied(mut slot) if slot.get().installs < result.installs => {
                slot.insert(SkillsShSearchMember {
                    skill_id: result.skill_id,
                    name: result.name,
                    installs: result.installs,
                });
            }
            Entry::Occupied(_) => {}
        }
    }

    let mut sources = groups
        .into_values()
        .map(|group| {
            let mut members = group.members.into_values().collect::<Vec<_>>();
            members.sort_by(|left, right| {
                right
                    .installs
                    .cmp(&left.installs)
                    .then_with(|| left.skill_id.cmp(&right.skill_id))
            });
            SkillsShSearchSource {
                source_input: group.source_input,
                supported: group.supported,
                members,
            }
        })
        .collect::<Vec<_>>();
    sources.sort_by(|left, right| {
        right
            .supported
            .cmp(&left.supported)
            .then_with(|| {
                right
                    .members
                    .first()
                    .map(|member| member.installs)
                    .cmp(&left.members.first().map(|member| member.installs))
            })
            .then_with(|| left.source_input.cmp(&right.source_input))
    });
    Ok((query.to_owned(), sources))
}

struct SearchGroup {
    source_input: String,
    supported: bool,
    members: BTreeMap<String, SkillsShSearchMember>,
}
