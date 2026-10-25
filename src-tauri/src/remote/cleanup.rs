//! Optional files are removed only after the exact submitted commit is read back.
//! Durable receipts contain hashes and immutable remote locations, never another Skill copy.
use super::*;
use std::io::Write;

fn receipt_dir(m: &Manager) -> Result<PathBuf, String> {
    let directory = m.storage.path("receipts")?;
    if directory.exists() {
        transaction::verify_root(&directory)?;
    }
    Ok(directory)
}

fn receipt_path(m: &Manager, id: &str) -> Result<PathBuf, String> {
    uuid::Uuid::parse_str(id).map_err(|_| "Invalid upload receipt")?;
    Ok(receipt_dir(m)?.join(format!("{id}.json")))
}

fn persist_new(path: &Path, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "Cannot create a durable upload receipt; optional files were retained")?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot persist the upload receipt; optional files were retained".into())
}

pub(super) fn planned(
    skill: &Value,
    selected: &[String],
    enabled: bool,
) -> Result<Vec<Value>, String> {
    if !enabled {
        return Ok(vec![]);
    }
    writable(skill)?;
    let root = local_root(skill)?;
    // An uncertain runtime closure preserves everything. The upload may still proceed.
    let required = match local::minimal_runtime_files(&root) {
        Ok(files) => files.into_iter().collect::<BTreeSet<_>>(),
        Err(_) => return Ok(vec![]),
    };
    let all = local::files(&root)?;
    selected
        .iter()
        .filter(|path| !required.contains(*path))
        .filter_map(|path| {
            all.get(path).map(|digest| {
                local::safe_join(&root, path).map(|absolute| {
                    json!({"path":path,"absolutePath":absolute.to_string_lossy(),"hash":digest,
                "reason":"本次已选可选文件；上传并独立校验成功后仅在 GitHub 保留"})
                })
            })
        })
        .collect()
}

fn retained(planned: &[Value], reason: &str) -> Value {
    json!({"removed":[],"retained":planned.iter().map(|row|json!({"path":row["path"],"absolutePath":row["absolutePath"],"reason":reason})).collect::<Vec<_>>()})
}

/// Keep the file object and its directory chain stable between hashing and
/// deletion. No delete disposition is requested until every check succeeds.
fn remove_verified_file(
    root: &Path,
    relative: &str,
    digest: &str,
    before_delete: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let absolute = local::safe_join(root, relative)?;
    #[cfg(windows)]
    {
        use std::{
            ffi::c_void,
            io::Read,
            os::windows::{
                fs::{MetadataExt, OpenOptionsExt},
                io::AsRawHandle,
            },
        };
        const OPEN_REPARSE: u32 = 0x0020_0000;
        const BACKUP_SEMANTICS: u32 = 0x0200_0000;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetFinalPathNameByHandleW(
                file: *mut c_void,
                path: *mut u16,
                length: u32,
                flags: u32,
            ) -> u32;
            fn SetFileInformationByHandle(
                file: *mut c_void,
                class: i32,
                information: *const c_void,
                size: u32,
            ) -> i32;
        }
        let mut directories = Vec::new();
        let parents = absolute
            .parent()
            .ok_or("文件没有父目录")?
            .ancestors()
            .filter(|p| p.is_absolute())
            .collect::<Vec<_>>();
        for parent in parents.into_iter().rev() {
            let directory = fs::OpenOptions::new()
                .access_mode(0x80)
                .share_mode(3)
                .custom_flags(OPEN_REPARSE | BACKUP_SEMANTICS)
                .open(parent)
                .map_err(|_| "目录正在变化或无法锁定，已保留文件")?;
            let metadata = directory
                .metadata()
                .map_err(|_| "无法核对目录，已保留文件")?;
            if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
                return Err("目录包含链接或重定向，已保留文件".into());
            }
            directories.push(directory);
        }
        // DELETE access plus no shared WRITE/DELETE prevents concurrent edits,
        // replacement, and rename. Shared reads allow dependency inspection.
        let mut file = fs::OpenOptions::new()
            .access_mode(0x8000_0000 | 0x0001_0000)
            .share_mode(1)
            .custom_flags(OPEN_REPARSE)
            .open(&absolute)
            .map_err(|_| "文件正在使用或无法锁定，已保留")?;
        let metadata = file.metadata().map_err(|_| "文件无法检查，已保留")?;
        if !metadata.is_file()
            || metadata.file_attributes() & 0x400 != 0
            || metadata.len() > MAX_FILE as u64
        {
            return Err("目标不是普通可核对文件，已保留".into());
        }
        let required =
            unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), std::ptr::null_mut(), 0, 0) };
        if required == 0 {
            return Err("无法核对已打开文件的真实路径，已保留".into());
        }
        let mut buffer = vec![0u16; required as usize + 1];
        let length = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                0,
            )
        };
        if length == 0 || length as usize >= buffer.len() {
            return Err("已打开文件的真实路径不可确认，已保留".into());
        }
        let actual =
            String::from_utf16(&buffer[..length as usize]).map_err(|_| "文件真实路径无法读取")?;
        let key = |path: &str| path.replace('/', "\\").to_lowercase();
        if key(&actual) != key(&absolute.to_string_lossy()) {
            return Err("文件真实目标与预览不一致，已保留".into());
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| "文件读取未完成，已保留")?;
        if hash(&bytes) != digest {
            return Err("上传期间本地文件发生变化，已保留".into());
        }
        before_delete()?;
        let disposition = 1u8;
        // FileDispositionInfo = 4, FILE_DISPOSITION_INFO contains one BOOLEAN.
        if unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                4,
                &disposition as *const u8 as *const c_void,
                1,
            )
        } == 0
        {
            return Err("本地删除未完成，文件已保留".into());
        }
        drop(file);
        drop(directories);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let metadata = fs::symlink_metadata(&absolute).map_err(|_| "本地文件不可读取，已保留")?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("目标不再是普通文件，已保留".into());
        }
        if hash(&fs::read(&absolute).map_err(|_| "本地文件不可读取，已保留")?) != digest
        {
            return Err("上传期间本地文件发生变化，已保留".into());
        }
        before_delete()?;
        // Automatic trimming is Windows-only until another platform provides
        // an equivalent object-bound deletion primitive.
        Err("当前平台尚未提供原子附件精简；文件已保留".into())
    }
}

/// All remote reads finish before the first local deletion. A failed/uncertain
/// commit read leaves every local optional file in place.
fn verified_cleanup(
    m: &Manager,
    skill: &Value,
    commit: &str,
    planned: &[Value],
    mut read: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Value {
    if planned.is_empty() {
        return json!({"removed":[],"retained":[]});
    }
    let check = (|| -> Result<(PathBuf, BTreeSet<String>), String> {
        writable(skill)?;
        let root = local_root(skill)?;
        let required = local::minimal_runtime_files(&root)?.into_iter().collect();
        for row in planned {
            let path = text(row, "path")?;
            github::validate_relative(path)?;
            if hash(&read(path)?) != text(row, "hash")? {
                return Err(format!("固定提交中的文件校验不一致：{path}"));
            }
        }
        Ok((root, required))
    })();
    let (root, required) = match check {
        Ok(v) => v,
        Err(e) => {
            return retained(
                planned,
                &format!("上传已成功；精简前校验未完成，已保留：{e}"),
            )
        }
    };
    let id = m.id();
    let receipt = json!({"formatVersion":1,"id":id,"skillId":skill["id"],"createdAt":now(),
        "repo":skill["repo"],"branch":skill["branch"],"remotePath":skill["remotePath"],
        "commit":commit,"localPath":root.to_string_lossy(),"files":planned});
    let write = (|| -> Result<(), String> {
        let directory = receipt_dir(m)?;
        fs::create_dir_all(&directory).map_err(|_| "Cannot create upload receipt directory")?;
        transaction::verify_root(&directory)?;
        persist_new(&receipt_path(m, &id)?, &receipt)
    })();
    if let Err(error) = write {
        return retained(planned, &error);
    }
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    for row in planned {
        let result = (|| -> Result<(), String> {
            let path = text(row, "path")?;
            if required.contains(path) {
                return Err("该文件现在属于必要运行文件，已保留".into());
            }
            let current_root = local_root(skill)?;
            if current_root != root {
                return Err("本地根目录已变化，已保留".into());
            }
            let absolute = local::safe_join(&root, path)?;
            if absolute.to_string_lossy() != text(row, "absolutePath")? {
                return Err("文件目标与预览不一致，已保留".into());
            }
            remove_verified_file(&root, path, text(row, "hash")?, || {
                // Re-evaluate while the candidate cannot be edited or replaced.
                if local::minimal_runtime_files(&root)?
                    .iter()
                    .any(|p| p == path)
                {
                    return Err("该文件现在属于必要运行文件，已保留".into());
                }
                local_root(skill)?;
                local::safe_join(&root, path)?;
                Ok(())
            })
        })();
        match result {
            Ok(()) => removed.push(json!({"path":row["path"],"absolutePath":row["absolutePath"]})),
            Err(reason) => kept.push(
                json!({"path":row["path"],"absolutePath":row["absolutePath"],"reason":reason}),
            ),
        }
    }
    let result = json!({"receiptId":id,"removed":removed,"retained":kept});
    // The immutable initial receipt is already sufficient to recover even if
    // the application stops before this final outcome is persisted.
    if let Ok(directory) = receipt_dir(m) {
        let _ = persist_new(&directory.join(format!("{id}.result.json")), &result);
    }
    result
}

pub(super) fn after_upload(
    m: &mut Manager,
    skill: &Value,
    commit: &str,
    planned: &[Value],
) -> Value {
    if planned.is_empty() {
        return json!({"removed":[],"retained":[]});
    }
    let snap = match Snapshot::at_commit_fresh(repo(skill).unwrap_or(""), commit) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return retained(
                planned,
                &format!("固定提交读取失败，已保留本地文件：{error}"),
            )
        }
    };
    let mut result = verified_cleanup(m, skill, commit, planned, |path| {
        snap.read(&join(remote_path(skill)?, path))
    });
    let removed = result["removed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["path"].as_str().map(str::to_owned))
        .collect::<BTreeSet<_>>();
    if !removed.is_empty() {
        let mut updated = skill.clone();
        let runtime = paths(&updated["runtimeFiles"])
            .into_iter()
            .filter(|path| !removed.contains(path))
            .collect::<Vec<_>>();
        updated["runtimeFiles"] = json!(runtime);
        for key in ["baselineHashes", "remoteHashes", "localHashes"] {
            if let Some(object) = updated[key].as_object_mut() {
                object.retain(|path, _| !removed.contains(path));
            }
        }
        let remote_pending = skill["remoteCheckRequired"] == true
            || skill["comparisonPending"] == true
            || !skill["error"].is_null();
        if remote_pending {
            // An immutable receipt proves only C. A prior read already observed
            // that the branch moved (or failed), so it cannot be cleared by C.
            updated["comparisonScope"] = json!(runtime);
        } else {
            association::reset_observation(&mut updated, &runtime);
        }
        if let Ok(audit) = local::runtime_audit(&updated) {
            if audit["canAutoConfirm"] != true && updated["runtimeConfirmationKind"] == "automatic"
            {
                updated["runtimeConfirmed"] = json!(false);
            }
            updated["runtimeAudit"] = audit;
        }
        // Recompute status against the same immutable submission after shrinking
        // the operational scope. This never changes historical installedCommit.
        if !remote_pending {
            let verified = verify_skill(&updated, &snap).and_then(|verified| {
                if snap.api.head(text(skill, "branch")?)? != commit {
                    return Err("上传后的分支已有新提交，请刷新核对最新内容".into());
                }
                Ok(verified)
            });
            match verified {
                Ok(verified) => record_verification(&mut updated, commit, snap.private, verified),
                Err(error) => {
                    updated["remoteCheckRequired"] = json!(true);
                    updated["status"] = json!("remote_check_required");
                    updated["comparisonPending"] = json!(true);
                    updated["error"] = json!(error);
                    updated["comparisonNote"] =
                        json!("附件已按上传提交安全精简；当前分支内容待重新核对");
                    updated["remoteHashes"] = Value::Null;
                }
            }
        }
        m.upsert_skill(updated);
        if let Err(error) = m.save() {
            result["recordError"] = json!(format!(
                "附件已精简且恢复记录已保存；管理范围保存失败，请刷新核对：{error}"
            ));
        }
    }
    result
}

fn load(m: &Manager, skill: &Value, id: &str) -> Result<Value, String> {
    let receipt: Value = serde_json::from_slice(
        &fs::read(receipt_path(m, id)?).map_err(|_| "Upload receipt is unavailable")?,
    )
    .map_err(|_| "Upload receipt is invalid")?;
    if receipt["skillId"] != skill["id"]
        || receipt["repo"] != skill["repo"]
        || receipt["remotePath"] != skill["remotePath"]
    {
        return Err("Upload receipt belongs to a different Skill source".into());
    }
    if local_root(skill)?.to_string_lossy() != text(&receipt, "localPath")? {
        return Err("Skill local directory changed since the upload".into());
    }
    github::sha(&receipt["commit"])?;
    Ok(receipt)
}

fn list(m: &Manager, req: &Value) -> Result<Value, String> {
    let skill = m.skill(text(req, "skillId")?)?;
    let directory = receipt_dir(m)?;
    let mut receipts = Vec::new();
    if directory.exists() {
        for entry in fs::read_dir(directory).map_err(|_| "Cannot list upload receipts")? {
            let entry = entry.map_err(|_| "Cannot read upload receipt entry")?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
                continue;
            };
            if uuid::Uuid::parse_str(id).is_err() {
                continue;
            }
            let Ok(mut receipt) = load(m, &skill, id) else {
                continue;
            };
            let root = local_root(&skill)?;
            for row in receipt["files"]
                .as_array_mut()
                .ok_or("Invalid upload receipt file list")?
            {
                let path = text(row, "path")?;
                row["missingLocally"] = json!(!local::safe_join(&root, path)?.exists());
            }
            receipts.push(receipt);
        }
    }
    receipts.sort_by(|a, b| b["createdAt"].as_str().cmp(&a["createdAt"].as_str()));
    Ok(json!({"receipts":receipts}))
}

fn preview_restore(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let skill = m.skill(text(req, "skillId")?)?;
    writable(&skill)?;
    let receipt = load(m, &skill, text(req, "receiptId")?)?;
    let root = local_root(&skill)?;
    let selected = receipt["files"]
        .as_array()
        .ok_or("Invalid upload receipt")?
        .iter()
        .filter_map(|row| row["path"].as_str())
        .filter(|path| local::safe_join(&root, path).is_ok_and(|absolute| !absolute.exists()))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err("没有缺失附件需要恢复；已有本地文件保持原样".into());
    }
    let snap = Snapshot::at_commit(repo(&skill)?, text(&receipt, "commit")?)?;
    let entries = snap.skill_files(remote_path(&skill)?)?;
    let expected: BTreeMap<String, String> = receipt["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            Ok((
                join(remote_path(&skill)?, text(row, "path")?),
                text(row, "hash")?.to_owned(),
            ))
        })
        .collect::<Result<_, String>>()?;
    let public =
        attachments::preview_with_reader(m, &skill, &selected, &snap.commit, &entries, |entry| {
            let bytes = snap.read_entry(entry)?;
            // Every restored file must match the independently verified upload receipt.
            if expected.get(&entry.source) != Some(&hash(&bytes)) {
                return Err("恢复文件与上传记录不一致".into());
            }
            Ok(bytes)
        })?;
    let id = text(&public, "id")?;
    m.previews.get_mut(id).ok_or("Restore preview expired")?["public"]["kind"] = json!("restore");
    Ok(m.previews[id]["public"].clone())
}

pub(super) fn handle(m: &mut Manager, req: &Value) -> Result<Option<Value>, String> {
    Ok(Some(match req["action"].as_str().unwrap_or("") {
        "get_cleanup_receipts" => list(m, req)?,
        "preview_restore" => preview_restore(m, req)?,
        "apply_restore" => attachments::apply_kind(m, req, "restore")?,
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    const COMMIT: &str = "1234567890123456789012345678901234567890";
    fn fixture() -> (tempfile::TempDir, Manager, Value, PathBuf) {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/cleanup-tests");
        fs::create_dir_all(parent.clone()).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("synthetic-")
            .tempdir_in(parent)
            .unwrap();
        let root = temp.path().join("skill");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: synthetic\ndescription: test\n---\nRun this skill.\n",
        )
        .unwrap();
        fs::write(root.join("README.md"), b"optional notes").unwrap();
        fs::write(root.join("guide.md"), b"extra guide").unwrap();
        let root = local::canonical(&root).unwrap();
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        let skill = json!({"id":"synthetic","localPath":root.to_string_lossy(),"readOnly":false,"repo":"synthetic/fixture","branch":"main","remotePath":"skills/synthetic","runtimeFiles":["SKILL.md","README.md"]});
        manager.upsert_skill(skill.clone());
        manager.save().unwrap();
        (temp, manager, skill, root)
    }
    fn selected() -> Vec<String> {
        vec!["SKILL.md".into(), "README.md".into(), "guide.md".into()]
    }
    #[test]
    fn plan_only_selected_optional_files_and_never_expands_to_other_local_files() {
        let (_temp, manager, skill, root) = fixture();
        let plan = planned(&skill, &["SKILL.md".into(), "README.md".into()], true).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0]["path"], "README.md");
        assert_eq!(
            plan[0]["absolutePath"],
            root.join("README.md").to_string_lossy().as_ref()
        );
        assert!(planned(&skill, &selected(), false).unwrap().is_empty());
        assert!(!receipt_dir(&manager).unwrap().exists());
        fs::write(
            root.join("SKILL.md"),
            "---\nname: synthetic\ndescription: test\n---\nRead [instructions](README.md).\n",
        )
        .unwrap();
        assert!(planned(&skill, &["README.md".into()], true)
            .unwrap()
            .is_empty());
    }
    #[test]
    fn verified_upload_removes_only_optional_bytes_and_persists_recovery_before_deletion() {
        let (temp, manager, skill, root) = fixture();
        let plan = planned(&skill, &selected(), true).unwrap();
        let expected = local::files(&root).unwrap();
        let result = verified_cleanup(&manager, &skill, COMMIT, &plan, |path| {
            fs::read(root.join(path)).map_err(|e| e.to_string())
        });
        assert_eq!(result["removed"].as_array().unwrap().len(), 2);
        assert!(root.join("SKILL.md").exists());
        assert!(!root.join("README.md").exists());
        assert!(!root.join("guide.md").exists());
        let id = text(&result, "receiptId").unwrap();
        let receipt = load(&manager, &skill, id).unwrap();
        assert_eq!(receipt["commit"], COMMIT);
        assert_eq!(receipt["files"][0]["hash"], expected["README.md"]);
        let text = fs::read_to_string(receipt_path(&manager, id).unwrap()).unwrap();
        assert!(!text.contains("optional notes"));
        let reopened = Manager::new(temp.path().join("data")).unwrap();
        let receipts = list(&reopened, &json!({"skillId":"synthetic"})).unwrap();
        assert_eq!(receipts["receipts"][0]["files"][0]["missingLocally"], true);
    }
    #[test]
    fn failed_independent_remote_read_or_hash_mismatch_keeps_every_file() {
        let (_temp, manager, skill, root) = fixture();
        let plan = planned(&skill, &selected(), true).unwrap();
        let before = local::files(&root).unwrap();
        let result = verified_cleanup(&manager, &skill, COMMIT, &plan, |_| Err("timeout".into()));
        assert!(result["removed"].as_array().unwrap().is_empty());
        assert_eq!(local::files(&root).unwrap(), before);
        let result = verified_cleanup(&manager, &skill, COMMIT, &plan, |_| {
            Ok(b"wrong commit bytes".to_vec())
        });
        assert!(result["removed"].as_array().unwrap().is_empty());
        assert_eq!(local::files(&root).unwrap(), before);
        assert!(!receipt_dir(&manager).unwrap().exists());
    }
    #[test]
    fn upload_period_edit_is_retained_but_other_verified_optional_file_can_be_trimmed() {
        let (_temp, manager, skill, root) = fixture();
        let plan = planned(&skill, &selected(), true).unwrap();
        let result = verified_cleanup(&manager, &skill, COMMIT, &plan, |path| {
            let bytes = fs::read(root.join(path)).unwrap();
            if path == "README.md" {
                fs::write(root.join(path), b"new local notes").unwrap();
            }
            Ok(bytes)
        });
        assert_eq!(result["removed"][0]["path"], "guide.md");
        assert_eq!(result["retained"][0]["path"], "README.md");
        assert_eq!(
            fs::read(root.join("README.md")).unwrap(),
            b"new local notes"
        );
        assert!(root.join("SKILL.md").exists());
    }
    #[test]
    fn new_required_reference_after_remote_read_prevents_its_deletion() {
        let (_temp, manager, skill, root) = fixture();
        let plan = planned(&skill, &["README.md".into()], true).unwrap();
        let result = verified_cleanup(&manager, &skill, COMMIT, &plan, |path| {
            fs::write(
                root.join("SKILL.md"),
                "---\nname: synthetic\ndescription: test\n---\nRead [instructions](README.md).\n",
            )
            .unwrap();
            Ok(fs::read(root.join(path)).unwrap())
        });
        assert!(result["removed"].as_array().unwrap().is_empty());
        assert!(root.join("README.md").exists());
    }
    #[test]
    fn unwritable_receipt_location_or_protected_skill_preserves_original_files() {
        let (_temp, manager, mut skill, root) = fixture();
        let plan = planned(&skill, &selected(), true).unwrap();
        fs::write(receipt_dir(&manager).unwrap(), b"existing file").unwrap();
        let result = verified_cleanup(&manager, &skill, COMMIT, &plan, |path| {
            Ok(fs::read(root.join(path)).unwrap())
        });
        assert!(result["removed"].as_array().unwrap().is_empty());
        assert!(root.join("README.md").exists());
        skill["readOnly"] = json!(true);
        let result = verified_cleanup(&manager, &skill, COMMIT, &plan, |_| {
            panic!("must not read protected source")
        });
        assert!(result["removed"].as_array().unwrap().is_empty());
        assert!(planned(&skill, &selected(), true).is_err());
    }
    #[test]
    fn stale_receipts_never_restore_to_another_source_or_directory() {
        let (_temp, manager, mut skill, root) = fixture();
        let plan = planned(&skill, &["README.md".into()], true).unwrap();
        let result = verified_cleanup(&manager, &skill, COMMIT, &plan, |path| {
            Ok(fs::read(root.join(path)).unwrap())
        });
        let id = text(&result, "receiptId").unwrap();
        skill["remotePath"] = json!("another/skill");
        assert!(load(&manager, &skill, id).is_err());
        skill["remotePath"] = json!("skills/synthetic");
        skill["localPath"] = json!(root.parent().unwrap().to_string_lossy());
        assert!(load(&manager, &skill, id).is_err());
        assert!(receipt_path(&manager, "../escape").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn atomic_delete_locks_writes_rename_and_ancestors_then_deletes_the_verified_object() {
        let (_temp, _manager, _skill, root) = fixture();
        let path = root.join("README.md");
        let digest = hash(&fs::read(&path).unwrap());
        remove_verified_file(&root, "README.md", &digest, || {
            assert!(fs::write(&path, b"concurrent edit must be refused").is_err());
            assert!(fs::rename(&path, root.join("renamed.md")).is_err());
            assert!(fs::rename(&root, root.with_file_name("changed-root")).is_err());
            // Shared reads support the final dependency closure check.
            assert_eq!(fs::read(&path).unwrap(), b"optional notes");
            assert!(local::minimal_runtime_files(&root)
                .unwrap()
                .contains(&"SKILL.md".to_owned()));
            Ok(())
        })
        .unwrap();
        assert!(!path.exists());
        assert!(root.join("SKILL.md").exists());
    }

    #[cfg(windows)]
    #[test]
    fn atomic_delete_never_requests_disposition_for_hash_failure_or_dependency_rejection() {
        let (_temp, _manager, _skill, root) = fixture();
        let path = root.join("README.md");
        assert!(
            remove_verified_file(&root, "README.md", "wrong-hash", || panic!(
                "hash failure must stop before disposition"
            ))
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"optional notes");
        let digest = hash(&fs::read(&path).unwrap());
        assert!(remove_verified_file(&root, "README.md", &digest, || Err(
            "new required reference".into()
        ))
        .is_err());
        assert_eq!(fs::read(&path).unwrap(), b"optional notes");
        // The failed operations release their handles, so normal editing works.
        fs::write(&path, b"user update after rejected delete").unwrap();
    }
}
