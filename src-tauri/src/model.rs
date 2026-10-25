use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub skills: Vec<Value>,
    pub sources: Vec<Value>,
    pub connections: Vec<Value>,
    pub settings: Value,
    #[serde(default, rename = "localRoots")]
    pub local_roots: Vec<Value>,
    #[serde(default, rename = "ignoredLocalPaths")]
    pub ignored_local_paths: Vec<String>,
    #[serde(default = "automatic_checks", rename = "checkMode")]
    pub check_mode: String,
    #[serde(default, rename = "checkGeneration")]
    pub check_generation: u64,
    #[serde(default, rename = "checkSummary")]
    pub check_summary: Value,
}

fn automatic_checks() -> String {
    "auto".into()
}

pub struct Manager {
    pub data_dir: PathBuf,
    pub(crate) storage: Arc<crate::storage::Storage>,
    pub state: State,
    pub previews: HashMap<String, Value>,
    pub management_undo: Vec<Value>,
    db: Option<Connection>,
    revision: AtomicU64,
    runtime: Option<Arc<dyn ManagerRuntime>>,
    state_events: Option<Arc<dyn Fn(u64) + Send + Sync>>,
}

// Detached task workers never own a database. Their saves are acknowledged only
// after the live single writer has safely merged and persisted their changes.
pub(crate) trait ManagerRuntime: Send + Sync {
    fn save(&self, manager: &Manager) -> Result<(), String>;
    fn snapshot(&self) -> Value;
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

impl Manager {
    #[cfg(test)]
    pub fn new(data_dir: PathBuf) -> Result<Self, String> {
        Self::with_storage(crate::storage::Storage::isolated(data_dir))
    }
    pub(crate) fn with_storage(storage: Arc<crate::storage::Storage>) -> Result<Self, String> {
        let data_dir = storage.path("state")?;
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let db = Connection::open(data_dir.join("manager.sqlite3")).map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS app_state (id INTEGER PRIMARY KEY CHECK(id=1), schema_version INTEGER NOT NULL, payload TEXT NOT NULL); CREATE TABLE IF NOT EXISTS workspace_state (id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL);").map_err(|e| e.to_string())?;
        let saved: Result<String, _> =
            db.query_row("SELECT payload FROM app_state WHERE id=1", [], |r| r.get(0));
        let (mut state, revision) = match saved {
            Ok(s) => {
                let value: Value = serde_json::from_str(&s).map_err(|e| {
                    format!("Cannot read saved state; original database preserved: {e}")
                })?;
                let revision = value["stateRevision"].as_u64().unwrap_or(0);
                (
                    serde_json::from_value(value).map_err(|e| e.to_string())?,
                    revision,
                )
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => (
                State {
                    skills: vec![],
                    sources: vec![],
                    connections: vec![],
                    settings: json!({"checkIntervalMinutes":30,"uiTheme":"light","uiZoom":100,"uiFontSize":16,"libraryMode":"list","libraryPageSize":20,"librarySort":"name","categories":[],"sidebarWidth":224,"sidebarCollapsed":false}),
                    local_roots: vec![],
                    ignored_local_paths: vec![],
                    check_mode: automatic_checks(),
                    check_generation: 0,
                    check_summary: Value::Null,
                },
                0,
            ),
            Err(e) => return Err(e.to_string()),
        };
        // The previously selected personal directory is the only approved
        // migration seed. Never infer arbitrary parents from registered Skills.
        crate::local_roots::migrate_selected_root(&mut state);
        Ok(Self {
            data_dir,
            storage,
            state,
            previews: HashMap::new(),
            management_undo: Vec::new(),
            db: Some(db),
            revision: AtomicU64::new(revision),
            runtime: None,
            state_events: None,
        })
    }
    pub fn snapshot(&self) -> Value {
        if let Some(runtime) = &self.runtime {
            return runtime.snapshot();
        }
        let mut v = serde_json::to_value(&self.state).expect("serializable state");
        v["dataDir"] = json!(self.data_dir.to_string_lossy());
        v["storage"] = self.storage.snapshot().unwrap_or(Value::Null);
        v["managementUndo"] = json!({"available":!self.management_undo.is_empty(),"label":self.management_undo.last().and_then(|entry|entry["label"].as_str())});
        v["stateRevision"] = json!(self.revision.load(Ordering::Relaxed));
        v["readGeneration"] = json!(crate::network::read_generation().ok());
        v
    }
    pub fn refresh_selection(&self, request: &Value) -> Result<Option<BTreeSet<String>>, String> {
        if request.get("skillId").is_some() && request.get("skillIds").is_some() {
            return Err("Choose skillId or skillIds, not both".into());
        }
        let values = if let Some(value) = request.get("skillIds") {
            let ids = value
                .as_array()
                .ok_or("skillIds must be a list of Skill IDs")?;
            if ids.is_empty() {
                return Err("Select at least one Skill to check".into());
            }
            ids.clone()
        } else if let Some(value) = request.get("skillId") {
            vec![value.clone()]
        } else {
            return Ok(None);
        };
        let mut ids = BTreeSet::new();
        for value in values {
            let id = value
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or("Invalid Skill ID in selection")?;
            self.skill(id)?;
            ids.insert(id.to_owned());
        }
        Ok(Some(ids))
    }
    pub fn save(&self) -> Result<(), String> {
        if let Some(runtime) = &self.runtime {
            return runtime.save(self);
        }
        let revision = self.revision.load(Ordering::Relaxed) + 1;
        let mut payload = serde_json::to_value(&self.state).map_err(|e| e.to_string())?;
        payload["stateRevision"] = json!(revision);
        self.db.as_ref().ok_or("Task worker cannot write a database")?.execute("INSERT INTO app_state(id,schema_version,payload) VALUES(1,1,?1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", params![payload.to_string()]).map_err(|e| e.to_string())?;
        self.revision.store(revision, Ordering::Relaxed);
        if let Some(events) = &self.state_events {
            events(revision);
        }
        Ok(())
    }
    pub(crate) fn detached(&self, runtime: Arc<dyn ManagerRuntime>) -> Self {
        Self {
            data_dir: self.data_dir.clone(),
            storage: self.storage.clone(),
            state: self.state.clone(),
            previews: self.previews.clone(),
            management_undo: self.management_undo.clone(),
            db: None,
            revision: AtomicU64::new(self.revision.load(Ordering::Relaxed)),
            runtime: Some(runtime),
            state_events: None,
        }
    }
    pub(crate) fn set_state_events(&mut self, events: Arc<dyn Fn(u64) + Send + Sync>) {
        self.state_events = Some(events);
    }
    pub(crate) fn workspace_state(&self) -> Result<Option<Value>, String> {
        let db = self
            .db
            .as_ref()
            .ok_or("Task worker has no workspace database")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS workspace_state (id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL)").map_err(|e|e.to_string())?;
        match db.query_row("SELECT payload FROM workspace_state WHERE id=1", [], |r| {
            r.get::<_, String>(0)
        }) {
            Ok(value) => serde_json::from_str(&value)
                .map(Some)
                .map_err(|e| format!("Cannot read workspace; original data preserved: {e}")),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
    pub(crate) fn copy_database(&self, destination: &std::path::Path) -> Result<(), String> {
        let db = self
            .db
            .as_ref()
            .ok_or("Task worker cannot copy a database")?;
        db.execute("VACUUM INTO ?1", params![destination.to_string_lossy()])
            .map_err(|e| e.to_string())?;
        let copy =
            Connection::open_with_flags(destination, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| e.to_string())?;
        let check: String = copy
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if check != "ok" {
            return Err("复制后的数据库校验失败，原数据库已保留".into());
        }
        drop(copy);
        std::fs::OpenOptions::new()
            .write(true)
            .open(destination)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())
    }
    pub(crate) fn store_workspace_state(&self, value: &Value) -> Result<(), String> {
        self.db.as_ref().ok_or("Task worker has no workspace database")?.execute("INSERT INTO workspace_state(id,payload) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", params![value.to_string()]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub(crate) fn store_workspace_with_history(
        &self,
        value: &Value,
        records: &[Value],
    ) -> Result<(), String> {
        let db = self
            .db
            .as_ref()
            .ok_or("Task worker has no workspace database")?;
        db.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        let result = (|| {
            db.execute("INSERT INTO workspace_state(id,payload) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",params![value.to_string()]).map_err(|e|e.to_string())?;
            db.execute("DELETE FROM task_history", [])
                .map_err(|e| e.to_string())?;
            for record in records {
                db.execute(
                    "INSERT INTO task_history(id,payload) VALUES(?1,?2)",
                    params![
                        record["id"].as_str().ok_or("Invalid task history ID")?,
                        record.to_string()
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            db.execute_batch("COMMIT").map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = db.execute_batch("ROLLBACK");
        }
        result
    }
    pub(crate) fn task_history(&self) -> Result<Vec<Value>, String> {
        let db = self
            .db
            .as_ref()
            .ok_or("Task worker has no history database")?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_history (id TEXT PRIMARY KEY, payload TEXT NOT NULL)",
        )
        .map_err(|e| e.to_string())?;
        let mut statement = db
            .prepare("SELECT payload FROM task_history")
            .map_err(|e| e.to_string())?;
        let records = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .map(|row| {
                row.map_err(|e| e.to_string())
                    .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
            })
            .collect();
        records
    }
    pub(crate) fn store_task_history(&self, records: &[Value]) -> Result<(), String> {
        let db = self
            .db
            .as_ref()
            .ok_or("Task worker has no history database")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS task_history (id TEXT PRIMARY KEY, payload TEXT NOT NULL); BEGIN IMMEDIATE").map_err(|e|e.to_string())?;
        let result = (|| {
            db.execute("DELETE FROM task_history", [])
                .map_err(|e| e.to_string())?;
            for record in records {
                db.execute(
                    "INSERT INTO task_history(id,payload) VALUES(?1,?2)",
                    params![
                        record["id"].as_str().ok_or("Invalid task history ID")?,
                        record.to_string()
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            db.execute_batch("COMMIT").map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = db.execute_batch("ROLLBACK");
        }
        result
    }
    pub fn skill(&self, id: &str) -> Result<Value, String> {
        self.state
            .skills
            .iter()
            .find(|s| s["id"] == id)
            .cloned()
            .ok_or_else(|| "Skill no longer exists".into())
    }
    pub fn upsert_skill(&mut self, skill: Value) {
        if let Some(i) = self
            .state
            .skills
            .iter()
            .position(|s| s["id"] == skill["id"])
        {
            self.state.skills[i] = skill;
        } else {
            self.state.skills.push(skill);
        }
    }
    pub fn id(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }
}
