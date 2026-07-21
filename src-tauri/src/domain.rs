use serde::{Deserialize, Serialize};

/// UI 只能通过这个封闭枚举表达业务意图，不能传入任意文件操作。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UiIntent {
    GetStartupState,
    StartInitialScan,
    RefreshLocalInventory,
    CreateFolderInstallPlan {
        input_path: String,
    },
    ConfirmInstallPlan {
        plan_id: String,
        selected_candidate_ids: Vec<String>,
    },
    RegisterProject {
        root_path: String,
    },
    CreateMountPlan {
        member_id: String,
        app_id: SupportedAppId,
        scope: MountScope,
        project_id: Option<String>,
    },
    CreateBatchMountPlan {
        bundle_id: String,
        requests: Vec<BatchMountRequest>,
    },
    CreateRemoveMountPlan {
        mount_id: String,
    },
    CreateRepairMountPlan {
        mount_id: String,
    },
    ConfirmMountPlan {
        plan_id: String,
    },
    ConfirmBatchMountPlan {
        plan_id: String,
        selected_item_ids: Vec<String>,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MountScope {
    Global,
    Project,
}

impl MountScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "global" => Some(Self::Global),
            "project" => Some(Self::Project),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MountOperation {
    Create,
    Remove,
}

/// Plan 的用户意图与底层文件效果分开；修复和首次创建都使用同一套安全创建事务。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MountPlanPurpose {
    Create,
    Repair,
    Remove,
}

impl MountPlanPurpose {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Repair => "repair",
            Self::Remove => "remove",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "create" => Some(Self::Create),
            "repair" => Some(Self::Repair),
            "remove" => Some(Self::Remove),
            _ => None,
        }
    }

    pub(crate) fn operation(self) -> MountOperation {
        match self {
            Self::Create | Self::Repair => MountOperation::Create,
            Self::Remove => MountOperation::Remove,
        }
    }
}

impl MountOperation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Remove => "remove",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "create" => Some(Self::Create),
            "remove" => Some(Self::Remove),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MountHealth {
    Healthy,
    Missing,
    Conflict,
}

impl MountHealth {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Missing => "missing",
            Self::Conflict => "conflict",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "healthy" => Some(Self::Healthy),
            "missing" => Some(Self::Missing),
            "conflict" => Some(Self::Conflict),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: String,
    pub display_name: String,
    pub root_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MountSummary {
    pub id: String,
    pub member_id: String,
    pub skill_name: String,
    pub app_id: SupportedAppId,
    pub scope: MountScope,
    pub project_id: Option<String>,
    pub project_display_name: Option<String>,
    pub target_path: String,
    pub expected_target: String,
    pub health: MountHealth,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MountPlan {
    pub id: String,
    pub operation: MountOperation,
    pub purpose: MountPlanPurpose,
    pub mount_id: String,
    pub member_id: String,
    pub skill_name: String,
    pub app_id: SupportedAppId,
    pub scope: MountScope,
    pub project_id: Option<String>,
    pub project_display_name: Option<String>,
    pub target_path: String,
    pub expected_target: String,
    pub target_health: MountHealth,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Batch Mount 的每一项仍然是一条独立 Mount 请求，不引入 Bundle 级 Mount。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchMountRequest {
    pub member_id: String,
    pub app_id: SupportedAppId,
    pub scope: MountScope,
    pub project_id: Option<String>,
}

/// 路径冲突与 scope 冲突必须分别展示，用户不能把冲突项带入最终事务。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BatchMountDisposition {
    Ready,
    PathConflict,
    ScopeConflict,
    AlreadyMounted,
}

impl BatchMountDisposition {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::PathConflict => "path_conflict",
            Self::ScopeConflict => "scope_conflict",
            Self::AlreadyMounted => "already_mounted",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "ready" => Some(Self::Ready),
            "path_conflict" => Some(Self::PathConflict),
            "scope_conflict" => Some(Self::ScopeConflict),
            "already_mounted" => Some(Self::AlreadyMounted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchMountPlanItem {
    pub id: String,
    pub member_id: String,
    pub skill_name: String,
    pub app_id: SupportedAppId,
    pub scope: MountScope,
    pub project_id: Option<String>,
    pub project_display_name: Option<String>,
    pub target_path: String,
    pub expected_target: String,
    pub disposition: BatchMountDisposition,
    pub selectable: bool,
    pub default_selected: bool,
    pub conflict_reason: Option<String>,
    pub target_health: MountHealth,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchMountPlan {
    pub id: String,
    pub bundle_id: String,
    pub bundle_display_name: String,
    pub items: Vec<BatchMountPlanItem>,
    pub created_at: i64,
    pub expires_at: i64,
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
    /// 仅用于比较本机观察变化，不作为 Skill Identity 或受管内容摘要。
    pub observed_fingerprint: String,
    pub root_key: ScanRootKey,
    /// 只有 project 扫描根携带 Project；global 与用户级共享根保持为空。
    pub project_id: Option<String>,
    pub stale: bool,
    pub management_kind: ManagementKind,
}

/// 主界面读模型合并扫描事实和受管领域记录，但两者仍分别持久化。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryItem {
    pub id: String,
    pub skill_name: String,
    pub declared_name: Option<String>,
    pub skill_root: String,
    pub skill_file: String,
    pub location_kind: InventoryLocationKind,
    pub metadata_status: SkillMetadataStatus,
    pub observed_by: Vec<SupportedAppId>,
    pub observed_fingerprint: String,
    pub root_key: Option<ScanRootKey>,
    pub project_id: Option<String>,
    pub stale: bool,
    pub management_kind: ManagementKind,
    pub bundle_id: Option<String>,
    /// 受管条目公开稳定 Member ID；扫描观察保持为空，前端不能解析展示 ID。
    pub member_id: Option<String>,
    pub bundle_display_name: Option<String>,
    pub source_display_name: Option<String>,
    pub project_display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInstallPlan {
    pub id: String,
    pub input_path: String,
    pub bundle_display_name: String,
    pub candidates: Vec<FolderInstallCandidate>,
    pub warnings: Vec<String>,
    pub will_mount: bool,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInstallCandidate {
    pub candidate_id: String,
    pub source_relative_path: String,
    pub skill_name: Option<String>,
    pub description: Option<String>,
    pub target_directory: Option<String>,
    pub selectable: bool,
    pub validation_errors: Vec<String>,
    pub warnings: Vec<String>,
    /// 全新安装默认选择全部有效成员，界面仍可在最终确认前取消。
    pub default_selected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanRootKey {
    CodexGlobal,
    ClaudeCodeGlobal,
    GitHubCopilotGlobal,
    SharedAgents,
    CodexProject,
    ClaudeCodeProject,
    GitHubCopilotProject,
    SharedAgentsProject,
}

impl ScanRootKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodexGlobal => "codex_global",
            Self::ClaudeCodeGlobal => "claude_code_global",
            Self::GitHubCopilotGlobal => "github_copilot_global",
            Self::SharedAgents => "shared_agents",
            Self::CodexProject => "codex_project",
            Self::ClaudeCodeProject => "claude_code_project",
            Self::GitHubCopilotProject => "github_copilot_project",
            Self::SharedAgentsProject => "shared_agents_project",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "codex_global" => Some(Self::CodexGlobal),
            "claude_code_global" => Some(Self::ClaudeCodeGlobal),
            "github_copilot_global" => Some(Self::GitHubCopilotGlobal),
            "shared_agents" => Some(Self::SharedAgents),
            "codex_project" => Some(Self::CodexProject),
            "claude_code_project" => Some(Self::ClaudeCodeProject),
            "github_copilot_project" => Some(Self::GitHubCopilotProject),
            "shared_agents_project" => Some(Self::SharedAgentsProject),
            _ => None,
        }
    }

    pub(crate) fn is_project(self) -> bool {
        matches!(
            self,
            Self::CodexProject
                | Self::ClaudeCodeProject
                | Self::GitHubCopilotProject
                | Self::SharedAgentsProject
        )
    }
}

/// 同一种 project root key 会出现在多个 Project 中，刷新必须按二者共同隔离结果。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub(crate) struct ScanRootIdentity {
    pub root_key: ScanRootKey,
    pub project_id: Option<String>,
}

impl ScanRootIdentity {
    pub(crate) fn global(root_key: ScanRootKey) -> Self {
        Self {
            root_key,
            project_id: None,
        }
    }

    pub(crate) fn project(root_key: ScanRootKey, project_id: &str) -> Self {
        Self {
            root_key,
            project_id: Some(project_id.to_owned()),
        }
    }

    pub(crate) fn stable_id(&self) -> String {
        match &self.project_id {
            Some(project_id) => {
                format!("project:{project_id}:{}", self.root_key.as_str())
            }
            None => format!("global:{}", self.root_key.as_str()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManagementKind {
    SkillYardManaged,
    TakeoverCandidate,
    AgentManaged,
    ProjectManaged,
}

impl ManagementKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SkillYardManaged => "skillyard_managed",
            Self::TakeoverCandidate => "takeover_candidate",
            Self::AgentManaged => "agent_managed",
            Self::ProjectManaged => "project_managed",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "skillyard_managed" => Some(Self::SkillYardManaged),
            "takeover_candidate" => Some(Self::TakeoverCandidate),
            "agent_managed" => Some(Self::AgentManaged),
            "project_managed" => Some(Self::ProjectManaged),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanIssueCode {
    InspectPath,
    RootNotDirectory,
    ReadRoot,
    ReadSkillContent,
}

impl ScanIssueCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InspectPath => "inspect_path",
            Self::RootNotDirectory => "root_not_directory",
            Self::ReadRoot => "read_root",
            Self::ReadSkillContent => "read_skill_content",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "inspect_path" => Some(Self::InspectPath),
            "root_not_directory" => Some(Self::RootNotDirectory),
            "read_root" => Some(Self::ReadRoot),
            "read_skill_content" => Some(Self::ReadSkillContent),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanIssue {
    pub root_id: String,
    pub root_key: ScanRootKey,
    pub project_id: Option<String>,
    pub path: String,
    pub code: ScanIssueCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRefreshSummary {
    pub completed_at: i64,
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryIssue {
    pub id: String,
    pub bundle_display_name: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InventoryLocationKind {
    AppGlobal,
    AppProject,
    SharedReadOnly,
    ManagedStore,
}

impl InventoryLocationKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AppGlobal => "app_global",
            Self::AppProject => "app_project",
            Self::SharedReadOnly => "shared_read_only",
            Self::ManagedStore => "managed_store",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "app_global" => Some(Self::AppGlobal),
            "app_project" => Some(Self::AppProject),
            "shared_read_only" => Some(Self::SharedReadOnly),
            "managed_store" => Some(Self::ManagedStore),
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
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
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
        entries: Vec<InventoryItem>,
        supported_apps: Vec<SupportedAppSummary>,
        last_local_refresh: Option<LocalRefreshSummary>,
        scan_issues: Vec<ScanIssue>,
        recovery_issues: Vec<RecoveryIssue>,
        projects: Vec<ProjectSummary>,
        mounts: Vec<MountSummary>,
    },
    FolderInstallPlan {
        plan: FolderInstallPlan,
    },
    MountPlan {
        plan: MountPlan,
    },
    BatchMountPlan {
        plan: BatchMountPlan,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_outcome_uses_the_frontend_camel_case_contract() {
        let outcome = UiOutcome::Inventory {
            scan_completed_at: 10,
            entries: Vec::new(),
            supported_apps: Vec::new(),
            last_local_refresh: None,
            scan_issues: Vec::new(),
            recovery_issues: vec![RecoveryIssue {
                id: "txn".to_owned(),
                bundle_display_name: "example".to_owned(),
                message: "需要人工恢复".to_owned(),
            }],
            projects: Vec::new(),
            mounts: Vec::new(),
        };

        let value = serde_json::to_value(outcome).expect("应序列化 UI 状态");
        assert_eq!(value["type"], "inventory");
        assert_eq!(value["scanCompletedAt"], 10);
        assert_eq!(value["recoveryIssues"][0]["bundleDisplayName"], "example");
        assert!(value.get("scan_completed_at").is_none());
        assert!(value.get("recovery_issues").is_none());

        let project_root =
            serde_json::to_value(ScanRootKey::GitHubCopilotProject).expect("应序列化扫描根");
        assert_eq!(project_root, "gitHubCopilotProject");
    }
}
