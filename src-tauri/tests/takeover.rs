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
const TAKEOVER_HARD_EXIT_PRESERVE: &str = "SKILLYARD_TAKEOVER_HARD_EXIT_PRESERVE";

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

    run_takeover_hard_exit_worker(&harness, &plan, "after-transaction-record", true);

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

    run_takeover_hard_exit_worker(&harness, &plan, "after-journal-before-phase", true);

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
        Ok("after-candidate-published") => {
            LifecycleFailpoint::HardExitAfterTakeoverCandidatePublishedBeforePhase
        }
        Ok("after-replacement-staged") => {
            LifecycleFailpoint::HardExitAfterTakeoverReplacementStaged
        }
        Ok("after-host-swapped") => LifecycleFailpoint::HardExitAfterTakeoverHostSwappedBeforePhase,
        Ok("after-state-committed") => {
            LifecycleFailpoint::HardExitAfterTakeoverStateCommittedBeforeJournal
        }
        Ok("after-original-moved") => {
            LifecycleFailpoint::HardExitAfterTakeoverOriginalMovedBeforeDiscard
        }
        _ => panic!("子进程收到未知 Takeover failpoint"),
    };
    let preserve_mount = env::var(TAKEOVER_HARD_EXIT_PRESERVE).as_deref() == Ok("1");
    let application = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.into(), home.into()),
        PlatformInfo::supported_for_test(),
        failpoint,
    );
    let preserved = if preserve_mount {
        vec![path_id]
    } else {
        Vec::new()
    };
    application
        .confirm_takeover_plan(&plan_id, &preserved)
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

fn run_takeover_hard_exit_worker(
    harness: &Harness,
    plan: &TakeoverPlan,
    point: &str,
    preserve_mount: bool,
) {
    let status = Command::new(env::current_exe().expect("应找到当前测试二进制"))
        .args(["--exact", "hard_exit_takeover_worker", "--nocapture"])
        .env(TAKEOVER_HARD_EXIT_WORKER, "1")
        .env(TAKEOVER_HARD_EXIT_DATA_ROOT, &harness.data_root)
        .env(TAKEOVER_HARD_EXIT_HOME, &harness.home)
        .env(TAKEOVER_HARD_EXIT_PLAN_ID, &plan.id)
        .env(TAKEOVER_HARD_EXIT_PATH_ID, &plan.paths[0].id)
        .env(TAKEOVER_HARD_EXIT_POINT, point)
        .env(
            TAKEOVER_HARD_EXIT_PRESERVE,
            if preserve_mount { "1" } else { "0" },
        )
        .status()
        .expect("应启动 Takeover hard-exit 子进程");
    assert_eq!(status.code(), Some(91), "子进程必须在 failpoint 直接退出");
}

#[test]
fn takeover_soft_interruption_recovery_matrix() {
    for (failpoint, preserve_mount, should_install) in [
        (
            LifecycleFailpoint::AfterTakeoverReplacementStaged,
            true,
            false,
        ),
        (LifecycleFailpoint::AfterTakeoverHostSwapped, true, true),
        (LifecycleFailpoint::AfterTakeoverHostSwapped, false, true),
        (LifecycleFailpoint::AfterTakeoverStateCommitted, true, true),
    ] {
        let harness = Harness::new();
        let original = harness.write_skill(".codex/skills/alpha", "alpha");
        fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
        let scanned = harness.scan();
        let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
        let plan = harness.create_plan(&id);
        let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
            harness.paths.clone(),
            PlatformInfo::supported_for_test(),
            failpoint,
        );
        let preserved = if preserve_mount {
            vec![plan.paths[0].id.clone()]
        } else {
            Vec::new()
        };

        interrupted
            .confirm_takeover_plan(&plan.id, &preserved)
            .expect_err("soft failpoint 应留下可恢复事务");

        if should_install {
            assert_takeover_restart_finishes_install(&harness, &plan, &original, preserve_mount);
        } else {
            assert_takeover_restart_restores_original(&harness, &plan, &original);
        }
    }
}

#[test]
fn takeover_hard_exit_recovery_matrix() {
    for (point, preserve_mount, should_install) in [
        ("after-candidate-published", true, false),
        ("after-replacement-staged", true, false),
        ("after-host-swapped", false, true),
        ("after-state-committed", true, true),
        ("after-original-moved", false, true),
    ] {
        let harness = Harness::new();
        let original = harness.write_skill(".codex/skills/alpha", "alpha");
        fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
        let plan = takeover_plan_for_hard_exit(&harness, "alpha", ScanRootKey::CodexGlobal);

        run_takeover_hard_exit_worker(&harness, &plan, point, preserve_mount);

        if should_install {
            assert_takeover_restart_finishes_install(&harness, &plan, &original, preserve_mount);
        } else {
            assert_takeover_restart_restores_original(&harness, &plan, &original);
        }
    }
}

#[test]
fn takeover_recovery_replays_each_persisted_host_swapped_window() {
    for sqlite_already_advanced in [false, true] {
        let harness = Harness::new();
        let original = harness.write_skill(".codex/skills/alpha", "alpha");
        fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
        let plan = takeover_plan_for_hard_exit(&harness, "alpha", ScanRootKey::CodexGlobal);
        run_takeover_hard_exit_worker(&harness, &plan, "after-host-swapped", true);

        let journal_path = only_takeover_journal(&harness);
        let mut journal: serde_json::Value =
            serde_json::from_slice(&fs::read(&journal_path).expect("应读取接管 Journal"))
                .expect("Journal 应为 JSON");
        let transaction_id = journal["transaction_id"]
            .as_str()
            .expect("Journal 应记录 transaction_id")
            .to_owned();
        // 恢复器先持久化 Journal，再推进 SQLite；两个相邻窗口都必须可以重放。
        journal["phase"] = serde_json::json!("host_swapped");
        fs::write(
            &journal_path,
            serde_json::to_vec_pretty(&journal).expect("应序列化 Journal"),
        )
        .expect("应模拟恢复已持久化 HostSwapped Journal");
        if sqlite_already_advanced {
            let database = Connection::open(harness.database()).expect("应打开测试数据库");
            database
                .execute(
                    "UPDATE takeover_transactions SET phase = 'host_swapped' WHERE id = ?1",
                    [transaction_id],
                )
                .expect("应模拟恢复已推进 SQLite phase");
        }

        assert_takeover_restart_finishes_install(&harness, &plan, &original, true);
    }
}

#[test]
fn completed_takeover_without_journal_finishes_both_cleanup_windows() {
    for remove_staging_before_restart in [false, true] {
        let harness = Harness::new();
        let original = harness.write_skill(".codex/skills/alpha", "alpha");
        fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
        let scanned = harness.scan();
        let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
        let plan = harness.create_plan(&id);
        let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
            harness.paths.clone(),
            PlatformInfo::supported_for_test(),
            LifecycleFailpoint::AfterTakeoverStateCommitted,
        );
        interrupted
            .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
            .expect_err("领域提交后 failpoint 应留下待清理原目录");

        let journal_path = only_takeover_journal(&harness);
        let journal: serde_json::Value =
            serde_json::from_slice(&fs::read(&journal_path).expect("应读取接管 Journal"))
                .expect("接管 Journal 应为 JSON");
        let transaction_id = journal["transaction_id"]
            .as_str()
            .expect("Journal 应记录 transaction_id");
        let hidden_name = journal["hidden_name"]
            .as_str()
            .expect("Journal 应记录 hidden_name");
        // 等价模拟：原目录已经按 manifest 删除，DB phase 已提交，cleanup 刚删完 Journal。
        fs::remove_dir_all(
            original
                .parent()
                .expect("Host 路径应有父目录")
                .join(hidden_name),
        )
        .expect("测试应模拟已安全删除隔离原目录");
        let database = Connection::open(harness.database()).expect("应打开测试数据库");
        database
            .execute(
                "UPDATE takeover_transactions SET phase = 'original_discarded' WHERE id = ?1",
                [transaction_id],
            )
            .expect("应模拟已提交原目录清理阶段");
        drop(database);
        fs::remove_file(&journal_path).expect("应模拟 cleanup 已删除 Journal");
        if remove_staging_before_restart {
            fs::remove_dir(harness.data_root.join("staging").join(transaction_id))
                .expect("应模拟 cleanup 已删除空 staging");
        }

        assert_takeover_restart_finishes_install(&harness, &plan, &original, true);
    }
}

#[test]
fn aborted_takeover_without_journal_forgets_the_verified_empty_transaction() {
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
        .expect_err("事务记录后 failpoint 应留下无 Journal 状态");
    let database = Connection::open(harness.database()).expect("应打开测试数据库");
    database
        .execute(
            "UPDATE takeover_transactions SET status = 'aborted' WHERE plan_id = ?1",
            [&plan.id],
        )
        .expect("应模拟 abort 已提交但尚未 forget 的合法窗口");
    drop(database);

    assert_takeover_restart_restores_original(&harness, &plan, &original);
}

#[test]
fn complete_atomic_write_temp_journal_is_verified_and_cleaned() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
    let plan = takeover_plan_for_hard_exit(&harness, "alpha", ScanRootKey::CodexGlobal);
    run_takeover_hard_exit_worker(&harness, &plan, "after-journal-before-phase", true);
    let journal = only_takeover_journal(&harness);
    let transaction_id = Connection::open(harness.database())
        .expect("应打开测试数据库")
        .query_row("SELECT id FROM takeover_transactions", [], |row| {
            row.get::<_, String>(0)
        })
        .expect("应读取接管事务 ID");
    let temporary = harness.data_root.join("journals").join(format!(
        ".{transaction_id}.json.tmp-{}",
        uuid::Uuid::new_v4()
    ));
    fs::rename(journal, &temporary).expect("应模拟正式 rename 前留下的完整临时 Journal");

    assert_takeover_restart_restores_original(&harness, &plan, &original);
    assert!(!temporary.exists());
}

#[test]
fn incomplete_first_write_temp_journal_is_safely_cleaned() {
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
        .expect_err("事务记录后 failpoint 应留下无正式 Journal 状态");
    let transaction_id = Connection::open(harness.database())
        .expect("应打开测试数据库")
        .query_row("SELECT id FROM takeover_transactions", [], |row| {
            row.get::<_, String>(0)
        })
        .expect("应读取接管事务 ID");
    let temporary = harness.data_root.join("journals").join(format!(
        ".{transaction_id}.json.tmp-{}",
        uuid::Uuid::new_v4()
    ));
    fs::write(&temporary, b"{incomplete").expect("应模拟未完成临时 Journal");

    assert_takeover_restart_restores_original(&harness, &plan, &original);
    assert!(!temporary.exists());
}

#[test]
fn formal_journal_wins_over_an_incomplete_atomic_write_temp() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
    let scanned = harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    let plan = harness.create_plan(&id);
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        harness.paths.clone(),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterTakeoverCandidatePrepared,
    );
    interrupted
        .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
        .expect_err("候选准备后 failpoint 应留下正式 Journal");
    let transaction_id = Connection::open(harness.database())
        .expect("应打开测试数据库")
        .query_row("SELECT id FROM takeover_transactions", [], |row| {
            row.get::<_, String>(0)
        })
        .expect("应读取接管事务 ID");
    let temporary = harness.data_root.join("journals").join(format!(
        ".{transaction_id}.json.tmp-{}",
        uuid::Uuid::new_v4()
    ));
    fs::write(&temporary, b"{new-phase-incomplete")
        .expect("应模拟更新既有 Journal 时留下的临时文件");

    assert_takeover_restart_restores_original(&harness, &plan, &original);
    assert!(!temporary.exists());
}

#[test]
fn missing_or_tampered_takeover_journal_blocks_only_that_object() {
    for tamper in [false, true] {
        let harness = Harness::new();
        let original = harness.write_skill(".codex/skills/alpha", "alpha");
        fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
        let scanned = harness.scan();
        let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
        let plan = harness.create_plan(&id);
        let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
            harness.paths.clone(),
            PlatformInfo::supported_for_test(),
            LifecycleFailpoint::AfterTakeoverCandidatePrepared,
        );
        interrupted
            .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
            .expect_err("候选准备后 failpoint 应留下 Journal");
        let journal_path = only_takeover_journal(&harness);
        if tamper {
            let mut journal: serde_json::Value =
                serde_json::from_slice(&fs::read(&journal_path).expect("应读取 Journal"))
                    .expect("Journal 应为 JSON");
            journal["original_entries"][0]["inode"] = serde_json::json!(1);
            fs::write(
                &journal_path,
                serde_json::to_vec_pretty(&journal).expect("应序列化篡改 Journal"),
            )
            .expect("应模拟外部篡改 Journal");
        } else {
            fs::remove_file(&journal_path).expect("应模拟外部删除 Journal");
        }

        assert_takeover_restart_is_blocked_but_readable(&harness, "alpha");
        assert!(original.is_dir());
        assert_eq!(
            fs::read_to_string(original.join("notes.txt")).expect("原 Skill 不得被修改"),
            "original payload"
        );
    }
}

#[test]
fn journal_phase_outside_the_adjacent_sqlite_window_is_blocked() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
    let scanned = harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    let plan = harness.create_plan(&id);
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        harness.paths.clone(),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterTakeoverCandidatePrepared,
    );
    interrupted
        .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
        .expect_err("候选准备后 failpoint 应留下 Journal");
    let journal_path = only_takeover_journal(&harness);
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).expect("应读取 Journal"))
            .expect("Journal 应为 JSON");
    // phase 不在 seal 内；恢复器仍必须用 SQLite 邻接表拒绝跨阶段伪造。
    journal["phase"] = serde_json::json!("original_discarded");
    fs::write(
        &journal_path,
        serde_json::to_vec_pretty(&journal).expect("应序列化 Journal"),
    )
    .expect("应写入越级 Journal phase");

    assert_takeover_restart_is_blocked_but_readable(&harness, "alpha");
    assert!(original.is_dir());
}

#[test]
fn external_host_or_central_changes_block_takeover_without_deleting_them() {
    for mutate_host in [false, true] {
        let harness = Harness::new();
        let original = harness.write_skill(".codex/skills/alpha", "alpha");
        fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
        let scanned = harness.scan();
        let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
        let plan = harness.create_plan(&id);
        let failpoint = if mutate_host {
            LifecycleFailpoint::AfterTakeoverHostSwapped
        } else {
            LifecycleFailpoint::AfterTakeoverReplacementStaged
        };
        let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
            harness.paths.clone(),
            PlatformInfo::supported_for_test(),
            failpoint,
        );
        interrupted
            .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
            .expect_err("failpoint 应留下待恢复事务");

        if mutate_host {
            fs::remove_file(&original).expect("应移除 SkillYard 创建的 Host Mount");
            symlink(harness.home.join("external-target"), &original)
                .expect("应模拟外部替换 Host Mount");
        } else {
            fs::write(
                Path::new(&plan.managed_directory).join("external.txt"),
                "keep",
            )
            .expect("应模拟外部修改 Central Bundle");
        }

        assert_takeover_restart_is_blocked_but_readable(&harness, "alpha");
        if mutate_host {
            assert_eq!(
                fs::read_link(&original).expect("外部 Host Mount 必须保留"),
                harness.home.join("external-target")
            );
        } else {
            assert_eq!(
                fs::read_to_string(Path::new(&plan.managed_directory).join("external.txt"))
                    .expect("Central 外部内容必须保留"),
                "keep"
            );
        }
    }
}

#[test]
fn changed_discard_manifest_blocks_takeover_without_deleting_the_change() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
    let plan = takeover_plan_for_hard_exit(&harness, "alpha", ScanRootKey::CodexGlobal);
    run_takeover_hard_exit_worker(&harness, &plan, "after-original-moved", false);
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(only_takeover_journal(&harness)).expect("应读取 Journal"))
            .expect("Journal 应为 JSON");
    let transaction_id = journal["transaction_id"]
        .as_str()
        .expect("Journal 应记录 transaction_id");
    let changed = harness
        .data_root
        .join("staging")
        .join(transaction_id)
        .join("discarding-original/notes.txt");
    fs::write(&changed, "external change").expect("应模拟外部修改 discard 文件");

    assert_takeover_restart_is_blocked_but_readable(&harness, "alpha");
    assert_eq!(
        fs::read_to_string(changed).expect("外部修改不得被删除"),
        "external change"
    );
}

#[test]
fn changed_host_or_central_while_discard_waits_preserves_the_original() {
    for mutate_host in [false, true] {
        let harness = Harness::new();
        let original = harness.write_skill(".codex/skills/alpha", "alpha");
        fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
        let plan = takeover_plan_for_hard_exit(&harness, "alpha", ScanRootKey::CodexGlobal);
        run_takeover_hard_exit_worker(&harness, &plan, "after-original-moved", true);
        let journal: serde_json::Value = serde_json::from_slice(
            &fs::read(only_takeover_journal(&harness)).expect("应读取 Journal"),
        )
        .expect("Journal 应为 JSON");
        let transaction_id = journal["transaction_id"]
            .as_str()
            .expect("Journal 应记录 transaction_id");
        let discard = harness
            .data_root
            .join("staging")
            .join(transaction_id)
            .join("discarding-original");

        if mutate_host {
            fs::remove_file(&original).expect("应移除 SkillYard 创建的 Host Mount");
            symlink(harness.home.join("external-target"), &original)
                .expect("应模拟外部替换 Host Mount");
        } else {
            fs::write(
                Path::new(&plan.managed_directory).join("external.txt"),
                "keep",
            )
            .expect("应模拟外部修改 Central Bundle");
        }

        assert_takeover_restart_is_blocked_but_readable(&harness, "alpha");
        assert!(discard.is_dir(), "状态变化后隔离原目录必须完整保留");
        assert_eq!(
            fs::read_to_string(discard.join("notes.txt")).expect("隔离原内容必须保留"),
            "original payload"
        );
    }
}

#[test]
fn partial_discard_resumes_after_a_file_or_subtree_was_already_deleted() {
    for delete_subtree in [false, true] {
        let harness = Harness::new();
        let original = harness.write_skill(".codex/skills/alpha", "alpha");
        fs::write(original.join("notes.txt"), "original payload").expect("应写入根文件");
        fs::create_dir(original.join("nested")).expect("应创建子目录");
        fs::write(original.join("nested/child.txt"), "nested payload").expect("应写入子目录文件");
        let plan = takeover_plan_for_hard_exit(&harness, "alpha", ScanRootKey::CodexGlobal);
        run_takeover_hard_exit_worker(&harness, &plan, "after-original-moved", false);
        let journal: serde_json::Value = serde_json::from_slice(
            &fs::read(only_takeover_journal(&harness)).expect("应读取 Journal"),
        )
        .expect("Journal 应为 JSON");
        let transaction_id = journal["transaction_id"]
            .as_str()
            .expect("Journal 应记录 transaction_id");
        let discard = harness
            .data_root
            .join("staging")
            .join(transaction_id)
            .join("discarding-original");
        if delete_subtree {
            fs::remove_dir_all(discard.join("nested")).expect("应模拟子树已删除");
        } else {
            fs::remove_file(discard.join("notes.txt")).expect("应模拟文件已删除");
        }

        assert_takeover_restart_finishes_install(&harness, &plan, &original, false);
        assert_eq!(
            fs::read_to_string(Path::new(&plan.expected_target).join("nested/child.txt"))
                .expect("Central 主副本不得受 discard 进度影响"),
            "nested payload"
        );
    }
}

#[test]
fn partial_discard_with_an_external_replacement_still_blocks() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "original payload").expect("应写入根文件");
    let plan = takeover_plan_for_hard_exit(&harness, "alpha", ScanRootKey::CodexGlobal);
    run_takeover_hard_exit_worker(&harness, &plan, "after-original-moved", false);
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(only_takeover_journal(&harness)).expect("应读取 Journal"))
            .expect("Journal 应为 JSON");
    let transaction_id = journal["transaction_id"]
        .as_str()
        .expect("Journal 应记录 transaction_id");
    let discard = harness
        .data_root
        .join("staging")
        .join(transaction_id)
        .join("discarding-original");
    fs::remove_file(discard.join("notes.txt")).expect("应模拟一个条目已经删除");
    let replacement = harness.home.join("replacement-skill.md");
    fs::write(&replacement, "external replacement").expect("应创建外部替换文件");
    fs::rename(&replacement, discard.join("SKILL.md")).expect("应替换仍存在的授权文件");

    assert_takeover_restart_is_blocked_but_readable(&harness, "alpha");
    assert_eq!(
        fs::read_to_string(discard.join("SKILL.md")).expect("外部替换不得被删除"),
        "external replacement"
    );
}

#[test]
fn partial_candidate_is_safely_removed_before_takeover_effect() {
    for remove_nested_tree in [false, true] {
        let harness = Harness::new();
        let original = harness.write_skill(".codex/skills/alpha", "alpha");
        fs::write(original.join("notes.txt"), "original payload").expect("应写入根文件");
        fs::create_dir(original.join("nested")).expect("应创建子目录");
        fs::write(original.join("nested/child.txt"), "nested payload").expect("应写入子目录文件");
        let scanned = harness.scan();
        let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
        let plan = harness.create_plan(&id);
        let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
            harness.paths.clone(),
            PlatformInfo::supported_for_test(),
            LifecycleFailpoint::AfterTakeoverCandidatePrepared,
        );
        interrupted
            .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
            .expect_err("候选准备后 failpoint 应留下 staging");
        let journal: serde_json::Value = serde_json::from_slice(
            &fs::read(only_takeover_journal(&harness)).expect("应读取 Journal"),
        )
        .expect("Journal 应为 JSON");
        let transaction_id = journal["transaction_id"]
            .as_str()
            .expect("Journal 应记录 transaction_id");
        let candidate = harness
            .data_root
            .join("staging")
            .join(transaction_id)
            .join("candidate/members/alpha");
        if remove_nested_tree {
            fs::remove_dir_all(candidate.join("nested")).expect("应模拟子树尚未复制完成");
        } else {
            fs::remove_file(candidate.join("notes.txt")).expect("应模拟文件尚未复制完成");
        }

        assert_takeover_restart_restores_original(&harness, &plan, &original);
    }
}

#[test]
fn partial_candidate_with_unknown_content_is_blocked_and_preserved() {
    for relative in ["unknown.txt", "members/alpha/unknown.txt"] {
        let harness = Harness::new();
        let _original = harness.write_skill(".codex/skills/alpha", "alpha");
        let scanned = harness.scan();
        let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
        let plan = harness.create_plan(&id);
        let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
            harness.paths.clone(),
            PlatformInfo::supported_for_test(),
            LifecycleFailpoint::AfterTakeoverCandidatePrepared,
        );
        interrupted
            .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
            .expect_err("候选准备后 failpoint 应留下 staging");
        let journal: serde_json::Value = serde_json::from_slice(
            &fs::read(only_takeover_journal(&harness)).expect("应读取 Journal"),
        )
        .expect("Journal 应为 JSON");
        let transaction_id = journal["transaction_id"]
            .as_str()
            .expect("Journal 应记录 transaction_id");
        let unknown = harness
            .data_root
            .join("staging")
            .join(transaction_id)
            .join("candidate")
            .join(relative);
        fs::write(&unknown, "keep").expect("应模拟未知候选条目");

        assert_takeover_restart_is_blocked_but_readable(&harness, "alpha");
        assert_eq!(
            fs::read_to_string(unknown).expect("未知条目不得被清理"),
            "keep"
        );
    }
}

#[test]
fn partial_bundle_publish_shapes_roll_back_before_host_effect() {
    for shape in [
        "empty-bundle",
        "empty-contents",
        "content",
        "temp-current",
        "current",
    ] {
        let harness = Harness::new();
        let original = harness.write_skill(".codex/skills/alpha", "alpha");
        fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
        let scanned = harness.scan();
        let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
        let plan = harness.create_plan(&id);
        let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
            harness.paths.clone(),
            PlatformInfo::supported_for_test(),
            LifecycleFailpoint::AfterTakeoverCandidatePrepared,
        );
        interrupted
            .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
            .expect_err("候选准备后 failpoint 应留下 publish 前现场");
        let journal: serde_json::Value = serde_json::from_slice(
            &fs::read(only_takeover_journal(&harness)).expect("应读取 Journal"),
        )
        .expect("Journal 应为 JSON");
        let transaction_id = journal["transaction_id"]
            .as_str()
            .expect("Journal 应记录 transaction_id");
        let content_id = journal["content_id"]
            .as_str()
            .expect("Journal 应记录 content_id");
        let current_target = journal["current_target"]
            .as_str()
            .expect("Journal 应记录 current_target");
        let bundle = Path::new(&plan.managed_directory);
        fs::create_dir(bundle).expect("应模拟 publish 已创建 Bundle");
        if shape != "empty-bundle" {
            fs::create_dir(bundle.join("contents")).expect("应模拟 publish 已创建 contents");
        }
        if matches!(shape, "content" | "temp-current" | "current") {
            fs::rename(
                harness
                    .data_root
                    .join("staging")
                    .join(transaction_id)
                    .join("candidate"),
                bundle.join("contents").join(content_id),
            )
            .expect("应模拟 candidate 已原子发布为 Content");
        }
        if matches!(shape, "temp-current" | "current") {
            let current_name = if shape == "current" {
                "current".to_owned()
            } else {
                format!(".current-{transaction_id}")
            };
            symlink(current_target, bundle.join(current_name)).expect("应模拟合法 current 软链接");
        }

        assert_takeover_restart_restores_original(&harness, &plan, &original);
    }
}

#[test]
fn candidate_and_published_content_together_are_blocked_and_preserved() {
    let harness = Harness::new();
    let original = harness.write_skill(".codex/skills/alpha", "alpha");
    fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
    let scanned = harness.scan();
    let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
    let plan = harness.create_plan(&id);
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        harness.paths.clone(),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterTakeoverCandidatePrepared,
    );
    interrupted
        .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
        .expect_err("候选准备后 failpoint 应留下 staging candidate");
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(only_takeover_journal(&harness)).expect("应读取 Journal"))
            .expect("Journal 应为 JSON");
    let transaction_id = journal["transaction_id"]
        .as_str()
        .expect("Journal 应记录 transaction_id");
    let content_id = journal["content_id"]
        .as_str()
        .expect("Journal 应记录 content_id");
    let current_target = journal["current_target"]
        .as_str()
        .expect("Journal 应记录 current_target");
    let candidate_skill = harness
        .data_root
        .join("staging")
        .join(transaction_id)
        .join("candidate/members/alpha");
    let bundle = Path::new(&plan.managed_directory);
    let published_skill = bundle
        .join("contents")
        .join(content_id)
        .join("members/alpha");
    fs::create_dir_all(&published_skill).expect("应模拟已发布 Content");
    for file in ["SKILL.md", "notes.txt"] {
        fs::copy(candidate_skill.join(file), published_skill.join(file))
            .expect("应复制完整候选内容");
    }
    symlink(current_target, bundle.join("current")).expect("应模拟正式 current");

    assert_takeover_restart_is_blocked_but_readable(&harness, "alpha");
    assert!(candidate_skill.is_dir(), "冲突 candidate 必须保留");
    assert!(published_skill.is_dir(), "冲突 published Content 必须保留");
}

#[test]
fn interrupted_publish_cleanup_resumes_from_partial_candidate() {
    for bundle_shape in ["missing", "empty-bundle", "empty-contents"] {
        let harness = Harness::new();
        let original = harness.write_skill(".codex/skills/alpha", "alpha");
        fs::write(original.join("notes.txt"), "original payload").expect("应写入 Skill 内容");
        let scanned = harness.scan();
        let id = observation_id(&scanned, "alpha", ScanRootKey::CodexGlobal);
        let plan = harness.create_plan(&id);
        let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
            harness.paths.clone(),
            PlatformInfo::supported_for_test(),
            LifecycleFailpoint::AfterTakeoverCandidatePrepared,
        );
        interrupted
            .confirm_takeover_plan(&plan.id, &[plan.paths[0].id.clone()])
            .expect_err("候选准备后 failpoint 应留下 staging candidate");
        let journal: serde_json::Value = serde_json::from_slice(
            &fs::read(only_takeover_journal(&harness)).expect("应读取 Journal"),
        )
        .expect("Journal 应为 JSON");
        let transaction_id = journal["transaction_id"]
            .as_str()
            .expect("Journal 应记录 transaction_id");
        let content_id = journal["content_id"]
            .as_str()
            .expect("Journal 应记录 content_id");
        let bundle = Path::new(&plan.managed_directory);
        let contents = bundle.join("contents");
        let staging = harness.data_root.join("staging").join(transaction_id);
        fs::create_dir_all(&contents).expect("应模拟 publish 创建 Bundle");
        fs::rename(staging.join("candidate"), contents.join(content_id))
            .expect("应模拟 Content 已发布");
        fs::rename(contents.join(content_id), staging.join("candidate"))
            .expect("应模拟回滚把 Content 原子移回 staging");
        match bundle_shape {
            "missing" => {
                fs::remove_dir(&contents).expect("应移除空 contents");
                fs::remove_dir(bundle).expect("应移除空 Bundle");
            }
            "empty-bundle" => {
                fs::remove_dir(&contents).expect("应留下空 Bundle");
            }
            "empty-contents" => {}
            _ => unreachable!("测试只构造已知 Bundle 骨架"),
        }
        fs::remove_file(staging.join("candidate/members/alpha/notes.txt"))
            .expect("应模拟 candidate 清理到一半时退出");

        assert_takeover_restart_restores_original(&harness, &plan, &original);
    }
}

fn only_takeover_journal(harness: &Harness) -> PathBuf {
    fs::read_dir(harness.data_root.join("journals"))
        .expect("Journal 目录应存在")
        .map(|entry| entry.expect("应读取 Journal 条目").path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !name.starts_with("mount-") && name.ends_with(".json"))
        })
        .expect("应存在唯一接管 Journal")
}

fn assert_takeover_restart_is_blocked_but_readable(harness: &Harness, name: &str) {
    let reopened =
        SkillYardApplication::new(harness.paths.clone(), PlatformInfo::supported_for_test());
    let UiOutcome::Inventory {
        entries,
        recovery_issues,
        ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("损坏事务只应阻塞自身，Inventory 仍应可读")
    else {
        panic!("阻塞恢复后应返回 Inventory");
    };
    assert!(entries.iter().any(|entry| entry.skill_name == name));
    assert_eq!(recovery_issues.len(), 1);
    let UiOutcome::Inventory {
        entries,
        recovery_issues,
        ..
    } = reopened
        .handle(UiIntent::GetStartupState)
        .expect("连续启动应继续隔离同一个损坏事务")
    else {
        panic!("连续阻塞恢复后仍应返回 Inventory");
    };
    assert!(entries.iter().any(|entry| entry.skill_name == name));
    assert_eq!(recovery_issues.len(), 1);
    let database = Connection::open(harness.database()).expect("应打开测试数据库");
    assert_eq!(
        database
            .query_row("SELECT status FROM takeover_transactions", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("应读取接管事务状态"),
        "blocked"
    );
}

fn assert_takeover_restart_finishes_install(
    harness: &Harness,
    plan: &TakeoverPlan,
    original: &Path,
    preserve_mount: bool,
) {
    let reopened =
        SkillYardApplication::new(harness.paths.clone(), PlatformInfo::supported_for_test());
    reopened
        .handle(UiIntent::GetStartupState)
        .expect("重启应前向完成已经生效的接管");
    reopened
        .handle(UiIntent::GetStartupState)
        .expect("前向恢复必须可以重复执行");
    if preserve_mount {
        let metadata = fs::symlink_metadata(original).expect("Host Mount 应存在");
        assert!(metadata.file_type().is_symlink());
        assert_eq!(
            fs::read_link(original).expect("Host Mount 应指向 Central Store"),
            Path::new(&plan.expected_target)
        );
    } else {
        assert!(!original.exists(), "未保留 Mount 时 Host 路径应保持不存在");
    }
    assert_eq!(
        fs::read_to_string(Path::new(&plan.expected_target).join("notes.txt"))
            .expect("Central Store 应保存原 Skill 内容"),
        "original payload"
    );
    let database = Connection::open(harness.database()).expect("应打开测试数据库");
    assert_eq!(table_count(&database, "bundles"), 1);
    assert_eq!(table_count(&database, "mounts"), i64::from(preserve_mount));
    drop(database);
    let notice = fs::read_to_string(harness.data_root.join("SKILLYARD-INFO.md"))
        .expect("前向恢复后应更新 Central Store 说明");
    assert!(notice.contains(&plan.bundle_display_name));
    assert!(notice.contains(&plan.managed_directory));
    assert_takeover_recovery_artifacts_clean(harness);
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
