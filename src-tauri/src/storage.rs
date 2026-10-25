//! All application-owned paths are resolved before opening SQLite or WebView.
//! Migration copies known data only, keeps originals, and never moves Skill sources.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const KINDS: &[&str] = &[
    "state", "webview", "recovery", "receipts", "temp", "newSkill", "export",
];
const DURABLE: &[&str] = &["state", "webview", "recovery", "receipts", "temp"];
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Config {
    pub managed_root: PathBuf,
    #[serde(default)]
    pub overrides: BTreeMap<String, PathBuf>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    id: String,
    previous: Config,
    files: Vec<CopyEntry>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Bootstrap {
    format_version: u32,
    config: Config,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<Pending>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_migration: Option<Value>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopyEntry {
    source: PathBuf,
    destination: PathBuf,
    bytes: u64,
    digest: String,
}
#[derive(Clone)]
struct Preview {
    id: String,
    from: Config,
    to: Config,
    created: Instant,
}
struct Inner {
    bootstrap: Bootstrap,
    preview: Option<Preview>,
    statistics: Option<(Instant, Value)>,
}
pub(crate) struct Storage {
    inner: Mutex<Inner>,
    stats_lock: Mutex<()>,
    bootstrap_path: Option<PathBuf>,
    source: String,
    locked: bool,
    project_root: Option<PathBuf>,
    developer_snapshot: Option<Value>,
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.as_os_str().is_empty() {
        return Err("保存位置必须是完整的绝对路径".into());
    }
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => return Err("保存位置不能包含上级目录跳转".into()),
            Component::CurDir => {}
            other => result.push(other.as_os_str()),
        }
    }
    let mut ancestor = Some(result.as_path());
    while let Some(path) = ancestor {
        if let Ok(meta) = fs::symlink_metadata(path) {
            if reparse(&meta) {
                return Err(format!("保存位置不能经过目录链接：{}", path.display()));
            }
            if !meta.is_dir() {
                return Err(format!("保存位置经过了普通文件：{}", path.display()));
            }
        }
        ancestor = path.parent();
    }
    Ok(result)
}
fn reparse(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}
fn key(path: &Path) -> String {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy().replace('/', "\\").to_lowercase();
        if let Some(rest) = value.strip_prefix("\\\\?\\unc\\") {
            format!("\\\\{rest}")
        } else {
            value.strip_prefix("\\\\?\\").unwrap_or(&value).to_owned()
        }
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().to_string()
    }
}
fn within(child: &Path, parent: &Path) -> bool {
    let child = key(child);
    let parent = key(parent);
    let parent = parent.trim_end_matches(std::path::MAIN_SEPARATOR);
    child == parent || child.starts_with(&(parent.to_owned() + std::path::MAIN_SEPARATOR_STR))
}
fn resolved(config: &Config, kind: &str) -> PathBuf {
    config
        .overrides
        .get(kind)
        .cloned()
        .unwrap_or_else(|| match kind {
            "state" => config.managed_root.clone(),
            "receipts" => config.managed_root.join("upload-receipts"),
            "newSkill" => config.managed_root.join("skills"),
            "export" => config.managed_root.join("exports"),
            other => config.managed_root.join(other),
        })
}
fn validate(mut config: Config) -> Result<Config, String> {
    config.managed_root = absolute(&config.managed_root)?;
    for (kind, path) in &mut config.overrides {
        if !KINDS.contains(&kind.as_str()) {
            return Err(format!("未知保存位置：{kind}"));
        }
        *path = absolute(path)?;
    }
    // The database may coexist with the root's source files; private trees must
    // remain disjoint so moving one cannot copy another recursively.
    let private = ["webview", "recovery", "receipts", "temp"];
    for (index, a) in private.iter().enumerate() {
        for b in &private[index + 1..] {
            let left = key(&resolved(&config, a));
            let right = key(&resolved(&config, b));
            if left == right
                || left.starts_with(&(right.clone() + std::path::MAIN_SEPARATOR_STR))
                || right.starts_with(&(left + std::path::MAIN_SEPARATOR_STR))
            {
                return Err(format!("{a} 与 {b} 保存目录不能互相包含"));
            }
        }
    }
    Ok(config)
}
fn bootstrap_write(path: &Path, value: &Bootstrap) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("保存位置校验清单过大，未切换位置，原文件已保留".into());
    }
    crate::transaction::atomic_write(path, &bytes)
}
fn digest(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    let mut state = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        state.update(&buffer[..n]);
    }
    Ok(format!("{:x}", state.finalize()))
}
fn enumerate(root: &Path, result: &mut Vec<PathBuf>, skipped: &mut usize) -> Result<(), String> {
    crate::network::check_cancelled()?;
    let meta = match fs::symlink_metadata(root) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    if reparse(&meta) {
        *skipped += 1;
        return Ok(());
    }
    if meta.is_file() {
        result.push(root.into());
        return Ok(());
    }
    if meta.is_dir() {
        for child in fs::read_dir(root).map_err(|e| e.to_string())? {
            enumerate(&child.map_err(|e| e.to_string())?.path(), result, skipped)?;
        }
    }
    Ok(())
}
fn destination_space(path: &Path) -> Result<(String, Option<u64>), String> {
    let mut ancestor = path;
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or("目标路径没有可用的父目录")?;
    }
    if !ancestor.is_dir() {
        return Err("目标的父路径不是目录".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetDiskFreeSpaceExW(
                path: *const u16,
                available: *mut u64,
                total: *mut u64,
                free: *mut u64,
            ) -> i32;
            fn CreateFileW(
                path: *const u16,
                access: u32,
                share: u32,
                security: *const std::ffi::c_void,
                creation: u32,
                flags: u32,
                template: isize,
            ) -> isize;
            fn CloseHandle(handle: isize) -> i32;
        }
        let wide: Vec<u16> = ancestor.as_os_str().encode_wide().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0x0002 | 0x0004 | 0x0040,
                7,
                std::ptr::null(),
                3,
                0x02000000,
                0,
            )
        };
        if handle == -1 {
            return Err(format!(
                "目标目录没有创建或替换文件的权限：{}",
                ancestor.display()
            ));
        }
        unsafe {
            CloseHandle(handle);
        }
        let mut available = 0;
        if unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(format!("无法核实目标可用空间：{}", ancestor.display()));
        }
        let volume = path
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .unwrap_or_default();
        Ok((volume, Some(available)))
    }
    #[cfg(not(windows))]
    {
        if fs::metadata(ancestor)
            .map_err(|e| e.to_string())?
            .permissions()
            .readonly()
        {
            return Err("目标目录为只读".into());
        }
        Ok((key(ancestor), None))
    }
}
fn exclusive_input(source: &Path) -> Result<fs::File, std::io::Error> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options.open(source)
}
fn await_profile_release(profile: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let mut files = Vec::new();
        let mut skipped = 0;
        enumerate(profile, &mut files, &mut skipped)?;
        if skipped > 0 {
            return Err("界面缓存中存在目录链接，未跟随或复制".into());
        }
        let mut waiting = None;
        for path in files {
            if let Err(error) = exclusive_input(&path) {
                if matches!(error.raw_os_error(), Some(32 | 33)) {
                    waiting = Some(path);
                    break;
                }
                // Chromium may remove its own transient files while closing.
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(format!("无法读取界面缓存 {}：{error}", path.display()));
                }
            }
        }
        if let Some(path) = waiting {
            if Instant::now() >= deadline {
                return Err(format!(
                    "界面缓存仍在使用，已保留旧位置，请稍后重试：{}",
                    path.display()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        } else {
            return Ok(());
        }
    }
}
fn project_root(executable: &Path) -> Option<PathBuf> {
    executable
        .parent()?
        .ancestors()
        .take(10)
        .find(|path| {
            path.join("package.json").is_file() && path.join("src-tauri/Cargo.toml").is_file()
        })
        .map(Path::to_path_buf)
}
fn developer_paths(project: &Path) -> Result<Value, String> {
    let config = project.join("developer-paths.json");
    let dependency = if config.exists() {
        let value: Value = serde_json::from_slice(&fs::read(&config).map_err(|e| e.to_string())?)
            .map_err(|e| format!("开发位置配置无效：{e}"))?;
        absolute(Path::new(
            value["dependencyRoot"]
                .as_str()
                .ok_or("开发配置缺少dependencyRoot")?,
        ))?
    } else {
        project.to_owned()
    };
    Ok(
        json!({"projectRoot":project,"dependencyRoot":dependency,"configurationFile":config,"configured":config.exists(),"script":"scripts/build.ps1","sourceMovesAutomatically":false,"paths":[{"kind":"developerBuild","label":"构建产物","path":dependency.join("target")},{"kind":"developerWork","label":"测试、记录与开发工作文件","path":project.join("work")},{"kind":"developerOutputs","label":"本地交付包","path":project.join("outputs")},{"kind":"developerFrontend","label":"前端依赖","path":project.join("node_modules")},{"kind":"developerDist","label":"前端构建","path":project.join("dist")},{"kind":"developerRust","label":"Rust 工具与缓存","path":dependency.join("rust")},{"kind":"developerCompiler","label":"便携编译组件","path":dependency.join("msvc-portable")},{"kind":"developerPnpm","label":"包缓存","path":dependency.join("pnpm-store")},{"kind":"developerNpm","label":"npm 缓存","path":dependency.join("npm-cache")},{"kind":"developerCorepack","label":"Corepack 缓存","path":dependency.join("corepack")},{"kind":"developerCache","label":"其他构建缓存","path":dependency.join("cache")}]}),
    )
}
impl Storage {
    #[cfg(test)]
    pub(crate) fn isolated(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                bootstrap: Bootstrap {
                    format_version: 1,
                    config: Config {
                        managed_root: root,
                        overrides: BTreeMap::new(),
                    },
                    pending: None,
                    last_migration: None,
                },
                preview: None,
                statistics: None,
            }),
            stats_lock: Mutex::new(()),
            bootstrap_path: None,
            source: "fixture".into(),
            locked: false,
            project_root: None,
            developer_snapshot: None,
        })
    }
    pub(crate) fn load(
        base: PathBuf,
        executable: &Path,
        source: &str,
        locked: bool,
    ) -> Result<Arc<Self>, String> {
        let base = if base.is_absolute() {
            base
        } else {
            std::env::current_dir()
                .map_err(|e| e.to_string())?
                .join(base)
        };
        let bootstrap_path = executable
            .parent()
            .ok_or("无法确定程序目录")?
            .join("storage-paths.json");
        let mut bootstrap = if bootstrap_path.exists() {
            let bytes = fs::read(&bootstrap_path).map_err(|e| e.to_string())?;
            if bytes.len() > 8 * 1024 * 1024 {
                return Err("保存位置配置过大，原文件已保留".into());
            }
            let value: Bootstrap = serde_json::from_slice(&bytes)
                .map_err(|e| format!("保存位置配置无效，未使用其他目录：{e}"))?;
            if value.format_version != 1 {
                return Err("无法识别保存位置配置版本".into());
            }
            value
        } else {
            Bootstrap {
                format_version: 1,
                config: Config {
                    managed_root: base.clone(),
                    overrides: BTreeMap::new(),
                },
                pending: None,
                last_migration: None,
            }
        };
        if locked {
            bootstrap.config.managed_root = base;
            bootstrap
                .config
                .overrides
                .retain(|kind, _| !DURABLE.contains(&kind.as_str()));
            bootstrap.pending = None;
        }
        bootstrap.config = validate(bootstrap.config)?;
        if let Some(pending) = bootstrap.pending.clone() {
            let verification = (|| -> Result<(), String> {
                for file in &pending.files {
                    if digest(&file.destination)? != file.digest {
                        return Err(format!(
                            "迁移后文件校验不一致：{}",
                            file.destination.display()
                        ));
                    }
                }
                let database = resolved(&bootstrap.config, "state").join("manager.sqlite3");
                let db = rusqlite::Connection::open_with_flags(
                    database,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .map_err(|e| e.to_string())?;
                let check: String = db
                    .query_row("PRAGMA integrity_check", [], |r| r.get(0))
                    .map_err(|e| e.to_string())?;
                if check != "ok" {
                    return Err("迁移后的数据库检查失败".into());
                }
                Ok(())
            })();
            bootstrap.pending = None;
            bootstrap.last_migration = Some(match verification {
                Ok(()) => {
                    json!({"id":pending.id,"status":"verified","verifiedAt":crate::model::now(),"originalsRetained":true})
                }
                Err(error) => {
                    bootstrap.config = validate(pending.previous)?;
                    json!({"id":pending.id,"status":"rolled_back","error":error,"originalsRetained":true})
                }
            });
            bootstrap_write(&bootstrap_path, &bootstrap)?;
        }
        let has_config = bootstrap_path.exists() && !locked;
        Ok(Arc::new(Self {
            inner: Mutex::new(Inner {
                bootstrap,
                preview: None,
                statistics: None,
            }),
            stats_lock: Mutex::new(()),
            bootstrap_path: Some(bootstrap_path),
            source: if has_config {
                "bootstrap".into()
            } else {
                source.into()
            },
            locked,
            project_root: project_root(executable),
            developer_snapshot: project_root(executable).map(|root| {
                developer_paths(&root)
                    .unwrap_or_else(|error| json!({"projectRoot":root,"error":error}))
            }),
        }))
    }
    pub(crate) fn path(&self, kind: &str) -> Result<PathBuf, String> {
        if !KINDS.contains(&kind) {
            return Err("未知保存位置".into());
        }
        Ok(resolved(
            &self
                .inner
                .lock()
                .map_err(|_| "保存位置锁定失败")?
                .bootstrap
                .config,
            kind,
        ))
    }
    pub(crate) fn snapshot(&self) -> Result<Value, String> {
        let inner = self.inner.lock().map_err(|_| "保存位置锁定失败")?;
        let config = &inner.bootstrap.config;
        let paths: serde_json::Map<String, Value> = KINDS.iter().map(|kind| (kind.to_string(), json!({"effective":resolved(config,kind),"configured":config.overrides.get(*kind),"source":if config.overrides.contains_key(*kind){"override"}else{self.source.as_str()},"locked":self.locked&&DURABLE.contains(kind),"restartRequired":DURABLE.contains(kind)}))).collect();
        let developer = &self.developer_snapshot;
        Ok(
            json!({"managedRoot":config.managed_root,"config":config,"paths":paths,"pending":inner.preview.as_ref().map(|p|json!({"id":p.id,"config":p.to})),"lastMigration":inner.bootstrap.last_migration,"bootstrapPath":self.bootstrap_path,"developer":developer,"systemManaged":["Windows 凭据管理器","系统 WebView2 运行时与系统记录","安装器注册信息与快捷方式","客户端规定的发现入口"],"atomicTemporary":"更新文件时在目标同目录短暂创建原子写临时文件"}),
        )
    }
    pub(crate) fn preview(&self, value: &Value, skills: &[Value]) -> Result<Value, String> {
        let config = validate(
            serde_json::from_value(value.clone()).map_err(|e| format!("保存位置配置无效：{e}"))?,
        )?;
        let from = self
            .inner
            .lock()
            .map_err(|_| "保存位置锁定失败")?
            .bootstrap
            .config
            .clone();
        if self.locked
            && DURABLE
                .iter()
                .any(|k| key(&resolved(&config, k)) != key(&resolved(&from, k)))
        {
            return Err("保存位置由启动参数或环境变量锁定，请先移除该覆盖再修改".into());
        }
        for kind in ["state", "webview", "recovery", "receipts", "temp"] {
            let destination = resolved(&config, kind);
            if skills
                .iter()
                .filter_map(|s| s["localPath"].as_str())
                .any(|root| within(&destination, Path::new(root)))
            {
                return Err("管理数据不能保存在某个 Skill 运行目录中".into());
            }
        }
        let mut changes: Vec<_> = KINDS.iter().filter(|k|key(&resolved(&config,k)) != key(&resolved(&from,k))).map(|k|json!({"kind":k,"from":resolved(&from,k),"to":resolved(&config,k),"restartRequired":DURABLE.contains(k)})).collect();
        let mut volumes: BTreeMap<String, (u64, Option<u64>)> = BTreeMap::new();
        let mut total_files = 0;
        let mut total_bytes = 0;
        for change in &mut changes {
            let kind = change["kind"].as_str().unwrap().to_string();
            let from = PathBuf::from(change["from"].as_str().unwrap());
            let to = PathBuf::from(change["to"].as_str().unwrap());
            let (volume, available) = destination_space(&to)?;
            let mut files = Vec::new();
            let mut skipped = 0;
            if kind == "state" {
                for name in ["manager.sqlite3", "manager.sqlite3-wal"] {
                    enumerate(&from.join(name), &mut files, &mut skipped)?;
                }
            } else if matches!(kind.as_str(), "webview" | "recovery" | "receipts") {
                enumerate(&from, &mut files, &mut skipped)?;
            }
            if skipped > 0 {
                return Err(format!("{kind} 包含目录链接，未跟随该链接，请先检查来源"));
            }
            let bytes = files.iter().try_fold(0u64, |sum, path| {
                fs::symlink_metadata(path)
                    .map(|m| sum.saturating_add(m.len()))
                    .map_err(|e| e.to_string())
            })?;
            change["files"] = json!(files.len());
            change["bytes"] = json!(bytes);
            change["availableBytes"] = json!(available);
            change["writable"] = json!(true);
            total_files += files.len();
            total_bytes += bytes;
            let entry = volumes.entry(volume).or_insert((0, available));
            entry.0 = entry.0.saturating_add(bytes);
        }
        for (volume, (needed, available)) in &volumes {
            if let Some(available) = available {
                let headroom = if *needed > 0 {
                    (*needed / 10).max(16 * 1024 * 1024)
                } else {
                    0
                };
                if *available < needed.saturating_add(headroom) {
                    return Err(format!(
                        "目标 {volume} 空间不足：需要约 {} 字节（含余量），可用 {available} 字节",
                        needed.saturating_add(headroom)
                    ));
                }
            }
        }
        if let Some(path) = &self.bootstrap_path {
            destination_space(path.parent().ok_or("启动配置没有父目录")?)?;
        }
        for change in &changes {
            let kind = change["kind"].as_str().unwrap();
            if !DURABLE.contains(&kind) || kind == "temp" {
                continue;
            }
            let from = PathBuf::from(change["from"].as_str().unwrap());
            let to = PathBuf::from(change["to"].as_str().unwrap());
            if kind != "state" && (within(&to, &from) || within(&from, &to)) {
                return Err("迁移前后目录不能互相包含；请使用独立目标目录".into());
            }
            let target = if kind == "state" {
                to.join("manager.sqlite3")
            } else {
                to
            };
            if target.exists()
                && (target.is_file()
                    || fs::read_dir(&target)
                        .map_err(|e| e.to_string())?
                        .next()
                        .is_some())
            {
                return Err(format!("目标中已有文件，未覆盖：{}", target.display()));
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        let restart_required = changes.iter().any(|v| v["restartRequired"] == true);
        let mut inner = self.inner.lock().map_err(|_| "保存位置锁定失败")?;
        if inner.bootstrap.config != from {
            return Err("保存位置已由另一个窗口改变，请重新预览".into());
        }
        inner.preview = Some(Preview {
            id: id.clone(),
            from,
            to: config,
            created: Instant::now(),
        });
        Ok(
            json!({"id":id,"changes":changes,"files":total_files,"bytes":total_bytes,"spaceChecked":cfg!(windows),"restartRequired":restart_required,"originalsRetained":true,"profileRequiresAllWindowsClosed":true,"scope":"仅复制数据库、WebView、恢复记录和上传凭据；源码、工具链、Skill 原件保持原位"}),
        )
    }
    pub(crate) fn apply_defaults(&self, id: &str) -> Result<Value, String> {
        let mut inner = self.inner.lock().map_err(|_| "保存位置锁定失败")?;
        let preview = inner
            .preview
            .as_ref()
            .filter(|p| p.id == id && p.created.elapsed() < Duration::from_secs(1800))
            .ok_or("保存位置预览已过期，请重新预览")?
            .clone();
        if DURABLE
            .iter()
            .any(|k| key(&resolved(&preview.from, k)) != key(&resolved(&preview.to, k)))
        {
            return Err("这些保存位置需要安全保存并重启后应用".into());
        }
        let mut next = inner.bootstrap.clone();
        next.config = preview.to;
        bootstrap_write(
            self.bootstrap_path
                .as_deref()
                .ok_or("测试环境没有可写入的启动配置")?,
            &next,
        )?;
        inner.bootstrap = next;
        inner.preview = None;
        inner.statistics = None;
        drop(inner);
        self.snapshot()
    }
    pub(crate) fn stats(&self) -> Result<Value, String> {
        let _guard = self.stats_lock.lock().map_err(|_| "统计锁定失败")?;
        if let Some((saved, value)) = &self
            .inner
            .lock()
            .map_err(|_| "保存位置锁定失败")?
            .statistics
        {
            if saved.elapsed() < Duration::from_secs(15) {
                return Ok(value.clone());
            }
        }
        let mut seen = BTreeSet::new();
        let mut rows = Vec::new();
        for kind in KINDS {
            let root = self.path(kind)?;
            let mut files = Vec::new();
            let mut skipped = 0;
            let result = if *kind == "state" {
                for suffix in ["", "-wal", "-shm"] {
                    enumerate(
                        &root.join(format!("manager.sqlite3{suffix}")),
                        &mut files,
                        &mut skipped,
                    )?;
                }
                Ok(())
            } else {
                enumerate(&root, &mut files, &mut skipped)
            };
            let mut bytes = 0;
            let mut count = 0;
            for file in files {
                if seen.insert(key(&file)) {
                    if let Ok(meta) = fs::symlink_metadata(&file) {
                        if meta.is_file() && !reparse(&meta) {
                            bytes += meta.len();
                            count += 1;
                        } else if reparse(&meta) {
                            skipped += 1;
                        }
                    }
                }
            }
            rows.push(json!({"kind":kind,"path":root,"bytes":bytes,"files":count,"skippedLinks":skipped,"error":result.err()}));
        }
        if let Some(project) = &self.project_root {
            let developer = developer_paths(project)?;
            for item in developer["paths"].as_array().into_iter().flatten() {
                let Some(path) = item["path"].as_str() else {
                    continue;
                };
                let mut files = Vec::new();
                let mut skipped = 0;
                let result = enumerate(Path::new(path), &mut files, &mut skipped);
                let mut count = 0;
                let mut bytes = 0u64;
                for file in files {
                    if seen.insert(key(&file)) {
                        if let Ok(meta) = fs::symlink_metadata(&file) {
                            if meta.is_file() && !reparse(&meta) {
                                count += 1;
                                bytes += meta.len();
                            }
                        }
                    }
                }
                rows.push(json!({"kind":item["kind"],"label":item["label"],"path":path,"bytes":bytes,"files":count,"skippedLinks":skipped,"error":result.err(),"developer":true}));
            }
        }
        let value = json!({"checkedAt":crate::model::now(),"rows":rows,"followsLinks":false});
        self.inner
            .lock()
            .map_err(|_| "保存位置锁定失败")?
            .statistics = Some((Instant::now(), value.clone()));
        Ok(value)
    }
    pub(crate) fn migrate(
        &self,
        id: &str,
        database_copy: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(), String> {
        let preview = {
            let inner = self.inner.lock().map_err(|_| "保存位置锁定失败")?;
            inner
                .preview
                .as_ref()
                .filter(|p| {
                    p.id == id
                        && p.created.elapsed() < Duration::from_secs(1800)
                        && p.from == inner.bootstrap.config
                })
                .ok_or("保存位置预览已过期，请重新预览")?
                .clone()
        };
        let bootstrap_path = self
            .bootstrap_path
            .as_deref()
            .ok_or("没有可写入的启动配置")?;
        if key(&resolved(&preview.from, "webview")) != key(&resolved(&preview.to, "webview")) {
            await_profile_release(&resolved(&preview.from, "webview"))?;
        }
        let mut copied = Vec::new();
        let mut database_copy = Some(database_copy);
        for kind in ["state", "recovery", "receipts", "webview"] {
            let from = resolved(&preview.from, kind);
            let to = resolved(&preview.to, kind);
            if key(&from) == key(&to) {
                continue;
            }
            absolute(&to)?;
            if kind == "state" {
                fs::create_dir_all(&to).map_err(|e| e.to_string())?;
                let destination = to.join("manager.sqlite3");
                if destination.exists() {
                    return Err("目标数据库已存在，原数据库未改变".into());
                }
                database_copy.take().ok_or("数据库重复复制")?(&destination)?;
                copied.push(CopyEntry {
                    source: from.join("manager.sqlite3"),
                    bytes: fs::metadata(&destination).map_err(|e| e.to_string())?.len(),
                    digest: digest(&destination)?,
                    destination,
                });
                continue;
            }
            let mut files = Vec::new();
            let mut skipped = 0;
            enumerate(&from, &mut files, &mut skipped)?;
            if skipped > 0 {
                return Err(format!("{kind} 包含目录链接，未迁移或跟随该链接"));
            }
            for source in files {
                let relative = source.strip_prefix(&from).map_err(|e| e.to_string())?;
                let destination = to.join(relative);
                if destination.exists() {
                    return Err(format!(
                        "迁移目标已有文件，已保留：{}",
                        destination.display()
                    ));
                }
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                // Native WebViews have been destroyed. Exclusive source opens on
                // Windows also refuse a still-running profile process.
                let mut input = exclusive_input(&source).map_err(|e| {
                    format!("文件仍在使用，旧位置已保留：{}：{e}", source.display())
                })?;
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)
                    .map_err(|e| e.to_string())?;
                std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
                output.sync_all().map_err(|e| e.to_string())?;
                drop(output);
                drop(input);
                let expected = digest(&source)?;
                if digest(&destination)? != expected {
                    return Err("复制校验不一致，旧位置已保留".into());
                }
                copied.push(CopyEntry {
                    source,
                    bytes: fs::metadata(&destination).map_err(|e| e.to_string())?.len(),
                    digest: expected,
                    destination,
                });
            }
        }
        let mut inner = self.inner.lock().map_err(|_| "保存位置锁定失败")?;
        let mut next = inner.bootstrap.clone();
        next.config = preview.to;
        next.pending = Some(Pending {
            id: preview.id,
            previous: preview.from,
            files: copied,
        });
        bootstrap_write(bootstrap_path, &next)?;
        // Effective paths remain the old paths until this process restarts.
        inner.preview = None;
        Ok(())
    }
    pub(crate) fn rollback_pending(&self) -> Result<(), String> {
        let inner = self.inner.lock().map_err(|_| "保存位置锁定失败")?;
        bootstrap_write(
            self.bootstrap_path.as_deref().ok_or("没有启动配置")?,
            &inner.bootstrap,
        )
    }
    pub(crate) fn record_failure(&self, error: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "保存位置锁定失败")?;
        inner.bootstrap.last_migration =
            Some(json!({"status":"failed","error":error,"originalsRetained":true}));
        bootstrap_write(
            self.bootstrap_path.as_deref().ok_or("没有启动配置")?,
            &inner.bootstrap,
        )
    }
}

pub(crate) fn handle(
    m: &mut crate::model::Manager,
    request: &Value,
) -> Result<Option<Value>, String> {
    Ok(Some(match request["action"].as_str().unwrap_or_default() {
        "storage_get" => json!({"storage":m.storage.snapshot()?}),
        "storage_stats" => m.storage.stats()?,
        "storage_preview" => {
            json!({"preview":m.storage.preview(&request["config"],&m.state.skills)?})
        }
        "storage_apply_defaults" => {
            let storage = m
                .storage
                .apply_defaults(request["previewId"].as_str().ok_or("缺少保存位置预览")?)?;
            m.save()?;
            json!({"storage":storage})
        }
        "storage_resolve_default_path" => {
            let kind = request["kind"].as_str().ok_or("缺少保存位置类型")?;
            if !matches!(kind, "newSkill" | "export") {
                return Err("只能读取下载或导出目录".into());
            }
            let path = absolute(&m.storage.path(kind)?)?;
            json!({"path":path})
        }
        "storage_default_path" => {
            let kind = request["kind"].as_str().ok_or("缺少保存位置类型")?;
            if !matches!(kind, "newSkill" | "export") {
                return Err("只能准备下载或导出目录".into());
            }
            let path = absolute(&m.storage.path(kind)?)?;
            fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            Ok::<_, String>(json!({"path":path}))?
        }
        "storage_open_path" => {
            let path = m
                .storage
                .path(request["kind"].as_str().ok_or("缺少保存位置类型")?)?;
            if !path.is_dir() {
                return Err("该保存目录尚未创建".into());
            }
            absolute(&path)?;
            #[cfg(windows)]
            let mut command = std::process::Command::new("explorer.exe");
            #[cfg(not(windows))]
            let mut command = std::process::Command::new("xdg-open");
            command.arg(&path).spawn().map_err(|e| e.to_string())?;
            json!({"opened":true,"path":path})
        }
        _ => return Ok(None),
    }))
}

/// The selected default is checked again under the actual export task. Merely
/// showing/cancelling a destination choice must not create output directories.
pub(crate) fn default_export_parent(
    storage: &Storage,
    destination: &str,
    create: bool,
) -> Result<(), String> {
    let parent = Path::new(destination)
        .parent()
        .ok_or("导出目标没有父目录")?;
    let selected = absolute(parent)?;
    let current = absolute(&storage.path("export")?)?;
    if key(&selected) != key(&current) {
        return Err("默认导出位置已变化，请重新选择保存位置".into());
    }
    if create {
        fs::create_dir_all(&selected).map_err(|e| e.to_string())?;
        absolute(&selected)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> tempfile::TempDir {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/storage-tests");
        fs::create_dir_all(&parent).unwrap();
        tempfile::tempdir_in(parent).unwrap()
    }
    fn loaded(root: &Path) -> Arc<Storage> {
        fs::create_dir_all(root.join("bin")).unwrap();
        Storage::load(
            root.join("old"),
            &root.join("bin/test.exe"),
            "legacySidecar",
            false,
        )
        .unwrap()
    }
    #[test]
    fn existing_default_paths_stay_in_place_and_do_not_create_bootstrap() {
        let fixture = fixture();
        let storage = loaded(fixture.path());
        assert_eq!(storage.path("state").unwrap(), fixture.path().join("old"));
        assert_eq!(
            storage.path("webview").unwrap(),
            fixture.path().join("old/webview")
        );
        assert_eq!(
            storage.path("receipts").unwrap(),
            fixture.path().join("old/upload-receipts")
        );
        assert!(!fixture.path().join("bin/storage-paths.json").exists());
        assert!(!fixture.path().join("old").exists());
    }
    #[test]
    fn output_defaults_apply_to_shared_service_without_changing_running_data_paths() {
        let fixture = fixture();
        let storage = loaded(fixture.path());
        let clone = storage.clone();
        let config = json!({"managedRoot":fixture.path().join("old"),"overrides":{"export":fixture.path().join("zip"),"newSkill":fixture.path().join("download")}});
        let preview = storage.preview(&config, &[]).unwrap();
        assert_eq!(preview["restartRequired"], false);
        assert_eq!(preview["files"], 0);
        assert!(!fixture.path().join("zip").exists());
        storage
            .apply_defaults(preview["id"].as_str().unwrap())
            .unwrap();
        assert_eq!(clone.path("export").unwrap(), fixture.path().join("zip"));
        assert_eq!(clone.path("state").unwrap(), fixture.path().join("old"));
        let restart = loaded(fixture.path());
        assert_eq!(restart.path("export").unwrap(), fixture.path().join("zip"));
    }
    #[test]
    fn command_line_locks_internal_paths_but_keeps_persisted_output_defaults() {
        let fixture = fixture();
        let storage = loaded(fixture.path());
        let config = json!({"managedRoot":fixture.path().join("old"),"overrides":{"export":fixture.path().join("zip")}});
        let preview = storage.preview(&config, &[]).unwrap();
        storage
            .apply_defaults(preview["id"].as_str().unwrap())
            .unwrap();
        let locked = Storage::load(
            fixture.path().join("explicit"),
            &fixture.path().join("bin/test.exe"),
            "commandLine",
            true,
        )
        .unwrap();
        assert_eq!(
            locked.path("state").unwrap(),
            fixture.path().join("explicit")
        );
        assert_eq!(locked.path("export").unwrap(), fixture.path().join("zip"));
        assert_eq!(locked.snapshot().unwrap()["paths"]["state"]["locked"], true);
        assert_eq!(
            locked.snapshot().unwrap()["paths"]["export"]["locked"],
            false
        );
        assert!(locked
            .preview(
                &json!({"managedRoot":fixture.path().join("elsewhere")}),
                &[]
            )
            .is_err());
    }
    #[test]
    fn migration_copies_only_known_data_verifies_on_next_load_and_keeps_originals() {
        let fixture = fixture();
        let storage = loaded(fixture.path());
        let manager = crate::model::Manager::with_storage(storage.clone()).unwrap();
        fs::write(
            fixture.path().join("old/source-code.txt"),
            b"unrelated source remains",
        )
        .unwrap();
        fs::create_dir_all(fixture.path().join("old/recovery/item")).unwrap();
        fs::write(
            fixture.path().join("old/recovery/item/journal.json"),
            b"retained recovery record",
        )
        .unwrap();
        fs::create_dir_all(fixture.path().join("old/upload-receipts")).unwrap();
        fs::write(
            fixture.path().join("old/upload-receipts/receipt.json"),
            b"retained receipt",
        )
        .unwrap();
        fs::create_dir_all(fixture.path().join("old/webview/profile")).unwrap();
        fs::write(
            fixture.path().join("old/webview/profile/preferences"),
            b"profile bytes",
        )
        .unwrap();
        let preview = storage
            .preview(&json!({"managedRoot":fixture.path().join("next")}), &[])
            .unwrap();
        assert!(preview["bytes"].as_u64().unwrap() > 0);
        assert!(preview["files"].as_u64().unwrap() >= 4);
        assert!(storage
            .apply_defaults(preview["id"].as_str().unwrap())
            .is_err());
        storage
            .migrate(preview["id"].as_str().unwrap(), |dest| {
                manager.copy_database(dest)
            })
            .unwrap();
        assert_eq!(storage.path("state").unwrap(), fixture.path().join("old"));
        assert!(!fixture.path().join("next/source-code.txt").exists());
        assert_eq!(
            fs::read(fixture.path().join("next/webview/profile/preferences")).unwrap(),
            b"profile bytes"
        );
        let restart = loaded(fixture.path());
        assert_eq!(restart.path("state").unwrap(), fixture.path().join("next"));
        assert_eq!(
            restart.snapshot().unwrap()["lastMigration"]["status"],
            "verified"
        );
        assert!(fixture.path().join("old/manager.sqlite3").is_file());
        assert!(fixture.path().join("old/source-code.txt").is_file());
        assert!(crate::model::Manager::with_storage(restart).is_ok());
    }
    #[test]
    fn changed_copy_rolls_back_before_database_open_and_preserves_both_copies() {
        let fixture = fixture();
        let storage = loaded(fixture.path());
        let manager = crate::model::Manager::with_storage(storage.clone()).unwrap();
        let preview = storage
            .preview(&json!({"managedRoot":fixture.path().join("next")}), &[])
            .unwrap();
        storage
            .migrate(preview["id"].as_str().unwrap(), |dest| {
                manager.copy_database(dest)
            })
            .unwrap();
        fs::write(
            fixture.path().join("next/manager.sqlite3"),
            b"damaged copied bytes",
        )
        .unwrap();
        let restart = loaded(fixture.path());
        assert_eq!(restart.path("state").unwrap(), fixture.path().join("old"));
        assert_eq!(
            restart.snapshot().unwrap()["lastMigration"]["status"],
            "rolled_back"
        );
        assert_eq!(
            fs::read(fixture.path().join("next/manager.sqlite3")).unwrap(),
            b"damaged copied bytes"
        );
        assert!(crate::model::Manager::with_storage(restart).is_ok());
    }
    #[test]
    fn target_race_keeps_old_entry_and_partial_copy_without_overwriting_user_file() {
        let fixture = fixture();
        let storage = loaded(fixture.path());
        let manager = crate::model::Manager::with_storage(storage.clone()).unwrap();
        fs::create_dir_all(fixture.path().join("old/recovery")).unwrap();
        fs::write(fixture.path().join("old/recovery/a"), b"old").unwrap();
        let preview = storage
            .preview(&json!({"managedRoot":fixture.path().join("next")}), &[])
            .unwrap();
        fs::create_dir_all(fixture.path().join("next/recovery")).unwrap();
        fs::write(fixture.path().join("next/recovery/a"), b"user file").unwrap();
        assert!(storage
            .migrate(preview["id"].as_str().unwrap(), |dest| manager
                .copy_database(dest))
            .is_err());
        assert!(!fixture.path().join("bin/storage-paths.json").exists());
        assert_eq!(
            fs::read(fixture.path().join("next/recovery/a")).unwrap(),
            b"user file"
        );
        assert!(fixture.path().join("next/manager.sqlite3").exists());
        assert_eq!(storage.path("state").unwrap(), fixture.path().join("old"));
    }
    #[test]
    fn stats_count_only_database_in_state_root_and_deduplicate_output_paths() {
        let fixture = fixture();
        let storage = Storage::isolated(fixture.path().to_owned());
        fs::write(fixture.path().join("source-large.bin"), [0u8; 4096]).unwrap();
        fs::write(fixture.path().join("manager.sqlite3"), b"database").unwrap();
        fs::create_dir_all(fixture.path().join("exports")).unwrap();
        fs::write(fixture.path().join("exports/a.zip"), b"zip bytes").unwrap();
        storage
            .inner
            .lock()
            .unwrap()
            .bootstrap
            .config
            .overrides
            .insert("newSkill".into(), fixture.path().join("exports"));
        let stats = storage.stats().unwrap();
        let rows = stats["rows"].as_array().unwrap();
        assert_eq!(
            rows.iter().find(|r| r["kind"] == "state").unwrap()["bytes"],
            8
        );
        assert_eq!(
            rows.iter().find(|r| r["kind"] == "newSkill").unwrap()["bytes"],
            9
        );
        assert_eq!(
            rows.iter().find(|r| r["kind"] == "export").unwrap()["bytes"],
            0
        );
    }
    #[cfg(windows)]
    #[test]
    fn previews_reject_reparse_targets_and_statistics_do_not_follow_junctions() {
        let fixture = fixture();
        let storage = Storage::isolated(fixture.path().join("data"));
        fs::create_dir_all(fixture.path().join("outside")).unwrap();
        fs::write(fixture.path().join("outside/private.bin"), b"not counted").unwrap();
        fs::create_dir_all(fixture.path().join("data/webview")).unwrap();
        let link = fixture.path().join("data/webview/link");
        junction::create(fixture.path().join("outside"), &link).unwrap();
        let stats = storage.stats().unwrap();
        let row = stats["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == "webview")
            .unwrap();
        assert_eq!(row["bytes"], 0);
        assert_eq!(row["skippedLinks"], 1);
        assert!(storage
            .preview(&json!({"managedRoot":link.join("new")}), &[])
            .is_err());
        // Remove only the synthetic junction itself, preserving its target; the
        // fixture guard owns both and no real client directory is involved.
        junction::delete(&link).unwrap();
    }
    #[test]
    fn preview_rejects_relative_overlap_existing_database_and_skill_nested_targets() {
        let fixture = fixture();
        let storage = loaded(fixture.path());
        assert!(storage
            .preview(&json!({"managedRoot":"relative"}), &[])
            .is_err());
        assert!(storage.preview(&json!({"managedRoot":fixture.path().join("old"),"overrides":{"webview":fixture.path().join("same"),"recovery":fixture.path().join("same/nested")}}),&[]).is_err());
        fs::create_dir_all(fixture.path().join("next")).unwrap();
        fs::write(
            fixture.path().join("next/manager.sqlite3"),
            b"existing data",
        )
        .unwrap();
        assert!(storage
            .preview(&json!({"managedRoot":fixture.path().join("next")}), &[])
            .is_err());
        assert!(storage
            .preview(
                &json!({"managedRoot":fixture.path().join("skill/data")}),
                &[json!({"localPath":fixture.path().join("skill")})]
            )
            .is_err());
        assert_eq!(
            fs::read(fixture.path().join("next/manager.sqlite3")).unwrap(),
            b"existing data"
        );
    }
    #[cfg(windows)]
    #[test]
    fn windows_storage_guards_compare_case_slashes_and_verbatim_prefixes() {
        let fixture = fixture();
        let storage = loaded(fixture.path());
        let root = fixture.path().join("Skill");
        fs::create_dir_all(&root).unwrap();
        let mixed =
            PathBuf::from(root.to_string_lossy().to_uppercase().replace('\\', "/")).join("manager");
        assert!(storage
            .preview(&json!({"managedRoot":mixed}), &[json!({"localPath":root})])
            .unwrap_err()
            .contains("Skill"));
        let extended = PathBuf::from(format!("\\\\?\\{}", root.display())).join("manager");
        assert!(within(&extended, &root));
        assert!(!within(&root.with_file_name("SkillSibling"), &root));
        let source = storage.path("webview").unwrap();
        let nested = PathBuf::from(source.to_string_lossy().to_uppercase().replace('\\', "/"))
            .join("nested");
        assert!(storage
            .preview(
                &json!({"managedRoot":fixture.path().join("old"),"overrides":{"webview":nested}}),
                &[]
            )
            .is_err());
    }
}
