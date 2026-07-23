use serde::{Deserialize, Serialize};

/// UI 只能通过这个封闭枚举表达业务意图，不能传入任意文件操作。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UiIntent {
    GetStartupState,
    StartInitialScan,
    RefreshLocalInventory,
    OpenSourceDiscovery,
    SearchSkillsSh {
        query: String,
    },
    ReloadGitHubSource {
        source_id: String,
    },
    AddGitHubSource {
        input: String,
        tracked_ref: Option<String>,
    },
    ConfirmSourceRefChange {
        plan_id: String,
    },
    CreateFolderInstallPlan {
        input_path: String,
    },
    CreateArchiveInstallPlan {
        input_path: String,
    },
    CreateUrlInstallPlan {
        url: String,
    },
    CreateEditableLocalInstallPlan {
        input_path: String,
    },
    CreateGithubInstallPlan {
        source_id: String,
    },
    ConfirmInstallPlan {
        plan_id: String,
        selected_candidate_ids: Vec<String>,
    },
    DiscardInstallPlan {
        #[serde(rename = "planId")]
        plan_id: String,
    },
    RegisterProject {
        root_path: String,
    },
    CreateTakeoverPlan {
        request: TakeoverPlanRequest,
    },
    ConfirmTakeoverPlan {
        #[serde(rename = "planId")]
        plan_id: String,
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
    CreateSourceAssociationPlan {
        #[serde(rename = "bundleId")]
        bundle_id: String,
        #[serde(rename = "sourceId")]
        source_id: String,
        #[serde(rename = "memberChoices")]
        member_choices: Vec<SourceMemberMappingChoice>,
    },
    ConfirmSourceAssociationPlan {
        #[serde(rename = "planId")]
        plan_id: String,
        #[serde(rename = "contentChoices")]
        content_choices: Vec<MergeContentChoice>,
    },
    DiscardSourceAssociationPlan {
        #[serde(rename = "planId")]
        plan_id: String,
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

/// 创建 Plan 时一次冻结全部用户选择；确认阶段只再接收不透明 Plan ID。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeoverPlanRequest {
    pub observation_ids: Vec<String>,
    pub selected_observation_id: String,
    pub preserved_observation_ids: Vec<String>,
    pub shared_targets: Vec<TakeoverSharedTargetRequest>,
}

/// 共享目录不能继续作为 Mount，用户要明确选择应用专属目标。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeoverSharedTargetRequest {
    pub shared_observation_id: String,
    pub app_id: SupportedAppId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TakeoverIdentityBasis {
    SingleOrigin,
    UserConfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TakeoverOriginDisposition {
    Mount,
    Remove,
}

/// Origin 是用户确认属于同一个 Skill Identity 的一个现有本地副本。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeoverPlanOrigin {
    pub observation_id: String,
    pub original_path: String,
    pub app_id: Option<SupportedAppId>,
    pub scope: Option<MountScope>,
    pub project_id: Option<String>,
    pub project_display_name: Option<String>,
    pub content_fingerprint: String,
    pub warnings: Vec<String>,
    pub final_disposition: TakeoverOriginDisposition,
}

/// Target 是接管完成后必须存在并指向唯一受管内容的一个 Mount。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeoverPlanTarget {
    pub mount_id: String,
    pub app_id: SupportedAppId,
    pub scope: MountScope,
    pub project_id: Option<String>,
    pub project_display_name: Option<String>,
    pub target_path: String,
    pub expected_target: String,
}

/// 一套 Plan 同时覆盖单副本、重复副本、scope 冲突与共享目录输入。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeoverPlan {
    pub id: String,
    pub identity_basis: TakeoverIdentityBasis,
    pub selected_observation_id: String,
    pub bundle_id: String,
    pub member_id: String,
    pub content_id: String,
    pub bundle_display_name: String,
    pub skill_name: String,
    pub skill_description: String,
    pub source_display_name: Option<String>,
    pub managed_directory: String,
    pub content_directory: String,
    pub expected_target: String,
    pub origins: Vec<TakeoverPlanOrigin>,
    pub targets: Vec<TakeoverPlanTarget>,
    pub warnings: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManagementEvidenceKind {
    GitHeadTracked,
}

impl ManagementEvidenceKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::GitHeadTracked => "git_head_tracked",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "git_head_tracked" => Some(Self::GitHeadTracked),
            _ => None,
        }
    }
}

/// 这份证据只说明 Project 当前 HEAD 维护该文件，不推断远端 Source 或可编辑性。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagementEvidence {
    pub kind: ManagementEvidenceKind,
    pub authority_root: String,
    pub snapshot_commit_oid: String,
    pub subject_path: String,
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
    pub management_evidence: Option<ManagementEvidence>,
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
    pub management_evidence: Option<ManagementEvidence>,
    pub bundle_id: Option<String>,
    /// 受管条目公开稳定 Member ID；扫描观察保持为空，前端不能解析展示 ID。
    pub member_id: Option<String>,
    pub bundle_display_name: Option<String>,
    pub source_display_name: Option<String>,
    pub project_display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPlan {
    pub id: String,
    pub input_kind: InstallInputKind,
    pub mode: InstallMode,
    pub input_path: String,
    pub bundle_display_name: String,
    pub candidates: Vec<InstallCandidate>,
    pub warnings: Vec<String>,
    pub will_mount: bool,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallInputKind {
    LocalFolder,
    Github,
    Archive,
    DirectUrl,
    EditableLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallMode {
    Create,
    Supplement,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallCandidate {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceCatalogStatus {
    Unloaded,
    Fresh,
    Stale,
}

/// Source kind 决定内容的取得方式，但不会改变统一的 Bundle 安装生命周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Github,
    Archive,
    DirectUrl,
    EditableLocal,
}

impl SourceKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Archive => "archive",
            Self::DirectUrl => "direct_url",
            Self::EditableLocal => "editable_local",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "github" => Some(Self::Github),
            "archive" => Some(Self::Archive),
            "direct_url" => Some(Self::DirectUrl),
            "editable_local" => Some(Self::EditableLocal),
            _ => None,
        }
    }
}

impl SourceCatalogStatus {
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "unloaded" => Some(Self::Unloaded),
            "fresh" => Some(Self::Fresh),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }
}

/// Catalog Member 只是上游发现 metadata；只有关联到 Member ID 后才表示已经安装。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCatalogMemberSummary {
    pub id: String,
    pub relative_path: String,
    pub skill_name: Option<String>,
    pub description: Option<String>,
    pub selectable: bool,
    pub validation_errors: Vec<String>,
    pub warnings: Vec<String>,
    pub installed_member_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSummary {
    pub id: String,
    pub kind: SourceKind,
    pub canonical_identity: String,
    pub display_name: String,
    pub locator: String,
    pub tracked_ref: Option<String>,
    pub member_path_hint: Option<String>,
    pub catalog_status: SourceCatalogStatus,
    /// marker 只用于确认来源内容基线，不向用户承诺版本或回滚历史。
    pub catalog_marker: Option<String>,
    pub catalog_fetched_at: Option<i64>,
    pub last_reload_at: Option<i64>,
    pub last_reload_error: Option<String>,
    pub bundle_id: Option<String>,
    pub adopted_marker: Option<String>,
    pub members: Vec<SourceCatalogMemberSummary>,
}

/// 同一份关联 Plan 用 mode 区分直接关联和归并，不建立第二套确认协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceAssociationMode {
    Link,
    Merge,
}

/// 创建 Plan 时，每个本地成员都必须明确选择“对应”或“不对应”。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceMemberMappingChoice {
    pub member_id: String,
    pub source_relative_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAssociationMemberChoice {
    pub member_id: String,
    pub skill_name: String,
    pub source_relative_path: Option<String>,
}

/// 成员快照让用户看到归并范围，也让确认阶段能够检查计划后的状态变化。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAssociationMember {
    pub member_id: String,
    pub bundle_id: String,
    pub bundle_display_name: String,
    pub skill_name: String,
    pub content_fingerprint: String,
}

/// 常见的一对一内容冲突可由用户选择一个成员；交叉冲突会单独阻塞计划。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAssociationConflict {
    pub id: String,
    pub label: String,
    pub candidate_member_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeContentChoice {
    pub conflict_id: String,
    pub member_id: String,
}

/// 公开 DTO 只包含确认界面需要展示的事实；额外竞态快照由编排器内部封存。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAssociationPlan {
    pub id: String,
    pub mode: SourceAssociationMode,
    pub source_id: String,
    pub source_display_name: String,
    pub target_bundle_id: String,
    pub target_bundle_display_name: String,
    pub retiring_bundle_id: Option<String>,
    pub retiring_bundle_display_name: Option<String>,
    pub member_choices: Vec<SourceAssociationMemberChoice>,
    pub members: Vec<SourceAssociationMember>,
    pub mounts: Vec<MountSummary>,
    pub conflicts: Vec<SourceAssociationConflict>,
    pub blocking_issues: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Tracked Ref 变更只冻结用户需要确认的 metadata，不创建文件系统事务。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRefChangePlan {
    pub id: String,
    pub source_id: String,
    pub source_display_name: String,
    pub current_ref: String,
    pub candidate_ref: String,
    pub candidate_commit_sha: String,
    pub member_path_hint: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

/// `skills.sh` 只返回发现线索；可支持的分组仍会转换成普通 GitHub Source。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsShSearchSource {
    pub source_input: String,
    pub supported: bool,
    pub members: Vec<SkillsShSearchMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsShSearchMember {
    pub skill_id: String,
    pub name: String,
    pub installs: u64,
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
    InspectManagementEvidence,
    RootNotDirectory,
    ReadRoot,
    ReadSkillContent,
}

impl ScanIssueCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InspectPath => "inspect_path",
            Self::InspectManagementEvidence => "inspect_management_evidence",
            Self::RootNotDirectory => "root_not_directory",
            Self::ReadRoot => "read_root",
            Self::ReadSkillContent => "read_skill_content",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "inspect_path" => Some(Self::InspectPath),
            "inspect_management_evidence" => Some(Self::InspectManagementEvidence),
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
    SourceDiscovery {
        sources: Vec<SourceSummary>,
        highlighted_source_id: Option<String>,
        highlighted_member_path: Option<String>,
    },
    SkillsShSearch {
        query: String,
        sources: Vec<SkillsShSearchSource>,
    },
    SourceRefChangePlan {
        plan: SourceRefChangePlan,
    },
    InstallPlan {
        plan: InstallPlan,
    },
    InstallPlanDiscarded,
    MountPlan {
        plan: MountPlan,
    },
    BatchMountPlan {
        plan: BatchMountPlan,
    },
    TakeoverPlan {
        plan: TakeoverPlan,
    },
    SourceAssociationPlan {
        plan: SourceAssociationPlan,
    },
    SourceAssociationPlanDiscarded,
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

    #[test]
    fn skills_sh_search_outcome_uses_the_frontend_camel_case_contract() {
        let outcome = UiOutcome::SkillsShSearch {
            query: "react".to_owned(),
            sources: vec![SkillsShSearchSource {
                source_input: "vercel-labs/agent-skills".to_owned(),
                supported: true,
                members: vec![SkillsShSearchMember {
                    skill_id: "react-best-practices".to_owned(),
                    name: "React Best Practices".to_owned(),
                    installs: 42,
                }],
            }],
        };

        let value = serde_json::to_value(outcome).expect("应序列化 skills.sh 搜索结果");
        assert_eq!(
            value,
            serde_json::json!({
                "type": "skillsShSearch",
                "query": "react",
                "sources": [{
                    "sourceInput": "vercel-labs/agent-skills",
                    "supported": true,
                    "members": [{
                        "skillId": "react-best-practices",
                        "name": "React Best Practices",
                        "installs": 42
                    }]
                }]
            })
        );
    }

    #[test]
    fn takeover_plan_request_uses_the_frontend_camel_case_contract() {
        let intent = UiIntent::CreateTakeoverPlan {
            request: TakeoverPlanRequest {
                observation_ids: vec!["observation-1".to_owned()],
                selected_observation_id: "observation-1".to_owned(),
                preserved_observation_ids: vec!["observation-1".to_owned()],
                shared_targets: vec![TakeoverSharedTargetRequest {
                    shared_observation_id: "shared-1".to_owned(),
                    app_id: SupportedAppId::ClaudeCode,
                }],
            },
        };

        let value = serde_json::to_value(intent).expect("应序列化 Takeover Plan 请求");
        assert_eq!(value["type"], "createTakeoverPlan");
        assert_eq!(
            value["request"]["observationIds"],
            serde_json::json!(["observation-1"])
        );
        assert_eq!(value["request"]["selectedObservationId"], "observation-1");
        assert_eq!(
            value["request"]["preservedObservationIds"],
            serde_json::json!(["observation-1"])
        );
        assert_eq!(
            value["request"]["sharedTargets"][0],
            serde_json::json!({
                "sharedObservationId": "shared-1",
                "appId": "claudeCode"
            })
        );
    }

    #[test]
    fn takeover_confirmation_contract_contains_only_the_camel_case_plan_id() {
        let value = serde_json::to_value(UiIntent::ConfirmTakeoverPlan {
            plan_id: "takeover-plan-1".to_owned(),
        })
        .expect("应序列化 Takeover 确认请求");

        assert_eq!(
            value,
            serde_json::json!({
                "type": "confirmTakeoverPlan",
                "planId": "takeover-plan-1"
            })
        );
    }

    #[test]
    fn install_plan_and_discard_use_the_canonical_frontend_contract() {
        let plan = InstallPlan {
            id: "install-plan-1".to_owned(),
            input_kind: InstallInputKind::Github,
            mode: InstallMode::Supplement,
            input_path: "https://github.com/example/skills".to_owned(),
            bundle_display_name: "example/skills".to_owned(),
            candidates: Vec::new(),
            warnings: Vec::new(),
            will_mount: false,
            created_at: 10,
            expires_at: 20,
        };
        let plan_value =
            serde_json::to_value(UiOutcome::InstallPlan { plan }).expect("应序列化安装 Plan");
        assert_eq!(plan_value["type"], "installPlan");
        assert_eq!(plan_value["plan"]["inputKind"], "github");
        assert_eq!(plan_value["plan"]["mode"], "supplement");
        assert_eq!(
            plan_value["plan"]["inputPath"],
            "https://github.com/example/skills"
        );

        let intent = serde_json::to_value(UiIntent::DiscardInstallPlan {
            plan_id: "install-plan-1".to_owned(),
        })
        .expect("应序列化放弃安装 Plan 请求");
        assert_eq!(
            intent,
            serde_json::json!({
                "type": "discardInstallPlan",
                "planId": "install-plan-1"
            })
        );
        assert_eq!(
            serde_json::to_value(UiOutcome::InstallPlanDiscarded).expect("应序列化放弃完成状态"),
            serde_json::json!({"type": "installPlanDiscarded"})
        );
    }

    #[test]
    fn source_association_intents_use_one_camel_case_contract() {
        let create = UiIntent::CreateSourceAssociationPlan {
            bundle_id: "bundle-local".to_owned(),
            source_id: "source-upstream".to_owned(),
            member_choices: vec![SourceMemberMappingChoice {
                member_id: "member-alpha".to_owned(),
                source_relative_path: Some("skills/alpha".to_owned()),
            }],
        };
        assert_eq!(
            serde_json::to_value(create).expect("应序列化来源关联请求"),
            serde_json::json!({
                "type": "createSourceAssociationPlan",
                "bundleId": "bundle-local",
                "sourceId": "source-upstream",
                "memberChoices": [{
                    "memberId": "member-alpha",
                    "sourceRelativePath": "skills/alpha"
                }]
            })
        );

        let confirm = UiIntent::ConfirmSourceAssociationPlan {
            plan_id: "association-plan".to_owned(),
            content_choices: vec![MergeContentChoice {
                conflict_id: "conflict-alpha".to_owned(),
                member_id: "member-alpha".to_owned(),
            }],
        };
        assert_eq!(
            serde_json::to_value(confirm).expect("应序列化来源关联确认请求"),
            serde_json::json!({
                "type": "confirmSourceAssociationPlan",
                "planId": "association-plan",
                "contentChoices": [{
                    "conflictId": "conflict-alpha",
                    "memberId": "member-alpha"
                }]
            })
        );
        assert_eq!(
            serde_json::to_value(UiOutcome::SourceAssociationPlanDiscarded)
                .expect("应序列化来源关联放弃结果"),
            serde_json::json!({"type": "sourceAssociationPlanDiscarded"})
        );
    }
}
