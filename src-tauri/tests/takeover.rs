use std::{
    env, fs,
    os::unix::fs::{MetadataExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};

use rusqlite::{Connection, params};
use skillyard_lib::{
    ApplicationPaths, LifecycleFailpoint, MountScope, PlatformInfo, ScanRootKey,
    SkillMetadataStatus, SkillYardApplication, SupportedAppId, TakeoverPlan, UiIntent, UiOutcome,
};
use tempfile::{TempDir, tempdir};

const TAKEOVER_HARD_EXIT_WORKER: &str = "SKILLYARD_TAKEOVER_HARD_EXIT_WORKER";
const TAKEOVER_HARD_EXIT_DATA_ROOT: &str = "SKILLYARD_TAKEOVER_HARD_EXIT_DATA_ROOT";
const TAKEOVER_HARD_EXIT_HOME: &str = "SKILLYARD_TAKEOVER_HARD_EXIT_HOME";
const TAKEOVER_HARD_EXIT_PLAN_ID: &str = "SKILLYARD_TAKEOVER_HARD_EXIT_PLAN_ID";
const TAKEOVER_HARD_EXIT_PATH_ID: &str = "SKILLYARD_TAKEOVER_HARD_EXIT_PATH_ID";
const TAKEOVER_HARD_EXIT_POINT: &str = "SKILLYARD_TAKEOVER_HARD_EXIT_POINT";

struct Harness {
    _sandbox: TempDir,
    home: PathBuf,
    data_root: PathBuf,
    paths: ApplicationPaths,
    application: SkillYardApplication,
}

impl Harness {
    fn new() -> Self {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let home = sandbox.path().join("home");
        let data_root = sandbox.path().join("application-support/SkillYard");
        fs::create_dir_all(&home).expect("应创建测试 home");
        let paths = ApplicationPaths::for_home(data_root.clone(), home.clone());
        let application =
            SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
        Self {
            _sandbox: sandbox,
            home,
            data_root,
            paths,
            application,
        }
    }

    fn write_skill(&self, relative: &str, name: &str) -> PathBuf {
        let root = self.home.join(relative);
        fs::create_dir_all(&root).expect("应创建 Skill 目录");
        fs::write(
            root.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} description\n---\n# {name}\n"),
        )
        .expect("应写入 Skill metadata");
        root
    }

    fn scan(&self) -> UiOutcome {
        self.application
            .handle(UiIntent::StartInitialScan)
            .expect("首次扫描应成功")
    }

    fn create_plan(&self, observation_id: &str) -> TakeoverPlan {
        let UiOutcome::TakeoverPlan { plan } = self
            .application
            .handle(UiIntent::CreateTakeoverPlan {
                observation_id: observation_id.to_owned(),
            })
            .expect("应生成接管计划")
        else {
            panic!("应返回 Takeover Plan");
        };
        plan
    }

    fn database(&self) -> PathBuf {
        self.data_root.join("skillyard.sqlite3")
    }
}

fn observation_id(outcome: &UiOutcome, name: &str, root_key: ScanRootKey) -> String {
    let UiOutcome::Inventory { entries, .. } = outcome else {
        panic!("应为 Inventory");
    };
    entries
        .iter()
        .find(|entry| entry.skill_name == name && entry.root_key == Some(root_key))
        .unwrap_or_else(|| panic!("应发现 {name}"))
        .id
        .clone()
}

fn table_count(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("应读取表行数")
}

#[test]
fn global_candidate_creates_unknown_source_read_only_plan() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "unchanged").expect("应写入内容文件");
    let before_root = fs::symlink_metadata(&original).expect("应读取原目录身份");
    let before_skill = fs::read(original.join("SKILL.md")).expect("应读取原内容");
    let scanned = harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);

    let plan = harness.create_plan(&id);

    assert!(plan.id.starts_with("takeover-"));
    assert_eq!(plan.observation_id, id);
    assert_eq!(plan.bundle_display_name, "alpha");
    assert_eq!(plan.source_display_name, None);
    assert_eq!(plan.source_notice, "来源未知；没有更新来源");
    assert_eq!(plan.skill_name, "alpha");
    assert_eq!(plan.skill_description, "alpha description");
    assert_eq!(plan.content_fingerprint.len(), 64);
    assert!(plan.warnings.is_empty());
    assert_eq!(plan.paths.len(), 1);
    let path = &plan.paths[0];
    assert_eq!(path.app_id, SupportedAppId::Codex);
    assert_eq!(path.scope, MountScope::Global);
    assert_eq!(path.project_id, None);
    assert_eq!(path.original_path, original.to_string_lossy());
    assert!(path.default_preserve_mount);
    assert_eq!(path.original_device, before_root.dev());
    assert_eq!(path.original_inode, before_root.ino());
    assert_eq!(path.original_mode, before_root.mode());
    assert_eq!(
        plan.expected_target,
        harness
            .data_root
            .join("bundles")
            .join(&plan.bundle_id)
            .join("current/members/alpha")
            .to_string_lossy()
    );
    assert_eq!(
        fs::read(original.join("SKILL.md")).expect("应重新读取原内容"),
        before_skill
    );
    let after_root = fs::symlink_metadata(&original).expect("应重新读取原目录身份");
    assert_eq!(
        (
            after_root.dev(),
            after_root.ino(),
            after_root.mtime(),
            after_root.mtime_nsec()
        ),
        (
            before_root.dev(),
            before_root.ino(),
            before_root.mtime(),
            before_root.mtime_nsec()
        )
    );

    let database = Connection::open(harness.database()).expect("应打开测试数据库");
    assert_eq!(table_count(&database, "takeover_plans"), 1);
    assert_eq!(table_count(&database, "takeover_plan_paths"), 1);
    for table in [
        "bundles",
        "skill_members",
        "member_selections",
        "mounts",
        "lifecycle_transactions",
        "mount_transactions",
        "batch_mount_transactions",
    ] {
        assert_eq!(table_count(&database, table), 0, "Plan 不得创建 {table} 行");
    }
    assert!(
        fs::read_dir(harness.data_root.join("bundles"))
            .expect("bundles 应存在")
            .next()
            .is_none()
    );
    assert!(
        fs::read_dir(harness.data_root.join("staging"))
            .expect("staging 应存在")
            .next()
            .is_none()
    );
    assert!(
        fs::read_dir(harness.data_root.join("journals"))
            .expect("journals 应存在")
            .next()
            .is_none()
    );
}

#[test]
fn project_candidate_uses_root_key_app_even_when_claude_is_observed_by_copilot() {
    let harness = Harness::new();
    harness.scan();
    let project = harness.home.join("work/demo");
    fs::create_dir_all(&project).expect("应创建 Project");
    let skill = project.join(".claude/skills/alpha");
    fs::create_dir_all(&skill).expect("应创建 Project Skill");
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: alpha\ndescription: project skill\n---\n",
    )
    .expect("应写入 Project Skill");

    let registered = harness
        .application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记 Project");
    let id = observation_id(&registered, "alpha", ScanRootKey::ClaudeCodeProject);
    let plan = harness.create_plan(&id);

    assert_eq!(plan.paths.len(), 1);
    let path = &plan.paths[0];
    assert_eq!(path.app_id, SupportedAppId::ClaudeCode);
    assert_eq!(path.scope, MountScope::Project);
    assert!(path.project_id.is_some());
    assert_eq!(
        path.original_path,
        fs::canonicalize(&skill)
            .expect("Project Skill 应可解析")
            .to_string_lossy()
    );
}

#[test]
fn same_named_candidates_at_different_app_paths_create_independent_bundles() {
    let harness = Harness::new();
    harness.write_skill(".codex/skills/alpha", "alpha");
    harness.write_skill(".claude/skills/alpha", "alpha");
    let scanned = harness.scan();
    let codex = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    let claude = observation_id(&scanned, "alpha", ScanRootKey::ClaudeCodeGlobal);

    let codex_plan = harness.create_plan(&codex);
    let claude_plan = harness.create_plan(&claude);

    assert_ne!(codex_plan.id, claude_plan.id);
    assert_ne!(codex_plan.bundle_id, claude_plan.bundle_id);
    assert_ne!(codex_plan.member_id, claude_plan.member_id);
    assert_ne!(codex_plan.paths[0].mount_id, claude_plan.paths[0].mount_id);
}

#[test]
fn stale_invalid_shared_and_non_candidate_observations_are_rejected() {
    let harness = Harness::new();
    harness.write_skill(".codex/skills/alpha", "alpha");
    harness.write_skill(".agents/skills/shared", "shared");
    let invalid = harness.home.join(".claude/skills/invalid");
    fs::create_dir_all(&invalid).expect("应创建无效 Skill");
    fs::write(invalid.join("SKILL.md"), "---\nname: []\n---\n").expect("应写入无效 metadata");
    let scanned = harness.scan();
    let alpha = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    let shared = observation_id(&scanned, "shared", ScanRootKey::SharedAgents);
    let invalid_id = {
        let UiOutcome::Inventory { entries, .. } = &scanned else {
            panic!("应为 Inventory");
        };
        let entry = entries
            .iter()
            .find(|entry| entry.root_key == Some(ScanRootKey::ClaudeCodeGlobal))
            .expect("应发现无效 Skill");
        assert_eq!(entry.metadata_status, SkillMetadataStatus::Invalid);
        entry.id.clone()
    };
    assert!(
        harness
            .application
            .handle(UiIntent::CreateTakeoverPlan {
                observation_id: shared
            })
            .is_err()
    );
    assert!(
        harness
            .application
            .handle(UiIntent::CreateTakeoverPlan {
                observation_id: invalid_id
            })
            .is_err()
    );

    let database = Connection::open(harness.database()).expect("应打开测试数据库");
    database
        .execute(
            "UPDATE inventory_observations SET stale = 1 WHERE id = ?1",
            [&alpha],
        )
        .expect("应伪造 stale");
    assert!(
        harness
            .application
            .handle(UiIntent::CreateTakeoverPlan {
                observation_id: alpha.clone()
            })
            .is_err()
    );
    for kind in ["agent_managed", "project_managed", "skillyard_managed"] {
        database
            .execute(
                "UPDATE inventory_observations SET stale = 0, management_kind = ?1 WHERE id = ?2",
                params![kind, alpha],
            )
            .expect("应伪造管理状态");
        assert!(
            harness
                .application
                .handle(UiIntent::CreateTakeoverPlan {
                    observation_id: alpha.clone()
                })
                .is_err()
        );
    }
    database.execute("UPDATE inventory_observations SET management_kind = 'takeover_candidate' WHERE id = ?1", [&alpha]).expect("应恢复候选状态");
    database.execute(
        "INSERT INTO inventory_management_evidence (observation_id, kind, authority_root, snapshot_commit_oid, subject_path) VALUES (?1, 'git_head_tracked', '/tmp/project', '0123456789012345678901234567890123456789', '.codex/skills/alpha/SKILL.md')",
        [&alpha],
    ).expect("应伪造管理证据");
    assert!(
        harness
            .application
            .handle(UiIntent::CreateTakeoverPlan {
                observation_id: alpha
            })
            .is_err()
    );
}

#[test]
fn content_or_project_management_changes_after_scan_require_refresh() {
    let harness = Harness::new();
    let root = harness.write_skill(".codex/skills/alpha", "alpha");
    let scanned = harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    fs::write(
        root.join("SKILL.md"),
        "---\nname: alpha\ndescription: changed after scan\n---\n",
    )
    .expect("应修改已扫描内容");
    assert!(
        harness
            .application
            .handle(UiIntent::CreateTakeoverPlan { observation_id: id })
            .is_err()
    );

    let project_harness = Harness::new();
    project_harness.scan();
    let project = project_harness.home.join("work/git-demo");
    let skill = project.join(".codex/skills/tracked");
    fs::create_dir_all(&skill).expect("应创建 Project Skill");
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: tracked\ndescription: tracked\n---\n",
    )
    .expect("应写入 Project Skill");
    let registered = project_harness
        .application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记无 Git 证据的 Project");
    let id = observation_id(&registered, "tracked", ScanRootKey::CodexProject);
    for args in [
        vec!["init"],
        vec!["config", "user.email", "skillyard@example.invalid"],
        vec!["config", "user.name", "SkillYard Test"],
        vec!["add", ".codex/skills/tracked/SKILL.md"],
        vec!["commit", "-m", "track skill"],
    ] {
        let status = Command::new("/usr/bin/git")
            .current_dir(&project)
            .args(args)
            .status()
            .expect("应执行本地 Git fixture");
        assert!(status.success());
    }
    assert!(
        project_harness
            .application
            .handle(UiIntent::CreateTakeoverPlan { observation_id: id })
            .is_err()
    );
}

#[test]
fn unsafe_root_and_unsafe_internal_entries_are_rejected() {
    let root_link_harness = Harness::new();
    let real = root_link_harness.write_skill("fixtures/alpha", "alpha");
    fs::create_dir_all(root_link_harness.home.join(".codex/skills")).expect("应创建 Host 根");
    symlink(&real, root_link_harness.home.join(".codex/skills/alpha")).expect("应创建根软链接");
    let scanned = root_link_harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    assert!(
        root_link_harness
            .application
            .handle(UiIntent::CreateTakeoverPlan { observation_id: id })
            .is_err()
    );

    let ancestor_link_harness = Harness::new();
    let real_parent = ancestor_link_harness.home.join("real-codex");
    fs::create_dir_all(real_parent.join("skills/alpha")).expect("应创建真实 Host 内容");
    fs::write(
        real_parent.join("skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: alpha\n---\n",
    )
    .expect("应写入 Skill");
    symlink(&real_parent, ancestor_link_harness.home.join(".codex"))
        .expect("应创建 Host 中间软链接");
    let scanned = ancestor_link_harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    assert!(
        ancestor_link_harness
            .application
            .handle(UiIntent::CreateTakeoverPlan { observation_id: id })
            .is_err()
    );

    for fixture in ["internal-symlink", "hardlink", "nested", "special"] {
        let harness = Harness::new();
        let root = harness.write_skill(&format!(".codex/skills/{fixture}"), fixture);
        match fixture {
            "internal-symlink" => {
                symlink("SKILL.md", root.join("linked.md")).expect("应创建内部软链接")
            }
            "hardlink" => {
                fs::hard_link(root.join("SKILL.md"), root.join("copy.md")).expect("应创建硬链接")
            }
            "nested" => {
                fs::create_dir_all(root.join("nested")).expect("应创建嵌套目录");
                fs::write(
                    root.join("nested/SKILL.md"),
                    "---\nname: child\ndescription: child\n---\n",
                )
                .expect("应写入嵌套 Skill");
            }
            "special" => {
                std::os::unix::net::UnixListener::bind(root.join("socket"))
                    .expect("应创建特殊文件");
            }
            _ => unreachable!(),
        }
        let scanned = harness.scan();
        let id = observation_id(&scanned, fixture, ScanRootKey::CodexGlobal);
        assert!(
            harness
                .application
                .handle(UiIntent::CreateTakeoverPlan { observation_id: id })
                .is_err(),
            "{fixture} 必须被拒绝"
        );
    }
}

#[test]
fn script_content_is_allowed_but_keeps_the_required_risk_warning() {
    let harness = Harness::new();
    let root = harness.write_skill(".codex/skills/alpha", "alpha");
    fs::create_dir_all(root.join("scripts")).expect("应创建脚本目录");
    fs::write(root.join("scripts/run.sh"), "#!/bin/sh\nexit 0\n").expect("应写入脚本");
    let scanned = harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);

    let plan = harness.create_plan(&id);

    assert_eq!(plan.warnings.len(), 1);
    assert!(plan.warnings[0].contains("脚本或可执行文件"));
}

#[test]
fn changed_project_identity_and_forged_host_leaf_are_rejected() {
    let harness = Harness::new();
    harness.scan();
    let project = harness.home.join("work/demo");
    let skill = project.join(".codex/skills/alpha");
    fs::create_dir_all(&skill).expect("应创建 Project Skill");
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: alpha\ndescription: alpha\n---\n",
    )
    .expect("应写入 Skill");
    let registered = harness
        .application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记 Project");
    let project_id = observation_id(&registered, "alpha", ScanRootKey::CodexProject);
    fs::rename(&project, harness.home.join("work/original")).expect("应移动原 Project");
    fs::create_dir_all(&project).expect("应在原路径创建替代 Project");
    assert!(
        harness
            .application
            .handle(UiIntent::CreateTakeoverPlan {
                observation_id: project_id
            })
            .is_err()
    );

    let global_harness = Harness::new();
    global_harness.write_skill(".codex/skills/alpha", "alpha");
    let scanned = global_harness.scan();
    let global_id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    let forged = global_harness.write_skill("elsewhere/alpha", "alpha");
    let database = Connection::open(global_harness.database()).expect("应打开测试数据库");
    database
        .execute(
            "UPDATE inventory_observations SET skill_root = ?1, skill_file = ?2 WHERE id = ?3",
            params![
                forged.to_string_lossy(),
                forged.join("SKILL.md").to_string_lossy(),
                global_id
            ],
        )
        .expect("应伪造观察路径");
    assert!(
        global_harness
            .application
            .handle(UiIntent::CreateTakeoverPlan {
                observation_id: global_id
            })
            .is_err()
    );
}

#[test]
fn restart_keeps_plan_persisted_without_executing_it() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    let scanned = harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    let plan = harness.create_plan(&id);
    let reopened =
        SkillYardApplication::new(harness.paths.clone(), PlatformInfo::supported_for_test());
    reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启应读取清单");
    assert!(original.is_dir());
    assert!(!Path::new(&plan.managed_directory).exists());
    assert_eq!(
        Connection::open(harness.database())
            .expect("应打开数据库")
            .query_row(
                "SELECT COUNT(*) FROM takeover_plans WHERE id = ?1",
                [&plan.id],
                |row| row.get::<_, i64>(0)
            )
            .expect("应读取持久化 Plan"),
        1
    );
}

#[test]
fn confirm_takeover_preserves_the_existing_host_path_as_a_managed_mount() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
    let scanned = harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    let plan = harness.create_plan(&id);

    let outcome = harness
        .application
        .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
        .expect("确认后应完成接管");

    assert!(
        fs::symlink_metadata(&original)
            .expect("Host 路径应继续存在")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(&original).expect("Host 路径应指向 Central Store"),
        Path::new(&plan.expected_target)
    );
    assert_eq!(
        fs::read_to_string(Path::new(&plan.expected_target).join("notes.txt"))
            .expect("Central Store 应保存完整内容"),
        "original payload"
    );
    let UiOutcome::Inventory {
        entries, mounts, ..
    } = outcome
    else {
        panic!("接管后应返回 Inventory");
    };
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.skill_name == "alpha")
            .count(),
        1
    );
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].target_path, original.to_string_lossy());

    let database = Connection::open(harness.database()).expect("应打开测试数据库");
    assert_eq!(table_count(&database, "bundles"), 1);
    assert_eq!(table_count(&database, "skill_members"), 1);
    assert_eq!(table_count(&database, "mounts"), 1);
    assert_eq!(table_count(&database, "takeover_transactions"), 0);
    assert_eq!(table_count(&database, "takeover_plans"), 0);
}

#[test]
fn confirm_takeover_can_install_without_leaving_a_host_mount() {
    let harness = Harness::new();
    let original = harness.write_skill(".claude/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
    let scanned = harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::ClaudeCodeGlobal);
    let plan = harness.create_plan(&id);

    let outcome = harness
        .application
        .confirm_takeover_plan(&plan.id, &[])
        .expect("确认后应完成不挂载接管");

    assert!(!original.exists(), "未保留挂载时 Host 路径应移除");
    assert_eq!(
        fs::read_to_string(Path::new(&plan.expected_target).join("notes.txt"))
            .expect("Central Store 应保存完整内容"),
        "original payload"
    );
    let UiOutcome::Inventory { mounts, .. } = outcome else {
        panic!("接管后应返回 Inventory");
    };
    assert!(mounts.is_empty());

    let database = Connection::open(harness.database()).expect("应打开测试数据库");
    assert_eq!(table_count(&database, "bundles"), 1);
    assert_eq!(table_count(&database, "mounts"), 0);
    assert_eq!(table_count(&database, "takeover_transactions"), 0);
}

#[test]
fn confirm_takeover_revalidates_content_before_consuming_the_plan() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    let scanned = harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    let plan = harness.create_plan(&id);
    fs::write(original.join("external.txt"), "changed after preview")
        .expect("应模拟确认前外部修改");

    harness
        .application
        .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
        .expect_err("确认必须拒绝已经变化的内容");

    assert!(original.is_dir());
    assert!(!Path::new(&plan.managed_directory).exists());
    let database = Connection::open(harness.database()).expect("应打开测试数据库");
    assert_eq!(
        database
            .query_row(
                "SELECT status FROM takeover_plans WHERE id = ?1",
                [&plan.id],
                |row| row.get::<_, String>(0),
            )
            .expect("确认前失败必须保留 Plan"),
        "pending"
    );
    assert_eq!(table_count(&database, "takeover_transactions"), 0);
}

#[test]
fn restart_aborts_takeover_interrupted_before_journal_without_touching_the_original() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
    let scanned = harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    let plan = harness.create_plan(&id);
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        harness.paths.clone(),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterTakeoverTransactionRecord,
    );

    interrupted
        .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
        .expect_err("failpoint 应在写入 Journal 前中断接管");

    let reopened =
        SkillYardApplication::new(harness.paths.clone(), PlatformInfo::supported_for_test());
    reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启应自动终止尚未写入 Journal 的接管");
    reopened
        .handle(UiIntent::GetStartupState)
        .expect("恢复必须可以重复执行");

    assert!(original.is_dir());
    assert_eq!(
        fs::read_to_string(original.join("notes.txt")).expect("原 Skill 应保持不变"),
        "original payload"
    );
    assert!(!Path::new(&plan.managed_directory).exists());
    assert_takeover_recovery_artifacts_clean(&harness);
}

#[test]
fn restart_aborts_takeover_interrupted_after_candidate_preparation() {
    let harness = Harness::new();
    let original = harness.write_skill(".claude/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
    let scanned = harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::ClaudeCodeGlobal);
    let plan = harness.create_plan(&id);
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        harness.paths.clone(),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterTakeoverCandidatePrepared,
    );

    interrupted
        .confirm_takeover_plan(&plan.id, &[])
        .expect_err("failpoint 应在候选准备后中断接管");

    let reopened =
        SkillYardApplication::new(harness.paths.clone(), PlatformInfo::supported_for_test());
    reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启应删除未生效候选并保留原 Skill");
    reopened
        .handle(UiIntent::GetStartupState)
        .expect("恢复必须可以重复执行");

    assert!(original.is_dir());
    assert_eq!(
        fs::read_to_string(original.join("notes.txt")).expect("原 Skill 应保持不变"),
        "original payload"
    );
    assert!(!Path::new(&plan.managed_directory).exists());
    assert_takeover_recovery_artifacts_clean(&harness);
}

#[test]
fn real_exit_after_takeover_transaction_record_recovers_the_original() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
    let plan = takeover_plan_for_hard_exit(&harness, "alpha", ScanRootKey::CodexGlobal);

    run_takeover_hard_exit_worker(&harness, &plan, "after-transaction-record");

    let database = Connection::open(harness.database()).expect("应打开测试数据库");
    assert_eq!(
        database
            .query_row("SELECT phase FROM takeover_transactions", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("应读取中断阶段"),
        "journal_pending"
    );
    assert!(
        fs::read_dir(harness.data_root.join("journals"))
            .expect("Journal 目录应存在")
            .next()
            .is_none()
    );
    drop(database);

    assert_takeover_restart_restores_original(&harness, &plan, &original);
}

#[test]
fn real_exit_after_journal_write_before_phase_recovers_the_original() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
    let plan = takeover_plan_for_hard_exit(&harness, "alpha", ScanRootKey::CodexGlobal);

    run_takeover_hard_exit_worker(&harness, &plan, "after-journal-before-phase");

    let database = Connection::open(harness.database()).expect("应打开测试数据库");
    assert_eq!(
        database
            .query_row("SELECT phase FROM takeover_transactions", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("应读取 SQLite 落后阶段"),
        "journal_pending"
    );
    assert!(
        fs::read_dir(harness.data_root.join("journals"))
            .expect("Journal 目录应存在")
            .next()
            .is_some(),
        "真实退出必须留下已持久化 Journal"
    );
    drop(database);

    assert_takeover_restart_restores_original(&harness, &plan, &original);
}

/// 父测试精确启动本 worker；`_exit` 会绕过析构并留下真实磁盘现场。
#[test]
fn hard_exit_takeover_worker() {
    if env::var_os(TAKEOVER_HARD_EXIT_WORKER).is_none() {
        return;
    }
    let data_root = env::var_os(TAKEOVER_HARD_EXIT_DATA_ROOT).expect("子进程必须收到数据目录");
    let home = env::var_os(TAKEOVER_HARD_EXIT_HOME).expect("子进程必须收到 home");
    let plan_id = env::var(TAKEOVER_HARD_EXIT_PLAN_ID).expect("子进程必须收到 Plan ID");
    let path_id = env::var(TAKEOVER_HARD_EXIT_PATH_ID).expect("子进程必须收到路径 ID");
    let failpoint = match env::var(TAKEOVER_HARD_EXIT_POINT).as_deref() {
        Ok("after-transaction-record") => {
            LifecycleFailpoint::HardExitAfterTakeoverTransactionRecord
        }
        Ok("after-journal-before-phase") => {
            LifecycleFailpoint::HardExitAfterTakeoverJournalWrittenBeforePhase
        }
        _ => panic!("子进程收到未知 Takeover failpoint"),
    };
    let application = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.into(), home.into()),
        PlatformInfo::supported_for_test(),
        failpoint,
    );
    application
        .confirm_takeover_plan(&plan_id, &[path_id])
        .expect("hard-exit failpoint 必须在返回前终止进程");
}

fn takeover_plan_for_hard_exit(
    harness: &Harness,
    name: &str,
    root_key: ScanRootKey,
) -> TakeoverPlan {
    let scanned = harness.scan();
    let id = observation_id(&scanned, name, root_key);
    harness.create_plan(&id)
}

fn run_takeover_hard_exit_worker(harness: &Harness, plan: &TakeoverPlan, point: &str) {
    let status = Command::new(env::current_exe().expect("应找到当前测试二进制"))
        .args(["--exact", "hard_exit_takeover_worker", "--nocapture"])
        .env(TAKEOVER_HARD_EXIT_WORKER, "1")
        .env(TAKEOVER_HARD_EXIT_DATA_ROOT, &harness.data_root)
        .env(TAKEOVER_HARD_EXIT_HOME, &harness.home)
        .env(TAKEOVER_HARD_EXIT_PLAN_ID, &plan.id)
        .env(TAKEOVER_HARD_EXIT_PATH_ID, &plan.paths[0].id)
        .env(TAKEOVER_HARD_EXIT_POINT, point)
        .status()
        .expect("应启动 Takeover hard-exit 子进程");
    assert_eq!(status.code(), Some(91), "子进程必须在 failpoint 直接退出");
}

fn assert_takeover_restart_restores_original(
    harness: &Harness,
    plan: &TakeoverPlan,
    original: &Path,
) {
    let reopened =
        SkillYardApplication::new(harness.paths.clone(), PlatformInfo::supported_for_test());
    reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启应自动终止生效前接管");
    reopened
        .handle(UiIntent::GetStartupState)
        .expect("恢复必须可以重复执行");
    assert!(original.is_dir());
    assert_eq!(
        fs::read_to_string(original.join("notes.txt")).expect("原 Skill 应保持不变"),
        "original payload"
    );
    assert!(!Path::new(&plan.managed_directory).exists());
    assert_takeover_recovery_artifacts_clean(harness);
}

fn assert_takeover_recovery_artifacts_clean(harness: &Harness) {
    let database = Connection::open(harness.database()).expect("应打开测试数据库");
    assert_eq!(table_count(&database, "takeover_transactions"), 0);
    assert_eq!(table_count(&database, "takeover_plans"), 0);
    for directory in ["staging", "journals"] {
        assert!(
            fs::read_dir(harness.data_root.join(directory))
                .expect("恢复目录应存在")
                .next()
                .is_none(),
            "恢复后 {directory} 应为空"
        );
    }
}

#[test]
fn project_takeover_can_preserve_the_registered_host_mount() {
    let harness = Harness::new();
    harness.scan();
    let project = harness.home.join("work/preserve-project");
    let original = project.join(".codex/skills/alpha");
    fs::create_dir_all(&original).expect("应创建 Project Skill");
    fs::write(
        original.join("SKILL.md"),
        "---\nname: alpha\ndescription: project alpha\n---\n",
    )
    .expect("应写入 Project Skill");
    let registered = harness
        .application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记 Project");
    let id = observation_id(&registered, "alpha", ScanRootKey::CodexProject);
    let plan = harness.create_plan(&id);

    let outcome = harness
        .application
        .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
        .expect("Project Skill 应完成保留挂载接管");

    assert!(
        fs::symlink_metadata(&original)
            .expect("Project Host 路径应存在")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(&original).unwrap(),
        Path::new(&plan.expected_target)
    );
    let UiOutcome::Inventory { mounts, .. } = outcome else {
        panic!("接管后应返回 Inventory");
    };
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].scope, MountScope::Project);
    assert_eq!(mounts[0].project_id, plan.paths[0].project_id);
}

#[test]
fn project_takeover_can_remove_the_host_path_without_a_mount() {
    let harness = Harness::new();
    harness.scan();
    let project = harness.home.join("work/unmounted-project");
    let original = project.join(".github/skills/alpha");
    fs::create_dir_all(&original).expect("应创建 Project Skill");
    fs::write(
        original.join("SKILL.md"),
        "---\nname: alpha\ndescription: project alpha\n---\n",
    )
    .expect("应写入 Project Skill");
    let registered = harness
        .application
        .handle(UiIntent::RegisterProject {
            root_path: project.to_string_lossy().into_owned(),
        })
        .expect("应登记 Project");
    let id = observation_id(&registered, "alpha", ScanRootKey::GitHubCopilotProject);
    let plan = harness.create_plan(&id);

    let outcome = harness
        .application
        .confirm_takeover_plan(&plan.id, &[])
        .expect("Project Skill 应完成未挂载接管");

    assert!(!original.exists());
    let UiOutcome::Inventory { mounts, .. } = outcome else {
        panic!("接管后应返回 Inventory");
    };
    assert!(mounts.is_empty());
}

#[test]
fn confirm_takeover_rejects_identical_content_with_replaced_inode_or_parent() {
    let inode_harness = Harness::new();
    let original = inode_harness.write_skill(".codex/skills/alpha", "alpha");
    let scanned = inode_harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    let plan = inode_harness.create_plan(&id);
    let replacement = inode_harness.write_skill("replacement/alpha", "alpha");
    fs::rename(&original, inode_harness.home.join("old-alpha")).expect("应移开原目录");
    fs::rename(&replacement, &original).expect("应放入内容相同的新 inode");
    inode_harness
        .application
        .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
        .expect_err("内容相同也不能接受已经替换的 inode");

    let parent_harness = Harness::new();
    let original = parent_harness.write_skill(".claude/skills/alpha", "alpha");
    let scanned = parent_harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::ClaudeCodeGlobal);
    let plan = parent_harness.create_plan(&id);
    let old_parent = parent_harness.home.join(".claude/skills-old");
    fs::rename(original.parent().unwrap(), &old_parent).expect("应移开原 Host 父目录");
    fs::create_dir(original.parent().unwrap()).expect("应创建替代父目录");
    fs::rename(old_parent.join("alpha"), &original).expect("应保留原 Skill inode");
    parent_harness
        .application
        .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
        .expect_err("原 Skill inode 未变也必须拒绝替换后的父目录");
}
