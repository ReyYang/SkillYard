//! GitHub public Source 的最外层网络协议、确定性输入解析与 Catalog 归档边界。

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions, Permissions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use reqwest::{
    blocking::{Client, Response},
    header::{ACCEPT, USER_AGENT},
    redirect::Policy,
};
use serde::Deserialize;
use thiserror::Error;
use url::Url;
use uuid::Uuid;
use zip::{CompressionMethod, ZipArchive};

use crate::content::{
    ContentValidationError, DiscoveredBundleCandidate, MAX_ENTRIES, MAX_SINGLE_FILE_BYTES,
    MAX_TOTAL_FILE_BYTES, validate_skill_bundle_folder,
};

/// 所有 GitHub 响应（包括 archive）都不能超过这个固定上限。
pub const MAX_RESPONSE_BYTES: u64 = 100 * 1024 * 1024;
const GITHUB_ACCEPT: &str = "application/vnd.github+json";
const GITHUB_USER_AGENT: &str = "SkillYard/1.0";
const GITHUB_TIMEOUT: Duration = Duration::from_secs(20);
const STREAM_BUFFER_BYTES: usize = 8 * 1024;

/// 可替换的 HTTP 请求，避免测试替换 GitHub 解析或 JSON 协议代码。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRequest {
    pub url: Url,
    pub accept: String,
}

/// 可替换网络边界返回的最小真实 HTTP 信息。
pub struct SourceResponse {
    pub status: u16,
    pub final_url: Url,
    pub body: Box<dyn Read + Send>,
}

/// GitHub Source 的唯一可替换网络边界。
pub trait SourceTransport: Send + Sync {
    fn get(&self, request: SourceRequest) -> Result<SourceResponse, SourceTransportError>;
}

/// 生产使用的无认证 blocking HTTPS client。
#[derive(Clone)]
pub struct ReqwestSourceTransport {
    client: Client,
}

impl ReqwestSourceTransport {
    pub fn new() -> Result<Self, SourceTransportError> {
        let client = Client::builder()
            .timeout(GITHUB_TIMEOUT)
            // 重定向目标只允许 GitHub 三个公开 HTTPS 域名，避免 archive 跳到任意站点。
            .redirect(Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 || !is_allowed_redirect(attempt.url()) {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(|_| SourceTransportError::Unavailable)?;
        Ok(Self { client })
    }
}

impl SourceTransport for ReqwestSourceTransport {
    fn get(&self, request: SourceRequest) -> Result<SourceResponse, SourceTransportError> {
        let response = self
            .client
            .get(request.url)
            .header(ACCEPT, request.accept)
            .header(USER_AGENT, GITHUB_USER_AGENT)
            .send()
            .map_err(|_| SourceTransportError::Unavailable)?;
        Ok(response_from_reqwest(response))
    }
}

fn response_from_reqwest(response: Response) -> SourceResponse {
    SourceResponse {
        status: response.status().as_u16(),
        final_url: response.url().clone(),
        body: Box::new(response),
    }
}

fn is_allowed_redirect(url: &Url) -> bool {
    url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some("api.github.com" | "github.com" | "codeload.github.com")
        )
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum SourceTransportError {
    #[error("GitHub 网络请求失败")]
    Unavailable,
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum GithubSourceError {
    #[error("不支持的 GitHub 来源输入")]
    UnsupportedInput,
    #[error("GitHub Tracked Ref 不合法")]
    InvalidTrackedRef,
    #[error("GitHub URL 与 Tracked Ref 冲突")]
    RefConflict,
    #[error("GitHub 网络请求失败")]
    Network,
    #[error("GitHub 返回了不支持的 HTTP 状态")]
    HttpStatus,
    #[error("GitHub 响应超过 100 MiB 限制")]
    ResponseTooLarge,
    #[error("GitHub 返回了无效响应")]
    InvalidResponse,
    #[error("GitHub 仓库不是公开仓库")]
    PrivateRepository,
    #[error("GitHub Catalog 临时区不可用")]
    StagingUnavailable,
    #[error("GitHub Catalog 临时内容清理失败")]
    StagingCleanupFailed,
    #[error("GitHub 返回了无效 ZIP archive")]
    InvalidArchive,
    #[error("GitHub archive 必须只有一个共同顶层目录")]
    InvalidArchiveRoot,
    #[error("GitHub archive 包含不安全路径：{path}")]
    UnsafeArchivePath { path: String },
    #[error("GitHub archive 包含重复规范化路径：{path}")]
    DuplicateArchivePath { path: String },
    #[error("GitHub archive 包含加密条目：{path}")]
    EncryptedArchiveEntry { path: String },
    #[error("GitHub archive 包含不支持的特殊条目：{path}")]
    UnsupportedArchiveEntry { path: String },
    #[error("GitHub archive 包含不支持的压缩格式：{path}")]
    UnsupportedArchiveCompression { path: String },
    #[error("GitHub archive 条目数超过固定上限 {limit}：已检测到 {actual}")]
    ArchiveEntryLimitExceeded { limit: usize, actual: usize },
    #[error("GitHub archive 普通文件总量超过固定上限 {limit} bytes：已检测到 {actual} bytes")]
    ArchiveTotalSizeLimitExceeded { limit: u64, actual: u64 },
    #[error("GitHub archive 普通文件超过固定单文件上限 {limit} bytes：{path} 为 {actual} bytes")]
    ArchiveFileSizeLimitExceeded {
        path: String,
        limit: u64,
        actual: u64,
    },
    #[error("GitHub Catalog 内容验证失败：{0}")]
    ContentValidation(String),
}

/// 解析完输入、尚未联网的仓库定位信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedGithubSource {
    pub owner: String,
    pub repository: String,
    pub tracked_ref: Option<String>,
    pub member_hint: Option<String>,
}

/// 已由 GitHub metadata 和 commit API 确认的 Source 身份。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedGithubSource {
    pub owner: String,
    pub repository: String,
    pub display_name: String,
    pub canonical_identity: String,
    pub repository_url: Url,
    pub tracked_ref: String,
    pub commit: String,
    pub member_hint: Option<String>,
}

/// Catalog reload 只接收已经持久化并规范化的仓库 metadata。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GithubCatalogTarget<'a> {
    pub owner: &'a str,
    pub repository: &'a str,
    pub canonical_identity: &'a str,
    pub display_name: &'a str,
    pub tracked_ref: &'a str,
}

/// 一次完整 Catalog 获取绑定的不可移动 commit 与候选 metadata。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchedGithubCatalog {
    pub commit_sha: String,
    pub candidates: Vec<DiscoveredBundleCandidate>,
}

/// `Arc<dyn SourceTransport>` 便于 application 只持有一个可替换的外部网络能力。
pub type SharedSourceTransport = Arc<dyn SourceTransport>;

/// 只接受产品契约列出的 GitHub 写法，并在联网前拒绝歧义和危险路径。
pub fn parse_github_source(
    input: &str,
    explicit_ref: Option<&str>,
) -> Result<ParsedGithubSource, GithubSourceError> {
    // 粘贴输入常带首尾空白；只清理外围，不改写仓库名、ref 或成员路径本身。
    let input = input.trim();
    let explicit_ref = explicit_ref
        .map(str::trim)
        .map(validate_tracked_ref)
        .transpose()?;
    // `url` 会规范化 dot segment；在此之前拒绝原始危险写法，避免它变成另一条合法 URL。
    if input.contains('\\')
        || input.split('/').any(|segment| {
            decode_url_path_segment(segment)
                .is_ok_and(|decoded| matches!(decoded.as_str(), "." | ".."))
        })
    {
        return Err(GithubSourceError::UnsupportedInput);
    }
    if let Ok(url) = Url::parse(input) {
        return parse_github_url(url, explicit_ref);
    }

    let segments = input.split('/').collect::<Vec<_>>();
    if segments.len() != 2
        || !valid_name_segment(segments[0])
        || !valid_repository_segment(segments[1])
    {
        return Err(GithubSourceError::UnsupportedInput);
    }
    Ok(ParsedGithubSource {
        owner: segments[0].to_owned(),
        repository: strip_git_suffix(segments[1])?.to_owned(),
        tracked_ref: explicit_ref,
        member_hint: None,
    })
}

/// 先确认公开 metadata，再将目标 ref 固定解析成不可移动的 commit SHA。
pub fn resolve_github_source(
    transport: &dyn SourceTransport,
    input: &str,
    explicit_ref: Option<&str>,
) -> Result<ResolvedGithubSource, GithubSourceError> {
    let parsed = parse_github_source(input, explicit_ref)?;
    let metadata_url = api_url(&["repos", &parsed.owner, &parsed.repository])?;
    let metadata: RepositoryMetadata = read_json(transport, metadata_url)?;
    if metadata.private {
        return Err(GithubSourceError::PrivateRepository);
    }
    let (owner, repository) = parse_full_name(&metadata.full_name)?;
    let tracked_ref = parsed
        .tracked_ref
        .unwrap_or_else(|| metadata.default_branch.clone());
    validate_tracked_ref(&tracked_ref)?;

    // 每个 path segment 单独编码，禁止 ref 中的 `/`、`?`、`#` 改写 REST 路由。
    let commit_url = api_url(&["repos", &owner, &repository, "commits", &tracked_ref])?;
    let commit: CommitMetadata = read_json(transport, commit_url)?;
    if !is_commit_sha(&commit.sha) {
        return Err(GithubSourceError::InvalidResponse);
    }
    let repository_url = Url::parse(&format!("https://github.com/{owner}/{repository}"))
        .map_err(|_| GithubSourceError::InvalidResponse)?;
    Ok(ResolvedGithubSource {
        canonical_identity: format!(
            "github:{}/{}",
            owner.to_ascii_lowercase(),
            repository.to_ascii_lowercase()
        ),
        owner,
        repository,
        display_name: metadata.full_name,
        repository_url,
        tracked_ref,
        commit: commit.sha,
        member_hint: parsed.member_hint,
    })
}

/// 重新验证持久化仓库 metadata，并把同一 Tracked Ref 固定到 SHA 后获取完整 Catalog。
pub fn fetch_github_catalog(
    transport: &dyn SourceTransport,
    target: GithubCatalogTarget<'_>,
    staging_root: &Path,
) -> Result<FetchedGithubCatalog, GithubSourceError> {
    validate_catalog_target(target)?;
    let metadata_url = api_url(&["repos", target.owner, target.repository])?;
    let metadata: RepositoryMetadata = read_json(transport, metadata_url)?;
    if metadata.private {
        return Err(GithubSourceError::PrivateRepository);
    }
    let (owner, repository) = parse_full_name(&metadata.full_name)?;
    let remote_identity = format!(
        "github:{}/{}",
        owner.to_ascii_lowercase(),
        repository.to_ascii_lowercase()
    );
    if remote_identity != target.canonical_identity {
        // 1.0 不跟随仓库改名或重定向，避免一次 reload 静默改变 Source 身份。
        return Err(GithubSourceError::InvalidResponse);
    }

    let commit_url = api_url(&["repos", &owner, &repository, "commits", target.tracked_ref])?;
    let commit: CommitMetadata = read_json(transport, commit_url)?;
    if !is_commit_sha(&commit.sha) {
        return Err(GithubSourceError::InvalidResponse);
    }

    let temporary = CatalogTempTree::create(staging_root)?;
    let result = fetch_catalog_into_temporary_tree(
        transport,
        &owner,
        &repository,
        target.repository,
        &commit.sha,
        &temporary.path,
    );
    let cleanup = temporary.cleanup();
    match cleanup {
        Ok(()) => result,
        Err(error) => Err(error),
    }
}

fn validate_catalog_target(target: GithubCatalogTarget<'_>) -> Result<(), GithubSourceError> {
    if !valid_name_segment(target.owner)
        || !valid_repository_segment(target.repository)
        || target.repository.ends_with(".git")
    {
        return Err(GithubSourceError::InvalidResponse);
    }
    validate_tracked_ref(target.tracked_ref)?;
    let expected_identity = format!(
        "github:{}/{}",
        target.owner.to_ascii_lowercase(),
        target.repository.to_ascii_lowercase()
    );
    let (display_owner, display_repository) = parse_full_name(target.display_name)?;
    if target.canonical_identity != expected_identity
        || !display_owner.eq_ignore_ascii_case(target.owner)
        || !display_repository.eq_ignore_ascii_case(target.repository)
    {
        return Err(GithubSourceError::InvalidResponse);
    }
    Ok(())
}

fn fetch_catalog_into_temporary_tree(
    transport: &dyn SourceTransport,
    owner: &str,
    repository: &str,
    repository_root_name: &str,
    commit_sha: &str,
    temporary_root: &Path,
) -> Result<FetchedGithubCatalog, GithubSourceError> {
    let archive_url = api_url(&["repos", owner, repository, "zipball", commit_sha])?;
    let archive_path = temporary_root.join("source.zip");
    download_archive(transport, archive_url, &archive_path)?;

    let archive_file =
        File::open(&archive_path).map_err(|_| GithubSourceError::StagingUnavailable)?;
    let mut archive =
        ZipArchive::new(archive_file).map_err(|_| GithubSourceError::InvalidArchive)?;
    let mut central_directory_file =
        File::open(&archive_path).map_err(|_| GithubSourceError::StagingUnavailable)?;
    let limits = ArchiveLimits::PRODUCTION;
    let plan = preflight_archive(&mut archive, &mut central_directory_file, limits)?;
    let repository_root = temporary_root.join(repository_root_name);
    extract_archive(&mut archive, &plan, &repository_root, limits)?;

    let canonical_repository_root =
        fs::canonicalize(&repository_root).map_err(|_| GithubSourceError::StagingUnavailable)?;
    let mut candidates = match validate_skill_bundle_folder(&canonical_repository_root) {
        Ok(validated) => validated.candidates,
        Err(ContentValidationError::NoSkillMetadataFound) => Vec::new(),
        Err(error) => {
            return Err(GithubSourceError::ContentValidation(
                normalize_temporary_error(
                    &error.to_string(),
                    temporary_root,
                    &canonical_repository_root,
                    repository_root_name,
                ),
            ));
        }
    };
    for candidate in &mut candidates {
        for error in &mut candidate.validation_errors {
            *error = normalize_temporary_error(
                error,
                temporary_root,
                &canonical_repository_root,
                repository_root_name,
            );
        }
    }
    Ok(FetchedGithubCatalog {
        commit_sha: commit_sha.to_owned(),
        candidates,
    })
}

fn download_archive(
    transport: &dyn SourceTransport,
    url: Url,
    archive_path: &Path,
) -> Result<(), GithubSourceError> {
    let mut response = transport
        .get(SourceRequest {
            url,
            accept: GITHUB_ACCEPT.to_owned(),
        })
        .map_err(|_| GithubSourceError::Network)?;
    if !(200..300).contains(&response.status) {
        return Err(GithubSourceError::HttpStatus);
    }
    let mut archive_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(archive_path)
        .map_err(|_| GithubSourceError::StagingUnavailable)?;
    match copy_with_limit(&mut response.body, &mut archive_file, MAX_RESPONSE_BYTES) {
        Ok(_) => archive_file
            .flush()
            .map_err(|_| GithubSourceError::StagingUnavailable),
        Err(StreamCopyError::Read) => Err(GithubSourceError::InvalidResponse),
        Err(StreamCopyError::Write) => Err(GithubSourceError::StagingUnavailable),
        Err(StreamCopyError::LimitExceeded) => Err(GithubSourceError::ResponseTooLarge),
    }
}

#[derive(Clone, Copy, Debug)]
struct ArchiveLimits {
    max_entries: usize,
    max_total_file_bytes: u64,
    max_single_file_bytes: u64,
}

impl ArchiveLimits {
    const PRODUCTION: Self = Self {
        max_entries: MAX_ENTRIES,
        max_total_file_bytes: MAX_TOTAL_FILE_BYTES,
        max_single_file_bytes: MAX_SINGLE_FILE_BYTES,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArchiveEntryKind {
    Directory,
    File,
}

#[derive(Debug)]
struct ArchiveEntryPlan {
    index: usize,
    relative_path: PathBuf,
    kind: ArchiveEntryKind,
    declared_size: u64,
    permissions: u32,
}

fn preflight_archive(
    archive: &mut ZipArchive<File>,
    central_directory_file: &mut File,
    limits: ArchiveLimits,
) -> Result<Vec<ArchiveEntryPlan>, GithubSourceError> {
    let entry_count = preflight_central_directory(
        central_directory_file,
        archive.central_directory_start(),
        limits.max_entries,
    )?;
    if entry_count == 0 {
        return Err(GithubSourceError::InvalidArchiveRoot);
    }
    if archive.len() != entry_count {
        // zip crate 会按原始名称折叠完全重复的 central entry，数量不一致只能来自重复项。
        return Err(GithubSourceError::DuplicateArchivePath {
            path: "<duplicate-entry>".to_owned(),
        });
    }

    let mut top_level = None::<String>;
    let mut declared_total = 0_u64;
    let mut kinds = BTreeMap::<PathBuf, ArchiveEntryKind>::new();
    let mut plan = Vec::with_capacity(entry_count);
    for index in 0..entry_count {
        // raw 模式不会开始解压，因此所有路径、类型和声明大小都会在首次写出前检查。
        let entry = archive
            .by_index_raw(index)
            .map_err(|_| GithubSourceError::InvalidArchive)?;
        let entry_display = archive_entry_display(entry.name_raw());
        if entry.encrypted() {
            return Err(GithubSourceError::EncryptedArchiveEntry {
                path: entry_display,
            });
        }
        if !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(GithubSourceError::UnsupportedArchiveCompression {
                path: entry_display,
            });
        }
        let (components, name_is_directory) = strict_archive_components(entry.name_raw())?;
        let (entry_top_level, stripped_components) = components
            .split_first()
            .ok_or(GithubSourceError::InvalidArchiveRoot)?;
        if top_level
            .as_ref()
            .is_some_and(|expected| expected != entry_top_level)
        {
            return Err(GithubSourceError::InvalidArchiveRoot);
        }
        top_level.get_or_insert_with(|| entry_top_level.clone());

        let kind = archive_entry_kind(entry.unix_mode(), name_is_directory, &entry_display)?;
        if stripped_components.is_empty() && kind != ArchiveEntryKind::Directory {
            return Err(GithubSourceError::InvalidArchiveRoot);
        }
        if kind == ArchiveEntryKind::Directory && entry.size() != 0 {
            return Err(GithubSourceError::InvalidArchive);
        }
        if kind == ArchiveEntryKind::File {
            if entry.size() > limits.max_single_file_bytes {
                return Err(GithubSourceError::ArchiveFileSizeLimitExceeded {
                    path: entry_display,
                    limit: limits.max_single_file_bytes,
                    actual: entry.size(),
                });
            }
            declared_total = declared_total.checked_add(entry.size()).ok_or(
                GithubSourceError::ArchiveTotalSizeLimitExceeded {
                    limit: limits.max_total_file_bytes,
                    actual: u64::MAX,
                },
            )?;
            if declared_total > limits.max_total_file_bytes {
                return Err(GithubSourceError::ArchiveTotalSizeLimitExceeded {
                    limit: limits.max_total_file_bytes,
                    actual: declared_total,
                });
            }
        }

        let relative_path = stripped_components.iter().collect::<PathBuf>();
        if kinds.insert(relative_path.clone(), kind).is_some() {
            return Err(GithubSourceError::DuplicateArchivePath {
                path: archive_relative_display(&relative_path),
            });
        }
        plan.push(ArchiveEntryPlan {
            index,
            relative_path,
            kind,
            declared_size: entry.size(),
            permissions: archive_permissions(entry.unix_mode(), kind),
        });
    }

    // 文件不能同时充当后续条目的父目录；这类冲突也必须在创建目录前拒绝。
    for (path, kind) in &kinds {
        let mut ancestor = path.parent();
        while let Some(candidate) = ancestor {
            if kinds.get(candidate) == Some(&ArchiveEntryKind::File) {
                return Err(GithubSourceError::DuplicateArchivePath {
                    path: archive_relative_display(path),
                });
            }
            if candidate.as_os_str().is_empty() {
                break;
            }
            ancestor = candidate.parent();
        }
        if path.as_os_str().is_empty() && *kind != ArchiveEntryKind::Directory {
            return Err(GithubSourceError::InvalidArchiveRoot);
        }
    }
    Ok(plan)
}

/// `zip` 会折叠完全同名的 central entry；最小化遍历原始 header 才能可靠计数并拒绝重复路径。
fn preflight_central_directory(
    file: &mut File,
    central_directory_start: u64,
    max_entries: usize,
) -> Result<usize, GithubSourceError> {
    const CENTRAL_ENTRY_SIGNATURE: u32 = 0x0201_4b50;
    const CENTRAL_ENTRY_FIXED_BYTES_AFTER_SIGNATURE: usize = 42;
    const CENTRAL_END_SIGNATURES: [u32; 4] = [0x0605_4b50, 0x0606_4b50, 0x0706_4b50, 0x0505_4b50];

    file.seek(SeekFrom::Start(central_directory_start))
        .map_err(|_| GithubSourceError::InvalidArchive)?;
    let mut count = 0_usize;
    let mut top_level = None::<String>;
    let mut normalized_paths = BTreeMap::<PathBuf, ()>::new();
    loop {
        let mut signature_bytes = [0_u8; 4];
        file.read_exact(&mut signature_bytes)
            .map_err(|_| GithubSourceError::InvalidArchive)?;
        let signature = u32::from_le_bytes(signature_bytes);
        if signature != CENTRAL_ENTRY_SIGNATURE {
            if count == 0 || !CENTRAL_END_SIGNATURES.contains(&signature) {
                return Err(GithubSourceError::InvalidArchive);
            }
            return Ok(count);
        }

        count = count
            .checked_add(1)
            .ok_or(GithubSourceError::ArchiveEntryLimitExceeded {
                limit: max_entries,
                actual: usize::MAX,
            })?;
        if count > max_entries {
            return Err(GithubSourceError::ArchiveEntryLimitExceeded {
                limit: max_entries,
                actual: count,
            });
        }
        let mut fixed = [0_u8; CENTRAL_ENTRY_FIXED_BYTES_AFTER_SIGNATURE];
        file.read_exact(&mut fixed)
            .map_err(|_| GithubSourceError::InvalidArchive)?;
        // 这三个长度位于固定 central header 的末段；实际名称仍交给统一严格路径解析。
        let name_length = u16::from_le_bytes([fixed[24], fixed[25]]) as usize;
        let extra_length = u16::from_le_bytes([fixed[26], fixed[27]]) as u64;
        let comment_length = u16::from_le_bytes([fixed[28], fixed[29]]) as u64;
        let mut raw_name = vec![0_u8; name_length];
        file.read_exact(&mut raw_name)
            .map_err(|_| GithubSourceError::InvalidArchive)?;
        file.seek(SeekFrom::Current(
            extra_length.saturating_add(comment_length) as i64,
        ))
        .map_err(|_| GithubSourceError::InvalidArchive)?;

        let (components, _) = strict_archive_components(&raw_name)?;
        let (entry_top_level, stripped_components) = components
            .split_first()
            .ok_or(GithubSourceError::InvalidArchiveRoot)?;
        if top_level
            .as_ref()
            .is_some_and(|expected| expected != entry_top_level)
        {
            return Err(GithubSourceError::InvalidArchiveRoot);
        }
        top_level.get_or_insert_with(|| entry_top_level.clone());
        let relative_path = stripped_components.iter().collect::<PathBuf>();
        if normalized_paths.insert(relative_path.clone(), ()).is_some() {
            return Err(GithubSourceError::DuplicateArchivePath {
                path: archive_relative_display(&relative_path),
            });
        }
    }
}

fn strict_archive_components(raw_name: &[u8]) -> Result<(Vec<String>, bool), GithubSourceError> {
    let display = archive_entry_display(raw_name);
    if raw_name.is_empty()
        || raw_name.contains(&b'\0')
        || raw_name.contains(&b'\\')
        || raw_name.first() == Some(&b'/')
    {
        return Err(GithubSourceError::UnsafeArchivePath { path: display });
    }
    let name = std::str::from_utf8(raw_name)
        .map_err(|_| GithubSourceError::UnsafeArchivePath { path: display })?;
    if name.chars().any(char::is_control) {
        return Err(GithubSourceError::UnsafeArchivePath {
            path: archive_entry_display(raw_name),
        });
    }
    let name_is_directory = name.ends_with('/');
    let normalized_name = name.strip_suffix('/').unwrap_or(name);
    let components = normalized_name.split('/').collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| component.is_empty() || matches!(*component, "." | ".."))
        || components[0].as_bytes().get(1) == Some(&b':')
    {
        return Err(GithubSourceError::UnsafeArchivePath {
            path: archive_entry_display(raw_name),
        });
    }
    Ok((
        components.into_iter().map(str::to_owned).collect(),
        name_is_directory,
    ))
}

fn archive_entry_kind(
    unix_mode: Option<u32>,
    name_is_directory: bool,
    display: &str,
) -> Result<ArchiveEntryKind, GithubSourceError> {
    let declared_type = unix_mode.unwrap_or(0) & u32::from(libc::S_IFMT);
    if declared_type == u32::from(libc::S_IFLNK) {
        return Err(GithubSourceError::UnsupportedArchiveEntry {
            path: display.to_owned(),
        });
    }
    let expected_type = if name_is_directory {
        u32::from(libc::S_IFDIR)
    } else {
        u32::from(libc::S_IFREG)
    };
    if declared_type != 0 && declared_type != expected_type {
        return Err(GithubSourceError::UnsupportedArchiveEntry {
            path: display.to_owned(),
        });
    }
    Ok(if name_is_directory {
        ArchiveEntryKind::Directory
    } else {
        ArchiveEntryKind::File
    })
}

fn archive_permissions(unix_mode: Option<u32>, kind: ArchiveEntryKind) -> u32 {
    unix_mode.map(|mode| mode & 0o777).unwrap_or(match kind {
        ArchiveEntryKind::Directory => 0o755,
        ArchiveEntryKind::File => 0o644,
    })
}

fn extract_archive(
    archive: &mut ZipArchive<File>,
    plan: &[ArchiveEntryPlan],
    repository_root: &Path,
    limits: ArchiveLimits,
) -> Result<(), GithubSourceError> {
    fs::create_dir(repository_root).map_err(|_| GithubSourceError::StagingUnavailable)?;
    let mut directory_permissions = Vec::<(PathBuf, u32)>::new();
    let mut actual_total = 0_u64;
    for planned in plan {
        let destination = repository_root.join(&planned.relative_path);
        match planned.kind {
            ArchiveEntryKind::Directory => {
                fs::create_dir_all(&destination)
                    .map_err(|_| GithubSourceError::StagingUnavailable)?;
                directory_permissions.push((destination, planned.permissions));
            }
            ArchiveEntryKind::File => {
                let parent = destination
                    .parent()
                    .ok_or(GithubSourceError::InvalidArchive)?;
                fs::create_dir_all(parent).map_err(|_| GithubSourceError::StagingUnavailable)?;
                let mut destination_file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)
                    .map_err(|error| {
                        if error.kind() == std::io::ErrorKind::AlreadyExists {
                            GithubSourceError::InvalidArchive
                        } else {
                            GithubSourceError::StagingUnavailable
                        }
                    })?;
                let mut source = archive
                    .by_index(planned.index)
                    .map_err(|_| GithubSourceError::InvalidArchive)?;
                let mut actual_file = 0_u64;
                let mut buffer = [0_u8; STREAM_BUFFER_BYTES];
                loop {
                    let remaining_single = limits.max_single_file_bytes.saturating_sub(actual_file);
                    let remaining_total = limits.max_total_file_bytes.saturating_sub(actual_total);
                    let remaining_declared = planned.declared_size.saturating_sub(actual_file);
                    let remaining = remaining_single
                        .min(remaining_total)
                        .min(remaining_declared);
                    let probe = if remaining == 0 {
                        1
                    } else {
                        remaining.min(STREAM_BUFFER_BYTES as u64) as usize
                    };
                    let count = source
                        .read(&mut buffer[..probe])
                        .map_err(|_| GithubSourceError::InvalidArchive)?;
                    if count == 0 {
                        break;
                    }
                    let next_file = actual_file.saturating_add(count as u64);
                    let next_total = actual_total.saturating_add(count as u64);
                    if next_file > limits.max_single_file_bytes {
                        return Err(GithubSourceError::ArchiveFileSizeLimitExceeded {
                            path: archive_relative_display(&planned.relative_path),
                            limit: limits.max_single_file_bytes,
                            actual: next_file,
                        });
                    }
                    if next_total > limits.max_total_file_bytes {
                        return Err(GithubSourceError::ArchiveTotalSizeLimitExceeded {
                            limit: limits.max_total_file_bytes,
                            actual: next_total,
                        });
                    }
                    if next_file > planned.declared_size {
                        return Err(GithubSourceError::InvalidArchive);
                    }
                    destination_file
                        .write_all(&buffer[..count])
                        .map_err(|_| GithubSourceError::StagingUnavailable)?;
                    actual_file = next_file;
                    actual_total = next_total;
                }
                if actual_file != planned.declared_size {
                    return Err(GithubSourceError::InvalidArchive);
                }
                destination_file
                    .flush()
                    .map_err(|_| GithubSourceError::StagingUnavailable)?;
                fs::set_permissions(&destination, Permissions::from_mode(planned.permissions))
                    .map_err(|_| GithubSourceError::StagingUnavailable)?;
            }
        }
    }

    // 最后从深到浅恢复目录权限，避免只读父目录阻断仍在进行的安全展开。
    directory_permissions.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (directory, permissions) in directory_permissions {
        fs::set_permissions(directory, Permissions::from_mode(permissions))
            .map_err(|_| GithubSourceError::StagingUnavailable)?;
    }
    Ok(())
}

fn archive_entry_display(raw_name: &[u8]) -> String {
    match std::str::from_utf8(raw_name) {
        Ok(name) => format!("{name:?}"),
        Err(_) => format!("{raw_name:?}"),
    }
}

fn archive_relative_display(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        "<repository>".to_owned()
    } else {
        path.to_string_lossy().into_owned()
    }
}

fn normalize_temporary_error(
    error: &str,
    temporary_root: &Path,
    repository_root: &Path,
    repository_name: &str,
) -> String {
    let mut normalized = error.replace(repository_root.to_string_lossy().as_ref(), repository_name);
    if let Ok(original_repository_root) = temporary_root.join(repository_name).canonicalize() {
        normalized = normalized.replace(
            original_repository_root.to_string_lossy().as_ref(),
            repository_name,
        );
    }
    normalized = normalized.replace(temporary_root.to_string_lossy().as_ref(), "<temporary>");
    if let Ok(canonical_temporary_root) = temporary_root.canonicalize() {
        normalized = normalized.replace(
            canonical_temporary_root.to_string_lossy().as_ref(),
            "<temporary>",
        );
    }
    normalized
}

struct CatalogTempTree {
    path: PathBuf,
    cleaned: bool,
}

impl CatalogTempTree {
    fn create(staging_root: &Path) -> Result<Self, GithubSourceError> {
        fs::create_dir_all(staging_root).map_err(|_| GithubSourceError::StagingUnavailable)?;
        for _ in 0..8 {
            let path = staging_root.join(format!(".github-catalog-{}", Uuid::new_v4()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    let temporary = Self {
                        path,
                        cleaned: false,
                    };
                    fs::set_permissions(&temporary.path, Permissions::from_mode(0o700))
                        .map_err(|_| GithubSourceError::StagingUnavailable)?;
                    return Ok(temporary);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(GithubSourceError::StagingUnavailable),
            }
        }
        Err(GithubSourceError::StagingUnavailable)
    }

    fn cleanup(mut self) -> Result<(), GithubSourceError> {
        fs::remove_dir_all(&self.path).map_err(|_| GithubSourceError::StagingCleanupFailed)?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for CatalogTempTree {
    fn drop(&mut self) {
        if !self.cleaned {
            // 显式清理会报告失败；Drop 只负责异常返回路径上的最后一次尽力清理。
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(
    transport: &dyn SourceTransport,
    url: Url,
) -> Result<T, GithubSourceError> {
    let mut response = transport
        .get(SourceRequest {
            url,
            accept: GITHUB_ACCEPT.to_owned(),
        })
        .map_err(|_| GithubSourceError::Network)?;
    if !(200..300).contains(&response.status) {
        return Err(GithubSourceError::HttpStatus);
    }
    let bytes = read_limited(&mut response.body)?;
    serde_json::from_slice(&bytes).map_err(|_| GithubSourceError::InvalidResponse)
}

/// 读取时累计计数，不能因为错误 Content-Length 或 chunked body 绕过资源限制。
pub fn read_limited(body: &mut dyn Read) -> Result<Vec<u8>, GithubSourceError> {
    let mut output = Vec::new();
    match copy_with_limit(body, &mut output, MAX_RESPONSE_BYTES) {
        Ok(_) => Ok(output),
        Err(StreamCopyError::Read | StreamCopyError::Write) => {
            Err(GithubSourceError::InvalidResponse)
        }
        Err(StreamCopyError::LimitExceeded) => Err(GithubSourceError::ResponseTooLarge),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StreamCopyError {
    Read,
    Write,
    LimitExceeded,
}

fn copy_with_limit(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    limit: u64,
) -> Result<u64, StreamCopyError> {
    let mut copied = 0_u64;
    let mut buffer = [0_u8; STREAM_BUFFER_BYTES];
    loop {
        // 达到上限后只探测下一字节，不能用一个完整 chunk 越过固定接收边界。
        let remaining = limit.saturating_sub(copied);
        let probe = if remaining == 0 {
            1
        } else {
            remaining.min(STREAM_BUFFER_BYTES as u64) as usize
        };
        let count = reader
            .read(&mut buffer[..probe])
            .map_err(|_| StreamCopyError::Read)?;
        if count == 0 {
            return Ok(copied);
        }
        let next = copied.saturating_add(count as u64);
        if next > limit {
            return Err(StreamCopyError::LimitExceeded);
        }
        writer
            .write_all(&buffer[..count])
            .map_err(|_| StreamCopyError::Write)?;
        copied = next;
    }
}

fn parse_github_url(
    url: Url,
    explicit_ref: Option<String>,
) -> Result<ParsedGithubSource, GithubSourceError> {
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(GithubSourceError::UnsupportedInput);
    }
    let segments = url
        .path_segments()
        .ok_or(GithubSourceError::UnsupportedInput)?
        .filter(|segment| !segment.is_empty())
        .map(decode_url_path_segment)
        .collect::<Result<Vec<_>, _>>()?;
    if segments.len() < 2
        || !valid_name_segment(&segments[0])
        || !valid_repository_segment(&segments[1])
    {
        return Err(GithubSourceError::UnsupportedInput);
    }
    let repository = strip_git_suffix(&segments[1])?.to_owned();
    if segments.len() == 2 {
        return Ok(ParsedGithubSource {
            owner: segments[0].clone(),
            repository,
            tracked_ref: explicit_ref,
            member_hint: None,
        });
    }
    if segments.len() < 4 || !matches!(segments[2].as_str(), "tree" | "blob") {
        return Err(GithubSourceError::UnsupportedInput);
    }
    let kind = segments[2].as_str();
    let tail = segments[3..].iter().map(String::as_str).collect::<Vec<_>>();
    let (tracked_ref, member_segments) = split_ref_and_member(&tail, explicit_ref)?;
    if member_segments
        .iter()
        .any(|segment| !valid_path_segment(segment))
        || (kind == "blob"
            && (member_segments.is_empty() || member_segments.last() != Some(&"SKILL.md")))
    {
        return Err(GithubSourceError::UnsupportedInput);
    }
    if kind == "tree" && member_segments.is_empty() {
        return Ok(ParsedGithubSource {
            owner: segments[0].clone(),
            repository,
            tracked_ref: Some(tracked_ref),
            member_hint: None,
        });
    }
    let member_hint = if kind == "blob" {
        member_segments[..member_segments.len() - 1].join("/")
    } else {
        member_segments.join("/")
    };
    if member_hint.is_empty() && kind != "blob" {
        return Err(GithubSourceError::UnsupportedInput);
    }
    Ok(ParsedGithubSource {
        owner: segments[0].clone(),
        repository,
        tracked_ref: Some(tracked_ref),
        member_hint: Some(member_hint),
    })
}

fn split_ref_and_member<'a>(
    tail: &'a [&'a str],
    explicit_ref: Option<String>,
) -> Result<(String, &'a [&'a str]), GithubSourceError> {
    if let Some(explicit_ref) = explicit_ref {
        let ref_segments = explicit_ref.split('/').collect::<Vec<_>>();
        if tail.len() < ref_segments.len() || tail[..ref_segments.len()] != ref_segments[..] {
            return Err(GithubSourceError::RefConflict);
        }
        let member_start = ref_segments.len();
        return Ok((explicit_ref, &tail[member_start..]));
    }
    if !valid_path_segment(tail[0]) {
        return Err(GithubSourceError::UnsupportedInput);
    }
    Ok((tail[0].to_owned(), &tail[1..]))
}

fn api_url(segments: &[&str]) -> Result<Url, GithubSourceError> {
    let mut url =
        Url::parse("https://api.github.com/").map_err(|_| GithubSourceError::InvalidResponse)?;
    let mut path = url
        .path_segments_mut()
        .map_err(|_| GithubSourceError::InvalidResponse)?;
    path.pop_if_empty();
    for segment in segments {
        path.push(segment);
    }
    drop(path);
    Ok(url)
}

fn parse_full_name(full_name: &str) -> Result<(String, String), GithubSourceError> {
    let parts = full_name.split('/').collect::<Vec<_>>();
    if parts.len() != 2 || !valid_name_segment(parts[0]) || !valid_repository_segment(parts[1]) {
        return Err(GithubSourceError::InvalidResponse);
    }
    Ok((parts[0].to_owned(), parts[1].to_owned()))
}

fn validate_tracked_ref(reference: &str) -> Result<String, GithubSourceError> {
    if reference.is_empty()
        || reference
            .split('/')
            .any(|segment| !valid_path_segment(segment))
    {
        return Err(GithubSourceError::InvalidTrackedRef);
    }
    Ok(reference.to_owned())
}

fn valid_name_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_repository_segment(segment: &str) -> bool {
    strip_git_suffix(segment).is_ok_and(|repository| {
        !repository.is_empty()
            && repository
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            && repository != "."
            && repository != ".."
    })
}

fn strip_git_suffix(segment: &str) -> Result<&str, GithubSourceError> {
    let repository = segment.strip_suffix(".git").unwrap_or(segment);
    if repository.is_empty() || repository.contains(".git.git") {
        return Err(GithubSourceError::UnsupportedInput);
    }
    Ok(repository)
}

fn valid_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.contains(['/', '\\', '\0'])
        && !segment.chars().any(char::is_control)
}

/// `url` 保留 path segment 的百分号编码；这里只解码一次，禁止编码后的分隔符改变层级。
fn decode_url_path_segment(segment: &str) -> Result<String, GithubSourceError> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(GithubSourceError::UnsupportedInput);
        }
        let high = decode_hex_digit(bytes[index + 1])?;
        let low = decode_hex_digit(bytes[index + 2])?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).map_err(|_| GithubSourceError::UnsupportedInput)
}

fn decode_hex_digit(byte: u8) -> Result<u8, GithubSourceError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(GithubSourceError::UnsupportedInput),
    }
}

fn is_commit_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Deserialize)]
struct RepositoryMetadata {
    full_name: String,
    default_branch: String,
    private: bool,
}

#[derive(Deserialize)]
struct CommitMetadata {
    sha: String,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        io::{Cursor, Write},
        sync::Mutex,
    };

    use tempfile::{NamedTempFile, tempdir};
    use zip::{ZipWriter, write::SimpleFileOptions};

    use super::*;

    #[test]
    fn parses_supported_repository_forms() {
        let bare = parse_github_source("Owner/repo", None).expect("裸仓库应可解析");
        assert_eq!(bare.owner, "Owner");
        assert_eq!(bare.repository, "repo");
        assert_eq!(bare.tracked_ref, None);

        let root = parse_github_source("https://github.com/Owner/repo.git/", Some("main"))
            .expect("根 URL 应可解析");
        assert_eq!(root.tracked_ref.as_deref(), Some("main"));

        let tree = parse_github_source(
            "https://github.com/Owner/repo/tree/main/examples/demo",
            None,
        )
        .expect("tree URL 应可解析");
        assert_eq!(tree.tracked_ref.as_deref(), Some("main"));
        assert_eq!(tree.member_hint.as_deref(), Some("examples/demo"));

        let tree_root = parse_github_source("https://github.com/Owner/repo/tree/main", None)
            .expect("只含 ref 的 tree URL 应可解析");
        assert_eq!(tree_root.tracked_ref.as_deref(), Some("main"));
        assert_eq!(tree_root.member_hint, None);

        let blob = parse_github_source(
            "https://github.com/Owner/repo/blob/main/demo/SKILL.md",
            None,
        )
        .expect("成员 URL 应可解析");
        assert_eq!(blob.member_hint.as_deref(), Some("demo"));

        let encoded = parse_github_source(
            "https://github.com/Owner/repo/tree/%E5%8F%91%E5%B8%83/skills/my%20skill",
            None,
        )
        .expect("URL path 应严格解码一次");
        assert_eq!(encoded.tracked_ref.as_deref(), Some("发布"));
        assert_eq!(encoded.member_hint.as_deref(), Some("skills/my skill"));

        let root_blob =
            parse_github_source("https://github.com/Owner/repo/blob/main/SKILL.md", None)
                .expect("仓库根部的 SKILL.md 也是成员 URL");
        assert_eq!(root_blob.member_hint.as_deref(), Some(""));
    }

    #[test]
    fn explicit_slash_ref_separates_member_hint() {
        let parsed = parse_github_source(
            "https://github.com/Owner/repo/tree/feature/skill/demo",
            Some("feature/skill"),
        )
        .expect("显式斜杠 ref 应能消除 URL 歧义");
        assert_eq!(parsed.tracked_ref.as_deref(), Some("feature/skill"));
        assert_eq!(parsed.member_hint.as_deref(), Some("demo"));
    }

    #[test]
    fn rejects_conflicting_and_unsupported_inputs() {
        assert_eq!(
            parse_github_source(
                "https://github.com/Owner/repo/tree/main/demo",
                Some("release")
            ),
            Err(GithubSourceError::RefConflict)
        );
        for input in [
            "git@github.com:Owner/repo.git",
            "https://github.example.com/Owner/repo",
            "https://github.com/Owner/repo?tab=readme",
            "https://github.com/Owner/repo/tree/main/../demo",
            "https://github.com/Owner/repo/tree/main/demo%2Fchild",
            "https://github.com/Owner/repo/tree/main/demo%5Cchild",
            "https://github.com/Owner/repo/tree/main/%00",
            "https://github.com/Owner/repo/blob/main/demo/README.md",
        ] {
            assert_eq!(
                parse_github_source(input, None),
                Err(GithubSourceError::UnsupportedInput)
            );
        }
    }

    #[test]
    fn resolves_metadata_and_commit_through_transport_boundary() {
        let transport = FixtureTransport::new([
            fixture(
                200,
                r#"{"full_name":"Owner/Repo","default_branch":"main","private":false}"#,
            ),
            fixture(200, r#"{"sha":"0123456789abcdef0123456789abcdef01234567"}"#),
        ]);
        let resolved = resolve_github_source(&transport, "owner/repo", None)
            .expect("公开仓库应解析成固定 commit");
        assert_eq!(resolved.canonical_identity, "github:owner/repo");
        assert_eq!(resolved.display_name, "Owner/Repo");
        assert_eq!(resolved.tracked_ref, "main");
        assert_eq!(resolved.commit, "0123456789abcdef0123456789abcdef01234567");
        let requests = transport.requests.lock().expect("应读取请求记录");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].url.path().ends_with("/repos/owner/repo"));
        assert!(requests[1].url.path().ends_with("/commits/main"));
    }

    #[test]
    fn encodes_commit_ref_as_one_api_path_segment() {
        let url = api_url(&["repos", "owner", "repo", "commits", "feature/next"])
            .expect("API URL 应能建立");
        assert_eq!(
            url.as_str(),
            "https://api.github.com/repos/owner/repo/commits/feature%2Fnext"
        );
    }

    #[test]
    fn fetches_sha_archive_preserves_candidate_errors_and_cleans_staging() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let archive = zip_fixture(&[
            (
                "repository-sha/skills/alpha/SKILL.md",
                "---\nname: alpha\ndescription: alpha catalog skill\n---\n",
            ),
            (
                "repository-sha/skills/parent/SKILL.md",
                "---\nname: parent\ndescription: parent catalog skill\n---\n",
            ),
            (
                "repository-sha/skills/parent/child/SKILL.md",
                "---\nname: child\ndescription: child catalog skill\n---\n",
            ),
        ]);
        let transport = FixtureTransport::new([
            fixture(
                200,
                r#"{"full_name":"Owner/Repo","default_branch":"main","private":false}"#,
            ),
            fixture(200, &format!(r#"{{"sha":"{sha}"}}"#)),
            binary_fixture(200, archive),
        ]);
        let sandbox = tempdir().expect("应创建隔离目录");
        let staging = sandbox.path().join("catalog-staging");

        let fetched = fetch_github_catalog(
            &transport,
            GithubCatalogTarget {
                owner: "Owner",
                repository: "Repo",
                canonical_identity: "github:owner/repo",
                display_name: "Owner/Repo",
                tracked_ref: "main",
            },
            &staging,
        )
        .expect("有效 SHA archive 应返回 Catalog");

        assert_eq!(fetched.commit_sha, sha);
        assert_eq!(fetched.candidates.len(), 3);
        assert!(
            fetched
                .candidates
                .iter()
                .any(|candidate| candidate.name.as_deref() == Some("alpha")
                    && candidate.selectable())
        );
        let nested_errors = fetched
            .candidates
            .iter()
            .flat_map(|candidate| &candidate.validation_errors)
            .collect::<Vec<_>>();
        assert!(!nested_errors.is_empty(), "嵌套成员错误必须保留在 metadata");
        assert!(
            nested_errors
                .iter()
                .all(|error| !error.contains(staging.to_string_lossy().as_ref()))
        );
        assert!(
            nested_errors
                .iter()
                .any(|error| error.contains("Repo/skills"))
        );
        assert_eq!(
            fs::read_dir(&staging)
                .expect("staging 根应保留且可读")
                .count(),
            0,
            "成功后不能保留 archive 或展开树"
        );
        let requests = transport.requests.lock().expect("应读取请求记录");
        assert_eq!(requests.len(), 3);
        assert!(requests[2].url.path().ends_with(&format!("/zipball/{sha}")));
    }

    #[test]
    fn valid_archive_without_skill_metadata_is_an_empty_catalog() {
        let sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let transport = FixtureTransport::new([
            fixture(
                200,
                r#"{"full_name":"Owner/Repo","default_branch":"main","private":false}"#,
            ),
            fixture(200, &format!(r#"{{"sha":"{sha}"}}"#)),
            binary_fixture(
                200,
                zip_fixture(&[("repository-sha/README.md", "# repository\n")]),
            ),
        ]);
        let sandbox = tempdir().expect("应创建隔离目录");
        let staging = sandbox.path().join("catalog-staging");

        let fetched = fetch_github_catalog(
            &transport,
            GithubCatalogTarget {
                owner: "Owner",
                repository: "Repo",
                canonical_identity: "github:owner/repo",
                display_name: "Owner/Repo",
                tracked_ref: "main",
            },
            &staging,
        )
        .expect("没有 SKILL.md 的合法仓库仍是成功 Catalog");

        assert!(fetched.candidates.is_empty());
        assert_eq!(fs::read_dir(staging).expect("staging 应可读").count(), 0);
    }

    #[test]
    fn receive_limit_accepts_exact_bytes_and_rejects_the_next_byte() {
        let mut exact_source = Cursor::new(b"12345".to_vec());
        let mut exact_destination = Vec::new();
        assert_eq!(
            copy_with_limit(&mut exact_source, &mut exact_destination, 5),
            Ok(5)
        );
        assert_eq!(exact_destination, b"12345");

        let mut source = Cursor::new(b"123456".to_vec());
        let mut destination = Vec::new();
        assert_eq!(
            copy_with_limit(&mut source, &mut destination, 5),
            Err(StreamCopyError::LimitExceeded)
        );
        assert_eq!(destination, b"12345");
        assert_eq!(source.position(), 6);
    }

    #[test]
    fn strict_archive_paths_reject_every_forbidden_component_form() {
        for path in [
            b"/root/file".as_slice(),
            b"C:/root/file".as_slice(),
            b"root/../file".as_slice(),
            b"root/./file".as_slice(),
            b"root\\file".as_slice(),
            b"root/\0file".as_slice(),
        ] {
            assert!(matches!(
                strict_archive_components(path),
                Err(GithubSourceError::UnsafeArchivePath { .. })
            ));
        }
    }

    #[test]
    fn preflight_rejects_normalized_duplicates_encryption_and_special_types() {
        let duplicate = normalized_duplicate_zip_fixture();
        let (mut archive, mut central_directory) = open_fixture_archive(&duplicate);
        assert!(matches!(
            preflight_archive(
                &mut archive,
                &mut central_directory,
                test_archive_limits(3, 10, 10),
            ),
            Err(GithubSourceError::DuplicateArchivePath { .. })
        ));

        let encrypted = archive_with_central_mode_or_flag(Some(0x0001), None);
        let (mut archive, mut central_directory) = open_fixture_archive(&encrypted);
        assert!(matches!(
            preflight_archive(
                &mut archive,
                &mut central_directory,
                test_archive_limits(1, 10, 10),
            ),
            Err(GithubSourceError::EncryptedArchiveEntry { .. })
        ));

        for special_type in [libc::S_IFLNK, libc::S_IFIFO] {
            let special =
                archive_with_central_mode_or_flag(None, Some(u32::from(special_type) | 0o777));
            let (mut archive, mut central_directory) = open_fixture_archive(&special);
            assert!(matches!(
                preflight_archive(
                    &mut archive,
                    &mut central_directory,
                    test_archive_limits(1, 10, 10),
                ),
                Err(GithubSourceError::UnsupportedArchiveEntry { .. })
            ));
        }
    }

    #[test]
    fn archive_entry_limit_accepts_exact_count_and_rejects_the_next_entry() {
        let exact = zip_fixture(&[("root/one.txt", "1"), ("root/two.txt", "2")]);
        let limits = test_archive_limits(2, 10, 10);
        let (mut archive, mut central_directory) = open_fixture_archive(&exact);
        assert_eq!(
            preflight_archive(&mut archive, &mut central_directory, limits)
                .expect("恰好达到条目上限应成功")
                .len(),
            2
        );

        let over = zip_fixture(&[
            ("root/one.txt", "1"),
            ("root/two.txt", "2"),
            ("root/three.txt", "3"),
        ]);
        let (mut archive, mut central_directory) = open_fixture_archive(&over);
        assert_eq!(
            preflight_archive(&mut archive, &mut central_directory, limits)
                .expect_err("下一条目必须被拒绝"),
            GithubSourceError::ArchiveEntryLimitExceeded {
                limit: 2,
                actual: 3,
            }
        );
    }

    #[test]
    fn expanded_total_limit_accepts_exact_bytes_and_rejects_the_next_declared_and_actual_byte() {
        let exact = zip_fixture(&[("root/one.txt", "123"), ("root/two.txt", "456")]);
        let strict = test_archive_limits(2, 6, 4);
        let (mut archive, mut central_directory) = open_fixture_archive(&exact);
        let plan = preflight_archive(&mut archive, &mut central_directory, strict)
            .expect("恰好达到展开总量应成功");
        let sandbox = tempdir().expect("应创建隔离目录");
        extract_archive(&mut archive, &plan, &sandbox.path().join("Repo"), strict)
            .expect("实际写出恰好达到展开总量也应成功");

        let over = zip_fixture(&[("root/one.txt", "123"), ("root/two.txt", "4567")]);
        let (mut archive, mut central_directory) = open_fixture_archive(&over);
        assert_eq!(
            preflight_archive(&mut archive, &mut central_directory, strict)
                .expect_err("下一声明字节必须被拒绝"),
            GithubSourceError::ArchiveTotalSizeLimitExceeded {
                limit: 6,
                actual: 7,
            }
        );

        let relaxed = test_archive_limits(2, 7, 4);
        let (mut archive, mut central_directory) = open_fixture_archive(&over);
        let plan = preflight_archive(&mut archive, &mut central_directory, relaxed)
            .expect("宽松预检应建立测试计划");
        let sandbox = tempdir().expect("应创建隔离目录");
        assert_eq!(
            extract_archive(&mut archive, &plan, &sandbox.path().join("Repo"), strict,)
                .expect_err("实际展开的下一字节必须被拒绝"),
            GithubSourceError::ArchiveTotalSizeLimitExceeded {
                limit: 6,
                actual: 7,
            }
        );
    }

    #[test]
    fn single_file_limit_accepts_exact_bytes_and_rejects_the_next_declared_and_actual_byte() {
        let exact = zip_fixture(&[("root/file.txt", "12345")]);
        let strict = test_archive_limits(1, 10, 5);
        let (mut archive, mut central_directory) = open_fixture_archive(&exact);
        let plan = preflight_archive(&mut archive, &mut central_directory, strict)
            .expect("恰好达到单文件上限应成功");
        let sandbox = tempdir().expect("应创建隔离目录");
        extract_archive(&mut archive, &plan, &sandbox.path().join("Repo"), strict)
            .expect("实际写出恰好达到单文件上限也应成功");

        let over = zip_fixture(&[("root/file.txt", "123456")]);
        let (mut archive, mut central_directory) = open_fixture_archive(&over);
        assert_eq!(
            preflight_archive(&mut archive, &mut central_directory, strict)
                .expect_err("下一声明字节必须被拒绝"),
            GithubSourceError::ArchiveFileSizeLimitExceeded {
                path: "\"root/file.txt\"".to_owned(),
                limit: 5,
                actual: 6,
            }
        );

        let relaxed = test_archive_limits(1, 10, 6);
        let (mut archive, mut central_directory) = open_fixture_archive(&over);
        let plan = preflight_archive(&mut archive, &mut central_directory, relaxed)
            .expect("宽松预检应建立测试计划");
        let sandbox = tempdir().expect("应创建隔离目录");
        assert_eq!(
            extract_archive(&mut archive, &plan, &sandbox.path().join("Repo"), strict,)
                .expect_err("实际单文件的下一字节必须被拒绝"),
            GithubSourceError::ArchiveFileSizeLimitExceeded {
                path: "file.txt".to_owned(),
                limit: 5,
                actual: 6,
            }
        );
    }

    struct FixtureTransport {
        responses: Mutex<VecDeque<Result<SourceResponse, SourceTransportError>>>,
        requests: Mutex<Vec<SourceRequest>>,
    }

    impl FixtureTransport {
        fn new(
            responses: impl IntoIterator<Item = Result<SourceResponse, SourceTransportError>>,
        ) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl SourceTransport for FixtureTransport {
        fn get(&self, request: SourceRequest) -> Result<SourceResponse, SourceTransportError> {
            self.requests.lock().expect("应记录请求").push(request);
            self.responses
                .lock()
                .expect("应读取 fixture")
                .pop_front()
                .expect("fixture 应足够")
        }
    }

    fn fixture(status: u16, json: &str) -> Result<SourceResponse, SourceTransportError> {
        binary_fixture(status, json.as_bytes().to_vec())
    }

    fn binary_fixture(status: u16, body: Vec<u8>) -> Result<SourceResponse, SourceTransportError> {
        Ok(SourceResponse {
            status,
            final_url: Url::parse("https://api.github.com/fixture").expect("fixture URL 应合法"),
            body: Box::new(Cursor::new(body)),
        })
    }

    fn zip_fixture(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o644);
        for (path, contents) in entries {
            archive
                .start_file(*path, options)
                .expect("应创建 ZIP entry");
            archive
                .write_all(contents.as_bytes())
                .expect("应写入 ZIP 内容");
        }
        archive.finish().expect("应完成 ZIP fixture").into_inner()
    }

    fn normalized_duplicate_zip_fixture() -> Vec<u8> {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().unix_permissions(0o644);
        archive
            .start_file("root/path", options)
            .expect("应创建普通文件 entry");
        archive.write_all(b"x").expect("应写入普通文件");
        archive
            .add_directory("root/path", options)
            .expect("尾随斜杠形成不同原始名称");
        archive.finish().expect("应完成 ZIP fixture").into_inner()
    }

    fn archive_with_central_mode_or_flag(
        encrypted_flag: Option<u16>,
        unix_mode: Option<u32>,
    ) -> Vec<u8> {
        let mut bytes = zip_fixture(&[("root/file.txt", "x")]);
        let central = bytes
            .windows(4)
            .position(|window| window == [0x50, 0x4b, 0x01, 0x02])
            .expect("fixture 应包含 central entry");
        if let Some(flag) = encrypted_flag {
            bytes[6..8].copy_from_slice(&flag.to_le_bytes());
            bytes[central + 8..central + 10].copy_from_slice(&flag.to_le_bytes());
        }
        if let Some(mode) = unix_mode {
            bytes[central + 38..central + 42].copy_from_slice(&(mode << 16).to_le_bytes());
        }
        bytes
    }

    fn open_fixture_archive(bytes: &[u8]) -> (ZipArchive<File>, File) {
        let mut archive_file = NamedTempFile::new().expect("应创建临时 ZIP");
        archive_file.write_all(bytes).expect("应写入临时 ZIP");
        let archive_reader = archive_file.reopen().expect("应重新打开临时 ZIP");
        let central_directory_reader = archive_reader.try_clone().expect("应复制临时 ZIP 句柄");
        (
            ZipArchive::new(archive_reader).expect("fixture 必须是有效 ZIP"),
            central_directory_reader,
        )
    }

    fn test_archive_limits(
        max_entries: usize,
        max_total_file_bytes: u64,
        max_single_file_bytes: u64,
    ) -> ArchiveLimits {
        ArchiveLimits {
            max_entries,
            max_total_file_bytes,
            max_single_file_bytes,
        }
    }
}
