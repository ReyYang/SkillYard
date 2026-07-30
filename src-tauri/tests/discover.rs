use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use rusqlite::{Connection, params};
use skillyard_lib::{
    ApplicationPaths, PlatformInfo, SkillYardApplication, SourceCatalogStatus, SourceRequest,
    SourceResponse, SourceTransport, SourceTransportError, UiIntent, UiOutcome,
};
use tempfile::tempdir;

#[test]
fn discover_reads_local_inventory_and_saved_source_catalog_without_network_or_writes() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/tdd");
    fs::create_dir_all(&skill_root).expect("应创建本机 Skill");
    fs::write(
        skill_root.join("SKILL.md"),
        "---\nname: tdd\ndescription: 测试驱动开发工作流\n---\n# TDD\n",
    )
    .expect("应写入本机 Skill");

    let transport = Arc::new(CountingTransport::default());
    let application = SkillYardApplication::new_with_source_transport(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
        transport.clone(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开测试 SQLite");
    let (source_id, source_name): (String, String) = connection
        .query_row(
            "SELECT id, display_name FROM sources ORDER BY sort_order LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("默认 Source 应存在");
    connection
        .execute(
            "UPDATE sources
             SET catalog_status = 'fresh',
                 catalog_generation = 1,
                 catalog_marker = 'fixture-marker',
                 catalog_fetched_at = 100,
                 last_reload_at = 100,
                 last_reload_error = NULL,
                 updated_at = 100
             WHERE id = ?1",
            [&source_id],
        )
        .expect("应保存已加载 Source fixture");
    connection
        .execute(
            "INSERT INTO source_catalog_members (
                id, source_id, catalog_generation, relative_path, skill_name,
                description, content_fingerprint, selectable,
                validation_errors_json, warnings_json, sort_order
             ) VALUES (?1, ?2, 1, 'skills/research', 'research',
                       '研究公开资料并整理结论', 'fixture-fingerprint', 1, '[]', '[]', 0)",
            params!["source-member-fixture", &source_id],
        )
        .expect("应保存未安装 Source Member fixture");
    drop(connection);

    let UiOutcome::Discover {
        local_skills,
        sources,
    } = application
        .handle(UiIntent::OpenDiscover)
        .expect("发现页应只读打开")
    else {
        panic!("应返回独立 Discover read model");
    };

    assert_eq!(local_skills.len(), 1);
    assert_eq!(local_skills[0].skill_name, "tdd");
    assert_eq!(
        local_skills[0].description.as_deref(),
        Some("测试驱动开发工作流")
    );
    let source = sources
        .iter()
        .find(|source| source.id == source_id)
        .expect("已加载 Source 应出现在发现页");
    assert_eq!(source.display_name, source_name);
    assert_eq!(source.members.len(), 1);
    assert_eq!(source.members[0].skill_name.as_deref(), Some("research"));
    assert_eq!(source.members[0].installed_member_id, None);
    assert!(
        sources
            .iter()
            .any(|source| source.catalog_status == SourceCatalogStatus::Unloaded),
        "从未加载的 Source 也必须保留明确状态"
    );
    assert_eq!(
        transport.requests.load(Ordering::SeqCst),
        0,
        "打开与读取发现页不能访问 Source 网络"
    );
}

#[derive(Default)]
struct CountingTransport {
    requests: AtomicUsize,
}

impl SourceTransport for CountingTransport {
    fn get(&self, _request: SourceRequest) -> Result<SourceResponse, SourceTransportError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        Err(SourceTransportError::Unavailable)
    }
}
