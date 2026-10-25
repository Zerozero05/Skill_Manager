use crate::network::{self, CachePolicy, Loaded};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{blocking::Client, Method, StatusCode};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::{cell::RefCell, collections::BTreeMap, io::Read, time::Duration};
use url::Url;

pub const MAX_FILE: usize = 4 * 1024 * 1024;
pub const MAX_TOTAL: usize = 32 * 1024 * 1024;
const MAX_RESPONSE: usize = 12 * 1024 * 1024;
const SERVICE: &str = "io.github.skill-manager.github";

// Only the current test thread and its synthetic repository can use this seam.
// Release builds retain the fixed GitHub endpoint and OS credential lookup.
#[cfg(test)]
thread_local! {
    static TEST_TRANSPORT: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(super) struct TestTransportGuard {
    previous: Option<(String, String)>,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(test)]
impl Drop for TestTransportGuard {
    fn drop(&mut self) {
        TEST_TRANSPORT.with(|transport| *transport.borrow_mut() = self.previous.take());
    }
}

#[cfg(test)]
pub(super) fn test_transport(repo: &str, endpoint: &str) -> TestTransportGuard {
    let url = Url::parse(endpoint).expect("test loopback URL");
    assert_eq!(url.scheme(), "http");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert!(repo.starts_with("synthetic-protocol/"));
    let previous =
        TEST_TRANSPORT.with(|transport| transport.replace(Some((repo.into(), endpoint.into()))));
    TestTransportGuard {
        previous,
        _thread_bound: std::marker::PhantomData,
    }
}

#[cfg(test)]
fn test_endpoint(repo: &str) -> Option<String> {
    TEST_TRANSPORT.with(|transport| {
        transport
            .borrow()
            .as_ref()
            .filter(|(test_repo, _)| test_repo == repo)
            .map(|(_, endpoint)| endpoint.clone())
    })
}

#[cfg(test)]
#[test]
fn synthetic_transport_override_is_thread_local_nested_and_restored_on_unwind() {
    let repo = "synthetic-protocol/scoped-a";
    assert!(test_endpoint(repo).is_none());
    {
        let _outer = test_transport(repo, "http://127.0.0.1:1/");
        assert_eq!(test_endpoint(repo).as_deref(), Some("http://127.0.0.1:1/"));
        assert!(std::thread::spawn(move || test_endpoint(repo))
            .join()
            .unwrap()
            .is_none());
        let result = std::panic::catch_unwind(|| {
            let _inner = test_transport("synthetic-protocol/scoped-b", "http://127.0.0.1:2/");
            assert!(test_endpoint(repo).is_none());
            panic!("synthetic guard unwind");
        });
        assert!(result.is_err());
        assert_eq!(test_endpoint(repo).as_deref(), Some("http://127.0.0.1:1/"));
    }
    assert!(test_endpoint(repo).is_none());
}

fn optional_lookup_miss(optional: bool, commit_lookup: bool, status: StatusCode) -> bool {
    optional
        && (status == StatusCode::NOT_FOUND
            || (commit_lookup && status == StatusCode::UNPROCESSABLE_ENTITY))
}

fn retry_wait() -> Result<(), String> {
    crate::tasks::report_progress("网络读取暂未成功，正在重试", None, None);
    for _ in 0..10 {
        network::check_cancelled()?;
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
fn response_error(response: &reqwest::blocking::Response) -> String {
    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        network::invalidate_credentials();
    }
    let remaining = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok());
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if status == 429 || status == 403 && (remaining == Some("0") || retry_after.is_some()) {
        let reset = response
            .headers()
            .get("x-ratelimit-reset")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(|reset| reset.saturating_sub(chrono::Utc::now().timestamp().max(0) as u64));
        let wait = retry_after.or(reset).unwrap_or(60).max(1);
        network::pause_api(Duration::from_secs(wait));
        return format!("GitHub 请求暂缓：服务器限流，约 {wait} 秒后重试");
    }
    match status {
        401 => "GitHub authentication failed. Update the token in Settings".into(),
        403 => "GitHub permission denied this request".into(),
        404 => "GitHub repository, version or file is unavailable (private repositories require access)".into(),
        409 | 422 => "GitHub rejected this change. The branch may have advanced or be protected; refresh and preview again".into(),
        code => format!("GitHub returned HTTP {code}"),
    }
}

/// Git hashes bytes with their blob header; this does not normalize line endings.
/// Used for immutable-tree comparisons, never instead of upload cleanup readback.
pub(crate) fn blob_hash(bytes: &[u8]) -> String {
    let mut digest = Sha1::new();
    digest.update(format!("blob {}\0", bytes.len()).as_bytes());
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

#[cfg(all(test, windows))]
#[test]
fn windows_secure_credential_roundtrip_uses_only_a_unique_test_entry() {
    let repo = format!("skill-manager-tests/{}", uuid::Uuid::new_v4().simple());
    struct Guard(String);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = save_credential(&self.0, None);
        }
    }
    assert!(credential(&repo).unwrap().is_none());
    let _guard = Guard(repo.clone());
    let value = "synthetic-test-value-with-no-service-access";
    save_credential(&repo, Some(value)).unwrap();
    assert_eq!(credential(&repo).unwrap().as_deref(), Some(value));
    save_credential(&repo, None).unwrap();
    assert!(credential(&repo).unwrap().is_none());
}

#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub repo: String,
    pub route: Vec<String>,
}

pub fn decode_component(s: &str) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("Invalid URL encoding".into());
            }
            let h = (bytes[i + 1] as char)
                .to_digit(16)
                .ok_or("Invalid URL encoding")?;
            let l = (bytes[i + 2] as char)
                .to_digit(16)
                .ok_or("Invalid URL encoding")?;
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "URL must contain UTF-8 paths".into())
}

pub fn parse_location(input: &str) -> Result<Location, String> {
    let input = input.trim();
    let parts: Vec<String> = if input.contains("://") {
        let u = Url::parse(input).map_err(|_| "Invalid GitHub URL")?;
        if u.scheme() != "https"
            || u.host_str() != Some("github.com")
            || !u.username().is_empty()
            || u.password().is_some()
            || u.port().is_some()
        {
            return Err("Use an HTTPS github.com repository or directory URL".into());
        }
        u.path_segments()
            .ok_or("Invalid GitHub path")?
            .filter(|s| !s.is_empty())
            .map(decode_component)
            .collect::<Result<_, _>>()?
    } else {
        input
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };
    if parts.len() < 2 {
        return Err("Expected owner/repository or a GitHub URL".into());
    }
    let owner = &parts[0];
    let name = parts[1].strip_suffix(".git").unwrap_or(&parts[1]);
    if [owner.as_str(), name].iter().any(|v| {
        v.is_empty()
            || *v == "."
            || *v == ".."
            || !v
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    }) {
        return Err("Invalid GitHub repository name".into());
    }
    let route = parts[2..].to_vec();
    if !route.is_empty() && (route.len() < 2 || !["tree", "blob"].contains(&route[0].as_str())) {
        return Err("Use a repository URL or its tree/directory URL".into());
    }
    Ok(Location {
        repo: format!("{owner}/{name}"),
        route,
    })
}

pub fn validate_relative(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > 2000 || path.contains('\\') || path.starts_with('/') {
        return Err("Expected a safe relative file path".into());
    }
    for component in path.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.ends_with(['.', ' '])
            || component
                .chars()
                .any(|c| c.is_control() || ":*?\"<>|".contains(c))
        {
            return Err(format!("Unsafe file path: {path}"));
        }
        let stem = component
            .split('.')
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if [".GIT", "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
            || component.eq_ignore_ascii_case(".git")
            || matches!(
                stem.strip_prefix("COM")
                    .or_else(|| stem.strip_prefix("LPT")),
                Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
            )
        {
            return Err(format!("Unsupported Windows path: {path}"));
        }
    }
    Ok(())
}

pub fn credential(repo: &str) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        let e = keyring::Entry::new(SERVICE, &repo.to_ascii_lowercase())
            .map_err(|_| "Cannot open Windows Credential Manager")?;
        match e.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => {
                Err("Cannot read the GitHub credential from Windows Credential Manager".into())
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (repo, SERVICE);
        Ok(None)
    }
}

pub fn save_credential(repo: &str, token: Option<&str>) -> Result<(), String> {
    if let Some(t) = token {
        if t.trim().is_empty() || t.len() > 4096 || t.chars().any(char::is_control) {
            return Err("Invalid GitHub token".into());
        }
    }
    #[cfg(windows)]
    {
        let e = keyring::Entry::new(SERVICE, &repo.to_ascii_lowercase())
            .map_err(|_| "Cannot open Windows Credential Manager")?;
        match token {
            Some(t) => e.set_password(t).map_err(|_| {
                "Cannot save the GitHub credential in Windows Credential Manager".into()
            }),
            None => match e.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err(
                    "Cannot remove the GitHub credential from Windows Credential Manager".into(),
                ),
            },
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (repo, token);
        Err("Secure credential storage is currently supported on Windows".into())
    }
}

pub struct Api {
    client: Client,
    token: Option<String>,
    pub repo: String,
    generation: u64,
}
impl Api {
    pub fn new(repo: &str) -> Result<Self, String> {
        let repo = parse_location(repo)?.repo;
        let (client, generation) = network::client()?;
        #[cfg(test)]
        let client = if test_endpoint(&repo).is_some() {
            Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "Cannot initialize test connection")?
        } else {
            client
        };
        #[cfg(test)]
        let token = if test_endpoint(&repo).is_some() {
            Some("synthetic-protocol-fixture".into())
        } else {
            credential(&repo)?
        };
        #[cfg(not(test))]
        let token = credential(&repo)?;
        Ok(Self {
            token,
            client,
            repo,
            generation,
        })
    }
    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }
    pub fn request(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&Value>,
        optional: bool,
    ) -> Result<Option<Value>, String> {
        self.request_mode(method, segments, body, optional, false)
    }
    fn request_mode(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&Value>,
        optional: bool,
        fresh: bool,
    ) -> Result<Option<Value>, String> {
        #[cfg(test)]
        let override_url = test_endpoint(&self.repo);
        #[cfg(test)]
        let base_url = override_url.as_deref().unwrap_or("https://api.github.com/");
        #[cfg(not(test))]
        let base_url = "https://api.github.com/";
        let mut u = Url::parse(base_url).expect("constant or validated test URL");
        {
            let mut p = u.path_segments_mut().map_err(|_| "Invalid API URL")?;
            p.push("repos");
            for s in self.repo.split('/') {
                p.push(s);
            }
            for s in segments {
                p.push(s);
            }
        }
        if segments.first() == Some(&"git")
            && segments.get(1) == Some(&"trees")
            && method == Method::GET
        {
            u.query_pairs_mut().append_pair("recursive", "1");
        }
        let commit_lookup = method == Method::GET && segments.first() == Some(&"commits");
        let load = |etag: Option<&str>| -> Result<Loaded, String> {
            for attempt in 0..2 {
                crate::tasks::report_progress("等待 GitHub 访问", None, None);
                let _permit = network::acquire(false, method == Method::GET)?;
                let stage = if method != Method::GET {
                    "提交 GitHub 更改"
                } else if segments.get(1) == Some(&"trees") {
                    "读取 GitHub 文件树"
                } else if segments.get(1) == Some(&"blobs") {
                    "读取 GitHub 文件"
                } else if commit_lookup || segments.get(1) == Some(&"ref") {
                    "检查 GitHub 分支"
                } else {
                    "读取 GitHub 来源信息"
                };
                crate::tasks::report_progress(stage, None, None);
                let mut req = self
                    .client
                    .request(method.clone(), u.clone())
                    .header("User-Agent", "Skill-Manager")
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2022-11-28");
                if let Some(token) = &self.token {
                    req = req.bearer_auth(token);
                }
                if let Some(body) = body {
                    req = req.json(body);
                }
                if let Some(etag) = etag {
                    req = req.header("If-None-Match", etag);
                }
                let response = match req.send() {
                    Ok(response) => response,
                    Err(error) => {
                        if attempt == 0
                            && method == Method::GET
                            && (error.is_connect() || error.is_timeout())
                        {
                            drop(_permit);
                            retry_wait()?;
                            continue;
                        }
                        return Err(if error.is_timeout() {
                            "GitHub request timed out"
                        } else {
                            "Cannot reach GitHub"
                        }
                        .into());
                    }
                };
                if response.status() == StatusCode::NOT_MODIFIED {
                    return Ok(Loaded {
                        body: vec![],
                        etag: None,
                        not_modified: true,
                    });
                }
                if optional_lookup_miss(optional, commit_lookup, response.status()) {
                    return Ok(Loaded {
                        body: b"null".to_vec(),
                        etag: None,
                        not_modified: false,
                    });
                }
                if !response.status().is_success() {
                    let status = response.status().as_u16();
                    if attempt == 0 && method == Method::GET && [502, 503, 504].contains(&status) {
                        drop(_permit);
                        retry_wait()?;
                        continue;
                    }
                    return Err(response_error(&response));
                }
                if response
                    .content_length()
                    .is_some_and(|n| n > MAX_RESPONSE as u64)
                {
                    return Err("GitHub response is too large".into());
                }
                let etag = response
                    .headers()
                    .get("etag")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let mut bytes = Vec::new();
                response
                    .take((MAX_RESPONSE + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "Cannot read GitHub response")?;
                if bytes.len() > MAX_RESPONSE {
                    return Err("GitHub response is too large".into());
                }
                serde_json::from_slice::<Value>(&bytes)
                    .map_err(|_| "GitHub returned an invalid JSON response")?;
                return Ok(Loaded {
                    body: bytes,
                    etag,
                    not_modified: false,
                });
            }
            Err("Cannot reach GitHub".into())
        };
        // File bodies enter the shared cache only after Snapshot validates the
        // decoded length and Git blob id, never as unverified JSON/base64.
        let bytes =
            if fresh || optional || method != Method::GET || segments.get(1) == Some(&"blobs") {
                load(None)?.body
            } else {
                let immutable = segments.first() == Some(&"git")
                    && segments.len() == 3
                    && ["trees", "blobs", "commits"].contains(&segments[1])
                    && sha(&json!(segments[2])).is_ok();
                let policy = if immutable {
                    CachePolicy::Immutable
                } else if segments.is_empty() {
                    CachePolicy::MaxAge(Duration::from_secs(900))
                } else {
                    CachePolicy::Revalidate
                };
                network::cached(&self.cache_key(u.as_str())?, policy, load)?
            };
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "GitHub returned an invalid JSON response")?;
        Ok((!value.is_null()).then_some(value))
    }
    fn cache_key(&self, url: &str) -> Result<String, String> {
        // The digest distinguishes credentials without retaining secrets in cache keys.
        let access = crate::remote::logic::hash(self.token.as_deref().unwrap_or("").as_bytes());
        Ok(format!(
            "{}:{}:{access}:{url}",
            self.generation,
            self.repo.to_ascii_lowercase()
        ))
    }
    pub fn get(&self, path: &[&str]) -> Result<Value, String> {
        self.request(Method::GET, path, None, false)?
            .ok_or_else(|| "Missing GitHub response".into())
    }
    fn get_fresh(&self, path: &[&str]) -> Result<Value, String> {
        self.request_mode(Method::GET, path, None, false, true)?
            .ok_or_else(|| "Missing GitHub response".into())
    }
    pub fn optional(&self, path: &[&str]) -> Result<Option<Value>, String> {
        self.request(Method::GET, path, None, true)
    }
    pub fn post(&self, path: &[&str], body: &Value) -> Result<Value, String> {
        self.request(Method::POST, path, Some(body), false)?
            .ok_or_else(|| "Missing GitHub response".into())
    }
    pub fn patch(&self, path: &[&str], body: &Value) -> Result<Value, String> {
        self.request(Method::PATCH, path, Some(body), false)?
            .ok_or_else(|| "Missing GitHub response".into())
    }
    pub fn head(&self, branch: &str) -> Result<String, String> {
        let mut path = vec!["git", "ref", "heads"];
        path.extend(branch.split('/'));
        let v = self.get(&path)?;
        if v["object"]["type"] != "commit" {
            return Err("The selected branch does not reference a commit".into());
        }
        sha(&v["object"]["sha"])
    }
    pub fn blob(&self, id: &str) -> Result<Vec<u8>, String> {
        self.blob_mode(id, false)
    }
    fn blob_mode(&self, id: &str, fresh: bool) -> Result<Vec<u8>, String> {
        let v = if fresh {
            self.get_fresh(&["git", "blobs", id])?
        } else {
            self.get(&["git", "blobs", id])?
        };
        if v["size"].as_u64().unwrap_or(u64::MAX) > MAX_FILE as u64 || v["encoding"] != "base64" {
            return Err("Remote file is too large or uses an unsupported encoding".into());
        }
        let content = v["content"].as_str().ok_or("Missing remote file content")?;
        let raw = STANDARD
            .decode(
                content
                    .bytes()
                    .filter(|c| !c.is_ascii_whitespace())
                    .collect::<Vec<_>>(),
            )
            .map_err(|_| "Invalid remote file encoding")?;
        if raw.len() > MAX_FILE {
            return Err("Remote file is too large".into());
        }
        if raw.starts_with(b"version https://git-lfs.github.com/spec/v1") {
            return Err("Git LFS files require an explicit external download and are not supported in runtime packages".into());
        }
        Ok(raw)
    }

    fn public_file_request(
        &self,
        commit: &str,
        source: &str,
    ) -> Result<reqwest::blocking::RequestBuilder, String> {
        let url = public_raw_url(&self.repo, commit, source)?;
        // The client has no default authorization header. Do not use the authenticated
        // API request builder here, even when this repository has a stored credential.
        Ok(self
            .client
            .get(url)
            .header("User-Agent", "Skill-Manager")
            .header("Accept", "application/octet-stream"))
    }

    fn public_file(
        &self,
        commit: &str,
        source: &str,
        expected_size: u64,
    ) -> Result<Vec<u8>, String> {
        if expected_size > MAX_FILE as u64 {
            return Err("Remote file exceeds the 4 MiB limit".into());
        }
        let load = |_: Option<&str>| -> Result<Loaded, String> {
            for attempt in 0..2 {
                crate::tasks::report_progress("等待 GitHub 文件访问", None, None);
                let _permit = network::acquire(true, true)?;
                crate::tasks::report_progress("读取 GitHub 文件", None, None);
                let response = match self.public_file_request(commit, source)?.send() {
                    Ok(response) => response,
                    Err(error) => {
                        if attempt == 0 && (error.is_connect() || error.is_timeout()) {
                            drop(_permit);
                            retry_wait()?;
                            continue;
                        }
                        return Err(if error.is_timeout() {
                            "Public GitHub file request timed out"
                        } else {
                            "Cannot read the public GitHub file"
                        }
                        .into());
                    }
                };
                if !response.status().is_success() {
                    if attempt == 0 && [502, 503, 504].contains(&response.status().as_u16()) {
                        drop(_permit);
                        retry_wait()?;
                        continue;
                    }
                    return Err(format!("Public GitHub file request returned HTTP {}. Refresh the source and check its access.",response.status().as_u16()));
                }
                if response
                    .content_length()
                    .is_some_and(|n| n > MAX_FILE as u64)
                {
                    return Err("Remote file exceeds the 4 MiB limit".into());
                }
                let mut bytes = Vec::new();
                response
                    .take((MAX_FILE + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "Cannot finish reading the public GitHub file")?;
                return Ok(Loaded {
                    body: validate_public_file(bytes, expected_size)?,
                    etag: None,
                    not_modified: false,
                });
            }
            Err("Cannot read the public GitHub file".into())
        };
        Ok(load(None)?.body)
    }
}

fn public_raw_url(repo: &str, commit: &str, source: &str) -> Result<Url, String> {
    let location = parse_location(repo)?;
    if !location.route.is_empty() {
        return Err("A repository identifier is required for a public file".into());
    }
    sha(&json!(commit))?;
    validate_relative(source)?;
    let mut url = Url::parse("https://raw.githubusercontent.com/").expect("constant URL");
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| "Invalid raw file URL")?;
        for segment in location.repo.split('/') {
            path.push(segment);
        }
        path.push(commit);
        for segment in source.split('/') {
            path.push(segment);
        }
    }
    Ok(url)
}

fn validate_public_file(bytes: Vec<u8>, expected_size: u64) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_FILE || expected_size > MAX_FILE as u64 {
        return Err("Remote file exceeds the 4 MiB limit".into());
    }
    if bytes.len() as u64 != expected_size {
        return Err(
            "Remote file length does not match the selected immutable repository tree".into(),
        );
    }
    if bytes.starts_with(b"version https://git-lfs.github.com/spec/v1") {
        return Err("Git LFS files require an explicit external download and are not supported in runtime packages".into());
    }
    Ok(bytes)
}

pub fn sha(v: &Value) -> Result<String, String> {
    let s = v.as_str().ok_or("Missing GitHub object identifier")?;
    if s.len() != 40 || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid GitHub object identifier".into());
    }
    Ok(s.into())
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub sha: String,
    pub mode: String,
    pub size: u64,
    pub source: String,
}

#[derive(Default)]
struct SnapshotBytes {
    files: BTreeMap<(String, String, u64), Vec<u8>>,
    total: usize,
}

pub struct Snapshot {
    pub api: Api,
    pub branch: String,
    pub commit: String,
    pub tree_sha: String,
    pub private: bool,
    pub tree: BTreeMap<String, Entry>,
    bytes: RefCell<SnapshotBytes>,
    fresh: bool,
}
impl Snapshot {
    pub fn load(repo: &str, branch: Option<&str>) -> Result<Self, String> {
        let api = Api::new(repo)?;
        let meta = api.get(&[])?;
        let branch = branch
            .or_else(|| meta["default_branch"].as_str())
            .ok_or("Repository has no default branch")?
            .to_owned();
        let c = api.get(&["commits", &branch])?;
        Self::at(
            api,
            branch,
            sha(&c["sha"])?,
            sha(&c["commit"]["tree"]["sha"])?,
            meta["private"].as_bool().unwrap_or(true),
        )
    }
    pub fn load_fresh(repo: &str, branch: Option<&str>) -> Result<Self, String> {
        let api = Api::new(repo)?;
        let meta = api.get_fresh(&[])?;
        let branch = branch
            .or_else(|| meta["default_branch"].as_str())
            .ok_or("Repository has no default branch")?
            .to_owned();
        let commit = api.get_fresh(&["commits", &branch])?;
        Self::at_mode(
            api,
            branch,
            sha(&commit["sha"])?,
            sha(&commit["commit"]["tree"]["sha"])?,
            meta["private"].as_bool().unwrap_or(true),
            true,
        )
    }
    /// A matching Git blob id proves these local bytes equal the immutable tree
    /// entry. This only avoids ordinary comparison downloads, never readback.
    pub fn use_local_files(
        &self,
        root: &std::path::Path,
        entries: &BTreeMap<String, Entry>,
    ) -> Result<(), String> {
        if self.fresh {
            return Ok(());
        }
        for (path, entry) in entries {
            network::check_cancelled()?;
            if !matches!(entry.mode.as_str(), "100644" | "100755") || entry.size > MAX_FILE as u64 {
                continue;
            }
            let Ok(bytes) = super::read_local(root, path) else {
                continue;
            };
            if bytes.len() as u64 != entry.size || blob_hash(&bytes) != entry.sha {
                continue;
            }
            let mut cache = self.bytes.borrow_mut();
            let key = (entry.sha.clone(), entry.source.clone(), entry.size);
            if !cache.files.contains_key(&key) && bytes.len() <= MAX_TOTAL - cache.total {
                cache.total += bytes.len();
                cache.files.insert(key, bytes);
            }
        }
        Ok(())
    }
    pub fn at_commit(repo: &str, commit: &str) -> Result<Self, String> {
        Self::at_commit_mode(repo, commit, false)
    }
    pub fn at_commit_fresh(repo: &str, commit: &str) -> Result<Self, String> {
        Self::at_commit_mode(repo, commit, true)
    }
    fn at_commit_mode(repo: &str, commit: &str, fresh: bool) -> Result<Self, String> {
        sha(&json!(commit))?;
        let api = Api::new(repo)?;
        let meta = if fresh {
            api.get_fresh(&[])?
        } else {
            api.get(&[])?
        };
        let c = if fresh {
            api.get_fresh(&["git", "commits", commit])?
        } else {
            api.get(&["git", "commits", commit])?
        };
        let mut snapshot = Self::at_mode(
            api,
            meta["default_branch"].as_str().unwrap_or("").into(),
            commit.into(),
            sha(&c["tree"]["sha"])?,
            meta["private"].as_bool().unwrap_or(true),
            fresh,
        )?;
        snapshot.fresh = fresh;
        Ok(snapshot)
    }
    fn at(
        api: Api,
        branch: String,
        commit: String,
        tree_sha: String,
        private: bool,
    ) -> Result<Self, String> {
        Self::at_mode(api, branch, commit, tree_sha, private, false)
    }
    fn at_mode(
        api: Api,
        branch: String,
        commit: String,
        tree_sha: String,
        private: bool,
        fresh: bool,
    ) -> Result<Self, String> {
        let tree = if fresh {
            api.get_fresh(&["git", "trees", &tree_sha])?
        } else {
            api.get(&["git", "trees", &tree_sha])?
        };
        if tree["truncated"].as_bool().unwrap_or(false) {
            return Err("Repository tree exceeds GitHub's complete listing limit; choose a smaller repository".into());
        }
        let entries = tree["tree"].as_array().ok_or("Missing repository tree")?;
        if entries.len() > 20000 {
            return Err("Repository has too many entries for a Skill source".into());
        }
        let mut files = BTreeMap::new();
        for v in entries {
            if v["type"] != "blob" {
                continue;
            }
            let path = v["path"]
                .as_str()
                .ok_or("Invalid repository entry")?
                .to_owned();
            files.insert(
                path.clone(),
                Entry {
                    sha: sha(&v["sha"])?,
                    mode: v["mode"].as_str().unwrap_or("").into(),
                    size: v["size"].as_u64().unwrap_or(u64::MAX),
                    source: path,
                },
            );
        }
        Ok(Self {
            api,
            branch,
            commit,
            tree_sha,
            private,
            tree: files,
            bytes: RefCell::new(SnapshotBytes::default()),
            fresh,
        })
    }
    pub fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        validate_relative(path)?;
        let e = self
            .tree
            .get(path)
            .ok_or_else(|| format!("File does not exist at this version: {path}"))?;
        self.read_entry(e)
    }
    pub fn read_entry(&self, e: &Entry) -> Result<Vec<u8>, String> {
        self.read_entry_with(e, || {
            let load = |_: Option<&str>| -> Result<Loaded, String> {
                network::check_cancelled()?;
                let bytes = if self.private {
                    if self.fresh {
                        self.api.blob_mode(&e.sha, true)?
                    } else {
                        self.api.blob(&e.sha)?
                    }
                } else {
                    self.api.public_file(&self.commit, &e.source, e.size)?
                };
                if bytes.len() as u64 != e.size || blob_hash(&bytes) != e.sha {
                    return Err("Remote file bytes do not match the immutable GitHub tree".into());
                }
                Ok(Loaded {
                    body: bytes,
                    etag: None,
                    not_modified: false,
                })
            };
            if self.fresh {
                Ok(load(None)?.body)
            } else {
                network::cached(
                    &self.api.cache_key(&format!("blob:{}:{}", e.sha, e.size))?,
                    CachePolicy::Immutable,
                    load,
                )
            }
        })
    }
    fn read_entry_with(
        &self,
        e: &Entry,
        load: impl FnOnce() -> Result<Vec<u8>, String>,
    ) -> Result<Vec<u8>, String> {
        if !["100644", "100755"].contains(&e.mode.as_str()) {
            return Err("Symbolic links are not allowed in Skill packages".into());
        }
        if e.size > MAX_FILE as u64 {
            return Err("Remote file exceeds the 4 MiB limit".into());
        }
        let key = (e.sha.clone(), e.source.clone(), e.size);
        if let Some(bytes) = self.bytes.borrow().files.get(&key) {
            return Ok(bytes.clone());
        }
        let bytes = load()?;
        let mut cache = self.bytes.borrow_mut();
        // The cache belongs to this immutable repository snapshot only. Once
        // full, later reads still work without retaining additional bytes.
        if bytes.len() <= MAX_TOTAL - cache.total {
            cache.total += bytes.len();
            cache.files.insert(key, bytes.clone());
        }
        Ok(bytes)
    }
    pub fn skill_files(&self, path: &str) -> Result<BTreeMap<String, Entry>, String> {
        if !path.is_empty() {
            validate_relative(path)?;
        }
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        let mut files: BTreeMap<String, Entry> = self
            .tree
            .iter()
            .filter_map(|(p, e)| p.strip_prefix(&prefix).map(|r| (r.to_owned(), e.clone())))
            .collect();
        // A repository license applies to packages below the repository root too.
        // A dependency's nested license does not replace the Skill's own
        // license. Only an explicit license at the package root overrides
        // the source repository's root license.
        if !files.keys().any(|p| !p.contains('/') && is_license(p)) {
            for (p, e) in &self.tree {
                if !p.contains('/') && is_license(p) {
                    files.entry(p.clone()).or_insert_with(|| e.clone());
                }
            }
        }
        if !files.contains_key("SKILL.md") {
            return Err("The selected directory does not contain SKILL.md".into());
        }
        Ok(files)
    }
    pub fn catalog(&self) -> Result<Option<Value>, String> {
        for name in ["skill-catalog.json", "skills/skill-catalog.json"] {
            if self.tree.contains_key(name) {
                let bytes = self.read(name)?;
                return serde_json::from_slice(&bytes)
                    .map(Some)
                    .map_err(|_| "The source skill-catalog.json is invalid JSON".into());
            }
        }
        Ok(None)
    }
}

pub fn is_license(path: &str) -> bool {
    let leaf = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    ["license", "licence", "copying", "notice"].iter().any(|v| {
        leaf == *v || leaf.starts_with(&format!("{v}.")) || leaf.starts_with(&format!("{v}-"))
    })
}

pub fn resolve_location(location: &Location) -> Result<(Snapshot, String), String> {
    if location.route.is_empty() {
        return Ok((Snapshot::load(&location.repo, None)?, String::new()));
    }
    let api = Api::new(&location.repo)?;
    // Branch names may contain slashes. Resolve the longest existing commit/ref prefix.
    for i in (2..=location.route.len()).rev() {
        let reference = location.route[1..i].join("/");
        if let Some(c) = api.optional(&["commits", &reference])? {
            let meta = api.get(&[])?;
            let snapshot = Snapshot::at(
                Api::new(&location.repo)?,
                reference,
                sha(&c["sha"])?,
                sha(&c["commit"]["tree"]["sha"])?,
                meta["private"].as_bool().unwrap_or(true),
            )?;
            let mut path = location.route[i..].join("/");
            if location.route[0] == "blob" {
                if !path.ends_with("/SKILL.md") && path != "SKILL.md" {
                    return Err("Use a directory URL or a SKILL.md URL".into());
                }
                path = path
                    .strip_suffix("SKILL.md")
                    .unwrap_or("")
                    .trim_end_matches('/')
                    .into();
            }
            if !path.is_empty() {
                validate_relative(&path)?;
            }
            return Ok((snapshot, path));
        }
    }
    Err("The GitHub URL does not contain an accessible branch or version".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripted_http(
        replies: Vec<(u16, Value, Option<&'static str>)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, value, etag) in replies {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(4)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                }
                requests.push(String::from_utf8(request).unwrap());
                let bytes = if status == 304 {
                    vec![]
                } else {
                    serde_json::to_vec(&value).unwrap()
                };
                let tag = etag
                    .map(|tag| format!("ETag: {tag}\r\n"))
                    .unwrap_or_default();
                let header=format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{tag}Connection: close\r\n\r\n",bytes.len());
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(&bytes).unwrap();
                let _ = stream.shutdown(std::net::Shutdown::Write);
                while stream.read(&mut buffer).is_ok_and(|count| count > 0) {}
            }
            requests
        });
        (endpoint, worker)
    }

    #[test]
    fn immutable_snapshots_share_http_reads_but_independent_readback_bypasses_them() {
        let commit = "a".repeat(40);
        let tree = "b".repeat(40);
        let content = b"original";
        let blob = blob_hash(content);
        let meta = json!({"private":true,"default_branch":"main"});
        let object = json!({"sha":commit,"tree":{"sha":tree}});
        let entries = json!({"truncated":false,"tree":[{"path":"SKILL.md","type":"blob","mode":"100644","sha":blob,"size":content.len()}]});
        let bytes = |data: &[u8]| json!({"encoding":"base64","size":data.len(),"content":STANDARD.encode(data)});
        let head = json!({"sha":commit,"commit":{"tree":{"sha":tree}}});
        let (endpoint, server) = scripted_http(vec![
            (200, meta.clone(), None),
            (200, object.clone(), None),
            (200, entries.clone(), None),
            (200, bytes(content), None),
            (200, head, Some("\"head-v1\"")),
            (304, Value::Null, None),
            (200, meta, None),
            (200, object, None),
            (200, entries, None),
            (200, bytes(b"readback"), None),
        ]);
        let repo = format!("synthetic-protocol/cache-{}", uuid::Uuid::new_v4().simple());
        let _transport = test_transport(&repo, &endpoint);
        let first = Snapshot::at_commit(&repo, &commit).unwrap();
        assert_eq!(first.read("SKILL.md").unwrap(), content);
        let second = Snapshot::at_commit(&repo, &commit).unwrap();
        assert_eq!(second.read("SKILL.md").unwrap(), content);
        let api = Api::new(&repo).unwrap();
        assert_eq!(api.get(&["commits", "main"]).unwrap()["sha"], commit);
        assert_eq!(api.get(&["commits", "main"]).unwrap()["sha"], commit);
        let independent = Snapshot::at_commit_fresh(&repo, &commit).unwrap();
        assert!(independent
            .read("SKILL.md")
            .unwrap_err()
            .contains("immutable GitHub tree"));
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 10);
        assert!(requests[5]
            .to_ascii_lowercase()
            .contains("if-none-match: \"head-v1\""));
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.contains("/git/blobs/"))
                .count(),
            2
        );
    }

    #[test]
    fn equal_local_blob_bytes_supply_comparison_without_becoming_fresh_readback() {
        assert_eq!(
            blob_hash(b"test content\n"),
            "d670460b4b4aece5915caf5c68d12f560a9fe3e4"
        );
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/network-tests");
        std::fs::create_dir_all(&root).unwrap();
        let fixture = tempfile::tempdir_in(root).unwrap();
        std::fs::write(fixture.path().join("SKILL.md"), b"local bytes").unwrap();
        let snapshot = cache_snapshot("synthetic/local-hash", &"a".repeat(40));
        let entry = Entry {
            sha: blob_hash(b"local bytes"),
            mode: "100644".into(),
            size: 11,
            source: "skill/SKILL.md".into(),
        };
        let entries = BTreeMap::from([("SKILL.md".into(), entry.clone())]);
        snapshot.use_local_files(fixture.path(), &entries).unwrap();
        assert_eq!(
            snapshot
                .read_entry_with(&entry, || panic!("equal bytes must not download"))
                .unwrap(),
            b"local bytes"
        );
        let mut fresh = cache_snapshot("synthetic/local-hash", &"a".repeat(40));
        fresh.fresh = true;
        fresh.use_local_files(fixture.path(), &entries).unwrap();
        assert!(fresh.bytes.borrow().files.is_empty());
        std::fs::write(fixture.path().join("SKILL.md"), b"local edits").unwrap();
        let changed = cache_snapshot("synthetic/local-hash", &"a".repeat(40));
        changed.use_local_files(fixture.path(), &entries).unwrap();
        assert!(changed.bytes.borrow().files.is_empty());
    }

    #[test]
    fn only_read_requests_retry_once_and_access_scopes_never_retain_plaintext_tokens() {
        let (endpoint, server) = scripted_http(vec![
            (503, json!({"message":"temporary"}), None),
            (200, json!({"ok":true}), None),
            (503, json!({"message":"write uncertainty"}), None),
        ]);
        let repo = format!("synthetic-protocol/retry-{}", uuid::Uuid::new_v4().simple());
        let _transport = test_transport(&repo, &endpoint);
        let api = Api::new(&repo).unwrap();
        assert_eq!(api.get(&["retry"]).unwrap()["ok"], true);
        assert!(api
            .post(&["write"], &json!({"value":"one submission"}))
            .is_err());
        let requests = server.join().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.starts_with("GET "))
                .count(),
            2
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.starts_with("POST "))
                .count(),
            1
        );
        let first = Api {
            client: api.client.clone(),
            repo: repo.clone(),
            token: Some("synthetic-secret-a".into()),
            generation: 1,
        };
        let second = Api {
            client: api.client.clone(),
            repo,
            token: Some("synthetic-secret-b".into()),
            generation: 1,
        };
        let key = first.cache_key("immutable-object").unwrap();
        assert!(!key.contains("synthetic-secret"));
        assert_ne!(key, second.cache_key("immutable-object").unwrap());
        let next = Api {
            generation: 2,
            ..first
        };
        assert_ne!(key, next.cache_key("immutable-object").unwrap());
    }

    #[test]
    fn invalid_successful_http_file_bytes_never_poison_a_later_retry() {
        let commit = "a".repeat(40);
        let tree = "b".repeat(40);
        let content = b"original";
        let blob = blob_hash(content);
        let blob_response = |bytes: &[u8]| json!({"encoding":"base64","size":bytes.len(),"content":STANDARD.encode(bytes)});
        let (endpoint, server) = scripted_http(vec![
            (200, json!({"private":true,"default_branch":"main"}), None),
            (200, json!({"sha":commit,"tree":{"sha":tree}}), None),
            (
                200,
                json!({"truncated":false,"tree":[{"path":"SKILL.md","type":"blob","mode":"100644","sha":blob,"size":content.len()}]}),
                None,
            ),
            (200, blob_response(b"badbytes"), None),
            (200, blob_response(content), None),
        ]);
        let repo = format!(
            "synthetic-protocol/corrupt-retry-{}",
            uuid::Uuid::new_v4().simple()
        );
        let _transport = test_transport(&repo, &endpoint);
        let first = Snapshot::at_commit(&repo, &commit).unwrap();
        assert!(first.read("SKILL.md").is_err());
        let retry = Snapshot::at_commit(&repo, &commit).unwrap();
        assert_eq!(retry.read("SKILL.md").unwrap(), content);
        assert_eq!(server.join().unwrap().len(), 5);
    }

    fn cache_snapshot(repo: &str, commit: &str) -> Snapshot {
        Snapshot {
            api: Api {
                client: Client::builder().build().unwrap(),
                token: None,
                repo: repo.into(),
                generation: 0,
            },
            branch: "main".into(),
            commit: commit.into(),
            tree_sha: "b".repeat(40),
            private: true,
            tree: BTreeMap::new(),
            bytes: RefCell::new(SnapshotBytes::default()),
            fresh: false,
        }
    }

    #[test]
    fn snapshot_reads_cache_only_successes_and_keep_entry_checks_before_hits() {
        let snapshot = cache_snapshot("synthetic/cache", &"a".repeat(40));
        let entry = Entry {
            sha: "c".repeat(40),
            mode: "100644".into(),
            size: 5,
            source: "skills/example/SKILL.md".into(),
        };
        let mut loads = 0;
        for _ in 0..3 {
            let bytes = snapshot
                .read_entry_with(&entry, || {
                    loads += 1;
                    Ok(b"skill".to_vec())
                })
                .unwrap();
            assert_eq!(bytes, b"skill");
        }
        assert_eq!(loads, 1);

        let mut another = entry.clone();
        another.source = "skills/another/SKILL.md".into();
        assert!(snapshot
            .read_entry_with(&another, || Err("temporary error".into()))
            .is_err());
        assert_eq!(
            snapshot
                .read_entry_with(&another, || Ok(b"other".to_vec()))
                .unwrap(),
            b"other"
        );
        assert_eq!(
            snapshot
                .read_entry_with(&another, || panic!("successful retry should be cached"))
                .unwrap(),
            b"other"
        );
        another.sha = "d".repeat(40);
        assert_eq!(
            snapshot
                .read_entry_with(&another, || Ok(b"fresh".to_vec()))
                .unwrap(),
            b"fresh"
        );

        let mut invalid = entry.clone();
        invalid.mode = "120000".into();
        assert!(snapshot
            .read_entry_with(&invalid, || panic!("links must be rejected"))
            .is_err());
        invalid.mode = "100644".into();
        invalid.size = (MAX_FILE + 1) as u64;
        assert!(snapshot
            .read_entry_with(&invalid, || panic!("oversize entries must be rejected"))
            .is_err());
        assert_eq!(snapshot.bytes.borrow().total, 15);
    }

    #[test]
    fn snapshot_byte_cache_is_bounded_and_never_shared_across_snapshots() {
        let commit = "a".repeat(40);
        let snapshot = cache_snapshot("synthetic/cache", &commit);
        for index in 0..MAX_TOTAL / MAX_FILE {
            let entry = Entry {
                sha: format!("{index:040x}"),
                mode: "100644".into(),
                size: MAX_FILE as u64,
                source: format!("assets/{index}.bin"),
            };
            assert_eq!(
                snapshot
                    .read_entry_with(&entry, || Ok(vec![7; MAX_FILE]))
                    .unwrap()
                    .len(),
                MAX_FILE
            );
        }
        assert_eq!(snapshot.bytes.borrow().total, MAX_TOTAL);
        let entry = Entry {
            sha: "e".repeat(40),
            mode: "100644".into(),
            size: 3,
            source: "SKILL.md".into(),
        };
        let mut loads = 0;
        for _ in 0..2 {
            assert_eq!(
                snapshot
                    .read_entry_with(&entry, || {
                        loads += 1;
                        Ok(b"end".to_vec())
                    })
                    .unwrap(),
                b"end"
            );
        }
        assert_eq!(loads, 2);
        assert_eq!(snapshot.bytes.borrow().total, MAX_TOTAL);
        assert_eq!(snapshot.bytes.borrow().files.len(), MAX_TOTAL / MAX_FILE);

        let cached = Entry {
            sha: format!("{:040x}", 0),
            mode: "100644".into(),
            size: MAX_FILE as u64,
            source: "assets/0.bin".into(),
        };
        assert_eq!(
            snapshot
                .read_entry_with(&cached, || panic!("full cache keeps existing entries"))
                .unwrap()
                .len(),
            MAX_FILE
        );
        for (repo, revision) in [
            ("synthetic/cache", commit.clone()),
            ("synthetic/other", commit),
            ("synthetic/cache", "f".repeat(40)),
        ] {
            let separate = cache_snapshot(repo, &revision);
            let mut called = false;
            assert_eq!(
                separate
                    .read_entry_with(&cached, || {
                        called = true;
                        Ok(vec![9; MAX_FILE])
                    })
                    .unwrap()[0],
                9
            );
            assert!(called);
        }
    }

    #[test]
    fn missing_commit_prefix_accepts_github_422_without_swallowing_auth_failures() {
        assert!(optional_lookup_miss(
            true,
            true,
            StatusCode::UNPROCESSABLE_ENTITY
        ));
        assert!(optional_lookup_miss(true, true, StatusCode::NOT_FOUND));
        assert!(!optional_lookup_miss(true, true, StatusCode::UNAUTHORIZED));
        assert!(!optional_lookup_miss(
            true,
            false,
            StatusCode::UNPROCESSABLE_ENTITY
        ));
        assert!(!optional_lookup_miss(
            false,
            true,
            StatusCode::UNPROCESSABLE_ENTITY
        ));
    }
    #[test]
    fn github_locations_are_restricted() {
        assert_eq!(
            parse_location("https://github.com/owner/repo.git")
                .unwrap()
                .repo,
            "owner/repo"
        );
        assert_eq!(
            parse_location("https://github.com/o/r/tree/main/my%20skill")
                .unwrap()
                .route[2],
            "my skill"
        );
        for p in [
            "https://evil.invalid/o/r",
            "http://github.com/o/r",
            "https://token@github.com/o/r",
            "owner",
            "o/r/issues",
        ] {
            assert!(parse_location(p).is_err(), "{p}");
        }
    }
    #[test]
    fn paths_reject_windows_escape_and_aliases() {
        for p in [
            "../a",
            "a/../b",
            "a\\b",
            "/a",
            "a//b",
            "C:/a",
            "x:stream",
            "a/CON.txt",
            ".git/config",
            "x./a",
            "a/NUL",
            "a/COM1.md",
        ] {
            assert!(validate_relative(p).is_err(), "{p}");
        }
        assert!(validate_relative("references/说明.md").is_ok());
        assert!(is_license("third-party/LICENSE-MIT"));
    }

    #[test]
    fn public_raw_urls_are_fixed_to_github_and_an_immutable_commit() {
        let commit = "a".repeat(40);
        let url = public_raw_url("owner/repo", &commit, "assets/a #图.png").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("raw.githubusercontent.com"));
        assert_eq!(url.query(), None);
        assert_eq!(url.fragment(), None);
        assert!(url
            .path()
            .starts_with(&format!("/owner/repo/{commit}/assets/")));
        assert!(url.as_str().contains("%20%23"));
        for (repo, revision, source) in [
            ("https://evil.invalid/o/r", commit.as_str(), "SKILL.md"),
            ("o/r", "main", "SKILL.md"),
            ("o/r", commit.as_str(), "../SKILL.md"),
            ("o/r", commit.as_str(), "https://evil.invalid/a"),
        ] {
            assert!(public_raw_url(repo, revision, source).is_err());
        }
    }

    #[test]
    fn public_file_request_never_attaches_a_saved_token() {
        let api = Api {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            token: Some("synthetic-test-credential".into()),
            repo: "owner/repo".into(),
            generation: 0,
        };
        let request = api
            .public_file_request(&"a".repeat(40), "SKILL.md")
            .unwrap()
            .build()
            .unwrap();
        assert!(!request
            .headers()
            .contains_key(reqwest::header::AUTHORIZATION));
        assert_eq!(request.url().host_str(), Some("raw.githubusercontent.com"));
    }

    #[test]
    fn public_files_keep_size_and_lfs_boundaries() {
        assert_eq!(
            validate_public_file(b"skill".to_vec(), 5).unwrap(),
            b"skill"
        );
        assert!(validate_public_file(b"short".to_vec(), 9).is_err());
        assert!(validate_public_file(vec![0; MAX_FILE + 1], (MAX_FILE + 1) as u64).is_err());
        let pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:example\n";
        assert!(validate_public_file(pointer.to_vec(), pointer.len() as u64).is_err());
    }

    #[test]
    fn nested_dependency_license_does_not_suppress_the_source_repository_license() {
        // Pure repository-tree fixture: no credential lookup or HTTP request.
        let snapshot = |paths: &[&str]| Snapshot {
            api: Api {
                client: Client::builder().build().unwrap(),
                token: None,
                repo: "synthetic/license-fixture".into(),
                generation: 0,
            },
            branch: "main".into(),
            commit: "a".repeat(40),
            tree_sha: "b".repeat(40),
            private: false,
            bytes: RefCell::new(SnapshotBytes::default()),
            fresh: false,
            tree: paths
                .iter()
                .map(|path| {
                    (
                        path.to_string(),
                        Entry {
                            sha: "c".repeat(40),
                            mode: "100644".into(),
                            size: 4,
                            source: path.to_string(),
                        },
                    )
                })
                .collect(),
        };
        let nested = snapshot(&[
            "LICENSE-ROOT",
            "skills/foo/SKILL.md",
            "skills/foo/assets/font/LICENSE",
        ]);
        let files = nested.skill_files("skills/foo").unwrap();
        assert_eq!(files["LICENSE-ROOT"].source, "LICENSE-ROOT");
        assert_eq!(
            files["assets/font/LICENSE"].source,
            "skills/foo/assets/font/LICENSE"
        );

        let explicit = snapshot(&[
            "LICENSE-ROOT",
            "skills/foo/SKILL.md",
            "skills/foo/LICENSE",
            "skills/foo/assets/font/LICENSE",
        ]);
        let files = explicit.skill_files("skills/foo").unwrap();
        assert!(!files.contains_key("LICENSE-ROOT"));
        assert_eq!(files["LICENSE"].source, "skills/foo/LICENSE");
        assert!(files.contains_key("assets/font/LICENSE"));
    }
}
