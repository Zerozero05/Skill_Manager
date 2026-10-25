//! Read-only merged inventory and explicit, pinned attachment operations.
//! Attachment choices never become runtime configuration or installation history.
use super::*;

const PROVENANCE: &str = "skill-manager-provenance.json";

fn pinned(m: &Manager, skill: &Value, req: &Value) -> Result<Snapshot, String> {
    let source = source_for(m, skill)?;
    if source["repo"] != skill["repo"] {
        return Err("Skill/source association is inconsistent".into());
    }
    let commit = text(req, "remoteCommit")?;
    github::sha(&json!(commit))?;
    Snapshot::at_commit(repo(skill)?, commit)
}

fn safe_entries(entries: &BTreeMap<String, Entry>) -> BTreeMap<String, Entry> {
    entries
        .iter()
        .filter(|(path, entry)| {
            github::validate_relative(path).is_ok()
                && matches!(entry.mode.as_str(), "100644" | "100755")
                && !path.split('/').any(|part| {
                    matches!(
                        part,
                        ".git" | ".skill-manager" | ".skill-manager-state.json"
                    )
                })
        })
        .map(|(path, entry)| (path.clone(), entry.clone()))
        .collect()
}

fn empty_inventory() -> Value {
    json!({"files":[],"runtimeConfirmed":false,"requiredFiles":[],"optionalFiles":[],
        "suggestedFiles":[],"requiredReasons":{},"warnings":[]})
}

/// The local classification is authoritative for an existing installation.
/// Upstream SKILL.md changes cannot silently broaden its saved runtime set.
fn merge_inventory(mut local: Value, remote: &BTreeMap<String, Entry>) -> Value {
    let mut rows: BTreeMap<String, Value> = local["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            row["path"]
                .as_str()
                .map(|path| (path.to_owned(), row.clone()))
        })
        .collect();
    for row in rows.values_mut() {
        row["localAvailable"] = json!(true);
        row["remoteAvailable"] = json!(false);
        row["localHash"] = row["hash"].clone();
        row["hashAlgorithm"] = json!("sha256");
    }
    for (path, entry) in remote {
        let row = rows.entry(path.clone()).or_insert_with(|| json!({"path":path,"size":entry.size,
            "hash":entry.sha,"hashAlgorithm":"git-blob-sha1","runtime":false,"localAvailable":false}));
        row["remoteAvailable"] = json!(true);
        row["remoteHash"] = json!(entry.sha);
        row["remoteHashAlgorithm"] = json!("git-blob-sha1");
        row["remoteSize"] = json!(entry.size);
        row["remoteSource"] = json!(entry.source);
        // The two hashes use different algorithms. Equality is unknown here.
    }
    let required: BTreeSet<_> = paths(&local["requiredFiles"]).into_iter().collect();
    local["optionalFiles"] = json!(rows
        .keys()
        .filter(|path| !required.contains(*path))
        .collect::<Vec<_>>());
    local["files"] = json!(rows.into_values().collect::<Vec<_>>());
    local
}

pub(crate) fn get_files(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let skill = crate::external::resolution_skill(m, &m.skill(text(req, "skillId")?)?);
    let has_local = skill["localPath"].as_str().is_some();
    let local = if has_local {
        local::handle(m, req)?.ok_or("Local file inventory is unavailable")?
    } else {
        empty_inventory()
    };
    if skill["sourceId"].as_str().is_none() {
        let mut result = merge_inventory(local, &BTreeMap::new());
        result["remoteStatus"] = json!("not_linked");
        return Ok(result);
    }
    let remote = (|| -> Result<Value, String> {
        let snap = if req["remoteCommit"].is_string() {
            pinned(m, &skill, req)?
        } else {
            snapshot(m, &skill)?
        };
        let entries = safe_entries(&snap.skill_files(remote_path(&skill)?)?);
        let mut result = if has_local {
            merge_inventory(local.clone(), &entries)
        } else {
            let (manifest, _) = manifests(&snap, remote_path(&skill)?)?;
            let selected = manifest.unwrap_or_else(|| paths(&skill["runtimeFiles"]));
            let selection = selected.iter().cloned().collect::<BTreeSet<_>>();
            let inspected = runtime::inspect(
                &snap,
                &entries,
                (!selection.is_empty()).then_some(&selection),
            )?;
            let base = json!({"files":[],"runtimeConfirmed":skill["runtimeConfirmed"],
                "requiredFiles":inspected.required_files,"optionalFiles":inspected.optional_files,
                "requiredReasons":inspected.required_reasons,"suggestedFiles":inspected.suggested_files,
                "warnings":inspected.warnings,"issues":inspected.issues});
            let mut result = merge_inventory(base, &entries);
            for row in result["files"]
                .as_array_mut()
                .ok_or("Invalid file inventory")?
            {
                row["runtime"] = json!(selected.contains(&text(row, "path")?.to_owned()));
            }
            result
        };
        let root = has_local.then(|| local_root(&skill)).transpose()?;
        // The local handle already verified and hashed this request's inventory.
        // Reconstruct it from that internal result instead of hashing every file again.
        let hashes = local["files"]
            .as_array()
            .ok_or("Local file inventory is unavailable")?
            .iter()
            .map(|entry| {
                Ok((
                    text(entry, "path")?.to_owned(),
                    text(entry, "hash")?.to_owned(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let scope = paths(&result["suggestedFiles"])
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut merged = crate::external::merged_classification(
            root.as_deref(),
            &hashes,
            &snap,
            remote_path(&skill)?,
            (!scope.is_empty()).then_some(&scope),
        )?;
        let mut issues =
            serde_json::from_value::<Vec<local::RuntimeIssue>>(result["issues"].clone())
                .unwrap_or_default();
        issues.extend(merged.issues.clone());
        issues.sort();
        issues.dedup();
        let external =
            crate::external::inspect_snapshot(&skill, root.as_deref(), &hashes, &issues, &snap);
        merged.issues = issues;
        crate::external::apply_resolutions(&mut merged, &external);
        result["issues"] = json!(merged.issues);
        result["warnings"] = json!(merged.warnings);
        result["externalDependencies"] = json!(external);
        result["externalSummary"] =
            crate::external::report(&external, &skill["id"])["summary"].clone();
        // Export uses a fresh minimal closure over the same local-first readers,
        // independently of a historical manually selected optional runtime set.
        let mut all = entries
            .iter()
            .map(|(path, entry)| (path.clone(), entry.sha.clone()))
            .collect::<BTreeMap<_, _>>();
        all.extend(hashes.clone());
        let roots = local::base_required_reasons(&all).into_keys().collect();
        let mut minimal = crate::external::merged_classification(
            root.as_deref(),
            &hashes,
            &snap,
            remote_path(&skill)?,
            Some(&roots),
        )?;
        crate::external::apply_resolutions(&mut minimal, &external);
        result["exportRequiredFiles"] = json!(minimal.required_files);
        result["exportError"] = json!(minimal
            .issues
            .iter()
            .find(|issue| issue.severity == "error" || issue.category == "uncertain")
            .map(|issue| format!("{}: {}", issue.target, issue.reason))
            .or_else(|| external
                .iter()
                .find(|item| item.required && item.status != "available")
                .map(|item| format!("{}: {}", item.target, item.reason))));
        result["remoteStatus"] = json!("ok");
        result["remoteCommit"] = json!(snap.commit);
        result["remoteCheckedAt"] = json!(now());
        Ok(result)
    })();
    Ok(match remote {
        Ok(result) => result,
        Err(error) => {
            let mut result = merge_inventory(local, &BTreeMap::new());
            if let Some(items) = result["externalDependencies"].as_array_mut() {
                for item in items {
                    item["remote"] = json!({"status":"error","error":error,"checkedAt":now()});
                }
            }
            result["remoteStatus"] = json!("error");
            result["remoteError"] = json!(error);
            result
        }
    })
}

fn read_remote(
    entries: &BTreeMap<String, Entry>,
    path: &str,
    read: impl FnOnce(&Entry) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    github::validate_relative(path)?;
    let entry = entries
        .get(path)
        .ok_or_else(|| format!("File does not exist at this version: {path}"))?;
    if !matches!(entry.mode.as_str(), "100644" | "100755") || entry.size > MAX_FILE as u64 {
        return Err("Remote file is a link or exceeds the 4 MiB limit".into());
    }
    let bytes = read(entry)?;
    if bytes.len() > MAX_FILE || bytes.len() as u64 != entry.size {
        return Err(
            "Remote file length does not match the selected immutable repository tree".into(),
        );
    }
    Ok(bytes)
}

fn read_file(m: &Manager, req: &Value) -> Result<Value, String> {
    let skill = m.skill(text(req, "skillId")?)?;
    let snap = pinned(m, &skill, req)?;
    let entries = safe_entries(&snap.skill_files(remote_path(&skill)?)?);
    let path = text(req, "path")?;
    let bytes = read_remote(&entries, path, |entry| snap.read_entry(entry))?;
    if bytes.len() > 1024 * 1024 || bytes.contains(&0) {
        return Err("File is binary or exceeds the 1 MiB preview limit".into());
    }
    let digest = hash(&bytes);
    let content = String::from_utf8(bytes).map_err(|_| "File is not UTF-8 text")?;
    Ok(
        json!({"content":content,"source":"github","remoteCommit":snap.commit,"hash":digest,
        "hashAlgorithm":"sha256","repositoryPath":entries[path].source}),
    )
}

fn checked_paths(files: Vec<String>) -> Result<Vec<String>, String> {
    let files = checked_selection(&files)?;
    for path in &files {
        if path == PROVENANCE
            || path.split('/').any(|part| {
                matches!(
                    part,
                    ".git" | ".skill-manager" | ".skill-manager-state.json"
                )
            })
        {
            return Err(format!(
                "Reserved management path cannot be selected: {path}"
            ));
        }
    }
    Ok(files)
}

/// Reject Windows aliases before downloads or writes, including directory prefixes.
fn reject_case_aliases<'a>(paths: impl IntoIterator<Item = &'a String>) -> Result<(), String> {
    let mut seen = BTreeMap::new();
    for path in paths {
        let mut prefix = String::new();
        for part in path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if let Some(prior) = seen.insert(prefix.to_lowercase(), prefix.clone()) {
                if prior != prefix {
                    return Err(format!("Case-colliding package paths: {prior} / {prefix}"));
                }
            }
        }
    }
    Ok(())
}

fn collect_export(
    root: &Path,
    expected: &BTreeMap<String, String>,
    runtime: &[String],
    extras: &[String],
    entries: &BTreeMap<String, Entry>,
    mut read: impl FnMut(&Entry) -> Result<Vec<u8>, String>,
    commit: Option<&str>,
) -> Result<(BTreeMap<String, Vec<u8>>, Value), String> {
    let selected: BTreeSet<_> = runtime.iter().chain(extras.iter()).cloned().collect();
    let selected = checked_paths(selected.into_iter().collect())?;
    reject_case_aliases(expected.keys().chain(selected.iter()))?;
    let mut data = BTreeMap::new();
    let mut sources = serde_json::Map::new();
    let mut total = 0usize;
    for path in &selected {
        if crate::external::private_resource(Path::new(path)) {
            return Err(format!(
                "凭据或机器私有配置不能加入 ZIP：{path}；请改为运行环境配置"
            ));
        }
        if let Some(expected_hash) = expected.get(path) {
            // Existing local assets retain the streaming export path and its
            // capacity; GitHub limits apply only to downloaded remote bytes.
            let actual = local::safe_join(root, path)?;
            if crate::external::private_resource(&actual) {
                return Err(format!("凭据或机器私有配置不能加入 ZIP：{path}"));
            }
            sources.insert(
                path.clone(),
                json!({"source":"local","hash":expected_hash,"hashAlgorithm":"sha256"}),
            );
            continue;
        }
        let commit =
            commit.ok_or("Refresh the GitHub file list before exporting remote attachments")?;
        if entries
            .get(path)
            .is_some_and(|entry| crate::external::private_resource(Path::new(&entry.source)))
        {
            return Err(format!("凭据或机器私有配置不能加入 ZIP：{path}"));
        }
        let bytes = read_remote(entries, path, &mut read)?;
        let entry = &entries[path];
        let receipt = json!({"source":"github","commit":commit,"repositoryPath":entry.source,
            "hash":hash(&bytes),"hashAlgorithm":"sha256","gitBlobSha":entry.sha});
        total = total
            .checked_add(bytes.len())
            .filter(|size| *size <= MAX_TOTAL)
            .ok_or("Remote export attachments exceed the 32 MiB limit")?;
        sources.insert(path.clone(), receipt);
        data.insert(path.clone(), bytes);
    }
    Ok((data, Value::Object(sources)))
}

pub(crate) struct PreparedExport {
    pub root: PathBuf,
    pub expected: BTreeMap<String, String>,
    pub local_paths: Vec<String>,
    pub data: BTreeMap<String, Vec<u8>>,
    pub provenance: Value,
}

pub(crate) fn prepare_export(m: &Manager, req: &Value) -> Result<PreparedExport, String> {
    let skill = crate::external::resolution_skill(m, &m.skill(text(req, "skillId")?)?);
    let root = local_root(&skill)?;
    let extras = req
        .get("extraFiles")
        .map(strings)
        .transpose()?
        .unwrap_or_default();
    // Reject unsafe extras before any network request.
    if !extras.is_empty() {
        checked_paths(extras.clone())?;
    }
    let expected = local::files(&root)?;
    let external_ids = req
        .get("externalIds")
        .map(strings)
        .transpose()?
        .unwrap_or_default();
    let needs_remote = extras.iter().any(|path| !expected.contains_key(path))
        || req["remoteCommit"].is_string()
        || skill["externalBindings"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|binding| {
                binding["sourceKind"] == "github"
                    && (binding["role"] == "required"
                        || binding["id"]
                            .as_str()
                            .is_some_and(|id| external_ids.iter().any(|selected| selected == id)))
            });
    let snap = needs_remote.then(|| pinned(m, &skill, req)).transpose()?;
    let entries = snap
        .as_ref()
        .map(|snap| snap.skill_files(remote_path(&skill)?))
        .transpose()?
        .unwrap_or_default();
    let entries = safe_entries(&entries);
    let runtime = if let Some(snap) = snap.as_ref() {
        let mut all = entries
            .iter()
            .map(|(path, entry)| (path.clone(), entry.sha.clone()))
            .collect::<BTreeMap<_, _>>();
        all.extend(expected.clone());
        let roots = local::base_required_reasons(&all).into_keys().collect();
        let mut selected = crate::external::merged_classification(
            Some(&root),
            &expected,
            snap,
            remote_path(&skill)?,
            Some(&roots),
        )?;
        let resolved = crate::external::inspect_snapshot(&skill,Some(&root),&expected,&selected.issues,snap);
        crate::external::apply_resolutions(&mut selected,&resolved);
        if let Some(issue) = selected
            .issues
            .iter()
            .find(|issue| issue.severity == "error" || issue.category == "uncertain")
        {
            return Err(format!(
                "必要文件未能完整解析：{}；{}",
                issue.target, issue.reason
            ));
        }
        selected.required_files
    } else {
        local::minimal_runtime_for_skill(&skill,&root,&expected)?
    };
    let (mut data, mut sources) = collect_export(
        &root,
        &expected,
        &runtime,
        &extras,
        &entries,
        |entry| {
            snap.as_ref()
                .ok_or("Remote file source missing")?
                .read_entry(entry)
        },
        snap.as_ref().map(|snap| snap.commit.as_str()),
    )?;
    let external = crate::external::export_resources_for_plan(
        &skill,
        &runtime,
        &external_ids,
        snap.as_ref(),
        req["allowEnvironmentFiles"] == true,
    )?;
    for (path, bytes) in &external.data {
        if expected
            .get(path)
            .is_some_and(|prior| prior != &hash(bytes))
            || data.get(path).is_some_and(|prior| prior != bytes)
        {
            return Err("外部资源的包内路径与 Skill 文件冲突，请调整依赖后重试".into());
        }
        data.insert(path.clone(), bytes.clone());
        // An already collected package file has the same verified bytes; keep
        // its original immutable GitHub/local receipt when a reference aliases it.
        if sources.get(path).is_none() {
            sources[path] =
                json!({"source":"external_resource","hash":hash(bytes),"hashAlgorithm":"sha256"});
        }
    }
    for (path, bytes) in &external.replacements {
        data.insert(path.clone(), bytes.clone());
        let mut receipt = sources
            .get(path)
            .cloned()
            .ok_or("改写文档缺少已核验的原始来源记录，请刷新导出清单")?;
        let original_hash = text(&receipt, "hash")?.to_owned();
        receipt["source"] = json!(if expected.contains_key(path) {
            "local_zip_copy"
        } else {
            "github_zip_copy"
        });
        receipt["originalHash"] = json!(original_hash);
        receipt["hash"] = json!(hash(bytes));
        receipt["referenceRewritten"] = json!(true);
        sources[path] = receipt;
    }
    if data.values().map(Vec::len).sum::<usize>() > MAX_TOTAL {
        return Err("ZIP 中需要读取的远端及外部资源总大小超过 32 MiB".into());
    }
    reject_case_aliases(expected.keys().chain(data.keys()))?;
    let selection = runtime.iter().cloned().collect();
    let classification = local::runtime_file_selection(&root, &expected, Some(&selection))?;
    let warnings =
        crate::external::sanitized_warnings(&classification.warnings, &classification.issues);
    let local_paths: BTreeSet<_> = runtime
        .iter()
        .chain(extras.iter())
        .filter(|path| expected.contains_key(*path) && !data.contains_key(*path))
        .cloned()
        .collect();
    let exported_hashes: BTreeMap<_, _> = local_paths
        .iter()
        .map(|path| (path.clone(), expected[path].clone()))
        .chain(hashes(&data))
        .collect();
    let license_files: Vec<_> = exported_hashes
        .keys()
        .filter(|path| local::license_path(path))
        .cloned()
        .collect();
    let provenance = json!({"formatVersion":2,"name":skill["name"],"version":skill["version"],
        "repo":skill["repo"],"branch":skill["branch"],"commit":skill["commit"],
        "remoteCommit":snap.as_ref().map(|snap| &snap.commit),"baselineCommit":skill["baselineCommit"],
        "remotePath":skill["remotePath"],"private":skill["private"],"exportedAt":now(),
        "runtimeConfirmed":skill["runtimeConfirmed"]==true,"runtimeConfirmationKind":skill["runtimeConfirmationKind"],
        "exportRuntimeVerification":"static_required_closure","runtimeFiles":runtime,"extraFiles":extras,"files":sources,
        "hashes":exported_hashes,"licenseFiles":license_files,
        "licenseStatus":if !license_files.is_empty() { "included" } else { "unknown; no license file found; redistribution rights are not established" },
        "externalResources":external.provenance,"warnings":warnings});
    Ok(PreparedExport {
        root,
        expected,
        local_paths: local_paths.into_iter().collect(),
        data,
        provenance,
    })
}

fn export(m: &Manager, req: &Value) -> Result<Value, String> {
    crate::export_plan::legacy_export(m, req)
}

pub(super) fn preview_with_reader(
    m: &mut Manager,
    skill: &Value,
    selected: &[String],
    commit: &str,
    entries: &BTreeMap<String, Entry>,
    mut read: impl FnMut(&Entry) -> Result<Vec<u8>, String>,
) -> Result<Value, String> {
    writable(skill)?;
    let root = local_root(skill)?;
    let expected = local::files(&root)?;
    let selected = checked_paths(selected.to_vec())?;
    reject_case_aliases(expected.keys().chain(selected.iter()))?;
    let mut writes = BTreeMap::new();
    let mut rows = Vec::new();
    let mut total = 0usize;
    for path in &selected {
        let destination = local::safe_join(&root, path)?;
        if fs::symlink_metadata(destination).is_ok() {
            return Err(format!("Attachment already exists locally; no file or directory will be overwritten: {path}"));
        }
        let bytes = read_remote(entries, path, &mut read)?;
        total = total
            .checked_add(bytes.len())
            .filter(|size| *size <= MAX_TOTAL)
            .ok_or("Attachment download exceeds the 32 MiB limit")?;
        rows.push(
            json!({"path":path,"change":"add","local":null,"remote":preview_text(&bytes),
            "hash":hash(&bytes),"size":bytes.len(),"repositoryPath":entries[path].source}),
        );
        writes.insert(path.clone(), STANDARD.encode(bytes));
    }
    if local::files(&root)? != expected {
        return Err("Local files changed while preparing attachments; preview again".into());
    }
    let id = m.id();
    let public = json!({"id":id,"kind":"attachments","skillId":skill["id"],"summary":format!("新增 {} 个附件；保留现有文件和运行范围",rows.len()),
        "files":rows,"warnings":[],"canApply":true,"targetPath":root.to_string_lossy(),"commit":commit});
    m.previews.clear();
    m.previews.insert(id,json!({"public":public,"skillStamp":skill_stamp(skill),"targetPath":root.to_string_lossy(),"expected":expected,"writes":writes}));
    Ok(public)
}

fn preview(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let skill = m.skill(text(req, "skillId")?)?;
    writable(&skill)?;
    let selected = checked_paths(strings(&req["files"])?)?;
    let snap = pinned(m, &skill, req)?;
    let entries = safe_entries(&snap.skill_files(remote_path(&skill)?)?);
    preview_with_reader(m, &skill, &selected, &snap.commit, &entries, |entry| {
        snap.read_entry(entry)
    })
}

fn apply(m: &mut Manager, req: &Value) -> Result<Value, String> {
    apply_kind(m, req, "attachments")
}

pub(super) fn apply_kind(m: &mut Manager, req: &Value, kind: &str) -> Result<Value, String> {
    let preview = consume_preview(m, req, kind)?;
    let skill = m.skill(text(&preview["public"], "skillId")?)?;
    writable(&skill)?;
    if preview["skillStamp"] != skill_stamp(&skill) {
        return Err("Skill configuration changed after preview; preview again".into());
    }
    let root = local_root(&skill)?;
    if root != Path::new(text(&preview, "targetPath")?) {
        return Err("Attachment target changed after preview".into());
    }
    let expected = mapping(&preview["expected"]);
    let encoded = mapping(&preview["writes"]);
    let mut writes = BTreeMap::new();
    for (path, bytes) in encoded {
        if fs::symlink_metadata(local::safe_join(&root, &path)?).is_ok() {
            return Err(format!(
                "Attachment appeared after preview; no file will be overwritten: {path}"
            ));
        }
        let decoded = STANDARD
            .decode(bytes)
            .map_err(|_| "Invalid internal attachment preview")?;
        writes.insert(path, decoded);
    }
    transaction::apply_in(&m.storage.path("recovery")?, &root, &expected, &writes, &[])?;
    // Adding optional documents is not a runtime update. The next explicit
    // content check may discover extras, but no baseline or status is fabricated.
    Ok(m.snapshot())
}

pub(super) fn handle(m: &mut Manager, req: &Value) -> Result<Option<Value>, String> {
    Ok(Some(match req["action"].as_str().unwrap_or("") {
        "get_files" if req["includeRemote"] == true => get_files(m, req)?,
        "read_file" if req["source"] == "github" => read_file(m, req)?,
        "export" => export(m, req)?,
        "preview_attachments" => preview(m, req)?,
        "apply_attachments" => apply(m, req)?,
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    const COMMIT: &str = "1234567890123456789012345678901234567890";

    fn fixture() -> (tempfile::TempDir, Manager, String, PathBuf) {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/attachment-tests");
        fs::create_dir_all(&parent).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("fixture-")
            .tempdir_in(parent)
            .unwrap();
        let root = temp.path().join("skill");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: synthetic\ndescription: Synthetic test\n---\nUse this skill.\n",
        )
        .unwrap();
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        local::handle(
            &mut manager,
            &json!({"action":"add_local","paths":[root.to_string_lossy()]}),
        )
        .unwrap();
        let id = manager.state.skills[0]["id"].as_str().unwrap().to_owned();
        local::handle(
            &mut manager,
            &json!({"action":"set_runtime_files","skillId":id,"files":["SKILL.md"]}),
        )
        .unwrap();
        let root = local::canonical(&root).unwrap();
        (temp, manager, id, root)
    }

    fn entries(paths: &[(&str, &str, &[u8])]) -> BTreeMap<String, Entry> {
        paths
            .iter()
            .map(|(path, source, bytes)| {
                (
                    path.to_string(),
                    Entry {
                        sha: "a".repeat(40),
                        mode: "100644".into(),
                        size: bytes.len() as u64,
                        source: source.to_string(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn merged_inventory_distinguishes_locations_keeps_local_runtime_and_hash_algorithms() {
        let (_temp, mut manager, id, root) = fixture();
        fs::write(root.join("README.md"), b"local notes").unwrap();
        let local = local::handle(&mut manager, &json!({"action":"get_files","skillId":id}))
            .unwrap()
            .unwrap();
        let remote = entries(&[
            (
                "SKILL.md",
                "skills/synthetic/SKILL.md",
                b"other instructions",
            ),
            ("README.md", "skills/synthetic/README.md", b"remote notes"),
            (
                "references/optional.md",
                "skills/synthetic/references/optional.md",
                b"extra",
            ),
        ]);
        let merged = merge_inventory(local, &remote);
        let files = merged["files"].as_array().unwrap();
        assert_eq!(files.len(), 3);
        let both = files.iter().find(|row| row["path"] == "README.md").unwrap();
        assert_eq!(both["localAvailable"], true);
        assert_eq!(both["remoteAvailable"], true);
        assert_eq!(both["localHash"].as_str().unwrap().len(), 64);
        assert_eq!(both["remoteHash"].as_str().unwrap().len(), 40);
        assert!(both.get("contentDifferent").is_none());
        assert_eq!(merged["requiredFiles"], json!(["SKILL.md"]));
        assert!(paths(&merged["optionalFiles"]).contains(&"references/optional.md".into()));
        assert_eq!(
            files
                .iter()
                .find(|row| row["path"] == "references/optional.md")
                .unwrap()["runtime"],
            false
        );
    }

    #[test]
    fn merged_local_only_and_failed_remote_reads_preserve_files_and_manager_state() {
        let (_temp, mut manager, id, _) = fixture();
        let before = manager.snapshot();
        let request = json!({"action":"get_files","skillId":id,"includeRemote":true});
        let result = handle(&mut manager, &request).unwrap().unwrap();
        assert_eq!(result["remoteStatus"], "not_linked");
        assert_eq!(result["files"][0]["localAvailable"], true);
        let after = manager.snapshot();
        let mut expected = before.clone();
        expected["skills"][0]["runtimeAudit"] = after["skills"][0]["runtimeAudit"].clone();
        expected["stateRevision"] = json!(before["stateRevision"].as_u64().unwrap() + 1);
        assert_eq!(
            after, expected,
            "The first read may save only its missing static evidence"
        );
        assert_eq!(
            after["skills"][0]["runtimeAudit"]["inventoryHashes"],
            after["skills"][0]["localHashes"]
        );
        manager.state.skills[0]["sourceId"] = json!("synthetic-source");
        manager.state.skills[0]["repo"] = json!("owner/skill");
        manager
            .state
            .sources
            .push(json!({"id":"synthetic-source","repo":"different/repo"}));
        let before = manager.snapshot();
        let result = handle(&mut manager, &request).unwrap().unwrap();
        assert_eq!(result["remoteStatus"], "error");
        assert!(result["remoteError"]
            .as_str()
            .unwrap()
            .contains("inconsistent"));
        assert_eq!(result["files"].as_array().unwrap().len(), 1);
        assert_eq!(result["runtimeConfirmed"], true);
        assert_eq!(manager.snapshot(), before);
    }

    #[test]
    fn export_prefers_local_customizations_streams_large_assets_and_pins_remote_license_source() {
        let (temp, manager, id, root) = fixture();
        fs::write(root.join("README.md"), b"user customized notes").unwrap();
        // Larger than BOTH remote limits. Local assets retain existing capacity.
        let asset = vec![7u8; MAX_TOTAL + 1];
        fs::write(root.join("asset.bin"), &asset).unwrap();
        let expected = local::files(&root).unwrap();
        let remote = entries(&[
            ("README.md", "synthetic/README.md", b"upstream notes"),
            ("LICENSE", "LICENSE", b"test license"),
        ]);
        let mut requests = Vec::new();
        let runtime = vec!["SKILL.md".into(), "asset.bin".into()];
        let extras = vec!["README.md".into(), "LICENSE".into()];
        let (data, sources) = collect_export(
            &root,
            &expected,
            &runtime,
            &extras,
            &remote,
            |entry| {
                requests.push(entry.source.clone());
                assert_eq!(entry.source, "LICENSE");
                Ok(b"test license".to_vec())
            },
            Some(COMMIT),
        )
        .unwrap();
        assert_eq!(requests, vec!["LICENSE"]);
        assert!(!data.contains_key("asset.bin"));
        assert_eq!(sources["README.md"]["source"], "local");
        assert_eq!(sources["LICENSE"]["commit"], COMMIT);
        assert_eq!(sources["LICENSE"]["repositoryPath"], "LICENSE");
        let before = manager.snapshot();
        let destination = temp.path().join("synthetic.zip");
        let local_paths = vec!["SKILL.md".into(), "asset.bin".into(), "README.md".into()];
        local::export_package(
            &root,
            destination.to_str().unwrap(),
            &expected,
            &local_paths,
            &data,
            &json!({"files":sources}),
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(fs::File::open(&destination).unwrap()).unwrap();
        assert_eq!(zip.by_name("asset.bin").unwrap().size(), asset.len() as u64);
        let mut content = String::new();
        zip.by_name("README.md")
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert_eq!(content, "user customized notes");
        assert!(zip.by_name("synthetic/SKILL.md").is_err());
        assert_eq!(local::files(&root).unwrap(), expected);
        assert_eq!(manager.snapshot(), before);
        assert!(local::export_package(
            &root,
            destination.to_str().unwrap(),
            &expected,
            &local_paths,
            &data,
            &json!({})
        )
        .is_err());
        assert!(local::export_package(
            &root,
            root.join("bad.zip").to_str().unwrap(),
            &expected,
            &local_paths,
            &data,
            &json!({})
        )
        .is_err());
        assert_eq!(
            manager.skill(&id).unwrap()["runtimeFiles"],
            json!(["SKILL.md"])
        );
    }

    #[test]
    fn export_rejects_unknown_commit_paths_and_incorrect_remote_bytes_without_creating_files() {
        let (_temp, _manager, _id, root) = fixture();
        let expected = local::files(&root).unwrap();
        let remote = entries(&[("README.md", "synthetic/README.md", b"expected")]);
        assert!(collect_export(
            &root,
            &expected,
            &["SKILL.md".into()],
            &["README.md".into()],
            &remote,
            |_| Ok(b"expected".to_vec()),
            None
        )
        .is_err());
        assert!(collect_export(
            &root,
            &expected,
            &["SKILL.md".into()],
            &["README.md".into()],
            &remote,
            |_| Ok(b"truncated".to_vec()),
            Some(COMMIT)
        )
        .is_err());
        assert!(checked_paths(vec!["../escape".into()]).is_err());
        assert!(checked_paths(vec![".git/config".into()]).is_err());
        assert!(checked_paths(vec![PROVENANCE.into()]).is_err());
        assert!(reject_case_aliases(
            ["References/a.md".to_string(), "references/b.md".to_string()].iter()
        )
        .is_err());
        assert_eq!(local::files(&root).unwrap(), expected);
    }

    #[test]
    fn downloaded_attachments_apply_preloaded_immutable_bytes_without_runtime_or_status_changes() {
        let (temp, mut manager, id, root) = fixture();
        let skill = manager.skill(&id).unwrap();
        let before = manager.snapshot();
        let entries = entries(&[
            ("README.md", "synthetic/README.md", b"commit-pinned content"),
            ("LICENSE", "LICENSE", b"source root license"),
        ]);
        let preview = preview_with_reader(
            &mut manager,
            &skill,
            &["README.md".into(), "LICENSE".into()],
            COMMIT,
            &entries,
            |entry| {
                Ok(if entry.source == "LICENSE" {
                    b"source root license".to_vec()
                } else {
                    b"commit-pinned content".to_vec()
                })
            },
        )
        .unwrap();
        assert_eq!(manager.snapshot(), before);
        assert!(!root.join("README.md").exists());
        assert_eq!(preview["kind"], "attachments");
        assert_eq!(preview["commit"], COMMIT);
        // No network is consulted at apply time, even if the branch moves.
        let request = json!({"action":"apply_attachments","previewId":preview["id"]});
        assert_eq!(handle(&mut manager, &request).unwrap().unwrap(), before);
        assert_eq!(
            fs::read(root.join("README.md")).unwrap(),
            b"commit-pinned content"
        );
        assert_eq!(
            fs::read(root.join("LICENSE")).unwrap(),
            b"source root license"
        );
        assert_eq!(manager.snapshot(), before);
        assert!(handle(&mut manager, &request).is_err());
        let journals: Vec<_> = fs::read_dir(temp.path().join("data/recovery"))
            .unwrap()
            .collect();
        assert_eq!(journals.len(), 1);
        let reopened = Manager::new(temp.path().join("data")).unwrap();
        assert_eq!(reopened.snapshot(), before);
    }

    #[test]
    fn attachment_preview_rejects_existing_files_protected_skills_and_late_name_races() {
        let (_temp, mut manager, id, root) = fixture();
        let entries = entries(&[("README.md", "synthetic/README.md", b"notes")]);
        let mut skill = manager.skill(&id).unwrap();
        skill["readOnly"] = json!(true);
        assert!(preview_with_reader(
            &mut manager,
            &skill,
            &["README.md".into()],
            COMMIT,
            &entries,
            |_| panic!("must not read")
        )
        .is_err());
        skill["readOnly"] = json!(false);
        let preview = preview_with_reader(
            &mut manager,
            &skill,
            &["README.md".into()],
            COMMIT,
            &entries,
            |_| Ok(b"notes".to_vec()),
        )
        .unwrap();
        fs::write(root.join("README.md"), b"user file appeared after preview").unwrap();
        let before = manager.snapshot();
        assert!(apply(&mut manager, &json!({"previewId":preview["id"]}))
            .unwrap_err()
            .contains("appeared"));
        assert_eq!(
            fs::read(root.join("README.md")).unwrap(),
            b"user file appeared after preview"
        );
        assert_eq!(manager.snapshot(), before);
        assert!(preview_with_reader(
            &mut manager,
            &skill,
            &["README.md".into()],
            COMMIT,
            &entries,
            |_| panic!("existing names must not download")
        )
        .is_err());
    }

    #[test]
    fn attachment_apply_rejects_unrelated_local_edit_and_selection_change_after_preview() {
        let (_temp, mut manager, id, root) = fixture();
        let entries = entries(&[("README.md", "synthetic/README.md", b"notes")]);
        let skill = manager.skill(&id).unwrap();
        let preview = preview_with_reader(
            &mut manager,
            &skill,
            &["README.md".into()],
            COMMIT,
            &entries,
            |_| Ok(b"notes".to_vec()),
        )
        .unwrap();
        fs::write(root.join("SKILL.md"), b"new local customization").unwrap();
        assert!(apply(&mut manager, &json!({"previewId":preview["id"]}))
            .unwrap_err()
            .contains("changed after preview"));
        assert!(!root.join("README.md").exists());
        let preview = preview_with_reader(
            &mut manager,
            &skill,
            &["README.md".into()],
            COMMIT,
            &entries,
            |_| Ok(b"notes".to_vec()),
        )
        .unwrap();
        manager.state.skills[0]["runtimeConfirmed"] = json!(false);
        assert!(apply(&mut manager, &json!({"previewId":preview["id"]}))
            .unwrap_err()
            .contains("configuration changed"));
        assert!(!root.join("README.md").exists());
    }

    #[test]
    fn legacy_local_inventory_and_read_requests_still_fall_through() {
        let (_temp, mut manager, id, _) = fixture();
        for action in ["get_files", "read_file"] {
            assert!(handle(&mut manager, &json!({"action":action,"skillId":id}))
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn default_export_ignores_old_optional_scope_and_does_not_need_installation_history() {
        let (temp, mut manager, id, root) = fixture();
        fs::write(root.join("README.md"), b"optional notes").unwrap();
        manager.state.skills[0]["runtimeFiles"] = json!(["SKILL.md", "README.md"]);
        manager.state.skills[0]["runtimeConfirmed"] = json!(false);
        let before = manager.snapshot();
        let destination = temp.path().join("synthetic.zip");
        export(
            &manager,
            &json!({"skillId":id,"destination":destination.to_str().unwrap()}),
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(fs::File::open(destination).unwrap()).unwrap();
        assert!(zip.by_name("SKILL.md").is_ok());
        assert!(zip.by_name("README.md").is_err());
        assert_eq!(manager.snapshot(), before);
        assert!(root.join("README.md").exists());
        let destination = temp.path().join("with-notes.zip");
        export(&manager,&json!({"skillId":id,"destination":destination.to_str().unwrap(),"extraFiles":["README.md"]})).unwrap();
        let mut zip = zip::ZipArchive::new(fs::File::open(destination).unwrap()).unwrap();
        assert!(zip.by_name("README.md").is_ok());
    }

    #[test]
    fn export_genuine_missing_dependency_is_actionable_even_with_confirmed_old_scope() {
        let (temp, manager, id, root) = fixture();
        fs::write(root.join("SKILL.md"),"---\nname: synthetic\ndescription: Test\n---\nRead [required](references/required.md).\n").unwrap();
        let destination = temp.path().join("incomplete.zip");
        let error = export(
            &manager,
            &json!({"skillId":id,"destination":destination.to_str().unwrap()}),
        )
        .unwrap_err();
        assert!(error.contains("references/required.md"));
        assert!(!destination.exists());
    }

    #[test]
    fn v020_remote_export_rejects_credential_name_and_original_repository_path_before_read() {
        let (_temp, _manager, _id, root) = fixture();
        let expected = local::files(&root).unwrap();
        for (logical, repository_path) in [
            ("auth.json", "skills/synthetic/auth.json"),
            ("data.json", "shared/.aws/data.json"),
        ] {
            let remote = entries(&[(
                logical,
                repository_path,
                b"synthetic credential placeholder",
            )]);
            let mut reads = 0;
            let error = collect_export(
                &root,
                &expected,
                &["SKILL.md".into()],
                &[logical.into()],
                &remote,
                |_| {
                    reads += 1;
                    Ok(b"synthetic credential placeholder".to_vec())
                },
                Some(COMMIT),
            )
            .unwrap_err();
            assert!(error.contains("凭据"));
            assert_eq!(reads, 0);
            assert!(!error.contains("synthetic credential placeholder"));
        }
    }
}
