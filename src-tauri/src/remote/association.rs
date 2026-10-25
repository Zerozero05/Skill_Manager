//! Identity and change tracking are deliberately separate from installed-version provenance.
use super::{logic, mapping, paths};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub type Hashes = BTreeMap<String, String>;

/// A verified installation replaces comparison history, while retaining identity provenance.
pub fn reset_observation(skill: &mut Value, scope: &[String]) {
    for field in [
        "comparisonLocalHashes",
        "comparisonRemoteHashes",
        "comparisonObservedAt",
        "comparisonRuntimeFiles",
        "comparisonRuntimeSelection",
        "comparisonCommit",
        "comparisonDocHash",
    ] {
        skill[field] = Value::Null;
    }
    skill["comparisonScope"] = json!(scope);
    skill["changeDetails"] = json!([]);
    skill["comparisonNote"] = Value::Null;
    skill["comparisonScopeUnverified"] = json!(false);
    skill["comparisonPending"] = json!(false);
    skill["associationNote"] = skill["associationReason"].clone();
}

/// The first documentation read establishes an observation, rather than an update.
pub fn documentation_changed(skill: &mut Value, current: &str) -> bool {
    if skill["baselineCommit"].as_str().is_some() {
        return skill["baselineDocHash"]
            .as_str()
            .is_some_and(|old| old != current);
    }
    match skill["comparisonDocHash"].as_str() {
        Some(old) => old != current,
        None => {
            skill["comparisonDocHash"] = json!(current);
            false
        }
    }
}

pub fn internal_name(bytes: &[u8]) -> Result<String, String> {
    let content = std::str::from_utf8(bytes).map_err(|_| "SKILL.md is not UTF-8 text")?;
    let mut lines = content.trim_start_matches('\u{feff}').lines();
    if lines.next().map(str::trim) != Some("---") {
        return Err("SKILL.md has no explicit YAML identity".into());
    }
    let mut yaml = String::new();
    let mut ended = false;
    for line in lines {
        if line.trim() == "---" {
            ended = true;
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    if !ended {
        return Err("SKILL.md has unterminated YAML identity".into());
    }
    let value: serde_yaml::Value =
        serde_yaml::from_str(&yaml).map_err(|_| "SKILL.md has invalid YAML identity")?;
    let name = value
        .get("name")
        .and_then(serde_yaml::Value::as_str)
        .ok_or("SKILL.md has no explicit internal name")?;
    if name.is_empty()
        || name != name.trim()
        || name.len() > 128
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err("SKILL.md has an invalid internal name".into());
    }
    Ok(name.to_owned())
}

pub fn folder_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
}

#[derive(Clone)]
pub struct Identity {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub eligible: bool,
    pub verified: bool,
}

/// Unknown/failed candidates may block uniqueness, but can never establish a match.
pub fn unique_pairs(local: &[Identity], remote: &[Identity]) -> Vec<(String, String)> {
    let mut left: BTreeMap<&str, Vec<&Identity>> = BTreeMap::new();
    let mut right: BTreeMap<&str, Vec<&Identity>> = BTreeMap::new();
    for item in local {
        if !item.name.is_empty() {
            left.entry(&item.name).or_default().push(item);
        }
    }
    for item in remote {
        if !item.name.is_empty() {
            right.entry(&item.name).or_default().push(item);
        }
    }
    left.into_iter()
        .filter_map(|(name, ls)| {
            let rs = right.get(name)?;
            if ls.len() != 1
                || rs.len() != 1
                || !ls[0].eligible
                || !rs[0].eligible
                || !ls[0].verified
                || !rs[0].verified
            {
                return None;
            }
            Some((ls[0].id.clone(), rs[0].id.clone()))
        })
        .collect()
}

fn change(before: Option<&String>, after: Option<&String>) -> &'static str {
    match (before, after) {
        (None, Some(_)) => "added",
        (Some(_), None) => "deleted",
        _ => "modified",
    }
}

pub fn details(
    local_base: &Hashes,
    remote_base: &Hashes,
    local: &Hashes,
    remote: &Hashes,
    show_initial_difference: bool,
) -> Vec<Value> {
    let keys: BTreeSet<_> = local_base
        .keys()
        .chain(remote_base.keys())
        .chain(local.keys())
        .chain(remote.keys())
        .cloned()
        .collect();
    keys.into_iter()
        .filter_map(|path| {
            let lc = local_base.get(&path) != local.get(&path);
            let rc = remote_base.get(&path) != remote.get(&path);
            let (kind, side) = match (lc, rc) {
                (true, true) => (
                    if local.get(&path) == remote.get(&path) {
                        change(local_base.get(&path), local.get(&path))
                    } else {
                        "modified"
                    },
                    "both",
                ),
                (true, false) => (change(local_base.get(&path), local.get(&path)), "local"),
                (false, true) => (change(remote_base.get(&path), remote.get(&path)), "remote"),
                (false, false)
                    if show_initial_difference && local.get(&path) != remote.get(&path) =>
                {
                    ("different", "both")
                }
                _ => return None,
            };
            Some(json!({"path":path,"change":kind,"side":side}))
        })
        .collect()
}

/// First observations of two different trees do not identify which side is newer.
/// Later changes are measured against each side's own immutable first observation.
pub fn compare_observed(
    skill: &mut Value,
    local: &Hashes,
    remote: &Hashes,
    scope: &[String],
    docs_changed: bool,
) -> &'static str {
    if skill["baselineCommit"].as_str().is_some() {
        let all_base = mapping(&skill["baselineHashes"]);
        let base: Hashes = scope
            .iter()
            .filter_map(|p| all_base.get(p).map(|h| (p.clone(), h.clone())))
            .collect();
        let known_local: Hashes = local
            .iter()
            .filter(|(p, _)| base.contains_key(*p))
            .map(|(p, h)| (p.clone(), h.clone()))
            .collect();
        let known_remote: Hashes = remote
            .iter()
            .filter(|(p, _)| base.contains_key(*p))
            .map(|(p, h)| (p.clone(), h.clone()))
            .collect();
        let mut changed = details(&base, &base, &known_local, &known_remote, false);
        let unknown: Vec<_> = scope
            .iter()
            .filter(|p| !all_base.contains_key(*p) && local.get(*p) != remote.get(*p))
            .cloned()
            .collect();
        for path in &unknown {
            changed.push(json!({"path":path,"change":"different","side":"both"}));
        }
        skill["changeDetails"] = json!(changed);
        skill["comparisonScopeUnverified"] = json!(!unknown.is_empty());
        if !unknown.is_empty() {
            skill["comparisonNote"] =
                json!("新增运行范围缺少该文件的安装历史，两侧当前内容不同，需核对后再决定更新方向");
            return "unknown_baseline";
        }
        skill["comparisonNote"] = Value::Null;
        return logic::compare(Some(&base), &known_local, &known_remote, docs_changed);
    }
    skill["comparisonNote"] = Value::Null;
    skill["comparisonScopeUnverified"] = json!(false);
    let selection = paths(&skill["runtimeFiles"]);
    let selection_changed = skill["comparisonRuntimeSelection"].is_array()
        && paths(&skill["comparisonRuntimeSelection"]) != selection;
    if !skill["comparisonLocalHashes"].is_object()
        || !skill["comparisonRemoteHashes"].is_object()
        || selection_changed
    {
        skill["comparisonLocalHashes"] = json!(local);
        skill["comparisonRemoteHashes"] = json!(remote);
        skill["comparisonObservedAt"] = json!(crate::model::now());
        skill["comparisonRuntimeFiles"] = json!(scope);
        skill["comparisonRuntimeSelection"] = json!(selection);
        skill["comparisonCommit"] = skill["commit"].clone();
    }
    let left = mapping(&skill["comparisonLocalHashes"]);
    let right = mapping(&skill["comparisonRemoteHashes"]);
    skill["changeDetails"] = json!(details(&left, &right, local, remote, true));
    if local == remote {
        return if docs_changed {
            "docs_updated"
        } else {
            "synced"
        };
    }
    match (local != &left, remote != &right) {
        (true, false) => "local_modified",
        (false, true) => "remote_updated",
        (true, true) => "both_modified",
        _ => "unknown_baseline",
    }
}

pub fn merge(local: &Value, remote: &Value, note: &str) -> Value {
    let mut merged = local.clone();
    if ["sourceId", "repo", "branch", "remotePath"]
        .iter()
        .any(|field| local[field] != remote[field])
    {
        merged["lastRemoteCheckedAt"] = Value::Null;
    }
    for field in [
        "sourceId",
        "remotePath",
        "repo",
        "branch",
        "commit",
        "private",
        "documentationPath",
        "catalogMetadata",
        "verifiedInternalName",
    ] {
        merged[field] = remote[field].clone();
    }
    if !local["categoryId"]
        .as_str()
        .is_some_and(|id| !id.is_empty())
    {
        merged["categoryId"] = remote["categoryId"].clone();
    }
    merged["favorite"] = json!(local["favorite"] == true || remote["favorite"] == true);
    // Installation provenance and runtime choices belong to the retained local record.
    merged["associationNote"] = json!(note);
    merged["associationPending"] = json!(false);
    merged["remoteMissing"] = json!(false);
    merged["remoteCheckRequired"] = json!(true);
    merged["error"] = Value::Null;
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hashes(value: &str) -> Hashes {
        BTreeMap::from([("SKILL.md".into(), value.into())])
    }
    fn id(id: &str, name: &str) -> Identity {
        Identity {
            id: id.into(),
            name: name.into(),
            folder: name.into(),
            eligible: true,
            verified: true,
        }
    }
    #[test]
    fn stable_yaml_identity_never_uses_display_or_folder_fallback() {
        assert_eq!(
            internal_name(b"---\nname: real-name\ndisplayName: Other\n---\n").unwrap(),
            "real-name"
        );
        assert!(internal_name(b"---\ndisplayName: fake\n---\n").is_err());
        assert!(internal_name(b"# name: fake").is_err());
    }
    #[test]
    fn ambiguous_failed_and_managed_identities_never_pair() {
        assert_eq!(unique_pairs(&[id("l", "one")], &[id("r", "one")]).len(), 1);
        assert!(unique_pairs(&[id("l", "one"), id("l2", "one")], &[id("r", "one")]).is_empty());
        assert!(unique_pairs(&[id("l", "one")], &[id("r", "one"), id("r2", "one")]).is_empty());
        let mut failed = id("r", "one");
        failed.verified = false;
        assert!(unique_pairs(&[id("l", "one")], &[failed]).is_empty());
        let mut managed = id("l", "one");
        managed.eligible = false;
        assert!(unique_pairs(&[managed], &[id("r", "one")]).is_empty());
    }
    #[test]
    fn differing_first_observations_do_not_invent_direction_or_installation() {
        let mut skill = json!({"runtimeFiles":["SKILL.md"],"commit":"remote-commit"});
        assert_eq!(
            compare_observed(
                &mut skill,
                &hashes("local"),
                &hashes("remote"),
                &["SKILL.md".into()],
                false
            ),
            "unknown_baseline"
        );
        assert!(skill["baselineCommit"].is_null());
        assert_eq!(skill["changeDetails"][0]["change"], "different");
        assert_eq!(skill["comparisonLocalHashes"]["SKILL.md"], "local");
        assert_eq!(skill["comparisonRemoteHashes"]["SKILL.md"], "remote");
    }
    #[test]
    fn subsequent_observation_changes_identify_each_side_without_rewriting_first_observation() {
        let initial = json!({"runtimeFiles":["SKILL.md"]});
        let mut observed = initial;
        compare_observed(
            &mut observed,
            &hashes("left"),
            &hashes("right"),
            &["SKILL.md".into()],
            false,
        );
        let mut local = observed.clone();
        assert_eq!(
            compare_observed(
                &mut local,
                &hashes("edited"),
                &hashes("right"),
                &["SKILL.md".into()],
                false
            ),
            "local_modified"
        );
        assert_eq!(local["changeDetails"][0]["side"], "local");
        let mut remote = observed.clone();
        assert_eq!(
            compare_observed(
                &mut remote,
                &hashes("left"),
                &hashes("updated"),
                &["SKILL.md".into()],
                false
            ),
            "remote_updated"
        );
        assert_eq!(remote["changeDetails"][0]["side"], "remote");
        let mut both = observed.clone();
        assert_eq!(
            compare_observed(
                &mut both,
                &hashes("edited"),
                &hashes("updated"),
                &["SKILL.md".into()],
                false
            ),
            "both_modified"
        );
        assert_eq!(both["changeDetails"][0]["side"], "both");
        assert_eq!(both["comparisonLocalHashes"]["SKILL.md"], "left");
        assert!(both["baselineCommit"].is_null());
        assert_eq!(
            compare_observed(
                &mut observed,
                &hashes("same"),
                &hashes("same"),
                &["SKILL.md".into()],
                false
            ),
            "synced"
        );
    }
    #[test]
    fn merging_keeps_local_identity_runtime_and_connection_targets() {
        let local = json!({"id":"local-id","localPath":"D:/skills/one","name":"one","displayName":"Custom","runtimeFiles":["SKILL.md"],"runtimeConfirmed":true,"baselineCommit":null});
        let remote = json!({"id":"remote-id","sourceId":"s","repo":"o/r","remotePath":"one","branch":"main","commit":"c"});
        let merged = merge(&local, &remote, "verified");
        for field in [
            "id",
            "localPath",
            "name",
            "displayName",
            "runtimeFiles",
            "runtimeConfirmed",
            "baselineCommit",
        ] {
            assert_eq!(merged[field], local[field]);
        }
        let connections =
            json!([{"id":"connection-id","skillId":"local-id","target":"D:/skills/one"}]);
        assert_eq!(connections[0]["skillId"], merged["id"]);
        assert_eq!(connections[0]["id"], "connection-id");
        assert_eq!(merged["sourceId"], "s");
    }

    #[test]
    fn merging_preserves_a_local_category_and_inherits_only_when_unclassified() {
        let remote = json!({"id":"remote","categoryId":"remote-category"});
        let local = json!({"id":"local","categoryId":"local-category"});
        assert_eq!(
            merge(&local, &remote, "explicit")["categoryId"],
            "local-category"
        );
        for category in [Value::Null, json!("")] {
            let local = json!({"id":"local","categoryId":category});
            let merged = merge(&local, &remote, "automatic");
            assert_eq!(merged["categoryId"], "remote-category");
            assert_eq!(merged["id"], "local");
        }
        assert_eq!(
            merge(&json!({"id":"local"}), &remote, "automatic")["categoryId"],
            "remote-category"
        );
        assert!(
            merge(&json!({"id":"local"}), &json!({"id":"remote"}), "automatic")["categoryId"]
                .is_null()
        );
    }

    #[test]
    fn merging_keeps_favorites_from_either_record_and_defaults_legacy_to_false() {
        for local in [Value::Null, json!(false), json!(true)] {
            for remote in [Value::Null, json!(false), json!(true)] {
                let merged = merge(
                    &json!({"id":"local","favorite":local,"categoryId":"mine"}),
                    &json!({"id":"remote","favorite":remote,"categoryId":"theirs"}),
                    "verified",
                );
                assert_eq!(merged["favorite"], local == true || remote == true);
                assert_eq!(merged["id"], "local");
                assert_eq!(merged["categoryId"], "mine");
            }
        }
        assert_eq!(
            merge(&json!({"id":"local"}), &json!({"id":"remote"}), "verified")["favorite"],
            false
        );
    }

    #[test]
    fn changing_a_remote_target_invalidates_only_its_successful_check_time() {
        let original = json!({"id":"local","sourceId":"source","repo":"old/repo","branch":"main","remotePath":"skill","lastRemoteCheckedAt":"2001-01-01T00:00:00Z","lastLocalCheckedAt":"2002-01-01T00:00:00Z"});
        let same = merge(&original, &original, "same target");
        assert_eq!(same["lastRemoteCheckedAt"], original["lastRemoteCheckedAt"]);
        for field in ["sourceId", "repo", "branch", "remotePath"] {
            let mut target = original.clone();
            target[field] = json!("changed");
            let merged = merge(&original, &target, "new target");
            assert!(merged["lastRemoteCheckedAt"].is_null(), "{field}");
            assert_eq!(merged["lastLocalCheckedAt"], original["lastLocalCheckedAt"]);
        }
    }
    #[test]
    fn changing_runtime_scope_does_not_invent_deletions_or_new_file_history() {
        let installed = json!({"baselineCommit":"installed","baselineHashes":{"SKILL.md":"same","README.md":"doc"},"runtimeFiles":["SKILL.md"]});
        let mut narrowed = installed.clone();
        assert_eq!(
            compare_observed(
                &mut narrowed,
                &hashes("same"),
                &hashes("same"),
                &["SKILL.md".into()],
                false
            ),
            "synced"
        );
        assert!(narrowed["changeDetails"].as_array().unwrap().is_empty());
        let scope = vec!["SKILL.md".into(), "scripts/new.py".into()];
        let mut expanded = installed.clone();
        let left = BTreeMap::from([
            ("SKILL.md".into(), "same".into()),
            ("scripts/new.py".into(), "local".into()),
        ]);
        let right = BTreeMap::from([
            ("SKILL.md".into(), "same".into()),
            ("scripts/new.py".into(), "remote".into()),
        ]);
        assert_eq!(
            compare_observed(&mut expanded, &left, &right, &scope, false),
            "unknown_baseline"
        );
        assert_eq!(expanded["changeDetails"][0]["path"], "scripts/new.py");
        assert_eq!(expanded["changeDetails"][0]["change"], "different");
        assert_eq!(
            compare_observed(&mut expanded, &left, &left, &scope, false),
            "synced"
        );
        assert_eq!(expanded["comparisonScopeUnverified"], false);
        assert!(expanded["baselineHashes"]["scripts/new.py"].is_null());
    }
}
