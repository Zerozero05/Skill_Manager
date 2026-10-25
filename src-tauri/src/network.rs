//! Shared transport and bounded, memory-only immutable content reuse.
use crate::model::Manager;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io::Read,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use url::Url;

const CACHE_LIMIT: usize = 64 * 1024 * 1024;
const PROXY_SERVICE: &str = "io.github.skill-manager.proxy";

#[derive(Clone)]
pub struct TaskContext {
    pub foreground: bool,
    pub cancel: Arc<AtomicBool>,
}
thread_local! {
    static CONTEXT: RefCell<Option<TaskContext>> = const { RefCell::new(None) };
}
pub fn with_task_context<T>(context: TaskContext, f: impl FnOnce() -> T) -> T {
    struct Reset(Option<TaskContext>);
    impl Drop for Reset {
        fn drop(&mut self) {
            CONTEXT.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _reset = Reset(CONTEXT.with(|slot| slot.replace(Some(context))));
    f()
}
pub fn check_cancelled() -> Result<(), String> {
    if CONTEXT.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|context| context.cancel.load(Ordering::Relaxed))
    }) {
        Err("操作已取消".into())
    } else {
        Ok(())
    }
}
fn foreground() -> bool {
    CONTEXT.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_none_or(|context| context.foreground)
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "auto")]
    pub mode: String,
    #[serde(default, rename = "proxyUrl")]
    pub proxy_url: String,
}
fn auto() -> String {
    "auto".into()
}
impl Settings {
    fn from_value(value: &Value) -> Result<Self, String> {
        let mut settings: Self = if value.is_null() {
            Self {
                mode: auto(),
                proxy_url: String::new(),
            }
        } else {
            serde_json::from_value(value.clone()).map_err(|_| "网络设置格式无效")?
        };
        if !["auto", "direct", "manual"].contains(&settings.mode.as_str()) {
            return Err("请选择跟随系统、直连或手动代理".into());
        }
        settings.proxy_url = settings.proxy_url.trim().into();
        if settings.mode == "manual" {
            settings.proxy_url = validate_proxy(&settings.proxy_url)?.to_string();
        } else {
            settings.proxy_url.clear();
        }
        Ok(settings)
    }
}
fn validate_proxy(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "代理地址无效，请输入协议、主机和端口")?;
    if !["http", "https", "socks5", "socks5h"].contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err("代理地址仅支持 HTTP、HTTPS、SOCKS5；用户名和密码请填写在凭据栏".into());
    }
    Ok(url)
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProxyCredential {
    username: String,
    password: String,
}
fn proxy_credential(endpoint: &str) -> Result<Option<ProxyCredential>, String> {
    if endpoint.is_empty() {
        return Ok(None);
    }
    let entry = keyring::Entry::new(PROXY_SERVICE, endpoint).map_err(|_| "无法访问系统代理凭据")?;
    match entry.get_password() {
        Ok(value) => serde_json::from_str(&value)
            .map(Some)
            .map_err(|_| "系统代理凭据格式无效，请重新保存".into()),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("无法读取系统代理凭据".into()),
    }
}
fn save_proxy_credential(
    endpoint: &str,
    credential: Option<&ProxyCredential>,
) -> Result<(), String> {
    let entry = keyring::Entry::new(PROXY_SERVICE, endpoint).map_err(|_| "无法访问系统代理凭据")?;
    if let Some(credential) = credential {
        entry
            .set_password(&serde_json::to_string(credential).map_err(|_| "代理凭据格式无效")?)
            .map_err(|_| "无法保存系统代理凭据".into())
    } else {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("无法移除系统代理凭据".into()),
        }
    }
}
fn build_client(
    settings: &Settings,
    credential: Option<ProxyCredential>,
) -> Result<Client, String> {
    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none());
    match settings.mode.as_str() {
        "direct" => builder = builder.no_proxy(),
        "manual" => {
            let mut url = validate_proxy(&settings.proxy_url)?;
            let mut proxy = reqwest::Proxy::all(url.as_str()).map_err(|_| "无法初始化代理")?;
            if let Some(credential) = credential {
                if matches!(url.scheme(), "socks5" | "socks5h") {
                    url.set_username(&credential.username)
                        .map_err(|_| "代理用户名无效")?;
                    url.set_password(Some(&credential.password))
                        .map_err(|_| "代理凭据无效")?;
                    proxy = reqwest::Proxy::all(url.as_str()).map_err(|_| "无法初始化代理")?;
                } else {
                    proxy = proxy.basic_auth(&credential.username, &credential.password);
                }
            }
            builder = builder.proxy(proxy);
        }
        _ => {}
    }
    builder
        .build()
        .map_err(|_| "无法初始化网络连接，请检查代理设置".into())
}
struct Transport {
    settings: Settings,
    client: Option<Client>,
    generation: u64,
}
impl Default for Transport {
    fn default() -> Self {
        Self {
            settings: Settings {
                mode: auto(),
                proxy_url: String::new(),
            },
            client: None,
            generation: 0,
        }
    }
}
fn transport() -> &'static Mutex<Transport> {
    static VALUE: OnceLock<Mutex<Transport>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(Transport::default()))
}
pub fn configure(settings: &Value) -> Result<(), String> {
    let desired = Settings::from_value(&settings["network"])?;
    let mut transport = transport().lock().map_err(|_| "网络设置锁不可用")?;
    if transport.client.is_some() && transport.settings == desired {
        return Ok(());
    }
    let credential = proxy_credential(&desired.proxy_url)?;
    let client = build_client(&desired, credential)?;
    transport.settings = desired;
    transport.client = Some(client);
    transport.generation += 1;
    clear_cache();
    Ok(())
}
pub fn client() -> Result<(Client, u64), String> {
    let mut transport = transport().lock().map_err(|_| "网络设置锁不可用")?;
    if transport.client.is_none() {
        transport.client = Some(build_client(
            &transport.settings,
            proxy_credential(&transport.settings.proxy_url)?,
        )?);
    }
    Ok((
        transport
            .client
            .as_ref()
            .expect("initialized client")
            .clone(),
        transport.generation,
    ))
}

fn generation_from(transport: &Mutex<Transport>) -> Result<u64, String> {
    transport
        .lock()
        .map(|value| value.generation)
        .map_err(|_| "网络设置锁不可用".into())
}

/// An opaque read namespace. It never reads or exposes account credentials.
pub fn read_generation() -> Result<u64, String> {
    generation_from(transport())
}
pub fn invalidate_credentials() {
    if let Ok(mut transport) = transport().lock() {
        transport.generation += 1;
    }
    clear_cache();
}

#[derive(Default)]
struct GateState {
    api_active: usize,
    raw_active: usize,
    foreground_api: usize,
    foreground_raw: usize,
    paused_until: Option<Instant>,
}
#[derive(Default)]
struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
}
fn gate() -> &'static Gate {
    static VALUE: OnceLock<Gate> = OnceLock::new();
    VALUE.get_or_init(Gate::default)
}
pub struct Permit {
    raw: bool,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let gate = gate();
        if let Ok(mut state) = gate.state.lock() {
            if self.raw {
                state.raw_active -= 1;
            } else {
                state.api_active -= 1;
            }
            gate.changed.notify_all();
        }
    }
}
pub fn acquire(raw: bool, cancellable: bool) -> Result<Permit, String> {
    let gate = gate();
    let priority = foreground();
    let mut state = gate.state.lock().map_err(|_| "网络队列不可用")?;
    if priority {
        if raw {
            state.foreground_raw += 1;
        } else {
            state.foreground_api += 1;
        }
    }
    loop {
        if cancellable && check_cancelled().is_err() {
            if priority {
                if raw {
                    state.foreground_raw -= 1;
                } else {
                    state.foreground_api -= 1;
                }
            }
            gate.changed.notify_all();
            return Err("操作已取消".into());
        }
        let paused = !raw
            && state
                .paused_until
                .is_some_and(|until| until > Instant::now());
        let waiting = if raw {
            state.foreground_raw
        } else {
            state.foreground_api
        };
        let active = if raw {
            state.raw_active
        } else {
            state.api_active
        };
        if !paused && active < if raw { 4 } else { 1 } && (priority || waiting == 0) {
            if raw {
                state.raw_active += 1;
                if priority {
                    state.foreground_raw -= 1;
                }
            } else {
                state.api_active += 1;
                if priority {
                    state.foreground_api -= 1;
                }
            }
            return Ok(Permit { raw });
        }
        if paused {
            // Return a visible deferred result; never occupy the worker for a minute.
            if priority {
                state.foreground_api -= 1;
            }
            return Err("GitHub 请求暂缓：服务器限流，请稍后重试".into());
        }
        state = gate
            .changed
            .wait_timeout(state, Duration::from_millis(50))
            .map_err(|_| "网络队列不可用")?
            .0;
    }
}
pub fn pause_api(duration: Duration) {
    if let Ok(mut state) = gate().state.lock() {
        state.paused_until = Some(Instant::now() + duration);
        gate().changed.notify_all();
    }
}

#[derive(Clone)]
struct Cached {
    body: Vec<u8>,
    etag: Option<String>,
    saved: Instant,
    used: u64,
}
#[derive(Default)]
struct Memory {
    entries: HashMap<String, Cached>,
    pending: HashSet<String>,
    bytes: usize,
    clock: u64,
}
struct Cache {
    memory: Mutex<Memory>,
    changed: Condvar,
    limit: usize,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            memory: Mutex::new(Memory::default()),
            changed: Condvar::new(),
            limit: CACHE_LIMIT,
        }
    }
}
fn cache() -> &'static Cache {
    static VALUE: OnceLock<Cache> = OnceLock::new();
    VALUE.get_or_init(Cache::default)
}
pub fn clear_cache() {
    if let Ok(mut memory) = cache().memory.lock() {
        memory.entries.clear();
        memory.bytes = 0;
    }
}
pub enum CachePolicy {
    Immutable,
    Revalidate,
    MaxAge(Duration),
}
pub struct Loaded {
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub not_modified: bool,
}
pub fn cached(
    key: &str,
    policy: CachePolicy,
    load: impl FnOnce(Option<&str>) -> Result<Loaded, String>,
) -> Result<Vec<u8>, String> {
    cached_with(cache(), key, policy, load)
}
fn cached_with(
    cache: &Cache,
    key: &str,
    policy: CachePolicy,
    load: impl FnOnce(Option<&str>) -> Result<Loaded, String>,
) -> Result<Vec<u8>, String> {
    let requested = Instant::now();
    let mut memory = cache.memory.lock().map_err(|_| "内存缓存不可用")?;
    loop {
        check_cancelled()?;
        memory.clock += 1;
        let used = memory.clock;
        if let Some(entry) = memory.entries.get_mut(key) {
            let valid = match policy {
                CachePolicy::Immutable => true,
                CachePolicy::MaxAge(age) => entry.saved.elapsed() < age,
                CachePolicy::Revalidate => entry.saved >= requested,
            };
            if valid {
                entry.used = used;
                return Ok(entry.body.clone());
            }
        }
        if !memory.pending.contains(key) {
            memory.pending.insert(key.into());
            break;
        }
        memory = cache
            .changed
            .wait_timeout(memory, Duration::from_millis(50))
            .map_err(|_| "内存缓存不可用")?
            .0;
    }
    let previous = memory.entries.get(key).cloned();
    drop(memory);
    let result =
        load(previous.as_ref().and_then(|entry| entry.etag.as_deref())).and_then(|loaded| {
            if loaded.not_modified {
                previous
                    .clone()
                    .map(|mut entry| {
                        entry.saved = Instant::now();
                        entry
                    })
                    .ok_or_else(|| "缓存缺失，无法处理304响应".into())
            } else {
                Ok(Cached {
                    body: loaded.body,
                    etag: loaded.etag,
                    saved: Instant::now(),
                    used: 0,
                })
            }
        });
    let mut memory = cache.memory.lock().map_err(|_| "内存缓存不可用")?;
    memory.pending.remove(key);
    if let Ok(entry) = &result {
        if entry.body.len() <= cache.limit {
            if let Some(old) = memory.entries.remove(key) {
                memory.bytes -= old.body.len();
            }
            while memory.bytes + entry.body.len() > cache.limit {
                let Some(oldest) = memory
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.used)
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                if let Some(old) = memory.entries.remove(&oldest) {
                    memory.bytes -= old.body.len();
                }
            }
            memory.clock += 1;
            let mut entry = entry.clone();
            entry.used = memory.clock;
            memory.bytes += entry.body.len();
            memory.entries.insert(key.into(), entry);
        }
    }
    cache.changed.notify_all();
    result.map(|entry| entry.body)
}

fn public_settings(settings: &Settings) -> Result<Value, String> {
    Ok(
        json!({"mode":settings.mode,"proxyUrl":settings.proxy_url,"hasProxyCredentials":proxy_credential(&settings.proxy_url)?.is_some()}),
    )
}
fn diagnostic(client: &Client, url: &str, raw: bool) -> Value {
    let start = Instant::now();
    let result = (|| -> Result<u16, String> {
        let _permit = acquire(raw, true)?;
        let mut response = client
            .get(url)
            .header("User-Agent", "Skill-Manager")
            .header(
                "Accept",
                if raw {
                    "text/plain"
                } else {
                    "application/vnd.github+json"
                },
            )
            .timeout(Duration::from_secs(10))
            .send()
            .map_err(|error| {
                if error.is_timeout() {
                    "连接超时"
                } else if error.is_connect() {
                    "连接失败，请检查代理和网络"
                } else {
                    "请求失败"
                }
            })?;
        let status = response.status().as_u16();
        let mut sink = Vec::new();
        response
            .by_ref()
            .take(64 * 1024)
            .read_to_end(&mut sink)
            .map_err(|_| "响应读取失败")?;
        Ok(status)
    })();
    match result {
        Ok(status) => {
            json!({"ok":(200..400).contains(&status),"status":status,"durationMs":start.elapsed().as_millis()})
        }
        Err(error) => json!({"ok":false,"durationMs":start.elapsed().as_millis(),"error":error}),
    }
}
pub fn handle(manager: &mut Manager, request: &Value) -> Result<Option<Value>, String> {
    Ok(Some(match request["action"].as_str().unwrap_or("") {
        "get_network_settings" => {
            public_settings(&Settings::from_value(&manager.state.settings["network"])?)?
        }
        "set_network_settings" => {
            let settings = Settings::from_value(&request["settings"])?;
            let previous_credential = proxy_credential(&settings.proxy_url)?;
            let mut next_credential = previous_credential.clone();
            if settings.mode == "manual" {
                if request["clearProxyCredentials"] == true {
                    next_credential = None;
                }
                if let Some(username) = request["proxyUsername"].as_str() {
                    if !username.is_empty()
                        || request["proxyPassword"]
                            .as_str()
                            .is_some_and(|password| !password.is_empty())
                    {
                        next_credential = Some(ProxyCredential {
                            username: username.into(),
                            password: request["proxyPassword"].as_str().unwrap_or("").into(),
                        });
                    }
                }
            }
            let next_client = build_client(&settings, next_credential.clone())?;
            let credential_changed = previous_credential != next_credential;
            if credential_changed {
                save_proxy_credential(&settings.proxy_url, next_credential.as_ref())?;
            }
            let old = manager.state.settings.clone();
            manager.state.settings["network"] =
                serde_json::to_value(&settings).map_err(|_| "网络设置格式无效")?;
            if let Err(error) = manager.save() {
                manager.state.settings = old;
                if credential_changed
                    && save_proxy_credential(&settings.proxy_url, previous_credential.as_ref())
                        .is_err()
                {
                    return Err(format!(
                        "网络设置未保存；代理凭据回退失败，请重新保存凭据：{error}"
                    ));
                }
                return Err(error);
            }
            {
                let mut transport = transport().lock().map_err(|_| "网络设置锁不可用")?;
                transport.settings = settings.clone();
                transport.client = Some(next_client);
                transport.generation += 1;
            }
            clear_cache();
            let mut snapshot = manager.snapshot();
            snapshot["networkSettings"] = public_settings(&settings)?;
            snapshot
        }
        "test_network" => {
            let (client, _) = client()?;
            let mode = transport()
                .lock()
                .map_err(|_| "网络设置锁不可用")?
                .settings
                .mode
                .clone();
            let api = diagnostic(&client, "https://api.github.com/rate_limit", false);
            let raw = diagnostic(
                &client,
                "https://raw.githubusercontent.com/github/gitignore/main/Global/Windows.gitignore",
                true,
            );
            json!({"mode":mode,"api":api,"raw":raw})
        }
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn v025_read_generation_only_reports_the_transport_epoch() {
        let transport = Mutex::new(Transport::default());
        assert_eq!(generation_from(&transport).unwrap(), 0);
        transport.lock().unwrap().generation += 1;
        assert_eq!(generation_from(&transport).unwrap(), 1);
        assert_eq!(
            serde_json::to_value(generation_from(&transport).unwrap()).unwrap(),
            json!(1)
        );
        assert!(transport.lock().unwrap().client.is_none());
    }
    #[test]
    fn lru_memory_bound_evicts_old_content_and_retains_recent_hits() {
        let cache = Cache {
            limit: 8,
            ..Cache::default()
        };
        let read = |key: &str| {
            cached_with(&cache, key, CachePolicy::Immutable, |_| {
                Ok(Loaded {
                    body: vec![1; 4],
                    etag: None,
                    not_modified: false,
                })
            })
        };
        read("a").unwrap();
        read("b").unwrap();
        read("a").unwrap();
        read("c").unwrap();
        let memory = cache.memory.lock().unwrap();
        assert_eq!(memory.bytes, 8);
        assert!(memory.entries.contains_key("a"));
        assert!(memory.entries.contains_key("c"));
        assert!(!memory.entries.contains_key("b"));
    }
    #[test]
    fn proxy_settings_never_accept_secrets_in_the_public_address() {
        for address in [
            "http://user:secret@127.0.0.1:8080",
            "http://localhost:8080/?token=secret",
            "file:///secret",
            "http://localhost:8080/path",
        ] {
            assert!(Settings::from_value(&json!({"mode":"manual","proxyUrl":address})).is_err());
        }
        for address in [
            "http://127.0.0.1:8080",
            "https://localhost:443",
            "socks5://localhost:1080",
            "socks5h://localhost:1080",
        ] {
            assert!(Settings::from_value(&json!({"mode":"manual","proxyUrl":address})).is_ok());
        }
    }
    #[test]
    fn memory_cache_revalidates_and_keeps_fixed_bytes_without_disk_files() {
        let key = format!("cache-test:{}", uuid::Uuid::new_v4());
        assert_eq!(
            cached(&key, CachePolicy::Immutable, |_| Ok(Loaded {
                body: b"value".to_vec(),
                etag: Some("v1".into()),
                not_modified: false
            }))
            .unwrap(),
            b"value"
        );
        assert_eq!(
            cached(&key, CachePolicy::Immutable, |_| panic!(
                "immutable cache should be reused"
            ))
            .unwrap(),
            b"value"
        );
        assert_eq!(
            cached(&key, CachePolicy::Revalidate, |tag| {
                assert_eq!(tag, Some("v1"));
                Ok(Loaded {
                    body: vec![],
                    etag: None,
                    not_modified: true,
                })
            })
            .unwrap(),
            b"value"
        );
    }
    #[test]
    fn cancelled_reads_leave_no_pending_cache_request() {
        let key = format!("cache-test:{}", uuid::Uuid::new_v4());
        with_task_context(
            TaskContext {
                foreground: true,
                cancel: Arc::new(AtomicBool::new(true)),
            },
            || {
                assert!(cached(&key, CachePolicy::Immutable, |_| panic!("cancelled")).is_err());
                assert!(acquire(false, true).is_err());
            },
        );
        assert!(!cache().memory.lock().unwrap().pending.contains(&key));
    }
    #[test]
    fn simultaneous_fixed_content_reads_share_one_loader_and_failures_are_not_saved() {
        use std::sync::{atomic::AtomicUsize, Barrier};
        let key = format!("cache-test:{}", uuid::Uuid::new_v4());
        let count = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(6));
        let workers = (0..6)
            .map(|_| {
                let key = key.clone();
                let count = count.clone();
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    cached(&key, CachePolicy::Immutable, |_| {
                        count.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(70));
                        Ok(Loaded {
                            body: b"same bytes".to_vec(),
                            etag: None,
                            not_modified: false,
                        })
                    })
                    .unwrap()
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            assert_eq!(worker.join().unwrap(), b"same bytes");
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let failed = format!("cache-test:{}", uuid::Uuid::new_v4());
        assert!(cached(&failed, CachePolicy::Immutable, |_| Err(
            "temporary failure".into()
        ))
        .is_err());
        assert_eq!(
            cached(&failed, CachePolicy::Immutable, |_| Ok(Loaded {
                body: b"recovered".to_vec(),
                etag: None,
                not_modified: false
            }))
            .unwrap(),
            b"recovered"
        );
    }
}
