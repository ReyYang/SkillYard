use std::{collections::BTreeMap, fs};

use serde::Deserialize;

use crate::{
    domain::{InstallationChain, InstallationChainKind},
    github_source::parse_github_source,
    paths::ApplicationPaths,
};

/// 分组证据只来自能够规范化的来源身份，成员路径、hash 和安装时间都不能改变 Bundle 边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TakeoverGroupEvidence {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
struct LockV3File {
    version: u32,
    skills: BTreeMap<String, LockV3Entry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LockV3Entry {
    source: String,
    source_type: String,
    source_url: String,
    // Vercel skills 使用 ref，GitHub CLI 的兼容 v3 收据使用 pinnedRef。
    #[serde(rename = "ref", alias = "pinnedRef")]
    tracked_ref: Option<String>,
    skill_path: Option<String>,
    skill_folder_hash: String,
    installed_at: String,
    updated_at: String,
}

/// lock 是可选的外部收据；缺失、损坏或非 v3 时保持来源未知，不阻断本机扫描。
pub(crate) fn read_lock_v3_installation_chains(
    paths: &ApplicationPaths,
) -> BTreeMap<String, InstallationChain> {
    let record_path = paths.skill_lock_path();
    let Ok(content) = fs::read_to_string(record_path) else {
        return BTreeMap::new();
    };
    let Ok(lock) = serde_json::from_str::<LockV3File>(&content) else {
        return BTreeMap::new();
    };
    if lock.version != 3 {
        return BTreeMap::new();
    }

    lock.skills
        .into_iter()
        .filter_map(|(skill_name, entry)| {
            let chain = InstallationChain {
                kind: InstallationChainKind::LockV3,
                record_path: record_path.to_string_lossy().into_owned(),
                source: entry.source,
                source_type: entry.source_type,
                source_locator: entry.source_url,
                skill_path: entry.skill_path,
                tracked_ref: entry.tracked_ref,
                content_marker: entry.skill_folder_hash,
                installed_at: entry.installed_at,
                updated_at: entry.updated_at,
            };
            (!skill_name.trim().is_empty() && chain.is_valid()).then_some((skill_name, chain))
        })
        .collect()
}

/// lock v3 目前只对 SkillYard 已支持的 GitHub 来源提供确定性 Bundle 分组。
pub(crate) fn takeover_group_evidence(chain: &InstallationChain) -> Option<TakeoverGroupEvidence> {
    if chain.kind != InstallationChainKind::LockV3
        || !chain.source_type.eq_ignore_ascii_case("github")
    {
        return None;
    }
    let parsed = parse_github_source(&chain.source_locator, None).ok()?;
    Some(TakeoverGroupEvidence {
        id: format!(
            "github:{}/{}",
            parsed.owner.to_ascii_lowercase(),
            parsed.repository.to_ascii_lowercase()
        ),
        // lock 的 source 是安装工具保存的来源名称；URL 只负责提供稳定的分组身份。
        display_name: chain.source.trim().to_owned(),
    })
}
