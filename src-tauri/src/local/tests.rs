use super::*;

#[test]
fn explicit_script_arguments_are_not_missing_package_files() {
    let temp = fixture();
    let root = skill(&temp.path().join("script-command"));
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("scripts/init-session.sh"), "#!/bin/sh\necho initialized\n").unwrap();
    let invocation = "scripts/init-session.sh \"Task Name\"";
    fs::write(root.join("SKILL.md"), format!("---\nname: fixture-skill\n---\nRun `{invocation}`.\n")).unwrap();
    let mut record = draft(&root).unwrap();
    record["runtimeFiles"] = json!(["SKILL.md", "scripts/init-session.sh"]);
    let audit = runtime_audit(&record).unwrap();
    assert_eq!(audit["status"], "passed");
    assert_eq!(audit["externalStatus"], "checked");
    let arguments = audit["externalDependencies"].as_array().unwrap().iter().find(|item| item["target"] == invocation).unwrap();
    assert_eq!(arguments["required"], false);
    assert_eq!(arguments["role"], "environment");
    assert!(paths_for_test(&audit["requiredFiles"]).contains(&"scripts/init-session.sh".to_owned()));
    record["externalBindings"] = json!([{"id":format!("{:x}", Sha256::digest(format!("SKILL.md\0{invocation}").as_bytes())),"source":"SKILL.md","target":invocation,"role":"required"}]);
    let rebound = runtime_audit(&record).unwrap();
    assert_eq!(rebound["status"], "passed", "an old whole-command binding cannot make arguments a required file");
    assert!(rebound["externalDependencies"].as_array().unwrap().iter().all(|item| item["required"] == false));
    fs::remove_file(root.join("scripts/init-session.sh")).unwrap();
    let failed = runtime_audit(&record).unwrap();
    assert_eq!(failed["status"], "needs_review");
    assert!(failed["issues"].as_array().unwrap().iter().any(|issue| issue["target"] == "scripts/init-session.sh" && issue["severity"] == "error"));
}

fn paths_for_test(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(|value| value.as_str().unwrap().to_owned()).collect()
}

#[test]
fn command_entry_matching_preserves_spaced_paths_and_ambiguous_filenames() {
    let all = BTreeMap::from([
        ("scripts/任务 初始化.sh".into(), "same".into()),
        ("scripts/exact.sh \"notes\"".into(), "same".into()),
    ]);
    assert_eq!(script_command_entry("scripts/任务 初始化.sh \"Task Name\"", &all), Some("scripts/任务 初始化.sh".into()));
    assert_eq!(script_command_entry("scripts/exact.sh \"notes\"", &all), None);
    assert_eq!(script_command_entry("scripts/真实 文件.sh", &all), None);
    assert_eq!(script_command_entry("scripts/ambiguous.py notes.txt", &all), None);
    assert_eq!(script_command_entry("scripts/missing.sh --check", &all), Some("scripts/missing.sh".into()));
}

fn fixture() -> tempfile::TempDir {
    let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("work/local-tests");
    fs::create_dir_all(&parent).unwrap();
    tempfile::Builder::new()
        .prefix("fixture-")
        .tempdir_in(parent)
        .unwrap()
}

fn skill(root: &Path) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    fs::write(
        root.join("SKILL.md"),
        "---\nname: fixture-skill\ndescription: Test only\n---\nUse this synthetic skill.\n",
    )
    .unwrap();
    root.to_path_buf()
}

fn manager(root: &Path) -> Manager {
    Manager::new(root.join("app-state")).unwrap()
}

fn register(m: &mut Manager, path: &Path) -> String {
    handle(
        m,
        &json!({"action":"add_local","paths":[path.to_string_lossy()]}),
    )
    .unwrap();
    m.state.skills[0]["id"].as_str().unwrap().to_owned()
}

fn confirm(m: &mut Manager, id: &str, paths: &[&str]) {
    handle(
        m,
        &json!({"action":"set_runtime_files","skillId":id,"files":paths}),
    )
    .unwrap();
}

#[test]
fn draft_parses_explicit_metadata_but_keeps_unknown_version_and_baseline_unknown() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    let value = draft(&root).unwrap();
    assert!(value["version"].is_null());
    assert!(value["baselineCommit"].is_null());
    assert_eq!(value["baselineHashes"], json!({}));
    assert_eq!(value["runtimeConfirmed"], false);
    assert_eq!(value["localHashes"]["SKILL.md"].as_str().unwrap().len(), 64);
    fs::write(root.join("SKILL.md"),"---\r\nname: explicit\r\ndescription: >\r\n  Two line\r\n  description.\r\nmetadata:\r\n  version: '1.2.3'\r\n  display_name: Explicit Name\r\n---\r\n").unwrap();
    let value = draft(&root).unwrap();
    assert_eq!(value["version"], "1.2.3");
    assert_eq!(value["displayName"], "Explicit Name");
    assert_eq!(value["description"], "Two line description.");
}

#[test]
fn draft_uses_existing_client_display_name_without_inventing_version() {
    let f = fixture();
    let root = skill(&f.path().join("source"));
    fs::create_dir(root.join("agents")).unwrap();
    fs::write(
        root.join("agents/openai.yaml"),
        "interface:\n  display_name: Friendly Skill\n",
    )
    .unwrap();
    let result = draft(&root).unwrap();
    assert_eq!(result["displayName"], "Friendly Skill");
    assert!(result["version"].is_null());
}

#[test]
fn scan_is_preview_only_and_reports_unreadable_skill_without_losing_good_results() {
    let fixture = fixture();
    let search = fixture.path().join("search");
    skill(&search.join("good"));
    let bad = skill(&search.join("bad"));
    fs::write(bad.join("SKILL.md"), "---\nname: [\n---\n").unwrap();
    let mut m = manager(fixture.path());
    let before = m.snapshot();
    let preview = handle(
        &mut m,
        &json!({"action":"scan_local","path":search.to_string_lossy()}),
    )
    .unwrap()
    .unwrap();
    assert_eq!(preview["skills"].as_array().unwrap().len(), 1);
    assert_eq!(preview["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(m.snapshot(), before);
}

#[test]
fn registration_keeps_folder_in_place_and_deduplicates_root_aliases() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("canonical"));
    let alias = fixture.path().join("alias");
    create_link(&canonical(&root).unwrap(), &alias).unwrap();
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    handle(
        &mut m,
        &json!({"action":"add_local","paths":[alias.to_string_lossy(),root.to_string_lossy()]}),
    )
    .unwrap();
    assert_eq!(m.state.skills.len(), 1);
    assert_eq!(m.state.skills[0]["id"], id);
    assert_eq!(
        canonical(Path::new(m.state.skills[0]["localPath"].as_str().unwrap())).unwrap(),
        canonical(&root).unwrap()
    );
    assert!(root.join("SKILL.md").is_file());
    assert!(!m.data_dir.join("skills").exists());
    remove_link(&alias, &canonical(&root).unwrap()).unwrap();
}

#[test]
fn batch_registration_does_not_partially_register_when_one_folder_is_invalid() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    let mut m = manager(fixture.path());
    let result = handle(
        &mut m,
        &json!({"action":"add_local","paths":[root.to_string_lossy(),fixture.path().join("missing").to_string_lossy()]}),
    );
    assert!(result.is_err());
    assert!(m.state.skills.is_empty());
}

#[test]
fn managed_skills_are_marked_readonly_and_not_deployed() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("plugins/cache/provider/skill"));
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    assert_eq!(m.state.skills[0]["readOnly"], true);
    let target = fixture.path().join("client/skill");
    assert!(handle(&mut m,&json!({"action":"connect","skillId":id,"client":"codex","targetPath":target.to_string_lossy()})).is_err());
    assert!(!target.exists());
    assert!(m.state.connections.is_empty());
}

#[test]
fn registration_via_managed_alias_remains_readonly_after_refresh() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("canonical"));
    let managed_parent = fixture.path().join(".system");
    fs::create_dir(&managed_parent).unwrap();
    let alias = managed_parent.join("fixture");
    create_link(&canonical(&root).unwrap(), &alias).unwrap();
    let mut m = manager(fixture.path());
    let id = register(&mut m, &alias);
    assert_eq!(m.state.skills[0]["readOnly"], true);
    handle(&mut m, &json!({"action":"refresh","skillId":id})).unwrap();
    assert_eq!(m.state.skills[0]["readOnly"], true);
    remove_link(&alias, &canonical(&root).unwrap()).unwrap();
}

#[test]
fn safe_paths_reject_traversal_absolute_paths_ads_device_aliases_and_interior_links() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    for bad in [
        "../outside",
        "/absolute",
        "a/../b",
        "a//b",
        "./SKILL.md",
        "a\\b",
        "C:/file",
        "SKILL.md:stream",
        "CON.txt",
        "NUL",
        "trailing.",
        "trailing ",
    ] {
        assert!(safe_join(&root, bad).is_err(), "{bad}");
    }
    let external = fixture.path().join("external");
    fs::create_dir_all(&external).unwrap();
    fs::write(external.join("secret.txt"), "synthetic outside file").unwrap();
    let link = root.join("linked");
    create_link(&canonical(&external).unwrap(), &link).unwrap();
    assert!(safe_join(&root, "linked/secret.txt").is_err());
    assert!(files(&root).is_err());
    remove_link(&link, &canonical(&external).unwrap()).unwrap();
}

#[test]
fn file_hashes_include_real_dependencies_and_preserve_git_without_reading_it() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".git/config"), "not runtime").unwrap();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("scripts/run.py"), "print('first')").unwrap();
    let before = files(&root).unwrap();
    assert!(!before.contains_key(".git/config"));
    fs::write(root.join("scripts/run.py"), "print('second')").unwrap();
    let after = files(&root).unwrap();
    assert_ne!(before["scripts/run.py"], after["scripts/run.py"]);
    assert!(root.join(".git/config").exists());
}

#[test]
fn connect_disconnect_are_real_links_and_forget_only_removes_the_record() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    let target = fixture.path().join("synthetic-client/fixture-skill");
    handle(&mut m,&json!({"action":"connect","skillId":id,"client":"codex","targetPath":target.to_string_lossy()})).unwrap();
    assert_eq!(canonical(&target).unwrap(), canonical(&root).unwrap());
    assert!(is_link(&fs::symlink_metadata(&target).unwrap()));
    assert_eq!(m.state.connections[0]["verified"], false);
    fs::write(root.join("proof.txt"), "one physical source").unwrap();
    assert_eq!(
        fs::read_to_string(target.join("proof.txt")).unwrap(),
        "one physical source"
    );
    assert!(handle(&mut m, &json!({"action":"forget","skillId":id})).is_err());
    let connection_id = m.state.connections[0]["id"].clone();
    handle(
        &mut m,
        &json!({"action":"disconnect","connectionId":connection_id}),
    )
    .unwrap();
    assert!(fs::symlink_metadata(&target).is_err());
    assert!(root.join("proof.txt").is_file());
    handle(&mut m, &json!({"action":"forget","skillId":id})).unwrap();
    assert!(m.state.skills.is_empty());
    assert!(root.join("SKILL.md").is_file());
}

#[test]
fn forgetting_associated_skill_excludes_rediscovery_without_deleting_runtime() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    m.state
        .sources
        .push(json!({"id":"source","repo":"fixture/repo","ignoredPaths":["other"]}));
    let record = m.state.skills.iter_mut().find(|s| s["id"] == id).unwrap();
    record["sourceId"] = json!("source");
    record["remotePath"] = json!("skills/example");
    handle(&mut m, &json!({"action":"forget","skillId":id})).unwrap();
    assert!(m.state.skills.is_empty());
    assert_eq!(
        m.state.sources[0]["ignoredPaths"],
        json!(["other", "skills/example"])
    );
    assert!(root.join("SKILL.md").is_file());
}

#[test]
fn connect_refuses_existing_folder_and_cannot_nest_a_client_inside_its_source() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    let target = fixture.path().join("occupied");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("keep.txt"), "existing").unwrap();
    assert!(handle(&mut m,&json!({"action":"connect","skillId":id,"client":"cursor","targetPath":target.to_string_lossy()})).is_err());
    assert_eq!(
        fs::read_to_string(target.join("keep.txt")).unwrap(),
        "existing"
    );
    assert!(handle(&mut m,&json!({"action":"connect","skillId":id,"client":"cursor","targetPath":root.join("nested").to_string_lossy()})).is_err());
    assert!(!root.join("nested").exists());
    assert!(m.state.connections.is_empty());
}

#[test]
fn disconnect_refuses_changed_link_target_or_replacement_real_directory() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    let other = skill(&fixture.path().join("other"));
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    let target = fixture.path().join("client/skill");
    handle(&mut m,&json!({"action":"connect","skillId":id,"client":"claude","targetPath":target.to_string_lossy()})).unwrap();
    let connection = m.state.connections[0]["id"].clone();
    remove_link(&target, &canonical(&root).unwrap()).unwrap();
    create_link(&canonical(&other).unwrap(), &target).unwrap();
    assert!(handle(
        &mut m,
        &json!({"action":"disconnect","connectionId":connection})
    )
    .is_err());
    assert_eq!(canonical(&target).unwrap(), canonical(&other).unwrap());
    assert_eq!(m.state.connections.len(), 1);
    remove_link(&target, &canonical(&other).unwrap()).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(target.join("keep.txt"), "keep").unwrap();
    assert!(handle(
        &mut m,
        &json!({"action":"disconnect","connectionId":connection})
    )
    .is_err());
    assert!(target.join("keep.txt").exists());
    assert!(root.join("SKILL.md").exists());
    assert!(other.join("SKILL.md").exists());
}

#[test]
fn runtime_confirmation_requires_interface_licenses_and_referenced_files_without_deleting_docs() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    fs::create_dir_all(root.join("agents")).unwrap();
    fs::write(root.join("agents/openai.yaml"), "interface: {}\n").unwrap();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("scripts/run.py"), "print('ok')").unwrap();
    fs::write(
        root.join("SKILL.md"),
        "---\nname: fixture\n---\nRun `scripts/run.py`.\n",
    )
    .unwrap();
    fs::write(root.join("LICENSE"), "MIT example fixture\n").unwrap();
    fs::write(
        root.join("README.md"),
        "remote-style documentation retained locally",
    )
    .unwrap();
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    for incomplete in [
        vec!["SKILL.md"],
        vec!["SKILL.md", "LICENSE"],
        vec!["SKILL.md", "LICENSE", "agents/openai.yaml"],
    ] {
        assert!(handle(
            &mut m,
            &json!({"action":"set_runtime_files","skillId":id,"files":incomplete})
        )
        .is_err());
    }
    confirm(
        &mut m,
        &id,
        &[
            "SKILL.md",
            "LICENSE",
            "agents/openai.yaml",
            "scripts/run.py",
        ],
    );
    assert_eq!(m.state.skills[0]["runtimeConfirmed"], true);
    assert!(root.join("README.md").exists());
    assert!(m.state.skills[0]["baselineCommit"].is_null());
    assert_eq!(m.state.skills[0]["baselineHashes"], json!({}));
}

#[test]
fn runtime_defaults_separate_optional_docs_from_required_and_recommended_resources() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::create_dir_all(root.join("references")).unwrap();
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::create_dir_all(root.join("agents")).unwrap();
    for (path, text) in [
        ("SKILL.md", "Read [workflow](references/workflow.md)."),
        ("references/workflow.md", "Run `scripts/run.py`."),
        ("scripts/run.py", "print('ok')\n"),
        ("references/extra.md", "Optional reference material."),
        ("agents/openai.yaml", "interface: {}\n"),
        ("agents/README.md", "Management documentation."),
        ("LICENSE", "Fixture license."),
        ("README.md", "Repository usage."),
        ("CHANGELOG.md", "Full history."),
        ("notes.txt", "Personal notes, not a dependency."),
    ] {
        fs::write(root.join(path), text).unwrap();
    }
    fs::write(root.join("assets/optional.png"), [0, 1, 2]).unwrap();
    let result = runtime_file_selection(&root, &files(&root).unwrap(), None).unwrap();
    for path in [
        "SKILL.md",
        "LICENSE",
        "agents/openai.yaml",
        "references/workflow.md",
        "scripts/run.py",
    ] {
        assert!(result.required_files.iter().any(|p| p == path), "{path}");
        assert!(!result.required_reasons[path].is_empty());
        assert!(result.suggested_files.iter().any(|p| p == path));
    }
    for path in ["README.md", "CHANGELOG.md", "notes.txt", "agents/README.md"] {
        assert!(result.optional_files.iter().any(|p| p == path), "{path}");
        assert!(!result.suggested_files.iter().any(|p| p == path), "{path}");
    }
    for path in ["references/extra.md", "assets/optional.png"] {
        assert!(result.optional_files.iter().any(|p| p == path));
        assert!(result.suggested_files.iter().any(|p| p == path));
    }
    let draft = draft(&root).unwrap();
    assert_eq!(draft["runtimeFiles"], json!(result.suggested_files));
    assert_eq!(draft["runtimeConfirmed"], false);
    assert!(root.join("README.md").exists());
}

#[test]
fn referenced_management_documents_become_required_through_recursive_links_and_cycles() {
    let contents = BTreeMap::from([
        ("SKILL.md", "Read [usage](README.md)."),
        ("README.md", "Read [details](docs/details.md)."),
        (
            "docs/details.md",
            "Back to [usage](../README.md); see [history](../CHANGELOG.md).",
        ),
        ("CHANGELOG.md", "Run `scripts/run.py`."),
        ("scripts/run.py", "print('ok')\n"),
        ("CONTRIBUTING.md", "Optional repository maintenance."),
    ]);
    let all = contents
        .keys()
        .map(|path| ((*path).to_owned(), "hash".to_owned()))
        .collect();
    let result =
        runtime_file_selection_with_reader(&all, None, |path| Ok(contents[path].to_owned()))
            .unwrap();
    for path in [
        "README.md",
        "docs/details.md",
        "CHANGELOG.md",
        "scripts/run.py",
    ] {
        assert!(result.required_files.iter().any(|p| p == path), "{path}");
    }
    assert!(result.optional_files.iter().any(|p| p == "CONTRIBUTING.md"));
    assert!(!result
        .suggested_files
        .iter()
        .any(|p| p == "CONTRIBUTING.md"));
    assert_eq!(result.required_reasons["SKILL.md"], "Skill 入口与执行规则");
}

#[test]
fn runtime_dependency_closure_retains_local_python_imports_and_script_resources() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    fs::create_dir_all(root.join("scripts/nested")).unwrap();
    fs::create_dir_all(root.join("assets")).unwrap();
    for (path, text) in [
        ("SKILL.md", "Run `scripts/run.py`."),
        (
            "scripts/run.py",
            "from helper import execute\nimport json\nexecute()\n",
        ),
        ("scripts/helper.py", "from nested import work\n"),
        (
            "scripts/nested/__init__.py",
            "# Local package initialization.\n",
        ),
        (
            "scripts/nested/work.py",
            "open('../../assets/schema.json')\n",
        ),
        ("assets/schema.json", "{\"type\":\"fixture\"}\n"),
        ("scripts/unrelated.py", "print('optional')\n"),
    ] {
        fs::write(root.join(path), text).unwrap();
    }
    let minimal = BTreeSet::from(["SKILL.md".to_owned()]);
    let result = runtime_file_selection(&root, &files(&root).unwrap(), Some(&minimal)).unwrap();
    let complete = [
        "SKILL.md",
        "scripts/run.py",
        "scripts/helper.py",
        "scripts/nested/__init__.py",
        "scripts/nested/work.py",
        "assets/schema.json",
    ]
    .map(str::to_owned)
    .to_vec();
    for path in &complete {
        assert!(result.required_files.contains(path), "{path}");
    }
    assert!(!result
        .required_files
        .iter()
        .any(|p| p == "scripts/unrelated.py"));
    assert!(result.warnings.iter().any(|w| w.contains("动态加载")));
    assert!(validate_runtime_files(&root, &complete).is_ok());
    for removed in &complete[1..] {
        let incomplete: Vec<_> = complete
            .iter()
            .filter(|path| *path != removed)
            .cloned()
            .collect();
        assert!(validate_runtime_files(&root, &incomplete)
            .unwrap_err()
            .contains(removed));
    }
    assert!(!result.required_files.iter().any(|p| p.ends_with("json.py")));
}

#[test]
fn runtime_classification_reports_missing_files_and_refuses_an_unreadable_entry() {
    let all = BTreeMap::from([("SKILL.md".to_owned(), "hash".to_owned())]);
    let result = runtime_file_selection_with_reader(&all, None, |_| {
        Ok("Read [missing](references/missing.md); run `scripts/missing.py`.".into())
    })
    .unwrap();
    assert_eq!(result.required_files, ["SKILL.md"]);
    assert_eq!(result.suggested_files, ["SKILL.md"]);
    assert!(result
        .warnings
        .iter()
        .any(|w| w.contains("references/missing.md")));
    assert!(result
        .warnings
        .iter()
        .any(|w| w.contains("scripts/missing.py")));
    assert!(
        runtime_file_selection_with_reader(&all, None, |_| Err("not readable".into()))
            .unwrap_err()
            .contains("SKILL.md")
    );
}

#[test]
fn file_listing_automatically_recognizes_saved_scope_and_preserves_manual_optional_choices() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    fs::create_dir_all(root.join("references")).unwrap();
    fs::write(root.join("README.md"), "Optional manual.").unwrap();
    fs::write(root.join("references/extra.md"), "Optional resource.").unwrap();
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    m.state.skills[0]["runtimeFiles"] = json!(["SKILL.md", "README.md", "references/extra.md"]);
    let result = handle(&mut m, &json!({"action":"get_files","skillId":id}))
        .unwrap()
        .unwrap();
    assert_eq!(result["runtimeConfirmed"], true);
    assert_eq!(result["runtimeConfirmationKind"], "automatic");
    assert!(result["suggestedFiles"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "README.md"));
    assert!(result["optionalFiles"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "README.md"));
    confirm(&mut m, &id, &["SKILL.md", "README.md"]);
    let before = m.state.skills[0]["runtimeFiles"].clone();
    let result = handle(&mut m, &json!({"action":"get_files","skillId":id}))
        .unwrap()
        .unwrap();
    assert_eq!(result["runtimeConfirmed"], true);
    assert_eq!(result["suggestedFiles"], before);
    assert_eq!(m.state.skills[0]["runtimeFiles"], before);
    assert!(root.join("README.md").exists());
    assert!(root.join("references/extra.md").exists());
}

#[test]
fn changing_confirmed_runtime_scope_invalidates_observations_but_keeps_installation_history() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    fs::write(root.join("README.md"), "Optional documentation.").unwrap();
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    confirm(&mut m, &id, &["SKILL.md"]);
    let fields = [
        "comparisonLocalHashes",
        "comparisonRemoteHashes",
        "comparisonObservedAt",
        "comparisonRuntimeFiles",
        "comparisonRuntimeSelection",
        "comparisonCommit",
        "comparisonScope",
        "comparisonDocHash",
    ];
    {
        let record = &mut m.state.skills[0];
        record["sourceId"] = json!("fixture-source");
        record["baselineCommit"] = json!("installed-commit");
        record["baselineHashes"] = json!({"SKILL.md":"installed-hash"});
        record["remoteHashes"] = json!({"SKILL.md":"old-scope-hash"});
        record["status"] = json!("synced");
        record["changeDetails"] = json!([{"path":"SKILL.md"}]);
        for field in fields {
            record[field] = json!("cached-value");
        }
    }
    confirm(&mut m, &id, &["SKILL.md", "README.md"]);
    let record = &m.state.skills[0];
    for field in fields {
        assert!(record[field].is_null(), "{field}");
    }
    assert!(record["remoteHashes"].is_null());
    assert_eq!(record["changeDetails"], json!([]));
    assert_eq!(record["remoteCheckRequired"], true);
    assert_eq!(record["status"], "unknown_baseline");
    assert_eq!(record["baselineCommit"], "installed-commit");
    assert_eq!(
        record["baselineHashes"],
        json!({"SKILL.md":"installed-hash"})
    );
    m.state.skills[0]["comparisonCommit"] = json!("new-observation");
    m.state.skills[0]["remoteHashes"] = json!({"README.md":"current-hash"});
    m.state.skills[0]["remoteCheckRequired"] = json!(false);
    confirm(&mut m, &id, &["README.md", "SKILL.md"]);
    assert_eq!(m.state.skills[0]["comparisonCommit"], "new-observation");
    assert_eq!(
        m.state.skills[0]["remoteHashes"],
        json!({"README.md":"current-hash"})
    );
    assert_eq!(m.state.skills[0]["remoteCheckRequired"], false);
    assert!(root.join("README.md").exists());
}

#[test]
fn runtime_references_use_package_paths_for_instruction_tokens_and_document_paths_for_links() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    fs::create_dir_all(root.join("references")).unwrap();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(
        root.join("SKILL.md"),
        "---\nname: example\n---\nRead [workflow](references/workflow.md).\n",
    )
    .unwrap();
    fs::write(
        root.join("references/workflow.md"),
        "Read `references/details.md` and invoke [runner](../scripts/run.py).\n",
    )
    .unwrap();
    fs::write(
        root.join("references/details.md"),
        "A real package reference.\n",
    )
    .unwrap();
    fs::write(root.join("scripts/run.py"), "print('real dependency')\n").unwrap();
    let complete = vec![
        "SKILL.md".to_owned(),
        "references/workflow.md".to_owned(),
        "references/details.md".to_owned(),
        "scripts/run.py".to_owned(),
    ];
    assert!(validate_runtime_files(&root, &complete).is_ok());
    assert!(validate_runtime_files(&root, &complete[..3])
        .unwrap_err()
        .contains("scripts/run.py"));
    let (_dependencies, warnings) = dependency_notes(
        &root,
        &complete.into_iter().collect(),
        &files(&root).unwrap(),
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn unknown_examples_warn_without_inventing_files_and_fenced_examples_are_not_dependencies() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    fs::write(root.join("SKILL.md"),"---\nname: examples\n---\nExample `scripts/not-installed.py`.\n[Future guide](references/future.md)\n```md\n[illustration](scripts/fenced.py)\nRun scripts/another-example.py\n```\n").unwrap();
    let selected = vec!["SKILL.md".to_owned()];
    assert!(validate_runtime_files(&root, &selected).is_ok());
    let (dependencies, warnings) = dependency_notes(
        &root,
        &selected.into_iter().collect(),
        &files(&root).unwrap(),
    )
    .unwrap();
    assert!(dependencies.is_empty());
    assert_eq!(warnings.len(), 2);
    assert!(warnings.iter().any(|w| w.contains("not-installed.py")));
    assert!(warnings
        .iter()
        .all(|w| !w.contains("fenced.py") && !w.contains("another-example.py")));
}

#[test]
fn export_contains_confirmed_files_license_and_provenance_and_never_overwrites_or_writes_into_source(
) {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    fs::write(root.join("LICENSE"), "MIT fixture").unwrap();
    fs::write(root.join("README.md"), "retained but not selected").unwrap();
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(root.join("assets/selected.bin"), [0, 1, 2, 3]).unwrap();
    fs::write(root.join("assets/unselected.bin"), [4, 5, 6]).unwrap();
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    let zip_path = fixture.path().join("export.zip");
    let automatic_zip = fixture.path().join("automatic-export.zip");
    handle(
        &mut m,
        &json!({"action":"export","skillId":id,"destination":automatic_zip.to_string_lossy()}),
    )
    .unwrap();
    let mut automatic_archive =
        zip::ZipArchive::new(fs::File::open(&automatic_zip).unwrap()).unwrap();
    let expected: BTreeSet<_> = string_list(&m.state.skills[0], "runtimeFiles")
        .unwrap()
        .into_iter()
        .chain(["skill-manager-provenance.json".into()])
        .collect();
    assert_eq!(
        automatic_archive
            .file_names()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>(),
        expected
    );
    assert!(automatic_archive.by_name("LICENSE").is_ok());
    assert!(automatic_archive.by_name("README.md").is_err());
    for entry in ["assets/selected.bin", "assets/unselected.bin"] {
        let mut actual = Vec::new();
        automatic_archive
            .by_name(entry)
            .unwrap()
            .read_to_end(&mut actual)
            .unwrap();
        assert_eq!(actual, fs::read(root.join(entry)).unwrap());
    }
    confirm(&mut m, &id, &["SKILL.md", "LICENSE", "assets/selected.bin"]);
    assert!(handle(&mut m,&json!({"action":"export","skillId":id,"destination":root.join("bad.zip").to_string_lossy()})).is_err());
    assert!(!root.join("bad.zip").exists());
    let alias = fixture.path().join("source-alias");
    create_link(&canonical(&root).unwrap(), &alias).unwrap();
    assert!(handle(&mut m,&json!({"action":"export","skillId":id,"destination":alias.join("hidden-in-source.zip").to_string_lossy()})).is_err());
    assert!(!root.join("hidden-in-source.zip").exists());
    remove_link(&alias, &canonical(&root).unwrap()).unwrap();
    handle(
        &mut m,
        &json!({"action":"export","skillId":id,"destination":zip_path.to_string_lossy()}),
    )
    .unwrap();
    let original = fs::read(&zip_path).unwrap();
    assert!(handle(
        &mut m,
        &json!({"action":"export","skillId":id,"destination":zip_path.to_string_lossy()})
    )
    .is_err());
    assert_eq!(fs::read(&zip_path).unwrap(), original);
    let mut archive = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
    assert_eq!(archive.len(), 4);
    assert!(archive.by_name("SKILL.md").is_ok());
    assert!(archive.by_name("LICENSE").is_ok());
    assert!(archive.by_name("README.md").is_err());
    assert!(archive.by_name("assets/selected.bin").is_ok());
    assert!(archive.by_name("assets/unselected.bin").is_err());
    let mut provenance = String::new();
    archive
        .by_name("skill-manager-provenance.json")
        .unwrap()
        .read_to_string(&mut provenance)
        .unwrap();
    let value: Value = serde_json::from_str(&provenance).unwrap();
    assert_eq!(value["licenseStatus"], "included");
    assert!(value["version"].is_null());
    assert!(value.get("localPath").is_none());
    assert!(root.join("README.md").exists());
    assert!(root.join("assets/unselected.bin").exists());
}

#[test]
fn local_refresh_updates_actual_hashes_without_inventing_a_remote_baseline() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    let before = m.state.skills[0]["localHashes"].clone();
    fs::write(root.join("new.txt"), "new local information").unwrap();
    handle(&mut m, &json!({"action":"refresh","skillId":id})).unwrap();
    let value = &m.state.skills[0];
    assert_ne!(value["localHashes"], before);
    assert!(value["version"].is_null());
    assert_eq!(value["baselineHashes"], json!({}));
    assert_eq!(value["status"], "local_only");
    drop(m);
    let m = manager(fixture.path());
    assert_eq!(
        m.state.skills[0]["localHashes"]["new.txt"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
}

#[test]
fn file_preview_is_bounded_and_remote_only_records_are_deferred() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    fs::write(root.join("binary.bin"), [0, 1, 2]).unwrap();
    assert!(handle(
        &mut m,
        &json!({"action":"read_file","skillId":id,"path":"binary.bin"})
    )
    .is_err());
    fs::write(
        root.join("large.txt"),
        vec![b'a'; MAX_TEXT_BYTES as usize + 1],
    )
    .unwrap();
    assert!(handle(
        &mut m,
        &json!({"action":"read_file","skillId":id,"path":"large.txt"})
    )
    .is_err());
    assert!(handle(
        &mut m,
        &json!({"action":"read_file","skillId":id,"path":"../outside"})
    )
    .is_err());
    m.state
        .skills
        .push(json!({"id":"remote","localPath":null,"sourceId":"github"}));
    assert!(
        handle(&mut m, &json!({"action":"get_files","skillId":"remote"}))
            .unwrap()
            .is_none()
    );
    assert!(
        handle(&mut m, &json!({"action":"refresh","skillId":"remote"}))
            .unwrap()
            .is_none()
    );
}

#[test]
fn v025_file_inventory_reuses_hashes_only_inside_one_read_and_still_detects_next_edit() {
    let fixture = fixture();
    let root = skill(&fixture.path().join("source"));
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("scripts/run.py"), "print('one')\n").unwrap();
    fs::write(
        root.join("SKILL.md"),
        "---\nname: fixture-skill\ndescription: Test only\n---\nRun [script](scripts/run.py).\n",
    )
    .unwrap();
    let mut m = manager(fixture.path());
    let id = register(&mut m, &root);
    let first = handle(&mut m, &json!({"action":"get_files","skillId":id}))
        .unwrap()
        .unwrap();
    let row_hash = |value: &Value| {
        value["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["path"] == "scripts/run.py")
            .unwrap()["hash"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let first_hash = row_hash(&first);
    assert!(first["exportRequiredFiles"]
        .as_array()
        .unwrap()
        .contains(&json!("scripts/run.py")));
    let current = files(&root).unwrap();
    assert_eq!(
        minimal_runtime_files_with_inventory(&root, &current).unwrap(),
        minimal_runtime_files(&root).unwrap()
    );
    fs::write(
        root.join("scripts/run.py"),
        "print('changed between requests')\n",
    )
    .unwrap();
    let second = handle(&mut m, &json!({"action":"get_files","skillId":id}))
        .unwrap()
        .unwrap();
    assert_ne!(row_hash(&second), first_hash);
    assert_eq!(row_hash(&second), files(&root).unwrap()["scripts/run.py"]);
    assert!(second["exportRequiredFiles"]
        .as_array()
        .unwrap()
        .contains(&json!("scripts/run.py")));
}
