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

use crate::domain::{
    BatchMountDisposition, InventoryItem, InventoryLocationKind, InventoryObservation,
    LocalRefreshSummary, ManagementEvidence, ManagementEvidenceKind, ManagementKind, MountHealth,
    MountOperation, MountPlanPurpose, MountScope, MountSummary, ProjectSummary, RecoveryIssue,
    ScanIssue, ScanIssueCode, ScanRootIdentity, ScanRootKey, SkillMetadataStatus,
    SourceCatalogMemberSummary, SourceCatalogStatus, SourceRefChangePlan, SourceSummary,
    SupportedAppId, SupportedAppSummary, TakeoverPlan, UiOutcome,
};

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
    #[error("无法读取本机清单状态：{0}")]
    ReadInventory(#[source] rusqlite::Error),
    #[error("无法读取 Source 状态：{0}")]
    ReadSources(#[source] rusqlite::Error),
    #[error("SQLite 中包含未知 Source Catalog 状态：{0}")]
    UnknownSourceCatalogStatus(String),
    #[error("Source Catalog 成员 metadata 无法解析：{0}")]
    InvalidSourceCatalogMetadata(#[source] serde_json::Error),
    #[error("无法保存 GitHub Source：{0}")]
    SaveSource(#[source] rusqlite::Error),
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
    #[error("SQLite 中包含未知扫描问题类型：{0}")]
    UnknownScanIssueCode(String),
    #[error("SQLite 中包含非法刷新统计值：{0}")]
    InvalidRefreshCount(i64),
    #[error("安装 Plan 未签发或已经不存在")]
    InstallPlanNotFound,
    #[error("安装 Plan 已经使用，不能重复确认")]
    InstallPlanConsumed,
    #[error("安装 Plan 已过期，请重新生成")]
    InstallPlanExpired,
    #[error("安装 Plan 没有可保存的候选成员")]
    EmptyInstallPlanCandidates,
    #[error("安装 Plan 不符合唯一的 folder/GitHub 安装协议")]
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
    #[error("无法保存生命周期事务：{0}")]
    SaveLifecycleTransaction(#[source] rusqlite::Error),
    #[error("无法读取生命周期事务：{0}")]
    ReadLifecycleTransaction(#[source] rusqlite::Error),
    #[error("无法读取人工恢复状态：{0}")]
    ReadRecoveryIssues(#[source] rusqlite::Error),
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
    /// GitHub 快照路径直接指向 repository root，并始终相对 Central Store。
    pub snapshot_relative_path: Option<String>,
    pub source_id: Option<String>,
    pub source_tracked_ref: Option<String>,
    pub source_catalog_generation: Option<i64>,
    pub source_commit_sha: Option<String>,
    pub expected_current_target: Option<String>,
    pub expected_adopted_commit_sha: Option<String>,
    pub bundle_id: String,
    pub bundle_display_name: String,
    pub expires_at: i64,
    pub status: String,
    pub candidates: Vec<StoredInstallCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredInstallCandidate {
    pub candidate_id: String,
    pub source_relative_path: String,
    pub skill_name: Option<String>,
    pub skill_description: Option<String>,
    pub content_fingerprint: Option<String>,
    pub selectable: bool,
    /// 已安装成员属于最终完整集合，但不能再次作为用户选择提交。
    pub preserve_existing: bool,
    pub validation_errors: Vec<String>,
    pub warnings: Vec<String>,
    pub default_selected: bool,
    pub selected: bool,
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
    pub source_commit_sha: Option<&'a str>,
    pub expected_current_target: Option<&'a str>,
    pub expected_adopted_commit_sha: Option<&'a str>,
    pub bundle_id: &'a str,
    pub bundle_display_name: &'a str,
    pub warnings: &'a [String],
    pub candidates: &'a [NewInstallCandidate<'a>],
    pub created_at: i64,
    pub expires_at: i64,
}

pub struct NewInstallCandidate<'a> {
    pub candidate_id: &'a str,
    pub source_relative_path: &'a str,
    pub skill_name: Option<&'a str>,
    pub skill_description: Option<&'a str>,
    pub content_fingerprint: Option<&'a str>,
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
    source_commit_sha: Option<String>,
    expected_current_target: Option<String>,
    expected_adopted_commit_sha: Option<String>,
    bundle_id: String,
    bundle_display_name: String,
    expires_at: i64,
    status: String,
}

pub struct NewGitHubSource<'a> {
    pub id: &'a str,
    pub canonical_identity: &'a str,
    pub owner: &'a str,
    pub repository: &'a str,
    pub display_name: &'a str,
    pub repository_url: &'a str,
    pub tracked_ref: &'a str,
    pub resolved_commit_sha: &'a str,
    pub member_path_hint: Option<&'a str>,
}

pub enum SaveGitHubSourceResult {
    Saved { source_id: String },
    RefChangeRequired { plan: SourceRefChangePlan },
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

/// Lifecycle 组 Plan 时只接收同一个 SQLite 快照中的 Fresh Catalog 与本地关联状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredGithubInstallSource {
    pub id: String,
    pub owner: String,
    pub repository: String,
    pub display_name: String,
    pub tracked_ref: String,
    pub catalog_generation: i64,
    pub catalog_commit_sha: String,
    pub catalog_members: Vec<StoredGithubInstallCatalogMember>,
    pub bundle: Option<StoredGithubInstallBundle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredGithubInstallCatalogMember {
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
pub struct StoredGithubInstallBundle {
    pub id: String,
    pub display_name: String,
    pub current_target: String,
    pub adopted_commit_sha: Option<String>,
    pub members: Vec<StoredGithubInstallBundleMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredGithubInstallBundleMember {
    pub id: String,
    pub skill_name: String,
    pub description: String,
    pub stable_relative_path: String,
    pub content_fingerprint: String,
    pub source_relative_path: String,
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
        now: i64,
    ) -> Result<(), StorageError> {
        let (fingerprint, managed_directory, current_target, stable_relative_path) =
            validate_takeover_domain_contract(&self.data_root, plan)?;
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
            || state.member_id != plan.member_id
            || state.reserved_paths != expected_reserved_paths
            || !is_normalized_relative_path(&state.journal_path)
        {
            return Err(StorageError::TakeoverStateConflict(
                transaction_id.to_owned(),
            ));
        }
        if state.phase == "origins_applied" && state.status == "in_progress" {
            // 首次提交必须认领一组全新的领域 ID；任一冲突都会让整个 SQLite 事务回滚。
            transaction
                .execute(
                    "INSERT INTO bundles (id, display_name, managed_directory, current_target, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![plan.bundle_id, plan.bundle_display_name, managed_directory, current_target, now],
                )
                .map_err(StorageError::SaveTakeoverTransaction)?;
            transaction
                .execute(
                    "INSERT INTO skill_members
                        (id, bundle_id, skill_name, description, stable_relative_path, content_fingerprint, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![plan.member_id, plan.bundle_id, plan.skill_name, plan.skill_description, stable_relative_path, fingerprint, now],
                )
                .map_err(StorageError::SaveTakeoverTransaction)?;
            transaction
                .execute(
                    "INSERT INTO member_selections (bundle_id, member_id, selected_at) VALUES (?1, ?2, ?3)",
                    params![plan.bundle_id, plan.member_id, now],
                )
                .map_err(StorageError::SaveTakeoverTransaction)?;
            for target in &plan.targets {
                transaction
                    .execute(
                        "INSERT INTO mounts
                        (id, member_id, app_id, scope, project_id, target_path, expected_target, health, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'healthy', ?8, ?8)",
                        params![target.mount_id, plan.member_id, target.app_id.as_str(), target.scope.as_str(), target.project_id, target.target_path, target.expected_target, now],
                    )
                    .map_err(StorageError::SaveTakeoverTransaction)?;
            }
            ensure_takeover_domain_matches(&transaction, &self.data_root, plan, &fingerprint)?;
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
            ensure_takeover_domain_matches(&transaction, &self.data_root, plan, &fingerprint)?;
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
            projects,
            mounts,
        }))
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
                "SELECT source.id, source.canonical_identity, source.display_name,
                        source.repository_url, source.tracked_ref, source.member_path_hint,
                        source.catalog_status, source.catalog_commit_sha,
                        source.catalog_fetched_at, source.last_reload_at,
                        source.last_reload_error, source.catalog_generation,
                        link.bundle_id, link.adopted_commit_sha
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
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
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
                canonical_identity,
                display_name,
                repository_url,
                tracked_ref,
                member_path_hint,
                catalog_status,
                catalog_commit_sha,
                catalog_fetched_at,
                last_reload_at,
                last_reload_error,
                catalog_generation,
                bundle_id,
                adopted_commit_sha,
            ) = row;
            let catalog_status = SourceCatalogStatus::from_str(&catalog_status)
                .ok_or_else(|| StorageError::UnknownSourceCatalogStatus(catalog_status.clone()))?;
            let members = read_source_catalog_members_from(&transaction, &id, catalog_generation)?;
            sources.push(SourceSummary {
                id,
                canonical_identity,
                display_name,
                repository_url,
                tracked_ref,
                member_path_hint,
                catalog_status,
                catalog_commit_sha,
                catalog_fetched_at,
                last_reload_at,
                last_reload_error,
                bundle_id,
                adopted_commit_sha,
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
                "SELECT tracked_ref FROM sources WHERE canonical_identity = ?1",
                [canonical_identity],
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::ReadSources)
    }

    pub fn read_github_sources(&self) -> Result<Vec<StoredGithubSource>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, canonical_identity, owner, repository, display_name, tracked_ref
                 FROM sources ORDER BY sort_order, id",
            )
            .map_err(StorageError::ReadSources)?;
        let rows = statement
            .query_map([], stored_github_source_from_row)
            .map_err(StorageError::ReadSources)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::ReadSources)
    }

    pub fn read_github_source(&self, source_id: &str) -> Result<StoredGithubSource, StorageError> {
        self.connection
            .query_row(
                "SELECT id, canonical_identity, owner, repository, display_name, tracked_ref
                 FROM sources WHERE id = ?1",
                [source_id],
                stored_github_source_from_row,
            )
            .optional()
            .map_err(StorageError::ReadSources)?
            .ok_or(StorageError::SourceNotFound)
    }

    pub fn read_github_install_source(
        &mut self,
        source_id: &str,
    ) -> Result<StoredGithubInstallSource, StorageError> {
        // Source、Catalog、Bundle 与成员关系必须来自同一个读快照。
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(StorageError::ReadSources)?;
        let source = read_github_install_source_from(&transaction, source_id)?;
        transaction.commit().map_err(StorageError::ReadSources)?;
        Ok(source)
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
        let (current_ref, current_generation) = transaction
            .query_row(
                "SELECT tracked_ref, catalog_generation FROM sources WHERE id = ?1",
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
                     catalog_commit_sha = ?3, catalog_fetched_at = ?4,
                     last_reload_at = ?4, last_reload_error = NULL, updated_at = ?4
                 WHERE id = ?1 AND tracked_ref = ?5",
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

    /// Reload 失败只更新结果状态；最近一次成功目录和 commit 始终保留。
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
        let changed = transaction
            .execute(
                "UPDATE sources
                 SET catalog_status = CASE
                         WHEN catalog_generation > 0 THEN 'stale'
                         ELSE 'unloaded'
                     END,
                     last_reload_at = ?3, last_reload_error = ?4, updated_at = ?3
                 WHERE id = ?1 AND tracked_ref = ?2",
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
                 FROM sources WHERE canonical_identity = ?1",
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
            if current_ref == source.tracked_ref {
                let changed = transaction
                    .execute(
                        "UPDATE sources
                         SET owner = ?2, repository = ?3, display_name = ?4,
                             repository_url = ?5, member_path_hint = ?6, updated_at = ?7
                         WHERE id = ?1 AND tracked_ref = ?8",
                        params![
                            source_id,
                            source.owner,
                            source.repository,
                            source.display_name,
                            source.repository_url,
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
                        display_name, repository_url, tracked_ref, member_path_hint,
                        sort_order, created_at, updated_at
                     ) VALUES (?1, 'github', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                    params![
                        source.id,
                        source.canonical_identity,
                        source.owner,
                        source.repository,
                        source.display_name,
                        source.repository_url,
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
                "SELECT source_id, current_ref, candidate_ref, member_path_hint,
                        expires_at, status
                 FROM source_ref_change_plans WHERE id = ?1",
                [plan_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(StorageError::SaveSource)?
            .ok_or(StorageError::SourceRefChangePlanNotFound)?;
        let (source_id, current_ref, candidate_ref, member_path_hint, expires_at, status) = plan;
        if status != "pending" {
            return Err(StorageError::SourceRefChangePlanConsumed);
        }
        if expires_at <= now {
            return Err(StorageError::SourceRefChangePlanExpired);
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
                 WHERE id = ?1 AND tracked_ref = ?5",
                params![source_id, candidate_ref, member_path_hint, now, current_ref,],
            )
            .map_err(StorageError::SaveSource)?;
        if changed != 1 {
            return Err(StorageError::SourceRefChangeStateChanged);
        }
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

    pub fn save_initial_scan(
        &mut self,
        scan_completed_at: i64,
        entries: &[InventoryObservation],
        supported_apps: &[SupportedAppSummary],
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveInitialScan)?;
        replace_inventory_rows(&transaction, entries, supported_apps, &[])
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
    ) -> Result<bool, StorageError> {
        mount_object_is_blocked(&self.connection, member_id, target_path)
    }

    pub(crate) fn github_install_object_is_blocked(
        &self,
        source_id: &str,
        bundle_id: Option<&str>,
    ) -> Result<bool, StorageError> {
        bundle_or_source_write_is_blocked(&self.connection, bundle_id, Some(source_id))
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
                && mount_object_is_blocked(&transaction, item.member_id, item.target_path)?
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
                    source_catalog_generation, source_commit_sha, expected_current_target,
                    expected_adopted_commit_sha, bundle_id, bundle_display_name,
                    warnings_json, created_at, expires_at, status
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                    ?15, ?16, ?17, ?18, ?19, 'pending'
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
                    plan.source_commit_sha,
                    plan.expected_current_target,
                    plan.expected_adopted_commit_sha,
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
                        skill_description, content_fingerprint, selectable,
                        preserve_existing, validation_errors_json, warnings_json,
                        default_selected, selected, sort_order
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12
                     )",
                    params![
                        plan.id,
                        candidate.candidate_id,
                        candidate.source_relative_path,
                        candidate.skill_name,
                        candidate.skill_description,
                        candidate.content_fingerprint,
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

    pub fn read_expired_pending_install_plans(
        &self,
        now: i64,
    ) -> Result<Vec<StoredInstallPlan>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id FROM install_plans
                 WHERE status = 'pending'
                   AND kind = 'github_snapshot'
                   AND expires_at <= ?1
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

    pub fn read_pending_github_install_plans_for_source(
        &self,
        source_id: &str,
    ) -> Result<Vec<StoredInstallPlan>, StorageError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id FROM install_plans
                 WHERE status = 'pending'
                   AND kind = 'github_snapshot'
                   AND source_id = ?1
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

    /// 只删除尚未进入生命周期事务的失效 Plan；快照内容由 Lifecycle 先安全清理。
    pub fn discard_pending_install_plan(&mut self, plan_id: &str) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::SaveInstallPlan)?;
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
                "SELECT display_name, repository_url
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

    fn read_mount_summaries(&self) -> Result<Vec<MountSummary>, StorageError> {
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
    if mount_object_is_blocked(connection, member_id, target_path)? {
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

    // Takeover 的领域 Member 可能尚未创建，因此 blocked 隔离必须同时按路径判断。
    let mut statement = connection
        .prepare(
            "SELECT id, member_id, reserved_paths_json
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
            ))
        })
        .map_err(StorageError::ReadTakeoverTransaction)?;
    for row in rows {
        let (transaction_id, reserved_member_id, reserved_paths_json) =
            row.map_err(StorageError::ReadTakeoverTransaction)?;
        let reserved_paths = decode_takeover_reserved_paths(&transaction_id, &reserved_paths_json)?;
        if reserved_member_id == member_id
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
    if mount_object_is_blocked(connection, &item.member_id, &item.target_path)? {
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
        plan.source_commit_sha,
        plan.expected_current_target,
        plan.expected_adopted_commit_sha,
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
        plan.source_commit_sha.as_deref(),
        plan.expected_current_target.as_deref(),
        plan.expected_adopted_commit_sha.as_deref(),
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
                &candidate.source_relative_path,
                candidate.skill_name.as_deref(),
                candidate.skill_description.as_deref(),
                candidate.content_fingerprint.as_deref(),
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
    source_commit_sha: Option<&str>,
    expected_current_target: Option<&str>,
    expected_adopted_commit_sha: Option<&str>,
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
        && source_commit_sha.is_none()
        && expected_current_target.is_none()
        && expected_adopted_commit_sha.is_none();
    let folder_is_valid = kind == "folder_snapshot"
        && install_mode == "create"
        && input_path.is_some_and(is_normalized_absolute_path)
        && snapshot_relative_path.is_none()
        && source_values_are_absent;
    let github_source_is_valid = source_id.is_some_and(is_single_path_component)
        && source_tracked_ref.is_some_and(|value| !value.is_empty())
        && source_catalog_generation.is_some_and(|generation| generation > 0)
        && source_commit_sha.is_some_and(|value| !value.is_empty())
        && snapshot_relative_path.is_some_and(is_normalized_relative_path)
        && input_path.is_none();
    let github_mode_is_valid = match install_mode {
        "create" => expected_current_target.is_none() && expected_adopted_commit_sha.is_none(),
        "supplement" => {
            expected_current_target.is_some_and(is_safe_current_target)
                && expected_adopted_commit_sha.is_none_or(|value| !value.is_empty())
        }
        _ => false,
    };
    if common_is_valid
        && (folder_is_valid
            || (kind == "github_snapshot" && github_source_is_valid && github_mode_is_valid))
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
    source_relative_path: &str,
    skill_name: Option<&str>,
    skill_description: Option<&str>,
    content_fingerprint: Option<&str>,
    selectable: bool,
    preserve_existing: bool,
    default_selected: bool,
    selected: bool,
) -> bool {
    let metadata_is_complete = skill_name.is_some_and(is_single_path_component)
        && skill_description.is_some()
        && content_fingerprint.is_some_and(|value| !value.is_empty());
    let source_path_is_valid =
        source_relative_path.is_empty() || is_normalized_relative_path(source_relative_path);
    is_single_path_component(candidate_id)
        && source_path_is_valid
        && (!selectable || metadata_is_complete)
        && (!preserve_existing
            || (plan_kind == "github_snapshot"
                && install_mode == "supplement"
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
    let source = read_github_install_source_from(connection, source_id)?;
    if plan.source_tracked_ref.as_deref() != Some(source.tracked_ref.as_str())
        || plan.source_catalog_generation != Some(source.catalog_generation)
        || plan.source_commit_sha.as_deref() != Some(source.catalog_commit_sha.as_str())
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
                || bundle.adopted_commit_sha != plan.expected_adopted_commit_sha
            {
                return Err(StorageError::SourceBundleStateConflict);
            }
            validate_preserved_install_members(plan, bundle)?;
            Some(bundle)
        }
        _ => return Err(StorageError::InvalidInstallPlan),
    };
    validate_github_install_candidates(plan, &source.catalog_members, bundle)
}

fn validate_preserved_install_members(
    plan: &StoredInstallPlan,
    bundle: &StoredGithubInstallBundle,
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

fn validate_github_install_candidates(
    plan: &StoredInstallPlan,
    catalog_members: &[StoredGithubInstallCatalogMember],
    bundle: Option<&StoredGithubInstallBundle>,
) -> Result<(), StorageError> {
    let existing_paths = bundle
        .into_iter()
        .flat_map(|bundle| bundle.members.iter())
        .map(|member| member.source_relative_path.as_str())
        .collect::<BTreeSet<_>>();
    let candidates = plan
        .candidates
        .iter()
        .filter(|candidate| !candidate.preserve_existing)
        .map(|candidate| (candidate.source_relative_path.as_str(), candidate))
        .collect::<BTreeMap<_, _>>();
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

fn validate_takeover_domain_contract(
    data_root: &Path,
    plan: &TakeoverPlan,
) -> Result<(String, String, String, String), StorageError> {
    let selected = plan
        .origins
        .iter()
        .filter(|origin| origin.observation_id == plan.selected_observation_id)
        .collect::<Vec<_>>();
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
    let stable_relative_path = format!("members/{}", plan.skill_name);
    let targets_are_valid = plan.targets.iter().all(|target| {
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
            && target.expected_target == plan.expected_target
            && Path::new(&target.target_path)
                .file_name()
                .and_then(|name| name.to_str())
                == Some(plan.skill_name.as_str())
    });
    if !is_single_path_component(&plan.id)
        || !is_single_path_component(&plan.bundle_id)
        || !is_single_path_component(&plan.member_id)
        || !is_single_path_component(&plan.content_id)
        || !is_single_path_component(&plan.skill_name)
        || plan.bundle_display_name.trim().is_empty()
        || plan.skill_description.trim().is_empty()
        || plan.source_display_name.is_some()
        || selected.len() != 1
        || selected[0].content_fingerprint.is_empty()
        || origin_ids.len() != plan.origins.len()
        || plan.origins.iter().any(|origin| {
            origin.observation_id.is_empty() || !is_normalized_absolute_path(&origin.original_path)
        })
        || mount_ids.len() != plan.targets.len()
        || target_paths.len() != plan.targets.len()
        || !targets_are_valid
        || Path::new(&plan.managed_directory) != data_root.join(&managed_directory)
        || Path::new(&plan.content_directory)
            != data_root.join(&managed_directory).join(&current_target)
        || Path::new(&plan.expected_target)
            != data_root
                .join(&managed_directory)
                .join("current")
                .join(&stable_relative_path)
    {
        return Err(StorageError::InvalidTakeoverPlan);
    }
    Ok((
        selected[0].content_fingerprint.clone(),
        managed_directory,
        current_target,
        stable_relative_path,
    ))
}

fn ensure_takeover_domain_matches(
    transaction: &Transaction<'_>,
    data_root: &Path,
    plan: &TakeoverPlan,
    fingerprint: &str,
) -> Result<(), StorageError> {
    let member = read_managed_member_from(transaction, data_root, &plan.member_id)?
        .ok_or(StorageError::InvalidTakeoverPlan)?;
    let bundle = transaction
        .query_row(
            "SELECT display_name, managed_directory, current_target,
                    (SELECT description FROM skill_members WHERE id = ?2),
                    EXISTS(SELECT 1 FROM member_selections WHERE bundle_id = ?1 AND member_id = ?2),
                    (SELECT COUNT(*) FROM skill_members WHERE bundle_id = ?1),
                    (SELECT COUNT(*) FROM member_selections WHERE bundle_id = ?1),
                    (SELECT COUNT(*) FROM mounts WHERE member_id = ?2)
             FROM bundles WHERE id = ?1",
            params![plan.bundle_id, plan.member_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::SaveTakeoverTransaction)?
        .ok_or(StorageError::InvalidTakeoverPlan)?;
    if member.bundle_id != plan.bundle_id
        || member.skill_name != plan.skill_name
        || member.content_fingerprint != fingerprint
        || member.expected_target != plan.expected_target
        || bundle.0 != plan.bundle_display_name
        || bundle.1 != member.managed_directory
        || bundle.2 != member.current_target
        || bundle.3 != plan.skill_description
        || !bundle.4
        || bundle.5 != 1
        || bundle.6 != 1
        || bundle.7 != plan.targets.len() as i64
    {
        return Err(StorageError::InvalidTakeoverPlan);
    }
    for target in &plan.targets {
        let mount = read_mount_from(transaction, data_root, &target.mount_id)?
            .ok_or(StorageError::InvalidTakeoverPlan)?;
        if mount.member_id != plan.member_id
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

fn read_scan_issues_from(connection: &Connection) -> Result<Vec<ScanIssue>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT root_id, root_key, project_id, path, code, message
             FROM inventory_scan_issues ORDER BY root_id",
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
        issues.push(ScanIssue {
            root_id,
            root_key,
            project_id,
            path,
            code: ScanIssueCode::from_str(&code)
                .ok_or_else(|| StorageError::UnknownScanIssueCode(code.clone()))?,
            message,
        });
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

fn read_install_plan_from(
    connection: &Connection,
    plan_id: &str,
) -> Result<Option<StoredInstallPlan>, StorageError> {
    let row = connection
        .query_row(
            "SELECT
                id, kind, install_mode, input_path, input_device, input_inode,
                input_fingerprint, snapshot_relative_path, source_id, source_tracked_ref,
                source_catalog_generation, source_commit_sha, expected_current_target,
                expected_adopted_commit_sha, bundle_id, bundle_display_name, expires_at, status
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
                    source_commit_sha: row.get(11)?,
                    expected_current_target: row.get(12)?,
                    expected_adopted_commit_sha: row.get(13)?,
                    bundle_id: row.get(14)?,
                    bundle_display_name: row.get(15)?,
                    expires_at: row.get(16)?,
                    status: row.get(17)?,
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
        source_commit_sha: row.source_commit_sha,
        expected_current_target: row.expected_current_target,
        expected_adopted_commit_sha: row.expected_adopted_commit_sha,
        bundle_id: row.bundle_id,
        bundle_display_name: row.bundle_display_name,
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
                    content_fingerprint, selectable, preserve_existing,
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
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
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

fn read_github_install_source_from(
    connection: &Connection,
    source_id: &str,
) -> Result<StoredGithubInstallSource, StorageError> {
    let source = connection
        .query_row(
            "SELECT owner, repository, display_name, tracked_ref, catalog_status,
                    catalog_generation, catalog_commit_sha
             FROM sources
             WHERE id = ?1 AND kind = 'github'",
            [source_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::ReadSources)?
        .ok_or(StorageError::SourceNotFound)?;
    let (
        owner,
        repository,
        display_name,
        tracked_ref,
        catalog_status,
        catalog_generation,
        catalog_commit_sha,
    ) = source;
    let Some(catalog_commit_sha) = catalog_commit_sha else {
        return Err(StorageError::SourceCatalogStateChanged);
    };
    if catalog_status != "fresh"
        || catalog_generation <= 0
        || tracked_ref.is_empty()
        || catalog_commit_sha.is_empty()
    {
        return Err(StorageError::SourceCatalogStateChanged);
    }

    let catalog_members =
        read_github_install_catalog_members_from(connection, source_id, catalog_generation)?;
    let linked = connection
        .query_row(
            "SELECT bundle_id, adopted_commit_sha
             FROM source_bundle_links
             WHERE source_id = ?1",
            [source_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(StorageError::ReadSources)?;
    let bundle = linked
        .map(|(bundle_id, adopted_commit_sha)| {
            read_github_install_bundle_from(connection, source_id, &bundle_id, adopted_commit_sha)
        })
        .transpose()?;

    Ok(StoredGithubInstallSource {
        id: source_id.to_owned(),
        owner,
        repository,
        display_name,
        tracked_ref,
        catalog_generation,
        catalog_commit_sha,
        catalog_members,
        bundle,
    })
}

fn read_github_install_catalog_members_from(
    connection: &Connection,
    source_id: &str,
    catalog_generation: i64,
) -> Result<Vec<StoredGithubInstallCatalogMember>, StorageError> {
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
        members.push(StoredGithubInstallCatalogMember {
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

fn read_github_install_bundle_from(
    connection: &Connection,
    source_id: &str,
    bundle_id: &str,
    adopted_commit_sha: Option<String>,
) -> Result<StoredGithubInstallBundle, StorageError> {
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
        let Some(source_relative_path) = source_relative_path else {
            return Err(StorageError::SourceBundleStateConflict);
        };
        if selected_member_id.as_deref() != Some(id.as_str())
            || stable_relative_path != format!("members/{skill_name}")
            || (!source_relative_path.is_empty()
                && !is_normalized_relative_path(&source_relative_path))
        {
            return Err(StorageError::SourceBundleStateConflict);
        }
        members.push(StoredGithubInstallBundleMember {
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
    if members.is_empty() || source_link_count != members.len() as i64 {
        return Err(StorageError::SourceBundleStateConflict);
    }
    Ok(StoredGithubInstallBundle {
        id: bundle_id.to_owned(),
        display_name,
        current_target,
        adopted_commit_sha,
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

fn inventory_item_from_observation(
    observation: InventoryObservation,
    project_display_name: Option<String>,
) -> InventoryItem {
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
        bundle_id: None,
        member_id: None,
        bundle_display_name: None,
        source_display_name: None,
        project_display_name,
    }
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
    if plan.kind == "github_snapshot" {
        let source_id = plan
            .source_id
            .as_deref()
            .ok_or(StorageError::InvalidInstallPlan)?;
        let source_commit_sha = plan
            .source_commit_sha
            .as_deref()
            .ok_or(StorageError::InvalidInstallPlan)?;
        transaction
            .execute(
                "INSERT INTO source_bundle_links (
                    source_id, bundle_id, adopted_commit_sha, linked_at
                 ) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(source_id) DO NOTHING",
                params![source_id, plan.bundle_id, source_commit_sha, now],
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
    if plan.kind != "github_snapshot" {
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
            "SELECT bundle_id, adopted_commit_sha
             FROM source_bundle_links
             WHERE source_id = ?1",
            [source_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(StorageError::SaveManagedBundle)?
        .ok_or(StorageError::SourceBundleStateConflict)?;
    if source_baseline
        != (
            plan.bundle_id.clone(),
            plan.expected_adopted_commit_sha.clone(),
        )
    {
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
    transaction
        .execute(
            "INSERT INTO source_member_links (
                source_id, source_relative_path, member_id, linked_at
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(source_id, source_relative_path) DO NOTHING",
            params![
                source_id,
                candidate.source_relative_path,
                candidate.candidate_id,
                now
            ],
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
    let expected_adopted_commit_sha = if plan.install_mode == "create" {
        plan.source_commit_sha.clone()
    } else {
        plan.expected_adopted_commit_sha.clone()
    };
    let source_bundle = transaction
        .query_row(
            "SELECT bundle_id, adopted_commit_sha
             FROM source_bundle_links
             WHERE source_id = ?1",
            [source_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(StorageError::SaveManagedBundle)?;
    if source_bundle != Some((plan.bundle_id.clone(), expected_adopted_commit_sha)) {
        return Err(StorageError::ManagedStateConflict);
    }
    let link_count = transaction
        .query_row(
            "SELECT COUNT(*) FROM source_member_links WHERE source_id = ?1",
            [source_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(StorageError::SaveManagedBundle)?;
    if link_count != selected.len() as i64 {
        return Err(StorageError::ManagedStateConflict);
    }
    for candidate in selected {
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
        if linked_path.as_deref() != Some(candidate.source_relative_path.as_str()) {
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
    let failed = scan_issues
        .iter()
        .map(scan_issue_identity)
        .collect::<BTreeSet<_>>();
    let previous_by_id = previous
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut combined = previous
        .iter()
        .filter(|entry| !successful.contains(&observation_root_identity(entry)))
        .cloned()
        .map(|mut entry| {
            if failed.contains(&observation_root_identity(&entry)) {
                entry.stale = true;
            }
            entry
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
        .map(|issue| (issue.root_id.clone(), issue))
        .collect::<BTreeMap<_, _>>();
    for issue in current {
        combined.insert(issue.root_id.clone(), issue.clone());
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
        TakeoverIdentityBasis, TakeoverOriginDisposition, TakeoverPlanOrigin, TakeoverPlanTarget,
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
            identity_basis: TakeoverIdentityBasis::SingleOrigin,
            selected_observation_id: observation_id.clone(),
            bundle_id,
            member_id,
            content_id,
            bundle_display_name: format!("Takeover {suffix}"),
            skill_name: skill_name.clone(),
            skill_description: "接管持久化测试 Skill".to_owned(),
            source_display_name: None,
            managed_directory: managed_directory.to_string_lossy().into_owned(),
            content_directory: content_directory.to_string_lossy().into_owned(),
            expected_target: expected_target.to_string_lossy().into_owned(),
            origins: vec![TakeoverPlanOrigin {
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
                    plan.skill_name,
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
                &plan.member_id,
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
            source_relative_path: "",
            skill_name: Some(member_id),
            skill_description: Some("测试 Skill"),
            content_fingerprint: Some("sha256:test"),
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
                source_commit_sha: None,
                expected_current_target: None,
                expected_adopted_commit_sha: None,
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
                source_relative_path: "skills/alpha",
                skill_name: Some("alpha"),
                skill_description: Some("Alpha Skill"),
                content_fingerprint: Some("sha256:alpha-v1"),
                selectable: true,
                preserve_existing: false,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
            NewInstallCandidate {
                candidate_id: "catalog-beta-v1",
                source_relative_path: "skills/beta",
                skill_name: Some("beta"),
                skill_description: Some("Beta Skill"),
                content_fingerprint: Some("sha256:beta"),
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
                kind: "github_snapshot",
                install_mode: "create",
                input_path: None,
                input_device: 10,
                input_inode: 20,
                input_fingerprint: "sha256:github-snapshot-v1",
                snapshot_relative_path: Some(&snapshot_relative_path),
                source_id: Some(TEST_GITHUB_SOURCE_ID),
                source_tracked_ref: Some("main"),
                source_catalog_generation: Some(1),
                source_commit_sha: Some(TEST_GITHUB_COMMIT_ONE),
                expected_current_target: None,
                expected_adopted_commit_sha: None,
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
                source_relative_path: "skills/alpha",
                skill_name: Some("alpha"),
                skill_description: Some("Alpha Skill"),
                content_fingerprint: Some("sha256:alpha-v1"),
                selectable: false,
                preserve_existing: true,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
            NewInstallCandidate {
                candidate_id: "catalog-beta-v2",
                source_relative_path: "skills/beta",
                skill_name: Some("beta"),
                skill_description: Some("Beta Skill"),
                content_fingerprint: Some("sha256:beta"),
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
                kind: "github_snapshot",
                install_mode: "supplement",
                input_path: None,
                input_device: 11,
                input_inode: 21,
                input_fingerprint: "sha256:github-snapshot-v2",
                snapshot_relative_path: Some(&snapshot_relative_path),
                source_id: Some(TEST_GITHUB_SOURCE_ID),
                source_tracked_ref: Some("main"),
                source_catalog_generation: Some(2),
                source_commit_sha: Some(TEST_GITHUB_COMMIT_TWO),
                expected_current_target: Some(expected_current_target),
                expected_adopted_commit_sha: Some(TEST_GITHUB_COMMIT_ONE),
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
                source_relative_path: &first_name,
                skill_name: Some(&first_name),
                skill_description: Some("第一个测试 Skill"),
                content_fingerprint: Some("sha256:alpha"),
                selectable: true,
                preserve_existing: false,
                validation_errors: &[],
                warnings: &[],
                default_selected: true,
            },
            NewInstallCandidate {
                candidate_id: &second_id,
                source_relative_path: &second_name,
                skill_name: Some(&second_name),
                skill_description: Some("第二个测试 Skill"),
                content_fingerprint: Some("sha256:beta"),
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
                source_commit_sha: None,
                expected_current_target: None,
                expected_adopted_commit_sha: None,
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
        assert_eq!(versions, (1..=13).collect::<Vec<_>>());
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
            "takeover_plans",
            "takeover_transactions",
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
    fn install_bundle_migration_rebuilds_the_canonical_protocol() {
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
            "source_commit_sha",
            "expected_current_target",
            "expected_adopted_commit_sha",
        ] {
            assert!(
                install_columns.contains_key(column),
                "统一安装协议应包含 {column}"
            );
        }
        assert_eq!(
            install_columns.get("input_path"),
            Some(&0),
            "GitHub Plan 不应伪造本地输入路径"
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
                source_catalog_generation, source_commit_sha, expected_current_target,
                expected_adopted_commit_sha, bundle_id, bundle_display_name,
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
            .read_github_install_source(TEST_GITHUB_SOURCE_ID)
            .expect("Fresh Catalog 应可组成安装输入");
        assert_eq!(source.owner, "anthropics");
        assert_eq!(source.repository, "skills");
        assert_eq!(source.catalog_generation, 1);
        assert_eq!(source.catalog_commit_sha, TEST_GITHUB_COMMIT_ONE);
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
            .read_github_install_source(TEST_GITHUB_SOURCE_ID)
            .expect("应读取已关联 Source");
        let bundle = source.bundle.expect("create 应关联唯一 Bundle");
        assert_eq!(bundle.id, "github-bundle");
        assert_eq!(
            bundle.adopted_commit_sha.as_deref(),
            Some(TEST_GITHUB_COMMIT_ONE)
        );
        assert_eq!(bundle.members.len(), 2);
        assert_eq!(
            bundle
                .members
                .iter()
                .map(|member| member.source_relative_path.as_str())
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
                "SELECT bundle.current_target, link.adopted_commit_sha,
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
        assert_eq!(transactions[0].member_id, plan.member_id);
        assert_eq!(
            transactions[0].reserved_paths,
            takeover_reserved_paths(&plan).expect("测试 Plan 应产生合法保留路径")
        );

        let mut mismatched = plan.clone();
        mismatched.member_id = "different-member".to_owned();
        assert!(matches!(
            storage.finalize_takeover("takeover-txn-subject", &mismatched, 202),
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
            storage.finalize_takeover("takeover-txn-subject", &plan, 203),
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
                    plan.member_id,
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
                params![plan.id, plan.bundle_id, plan.member_id, reserved_paths_json],
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
            storage.finalize_takeover("takeover-txn-conflict", &plan, 206),
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
                    plan.member_id,
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
            .finalize_takeover("takeover-txn-replay", &plan, 205)
            .expect("首次 Takeover 提交应成功");
        storage
            .connection
            .execute(
                "DELETE FROM mounts WHERE id = ?1",
                [&plan.targets[0].mount_id],
            )
            .expect("应模拟提交后的领域记录丢失");

        assert!(matches!(
            storage.finalize_takeover("takeover-txn-replay", &plan, 206),
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
            .save_initial_scan(500, &[], &[])
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
            .save_initial_scan(500, &[], &[])
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
            .save_initial_scan(202, &[], &[])
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
            .save_initial_scan(204, &[], &[])
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
