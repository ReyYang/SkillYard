use std::{collections::BTreeMap, fs};

use serde::Deserialize;

use crate::{
    domain::{InstallationChain, InstallationChainKind},
    paths::ApplicationPaths,
};

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
    #[serde(rename = "ref")]
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
