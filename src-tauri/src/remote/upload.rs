//! First upload attaches the existing local identity only after a confirmed commit.
use super::*;

fn source_stamp(source: &Value) -> String {
    hash(
        &serde_json::to_vec(
            &json!({"id":source["id"],"repo":source["repo"],"branch":source["branch"]}),
        )
        .unwrap(),
    )
}

fn target(m: &Manager, req: &Value) -> Result<(Value, String), String> {
    let source = m
        .state
        .sources
        .iter()
        .find(|s| s["id"] == req["sourceId"])
        .cloned()
        .ok_or("请选择已配置的 GitHub 来源")?;
    let path = text(req, "remotePath")?.trim().to_owned();
    if !path.is_empty() {
        github::validate_relative(&path)?;
    }
    if m.state.skills.iter().any(|s| {
        s["id"] != req["skillId"]
            && s["localPath"].as_str().is_some()
            && s["repo"] == source["repo"]
            && s["remotePath"] == path
            && s["branch"].as_str()
                == req["branch"]
                    .as_str()
                    .filter(|branch| !branch.trim().is_empty())
                    .or_else(|| source["branch"].as_str())
    }) {
        return Err("该远端目录已经关联另一份本地 Skill，请先核对来源".into());
    }
    Ok((source, path))
}

fn upload_plan(
    root: &Path,
    selected: &[String],
    entries: &BTreeMap<String, Entry>,
    remote_path: &str,
    mut read: impl FnMut(&Entry) -> Result<Vec<u8>, String>,
) -> Result<(Value, Value, BTreeMap<String, String>), String> {
    let expected = local::files(root)?;
    let mut rows = Vec::new();
    let mut writes = serde_json::Map::new();
    let mut total = 0usize;
    // A blob or symlink cannot be replaced by a selected nested file implicitly.
    for path in selected {
        let destination = join(remote_path, path);
        let mut prefix = String::new();
        let pieces = destination.split('/').collect::<Vec<_>>();
        for piece in &pieces[..pieces.len() - 1] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(piece);
            if entries.contains_key(&prefix) {
                return Err(format!("远端父路径是文件或链接，无法上传：{prefix}"));
            }
        }
        if entries
            .keys()
            .any(|other| other.eq_ignore_ascii_case(&destination) && other != &destination)
        {
            return Err(format!(
                "远端存在仅大小写不同的文件，请先核对：{destination}"
            ));
        }
        if entries
            .keys()
            .any(|other| other.starts_with(&format!("{destination}/")))
        {
            return Err(format!(
                "远端目标是含文件的目录，无法用文件替换：{destination}"
            ));
        }
        let bytes = read_local(root, path)?;
        if expected.get(path) != Some(&hash(&bytes)) {
            return Err("构建预览时本地文件变化，请重新预览".into());
        }
        total += bytes.len();
        if total > MAX_TOTAL {
            return Err("Selected upload exceeds the 32 MiB limit".into());
        }
        let remote = entries.get(&destination).map(&mut read).transpose()?;
        let change = match remote.as_ref() {
            None => "added",
            Some(old) if old == &bytes => "unchanged",
            _ => "modified",
        };
        rows.push(json!({"path":path,"repositoryPath":destination,"change":change,"local":preview_text(&bytes),"remote":remote.as_ref().map(|b|preview_text(b))}));
        if change != "unchanged" {
            writes.insert(path.clone(), json!(STANDARD.encode(bytes)));
        }
    }
    if local::files(root)? != expected {
        return Err("构建预览时本地文件变化，请重新预览".into());
    }
    Ok((json!(rows), Value::Object(writes), expected))
}

fn preview(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let skill = m.skill(text(req, "skillId")?)?;
    writable(&skill)?;
    if skill["sourceId"].as_str().is_some() {
        return Err("已关联的 Skill 请使用“上传本地修改”".into());
    }
    let root = local_root(&skill)?;
    let files = checked_selection(&strings(&req["files"])?)?;
    if !files.iter().any(|p| p == "SKILL.md") {
        return Err("首次上传必须包含 SKILL.md".into());
    }
    for required in local::minimal_runtime_files(&root)? {
        if !files.contains(&required) {
            return Err(format!("首次上传需包含必要运行文件：{required}"));
        }
    }
    for path in &files {
        if logic::disallowed_upload(path) {
            return Err(format!("禁止上传管理数据或凭据文件：{path}"));
        }
    }
    let (source, remote_path) = target(m, req)?;
    let snap = Snapshot::load(
        text(&source, "repo")?,
        req["branch"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| source["branch"].as_str()),
    )?;
    if !snap.api.has_token() {
        return Err("请先在设置中保存具有该仓库 Contents 写入权限的访问令牌".into());
    }
    if snap.api.head(&snap.branch)? != snap.commit {
        return Err("请选择可写分支后上传".into());
    }
    let (rows, writes, expected) = upload_plan(&root, &files, &snap.tree, &remote_path, |entry| {
        snap.read_entry(entry)
    })?;
    let cleanup_files = cleanup::planned(&skill, &files, req["trimOptional"] != false)?;
    let existing = snap
        .tree
        .keys()
        .any(|p| p == &remote_path || p.starts_with(&format!("{remote_path}/")));
    let id = m.id();
    let changed = writes.as_object().unwrap().len();
    let public = json!({"id":id,"kind":"upload","skillId":skill["id"],"summary":format!("上传 {changed} 个变化并关联现有本地 Skill"),
        "files":rows,"warnings":if existing {vec!["目标目录已有文件：请核对差异；应用后仅替换本次所选变化，未选择的远端文件保留。"]} else {vec!["上传成功后关联当前本地目录；未选择的远端文件保留。"]},
        "canApply":true,"commit":snap.commit,"targetRepo":source["repo"],"targetBranch":snap.branch,"targetPath":remote_path,"cleanupFiles":cleanup_files});
    m.previews.clear();
    m.previews.insert(id,json!({"public":public,"skillStamp":skill_stamp(&skill),"sourceStamp":source_stamp(&source),"sourceId":source["id"],
        "repo":source["repo"],"branch":snap.branch,"commit":snap.commit,"treeSha":snap.tree_sha,"remotePath":remote_path,
        "root":root.to_string_lossy(),"expected":expected,"selected":files,"writes":writes,"private":snap.private,
        "cleanupFiles":cleanup_files,"modes":snap.tree.iter().map(|(p,e)|(p.clone(),e.mode.clone())).collect::<BTreeMap<_,_>>()}));
    Ok(public)
}

fn apply(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let message = text(req, "message")?.trim();
    if message.is_empty() || message.len() > 4000 || message.contains('\0') {
        return Err("请输入 1–4000 字符的提交说明".into());
    }
    let p = consume_preview(m, req, "upload")?;
    let mut skill = m.skill(text(&p["public"], "skillId")?)?;
    writable(&skill)?;
    if p["skillStamp"] != skill_stamp(&skill) {
        return Err("Skill 配置已变化，请重新预览".into());
    }
    let mut source = m
        .state
        .sources
        .iter()
        .find(|s| s["id"] == p["sourceId"])
        .cloned()
        .ok_or("GitHub 来源已移除")?;
    if p["sourceStamp"] != source_stamp(&source) {
        return Err("GitHub 来源已变化，请重新预览".into());
    }
    let root = local_root(&skill)?;
    if root != Path::new(text(&p, "root")?) || local::files(&root)? != mapping(&p["expected"]) {
        return Err("本地文件已变化，请重新预览".into());
    }
    let branch = text(&p, "branch")?;
    let remote_path = text(&p, "remotePath")?;
    if m.state.skills.iter().any(|s| {
        s["id"] != skill["id"]
            && s["localPath"].as_str().is_some()
            && s["repo"] == p["repo"]
            && s["branch"] == branch
            && s["remotePath"] == remote_path
    }) {
        return Err("预览后该远端目录已关联另一份本地 Skill，请重新核对".into());
    }
    // All fallible local configuration work precedes the remote commit.
    // A later verification failure must never be reported as a failed upload.
    if source["branch"] != branch {
        source["id"] = json!(m.id());
        source["branch"] = json!(branch);
        source["roots"] = json!([]);
        source["ignoredPaths"] = json!([]);
    }
    track_scope(&mut source, &m.state.skills, remote_path, false)?;
    let necessary = local::minimal_runtime_files(&root)?;
    let runtime = paths(&skill["runtimeFiles"])
        .into_iter()
        .chain(necessary)
        .collect::<BTreeSet<_>>();
    if local::validate_runtime_files(&root, &runtime.iter().cloned().collect::<Vec<_>>()).is_ok() {
        skill["runtimeFiles"] = json!(runtime.iter().collect::<Vec<_>>());
        skill["runtimeConfirmed"] = json!(true);
        skill["runtimeConfirmationKind"] = json!("automatic");
        skill["runtimeAudit"] = local::runtime_audit(&skill)?;
    }
    let api = Api::new(text(&p, "repo")?)?;
    if !api.has_token() {
        return Err("访问令牌不可用，请在设置中检查".into());
    }
    let mut writes = BTreeMap::new();
    for (path, encoded) in mapping(&p["writes"]) {
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| "Invalid upload preview")?;
        let destination = join(remote_path, &path);
        let mode = p["modes"][&destination]
            .as_str()
            .filter(|mode| *mode == "100755")
            .unwrap_or("100644")
            .to_owned();
        writes.insert(destination, (mode, bytes));
    }
    let unchanged = writes.is_empty();
    let commit = git_write::submit_or_verify(
        &api,
        branch,
        text(&p, "commit")?,
        text(&p, "treeSha")?,
        message,
        &writes,
        &[],
    )?;
    source["commit"] = json!(commit);
    source["lastChecked"] = json!(now());
    skill["sourceId"] = source["id"].clone();
    skill["repo"] = p["repo"].clone();
    skill["branch"] = json!(branch);
    skill["remotePath"] = json!(remote_path);
    skill["commit"] = json!(commit);
    skill["private"] = p["private"].clone();
    skill["remoteCheckRequired"] = json!(true);
    skill["remoteMissing"] = json!(false);
    skill["associationReason"] = json!("首次上传成功后关联现有本地目录");
    let mut verification_error = None;
    match Snapshot::at_commit_fresh(text(&p, "repo")?, &commit)
        .and_then(|snap| verify_skill(&skill, &snap).map(|verified| (snap, verified)))
    {
        Ok((snap, verified)) => {
            record_push_verification(&mut skill, &commit, snap.private, verified)
        }
        Err(error) => {
            skill["status"] = json!("unknown_baseline");
            skill["error"] = json!("上传成功；后续内容核对未完成，请刷新，勿重复上传");
            verification_error = Some(error);
        }
    }
    // Remove only a duplicate remote-only discovery row for this exact target.
    m.state.skills.retain(|s| {
        !(s["id"] != skill["id"]
            && s["localPath"].is_null()
            && s["repo"] == p["repo"]
            && s["branch"] == branch
            && s["remotePath"] == remote_path)
    });
    if let Some(index) = m.state.sources.iter().position(|s| s["id"] == source["id"]) {
        m.state.sources[index] = source;
    } else {
        m.state.sources.push(source);
    }
    m.upsert_skill(skill.clone());
    m.save().map_err(|e| {
        format!("上传成功，但本地关联保存失败；文件已保留，请刷新检查后再操作：{e}")
    })?;
    let cleanup = cleanup::after_upload(
        m,
        &skill,
        &commit,
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
        .unwrap_or_else(|| json!(verification_error));
    result["transferResult"] = json!({"submitted":true,"unchanged":unchanged,"commit":commit,"cleanup":cleanup,"verificationError":final_verification_error});
    Ok(result)
}

pub(super) fn handle(m: &mut Manager, req: &Value) -> Result<Option<Value>, String> {
    Ok(Some(match req["action"].as_str().unwrap_or("") {
        "preview_upload" => preview(m, req)?,
        "apply_upload" => apply(m, req)?,
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/upload-tests");
        fs::create_dir_all(&parent).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("synthetic-")
            .tempdir_in(parent)
            .unwrap();
        let root = temp.path().join("skill");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("SKILL.md"), b"new instructions").unwrap();
        fs::write(root.join("README.md"), b"local notes").unwrap();
        let root = local::canonical(&root).unwrap();
        (temp, root)
    }
    fn entries(values: &[(&str, &[u8])]) -> BTreeMap<String, Entry> {
        values
            .iter()
            .map(|(path, bytes)| {
                (
                    path.to_string(),
                    Entry {
                        sha: "a".repeat(40),
                        size: bytes.len() as u64,
                        mode: "100644".into(),
                        source: path.to_string(),
                    },
                )
            })
            .collect()
    }
    #[test]
    fn first_upload_preview_shows_existing_conflict_and_never_deletes_omitted_readme() {
        let (_temp, root) = fixture();
        let remote = entries(&[
            ("skills/synthetic/SKILL.md", b"old instructions"),
            ("skills/synthetic/README.md", b"remote notes"),
        ]);
        let mut reads = Vec::new();
        let (rows, writes, expected) = upload_plan(
            &root,
            &["SKILL.md".into()],
            &remote,
            "skills/synthetic",
            |entry| {
                reads.push(entry.source.clone());
                Ok(b"old instructions".to_vec())
            },
        )
        .unwrap();
        assert_eq!(rows[0]["change"], "modified");
        assert_eq!(rows[0]["remote"], "old instructions");
        assert_eq!(
            writes.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["SKILL.md"]
        );
        assert_eq!(reads, vec!["skills/synthetic/SKILL.md"]);
        assert_eq!(local::files(&root).unwrap(), expected);
    }
    #[test]
    fn identical_upload_needs_no_duplicate_commit_but_can_attach_and_trim() {
        let (_temp, root) = fixture();
        let remote = entries(&[("skills/synthetic/SKILL.md", b"new instructions")]);
        let (rows, writes, _) = upload_plan(
            &root,
            &["SKILL.md".into()],
            &remote,
            "skills/synthetic",
            |_| Ok(b"new instructions".to_vec()),
        )
        .unwrap();
        assert_eq!(rows[0]["change"], "unchanged");
        assert!(writes.as_object().unwrap().is_empty());
    }
    #[test]
    fn preview_rejects_parent_blob_case_alias_and_directory_replacement() {
        let (_temp, root) = fixture();
        for remote in [
            entries(&[("skills", b"blob")]),
            entries(&[("skills/synthetic/skill.md", b"alias")]),
            entries(&[("skills/synthetic/SKILL.md/nested", b"nested")]),
        ] {
            assert!(upload_plan(
                &root,
                &["SKILL.md".into()],
                &remote,
                "skills/synthetic",
                |_| panic!("invalid targets must not read")
            )
            .is_err());
        }
    }
    #[test]
    fn local_edit_during_preview_is_not_bound_to_stale_bytes() {
        let (_temp, root) = fixture();
        let remote = entries(&[("skills/synthetic/SKILL.md", b"old")]);
        assert!(upload_plan(
            &root,
            &["SKILL.md".into()],
            &remote,
            "skills/synthetic",
            |_| {
                fs::write(root.join("SKILL.md"), b"changed during read").unwrap();
                Ok(b"old".to_vec())
            }
        )
        .unwrap_err()
        .contains("本地文件变化"));
    }
    #[test]
    fn new_target_previews_only_explicit_local_files_and_rejects_missing_files() {
        let (_temp, root) = fixture();
        let (rows, writes, _) = upload_plan(
            &root,
            &["SKILL.md".into()],
            &BTreeMap::new(),
            "skills/synthetic",
            |_| panic!(),
        )
        .unwrap();
        assert_eq!(rows[0]["change"], "added");
        assert!(writes["README.md"].is_null());
        assert!(upload_plan(
            &root,
            &["missing.md".into()],
            &BTreeMap::new(),
            "skills/synthetic",
            |_| panic!()
        )
        .is_err());
    }
}
