//! Native windows share one task service and one durable metadata writer.
use crate::tasks::TaskSystem;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};

const MAX_PANES: usize = 80;
const RUNTIME_LIMIT: usize = 8 * 1024 * 1024;
const KINDS: &[&str] = &[
    "home",
    "detail",
    "operation",
    "runtime",
    "export",
    "preview",
    "receipts",
    "audit",
    "history",
    "task",
    "local",
    "github",
    "associate",
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowRecord {
    label: String,
    panes: Vec<Value>,
    active_pane_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bounds: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    split_view: Option<Value>,
}
fn split_view(value: &Value, window: &WindowRecord) -> Result<Option<Value>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let right = identifier(value, "rightPaneId")?;
    if !window
        .panes
        .iter()
        .any(|p| p["id"] == right && p["kind"] != "home")
    {
        return Err("分栏右侧必须是当前窗口中的业务页面".into());
    }
    let ratio = value["ratio"]
        .as_f64()
        .filter(|n| n.is_finite())
        .unwrap_or(0.55)
        .clamp(0.1, 0.9);
    Ok(Some(json!({"rightPaneId":right,"ratio":ratio})))
}
fn normalize_split(window: &mut WindowRecord) {
    let Some(current) = window.split_view.clone() else {
        return;
    };
    let ratio = current["ratio"]
        .as_f64()
        .filter(|n| n.is_finite())
        .unwrap_or(0.55)
        .clamp(0.1, 0.9);
    let right = window
        .panes
        .iter()
        .find(|p| p["kind"] != "home" && p["id"] == current["rightPaneId"])
        .or_else(|| {
            window
                .panes
                .iter()
                .find(|p| p["kind"] != "home" && p["id"] == window.active_pane_id)
        })
        .or_else(|| window.panes.iter().rev().find(|p| p["kind"] != "home"));
    window.split_view = right.map(|pane| json!({"rightPaneId":pane["id"],"ratio":ratio}));
}
fn activate(window: &mut WindowRecord, id: &str) {
    window.active_pane_id = id.to_owned();
    if let Some(split) = &mut window.split_view {
        if id != home_id(&window.label) {
            split["rightPaneId"] = json!(id);
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Layout {
    revision: u64,
    windows: Vec<WindowRecord>,
}
#[derive(Clone)]
struct Transfer {
    id: String,
    pane_id: String,
    source: String,
    target: String,
    created: Instant,
}
struct CloseIntent {
    id: String,
    labels: BTreeSet<String>,
    acknowledgements: BTreeSet<String>,
    error: Option<String>,
    prepared: bool,
    round: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportSelection {
    id: String,
    skill_id: String,
    revision: u64,
    selected_ids: BTreeSet<String>,
    version: u64,
    #[serde(skip)]
    participants: BTreeMap<(String, String), String>,
}
struct Registry {
    layout: Layout,
    live: BTreeSet<String>,
    closing_windows: BTreeSet<String>,
    transfers: BTreeMap<String, Transfer>,
    runtime: BTreeMap<String, Value>,
    export_selections: BTreeMap<String, ExportSelection>,
    export_version: u64,
    close: Option<CloseIntent>,
    last_focus: Option<Instant>,
}

fn home_id(label: &str) -> String {
    if label == "main" {
        "home".into()
    } else {
        format!("home:{label}")
    }
}
fn home(label: &str) -> Value {
    json!({"id":home_id(label),"kind":"home","title":"工作台","state":{"view":"skills"}})
}
fn identifier(value: &Value, key: &str) -> Result<String, String> {
    value[key]
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 160
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_:".contains(c))
        })
        .map(str::to_owned)
        .ok_or_else(|| format!("Invalid {key}"))
}
fn pane(value: &Value, label: &str, restored: bool) -> Result<Value, String> {
    let id = identifier(value, "id")?;
    let kind = value["kind"]
        .as_str()
        .filter(|kind| KINDS.contains(kind))
        .ok_or("Invalid pane kind")?;
    if kind == "home" && id != home_id(label) {
        return Err("Home pane identity belongs to another window".into());
    }
    let mut result = json!({"id":id,"kind":kind,"state":{}});
    for key in ["title", "skillId"] {
        if let Some(text) = value[key].as_str() {
            result[key] = json!(text.chars().take(512).collect::<String>());
        }
    }
    for key in [
        "section",
        "documentPath",
        "docPath",
        "documentVersion",
        "view",
        "clientId",
        "taskId",
        "operation",
    ] {
        if let Some(text) = value["state"][key].as_str() {
            result["state"][key] = json!(text.chars().take(1024).collect::<String>());
        }
    }
    if let Some(n) = value["state"]["scrollTop"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 10_000_000.0)
    {
        result["state"]["scrollTop"] = json!(n);
    }
    for key in ["outlineExpanded", "reviewRequired"] {
        if let Some(b) = value["state"][key].as_bool() {
            result["state"][key] = json!(b);
        }
    }
    if let Some(items) = value["state"]["expandedSections"].as_array() {
        let mut seen = BTreeSet::new();
        result["state"]["expandedSections"] = json!(items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.encode_utf16().count() <= 4096)
            .filter(|s| seen.insert((*s).to_owned()))
            .take(100)
            .collect::<Vec<_>>());
    }
    if kind == "history" {
        result["state"]
            .as_object_mut()
            .unwrap()
            .remove("reviewRequired");
    } else if restored && !matches!(kind, "home" | "detail") {
        result["state"]["reviewRequired"] = json!(true);
    }
    Ok(result)
}
fn focus_navigation(pane: &mut Value, request: &Value) {
    if let Some(section) = request["section"]
        .as_str()
        .filter(|section| matches!(*section, "docs" | "files" | "source"))
    {
        pane["state"]["section"] = json!(section);
    }
    if pane["kind"] != "home" {
        return;
    }
    if let Some(view) = request["view"]
        .as_str()
        .filter(|view| matches!(*view, "skills" | "changes" | "clients" | "settings"))
    {
        pane["state"]["view"] = json!(view);
    }
    if let Some(client) = request["clientId"]
        .as_str()
        .filter(|client| matches!(*client, "codex" | "claude" | "cursor"))
    {
        pane["state"]["clientId"] = json!(client);
    }
}
fn bounds(value: &Value) -> Option<Value> {
    let x = value["x"].as_f64()?;
    let y = value["y"].as_f64()?;
    let width = value["width"].as_f64()?;
    let height = value["height"].as_f64()?;
    (x.is_finite()
        && y.is_finite()
        && x.abs() <= 100_000.0
        && y.abs() <= 100_000.0
        && (640.0..=10000.0).contains(&width)
        && (480.0..=10000.0).contains(&height))
    .then(|| json!({"x":x,"y":y,"width":width,"height":height,"maximized":value["maximized"].as_bool().unwrap_or(false)}))
}
fn validate_runtime(value: &Value) -> Result<(), String> {
    fn forbidden(value: &Value) -> bool {
        match value {
            Value::Object(map) => map.iter().any(|(k, v)| {
                matches!(
                    k.as_str(),
                    "token" | "proxyPassword" | "proxyUsername" | "password" | "accessToken"
                ) || forbidden(v)
            }),
            Value::Array(items) => items.iter().any(forbidden),
            _ => false,
        }
    }
    if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > RUNTIME_LIMIT {
        return Err("页面临时状态过大，请先关闭不需要的预览".into());
    }
    if forbidden(value) {
        return Err("凭据不能保存在页面转移状态中".into());
    }
    Ok(())
}
impl Registry {
    fn new(saved: Option<Value>) -> Result<Self, String> {
        let mut layout = if let Some(value) = saved {
            serde_json::from_value::<Layout>(value)
                .map_err(|e| format!("无法读取窗口布局，原数据已保留：{e}"))?
        } else {
            Layout {
                revision: 0,
                windows: vec![],
            }
        };
        let mut ids = BTreeSet::new();
        for window in &mut layout.windows {
            if window.label != "main" && !window.label.starts_with("workspace-") {
                return Err("Invalid saved window label".into());
            }
            let mut panes = vec![home(&window.label)];
            ids.insert(home_id(&window.label));
            for item in &window.panes {
                if item["kind"] == "home" {
                    continue;
                }
                let next = pane(item, &window.label, true)?;
                if !ids.insert(next["id"].as_str().unwrap().to_owned()) {
                    return Err("Duplicate saved pane identity".into());
                }
                panes.push(next);
            }
            if panes.len() > MAX_PANES {
                return Err("Too many saved panes".into());
            }
            window.panes = panes;
            if !window
                .panes
                .iter()
                .any(|p| p["id"] == window.active_pane_id)
            {
                window.active_pane_id = home_id(&window.label);
            }
            window.bounds = window.bounds.as_ref().and_then(bounds);
            normalize_split(window);
        }
        if !layout.windows.iter().any(|w| w.label == "main") {
            layout.windows.insert(
                0,
                WindowRecord {
                    label: "main".into(),
                    panes: vec![home("main")],
                    active_pane_id: "home".into(),
                    bounds: None,
                    split_view: None,
                },
            );
        }
        Ok(Self {
            layout,
            live: BTreeSet::new(),
            closing_windows: BTreeSet::new(),
            transfers: BTreeMap::new(),
            runtime: BTreeMap::new(),
            export_selections: BTreeMap::new(),
            export_version: 0,
            close: None,
            last_focus: None,
        })
    }
    fn window(&self, label: &str) -> Result<&WindowRecord, String> {
        self.layout
            .windows
            .iter()
            .find(|w| w.label == label)
            .ok_or_else(|| "Window is no longer registered".into())
    }
    fn window_mut(&mut self, label: &str) -> Result<&mut WindowRecord, String> {
        self.layout
            .windows
            .iter_mut()
            .find(|w| w.label == label)
            .ok_or_else(|| "Window is no longer registered".into())
    }
    fn owner(&self, id: &str) -> Option<&str> {
        self.layout
            .windows
            .iter()
            .find(|w| w.panes.iter().any(|p| p["id"] == id))
            .map(|w| w.label.as_str())
    }
    fn export_context(
        &self,
        label: &str,
        request: &Value,
        pending_target: bool,
    ) -> Result<(String, String), String> {
        let skill = identifier(request, "skillId")?;
        let pane_id = identifier(request, "paneId")?;
        let owner = self.owner(&pane_id).ok_or("相关页面已关闭")?;
        if owner != label
            && !(pending_target
                && self
                    .transfers
                    .values()
                    .any(|t| t.pane_id == pane_id && t.target == label))
        {
            return Err("导出选择页面已移至其他窗口".into());
        }
        let pane = self
            .window(owner)?
            .panes
            .iter()
            .find(|p| p["id"] == pane_id)
            .ok_or("相关页面已关闭")?;
        if pane["skillId"].as_str().is_some_and(|id| id != skill) {
            return Err("导出选择与当前 Skill 不一致".into());
        }
        Ok((skill, pane_id))
    }
    fn export_event(&self, skill: &str) -> Value {
        json!({"skillId":skill,"version":self.export_version,"draft":self.export_selections.get(skill)})
    }
    fn related_export_pane(&self, skill: &str, except: Option<&str>) -> bool {
        self.layout
            .windows
            .iter()
            .flat_map(|w| w.panes.iter())
            .any(|pane| {
                pane["skillId"] == skill
                    && pane["id"].as_str() != except
                    && (pane["kind"] == "detail"
                        || pane["kind"] == "export"
                        || pane["state"]["operation"] == "export")
            })
    }
    fn get_export_selection(
        &mut self,
        label: &str,
        request: &Value,
    ) -> Result<(Value, Option<Value>), String> {
        let (skill, pane_id) = self.export_context(label, request, true)?;
        let created = !self.export_selections.contains_key(&skill);
        if created {
            if self.export_selections.len() >= MAX_PANES * 4 {
                return Err("打开的导出选择过多，请关闭不用的页面".into());
            }
            self.export_version += 1;
            self.export_selections.insert(
                skill.clone(),
                ExportSelection {
                    id: uuid::Uuid::new_v4().to_string(),
                    skill_id: skill.clone(),
                    revision: 0,
                    selected_ids: BTreeSet::new(),
                    version: self.export_version,
                    participants: BTreeMap::new(),
                },
            );
        }
        let owner = self.owner(&pane_id).map(str::to_owned);
        let draft = self
            .export_selections
            .get_mut(&skill)
            .expect("created export selection");
        // An acknowledged destination takes over its pane lease. During handoff,
        // keep the source lease valid so a failed destination cannot strand it.
        if owner.as_deref() == Some(label) {
            draft.participants.retain(|(_, pane), _| pane != &pane_id);
        }
        let lease = uuid::Uuid::new_v4().to_string();
        draft
            .participants
            .insert((label.to_owned(), pane_id), lease.clone());
        let response = json!({"draft":draft,"leaseId":lease});
        Ok((response, created.then(|| self.export_event(&skill))))
    }
    fn update_export_selection(
        &mut self,
        label: &str,
        request: &Value,
    ) -> Result<(Value, Option<Value>), String> {
        let (skill, pane_id) = self.export_context(label, request, false)?;
        let draft = self
            .export_selections
            .get(&skill)
            .ok_or("本次导出选择已结束，请重新打开")?;
        if request["operation"] == "clear"
            && request["expectedRevision"].is_u64()
            && request["draftId"] != draft.id
        {
            return Ok((json!({"draft":draft,"cleared":false,"changed":false}), None));
        }
        if request["draftId"] != draft.id
            || request["leaseId"].as_str()
                != draft
                    .participants
                    .get(&(label.to_owned(), pane_id))
                    .map(String::as_str)
        {
            return Err("本次导出选择已更新，请刷新后重试".into());
        }
        if request["expectedRevision"]
            .as_u64()
            .is_some_and(|revision| revision != draft.revision)
        {
            return Ok((json!({"draft":draft,"cleared":false,"changed":false}), None));
        }
        let mut selected = draft.selected_ids.clone();
        let operation = request["operation"].as_str().ok_or("缺少导出选择操作")?;
        match operation {
            "toggle" => {
                let id = export_resource_id(&request["resourceId"])?;
                match request["included"].as_bool().ok_or("缺少加入 ZIP 的选择")? {
                    true => {
                        selected.insert(id);
                    }
                    false => {
                        selected.remove(&id);
                    }
                }
            }
            "set" => {
                let values = request["selectedIds"]
                    .as_array()
                    .filter(|v| v.len() <= 2048)
                    .ok_or("一次最多选择 2048 项导出文件")?;
                selected = values
                    .iter()
                    .map(export_resource_id)
                    .collect::<Result<_, _>>()?;
            }
            "clear" => selected.clear(),
            _ => return Err("未知导出选择操作".into()),
        }
        if selected.len() > 2048 {
            return Err("一次最多选择 2048 项导出文件".into());
        }
        let changed = selected != draft.selected_ids;
        if changed {
            self.export_version += 1;
            let draft = self
                .export_selections
                .get_mut(&skill)
                .expect("existing export selection");
            draft.selected_ids = selected;
            draft.revision += 1;
            draft.version = self.export_version;
        }
        let response = json!({"draft":self.export_selections.get(&skill),"changed":changed,"cleared":operation=="clear"});
        Ok((response, changed.then(|| self.export_event(&skill))))
    }
    fn release_export_selection(
        &mut self,
        label: &str,
        request: &Value,
    ) -> Result<(Value, Option<Value>), String> {
        let skill = identifier(request, "skillId")?;
        let pane_id = identifier(request, "paneId")?;
        // Teardown from an old owner or a temporary migration-close is not the
        // user closing the final related page. Keep its existing shared draft.
        if self.close.is_some()
            || self.owner(&pane_id) != Some(label)
            || self.transfers.values().any(|t| t.pane_id == pane_id)
        {
            return Ok((json!({"released":false,"ignored":true}), None));
        }
        let Some(draft) = self.export_selections.get_mut(&skill) else {
            return Ok((json!({"released":false,"ignored":true}), None));
        };
        let participant = (label.to_owned(), pane_id);
        if request["draftId"] != draft.id
            || request["leaseId"].as_str()
                != draft.participants.get(&participant).map(String::as_str)
        {
            return Ok((json!({"released":false,"ignored":true}), None));
        }
        draft.participants.remove(&participant);
        if draft.participants.is_empty() && !self.related_export_pane(&skill, Some(&participant.1))
        {
            self.export_selections.remove(&skill);
            self.export_version += 1;
            return Ok((
                json!({"released":true,"removed":true}),
                Some(self.export_event(&skill)),
            ));
        }
        Ok((json!({"released":true,"removed":false}), None))
    }
    fn prune_export_selections(&mut self) -> Vec<Value> {
        let panes: BTreeSet<_> = self
            .layout
            .windows
            .iter()
            .flat_map(|w| w.panes.iter())
            .filter_map(|p| p["id"].as_str().map(str::to_owned))
            .collect();
        // Pane identity survives native-window transfers, including the short
        // gap before the destination subscribes. Closing that pane removes it.
        for draft in self.export_selections.values_mut() {
            draft
                .participants
                .retain(|(_, pane), _| panes.contains(pane));
        }
        let empty: Vec<_> = self
            .export_selections
            .iter()
            .filter(|(skill, d)| {
                d.participants.is_empty() && !self.related_export_pane(skill, None)
            })
            .map(|(s, _)| s.clone())
            .collect();
        empty
            .into_iter()
            .map(|skill| {
                self.export_selections.remove(&skill);
                self.export_version += 1;
                self.export_event(&skill)
            })
            .collect()
    }
    fn creation_window(&self, caller: &str, request: &Value) -> Result<String, String> {
        self.window(caller)?;
        if request.get("parentPaneId").is_some() {
            let parent = identifier(request, "parentPaneId")?;
            self.owner(&parent)
                .map(str::to_owned)
                .ok_or_else(|| "原页面已关闭，本次结果请从操作记录查看".into())
        } else {
            Ok(caller.to_owned())
        }
    }
    fn existing_identity_owner(&self, next: &Value) -> Result<Option<String>, String> {
        let id = next["id"].as_str().ok_or("Invalid pane identity")?;
        if let Some(owner) = self.owner(id) {
            let existing = self
                .window(owner)?
                .panes
                .iter()
                .find(|pane| pane["id"] == id)
                .ok_or("Pane disappeared")?;
            if existing["kind"] != next["kind"] || existing["skillId"] != next["skillId"] {
                return Err("页面标识已用于另一项操作，现有页面已保留".into());
            }
            Ok(Some(owner.to_owned()))
        } else {
            Ok(None)
        }
    }
    fn close_labels(&self, label: &str, all: bool) -> (bool, BTreeSet<String>) {
        let available: BTreeSet<_> = self
            .live
            .difference(&self.closing_windows)
            .cloned()
            .collect();
        let final_close = all || available.len() <= 1;
        (
            final_close,
            if final_close {
                available
            } else {
                BTreeSet::from([label.to_owned()])
            },
        )
    }
    fn payload(&self, label: &str) -> Value {
        let mut layout = serde_json::to_value(&self.layout).expect("layout serializable");
        layout["transfers"]=json!(self.transfers.values().map(|t|json!({"id":t.id,"paneId":t.pane_id,"sourceWindowLabel":t.source,"targetWindowLabel":t.target})).collect::<Vec<_>>());
        json!({"windowLabel":label,"workspace":layout})
    }
    fn restore(&self) -> Value {
        serde_json::to_value(&self.layout).expect("layout serializable")
    }
    fn replace_window(&mut self, label: &str, request: &Value) -> Result<(), String> {
        let values = request["panes"]
            .as_array()
            .ok_or("panes must be an array")?;
        if values.len() > MAX_PANES {
            return Err("Too many open panes".into());
        }
        let mut ids = BTreeSet::new();
        let mut panes = Vec::new();
        for value in values {
            let next = pane(value, label, false)?;
            let id = next["id"].as_str().unwrap();
            if !ids.insert(id.to_owned()) {
                return Err("Duplicate pane identity".into());
            }
            if self.owner(id).is_some_and(|owner| owner != label) {
                return Err("页面仍属于另一个窗口，请通过移动操作转移".into());
            }
            panes.push(next);
        }
        if !ids.contains(&home_id(label)) {
            panes.insert(0, home(label));
        }
        for transfer in self.transfers.values().filter(|t| t.source == label) {
            if !panes.iter().any(|p| p["id"] == transfer.pane_id) {
                return Err("目标窗口尚未接收页面，原页面必须保留".into());
            }
        }
        let active = request["activePaneId"].as_str().unwrap_or_default();
        let active = if panes.iter().any(|p| p["id"] == active) {
            active.to_owned()
        } else {
            home_id(label)
        };
        let window = self.window_mut(label)?;
        window.panes = panes;
        window.active_pane_id = active;
        if let Some(value) = request.get("splitView") {
            window.split_view = split_view(value, window)?;
        }
        normalize_split(window);
        let active = window.active_pane_id.clone();
        activate(window, &active);
        if request.get("bounds").is_some() {
            window.bounds = bounds(&request["bounds"]);
        }
        Ok(())
    }
}

fn export_resource_id(value: &Value) -> Result<String, String> {
    value
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 4096 && !id.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(|| "无效的导出文件标识".into())
}

pub(crate) struct WorkspaceSystem {
    registry: Mutex<Registry>,
    service: Arc<TaskSystem>,
    profile_dir: PathBuf,
    window_config: tauri::utils::config::WindowConfig,
    started: AtomicBool,
    close_wake: Condvar,
    migrating: AtomicBool,
}
impl WorkspaceSystem {
    pub(crate) fn new(
        service: Arc<TaskSystem>,
        _data_dir: PathBuf,
        window_config: tauri::utils::config::WindowConfig,
    ) -> Result<Arc<Self>, String> {
        let registry = Registry::new(service.workspace_state()?)?;
        Ok(Arc::new(Self {
            registry: Mutex::new(registry),
            profile_dir: service.storage()?.path("webview")?,
            service,
            window_config,
            started: AtomicBool::new(false),
            close_wake: Condvar::new(),
            migrating: AtomicBool::new(false),
        }))
    }
    fn persist(&self, r: &mut Registry) -> Result<(), String> {
        let previous = r.layout.revision;
        r.layout.revision += 1;
        if let Err(error) = self.service.store_workspace_state(&r.restore()) {
            r.layout.revision = previous;
            return Err(error);
        }
        Ok(())
    }
    fn emit(&self, app: &tauri::AppHandle) {
        if let Ok(r) = self.registry.lock() {
            let _ = app.emit("workspace-changed", r.payload("")["workspace"].clone());
        }
    }
    fn create_window(
        &self,
        app: &tauri::AppHandle,
        label: &str,
        geometry: Option<&Value>,
    ) -> Result<(), String> {
        let mut config = self.window_config.clone();
        config.label = label.to_owned();
        config.title = "Skill Manager".into();
        config.url = tauri::WebviewUrl::App("index.html".into());
        config.visible = true;
        config.create = true;
        config.resizable = true;
        config.width = 1280.0;
        config.height = 850.0;
        config.min_width = Some(640.0);
        config.min_height = Some(480.0);
        config.center = true;
        if let Some(g) = geometry.and_then(bounds) {
            config.width = g["width"].as_f64().unwrap();
            config.height = g["height"].as_f64().unwrap();
            config.x = Some(g["x"].as_f64().unwrap());
            config.y = Some(g["y"].as_f64().unwrap());
            config.center = false;
            config.maximized = g["maximized"] == true;
        }
        // Reuse one absolute D-root WebView profile; create only the bundled app.
        let window = tauri::WebviewWindowBuilder::from_config(app, &config)
            .map_err(|e| e.to_string())?
            .data_directory(self.profile_dir.clone())
            .build()
            .map_err(|e| e.to_string())?;
        self.registry
            .lock()
            .map_err(|_| "Workspace lock failed")?
            .live
            .insert(label.to_owned());
        // Clamp restored coordinates if the previously used monitor is gone.
        if let (Ok(position), Ok(size), Ok(monitors)) = (
            window.outer_position(),
            window.outer_size(),
            window.available_monitors(),
        ) {
            let visible = monitors.iter().any(|m| {
                let p = m.position();
                let s = m.size();
                position.x + size.width as i32 > p.x + 80
                    && position.y + 40 < p.y + s.height as i32
                    && position.x < p.x + s.width as i32 - 80
                    && position.y >= p.y - 40
            });
            if !visible {
                let _ = window.center();
            }
        }
        Ok(())
    }
    pub(crate) fn restore_windows(&self, app: &tauri::AppHandle) -> Result<(), String> {
        let records = self
            .registry
            .lock()
            .map_err(|_| "Workspace lock failed")?
            .layout
            .windows
            .clone();
        for record in records {
            if app.get_webview_window(&record.label).is_none() {
                self.create_window(app, &record.label, record.bounds.as_ref())?;
            }
        }
        Ok(())
    }
    pub(crate) fn destroyed(&self, app: &tauri::AppHandle, label: &str) {
        let mut export_events = Vec::new();
        if let Ok(mut r) = self.registry.lock() {
            r.live.remove(label);
            r.closing_windows.remove(label);
            let quitting = r
                .close
                .as_ref()
                .is_some_and(|c| c.labels.len() > 1 || c.labels.contains(label));
            if !quitting && !r.live.is_empty() {
                r.layout.windows.retain(|w| w.label != label);
                let _ = self.persist(&mut r);
            }
            r.transfers
                .retain(|_, t| t.source != label && t.target != label);
            export_events = r.prune_export_selections();
        }
        for event in export_events {
            let _ = app.emit("workspace-export-selection-changed", event);
        }
        self.emit(app);
    }
    fn schedule(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.service.background(
            json!({"action":"refresh","localOnly":true}),
            "正在刷新本地文件",
            crate::dispatch,
        );
        let _ = self.service.background(
            json!({"action":"refresh_clients"}),
            "正在发现客户端连接",
            crate::dispatch,
        );
        let audit = self.service.background(
            json!({"action":"audit_skills"}),
            "正在核对运行文件与远端内容",
            crate::dispatch,
        );
        self.service.match_external_background(
            audit
                .ok()
                .and_then(|task| task["id"].as_str().map(str::to_owned)),
        );
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(30 * 60));
            let Some(s) = weak.upgrade() else {
                break;
            };
            let refresh = s.service.background(
                json!({"action":"refresh"}),
                "正在检查本地与远端变化",
                crate::dispatch,
            );
            s.service.match_external_background(
                refresh
                    .ok()
                    .and_then(|task| task["id"].as_str().map(str::to_owned)),
            );
        });
    }
    pub(crate) fn inject_origin(&self, label: &str, request: &mut Value) -> Result<(), String> {
        if request["action"] != "start_task" {
            return Ok(());
        }
        if !request["origin"].is_object() {
            request["origin"] = json!({});
        }
        request["origin"]["windowLabel"] = json!(label);
        if let Some(id) = request["origin"]["paneId"].as_str() {
            let r = self.registry.lock().map_err(|_| "Workspace lock failed")?;
            let pending_read = r
                .transfers
                .values()
                .any(|transfer| transfer.pane_id == id && transfer.target == label)
                && crate::tasks::read_request(&request["request"]);
            if r.owner(id) != Some(label) && !pending_read {
                return Err("Task pane does not belong to this window".into());
            }
        }
        Ok(())
    }
    fn wait_close_acknowledgements(&self, id: &str, round: &str) -> Result<Vec<String>, String> {
        self.wait_close_until(id, round, Duration::from_secs(30))
    }
    fn wait_close_until(
        &self,
        id: &str,
        round: &str,
        timeout: Duration,
    ) -> Result<Vec<String>, String> {
        let deadline = Instant::now() + timeout;
        let mut registry = self.registry.lock().map_err(|_| "Workspace lock failed")?;
        loop {
            let close = registry
                .close
                .as_ref()
                .filter(|close| close.id == id)
                .ok_or("关闭已取消，窗口已保留")?;
            if let Some(error) = &close.error {
                return Err(error.clone());
            }
            if close.round == round && close.acknowledgements == close.labels {
                return Ok(close.labels.iter().cloned().collect());
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err("有窗口未能及时保存；所有仍打开的窗口已保留，请返回重试".into());
            };
            registry = self
                .close_wake
                .wait_timeout(registry, remaining)
                .map_err(|_| "Workspace close wait failed")?
                .0;
        }
    }
    fn cancel_close(&self, app: &tauri::AppHandle) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.close = None;
            registry.closing_windows.clear();
        }
        self.close_wake.notify_all();
        let _ = self
            .service
            .execute(json!({"action":"cancel_close"}), crate::dispatch);
        let _ = app.emit("workspace-close-cancelled", json!({}));
    }
    pub(crate) fn handle(
        self: &Arc<Self>,
        app: &tauri::AppHandle,
        label: &str,
        request: &Value,
    ) -> Result<Option<Value>, String> {
        let action = request["action"].as_str().unwrap_or_default();
        if !action.starts_with("workspace_") {
            return Ok(None);
        }
        if action == "workspace_ready" {
            self.schedule();
        }
        if action == "workspace_focus" {
            let run = {
                let mut r = self.registry.lock().map_err(|_| "Workspace lock failed")?;
                let run = r
                    .last_focus
                    .is_none_or(|t| t.elapsed() >= Duration::from_secs(15));
                if run {
                    r.last_focus = Some(Instant::now());
                }
                run
            };
            if run {
                let _ = self.service.background(
                    json!({"action":"refresh","localOnly":true}),
                    "正在刷新本地文件",
                    crate::dispatch,
                );
            }
            return Ok(Some(json!({"checked":run})));
        }
        if action == "workspace_detach" || action == "workspace_move" {
            let pane_id = identifier(request, "paneId")?;
            let (transfer_id, target, create, geometry) = {
                let mut r = self.registry.lock().map_err(|_| "Workspace lock failed")?;
                if r.owner(&pane_id) != Some(label) || pane_id == home_id(label) {
                    return Err("只能移动当前窗口中的业务页面".into());
                }
                if r.transfers.values().any(|t| t.pane_id == pane_id) {
                    return Err("页面正在转移，请等待目标窗口就绪".into());
                }
                let target = if action == "workspace_detach" {
                    format!("workspace-{}", uuid::Uuid::new_v4())
                } else {
                    identifier(request, "targetWindowLabel")?
                };
                if target == label {
                    return Ok(Some(r.payload(label)));
                }
                let create = !r.live.contains(&target);
                if !create {
                    r.window(&target)?;
                } else if action == "workspace_move" && target != "main" {
                    return Err("目标窗口已关闭".into());
                }
                if create {
                    let geometry = bounds(&request["bounds"]).or_else(|| {
                        let x = request["screenX"].as_f64()?;
                        let y = request["screenY"].as_f64()?;
                        bounds(&json!({"x":x-120.0,"y":y-20.0,"width":1100.0,"height":780.0}))
                    });
                    r.layout.windows.push(WindowRecord {
                        label: target.clone(),
                        panes: vec![home(&target)],
                        active_pane_id: home_id(&target),
                        bounds: geometry,
                        split_view: None,
                    });
                }
                let id = uuid::Uuid::new_v4().to_string();
                r.transfers.insert(
                    id.clone(),
                    Transfer {
                        id: id.clone(),
                        pane_id,
                        source: label.to_owned(),
                        target: target.clone(),
                        created: Instant::now(),
                    },
                );
                (
                    id,
                    target.clone(),
                    create,
                    r.window(&target)?.bounds.clone(),
                )
            };
            if create {
                if let Err(error) = self.create_window(app, &target, geometry.as_ref()) {
                    let mut r = self.registry.lock().map_err(|_| "Workspace lock failed")?;
                    r.transfers.remove(&transfer_id);
                    r.layout.windows.retain(|w| w.label != target);
                    return Err(error);
                }
            }
            self.emit(app);
            let _ = app.emit_to(
                target.as_str(),
                "workspace-transfer",
                json!({"transferId":transfer_id,"paneId":request["paneId"]}),
            );
            let mut reply = self
                .registry
                .lock()
                .map_err(|_| "Workspace lock failed")?
                .payload(label);
            reply["transferId"] = json!(transfer_id);
            reply["targetWindowLabel"] = json!(target);
            return Ok(Some(reply));
        }
        if action == "workspace_finish_close" {
            let close_id = identifier(request, "closeId")?;
            let outcome = (|| -> Result<Vec<String>, String> {
                let labels = self.wait_close_acknowledgements(&close_id, "initial")?;
                self.service
                    .execute(json!({"action":"prepare_close"}), crate::dispatch)?;
                {
                    let mut registry = self.registry.lock().map_err(|_| "Workspace lock failed")?;
                    let close = registry
                        .close
                        .as_mut()
                        .filter(|close| close.id == close_id)
                        .ok_or("关闭已取消，窗口已保留")?;
                    close.prepared = true;
                    close.round = "final".into();
                    close.acknowledgements.clear();
                }
                for target in &labels {
                    app.emit_to(
                        target.as_str(),
                        "workspace-close-request",
                        json!({"closeId":close_id,"round":"final","preserveRuntime":request["storagePreviewId"].is_string()}),
                    )
                    .map_err(|e| e.to_string())?;
                }
                self.wait_close_acknowledgements(&close_id, "final")
            })();
            let labels = match outcome {
                Ok(labels) => labels,
                Err(error) => {
                    self.cancel_close(app);
                    return Err(error);
                }
            };
            let migration = request["storagePreviewId"].as_str();
            if migration.is_some() {
                self.migrating.store(true, Ordering::SeqCst);
            }
            for target in labels {
                if let Some(window) = app.get_webview_window(&target) {
                    if let Err(error) = window.destroy() {
                        self.cancel_close(app);
                        self.migrating.store(false, Ordering::SeqCst);
                        return Err(error.to_string());
                    }
                }
            }
            if let Some(preview_id) = migration {
                // Keep the event loop alive with no windows while releasing the
                // WebView profile; a failed copy reopens the original workspace.
                let deadline = Instant::now() + Duration::from_secs(10);
                while !app.webview_windows().is_empty() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                let result = self.service.migrate_storage(preview_id);
                if let Err(error) = result {
                    let _ = self.service.storage()?.rollback_pending();
                    let _ = self.service.storage()?.record_failure(&error);
                    self.cancel_close(app);
                    let restored = self.restore_windows(app);
                    self.migrating.store(false, Ordering::SeqCst);
                    return Err(format!(
                        "保存位置未切换，原文件已保留：{error}{}",
                        restored
                            .err()
                            .map(|e| format!("；重新打开窗口失败：{e}"))
                            .unwrap_or_default()
                    ));
                }
                // request_restart preserves normal startup args and starts one
                // replacement process after this event loop has exited.
                app.request_restart();
                return Ok(Some(json!({"ready":true,"restarting":true})));
            }
            return Ok(Some(json!({"ready":true})));
        }
        let mut r = self.registry.lock().map_err(|_| "Workspace lock failed")?;
        r.window(label)?;
        // An unacknowledged transfer never displaces the source, even after failure.
        let expired: Vec<_> = r
            .transfers
            .iter()
            .filter(|(_, t)| t.created.elapsed() > Duration::from_secs(90))
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            r.transfers.remove(&id);
        }
        if matches!(
            action,
            "workspace_get_export_selection"
                | "workspace_update_export_selection"
                | "workspace_release_export_selection"
        ) {
            let (response, event) = match action {
                "workspace_get_export_selection" => r.get_export_selection(label, request)?,
                "workspace_update_export_selection" => r.update_export_selection(label, request)?,
                _ if self.migrating.load(Ordering::SeqCst) => {
                    (json!({"released":false,"ignored":true}), None)
                }
                _ => r.release_export_selection(label, request)?,
            };
            drop(r);
            if let Some(event) = event {
                let _ = app.emit("workspace-export-selection-changed", event);
            }
            return Ok(Some(response));
        }
        let mut changed = false;
        let result = match action {
            "workspace_get" => r.payload(label),
            "workspace_set_split" => {
                let before = r.layout.clone();
                let window = r.window_mut(label)?;
                window.split_view =
                    split_view(request.get("splitView").ok_or("缺少分栏设置")?, window)?;
                if let Err(error) = self.persist(&mut r) {
                    r.layout = before;
                    return Err(error);
                }
                changed = true;
                r.payload(label)
            }
            "workspace_save" => {
                if request["expectedRevision"]
                    .as_u64()
                    .is_some_and(|revision| revision != r.layout.revision)
                {
                    return Err("窗口布局已变化，请刷新后重新保存".into());
                }
                let before = r.layout.clone();
                r.replace_window(label, request)?;
                if let Err(e) = self.persist(&mut r) {
                    r.layout = before;
                    return Err(e);
                }
                changed = true;
                r.payload(label)
            }
            "workspace_update_pane" => {
                let id = identifier(request, "paneId")?;
                if r.owner(&id) != Some(label) {
                    return Err("页面已移至其他窗口".into());
                }
                let before = r.layout.clone();
                let window = r.window_mut(label)?;
                let current = window
                    .panes
                    .iter_mut()
                    .find(|p| p["id"] == id)
                    .ok_or("页面已关闭")?;
                let mut next = current.clone();
                if let Some(title) = request["title"].as_str() {
                    next["title"] = json!(title);
                }
                if let Some(state) = request["state"].as_object() {
                    for (k, v) in state {
                        next["state"][k] = v.clone();
                    }
                }
                *current = pane(&next, label, false)?;
                if let Err(e) = self.persist(&mut r) {
                    r.layout = before;
                    return Err(e);
                }
                changed = true;
                r.payload(label)
            }
            "workspace_activate" => {
                let id = identifier(request, "paneId")?;
                if r.owner(&id) != Some(label) {
                    return Err("页面已移至其他窗口".into());
                }
                let before = r.layout.clone();
                activate(r.window_mut(label)?, &id);
                if let Err(error) = self.persist(&mut r) {
                    r.layout = before;
                    return Err(error);
                }
                changed = true;
                r.payload(label)
            }
            "workspace_open_pane" => {
                let destination = r.creation_window(label, request)?;
                let next = pane(&request["pane"], &destination, false)?;
                let id = next["id"].as_str().unwrap().to_owned();
                if let Some(owner) = r.existing_identity_owner(&next)? {
                    if request["activate"] != false && r.window(&owner)?.active_pane_id != id {
                        let before = r.layout.clone();
                        activate(r.window_mut(&owner)?, &id);
                        if let Err(error) = self.persist(&mut r) {
                            r.layout = before;
                            return Err(error);
                        }
                        changed = true;
                    }
                } else {
                    if let Some(runtime) = request.get("runtime") {
                        validate_runtime(runtime)?;
                    }
                    let before = r.layout.clone();
                    let window = r.window_mut(&destination)?;
                    if window.panes.len() >= MAX_PANES {
                        return Err("Too many open panes".into());
                    }
                    window.panes.push(next);
                    if request["activate"] != false {
                        activate(window, &id);
                    }
                    if let Err(error) = self.persist(&mut r) {
                        r.layout = before;
                        return Err(error);
                    }
                    if let Some(runtime) = request.get("runtime") {
                        r.runtime.insert(id, runtime.clone());
                    }
                    changed = true;
                }
                r.payload(label)
            }
            "workspace_set_pane_runtime" => {
                let id = identifier(request, "paneId")?;
                if r.owner(&id) != Some(label) {
                    return Err("页面不属于当前窗口".into());
                }
                let runtime = request.get("runtime").ok_or("Missing runtime")?;
                validate_runtime(runtime)?;
                r.runtime.insert(id, runtime.clone());
                json!({"saved":true})
            }
            "workspace_get_pane_runtime" => {
                let id = identifier(request, "paneId")?;
                if r.owner(&id) != Some(label)
                    && !r
                        .transfers
                        .values()
                        .any(|t| t.pane_id == id && t.target == label)
                {
                    return Err("页面不属于当前窗口".into());
                }
                json!({"runtime":r.runtime.get(&id).cloned().unwrap_or(Value::Null)})
            }
            "workspace_ready" => {
                let transfer_id = request["transferId"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| {
                        r.transfers
                            .values()
                            .find(|t| t.target == label)
                            .map(|t| t.id.clone())
                    });
                if let Some(id) = transfer_id {
                    let transfer = r
                        .transfers
                        .get(&id)
                        .filter(|t| t.target == label)
                        .cloned()
                        .ok_or("页面转移已过期，原页面仍保留")?;
                    let before = r.layout.clone();
                    let item = r
                        .window(&transfer.source)?
                        .panes
                        .iter()
                        .find(|p| p["id"] == transfer.pane_id)
                        .cloned()
                        .ok_or("原页面已关闭")?;
                    let source = r.window_mut(&transfer.source)?;
                    source.panes.retain(|p| p["id"] != transfer.pane_id);
                    if source.active_pane_id == transfer.pane_id {
                        source.active_pane_id = home_id(&transfer.source);
                    }
                    normalize_split(source);
                    let target = r.window_mut(label)?;
                    target.panes.push(item);
                    activate(target, &transfer.pane_id);
                    r.layout.revision += 1;
                    if let Err(e) =
                        self.service
                            .store_workspace_move(&r.restore(), &transfer.pane_id, label)
                    {
                        r.layout = before;
                        return Err(e);
                    }
                    r.transfers.remove(&id);
                    changed = true;
                }
                r.payload(label)
            }
            "workspace_focus_pane" => {
                let id = identifier(request, "paneId")?;
                let owner = r.owner(&id).ok_or("页面已关闭")?.to_owned();
                let before = r.layout.clone();
                let section = request["section"]
                    .as_str()
                    .filter(|section| matches!(*section, "docs" | "files" | "source"));
                let target = r.window_mut(&owner)?;
                activate(target, &id);
                if let Some(pane) = target.panes.iter_mut().find(|pane| pane["id"] == id) {
                    focus_navigation(pane, request);
                }
                if let Err(error) = self.persist(&mut r) {
                    r.layout = before;
                    return Err(error);
                }
                if let Some(window) = app.get_webview_window(&owner) {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                let _ = app.emit_to(
                    owner.as_str(),
                    "workspace-activate",
                    json!({"paneId":id,"section":section}),
                );
                changed = true;
                r.payload(label)
            }
            "workspace_close_pane" => {
                let id = identifier(request, "paneId")?;
                if id == home_id(label) || r.owner(&id) != Some(label) {
                    return Err("工作台不能关闭，业务页面必须属于当前窗口".into());
                }
                if r.transfers.values().any(|t| t.pane_id == id) {
                    return Err("页面正在转移，请等待完成".into());
                }
                let before = r.layout.clone();
                let window = r.window_mut(label)?;
                window.panes.retain(|p| p["id"] != id);
                if window.active_pane_id == id {
                    window.active_pane_id = home_id(label);
                }
                normalize_split(window);
                if let Err(e) = self.persist(&mut r) {
                    r.layout = before;
                    return Err(e);
                }
                r.runtime.remove(&id);
                changed = true;
                r.payload(label)
            }
            "workspace_reset_layout" => {
                let before = r.layout.clone();
                for window in &mut r.layout.windows {
                    window.bounds = None;
                }
                if let Err(error) = self.persist(&mut r) {
                    r.layout = before;
                    return Err(error);
                }
                for window in app.webview_windows().values() {
                    let _ = window.unmaximize();
                    let _ = window.set_size(tauri::LogicalSize::new(1280.0, 850.0));
                    let _ = window.center();
                }
                changed = true;
                r.payload(label)
            }
            "workspace_close_intent" => {
                if r.close.is_some() {
                    return Err("已有窗口正在保存并关闭，请稍候".into());
                }
                if r.transfers
                    .values()
                    .any(|transfer| transfer.source == label)
                {
                    return Err("页面正在转移，请等待目标窗口接收后再关闭".into());
                }
                let all = request["all"] == true;
                if r.closing_windows.contains(label) {
                    return Err("当前窗口已开始关闭".into());
                }
                let (final_close, labels) = r.close_labels(label, all);
                r.closing_windows.insert(label.to_owned());
                let id = uuid::Uuid::new_v4().to_string();
                let pending: Vec<_> = labels
                    .iter()
                    .filter(|w| w.as_str() != label)
                    .cloned()
                    .collect();
                if final_close {
                    r.close = Some(CloseIntent {
                        id: id.clone(),
                        labels: labels.clone(),
                        acknowledgements: BTreeSet::from([label.to_owned()]),
                        error: None,
                        prepared: false,
                        round: "initial".into(),
                    });
                    for target in &pending {
                        let _ = app.emit_to(
                            target.as_str(),
                            "workspace-close-request",
                            json!({"closeId":id,"round":"initial","preserveRuntime":request["storageMigration"]==true}),
                        );
                    }
                }
                json!({"final":final_close,"closeId":id,"windowLabels":labels,"pendingWindowLabels":pending})
            }
            "workspace_close_ack" => {
                let id = identifier(request, "closeId")?;
                let Some(close) = r.close.as_mut().filter(|c| c.id == id) else {
                    return Ok(Some(json!({"ready":false,"ignored":true})));
                };
                if !close.labels.contains(label) {
                    return Err("当前窗口不属于此关闭请求".into());
                }
                if request["round"].as_str().unwrap_or("initial") != close.round {
                    return Ok(Some(
                        json!({"ready":false,"ignored":true,"round":close.round}),
                    ));
                }
                if request["ok"] != true {
                    close.error = Some("有窗口未能保存，请返回该窗口重试".into());
                } else {
                    close.acknowledgements.insert(label.to_owned());
                }
                self.close_wake.notify_all();
                json!({"ready":close.acknowledgements==close.labels,"round":close.round,"pendingWindowLabels":close.labels.difference(&close.acknowledgements).cloned().collect::<Vec<_>>(),"error":close.error})
            }
            "workspace_close_status" => {
                let close = r
                    .close
                    .as_ref()
                    .filter(|c| request["closeId"] == c.id)
                    .ok_or("Close request expired")?;
                json!({"ready":close.acknowledgements==close.labels,"round":close.round,"pendingWindowLabels":close.labels.difference(&close.acknowledgements).cloned().collect::<Vec<_>>(),"error":close.error})
            }
            "workspace_cancel_close" => {
                r.close = None;
                r.closing_windows.clear();
                self.close_wake.notify_all();
                let _ = self
                    .service
                    .execute(json!({"action":"cancel_close"}), crate::dispatch);
                let _ = app.emit("workspace-close-cancelled", json!({}));
                json!({"ready":true})
            }
            _ => return Err("Unknown workspace action".into()),
        };
        let export_events = if changed {
            r.prune_export_selections()
        } else {
            vec![]
        };
        drop(r);
        for event in export_events {
            let _ = app.emit("workspace-export-selection-changed", event);
        }
        if changed {
            self.emit(app);
        }
        Ok(Some(result))
    }
    pub(crate) fn migration_in_progress(&self) -> bool {
        self.migrating.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn export_registry() -> Registry {
        let mut r = Registry::new(None).unwrap();
        r.window_mut("main").unwrap().panes.extend([
            json!({"id":"detail-one","kind":"detail","skillId":"skill-one","state":{}}),
            json!({"id":"export-one","kind":"operation","skillId":"skill-one","state":{"operation":"export"}}),
        ]);
        r
    }
    fn selection_request(pane_id: &str) -> Value {
        json!({"skillId":"skill-one","paneId":pane_id})
    }
    fn selection_change(pane_id: &str, attached: &Value, operation: &str) -> Value {
        json!({"skillId":"skill-one","paneId":pane_id,"draftId":attached["draft"]["id"],"leaseId":attached["leaseId"],"operation":operation})
    }
    #[test]
    fn workspace_export_atomic_toggle_shares_both_panes_and_windows() {
        let mut r = export_registry();
        let (a, _) = r
            .get_export_selection("main", &selection_request("detail-one"))
            .unwrap();
        let export = r.window_mut("main").unwrap().panes.remove(2);
        r.layout.windows.push(WindowRecord {
            label: "workspace-shared".into(),
            panes: vec![home("workspace-shared"), export],
            active_pane_id: "export-one".into(),
            bounds: None,
            split_view: None,
        });
        let (b, _) = r
            .get_export_selection("workspace-shared", &selection_request("export-one"))
            .unwrap();
        assert_eq!(a["draft"]["id"], b["draft"]["id"]);
        let mut update = selection_change("detail-one", &a, "toggle");
        update["resourceId"] = json!("file:README.md");
        update["included"] = json!(true);
        let (first, event) = r.update_export_selection("main", &update).unwrap();
        assert_eq!(event.unwrap()["draft"]["revision"], 1);
        // The second pane can toggle using its original subscription without
        // replacing another pane's newer selection with an old complete array.
        update = selection_change("export-one", &b, "toggle");
        update["resourceId"] = json!("external:shared-rules");
        update["included"] = json!(true);
        let (second, _) = r
            .update_export_selection("workspace-shared", &update)
            .unwrap();
        assert_eq!(
            second["draft"]["selectedIds"],
            json!(["external:shared-rules", "file:README.md"])
        );
        assert!(second["draft"]["version"].as_u64() > first["draft"]["version"].as_u64());
        let revision = second["draft"]["revision"].clone();
        let (same, event) = r
            .update_export_selection("workspace-shared", &update)
            .unwrap();
        assert_eq!(same["draft"]["revision"], revision);
        assert!(event.is_none());
    }
    #[test]
    fn workspace_export_success_clear_does_not_erase_newer_or_new_epoch_choices() {
        let mut r = export_registry();
        let (a, _) = r
            .get_export_selection("main", &selection_request("detail-one"))
            .unwrap();
        let mut update = selection_change("detail-one", &a, "toggle");
        update["resourceId"] = json!("file:README.md");
        update["included"] = json!(true);
        r.update_export_selection("main", &update).unwrap();
        let mut clear = selection_change("detail-one", &a, "clear");
        clear["expectedRevision"] = json!(0);
        let (kept, event) = r.update_export_selection("main", &clear).unwrap();
        assert_eq!(kept["cleared"], false);
        assert!(event.is_none());
        assert_eq!(kept["draft"]["selectedIds"], json!(["file:README.md"]));
        clear["expectedRevision"] = json!(1);
        let (cleared, _) = r.update_export_selection("main", &clear).unwrap();
        assert_eq!(cleared["cleared"], true);
        assert_eq!(cleared["draft"]["selectedIds"], json!([]));
        // Same revision in a later draft is a different export session.
        r.export_selections.remove("skill-one");
        let (next, _) = r
            .get_export_selection("main", &selection_request("detail-one"))
            .unwrap();
        assert_ne!(a["draft"]["id"], next["draft"]["id"]);
        clear["expectedRevision"] = json!(0);
        clear["leaseId"] = next["leaseId"].clone();
        let (kept, _) = r.update_export_selection("main", &clear).unwrap();
        assert_eq!(kept["cleared"], false);
        assert_eq!(kept["draft"]["id"], next["draft"]["id"]);
    }
    #[test]
    fn workspace_export_old_effect_cleanup_cannot_release_new_same_pane_lease() {
        let mut r = export_registry();
        let (old, _) = r
            .get_export_selection("main", &selection_request("detail-one"))
            .unwrap();
        let (new, _) = r
            .get_export_selection("main", &selection_request("detail-one"))
            .unwrap();
        let stale_release = selection_change("detail-one", &old, "clear");
        let (reply, event) = r.release_export_selection("main", &stale_release).unwrap();
        assert_eq!(reply["released"], false);
        assert!(event.is_none());
        let mut update = selection_change("detail-one", &new, "toggle");
        update["resourceId"] = json!("file:README.md");
        update["included"] = json!(true);
        r.update_export_selection("main", &update).unwrap();
        assert_eq!(r.export_selections["skill-one"].selected_ids.len(), 1);
    }
    #[test]
    fn workspace_export_handoff_preserves_choices_and_rejects_old_owner_updates() {
        let mut r = export_registry();
        let (source, _) = r
            .get_export_selection("main", &selection_request("detail-one"))
            .unwrap();
        let mut update = selection_change("detail-one", &source, "toggle");
        update["resourceId"] = json!("external:rules");
        update["included"] = json!(true);
        r.update_export_selection("main", &update).unwrap();
        r.layout.windows.push(WindowRecord {
            label: "workspace-child".into(),
            panes: vec![home("workspace-child")],
            active_pane_id: home_id("workspace-child"),
            bounds: None,
            split_view: None,
        });
        r.transfers.insert(
            "moving".into(),
            Transfer {
                id: "moving".into(),
                pane_id: "detail-one".into(),
                source: "main".into(),
                target: "workspace-child".into(),
                created: Instant::now(),
            },
        );
        let release = selection_change("detail-one", &source, "clear");
        assert_eq!(
            r.release_export_selection("main", &release).unwrap().0["released"],
            false
        );
        let (pending, _) = r
            .get_export_selection("workspace-child", &selection_request("detail-one"))
            .unwrap();
        assert!(r
            .update_export_selection(
                "workspace-child",
                &selection_change("detail-one", &pending, "clear")
            )
            .is_err());
        // Pending destination does not invalidate the still-owned source lease.
        r.update_export_selection("main", &update).unwrap();
        let moving = r.window_mut("main").unwrap().panes.remove(1);
        r.window_mut("workspace-child").unwrap().panes.push(moving);
        r.transfers.remove("moving");
        assert!(r.prune_export_selections().is_empty());
        assert_eq!(
            r.release_export_selection("main", &release).unwrap().0["released"],
            false
        );
        assert!(r.update_export_selection("main", &update).is_err());
        let (target, _) = r
            .get_export_selection("workspace-child", &selection_request("detail-one"))
            .unwrap();
        assert_eq!(target["draft"]["id"], source["draft"]["id"]);
        assert_eq!(target["draft"]["selectedIds"], json!(["external:rules"]));
    }
    #[test]
    fn workspace_export_quick_new_export_pane_keeps_draft_until_last_related_close() {
        let mut r = export_registry();
        let (a, _) = r
            .get_export_selection("main", &selection_request("detail-one"))
            .unwrap();
        r.window_mut("main")
            .unwrap()
            .panes
            .retain(|p| p["id"] != "detail-one");
        // The new export pane has been created, but its initial subscription has
        // not arrived yet. Closing the origin must not drop the shared choice.
        assert!(r.prune_export_selections().is_empty());
        let (b, _) = r
            .get_export_selection("main", &selection_request("export-one"))
            .unwrap();
        assert_eq!(a["draft"]["id"], b["draft"]["id"]);
        r.window_mut("main")
            .unwrap()
            .panes
            .retain(|p| p["id"] != "export-one");
        let removed = r.prune_export_selections();
        assert_eq!(removed.len(), 1);
        assert!(removed[0]["draft"].is_null());
        assert!(r.export_selections.is_empty());
    }
    #[test]
    fn workspace_export_subscriber_release_keeps_other_window_and_ignores_close_flush() {
        let mut r = export_registry();
        let (a, _) = r
            .get_export_selection("main", &selection_request("detail-one"))
            .unwrap();
        let (b, _) = r
            .get_export_selection("main", &selection_request("export-one"))
            .unwrap();
        assert_eq!(
            r.release_export_selection("main", &selection_change("detail-one", &a, "clear"))
                .unwrap()
                .0["removed"],
            false
        );
        r.close = Some(CloseIntent {
            id: "migration-close".into(),
            labels: BTreeSet::from(["main".into()]),
            acknowledgements: BTreeSet::new(),
            error: None,
            prepared: false,
            round: "initial".into(),
        });
        assert_eq!(
            r.release_export_selection("main", &selection_change("export-one", &b, "clear"))
                .unwrap()
                .0["released"],
            false
        );
        r.close = None;
        let (after, _) = r
            .get_export_selection("main", &selection_request("export-one"))
            .unwrap();
        assert_eq!(after["draft"]["id"], b["draft"]["id"]);
    }
    #[test]
    fn workspace_export_selection_never_enters_sqlite_or_restored_runtime() {
        let (dir, workspace) = fixture();
        {
            let mut r = workspace.registry.lock().unwrap();
            let (a, _) = r
                .get_export_selection("main", &selection_request("home"))
                .unwrap();
            let mut update = selection_change("home", &a, "toggle");
            update["resourceId"] = json!("file:PRIVATE_TEST_EXPORT_CHOICE.md");
            update["included"] = json!(true);
            r.update_export_selection("main", &update).unwrap();
            workspace.persist(&mut r).unwrap();
        }
        let db = rusqlite::Connection::open(dir.path().join("manager.sqlite3")).unwrap();
        let saved: String = db
            .query_row(
                "SELECT payload FROM workspace_state WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!saved.contains("PRIVATE_TEST_EXPORT_CHOICE"));
        assert!(!saved.contains("selectedIds"));
        assert!(!saved.contains("export_selections"));
        let restored = Registry::new(Some(serde_json::from_str(&saved).unwrap())).unwrap();
        assert!(restored.export_selections.is_empty());
        assert_eq!(restored.export_version, 0);
    }
    #[test]
    fn workspace_export_invalid_ids_and_wrong_skill_do_not_change_draft() {
        let mut r = export_registry();
        let (a, _) = r
            .get_export_selection("main", &selection_request("detail-one"))
            .unwrap();
        let mut update = selection_change("detail-one", &a, "set");
        update["selectedIds"] = json!(["file:good.md", "bad\nidentifier"]);
        assert!(r.update_export_selection("main", &update).is_err());
        assert!(r.export_selections["skill-one"].selected_ids.is_empty());
        assert!(r
            .get_export_selection(
                "main",
                &json!({"skillId":"wrong-skill","paneId":"detail-one"})
            )
            .is_err());
        assert!(r
            .get_export_selection("main", &json!({"skillId":"skill-one","paneId":"closed"}))
            .is_err());
    }
    #[test]
    fn split_keeps_home_fixed_and_background_pages_do_not_change_right() {
        let mut registry = Registry::new(None).unwrap();
        let window = registry.window_mut("main").unwrap();
        window
            .panes
            .push(json!({"id":"detail-a","kind":"detail","state":{}}));
        window
            .panes
            .push(json!({"id":"operation-b","kind":"operation","state":{}}));
        window.split_view =
            split_view(&json!({"rightPaneId":"detail-a","ratio":1.2}), window).unwrap();
        assert_eq!(window.split_view.as_ref().unwrap()["ratio"], 0.9);
        activate(window, "home");
        assert_eq!(window.active_pane_id, "home");
        assert_eq!(
            window.split_view.as_ref().unwrap()["rightPaneId"],
            "detail-a"
        );
        window
            .panes
            .push(json!({"id":"background-result","kind":"operation","state":{}}));
        assert_eq!(
            window.split_view.as_ref().unwrap()["rightPaneId"],
            "detail-a"
        );
        activate(window, "operation-b");
        assert_eq!(
            window.split_view.as_ref().unwrap()["rightPaneId"],
            "operation-b"
        );
        assert!(split_view(&json!({"rightPaneId":"home"}), window).is_err());
        assert!(split_view(&json!({"rightPaneId":"other-window-pane"}), window).is_err());
    }
    #[test]
    fn split_survives_legacy_save_and_restore_and_falls_back_on_right_removal() {
        let mut registry = Registry::new(None).unwrap();
        let window = registry.window_mut("main").unwrap();
        window
            .panes
            .push(json!({"id":"a","kind":"detail","state":{}}));
        window
            .panes
            .push(json!({"id":"b","kind":"operation","state":{}}));
        window.split_view = Some(json!({"rightPaneId":"a","ratio":0.4}));
        let panes = window.panes.clone();
        registry
            .replace_window("main", &json!({"panes":panes,"activePaneId":"home"}))
            .unwrap();
        let restored = Registry::new(Some(registry.restore())).unwrap();
        assert_eq!(
            restored.window("main").unwrap().split_view,
            Some(json!({"rightPaneId":"a","ratio":0.4}))
        );
        let window = registry.window_mut("main").unwrap();
        window.panes.retain(|p| p["id"] != "a");
        normalize_split(window);
        assert_eq!(window.split_view.as_ref().unwrap()["rightPaneId"], "b");
        window.panes.retain(|p| p["kind"] == "home");
        normalize_split(window);
        assert!(window.split_view.is_none());
        assert!(Registry::new(None)
            .unwrap()
            .window("main")
            .unwrap()
            .split_view
            .is_none());
    }
    fn fixture() -> (tempfile::TempDir, Arc<WorkspaceSystem>) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/workspace-tests");
        std::fs::create_dir_all(&root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        let manager = crate::model::Manager::new(dir.path().into()).unwrap();
        let tasks = TaskSystem::new(manager).unwrap();
        let workspace = WorkspaceSystem::new(tasks, dir.path().into(), Default::default()).unwrap();
        (dir, workspace)
    }
    #[test]
    fn injected_task_window_comes_from_native_caller_and_rejects_other_pane_owner() {
        let (_dir, workspace) = fixture();
        let mut request = json!({"action":"start_task","request":{"action":"get_state"},"origin":{"paneId":"home","windowLabel":"spoofed"}});
        workspace.inject_origin("main", &mut request).unwrap();
        assert_eq!(request["origin"]["windowLabel"], "main");
        assert!(workspace
            .inject_origin("workspace-other", &mut request)
            .is_err());
    }
    #[test]
    fn close_requires_all_windows_to_acknowledge_the_latest_round() {
        let (_dir, workspace) = fixture();
        {
            let mut r = workspace.registry.lock().unwrap();
            r.close = Some(CloseIntent {
                id: "close-1".into(),
                labels: BTreeSet::from(["main".into(), "workspace-child".into()]),
                acknowledgements: BTreeSet::from(["main".into(), "workspace-child".into()]),
                error: None,
                prepared: false,
                round: "initial".into(),
            });
        }
        assert_eq!(
            workspace
                .wait_close_until("close-1", "initial", Duration::from_millis(1))
                .unwrap()
                .len(),
            2
        );
        {
            let mut r = workspace.registry.lock().unwrap();
            let close = r.close.as_mut().unwrap();
            close.round = "final".into();
            close.acknowledgements.clear();
            close.acknowledgements.insert("main".into());
        }
        assert!(workspace
            .wait_close_until("close-1", "final", Duration::from_millis(10))
            .is_err());
        {
            let mut r = workspace.registry.lock().unwrap();
            r.close
                .as_mut()
                .unwrap()
                .acknowledgements
                .insert("workspace-child".into());
        }
        assert!(workspace
            .wait_close_until("close-1", "final", Duration::from_millis(1))
            .is_ok());
    }
    #[test]
    fn simultaneous_window_close_reserves_the_first_and_last_still_prepares_global_tasks() {
        let mut registry = Registry::new(None).unwrap();
        registry.live = BTreeSet::from(["main".into(), "workspace-child".into()]);
        let (first, labels) = registry.close_labels("main", false);
        assert!(!first);
        assert_eq!(labels, BTreeSet::from(["main".into()]));
        registry.closing_windows.insert("main".into());
        let (last, labels) = registry.close_labels("workspace-child", false);
        assert!(last);
        assert_eq!(labels, BTreeSet::from(["workspace-child".into()]));
        registry.closing_windows.clear();
        assert!(!registry.close_labels("workspace-child", false).0);
    }
    #[test]
    fn a_late_operation_creation_follows_the_parent_current_owner_without_creating_windows() {
        let mut registry = Registry::new(None).unwrap();
        registry
            .window_mut("main")
            .unwrap()
            .panes
            .push(json!({"id":"parent-detail","kind":"detail","state":{}}));
        let request = json!({"parentPaneId":"parent-detail","pane":{"id":"operation-result","kind":"operation","state":{"operation":"runtime"}},"windowLabel":"forged-window"});
        assert_eq!(registry.creation_window("main", &request).unwrap(), "main");
        let parent = registry.window_mut("main").unwrap().panes.pop().unwrap();
        registry.layout.windows.push(WindowRecord {
            label: "workspace-child".into(),
            panes: vec![home("workspace-child"), parent],
            active_pane_id: "parent-detail".into(),
            bounds: None,
            split_view: None,
        });
        assert_eq!(
            registry.creation_window("main", &request).unwrap(),
            "workspace-child"
        );
        assert_eq!(
            registry.creation_window("main", &json!({})).unwrap(),
            "main"
        );
        assert!(registry.creation_window("forged-window", &request).is_err());
        registry.window_mut("workspace-child").unwrap().panes.pop();
        assert!(registry.creation_window("main", &request).is_err());
        assert_eq!(registry.layout.windows.len(), 2);
    }
    #[test]
    fn deterministic_outcome_identity_reuses_existing_pane_and_preserves_its_draft() {
        let mut registry = Registry::new(None).unwrap();
        let outcome = json!({"id":"outcome-task-one","kind":"operation","skillId":"skill-one","state":{"operation":"runtime","scrollTop":88}});
        registry
            .window_mut("main")
            .unwrap()
            .panes
            .push(outcome.clone());
        registry.runtime.insert(
            "outcome-task-one".into(),
            json!({"selectedFiles":["SKILL.md"],"input":"current user choice"}),
        );
        let duplicate = json!({"id":"outcome-task-one","kind":"operation","skillId":"skill-one","state":{"operation":"files","scrollTop":0}});
        assert_eq!(
            registry.existing_identity_owner(&duplicate).unwrap(),
            Some("main".into())
        );
        assert_eq!(registry.window("main").unwrap().panes[1], outcome);
        assert_eq!(
            registry.runtime["outcome-task-one"]["input"],
            "current user choice"
        );
        let mut conflict = duplicate;
        conflict["skillId"] = json!("another-skill");
        assert!(registry.existing_identity_owner(&conflict).is_err());
        conflict["skillId"] = json!("skill-one");
        conflict["kind"] = json!("detail");
        assert!(registry.existing_identity_owner(&conflict).is_err());
    }
    #[test]
    fn workspace_metadata_failure_does_not_advance_revision_or_write_runtime() {
        let (dir, workspace) = fixture();
        {
            let mut r = workspace.registry.lock().unwrap();
            r.runtime
                .insert("home".into(), json!({"document":"memory-only preview"}));
            workspace.persist(&mut r).unwrap();
        }
        let db = rusqlite::Connection::open(dir.path().join("manager.sqlite3")).unwrap();
        let stored: String = db
            .query_row("SELECT payload FROM workspace_state", [], |row| row.get(0))
            .unwrap();
        assert!(!stored.contains("memory-only"));
        db.execute_batch("CREATE TRIGGER stop_workspace BEFORE INSERT ON workspace_state BEGIN SELECT RAISE(ABORT,'synthetic failure'); END").unwrap();
        let mut r = workspace.registry.lock().unwrap();
        let revision = r.layout.revision;
        assert!(workspace.persist(&mut r).is_err());
        assert_eq!(r.layout.revision, revision);
    }
    #[test]
    fn restored_layout_only_keeps_navigation_and_marks_write_pages_for_review() {
        let r=Registry::new(Some(json!({"revision":4,"windows":[{"label":"main","activePaneId":"export-1","panes":[{"id":"export-1","kind":"operation","state":{"operation":"export","scrollTop":42,"document":"private content","destination":"private path","selectedFiles":["x"],"token":"secret"}}]}]}))).unwrap();
        let p = &r.layout.windows[0].panes[1];
        assert_eq!(p["state"]["reviewRequired"], true);
        assert_eq!(p["state"]["scrollTop"], 42.0);
        for key in ["document", "destination", "selectedFiles", "token"] {
            assert!(p["state"].get(key).is_none());
        }
    }
    #[test]
    fn history_restores_as_read_only_navigation_without_results_or_replay_state() {
        let r = Registry::new(Some(json!({"revision":7,"windows":[{"label":"main","activePaneId":"history-main","panes":[{"id":"history-main","kind":"history","state":{"scrollTop":230,"reviewRequired":true,"search":"private filter","status":"failed","result":"private result","request":{"action":"apply"}}}]}]}))).unwrap();
        let history = &r.layout.windows[0].panes[1];
        assert_eq!(r.layout.revision, 7);
        assert_eq!(r.layout.windows[0].active_pane_id, "history-main");
        assert_eq!(history["kind"], "history");
        assert_eq!(history["state"], json!({"scrollTop":230.0}));
        assert!(r.runtime.is_empty());
        assert!(!r.restore().to_string().contains("private"));
        assert!(pane(
            &json!({"id":"history-live","kind":"history","state":{"reviewRequired":true}}),
            "main",
            false
        )
        .unwrap()["state"]
            .get("reviewRequired")
            .is_none());
    }
    #[test]
    fn focus_navigation_restores_home_context_without_a_generic_cross_window_update() {
        let mut target = home("workspace-child");
        target["state"] = json!({"view":"settings","clientId":"codex","scrollTop":77.0});
        focus_navigation(
            &mut target,
            &json!({"view":"clients","clientId":"cursor","section":"files","runtime":{"request":"never"},"scrollTop":900}),
        );
        assert_eq!(
            target["state"],
            json!({"view":"clients","clientId":"cursor","section":"files","scrollTop":77.0})
        );
        for view in ["skills", "changes", "clients", "settings"] {
            focus_navigation(&mut target, &json!({"view":view}));
            assert_eq!(target["state"]["view"], view);
        }
        for client in ["codex", "claude", "cursor"] {
            focus_navigation(&mut target, &json!({"clientId":client}));
            assert_eq!(target["state"]["clientId"], client);
        }
        let before = target.clone();
        focus_navigation(
            &mut target,
            &json!({"view":"execute","clientId":"other","section":"apply"}),
        );
        assert_eq!(target, before);
        let mut detail = json!({"id":"detail-focus","kind":"detail","state":{"section":"docs"}});
        focus_navigation(
            &mut detail,
            &json!({"view":"clients","clientId":"cursor","section":"source"}),
        );
        assert_eq!(detail["state"], json!({"section":"source"}));
        let restored = pane(&target, "workspace-child", true).unwrap();
        assert_eq!(restored["state"], target["state"]);
    }
    #[test]
    fn history_views_keep_distinct_runtime_drafts_and_do_not_gain_kind_based_reuse() {
        let mut r = Registry::new(None).unwrap();
        r.window_mut("main")
            .unwrap()
            .panes
            .push(json!({"id":"history-a","kind":"history","state":{}}));
        let second = json!({"id":"history-b","kind":"history","state":{}});
        assert_eq!(r.existing_identity_owner(&second).unwrap(), None);
        r.layout.windows.push(WindowRecord {
            label: "workspace-history".into(),
            panes: vec![home("workspace-history"), second],
            active_pane_id: "history-b".into(),
            bounds: None,
            split_view: None,
        });
        r.runtime.insert(
            "history-a".into(),
            json!({"search":"first view","status":"active","scrollTop":90}),
        );
        r.runtime.insert(
            "history-b".into(),
            json!({"search":"second view","status":"ended","scrollTop":180}),
        );
        assert_eq!(r.owner("history-a"), Some("main"));
        assert_eq!(r.owner("history-b"), Some("workspace-history"));
        assert_eq!(r.runtime["history-a"]["search"], "first view");
        assert_eq!(r.runtime["history-b"]["search"], "second view");
        assert!(!r.restore().to_string().contains("first view"));
        assert!(!r.restore().to_string().contains("second view"));
    }
    #[test]
    fn expanded_fold_keys_survive_restoration_whole_with_matching_utf16_limits() {
        let key = format!("file:D:/materials/{}:references", "文件 😀".repeat(130));
        let limit = "😀".repeat(2048);
        let over_limit = "😀".repeat(2049);
        let source = json!({"id":"detail-long-fold","kind":"detail","state":{"expandedSections":[format!("  {key}  "),key,limit,over_limit,"",25]}});
        let first = pane(&source, "main", false).unwrap();
        assert_eq!(first["state"]["expandedSections"], json!([key, limit]));
        let restored = pane(&first, "main", true).unwrap();
        assert_eq!(
            restored["state"]["expandedSections"],
            first["state"]["expandedSections"]
        );
        let entries: Vec<String> = (0..120).map(|n| format!("fold-{n}")).collect();
        let capped = pane(
            &json!({"id":"detail-capped","kind":"detail","state":{"expandedSections":entries}}),
            "main",
            true,
        )
        .unwrap();
        assert_eq!(
            capped["state"]["expandedSections"]
                .as_array()
                .unwrap()
                .len(),
            100
        );
    }
    #[test]
    fn save_cannot_claim_other_window_panes_or_drop_unacknowledged_source() {
        let mut r = Registry::new(None).unwrap();
        r.layout.windows.push(WindowRecord {
            label: "workspace-test".into(),
            panes: vec![
                home("workspace-test"),
                json!({"id":"detail-a","kind":"detail","state":{}}),
            ],
            active_pane_id: "detail-a".into(),
            bounds: None,
            split_view: None,
        });
        assert!(r
            .replace_window(
                "main",
                &json!({"panes":[{"id":"detail-a","kind":"detail"}]})
            )
            .is_err());
        r.transfers.insert(
            "transfer".into(),
            Transfer {
                id: "transfer".into(),
                pane_id: "detail-a".into(),
                source: "workspace-test".into(),
                target: "main".into(),
                created: Instant::now(),
            },
        );
        assert!(r
            .replace_window("workspace-test", &json!({"panes":[]}))
            .is_err());
        assert_eq!(r.owner("detail-a"), Some("workspace-test"));
    }
    #[test]
    fn homes_are_unique_and_runtime_is_bounded_never_persisted() {
        let r = Registry::new(None).unwrap();
        assert_eq!(r.layout.windows[0].panes[0]["id"], "home");
        assert_eq!(home("workspace-x")["id"], "home:workspace-x");
        assert!(validate_runtime(&json!({"request":{"token":"secret"}})).is_err());
        assert!(validate_runtime(&json!({"description":"preview content"})).is_ok());
        assert!(r.restore().get("runtime").is_none());
    }
    #[test]
    fn corrupted_or_duplicate_saved_panes_fail_without_silent_replacement() {
        assert!(Registry::new(Some(json!({"revision":0,"windows":[{"label":"https://other","panes":[],"activePaneId":"home"}]}))).is_err());
        assert!(Registry::new(Some(json!({"revision":0,"windows":[{"label":"main","panes":[{"id":"x","kind":"detail"},{"id":"x","kind":"detail"}],"activePaneId":"x"}]}))).is_err());
    }
}
