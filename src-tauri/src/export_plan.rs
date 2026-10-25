//! One authoritative export inventory and byte manifest, shared by both UI surfaces.
//! Plans and selections live only in memory. Actual writes revalidate the same sources.
use crate::{
    external::{self, Dependency},
    local,
    model::{now, Manager},
    remote,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::Path,
};

const PROVENANCE: &str = "skill-manager-provenance.json";

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field]
        .as_str()
        .ok_or_else(|| format!("Missing {field}"))
}
fn paths(value: &Value) -> Result<Vec<String>, String> {
    value
        .as_array()
        .ok_or("Expected a list of resource IDs")?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "Invalid resource ID".into())
        })
        .collect()
}
fn display(path: &Path) -> String {
    let path = path.to_string_lossy();
    path.strip_prefix(r"\\?\UNC\")
        .map(|tail| format!(r"\\{tail}"))
        .or_else(|| path.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| path.into_owned())
}
fn identity(path: &str) -> String {
    let path = display(Path::new(path)).replace('\\', "/");
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path
    }
}
fn stamp(skill: &Value) -> String {
    hash(&serde_json::to_vec(&json!({"localPath":skill["localPath"],"sourceId":skill["sourceId"],"repo":skill["repo"],"branch":skill["branch"],"remotePath":skill["remotePath"],"runtimeFiles":skill["runtimeFiles"],"runtimeConfirmed":skill["runtimeConfirmed"],"externalBindings":skill["externalBindings"]})).expect("serializable export stamp"))
}
fn resource_id(item: &Dependency, source: &str) -> String {
    let key = if source == "local" {
        format!(
            "local:{}",
            identity(item.canonical_path.as_deref().unwrap_or(&item.target))
        )
    } else if source == "github" {
        format!(
            "github:{}:{}:{}",
            item.remote["repo"].as_str().unwrap_or("").to_lowercase(),
            item.remote["commit"].as_str().unwrap_or(""),
            item.remote["repositoryPath"].as_str().unwrap_or("")
        )
    } else {
        format!("reference:{}", item.id)
    };
    format!("external:{}", hash(key.as_bytes()))
}
fn primary_path(
    item: &Dependency,
    source: &str,
    root: &Path,
    files: &[Value],
    skill: &Value,
    commit: Option<&str>,
) -> Option<String> {
    if matches!(
        item.reference_kind.as_str(),
        "command" | "directory" | "skill_alias"
    ) {
        return None;
    }
    if source == "local" || source == "none" {
        let canonical = item.canonical_path.as_deref()?;
        // Dependencies use display paths; Windows canonical roots use an
        // extended prefix. Compare complete normalized identities, preserving
        // a component boundary and the original package spelling for ZIPs.
        let canonical = identity(canonical);
        let root = identity(&display(root));
        let relative = canonical.strip_prefix(&format!("{}/", root.trim_end_matches('/')))?;
        return files
            .iter()
            .find(|file| {
                file["localAvailable"] == true
                    && file["path"]
                        .as_str()
                        .is_some_and(|path| identity(path) == relative)
            })
            .and_then(|file| file["path"].as_str().map(str::to_owned));
    }
    if item.remote["status"] != "available"
        || item.remote["commit"].as_str() != commit
        || item.remote["repo"] != skill["repo"]
    {
        return None;
    }
    let prefix = skill["remotePath"].as_str().unwrap_or("");
    let full = item.remote["repositoryPath"].as_str()?;
    let path = if prefix.is_empty() {
        full
    } else {
        full.strip_prefix(&format!("{prefix}/"))?
    };
    files
        .iter()
        .any(|file| {
            file["path"] == path
                && file["remoteAvailable"] == true
                && file["localAvailable"] != true
        })
        .then(|| path.to_owned())
}

struct Inventory {
    skill: Value,
    resources: Vec<Value>,
    selected: BTreeSet<String>,
    request: Value,
    issues: Vec<String>,
    remote_status: Value,
    remote_error: Value,
}

fn inventory(m: &mut Manager, req: &Value) -> Result<Inventory, String> {
    let skill_id = text(req, "skillId")?;
    let skill = external::resolution_skill(m, &m.skill(skill_id)?);
    let root = local::canonical(Path::new(
        skill["localPath"]
            .as_str()
            .ok_or("此 Skill 尚无本地运行目录")?,
    ))?;
    let mut request = json!({"action":"get_files","skillId":skill_id,"includeRemote":true});
    if let Some(commit) = req["remoteCommit"].as_str() {
        request["remoteCommit"] = json!(commit);
    }
    let result = remote::attachments::get_files(m, &request)?;
    // Inventory reads may refresh the local static range. Bind this plan to
    // the resulting record so its unchanged scope passes the existing stamp.
    let skill = external::resolution_skill(m, &m.skill(skill_id)?);
    let files = result["files"].as_array().cloned().unwrap_or_default();
    let required: BTreeSet<String> = paths(
        result
            .get("exportRequiredFiles")
            .unwrap_or(&result["requiredFiles"]),
    )
    .unwrap_or_default()
    .into_iter()
    .collect();
    let mut selected: BTreeSet<String> = req
        .get("selectedIds")
        .map(paths)
        .transpose()?
        .unwrap_or_default()
        .into_iter()
        .collect();
    if selected.len() > 2000 {
        return Err("一次最多选择 2000 项资源".into());
    }
    for path in req
        .get("extraFiles")
        .map(paths)
        .transpose()?
        .unwrap_or_default()
    {
        selected.insert(format!("file:{path}"));
    }
    let legacy_external: BTreeSet<String> = req
        .get("externalIds")
        .map(paths)
        .transpose()?
        .unwrap_or_default()
        .into_iter()
        .collect();
    let commit = result["remoteCommit"].as_str();
    let (snap, snapshot_error) = if let Some(commit) = commit {
        match remote::dependency_snapshot(m, &skill, Some(commit)) {
            Ok(snapshot) => (Some(snapshot), None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };
    let references: Vec<Dependency> = result
        .get("externalDependencies")
        .map(|items| {
            serde_json::from_value(items.clone())
                .map_err(|error| format!("外部文件清单格式错误：{error}"))
        })
        .transpose()?
        .unwrap_or_default();
    let mut resources = BTreeMap::<String, Value>::new();
    let mut issues = vec![];
    if let Some(error) = result["exportError"].as_str() {
        issues.push(error.to_owned());
    }
    for file in &files {
        let path = text(file, "path")?;
        let id = format!("file:{path}");
        let needed = required.contains(path);
        let available = file["localAvailable"] == true || file["remoteAvailable"] == true;
        let protected = path == PROVENANCE
            || external::private_resource(Path::new(path))
            || external::private_resource(&local::safe_join(&root, path)?);
        let source = if file["localAvailable"] == true {
            "local"
        } else if file["remoteAvailable"] == true {
            "github"
        } else {
            "none"
        };
        let reason = if path == PROVENANCE {
            "这是保留的导出记录名称，由程序生成"
        } else if protected {
            "凭据或机器私有配置不能加入 ZIP"
        } else if !available {
            "文件暂不可读取"
        } else if needed {
            "必要文件自动包含"
        } else {
            "仅本次选择，不改变运行清单"
        };
        let included = needed || selected.contains(&id);
        if included && (protected || !available) {
            issues.push(format!("{path}：{reason}"));
        }
        resources.insert(id.clone(), json!({"id":id,"label":path,"path":path,"group":if needed{"required"}else{"optional_package"},"included":included,"selectable":!needed&&!protected&&available,"materializable":!protected&&available,"source":source,"size":file["size"],"zipPath":path,"reason":reason,"referenceIds":[],"localPath":if source=="local"{Some(display(&local::safe_join(&root,path)?))}else{None}}));
    }
    for path in &required {
        let id = format!("file:{path}");
        resources.entry(id.clone()).or_insert_with(|| {
            issues.push(format!("必要文件缺失：{path}"));
            json!({"id":id,"label":path,"path":path,"group":"required","included":true,"selectable":false,"materializable":false,"source":"none","zipPath":path,"reason":"必要文件缺失，请先修复来源","referenceIds":[]})
        });
    }
    let mut reference_groups = BTreeMap::new();
    for item in &references {
        let capability = external::export_candidate(&skill, item, snap.as_ref(), false);
        let candidate = capability.as_ref().ok();
        let source = candidate
            .and_then(|item| item.effective_source.as_deref())
            .unwrap_or("none");
        let primary = primary_path(item, source, &root, &files, &skill, commit)
            .map(|path| format!("file:{path}"));
        let id = primary
            .filter(|id| resources.contains_key(id))
            .unwrap_or_else(|| resource_id(item, source));
        reference_groups.insert(item.id.clone(), id.clone());
        if legacy_external.contains(&item.id) {
            if !matches!(item.role.as_str(), "required" | "optional")
                && req["allowEnvironmentFiles"] != true
            {
                return Err("所选外部文件已变化，请重新核对并勾选".into());
            }
            selected.insert(id.clone());
        }
        let needed = item.required;
        let included = needed || selected.contains(&id);
        if let Some(resource) = resources.get_mut(&id) {
            resource["referenceIds"]
                .as_array_mut()
                .expect("reference list")
                .push(json!(item.id));
            if needed {
                resource["group"] = json!("required");
                resource["included"] = json!(true);
                resource["selectable"] = json!(false);
            }
            continue;
        }
        let reason = capability.as_ref().err().cloned().unwrap_or_else(|| {
            if needed {
                "必要外部文件自动包含".into()
            } else {
                "可仅本次携带，保留原用途".into()
            }
        });
        let package = candidate.and_then(|item| item.package_path.clone());
        let size = candidate.and_then(|candidate| {
            if source == "local" {
                candidate
                    .canonical_path
                    .as_deref()
                    .and_then(|path| fs::metadata(path).ok())
                    .map(|meta| meta.len())
            } else {
                item.remote["size"].as_u64()
            }
        });
        if included && capability.is_err() {
            issues.push(format!("{}：{reason}", item.target));
        }
        resources.insert(id.clone(), json!({"id":id,"label":item.canonical_path.as_deref().unwrap_or(&item.target),"path":item.target,"group":if needed{"required"}else if capability.is_ok(){"optional_external"}else{"environment"},"included":included,"selectable":!needed&&capability.is_ok(),"materializable":capability.is_ok(),"source":source,"size":size,"zipPath":package,"reason":reason,"referenceIds":[item.id],"localPath":item.canonical_path}));
    }
    for id in &selected {
        if let Some(resource) = resources.get_mut(id) {
            resource["included"] = json!(true);
            if resource["source"] == "none" {
                issues.push(format!(
                    "{}：{}",
                    resource["label"].as_str().unwrap_or(id),
                    resource["reason"].as_str().unwrap_or("来源不可用")
                ));
            }
        } else {
            issues.push(format!("已选资源发生变化：{id}，请重新核对或取消选择"));
            resources.insert(id.clone(),json!({"id":id,"label":id,"path":id,"group":"optional_external","included":true,"selectable":true,"materializable":false,"source":"none","reason":"原选择已失效，请取消后重新选择当前资源","referenceIds":[]}));
        }
    }
    for id in &legacy_external {
        if !reference_groups.contains_key(id) {
            return Err("所选外部文件已变化，请重新核对并勾选".into());
        }
    }
    let extras: Vec<String> = resources
        .values()
        .filter(|row| row["included"] == true && row["group"] == "optional_package")
        .filter_map(|row| row["zipPath"].as_str().map(str::to_owned))
        .collect();
    let external_ids: Vec<_> = references
        .iter()
        .filter(|item| {
            let Some(id) = reference_groups.get(&item.id) else {
                return false;
            };
            let resource = &resources[id];
            resource["included"] == true
                && (!id.starts_with("file:")
                    || matches!(item.role.as_str(), "required" | "optional"))
        })
        .map(|item| item.id.clone())
        .collect();
    let mut prepared_request = json!({"skillId":skill_id,"extraFiles":extras,"externalIds":external_ids,"allowEnvironmentFiles":true});
    if let Some(commit) = commit {
        prepared_request["remoteCommit"] = json!(commit);
    }
    Ok(Inventory {
        skill,
        resources: resources.into_values().collect(),
        selected,
        request: prepared_request,
        issues,
        remote_status: if snapshot_error.is_some() {
            json!("error")
        } else {
            result["remoteStatus"].clone()
        },
        remote_error: snapshot_error
            .map(Value::String)
            .unwrap_or_else(|| result["remoteError"].clone()),
    })
}

fn selected_hashes(prepared: &remote::attachments::PreparedExport) -> BTreeMap<String, String> {
    prepared
        .local_paths
        .iter()
        .map(|path| (path.clone(), prepared.expected[path].clone()))
        .chain(
            prepared
                .data
                .iter()
                .map(|(path, bytes)| (path.clone(), hash(bytes))),
        )
        .collect()
}
fn proof(prepared: &remote::attachments::PreparedExport) -> Value {
    json!({"expected":prepared.expected,"hashes":selected_hashes(prepared),"external":prepared.provenance["externalResources"],"sources":prepared.provenance["files"]})
}

fn create(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let mut inventory = inventory(m, req)?;
    let prepared = if inventory.issues.is_empty() {
        match remote::attachments::prepare_export(m, &inventory.request) {
            Ok(value) => Some(value),
            Err(error) => {
                inventory.issues.push(error);
                None
            }
        }
    } else {
        None
    };
    let id = m.id();
    let generated = prepared.as_ref().map(|value| {
        serde_json::to_vec_pretty(&value.provenance).expect("serializable provenance")
    });
    let mut manifest:Vec<Value>=inventory.resources.iter().filter(|row|row["included"]==true&&row["zipPath"].is_string()).map(|row|json!({"path":row["zipPath"],"source":row["source"],"size":row["size"],"resourceId":row["id"]})).collect();
    if let Some(prepared) = &prepared {
        manifest.clear();
        for (path, _) in selected_hashes(prepared) {
            let resource = inventory
                .resources
                .iter()
                .find(|resource| resource["included"] == true && resource["zipPath"] == path);
            let source = resource
                .and_then(|row| row["source"].as_str())
                .unwrap_or("local");
            let size = prepared
                .data
                .get(&path)
                .map(|bytes| bytes.len() as u64)
                .or_else(|| {
                    fs::metadata(prepared.root.join(&path))
                        .ok()
                        .map(|meta| meta.len())
                })
                .unwrap_or(0);
            manifest.push(json!({"path":path,"source":source,"size":size,"resourceId":resource.map(|row|row["id"].clone())}));
        }
    }
    inventory.resources.push(json!({"id":"generated:provenance","label":"包内来源记录（自动生成）","path":PROVENANCE,"zipPath":PROVENANCE,"group":"generated","included":true,"selectable":false,"materializable":true,"source":"generated","size":generated.as_ref().map(Vec::len),"reason":"沿用现有导出来源记录，只写入 ZIP","referenceIds":[]}));
    manifest.push(json!({"path":PROVENANCE,"source":"generated","size":generated.as_ref().map(Vec::len),"resourceId":"generated:provenance"}));
    let count = |group: &str, included: bool| {
        inventory
            .resources
            .iter()
            .filter(|row| row["group"] == group && (!included || row["included"] == true))
            .count()
    };
    let summary = json!({"required":count("required",true),"optionalPackageSelected":count("optional_package",true),"optionalPackageTotal":count("optional_package",false),"optionalExternalSelected":count("optional_external",true),"optionalExternalTotal":count("optional_external",false),"generated":1,"total":manifest.len(),"remoteCount":manifest.iter().filter(|row|row["source"]=="github").count(),"bytes":manifest.iter().filter_map(|row|row["size"].as_u64()).sum::<u64>()});
    let public = json!({"id":id,"skillId":inventory.skill["id"],"remoteCommit":inventory.request["remoteCommit"],"remoteStatus":inventory.remote_status,"remoteError":inventory.remote_error,"selectedIds":inventory.selected,"resources":inventory.resources,"manifest":manifest,"summary":summary,"canExport":inventory.issues.is_empty()&&prepared.is_some(),"issues":inventory.issues,"checkedAt":now()});
    let value = json!({"public":public,"kind":"export_plan","createdAt":chrono::Utc::now().timestamp_millis(),"skillStamp":stamp(&inventory.skill),"request":inventory.request,"proof":prepared.as_ref().map(proof),"provenance":prepared.as_ref().map(|value|&value.provenance)});
    m.previews.insert(id, value);
    trim_previews(&mut m.previews);
    Ok(public)
}

pub(crate) fn trim_previews(previews: &mut HashMap<String, Value>) {
    let mut exports: Vec<_> = previews
        .iter()
        .filter(|(_, value)| value["kind"] == "export_plan")
        .map(|(id, value)| (value["createdAt"].as_i64().unwrap_or(0), id.clone()))
        .collect();
    exports.sort();
    let remove = exports.len().saturating_sub(32);
    for (_, id) in exports.into_iter().take(remove) {
        previews.remove(&id);
    }
}

fn write(
    prepared: remote::attachments::PreparedExport,
    destination: &str,
    provenance: &Value,
) -> Result<Value, String> {
    let mut result = local::export_package(
        &prepared.root,
        destination,
        &prepared.expected,
        &prepared.local_paths,
        &prepared.data,
        provenance,
    )?;
    result["externalResources"] = provenance["externalResources"].clone();
    Ok(result)
}

pub(crate) fn legacy_export(m: &Manager, req: &Value) -> Result<Value, String> {
    let skill = m.skill(text(req, "skillId")?)?;
    let root = local::canonical(Path::new(
        skill["localPath"]
            .as_str()
            .ok_or("此 Skill 尚无本地运行目录")?,
    ))?;
    local::export_destination(&root, text(req, "destination")?)?;
    let prepared = remote::attachments::prepare_export(m, req)?;
    let provenance = prepared.provenance.clone();
    write(prepared, text(req, "destination")?, &provenance)
}

pub(crate) fn handle(m: &mut Manager, req: &Value) -> Result<Option<Value>, String> {
    match req["action"].as_str().unwrap_or("") {
        "get_export_plan" => create(m, req).map(Some),
        "export" if req["planId"].is_string() => {
            let current = m.skill(text(req, "skillId")?)?;
            let root = local::canonical(Path::new(
                current["localPath"]
                    .as_str()
                    .ok_or("此 Skill 尚无本地运行目录")?,
            ))?;
            let default_destination = match req.get("destinationMode") {
                None | Some(Value::Null) => false,
                Some(Value::String(mode)) if mode == "custom" => false,
                Some(Value::String(mode)) if mode == "default" => true,
                _ => return Err("请选择默认位置或自定义位置".into()),
            };
            if default_destination {
                crate::storage::default_export_parent(
                    &m.storage,
                    text(req, "destination")?,
                    false,
                )?;
                local::pending_export_destination(&root, text(req, "destination")?)?;
            } else {
                local::export_destination(&root, text(req, "destination")?)?;
            }
            let id = text(req, "planId")?;
            let saved = m
                .previews
                .get(id)
                .cloned()
                .filter(|value| value["kind"] == "export_plan")
                .ok_or("导出清单已过期，请刷新后再导出")?;
            if saved["public"]["skillId"] != req["skillId"] || saved["public"]["canExport"] != true
            {
                return Err("导出清单尚未通过核验".into());
            }
            let skill = external::resolution_skill(m, &m.skill(text(req, "skillId")?)?);
            if saved["skillStamp"] != stamp(&skill) {
                return Err("Skill 来源、用途或文件范围已变化，请刷新导出清单".into());
            }
            let prepared = remote::attachments::prepare_export(m, &saved["request"])?;
            if saved["proof"] != proof(&prepared) {
                return Err("选定文件或来源已变化，请重新核对导出清单".into());
            }
            if default_destination {
                crate::storage::default_export_parent(&m.storage, text(req, "destination")?, true)?;
            }
            let mut result = write(prepared, text(req, "destination")?, &saved["provenance"])?;
            result["planId"] = json!(id);
            result["manifest"] = saved["public"]["manifest"].clone();
            Ok(Some(result))
        }
        "export" => legacy_export(m, req).map(Some),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn fixture() -> (tempfile::TempDir, Manager, String, std::path::PathBuf) {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/export-plan-tests");
        fs::create_dir_all(&parent).unwrap();
        let temp = tempfile::tempdir_in(parent).unwrap();
        let root = temp.path().join("skill");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: plan-test\ndescription: Synthetic\n---\n# Instructions\n",
        )
        .unwrap();
        fs::write(root.join("README.md"), "Optional package note").unwrap();
        let mut manager = Manager::new(temp.path().join("state")).unwrap();
        crate::dispatch(&mut manager, &json!({"action":"add_local","paths":[root]})).unwrap();
        let id = manager.state.skills[0]["id"].as_str().unwrap().to_owned();
        (temp, manager, id, root)
    }
    fn zip_entries(path: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut zip = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
        (0..zip.len())
            .map(|index| {
                let mut file = zip.by_index(index).unwrap();
                let name = file.name().to_owned();
                let mut bytes = vec![];
                file.read_to_end(&mut bytes).unwrap();
                (name, bytes)
            })
            .collect()
    }
    #[test]
    fn destination_query_and_failed_default_export_never_create_output_folders() {
        let (_temp, mut manager, id, root) = fixture();
        let selected = crate::dispatch(
            &mut manager,
            &json!({"action":"storage_resolve_default_path","kind":"export"}),
        )
        .unwrap();
        let directory = std::path::PathBuf::from(selected["path"].as_str().unwrap());
        assert!(!directory.exists());
        assert!(crate::dispatch(
            &mut manager,
            &json!({"action":"storage_resolve_default_path","kind":"state"})
        )
        .is_err());
        let destination = directory.join("plan-test.zip");
        let invalid = json!({"action":"export","skillId":id,"planId":"missing-plan","destinationMode":"default","destination":destination});
        assert!(handle(&mut manager, &invalid).unwrap_err().contains("过期"));
        assert!(!directory.exists());
        let plan = create(&mut manager, &json!({"skillId":id,"selectedIds":[]})).unwrap();
        assert_eq!(plan["canExport"], true);
        fs::write(root.join("SKILL.md"), "changed after preview").unwrap();
        assert!(handle(&mut manager, &json!({"action":"export","skillId":id,"planId":plan["id"],"destinationMode":"default","destination":destination})).is_err());
        assert!(!directory.exists());
    }
    #[test]
    fn explicit_default_export_creates_only_its_selected_parent_after_valid_proof() {
        let (temp, mut manager, id, _root) = fixture();
        let directory = manager.storage.path("export").unwrap();
        let destination = directory.join("plan-test.zip");
        let plan = create(&mut manager, &json!({"skillId":id,"selectedIds":[]})).unwrap();
        assert_eq!(plan["canExport"], true);
        let wrong = temp.path().join("unselected/new/plan-test.zip");
        assert!(handle(&mut manager, &json!({"action":"export","skillId":id,"planId":plan["id"],"destinationMode":"default","destination":wrong})).unwrap_err().contains("默认导出位置已变化"));
        assert!(!wrong.parent().unwrap().exists());
        assert!(!directory.exists());
        let result = handle(&mut manager, &json!({"action":"export","skillId":id,"planId":plan["id"],"destinationMode":"default","destination":destination})).unwrap().unwrap();
        assert_eq!(result["planId"], plan["id"]);
        let entries = zip_entries(&destination);
        assert!(entries.contains_key("SKILL.md"));
        assert!(!entries.contains_key("README.md"));
        assert!(entries.contains_key(PROVENANCE));
        assert!(handle(&mut manager, &json!({"action":"export","skillId":id,"planId":plan["id"],"destinationMode":"default","destination":destination})).is_err());
        assert_eq!(zip_entries(&destination), entries);
    }
    fn add_reference(root: &Path, paths: &[&Path]) {
        let mut content = fs::read_to_string(root.join("SKILL.md")).unwrap();
        for path in paths {
            content.push_str(&format!("\n共享环境规则：`{}`\n", display(path)));
        }
        fs::write(root.join("SKILL.md"), content).unwrap();
    }

    #[test]
    fn v023_plan_defaults_and_selected_manifest_are_the_actual_zip_entries() {
        let (temp, mut m, id, root) = fixture();
        let plan = crate::dispatch(
            &mut m,
            &json!({"action":"get_export_plan","skillId":id,"selectedIds":[]}),
        )
        .unwrap();
        assert_eq!(plan["canExport"], true);
        assert_eq!(plan["summary"]["required"], 1);
        assert_eq!(plan["summary"]["optionalPackageTotal"], 1);
        assert_eq!(plan["summary"]["total"], 2);
        let default = temp.path().join("default.zip");
        crate::dispatch(
            &mut m,
            &json!({"action":"export","skillId":id,"planId":plan["id"],"destination":default}),
        )
        .unwrap();
        assert_eq!(
            zip_entries(&default).keys().cloned().collect::<Vec<_>>(),
            vec!["SKILL.md", PROVENANCE]
        );
        let plan = crate::dispatch(
            &mut m,
            &json!({"action":"get_export_plan","skillId":id,"selectedIds":["file:README.md"]}),
        )
        .unwrap();
        let destination = temp.path().join("selected.zip");
        crate::dispatch(
            &mut m,
            &json!({"action":"export","skillId":id,"planId":plan["id"],"destination":destination}),
        )
        .unwrap();
        let entries = zip_entries(&destination);
        assert_eq!(
            entries.keys().cloned().collect::<BTreeSet<_>>(),
            plan["manifest"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["path"].as_str().unwrap().to_owned())
                .collect()
        );
        assert_eq!(
            entries.values().map(Vec::len).sum::<usize>() as u64,
            plan["summary"]["bytes"].as_u64().unwrap()
        );
        assert_eq!(entries["README.md"], b"Optional package note");
        assert_eq!(local::files(&root).unwrap().len(), 2);
    }

    #[test]
    fn v023_absolute_package_references_share_required_file_identity_on_windows() {
        let (temp, mut manager, id, root) = fixture();
        fs::create_dir(root.join("scripts")).unwrap();
        fs::create_dir(root.join("references")).unwrap();
        let script = root.join("scripts/Run.ps1");
        fs::write(&script, "# Synthetic script, never executed\n").unwrap();
        let path = display(&script).replace('\\', "/");
        let alternate = if cfg!(windows) {
            path.to_lowercase().replace('/', "\\")
        } else {
            path.clone()
        };
        fs::OpenOptions::new().append(true).open(root.join("SKILL.md")).unwrap().write_all(format!("\nUse `scripts/Run.ps1` and read `references/usage.md`.\n本包环境入口：`{path}`。\n").as_bytes()).unwrap();
        fs::write(
            root.join("references/usage.md"),
            format!("本包环境入口：`{alternate}`。\n"),
        )
        .unwrap();
        let before = local::files(&root).unwrap();
        let plan = crate::dispatch(
            &mut manager,
            &json!({"action":"get_export_plan","skillId":id,"selectedIds":[]}),
        )
        .unwrap();
        assert_eq!(plan["canExport"], true, "{plan}");
        assert_eq!(plan["summary"]["optionalExternalTotal"], 0, "{plan}");
        let script_row = plan["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "file:scripts/Run.ps1")
            .unwrap();
        assert_eq!(script_row["group"], "required");
        assert_eq!(script_row["referenceIds"].as_array().unwrap().len(), 2);
        assert_eq!(
            plan["resources"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["zipPath"] == "scripts/Run.ps1")
                .count(),
            1
        );
        let destination = temp.path().join("package-alias.zip");
        crate::dispatch(
            &mut manager,
            &json!({"action":"export","skillId":id,"planId":plan["id"],"destination":destination}),
        )
        .unwrap();
        let entries = zip_entries(&destination);
        assert!(entries.contains_key("scripts/Run.ps1"));
        assert!(!entries.keys().any(|path| path.starts_with("external/")));
        assert_eq!(local::files(&root).unwrap(), before);
    }

    #[test]
    fn v023_shared_environment_files_can_be_selected_without_changing_roles_or_originals() {
        let (temp, mut m, id, root) = fixture();
        let shared = temp.path().join("AGENTS.md");
        fs::write(&shared, "Synthetic shared rules").unwrap();
        add_reference(&root, &[&shared]);
        let before = m.skill(&id).unwrap();
        let originals = local::files(&root).unwrap();
        let plan = crate::dispatch(
            &mut m,
            &json!({"action":"get_export_plan","skillId":id,"selectedIds":[]}),
        )
        .unwrap();
        let resource = plan["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["group"] == "optional_external")
            .expect("shared rules are one-time optional");
        assert_eq!(resource["included"], false);
        assert_eq!(resource["selectable"], true);
        assert_eq!(plan["canExport"], true);
        let selected = crate::dispatch(
            &mut m,
            &json!({"action":"get_export_plan","skillId":id,"selectedIds":[resource["id"]]}),
        )
        .unwrap();
        assert_eq!(selected["canExport"], true, "{selected}");
        assert_eq!(selected["summary"]["optionalExternalSelected"], 1);
        let destination = temp.path().join("shared.zip");
        crate::dispatch(&mut m,&json!({"action":"export","skillId":id,"planId":selected["id"],"destination":destination})).unwrap();
        let entries = zip_entries(&destination);
        let path = resource["zipPath"].as_str().unwrap();
        assert_eq!(entries[path], b"Synthetic shared rules");
        let after = m.skill(&id).unwrap();
        let mut expected = before;
        for field in ["runtimeAudit", "runtimeIssues", "runtimeWarnings"] {
            expected[field] = after[field].clone();
        }
        assert_eq!(
            after, expected,
            "Only newly detected static evidence may change"
        );
        assert_eq!(after["runtimeAudit"]["inventoryHashes"], json!(originals));
        assert_eq!(local::files(&root).unwrap(), originals);
        assert_eq!(fs::read(&shared).unwrap(), b"Synthetic shared rules");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        let provenance: Value = serde_json::from_slice(&entries[PROVENANCE]).unwrap();
        assert_eq!(
            provenance["externalResources"]["files"][0]["role"],
            "environment"
        );
    }

    #[test]
    fn v023_duplicate_references_group_one_file_and_same_name_different_files_stay_distinct() {
        let (temp, mut m, id, root) = fixture();
        let a = temp.path().join("one");
        let b = temp.path().join("two");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let a = a.join("AGENTS.md");
        let b = b.join("AGENTS.md");
        fs::write(&a, "same bytes at separate paths").unwrap();
        fs::write(&b, "same bytes at separate paths").unwrap();
        add_reference(&root, &[&a, &b]);
        fs::create_dir(root.join("references")).unwrap();
        fs::write(
            root.join("references/usage.md"),
            format!("共享规则：`{}`", display(&a)),
        )
        .unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(root.join("SKILL.md"))
            .unwrap()
            .write_all(b"\nRead `references/usage.md`.\n")
            .unwrap();
        let plan = crate::dispatch(
            &mut m,
            &json!({"action":"get_export_plan","skillId":id,"selectedIds":[]}),
        )
        .unwrap();
        let external: Vec<_> = plan["resources"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["group"] == "optional_external")
            .collect();
        assert_eq!(external.len(), 2, "{plan}");
        assert_ne!(external[0]["id"], external[1]["id"]);
        assert_ne!(external[0]["zipPath"], external[1]["zipPath"]);
        assert!(external
            .iter()
            .any(|row| row["referenceIds"].as_array().unwrap().len() == 2));
        let ids: Vec<_> = external.iter().map(|row| row["id"].clone()).collect();
        let selected = crate::dispatch(
            &mut m,
            &json!({"action":"get_export_plan","skillId":id,"selectedIds":ids}),
        )
        .unwrap();
        assert_eq!(selected["canExport"], true, "{selected}");
        assert_eq!(selected["summary"]["optionalExternalSelected"], 2);
        let destination = temp.path().join("both.zip");
        crate::dispatch(&mut m,&json!({"action":"export","skillId":id,"planId":selected["id"],"destination":destination})).unwrap();
        assert_eq!(
            zip_entries(&destination)
                .keys()
                .filter(|path| path.starts_with("external/"))
                .count(),
            2
        );
    }

    #[test]
    fn v023_file_changes_and_unknown_selected_resources_block_before_creating_zip() {
        let (temp, mut m, id, root) = fixture();
        let shared = temp.path().join("AGENTS.md");
        fs::write(&shared, "before").unwrap();
        add_reference(&root, &[&shared]);
        let base = crate::dispatch(
            &mut m,
            &json!({"action":"get_export_plan","skillId":id,"selectedIds":[]}),
        )
        .unwrap();
        let resource = base["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["group"] == "optional_external")
            .unwrap()["id"]
            .clone();
        let plan = crate::dispatch(
            &mut m,
            &json!({"action":"get_export_plan","skillId":id,"selectedIds":[resource]}),
        )
        .unwrap();
        fs::write(&shared, "after").unwrap();
        let destination = temp.path().join("changed.zip");
        assert!(crate::dispatch(
            &mut m,
            &json!({"action":"export","skillId":id,"planId":plan["id"],"destination":destination})
        )
        .unwrap_err()
        .contains("变化"));
        assert!(!destination.exists());
        let ghost = crate::dispatch(
            &mut m,
            &json!({"action":"get_export_plan","skillId":id,"selectedIds":["external:removed"]}),
        )
        .unwrap();
        assert_eq!(ghost["canExport"], false);
        assert!(ghost["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "external:removed" && row["included"] == true));
        assert!(!ghost["manifest"].as_array().unwrap().is_empty());
    }

    #[test]
    fn v023_commands_directories_and_credentials_remain_explicitly_ineligible() {
        let (temp, mut m, id, root) = fixture();
        let directory = temp.path().join("folder");
        fs::create_dir(&directory).unwrap();
        let secret = temp.path().join("auth.json");
        fs::write(&secret, "synthetic credential placeholder").unwrap();
        add_reference(&root, &[&directory, &secret]);
        fs::OpenOptions::new()
            .append(true)
            .open(root.join("SKILL.md"))
            .unwrap()
            .write_all(b"\nUse `/approve`.\n")
            .unwrap();
        let before = m.skill(&id).unwrap();
        let plan = crate::dispatch(
            &mut m,
            &json!({"action":"get_export_plan","skillId":id,"selectedIds":[]}),
        )
        .unwrap();
        let environments: Vec<_> = plan["resources"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["group"] == "environment")
            .collect();
        assert!(environments.len() >= 3, "{plan}");
        assert!(environments
            .iter()
            .all(|row| row["selectable"] == false && row["included"] == false));
        let ids: Vec<_> = environments.iter().map(|row| row["id"].clone()).collect();
        let blocked = crate::dispatch(
            &mut m,
            &json!({"action":"get_export_plan","skillId":id,"selectedIds":ids}),
        )
        .unwrap();
        assert_eq!(blocked["canExport"], false);
        let after = m.skill(&id).unwrap();
        let mut expected = before;
        for field in ["runtimeAudit", "runtimeIssues", "runtimeWarnings"] {
            expected[field] = after[field].clone();
        }
        assert_eq!(
            after, expected,
            "Resource exclusions cannot change scope, roles, source or history"
        );
        assert_eq!(
            after["runtimeAudit"]["inventoryHashes"],
            json!(local::files(&root).unwrap())
        );
    }

    #[test]
    fn v023_preview_retention_bounds_only_export_plans() {
        let mut previews = HashMap::new();
        previews.insert("update".into(), json!({"kind":"update"}));
        for index in 0..50 {
            previews.insert(
                format!("export-{index:02}"),
                json!({"kind":"export_plan","createdAt":index}),
            );
        }
        trim_previews(&mut previews);
        assert_eq!(previews.len(), 33);
        assert!(previews.contains_key("update"));
        assert!(!previews.contains_key("export-00"));
        assert!(previews.contains_key("export-49"));
    }
}
