//! Batch package checks preserve existing scopes and never execute Skill code.
use crate::{
    local,
    model::{now, Manager},
    remote,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn paths(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect()
}

pub(crate) fn preserves_manual_scope(skill: &Value) -> bool {
    skill["runtimeConfirmationKind"] == "manual"
        || skill["runtimeConfirmed"] == true && skill["runtimeConfirmationKind"] != "automatic"
}

fn manifest_stamp(skill: &Value) -> Value {
    let mut manifest = remote::declared_runtime(Some(&skill["catalogMetadata"]));
    if let Some(files) = &mut manifest {
        files.sort();
        files.dedup();
    }
    json!({"sourceId":skill["sourceId"],"repo":skill["repo"],"branch":skill["branch"],
        "remotePath":skill["remotePath"],"runtimeFiles":manifest})
}

fn binding_stamp(skill: &Value) -> String {
    // Explicit external bindings may change without any Skill file changing.
    // Metadata is a cheap cache invalidator; actual audits/exports still read
    // and hash the selected resource before treating it as available.
    let local_files: Vec<_> = skill["externalBindings"].as_array().into_iter().flatten()
        .filter(|binding|binding["sourceKind"]!="github")
        .filter_map(|binding|binding["path"].as_str())
        .map(|path| {
            let canonical=std::fs::canonicalize(path).ok();
            let metadata=std::fs::metadata(path).ok();
            let modified=metadata.as_ref().and_then(|metadata|metadata.modified().ok())
                .and_then(|time|time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|time|time.as_nanos().to_string());
            json!({"path":path,"canonical":canonical.map(|path|path.to_string_lossy().into_owned()),"length":metadata.as_ref().map(|metadata|metadata.len()),"modified":modified})
        }).collect();
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(
                &json!({"bindings":skill["externalBindings"],"localFiles":local_files})
            )
            .expect("serializable bindings")
        )
    )
}

fn invalidate_comparison(skill: &mut Value) {
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
    skill["remoteHashes"] = Value::Null;
    skill["changeDetails"] = json!([]);
    if skill["sourceId"].as_str().is_some() {
        skill["remoteCheckRequired"] = json!(true);
        skill["status"] = json!("unknown_baseline");
    }
}

fn apply_local_audit(skill: &mut Value) -> Result<(), String> {
    apply_local_audit_with_inventory(skill, None)
}

fn apply_local_audit_with_inventory(
    skill: &mut Value,
    inventory: Option<&BTreeMap<String, String>>,
) -> Result<(), String> {
    let owned_inventory = if inventory.is_none() {
        skill["localPath"]
            .as_str()
            .map(|root| local::files(std::path::Path::new(root)))
            .transpose()?
    } else {
        None
    };
    let inventory = inventory.or(owned_inventory.as_ref());
    if let Some(inventory) = inventory {
        local::retire_obsolete_reference_bindings(skill, inventory);
    }
    let manual = preserves_manual_scope(skill);
    let mut audit = if let Some(inventory) = inventory {
        local::runtime_audit_with_inventory(skill, inventory)?
    } else {
        local::runtime_audit(skill)?
    };
    if audit["status"] != "passed" && skill["runtimeAudit"]["invalidatedAt"].as_str().is_some() {
        audit["invalidatedAt"] = skill["runtimeAudit"]["invalidatedAt"].clone();
    }
    if manual {
        let selected = paths(&skill["runtimeFiles"]);
        let missing: Vec<_> = paths(&audit["requiredFiles"])
            .difference(&selected)
            .cloned()
            .collect();
        if !missing.is_empty() {
            let reason = format!(
                "已保留现有手动文件范围；新识别的必要文件尚未纳入该范围：{}。请核对后调整。",
                missing.join("、")
            );
            audit["status"] = json!("needs_review");
            audit["canAutoConfirm"] = json!(false);
            audit["reason"] = json!(reason);
            audit["issues"].as_array_mut().expect("audit issue list").push(json!({"category":"uncertain","severity":"warning","source":"","target":"","reason":reason}));
            invalidate_comparison(skill);
        }
    }
    skill["runtimeIssues"] = audit["issues"].clone();
    skill["runtimeWarnings"] = audit["warnings"].clone();
    if !manual && audit["canAutoConfirm"] == true && skill["readOnly"] != true {
        if paths(&skill["runtimeFiles"]) != paths(&audit["proposedScope"]) {
            invalidate_comparison(skill);
        }
        skill["runtimeFiles"] = audit["proposedScope"].clone();
        if skill["runtimeConfirmed"] != true {
            skill["runtimeConfirmationKind"] = json!("automatic");
        }
        skill["runtimeConfirmed"] = json!(true);
    } else if !manual
        && audit["canAutoConfirm"] != true
        && skill["runtimeConfirmationKind"] == "automatic"
    {
        skill["runtimeConfirmed"] = json!(false);
    }
    audit["externalBindingStamp"] = json!(binding_stamp(skill));
    audit["manifestStamp"] = manifest_stamp(skill);
    skill["runtimeAudit"] = audit;
    Ok(())
}

/// Reuse current local hashes at existing registration/read/refresh boundaries.
/// Unchanged reads retain the same evidence and do not produce database writes.
pub(crate) fn refresh_local_audit(
    skill: &mut Value,
    inventory: Option<&BTreeMap<String, String>>,
) -> bool {
    let before = skill.clone();
    if let Some(inventory) = inventory {
        local::invalidate_runtime_audit(skill, inventory);
        let audit = &skill["runtimeAudit"];
        let recorded_scope = if !preserves_manual_scope(skill)
            && audit["canAutoConfirm"] == true
            && skill["readOnly"] != true
        {
            &audit["proposedScope"]
        } else {
            &audit["preservedScope"]
        };
        if audit["analyzerVersion"] == local::RUNTIME_ANALYZER_VERSION
            && audit["inventoryHashes"] == json!(inventory)
            && paths(recorded_scope) == paths(&skill["runtimeFiles"])
            && audit["externalBindingStamp"] == binding_stamp(skill)
            && audit["manifestStamp"] == manifest_stamp(skill)
        {
            return false;
        }
    }
    if let Err(error) = apply_local_audit_with_inventory(skill, inventory) {
        if skill["runtimeConfirmationKind"] == "automatic" {
            skill["runtimeConfirmed"] = json!(false);
        }
        let invalidated = skill["runtimeAudit"]["invalidatedAt"].clone();
        skill["runtimeAudit"] = json!({"status":if invalidated.is_string(){"needs_review"}else{"failed"},"checkedAt":now(),"method":"static_package","analyzerVersion":local::RUNTIME_ANALYZER_VERSION,"reason":error,"canAutoConfirm":false,"preservedScope":skill["runtimeFiles"],"inventoryHashes":inventory,"externalBindingStamp":binding_stamp(skill),"invocationStatus":"not_verified"});
        skill["runtimeAudit"]["manifestStamp"] = manifest_stamp(skill);
        if invalidated.is_string() {
            skill["runtimeAudit"]["invalidatedAt"] = invalidated;
        }
    }
    *skill != before
}

pub fn handle(m: &mut Manager, request: &Value) -> Result<Option<Value>, String> {
    if request["action"] != "audit_skills" {
        return Ok(None);
    }
    let selected = m.refresh_selection(request)?;
    let ids: Vec<String> = m
        .state
        .skills
        .iter()
        .filter(|skill| {
            selected
                .as_ref()
                .is_none_or(|ids| skill["id"].as_str().is_some_and(|id| ids.contains(id)))
        })
        .filter_map(|skill| skill["id"].as_str().map(str::to_owned))
        .collect();
    let previous = m.state.clone();
    for skill in m.state.skills.iter_mut().filter(|skill| {
        skill["id"]
            .as_str()
            .is_some_and(|id| ids.iter().any(|item| item == id))
    }) {
        if skill["localPath"].as_str().is_none() {
            skill["runtimeAudit"] = json!({"status":"needs_review","checkedAt":now(),"method":"static_package","reason":"尚无本地运行文件，可在下载预览中检查远端清单","invocationStatus":"not_verified"});
            continue;
        }
        if let Err(error) = apply_local_audit(skill) {
            if skill["runtimeConfirmationKind"] == "automatic" {
                skill["runtimeConfirmed"] = json!(false);
            }
            skill["runtimeAudit"] = json!({"status":"failed","checkedAt":now(),"method":"static_package","reason":error,"invocationStatus":"not_verified"});
        }
    }
    if let Err(error) = m.save() {
        m.state = previous;
        return Err(error);
    }
    // Remote refresh obtains a fresh immutable commit and rehashes local bytes;
    // cached equality alone must never create a comparison baseline.
    if request["localOnly"] != true && !ids.is_empty() {
        remote::handle(m, &json!({"action":"refresh","skillIds":ids}))?;
    }
    let mut passed = 0usize;
    let mut needs_review = 0usize;
    let mut failed = 0usize;
    let items: Vec<Value> = ids.iter().filter_map(|id| m.state.skills.iter().find(|skill| skill["id"] == *id)).map(|skill| {
        let mut status = skill["runtimeAudit"]["status"].as_str().unwrap_or("failed");
        let mut reason = skill["runtimeAudit"]["reason"].as_str().map(str::to_owned);
        let comparison = if skill["sourceId"].is_null() { "local_only" }
            else if request["localOnly"] == true || skill["remoteCheckRequired"] == true || skill["error"].as_str().is_some() { "unavailable" }
            else if matches!(skill["status"].as_str(),Some("synced" | "docs_updated")) { "matched" } else { "different" };
        if status == "passed" && skill["sourceId"].as_str().is_some() && comparison != "matched" {
            status = "needs_review";
            reason = Some(if comparison == "different" { "运行文件结构核对通过；双方内容不同，需核对差异" } else { "运行文件结构核对通过；远端检查尚未完成，未建立新的比较基准" }.into());
        }
        match status { "passed" => passed += 1, "needs_review" => needs_review += 1, _ => failed += 1 }
        json!({"skillId":skill["id"],"status":status,"reason":reason,"comparisonStatus":comparison,"audit":skill["runtimeAudit"]})
    }).collect();
    let mut snapshot = m.snapshot();
    snapshot["auditSummary"] = json!({"checked":items.len(),"passed":passed,"needsReview":needs_review,"failed":failed,"items":items});
    Ok(Some(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};
    #[test]
    fn catalog_and_source_changes_invalidate_audit_but_manifest_order_does_not() {
        let temp = fixture();
        let root = temp.path().join("manifest-cache");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("SKILL.md"), "---\nname: fixture\n---\nInstructions.\n").unwrap();
        fs::write(root.join("README.md"), "Optional notes.\n").unwrap();
        let mut record = local::draft(&root).unwrap();
        record["sourceId"] = json!("source");
        record["catalogMetadata"] = json!({"runtime_files":["SKILL.md"]});
        let inventory = local::files(&root).unwrap();
        assert!(refresh_local_audit(&mut record, Some(&inventory)));
        assert!(!refresh_local_audit(&mut record, Some(&inventory)));
        let first = record["runtimeAudit"]["manifestStamp"].clone();
        record["catalogMetadata"]["runtime_files"] = json!(["README.md", "SKILL.md"]);
        assert!(refresh_local_audit(&mut record, Some(&inventory)));
        assert_ne!(record["runtimeAudit"]["manifestStamp"], first);
        record["catalogMetadata"]["runtime_files"] = json!(["SKILL.md", "README.md"]);
        assert!(!refresh_local_audit(&mut record, Some(&inventory)));
        record["sourceId"] = json!("other-source");
        assert!(refresh_local_audit(&mut record, Some(&inventory)));
    }
    fn fixture() -> tempfile::TempDir {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/audit-tests");
        fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }
    #[test]
    fn analyzer_upgrade_recomputes_unchanged_old_evidence_and_retires_only_composite_binding() {
        let fixture = fixture();
        let root = fixture.path().join("skill");
        fs::create_dir_all(root.join("references")).unwrap();
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: fixture\n---\nRead references/tex-output.md。\n",
        )
        .unwrap();
        let composite =
            "assets/model-extract.tex和assets/model-report.tex以@@UPPER_CASE@@标记待填项";
        fs::write(
            root.join("references/tex-output.md"),
            format!("{composite}。"),
        )
        .unwrap();
        for path in ["assets/model-extract.tex", "assets/model-report.tex"] {
            fs::write(root.join(path), "template").unwrap();
        }
        let mut skill = local::draft(&root).unwrap();
        let malformed = json!({"id":"old-composite","source":"references/tex-output.md","target":composite,"path":root.join("references/tex-output.md").to_string_lossy(),"role":"required"});
        let legitimate = json!({"id":"unchanged-user-binding","source":"other.md","target":"D:/rules.md","role":"environment"});
        skill["externalBindings"] = json!([malformed, legitimate]);
        apply_local_audit(&mut skill).unwrap();
        let inventory = local::files(&root).unwrap();
        skill["runtimeAudit"]["analyzerVersion"] = json!(1);
        skill["runtimeAudit"]["status"] = json!("needs_review");
        skill["runtimeAudit"]["issues"] = json!([{"category":"package_required","severity":"error","source":"references/tex-output.md","target":composite,"reason":"old parser"}]);
        assert!(refresh_local_audit(&mut skill, Some(&inventory)));
        assert_eq!(skill["runtimeAudit"]["status"], "passed");
        assert_eq!(
            skill["runtimeAudit"]["analyzerVersion"],
            local::RUNTIME_ANALYZER_VERSION
        );
        assert_eq!(skill["externalBindings"], json!([legitimate]));
        assert_eq!(skill["externalBindingHistory"][0]["id"], "old-composite");
        assert!(
            !refresh_local_audit(&mut skill, Some(&inventory)),
            "unchanged current evidence should be reused"
        );
    }

    #[test]
    fn replacement_binding_resolves_audit_files_and_export_but_source_self_binding_does_not() {
        let fixture = fixture();
        let root = fixture.path().join("skill");
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: fixture\n---\nUse `assets/old-template.tex`.\n",
        )
        .unwrap();
        fs::write(root.join("assets/actual-template.tex"), "real template").unwrap();
        let mut manager = Manager::new(fixture.path().join("state")).unwrap();
        let mut skill = local::draft(&root).unwrap();
        skill["id"] = json!("fixture");
        manager.state.skills.push(skill);
        let report=crate::external::handle(&mut manager,&json!({"action":"get_external_dependencies","skillId":"fixture","includeRemote":false})).unwrap().unwrap();
        let item = &report["items"][0];
        let self_binding = crate::external::handle(
            &mut manager,
            &json!({"action":"bind_external_dependency","skillId":"fixture","dependencyId":item["id"],"role":"required","path":root.join("SKILL.md").to_string_lossy(),"includeRemote":false}),
        );
        assert!(self_binding.unwrap_err().contains("引用说明本身"));
        assert!(manager.state.skills[0]["externalBindings"].is_null());
        crate::external::handle(&mut manager,&json!({"action":"bind_external_dependency","skillId":"fixture","dependencyId":item["id"],"role":"required","path":root.join("assets/actual-template.tex").to_string_lossy(),"includeRemote":false})).unwrap();
        assert_eq!(manager.state.skills[0]["runtimeAudit"]["status"], "passed");
        assert_eq!(
            manager.state.skills[0]["runtimeAudit"]["issues"][0]["category"],
            "resolved"
        );
        assert_eq!(
            manager.state.skills[0]["runtimeAudit"]["rawIssues"][0]["category"],
            "package_required"
        );
        let files = local::handle(
            &mut manager,
            &json!({"action":"get_files","skillId":"fixture"}),
        )
        .unwrap()
        .unwrap();
        assert!(files["exportError"].is_null(), "{files}");
        assert!(files["issues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|issue| issue["severity"] != "error"));
        let prepared =
            crate::remote::attachments::prepare_export(&manager, &json!({"skillId":"fixture"}))
                .unwrap();
        assert_eq!(
            prepared.data["assets/actual-template.tex"].as_slice(),
            b"real template"
        );
        assert!(String::from_utf8(prepared.data["SKILL.md"].clone())
            .unwrap()
            .contains("assets/actual-template.tex"));
        assert_eq!(
            local::files(&root).unwrap().len(),
            2,
            "binding/export must not create local copies"
        );
        manager.state.skills[0]["externalBindings"] = json!([{"id":item["id"],"referenceFingerprint":item["referenceFingerprint"],"source":"SKILL.md","target":"assets/old-template.tex","role":"required","sourceKind":"local","path":root.join("SKILL.md").to_string_lossy()}]);
        refresh_local_audit(&mut manager.state.skills[0], None);
        assert_eq!(
            manager.state.skills[0]["runtimeAudit"]["status"],
            "needs_review"
        );
        assert_eq!(
            manager.state.skills[0]["runtimeAudit"]["externalDependencies"][0]["status"],
            "needs_selection"
        );
    }

    #[test]
    fn external_binding_file_removal_invalidates_unchanged_package_audit_cache() {
        let fixture = fixture();
        let root = fixture.path().join("skill");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: fixture\n---\nRead `assets/missing.txt`.\n",
        )
        .unwrap();
        let replacement = fixture.path().join("replacement.txt");
        fs::write(&replacement, "actual external file").unwrap();
        let mut manager = Manager::new(fixture.path().join("state")).unwrap();
        let mut skill = local::draft(&root).unwrap();
        skill["id"] = json!("fixture");
        manager.state.skills.push(skill);
        let report=crate::external::handle(&mut manager,&json!({"action":"get_external_dependencies","skillId":"fixture","includeRemote":false})).unwrap().unwrap();
        crate::external::handle(&mut manager,&json!({"action":"bind_external_dependency","skillId":"fixture","dependencyId":report["items"][0]["id"],"role":"required","path":replacement.to_string_lossy(),"includeRemote":false})).unwrap();
        let inventory = local::files(&root).unwrap();
        assert_eq!(manager.state.skills[0]["runtimeAudit"]["status"], "passed");
        assert!(!refresh_local_audit(
            &mut manager.state.skills[0],
            Some(&inventory)
        ));
        fs::remove_file(replacement).unwrap();
        assert!(refresh_local_audit(
            &mut manager.state.skills[0],
            Some(&inventory)
        ));
        assert_eq!(
            manager.state.skills[0]["runtimeAudit"]["status"],
            "needs_review"
        );
    }
    #[test]
    fn automatic_audit_keeps_optional_scope_and_never_invents_installation_or_invocation() {
        let fixture = fixture();
        let root = fixture.path().join("skill");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: fixture\n---\nUse the skill.\n",
        )
        .unwrap();
        fs::write(root.join("README.md"), "Optional saved choice.").unwrap();
        let mut skill = local::draft(&root).unwrap();
        skill["id"] = json!("fixture");
        skill["runtimeFiles"] = json!(["SKILL.md", "README.md"]);
        apply_local_audit(&mut skill).unwrap();
        assert_eq!(skill["runtimeConfirmed"], true);
        assert_eq!(skill["runtimeFiles"], json!(["README.md", "SKILL.md"]));
        assert_eq!(
            skill["runtimeAudit"]["optionalInScope"],
            json!(["README.md"])
        );
        assert!(skill["baselineCommit"].is_null());
        assert_eq!(skill["runtimeAudit"]["invocationStatus"], "not_verified");
        assert_eq!(
            local::minimal_runtime_files(&root).unwrap(),
            vec!["SKILL.md"]
        );
    }
    #[test]
    fn real_missing_dependency_blocks_auto_confirmation_and_failed_scan_preserves_manual_choices() {
        let fixture = fixture();
        let root = fixture.path().join("skill");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: fixture\n---\nRun `scripts/missing.py`.\n",
        )
        .unwrap();
        let mut skill = local::draft(&root).unwrap();
        apply_local_audit(&mut skill).unwrap();
        assert_eq!(skill["runtimeConfirmed"], false);
        assert_eq!(skill["runtimeAudit"]["status"], "needs_review");
        assert!(local::minimal_runtime_files(&root).is_err());
        skill["runtimeConfirmed"] = json!(true);
        skill["runtimeConfirmationKind"] = json!("manual");
        apply_local_audit(&mut skill).unwrap();
        assert_eq!(skill["runtimeConfirmed"], true);
        assert_eq!(skill["runtimeAudit"]["status"], "needs_review");
    }
    #[test]
    fn batch_audit_is_selected_persistent_and_does_not_change_runtime_bytes() {
        let fixture = fixture();
        let mut m = Manager::new(fixture.path().join("state")).unwrap();
        for name in ["one", "two"] {
            let root = fixture.path().join(name);
            fs::create_dir_all(&root).unwrap();
            fs::write(
                root.join("SKILL.md"),
                format!("---\nname: {name}\n---\nUse skill.\n"),
            )
            .unwrap();
            let mut skill = local::draft(&root).unwrap();
            skill["id"] = json!(name);
            m.state.skills.push(skill);
        }
        let before =
            local::files(Path::new(m.state.skills[0]["localPath"].as_str().unwrap())).unwrap();
        let result = handle(
            &mut m,
            &json!({"action":"audit_skills","skillIds":["one"],"localOnly":true}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(result["auditSummary"]["checked"], 1);
        assert_eq!(result["auditSummary"]["passed"], 1);
        assert_eq!(m.state.skills[0]["runtimeConfirmed"], true);
        assert_eq!(m.state.skills[1]["runtimeConfirmed"], false);
        assert_eq!(
            before,
            local::files(Path::new(m.state.skills[0]["localPath"].as_str().unwrap())).unwrap()
        );
        drop(m);
        let reopened = Manager::new(fixture.path().join("state")).unwrap();
        assert_eq!(reopened.state.skills[0]["runtimeAudit"]["status"], "passed");
        assert!(reopened.state.skills[0]["baselineCommit"].is_null());
    }
    #[test]
    fn newly_required_scope_invalidates_observation_without_shrinking_choices_or_install_history() {
        let fixture = fixture();
        let root = fixture.path().join("skill");
        fs::create_dir_all(root.join("references")).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: fixture\n---\nRead [rules](references/rules.md).\n",
        )
        .unwrap();
        fs::write(root.join("references/rules.md"), "Package rules.").unwrap();
        fs::write(
            root.join("README.md"),
            "User's previously saved optional choice.",
        )
        .unwrap();
        let mut skill = local::draft(&root).unwrap();
        skill["sourceId"] = json!("source");
        skill["runtimeFiles"] = json!(["SKILL.md", "README.md"]);
        skill["baselineCommit"] = json!("historical-install");
        skill["baselineHashes"] = json!({"SKILL.md":"historical"});
        skill["comparisonLocalHashes"] = json!({"SKILL.md":"previous"});
        apply_local_audit(&mut skill).unwrap();
        assert_eq!(
            skill["runtimeFiles"],
            json!(["README.md", "SKILL.md", "references/rules.md"])
        );
        assert!(skill["comparisonLocalHashes"].is_null());
        assert_eq!(skill["remoteCheckRequired"], true);
        assert_eq!(skill["baselineCommit"], "historical-install");
        assert_eq!(skill["baselineHashes"], json!({"SKILL.md":"historical"}));
        assert_eq!(skill["runtimeConfirmationKind"], "automatic");
        fs::remove_file(root.join("references/rules.md")).unwrap();
        apply_local_audit(&mut skill).unwrap();
        assert_eq!(skill["runtimeConfirmed"], false);
        assert_eq!(skill["runtimeAudit"]["status"], "needs_review");
        assert!(paths(&skill["runtimeFiles"]).contains("references/rules.md"));
    }

    #[test]
    fn ordinary_local_refresh_rechecks_changed_automatic_evidence_without_a_manual_step() {
        let fixture = fixture();
        let root = fixture.path().join("skill");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: fixture\n---\nOriginal rules.\n",
        )
        .unwrap();
        let mut m = Manager::new(fixture.path().join("state")).unwrap();
        let mut skill = local::draft(&root).unwrap();
        skill["id"] = json!("fixture");
        m.state.skills.push(skill);
        handle(&mut m, &json!({"action":"audit_skills","localOnly":true})).unwrap();
        assert_eq!(m.state.skills[0]["runtimeConfirmed"], true);
        fs::write(
            root.join("SKILL.md"),
            "---\nname: fixture\n---\nUpdated local rules.\n",
        )
        .unwrap();
        local::handle(&mut m, &json!({"action":"refresh","skillId":"fixture"})).unwrap();
        assert_eq!(m.state.skills[0]["runtimeConfirmed"], true);
        assert_eq!(m.state.skills[0]["runtimeAudit"]["status"], "passed");
        assert_eq!(m.state.skills[0]["runtimeAudit"]["canAutoConfirm"], true);
        assert_eq!(
            m.state.skills[0]["runtimeAudit"]["inventoryHashes"],
            json!(local::files(&root).unwrap())
        );
        assert!(m.state.skills[0]["baselineCommit"].is_null());
        handle(&mut m, &json!({"action":"audit_skills","localOnly":true})).unwrap();
        assert_eq!(m.state.skills[0]["runtimeAudit"]["status"], "passed");
    }

    #[test]
    fn registration_and_changed_inventory_reads_automatically_record_scope_once() {
        // The real task service configures the process transport before exposing
        // snapshots. Establish that same context before asserting full equality;
        // another parallel TaskSystem must not perform its first initialization
        // between these two otherwise identical inventory observations.
        crate::network::configure(&json!({})).unwrap();
        let fixture = fixture();
        let root = fixture.path().join("skill");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: fixture\n---\nUse the skill.\n",
        )
        .unwrap();
        let mut m = Manager::new(fixture.path().join("state")).unwrap();
        local::handle(
            &mut m,
            &json!({"action":"add_local","paths":[root.to_string_lossy()]}),
        )
        .unwrap();
        let skill = m.state.skills[0].clone();
        let id = skill["id"].clone();
        assert_eq!(skill["runtimeConfirmed"], true);
        assert_eq!(skill["runtimeConfirmationKind"], "automatic");
        assert_eq!(skill["runtimeFiles"], json!(["SKILL.md"]));
        assert_eq!(skill["runtimeAudit"]["method"], "static_package");
        assert_eq!(skill["runtimeAudit"]["invocationStatus"], "not_verified");
        assert!(skill["baselineCommit"].is_null());
        assert!(skill["installedCommit"].is_null());
        let before = m.snapshot();
        local::handle(&mut m, &json!({"action":"get_files","skillId":id})).unwrap();
        assert_eq!(
            m.snapshot(),
            before,
            "An unchanged inventory read must not save or replace its evidence"
        );
        fs::write(root.join("README.md"), "New optional documentation.").unwrap();
        let listed = local::handle(&mut m, &json!({"action":"get_files","skillId":id}))
            .unwrap()
            .unwrap();
        assert_eq!(listed["runtimeAudit"]["status"], "passed");
        assert!(listed["runtimeAudit"]["inventoryHashes"]["README.md"]
            .as_str()
            .is_some());
        assert_eq!(m.state.skills[0]["runtimeFiles"], json!(["SKILL.md"]));
        assert_ne!(m.snapshot()["stateRevision"], before["stateRevision"]);
        let refreshed = m.snapshot();
        local::handle(&mut m, &json!({"action":"get_files","skillId":id})).unwrap();
        assert_eq!(m.snapshot(), refreshed);
        let reopened = Manager::new(fixture.path().join("state")).unwrap();
        assert_eq!(
            reopened.state.skills[0]["runtimeAudit"],
            m.state.skills[0]["runtimeAudit"]
        );
        assert_eq!(
            fs::read_to_string(root.join("README.md")).unwrap(),
            "New optional documentation."
        );
    }

    #[test]
    fn manual_and_legacy_confirmed_ranges_remain_exact_when_new_required_files_are_detected() {
        for kind in [Value::Null, json!("manual"), json!("legacy_unknown")] {
            let fixture = fixture();
            let root = fixture.path().join("skill");
            fs::create_dir_all(root.join("references")).unwrap();
            fs::write(
                root.join("SKILL.md"),
                "---\nname: fixture\n---\nUse the skill.\n",
            )
            .unwrap();
            fs::write(root.join("README.md"), "Kept user selection.").unwrap();
            let mut m = Manager::new(fixture.path().join("state")).unwrap();
            let mut skill = local::draft(&root).unwrap();
            skill["id"] = json!("fixture");
            skill["runtimeConfirmed"] = json!(true);
            skill["runtimeConfirmationKind"] = kind.clone();
            skill["runtimeFiles"] = json!(["SKILL.md", "README.md"]);
            skill["sourceId"] = json!("source");
            skill["baselineCommit"] = json!("historical-baseline");
            skill["installedCommit"] = json!("historical-installation");
            skill["comparisonLocalHashes"] = json!({"SKILL.md":"old-proof"});
            m.upsert_skill(skill);
            local::handle(&mut m, &json!({"action":"get_files","skillId":"fixture"})).unwrap();
            fs::write(root.join("references/rules.md"), "Newly required rules.").unwrap();
            fs::write(
                root.join("SKILL.md"),
                "---\nname: fixture\n---\nRead [rules](references/rules.md).\n",
            )
            .unwrap();
            let listed = local::handle(&mut m, &json!({"action":"get_files","skillId":"fixture"}))
                .unwrap()
                .unwrap();
            let record = &m.state.skills[0];
            assert_eq!(record["runtimeFiles"], json!(["SKILL.md", "README.md"]));
            assert_eq!(
                record["runtimeConfirmed"], true,
                "A manual choice remains a choice, not an automatic proof"
            );
            assert_eq!(record["runtimeConfirmationKind"], kind);
            assert_eq!(record["runtimeAudit"]["status"], "needs_review");
            assert_eq!(record["runtimeAudit"]["canAutoConfirm"], false);
            assert!(record["runtimeAudit"]["reason"]
                .as_str()
                .unwrap()
                .contains("references/rules.md"));
            assert!(listed["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["category"] == "uncertain"
                    && issue["reason"]
                        .as_str()
                        .unwrap_or("")
                        .contains("手动文件范围")));
            assert_eq!(record["baselineCommit"], "historical-baseline");
            assert_eq!(record["installedCommit"], "historical-installation");
            assert!(record["comparisonLocalHashes"].is_null());
            assert_eq!(record["remoteCheckRequired"], true);
            let unchanged = m.snapshot();
            local::handle(&mut m, &json!({"action":"get_files","skillId":"fixture"})).unwrap();
            assert_eq!(
                m.snapshot(),
                unchanged,
                "A pending manual range is not repeatedly saved by unchanged reads"
            );
        }
    }

    #[test]
    fn newly_proven_automatic_scope_invalidates_only_comparison_evidence_and_never_history() {
        let fixture = fixture();
        let root = fixture.path().join("skill");
        fs::create_dir_all(root.join("references")).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: fixture\n---\nRead [rules](references/rules.md).\n",
        )
        .unwrap();
        let mut skill = local::draft(&root).unwrap();
        skill["runtimeConfirmationKind"] = json!("automatic");
        skill["runtimeFiles"] = json!(["SKILL.md"]);
        skill["sourceId"] = json!("source");
        skill["baselineCommit"] = json!("history");
        skill["baselineHashes"] = json!({"SKILL.md":"old"});
        skill["installedCommit"] = json!("installed-history");
        skill["comparisonLocalHashes"] = json!({"SKILL.md":"previous"});
        refresh_local_audit(&mut skill, None);
        assert_eq!(skill["runtimeAudit"]["status"], "needs_review");
        fs::write(root.join("references/rules.md"), "Now available.").unwrap();
        assert!(refresh_local_audit(
            &mut skill,
            Some(&local::files(&root).unwrap())
        ));
        assert_eq!(
            skill["runtimeFiles"],
            json!(["SKILL.md", "references/rules.md"])
        );
        assert_eq!(skill["runtimeConfirmed"], true);
        assert!(skill["comparisonLocalHashes"].is_null());
        assert_eq!(skill["remoteCheckRequired"], true);
        assert_eq!(skill["baselineCommit"], "history");
        assert_eq!(skill["baselineHashes"], json!({"SKILL.md":"old"}));
        assert_eq!(skill["installedCommit"], "installed-history");
        assert_eq!(skill["runtimeAudit"]["invocationStatus"], "not_verified");
    }

    #[test]
    fn missing_registered_root_read_persists_failure_without_changing_manual_scope_or_history() {
        for manual in [false, true] {
            let fixture = fixture();
            let root = fixture.path().join("skill");
            fs::create_dir_all(&root).unwrap();
            fs::write(
                root.join("SKILL.md"),
                "---\nname: fixture\n---\nUse the skill.\n",
            )
            .unwrap();
            let mut m = Manager::new(fixture.path().join("state")).unwrap();
            local::handle(
                &mut m,
                &json!({"action":"add_local","paths":[root.to_string_lossy()]}),
            )
            .unwrap();
            let id = m.state.skills[0]["id"].clone();
            if manual {
                local::handle(
                    &mut m,
                    &json!({"action":"set_runtime_files","skillId":id,"files":["SKILL.md"]}),
                )
                .unwrap();
            }
            m.state.skills[0]["baselineCommit"] = json!("saved-baseline");
            m.state.skills[0]["installedCommit"] = json!("saved-installation");
            let selected = m.state.skills[0]["runtimeFiles"].clone();
            fs::rename(&root, fixture.path().join("moved-skill")).unwrap();
            let error =
                local::handle(&mut m, &json!({"action":"get_files","skillId":id})).unwrap_err();
            let record = &m.state.skills[0];
            assert_eq!(record["runtimeConfirmed"], manual);
            assert_eq!(record["runtimeFiles"], selected);
            assert_eq!(record["runtimeAudit"]["status"], "failed");
            assert_eq!(record["runtimeAudit"]["reason"], error);
            assert_eq!(record["runtimeAudit"]["canAutoConfirm"], false);
            assert_eq!(record["runtimeAudit"]["invocationStatus"], "not_verified");
            assert_eq!(record["baselineCommit"], "saved-baseline");
            assert_eq!(record["installedCommit"], "saved-installation");
            let reopened = Manager::new(fixture.path().join("state")).unwrap();
            assert_eq!(
                reopened.state.skills[0]["runtimeAudit"],
                record["runtimeAudit"]
            );
            let failed_snapshot = m.snapshot();
            assert!(local::handle(&mut m, &json!({"action":"get_files","skillId":id})).is_err());
            assert_eq!(
                m.snapshot(),
                failed_snapshot,
                "An unchanged failed read must not repeatedly save the same failure"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_registered_file_read_persists_failed_evidence_without_bypassing_the_lock() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = fixture();
        let root = fixture.path().join("skill");
        fs::create_dir_all(&root).unwrap();
        let content = "---\nname: fixture\n---\nUse the skill.\n";
        fs::write(root.join("SKILL.md"), content).unwrap();
        let mut m = Manager::new(fixture.path().join("state")).unwrap();
        local::handle(
            &mut m,
            &json!({"action":"add_local","paths":[root.to_string_lossy()]}),
        )
        .unwrap();
        let id = m.state.skills[0]["id"].clone();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(root.join("SKILL.md"))
            .unwrap();
        let error = local::handle(&mut m, &json!({"action":"get_files","skillId":id})).unwrap_err();
        assert_eq!(m.state.skills[0]["runtimeAudit"]["status"], "failed");
        assert_eq!(m.state.skills[0]["runtimeConfirmed"], false);
        assert_eq!(m.state.skills[0]["runtimeAudit"]["reason"], error);
        drop(lock);
        assert_eq!(fs::read_to_string(root.join("SKILL.md")).unwrap(), content);
        let recovered = local::handle(&mut m, &json!({"action":"get_files","skillId":id}))
            .unwrap()
            .unwrap();
        assert_eq!(recovered["runtimeConfirmed"], true);
        assert_eq!(recovered["runtimeAudit"]["status"], "passed");
        assert_eq!(recovered["runtimeFiles"], json!(["SKILL.md"]));
    }

    #[test]
    fn real_single_writer_inventory_and_plan_reads_persist_evidence_even_when_a_later_read_fails() {
        let fixture = fixture();
        let root = fixture.path().join("skill");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: fixture\n---\nUse the skill.\n",
        )
        .unwrap();
        let mut m = Manager::new(fixture.path().join("state")).unwrap();
        let mut skill = local::draft(&root).unwrap();
        skill["id"] = json!("fixture");
        m.upsert_skill(skill);
        m.save().unwrap();
        let service = crate::tasks::TaskSystem::new(m).unwrap();
        let listed = service
            .execute(
                json!({"action":"get_files","skillId":"fixture"}),
                crate::dispatch,
            )
            .unwrap();
        assert_eq!(listed["runtimeConfirmed"], true);
        assert_eq!(listed["runtimeFiles"], json!(["SKILL.md"]));
        let snapshot = service
            .execute(json!({"action":"get_state"}), crate::dispatch)
            .unwrap();
        assert_eq!(snapshot["skills"][0]["runtimeAudit"]["status"], "passed");
        let plan = service
            .execute(
                json!({"action":"get_export_plan","skillId":"fixture","selectedIds":[]}),
                crate::dispatch,
            )
            .unwrap();
        assert_eq!(plan["canExport"], true);
        assert_eq!(
            service
                .execute(json!({"action":"get_state"}), crate::dispatch)
                .unwrap(),
            snapshot
        );
        fs::rename(&root, fixture.path().join("moved-skill")).unwrap();
        let error = service
            .execute(
                json!({"action":"get_files","skillId":"fixture"}),
                crate::dispatch,
            )
            .unwrap_err();
        let failed = service
            .execute(json!({"action":"get_state"}), crate::dispatch)
            .unwrap();
        assert_eq!(failed["skills"][0]["runtimeAudit"]["status"], "failed");
        assert_eq!(failed["skills"][0]["runtimeAudit"]["reason"], error);
        assert_eq!(failed["skills"][0]["runtimeConfirmed"], false);
        let reopened = Manager::new(fixture.path().join("state")).unwrap();
        assert_eq!(
            reopened.state.skills[0]["runtimeAudit"],
            failed["skills"][0]["runtimeAudit"]
        );
        assert!(reopened.state.skills[0]["baselineCommit"].is_null());
        assert!(reopened.state.skills[0]["installedCommit"].is_null());
    }
}
