//! Real blocking HTTP requests against a loopback Git object store. No service
//! credentials, real repository writes, or real Skill folders participate.
use super::*;
use std::{
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone)]
struct Commit {
    tree: String,
    parent: Option<String>,
}
#[derive(Clone)]
struct Request {
    method: String,
    route: Vec<String>,
    body: Value,
    fixture_auth: bool,
}
struct Repository {
    repo: String,
    next: u64,
    head: String,
    commits: BTreeMap<String, Commit>,
    trees: BTreeMap<String, BTreeMap<String, (String, String)>>,
    blobs: BTreeMap<String, Vec<u8>>,
    requests: Vec<Request>,
    patches: usize,
    fail_after_patch: bool,
    corrupt_path_after_patch: Option<String>,
    advance_after_patch: bool,
    unexpected: Vec<String>,
}

impl Repository {
    fn id(&mut self) -> String {
        self.next += 1;
        format!("{:040x}", self.next)
    }
    fn new(repo: String, files: &[(&str, &[u8])]) -> Self {
        let mut store = Self {
            repo,
            next: 0,
            head: String::new(),
            commits: BTreeMap::new(),
            trees: BTreeMap::new(),
            blobs: BTreeMap::new(),
            requests: vec![],
            patches: 0,
            fail_after_patch: false,
            corrupt_path_after_patch: None,
            advance_after_patch: false,
            unexpected: vec![],
        };
        let mut tree = BTreeMap::new();
        for (path, bytes) in files {
            let id = github::blob_hash(bytes);
            store.blobs.insert(id.clone(), bytes.to_vec());
            tree.insert((*path).to_owned(), ("100644".into(), id));
        }
        let tree_id = store.id();
        store.trees.insert(tree_id.clone(), tree);
        let commit = store.id();
        store.commits.insert(
            commit.clone(),
            Commit {
                tree: tree_id,
                parent: None,
            },
        );
        store.head = commit;
        store
    }
    fn file(&self, commit: &str, path: &str) -> Vec<u8> {
        let tree = &self.commits[commit].tree;
        self.blobs[&self.trees[tree][path].1].clone()
    }
    fn advance(&mut self, change: Option<(&str, &[u8])>) {
        let mut tree = self.trees[&self.commits[&self.head].tree].clone();
        if let Some((path, bytes)) = change {
            let blob = github::blob_hash(bytes);
            self.blobs.insert(blob.clone(), bytes.to_vec());
            tree.insert(path.into(), ("100644".into(), blob));
        }
        let tree_id = self.id();
        self.trees.insert(tree_id.clone(), tree);
        let commit = self.id();
        self.commits.insert(
            commit.clone(),
            Commit {
                tree: tree_id,
                parent: Some(self.head.clone()),
            },
        );
        self.head = commit;
    }
    fn respond(&mut self, request: Request) -> (u16, Value) {
        self.requests.push(request.clone());
        if self.fail_after_patch && self.patches > 0 && request.method == "GET" {
            return (503, json!({"message":"synthetic read failure"}));
        }
        match self.route(&request) {
            Ok(value) => (200, value),
            Err(error) => {
                self.unexpected.push(error.clone());
                (422, json!({"message":error}))
            }
        }
    }
    fn route(&mut self, request: &Request) -> Result<Value, String> {
        let route = request.route.iter().map(String::as_str).collect::<Vec<_>>();
        let body = &request.body;
        match (request.method.as_str(), route.as_slice()) {
            ("GET", []) => {
                Ok(json!({"private":true,"default_branch":"main","full_name":self.repo}))
            }
            ("GET", ["commits", reference]) => {
                let commit = if *reference == "main" {
                    self.head.as_str()
                } else {
                    *reference
                };
                let object = self.commits.get(commit).ok_or("Unknown commit")?;
                Ok(json!({"sha":commit,"commit":{"tree":{"sha":object.tree}}}))
            }
            ("GET", ["git", "ref", "heads", "main"]) => {
                Ok(json!({"object":{"type":"commit","sha":self.head}}))
            }
            ("GET", ["git", "commits", id]) => {
                let object = self.commits.get(*id).ok_or("Unknown immutable commit")?;
                Ok(
                    json!({"sha":id,"tree":{"sha":object.tree},"parents":object.parent.iter().map(|parent|json!({"sha":parent})).collect::<Vec<_>>()}),
                )
            }
            ("GET", ["git", "trees", id]) => {
                let tree = self.trees.get(*id).ok_or("Unknown tree")?;
                let rows = tree.iter().map(|(path,(mode,sha))|json!({"path":path,"mode":mode,"type":"blob","sha":sha,"size":self.blobs[sha].len()})).collect::<Vec<_>>();
                Ok(json!({"sha":id,"truncated":false,"tree":rows}))
            }
            ("GET", ["git", "blobs", id]) => {
                let original = self.blobs.get(*id).ok_or("Unknown blob")?;
                let corrupt = self.patches > 0
                    && self.corrupt_path_after_patch.as_ref().is_some_and(|path| {
                        self.trees[&self.commits[&self.head].tree]
                            .get(path)
                            .is_some_and(|(_, blob)| blob.as_str() == *id)
                    });
                let bytes = if corrupt {
                    b"synthetic corrupt readback".as_slice()
                } else {
                    original.as_slice()
                };
                Ok(
                    json!({"sha":id,"encoding":"base64","size":bytes.len(),"content":STANDARD.encode(bytes)}),
                )
            }
            ("POST", ["git", "blobs"]) => {
                if body["encoding"] != "base64" {
                    return Err("Expected base64 upload".into());
                }
                let bytes = STANDARD
                    .decode(text(body, "content")?)
                    .map_err(|_| "Invalid blob body")?;
                let id = github::blob_hash(&bytes);
                self.blobs.insert(id.clone(), bytes);
                Ok(json!({"sha":id}))
            }
            ("POST", ["git", "trees"]) => {
                let mut tree = self
                    .trees
                    .get(text(body, "base_tree")?)
                    .cloned()
                    .ok_or("Unknown base tree")?;
                for row in body["tree"].as_array().ok_or("Missing tree changes")? {
                    let path = text(row, "path")?;
                    if row["sha"].is_null() {
                        tree.remove(path);
                    } else {
                        let id = text(row, "sha")?;
                        if !self.blobs.contains_key(id) {
                            return Err("Tree references a missing blob".into());
                        }
                        tree.insert(path.into(), (text(row, "mode")?.into(), id.into()));
                    }
                }
                let id = self.id();
                self.trees.insert(id.clone(), tree);
                Ok(json!({"sha":id}))
            }
            ("POST", ["git", "commits"]) => {
                let tree = text(body, "tree")?;
                if !self.trees.contains_key(tree) {
                    return Err("Commit references missing tree".into());
                }
                let parents = body["parents"].as_array().ok_or("Missing commit parent")?;
                if parents.len() != 1 {
                    return Err("Expected one commit parent".into());
                }
                let parent = parents[0].as_str().ok_or("Invalid parent")?;
                if !self.commits.contains_key(parent) {
                    return Err("Unknown parent".into());
                }
                let object = Commit {
                    tree: tree.into(),
                    parent: Some(parent.into()),
                };
                let id = self.id();
                self.commits.insert(id.clone(), object);
                Ok(json!({"sha":id}))
            }
            ("PATCH", ["git", "refs", "heads", "main"]) => {
                if body["force"] != false {
                    return Err("Force push is prohibited".into());
                }
                let id = text(body, "sha")?;
                if self
                    .commits
                    .get(id)
                    .and_then(|commit| commit.parent.as_deref())
                    != Some(self.head.as_str())
                {
                    return Err("Non-fast-forward update".into());
                }
                self.head = id.into();
                self.patches += 1;
                let result = json!({"object":{"type":"commit","sha":id}});
                if self.advance_after_patch {
                    self.advance(Some((
                        "skills/protocol-skill/SKILL.md",
                        b"Concurrent remote changes after this upload.",
                    )));
                }
                Ok(result)
            }
            _ => Err(format!(
                "Unsupported synthetic protocol route: {} {}",
                request.method,
                route.join("/")
            )),
        }
    }
}

struct Server {
    address: String,
    state: Arc<Mutex<Repository>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Server {
    fn new(files: &[(&str, &[u8])]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        listener.set_nonblocking(true).unwrap();
        let repo = format!("synthetic-protocol/{}", uuid::Uuid::new_v4().simple());
        let state = Arc::new(Mutex::new(Repository::new(repo, files)));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = state.clone();
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Windows accepts inherit the nonblocking listener's
                        // mode. Reading before bytes arrive must wait, rather
                        // than treating WSAEWOULDBLOCK as an HTTP failure.
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let repo = shared.lock().unwrap().repo.clone();
                        let response = read_request(&mut stream, &repo)
                            .map(|request| shared.lock().unwrap().respond(request));
                        let (status, value) =
                            response.unwrap_or_else(|error| (400, json!({"message":error})));
                        let bytes = serde_json::to_vec(&value).unwrap();
                        let header = format!("HTTP/1.1 {status} Synthetic\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len());
                        let _ = stream
                            .write_all(header.as_bytes())
                            .and_then(|_| stream.write_all(&bytes));
                        // Complete the HTTP close handshake before dropping a
                        // Windows socket; an unread close can otherwise reset
                        // a response while parallel clients are consuming it.
                        let _ = stream.shutdown(Shutdown::Write);
                        let mut drain = [0; 256];
                        while stream.read(&mut drain).is_ok_and(|size| size > 0) {}
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            state,
            stop,
            worker: Some(worker),
        }
    }
    fn transport(&self) -> github::TestTransportGuard {
        github::test_transport(
            &self.state.lock().unwrap().repo,
            &format!("http://{}/", self.address),
        )
    }
    fn repo(&self) -> String {
        self.state.lock().unwrap().repo.clone()
    }
    fn head(&self) -> String {
        self.state.lock().unwrap().head.clone()
    }
    fn file(&self, commit: &str, path: &str) -> Vec<u8> {
        self.state.lock().unwrap().file(commit, path)
    }
    fn advance(&self, change: Option<(&str, &[u8])>) {
        self.state.lock().unwrap().advance(change);
    }
    fn assert_clean(&self) {
        let state = self.state.lock().unwrap();
        assert!(state.unexpected.is_empty(), "{:?}", state.unexpected);
        assert!(state.requests.iter().all(|request| request.fixture_auth));
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn read_request(stream: &mut TcpStream, repo: &str) -> Result<Request, String> {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut buffer = [0; 4096];
        let size = stream.read(&mut buffer).map_err(|error| {
            format!(
                "Cannot read synthetic request: {:?} {:?}",
                error.kind(),
                error.raw_os_error()
            )
        })?;
        if size == 0 {
            return Err("Incomplete synthetic request".into());
        }
        bytes.extend_from_slice(&buffer[..size]);
        if let Some(i) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break i + 4;
        }
        if bytes.len() > 64 * 1024 {
            return Err("Header too large".into());
        }
    };
    let header = std::str::from_utf8(&bytes[..header_end]).map_err(|_| "Invalid request header")?;
    let mut first = header
        .lines()
        .next()
        .ok_or("Missing request")?
        .split_whitespace();
    let method = first.next().ok_or("Missing method")?.to_owned();
    let target = first.next().ok_or("Missing route")?;
    let length = header
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>())
        .transpose()
        .map_err(|_| "Invalid content length")?
        .unwrap_or(0);
    let fixture_auth =
        header
            .lines()
            .filter_map(|line| line.split_once(':'))
            .any(|(key, value)| {
                key.eq_ignore_ascii_case("authorization")
                    && value.trim() == "Bearer synthetic-protocol-fixture"
            });
    let prefix = format!("/repos/{repo}");
    let route = target
        .split('?')
        .next()
        .unwrap()
        .strip_prefix(&prefix)
        .ok_or("Wrong synthetic repository")?
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .map(github::decode_component)
        .collect::<Result<Vec<_>, _>>()?;
    while bytes.len() < header_end + length {
        let mut buffer = [0; 4096];
        let size = stream.read(&mut buffer).map_err(|error| {
            format!(
                "Cannot read request body: {:?} {:?}",
                error.kind(),
                error.raw_os_error()
            )
        })?;
        if size == 0 {
            return Err("Incomplete body".into());
        }
        bytes.extend_from_slice(&buffer[..size]);
    }
    let body = if length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[header_end..header_end + length])
            .map_err(|_| "Invalid request JSON")?
    };
    Ok(Request {
        method,
        route,
        body,
        fixture_auth,
    })
}

struct Fixture {
    _temp: tempfile::TempDir,
    manager: Manager,
    root: PathBuf,
    original_runtime: Vec<u8>,
    optional: Vec<u8>,
}
fn fixture(server: &Server) -> Fixture {
    let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("work/protocol-tests");
    fs::create_dir_all(&parent).unwrap();
    let temp = tempfile::tempdir_in(parent).unwrap();
    let root = temp.path().join("skill");
    fs::create_dir(&root).unwrap();
    let original_runtime =
        b"---\nname: protocol-skill\ndescription: synthetic\n---\nUse the selected runtime.\n"
            .to_vec();
    let optional = b"Optional documentation selected for this upload.\n".to_vec();
    fs::write(root.join("SKILL.md"), &original_runtime).unwrap();
    fs::write(root.join("README.md"), &optional).unwrap();
    let mut manager = Manager::new(temp.path().join("state")).unwrap();
    let mut skill = local::draft(&root).unwrap();
    skill["id"] = json!("local-id");
    skill["categoryId"] = json!("category");
    skill["favorite"] = json!(true);
    manager.state.skills.push(skill);
    manager.state.sources.push(
        json!({"id":"source","repo":server.repo(),"branch":"main","roots":[],"ignoredPaths":[]}),
    );
    manager.save().unwrap();
    Fixture {
        _temp: temp,
        manager,
        root,
        original_runtime,
        optional,
    }
}

#[test]
fn v029_selected_check_discovers_external_upload_and_preserves_other_records() {
    let body = b"---\nname: protocol-skill\ndescription: synthetic\n---\nUse the selected runtime.\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body),
        ("skills/unrelated/SKILL.md", b"---\nname: unrelated\n---\nOther skill.\n"),
    ]);
    let _transport = server.transport();
    let mut fixture = fixture(&server);
    fixture.manager.state.sources[0]["roots"] = json!(["skills"]);
    let untouched = json!({"id":"other-local","name":"other","status":"local_only","categoryId":"other-category","favorite":true});
    fixture.manager.state.skills.push(untouched.clone());
    fixture.manager.state.connections.push(json!({"id":"connection","skillId":"local-id","target":"synthetic-client-entry"}));
    let before_files = local::files(&fixture.root).unwrap();
    let result = check(&mut fixture.manager, &json!({"skillIds":["local-id"]})).unwrap();
    let local = fixture.manager.skill("local-id").unwrap();
    assert_eq!(local["sourceId"], "source");
    assert_eq!(local["remotePath"], "skills/protocol-skill");
    assert_eq!(local["categoryId"], "category");
    assert_eq!(local["favorite"], true);
    assert_eq!(local["associationPending"], false);
    assert!(local["installedCommit"].is_null());
    assert_eq!(result["checkSummary"]["associated"], 1);
    assert_eq!(fixture.manager.state.skills.len(), 2);
    assert_eq!(fixture.manager.skill("other-local").unwrap(), untouched);
    assert_eq!(fixture.manager.state.connections[0]["skillId"], "local-id");
    assert_eq!(local::files(&fixture.root).unwrap(), before_files);
    let next = check(&mut fixture.manager, &json!({"automatic":true,"remoteOnly":true})).unwrap();
    assert_eq!(next["checkSummary"]["newRemote"], 1, "A selected check must not consume unrelated discovery for the next full tick");
    assert_eq!(fixture.manager.skill("other-local").unwrap(), untouched);
    assert!(fixture.manager.state.skills.iter().any(|skill| skill["remotePath"] == "skills/unrelated"));
    assert!(server.state.lock().unwrap().requests.iter().all(|request| request.method == "GET"));
    server.assert_clean();
}

#[test]
fn v029_selected_discovery_keeps_ambiguous_remote_candidates_unassociated() {
    let body = b"---\nname: protocol-skill\ndescription: synthetic\n---\nUse the selected runtime.\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body),
        ("skills/copy/SKILL.md", body),
    ]);
    let _transport = server.transport();
    let mut fixture = fixture(&server);
    fixture.manager.state.sources[0]["roots"] = json!(["skills"]);
    let result = check(&mut fixture.manager, &json!({"skillIds":["local-id"]})).unwrap();
    assert!(fixture.manager.skill("local-id").unwrap()["sourceId"].is_null());
    assert_eq!(fixture.manager.state.skills.len(), 1);
    assert_eq!(result["checkSummary"]["associated"], 0);
    assert_eq!(result["checkSummary"]["associationPending"], 1);
    assert_eq!(fixture.manager.skill("local-id").unwrap()["associationPending"], true);
    assert!(fixture.manager.skill("local-id").unwrap()["associationNote"].as_str().unwrap().contains("不唯一"));
    server.assert_clean();
}

#[test]
fn v029_unchanged_automatic_tick_only_probes_commit_without_state_or_file_reads() {
    let body = b"---\nname: protocol-skill\ndescription: synthetic\n---\nUse the selected runtime.\n";
    let server = Server::new(&[("skills/protocol-skill/SKILL.md", body)]);
    let _transport = server.transport();
    let mut fixture = fixture(&server);
    fixture.manager.state.sources[0]["roots"] = json!(["skills"]);
    check(&mut fixture.manager, &json!({})).unwrap();
    let before = serde_json::to_value(&fixture.manager.state).unwrap();
    let before_revision = fixture.manager.snapshot()["stateRevision"].clone();
    let request_count = server.state.lock().unwrap().requests.len();
    // Local changes belong to the native watcher/dirty-Skill task, rather than
    // forcing another directory inventory during every remote commit probe.
    fs::remove_file(fixture.root.join("SKILL.md")).unwrap();
    check(&mut fixture.manager, &json!({"automatic":true,"remoteOnly":true})).unwrap();
    assert_eq!(serde_json::to_value(&fixture.manager.state).unwrap(), before);
    assert_eq!(fixture.manager.snapshot()["stateRevision"], before_revision);
    fs::write(fixture.root.join("SKILL.md"), &fixture.original_runtime).unwrap();
    {
        let state = server.state.lock().unwrap();
        let requests = &state.requests[request_count..];
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].route, vec!["commits", "main"]);
    }
    server.advance(Some(("skills/new-upload/SKILL.md", b"---\nname: new-upload\n---\nA new remote skill.\n")));
    let result = check(&mut fixture.manager, &json!({"automatic":true,"remoteOnly":true})).unwrap();
    assert_eq!(result["checkSummary"]["newRemote"], 1);
    assert!(fixture.manager.state.skills.iter().any(|skill| skill["remotePath"] == "skills/new-upload"));
    assert!(fixture.manager.skill("local-id").unwrap()["installedCommit"].is_null());
    assert!(server.state.lock().unwrap().requests.iter().all(|request| request.method == "GET"));
    server.assert_clean();
}

#[test]
fn v029_periodic_discovery_obeys_source_scope_and_ignored_paths() {
    let server = Server::new(&[
        ("skills/hidden/SKILL.md", b"---\nname: hidden\n---\nHidden.\n"),
        ("outside/elsewhere/SKILL.md", b"---\nname: elsewhere\n---\nOutside.\n"),
        ("skills/visible/SKILL.md", b"---\nname: visible\n---\nVisible.\n"),
    ]);
    let _transport = server.transport();
    let mut fixture = fixture(&server);
    fixture.manager.state.sources[0]["roots"] = json!(["skills"]);
    fixture.manager.state.sources[0]["ignoredPaths"] = json!(["skills/hidden"]);
    check(&mut fixture.manager, &json!({"automatic":true,"remoteOnly":true})).unwrap();
    assert!(fixture.manager.state.skills.iter().any(|skill| skill["remotePath"] == "skills/visible"));
    assert!(!fixture.manager.state.skills.iter().any(|skill| skill["remotePath"] == "skills/hidden" || skill["remotePath"] == "outside/elsewhere"));
    server.assert_clean();
}
fn action(manager: &mut Manager, request: Value) -> Result<Value, String> {
    if let Some(value) = file_request(manager, &request)? {
        return Ok(value);
    }
    handle(manager, &request)?.ok_or("Unsupported test action".into())
}
fn upload_preview(fixture: &mut Fixture) -> Value {
    action(&mut fixture.manager,json!({"action":"preview_upload","skillId":"local-id","sourceId":"source","remotePath":"skills/protocol-skill","files":["SKILL.md","README.md"],"trimOptional":true})).unwrap()
}
fn apply_upload(fixture: &mut Fixture, preview: &Value) -> Value {
    action(&mut fixture.manager,json!({"action":"apply_upload","previewId":preview["id"],"message":"Synthetic first upload"})).unwrap()
}
fn restore(fixture: &mut Fixture, receipt: &Value) {
    let preview = action(
        &mut fixture.manager,
        json!({"action":"preview_restore","skillId":"local-id","receiptId":receipt}),
    )
    .unwrap();
    assert_eq!(preview["kind"], "restore");
    action(
        &mut fixture.manager,
        json!({"action":"apply_restore","previewId":preview["id"]}),
    )
    .unwrap();
    assert_eq!(
        fs::read(fixture.root.join("README.md")).unwrap(),
        fixture.optional
    );
}
fn assert_one_write_transaction(server: &Server, selected: &[&str]) {
    let state = server.state.lock().unwrap();
    let transactions = state
        .requests
        .iter()
        .filter(|request| request.method == "POST" && request.route == ["git", "commits"])
        .collect::<Vec<_>>();
    assert_eq!(transactions.len(), 1);
    let tree = state
        .requests
        .iter()
        .find(|request| request.method == "POST" && request.route == ["git", "trees"])
        .unwrap();
    let actual = tree.body["tree"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["path"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, selected.iter().copied().collect());
    let patches = state
        .requests
        .iter()
        .filter(|request| request.method == "PATCH")
        .collect::<Vec<_>>();
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].body["force"], false);
    let patch_position = state
        .requests
        .iter()
        .position(|request| request.method == "PATCH")
        .unwrap();
    let read_back = state
        .requests
        .iter()
        .skip(patch_position + 1)
        .any(|request| {
            request.method == "GET"
                && request.route.first().is_some_and(|part| part == "git")
                && request.route.get(1).is_some_and(|part| part == "blobs")
        });
    assert!(
        read_back,
        "Uploaded blobs must be independently read after the reference update"
    );
}

#[test]
fn accepted_connection_waits_for_a_delayed_request_before_responding() {
    let server = Server::new(&[]);
    let mut stream = TcpStream::connect(&server.address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    // Accept is deliberately ready before the client has transmitted headers.
    thread::sleep(Duration::from_millis(30));
    let request = format!("GET /repos/{} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer synthetic-protocol-fixture\r\nConnection: close\r\n\r\n",server.repo());
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
    server.assert_clean();
}

#[test]
fn first_upload_uses_http_objects_preserves_identity_trims_verified_docs_and_restores_fixed_commit()
{
    let server = Server::new(&[
        ("README.md", b"Repository documentation stays remote."),
        ("other/SKILL.md", b"Unselected other Skill."),
    ]);
    let _transport = server.transport();
    let mut fixture = fixture(&server);
    let preview = upload_preview(&mut fixture);
    assert_eq!(preview["cleanupFiles"][0]["path"], "README.md");
    assert!(preview["cleanupFiles"][0]["absolutePath"]
        .as_str()
        .unwrap()
        .ends_with("README.md"));
    let result = apply_upload(&mut fixture, &preview);
    let commit = text(&result["transferResult"], "commit").unwrap();
    assert_eq!(server.head(), commit);
    assert_eq!(
        server.file(commit, "skills/protocol-skill/SKILL.md"),
        fixture.original_runtime
    );
    assert_eq!(
        server.file(commit, "skills/protocol-skill/README.md"),
        fixture.optional
    );
    assert_eq!(
        server.file(commit, "README.md"),
        b"Repository documentation stays remote."
    );
    assert_eq!(
        result["transferResult"]["cleanup"]["removed"][0]["path"],
        "README.md"
    );
    assert!(!fixture.root.join("README.md").exists());
    assert_eq!(
        fs::read(fixture.root.join("SKILL.md")).unwrap(),
        fixture.original_runtime
    );
    let skill = fixture.manager.skill("local-id").unwrap();
    assert_eq!(skill["categoryId"], "category");
    assert_eq!(skill["favorite"], true);
    assert_eq!(skill["sourceId"], "source");
    assert!(skill["installedCommit"].is_null());
    assert_one_write_transaction(
        &server,
        &[
            "skills/protocol-skill/README.md",
            "skills/protocol-skill/SKILL.md",
        ],
    );
    server.advance(Some((
        "skills/protocol-skill/README.md",
        b"Later remote documentation.",
    )));
    restore(
        &mut fixture,
        &result["transferResult"]["cleanup"]["receiptId"],
    );
    server.assert_clean();
}

#[test]
fn linked_push_uses_selected_changes_and_leaves_omitted_remote_docs_intact() {
    let old = b"---\nname: protocol-skill\ndescription: synthetic\n---\nPrevious runtime.\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", old),
        (
            "skills/protocol-skill/README.md",
            b"Previous optional docs.",
        ),
        (
            "skills/protocol-skill/guide.md",
            b"Not downloaded; must stay on GitHub.",
        ),
    ]);
    let _transport = server.transport();
    let mut fixture = fixture(&server);
    let mut skill = fixture.manager.skill("local-id").unwrap();
    skill["sourceId"] = json!("source");
    skill["repo"] = json!(server.repo());
    skill["branch"] = json!("main");
    skill["remotePath"] = json!("skills/protocol-skill");
    skill["runtimeFiles"] = json!(["SKILL.md"]);
    skill["runtimeConfirmed"] = json!(true);
    skill["baselineCommit"] = json!(server.head());
    skill["baselineHashes"] = json!({"SKILL.md":hash(old)});
    fixture.manager.upsert_skill(skill);
    let preview=action(&mut fixture.manager,json!({"action":"preview_push","skillId":"local-id","files":["SKILL.md","README.md"],"trimOptional":true})).unwrap();
    assert_eq!(preview["canApply"], true);
    let result=action(&mut fixture.manager,json!({"action":"apply_push","previewId":preview["id"],"message":"Synthetic selected upload"})).unwrap();
    let commit = text(&result["transferResult"], "commit").unwrap();
    assert_eq!(
        server.file(commit, "skills/protocol-skill/guide.md"),
        b"Not downloaded; must stay on GitHub."
    );
    assert_eq!(
        server.file(commit, "skills/protocol-skill/SKILL.md"),
        fixture.original_runtime
    );
    assert!(!fixture.root.join("README.md").exists());
    assert_one_write_transaction(
        &server,
        &[
            "skills/protocol-skill/README.md",
            "skills/protocol-skill/SKILL.md",
        ],
    );
    restore(
        &mut fixture,
        &result["transferResult"]["cleanup"]["receiptId"],
    );
    server.assert_clean();
}

#[test]
fn branch_advance_after_push_is_not_cleared_by_successful_fixed_commit_attachment_cleanup() {
    let old = b"---\nname: protocol-skill\ndescription: synthetic\n---\nPrevious runtime.\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", old),
        (
            "skills/protocol-skill/README.md",
            b"Previous optional docs.",
        ),
    ]);
    let _transport = server.transport();
    let mut fixture = fixture(&server);
    let mut skill = fixture.manager.skill("local-id").unwrap();
    skill["sourceId"] = json!("source");
    skill["repo"] = json!(server.repo());
    skill["branch"] = json!("main");
    skill["remotePath"] = json!("skills/protocol-skill");
    skill["runtimeFiles"] = json!(["SKILL.md"]);
    skill["runtimeConfirmed"] = json!(true);
    let baseline = server.head();
    skill["baselineCommit"] = json!(baseline);
    skill["baselineHashes"] = json!({"SKILL.md":hash(old)});
    fixture.manager.upsert_skill(skill);
    let preview=action(&mut fixture.manager,json!({"action":"preview_push","skillId":"local-id","files":["SKILL.md","README.md"],"trimOptional":true})).unwrap();
    server.state.lock().unwrap().advance_after_patch = true;
    let result=action(&mut fixture.manager,json!({"action":"apply_push","previewId":preview["id"],"message":"Upload then concurrent advance"})).unwrap();
    let commit = text(&result["transferResult"], "commit").unwrap();
    assert_ne!(commit, server.head());
    assert_eq!(
        result["transferResult"]["cleanup"]["removed"][0]["path"],
        "README.md"
    );
    assert!(!fixture.root.join("README.md").exists());
    assert_eq!(
        server.file(commit, "skills/protocol-skill/README.md"),
        fixture.optional
    );
    let current = fixture.manager.skill("local-id").unwrap();
    assert_eq!(current["remoteCheckRequired"], true);
    assert_eq!(current["comparisonPending"], true);
    assert!(current["remoteHashes"].is_null());
    assert!(current["error"].as_str().is_some());
    assert!(result["transferResult"]["verificationError"]
        .as_str()
        .is_some());
    assert_ne!(current["status"], "synced");
    assert_eq!(current["baselineCommit"], baseline);
    assert!(current["installedCommit"].is_null());
    assert!(current["lastRemoteCheckedAt"].is_null());
    let reopened = Manager::new(fixture.manager.data_dir.clone()).unwrap();
    assert_eq!(
        reopened.skill("local-id").unwrap()["remoteCheckRequired"],
        true
    );
    restore(
        &mut fixture,
        &result["transferResult"]["cleanup"]["receiptId"],
    );
    server.assert_clean();
}

#[test]
fn first_upload_cleanup_checks_current_branch_before_advertising_a_fixed_commit_as_current() {
    let server = Server::new(&[]);
    let _transport = server.transport();
    let mut fixture = fixture(&server);
    let preview = upload_preview(&mut fixture);
    server.state.lock().unwrap().advance_after_patch = true;
    let result = apply_upload(&mut fixture, &preview);
    assert_eq!(
        result["transferResult"]["cleanup"]["removed"][0]["path"],
        "README.md"
    );
    assert_ne!(
        text(&result["transferResult"], "commit").unwrap(),
        server.head()
    );
    let current = fixture.manager.skill("local-id").unwrap();
    assert_eq!(current["remoteCheckRequired"], true);
    assert_eq!(current["comparisonPending"], true);
    assert!(current["error"].as_str().is_some());
    assert!(current["remoteHashes"].is_null());
    assert!(current["installedCommit"].is_null());
    assert!(result["transferResult"]["verificationError"]
        .as_str()
        .is_some());
    server.assert_clean();
}

#[test]
fn stale_branch_after_preview_rejects_upload_without_any_remote_write_or_local_delete() {
    let server = Server::new(&[]);
    let _transport = server.transport();
    let mut fixture = fixture(&server);
    let preview = upload_preview(&mut fixture);
    server.advance(None);
    let error=action(&mut fixture.manager,json!({"action":"apply_upload","previewId":preview["id"],"message":"Must reject stale preview"})).unwrap_err();
    assert!(error.contains("advanced"));
    assert!(fixture.root.join("README.md").exists());
    assert!(fixture.manager.skill("local-id").unwrap()["sourceId"].is_null());
    assert!(server
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|request| request.method == "GET"));
    server.assert_clean();
}

#[test]
fn independent_blob_readback_mismatch_retains_optional_local_file_after_successful_upload() {
    let server = Server::new(&[]);
    let _transport = server.transport();
    let mut fixture = fixture(&server);
    let preview = upload_preview(&mut fixture);
    server.state.lock().unwrap().corrupt_path_after_patch =
        Some("skills/protocol-skill/README.md".into());
    let result = apply_upload(&mut fixture, &preview);
    assert_eq!(result["transferResult"]["submitted"], true);
    assert!(result["transferResult"]["cleanup"]["removed"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        result["transferResult"]["cleanup"]["retained"][0]["path"],
        "README.md"
    );
    let reason = result["transferResult"]["cleanup"]["retained"][0]["reason"]
        .as_str()
        .unwrap();
    assert!(
        reason.contains("校验"),
        "Expected independent content verification: {reason}"
    );
    assert_eq!(
        fs::read(fixture.root.join("README.md")).unwrap(),
        fixture.optional
    );
    assert_eq!(
        server.file(&server.head(), "skills/protocol-skill/README.md"),
        fixture.optional
    );
    server.assert_clean();
}

#[test]
fn successful_reference_update_followed_by_http_failure_is_not_reported_as_failed_upload() {
    let server = Server::new(&[]);
    let _transport = server.transport();
    let mut fixture = fixture(&server);
    let preview = upload_preview(&mut fixture);
    server.state.lock().unwrap().fail_after_patch = true;
    let result = apply_upload(&mut fixture, &preview);
    assert_eq!(result["transferResult"]["submitted"], true);
    assert!(result["transferResult"]["verificationError"]
        .as_str()
        .is_some());
    assert!(result["transferResult"]["cleanup"]["removed"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        fs::read(fixture.root.join("README.md")).unwrap(),
        fixture.optional
    );
    assert_eq!(
        fixture.manager.skill("local-id").unwrap()["sourceId"],
        "source"
    );
    assert_eq!(server.state.lock().unwrap().patches, 1);
    server.assert_clean();
}

fn external_linked_fixture(server: &Server, body: &str) -> Fixture {
    let mut fixture = fixture(server);
    fs::write(fixture.root.join("SKILL.md"), body).unwrap();
    let mut skill = fixture.manager.skill("local-id").unwrap();
    skill["sourceId"] = json!("source");
    skill["repo"] = json!(server.repo());
    skill["branch"] = json!("main");
    skill["remotePath"] = json!("skills/protocol-skill");
    skill["commit"] = json!(server.head());
    fixture.manager.upsert_skill(skill);
    fixture.manager.save().unwrap();
    fixture
}

#[test]
fn external_v020_remote_required_closure_outside_skill_is_pinned_and_zip_only() {
    use std::io::Read;
    let body="---\nname: protocol-skill\ndescription: synthetic\n---\n必须读取 [运行说明](references/runtime.md)。\n必须读取 [模板](../shared/template.json)。\n按需读取 [附加说明](../shared/guide.md)。\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        (
            "skills/protocol-skill/references/runtime.md",
            b"Runtime reference document\n",
        ),
        ("skills/shared/template.json", b"{\"pinned\":true}"),
        ("skills/shared/guide.md", b"Optional remote guide\n"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let before = local::files(&fixture.root).unwrap();
    let inventory = action(
        &mut fixture.manager,
        json!({"action":"get_files","skillId":"local-id","includeRemote":true}),
    )
    .unwrap();
    assert!(inventory["exportError"].is_null(), "{inventory:#}");
    assert!(paths(&inventory["exportRequiredFiles"]).contains(&"references/runtime.md".to_owned()));
    let dependencies = inventory["externalDependencies"].as_array().unwrap();
    let required = dependencies
        .iter()
        .find(|item| item["target"] == "../shared/template.json")
        .unwrap();
    assert_eq!(required["required"], true);
    assert_eq!(required["local"]["status"], "missing");
    assert_eq!(
        required["remote"]["repositoryPath"],
        "skills/shared/template.json"
    );
    assert_eq!(required["effectiveSource"], "github");
    let optional = dependencies
        .iter()
        .find(|item| item["target"] == "../shared/guide.md")
        .unwrap();
    assert_eq!(optional["role"], "optional");
    assert_eq!(optional["defaultIncluded"], false);
    let pinned = inventory["remoteCommit"].clone();
    server.advance(Some(("skills/shared/template.json", b"{\"pinned\":false}")));
    let zip_path = fixture._temp.path().join("necessary.zip");
    action(&mut fixture.manager,json!({"action":"export","skillId":"local-id","destination":zip_path,"remoteCommit":pinned})).unwrap();
    let mut archive = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
    let names = archive.file_names().map(str::to_owned).collect::<Vec<_>>();
    assert!(names.contains(&"references/runtime.md".to_owned()));
    assert!(!names.iter().any(|name| name.ends_with("guide.md")));
    let name = names
        .iter()
        .find(|name| name.ends_with("template.json"))
        .unwrap();
    let mut bytes = vec![];
    archive
        .by_name(name)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"{\"pinned\":true}");
    let mut text = String::new();
    archive
        .by_name("SKILL.md")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert!(text.contains("](external/"));
    assert_eq!(local::files(&fixture.root).unwrap(), before);
    assert!(!fixture.root.join("external").exists());
    assert!(!fixture.root.join("references").exists());
    let optional_zip = fixture._temp.path().join("with-guide.zip");
    action(&mut fixture.manager,json!({"action":"export","skillId":"local-id","destination":optional_zip,"remoteCommit":pinned,"externalIds":[optional["id"]]})).unwrap();
    let archive = zip::ZipArchive::new(fs::File::open(&optional_zip).unwrap()).unwrap();
    assert!(archive.file_names().any(|name| name.ends_with("guide.md")));
    assert_eq!(local::files(&fixture.root).unwrap(), before);
    assert!(server
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|request| request.method == "GET"));
    server.assert_clean();
}

#[test]
fn external_v020_absolute_same_names_need_explicit_mapping_and_batch_has_guarded_results() {
    let body="---\nname: protocol-skill\ndescription: synthetic\n---\n必须读取 `Z:/synthetic-only/template.json`。\n按需读取 `Z:/synthetic-only/guide.md`。\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("shared/a/template.json", b"first"),
        ("shared/b/template.json", b"second"),
        ("shared/guide.md", b"guide"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let report = crate::external::handle(
        &mut fixture.manager,
        &json!({"action":"check_external_dependencies","skillId":"local-id"}),
    )
    .unwrap()
    .unwrap();
    let required = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["required"] == true)
        .unwrap();
    assert_eq!(required["status"], "needs_selection");
    assert!(required["effectiveSource"].is_null());
    assert_eq!(required["candidates"].as_array().unwrap().len(), 2);
    let optional = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["role"] == "optional")
        .unwrap();
    let saved=crate::external::handle(&mut fixture.manager,&json!({"action":"batch_external_dependencies","operation":"bind","skillId":"local-id","items":[
        {"id":required["id"],"expectedFingerprint":required["referenceFingerprint"],"source":"github","repositoryPath":"shared/b/template.json","remoteCommit":server.head()},
        {"id":optional["id"],"expectedFingerprint":"stale","role":"required"}
    ]})).unwrap().unwrap();
    assert_eq!(saved["results"][0]["status"], "success");
    assert_eq!(saved["results"][1]["status"], "error");
    let current = fixture.manager.skill("local-id").unwrap();
    let bindings = current["externalBindings"].as_array().unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0]["role"], "required");
    let selected = saved["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == required["id"])
        .unwrap();
    assert_eq!(selected["status"], "available");
    assert_eq!(
        selected["remote"]["repositoryPath"],
        "shared/b/template.json"
    );
    assert!(server
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|request| request.method == "GET"));
    server.assert_clean();
}

#[test]
fn external_v020_local_customization_wins_and_remote_error_keeps_local_evidence() {
    let body="---\nname: protocol-skill\ndescription: synthetic\n---\n必须读取 [模板](../shared/template.json)。\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("skills/shared/template.json", b"remote"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    fs::create_dir(fixture._temp.path().join("shared")).unwrap();
    fs::write(
        fixture._temp.path().join("shared/template.json"),
        b"local customized",
    )
    .unwrap();
    let report = crate::external::handle(
        &mut fixture.manager,
        &json!({"action":"check_external_dependencies","skillId":"local-id"}),
    )
    .unwrap()
    .unwrap();
    let item = &report["items"][0];
    assert_eq!(item["local"]["status"], "available");
    assert_eq!(item["remote"]["status"], "available");
    assert_eq!(item["effectiveSource"], "local");
    let root_before = local::files(&fixture.root).unwrap();
    let out = fixture._temp.path().join("local-wins.zip");
    action(&mut fixture.manager,json!({"action":"export","skillId":"local-id","destination":out,"remoteCommit":server.head()})).unwrap();
    let mut archive = zip::ZipArchive::new(fs::File::open(out).unwrap()).unwrap();
    let name = archive
        .file_names()
        .find(|name| name.ends_with("template.json"))
        .unwrap()
        .to_owned();
    let mut bytes = vec![];
    Read::read_to_end(&mut archive.by_name(&name).unwrap(), &mut bytes).unwrap();
    assert_eq!(bytes, b"local customized");
    assert_eq!(local::files(&fixture.root).unwrap(), root_before);
    {
        let mut state = server.state.lock().unwrap();
        state.fail_after_patch = true;
        state.patches = 1;
    }
    let unavailable=crate::external::handle(&mut fixture.manager,&json!({"action":"check_external_dependencies","skillId":"local-id","remoteCommit":"ffffffffffffffffffffffffffffffffffffffff"})).unwrap().unwrap();
    assert_eq!(unavailable["items"][0]["local"]["status"], "available");
    assert_eq!(unavailable["items"][0]["remote"]["status"], "error");
    assert_eq!(unavailable["items"][0]["effectiveSource"], "local");
    assert_eq!(local::files(&fixture.root).unwrap(), root_before);
    server.assert_clean();
}

#[test]
fn external_v021_missing_absolute_package_file_auto_maps_and_is_idempotent_without_source_copies() {
    let body = "---\nname: protocol-skill\ndescription: synthetic\n---\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        (
            "skills/protocol-skill/assets/template.json",
            b"remote necessary",
        ),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let missing = fixture.root.join("assets/template.json");
    fs::write(
        fixture.root.join("SKILL.md"),
        format!(
            "{body}必须读取 [模板]({})。\n",
            missing.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let before = local::files(&fixture.root).unwrap();
    let result=crate::external::handle(&mut fixture.manager,&json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id"})).unwrap().unwrap();
    assert_eq!(result["results"][0]["status"], "matched");
    assert_eq!(result["autoMatchSummary"]["matched"], 1);
    assert_eq!(result["autoMatchSummary"]["recordsUpdated"], 1);
    assert_eq!(result["autoMatchSummary"]["recordsRepaired"], 0);
    assert_eq!(result["autoMatchSummary"]["remoteReady"], 1);
    assert_eq!(result["autoMatchSummary"]["localPreviewRequired"], 1);
    assert_eq!(result["autoMatchSummary"]["contentDiffers"], 0);
    let item = &result["items"][0];
    assert_eq!(
        item["remote"]["repositoryPath"],
        "skills/protocol-skill/assets/template.json"
    );
    assert_eq!(item["local"]["status"], "missing");
    assert_eq!(item["effectiveSource"], "github");
    assert_eq!(item["role"], "required");
    let saved = fixture.manager.skill("local-id").unwrap();
    assert_eq!(saved["externalBindings"][0]["automatic"], true);
    let revision = fixture.manager.snapshot()["stateRevision"].clone();
    let again=crate::external::handle(&mut fixture.manager,&json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id"})).unwrap().unwrap();
    assert_eq!(again["results"][0]["status"], "matched");
    assert_eq!(again["autoMatchSummary"]["recordsUpdated"], 0);
    assert_eq!(again["autoMatchSummary"]["recordsRepaired"], 0);
    assert_eq!(again["results"][0]["localPreviewRequired"], true);
    assert_eq!(fixture.manager.snapshot()["stateRevision"], revision);
    assert_eq!(fixture.manager.skill("local-id").unwrap(), saved);
    let destination = fixture._temp.path().join("necessary.zip");
    action(&mut fixture.manager,json!({"action":"export","skillId":"local-id","destination":destination,"remoteCommit":server.head()})).unwrap();
    let mut zip = zip::ZipArchive::new(fs::File::open(destination).unwrap()).unwrap();
    let mut bytes = vec![];
    zip.by_name("assets/template.json")
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"remote necessary");
    assert_eq!(local::files(&fixture.root).unwrap(), before);
    assert!(!missing.exists());
    server.assert_clean();
}

#[test]
fn external_v021_unique_content_matches_full_tree_but_duplicate_objects_and_same_names_do_not() {
    let body = "---\nname: protocol-skill\ndescription: synthetic\n---\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("shared/renamed.json", b"unique content"),
        ("shared/one.json", b"duplicate"),
        ("shared/two.json", b"duplicate"),
        ("shared/missing.json", b"only name"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let one = fixture._temp.path().join("original.json");
    let two = fixture._temp.path().join("duplicate.json");
    let three = fixture._temp.path().join("missing.json");
    fs::write(&one, b"unique content").unwrap();
    fs::write(&two, b"duplicate").unwrap();
    fs::write(
        fixture.root.join("SKILL.md"),
        format!(
            "{body}按需读取 [{}]({})。\n按需读取 [{}]({})。\n必须读取 [{}]({})。\n",
            "one",
            one.display().to_string().replace('\\', "/"),
            "two",
            two.display().to_string().replace('\\', "/"),
            "three",
            three.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let result=crate::external::handle(&mut fixture.manager,&json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id"})).unwrap().unwrap();
    let rows = result["items"].as_array().unwrap();
    let unique = rows
        .iter()
        .find(|item| item["target"].as_str().unwrap().ends_with("original.json"))
        .unwrap();
    assert_eq!(unique["remote"]["repositoryPath"], "shared/renamed.json");
    assert_eq!(unique["remote"]["matchKind"], "content");
    assert_eq!(unique["effectiveSource"], "local");
    assert_eq!(unique["role"], "optional");
    assert_eq!(unique["defaultIncluded"], false);
    let bindings = fixture.manager.skill("local-id").unwrap()["externalBindings"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(bindings.len(), 1);
    assert_eq!(result["autoMatchSummary"]["matched"], 1);
    assert_eq!(result["autoMatchSummary"]["needsSelection"], 2);
    assert!(!three.exists());
    assert_eq!(fs::read(one).unwrap(), b"unique content");
    let changed=crate::external::handle(&mut fixture.manager,&json!({"action":"bind_external_dependency","skillId":"local-id","dependencyId":unique["id"],"role":"required","expectedFingerprint":unique["referenceFingerprint"]})).unwrap().unwrap();
    let current = changed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == unique["id"])
        .unwrap();
    assert_eq!(current["role"], "required");
    assert_eq!(current["effectiveSource"], "local");
    assert_eq!(
        fixture.manager.skill("local-id").unwrap()["externalBindings"][0]["automatic"],
        true
    );
    assert!(server
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|request| request.method == "GET"));
    server.assert_clean();
}

#[test]
fn external_v021_manual_mapping_and_stale_fingerprint_are_preserved() {
    let body="---\nname: protocol-skill\ndescription: synthetic\n---\n必须读取 `Z:/synthetic/asset.json`。\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("shared/asset.json", b"remote"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let report = crate::external::handle(
        &mut fixture.manager,
        &json!({"action":"check_external_dependencies","skillId":"local-id"}),
    )
    .unwrap()
    .unwrap();
    let item = &report["items"][0];
    crate::external::handle(&mut fixture.manager,&json!({"action":"bind_external_dependency","skillId":"local-id","dependencyId":item["id"],"expectedFingerprint":item["referenceFingerprint"],"source":"github","repositoryPath":"shared/asset.json","remoteCommit":server.head()})).unwrap();
    let before = fixture.manager.skill("local-id").unwrap();
    let revision = fixture.manager.snapshot()["stateRevision"].clone();
    let result=crate::external::handle(&mut fixture.manager,&json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id"})).unwrap().unwrap();
    assert_eq!(result["results"][0]["status"], "matched");
    assert_eq!(fixture.manager.skill("local-id").unwrap(), before);
    assert_eq!(fixture.manager.snapshot()["stateRevision"], revision);
    let result=crate::external::handle(&mut fixture.manager,&json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id","items":[{"id":item["id"],"expectedFingerprint":"stale"}]})).unwrap().unwrap();
    assert_eq!(result["results"][0]["status"], "error");
    assert_eq!(fixture.manager.skill("local-id").unwrap(), before);
    server.assert_clean();
}

#[cfg(windows)]
#[test]
fn external_v021_confirmed_alias_missing_leaf_and_directory_alias_have_distinct_evidence() {
    let body = "---\nname: protocol-skill\ndescription: synthetic\n---\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("skills/protocol-skill/assets/config.json", b"remote"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let alias = fixture._temp.path().join("client-entry");
    junction::create(&fixture.root, &alias).unwrap();
    fixture.manager.state.connections.push(
        json!({"id":"alias","skillId":"local-id","linked":true,"path":alias,"target":fixture.root}),
    );
    fs::write(
        fixture.root.join("SKILL.md"),
        format!(
            "{body}必须读取 [配置]({})。\n客户端入口：`{}`。\n",
            alias
                .join("assets/config.json")
                .display()
                .to_string()
                .replace('\\', "/"),
            alias.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let result=crate::external::handle(&mut fixture.manager,&json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id"})).unwrap().unwrap();
    let items = result["items"].as_array().unwrap();
    let file = items
        .iter()
        .find(|row| row["target"].as_str().unwrap().ends_with("config.json"))
        .unwrap();
    assert_eq!(
        file["remote"]["repositoryPath"],
        "skills/protocol-skill/assets/config.json"
    );
    assert_eq!(file["effectiveSource"], "github");
    let directory = items
        .iter()
        .find(|row| row["referenceKind"] == "skill_alias")
        .unwrap();
    assert_eq!(directory["remote"]["status"], "not_applicable");
    assert!(fixture
        .manager
        .skill("local-id")
        .unwrap()
        .get("_externalAliasRoots")
        .is_none());
    junction::delete(&alias).unwrap();
    fs::remove_dir(&alias).unwrap();
    server.assert_clean();
}

#[test]
fn external_v021_content_uniqueness_uses_full_tree_not_first_forty_suggestions() {
    let body = "---\nname: protocol-skill\ndescription: synthetic\n---\n";
    let mut files = vec![(
        "skills/protocol-skill/SKILL.md".to_owned(),
        body.as_bytes().to_vec(),
    )];
    for index in 0..42 {
        files.push((
            format!("shared/{index:02}/template.json"),
            if matches!(index, 0 | 41) {
                b"matching".to_vec()
            } else {
                b"different".to_vec()
            },
        ));
    }
    let refs = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect::<Vec<_>>();
    let server = Server::new(&refs);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let local = fixture._temp.path().join("template.json");
    fs::write(&local, b"matching").unwrap();
    fs::write(
        fixture.root.join("SKILL.md"),
        format!(
            "{body}必须读取 [模板]({})。\n",
            local.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let report=crate::external::handle(&mut fixture.manager,&json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id"})).unwrap().unwrap();
    let item = &report["items"][0];
    assert_eq!(item["candidates"].as_array().unwrap().len(), 40);
    assert_eq!(item["remote"]["candidateCount"], 42);
    assert_eq!(item["remote"]["candidatesTruncated"], true);
    assert_eq!(report["results"][0]["status"], "needs_selection");
    assert!(fixture.manager.skill("local-id").unwrap()["externalBindings"].is_null());
    server.assert_clean();
}

#[test]
fn external_v021_automatic_matching_does_not_adopt_other_skill_or_private_resources() {
    let body = "---\nname: protocol-skill\ndescription: synthetic\n---\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("other/SKILL.md", b"---\nname: other\n---\n"),
        ("other/template.json", b"matching"),
        ("skills/protocol-skill/auth.json", b"private fixture"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let local = fixture._temp.path().join("template.json");
    fs::write(&local, b"matching").unwrap();
    let private = fixture.root.join("auth.json");
    fs::write(
        fixture.root.join("SKILL.md"),
        format!(
            "{body}必须读取 [模板]({})。\n必须读取 [配置]({})。\n",
            local.display().to_string().replace('\\', "/"),
            private.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let report=crate::external::handle(&mut fixture.manager,&json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id"})).unwrap().unwrap();
    assert_eq!(report["autoMatchSummary"]["matched"], 0);
    assert!(fixture.manager.skill("local-id").unwrap()["externalBindings"].is_null());
    assert!(!private.exists());
    assert!(
        report["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["target"].as_str().unwrap().ends_with("auth.json"))
            .unwrap()["remote"]["status"]
            == "unsupported"
    );
    assert!(server
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|request| !request
            .route
            .iter()
            .any(|part| part == "blobs" || part == "contents")));
    server.assert_clean();
}

#[test]
fn external_v021_missing_manual_remote_path_never_falls_back_to_unique_same_content() {
    let body = "---\nname: protocol-skill\ndescription: synthetic\n---\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("shared/chosen.json", b"same bytes"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let resource = fixture._temp.path().join("local.json");
    fs::write(&resource, b"same bytes").unwrap();
    fs::write(
        fixture.root.join("SKILL.md"),
        format!(
            "{body}必须读取 [配置]({})。\n",
            resource.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let report = crate::external::handle(
        &mut fixture.manager,
        &json!({"action":"check_external_dependencies","skillId":"local-id"}),
    )
    .unwrap()
    .unwrap();
    let item = &report["items"][0];
    crate::external::handle(&mut fixture.manager,&json!({"action":"bind_external_dependency","skillId":"local-id","dependencyId":item["id"],"source":"github","repositoryPath":"shared/chosen.json","remoteCommit":server.head(),"expectedFingerprint":item["referenceFingerprint"]})).unwrap();
    server.advance(Some(("shared/renamed.json", b"same bytes")));
    {
        let mut state = server.state.lock().unwrap();
        let tree = state.commits[&state.head].tree.clone();
        state
            .trees
            .get_mut(&tree)
            .unwrap()
            .remove("shared/chosen.json");
    }
    let before = fixture.manager.skill("local-id").unwrap();
    let result=crate::external::handle(&mut fixture.manager,&json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id"})).unwrap().unwrap();
    assert_eq!(result["items"][0]["remote"]["status"], "needs_selection");
    assert_eq!(
        result["items"][0]["remote"]["requiresCommitConfirmation"],
        true
    );
    assert_eq!(result["results"][0]["status"], "needs_selection");
    assert_ne!(
        result["items"][0]["remote"]["repositoryPath"],
        "shared/renamed.json"
    );
    assert_eq!(fixture.manager.skill("local-id").unwrap(), before);
    server.assert_clean();
}

#[test]
fn external_v022_automatic_proof_refresh_is_saved_without_changing_intent_or_source_files() {
    let body="---\nname: protocol-skill\ndescription: synthetic\n---\n必须读取 [模板](../shared/template.json)。\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("skills/shared/template.json", b"same content"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    fs::create_dir(fixture._temp.path().join("shared")).unwrap();
    fs::write(
        fixture._temp.path().join("shared/template.json"),
        b"same content",
    )
    .unwrap();
    let request = json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id"});
    let first = crate::external::handle(&mut fixture.manager, &request)
        .unwrap()
        .unwrap();
    let original = fixture.manager.skill("local-id").unwrap();
    assert_eq!(first["autoMatchSummary"]["recordsUpdated"], 1);
    fs::write(
        fixture.root.join("SKILL.md"),
        format!("{body}\nUnrelated description updated.\n"),
    )
    .unwrap();
    let before = local::files(&fixture.root).unwrap();
    let repaired = crate::external::handle(&mut fixture.manager, &request)
        .unwrap()
        .unwrap();
    let saved = fixture.manager.skill("local-id").unwrap();
    assert_eq!(repaired["autoMatchSummary"]["recordsRepaired"], 1);
    assert_eq!(repaired["autoMatchSummary"]["localPreviewRequired"], 0);
    assert_eq!(
        saved["externalBindings"][0]["referenceFingerprint"],
        original["externalBindings"][0]["referenceFingerprint"]
    );
    assert_eq!(saved["externalBindings"][0]["role"], "required");
    assert_eq!(
        saved["externalBindings"][0]["boundAt"],
        original["externalBindings"][0]["boundAt"]
    );
    assert_ne!(
        saved["externalBindings"][0]["sourceHash"],
        original["externalBindings"][0]["sourceHash"]
    );
    assert_eq!(
        saved["externalBindings"][0]["sourceHash"],
        repaired["items"][0]["sourceHash"]
    );
    server.advance(None);
    let refreshed = crate::external::handle(&mut fixture.manager, &request)
        .unwrap()
        .unwrap();
    assert_eq!(refreshed["autoMatchSummary"]["recordsRepaired"], 1);
    assert_eq!(
        fixture.manager.skill("local-id").unwrap()["externalBindings"][0]["remoteCommit"],
        server.head()
    );
    let revision = fixture.manager.snapshot()["stateRevision"].clone();
    let unchanged = crate::external::handle(&mut fixture.manager, &request)
        .unwrap()
        .unwrap();
    assert_eq!(unchanged["autoMatchSummary"]["recordsUpdated"], 0);
    assert_eq!(fixture.manager.snapshot()["stateRevision"], revision);
    assert_eq!(local::files(&fixture.root).unwrap(), before);
    assert_eq!(
        fs::read(fixture._temp.path().join("shared/template.json")).unwrap(),
        b"same content"
    );
    server.assert_clean();
}

#[test]
fn external_v022_equivalent_repo_case_repairs_only_automatic_records() {
    let body="---\nname: protocol-skill\ndescription: synthetic\n---\n必须读取 [模板](../shared/template.json)。\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("skills/shared/template.json", b"remote"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let request = json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id"});
    crate::external::handle(&mut fixture.manager, &request).unwrap();
    let mut saved = fixture.manager.skill("local-id").unwrap();
    saved["externalBindings"][0]["repo"] =
        json!(saved["repo"].as_str().unwrap().to_ascii_uppercase());
    fixture.manager.upsert_skill(saved);
    let before = local::files(&fixture.root).unwrap();
    let repaired = crate::external::handle(&mut fixture.manager, &request)
        .unwrap()
        .unwrap();
    assert_eq!(repaired["items"][0]["bindingStale"], false);
    assert_eq!(repaired["autoMatchSummary"]["recordsRepaired"], 1);
    let mut saved = fixture.manager.skill("local-id").unwrap();
    assert_eq!(saved["externalBindings"][0]["repo"], saved["repo"]);
    saved["externalBindings"][0]["automatic"] = json!(false);
    saved["externalBindings"][0]["repo"] =
        json!(saved["repo"].as_str().unwrap().to_ascii_uppercase());
    fixture.manager.upsert_skill(saved.clone());
    let preserved = crate::external::handle(&mut fixture.manager, &request)
        .unwrap()
        .unwrap();
    assert_eq!(preserved["results"][0]["status"], "matched");
    assert_eq!(preserved["autoMatchSummary"]["recordsUpdated"], 0);
    assert_eq!(fixture.manager.skill("local-id").unwrap(), saved);
    assert_eq!(local::files(&fixture.root).unwrap(), before);
    server.assert_clean();
}

#[test]
fn external_v022_local_content_difference_is_not_reported_as_a_file_repair() {
    let body="---\nname: protocol-skill\ndescription: synthetic\n---\n必须读取 [模板](../shared/template.json)。\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("skills/shared/template.json", b"remote source"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    fs::create_dir(fixture._temp.path().join("shared")).unwrap();
    let local = fixture._temp.path().join("shared/template.json");
    fs::write(&local, b"local customization").unwrap();
    let before = local::files(&fixture.root).unwrap();
    let request = json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id"});
    let mapped = crate::external::handle(&mut fixture.manager, &request)
        .unwrap()
        .unwrap();
    assert_eq!(mapped["autoMatchSummary"]["recordsUpdated"], 1);
    assert_eq!(mapped["autoMatchSummary"]["recordsRepaired"], 0);
    assert_eq!(mapped["autoMatchSummary"]["remoteReady"], 1);
    assert_eq!(mapped["autoMatchSummary"]["localPreviewRequired"], 0);
    assert_eq!(mapped["autoMatchSummary"]["contentDiffers"], 1);
    assert_eq!(mapped["items"][0]["effectiveSource"], "local");
    let saved = fixture.manager.skill("local-id").unwrap();
    fs::write(
        fixture.root.join("SKILL.md"),
        body.replace("必须读取", "历史来源："),
    )
    .unwrap();
    let changed = crate::external::handle(&mut fixture.manager, &request)
        .unwrap()
        .unwrap();
    assert_eq!(changed["results"][0]["status"], "needs_selection");
    assert_eq!(changed["autoMatchSummary"]["recordsUpdated"], 0);
    assert_eq!(changed["autoMatchSummary"]["remoteReady"], 0);
    assert_eq!(fixture.manager.skill("local-id").unwrap(), saved);
    assert_eq!(fs::read(&local).unwrap(), b"local customization");
    fs::write(fixture.root.join("SKILL.md"), body).unwrap();
    assert_eq!(local::files(&fixture.root).unwrap(), before);
    server.assert_clean();
}

#[test]
fn external_v022_missing_local_file_cannot_repair_mapping_with_a_cached_content_hash() {
    let body = "---\nname: protocol-skill\ndescription: synthetic\n---\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("shared/first.json", b"same content"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let local = fixture._temp.path().join("unrelated-name.json");
    fs::write(&local, b"same content").unwrap();
    fs::write(
        fixture.root.join("SKILL.md"),
        format!(
            "{body}必须读取 [模板]({})。\n",
            local.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let request = json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id"});
    let initial = crate::external::handle(&mut fixture.manager, &request)
        .unwrap()
        .unwrap();
    assert_eq!(initial["results"][0]["status"], "matched");
    let saved = fixture.manager.skill("local-id").unwrap();
    assert!(saved["externalBindings"][0]["hash"].is_string());
    // This synthetic deletion changes the availability proof. The saved hash
    // must never stand in for reading a file which no longer exists.
    fs::remove_file(&local).unwrap();
    server.advance(Some(("shared/second.json", b"same content")));
    {
        let mut state = server.state.lock().unwrap();
        let tree = state.commits[&state.head].tree.clone();
        state
            .trees
            .get_mut(&tree)
            .unwrap()
            .remove("shared/first.json");
    }
    let before = local::files(&fixture.root).unwrap();
    let rejected = crate::external::handle(&mut fixture.manager, &request)
        .unwrap()
        .unwrap();
    assert_eq!(rejected["results"][0]["status"], "missing");
    assert_eq!(rejected["autoMatchSummary"]["recordsUpdated"], 0);
    assert_eq!(rejected["autoMatchSummary"]["remoteReady"], 0);
    assert_eq!(rejected["autoMatchSummary"]["localPreviewRequired"], 0);
    assert_eq!(fixture.manager.skill("local-id").unwrap(), saved);
    assert_eq!(local::files(&fixture.root).unwrap(), before);
    assert!(!local.exists());
    server.assert_clean();
}

#[test]
fn external_v022_manual_pin_mismatch_requires_confirmation_before_exporting_current_bytes() {
    let body = "---\nname: protocol-skill\ndescription: synthetic\n---\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("shared/chosen.json", b"original remote"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let local = fixture._temp.path().join("original-local.json");
    fs::write(&local, b"local customization").unwrap();
    fs::write(
        fixture.root.join("SKILL.md"),
        format!(
            "{body}必须读取 [配置]({})。\n",
            local.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let inspected = crate::external::handle(
        &mut fixture.manager,
        &json!({"action":"check_external_dependencies","skillId":"local-id"}),
    )
    .unwrap()
    .unwrap();
    let item = &inspected["items"][0];
    let pinned = server.head();
    crate::external::handle(&mut fixture.manager,&json!({"action":"bind_external_dependency","skillId":"local-id","dependencyId":item["id"],"source":"github","repositoryPath":"shared/chosen.json","remoteCommit":pinned,"expectedFingerprint":item["referenceFingerprint"]})).unwrap();
    let saved = fixture.manager.skill("local-id").unwrap();
    let before = local::files(&fixture.root).unwrap();
    server.advance(Some(("shared/chosen.json", b"new remote bytes")));
    let current = server.head();
    let request = json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"local-id","remoteCommit":current});
    let waiting = crate::external::handle(&mut fixture.manager, &request)
        .unwrap()
        .unwrap();
    assert_eq!(waiting["items"][0]["local"]["readable"], true);
    assert_eq!(waiting["items"][0]["status"], "needs_selection");
    assert_eq!(waiting["items"][0]["remote"]["pinnedCommit"], pinned);
    assert_eq!(waiting["items"][0]["remote"]["commit"], current);
    assert_eq!(
        waiting["items"][0]["candidates"][0]["repositoryPath"],
        "shared/chosen.json"
    );
    assert_eq!(
        waiting["items"][0]["candidates"][0]["requiresCommitConfirmation"],
        true
    );
    assert_eq!(waiting["results"][0]["status"], "needs_selection");
    assert_eq!(waiting["autoMatchSummary"]["recordsUpdated"], 0);
    assert_eq!(waiting["autoMatchSummary"]["remoteReady"], 0);
    assert_eq!(waiting["autoMatchSummary"]["localPreviewRequired"], 0);
    assert_eq!(fixture.manager.skill("local-id").unwrap(), saved);
    let refused = fixture._temp.path().join("not-confirmed.zip");
    let error=action(&mut fixture.manager,json!({"action":"export","skillId":"local-id","destination":refused,"remoteCommit":current})).unwrap_err();
    assert!(error.contains("固定提交"), "{error}");
    assert!(!refused.exists());
    let role_only=crate::external::handle(&mut fixture.manager,&json!({"action":"bind_external_dependency","skillId":"local-id","dependencyId":item["id"],"role":"required","expectedFingerprint":item["referenceFingerprint"]})).unwrap().unwrap();
    assert_eq!(
        role_only["items"][0]["remote"]["requiresCommitConfirmation"],
        true
    );
    assert_eq!(
        fixture.manager.skill("local-id").unwrap()["externalBindings"][0]["remoteCommit"],
        pinned
    );
    let confirmed=crate::external::handle(&mut fixture.manager,&json!({"action":"bind_external_dependency","skillId":"local-id","dependencyId":item["id"],"source":"github","repositoryPath":"shared/chosen.json","remoteCommit":current,"expectedFingerprint":item["referenceFingerprint"]})).unwrap().unwrap();
    assert_eq!(confirmed["items"][0]["status"], "available");
    assert_eq!(confirmed["items"][0]["effectiveSource"], "github");
    assert_eq!(
        fixture.manager.skill("local-id").unwrap()["externalBindings"][0]["remoteCommit"],
        current
    );
    let destination = fixture._temp.path().join("confirmed.zip");
    action(&mut fixture.manager,json!({"action":"export","skillId":"local-id","destination":destination,"remoteCommit":current})).unwrap();
    let mut archive = zip::ZipArchive::new(fs::File::open(destination).unwrap()).unwrap();
    let path = archive
        .file_names()
        .find(|name| name.ends_with("chosen.json"))
        .unwrap()
        .to_owned();
    let mut bytes = vec![];
    archive
        .by_name(&path)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"new remote bytes");
    assert_eq!(fs::read(&local).unwrap(), b"local customization");
    assert_eq!(local::files(&fixture.root).unwrap(), before);
    server.assert_clean();
}

#[test]
fn v023_export_plan_remote_only_package_and_environment_files_use_one_pinned_manifest() {
    let body="---\nname: protocol-skill\ndescription: synthetic\n---\n共享环境规则：[共享规则](../shared/AGENTS.md)。\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("skills/protocol-skill/guide.md", b"Optional GitHub note"),
        ("skills/shared/AGENTS.md", b"Shared GitHub rules"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let before = local::files(&fixture.root).unwrap();
    let skill_before = fixture.manager.skill("local-id").unwrap();
    let plan = crate::dispatch(
        &mut fixture.manager,
        &json!({"action":"get_export_plan","skillId":"local-id","selectedIds":[]}),
    )
    .unwrap();
    assert_eq!(plan["canExport"], true, "{plan}");
    let external = plan["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["group"] == "optional_external")
        .unwrap();
    assert_eq!(external["source"], "github");
    assert_eq!(external["included"], false);
    let selected=crate::dispatch(&mut fixture.manager,&json!({"action":"get_export_plan","skillId":"local-id","selectedIds":["file:guide.md",external["id"]],"remoteCommit":plan["remoteCommit"]})).unwrap();
    assert_eq!(selected["canExport"], true, "{selected}");
    assert_eq!(selected["summary"]["remoteCount"], 2);
    assert_eq!(selected["summary"]["optionalExternalSelected"], 1);
    server.advance(Some(("skills/shared/AGENTS.md", b"New branch rules")));
    let destination = fixture._temp.path().join("selected-plan.zip");
    crate::dispatch(&mut fixture.manager,&json!({"action":"export","skillId":"local-id","planId":selected["id"],"destination":destination})).unwrap();
    let mut archive = zip::ZipArchive::new(fs::File::open(&destination).unwrap()).unwrap();
    let actual = archive
        .file_names()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let manifest = selected["manifest"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["path"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, manifest);
    assert_eq!(
        archive.len() as u64,
        selected["summary"]["total"].as_u64().unwrap()
    );
    let mut bytes = vec![];
    archive
        .by_name(external["zipPath"].as_str().unwrap())
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"Shared GitHub rules");
    assert_eq!(local::files(&fixture.root).unwrap(), before);
    let skill_after = fixture.manager.skill("local-id").unwrap();
    let mut expected_skill = skill_before;
    for field in [
        "runtimeAudit",
        "runtimeIssues",
        "runtimeWarnings",
        "runtimeConfirmed",
        "runtimeConfirmationKind",
    ] {
        expected_skill[field] = skill_after[field].clone();
    }
    assert_eq!(
        skill_after, expected_skill,
        "Detected static evidence preserves exact source, selections, bindings and history"
    );
    assert_eq!(skill_after["runtimeConfirmed"], true);
    assert_eq!(skill_after["runtimeConfirmationKind"], "automatic");
    assert_eq!(
        skill_after["runtimeAudit"]["inventoryHashes"],
        json!(before)
    );
    assert!(server
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|request| request.method == "GET"));
    server.assert_clean();
}

#[test]
fn v023_offline_local_export_survives_unknown_unselected_remote_inventory() {
    let body="---\nname: protocol-skill\ndescription: synthetic\n---\n共享环境规则：[规则](../shared/AGENTS.md)。\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        (
            "skills/protocol-skill/guide.md",
            b"Unknown remote optional file",
        ),
        (
            "skills/shared/AGENTS.md",
            b"Unselected remote environment rules",
        ),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let references=crate::external::handle(&mut fixture.manager,&json!({"action":"check_external_dependencies","skillId":"local-id","remoteCommit":server.head()})).unwrap().unwrap();
    let item = &references["items"][0];
    crate::external::handle(&mut fixture.manager,&json!({"action":"bind_external_dependency","skillId":"local-id","dependencyId":item["id"],"role":"environment","source":"github","repositoryPath":"skills/shared/AGENTS.md","remoteCommit":server.head(),"expectedFingerprint":item["referenceFingerprint"]})).unwrap();
    {
        let mut state = server.state.lock().unwrap();
        state.fail_after_patch = true;
        state.patches = 1;
    }
    let plan = crate::dispatch(
        &mut fixture.manager,
        &json!({"action":"get_export_plan","skillId":"local-id","selectedIds":[]}),
    )
    .unwrap();
    assert_eq!(plan["remoteStatus"], "error");
    assert!(plan["remoteError"].is_string());
    assert_eq!(plan["canExport"], true, "{plan}");
    let destination = fixture._temp.path().join("offline.zip");
    crate::dispatch(&mut fixture.manager,&json!({"action":"export","skillId":"local-id","planId":plan["id"],"destination":destination})).unwrap();
    assert!(destination.is_file());
    let selected = crate::dispatch(
        &mut fixture.manager,
        &json!({"action":"get_export_plan","skillId":"local-id","selectedIds":["file:guide.md"]}),
    )
    .unwrap();
    assert_eq!(selected["canExport"], false);
    assert!(selected["resources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == "file:guide.md" && row["included"] == true));
    server.assert_clean();
}

#[test]
fn v023_selected_remote_read_failure_keeps_choice_and_blocks_zip_creation() {
    let body = "---\nname: protocol-skill\ndescription: synthetic\n---\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        ("skills/protocol-skill/guide.md", b"Unread optional file"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let inventory = crate::dispatch(
        &mut fixture.manager,
        &json!({"action":"get_export_plan","skillId":"local-id","selectedIds":[]}),
    )
    .unwrap();
    {
        let mut state = server.state.lock().unwrap();
        state.fail_after_patch = true;
        state.patches = 1;
    }
    let selected=crate::dispatch(&mut fixture.manager,&json!({"action":"get_export_plan","skillId":"local-id","remoteCommit":inventory["remoteCommit"],"selectedIds":["file:guide.md"]})).unwrap();
    assert_eq!(selected["canExport"], false, "{selected}");
    assert!(selected["resources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == "file:guide.md" && row["included"] == true));
    let destination = fixture._temp.path().join("failed-read.zip");
    assert!(crate::dispatch(&mut fixture.manager,&json!({"action":"export","skillId":"local-id","planId":selected["id"],"destination":destination})).is_err());
    assert!(!destination.exists());
    server.assert_clean();
}

#[test]
fn v023_remote_only_reference_rewrite_preserves_immutable_source_receipt() {
    let body = "---\nname: protocol-skill\ndescription: synthetic\n---\n必须读取 [运行说明](references/runtime.md)。\n";
    let reference = "# Runtime reference\n\n共享环境规则：[规则](../../shared/AGENTS.md)。\n";
    let server = Server::new(&[
        ("skills/protocol-skill/SKILL.md", body.as_bytes()),
        (
            "skills/protocol-skill/references/runtime.md",
            reference.as_bytes(),
        ),
        ("skills/shared/AGENTS.md", b"Shared immutable rules"),
    ]);
    let _transport = server.transport();
    let mut fixture = external_linked_fixture(&server, body);
    let before = local::files(&fixture.root).unwrap();
    let base = crate::dispatch(
        &mut fixture.manager,
        &json!({"action":"get_export_plan","skillId":"local-id","selectedIds":[]}),
    )
    .unwrap();
    assert_eq!(base["canExport"], true, "{base}");
    let resource = base["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["group"] == "optional_external")
        .unwrap();
    let selected = crate::dispatch(&mut fixture.manager,&json!({"action":"get_export_plan","skillId":"local-id","selectedIds":[resource["id"]],"remoteCommit":base["remoteCommit"]})).unwrap();
    assert_eq!(selected["canExport"], true, "{selected}");
    let pinned = base["remoteCommit"].clone();
    server.advance(Some((
        "skills/protocol-skill/references/runtime.md",
        b"New current document",
    )));
    let destination = fixture._temp.path().join("remote-rewritten.zip");
    crate::dispatch(&mut fixture.manager,&json!({"action":"export","skillId":"local-id","planId":selected["id"],"destination":destination})).unwrap();
    let mut archive = zip::ZipArchive::new(fs::File::open(&destination).unwrap()).unwrap();
    let mut text = String::new();
    archive
        .by_name("references/runtime.md")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert!(text.contains("../external/"), "{text}");
    assert!(!text.contains("../../shared/AGENTS.md"));
    let mut provenance = String::new();
    archive
        .by_name("skill-manager-provenance.json")
        .unwrap()
        .read_to_string(&mut provenance)
        .unwrap();
    let provenance: Value = serde_json::from_str(&provenance).unwrap();
    let receipt = &provenance["files"]["references/runtime.md"];
    assert_eq!(receipt["source"], "github_zip_copy");
    assert_eq!(receipt["commit"], pinned);
    assert_eq!(
        receipt["repositoryPath"],
        "skills/protocol-skill/references/runtime.md"
    );
    assert_eq!(receipt["originalHash"], hash(reference.as_bytes()));
    assert_eq!(
        receipt["gitBlobSha"],
        github::blob_hash(reference.as_bytes())
    );
    assert_eq!(receipt["hash"], hash(text.as_bytes()));
    assert_eq!(receipt["referenceRewritten"], true);
    assert_eq!(local::files(&fixture.root).unwrap(), before);
    assert!(!fixture.root.join("references").exists());
    assert!(server
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|request| request.method == "GET"));
    server.assert_clean();
}
