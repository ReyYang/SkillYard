use std::fs;

use skillyard_lib::{
    AiPreferences, AiProvider, ApplicationPaths, InterfaceLanguage, PlatformInfo,
    SkillYardApplication, UiIntent, UiOutcome,
};
use tempfile::tempdir;

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
    let application = SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());

    assert_eq!(
        application
            .handle(UiIntent::GetPreferences)
            .expect("首次读取偏好应成功"),
        UiOutcome::Preferences {
            language: InterfaceLanguage::ZhCn,
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
            ai: default_ai_preferences(),
        }
    );

    let reopened = SkillYardApplication::new(paths, PlatformInfo::supported_for_test());
    assert_eq!(
        reopened
            .handle(UiIntent::GetPreferences)
            .expect("重启后读取偏好应成功"),
        UiOutcome::Preferences {
            language: InterfaceLanguage::En,
            ai: default_ai_preferences(),
        }
    );
    assert_eq!(
        fs::read(skill_file).expect("应读取未被修改的 Skill"),
        original
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
