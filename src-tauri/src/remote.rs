//! GitHub is the complete source; local installations contain only the confirmed runtime set.
//! No network writes or local replacement is performed without a one-shot content-bound preview.
mod association;
pub(crate) mod attachments;
mod cleanup;
mod git_write;
pub(crate) mod github;
mod information;
mod logic;
#[cfg(test)]
mod protocol_tests;
mod runtime;
mod upload;

use crate::{
    local,
    model::{now, Manager},
    transaction,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use github::{Api, Entry, Snapshot, MAX_FILE, MAX_TOTAL};
use logic::{checked_selection, hash, selected_changes};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use url::Url;

fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v[key].as_str().ok_or_else(|| format!("Missing {key}"))
}
fn strings(v: &Value) -> Result<Vec<String>, String> {
    v.as_array()
        .ok_or("Expected a list of files")?
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "Invalid file list".into())
        })
        .collect()
}
fn mapping(v: &Value) -> BTreeMap<String, String> {
    serde_json::from_value(v.clone()).unwrap_or_default()
}
fn paths(v: &Value) -> Vec<String> {
    strings(v).unwrap_or_default()
}
fn join(prefix: &str, path: &str) -> String {
    if prefix.is_empty() {
        path.into()
    } else {
        format!("{prefix}/{path}")
    }
}
fn repo(skill: &Value) -> Result<&str, String> {
    text(skill, "repo")
}
fn remote_path(skill: &Value) -> Result<&str, String> {
    let p = text(skill, "remotePath")?;
    if !p.is_empty() {
        github::validate_relative(p)?;
    }
    Ok(p)
}
fn source_for(m: &Manager, skill: &Value) -> Result<Value, String> {
    m.state
        .sources
        .iter()
        .find(|s| s["id"] == skill["sourceId"])
        .cloned()
        .ok_or_else(|| "The Skill has no associated GitHub source".into())
}
fn snapshot(m: &Manager, skill: &Value) -> Result<Snapshot, String> {
    let source = source_for(m, skill)?;
    if source["repo"] != skill["repo"] {
        return Err("Skill/source association is inconsistent".into());
    }
    Snapshot::load(
        repo(skill)?,
        skill["branch"]
            .as_str()
            .or_else(|| source["branch"].as_str()),
    )
}
/// Dependency resolution shares the existing authenticated, immutable repository
/// reader. A supplied commit is always checked against the associated source.
pub(crate) fn dependency_snapshot(
    m: &Manager,
    skill: &Value,
    commit: Option<&str>,
) -> Result<Snapshot, String> {
    let source = source_for(m, skill)?;
    if source["repo"] != skill["repo"] {
        return Err("Skill/source association is inconsistent".into());
    }
    if let Some(commit) = commit {
        github::sha(&json!(commit))?;
        Snapshot::at_commit(repo(skill)?, commit)
    } else {
        snapshot(m, skill)
    }
}
fn writable(skill: &Value) -> Result<(), String> {
    if skill["readOnly"].as_bool().unwrap_or(false) {
        Err("This Skill is read-only".into())
    } else {
        Ok(())
    }
}
fn local_root(skill: &Value) -> Result<PathBuf, String> {
    // Registration/installation already stores the canonical runtime path.
    // A later root or ancestor junction must not silently redirect operations.
    let registered = Path::new(text(skill, "localPath")?);
    transaction::verify_root(registered)?;
    local::canonical(registered)
}
fn current_hashes(skill: &Value) -> Result<BTreeMap<String, String>, String> {
    if skill["localPath"].as_str().is_some() {
        local::files(&local_root(skill)?)
    } else {
        Ok(BTreeMap::new())
    }
}
fn refresh_declared_local_version(skill: &mut Value) {
    let version = (|| -> Option<String> {
        skill["localPath"].as_str()?;
        let root = local_root(skill).ok()?;
        let bytes = read_local(&root, "SKILL.md").ok()?;
        let content = std::str::from_utf8(&bytes)
            .ok()?
            .trim_start_matches('\u{feff}');
        let mut lines = content.lines();
        if lines.next().map(str::trim) != Some("---") {
            return None;
        }
        let mut yaml = String::new();
        let mut ended = false;
        for line in lines {
            if matches!(line.trim(), "---" | "...") {
                ended = true;
                break;
            }
            yaml.push_str(line);
            yaml.push('\n');
        }
        if !ended {
            return None;
        }
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).ok()?;
        let meta = serde_json::to_value(parsed).ok()?;
        let scalar = |value: &Value| match value {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        };
        scalar(&meta["version"]).or_else(|| scalar(&meta["metadata"]["version"]))
    })();
    // Display metadata never establishes identity, installation history or synchronization.
    if let Some(version) = version {
        skill["version"] = json!(version);
    }
}
fn subset(all: &BTreeMap<String, String>, files: &[String]) -> BTreeMap<String, String> {
    files
        .iter()
        .filter_map(|f| all.get(f).map(|h| (f.clone(), h.clone())))
        .collect()
}
fn catalog_items(catalog: Option<&Value>) -> Vec<Value> {
    match catalog.map(|v| &v["skills"]) {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Object(o)) => o
            .iter()
            .map(|(key, value)| {
                let mut v = value.clone();
                if v["path"].is_null() {
                    v["path"] = json!(key);
                }
                v
            })
            .collect(),
        _ => vec![],
    }
}
fn catalog_path(item: &Value) -> Option<String> {
    if let Some(source) = item["source"].as_str() {
        if source == "SKILL.md" {
            return Some(String::new());
        }
        if let Some(path) = source.strip_suffix("/SKILL.md") {
            return Some(path.into());
        }
    }
    ["path", "remotePath", "relativePath"].iter().find_map(|k| {
        item[*k]
            .as_str()
            .map(|v| v.trim_end_matches('/').to_owned())
    })
}
fn catalog_item<'a>(items: &'a [Value], path: &str) -> Option<&'a Value> {
    items
        .iter()
        .find(|i| catalog_path(i).as_deref() == Some(path))
}
pub(crate) fn declared_runtime(item: Option<&Value>) -> Option<Vec<String>> {
    item.and_then(|i| {
        [
            i.get("runtime_files"),
            i.get("runtimeFiles"),
            i.get("runtime").and_then(|r| r.get("files")),
        ]
        .into_iter()
        .flatten()
        .find_map(|v| strings(v).ok())
    })
}
fn documentation_path(skill: &Value, snap: &Snapshot) -> Result<String, String> {
    // Resolve the usage path from this exact commit, including older installed versions.
    let (_, item) = manifests(snap, remote_path(skill)?)?;
    if let Some(path) = item.as_ref().and_then(|v| v["usage"].as_str()) {
        github::validate_relative(path)?;
        if snap.tree.contains_key(path) {
            return Ok(path.into());
        }
    }
    if let Some(path) = skill["documentationPath"].as_str() {
        github::validate_relative(path)?;
        if snap.tree.contains_key(path) {
            return Ok(path.into());
        }
    }
    let root = remote_path(skill)?;
    for leaf in ["README.md", "readme.md", "Readme.md", "SKILL.md"] {
        let path = join(root, leaf);
        if snap.tree.contains_key(&path) {
            return Ok(path);
        }
    }
    Err("No documentation is present at the selected version".into())
}
fn doc_hash(
    snap: &Snapshot,
    root: &str,
    runtime: &[String],
    item: Option<&Value>,
) -> Result<String, String> {
    let entries = snap.skill_files(root)?;
    let runtime: BTreeSet<&str> = runtime.iter().map(String::as_str).collect();
    let docs: BTreeMap<&str, &str> = entries
        .iter()
        .filter(|(p, _)| !runtime.contains(p.as_str()))
        .map(|(p, e)| (p.as_str(), e.sha.as_str()))
        .collect();
    Ok(hash(
        &serde_json::to_vec(&json!({"files":docs,"metadata":item}))
            .map_err(|_| "Cannot fingerprint documentation")?,
    ))
}
fn manifests(snap: &Snapshot, root: &str) -> Result<(Option<Vec<String>>, Option<Value>), String> {
    let catalog = snap.catalog()?;
    let items = catalog_items(catalog.as_ref());
    let item = catalog_item(&items, root).cloned();
    let files = declared_runtime(item.as_ref());
    Ok((files, item))
}
fn download(
    snap: &Snapshot,
    entries: &BTreeMap<String, Entry>,
    selected: &[String],
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut data = BTreeMap::new();
    let mut total = 0usize;
    for (index, f) in selected.iter().enumerate() {
        crate::network::check_cancelled()?;
        crate::tasks::report_progress("读取运行文件", Some(index), Some(selected.len()));
        github::validate_relative(f)?;
        let e = entries
            .get(f)
            .ok_or_else(|| format!("Selected runtime file no longer exists: {f}"))?;
        let b = snap.read_entry(e)?;
        total += b.len();
        if total > MAX_TOTAL {
            return Err("Runtime package exceeds the 32 MiB limit".into());
        }
        data.insert(f.clone(), b);
        crate::tasks::report_progress("读取运行文件", Some(index + 1), Some(selected.len()));
    }
    Ok(data)
}
fn hashes(data: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, String> {
    data.iter().map(|(p, b)| (p.clone(), hash(b))).collect()
}
fn validate_runtime(
    entries: &BTreeMap<String, Entry>,
    selected: &[String],
    data: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<String>, String> {
    let selected: BTreeSet<String> = selected.iter().cloned().collect();
    let all = entries
        .iter()
        .filter(|(p, e)| {
            github::validate_relative(p).is_ok() && matches!(e.mode.as_str(), "100644" | "100755")
        })
        .map(|(p, e)| (p.clone(), e.sha.clone()))
        .collect();
    let classification =
        local::runtime_file_selection_with_reader(&all, Some(&selected), |path| {
            let bytes = data.get(path).ok_or_else(|| {
                format!("Dependency was not selected for this runtime preview: {path}")
            })?;
            String::from_utf8(bytes.clone())
                .map_err(|_| format!("Dependency is not UTF-8 text: {path}"))
        })?;
    let missing: Vec<_> = classification
        .required_files
        .iter()
        .filter(|p| !selected.contains(*p))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "Include these required runtime files: {}",
            missing.join(", ")
        ));
    }
    Ok(classification.warnings)
}
fn update_source(m: &mut Manager, snap: &Snapshot, id: &Value) {
    if let Some(source) = m.state.sources.iter_mut().find(|s| &s["id"] == id) {
        source["commit"] = json!(snap.commit);
        source["lastChecked"] = json!(now());
        source["error"] = Value::Null;
        source["private"] = json!(snap.private);
    }
}

fn within_scope(path: &str, root: &str) -> bool {
    root.is_empty()
        || path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn checked_directories(value: &Value) -> Result<Vec<String>, String> {
    let mut directories = BTreeSet::new();
    for path in strings(value)? {
        if !path.is_empty() {
            github::validate_relative(&path)?;
        }
        directories.insert(path);
    }
    Ok(directories.into_iter().collect())
}

fn source_roots(source: &Value, skills: &[Value]) -> Result<Vec<String>, String> {
    if source.get("roots").is_some() {
        return checked_directories(&source["roots"]);
    }
    // Older records did not save the original directory URL. Infer only the known
    // Skill directories; never silently grant discovery access to a wider scope.
    let known: Vec<String> = skills
        .iter()
        .filter(|s| s["sourceId"] == source["id"])
        .filter_map(|s| s["remotePath"].as_str().map(str::to_owned))
        .collect();
    checked_directories(&json!(known))
}

fn ignored_paths(source: &Value) -> Result<Vec<String>, String> {
    source
        .get("ignoredPaths")
        .map(checked_directories)
        .unwrap_or_else(|| Ok(vec![]))
}

fn track_scope(
    source: &mut Value,
    skills: &[Value],
    root: &str,
    restore_descendants: bool,
) -> Result<(), String> {
    if !root.is_empty() {
        github::validate_relative(root)?;
    }
    let mut roots = source_roots(source, skills)?;
    roots.push(root.to_owned());
    roots.sort();
    roots.dedup();
    let ignored: Vec<_> = ignored_paths(source)?
        .into_iter()
        .filter(|p| {
            if restore_descendants {
                !within_scope(p, root)
            } else {
                p != root
            }
        })
        .collect();
    source["roots"] = json!(roots);
    source["ignoredPaths"] = json!(ignored);
    Ok(())
}

fn discover_paths(
    tree: &BTreeMap<String, Entry>,
    roots: &[String],
    ignored: &[String],
) -> Result<BTreeSet<String>, String> {
    let mut discovered = BTreeSet::new();
    for (path, entry) in tree {
        if !["100644", "100755"].contains(&entry.mode.as_str()) {
            continue;
        }
        let parent = if path == "SKILL.md" {
            ""
        } else if let Some(parent) = path.strip_suffix("/SKILL.md") {
            parent
        } else {
            continue;
        };
        if !parent.is_empty() && github::validate_relative(parent).is_err() {
            continue;
        }
        if roots.iter().any(|root| within_scope(parent, root))
            && !ignored.iter().any(|p| p == parent)
        {
            discovered.insert(parent.to_owned());
        }
    }
    if discovered.len() > 500 {
        return Err("Source contains more than 500 Skills in its selected directories; choose a smaller scope".into());
    }
    Ok(discovered)
}

fn reconcile_discovery(
    skills: &mut Vec<Value>,
    source: &Value,
    tree: &BTreeMap<String, Entry>,
    items: &[Value],
) -> Result<(), String> {
    let roots = checked_directories(&source["roots"])?;
    let ignored = ignored_paths(source)?;
    let discovered = discover_paths(tree, &roots, &ignored)?;
    // A remote removal is a visible condition, never an instruction to remove local files
    // or to replace their baseline. Existing identities and local metadata stay untouched.
    for skill in skills.iter_mut().filter(|s| s["sourceId"] == source["id"]) {
        let Some(path) = skill["remotePath"].as_str() else {
            continue;
        };
        let missing = !tree.contains_key(&join(path, "SKILL.md"));
        let was_missing = skill["remoteMissing"] == true;
        skill["remoteMissing"] = json!(missing);
        if missing {
            skill["error"] = json!("The source no longer contains this Skill's SKILL.md. Local files and the installed baseline are preserved.");
            skill["remoteCheckRequired"] = json!(true);
            skill["remoteHashes"] = Value::Null;
            skill["lastChecked"] = source["lastChecked"].clone();
        } else if was_missing {
            skill["error"] = Value::Null;
        }
    }
    for path in discovered {
        if skills
            .iter()
            .any(|s| s["sourceId"] == source["id"] && s["remotePath"] == path)
        {
            continue;
        }
        let item = catalog_item(items, &path);
        let name = item.and_then(|i| i["id"].as_str()).unwrap_or_else(|| {
            path.rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or("root-skill")
        });
        let runtime = declared_runtime(item);
        skills.push(json!({"id":uuid::Uuid::new_v4().to_string(),"name":name,
            "displayName":item.and_then(|i|i["display_name"].as_str().or_else(||i["displayName"].as_str())).unwrap_or(name),
            "description":item.and_then(|i|i["purpose_zh"].as_str().or_else(||i["description"].as_str())).unwrap_or(""),
            "version":item.and_then(|i|i["version_in_source"].as_str().or_else(||i["version"].as_str())),
            "localPath":null,"readOnly":false,"sourceId":source["id"],"remotePath":path,"repo":source["repo"],"branch":source["branch"],"commit":source["commit"],
            "baselineCommit":null,"baselineHashes":{},"runtimeFiles":runtime.clone().unwrap_or_default(),"runtimeConfirmed":runtime.is_some(),
            "status":"remote_only","lastChecked":source["lastChecked"],"error":null,"private":source["private"],"remoteMissing":false,
            "documentationPath":item.and_then(|i|i["usage"].as_str()),"catalogMetadata":item,"changeDetails":[],"associationNote":null}));
    }
    Ok(())
}

fn add_github(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let previous = m.state.clone();
    let location = github::parse_location(text(req, "url")?)?;
    let (snap, root) = github::resolve_location(&location)?;
    let catalog = snap.catalog()?;
    let items = catalog_items(catalog.as_ref());
    if discover_paths(&snap.tree, std::slice::from_ref(&root), &[])?.is_empty() {
        return Err("No SKILL.md was found in this repository directory".into());
    }
    let existing = m
        .state
        .sources
        .iter()
        .find(|s| {
            s["repo"]
                .as_str()
                .is_some_and(|repo| repo.eq_ignore_ascii_case(&location.repo))
                && s["branch"] == snap.branch
        })
        .cloned();
    let source_id = existing
        .as_ref()
        .map(|s| s["id"].clone())
        .unwrap_or_else(|| json!(m.id()));
    let mut source = existing.unwrap_or_else(|| json!({"id":source_id,"kind":"github","repo":location.repo,"branch":snap.branch,"ignoredPaths":[]}));
    // Re-adding an explicit scope intentionally restores forgotten Skills inside it.
    track_scope(&mut source, &m.state.skills, &root, true)?;
    source["commit"] = json!(snap.commit);
    source["private"] = json!(snap.private);
    source["lastChecked"] = json!(now());
    source["error"] = Value::Null;
    let mut skills = m.state.skills.clone();
    reconcile_discovery(&mut skills, &source, &snap.tree, &items)?;
    if let Some(i) = m.state.sources.iter().position(|s| s["id"] == source_id) {
        m.state.sources[i] = source;
    } else {
        m.state.sources.push(source);
    }
    m.state.skills = skills;
    let mut snapshots = BTreeMap::new();
    if let Some(id) = source_id.as_str() {
        snapshots.insert(id.to_owned(), snap);
    }
    auto_associate_with(m, &snapshots);
    save_association_state(m, previous)
}

fn associate(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let previous = m.state.clone();
    let mut skill = m.skill(text(req, "skillId")?)?;
    let mut source = m
        .state
        .sources
        .iter()
        .find(|s| s["id"] == req["sourceId"])
        .cloned()
        .ok_or("Source does not exist")?;
    let root = text(req, "remotePath")?;
    if !root.is_empty() {
        github::validate_relative(root)?;
    }
    let snap = Snapshot::load(text(&source, "repo")?, source["branch"].as_str())?;
    snap.skill_files(root)?;
    let (_, item) = manifests(&snap, root)?;
    track_scope(&mut source, &m.state.skills, root, false)?;
    skill["sourceId"] = source["id"].clone();
    skill["repo"] = source["repo"].clone();
    skill["branch"] = json!(snap.branch);
    skill["remotePath"] = json!(root);
    skill["commit"] = json!(snap.commit);
    skill["baselineCommit"] = Value::Null;
    skill["baselineKind"] = Value::Null;
    skill["baselineVerifiedAt"] = Value::Null;
    skill["installedCommit"] = Value::Null;
    skill["baselineHashes"] = json!({});
    skill["baselineDocHash"] = Value::Null;
    skill["remoteHashes"] = Value::Null;
    skill["lastRemoteCheckedAt"] = Value::Null;
    skill["remoteDocHash"] = Value::Null;
    skill["remoteCheckRequired"] = json!(true);
    skill["runtimeSources"] = json!({});
    for field in [
        "comparisonLocalHashes",
        "comparisonRemoteHashes",
        "comparisonObservedAt",
        "comparisonRuntimeFiles",
        "comparisonRuntimeSelection",
        "comparisonCommit",
        "comparisonScope",
        "comparisonDocHash",
    ] {
        skill[field] = Value::Null;
    }
    skill["changeDetails"] = json!([]);
    skill["associationReason"] = json!("用户明确选择了此 GitHub 来源");
    skill["private"] = json!(snap.private);
    skill["lastChecked"] = json!(now());
    skill["error"] = Value::Null;
    skill["remoteMissing"] = json!(false);
    skill["documentationPath"] = item
        .as_ref()
        .and_then(|i| i.get("usage"))
        .cloned()
        .unwrap_or(Value::Null);
    skill["status"] = json!(if skill["localPath"].as_str().is_some() {
        "unknown_baseline"
    } else {
        "remote_only"
    });
    // Explicit association unifies an existing remote-only listing; unrelated local folders remain independent.
    let local_id = skill["id"].clone();
    for remote in m.state.skills.iter().filter(|s| {
        s["id"] != local_id
            && s["sourceId"] == source["id"]
            && s["remotePath"] == root
            && s["localPath"].is_null()
    }) {
        if remote["favorite"] == true {
            skill["favorite"] = json!(true);
        }
        if !skill["categoryId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
        {
            skill["categoryId"] = remote["categoryId"].clone();
        }
    }
    m.state.skills.retain(|s| {
        s["id"] == skill["id"]
            || !(s["sourceId"] == source["id"]
                && s["remotePath"] == root
                && s["localPath"].is_null())
    });
    if let Err(error) = refresh_skill(&mut skill, &snap) {
        skill["error"] = json!(error);
        skill["remoteCheckRequired"] = json!(true);
    }
    m.upsert_skill(skill);
    if let Some(stored) = m.state.sources.iter_mut().find(|s| s["id"] == source["id"]) {
        *stored = source.clone();
    }
    update_source(m, &snap, &source["id"]);
    save_association_state(m, previous)
}

fn get_remote_files(m: &Manager, skill: &Value) -> Result<Value, String> {
    let snap = snapshot(m, skill)?;
    let entries = snap.skill_files(remote_path(skill)?)?;
    let (manifest, _) = manifests(&snap, remote_path(skill)?)?;
    let selected = manifest.unwrap_or_else(|| paths(&skill["runtimeFiles"]));
    let selected_set: BTreeSet<_> = selected.iter().cloned().collect();
    let classification = runtime::inspect(
        &snap,
        &entries,
        if selected_set.is_empty() {
            None
        } else {
            Some(&selected_set)
        },
    )?;
    let mut files = Vec::new();
    // A file listing needs only the immutable tree, not a content download for every row.
    for (p, e) in &entries {
        if github::validate_relative(p).is_err() || !["100644", "100755"].contains(&e.mode.as_str())
        {
            continue;
        }
        files.push(json!({"path":p,"size":e.size,"hash":e.sha,"hashAlgorithm":"git-blob-sha1","runtime":selected.contains(p)}));
    }
    Ok(
        json!({"files":files,"runtimeConfirmed":skill["runtimeConfirmed"],"requiredFiles":classification.required_files,"optionalFiles":classification.optional_files,
            "suggestedFiles":classification.suggested_files,"requiredReasons":classification.required_reasons,"warnings":classification.warnings,"issues":classification.issues}),
    )
}

fn documentation_snapshot(m: &Manager, skill: &Value, version: &str) -> Result<Snapshot, String> {
    match version {
        "latest" => match skill["commit"].as_str() {
            Some(commit) => Snapshot::at_commit(repo(skill)?, commit),
            None => snapshot(m, skill),
        },
        "installed" => Snapshot::at_commit(
            repo(skill)?,
            skill["installedCommit"].as_str().ok_or(
                "Installed version is unknown; a comparison baseline is not an installation record",
            )?,
        ),
        _ => Err("Choose installed or latest documentation".into()),
    }
}
fn requested_documentation_snapshot(
    m: &Manager,
    skill: &Value,
    req: &Value,
) -> Result<Snapshot, String> {
    if let Some(commit) = req["remoteCommit"].as_str() {
        Snapshot::at_commit(repo(skill)?, commit)
    } else {
        documentation_snapshot(m, skill, req["version"].as_str().unwrap_or("latest"))
    }
}
fn get_document(m: &Manager, req: &Value) -> Result<Value, String> {
    let generation = crate::network::read_generation()?;
    let skill = m.skill(text(req, "skillId")?)?;
    if req["origin"] == "local"
        || skill["sourceId"].is_null() && skill["localPath"].as_str().is_some()
    {
        let root = local_root(&skill)?;
        let path = req["path"].as_str().unwrap_or("SKILL.md");
        let raw = read_local(&root, path)?;
        if raw.len() > 1024 * 1024 {
            return Err("Documentation exceeds the 1 MiB limit".into());
        }
        let fingerprint = hash(&raw);
        let markdown =
            String::from_utf8(raw).map_err(|_| "This local document is not UTF-8 text")?;
        if crate::network::read_generation()? != generation {
            return Err("读取来源或访问凭据已变化，请重新读取说明".into());
        }
        return Ok(
            json!({"markdown":markdown,"documentPath":path,"basePath":path.rsplit_once('/').map(|(p,_)|p).unwrap_or(""),"baseUrl":null,"source":"本地运行文件","commit":null,"localFingerprint":fingerprint,"readGeneration":generation,"checkedAt":now(),"private":false}),
        );
    }
    let snap = requested_documentation_snapshot(m, &skill, req)?;
    let path = match req["path"].as_str() {
        Some(p) => {
            github::validate_relative(p)?;
            p.into()
        }
        None => documentation_path(&skill, &snap)?,
    };
    let raw = snap.read(&path)?;
    if raw.len() > 1024 * 1024 {
        return Err("Documentation exceeds the 1 MiB limit".into());
    }
    let markdown = String::from_utf8(raw).map_err(|_| "This document is not UTF-8 text")?;
    let base_path = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let mut base = Url::parse("https://github.com/").expect("constant URL");
    {
        let mut parts = base
            .path_segments_mut()
            .map_err(|_| "Invalid documentation URL")?;
        for p in snap.api.repo.split('/') {
            parts.push(p);
        }
        parts.push("blob");
        parts.push(&snap.commit);
        for p in path.split('/') {
            parts.push(p);
        }
    }
    if crate::network::read_generation()? != generation {
        return Err("读取来源或访问凭据已变化，请重新读取说明".into());
    }
    Ok(
        json!({"markdown":markdown,"documentPath":path,"basePath":base_path,"baseUrl":base.as_str(),"source":snap.api.repo,"commit":snap.commit,"readGeneration":generation,"checkedAt":now(),"private":snap.private}),
    )
}
fn get_doc_asset(m: &Manager, req: &Value) -> Result<Value, String> {
    let skill = m.skill(text(req, "skillId")?)?;
    let path = text(req, "path")?;
    github::validate_relative(path)?;
    let raw = if req["origin"] == "local"
        || skill["sourceId"].is_null() && skill["localPath"].as_str().is_some()
    {
        read_local(&local_root(&skill)?, path)?
    } else {
        requested_documentation_snapshot(m, &skill, req)?.read(path)?
    };
    let extension = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    // SVG/HTML are deliberately excluded: assets must not execute source-supplied scripts.
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return Err("Only PNG, JPEG, GIF and WebP documentation assets are supported".into()),
    };
    Ok(json!({"dataUrl":format!("data:{mime};base64,{}",STANDARD.encode(raw))}))
}

struct VerifiedSkill {
    scope: Vec<String>,
    all: BTreeMap<String, String>,
    local: BTreeMap<String, String>,
    remote: BTreeMap<String, String>,
    docs: String,
    item: Option<Value>,
    internal_name: String,
    can_adopt: bool,
}

fn verify_skill(skill: &Value, snap: &Snapshot) -> Result<VerifiedSkill, String> {
    let root = remote_path(skill)?.to_owned();
    let entries = snap.skill_files(&root)?;
    if skill["localPath"].as_str().is_some() {
        snap.use_local_files(&local_root(skill)?, &entries)?;
    }
    let (manifest, item) = manifests(snap, &root)?;
    let selected = paths(&skill["runtimeFiles"]);
    let selected_set: BTreeSet<String> = selected.iter().cloned().collect();
    let classification = runtime::inspect(snap, &entries, Some(&selected_set))?;
    let mut scope = selected_set.clone();
    if let Some(declared) = manifest {
        scope.extend(declared);
    }
    scope.extend(classification.required_files);
    scope.insert("SKILL.md".into());
    let scope = checked_selection(&scope.into_iter().collect::<Vec<_>>())?;
    let available: Vec<_> = scope
        .iter()
        .filter(|p| entries.contains_key(*p))
        .cloned()
        .collect();
    let data = download(snap, &entries, &available)?;
    let remote = hashes(&data);
    let internal_name =
        association::internal_name(data.get("SKILL.md").ok_or("Remote SKILL.md is missing")?)?;
    let all = current_hashes(skill)?;
    let local_hashes = subset(&all, &scope);
    let can_adopt = !classification
        .issues
        .iter()
        .any(|issue| issue.severity == "error" || issue.category == "uncertain")
        && skill["runtimeConfirmed"] == true
        && skill["localPath"].as_str().is_some()
        && selected_set.contains("SKILL.md")
        && scope.iter().cloned().collect::<BTreeSet<_>>() == selected_set
        && local_hashes == remote
        && selected
            .iter()
            .all(|p| all.contains_key(p) && remote.contains_key(p))
        && local::validate_runtime_files(&local_root(skill)?, &selected).is_ok();
    let docs = doc_hash(snap, &root, &scope, item.as_ref())?;
    Ok(VerifiedSkill {
        scope,
        all,
        local: local_hashes,
        remote,
        docs,
        item,
        internal_name,
        can_adopt,
    })
}

fn record_verification(skill: &mut Value, commit: &str, private: bool, verified: VerifiedSkill) {
    skill["commit"] = json!(commit);
    if skill["baselineCommit"].is_null() && verified.can_adopt {
        association::reset_observation(skill, &verified.scope);
        skill["baselineCommit"] = json!(commit);
        skill["baselineHashes"] = json!(verified.remote);
        skill["baselineDocHash"] = json!(verified.docs);
        skill["associationBaselineVerifiedAt"] = json!(now());
        skill["baselineKind"] = json!("comparison");
        skill["baselineVerifiedAt"] = json!(now());
    }
    let docs_changed = association::documentation_changed(skill, &verified.docs);
    let status = if skill["localPath"].as_str().is_none() {
        "remote_only"
    } else {
        association::compare_observed(
            skill,
            &verified.local,
            &verified.remote,
            &verified.scope,
            docs_changed,
        )
    };
    skill["status"] = json!(status);
    note_untracked(skill, &verified.all, &verified.scope);
    skill["private"] = json!(private);
    let checked_at = json!(now());
    skill["lastChecked"] = checked_at.clone();
    skill["lastRemoteCheckedAt"] = checked_at.clone();
    if skill["localPath"].as_str().is_some() {
        skill["lastLocalCheckedAt"] = checked_at;
    }
    skill["error"] = Value::Null;
    skill["remoteMissing"] = json!(false);
    skill["remoteHashes"] = json!(verified.remote);
    skill["localHashes"] = json!(verified.local);
    skill["remoteDocHash"] = json!(verified.docs);
    skill["remoteCheckRequired"] = json!(false);
    skill["associationPending"] = json!(false);
    skill["comparisonPending"] = json!(false);
    skill["comparisonScope"] = json!(verified.scope);
    skill["verifiedInternalName"] = json!(verified.internal_name);
    if skill["changeDetails"].is_null() {
        skill["changeDetails"] = json!([]);
    }
    if skill["localPath"].as_str().is_some() && skill["baselineCommit"].is_null() {
        let reason = skill["associationReason"]
            .as_str()
            .unwrap_or("已关联 GitHub 来源");
        let explanation = match status {
            "local_modified" => "本地相对首次观测有变化，安装基线仍未知",
            "remote_updated" => "远端相对首次观测有变化，安装基线仍未知",
            "both_modified" => "双方相对首次观测均有变化，安装基线仍未知",
            "synced" => "当前比较内容一致，运行清单尚未完整确认，未冒认安装版本",
            "docs_updated" => "运行内容一致，远端说明相对首次观测有更新，安装基线仍未知",
            _ => "首次内容不同，已保存双方观测值，尚不能判断哪侧较新",
        };
        skill["associationNote"] = json!(format!("{reason}；{explanation}"));
    }
    if let Some(item) = verified.item {
        skill["catalogMetadata"] = item.clone();
        if let Some(p) = item["usage"].as_str() {
            skill["documentationPath"] = json!(p);
        }
    }
}

fn obsolete_development_selection(path: &str) -> bool {
    path.split('/')
        .any(|part| matches!(part, "tests" | "__pycache__"))
        || path.ends_with(".pyc")
}

// A stale unconfirmed suggestion is not evidence that a file is required.
// Repair only absent development entries, using a complete catalog candidate
// and fresh equality proof. The original record survives every failed proof.
fn rebuilt_automatic_scope(
    skill: &Value,
    manifest: Option<&[String]>,
    item: Option<&Value>,
    verify: impl FnOnce(&Value) -> Result<VerifiedSkill, String>,
) -> Result<Option<(Value, VerifiedSkill)>, String> {
    if crate::audit::preserves_manual_scope(skill)
        || skill["readOnly"] == true
        || skill["localPath"].as_str().is_none()
        || !(skill["runtimeConfirmationKind"] == "automatic"
            || skill["runtimeConfirmed"] == false
                && skill["runtimeAudit"]["method"] == "static_package")
    {
        return Ok(None);
    }
    let Some(manifest) = manifest else {
        return Ok(None);
    };
    let old: BTreeSet<_> = paths(&skill["runtimeFiles"]).into_iter().collect();
    let mut proposed: BTreeSet<_> = checked_selection(manifest)?.into_iter().collect();
    if !old
        .difference(&proposed)
        .any(|path| obsolete_development_selection(path))
    {
        return Ok(None);
    }
    let inventory = current_hashes(skill)?;
    // Existing extra choices are retained, even for automatic/legacy records.
    proposed.extend(
        old.iter()
            .filter(|path| inventory.contains_key(*path))
            .cloned(),
    );
    let removed: Vec<_> = old.difference(&proposed).collect();
    if removed.is_empty()
        || removed
            .iter()
            .any(|path| !obsolete_development_selection(path))
        || proposed.iter().any(|path| !inventory.contains_key(path))
    {
        return Ok(None);
    }
    let mut candidate = skill.clone();
    candidate["runtimeFiles"] = json!(proposed);
    candidate["runtimeConfirmed"] = json!(false);
    if let Some(item) = item {
        candidate["catalogMetadata"] = item.clone();
    }
    // Invalidate only observations, never installation history.
    let candidate_scope = paths(&candidate["runtimeFiles"]);
    association::reset_observation(&mut candidate, &candidate_scope);
    candidate["runtimeAudit"] = Value::Null;
    crate::audit::refresh_local_audit(&mut candidate, Some(&inventory));
    if candidate["runtimeAudit"]["status"] != "passed"
        || candidate["runtimeAudit"]["canAutoConfirm"] != true
        || candidate["runtimeConfirmed"] != true
    {
        return Ok(None);
    }
    let verified = verify(&candidate)?;
    if !verified.can_adopt
        || verified.local != verified.remote
        || verified.all != inventory
        || current_hashes(&candidate)? != inventory
    {
        return Ok(None);
    }
    candidate["runtimeConfirmationKind"] = json!("automatic");
    Ok(Some((candidate, verified)))
}

fn refresh_skill(skill: &mut Value, snap: &Snapshot) -> Result<(), String> {
    let (manifest, item) = manifests(snap, remote_path(skill)?)?;
    let repair = rebuilt_automatic_scope(skill, manifest.as_deref(), item.as_ref(), |candidate| {
        verify_skill(candidate, snap)
    })?;
    let (mut next, verified) = match repair {
        Some(repaired) => repaired,
        None => (skill.clone(), verify_skill(skill, snap)?),
    };
    refresh_declared_local_version(&mut next);
    if let Some(item) = &verified.item {
        next["catalogMetadata"] = item.clone();
    }
    local::invalidate_runtime_audit(&mut next, &verified.all);
    crate::audit::refresh_local_audit(&mut next, Some(&verified.all));
    record_verification(&mut next, &snap.commit, snap.private, verified);
    *skill = next;
    Ok(())
}

fn record_push_verification(
    skill: &mut Value,
    commit: &str,
    private: bool,
    verified: VerifiedSkill,
) {
    if verified.can_adopt {
        association::reset_observation(skill, &verified.scope);
        skill["baselineCommit"] = json!(commit);
        skill["baselineHashes"] = json!(verified.remote);
        skill["baselineDocHash"] = json!(verified.docs);
        skill["baselineKind"] = json!("comparison");
        skill["baselineVerifiedAt"] = json!(now());
    }
    record_verification(skill, commit, private, verified);
}

fn finish_auto_pair(
    local: &Value,
    remote: &Value,
    left: &association::Identity,
    right: &association::Identity,
    verified: VerifiedSkill,
    commit: &str,
    private: bool,
) -> Result<Value, String> {
    let same_bytes = verified.local == verified.remote && !verified.local.is_empty();
    let same_directories = left.folder == left.name && right.folder == right.name;
    if !same_directories && !same_bytes {
        return Err("内部 name 相同，但目录标识和完整比较内容不一致，请手动关联".into());
    }
    let reason = if same_directories {
        "内部 name 唯一且两端目录标识一致，已自动关联"
    } else {
        "内部 name 唯一且运行内容逐字节一致，已自动关联"
    };
    let mut merged = association::merge(local, remote, reason);
    merged["associationReason"] = json!(reason);
    record_verification(&mut merged, commit, private, verified);
    Ok(merged)
}

fn retain_local_merge(m: &mut Manager, merged: Value, remote_id: &str) {
    m.upsert_skill(merged);
    m.state.skills.retain(|s| s["id"] != remote_id);
}

fn protected_record(skill: &Value) -> bool {
    skill["readOnly"] == true
        || skill["managed"] == true
        || skill["pluginManaged"] == true
        || skill["systemManaged"] == true
}

fn auto_associate_with(m: &mut Manager, snapshots: &BTreeMap<String, Snapshot>) {
    auto_associate_selected(m, snapshots, &None);
}

fn auto_associate_selected(
    m: &mut Manager,
    snapshots: &BTreeMap<String, Snapshot>,
    selected: &Option<BTreeSet<String>>,
) {
    let records = m.state.skills.clone();
    let mut local_identities = Vec::new();
    let mut remote_identities = Vec::new();
    let mut local_identity_hashes = BTreeMap::new();
    let mut verified_remotes = BTreeMap::new();
    for skill in &records {
        let Some(id) = skill["id"].as_str() else {
            continue;
        };
        if skill["localPath"].as_str().is_some() {
            let result = (|| -> Result<(String, String, bool), String> {
                let root = local_root(skill)?;
                let bytes = read_local(&root, "SKILL.md")?;
                Ok((
                    association::internal_name(&bytes)?,
                    hash(&bytes),
                    local::readonly_reason(&root).is_none() && !protected_record(skill),
                ))
            })();
            let (name, verified, eligible) = match result {
                Ok((name, h, allowed)) => {
                    local_identity_hashes.insert(id.to_owned(), h);
                    (
                        name,
                        true,
                        allowed
                            && refresh_matches(selected, skill)
                            && skill["sourceId"].is_null()
                            && skill["baselineCommit"].is_null(),
                    )
                }
                Err(_) => (
                    skill["name"].as_str().unwrap_or("").to_owned(),
                    false,
                    false,
                ),
            };
            local_identities.push(association::Identity {
                id: id.into(),
                name,
                folder: association::folder_name(skill["localPath"].as_str().unwrap_or("")).into(),
                eligible,
                verified,
            });
        } else if skill["sourceId"].as_str().is_some() {
            let source = m
                .state
                .sources
                .iter()
                .find(|s| s["id"] == skill["sourceId"]);
            let source_ok = source.is_some_and(|s| {
                s["error"].is_null()
                    && ignored_paths(s)
                        .is_ok_and(|ignored| !ignored.iter().any(|p| skill["remotePath"] == *p))
            }) && skill["error"].is_null()
                && skill["remoteMissing"] != true
                && !protected_record(skill);
            let snap = skill["sourceId"].as_str().and_then(|s| snapshots.get(s));
            let identity = if source_ok {
                snap.and_then(|s| {
                    remote_path(skill)
                        .ok()
                        .and_then(|p| s.read(&join(p, "SKILL.md")).ok())
                })
                .and_then(|bytes| association::internal_name(&bytes).ok())
            } else {
                None
            };
            let verified = identity.is_some();
            let name = identity.unwrap_or_else(|| {
                skill["verifiedInternalName"]
                    .as_str()
                    .or_else(|| skill["name"].as_str())
                    .unwrap_or("")
                    .to_owned()
            });
            let mut remote = skill.clone();
            if verified {
                remote["verifiedInternalName"] = json!(name);
            }
            verified_remotes.insert(id.to_owned(), remote);
            remote_identities.push(association::Identity {
                id: id.into(),
                name,
                folder: association::folder_name(skill["remotePath"].as_str().unwrap_or("")).into(),
                eligible: source_ok && verified,
                verified,
            });
        }
    }
    let pairs = association::unique_pairs(&local_identities, &remote_identities);
    let matched: BTreeSet<_> = pairs.iter().map(|(id, _)| id.clone()).collect();
    let complete_sources = m.state.sources.iter().all(|source| {
        source["error"].is_null() && snapshots.contains_key(source["id"].as_str().unwrap_or(""))
    });
    for identity in local_identities.iter().filter(|i| !matched.contains(&i.id)) {
        if remote_identities.iter().any(|candidate| {
            candidate.name == identity.name
                && verified_remotes
                    .get(&candidate.id)
                    .is_some_and(|remote| remote["remoteMissing"] != true)
        }) {
            if let Some(local) = m.state.skills.iter_mut().find(|s| {
                s["id"] == identity.id && s["sourceId"].is_null() && refresh_matches(selected, s)
            }) {
                local["associationPending"] = json!(!protected_record(local));
                local["associationNote"] = json!(if !identity.eligible {
                    "本地项目受托管保护、身份未核实或已有来源，保留独立记录"
                } else {
                    "同名候选不唯一或来源尚未核实，保留独立记录供手动关联"
                });
            }
        } else if complete_sources {
            if let Some(local) = m
                .state
                .skills
                .iter_mut()
                .find(|skill| skill["id"] == identity.id && refresh_matches(selected, skill))
            {
                if local["associationPending"] == true {
                    local["associationPending"] = json!(false);
                    local["associationNote"] = Value::Null;
                }
            }
        }
    }
    for (local_id, remote_id) in pairs {
        let Some(local) = m
            .state
            .skills
            .iter()
            .find(|s| s["id"] == local_id && s["sourceId"].is_null())
            .cloned()
        else {
            continue;
        };
        let Some(remote) = verified_remotes.get(&remote_id) else {
            continue;
        };
        let Some(snap) = remote["sourceId"].as_str().and_then(|s| snapshots.get(s)) else {
            continue;
        };
        let Some(li) = local_identities.iter().find(|i| i.id == local_id) else {
            continue;
        };
        let Some(ri) = remote_identities.iter().find(|i| i.id == remote_id) else {
            continue;
        };
        let result = (|| -> Result<Value, String> {
            if m.state
                .connections
                .iter()
                .any(|c| c["skillId"] == remote_id)
            {
                return Err("远端占位记录存在客户端连接，请人工核对后关联".into());
            }
            let merged = association::merge(&local, remote, "内部 name 唯一匹配，已自动关联");
            let verified = verify_skill(&merged, snap)?;
            if local_identity_hashes.get(&local_id) != verified.local.get("SKILL.md") {
                return Err("核对期间本地身份文件变化，已保留独立记录".into());
            }
            finish_auto_pair(&local, remote, li, ri, verified, &snap.commit, snap.private)
        })();
        match result {
            Ok(merged) => retain_local_merge(m, merged, &remote_id),
            Err(error) => {
                if let Some(local) = m.state.skills.iter_mut().find(|s| s["id"] == local_id) {
                    local["associationPending"] = json!(true);
                    local["associationNote"] = json!(format!("自动关联未完成：{error}"));
                }
            }
        }
    }
}

fn save_association_state(m: &mut Manager, previous: crate::model::State) -> Result<Value, String> {
    if let Err(error) = m.save() {
        m.state = previous;
        return Err(error);
    }
    Ok(m.snapshot())
}

fn auto_associate(m: &mut Manager) -> Result<Value, String> {
    let previous = m.state.clone();
    let mut snapshots = BTreeMap::new();
    if m.state.skills.iter().any(|s| {
        s["localPath"].as_str().is_some() && s["sourceId"].is_null() && !protected_record(s)
    }) {
        for source in m.state.sources.clone() {
            if !source["error"].is_null() {
                continue;
            }
            let loaded = (|| -> Result<Snapshot, String> {
                Snapshot::load(text(&source, "repo")?, source["branch"].as_str())
            })();
            match loaded {
                Ok(snap) => {
                    update_source(m, &snap, &source["id"]);
                    let mut discovered_source = source.clone();
                    discovered_source["commit"] = json!(snap.commit);
                    discovered_source["lastChecked"] = json!(now());
                    discovered_source["private"] = json!(snap.private);
                    if discovered_source.get("roots").is_none() {
                        discovered_source["roots"] = json!(source_roots(&source, &m.state.skills)?);
                    }
                    let catalog = snap.catalog()?;
                    reconcile_discovery(
                        &mut m.state.skills,
                        &discovered_source,
                        &snap.tree,
                        &catalog_items(catalog.as_ref()),
                    )?;
                    if let Some(id) = source["id"].as_str() {
                        snapshots.insert(id.to_owned(), snap);
                    }
                }
                Err(error) => {
                    if let Some(s) = m.state.sources.iter_mut().find(|s| s["id"] == source["id"]) {
                        s["error"] = json!(error);
                        s["lastChecked"] = json!(now());
                    }
                    for skill in m
                        .state
                        .skills
                        .iter_mut()
                        .filter(|s| s["sourceId"] == source["id"])
                    {
                        skill["error"] = json!(error);
                        skill["remoteCheckRequired"] = json!(true);
                    }
                }
            }
        }
        auto_associate_with(m, &snapshots);
    }
    save_association_state(m, previous)
}

fn refresh(m: &mut Manager, req: &Value) -> Result<Value, String> {
    if req["localOnly"] == true {
        return refresh_local(m, req);
    }
    let selected = m.refresh_selection(req)?;
    let previous = m.state.clone();
    let automatic = req["automatic"] == true;
    let remote_only = req["remoteOnly"] == true;
    let mut snapshots = BTreeMap::new();
    // Local declarations remain readable even when the remote request later fails.
    for skill in m.state.skills.iter_mut().filter(|s| {
        !remote_only && s["localPath"].as_str().is_some() && refresh_matches(&selected, s)
    }) {
        refresh_declared_local_version(skill);
        match current_hashes(skill) {
            Ok(current) => {
                local::invalidate_runtime_audit(skill, &current);
                crate::audit::refresh_local_audit(skill, Some(&current));
                skill["lastLocalCheckedAt"] = json!(now());
            }
            Err(_) => {
                local::invalidate_runtime_audit(skill, &BTreeMap::new());
                crate::audit::refresh_local_audit(skill, None);
            }
        }
    }
    let selected_unlinked = m.state.skills.iter().any(|skill| {
        refresh_matches(&selected, skill)
            && skill["localPath"].is_string()
            && skill["sourceId"].is_null()
            && !protected_record(skill)
    });
    let ids: Vec<Value> = m
        .state
        .sources
        .iter()
        .filter(|s| {
            selected.is_none()
                || selected_unlinked
                || m.state
                    .skills
                    .iter()
                    .any(|k| refresh_matches(&selected, k) && k["sourceId"] == s["id"])
        })
        .map(|s| s["id"].clone())
        .collect();
    for id in ids {
        crate::network::check_cancelled()?;
        let source = m
            .state
            .sources
            .iter()
            .find(|s| s["id"] == id)
            .cloned()
            .ok_or("Missing source")?;
        let fingerprint = check_fingerprint(&source, &m.state.skills)?;
        let loaded = if automatic {
            Snapshot::probe(
                text(&source, "repo")?,
                source["branch"].as_str(),
                source["commit"].as_str(),
                remote_only
                    && source["error"].is_null()
                    && source["checkFingerprint"] == fingerprint
                    && m.state
                        .skills
                        .iter()
                        .filter(|skill| skill["sourceId"] == id)
                        .all(|skill| {
                            skill["commit"] == source["commit"]
                                && skill["remoteCheckRequired"] != true
                        }),
            )
        } else {
            Snapshot::load(text(&source, "repo")?, source["branch"].as_str()).map(Some)
        };
        match loaded {
            Ok(None) => continue,
            Ok(Some(snap)) => {
                update_source(m, &snap, &id);
                let mut refreshed_source = m
                    .state
                    .sources
                    .iter()
                    .find(|s| s["id"] == id)
                    .cloned()
                    .ok_or("Missing refreshed source")?;
                let (items, catalog_error) = match snap.catalog() {
                    Ok(catalog) => (catalog_items(catalog.as_ref()), None),
                    Err(error) => (vec![], Some(error)),
                };
                let discovery = (|| -> Result<(), String> {
                    if refreshed_source.get("roots").is_none() {
                        refreshed_source["roots"] =
                            json!(source_roots(&refreshed_source, &m.state.skills)?);
                        refreshed_source["rootsInferred"] = json!(true);
                    }
                    reconcile_discovery(&mut m.state.skills, &refreshed_source, &snap.tree, &items)
                })();
                if let Some(error) = discovery.err().or(catalog_error) {
                    refreshed_source["error"] = json!(error);
                }
                if let Some(stored) = m.state.sources.iter_mut().find(|s| s["id"] == id) {
                    *stored = refreshed_source;
                }
                for skill in m
                    .state
                    .skills
                    .iter_mut()
                    .filter(|s| s["sourceId"] == id && refresh_matches(&selected, s))
                {
                    if selected.is_some() {
                        if let Ok(root) = remote_path(skill) {
                            let path = if root.is_empty() {
                                "SKILL.md".to_string()
                            } else {
                                format!("{root}/SKILL.md")
                            };
                            let missing = !snap.tree.contains_key(&path);
                            skill["remoteMissing"] = json!(missing);
                            if missing {
                                skill["error"] = json!("Remote SKILL.md is missing; local files, record and baseline were preserved.");
                                skill["lastChecked"] = json!(now());
                            }
                        }
                    }
                    if skill["remoteMissing"] == true {
                        continue;
                    }
                    if let Err(e) = refresh_skill(skill, &snap) {
                        skill["error"] = json!(e);
                        skill["lastChecked"] = json!(now());
                        skill["remoteCheckRequired"] = json!(true);
                    }
                }
                if let Some(id) = id.as_str() {
                    snapshots.insert(id.to_owned(), snap);
                }
            }
            Err(error) => {
                if let Some(s) = m.state.sources.iter_mut().find(|s| s["id"] == id) {
                    s["error"] = json!(error);
                    s["lastChecked"] = json!(now());
                }
                for skill in m
                    .state
                    .skills
                    .iter_mut()
                    .filter(|s| s["sourceId"] == id && refresh_matches(&selected, s))
                {
                    skill["error"] = json!(error);
                    skill["lastChecked"] = json!(now());
                    skill["remoteCheckRequired"] = json!(true);
                }
            }
        }
    }
    for skill in m
        .state
        .skills
        .iter_mut()
        .filter(|s| !remote_only && s["sourceId"].is_null() && refresh_matches(&selected, s))
    {
        skill["lastChecked"] = json!(now());
        match current_hashes(skill) {
            Ok(h) => {
                skill["lastLocalCheckedAt"] = json!(now());
                skill["localHashes"] = json!(h);
                skill["error"] = Value::Null;
                skill["status"] = json!("local_only");
                if skill["changeDetails"].is_null() {
                    skill["changeDetails"] = json!([]);
                }
            }
            Err(e) => skill["error"] = json!(e),
        }
    }
    if !snapshots.is_empty() || !remote_only {
        auto_associate_selected(m, &snapshots, &selected);
    }
    if selected.is_some() {
        preserve_unselected(&mut m.state.skills, &previous.skills, &selected);
    }
    for source in &mut m.state.sources {
        if snapshots.contains_key(source["id"].as_str().unwrap_or("")) && source["error"].is_null()
        {
            let already_discovered = previous
                .sources
                .iter()
                .find(|old| old["id"] == source["id"])
                .is_some_and(|old| {
                    old["commit"] == source["commit"]
                        && check_fingerprint(old, &previous.skills)
                            .is_ok_and(|stamp| old["checkFingerprint"] == stamp)
                });
            source["checkFingerprint"] = if selected.is_none() || already_discovered {
                json!(check_fingerprint(source, &m.state.skills)?)
            } else {
                // Selected checks intentionally omit unrelated discoveries and
                // preserve their records; the next automatic full check must
                // still reconcile the rest of this source at the same commit.
                Value::Null
            };
        }
    }
    if automatic && semantic_state(&m.state)? == semantic_state(&previous)? {
        m.state = previous;
        return Ok(m.snapshot());
    }
    save_association_state(m, previous)
}

fn check_fingerprint(source: &Value, skills: &[Value]) -> Result<String, String> {
    let candidates: Vec<_> = skills
        .iter()
        .filter(|skill| skill["sourceId"].is_null() && skill["localPath"].is_string())
        .map(|skill| json!({"id":skill["id"],"path":skill["localPath"],"entry":skill["localHashes"]["SKILL.md"],"protected":protected_record(skill)}))
        .collect();
    Ok(hash(
        &serde_json::to_vec(&json!({
            "roots":source_roots(source, skills)?,"ignored":ignored_paths(source)?,
            "repo":source["repo"],"branch":source["branch"],"candidates":candidates,
            "runtimeAnalyzerVersion":local::RUNTIME_ANALYZER_VERSION
        }))
        .map_err(|e| e.to_string())?,
    ))
}

/// Candidate discovery checks the complete source to reject ambiguous names;
/// a selected check commits only its chosen records. A merged remote placeholder
/// is consumed only when its selected local identity now carries that association.
fn preserve_unselected(
    skills: &mut Vec<Value>,
    before: &[Value],
    selected: &Option<BTreeSet<String>>,
) {
    let checked: Vec<_> = skills
        .iter()
        .filter(|s| refresh_matches(selected, s))
        .cloned()
        .collect();
    *skills = before
        .iter()
        .filter_map(|old| {
            if refresh_matches(selected, old) {
                return checked.iter().find(|s| s["id"] == old["id"]).cloned();
            }
            let consumed = old["localPath"].is_null()
                && checked.iter().any(|current| {
                    current["localPath"].is_string()
                        && !current["sourceId"].is_null()
                        && current["sourceId"] == old["sourceId"]
                        && current["remotePath"] == old["remotePath"]
                });
            (!consumed).then(|| old.clone())
        })
        .collect();
}

fn semantic_state(state: &crate::model::State) -> Result<Value, String> {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for key in [
                    "lastChecked",
                    "lastLocalChecked",
                    "lastLocalCheckedAt",
                    "lastRemoteCheckedAt",
                    "checkedAt",
                    "baselineVerifiedAt",
                    "associationBaselineVerifiedAt",
                    "comparisonObservedAt",
                ] {
                    map.remove(key);
                }
                for item in map.values_mut() {
                    strip(item);
                }
            }
            Value::Array(items) => {
                for item in items {
                    strip(item);
                }
            }
            _ => {}
        }
    }
    let mut value = serde_json::to_value(state).map_err(|e| e.to_string())?;
    strip(&mut value);
    Ok(value)
}

pub(crate) fn check(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let before = m.state.skills.clone();
    let mut result = refresh(m, req)?;
    let associated = m
        .state
        .skills
        .iter()
        .filter(|skill| {
            !skill["sourceId"].is_null()
                && skill["localPath"].is_string()
                && before
                    .iter()
                    .any(|old| old["id"] == skill["id"] && old["sourceId"].is_null())
        })
        .count();
    let new_remote = m
        .state
        .skills
        .iter()
        .filter(|skill| {
            skill["localPath"].is_null() && !before.iter().any(|old| old["id"] == skill["id"])
        })
        .count();
    let errors = m
        .state
        .skills
        .iter()
        .filter(|skill| !skill["error"].is_null())
        .count();
    let attention = attention_count(&m.state.skills);
    let association_pending = m
        .state
        .skills
        .iter()
        .filter(|skill| skill["associationPending"] == true)
        .count();
    let docs_updated = documentation_updates(&before, &m.state.skills);
    result["checkSummary"] = json!({"newRemote":new_remote,"associated":associated,"associationPending":association_pending,"needsAttention":attention,"errors":errors,"docsUpdated":docs_updated});
    Ok(result)
}

fn documentation_updates(before: &[Value], current: &[Value]) -> usize {
    current
        .iter()
        .filter(|skill| {
            skill["status"] == "docs_updated"
                && skill["error"].is_null()
                && skill["remoteCheckRequired"] != true
                && skill["remoteDocHash"].is_string()
                && before.iter().any(|old| {
                    old["id"] == skill["id"]
                        && old["remoteDocHash"].is_string()
                        && old["remoteDocHash"] != skill["remoteDocHash"]
                        && ["sourceId", "repo", "branch", "remotePath"]
                            .iter()
                            .all(|field| old[*field] == skill[*field])
                })
        })
        .count()
}

fn skill_needs_attention(skill: &Value) -> bool {
    let runtime_attention = match skill.get("runtimeIssues") {
        Some(Value::Array(issues)) => issues.iter().any(|issue| {
            issue["severity"] != "info"
                && matches!(
                    issue["category"].as_str(),
                    Some("package_required" | "uncertain")
                )
        }),
        Some(_) => false,
        None => skill["runtimeWarnings"]
            .as_array()
            .is_some_and(|warnings| !warnings.is_empty()),
    };
    !skill["error"].is_null()
        || skill["remoteMissing"] == true
        || skill["remoteCheckRequired"] == true
        || matches!(
            skill["status"].as_str(),
            Some(
                "remote_updated"
                    | "local_modified"
                    | "both_modified"
                    | "unknown_baseline"
                    | "local_extra"
                    | "remote_missing"
                    | "remote_check_required"
                    | "error"
            )
        )
        || skill["localNotice"]
            .as_str()
            .is_some_and(|notice| !notice.is_empty())
        || skill["untrackedFiles"]
            .as_array()
            .is_some_and(|files| !files.is_empty())
        || runtime_attention
        || matches!(
            skill["runtimeAudit"]["status"].as_str(),
            Some("needs_review" | "failed")
        )
        || skill["associationPending"] == true
}

fn attention_count(skills: &[Value]) -> usize {
    skills
        .iter()
        .filter(|skill| skill_needs_attention(skill))
        .count()
}
fn refresh_matches(selected: &Option<BTreeSet<String>>, skill: &Value) -> bool {
    selected
        .as_ref()
        .is_none_or(|ids| skill["id"].as_str().is_some_and(|id| ids.contains(id)))
}
fn refresh_local(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let selected = m.refresh_selection(req)?;
    let previous = m.state.clone();
    let source_errors: BTreeMap<String, Value> = m
        .state
        .sources
        .iter()
        .filter(|s| !s["error"].is_null())
        .filter_map(|s| {
            s["id"]
                .as_str()
                .map(|id| (id.to_owned(), s["error"].clone()))
        })
        .collect();
    for skill in m
        .state
        .skills
        .iter_mut()
        .filter(|s| s["localPath"].as_str().is_some() && refresh_matches(&selected, s))
    {
        skill["lastLocalChecked"] = json!(now());
        refresh_declared_local_version(skill);
        match current_hashes(skill) {
            Err(error) => {
                local::invalidate_runtime_audit(skill, &BTreeMap::new());
                crate::audit::refresh_local_audit(skill, None);
                skill["error"] = json!(error);
            }
            Ok(all) => {
                local::invalidate_runtime_audit(skill, &all);
                crate::audit::refresh_local_audit(skill, Some(&all));
                skill["lastLocalCheckedAt"] = json!(now());
                let runtime = paths(&skill["runtimeFiles"]);
                let scope = if skill["comparisonScope"].is_array() {
                    paths(&skill["comparisonScope"])
                } else {
                    runtime.clone()
                };
                let local_hashes = subset(&all, &scope);
                skill["localHashes"] = json!(local_hashes);
                if skill["sourceId"].is_null() {
                    skill["status"] = json!("local_only");
                    skill["error"] = Value::Null;
                    if skill["changeDetails"].is_null() {
                        skill["changeDetails"] = json!([]);
                    }
                    continue;
                }
                if skill["remoteMissing"] == true {
                    note_untracked(skill, &all, &scope);
                    continue;
                }
                if let Some(error) = skill["sourceId"]
                    .as_str()
                    .and_then(|id| source_errors.get(id))
                {
                    skill["error"] = error.clone();
                    skill["remoteCheckRequired"] = json!(true);
                    skill["status"] = json!("unknown_baseline");
                    skill["associationNote"] = json!(
                        "本地文件已检查，但远端来源检查失败；保留上次观测，不宣称当前远端已同步"
                    );
                    let base = if skill["comparisonLocalHashes"].is_object() {
                        mapping(&skill["comparisonLocalHashes"])
                    } else {
                        subset(&mapping(&skill["baselineHashes"]), &scope)
                    };
                    skill["changeDetails"] = json!(association::details(
                        &base,
                        &BTreeMap::new(),
                        &local_hashes,
                        &BTreeMap::new(),
                        false
                    ));
                    note_untracked(skill, &all, &scope);
                    continue;
                }
                if skill["remoteHashes"].is_object() {
                    let remote = mapping(&skill["remoteHashes"]);
                    let docs = skill["remoteDocHash"].as_str().map(str::to_owned);
                    let docs_changed = docs
                        .as_deref()
                        .is_some_and(|hash| association::documentation_changed(skill, hash));
                    skill["status"] = json!(association::compare_observed(
                        skill,
                        &local_hashes,
                        &remote,
                        &scope,
                        docs_changed
                    ));
                } else {
                    skill["remoteCheckRequired"] = json!(true);
                    skill["status"] = json!("unknown_baseline");
                    skill["error"]=json!("No verified remote comparison is cached. Check updates to establish the remote state.");
                }
                note_untracked(skill, &all, &scope);
            }
        }
    }
    save_association_state(m, previous)
}
fn note_untracked(skill: &mut Value, all: &BTreeMap<String, String>, scope: &[String]) {
    let untracked: Vec<_> = all.keys().filter(|p| !scope.contains(p)).cloned().collect();
    skill["untrackedFiles"] = json!(untracked);
    if untracked.is_empty() {
        skill["localNotice"] = Value::Null;
    } else {
        skill["localNotice"] = json!("存在未纳入当前比较范围的本地文件；请核对是否需要纳入运行清单。这不表示这些文件已属于待上传修改。");
        if skill["status"] == "synced" {
            skill["status"] = json!("local_extra");
        }
    }
}

fn skill_stamp(skill: &Value) -> String {
    hash(&serde_json::to_vec(&json!({"localPath":skill["localPath"],"sourceId":skill["sourceId"],"repo":skill["repo"],"branch":skill["branch"],
        "remotePath":skill["remotePath"],"baselineCommit":skill["baselineCommit"],"baselineHashes":skill["baselineHashes"],
        "runtimeFiles":skill["runtimeFiles"],"runtimeConfirmed":skill["runtimeConfirmed"],"readOnly":skill["readOnly"]})).expect("serializable stamp"))
}
fn preview_text(data: &[u8]) -> String {
    if data.len() > 128 * 1024 {
        return format!("[File too large for inline preview: {} bytes]", data.len());
    }
    std::str::from_utf8(data)
        .map(str::to_owned)
        .unwrap_or_else(|_| format!("[Binary file: {} bytes]", data.len()))
}
fn target_directory(input: &str) -> Result<PathBuf, String> {
    let path = Path::new(input);
    if !path.is_absolute()
        || path
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        return Err("Choose an absolute installation directory without parent traversal".into());
    }
    if path.exists() {
        let path = local::canonical(path)?;
        if !path.is_dir() {
            return Err("Installation target is not a directory".into());
        }
        return Ok(path);
    }
    let mut missing = Vec::new();
    let mut parent = path;
    while !parent.exists() {
        let name = parent
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("Invalid installation path")?;
        github::validate_relative(name)?;
        missing.push(name.to_owned());
        parent = parent
            .parent()
            .ok_or("Installation path has no existing parent")?;
    }
    let mut resolved = local::canonical(parent)?;
    for part in missing.iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn checked_preview_target(input: &str) -> Result<PathBuf, String> {
    // Preview resolved the user's original choice once. Validate that exact
    // path at apply time; canonicalizing again would hide a newly inserted
    // junction and authorize a different directory with identical file bytes.
    let recorded = PathBuf::from(input);
    transaction::verify_root(&recorded)?;
    if recorded.exists() && !recorded.is_dir() {
        return Err("The preview installation target is no longer a directory".into());
    }
    Ok(recorded)
}
fn selection_preview(
    skill: &Value,
    snap: &Snapshot,
    entries: &BTreeMap<String, Entry>,
    selected: Option<&BTreeSet<String>>,
    mut warnings: Vec<String>,
) -> Result<Value, String> {
    let classification = runtime::inspect(snap, entries, selected)?;
    warnings.extend(classification.warnings.clone());
    warnings.sort();
    warnings.dedup();
    let available: Vec<_> = entries
        .iter()
        .filter(|(p, e)| {
            github::validate_relative(p).is_ok() && ["100644", "100755"].contains(&e.mode.as_str())
        })
        .map(|(p, e)| json!({"path":p,"size":e.size,"runtime":classification.required_files.contains(p)}))
        .collect();
    Ok(
        json!({"id":null,"kind":"runtime_selection","skillId":skill["id"],"summary":"Confirm which files form the runtime package","files":[],"warnings":warnings,
        "canApply":false,"needsRuntimeSelection":true,"availableFiles":available,"requiredFiles":classification.required_files,"optionalFiles":classification.optional_files,
        "suggestedFiles":classification.suggested_files,"requiredReasons":classification.required_reasons,"issues":classification.issues}),
    )
}

fn preview_install(m: &mut Manager, req: &Value, updating: bool) -> Result<Value, String> {
    let skill = m.skill(text(req, "skillId")?)?;
    writable(&skill)?;
    if !updating && skill["localPath"].as_str().is_some() {
        return Err("This Skill already has a local directory; use Update".into());
    }
    let snap = snapshot(m, &skill)?;
    let root_path = remote_path(&skill)?;
    let entries = snap.skill_files(root_path)?;
    let (manifest, item) = manifests(&snap, root_path)?;
    let chosen = if req.get("files").is_some() {
        Some(strings(&req["files"])?)
    } else {
        manifest.or_else(|| {
            if updating && skill["runtimeConfirmed"] == true {
                Some(paths(&skill["runtimeFiles"]))
            } else {
                None
            }
        })
    };
    let Some(mut selected) = chosen else {
        return selection_preview(&skill,&snap,&entries,None,vec!["This source has no runtime manifest. Choose the required runtime files explicitly.".into()]);
    };
    if updating && req.get("files").is_none() {
        selected.retain(|p| entries.contains_key(p));
    }
    let selected = checked_selection(&selected)?;
    let selected_set = selected.iter().cloned().collect();
    let classification = runtime::inspect(&snap, &entries, Some(&selected_set))?;
    let missing: Vec<_> = classification
        .required_files
        .iter()
        .filter(|p| !selected_set.contains(*p))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return selection_preview(
            &skill,
            &snap,
            &entries,
            Some(&selected_set),
            vec![format!(
                "Include these required runtime files: {}",
                missing.join(", ")
            )],
        );
    }
    let data = download(&snap, &entries, &selected)?;
    let reference_warnings = match validate_runtime(&entries, &selected, &data) {
        Ok(w) => w,
        Err(error) => {
            return selection_preview(&skill, &snap, &entries, Some(&selected_set), vec![error])
        }
    };
    let target = if updating {
        local_root(&skill)?
    } else {
        target_directory(text(req, "targetPath")?)?
    };
    let expected = if target.exists() {
        local::files(&target)?
    } else {
        BTreeMap::new()
    };
    if !updating
        && target.exists()
        && fs::read_dir(&target)
            .map_err(|_| "Cannot inspect installation directory")?
            .next()
            .is_some()
    {
        return Err(
            "Installation target must be new or empty. Existing files will not be overwritten."
                .into(),
        );
    }
    let baseline = mapping(&skill["baselineHashes"]);
    let old_runtime = paths(&skill["runtimeFiles"]);
    let new_hashes = hashes(&data);
    let baseline_known = skill["baselineCommit"].as_str().is_some();
    let safety = logic::update_safety(
        (updating && baseline_known).then_some(&baseline),
        &expected,
        &old_runtime,
        &new_hashes,
        &entries.keys().cloned().collect(),
    );
    let mut warnings = reference_warnings;
    let mut can_apply = true;
    if updating {
        if skill["baselineCommit"].as_str().is_none() {
            if subset(&expected, &selected) == new_hashes {
                warnings.push("The selected local runtime bytes exactly match this remote commit. Confirming records that verified baseline without replacing any file.".into());
            } else {
                warnings.push("The installed baseline is unknown and local content differs. Review or submit your changes first; this operation will not overwrite the unverified installation.".into());
                can_apply = false;
            }
        } else if safety.local_changes {
            warnings.push("Local runtime changes exist. Preserve or submit them before updating; automatic merging is disabled.".into());
            can_apply = false;
        }
    }
    let deletes = safety.deletes;
    let mut rows = Vec::new();
    for p in selected.iter().chain(deletes.iter()) {
        let old = expected.get(p);
        let new = new_hashes.get(p);
        let conflict = updating && safety.conflicts.contains(p);
        if conflict {
            can_apply = false;
            warnings.push(format!("An untracked local file would be overwritten: {p}"));
        }
        let change = if conflict {
            "conflict"
        } else {
            match (old, new) {
                (Some(a), Some(b)) if a == b => "unchanged",
                (Some(_), Some(_)) => "modified",
                (None, Some(_)) => "added",
                (Some(_), None) => "deleted",
                (None, None) => "unchanged",
            }
        };
        let original = if old.is_some() {
            let bytes = read_local(&target, p)?;
            if old != Some(&hash(&bytes)) {
                return Err("Local files changed while building the preview; preview again".into());
            }
            Some(preview_text(&bytes))
        } else {
            None
        };
        rows.push(json!({"path":p,"change":change,"local":original,"remote":data.get(p).map(|b|preview_text(b))}));
    }
    if updating && expected.keys().any(|p| !old_runtime.contains(p)) {
        warnings.push("Untracked local files are preserved and will not be removed.".into());
    }
    let changed = rows.iter().filter(|r| r["change"] != "unchanged").count();
    let summary = if changed == 0 {
        "Runtime content is unchanged; confirm the verified baseline and documentation version"
            .into()
    } else {
        format!(
            "{} {} runtime file changes",
            if updating { "Update:" } else { "Install:" },
            changed
        )
    };
    let id = m.id();
    let kind = if updating { "update" } else { "install" };
    let public = json!({"id":id,"kind":kind,"skillId":skill["id"],"summary":summary,"files":rows,"warnings":warnings,"canApply":can_apply,"targetPath":target.to_string_lossy(),"commit":snap.commit});
    let encoded: BTreeMap<String, String> = data
        .iter()
        .map(|(p, b)| (p.clone(), STANDARD.encode(b)))
        .collect();
    m.previews.clear();
    m.previews.insert(id,json!({"public":public,"skillStamp":skill_stamp(&skill),"repo":snap.api.repo,"branch":snap.branch,"commit":snap.commit,
        "targetPath":target.to_string_lossy(),"expected":expected,"writes":encoded,"deletes":deletes,"runtimeFiles":selected,"runtimeSources":entries.iter().filter(|(p,_)|new_hashes.contains_key(*p)).map(|(p,e)|(p.clone(),e.source.clone())).collect::<BTreeMap<_,_>>(),
        "newHashes":new_hashes,"docHash":doc_hash(&snap,root_path,&selected,item.as_ref())?,"private":snap.private,"version":item.as_ref().and_then(|i|i.get("version_in_source")).cloned().unwrap_or(Value::Null)}));
    Ok(public)
}
fn read_local(root: &Path, path: &str) -> Result<Vec<u8>, String> {
    let p = local::safe_join(root, path)?;
    let meta = fs::metadata(&p).map_err(|_| "Cannot inspect local file")?;
    if meta.len() > MAX_FILE as u64 {
        return Err(format!("Local file exceeds the 4 MiB limit: {path}"));
    }
    fs::read(p).map_err(|_| format!("Cannot read local file: {path}"))
}
fn consume_preview(m: &mut Manager, req: &Value, kind: &str) -> Result<Value, String> {
    let id = text(req, "previewId")?;
    let p = m
        .previews
        .remove(id)
        .ok_or("Preview expired or was already used; create a fresh preview")?;
    if p["public"]["kind"] != kind || p["public"]["canApply"] != true {
        return Err("This preview cannot be applied".into());
    }
    Ok(p)
}
fn apply_install(m: &mut Manager, req: &Value, updating: bool) -> Result<Value, String> {
    let p = consume_preview(m, req, if updating { "update" } else { "install" })?;
    let mut skill = m.skill(text(&p["public"], "skillId")?)?;
    writable(&skill)?;
    if p["skillStamp"] != skill_stamp(&skill) {
        return Err("Skill configuration changed after preview; preview again".into());
    }
    let snap = snapshot(m, &skill)?;
    if snap.commit != text(&p, "commit")? {
        return Err("The remote branch changed after preview; refresh and preview again".into());
    }
    let target = checked_preview_target(text(&p, "targetPath")?)?;
    let expected = mapping(&p["expected"]);
    if target.exists() {
        if local::files(&target)? != expected {
            return Err("Local files changed after preview; preview again".into());
        }
        if !updating
            && fs::read_dir(&target)
                .map_err(|_| "Cannot inspect installation target")?
                .next()
                .is_some()
        {
            return Err("The installation directory is no longer empty".into());
        }
    } else if !expected.is_empty() {
        return Err("The local installation disappeared after preview".into());
    }
    let mut writes = BTreeMap::new();
    let mut total = 0usize;
    for (path, encoded) in mapping(&p["writes"]) {
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| "Invalid internal preview")?;
        total += bytes.len();
        if bytes.len() > MAX_FILE || total > MAX_TOTAL {
            return Err("Preview package exceeds size limits".into());
        }
        writes.insert(path, bytes);
    }
    let deletes = paths(&p["deletes"]);
    // Documentation-only changes update the verified receipt without touching runtime bytes.
    writes.retain(|path, bytes| expected.get(path) != Some(&hash(bytes)));
    if !writes.is_empty() || !deletes.is_empty() {
        transaction::apply_in(
            &m.storage.path("recovery")?,
            &target,
            &expected,
            &writes,
            &deletes,
        )?;
    }
    skill["localPath"] = json!(target.to_string_lossy());
    skill["runtimeFiles"] = p["runtimeFiles"].clone();
    skill["runtimeConfirmed"] = json!(true);
    skill["runtimeSources"] = p["runtimeSources"].clone();
    skill["baselineHashes"] = p["newHashes"].clone();
    skill["baselineCommit"] = p["commit"].clone();
    skill["baselineKind"] = json!("installation");
    skill["baselineVerifiedAt"] = json!(now());
    skill["installedCommit"] = p["commit"].clone();
    skill["baselineDocHash"] = p["docHash"].clone();
    skill["commit"] = p["commit"].clone();
    skill["remoteHashes"] = p["newHashes"].clone();
    skill["localHashes"] = p["newHashes"].clone();
    skill["remoteDocHash"] = p["docHash"].clone();
    skill["remoteCheckRequired"] = json!(false);
    skill["status"] = json!("synced");
    skill["private"] = p["private"].clone();
    skill["lastChecked"] = json!(now());
    skill["lastLocalCheckedAt"] = skill["lastChecked"].clone();
    skill["lastRemoteCheckedAt"] = skill["lastChecked"].clone();
    skill["error"] = Value::Null;
    association::reset_observation(&mut skill, &paths(&p["runtimeFiles"]));
    let mut installed = expected;
    for deleted in &deletes {
        installed.remove(deleted);
    }
    installed.extend(mapping(&p["newHashes"]));
    note_untracked(&mut skill, &installed, &paths(&p["runtimeFiles"]));
    if let Ok(draft) = local::draft(&target) {
        for field in ["name", "description", "version"] {
            if !draft[field].is_null() {
                skill[field] = draft[field].clone();
            }
        }
    }
    if !p["version"].is_null() {
        skill["version"] = p["version"].clone();
    }
    update_source(m, &snap, &skill["sourceId"]);
    m.upsert_skill(skill);
    m.save()?;
    Ok(m.snapshot())
}

fn preview_push(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let skill = m.skill(text(req, "skillId")?)?;
    writable(&skill)?;
    let root = local_root(&skill)?;
    let selected = checked_selection(&strings(&req["files"])?)?;
    let snap = snapshot(m, &skill)?;
    let entries = snap.skill_files(remote_path(&skill)?)?;
    if !snap.api.has_token() {
        return Err(
            "Save a repository token with Contents write permission before previewing a push"
                .into(),
        );
    }
    // A tag or immutable commit is readable, but it must never be silently converted into a writable branch.
    if snap.api.head(&snap.branch)? != snap.commit {
        return Err("Select a writable branch before submitting changes".into());
    }
    let expected = local::files(&root)?;
    let remote_selected: Vec<_> = selected
        .iter()
        .filter(|p| entries.contains_key(*p))
        .cloned()
        .collect();
    let remote_data = download(&snap, &entries, &remote_selected)?;
    let remote_hashes = hashes(&remote_data);
    let baseline = mapping(&skill["baselineHashes"]);
    let changes = selected_changes(&selected, &expected, &remote_hashes, &baseline)?;
    let mut data = BTreeMap::new();
    let mut total = 0usize;
    let mut rows = Vec::new();
    let mut writes = BTreeMap::new();
    let mut deletes = Vec::new();
    for (path, change) in &changes {
        let destination = entries
            .get(path)
            .map(|e| e.source.clone())
            .unwrap_or_else(|| join(remote_path(&skill).unwrap_or(""), path));
        github::validate_relative(&destination)?;
        if *change != "unchanged" && destination != join(remote_path(&skill)?, path) {
            return Err(format!("{path} comes from a repository-level license. Change that shared source directly rather than through a Skill upload."));
        }
        let original = if expected.contains_key(path) {
            let b = read_local(&root, path)?;
            if expected.get(path) != Some(&hash(&b)) {
                return Err("Local files changed while building the preview; preview again".into());
            }
            total += b.len();
            if total > MAX_TOTAL {
                return Err("Selected upload exceeds the 32 MiB limit".into());
            }
            data.insert(path.clone(), b.clone());
            Some(preview_text(&b))
        } else {
            None
        };
        rows.push(json!({"path":path,"repositoryPath":destination,"change":change,"local":original,"remote":remote_data.get(path).map(|b|preview_text(b))}));
        if *change == "deleted" {
            deletes.push(path.clone());
        } else if *change != "unchanged" {
            writes.insert(
                path.clone(),
                STANDARD.encode(data.get(path).ok_or("Missing selected local content")?),
            );
        }
    }
    let mut warnings:Vec<String>=vec!["Only the selected files are changed. Omitted remote README and management files remain untouched.".into()];
    if req["trimOptional"] != false {
        if let Err(error) = local::minimal_runtime_for_skill(&skill, &root, &expected) {
            warnings.push(format!(
                "本次可上传所选变化；运行依赖尚待核对，上传后保留全部本地文件：{error}"
            ));
        }
    }
    if skill["baselineCommit"].as_str().is_none() {
        warnings.push("The original installed version is unknown. Review the selected differences carefully; this does not establish a complete baseline by itself.".into());
    }
    let changed = changes.iter().filter(|(_, c)| *c != "unchanged").count();
    let id = m.id();
    let cleanup_files = cleanup::planned(&skill, &selected, req["trimOptional"] != false)?;
    let summary = if changed == 0 && !cleanup_files.is_empty() {
        "远端已一致，校验后精简本地；无需新增提交".to_owned()
    } else {
        format!("上传 {changed} 个所选变化；未选远端文件保留")
    };
    let public = json!({"id":id,"kind":"push","skillId":skill["id"],"summary":summary,"files":rows,"warnings":warnings,"canApply":changed>0 || !cleanup_files.is_empty(),"commit":snap.commit,"cleanupFiles":cleanup_files});
    m.previews.clear();
    m.previews.insert(id,json!({"public":public,"skillStamp":skill_stamp(&skill),"repo":snap.api.repo,"branch":snap.branch,"commit":snap.commit,"treeSha":snap.tree_sha,
        "root":root.to_string_lossy(),"expected":expected,"writes":writes,"deletes":deletes,"selected":selected,"remoteHashes":remote_hashes,
        "cleanupFiles":cleanup_files,"modes":entries.iter().map(|(p,e)|(p.clone(),e.mode.clone())).collect::<BTreeMap<_,_>>()}));
    Ok(public)
}

fn apply_push(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let message = text(req, "message")?.trim();
    if message.is_empty() || message.len() > 4000 || message.contains('\0') {
        return Err("Enter a commit message of 1–4000 characters".into());
    }
    let p = consume_preview(m, req, "push")?;
    let mut skill = m.skill(text(&p["public"], "skillId")?)?;
    writable(&skill)?;
    if p["skillStamp"] != skill_stamp(&skill) {
        return Err("Skill configuration changed after preview; preview again".into());
    }
    let root = local_root(&skill)?;
    if root != Path::new(text(&p, "root")?) || local::files(&root)? != mapping(&p["expected"]) {
        return Err("Local files changed after preview; preview again".into());
    }
    let api = Api::new(repo(&skill)?)?;
    if !api.has_token() {
        return Err("A repository token is required to submit changes".into());
    }
    let branch = text(&p, "branch")?;
    let base = text(&p, "commit")?;
    if api.head(branch)? != base {
        return Err("The remote branch advanced after preview; refresh and preview again".into());
    }
    let modes = mapping(&p["modes"]);
    let writes = mapping(&p["writes"]);
    let deletes = paths(&p["deletes"]);
    let mut upload = BTreeMap::new();
    let mut removed = Vec::new();
    let mut local_changes = BTreeMap::new();
    let mut total = 0usize;
    for (path, encoded) in &writes {
        github::validate_relative(path)?;
        if logic::disallowed_upload(path) {
            return Err("A selected upload contains a prohibited file".into());
        }
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| "Invalid internal upload preview")?;
        total += bytes.len();
        if bytes.len() > MAX_FILE || total > MAX_TOTAL {
            return Err("Upload exceeds package size limits".into());
        }
        local_changes.insert(path.clone(), hash(&bytes));
        upload.insert(
            join(remote_path(&skill)?, path),
            (
                modes
                    .get(path)
                    .map(String::as_str)
                    .filter(|m| *m == "100755")
                    .unwrap_or("100644")
                    .to_owned(),
                bytes,
            ),
        );
    }
    for path in &deletes {
        github::validate_relative(path)?;
        removed.push((
            join(remote_path(&skill)?, path),
            modes
                .get(path)
                .map(String::as_str)
                .unwrap_or("100644")
                .to_owned(),
        ));
    }
    let unchanged = upload.is_empty() && removed.is_empty();
    let commit_id = git_write::submit_or_verify(
        &api,
        branch,
        base,
        text(&p, "treeSha")?,
        message,
        &upload,
        &removed,
    )?;
    // A selected partial upload does not certify omitted local files or invent an installed version.
    let runtime = paths(&skill["runtimeFiles"]);
    let mut baseline = mapping(&skill["baselineHashes"]);
    for (path, h) in &local_changes {
        if runtime.contains(path) {
            baseline.insert(path.clone(), h.clone());
        }
    }
    for path in &deletes {
        baseline.remove(path);
    }
    let old_baseline_known = skill["baselineCommit"].as_str().is_some();
    skill["baselineHashes"] = json!(baseline);
    skill["commit"] = json!(commit_id);
    skill["lastChecked"] = json!(now());
    skill["error"] = Value::Null;
    // Failure after the ref update is a verification failure, not a failed submission.
    let verification = (|| -> Result<(Snapshot, VerifiedSkill), String> {
        let snap = Snapshot::load_fresh(repo(&skill)?, Some(branch))?;
        if snap.commit != commit_id {
            return Err("The branch advanced again after this submission".into());
        }
        let verified = verify_skill(&skill, &snap)?;
        Ok((snap, verified))
    })();
    match verification {
        Ok((snap, verified)) => {
            record_push_verification(&mut skill, &commit_id, snap.private, verified);
            update_source(m, &snap, &skill["sourceId"]);
        }
        Err(_) => {
            skill["remoteCheckRequired"] = json!(true);
            skill["remoteHashes"] = Value::Null;
            skill["error"]=json!("The commit was submitted, but the follow-up read failed. Refresh to verify the current remote state.");
            skill["comparisonPending"] = json!(true);
            skill["comparisonNote"] =
                json!("更改已提交，后续远端读取未完成；下方差异为上次记录，等待重新核验");
            skill["status"] = json!(if old_baseline_known {
                "local_modified"
            } else {
                "unknown_baseline"
            });
        }
    }
    m.upsert_skill(skill.clone());
    m.save().map_err(|error| {
        format!("The commit was submitted, but the local management record could not be saved. Refresh to verify the remote state before retrying: {error}")
    })?;
    let cleanup = cleanup::after_upload(
        m,
        &skill,
        &commit_id,
        p["cleanupFiles"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]),
    );
    let mut result = m.snapshot();
    let final_verification_error = result["skills"]
        .as_array()
        .and_then(|skills| skills.iter().find(|current| current["id"] == skill["id"]))
        .map(|current| current["error"].clone())
        .unwrap_or_else(|| skill["error"].clone());
    result["transferResult"] = json!({"submitted":true,"unchanged":unchanged,"commit":commit_id,"cleanup":cleanup,"verificationError":final_verification_error});
    Ok(result)
}

pub fn file_request(m: &mut Manager, req: &Value) -> Result<Option<Value>, String> {
    if let Some(result) = upload::handle(m, req)? {
        return Ok(Some(result));
    }
    if let Some(result) = cleanup::handle(m, req)? {
        return Ok(Some(result));
    }
    if req["action"] == "get_skill_info" {
        return information::get(m, req).map(Some);
    }
    attachments::handle(m, req)
}

fn save_access_change(
    m: &Manager,
    save: impl FnOnce() -> Result<(), String>,
) -> Result<Value, String> {
    save()?;
    crate::network::invalidate_credentials();
    m.save().map_err(|error| {
        format!("访问凭据已变更，但状态通知未保存，请重新读取当前状态：{error}")
    })?;
    let mut result = m.snapshot();
    result["readGeneration"] = json!(crate::network::read_generation()?);
    result["saved"] = json!(true);
    Ok(result)
}

pub fn handle(m: &mut Manager, req: &Value) -> Result<Option<Value>, String> {
    let result = match req["action"].as_str().unwrap_or("") {
        "add_github" => add_github(m, req)?,
        "auto_associate" => auto_associate(m)?,
        "associate" => associate(m, req)?,
        "refresh" => refresh(m, req)?,
        "get_document" => get_document(m, req)?,
        "get_doc_asset" => get_doc_asset(m, req)?,
        "get_files" => {
            let skill = m.skill(text(req, "skillId")?)?;
            if skill["localPath"].as_str().is_some() {
                return Ok(None);
            }
            get_remote_files(m, &skill)?
        }
        "preview_install" => preview_install(m, req, false)?,
        "preview_update" => preview_install(m, req, true)?,
        "apply_install" => apply_install(m, req, false)?,
        "apply_update" => apply_install(m, req, true)?,
        "preview_push" => preview_push(m, req)?,
        "apply_push" => apply_push(m, req)?,
        "set_token" | "delete_token" => {
            let location = github::parse_location(text(req, "repo")?)?;
            save_access_change(m, || {
                github::save_credential(
                    &location.repo,
                    if req["action"] == "set_token" {
                        Some(text(req, "token")?)
                    } else {
                        None
                    },
                )
            })?
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn v025_access_change_broadcasts_new_read_context_without_persisting_credentials() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/v025-performance-rust");
        fs::create_dir_all(&parent).unwrap();
        let temp = tempfile::tempdir_in(parent).unwrap();
        let mut m = Manager::new(temp.path().join("state")).unwrap();
        let events = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let sink = events.clone();
        m.set_state_events(std::sync::Arc::new(move |revision| {
            sink.store(revision, std::sync::atomic::Ordering::Relaxed);
        }));
        let prior = serde_json::to_value(&m.state).unwrap();
        let before = crate::network::read_generation().unwrap();
        let store = std::cell::RefCell::new(None::<String>);
        let set = save_access_change(&m, || {
            *store.borrow_mut() = Some("synthetic-only-access-value".into());
            Ok(())
        })
        .unwrap();
        assert_eq!(set["saved"], true);
        assert!(set["readGeneration"].as_u64().unwrap() > before);
        assert!(events.load(std::sync::atomic::Ordering::Relaxed) > 0);
        let set_revision = set["stateRevision"].as_u64().unwrap();
        let deleted = save_access_change(&m, || {
            *store.borrow_mut() = None;
            Ok(())
        })
        .unwrap();
        assert!(store.borrow().is_none());
        assert!(deleted["stateRevision"].as_u64().unwrap() > set_revision);
        assert_eq!(serde_json::to_value(&m.state).unwrap(), prior);
        let db = rusqlite::Connection::open(temp.path().join("state/manager.sqlite3")).unwrap();
        let payload: String = db
            .query_row("SELECT payload FROM app_state WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(!payload.contains("synthetic-only-access-value"));
    }
    #[test]
    fn v025_failed_access_store_does_not_publish_a_success_snapshot() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/v025-performance-rust");
        fs::create_dir_all(&parent).unwrap();
        let temp = tempfile::tempdir_in(parent).unwrap();
        let m = Manager::new(temp.path().join("state")).unwrap();
        let prior = m.snapshot()["stateRevision"].clone();
        assert!(save_access_change(&m, || Err("synthetic store unavailable".into())).is_err());
        assert_eq!(m.snapshot()["stateRevision"], prior);
    }
    #[test]
    fn local_documents_work_offline_and_cannot_escape_the_source() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/document-tests");
        fs::create_dir_all(&parent).unwrap();
        let temp = tempfile::tempdir_in(parent).unwrap();
        let source = temp.path().join("skill");
        fs::create_dir(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: local-document\ndescription: Offline test\n---\n# Offline document\n",
        )
        .unwrap();
        let mut m = Manager::new(temp.path().join("data")).unwrap();
        let mut record = local::draft(&source).unwrap();
        record["id"] = json!("offline");
        m.upsert_skill(record);
        let doc = get_document(&m, &json!({"skillId":"offline","version":"latest"})).unwrap();
        assert!(doc["markdown"]
            .as_str()
            .unwrap()
            .contains("Offline document"));
        assert_eq!(doc["source"], "本地运行文件");
        assert!(doc["commit"].is_null());
        assert_eq!(doc["localFingerprint"].as_str().unwrap().len(), 64);
        assert!(doc["readGeneration"].is_u64());
        fs::write(source.join("SKILL.md"), "# Offline document changed\n").unwrap();
        let changed = get_document(&m, &json!({"skillId":"offline","version":"latest"})).unwrap();
        assert_ne!(doc["localFingerprint"], changed["localFingerprint"]);
        assert!(changed["markdown"].as_str().unwrap().contains("changed"));
        assert!(get_document(&m, &json!({"skillId":"offline","path":"../outside.md"})).is_err());
    }
    #[test]
    fn existing_personal_catalog_is_understood_without_changing_it() {
        let catalog = json!({"skills":[{"id":"paper-search","source":"paper-search/SKILL.md","usage":"paper-search/README.md","version_in_source":"0.1.0"}]});
        let items = catalog_items(Some(&catalog));
        let item = catalog_item(&items, "paper-search").unwrap();
        assert_eq!(item["usage"], "paper-search/README.md");
        assert!(declared_runtime(Some(item)).is_none());
        let enhanced = json!({"source":"s/SKILL.md","runtime_files":["SKILL.md","scripts/run.py"]});
        assert_eq!(declared_runtime(Some(&enhanced)).unwrap().len(), 2);
    }
    #[test]
    fn preview_stamp_changes_with_association_and_baseline_not_refresh_time() {
        let mut skill = json!({"localPath":"D:/skills/a","baselineHashes":{"SKILL.md":"a"}});
        let first = skill_stamp(&skill);
        skill["lastChecked"] = json!(now());
        assert_eq!(first, skill_stamp(&skill));
        skill["baselineHashes"]["SKILL.md"] = json!("b");
        assert_ne!(first, skill_stamp(&skill));
    }
    #[test]
    fn required_dependencies_cannot_be_silently_omitted() {
        let entries: BTreeMap<_, _> = [
            "SKILL.md",
            "agents/openai.yaml",
            "references/rules.md",
            "LICENSE",
        ]
        .iter()
        .map(|p| {
            (
                p.to_string(),
                Entry {
                    sha: "a".repeat(40),
                    mode: "100644".into(),
                    size: 5,
                    source: p.to_string(),
                },
            )
        })
        .collect();
        let data = BTreeMap::from([
            ("SKILL.md".into(), b"Read `references/rules.md`".to_vec()),
            ("agents/openai.yaml".into(), b"interface: {}".to_vec()),
            (
                "references/rules.md".into(),
                b"Use the package rules.".to_vec(),
            ),
            ("LICENSE".into(), b"Synthetic test license".to_vec()),
        ]);
        assert!(validate_runtime(&entries, &["SKILL.md".into()], &data).is_err());
        let selection = entries.keys().cloned().collect::<Vec<_>>();
        assert!(validate_runtime(&entries, &selection, &data).is_ok());
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;

    fn tree(files: &[&str]) -> BTreeMap<String, Entry> {
        files
            .iter()
            .map(|path| {
                (
                    path.to_string(),
                    Entry {
                        sha: "a".repeat(40),
                        mode: "100644".into(),
                        size: 10,
                        source: path.to_string(),
                    },
                )
            })
            .collect()
    }
    fn source(roots: &[&str]) -> Value {
        json!({"id":"source","repo":"owner/repo","branch":"main","commit":"b".repeat(40),
            "private":true,"roots":roots,"ignoredPaths":[],"lastChecked":"checked"})
    }
    fn installed() -> Value {
        json!({"id":"installed","sourceId":"source","remotePath":"skills/existing","name":"my-local-name",
            "displayName":"My own display name","description":"Local custom description","version":"custom",
            "localPath":"D:/synthetic/skill","baselineCommit":"a".repeat(40),"baselineHashes":{"SKILL.md":"original"},
            "runtimeFiles":["SKILL.md"],"runtimeConfirmed":true,"status":"synced","categoryId":"my-category"})
    }

    #[test]
    fn scopes_include_the_directory_and_its_children_without_prefix_siblings() {
        let snapshot = tree(&[
            "SKILL.md",
            "skills/SKILL.md",
            "skills/one/SKILL.md",
            "skills-extra/two/SKILL.md",
            "other/SKILL.md",
        ]);
        let found = discover_paths(&snapshot, &["skills".into()], &[]).unwrap();
        assert_eq!(
            found,
            BTreeSet::from(["skills".into(), "skills/one".into()])
        );
        assert_eq!(
            discover_paths(&snapshot, &[String::new()], &[])
                .unwrap()
                .len(),
            5
        );
        assert!(discover_paths(&snapshot, &[], &[]).unwrap().is_empty());
    }

    #[test]
    fn refresh_discovers_new_catalog_skills_without_replacing_installed_metadata() {
        let original = installed();
        let mut skills = vec![original.clone()];
        let snapshot = tree(&[
            "skills/existing/SKILL.md",
            "skills/new/SKILL.md",
            "outside/SKILL.md",
        ]);
        let items = vec![
            json!({"source":"skills/existing/SKILL.md","id":"upstream-new-name","version_in_source":"99"}),
            json!({"source":"skills/new/SKILL.md","id":"new","display_name":"New Skill","purpose_zh":"新技能","runtime_files":["SKILL.md"]}),
        ];
        reconcile_discovery(&mut skills, &source(&["skills"]), &snapshot, &items).unwrap();
        assert_eq!(skills.len(), 2);
        let old = skills.iter().find(|s| s["id"] == "installed").unwrap();
        for field in [
            "id",
            "name",
            "displayName",
            "description",
            "version",
            "localPath",
            "baselineCommit",
            "baselineHashes",
            "runtimeFiles",
            "status",
            "categoryId",
        ] {
            assert_eq!(old[field], original[field], "{field}");
        }
        let new = skills
            .iter()
            .find(|s| s["remotePath"] == "skills/new")
            .unwrap();
        assert_eq!(new["displayName"], "New Skill");
        assert_eq!(new["status"], "remote_only");
        assert!(new["localPath"].is_null());
        assert!(new["baselineCommit"].is_null());
        assert!(new["categoryId"].is_null());
        reconcile_discovery(&mut skills, &source(&["skills"]), &snapshot, &items).unwrap();
        assert_eq!(
            skills.len(),
            2,
            "Repeated discovery must not duplicate records"
        );
    }

    #[test]
    fn missing_remote_skill_keeps_the_local_record_and_recovers_when_restored() {
        let original = installed();
        let mut skills = vec![original.clone()];
        reconcile_discovery(&mut skills, &source(&["skills"]), &tree(&[]), &[]).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["remoteMissing"], true);
        assert!(skills[0]["error"].as_str().unwrap().contains("preserved"));
        assert_eq!(skills[0]["baselineHashes"], original["baselineHashes"]);
        assert_eq!(skills[0]["localPath"], original["localPath"]);
        assert_eq!(skills[0]["categoryId"], original["categoryId"]);
        reconcile_discovery(
            &mut skills,
            &source(&["skills"]),
            &tree(&["skills/existing/SKILL.md"]),
            &[],
        )
        .unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["remoteMissing"], false);
        assert!(skills[0]["error"].is_null());
        assert_eq!(skills[0]["baselineCommit"], original["baselineCommit"]);
    }

    #[test]
    fn forgotten_paths_stay_hidden_until_their_scope_is_explicitly_added() {
        let mut source = source(&["skills"]);
        source["ignoredPaths"] = json!(["skills/hidden", "other/hidden"]);
        let snapshot = tree(&[
            "skills/hidden/SKILL.md",
            "skills/new/SKILL.md",
            "other/hidden/SKILL.md",
        ]);
        let mut skills = vec![];
        reconcile_discovery(&mut skills, &source, &snapshot, &[]).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["remotePath"], "skills/new");
        track_scope(&mut source, &skills, "skills", true).unwrap();
        assert_eq!(source["ignoredPaths"], json!(["other/hidden"]));
        reconcile_discovery(&mut skills, &source, &snapshot, &[]).unwrap();
        assert_eq!(skills.len(), 2);
    }

    #[test]
    fn adding_scopes_merges_them_and_association_unignores_only_its_target() {
        let mut source = source(&["one"]);
        source["ignoredPaths"] = json!(["two", "two/child", "outside"]);
        track_scope(&mut source, &[], "two", false).unwrap();
        assert_eq!(source["roots"], json!(["one", "two"]));
        assert_eq!(source["ignoredPaths"], json!(["outside", "two/child"]));
        track_scope(&mut source, &[], "two", false).unwrap();
        assert_eq!(source["roots"], json!(["one", "two"]));
        track_scope(&mut source, &[], "", true).unwrap();
        assert_eq!(source["ignoredPaths"], json!([]));
    }

    #[test]
    fn legacy_scope_inference_never_expands_to_unknown_repository_directories() {
        let legacy = json!({"id":"source"});
        let roots = source_roots(&legacy, &[installed()]).unwrap();
        assert_eq!(roots, vec!["skills/existing".to_string()]);
        assert_eq!(
            discover_paths(
                &tree(&["skills/existing/SKILL.md", "skills/new/SKILL.md"]),
                &roots,
                &[]
            )
            .unwrap(),
            BTreeSet::from(["skills/existing".into()])
        );
        assert!(source_roots(&legacy, &[]).unwrap().is_empty());
        assert!(checked_directories(&json!(["../outside"])).is_err());
    }
}

#[cfg(test)]
mod association_behavior_tests {
    use super::*;

    fn stale_development_scope() -> (tempfile::TempDir, PathBuf, PathBuf, Value, Vec<String>) {
        let temp = fixture();
        let left = package(&temp.path().join("local"), "shared-skill", "Instructions.");
        let right = package(&temp.path().join("remote"), "shared-skill", "Instructions.");
        for root in [&left, &right] {
            fs::create_dir_all(root.join("scripts")).unwrap();
            for index in 0..20 {
                fs::write(
                    root.join(format!("scripts/resource-{index}.txt")),
                    "Runtime resource.\n",
                )
                .unwrap();
            }
        }
        let manifest: Vec<_> = local::files(&left).unwrap().into_keys().collect();
        assert_eq!(manifest.len(), 23);
        fs::create_dir_all(right.join("scripts/tests")).unwrap();
        fs::create_dir_all(right.join("scripts/__pycache__")).unwrap();
        for index in 0..4 {
            fs::write(
                right.join(format!("scripts/tests/test_{index}.py")),
                "# Development only\n",
            )
            .unwrap();
        }
        fs::write(
            right.join("scripts/__pycache__/test.cpython-313.pyc"),
            b"cache",
        )
        .unwrap();
        let mut record = local_record(&left, false);
        record["sourceId"] = json!("source");
        record["repo"] = json!("fixture/repo");
        record["remotePath"] = json!("shared-skill");
        record["runtimeFiles"] = json!(local::files(&right)
            .unwrap()
            .into_keys()
            .collect::<Vec<_>>());
        record["runtimeAudit"] =
            json!({"method":"static_package","status":"needs_review","canAutoConfirm":false});
        record["comparisonScope"] = record["runtimeFiles"].clone();
        record["status"] = json!("unknown_baseline");
        assert_eq!(paths(&record["runtimeFiles"]).len(), 28);
        (temp, left, right, record, manifest)
    }

    #[test]
    fn proven_legacy_development_scope_rebuilds_without_changing_files_or_installation_history() {
        let (_temp, left, right, record, manifest) = stale_development_scope();
        let before = local::files(&left).unwrap();
        let original = record.clone();
        let item = json!({"runtime_files":manifest});
        let (mut repaired, proof) =
            rebuilt_automatic_scope(&record, Some(&manifest), Some(&item), |candidate| {
                Ok(verified(candidate, &right))
            })
            .unwrap()
            .unwrap();
        record_verification(&mut repaired, "verified-current", true, proof);
        assert_eq!(paths(&repaired["runtimeFiles"]), manifest);
        assert_eq!(paths(&repaired["comparisonScope"]).len(), 23);
        assert_eq!(repaired["runtimeConfirmed"], true);
        assert_eq!(repaired["runtimeConfirmationKind"], "automatic");
        assert_eq!(repaired["runtimeAudit"]["status"], "passed");
        assert_eq!(repaired["status"], "synced");
        assert!(!skill_needs_attention(&repaired));
        assert_eq!(repaired["baselineKind"], "comparison");
        assert!(repaired["installedCommit"].is_null());
        assert_eq!(record, original);
        assert_eq!(local::files(&left).unwrap(), before);
    }

    #[test]
    fn manual_legacy_confirmed_and_non_development_missing_selections_are_never_shrunk() {
        let (_temp, _left, _right, record, manifest) = stale_development_scope();
        for (confirmed, kind) in [
            (true, Value::Null),
            (true, json!("manual")),
            (false, json!("manual")),
        ] {
            let mut manual = record.clone();
            manual["runtimeConfirmed"] = json!(confirmed);
            manual["runtimeConfirmationKind"] = kind;
            assert!(
                rebuilt_automatic_scope(&manual, Some(&manifest), None, |_| panic!(
                    "manual scope must not be verified as a replacement"
                ))
                .unwrap()
                .is_none()
            );
        }
        let mut unknown = record.clone();
        unknown["runtimeFiles"]
            .as_array_mut()
            .unwrap()
            .push(json!("assets/missing-template.txt"));
        assert!(
            rebuilt_automatic_scope(&unknown, Some(&manifest), None, |_| panic!(
                "real missing resources must remain unresolved"
            ))
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn invalid_catalog_real_dependency_network_failure_and_stale_bytes_cannot_commit_scope_repair()
    {
        let (_temp, left, right, record, manifest) = stale_development_scope();
        let original = record.clone();
        let mut incomplete = manifest.clone();
        incomplete.push("scripts/missing-runtime.py".into());
        assert!(
            rebuilt_automatic_scope(&record, Some(&incomplete), None, |_| panic!(
                "missing catalog entry must not be accepted"
            ))
            .unwrap()
            .is_none()
        );
        assert!(
            rebuilt_automatic_scope(&record, Some(&["../outside".into()]), None, |_| panic!(
                "invalid catalog must fail"
            ))
            .is_err()
        );
        assert!(
            rebuilt_automatic_scope(&record, Some(&manifest), None, |_| Err(
                "synthetic network failure".into()
            ))
            .is_err()
        );
        let stale = rebuilt_automatic_scope(&record, Some(&manifest), None, |candidate| {
            let proof = verified(candidate, &right);
            fs::write(
                left.join("scripts/resource-0.txt"),
                "Concurrent user edit.\n",
            )
            .unwrap();
            Ok(proof)
        })
        .unwrap();
        assert!(stale.is_none());
        assert_eq!(record, original);
        fs::write(
            left.join("SKILL.md"),
            "---\nname: shared-skill\n---\nRead `scripts/tests/test_0.py`.\n",
        )
        .unwrap();
        assert!(
            rebuilt_automatic_scope(&record, Some(&manifest), None, |_| panic!(
                "a required test is a real dependency, regardless of its folder"
            ))
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn docs_only_results_are_normal_and_summary_counts_only_this_check() {
        let old = json!({"id":"one","sourceId":"source","repo":"fixture/repo","remotePath":"one","status":"synced","remoteDocHash":"old","error":null});
        let mut updated = old.clone();
        updated["status"] = json!("docs_updated");
        updated["remoteDocHash"] = json!("new");
        assert!(!skill_needs_attention(&updated));
        assert_eq!(
            documentation_updates(std::slice::from_ref(&old), std::slice::from_ref(&updated)),
            1
        );
        assert_eq!(
            documentation_updates(
                std::slice::from_ref(&updated),
                std::slice::from_ref(&updated)
            ),
            0
        );
        assert_eq!(
            documentation_updates(&[], std::slice::from_ref(&updated)),
            0
        );
        for status in [
            "local_modified",
            "remote_updated",
            "both_modified",
            "unknown_baseline",
        ] {
            let mut pending = updated.clone();
            pending["status"] = json!(status);
            assert!(skill_needs_attention(&pending));
        }
        updated["runtimeAudit"] = json!({"status":"needs_review"});
        assert!(skill_needs_attention(&updated));
        updated["error"] = json!("offline");
        assert_eq!(documentation_updates(&[old], &[updated]), 0);
    }

    fn fixture() -> tempfile::TempDir {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/association-tests");
        fs::create_dir_all(&parent).unwrap();
        tempfile::tempdir_in(parent).unwrap()
    }
    fn package(parent: &Path, folder: &str, body: &str) -> PathBuf {
        let root = parent.join(folder);
        fs::create_dir_all(root.join("agents")).unwrap();
        write_entry(&root, body);
        fs::write(
            root.join("agents/openai.yaml"),
            "interface:\n  display_name: Test Skill\n",
        )
        .unwrap();
        fs::write(root.join("LICENSE"), "Synthetic test permission notice").unwrap();
        local::canonical(&root).unwrap()
    }
    fn write_entry(root: &Path, body: &str) {
        fs::write(
            root.join("SKILL.md"),
            format!("---\nname: shared-skill\ndescription: Test fixture\n---\n{body}\n"),
        )
        .unwrap();
    }
    fn local_record(root: &Path, confirmed: bool) -> Value {
        let mut skill = local::draft(root).unwrap();
        skill["id"] = json!("retained-local");
        skill["displayName"] = json!("My display");
        skill["version"] = json!("my-version");
        skill["categoryId"] = json!("my-category");
        skill["runtimeFiles"] = json!(["LICENSE", "SKILL.md", "agents/openai.yaml"]);
        skill["runtimeConfirmed"] = json!(confirmed);
        skill
    }
    fn remote_record() -> Value {
        json!({"id":"remote-placeholder","sourceId":"source","remotePath":"shared-skill",
            "repo":"fixture/repo","branch":"main","name":"shared-skill","commit":"first",
            "catalogMetadata":{"display_name":"Upstream name"},"status":"remote_only"})
    }
    fn identity(id: &str, folder: &str) -> association::Identity {
        association::Identity {
            id: id.into(),
            name: "shared-skill".into(),
            folder: folder.into(),
            eligible: true,
            verified: true,
        }
    }
    // Files are read and hashed from two independent fixture directories. No API or real source is used.
    fn verified(record: &Value, remote: &Path) -> VerifiedSkill {
        let all = current_hashes(record).unwrap();
        let scope = paths(&record["runtimeFiles"]);
        let left = subset(&all, &scope);
        let right = subset(&local::files(remote).unwrap(), &scope);
        let required = local::validate_runtime_files(remote, &scope).unwrap();
        let can_adopt = record["runtimeConfirmed"] == true
            && left == right
            && scope
                .iter()
                .all(|p| all.contains_key(p) && right.contains_key(p))
            && local::validate_runtime_files(&local_root(record).unwrap(), &scope).is_ok()
            && required.len() == scope.len();
        VerifiedSkill {
            scope,
            all,
            local: left,
            remote: right,
            docs: "same-docs".into(),
            item: None,
            internal_name: association::internal_name(&fs::read(remote.join("SKILL.md")).unwrap())
                .unwrap(),
            can_adopt,
        }
    }
    fn linked(local: &Value, remote: &Path) -> Value {
        finish_auto_pair(
            local,
            &remote_record(),
            &identity("retained-local", "shared-skill"),
            &identity("remote-placeholder", "shared-skill"),
            verified(local, remote),
            "first",
            true,
        )
        .unwrap()
    }

    fn audited_linked_fixture(parent: &Path, manual: bool) -> (Manager, PathBuf) {
        let left = package(&parent.join("local"), "shared-skill", "Original rules.");
        let right = package(&parent.join("remote"), "shared-skill", "Original rules.");
        let mut record = linked(&local_record(&left, true), &right);
        record["repo"] = json!("synthetic-protocol/runtime-refresh-evidence");
        record["installedCommit"] = json!("historical-installation");
        record["lastRemoteCheckedAt"] = json!("2001-01-01T00:00:00Z");
        record["runtimeConfirmed"] = json!(manual);
        if manual {
            fs::write(left.join("README.md"), "User's saved optional choice.").unwrap();
            record["runtimeFiles"] =
                json!(["LICENSE", "README.md", "SKILL.md", "agents/openai.yaml"]);
            record["runtimeConfirmationKind"] = json!("manual");
        }
        let mut m = Manager::new(parent.join("state")).unwrap();
        m.state.sources =
            vec![json!({"id":"source","repo":record["repo"],"branch":"main","error":null})];
        m.upsert_skill(record);
        crate::dispatch(
            &mut m,
            &json!({"action":"audit_skills","skillId":"retained-local","localOnly":true}),
        )
        .unwrap();
        assert_eq!(m.state.skills[0]["runtimeAudit"]["status"], "passed");
        assert_eq!(m.state.skills[0]["runtimeConfirmed"], true);
        assert_eq!(m.state.skills[0]["sourceId"], "source");
        assert_eq!(m.state.skills[0]["baselineCommit"], "first");
        assert_eq!(
            m.state.skills[0]["runtimeConfirmationKind"],
            if manual { "manual" } else { "automatic" }
        );
        (m, left)
    }

    fn assert_history_preserved(skill: &Value, before: &Value) {
        for field in [
            "baselineCommit",
            "baselineHashes",
            "baselineKind",
            "baselineVerifiedAt",
            "installedCommit",
            "lastRemoteCheckedAt",
            "runtimeFiles",
        ] {
            assert_eq!(skill[field], before[field], "{field}");
        }
        assert_eq!(skill["runtimeAudit"]["status"], "needs_review");
        assert_eq!(skill["runtimeAudit"]["canAutoConfirm"], false);
        assert!(skill["runtimeAudit"]["invalidatedAt"].as_str().is_some());
    }

    #[test]
    fn linked_local_refresh_invalidates_automatic_evidence_for_new_missing_dependency() {
        let d = fixture();
        let (mut m, root) = audited_linked_fixture(d.path(), false);
        let before = m.state.skills[0].clone();
        write_entry(&root, "Run `scripts/missing.py`.");
        crate::dispatch(
            &mut m,
            &json!({"action":"refresh","skillId":"retained-local","localOnly":true}),
        )
        .unwrap();
        assert_history_preserved(&m.state.skills[0], &before);
        assert_eq!(m.state.skills[0]["runtimeConfirmed"], false);
        assert_eq!(m.state.skills[0]["status"], "local_modified");
        assert!(m.state.skills[0]["changeDetails"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["path"] == "SKILL.md"));
        let reopened = Manager::new(d.path().join("state")).unwrap();
        assert_eq!(
            reopened.state.skills[0]["runtimeAudit"]["status"],
            "needs_review"
        );
        crate::dispatch(
            &mut m,
            &json!({"action":"audit_skills","skillId":"retained-local","localOnly":true}),
        )
        .unwrap();
        assert_eq!(m.state.skills[0]["runtimeAudit"]["status"], "needs_review");
        assert_eq!(m.state.skills[0]["runtimeConfirmed"], false);
        assert!(m.state.skills[0]["runtimeIssues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["target"] == "scripts/missing.py"));
    }

    #[test]
    fn linked_failed_local_hash_invalidates_evidence_in_local_and_offline_full_refresh() {
        let _transport = github::test_transport(
            "synthetic-protocol/runtime-refresh-evidence",
            "http://127.0.0.1:1/",
        );
        for local_only in [true, false] {
            let d = fixture();
            let (mut m, root) = audited_linked_fixture(d.path(), false);
            let before = m.state.skills[0].clone();
            fs::rename(&root, root.with_file_name("temporarily-unavailable")).unwrap();
            crate::dispatch(
                &mut m,
                &json!({"action":"refresh","skillId":"retained-local","localOnly":local_only}),
            )
            .unwrap();
            assert_history_preserved(&m.state.skills[0], &before);
            assert_eq!(m.state.skills[0]["runtimeConfirmed"], false);
            assert_eq!(
                m.state.skills[0]["lastLocalCheckedAt"],
                before["lastLocalCheckedAt"]
            );
            assert!(m.state.skills[0]["error"].as_str().is_some());
        }
    }

    #[test]
    fn linked_offline_full_refresh_invalidates_evidence_before_remote_failure() {
        let _transport = github::test_transport(
            "synthetic-protocol/runtime-refresh-evidence",
            "http://127.0.0.1:1/",
        );
        let d = fixture();
        let (mut m, root) = audited_linked_fixture(d.path(), false);
        let before = m.state.skills[0].clone();
        write_entry(&root, "Run `scripts/missing.py`.");
        crate::dispatch(
            &mut m,
            &json!({"action":"refresh","skillId":"retained-local"}),
        )
        .unwrap();
        assert_history_preserved(&m.state.skills[0], &before);
        assert_eq!(m.state.skills[0]["runtimeConfirmed"], false);
        assert_eq!(m.state.skills[0]["remoteCheckRequired"], true);
        assert!(m.state.skills[0]["error"].as_str().is_some());
        assert!(m.state.sources[0]["error"].as_str().is_some());
    }

    #[test]
    fn linked_offline_refresh_invalidates_proof_but_preserves_manual_optional_scope() {
        let _transport = github::test_transport(
            "synthetic-protocol/runtime-refresh-evidence",
            "http://127.0.0.1:1/",
        );
        for unavailable in [false, true] {
            let d = fixture();
            let (mut m, root) = audited_linked_fixture(d.path(), true);
            let before = m.state.skills[0].clone();
            if unavailable {
                fs::rename(&root, root.with_file_name("temporarily-unavailable")).unwrap();
            } else {
                write_entry(&root, "Run `scripts/missing.py`.");
            }
            crate::dispatch(
                &mut m,
                &json!({"action":"refresh","skillId":"retained-local"}),
            )
            .unwrap();
            assert_history_preserved(&m.state.skills[0], &before);
            assert_eq!(m.state.skills[0]["runtimeConfirmed"], true);
            assert_eq!(m.state.skills[0]["runtimeConfirmationKind"], "manual");
            assert!(paths(&m.state.skills[0]["runtimeFiles"]).contains(&"README.md".to_owned()));
            assert_eq!(m.state.skills[0]["remoteCheckRequired"], true);
        }
    }

    #[test]
    fn merge_keeps_real_local_files_connections_and_unknown_installation() {
        let d = fixture();
        let left = package(
            &d.path().join("local"),
            "shared-skill",
            "my local instructions",
        );
        let right = package(
            &d.path().join("remote"),
            "shared-skill",
            "remote instructions",
        );
        let local = local_record(&left, true);
        let before = local::files(&left).unwrap();
        let merged = linked(&local, &right);
        assert_eq!(merged["status"], "unknown_baseline");
        assert!(merged["baselineCommit"].is_null());
        assert_eq!(
            merged["comparisonLocalHashes"]["SKILL.md"],
            before["SKILL.md"]
        );
        assert_eq!(
            merged["comparisonRemoteHashes"]["SKILL.md"],
            local::files(&right).unwrap()["SKILL.md"]
        );
        let mut m = Manager::new(d.path().join("data")).unwrap();
        m.state.skills = vec![local.clone(), remote_record()];
        m.state.connections = vec![
            json!({"id":"client-connection","skillId":"retained-local","target":left.join("client-link")}),
        ];
        let connections = m.state.connections.clone();
        retain_local_merge(&mut m, merged, "remote-placeholder");
        assert_eq!(m.state.skills.len(), 1);
        assert_eq!(m.state.connections, connections);
        for field in [
            "id",
            "localPath",
            "displayName",
            "version",
            "runtimeFiles",
            "runtimeConfirmed",
            "categoryId",
        ] {
            assert_eq!(m.state.skills[0][field], local[field], "{field}");
        }
        assert_eq!(local::files(&left).unwrap(), before);
    }
    #[test]
    fn disk_edits_after_first_observation_report_local_remote_and_both() {
        let d = fixture();
        let left = package(&d.path().join("local"), "shared-skill", "local first");
        let right = package(&d.path().join("remote"), "shared-skill", "remote first");
        let initial = linked(&local_record(&left, true), &right);
        let mut m = Manager::new(d.path().join("data")).unwrap();
        m.state.sources = vec![json!({"id":"source","error":null})];
        m.upsert_skill(initial.clone());
        write_entry(&left, "local edited");
        refresh_local(&mut m, &json!({"skillId":"retained-local"})).unwrap();
        assert_eq!(m.state.skills[0]["status"], "local_modified");
        assert_eq!(m.state.skills[0]["changeDetails"][0]["side"], "local");
        assert_eq!(
            m.state.skills[0]["comparisonLocalHashes"],
            initial["comparisonLocalHashes"]
        );
        write_entry(&left, "local first");
        write_entry(&right, "remote edited");
        let mut remote_only = initial.clone();
        let observation = verified(&remote_only, &right);
        record_verification(&mut remote_only, "second", true, observation);
        assert_eq!(remote_only["status"], "remote_updated");
        assert_eq!(remote_only["changeDetails"][0]["side"], "remote");
        write_entry(&left, "local edited");
        let mut both = initial.clone();
        let observation = verified(&both, &right);
        record_verification(&mut both, "second", true, observation);
        assert_eq!(both["status"], "both_modified");
        assert_eq!(both["changeDetails"][0]["side"], "both");
        assert!(both["baselineCommit"].is_null());
        assert_eq!(
            both["comparisonRemoteHashes"],
            initial["comparisonRemoteHashes"]
        );
    }
    #[test]
    fn only_confirmed_complete_identical_runtime_adopts_a_commit_without_replacing_files() {
        let d = fixture();
        let left = package(&d.path().join("local"), "shared-skill", "same instructions");
        let right = package(
            &d.path().join("remote"),
            "shared-skill",
            "same instructions",
        );
        let before = local::files(&left).unwrap();
        let confirmed = linked(&local_record(&left, true), &right);
        assert_eq!(confirmed["baselineCommit"], "first");
        assert_eq!(confirmed["status"], "synced");
        assert_eq!(mapping(&confirmed["baselineHashes"]), before);
        let unconfirmed = linked(&local_record(&left, false), &right);
        assert!(unconfirmed["baselineCommit"].is_null());
        assert_eq!(unconfirmed["status"], "synced");
        assert!(unconfirmed["associationNote"]
            .as_str()
            .unwrap()
            .contains("未冒认安装版本"));
        assert_eq!(local::files(&left).unwrap(), before);
    }
    #[test]
    fn documentation_observation_and_installation_reset_have_separate_lifetimes() {
        let d = fixture();
        let left = package(&d.path().join("local"), "shared-skill", "same instructions");
        let right = package(
            &d.path().join("remote"),
            "shared-skill",
            "same instructions",
        );
        let mut record = linked(&local_record(&left, false), &right);
        assert_eq!(
            record["status"], "synced",
            "The first document read is not an update"
        );
        assert_eq!(record["comparisonDocHash"], "same-docs");
        let mut updated = verified(&record, &right);
        updated.docs = "new-docs".into();
        record_verification(&mut record, "second", true, updated);
        assert_eq!(record["status"], "docs_updated");
        assert_eq!(record["comparisonDocHash"], "same-docs");
        let mut m = Manager::new(d.path().join("data")).unwrap();
        m.state.sources = vec![json!({"id":"source","error":null})];
        m.upsert_skill(record.clone());
        refresh_local(&mut m, &json!({"skillId":"retained-local"})).unwrap();
        assert_eq!(m.state.skills[0]["status"], "docs_updated");
        record["comparisonScopeUnverified"] = json!(true);
        record["comparisonNote"] = json!("old difference");
        record["changeDetails"] = json!([{"path":"SKILL.md","change":"different","side":"both"}]);
        let reason = record["associationReason"].clone();
        let scope = paths(&record["runtimeFiles"]);
        association::reset_observation(&mut record, &scope);
        assert_eq!(record["comparisonScope"], json!(scope));
        assert!(record["changeDetails"].as_array().unwrap().is_empty());
        assert!(record["comparisonDocHash"].is_null());
        assert!(record["comparisonLocalHashes"].is_null());
        assert!(record["comparisonRemoteHashes"].is_null());
        assert!(record["comparisonNote"].is_null());
        assert_eq!(record["comparisonScopeUnverified"], false);
        assert_eq!(record["associationNote"], reason);
    }
    #[test]
    fn partial_push_keeps_unsubmitted_differences_and_full_verification_replaces_history() {
        let d = fixture();
        let left = package(
            &d.path().join("local"),
            "shared-skill",
            "initial instructions",
        );
        let right = package(
            &d.path().join("remote"),
            "shared-skill",
            "initial instructions",
        );
        let mut record = linked(&local_record(&left, true), &right);
        write_entry(&left, "edited instructions");
        fs::write(
            left.join("agents/openai.yaml"),
            "interface:\n  display_name: Edited Skill\n",
        )
        .unwrap();
        // Only SKILL.md was submitted; the changed client configuration remains local.
        fs::write(
            right.join("SKILL.md"),
            fs::read(left.join("SKILL.md")).unwrap(),
        )
        .unwrap();
        record["baselineHashes"]["SKILL.md"] = json!(local::files(&left).unwrap()["SKILL.md"]);
        record["changeDetails"] = json!([{"path":"SKILL.md","change":"modified","side":"local"}]);
        let partial = verified(&record, &right);
        record_push_verification(&mut record, "partial", true, partial);
        assert_eq!(record["baselineCommit"], "first");
        assert_eq!(record["status"], "local_modified");
        assert_eq!(record["changeDetails"].as_array().unwrap().len(), 1);
        assert_eq!(record["changeDetails"][0]["path"], "agents/openai.yaml");
        assert_eq!(record["changeDetails"][0]["side"], "local");
        fs::write(
            right.join("agents/openai.yaml"),
            fs::read(left.join("agents/openai.yaml")).unwrap(),
        )
        .unwrap();
        record["comparisonLocalHashes"] = json!({"SKILL.md":"obsolete"});
        record["comparisonPending"] = json!(true);
        let full = verified(&record, &right);
        record_push_verification(&mut record, "full", true, full);
        assert_eq!(record["baselineCommit"], "full");
        assert_eq!(record["status"], "synced");
        assert!(record["changeDetails"].as_array().unwrap().is_empty());
        assert!(record["comparisonLocalHashes"].is_null());
        assert_eq!(record["comparisonPending"], false);
    }
    #[test]
    fn renamed_directories_need_verified_identical_content() {
        let d = fixture();
        let left = package(
            &d.path().join("local"),
            "custom-local",
            "local instructions",
        );
        let right = package(
            &d.path().join("remote"),
            "custom-remote",
            "remote instructions",
        );
        let local = local_record(&left, true);
        assert!(finish_auto_pair(
            &local,
            &remote_record(),
            &identity("retained-local", "custom-local"),
            &identity("remote-placeholder", "custom-remote"),
            verified(&local, &right),
            "first",
            true
        )
        .is_err());
        write_entry(&right, "local instructions");
        assert!(finish_auto_pair(
            &local,
            &remote_record(),
            &identity("retained-local", "custom-local"),
            &identity("remote-placeholder", "custom-remote"),
            verified(&local, &right),
            "first",
            true
        )
        .is_ok());
    }
    #[test]
    fn failed_sources_never_merge_or_turn_cached_comparisons_into_success() {
        let d = fixture();
        let left = package(&d.path().join("local"), "shared-skill", "local first");
        let right = package(&d.path().join("remote"), "shared-skill", "remote first");
        let mut m = Manager::new(d.path().join("data")).unwrap();
        m.state.sources = vec![
            json!({"id":"source","repo":"invalid-repo-never-contacted","error":"offline failure"}),
        ];
        m.state.skills = vec![local_record(&left, true), remote_record()];
        let before = m.state.skills.clone();
        auto_associate(&mut m).unwrap();
        assert_eq!(m.state.skills.len(), 2);
        assert!(m.state.skills[0]["sourceId"].is_null());
        assert_eq!(m.state.skills[1], before[1]);
        assert_eq!(m.state.sources[0]["error"], "offline failure");
        let mut associated = linked(&before[0], &right);
        associated["status"] = json!("synced");
        m.state.skills = vec![associated];
        refresh_local(&mut m, &json!({})).unwrap();
        assert_eq!(m.state.skills[0]["error"], "offline failure");
        assert_eq!(m.state.skills[0]["remoteCheckRequired"], true);
        assert_ne!(m.state.skills[0]["status"], "synced");
        assert!(m.state.skills[0]["baselineCommit"].is_null());
    }
    #[test]
    fn local_declared_version_refreshes_for_full_and_local_checks_without_changing_provenance() {
        let d = fixture();
        let left = package(
            &d.path().join("local"),
            "shared-skill",
            "initial instructions",
        );
        let right = package(
            &d.path().join("remote"),
            "shared-skill",
            "initial instructions",
        );
        let mut record = linked(&local_record(&left, true), &right);
        record["version"] = json!("0.1.0");
        let original = record.clone();
        let mut m = Manager::new(d.path().join("data")).unwrap();
        // parse_location rejects this before HTTP or credential access.
        m.state.sources =
            vec![json!({"id":"source","repo":"invalid-repo-never-contacted","error":null})];
        m.upsert_skill(record);
        fs::write(left.join("SKILL.md"), "---\nname: changed-in-file\nversion: 9.9.9\nmetadata:\n  version: ignored-nested\n---\nUpgraded content\n").unwrap();
        refresh(&mut m, &json!({"skillId":"retained-local"})).unwrap();
        assert_eq!(m.state.skills[0]["version"], "9.9.9");
        assert!(m.state.skills[0]["error"].as_str().is_some());
        for field in [
            "id",
            "name",
            "displayName",
            "localPath",
            "sourceId",
            "remotePath",
            "repo",
            "baselineCommit",
            "baselineHashes",
            "categoryId",
        ] {
            assert_eq!(m.state.skills[0][field], original[field], "{field}");
        }
        fs::write(
            left.join("SKILL.md"),
            "---\nname: changed-in-file\nmetadata:\n  version: ' 10.0.0 '\n...\nUpgraded again\n",
        )
        .unwrap();
        refresh(
            &mut m,
            &json!({"skillId":"retained-local","localOnly":true}),
        )
        .unwrap();
        assert_eq!(m.state.skills[0]["version"], "10.0.0");
        assert_eq!(m.state.skills[0]["remoteCheckRequired"], true);
        for field in [
            "name",
            "displayName",
            "sourceId",
            "baselineCommit",
            "baselineHashes",
            "categoryId",
        ] {
            assert_eq!(m.state.skills[0][field], original[field], "{field}");
        }
    }
    #[test]
    fn missing_or_invalid_local_version_preserves_the_existing_source_version() {
        let d = fixture();
        let left = package(
            &d.path().join("local"),
            "shared-skill",
            "initial instructions",
        );
        let mut record = local_record(&left, true);
        record["version"] = json!("source-version");
        let mut m = Manager::new(d.path().join("data")).unwrap();
        m.upsert_skill(record.clone());
        for content in [
            "---\nname: shared-skill\n---\nNo version\n",
            "---\nname: shared-skill\nversion: [invalid\n---\nInvalid YAML\n",
            "---\nname: shared-skill\nversion: false\nmetadata:\n  version: {}\n---\nInvalid scalar\n",
            "---\nname: shared-skill\nversion: 12.0.0\nUnterminated YAML\n",
        ] {
            fs::write(left.join("SKILL.md"), content).unwrap();
            refresh(&mut m, &json!({"skillId":"retained-local","localOnly":true})).unwrap();
            assert_eq!(m.state.skills[0]["version"], "source-version");
            assert_eq!(m.state.skills[0]["name"], record["name"]);
            assert_eq!(m.state.skills[0]["baselineHashes"], record["baselineHashes"]);
        }
        fs::write(
            left.join("SKILL.md"),
            "---\nname: shared-skill\nversion: 3\n---\nNumeric version\n",
        )
        .unwrap();
        refresh(
            &mut m,
            &json!({"skillId":"retained-local","localOnly":true}),
        )
        .unwrap();
        assert_eq!(m.state.skills[0]["version"], "3");
    }
    #[test]
    fn scoped_local_refresh_preserves_every_unselected_record() {
        let d = fixture();
        let left = package(&d.path().join("local"), "shared-skill", "local first");
        let right = package(&d.path().join("remote"), "shared-skill", "remote first");
        let mut m = Manager::new(d.path().join("data")).unwrap();
        m.state.sources = vec![json!({"id":"source","error":null})];
        m.state.skills = vec![
            linked(&local_record(&left, true), &right),
            json!({"id":"unselected","status":"remote_only","name":"leave-me","sourceId":"source"}),
        ];
        let untouched = m.state.skills[1].clone();
        write_entry(&left, "new local instructions");
        refresh_local(&mut m, &json!({"skillIds":["retained-local"]})).unwrap();
        assert_eq!(m.state.skills[0]["status"], "local_modified");
        assert_eq!(m.state.skills[1], untouched);
    }

    #[test]
    fn extra_files_are_secondary_to_runtime_changes_and_documentation_updates() {
        let d = fixture();
        let left = package(&d.path().join("local"), "shared-skill", "same instructions");
        let right = package(
            &d.path().join("remote"),
            "shared-skill",
            "same instructions",
        );
        let mut record = linked(&local_record(&left, true), &right);
        fs::write(
            left.join("README.md"),
            "My local notes, outside runtime scope",
        )
        .unwrap();
        let original_files = local::files(&left).unwrap();
        let checked = verified(&record, &right);
        record_verification(&mut record, "same-runtime", true, checked);
        assert_eq!(record["status"], "local_extra");
        assert_eq!(record["untrackedFiles"], json!(["README.md"]));
        assert_eq!(attention_count(std::slice::from_ref(&record)), 1);
        assert!(record["changeDetails"].as_array().unwrap().is_empty());
        let mut m = Manager::new(d.path().join("data")).unwrap();
        m.state.sources = vec![json!({"id":"source","error":null})];
        m.upsert_skill(record.clone());
        refresh_local(&mut m, &json!({"skillId":"retained-local"})).unwrap();
        assert_eq!(m.state.skills[0]["status"], "local_extra");
        assert_eq!(local::files(&left).unwrap(), original_files);
        let mut docs = verified(&record, &right);
        docs.docs = "new-docs".into();
        record_verification(&mut record, "documentation-update", true, docs);
        assert_eq!(record["status"], "docs_updated");
        assert_eq!(record["untrackedFiles"], json!(["README.md"]));
        assert_eq!(
            attention_count(std::slice::from_ref(&record)),
            1,
            "docs-only refresh does not resolve an independent extra-file decision"
        );
        let mut docs_only = record.clone();
        docs_only["untrackedFiles"] = json!([]);
        docs_only["localNotice"] = Value::Null;
        assert_eq!(attention_count(std::slice::from_ref(&docs_only)), 0);
        docs_only["runtimeIssues"] = json!([{"category":"uncertain","severity":"warning"}]);
        assert_eq!(attention_count(std::slice::from_ref(&docs_only)), 1);
        docs_only["runtimeIssues"] = json!([{"category":"external","severity":"info"}]);
        docs_only["runtimeAudit"] = json!({"status":"passed"});
        assert_eq!(attention_count(std::slice::from_ref(&docs_only)), 0);
        for status in ["needs_review", "failed"] {
            docs_only["runtimeAudit"] = json!({"status":status});
            assert_eq!(attention_count(std::slice::from_ref(&docs_only)), 1, "an unresolved or failed audit remains actionable even if its individual issues are external information");
        }
        m.upsert_skill(record.clone());
        refresh_local(&mut m, &json!({"skillId":"retained-local"})).unwrap();
        assert_eq!(m.state.skills[0]["status"], "docs_updated");
        assert_eq!(attention_count(&m.state.skills), 1);
        write_entry(&left, "changed local runtime");
        refresh_local(&mut m, &json!({"skillId":"retained-local"})).unwrap();
        assert_eq!(m.state.skills[0]["status"], "local_modified");
        assert_eq!(m.state.skills[0]["changeDetails"][0]["path"], "SKILL.md");
        write_entry(&left, "same instructions");
        write_entry(&right, "changed remote runtime");
        let checked = verified(&record, &right);
        record_verification(&mut record, "remote-update", true, checked);
        assert_eq!(record["status"], "remote_updated");
        write_entry(&left, "changed local runtime");
        let checked = verified(&record, &right);
        record_verification(&mut record, "remote-update", true, checked);
        assert_eq!(record["status"], "both_modified");
        assert_eq!(record["untrackedFiles"], json!(["README.md"]));
    }

    #[test]
    fn comparison_dependencies_are_not_extras_and_remote_only_success_has_no_local_time() {
        let d = fixture();
        let left = package(&d.path().join("local"), "shared-skill", "same instructions");
        let right = package(
            &d.path().join("remote"),
            "shared-skill",
            "same instructions",
        );
        let mut record = local_record(&left, false);
        let checked = verified(&record, &right);
        // SKILL.md alone was selected, but dependency discovery includes the client config and license.
        record["runtimeFiles"] = json!(["SKILL.md"]);
        record_verification(&mut record, "scope-verified", true, checked);
        assert_eq!(record["status"], "synced");
        assert!(record["untrackedFiles"].as_array().unwrap().is_empty());
        let mut m = Manager::new(d.path().join("data")).unwrap();
        m.upsert_skill(record);
        m.state.skills[0]["sourceId"] = json!("source");
        refresh_local(&mut m, &json!({"skillId":"retained-local"})).unwrap();
        assert!(m.state.skills[0]["untrackedFiles"]
            .as_array()
            .unwrap()
            .is_empty());
        let mut remote = remote_record();
        let scope = vec![
            "LICENSE".into(),
            "SKILL.md".into(),
            "agents/openai.yaml".into(),
        ];
        let checked = VerifiedSkill {
            scope,
            all: BTreeMap::new(),
            local: BTreeMap::new(),
            remote: local::files(&right).unwrap(),
            docs: "same-docs".into(),
            item: None,
            internal_name: "shared-skill".into(),
            can_adopt: false,
        };
        record_verification(&mut remote, "remote-verified", true, checked);
        assert_eq!(remote["status"], "remote_only");
        assert!(remote["lastRemoteCheckedAt"].as_str().is_some());
        assert!(remote["lastLocalCheckedAt"].is_null());
    }

    #[test]
    fn full_and_cached_local_checks_keep_distinct_success_times_and_favorites() {
        let d = fixture();
        let left = package(&d.path().join("local"), "shared-skill", "same instructions");
        let right = package(
            &d.path().join("remote"),
            "shared-skill",
            "same instructions",
        );
        let mut record = linked(&local_record(&left, true), &right);
        assert!(chrono::DateTime::parse_from_rfc3339(
            record["lastLocalCheckedAt"].as_str().unwrap()
        )
        .is_ok());
        assert_eq!(record["lastLocalCheckedAt"], record["lastRemoteCheckedAt"]);
        record["favorite"] = json!(true);
        record["lastLocalCheckedAt"] = json!("2000-01-01T00:00:00Z");
        record["lastRemoteCheckedAt"] = json!("2001-01-01T00:00:00Z");
        let mut m = Manager::new(d.path().join("data")).unwrap();
        m.state.sources = vec![json!({"id":"source","error":null})];
        m.upsert_skill(record.clone());
        refresh_local(&mut m, &json!({"skillId":"retained-local"})).unwrap();
        assert_ne!(
            m.state.skills[0]["lastLocalCheckedAt"],
            record["lastLocalCheckedAt"]
        );
        assert_eq!(
            m.state.skills[0]["lastRemoteCheckedAt"],
            record["lastRemoteCheckedAt"]
        );
        assert_eq!(m.state.skills[0]["favorite"], true);
        // Full checks read local files even if the remote source is rejected before HTTP.
        m.state.skills[0]["lastLocalCheckedAt"] = record["lastLocalCheckedAt"].clone();
        m.state.sources[0]["repo"] = json!("invalid-repo-never-contacted");
        refresh(&mut m, &json!({"skillId":"retained-local"})).unwrap();
        assert_ne!(
            m.state.skills[0]["lastLocalCheckedAt"],
            record["lastLocalCheckedAt"]
        );
        assert_eq!(
            m.state.skills[0]["lastRemoteCheckedAt"],
            record["lastRemoteCheckedAt"]
        );
        assert_eq!(m.state.skills[0]["favorite"], true);
        let success = m.state.skills[0]["lastLocalCheckedAt"].clone();
        m.state.skills[0]["localPath"] = json!(d.path().join("never-created"));
        refresh_local(&mut m, &json!({"skillId":"retained-local"})).unwrap();
        assert_eq!(m.state.skills[0]["lastLocalCheckedAt"], success);
        assert_eq!(
            m.state.skills[0]["lastRemoteCheckedAt"],
            record["lastRemoteCheckedAt"]
        );
        assert!(m.state.skills[0]["error"].as_str().is_some());
    }

    #[test]
    fn legacy_check_times_stay_unknown_until_the_corresponding_read_succeeds() {
        let d = fixture();
        let left = package(&d.path().join("local"), "shared-skill", "same instructions");
        let right = package(
            &d.path().join("remote"),
            "shared-skill",
            "same instructions",
        );
        let mut record = linked(&local_record(&left, true), &right);
        for field in ["lastLocalCheckedAt", "lastRemoteCheckedAt"] {
            record.as_object_mut().unwrap().remove(field);
        }
        record["lastChecked"] = json!("2025-01-01T00:00:00Z");
        let mut m = Manager::new(d.path().join("data")).unwrap();
        m.state.sources = vec![json!({"id":"source","error":null})];
        m.upsert_skill(record);
        m.save().unwrap();
        drop(m);
        let mut m = Manager::new(d.path().join("data")).unwrap();
        assert!(m.state.skills[0]["lastLocalCheckedAt"].is_null());
        assert!(m.state.skills[0]["lastRemoteCheckedAt"].is_null());
        refresh_local(&mut m, &json!({"skillId":"retained-local"})).unwrap();
        assert!(m.state.skills[0]["lastLocalCheckedAt"].as_str().is_some());
        assert!(m.state.skills[0]["lastRemoteCheckedAt"].is_null());
        let checked = verified(&m.state.skills[0], &right);
        record_verification(&mut m.state.skills[0], "full", true, checked);
        assert!(m.state.skills[0]["lastRemoteCheckedAt"].as_str().is_some());
    }

    #[test]
    fn failed_verification_after_target_change_does_not_reuse_prior_source_time() {
        let d = fixture();
        let left = package(&d.path().join("local"), "shared-skill", "same instructions");
        let right = package(
            &d.path().join("remote"),
            "shared-skill",
            "same instructions",
        );
        let original = linked(&local_record(&left, true), &right);
        assert!(original["lastRemoteCheckedAt"].as_str().is_some());
        let mut new_target = remote_record();
        new_target["sourceId"] = json!("new-source");
        // Invalid repository syntax rejects before network or credential access.
        new_target["repo"] = json!("invalid-repo-never-contacted");
        let merged = association::merge(&original, &new_target, "new target");
        assert!(merged["lastRemoteCheckedAt"].is_null());
        let mut m = Manager::new(d.path().join("data")).unwrap();
        m.state.sources = vec![
            json!({"id":"new-source","repo":"invalid-repo-never-contacted","branch":"main","error":null}),
        ];
        m.upsert_skill(merged);
        refresh(&mut m, &json!({"skillId":"retained-local"})).unwrap();
        assert!(m.state.skills[0]["error"].as_str().is_some());
        assert_eq!(m.state.skills[0]["remoteCheckRequired"], true);
        assert!(m.state.skills[0]["lastRemoteCheckedAt"].is_null());
        assert!(m.state.skills[0]["lastLocalCheckedAt"].as_str().is_some());
    }

    #[test]
    fn registered_local_only_checks_record_local_success_and_preserve_it_on_failure() {
        let d = fixture();
        let left = package(
            &d.path().join("local"),
            "shared-skill",
            "local instructions",
        );
        let mut record = local_record(&left, true);
        assert!(record["lastLocalCheckedAt"].as_str().is_some());
        assert!(record["lastRemoteCheckedAt"].is_null());
        record["lastLocalCheckedAt"] = json!("2000-01-01T00:00:00Z");
        record["favorite"] = json!(true);
        let mut m = Manager::new(d.path().join("data")).unwrap();
        m.upsert_skill(record.clone());
        crate::dispatch(
            &mut m,
            &json!({"action":"refresh","skillId":"retained-local"}),
        )
        .unwrap();
        assert_ne!(
            m.state.skills[0]["lastLocalCheckedAt"],
            record["lastLocalCheckedAt"]
        );
        assert_eq!(m.state.skills[0]["favorite"], true);
        let success = m.state.skills[0]["lastLocalCheckedAt"].clone();
        m.state.skills[0]["localPath"] = json!(d.path().join("never-created"));
        crate::dispatch(
            &mut m,
            &json!({"action":"refresh","skillId":"retained-local"}),
        )
        .unwrap();
        assert_eq!(m.state.skills[0]["lastLocalCheckedAt"], success);
        assert!(m.state.skills[0]["lastRemoteCheckedAt"].is_null());
        assert!(m.state.skills[0]["error"].as_str().is_some());
    }
}

#[cfg(all(test, windows))]
mod preview_target_tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/test-fixtures");
        fs::create_dir_all(&root).unwrap();
        tempfile::Builder::new()
            .prefix("preview-target-")
            .tempdir_in(root)
            .unwrap()
    }

    #[test]
    fn preview_accepts_initial_link_but_rejects_retargeted_canonical_runtime_root() {
        let d = fixture();
        let real = d.path().join("real");
        let other = d.path().join("other");
        fs::create_dir(&real).unwrap();
        fs::create_dir(&other).unwrap();
        for path in [&real, &other] {
            fs::write(path.join("SKILL.md"), b"identical preview bytes").unwrap();
        }
        let initial_choice = d.path().join("chosen-link");
        junction::create(&real, &initial_choice).unwrap();
        let recorded = target_directory(initial_choice.to_str().unwrap()).unwrap();
        assert_eq!(recorded, local::canonical(&real).unwrap());
        assert!(checked_preview_target(recorded.to_str().unwrap()).is_ok());
        fs::remove_dir(&initial_choice).unwrap();
        junction::create(&other, &initial_choice).unwrap();
        // Repointing the original selection cannot change the saved canonical target.
        assert_eq!(
            checked_preview_target(recorded.to_str().unwrap()).unwrap(),
            recorded
        );
        fs::remove_dir(&initial_choice).unwrap();

        let before = local::files(&real).unwrap();
        fs::rename(&real, d.path().join("displaced-original")).unwrap();
        junction::create(&other, &real).unwrap();
        assert_eq!(
            local::files(&recorded).unwrap(),
            before,
            "A byte-only drift check would miss this redirection"
        );
        assert!(checked_preview_target(recorded.to_str().unwrap()).is_err());
        assert!(local_root(&json!({"localPath":recorded.to_string_lossy()})).is_err());
        assert_eq!(
            fs::read(other.join("SKILL.md")).unwrap(),
            b"identical preview bytes"
        );
        fs::remove_dir(&real).unwrap();
    }

    #[test]
    fn preview_rejects_a_new_target_when_its_existing_parent_becomes_a_junction() {
        let d = fixture();
        let parent = d.path().join("selected-parent");
        let other = d.path().join("other-parent");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&other).unwrap();
        let recorded = target_directory(parent.join("new-skill").to_str().unwrap()).unwrap();
        assert!(checked_preview_target(recorded.to_str().unwrap()).is_ok());
        fs::rename(&parent, d.path().join("original-parent")).unwrap();
        junction::create(&other, &parent).unwrap();
        assert!(checked_preview_target(recorded.to_str().unwrap()).is_err());
        assert!(!other.join("new-skill").exists());
        assert!(!d.path().join("original-parent/new-skill").exists());
        fs::remove_dir(&parent).unwrap();
    }
}
