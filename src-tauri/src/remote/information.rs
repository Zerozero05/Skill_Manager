//! Read-only provenance for source declarations. This never establishes runtime verification.
use super::{
    catalog_item, catalog_items, github, join, local_root, read_local, remote_path, repo, snapshot,
    source_for, text, Snapshot,
};
use crate::model::{now, Manager};
use serde_json::{json, Value};
use url::Url;

const MAX_DOCUMENT: usize = 1024 * 1024;

fn markdown(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.len() > MAX_DOCUMENT {
        return Err("Documentation exceeds the 1 MiB limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "This document is not UTF-8 text".into())
}

fn declarations(markdown: &str) -> Result<(Value, Value), String> {
    let mut lines = markdown.trim_start_matches('\u{feff}').lines();
    if lines.next().map(str::trim) != Some("---") {
        return Ok((Value::Null, Value::Null));
    }
    let mut yaml = String::new();
    for line in lines {
        if matches!(line.trim(), "---" | "...") {
            let meta: Value = serde_yaml::from_str(&yaml)
                .map_err(|_| "Local SKILL.md frontmatter cannot be read")?;
            return Ok((
                meta.get("compatibility")
                    .or_else(|| meta.get("dependencies"))
                    .cloned()
                    .unwrap_or(Value::Null),
                meta.get("verification")
                    .or_else(|| meta.get("metadata").and_then(|v| v.get("verification")))
                    .cloned()
                    .unwrap_or(Value::Null),
            ));
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    Err("Local SKILL.md frontmatter is incomplete".into())
}

fn local_information(skill: &Value) -> Value {
    let mut result =
        json!({"dependencies":null,"verification":null,"readStatus":"absent","source":"本地声明"});
    if skill["localPath"].as_str().is_none() {
        return result;
    }
    let read = (|| -> Result<(), String> {
        let root = local_root(skill)?;
        let entry = markdown(read_local(&root, "SKILL.md")?)?;
        let (dependencies, verification) = declarations(&entry)?;
        result["dependencies"] = dependencies;
        result["verification"] = verification;
        result["declarationPath"] = json!("SKILL.md");
        result["checkedAt"] = json!(now());
        let path = ["README.md", "readme.md", "Readme.md"]
            .into_iter()
            .find(|path| root.join(path).is_file())
            .unwrap_or("SKILL.md");
        let document = if path == "SKILL.md" {
            entry
        } else {
            markdown(read_local(&root, path)?)?
        };
        result["documentPath"] = json!(path);
        result["markdown"] = json!(document);
        result["readStatus"] = json!("ok");
        Ok(())
    })();
    match read {
        Ok(()) => result,
        Err(error) => {
            result["readStatus"] = json!("error");
            result["error"] = json!(error);
            result
        }
    }
}

fn catalog_information(catalog: Result<Option<Value>, String>, root: &str, path: &str) -> Value {
    match catalog {
        Ok(Some(value)) => {
            let items = catalog_items(Some(&value));
            let item = catalog_item(&items, root);
            json!({"readStatus":if item.is_some() {"ok"} else {"absent"},"documentPath":path,
                "dependencies":item.and_then(|v|v.get("dependencies").or_else(||v.get("compatibility"))),
                "verification":item.and_then(|v|v.get("verification")),"usage":item.and_then(|v|v.get("usage"))})
        }
        Ok(None) => {
            json!({"readStatus":"absent","documentPath":path,"dependencies":null,"verification":null})
        }
        Err(error) => {
            json!({"readStatus":"error","documentPath":path,"error":error,"dependencies":null,"verification":null})
        }
    }
}

fn documentation_location(
    skill: &Value,
    snap: &Snapshot,
    catalog: &Value,
) -> Result<Option<String>, String> {
    for path in [
        catalog["usage"].as_str(),
        skill["documentationPath"].as_str(),
    ]
    .into_iter()
    .flatten()
    {
        github::validate_relative(path)?;
        if snap.tree.contains_key(path) {
            return Ok(Some(path.into()));
        }
    }
    for leaf in ["README.md", "readme.md", "Readme.md", "SKILL.md"] {
        let path = join(remote_path(skill)?, leaf);
        if snap.tree.contains_key(&path) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn document_url(repository: &str, commit: &str, path: &str) -> Result<String, String> {
    let mut base = Url::parse("https://github.com/").expect("constant URL");
    {
        let mut parts = base
            .path_segments_mut()
            .map_err(|_| "Invalid documentation URL")?;
        for part in repository.split('/') {
            parts.push(part);
        }
        parts.push("blob");
        parts.push(commit);
        for part in path.split('/') {
            parts.push(part);
        }
    }
    Ok(base.to_string())
}

fn remote_information(skill: &Value, snap: &Snapshot) -> Result<Value, String> {
    let root = remote_path(skill)?;
    let catalog_path = ["skill-catalog.json", "skills/skill-catalog.json"]
        .into_iter()
        .find(|path| snap.tree.contains_key(*path))
        .unwrap_or("skill-catalog.json");
    // Catalog and documentation failures remain independent: one readable source can
    // still supply useful declarations when the other source cannot be read.
    let catalog = catalog_information(snap.catalog(), root, catalog_path);
    let document = match documentation_location(skill, snap, &catalog) {
        Ok(Some(path)) => match snap.read(&path).and_then(markdown) {
            Ok(contents) => json!({"readStatus":"ok","markdown":contents,"documentPath":path,
                "baseUrl":document_url(&snap.api.repo,&snap.commit,&path)?,"commit":snap.commit,
                "source":snap.api.repo,"private":snap.private,"checkedAt":now()}),
            Err(error) => json!({"readStatus":"error","error":error,"documentPath":path}),
        },
        Ok(None) => json!({"readStatus":"absent"}),
        Err(error) => json!({"readStatus":"error","error":error}),
    };
    let errors: Vec<&str> = [&catalog, &document]
        .into_iter()
        .filter_map(|v| v["error"].as_str())
        .collect();
    Ok(
        json!({"readStatus":if errors.is_empty(){"ok"}else{"error"},"remoteError":if errors.is_empty(){None}else{Some(errors.join("; "))},
        "commit":snap.commit,"checkedAt":now(),"source":snap.api.repo,"private":snap.private,
        "catalog":catalog,"document":document}),
    )
}

pub fn get(m: &Manager, request: &Value) -> Result<Value, String> {
    let skill = m.skill(text(request, "skillId")?)?;
    let local = local_information(&skill);
    let remote = if skill["sourceId"].as_str().is_none() || request["localOnly"] == true {
        json!({"readStatus":"not_applicable"})
    } else {
        let read = (|| -> Result<Value, String> {
            let source = source_for(m, &skill)?;
            if source["repo"] != skill["repo"] {
                return Err("Skill/source association is inconsistent".into());
            }
            let snap = match request.get("remoteCommit").filter(|v| !v.is_null()) {
                Some(value) => Snapshot::at_commit(
                    repo(&skill)?,
                    value.as_str().ok_or("Invalid remoteCommit")?,
                )?,
                None => match skill["commit"].as_str() {
                    Some(commit) => Snapshot::at_commit(repo(&skill)?, commit)?,
                    None => snapshot(m, &skill)?,
                },
            };
            remote_information(&skill, &snap)
        })();
        read.unwrap_or_else(|error| json!({"readStatus":"error","remoteError":error,
            "catalog":{"readStatus":"error","error":"远端读取未完成"},"document":{"readStatus":"error","error":"远端读取未完成"}}))
    };
    Ok(json!({"skillId":skill["id"],"local":local,"remote":remote}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn declarations_preserve_conditional_dependencies_and_separate_local_evidence() {
        let (dependencies, verification) = declarations("---\ncompatibility: 仅可选脚本需要 Python\nmetadata:\n  verification: 源码检查；未验证调用\n---\n# Body\n").unwrap();
        assert_eq!(dependencies, "仅可选脚本需要 Python");
        assert_eq!(verification, "源码检查；未验证调用");
        assert!(declarations("---\ncompatibility: a").is_err());
        assert_eq!(
            declarations("# No metadata").unwrap(),
            (Value::Null, Value::Null)
        );
    }

    #[test]
    fn catalog_absence_and_errors_are_distinct_from_structured_source_evidence() {
        let catalog = json!({"skills":[{"source":"s/SKILL.md","dependencies":{"python":"optional"},"verification":{"summary":"未验证业务调用"},"usage":"s/README.md"}]});
        let data = catalog_information(Ok(Some(catalog)), "s", "skill-catalog.json");
        assert_eq!(data["dependencies"]["python"], "optional");
        assert_eq!(data["verification"]["summary"], "未验证业务调用");
        assert_eq!(data["usage"], "s/README.md");
        assert_eq!(
            catalog_information(Ok(None), "s", "skill-catalog.json")["readStatus"],
            "absent"
        );
        assert_eq!(
            catalog_information(Err("network failed".into()), "s", "skill-catalog.json")
                ["readStatus"],
            "error"
        );
    }

    #[test]
    fn offline_read_prefers_readme_keeps_local_declarations_and_does_not_mutate_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("skill");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\ncompatibility: no extra dependencies\n---\n# Runtime",
        )
        .unwrap();
        fs::write(root.join("README.md"), "## 验证与局限\n未验证调用\n").unwrap();
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        manager.state.skills.push(json!({"id":"s","localPath":root,"lastLocalCheckedAt":"old-local","lastRemoteCheckedAt":"old-remote"}));
        let before = manager.snapshot();
        let result = get(&manager, &json!({"skillId":"s"})).unwrap();
        assert_eq!(result["local"]["documentPath"], "README.md");
        assert_eq!(result["local"]["dependencies"], "no extra dependencies");
        assert!(result["local"]["markdown"]
            .as_str()
            .unwrap()
            .contains("未验证调用"));
        assert_eq!(result["remote"]["readStatus"], "not_applicable");
        assert_eq!(manager.snapshot(), before);
    }

    #[test]
    fn rejected_remote_commit_retains_local_information_and_never_falls_back_to_latest() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("skill");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\ncompatibility: Local only\n---\n# Runtime",
        )
        .unwrap();
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        manager
            .state
            .sources
            .push(json!({"id":"remote","repo":"synthetic/repo"}));
        manager.state.skills.push(json!({"id":"s","localPath":root,"sourceId":"remote","repo":"synthetic/repo","remotePath":"s"}));
        let before = manager.snapshot();
        let result = get(&manager, &json!({"skillId":"s","remoteCommit":"invalid"})).unwrap();
        assert_eq!(result["local"]["dependencies"], "Local only");
        assert_eq!(result["remote"]["readStatus"], "error");
        assert!(result["remote"]["remoteError"]
            .as_str()
            .unwrap()
            .contains("object identifier"));
        assert_eq!(manager.snapshot(), before);
    }

    #[test]
    fn cached_remote_can_be_combined_with_fresh_local_declarations_without_network() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("skill");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\ncompatibility: Old declaration\n---\n# Runtime",
        )
        .unwrap();
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        manager.state.skills.push(json!({"id":"s","localPath":root,"sourceId":"missing-source","dependencies":"Stale registered value"}));
        let first = get(&manager, &json!({"skillId":"s","localOnly":true})).unwrap();
        assert_eq!(first["local"]["dependencies"], "Old declaration");
        fs::write(
            root.join("SKILL.md"),
            "---\ncompatibility: New declaration\n---\n# Runtime",
        )
        .unwrap();
        let second = get(&manager, &json!({"skillId":"s","localOnly":true})).unwrap();
        assert_eq!(second["local"]["dependencies"], "New declaration");
        assert_eq!(second["remote"]["readStatus"], "not_applicable");
    }
}
