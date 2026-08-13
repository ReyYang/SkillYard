use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use skillyard_lib::{
    ApplicationPaths, BatchMountRequest, BundleUpdateStatus, ManagementKind, MountHealth,
    MountScope, PlatformInfo, SkillYardApplication, SupportedAppId, TakeoverMemberRequest,
    TakeoverPlanRequest, UiIntent, UiOutcome,
};

const FIXTURE_VERSION: &str = "ticket8-five-bundles-v1";

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct FixtureReport {
    fixture_version: &'static str,
    group_count: usize,
    inventory_count: usize,
    managed_source_count: usize,
    mount_count: usize,
    bundle_counts: BTreeMap<String, usize>,
}

#[derive(Debug)]
struct SyntheticGroup {
    display_name: &'static str,
    source_url: &'static str,
    members: Vec<(String, String)>,
}

fn provision(
    home: &Path,
    data_root: &Path,
    platform: PlatformInfo,
) -> Result<FixtureReport, Box<dyn Error>> {
    validate_target(home, data_root)?;
    let groups = synthetic_groups();
    write_synthetic_inputs(home, &groups)?;

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.to_path_buf(), home.to_path_buf()),
        platform,
    );
    application.handle(UiIntent::StartInitialScan)?;

    take_over_group(&application, "anthropics/skills", false)?;
    let matt_bundle_id = take_over_group(&application, "mattpocock/skills", true)?;
    mount_bundle_for_claude(&application, &matt_bundle_id)?;
    take_over_group(&application, "vercel/skills", true)?;
    let final_inventory = application.handle(UiIntent::RefreshLocalInventory)?;

    validate_final_state(&application, final_inventory)
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() != 2 {
        return Err("用法：ticket8_visual_fixture <absolute-home> <absolute-data-root>".into());
    }
    let report = provision(
        Path::new(&arguments[0]),
        Path::new(&arguments[1]),
        PlatformInfo::current(),
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn validate_target(home: &Path, data_root: &Path) -> Result<(), Box<dyn Error>> {
    if !home.is_absolute() || !data_root.is_absolute() {
        return Err("home 和 data root 必须是绝对路径".into());
    }
    if !home.is_dir() {
        return Err(format!("home 不存在或不是目录：{}", home.display()).into());
    }
    let expected_data_root = home.join("Library/Application Support/SkillYard");
    if data_root != expected_data_root {
        return Err(format!(
            "data root 必须是当前 home 的 SkillYard 目录：{}",
            expected_data_root.display()
        )
        .into());
    }
    if data_root.exists() {
        return Err(format!("SkillYard data root 已存在：{}", data_root.display()).into());
    }

    for root in fixed_scan_roots(home) {
        ensure_absent_or_empty_directory(&root)?;
    }
    let lock = home.join(".agents/.skill-lock.json");
    if lock.exists() {
        return Err(format!("lock v3 已存在：{}", lock.display()).into());
    }
    Ok(())
}

fn fixed_scan_roots(home: &Path) -> [PathBuf; 7] {
    [
        home.join(".codex/skills"),
        home.join(".claude/skills"),
        home.join(".copilot/skills"),
        home.join(".agents/skills"),
        home.join(".codex/plugins/cache/openai-primary-runtime"),
        home.join(".codex/plugins/cache/openai-bundled"),
        home.join(".codex/plugins/cache/openai-curated-remote"),
    ]
}

fn ensure_absent_or_empty_directory(path: &Path) -> Result<(), Box<dyn Error>> {
    if !path.exists() {
        return Ok(());
    }
    if !path.is_dir() {
        return Err(format!("扫描根存在且不是目录：{}", path.display()).into());
    }
    if fs::read_dir(path)?.next().is_some() {
        return Err(format!("扫描根不是空目录：{}", path.display()).into());
    }
    Ok(())
}

fn synthetic_groups() -> Vec<SyntheticGroup> {
    let mut matt_members = vec![
        ("grill-me".to_owned(), "代码审查与改进".to_owned()),
        ("qa".to_owned(), "测试与质量保障工作流".to_owned()),
        ("refactor".to_owned(), "重构与优化实践".to_owned()),
        ("research".to_owned(), "研究与探索方法论".to_owned()),
        ("tdd".to_owned(), "测试驱动开发实践".to_owned()),
    ];
    matt_members.extend(numbered_members("zz-matt", 6, 41));
    vec![
        SyntheticGroup {
            display_name: "mattpocock/skills",
            source_url: "https://github.com/mattpocock/skills.git",
            members: matt_members,
        },
        SyntheticGroup {
            display_name: "anthropics/skills",
            source_url: "https://github.com/anthropics/skills.git",
            members: numbered_members("anthropic", 1, 85),
        },
        SyntheticGroup {
            display_name: "larkcli",
            source_url: "https://github.com/larksuite/cli.git",
            members: numbered_members("lark", 1, 38),
        },
        SyntheticGroup {
            display_name: "vercel/skills",
            source_url: "https://github.com/vercel/skills.git",
            members: numbered_members("vercel", 1, 9),
        },
    ]
}

fn numbered_members(prefix: &str, first: usize, last: usize) -> Vec<(String, String)> {
    (first..=last)
        .map(|index| {
            let name = format!("{prefix}-{index:03}");
            let description = format!("Ticket 8 视觉验收成员 {name}");
            (name, description)
        })
        .collect()
}

fn write_synthetic_inputs(home: &Path, groups: &[SyntheticGroup]) -> Result<(), Box<dyn Error>> {
    let skill_root = home.join(".codex/skills");
    let mut lock_entries = BTreeMap::<String, Value>::new();
    for group in groups {
        for (name, description) in &group.members {
            let contents = skill_contents(name, description);
            write_skill(&skill_root.join(name), &contents)?;
            let marker = format!("{:x}", Sha256::digest(contents.as_bytes()));
            lock_entries.insert(
                name.clone(),
                json!({
                    "source": group.display_name,
                    "sourceType": "github",
                    "sourceUrl": group.source_url,
                    "ref": "main",
                    "skillPath": format!("skills/{name}/SKILL.md"),
                    "skillFolderHash": marker,
                    "installedAt": "2026-08-12T00:00:00.000Z",
                    "updatedAt": "2026-08-12T00:00:00.000Z"
                }),
            );
        }
    }

    let lock_directory = home.join(".agents");
    fs::create_dir_all(&lock_directory)?;
    fs::write(
        lock_directory.join(".skill-lock.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 3,
            "skills": lock_entries,
        }))?,
    )?;

    let plugin_root = home.join(".codex/plugins/cache/openai-bundled/Codex 官方插件/1.0.0/skills");
    for (name, description) in numbered_members("official", 1, 27) {
        write_skill(
            &plugin_root.join(&name),
            &skill_contents(&name, &description),
        )?;
    }
    Ok(())
}

fn skill_contents(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n")
}

fn write_skill(root: &Path, contents: &str) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(root)?;
    fs::write(root.join("SKILL.md"), contents)?;
    Ok(())
}

fn take_over_group(
    application: &SkillYardApplication,
    display_name: &str,
    preserve_codex_locations: bool,
) -> Result<String, Box<dyn Error>> {
    let UiOutcome::Inventory { entries, .. } = application.handle(UiIntent::GetStartupState)?
    else {
        return Err("接管前应处于 Inventory".into());
    };
    let mut candidates = entries
        .into_iter()
        .filter(|entry| {
            entry.management_kind == ManagementKind::TakeoverCandidate
                && entry.takeover_group_display_name.as_deref() == Some(display_name)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.skill_name.cmp(&right.skill_name));
    if candidates.is_empty() {
        return Err(format!("没有找到待接管 Bundle：{display_name}").into());
    }
    let members = candidates
        .into_iter()
        .map(|entry| TakeoverMemberRequest {
            observation_ids: vec![entry.id.clone()],
            selected_observation_id: entry.id.clone(),
            preserved_observation_ids: preserve_codex_locations
                .then_some(vec![entry.id])
                .unwrap_or_default(),
        })
        .collect();
    let UiOutcome::TakeoverPlan { plan } = application.handle(UiIntent::CreateTakeoverPlan {
        request: TakeoverPlanRequest {
            members,
            shared_targets: Vec::new(),
        },
    })?
    else {
        return Err(format!("未返回 {display_name} 的 Takeover Plan").into());
    };
    let bundle_id = plan.bundle_id.clone();
    application.handle(UiIntent::ConfirmTakeoverPlan { plan_id: plan.id })?;
    Ok(bundle_id)
}

fn mount_bundle_for_claude(
    application: &SkillYardApplication,
    bundle_id: &str,
) -> Result<(), Box<dyn Error>> {
    let UiOutcome::Inventory { entries, .. } = application.handle(UiIntent::GetStartupState)?
    else {
        return Err("挂载前应处于 Inventory".into());
    };
    let requests = entries
        .into_iter()
        .filter(|entry| entry.bundle_id.as_deref() == Some(bundle_id))
        .map(|entry| {
            Ok(BatchMountRequest {
                member_id: entry.member_id.ok_or("受管 Inventory 缺少 member ID")?,
                app_id: SupportedAppId::ClaudeCode,
                scope: MountScope::Global,
                project_id: None,
            })
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let UiOutcome::BatchMountPlan { plan } =
        application.handle(UiIntent::CreateBatchMountPlan {
            bundle_id: bundle_id.to_owned(),
            requests,
        })?
    else {
        return Err("未返回 Claude Code Batch Mount Plan".into());
    };
    let selected_item_ids = plan
        .items
        .iter()
        .filter(|item| item.selectable)
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    if selected_item_ids.len() != 41 {
        return Err(format!(
            "Claude Code Batch Mount 应有 41 项，实际 {} 项",
            selected_item_ids.len()
        )
        .into());
    }
    application.handle(UiIntent::ConfirmBatchMountPlan {
        plan_id: plan.id,
        selected_item_ids,
    })?;
    Ok(())
}

fn validate_final_state(
    application: &SkillYardApplication,
    outcome: UiOutcome,
) -> Result<FixtureReport, Box<dyn Error>> {
    let UiOutcome::Inventory {
        entries,
        mounts,
        bundle_updates,
        scan_issues,
        recovery_issues,
        ..
    } = outcome
    else {
        return Err("最终刷新应返回 Inventory".into());
    };
    if !scan_issues.is_empty() || !recovery_issues.is_empty() {
        return Err("最终 Inventory 包含扫描或恢复问题".into());
    }
    if mounts.len() != 91
        || mounts
            .iter()
            .any(|mount| mount.health != MountHealth::Healthy)
    {
        return Err(format!("最终应有 91 个健康 Mount，实际 {} 个", mounts.len()).into());
    }

    let expected_counts = BTreeMap::from([
        ("Codex 官方插件".to_owned(), 27),
        ("anthropics/skills".to_owned(), 85),
        ("larkcli".to_owned(), 38),
        ("mattpocock/skills".to_owned(), 41),
        ("vercel/skills".to_owned(), 9),
    ]);
    let mut actual_counts = BTreeMap::<String, usize>::new();
    for entry in &entries {
        let group = entry
            .bundle_display_name
            .as_deref()
            .or(entry.takeover_group_display_name.as_deref())
            .or(entry.external_group_display_name.as_deref())
            .ok_or("Inventory entry 缺少可验证的分组名称")?;
        *actual_counts.entry(group.to_owned()).or_default() += 1;
    }
    if actual_counts != expected_counts {
        return Err(format!("五 Bundle 数量不符：{actual_counts:?}").into());
    }

    let managed_bundle_ids = entries
        .iter()
        .filter_map(|entry| entry.bundle_id.as_deref())
        .collect::<BTreeSet<_>>();
    if managed_bundle_ids.len() != 3
        || bundle_updates.len() != 3
        || bundle_updates
            .iter()
            .any(|summary| summary.status != BundleUpdateStatus::Available)
    {
        return Err("三个受管 Bundle 应全部保留真实可更新状态".into());
    }

    let UiOutcome::SourceDiscovery { sources, .. } =
        application.handle(UiIntent::OpenSourceDiscovery)?
    else {
        return Err("Source 回读应返回 SourceDiscovery".into());
    };
    let managed_source_count = sources
        .iter()
        .filter(|source| source.bundle_id.is_some())
        .count();
    if managed_source_count != 3 {
        return Err(format!("应有 3 个受管 Source，实际 {managed_source_count} 个").into());
    }

    Ok(FixtureReport {
        fixture_version: FIXTURE_VERSION,
        group_count: actual_counts.len(),
        inventory_count: entries.len(),
        managed_source_count,
        mount_count: mounts.len(),
        bundle_counts: actual_counts,
    })
}

#[cfg(test)]
mod tests {
    use skillyard_lib::{
        ApplicationPaths, PlatformInfo, SkillYardApplication, UiIntent, UiOutcome,
    };
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn builds_five_bundle_state_through_the_production_application_seam() {
        let sandbox = tempdir().expect("应创建隔离 Ticket 8 fixture 目录");
        let home = sandbox.path().join("home");
        let data_root = home.join("Library/Application Support/SkillYard");
        std::fs::create_dir_all(&home).expect("应创建隔离 home");

        let report = provision(&home, &data_root, PlatformInfo::supported_for_test())
            .expect("应生成 Ticket 8 五 Bundle fixture");
        assert_eq!(
            report,
            FixtureReport {
                fixture_version: FIXTURE_VERSION,
                group_count: 5,
                inventory_count: 200,
                managed_source_count: 3,
                mount_count: 91,
                bundle_counts: BTreeMap::from([
                    ("Codex 官方插件".to_owned(), 27),
                    ("anthropics/skills".to_owned(), 85),
                    ("larkcli".to_owned(), 38),
                    ("mattpocock/skills".to_owned(), 41),
                    ("vercel/skills".to_owned(), 9),
                ]),
            }
        );

        let restarted = SkillYardApplication::new(
            ApplicationPaths::for_home(data_root, home),
            PlatformInfo::supported_for_test(),
        );
        let UiOutcome::Inventory {
            entries,
            mounts,
            scan_issues,
            recovery_issues,
            ..
        } = restarted
            .handle(UiIntent::GetStartupState)
            .expect("重启后应从正式 seam 读取 fixture")
        else {
            panic!("重启后应返回 Inventory");
        };
        assert_eq!(entries.len(), 200);
        assert_eq!(mounts.len(), 91);
        assert!(scan_issues.is_empty());
        assert!(recovery_issues.is_empty());
    }
}
