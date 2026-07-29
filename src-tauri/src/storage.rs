use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use thiserror::Error;
use uuid::Uuid;

use crate::domain::{
    AiPreferences, AiProvider, BatchMountDisposition, BundleUpdateAction, BundleUpdateStatus,
    BundleUpdateSummary, EditableLocalRelinkMember, EditableLocalRelinkPlan, InstallationChain,
    InstallationChainKind, InterfaceLanguage, InventoryItem, InventoryLocationKind,
    InventoryObservation, LocalRefreshSummary, ManagementEvidence, ManagementEvidenceKind,
    ManagementKind, MountHealth, MountOperation, MountPlanPurpose, MountScope, MountSummary,
    ProjectSummary, RecoveryIssue, ScanIssue, ScanIssueCode, ScanRootIdentity, ScanRootKey,
    SkillMetadataStatus, SourceCatalogMemberSummary, SourceCatalogStatus, SourceKind,
    SourceRefChangePlan, SourceSummary, SupportedAppId, SupportedAppSummary, TakeoverPlan,
    UiOutcome,
};
use crate::github_source::parse_github_source;
use crate::installation_chain::takeover_group_evidence;

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_initial.sql")),
    (2, include_str!("../migrations/0002_local_inventory.sql")),
    (3, include_str!("../migrations/0003_folder_install.sql")),
    (
        4,
        include_str!("../migrations/0004_bundle_install_candidates.sql"),
    ),
    (5, include_str!("../migrations/0005_codex_mounts.sql")),
    (6, include_str!("../migrations/0006_mount_plan_purpose.sql")),
    (7, include_str!("../migrations/0007_project_inventory.sql")),
    (8, include_str!("../migrations/0008_mount_batches.sql")),
    (
        9,
        include_str!("../migrations/0009_management_evidence.sql"),
    ),
    (10, include_str!("../migrations/0010_takeover.sql")),
    (
        11,
        include_str!("../migrations/0011_takeover_transactions.sql"),
    ),
    (12, include_str!("../migrations/0012_github_sources.sql")),
    (
        13,
        include_str!("../migrations/0013_install_bundle_protocol.sql"),
    ),
    (
        14,
        include_str!("../migrations/0014_general_source_install.sql"),
    ),
    (
        15,
        include_str!("../migrations/0015_source_association_plans.sql"),
    ),
    (
        16,
        include_str!("../migrations/0016_source_association_transactions.sql"),
    ),
    (
        17,
        include_str!("../migrations/0017_bundle_update_checks.sql"),
    ),
    (18, include_str!("../migrations/0018_bundle_update.sql")),
    (
        19,
        include_str!("../migrations/0019_bundle_update_batches.sql"),
    ),
    (20, include_str!("../migrations/0020_removals.sql")),
    (
        21,
        include_str!("../migrations/0021_editable_local_relink.sql"),
    ),
    (
        22,
        include_str!("../migrations/0022_installation_chain.sql"),
    ),
    (
        23,
        include_str!("../migrations/0023_bundle_display_name_from_lock_source.sql"),
    ),
    (
        24,
        include_str!("../migrations/0024_takeover_source_from_lock.sql"),
    ),
    (
        25,
        include_str!("../migrations/0025_bundle_mount_removal.sql"),
    ),
    (26, include_str!("../migrations/0026_scan_issue_paths.sql")),
    (
        27,
        include_str!("../migrations/0027_interface_language.sql"),
    ),
    (28, include_str!("../migrations/0028_ai_preferences.sql")),
];

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("无法创建 SkillYard 数据目录：{0}")]
    CreateDataRoot(#[source] std::io::Error),
    #[error("无法检查 SkillYard 数据目录：{0}")]
    InspectDataRoot(#[source] std::io::Error),
    #[error("SkillYard 数据目录不能是符号链接或其他文件类型：{0}")]
    UnsafeDataRoot(PathBuf),
    #[error("无法检查 SkillYard SQLite 路径：{0}")]
    InspectDatabase(#[source] std::io::Error),
    #[error("SkillYard SQLite 不能是符号链接或其他文件类型：{0}")]
    UnsafeDatabase(PathBuf),
    #[error("无法打开 SkillYard SQLite：{0}")]
    OpenDatabase(#[source] rusqlite::Error),
    #[error("无法执行 SkillYard SQLite migration：{0}")]
    Migration(#[source] rusqlite::Error),
    #[error("无法读取界面语言偏好：{0}")]
    ReadPreferences(#[source] rusqlite::Error),
    #[error("无法保存界面语言偏好：{0}")]
    SavePreferences(#[source] rusqlite::Error),
    #[error("SQLite 中包含未知界面语言：{0}")]
    UnknownInterfaceLanguage(String),
    #[error("无法读取 AI 偏好：{0}")]
    ReadAiPreferences(#[source] rusqlite::Error),
    #[error("无法保存 AI 偏好：{0}")]
    SaveAiPreferences(#[source] rusqlite::Error),
    #[error("SQLite 中包含未知 AI Provider：{0}")]
    UnknownAiProvider(String),
    #[error("无法读取本机清单状态：{0}")]
    ReadInventory(#[source] rusqlite::Error),
    #[error("无法读取 Source 状态：{0}")]
    ReadSources(#[source] rusqlite::Error),
    #[error("SQLite 中包含未知 Source Catalog 状态：{0}")]
    UnknownSourceCatalogStatus(String),
    #[error("SQLite 中包含未知 Source kind：{0}")]
    UnknownSourceKind(String),
    #[error("SQLite 中包含未知 Bundle 更新检查状态：{0}")]
    UnknownBundleUpdateStatus(String),
    #[error("Source Catalog 成员 metadata 无法解析：{0}")]
    InvalidSourceCatalogMetadata(#[source] serde_json::Error),
    #[error("无法保存 Source：{0}")]
    SaveSource(#[source] rusqlite::Error),
    #[error("Source 输入不符合 1.0 的来源协议")]
    InvalidSourceDefinition,
    #[error("Editable Local Source 的目录位置已变化；请从原路径继续使用")]
    EditableLocalPathChanged,
    #[error("无法保存 Source Catalog：{0}")]
    SaveSourceCatalog(#[source] rusqlite::Error),
    #[error("无法编码 Source Catalog metadata：{0}")]
    SerializeSourceCatalogMetadata(#[source] serde_json::Error),
    #[error("Source 已经不存在")]
    SourceNotFound,
    #[error("Source 状态已经变化，请重新加载来源")]
    SourceCatalogStateChanged,
    #[error("Source 与已关联 Bundle 的持久化状态不一致")]
    SourceBundleStateConflict,
    #[error("Tracked Ref 变更 Plan 未签发或已经不存在")]
    SourceRefChangePlanNotFound,
    #[error("Tracked Ref 变更 Plan 已经使用，不能重复确认")]
    SourceRefChangePlanConsumed,
    #[error("Tracked Ref 变更 Plan 已过期，请重新添加来源")]
    SourceRefChangePlanExpired,
    #[error("Source 的 Tracked Ref 已经变化，请重新添加来源")]
    SourceRefChangeStateChanged,
    #[error("Editable Local 重新关联 Plan 未签发或已经不存在")]
    EditableLocalRelinkPlanNotFound,
    #[error("Editable Local 重新关联 Plan 已经使用，不能重复确认")]
    EditableLocalRelinkPlanConsumed,
    #[error("Editable Local 重新关联 Plan 已过期，请重新选择目录")]
    EditableLocalRelinkPlanExpired,
    #[error("Editable Local Source 或候选目录已经变化，请重新选择目录")]
    EditableLocalRelinkStateChanged,
    #[error("无法编码 Editable Local 重新关联成员：{0}")]
    SerializeEditableLocalRelinkMetadata(#[source] serde_json::Error),
    #[error("无法解析 Editable Local 重新关联成员：{0}")]
    InvalidEditableLocalRelinkMetadata(#[source] serde_json::Error),
    #[error("无法保存首次扫描结果：{0}")]
    SaveInitialScan(#[source] rusqlite::Error),
    #[error("无法保存本机刷新结果：{0}")]
    SaveLocalRefresh(#[source] rusqlite::Error),
    #[error("无法保存 Project 扫描结果：{0}")]
    SaveProjectScan(#[source] rusqlite::Error),
    #[error("SQLite 中包含未知 Supported App：{0}")]
    UnknownSupportedApp(String),
    #[error("SQLite 中包含未知 Inventory location：{0}")]
    UnknownInventoryLocation(String),
    #[error("SQLite 中包含未知 Skill metadata 状态：{0}")]
    UnknownMetadataStatus(String),
    #[error("SQLite 中包含未知扫描根：{0}")]
    UnknownScanRoot(String),
    #[error("SQLite 中的扫描根与 Project 关系不一致：{0}")]
    InvalidScanRootIdentity(String),
    #[error("SQLite 中包含未知管理状态：{0}")]
    UnknownManagementKind(String),
    #[error("SQLite 中包含未知管理证据类型：{0}")]
    UnknownManagementEvidenceKind(String),
    #[error("SQLite 中包含未知 Installation Chain 类型：{0}")]
    UnknownInstallationChainKind(String),
    #[error("SQLite 中的 Installation Chain 记录不完整")]
    InvalidInstallationChain,
    #[error("SQLite 中包含未知扫描问题类型：{0}")]
    UnknownScanIssueCode(String),
    #[error("SQLite 中包含非法刷新统计值：{0}")]
    InvalidRefreshCount(i64),
    #[error("安装 Plan 未签发或已经不存在")]
    InstallPlanNotFound,
    #[error("安装 Plan 已经使用，不能重复确认")]
    InstallPlanConsumed,
    #[error("安装 Plan 由“全部更新”协调器持有，不能单独确认或放弃")]
    InstallPlanOwnedByBundleUpdateBatch,
    #[error("安装 Plan 已过期，请重新生成")]
    InstallPlanExpired,
    #[error("安装 Plan 没有可保存的候选成员")]
    EmptyInstallPlanCandidates,
    #[error("安装 Plan 不符合唯一的 folder/Source 安装协议")]
    InvalidInstallPlan,
    #[error("确认的成员选择不属于当前安装 Plan")]
    InvalidInstallSelection,
    #[error("已有一项生命周期写事务正在执行")]
    ActiveLifecycleTransaction,
    #[error("无法保存安装 Plan：{0}")]
    SaveInstallPlan(#[source] rusqlite::Error),
    #[error("无法读取安装 Plan：{0}")]
    ReadInstallPlan(#[source] rusqlite::Error),
    #[error("安装 Plan 中的风险提示损坏：{0}")]
    InvalidPlanWarnings(#[source] serde_json::Error),
    #[error("安装 Plan 中的验证结果损坏：{0}")]
    InvalidPlanValidationErrors(#[source] serde_json::Error),
    #[error("安装 Plan 中包含非法布尔值：{0}")]
    InvalidPlanBoolean(i64),
    #[error("当前没有可用的“全部更新”计划")]
    BundleUpdateBatchNotFound,
    #[error("已有一份尚未处理的“全部更新”计划")]
    BundleUpdateBatchAlreadyOpen,
    #[error("“全部更新”计划已经确认，不能重复修改")]
    BundleUpdateBatchConsumed,
    #[error("“全部更新”计划已过期，请重新生成")]
    BundleUpdateBatchExpired,
    #[error("“全部更新”的 Bundle 选择或顺序无效")]
    InvalidBundleUpdateBatchSelection,
    #[error("“全部更新”的持久化状态不一致")]
    InvalidBundleUpdateBatch,
    #[error("无法保存“全部更新”状态：{0}")]
    SaveBundleUpdateBatch(#[source] rusqlite::Error),
    #[error("无法读取“全部更新”状态：{0}")]
    ReadBundleUpdateBatch(#[source] rusqlite::Error),
    #[error("无法保存生命周期事务：{0}")]
    SaveLifecycleTransaction(#[source] rusqlite::Error),
    #[error("无法读取生命周期事务：{0}")]
    ReadLifecycleTransaction(#[source] rusqlite::Error),
    #[error("无法读取人工恢复状态：{0}")]
    ReadRecoveryIssues(#[source] rusqlite::Error),
    #[error("Removal Plan 未签发、已使用或已经不存在")]
    RemovalPlanNotFound,
    #[error("Removal Plan 已过期，请重新生成")]
    RemovalPlanExpired,
    #[error("Removal Plan 或事务状态已经变化，请重新预览")]
    RemovalStateConflict,
    #[error("无法保存 Removal 状态：{0}")]
    SaveRemoval(#[source] rusqlite::Error),
    #[error("无法读取 Removal 状态：{0}")]
    ReadRemoval(#[source] rusqlite::Error),
    #[error("无法保存受管 Bundle：{0}")]
    SaveManagedBundle(#[source] rusqlite::Error),
    #[error("受管 Bundle 的持久化状态与事务计划不一致")]
    ManagedStateConflict,
    #[error("生命周期事务不存在或当前状态不允许该操作：{0}")]
    LifecycleStateConflict(String),
    #[error("SQLite 中包含未知生命周期阶段：{0}")]
    InvalidLifecyclePhase(String),
    #[error("受管内容路径不符合 Central Store 固定布局：{0}")]
    UnsafeManagedPath(String),
    #[error("Project 不存在：{0}")]
    ProjectNotFound(String),
    #[error("Project 路径或文件系统身份已经由另一条记录占用")]
    ProjectIdentityConflict,
    #[error("受管 Skill Member 不存在：{0}")]
    ManagedMemberNotFound(String),
    #[error("Mount 不存在：{0}")]
    MountNotFound(String),
    #[error("Mount Plan 未签发或已经不存在")]
    MountPlanNotFound,
    #[error("Mount Plan 已经使用，不能重复确认")]
    MountPlanConsumed,
    #[error("Mount Plan 已过期，请重新生成")]
    MountPlanExpired,
    #[error("Mount Plan 的成员、Project 或目标状态不一致")]
    InvalidMountPlan,
    #[error("无法保存 Project：{0}")]
    SaveProject(#[source] rusqlite::Error),
    #[error("无法读取 Project：{0}")]
    ReadProject(#[source] rusqlite::Error),
    #[error("无法读取受管 Skill Member：{0}")]
    ReadManagedMember(#[source] rusqlite::Error),
    #[error("无法保存 Mount Plan：{0}")]
    SaveMountPlan(#[source] rusqlite::Error),
    #[error("无法读取 Mount Plan：{0}")]
    ReadMountPlan(#[source] rusqlite::Error),
    #[error("无法保存 Mount 事务：{0}")]
    SaveMountTransaction(#[source] rusqlite::Error),
    #[error("无法读取 Mount 事务：{0}")]
    ReadMountTransaction(#[source] rusqlite::Error),
    #[error("Mount 事务不存在或当前状态不允许该操作：{0}")]
    MountStateConflict(String),
    #[error("SQLite 中包含未知 Mount 事务阶段：{0}")]
    InvalidMountPhase(String),
    #[error("SQLite 中包含未知 Mount operation：{0}")]
    UnknownMountOperation(String),
    #[error("SQLite 中包含未知 Mount scope：{0}")]
    UnknownMountScope(String),
    #[error("SQLite 中包含未知 Mount health：{0}")]
    UnknownMountHealth(String),
    #[error("Batch Mount Plan 未签发或已经不存在")]
    BatchMountPlanNotFound,
    #[error("Batch Mount Plan 已经使用，不能重复确认")]
    BatchMountPlanConsumed,
    #[error("Batch Mount Plan 已过期，请重新生成")]
    BatchMountPlanExpired,
    #[error("Batch Mount Plan 的确认集合无效")]
    InvalidBatchMountSelection,
    #[error("Batch Mount Plan 的 Bundle、成员、Project 或目标状态不一致")]
    InvalidBatchMountPlan,
    #[error("相关 Skill 或 Mount 路径正在等待人工恢复，暂时不能修改")]
    ManagedObjectBlocked,
    #[error("SQLite 中包含未知 Batch Mount disposition：{0}")]
    UnknownBatchMountDisposition(String),
    #[error("无法保存 Batch Mount Plan：{0}")]
    SaveBatchMountPlan(#[source] rusqlite::Error),
    #[error("无法读取 Batch Mount Plan：{0}")]
    ReadBatchMountPlan(#[source] rusqlite::Error),
    #[error("无法保存 Batch Mount 事务：{0}")]
    SaveBatchMountTransaction(#[source] rusqlite::Error),
    #[error("无法读取 Batch Mount 事务：{0}")]
    ReadBatchMountTransaction(#[source] rusqlite::Error),
    #[error("Batch Mount 事务不存在或当前状态不允许该操作：{0}")]
    BatchMountStateConflict(String),
    #[error("SQLite 中包含未知 Batch Mount 事务阶段：{0}")]
    InvalidBatchMountPhase(String),
    #[error("SQLite 中包含非法文件系统身份值：{0}")]
    InvalidFilesystemIdentity(i64),
    #[error("Takeover Plan 未签发或已经不存在")]
    TakeoverPlanNotFound,
    #[error("Takeover Plan 已经使用，不能重复确认")]
    TakeoverPlanConsumed,
    #[error("Takeover Plan 已过期，请重新生成")]
    TakeoverPlanExpired,
    #[error("Takeover Plan 的不可变合同已经损坏")]
    InvalidTakeoverPlan,
    #[error("无法保存 Takeover Plan：{0}")]
    SaveTakeoverPlan(#[source] rusqlite::Error),
    #[error("无法读取 Takeover Plan：{0}")]
    ReadTakeoverPlan(#[source] rusqlite::Error),
    #[error("无法保存 Takeover 事务：{0}")]
    SaveTakeoverTransaction(#[source] rusqlite::Error),
    #[error("无法读取 Takeover 事务：{0}")]
    ReadTakeoverTransaction(#[source] rusqlite::Error),
    #[error("Takeover 事务不存在或当前状态不允许该操作：{0}")]
    TakeoverStateConflict(String),
    #[error("SQLite 中包含未知 Takeover 事务阶段：{0}")]
    InvalidTakeoverPhase(String),
    #[error("Source 关联 Plan 未签发或已经不存在")]
    SourceAssociationPlanNotFound,
    #[error("Source 关联 Plan 已经使用，不能重复确认")]
    SourceAssociationPlanConsumed,
    #[error("Source 关联 Plan 已过期，请重新生成")]
    SourceAssociationPlanExpired,
    #[error("Source 关联 Plan 的不可变合同已经损坏")]
    InvalidSourceAssociationPlan,
    #[error("无法保存 Source 关联 Plan：{0}")]
    SaveSourceAssociationPlan(#[source] rusqlite::Error),
    #[error("无法读取 Source 关联 Plan：{0}")]
    ReadSourceAssociationPlan(#[source] rusqlite::Error),
    #[error("无法保存 Source 关联事务：{0}")]
    SaveSourceAssociationTransaction(#[source] rusqlite::Error),
    #[error("无法读取 Source 关联事务：{0}")]
    ReadSourceAssociationTransaction(#[source] rusqlite::Error),
    #[error("Source 关联事务不存在或当前状态不允许该操作：{0}")]
    SourceAssociationStateConflict(String),
    #[error("SQLite 中包含未知 Source 关联事务阶段：{0}")]
    InvalidSourceAssociationPhase(String),
}

pub struct Storage {
    connection: Connection,
    data_root: PathBuf,
}

/// Storage 不解释 Plan 内容，只原样保存由 Takeover 模块签发的不可变合同。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredTakeoverPlanRow {
    pub id: String,
    pub payload_json: String,
    pub payload_sha256: String,
    pub status: String,
    pub created_at: i64,
    pub expires_at: i64,
}

/// 关联层负责解释 payload；Storage 只保存并消费这一份不可变确认合同。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredSourceAssociationPlanRow {
    pub id: String,
    pub payload_json: String,
    pub payload_sha256: String,
    pub status: String,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Merge 恢复只读取这一行和对应 Journal，不另建第二套步骤状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredSourceAssociationTransaction {
    pub id: String,
    pub plan_id: String,
    pub source_id: String,
    pub target_bundle_id: String,
    pub retiring_bundle_id: String,
    pub content_choices_json: String,
    pub source_mappings_json: String,
    pub journal_path: String,
    pub phase: String,
    pub status: String,
}

/// Merge 事务把最终 mapping 保存成排序后的 JSON，恢复与最终提交只接受同一份合同。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct StoredSourceAssociationMemberMapping {
    source_relative_path: String,
    member_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredTakeoverTransaction {
    pub id: String,
    pub plan_id: String,
    pub bundle_id: String,
    pub member_id: String,
    pub reserved_paths: Vec<String>,
    pub journal_path: String,
    pub phase: String,
    pub status: String,
}

/// SQLite 行先保留 JSON 原文，离开 rusqlite 回调后再按 Takeover 领域约束解析。
struct RawStoredTakeoverTransaction {
    id: String,
    plan_id: String,
    bundle_id: String,
    member_id: String,
    reserved_paths_json: String,
    journal_path: String,
    phase: String,
    status: String,
}

pub struct SavedLocalRefresh {
    pub entries: Vec<InventoryItem>,
    pub supported_apps: Vec<SupportedAppSummary>,
    pub summary: LocalRefreshSummary,
    pub recovery_issues: Vec<RecoveryIssue>,
    pub projects: Vec<ProjectSummary>,
    pub mounts: Vec<MountSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredInstallPlan {
    pub id: String,
    pub kind: String,
    pub install_mode: String,
    pub input_path: Option<String>,
    pub input_device: u64,
    pub input_inode: u64,
    pub input_fingerprint: String,
    /// Source 快照路径直接指向已验证的内容根，并始终相对 Central Store。
    pub snapshot_relative_path: Option<String>,
    pub source_id: Option<String>,
    pub source_tracked_ref: Option<String>,
    pub source_catalog_generation: Option<i64>,
    pub source_marker: Option<String>,
    pub expected_source_marker: Option<String>,
    pub expected_current_target: Option<String>,
    pub expected_adopted_marker: Option<String>,
    pub bundle_id: String,
    pub bundle_display_name: String,
    pub warnings: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub status: String,
    pub candidates: Vec<StoredInstallCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredInstallCandidate {
    pub candidate_id: String,
    /// “不对应”的既有成员没有 Source 路径；新候选始终为 `Some`。
    pub source_relative_path: Option<String>,
    pub skill_name: Option<String>,
    pub skill_description: Option<String>,
    pub content_fingerprint: Option<String>,
    /// 已安装成员的旧 fingerprint 用于更新前校验，不能当作可回滚版本。
    pub previous_content_fingerprint: Option<String>,
    pub selectable: bool,
    /// 已安装成员属于最终完整集合，但不能再次作为用户选择提交。
    pub preserve_existing: bool,
    pub validation_errors: Vec<String>,
    pub warnings: Vec<String>,
    pub default_selected: bool,
    pub selected: bool,
}

/// Coordinator 复用 Inventory 的 GitHub Available 语义；Editable Local 必须显式检查。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredBundleUpdateEligibility {
    pub source_id: String,
    pub bundle_id: String,
    pub bundle_display_name: String,
    pub target_marker: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredBundleUpdateBatch {
    pub id: String,
    pub status: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub confirmed_at: Option<i64>,
    pub updated_at: i64,
    pub items: Vec<StoredBundleUpdateBatchItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredBundleUpdateBatchItem {
    pub id: String,
    pub source_id: String,
    pub bundle_id: String,
    pub display_name: String,
    pub install_plan_id: Option<String>,
    pub target_marker: String,
    pub status: String,
    pub error: Option<String>,
    pub display_order: i64,
    pub confirmed_order: Option<i64>,
}

pub(crate) struct NewBundleUpdateBatchItem<'a> {
    pub id: &'a str,
    pub source_id: &'a str,
    pub bundle_id: &'a str,
    pub display_name: &'a str,
    pub install_plan_id: Option<&'a str>,
    pub target_marker: &'a str,
    pub status: &'a str,
    pub error: Option<&'a str>,
}

/// 只有 coordinator 持有的 batch/item 对才能消费对应 child Install Plan。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BundleUpdateBatchChildOwner<'a> {
    pub batch_id: &'a str,
    pub item_id: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BundleUpdateBatchChildOperation {
    Confirm,
    Discard,
}

pub struct NewInstallPlan<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub install_mode: &'a str,
    pub input_path: Option<&'a str>,
    pub input_device: u64,
    pub input_inode: u64,
    pub input_fingerprint: &'a str,
    pub snapshot_relative_path: Option<&'a str>,
    pub source_id: Option<&'a str>,
    pub source_tracked_ref: Option<&'a str>,
    pub source_catalog_generation: Option<i64>,
    pub source_marker: Option<&'a str>,
    pub expected_source_marker: Option<&'a str>,
    pub expected_current_target: Option<&'a str>,
    pub expected_adopted_marker: Option<&'a str>,
    pub bundle_id: &'a str,
    pub bundle_display_name: &'a str,
    pub warnings: &'a [String],
    pub candidates: &'a [NewInstallCandidate<'a>],
    pub created_at: i64,
    pub expires_at: i64,
}

pub struct NewInstallCandidate<'a> {
    pub candidate_id: &'a str,
    pub source_relative_path: Option<&'a str>,
    pub skill_name: Option<&'a str>,
    pub skill_description: Option<&'a str>,
    pub content_fingerprint: Option<&'a str>,
    pub previous_content_fingerprint: Option<&'a str>,
    pub selectable: bool,
    pub preserve_existing: bool,
    pub validation_errors: &'a [String],
    pub warnings: &'a [String],
    pub default_selected: bool,
}

/// SQLite 回调只负责读取原始值，组合协议在离开回调后统一验证。
struct RawStoredInstallPlan {
    id: String,
    kind: String,
    install_mode: String,
    input_path: Option<String>,
    input_device: i64,
    input_inode: i64,
    input_fingerprint: String,
    snapshot_relative_path: Option<String>,
    source_id: Option<String>,
    source_tracked_ref: Option<String>,
    source_catalog_generation: Option<i64>,
    source_marker: Option<String>,
    expected_source_marker: Option<String>,
    expected_current_target: Option<String>,
    expected_adopted_marker: Option<String>,
    bundle_id: String,
    bundle_display_name: String,
    warnings_json: String,
    created_at: i64,
    expires_at: i64,
    status: String,
}

pub struct NewGitHubSource<'a> {
    pub id: &'a str,
    pub canonical_identity: &'a str,
    pub owner: &'a str,
    pub repository: &'a str,
    pub display_name: &'a str,
    pub locator: &'a str,
    pub tracked_ref: &'a str,
    pub resolved_commit_sha: &'a str,
    pub member_path_hint: Option<&'a str>,
}

pub enum SaveGitHubSourceResult {
    Saved { source_id: String },
    RefChangeRequired { plan: SourceRefChangePlan },
}

/// Storage 会用完整 Source 快照签发 metadata-only Relink Plan。
pub(crate) struct NewEditableLocalRelinkPlan<'a> {
    pub id: &'a str,
    pub source: &'a StoredSourceInstallSource,
    pub candidate_path: &'a str,
    pub candidate_display_name: &'a str,
    pub candidate_marker: &'a str,
    pub members: &'a [EditableLocalRelinkMember],
    pub created_at: i64,
    pub expires_at: i64,
}

/// 确认阶段会重新读取候选目录，并与这里封存的事实逐项核对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredEditableLocalRelinkPlan {
    pub public: EditableLocalRelinkPlan,
    pub expected_canonical_identity: String,
    pub expected_device: u64,
    pub expected_inode: u64,
    pub expected_catalog_generation: i64,
    pub expected_catalog_marker: String,
    pub expected_bundle_id: Option<String>,
    pub candidate_marker: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredGithubSource {
    pub id: String,
    pub canonical_identity: String,
    pub owner: String,
    pub repository: String,
    pub display_name: String,
    pub tracked_ref: String,
}

/// Update Check 只读取已关联 GitHub Source 的稳定身份和现有采用基线。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredGithubBundleUpdateSource {
    pub source_id: String,
    pub bundle_id: String,
    pub canonical_identity: String,
    pub locator: String,
    pub tracked_ref: String,
    pub adopted_marker: Option<String>,
}

/// Lifecycle 组 Plan 时只接收同一个 SQLite 快照中的 Fresh Catalog 与本地关联状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSourceInstallSource {
    pub id: String,
    pub kind: String,
    pub canonical_identity: String,
    pub owner: Option<String>,
    pub repository: Option<String>,
    pub display_name: String,
    pub locator: String,
    pub tracked_ref: Option<String>,
    pub filesystem_device: Option<u64>,
    pub filesystem_inode: Option<u64>,
    pub catalog_generation: i64,
    pub catalog_marker: String,
    pub catalog_members: Vec<StoredSourceInstallCatalogMember>,
    pub bundle: Option<StoredSourceInstallBundle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSourceInstallCatalogMember {
    pub id: String,
    pub relative_path: String,
    pub skill_name: Option<String>,
    pub description: Option<String>,
    pub content_fingerprint: Option<String>,
    pub selectable: bool,
    pub validation_errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSourceInstallBundle {
    pub id: String,
    pub display_name: String,
    pub current_target: String,
    pub adopted_marker: Option<String>,
    pub update_check_status: BundleUpdateStatus,
    pub update_checked_marker: Option<String>,
    pub update_checked_at: Option<i64>,
    pub members: Vec<StoredSourceInstallBundleMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSourceInstallBundleMember {
    pub id: String,
    pub skill_name: String,
    pub description: String,
    pub stable_relative_path: String,
    pub content_fingerprint: String,
    pub source_relative_path: Option<String>,
}

/// 关联 Plan 必须从一个 SQLite 读快照取得 Bundle、成员和 Mount。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredSourceAssociationBundle {
    pub id: String,
    pub display_name: String,
    pub managed_directory: String,
    pub current_target: String,
    pub source_id: Option<String>,
    pub adopted_marker: Option<String>,
    pub members: Vec<StoredSourceAssociationBundleMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredSourceAssociationBundleMember {
    pub id: String,
    pub skill_name: String,
    pub description: String,
    pub stable_relative_path: String,
    pub content_fingerprint: String,
    pub source_relative_path: Option<String>,
    pub mounts: Vec<StoredMount>,
}

/// Takeover 补充既有 Bundle 时封存完整 SQLite 前置状态，确认与恢复只接受这一个快照。
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StoredTakeoverBundleSnapshot {
    pub id: String,
    pub display_name: String,
    pub managed_directory: String,
    pub current_target: String,
    pub source_id: Option<String>,
    pub source_display_name: Option<String>,
    pub adopted_marker: Option<String>,
    pub members: Vec<StoredTakeoverBundleMemberSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StoredTakeoverBundleMemberSnapshot {
    pub id: String,
    pub skill_name: String,
    pub description: String,
    pub stable_relative_path: String,
    pub content_fingerprint: String,
    pub source_relative_path: Option<String>,
    pub installation_chain: Option<InstallationChain>,
    pub mounts: Vec<StoredTakeoverMountSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StoredTakeoverMountSnapshot {
    pub id: String,
    pub member_id: String,
    pub bundle_id: String,
    pub skill_name: String,
    pub member_fingerprint: String,
    pub app_id: SupportedAppId,
    pub scope: MountScope,
    pub project_id: Option<String>,
    pub project_display_name: Option<String>,
    pub project_root_path: Option<String>,
    pub project_root_device: Option<u64>,
    pub project_root_inode: Option<u64>,
    pub target_path: String,
    pub expected_target: String,
    pub health: MountHealth,
}

/// 直接关联在同一个 SQLite 事务中重检这份完整成员快照。
pub(crate) struct DirectSourceAssociation<'a> {
    pub plan_id: &'a str,
    pub source_id: &'a str,
    pub source_catalog_generation: i64,
    pub source_marker: &'a str,
    pub bundle_id: &'a str,
    pub expected_current_target: &'a str,
    pub expected_members: &'a [DirectSourceAssociationMember<'a>],
    pub member_mappings: &'a [DirectSourceAssociationMemberMapping<'a>],
    pub now: i64,
}

pub(crate) struct DirectSourceAssociationMember<'a> {
    pub member_id: &'a str,
    pub content_fingerprint: &'a str,
}

pub(crate) struct DirectSourceAssociationMemberMapping<'a> {
    pub member_id: &'a str,
    pub source_relative_path: &'a str,
}

/// Merge 最终成员只能复用原两个 Bundle 中的成员 ID。
pub(crate) struct FinalSourceAssociationMember<'a> {
    pub member_id: &'a str,
    pub skill_name: &'a str,
    pub description: &'a str,
    pub stable_relative_path: &'a str,
    pub content_fingerprint: &'a str,
}

/// 每个原 Mount 必须且只能指派给一个最终成员。
pub(crate) struct FinalSourceAssociationMountAssignment<'a> {
    pub mount_id: &'a str,
    pub member_id: &'a str,
}

/// “不对应”的最终成员不会出现在 Source mapping 列表中。
pub(crate) struct FinalSourceAssociationMemberMapping<'a> {
    pub source_relative_path: &'a str,
    pub member_id: &'a str,
}

/// Storage 只提交已由编排层准备好的最终状态，并以两个完整原始快照防止竞态覆盖。
pub(crate) struct FinalSourceAssociationMerge<'a> {
    pub transaction_id: &'a str,
    pub source_id: &'a str,
    pub expected_target_bundle: &'a StoredSourceAssociationBundle,
    pub expected_retiring_bundle: &'a StoredSourceAssociationBundle,
    pub final_current_target: &'a str,
    pub final_members: &'a [FinalSourceAssociationMember<'a>],
    pub mount_assignments: &'a [FinalSourceAssociationMountAssignment<'a>],
    pub source_mappings: &'a [FinalSourceAssociationMemberMapping<'a>],
    pub now: i64,
}

pub struct NewSourceCatalogMember<'a> {
    pub id: &'a str,
    pub relative_path: &'a str,
    pub skill_name: Option<&'a str>,
    pub description: Option<&'a str>,
    pub content_fingerprint: Option<&'a str>,
    pub selectable: bool,
    pub validation_errors: &'a [String],
    pub warnings: &'a [String],
}

/// Archive、URL 与 Editable Local 在内容验证后一次性保存 Source 与 Fresh Catalog。
pub struct NewManualSource<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub canonical_identity: &'a str,
    pub display_name: &'a str,
    pub locator: &'a str,
    pub filesystem_device: Option<u64>,
    pub filesystem_inode: Option<u64>,
    pub catalog_marker: &'a str,
    pub members: &'a [NewSourceCatalogMember<'a>],
    pub saved_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedManualSource {
    pub source_id: String,
    pub catalog_generation: i64,
}

#[derive(Debug, Clone)]
pub struct StoredLifecycleTransaction {
    pub id: String,
    pub plan_id: String,
    pub bundle_id: String,
    pub member_id: String,
    pub journal_path: String,
    pub phase: String,
    pub status: String,
}

pub(crate) struct NewRemovalPlan<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub target_id: &'a str,
    pub payload_json: &'a str,
    pub payload_sha256: &'a str,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredRemovalPlan {
    pub id: String,
    pub kind: String,
    pub target_id: String,
    pub payload_json: String,
    pub payload_sha256: String,
    pub status: String,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredRemovalTransaction {
    pub id: String,
    pub plan_id: String,
    pub kind: String,
    pub target_id: String,
    pub journal_path: String,
    pub phase: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredProject {
    pub id: String,
    pub display_name: String,
    pub root_path: String,
    pub root_device: u64,
    pub root_inode: u64,
    pub created_at: i64,
}

pub(crate) struct NewProject<'a> {
    pub id: &'a str,
    pub display_name: &'a str,
    pub root_path: &'a str,
    pub root_device: u64,
    pub root_inode: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredManagedMember {
    pub id: String,
    pub bundle_id: String,
    pub skill_name: String,
    pub content_fingerprint: String,
    pub managed_directory: String,
    pub current_target: String,
    pub stable_relative_path: String,
    /// Mount 始终指向 Bundle 的稳定 `current` 入口，不直接绑定某次内容目录。
    pub expected_target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredMount {
    pub id: String,
    pub member_id: String,
    pub bundle_id: String,
    pub skill_name: String,
    pub member_fingerprint: String,
    pub app_id: SupportedAppId,
    pub scope: MountScope,
    pub project_id: Option<String>,
    pub project_display_name: Option<String>,
    pub project_root_path: Option<String>,
    pub project_root_device: Option<u64>,
    pub project_root_inode: Option<u64>,
    pub target_path: String,
    pub expected_target: String,
    pub health: MountHealth,
}

pub(crate) struct NewMountPlan<'a> {
    pub id: &'a str,
    pub operation: MountOperation,
    pub purpose: MountPlanPurpose,
    pub mount_id: &'a str,
    pub member_id: &'a str,
    pub app_id: SupportedAppId,
    pub scope: MountScope,
    pub project_id: Option<&'a str>,
    pub target_path: &'a str,
    pub expected_target: &'a str,
    pub member_fingerprint: &'a str,
    /// 生命周期层生成的不透明快照，用于确认目标路径仍处于预览状态。
    pub target_observation: &'a str,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredMountPlan {
    pub id: String,
    pub operation: MountOperation,
    pub purpose: MountPlanPurpose,
    pub mount_id: String,
    pub member_id: String,
    pub bundle_id: String,
    pub skill_name: String,
    pub app_id: SupportedAppId,
    pub scope: MountScope,
    pub project_id: Option<String>,
    pub project_display_name: Option<String>,
    pub project_root_path: Option<String>,
    pub project_root_device: Option<u64>,
    pub project_root_inode: Option<u64>,
    pub target_path: String,
    pub expected_target: String,
    pub member_fingerprint: String,
    pub target_observation: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredMountTransaction {
    pub id: String,
    pub plan_id: String,
    pub mount_id: String,
    pub operation: MountOperation,
    pub journal_path: String,
    pub phase: String,
    pub status: String,
}

pub(crate) struct NewBatchMountPlan<'a> {
    pub id: &'a str,
    pub bundle_id: &'a str,
    pub items: &'a [NewBatchMountPlanItem<'a>],
    pub created_at: i64,
    pub expires_at: i64,
}

pub(crate) struct NewBatchMountPlanItem<'a> {
    pub id: &'a str,
    pub mount_id: &'a str,
    pub member_id: &'a str,
    pub app_id: SupportedAppId,
    pub scope: MountScope,
    pub project_id: Option<&'a str>,
    pub target_path: &'a str,
    pub expected_target: &'a str,
    pub member_fingerprint: &'a str,
    /// 生命周期层生成的不透明目标快照；确认时会再次与文件系统事实核对。
    pub target_observation: &'a str,
    pub disposition: BatchMountDisposition,
    pub selectable: bool,
    pub default_selected: bool,
    pub conflict_reason: Option<&'a str>,
    pub target_health: MountHealth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredBatchMountPlan {
    pub id: String,
    pub bundle_id: String,
    pub bundle_display_name: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub status: String,
    pub items: Vec<StoredBatchMountPlanItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredBatchMountPlanItem {
    pub id: String,
    pub mount_id: String,
    pub member_id: String,
    pub bundle_id: String,
    pub skill_name: String,
    pub app_id: SupportedAppId,
    pub scope: MountScope,
    pub project_id: Option<String>,
    pub project_display_name: Option<String>,
    pub project_root_path: Option<String>,
    pub project_root_device: Option<u64>,
    pub project_root_inode: Option<u64>,
    pub target_path: String,
    pub expected_target: String,
    pub member_fingerprint: String,
    pub target_observation: String,
    pub disposition: BatchMountDisposition,
    pub selectable: bool,
    pub default_selected: bool,
    pub selected: bool,
    pub conflict_reason: Option<String>,
    pub target_health: MountHealth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredBatchMountTransaction {
    pub id: String,
    pub plan_id: String,
    pub bundle_id: String,
    pub journal_path: String,
    pub phase: String,
    pub status: String,
    pub selected_item_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LifecyclePhase {
    JournalPending,
    JournalReady,
    CandidateReady,
    Activated,
    StateCommitted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MountTransactionPhase {
    JournalPending,
    JournalReady,
    TargetApplied,
    StateCommitted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BatchMountTransactionPhase {
    JournalPending,
    JournalReady,
    Applying,
    TargetsApplied,
    RollingBack,
    StateCommitted,
}

impl BatchMountTransactionPhase {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "journal_pending" => Some(Self::JournalPending),
            "journal_ready" => Some(Self::JournalReady),
            "applying" => Some(Self::Applying),
            "targets_applied" => Some(Self::TargetsApplied),
            "rolling_back" => Some(Self::RollingBack),
            "state_committed" => Some(Self::StateCommitted),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::JournalPending => "journal_pending",
            Self::JournalReady => "journal_ready",
            Self::Applying => "applying",
            Self::TargetsApplied => "targets_applied",
            Self::RollingBack => "rolling_back",
            Self::StateCommitted => "state_committed",
        }
    }

    fn previous(self) -> Option<Self> {
        match self {
            Self::JournalPending => Some(Self::JournalPending),
            Self::JournalReady => Some(Self::JournalPending),
            Self::Applying => Some(Self::JournalReady),
            Self::TargetsApplied => Some(Self::Applying),
            Self::RollingBack | Self::StateCommitted => None,
        }
    }
}

impl MountTransactionPhase {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "journal_pending" => Some(Self::JournalPending),
            "journal_ready" => Some(Self::JournalReady),
            "target_applied" => Some(Self::TargetApplied),
            "state_committed" => Some(Self::StateCommitted),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::JournalPending => "journal_pending",
            Self::JournalReady => "journal_ready",
            Self::TargetApplied => "target_applied",
            Self::StateCommitted => "state_committed",
        }
    }

    fn previous(self) -> Option<Self> {
        match self {
            Self::JournalPending => Some(Self::JournalPending),
            Self::JournalReady => Some(Self::JournalPending),
            Self::TargetApplied => Some(Self::JournalReady),
            Self::StateCommitted => None,
        }
    }
}

impl LifecyclePhase {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "journal_pending" => Some(Self::JournalPending),
            "journal_ready" => Some(Self::JournalReady),
            "candidate_ready" => Some(Self::CandidateReady),
            "activated" => Some(Self::Activated),
            "state_committed" => Some(Self::StateCommitted),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::JournalPending => "journal_pending",
            Self::JournalReady => "journal_ready",
            Self::CandidateReady => "candidate_ready",
            Self::Activated => "activated",
            Self::StateCommitted => "state_committed",
        }
    }

    fn previous(self) -> Option<Self> {
        match self {
            Self::JournalPending => Some(Self::JournalPending),
            Self::JournalReady => Some(Self::JournalPending),
            Self::CandidateReady => Some(Self::JournalReady),
            Self::Activated => Some(Self::CandidateReady),
            Self::StateCommitted => None,
        }
    }
}

impl Storage {
    pub fn open(data_root: &Path, database: &Path) -> Result<Self, StorageError> {
        ensure_safe_data_root(data_root)?;
        let database_open_path = safe_database_open_path(data_root, database)?;
        // SQLite 自己在最终打开点执行 no-follow，补上检查与 open 之间的竞态防线。
        let connection = Connection::open_with_flags(
            database_open_path,
            OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(StorageError::OpenDatabase)?;
        let mut storage = Self {
            connection,
            data_root: data_root.to_owned(),
        };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&mut self) -> Result<(), StorageError> {
        // migration 目录自身必须先存在，后续版本才可以被真正跳过。
        self.connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA synchronous = FULL;
                 PRAGMA fullfsync = ON;
                 CREATE TABLE IF NOT EXISTS schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                 );",
            )
            .map_err(StorageError::Migration)?;

        for (version, migration) in MIGRATIONS {
            // IMMEDIATE 锁让多个同时启动的进程在检查版本前排队，避免重复执行 ALTER TABLE。
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(StorageError::Migration)?;
            let applied = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
                    [version],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(StorageError::Migration)?;
            if !applied {
                transaction
                    .execute_batch(migration)
                    .map_err(StorageError::Migration)?;
                transaction
                    .execute(
                        "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, unixepoch())",
                        [version],
                    )
                    .map_err(StorageError::Migration)?;
            }
            transaction.commit().map_err(StorageError::Migration)?;
        }
        Ok(())
    }

    pub(crate) fn read_interface_language(&self) -> Result<InterfaceLanguage, StorageError> {
        let stored = self
            .connection
            .query_row(
                "SELECT interface_language FROM app_preferences WHERE singleton_id = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .map_err(StorageError::ReadPreferences)?;
        InterfaceLanguage::from_str(&stored).ok_or(StorageError::UnknownInterfaceLanguage(stored))
    }

    pub(crate) fn save_interface_language(
        &mut self,
        language: InterfaceLanguage,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection
            .execute(
                "UPDATE app_preferences
                 SET interface_language = ?1
                 WHERE singleton_id = 1",
                [language.as_str()],
            )
            .map_err(StorageError::SavePreferences)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StorageError::SavePreferences(
                rusqlite::Error::QueryReturnedNoRows,
            ))
        }
    }

    pub(crate) fn read_ai_preferences(&self) -> Result<AiPreferences, StorageError> {
        let (enabled, disclosure_accepted, provider, model, verified) = self
            .connection
            .query_row(
                "SELECT enabled, disclosure_accepted, provider, model, verified
                 FROM ai_preferences
                 WHERE singleton_id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, bool>(0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, bool>(4)?,
                    ))
                },
            )
            .map_err(StorageError::ReadAiPreferences)?;
        let provider = AiProvider::from_str(&provider)
            .ok_or_else(|| StorageError::UnknownAiProvider(provider))?;
        Ok(AiPreferences {
            enabled,
            disclosure_accepted,
            provider,
            model,
            // Keychain 状态由 Application 在返回界面前填充。
            has_api_key: false,
            verified,
        })
    }

    pub(crate) fn save_ai_configuration(
        &mut self,
        enabled: bool,
        disclosure_accepted: bool,
        provider: AiProvider,
        model: &str,
    ) -> Result<(), StorageError> {
        let current = self.read_ai_preferences()?;
        let invalidates_verification = current.provider != provider || current.model != model;
        self.connection
            .execute(
                "UPDATE ai_preferences
                 SET enabled = ?1,
                     disclosure_accepted = ?2,
                     provider = ?3,
                     model = ?4,
                     verified = CASE WHEN ?5 THEN 0 ELSE verified END
                 WHERE singleton_id = 1",
                params![
                    enabled,
                    disclosure_accepted,
                    provider.as_str(),
                    model,
                    invalidates_verification
                ],
            )
            .map_err(StorageError::SaveAiPreferences)?;
        Ok(())
    }

    pub(crate) fn set_ai_verified(&mut self, verified: bool) -> Result<(), StorageError> {
        self.connection
            .execute(
                "UPDATE ai_preferences
                 SET verified = ?1
                 WHERE singleton_id = 1",
                [verified],
            )
            .map_err(StorageError::SaveAiPreferences)?;
        Ok(())
    }

    pub(crate) fn save_takeover_plan(
        &mut self,
        plan: &StoredTakeoverPlanRow,
    ) -> Result<(), StorageError> {
        if plan.id.is_empty()
            || plan.payload_json.is_empty()
            || plan.payload_sha256.len() != 64
            || !plan
                .payload_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || plan.status != "pending"
            || plan.created_at < 0
            || plan.expires_at < plan.created_at
        {
            return Err(StorageError::InvalidTakeoverPlan);
        }
        self.connection
            .execute(
                "INSERT INTO takeover_plans (
                    id, payload_json, payload_sha256, status, created_at, expires_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    plan.id,
                    plan.payload_json,
                    plan.payload_sha256,
                    plan.status,
                    plan.created_at,
                    plan.expires_at
                ],
            )
            .map_err(StorageError::SaveTakeoverPlan)?;
        Ok(())
    }

    pub(crate) fn read_takeover_plan(
        &self,
        plan_id: &str,
    ) -> Result<StoredTakeoverPlanRow, StorageError> {
        read_takeover_plan_from(&self.connection, plan_id)?
            .ok_or(StorageError::TakeoverPlanNotFound)
    }

    pub(crate) fn save_source_association_plan(
        &mut self,
        plan: &StoredSourceAssociationPlanRow,
    ) -> Result<(), StorageError> {
        validate_source_association_plan_row(plan)?;
        if plan.status != "pending" {
            return Err(StorageError::InvalidSourceAssociationPlan);
        }
        self.connection
            .execute(
                "INSERT INTO source_association_plans (
                    id, payload_json, payload_sha256, status, created_at, expires_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    plan.id,
                    plan.payload_json,
                    plan.payload_sha256,
                    plan.status,
                    plan.created_at,
                    plan.expires_at
                ],
            )
            .map_err(StorageError::SaveSourceAssociationPlan)?;
        Ok(())
    }

    pub(crate) fn read_source_association_plan(
        &self,
        plan_id: &str,
    ) -> Result<StoredSourceAssociationPlanRow, StorageError> {
        read_source_association_plan_from(&self.connection, plan_id)?
            .ok_or(StorageError::SourceAssociationPlanNotFound)
    }

    pub(crate) fn discard_source_association_plan(
        &mut self,
        plan_id: &str,
    ) -> Result<(), StorageError> {
        let deleted = self
            .connection
            .execute(
                "DELETE FROM source_association_plans
                 WHERE id = ?1 AND status = 'pending'",
                [plan_id],
            )
            .map_err(StorageError::SaveSourceAssociationPlan)?;
        if deleted == 1 {
            Ok(())
        } else if self
            .connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM source_association_plans WHERE id = ?1
                 )",
                [plan_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StorageError::ReadSourceAssociationPlan)?
        {
            Err(StorageError::SourceAssociationPlanConsumed)
        } else {
            Err(StorageError::SourceAssociationPlanNotFound)
        }
    }

    /// Merge 开始时在一个 Immediate 事务中消费 Plan，并封存恢复所需的全部标识。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn begin_source_association_merge(
        &mut self,
        plan_id: &str,
        transaction_id: &str,
        source_id: &str,
        source_catalog_generation: i64,
        source_marker: &str,
        expected_target_bundle: &StoredSourceAssociationBundle,
        expected_retiring_bundle: &StoredSourceAssociationBundle,
        content_choices_json: &str,
        source_mappings: &[FinalSourceAssociationMemberMapping<'_>],
        journal_path: &str,
        now: i64,
    ) -> Result<StoredSourceAssociationPlanRow, StorageError> {
        let target_bundle_id = expected_target_bundle.id.as_str();
        let retiring_bundle_id = expected_retiring_bundle.id.as_str();
        let original_member_ids = expected_target_bundle
            .members
            .iter()
            .chain(expected_retiring_bundle.members.iter())
            .map(|member| member.id.as_str())
            .collect::<BTreeSet<_>>();
        let source_mappings_json =
            canonical_source_association_mappings_json(source_mappings, &original_member_ids)?;
        let final_source_paths = source_mappings
            .iter()
            .map(|mapping| mapping.source_relative_path)
            .collect::<BTreeSet<_>>();
        if expected_target_bundle
            .members
            .iter()
            .filter_map(|member| member.source_relative_path.as_deref())
            .any(|source_path| !final_source_paths.contains(source_path))
        {
            return Err(StorageError::InvalidSourceAssociationPlan);
        }
        if !is_single_path_component(transaction_id)
            || !is_single_path_component(source_id)
            || !is_single_path_component(target_bundle_id)
            || !is_single_path_component(retiring_bundle_id)
            || target_bundle_id == retiring_bundle_id
            || source_catalog_generation <= 0
            || source_marker.is_empty()
            || !is_normalized_relative_path(journal_path)
            || serde_json::from_str::<Vec<serde_json::Value>>(content_choices_json).is_err()
        {
            return Err(StorageError::InvalidSourceAssociationPlan);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        let mut plan = read_source_association_plan_from(&transaction, plan_id)?
            .ok_or(StorageError::SourceAssociationPlanNotFound)?;
        ensure_source_association_plan_is_confirmable(&plan, now)?;
        let source_state = transaction
            .query_row(
                "SELECT catalog_status, catalog_generation, catalog_marker
                 FROM sources
                 WHERE id = ?1",
                [source_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(StorageError::ReadSources)?
            .ok_or(StorageError::SourceNotFound)?;
        if source_state.0 != "fresh"
            || source_state.1 != source_catalog_generation
            || source_state.2.as_deref() != Some(source_marker)
        {
            return Err(StorageError::SourceCatalogStateChanged);
        }
        ensure_source_association_merge_relationships(
            &transaction,
            source_id,
            target_bundle_id,
            retiring_bundle_id,
        )?;
        if bundle_or_source_write_is_blocked(&transaction, Some(target_bundle_id), Some(source_id))?
            || bundle_or_source_write_is_blocked(&transaction, Some(retiring_bundle_id), None)?
        {
            return Err(StorageError::ManagedObjectBlocked);
        }
        let current_target =
            read_source_association_bundle_from(&transaction, &self.data_root, target_bundle_id)?;
        let current_retiring =
            read_source_association_bundle_from(&transaction, &self.data_root, retiring_bundle_id)?;
        if &current_target != expected_target_bundle
            || &current_retiring != expected_retiring_bundle
        {
            return Err(StorageError::SourceBundleStateConflict);
        }
        validate_source_association_mappings_for_generation(
            &transaction,
            source_id,
            source_catalog_generation,
            source_mappings,
        )?;

        // Merge 会删除或移动成员；先在同一事务内作废仍引用旧快照的 pending Plan。
        transaction
            .execute(
                "DELETE FROM mount_plans
                 WHERE status = 'pending'
                   AND member_id IN (
                       SELECT id FROM skill_members WHERE bundle_id IN (?1, ?2)
                   )",
                params![target_bundle_id, retiring_bundle_id],
            )
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        transaction
            .execute(
                "DELETE FROM batch_mount_plans
                 WHERE status = 'pending' AND bundle_id IN (?1, ?2)",
                params![target_bundle_id, retiring_bundle_id],
            )
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        transaction
            .execute(
                "INSERT INTO source_association_transactions (
                    id, plan_id, source_id, target_bundle_id, retiring_bundle_id,
                    content_choices_json, source_mappings_json, journal_path,
                    phase, status, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                    'journal_pending', 'in_progress', ?9, ?9
                 )",
                params![
                    transaction_id,
                    plan_id,
                    source_id,
                    target_bundle_id,
                    retiring_bundle_id,
                    content_choices_json,
                    source_mappings_json,
                    journal_path,
                    now
                ],
            )
            .map_err(map_source_association_transaction_insert_error)?;
        let consumed = transaction
            .execute(
                "UPDATE source_association_plans
                 SET status = 'consumed'
                 WHERE id = ?1 AND status = 'pending'",
                [plan_id],
            )
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        ensure_one_source_association_row(consumed, transaction_id)?;
        plan.status = "consumed".to_owned();
        transaction
            .commit()
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        Ok(plan)
    }

    pub(crate) fn recoverable_source_association_transactions(
        &self,
    ) -> Result<Vec<StoredSourceAssociationTransaction>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, plan_id, source_id, target_bundle_id, retiring_bundle_id,
                        content_choices_json, source_mappings_json, journal_path, phase, status
                 FROM source_association_transactions
                 ORDER BY created_at, id",
            )
            .map_err(StorageError::ReadSourceAssociationTransaction)?;
        let rows = statement
            .query_map([], stored_source_association_transaction_from_row)
            .map_err(StorageError::ReadSourceAssociationTransaction)?;
        let stored = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadSourceAssociationTransaction)?;
        for transaction in &stored {
            validate_stored_source_association_transaction(transaction)?;
        }
        Ok(stored)
    }

    pub(crate) fn update_source_association_transaction_phase(
        &mut self,
        transaction_id: &str,
        phase: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let previous = previous_source_association_phase(phase)?.ok_or_else(|| {
            StorageError::SourceAssociationStateConflict(transaction_id.to_owned())
        })?;
        let changed = self
            .connection
            .execute(
                "UPDATE source_association_transactions
                 SET phase = ?2, updated_at = ?4
                 WHERE id = ?1 AND status = 'in_progress' AND phase IN (?2, ?3)",
                params![transaction_id, phase, previous, now],
            )
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        ensure_one_source_association_row(changed, transaction_id)
    }

    pub(crate) fn abort_source_association_transaction(
        &mut self,
        transaction_id: &str,
        error_message: Option<&str>,
        now: i64,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection
            .execute(
                "UPDATE source_association_transactions
                 SET status = 'aborted', error_message = ?2, updated_at = ?3
                 WHERE id = ?1 AND status = 'in_progress' AND phase != 'state_committed'",
                params![transaction_id, error_message, now],
            )
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        ensure_one_source_association_row(changed, transaction_id)
    }

    pub(crate) fn block_source_association_transaction(
        &mut self,
        transaction_id: &str,
        error_message: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection
            .execute(
                "UPDATE source_association_transactions
                 SET status = 'blocked', error_message = ?2, updated_at = ?3
                 WHERE id = ?1 AND status IN ('in_progress', 'completed', 'aborted')",
                params![transaction_id, error_message, now],
            )
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        ensure_one_source_association_row(changed, transaction_id)
    }

    pub(crate) fn forget_terminal_source_association_transaction(
        &mut self,
        transaction_id: &str,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        let stored = transaction
            .query_row(
                "SELECT plan_id, status
                 FROM source_association_transactions
                 WHERE id = ?1",
                [transaction_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        if let Some((plan_id, status)) = stored {
            if !matches!(status.as_str(), "completed" | "aborted") {
                return Err(StorageError::SourceAssociationStateConflict(
                    transaction_id.to_owned(),
                ));
            }
            transaction
                .execute(
                    "DELETE FROM source_association_transactions WHERE id = ?1",
                    [transaction_id],
                )
                .map_err(StorageError::SaveSourceAssociationTransaction)?;
            transaction
                .execute(
                    "DELETE FROM source_association_plans WHERE id = ?1",
                    [plan_id],
                )
                .map_err(StorageError::SaveSourceAssociationTransaction)?;
        }
        transaction
            .commit()
            .map_err(StorageError::SaveSourceAssociationTransaction)
    }

    /// 开始时必须一次封存对象、路径与 Journal；拆成后续写入会留下未被隔离的窗口。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn begin_takeover_transaction(
        &mut self,
        plan_id: &str,
        transaction_id: &str,
        bundle_id: &str,
        member_id: &str,
        reserved_paths: &[String],
        journal_path: &str,
        now: i64,
    ) -> Result<StoredTakeoverPlanRow, StorageError> {
        if !is_single_path_component(transaction_id)
            || !is_single_path_component(bundle_id)
            || !is_single_path_component(member_id)
            || !takeover_reserved_paths_are_valid(reserved_paths)
            || !is_normalized_relative_path(journal_path)
        {
            return Err(StorageError::InvalidTakeoverPlan);
        }
        let reserved_paths_json =
            serde_json::to_string(reserved_paths).map_err(|_| StorageError::InvalidTakeoverPlan)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveTakeoverTransaction)?;
        let mut plan = read_takeover_plan_from(&transaction, plan_id)?
            .ok_or(StorageError::TakeoverPlanNotFound)?;
        if plan.status != "pending" {
            return Err(StorageError::TakeoverPlanConsumed);
        }
        if plan.expires_at <= now {
            return Err(StorageError::TakeoverPlanExpired);
        }
        transaction
            .execute(
                "INSERT INTO takeover_transactions (
                    id, plan_id, bundle_id, member_id, reserved_paths_json, journal_path,
                    phase, status, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'journal_pending', 'in_progress', ?7, ?7)",
                params![
                    transaction_id,
                    plan.id,
                    bundle_id,
                    member_id,
                    reserved_paths_json,
                    journal_path,
                    now
                ],
            )
            .map_err(map_takeover_transaction_insert_error)?;
        let consumed = transaction
            .execute(
                "UPDATE takeover_plans SET status = 'consumed' WHERE id = ?1 AND status = 'pending'",
                [&plan.id],
            )
            .map_err(StorageError::SaveTakeoverTransaction)?;
        ensure_one_takeover_row(consumed, transaction_id)?;
        plan.status = "consumed".to_owned();
        transaction
            .commit()
            .map_err(StorageError::SaveTakeoverTransaction)?;
        Ok(plan)
    }

    pub(crate) fn update_takeover_transaction_phase(
        &mut self,
        transaction_id: &str,
        phase: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let previous = previous_takeover_phase(phase)?
            .ok_or_else(|| StorageError::TakeoverStateConflict(transaction_id.to_owned()))?;
        let changed = self
            .connection
            .execute(
                "UPDATE takeover_transactions SET phase = ?2, updated_at = ?4
                 WHERE id = ?1 AND status = 'in_progress' AND phase IN (?2, ?3)",
                params![transaction_id, phase, previous, now],
            )
            .map_err(StorageError::SaveTakeoverTransaction)?;
        ensure_one_takeover_row(changed, transaction_id)
    }

    pub(crate) fn finalize_takeover(
        &mut self,
        transaction_id: &str,
        plan: &TakeoverPlan,
        existing_bundle: Option<&StoredTakeoverBundleSnapshot>,
        now: i64,
    ) -> Result<(), StorageError> {
        let validated = validate_takeover_domain_contract(&self.data_root, plan)?;
        let takeover_source = validate_takeover_source_contract(plan, existing_bundle)?;
        let anchor_member_id = &plan
            .members
            .first()
            .ok_or(StorageError::InvalidTakeoverPlan)?
            .member_id;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveTakeoverTransaction)?;
        let raw_state = transaction
            .query_row(
                "SELECT id, plan_id, bundle_id, member_id, reserved_paths_json,
                        journal_path, phase, status
                 FROM takeover_transactions WHERE id = ?1",
                [transaction_id],
                raw_takeover_transaction_from_row,
            )
            .optional()
            .map_err(StorageError::SaveTakeoverTransaction)?
            .ok_or_else(|| StorageError::TakeoverStateConflict(transaction_id.to_owned()))?;
        let state = decode_takeover_transaction(raw_state)?;
        let expected_reserved_paths = takeover_reserved_paths(plan)?;
        if state.id != transaction_id
            || state.plan_id != plan.id
            || state.bundle_id != plan.bundle_id
            || state.member_id != *anchor_member_id
            || state.reserved_paths != expected_reserved_paths
            || !is_normalized_relative_path(&state.journal_path)
        {
            return Err(StorageError::TakeoverStateConflict(
                transaction_id.to_owned(),
            ));
        }
        if state.phase == "origins_applied" && state.status == "in_progress" {
            if let Some(existing) = existing_bundle {
                let current = read_takeover_bundle_snapshot_from(
                    &transaction,
                    &self.data_root,
                    &existing.id,
                )?;
                if current != *existing
                    || existing.id != plan.bundle_id
                    || existing.display_name != plan.bundle_display_name
                    || existing.managed_directory != validated.managed_directory
                {
                    return Err(StorageError::TakeoverStateConflict(
                        transaction_id.to_owned(),
                    ));
                }
                let changed = transaction
                    .execute(
                        "UPDATE bundles SET current_target = ?2
                         WHERE id = ?1 AND current_target = ?3",
                        params![
                            plan.bundle_id,
                            validated.current_target,
                            existing.current_target
                        ],
                    )
                    .map_err(StorageError::SaveTakeoverTransaction)?;
                ensure_one_takeover_row(changed, transaction_id)?;
            } else {
                // 全新接管必须认领一组新领域 ID；任一冲突都会让整个 SQLite 事务回滚。
                transaction
                    .execute(
                        "INSERT INTO bundles (id, display_name, managed_directory, current_target, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![plan.bundle_id, plan.bundle_display_name, validated.managed_directory, validated.current_target, now],
                    )
                    .map_err(StorageError::SaveTakeoverTransaction)?;
            }
            for member in &plan.members {
                let validated_member = validated
                    .members
                    .get(&member.member_id)
                    .ok_or(StorageError::InvalidTakeoverPlan)?;
                transaction
                    .execute(
                        "INSERT INTO skill_members
                            (id, bundle_id, skill_name, description, stable_relative_path, content_fingerprint, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            member.member_id,
                            plan.bundle_id,
                            member.skill_name,
                            member.skill_description,
                            validated_member.stable_relative_path,
                            validated_member.fingerprint,
                            now
                        ],
                    )
                    .map_err(StorageError::SaveTakeoverTransaction)?;
                if let Some(chain) = &member.installation_chain {
                    insert_member_installation_chain(&transaction, &member.member_id, chain)
                        .map_err(StorageError::SaveTakeoverTransaction)?;
                }
                transaction
                    .execute(
                        "INSERT INTO member_selections (bundle_id, member_id, selected_at) VALUES (?1, ?2, ?3)",
                        params![plan.bundle_id, member.member_id, now],
                    )
                    .map_err(StorageError::SaveTakeoverTransaction)?;
            }
            for target in &plan.targets {
                transaction
                    .execute(
                        "INSERT INTO mounts
                        (id, member_id, app_id, scope, project_id, target_path, expected_target, health, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'healthy', ?8, ?8)",
                        params![target.mount_id, target.member_id, target.app_id.as_str(), target.scope.as_str(), target.project_id, target.target_path, target.expected_target, now],
                    )
                    .map_err(StorageError::SaveTakeoverTransaction)?;
            }
            let persisted_source_ref = takeover_source
                .as_ref()
                .map(|source| persist_takeover_source(&transaction, &plan.bundle_id, source, now))
                .transpose()?;
            ensure_takeover_domain_matches(
                &transaction,
                &self.data_root,
                plan,
                &validated,
                existing_bundle,
                takeover_source.as_ref(),
                persisted_source_ref.as_deref(),
            )?;
            for origin in &plan.origins {
                let deleted = transaction
                    .execute(
                        "DELETE FROM inventory_observations WHERE id = ?1 AND skill_root = ?2",
                        params![origin.observation_id, origin.original_path],
                    )
                    .map_err(StorageError::SaveTakeoverTransaction)?;
                ensure_one_takeover_row(deleted, transaction_id)?;
            }
            let changed = transaction
                .execute(
                    "UPDATE takeover_transactions SET phase = 'state_committed', status = 'completed', updated_at = ?3
                     WHERE id = ?1 AND plan_id = ?2 AND phase = 'origins_applied' AND status = 'in_progress'",
                    params![transaction_id, plan.id, now],
                )
                .map_err(StorageError::SaveTakeoverTransaction)?;
            ensure_one_takeover_row(changed, transaction_id)?;
        } else if state.phase == "state_committed" && state.status == "completed" {
            // 重放只验证已提交事实，不能把外部删除的领域行静默补回。
            ensure_takeover_domain_matches(
                &transaction,
                &self.data_root,
                plan,
                &validated,
                existing_bundle,
                takeover_source.as_ref(),
                None,
            )?;
        } else {
            return Err(StorageError::TakeoverStateConflict(
                transaction_id.to_owned(),
            ));
        }
        transaction
            .commit()
            .map_err(StorageError::SaveTakeoverTransaction)
    }

    pub(crate) fn abort_takeover_transaction(
        &mut self,
        transaction_id: &str,
        error_message: Option<&str>,
        now: i64,
    ) -> Result<(), StorageError> {
        let changed = self.connection.execute(
            "UPDATE takeover_transactions SET status = 'aborted', error_message = ?2, updated_at = ?3
             WHERE id = ?1 AND status = 'in_progress' AND phase != 'state_committed'",
            params![transaction_id, error_message, now],
        ).map_err(StorageError::SaveTakeoverTransaction)?;
        ensure_one_takeover_row(changed, transaction_id)
    }

    pub(crate) fn block_takeover_transaction(
        &mut self,
        transaction_id: &str,
        error_message: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let changed = self.connection.execute(
            "UPDATE takeover_transactions SET status = 'blocked', error_message = ?2, updated_at = ?3
             WHERE id = ?1 AND status IN ('in_progress', 'completed', 'aborted')",
            params![transaction_id, error_message, now],
        ).map_err(StorageError::SaveTakeoverTransaction)?;
        ensure_one_takeover_row(changed, transaction_id)
    }

    pub(crate) fn forget_terminal_takeover_transaction(
        &mut self,
        transaction_id: &str,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveTakeoverTransaction)?;
        let stored = transaction
            .query_row(
                "SELECT plan_id, status FROM takeover_transactions WHERE id = ?1",
                [transaction_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StorageError::SaveTakeoverTransaction)?;
        if let Some((plan_id, status)) = stored {
            if !matches!(status.as_str(), "completed" | "aborted") {
                return Err(StorageError::TakeoverStateConflict(
                    transaction_id.to_owned(),
                ));
            }
            transaction
                .execute(
                    "DELETE FROM takeover_transactions WHERE id = ?1",
                    [transaction_id],
                )
                .map_err(StorageError::SaveTakeoverTransaction)?;
            transaction
                .execute("DELETE FROM takeover_plans WHERE id = ?1", [plan_id])
                .map_err(StorageError::SaveTakeoverTransaction)?;
        }
        transaction
            .commit()
            .map_err(StorageError::SaveTakeoverTransaction)
    }

    pub(crate) fn recoverable_takeover_transactions(
        &self,
    ) -> Result<Vec<StoredTakeoverTransaction>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, plan_id, bundle_id, member_id, reserved_paths_json,
                        journal_path, phase, status
                 FROM takeover_transactions
                 ORDER BY created_at, id",
            )
            .map_err(StorageError::ReadTakeoverTransaction)?;
        let rows = statement
            .query_map([], raw_takeover_transaction_from_row)
            .map_err(StorageError::ReadTakeoverTransaction)?;
        let raw_stored = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadTakeoverTransaction)?;
        let stored = raw_stored
            .into_iter()
            .map(decode_takeover_transaction)
            .collect::<Result<Vec<_>, _>>()?;

        for transaction in &stored {
            if !is_single_path_component(&transaction.id)
                || !is_single_path_component(&transaction.plan_id)
                || !is_single_path_component(&transaction.bundle_id)
                || !is_single_path_component(&transaction.member_id)
                || !takeover_reserved_paths_are_valid(&transaction.reserved_paths)
                || !is_normalized_relative_path(&transaction.journal_path)
                || !matches!(
                    transaction.status.as_str(),
                    "in_progress" | "completed" | "aborted" | "blocked"
                )
                || !takeover_phase_and_status_are_consistent(
                    &transaction.phase,
                    &transaction.status,
                )
            {
                return Err(StorageError::TakeoverStateConflict(transaction.id.clone()));
            }
        }
        Ok(stored)
    }

    pub fn read_initial_scan(&self) -> Result<Option<UiOutcome>, StorageError> {
        let (initial_scan_completed_at, refresh_at, added, changed, removed): (
            Option<i64>,
            Option<i64>,
            i64,
            i64,
            i64,
        ) = self
            .connection
            .query_row(
                "SELECT initial_scan_completed_at, last_local_refresh_at, last_local_refresh_added, last_local_refresh_changed, last_local_refresh_removed FROM app_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .map_err(StorageError::ReadInventory)?;
        let Some(scan_completed_at) = initial_scan_completed_at else {
            return Ok(None);
        };

        let supported_apps = self.read_supported_apps()?;
        let entries = self.with_managed_entries(self.read_inventory_entries()?)?;
        let scan_issues = self.read_scan_issues()?;
        let recovery_issues = self.read_recovery_issues()?;
        let projects = self.read_projects()?;
        let mounts = self.read_mount_summaries()?;
        let bundle_updates = self.read_bundle_update_summaries()?;
        let last_local_refresh = refresh_at
            .map(|completed_at| {
                Ok(LocalRefreshSummary {
                    completed_at,
                    added: refresh_count(added)?,
                    changed: refresh_count(changed)?,
                    removed: refresh_count(removed)?,
                })
            })
            .transpose()?;

        Ok(Some(UiOutcome::Inventory {
            scan_completed_at,
            entries,
            supported_apps,
            last_local_refresh,
            scan_issues,
            recovery_issues,
            recovered_interrupted_operation: false,
            projects,
            mounts,
            bundle_updates,
        }))
    }

    /// Inventory 直接给出每个本地 Bundle 的更新入口，不要求前端拼接 Source 状态。
    pub fn read_bundle_update_summaries(&self) -> Result<Vec<BundleUpdateSummary>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT bundle.id, source.id, source.kind, source.locator,
                        link.adopted_marker, link.update_check_status,
                        link.update_checked_at, link.update_check_error
                 FROM bundles AS bundle
                 LEFT JOIN source_bundle_links AS link ON link.bundle_id = bundle.id
                 LEFT JOIN sources AS source ON source.id = link.source_id
                 ORDER BY bundle.display_name, bundle.id",
            )
            .map_err(StorageError::ReadSources)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            })
            .map_err(StorageError::ReadSources)?;
        let mut summaries = Vec::new();
        for row in rows {
            let (
                bundle_id,
                source_id,
                kind,
                locator,
                adopted_marker,
                stored_status,
                checked_at,
                check_error,
            ) = row.map_err(StorageError::ReadSources)?;
            let Some(source_id) = source_id else {
                summaries.push(BundleUpdateSummary {
                    bundle_id,
                    status: BundleUpdateStatus::NoSource,
                    action: None,
                    checked_at: None,
                    message: "没有更新来源".to_owned(),
                    upstream_url: None,
                });
                continue;
            };
            let kind = kind.ok_or_else(|| StorageError::UnknownSourceKind("NULL".to_owned()))?;
            let kind = SourceKind::from_str(&kind)
                .ok_or_else(|| StorageError::UnknownSourceKind(kind.clone()))?;
            let upstream_url = match kind {
                SourceKind::Github | SourceKind::DirectUrl => locator,
                SourceKind::Archive | SourceKind::EditableLocal => None,
            };
            let stored_status = stored_status
                .ok_or_else(|| StorageError::UnknownBundleUpdateStatus("NULL".to_owned()))?;
            let stored_status = BundleUpdateStatus::from_stored_str(&stored_status)
                .ok_or_else(|| StorageError::UnknownBundleUpdateStatus(stored_status.clone()))?;

            let (status, action, message) = match kind {
                SourceKind::Github
                    if adopted_marker.is_none()
                        && stored_status == BundleUpdateStatus::NotChecked =>
                {
                    (
                        BundleUpdateStatus::Available,
                        Some(BundleUpdateAction::Update),
                        "尚未建立上游基线，可以更新整个 Bundle".to_owned(),
                    )
                }
                SourceKind::Github => {
                    let action = if stored_status == BundleUpdateStatus::Available {
                        Some(BundleUpdateAction::Update)
                    } else {
                        None
                    };
                    let message = match stored_status {
                        BundleUpdateStatus::NotChecked => "尚未检查更新".to_owned(),
                        BundleUpdateStatus::Available => "发现新的上游 commit".to_owned(),
                        BundleUpdateStatus::UpToDate => "已是最新".to_owned(),
                        BundleUpdateStatus::UnableToCheck => check_error
                            .as_deref()
                            .map(|error| format!("无法检查更新：{error}"))
                            .unwrap_or_else(|| "无法检查更新".to_owned()),
                        BundleUpdateStatus::SourceUnavailable => "更新来源当前不可用".to_owned(),
                        BundleUpdateStatus::Manual | BundleUpdateStatus::NoSource => {
                            return Err(StorageError::UnknownBundleUpdateStatus(
                                stored_status
                                    .as_stored_str()
                                    .unwrap_or("derived")
                                    .to_owned(),
                            ));
                        }
                    };
                    (stored_status, action, message)
                }
                SourceKind::Archive | SourceKind::DirectUrl => (
                    BundleUpdateStatus::Manual,
                    Some(BundleUpdateAction::ImportReplacement),
                    "选择新的归档或文件来更新".to_owned(),
                ),
                SourceKind::EditableLocal if stored_status == BundleUpdateStatus::Available => (
                    BundleUpdateStatus::Available,
                    Some(BundleUpdateAction::Update),
                    "发现尚未采用的本地改动".to_owned(),
                ),
                SourceKind::EditableLocal => {
                    let message = match stored_status {
                        BundleUpdateStatus::NotChecked => "尚未检查本地改动",
                        BundleUpdateStatus::UpToDate => "已采用当前本地内容",
                        BundleUpdateStatus::SourceUnavailable => "本地来源当前不可用",
                        BundleUpdateStatus::UnableToCheck => "上次检查本地改动失败",
                        BundleUpdateStatus::Available => unreachable!("Available 已单独处理"),
                        BundleUpdateStatus::Manual | BundleUpdateStatus::NoSource => {
                            return Err(StorageError::UnknownBundleUpdateStatus(
                                stored_status
                                    .as_stored_str()
                                    .unwrap_or("derived")
                                    .to_owned(),
                            ));
                        }
                    };
                    (
                        stored_status,
                        Some(BundleUpdateAction::CheckEditableLocal),
                        message.to_owned(),
                    )
                }
            };
            let is_blocked = bundle_or_source_write_is_blocked(
                &self.connection,
                Some(&bundle_id),
                Some(&source_id),
            )?;
            summaries.push(BundleUpdateSummary {
                bundle_id,
                status,
                action: if is_blocked { None } else { action },
                checked_at,
                message: if is_blocked {
                    format!("{message}；等待人工恢复")
                } else {
                    message
                },
                upstream_url,
            });
        }
        Ok(summaries)
    }

    /// 发现页从一个 SQLite 快照读取 Source、Catalog 与已安装关系，不自行访问网络。
    pub fn read_source_summaries(&mut self) -> Result<Vec<SourceSummary>, StorageError> {
        // Source、Catalog generation 与成员必须来自同一个快照，不能和另一实例的 reload 拼接。
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(StorageError::ReadSources)?;
        let mut statement = transaction
            .prepare(
                "SELECT source.id, source.kind, source.canonical_identity, source.display_name,
                        source.locator, source.tracked_ref, source.member_path_hint,
                        source.catalog_status, source.catalog_marker,
                        source.catalog_fetched_at, source.last_reload_at,
                        source.last_reload_error, source.catalog_generation,
                        link.bundle_id, link.adopted_marker
                 FROM sources AS source
                 LEFT JOIN source_bundle_links AS link ON link.source_id = source.id
                 ORDER BY source.sort_order, source.id",
            )
            .map_err(StorageError::ReadSources)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                ))
            })
            .map_err(StorageError::ReadSources)?;
        let rows = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadSources)?;
        drop(statement);
        let mut sources = Vec::new();
        for row in rows {
            let (
                id,
                kind,
                canonical_identity,
                display_name,
                locator,
                tracked_ref,
                member_path_hint,
                catalog_status,
                catalog_marker,
                catalog_fetched_at,
                last_reload_at,
                last_reload_error,
                catalog_generation,
                bundle_id,
                adopted_marker,
            ) = row;
            let kind = SourceKind::from_str(&kind)
                .ok_or_else(|| StorageError::UnknownSourceKind(kind.clone()))?;
            let catalog_status = SourceCatalogStatus::from_str(&catalog_status)
                .ok_or_else(|| StorageError::UnknownSourceCatalogStatus(catalog_status.clone()))?;
            let members = read_source_catalog_members_from(&transaction, &id, catalog_generation)?;
            sources.push(SourceSummary {
                id,
                kind,
                canonical_identity,
                display_name,
                locator,
                tracked_ref,
                member_path_hint,
                catalog_status,
                catalog_marker,
                catalog_fetched_at,
                last_reload_at,
                last_reload_error,
                bundle_id,
                adopted_marker,
                members,
            });
        }
        transaction.commit().map_err(StorageError::ReadSources)?;
        Ok(sources)
    }

    /// 无 ref 的重复入口沿用当前 Tracked Ref；首次登记才读取 GitHub default branch。
    pub fn read_source_tracked_ref(
        &self,
        canonical_identity: &str,
    ) -> Result<Option<String>, StorageError> {
        self.connection
            .query_row(
                "SELECT tracked_ref
                 FROM sources
                 WHERE canonical_identity = ?1 AND kind = 'github'",
                [canonical_identity],
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::ReadSources)
    }

    pub fn read_github_source(&self, source_id: &str) -> Result<StoredGithubSource, StorageError> {
        self.connection
            .query_row(
                "SELECT id, canonical_identity, owner, repository, display_name, tracked_ref
                 FROM sources WHERE id = ?1 AND kind = 'github'",
                [source_id],
                stored_github_source_from_row,
            )
            .optional()
            .map_err(StorageError::ReadSources)?
            .ok_or(StorageError::SourceNotFound)
    }

    /// 全局检查只遍历已经关联本地 Bundle 的 GitHub Source。
    pub(crate) fn read_github_bundle_update_sources(
        &self,
    ) -> Result<Vec<StoredGithubBundleUpdateSource>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT source.id, link.bundle_id, source.canonical_identity,
                        source.locator, source.tracked_ref, link.adopted_marker
                 FROM sources AS source
                 JOIN source_bundle_links AS link ON link.source_id = source.id
                 WHERE source.kind = 'github'
                 ORDER BY source.sort_order, source.id",
            )
            .map_err(StorageError::ReadSources)?;
        let rows = statement
            .query_map([], |row| {
                Ok(StoredGithubBundleUpdateSource {
                    source_id: row.get(0)?,
                    bundle_id: row.get(1)?,
                    canonical_identity: row.get(2)?,
                    locator: row.get(3)?,
                    tracked_ref: row.get(4)?,
                    adopted_marker: row.get(5)?,
                })
            })
            .map_err(StorageError::ReadSources)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadSources)
    }

    /// 成功结果替换最近上游标识；失败方法则有意保留这个字段。
    pub(crate) fn save_bundle_update_check_success(
        &mut self,
        source_id: &str,
        bundle_id: &str,
        status: BundleUpdateStatus,
        upstream_marker: &str,
        checked_at: i64,
    ) -> Result<(), StorageError> {
        let Some(status) = status.as_stored_str() else {
            return Err(StorageError::InvalidSourceDefinition);
        };
        if !matches!(status, "available" | "up_to_date")
            || source_id.is_empty()
            || bundle_id.is_empty()
            || upstream_marker.is_empty()
            || checked_at < 0
        {
            return Err(StorageError::InvalidSourceDefinition);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSource)?;
        if bundle_or_source_write_is_blocked(&transaction, Some(bundle_id), Some(source_id))? {
            return Err(StorageError::ManagedObjectBlocked);
        }
        let changed = transaction
            .execute(
                "UPDATE source_bundle_links
                 SET update_check_status = ?3,
                     update_checked_marker = ?4,
                     update_checked_at = ?5,
                     update_check_error = NULL
                 WHERE source_id = ?1 AND bundle_id = ?2",
                params![source_id, bundle_id, status, upstream_marker, checked_at],
            )
            .map_err(StorageError::SaveSource)?;
        if changed != 1 {
            return Err(StorageError::SourceBundleStateConflict);
        }
        transaction.commit().map_err(StorageError::SaveSource)?;
        Ok(())
    }

    pub(crate) fn save_bundle_update_check_failure(
        &mut self,
        source_id: &str,
        bundle_id: &str,
        checked_at: i64,
        error: &str,
    ) -> Result<(), StorageError> {
        if source_id.is_empty() || bundle_id.is_empty() || checked_at < 0 || error.is_empty() {
            return Err(StorageError::InvalidSourceDefinition);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSource)?;
        if bundle_or_source_write_is_blocked(&transaction, Some(bundle_id), Some(source_id))? {
            return Err(StorageError::ManagedObjectBlocked);
        }
        let changed = transaction
            .execute(
                "UPDATE source_bundle_links
                 SET update_check_status = 'unable_to_check',
                     update_checked_at = ?3,
                     update_check_error = ?4
                 WHERE source_id = ?1 AND bundle_id = ?2",
                params![source_id, bundle_id, checked_at, error],
            )
            .map_err(StorageError::SaveSource)?;
        if changed != 1 {
            return Err(StorageError::SourceBundleStateConflict);
        }
        transaction.commit().map_err(StorageError::SaveSource)?;
        Ok(())
    }

    /// Editable Local 检查在一个事务中替换 Catalog 并记录与 adopted marker 的比较结果。
    pub(crate) fn save_editable_local_check_success(
        &mut self,
        expected: &StoredSourceInstallSource,
        marker: &str,
        checked_at: i64,
        members: &[NewSourceCatalogMember<'_>],
    ) -> Result<BundleUpdateStatus, StorageError> {
        let bundle = expected
            .bundle
            .as_ref()
            .ok_or(StorageError::SourceBundleStateConflict)?;
        let expected_device = expected
            .filesystem_device
            .map(filesystem_identity_to_sql)
            .transpose()?;
        let expected_inode = expected
            .filesystem_inode
            .map(filesystem_identity_to_sql)
            .transpose()?;
        if expected.kind != SourceKind::EditableLocal.as_str()
            || expected_device.is_none()
            || expected_inode.is_none()
            || marker.is_empty()
            || checked_at < 0
            || members.is_empty()
        {
            return Err(StorageError::InvalidSourceDefinition);
        }
        let encoded_members = members
            .iter()
            .map(|member| {
                Ok((
                    serde_json::to_string(member.validation_errors)
                        .map_err(StorageError::SerializeSourceCatalogMetadata)?,
                    serde_json::to_string(member.warnings)
                        .map_err(StorageError::SerializeSourceCatalogMetadata)?,
                ))
            })
            .collect::<Result<Vec<_>, StorageError>>()?;

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSourceCatalog)?;
        if bundle_or_source_write_is_blocked(&transaction, Some(&bundle.id), Some(&expected.id))? {
            return Err(StorageError::ManagedObjectBlocked);
        }
        let source_state = transaction
            .query_row(
                "SELECT kind, canonical_identity, locator, filesystem_device,
                        filesystem_inode, catalog_generation, catalog_marker
                 FROM sources
                 WHERE id = ?1",
                [&expected.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(StorageError::SaveSourceCatalog)?
            .ok_or(StorageError::SourceNotFound)?;
        if source_state
            != (
                expected.kind.clone(),
                expected.canonical_identity.clone(),
                expected.locator.clone(),
                expected_device,
                expected_inode,
                expected.catalog_generation,
                Some(expected.catalog_marker.clone()),
            )
        {
            return Err(StorageError::SourceCatalogStateChanged);
        }
        let link_state = transaction
            .query_row(
                "SELECT bundle_id, adopted_marker
                 FROM source_bundle_links
                 WHERE source_id = ?1",
                [&expected.id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(StorageError::SaveSourceCatalog)?
            .ok_or(StorageError::SourceBundleStateConflict)?;
        if link_state != (bundle.id.clone(), bundle.adopted_marker.clone()) {
            return Err(StorageError::SourceBundleStateConflict);
        }
        let next_generation = expected
            .catalog_generation
            .checked_add(1)
            .ok_or(StorageError::SourceCatalogStateChanged)?;
        transaction
            .execute(
                "DELETE FROM source_catalog_members WHERE source_id = ?1",
                [&expected.id],
            )
            .map_err(StorageError::SaveSourceCatalog)?;
        for (sort_order, (member, (validation_errors, warnings))) in
            members.iter().zip(encoded_members.iter()).enumerate()
        {
            transaction
                .execute(
                    "INSERT INTO source_catalog_members (
                        id, source_id, catalog_generation, relative_path,
                        skill_name, description, content_fingerprint, selectable,
                        validation_errors_json, warnings_json, sort_order
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        member.id,
                        expected.id,
                        next_generation,
                        member.relative_path,
                        member.skill_name,
                        member.description,
                        member.content_fingerprint,
                        member.selectable,
                        validation_errors,
                        warnings,
                        sort_order as i64,
                    ],
                )
                .map_err(StorageError::SaveSourceCatalog)?;
        }
        let source_changed = transaction
            .execute(
                "UPDATE sources
                 SET catalog_status = 'fresh', catalog_generation = ?2,
                     catalog_marker = ?3, catalog_fetched_at = ?4,
                     last_reload_at = ?4, last_reload_error = NULL, updated_at = ?4
                 WHERE id = ?1 AND kind = 'editable_local'",
                params![expected.id, next_generation, marker, checked_at],
            )
            .map_err(StorageError::SaveSourceCatalog)?;
        if source_changed != 1 {
            return Err(StorageError::SourceCatalogStateChanged);
        }
        let status = if bundle.adopted_marker.as_deref() == Some(marker) {
            BundleUpdateStatus::UpToDate
        } else {
            BundleUpdateStatus::Available
        };
        let status_value = status
            .as_stored_str()
            .ok_or(StorageError::InvalidSourceDefinition)?;
        let link_changed = transaction
            .execute(
                "UPDATE source_bundle_links
                 SET update_check_status = ?3,
                     update_checked_marker = ?4,
                     update_checked_at = ?5,
                     update_check_error = NULL
                 WHERE source_id = ?1 AND bundle_id = ?2",
                params![expected.id, bundle.id, status_value, marker, checked_at],
            )
            .map_err(StorageError::SaveSourceCatalog)?;
        if link_changed != 1 {
            return Err(StorageError::SourceBundleStateConflict);
        }
        transaction
            .commit()
            .map_err(StorageError::SaveSourceCatalog)?;
        Ok(status)
    }

    /// 路径不可访问或 inode 不符时只记录可重试状态，保留 Catalog、adopted 与当前内容。
    pub(crate) fn save_editable_local_check_unavailable(
        &mut self,
        expected: &StoredSourceInstallSource,
        checked_at: i64,
        error: &str,
    ) -> Result<(), StorageError> {
        let bundle = expected
            .bundle
            .as_ref()
            .ok_or(StorageError::SourceBundleStateConflict)?;
        let expected_device = expected
            .filesystem_device
            .map(filesystem_identity_to_sql)
            .transpose()?;
        let expected_inode = expected
            .filesystem_inode
            .map(filesystem_identity_to_sql)
            .transpose()?;
        if expected.kind != SourceKind::EditableLocal.as_str()
            || expected_device.is_none()
            || expected_inode.is_none()
            || checked_at < 0
            || error.is_empty()
        {
            return Err(StorageError::InvalidSourceDefinition);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSource)?;
        if bundle_or_source_write_is_blocked(&transaction, Some(&bundle.id), Some(&expected.id))? {
            return Err(StorageError::ManagedObjectBlocked);
        }
        let stable_source = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM sources
                    WHERE id = ?1
                      AND kind = 'editable_local'
                      AND canonical_identity = ?2
                      AND locator = ?3
                      AND filesystem_device = ?4
                      AND filesystem_inode = ?5
                 )",
                params![
                    expected.id,
                    expected.canonical_identity,
                    expected.locator,
                    expected_device,
                    expected_inode
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StorageError::SaveSource)?;
        if !stable_source {
            return Err(StorageError::SourceCatalogStateChanged);
        }
        let changed = transaction
            .execute(
                "UPDATE source_bundle_links
                 SET update_check_status = 'source_unavailable',
                     update_checked_at = ?3,
                     update_check_error = ?4
                 WHERE source_id = ?1 AND bundle_id = ?2 AND adopted_marker IS ?5",
                params![
                    expected.id,
                    bundle.id,
                    checked_at,
                    error,
                    bundle.adopted_marker
                ],
            )
            .map_err(StorageError::SaveSource)?;
        if changed != 1 {
            return Err(StorageError::SourceBundleStateConflict);
        }
        transaction.commit().map_err(StorageError::SaveSource)
    }

    pub fn read_source_install_source(
        &mut self,
        source_id: &str,
    ) -> Result<StoredSourceInstallSource, StorageError> {
        // Source、Catalog、Bundle 与成员关系必须来自同一个读快照。
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(StorageError::ReadSources)?;
        let source = read_source_install_source_from(&transaction, source_id)?;
        transaction.commit().map_err(StorageError::ReadSources)?;
        Ok(source)
    }

    /// Bundle Update 从一份读快照取得唯一关联 Source 及完整本地成员状态。
    pub(crate) fn read_bundle_update_install_source(
        &mut self,
        bundle_id: &str,
    ) -> Result<StoredSourceInstallSource, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(StorageError::ReadSources)?;
        let source_id = transaction
            .query_row(
                "SELECT source_id FROM source_bundle_links WHERE bundle_id = ?1",
                [bundle_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StorageError::ReadSources)?
            .ok_or(StorageError::SourceNotFound)?;
        let source = read_source_install_source_from(&transaction, &source_id)?;
        if source.bundle.as_ref().map(|bundle| bundle.id.as_str()) != Some(bundle_id) {
            return Err(StorageError::SourceBundleStateConflict);
        }
        transaction.commit().map_err(StorageError::ReadSources)?;
        Ok(source)
    }

    pub(crate) fn read_eligible_bundle_updates(
        &self,
    ) -> Result<Vec<StoredBundleUpdateEligibility>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT source.id, link.bundle_id, bundle.display_name,
                        CASE
                            WHEN source.kind = 'github'
                             AND link.adopted_marker IS NULL
                             AND link.update_check_status = 'not_checked'
                            THEN source.catalog_marker
                            ELSE link.update_checked_marker
                        END
                 FROM sources AS source
                 JOIN source_bundle_links AS link ON link.source_id = source.id
                 JOIN bundles AS bundle ON bundle.id = link.bundle_id
                 WHERE (
                       source.kind = 'github'
                       AND (
                           (
                               link.adopted_marker IS NULL
                               AND link.update_check_status = 'not_checked'
                               AND source.catalog_marker IS NOT NULL
                           )
                           OR (
                               link.update_check_status = 'available'
                               AND link.update_checked_marker IS NOT NULL
                               AND link.update_checked_at IS NOT NULL
                           )
                       )
                   )
                   OR (
                       source.kind = 'editable_local'
                       AND link.update_check_status = 'available'
                       AND link.update_checked_marker IS NOT NULL
                       AND link.update_checked_at IS NOT NULL
                       AND link.update_checked_marker = source.catalog_marker
                   )
                 ORDER BY source.sort_order, source.id",
            )
            .map_err(StorageError::ReadBundleUpdateBatch)?;
        let rows = statement
            .query_map([], |row| {
                Ok(StoredBundleUpdateEligibility {
                    source_id: row.get(0)?,
                    bundle_id: row.get(1)?,
                    bundle_display_name: row.get(2)?,
                    target_marker: row.get(3)?,
                })
            })
            .map_err(StorageError::ReadBundleUpdateBatch)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadBundleUpdateBatch)
    }

    pub(crate) fn read_source_association_bundle(
        &mut self,
        bundle_id: &str,
    ) -> Result<StoredSourceAssociationBundle, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(StorageError::ReadSources)?;
        let bundle = read_source_association_bundle_from(&transaction, &self.data_root, bundle_id)?;
        transaction.commit().map_err(StorageError::ReadSources)?;
        Ok(bundle)
    }

    /// 同一确定性安装组只能补充一个既有 Bundle；重复历史状态必须阻止继续拆分。
    pub(crate) fn read_takeover_bundle_for_group(
        &self,
        group_id: &str,
    ) -> Result<Option<StoredTakeoverBundleSnapshot>, StorageError> {
        let bundle_ids = read_managed_entries_from(&self.connection, &self.data_root)?
            .into_iter()
            .filter(|entry| {
                entry
                    .installation_chain
                    .as_ref()
                    .and_then(takeover_group_evidence)
                    .is_some_and(|evidence| evidence.id == group_id)
            })
            .filter_map(|entry| entry.bundle_id)
            .collect::<BTreeSet<_>>();
        match bundle_ids.len() {
            0 => Ok(None),
            1 => {
                let bundle_id = bundle_ids
                    .first()
                    .ok_or_else(|| StorageError::TakeoverStateConflict(group_id.to_owned()))?;
                read_takeover_bundle_snapshot_from(&self.connection, &self.data_root, bundle_id)
                    .map(Some)
            }
            _ => Err(StorageError::TakeoverStateConflict(group_id.to_owned())),
        }
    }

    pub(crate) fn read_takeover_bundle_snapshot(
        &self,
        bundle_id: &str,
    ) -> Result<StoredTakeoverBundleSnapshot, StorageError> {
        read_takeover_bundle_snapshot_from(&self.connection, &self.data_root, bundle_id)
    }

    /// 直接关联只写元数据，并在一个事务中消费 Plan，不能留下半条一对一关系。
    pub(crate) fn finalize_direct_source_association(
        &mut self,
        association: DirectSourceAssociation<'_>,
    ) -> Result<(), StorageError> {
        validate_direct_source_association_input(&association)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSourceAssociationPlan)?;
        let plan = read_source_association_plan_from(&transaction, association.plan_id)?
            .ok_or(StorageError::SourceAssociationPlanNotFound)?;
        ensure_source_association_plan_is_confirmable(&plan, association.now)?;

        let source_state = transaction
            .query_row(
                "SELECT catalog_status, catalog_generation, catalog_marker
                 FROM sources
                 WHERE id = ?1",
                [association.source_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(StorageError::ReadSources)?
            .ok_or(StorageError::SourceNotFound)?;
        if source_state.0 != "fresh"
            || source_state.1 != association.source_catalog_generation
            || source_state.2.as_deref() != Some(association.source_marker)
        {
            return Err(StorageError::SourceCatalogStateChanged);
        }
        let relationship_exists = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM source_bundle_links
                    WHERE source_id = ?1 OR bundle_id = ?2
                 )",
                params![association.source_id, association.bundle_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StorageError::ReadSources)?;
        if relationship_exists {
            return Err(StorageError::SourceBundleStateConflict);
        }
        if bundle_or_source_write_is_blocked(
            &transaction,
            Some(association.bundle_id),
            Some(association.source_id),
        )? {
            return Err(StorageError::ManagedObjectBlocked);
        }

        let bundle = read_source_association_bundle_from(
            &transaction,
            &self.data_root,
            association.bundle_id,
        )?;
        ensure_direct_source_association_snapshot_matches(&association, &bundle)?;
        validate_direct_source_association_mappings(&transaction, &association)?;

        transaction
            .execute(
                "INSERT INTO source_bundle_links (
                    source_id, bundle_id, adopted_marker, linked_at
                 ) VALUES (?1, ?2, NULL, ?3)",
                params![
                    association.source_id,
                    association.bundle_id,
                    association.now
                ],
            )
            .map_err(StorageError::SaveSourceAssociationPlan)?;
        for mapping in association.member_mappings {
            transaction
                .execute(
                    "INSERT INTO source_member_links (
                        source_id, source_relative_path, member_id, linked_at
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        association.source_id,
                        mapping.source_relative_path,
                        mapping.member_id,
                        association.now
                    ],
                )
                .map_err(StorageError::SaveSourceAssociationPlan)?;
        }
        let consumed = transaction
            .execute(
                "UPDATE source_association_plans
                 SET status = 'consumed'
                 WHERE id = ?1 AND status = 'pending'",
                [association.plan_id],
            )
            .map_err(StorageError::SaveSourceAssociationPlan)?;
        if consumed != 1 {
            return Err(StorageError::SourceAssociationPlanConsumed);
        }
        transaction
            .commit()
            .map_err(StorageError::SaveSourceAssociationPlan)
    }

    /// Merge 的领域提交只发生一次；文件系统切换和 Mount 应用必须已经由 Journal 记录完成。
    pub(crate) fn finalize_source_association_merge(
        &mut self,
        merge: FinalSourceAssociationMerge<'_>,
    ) -> Result<(), StorageError> {
        validate_final_source_association_merge(&merge)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        let stored = transaction
            .query_row(
                "SELECT id, plan_id, source_id, target_bundle_id, retiring_bundle_id,
                        content_choices_json, source_mappings_json, journal_path, phase, status
                 FROM source_association_transactions
                 WHERE id = ?1",
                [merge.transaction_id],
                stored_source_association_transaction_from_row,
            )
            .optional()
            .map_err(StorageError::ReadSourceAssociationTransaction)?
            .ok_or_else(|| {
                StorageError::SourceAssociationStateConflict(merge.transaction_id.to_owned())
            })?;
        validate_stored_source_association_transaction(&stored)?;
        if stored.source_id != merge.source_id
            || stored.target_bundle_id != merge.expected_target_bundle.id
            || stored.retiring_bundle_id != merge.expected_retiring_bundle.id
        {
            return Err(StorageError::SourceAssociationStateConflict(
                merge.transaction_id.to_owned(),
            ));
        }
        let final_member_ids = merge
            .final_members
            .iter()
            .map(|member| member.member_id)
            .collect::<BTreeSet<_>>();
        let source_mappings_json =
            canonical_source_association_mappings_json(merge.source_mappings, &final_member_ids)?;
        if stored.source_mappings_json != source_mappings_json {
            return Err(StorageError::SourceAssociationStateConflict(
                merge.transaction_id.to_owned(),
            ));
        }

        if stored.phase == "mounts_applied" && stored.status == "in_progress" {
            ensure_source_association_merge_relationships(
                &transaction,
                merge.source_id,
                &stored.target_bundle_id,
                &stored.retiring_bundle_id,
            )?;
            let current_target = read_source_association_bundle_from(
                &transaction,
                &self.data_root,
                &stored.target_bundle_id,
            )?;
            let current_retiring = read_source_association_bundle_from(
                &transaction,
                &self.data_root,
                &stored.retiring_bundle_id,
            )?;
            if &current_target != merge.expected_target_bundle
                || &current_retiring != merge.expected_retiring_bundle
            {
                return Err(StorageError::SourceBundleStateConflict);
            }
            apply_final_source_association_merge(&transaction, &self.data_root, &merge)?;
            let changed = transaction
                .execute(
                    "UPDATE source_association_transactions
                     SET phase = 'state_committed', status = 'completed', updated_at = ?2
                     WHERE id = ?1 AND phase = 'mounts_applied' AND status = 'in_progress'",
                    params![merge.transaction_id, merge.now],
                )
                .map_err(StorageError::SaveSourceAssociationTransaction)?;
            ensure_one_source_association_row(changed, merge.transaction_id)?;
        } else if stored.phase == "state_committed" && stored.status == "completed" {
            // 重放只核验最终事实，不能静默补回被外部修改的领域行。
            ensure_final_source_association_merge_matches(&transaction, &self.data_root, &merge)?;
        } else {
            return Err(StorageError::SourceAssociationStateConflict(
                merge.transaction_id.to_owned(),
            ));
        }
        transaction
            .commit()
            .map_err(StorageError::SaveSourceAssociationTransaction)
    }

    /// Catalog 成功结果整体替换；旧成员与 Source 状态不会在事务中间对外可见。
    pub fn save_source_catalog_success(
        &mut self,
        source_id: &str,
        expected_tracked_ref: &str,
        commit_sha: &str,
        fetched_at: i64,
        members: &[NewSourceCatalogMember<'_>],
    ) -> Result<(), StorageError> {
        let encoded_members = members
            .iter()
            .map(|member| {
                Ok((
                    serde_json::to_string(member.validation_errors)
                        .map_err(StorageError::SerializeSourceCatalogMetadata)?,
                    serde_json::to_string(member.warnings)
                        .map_err(StorageError::SerializeSourceCatalogMetadata)?,
                ))
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSourceCatalog)?;
        if bundle_or_source_write_is_blocked(&transaction, None, Some(source_id))? {
            return Err(StorageError::ManagedObjectBlocked);
        }
        let (current_ref, current_generation) = transaction
            .query_row(
                "SELECT tracked_ref, catalog_generation
                 FROM sources
                 WHERE id = ?1 AND kind = 'github'",
                [source_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(StorageError::SaveSourceCatalog)?
            .ok_or(StorageError::SourceNotFound)?;
        if current_ref != expected_tracked_ref {
            return Err(StorageError::SourceCatalogStateChanged);
        }
        let next_generation = current_generation
            .checked_add(1)
            .ok_or(StorageError::SourceCatalogStateChanged)?;
        transaction
            .execute(
                "DELETE FROM source_catalog_members WHERE source_id = ?1",
                [source_id],
            )
            .map_err(StorageError::SaveSourceCatalog)?;
        for (sort_order, (member, (validation_errors, warnings))) in
            members.iter().zip(encoded_members.iter()).enumerate()
        {
            transaction
                .execute(
                    "INSERT INTO source_catalog_members (
                        id, source_id, catalog_generation, relative_path,
                        skill_name, description, content_fingerprint, selectable,
                        validation_errors_json, warnings_json, sort_order
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        member.id,
                        source_id,
                        next_generation,
                        member.relative_path,
                        member.skill_name,
                        member.description,
                        member.content_fingerprint,
                        member.selectable,
                        validation_errors,
                        warnings,
                        sort_order as i64,
                    ],
                )
                .map_err(StorageError::SaveSourceCatalog)?;
        }
        let changed = transaction
            .execute(
                "UPDATE sources
                 SET catalog_status = 'fresh', catalog_generation = ?2,
                     catalog_marker = ?3, catalog_fetched_at = ?4,
                     last_reload_at = ?4, last_reload_error = NULL, updated_at = ?4
                 WHERE id = ?1 AND tracked_ref = ?5 AND kind = 'github'",
                params![
                    source_id,
                    next_generation,
                    commit_sha,
                    fetched_at,
                    expected_tracked_ref,
                ],
            )
            .map_err(StorageError::SaveSourceCatalog)?;
        if changed != 1 {
            return Err(StorageError::SourceCatalogStateChanged);
        }
        transaction
            .commit()
            .map_err(StorageError::SaveSourceCatalog)
    }

    /// Reload 失败只更新结果状态；最近一次成功目录和 marker 始终保留。
    pub fn save_source_catalog_failure(
        &mut self,
        source_id: &str,
        expected_tracked_ref: &str,
        failed_at: i64,
        error: &str,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSourceCatalog)?;
        if bundle_or_source_write_is_blocked(&transaction, None, Some(source_id))? {
            return Err(StorageError::ManagedObjectBlocked);
        }
        let changed = transaction
            .execute(
                "UPDATE sources
                 SET catalog_status = CASE
                         WHEN catalog_generation > 0 THEN 'stale'
                         ELSE 'unloaded'
                     END,
                     last_reload_at = ?3, last_reload_error = ?4, updated_at = ?3
                 WHERE id = ?1 AND tracked_ref = ?2 AND kind = 'github'",
                params![source_id, expected_tracked_ref, failed_at, error],
            )
            .map_err(StorageError::SaveSourceCatalog)?;
        if changed != 1 {
            return Err(StorageError::SourceCatalogStateChanged);
        }
        transaction
            .commit()
            .map_err(StorageError::SaveSourceCatalog)
    }

    /// 手动来源在内容已验证后整体写入；相同 canonical identity 只刷新现有 Source。
    pub fn save_manual_source_with_catalog(
        &mut self,
        source: NewManualSource<'_>,
    ) -> Result<SavedManualSource, StorageError> {
        validate_new_manual_source(&source)?;
        let filesystem_device = source
            .filesystem_device
            .map(filesystem_identity_to_sql)
            .transpose()?;
        let filesystem_inode = source
            .filesystem_inode
            .map(filesystem_identity_to_sql)
            .transpose()?;
        let encoded_members = source
            .members
            .iter()
            .map(|member| {
                Ok((
                    serde_json::to_string(member.validation_errors)
                        .map_err(StorageError::SerializeSourceCatalogMetadata)?,
                    serde_json::to_string(member.warnings)
                        .map_err(StorageError::SerializeSourceCatalogMetadata)?,
                ))
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSource)?;
        let existing = transaction
            .query_row(
                "SELECT id, kind, catalog_generation, locator
                 FROM sources
                 WHERE canonical_identity = ?1",
                [source.canonical_identity],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(StorageError::SaveSource)?;
        let (source_id, catalog_generation) =
            if let Some((source_id, stored_kind, current_generation, stored_locator)) = existing {
                if stored_kind != source.kind {
                    return Err(StorageError::InvalidSourceDefinition);
                }
                // Editable Local 的 inode 可跨重命名保持不变，但 1.0 尚未提供明确的重新关联流程。
                if stored_kind == "editable_local" && stored_locator != source.locator {
                    return Err(StorageError::EditableLocalPathChanged);
                }
                if bundle_or_source_write_is_blocked(&transaction, None, Some(&source_id))? {
                    return Err(StorageError::ManagedObjectBlocked);
                }
                let next_generation = current_generation
                    .checked_add(1)
                    .ok_or(StorageError::SourceCatalogStateChanged)?;
                let changed = transaction
                    .execute(
                        "UPDATE sources
                         SET display_name = ?2, locator = ?3,
                             filesystem_device = ?4, filesystem_inode = ?5,
                             updated_at = ?6
                         WHERE id = ?1 AND kind = ?7",
                        params![
                            source_id,
                            source.display_name,
                            source.locator,
                            filesystem_device,
                            filesystem_inode,
                            source.saved_at,
                            source.kind,
                        ],
                    )
                    .map_err(StorageError::SaveSource)?;
                if changed != 1 {
                    return Err(StorageError::SourceCatalogStateChanged);
                }
                (source_id, next_generation)
            } else {
                let sort_order = transaction
                    .query_row(
                        "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM sources",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(StorageError::SaveSource)?;
                transaction
                    .execute(
                        "INSERT INTO sources (
                            id, kind, canonical_identity, owner, repository,
                            display_name, locator, tracked_ref, member_path_hint,
                            filesystem_device, filesystem_inode, sort_order,
                            created_at, updated_at
                         ) VALUES (
                            ?1, ?2, ?3, NULL, NULL, ?4, ?5, NULL, NULL,
                            ?6, ?7, ?8, ?9, ?9
                         )",
                        params![
                            source.id,
                            source.kind,
                            source.canonical_identity,
                            source.display_name,
                            source.locator,
                            filesystem_device,
                            filesystem_inode,
                            sort_order,
                            source.saved_at,
                        ],
                    )
                    .map_err(StorageError::SaveSource)?;
                (source.id.to_owned(), 1)
            };

        transaction
            .execute(
                "DELETE FROM source_catalog_members WHERE source_id = ?1",
                [&source_id],
            )
            .map_err(StorageError::SaveSourceCatalog)?;
        for (sort_order, (member, (validation_errors, warnings))) in source
            .members
            .iter()
            .zip(encoded_members.iter())
            .enumerate()
        {
            transaction
                .execute(
                    "INSERT INTO source_catalog_members (
                        id, source_id, catalog_generation, relative_path,
                        skill_name, description, content_fingerprint, selectable,
                        validation_errors_json, warnings_json, sort_order
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        member.id,
                        source_id,
                        catalog_generation,
                        member.relative_path,
                        member.skill_name,
                        member.description,
                        member.content_fingerprint,
                        member.selectable,
                        validation_errors,
                        warnings,
                        sort_order as i64,
                    ],
                )
                .map_err(StorageError::SaveSourceCatalog)?;
        }
        let changed = transaction
            .execute(
                "UPDATE sources
                 SET catalog_status = 'fresh', catalog_generation = ?2,
                     catalog_marker = ?3, catalog_fetched_at = ?4,
                     last_reload_at = ?4, last_reload_error = NULL, updated_at = ?4
                 WHERE id = ?1 AND kind = ?5",
                params![
                    source_id,
                    catalog_generation,
                    source.catalog_marker,
                    source.saved_at,
                    source.kind,
                ],
            )
            .map_err(StorageError::SaveSourceCatalog)?;
        if changed != 1 {
            return Err(StorageError::SourceCatalogStateChanged);
        }
        transaction
            .commit()
            .map_err(StorageError::SaveSourceCatalog)?;
        Ok(SavedManualSource {
            source_id,
            catalog_generation,
        })
    }

    /// 已验证 GitHub 输入要么复用 canonical Source，要么签发一次显式 Ref 变更确认。
    pub fn save_or_prepare_github_source(
        &mut self,
        source: NewGitHubSource<'_>,
        ref_change_plan_id: &str,
        now: i64,
        expires_at: i64,
    ) -> Result<SaveGitHubSourceResult, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSource)?;
        let existing = transaction
            .query_row(
                "SELECT id, display_name, tracked_ref
                 FROM sources WHERE canonical_identity = ?1 AND kind = 'github'",
                [source.canonical_identity],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(StorageError::SaveSource)?;

        let result = if let Some((source_id, source_display_name, current_ref)) = existing {
            if bundle_or_source_write_is_blocked(&transaction, None, Some(&source_id))? {
                return Err(StorageError::ManagedObjectBlocked);
            }
            if current_ref == source.tracked_ref {
                let changed = transaction
                    .execute(
                        "UPDATE sources
                         SET owner = ?2, repository = ?3, display_name = ?4,
                             locator = ?5, member_path_hint = ?6, updated_at = ?7
                         WHERE id = ?1 AND tracked_ref = ?8 AND kind = 'github'",
                        params![
                            source_id,
                            source.owner,
                            source.repository,
                            source.display_name,
                            source.locator,
                            source.member_path_hint,
                            now,
                            current_ref,
                        ],
                    )
                    .map_err(StorageError::SaveSource)?;
                if changed != 1 {
                    return Err(StorageError::SourceRefChangeStateChanged);
                }
                SaveGitHubSourceResult::Saved { source_id }
            } else {
                let plan = SourceRefChangePlan {
                    id: ref_change_plan_id.to_owned(),
                    source_id: source_id.clone(),
                    source_display_name,
                    current_ref: current_ref.clone(),
                    candidate_ref: source.tracked_ref.to_owned(),
                    candidate_commit_sha: source.resolved_commit_sha.to_owned(),
                    member_path_hint: source.member_path_hint.map(str::to_owned),
                    created_at: now,
                    expires_at,
                };
                transaction
                    .execute(
                        "INSERT INTO source_ref_change_plans (
                            id, source_id, current_ref, candidate_ref,
                            candidate_commit_sha, member_path_hint,
                            created_at, expires_at, status
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending')",
                        params![
                            plan.id,
                            plan.source_id,
                            plan.current_ref,
                            plan.candidate_ref,
                            plan.candidate_commit_sha,
                            plan.member_path_hint,
                            plan.created_at,
                            plan.expires_at,
                        ],
                    )
                    .map_err(StorageError::SaveSource)?;
                SaveGitHubSourceResult::RefChangeRequired { plan }
            }
        } else {
            let sort_order = transaction
                .query_row(
                    "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM sources",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(StorageError::SaveSource)?;
            transaction
                .execute(
                    "INSERT INTO sources (
                        id, kind, canonical_identity, owner, repository,
                        display_name, locator, tracked_ref, member_path_hint,
                        sort_order, created_at, updated_at
                     ) VALUES (?1, 'github', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                    params![
                        source.id,
                        source.canonical_identity,
                        source.owner,
                        source.repository,
                        source.display_name,
                        source.locator,
                        source.tracked_ref,
                        source.member_path_hint,
                        sort_order,
                        now,
                    ],
                )
                .map_err(StorageError::SaveSource)?;
            SaveGitHubSourceResult::Saved {
                source_id: source.id.to_owned(),
            }
        };
        transaction.commit().map_err(StorageError::SaveSource)?;
        Ok(result)
    }

    pub fn confirm_source_ref_change(
        &mut self,
        plan_id: &str,
        now: i64,
    ) -> Result<String, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSource)?;
        let plan = transaction
            .query_row(
                "SELECT source_id, current_ref, candidate_ref, candidate_commit_sha,
                        member_path_hint, expires_at, status
                 FROM source_ref_change_plans WHERE id = ?1",
                [plan_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(StorageError::SaveSource)?
            .ok_or(StorageError::SourceRefChangePlanNotFound)?;
        let (
            source_id,
            current_ref,
            candidate_ref,
            candidate_commit_sha,
            member_path_hint,
            expires_at,
            status,
        ) = plan;
        if status != "pending" {
            return Err(StorageError::SourceRefChangePlanConsumed);
        }
        if expires_at <= now {
            return Err(StorageError::SourceRefChangePlanExpired);
        }
        if bundle_or_source_write_is_blocked(&transaction, None, Some(&source_id))? {
            return Err(StorageError::ManagedObjectBlocked);
        }
        let changed = transaction
            .execute(
                "UPDATE sources
                 SET tracked_ref = ?2,
                     member_path_hint = ?3,
                     catalog_status = CASE
                         WHEN catalog_generation > 0 THEN 'stale'
                         ELSE 'unloaded'
                     END,
                     last_reload_error = CASE
                         WHEN catalog_generation > 0 THEN 'Tracked Ref 已切换，请重新加载来源'
                         ELSE NULL
                     END,
                     updated_at = ?4
                 WHERE id = ?1 AND tracked_ref = ?5 AND kind = 'github'",
                params![source_id, candidate_ref, member_path_hint, now, current_ref,],
            )
            .map_err(StorageError::SaveSource)?;
        if changed != 1 {
            return Err(StorageError::SourceRefChangeStateChanged);
        }
        transaction
            .execute(
                "UPDATE source_bundle_links
                 SET update_check_status = CASE
                         WHEN adopted_marker = ?2 THEN 'up_to_date'
                         ELSE 'available'
                     END,
                     update_checked_marker = ?2,
                     update_checked_at = ?3,
                     update_check_error = NULL
                 WHERE source_id = ?1",
                params![source_id, candidate_commit_sha, now],
            )
            .map_err(StorageError::SaveSource)?;
        let consumed = transaction
            .execute(
                "UPDATE source_ref_change_plans
                 SET status = 'consumed'
                 WHERE id = ?1 AND status = 'pending'",
                [plan_id],
            )
            .map_err(StorageError::SaveSource)?;
        if consumed != 1 {
            return Err(StorageError::SourceRefChangeStateChanged);
        }
        transaction.commit().map_err(StorageError::SaveSource)?;
        Ok(source_id)
    }

    pub(crate) fn create_editable_local_relink_plan(
        &mut self,
        new_plan: NewEditableLocalRelinkPlan<'_>,
    ) -> Result<EditableLocalRelinkPlan, StorageError> {
        let source = new_plan.source;
        let expected_device = source
            .filesystem_device
            .map(filesystem_identity_to_sql)
            .transpose()?
            .ok_or(StorageError::InvalidSourceDefinition)?;
        let expected_inode = source
            .filesystem_inode
            .map(filesystem_identity_to_sql)
            .transpose()?
            .ok_or(StorageError::InvalidSourceDefinition)?;
        if source.kind != SourceKind::EditableLocal.as_str()
            || source.catalog_generation <= 0
            || source.catalog_marker.is_empty()
            || new_plan.candidate_path.is_empty()
            || !Path::new(new_plan.candidate_path).is_absolute()
            || new_plan.candidate_path == source.locator
            || new_plan.candidate_display_name.is_empty()
            || new_plan.candidate_marker.is_empty()
            || new_plan.members.is_empty()
            || new_plan.created_at < 0
            || new_plan.expires_at <= new_plan.created_at
        {
            return Err(StorageError::InvalidSourceDefinition);
        }
        let candidate_members_json = serde_json::to_string(new_plan.members)
            .map_err(StorageError::SerializeEditableLocalRelinkMetadata)?;
        let expected_bundle_id = source.bundle.as_ref().map(|bundle| bundle.id.clone());
        let expected_bundle_display_name = source
            .bundle
            .as_ref()
            .map(|bundle| bundle.display_name.clone());

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSource)?;
        if bundle_or_source_write_is_blocked(
            &transaction,
            expected_bundle_id.as_deref(),
            Some(&source.id),
        )? {
            return Err(StorageError::ManagedObjectBlocked);
        }
        let stored_source = transaction
            .query_row(
                "SELECT kind, canonical_identity, display_name, locator,
                        filesystem_device, filesystem_inode,
                        catalog_generation, catalog_marker
                 FROM sources
                 WHERE id = ?1",
                [&source.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(StorageError::SaveSource)?
            .ok_or(StorageError::SourceNotFound)?;
        let expected_source = (
            source.kind.clone(),
            source.canonical_identity.clone(),
            source.display_name.clone(),
            source.locator.clone(),
            Some(expected_device),
            Some(expected_inode),
            source.catalog_generation,
            Some(source.catalog_marker.clone()),
        );
        if stored_source != expected_source {
            return Err(StorageError::EditableLocalRelinkStateChanged);
        }
        let stored_bundle = transaction
            .query_row(
                "SELECT link.bundle_id, bundle.display_name
                 FROM source_bundle_links AS link
                 JOIN bundles AS bundle ON bundle.id = link.bundle_id
                 WHERE link.source_id = ?1",
                [&source.id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StorageError::SaveSource)?;
        if stored_bundle
            != expected_bundle_id
                .clone()
                .zip(expected_bundle_display_name.clone())
        {
            return Err(StorageError::EditableLocalRelinkStateChanged);
        }

        // 桌面端一次只能确认一项 metadata 变更，新计划会明确替代旧的未确认计划。
        transaction
            .execute(
                "UPDATE editable_local_relink_plans
                 SET status = 'consumed'
                 WHERE status = 'pending'",
                [],
            )
            .map_err(StorageError::SaveSource)?;
        transaction
            .execute(
                "INSERT INTO editable_local_relink_plans (
                    id, source_id, expected_canonical_identity,
                    expected_source_display_name, expected_locator,
                    expected_device, expected_inode, expected_catalog_generation,
                    expected_catalog_marker, expected_bundle_id,
                    expected_bundle_display_name, candidate_path,
                    candidate_display_name, candidate_marker,
                    candidate_members_json, created_at, expires_at, status
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                    ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, 'pending'
                 )",
                params![
                    new_plan.id,
                    source.id,
                    source.canonical_identity,
                    source.display_name,
                    source.locator,
                    expected_device,
                    expected_inode,
                    source.catalog_generation,
                    source.catalog_marker,
                    expected_bundle_id,
                    expected_bundle_display_name,
                    new_plan.candidate_path,
                    new_plan.candidate_display_name,
                    new_plan.candidate_marker,
                    candidate_members_json,
                    new_plan.created_at,
                    new_plan.expires_at,
                ],
            )
            .map_err(StorageError::SaveSource)?;
        transaction.commit().map_err(StorageError::SaveSource)?;
        Ok(EditableLocalRelinkPlan {
            id: new_plan.id.to_owned(),
            source_id: source.id.clone(),
            source_display_name: source.display_name.clone(),
            current_path: source.locator.clone(),
            candidate_path: new_plan.candidate_path.to_owned(),
            candidate_display_name: new_plan.candidate_display_name.to_owned(),
            bundle_display_name: source
                .bundle
                .as_ref()
                .map(|bundle| bundle.display_name.clone()),
            members: new_plan.members.to_vec(),
            created_at: new_plan.created_at,
            expires_at: new_plan.expires_at,
        })
    }

    pub(crate) fn read_open_editable_local_relink_plan(
        &self,
        now: i64,
    ) -> Result<Option<EditableLocalRelinkPlan>, StorageError> {
        let stored = self
            .connection
            .query_row(
                EDITABLE_LOCAL_RELINK_PLAN_SELECT,
                params![now],
                stored_editable_local_relink_plan_from_row,
            )
            .optional()
            .map_err(StorageError::ReadSources)?;
        stored
            .map(validate_stored_editable_local_relink_plan)
            .transpose()
            .map(|plan| plan.map(|plan| plan.public))
    }

    pub(crate) fn read_editable_local_relink_plan(
        &self,
        plan_id: &str,
        now: i64,
    ) -> Result<StoredEditableLocalRelinkPlan, StorageError> {
        let stored = self
            .connection
            .query_row(
                EDITABLE_LOCAL_RELINK_PLAN_BY_ID_SELECT,
                [plan_id],
                stored_editable_local_relink_plan_from_row,
            )
            .optional()
            .map_err(StorageError::ReadSources)?
            .ok_or(StorageError::EditableLocalRelinkPlanNotFound)?;
        let stored = validate_stored_editable_local_relink_plan(stored)?;
        if stored.status != "pending" {
            return Err(StorageError::EditableLocalRelinkPlanConsumed);
        }
        if stored.public.expires_at <= now {
            return Err(StorageError::EditableLocalRelinkPlanExpired);
        }
        Ok(stored)
    }

    pub(crate) fn confirm_editable_local_relink_plan(
        &mut self,
        expected: &StoredEditableLocalRelinkPlan,
        observed_candidate_path: &str,
        observed_candidate_display_name: &str,
        observed_candidate_marker: &str,
        now: i64,
    ) -> Result<String, StorageError> {
        if observed_candidate_path != expected.public.candidate_path
            || observed_candidate_display_name != expected.public.candidate_display_name
            || observed_candidate_marker != expected.candidate_marker
        {
            return Err(StorageError::EditableLocalRelinkStateChanged);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSource)?;
        let stored = transaction
            .query_row(
                EDITABLE_LOCAL_RELINK_PLAN_BY_ID_SELECT,
                [&expected.public.id],
                stored_editable_local_relink_plan_from_row,
            )
            .optional()
            .map_err(StorageError::SaveSource)?
            .ok_or(StorageError::EditableLocalRelinkPlanNotFound)?;
        let stored = validate_stored_editable_local_relink_plan(stored)?;
        if &stored != expected {
            return Err(StorageError::EditableLocalRelinkStateChanged);
        }
        if stored.status != "pending" {
            return Err(StorageError::EditableLocalRelinkPlanConsumed);
        }
        if stored.public.expires_at <= now {
            return Err(StorageError::EditableLocalRelinkPlanExpired);
        }
        if bundle_or_source_write_is_blocked(
            &transaction,
            stored.expected_bundle_id.as_deref(),
            Some(&stored.public.source_id),
        )? {
            return Err(StorageError::ManagedObjectBlocked);
        }
        let expected_device = filesystem_identity_to_sql(stored.expected_device)?;
        let expected_inode = filesystem_identity_to_sql(stored.expected_inode)?;
        let changed = transaction
            .execute(
                "UPDATE sources
                 SET display_name = ?2, locator = ?3, updated_at = ?4
                 WHERE id = ?1
                   AND kind = 'editable_local'
                   AND canonical_identity = ?5
                   AND display_name = ?6
                   AND locator = ?7
                   AND filesystem_device = ?8
                   AND filesystem_inode = ?9
                   AND catalog_generation = ?10
                   AND catalog_marker = ?11",
                params![
                    stored.public.source_id,
                    observed_candidate_display_name,
                    observed_candidate_path,
                    now,
                    stored.expected_canonical_identity,
                    stored.public.source_display_name,
                    stored.public.current_path,
                    expected_device,
                    expected_inode,
                    stored.expected_catalog_generation,
                    stored.expected_catalog_marker,
                ],
            )
            .map_err(StorageError::SaveSource)?;
        if changed != 1 {
            return Err(StorageError::EditableLocalRelinkStateChanged);
        }
        match &stored.expected_bundle_id {
            Some(bundle_id) => {
                let changed = transaction
                    .execute(
                        "UPDATE source_bundle_links
                         SET update_check_status = 'not_checked',
                             update_checked_marker = NULL,
                             update_checked_at = NULL,
                             update_check_error = NULL
                         WHERE source_id = ?1 AND bundle_id = ?2",
                        params![stored.public.source_id, bundle_id],
                    )
                    .map_err(StorageError::SaveSource)?;
                if changed != 1 {
                    return Err(StorageError::EditableLocalRelinkStateChanged);
                }
            }
            None => {
                let has_bundle = transaction
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM source_bundle_links WHERE source_id = ?1
                         )",
                        [&stored.public.source_id],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(StorageError::SaveSource)?;
                if has_bundle {
                    return Err(StorageError::EditableLocalRelinkStateChanged);
                }
            }
        }
        let consumed = transaction
            .execute(
                "UPDATE editable_local_relink_plans
                 SET status = 'consumed'
                 WHERE id = ?1 AND status = 'pending'",
                [&stored.public.id],
            )
            .map_err(StorageError::SaveSource)?;
        if consumed != 1 {
            return Err(StorageError::EditableLocalRelinkStateChanged);
        }
        transaction.commit().map_err(StorageError::SaveSource)?;
        Ok(stored.public.source_id)
    }

    pub(crate) fn discard_editable_local_relink_plan(
        &mut self,
        plan_id: &str,
    ) -> Result<String, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveSource)?;
        let state = transaction
            .query_row(
                "SELECT source_id, status
                 FROM editable_local_relink_plans
                 WHERE id = ?1",
                [plan_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StorageError::SaveSource)?
            .ok_or(StorageError::EditableLocalRelinkPlanNotFound)?;
        if state.1 != "pending" {
            return Err(StorageError::EditableLocalRelinkPlanConsumed);
        }
        let changed = transaction
            .execute(
                "UPDATE editable_local_relink_plans
                 SET status = 'consumed'
                 WHERE id = ?1 AND status = 'pending'",
                [plan_id],
            )
            .map_err(StorageError::SaveSource)?;
        if changed != 1 {
            return Err(StorageError::EditableLocalRelinkStateChanged);
        }
        transaction.commit().map_err(StorageError::SaveSource)?;
        Ok(state.0)
    }

    pub fn save_initial_scan(
        &mut self,
        scan_completed_at: i64,
        entries: &[InventoryObservation],
        supported_apps: &[SupportedAppSummary],
        scan_issues: &[ScanIssue],
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveInitialScan)?;
        // 首次扫描与刷新采用同一部分成功语义，扫描问题和完成标记必须原子保存。
        replace_inventory_rows(&transaction, entries, supported_apps, scan_issues)
            .map_err(StorageError::SaveInitialScan)?;
        transaction
            .execute(
                "UPDATE app_state
                 SET initial_scan_completed_at = ?1,
                     last_local_refresh_at = NULL,
                     last_local_refresh_added = 0,
                     last_local_refresh_changed = 0,
                     last_local_refresh_removed = 0
                 WHERE singleton = 1",
                [scan_completed_at],
            )
            .map_err(StorageError::SaveInitialScan)?;
        transaction.commit().map_err(StorageError::SaveInitialScan)
    }

    pub fn save_local_refresh(
        &mut self,
        completed_at: i64,
        scanned_entries: &[InventoryObservation],
        scanned_apps: &[SupportedAppSummary],
        successful_roots: &[ScanRootIdentity],
        scan_issues: &[ScanIssue],
        mount_health: &[(String, MountHealth)],
    ) -> Result<SavedLocalRefresh, StorageError> {
        // 读取旧快照和写入新快照必须处于同一个写事务，不能让并发命令覆盖较新结果。
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveLocalRefresh)?;
        update_mount_health_rows(&transaction, mount_health, completed_at)?;
        let previous_entries = read_inventory_entries_from(&transaction)?;
        let previous_apps = read_supported_apps_from(&transaction)?;
        let previous_issues = read_scan_issues_from(&transaction)?;
        let mount_targets = read_mount_target_paths_from(&transaction)?;
        let scanned_entries = scanned_entries
            .iter()
            .filter(|entry| !mount_targets.contains(&entry.skill_root))
            .cloned()
            .collect::<Vec<_>>();
        let entries = reconcile_entries(
            &previous_entries,
            &scanned_entries,
            successful_roots,
            scan_issues,
        );
        let scan_issues = reconcile_scan_issues(&previous_issues, successful_roots, scan_issues);
        let supported_apps = reconcile_supported_apps(&previous_apps, scanned_apps);
        let summary = summarize_changes(completed_at, &previous_entries, &entries);
        replace_inventory_rows(&transaction, &entries, &supported_apps, &scan_issues)
            .map_err(StorageError::SaveLocalRefresh)?;
        transaction
            .execute(
                "UPDATE app_state
                 SET last_local_refresh_at = ?1,
                     last_local_refresh_added = ?2,
                     last_local_refresh_changed = ?3,
                     last_local_refresh_removed = ?4
                 WHERE singleton = 1",
                params![
                    summary.completed_at,
                    summary.added as i64,
                    summary.changed as i64,
                    summary.removed as i64
                ],
            )
            .map_err(StorageError::SaveLocalRefresh)?;
        transaction
            .commit()
            .map_err(StorageError::SaveLocalRefresh)?;

        Ok(SavedLocalRefresh {
            entries: self.with_managed_entries(entries)?,
            supported_apps,
            summary,
            recovery_issues: self.read_recovery_issues()?,
            projects: self.read_projects()?,
            mounts: self.read_mount_summaries()?,
        })
    }

    /// Project 记录与首次项目扫描在同一事务提交，失败时不能留下“已登记但未扫描”。
    pub(crate) fn register_project_with_scan(
        &mut self,
        project: &StoredProject,
        scanned_entries: &[InventoryObservation],
        successful_roots: &[ScanRootIdentity],
        scan_issues: &[ScanIssue],
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveProjectScan)?;
        persist_project_from(&transaction, project)?;
        let previous_entries = read_inventory_entries_from(&transaction)?;
        let supported_apps = read_supported_apps_from(&transaction)?;
        let previous_issues = read_scan_issues_from(&transaction)?;
        let mount_targets = read_mount_target_paths_from(&transaction)?;
        let scanned_entries = scanned_entries
            .iter()
            .filter(|entry| !mount_targets.contains(&entry.skill_root))
            .cloned()
            .collect::<Vec<_>>();
        let entries = reconcile_entries(
            &previous_entries,
            &scanned_entries,
            successful_roots,
            scan_issues,
        );
        let scan_issues = reconcile_scan_issues(&previous_issues, successful_roots, scan_issues);
        replace_inventory_rows(&transaction, &entries, &supported_apps, &scan_issues)
            .map_err(StorageError::SaveProjectScan)?;
        transaction.commit().map_err(StorageError::SaveProjectScan)
    }

    pub(crate) fn prepare_project(
        &self,
        project: NewProject<'_>,
    ) -> Result<StoredProject, StorageError> {
        validate_new_project(&project)?;
        let root_device = filesystem_identity_to_sql(project.root_device)?;
        let root_inode = filesystem_identity_to_sql(project.root_inode)?;
        let existing = read_project_by_identity_from(
            &self.connection,
            project.id,
            project.root_path,
            root_device,
            root_inode,
        )?;
        if let Some(existing) = existing {
            if existing.root_path != project.root_path
                || existing.root_device != project.root_device
                || existing.root_inode != project.root_inode
            {
                return Err(StorageError::ProjectIdentityConflict);
            }
            return Ok(existing);
        }
        Ok(StoredProject {
            id: project.id.to_owned(),
            display_name: project.display_name.to_owned(),
            root_path: project.root_path.to_owned(),
            root_device: project.root_device,
            root_inode: project.root_inode,
            created_at: project.created_at,
        })
    }

    #[cfg(test)]
    pub(crate) fn register_project(
        &mut self,
        project: NewProject<'_>,
    ) -> Result<StoredProject, StorageError> {
        let project = self.prepare_project(project)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveProject)?;
        persist_project_from(&transaction, &project)?;
        transaction.commit().map_err(StorageError::SaveProject)?;
        Ok(project)
    }

    pub(crate) fn read_project(&self, project_id: &str) -> Result<StoredProject, StorageError> {
        read_project_from(&self.connection, project_id)?
            .ok_or_else(|| StorageError::ProjectNotFound(project_id.to_owned()))
    }

    pub(crate) fn read_projects(&self) -> Result<Vec<ProjectSummary>, StorageError> {
        Ok(self
            .read_stored_projects()?
            .into_iter()
            .map(|project| ProjectSummary {
                id: project.id,
                display_name: project.display_name,
                root_path: project.root_path,
            })
            .collect())
    }

    pub(crate) fn read_stored_projects(&self) -> Result<Vec<StoredProject>, StorageError> {
        let mut statement = self
            .connection
            .prepare("SELECT id, display_name, root_path, root_device, root_inode, created_at FROM projects ORDER BY display_name, id")
            .map_err(StorageError::ReadProject)?;
        let rows = statement
            .query_map([], stored_project_from_row)
            .map_err(StorageError::ReadProject)?;
        let projects = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadProject)?;
        for project in &projects {
            if !is_normalized_absolute_path(&project.root_path) {
                return Err(StorageError::ProjectIdentityConflict);
            }
        }
        Ok(projects)
    }

    pub(crate) fn read_managed_member(
        &self,
        member_id: &str,
    ) -> Result<StoredManagedMember, StorageError> {
        read_managed_member_from(&self.connection, &self.data_root, member_id)?
            .ok_or_else(|| StorageError::ManagedMemberNotFound(member_id.to_owned()))
    }

    pub(crate) fn read_mount(&self, mount_id: &str) -> Result<StoredMount, StorageError> {
        read_mount_from(&self.connection, &self.data_root, mount_id)?
            .ok_or_else(|| StorageError::MountNotFound(mount_id.to_owned()))
    }

    pub(crate) fn read_mounts(&self) -> Result<Vec<StoredMount>, StorageError> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM mounts ORDER BY target_path, id")
            .map_err(StorageError::ReadMountTransaction)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(StorageError::ReadMountTransaction)?;
        let ids = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadMountTransaction)?;
        ids.into_iter().map(|id| self.read_mount(&id)).collect()
    }

    pub(crate) fn mount_target_paths(&self) -> Result<BTreeSet<PathBuf>, StorageError> {
        read_mount_target_paths_from(&self.connection).map(|paths| {
            paths
                .into_iter()
                .map(PathBuf::from)
                .collect::<BTreeSet<_>>()
        })
    }

    pub(crate) fn is_batch_mount_object_blocked(
        &self,
        member_id: &str,
        target_path: &str,
        project_id: Option<&str>,
    ) -> Result<bool, StorageError> {
        mount_object_is_blocked(&self.connection, member_id, target_path, project_id)
    }

    pub(crate) fn source_install_object_is_blocked(
        &self,
        source_id: &str,
        bundle_id: Option<&str>,
    ) -> Result<bool, StorageError> {
        bundle_or_source_write_is_blocked(&self.connection, bundle_id, Some(source_id))
    }

    pub(crate) fn removal_object_is_blocked(
        &self,
        kind: &str,
        target_id: &str,
    ) -> Result<bool, StorageError> {
        let already_blocked = self
            .connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM removal_transactions AS removal_tx
                    WHERE removal_tx.status = 'blocked'
                      AND (
                        (removal_tx.kind = ?1 AND removal_tx.target_id = ?2)
                        OR (
                            ?1 = 'bundle'
                            AND removal_tx.kind = 'bundle_mounts'
                            AND removal_tx.target_id = ?2
                        )
                        OR (
                            ?1 = 'bundle'
                            AND removal_tx.kind = 'project'
                            AND EXISTS(
                                SELECT 1
                                FROM mounts AS mount
                                JOIN skill_members AS member ON member.id = mount.member_id
                                WHERE mount.project_id = removal_tx.target_id
                                  AND member.bundle_id = ?2
                            )
                        )
                        OR (
                            ?1 = 'project'
                            AND removal_tx.kind IN ('bundle', 'bundle_mounts')
                            AND EXISTS(
                                SELECT 1
                                FROM mounts AS mount
                                JOIN skill_members AS member ON member.id = mount.member_id
                                WHERE mount.project_id = ?2
                                  AND member.bundle_id = removal_tx.target_id
                            )
                        )
                      )
                 )",
                params![kind, target_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StorageError::ReadRecoveryIssues)?;
        if already_blocked {
            return Ok(true);
        }
        match kind {
            "bundle" => bundle_or_source_write_is_blocked(&self.connection, Some(target_id), None),
            "source" => bundle_or_source_write_is_blocked(&self.connection, None, Some(target_id)),
            "project" => self
                .connection
                .query_row(
                    "SELECT
                        EXISTS(
                            SELECT 1
                            FROM mount_transactions AS mount_tx
                            JOIN mount_plans AS mount_plan ON mount_plan.id = mount_tx.plan_id
                            WHERE mount_tx.status = 'blocked'
                              AND mount_plan.project_id = ?1
                        )
                        OR EXISTS(
                            SELECT 1
                            FROM batch_mount_transactions AS batch_tx
                            JOIN batch_mount_plan_items AS item
                              ON item.plan_id = batch_tx.plan_id
                            WHERE batch_tx.status = 'blocked'
                              AND item.project_id = ?1
                        )",
                    [target_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(StorageError::ReadRecoveryIssues),
            _ => Err(StorageError::RemovalStateConflict),
        }
    }

    pub(crate) fn save_mount_plan(&mut self, plan: NewMountPlan<'_>) -> Result<(), StorageError> {
        validate_new_mount_plan_shape(&plan)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveMountPlan)?;
        validate_mount_plan_state(
            &transaction,
            &self.data_root,
            plan.operation,
            plan.purpose,
            plan.mount_id,
            plan.member_id,
            plan.app_id,
            plan.scope,
            plan.project_id,
            plan.target_path,
            plan.expected_target,
            plan.member_fingerprint,
        )?;
        let project_snapshot = plan
            .project_id
            .map(|project_id| {
                read_project_from(&transaction, project_id)?
                    .ok_or_else(|| StorageError::ProjectNotFound(project_id.to_owned()))
            })
            .transpose()?;
        transaction
            .execute(
                "INSERT INTO mount_plans (
                    id, operation, purpose, mount_id, member_id, app_id, scope, project_id,
                    project_root_path, project_root_device, project_root_inode, target_path,
                    expected_target, member_fingerprint, target_observation, created_at,
                    expires_at, status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, 'pending')",
                params![
                    plan.id,
                    plan.operation.as_str(),
                    plan.purpose.as_str(),
                    plan.mount_id,
                    plan.member_id,
                    plan.app_id.as_str(),
                    plan.scope.as_str(),
                    plan.project_id,
                    project_snapshot.as_ref().map(|project| project.root_path.as_str()),
                    project_snapshot
                        .as_ref()
                        .map(|project| filesystem_identity_to_sql(project.root_device))
                        .transpose()?,
                    project_snapshot
                        .as_ref()
                        .map(|project| filesystem_identity_to_sql(project.root_inode))
                        .transpose()?,
                    plan.target_path,
                    plan.expected_target,
                    plan.member_fingerprint,
                    plan.target_observation,
                    plan.created_at,
                    plan.expires_at,
                ],
            )
            .map_err(StorageError::SaveMountPlan)?;
        transaction.commit().map_err(StorageError::SaveMountPlan)
    }

    pub(crate) fn read_mount_plan(&self, plan_id: &str) -> Result<StoredMountPlan, StorageError> {
        read_mount_plan_from(&self.connection, &self.data_root, plan_id)?
            .ok_or(StorageError::MountPlanNotFound)
    }

    pub(crate) fn begin_mount_transaction(
        &mut self,
        plan_id: &str,
        transaction_id: &str,
        journal_path: &str,
        now: i64,
    ) -> Result<StoredMountPlan, StorageError> {
        if !is_single_path_component(transaction_id) || !is_normalized_relative_path(journal_path) {
            return Err(StorageError::InvalidMountPlan);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveMountTransaction)?;
        let mut plan = read_mount_plan_from(&transaction, &self.data_root, plan_id)?
            .ok_or(StorageError::MountPlanNotFound)?;
        if plan.status != "pending" {
            return Err(StorageError::MountPlanConsumed);
        }
        if plan.expires_at <= now {
            return Err(StorageError::MountPlanExpired);
        }
        validate_stored_mount_plan_state(&transaction, &self.data_root, &plan)?;
        transaction
            .execute(
                "INSERT INTO mount_transactions (
                    id, plan_id, mount_id, journal_path, phase, status, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, 'journal_pending', 'in_progress', ?5, ?5)",
                params![transaction_id, plan.id, plan.mount_id, journal_path, now],
            )
            .map_err(map_mount_transaction_insert_error)?;
        let consumed = transaction
            .execute(
                "UPDATE mount_plans SET status = 'consumed' WHERE id = ?1 AND status = 'pending'",
                [&plan.id],
            )
            .map_err(StorageError::SaveMountTransaction)?;
        ensure_one_mount_row(consumed, transaction_id)?;
        plan.status = "consumed".to_owned();
        transaction
            .commit()
            .map_err(StorageError::SaveMountTransaction)?;
        Ok(plan)
    }

    pub(crate) fn update_mount_transaction_phase(
        &mut self,
        transaction_id: &str,
        phase: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let next = MountTransactionPhase::from_str(phase)
            .ok_or_else(|| StorageError::InvalidMountPhase(phase.to_owned()))?;
        let Some(previous) = next.previous() else {
            return Err(StorageError::MountStateConflict(transaction_id.to_owned()));
        };
        let changed = self
            .connection
            .execute(
                "UPDATE mount_transactions
                 SET phase = ?2, updated_at = ?4
                 WHERE id = ?1 AND status = 'in_progress' AND phase IN (?2, ?3)",
                params![transaction_id, next.as_str(), previous.as_str(), now],
            )
            .map_err(StorageError::SaveMountTransaction)?;
        ensure_one_mount_row(changed, transaction_id)
    }

    pub(crate) fn finalize_mount_create(
        &mut self,
        transaction_id: &str,
        plan: &StoredMountPlan,
        now: i64,
    ) -> Result<(), StorageError> {
        if plan.operation != MountOperation::Create
            || !matches!(
                plan.purpose,
                MountPlanPurpose::Create | MountPlanPurpose::Repair
            )
        {
            return Err(StorageError::InvalidMountPlan);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveMountTransaction)?;
        let already_completed =
            validate_mount_finalization(&transaction, &self.data_root, transaction_id, plan)?;
        if already_completed {
            let stored = read_mount_from(&transaction, &self.data_root, &plan.mount_id)?
                .ok_or_else(|| StorageError::MountStateConflict(transaction_id.to_owned()))?;
            ensure_mount_matches_plan(&stored, plan)?;
            return transaction
                .commit()
                .map_err(StorageError::SaveMountTransaction);
        }
        if plan.purpose == MountPlanPurpose::Repair {
            // 修复只能更新生成 Plan 时已存在的关系，不能把已正式移除的 Mount 复活。
            let changed = transaction
                .execute(
                    "UPDATE mounts SET health = ?2, updated_at = ?3 WHERE id = ?1",
                    params![plan.mount_id, MountHealth::Healthy.as_str(), now],
                )
                .map_err(StorageError::SaveMountTransaction)?;
            ensure_one_mount_row(changed, &plan.mount_id)?;
        } else {
            transaction
                .execute(
                    "INSERT INTO mounts (
                        id, member_id, app_id, scope, project_id, target_path,
                        expected_target, health, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
                     ON CONFLICT(id) DO UPDATE SET
                        health = excluded.health,
                        updated_at = excluded.updated_at",
                    params![
                        plan.mount_id,
                        plan.member_id,
                        plan.app_id.as_str(),
                        plan.scope.as_str(),
                        plan.project_id,
                        plan.target_path,
                        plan.expected_target,
                        MountHealth::Healthy.as_str(),
                        now,
                    ],
                )
                .map_err(StorageError::SaveMountTransaction)?;
        }
        let stored = read_mount_from(&transaction, &self.data_root, &plan.mount_id)?
            .ok_or_else(|| StorageError::MountStateConflict(transaction_id.to_owned()))?;
        ensure_mount_matches_plan(&stored, plan)?;
        // 已登记 Mount 已有明确管理解释，不能继续作为待接管观察重复展示。
        transaction
            .execute(
                "DELETE FROM inventory_observations WHERE skill_root = ?1",
                [&plan.target_path],
            )
            .map_err(StorageError::SaveMountTransaction)?;
        complete_mount_transaction(&transaction, transaction_id, plan, now)?;
        transaction
            .commit()
            .map_err(StorageError::SaveMountTransaction)
    }

    pub(crate) fn finalize_mount_remove(
        &mut self,
        transaction_id: &str,
        plan: &StoredMountPlan,
        now: i64,
    ) -> Result<(), StorageError> {
        if plan.operation != MountOperation::Remove || plan.purpose != MountPlanPurpose::Remove {
            return Err(StorageError::InvalidMountPlan);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveMountTransaction)?;
        let already_completed =
            validate_mount_finalization(&transaction, &self.data_root, transaction_id, plan)?;
        if already_completed {
            if read_mount_from(&transaction, &self.data_root, &plan.mount_id)?.is_some() {
                return Err(StorageError::MountStateConflict(transaction_id.to_owned()));
            }
            return transaction
                .commit()
                .map_err(StorageError::SaveMountTransaction);
        }
        let stored = read_mount_from(&transaction, &self.data_root, &plan.mount_id)?
            .ok_or_else(|| StorageError::MountStateConflict(transaction_id.to_owned()))?;
        ensure_mount_matches_plan(&stored, plan)?;
        let deleted = transaction
            .execute("DELETE FROM mounts WHERE id = ?1", [&plan.mount_id])
            .map_err(StorageError::SaveMountTransaction)?;
        ensure_one_mount_row(deleted, transaction_id)?;
        complete_mount_transaction(&transaction, transaction_id, plan, now)?;
        transaction
            .commit()
            .map_err(StorageError::SaveMountTransaction)
    }

    pub(crate) fn abort_mount_transaction(
        &mut self,
        transaction_id: &str,
        error_message: Option<&str>,
        now: i64,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection
            .execute(
                "UPDATE mount_transactions
                 SET status = 'aborted', error_message = ?2, updated_at = ?3
                 WHERE id = ?1 AND status = 'in_progress'
                   AND phase IN ('journal_pending', 'journal_ready')",
                params![transaction_id, error_message, now],
            )
            .map_err(StorageError::SaveMountTransaction)?;
        ensure_one_mount_row(changed, transaction_id)
    }

    pub(crate) fn block_mount_transaction(
        &mut self,
        transaction_id: &str,
        error_message: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection
            .execute(
                "UPDATE mount_transactions
                 SET status = 'blocked', error_message = ?2, updated_at = ?3
                 WHERE id = ?1 AND status IN ('in_progress', 'completed', 'aborted')",
                params![transaction_id, error_message, now],
            )
            .map_err(StorageError::SaveMountTransaction)?;
        ensure_one_mount_row(changed, transaction_id)
    }

    pub(crate) fn forget_terminal_mount_transaction(
        &mut self,
        transaction_id: &str,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveMountTransaction)?;
        let stored = transaction
            .query_row(
                "SELECT plan_id, status FROM mount_transactions WHERE id = ?1",
                [transaction_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StorageError::SaveMountTransaction)?;
        if let Some((plan_id, status)) = stored {
            if !matches!(status.as_str(), "completed" | "aborted") {
                return Err(StorageError::MountStateConflict(transaction_id.to_owned()));
            }
            transaction
                .execute(
                    "DELETE FROM mount_transactions WHERE id = ?1",
                    [transaction_id],
                )
                .map_err(StorageError::SaveMountTransaction)?;
            transaction
                .execute("DELETE FROM mount_plans WHERE id = ?1", [plan_id])
                .map_err(StorageError::SaveMountTransaction)?;
        }
        transaction
            .commit()
            .map_err(StorageError::SaveMountTransaction)
    }

    pub(crate) fn recoverable_mount_transactions(
        &self,
    ) -> Result<Vec<StoredMountTransaction>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT mount_tx.id, mount_tx.plan_id, mount_tx.mount_id,
                        plan.operation, mount_tx.journal_path, mount_tx.phase, mount_tx.status
                 FROM mount_transactions AS mount_tx
                 JOIN mount_plans AS plan ON plan.id = mount_tx.plan_id
                 WHERE mount_tx.status IN ('in_progress', 'completed', 'aborted', 'blocked')
                 ORDER BY mount_tx.created_at, mount_tx.id",
            )
            .map_err(StorageError::ReadMountTransaction)?;
        let rows = statement
            .query_map([], |row| {
                let operation = row.get::<_, String>(3)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    operation,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(StorageError::ReadMountTransaction)?;
        let mut transactions = Vec::new();
        for row in rows {
            let (id, plan_id, mount_id, operation, journal_path, phase, status) =
                row.map_err(StorageError::ReadMountTransaction)?;
            transactions.push(StoredMountTransaction {
                id,
                plan_id,
                mount_id,
                operation: MountOperation::from_str(&operation)
                    .ok_or_else(|| StorageError::UnknownMountOperation(operation.clone()))?,
                journal_path,
                phase,
                status,
            });
        }
        Ok(transactions)
    }

    pub(crate) fn save_batch_mount_plan(
        &mut self,
        plan: NewBatchMountPlan<'_>,
    ) -> Result<(), StorageError> {
        validate_new_batch_mount_plan_shape(&plan)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveBatchMountPlan)?;
        let bundle_exists = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM bundles WHERE id = ?1)",
                [plan.bundle_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StorageError::SaveBatchMountPlan)?;
        if !bundle_exists {
            return Err(StorageError::InvalidBatchMountPlan);
        }
        transaction
            .execute(
                "INSERT INTO batch_mount_plans (
                    id, bundle_id, created_at, expires_at, status
                 ) VALUES (?1, ?2, ?3, ?4, 'pending')",
                params![plan.id, plan.bundle_id, plan.created_at, plan.expires_at],
            )
            .map_err(StorageError::SaveBatchMountPlan)?;
        for (sort_order, item) in plan.items.iter().enumerate() {
            validate_new_batch_mount_item_shape(item)?;
            let member = read_managed_member_from(&transaction, &self.data_root, item.member_id)?
                .ok_or_else(|| {
                StorageError::ManagedMemberNotFound(item.member_id.to_owned())
            })?;
            if member.bundle_id != plan.bundle_id
                || member.content_fingerprint != item.member_fingerprint
                || member.expected_target != item.expected_target
                || Path::new(item.target_path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    != Some(member.skill_name.as_str())
            {
                return Err(StorageError::InvalidBatchMountPlan);
            }
            if item.disposition == BatchMountDisposition::Ready
                && mount_object_is_blocked(
                    &transaction,
                    item.member_id,
                    item.target_path,
                    item.project_id,
                )?
            {
                return Err(StorageError::ManagedObjectBlocked);
            }
            let project_snapshot = item
                .project_id
                .map(|project_id| {
                    read_project_from(&transaction, project_id)?
                        .ok_or_else(|| StorageError::ProjectNotFound(project_id.to_owned()))
                })
                .transpose()?;
            transaction
                .execute(
                    "INSERT INTO batch_mount_plan_items (
                        id, plan_id, bundle_id, mount_id, member_id, app_id, scope, project_id,
                        project_root_path, project_root_device, project_root_inode, target_path,
                        expected_target, member_fingerprint, target_observation, disposition,
                        selectable, default_selected, conflict_reason, target_health, sort_order
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                        ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21
                     )",
                    params![
                        item.id,
                        plan.id,
                        plan.bundle_id,
                        item.mount_id,
                        item.member_id,
                        item.app_id.as_str(),
                        item.scope.as_str(),
                        item.project_id,
                        project_snapshot
                            .as_ref()
                            .map(|project| project.root_path.as_str()),
                        project_snapshot
                            .as_ref()
                            .map(|project| filesystem_identity_to_sql(project.root_device))
                            .transpose()?,
                        project_snapshot
                            .as_ref()
                            .map(|project| filesystem_identity_to_sql(project.root_inode))
                            .transpose()?,
                        item.target_path,
                        item.expected_target,
                        item.member_fingerprint,
                        item.target_observation,
                        item.disposition.as_str(),
                        i64::from(item.selectable),
                        i64::from(item.default_selected),
                        item.conflict_reason,
                        item.target_health.as_str(),
                        sort_order as i64,
                    ],
                )
                .map_err(StorageError::SaveBatchMountPlan)?;
        }
        transaction
            .commit()
            .map_err(StorageError::SaveBatchMountPlan)
    }

    pub(crate) fn read_batch_mount_plan(
        &self,
        plan_id: &str,
    ) -> Result<StoredBatchMountPlan, StorageError> {
        read_batch_mount_plan_from(&self.connection, &self.data_root, plan_id)?
            .ok_or(StorageError::BatchMountPlanNotFound)
    }

    pub(crate) fn begin_batch_mount_transaction(
        &mut self,
        plan_id: &str,
        selected_item_ids: &[String],
        transaction_id: &str,
        journal_path: &str,
        now: i64,
    ) -> Result<StoredBatchMountPlan, StorageError> {
        if !is_single_path_component(transaction_id) || !is_normalized_relative_path(journal_path) {
            return Err(StorageError::InvalidBatchMountPlan);
        }
        let selected = selected_item_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if selected.is_empty() || selected.len() != selected_item_ids.len() {
            return Err(StorageError::InvalidBatchMountSelection);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveBatchMountTransaction)?;
        let mut plan = read_batch_mount_plan_from(&transaction, &self.data_root, plan_id)?
            .ok_or(StorageError::BatchMountPlanNotFound)?;
        if plan.status != "pending" {
            return Err(StorageError::BatchMountPlanConsumed);
        }
        if plan.expires_at <= now {
            return Err(StorageError::BatchMountPlanExpired);
        }
        for item in &mut plan.items {
            item.selected = selected.contains(item.id.as_str());
        }
        let selected_items = selected_batch_mount_items(&plan)?;
        if selected_items.len() != selected.len() {
            return Err(StorageError::InvalidBatchMountSelection);
        }
        for item in selected_items {
            validate_batch_ready_item_state(&transaction, &self.data_root, item)?;
        }
        let inserted = transaction
            .execute(
                "INSERT INTO batch_mount_transactions (
                    id, plan_id, bundle_id, journal_path, phase, status, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, 'journal_pending', 'in_progress', ?5, ?5)",
                params![transaction_id, plan.id, plan.bundle_id, journal_path, now],
            )
            .map_err(map_batch_mount_transaction_insert_error)?;
        ensure_one_batch_mount_row(inserted, transaction_id)?;
        let mut selected_order = 0_i64;
        for item in &plan.items {
            if selected.contains(item.id.as_str()) {
                transaction
                    .execute(
                        "INSERT INTO batch_mount_transaction_items (
                            transaction_id, plan_id, item_id, sort_order
                         ) VALUES (?1, ?2, ?3, ?4)",
                        params![transaction_id, plan.id, item.id, selected_order],
                    )
                    .map_err(StorageError::SaveBatchMountTransaction)?;
                selected_order += 1;
            }
        }
        let consumed = transaction
            .execute(
                "UPDATE batch_mount_plans
                 SET status = 'consumed'
                 WHERE id = ?1 AND status = 'pending'",
                [&plan.id],
            )
            .map_err(StorageError::SaveBatchMountTransaction)?;
        ensure_one_batch_mount_row(consumed, transaction_id)?;
        plan.status = "consumed".to_owned();
        transaction
            .commit()
            .map_err(StorageError::SaveBatchMountTransaction)?;
        Ok(plan)
    }

    pub(crate) fn update_batch_mount_transaction_phase(
        &mut self,
        transaction_id: &str,
        phase: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let next = BatchMountTransactionPhase::from_str(phase)
            .ok_or_else(|| StorageError::InvalidBatchMountPhase(phase.to_owned()))?;
        let Some(previous) = next.previous() else {
            return Err(StorageError::BatchMountStateConflict(
                transaction_id.to_owned(),
            ));
        };
        let changed = self
            .connection
            .execute(
                "UPDATE batch_mount_transactions
                 SET phase = ?2, updated_at = ?4
                 WHERE id = ?1 AND status = 'in_progress' AND phase IN (?2, ?3)",
                params![transaction_id, next.as_str(), previous.as_str(), now],
            )
            .map_err(StorageError::SaveBatchMountTransaction)?;
        ensure_one_batch_mount_row(changed, transaction_id)
    }

    pub(crate) fn begin_batch_mount_rollback(
        &mut self,
        transaction_id: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection
            .execute(
                "UPDATE batch_mount_transactions
                 SET phase = 'rolling_back', updated_at = ?2
                 WHERE id = ?1 AND status = 'in_progress'
                   AND phase IN ('journal_ready', 'applying', 'targets_applied', 'rolling_back')",
                params![transaction_id, now],
            )
            .map_err(StorageError::SaveBatchMountTransaction)?;
        ensure_one_batch_mount_row(changed, transaction_id)
    }

    pub(crate) fn finalize_batch_mount_create(
        &mut self,
        transaction_id: &str,
        plan: &StoredBatchMountPlan,
        now: i64,
    ) -> Result<(), StorageError> {
        let selected = selected_batch_mount_items(plan)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveBatchMountTransaction)?;
        let already_completed =
            validate_batch_mount_finalization(&transaction, &self.data_root, transaction_id, plan)?;
        if already_completed {
            for item in selected {
                let mount = read_mount_from(&transaction, &self.data_root, &item.mount_id)?
                    .ok_or_else(|| {
                        StorageError::BatchMountStateConflict(transaction_id.to_owned())
                    })?;
                ensure_batch_mount_matches_item(&mount, item)?;
            }
            return transaction
                .commit()
                .map_err(StorageError::SaveBatchMountTransaction);
        }
        for item in selected {
            transaction
                .execute(
                    "INSERT INTO mounts (
                        id, member_id, app_id, scope, project_id, target_path,
                        expected_target, health, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                    params![
                        item.mount_id,
                        item.member_id,
                        item.app_id.as_str(),
                        item.scope.as_str(),
                        item.project_id,
                        item.target_path,
                        item.expected_target,
                        MountHealth::Healthy.as_str(),
                        now,
                    ],
                )
                .map_err(StorageError::SaveBatchMountTransaction)?;
            let mount = read_mount_from(&transaction, &self.data_root, &item.mount_id)?
                .ok_or_else(|| StorageError::BatchMountStateConflict(transaction_id.to_owned()))?;
            ensure_batch_mount_matches_item(&mount, item)?;
            // Mount 关系一旦提交，对应路径就不再作为待接管观察重复展示。
            transaction
                .execute(
                    "DELETE FROM inventory_observations WHERE skill_root = ?1",
                    [&item.target_path],
                )
                .map_err(StorageError::SaveBatchMountTransaction)?;
        }
        let changed = transaction
            .execute(
                "UPDATE batch_mount_transactions
                 SET phase = 'state_committed', status = 'completed', updated_at = ?4
                 WHERE id = ?1 AND plan_id = ?2 AND bundle_id = ?3
                   AND phase = 'targets_applied' AND status = 'in_progress'",
                params![transaction_id, plan.id, plan.bundle_id, now],
            )
            .map_err(StorageError::SaveBatchMountTransaction)?;
        ensure_one_batch_mount_row(changed, transaction_id)?;
        transaction
            .commit()
            .map_err(StorageError::SaveBatchMountTransaction)
    }

    pub(crate) fn abort_batch_mount_transaction(
        &mut self,
        transaction_id: &str,
        error_message: Option<&str>,
        now: i64,
    ) -> Result<(), StorageError> {
        // 生命周期层只有在文件系统已恢复到事务开始前状态后，才会把 applying 之后的事务标为 aborted。
        let changed = self
            .connection
            .execute(
                "UPDATE batch_mount_transactions
                 SET status = 'aborted', error_message = ?2, updated_at = ?3
                 WHERE id = ?1 AND status = 'in_progress'
                   AND phase IN (
                       'journal_pending', 'journal_ready', 'applying',
                       'targets_applied', 'rolling_back'
                   )",
                params![transaction_id, error_message, now],
            )
            .map_err(StorageError::SaveBatchMountTransaction)?;
        ensure_one_batch_mount_row(changed, transaction_id)
    }

    pub(crate) fn block_batch_mount_transaction(
        &mut self,
        transaction_id: &str,
        error_message: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection
            .execute(
                "UPDATE batch_mount_transactions
                 SET status = 'blocked', error_message = ?2, updated_at = ?3
                 WHERE id = ?1 AND status IN ('in_progress', 'completed', 'aborted', 'blocked')",
                params![transaction_id, error_message, now],
            )
            .map_err(StorageError::SaveBatchMountTransaction)?;
        ensure_one_batch_mount_row(changed, transaction_id)
    }

    pub(crate) fn forget_terminal_batch_mount_transaction(
        &mut self,
        transaction_id: &str,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveBatchMountTransaction)?;
        let stored = transaction
            .query_row(
                "SELECT plan_id, status FROM batch_mount_transactions WHERE id = ?1",
                [transaction_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StorageError::SaveBatchMountTransaction)?;
        if let Some((plan_id, status)) = stored {
            if !matches!(status.as_str(), "completed" | "aborted") {
                return Err(StorageError::BatchMountStateConflict(
                    transaction_id.to_owned(),
                ));
            }
            let deleted = transaction
                .execute(
                    "DELETE FROM batch_mount_transactions WHERE id = ?1",
                    [transaction_id],
                )
                .map_err(StorageError::SaveBatchMountTransaction)?;
            ensure_one_batch_mount_row(deleted, transaction_id)?;
            let deleted = transaction
                .execute("DELETE FROM batch_mount_plans WHERE id = ?1", [plan_id])
                .map_err(StorageError::SaveBatchMountTransaction)?;
            ensure_one_batch_mount_row(deleted, transaction_id)?;
        }
        transaction
            .commit()
            .map_err(StorageError::SaveBatchMountTransaction)
    }

    pub(crate) fn recoverable_batch_mount_transactions(
        &self,
    ) -> Result<Vec<StoredBatchMountTransaction>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, plan_id, bundle_id, journal_path, phase, status
                 FROM batch_mount_transactions
                 WHERE status IN ('in_progress', 'completed', 'aborted', 'blocked')
                 ORDER BY created_at, id",
            )
            .map_err(StorageError::ReadBatchMountTransaction)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(StorageError::ReadBatchMountTransaction)?;
        let stored = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadBatchMountTransaction)?;
        let mut transactions = Vec::with_capacity(stored.len());
        for (id, plan_id, bundle_id, journal_path, phase, status) in stored {
            if !is_single_path_component(&id)
                || !is_single_path_component(&plan_id)
                || !is_single_path_component(&bundle_id)
                || !is_normalized_relative_path(&journal_path)
                || BatchMountTransactionPhase::from_str(&phase).is_none()
                || !matches!(
                    status.as_str(),
                    "in_progress" | "completed" | "aborted" | "blocked"
                )
            {
                return Err(StorageError::BatchMountStateConflict(id));
            }
            let selected_item_ids = read_batch_mount_transaction_item_ids(&self.connection, &id)?;
            if selected_item_ids.is_empty() {
                return Err(StorageError::BatchMountStateConflict(id));
            }
            transactions.push(StoredBatchMountTransaction {
                id,
                plan_id,
                bundle_id,
                journal_path,
                phase,
                status,
                selected_item_ids,
            });
        }
        Ok(transactions)
    }

    pub(crate) fn update_mount_health_batch(
        &mut self,
        updates: &[(String, MountHealth)],
        now: i64,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveMountTransaction)?;
        update_mount_health_rows(&transaction, updates, now)?;
        transaction
            .commit()
            .map_err(StorageError::SaveMountTransaction)
    }

    pub fn save_install_plan(&mut self, plan: NewInstallPlan<'_>) -> Result<(), StorageError> {
        validate_new_install_plan_contract(&plan)?;
        let warnings =
            serde_json::to_string(plan.warnings).map_err(StorageError::InvalidPlanWarnings)?;
        let input_device =
            i64::try_from(plan.input_device).map_err(|_| StorageError::InvalidInstallPlan)?;
        let input_inode =
            i64::try_from(plan.input_inode).map_err(|_| StorageError::InvalidInstallPlan)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveInstallPlan)?;
        transaction
            .execute(
                "INSERT INTO install_plans (
                    id, kind, install_mode, input_path, input_device, input_inode,
                    input_fingerprint, snapshot_relative_path, source_id, source_tracked_ref,
                    source_catalog_generation, source_marker, expected_source_marker,
                    expected_current_target, expected_adopted_marker, bundle_id, bundle_display_name,
                    warnings_json, created_at, expires_at, status
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                    ?15, ?16, ?17, ?18, ?19, ?20, 'pending'
                 )",
                params![
                    plan.id,
                    plan.kind,
                    plan.install_mode,
                    plan.input_path,
                    input_device,
                    input_inode,
                    plan.input_fingerprint,
                    plan.snapshot_relative_path,
                    plan.source_id,
                    plan.source_tracked_ref,
                    plan.source_catalog_generation,
                    plan.source_marker,
                    plan.expected_source_marker,
                    plan.expected_current_target,
                    plan.expected_adopted_marker,
                    plan.bundle_id,
                    plan.bundle_display_name,
                    warnings,
                    plan.created_at,
                    plan.expires_at
                ],
            )
            .map_err(StorageError::SaveInstallPlan)?;
        for (sort_order, candidate) in plan.candidates.iter().enumerate() {
            let validation_errors = serde_json::to_string(candidate.validation_errors)
                .map_err(StorageError::InvalidPlanValidationErrors)?;
            let candidate_warnings = serde_json::to_string(candidate.warnings)
                .map_err(StorageError::InvalidPlanWarnings)?;
            transaction
                .execute(
                    "INSERT INTO install_plan_candidates (
                        plan_id, candidate_id, source_relative_path, skill_name,
                        skill_description, content_fingerprint, previous_content_fingerprint, selectable,
                        preserve_existing, validation_errors_json, warnings_json,
                        default_selected, selected, sort_order
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?13
                     )",
                    params![
                        plan.id,
                        candidate.candidate_id,
                        candidate.source_relative_path,
                        candidate.skill_name,
                        candidate.skill_description,
                        candidate.content_fingerprint,
                        candidate.previous_content_fingerprint,
                        i64::from(candidate.selectable),
                        i64::from(candidate.preserve_existing),
                        validation_errors,
                        candidate_warnings,
                        i64::from(candidate.default_selected),
                        sort_order as i64,
                    ],
                )
                .map_err(StorageError::SaveInstallPlan)?;
        }
        let stored = read_install_plan_from(&transaction, plan.id)?
            .ok_or(StorageError::InvalidInstallPlan)?;
        validate_install_plan_source_contract(&transaction, &stored)?;
        transaction.commit().map_err(StorageError::SaveInstallPlan)
    }

    pub fn read_install_plan(&self, plan_id: &str) -> Result<StoredInstallPlan, StorageError> {
        read_install_plan_from(&self.connection, plan_id)?.ok_or(StorageError::InstallPlanNotFound)
    }

    pub(crate) fn save_bundle_update_batch(
        &mut self,
        batch_id: &str,
        items: &[NewBundleUpdateBatchItem<'_>],
        created_at: i64,
        expires_at: i64,
    ) -> Result<(), StorageError> {
        validate_new_bundle_update_batch(batch_id, items, created_at, expires_at)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        transaction
            .execute(
                "INSERT INTO bundle_update_batches (
                    id, status, created_at, expires_at, confirmed_at, updated_at
                 ) VALUES (?1, 'pending', ?2, ?3, NULL, ?2)",
                params![batch_id, created_at, expires_at],
            )
            .map_err(map_bundle_update_batch_insert_error)?;
        for (display_order, item) in items.iter().enumerate() {
            transaction
                .execute(
                    "INSERT INTO bundle_update_batch_items (
                        id, batch_id, source_id, bundle_id, display_name,
                        install_plan_id, target_marker, status, error,
                        display_order, confirmed_order
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL)",
                    params![
                        item.id,
                        batch_id,
                        item.source_id,
                        item.bundle_id,
                        item.display_name,
                        item.install_plan_id,
                        item.target_marker,
                        item.status,
                        item.error,
                        display_order as i64,
                    ],
                )
                .map_err(StorageError::SaveBundleUpdateBatch)?;
        }
        let stored = read_bundle_update_batch_from(&transaction, batch_id)?
            .ok_or(StorageError::InvalidBundleUpdateBatch)?;
        validate_stored_bundle_update_batch(&stored)?;
        transaction
            .commit()
            .map_err(StorageError::SaveBundleUpdateBatch)
    }

    pub(crate) fn read_open_bundle_update_batch(
        &self,
    ) -> Result<Option<StoredBundleUpdateBatch>, StorageError> {
        let id = self
            .connection
            .query_row(
                "SELECT id FROM bundle_update_batches ORDER BY created_at, id LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StorageError::ReadBundleUpdateBatch)?;
        id.map(|id| {
            read_bundle_update_batch_from(&self.connection, &id)?
                .ok_or(StorageError::BundleUpdateBatchNotFound)
        })
        .transpose()
    }

    pub(crate) fn read_bundle_update_batch(
        &self,
        batch_id: &str,
    ) -> Result<StoredBundleUpdateBatch, StorageError> {
        read_bundle_update_batch_from(&self.connection, batch_id)?
            .ok_or(StorageError::BundleUpdateBatchNotFound)
    }

    pub(crate) fn begin_bundle_update_batch(
        &mut self,
        batch_id: &str,
        selected_item_ids: &[String],
        now: i64,
    ) -> Result<StoredBundleUpdateBatch, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        let batch = read_bundle_update_batch_from(&transaction, batch_id)?
            .ok_or(StorageError::BundleUpdateBatchNotFound)?;
        if batch.status != "pending" {
            return Err(StorageError::BundleUpdateBatchConsumed);
        }
        if batch.expires_at <= now {
            return Err(StorageError::BundleUpdateBatchExpired);
        }
        let selected = selected_item_ids.iter().collect::<BTreeSet<_>>();
        if selected.is_empty()
            || selected.len() != selected_item_ids.len()
            || selected.iter().any(|item_id| {
                !batch.items.iter().any(|item| {
                    item.id.as_str() == item_id.as_str()
                        && item.status == "ready"
                        && item.install_plan_id.is_some()
                })
            })
        {
            return Err(StorageError::InvalidBundleUpdateBatchSelection);
        }
        for item_id in selected_item_ids {
            let changed = transaction
                .execute(
                    "UPDATE install_plans
                     SET expires_at = ?3
                     WHERE id = (
                         SELECT install_plan_id
                         FROM bundle_update_batch_items
                         WHERE batch_id = ?1 AND id = ?2 AND status = 'ready'
                     )
                       AND status = 'pending'",
                    params![batch_id, item_id, i64::MAX],
                )
                .map_err(StorageError::SaveBundleUpdateBatch)?;
            if changed != 1 {
                return Err(StorageError::InvalidBundleUpdateBatchSelection);
            }
        }
        transaction
            .execute(
                "UPDATE bundle_update_batch_items
                 SET status = 'failed'
                 WHERE batch_id = ?1 AND status = 'preparation_failed'",
                [batch_id],
            )
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        transaction
            .execute(
                "UPDATE bundle_update_batch_items
                 SET status = 'not_executed'
                 WHERE batch_id = ?1 AND status = 'ready'",
                [batch_id],
            )
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        for (confirmed_order, item_id) in selected_item_ids.iter().enumerate() {
            let changed = transaction
                .execute(
                    "UPDATE bundle_update_batch_items
                     SET status = 'ready', confirmed_order = ?3
                     WHERE batch_id = ?1 AND id = ?2 AND status = 'not_executed'",
                    params![batch_id, item_id, confirmed_order as i64],
                )
                .map_err(StorageError::SaveBundleUpdateBatch)?;
            if changed != 1 {
                return Err(StorageError::InvalidBundleUpdateBatchSelection);
            }
        }
        let changed = transaction
            .execute(
                "UPDATE bundle_update_batches
                 SET status = 'running', confirmed_at = ?2, updated_at = ?2
                 WHERE id = ?1 AND status = 'pending'",
                params![batch_id, now],
            )
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        if changed != 1 {
            return Err(StorageError::BundleUpdateBatchConsumed);
        }
        let started = read_bundle_update_batch_from(&transaction, batch_id)?
            .ok_or(StorageError::BundleUpdateBatchNotFound)?;
        validate_stored_bundle_update_batch(&started)?;
        transaction
            .commit()
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        Ok(started)
    }

    pub(crate) fn save_bundle_update_batch_item_result(
        &mut self,
        batch_id: &str,
        item_id: &str,
        status: &str,
        error: Option<&str>,
        now: i64,
    ) -> Result<(), StorageError> {
        if !matches!(status, "succeeded" | "failed" | "blocked")
            || (status == "succeeded" && error.is_some())
            || (status != "succeeded" && error.is_none())
        {
            return Err(StorageError::InvalidBundleUpdateBatch);
        }
        let changed = self
            .connection
            .execute(
                "UPDATE bundle_update_batch_items
                 SET status = ?3, error = ?4
                 WHERE batch_id = ?1
                   AND id = ?2
                   AND status = 'ready'
                   AND confirmed_order IS NOT NULL
                   AND EXISTS (
                       SELECT 1 FROM bundle_update_batches AS batch
                       WHERE batch.id = ?1 AND batch.status = 'running'
                   )",
                params![batch_id, item_id, status, error],
            )
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        if changed == 0 {
            let existing = self.read_bundle_update_batch(batch_id)?;
            let matches_terminal = existing
                .items
                .iter()
                .find(|item| item.id == item_id)
                .is_some_and(|item| item.status == status && item.error.as_deref() == error);
            if !matches_terminal {
                return Err(StorageError::InvalidBundleUpdateBatch);
            }
        }
        self.connection
            .execute(
                "UPDATE bundle_update_batches
                 SET updated_at = ?2
                 WHERE id = ?1 AND status = 'running'",
                params![batch_id, now],
            )
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        Ok(())
    }

    pub(crate) fn finish_bundle_update_batch(
        &mut self,
        batch_id: &str,
        status: &str,
        now: i64,
    ) -> Result<StoredBundleUpdateBatch, StorageError> {
        if !matches!(status, "completed" | "blocked") {
            return Err(StorageError::InvalidBundleUpdateBatch);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        if status == "blocked" {
            transaction
                .execute(
                    "UPDATE bundle_update_batch_items
                     SET status = 'not_executed'
                     WHERE batch_id = ?1 AND status = 'ready'",
                    [batch_id],
                )
                .map_err(StorageError::SaveBundleUpdateBatch)?;
        }
        let unfinished = transaction
            .query_row(
                "SELECT COUNT(*) FROM bundle_update_batch_items
                 WHERE batch_id = ?1 AND status = 'ready'",
                [batch_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StorageError::ReadBundleUpdateBatch)?;
        let blocked = transaction
            .query_row(
                "SELECT COUNT(*) FROM bundle_update_batch_items
                 WHERE batch_id = ?1 AND status = 'blocked'",
                [batch_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StorageError::ReadBundleUpdateBatch)?;
        if unfinished != 0
            || (status == "completed" && blocked != 0)
            || (status == "blocked" && blocked != 1)
        {
            return Err(StorageError::InvalidBundleUpdateBatch);
        }
        let changed = transaction
            .execute(
                "UPDATE bundle_update_batches
                 SET status = ?2, updated_at = ?3
                 WHERE id = ?1 AND status = 'running'",
                params![batch_id, status, now],
            )
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        if changed != 1 {
            return Err(StorageError::BundleUpdateBatchConsumed);
        }
        let finished = read_bundle_update_batch_from(&transaction, batch_id)?
            .ok_or(StorageError::BundleUpdateBatchNotFound)?;
        validate_stored_bundle_update_batch(&finished)?;
        transaction
            .commit()
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        Ok(finished)
    }

    pub(crate) fn delete_pending_bundle_update_batch(
        &mut self,
        batch_id: &str,
    ) -> Result<(), StorageError> {
        let deleted = self
            .connection
            .execute(
                "DELETE FROM bundle_update_batches
                 WHERE id = ?1 AND status = 'pending'",
                [batch_id],
            )
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        if deleted != 1 {
            return Err(StorageError::BundleUpdateBatchConsumed);
        }
        Ok(())
    }

    pub(crate) fn acknowledge_bundle_update_batch(
        &mut self,
        batch_id: &str,
    ) -> Result<(), StorageError> {
        let deleted = self
            .connection
            .execute(
                "DELETE FROM bundle_update_batches
                 WHERE id = ?1 AND status = 'completed'",
                [batch_id],
            )
            .map_err(StorageError::SaveBundleUpdateBatch)?;
        if deleted != 1 {
            return Err(StorageError::BundleUpdateBatchConsumed);
        }
        Ok(())
    }

    pub(crate) fn bundle_has_adopted_marker(
        &self,
        bundle_id: &str,
        marker: &str,
    ) -> Result<bool, StorageError> {
        self.connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM source_bundle_links
                    WHERE bundle_id = ?1 AND adopted_marker = ?2
                 )",
                params![bundle_id, marker],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StorageError::ReadBundleUpdateBatch)
    }

    pub(crate) fn install_plan_transaction_status(
        &self,
        plan_id: &str,
    ) -> Result<Option<String>, StorageError> {
        self.connection
            .query_row(
                "SELECT status FROM lifecycle_transactions WHERE plan_id = ?1",
                [plan_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StorageError::ReadBundleUpdateBatch)
    }

    pub(crate) fn source_kind_and_locator(
        &self,
        source_id: &str,
    ) -> Result<(SourceKind, String), StorageError> {
        let (kind, locator) = self
            .connection
            .query_row(
                "SELECT kind, locator FROM sources WHERE id = ?1",
                [source_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StorageError::ReadBundleUpdateBatch)?
            .ok_or(StorageError::SourceNotFound)?;
        let kind = SourceKind::from_str(&kind)
            .ok_or_else(|| StorageError::UnknownSourceKind(kind.clone()))?;
        Ok((kind, locator))
    }

    pub fn read_expired_pending_install_plans(
        &self,
        now: i64,
    ) -> Result<Vec<StoredInstallPlan>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id FROM install_plans
                 WHERE status = 'pending'
                   AND kind = 'source_snapshot'
                   AND expires_at <= ?1
                   AND NOT EXISTS (
                       SELECT 1
                       FROM bundle_update_batch_items AS batch_item
                       JOIN bundle_update_batches AS batch
                         ON batch.id = batch_item.batch_id
                       WHERE batch_item.install_plan_id = install_plans.id
                   )
                 ORDER BY created_at, id",
            )
            .map_err(StorageError::ReadInstallPlan)?;
        let ids = statement
            .query_map([now], |row| row.get::<_, String>(0))
            .map_err(StorageError::ReadInstallPlan)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadInstallPlan)?;
        ids.into_iter()
            .map(|id| {
                read_install_plan_from(&self.connection, &id)?
                    .ok_or(StorageError::InstallPlanNotFound)
            })
            .collect()
    }

    pub fn read_pending_source_install_plans_for_source(
        &self,
        source_id: &str,
    ) -> Result<Vec<StoredInstallPlan>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id FROM install_plans
                 WHERE status = 'pending'
                   AND kind = 'source_snapshot'
                   AND source_id = ?1
                   AND NOT EXISTS (
                       SELECT 1
                       FROM bundle_update_batch_items AS batch_item
                       JOIN bundle_update_batches AS batch
                         ON batch.id = batch_item.batch_id
                       WHERE batch_item.install_plan_id = install_plans.id
                   )
                 ORDER BY created_at, id",
            )
            .map_err(StorageError::ReadInstallPlan)?;
        let ids = statement
            .query_map([source_id], |row| row.get::<_, String>(0))
            .map_err(StorageError::ReadInstallPlan)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadInstallPlan)?;
        ids.into_iter()
            .map(|id| {
                read_install_plan_from(&self.connection, &id)?
                    .ok_or(StorageError::InstallPlanNotFound)
            })
            .collect()
    }

    /// 文件清理前先验证 owner，避免普通放弃入口破坏 Batch 持有的快照。
    pub(crate) fn ensure_install_plan_discard_owner(
        &self,
        plan_id: &str,
        owner: Option<BundleUpdateBatchChildOwner<'_>>,
    ) -> Result<(), StorageError> {
        validate_bundle_update_batch_child_owner(
            &self.connection,
            plan_id,
            owner,
            BundleUpdateBatchChildOperation::Discard,
        )
    }

    /// 只删除尚未进入生命周期事务的失效 Plan；普通入口不能消费 Batch child。
    pub fn discard_pending_install_plan(&mut self, plan_id: &str) -> Result<(), StorageError> {
        self.discard_pending_install_plan_with_owner(plan_id, None)
    }

    pub(crate) fn discard_pending_bundle_update_batch_child_plan(
        &mut self,
        plan_id: &str,
        owner: BundleUpdateBatchChildOwner<'_>,
    ) -> Result<(), StorageError> {
        self.discard_pending_install_plan_with_owner(plan_id, Some(owner))
    }

    fn discard_pending_install_plan_with_owner(
        &mut self,
        plan_id: &str,
        owner: Option<BundleUpdateBatchChildOwner<'_>>,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveInstallPlan)?;
        validate_bundle_update_batch_child_owner(
            &transaction,
            plan_id,
            owner,
            BundleUpdateBatchChildOperation::Discard,
        )?;
        let deleted = transaction
            .execute(
                "DELETE FROM install_plans
                 WHERE id = ?1
                   AND status = 'pending'
                   AND NOT EXISTS (
                       SELECT 1 FROM lifecycle_transactions WHERE plan_id = ?1
                   )",
                [plan_id],
            )
            .map_err(StorageError::SaveInstallPlan)?;
        if deleted != 1 {
            return Err(StorageError::InstallPlanConsumed);
        }
        transaction.commit().map_err(StorageError::SaveInstallPlan)
    }

    /// Plan 行可能被恢复器清理；进入过确认的 Plan 仍不能再走放弃入口。
    pub fn install_plan_confirmation_has_started(
        &self,
        plan_id: &str,
    ) -> Result<bool, StorageError> {
        self.connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM install_plans
                    WHERE id = ?1 AND status <> 'pending'
                    UNION ALL
                    SELECT 1 FROM lifecycle_transactions WHERE plan_id = ?1
                 )",
                [plan_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|exists| exists != 0)
            .map_err(StorageError::ReadInstallPlan)
    }

    #[cfg(test)]
    pub fn begin_install_transaction(
        &mut self,
        plan_id: &str,
        transaction_id: &str,
        journal_path: &str,
        now: i64,
    ) -> Result<StoredInstallPlan, StorageError> {
        let selected_candidate_ids = self
            .read_install_plan(plan_id)?
            .candidates
            .into_iter()
            .filter(|candidate| candidate.selectable && candidate.default_selected)
            .map(|candidate| candidate.candidate_id)
            .collect::<Vec<_>>();
        self.begin_install_transaction_with_selection(
            plan_id,
            &selected_candidate_ids,
            transaction_id,
            journal_path,
            now,
        )
    }

    pub fn begin_install_transaction_with_selection(
        &mut self,
        plan_id: &str,
        selected_candidate_ids: &[String],
        transaction_id: &str,
        journal_path: &str,
        now: i64,
    ) -> Result<StoredInstallPlan, StorageError> {
        self.begin_install_transaction_with_selection_owned(
            plan_id,
            selected_candidate_ids,
            transaction_id,
            journal_path,
            now,
            None,
        )
    }

    pub(crate) fn begin_bundle_update_batch_child_transaction_with_selection(
        &mut self,
        plan_id: &str,
        selected_candidate_ids: &[String],
        transaction_id: &str,
        journal_path: &str,
        now: i64,
        owner: BundleUpdateBatchChildOwner<'_>,
    ) -> Result<StoredInstallPlan, StorageError> {
        self.begin_install_transaction_with_selection_owned(
            plan_id,
            selected_candidate_ids,
            transaction_id,
            journal_path,
            now,
            Some(owner),
        )
    }

    fn begin_install_transaction_with_selection_owned(
        &mut self,
        plan_id: &str,
        selected_candidate_ids: &[String],
        transaction_id: &str,
        journal_path: &str,
        now: i64,
        owner: Option<BundleUpdateBatchChildOwner<'_>>,
    ) -> Result<StoredInstallPlan, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveLifecycleTransaction)?;
        let mut plan = read_install_plan_from(&transaction, plan_id)?
            .ok_or(StorageError::InstallPlanNotFound)?;
        if plan.status != "pending" {
            return Err(StorageError::InstallPlanConsumed);
        }
        if plan.expires_at <= now {
            return Err(StorageError::InstallPlanExpired);
        }
        validate_bundle_update_batch_child_owner(
            &transaction,
            plan_id,
            owner,
            BundleUpdateBatchChildOperation::Confirm,
        )?;
        validate_install_plan_source_contract(&transaction, &plan)?;
        if bundle_or_source_write_is_blocked(
            &transaction,
            Some(&plan.bundle_id),
            plan.source_id.as_deref(),
        )? {
            return Err(StorageError::ManagedObjectBlocked);
        }
        let selected = selected_candidate_ids.iter().collect::<BTreeSet<_>>();
        if selected.is_empty() || selected.len() != selected_candidate_ids.len() {
            return Err(StorageError::InvalidInstallSelection);
        }
        if plan.install_mode == "update" {
            let required = plan
                .candidates
                .iter()
                .filter(|candidate| !candidate.preserve_existing)
                .map(|candidate| &candidate.candidate_id)
                .collect::<BTreeSet<_>>();
            if selected != required
                || plan
                    .candidates
                    .iter()
                    .filter(|candidate| !candidate.preserve_existing)
                    .any(|candidate| !candidate.selectable)
            {
                return Err(StorageError::InvalidInstallSelection);
            }
        }
        if selected.iter().any(|candidate_id| {
            !plan.candidates.iter().any(|candidate| {
                candidate.selectable
                    && !candidate.preserve_existing
                    && candidate.candidate_id.as_str() == candidate_id.as_str()
            })
        }) {
            return Err(StorageError::InvalidInstallSelection);
        }
        transaction
            .execute(
                "UPDATE install_plan_candidates
                 SET selected = preserve_existing
                 WHERE plan_id = ?1",
                [&plan.id],
            )
            .map_err(StorageError::SaveLifecycleTransaction)?;
        for candidate_id in &selected {
            let changed = transaction
                .execute(
                    "UPDATE install_plan_candidates
                     SET selected = 1
                     WHERE plan_id = ?1 AND candidate_id = ?2 AND selectable = 1",
                    params![plan.id, candidate_id.as_str()],
                )
                .map_err(StorageError::SaveLifecycleTransaction)?;
            ensure_one_lifecycle_row(changed, transaction_id)?;
        }
        let anchor_member_id = plan
            .candidates
            .iter()
            .find(|candidate| {
                candidate.preserve_existing || selected.contains(&candidate.candidate_id)
            })
            .expect("前面已拒绝空选择")
            .candidate_id
            .as_str();
        let inserted = transaction
            .execute(
                "INSERT INTO lifecycle_transactions (id, kind, plan_id, bundle_id, member_id, journal_path, phase, status, created_at, updated_at)
                 VALUES (?1, 'install_bundle', ?2, ?3, ?4, ?5, 'journal_pending', 'in_progress', ?6, ?6)",
                params![
                    transaction_id,
                    plan.id,
                    plan.bundle_id,
                    anchor_member_id,
                    journal_path,
                    now
                ],
            )
            .map_err(map_lifecycle_insert_error)?;
        ensure_one_lifecycle_row(inserted, transaction_id)?;
        let consumed = transaction
            .execute(
                "UPDATE install_plans SET status = 'consumed' WHERE id = ?1 AND status = 'pending'",
                [&plan.id],
            )
            .map_err(StorageError::SaveLifecycleTransaction)?;
        ensure_one_lifecycle_row(consumed, transaction_id)?;
        // 返回这次 SQLite 事务实际确认的内存快照，避免提交后第三次读取引入新的竞态窗口。
        plan.status = "consumed".to_owned();
        for candidate in &mut plan.candidates {
            candidate.selected =
                candidate.preserve_existing || selected.contains(&candidate.candidate_id);
        }
        transaction
            .commit()
            .map_err(StorageError::SaveLifecycleTransaction)?;
        Ok(plan)
    }

    pub fn update_lifecycle_phase(
        &mut self,
        transaction_id: &str,
        phase: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let next = LifecyclePhase::from_str(phase)
            .ok_or_else(|| StorageError::InvalidLifecyclePhase(phase.to_owned()))?;
        let Some(previous) = next.previous() else {
            return Err(StorageError::LifecycleStateConflict(
                transaction_id.to_owned(),
            ));
        };
        let changed = self
            .connection
            .execute(
                "UPDATE lifecycle_transactions
                 SET phase = ?2, updated_at = ?4
                 WHERE id = ?1
                   AND status = 'in_progress'
                   AND phase IN (?2, ?3)",
                params![transaction_id, next.as_str(), previous.as_str(), now],
            )
            .map_err(StorageError::SaveLifecycleTransaction)?;
        ensure_one_lifecycle_row(changed, transaction_id)
    }

    pub fn abort_lifecycle_transaction(
        &mut self,
        transaction_id: &str,
        error_message: Option<&str>,
        now: i64,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection
            .execute(
                "UPDATE lifecycle_transactions
                 SET status = 'aborted', error_message = ?2, updated_at = ?3
                 WHERE id = ?1
                   AND status = 'in_progress'
                   AND phase IN ('journal_pending', 'journal_ready', 'candidate_ready')",
                params![transaction_id, error_message, now],
            )
            .map_err(StorageError::SaveLifecycleTransaction)?;
        ensure_one_lifecycle_row(changed, transaction_id)
    }

    pub fn block_lifecycle_transaction(
        &mut self,
        transaction_id: &str,
        error_message: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection
            .execute(
                "UPDATE lifecycle_transactions SET status = 'blocked', error_message = ?2, updated_at = ?3 WHERE id = ?1 AND status IN ('in_progress', 'completed', 'aborted')",
                params![transaction_id, error_message, now],
            )
            .map_err(StorageError::SaveLifecycleTransaction)?;
        ensure_one_lifecycle_row(changed, transaction_id)
    }

    pub fn forget_terminal_transaction(
        &mut self,
        transaction_id: &str,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveLifecycleTransaction)?;
        let stored = transaction
            .query_row(
                "SELECT plan_id, status FROM lifecycle_transactions WHERE id = ?1",
                [transaction_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StorageError::SaveLifecycleTransaction)?;
        if let Some((plan_id, status)) = stored {
            if !matches!(status.as_str(), "completed" | "aborted") {
                return Err(StorageError::LifecycleStateConflict(
                    transaction_id.to_owned(),
                ));
            }
            let deleted_transaction = transaction
                .execute(
                    "DELETE FROM lifecycle_transactions WHERE id = ?1 AND status IN ('completed', 'aborted')",
                    [transaction_id],
                )
                .map_err(StorageError::SaveLifecycleTransaction)?;
            ensure_one_lifecycle_row(deleted_transaction, transaction_id)?;
            let deleted_plan = transaction
                .execute("DELETE FROM install_plans WHERE id = ?1", [plan_id])
                .map_err(StorageError::SaveLifecycleTransaction)?;
            ensure_one_lifecycle_row(deleted_plan, transaction_id)?;
        }
        transaction
            .commit()
            .map_err(StorageError::SaveLifecycleTransaction)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finalize_install(
        &mut self,
        transaction_id: &str,
        plan: &StoredInstallPlan,
        managed_directory: &str,
        current_target: &str,
        stable_relative_path: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let selected = selected_install_candidates(plan)?;
        let anchor = selected
            .first()
            .expect("selected_install_candidates 已拒绝空集合");
        validate_managed_install_paths(
            transaction_id,
            plan,
            &selected,
            managed_directory,
            current_target,
            stable_relative_path,
        )?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveManagedBundle)?;
        let persisted_plan = read_install_plan_from(&transaction, &plan.id)?
            .ok_or_else(|| StorageError::LifecycleStateConflict(transaction_id.to_owned()))?;
        if persisted_plan != *plan {
            return Err(StorageError::ManagedStateConflict);
        }
        ensure_install_transaction_can_finalize(
            &transaction,
            transaction_id,
            plan,
            &anchor.candidate_id,
        )?;
        match plan.install_mode.as_str() {
            "create" => finalize_install_create_rows(
                &transaction,
                plan,
                &selected,
                managed_directory,
                current_target,
                now,
            )?,
            "supplement" => finalize_install_supplement_rows(
                &transaction,
                plan,
                &selected,
                managed_directory,
                current_target,
                now,
            )?,
            "update" => finalize_install_update_rows(
                &transaction,
                plan,
                &selected,
                managed_directory,
                current_target,
                now,
            )?,
            _ => return Err(StorageError::InvalidInstallPlan),
        }
        ensure_managed_state_matches(
            &transaction,
            plan,
            &selected,
            managed_directory,
            current_target,
        )?;
        ensure_source_install_state_matches(&transaction, plan, &selected)?;
        let changed = transaction
            .execute(
                "UPDATE lifecycle_transactions
                 SET phase = 'state_committed', status = 'completed', updated_at = ?5
                 WHERE id = ?1
                   AND kind = 'install_bundle'
                   AND plan_id = ?2
                   AND bundle_id = ?3
                   AND member_id = ?4
                   AND (
                       (status = 'in_progress' AND phase IN ('candidate_ready', 'activated'))
                       OR (status = 'completed' AND phase = 'state_committed')
                   )",
                params![
                    transaction_id,
                    plan.id,
                    plan.bundle_id,
                    anchor.candidate_id,
                    now
                ],
            )
            .map_err(StorageError::SaveManagedBundle)?;
        ensure_one_lifecycle_row(changed, transaction_id)?;
        transaction
            .commit()
            .map_err(StorageError::SaveManagedBundle)
    }

    pub fn recoverable_lifecycle_transactions(
        &self,
    ) -> Result<Vec<StoredLifecycleTransaction>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, plan_id, bundle_id, member_id, journal_path, phase, status FROM lifecycle_transactions WHERE status IN ('in_progress', 'completed', 'aborted', 'blocked') ORDER BY created_at",
            )
            .map_err(StorageError::ReadLifecycleTransaction)?;
        let rows = statement
            .query_map([], |row| {
                Ok(StoredLifecycleTransaction {
                    id: row.get(0)?,
                    plan_id: row.get(1)?,
                    bundle_id: row.get(2)?,
                    member_id: row.get(3)?,
                    journal_path: row.get(4)?,
                    phase: row.get(5)?,
                    status: row.get(6)?,
                })
            })
            .map_err(StorageError::ReadLifecycleTransaction)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadLifecycleTransaction)
    }

    /// 启动提示只关心恢复器即将重放的普通事务；pending Plan 和 blocked 状态不算恢复成功。
    pub(crate) fn has_pending_recovery_work(&self) -> Result<bool, StorageError> {
        if self
            .recoverable_lifecycle_transactions()?
            .iter()
            .any(|transaction| transaction.status != "blocked")
            || self
                .recoverable_source_association_transactions()?
                .iter()
                .any(|transaction| transaction.status != "blocked")
            || self
                .recoverable_mount_transactions()?
                .iter()
                .any(|transaction| transaction.status != "blocked")
            || self
                .recoverable_batch_mount_transactions()?
                .iter()
                .any(|transaction| transaction.status != "blocked")
            || self
                .recoverable_takeover_transactions()?
                .iter()
                .any(|transaction| transaction.status != "blocked")
            || self
                .recoverable_removal_transactions()?
                .iter()
                .any(|transaction| transaction.status != "blocked")
        {
            return Ok(true);
        }

        Ok(self
            .read_open_bundle_update_batch()?
            .is_some_and(|batch| batch.status == "running"))
    }

    pub fn managed_bundle_notice_rows(
        &self,
    ) -> Result<Vec<(String, String, Vec<String>)>, StorageError> {
        let mut mounts_by_bundle = BTreeMap::<String, Vec<String>>::new();
        for mount in self.read_mounts()? {
            mounts_by_bundle
                .entry(mount.bundle_id)
                .or_default()
                .push(mount.target_path);
        }
        for targets in mounts_by_bundle.values_mut() {
            targets.sort();
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, display_name, managed_directory, current_target FROM bundles ORDER BY display_name, id",
            )
            .map_err(StorageError::ReadInventory)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(StorageError::ReadInventory)?;
        let mut safe_rows = Vec::new();
        for row in rows {
            let (bundle_id, display_name, managed_directory, current_target) =
                row.map_err(StorageError::ReadInventory)?;
            if !is_single_path_component(&bundle_id)
                || managed_directory != format!("bundles/{bundle_id}")
                || !is_safe_current_target(&current_target)
            {
                return Err(StorageError::UnsafeManagedPath(managed_directory));
            }
            safe_rows.push((
                display_name,
                managed_directory,
                mounts_by_bundle.remove(&bundle_id).unwrap_or_default(),
            ));
        }
        Ok(safe_rows)
    }

    /// Notice 只展示 Source 的稳定发布页；Catalog 与错误详情仍以 SQLite 为准。
    pub fn source_notice_rows(&self) -> Result<Vec<(String, String)>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT display_name, locator
                 FROM sources
                 ORDER BY sort_order, id",
            )
            .map_err(StorageError::ReadSources)?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(StorageError::ReadSources)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadSources)
    }

    /// Removal Plan 的 JSON 与摘要一同持久化，确认时必须重新核对摘要。
    pub(crate) fn save_removal_plan(
        &mut self,
        plan: NewRemovalPlan<'_>,
    ) -> Result<(), StorageError> {
        if !is_single_path_component(plan.id)
            || !is_single_path_component(plan.target_id)
            || !matches!(plan.kind, "project" | "source" | "bundle" | "bundle_mounts")
            || plan.payload_json.is_empty()
            || plan.payload_sha256.len() != 64
            || !plan
                .payload_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || plan.created_at < 0
            || plan.expires_at <= plan.created_at
        {
            return Err(StorageError::RemovalStateConflict);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveRemoval)?;
        transaction
            .execute(
                "DELETE FROM removal_plans
                 WHERE kind = ?1 AND target_id = ?2 AND status = 'pending'",
                params![plan.kind, plan.target_id],
            )
            .map_err(StorageError::SaveRemoval)?;
        transaction
            .execute(
                "INSERT INTO removal_plans (
                    id, kind, target_id, payload_json, payload_sha256,
                    status, created_at, expires_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7)",
                params![
                    plan.id,
                    plan.kind,
                    plan.target_id,
                    plan.payload_json,
                    plan.payload_sha256,
                    plan.created_at,
                    plan.expires_at
                ],
            )
            .map_err(StorageError::SaveRemoval)?;
        transaction.commit().map_err(StorageError::SaveRemoval)
    }

    pub(crate) fn read_removal_plan(
        &self,
        plan_id: &str,
    ) -> Result<StoredRemovalPlan, StorageError> {
        let plan = self
            .connection
            .query_row(
                "SELECT id, kind, target_id, payload_json, payload_sha256,
                        status, created_at, expires_at
                 FROM removal_plans WHERE id = ?1",
                [plan_id],
                stored_removal_plan_from_row,
            )
            .optional()
            .map_err(StorageError::ReadRemoval)?
            .ok_or(StorageError::RemovalPlanNotFound)?;
        validate_stored_removal_plan(&plan)?;
        Ok(plan)
    }

    pub(crate) fn read_pending_removal_plan(
        &self,
    ) -> Result<Option<StoredRemovalPlan>, StorageError> {
        let plan = self
            .connection
            .query_row(
                "SELECT id, kind, target_id, payload_json, payload_sha256,
                        status, created_at, expires_at
                 FROM removal_plans
                 WHERE status = 'pending'
                 ORDER BY created_at, id
                 LIMIT 1",
                [],
                stored_removal_plan_from_row,
            )
            .optional()
            .map_err(StorageError::ReadRemoval)?;
        if let Some(plan) = &plan {
            validate_stored_removal_plan(plan)?;
        }
        Ok(plan)
    }

    pub(crate) fn discard_removal_plan(&mut self, plan_id: &str) -> Result<(), StorageError> {
        let deleted = self
            .connection
            .execute(
                "DELETE FROM removal_plans WHERE id = ?1 AND status = 'pending'",
                [plan_id],
            )
            .map_err(StorageError::SaveRemoval)?;
        if deleted == 1 {
            Ok(())
        } else {
            Err(StorageError::RemovalPlanNotFound)
        }
    }

    /// 先提交 journal_pending 行，随后生命周期层才能写 Journal。
    pub(crate) fn begin_removal_transaction(
        &mut self,
        plan_id: &str,
        transaction_id: &str,
        journal_path: &str,
        now: i64,
    ) -> Result<StoredRemovalPlan, StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveRemoval)?;
        let plan = transaction
            .query_row(
                "SELECT id, kind, target_id, payload_json, payload_sha256,
                        status, created_at, expires_at
                 FROM removal_plans WHERE id = ?1",
                [plan_id],
                stored_removal_plan_from_row,
            )
            .optional()
            .map_err(StorageError::ReadRemoval)?
            .ok_or(StorageError::RemovalPlanNotFound)?;
        validate_stored_removal_plan(&plan)?;
        if plan.status != "pending" {
            return Err(StorageError::RemovalPlanNotFound);
        }
        if plan.expires_at <= now {
            return Err(StorageError::RemovalPlanExpired);
        }
        if !matches!(plan.kind.as_str(), "project" | "bundle" | "bundle_mounts")
            || !is_single_path_component(transaction_id)
            || journal_path != format!("journals/{transaction_id}.json")
        {
            return Err(StorageError::RemovalStateConflict);
        }
        transaction
            .execute(
                "INSERT INTO removal_transactions (
                    id, plan_id, kind, target_id, journal_path, phase, status,
                    error_message, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, 'journal_pending', 'in_progress',
                    NULL, ?6, ?6
                 )",
                params![
                    transaction_id,
                    plan.id,
                    plan.kind,
                    plan.target_id,
                    journal_path,
                    now
                ],
            )
            .map_err(map_removal_transaction_insert_error)?;
        transaction
            .execute(
                "UPDATE removal_plans SET status = 'consumed'
                 WHERE id = ?1 AND status = 'pending'",
                [plan_id],
            )
            .map_err(StorageError::SaveRemoval)?;
        transaction.commit().map_err(StorageError::SaveRemoval)?;
        Ok(StoredRemovalPlan {
            status: "consumed".to_owned(),
            ..plan
        })
    }

    pub(crate) fn update_removal_phase(
        &mut self,
        transaction_id: &str,
        expected_phase: &str,
        next_phase: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let updated = self
            .connection
            .execute(
                "UPDATE removal_transactions
                 SET phase = ?3, updated_at = ?4
                 WHERE id = ?1 AND phase = ?2 AND status = 'in_progress'",
                params![transaction_id, expected_phase, next_phase, now],
            )
            .map_err(StorageError::SaveRemoval)?;
        if updated == 1 {
            Ok(())
        } else {
            Err(StorageError::RemovalStateConflict)
        }
    }

    pub(crate) fn recoverable_removal_transactions(
        &self,
    ) -> Result<Vec<StoredRemovalTransaction>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, plan_id, kind, target_id, journal_path, phase, status
                 FROM removal_transactions
                 ORDER BY created_at, id",
            )
            .map_err(StorageError::ReadRemoval)?;
        let rows = statement
            .query_map([], stored_removal_transaction_from_row)
            .map_err(StorageError::ReadRemoval)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadRemoval)
    }

    pub(crate) fn abort_removal_transaction(
        &mut self,
        transaction_id: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        let updated = self
            .connection
            .execute(
                "UPDATE removal_transactions
                 SET status = 'aborted', error_message = NULL, updated_at = ?2
                 WHERE id = ?1 AND status = 'in_progress'",
                params![transaction_id, now],
            )
            .map_err(StorageError::SaveRemoval)?;
        if updated == 1 {
            Ok(())
        } else if self
            .connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM removal_transactions
                    WHERE id = ?1 AND status = 'aborted'
                 )",
                [transaction_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StorageError::ReadRemoval)?
        {
            // 崩溃可能发生在标记 aborted 之后；重复恢复不能因此转成人工阻塞。
            Ok(())
        } else {
            Err(StorageError::RemovalStateConflict)
        }
    }

    pub(crate) fn block_removal_transaction(
        &mut self,
        transaction_id: &str,
        message: &str,
        now: i64,
    ) -> Result<(), StorageError> {
        self.connection
            .execute(
                "UPDATE removal_transactions
                 SET status = 'blocked', error_message = ?2, updated_at = ?3
                 WHERE id = ?1 AND status <> 'blocked'",
                params![transaction_id, message, now],
            )
            .map_err(StorageError::SaveRemoval)?;
        Ok(())
    }

    pub(crate) fn forget_terminal_removal_transaction(
        &mut self,
        transaction_id: &str,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveRemoval)?;
        let plan_id = transaction
            .query_row(
                "SELECT plan_id FROM removal_transactions
                 WHERE id = ?1 AND status IN ('completed', 'aborted')",
                [transaction_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StorageError::ReadRemoval)?
            .ok_or(StorageError::RemovalStateConflict)?;
        transaction
            .execute(
                "DELETE FROM removal_transactions WHERE id = ?1",
                [transaction_id],
            )
            .map_err(StorageError::SaveRemoval)?;
        transaction
            .execute("DELETE FROM removal_plans WHERE id = ?1", [plan_id])
            .map_err(StorageError::SaveRemoval)?;
        transaction.commit().map_err(StorageError::SaveRemoval)
    }

    pub(crate) fn read_pending_install_plans_for_bundle(
        &self,
        bundle_id: &str,
    ) -> Result<Vec<StoredInstallPlan>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id FROM install_plans
                 WHERE status = 'pending'
                   AND bundle_id = ?1
                   AND NOT EXISTS (
                       SELECT 1
                       FROM bundle_update_batch_items AS batch_item
                       JOIN bundle_update_batches AS batch
                         ON batch.id = batch_item.batch_id
                       WHERE batch_item.install_plan_id = install_plans.id
                   )
                 ORDER BY created_at, id",
            )
            .map_err(StorageError::ReadInstallPlan)?;
        let ids = statement
            .query_map([bundle_id], |row| row.get::<_, String>(0))
            .map_err(StorageError::ReadInstallPlan)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadInstallPlan)?;
        ids.into_iter()
            .map(|id| {
                read_install_plan_from(&self.connection, &id)?
                    .ok_or(StorageError::InstallPlanNotFound)
            })
            .collect()
    }

    /// Source 删除是纯 SQLite 事务；关联 Bundle 与其 Current Content 不参与删除。
    pub(crate) fn finalize_source_removal(
        &mut self,
        plan_id: &str,
        source_id: &str,
        canonical_identity: &str,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveRemoval)?;
        let plan = read_removal_plan_from(&transaction, plan_id)?
            .ok_or(StorageError::RemovalPlanNotFound)?;
        if plan.kind != "source"
            || plan.target_id != source_id
            || plan.status != "pending"
            || transaction
                .query_row(
                    "SELECT canonical_identity FROM sources WHERE id = ?1",
                    [source_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(StorageError::ReadSources)?
                .as_deref()
                != Some(canonical_identity)
        {
            return Err(StorageError::RemovalStateConflict);
        }
        let pending_count = transaction
            .query_row(
                "SELECT COUNT(*) FROM install_plans WHERE source_id = ?1",
                [source_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StorageError::ReadInstallPlan)?;
        if pending_count != 0 {
            return Err(StorageError::RemovalStateConflict);
        }
        transaction
            .execute("DELETE FROM sources WHERE id = ?1", [source_id])
            .map_err(StorageError::SaveRemoval)?;
        transaction
            .execute("DELETE FROM removal_plans WHERE id = ?1", [plan_id])
            .map_err(StorageError::SaveRemoval)?;
        transaction.commit().map_err(StorageError::SaveRemoval)
    }

    pub(crate) fn finalize_project_removal(
        &mut self,
        transaction_id: &str,
        project: &StoredProject,
        expected_mount_ids: &[String],
        now: i64,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveRemoval)?;
        ensure_removal_transaction_phase(
            &transaction,
            transaction_id,
            "project",
            &project.id,
            "mounts_isolated",
        )?;
        let current_project = read_project_from(&transaction, &project.id)?
            .ok_or(StorageError::RemovalStateConflict)?;
        if &current_project != project {
            return Err(StorageError::RemovalStateConflict);
        }
        let current_mount_ids = read_sorted_string_column(
            &transaction,
            "SELECT id FROM mounts WHERE project_id = ?1 ORDER BY id",
            &project.id,
        )?;
        let mut expected_mount_ids = expected_mount_ids.to_vec();
        expected_mount_ids.sort();
        if current_mount_ids != expected_mount_ids {
            return Err(StorageError::RemovalStateConflict);
        }
        transaction
            .execute(
                "DELETE FROM batch_mount_plans
                 WHERE id IN (
                    SELECT plan_id FROM batch_mount_plan_items WHERE project_id = ?1
                 )",
                [&project.id],
            )
            .map_err(StorageError::SaveRemoval)?;
        transaction
            .execute(
                "DELETE FROM mount_plans WHERE project_id = ?1",
                [&project.id],
            )
            .map_err(StorageError::SaveRemoval)?;
        transaction
            .execute("DELETE FROM mounts WHERE project_id = ?1", [&project.id])
            .map_err(StorageError::SaveRemoval)?;
        let deleted = transaction
            .execute("DELETE FROM projects WHERE id = ?1", [&project.id])
            .map_err(StorageError::SaveRemoval)?;
        if deleted != 1 {
            return Err(StorageError::RemovalStateConflict);
        }
        transaction
            .execute(
                "UPDATE removal_transactions
                 SET phase = 'state_committed', status = 'completed', updated_at = ?2
                 WHERE id = ?1 AND status = 'in_progress' AND phase = 'mounts_isolated'",
                params![transaction_id, now],
            )
            .map_err(StorageError::SaveRemoval)?;
        transaction.commit().map_err(StorageError::SaveRemoval)
    }

    pub(crate) fn finalize_bundle_mount_removal(
        &mut self,
        transaction_id: &str,
        bundle_id: &str,
        expected_mount_ids: &[String],
        now: i64,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveRemoval)?;
        ensure_removal_transaction_phase(
            &transaction,
            transaction_id,
            "bundle_mounts",
            bundle_id,
            "mounts_isolated",
        )?;
        let current_mount_ids = read_sorted_string_column(
            &transaction,
            "SELECT mount.id
             FROM mounts AS mount
             JOIN skill_members AS member ON member.id = mount.member_id
             WHERE member.bundle_id = ?1
             ORDER BY mount.id",
            bundle_id,
        )?;
        let mut expected_mount_ids = expected_mount_ids.to_vec();
        expected_mount_ids.sort();
        if current_mount_ids != expected_mount_ids || current_mount_ids.is_empty() {
            return Err(StorageError::RemovalStateConflict);
        }
        transaction
            .execute(
                "DELETE FROM mount_plans
                 WHERE member_id IN (
                    SELECT id FROM skill_members WHERE bundle_id = ?1
                 )",
                [bundle_id],
            )
            .map_err(StorageError::SaveRemoval)?;
        transaction
            .execute(
                "DELETE FROM batch_mount_plans WHERE bundle_id = ?1",
                [bundle_id],
            )
            .map_err(StorageError::SaveRemoval)?;
        transaction
            .execute(
                "DELETE FROM mounts
                 WHERE member_id IN (
                    SELECT id FROM skill_members WHERE bundle_id = ?1
                 )",
                [bundle_id],
            )
            .map_err(StorageError::SaveRemoval)?;
        let updated = transaction
            .execute(
                "UPDATE removal_transactions
                 SET phase = 'state_committed', status = 'completed', updated_at = ?2
                 WHERE id = ?1 AND status = 'in_progress' AND phase = 'mounts_isolated'",
                params![transaction_id, now],
            )
            .map_err(StorageError::SaveRemoval)?;
        if updated != 1 {
            return Err(StorageError::RemovalStateConflict);
        }
        transaction.commit().map_err(StorageError::SaveRemoval)
    }

    // 这些参数共同构成确认页封存的 Bundle 删除合同，保持扁平可避免再造第二个领域类型。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finalize_bundle_removal(
        &mut self,
        transaction_id: &str,
        bundle_id: &str,
        display_name: &str,
        managed_directory: &str,
        current_target: &str,
        expected_member_ids: &[String],
        expected_mount_ids: &[String],
        now: i64,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveRemoval)?;
        ensure_removal_transaction_phase(
            &transaction,
            transaction_id,
            "bundle",
            bundle_id,
            "bundle_isolated",
        )?;
        let current_bundle = transaction
            .query_row(
                "SELECT display_name, managed_directory, current_target
                 FROM bundles WHERE id = ?1",
                [bundle_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(StorageError::ReadInventory)?
            .ok_or(StorageError::RemovalStateConflict)?;
        if current_bundle
            != (
                display_name.to_owned(),
                managed_directory.to_owned(),
                current_target.to_owned(),
            )
        {
            return Err(StorageError::RemovalStateConflict);
        }
        let current_member_ids = read_sorted_string_column(
            &transaction,
            "SELECT id FROM skill_members WHERE bundle_id = ?1 ORDER BY id",
            bundle_id,
        )?;
        let mut expected_member_ids = expected_member_ids.to_vec();
        expected_member_ids.sort();
        if current_member_ids != expected_member_ids {
            return Err(StorageError::RemovalStateConflict);
        }
        let current_mount_ids = read_sorted_string_column(
            &transaction,
            "SELECT mount.id
             FROM mounts AS mount
             JOIN skill_members AS member ON member.id = mount.member_id
             WHERE member.bundle_id = ?1
             ORDER BY mount.id",
            bundle_id,
        )?;
        let mut expected_mount_ids = expected_mount_ids.to_vec();
        expected_mount_ids.sort();
        if current_mount_ids != expected_mount_ids {
            return Err(StorageError::RemovalStateConflict);
        }
        let pending_plan_count = transaction
            .query_row(
                "SELECT COUNT(*) FROM install_plans WHERE bundle_id = ?1",
                [bundle_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StorageError::ReadInstallPlan)?;
        if pending_plan_count != 0 {
            return Err(StorageError::RemovalStateConflict);
        }
        transaction
            .execute(
                "DELETE FROM mount_plans
                 WHERE member_id IN (
                    SELECT id FROM skill_members WHERE bundle_id = ?1
                 )",
                [bundle_id],
            )
            .map_err(StorageError::SaveRemoval)?;
        transaction
            .execute(
                "DELETE FROM batch_mount_plans WHERE bundle_id = ?1",
                [bundle_id],
            )
            .map_err(StorageError::SaveRemoval)?;
        transaction
            .execute(
                "DELETE FROM mounts
                 WHERE member_id IN (
                    SELECT id FROM skill_members WHERE bundle_id = ?1
                 )",
                [bundle_id],
            )
            .map_err(StorageError::SaveRemoval)?;
        let deleted = transaction
            .execute("DELETE FROM bundles WHERE id = ?1", [bundle_id])
            .map_err(StorageError::SaveRemoval)?;
        if deleted != 1 {
            return Err(StorageError::RemovalStateConflict);
        }
        transaction
            .execute(
                "UPDATE removal_transactions
                 SET phase = 'state_committed', status = 'completed', updated_at = ?2
                 WHERE id = ?1 AND status = 'in_progress' AND phase = 'bundle_isolated'",
                params![transaction_id, now],
            )
            .map_err(StorageError::SaveRemoval)?;
        transaction.commit().map_err(StorageError::SaveRemoval)
    }

    fn with_managed_entries(
        &self,
        entries: Vec<InventoryObservation>,
    ) -> Result<Vec<InventoryItem>, StorageError> {
        let project_names = self
            .read_projects()?
            .into_iter()
            .map(|project| (project.id, project.display_name))
            .collect::<BTreeMap<_, _>>();
        let mut entries = entries
            .into_iter()
            .map(|observation| {
                let project_display_name = observation
                    .project_id
                    .as_ref()
                    .and_then(|project_id| project_names.get(project_id))
                    .cloned();
                inventory_item_from_observation(observation, project_display_name)
            })
            .collect::<Vec<_>>();
        entries.extend(read_managed_entries_from(
            &self.connection,
            &self.data_root,
        )?);
        entries.sort_by(|left, right| left.skill_root.cmp(&right.skill_root));
        Ok(entries)
    }

    fn read_supported_apps(&self) -> Result<Vec<SupportedAppSummary>, StorageError> {
        read_supported_apps_from(&self.connection)
    }

    fn read_inventory_entries(&self) -> Result<Vec<InventoryObservation>, StorageError> {
        read_inventory_entries_from(&self.connection)
    }

    fn read_scan_issues(&self) -> Result<Vec<ScanIssue>, StorageError> {
        read_scan_issues_from(&self.connection)
    }

    pub(crate) fn read_mount_summaries(&self) -> Result<Vec<MountSummary>, StorageError> {
        self.read_mounts()?
            .into_iter()
            .map(|mount| {
                Ok(MountSummary {
                    id: mount.id,
                    member_id: mount.member_id,
                    skill_name: mount.skill_name,
                    app_id: mount.app_id,
                    scope: mount.scope,
                    project_id: mount.project_id,
                    project_display_name: mount.project_display_name,
                    target_path: mount.target_path,
                    expected_target: mount.expected_target,
                    health: mount.health,
                })
            })
            .collect()
    }

    fn read_recovery_issues(&self) -> Result<Vec<RecoveryIssue>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, display_name, message FROM (
                    SELECT lifecycle.id AS id,
                           COALESCE(plan.bundle_display_name, lifecycle.bundle_id) AS display_name,
                           COALESCE(lifecycle.error_message, '事务状态无法自动判断') AS message,
                           lifecycle.created_at AS created_at
                    FROM lifecycle_transactions AS lifecycle
                    LEFT JOIN install_plans AS plan ON plan.id = lifecycle.plan_id
                    WHERE lifecycle.status = 'blocked'
                    UNION ALL
                    SELECT mount_tx.id AS id,
                           COALESCE(member.skill_name, mount_tx.mount_id) AS display_name,
                           COALESCE(mount_tx.error_message, 'Mount 事务状态无法自动判断') AS message,
                           mount_tx.created_at AS created_at
                    FROM mount_transactions AS mount_tx
                    LEFT JOIN mount_plans AS plan ON plan.id = mount_tx.plan_id
                    LEFT JOIN skill_members AS member ON member.id = plan.member_id
                    WHERE mount_tx.status = 'blocked'
                    UNION ALL
                    SELECT batch_tx.id AS id,
                           COALESCE(bundle.display_name, batch_tx.bundle_id) AS display_name,
                           COALESCE(batch_tx.error_message, 'Batch Mount 事务状态无法自动判断') AS message,
                           batch_tx.created_at AS created_at
                    FROM batch_mount_transactions AS batch_tx
                    LEFT JOIN bundles AS bundle ON bundle.id = batch_tx.bundle_id
                    WHERE batch_tx.status = 'blocked'
                    UNION ALL
                    SELECT takeover_tx.id AS id,
                           takeover_tx.plan_id AS display_name,
                           COALESCE(takeover_tx.error_message, 'Takeover 事务状态无法自动判断') AS message,
                           takeover_tx.created_at AS created_at
                    FROM takeover_transactions AS takeover_tx
                    WHERE takeover_tx.status = 'blocked'
                    UNION ALL
                    SELECT association_tx.id AS id,
                           COALESCE(bundle.display_name, association_tx.target_bundle_id) AS display_name,
                           COALESCE(
                               association_tx.error_message,
                               'Bundle Merge 事务状态无法自动判断'
                           ) AS message,
                           association_tx.created_at AS created_at
                    FROM source_association_transactions AS association_tx
                    LEFT JOIN bundles AS bundle
                      ON bundle.id = association_tx.target_bundle_id
                    WHERE association_tx.status = 'blocked'
                    UNION ALL
                    SELECT removal_tx.id AS id,
                           COALESCE(
                               bundle.display_name,
                               project.display_name,
                               source.display_name,
                               removal_tx.target_id
                           ) AS display_name,
                           COALESCE(
                               removal_tx.error_message,
                               'Removal 事务状态无法自动判断'
                           ) AS message,
                           removal_tx.created_at AS created_at
                    FROM removal_transactions AS removal_tx
                    LEFT JOIN bundles AS bundle
                      ON removal_tx.kind IN ('bundle', 'bundle_mounts')
                     AND bundle.id = removal_tx.target_id
                    LEFT JOIN projects AS project
                      ON removal_tx.kind = 'project' AND project.id = removal_tx.target_id
                    LEFT JOIN sources AS source
                      ON removal_tx.kind = 'source' AND source.id = removal_tx.target_id
                    WHERE removal_tx.status = 'blocked'
                 ) ORDER BY created_at, id",
            )
            .map_err(StorageError::ReadRecoveryIssues)?;
        let rows = statement
            .query_map([], |row| {
                Ok(RecoveryIssue {
                    id: row.get(0)?,
                    bundle_display_name: row.get(1)?,
                    message: row.get(2)?,
                })
            })
            .map_err(StorageError::ReadRecoveryIssues)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadRecoveryIssues)
    }
}

fn ensure_safe_data_root(data_root: &Path) -> Result<(), StorageError> {
    match fs::symlink_metadata(data_root) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(StorageError::UnsafeDataRoot(data_root.to_owned()));
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            fs::create_dir_all(data_root).map_err(StorageError::CreateDataRoot)?;
        }
        Err(error) => return Err(StorageError::InspectDataRoot(error)),
    }

    // 创建后再检查一次，避免把并发替换成符号链接的目录交给 SQLite。
    let metadata = fs::symlink_metadata(data_root).map_err(StorageError::InspectDataRoot)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StorageError::UnsafeDataRoot(data_root.to_owned()));
    }
    Ok(())
}

fn safe_database_open_path(data_root: &Path, database: &Path) -> Result<PathBuf, StorageError> {
    if database.parent() != Some(data_root) {
        return Err(StorageError::UnsafeDatabase(database.to_owned()));
    }
    match fs::symlink_metadata(database) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.nlink() != 1 {
                return Err(StorageError::UnsafeDatabase(database.to_owned()));
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(StorageError::InspectDatabase(error)),
    }
    let file_name = database.file_name().ok_or_else(|| {
        StorageError::InspectDatabase(std::io::Error::new(
            ErrorKind::InvalidInput,
            "SQLite 路径缺少文件名",
        ))
    })?;
    if !is_single_path_component(file_name.to_string_lossy().as_ref()) {
        return Err(StorageError::UnsafeDatabase(database.to_owned()));
    }
    // macOS 的 /var 本身是符号链接；只规范化已校验目录，最终数据库文件仍由 NOFOLLOW 保护。
    let canonical_root = fs::canonicalize(data_root).map_err(StorageError::InspectDatabase)?;
    Ok(canonical_root.join(file_name))
}

fn ensure_one_lifecycle_row(changed: usize, transaction_id: &str) -> Result<(), StorageError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(StorageError::LifecycleStateConflict(
            transaction_id.to_owned(),
        ))
    }
}

fn ensure_one_mount_row(changed: usize, transaction_id: &str) -> Result<(), StorageError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(StorageError::MountStateConflict(transaction_id.to_owned()))
    }
}

fn ensure_one_batch_mount_row(changed: usize, transaction_id: &str) -> Result<(), StorageError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(StorageError::BatchMountStateConflict(
            transaction_id.to_owned(),
        ))
    }
}

fn ensure_one_takeover_row(changed: usize, transaction_id: &str) -> Result<(), StorageError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(StorageError::TakeoverStateConflict(
            transaction_id.to_owned(),
        ))
    }
}

fn ensure_one_source_association_row(
    changed: usize,
    transaction_id: &str,
) -> Result<(), StorageError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(StorageError::SourceAssociationStateConflict(
            transaction_id.to_owned(),
        ))
    }
}

fn previous_source_association_phase(phase: &str) -> Result<Option<&'static str>, StorageError> {
    // state_committed 只能由最终领域提交写入，通用阶段 API 不得伪造完成。
    match phase {
        "journal_pending" => Ok(None),
        "journal_ready" => Ok(Some("journal_pending")),
        "candidate_ready" => Ok(Some("journal_ready")),
        "current_activated" => Ok(Some("candidate_ready")),
        "mounts_applied" => Ok(Some("current_activated")),
        "state_committed" => Ok(None),
        unknown => Err(StorageError::InvalidSourceAssociationPhase(
            unknown.to_owned(),
        )),
    }
}

fn source_association_phase_and_status_are_consistent(phase: &str, status: &str) -> bool {
    let phase_is_known = matches!(
        phase,
        "journal_pending"
            | "journal_ready"
            | "candidate_ready"
            | "current_activated"
            | "mounts_applied"
            | "state_committed"
    );
    let terminal_state_is_consistent = match status {
        "completed" => phase == "state_committed",
        "in_progress" | "aborted" => phase != "state_committed",
        "blocked" => true,
        _ => false,
    };
    phase_is_known && terminal_state_is_consistent
}

fn previous_takeover_phase(phase: &str) -> Result<Option<&'static str>, StorageError> {
    // state_committed 只能由领域提交原子写入，通用阶段 API 不得提前越过生效点。
    match phase {
        "journal_pending" => Ok(None),
        "journal_ready" => Ok(Some("journal_pending")),
        "candidate_ready" => Ok(Some("journal_ready")),
        "current_activated" => Ok(Some("candidate_ready")),
        "origins_applied" => Ok(Some("current_activated")),
        "state_committed" => Ok(None),
        unknown => Err(StorageError::InvalidTakeoverPhase(unknown.to_owned())),
    }
}

fn takeover_phase_and_status_are_consistent(phase: &str, status: &str) -> bool {
    // 恢复只接受 migration 定义的已知状态，避免将损坏记录解释为可重放事务。
    let phase_is_known = matches!(
        phase,
        "journal_pending"
            | "journal_ready"
            | "candidate_ready"
            | "current_activated"
            | "origins_applied"
            | "state_committed"
    );
    let terminal_state_is_consistent = match status {
        "completed" => phase == "state_committed",
        "in_progress" | "aborted" => phase != "state_committed",
        "blocked" => true,
        _ => false,
    };
    phase_is_known && terminal_state_is_consistent
}

fn update_mount_health_rows(
    connection: &Connection,
    updates: &[(String, MountHealth)],
    now: i64,
) -> Result<(), StorageError> {
    for (mount_id, health) in updates {
        let changed = connection
            .execute(
                "UPDATE mounts SET health = ?2, updated_at = ?3 WHERE id = ?1",
                params![mount_id, health.as_str(), now],
            )
            .map_err(StorageError::SaveMountTransaction)?;
        ensure_one_mount_row(changed, mount_id)?;
    }
    Ok(())
}

fn filesystem_identity_to_sql(value: u64) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| StorageError::InvalidFilesystemIdentity(i64::MAX))
}

fn filesystem_identity_from_sql(value: i64) -> Result<u64, StorageError> {
    u64::try_from(value).map_err(|_| StorageError::InvalidFilesystemIdentity(value))
}

fn is_normalized_absolute_path(value: &str) -> bool {
    let path = Path::new(value);
    if !path.is_absolute() {
        return false;
    }
    let normalized = path.components().collect::<PathBuf>();
    normalized.to_str() == Some(value)
        && path.components().all(|component| {
            matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
}

fn is_normalized_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        && path.components().collect::<PathBuf>().to_str() == Some(value)
}

fn stored_project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredProject> {
    let root_device = row.get::<_, i64>(3)?;
    let root_inode = row.get::<_, i64>(4)?;
    Ok(StoredProject {
        id: row.get(0)?,
        display_name: row.get(1)?,
        root_path: row.get(2)?,
        root_device: u64::try_from(root_device)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(3, root_device))?,
        root_inode: u64::try_from(root_inode)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(4, root_inode))?,
        created_at: row.get(5)?,
    })
}

fn validate_new_project(project: &NewProject<'_>) -> Result<(), StorageError> {
    if !is_single_path_component(project.id)
        || project.display_name.trim().is_empty()
        || !is_normalized_absolute_path(project.root_path)
    {
        return Err(StorageError::ProjectIdentityConflict);
    }
    Ok(())
}

fn persist_project_from(
    connection: &Connection,
    project: &StoredProject,
) -> Result<(), StorageError> {
    if !is_single_path_component(&project.id)
        || project.display_name.trim().is_empty()
        || !is_normalized_absolute_path(&project.root_path)
    {
        return Err(StorageError::ProjectIdentityConflict);
    }
    let root_device = filesystem_identity_to_sql(project.root_device)?;
    let root_inode = filesystem_identity_to_sql(project.root_inode)?;
    if let Some(existing) = read_project_by_identity_from(
        connection,
        &project.id,
        &project.root_path,
        root_device,
        root_inode,
    )? {
        if existing != *project {
            return Err(StorageError::ProjectIdentityConflict);
        }
        return Ok(());
    }
    connection
        .execute(
            "INSERT INTO projects (id, display_name, root_path, root_device, root_inode, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                project.id,
                project.display_name,
                project.root_path,
                root_device,
                root_inode,
                project.created_at
            ],
        )
        .map_err(StorageError::SaveProject)?;
    Ok(())
}

fn read_project_from(
    connection: &Connection,
    project_id: &str,
) -> Result<Option<StoredProject>, StorageError> {
    connection
        .query_row(
            "SELECT id, display_name, root_path, root_device, root_inode, created_at
             FROM projects WHERE id = ?1",
            [project_id],
            stored_project_from_row,
        )
        .optional()
        .map_err(StorageError::ReadProject)
}

fn read_project_by_identity_from(
    connection: &Connection,
    project_id: &str,
    root_path: &str,
    root_device: i64,
    root_inode: i64,
) -> Result<Option<StoredProject>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT id, display_name, root_path, root_device, root_inode, created_at
             FROM projects
             WHERE id = ?1 OR root_path = ?2 OR (root_device = ?3 AND root_inode = ?4)",
        )
        .map_err(StorageError::ReadProject)?;
    let rows = statement
        .query_map(
            params![project_id, root_path, root_device, root_inode],
            stored_project_from_row,
        )
        .map_err(StorageError::ReadProject)?;
    let projects = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::ReadProject)?;
    match projects.as_slice() {
        [] => Ok(None),
        [project] => Ok(Some(project.clone())),
        _ => Err(StorageError::ProjectIdentityConflict),
    }
}

fn read_managed_member_from(
    connection: &Connection,
    data_root: &Path,
    member_id: &str,
) -> Result<Option<StoredManagedMember>, StorageError> {
    let stored = connection
        .query_row(
            "SELECT member.id, member.bundle_id, member.skill_name, member.content_fingerprint,
                    bundle.managed_directory, bundle.current_target, member.stable_relative_path
             FROM skill_members AS member
             JOIN bundles AS bundle ON bundle.id = member.bundle_id
             WHERE member.id = ?1",
            [member_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadManagedMember)?;
    let Some((
        id,
        bundle_id,
        skill_name,
        content_fingerprint,
        managed_directory,
        current_target,
        stable_relative_path,
    )) = stored
    else {
        return Ok(None);
    };
    validate_stored_managed_paths(
        &bundle_id,
        &skill_name,
        &managed_directory,
        &current_target,
        &stable_relative_path,
    )?;
    let expected_target = data_root
        .join(&managed_directory)
        .join("current")
        .join(&stable_relative_path);
    let expected_target = expected_target
        .to_str()
        .ok_or_else(|| StorageError::UnsafeManagedPath(managed_directory.clone()))?
        .to_owned();
    if !is_normalized_absolute_path(&expected_target) {
        return Err(StorageError::UnsafeManagedPath(managed_directory));
    }
    Ok(Some(StoredManagedMember {
        id,
        bundle_id,
        skill_name,
        content_fingerprint,
        managed_directory,
        current_target,
        stable_relative_path,
        expected_target,
    }))
}

fn validate_new_mount_plan_shape(plan: &NewMountPlan<'_>) -> Result<(), StorageError> {
    let project_shape_is_valid = match plan.scope {
        MountScope::Global => plan.project_id.is_none(),
        MountScope::Project => plan.project_id.is_some_and(is_single_path_component),
    };
    if !is_single_path_component(plan.id)
        || plan.purpose.operation() != plan.operation
        || !is_single_path_component(plan.mount_id)
        || !is_single_path_component(plan.member_id)
        || !project_shape_is_valid
        || !is_normalized_absolute_path(plan.target_path)
        || !is_normalized_absolute_path(plan.expected_target)
        || plan.member_fingerprint.is_empty()
        || plan.target_observation.is_empty()
        || plan.expires_at <= plan.created_at
    {
        return Err(StorageError::InvalidMountPlan);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_mount_plan_state(
    connection: &Connection,
    data_root: &Path,
    operation: MountOperation,
    purpose: MountPlanPurpose,
    mount_id: &str,
    member_id: &str,
    app_id: SupportedAppId,
    scope: MountScope,
    project_id: Option<&str>,
    target_path: &str,
    expected_target: &str,
    member_fingerprint: &str,
) -> Result<(), StorageError> {
    if purpose.operation() != operation {
        return Err(StorageError::InvalidMountPlan);
    }
    let member = read_managed_member_from(connection, data_root, member_id)?
        .ok_or_else(|| StorageError::ManagedMemberNotFound(member_id.to_owned()))?;
    if member.content_fingerprint != member_fingerprint || member.expected_target != expected_target
    {
        return Err(StorageError::InvalidMountPlan);
    }
    if Path::new(target_path)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(member.skill_name.as_str())
    {
        return Err(StorageError::InvalidMountPlan);
    }
    if mount_object_is_blocked(connection, member_id, target_path, project_id)? {
        return Err(StorageError::ManagedObjectBlocked);
    }
    match (scope, project_id) {
        (MountScope::Global, None) => {}
        (MountScope::Project, Some(project_id)) => {
            read_project_from(connection, project_id)?
                .ok_or_else(|| StorageError::ProjectNotFound(project_id.to_owned()))?;
        }
        _ => return Err(StorageError::InvalidMountPlan),
    }

    let mut statement = connection
        .prepare(
            "SELECT id FROM mounts
             WHERE (member_id = ?1 AND app_id = ?2) OR target_path = ?3
             ORDER BY id",
        )
        .map_err(StorageError::ReadMountPlan)?;
    let rows = statement
        .query_map(params![member_id, app_id.as_str(), target_path], |row| {
            row.get::<_, String>(0)
        })
        .map_err(StorageError::ReadMountPlan)?;
    let ids = rows
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(StorageError::ReadMountPlan)?;
    let mounts = ids
        .into_iter()
        .map(|id| {
            read_mount_from(connection, data_root, &id)?
                .ok_or_else(|| StorageError::MountNotFound(id.clone()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    match purpose {
        MountPlanPurpose::Create => {
            for existing in mounts {
                let same_slot = existing.member_id == member_id
                    && existing.app_id == app_id
                    && existing.scope == scope
                    && existing.project_id.as_deref() == project_id;
                let exact = same_slot
                    && existing.id == mount_id
                    && existing.target_path == target_path
                    && existing.expected_target == expected_target;
                if existing.target_path == target_path && !exact {
                    return Err(StorageError::InvalidMountPlan);
                }
                if existing.member_id == member_id
                    && existing.app_id == app_id
                    && (existing.scope != scope || (same_slot && !exact))
                {
                    return Err(StorageError::InvalidMountPlan);
                }
            }
        }
        MountPlanPurpose::Repair | MountPlanPurpose::Remove => {
            let existing = mounts
                .iter()
                .find(|mount| mount.id == mount_id)
                .ok_or_else(|| StorageError::MountNotFound(mount_id.to_owned()))?;
            if existing.member_id != member_id
                || existing.app_id != app_id
                || existing.scope != scope
                || existing.project_id.as_deref() != project_id
                || existing.target_path != target_path
                || existing.expected_target != expected_target
                || existing.member_fingerprint != member_fingerprint
            {
                return Err(StorageError::InvalidMountPlan);
            }
        }
    }
    Ok(())
}

fn validate_stored_mount_plan_state(
    connection: &Connection,
    data_root: &Path,
    plan: &StoredMountPlan,
) -> Result<(), StorageError> {
    validate_mount_plan_state(
        connection,
        data_root,
        plan.operation,
        plan.purpose,
        &plan.mount_id,
        &plan.member_id,
        plan.app_id,
        plan.scope,
        plan.project_id.as_deref(),
        &plan.target_path,
        &plan.expected_target,
        &plan.member_fingerprint,
    )
}

/// 人工恢复按 Source／Bundle 隔离写操作；其他 Bundle 仍可正常使用。
fn bundle_or_source_write_is_blocked(
    connection: &Connection,
    bundle_id: Option<&str>,
    source_id: Option<&str>,
) -> Result<bool, StorageError> {
    connection
        .query_row(
            "SELECT
                EXISTS(
                    SELECT 1
                    FROM lifecycle_transactions AS lifecycle
                    JOIN install_plans AS plan ON plan.id = lifecycle.plan_id
                    WHERE lifecycle.status = 'blocked'
                      AND (
                        (?1 IS NOT NULL AND lifecycle.bundle_id = ?1)
                        OR (?2 IS NOT NULL AND plan.source_id = ?2)
                      )
                )
                OR EXISTS(
                    SELECT 1
                    FROM mount_transactions AS mount_tx
                    JOIN mount_plans AS mount_plan ON mount_plan.id = mount_tx.plan_id
                    JOIN skill_members AS member ON member.id = mount_plan.member_id
                    WHERE mount_tx.status = 'blocked'
                      AND ?1 IS NOT NULL
                      AND member.bundle_id = ?1
                )
                OR EXISTS(
                    SELECT 1
                    FROM batch_mount_transactions AS batch_tx
                    WHERE batch_tx.status = 'blocked'
                      AND ?1 IS NOT NULL
                      AND batch_tx.bundle_id = ?1
                )
                OR EXISTS(
                    SELECT 1
                    FROM takeover_transactions AS takeover_tx
                    WHERE takeover_tx.status = 'blocked'
                      AND ?1 IS NOT NULL
                      AND takeover_tx.bundle_id = ?1
                )
                OR EXISTS(
                    SELECT 1
                    FROM source_association_transactions AS association_tx
                    WHERE association_tx.status = 'blocked'
                      AND (
                        (?1 IS NOT NULL AND (
                            association_tx.target_bundle_id = ?1
                            OR association_tx.retiring_bundle_id = ?1
                        ))
                        OR (?2 IS NOT NULL AND association_tx.source_id = ?2)
                      )
                )
                OR EXISTS(
                    SELECT 1
                    FROM removal_transactions AS removal_tx
                    LEFT JOIN source_bundle_links AS removal_link
                      ON removal_tx.kind IN ('bundle', 'bundle_mounts')
                     AND removal_link.bundle_id = removal_tx.target_id
                    WHERE removal_tx.status = 'blocked'
                      AND removal_tx.kind IN ('bundle', 'bundle_mounts')
                      AND (
                        (?1 IS NOT NULL AND removal_tx.target_id = ?1)
                        OR (?2 IS NOT NULL AND removal_link.source_id = ?2)
                      )
                )",
            params![bundle_id, source_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StorageError::ReadRecoveryIssues)
}

fn mount_object_is_blocked(
    connection: &Connection,
    member_id: &str,
    target_path: &str,
    project_id: Option<&str>,
) -> Result<bool, StorageError> {
    let bundle_id = connection
        .query_row(
            "SELECT bundle_id FROM skill_members WHERE id = ?1",
            [member_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(StorageError::ReadManagedMember)?;
    let install_is_blocked = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM lifecycle_transactions
                WHERE status = 'blocked'
                  AND ?1 IS NOT NULL
                  AND bundle_id = ?1
             )",
            [bundle_id.as_deref()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StorageError::ReadLifecycleTransaction)?;
    if install_is_blocked {
        return Ok(true);
    }

    let removal_is_blocked = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM removal_transactions AS removal_tx
                WHERE removal_tx.status = 'blocked'
                  AND (
                    (
                        removal_tx.kind IN ('bundle', 'bundle_mounts')
                        AND removal_tx.target_id = ?1
                    )
                    OR (
                        removal_tx.kind = 'project'
                        AND ?2 IS NOT NULL
                        AND removal_tx.target_id = ?2
                    )
                  )
             )",
            params![bundle_id.as_deref(), project_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StorageError::ReadRecoveryIssues)?;
    if removal_is_blocked {
        return Ok(true);
    }

    let mount_is_blocked = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM mount_transactions AS mount_tx
                JOIN mount_plans AS plan ON plan.id = mount_tx.plan_id
                WHERE mount_tx.status = 'blocked'
                  AND (plan.member_id = ?1 OR plan.target_path = ?2)
             )",
            params![member_id, target_path],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StorageError::ReadMountTransaction)?;
    if mount_is_blocked {
        return Ok(true);
    }

    let batch_is_blocked = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM batch_mount_transactions AS batch
                JOIN batch_mount_transaction_items AS selected
                  ON selected.transaction_id = batch.id
                JOIN batch_mount_plan_items AS item
                  ON item.plan_id = selected.plan_id AND item.id = selected.item_id
                WHERE batch.status = 'blocked'
                  AND (item.member_id = ?1 OR item.target_path = ?2)
             )",
            params![member_id, target_path],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StorageError::ReadBatchMountTransaction)?;
    if batch_is_blocked {
        return Ok(true);
    }

    let association_is_blocked = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM source_association_transactions AS association_tx
                LEFT JOIN skill_members AS member ON member.id = ?1
                WHERE association_tx.status = 'blocked'
                  AND (
                    association_tx.target_bundle_id = member.bundle_id
                    OR association_tx.retiring_bundle_id = member.bundle_id
                  )
             )",
            [member_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StorageError::ReadSourceAssociationTransaction)?;
    if association_is_blocked {
        return Ok(true);
    }

    // 提交前 Member 尚未创建，只能按锚点和路径隔离；提交后整个新 Bundle 都属于同一恢复对象。
    let mut statement = connection
        .prepare(
            "SELECT id, bundle_id, member_id, reserved_paths_json
             FROM takeover_transactions
             WHERE status = 'blocked'",
        )
        .map_err(StorageError::ReadTakeoverTransaction)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(StorageError::ReadTakeoverTransaction)?;
    for row in rows {
        let (transaction_id, reserved_bundle_id, reserved_member_id, reserved_paths_json) =
            row.map_err(StorageError::ReadTakeoverTransaction)?;
        let reserved_paths = decode_takeover_reserved_paths(&transaction_id, &reserved_paths_json)?;
        if bundle_id.as_deref() == Some(reserved_bundle_id.as_str())
            || reserved_member_id == member_id
            || reserved_paths
                .binary_search_by(|reserved| reserved.as_str().cmp(target_path))
                .is_ok()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn validate_new_batch_mount_plan_shape(plan: &NewBatchMountPlan<'_>) -> Result<(), StorageError> {
    if !is_single_path_component(plan.id)
        || !is_single_path_component(plan.bundle_id)
        || plan.items.is_empty()
        || plan.expires_at <= plan.created_at
    {
        return Err(StorageError::InvalidBatchMountPlan);
    }
    let mut item_ids = BTreeSet::new();
    let mut mount_ids = BTreeSet::new();
    let mut ready_targets = BTreeSet::new();
    for item in plan.items {
        validate_new_batch_mount_item_shape(item)?;
        if !item_ids.insert(item.id) || !mount_ids.insert(item.mount_id) {
            return Err(StorageError::InvalidBatchMountPlan);
        }
        if item.disposition == BatchMountDisposition::Ready
            && !ready_targets.insert(item.target_path)
        {
            return Err(StorageError::InvalidBatchMountPlan);
        }
    }
    Ok(())
}

fn validate_new_batch_mount_item_shape(
    item: &NewBatchMountPlanItem<'_>,
) -> Result<(), StorageError> {
    let valid_project = match item.scope {
        MountScope::Global => item.project_id.is_none(),
        MountScope::Project => item.project_id.is_some_and(is_single_path_component),
    };
    let valid_disposition = match item.disposition {
        BatchMountDisposition::Ready => {
            item.selectable
                && item.conflict_reason.is_none()
                && item.target_health != MountHealth::Conflict
        }
        BatchMountDisposition::AlreadyMounted => {
            // 已登记关系可能发生 Drift；批量预览仍需展示真实 health，但不能重复选择。
            !item.selectable && item.conflict_reason.is_none()
        }
        BatchMountDisposition::PathConflict | BatchMountDisposition::ScopeConflict => {
            !item.selectable
                && item
                    .conflict_reason
                    .is_some_and(|reason| !reason.trim().is_empty())
        }
    };
    if !is_single_path_component(item.id)
        || !is_single_path_component(item.mount_id)
        || !is_single_path_component(item.member_id)
        || !valid_project
        || !is_normalized_absolute_path(item.target_path)
        || !is_normalized_absolute_path(item.expected_target)
        || item.member_fingerprint.is_empty()
        || item.target_observation.is_empty()
        || !valid_disposition
        || (item.default_selected && !item.selectable)
    {
        return Err(StorageError::InvalidBatchMountPlan);
    }
    Ok(())
}

fn validate_batch_ready_item_state(
    connection: &Connection,
    data_root: &Path,
    item: &StoredBatchMountPlanItem,
) -> Result<(), StorageError> {
    if item.disposition != BatchMountDisposition::Ready
        || !item.selectable
        || item.conflict_reason.is_some()
    {
        return Err(StorageError::InvalidBatchMountPlan);
    }
    let member = read_managed_member_from(connection, data_root, &item.member_id)?
        .ok_or_else(|| StorageError::ManagedMemberNotFound(item.member_id.clone()))?;
    if member.bundle_id != item.bundle_id
        || member.skill_name != item.skill_name
        || member.content_fingerprint != item.member_fingerprint
        || member.expected_target != item.expected_target
        || Path::new(&item.target_path)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(item.skill_name.as_str())
    {
        return Err(StorageError::InvalidBatchMountPlan);
    }
    if mount_object_is_blocked(
        connection,
        &item.member_id,
        &item.target_path,
        item.project_id.as_deref(),
    )? {
        return Err(StorageError::ManagedObjectBlocked);
    }
    match item.scope {
        MountScope::Global => {
            validate_project_binding(
                item.scope,
                item.project_id.as_deref(),
                item.project_display_name.as_deref(),
                item.project_root_path.as_deref(),
                item.project_root_device,
                item.project_root_inode,
            )?;
        }
        MountScope::Project => {
            let project_id = item
                .project_id
                .as_deref()
                .ok_or(StorageError::InvalidBatchMountPlan)?;
            let project = read_project_from(connection, project_id)?
                .ok_or_else(|| StorageError::ProjectNotFound(project_id.to_owned()))?;
            if item.project_display_name.as_deref() != Some(project.display_name.as_str())
                || item.project_root_path.as_deref() != Some(project.root_path.as_str())
                || item.project_root_device != Some(project.root_device)
                || item.project_root_inode != Some(project.root_inode)
            {
                return Err(StorageError::InvalidBatchMountPlan);
            }
        }
    }
    let conflicting = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM mounts
                WHERE id = ?1
                   OR target_path = ?2
                   OR (
                        member_id = ?3 AND app_id = ?4 AND (
                            scope != ?5
                            OR ?5 = 'global'
                            OR project_id IS ?6
                        )
                   )
             )",
            params![
                item.mount_id,
                item.target_path,
                item.member_id,
                item.app_id.as_str(),
                item.scope.as_str(),
                item.project_id,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StorageError::ReadBatchMountPlan)?;
    if conflicting {
        return Err(StorageError::InvalidBatchMountPlan);
    }
    Ok(())
}

fn read_batch_mount_plan_from(
    connection: &Connection,
    data_root: &Path,
    plan_id: &str,
) -> Result<Option<StoredBatchMountPlan>, StorageError> {
    let stored = connection
        .query_row(
            "SELECT plan.id, plan.bundle_id, bundle.display_name,
                    plan.created_at, plan.expires_at, plan.status
             FROM batch_mount_plans AS plan
             JOIN bundles AS bundle ON bundle.id = plan.bundle_id
             WHERE plan.id = ?1",
            [plan_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadBatchMountPlan)?;
    let Some((id, bundle_id, bundle_display_name, created_at, expires_at, status)) = stored else {
        return Ok(None);
    };
    if !is_single_path_component(&id)
        || !is_single_path_component(&bundle_id)
        || bundle_display_name.trim().is_empty()
        || expires_at <= created_at
        || !matches!(status.as_str(), "pending" | "consumed")
    {
        return Err(StorageError::InvalidBatchMountPlan);
    }
    let items = read_batch_mount_plan_items_from(connection, data_root, &id, &bundle_id)?;
    if items.is_empty() {
        return Err(StorageError::InvalidBatchMountPlan);
    }
    Ok(Some(StoredBatchMountPlan {
        id,
        bundle_id,
        bundle_display_name,
        created_at,
        expires_at,
        status,
        items,
    }))
}

fn read_batch_mount_plan_items_from(
    connection: &Connection,
    data_root: &Path,
    plan_id: &str,
    plan_bundle_id: &str,
) -> Result<Vec<StoredBatchMountPlanItem>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT item.id, item.mount_id, item.member_id, item.bundle_id,
                    member.skill_name, item.app_id, item.scope, item.project_id,
                    project.display_name, item.project_root_path, item.project_root_device,
                    item.project_root_inode, item.target_path, item.expected_target,
                    item.member_fingerprint, item.target_observation, item.disposition,
                    item.selectable, item.default_selected, item.conflict_reason,
                    item.target_health,
                    EXISTS(
                        SELECT 1 FROM batch_mount_transaction_items AS selected
                        WHERE selected.plan_id = item.plan_id AND selected.item_id = item.id
                    ) AS selected
             FROM batch_mount_plan_items AS item
             JOIN skill_members AS member
               ON member.id = item.member_id AND member.bundle_id = item.bundle_id
             LEFT JOIN projects AS project ON project.id = item.project_id
             WHERE item.plan_id = ?1
             ORDER BY item.sort_order",
        )
        .map_err(StorageError::ReadBatchMountPlan)?;
    let rows = statement
        .query_map([plan_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, Option<i64>>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, String>(14)?,
                row.get::<_, String>(15)?,
                row.get::<_, String>(16)?,
                row.get::<_, i64>(17)?,
                row.get::<_, i64>(18)?,
                row.get::<_, Option<String>>(19)?,
                row.get::<_, String>(20)?,
                row.get::<_, i64>(21)?,
            ))
        })
        .map_err(StorageError::ReadBatchMountPlan)?;
    let mut items = Vec::new();
    for row in rows {
        let (
            id,
            mount_id,
            member_id,
            bundle_id,
            skill_name,
            app_id,
            scope,
            project_id,
            project_display_name,
            project_root_path,
            project_root_device,
            project_root_inode,
            target_path,
            expected_target,
            member_fingerprint,
            target_observation,
            disposition,
            selectable,
            default_selected,
            conflict_reason,
            target_health,
            selected,
        ) = row.map_err(StorageError::ReadBatchMountPlan)?;
        let app_id = SupportedAppId::from_str(&app_id)
            .ok_or_else(|| StorageError::UnknownSupportedApp(app_id.clone()))?;
        let scope = MountScope::from_str(&scope)
            .ok_or_else(|| StorageError::UnknownMountScope(scope.clone()))?;
        let disposition = BatchMountDisposition::from_str(&disposition)
            .ok_or_else(|| StorageError::UnknownBatchMountDisposition(disposition.clone()))?;
        let target_health = MountHealth::from_str(&target_health)
            .ok_or_else(|| StorageError::UnknownMountHealth(target_health.clone()))?;
        let project_root_device = project_root_device
            .map(filesystem_identity_from_sql)
            .transpose()?;
        let project_root_inode = project_root_inode
            .map(filesystem_identity_from_sql)
            .transpose()?;
        validate_project_binding(
            scope,
            project_id.as_deref(),
            project_display_name.as_deref(),
            project_root_path.as_deref(),
            project_root_device,
            project_root_inode,
        )?;
        let selectable = sqlite_bool(selectable)?;
        let default_selected = sqlite_bool(default_selected)?;
        let selected = sqlite_bool(selected)?;
        let item = StoredBatchMountPlanItem {
            id,
            mount_id,
            member_id,
            bundle_id,
            skill_name,
            app_id,
            scope,
            project_id,
            project_display_name,
            project_root_path,
            project_root_device,
            project_root_inode,
            target_path,
            expected_target,
            member_fingerprint,
            target_observation,
            disposition,
            selectable,
            default_selected,
            selected,
            conflict_reason,
            target_health,
        };
        validate_stored_batch_mount_item_shape(&item, plan_bundle_id)?;
        let member = read_managed_member_from(connection, data_root, &item.member_id)?
            .ok_or_else(|| StorageError::ManagedMemberNotFound(item.member_id.clone()))?;
        if member.bundle_id != item.bundle_id
            || member.skill_name != item.skill_name
            || member.content_fingerprint != item.member_fingerprint
            || member.expected_target != item.expected_target
        {
            return Err(StorageError::InvalidBatchMountPlan);
        }
        items.push(item);
    }
    Ok(items)
}

fn validate_stored_batch_mount_item_shape(
    item: &StoredBatchMountPlanItem,
    plan_bundle_id: &str,
) -> Result<(), StorageError> {
    let valid_disposition = match item.disposition {
        BatchMountDisposition::Ready => {
            item.selectable
                && item.conflict_reason.is_none()
                && item.target_health != MountHealth::Conflict
        }
        BatchMountDisposition::AlreadyMounted => {
            // Drift 不改变关系已经登记这一 disposition，只影响 target_health 的展示。
            !item.selectable && item.conflict_reason.is_none()
        }
        BatchMountDisposition::PathConflict | BatchMountDisposition::ScopeConflict => {
            !item.selectable
                && item
                    .conflict_reason
                    .as_deref()
                    .is_some_and(|reason| !reason.trim().is_empty())
        }
    };
    if item.bundle_id != plan_bundle_id
        || !is_single_path_component(&item.id)
        || !is_single_path_component(&item.mount_id)
        || !is_single_path_component(&item.member_id)
        || !is_normalized_absolute_path(&item.target_path)
        || !is_normalized_absolute_path(&item.expected_target)
        || item.member_fingerprint.is_empty()
        || item.target_observation.is_empty()
        || Path::new(&item.target_path)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(item.skill_name.as_str())
        || !valid_disposition
        || (item.default_selected && !item.selectable)
        || (item.selected && !item.selectable)
    {
        return Err(StorageError::InvalidBatchMountPlan);
    }
    Ok(())
}

fn selected_batch_mount_items(
    plan: &StoredBatchMountPlan,
) -> Result<Vec<&StoredBatchMountPlanItem>, StorageError> {
    let selected = plan
        .items
        .iter()
        .filter(|item| item.selected)
        .collect::<Vec<_>>();
    if selected.is_empty()
        || selected
            .iter()
            .any(|item| !item.selectable || item.disposition != BatchMountDisposition::Ready)
    {
        return Err(StorageError::InvalidBatchMountSelection);
    }
    let mut targets = BTreeSet::new();
    let mut scopes = BTreeMap::<(String, &'static str), MountScope>::new();
    for item in &selected {
        if !targets.insert(item.target_path.as_str()) {
            return Err(StorageError::InvalidBatchMountSelection);
        }
        let key = (item.member_id.clone(), item.app_id.as_str());
        if scopes
            .insert(key, item.scope)
            .is_some_and(|scope| scope != item.scope)
        {
            return Err(StorageError::InvalidBatchMountSelection);
        }
    }
    Ok(selected)
}

fn validate_batch_mount_finalization(
    transaction: &Transaction<'_>,
    data_root: &Path,
    transaction_id: &str,
    plan: &StoredBatchMountPlan,
) -> Result<bool, StorageError> {
    let stored_plan = read_batch_mount_plan_from(transaction, data_root, &plan.id)?
        .ok_or(StorageError::BatchMountPlanNotFound)?;
    if &stored_plan != plan || plan.status != "consumed" {
        return Err(StorageError::InvalidBatchMountPlan);
    }
    let state = transaction
        .query_row(
            "SELECT plan_id, bundle_id, phase, status
             FROM batch_mount_transactions WHERE id = ?1",
            [transaction_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadBatchMountTransaction)?
        .ok_or_else(|| StorageError::BatchMountStateConflict(transaction_id.to_owned()))?;
    if state.0 != plan.id || state.1 != plan.bundle_id {
        return Err(StorageError::BatchMountStateConflict(
            transaction_id.to_owned(),
        ));
    }
    let selected_ids = read_batch_mount_transaction_item_ids(transaction, transaction_id)?;
    let plan_selected_ids = plan
        .items
        .iter()
        .filter(|item| item.selected)
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    if selected_ids != plan_selected_ids {
        return Err(StorageError::InvalidBatchMountPlan);
    }
    if state.2 == "state_committed" && state.3 == "completed" {
        return Ok(true);
    }
    if state.2 != "targets_applied" || state.3 != "in_progress" {
        return Err(StorageError::BatchMountStateConflict(
            transaction_id.to_owned(),
        ));
    }
    for item in selected_batch_mount_items(plan)? {
        validate_batch_ready_item_state(transaction, data_root, item)?;
    }
    Ok(false)
}

fn ensure_batch_mount_matches_item(
    mount: &StoredMount,
    item: &StoredBatchMountPlanItem,
) -> Result<(), StorageError> {
    if mount.id == item.mount_id
        && mount.member_id == item.member_id
        && mount.bundle_id == item.bundle_id
        && mount.member_fingerprint == item.member_fingerprint
        && mount.app_id == item.app_id
        && mount.scope == item.scope
        && mount.project_id == item.project_id
        && mount.target_path == item.target_path
        && mount.expected_target == item.expected_target
        && mount.health == MountHealth::Healthy
    {
        Ok(())
    } else {
        Err(StorageError::InvalidBatchMountPlan)
    }
}

fn read_batch_mount_transaction_item_ids(
    connection: &Connection,
    transaction_id: &str,
) -> Result<Vec<String>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT item_id FROM batch_mount_transaction_items
             WHERE transaction_id = ?1 ORDER BY sort_order",
        )
        .map_err(StorageError::ReadBatchMountTransaction)?;
    let rows = statement
        .query_map([transaction_id], |row| row.get::<_, String>(0))
        .map_err(StorageError::ReadBatchMountTransaction)?;
    let ids = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::ReadBatchMountTransaction)?;
    if ids.iter().any(|id| !is_single_path_component(id)) {
        return Err(StorageError::BatchMountStateConflict(
            transaction_id.to_owned(),
        ));
    }
    Ok(ids)
}

fn read_mount_plan_from(
    connection: &Connection,
    data_root: &Path,
    plan_id: &str,
) -> Result<Option<StoredMountPlan>, StorageError> {
    let stored = connection
        .query_row(
            "SELECT plan.id, plan.operation, plan.purpose, plan.mount_id, plan.member_id,
                    member.bundle_id, member.skill_name, plan.app_id, plan.scope,
                    plan.project_id, project.display_name, plan.project_root_path,
                    plan.project_root_device, plan.project_root_inode, plan.target_path,
                    plan.expected_target, plan.member_fingerprint, plan.target_observation,
                    plan.created_at, plan.expires_at, plan.status
             FROM mount_plans AS plan
             JOIN skill_members AS member ON member.id = plan.member_id
             LEFT JOIN projects AS project ON project.id = plan.project_id
             WHERE plan.id = ?1",
            [plan_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, String>(14)?,
                    row.get::<_, String>(15)?,
                    row.get::<_, String>(16)?,
                    row.get::<_, String>(17)?,
                    row.get::<_, i64>(18)?,
                    row.get::<_, i64>(19)?,
                    row.get::<_, String>(20)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadMountPlan)?;
    let Some((
        id,
        operation,
        purpose,
        mount_id,
        member_id,
        bundle_id,
        skill_name,
        app_id,
        scope,
        project_id,
        project_display_name,
        project_root_path,
        project_root_device,
        project_root_inode,
        target_path,
        expected_target,
        member_fingerprint,
        target_observation,
        created_at,
        expires_at,
        status,
    )) = stored
    else {
        return Ok(None);
    };
    let operation = MountOperation::from_str(&operation)
        .ok_or_else(|| StorageError::UnknownMountOperation(operation.clone()))?;
    let purpose = MountPlanPurpose::from_str(&purpose).ok_or(StorageError::InvalidMountPlan)?;
    let app_id = SupportedAppId::from_str(&app_id)
        .ok_or_else(|| StorageError::UnknownSupportedApp(app_id.clone()))?;
    let scope = MountScope::from_str(&scope)
        .ok_or_else(|| StorageError::UnknownMountScope(scope.clone()))?;
    let project_root_device = project_root_device
        .map(filesystem_identity_from_sql)
        .transpose()?;
    let project_root_inode = project_root_inode
        .map(filesystem_identity_from_sql)
        .transpose()?;
    validate_project_binding(
        scope,
        project_id.as_deref(),
        project_display_name.as_deref(),
        project_root_path.as_deref(),
        project_root_device,
        project_root_inode,
    )?;
    if !is_single_path_component(&id)
        || purpose.operation() != operation
        || !is_single_path_component(&mount_id)
        || !is_single_path_component(&member_id)
        || !is_normalized_absolute_path(&target_path)
        || !is_normalized_absolute_path(&expected_target)
        || member_fingerprint.is_empty()
        || target_observation.is_empty()
        || !matches!(status.as_str(), "pending" | "consumed")
    {
        return Err(StorageError::InvalidMountPlan);
    }
    let plan = StoredMountPlan {
        id,
        operation,
        purpose,
        mount_id,
        member_id,
        bundle_id,
        skill_name,
        app_id,
        scope,
        project_id,
        project_display_name,
        project_root_path,
        project_root_device,
        project_root_inode,
        target_path,
        expected_target,
        member_fingerprint,
        target_observation,
        created_at,
        expires_at,
        status,
    };
    let member = read_managed_member_from(connection, data_root, &plan.member_id)?
        .ok_or_else(|| StorageError::ManagedMemberNotFound(plan.member_id.clone()))?;
    if member.bundle_id != plan.bundle_id
        || member.skill_name != plan.skill_name
        || member.content_fingerprint != plan.member_fingerprint
        || member.expected_target != plan.expected_target
        || Path::new(&plan.target_path)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(plan.skill_name.as_str())
    {
        return Err(StorageError::InvalidMountPlan);
    }
    Ok(Some(plan))
}

fn stored_removal_plan_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRemovalPlan> {
    Ok(StoredRemovalPlan {
        id: row.get(0)?,
        kind: row.get(1)?,
        target_id: row.get(2)?,
        payload_json: row.get(3)?,
        payload_sha256: row.get(4)?,
        status: row.get(5)?,
        created_at: row.get(6)?,
        expires_at: row.get(7)?,
    })
}

fn read_removal_plan_from(
    connection: &Connection,
    plan_id: &str,
) -> Result<Option<StoredRemovalPlan>, StorageError> {
    connection
        .query_row(
            "SELECT id, kind, target_id, payload_json, payload_sha256,
                    status, created_at, expires_at
             FROM removal_plans WHERE id = ?1",
            [plan_id],
            stored_removal_plan_from_row,
        )
        .optional()
        .map_err(StorageError::ReadRemoval)
}

fn validate_stored_removal_plan(plan: &StoredRemovalPlan) -> Result<(), StorageError> {
    if !is_single_path_component(&plan.id)
        || !is_single_path_component(&plan.target_id)
        || !matches!(
            plan.kind.as_str(),
            "project" | "source" | "bundle" | "bundle_mounts"
        )
        || !matches!(plan.status.as_str(), "pending" | "consumed")
        || plan.payload_json.is_empty()
        || plan.payload_sha256.len() != 64
        || !plan
            .payload_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || plan.created_at < 0
        || plan.expires_at <= plan.created_at
    {
        Err(StorageError::RemovalStateConflict)
    } else {
        Ok(())
    }
}

fn stored_removal_transaction_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StoredRemovalTransaction> {
    Ok(StoredRemovalTransaction {
        id: row.get(0)?,
        plan_id: row.get(1)?,
        kind: row.get(2)?,
        target_id: row.get(3)?,
        journal_path: row.get(4)?,
        phase: row.get(5)?,
        status: row.get(6)?,
    })
}

fn ensure_removal_transaction_phase(
    connection: &Connection,
    transaction_id: &str,
    kind: &str,
    target_id: &str,
    phase: &str,
) -> Result<(), StorageError> {
    let matches = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM removal_transactions
                WHERE id = ?1 AND kind = ?2 AND target_id = ?3
                  AND phase = ?4 AND status = 'in_progress'
             )",
            params![transaction_id, kind, target_id, phase],
            |row| row.get::<_, i64>(0),
        )
        .map_err(StorageError::ReadRemoval)?;
    if matches == 1 {
        Ok(())
    } else {
        Err(StorageError::RemovalStateConflict)
    }
}

fn read_sorted_string_column(
    connection: &Connection,
    sql: &str,
    parameter: &str,
) -> Result<Vec<String>, StorageError> {
    let mut statement = connection.prepare(sql).map_err(StorageError::ReadRemoval)?;
    let rows = statement
        .query_map([parameter], |row| row.get::<_, String>(0))
        .map_err(StorageError::ReadRemoval)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::ReadRemoval)
}

fn read_mount_from(
    connection: &Connection,
    data_root: &Path,
    mount_id: &str,
) -> Result<Option<StoredMount>, StorageError> {
    let stored = connection
        .query_row(
            "SELECT mount.id, mount.member_id, member.bundle_id, member.skill_name,
                    member.content_fingerprint, mount.app_id, mount.scope, mount.project_id,
                    project.display_name, project.root_path, project.root_device,
                    project.root_inode, mount.target_path, mount.expected_target, mount.health
             FROM mounts AS mount
             JOIN skill_members AS member ON member.id = mount.member_id
             LEFT JOIN projects AS project ON project.id = mount.project_id
             WHERE mount.id = ?1",
            [mount_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, String>(14)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadMountTransaction)?;
    let Some((
        id,
        member_id,
        bundle_id,
        skill_name,
        member_fingerprint,
        app_id,
        scope,
        project_id,
        project_display_name,
        project_root_path,
        project_root_device,
        project_root_inode,
        target_path,
        expected_target,
        health,
    )) = stored
    else {
        return Ok(None);
    };
    let app_id = SupportedAppId::from_str(&app_id)
        .ok_or_else(|| StorageError::UnknownSupportedApp(app_id.clone()))?;
    let scope = MountScope::from_str(&scope)
        .ok_or_else(|| StorageError::UnknownMountScope(scope.clone()))?;
    let health = MountHealth::from_str(&health)
        .ok_or_else(|| StorageError::UnknownMountHealth(health.clone()))?;
    let project_root_device = project_root_device
        .map(filesystem_identity_from_sql)
        .transpose()?;
    let project_root_inode = project_root_inode
        .map(filesystem_identity_from_sql)
        .transpose()?;
    validate_project_binding(
        scope,
        project_id.as_deref(),
        project_display_name.as_deref(),
        project_root_path.as_deref(),
        project_root_device,
        project_root_inode,
    )?;
    let member = read_managed_member_from(connection, data_root, &member_id)?
        .ok_or_else(|| StorageError::ManagedMemberNotFound(member_id.clone()))?;
    if member.bundle_id != bundle_id
        || member.skill_name != skill_name
        || member.content_fingerprint != member_fingerprint
        || member.expected_target != expected_target
        || !is_normalized_absolute_path(&target_path)
        || Path::new(&target_path)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(skill_name.as_str())
    {
        return Err(StorageError::InvalidMountPlan);
    }
    Ok(Some(StoredMount {
        id,
        member_id,
        bundle_id,
        skill_name,
        member_fingerprint,
        app_id,
        scope,
        project_id,
        project_display_name,
        project_root_path,
        project_root_device,
        project_root_inode,
        target_path,
        expected_target,
        health,
    }))
}

fn validate_project_binding(
    scope: MountScope,
    project_id: Option<&str>,
    project_display_name: Option<&str>,
    project_root_path: Option<&str>,
    project_root_device: Option<u64>,
    project_root_inode: Option<u64>,
) -> Result<(), StorageError> {
    let valid = match scope {
        MountScope::Global => {
            project_id.is_none()
                && project_display_name.is_none()
                && project_root_path.is_none()
                && project_root_device.is_none()
                && project_root_inode.is_none()
        }
        MountScope::Project => {
            project_id.is_some_and(is_single_path_component)
                && project_display_name.is_some_and(|name| !name.trim().is_empty())
                && project_root_path.is_some_and(is_normalized_absolute_path)
                && project_root_device.is_some()
                && project_root_inode.is_some()
        }
    };
    if valid {
        Ok(())
    } else {
        Err(StorageError::InvalidMountPlan)
    }
}

fn validate_mount_finalization(
    transaction: &Transaction<'_>,
    data_root: &Path,
    transaction_id: &str,
    plan: &StoredMountPlan,
) -> Result<bool, StorageError> {
    let stored_plan = read_mount_plan_from(transaction, data_root, &plan.id)?
        .ok_or(StorageError::MountPlanNotFound)?;
    if &stored_plan != plan || plan.status != "consumed" {
        return Err(StorageError::InvalidMountPlan);
    }
    let state = transaction
        .query_row(
            "SELECT plan_id, mount_id, phase, status
             FROM mount_transactions WHERE id = ?1",
            [transaction_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadMountTransaction)?
        .ok_or_else(|| StorageError::MountStateConflict(transaction_id.to_owned()))?;
    if state.0 != plan.id || state.1 != plan.mount_id {
        return Err(StorageError::MountStateConflict(transaction_id.to_owned()));
    }
    if state.2 == "state_committed" && state.3 == "completed" {
        return Ok(true);
    }
    if state.2 != "target_applied" || state.3 != "in_progress" {
        return Err(StorageError::MountStateConflict(transaction_id.to_owned()));
    }
    validate_stored_mount_plan_state(transaction, data_root, plan)?;
    Ok(false)
}

fn ensure_mount_matches_plan(
    mount: &StoredMount,
    plan: &StoredMountPlan,
) -> Result<(), StorageError> {
    if mount.id == plan.mount_id
        && mount.member_id == plan.member_id
        && mount.member_fingerprint == plan.member_fingerprint
        && mount.app_id == plan.app_id
        && mount.scope == plan.scope
        && mount.project_id == plan.project_id
        && mount.target_path == plan.target_path
        && mount.expected_target == plan.expected_target
    {
        Ok(())
    } else {
        Err(StorageError::InvalidMountPlan)
    }
}

fn complete_mount_transaction(
    transaction: &Transaction<'_>,
    transaction_id: &str,
    plan: &StoredMountPlan,
    now: i64,
) -> Result<(), StorageError> {
    let changed = transaction
        .execute(
            "UPDATE mount_transactions
             SET phase = 'state_committed', status = 'completed', updated_at = ?4
             WHERE id = ?1 AND plan_id = ?2 AND mount_id = ?3
               AND status = 'in_progress' AND phase = 'target_applied'",
            params![transaction_id, plan.id, plan.mount_id, now],
        )
        .map_err(StorageError::SaveMountTransaction)?;
    ensure_one_mount_row(changed, transaction_id)
}

fn validate_new_manual_source(source: &NewManualSource<'_>) -> Result<(), StorageError> {
    let identity_is_valid = is_single_path_component(source.id)
        && !source.canonical_identity.trim().is_empty()
        && !source.display_name.trim().is_empty()
        && !source.catalog_marker.trim().is_empty()
        && !source.members.is_empty();
    // canonical identity 必须能从来源类型的稳定事实重建，避免同一来源被重复登记。
    let canonical_identity_is_valid = match source.kind {
        "archive" => source.canonical_identity == format!("archive:{}", source.locator),
        "direct_url" => source.canonical_identity == format!("direct-url:{}", source.locator),
        "editable_local" => source
            .filesystem_device
            .zip(source.filesystem_inode)
            .is_some_and(|(device, inode)| {
                source.canonical_identity == format!("editable-local:{device}:{inode}")
            }),
        _ => false,
    };
    let location_is_valid = match source.kind {
        "archive" => {
            is_normalized_absolute_path(source.locator)
                && source.filesystem_device.is_none()
                && source.filesystem_inode.is_none()
        }
        "direct_url" => {
            source.locator.starts_with("https://")
                && !source.locator.chars().any(char::is_whitespace)
                && source.filesystem_device.is_none()
                && source.filesystem_inode.is_none()
        }
        "editable_local" => {
            is_normalized_absolute_path(source.locator)
                && source.filesystem_device.is_some()
                && source.filesystem_inode.is_some()
        }
        _ => false,
    };
    let members_are_valid = source.members.iter().all(|member| {
        is_single_path_component(member.id)
            && (member.relative_path.is_empty()
                || is_normalized_relative_path(member.relative_path))
    });
    if identity_is_valid && canonical_identity_is_valid && location_is_valid && members_are_valid {
        Ok(())
    } else {
        Err(StorageError::InvalidSourceDefinition)
    }
}

fn validate_new_install_plan_contract(plan: &NewInstallPlan<'_>) -> Result<(), StorageError> {
    if plan.candidates.is_empty() {
        return Err(StorageError::EmptyInstallPlanCandidates);
    }
    validate_install_plan_shape(
        plan.kind,
        plan.install_mode,
        plan.input_path,
        plan.snapshot_relative_path,
        plan.source_id,
        plan.source_tracked_ref,
        plan.source_catalog_generation,
        plan.source_marker,
        plan.expected_source_marker,
        plan.expected_current_target,
        plan.expected_adopted_marker,
        plan.id,
        plan.bundle_id,
        plan.bundle_display_name,
    )?;
    if plan.candidates.iter().any(|candidate| {
        !valid_install_candidate_shape(
            plan.kind,
            plan.install_mode,
            candidate.candidate_id,
            candidate.source_relative_path,
            candidate.skill_name,
            candidate.skill_description,
            candidate.content_fingerprint,
            candidate.previous_content_fingerprint,
            candidate.selectable,
            candidate.preserve_existing,
            candidate.default_selected,
            candidate.default_selected,
        )
    }) {
        return Err(StorageError::InvalidInstallPlan);
    }
    Ok(())
}

fn validate_stored_install_plan_contract(plan: &StoredInstallPlan) -> Result<(), StorageError> {
    validate_install_plan_shape(
        &plan.kind,
        &plan.install_mode,
        plan.input_path.as_deref(),
        plan.snapshot_relative_path.as_deref(),
        plan.source_id.as_deref(),
        plan.source_tracked_ref.as_deref(),
        plan.source_catalog_generation,
        plan.source_marker.as_deref(),
        plan.expected_source_marker.as_deref(),
        plan.expected_current_target.as_deref(),
        plan.expected_adopted_marker.as_deref(),
        &plan.id,
        &plan.bundle_id,
        &plan.bundle_display_name,
    )?;
    if !matches!(plan.status.as_str(), "pending" | "consumed")
        || plan.candidates.is_empty()
        || plan.candidates.iter().any(|candidate| {
            !valid_install_candidate_shape(
                &plan.kind,
                &plan.install_mode,
                &candidate.candidate_id,
                candidate.source_relative_path.as_deref(),
                candidate.skill_name.as_deref(),
                candidate.skill_description.as_deref(),
                candidate.content_fingerprint.as_deref(),
                candidate.previous_content_fingerprint.as_deref(),
                candidate.selectable,
                candidate.preserve_existing,
                candidate.default_selected,
                candidate.selected,
            )
        })
    {
        return Err(StorageError::InvalidInstallPlan);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_install_plan_shape(
    kind: &str,
    install_mode: &str,
    input_path: Option<&str>,
    snapshot_relative_path: Option<&str>,
    source_id: Option<&str>,
    source_tracked_ref: Option<&str>,
    source_catalog_generation: Option<i64>,
    source_marker: Option<&str>,
    expected_source_marker: Option<&str>,
    expected_current_target: Option<&str>,
    expected_adopted_marker: Option<&str>,
    plan_id: &str,
    bundle_id: &str,
    bundle_display_name: &str,
) -> Result<(), StorageError> {
    let common_is_valid = is_single_path_component(plan_id)
        && is_single_path_component(bundle_id)
        && !bundle_display_name.trim().is_empty();
    let source_values_are_absent = source_id.is_none()
        && source_tracked_ref.is_none()
        && source_catalog_generation.is_none()
        && source_marker.is_none()
        && expected_source_marker.is_none()
        && expected_current_target.is_none()
        && expected_adopted_marker.is_none();
    let folder_is_valid = kind == "folder_snapshot"
        && install_mode == "create"
        && input_path.is_some_and(is_normalized_absolute_path)
        && snapshot_relative_path.is_none()
        && source_values_are_absent;
    let source_snapshot_is_valid = source_id.is_some_and(is_single_path_component)
        && source_tracked_ref.is_none_or(|value| !value.is_empty())
        && source_catalog_generation.is_some_and(|generation| generation > 0)
        && source_marker.is_some_and(|value| !value.is_empty())
        && snapshot_relative_path.is_some_and(is_normalized_relative_path)
        && input_path.is_none();
    let source_mode_is_valid = match install_mode {
        "create" => {
            expected_source_marker.is_none()
                && expected_current_target.is_none()
                && expected_adopted_marker.is_none()
        }
        "supplement" => {
            expected_source_marker.is_none()
                && expected_current_target.is_some_and(is_safe_current_target)
                && expected_adopted_marker.is_none_or(|value| !value.is_empty())
        }
        "update" => {
            source_tracked_ref.is_none_or(|value| !value.is_empty())
                && expected_source_marker.is_some_and(|value| !value.is_empty())
                && expected_current_target.is_some_and(is_safe_current_target)
                && expected_adopted_marker.is_none_or(|value| !value.is_empty())
        }
        _ => false,
    };
    if common_is_valid
        && (folder_is_valid
            || (kind == "source_snapshot" && source_snapshot_is_valid && source_mode_is_valid))
    {
        Ok(())
    } else {
        Err(StorageError::InvalidInstallPlan)
    }
}

#[allow(clippy::too_many_arguments)]
fn valid_install_candidate_shape(
    plan_kind: &str,
    install_mode: &str,
    candidate_id: &str,
    source_relative_path: Option<&str>,
    skill_name: Option<&str>,
    skill_description: Option<&str>,
    content_fingerprint: Option<&str>,
    previous_content_fingerprint: Option<&str>,
    selectable: bool,
    preserve_existing: bool,
    default_selected: bool,
    selected: bool,
) -> bool {
    let metadata_is_complete = skill_name.is_some_and(is_single_path_component)
        && skill_description.is_some()
        && content_fingerprint.is_some_and(|value| !value.is_empty());
    let source_path_is_valid = source_relative_path
        .is_some_and(|path| path.is_empty() || is_normalized_relative_path(path))
        || (source_relative_path.is_none() && preserve_existing);
    let previous_fingerprint_is_valid = match install_mode {
        "create" => previous_content_fingerprint.is_none(),
        "supplement" => {
            if preserve_existing {
                previous_content_fingerprint == content_fingerprint
            } else {
                previous_content_fingerprint.is_none()
            }
        }
        "update" => previous_content_fingerprint.is_none_or(|value| !value.is_empty()),
        _ => false,
    };
    is_single_path_component(candidate_id)
        && source_path_is_valid
        && previous_fingerprint_is_valid
        && (!selectable || metadata_is_complete)
        && (!preserve_existing
            || (plan_kind == "source_snapshot"
                && matches!(install_mode, "supplement" | "update")
                && !selectable
                && default_selected
                && selected
                && metadata_is_complete))
        && (!default_selected || selectable || preserve_existing)
        && (!selected || selectable || preserve_existing)
}

fn validate_install_plan_source_contract(
    connection: &Connection,
    plan: &StoredInstallPlan,
) -> Result<(), StorageError> {
    if plan.kind == "folder_snapshot" {
        return if plan
            .candidates
            .iter()
            .all(|candidate| !candidate.preserve_existing)
        {
            Ok(())
        } else {
            Err(StorageError::InvalidInstallPlan)
        };
    }

    let source_id = plan
        .source_id
        .as_deref()
        .ok_or(StorageError::InvalidInstallPlan)?;
    let source = read_source_install_source_from(connection, source_id)?;
    let source_marker_matches = if plan.install_mode == "update" {
        plan.expected_source_marker.as_deref() == Some(source.catalog_marker.as_str())
    } else {
        plan.source_marker.as_deref() == Some(source.catalog_marker.as_str())
    };
    if plan.source_tracked_ref != source.tracked_ref
        || plan.source_catalog_generation != Some(source.catalog_generation)
        || !source_marker_matches
    {
        return Err(StorageError::SourceCatalogStateChanged);
    }

    let bundle = match plan.install_mode.as_str() {
        "create" => {
            let bundle_exists = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM bundles WHERE id = ?1)",
                    [&plan.bundle_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(StorageError::ReadSources)?;
            if source.bundle.is_some() || bundle_exists {
                return Err(StorageError::SourceBundleStateConflict);
            }
            None
        }
        "supplement" => {
            let bundle = source
                .bundle
                .as_ref()
                .ok_or(StorageError::SourceBundleStateConflict)?;
            if bundle.id != plan.bundle_id
                || bundle.display_name != plan.bundle_display_name
                || Some(bundle.current_target.as_str()) != plan.expected_current_target.as_deref()
                || bundle.adopted_marker != plan.expected_adopted_marker
            {
                return Err(StorageError::SourceBundleStateConflict);
            }
            validate_preserved_install_members(plan, bundle)?;
            Some(bundle)
        }
        "update" => {
            let bundle = source
                .bundle
                .as_ref()
                .ok_or(StorageError::SourceBundleStateConflict)?;
            let source_kind = SourceKind::from_str(&source.kind)
                .ok_or_else(|| StorageError::UnknownSourceKind(source.kind.clone()))?;
            let editable_check_is_current = source_kind != SourceKind::EditableLocal
                || (bundle.update_check_status == BundleUpdateStatus::Available
                    && bundle.update_checked_marker == plan.source_marker
                    && bundle.update_checked_at.is_some()
                    && plan.source_marker == plan.expected_source_marker);
            if !editable_check_is_current
                || bundle.id != plan.bundle_id
                || bundle.display_name != plan.bundle_display_name
                || Some(bundle.current_target.as_str()) != plan.expected_current_target.as_deref()
                || bundle.adopted_marker != plan.expected_adopted_marker
            {
                return Err(StorageError::SourceBundleStateConflict);
            }
            validate_update_install_members(plan, bundle)?;
            return Ok(());
        }
        _ => return Err(StorageError::InvalidInstallPlan),
    };
    validate_source_install_candidates(plan, &source.catalog_members, bundle)
}

/// Update 的候选同时描述新 Source 内容和旧 current 校验，且每个旧成员只能出现一次。
fn validate_update_install_members(
    plan: &StoredInstallPlan,
    bundle: &StoredSourceInstallBundle,
) -> Result<(), StorageError> {
    let previous = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.previous_content_fingerprint.is_some())
        .map(|candidate| (candidate.candidate_id.as_str(), candidate))
        .collect::<BTreeMap<_, _>>();
    if previous.len() != bundle.members.len() {
        return Err(StorageError::SourceBundleStateConflict);
    }
    for member in &bundle.members {
        let candidate = previous
            .get(member.id.as_str())
            .ok_or(StorageError::SourceBundleStateConflict)?;
        if candidate.skill_name.as_deref() != Some(member.skill_name.as_str())
            || candidate.previous_content_fingerprint.as_deref()
                != Some(member.content_fingerprint.as_str())
            || (candidate.preserve_existing
                && (candidate.skill_description.as_deref() != Some(member.description.as_str())
                    || candidate.content_fingerprint.as_deref()
                        != Some(member.content_fingerprint.as_str())))
            || (!candidate.preserve_existing
                && candidate.source_relative_path != member.source_relative_path)
        {
            return Err(StorageError::SourceBundleStateConflict);
        }
    }

    let current = plan
        .candidates
        .iter()
        .filter(|candidate| !candidate.preserve_existing)
        .collect::<Vec<_>>();
    if current.is_empty()
        || current
            .iter()
            .any(|candidate| candidate.source_relative_path.is_none())
    {
        return Err(StorageError::InvalidInstallPlan);
    }
    Ok(())
}

fn validate_preserved_install_members(
    plan: &StoredInstallPlan,
    bundle: &StoredSourceInstallBundle,
) -> Result<(), StorageError> {
    let preserved = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.preserve_existing)
        .map(|candidate| (candidate.candidate_id.as_str(), candidate))
        .collect::<BTreeMap<_, _>>();
    if preserved.len() != bundle.members.len() {
        return Err(StorageError::SourceBundleStateConflict);
    }
    for member in &bundle.members {
        let candidate = preserved
            .get(member.id.as_str())
            .ok_or(StorageError::SourceBundleStateConflict)?;
        if candidate.source_relative_path != member.source_relative_path
            || candidate.skill_name.as_deref() != Some(member.skill_name.as_str())
            || candidate.skill_description.as_deref() != Some(member.description.as_str())
            || candidate.content_fingerprint.as_deref() != Some(member.content_fingerprint.as_str())
            || candidate.selectable
            || !candidate.default_selected
            || !candidate.selected
        {
            return Err(StorageError::SourceBundleStateConflict);
        }
    }
    Ok(())
}

fn validate_source_install_candidates(
    plan: &StoredInstallPlan,
    catalog_members: &[StoredSourceInstallCatalogMember],
    bundle: Option<&StoredSourceInstallBundle>,
) -> Result<(), StorageError> {
    let existing_paths = bundle
        .into_iter()
        .flat_map(|bundle| bundle.members.iter())
        .filter_map(|member| member.source_relative_path.as_deref())
        .collect::<BTreeSet<_>>();
    let candidates = plan
        .candidates
        .iter()
        .filter(|candidate| !candidate.preserve_existing)
        .map(|candidate| {
            candidate
                .source_relative_path
                .as_deref()
                .map(|path| (path, candidate))
                .ok_or(StorageError::InvalidInstallPlan)
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let expected = catalog_members
        .iter()
        .filter(|member| !existing_paths.contains(member.relative_path.as_str()))
        .collect::<Vec<_>>();
    if candidates.len() != expected.len() {
        return Err(StorageError::InvalidInstallPlan);
    }
    for member in expected {
        let candidate = candidates
            .get(member.relative_path.as_str())
            .ok_or(StorageError::InvalidInstallPlan)?;
        if candidate.candidate_id != member.id
            || candidate.skill_name != member.skill_name
            || candidate.skill_description != member.description
            || candidate.content_fingerprint != member.content_fingerprint
            || candidate.selectable != member.selectable
            || candidate.validation_errors != member.validation_errors
            || candidate.warnings != member.warnings
            || candidate.default_selected != member.selectable
        {
            return Err(StorageError::InvalidInstallPlan);
        }
    }
    Ok(())
}

fn validate_managed_install_paths(
    transaction_id: &str,
    plan: &StoredInstallPlan,
    selected: &[&StoredInstallCandidate],
    managed_directory: &str,
    current_target: &str,
    stable_relative_path: &str,
) -> Result<(), StorageError> {
    let anchor_skill_name = selected
        .first()
        .and_then(|candidate| candidate.skill_name.as_deref())
        .ok_or(StorageError::InvalidInstallSelection)?;
    let members_are_safe = selected.iter().all(|candidate| {
        (candidate.selectable || candidate.preserve_existing)
            && candidate
                .skill_name
                .as_deref()
                .is_some_and(is_single_path_component)
            && candidate.skill_description.is_some()
            && candidate.content_fingerprint.is_some()
            && is_single_path_component(&candidate.candidate_id)
    });
    if !is_single_path_component(transaction_id)
        || !is_single_path_component(&plan.bundle_id)
        || !members_are_safe
        || managed_directory != format!("bundles/{}", plan.bundle_id)
        || current_target != format!("contents/{transaction_id}")
        || stable_relative_path != format!("members/{anchor_skill_name}")
    {
        return Err(StorageError::UnsafeManagedPath(
            managed_directory.to_owned(),
        ));
    }
    Ok(())
}

fn selected_install_candidates(
    plan: &StoredInstallPlan,
) -> Result<Vec<&StoredInstallCandidate>, StorageError> {
    let selected = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.selected)
        .collect::<Vec<_>>();
    if selected.is_empty()
        || selected
            .iter()
            .any(|candidate| !candidate.selectable && !candidate.preserve_existing)
        || !selected
            .iter()
            .any(|candidate| candidate.selectable && !candidate.preserve_existing)
    {
        Err(StorageError::InvalidInstallSelection)
    } else {
        Ok(selected)
    }
}

fn validate_stored_managed_paths(
    bundle_id: &str,
    skill_name: &str,
    managed_directory: &str,
    current_target: &str,
    stable_relative_path: &str,
) -> Result<(), StorageError> {
    if !is_single_path_component(bundle_id)
        || !is_single_path_component(skill_name)
        || managed_directory != format!("bundles/{bundle_id}")
        || stable_relative_path != format!("members/{skill_name}")
        || !is_safe_current_target(current_target)
    {
        return Err(StorageError::UnsafeManagedPath(
            managed_directory.to_owned(),
        ));
    }
    Ok(())
}

fn is_safe_current_target(value: &str) -> bool {
    let mut components = Path::new(value).components();
    let prefix_is_contents = matches!(
        components.next(),
        Some(std::path::Component::Normal(prefix))
            if prefix == std::ffi::OsStr::new("contents")
    );
    let content_id_is_safe = matches!(
        components.next(),
        Some(std::path::Component::Normal(content_id))
            if is_single_path_component(content_id.to_string_lossy().as_ref())
    );
    prefix_is_contents && content_id_is_safe && components.next().is_none()
}

fn is_single_path_component(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

fn stored_source_association_transaction_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StoredSourceAssociationTransaction> {
    Ok(StoredSourceAssociationTransaction {
        id: row.get(0)?,
        plan_id: row.get(1)?,
        source_id: row.get(2)?,
        target_bundle_id: row.get(3)?,
        retiring_bundle_id: row.get(4)?,
        content_choices_json: row.get(5)?,
        source_mappings_json: row.get(6)?,
        journal_path: row.get(7)?,
        phase: row.get(8)?,
        status: row.get(9)?,
    })
}

fn validate_stored_source_association_transaction(
    transaction: &StoredSourceAssociationTransaction,
) -> Result<(), StorageError> {
    let stored_mappings = serde_json::from_str::<Vec<StoredSourceAssociationMemberMapping>>(
        &transaction.source_mappings_json,
    );
    let mappings_are_canonical = stored_mappings.is_ok_and(|mappings| {
        canonical_stored_source_association_mappings_json(mappings)
            .is_ok_and(|canonical| canonical == transaction.source_mappings_json)
    });
    if !is_single_path_component(&transaction.id)
        || !is_single_path_component(&transaction.plan_id)
        || !is_single_path_component(&transaction.source_id)
        || !is_single_path_component(&transaction.target_bundle_id)
        || !is_single_path_component(&transaction.retiring_bundle_id)
        || transaction.target_bundle_id == transaction.retiring_bundle_id
        || serde_json::from_str::<Vec<serde_json::Value>>(&transaction.content_choices_json)
            .is_err()
        || !mappings_are_canonical
        || !is_normalized_relative_path(&transaction.journal_path)
        || !source_association_phase_and_status_are_consistent(
            &transaction.phase,
            &transaction.status,
        )
    {
        return Err(StorageError::SourceAssociationStateConflict(
            transaction.id.clone(),
        ));
    }
    Ok(())
}

fn raw_takeover_transaction_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RawStoredTakeoverTransaction> {
    Ok(RawStoredTakeoverTransaction {
        id: row.get(0)?,
        plan_id: row.get(1)?,
        bundle_id: row.get(2)?,
        member_id: row.get(3)?,
        reserved_paths_json: row.get(4)?,
        journal_path: row.get(5)?,
        phase: row.get(6)?,
        status: row.get(7)?,
    })
}

fn decode_takeover_transaction(
    raw: RawStoredTakeoverTransaction,
) -> Result<StoredTakeoverTransaction, StorageError> {
    let reserved_paths = decode_takeover_reserved_paths(&raw.id, &raw.reserved_paths_json)?;
    Ok(StoredTakeoverTransaction {
        id: raw.id,
        plan_id: raw.plan_id,
        bundle_id: raw.bundle_id,
        member_id: raw.member_id,
        reserved_paths,
        journal_path: raw.journal_path,
        phase: raw.phase,
        status: raw.status,
    })
}

fn decode_takeover_reserved_paths(
    transaction_id: &str,
    reserved_paths_json: &str,
) -> Result<Vec<String>, StorageError> {
    let reserved_paths = serde_json::from_str::<Vec<String>>(reserved_paths_json)
        .map_err(|_| StorageError::TakeoverStateConflict(transaction_id.to_owned()))?;
    if !takeover_reserved_paths_are_valid(&reserved_paths) {
        return Err(StorageError::TakeoverStateConflict(
            transaction_id.to_owned(),
        ));
    }
    Ok(reserved_paths)
}

fn takeover_reserved_paths_are_valid(reserved_paths: &[String]) -> bool {
    !reserved_paths.is_empty()
        && reserved_paths
            .iter()
            .all(|path| is_normalized_absolute_path(path))
        && reserved_paths.windows(2).all(|pair| pair[0] < pair[1])
}

pub(crate) fn takeover_reserved_paths(plan: &TakeoverPlan) -> Result<Vec<String>, StorageError> {
    // 来源和 Host 目标共同组成 Takeover 期间不可被其他生命周期操作占用的路径集合。
    let reserved_paths = plan
        .origins
        .iter()
        .map(|origin| origin.original_path.clone())
        .chain(plan.targets.iter().map(|target| target.target_path.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !takeover_reserved_paths_are_valid(&reserved_paths) {
        return Err(StorageError::InvalidTakeoverPlan);
    }
    Ok(reserved_paths)
}

fn read_takeover_plan_from(
    connection: &Connection,
    plan_id: &str,
) -> Result<Option<StoredTakeoverPlanRow>, StorageError> {
    let plan = connection
        .query_row(
            "SELECT id, payload_json, payload_sha256, status, created_at, expires_at
             FROM takeover_plans WHERE id = ?1",
            [plan_id],
            |row| {
                Ok(StoredTakeoverPlanRow {
                    id: row.get(0)?,
                    payload_json: row.get(1)?,
                    payload_sha256: row.get(2)?,
                    status: row.get(3)?,
                    created_at: row.get(4)?,
                    expires_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(StorageError::ReadTakeoverPlan)?;
    if let Some(plan) = &plan
        && (plan.id != plan_id
            || plan.payload_json.is_empty()
            || plan.payload_sha256.len() != 64
            || !plan
                .payload_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || !matches!(plan.status.as_str(), "pending" | "consumed")
            || plan.created_at < 0
            || plan.expires_at < plan.created_at)
    {
        return Err(StorageError::InvalidTakeoverPlan);
    }
    Ok(plan)
}

fn read_source_association_plan_from(
    connection: &Connection,
    plan_id: &str,
) -> Result<Option<StoredSourceAssociationPlanRow>, StorageError> {
    let plan = connection
        .query_row(
            "SELECT id, payload_json, payload_sha256, status, created_at, expires_at
             FROM source_association_plans
             WHERE id = ?1",
            [plan_id],
            |row| {
                Ok(StoredSourceAssociationPlanRow {
                    id: row.get(0)?,
                    payload_json: row.get(1)?,
                    payload_sha256: row.get(2)?,
                    status: row.get(3)?,
                    created_at: row.get(4)?,
                    expires_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(StorageError::ReadSourceAssociationPlan)?;
    if let Some(plan) = &plan {
        validate_source_association_plan_row(plan)?;
        if plan.id != plan_id {
            return Err(StorageError::InvalidSourceAssociationPlan);
        }
    }
    Ok(plan)
}

fn validate_source_association_plan_row(
    plan: &StoredSourceAssociationPlanRow,
) -> Result<(), StorageError> {
    let hash_is_valid = plan.payload_sha256.len() == 64
        && plan
            .payload_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    if is_single_path_component(&plan.id)
        && !plan.payload_json.is_empty()
        && hash_is_valid
        && matches!(plan.status.as_str(), "pending" | "consumed")
        && plan.created_at >= 0
        && plan.expires_at >= plan.created_at
    {
        Ok(())
    } else {
        Err(StorageError::InvalidSourceAssociationPlan)
    }
}

fn ensure_source_association_plan_is_confirmable(
    plan: &StoredSourceAssociationPlanRow,
    now: i64,
) -> Result<(), StorageError> {
    if plan.status != "pending" {
        Err(StorageError::SourceAssociationPlanConsumed)
    } else if now < 0 {
        Err(StorageError::InvalidSourceAssociationPlan)
    } else if plan.expires_at <= now {
        Err(StorageError::SourceAssociationPlanExpired)
    } else {
        Ok(())
    }
}

fn validate_direct_source_association_input(
    association: &DirectSourceAssociation<'_>,
) -> Result<(), StorageError> {
    let expected_member_ids = association
        .expected_members
        .iter()
        .map(|member| member.member_id)
        .collect::<BTreeSet<_>>();
    let mapping_member_ids = association
        .member_mappings
        .iter()
        .map(|mapping| mapping.member_id)
        .collect::<BTreeSet<_>>();
    let mapping_source_paths = association
        .member_mappings
        .iter()
        .map(|mapping| mapping.source_relative_path)
        .collect::<BTreeSet<_>>();
    let expected_members_are_valid = !association.expected_members.is_empty()
        && expected_member_ids.len() == association.expected_members.len()
        && association.expected_members.iter().all(|member| {
            is_single_path_component(member.member_id) && !member.content_fingerprint.is_empty()
        });
    let mappings_are_valid = mapping_member_ids.len() == association.member_mappings.len()
        && mapping_source_paths.len() == association.member_mappings.len()
        && association.member_mappings.iter().all(|mapping| {
            expected_member_ids.contains(mapping.member_id)
                && is_single_path_component(mapping.member_id)
                && (mapping.source_relative_path.is_empty()
                    || is_normalized_relative_path(mapping.source_relative_path))
        });
    if is_single_path_component(association.plan_id)
        && is_single_path_component(association.source_id)
        && association.source_catalog_generation > 0
        && !association.source_marker.is_empty()
        && is_single_path_component(association.bundle_id)
        && is_safe_current_target(association.expected_current_target)
        && association.now >= 0
        && expected_members_are_valid
        && mappings_are_valid
    {
        Ok(())
    } else {
        Err(StorageError::InvalidSourceAssociationPlan)
    }
}

fn ensure_direct_source_association_snapshot_matches(
    association: &DirectSourceAssociation<'_>,
    bundle: &StoredSourceAssociationBundle,
) -> Result<(), StorageError> {
    let expected = association
        .expected_members
        .iter()
        .map(|member| (member.member_id, member.content_fingerprint))
        .collect::<BTreeMap<_, _>>();
    let actual = bundle
        .members
        .iter()
        .map(|member| (member.id.as_str(), member.content_fingerprint.as_str()))
        .collect::<BTreeMap<_, _>>();
    if bundle.id == association.bundle_id
        && bundle.current_target == association.expected_current_target
        && bundle.source_id.is_none()
        && bundle.adopted_marker.is_none()
        && actual == expected
    {
        Ok(())
    } else {
        Err(StorageError::SourceBundleStateConflict)
    }
}

fn validate_direct_source_association_mappings(
    transaction: &Transaction<'_>,
    association: &DirectSourceAssociation<'_>,
) -> Result<(), StorageError> {
    for mapping in association.member_mappings {
        let selectable = transaction
            .query_row(
                "SELECT selectable
                 FROM source_catalog_members
                 WHERE source_id = ?1
                   AND catalog_generation = ?2
                   AND relative_path = ?3",
                params![
                    association.source_id,
                    association.source_catalog_generation,
                    mapping.source_relative_path
                ],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(StorageError::ReadSources)?;
        if selectable.map(sqlite_bool).transpose()? != Some(true) {
            return Err(StorageError::SourceCatalogStateChanged);
        }
    }
    Ok(())
}

fn canonical_source_association_mappings_json(
    mappings: &[FinalSourceAssociationMemberMapping<'_>],
    allowed_member_ids: &BTreeSet<&str>,
) -> Result<String, StorageError> {
    if mappings
        .iter()
        .any(|mapping| !allowed_member_ids.contains(mapping.member_id))
    {
        return Err(StorageError::InvalidSourceAssociationPlan);
    }
    canonical_stored_source_association_mappings_json(
        mappings
            .iter()
            .map(|mapping| StoredSourceAssociationMemberMapping {
                source_relative_path: mapping.source_relative_path.to_owned(),
                member_id: mapping.member_id.to_owned(),
            })
            .collect(),
    )
}

fn canonical_stored_source_association_mappings_json(
    mut mappings: Vec<StoredSourceAssociationMemberMapping>,
) -> Result<String, StorageError> {
    let source_paths = mappings
        .iter()
        .map(|mapping| mapping.source_relative_path.as_str())
        .collect::<BTreeSet<_>>();
    let member_ids = mappings
        .iter()
        .map(|mapping| mapping.member_id.as_str())
        .collect::<BTreeSet<_>>();
    if source_paths.len() != mappings.len()
        || member_ids.len() != mappings.len()
        || mappings.iter().any(|mapping| {
            !is_single_path_component(&mapping.member_id)
                || (!mapping.source_relative_path.is_empty()
                    && !is_normalized_relative_path(&mapping.source_relative_path))
        })
    {
        return Err(StorageError::InvalidSourceAssociationPlan);
    }
    mappings.sort();
    serde_json::to_string(&mappings).map_err(|_| StorageError::InvalidSourceAssociationPlan)
}

fn validate_source_association_mappings_for_generation(
    transaction: &Transaction<'_>,
    source_id: &str,
    source_catalog_generation: i64,
    mappings: &[FinalSourceAssociationMemberMapping<'_>],
) -> Result<(), StorageError> {
    for mapping in mappings {
        let selectable = transaction
            .query_row(
                "SELECT selectable
                 FROM source_catalog_members
                 WHERE source_id = ?1
                   AND catalog_generation = ?2
                   AND relative_path = ?3",
                params![
                    source_id,
                    source_catalog_generation,
                    mapping.source_relative_path
                ],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(StorageError::ReadSources)?;
        if selectable.map(sqlite_bool).transpose()? != Some(true) {
            return Err(StorageError::SourceCatalogStateChanged);
        }
    }
    Ok(())
}

fn ensure_source_association_merge_relationships(
    connection: &Connection,
    source_id: &str,
    target_bundle_id: &str,
    retiring_bundle_id: &str,
) -> Result<(), StorageError> {
    let linked_bundle = connection
        .query_row(
            "SELECT bundle_id FROM source_bundle_links WHERE source_id = ?1",
            [source_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(StorageError::ReadSources)?;
    let retiring_exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM bundles WHERE id = ?1)",
            [retiring_bundle_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StorageError::ReadSources)?;
    let retiring_is_linked = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM source_bundle_links WHERE bundle_id = ?1
             )",
            [retiring_bundle_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StorageError::ReadSources)?;
    if linked_bundle.as_deref() == Some(target_bundle_id)
        && target_bundle_id != retiring_bundle_id
        && retiring_exists
        && !retiring_is_linked
    {
        Ok(())
    } else {
        Err(StorageError::SourceBundleStateConflict)
    }
}

fn validate_final_source_association_merge(
    merge: &FinalSourceAssociationMerge<'_>,
) -> Result<(), StorageError> {
    let target = merge.expected_target_bundle;
    let retiring = merge.expected_retiring_bundle;
    let original_members = target
        .members
        .iter()
        .chain(retiring.members.iter())
        .map(|member| (member.id.as_str(), member))
        .collect::<BTreeMap<_, _>>();
    let final_member_ids = merge
        .final_members
        .iter()
        .map(|member| member.member_id)
        .collect::<BTreeSet<_>>();
    let final_members_by_id = merge
        .final_members
        .iter()
        .map(|member| (member.member_id, member))
        .collect::<BTreeMap<_, _>>();
    let final_names = merge
        .final_members
        .iter()
        .map(|member| member.skill_name)
        .collect::<BTreeSet<_>>();
    let final_paths = merge
        .final_members
        .iter()
        .map(|member| member.stable_relative_path)
        .collect::<BTreeSet<_>>();
    let final_members_are_valid = !merge.final_members.is_empty()
        && final_member_ids.len() == merge.final_members.len()
        && final_names.len() == merge.final_members.len()
        && final_paths.len() == merge.final_members.len()
        && merge.final_members.iter().all(|member| {
            original_members
                .get(member.member_id)
                .is_some_and(|original| {
                    member.skill_name == original.skill_name
                        && member.description == original.description
                        && member.stable_relative_path == original.stable_relative_path
                        && member.content_fingerprint == original.content_fingerprint
                        && is_single_path_component(member.member_id)
                        && is_single_path_component(member.skill_name)
                        && member.stable_relative_path == format!("members/{}", member.skill_name)
                })
        });

    let original_mounts = original_members
        .values()
        .flat_map(|member| member.mounts.iter())
        .map(|mount| (mount.id.as_str(), mount))
        .collect::<BTreeMap<_, _>>();
    let original_mount_ids = original_mounts.keys().copied().collect::<BTreeSet<_>>();
    let assigned_mount_ids = merge
        .mount_assignments
        .iter()
        .map(|assignment| assignment.mount_id)
        .collect::<BTreeSet<_>>();
    let mut assignments_are_valid = original_mount_ids.len() == merge.mount_assignments.len()
        && assigned_mount_ids == original_mount_ids
        && merge.mount_assignments.iter().all(|assignment| {
            is_single_path_component(assignment.mount_id)
                && final_member_ids.contains(assignment.member_id)
        });
    let mut global_assignments = BTreeSet::<(&str, &'static str)>::new();
    let mut project_assignments = BTreeSet::<(&str, &'static str, &str)>::new();
    let mut assigned_scopes = BTreeMap::<(&str, &'static str), MountScope>::new();
    for assignment in merge.mount_assignments {
        let Some(mount) = original_mounts.get(assignment.mount_id) else {
            assignments_are_valid = false;
            continue;
        };
        let Some(final_member) = final_members_by_id.get(assignment.member_id) else {
            assignments_are_valid = false;
            continue;
        };
        if Path::new(&mount.target_path)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(final_member.skill_name)
        {
            assignments_are_valid = false;
        }
        let key = (assignment.member_id, mount.app_id.as_str());
        match mount.scope {
            MountScope::Global => {
                if !global_assignments.insert(key)
                    || assigned_scopes
                        .get(&key)
                        .is_some_and(|scope| *scope == MountScope::Project)
                {
                    assignments_are_valid = false;
                }
                assigned_scopes.entry(key).or_insert(MountScope::Global);
            }
            MountScope::Project => {
                let Some(project_id) = mount.project_id.as_deref() else {
                    assignments_are_valid = false;
                    continue;
                };
                if !project_assignments.insert((key.0, key.1, project_id))
                    || assigned_scopes
                        .get(&key)
                        .is_some_and(|scope| *scope == MountScope::Global)
                {
                    assignments_are_valid = false;
                }
                assigned_scopes.entry(key).or_insert(MountScope::Project);
            }
        }
    }

    let mapping_paths = merge
        .source_mappings
        .iter()
        .map(|mapping| mapping.source_relative_path)
        .collect::<BTreeSet<_>>();
    let mapping_members = merge
        .source_mappings
        .iter()
        .map(|mapping| mapping.member_id)
        .collect::<BTreeSet<_>>();
    let mappings_are_valid = mapping_paths.len() == merge.source_mappings.len()
        && mapping_members.len() == merge.source_mappings.len()
        && merge.source_mappings.iter().all(|mapping| {
            (mapping.source_relative_path.is_empty()
                || is_normalized_relative_path(mapping.source_relative_path))
                && final_member_ids.contains(mapping.member_id)
        });

    if is_single_path_component(merge.transaction_id)
        && is_single_path_component(merge.source_id)
        && target.id != retiring.id
        && target.source_id.as_deref() == Some(merge.source_id)
        && retiring.source_id.is_none()
        && retiring.adopted_marker.is_none()
        && merge.final_current_target == format!("contents/{}", merge.transaction_id)
        && merge.now >= 0
        && final_members_are_valid
        && assignments_are_valid
        && mappings_are_valid
    {
        Ok(())
    } else {
        Err(StorageError::InvalidSourceAssociationPlan)
    }
}

fn apply_final_source_association_merge(
    transaction: &Transaction<'_>,
    data_root: &Path,
    merge: &FinalSourceAssociationMerge<'_>,
) -> Result<(), StorageError> {
    validate_final_source_association_source_mappings(transaction, merge)?;
    let final_members = merge
        .final_members
        .iter()
        .map(|member| (member.member_id, member))
        .collect::<BTreeMap<_, _>>();

    // Mount 必须先离开 loser，随后才能安全删除 loser Member。
    for assignment in merge.mount_assignments {
        let member = final_members
            .get(assignment.member_id)
            .ok_or(StorageError::InvalidSourceAssociationPlan)?;
        let expected_target = final_source_association_member_target(
            data_root,
            merge.expected_target_bundle,
            member.stable_relative_path,
        )?;
        let changed = transaction
            .execute(
                "UPDATE mounts
                 SET member_id = ?2, expected_target = ?3, health = 'healthy', updated_at = ?4
                 WHERE id = ?1",
                params![
                    assignment.mount_id,
                    assignment.member_id,
                    expected_target,
                    merge.now
                ],
            )
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        ensure_one_source_association_row(changed, merge.transaction_id)?;
    }

    transaction
        .execute(
            "DELETE FROM source_member_links WHERE source_id = ?1",
            [merge.source_id],
        )
        .map_err(StorageError::SaveSourceAssociationTransaction)?;
    transaction
        .execute(
            "DELETE FROM member_selections WHERE bundle_id IN (?1, ?2)",
            params![
                merge.expected_target_bundle.id,
                merge.expected_retiring_bundle.id
            ],
        )
        .map_err(StorageError::SaveSourceAssociationTransaction)?;

    for original in merge
        .expected_target_bundle
        .members
        .iter()
        .chain(merge.expected_retiring_bundle.members.iter())
        .filter(|member| !final_members.contains_key(member.id.as_str()))
    {
        let deleted = transaction
            .execute(
                "DELETE FROM skill_members
                 WHERE id = ?1 AND bundle_id IN (?2, ?3)",
                params![
                    original.id,
                    merge.expected_target_bundle.id,
                    merge.expected_retiring_bundle.id
                ],
            )
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        ensure_one_source_association_row(deleted, merge.transaction_id)?;
    }

    for member in merge.final_members {
        let changed = transaction
            .execute(
                "UPDATE skill_members
                 SET bundle_id = ?2, skill_name = ?3, description = ?4,
                     stable_relative_path = ?5, content_fingerprint = ?6
                 WHERE id = ?1 AND bundle_id IN (?2, ?7)",
                params![
                    member.member_id,
                    merge.expected_target_bundle.id,
                    member.skill_name,
                    member.description,
                    member.stable_relative_path,
                    member.content_fingerprint,
                    merge.expected_retiring_bundle.id
                ],
            )
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
        ensure_one_source_association_row(changed, merge.transaction_id)?;
        transaction
            .execute(
                "INSERT INTO member_selections (bundle_id, member_id, selected_at)
                 VALUES (?1, ?2, ?3)",
                params![merge.expected_target_bundle.id, member.member_id, merge.now],
            )
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
    }

    let updated = transaction
        .execute(
            "UPDATE bundles
             SET current_target = ?2
             WHERE id = ?1",
            params![merge.expected_target_bundle.id, merge.final_current_target],
        )
        .map_err(StorageError::SaveSourceAssociationTransaction)?;
    ensure_one_source_association_row(updated, merge.transaction_id)?;

    for mapping in merge.source_mappings {
        transaction
            .execute(
                "INSERT INTO source_member_links (
                    source_id, source_relative_path, member_id, linked_at
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    merge.source_id,
                    mapping.source_relative_path,
                    mapping.member_id,
                    merge.now
                ],
            )
            .map_err(StorageError::SaveSourceAssociationTransaction)?;
    }

    let deleted = transaction
        .execute(
            "DELETE FROM bundles WHERE id = ?1",
            [&merge.expected_retiring_bundle.id],
        )
        .map_err(StorageError::SaveSourceAssociationTransaction)?;
    ensure_one_source_association_row(deleted, merge.transaction_id)?;
    ensure_final_source_association_merge_matches(transaction, data_root, merge)
}

fn validate_final_source_association_source_mappings(
    transaction: &Transaction<'_>,
    merge: &FinalSourceAssociationMerge<'_>,
) -> Result<(), StorageError> {
    for mapping in merge.source_mappings {
        let selectable = transaction
            .query_row(
                "SELECT member.selectable
                 FROM source_catalog_members AS member
                 JOIN sources AS source
                   ON source.id = member.source_id
                  AND source.catalog_generation = member.catalog_generation
                 WHERE member.source_id = ?1 AND member.relative_path = ?2",
                params![merge.source_id, mapping.source_relative_path],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(StorageError::ReadSources)?;
        if selectable.map(sqlite_bool).transpose()? != Some(true) {
            return Err(StorageError::SourceCatalogStateChanged);
        }
    }
    Ok(())
}

fn final_source_association_member_target(
    data_root: &Path,
    target_bundle: &StoredSourceAssociationBundle,
    stable_relative_path: &str,
) -> Result<String, StorageError> {
    data_root
        .join(&target_bundle.managed_directory)
        .join("current")
        .join(stable_relative_path)
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| StorageError::UnsafeManagedPath(target_bundle.managed_directory.clone()))
}

fn ensure_final_source_association_merge_matches(
    transaction: &Transaction<'_>,
    data_root: &Path,
    merge: &FinalSourceAssociationMerge<'_>,
) -> Result<(), StorageError> {
    let retiring_exists = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM bundles WHERE id = ?1)",
            [&merge.expected_retiring_bundle.id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StorageError::ReadSources)?;
    if retiring_exists {
        return Err(StorageError::SourceBundleStateConflict);
    }
    let bundle = read_source_association_bundle_from(
        transaction,
        data_root,
        &merge.expected_target_bundle.id,
    )?;
    if bundle.display_name != merge.expected_target_bundle.display_name
        || bundle.managed_directory != merge.expected_target_bundle.managed_directory
        || bundle.current_target != merge.final_current_target
        || bundle.source_id.as_deref() != Some(merge.source_id)
        || bundle.adopted_marker != merge.expected_target_bundle.adopted_marker
        || bundle.members.len() != merge.final_members.len()
    {
        return Err(StorageError::SourceBundleStateConflict);
    }
    let mappings = merge
        .source_mappings
        .iter()
        .map(|mapping| (mapping.member_id, mapping.source_relative_path))
        .collect::<BTreeMap<_, _>>();
    let assignments = merge
        .mount_assignments
        .iter()
        .map(|assignment| (assignment.mount_id, assignment.member_id))
        .collect::<BTreeMap<_, _>>();
    let original_mounts = merge
        .expected_target_bundle
        .members
        .iter()
        .chain(merge.expected_retiring_bundle.members.iter())
        .flat_map(|member| member.mounts.iter())
        .map(|mount| (mount.id.as_str(), mount))
        .collect::<BTreeMap<_, _>>();
    for expected in merge.final_members {
        let actual = bundle
            .members
            .iter()
            .find(|member| member.id == expected.member_id)
            .ok_or(StorageError::SourceBundleStateConflict)?;
        let expected_target = final_source_association_member_target(
            data_root,
            merge.expected_target_bundle,
            expected.stable_relative_path,
        )?;
        if actual.skill_name != expected.skill_name
            || actual.description != expected.description
            || actual.stable_relative_path != expected.stable_relative_path
            || actual.content_fingerprint != expected.content_fingerprint
            || actual.source_relative_path.as_deref() != mappings.get(expected.member_id).copied()
            || actual.mounts.iter().any(|mount| {
                let original = original_mounts.get(mount.id.as_str());
                assignments.get(mount.id.as_str()).copied() != Some(expected.member_id)
                    || mount.member_id != expected.member_id
                    || mount.bundle_id != merge.expected_target_bundle.id
                    || mount.member_fingerprint != expected.content_fingerprint
                    || mount.expected_target != expected_target
                    || mount.health != MountHealth::Healthy
                    || original.is_none_or(|original| {
                        mount.app_id != original.app_id
                            || mount.scope != original.scope
                            || mount.project_id != original.project_id
                            || mount.project_display_name != original.project_display_name
                            || mount.project_root_path != original.project_root_path
                            || mount.project_root_device != original.project_root_device
                            || mount.project_root_inode != original.project_root_inode
                            || mount.target_path != original.target_path
                    })
            })
        {
            return Err(StorageError::SourceBundleStateConflict);
        }
    }
    let actual_mount_count = bundle
        .members
        .iter()
        .map(|member| member.mounts.len())
        .sum::<usize>();
    if actual_mount_count != merge.mount_assignments.len() {
        return Err(StorageError::SourceBundleStateConflict);
    }
    Ok(())
}

struct ValidatedTakeoverMember {
    fingerprint: String,
    stable_relative_path: String,
}

struct ValidatedTakeoverDomain {
    managed_directory: String,
    current_target: String,
    members: BTreeMap<String, ValidatedTakeoverMember>,
}

struct ValidatedTakeoverSource {
    canonical_identity: String,
    owner: String,
    repository: String,
    display_name: String,
    locator: String,
    explicit_tracked_ref: Option<String>,
    member_paths: BTreeMap<String, String>,
    existing_source_id: Option<String>,
}

/// 已核验的 lock v3 来源直接成为接管事务的一部分，不能只停留在预览文案中。
fn validate_takeover_source_contract(
    plan: &TakeoverPlan,
    existing_bundle: Option<&StoredTakeoverBundleSnapshot>,
) -> Result<Option<ValidatedTakeoverSource>, StorageError> {
    let members = plan
        .retained_members
        .iter()
        .map(|member| {
            (
                member.member_id.as_str(),
                member.installation_chain.as_deref(),
            )
        })
        .chain(plan.members.iter().map(|member| {
            (
                member.member_id.as_str(),
                member.installation_chain.as_deref(),
            )
        }))
        .collect::<Vec<_>>();
    let mut source: Option<ValidatedTakeoverSource> = None;
    let mut evidenced_members = 0;
    let mut source_paths = BTreeSet::new();

    for (member_id, chain) in &members {
        let Some(chain) = chain else {
            continue;
        };
        let Some(evidence) = takeover_group_evidence(chain) else {
            continue;
        };
        let parsed = parse_github_source(&chain.source_locator, chain.tracked_ref.as_deref())
            .map_err(|_| StorageError::InvalidTakeoverPlan)?;
        let canonical_identity = format!(
            "github:{}/{}",
            parsed.owner.to_ascii_lowercase(),
            parsed.repository.to_ascii_lowercase()
        );
        let display_name = evidence.display_name.trim().to_owned();
        let locator = format!("https://github.com/{}/{}", parsed.owner, parsed.repository);
        if evidence.id != canonical_identity || display_name.is_empty() {
            return Err(StorageError::InvalidTakeoverPlan);
        }

        match source.as_mut() {
            Some(existing)
                if existing.canonical_identity != canonical_identity
                    || existing.display_name != display_name
                    || existing.owner != parsed.owner
                    || existing.repository != parsed.repository =>
            {
                return Err(StorageError::InvalidTakeoverPlan);
            }
            Some(existing) => {
                if let Some(tracked_ref) = parsed.tracked_ref.as_ref() {
                    if existing
                        .explicit_tracked_ref
                        .as_ref()
                        .is_some_and(|current| current != tracked_ref)
                    {
                        return Err(StorageError::InvalidTakeoverPlan);
                    }
                    existing.explicit_tracked_ref = Some(tracked_ref.clone());
                }
            }
            None => {
                source = Some(ValidatedTakeoverSource {
                    canonical_identity,
                    owner: parsed.owner,
                    repository: parsed.repository,
                    display_name,
                    locator,
                    explicit_tracked_ref: parsed.tracked_ref,
                    member_paths: BTreeMap::new(),
                    existing_source_id: None,
                });
            }
        }

        if let Some(skill_path) = chain.skill_path.as_deref() {
            let source_relative_path = lock_skill_source_relative_path(skill_path)?;
            if !source_paths.insert(source_relative_path.clone()) {
                return Err(StorageError::InvalidTakeoverPlan);
            }
            source
                .as_mut()
                .ok_or(StorageError::InvalidTakeoverPlan)?
                .member_paths
                .insert((*member_id).to_owned(), source_relative_path);
        }
        evidenced_members += 1;
    }

    let Some(source) = source else {
        if plan.source_display_name.is_some() {
            return Err(StorageError::InvalidTakeoverPlan);
        }
        return Ok(None);
    };
    if evidenced_members != members.len() {
        return Err(StorageError::InvalidTakeoverPlan);
    }
    let mut source = source;
    if let Some(existing_bundle) = existing_bundle.filter(|bundle| bundle.source_id.is_some()) {
        source.existing_source_id = existing_bundle.source_id.clone();
        source.display_name = existing_bundle
            .source_display_name
            .clone()
            .ok_or(StorageError::InvalidTakeoverPlan)?;
    }
    if plan.source_display_name.as_deref() != Some(source.display_name.as_str()) {
        return Err(StorageError::InvalidTakeoverPlan);
    }
    Ok(Some(source))
}

fn lock_skill_source_relative_path(skill_path: &str) -> Result<String, StorageError> {
    let path = Path::new(skill_path.trim());
    if path.is_absolute()
        || path.file_name() != Some(std::ffi::OsStr::new("SKILL.md"))
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(StorageError::InvalidTakeoverPlan);
    }
    let parent = path
        .parent()
        .and_then(Path::to_str)
        .ok_or(StorageError::InvalidTakeoverPlan)?
        .to_owned();
    if !parent.is_empty() && !is_normalized_relative_path(&parent) {
        return Err(StorageError::InvalidTakeoverPlan);
    }
    Ok(parent)
}

fn validate_takeover_domain_contract(
    data_root: &Path,
    plan: &TakeoverPlan,
) -> Result<ValidatedTakeoverDomain, StorageError> {
    let origin_ids = plan
        .origins
        .iter()
        .map(|origin| origin.observation_id.as_str())
        .collect::<BTreeSet<_>>();
    let mount_ids = plan
        .targets
        .iter()
        .map(|target| target.mount_id.as_str())
        .collect::<BTreeSet<_>>();
    let target_paths = plan
        .targets
        .iter()
        .map(|target| target.target_path.as_str())
        .collect::<BTreeSet<_>>();
    let managed_directory = format!("bundles/{}", plan.bundle_id);
    let current_target = format!("contents/{}", plan.content_id);
    let mut validated_members = BTreeMap::new();
    let member_ids = plan
        .members
        .iter()
        .map(|member| member.member_id.as_str())
        .collect::<BTreeSet<_>>();
    let skill_names = plan
        .members
        .iter()
        .map(|member| member.skill_name.as_str())
        .collect::<BTreeSet<_>>();
    for member in &plan.members {
        let selected = plan
            .origins
            .iter()
            .filter(|origin| {
                origin.member_id == member.member_id
                    && origin.observation_id == member.selected_observation_id
            })
            .collect::<Vec<_>>();
        let stable_relative_path = format!("members/{}", member.skill_name);
        if !is_single_path_component(&member.member_id)
            || !is_single_path_component(&member.skill_name)
            || member.skill_description.trim().is_empty()
            || member
                .installation_chain
                .as_ref()
                .is_some_and(|chain| !chain.is_valid())
            || selected.len() != 1
            || selected[0].content_fingerprint.is_empty()
            || Path::new(&member.expected_target)
                != data_root
                    .join(&managed_directory)
                    .join("current")
                    .join(&stable_relative_path)
        {
            return Err(StorageError::InvalidTakeoverPlan);
        }
        validated_members.insert(
            member.member_id.clone(),
            ValidatedTakeoverMember {
                fingerprint: selected[0].content_fingerprint.clone(),
                stable_relative_path,
            },
        );
    }
    let targets_are_valid = plan.targets.iter().all(|target| {
        let Some(member) = plan
            .members
            .iter()
            .find(|member| member.member_id == target.member_id)
        else {
            return false;
        };
        let project_is_valid = match target.scope {
            MountScope::Global => {
                target.project_id.is_none() && target.project_display_name.is_none()
            }
            MountScope::Project => target
                .project_id
                .as_deref()
                .is_some_and(is_single_path_component),
        };
        is_single_path_component(&target.mount_id)
            && project_is_valid
            && is_normalized_absolute_path(&target.target_path)
            && target.expected_target == member.expected_target
            && Path::new(&target.target_path)
                .file_name()
                .and_then(|name| name.to_str())
                == Some(member.skill_name.as_str())
    });
    if !is_single_path_component(&plan.id)
        || !is_single_path_component(&plan.bundle_id)
        || !is_single_path_component(&plan.content_id)
        || plan.members.is_empty()
        || member_ids.len() != plan.members.len()
        || skill_names.len() != plan.members.len()
        || plan.bundle_display_name.trim().is_empty()
        || plan
            .source_display_name
            .as_deref()
            .is_some_and(|name| name.trim().is_empty())
        || origin_ids.len() != plan.origins.len()
        || plan.origins.iter().any(|origin| {
            !member_ids.contains(origin.member_id.as_str())
                || origin.observation_id.is_empty()
                || !is_normalized_absolute_path(&origin.original_path)
        })
        || mount_ids.len() != plan.targets.len()
        || target_paths.len() != plan.targets.len()
        || !targets_are_valid
        || Path::new(&plan.managed_directory) != data_root.join(&managed_directory)
        || Path::new(&plan.content_directory)
            != data_root.join(&managed_directory).join(&current_target)
    {
        return Err(StorageError::InvalidTakeoverPlan);
    }
    Ok(ValidatedTakeoverDomain {
        managed_directory,
        current_target,
        members: validated_members,
    })
}

fn persist_takeover_source(
    transaction: &Transaction<'_>,
    bundle_id: &str,
    source: &ValidatedTakeoverSource,
    now: i64,
) -> Result<String, StorageError> {
    let read_existing = |row: &rusqlite::Row<'_>| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    };
    let existing = if let Some(existing_source_id) = source.existing_source_id.as_deref() {
        transaction
            .query_row(
                "SELECT source.id, source.kind, source.canonical_identity,
                        source.tracked_ref, link.bundle_id
                 FROM sources AS source
                 LEFT JOIN source_bundle_links AS link ON link.source_id = source.id
                 WHERE source.id = ?1",
                [existing_source_id],
                read_existing,
            )
            .optional()
    } else {
        transaction
            .query_row(
                "SELECT source.id, source.kind, source.canonical_identity,
                        source.tracked_ref, link.bundle_id
                 FROM sources AS source
                 LEFT JOIN source_bundle_links AS link ON link.source_id = source.id
                 WHERE source.canonical_identity = ?1",
                [source.canonical_identity.as_str()],
                read_existing,
            )
            .optional()
    }
    .map_err(StorageError::SaveTakeoverTransaction)?;

    let (source_id, tracked_ref) = if let Some((
        source_id,
        kind,
        canonical_identity,
        current_ref,
        linked_bundle_id,
    )) = existing
    {
        if kind != "github"
            || canonical_identity != source.canonical_identity
            || linked_bundle_id
                .as_deref()
                .is_some_and(|linked_bundle_id| linked_bundle_id != bundle_id)
        {
            return Err(StorageError::SourceBundleStateConflict);
        }
        if source.existing_source_id.is_some() {
            (source_id, current_ref)
        } else {
            // 同一 Source 已保存的 Tracked Ref 只能由独立确认流程修改；接管只补关系和来源证据。
            let changed = transaction
                .execute(
                    "UPDATE sources
                 SET owner = ?2, repository = ?3, display_name = ?4,
                     locator = ?5, tracked_ref = ?6, updated_at = ?7
                 WHERE id = ?1 AND kind = 'github'",
                    params![
                        source_id,
                        source.owner,
                        source.repository,
                        source.display_name,
                        source.locator,
                        current_ref,
                        now,
                    ],
                )
                .map_err(StorageError::SaveTakeoverTransaction)?;
            if changed != 1 {
                return Err(StorageError::SourceBundleStateConflict);
            }
            (source_id, current_ref)
        }
    } else {
        if source.existing_source_id.is_some() {
            return Err(StorageError::SourceBundleStateConflict);
        }
        let source_id = Uuid::new_v4().to_string();
        let sort_order = transaction
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM sources",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StorageError::SaveTakeoverTransaction)?;
        let tracked_ref = source
            .explicit_tracked_ref
            .as_deref()
            .unwrap_or("HEAD")
            .to_owned();
        transaction
            .execute(
                "INSERT INTO sources (
                    id, kind, canonical_identity, owner, repository,
                    display_name, locator, tracked_ref, member_path_hint,
                    sort_order, created_at, updated_at
                 ) VALUES (?1, 'github', ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?9)",
                params![
                    source_id,
                    source.canonical_identity,
                    source.owner,
                    source.repository,
                    source.display_name,
                    source.locator,
                    tracked_ref,
                    sort_order,
                    now,
                ],
            )
            .map_err(StorageError::SaveTakeoverTransaction)?;
        (source_id, tracked_ref)
    };

    let linked_source_id = transaction
        .query_row(
            "SELECT source_id FROM source_bundle_links WHERE bundle_id = ?1",
            [bundle_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(StorageError::SaveTakeoverTransaction)?;
    match linked_source_id {
        Some(linked_source_id) if linked_source_id == source_id => {}
        Some(_) => return Err(StorageError::SourceBundleStateConflict),
        None => {
            transaction
                .execute(
                    "INSERT INTO source_bundle_links (
                        source_id, bundle_id, adopted_marker, linked_at
                     ) VALUES (?1, ?2, NULL, ?3)",
                    params![source_id, bundle_id, now],
                )
                .map_err(StorageError::SaveTakeoverTransaction)?;
        }
    }

    for (member_id, source_relative_path) in &source.member_paths {
        let existing_mapping = transaction
            .query_row(
                "SELECT source_id, source_relative_path
                 FROM source_member_links WHERE member_id = ?1",
                [member_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StorageError::SaveTakeoverTransaction)?;
        match existing_mapping {
            Some((existing_source_id, existing_path))
                if existing_source_id == source_id && existing_path == *source_relative_path => {}
            Some(_) => return Err(StorageError::SourceBundleStateConflict),
            None => {
                transaction
                    .execute(
                        "INSERT INTO source_member_links (
                            source_id, source_relative_path, member_id, linked_at
                         ) VALUES (?1, ?2, ?3, ?4)",
                        params![source_id, source_relative_path, member_id, now],
                    )
                    .map_err(StorageError::SaveTakeoverTransaction)?;
            }
        }
    }
    Ok(tracked_ref)
}

fn ensure_takeover_domain_matches(
    transaction: &Transaction<'_>,
    data_root: &Path,
    plan: &TakeoverPlan,
    validated: &ValidatedTakeoverDomain,
    existing_bundle: Option<&StoredTakeoverBundleSnapshot>,
    takeover_source: Option<&ValidatedTakeoverSource>,
    persisted_source_ref: Option<&str>,
) -> Result<(), StorageError> {
    let bundle = transaction
        .query_row(
            "SELECT display_name, managed_directory, current_target,
                    (SELECT COUNT(*) FROM skill_members WHERE bundle_id = ?1),
                    (SELECT COUNT(*) FROM member_selections WHERE bundle_id = ?1),
                    (SELECT COUNT(*) FROM mounts
                     WHERE member_id IN (SELECT id FROM skill_members WHERE bundle_id = ?1))
             FROM bundles WHERE id = ?1",
            [plan.bundle_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::SaveTakeoverTransaction)?
        .ok_or(StorageError::InvalidTakeoverPlan)?;
    let retained_member_count = existing_bundle.map_or(0, |bundle| bundle.members.len());
    let retained_mount_count = existing_bundle.map_or(0, |bundle| {
        bundle
            .members
            .iter()
            .map(|member| member.mounts.len())
            .sum()
    });
    if bundle.0 != plan.bundle_display_name
        || bundle.1 != validated.managed_directory
        || bundle.2 != validated.current_target
        || bundle.3 != (retained_member_count + plan.members.len()) as i64
        || bundle.4 != (retained_member_count + plan.members.len()) as i64
        || bundle.5 != (retained_mount_count + plan.targets.len()) as i64
    {
        return Err(StorageError::InvalidTakeoverPlan);
    }
    ensure_takeover_source_matches(
        transaction,
        &plan.bundle_id,
        takeover_source,
        existing_bundle.and_then(|bundle| bundle.source_id.as_deref()),
        persisted_source_ref,
    )?;
    if let Some(existing) = existing_bundle {
        let actual = read_takeover_bundle_snapshot_from(transaction, data_root, &plan.bundle_id)?;
        let source_was_added = existing.source_id.is_none() && takeover_source.is_some();
        if actual.id != existing.id
            || actual.display_name != existing.display_name
            || actual.managed_directory != existing.managed_directory
            || actual.current_target != validated.current_target
            || (!source_was_added && actual.source_id != existing.source_id)
            || (!source_was_added && actual.source_display_name != existing.source_display_name)
            || (source_was_added
                && actual.source_display_name.as_deref()
                    != takeover_source.map(|source| source.display_name.as_str()))
            || actual.adopted_marker
                != if source_was_added {
                    None
                } else {
                    existing.adopted_marker.clone()
                }
        {
            return Err(StorageError::InvalidTakeoverPlan);
        }
        for expected in &existing.members {
            let Some(member) = actual
                .members
                .iter()
                .find(|member| member.id == expected.id)
            else {
                return Err(StorageError::InvalidTakeoverPlan);
            };
            let member_matches = if let Some(source) = takeover_source.filter(|_| source_was_added)
            {
                member.id == expected.id
                    && member.skill_name == expected.skill_name
                    && member.description == expected.description
                    && member.stable_relative_path == expected.stable_relative_path
                    && member.content_fingerprint == expected.content_fingerprint
                    && member.installation_chain == expected.installation_chain
                    && member.mounts == expected.mounts
                    && member.source_relative_path.as_deref()
                        == source.member_paths.get(&expected.id).map(String::as_str)
            } else {
                member == expected
            };
            if !member_matches {
                return Err(StorageError::InvalidTakeoverPlan);
            }
        }
    }
    for planned in &plan.members {
        let expected = validated
            .members
            .get(&planned.member_id)
            .ok_or(StorageError::InvalidTakeoverPlan)?;
        let member = read_managed_member_from(transaction, data_root, &planned.member_id)?
            .ok_or(StorageError::InvalidTakeoverPlan)?;
        let installation_chain =
            read_member_installation_chain_from(transaction, &planned.member_id)?;
        let (description, selected, source_relative_path) = transaction
            .query_row(
                "SELECT description,
                        EXISTS(
                            SELECT 1 FROM member_selections
                            WHERE bundle_id = ?1 AND member_id = ?2
                        ),
                        (
                            SELECT source_relative_path
                            FROM source_member_links
                            WHERE member_id = ?2
                        )
                 FROM skill_members WHERE id = ?2 AND bundle_id = ?1",
                params![plan.bundle_id, planned.member_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(StorageError::SaveTakeoverTransaction)?
            .ok_or(StorageError::InvalidTakeoverPlan)?;
        if member.bundle_id != plan.bundle_id
            || member.skill_name != planned.skill_name
            || member.content_fingerprint != expected.fingerprint
            || member.stable_relative_path != expected.stable_relative_path
            || member.expected_target != planned.expected_target
            || source_relative_path.as_deref()
                != takeover_source
                    .and_then(|source| source.member_paths.get(&planned.member_id))
                    .map(String::as_str)
            || installation_chain.as_ref() != planned.installation_chain.as_deref()
            || description != planned.skill_description
            || !selected
        {
            return Err(StorageError::InvalidTakeoverPlan);
        }
    }
    for target in &plan.targets {
        let mount = read_mount_from(transaction, data_root, &target.mount_id)?
            .ok_or(StorageError::InvalidTakeoverPlan)?;
        if mount.member_id != target.member_id
            || mount.app_id != target.app_id
            || mount.scope != target.scope
            || mount.project_id != target.project_id
            || mount.target_path != target.target_path
            || mount.expected_target != target.expected_target
            || mount.health != MountHealth::Healthy
        {
            return Err(StorageError::InvalidTakeoverPlan);
        }
    }
    Ok(())
}

fn ensure_takeover_source_matches(
    transaction: &Transaction<'_>,
    bundle_id: &str,
    expected: Option<&ValidatedTakeoverSource>,
    existing_source_id: Option<&str>,
    expected_tracked_ref: Option<&str>,
) -> Result<(), StorageError> {
    let actual = transaction
        .query_row(
            "SELECT source.id, source.canonical_identity, source.owner,
                    source.repository, source.display_name, source.locator,
                    source.tracked_ref, link.adopted_marker
             FROM source_bundle_links AS link
             JOIN sources AS source ON source.id = link.source_id
             WHERE link.bundle_id = ?1",
            [bundle_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::SaveTakeoverTransaction)?;

    let Some(expected) = expected else {
        if existing_source_id.is_none() && actual.is_some() {
            return Err(StorageError::InvalidTakeoverPlan);
        }
        return Ok(());
    };
    let actual = actual.ok_or(StorageError::InvalidTakeoverPlan)?;
    if actual.1 != expected.canonical_identity
        || actual.2 != expected.owner
        || actual.3 != expected.repository
        || actual.4 != expected.display_name
        || actual.5 != expected.locator
        || actual.6.is_empty()
        || expected_tracked_ref.is_some_and(|tracked_ref| tracked_ref != actual.6)
        || actual.7.is_some()
    {
        return Err(StorageError::InvalidTakeoverPlan);
    }
    let mapping_count = transaction
        .query_row(
            "SELECT COUNT(*) FROM source_member_links WHERE source_id = ?1",
            [actual.0.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(StorageError::SaveTakeoverTransaction)?;
    if mapping_count != expected.member_paths.len() as i64 {
        return Err(StorageError::InvalidTakeoverPlan);
    }
    for (member_id, source_relative_path) in &expected.member_paths {
        let mapping_matches = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM source_member_links
                    WHERE source_id = ?1 AND source_relative_path = ?2 AND member_id = ?3
                 )",
                params![actual.0, source_relative_path, member_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StorageError::SaveTakeoverTransaction)?;
        if !mapping_matches {
            return Err(StorageError::InvalidTakeoverPlan);
        }
    }
    Ok(())
}

fn read_supported_apps_from(
    connection: &Connection,
) -> Result<Vec<SupportedAppSummary>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT app_id, display_name, detected FROM supported_app_status ORDER BY sort_order",
        )
        .map_err(StorageError::ReadInventory)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })
        .map_err(StorageError::ReadInventory)?;
    let mut supported_apps = Vec::new();
    for row in rows {
        let (id, display_name, detected) = row.map_err(StorageError::ReadInventory)?;
        supported_apps.push(SupportedAppSummary {
            id: SupportedAppId::from_str(&id)
                .ok_or_else(|| StorageError::UnknownSupportedApp(id.clone()))?,
            display_name,
            detected: Some(detected),
        });
    }
    Ok(supported_apps)
}

fn read_inventory_entries_from(
    connection: &Connection,
) -> Result<Vec<InventoryObservation>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT id, skill_name, declared_name, skill_root, skill_file, location_kind, metadata_status, observed_fingerprint, root_key, project_id, stale, management_kind FROM inventory_observations ORDER BY skill_root",
        )
        .map_err(StorageError::ReadInventory)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, bool>(10)?,
                row.get::<_, String>(11)?,
            ))
        })
        .map_err(StorageError::ReadInventory)?;
    let mut entries = Vec::new();
    for row in rows {
        let (
            id,
            skill_name,
            declared_name,
            skill_root,
            skill_file,
            location_kind,
            metadata_status,
            observed_fingerprint,
            root_key,
            project_id,
            stale,
            management_kind,
        ) = row.map_err(StorageError::ReadInventory)?;
        let root_key = ScanRootKey::from_str(&root_key)
            .ok_or_else(|| StorageError::UnknownScanRoot(root_key.clone()))?;
        validate_scan_root_identity(root_key, project_id.as_deref())?;
        entries.push(InventoryObservation {
            id,
            skill_name,
            declared_name,
            skill_root,
            skill_file,
            location_kind: InventoryLocationKind::from_str(&location_kind)
                .ok_or_else(|| StorageError::UnknownInventoryLocation(location_kind.clone()))?,
            metadata_status: SkillMetadataStatus::from_str(&metadata_status)
                .ok_or_else(|| StorageError::UnknownMetadataStatus(metadata_status.clone()))?,
            observed_by: Vec::new(),
            observed_fingerprint,
            root_key,
            project_id,
            stale,
            management_kind: ManagementKind::from_str(&management_kind)
                .ok_or_else(|| StorageError::UnknownManagementKind(management_kind.clone()))?,
            management_evidence: None,
            installation_chain: None,
        });
    }

    for entry in &mut entries {
        let stored_evidence = connection
            .query_row(
                "SELECT kind, authority_root, snapshot_commit_oid, subject_path
                 FROM inventory_management_evidence
                 WHERE observation_id = ?1",
                [&entry.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(StorageError::ReadInventory)?;
        if let Some((kind, authority_root, snapshot_commit_oid, subject_path)) = stored_evidence {
            entry.management_evidence = Some(ManagementEvidence {
                kind: ManagementEvidenceKind::from_str(&kind)
                    .ok_or_else(|| StorageError::UnknownManagementEvidenceKind(kind.clone()))?,
                authority_root,
                snapshot_commit_oid,
                subject_path,
            });
        }
        entry.installation_chain = read_observation_installation_chain_from(connection, &entry.id)?;
        let mut app_statement = connection
            .prepare(
                "SELECT app_id FROM inventory_observation_apps WHERE observation_id = ?1 ORDER BY app_id",
            )
            .map_err(StorageError::ReadInventory)?;
        let app_rows = app_statement
            .query_map([&entry.id], |row| row.get::<_, String>(0))
            .map_err(StorageError::ReadInventory)?;
        for row in app_rows {
            let id = row.map_err(StorageError::ReadInventory)?;
            entry.observed_by.push(
                SupportedAppId::from_str(&id)
                    .ok_or_else(|| StorageError::UnknownSupportedApp(id.clone()))?,
            );
        }
    }
    Ok(entries)
}

type InstallationChainRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
);

fn read_observation_installation_chain_from(
    connection: &Connection,
    observation_id: &str,
) -> Result<Option<InstallationChain>, StorageError> {
    read_installation_chain_query(
        connection,
        "SELECT kind, record_path, source, source_type, source_locator, skill_path,
                tracked_ref, content_marker, installed_at, updated_at
         FROM inventory_installation_chains WHERE observation_id = ?1",
        observation_id,
    )
}

fn read_member_installation_chain_from(
    connection: &Connection,
    member_id: &str,
) -> Result<Option<InstallationChain>, StorageError> {
    read_installation_chain_query(
        connection,
        "SELECT kind, record_path, source, source_type, source_locator, skill_path,
                tracked_ref, content_marker, installed_at, updated_at
         FROM member_installation_chains WHERE member_id = ?1",
        member_id,
    )
}

fn read_installation_chain_query(
    connection: &Connection,
    query: &str,
    owner_id: &str,
) -> Result<Option<InstallationChain>, StorageError> {
    let row = connection
        .query_row(query, [owner_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })
        .optional()
        .map_err(StorageError::ReadInventory)?;
    row.map(decode_installation_chain).transpose()
}

fn decode_installation_chain(row: InstallationChainRow) -> Result<InstallationChain, StorageError> {
    let chain = InstallationChain {
        kind: InstallationChainKind::from_str(&row.0)
            .ok_or_else(|| StorageError::UnknownInstallationChainKind(row.0.clone()))?,
        record_path: row.1,
        source: row.2,
        source_type: row.3,
        source_locator: row.4,
        skill_path: row.5,
        tracked_ref: row.6,
        content_marker: row.7,
        installed_at: row.8,
        updated_at: row.9,
    };
    if !chain.is_valid() {
        return Err(StorageError::InvalidInstallationChain);
    }
    Ok(chain)
}

fn read_scan_issues_from(connection: &Connection) -> Result<Vec<ScanIssue>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT root_id, root_key, project_id, path, code, message
             FROM inventory_scan_issues ORDER BY root_id, path, code",
        )
        .map_err(StorageError::ReadInventory)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(StorageError::ReadInventory)?;
    let mut issues = Vec::new();
    for row in rows {
        let (root_id, root_key, project_id, path, code, message) =
            row.map_err(StorageError::ReadInventory)?;
        let root_key = ScanRootKey::from_str(&root_key)
            .ok_or_else(|| StorageError::UnknownScanRoot(root_key.clone()))?;
        let identity = validate_scan_root_identity(root_key, project_id.as_deref())?;
        if root_id != identity.stable_id() {
            return Err(StorageError::InvalidScanRootIdentity(root_id));
        }
        let code = ScanIssueCode::from_str(&code)
            .ok_or_else(|| StorageError::UnknownScanIssueCode(code.clone()))?;
        issues.push(ScanIssue::new(identity, path, code, message));
    }
    Ok(issues)
}

fn validate_scan_root_identity(
    root_key: ScanRootKey,
    project_id: Option<&str>,
) -> Result<ScanRootIdentity, StorageError> {
    match (root_key.is_project(), project_id) {
        (false, None) => Ok(ScanRootIdentity::global(root_key)),
        (true, Some(project_id)) if is_single_path_component(project_id) => {
            Ok(ScanRootIdentity::project(root_key, project_id))
        }
        _ => Err(StorageError::InvalidScanRootIdentity(
            root_key.as_str().to_owned(),
        )),
    }
}

fn read_mount_target_paths_from(connection: &Connection) -> Result<BTreeSet<String>, StorageError> {
    let mut statement = connection
        .prepare("SELECT target_path FROM mounts ORDER BY target_path")
        .map_err(StorageError::ReadMountTransaction)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(StorageError::ReadMountTransaction)?;
    let mut targets = BTreeSet::new();
    for row in rows {
        let target = row.map_err(StorageError::ReadMountTransaction)?;
        if !is_normalized_absolute_path(&target) {
            return Err(StorageError::UnsafeManagedPath(target));
        }
        targets.insert(target);
    }
    Ok(targets)
}

fn validate_new_bundle_update_batch(
    batch_id: &str,
    items: &[NewBundleUpdateBatchItem<'_>],
    created_at: i64,
    expires_at: i64,
) -> Result<(), StorageError> {
    let mut item_ids = BTreeSet::new();
    let mut bundle_ids = BTreeSet::new();
    if !is_single_path_component(batch_id)
        || items.is_empty()
        || created_at < 0
        || expires_at <= created_at
        || items.iter().any(|item| {
            !is_single_path_component(item.id)
                || !is_single_path_component(item.source_id)
                || !is_single_path_component(item.bundle_id)
                || item.display_name.trim().is_empty()
                || item.target_marker.is_empty()
                || !item_ids.insert(item.id)
                || !bundle_ids.insert(item.bundle_id)
                || !matches!(
                    (item.status, item.install_plan_id, item.error),
                    ("ready", Some(_), None) | ("preparation_failed", None, Some(_))
                )
        })
    {
        return Err(StorageError::InvalidBundleUpdateBatch);
    }
    Ok(())
}

fn read_bundle_update_batch_from(
    connection: &Connection,
    batch_id: &str,
) -> Result<Option<StoredBundleUpdateBatch>, StorageError> {
    let batch = connection
        .query_row(
            "SELECT id, status, created_at, expires_at, confirmed_at, updated_at
             FROM bundle_update_batches
             WHERE id = ?1",
            [batch_id],
            |row| {
                Ok(StoredBundleUpdateBatch {
                    id: row.get(0)?,
                    status: row.get(1)?,
                    created_at: row.get(2)?,
                    expires_at: row.get(3)?,
                    confirmed_at: row.get(4)?,
                    updated_at: row.get(5)?,
                    items: Vec::new(),
                })
            },
        )
        .optional()
        .map_err(StorageError::ReadBundleUpdateBatch)?;
    let Some(mut batch) = batch else {
        return Ok(None);
    };
    let mut statement = connection
        .prepare(
            "SELECT id, source_id, bundle_id, display_name, install_plan_id,
                    target_marker, status, error, display_order, confirmed_order
             FROM bundle_update_batch_items
             WHERE batch_id = ?1
             ORDER BY display_order, id",
        )
        .map_err(StorageError::ReadBundleUpdateBatch)?;
    let rows = statement
        .query_map([batch_id], |row| {
            Ok(StoredBundleUpdateBatchItem {
                id: row.get(0)?,
                source_id: row.get(1)?,
                bundle_id: row.get(2)?,
                display_name: row.get(3)?,
                install_plan_id: row.get(4)?,
                target_marker: row.get(5)?,
                status: row.get(6)?,
                error: row.get(7)?,
                display_order: row.get(8)?,
                confirmed_order: row.get(9)?,
            })
        })
        .map_err(StorageError::ReadBundleUpdateBatch)?;
    batch.items = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::ReadBundleUpdateBatch)?;
    validate_stored_bundle_update_batch(&batch)?;
    Ok(Some(batch))
}

fn validate_bundle_update_batch_child_owner(
    connection: &Connection,
    plan_id: &str,
    owner: Option<BundleUpdateBatchChildOwner<'_>>,
    operation: BundleUpdateBatchChildOperation,
) -> Result<(), StorageError> {
    let ownership = connection
        .query_row(
            "SELECT item.batch_id, item.id, batch.status, item.status, item.confirmed_order
             FROM bundle_update_batch_items AS item
             JOIN bundle_update_batches AS batch ON batch.id = item.batch_id
             WHERE item.install_plan_id = ?1",
            [plan_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadBundleUpdateBatch)?;
    let Some((batch_id, item_id, batch_status, item_status, confirmed_order)) = ownership else {
        return if owner.is_none() {
            Ok(())
        } else {
            Err(StorageError::InvalidBundleUpdateBatch)
        };
    };
    let Some(owner) = owner else {
        return Err(StorageError::InstallPlanOwnedByBundleUpdateBatch);
    };
    if owner.batch_id != batch_id || owner.item_id != item_id {
        return Err(StorageError::InvalidBundleUpdateBatch);
    }

    let is_valid = match operation {
        BundleUpdateBatchChildOperation::Confirm => {
            batch_status == "running" && item_status == "ready" && confirmed_order.is_some()
        }
        BundleUpdateBatchChildOperation::Discard => {
            (batch_status == "running" && item_status == "ready" && confirmed_order.is_some())
                || (matches!(batch_status.as_str(), "running" | "blocked")
                    && item_status == "not_executed")
        }
    };
    if is_valid {
        Ok(())
    } else {
        Err(StorageError::InvalidBundleUpdateBatch)
    }
}

fn validate_stored_bundle_update_batch(
    batch: &StoredBundleUpdateBatch,
) -> Result<(), StorageError> {
    let mut item_ids = BTreeSet::new();
    let mut bundle_ids = BTreeSet::new();
    let mut display_orders = BTreeSet::new();
    let mut confirmed_orders = BTreeSet::new();
    let common_is_valid = is_single_path_component(&batch.id)
        && !batch.items.is_empty()
        && batch.created_at >= 0
        && batch.expires_at > batch.created_at
        && batch.updated_at >= batch.created_at
        && batch.items.iter().all(|item| {
            is_single_path_component(&item.id)
                && is_single_path_component(&item.source_id)
                && is_single_path_component(&item.bundle_id)
                && !item.display_name.trim().is_empty()
                && !item.target_marker.is_empty()
                && item.display_order >= 0
                && item.confirmed_order.is_none_or(|order| order >= 0)
                && item_ids.insert(item.id.as_str())
                && bundle_ids.insert(item.bundle_id.as_str())
                && display_orders.insert(item.display_order)
                && item
                    .confirmed_order
                    .is_none_or(|order| confirmed_orders.insert(order))
                && matches!(
                    (item.status.as_str(), item.error.as_deref()),
                    ("ready", None)
                        | ("preparation_failed", Some(_))
                        | ("succeeded", None)
                        | ("failed", Some(_))
                        | ("blocked", Some(_))
                        | ("not_executed", None)
                )
        });
    let state_is_valid = match batch.status.as_str() {
        "pending" => {
            batch.confirmed_at.is_none()
                && batch.items.iter().all(|item| {
                    matches!(item.status.as_str(), "ready" | "preparation_failed")
                        && item.confirmed_order.is_none()
                        && ((item.status == "ready" && item.install_plan_id.is_some())
                            || (item.status == "preparation_failed"
                                && item.install_plan_id.is_none()))
                })
        }
        "running" => {
            batch.confirmed_at.is_some()
                && batch
                    .items
                    .iter()
                    .all(|item| item.status != "preparation_failed")
                && batch
                    .items
                    .iter()
                    .filter(|item| item.status == "blocked")
                    .count()
                    <= 1
        }
        "completed" => {
            batch.confirmed_at.is_some()
                && batch.items.iter().all(|item| {
                    matches!(
                        item.status.as_str(),
                        "succeeded" | "failed" | "not_executed"
                    )
                })
        }
        "blocked" => {
            batch.confirmed_at.is_some()
                && batch
                    .items
                    .iter()
                    .filter(|item| item.status == "blocked")
                    .count()
                    == 1
                && batch.items.iter().all(|item| {
                    matches!(
                        item.status.as_str(),
                        "succeeded" | "failed" | "blocked" | "not_executed"
                    )
                })
        }
        _ => false,
    };
    if common_is_valid && state_is_valid {
        Ok(())
    } else {
        Err(StorageError::InvalidBundleUpdateBatch)
    }
}

fn read_install_plan_from(
    connection: &Connection,
    plan_id: &str,
) -> Result<Option<StoredInstallPlan>, StorageError> {
    let row = connection
        .query_row(
            "SELECT
                id, kind, install_mode, input_path, input_device, input_inode,
                input_fingerprint, snapshot_relative_path, source_id, source_tracked_ref,
                source_catalog_generation, source_marker, expected_source_marker,
                expected_current_target, expected_adopted_marker, bundle_id, bundle_display_name,
                warnings_json, created_at, expires_at, status
             FROM install_plans
             WHERE id = ?1",
            [plan_id],
            |row| {
                Ok(RawStoredInstallPlan {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    install_mode: row.get(2)?,
                    input_path: row.get(3)?,
                    input_device: row.get(4)?,
                    input_inode: row.get(5)?,
                    input_fingerprint: row.get(6)?,
                    snapshot_relative_path: row.get(7)?,
                    source_id: row.get(8)?,
                    source_tracked_ref: row.get(9)?,
                    source_catalog_generation: row.get(10)?,
                    source_marker: row.get(11)?,
                    expected_source_marker: row.get(12)?,
                    expected_current_target: row.get(13)?,
                    expected_adopted_marker: row.get(14)?,
                    bundle_id: row.get(15)?,
                    bundle_display_name: row.get(16)?,
                    warnings_json: row.get(17)?,
                    created_at: row.get(18)?,
                    expires_at: row.get(19)?,
                    status: row.get(20)?,
                })
            },
        )
        .optional()
        .map_err(StorageError::ReadInstallPlan)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let input_device =
        u64::try_from(row.input_device).map_err(|_| StorageError::InvalidInstallPlan)?;
    let input_inode =
        u64::try_from(row.input_inode).map_err(|_| StorageError::InvalidInstallPlan)?;
    let warnings =
        serde_json::from_str(&row.warnings_json).map_err(StorageError::InvalidPlanWarnings)?;
    let candidates = read_install_candidates_from(connection, &row.id)?;
    let plan = StoredInstallPlan {
        id: row.id,
        kind: row.kind,
        install_mode: row.install_mode,
        input_path: row.input_path,
        input_device,
        input_inode,
        input_fingerprint: row.input_fingerprint,
        snapshot_relative_path: row.snapshot_relative_path,
        source_id: row.source_id,
        source_tracked_ref: row.source_tracked_ref,
        source_catalog_generation: row.source_catalog_generation,
        source_marker: row.source_marker,
        expected_source_marker: row.expected_source_marker,
        expected_current_target: row.expected_current_target,
        expected_adopted_marker: row.expected_adopted_marker,
        bundle_id: row.bundle_id,
        bundle_display_name: row.bundle_display_name,
        warnings,
        created_at: row.created_at,
        expires_at: row.expires_at,
        status: row.status,
        candidates,
    };
    validate_stored_install_plan_contract(&plan)?;
    Ok(Some(plan))
}

fn read_install_candidates_from(
    connection: &Connection,
    plan_id: &str,
) -> Result<Vec<StoredInstallCandidate>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT candidate_id, source_relative_path, skill_name, skill_description,
                    content_fingerprint, previous_content_fingerprint, selectable, preserve_existing,
                    validation_errors_json, warnings_json, default_selected, selected
             FROM install_plan_candidates
             WHERE plan_id = ?1
             ORDER BY sort_order",
        )
        .map_err(StorageError::ReadInstallPlan)?;
    let rows = statement
        .query_map([plan_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, i64>(11)?,
            ))
        })
        .map_err(StorageError::ReadInstallPlan)?;
    let mut candidates = Vec::new();
    for row in rows {
        let (
            candidate_id,
            source_relative_path,
            skill_name,
            skill_description,
            content_fingerprint,
            previous_content_fingerprint,
            selectable,
            preserve_existing,
            validation_errors_json,
            warnings_json,
            default_selected,
            selected,
        ) = row.map_err(StorageError::ReadInstallPlan)?;
        candidates.push(StoredInstallCandidate {
            candidate_id,
            source_relative_path,
            skill_name,
            skill_description,
            content_fingerprint,
            previous_content_fingerprint,
            selectable: sqlite_bool(selectable)?,
            preserve_existing: sqlite_bool(preserve_existing)?,
            validation_errors: serde_json::from_str(&validation_errors_json)
                .map_err(StorageError::InvalidPlanValidationErrors)?,
            warnings: serde_json::from_str(&warnings_json)
                .map_err(StorageError::InvalidPlanWarnings)?,
            default_selected: sqlite_bool(default_selected)?,
            selected: sqlite_bool(selected)?,
        });
    }
    Ok(candidates)
}

fn stored_github_source_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredGithubSource> {
    Ok(StoredGithubSource {
        id: row.get(0)?,
        canonical_identity: row.get(1)?,
        owner: row.get(2)?,
        repository: row.get(3)?,
        display_name: row.get(4)?,
        tracked_ref: row.get(5)?,
    })
}

fn read_source_install_source_from(
    connection: &Connection,
    source_id: &str,
) -> Result<StoredSourceInstallSource, StorageError> {
    let source = connection
        .query_row(
            "SELECT kind, canonical_identity, owner, repository, display_name,
                    locator, tracked_ref, filesystem_device, filesystem_inode,
                    catalog_status, catalog_generation, catalog_marker
             FROM sources
             WHERE id = ?1",
            [source_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, Option<String>>(11)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadSources)?
        .ok_or(StorageError::SourceNotFound)?;
    let (
        kind,
        canonical_identity,
        owner,
        repository,
        display_name,
        locator,
        tracked_ref,
        filesystem_device,
        filesystem_inode,
        catalog_status,
        catalog_generation,
        catalog_marker,
    ) = source;
    let filesystem_device = filesystem_device
        .map(filesystem_identity_from_sql)
        .transpose()?;
    let filesystem_inode = filesystem_inode
        .map(filesystem_identity_from_sql)
        .transpose()?;
    let Some(catalog_marker) = catalog_marker else {
        return Err(StorageError::SourceCatalogStateChanged);
    };
    if catalog_status != "fresh"
        || catalog_generation <= 0
        || catalog_marker.is_empty()
        || (kind == "github"
            && tracked_ref
                .as_deref()
                .is_none_or(|tracked_ref| tracked_ref.is_empty()))
        || (kind == "github"
            && (owner.as_deref().is_none_or(str::is_empty)
                || repository.as_deref().is_none_or(str::is_empty)))
        || (kind != "github" && (owner.is_some() || repository.is_some() || tracked_ref.is_some()))
        || (kind == "editable_local" && (filesystem_device.is_none() || filesystem_inode.is_none()))
        || (kind != "editable_local" && (filesystem_device.is_some() || filesystem_inode.is_some()))
    {
        return Err(StorageError::SourceCatalogStateChanged);
    }

    let catalog_members =
        read_source_install_catalog_members_from(connection, source_id, catalog_generation)?;
    let linked = connection
        .query_row(
            "SELECT bundle_id, adopted_marker, update_check_status,
                    update_checked_marker, update_checked_at
             FROM source_bundle_links
             WHERE source_id = ?1",
            [source_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadSources)?;
    let bundle = linked
        .map(
            |(
                bundle_id,
                adopted_marker,
                update_check_status,
                update_checked_marker,
                update_checked_at,
            )| {
                read_source_install_bundle_from(
                    connection,
                    source_id,
                    &bundle_id,
                    adopted_marker,
                    update_check_status,
                    update_checked_marker,
                    update_checked_at,
                )
            },
        )
        .transpose()?;

    Ok(StoredSourceInstallSource {
        id: source_id.to_owned(),
        kind,
        canonical_identity,
        owner,
        repository,
        display_name,
        locator,
        tracked_ref,
        filesystem_device,
        filesystem_inode,
        catalog_generation,
        catalog_marker,
        catalog_members,
        bundle,
    })
}

fn read_source_install_catalog_members_from(
    connection: &Connection,
    source_id: &str,
    catalog_generation: i64,
) -> Result<Vec<StoredSourceInstallCatalogMember>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT id, relative_path, skill_name, description, content_fingerprint,
                    selectable, validation_errors_json, warnings_json
             FROM source_catalog_members
             WHERE source_id = ?1 AND catalog_generation = ?2
             ORDER BY sort_order, relative_path",
        )
        .map_err(StorageError::ReadSources)?;
    let rows = statement
        .query_map(params![source_id, catalog_generation], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(StorageError::ReadSources)?;
    let mut members = Vec::new();
    for row in rows {
        let (
            id,
            relative_path,
            skill_name,
            description,
            content_fingerprint,
            selectable,
            validation_errors,
            warnings,
        ) = row.map_err(StorageError::ReadSources)?;
        if !is_single_path_component(&id)
            || (!relative_path.is_empty() && !is_normalized_relative_path(&relative_path))
        {
            return Err(StorageError::SourceCatalogStateChanged);
        }
        members.push(StoredSourceInstallCatalogMember {
            id,
            relative_path,
            skill_name,
            description,
            content_fingerprint,
            selectable: sqlite_bool(selectable)?,
            validation_errors: serde_json::from_str(&validation_errors)
                .map_err(StorageError::InvalidSourceCatalogMetadata)?,
            warnings: serde_json::from_str(&warnings)
                .map_err(StorageError::InvalidSourceCatalogMetadata)?,
        });
    }
    Ok(members)
}

fn read_source_install_bundle_from(
    connection: &Connection,
    source_id: &str,
    bundle_id: &str,
    adopted_marker: Option<String>,
    update_check_status: String,
    update_checked_marker: Option<String>,
    update_checked_at: Option<i64>,
) -> Result<StoredSourceInstallBundle, StorageError> {
    let bundle = connection
        .query_row(
            "SELECT display_name, managed_directory, current_target
             FROM bundles
             WHERE id = ?1",
            [bundle_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadSources)?
        .ok_or(StorageError::SourceBundleStateConflict)?;
    let (display_name, managed_directory, current_target) = bundle;
    let update_check_status = BundleUpdateStatus::from_stored_str(&update_check_status)
        .ok_or_else(|| StorageError::UnknownBundleUpdateStatus(update_check_status.clone()))?;
    if !is_single_path_component(bundle_id)
        || managed_directory != format!("bundles/{bundle_id}")
        || !is_safe_current_target(&current_target)
    {
        return Err(StorageError::SourceBundleStateConflict);
    }

    let mut statement = connection
        .prepare(
            "SELECT member.id, member.skill_name, member.description,
                    member.stable_relative_path, member.content_fingerprint,
                    source_link.source_relative_path, selection.member_id
             FROM skill_members AS member
             LEFT JOIN member_selections AS selection
               ON selection.bundle_id = member.bundle_id
              AND selection.member_id = member.id
             LEFT JOIN source_member_links AS source_link
               ON source_link.source_id = ?1
              AND source_link.member_id = member.id
             WHERE member.bundle_id = ?2
             ORDER BY member.stable_relative_path, member.id",
        )
        .map_err(StorageError::ReadSources)?;
    let rows = statement
        .query_map(params![source_id, bundle_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(StorageError::ReadSources)?;
    let mut members = Vec::new();
    for row in rows {
        let (
            id,
            skill_name,
            description,
            stable_relative_path,
            content_fingerprint,
            source_relative_path,
            selected_member_id,
        ) = row.map_err(StorageError::ReadSources)?;
        if selected_member_id.as_deref() != Some(id.as_str())
            || stable_relative_path != format!("members/{skill_name}")
            || source_relative_path
                .as_deref()
                .is_some_and(|path| !path.is_empty() && !is_normalized_relative_path(path))
        {
            return Err(StorageError::SourceBundleStateConflict);
        }
        members.push(StoredSourceInstallBundleMember {
            id,
            skill_name,
            description,
            stable_relative_path,
            content_fingerprint,
            source_relative_path,
        });
    }
    drop(statement);
    let source_link_count = connection
        .query_row(
            "SELECT COUNT(*) FROM source_member_links WHERE source_id = ?1",
            [source_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(StorageError::ReadSources)?;
    let mapped_member_count = members
        .iter()
        .filter(|member| member.source_relative_path.is_some())
        .count() as i64;
    if members.is_empty() || source_link_count != mapped_member_count {
        return Err(StorageError::SourceBundleStateConflict);
    }
    Ok(StoredSourceInstallBundle {
        id: bundle_id.to_owned(),
        display_name,
        current_target,
        adopted_marker,
        update_check_status,
        update_checked_marker,
        update_checked_at,
        members,
    })
}

fn read_source_association_bundle_from(
    connection: &Connection,
    data_root: &Path,
    bundle_id: &str,
) -> Result<StoredSourceAssociationBundle, StorageError> {
    let bundle = connection
        .query_row(
            "SELECT bundle.display_name, bundle.managed_directory, bundle.current_target,
                    source_link.source_id, source_link.adopted_marker
             FROM bundles AS bundle
             LEFT JOIN source_bundle_links AS source_link
               ON source_link.bundle_id = bundle.id
             WHERE bundle.id = ?1",
            [bundle_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadSources)?
        .ok_or_else(|| StorageError::ManagedMemberNotFound(bundle_id.to_owned()))?;
    let (display_name, managed_directory, current_target, source_id, adopted_marker) = bundle;
    if !is_single_path_component(bundle_id)
        || display_name.trim().is_empty()
        || managed_directory != format!("bundles/{bundle_id}")
        || !is_safe_current_target(&current_target)
        || (source_id.is_none() && adopted_marker.is_some())
    {
        return Err(StorageError::SourceBundleStateConflict);
    }

    let mut statement = connection
        .prepare(
            "SELECT member.id, member.skill_name, member.description,
                    member.stable_relative_path, member.content_fingerprint,
                    selection.member_id, source_member.source_relative_path
             FROM skill_members AS member
             LEFT JOIN member_selections AS selection
               ON selection.bundle_id = member.bundle_id
              AND selection.member_id = member.id
             LEFT JOIN source_bundle_links AS bundle_source
               ON bundle_source.bundle_id = member.bundle_id
             LEFT JOIN source_member_links AS source_member
               ON source_member.member_id = member.id
              AND source_member.source_id = bundle_source.source_id
             WHERE member.bundle_id = ?1
             ORDER BY member.stable_relative_path, member.id",
        )
        .map_err(StorageError::ReadSources)?;
    let rows = statement
        .query_map([bundle_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(StorageError::ReadSources)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::ReadSources)?;
    drop(statement);

    let mut members = Vec::with_capacity(rows.len());
    for (
        id,
        skill_name,
        description,
        stable_relative_path,
        content_fingerprint,
        selected_member_id,
        source_relative_path,
    ) in rows
    {
        if selected_member_id.as_deref() != Some(id.as_str())
            || !is_single_path_component(&id)
            || !is_single_path_component(&skill_name)
            || stable_relative_path != format!("members/{skill_name}")
            || content_fingerprint.is_empty()
            || source_relative_path
                .as_deref()
                .is_some_and(|path| !path.is_empty() && !is_normalized_relative_path(path))
            || (source_id.is_none() && source_relative_path.is_some())
        {
            return Err(StorageError::SourceBundleStateConflict);
        }
        let mut mount_statement = connection
            .prepare("SELECT id FROM mounts WHERE member_id = ?1 ORDER BY target_path, id")
            .map_err(StorageError::ReadMountTransaction)?;
        let mount_ids = mount_statement
            .query_map([&id], |row| row.get::<_, String>(0))
            .map_err(StorageError::ReadMountTransaction)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadMountTransaction)?;
        drop(mount_statement);
        let mounts = mount_ids
            .into_iter()
            .map(|mount_id| {
                read_mount_from(connection, data_root, &mount_id)?
                    .ok_or(StorageError::MountNotFound(mount_id))
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        members.push(StoredSourceAssociationBundleMember {
            id,
            skill_name,
            description,
            stable_relative_path,
            content_fingerprint,
            source_relative_path,
            mounts,
        });
    }
    let stored_mapping_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM source_member_links AS source_member
             JOIN skill_members AS member ON member.id = source_member.member_id
             WHERE member.bundle_id = ?1",
            [bundle_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(StorageError::ReadSources)?;
    let visible_mapping_count = members
        .iter()
        .filter(|member| member.source_relative_path.is_some())
        .count() as i64;
    if members.is_empty()
        || stored_mapping_count != visible_mapping_count
        || (source_id.is_none() && stored_mapping_count != 0)
    {
        return Err(StorageError::SourceBundleStateConflict);
    }
    Ok(StoredSourceAssociationBundle {
        id: bundle_id.to_owned(),
        display_name,
        managed_directory,
        current_target,
        source_id,
        adopted_marker,
        members,
    })
}

fn read_takeover_bundle_snapshot_from(
    connection: &Connection,
    data_root: &Path,
    bundle_id: &str,
) -> Result<StoredTakeoverBundleSnapshot, StorageError> {
    let bundle = read_source_association_bundle_from(connection, data_root, bundle_id)?;
    let source_display_name = bundle
        .source_id
        .as_deref()
        .map(|source_id| {
            connection.query_row(
                "SELECT display_name FROM sources WHERE id = ?1",
                [source_id],
                |row| row.get::<_, String>(0),
            )
        })
        .transpose()
        .map_err(StorageError::ReadSources)?;
    let members = bundle
        .members
        .into_iter()
        .map(|member| {
            let installation_chain = read_member_installation_chain_from(connection, &member.id)?;
            let mounts = member
                .mounts
                .into_iter()
                .map(|mount| StoredTakeoverMountSnapshot {
                    id: mount.id,
                    member_id: mount.member_id,
                    bundle_id: mount.bundle_id,
                    skill_name: mount.skill_name,
                    member_fingerprint: mount.member_fingerprint,
                    app_id: mount.app_id,
                    scope: mount.scope,
                    project_id: mount.project_id,
                    project_display_name: mount.project_display_name,
                    project_root_path: mount.project_root_path,
                    project_root_device: mount.project_root_device,
                    project_root_inode: mount.project_root_inode,
                    target_path: mount.target_path,
                    expected_target: mount.expected_target,
                    health: mount.health,
                })
                .collect();
            Ok(StoredTakeoverBundleMemberSnapshot {
                id: member.id,
                skill_name: member.skill_name,
                description: member.description,
                stable_relative_path: member.stable_relative_path,
                content_fingerprint: member.content_fingerprint,
                source_relative_path: member.source_relative_path,
                installation_chain,
                mounts,
            })
        })
        .collect::<Result<Vec<_>, StorageError>>()?;
    Ok(StoredTakeoverBundleSnapshot {
        id: bundle.id,
        display_name: bundle.display_name,
        managed_directory: bundle.managed_directory,
        current_target: bundle.current_target,
        source_id: bundle.source_id,
        source_display_name,
        adopted_marker: bundle.adopted_marker,
        members,
    })
}

fn read_source_catalog_members_from(
    connection: &Connection,
    source_id: &str,
    catalog_generation: i64,
) -> Result<Vec<SourceCatalogMemberSummary>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT member.id, member.relative_path, member.skill_name,
                    member.description, member.selectable,
                    member.validation_errors_json, member.warnings_json,
                    link.member_id
             FROM source_catalog_members AS member
             LEFT JOIN source_member_links AS link
               ON link.source_id = member.source_id
              AND link.source_relative_path = member.relative_path
             WHERE member.source_id = ?1 AND member.catalog_generation = ?2
             ORDER BY member.sort_order, member.relative_path",
        )
        .map_err(StorageError::ReadSources)?;
    let rows = statement
        .query_map(params![source_id, catalog_generation], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })
        .map_err(StorageError::ReadSources)?;
    let mut members = Vec::new();
    for row in rows {
        let (
            id,
            relative_path,
            skill_name,
            description,
            selectable,
            validation_errors,
            warnings,
            installed_member_id,
        ) = row.map_err(StorageError::ReadSources)?;
        members.push(SourceCatalogMemberSummary {
            id,
            relative_path,
            skill_name,
            description,
            selectable: sqlite_bool(selectable)?,
            validation_errors: serde_json::from_str(&validation_errors)
                .map_err(StorageError::InvalidSourceCatalogMetadata)?,
            warnings: serde_json::from_str(&warnings)
                .map_err(StorageError::InvalidSourceCatalogMetadata)?,
            installed_member_id,
        });
    }
    Ok(members)
}

fn sqlite_bool(value: i64) -> Result<bool, StorageError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(StorageError::InvalidPlanBoolean(other)),
    }
}

const EDITABLE_LOCAL_RELINK_PLAN_SELECT: &str = "
    SELECT
        id, source_id, expected_canonical_identity,
        expected_source_display_name, expected_locator,
        expected_device, expected_inode, expected_catalog_generation,
        expected_catalog_marker, expected_bundle_id,
        expected_bundle_display_name, candidate_path,
        candidate_display_name, candidate_marker,
        candidate_members_json, created_at, expires_at, status
    FROM editable_local_relink_plans
    WHERE status = 'pending' AND expires_at > ?1
    ORDER BY created_at, id
    LIMIT 1
";

const EDITABLE_LOCAL_RELINK_PLAN_BY_ID_SELECT: &str = "
    SELECT
        id, source_id, expected_canonical_identity,
        expected_source_display_name, expected_locator,
        expected_device, expected_inode, expected_catalog_generation,
        expected_catalog_marker, expected_bundle_id,
        expected_bundle_display_name, candidate_path,
        candidate_display_name, candidate_marker,
        candidate_members_json, created_at, expires_at, status
    FROM editable_local_relink_plans
    WHERE id = ?1
";

#[derive(Debug)]
struct RawStoredEditableLocalRelinkPlan {
    id: String,
    source_id: String,
    expected_canonical_identity: String,
    expected_source_display_name: String,
    expected_locator: String,
    expected_device: i64,
    expected_inode: i64,
    expected_catalog_generation: i64,
    expected_catalog_marker: String,
    expected_bundle_id: Option<String>,
    expected_bundle_display_name: Option<String>,
    candidate_path: String,
    candidate_display_name: String,
    candidate_marker: String,
    candidate_members_json: String,
    created_at: i64,
    expires_at: i64,
    status: String,
}

fn stored_editable_local_relink_plan_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RawStoredEditableLocalRelinkPlan> {
    Ok(RawStoredEditableLocalRelinkPlan {
        id: row.get(0)?,
        source_id: row.get(1)?,
        expected_canonical_identity: row.get(2)?,
        expected_source_display_name: row.get(3)?,
        expected_locator: row.get(4)?,
        expected_device: row.get(5)?,
        expected_inode: row.get(6)?,
        expected_catalog_generation: row.get(7)?,
        expected_catalog_marker: row.get(8)?,
        expected_bundle_id: row.get(9)?,
        expected_bundle_display_name: row.get(10)?,
        candidate_path: row.get(11)?,
        candidate_display_name: row.get(12)?,
        candidate_marker: row.get(13)?,
        candidate_members_json: row.get(14)?,
        created_at: row.get(15)?,
        expires_at: row.get(16)?,
        status: row.get(17)?,
    })
}

fn validate_stored_editable_local_relink_plan(
    raw: RawStoredEditableLocalRelinkPlan,
) -> Result<StoredEditableLocalRelinkPlan, StorageError> {
    let expected_device = filesystem_identity_from_sql(raw.expected_device)?;
    let expected_inode = filesystem_identity_from_sql(raw.expected_inode)?;
    let members =
        serde_json::from_str::<Vec<EditableLocalRelinkMember>>(&raw.candidate_members_json)
            .map_err(StorageError::InvalidEditableLocalRelinkMetadata)?;
    let bundle_pair_is_valid = matches!(
        (&raw.expected_bundle_id, &raw.expected_bundle_display_name),
        (Some(id), Some(name)) if !id.is_empty() && !name.is_empty()
    ) || (raw.expected_bundle_id.is_none()
        && raw.expected_bundle_display_name.is_none());
    let members_are_valid = !members.is_empty()
        && members.iter().all(|member| {
            (member.relative_path.is_empty() || is_normalized_relative_path(&member.relative_path))
                && member
                    .skill_name
                    .as_deref()
                    .is_none_or(|name| !name.is_empty())
        });
    if raw.id.is_empty()
        || raw.source_id.is_empty()
        || raw.expected_source_display_name.is_empty()
        || raw.candidate_display_name.is_empty()
        || raw.expected_catalog_generation <= 0
        || raw.expected_catalog_marker.is_empty()
        || raw.candidate_marker.is_empty()
        || raw.created_at < 0
        || raw.expires_at <= raw.created_at
        || !matches!(raw.status.as_str(), "pending" | "consumed")
        || raw.expected_locator == raw.candidate_path
        || !is_normalized_absolute_path(&raw.expected_locator)
        || !is_normalized_absolute_path(&raw.candidate_path)
        || raw.expected_canonical_identity
            != format!("editable-local:{expected_device}:{expected_inode}")
        || !bundle_pair_is_valid
        || !members_are_valid
    {
        return Err(StorageError::EditableLocalRelinkStateChanged);
    }
    Ok(StoredEditableLocalRelinkPlan {
        public: EditableLocalRelinkPlan {
            id: raw.id,
            source_id: raw.source_id,
            source_display_name: raw.expected_source_display_name,
            current_path: raw.expected_locator,
            candidate_path: raw.candidate_path,
            candidate_display_name: raw.candidate_display_name,
            bundle_display_name: raw.expected_bundle_display_name,
            members,
            created_at: raw.created_at,
            expires_at: raw.expires_at,
        },
        expected_canonical_identity: raw.expected_canonical_identity,
        expected_device,
        expected_inode,
        expected_catalog_generation: raw.expected_catalog_generation,
        expected_catalog_marker: raw.expected_catalog_marker,
        expected_bundle_id: raw.expected_bundle_id,
        candidate_marker: raw.candidate_marker,
        status: raw.status,
    })
}

fn inventory_item_from_observation(
    observation: InventoryObservation,
    project_display_name: Option<String>,
) -> InventoryItem {
    let external_group_display_name = official_plugin_display_name(&observation);
    let takeover_group = observation
        .installation_chain
        .as_ref()
        .and_then(takeover_group_evidence);
    InventoryItem {
        id: observation.id,
        skill_name: observation.skill_name,
        declared_name: observation.declared_name,
        skill_root: observation.skill_root,
        skill_file: observation.skill_file,
        location_kind: observation.location_kind,
        metadata_status: observation.metadata_status,
        observed_by: observation.observed_by,
        observed_fingerprint: observation.observed_fingerprint,
        root_key: Some(observation.root_key),
        project_id: observation.project_id,
        stale: observation.stale,
        management_kind: observation.management_kind,
        management_evidence: observation.management_evidence,
        installation_chain: observation.installation_chain,
        takeover_group_id: takeover_group.as_ref().map(|group| group.id.clone()),
        takeover_group_display_name: takeover_group.map(|group| group.display_name),
        external_group_display_name,
        bundle_id: None,
        member_id: None,
        bundle_display_name: None,
        source_display_name: None,
        project_display_name,
    }
}

fn official_plugin_display_name(observation: &InventoryObservation) -> Option<String> {
    if observation.root_key != ScanRootKey::CodexOfficialPlugins {
        return None;
    }
    // 固定缓存结构为 <marketplace>/<plugin>/<version>/skills/<skill>。
    Path::new(&observation.skill_root)
        .parent()?
        .parent()?
        .parent()?
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

fn read_managed_entries_from(
    connection: &Connection,
    data_root: &Path,
) -> Result<Vec<InventoryItem>, StorageError> {
    // folder、takeover 与已删除 Source 的 Bundle 都没有来源关联，因此这里必须保留 LEFT JOIN。
    let mut statement = connection
        .prepare(
            "SELECT member.id, member.skill_name, member.description, member.stable_relative_path, member.content_fingerprint, bundle.id, bundle.display_name, bundle.managed_directory, bundle.current_target, source.display_name
             FROM skill_members member
             JOIN member_selections selection ON selection.member_id = member.id AND selection.bundle_id = member.bundle_id
             JOIN bundles bundle ON bundle.id = member.bundle_id
             LEFT JOIN source_bundle_links source_link ON source_link.bundle_id = bundle.id
             LEFT JOIN sources source ON source.id = source_link.source_id
             ORDER BY bundle.display_name, member.skill_name, member.id",
        )
        .map_err(StorageError::ReadInventory)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })
        .map_err(StorageError::ReadInventory)?;
    let mut entries = Vec::new();
    for row in rows {
        let (
            member_id,
            skill_name,
            _description,
            stable_relative_path,
            content_fingerprint,
            bundle_id,
            bundle_display_name,
            managed_directory,
            current_target,
            source_display_name,
        ) = row.map_err(StorageError::ReadInventory)?;
        validate_stored_managed_paths(
            &bundle_id,
            &skill_name,
            &managed_directory,
            &current_target,
            &stable_relative_path,
        )?;
        let installation_chain = read_member_installation_chain_from(connection, &member_id)?;
        let skill_root = data_root
            .join(&managed_directory)
            .join("current")
            .join(&stable_relative_path);
        entries.push(InventoryItem {
            id: format!("managed:{member_id}"),
            declared_name: Some(skill_name.clone()),
            skill_name,
            skill_file: skill_root.join("SKILL.md").to_string_lossy().into_owned(),
            skill_root: skill_root.to_string_lossy().into_owned(),
            location_kind: InventoryLocationKind::ManagedStore,
            metadata_status: SkillMetadataStatus::Valid,
            observed_by: Vec::new(),
            observed_fingerprint: content_fingerprint,
            root_key: None,
            project_id: None,
            stale: false,
            management_kind: ManagementKind::SkillYardManaged,
            management_evidence: None,
            installation_chain,
            takeover_group_id: None,
            takeover_group_display_name: None,
            external_group_display_name: None,
            bundle_id: Some(bundle_id),
            member_id: Some(member_id),
            bundle_display_name: Some(bundle_display_name),
            source_display_name,
            project_display_name: None,
        });
    }
    Ok(entries)
}

fn map_lifecycle_insert_error(error: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::SqliteFailure(code, Some(message)) = &error {
        // 只有单写者 partial unique index 冲突才表示已有活跃事务；主键等约束必须保留原因。
        if (code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
            && message.contains("lifecycle_single_active"))
            || message.contains("active_lifecycle_transaction")
        {
            return StorageError::ActiveLifecycleTransaction;
        }
    }
    StorageError::SaveLifecycleTransaction(error)
}

fn map_bundle_update_batch_insert_error(error: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::SqliteFailure(code, Some(message)) = &error
        && code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
        && message.contains("bundle_update_batch_single_open")
    {
        return StorageError::BundleUpdateBatchAlreadyOpen;
    }
    StorageError::SaveBundleUpdateBatch(error)
}

fn map_mount_transaction_insert_error(error: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::SqliteFailure(code, Some(message)) = &error
        && ((code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
            && message.contains("mount_transaction_single_active"))
            || message.contains("active_lifecycle_transaction"))
    {
        return StorageError::ActiveLifecycleTransaction;
    }
    StorageError::SaveMountTransaction(error)
}

fn map_batch_mount_transaction_insert_error(error: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::SqliteFailure(code, Some(message)) = &error
        && ((code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
            && message.contains("batch_mount_transaction_single_active"))
            || message.contains("active_lifecycle_transaction"))
    {
        return StorageError::ActiveLifecycleTransaction;
    }
    StorageError::SaveBatchMountTransaction(error)
}

fn map_takeover_transaction_insert_error(error: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::SqliteFailure(code, Some(message)) = &error
        && ((code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
            && message.contains("takeover_transaction_single_active"))
            || message.contains("active_lifecycle_transaction"))
    {
        return StorageError::ActiveLifecycleTransaction;
    }
    StorageError::SaveTakeoverTransaction(error)
}

fn map_source_association_transaction_insert_error(error: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::SqliteFailure(code, Some(message)) = &error
        && ((code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
            && message.contains("source_association_transaction_single_active"))
            || message.contains("active_lifecycle_transaction"))
    {
        return StorageError::ActiveLifecycleTransaction;
    }
    StorageError::SaveSourceAssociationTransaction(error)
}

fn map_removal_transaction_insert_error(error: rusqlite::Error) -> StorageError {
    match &error {
        rusqlite::Error::SqliteFailure(_, Some(message))
            if message.contains("active_lifecycle_transaction") =>
        {
            StorageError::ActiveLifecycleTransaction
        }
        _ => StorageError::SaveRemoval(error),
    }
}

fn ensure_install_transaction_can_finalize(
    transaction: &Transaction<'_>,
    transaction_id: &str,
    plan: &StoredInstallPlan,
    anchor_member_id: &str,
) -> Result<(), StorageError> {
    let stored = transaction
        .query_row(
            "SELECT kind, plan_id, bundle_id, member_id, phase, status
             FROM lifecycle_transactions
             WHERE id = ?1",
            [transaction_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::SaveManagedBundle)?;
    let Some((kind, plan_id, bundle_id, member_id, phase, status)) = stored else {
        return Err(StorageError::LifecycleStateConflict(
            transaction_id.to_owned(),
        ));
    };
    let state_can_finalize = (status == "in_progress"
        && matches!(phase.as_str(), "candidate_ready" | "activated"))
        || (status == "completed" && phase == "state_committed");
    if kind != "install_bundle"
        || plan_id != plan.id
        || bundle_id != plan.bundle_id
        || member_id != anchor_member_id
        || !state_can_finalize
    {
        return Err(StorageError::LifecycleStateConflict(
            transaction_id.to_owned(),
        ));
    }
    Ok(())
}

fn finalize_install_create_rows(
    transaction: &Transaction<'_>,
    plan: &StoredInstallPlan,
    selected: &[&StoredInstallCandidate],
    managed_directory: &str,
    current_target: &str,
    now: i64,
) -> Result<(), StorageError> {
    if selected.iter().any(|candidate| candidate.preserve_existing) {
        return Err(StorageError::InvalidInstallPlan);
    }
    transaction
        .execute(
            "INSERT INTO bundles (
                id, display_name, managed_directory, current_target, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO NOTHING",
            params![
                plan.bundle_id,
                plan.bundle_display_name,
                managed_directory,
                current_target,
                now
            ],
        )
        .map_err(StorageError::SaveManagedBundle)?;
    for candidate in selected {
        persist_install_member(transaction, &plan.bundle_id, candidate, now)?;
    }
    if plan.kind == "source_snapshot" {
        let source_id = plan
            .source_id
            .as_deref()
            .ok_or(StorageError::InvalidInstallPlan)?;
        let source_marker = plan
            .source_marker
            .as_deref()
            .ok_or(StorageError::InvalidInstallPlan)?;
        transaction
            .execute(
                "INSERT INTO source_bundle_links (
                    source_id, bundle_id, adopted_marker, linked_at
                 ) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(source_id) DO NOTHING",
                params![source_id, plan.bundle_id, source_marker, now],
            )
            .map_err(StorageError::SaveManagedBundle)?;
        for candidate in selected {
            persist_source_member_link(transaction, source_id, candidate, now)?;
        }
    }
    Ok(())
}

fn finalize_install_supplement_rows(
    transaction: &Transaction<'_>,
    plan: &StoredInstallPlan,
    selected: &[&StoredInstallCandidate],
    managed_directory: &str,
    current_target: &str,
    now: i64,
) -> Result<(), StorageError> {
    if plan.kind != "source_snapshot" {
        return Err(StorageError::InvalidInstallPlan);
    }
    let source_id = plan
        .source_id
        .as_deref()
        .ok_or(StorageError::InvalidInstallPlan)?;
    let expected_current_target = plan
        .expected_current_target
        .as_deref()
        .ok_or(StorageError::InvalidInstallPlan)?;
    let source_baseline = transaction
        .query_row(
            "SELECT bundle_id, adopted_marker
             FROM source_bundle_links
             WHERE source_id = ?1",
            [source_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(StorageError::SaveManagedBundle)?
        .ok_or(StorageError::SourceBundleStateConflict)?;
    if source_baseline != (plan.bundle_id.clone(), plan.expected_adopted_marker.clone()) {
        return Err(StorageError::SourceBundleStateConflict);
    }
    let changed = transaction
        .execute(
            "UPDATE bundles
             SET current_target = ?4
             WHERE id = ?1
               AND display_name = ?2
               AND managed_directory = ?3
               AND current_target IN (?4, ?5)",
            params![
                plan.bundle_id,
                plan.bundle_display_name,
                managed_directory,
                current_target,
                expected_current_target
            ],
        )
        .map_err(StorageError::SaveManagedBundle)?;
    if changed != 1 {
        return Err(StorageError::SourceBundleStateConflict);
    }
    for candidate in selected
        .iter()
        .filter(|candidate| !candidate.preserve_existing)
    {
        persist_install_member(transaction, &plan.bundle_id, candidate, now)?;
        persist_source_member_link(transaction, source_id, candidate, now)?;
    }
    Ok(())
}

fn finalize_install_update_rows(
    transaction: &Transaction<'_>,
    plan: &StoredInstallPlan,
    selected: &[&StoredInstallCandidate],
    managed_directory: &str,
    current_target: &str,
    now: i64,
) -> Result<(), StorageError> {
    if plan.kind != "source_snapshot" {
        return Err(StorageError::InvalidInstallPlan);
    }
    let source_id = plan
        .source_id
        .as_deref()
        .ok_or(StorageError::InvalidInstallPlan)?;
    let target_marker = plan
        .source_marker
        .as_deref()
        .ok_or(StorageError::InvalidInstallPlan)?;
    let expected_source_marker = plan
        .expected_source_marker
        .as_deref()
        .ok_or(StorageError::InvalidInstallPlan)?;
    let expected_current_target = plan
        .expected_current_target
        .as_deref()
        .ok_or(StorageError::InvalidInstallPlan)?;
    let target_generation = plan
        .source_catalog_generation
        .and_then(|generation| generation.checked_add(1))
        .ok_or(StorageError::InvalidInstallPlan)?;

    let source_baseline = transaction
        .query_row(
            "SELECT bundle_id, adopted_marker
             FROM source_bundle_links
             WHERE source_id = ?1",
            [source_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(StorageError::SaveManagedBundle)?
        .ok_or(StorageError::SourceBundleStateConflict)?;
    if source_baseline.0 != plan.bundle_id
        || (source_baseline.1 != plan.expected_adopted_marker
            && source_baseline.1.as_deref() != Some(target_marker))
    {
        return Err(StorageError::SourceBundleStateConflict);
    }
    let source_catalog = transaction
        .query_row(
            "SELECT catalog_generation, catalog_marker FROM sources WHERE id = ?1",
            [source_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .map_err(StorageError::SaveManagedBundle)?;
    let old_catalog = (
        plan.source_catalog_generation
            .ok_or(StorageError::InvalidInstallPlan)?,
        Some(expected_source_marker.to_owned()),
    );
    let new_catalog = (target_generation, Some(target_marker.to_owned()));
    if source_catalog != old_catalog && source_catalog != new_catalog {
        return Err(StorageError::SourceCatalogStateChanged);
    }

    let changed = transaction
        .execute(
            "UPDATE bundles
             SET current_target = ?4
             WHERE id = ?1
               AND display_name = ?2
               AND managed_directory = ?3
               AND current_target IN (?4, ?5)",
            params![
                plan.bundle_id,
                plan.bundle_display_name,
                managed_directory,
                current_target,
                expected_current_target
            ],
        )
        .map_err(StorageError::SaveManagedBundle)?;
    if changed != 1 {
        return Err(StorageError::SourceBundleStateConflict);
    }

    for candidate in selected {
        if let Some(previous_fingerprint) = candidate.previous_content_fingerprint.as_deref() {
            let skill_name = candidate
                .skill_name
                .as_deref()
                .ok_or(StorageError::InvalidInstallSelection)?;
            let description = candidate
                .skill_description
                .as_deref()
                .ok_or(StorageError::InvalidInstallSelection)?;
            let fingerprint = candidate
                .content_fingerprint
                .as_deref()
                .ok_or(StorageError::InvalidInstallSelection)?;
            let updated = transaction
                .execute(
                    "UPDATE skill_members
                     SET description = ?4, content_fingerprint = ?5
                     WHERE id = ?1
                       AND bundle_id = ?2
                       AND skill_name = ?3
                       AND stable_relative_path = ?6
                       AND content_fingerprint IN (?5, ?7)",
                    params![
                        candidate.candidate_id,
                        plan.bundle_id,
                        skill_name,
                        description,
                        fingerprint,
                        format!("members/{skill_name}"),
                        previous_fingerprint
                    ],
                )
                .map_err(StorageError::SaveManagedBundle)?;
            if updated != 1 {
                return Err(StorageError::SourceBundleStateConflict);
            }
        } else {
            persist_install_member(transaction, &plan.bundle_id, candidate, now)?;
        }
        transaction
            .execute(
                "INSERT INTO member_selections (bundle_id, member_id, selected_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(bundle_id, member_id) DO NOTHING",
                params![plan.bundle_id, candidate.candidate_id, now],
            )
            .map_err(StorageError::SaveManagedBundle)?;
    }

    transaction
        .execute(
            "DELETE FROM source_catalog_members WHERE source_id = ?1",
            [source_id],
        )
        .map_err(StorageError::SaveManagedBundle)?;
    for (sort_order, candidate) in selected
        .iter()
        .filter(|candidate| !candidate.preserve_existing)
        .enumerate()
    {
        let validation_errors = serde_json::to_string(&candidate.validation_errors)
            .map_err(StorageError::InvalidPlanValidationErrors)?;
        let warnings = serde_json::to_string(&candidate.warnings)
            .map_err(StorageError::InvalidPlanWarnings)?;
        transaction
            .execute(
                "INSERT INTO source_catalog_members (
                    id, source_id, catalog_generation, relative_path, skill_name,
                    description, content_fingerprint, selectable,
                    validation_errors_json, warnings_json, sort_order
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?10)",
                params![
                    candidate.candidate_id,
                    source_id,
                    target_generation,
                    candidate.source_relative_path,
                    candidate.skill_name,
                    candidate.skill_description,
                    candidate.content_fingerprint,
                    validation_errors,
                    warnings,
                    sort_order as i64
                ],
            )
            .map_err(StorageError::SaveManagedBundle)?;
    }
    let source_changed = transaction
        .execute(
            "UPDATE sources
             SET catalog_status = 'fresh',
                 catalog_generation = ?2,
                 catalog_marker = ?3,
                 catalog_fetched_at = ?4,
                 last_reload_at = ?4,
                 last_reload_error = NULL,
                 updated_at = ?4
             WHERE id = ?1
               AND (
                   (catalog_generation = ?5 AND catalog_marker = ?6)
                   OR (catalog_generation = ?2 AND catalog_marker = ?3)
               )",
            params![
                source_id,
                target_generation,
                target_marker,
                now,
                plan.source_catalog_generation,
                expected_source_marker
            ],
        )
        .map_err(StorageError::SaveManagedBundle)?;
    if source_changed != 1 {
        return Err(StorageError::SourceCatalogStateChanged);
    }

    transaction
        .execute(
            "DELETE FROM source_member_links WHERE source_id = ?1",
            [source_id],
        )
        .map_err(StorageError::SaveManagedBundle)?;
    for candidate in selected {
        persist_source_member_link(transaction, source_id, candidate, now)?;
    }
    let linked = transaction
        .execute(
            "UPDATE source_bundle_links
             SET adopted_marker = ?3,
                 update_check_status = 'up_to_date',
                 update_checked_marker = ?3,
                 update_checked_at = ?4,
                 update_check_error = NULL
             WHERE source_id = ?1
               AND bundle_id = ?2
               AND (adopted_marker IS ?5 OR adopted_marker = ?3)",
            params![
                source_id,
                plan.bundle_id,
                target_marker,
                now,
                plan.expected_adopted_marker
            ],
        )
        .map_err(StorageError::SaveManagedBundle)?;
    if linked != 1 {
        return Err(StorageError::SourceBundleStateConflict);
    }
    Ok(())
}

fn persist_install_member(
    transaction: &Transaction<'_>,
    bundle_id: &str,
    candidate: &StoredInstallCandidate,
    now: i64,
) -> Result<(), StorageError> {
    let skill_name = candidate
        .skill_name
        .as_deref()
        .ok_or(StorageError::InvalidInstallSelection)?;
    let description = candidate
        .skill_description
        .as_deref()
        .ok_or(StorageError::InvalidInstallSelection)?;
    let fingerprint = candidate
        .content_fingerprint
        .as_deref()
        .ok_or(StorageError::InvalidInstallSelection)?;
    let stable_path = format!("members/{skill_name}");
    transaction
        .execute(
            "INSERT INTO skill_members (
                id, bundle_id, skill_name, description, stable_relative_path,
                content_fingerprint, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO NOTHING",
            params![
                candidate.candidate_id,
                bundle_id,
                skill_name,
                description,
                stable_path,
                fingerprint,
                now
            ],
        )
        .map_err(StorageError::SaveManagedBundle)?;
    transaction
        .execute(
            "INSERT INTO member_selections (bundle_id, member_id, selected_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(bundle_id, member_id) DO NOTHING",
            params![bundle_id, candidate.candidate_id, now],
        )
        .map_err(StorageError::SaveManagedBundle)?;
    Ok(())
}

fn persist_source_member_link(
    transaction: &Transaction<'_>,
    source_id: &str,
    candidate: &StoredInstallCandidate,
    now: i64,
) -> Result<(), StorageError> {
    let Some(source_relative_path) = candidate.source_relative_path.as_deref() else {
        // “不对应”的保留成员属于完整 Bundle，但不制造假的上游路径。
        return Ok(());
    };
    transaction
        .execute(
            "INSERT INTO source_member_links (
                source_id, source_relative_path, member_id, linked_at
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(source_id, source_relative_path) DO NOTHING",
            params![source_id, source_relative_path, candidate.candidate_id, now],
        )
        .map_err(StorageError::SaveManagedBundle)?;
    Ok(())
}

fn ensure_source_install_state_matches(
    transaction: &Transaction<'_>,
    plan: &StoredInstallPlan,
    selected: &[&StoredInstallCandidate],
) -> Result<(), StorageError> {
    if plan.kind == "folder_snapshot" {
        return Ok(());
    }
    let source_id = plan
        .source_id
        .as_deref()
        .ok_or(StorageError::InvalidInstallPlan)?;
    let expected_adopted_marker = if matches!(plan.install_mode.as_str(), "create" | "update") {
        plan.source_marker.clone()
    } else {
        plan.expected_adopted_marker.clone()
    };
    let source_bundle = transaction
        .query_row(
            "SELECT bundle_id, adopted_marker
             FROM source_bundle_links
             WHERE source_id = ?1",
            [source_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(StorageError::SaveManagedBundle)?;
    if source_bundle != Some((plan.bundle_id.clone(), expected_adopted_marker)) {
        return Err(StorageError::ManagedStateConflict);
    }
    let linked_candidates = selected
        .iter()
        .filter_map(|candidate| {
            candidate
                .source_relative_path
                .as_deref()
                .map(|path| (*candidate, path))
        })
        .collect::<Vec<_>>();
    let link_count = transaction
        .query_row(
            "SELECT COUNT(*) FROM source_member_links WHERE source_id = ?1",
            [source_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(StorageError::SaveManagedBundle)?;
    if link_count != linked_candidates.len() as i64 {
        return Err(StorageError::ManagedStateConflict);
    }
    for (candidate, source_relative_path) in linked_candidates {
        let linked_path = transaction
            .query_row(
                "SELECT source_relative_path
                 FROM source_member_links
                 WHERE source_id = ?1 AND member_id = ?2",
                params![source_id, candidate.candidate_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StorageError::SaveManagedBundle)?;
        if linked_path.as_deref() != Some(source_relative_path) {
            return Err(StorageError::ManagedStateConflict);
        }
    }
    Ok(())
}

fn ensure_managed_state_matches(
    transaction: &Transaction<'_>,
    plan: &StoredInstallPlan,
    selected: &[&StoredInstallCandidate],
    managed_directory: &str,
    current_target: &str,
) -> Result<(), StorageError> {
    let bundle = transaction
        .query_row(
            "SELECT display_name, managed_directory, current_target FROM bundles WHERE id = ?1",
            [&plan.bundle_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(StorageError::SaveManagedBundle)?;
    let bundle_matches = bundle
        == (
            plan.bundle_display_name.clone(),
            managed_directory.to_owned(),
            current_target.to_owned(),
        );
    if !bundle_matches {
        return Err(StorageError::ManagedStateConflict);
    }
    let actual_count = transaction
        .query_row(
            "SELECT COUNT(*) FROM skill_members WHERE bundle_id = ?1",
            [&plan.bundle_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(StorageError::SaveManagedBundle)?;
    if actual_count != selected.len() as i64 {
        return Err(StorageError::ManagedStateConflict);
    }
    for candidate in selected {
        let skill_name = candidate
            .skill_name
            .as_ref()
            .ok_or(StorageError::InvalidInstallSelection)?;
        let description = candidate
            .skill_description
            .as_ref()
            .ok_or(StorageError::InvalidInstallSelection)?;
        let fingerprint = candidate
            .content_fingerprint
            .as_ref()
            .ok_or(StorageError::InvalidInstallSelection)?;
        let member = transaction
            .query_row(
                "SELECT bundle_id, skill_name, description, stable_relative_path, content_fingerprint FROM skill_members WHERE id = ?1",
                [&candidate.candidate_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .map_err(StorageError::SaveManagedBundle)?;
        let selected_row = transaction
            .query_row(
                "SELECT COUNT(*) FROM member_selections WHERE bundle_id = ?1 AND member_id = ?2",
                params![plan.bundle_id, candidate.candidate_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StorageError::SaveManagedBundle)?;
        if member
            != (
                plan.bundle_id.clone(),
                skill_name.clone(),
                description.clone(),
                format!("members/{skill_name}"),
                fingerprint.clone(),
            )
            || selected_row != 1
        {
            return Err(StorageError::ManagedStateConflict);
        }
    }
    Ok(())
}

fn insert_inventory_installation_chain(
    transaction: &Transaction<'_>,
    observation_id: &str,
    chain: &InstallationChain,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO inventory_installation_chains (
            observation_id, kind, record_path, source, source_type, source_locator,
            skill_path, tracked_ref, content_marker, installed_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            observation_id,
            chain.kind.as_str(),
            chain.record_path,
            chain.source,
            chain.source_type,
            chain.source_locator,
            chain.skill_path,
            chain.tracked_ref,
            chain.content_marker,
            chain.installed_at,
            chain.updated_at
        ],
    )?;
    Ok(())
}

fn insert_member_installation_chain(
    transaction: &Transaction<'_>,
    member_id: &str,
    chain: &InstallationChain,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO member_installation_chains (
            member_id, kind, record_path, source, source_type, source_locator,
            skill_path, tracked_ref, content_marker, installed_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            member_id,
            chain.kind.as_str(),
            chain.record_path,
            chain.source,
            chain.source_type,
            chain.source_locator,
            chain.skill_path,
            chain.tracked_ref,
            chain.content_marker,
            chain.installed_at,
            chain.updated_at
        ],
    )?;
    Ok(())
}

fn replace_inventory_rows(
    transaction: &Transaction<'_>,
    entries: &[InventoryObservation],
    supported_apps: &[SupportedAppSummary],
    scan_issues: &[ScanIssue],
) -> rusqlite::Result<()> {
    transaction.execute("DELETE FROM inventory_observation_apps", [])?;
    transaction.execute("DELETE FROM inventory_observations", [])?;
    transaction.execute("DELETE FROM supported_app_status", [])?;
    transaction.execute("DELETE FROM inventory_scan_issues", [])?;

    for (sort_order, app) in supported_apps.iter().enumerate() {
        transaction.execute(
            "INSERT INTO supported_app_status (app_id, display_name, detected, sort_order) VALUES (?1, ?2, ?3, ?4)",
            params![app.id.as_str(), app.display_name, app.detected.unwrap_or(false), sort_order as i64],
        )?;
    }

    for entry in entries {
        transaction.execute(
            "INSERT INTO inventory_observations (id, skill_name, declared_name, skill_root, skill_file, location_kind, metadata_status, observed_fingerprint, root_key, project_id, stale, management_kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                entry.id,
                entry.skill_name,
                entry.declared_name,
                entry.skill_root,
                entry.skill_file,
                entry.location_kind.as_str(),
                entry.metadata_status.as_str(),
                entry.observed_fingerprint,
                entry.root_key.as_str(),
                entry.project_id,
                entry.stale,
                entry.management_kind.as_str()
            ],
        )?;
        for app in &entry.observed_by {
            transaction.execute(
                "INSERT INTO inventory_observation_apps (observation_id, app_id) VALUES (?1, ?2)",
                params![entry.id, app.as_str()],
            )?;
        }
        if let Some(evidence) = &entry.management_evidence {
            transaction.execute(
                "INSERT INTO inventory_management_evidence
                 (observation_id, kind, authority_root, snapshot_commit_oid, subject_path)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    entry.id,
                    evidence.kind.as_str(),
                    evidence.authority_root,
                    evidence.snapshot_commit_oid,
                    evidence.subject_path
                ],
            )?;
        }
        if let Some(chain) = &entry.installation_chain {
            insert_inventory_installation_chain(transaction, &entry.id, chain)?;
        }
    }

    for issue in scan_issues {
        transaction.execute(
            "INSERT INTO inventory_scan_issues (root_id, root_key, project_id, path, code, message) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                issue.root_id,
                issue.root_key.as_str(),
                issue.project_id,
                issue.path,
                issue.code.as_str(),
                issue.message
            ],
        )?;
    }
    Ok(())
}

fn reconcile_entries(
    previous: &[InventoryObservation],
    scanned: &[InventoryObservation],
    successful_roots: &[ScanRootIdentity],
    scan_issues: &[ScanIssue],
) -> Vec<InventoryObservation> {
    let successful = successful_roots.iter().cloned().collect::<BTreeSet<_>>();
    let root_failures = scan_issues
        .iter()
        .filter(|issue| !issue.is_entry_scoped())
        .map(scan_issue_identity)
        .collect::<BTreeSet<_>>();
    let entry_failures = scan_issues
        .iter()
        .filter(|issue| issue.is_entry_scoped())
        .map(|issue| (scan_issue_identity(issue), issue.path.clone()))
        .collect::<BTreeSet<_>>();
    let previous_by_id = previous
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut combined = previous
        .iter()
        .filter_map(|entry| {
            let identity = observation_root_identity(entry);
            if root_failures.contains(&identity)
                || entry_failures.contains(&(identity.clone(), entry.skill_root.clone()))
            {
                let mut entry = entry.clone();
                entry.stale = true;
                return Some(entry);
            }
            (!successful.contains(&identity)).then(|| entry.clone())
        })
        .collect::<Vec<_>>();
    combined.extend(scanned.iter().cloned().map(|mut entry| {
        // AgentManaged 由独立识别器负责；Git HEAD 证据只重算项目候选与 ProjectManaged。
        if let Some(previous_entry) = previous_by_id.get(entry.id.as_str())
            && (previous_entry.management_kind == ManagementKind::AgentManaged
                || previous_entry.management_kind == ManagementKind::SkillYardManaged
                || entry.project_id.is_none())
        {
            entry.management_kind = previous_entry.management_kind;
            entry.management_evidence = previous_entry.management_evidence.clone();
        }
        entry
    }));
    combined.sort_by(|left, right| left.skill_root.cmp(&right.skill_root));
    combined
}

fn reconcile_scan_issues(
    previous: &[ScanIssue],
    successful_roots: &[ScanRootIdentity],
    current: &[ScanIssue],
) -> Vec<ScanIssue> {
    let mut touched = successful_roots.iter().cloned().collect::<BTreeSet<_>>();
    touched.extend(current.iter().map(scan_issue_identity));
    let mut combined = previous
        .iter()
        .filter(|issue| !touched.contains(&scan_issue_identity(issue)))
        .cloned()
        .map(|issue| (issue.id.clone(), issue))
        .collect::<BTreeMap<_, _>>();
    for issue in current {
        combined.insert(issue.id.clone(), issue.clone());
    }
    combined.into_values().collect()
}

fn observation_root_identity(entry: &InventoryObservation) -> ScanRootIdentity {
    ScanRootIdentity {
        root_key: entry.root_key,
        project_id: entry.project_id.clone(),
    }
}

fn scan_issue_identity(issue: &ScanIssue) -> ScanRootIdentity {
    ScanRootIdentity {
        root_key: issue.root_key,
        project_id: issue.project_id.clone(),
    }
}

fn reconcile_supported_apps(
    previous: &[SupportedAppSummary],
    scanned: &[SupportedAppSummary],
) -> Vec<SupportedAppSummary> {
    let mut combined = previous.to_vec();
    for app in scanned {
        if let Some(index) = combined.iter().position(|current| current.id == app.id) {
            combined[index] = app.clone();
        } else {
            combined.push(app.clone());
        }
    }
    combined.sort_by_key(|app| match app.id {
        SupportedAppId::Codex => 0,
        SupportedAppId::ClaudeCode => 1,
        SupportedAppId::GitHubCopilot => 2,
    });
    combined
}

fn summarize_changes(
    completed_at: i64,
    previous: &[InventoryObservation],
    current: &[InventoryObservation],
) -> LocalRefreshSummary {
    let previous_by_id = previous
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let current_by_id = current
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let added = current_by_id
        .keys()
        .filter(|id| !previous_by_id.contains_key(*id))
        .count();
    let removed = previous_by_id
        .keys()
        .filter(|id| !current_by_id.contains_key(*id))
        .count();
    let changed = current_by_id
        .iter()
        .filter(|(id, current_entry)| {
            previous_by_id
                .get(*id)
                .is_some_and(|previous_entry| observation_changed(previous_entry, current_entry))
        })
        .count();

    LocalRefreshSummary {
        completed_at,
        added,
        changed,
        removed,
    }
}

fn observation_changed(previous: &InventoryObservation, current: &InventoryObservation) -> bool {
    previous.skill_name != current.skill_name
        || previous.declared_name != current.declared_name
        || previous.skill_file != current.skill_file
        || previous.location_kind != current.location_kind
        || previous.metadata_status != current.metadata_status
        || previous.observed_by != current.observed_by
        || previous.observed_fingerprint != current.observed_fingerprint
        || previous.root_key != current.root_key
        || previous.project_id != current.project_id
        || previous.management_kind != current.management_kind
        || previous.installation_chain != current.installation_chain
}

fn refresh_count(value: i64) -> Result<usize, StorageError> {
    usize::try_from(value).map_err(|_| StorageError::InvalidRefreshCount(value))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::symlink,
        path::Path,
        sync::{Arc, Barrier},
    };

    use tempfile::tempdir;

    use super::*;
    use crate::domain::{
        TakeoverIdentityBasis, TakeoverOriginDisposition, TakeoverPlanMember, TakeoverPlanOrigin,
        TakeoverPlanTarget,
    };

    fn open_test_storage(root: &Path) -> Storage {
        let data_root = root.join("data");
        let database = data_root.join("skillyard.sqlite3");
        Storage::open(&data_root, &database).expect("应打开隔离 SQLite")
    }

    const TEST_GITHUB_SOURCE_ID: &str = "source-anthropics-skills";
    const TEST_GITHUB_COMMIT_ONE: &str = "1111111111111111111111111111111111111111";
    const TEST_GITHUB_COMMIT_TWO: &str = "2222222222222222222222222222222222222222";

    fn takeover_plan_for(storage: &Storage, root: &Path, suffix: &str) -> TakeoverPlan {
        let skill_name = format!("skill-{suffix}");
        let bundle_id = format!("takeover-bundle-{suffix}");
        let member_id = format!("takeover-member-{suffix}");
        let content_id = format!("takeover-content-{suffix}");
        let observation_id = format!("takeover-observation-{suffix}");
        let mount_id = format!("takeover-mount-{suffix}");
        let original_path = root.join("home/.codex/skills").join(&skill_name);
        let managed_directory = storage.data_root.join("bundles").join(&bundle_id);
        let content_directory = managed_directory.join("contents").join(&content_id);
        let expected_target = managed_directory.join("current/members").join(&skill_name);
        TakeoverPlan {
            id: format!("takeover-plan-{suffix}"),
            bundle_id,
            content_id,
            bundle_display_name: format!("Takeover {suffix}"),
            source_display_name: None,
            managed_directory: managed_directory.to_string_lossy().into_owned(),
            content_directory: content_directory.to_string_lossy().into_owned(),
            retained_members: Vec::new(),
            members: vec![TakeoverPlanMember {
                member_id: member_id.clone(),
                identity_basis: TakeoverIdentityBasis::SingleOrigin,
                selected_observation_id: observation_id.clone(),
                skill_name: skill_name.clone(),
                skill_description: "接管持久化测试 Skill".to_owned(),
                installation_chain: None,
                expected_target: expected_target.to_string_lossy().into_owned(),
                warnings: Vec::new(),
            }],
            origins: vec![TakeoverPlanOrigin {
                member_id: member_id.clone(),
                observation_id,
                original_path: original_path.to_string_lossy().into_owned(),
                app_id: Some(SupportedAppId::Codex),
                scope: Some(MountScope::Global),
                project_id: None,
                project_display_name: None,
                content_fingerprint: format!("sha256:{suffix}"),
                warnings: Vec::new(),
                final_disposition: TakeoverOriginDisposition::Mount,
            }],
            targets: vec![TakeoverPlanTarget {
                member_id,
                mount_id,
                app_id: SupportedAppId::Codex,
                scope: MountScope::Global,
                project_id: None,
                project_display_name: None,
                target_path: original_path.to_string_lossy().into_owned(),
                expected_target: expected_target.to_string_lossy().into_owned(),
            }],
            warnings: Vec::new(),
            created_at: 100,
            expires_at: 1_000,
        }
    }

    fn begin_test_takeover(storage: &mut Storage, plan: &TakeoverPlan, transaction_id: &str) {
        storage
            .connection
            .execute(
                "INSERT INTO inventory_observations (
                    id, skill_name, declared_name, skill_root, skill_file, location_kind,
                    metadata_status, observed_fingerprint, root_key, project_id, stale,
                    management_kind
                 ) VALUES (?1, ?2, ?2, ?3, ?4, 'app_global', 'ready', ?5,
                           'codex_global', NULL, 0, 'takeover_candidate')",
                params![
                    plan.origins[0].observation_id,
                    plan.members[0].skill_name,
                    plan.origins[0].original_path,
                    Path::new(&plan.origins[0].original_path)
                        .join("SKILL.md")
                        .to_string_lossy(),
                    plan.origins[0].content_fingerprint,
                ],
            )
            .expect("应建立待接管 Inventory 观察");
        storage
            .save_takeover_plan(&StoredTakeoverPlanRow {
                id: plan.id.clone(),
                payload_json: "{}".to_owned(),
                payload_sha256: "0".repeat(64),
                status: "pending".to_owned(),
                created_at: plan.created_at,
                expires_at: plan.expires_at,
            })
            .expect("应保存测试 Takeover Plan");
        storage
            .begin_takeover_transaction(
                &plan.id,
                transaction_id,
                &plan.bundle_id,
                &plan.members[0].member_id,
                &takeover_reserved_paths(plan).expect("测试 Plan 应产生合法保留路径"),
                &format!("journals/{transaction_id}.json"),
                200,
            )
            .expect("应开始 Takeover 事务");
        for (phase, now) in [
            ("journal_ready", 201),
            ("candidate_ready", 202),
            ("current_activated", 203),
            ("origins_applied", 204),
        ] {
            storage
                .update_takeover_transaction_phase(transaction_id, phase, now)
                .expect("应按顺序推进 Takeover 阶段");
        }
    }

    fn save_test_plan(storage: &mut Storage, plan_id: &str, bundle_id: &str, member_id: &str) {
        let candidates = [NewInstallCandidate {
            candidate_id: member_id,
            source_relative_path: Some(""),
            skill_name: Some(member_id),
            skill_description: Some("测试 Skill"),
            content_fingerprint: Some("sha256:test"),
            previous_content_fingerprint: None,
            selectable: true,
            preserve_existing: false,
            validation_errors: &[],
            warnings: &[],
            default_selected: true,
        }];
        storage
            .save_install_plan(NewInstallPlan {
                id: plan_id,
                kind: "folder_snapshot",
                install_mode: "create",
                input_path: Some("/tmp/example-skill"),
                input_device: 1,
                input_inode: 2,
                input_fingerprint: "sha256:test",
                snapshot_relative_path: None,
                source_id: None,
                source_tracked_ref: None,
                source_catalog_generation: None,
                source_marker: None,
                expected_source_marker: None,
                expected_current_target: None,
                expected_adopted_marker: None,
                bundle_id,
                bundle_display_name: bundle_id,
                warnings: &[],
                candidates: &candidates,
                created_at: 100,
                expires_at: 1_000,
            })
            .expect("应保存安装 Plan");
    }

    fn save_test_github_catalog(
        storage: &mut Storage,
        alpha_id: &str,
        beta_id: &str,
        commit_sha: &str,
        alpha_fingerprint: &str,
        fetched_at: i64,
    ) {
        let members = [
            NewSourceCatalogMember {
                id: alpha_id,
                relative_path: "skills/alpha",
                skill_name: Some("alpha"),
                description: Some("Alpha Skill"),
                content_fingerprint: Some(alpha_fingerprint),
                selectable: true,
                validation_errors: &[],
                warnings: &[],
            },
            NewSourceCatalogMember {
                id: beta_id,
                relative_path: "skills/beta",
                skill_name: Some("beta"),
                description: Some("Beta Skill"),
                content_fingerprint: Some("sha256:beta"),
                selectable: true,
                validation_errors: &[],
                warnings: &[],
            },
        ];
        storage
            .save_source_catalog_success(
                TEST_GITHUB_SOURCE_ID,
                "main",
                commit_sha,
                fetched_at,
                &members,
            )
            .expect("应保存 Fresh GitHub Catalog");
    }

    fn save_test_github_create_plan(storage: &mut Storage, plan_id: &str, bundle_id: &str) {
        let snapshot_relative_path = format!("staging/.install-plan-{plan_id}/skills");
        let candidates = [
            NewInstallCandidate {
                candidate_id: "catalog-alpha-v1",
                source_relative_path: Some("skills/alpha"),
                skill_name: Some("alpha"),
                skill_description: Some("Alpha Skill"),
                content_fingerprint: Some("sha256:alpha-v1"),
                previous_content_fingerprint: None,
                selectable: true,
                preserve_existing: false,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
            NewInstallCandidate {
                candidate_id: "catalog-beta-v1",
                source_relative_path: Some("skills/beta"),
                skill_name: Some("beta"),
                skill_description: Some("Beta Skill"),
                content_fingerprint: Some("sha256:beta"),
                previous_content_fingerprint: None,
                selectable: true,
                preserve_existing: false,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
        ];
        storage
            .save_install_plan(NewInstallPlan {
                id: plan_id,
                kind: "source_snapshot",
                install_mode: "create",
                input_path: None,
                input_device: 10,
                input_inode: 20,
                input_fingerprint: "sha256:github-snapshot-v1",
                snapshot_relative_path: Some(&snapshot_relative_path),
                source_id: Some(TEST_GITHUB_SOURCE_ID),
                source_tracked_ref: Some("main"),
                source_catalog_generation: Some(1),
                source_marker: Some(TEST_GITHUB_COMMIT_ONE),
                expected_source_marker: None,
                expected_current_target: None,
                expected_adopted_marker: None,
                bundle_id,
                bundle_display_name: "anthropics/skills",
                warnings: &[],
                candidates: &candidates,
                created_at: 100,
                expires_at: 1_000,
            })
            .expect("应保存 GitHub create Plan");
    }

    fn save_test_github_supplement_plan(
        storage: &mut Storage,
        plan_id: &str,
        bundle_id: &str,
        expected_current_target: &str,
    ) {
        let snapshot_relative_path = format!("staging/.install-plan-{plan_id}/skills");
        let candidates = [
            NewInstallCandidate {
                candidate_id: "catalog-alpha-v1",
                source_relative_path: Some("skills/alpha"),
                skill_name: Some("alpha"),
                skill_description: Some("Alpha Skill"),
                content_fingerprint: Some("sha256:alpha-v1"),
                previous_content_fingerprint: Some("sha256:alpha-v1"),
                selectable: false,
                preserve_existing: true,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
            NewInstallCandidate {
                candidate_id: "catalog-beta-v2",
                source_relative_path: Some("skills/beta"),
                skill_name: Some("beta"),
                skill_description: Some("Beta Skill"),
                content_fingerprint: Some("sha256:beta"),
                previous_content_fingerprint: None,
                selectable: true,
                preserve_existing: false,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
        ];
        storage
            .save_install_plan(NewInstallPlan {
                id: plan_id,
                kind: "source_snapshot",
                install_mode: "supplement",
                input_path: None,
                input_device: 11,
                input_inode: 21,
                input_fingerprint: "sha256:github-snapshot-v2",
                snapshot_relative_path: Some(&snapshot_relative_path),
                source_id: Some(TEST_GITHUB_SOURCE_ID),
                source_tracked_ref: Some("main"),
                source_catalog_generation: Some(2),
                source_marker: Some(TEST_GITHUB_COMMIT_TWO),
                expected_source_marker: None,
                expected_current_target: Some(expected_current_target),
                expected_adopted_marker: Some(TEST_GITHUB_COMMIT_ONE),
                bundle_id,
                bundle_display_name: "anthropics/skills",
                warnings: &[],
                candidates: &candidates,
                created_at: 300,
                expires_at: 2_000,
            })
            .expect("应保存 GitHub supplement Plan");
    }

    fn create_test_github_bundle_with_alpha(
        storage: &mut Storage,
        plan_id: &str,
        transaction_id: &str,
        bundle_id: &str,
    ) {
        save_test_github_catalog(
            storage,
            "catalog-alpha-v1",
            "catalog-beta-v1",
            TEST_GITHUB_COMMIT_ONE,
            "sha256:alpha-v1",
            50,
        );
        save_test_github_create_plan(storage, plan_id, bundle_id);
        let plan = storage
            .begin_install_transaction_with_selection(
                plan_id,
                &["catalog-alpha-v1".to_owned()],
                transaction_id,
                &format!("journals/{transaction_id}.json"),
                200,
            )
            .expect("应开始 GitHub create 事务");
        advance_to_candidate_ready(storage, transaction_id);
        storage
            .finalize_install(
                transaction_id,
                &plan,
                &format!("bundles/{bundle_id}"),
                &format!("contents/{transaction_id}"),
                "members/alpha",
                203,
            )
            .expect("应创建 Source-backed Bundle");
        storage
            .forget_terminal_transaction(transaction_id)
            .expect("应清理 GitHub create 事务");
    }

    fn advance_to_candidate_ready(storage: &mut Storage, transaction_id: &str) {
        storage
            .update_lifecycle_phase(transaction_id, "journal_ready", 201)
            .expect("应记录 Journal 已就绪");
        storage
            .update_lifecycle_phase(transaction_id, "candidate_ready", 202)
            .expect("应记录候选内容已就绪");
    }

    fn save_test_managed_member(storage: &mut Storage, suffix: &str) -> StoredManagedMember {
        let plan_id = format!("install-plan-{suffix}");
        let bundle_id = format!("bundle-{suffix}");
        let member_id = format!("member-{suffix}");
        let transaction_id = format!("install-txn-{suffix}");
        save_test_plan(storage, &plan_id, &bundle_id, &member_id);
        let plan = storage
            .begin_install_transaction(
                &plan_id,
                &transaction_id,
                &format!("journals/{transaction_id}.json"),
                200,
            )
            .expect("应开始测试安装事务");
        advance_to_candidate_ready(storage, &transaction_id);
        storage
            .finalize_install(
                &transaction_id,
                &plan,
                &format!("bundles/{bundle_id}"),
                &format!("contents/{transaction_id}"),
                &format!("members/{member_id}"),
                203,
            )
            .expect("应建立测试受管成员");
        storage
            .forget_terminal_transaction(&transaction_id)
            .expect("测试安装事务应完成清理");
        storage
            .read_managed_member(&member_id)
            .expect("应读取测试受管成员")
    }

    fn save_test_managed_bundle(storage: &mut Storage, suffix: &str) -> Vec<StoredManagedMember> {
        let plan_id = format!("install-plan-batch-{suffix}");
        let bundle_id = format!("bundle-batch-{suffix}");
        let first_id = format!("member-alpha-{suffix}");
        let second_id = format!("member-beta-{suffix}");
        let first_name = format!("alpha-{suffix}");
        let second_name = format!("beta-{suffix}");
        let candidates = [
            NewInstallCandidate {
                candidate_id: &first_id,
                source_relative_path: Some(&first_name),
                skill_name: Some(&first_name),
                skill_description: Some("第一个测试 Skill"),
                content_fingerprint: Some("sha256:alpha"),
                previous_content_fingerprint: None,
                selectable: true,
                preserve_existing: false,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
            NewInstallCandidate {
                candidate_id: &second_id,
                source_relative_path: Some(&second_name),
                skill_name: Some(&second_name),
                skill_description: Some("第二个测试 Skill"),
                content_fingerprint: Some("sha256:beta"),
                previous_content_fingerprint: None,
                selectable: true,
                preserve_existing: false,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
        ];
        storage
            .save_install_plan(NewInstallPlan {
                id: &plan_id,
                kind: "folder_snapshot",
                install_mode: "create",
                input_path: Some("/tmp/batch-bundle"),
                input_device: 1,
                input_inode: 2,
                input_fingerprint: "sha256:bundle",
                snapshot_relative_path: None,
                source_id: None,
                source_tracked_ref: None,
                source_catalog_generation: None,
                source_marker: None,
                expected_source_marker: None,
                expected_current_target: None,
                expected_adopted_marker: None,
                bundle_id: &bundle_id,
                bundle_display_name: &format!("Batch {suffix}"),
                warnings: &[],
                candidates: &candidates,
                created_at: 100,
                expires_at: 1_000,
            })
            .expect("应保存多成员安装 Plan");
        let selected = vec![first_id.clone(), second_id.clone()];
        let transaction_id = format!("install-txn-batch-{suffix}");
        let plan = storage
            .begin_install_transaction_with_selection(
                &plan_id,
                &selected,
                &transaction_id,
                &format!("journals/{transaction_id}.json"),
                200,
            )
            .expect("应开始多成员安装事务");
        advance_to_candidate_ready(storage, &transaction_id);
        storage
            .finalize_install(
                &transaction_id,
                &plan,
                &format!("bundles/{bundle_id}"),
                &format!("contents/{transaction_id}"),
                &format!("members/{first_name}"),
                203,
            )
            .expect("应建立多成员测试 Bundle");
        storage
            .forget_terminal_transaction(&transaction_id)
            .expect("多成员安装事务应完成清理");
        selected
            .iter()
            .map(|member_id| {
                storage
                    .read_managed_member(member_id)
                    .expect("应读取多成员测试 Bundle")
            })
            .collect()
    }

    fn save_test_source_association_plan(
        storage: &mut Storage,
        plan_id: &str,
        created_at: i64,
        expires_at: i64,
    ) {
        storage
            .save_source_association_plan(&StoredSourceAssociationPlanRow {
                id: plan_id.to_owned(),
                payload_json: "{}".to_owned(),
                payload_sha256: "0".repeat(64),
                status: "pending".to_owned(),
                created_at,
                expires_at,
            })
            .expect("应保存 Source 关联 Plan");
    }

    fn setup_test_source_association_merge(
        storage: &mut Storage,
        suffix: &str,
    ) -> (StoredSourceAssociationBundle, StoredSourceAssociationBundle) {
        save_test_github_catalog(
            storage,
            "catalog-alpha-v1",
            "catalog-beta-v1",
            TEST_GITHUB_COMMIT_ONE,
            "sha256:alpha-v1",
            50,
        );
        let target_members = save_test_managed_bundle(storage, &format!("merge-target-{suffix}"));
        let target_id = target_members[0].bundle_id.clone();
        let target = storage
            .read_source_association_bundle(&target_id)
            .expect("应读取 Merge 目标 Bundle");
        let link_plan_id = format!("merge-link-plan-{suffix}");
        save_test_source_association_plan(storage, &link_plan_id, 100, 1_000);
        let expected_members = target_members
            .iter()
            .map(|member| DirectSourceAssociationMember {
                member_id: &member.id,
                content_fingerprint: &member.content_fingerprint,
            })
            .collect::<Vec<_>>();
        let source_mappings = [DirectSourceAssociationMemberMapping {
            member_id: &target_members[0].id,
            source_relative_path: "skills/alpha",
        }];
        storage
            .finalize_direct_source_association(DirectSourceAssociation {
                plan_id: &link_plan_id,
                source_id: TEST_GITHUB_SOURCE_ID,
                source_catalog_generation: 1,
                source_marker: TEST_GITHUB_COMMIT_ONE,
                bundle_id: &target_id,
                expected_current_target: &target.current_target,
                expected_members: &expected_members,
                member_mappings: &source_mappings,
                now: 110,
            })
            .expect("应建立 Merge 前的 Source-Bundle 关系");
        let retiring_members =
            save_test_managed_bundle(storage, &format!("merge-retiring-{suffix}"));
        let retiring_id = retiring_members[0].bundle_id.clone();
        (
            storage
                .read_source_association_bundle(&target_id)
                .expect("应读取已关联 Source 的目标 Bundle"),
            storage
                .read_source_association_bundle(&retiring_id)
                .expect("应读取待归入 Bundle"),
        )
    }

    fn begin_test_source_association_merge(
        storage: &mut Storage,
        suffix: &str,
        target: &StoredSourceAssociationBundle,
        retiring: &StoredSourceAssociationBundle,
    ) -> String {
        let plan_id = format!("merge-plan-{suffix}");
        let transaction_id = format!("merge-tx-{suffix}");
        save_test_source_association_plan(storage, &plan_id, 120, 1_000);
        let source_mappings = target
            .members
            .iter()
            .chain(retiring.members.iter())
            .filter_map(|member| {
                member.source_relative_path.as_deref().map(|source_path| {
                    FinalSourceAssociationMemberMapping {
                        source_relative_path: source_path,
                        member_id: &member.id,
                    }
                })
            })
            .collect::<Vec<_>>();
        storage
            .begin_source_association_merge(
                &plan_id,
                &transaction_id,
                TEST_GITHUB_SOURCE_ID,
                1,
                TEST_GITHUB_COMMIT_ONE,
                target,
                retiring,
                r#"[{"conflictId":"example","memberId":"winner"}]"#,
                &source_mappings,
                &format!("journals/{transaction_id}.json"),
                130,
            )
            .expect("应原子消费 Merge Plan 并开始事务");
        transaction_id
    }

    fn advance_source_association_to_mounts_applied(storage: &mut Storage, transaction_id: &str) {
        for (phase, now) in [
            ("journal_ready", 131),
            ("candidate_ready", 132),
            ("current_activated", 133),
            ("mounts_applied", 134),
        ] {
            storage
                .update_source_association_transaction_phase(transaction_id, phase, now)
                .expect("应按唯一 Merge 阶段顺序推进");
        }
    }

    fn test_source_association_mount(
        id: &str,
        bundle_id: &str,
        member: &StoredSourceAssociationBundleMember,
        scope: MountScope,
        project_id: Option<&str>,
        target_path: &str,
    ) -> StoredMount {
        StoredMount {
            id: id.to_owned(),
            member_id: member.id.clone(),
            bundle_id: bundle_id.to_owned(),
            skill_name: member.skill_name.clone(),
            member_fingerprint: member.content_fingerprint.clone(),
            app_id: SupportedAppId::Codex,
            scope,
            project_id: project_id.map(str::to_owned),
            project_display_name: project_id.map(|_| "测试项目".to_owned()),
            project_root_path: project_id.map(|_| "/project".to_owned()),
            project_root_device: project_id.map(|_| 10),
            project_root_inode: project_id.map(|_| 20),
            target_path: target_path.to_owned(),
            expected_target: "/managed/member".to_owned(),
            health: MountHealth::Healthy,
        }
    }

    fn validate_test_source_association_merge(
        target: &StoredSourceAssociationBundle,
        retiring: &StoredSourceAssociationBundle,
        mount_assignments: &[FinalSourceAssociationMountAssignment<'_>],
        final_current_target: &str,
        description: &str,
        content_fingerprint: &str,
    ) -> Result<(), StorageError> {
        let winner = &target.members[0];
        let final_members = [FinalSourceAssociationMember {
            member_id: &winner.id,
            skill_name: &winner.skill_name,
            description,
            stable_relative_path: &winner.stable_relative_path,
            content_fingerprint,
        }];
        let source_mappings = [FinalSourceAssociationMemberMapping {
            source_relative_path: winner
                .source_relative_path
                .as_deref()
                .expect("测试 winner 应有 Source mapping"),
            member_id: &winner.id,
        }];
        validate_final_source_association_merge(&FinalSourceAssociationMerge {
            transaction_id: "merge-validation",
            source_id: TEST_GITHUB_SOURCE_ID,
            expected_target_bundle: target,
            expected_retiring_bundle: retiring,
            final_current_target,
            final_members: &final_members,
            mount_assignments,
            source_mappings: &source_mappings,
            now: 150,
        })
    }

    fn register_test_project(storage: &mut Storage, root: &Path) -> StoredProject {
        storage
            .register_project(NewProject {
                id: "project-one",
                display_name: "示例项目",
                root_path: root.to_str().expect("测试路径应是 UTF-8"),
                root_device: 11,
                root_inode: 22,
                created_at: 300,
            })
            .expect("应登记测试 Project")
    }

    // 测试需要逐项控制持久化前置条件，保留显式参数比另建仅测试用配置类型更清楚。
    #[allow(clippy::too_many_arguments)]
    fn save_test_mount_plan(
        storage: &mut Storage,
        member: &StoredManagedMember,
        operation: MountOperation,
        mount_id: &str,
        plan_id: &str,
        scope: MountScope,
        project_id: Option<&str>,
        target_path: &Path,
    ) {
        storage
            .save_mount_plan(NewMountPlan {
                id: plan_id,
                operation,
                purpose: match operation {
                    MountOperation::Create => MountPlanPurpose::Create,
                    MountOperation::Remove => MountPlanPurpose::Remove,
                },
                mount_id,
                member_id: &member.id,
                app_id: SupportedAppId::Codex,
                scope,
                project_id,
                target_path: target_path.to_str().expect("测试路径应是 UTF-8"),
                expected_target: &member.expected_target,
                member_fingerprint: &member.content_fingerprint,
                target_observation: "missing",
                created_at: 300,
                expires_at: 1_000,
            })
            .expect("应保存测试 Mount Plan");
    }

    fn finalize_test_mount_create(
        storage: &mut Storage,
        plan_id: &str,
        transaction_id: &str,
    ) -> StoredMountPlan {
        let plan = storage
            .begin_mount_transaction(
                plan_id,
                transaction_id,
                &format!("journals/{transaction_id}.json"),
                400,
            )
            .expect("应开始 Mount 事务");
        storage
            .update_mount_transaction_phase(transaction_id, "journal_ready", 401)
            .expect("应记录 Mount Journal 已就绪");
        storage
            .update_mount_transaction_phase(transaction_id, "target_applied", 402)
            .expect("应记录 Mount 已作用于目标");
        storage
            .finalize_mount_create(transaction_id, &plan, 403)
            .expect("应提交 Mount 状态");
        plan
    }

    #[test]
    fn mount_migration_creates_normalized_tables_and_constraints() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let storage = open_test_storage(sandbox.path());
        let versions = storage
            .connection
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .expect("应读取 migration")
            .query_map([], |row| row.get::<_, i64>(0))
            .expect("应查询 migration")
            .collect::<Result<Vec<_>, _>>()
            .expect("应收集 migration");
        assert_eq!(versions, (1..=28).collect::<Vec<_>>());
        for table in [
            "projects",
            "mount_plans",
            "mounts",
            "mount_transactions",
            "batch_mount_plans",
            "batch_mount_plan_items",
            "batch_mount_transactions",
            "batch_mount_transaction_items",
            "inventory_management_evidence",
            "inventory_installation_chains",
            "member_installation_chains",
            "takeover_plans",
            "takeover_transactions",
            "source_association_plans",
            "source_association_transactions",
            "removal_plans",
            "removal_transactions",
            "editable_local_relink_plans",
        ] {
            let exists = storage
                .connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
                    [table],
                    |row| row.get::<_, bool>(0),
                )
                .expect("应检查 Mount 表");
            assert!(exists, "migration 应创建 {table}");
        }
        let foreign_key_issues = storage
            .connection
            .prepare("PRAGMA foreign_key_check")
            .expect("应检查外键")
            .query_map([], |_| Ok(()))
            .expect("应执行外键检查")
            .count();
        assert_eq!(foreign_key_issues, 0);
    }

    #[test]
    fn scan_issue_migration_preserves_old_rows_and_allows_multiple_paths_per_root() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        fs::create_dir(&data_root).expect("应创建数据目录");
        let database = data_root.join("skillyard.sqlite3");
        let connection = Connection::open(&database).expect("应创建 version 25 SQLite");
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                 );",
            )
            .expect("应建立 migration 表");
        for (version, migration) in MIGRATIONS.iter().take(25) {
            connection
                .execute_batch(migration)
                .expect("应建立 version 25 schema");
            connection
                .execute(
                    "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, 1)",
                    [version],
                )
                .expect("应记录旧 migration");
        }
        connection
            .execute(
                "INSERT INTO inventory_scan_issues (
                    root_id, root_key, project_id, path, code, message
                 ) VALUES (
                    'global:shared_agents', 'shared_agents', NULL,
                    '/tmp/.agents/skills/first', 'read_skill_content', 'first'
                 )",
                [],
            )
            .expect("应准备旧扫描问题");
        drop(connection);

        let storage = Storage::open(&data_root, &database).expect("应升级扫描问题 schema");
        storage
            .connection
            .execute(
                "INSERT INTO inventory_scan_issues (
                    root_id, root_key, project_id, path, code, message
                 ) VALUES (
                    'global:shared_agents', 'shared_agents', NULL,
                    '/tmp/.agents/skills/second', 'read_skill_content', 'second'
                 )",
                [],
            )
            .expect("同一根应保存第二个条目问题");

        let issues = read_scan_issues_from(&storage.connection).expect("应读取升级后的扫描问题");
        assert_eq!(issues.len(), 2);
        assert_ne!(issues[0].id, issues[1].id);
        assert!(issues.iter().all(|issue| issue.is_entry_scoped()));
    }

    #[test]
    fn bundle_display_name_migration_repairs_verified_lock_source_state() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        fs::create_dir(&data_root).expect("应创建数据目录");
        let database = data_root.join("skillyard.sqlite3");
        let connection = Connection::open(&database).expect("应创建 version 22 SQLite");
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                 );",
            )
            .expect("应建立 migration 表");
        for (version, migration) in MIGRATIONS.iter().take(22) {
            connection
                .execute_batch(migration)
                .expect("应建立 version 22 schema");
            connection
                .execute(
                    "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, 1)",
                    [version],
                )
                .expect("应记录旧 migration");
        }
        connection
            .execute_batch(
                "INSERT INTO bundles (
                    id, display_name, managed_directory, current_target, created_at
                 ) VALUES (
                    'bundle-mattpocock', 'codebase-design',
                    'bundles/bundle-mattpocock', 'contents/current', 1
                 );
                 INSERT INTO skill_members (
                    id, bundle_id, skill_name, description, stable_relative_path,
                    content_fingerprint, created_at
                 ) VALUES
                    ('member-codebase', 'bundle-mattpocock', 'codebase-design',
                     '设计代码结构', 'members/codebase-design', 'sha256:one', 1),
                    ('member-tdd', 'bundle-mattpocock', 'tdd',
                     '测试驱动开发', 'members/tdd', 'sha256:two', 1);
                 INSERT INTO member_selections (bundle_id, member_id, selected_at)
                 VALUES
                    ('bundle-mattpocock', 'member-codebase', 1),
                    ('bundle-mattpocock', 'member-tdd', 1);
                 INSERT INTO member_installation_chains (
                    member_id, kind, record_path, source, source_type, source_locator,
                    skill_path, tracked_ref, content_marker, installed_at, updated_at
                 ) VALUES
                    ('member-codebase', 'lock_v3', '/home/.agents/.skill-lock.json',
                     'mattpocock/skills', 'github',
                     'https://github.com/mattpocock/skills.git',
                     'skills/codebase-design/SKILL.md', NULL, 'hash-one', '1', '1'),
                    ('member-tdd', 'lock_v3', '/home/.agents/.skill-lock.json',
                     'mattpocock/skills', 'github',
                     'https://github.com/mattpocock/skills.git',
                     'skills/tdd/SKILL.md', NULL, 'hash-two', '1', '1');",
            )
            .expect("应准备已核验的旧 Bundle 命名状态");
        drop(connection);

        let storage =
            Storage::open(&data_root, &database).expect("应完成 Bundle 名称与 Source 修正");
        let saved_source = storage
            .connection
            .query_row(
                "SELECT bundle.display_name, source.canonical_identity,
                        source.display_name, source.locator, source.tracked_ref,
                        link.adopted_marker
                 FROM bundles AS bundle
                 JOIN source_bundle_links AS link ON link.bundle_id = bundle.id
                 JOIN sources AS source ON source.id = link.source_id
                 WHERE bundle.id = 'bundle-mattpocock'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .expect("应读取修正后的 Bundle Source");
        assert_eq!(
            saved_source,
            (
                "mattpocock/skills".to_owned(),
                "github:mattpocock/skills".to_owned(),
                "mattpocock/skills".to_owned(),
                "https://github.com/mattpocock/skills".to_owned(),
                "HEAD".to_owned(),
                None,
            )
        );
        let mapping_count = storage
            .connection
            .query_row(
                "SELECT COUNT(*)
                 FROM source_member_links AS mapping
                 JOIN source_bundle_links AS link ON link.source_id = mapping.source_id
                 WHERE link.bundle_id = 'bundle-mattpocock'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("应读取自动保存的成员路径");
        assert_eq!(mapping_count, 2);
    }

    #[test]
    fn bundle_mount_removal_migration_preserves_existing_removal_state() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        fs::create_dir(&data_root).expect("应创建数据目录");
        let database = data_root.join("skillyard.sqlite3");
        let connection = Connection::open(&database).expect("应创建 version 24 SQLite");
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                 );",
            )
            .expect("应建立 migration 表");
        for (version, migration) in MIGRATIONS.iter().take(24) {
            connection
                .execute_batch(migration)
                .expect("应建立 version 24 schema");
            connection
                .execute(
                    "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, 1)",
                    [version],
                )
                .expect("应记录旧 migration");
        }
        connection
            .execute(
                "INSERT INTO removal_plans (
                    id, kind, target_id, payload_json, payload_sha256,
                    status, created_at, expires_at
                 ) VALUES (
                    'old-removal-plan', 'project', 'old-project', '{}', ?1,
                    'consumed', 1, 2
                 )",
                ["0".repeat(64)],
            )
            .expect("应准备旧 Removal Plan");
        connection
            .execute(
                "INSERT INTO removal_transactions (
                    id, plan_id, kind, target_id, journal_path, phase, status,
                    error_message, created_at, updated_at
                 ) VALUES (
                    'old-removal-transaction', 'old-removal-plan', 'project',
                    'old-project', 'journals/old-removal-transaction.json',
                    'journal_ready', 'blocked', '等待人工处理', 1, 1
                 )",
                [],
            )
            .expect("应准备旧 Removal 事务");
        drop(connection);

        let storage = Storage::open(&data_root, &database).expect("应升级 Removal schema");
        let preserved = storage
            .connection
            .query_row(
                "SELECT plan.kind, removal_tx.kind, removal_tx.phase,
                        removal_tx.status, removal_tx.error_message
                 FROM removal_plans AS plan
                 JOIN removal_transactions AS removal_tx
                   ON removal_tx.plan_id = plan.id
                 WHERE plan.id = 'old-removal-plan'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .expect("应读取迁移后的 Removal 状态");
        assert_eq!(
            preserved,
            (
                "project".to_owned(),
                "project".to_owned(),
                "journal_ready".to_owned(),
                "blocked".to_owned(),
                Some("等待人工处理".to_owned()),
            )
        );
        let foreign_key_issues = storage
            .connection
            .prepare("PRAGMA foreign_key_check")
            .expect("应检查迁移后的外键")
            .query_map([], |_| Ok(()))
            .expect("应执行外键检查")
            .count();
        assert_eq!(foreign_key_issues, 0);
    }

    #[test]
    fn source_association_transaction_migration_seals_final_mappings() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let storage = open_test_storage(sandbox.path());
        let columns = storage
            .connection
            .prepare("PRAGMA table_info(source_association_transactions)")
            .expect("应读取 Source 关联事务 schema")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("应查询 Source 关联事务列")
            .collect::<Result<BTreeSet<_>, _>>()
            .expect("应收集 Source 关联事务列");

        assert!(
            columns.contains("source_mappings_json"),
            "Merge 开始时必须把最终 Source mapping 封存在 canonical 事务中"
        );
    }

    #[test]
    fn general_source_migration_keeps_one_install_protocol() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let storage = open_test_storage(sandbox.path());
        let install_columns = storage
            .connection
            .prepare("PRAGMA table_info(install_plans)")
            .expect("应读取安装 Plan schema")
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(3)?))
            })
            .expect("应查询安装 Plan schema")
            .collect::<Result<BTreeMap<_, _>, _>>()
            .expect("应收集安装 Plan schema");
        for column in [
            "kind",
            "install_mode",
            "snapshot_relative_path",
            "source_id",
            "source_tracked_ref",
            "source_catalog_generation",
            "source_marker",
            "expected_source_marker",
            "expected_current_target",
            "expected_adopted_marker",
        ] {
            assert!(
                install_columns.contains_key(column),
                "统一安装协议应包含 {column}"
            );
        }
        assert_eq!(
            install_columns.get("input_path"),
            Some(&0),
            "Source Plan 不应伪造本地输入路径"
        );
        for legacy_anchor in ["member_id", "skill_name", "skill_description"] {
            assert!(
                !install_columns.contains_key(legacy_anchor),
                "Bundle Plan 的成员身份只能来自候选集合，不能保留 {legacy_anchor}"
            );
        }

        let candidate_columns = storage
            .connection
            .prepare("PRAGMA table_info(install_plan_candidates)")
            .expect("应读取候选 schema")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("应查询候选 schema")
            .collect::<Result<BTreeSet<_>, _>>()
            .expect("应收集候选 schema");
        assert!(candidate_columns.contains("preserve_existing"));
        assert!(candidate_columns.contains("previous_content_fingerprint"));
        let source_path_not_null = storage
            .connection
            .query_row(
                "SELECT \"notnull\"
                 FROM pragma_table_info('install_plan_candidates')
                 WHERE name = 'source_relative_path'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("应读取 optional Source 路径约束");
        assert_eq!(
            source_path_not_null, 0,
            "“不对应”的保留成员不能被迫制造 Source 路径"
        );

        let source_columns = storage
            .connection
            .prepare("PRAGMA table_info(sources)")
            .expect("应读取 Source schema")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("应查询 Source schema")
            .collect::<Result<BTreeSet<_>, _>>()
            .expect("应收集 Source schema");
        for column in [
            "locator",
            "filesystem_device",
            "filesystem_inode",
            "catalog_marker",
        ] {
            assert!(
                source_columns.contains(column),
                "通用 Source 协议应包含 {column}"
            );
        }
        assert!(!source_columns.contains("repository_url"));
        assert!(!source_columns.contains("catalog_commit_sha"));

        let lifecycle_sql = storage
            .connection
            .query_row(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'table' AND name = 'lifecycle_transactions'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("应读取唯一安装事务 schema");
        assert!(lifecycle_sql.contains("kind = 'install_bundle'"));

        for trigger in [
            "mount_transaction_reject_active_install",
            "install_transaction_reject_active_mount",
            "batch_mount_transaction_reject_active_writer",
            "install_transaction_reject_active_batch_mount",
            "takeover_transaction_reject_active_writer",
            "install_transaction_reject_active_takeover",
            "source_association_transaction_reject_active_writer",
            "install_transaction_reject_active_source_association",
        ] {
            let exists = storage
                .connection
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM sqlite_schema WHERE type = 'trigger' AND name = ?1
                     )",
                    [trigger],
                    |row| row.get::<_, bool>(0),
                )
                .expect("应检查跨事务单写者约束");
            assert!(exists, "migration 应重建 {trigger}");
        }
    }

    #[test]
    fn general_source_migration_preserves_pending_plans_and_lifecycle_rows() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        fs::create_dir(&data_root).expect("应创建数据目录");
        let database = data_root.join("skillyard.sqlite3");
        let connection = Connection::open(&database).expect("应创建 version 13 SQLite");
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                 );",
            )
            .expect("应建立 migration 表");
        for (version, migration) in MIGRATIONS.iter().take(13) {
            connection
                .execute_batch(migration)
                .expect("应建立 version 13 schema");
            connection
                .execute(
                    "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, 1)",
                    [version],
                )
                .expect("应记录旧 migration");
        }
        connection
            .execute_batch(
                "UPDATE sources
                 SET catalog_status = 'fresh', catalog_generation = 1,
                     catalog_commit_sha = 'commit-one', catalog_fetched_at = 10
                 WHERE id = 'source-anthropics-skills';

                 INSERT INTO install_plans (
                    id, kind, install_mode, input_path, input_device, input_inode,
                    input_fingerprint, snapshot_relative_path, source_id, source_tracked_ref,
                    source_catalog_generation, source_commit_sha, expected_current_target,
                    expected_adopted_commit_sha, bundle_id, bundle_display_name,
                    warnings_json, created_at, expires_at, status
                 ) VALUES
                    ('pending-plan', 'github_snapshot', 'create', NULL, 1, 2,
                     'sha256:pending', 'staging/pending/source',
                     'source-anthropics-skills', 'main', 1, 'commit-one', NULL, NULL,
                     'pending-bundle', 'Pending Bundle', '[]', 10, 1000, 'pending'),
                    ('active-plan', 'github_snapshot', 'create', NULL, 3, 4,
                     'sha256:active', 'staging/active/source',
                     'source-anthropics-skills', 'main', 1, 'commit-one', NULL, NULL,
                     'active-bundle', 'Active Bundle', '[]', 11, 1000, 'consumed');

                 INSERT INTO install_plan_candidates (
                    plan_id, candidate_id, source_relative_path, skill_name,
                    skill_description, content_fingerprint, selectable, preserve_existing,
                    validation_errors_json, warnings_json, default_selected, selected, sort_order
                 ) VALUES
                    ('pending-plan', 'pending-member', 'skills/pending', 'pending',
                     'Pending', 'sha256:pending', 1, 0, '[]', '[]', 1, 1, 0),
                    ('active-plan', 'active-member', 'skills/active', 'active',
                     'Active', 'sha256:active', 1, 0, '[]', '[]', 1, 1, 0);

                 INSERT INTO lifecycle_transactions (
                    id, kind, plan_id, bundle_id, member_id, journal_path,
                    phase, status, created_at, updated_at
                 ) VALUES (
                    'active-transaction', 'install_bundle', 'active-plan',
                    'active-bundle', 'active-member', 'journals/active.json',
                    'journal_ready', 'in_progress', 12, 12
                 );",
            )
            .expect("应写入迁移前的 Plan 与事务");
        drop(connection);

        let storage = Storage::open(&data_root, &database).expect("0014 应保留旧状态并完成迁移");
        let pending = storage
            .read_install_plan("pending-plan")
            .expect("pending Plan 应可继续读取");
        assert_eq!(pending.kind, "source_snapshot");
        assert_eq!(pending.source_marker.as_deref(), Some("commit-one"));
        let lifecycle = storage
            .recoverable_lifecycle_transactions()
            .expect("应读取迁移前的生命周期事务");
        assert!(lifecycle.iter().any(|row| {
            row.id == "active-transaction"
                && row.plan_id == "active-plan"
                && row.phase == "journal_ready"
                && row.status == "in_progress"
        }));
        let source = storage
            .connection
            .query_row(
                "SELECT locator, catalog_marker
                 FROM sources
                 WHERE id = 'source-anthropics-skills'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("GitHub Source 字段应精确映射");
        assert_eq!(
            source,
            (
                "https://github.com/anthropics/skills".to_owned(),
                "commit-one".to_owned()
            )
        );
        let foreign_key_issues = storage
            .connection
            .prepare("PRAGMA foreign_key_check")
            .expect("应准备外键检查")
            .query_map([], |_| Ok(()))
            .expect("应执行外键检查")
            .count();
        assert_eq!(foreign_key_issues, 0);
    }

    #[test]
    fn source_association_migration_preserves_pending_plan_and_active_lifecycle() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        fs::create_dir(&data_root).expect("应创建数据目录");
        let database = data_root.join("skillyard.sqlite3");
        let connection = Connection::open(&database).expect("应创建 version 14 SQLite");
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                 );",
            )
            .expect("应建立 migration 表");
        for (version, migration) in MIGRATIONS.iter().take(14) {
            connection
                .execute_batch(migration)
                .expect("应建立 version 14 schema");
            connection
                .execute(
                    "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, 1)",
                    [version],
                )
                .expect("应记录旧 migration");
        }
        connection
            .execute_batch(
                "INSERT INTO install_plans (
                    id, kind, install_mode, input_path, input_device, input_inode,
                    input_fingerprint, snapshot_relative_path, source_id, source_tracked_ref,
                    source_catalog_generation, source_marker, expected_current_target,
                    expected_adopted_marker, bundle_id, bundle_display_name,
                    warnings_json, created_at, expires_at, status
                 ) VALUES
                    ('pending-before-association', 'folder_snapshot', 'create',
                     '/tmp/pending', 1, 2, 'sha256:pending', NULL, NULL, NULL,
                     NULL, NULL, NULL, NULL, 'pending-bundle', 'Pending',
                     '[]', 10, 1000, 'pending'),
                    ('active-before-association', 'folder_snapshot', 'create',
                     '/tmp/active', 3, 4, 'sha256:active', NULL, NULL, NULL,
                     NULL, NULL, NULL, NULL, 'active-bundle', 'Active',
                     '[]', 11, 1000, 'consumed');

                 INSERT INTO install_plan_candidates (
                    plan_id, candidate_id, source_relative_path, skill_name,
                    skill_description, content_fingerprint, selectable, preserve_existing,
                    validation_errors_json, warnings_json, default_selected, selected, sort_order
                 ) VALUES
                    ('pending-before-association', 'pending-member', '', 'pending',
                     'Pending', 'sha256:pending', 1, 0, '[]', '[]', 1, 1, 0),
                    ('active-before-association', 'active-member', '', 'active',
                     'Active', 'sha256:active', 1, 0, '[]', '[]', 1, 1, 0);

                 INSERT INTO lifecycle_transactions (
                    id, kind, plan_id, bundle_id, member_id, journal_path,
                    phase, status, created_at, updated_at
                 ) VALUES (
                    'active-before-association-tx', 'install_bundle',
                    'active-before-association', 'active-bundle', 'active-member',
                    'journals/active-before-association.json',
                    'journal_ready', 'in_progress', 12, 12
                 );",
            )
            .expect("应写入 0015 迁移前状态");
        drop(connection);

        let storage = Storage::open(&data_root, &database).expect("0015 应保留未完成状态");
        let pending = storage
            .read_install_plan("pending-before-association")
            .expect("pending Plan 应保持可读");
        assert_eq!(
            pending.candidates[0].source_relative_path.as_deref(),
            Some("")
        );
        assert!(
            storage
                .recoverable_lifecycle_transactions()
                .expect("active lifecycle 应保持可恢复")
                .iter()
                .any(|transaction| transaction.id == "active-before-association-tx")
        );
        let foreign_key_issues = storage
            .connection
            .prepare("PRAGMA foreign_key_check")
            .expect("应准备外键检查")
            .query_map([], |_| Ok(()))
            .expect("应执行外键检查")
            .count();
        assert_eq!(foreign_key_issues, 0);
    }

    #[test]
    fn bundle_update_migration_preserves_pending_and_blocked_install_state() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        fs::create_dir(&data_root).expect("应创建数据目录");
        let database = data_root.join("skillyard.sqlite3");
        let connection = Connection::open(&database).expect("应创建 version 17 SQLite");
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                 );",
            )
            .expect("应建立 migration 表");
        for (version, migration) in MIGRATIONS.iter().take(17) {
            connection
                .execute_batch(migration)
                .expect("应建立 version 17 schema");
            connection
                .execute(
                    "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, 1)",
                    [version],
                )
                .expect("应记录旧 migration");
        }
        connection
            .execute_batch(
                "INSERT INTO install_plans (
                    id, kind, install_mode, input_path, input_device, input_inode,
                    input_fingerprint, snapshot_relative_path, source_id, source_tracked_ref,
                    source_catalog_generation, source_marker, expected_current_target,
                    expected_adopted_marker, bundle_id, bundle_display_name,
                    warnings_json, created_at, expires_at, status
                 ) VALUES
                    ('pending-before-update', 'folder_snapshot', 'create',
                     '/tmp/pending-update', 1, 2, 'sha256:pending', NULL, NULL, NULL,
                     NULL, NULL, NULL, NULL, 'pending-update-bundle', 'Pending',
                     '[]', 10, 1000, 'pending'),
                    ('blocked-before-update', 'folder_snapshot', 'create',
                     '/tmp/blocked-update', 3, 4, 'sha256:blocked', NULL, NULL, NULL,
                     NULL, NULL, NULL, NULL, 'blocked-update-bundle', 'Blocked',
                     '[]', 11, 1000, 'consumed');

                 INSERT INTO install_plan_candidates (
                    plan_id, candidate_id, source_relative_path, skill_name,
                    skill_description, content_fingerprint, selectable, preserve_existing,
                    validation_errors_json, warnings_json, default_selected, selected, sort_order
                 ) VALUES
                    ('pending-before-update', 'pending-update-member', '', 'pending-update',
                     'Pending', 'sha256:pending', 1, 0, '[]', '[]', 1, 1, 0),
                    ('blocked-before-update', 'blocked-update-member', '', 'blocked-update',
                     'Blocked', 'sha256:blocked', 1, 0, '[]', '[]', 1, 1, 0);

                 INSERT INTO lifecycle_transactions (
                    id, kind, plan_id, bundle_id, member_id, journal_path,
                    phase, status, error_message, created_at, updated_at
                 ) VALUES (
                    'blocked-before-update-tx', 'install_bundle',
                    'blocked-before-update', 'blocked-update-bundle',
                    'blocked-update-member', 'journals/blocked-before-update.json',
                    'journal_ready', 'blocked', '测试人工恢复', 12, 12
                 );",
            )
            .expect("应写入 0018 迁移前状态");
        drop(connection);

        let storage = Storage::open(&data_root, &database).expect("0018 应保留未完成状态");
        let pending = storage
            .read_install_plan("pending-before-update")
            .expect("pending Plan 应保持可读");
        assert!(pending.expected_source_marker.is_none());
        assert!(pending.candidates[0].previous_content_fingerprint.is_none());
        assert!(
            storage
                .recoverable_lifecycle_transactions()
                .expect("blocked lifecycle 应保持可恢复")
                .iter()
                .any(|transaction| {
                    transaction.id == "blocked-before-update-tx" && transaction.status == "blocked"
                })
        );
        let foreign_key_issues = storage
            .connection
            .prepare("PRAGMA foreign_key_check")
            .expect("应准备外键检查")
            .query_map([], |_| Ok(()))
            .expect("应执行外键检查")
            .count();
        assert_eq!(foreign_key_issues, 0);
    }

    #[test]
    fn source_association_plan_storage_enforces_pending_and_hash_contract() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let invalid = storage
            .save_source_association_plan(&StoredSourceAssociationPlanRow {
                id: "invalid-association-plan".to_owned(),
                payload_json: "{}".to_owned(),
                payload_sha256: "ABC".to_owned(),
                status: "pending".to_owned(),
                created_at: 100,
                expires_at: 200,
            })
            .expect_err("非法 hash 不能进入 Plan 表");
        assert!(matches!(
            invalid,
            StorageError::InvalidSourceAssociationPlan
        ));

        save_test_source_association_plan(&mut storage, "pending-association-plan", 100, 200);
        assert_eq!(
            storage
                .read_source_association_plan("pending-association-plan")
                .expect("应读回 pending Plan")
                .status,
            "pending"
        );
        storage
            .discard_source_association_plan("pending-association-plan")
            .expect("pending Plan 可被放弃");
        assert!(matches!(
            storage
                .read_source_association_plan("pending-association-plan")
                .expect_err("放弃后 Plan 应不存在"),
            StorageError::SourceAssociationPlanNotFound
        ));
    }

    #[test]
    fn source_association_merge_single_writer_is_bidirectional_and_begin_is_atomic() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let (target, retiring) = setup_test_source_association_merge(&mut storage, "writer");
        save_test_source_association_plan(&mut storage, "merge-writer-plan", 120, 1_000);
        save_test_plan(
            &mut storage,
            "merge-writer-install-plan",
            "merge-writer-install-bundle",
            "merge-writer-install-member",
        );
        storage
            .begin_install_transaction(
                "merge-writer-install-plan",
                "merge-writer-install-tx",
                "journals/merge-writer-install.json",
                125,
            )
            .expect("应开始既有安装事务");
        let source_mappings = [FinalSourceAssociationMemberMapping {
            source_relative_path: target.members[0]
                .source_relative_path
                .as_deref()
                .expect("目标成员应保留 Source mapping"),
            member_id: &target.members[0].id,
        }];
        assert!(matches!(
            storage
                .begin_source_association_merge(
                    "merge-writer-plan",
                    "merge-writer-tx",
                    TEST_GITHUB_SOURCE_ID,
                    1,
                    TEST_GITHUB_COMMIT_ONE,
                    &target,
                    &retiring,
                    "[]",
                    &source_mappings,
                    "journals/merge-writer.json",
                    130,
                )
                .expect_err("活跃安装事务必须阻止 Merge"),
            StorageError::ActiveLifecycleTransaction
        ));
        assert_eq!(
            storage
                .read_source_association_plan("merge-writer-plan")
                .expect("单写者拒绝不能消费 Plan")
                .status,
            "pending"
        );
        storage
            .abort_lifecycle_transaction("merge-writer-install-tx", None, 131)
            .expect("应中止测试安装事务");
        storage
            .forget_terminal_transaction("merge-writer-install-tx")
            .expect("应清理测试安装事务");

        let consumed = storage
            .begin_source_association_merge(
                "merge-writer-plan",
                "merge-writer-tx",
                TEST_GITHUB_SOURCE_ID,
                1,
                TEST_GITHUB_COMMIT_ONE,
                &target,
                &retiring,
                r#"[{"conflictId":"one","memberId":"winner"}]"#,
                &source_mappings,
                "journals/merge-writer.json",
                132,
            )
            .expect("应原子消费 Plan 并开始 Merge");
        assert_eq!(consumed.status, "consumed");
        let stored = storage
            .recoverable_source_association_transactions()
            .expect("应读回 Merge 恢复状态");
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored[0].content_choices_json,
            r#"[{"conflictId":"one","memberId":"winner"}]"#
        );

        save_test_plan(
            &mut storage,
            "merge-writer-later-install-plan",
            "merge-writer-later-bundle",
            "merge-writer-later-member",
        );
        assert!(matches!(
            storage
                .begin_install_transaction(
                    "merge-writer-later-install-plan",
                    "merge-writer-later-install-tx",
                    "journals/merge-writer-later.json",
                    133,
                )
                .expect_err("活跃 Merge 必须阻止安装事务"),
            StorageError::ActiveLifecycleTransaction
        ));
    }

    #[test]
    fn source_association_merge_begin_rechecks_snapshots_before_invalidating_pending_plans() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let (initial_target, initial_retiring) =
            setup_test_source_association_merge(&mut storage, "begin-snapshot");
        let retiring_member = storage
            .read_managed_member(&initial_retiring.members[0].id)
            .expect("应读取 retiring Member");
        let mounted_target = sandbox
            .path()
            .join("home/.codex/skills")
            .join(&retiring_member.skill_name);
        save_test_mount_plan(
            &mut storage,
            &retiring_member,
            MountOperation::Create,
            "merge-begin-existing-mount",
            "merge-begin-existing-plan",
            MountScope::Global,
            None,
            &mounted_target,
        );
        finalize_test_mount_create(
            &mut storage,
            "merge-begin-existing-plan",
            "merge-begin-existing-tx",
        );
        storage
            .forget_terminal_mount_transaction("merge-begin-existing-tx")
            .expect("应清理既有 Mount 事务");
        let target = storage
            .read_source_association_bundle(&initial_target.id)
            .expect("应读取目标完整快照");
        let retiring = storage
            .read_source_association_bundle(&initial_retiring.id)
            .expect("应读取含 Mount 的 retiring 快照");

        let target_member = storage
            .read_managed_member(&target.members[0].id)
            .expect("应读取目标 Member");
        let pending_target = sandbox
            .path()
            .join("home/.claude/skills")
            .join(&target_member.skill_name);
        save_test_mount_plan(
            &mut storage,
            &target_member,
            MountOperation::Create,
            "merge-begin-pending-mount",
            "merge-begin-pending-plan",
            MountScope::Global,
            None,
            &pending_target,
        );
        let retiring_batch_member = storage
            .read_managed_member(&retiring.members[1].id)
            .expect("应读取 Batch retiring Member");
        let pending_batch_target = sandbox
            .path()
            .join("home/.claude/skills")
            .join(&retiring_batch_member.skill_name);
        let batch_items = [NewBatchMountPlanItem {
            id: "merge-begin-pending-batch-item",
            mount_id: "merge-begin-pending-batch-mount",
            member_id: &retiring_batch_member.id,
            app_id: SupportedAppId::ClaudeCode,
            scope: MountScope::Global,
            project_id: None,
            target_path: pending_batch_target.to_str().expect("测试路径应是 UTF-8"),
            expected_target: &retiring_batch_member.expected_target,
            member_fingerprint: &retiring_batch_member.content_fingerprint,
            target_observation: "absent",
            disposition: BatchMountDisposition::Ready,
            selectable: true,
            default_selected: true,
            conflict_reason: None,
            target_health: MountHealth::Missing,
        }];
        storage
            .save_batch_mount_plan(NewBatchMountPlan {
                id: "merge-begin-pending-batch-plan",
                bundle_id: &retiring.id,
                items: &batch_items,
                created_at: 120,
                expires_at: 1_000,
            })
            .expect("应保存 retiring Bundle 的 pending Batch Plan");
        save_test_source_association_plan(&mut storage, "merge-begin-plan", 120, 1_000);
        let source_mappings = [FinalSourceAssociationMemberMapping {
            source_relative_path: target.members[0]
                .source_relative_path
                .as_deref()
                .expect("目标成员应保留 Source mapping"),
            member_id: &target.members[0].id,
        }];

        assert!(matches!(
            storage
                .begin_source_association_merge(
                    "merge-begin-plan",
                    "merge-begin-dropped-mapping",
                    TEST_GITHUB_SOURCE_ID,
                    1,
                    TEST_GITHUB_COMMIT_ONE,
                    &target,
                    &retiring,
                    "[]",
                    &[],
                    "journals/merge-begin-dropped-mapping.json",
                    129,
                )
                .expect_err("最终 mapping 不能丢失目标 Bundle 已有的 Source path"),
            StorageError::InvalidSourceAssociationPlan
        ));
        storage
            .connection
            .execute(
                "UPDATE sources SET catalog_marker = ?2 WHERE id = ?1",
                params![TEST_GITHUB_SOURCE_ID, TEST_GITHUB_COMMIT_TWO],
            )
            .expect("应模拟 Source marker 竞态");
        assert!(matches!(
            storage
                .begin_source_association_merge(
                    "merge-begin-plan",
                    "merge-begin-source-race",
                    TEST_GITHUB_SOURCE_ID,
                    1,
                    TEST_GITHUB_COMMIT_ONE,
                    &target,
                    &retiring,
                    "[]",
                    &source_mappings,
                    "journals/merge-begin-source-race.json",
                    130,
                )
                .expect_err("Source marker 变化必须拒绝开始"),
            StorageError::SourceCatalogStateChanged
        ));
        storage
            .connection
            .execute(
                "UPDATE sources SET catalog_marker = ?2 WHERE id = ?1",
                params![TEST_GITHUB_SOURCE_ID, TEST_GITHUB_COMMIT_ONE],
            )
            .expect("应恢复 Source marker");

        storage
            .connection
            .execute(
                "UPDATE skill_members SET description = '竞态描述' WHERE id = ?1",
                [&target.members[0].id],
            )
            .expect("应模拟 Bundle 成员竞态");
        assert!(matches!(
            storage
                .begin_source_association_merge(
                    "merge-begin-plan",
                    "merge-begin-bundle-race",
                    TEST_GITHUB_SOURCE_ID,
                    1,
                    TEST_GITHUB_COMMIT_ONE,
                    &target,
                    &retiring,
                    "[]",
                    &source_mappings,
                    "journals/merge-begin-bundle-race.json",
                    131,
                )
                .expect_err("Bundle 快照变化必须拒绝开始"),
            StorageError::SourceBundleStateConflict
        ));
        storage
            .connection
            .execute(
                "UPDATE skill_members SET description = ?2 WHERE id = ?1",
                params![target.members[0].id, target.members[0].description],
            )
            .expect("应恢复 Bundle 成员");

        let changed_mount_target = sandbox
            .path()
            .join("alternate/.codex/skills")
            .join(&retiring_member.skill_name);
        storage
            .connection
            .execute(
                "UPDATE mounts SET target_path = ?2 WHERE id = ?1",
                params![
                    retiring.members[0].mounts[0].id,
                    changed_mount_target.to_str().expect("测试路径应是 UTF-8")
                ],
            )
            .expect("应模拟 Mount SQLite 快照竞态");
        assert!(matches!(
            storage
                .begin_source_association_merge(
                    "merge-begin-plan",
                    "merge-begin-mount-race",
                    TEST_GITHUB_SOURCE_ID,
                    1,
                    TEST_GITHUB_COMMIT_ONE,
                    &target,
                    &retiring,
                    "[]",
                    &source_mappings,
                    "journals/merge-begin-mount-race.json",
                    132,
                )
                .expect_err("Mount 快照变化必须拒绝开始"),
            StorageError::SourceBundleStateConflict
        ));
        storage
            .connection
            .execute(
                "UPDATE mounts SET target_path = ?2 WHERE id = ?1",
                params![
                    retiring.members[0].mounts[0].id,
                    retiring.members[0].mounts[0].target_path
                ],
            )
            .expect("应恢复 Mount 快照");

        for table in ["mount_plans", "batch_mount_plans"] {
            let pending = storage
                .connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE status = 'pending'"),
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("失败开始不能删除 pending Plan");
            assert_eq!(pending, 1, "失败开始必须原子保留 {table}");
        }
        storage
            .begin_source_association_merge(
                "merge-begin-plan",
                "merge-begin-success",
                TEST_GITHUB_SOURCE_ID,
                1,
                TEST_GITHUB_COMMIT_ONE,
                &target,
                &retiring,
                "[]",
                &source_mappings,
                "journals/merge-begin-success.json",
                133,
            )
            .expect("快照一致时应开始 Merge");
        for table in ["mount_plans", "batch_mount_plans"] {
            let pending = storage
                .connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE status = 'pending'"),
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("应检查 pending Plan 作废结果");
            assert_eq!(pending, 0, "成功开始必须作废相关 {table}");
        }
    }

    #[test]
    fn blocked_source_association_merge_is_visible_and_isolates_all_objects() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let (target, retiring) = setup_test_source_association_merge(&mut storage, "blocked");
        let transaction_id =
            begin_test_source_association_merge(&mut storage, "blocked", &target, &retiring);
        storage
            .block_source_association_transaction(&transaction_id, "测试 Merge 人工恢复", 140)
            .expect("应把 Merge 标记为 blocked");

        assert!(
            bundle_or_source_write_is_blocked(
                &storage.connection,
                Some(&target.id),
                Some(TEST_GITHUB_SOURCE_ID),
            )
            .expect("应检查目标 Bundle 与 Source 隔离")
        );
        assert!(
            bundle_or_source_write_is_blocked(&storage.connection, Some(&retiring.id), None,)
                .expect("应检查 retiring Bundle 隔离")
        );
        let recoverable = storage
            .recoverable_source_association_transactions()
            .expect("blocked Merge 必须可恢复");
        assert_eq!(recoverable[0].status, "blocked");
        let issues = storage
            .read_recovery_issues()
            .expect("blocked Merge 必须进入 RecoveryIssue");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].id, transaction_id);
        assert_eq!(issues[0].message, "测试 Merge 人工恢复");
    }

    #[test]
    fn blocked_source_association_rejects_github_catalog_and_ref_writes() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let (target, retiring) =
            setup_test_source_association_merge(&mut storage, "blocked-catalog-success");
        let ref_change = storage
            .save_or_prepare_github_source(
                NewGitHubSource {
                    id: "ignored-existing-source",
                    canonical_identity: "github:anthropics/skills",
                    owner: "anthropics",
                    repository: "skills",
                    display_name: "anthropics/skills",
                    locator: "https://github.com/anthropics/skills",
                    tracked_ref: "next",
                    resolved_commit_sha: TEST_GITHUB_COMMIT_TWO,
                    member_path_hint: None,
                },
                "blocked-ref-change-plan",
                115,
                1_000,
            )
            .expect("应先签发 Tracked Ref 变更 Plan");
        assert!(matches!(
            ref_change,
            SaveGitHubSourceResult::RefChangeRequired { .. }
        ));
        let transaction_id = begin_test_source_association_merge(
            &mut storage,
            "blocked-catalog-success",
            &target,
            &retiring,
        );
        storage
            .block_source_association_transaction(&transaction_id, "测试 Source 写隔离", 140)
            .expect("应把 Merge 标记为 blocked");
        let before = storage
            .read_source_install_source(TEST_GITHUB_SOURCE_ID)
            .expect("应读取 blocked 前 Source");
        let github_before = storage
            .read_github_source(TEST_GITHUB_SOURCE_ID)
            .expect("应读取 blocked 前 GitHub metadata");
        let members = [NewSourceCatalogMember {
            id: "blocked-catalog-new",
            relative_path: "skills/new",
            skill_name: Some("new"),
            description: Some("New Skill"),
            content_fingerprint: Some("sha256:new"),
            selectable: true,
            validation_errors: &[],
            warnings: &[],
        }];

        let error = storage
            .save_source_catalog_success(
                TEST_GITHUB_SOURCE_ID,
                "main",
                TEST_GITHUB_COMMIT_TWO,
                150,
                &members,
            )
            .expect_err("blocked Merge 影响的 Source 不能替换 Catalog");
        assert!(matches!(error, StorageError::ManagedObjectBlocked));
        assert_eq!(
            storage
                .read_source_install_source(TEST_GITHUB_SOURCE_ID)
                .expect("拒绝后 Source 应保持原状"),
            before
        );

        let error = storage
            .save_source_catalog_failure(TEST_GITHUB_SOURCE_ID, "main", 151, "测试 Reload 失败")
            .expect_err("blocked Merge 影响的 Source 不能写入 Catalog 失败状态");
        assert!(matches!(error, StorageError::ManagedObjectBlocked));
        assert_eq!(
            storage
                .read_source_install_source(TEST_GITHUB_SOURCE_ID)
                .expect("拒绝失败状态后 Source 应保持原状"),
            before
        );

        let error = storage
            .confirm_source_ref_change("blocked-ref-change-plan", 152)
            .expect_err("blocked Merge 影响的 Source 不能确认 Tracked Ref 变更");
        assert!(matches!(error, StorageError::ManagedObjectBlocked));
        assert_eq!(
            storage
                .read_source_tracked_ref("github:anthropics/skills")
                .expect("应读取被拒绝后的 Tracked Ref"),
            Some("main".to_owned())
        );
        let same_ref_error = match storage.save_or_prepare_github_source(
            NewGitHubSource {
                id: "ignored-blocked-same-ref",
                canonical_identity: "github:anthropics/skills",
                owner: "anthropics",
                repository: "skills",
                display_name: "Blocked Changed",
                locator: "https://github.com/anthropics/skills",
                tracked_ref: "main",
                resolved_commit_sha: TEST_GITHUB_COMMIT_ONE,
                member_path_hint: Some("skills"),
            },
            "unused-same-ref-plan",
            153,
            1_000,
        ) {
            Err(error) => error,
            Ok(_) => panic!("blocked Source 不能通过重复 GitHub 输入更新 metadata"),
        };
        assert!(matches!(same_ref_error, StorageError::ManagedObjectBlocked));
        assert_eq!(
            storage
                .read_github_source(TEST_GITHUB_SOURCE_ID)
                .expect("拒绝后 GitHub metadata 应保持原状"),
            github_before
        );
        let different_ref_error = match storage.save_or_prepare_github_source(
            NewGitHubSource {
                id: "ignored-blocked-different-ref",
                canonical_identity: "github:anthropics/skills",
                owner: "anthropics",
                repository: "skills",
                display_name: "anthropics/skills",
                locator: "https://github.com/anthropics/skills",
                tracked_ref: "other",
                resolved_commit_sha: TEST_GITHUB_COMMIT_TWO,
                member_path_hint: None,
            },
            "blocked-second-ref-plan",
            154,
            1_000,
        ) {
            Err(error) => error,
            Ok(_) => panic!("blocked Source 不能签发新的 Tracked Ref Plan"),
        };
        assert!(matches!(
            different_ref_error,
            StorageError::ManagedObjectBlocked
        ));

        let independent_members = [NewSourceCatalogMember {
            id: "independent-catalog-member",
            relative_path: "skills/independent",
            skill_name: Some("independent"),
            description: Some("Independent Skill"),
            content_fingerprint: Some("sha256:independent"),
            selectable: true,
            validation_errors: &[],
            warnings: &[],
        }];
        storage
            .save_source_catalog_success(
                "source-jimliu-baoyu-skills",
                "main",
                TEST_GITHUB_COMMIT_ONE,
                155,
                &independent_members,
            )
            .expect("独立 Source 仍可刷新 Catalog");
        let independent_ref_change = storage
            .save_or_prepare_github_source(
                NewGitHubSource {
                    id: "ignored-independent-source",
                    canonical_identity: "github:jimliu/baoyu-skills",
                    owner: "JimLiu",
                    repository: "baoyu-skills",
                    display_name: "baoyu-skills",
                    locator: "https://github.com/jimliu/baoyu-skills",
                    tracked_ref: "next",
                    resolved_commit_sha: TEST_GITHUB_COMMIT_TWO,
                    member_path_hint: None,
                },
                "independent-ref-change-plan",
                156,
                1_000,
            )
            .expect("独立 Source 应可签发 Ref 变更");
        assert!(matches!(
            independent_ref_change,
            SaveGitHubSourceResult::RefChangeRequired { .. }
        ));
        assert_eq!(
            storage
                .confirm_source_ref_change("independent-ref-change-plan", 157)
                .expect("独立 Source 应可确认 Ref 变更"),
            "source-jimliu-baoyu-skills"
        );
        storage
            .save_source_catalog_failure(
                "source-jimliu-baoyu-skills",
                "next",
                158,
                "独立 Source Reload 失败",
            )
            .expect("独立 Source 仍可记录 Catalog 失败状态");
        let new_source = storage
            .save_or_prepare_github_source(
                NewGitHubSource {
                    id: "new-source-while-other-blocked",
                    canonical_identity: "github:example/new-source",
                    owner: "example",
                    repository: "new-source",
                    display_name: "example/new-source",
                    locator: "https://github.com/example/new-source",
                    tracked_ref: "main",
                    resolved_commit_sha: TEST_GITHUB_COMMIT_ONE,
                    member_path_hint: None,
                },
                "unused-new-source-ref-plan",
                159,
                1_000,
            )
            .expect("其他 Source blocked 时仍可创建全新 Source");
        assert!(matches!(
            new_source,
            SaveGitHubSourceResult::Saved { source_id }
                if source_id == "new-source-while-other-blocked"
        ));
    }

    #[test]
    fn source_association_merge_finalize_supports_root_mapping_and_replay() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let (initial_target, initial_retiring) =
            setup_test_source_association_merge(&mut storage, "finalize");
        storage
            .connection
            .execute(
                "UPDATE source_catalog_members
                 SET relative_path = ''
                 WHERE source_id = ?1 AND relative_path = 'skills/alpha'",
                [TEST_GITHUB_SOURCE_ID],
            )
            .expect("应模拟 Source 根成员");
        storage
            .connection
            .execute(
                "UPDATE source_member_links
                 SET source_relative_path = ''
                 WHERE source_id = ?1",
                [TEST_GITHUB_SOURCE_ID],
            )
            .expect("应把既有 mapping 调整为根成员");
        let retiring_member = storage
            .read_managed_member(&initial_retiring.members[0].id)
            .expect("应读取 retiring Member");
        let mount_target = sandbox
            .path()
            .join("home/.codex/skills")
            .join(&retiring_member.skill_name);
        save_test_mount_plan(
            &mut storage,
            &retiring_member,
            MountOperation::Create,
            "merge-finalize-mount",
            "merge-finalize-mount-plan",
            MountScope::Global,
            None,
            &mount_target,
        );
        finalize_test_mount_create(
            &mut storage,
            "merge-finalize-mount-plan",
            "merge-finalize-mount-tx",
        );
        storage
            .forget_terminal_mount_transaction("merge-finalize-mount-tx")
            .expect("应清理测试 Mount 事务");
        let target = storage
            .read_source_association_bundle(&initial_target.id)
            .expect("应读取根 mapping 目标快照");
        let retiring = storage
            .read_source_association_bundle(&initial_retiring.id)
            .expect("应读取带 Mount 的 retiring 快照");
        assert_eq!(target.members[0].source_relative_path.as_deref(), Some(""));
        let transaction_id =
            begin_test_source_association_merge(&mut storage, "finalize", &target, &retiring);
        advance_source_association_to_mounts_applied(&mut storage, &transaction_id);

        let final_members = target
            .members
            .iter()
            .chain(retiring.members.iter())
            .map(|member| FinalSourceAssociationMember {
                member_id: &member.id,
                skill_name: &member.skill_name,
                description: &member.description,
                stable_relative_path: &member.stable_relative_path,
                content_fingerprint: &member.content_fingerprint,
            })
            .collect::<Vec<_>>();
        let mount_assignments = retiring
            .members
            .iter()
            .flat_map(|member| {
                member
                    .mounts
                    .iter()
                    .map(|mount| FinalSourceAssociationMountAssignment {
                        mount_id: &mount.id,
                        member_id: &member.id,
                    })
            })
            .collect::<Vec<_>>();
        let source_mappings = [FinalSourceAssociationMemberMapping {
            source_relative_path: "",
            member_id: &target.members[0].id,
        }];
        let final_current_target = format!("contents/{transaction_id}");
        let merge = FinalSourceAssociationMerge {
            transaction_id: &transaction_id,
            source_id: TEST_GITHUB_SOURCE_ID,
            expected_target_bundle: &target,
            expected_retiring_bundle: &retiring,
            final_current_target: &final_current_target,
            final_members: &final_members,
            mount_assignments: &mount_assignments,
            source_mappings: &source_mappings,
            now: 150,
        };
        let stored = storage
            .recoverable_source_association_transactions()
            .expect("应读回封存后的最终 Source mapping");
        assert_eq!(
            stored[0].source_mappings_json,
            format!(
                r#"[{{"source_relative_path":"","member_id":"{}"}}]"#,
                target.members[0].id
            )
        );
        let different_source_mappings = [FinalSourceAssociationMemberMapping {
            source_relative_path: "skills/beta",
            member_id: &retiring.members[0].id,
        }];
        assert!(matches!(
            storage
                .finalize_source_association_merge(FinalSourceAssociationMerge {
                    transaction_id: &transaction_id,
                    source_id: TEST_GITHUB_SOURCE_ID,
                    expected_target_bundle: &target,
                    expected_retiring_bundle: &retiring,
                    final_current_target: &final_current_target,
                    final_members: &final_members,
                    mount_assignments: &mount_assignments,
                    source_mappings: &different_source_mappings,
                    now: 150,
                })
                .expect_err("最终提交不能替换 begin 时封存的 Source mapping"),
            StorageError::SourceAssociationStateConflict(_)
        ));
        assert_eq!(
            storage
                .recoverable_source_association_transactions()
                .expect("mapping 不一致后事务仍应可恢复")[0]
                .phase,
            "mounts_applied"
        );
        storage
            .finalize_source_association_merge(merge)
            .expect("应在一个 SQLite 事务中提交最终 Merge 状态");
        let merged = storage
            .read_source_association_bundle(&target.id)
            .expect("应读取最终目标 Bundle");
        assert_eq!(merged.members.len(), 4);
        assert_eq!(merged.current_target, final_current_target);
        assert_eq!(
            merged
                .members
                .iter()
                .find(|member| member.id == target.members[0].id)
                .and_then(|member| member.source_relative_path.as_deref()),
            Some("")
        );
        assert!(matches!(
            storage
                .read_source_association_bundle(&retiring.id)
                .expect_err("retiring Bundle 必须删除"),
            StorageError::ManagedMemberNotFound(_)
        ));
        assert_eq!(
            storage
                .recoverable_source_association_transactions()
                .expect("完成状态在清理前仍可恢复")[0]
                .status,
            "completed"
        );
        storage
            .finalize_source_association_merge(FinalSourceAssociationMerge {
                transaction_id: &transaction_id,
                source_id: TEST_GITHUB_SOURCE_ID,
                expected_target_bundle: &target,
                expected_retiring_bundle: &retiring,
                final_current_target: &final_current_target,
                final_members: &final_members,
                mount_assignments: &mount_assignments,
                source_mappings: &source_mappings,
                now: 151,
            })
            .expect("state_committed 重放只能核验最终事实");

        let mount_id = mount_assignments[0].mount_id;
        storage
            .connection
            .execute(
                "UPDATE mounts SET app_id = 'claude_code' WHERE id = ?1",
                [mount_id],
            )
            .expect("应模拟 completed 后 Mount app 变化");
        assert!(matches!(
            storage.finalize_source_association_merge(FinalSourceAssociationMerge {
                transaction_id: &transaction_id,
                source_id: TEST_GITHUB_SOURCE_ID,
                expected_target_bundle: &target,
                expected_retiring_bundle: &retiring,
                final_current_target: &final_current_target,
                final_members: &final_members,
                mount_assignments: &mount_assignments,
                source_mappings: &source_mappings,
                now: 152,
            }),
            Err(StorageError::SourceBundleStateConflict)
        ));
        storage
            .connection
            .execute(
                "UPDATE mounts SET app_id = 'codex' WHERE id = ?1",
                [mount_id],
            )
            .expect("应恢复 Mount app");

        let replay_project =
            register_test_project(&mut storage, &sandbox.path().join("replay-project"));
        storage
            .connection
            .execute(
                "UPDATE mounts SET scope = 'project', project_id = ?2 WHERE id = ?1",
                params![mount_id, replay_project.id],
            )
            .expect("应模拟 completed 后 Mount scope/project 变化");
        assert!(matches!(
            storage.finalize_source_association_merge(FinalSourceAssociationMerge {
                transaction_id: &transaction_id,
                source_id: TEST_GITHUB_SOURCE_ID,
                expected_target_bundle: &target,
                expected_retiring_bundle: &retiring,
                final_current_target: &final_current_target,
                final_members: &final_members,
                mount_assignments: &mount_assignments,
                source_mappings: &source_mappings,
                now: 153,
            }),
            Err(StorageError::SourceBundleStateConflict)
        ));
        storage
            .connection
            .execute(
                "UPDATE mounts SET scope = 'global', project_id = NULL WHERE id = ?1",
                [mount_id],
            )
            .expect("应恢复 Mount scope/project");

        let changed_target_path = sandbox
            .path()
            .join("alternate/.codex/skills")
            .join(&retiring_member.skill_name);
        storage
            .connection
            .execute(
                "UPDATE mounts SET target_path = ?2 WHERE id = ?1",
                params![
                    mount_id,
                    changed_target_path.to_str().expect("测试路径应是 UTF-8")
                ],
            )
            .expect("应模拟 completed 后 Mount target 变化");
        assert!(matches!(
            storage.finalize_source_association_merge(FinalSourceAssociationMerge {
                transaction_id: &transaction_id,
                source_id: TEST_GITHUB_SOURCE_ID,
                expected_target_bundle: &target,
                expected_retiring_bundle: &retiring,
                final_current_target: &final_current_target,
                final_members: &final_members,
                mount_assignments: &mount_assignments,
                source_mappings: &source_mappings,
                now: 154,
            }),
            Err(StorageError::SourceBundleStateConflict)
        ));
    }

    #[test]
    fn source_association_merge_finalize_race_rolls_back_all_domain_changes() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let (target, retiring) = setup_test_source_association_merge(&mut storage, "race");
        let transaction_id =
            begin_test_source_association_merge(&mut storage, "race", &target, &retiring);
        advance_source_association_to_mounts_applied(&mut storage, &transaction_id);
        storage
            .connection
            .execute(
                "UPDATE skill_members SET description = '并发修改'
                 WHERE id = ?1",
                [&retiring.members[0].id],
            )
            .expect("应模拟 Plan 后成员竞态");
        let final_members = target
            .members
            .iter()
            .chain(retiring.members.iter())
            .map(|member| FinalSourceAssociationMember {
                member_id: &member.id,
                skill_name: &member.skill_name,
                description: &member.description,
                stable_relative_path: &member.stable_relative_path,
                content_fingerprint: &member.content_fingerprint,
            })
            .collect::<Vec<_>>();
        let error = storage
            .finalize_source_association_merge(FinalSourceAssociationMerge {
                transaction_id: &transaction_id,
                source_id: TEST_GITHUB_SOURCE_ID,
                expected_target_bundle: &target,
                expected_retiring_bundle: &retiring,
                final_current_target: &format!("contents/{transaction_id}"),
                final_members: &final_members,
                mount_assignments: &[],
                source_mappings: &[FinalSourceAssociationMemberMapping {
                    source_relative_path: "skills/alpha",
                    member_id: &target.members[0].id,
                }],
                now: 150,
            })
            .expect_err("成员竞态必须拒绝最终提交");
        assert!(matches!(error, StorageError::SourceBundleStateConflict));
        assert!(
            storage.read_source_association_bundle(&retiring.id).is_ok(),
            "失败提交不能删除 retiring Bundle"
        );
        assert_eq!(
            storage
                .recoverable_source_association_transactions()
                .expect("失败后事务仍应可恢复")[0]
                .phase,
            "mounts_applied"
        );
    }

    #[test]
    fn source_association_merge_moves_loser_mount_to_winner_fingerprint() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let (initial_target, initial_retiring) =
            setup_test_source_association_merge(&mut storage, "mount-winner");
        let winner = &initial_target.members[0];
        let loser = &initial_retiring.members[0];
        storage
            .connection
            .execute(
                "UPDATE skill_members
                 SET skill_name = ?2, stable_relative_path = ?3
                 WHERE id = ?1",
                params![loser.id, winner.skill_name, winner.stable_relative_path],
            )
            .expect("应建立同名内容冲突");
        let renamed_loser = storage
            .read_managed_member(&loser.id)
            .expect("应读取同名 loser");
        let mount_target = sandbox
            .path()
            .join("alternate/.codex/skills")
            .join(&winner.skill_name);
        save_test_mount_plan(
            &mut storage,
            &renamed_loser,
            MountOperation::Create,
            "merge-winner-mount",
            "merge-winner-mount-plan",
            MountScope::Global,
            None,
            &mount_target,
        );
        finalize_test_mount_create(
            &mut storage,
            "merge-winner-mount-plan",
            "merge-winner-mount-tx",
        );
        storage
            .forget_terminal_mount_transaction("merge-winner-mount-tx")
            .expect("应清理 loser Mount 事务");
        let target = storage
            .read_source_association_bundle(&initial_target.id)
            .expect("应读取目标快照");
        let retiring = storage
            .read_source_association_bundle(&initial_retiring.id)
            .expect("应读取同名 retiring 快照");
        let transaction_id =
            begin_test_source_association_merge(&mut storage, "mount-winner", &target, &retiring);
        advance_source_association_to_mounts_applied(&mut storage, &transaction_id);
        let final_members = target
            .members
            .iter()
            .chain(
                retiring
                    .members
                    .iter()
                    .filter(|member| member.id != renamed_loser.id),
            )
            .map(|member| FinalSourceAssociationMember {
                member_id: &member.id,
                skill_name: &member.skill_name,
                description: &member.description,
                stable_relative_path: &member.stable_relative_path,
                content_fingerprint: &member.content_fingerprint,
            })
            .collect::<Vec<_>>();
        let mount_assignments = [FinalSourceAssociationMountAssignment {
            mount_id: "merge-winner-mount",
            member_id: &winner.id,
        }];
        let source_mappings = [FinalSourceAssociationMemberMapping {
            source_relative_path: target.members[0]
                .source_relative_path
                .as_deref()
                .expect("winner 应保留 Source mapping"),
            member_id: &winner.id,
        }];
        storage
            .finalize_source_association_merge(FinalSourceAssociationMerge {
                transaction_id: &transaction_id,
                source_id: TEST_GITHUB_SOURCE_ID,
                expected_target_bundle: &target,
                expected_retiring_bundle: &retiring,
                final_current_target: &format!("contents/{transaction_id}"),
                final_members: &final_members,
                mount_assignments: &mount_assignments,
                source_mappings: &source_mappings,
                now: 150,
            })
            .expect("同名 loser Mount 应迁移到 winner");
        let merged = storage
            .read_source_association_bundle(&target.id)
            .expect("应读取 Merge 结果");
        let moved_mount = merged
            .members
            .iter()
            .find(|member| member.id == winner.id)
            .and_then(|member| member.mounts.first())
            .expect("winner 应接收 loser Mount");
        assert_eq!(moved_mount.member_id, winner.id);
        assert_eq!(moved_mount.member_fingerprint, winner.content_fingerprint);
    }

    #[test]
    fn source_association_merge_validates_final_member_and_current_contract() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let (target, retiring) =
            setup_test_source_association_merge(&mut storage, "final-contract");
        let winner = &target.members[0];

        validate_test_source_association_merge(
            &target,
            &retiring,
            &[],
            "contents/merge-validation",
            &winner.description,
            &winner.content_fingerprint,
        )
        .expect("原成员合同与事务 current 目标应通过");
        assert!(matches!(
            validate_test_source_association_merge(
                &target,
                &retiring,
                &[],
                "contents/merge-validation",
                "被篡改的描述",
                &winner.content_fingerprint,
            ),
            Err(StorageError::InvalidSourceAssociationPlan)
        ));
        assert!(matches!(
            validate_test_source_association_merge(
                &target,
                &retiring,
                &[],
                "contents/merge-validation",
                &winner.description,
                "sha256:changed",
            ),
            Err(StorageError::InvalidSourceAssociationPlan)
        ));
        assert!(matches!(
            validate_test_source_association_merge(
                &target,
                &retiring,
                &[],
                "contents/other-transaction",
                &winner.description,
                &winner.content_fingerprint,
            ),
            Err(StorageError::InvalidSourceAssociationPlan)
        ));
    }

    #[test]
    fn source_association_merge_validates_mount_assignment_keys_and_target_basename() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let (target, retiring) =
            setup_test_source_association_merge(&mut storage, "mount-contract");
        let winner = &target.members[0];

        let mut duplicate_global_target = target.clone();
        let mut duplicate_global_retiring = retiring.clone();
        duplicate_global_retiring.members[0].skill_name = winner.skill_name.clone();
        duplicate_global_retiring.members[0].stable_relative_path =
            winner.stable_relative_path.clone();
        duplicate_global_target.members[0].mounts = vec![test_source_association_mount(
            "duplicate-global-one",
            &duplicate_global_target.id,
            &duplicate_global_target.members[0],
            MountScope::Global,
            None,
            &format!("/one/{}", winner.skill_name),
        )];
        duplicate_global_retiring.members[0].mounts = vec![test_source_association_mount(
            "duplicate-global-two",
            &duplicate_global_retiring.id,
            &duplicate_global_retiring.members[0],
            MountScope::Global,
            None,
            &format!("/two/{}", winner.skill_name),
        )];
        let duplicate_global_assignments = [
            FinalSourceAssociationMountAssignment {
                mount_id: "duplicate-global-one",
                member_id: &winner.id,
            },
            FinalSourceAssociationMountAssignment {
                mount_id: "duplicate-global-two",
                member_id: &winner.id,
            },
        ];
        assert!(matches!(
            validate_test_source_association_merge(
                &duplicate_global_target,
                &duplicate_global_retiring,
                &duplicate_global_assignments,
                "contents/merge-validation",
                &winner.description,
                &winner.content_fingerprint,
            ),
            Err(StorageError::InvalidSourceAssociationPlan)
        ));

        let mut duplicate_project_target = duplicate_global_target.clone();
        let mut duplicate_project_retiring = duplicate_global_retiring.clone();
        duplicate_project_target.members[0].mounts[0].scope = MountScope::Project;
        duplicate_project_target.members[0].mounts[0].project_id = Some("project-one".to_owned());
        duplicate_project_retiring.members[0].mounts[0].scope = MountScope::Project;
        duplicate_project_retiring.members[0].mounts[0].project_id = Some("project-one".to_owned());
        assert!(matches!(
            validate_test_source_association_merge(
                &duplicate_project_target,
                &duplicate_project_retiring,
                &duplicate_global_assignments,
                "contents/merge-validation",
                &winner.description,
                &winner.content_fingerprint,
            ),
            Err(StorageError::InvalidSourceAssociationPlan)
        ));

        duplicate_project_retiring.members[0].mounts[0].project_id = Some("project-two".to_owned());
        validate_test_source_association_merge(
            &duplicate_project_target,
            &duplicate_project_retiring,
            &duplicate_global_assignments,
            "contents/merge-validation",
            &winner.description,
            &winner.content_fingerprint,
        )
        .expect("同一 member/app 在不同 Project 中各保留一个 Mount 应合法");

        let mut mixed_scope_target = duplicate_global_target.clone();
        let mut mixed_scope_retiring = duplicate_global_retiring.clone();
        mixed_scope_retiring.members[0].mounts[0].scope = MountScope::Project;
        mixed_scope_retiring.members[0].mounts[0].project_id = Some("project-one".to_owned());
        assert!(matches!(
            validate_test_source_association_merge(
                &mixed_scope_target,
                &mixed_scope_retiring,
                &duplicate_global_assignments,
                "contents/merge-validation",
                &winner.description,
                &winner.content_fingerprint,
            ),
            Err(StorageError::InvalidSourceAssociationPlan)
        ));

        mixed_scope_target.members[0].mounts.clear();
        let wrong_member_assignment = [FinalSourceAssociationMountAssignment {
            mount_id: "duplicate-global-two",
            member_id: &winner.id,
        }];
        mixed_scope_retiring.members[0].skill_name = retiring.members[0].skill_name.clone();
        mixed_scope_retiring.members[0].stable_relative_path =
            retiring.members[0].stable_relative_path.clone();
        mixed_scope_retiring.members[0].mounts[0].skill_name =
            retiring.members[0].skill_name.clone();
        mixed_scope_retiring.members[0].mounts[0].target_path =
            format!("/project/{}", retiring.members[0].skill_name);
        assert!(matches!(
            validate_test_source_association_merge(
                &mixed_scope_target,
                &mixed_scope_retiring,
                &wrong_member_assignment,
                "contents/merge-validation",
                &winner.description,
                &winner.content_fingerprint,
            ),
            Err(StorageError::InvalidSourceAssociationPlan)
        ));
    }

    #[test]
    fn direct_source_association_keeps_partial_mapping_and_mount_state() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_github_catalog(
            &mut storage,
            "catalog-alpha-v1",
            "catalog-beta-v1",
            TEST_GITHUB_COMMIT_ONE,
            "sha256:alpha-v1",
            50,
        );
        let members = save_test_managed_bundle(&mut storage, "partial-association");
        let bundle_id = members[0].bundle_id.clone();
        let member = &members[0];
        let mount_target = sandbox
            .path()
            .join("home/.codex/skills")
            .join(&member.skill_name);
        save_test_mount_plan(
            &mut storage,
            member,
            MountOperation::Create,
            "partial-association-mount",
            "partial-association-mount-plan",
            MountScope::Global,
            None,
            &mount_target,
        );
        finalize_test_mount_create(
            &mut storage,
            "partial-association-mount-plan",
            "partial-association-mount-tx",
        );
        storage
            .forget_terminal_mount_transaction("partial-association-mount-tx")
            .expect("测试 Mount 事务应清理");

        let before = storage
            .read_source_association_bundle(&bundle_id)
            .expect("应从同一快照读取待关联 Bundle");
        assert!(before.source_id.is_none());
        assert_eq!(
            before
                .members
                .iter()
                .map(|member| member.mounts.len())
                .sum::<usize>(),
            1
        );
        save_test_source_association_plan(&mut storage, "partial-association-plan", 100, 1_000);
        let expected_members = members
            .iter()
            .map(|member| DirectSourceAssociationMember {
                member_id: &member.id,
                content_fingerprint: &member.content_fingerprint,
            })
            .collect::<Vec<_>>();
        let mappings = [DirectSourceAssociationMemberMapping {
            member_id: &members[0].id,
            source_relative_path: "skills/alpha",
        }];
        storage
            .finalize_direct_source_association(DirectSourceAssociation {
                plan_id: "partial-association-plan",
                source_id: TEST_GITHUB_SOURCE_ID,
                source_catalog_generation: 1,
                source_marker: TEST_GITHUB_COMMIT_ONE,
                bundle_id: &bundle_id,
                expected_current_target: &before.current_target,
                expected_members: &expected_members,
                member_mappings: &mappings,
                now: 200,
            })
            .expect("直接关联应只写一对一关系与用户选择的映射");
        assert_eq!(
            storage
                .read_source_association_plan("partial-association-plan")
                .expect("确认后仍可读取封存 Plan")
                .status,
            "consumed"
        );
        assert!(matches!(
            storage
                .discard_source_association_plan("partial-association-plan")
                .expect_err("已确认 Plan 不能再次放弃"),
            StorageError::SourceAssociationPlanConsumed
        ));

        let source = storage
            .read_source_install_source(TEST_GITHUB_SOURCE_ID)
            .expect("部分映射的 Source-backed Bundle 必须可读");
        let linked = source.bundle.expect("Source 应关联本地 Bundle");
        let relationship_count = storage
            .connection
            .query_row(
                "SELECT COUNT(*) FROM source_bundle_links
                 WHERE source_id = ?1 AND bundle_id = ?2",
                params![TEST_GITHUB_SOURCE_ID, bundle_id],
                |row| row.get::<_, i64>(0),
            )
            .expect("应检查唯一 Source-Bundle 关系");
        assert_eq!(relationship_count, 1);
        assert!(linked.adopted_marker.is_none());
        assert_eq!(linked.members.len(), 2);
        assert_eq!(
            linked
                .members
                .iter()
                .filter(|member| member.source_relative_path.is_some())
                .count(),
            1
        );
        assert_eq!(
            linked
                .members
                .iter()
                .find(|linked| linked.id == members[0].id)
                .and_then(|linked| linked.source_relative_path.as_deref()),
            Some("skills/alpha")
        );
        let after = storage
            .read_source_association_bundle(&bundle_id)
            .expect("关联后仍应读取同一 Bundle 与 Mount");
        assert_eq!(after.source_id.as_deref(), Some(TEST_GITHUB_SOURCE_ID));
        assert_eq!(
            after
                .members
                .iter()
                .map(|member| member.mounts.len())
                .sum::<usize>(),
            1,
            "直接关联不能改变 Mount"
        );
    }

    #[test]
    fn all_not_corresponding_members_allow_multiple_null_preserved_candidates() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_github_catalog(
            &mut storage,
            "catalog-alpha-v1",
            "catalog-beta-v1",
            TEST_GITHUB_COMMIT_ONE,
            "sha256:alpha-v1",
            50,
        );
        let members = save_test_managed_bundle(&mut storage, "null-association");
        let bundle_id = members[0].bundle_id.clone();
        let bundle = storage
            .read_source_association_bundle(&bundle_id)
            .expect("应读取待关联 Bundle");
        save_test_source_association_plan(&mut storage, "null-association-plan", 100, 1_000);
        let expected_members = members
            .iter()
            .map(|member| DirectSourceAssociationMember {
                member_id: &member.id,
                content_fingerprint: &member.content_fingerprint,
            })
            .collect::<Vec<_>>();
        storage
            .finalize_direct_source_association(DirectSourceAssociation {
                plan_id: "null-association-plan",
                source_id: TEST_GITHUB_SOURCE_ID,
                source_catalog_generation: 1,
                source_marker: TEST_GITHUB_COMMIT_ONE,
                bundle_id: &bundle_id,
                expected_current_target: &bundle.current_target,
                expected_members: &expected_members,
                member_mappings: &[],
                now: 200,
            })
            .expect("全部选择“不对应”仍是合法 Source 关联");
        assert!(
            storage
                .read_source_install_source(TEST_GITHUB_SOURCE_ID)
                .expect("全无映射 Bundle 必须可读")
                .bundle
                .expect("Source 应关联 Bundle")
                .members
                .iter()
                .all(|member| member.source_relative_path.is_none())
        );

        let candidates = [
            NewInstallCandidate {
                candidate_id: &members[0].id,
                source_relative_path: None,
                skill_name: Some(&members[0].skill_name),
                skill_description: Some("第一个测试 Skill"),
                content_fingerprint: Some(&members[0].content_fingerprint),
                previous_content_fingerprint: Some(&members[0].content_fingerprint),
                selectable: false,
                preserve_existing: true,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
            NewInstallCandidate {
                candidate_id: &members[1].id,
                source_relative_path: None,
                skill_name: Some(&members[1].skill_name),
                skill_description: Some("第二个测试 Skill"),
                content_fingerprint: Some(&members[1].content_fingerprint),
                previous_content_fingerprint: Some(&members[1].content_fingerprint),
                selectable: false,
                preserve_existing: true,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
            NewInstallCandidate {
                candidate_id: "catalog-alpha-v1",
                source_relative_path: Some("skills/alpha"),
                skill_name: Some("alpha"),
                skill_description: Some("Alpha Skill"),
                content_fingerprint: Some("sha256:alpha-v1"),
                previous_content_fingerprint: None,
                selectable: true,
                preserve_existing: false,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
            NewInstallCandidate {
                candidate_id: "catalog-beta-v1",
                source_relative_path: Some("skills/beta"),
                skill_name: Some("beta"),
                skill_description: Some("Beta Skill"),
                content_fingerprint: Some("sha256:beta"),
                previous_content_fingerprint: None,
                selectable: true,
                preserve_existing: false,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
        ];
        storage
            .save_install_plan(NewInstallPlan {
                id: "null-preserved-supplement",
                kind: "source_snapshot",
                install_mode: "supplement",
                input_path: None,
                input_device: 10,
                input_inode: 20,
                input_fingerprint: "sha256:null-preserved-snapshot",
                snapshot_relative_path: Some("staging/null-preserved-supplement/source"),
                source_id: Some(TEST_GITHUB_SOURCE_ID),
                source_tracked_ref: Some("main"),
                source_catalog_generation: Some(1),
                source_marker: Some(TEST_GITHUB_COMMIT_ONE),
                expected_source_marker: None,
                expected_current_target: Some(&bundle.current_target),
                expected_adopted_marker: None,
                bundle_id: &bundle_id,
                bundle_display_name: &bundle.display_name,
                warnings: &[],
                candidates: &candidates,
                created_at: 300,
                expires_at: 1_000,
            })
            .expect("同一 supplement Plan 应允许多个无映射保留成员");
        let stored = storage
            .read_install_plan("null-preserved-supplement")
            .expect("应读回 optional mapping");
        assert_eq!(
            stored
                .candidates
                .iter()
                .filter(|candidate| {
                    candidate.preserve_existing && candidate.source_relative_path.is_none()
                })
                .count(),
            2
        );
    }

    #[test]
    fn direct_source_association_race_rolls_back_relationship_and_plan_consumption() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_github_catalog(
            &mut storage,
            "catalog-alpha-v1",
            "catalog-beta-v1",
            TEST_GITHUB_COMMIT_ONE,
            "sha256:alpha-v1",
            50,
        );
        let members = save_test_managed_bundle(&mut storage, "association-race");
        let bundle_id = members[0].bundle_id.clone();
        let before = storage
            .read_source_association_bundle(&bundle_id)
            .expect("应读取关联前快照");
        save_test_source_association_plan(&mut storage, "association-race-plan", 100, 1_000);
        storage
            .connection
            .execute(
                "UPDATE bundles
                 SET current_target = 'contents/raced'
                 WHERE id = ?1",
                [&bundle_id],
            )
            .expect("应模拟 Plan 后 Bundle current 变化");
        let expected_members = members
            .iter()
            .map(|member| DirectSourceAssociationMember {
                member_id: &member.id,
                content_fingerprint: &member.content_fingerprint,
            })
            .collect::<Vec<_>>();
        let error = storage
            .finalize_direct_source_association(DirectSourceAssociation {
                plan_id: "association-race-plan",
                source_id: TEST_GITHUB_SOURCE_ID,
                source_catalog_generation: 1,
                source_marker: TEST_GITHUB_COMMIT_ONE,
                bundle_id: &bundle_id,
                expected_current_target: &before.current_target,
                expected_members: &expected_members,
                member_mappings: &[],
                now: 200,
            })
            .expect_err("快照变化必须拒绝确认");
        assert!(matches!(error, StorageError::SourceBundleStateConflict));
        let relationship_count = storage
            .connection
            .query_row(
                "SELECT COUNT(*) FROM source_bundle_links
                 WHERE source_id = ?1 OR bundle_id = ?2",
                params![TEST_GITHUB_SOURCE_ID, bundle_id],
                |row| row.get::<_, i64>(0),
            )
            .expect("应检查一对一关系");
        assert_eq!(relationship_count, 0, "失败确认不能留下半条关系");
        assert_eq!(
            storage
                .read_source_association_plan("association-race-plan")
                .expect("失败确认必须保留 Plan")
                .status,
            "pending"
        );
    }

    #[test]
    fn install_bundle_migration_rejects_an_existing_lifecycle_transaction() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        fs::create_dir(&data_root).expect("应创建数据目录");
        let database = data_root.join("skillyard.sqlite3");
        let connection = Connection::open(&database).expect("应创建旧版 SQLite");
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                 );",
            )
            .expect("应建立 migration 表");
        for (version, migration) in MIGRATIONS.iter().take(12) {
            connection
                .execute_batch(migration)
                .expect("应建立 version 12 schema");
            connection
                .execute(
                    "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, 1)",
                    [version],
                )
                .expect("应记录旧 migration");
        }
        connection
            .execute(
                "INSERT INTO install_plans (
                    id, kind, input_path, input_device, input_inode, input_fingerprint,
                    bundle_id, bundle_display_name, member_id, skill_name,
                    skill_description, warnings_json, created_at, expires_at, status
                 ) VALUES (
                    'old-plan', 'folder_snapshot', '/tmp/old', 1, 2, 'sha256:old',
                    'old-bundle', 'Old', 'old-member', 'old-member',
                    '旧事务', '[]', 1, 2, 'consumed'
                 )",
                [],
            )
            .expect("应建立旧 Plan");
        connection
            .execute(
                "INSERT INTO lifecycle_transactions (
                    id, kind, plan_id, bundle_id, member_id, journal_path,
                    phase, status, created_at, updated_at
                 ) VALUES (
                    'old-transaction', 'install_folder', 'old-plan', 'old-bundle',
                    'old-member', 'journals/old.json', 'journal_pending',
                    'in_progress', 1, 1
                 )",
                [],
            )
            .expect("应建立旧生命周期事务");
        drop(connection);

        let error = Storage::open(&data_root, &database)
            .err()
            .expect("非空旧事务必须阻止迁移");
        assert!(matches!(error, StorageError::Migration(_)));
        let connection = Connection::open(&database).expect("失败后旧库应保持可读");
        let latest = connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("应读取旧 migration 版本");
        let transaction_count = connection
            .query_row("SELECT COUNT(*) FROM lifecycle_transactions", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("失败迁移不能删除旧事务");
        assert_eq!((latest, transaction_count), (12, 1));
    }

    #[test]
    fn install_bundle_schema_rejects_mixed_plan_and_preserved_candidate_shapes() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let mixed_plan = storage.connection.execute(
            "INSERT INTO install_plans (
                id, kind, install_mode, input_path, input_device, input_inode,
                input_fingerprint, snapshot_relative_path, source_id, source_tracked_ref,
                source_catalog_generation, source_marker, expected_current_target,
                expected_adopted_marker, bundle_id, bundle_display_name,
                warnings_json, created_at, expires_at, status
             ) VALUES (
                'mixed-plan', 'folder_snapshot', 'supplement', '/tmp/input', 1, 2,
                'sha256:test', NULL, NULL, NULL, NULL, NULL, 'contents/old', NULL,
                'bundle', 'Bundle', '[]', 1, 2, 'pending'
             )",
            [],
        );
        assert!(mixed_plan.is_err(), "folder Plan 不能伪装成 supplement");

        save_test_plan(
            &mut storage,
            "folder-plan",
            "folder-bundle",
            "folder-member",
        );
        let invalid_preserved = storage.connection.execute(
            "INSERT INTO install_plan_candidates (
                plan_id, candidate_id, source_relative_path, skill_name,
                skill_description, content_fingerprint, selectable, preserve_existing,
                validation_errors_json, warnings_json, default_selected, selected, sort_order
             ) VALUES (
                'folder-plan', 'preserved', 'preserved', 'preserved',
                'Preserved', 'sha256:preserved', 1, 1, '[]', '[]', 1, 1, 1
             )",
            [],
        );
        assert!(
            invalid_preserved.is_err(),
            "preserved 成员必须是不可选择且由 Core 强制保留"
        );
    }

    #[test]
    fn manual_source_reuses_identity_and_installs_through_source_snapshot() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let first_members = [NewSourceCatalogMember {
            id: "manual-alpha-v1",
            relative_path: "skills/alpha",
            skill_name: Some("alpha"),
            description: Some("Alpha Skill"),
            content_fingerprint: Some("sha256:alpha-v1"),
            selectable: true,
            validation_errors: &[],
            warnings: &[],
        }];
        let first = storage
            .save_manual_source_with_catalog(NewManualSource {
                id: "manual-source-one",
                kind: "archive",
                canonical_identity: "archive:/tmp/SuperPowers.skill",
                display_name: "SuperPowers",
                locator: "/tmp/SuperPowers.skill",
                filesystem_device: None,
                filesystem_inode: None,
                catalog_marker: "sha256:archive-one",
                members: &first_members,
                saved_at: 100,
            })
            .expect("应保存大小写敏感的 Archive Source");
        assert_eq!(first.catalog_generation, 1);

        let second_members = [NewSourceCatalogMember {
            id: "manual-alpha-v2",
            relative_path: "skills/alpha",
            skill_name: Some("alpha"),
            description: Some("Alpha Skill"),
            content_fingerprint: Some("sha256:alpha-v2"),
            selectable: true,
            validation_errors: &[],
            warnings: &[],
        }];
        let second = storage
            .save_manual_source_with_catalog(NewManualSource {
                id: "ignored-new-id",
                kind: "archive",
                canonical_identity: "archive:/tmp/SuperPowers.skill",
                display_name: "SuperPowers",
                locator: "/tmp/SuperPowers.skill",
                filesystem_device: None,
                filesystem_inode: None,
                catalog_marker: "sha256:archive-two",
                members: &second_members,
                saved_at: 200,
            })
            .expect("相同 identity 应刷新原 Source");
        assert_eq!(second.source_id, first.source_id);
        assert_eq!(second.catalog_generation, 2);

        let source = storage
            .read_source_install_source(&second.source_id)
            .expect("手动来源应通过通用 Source reader 读取");
        assert_eq!(source.kind, "archive");
        assert_eq!(source.canonical_identity, "archive:/tmp/SuperPowers.skill");
        assert_eq!(source.catalog_marker, "sha256:archive-two");
        assert_eq!(source.catalog_members.len(), 1);
        assert!(source.owner.is_none());
        assert!(source.repository.is_none());
        assert!(source.tracked_ref.is_none());

        let candidates = [NewInstallCandidate {
            candidate_id: "manual-alpha-v2",
            source_relative_path: Some("skills/alpha"),
            skill_name: Some("alpha"),
            skill_description: Some("Alpha Skill"),
            content_fingerprint: Some("sha256:alpha-v2"),
            previous_content_fingerprint: None,
            selectable: true,
            preserve_existing: false,
            validation_errors: &[],
            warnings: &[],
            default_selected: true,
        }];
        storage
            .save_install_plan(NewInstallPlan {
                id: "manual-source-plan",
                kind: "source_snapshot",
                install_mode: "create",
                input_path: None,
                input_device: 10,
                input_inode: 20,
                input_fingerprint: "sha256:manual-snapshot",
                snapshot_relative_path: Some("staging/manual-source-plan/source"),
                source_id: Some(&source.id),
                source_tracked_ref: None,
                source_catalog_generation: Some(source.catalog_generation),
                source_marker: Some(&source.catalog_marker),
                expected_source_marker: None,
                expected_current_target: None,
                expected_adopted_marker: None,
                bundle_id: "manual-bundle",
                bundle_display_name: "SuperPowers",
                warnings: &[],
                candidates: &candidates,
                created_at: 210,
                expires_at: 1_000,
            })
            .expect("手动来源应保存为同一种 source_snapshot Plan");
        let plan = storage
            .begin_install_transaction(
                "manual-source-plan",
                "manual-source-transaction",
                "journals/manual-source.json",
                300,
            )
            .expect("手动来源应开始唯一生命周期事务");
        advance_to_candidate_ready(&mut storage, "manual-source-transaction");
        storage
            .finalize_install(
                "manual-source-transaction",
                &plan,
                "bundles/manual-bundle",
                "contents/manual-source-transaction",
                "members/alpha",
                400,
            )
            .expect("手动来源应原子保存 Source-Bundle 关联");
        let linked = storage
            .connection
            .query_row(
                "SELECT adopted_marker
                 FROM source_bundle_links
                 WHERE source_id = ?1",
                [&source.id],
                |row| row.get::<_, String>(0),
            )
            .expect("应保存采用的通用 marker");
        assert_eq!(linked, "sha256:archive-two");
    }

    #[test]
    fn blocked_manual_source_rejects_refresh_without_changing_metadata() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let original_members = [NewSourceCatalogMember {
            id: "blocked-alpha",
            relative_path: "skills/alpha",
            skill_name: Some("alpha"),
            description: Some("Alpha Skill"),
            content_fingerprint: Some("sha256:alpha-original"),
            selectable: true,
            validation_errors: &[],
            warnings: &[],
        }];
        let saved = storage
            .save_manual_source_with_catalog(NewManualSource {
                id: "blocked-source",
                kind: "archive",
                canonical_identity: "archive:/tmp/Blocked.skill",
                display_name: "Blocked Original",
                locator: "/tmp/Blocked.skill",
                filesystem_device: None,
                filesystem_inode: None,
                catalog_marker: "sha256:archive-original",
                members: &original_members,
                saved_at: 100,
            })
            .expect("应先保存待阻塞的 Source");
        let candidates = [NewInstallCandidate {
            candidate_id: "blocked-alpha",
            source_relative_path: Some("skills/alpha"),
            skill_name: Some("alpha"),
            skill_description: Some("Alpha Skill"),
            content_fingerprint: Some("sha256:alpha-original"),
            previous_content_fingerprint: None,
            selectable: true,
            preserve_existing: false,
            validation_errors: &[],
            warnings: &[],
            default_selected: true,
        }];
        storage
            .save_install_plan(NewInstallPlan {
                id: "blocked-source-plan",
                kind: "source_snapshot",
                install_mode: "create",
                input_path: None,
                input_device: 10,
                input_inode: 20,
                input_fingerprint: "sha256:blocked-snapshot",
                snapshot_relative_path: Some("staging/blocked-source-plan/source"),
                source_id: Some(&saved.source_id),
                source_tracked_ref: None,
                source_catalog_generation: Some(saved.catalog_generation),
                source_marker: Some("sha256:archive-original"),
                expected_source_marker: None,
                expected_current_target: None,
                expected_adopted_marker: None,
                bundle_id: "blocked-bundle",
                bundle_display_name: "Blocked Original",
                warnings: &[],
                candidates: &candidates,
                created_at: 110,
                expires_at: 1_000,
            })
            .expect("应保存 Source Plan");
        storage
            .begin_install_transaction(
                "blocked-source-plan",
                "blocked-source-transaction",
                "journals/blocked-source.json",
                120,
            )
            .expect("应开始 Source 生命周期事务");
        storage
            .block_lifecycle_transaction("blocked-source-transaction", "测试人工恢复阻塞", 130)
            .expect("应把 Source 事务标记为 blocked");
        let before = storage
            .read_source_install_source(&saved.source_id)
            .expect("阻塞前 metadata 应可读取");

        let changed_members = [NewSourceCatalogMember {
            id: "blocked-alpha-new",
            relative_path: "skills/alpha",
            skill_name: Some("alpha"),
            description: Some("Changed Skill"),
            content_fingerprint: Some("sha256:alpha-changed"),
            selectable: true,
            validation_errors: &[],
            warnings: &[],
        }];
        let error = storage
            .save_manual_source_with_catalog(NewManualSource {
                id: "ignored-blocked-source",
                kind: "archive",
                canonical_identity: "archive:/tmp/Blocked.skill",
                display_name: "Blocked Changed",
                locator: "/tmp/Blocked.skill",
                filesystem_device: None,
                filesystem_inode: None,
                catalog_marker: "sha256:archive-changed",
                members: &changed_members,
                saved_at: 200,
            })
            .expect_err("blocked Source 不能刷新 metadata");
        assert!(matches!(error, StorageError::ManagedObjectBlocked));
        let after = storage
            .read_source_install_source(&saved.source_id)
            .expect("被拒绝后原 metadata 应继续可读");
        assert_eq!(after, before, "被阻塞的 Source 不能发生部分更新");
    }

    #[test]
    fn github_create_finalize_persists_one_source_backed_bundle() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_github_catalog(
            &mut storage,
            "catalog-alpha-v1",
            "catalog-beta-v1",
            TEST_GITHUB_COMMIT_ONE,
            "sha256:alpha-v1",
            50,
        );
        let source = storage
            .read_source_install_source(TEST_GITHUB_SOURCE_ID)
            .expect("Fresh Catalog 应可组成安装输入");
        assert_eq!(source.kind, "github");
        assert_eq!(source.locator, "https://github.com/anthropics/skills");
        assert_eq!(source.catalog_generation, 1);
        assert_eq!(source.catalog_marker, TEST_GITHUB_COMMIT_ONE);
        assert_eq!(source.catalog_members.len(), 2);
        assert_eq!(
            source.catalog_members[0].content_fingerprint.as_deref(),
            Some("sha256:alpha-v1")
        );
        assert!(source.bundle.is_none());

        save_test_github_create_plan(&mut storage, "github-create-plan", "github-bundle");
        let selected = vec!["catalog-alpha-v1".to_owned(), "catalog-beta-v1".to_owned()];
        let plan = storage
            .begin_install_transaction_with_selection(
                "github-create-plan",
                &selected,
                "github-create-txn",
                "journals/github-create.json",
                200,
            )
            .expect("应开始统一安装事务");
        let anchor = storage
            .connection
            .query_row(
                "SELECT kind, member_id FROM lifecycle_transactions WHERE id = ?1",
                ["github-create-txn"],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("应读取安装事务 anchor");
        assert_eq!(
            anchor,
            ("install_bundle".to_owned(), "catalog-alpha-v1".to_owned())
        );
        advance_to_candidate_ready(&mut storage, "github-create-txn");
        storage
            .finalize_install(
                "github-create-txn",
                &plan,
                "bundles/github-bundle",
                "contents/github-create-txn",
                "members/alpha",
                203,
            )
            .expect("应原子提交 Source-backed Bundle");

        let source = storage
            .read_source_install_source(TEST_GITHUB_SOURCE_ID)
            .expect("应读取已关联 Source");
        let bundle = source.bundle.expect("create 应关联唯一 Bundle");
        assert_eq!(bundle.id, "github-bundle");
        assert_eq!(
            bundle.adopted_marker.as_deref(),
            Some(TEST_GITHUB_COMMIT_ONE)
        );
        assert_eq!(bundle.members.len(), 2);
        assert_eq!(
            bundle
                .members
                .iter()
                .filter_map(|member| member.source_relative_path.as_deref())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["skills/alpha", "skills/beta"])
        );
        let mount_count = storage
            .connection
            .query_row("SELECT COUNT(*) FROM mounts", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("应读取 Mount 数量");
        assert_eq!(mount_count, 0, "GitHub 安装不能自动创建 Mount");
    }

    #[test]
    fn github_supplement_keeps_existing_member_mount_and_adopted_commit() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        create_test_github_bundle_with_alpha(
            &mut storage,
            "seed-create-plan",
            "seed-create-txn",
            "github-bundle",
        );
        let member = storage
            .read_managed_member("catalog-alpha-v1")
            .expect("应读取旧成员");
        let mount_target = sandbox.path().join("home/.codex/skills/alpha");
        save_test_mount_plan(
            &mut storage,
            &member,
            MountOperation::Create,
            "alpha-mount",
            "alpha-mount-plan",
            MountScope::Global,
            None,
            &mount_target,
        );
        finalize_test_mount_create(&mut storage, "alpha-mount-plan", "alpha-mount-txn");
        storage
            .forget_terminal_mount_transaction("alpha-mount-txn")
            .expect("应清理 Mount 事务");
        let mount_before = storage
            .connection
            .query_row(
                "SELECT member_id, target_path, expected_target, created_at
                 FROM mounts WHERE id = 'alpha-mount'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .expect("应读取旧 Mount");

        save_test_github_catalog(
            &mut storage,
            "catalog-alpha-v2",
            "catalog-beta-v2",
            TEST_GITHUB_COMMIT_TWO,
            "sha256:alpha-upstream-changed",
            300,
        );
        save_test_github_supplement_plan(
            &mut storage,
            "github-supplement-plan",
            "github-bundle",
            "contents/seed-create-txn",
        );
        let preserved_selection_error = storage
            .begin_install_transaction_with_selection(
                "github-supplement-plan",
                &["catalog-alpha-v1".to_owned()],
                "invalid-preserved-txn",
                "journals/invalid-preserved.json",
                400,
            )
            .expect_err("用户不能把 preserved 成员作为选择提交");
        assert!(matches!(
            preserved_selection_error,
            StorageError::InvalidInstallSelection
        ));
        let plan = storage
            .begin_install_transaction_with_selection(
                "github-supplement-plan",
                &["catalog-beta-v2".to_owned()],
                "github-supplement-txn",
                "journals/github-supplement.json",
                400,
            )
            .expect("应开始 supplement 事务");
        assert!(
            plan.candidates
                .iter()
                .find(|candidate| candidate.candidate_id == "catalog-alpha-v1")
                .is_some_and(|candidate| candidate.preserve_existing && candidate.selected),
            "旧成员必须由 Core 自动保留"
        );
        let anchor = storage
            .recoverable_lifecycle_transactions()
            .expect("应读取 supplement 事务")
            .into_iter()
            .find(|transaction| transaction.id == "github-supplement-txn")
            .expect("supplement 事务应存在")
            .member_id;
        assert_eq!(anchor, "catalog-alpha-v1");
        advance_to_candidate_ready(&mut storage, "github-supplement-txn");
        storage
            .finalize_install(
                "github-supplement-txn",
                &plan,
                "bundles/github-bundle",
                "contents/github-supplement-txn",
                "members/alpha",
                403,
            )
            .expect("应原子补装新成员");

        let state = storage
            .connection
            .query_row(
                "SELECT bundle.current_target, link.adopted_marker,
                        (SELECT COUNT(*) FROM skill_members WHERE bundle_id = bundle.id),
                        (SELECT content_fingerprint FROM skill_members
                         WHERE id = 'catalog-alpha-v1')
                 FROM bundles AS bundle
                 JOIN source_bundle_links AS link ON link.bundle_id = bundle.id
                 WHERE bundle.id = 'github-bundle'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .expect("应读取 supplement 后状态");
        assert_eq!(state.0, "contents/github-supplement-txn");
        assert_eq!(state.1.as_deref(), Some(TEST_GITHUB_COMMIT_ONE));
        assert_eq!(state.2, 2);
        assert_eq!(state.3, "sha256:alpha-v1");
        let mount_after = storage
            .connection
            .query_row(
                "SELECT member_id, target_path, expected_target, created_at
                 FROM mounts WHERE id = 'alpha-mount'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .expect("应读取保留的 Mount");
        assert_eq!(mount_after, mount_before);
    }

    #[test]
    fn github_begin_rolls_back_when_source_catalog_or_current_changes() {
        let stale_sandbox = tempdir().expect("应创建隔离测试目录");
        let mut stale_storage = open_test_storage(stale_sandbox.path());
        save_test_github_catalog(
            &mut stale_storage,
            "catalog-alpha-v1",
            "catalog-beta-v1",
            TEST_GITHUB_COMMIT_ONE,
            "sha256:alpha-v1",
            50,
        );
        save_test_github_create_plan(&mut stale_storage, "stale-plan", "stale-bundle");
        stale_storage
            .save_source_catalog_failure(TEST_GITHUB_SOURCE_ID, "main", 60, "测试网络失败")
            .expect("应把 Source 标记为 stale");
        let stale_error = stale_storage
            .begin_install_transaction_with_selection(
                "stale-plan",
                &["catalog-alpha-v1".to_owned()],
                "stale-txn",
                "journals/stale.json",
                200,
            )
            .expect_err("stale Source 不能授权安装");
        assert!(matches!(
            stale_error,
            StorageError::SourceCatalogStateChanged
        ));
        assert_eq!(
            stale_storage
                .read_install_plan("stale-plan")
                .expect("失败确认不能消耗 Plan")
                .status,
            "pending"
        );

        let catalog_sandbox = tempdir().expect("应创建隔离测试目录");
        let mut catalog_storage = open_test_storage(catalog_sandbox.path());
        save_test_github_catalog(
            &mut catalog_storage,
            "catalog-alpha-v1",
            "catalog-beta-v1",
            TEST_GITHUB_COMMIT_ONE,
            "sha256:alpha-v1",
            50,
        );
        save_test_github_create_plan(&mut catalog_storage, "catalog-plan", "catalog-bundle");
        save_test_github_catalog(
            &mut catalog_storage,
            "catalog-alpha-v2",
            "catalog-beta-v2",
            TEST_GITHUB_COMMIT_TWO,
            "sha256:alpha-v2",
            60,
        );
        let catalog_error = catalog_storage
            .begin_install_transaction_with_selection(
                "catalog-plan",
                &["catalog-alpha-v1".to_owned()],
                "catalog-txn",
                "journals/catalog.json",
                200,
            )
            .expect_err("Catalog generation 变化后必须重新生成 Plan");
        assert!(matches!(
            catalog_error,
            StorageError::SourceCatalogStateChanged
        ));
        assert_eq!(
            catalog_storage
                .connection
                .query_row("SELECT COUNT(*) FROM lifecycle_transactions", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("应读取事务数量"),
            0
        );

        let current_sandbox = tempdir().expect("应创建隔离测试目录");
        let mut current_storage = open_test_storage(current_sandbox.path());
        create_test_github_bundle_with_alpha(
            &mut current_storage,
            "current-seed-plan",
            "current-seed-txn",
            "current-bundle",
        );
        save_test_github_catalog(
            &mut current_storage,
            "catalog-alpha-v2",
            "catalog-beta-v2",
            TEST_GITHUB_COMMIT_TWO,
            "sha256:alpha-v2",
            300,
        );
        save_test_github_supplement_plan(
            &mut current_storage,
            "current-plan",
            "current-bundle",
            "contents/current-seed-txn",
        );
        current_storage
            .connection
            .execute(
                "UPDATE bundles SET current_target = 'contents/external'
                 WHERE id = 'current-bundle'",
                [],
            )
            .expect("应模拟 current 前置状态变化");
        let current_error = current_storage
            .begin_install_transaction_with_selection(
                "current-plan",
                &["catalog-beta-v2".to_owned()],
                "current-txn",
                "journals/current.json",
                400,
            )
            .expect_err("current baseline 变化后不能开始 supplement");
        assert!(matches!(
            current_error,
            StorageError::SourceBundleStateConflict
        ));
        assert_eq!(
            current_storage
                .read_install_plan("current-plan")
                .expect("失败确认不能消耗 supplement Plan")
                .status,
            "pending"
        );
        assert_eq!(
            current_storage
                .connection
                .query_row("SELECT COUNT(*) FROM lifecycle_transactions", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("应读取事务数量"),
            0
        );
    }

    #[test]
    fn takeover_phase_api_stops_before_domain_commit() {
        assert_eq!(
            previous_takeover_phase("journal_ready").unwrap(),
            Some("journal_pending")
        );
        assert_eq!(
            previous_takeover_phase("candidate_ready").unwrap(),
            Some("journal_ready")
        );
        assert_eq!(
            previous_takeover_phase("current_activated").unwrap(),
            Some("candidate_ready")
        );
        assert_eq!(
            previous_takeover_phase("origins_applied").unwrap(),
            Some("current_activated")
        );
        assert_eq!(previous_takeover_phase("state_committed").unwrap(), None);
        assert!(matches!(
            previous_takeover_phase("unknown"),
            Err(StorageError::InvalidTakeoverPhase(phase)) if phase == "unknown"
        ));
    }

    #[test]
    fn takeover_transaction_persists_and_validates_its_affected_object_and_paths() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let plan = takeover_plan_for(&storage, sandbox.path(), "transaction-subject");
        begin_test_takeover(&mut storage, &plan, "takeover-txn-subject");

        let transactions = storage
            .recoverable_takeover_transactions()
            .expect("应读取 Takeover 恢复对象");
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].bundle_id, plan.bundle_id);
        assert_eq!(transactions[0].member_id, plan.members[0].member_id);
        assert_eq!(
            transactions[0].reserved_paths,
            takeover_reserved_paths(&plan).expect("测试 Plan 应产生合法保留路径")
        );

        let mut mismatched = plan.clone();
        // 使用另一个合法 ID，只验证事务锚点不匹配，避免把无效 Plan 与状态冲突混在一起。
        let mismatched_member_id = "00000000-0000-4000-8000-000000000001".to_owned();
        mismatched.members[0].member_id = mismatched_member_id.clone();
        for origin in &mut mismatched.origins {
            origin.member_id = mismatched_member_id.clone();
        }
        for target in &mut mismatched.targets {
            target.member_id = mismatched_member_id.clone();
        }
        assert!(matches!(
            storage.finalize_takeover("takeover-txn-subject", &mismatched, None, 202),
            Err(StorageError::TakeoverStateConflict(id)) if id == "takeover-txn-subject"
        ));

        let different_paths = serde_json::to_string(&vec![
            sandbox
                .path()
                .join("different")
                .to_string_lossy()
                .into_owned(),
        ])
        .expect("不同保留路径应能编码为 JSON");
        storage
            .connection
            .execute(
                "UPDATE takeover_transactions SET reserved_paths_json = ?2 WHERE id = ?1",
                params!["takeover-txn-subject", different_paths],
            )
            .expect("应能模拟 SQLite 中与 Plan 不一致的保留路径");
        assert!(matches!(
            storage.finalize_takeover("takeover-txn-subject", &plan, None, 203),
            Err(StorageError::TakeoverStateConflict(id)) if id == "takeover-txn-subject"
        ));

        storage
            .connection
            .execute(
                "UPDATE takeover_transactions SET reserved_paths_json = 'not-json' WHERE id = ?1",
                ["takeover-txn-subject"],
            )
            .expect("应能模拟损坏的保留路径 JSON");
        assert!(matches!(
            storage.recoverable_takeover_transactions(),
            Err(StorageError::TakeoverStateConflict(id)) if id == "takeover-txn-subject"
        ));
    }

    #[test]
    fn takeover_migration_rejects_inconsistent_phase_and_status() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let plan = takeover_plan_for(&storage, sandbox.path(), "phase-check");
        storage
            .save_takeover_plan(&StoredTakeoverPlanRow {
                id: plan.id.clone(),
                payload_json: "{}".to_owned(),
                payload_sha256: "0".repeat(64),
                status: "pending".to_owned(),
                created_at: plan.created_at,
                expires_at: plan.expires_at,
            })
            .expect("应保存约束测试 Plan");
        let reserved_paths_json = serde_json::to_string(
            &takeover_reserved_paths(&plan).expect("测试 Plan 应产生合法保留路径"),
        )
        .expect("保留路径应能编码为 JSON");

        for (id, phase, status) in [
            ("completed-too-early", "origins_applied", "completed"),
            ("uncommitted-final-phase", "state_committed", "in_progress"),
        ] {
            let result = storage.connection.execute(
                "INSERT INTO takeover_transactions (
                    id, plan_id, bundle_id, member_id, reserved_paths_json, journal_path,
                    phase, status, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, 1)",
                params![
                    id,
                    plan.id,
                    plan.bundle_id,
                    plan.members[0].member_id,
                    reserved_paths_json,
                    format!("journals/{id}.json"),
                    phase,
                    status
                ],
            );
            assert!(result.is_err(), "SQLite 必须拒绝 {phase}/{status}");
        }

        storage
            .connection
            .execute(
                "INSERT INTO takeover_transactions (
                    id, plan_id, bundle_id, member_id, reserved_paths_json, journal_path,
                    phase, status, created_at, updated_at
                 ) VALUES (
                    'blocked-after-commit', ?1, ?2, ?3, ?4, 'journals/blocked.json',
                    'state_committed', 'blocked', 1, 1
                 )",
                params![
                    plan.id,
                    plan.bundle_id,
                    plan.members[0].member_id,
                    reserved_paths_json
                ],
            )
            .expect("提交后的人工恢复必须可以保持 blocked");
    }

    #[test]
    fn takeover_first_commit_rolls_back_when_any_domain_identity_exists() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let plan = takeover_plan_for(&storage, sandbox.path(), "identity-conflict");
        begin_test_takeover(&mut storage, &plan, "takeover-txn-conflict");
        assert!(matches!(
            storage.update_takeover_transaction_phase(
                "takeover-txn-conflict",
                "state_committed",
                205,
            ),
            Err(StorageError::TakeoverStateConflict(id)) if id == "takeover-txn-conflict"
        ));
        storage
            .connection
            .execute(
                "INSERT INTO bundles (id, display_name, managed_directory, current_target, created_at)
                 VALUES (?1, '已有 Bundle', ?2, 'contents/existing', 1)",
                params![plan.bundle_id, format!("bundles/{}", plan.bundle_id)],
            )
            .expect("应预置冲突 Bundle");

        assert!(matches!(
            storage.finalize_takeover("takeover-txn-conflict", &plan, None, 206),
            Err(StorageError::SaveTakeoverTransaction(_))
        ));
        let (display_name, member_count, mount_count, observation_count, phase, status) = storage
            .connection
            .query_row(
                "SELECT
                    (SELECT display_name FROM bundles WHERE id = ?1),
                    (SELECT COUNT(*) FROM skill_members WHERE id = ?2),
                    (SELECT COUNT(*) FROM mounts WHERE id = ?3),
                    (SELECT COUNT(*) FROM inventory_observations WHERE id = ?4),
                    (SELECT phase FROM takeover_transactions WHERE id = 'takeover-txn-conflict'),
                    (SELECT status FROM takeover_transactions WHERE id = 'takeover-txn-conflict')",
                params![
                    plan.bundle_id,
                    plan.members[0].member_id,
                    plan.targets[0].mount_id,
                    plan.origins[0].observation_id,
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .expect("应读取冲突后的原子状态");
        assert_eq!(display_name, "已有 Bundle");
        assert_eq!((member_count, mount_count, observation_count), (0, 0, 1));
        assert_eq!(
            (phase.as_str(), status.as_str()),
            ("origins_applied", "in_progress")
        );
    }

    #[test]
    fn completed_takeover_replay_never_recreates_missing_domain_rows() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let plan = takeover_plan_for(&storage, sandbox.path(), "completed-replay");
        begin_test_takeover(&mut storage, &plan, "takeover-txn-replay");
        storage
            .finalize_takeover("takeover-txn-replay", &plan, None, 205)
            .expect("首次 Takeover 提交应成功");
        storage
            .connection
            .execute(
                "DELETE FROM mounts WHERE id = ?1",
                [&plan.targets[0].mount_id],
            )
            .expect("应模拟提交后的领域记录丢失");

        assert!(matches!(
            storage.finalize_takeover("takeover-txn-replay", &plan, None, 206),
            Err(StorageError::InvalidTakeoverPlan)
        ));
        let mount_count = storage
            .connection
            .query_row(
                "SELECT COUNT(*) FROM mounts WHERE id = ?1",
                [&plan.targets[0].mount_id],
                |row| row.get::<_, i64>(0),
            )
            .expect("应检查重放后的 Mount");
        assert_eq!(mount_count, 0, "completed 重放只能报错，不能补回领域记录");
    }

    #[test]
    fn batch_mount_plan_round_trips_multiple_members_and_app_targets() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let members = save_test_managed_bundle(&mut storage, "roundtrip");
        let project = register_test_project(&mut storage, &sandbox.path().join("project"));
        let global_target = sandbox
            .path()
            .join("home/.codex/skills")
            .join(&members[0].skill_name);
        let project_target = PathBuf::from(&project.root_path)
            .join(".claude/skills")
            .join(&members[1].skill_name);
        let items = [
            NewBatchMountPlanItem {
                id: "batch-item-global",
                mount_id: "batch-mount-global",
                member_id: &members[0].id,
                app_id: SupportedAppId::Codex,
                scope: MountScope::Global,
                project_id: None,
                target_path: global_target.to_str().expect("测试路径应是 UTF-8"),
                expected_target: &members[0].expected_target,
                member_fingerprint: &members[0].content_fingerprint,
                target_observation: "absent",
                disposition: BatchMountDisposition::Ready,
                selectable: true,
                default_selected: true,
                conflict_reason: None,
                target_health: MountHealth::Missing,
            },
            NewBatchMountPlanItem {
                id: "batch-item-project",
                mount_id: "batch-mount-project",
                member_id: &members[1].id,
                app_id: SupportedAppId::ClaudeCode,
                scope: MountScope::Project,
                project_id: Some(&project.id),
                target_path: project_target.to_str().expect("测试路径应是 UTF-8"),
                expected_target: &members[1].expected_target,
                member_fingerprint: &members[1].content_fingerprint,
                target_observation: "absent",
                disposition: BatchMountDisposition::Ready,
                selectable: true,
                default_selected: false,
                conflict_reason: None,
                target_health: MountHealth::Missing,
            },
        ];
        storage
            .save_batch_mount_plan(NewBatchMountPlan {
                id: "batch-plan-roundtrip",
                bundle_id: &members[0].bundle_id,
                items: &items,
                created_at: 300,
                expires_at: 1_000,
            })
            .expect("应保存 Batch Mount Plan");

        let plan = storage
            .read_batch_mount_plan("batch-plan-roundtrip")
            .expect("应读取 Batch Mount Plan");
        assert_eq!(plan.bundle_id, members[0].bundle_id);
        assert_eq!(plan.bundle_display_name, "Batch roundtrip");
        assert_eq!(plan.status, "pending");
        assert_eq!(plan.items.len(), 2);
        assert_eq!(plan.items[0].disposition, BatchMountDisposition::Ready);
        assert!(plan.items[0].default_selected);
        assert!(!plan.items[0].selected);
        assert_eq!(plan.items[1].app_id, SupportedAppId::ClaudeCode);
        assert_eq!(
            plan.items[1].project_id.as_deref(),
            Some(project.id.as_str())
        );
        assert_eq!(
            plan.items[1].project_display_name.as_deref(),
            Some("示例项目")
        );
        assert_eq!(plan.items[1].target_health, MountHealth::Missing);
    }

    #[test]
    fn sqlite_constraints_reject_scope_target_and_project_tampering() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let first = save_test_managed_member(&mut storage, "constraint-one");
        let second = save_test_managed_member(&mut storage, "constraint-two");
        let project = register_test_project(&mut storage, &sandbox.path().join("project"));
        let first_target = sandbox
            .path()
            .join("home/.codex/skills")
            .join(&first.skill_name);
        storage
            .connection
            .execute(
                "INSERT INTO mounts (
                    id, member_id, app_id, scope, project_id, target_path,
                    expected_target, health, created_at, updated_at
                 ) VALUES (?1, ?2, 'codex', 'global', NULL, ?3, ?4, 'healthy', 1, 1)",
                params![
                    "mount-global",
                    first.id,
                    first_target.to_str().expect("测试路径应是 UTF-8"),
                    first.expected_target,
                ],
            )
            .expect("应建立约束测试基线 Mount");

        let project_target = PathBuf::from(&project.root_path)
            .join(".codex/skills")
            .join(&first.skill_name);
        let scope_conflict = storage.connection.execute(
            "INSERT INTO mounts (
                id, member_id, app_id, scope, project_id, target_path,
                expected_target, health, created_at, updated_at
             ) VALUES (?1, ?2, 'codex', 'project', ?3, ?4, ?5, 'healthy', 1, 1)",
            params![
                "mount-project",
                first.id,
                project.id,
                project_target.to_str().expect("测试路径应是 UTF-8"),
                first.expected_target,
            ],
        );
        assert!(
            scope_conflict.is_err(),
            "SQLite trigger 必须拒绝 scope 并存"
        );

        let duplicate_target = storage.connection.execute(
            "INSERT INTO mounts (
                id, member_id, app_id, scope, project_id, target_path,
                expected_target, health, created_at, updated_at
             ) VALUES (?1, ?2, 'claude_code', 'global', NULL, ?3, ?4, 'healthy', 1, 1)",
            params![
                "mount-duplicate-target",
                second.id,
                first_target.to_str().expect("测试路径应是 UTF-8"),
                second.expected_target,
            ],
        );
        assert!(duplicate_target.is_err(), "Mount target 必须全局唯一");

        let missing_project = storage.connection.execute(
            "INSERT INTO mounts (
                id, member_id, app_id, scope, project_id, target_path,
                expected_target, health, created_at, updated_at
             ) VALUES (?1, ?2, 'codex', 'project', 'missing', ?3, ?4, 'healthy', 1, 1)",
            params![
                "mount-missing-project",
                second.id,
                sandbox
                    .path()
                    .join("missing/.codex/skills")
                    .join(&second.skill_name)
                    .to_str()
                    .expect("测试路径应是 UTF-8"),
                second.expected_target,
            ],
        );
        assert!(
            missing_project.is_err(),
            "project Mount 必须绑定已登记 Project"
        );

        let invalid_plan_binding = storage.connection.execute(
            "INSERT INTO mount_plans (
                id, operation, mount_id, member_id, app_id, scope, project_id,
                target_path, expected_target, member_fingerprint, target_observation,
                created_at, expires_at, status
             ) VALUES ('bad-plan', 'create', 'bad-mount', ?1, 'codex', 'project', NULL,
                       ?2, ?3, ?4, 'absent', 1, 2, 'pending')",
            params![
                second.id,
                sandbox
                    .path()
                    .join("bad/.codex/skills")
                    .join(&second.skill_name)
                    .to_str()
                    .expect("测试路径应是 UTF-8"),
                second.expected_target,
                second.content_fingerprint,
            ],
        );
        assert!(
            invalid_plan_binding.is_err(),
            "Mount Plan 的 scope 与 Project 绑定必须由 CHECK 保护"
        );
    }

    #[test]
    fn project_registration_is_idempotent_but_rejects_identity_conflicts() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let project_root = sandbox.path().join("project");
        let project = register_test_project(&mut storage, &project_root);

        let same = storage
            .register_project(NewProject {
                id: "another-id",
                display_name: "不会覆盖原名称",
                root_path: project_root.to_str().expect("测试路径应是 UTF-8"),
                root_device: 11,
                root_inode: 22,
                created_at: 301,
            })
            .expect("同一个 canonical Project 应复用原记录");
        assert_eq!(same, project);
        assert_eq!(storage.read_projects().expect("应读取 Project").len(), 1);

        let other_root = sandbox.path().join("other");
        let conflict = storage
            .register_project(NewProject {
                id: "project-one",
                display_name: "冲突项目",
                root_path: other_root.to_str().expect("测试路径应是 UTF-8"),
                root_device: 33,
                root_inode: 44,
                created_at: 302,
            })
            .expect_err("同一 ID 不能改指另一目录");
        assert!(matches!(conflict, StorageError::ProjectIdentityConflict));
    }

    #[test]
    fn mount_create_finalization_is_atomic_idempotent_and_visible_in_inventory() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let member = save_test_managed_member(&mut storage, "alpha");
        let project = register_test_project(&mut storage, &sandbox.path().join("project"));
        let target = PathBuf::from(&project.root_path)
            .join(".codex/skills")
            .join(&member.skill_name);
        save_test_mount_plan(
            &mut storage,
            &member,
            MountOperation::Create,
            "mount-one",
            "mount-plan-create",
            MountScope::Project,
            Some(&project.id),
            &target,
        );

        let plan =
            finalize_test_mount_create(&mut storage, "mount-plan-create", "mount-txn-create");
        storage
            .finalize_mount_create("mount-txn-create", &plan, 404)
            .expect("重复 finalize 应保持幂等");
        let mount = storage.read_mount("mount-one").expect("应读取 Mount");
        assert_eq!(mount.member_id, member.id);
        assert_eq!(mount.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(mount.health, MountHealth::Healthy);

        storage
            .save_initial_scan(500, &[], &[], &[])
            .expect("应保存初始清单");
        let Some(UiOutcome::Inventory {
            projects, mounts, ..
        }) = storage.read_initial_scan().expect("应读取清单")
        else {
            panic!("完成扫描后应返回 Inventory");
        };
        assert_eq!(projects.len(), 1);
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].id, "mount-one");
    }

    #[test]
    fn mount_scope_is_mutually_exclusive_per_member_and_app() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let member = save_test_managed_member(&mut storage, "scope");
        let global_target = sandbox
            .path()
            .join("home/.codex/skills")
            .join(&member.skill_name);
        save_test_mount_plan(
            &mut storage,
            &member,
            MountOperation::Create,
            "mount-global",
            "mount-plan-global",
            MountScope::Global,
            None,
            &global_target,
        );
        finalize_test_mount_create(&mut storage, "mount-plan-global", "mount-txn-global");
        storage
            .forget_terminal_mount_transaction("mount-txn-global")
            .expect("应清理已完成事务");

        let project = register_test_project(&mut storage, &sandbox.path().join("project"));
        let project_target = PathBuf::from(&project.root_path)
            .join(".codex/skills")
            .join(&member.skill_name);
        let error = storage
            .save_mount_plan(NewMountPlan {
                id: "mount-plan-project",
                operation: MountOperation::Create,
                purpose: MountPlanPurpose::Create,
                mount_id: "mount-project",
                member_id: &member.id,
                app_id: SupportedAppId::Codex,
                scope: MountScope::Project,
                project_id: Some(&project.id),
                target_path: project_target.to_str().expect("测试路径应是 UTF-8"),
                expected_target: &member.expected_target,
                member_fingerprint: &member.content_fingerprint,
                target_observation: "missing",
                created_at: 500,
                expires_at: 1_000,
            })
            .expect_err("同一 App 已有 global Mount 时必须拒绝 project Mount");
        assert!(matches!(error, StorageError::InvalidMountPlan));
    }

    #[test]
    fn mount_remove_finalization_only_removes_the_selected_mount() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let member = save_test_managed_member(&mut storage, "remove");
        let target = sandbox
            .path()
            .join("home/.codex/skills")
            .join(&member.skill_name);
        save_test_mount_plan(
            &mut storage,
            &member,
            MountOperation::Create,
            "mount-remove",
            "mount-plan-seed",
            MountScope::Global,
            None,
            &target,
        );
        finalize_test_mount_create(&mut storage, "mount-plan-seed", "mount-txn-seed");
        storage
            .forget_terminal_mount_transaction("mount-txn-seed")
            .expect("应清理创建事务");
        save_test_mount_plan(
            &mut storage,
            &member,
            MountOperation::Remove,
            "mount-remove",
            "mount-plan-remove",
            MountScope::Global,
            None,
            &target,
        );
        let plan = storage
            .begin_mount_transaction(
                "mount-plan-remove",
                "mount-txn-remove",
                "journals/mount-txn-remove.json",
                600,
            )
            .expect("应开始移除事务");
        storage
            .update_mount_transaction_phase("mount-txn-remove", "journal_ready", 601)
            .expect("应记录 Journal");
        storage
            .update_mount_transaction_phase("mount-txn-remove", "target_applied", 602)
            .expect("应记录目标已移除");
        storage
            .finalize_mount_remove("mount-txn-remove", &plan, 603)
            .expect("应提交移除状态");
        storage
            .finalize_mount_remove("mount-txn-remove", &plan, 604)
            .expect("重复 finalize 应保持幂等");
        assert!(matches!(
            storage.read_mount("mount-remove"),
            Err(StorageError::MountNotFound(_))
        ));
        assert_eq!(storage.read_managed_member(&member.id).unwrap(), member);
    }

    #[test]
    fn mount_transaction_rejects_invalid_phases_and_exposes_blocked_recovery() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let member = save_test_managed_member(&mut storage, "recovery");
        let target = sandbox
            .path()
            .join("home/.codex/skills")
            .join(&member.skill_name);
        save_test_mount_plan(
            &mut storage,
            &member,
            MountOperation::Create,
            "mount-recovery",
            "mount-plan-recovery",
            MountScope::Global,
            None,
            &target,
        );
        storage
            .begin_mount_transaction(
                "mount-plan-recovery",
                "mount-txn-recovery",
                "journals/mount-txn-recovery.json",
                400,
            )
            .expect("应开始 Mount 事务");
        assert!(matches!(
            storage.update_mount_transaction_phase("mount-txn-recovery", "target_applied", 401),
            Err(StorageError::MountStateConflict(_))
        ));
        storage
            .block_mount_transaction("mount-txn-recovery", "目标归属无法判断", 402)
            .expect("应阻塞 Mount 事务");
        assert!(
            bundle_or_source_write_is_blocked(&storage.connection, Some(&member.bundle_id), None,)
                .expect("应检查相关 Bundle"),
            "blocked Mount 必须阻止同 Bundle 的安装与补装"
        );
        let unrelated = save_test_managed_member(&mut storage, "unrelated-recovery");
        assert!(
            !bundle_or_source_write_is_blocked(
                &storage.connection,
                Some(&unrelated.bundle_id),
                None,
            )
            .expect("应检查无关 Bundle"),
            "人工恢复不能阻止其他 Bundle"
        );
        let recoverable = storage
            .recoverable_mount_transactions()
            .expect("应读取 Mount 恢复事务");
        assert_eq!(recoverable.len(), 1);
        assert_eq!(recoverable[0].status, "blocked");

        storage
            .save_initial_scan(500, &[], &[], &[])
            .expect("应保存初始清单");
        let Some(UiOutcome::Inventory {
            recovery_issues, ..
        }) = storage.read_initial_scan().expect("应读取恢复问题")
        else {
            panic!("完成扫描后应返回 Inventory");
        };
        assert_eq!(recovery_issues.len(), 1);
        assert_eq!(recovery_issues[0].id, "mount-txn-recovery");
        assert_eq!(recovery_issues[0].bundle_display_name, member.skill_name);
    }

    #[test]
    fn mount_plan_confirmation_rejects_sqlite_tampering_without_consuming_it() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let member = save_test_managed_member(&mut storage, "tamper");
        let target = sandbox
            .path()
            .join("home/.codex/skills")
            .join(&member.skill_name);
        save_test_mount_plan(
            &mut storage,
            &member,
            MountOperation::Create,
            "mount-tamper",
            "mount-plan-tamper",
            MountScope::Global,
            None,
            &target,
        );
        storage
            .connection
            .execute(
                "UPDATE mount_plans SET member_fingerprint = 'sha256:tampered' WHERE id = 'mount-plan-tamper'",
                [],
            )
            .expect("应模拟 SQLite 外部篡改");

        assert!(matches!(
            storage.begin_mount_transaction(
                "mount-plan-tamper",
                "mount-txn-tamper",
                "journals/mount-txn-tamper.json",
                400,
            ),
            Err(StorageError::InvalidMountPlan)
        ));
        let status = storage
            .connection
            .query_row(
                "SELECT status FROM mount_plans WHERE id = 'mount-plan-tamper'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("失败确认后 Plan 应仍存在");
        assert_eq!(status, "pending");
    }

    #[test]
    fn install_and_mount_transactions_share_one_active_writer_boundary() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        let member = save_test_managed_member(&mut storage, "writer");
        let target = sandbox
            .path()
            .join("home/.codex/skills")
            .join(&member.skill_name);
        save_test_mount_plan(
            &mut storage,
            &member,
            MountOperation::Create,
            "mount-writer",
            "mount-plan-writer",
            MountScope::Global,
            None,
            &target,
        );
        storage
            .begin_mount_transaction(
                "mount-plan-writer",
                "mount-txn-writer",
                "journals/mount-txn-writer.json",
                400,
            )
            .expect("应开始 Mount 事务");
        save_test_plan(
            &mut storage,
            "install-plan-concurrent",
            "bundle-concurrent",
            "member-concurrent",
        );
        let error = storage
            .begin_install_transaction(
                "install-plan-concurrent",
                "install-txn-concurrent",
                "journals/install-txn-concurrent.json",
                401,
            )
            .expect_err("Mount 执行中不能开始安装事务");
        assert!(matches!(error, StorageError::ActiveLifecycleTransaction));
        assert_eq!(
            storage
                .read_install_plan("install-plan-concurrent")
                .expect("失败事务不能消费安装 Plan")
                .status,
            "pending"
        );
    }

    #[test]
    fn open_rejects_a_data_root_symlink_or_file() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let actual = sandbox.path().join("actual");
        fs::create_dir(&actual).expect("应创建符号链接目标目录");
        let data_root = sandbox.path().join("data");
        symlink(&actual, &data_root).expect("应创建数据目录符号链接");

        let error = Storage::open(&data_root, &data_root.join("skillyard.sqlite3"))
            .err()
            .expect("数据根目录是符号链接时必须拒绝");

        assert!(matches!(error, StorageError::UnsafeDataRoot(path) if path == data_root));
        assert!(
            !actual.join("skillyard.sqlite3").exists(),
            "拒绝前不能在符号链接目标创建 SQLite"
        );

        fs::remove_file(&data_root).expect("应移除测试符号链接");
        fs::write(&data_root, []).expect("应创建同名普通文件");
        let file_error = Storage::open(&data_root, &data_root.join("skillyard.sqlite3"))
            .err()
            .expect("数据根目录是普通文件时必须拒绝");
        assert!(matches!(
            file_error,
            StorageError::UnsafeDataRoot(path) if path == data_root
        ));
    }

    #[test]
    fn open_rejects_a_database_symlink_or_directory() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        fs::create_dir(&data_root).expect("应创建数据目录");
        let target = data_root.join("actual.sqlite3");
        fs::write(&target, []).expect("应创建符号链接目标文件");
        let database = data_root.join("skillyard.sqlite3");
        symlink(&target, &database).expect("应创建数据库符号链接");

        let symlink_error = Storage::open(&data_root, &database)
            .err()
            .expect("数据库是符号链接时必须拒绝");
        assert!(matches!(
            symlink_error,
            StorageError::UnsafeDatabase(path) if path == database
        ));

        fs::remove_file(&database).expect("应移除测试符号链接");
        fs::create_dir(&database).expect("应创建同名目录");
        let directory_error = Storage::open(&data_root, &database)
            .err()
            .expect("数据库路径是目录时必须拒绝");
        assert!(matches!(
            directory_error,
            StorageError::UnsafeDatabase(path) if path == database
        ));
    }

    #[test]
    fn open_rejects_a_hard_linked_database() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        fs::create_dir(&data_root).expect("应创建数据目录");
        let outside = sandbox.path().join("shared.sqlite3");
        fs::write(&outside, []).expect("应创建外部文件");
        let database = data_root.join("skillyard.sqlite3");
        fs::hard_link(&outside, &database).expect("应创建数据库硬链接");

        let error = Storage::open(&data_root, &database)
            .err()
            .expect("数据库硬链接必须被拒绝");

        assert!(matches!(error, StorageError::UnsafeDatabase(path) if path == database));
        assert!(fs::read(&outside).expect("外部文件应保持可读").is_empty());
    }

    #[test]
    fn open_rejects_a_database_outside_the_data_root() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        let outside_database = sandbox.path().join("outside.sqlite3");

        let error = Storage::open(&data_root, &outside_database)
            .err()
            .expect("SQLite 必须属于 Central Store 根目录");

        assert!(matches!(
            error,
            StorageError::UnsafeDatabase(path) if path == outside_database
        ));
        assert!(!outside_database.exists(), "拒绝前不能创建外部 SQLite");
    }

    #[test]
    fn aborted_and_completed_transactions_remain_recoverable_until_cleanup() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan-abort", "bundle-abort", "member-abort");
        let aborted_plan = storage
            .begin_install_transaction("plan-abort", "txn-abort", "journals/abort.json", 200)
            .expect("应开始待中止事务");
        storage
            .abort_lifecycle_transaction("txn-abort", None, 201)
            .expect("应持久化中止状态");

        save_test_plan(
            &mut storage,
            "plan-complete",
            "bundle-complete",
            "member-complete",
        );
        let completed_plan = storage
            .begin_install_transaction(
                "plan-complete",
                "txn-complete",
                "journals/complete.json",
                202,
            )
            .expect("应开始待完成事务");
        advance_to_candidate_ready(&mut storage, "txn-complete");
        storage
            .finalize_install(
                "txn-complete",
                &completed_plan,
                "bundles/bundle-complete",
                "contents/txn-complete",
                "members/member-complete",
                203,
            )
            .expect("应提交受管状态");

        let statuses = storage
            .recoverable_lifecycle_transactions()
            .expect("应读取可恢复事务")
            .into_iter()
            .map(|transaction| (transaction.id, transaction.status))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            statuses.get("txn-abort").map(String::as_str),
            Some("aborted")
        );
        assert_eq!(
            statuses.get("txn-complete").map(String::as_str),
            Some("completed")
        );
        assert_eq!(aborted_plan.id, "plan-abort");
    }

    #[test]
    fn lifecycle_state_changes_reject_unknown_or_invalid_transactions() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());

        assert!(matches!(
            storage.update_lifecycle_phase("missing", "journal_ready", 200),
            Err(StorageError::LifecycleStateConflict(id)) if id == "missing"
        ));
        assert!(matches!(
            storage.abort_lifecycle_transaction("missing", None, 200),
            Err(StorageError::LifecycleStateConflict(id)) if id == "missing"
        ));
        assert!(matches!(
            storage.block_lifecycle_transaction("missing", "测试", 200),
            Err(StorageError::LifecycleStateConflict(id)) if id == "missing"
        ));

        save_test_plan(&mut storage, "plan", "bundle", "member");
        storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 201)
            .expect("应开始事务");
        assert!(matches!(
            storage.update_lifecycle_phase("txn", "candidate_ready", 202),
            Err(StorageError::LifecycleStateConflict(id)) if id == "txn"
        ));
        storage
            .update_lifecycle_phase("txn", "journal_ready", 203)
            .expect("应允许进入下一阶段");
        assert!(matches!(
            storage.update_lifecycle_phase("txn", "activated", 204),
            Err(StorageError::LifecycleStateConflict(id)) if id == "txn"
        ));
        storage
            .update_lifecycle_phase("txn", "candidate_ready", 205)
            .expect("应允许进入候选阶段");
        storage
            .update_lifecycle_phase("txn", "activated", 206)
            .expect("应允许进入生效阶段");
        assert!(matches!(
            storage.abort_lifecycle_transaction("txn", None, 207),
            Err(StorageError::LifecycleStateConflict(id)) if id == "txn"
        ));
        assert!(matches!(
            storage.update_lifecycle_phase("txn", "unknown", 208),
            Err(StorageError::InvalidLifecyclePhase(phase)) if phase == "unknown"
        ));
    }

    #[test]
    fn a_completed_transaction_can_be_blocked_when_recovery_finds_damage() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        let plan = storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        advance_to_candidate_ready(&mut storage, "txn");
        storage
            .finalize_install(
                "txn",
                &plan,
                "bundles/bundle",
                "contents/txn",
                "members/member",
                201,
            )
            .expect("应完成事务");

        storage
            .block_lifecycle_transaction("txn", "current 已损坏", 202)
            .expect("完成后的清理恢复仍应允许阻塞异常状态");

        let transaction = storage
            .recoverable_lifecycle_transactions()
            .expect("应读取阻塞事务")
            .into_iter()
            .find(|transaction| transaction.id == "txn")
            .expect("阻塞事务应保留");
        assert_eq!(transaction.status, "blocked");
    }

    #[test]
    fn an_aborted_transaction_can_be_blocked_when_cleanup_finds_damage() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        storage
            .abort_lifecycle_transaction("txn", None, 201)
            .expect("应中止事务");

        storage
            .block_lifecycle_transaction("txn", "清理时发现外部内容", 202)
            .expect("中止后的清理异常必须持久化为阻塞");

        let transaction = storage
            .recoverable_lifecycle_transactions()
            .expect("应读取阻塞事务")
            .into_iter()
            .find(|transaction| transaction.id == "txn")
            .expect("阻塞事务应保留");
        assert_eq!(transaction.status, "blocked");
    }

    #[test]
    fn blocked_transactions_are_visible_in_startup_and_refresh_inventory() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        storage
            .block_lifecycle_transaction("txn", "current 状态无法判断", 201)
            .expect("应保存阻塞恢复状态");
        storage
            .save_initial_scan(202, &[], &[], &[])
            .expect("应保存初始清单");

        let Some(UiOutcome::Inventory {
            recovery_issues, ..
        }) = storage.read_initial_scan().expect("应读取启动清单")
        else {
            panic!("完成过扫描后应返回 Inventory");
        };
        assert_eq!(recovery_issues.len(), 1);
        assert_eq!(recovery_issues[0].id, "txn");
        assert_eq!(recovery_issues[0].bundle_display_name, "bundle");
        assert_eq!(recovery_issues[0].message, "current 状态无法判断");

        let refreshed = storage
            .save_local_refresh(203, &[], &[], &[], &[], &[])
            .expect("刷新不应隐藏人工恢复状态");
        assert_eq!(refreshed.recovery_issues, recovery_issues);
    }

    #[test]
    fn blocked_transaction_remains_visible_when_its_plan_row_is_missing() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle-fallback", "member");
        storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        storage
            .block_lifecycle_transaction("txn", "Plan 已损坏", 201)
            .expect("应保存阻塞状态");
        storage
            .connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF; DELETE FROM install_plans; PRAGMA foreign_keys = ON;",
            )
            .expect("应模拟损坏数据库中缺失 Plan");

        let issues = storage.read_recovery_issues().expect("阻塞事务必须仍可见");

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].bundle_display_name, "bundle-fallback");
        assert_eq!(issues[0].message, "Plan 已损坏");
    }

    #[test]
    fn unrelated_unique_constraint_is_not_reported_as_an_active_transaction() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan-one", "bundle-one", "member-one");
        storage
            .begin_install_transaction("plan-one", "same-id", "journals/one.json", 200)
            .expect("应开始首个事务");
        storage
            .abort_lifecycle_transaction("same-id", None, 201)
            .expect("应中止首个事务");
        save_test_plan(&mut storage, "plan-two", "bundle-two", "member-two");

        let error = storage
            .begin_install_transaction("plan-two", "same-id", "journals/two.json", 202)
            .expect_err("重复事务 ID 应被 SQLite 拒绝");

        assert!(matches!(error, StorageError::SaveLifecycleTransaction(_)));
        assert_eq!(
            storage
                .read_install_plan("plan-two")
                .expect("失败事务不应消耗 Plan")
                .status,
            "pending"
        );
    }

    #[test]
    fn only_the_single_writer_index_is_reported_as_an_active_transaction() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan-one", "bundle-one", "member-one");
        storage
            .begin_install_transaction("plan-one", "txn-one", "journals/one.json", 200)
            .expect("应开始首个事务");
        save_test_plan(&mut storage, "plan-two", "bundle-two", "member-two");

        let error = storage
            .begin_install_transaction("plan-two", "txn-two", "journals/two.json", 201)
            .expect_err("单写者索引应拒绝第二个活跃事务");

        assert!(matches!(error, StorageError::ActiveLifecycleTransaction));
        assert_eq!(
            storage
                .read_install_plan("plan-two")
                .expect("失败事务不应消耗 Plan")
                .status,
            "pending"
        );
    }

    #[test]
    fn finalize_rejects_a_missing_transaction_and_rolls_back_managed_rows() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        let plan = storage.read_install_plan("plan").expect("应读取安装 Plan");

        let error = storage
            .finalize_install(
                "missing",
                &plan,
                "bundles/bundle",
                "contents/missing",
                "members/member",
                200,
            )
            .expect_err("缺少生命周期事务时不能提交受管状态");

        assert!(matches!(
            error,
            StorageError::LifecycleStateConflict(id) if id == "missing"
        ));
        assert!(
            storage
                .managed_bundle_notice_rows()
                .expect("应读取受管 Bundle")
                .is_empty(),
            "失败的 finalize 必须回滚已插入的 Bundle"
        );
    }

    #[test]
    fn finalize_requires_the_matching_plan_and_a_publishable_phase() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan-one", "bundle-one", "member-one");
        let plan_one = storage
            .begin_install_transaction("plan-one", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        save_test_plan(&mut storage, "plan-two", "bundle-two", "member-two");
        let plan_two = storage
            .read_install_plan("plan-two")
            .expect("应读取另一份 Plan");

        let early_error = storage
            .finalize_install(
                "txn",
                &plan_one,
                "bundles/bundle-one",
                "contents/txn",
                "members/member-one",
                201,
            )
            .expect_err("尚未发布候选内容时不能 finalize");
        assert!(matches!(
            early_error,
            StorageError::LifecycleStateConflict(id) if id == "txn"
        ));

        advance_to_candidate_ready(&mut storage, "txn");
        let wrong_plan_error = storage
            .finalize_install(
                "txn",
                &plan_two,
                "bundles/bundle-two",
                "contents/txn",
                "members/member-two",
                203,
            )
            .expect_err("事务不能提交另一份 Plan");
        assert!(matches!(
            wrong_plan_error,
            StorageError::LifecycleStateConflict(id) if id == "txn"
        ));
        assert!(
            storage
                .managed_bundle_notice_rows()
                .expect("应读取受管 Bundle")
                .is_empty(),
            "身份或阶段不匹配时必须回滚受管记录"
        );
    }

    #[test]
    fn finalize_rejects_paths_outside_the_fixed_bundle_layout() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        let plan = storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        advance_to_candidate_ready(&mut storage, "txn");

        for (managed_directory, current_target, stable_relative_path) in [
            ("../bundle", "contents/txn", "members/member"),
            ("bundles/bundle", "/tmp/txn", "members/member"),
            ("bundles/bundle", "contents/txn", "members/../member"),
        ] {
            assert!(matches!(
                storage.finalize_install(
                    "txn",
                    &plan,
                    managed_directory,
                    current_target,
                    stable_relative_path,
                    203,
                ),
                Err(StorageError::UnsafeManagedPath(_))
            ));
        }
        assert!(
            storage
                .managed_bundle_notice_rows()
                .expect("应读取受管 Bundle")
                .is_empty(),
            "非法路径不能留下受管记录"
        );
    }

    #[test]
    fn managed_inventory_rejects_paths_tampered_in_sqlite() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        let plan = storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");
        advance_to_candidate_ready(&mut storage, "txn");
        storage
            .finalize_install(
                "txn",
                &plan,
                "bundles/bundle",
                "contents/txn",
                "members/member",
                203,
            )
            .expect("应完成事务");
        storage
            .save_initial_scan(204, &[], &[], &[])
            .expect("应建立可读取的清单状态");
        storage
            .connection
            .execute(
                "UPDATE bundles SET managed_directory = '../../outside' WHERE id = 'bundle'",
                [],
            )
            .expect("应模拟 SQLite 被外部篡改");

        assert!(matches!(
            storage.read_initial_scan(),
            Err(StorageError::UnsafeManagedPath(path)) if path == "../../outside"
        ));
        assert!(matches!(
            storage.managed_bundle_notice_rows(),
            Err(StorageError::UnsafeManagedPath(path)) if path == "../../outside"
        ));
    }

    #[test]
    fn cleanup_rejects_an_existing_non_terminal_transaction_but_is_idempotent_when_missing() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let mut storage = open_test_storage(sandbox.path());
        save_test_plan(&mut storage, "plan", "bundle", "member");
        storage
            .begin_install_transaction("plan", "txn", "journals/txn.json", 200)
            .expect("应开始事务");

        assert!(matches!(
            storage.forget_terminal_transaction("txn"),
            Err(StorageError::LifecycleStateConflict(id)) if id == "txn"
        ));
        storage
            .forget_terminal_transaction("missing")
            .expect("已经清理的事务应保持幂等");
    }

    #[test]
    fn concurrent_open_applies_each_migration_only_once() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        let database = data_root.join("skillyard.sqlite3");
        let barrier = Arc::new(Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let data_root = data_root.clone();
                let database = database.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    Storage::open(&data_root, &database).map(|_| ())
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle
                .join()
                .expect("并发 migration 线程不应 panic")
                .expect("并发打开应成功");
        }
    }
}
