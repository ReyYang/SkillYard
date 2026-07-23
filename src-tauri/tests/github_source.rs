use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, PlatformInfo, SkillYardApplication, SourceCatalogStatus, UiIntent, UiOutcome,
};
use tempfile::tempdir;

#[test]
fn recommended_github_sources_exist_without_creating_bundles_and_survive_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");

    let first = open_source_discovery(&application);
    assert_eq!(
        first
            .iter()
            .map(|source| (source.display_name.as_str(), source.tracked_ref.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("anthropics/skills", "main"),
            ("ComposioHQ/awesome-claude-skills", "master"),
            ("cexll/myclaude", "master"),
            ("JimLiu/baoyu-skills", "main"),
        ]
    );
    assert!(first.iter().all(|source| {
        source.catalog_status == SourceCatalogStatus::Unloaded
            && source.bundle_id.is_none()
            && source.members.is_empty()
    }));
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let counts = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM sources), (SELECT COUNT(*) FROM bundles)",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .expect("应读取 Source 与 Bundle 数量");
    assert_eq!(counts, (4, 0));
    drop(connection);

    drop(application);
    let restarted = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    assert_eq!(open_source_discovery(&restarted), first);
}

#[test]
fn deleting_a_recommended_source_does_not_seed_it_again_on_restart() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    application
        .handle(UiIntent::StartInitialScan)
        .expect("首次扫描应成功");
    drop(application);

    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    connection
        .execute(
            "DELETE FROM sources WHERE id = 'source-anthropics-skills'",
            [],
        )
        .expect("应模拟后续 Source 删除功能的领域结果");
    drop(connection);

    let restarted = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let sources = open_source_discovery(&restarted);
    assert_eq!(sources.len(), 3);
    assert!(
        sources
            .iter()
            .all(|source| source.id != "source-anthropics-skills")
    );
}

fn open_source_discovery(application: &SkillYardApplication) -> Vec<skillyard_lib::SourceSummary> {
    match application
        .handle(UiIntent::OpenSourceDiscovery)
        .expect("已完成首次扫描后应能浏览 Source")
    {
        UiOutcome::SourceDiscovery { sources, .. } => sources,
        _ => panic!("应返回 Source 发现状态"),
    }
}
