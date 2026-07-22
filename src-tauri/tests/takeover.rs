use std::{fs, os::unix::fs::MetadataExt, path::Path};

use skillyard_lib::{
    ApplicationPaths, MountScope, PlatformInfo, SkillYardApplication, SupportedAppId,
    TakeoverIdentityBasis, TakeoverOriginDisposition, TakeoverPlanRequest, UiIntent, UiOutcome,
};
use tempfile::tempdir;

#[test]
fn single_existing_skill_produces_a_read_only_takeover_plan() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/alpha");
    write_skill(&skill_root, "alpha", "接管测试");

    let original_metadata = fs::metadata(&skill_root).expect("应读取原 Skill 元数据");
    let original_content = fs::read(skill_root.join("SKILL.md")).expect("应读取原 Skill 内容");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let observation_id = match application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    {
        UiOutcome::Inventory { entries, .. } => {
            entries
                .into_iter()
                .find(|entry| entry.skill_name == "alpha")
                .expect("应发现待接管 Skill")
                .id
        }
        _ => panic!("首次扫描应返回 Inventory"),
    };

    let outcome = application
        .handle(UiIntent::CreateTakeoverPlan {
            request: TakeoverPlanRequest {
                observation_ids: vec![observation_id.clone()],
                selected_observation_id: observation_id.clone(),
                preserved_observation_ids: vec![observation_id],
                shared_targets: Vec::new(),
            },
        })
        .expect("应生成只读接管计划");
    let UiOutcome::TakeoverPlan { plan } = outcome else {
        panic!("应返回 Takeover Plan");
    };

    assert_eq!(plan.skill_name, "alpha");
    assert_eq!(plan.origins.len(), 1);
    assert_eq!(
        plan.origins[0].final_disposition,
        TakeoverOriginDisposition::Mount
    );
    assert_eq!(plan.origins[0].original_path, path_text(&skill_root));
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(plan.targets[0].app_id, SupportedAppId::Codex);
    assert_eq!(plan.targets[0].scope, MountScope::Global);
    assert_eq!(plan.targets[0].target_path, path_text(&skill_root));
    assert!(plan.source_display_name.is_none());
    assert!(!data_root.join("bundles").join(&plan.bundle_id).exists());

    let after_metadata = fs::metadata(&skill_root).expect("Plan 后原 Skill 必须仍存在");
    assert_eq!(
        (
            after_metadata.dev(),
            after_metadata.ino(),
            after_metadata.mode()
        ),
        (
            original_metadata.dev(),
            original_metadata.ino(),
            original_metadata.mode()
        ),
        "生成 Plan 不能替换原 Skill 目录"
    );
    assert_eq!(
        fs::read(skill_root.join("SKILL.md")).expect("Plan 后应读取原 Skill 内容"),
        original_content,
        "生成 Plan 不能修改原 Skill 内容"
    );
}

#[test]
fn user_selected_origins_form_one_identity_with_one_selected_content() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let codex_root = home.join(".codex/skills/alpha");
    let claude_root = home.join(".claude/skills/alpha");
    let copilot_root = home.join(".copilot/skills/alpha");
    write_skill(&codex_root, "alpha", "采用这份内容");
    write_skill(&claude_root, "alpha", "会被统一替换");
    write_skill(&copilot_root, "alpha", "未被用户选择");
    let original_files = [
        read_skill_file(&codex_root),
        read_skill_file(&claude_root),
        read_skill_file(&copilot_root),
    ];
    let original_identities = [
        file_identity(&codex_root),
        file_identity(&claude_root),
        file_identity(&copilot_root),
    ];

    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let entries = match application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功")
    {
        UiOutcome::Inventory { entries, .. } => entries,
        _ => panic!("首次扫描应返回 Inventory"),
    };
    let codex_id = observation_id_at(&entries, &codex_root);
    let claude_id = observation_id_at(&entries, &claude_root);
    let copilot_id = observation_id_at(&entries, &copilot_root);

    let outcome = application
        .handle(UiIntent::CreateTakeoverPlan {
            request: TakeoverPlanRequest {
                observation_ids: vec![codex_id.clone(), claude_id.clone()],
                selected_observation_id: codex_id.clone(),
                preserved_observation_ids: vec![codex_id.clone(), claude_id.clone()],
                shared_targets: Vec::new(),
            },
        })
        .expect("显式选择的同名副本应生成一份接管计划");
    let UiOutcome::TakeoverPlan { plan } = outcome else {
        panic!("应返回 Takeover Plan");
    };

    assert_eq!(plan.identity_basis, TakeoverIdentityBasis::UserConfirmed);
    assert_eq!(plan.selected_observation_id, codex_id);
    assert_eq!(plan.skill_description, "采用这份内容");
    assert_eq!(plan.origins.len(), 2);
    assert_eq!(plan.targets.len(), 2);
    assert!(
        plan.origins
            .iter()
            .all(|origin| origin.observation_id != copilot_id),
        "同名不能让未选择的观察自动进入计划"
    );
    assert!(
        plan.targets
            .iter()
            .any(|target| target.app_id == SupportedAppId::Codex)
    );
    assert!(
        plan.targets
            .iter()
            .any(|target| target.app_id == SupportedAppId::ClaudeCode)
    );
    assert!(
        plan.targets
            .iter()
            .all(|target| target.expected_target == plan.expected_target)
    );
    assert!(!data_root.join("bundles").join(&plan.bundle_id).exists());
    assert_eq!(read_skill_file(&codex_root), original_files[0]);
    assert_eq!(read_skill_file(&claude_root), original_files[1]);
    assert_eq!(read_skill_file(&copilot_root), original_files[2]);
    assert_eq!(file_identity(&codex_root), original_identities[0]);
    assert_eq!(file_identity(&claude_root), original_identities[1]);
    assert_eq!(file_identity(&copilot_root), original_identities[2]);
}

fn write_skill(root: &Path, name: &str, description: &str) {
    fs::create_dir_all(root).expect("应创建 Skill 根目录");
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
    )
    .expect("应写入有效 Skill");
}

fn read_skill_file(root: &Path) -> Vec<u8> {
    fs::read(root.join("SKILL.md")).expect("应读取原 Skill 内容")
}

fn file_identity(root: &Path) -> (u64, u64, u32) {
    let metadata = fs::metadata(root).expect("应读取原 Skill 元数据");
    (metadata.dev(), metadata.ino(), metadata.mode())
}

fn observation_id_at(entries: &[skillyard_lib::InventoryItem], root: &Path) -> String {
    entries
        .iter()
        .find(|entry| entry.skill_root == path_text(root))
        .unwrap_or_else(|| panic!("应发现 {}", root.display()))
        .id
        .clone()
}

fn path_text(path: &Path) -> String {
    path.to_str().expect("测试路径应为 UTF-8").to_owned()
}
