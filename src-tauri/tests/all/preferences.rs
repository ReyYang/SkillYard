use std::{
    collections::BTreeMap,
    fs,
    sync::{Arc, Mutex},
};

use rusqlite::Connection;
use skillyard_lib::{
    AgentProviderEndpoints, AiPreferences, AiProvider, ApplicationPaths, InterfaceLanguage,
    PlatformInfo, SecretStore, SecretStoreError, SkillYardApplication, ThemePreset, UiIntent,
    UiOutcome,
};
use tempfile::tempdir;

const RELEASE_DATABASE: &[u8] = include_bytes!("../fixtures/v1.0.1/skillyard.sqlite3");
const SCHEMA_30_ARCHIVE_THEME: &str = include_str!("../fixtures/schema30_archive_theme.sql");

#[test]
fn interface_language_is_saved_through_the_application_seam_and_restored() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let skill_root = home.join(".codex/skills/example");
    fs::create_dir_all(&skill_root).expect("应创建 Skill fixture");
    let skill_file = skill_root.join("SKILL.md");
    let original = b"---\nname: example\ndescription: language fixture\n---\n";
    fs::write(&skill_file, original).expect("应写入 Skill fixture");

    let paths = ApplicationPaths::for_home(data_root, home);
    let secrets = Arc::new(FixtureSecretStore::default());
    let application = SkillYardApplication::new_with_agent_dependencies(
        paths.clone(),
        PlatformInfo::supported_for_test(),
        secrets.clone(),
        AgentProviderEndpoints::for_test("http://127.0.0.1:9".to_owned()),
    );

    assert_eq!(
        application
            .handle(UiIntent::GetPreferences)
            .expect("首次读取偏好应成功"),
        UiOutcome::Preferences {
            language: InterfaceLanguage::ZhCn,
            theme: ThemePreset::Ledger,
            ai: default_ai_preferences(),
        }
    );

    assert_eq!(
        application
            .handle(UiIntent::SetThemePreset {
                theme: ThemePreset::Layers,
            })
            .expect("保存主题偏好应成功"),
        UiOutcome::Preferences {
            language: InterfaceLanguage::ZhCn,
            theme: ThemePreset::Layers,
            ai: default_ai_preferences(),
        }
    );

    assert_eq!(
        application
            .handle(UiIntent::SetInterfaceLanguage {
                language: InterfaceLanguage::En,
            })
            .expect("保存英文偏好应成功"),
        UiOutcome::Preferences {
            language: InterfaceLanguage::En,
            theme: ThemePreset::Layers,
            ai: default_ai_preferences(),
        }
    );

    let reopened = SkillYardApplication::new_with_agent_dependencies(
        paths,
        PlatformInfo::supported_for_test(),
        secrets,
        AgentProviderEndpoints::for_test("http://127.0.0.1:9".to_owned()),
    );
    assert_eq!(
        reopened
            .handle(UiIntent::GetPreferences)
            .expect("重启后读取偏好应成功"),
        UiOutcome::Preferences {
            language: InterfaceLanguage::En,
            theme: ThemePreset::Layers,
            ai: default_ai_preferences(),
        }
    );
    assert_eq!(
        fs::read(skill_file).expect("应读取未被修改的 Skill"),
        original
    );
}

#[test]
fn archive_theme_from_schema_30_is_normalized_and_restored_as_ledger() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let home = sandbox.path().join("home");
    let data_root = sandbox.path().join("application-support/SkillYard");
    let paths = ApplicationPaths::for_home(data_root.clone(), home);
    let secrets = Arc::new(FixtureSecretStore::default());
    fs::create_dir_all(&data_root).expect("应创建真实 SQLite fixture 目录");
    let database = data_root.join("skillyard.sqlite3");
    fs::write(&database, RELEASE_DATABASE).expect("应写入已发布 SQLite fixture");
    let connection = Connection::open(&database).expect("应打开 schema 30 SQLite fixture");
    connection
        .execute_batch(include_str!("../../migrations/0027_interface_language.sql"))
        .expect("应应用正式 migration 27");
    connection
        .execute_batch(include_str!("../../migrations/0028_ai_preferences.sql"))
        .expect("应应用正式 migration 28");
    connection
        .execute_batch(include_str!(
            "../../migrations/0029_skill_ai_explanations.sql"
        ))
        .expect("应应用正式 migration 29");
    connection
        .execute_batch(SCHEMA_30_ARCHIVE_THEME)
        .expect("应应用三主题开发版 migration 30 fixture");
    for version in 27..=30 {
        connection
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, unixepoch())",
                [version],
            )
            .expect("应记录 schema 30 fixture migration");
    }
    assert_eq!(
        connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("应读取 fixture schema version"),
        30
    );
    drop(connection);

    let upgraded = SkillYardApplication::new_with_agent_dependencies(
        paths.clone(),
        PlatformInfo::supported_for_test(),
        secrets.clone(),
        AgentProviderEndpoints::for_test("http://127.0.0.1:9".to_owned()),
    );
    assert_eq!(
        upgraded
            .handle(UiIntent::GetPreferences)
            .expect("升级后读取旧主题偏好应成功"),
        UiOutcome::Preferences {
            language: InterfaceLanguage::En,
            theme: ThemePreset::Ledger,
            ai: default_ai_preferences(),
        }
    );
    drop(upgraded);

    let upgraded_connection = Connection::open(&database).expect("应重开升级后的 SQLite");
    assert_eq!(
        upgraded_connection
            .query_row(
                "SELECT theme_preset FROM app_preferences WHERE singleton_id = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("应读取已归一化的持久主题偏好"),
        "ledger"
    );
    assert!(
        upgraded_connection
            .execute(
                "UPDATE app_preferences SET theme_preset = 'archive' WHERE singleton_id = 1",
                [],
            )
            .is_err(),
        "升级后的 SQLite CHECK 必须拒绝已删除的 archive 主题"
    );
    drop(upgraded_connection);

    let reopened = SkillYardApplication::new_with_agent_dependencies(
        paths,
        PlatformInfo::supported_for_test(),
        secrets,
        AgentProviderEndpoints::for_test("http://127.0.0.1:9".to_owned()),
    );
    assert_eq!(
        reopened
            .handle(UiIntent::GetPreferences)
            .expect("再次重启后读取已归一化主题偏好应成功"),
        UiOutcome::Preferences {
            language: InterfaceLanguage::En,
            theme: ThemePreset::Ledger,
            ai: default_ai_preferences(),
        }
    );
}

fn default_ai_preferences() -> AiPreferences {
    AiPreferences {
        enabled: false,
        disclosure_accepted: false,
        provider: AiProvider::OpenAi,
        model: "gpt-5.6-terra".to_owned(),
        has_api_key: false,
        verified: false,
    }
}

#[derive(Default)]
struct FixtureSecretStore {
    values: Mutex<BTreeMap<String, String>>,
}

impl SecretStore for FixtureSecretStore {
    fn read(&self, account: &str) -> Result<Option<String>, SecretStoreError> {
        Ok(self
            .values
            .lock()
            .expect("fixture secret lock 应可用")
            .get(account)
            .cloned())
    }

    fn write(&self, account: &str, value: &str) -> Result<(), SecretStoreError> {
        self.values
            .lock()
            .expect("fixture secret lock 应可用")
            .insert(account.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretStoreError> {
        self.values
            .lock()
            .expect("fixture secret lock 应可用")
            .remove(account);
        Ok(())
    }
}
