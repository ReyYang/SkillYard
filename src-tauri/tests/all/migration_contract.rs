use std::{
    ffi::OsString,
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use skillyard_lib::{
    ApplicationPaths, ManagementKind, MountHealth, PlatformInfo, SkillYardApplication,
    SourceCatalogStatus, SourceKind, UiIntent, UiOutcome,
};
use tempfile::tempdir;

const RELEASE_DATABASE: &[u8] = include_bytes!("../fixtures/v1.0.1/skillyard.sqlite3");
const RELEASE_MANIFEST: &[u8] = include_bytes!("../../migrations/released-prefix.json");
const RELEASE_SEED: &[u8] = include_bytes!("../fixtures/v1.0.1/seed.sql");
const SNAPSHOT_METADATA: &str = include_str!("../fixtures/v1.0.1/snapshot.json");
const EXPECTED_MANIFEST_SHA256: &str =
    "be918484b3843edf40ebdf9ab404c5f8e37f52eca2efd7ab6330a3b3de946d7d";
const EXPECTED_SEED_SHA256: &str =
    "a3cd9c26252a6640a96f72d4e04fcb7fb5f7fcc4b8518a142c522b3810637510";
const EXPECTED_SNAPSHOT_SHA256: &str =
    "12f37791b50f673056e9f5bb5c6840c80e1e59a0e02329eb73ac15565ad805e3";
const EXPECTED_FIXTURE_SHA256: &str =
    "7d67529810956f8031764c52715de0bf0a716e7901b40b02e174a2971a04e47e";
const EXPECTED_TAG_OBJECT: &str = "2871263930d3dd94c42ab1af5782778471104e80";
const EXPECTED_PEELED_COMMIT: &str = "edc296411dcf38f33a7c4024e8761c61189d428e";
const EXPECTED_RELEASED_AGGREGATE: &str =
    "c7ca23bcfe64a21a38dbf2be0ed073e202251fd417bfb09af43d001888a48df1";
const EXPECTED_SCHEMA_SHA256: &str =
    "5b966f41550777cb196b3fff8312f7cb2895089b47a829c24136137806a0b759";
const EXPECTED_SEED_DATA_SHA256: &str =
    "1be5fcca1f5beac8512e178eabf03cfca3dc424bc14cb1dd7a1136229c121715";
const EXPECTED_SQLITE_VERSION: &str = "3.51.0 2025-06-12 13:14:41 f0ca7bba1c5e232e5d279fad6338121ab55af0c8c68c84cdfb18ba5114dcaapl (64-bit)";
const SCHEMA_FINGERPRINT_QUERY: &str = "SELECT quote(type)||'|'||quote(name)||'|'||quote(tbl_name)||'|'||quote(sql) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name,tbl_name,sql;";
const SCHEMA_ROW_ENCODING: &str =
    "Each UTF-8 TEXT result row followed by one LF, including after the final row.";
const DATA_QUERY_TEMPLATE: &str = "SELECT <quote(\"column\") joined by \" || '|' || \"> FROM \"<table>\" ORDER BY <\"column\" joined by \", \">;";
const DATA_ROW_ENCODING: &str = "For each table in listed order, emit <table>|<query TEXT row><LF>, including after the final row.";
const COMMITTED_FIXTURE_WAL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/v1.0.1/skillyard.sqlite3-wal"
);
const COMMITTED_FIXTURE_SHM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/v1.0.1/skillyard.sqlite3-shm"
);
const HOME_SENTINEL: &str = "/__skillyard_fixture__/home/.codex/skills/release-fixture";
const DATA_SENTINEL: &str =
    "/__skillyard_fixture__/data/bundles/bundle-v101/current/members/release-fixture";

struct DataTableSpec {
    table: &'static str,
    columns: &'static [&'static str],
    order_by: &'static [&'static str],
    row_count: u64,
}

const DATA_TABLES: &[DataTableSpec] = &[
    DataTableSpec {
        table: "app_state",
        columns: &[
            "singleton",
            "initial_scan_completed_at",
            "last_local_refresh_at",
            "last_local_refresh_added",
            "last_local_refresh_changed",
            "last_local_refresh_removed",
        ],
        order_by: &["singleton"],
        row_count: 1,
    },
    DataTableSpec {
        table: "supported_app_status",
        columns: &["app_id", "display_name", "detected", "sort_order"],
        order_by: &["app_id"],
        row_count: 3,
    },
    DataTableSpec {
        table: "bundles",
        columns: &[
            "id",
            "display_name",
            "managed_directory",
            "current_target",
            "created_at",
        ],
        order_by: &["id"],
        row_count: 1,
    },
    DataTableSpec {
        table: "skill_members",
        columns: &[
            "id",
            "bundle_id",
            "skill_name",
            "description",
            "stable_relative_path",
            "content_fingerprint",
            "created_at",
        ],
        order_by: &["id"],
        row_count: 1,
    },
    DataTableSpec {
        table: "member_selections",
        columns: &["bundle_id", "member_id", "selected_at"],
        order_by: &["bundle_id", "member_id"],
        row_count: 1,
    },
    DataTableSpec {
        table: "sources",
        columns: &[
            "id",
            "kind",
            "canonical_identity",
            "owner",
            "repository",
            "display_name",
            "locator",
            "tracked_ref",
            "member_path_hint",
            "filesystem_device",
            "filesystem_inode",
            "catalog_status",
            "catalog_generation",
            "catalog_marker",
            "catalog_fetched_at",
            "last_reload_at",
            "last_reload_error",
            "sort_order",
            "created_at",
            "updated_at",
        ],
        order_by: &["id"],
        row_count: 1,
    },
    DataTableSpec {
        table: "source_catalog_members",
        columns: &[
            "id",
            "source_id",
            "catalog_generation",
            "relative_path",
            "skill_name",
            "description",
            "content_fingerprint",
            "selectable",
            "validation_errors_json",
            "warnings_json",
            "sort_order",
        ],
        order_by: &["id"],
        row_count: 1,
    },
    DataTableSpec {
        table: "source_bundle_links",
        columns: &[
            "source_id",
            "bundle_id",
            "adopted_marker",
            "linked_at",
            "update_check_status",
            "update_checked_marker",
            "update_checked_at",
            "update_check_error",
        ],
        order_by: &["source_id"],
        row_count: 1,
    },
    DataTableSpec {
        table: "source_member_links",
        columns: &[
            "source_id",
            "source_relative_path",
            "member_id",
            "linked_at",
        ],
        order_by: &["source_id", "source_relative_path"],
        row_count: 1,
    },
    DataTableSpec {
        table: "mounts",
        columns: &[
            "id",
            "member_id",
            "app_id",
            "scope",
            "project_id",
            "target_path",
            "expected_target",
            "health",
            "created_at",
            "updated_at",
        ],
        order_by: &["id"],
        row_count: 1,
    },
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotContract {
    schema_version: u64,
    protocol_id: String,
    source_identity: String,
    release: SnapshotRelease,
    released_sql: SnapshotReleasedSql,
    sqlite_materialization: SnapshotSqliteMaterialization,
    seed: SnapshotSeed,
    determinism: SnapshotDeterminism,
    fixture: SnapshotFixture,
    schema_fingerprint: SnapshotSchemaFingerprint,
    seed_data_fingerprint: SnapshotSeedDataFingerprint,
    path_sentinels: Vec<SnapshotPathSentinel>,
    privacy: SnapshotPrivacy,
    disclaimers: SnapshotDisclaimers,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotRelease {
    version: String,
    tag: String,
    tag_object_oid: String,
    tag_object_signature: String,
    peeled_commit_oid: String,
    release_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotReleasedSql {
    manifest_path: String,
    manifest_sha256: String,
    migration_count: u64,
    versions: Vec<u64>,
    aggregate_rule: String,
    aggregate_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotSqliteMaterialization {
    binary: String,
    version: String,
    migration_transaction_rule: String,
    fixed_applied_at: i64,
    seed_transaction: String,
    vacuum_after_seed: bool,
    generation_connection_pragmas: SnapshotGenerationPragmas,
    reopened_persistent_pragmas: SnapshotPersistentPragmas,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotGenerationPragmas {
    page_size: i64,
    journal_mode: String,
    auto_vacuum: i64,
    foreign_keys: i64,
    synchronous: i64,
    fullfsync: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotPersistentPragmas {
    page_size: i64,
    journal_mode: String,
    auto_vacuum: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotSeed {
    path: String,
    sha256: String,
    fixed_timestamp: i64,
    purpose: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotDeterminism {
    fresh_initially_empty_runs: u64,
    retained_run_roots: Vec<String>,
    byte_identical: bool,
    first_sha256: String,
    second_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotFixture {
    path: String,
    size_bytes: u64,
    sha256: String,
    schema_migrations: Vec<u64>,
    integrity_check: String,
    foreign_key_check: Vec<serde_json::Value>,
    wal_or_shm_files: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotSchemaFingerprint {
    algorithm: String,
    object_count: u64,
    canonical_query: String,
    row_encoding: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotSeedDataFingerprint {
    algorithm: String,
    canonical_encoding: SnapshotCanonicalEncoding,
    row_counts: SnapshotRowCounts,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotCanonicalEncoding {
    query_template: String,
    row_encoding: String,
    tables: Vec<SnapshotDataTable>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotDataTable {
    table: String,
    columns: Vec<String>,
    order_by: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotRowCounts {
    app_state: u64,
    supported_app_status: u64,
    bundles: u64,
    skill_members: u64,
    member_selections: u64,
    sources: u64,
    source_catalog_members: u64,
    source_bundle_links: u64,
    source_member_links: u64,
    mounts: u64,
}

impl SnapshotRowCounts {
    fn for_table(&self, table: &str) -> Option<u64> {
        match table {
            "app_state" => Some(self.app_state),
            "supported_app_status" => Some(self.supported_app_status),
            "bundles" => Some(self.bundles),
            "skill_members" => Some(self.skill_members),
            "member_selections" => Some(self.member_selections),
            "sources" => Some(self.sources),
            "source_catalog_members" => Some(self.source_catalog_members),
            "source_bundle_links" => Some(self.source_bundle_links),
            "source_member_links" => Some(self.source_member_links),
            "mounts" => Some(self.mounts),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotPathSentinel {
    table: String,
    column: String,
    value: String,
    logical_row_count: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotPrivacy {
    machine_absolute_paths: bool,
    users_path_present: bool,
    credentials_present: bool,
    only_filesystem_absolute_values_are_the_two_recorded_sentinels: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotDisclaimers {
    not_captured_from_user_database: bool,
    not_claimed_byte_equal_to_release_app_database: bool,
    not_a_production_migration_runner: bool,
}

#[test]
fn v1_0_1_snapshot_upgrades_restarts_and_reads_core_state_through_application() {
    validate_release_fixture_contract(
        RELEASE_MANIFEST,
        RELEASE_SEED,
        SNAPSHOT_METADATA.as_bytes(),
        RELEASE_DATABASE,
    )
    .expect("committed v1.0.1 fixture inputs 应形成完整 contract");
    assert_fixture_contract_mutations_fail_closed();

    let sandbox = tempdir().expect("应创建真实隔离文件系统");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    fs::create_dir_all(&data_root).expect("应创建隔离 data root");
    fs::create_dir_all(&home).expect("应创建隔离 home");
    let database_path = data_root.join("skillyard.sqlite3");
    fs::write(&database_path, RELEASE_DATABASE).expect("应复制 v1.0.1 SQLite fixture");

    let expected_target = data_root.join("bundles/bundle-v101/current/members/release-fixture");
    let mount_target = home.join(".codex/skills/release-fixture");
    relocate_snapshot_sentinels(&database_path, &mount_target, &expected_target);
    create_release_fixture_filesystem(&data_root, &mount_target, &expected_target);

    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());
    let first = application
        .handle(UiIntent::GetStartupState)
        .expect("公开 startup seam 应升级 v1.0.1 fixture");
    assert_release_fixture_inventory(&first, &mount_target, &expected_target);

    let sources = application
        .handle(UiIntent::OpenSourceDiscovery)
        .expect("公开 Source seam 应读取升级后的关联");
    assert_release_fixture_source(&sources);
    assert_fixture_links(&data_root, &mount_target, &expected_target);

    drop(application);
    let restarted = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    let second = restarted
        .handle(UiIntent::GetStartupState)
        .expect("升级后的数据库应通过相同公开 seam 重启");
    assert_release_fixture_inventory(&second, &mount_target, &expected_target);
    let restarted_sources = restarted
        .handle(UiIntent::OpenSourceDiscovery)
        .expect("重启后 Source 关联应保持不变");
    assert_release_fixture_source(&restarted_sources);
    assert_fixture_links(&data_root, &mount_target, &expected_target);
    drop(restarted);

    let connection = Connection::open_with_flags(&database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("最终只能只读打开已升级 SQLite");
    assert_eq!(
        migration_versions(&connection),
        (1..=31).collect::<Vec<_>>()
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM bundles),
                        (SELECT COUNT(*) FROM skill_members),
                        (SELECT COUNT(*) FROM member_selections),
                        (SELECT COUNT(*) FROM sources),
                        (SELECT COUNT(*) FROM source_bundle_links),
                        (SELECT COUNT(*) FROM mounts)",
                [],
                |row| Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?
                )),
            )
            .expect("应只读确认核心数据没有重复"),
        (1, 1, 1, 1, 1, 1)
    );
}

fn validate_release_fixture_contract(
    manifest: &[u8],
    seed: &[u8],
    snapshot: &[u8],
    database: &[u8],
) -> Result<(), String> {
    require(
        !Path::new(COMMITTED_FIXTURE_WAL).exists() && !Path::new(COMMITTED_FIXTURE_SHM).exists(),
        "committed fixture 不得附带 WAL 或 SHM sidecar",
    )?;
    require(
        sha256_hex(manifest) == EXPECTED_MANIFEST_SHA256,
        "released manifest SHA-256 未匹配独立常量",
    )?;
    require(
        sha256_hex(seed) == EXPECTED_SEED_SHA256,
        "seed SHA-256 未匹配独立常量",
    )?;

    let manifest_json: serde_json::Value =
        serde_json::from_slice(manifest).map_err(|error| format!("manifest JSON: {error}"))?;
    let metadata: SnapshotContract =
        serde_json::from_slice(snapshot).map_err(|error| format!("snapshot schema: {error}"))?;
    let release_versions = (1..=26).collect::<Vec<u64>>();
    let manifest_versions = manifest_json["migrations"]
        .as_array()
        .ok_or_else(|| "manifest migrations 必须为数组".to_owned())?
        .iter()
        .map(|migration| {
            migration["version"]
                .as_u64()
                .ok_or_else(|| "manifest migration version 必须为整数".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    require(
        manifest_json["release"]["tag_object_oid"].as_str() == Some(EXPECTED_TAG_OBJECT),
        "manifest tag object 未匹配独立常量",
    )?;
    require(
        manifest_json["release"]["peeled_commit_oid"].as_str() == Some(EXPECTED_PEELED_COMMIT),
        "manifest peeled commit 未匹配独立常量",
    )?;
    require(
        manifest_json["release"]["tag_object_signature"]["status"].as_str() == Some("unsigned"),
        "manifest 必须记录 unsigned tag object",
    )?;
    require(
        manifest_json["canonical_bytes"]["sha256"].as_str() == Some(EXPECTED_RELEASED_AGGREGATE),
        "manifest released aggregate 未匹配独立常量",
    )?;
    require(
        manifest_versions == release_versions,
        "manifest versions 必须精确为 1..26",
    )?;

    require(
        metadata.schema_version == 1,
        "snapshot schema_version 必须为 1",
    )?;
    require(
        metadata.protocol_id == "skillyard-v1.0.1-release-sql-snapshot-v1",
        "snapshot protocol_id 不匹配",
    )?;
    require(
        metadata.source_identity == "deterministic_release_sql_materialization",
        "snapshot source_identity 不匹配",
    )?;
    require(
        metadata.release.version == "1.0.1",
        "snapshot release version 不匹配",
    )?;
    require(
        metadata.release.tag == "v1.0.1",
        "snapshot release tag 不匹配",
    )?;
    require(
        metadata.release.tag_object_oid == EXPECTED_TAG_OBJECT,
        "snapshot tag object 未匹配独立常量",
    )?;
    require(
        metadata.release.tag_object_signature == "unsigned",
        "snapshot tag object 必须为 unsigned",
    )?;
    require(
        metadata.release.peeled_commit_oid == EXPECTED_PEELED_COMMIT,
        "snapshot peeled commit 未匹配独立常量",
    )?;
    require(
        metadata.release.release_url == "https://github.com/ReyYang/SkillYard/releases/tag/v1.0.1",
        "snapshot release URL 不匹配",
    )?;

    require(
        metadata.released_sql.manifest_path == "src-tauri/migrations/released-prefix.json",
        "snapshot manifest path 不匹配",
    )?;
    require(
        metadata.released_sql.manifest_sha256 == EXPECTED_MANIFEST_SHA256
            && metadata.released_sql.manifest_sha256 == sha256_hex(manifest),
        "snapshot manifest SHA-256 未绑定 actual manifest",
    )?;
    require(
        metadata.released_sql.migration_count == 26,
        "snapshot migration_count 必须为 26",
    )?;
    require(
        metadata.released_sql.versions == release_versions,
        "snapshot released versions 必须精确为 1..26",
    )?;
    require(
        metadata.released_sql.aggregate_rule
            == "For each manifest entry in LC_ALL=C ascending repo-relative path order: lowercase SHA-256, two ASCII spaces, repo-relative path, LF.",
        "snapshot aggregate rule 不匹配",
    )?;
    require(
        metadata.released_sql.aggregate_sha256 == EXPECTED_RELEASED_AGGREGATE
            && manifest_json["canonical_bytes"]["sha256"].as_str()
                == Some(metadata.released_sql.aggregate_sha256.as_str()),
        "snapshot released aggregate 未绑定 manifest",
    )?;

    let sqlite = &metadata.sqlite_materialization;
    require(
        sqlite.binary == "sqlite3",
        "snapshot sqlite binary token 不匹配",
    )?;
    require(
        sqlite.version == EXPECTED_SQLITE_VERSION,
        "snapshot sqlite version 不匹配",
    )?;
    require(
        sqlite.migration_transaction_rule
            == "BEGIN IMMEDIATE; exact peeled-release SQL bytes; INSERT schema_migrations with fixed applied_at; COMMIT",
        "snapshot migration transaction rule 不匹配",
    )?;
    require(
        sqlite.fixed_applied_at == 1_700_000_000_000,
        "snapshot fixed applied_at 不匹配",
    )?;
    require(
        sqlite.seed_transaction == "BEGIN IMMEDIATE; exact seed.sql bytes; COMMIT",
        "snapshot seed transaction rule 不匹配",
    )?;
    require(sqlite.vacuum_after_seed, "snapshot 必须记录 seed 后 VACUUM")?;
    let generation = &sqlite.generation_connection_pragmas;
    require(
        generation.page_size == 4096
            && generation.journal_mode == "delete"
            && generation.auto_vacuum == 0
            && generation.foreign_keys == 1
            && generation.synchronous == 2
            && generation.fullfsync == 1,
        "snapshot generation PRAGMAs 不匹配",
    )?;
    let persistent = &sqlite.reopened_persistent_pragmas;
    require(
        persistent.page_size == 4096
            && persistent.journal_mode == "delete"
            && persistent.auto_vacuum == 0,
        "snapshot persistent PRAGMAs 不匹配",
    )?;

    require(
        metadata.seed.path == "src-tauri/tests/fixtures/v1.0.1/seed.sql",
        "snapshot seed path 不匹配",
    )?;
    require(
        metadata.seed.sha256 == EXPECTED_SEED_SHA256 && metadata.seed.sha256 == sha256_hex(seed),
        "snapshot seed SHA-256 未绑定 actual seed 与独立常量",
    )?;
    require(
        metadata.seed.fixed_timestamp == 1_700_000_000_000,
        "snapshot seed fixed timestamp 不匹配",
    )?;
    require(
        metadata.seed.purpose
            == "Synthetic core Inventory, Source, Bundle, Member selection, and Mount state; release-default unlinked recommendations are removed before the one fixture Source is inserted.",
        "snapshot seed purpose 不匹配",
    )?;

    require(
        metadata.fixture.path == "src-tauri/tests/fixtures/v1.0.1/skillyard.sqlite3",
        "snapshot fixture path 不匹配",
    )?;
    require(
        metadata.fixture.size_bytes == 561_152 && database.len() == 561_152,
        "snapshot fixture size 未绑定 actual binary",
    )?;
    require(
        metadata.fixture.sha256 == EXPECTED_FIXTURE_SHA256
            && sha256_hex(database) == EXPECTED_FIXTURE_SHA256,
        "snapshot fixture SHA-256 未绑定 actual binary 与独立常量",
    )?;
    require(
        metadata.fixture.schema_migrations == release_versions,
        "snapshot fixture versions 必须精确为 1..26",
    )?;
    require(
        metadata.fixture.integrity_check == "ok"
            && metadata.fixture.foreign_key_check.is_empty()
            && metadata.fixture.wal_or_shm_files.is_empty(),
        "snapshot fixture health metadata 不匹配",
    )?;
    require(
        metadata.determinism.fresh_initially_empty_runs == 2
            && metadata.determinism.retained_run_roots
                == ["$A4_SNAPSHOT_RUN_1", "$A4_SNAPSHOT_RUN_2"]
            && metadata.determinism.byte_identical
            && metadata.determinism.first_sha256 == EXPECTED_FIXTURE_SHA256
            && metadata.determinism.second_sha256 == EXPECTED_FIXTURE_SHA256,
        "snapshot deterministic runs 未绑定 fixture 常量",
    )?;

    require(
        metadata.schema_fingerprint.algorithm == "sha256"
            && metadata.schema_fingerprint.object_count == 81
            && metadata.schema_fingerprint.canonical_query == SCHEMA_FINGERPRINT_QUERY
            && metadata.schema_fingerprint.row_encoding == SCHEMA_ROW_ENCODING
            && metadata.schema_fingerprint.sha256 == EXPECTED_SCHEMA_SHA256,
        "snapshot schema fingerprint SHA-256 或 canonical metadata 不匹配独立常量",
    )?;
    require(
        metadata.seed_data_fingerprint.algorithm == "sha256"
            && metadata
                .seed_data_fingerprint
                .canonical_encoding
                .query_template
                == DATA_QUERY_TEMPLATE
            && metadata
                .seed_data_fingerprint
                .canonical_encoding
                .row_encoding
                == DATA_ROW_ENCODING,
        "snapshot seed-data canonical encoding 不匹配",
    )?;
    let metadata_tables = &metadata.seed_data_fingerprint.canonical_encoding.tables;
    require(
        metadata_tables.len() == DATA_TABLES.len(),
        "snapshot seed-data tables 数量不匹配",
    )?;
    for (actual, expected) in metadata_tables.iter().zip(DATA_TABLES) {
        require(
            actual.table == expected.table,
            "snapshot seed-data table 顺序不匹配",
        )?;
        require(
            actual
                .columns
                .iter()
                .map(String::as_str)
                .eq(expected.columns.iter().copied()),
            "snapshot seed-data columns 不匹配",
        )?;
        require(
            actual
                .order_by
                .iter()
                .map(String::as_str)
                .eq(expected.order_by.iter().copied()),
            "snapshot seed-data order_by 不匹配",
        )?;
        require(
            metadata
                .seed_data_fingerprint
                .row_counts
                .for_table(expected.table)
                == Some(expected.row_count),
            "snapshot seed-data row count 不匹配",
        )?;
    }
    require(
        metadata.seed_data_fingerprint.sha256 == EXPECTED_SEED_DATA_SHA256,
        "snapshot seed-data fingerprint SHA-256 不匹配独立常量",
    )?;

    require(
        metadata.path_sentinels.len() == 2
            && metadata.path_sentinels[0].table == "mounts"
            && metadata.path_sentinels[0].column == "target_path"
            && metadata.path_sentinels[0].value == HOME_SENTINEL
            && metadata.path_sentinels[0].logical_row_count == 1
            && metadata.path_sentinels[1].table == "mounts"
            && metadata.path_sentinels[1].column == "expected_target"
            && metadata.path_sentinels[1].value == DATA_SENTINEL
            && metadata.path_sentinels[1].logical_row_count == 1,
        "snapshot path sentinels 不匹配",
    )?;
    require(
        !metadata.privacy.machine_absolute_paths
            && !metadata.privacy.users_path_present
            && !metadata.privacy.credentials_present
            && metadata
                .privacy
                .only_filesystem_absolute_values_are_the_two_recorded_sentinels,
        "snapshot privacy contract 不匹配",
    )?;
    require(
        metadata.disclaimers.not_captured_from_user_database
            && metadata
                .disclaimers
                .not_claimed_byte_equal_to_release_app_database
            && metadata.disclaimers.not_a_production_migration_runner,
        "snapshot disclaimers 不完整",
    )?;
    require(
        sha256_hex(snapshot) == EXPECTED_SNAPSHOT_SHA256,
        "snapshot metadata bytes 未匹配独立常量",
    )?;

    validate_release_database(database, &metadata)
}

fn validate_release_database(database: &[u8], metadata: &SnapshotContract) -> Result<(), String> {
    let raw = String::from_utf8_lossy(database);
    let privacy_scan = raw.replace(HOME_SENTINEL, "").replace(DATA_SENTINEL, "");
    for forbidden in [
        "/Users/",
        "/home/",
        "/var/folders/",
        "/private/var/",
        "C:\\Users\\",
        "-----BEGIN PRIVATE KEY-----",
        "-----BEGIN OPENSSH PRIVATE KEY-----",
        "github_pat_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "Authorization: Bearer ",
    ] {
        require(
            !privacy_scan.contains(forbidden),
            &format!("fixture 包含禁止的机器路径或凭据标记: {forbidden}"),
        )?;
    }

    let sandbox = tempdir().map_err(|error| format!("fixture validation tempdir: {error}"))?;
    let database_path = sandbox.path().join("skillyard.sqlite3");
    fs::write(&database_path, database).map_err(|error| format!("写入 fixture 副本: {error}"))?;
    require_no_sqlite_sidecars(&database_path)?;
    let connection = Connection::open_with_flags(&database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("只读打开 fixture: {error}"))?;

    require(
        query_migration_versions(&connection)? == (1..=26).collect::<Vec<i64>>(),
        "fixture schema_migrations 必须精确为 1..26",
    )?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| format!("PRAGMA integrity_check: {error}"))?;
    require(integrity == "ok", "fixture integrity_check 必须为 ok")?;
    let mut foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(|error| format!("prepare foreign_key_check: {error}"))?;
    let mut foreign_key_rows = foreign_keys
        .query([])
        .map_err(|error| format!("query foreign_key_check: {error}"))?;
    require(
        foreign_key_rows
            .next()
            .map_err(|error| format!("read foreign_key_check: {error}"))?
            .is_none(),
        "fixture foreign_key_check 必须为空",
    )?;
    drop(foreign_key_rows);
    drop(foreign_keys);

    let page_size: i64 = connection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|error| format!("PRAGMA page_size: {error}"))?;
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(|error| format!("PRAGMA journal_mode: {error}"))?;
    let auto_vacuum: i64 = connection
        .query_row("PRAGMA auto_vacuum", [], |row| row.get(0))
        .map_err(|error| format!("PRAGMA auto_vacuum: {error}"))?;
    require(
        page_size
            == metadata
                .sqlite_materialization
                .reopened_persistent_pragmas
                .page_size
            && journal_mode
                == metadata
                    .sqlite_materialization
                    .reopened_persistent_pragmas
                    .journal_mode
            && auto_vacuum
                == metadata
                    .sqlite_materialization
                    .reopened_persistent_pragmas
                    .auto_vacuum,
        "fixture persistent PRAGMAs 与 metadata 不匹配",
    )?;

    let schema_rows = query_text_rows(&connection, SCHEMA_FINGERPRINT_QUERY)?;
    require(
        schema_rows.len() as u64 == metadata.schema_fingerprint.object_count,
        "fixture schema object count 与 metadata 不匹配",
    )?;
    let schema_bytes = canonical_rows(&schema_rows, None);
    require(
        sha256_hex(&schema_bytes) == EXPECTED_SCHEMA_SHA256
            && sha256_hex(&schema_bytes) == metadata.schema_fingerprint.sha256,
        "fixture schema fingerprint SHA-256 不匹配",
    )?;

    let mut seed_data_bytes = Vec::new();
    for table in DATA_TABLES {
        let expression = table
            .columns
            .iter()
            .map(|column| format!("quote({})", quote_identifier(column)))
            .collect::<Vec<_>>()
            .join(" || '|' || ");
        let order_by = table
            .order_by
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT {expression} FROM {} ORDER BY {order_by};",
            quote_identifier(table.table)
        );
        let rows = query_text_rows(&connection, &query)?;
        require(
            rows.len() as u64 == table.row_count
                && metadata
                    .seed_data_fingerprint
                    .row_counts
                    .for_table(table.table)
                    == Some(rows.len() as u64),
            &format!("fixture {} row count 不匹配", table.table),
        )?;
        seed_data_bytes.extend(canonical_rows(&rows, Some(table.table)));
    }
    require(
        sha256_hex(&seed_data_bytes) == EXPECTED_SEED_DATA_SHA256
            && sha256_hex(&seed_data_bytes) == metadata.seed_data_fingerprint.sha256,
        "fixture seed-data fingerprint SHA-256 不匹配",
    )?;

    let sentinel_counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM mounts WHERE target_path = ?1),
                (SELECT COUNT(*) FROM mounts WHERE expected_target = ?2)",
            (HOME_SENTINEL, DATA_SENTINEL),
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(|error| format!("读取 fixture sentinels: {error}"))?;
    require(
        sentinel_counts == (1, 1),
        "fixture 两个 path sentinel 必须各有一条逻辑记录",
    )?;
    let mount_paths = connection
        .query_row(
            "SELECT target_path, expected_target FROM mounts",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|error| format!("读取 fixture mount paths: {error}"))?;
    require(
        mount_paths == (HOME_SENTINEL.to_owned(), DATA_SENTINEL.to_owned()),
        "fixture Mount 只能包含两个记录的 path sentinel",
    )?;

    drop(connection);
    require_no_sqlite_sidecars(&database_path)
}

fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn query_text_rows(connection: &Connection, query: &str) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(query)
        .map_err(|error| format!("prepare canonical query: {error}"))?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(|error| format!("run canonical query: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read canonical query: {error}"))
}

fn canonical_rows(rows: &[String], table: Option<&str>) -> Vec<u8> {
    let mut bytes = Vec::new();
    for row in rows {
        if let Some(table) = table {
            bytes.extend_from_slice(table.as_bytes());
            bytes.push(b'|');
        }
        bytes.extend_from_slice(row.as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn sqlite_sidecar_path(database_path: &Path, suffix: &str) -> PathBuf {
    let mut path = OsString::from(database_path.as_os_str());
    path.push(suffix);
    PathBuf::from(path)
}

fn require_no_sqlite_sidecars(database_path: &Path) -> Result<(), String> {
    for suffix in ["-wal", "-shm"] {
        require(
            !sqlite_sidecar_path(database_path, suffix).exists(),
            &format!("fixture 不得产生 SQLite {suffix} sidecar"),
        )?;
    }
    Ok(())
}

fn query_migration_versions(connection: &Connection) -> Result<Vec<i64>, String> {
    let mut statement = connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .map_err(|error| format!("prepare migration versions: {error}"))?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(|error| format!("query migration versions: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read migration versions: {error}"))
}

fn assert_fixture_contract_mutations_fail_closed() {
    let mut seed = RELEASE_SEED.to_vec();
    seed.extend_from_slice(b"\n-- seed drift\n");
    let mut seed_snapshot: serde_json::Value =
        serde_json::from_str(SNAPSHOT_METADATA).expect("snapshot metadata 应为有效 JSON");
    seed_snapshot["seed"]["sha256"] =
        serde_json::Value::String(format!("{:x}", Sha256::digest(&seed)));
    assert_contract_rejection(
        "seed drift",
        "seed SHA-256",
        RELEASE_MANIFEST,
        &seed,
        &serde_json::to_vec(&seed_snapshot).expect("变异 metadata 应可编码"),
        RELEASE_DATABASE,
    );

    let mut manifest_snapshot: serde_json::Value =
        serde_json::from_str(SNAPSHOT_METADATA).expect("snapshot metadata 应为有效 JSON");
    manifest_snapshot["released_sql"]["manifest_sha256"] =
        serde_json::Value::String("0".repeat(64));
    assert_contract_rejection(
        "snapshot manifest-SHA drift",
        "manifest SHA-256",
        RELEASE_MANIFEST,
        RELEASE_SEED,
        &serde_json::to_vec(&manifest_snapshot).expect("变异 metadata 应可编码"),
        RELEASE_DATABASE,
    );

    for field in ["schema_fingerprint", "seed_data_fingerprint"] {
        let mut fingerprint_snapshot: serde_json::Value =
            serde_json::from_str(SNAPSHOT_METADATA).expect("snapshot metadata 应为有效 JSON");
        fingerprint_snapshot[field]["sha256"] = serde_json::Value::String("0".repeat(64));
        assert_contract_rejection(
            &format!("metadata {field} drift"),
            if field == "schema_fingerprint" {
                "schema fingerprint SHA-256"
            } else {
                "seed-data fingerprint SHA-256"
            },
            RELEASE_MANIFEST,
            RELEASE_SEED,
            &serde_json::to_vec(&fingerprint_snapshot).expect("变异 metadata 应可编码"),
            RELEASE_DATABASE,
        );
    }

    let mut database = RELEASE_DATABASE.to_vec();
    let last = database.last_mut().expect("fixture 不应为空");
    *last ^= 1;
    let mutated_sha = format!("{:x}", Sha256::digest(&database));
    let mut database_snapshot: serde_json::Value =
        serde_json::from_str(SNAPSHOT_METADATA).expect("snapshot metadata 应为有效 JSON");
    database_snapshot["fixture"]["sha256"] = serde_json::Value::String(mutated_sha.clone());
    database_snapshot["determinism"]["first_sha256"] =
        serde_json::Value::String(mutated_sha.clone());
    database_snapshot["determinism"]["second_sha256"] = serde_json::Value::String(mutated_sha);
    assert_contract_rejection(
        "binary and metadata synchronized drift",
        "fixture SHA-256",
        RELEASE_MANIFEST,
        RELEASE_SEED,
        &serde_json::to_vec(&database_snapshot).expect("变异 metadata 应可编码"),
        &database,
    );
}

fn assert_contract_rejection(
    label: &str,
    expected_error: &str,
    manifest: &[u8],
    seed: &[u8],
    snapshot: &[u8],
    database: &[u8],
) {
    let error = validate_release_fixture_contract(manifest, seed, snapshot, database)
        .expect_err("变异 fixture contract 必须 fail closed");
    assert!(
        error.contains(expected_error),
        "{label} 应由 {expected_error} guard 拒绝，实际错误：{error}"
    );
}

fn relocate_snapshot_sentinels(database_path: &Path, mount_target: &Path, expected_target: &Path) {
    let mut connection = Connection::open(database_path).expect("应打开 fixture 副本");
    assert_eq!(
        migration_versions(&connection),
        (1..=26).collect::<Vec<_>>()
    );
    let sentinel_counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM mounts WHERE target_path = ?1),
                (SELECT COUNT(*) FROM mounts WHERE expected_target = ?2)",
            (HOME_SENTINEL, DATA_SENTINEL),
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .expect("应读取两个固定 sentinel");
    assert_eq!(sentinel_counts, (1, 1));

    let transaction = connection
        .transaction()
        .expect("sentinel relocation 应为一个事务");
    assert_eq!(
        transaction
            .execute(
                "UPDATE mounts SET target_path = ?1 WHERE target_path = ?2",
                (mount_target.to_string_lossy().as_ref(), HOME_SENTINEL),
            )
            .expect("应只 relocate mount target sentinel"),
        1
    );
    assert_eq!(
        transaction
            .execute(
                "UPDATE mounts SET expected_target = ?1 WHERE expected_target = ?2",
                (expected_target.to_string_lossy().as_ref(), DATA_SENTINEL),
            )
            .expect("应只 relocate managed target sentinel"),
        1
    );
    transaction
        .commit()
        .expect("应提交两个 sentinel relocation");
}

fn create_release_fixture_filesystem(
    data_root: &Path,
    mount_target: &Path,
    expected_target: &Path,
) {
    let bundle_root = data_root.join("bundles/bundle-v101");
    let member_root = bundle_root.join("contents/release-content/members/release-fixture");
    fs::create_dir_all(&member_root).expect("应创建真实 Central Content");
    fs::write(
        member_root.join("SKILL.md"),
        "---\nname: release-fixture\ndescription: deterministic v1.0.1 fixture\n---\n# Release fixture\n",
    )
    .expect("应写入真实 Skill 内容");
    symlink("contents/release-content", bundle_root.join("current"))
        .expect("应创建真实 current symlink");
    fs::create_dir_all(mount_target.parent().expect("Mount 应有父目录"))
        .expect("应创建真实 Mount 父目录");
    symlink(expected_target, mount_target).expect("应创建真实 Mount symlink");
}

fn assert_release_fixture_inventory(
    outcome: &UiOutcome,
    mount_target: &Path,
    expected_target: &Path,
) {
    let UiOutcome::Inventory {
        entries,
        mounts,
        recovery_issues,
        recovered_interrupted_operation,
        ..
    } = outcome
    else {
        panic!("v1.0.1 fixture 启动后应返回 Inventory");
    };
    assert!(recovery_issues.is_empty());
    assert!(!recovered_interrupted_operation);
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.skill_name, "release-fixture");
    assert_eq!(entry.member_id.as_deref(), Some("member-release-fixture"));
    assert_eq!(entry.bundle_id.as_deref(), Some("bundle-v101"));
    assert_eq!(
        entry.bundle_display_name.as_deref(),
        Some("Release Fixture Bundle")
    );
    assert_eq!(
        entry.source_display_name.as_deref(),
        Some("skillyard-fixture/release-fixture")
    );
    assert_eq!(entry.management_kind, ManagementKind::SkillYardManaged);
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].id, "mount-release-fixture");
    assert_eq!(mounts[0].health, MountHealth::Healthy);
    assert_eq!(mounts[0].target_path, mount_target.to_string_lossy());
    assert_eq!(mounts[0].expected_target, expected_target.to_string_lossy());
}

fn assert_release_fixture_source(outcome: &UiOutcome) {
    let UiOutcome::SourceDiscovery { sources, .. } = outcome else {
        panic!("公开 Source seam 应返回 SourceDiscovery");
    };
    assert_eq!(sources.len(), 1);
    let source = &sources[0];
    assert_eq!(source.id, "source-release-fixture");
    assert_eq!(source.kind, SourceKind::Github);
    assert_eq!(source.catalog_status, SourceCatalogStatus::Fresh);
    assert_eq!(source.bundle_id.as_deref(), Some("bundle-v101"));
    assert_eq!(source.members.len(), 1);
    assert_eq!(
        source.members[0].installed_member_id.as_deref(),
        Some("member-release-fixture")
    );
}

fn assert_fixture_links(data_root: &Path, mount_target: &Path, expected_target: &Path) {
    assert_eq!(
        fs::read_link(data_root.join("bundles/bundle-v101/current"))
            .expect("current symlink 应保持存在"),
        Path::new("contents/release-content")
    );
    assert_eq!(
        fs::read_link(mount_target).expect("Mount symlink 应保持存在"),
        expected_target
    );
    assert!(expected_target.join("SKILL.md").is_file());
}

fn migration_versions(connection: &Connection) -> Vec<i64> {
    let mut statement = connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .expect("应读取 migration versions");
    statement
        .query_map([], |row| row.get(0))
        .expect("应查询 migration versions")
        .collect::<Result<Vec<_>, _>>()
        .expect("migration versions 应有效")
}
