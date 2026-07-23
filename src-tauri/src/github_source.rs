//! GitHub public Source 的最外层网络协议与确定性输入解析。

use std::{io::Read, sync::Arc, time::Duration};

use reqwest::{
    blocking::{Client, Response},
    header::{ACCEPT, USER_AGENT},
    redirect::Policy,
};
use serde::Deserialize;
use thiserror::Error;
use url::Url;

/// 所有 GitHub 响应（包括 archive）都不能超过这个固定上限。
pub const MAX_RESPONSE_BYTES: u64 = 100 * 1024 * 1024;
const GITHUB_ACCEPT: &str = "application/vnd.github+json";
const GITHUB_USER_AGENT: &str = "SkillYard/1.0";
const GITHUB_TIMEOUT: Duration = Duration::from_secs(20);

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
    let mut buffer = [0_u8; 8192];
    loop {
        let count = body
            .read(&mut buffer)
            .map_err(|_| GithubSourceError::InvalidResponse)?;
        if count == 0 {
            return Ok(output);
        }
        let next = output.len() as u64 + count as u64;
        if next > MAX_RESPONSE_BYTES {
            return Err(GithubSourceError::ResponseTooLarge);
        }
        output.extend_from_slice(&buffer[..count]);
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
    use std::{collections::VecDeque, io::Cursor, sync::Mutex};

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
        Ok(SourceResponse {
            status,
            final_url: Url::parse("https://api.github.com/fixture").expect("fixture URL 应合法"),
            body: Box::new(Cursor::new(json.as_bytes().to_vec())),
        })
    }
}
