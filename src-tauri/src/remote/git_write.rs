//! Git database submission is isolated so branch movement and selected-file semantics can
//! be tested without credentials, HTTP or modifications to a real repository.
use super::github::{self, Api};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub trait GitWriter {
    fn head(&self, branch: &str) -> Result<String, String>;
    fn create(&self, path: &[&str], body: &Value) -> Result<Value, String>;
    fn update(&self, branch: &str, commit: &str, force: bool) -> Result<(), String>;
}
impl GitWriter for Api {
    fn head(&self, branch: &str) -> Result<String, String> {
        Api::head(self, branch)
    }
    fn create(&self, path: &[&str], body: &Value) -> Result<Value, String> {
        self.post(path, body)
    }
    fn update(&self, branch: &str, commit: &str, force: bool) -> Result<(), String> {
        let mut path = vec!["git", "refs", "heads"];
        path.extend(branch.split('/'));
        self.patch(&path, &json!({"sha":commit,"force":force}))?;
        Ok(())
    }
}

pub fn submit(
    api: &impl GitWriter,
    branch: &str,
    expected: &str,
    base_tree: &str,
    message: &str,
    writes: &BTreeMap<String, (String, Vec<u8>)>,
    deletes: &[(String, String)],
) -> Result<String, String> {
    if api.head(branch)? != expected {
        return Err("The remote branch advanced after preview; refresh and preview again".into());
    }
    let mut entries = Vec::new();
    for (path, (mode, bytes)) in writes {
        github::validate_relative(path)?;
        let blob = api.create(
            &["git", "blobs"],
            &json!({"content":STANDARD.encode(bytes),"encoding":"base64"}),
        )?;
        entries
            .push(json!({"path":path,"mode":mode,"type":"blob","sha":github::sha(&blob["sha"])?}));
    }
    for (path, mode) in deletes {
        github::validate_relative(path)?;
        entries.push(json!({"path":path,"mode":mode,"type":"blob","sha":null}));
    }
    if entries.is_empty() {
        return Err("No selected differences remain".into());
    }
    let tree = api.create(
        &["git", "trees"],
        &json!({"base_tree":base_tree,"tree":entries}),
    )?;
    let commit = api.create(
        &["git", "commits"],
        &json!({"message":message,"tree":github::sha(&tree["sha"])?,"parents":[expected]}),
    )?;
    let id = github::sha(&commit["sha"])?;
    if api.head(branch)? != expected {
        return Err("The remote branch advanced during upload. Its reference was not changed; refresh and preview again.".into());
    }
    // The commit's only parent is the preview head. A concurrent divergent advance cannot
    // fast-forward to this commit, so GitHub also rejects races after the second check.
    if api.update(branch, &id, false).is_err() {
        // A lost response does not prove that the server failed. Verify rather than retrying a write.
        if api.head(branch).ok().as_deref() != Some(id.as_str()) {
            return Err("GitHub did not confirm the branch update. Refresh and inspect the branch before submitting again.".into());
        }
    }
    Ok(id)
}

/// Identical selected files still permit a verified association/optional trim,
/// without manufacturing an empty commit. The branch must remain the preview head.
pub fn submit_or_verify(
    api: &impl GitWriter,
    branch: &str,
    expected: &str,
    base_tree: &str,
    message: &str,
    writes: &BTreeMap<String, (String, Vec<u8>)>,
    deletes: &[(String, String)],
) -> Result<String, String> {
    if writes.is_empty() && deletes.is_empty() {
        if api.head(branch)? != expected {
            return Err("远端在预览后发生变化，请重新检查后再精简本地附件".into());
        }
        return Ok(expected.to_owned());
    }
    submit(api, branch, expected, base_tree, message, writes, deletes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    struct Fake {
        heads: Cell<usize>,
        advance: usize,
        calls: RefCell<Vec<(String, Value)>>,
    }
    impl Fake {
        fn new(advance: usize) -> Self {
            Self {
                heads: Cell::new(0),
                advance,
                calls: RefCell::new(vec![]),
            }
        }
    }
    impl GitWriter for Fake {
        fn head(&self, _: &str) -> Result<String, String> {
            let n = self.heads.get() + 1;
            self.heads.set(n);
            Ok(if n >= self.advance {
                "f".repeat(40)
            } else {
                "a".repeat(40)
            })
        }
        fn create(&self, path: &[&str], body: &Value) -> Result<Value, String> {
            self.calls.borrow_mut().push((path.join("/"), body.clone()));
            Ok(json!({"sha":"b".repeat(40)}))
        }
        fn update(&self, branch: &str, commit: &str, force: bool) -> Result<(), String> {
            self.calls.borrow_mut().push((
                "update".into(),
                json!({"branch":branch,"sha":commit,"force":force}),
            ));
            Ok(())
        }
    }
    fn write() -> BTreeMap<String, (String, Vec<u8>)> {
        BTreeMap::from([(
            "skill/SKILL.md".into(),
            ("100644".into(), b"selected content".to_vec()),
        )])
    }
    #[test]
    fn initial_advance_prevents_every_write() {
        let api = Fake::new(1);
        assert!(submit(
            &api,
            "main",
            &"a".repeat(40),
            &"c".repeat(40),
            "change",
            &write(),
            &[]
        )
        .is_err());
        assert!(api.calls.borrow().is_empty());
    }
    #[test]
    fn advance_during_upload_never_changes_ref() {
        let api = Fake::new(2);
        assert!(submit(
            &api,
            "main",
            &"a".repeat(40),
            &"c".repeat(40),
            "change",
            &write(),
            &[]
        )
        .is_err());
        assert!(!api.calls.borrow().iter().any(|(path, _)| path == "update"));
    }
    #[test]
    fn one_commit_preserves_omitted_docs_and_never_force_pushes() {
        let api = Fake::new(usize::MAX);
        submit(
            &api,
            "feature/one",
            &"a".repeat(40),
            &"c".repeat(40),
            "change",
            &write(),
            &[],
        )
        .unwrap();
        let calls = api.calls.borrow();
        let tree = &calls.iter().find(|(p, _)| p == "git/trees").unwrap().1;
        assert_eq!(tree["base_tree"], "c".repeat(40));
        assert_eq!(tree["tree"].as_array().unwrap().len(), 1);
        assert_eq!(tree["tree"][0]["path"], "skill/SKILL.md");
        assert_eq!(calls.iter().filter(|(p, _)| p == "git/commits").count(), 1);
        assert_eq!(calls.last().unwrap().1["force"], false);
    }
    #[test]
    fn unchanged_selected_optional_files_verify_head_without_creating_any_git_objects() {
        let api = Fake::new(usize::MAX);
        let expected = "a".repeat(40);
        assert_eq!(
            submit_or_verify(
                &api,
                "main",
                &expected,
                &"c".repeat(40),
                "verify already uploaded",
                &BTreeMap::new(),
                &[]
            )
            .unwrap(),
            expected
        );
        assert!(api.calls.borrow().is_empty());
        assert_eq!(api.heads.get(), 1);
        let moved = Fake::new(1);
        assert!(submit_or_verify(
            &moved,
            "main",
            &expected,
            &"c".repeat(40),
            "verify already uploaded",
            &BTreeMap::new(),
            &[]
        )
        .is_err());
        assert!(moved.calls.borrow().is_empty());
    }
}
