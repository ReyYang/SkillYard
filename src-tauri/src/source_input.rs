//! 非 GitHub Source 输入只负责准备受控快照，不创建第二套安装事务。

use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions, Permissions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::{
    content::{
        ContentValidationError, DiscoveredBundleCandidate, copy_skill_bundle_tree,
        validate_skill_bundle_folder,
    },
    github_source::{MAX_RESPONSE_BYTES, SourceRequest, SourceTransport, SourceTransportError},
    source_archive::{ArchiveWrapperPolicy, SourceArchiveError, extract_zip_archive},
};

const ARCHIVE_ACCEPT: &str = "application/zip";
const STREAM_BUFFER_BYTES: usize = 64 * 1024;
const TEMPORARY_ARCHIVE_NAME: &str = ".source-archive";

/// 三种输入在进入同一 Install Plan 前保留的最小来源类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreparedSourceKind {
    Archive,
    DirectUrl,
    EditableLocal,
}

/// Editable Local Source 的身份来自目录 inode，而不是可能变化的显示路径。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SourceFilesystemIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

/// 重新关联只读识别候选目录；这里不会复制或采用其中的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InspectedEditableLocalSource {
    pub(crate) canonical_path: PathBuf,
    pub(crate) display_name: String,
    pub(crate) marker: String,
    pub(crate) candidates: Vec<DiscoveredBundleCandidate>,
}

/// 已验证的 Plan 快照；未显式持久化时离开作用域会自动清理。
#[derive(Debug)]
pub(crate) struct PreparedSourceSnapshot {
    snapshot_root: PathBuf,
    content_root: PathBuf,
    kind: PreparedSourceKind,
    canonical_identity: String,
    display_name: String,
    locator: String,
    filesystem_identity: Option<SourceFilesystemIdentity>,
    marker: String,
    persisted: bool,
}

impl PreparedSourceSnapshot {
    fn create(
        staging_root: &Path,
        plan_id: &str,
        display_name: String,
        kind: PreparedSourceKind,
        canonical_identity: String,
        locator: String,
        filesystem_identity: Option<SourceFilesystemIdentity>,
    ) -> Result<Self, SourceInputError> {
        validate_plan_id(plan_id)?;
        fs::create_dir_all(staging_root).map_err(|_| SourceInputError::StagingUnavailable)?;
        let snapshot_root = staging_root.join(format!(".install-plan-{plan_id}"));
        fs::create_dir(&snapshot_root).map_err(|_| SourceInputError::StagingUnavailable)?;
        fs::set_permissions(&snapshot_root, Permissions::from_mode(0o700))
            .map_err(|_| SourceInputError::StagingUnavailable)?;
        Ok(Self {
            content_root: snapshot_root.join(&display_name),
            snapshot_root,
            kind,
            canonical_identity,
            display_name,
            locator,
            filesystem_identity,
            marker: String::new(),
            persisted: false,
        })
    }

    #[cfg(test)]
    pub(crate) fn snapshot_root(&self) -> &Path {
        &self.snapshot_root
    }

    pub(crate) fn content_root(&self) -> &Path {
        &self.content_root
    }

    pub(crate) fn kind(&self) -> PreparedSourceKind {
        self.kind
    }

    pub(crate) fn canonical_identity(&self) -> &str {
        &self.canonical_identity
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.display_name
    }

    pub(crate) fn locator(&self) -> &str {
        &self.locator
    }

    pub(crate) fn filesystem_identity(&self) -> Option<SourceFilesystemIdentity> {
        self.filesystem_identity
    }

    pub(crate) fn marker(&self) -> &str {
        &self.marker
    }

    /// 将快照交给现有 Plan 生命周期；之后 Drop 不再删除它。
    pub(crate) fn persist(mut self) -> PathBuf {
        self.persisted = true;
        self.snapshot_root.clone()
    }
}

impl Drop for PreparedSourceSnapshot {
    fn drop(&mut self) {
        if !self.persisted {
            // Drop 无法返回错误；正常失败路径仍会清理整个本次 Plan 隔离目录。
            let _ = fs::remove_dir_all(&self.snapshot_root);
        }
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub(crate) enum SourceInputError {
    #[error("Install Plan ID 不合法")]
    InvalidPlanId,
    #[error("本地 Source 归档必须是普通的 .zip 或 .skill 文件")]
    InvalidArchiveInput,
    #[error("Editable Local Source 必须是可读取的普通目录")]
    InvalidEditableLocal,
    #[error("Editable Local Source 当前不可访问或已不是登记时的目录")]
    EditableLocalUnavailable,
    #[error("直接下载 URL 必须是无账号信息和 fragment 的 HTTPS .zip 或 .skill 地址")]
    InvalidDirectUrl,
    #[error("直接下载 URL 重定向超出允许的 HTTPS host 边界")]
    RedirectBoundary,
    #[error("Source 下载失败")]
    Network,
    #[error("Source 下载返回了不支持的 HTTP 状态 {status}")]
    HttpStatus { status: u16 },
    #[error("Source 内容超过固定上限 {limit} bytes：已检测到 {actual} bytes")]
    ResponseTooLarge { limit: u64, actual: u64 },
    #[error("Source 内容读取失败")]
    SourceRead,
    #[error("Source 临时区不可用")]
    StagingUnavailable,
    #[error("{0}")]
    Archive(#[from] SourceArchiveError),
    #[error("Source 内容验证失败：{0}")]
    Content(String),
}

impl From<ContentValidationError> for SourceInputError {
    fn from(error: ContentValidationError) -> Self {
        Self::Content(error.to_string())
    }
}

impl From<SourceTransportError> for SourceInputError {
    fn from(_: SourceTransportError) -> Self {
        Self::Network
    }
}

/// 本地 ZIP 与 `.skill` 以 canonical path 作为 Source identity，以原始字节摘要作为 marker。
pub(crate) fn prepare_archive_source(
    path: &Path,
    staging_root: &Path,
    plan_id: &str,
) -> Result<PreparedSourceSnapshot, SourceInputError> {
    validate_plan_id(plan_id)?;
    let display_name = archive_display_name(path).ok_or(SourceInputError::InvalidArchiveInput)?;
    let (canonical_path, identity) = canonical_regular_archive(path)?;
    let locator = canonical_path
        .to_str()
        .ok_or(SourceInputError::InvalidArchiveInput)?
        .to_owned();
    let mut prepared = PreparedSourceSnapshot::create(
        staging_root,
        plan_id,
        display_name,
        PreparedSourceKind::Archive,
        format!("archive:{locator}"),
        locator,
        None,
    )?;
    let temporary_archive = prepared.snapshot_root.join(TEMPORARY_ARCHIVE_NAME);
    let mut input = open_unchanged_regular_file(&canonical_path, identity)?;
    let mut output = create_temporary_archive(&temporary_archive)?;
    let (_, marker) = copy_with_digest(&mut input, &mut output, MAX_RESPONSE_BYTES)?;
    output
        .flush()
        .and_then(|()| output.sync_all())
        .map_err(|_| SourceInputError::StagingUnavailable)?;
    ensure_file_identity(&canonical_path, &input, identity)?;
    expand_and_validate(&temporary_archive, &prepared.content_root)?;
    fs::remove_file(&temporary_archive).map_err(|_| SourceInputError::StagingUnavailable)?;
    prepared.marker = marker;
    Ok(prepared)
}

/// 确定性 HTTPS URL 只下载受支持归档，并复用与本地归档相同的展开核心。
pub(crate) fn prepare_direct_url_source(
    transport: &dyn SourceTransport,
    input: &str,
    staging_root: &Path,
    plan_id: &str,
) -> Result<PreparedSourceSnapshot, SourceInputError> {
    validate_plan_id(plan_id)?;
    let (url, canonical_identity) = canonical_direct_url_identity(input)?;
    let display_name =
        archive_display_name(Path::new(url.path())).ok_or(SourceInputError::InvalidDirectUrl)?;
    let locator = url.as_str().to_owned();
    let mut prepared = PreparedSourceSnapshot::create(
        staging_root,
        plan_id,
        display_name,
        PreparedSourceKind::DirectUrl,
        canonical_identity,
        locator,
        None,
    )?;
    let mut response = transport.get(SourceRequest {
        url: url.clone(),
        accept: ARCHIVE_ACCEPT.to_owned(),
    })?;
    validate_direct_archive_url(&response.final_url)?;
    if !same_download_boundary(&url, &response.final_url) {
        return Err(SourceInputError::RedirectBoundary);
    }
    if !(200..300).contains(&response.status) {
        return Err(SourceInputError::HttpStatus {
            status: response.status,
        });
    }

    let temporary_archive = prepared.snapshot_root.join(TEMPORARY_ARCHIVE_NAME);
    let mut output = create_temporary_archive(&temporary_archive)?;
    let (_, marker) = copy_with_digest(&mut response.body, &mut output, MAX_RESPONSE_BYTES)?;
    output
        .flush()
        .and_then(|()| output.sync_all())
        .map_err(|_| SourceInputError::StagingUnavailable)?;
    expand_and_validate(&temporary_archive, &prepared.content_root)?;
    fs::remove_file(&temporary_archive).map_err(|_| SourceInputError::StagingUnavailable)?;
    prepared.marker = marker;
    Ok(prepared)
}

/// 发现页只做格式判断；真正下载与归档校验仍必须进入既有 Install Plan。
pub(crate) fn canonical_direct_url_identity(
    input: &str,
) -> Result<(Url, String), SourceInputError> {
    let url = Url::parse(input.trim()).map_err(|_| SourceInputError::InvalidDirectUrl)?;
    validate_direct_archive_url(&url)?;
    let identity = format!("direct-url:{}", url.as_str());
    Ok((url, identity))
}

/// Editable Local Source 保留用户原目录，只把同一时刻的完整安全目录复制成 Plan 快照。
pub(crate) fn prepare_editable_local_source(
    path: &Path,
    staging_root: &Path,
    plan_id: &str,
) -> Result<PreparedSourceSnapshot, SourceInputError> {
    validate_plan_id(plan_id)?;
    let supplied =
        fs::symlink_metadata(path).map_err(|_| SourceInputError::InvalidEditableLocal)?;
    if supplied.file_type().is_symlink() || !supplied.is_dir() {
        return Err(SourceInputError::InvalidEditableLocal);
    }
    let canonical_path =
        fs::canonicalize(path).map_err(|_| SourceInputError::InvalidEditableLocal)?;
    let canonical_metadata = fs::symlink_metadata(&canonical_path)
        .map_err(|_| SourceInputError::InvalidEditableLocal)?;
    let identity = SourceFilesystemIdentity {
        device: canonical_metadata.dev(),
        inode: canonical_metadata.ino(),
    };
    if supplied.dev() != identity.device || supplied.ino() != identity.inode {
        return Err(SourceInputError::InvalidEditableLocal);
    }
    let display_name = directory_display_name(&canonical_path)?;
    let locator = canonical_path
        .to_str()
        .ok_or(SourceInputError::InvalidEditableLocal)?
        .to_owned();

    // 先要求至少存在可解释的 Skill 候选；完整复制随后会拒绝目录中的任意链接或特殊文件。
    validate_skill_bundle_folder(&canonical_path)?;
    let mut prepared = PreparedSourceSnapshot::create(
        staging_root,
        plan_id,
        display_name,
        PreparedSourceKind::EditableLocal,
        format!("editable-local:{}:{}", identity.device, identity.inode),
        locator,
        Some(identity),
    )?;
    let marker = copy_skill_bundle_tree(&canonical_path, &prepared.content_root)?;
    let copied = validate_skill_bundle_folder(&prepared.content_root)?;
    if copied.fingerprint != marker {
        return Err(SourceInputError::Content(
            "Editable Local Source 快照校验不一致".to_owned(),
        ));
    }
    prepared.marker = marker;
    Ok(prepared)
}

/// 已登记目录必须先匹配持久化的设备与 inode，不能因路径复用而静默改绑 Source。
pub(crate) fn prepare_registered_editable_local_source(
    path: &Path,
    expected_identity: SourceFilesystemIdentity,
    staging_root: &Path,
    plan_id: &str,
) -> Result<PreparedSourceSnapshot, SourceInputError> {
    let inspected = inspect_editable_local_source(path, expected_identity)?;
    if inspected.canonical_path != path {
        return Err(SourceInputError::EditableLocalUnavailable);
    }

    let prepared = prepare_editable_local_source(&inspected.canonical_path, staging_root, plan_id)
        .map_err(|error| {
            if error == SourceInputError::InvalidEditableLocal {
                SourceInputError::EditableLocalUnavailable
            } else {
                error
            }
        })?;
    if prepared.filesystem_identity() != Some(expected_identity) {
        return Err(SourceInputError::EditableLocalUnavailable);
    }
    Ok(prepared)
}

/// 候选路径必须仍是登记时的同一目录；名称或内容相似都不能替代 inode 证据。
pub(crate) fn inspect_editable_local_source(
    path: &Path,
    expected_identity: SourceFilesystemIdentity,
) -> Result<InspectedEditableLocalSource, SourceInputError> {
    let supplied =
        fs::symlink_metadata(path).map_err(|_| SourceInputError::EditableLocalUnavailable)?;
    if supplied.file_type().is_symlink()
        || !supplied.is_dir()
        || supplied.dev() != expected_identity.device
        || supplied.ino() != expected_identity.inode
    {
        return Err(SourceInputError::EditableLocalUnavailable);
    }
    let canonical =
        fs::canonicalize(path).map_err(|_| SourceInputError::EditableLocalUnavailable)?;
    let canonical_metadata =
        fs::symlink_metadata(&canonical).map_err(|_| SourceInputError::EditableLocalUnavailable)?;
    if canonical_metadata.dev() != expected_identity.device
        || canonical_metadata.ino() != expected_identity.inode
    {
        return Err(SourceInputError::EditableLocalUnavailable);
    }
    let validated = validate_skill_bundle_folder(&canonical)?;
    Ok(InspectedEditableLocalSource {
        display_name: directory_display_name(&canonical)?,
        canonical_path: canonical,
        marker: validated.fingerprint,
        candidates: validated.candidates,
    })
}

fn expand_and_validate(archive_path: &Path, content_root: &Path) -> Result<(), SourceInputError> {
    extract_zip_archive(
        archive_path,
        content_root,
        ArchiveWrapperPolicy::OptionalCommonWrapper,
    )?;
    validate_skill_bundle_folder(content_root)?;
    Ok(())
}

fn canonical_regular_archive(path: &Path) -> Result<(PathBuf, FileIdentity), SourceInputError> {
    if archive_display_name(path).is_none() {
        return Err(SourceInputError::InvalidArchiveInput);
    }
    let supplied = fs::symlink_metadata(path).map_err(|_| SourceInputError::InvalidArchiveInput)?;
    if supplied.file_type().is_symlink() || !supplied.is_file() {
        return Err(SourceInputError::InvalidArchiveInput);
    }
    let canonical = fs::canonicalize(path).map_err(|_| SourceInputError::InvalidArchiveInput)?;
    let canonical_metadata =
        fs::symlink_metadata(&canonical).map_err(|_| SourceInputError::InvalidArchiveInput)?;
    let identity = FileIdentity::from_metadata(&canonical_metadata);
    if FileIdentity::from_metadata(&supplied) != identity {
        return Err(SourceInputError::InvalidArchiveInput);
    }
    Ok((canonical, identity))
}

fn open_unchanged_regular_file(
    path: &Path,
    expected: FileIdentity,
) -> Result<File, SourceInputError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| SourceInputError::SourceRead)?;
    let metadata = file.metadata().map_err(|_| SourceInputError::SourceRead)?;
    if !metadata.is_file() || FileIdentity::from_metadata(&metadata) != expected {
        return Err(SourceInputError::SourceRead);
    }
    Ok(file)
}

fn ensure_file_identity(
    path: &Path,
    opened: &File,
    expected: FileIdentity,
) -> Result<(), SourceInputError> {
    let opened_metadata = opened
        .metadata()
        .map_err(|_| SourceInputError::SourceRead)?;
    let visible_metadata = fs::symlink_metadata(path).map_err(|_| SourceInputError::SourceRead)?;
    if visible_metadata.file_type().is_symlink()
        || FileIdentity::from_metadata(&opened_metadata) != expected
        || FileIdentity::from_metadata(&visible_metadata) != expected
    {
        return Err(SourceInputError::SourceRead);
    }
    Ok(())
}

fn create_temporary_archive(path: &Path) -> Result<File, SourceInputError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| SourceInputError::StagingUnavailable)
}

fn copy_with_digest(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    limit: u64,
) -> Result<(u64, String), SourceInputError> {
    let mut received = 0_u64;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; STREAM_BUFFER_BYTES];
    loop {
        let remaining = limit.saturating_sub(received);
        let probe = if remaining == 0 {
            1
        } else {
            remaining.min(STREAM_BUFFER_BYTES as u64) as usize
        };
        let count = reader
            .read(&mut buffer[..probe])
            .map_err(|_| SourceInputError::SourceRead)?;
        if count == 0 {
            break;
        }
        let next = received.saturating_add(count as u64);
        if next > limit {
            return Err(SourceInputError::ResponseTooLarge {
                limit,
                actual: next,
            });
        }
        writer
            .write_all(&buffer[..count])
            .map_err(|_| SourceInputError::StagingUnavailable)?;
        hasher.update(&buffer[..count]);
        received = next;
    }
    Ok((received, hex_digest(hasher.finalize())))
}

fn validate_plan_id(plan_id: &str) -> Result<(), SourceInputError> {
    let parsed = Uuid::parse_str(plan_id).map_err(|_| SourceInputError::InvalidPlanId)?;
    if parsed.hyphenated().to_string() != plan_id {
        return Err(SourceInputError::InvalidPlanId);
    }
    Ok(())
}

fn archive_display_name(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?;
    if !matches!(extension.to_ascii_lowercase().as_str(), "zip" | "skill") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?.trim();
    if stem.is_empty()
        || matches!(stem, "." | "..")
        || stem.starts_with('.')
        || stem.chars().any(char::is_control)
    {
        return None;
    }
    Some(stem.to_owned())
}

fn directory_display_name(path: &Path) -> Result<String, SourceInputError> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .ok_or(SourceInputError::InvalidEditableLocal)?;
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.starts_with('.')
        || name.chars().any(char::is_control)
    {
        return Err(SourceInputError::InvalidEditableLocal);
    }
    Ok(name.to_owned())
}

fn validate_direct_archive_url(url: &Url) -> Result<(), SourceInputError> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
        || archive_display_name(Path::new(url.path())).is_none()
    {
        return Err(SourceInputError::InvalidDirectUrl);
    }
    Ok(())
}

fn same_download_boundary(initial: &Url, final_url: &Url) -> bool {
    if initial.scheme() != "https" || final_url.scheme() != "https" {
        return false;
    }
    if is_github_host(initial.host_str()) {
        return is_github_host(final_url.host_str());
    }
    initial.host_str() == final_url.host_str()
        && initial.port_or_known_default() == final_url.port_or_known_default()
}

fn is_github_host(host: Option<&str>) -> bool {
    matches!(
        host,
        Some("api.github.com" | "github.com" | "codeload.github.com")
    )
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let bytes = digest.as_ref();
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut result, "{byte:02x}").expect("写入 String 不会失败");
    }
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl FileIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        io::{Cursor, Error, ErrorKind, Read, Write},
        os::unix::fs::symlink,
        sync::Mutex,
    };

    use tempfile::tempdir;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use super::*;
    use crate::github_source::SourceResponse;

    #[test]
    fn local_skill_archive_prepares_a_deterministic_snapshot_and_cleans_on_drop() {
        let sandbox = tempdir().expect("应创建隔离目录");
        let archive = sandbox.path().join("bundle.skill");
        let bytes = zip_fixture(&[(
            "wrapper/SKILL.md",
            "---\nname: bundle\ndescription: 本地归档 Skill\n---\n",
        )]);
        fs::write(&archive, &bytes).expect("应写入 .skill fixture");
        let staging = sandbox.path().join("staging");
        let plan_id = "11111111-1111-4111-8111-111111111111";

        let prepared = prepare_archive_source(&archive, &staging, plan_id)
            .expect("合法 .skill 应生成受控快照");
        let snapshot_root = prepared.snapshot_root().to_owned();

        assert_eq!(prepared.kind(), PreparedSourceKind::Archive);
        assert_eq!(prepared.display_name(), "bundle");
        assert_eq!(prepared.marker(), sha256(&bytes));
        assert_eq!(
            fs::read_to_string(prepared.content_root().join("SKILL.md")).expect("wrapper 应被剥离"),
            "---\nname: bundle\ndescription: 本地归档 Skill\n---\n"
        );
        assert!(!snapshot_root.join(TEMPORARY_ARCHIVE_NAME).exists());
        drop(prepared);
        assert!(!snapshot_root.exists(), "未交给 Plan 的快照必须自动清理");
        assert_eq!(fs::read(&archive).expect("原始归档应保持不变"), bytes);
    }

    #[test]
    fn local_archive_input_rejects_a_symlink_before_creating_staging() {
        let sandbox = tempdir().expect("应创建隔离目录");
        let archive = sandbox.path().join("original.zip");
        fs::write(&archive, zip_fixture(&[("README.md", "fixture")])).expect("应写入原始归档");
        let linked = sandbox.path().join("linked.zip");
        symlink(&archive, &linked).expect("应创建归档软链接");
        let staging = sandbox.path().join("staging");

        assert!(matches!(
            prepare_archive_source(&linked, &staging, "77777777-7777-4777-8777-777777777777",),
            Err(SourceInputError::InvalidArchiveInput)
        ));
        assert!(!staging.exists());
    }

    #[test]
    fn direct_url_uses_the_same_archive_snapshot_and_preserves_query_identity() {
        let bytes = zip_fixture(&[(
            "SKILL.md",
            "---\nname: remote\ndescription: 远程归档 Skill\n---\n",
        )]);
        let transport = FixtureTransport::new([Ok(SourceResponse {
            status: 200,
            final_url: Url::parse(
                "https://downloads.example.com/remote.zip?fixture_query=preserve-identity",
            )
            .expect("fixture URL 应合法"),
            body: Box::new(Cursor::new(bytes.clone())),
        })]);
        let sandbox = tempdir().expect("应创建隔离目录");
        let prepared = prepare_direct_url_source(
            &transport,
            "https://downloads.example.com/remote.zip?fixture_query=preserve-identity",
            &sandbox.path().join("staging"),
            "22222222-2222-4222-8222-222222222222",
        )
        .expect("同 host HTTPS 归档应生成快照");

        assert_eq!(prepared.kind(), PreparedSourceKind::DirectUrl);
        assert_eq!(
            prepared.canonical_identity(),
            "direct-url:https://downloads.example.com/remote.zip?fixture_query=preserve-identity"
        );
        assert_eq!(
            prepared.locator(),
            "https://downloads.example.com/remote.zip?fixture_query=preserve-identity"
        );
        assert_eq!(prepared.marker(), sha256(&bytes));
        let requests = transport.requests.lock().expect("应读取请求");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].accept, ARCHIVE_ACCEPT);
        drop(requests);
        let snapshot_root = prepared.snapshot_root().to_owned();
        assert_eq!(prepared.persist(), snapshot_root);
        assert!(snapshot_root.exists(), "交给 Plan 后快照必须继续存在");
    }

    #[test]
    fn direct_url_rejects_invalid_input_and_cross_host_final_url_without_leaving_staging() {
        let sandbox = tempdir().expect("应创建隔离目录");
        let staging = sandbox.path().join("staging");
        let empty = FixtureTransport::new([]);
        assert!(matches!(
            prepare_direct_url_source(
                &empty,
                "http://downloads.example.com/remote.zip",
                &staging,
                "33333333-3333-4333-8333-333333333333",
            ),
            Err(SourceInputError::InvalidDirectUrl)
        ));
        assert!(!staging.exists());

        let transport = FixtureTransport::new([Ok(SourceResponse {
            status: 200,
            final_url: Url::parse("https://cdn.example.com/remote.zip")
                .expect("fixture URL 应合法"),
            body: Box::new(Cursor::new(Vec::new())),
        })]);
        assert!(matches!(
            prepare_direct_url_source(
                &transport,
                "https://downloads.example.com/remote.zip",
                &staging,
                "44444444-4444-4444-8444-444444444444",
            ),
            Err(SourceInputError::RedirectBoundary)
        ));
        assert_eq!(
            fs::read_dir(&staging).expect("staging 根应存在").count(),
            0,
            "失败不能保留部分 Plan 快照"
        );
    }

    #[test]
    fn editable_local_source_copies_the_complete_bundle_but_keeps_the_original_owned_by_user() {
        let sandbox = tempdir().expect("应创建隔离目录");
        let source = sandbox.path().join("editable");
        let member = source.join("skills/alpha");
        fs::create_dir_all(&member).expect("应创建本地成员");
        fs::write(
            member.join("SKILL.md"),
            "---\nname: alpha\ndescription: 可编辑 Skill\n---\n",
        )
        .expect("应写入 metadata");
        fs::write(source.join("README.md"), "用户仓库文件").expect("应写入仓库文件");
        let prepared = prepare_editable_local_source(
            &source,
            &sandbox.path().join("staging"),
            "55555555-5555-4555-8555-555555555555",
        )
        .expect("安全本地目录应生成完整快照");

        assert_eq!(prepared.kind(), PreparedSourceKind::EditableLocal);
        assert_eq!(
            fs::read_to_string(prepared.content_root().join("README.md"))
                .expect("完整 Bundle 文件应保留"),
            "用户仓库文件"
        );
        assert!(source.join("skills/alpha/SKILL.md").is_file());
        let identity = prepared.filesystem_identity().expect("应保存目录身份");
        assert_eq!(
            prepared.canonical_identity(),
            format!("editable-local:{}:{}", identity.device, identity.inode)
        );
    }

    #[test]
    fn editable_local_source_rejects_internal_symlinks_without_a_partial_snapshot() {
        let sandbox = tempdir().expect("应创建隔离目录");
        let source = sandbox.path().join("editable");
        fs::create_dir(&source).expect("应创建来源目录");
        fs::write(
            source.join("SKILL.md"),
            "---\nname: editable\ndescription: 可编辑 Skill\n---\n",
        )
        .expect("应写入 metadata");
        symlink("SKILL.md", source.join("linked.md")).expect("应创建软链接");
        let staging = sandbox.path().join("staging");

        let error = prepare_editable_local_source(
            &source,
            &staging,
            "66666666-6666-4666-8666-666666666666",
        )
        .expect_err("内部软链接必须被拒绝");

        assert!(matches!(error, SourceInputError::Content(_)));
        assert_eq!(fs::read_dir(staging).expect("staging 应存在").count(), 0);
    }

    #[test]
    fn streaming_digest_reports_the_first_byte_over_the_limit_and_read_failures() {
        let mut input = Cursor::new(b"123456".to_vec());
        let mut output = Vec::new();
        assert!(matches!(
            copy_with_digest(&mut input, &mut output, 5),
            Err(SourceInputError::ResponseTooLarge {
                limit: 5,
                actual: 6,
            })
        ));
        assert_eq!(output, b"12345");

        let mut interrupted = InterruptedReader;
        assert!(matches!(
            copy_with_digest(&mut interrupted, &mut Vec::new(), 5),
            Err(SourceInputError::SourceRead)
        ));
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

    struct InterruptedReader;

    impl Read for InterruptedReader {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(Error::new(ErrorKind::UnexpectedEof, "模拟断流"))
        }
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

    fn sha256(bytes: &[u8]) -> String {
        hex_digest(Sha256::digest(bytes))
    }
}
