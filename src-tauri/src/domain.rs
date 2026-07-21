use serde::{Deserialize, Serialize};

/// UI 只能通过这个封闭枚举表达业务意图，不能传入任意文件操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UiIntent {
    GetStartupState,
    StartInitialScan,
}

/// 固定 Supported App 的稳定标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportedAppId {
    Codex,
    ClaudeCode,
    GitHubCopilot,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedAppSummary {
    pub id: SupportedAppId,
    pub display_name: String,
    /// 首次扫描前不读取检测路径，因此这里保持未知。
    pub detected: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryObservation {
    pub id: String,
    pub skill_name: String,
    pub declared_name: Option<String>,
    pub skill_root: String,
    pub skill_file: String,
    pub location_kind: InventoryLocationKind,
    pub metadata_status: SkillMetadataStatus,
    pub observed_by: Vec<SupportedAppId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InventoryLocationKind {
    AppGlobal,
    SharedReadOnly,
}

impl InventoryLocationKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AppGlobal => "app_global",
            Self::SharedReadOnly => "shared_read_only",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "app_global" => Some(Self::AppGlobal),
            "shared_read_only" => Some(Self::SharedReadOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillMetadataStatus {
    Valid,
    Invalid,
    Unreadable,
}

impl SkillMetadataStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::Unreadable => "unreadable",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "valid" => Some(Self::Valid),
            "invalid" => Some(Self::Invalid),
            "unreadable" => Some(Self::Unreadable),
            _ => None,
        }
    }
}

/// Rust Core 返回给界面的完整可观察状态。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UiOutcome {
    UnsupportedPlatform {
        actual_os: String,
        actual_architecture: String,
        actual_major_version: u64,
        required_architecture: String,
        minimum_major_version: u64,
    },
    OnboardingRequired {
        supported_apps: Vec<SupportedAppSummary>,
    },
    Inventory {
        scan_completed_at: i64,
        entries: Vec<InventoryObservation>,
        supported_apps: Vec<SupportedAppSummary>,
    },
}

impl UiOutcome {
    pub fn onboarding_required() -> Self {
        Self::OnboardingRequired {
            supported_apps: supported_app_summaries(),
        }
    }
}

pub(crate) fn supported_app_summaries() -> Vec<SupportedAppSummary> {
    vec![
        SupportedAppSummary {
            id: SupportedAppId::Codex,
            display_name: "Codex".to_owned(),
            detected: None,
        },
        SupportedAppSummary {
            id: SupportedAppId::ClaudeCode,
            display_name: "Claude Code".to_owned(),
            detected: None,
        },
        SupportedAppSummary {
            id: SupportedAppId::GitHubCopilot,
            display_name: "GitHub Copilot".to_owned(),
            detected: None,
        },
    ]
}

impl SupportedAppId {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude_code",
            Self::GitHubCopilot => "github_copilot",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "codex" => Some(Self::Codex),
            "claude_code" => Some(Self::ClaudeCode),
            "github_copilot" => Some(Self::GitHubCopilot),
            _ => None,
        }
    }
}

/// 平台信息可在测试中注入，生产环境会在后续 Tauri 入口中读取真实系统。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformInfo {
    pub os: String,
    pub architecture: String,
    pub major_version: u64,
}

impl PlatformInfo {
    pub fn supported_for_test() -> Self {
        Self {
            os: "macos".to_owned(),
            architecture: "aarch64".to_owned(),
            major_version: 14,
        }
    }

    pub fn current() -> Self {
        let info = os_info::get();
        let os = if info.os_type() == os_info::Type::Macos {
            "macos".to_owned()
        } else {
            info.os_type().to_string().to_lowercase()
        };
        let major_version = match info.version() {
            os_info::Version::Semantic(major, _, _) => *major,
            other => other
                .to_string()
                .split('.')
                .next()
                .and_then(|part| part.parse().ok())
                .unwrap_or_default(),
        };

        Self {
            os,
            architecture: std::env::consts::ARCH.to_owned(),
            major_version,
        }
    }

    pub(crate) fn is_supported(&self) -> bool {
        self.os == "macos" && self.architecture == "aarch64" && self.major_version >= 14
    }
}
