use super::github::validate_relative;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn compare(
    baseline: Option<&BTreeMap<String, String>>,
    local: &BTreeMap<String, String>,
    remote: &BTreeMap<String, String>,
    docs_changed: bool,
) -> &'static str {
    let Some(base) = baseline else {
        return "unknown_baseline";
    };
    if local == remote {
        return if docs_changed {
            "docs_updated"
        } else {
            "synced"
        };
    }
    match (local != base, remote != base) {
        (true, true) => "both_modified",
        (true, false) => "local_modified",
        (false, true) => "remote_updated",
        (false, false) => "synced",
    }
}

pub fn checked_selection(files: &[String]) -> Result<Vec<String>, String> {
    if files.is_empty() {
        return Err("Choose the runtime files before continuing".into());
    }
    if files.len() > 2000 {
        return Err("A runtime package may contain at most 2000 files".into());
    }
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for f in files {
        validate_relative(f)?;
        if !seen.insert(f.to_lowercase()) {
            return Err(format!("Duplicate or case-colliding path: {f}"));
        }
        result.push(f.clone());
    }
    result.sort();
    Ok(result)
}

pub struct UpdateSafety {
    pub local_changes: bool,
    pub conflicts: BTreeSet<String>,
    pub deletes: Vec<String>,
}

/// History outside the selected runtime is retained as a receipt, not as authority
/// to remove omitted local files. Only actual upstream removals remain deletions.
pub fn update_safety(
    baseline: Option<&BTreeMap<String, String>>,
    local: &BTreeMap<String, String>,
    old_runtime: &[String],
    selected_remote: &BTreeMap<String, String>,
    available_remote: &BTreeSet<String>,
) -> UpdateSafety {
    let managed: BTreeSet<_> = old_runtime.iter().collect();
    let deletes: Vec<String> = baseline
        .into_iter()
        .flat_map(|base| base.keys())
        .filter(|path| managed.contains(path) && !available_remote.contains(*path))
        .cloned()
        .collect();
    let local_changes = baseline.is_some_and(|base| {
        base.iter().any(|(path, hash)| {
            (selected_remote.contains_key(path) || deletes.contains(path))
                && local.get(path) != Some(hash)
        })
    });
    let conflicts = selected_remote
        .iter()
        .filter_map(|(path, hash)| {
            let untracked = baseline.is_none_or(|base| !base.contains_key(path));
            (untracked && local.get(path).is_some_and(|current| current != hash))
                .then(|| path.clone())
        })
        .collect();
    UpdateSafety {
        local_changes,
        conflicts,
        deletes,
    }
}

/// Explicit package-relative references only. Absolute machine paths and external URLs are not packaged.
#[cfg(test)]
pub fn referenced_files(text: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let mut fenced = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        for raw in line.split(|c: char| c.is_whitespace() || "`\"'()[]<>".contains(c)) {
            let token = raw.trim_end_matches([',', ';', ':', '.']);
            let token = token.strip_prefix("./").unwrap_or(token);
            if ![
                "scripts/",
                "references/",
                "assets/",
                "templates/",
                "agents/",
            ]
            .iter()
            .any(|p| token.starts_with(p))
            {
                continue;
            }
            let token = token
                .split(['#', '?'])
                .next()
                .unwrap_or("")
                .trim_end_matches('/');
            // Ignore placeholders in examples; they do not identify an installable dependency.
            if token.contains(['{', '}', '*', '$']) || token.contains("...") {
                continue;
            }
            if validate_relative(token).is_ok() {
                result.insert(token.to_owned());
            }
        }
    }
    result
}

pub fn disallowed_upload(path: &str) -> bool {
    path.split('/').any(|p| {
        let p = p.to_ascii_lowercase();
        p == ".git"
            || p == ".env"
            || (p.starts_with(".env.") && !p.ends_with(".example"))
            || [
                "id_rsa",
                "id_ed25519",
                "credentials.json",
                "manager.sqlite3",
                ".ssh",
                "node_modules",
                "__pycache__",
            ]
            .contains(&p.as_str())
            || p.ends_with(".pem")
            || p.ends_with(".key")
    })
}

pub fn selected_changes(
    selected: &[String],
    local: &BTreeMap<String, String>,
    remote: &BTreeMap<String, String>,
    baseline: &BTreeMap<String, String>,
) -> Result<Vec<(String, &'static str)>, String> {
    let selected = checked_selection(selected)?;
    selected
        .into_iter()
        .map(|p| {
            if disallowed_upload(&p) {
                return Err(format!("This file is not allowed in a Skill upload: {p}"));
            }
            let l = local.get(&p);
            let r = remote.get(&p);
            let change = match (l, r) {
                (Some(a), Some(b)) if a == b => "unchanged",
                (Some(_), Some(_)) => "modified",
                (Some(_), None) => "added",
                (None, Some(_)) if baseline.contains_key(&p) => "deleted",
                (None, Some(_)) => {
                    return Err(format!(
                        "A missing local file cannot delete an untracked remote file: {p}"
                    ))
                }
                (None, None) => return Err(format!("The selected file does not exist: {p}")),
            };
            Ok((p, change))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hashes(items: &[(&str, &str)]) -> BTreeMap<String, String> {
        items
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }
    #[test]
    fn three_way_status_has_no_invented_baseline() {
        let b = hashes(&[("SKILL.md", "a")]);
        let l = hashes(&[("SKILL.md", "b")]);
        let r = hashes(&[("SKILL.md", "c")]);
        assert_eq!(compare(None, &b, &b, false), "unknown_baseline");
        assert_eq!(compare(Some(&b), &l, &b, false), "local_modified");
        assert_eq!(compare(Some(&b), &b, &r, false), "remote_updated");
        assert_eq!(compare(Some(&b), &l, &r, false), "both_modified");
        assert_eq!(compare(Some(&b), &l, &l, false), "synced");
        assert_eq!(compare(Some(&b), &b, &b, true), "docs_updated");
    }
    #[test]
    fn omitted_remote_readme_is_never_a_deletion() {
        let local = hashes(&[("SKILL.md", "b")]);
        let remote = hashes(&[("SKILL.md", "a"), ("README.md", "documentation")]);
        let result =
            selected_changes(&["SKILL.md".into()], &local, &remote, &BTreeMap::new()).unwrap();
        assert_eq!(result, vec![("SKILL.md".into(), "modified")]);
        assert!(
            selected_changes(&["README.md".into()], &local, &remote, &BTreeMap::new()).is_err()
        );
    }
    #[test]
    fn only_baseline_owned_selected_files_can_be_deleted() {
        let remote = hashes(&[("old.py", "old")]);
        let base = remote.clone();
        assert_eq!(
            selected_changes(&["old.py".into()], &BTreeMap::new(), &remote, &base).unwrap()[0].1,
            "deleted"
        );
        assert!(selected_changes(
            &[".env".into()],
            &hashes(&[(".env", "secret")]),
            &BTreeMap::new(),
            &base
        )
        .is_err());
    }
    #[test]
    fn references_and_case_collision_are_checked() {
        let refs = referenced_files(
            "Read `references/rules.md` and [run](scripts/a.py#usage), assets/icon.svg",
        );
        assert!(refs.contains("references/rules.md"));
        assert!(refs.contains("scripts/a.py"));
        assert!(checked_selection(&["SKILL.md".into(), "skill.md".into()]).is_err());
        let refs =
            referenced_files("```python\nrun scripts/example.py\n```\nRead `./assets/icon.svg`");
        assert!(!refs.contains("scripts/example.py"));
        assert!(refs.contains("assets/icon.svg"));
    }
    #[test]
    fn update_scope_changes_preserve_cancelled_readme_and_ignore_old_receipt_keys() {
        let base = hashes(&[("SKILL.md", "entry"), ("README.md", "original docs")]);
        let local = hashes(&[("SKILL.md", "entry"), ("README.md", "my retained docs")]);
        let remote = hashes(&[("SKILL.md", "updated entry")]);
        let available = BTreeSet::from(["SKILL.md".into(), "README.md".into()]);
        for old_runtime in [
            vec!["SKILL.md".into()],
            vec!["SKILL.md".into(), "README.md".into()],
        ] {
            let safety = update_safety(Some(&base), &local, &old_runtime, &remote, &available);
            assert!(!safety.local_changes);
            assert!(safety.conflicts.is_empty());
            assert!(
                safety.deletes.is_empty(),
                "Omission is not upstream deletion"
            );
        }
        // Re-selecting a previously managed file must still protect its changed local bytes.
        let selected = hashes(&[("SKILL.md", "entry"), ("README.md", "upstream docs")]);
        assert!(
            update_safety(
                Some(&base),
                &local,
                &["SKILL.md".into()],
                &selected,
                &available
            )
            .local_changes
        );
    }
    #[test]
    fn new_runtime_files_need_equal_bytes_before_existing_content_can_be_adopted() {
        let base = hashes(&[("SKILL.md", "entry")]);
        let remote = hashes(&[("SKILL.md", "entry"), ("scripts/new.py", "remote")]);
        let available = remote.keys().cloned().collect();
        let equal = update_safety(
            Some(&base),
            &remote,
            &["SKILL.md".into(), "scripts/new.py".into()],
            &remote,
            &available,
        );
        assert!(!equal.local_changes);
        assert!(equal.conflicts.is_empty());
        let local = hashes(&[("SKILL.md", "entry"), ("scripts/new.py", "local")]);
        let different = update_safety(
            Some(&base),
            &local,
            &["SKILL.md".into(), "scripts/new.py".into()],
            &remote,
            &available,
        );
        assert!(different.conflicts.contains("scripts/new.py"));
    }
    #[test]
    fn real_upstream_deletion_requires_current_management_and_verified_history() {
        let base = hashes(&[("SKILL.md", "entry"), ("old.py", "old")]);
        let remote = hashes(&[("SKILL.md", "updated")]);
        let available = remote.keys().cloned().collect();
        let managed = vec!["SKILL.md".into(), "old.py".into()];
        let safety = update_safety(Some(&base), &base, &managed, &remote, &available);
        assert_eq!(safety.deletes, vec!["old.py"]);
        assert!(!safety.local_changes);
        assert!(update_safety(
            Some(&base),
            &base,
            &["SKILL.md".into()],
            &remote,
            &available
        )
        .deletes
        .is_empty());
        assert!(update_safety(None, &base, &managed, &remote, &available)
            .deletes
            .is_empty());
        let changed = hashes(&[("SKILL.md", "entry"), ("old.py", "my changed file")]);
        assert!(update_safety(Some(&base), &changed, &managed, &remote, &available).local_changes);
    }
}
