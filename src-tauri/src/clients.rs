//! Client entries refer to the existing runtime folder; no client deployment is copied.
use crate::{
    local,
    model::{now, Manager},
    transaction,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

const CLIENTS: [(&str, &str); 3] = [
    ("codex", ".agents"),
    ("claude", ".claude"),
    ("cursor", ".cursor"),
];

fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("缺少客户端连接参数：{field}"))
}

fn readable(path: &Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            return rest.to_owned();
        }
    }
    value.into_owned()
}

fn key(path: &Path) -> String {
    #[cfg(windows)]
    {
        readable(path).replace('/', "\\").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        readable(path)
    }
}

fn linked(meta: &fs::Metadata) -> bool {
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

fn within(path: &Path, root: &Path) -> bool {
    let path = key(path);
    let root = key(root);
    let separator = if cfg!(windows) { "\\" } else { "/" };
    path == root
        || path.starts_with(&format!(
            "{}{separator}",
            root.trim_end_matches(['/', '\\'])
        ))
}

fn valid_name(name: &str) -> bool {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"|?*\\/".contains(c))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !matches!(
        stem.strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT")),
        Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
    )
}

fn client_folder(client: &str) -> Result<&'static str, String> {
    CLIENTS
        .iter()
        .find(|entry| entry.0 == client)
        .map(|entry| entry.1)
        .ok_or_else(|| "无法识别此客户端。".into())
}

fn home_roots() -> Result<Vec<(String, PathBuf)>, String> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok_or("无法读取用户主目录，请选择客户端目录。")?;
    Ok(CLIENTS
        .iter()
        .map(|(client, folder)| {
            (
                client.to_string(),
                PathBuf::from(&home).join(folder).join("skills"),
            )
        })
        .collect())
}

fn runtime_root(skill: &Value) -> Result<PathBuf, String> {
    let root = local::canonical(Path::new(text(skill, "localPath")?))?;
    if !root.is_dir() {
        return Err("Skill 原文件目录不可用。".into());
    }
    Ok(root)
}

fn destination(skill: &Value, request: &Value) -> Result<PathBuf, String> {
    let path = if let Some(path) = request["targetPath"].as_str().filter(|s| !s.is_empty()) {
        PathBuf::from(path)
    } else {
        let client = text(request, "client")?;
        client_folder(client)?;
        let name = text(skill, "name")?;
        if !valid_name(name) {
            return Err("Skill 名称不能用作客户端文件夹名，请选择完整目标路径。".into());
        }
        home_roots()?
            .into_iter()
            .find(|entry| entry.0 == client)
            .unwrap()
            .1
            .join(name)
    };
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(valid_name)
    {
        return Err("请选择不含路径跳转或特殊设备名称的完整客户端路径。".into());
    }
    let parent = path.parent().ok_or("客户端目标没有父目录。")?;
    transaction::verify_root(parent)
        .map_err(|e| format!("客户端目标目录不可安全使用：{}；{e}", readable(parent)))?;
    Ok(path)
}

fn conflict(path: &Path, expected: &Path, reason: &str) -> String {
    let actual = fs::read_link(path)
        .or_else(|_| fs::canonicalize(path))
        .map(|path| readable(&path))
        .unwrap_or_else(|_| "无法解析（可能为断开的链接）".into());
    format!("{reason}，现有文件和链接已保留。\n客户端位置：{}\n当前目标：{actual}\n预期 Skill 原目录：{}", readable(path), readable(expected))
}

fn inspect(path: &Path, expected: &Path) -> Result<&'static str, String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("无法读取客户端位置 {}：{e}", readable(path)))?;
    let actual = fs::canonicalize(path)
        .map_err(|_| conflict(path, expected, "客户端位置不可读取或链接已断开"))?;
    if key(&actual) != key(expected) {
        return Err(conflict(path, expected, "客户端位置已被其他目录占用"));
    }
    if linked(&meta) {
        Ok("link")
    } else if meta.is_dir() && key(path) == key(expected) {
        Ok("direct")
    } else {
        Err(conflict(
            path,
            expected,
            "客户端位置不是可接纳的同源目录链接",
        ))
    }
}

fn check_record(connection: &mut Value) {
    if connection["origin"].as_str().is_none() {
        connection["origin"] = json!("legacy");
    }
    connection["checkedAt"] = json!(now());
    let result = (|| {
        let path = Path::new(text(connection, "path")?);
        let expected = local::canonical(Path::new(text(connection, "target")?))?;
        inspect(path, &expected)
    })();
    match result {
        Ok(kind) => {
            connection["kind"] = json!(kind);
            connection["health"] = json!("ready");
            connection["linked"] = json!(true);
            connection["error"] = Value::Null;
        }
        Err(error) => {
            let missing = connection["path"].as_str().is_some_and(|path| {
                fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            });
            connection["health"] = json!(if missing { "missing" } else { "changed" });
            connection["linked"] = json!(false);
            connection["verified"] = json!(false);
            connection["error"] = json!(error);
        }
    }
}

fn content_fingerprint(root: &Path) -> Result<String, String> {
    let inventory = local::files(root)?;
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&inventory).map_err(|e| e.to_string())?)
    ))
}

fn update_usage(connection: &mut Value, fingerprint: Option<&Result<String, String>>) {
    if !connection["usageConfirmation"].is_object() {
        connection["usageStatus"] = json!("unconfirmed");
        return;
    }
    connection["usageStatus"] = json!(match fingerprint {
        Some(Ok(current))
            if connection["health"] == "ready"
                && connection["usageConfirmation"]["client"] == connection["client"] =>
        {
            if connection["usageConfirmation"]["contentFingerprint"] == *current {
                "current"
            } else {
                "stale"
            }
        }
        _ => "unavailable",
    });
}

fn repair_eligibility(connection: &Value, skill: &Value) -> Result<(), String> {
    if connection["origin"] != "created" || connection["kind"] == "direct" {
        return Err("仅自动恢复管理器创建的目录链接。".into());
    }
    if skill["readOnly"] == true {
        return Err("应用或插件托管的入口由所属应用管理。".into());
    }
    let root = runtime_root(skill)?;
    if key(&local::canonical(Path::new(text(connection, "target")?))?) != key(&root) {
        return Err("Skill 原目录已变化，请检查后重新连接。".into());
    }
    let path = destination(
        skill,
        &json!({"client":connection["client"],"targetPath":connection["path"]}),
    )?;
    if within(&path, &root) || within(&root, &path) {
        return Err("客户端入口与原目录不能互相嵌套。".into());
    }
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err("客户端位置已有内容，不自动替换。".into()),
        Err(error) => Err(format!("无法核对客户端入口：{error}")),
    }
}

fn ignored(manager: &Manager, client: &str, path: &Path, target: &Path) -> bool {
    manager.state.settings["ignoredClientConnections"]
        .as_array()
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry["client"] == client
                    && entry["path"]
                        .as_str()
                        .is_some_and(|value| key(Path::new(value)) == key(path))
                    && entry["target"]
                        .as_str()
                        .is_some_and(|value| key(Path::new(value)) == key(target))
            })
        })
}

fn clear_ignored(manager: &mut Manager, client: &str, path: &Path, target: &Path) {
    if let Some(entries) = manager.state.settings["ignoredClientConnections"].as_array_mut() {
        entries.retain(|entry| {
            !(entry["client"] == client
                && entry["path"]
                    .as_str()
                    .is_some_and(|value| key(Path::new(value)) == key(path))
                && entry["target"]
                    .as_str()
                    .is_some_and(|value| key(Path::new(value)) == key(target)))
        });
    }
}

/// Updates observations in memory only. The caller persists its larger refresh transaction.
pub(crate) fn refresh(manager: &mut Manager) -> Result<(), String> {
    refresh_roots(manager, &home_roots()?)
}

fn refresh_roots(manager: &mut Manager, roots: &[(String, PathBuf)]) -> Result<(), String> {
    let mut fingerprints = BTreeMap::<String, Result<String, String>>::new();
    for connection in &mut manager.state.connections {
        check_record(connection);
        connection["autoRepairable"] = json!(false);
        connection["repairReason"] = Value::Null;
        let skill = manager
            .state
            .skills
            .iter()
            .find(|skill| skill["id"] == connection["skillId"]);
        let root = skill.and_then(|skill| runtime_root(skill).ok());
        if let Some(root) = &root {
            // A current registered source plus an actual same-target link is sufficient
            // to repair a stale management target; no filesystem content is replaced.
            let path = connection["path"].as_str().map(PathBuf::from);
            match path
                .as_deref()
                .ok_or_else(|| "连接记录缺少客户端位置。".to_owned())
                .and_then(|path| inspect(path, root))
            {
                Ok(kind) => {
                    connection["target"] = json!(readable(root));
                    connection["kind"] = json!(kind);
                    connection["health"] = json!("ready");
                    connection["linked"] = json!(true);
                    connection["error"] = Value::Null;
                }
                Err(error) => {
                    connection["linked"] = json!(false);
                    connection["verified"] = json!(false);
                    connection["health"] = json!(if path
                        .as_deref()
                        .is_some_and(|path| fs::symlink_metadata(path)
                            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound))
                    {
                        "missing"
                    } else {
                        "changed"
                    });
                    connection["error"] = json!(error);
                }
            }
            if connection["usageConfirmation"].is_object() && connection["health"] == "ready" {
                let fingerprint = fingerprints
                    .entry(key(root))
                    .or_insert_with(|| content_fingerprint(root));
                update_usage(connection, Some(fingerprint));
            } else {
                update_usage(connection, None);
            }
        } else {
            update_usage(connection, None);
        }
        if let Some(skill) = skill {
            if connection["health"] == "missing" && repair_eligibility(connection, skill).is_ok() {
                let ignored = manager.state.settings["ignoredClientConnections"]
                    .as_array()
                    .is_some_and(|items| {
                        items.iter().any(|item| {
                            item["client"] == connection["client"]
                                && item["path"]
                                    .as_str()
                                    .zip(connection["path"].as_str())
                                    .is_some_and(|(left, right)| {
                                        key(Path::new(left)) == key(Path::new(right))
                                    })
                                && item["target"]
                                    .as_str()
                                    .zip(connection["target"].as_str())
                                    .is_some_and(|(left, right)| {
                                        key(Path::new(left)) == key(Path::new(right))
                                    })
                        })
                    });
                if !ignored {
                    connection["autoRepairable"] = json!(true);
                    connection["repairReason"] =
                        json!("管理器创建的入口缺失，接入检查后将尝试安全恢复。");
                }
            }
        }
    }
    let skills: Vec<_> = manager
        .state
        .skills
        .iter()
        .filter_map(|skill| runtime_root(skill).ok().map(|root| (skill.clone(), root)))
        .collect();
    let mut known: BTreeSet<_> = manager
        .state
        .connections
        .iter()
        .filter_map(|connection| connection["path"].as_str().map(|path| key(Path::new(path))))
        .collect();
    let mut issues = vec![];
    for (client, root) in roots {
        let children = match fs::read_dir(root) {
            Ok(children) => children,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                issues.push(json!({"client":client,"path":readable(root),"message":format!("无法读取客户端目录：{e}")}));
                continue;
            }
        };
        for child in children {
            let child = match child {
                Ok(child) => child,
                Err(e) => {
                    issues.push(json!({"client":client,"path":readable(root),"message":format!("无法检查客户端条目：{e}")}));
                    continue;
                }
            };
            let path = child.path();
            if known.contains(&key(&path)) {
                continue;
            }
            let actual = match fs::canonicalize(&path) {
                Ok(actual) => actual,
                Err(_) => continue,
            };
            if ignored(manager, client, &path, &actual) {
                continue;
            }
            if let Some((skill, expected)) = skills
                .iter()
                .find(|(_, expected)| key(&actual) == key(expected))
            {
                let kind = match inspect(&path, expected) {
                    Ok(kind) => kind,
                    Err(_) => continue,
                };
                let mut connection = json!({"id":manager.id(),"skillId":skill["id"],"client":client,"path":readable(&path),"target":readable(expected),"origin":"existing","kind":kind,"discovered":true,"linked":true,"verified":false});
                check_record(&mut connection);
                manager.state.connections.push(connection);
                known.insert(key(&path));
            }
        }
    }
    manager.state.settings["clientIssues"] = json!(issues);
    Ok(())
}

fn create_link(source: &Path, path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        junction::create(source, path)
            .map_err(|e| format!("无法创建客户端目录链接 {}：{e}", readable(path)))
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, path)
            .map_err(|e| format!("无法创建客户端目录链接 {}：{e}", readable(path)))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (source, path);
        Err("当前平台尚不支持目录链接。".into())
    }
}

fn verify_link(path: &Path, expected: &Path) -> Result<(), String> {
    transaction::verify_root(path.parent().ok_or("客户端位置没有父目录。")?)?;
    if inspect(path, expected)? != "link" {
        return Err(conflict(path, expected, "此位置已不是目录链接，不能解除"));
    }
    Ok(())
}

fn remove_link(path: &Path, expected: &Path) -> Result<(), String> {
    verify_link(path, expected)?;
    #[cfg(windows)]
    {
        fs::remove_dir(path).map_err(|e| format!("无法解除目录链接 {}：{e}", readable(path)))
    }
    #[cfg(unix)]
    {
        fs::remove_file(path).map_err(|e| format!("无法解除目录链接 {}：{e}", readable(path)))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (path, expected);
        Err("当前平台尚不支持解除目录链接。".into())
    }
}

fn connect(manager: &mut Manager, request: &Value) -> Result<Value, String> {
    let skill = manager.skill(text(request, "skillId")?)?;
    if skill["readOnly"] == true {
        return Err("应用或插件托管的只读 Skill 不能由此管理器部署。".into());
    }
    let client = text(request, "client")?;
    client_folder(client)?;
    let root = runtime_root(&skill)?;
    local::files(&root)?;
    let path = destination(&skill, request)?;
    let index = manager.state.connections.iter().position(|connection| {
        connection["path"]
            .as_str()
            .is_some_and(|current| key(Path::new(current)) == key(&path))
    });
    if let Some(index) = index {
        let connection = &manager.state.connections[index];
        if connection["skillId"] != skill["id"] || connection["client"] != client {
            return Err(conflict(&path, &root, "该客户端位置已登记给其他连接"));
        }
    }
    let (origin, kind, created) = match fs::symlink_metadata(&path) {
        Ok(_) => {
            let kind = inspect(&path, &root)?;
            let origin = index
                .and_then(|index| manager.state.connections[index]["origin"].as_str())
                .filter(|origin| *origin == "created")
                .unwrap_or("existing")
                .to_owned();
            (origin, kind, false)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if within(&path, &root) || within(&root, &path) {
                return Err("客户端链接不能嵌套在 Skill 原目录内，也不能包含原目录。".into());
            }
            let parent = path.parent().ok_or("客户端目标没有父目录。")?;
            fs::create_dir_all(parent)
                .map_err(|e| format!("无法创建客户端目录 {}：{e}", readable(parent)))?;
            transaction::verify_root(parent)?;
            create_link(&root, &path)?;
            if let Err(e) = verify_link(&path, &root) {
                return Err(format!("新建链接未通过核验，未登记连接：{e}"));
            }
            ("created".to_owned(), "link", true)
        }
        Err(e) => return Err(format!("无法检查客户端位置 {}：{e}", readable(&path))),
    };
    let verified = !created
        && index.is_some_and(|index| {
            manager.state.connections[index]["verified"] == true
                && manager.state.connections[index]["linked"] == true
        });
    let previous = manager.state.clone();
    clear_ignored(manager, client, &path, &root);
    let mut connection = json!({"id":index.map(|index| manager.state.connections[index]["id"].clone()).unwrap_or_else(|| json!(manager.id())),"skillId":skill["id"],"client":client,"path":readable(&path),"target":readable(&root),"origin":origin,"kind":kind,"discovered":true,"linked":true,"verified":verified});
    if let Some(index) = index {
        if manager.state.connections[index]["usageConfirmation"].is_object() {
            connection["usageConfirmation"] =
                manager.state.connections[index]["usageConfirmation"].clone();
        }
    }
    check_record(&mut connection);
    let fingerprint = connection["usageConfirmation"]
        .is_object()
        .then(|| content_fingerprint(&root));
    update_usage(&mut connection, fingerprint.as_ref());
    if let Some(index) = index {
        manager.state.connections[index] = connection;
    } else {
        manager.state.connections.push(connection);
    }
    if let Err(e) = manager.save() {
        manager.state = previous;
        if created {
            let rollback = remove_link(&path, &root);
            return Err(format!(
                "无法保存客户端连接：{e}；本次新建链接回退：{}",
                rollback.err().unwrap_or_else(|| "已完成".into())
            ));
        }
        return Err(format!("无法保存客户端连接：{e}；已有链接未修改。"));
    }
    Ok(manager.snapshot())
}

fn connection(manager: &Manager, id: &str) -> Result<Value, String> {
    manager
        .state
        .connections
        .iter()
        .find(|connection| connection["id"] == id)
        .cloned()
        .ok_or_else(|| "客户端连接记录已不存在。".into())
}

fn confirm_usage(manager: &mut Manager, request: &Value) -> Result<Value, String> {
    let id = text(request, "connectionId")?;
    let mut record = connection(manager, id)?;
    let skill = manager.skill(text(&record, "skillId")?)?;
    let root = runtime_root(&skill)?;
    let kind = inspect(Path::new(text(&record, "path")?), &root)?;
    let fingerprint = content_fingerprint(&root)?;
    let previous = manager.state.clone();
    record["target"] = json!(readable(&root));
    record["kind"] = json!(kind);
    record["health"] = json!("ready");
    record["linked"] = json!(true);
    record["error"] = Value::Null;
    record["checkedAt"] = json!(now());
    record["usageConfirmation"] =
        json!({"client":record["client"],"confirmedAt":now(),"contentFingerprint":fingerprint});
    record["usageStatus"] = json!("current");
    if let Some(target) = manager
        .state
        .connections
        .iter_mut()
        .find(|entry| entry["id"] == id)
    {
        *target = record;
    }
    if let Err(error) = manager.save() {
        manager.state = previous;
        return Err(format!("使用记录未保存：{error}"));
    }
    Ok(manager.snapshot())
}

fn repair_one(manager: &mut Manager, id: &str) -> Result<Value, String> {
    let record = connection(manager, id)?;
    let skill = manager.skill(text(&record, "skillId")?)?;
    let root = runtime_root(&skill)?;
    let path = Path::new(text(&record, "path")?);
    let client = text(&record, "client")?;
    if ignored(manager, client, path, &root) {
        return Err("此入口已停止管理，未自动恢复。".into());
    }
    if inspect(path, &root).is_ok() {
        return connect(
            manager,
            &json!({"skillId":skill["id"],"client":client,"targetPath":readable(path)}),
        );
    }
    repair_eligibility(&record, &skill)?;
    // This action is classified as an exclusive write by the task service. Read
    // refreshes only advertise candidates and never create an entry themselves.
    connect(
        manager,
        &json!({"skillId":skill["id"],"client":client,"targetPath":readable(path)}),
    )
}

fn repair_connections(manager: &mut Manager, request: &Value) -> Result<Value, String> {
    let ids: Vec<String> = if let Some(ids) = request["connectionIds"].as_array() {
        ids.iter()
            .map(|id| {
                id.as_str()
                    .map(str::to_owned)
                    .ok_or("连接标识无效".to_owned())
            })
            .collect::<Result<_, _>>()?
    } else {
        manager
            .state
            .connections
            .iter()
            .filter(|record| record["autoRepairable"] == true)
            .filter_map(|record| record["id"].as_str().map(str::to_owned))
            .collect()
    };
    let mut results = vec![];
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id.clone()) {
            continue;
        }
        let record = connection(manager, &id).ok();
        let label = record
            .as_ref()
            .and_then(|record| record["skillId"].as_str())
            .and_then(|id| manager.skill(id).ok())
            .and_then(|skill| {
                skill["displayName"]
                    .as_str()
                    .or_else(|| skill["name"].as_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| id.clone());
        crate::tasks::report_progress(
            "正在安全恢复客户端入口",
            Some(results.len()),
            Some(seen.len().max(results.len() + 1)),
        );
        results.push(match repair_one(manager, &id) {
            Ok(_) => json!({"id":id,"label":label,"success":true}),
            Err(error) => json!({"id":id,"label":label,"success":false,"error":error}),
        });
    }
    // Refresh only records here; do not scan another process's home directories.
    let previous = manager.state.clone();
    let previous_issues = manager.state.settings["clientIssues"].clone();
    let refreshed = refresh_roots(manager, &[]).and_then(|_| {
        manager.state.settings["clientIssues"] = previous_issues;
        manager.save()
    });
    if let Err(error) = refreshed {
        manager.state = previous;
        results.push(json!({"id":"repair-check","label":"恢复后的接入检查","success":false,"error":format!("检查记录未保存：{error}；前面已成功的入口恢复保持。请重新检查。") }));
    }
    let mut value = manager.snapshot();
    value["failed"] = json!(results
        .iter()
        .filter(|item| item["success"] == false)
        .count());
    value["connectionResults"] = json!(results);
    Ok(value)
}

fn remove_record(manager: &mut Manager, id: &str) -> Result<Value, String> {
    let previous = manager.state.clone();
    let connection = connection(manager, id)?;
    {
        let path = Path::new(text(&connection, "path")?);
        let target = Path::new(text(&connection, "target")?);
        let target = local::canonical(target).unwrap_or_else(|_| target.to_path_buf());
        let client = text(&connection, "client")?;
        if !ignored(manager, client, path, &target) {
            if !manager.state.settings["ignoredClientConnections"].is_array() {
                manager.state.settings["ignoredClientConnections"] = json!([]);
            }
            manager.state.settings["ignoredClientConnections"]
                .as_array_mut()
                .unwrap()
                .push(json!({"client":client,"path":readable(path),"target":readable(&target)}));
        }
    }
    manager
        .state
        .connections
        .retain(|connection| connection["id"] != id);
    if let Err(e) = manager.save() {
        manager.state = previous;
        return Err(format!("无法移除客户端管理记录：{e}"));
    }
    Ok(manager.snapshot())
}

// Bind a destructive preview to the link object as well as its destination. Replacing
// a link with another link to the same source must invalidate the original preview.
fn file_identity(path: &Path) -> Result<String, String> {
    #[cfg(windows)]
    {
        use std::{
            ffi::c_void,
            os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
        };
        #[repr(C)]
        #[derive(Default)]
        struct FileTime {
            low: u32,
            high: u32,
        }
        #[repr(C)]
        #[derive(Default)]
        struct FileInfo {
            attributes: u32,
            created: FileTime,
            accessed: FileTime,
            written: FileTime,
            volume: u32,
            size_high: u32,
            size_low: u32,
            links: u32,
            index_high: u32,
            index_low: u32,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetFileInformationByHandle(file: *mut c_void, info: *mut FileInfo) -> i32;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .share_mode(7)
            .custom_flags(0x0020_0000 | 0x0200_0000)
            .open(path)
            .map_err(|e| format!("无法核验链接身份 {}：{e}", readable(path)))?;
        let mut info = FileInfo::default();
        // The handle opens the reparse point itself, without following its target.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(format!(
                "无法核验链接身份：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(format!(
            "{}:{}:{}:{}:{}",
            info.volume, info.index_high, info.index_low, info.created.high, info.created.low
        ))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        Ok(format!(
            "{}:{}:{}:{}",
            meta.dev(),
            meta.ino(),
            meta.ctime(),
            meta.ctime_nsec()
        ))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = path;
        Err("当前平台尚不支持安全解除已有链接。".into())
    }
}

fn link_identity(path: &Path, expected: &Path) -> Result<Value, String> {
    verify_link(path, expected)?;
    let target = fs::read_link(path).map_err(|e| format!("无法读取目录链接目标：{e}"))?;
    Ok(
        json!({"fileId":file_identity(path)?,"rawTarget":target.to_string_lossy(),"resolvedTarget":key(&local::canonical(path)?)}),
    )
}

fn connection_binding(connection: &Value) -> Value {
    json!({"id":connection["id"],"skillId":connection["skillId"],"client":connection["client"],"path":connection["path"],"target":connection["target"],"origin":connection["origin"],"kind":connection["kind"]})
}

fn unlink_record(
    manager: &mut Manager,
    connection: &Value,
    identity: &Value,
) -> Result<Value, String> {
    let path = Path::new(text(connection, "path")?);
    let expected = local::canonical(Path::new(text(connection, "target")?))?;
    if link_identity(path, &expected)? != *identity {
        return Err("客户端链接自预览后已被替换或改动，请重新预览；现有内容已保留。".into());
    }
    let previous = manager.state.clone();
    manager
        .state
        .connections
        .retain(|entry| entry["id"] != connection["id"]);
    if let Err(e) = manager.save() {
        manager.state = previous;
        return Err(format!("无法保存断开操作：{e}；目录链接未修改。"));
    }
    // Save first: a failed database write must never remove a pre-existing link.
    let result = (|| {
        if link_identity(path, &expected)? != *identity {
            return Err("客户端链接已发生变化，请重新检查。".into());
        }
        remove_link(path, &expected)
    })();
    if let Err(e) = result {
        manager.state = previous;
        let restore = manager.save();
        return Err(format!(
            "未能解除客户端链接：{e}；管理记录恢复：{}",
            restore.err().unwrap_or_else(|| "已完成".into())
        ));
    }
    Ok(manager.snapshot())
}

fn disconnect(manager: &mut Manager, request: &Value) -> Result<Value, String> {
    let id = text(request, "connectionId")?;
    let connection = connection(manager, id)?;
    if connection["origin"] != "created" || connection["kind"] == "direct" {
        return remove_record(manager, id);
    }
    let path = Path::new(text(&connection, "path")?);
    if fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return remove_record(manager, id);
    }
    let skill = manager.skill(text(&connection, "skillId")?)?;
    if skill["readOnly"] == true {
        return Err("此 Skill 已由应用或插件托管，请通过所属应用管理其入口。".into());
    }
    let expected = local::canonical(Path::new(text(&connection, "target")?))?;
    if key(&runtime_root(&skill)?) != key(&expected) {
        return Err("连接记录的原目录与当前 Skill 不一致，请重新检查；目录链接已保留。".into());
    }
    let identity = link_identity(path, &expected)?;
    unlink_record(manager, &connection, &identity)
}

fn management_entry(manager: &Manager, id: &str, mode: &str) -> Result<Value, String> {
    let record = connection(manager, id)?;
    let skill = manager.skill(text(&record, "skillId")?).ok();
    let label = skill
        .as_ref()
        .and_then(|skill| {
            skill["displayName"]
                .as_str()
                .or_else(|| skill["name"].as_str())
        })
        .unwrap_or("已移除的 Skill");
    let mut entry = json!({"connectionId":record["id"],"skillId":record["skillId"],"client":record["client"],"label":label,"path":record["path"],"target":record["target"],"mode":mode,"canApply":true,"binding":connection_binding(&record)});
    if mode == "stop" {
        return Ok(entry);
    }
    let skill = skill.ok_or("Skill 管理记录已不存在，可仅停止管理此入口。")?;
    if skill["readOnly"] == true {
        return Err("此 Skill 由应用或插件托管，请通过所属应用管理其入口。".into());
    }
    if record["kind"] == "direct" {
        return Err("此客户端直接使用 Skill 原目录，可仅停止管理，不能删除原目录。".into());
    }
    let expected = runtime_root(&skill)?;
    if key(&local::canonical(Path::new(text(&record, "target")?))?) != key(&expected) {
        return Err("连接记录的原目录与当前 Skill 不一致，请重新检查。".into());
    }
    entry["target"] = json!(readable(&expected));
    entry["linkIdentity"] = link_identity(Path::new(text(&record, "path")?), &expected)?;
    Ok(entry)
}

fn preview_disconnect(manager: &mut Manager, request: &Value) -> Result<Value, String> {
    let mode = request["mode"].as_str().unwrap_or("disconnect");
    if !matches!(mode, "disconnect" | "stop") {
        return Err("连接管理方式无效。".into());
    }
    let batch = request["connectionIds"].is_array();
    let ids: Vec<String> = if let Some(ids) = request["connectionIds"].as_array() {
        ids.iter()
            .map(|id| {
                id.as_str()
                    .map(str::to_owned)
                    .ok_or("连接标识无效".to_owned())
            })
            .collect::<Result<_, _>>()?
    } else {
        vec![text(request, "connectionId")?.to_owned()]
    };
    if ids.is_empty() {
        return Err("请选择要管理的连接。".into());
    }
    let mut entries = vec![];
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id.clone()) {
            continue;
        }
        let entry = match management_entry(manager, &id, mode) {
            Ok(entry) => entry,
            Err(error) if batch => {
                let record = connection(manager, &id).unwrap_or(Value::Null);
                json!({"connectionId":id,"skillId":record["skillId"],"client":record["client"],"label":manager.skill(record["skillId"].as_str().unwrap_or("")).ok().and_then(|skill| skill["displayName"].as_str().or_else(|| skill["name"].as_str()).map(str::to_owned)).unwrap_or_else(|| "连接记录".into()),"path":record["path"],"target":record["target"],"mode":mode,"canApply":false,"error":error})
            }
            Err(error) => return Err(error),
        };
        entries.push(entry);
    }
    let id = manager.id();
    let mut preview = json!({"id":id,"kind":"disconnect","mode":mode,"summary":if mode == "stop" {"仅停止管理，保留客户端入口与 Skill 原文件。"} else {"仅解除所列客户端目录链接，Skill 原文件全部保留。"},"files":[],"canApply":entries.iter().any(|entry| entry["canApply"] == true),"warnings":[],"entries":entries,"createdAt":chrono::Utc::now().timestamp()});
    if preview["entries"]
        .as_array()
        .is_some_and(|entries| entries.len() == 1)
    {
        for field in [
            "connectionId",
            "skillId",
            "path",
            "target",
            "binding",
            "linkIdentity",
        ] {
            preview[field] = preview["entries"][0][field].clone();
        }
    }
    manager.previews.insert(id, preview.clone());
    Ok(preview)
}

fn apply_disconnect(manager: &mut Manager, request: &Value) -> Result<Value, String> {
    let preview = manager
        .previews
        .remove(text(request, "previewId")?)
        .ok_or("解除链接预览已失效，请重新预览。")?;
    if preview["kind"] != "disconnect" {
        return Err("此预览不能用于解除客户端链接。".into());
    }
    if preview["createdAt"]
        .as_i64()
        .is_none_or(|created| chrono::Utc::now().timestamp() - created > 600)
    {
        return Err("解除链接预览已过期，请重新预览。".into());
    }
    let entries = preview["entries"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![preview.clone()]);
    let mut results = vec![];
    let single = entries.len() == 1;
    let total = entries.len();
    for entry in entries {
        let result = (|| {
            if entry["canApply"] == false {
                return Err(entry["error"].as_str().unwrap_or("此项不可应用").to_owned());
            }
            let record = connection(manager, text(&entry, "connectionId")?)?;
            if connection_binding(&record) != entry["binding"] {
                return Err("客户端连接记录已改变，请重新预览。".into());
            }
            if entry["mode"] == "stop" {
                return remove_record(manager, text(&record, "id")?);
            }
            let skill = manager.skill(text(&record, "skillId")?)?;
            if skill["readOnly"] == true {
                return Err("此 Skill 已变为应用托管的只读项目，不能解除入口。".into());
            }
            let expected = runtime_root(&skill)?;
            if key(&expected) != key(Path::new(text(&entry, "target")?)) {
                return Err("Skill 原目录已变化，请重新预览。".into());
            }
            unlink_record(manager, &record, &entry["linkIdentity"])
        })();
        // Keep the original single-item error contract; a batch records every
        // independent outcome and never rolls back already successful entries.
        if single {
            if let Err(error) = &result {
                return Err(error.clone());
            }
        }
        results.push(match result { Ok(_) => json!({"id":entry["connectionId"],"label":entry["label"],"success":true}), Err(error) => json!({"id":entry["connectionId"],"label":entry["label"],"success":false,"error":error}) });
        crate::tasks::report_progress("正在逐项管理客户端入口", Some(results.len()), Some(total));
    }
    let mut snapshot = manager.snapshot();
    snapshot["failed"] = json!(results
        .iter()
        .filter(|item| item["success"] == false)
        .count());
    snapshot["connectionResults"] = json!(results);
    snapshot["mode"] = preview["mode"].clone();
    Ok(snapshot)
}

pub(crate) fn handle(manager: &mut Manager, request: &Value) -> Result<Option<Value>, String> {
    let result = match request["action"].as_str().unwrap_or("") {
        "connect_batch" => {
            let client = text(request, "client")?;
            let ids = request["skillIds"]
                .as_array()
                .ok_or("请选择要连接的 Skill")?;
            if ids.is_empty() {
                return Err("请选择要连接的 Skill".into());
            }
            let mut results = Vec::new();
            let mut seen = BTreeSet::new();
            for id in ids {
                let id = id.as_str().ok_or("Skill 标识无效")?;
                if !seen.insert(id) {
                    continue;
                }
                let label = manager
                    .skill(id)
                    .ok()
                    .and_then(|skill| {
                        skill["displayName"]
                            .as_str()
                            .or_else(|| skill["name"].as_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| id.into());
                crate::tasks::report_progress(
                    "正在逐项连接客户端",
                    Some(results.len()),
                    Some(ids.len()),
                );
                let mut item = json!({"skillId":id,"client":client});
                if let Some(path) = request["targetPaths"][id].as_str() {
                    item["targetPath"] = json!(path);
                }
                let result = match connect(manager, &item) {
                    Ok(_) => json!({"id":id,"label":label,"success":true}),
                    Err(error) => json!({"id":id,"label":label,"success":false,"error":error}),
                };
                results.push(result);
            }
            crate::tasks::report_progress(
                "客户端批量连接完成",
                Some(results.len()),
                Some(results.len()),
            );
            let failed = results
                .iter()
                .filter(|item| item["success"] == false)
                .count();
            let mut value = manager.snapshot();
            value["connectionResults"] = json!(results);
            value["failed"] = json!(failed);
            value
        }
        "connect" => connect(manager, request)?,
        "disconnect" => disconnect(manager, request)?,
        "remove_connection_record" => remove_record(manager, text(request, "connectionId")?)?,
        "preview_disconnect" => preview_disconnect(manager, request)?,
        "apply_disconnect" => apply_disconnect(manager, request)?,
        "confirm_client_usage" => confirm_usage(manager, request)?,
        "repair_client_connections" => repair_connections(manager, request)?,
        "refresh_clients" => {
            let previous = manager.state.clone();
            if let Err(e) = refresh(manager).and_then(|_| manager.save()) {
                manager.state = previous;
                return Err(e);
            }
            manager.snapshot()
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/client-tests");
        fs::create_dir_all(&parent).unwrap();
        tempfile::Builder::new()
            .prefix("fixture-")
            .tempdir_in(parent)
            .unwrap()
    }

    fn setup(path: &Path) -> (Manager, PathBuf, String) {
        let root = path.join("source");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: fixture\ndescription: Synthetic client fixture\n---\nFixture.\n",
        )
        .unwrap();
        let mut manager = Manager::new(path.join("state")).unwrap();
        local::handle(
            &mut manager,
            &json!({"action":"add_local","paths":[readable(&root)]}),
        )
        .unwrap();
        let id = manager.state.skills[0]["id"].as_str().unwrap().to_owned();
        (manager, root, id)
    }

    fn call(manager: &mut Manager, request: Value) -> Result<Value, String> {
        handle(manager, &request).map(|value| value.unwrap())
    }
    fn connect_to(manager: &mut Manager, id: &str, path: &Path) -> Result<Value, String> {
        call(
            manager,
            json!({"action":"connect","client":"codex","skillId":id,"targetPath":readable(path)}),
        )
    }
    fn external_link(root: &Path, path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        create_link(&local::canonical(root).unwrap(), path).unwrap();
    }
    fn original(root: &Path) -> std::collections::BTreeMap<String, String> {
        local::files(root).unwrap()
    }

    #[test]
    fn adopts_existing_same_target_without_replacing_it_and_is_idempotent() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        external_link(&root, &path);
        let before = file_identity(&path).unwrap();
        let hashes = original(&root);
        connect_to(&mut manager, &id, &path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        connect_to(&mut manager, &id, &path).unwrap();
        assert_eq!(manager.state.connections.len(), 1);
        assert_eq!(manager.state.connections[0]["id"], connection_id);
        assert_eq!(manager.state.connections[0]["origin"], "existing");
        assert_eq!(manager.state.connections[0]["kind"], "link");
        assert_eq!(manager.state.connections[0]["linked"], true);
        assert_eq!(manager.state.connections[0]["verified"], false);
        assert_eq!(file_identity(&path).unwrap(), before);
        assert_eq!(original(&root), hashes);
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn scans_existing_links_and_direct_folders_without_duplicates_or_filesystem_writes() {
        let fixture = fixture();
        let (mut manager, root, _) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        external_link(&root, &path);
        let before = file_identity(&path).unwrap();
        let roots = vec![
            ("codex".into(), path.parent().unwrap().to_path_buf()),
            ("claude".into(), root.parent().unwrap().to_path_buf()),
        ];
        refresh_roots(&mut manager, &roots).unwrap();
        refresh_roots(&mut manager, &roots).unwrap();
        assert_eq!(manager.state.connections.len(), 2);
        assert_eq!(
            manager
                .state
                .connections
                .iter()
                .filter(|c| c["kind"] == "direct")
                .count(),
            1
        );
        assert!(manager
            .state
            .connections
            .iter()
            .all(|c| c["origin"] == "existing" && c["linked"] == true && c["verified"] == false));
        assert_eq!(file_identity(&path).unwrap(), before);
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn creates_single_source_link_and_disconnect_preserves_runtime() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        let hashes = original(&root);
        connect_to(&mut manager, &id, &path).unwrap();
        assert_eq!(manager.state.connections[0]["origin"], "created");
        assert_eq!(
            key(&local::canonical(&path).unwrap()),
            key(&local::canonical(&root).unwrap())
        );
        let connection_id = manager.state.connections[0]["id"].clone();
        call(
            &mut manager,
            json!({"action":"disconnect","connectionId":connection_id}),
        )
        .unwrap();
        assert!(manager.state.connections.is_empty());
        assert!(fs::symlink_metadata(path).is_err());
        assert_eq!(original(&root), hashes);
    }

    #[test]
    fn unregistering_external_or_legacy_links_keeps_them_untouched() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        for origin in [Some("existing"), None] {
            let path = fixture.path().join("client/fixture");
            if !path.exists() {
                external_link(&root, &path);
            }
            connect_to(&mut manager, &id, &path).unwrap();
            if origin.is_none() {
                manager.state.connections[0]
                    .as_object_mut()
                    .unwrap()
                    .remove("origin");
            }
            let before = file_identity(&path).unwrap();
            let connection_id = manager.state.connections[0]["id"].clone();
            call(
                &mut manager,
                json!({"action":"disconnect","connectionId":connection_id}),
            )
            .unwrap();
            assert!(manager.state.connections.is_empty());
            assert_eq!(file_identity(&path).unwrap(), before);
        }
        remove_link(
            &fixture.path().join("client/fixture"),
            &local::canonical(&root).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn source_already_in_client_is_direct_and_cannot_be_unlinked() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let hashes = original(&root);
        connect_to(&mut manager, &id, &root).unwrap();
        assert_eq!(manager.state.connections[0]["kind"], "direct");
        let connection_id = manager.state.connections[0]["id"].clone();
        assert!(call(
            &mut manager,
            json!({"action":"preview_disconnect","connectionId":connection_id})
        )
        .is_err());
        call(
            &mut manager,
            json!({"action":"disconnect","connectionId":connection_id}),
        )
        .unwrap();
        assert_eq!(original(&root), hashes);
        assert!(!linked(&fs::symlink_metadata(root).unwrap()));
    }

    #[test]
    fn refuses_ordinary_conflicting_and_broken_entries_with_paths() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("keep.txt"), "keep").unwrap();
        let error = connect_to(&mut manager, &id, &path).unwrap_err();
        assert!(
            error.contains("客户端位置")
                && error.contains(&readable(&path))
                && error.contains(&readable(&local::canonical(&root).unwrap()))
        );
        assert_eq!(fs::read_to_string(path.join("keep.txt")).unwrap(), "keep");
        let other = fixture.path().join("other");
        fs::create_dir_all(&other).unwrap();
        let link = fixture.path().join("client/different");
        external_link(&other, &link);
        assert!(connect_to(&mut manager, &id, &link).is_err());
        assert_eq!(
            key(&local::canonical(&link).unwrap()),
            key(&local::canonical(&other).unwrap())
        );
        remove_link(&link, &local::canonical(&other).unwrap()).unwrap();
        let broken = fixture.path().join("client/broken");
        external_link(&other, &broken);
        fs::remove_dir(&other).unwrap();
        assert!(connect_to(&mut manager, &id, &broken).is_err());
        assert!(linked(&fs::symlink_metadata(&broken).unwrap()));
        #[cfg(windows)]
        fs::remove_dir(&broken).unwrap();
        #[cfg(unix)]
        fs::remove_file(&broken).unwrap();
        assert!(manager.state.connections.is_empty());
    }

    #[test]
    fn refresh_reports_missing_and_changed_links_and_keeps_invocation_unverified() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        connect_to(&mut manager, &id, &path).unwrap();
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
        refresh_roots(&mut manager, &[]).unwrap();
        assert_eq!(manager.state.connections[0]["health"], "missing");
        assert_eq!(manager.state.connections[0]["linked"], false);
        let other = fixture.path().join("other");
        fs::create_dir_all(&other).unwrap();
        external_link(&other, &path);
        refresh_roots(&mut manager, &[]).unwrap();
        assert_eq!(manager.state.connections[0]["health"], "changed");
        let connection_id = manager.state.connections[0]["id"].clone();
        assert!(call(
            &mut manager,
            json!({"action":"disconnect","connectionId":connection_id})
        )
        .is_err());
        assert_eq!(
            key(&local::canonical(&path).unwrap()),
            key(&local::canonical(&other).unwrap())
        );
        remove_link(&path, &local::canonical(&other).unwrap()).unwrap();
    }

    #[test]
    fn existing_link_requires_bound_preview_for_real_detachment() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        let hashes = original(&root);
        external_link(&root, &path);
        connect_to(&mut manager, &id, &path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        let preview = call(
            &mut manager,
            json!({"action":"preview_disconnect","connectionId":connection_id}),
        )
        .unwrap();
        assert_eq!(preview["path"], readable(&path));
        assert_eq!(
            preview["target"],
            readable(&local::canonical(&root).unwrap())
        );
        assert!(path.exists());
        call(
            &mut manager,
            json!({"action":"apply_disconnect","previewId":preview["id"]}),
        )
        .unwrap();
        assert!(manager.state.connections.is_empty());
        assert!(fs::symlink_metadata(path).is_err());
        assert_eq!(original(&root), hashes);
        assert!(call(
            &mut manager,
            json!({"action":"apply_disconnect","previewId":preview["id"]})
        )
        .is_err());
    }

    #[test]
    fn replacing_link_even_with_same_target_invalidates_preview() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        external_link(&root, &path);
        connect_to(&mut manager, &id, &path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        let preview = call(
            &mut manager,
            json!({"action":"preview_disconnect","connectionId":connection_id}),
        )
        .unwrap();
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
        external_link(&root, &path);
        assert!(call(
            &mut manager,
            json!({"action":"apply_disconnect","previewId":preview["id"]})
        )
        .is_err());
        assert!(path.exists());
        assert_eq!(manager.state.connections.len(), 1);
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn replacing_link_with_real_folder_invalidates_preview_and_preserves_files() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        external_link(&root, &path);
        connect_to(&mut manager, &id, &path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        let preview = call(
            &mut manager,
            json!({"action":"preview_disconnect","connectionId":connection_id}),
        )
        .unwrap();
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep.txt"), "keep").unwrap();
        assert!(call(
            &mut manager,
            json!({"action":"apply_disconnect","previewId":preview["id"]})
        )
        .is_err());
        assert_eq!(fs::read_to_string(path.join("keep.txt")).unwrap(), "keep");
        assert!(root.join("SKILL.md").exists());
    }

    fn fail_saves(manager: &Manager) {
        let db = rusqlite::Connection::open(manager.data_dir.join("manager.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER client_test_no_update BEFORE UPDATE ON app_state BEGIN SELECT RAISE(FAIL, 'synthetic save failure'); END;").unwrap();
    }

    #[test]
    fn failed_save_rolls_back_only_new_link_and_never_existing_link() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        fail_saves(&manager);
        let created = fixture.path().join("client/new");
        assert!(connect_to(&mut manager, &id, &created).is_err());
        assert!(fs::symlink_metadata(created).is_err());
        let existing = fixture.path().join("client/existing");
        external_link(&root, &existing);
        let before = file_identity(&existing).unwrap();
        assert!(connect_to(&mut manager, &id, &existing).is_err());
        assert_eq!(file_identity(&existing).unwrap(), before);
        assert!(manager.state.connections.is_empty());
        remove_link(&existing, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn save_failure_during_detachment_leaves_original_link_and_record() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        external_link(&root, &path);
        connect_to(&mut manager, &id, &path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        let preview = call(
            &mut manager,
            json!({"action":"preview_disconnect","connectionId":connection_id}),
        )
        .unwrap();
        let before = file_identity(&path).unwrap();
        fail_saves(&manager);
        assert!(call(
            &mut manager,
            json!({"action":"apply_disconnect","previewId":preview["id"]})
        )
        .is_err());
        assert_eq!(file_identity(&path).unwrap(), before);
        assert_eq!(manager.state.connections.len(), 1);
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn rejects_readonly_skill_and_linked_destination_parent_without_creating_files() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        manager.state.skills[0]["readOnly"] = json!(true);
        assert!(connect_to(&mut manager, &id, &path).is_err());
        assert!(!path.exists());
        manager.state.skills[0]["readOnly"] = json!(false);
        let alias = fixture.path().join("alias");
        external_link(&root, &alias);
        assert!(connect_to(&mut manager, &id, &alias.join("nested")).is_err());
        assert!(!root.join("nested").exists());
        remove_link(&alias, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn missing_owned_link_only_removes_record_and_reconnect_repairs_explicitly() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        connect_to(&mut manager, &id, &path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
        connect_to(&mut manager, &id, &path).unwrap();
        assert_eq!(manager.state.connections.len(), 1);
        assert_eq!(manager.state.connections[0]["id"], connection_id);
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
        call(
            &mut manager,
            json!({"action":"disconnect","connectionId":connection_id}),
        )
        .unwrap();
        assert!(manager.state.connections.is_empty());
        assert!(root.join("SKILL.md").exists());
    }

    #[test]
    fn removing_existing_record_persists_exact_ignore_until_explicit_reconnect() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        external_link(&root, &path);
        let before = file_identity(&path).unwrap();
        let roots = vec![("codex".into(), path.parent().unwrap().to_path_buf())];
        refresh_roots(&mut manager, &roots).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        call(
            &mut manager,
            json!({"action":"disconnect","connectionId":connection_id}),
        )
        .unwrap();
        refresh_roots(&mut manager, &roots).unwrap();
        assert!(manager.state.connections.is_empty());
        assert_eq!(
            manager.state.settings["ignoredClientConnections"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let data_dir = manager.data_dir.clone();
        drop(manager);
        let mut manager = Manager::new(data_dir).unwrap();
        refresh_roots(&mut manager, &roots).unwrap();
        assert!(manager.state.connections.is_empty());
        connect_to(&mut manager, &id, &path).unwrap();
        assert_eq!(manager.state.connections.len(), 1);
        assert!(manager.state.settings["ignoredClientConnections"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(file_identity(&path).unwrap(), before);
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn ignored_path_can_discover_different_skill_target_without_affecting_original() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        external_link(&root, &path);
        connect_to(&mut manager, &id, &path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        call(
            &mut manager,
            json!({"action":"disconnect","connectionId":connection_id}),
        )
        .unwrap();
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
        let other = fixture.path().join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(
            other.join("SKILL.md"),
            "---\nname: other\ndescription: Another synthetic skill\n---\nOther fixture.\n",
        )
        .unwrap();
        local::handle(
            &mut manager,
            &json!({"action":"add_local","paths":[readable(&other)]}),
        )
        .unwrap();
        let other_id = manager
            .state
            .skills
            .iter()
            .find(|skill| skill["name"] == "other")
            .unwrap()["id"]
            .clone();
        external_link(&other, &path);
        refresh_roots(
            &mut manager,
            &[("codex".into(), path.parent().unwrap().to_path_buf())],
        )
        .unwrap();
        assert_eq!(manager.state.connections.len(), 1);
        assert_eq!(manager.state.connections[0]["skillId"], other_id);
        assert!(root.join("SKILL.md").exists());
        remove_link(&path, &local::canonical(&other).unwrap()).unwrap();
    }

    #[test]
    fn removing_direct_record_does_not_rediscover_or_delete_source() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let hashes = original(&root);
        connect_to(&mut manager, &id, &root).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        call(
            &mut manager,
            json!({"action":"disconnect","connectionId":connection_id}),
        )
        .unwrap();
        refresh_roots(
            &mut manager,
            &[("codex".into(), root.parent().unwrap().to_path_buf())],
        )
        .unwrap();
        assert!(manager.state.connections.is_empty());
        assert_eq!(original(&root), hashes);
    }

    #[test]
    fn failed_record_removal_rolls_back_ignore_without_touching_existing_link() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        external_link(&root, &path);
        connect_to(&mut manager, &id, &path).unwrap();
        let before = file_identity(&path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        fail_saves(&manager);
        assert!(call(
            &mut manager,
            json!({"action":"disconnect","connectionId":connection_id})
        )
        .is_err());
        assert_eq!(manager.state.connections.len(), 1);
        assert!(manager.state.settings["ignoredClientConnections"].is_null());
        assert_eq!(file_identity(&path).unwrap(), before);
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn owned_link_cannot_be_detached_after_skill_becomes_readonly_or_changes_source() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        connect_to(&mut manager, &id, &path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        manager.state.skills[0]["readOnly"] = json!(true);
        assert!(call(
            &mut manager,
            json!({"action":"disconnect","connectionId":connection_id})
        )
        .is_err());
        manager.state.skills[0]["readOnly"] = json!(false);
        let other = fixture.path().join("other");
        fs::create_dir_all(&other).unwrap();
        manager.state.skills[0]["localPath"] = json!(readable(&other));
        assert!(call(
            &mut manager,
            json!({"action":"disconnect","connectionId":connection_id})
        )
        .is_err());
        assert!(path.exists());
        assert_eq!(manager.state.connections.len(), 1);
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn v025_user_usage_confirmation_binds_current_content_and_expires_after_edits() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        connect_to(&mut manager, &id, &path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        let value = call(
            &mut manager,
            json!({"action":"confirm_client_usage","connectionId":connection_id}),
        )
        .unwrap();
        let confirmation = value["connections"][0]["usageConfirmation"].clone();
        assert_eq!(confirmation["client"], "codex");
        assert_eq!(
            confirmation["contentFingerprint"].as_str().unwrap().len(),
            64
        );
        assert!(confirmation["confirmedAt"].as_str().is_some());
        assert_eq!(value["connections"][0]["usageStatus"], "current");
        assert_eq!(value["connections"][0]["verified"], false);
        refresh_roots(&mut manager, &[]).unwrap();
        assert_eq!(manager.state.connections[0]["usageStatus"], "current");
        fs::write(root.join("README.md"), "Changed synthetic documentation\n").unwrap();
        refresh_roots(&mut manager, &[]).unwrap();
        assert_eq!(manager.state.connections[0]["usageStatus"], "stale");
        assert_eq!(
            manager.state.connections[0]["usageConfirmation"],
            confirmation
        );
        call(
            &mut manager,
            json!({"action":"confirm_client_usage","connectionId":connection_id}),
        )
        .unwrap();
        assert_ne!(
            manager.state.connections[0]["usageConfirmation"]["contentFingerprint"],
            confirmation["contentFingerprint"]
        );
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
        refresh_roots(&mut manager, &[]).unwrap();
        assert_eq!(manager.state.connections[0]["usageStatus"], "unavailable");
        assert_eq!(manager.state.connections[0]["verified"], false);
    }

    #[test]
    fn v025_read_check_advertises_missing_owned_entry_and_explicit_write_repairs_it() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        let source_hashes = original(&root);
        connect_to(&mut manager, &id, &path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
        refresh_roots(&mut manager, &[]).unwrap();
        assert_eq!(manager.state.connections[0]["autoRepairable"], true);
        assert!(
            fs::symlink_metadata(&path).is_err(),
            "A read check cannot create an entry"
        );
        let prior_issues = json!([{"client":"cursor","message":"synthetic unreadable root"}]);
        manager.state.settings["clientIssues"] = prior_issues.clone();
        let value = call(
            &mut manager,
            json!({"action":"repair_client_connections","connectionIds":[connection_id]}),
        )
        .unwrap();
        assert_eq!(value["failed"], 0);
        assert_eq!(value["connections"][0]["id"], connection_id);
        assert_eq!(value["connections"][0]["health"], "ready");
        assert_eq!(value["connections"][0]["autoRepairable"], false);
        assert_eq!(value["settings"]["clientIssues"], prior_issues);
        assert!(value["settings"].get("clientRepairCandidates").is_none());
        assert_eq!(original(&root), source_hashes);
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn v025_repair_refuses_conflicts_readonly_changed_sources_and_exact_ignores() {
        for scenario in ["conflict", "readonly", "changed", "ignored"] {
            let fixture = fixture();
            let (mut manager, root, id) = setup(fixture.path());
            let path = fixture.path().join("client/fixture");
            connect_to(&mut manager, &id, &path).unwrap();
            let connection_id = manager.state.connections[0]["id"].clone();
            remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
            match scenario {
                "conflict" => {
                    fs::create_dir(&path).unwrap();
                    fs::write(path.join("keep.txt"), "preserve conflict").unwrap();
                }
                "readonly" => manager.state.skills[0]["readOnly"] = json!(true),
                "changed" => {
                    let other = fixture.path().join("changed-source");
                    fs::create_dir(&other).unwrap();
                    fs::write(other.join("SKILL.md"), "Changed source").unwrap();
                    manager.state.skills[0]["localPath"] = json!(readable(&other));
                }
                "ignored" => {
                    manager.state.settings["ignoredClientConnections"] = json!([{"client":"codex","path":readable(&path),"target":readable(&local::canonical(&root).unwrap())}])
                }
                _ => unreachable!(),
            }
            refresh_roots(&mut manager, &[]).unwrap();
            assert_eq!(
                manager.state.connections[0]["autoRepairable"], false,
                "{scenario}"
            );
            let value = call(
                &mut manager,
                json!({"action":"repair_client_connections","connectionIds":[connection_id]}),
            )
            .unwrap();
            assert_eq!(
                value["connectionResults"][0]["success"], false,
                "{scenario}"
            );
            if scenario == "conflict" {
                assert_eq!(
                    fs::read_to_string(path.join("keep.txt")).unwrap(),
                    "preserve conflict"
                );
            } else {
                assert!(fs::symlink_metadata(&path).is_err(), "{scenario}");
            }
            assert!(root.join("SKILL.md").is_file());
        }
    }

    #[test]
    fn v025_stop_management_of_created_link_preserves_it_and_blocks_rediscovery() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        connect_to(&mut manager, &id, &path).unwrap();
        let identity = file_identity(&path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        let preview = call(
            &mut manager,
            json!({"action":"preview_disconnect","connectionIds":[connection_id],"mode":"stop"}),
        )
        .unwrap();
        assert_eq!(preview["entries"][0]["mode"], "stop");
        assert!(path.exists());
        call(
            &mut manager,
            json!({"action":"apply_disconnect","previewId":preview["id"]}),
        )
        .unwrap();
        assert!(manager.state.connections.is_empty());
        assert_eq!(file_identity(&path).unwrap(), identity);
        let data = manager.data_dir.clone();
        drop(manager);
        let mut manager = Manager::new(data).unwrap();
        refresh_roots(
            &mut manager,
            &[("codex".into(), path.parent().unwrap().into())],
        )
        .unwrap();
        assert!(manager.state.connections.is_empty());
        assert_eq!(
            manager.state.settings["ignoredClientConnections"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        connect_to(&mut manager, &id, &path).unwrap();
        assert!(manager.state.settings["ignoredClientConnections"]
            .as_array()
            .unwrap()
            .is_empty());
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn v025_batch_disconnect_keeps_success_and_reports_stale_and_direct_entries() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let (owned, existing) = (
            fixture.path().join("client/owned"),
            fixture.path().join("client/existing"),
        );
        connect_to(&mut manager, &id, &owned).unwrap();
        external_link(&root, &existing);
        connect_to(&mut manager, &id, &existing).unwrap();
        connect_to(&mut manager, &id, &root).unwrap();
        let ids: Vec<Value> = manager
            .state
            .connections
            .iter()
            .map(|record| record["id"].clone())
            .collect();
        let hashes = original(&root);
        let preview = call(
            &mut manager,
            json!({"action":"preview_disconnect","connectionIds":ids}),
        )
        .unwrap();
        assert_eq!(preview["entries"].as_array().unwrap().len(), 3);
        assert_eq!(preview["entries"][2]["canApply"], false);
        remove_link(&existing, &local::canonical(&root).unwrap()).unwrap();
        external_link(&root, &existing);
        let result = call(
            &mut manager,
            json!({"action":"apply_disconnect","previewId":preview["id"]}),
        )
        .unwrap();
        assert_eq!(result["connectionResults"][0]["success"], true);
        assert_eq!(result["connectionResults"][1]["success"], false);
        assert_eq!(result["connectionResults"][2]["success"], false);
        assert_eq!(result["failed"], 2);
        assert!(fs::symlink_metadata(&owned).is_err());
        assert!(existing.exists());
        assert_eq!(original(&root), hashes);
        assert!(call(
            &mut manager,
            json!({"action":"apply_disconnect","previewId":preview["id"]})
        )
        .is_err());
        remove_link(&existing, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn v025_confirmation_save_failure_preserves_record_and_source() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        connect_to(&mut manager, &id, &path).unwrap();
        let (before, hashes, identity) = (
            manager.state.connections[0].clone(),
            original(&root),
            file_identity(&path).unwrap(),
        );
        fail_saves(&manager);
        assert!(call(
            &mut manager,
            json!({"action":"confirm_client_usage","connectionId":before["id"]})
        )
        .is_err());
        assert_eq!(manager.state.connections[0], before);
        assert_eq!(original(&root), hashes);
        assert_eq!(file_identity(&path).unwrap(), identity);
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn v025_failed_repair_save_rolls_back_only_its_new_entry() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        connect_to(&mut manager, &id, &path).unwrap();
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
        refresh_roots(&mut manager, &[]).unwrap();
        let (connection_id, hashes) = (manager.state.connections[0]["id"].clone(), original(&root));
        fail_saves(&manager);
        let result = call(
            &mut manager,
            json!({"action":"repair_client_connections","connectionIds":[connection_id]}),
        )
        .unwrap();
        assert_eq!(result["connectionResults"][0]["success"], false);
        assert!(result["failed"].as_u64().unwrap() > 0);
        assert!(fs::symlink_metadata(&path).is_err());
        assert_eq!(manager.state.connections.len(), 1);
        assert_eq!(original(&root), hashes);
    }

    #[test]
    fn v025_same_current_target_repairs_management_proof_without_replacing_link() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        connect_to(&mut manager, &id, &path).unwrap();
        let identity = file_identity(&path).unwrap();
        manager.state.connections[0]["target"] =
            json!(readable(&fixture.path().join("old-missing-source")));
        refresh_roots(&mut manager, &[]).unwrap();
        assert_eq!(manager.state.connections[0]["health"], "ready");
        assert_eq!(
            manager.state.connections[0]["target"],
            readable(&local::canonical(&root).unwrap())
        );
        assert_eq!(file_identity(&path).unwrap(), identity);
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn v025_repair_rechecks_a_new_conflict_after_the_read_candidate_was_recorded() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        connect_to(&mut manager, &id, &path).unwrap();
        let connection_id = manager.state.connections[0]["id"].clone();
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
        refresh_roots(&mut manager, &[]).unwrap();
        assert_eq!(manager.state.connections[0]["autoRepairable"], true);
        fs::create_dir(&path).unwrap();
        fs::write(path.join("new.txt"), "New content after check").unwrap();
        let result = call(
            &mut manager,
            json!({"action":"repair_client_connections","connectionIds":[connection_id]}),
        )
        .unwrap();
        assert_eq!(result["connectionResults"][0]["success"], false);
        assert_eq!(
            fs::read_to_string(path.join("new.txt")).unwrap(),
            "New content after check"
        );
        assert_eq!(manager.state.connections[0]["health"], "changed");
        assert_eq!(manager.state.connections[0]["autoRepairable"], false);
    }

    #[test]
    fn v025_stop_preview_rejects_a_changed_record_and_preserves_the_link() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        connect_to(&mut manager, &id, &path).unwrap();
        let (connection_id, identity) = (
            manager.state.connections[0]["id"].clone(),
            file_identity(&path).unwrap(),
        );
        let preview = call(
            &mut manager,
            json!({"action":"preview_disconnect","connectionId":connection_id,"mode":"stop"}),
        )
        .unwrap();
        manager.state.connections[0]["origin"] = json!("existing");
        assert!(call(
            &mut manager,
            json!({"action":"apply_disconnect","previewId":preview["id"]})
        )
        .is_err());
        assert_eq!(manager.state.connections.len(), 1);
        assert_eq!(file_identity(&path).unwrap(), identity);
        assert!(manager.state.settings["ignoredClientConnections"].is_null());
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }

    #[test]
    fn v025_one_malformed_legacy_entry_does_not_abort_other_access_checks() {
        let fixture = fixture();
        let (mut manager, root, id) = setup(fixture.path());
        let path = fixture.path().join("client/fixture");
        connect_to(&mut manager, &id, &path).unwrap();
        manager
            .state
            .connections
            .push(json!({"id":"malformed","skillId":id,"client":"codex","target":readable(&root)}));
        refresh_roots(&mut manager, &[]).unwrap();
        assert_eq!(manager.state.connections[0]["health"], "ready");
        assert_eq!(manager.state.connections[1]["health"], "changed");
        assert_eq!(manager.state.connections[1]["autoRepairable"], false);
        remove_link(&path, &local::canonical(&root).unwrap()).unwrap();
    }
}
