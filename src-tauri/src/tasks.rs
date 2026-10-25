//! Background operations share one durable state writer, never a long I/O lock.
use crate::model::{now, Manager, ManagerRuntime, State};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, Weak,
    },
    time::Duration,
};

const HISTORY_LIMIT: usize = 100;
const CANCELLED: &str = "任务已取消";
type Dispatch = fn(&mut Manager, &Value) -> Result<Value, String>;
type EventSink = Arc<dyn Fn(Value) + Send + Sync>;
type Completion = Arc<(Mutex<Option<Result<Value, String>>>, Condvar)>;
type ProgressSink = Arc<dyn Fn(&str, Option<usize>, Option<usize>) + Send + Sync>;
thread_local! { static PROGRESS: RefCell<Option<ProgressSink>> = const { RefCell::new(None) }; }

pub(crate) fn report_progress(stage: &str, completed: Option<usize>, total: Option<usize>) {
    let sink = PROGRESS.with(|slot| slot.borrow().clone());
    if let Some(sink) = sink {
        sink(stage, completed, total);
    }
}
fn with_progress<T>(sink: ProgressSink, run: impl FnOnce() -> T) -> T {
    struct Reset(Option<ProgressSink>);
    impl Drop for Reset {
        fn drop(&mut self) {
            PROGRESS.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _reset = Reset(PROGRESS.with(|slot| slot.replace(Some(sink))));
    run()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Task {
    id: String,
    action: String,
    label: String,
    status: String,
    stage: String,
    created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finished_at: Option<String>,
    #[serde(default)]
    origin: Value,
    #[serde(default)]
    progress: Value,
    #[serde(default)]
    cancellable: bool,
    #[serde(default)]
    cancel_requested: bool,
    #[serde(default)]
    result_available: bool,
    #[serde(default)]
    result_expired: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(default)]
    resources: Vec<String>,
}

impl Task {
    fn terminal(&self) -> bool {
        !matches!(self.status.as_str(), "queued" | "running")
    }
    fn metadata(&self) -> Value {
        let mut value = serde_json::to_value(self).expect("serializable task");
        value.as_object_mut().expect("task object").remove("result");
        value
    }
}

struct Job {
    task: Task,
    cancel: Arc<AtomicBool>,
    completion: Completion,
    visible: bool,
    recorded: bool,
    consumers: usize,
    dedup: Option<String>,
}
fn history_records(jobs: &BTreeMap<String, Job>) -> Vec<Value> {
    let mut terminal: Vec<_> = jobs
        .values()
        .filter(|job| job.visible && job.task.terminal())
        .map(|job| job.task.metadata())
        .collect();
    terminal.sort_by(|a, b| b["finishedAt"].as_str().cmp(&a["finishedAt"].as_str()));
    terminal.truncate(HISTORY_LIMIT);
    terminal.extend(
        jobs.values()
            .filter(|job| job.visible && !job.task.terminal())
            .map(|job| job.task.metadata()),
    );
    terminal
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Resource {
    key: String,
    write: bool,
}

fn conflicts(left: &[Resource], right: &[Resource]) -> bool {
    left.iter()
        .any(|a| right.iter().any(|b| a.key == b.key && (a.write || b.write)))
}

#[derive(Default)]
struct Scheduler {
    waiting: Vec<(String, bool, Vec<Resource>)>,
    active: HashMap<String, Vec<Resource>>,
}

pub(crate) struct TaskSystem {
    manager: Arc<Mutex<Manager>>,
    jobs: Mutex<BTreeMap<String, Job>>,
    scheduler: Mutex<Scheduler>,
    wake: Condvar,
    events: Mutex<Option<EventSink>>,
    closing: AtomicBool,
    history_lock: Mutex<()>,
}

impl TaskSystem {
    pub(crate) fn new(manager: Manager) -> Result<Arc<Self>, String> {
        let history = manager.task_history()?;
        let mut jobs = BTreeMap::new();
        for value in history {
            let mut task: Task = serde_json::from_value(value)
                .map_err(|e| format!("无法读取操作记录；原数据库已保留：{e}"))?;
            if !task.terminal() {
                task.status = "interrupted".into();
                task.stage = "上次运行已中断，请检查当前状态后重新操作".into();
                task.finished_at = Some(now());
                task.error = Some("程序在任务完成前关闭；不会自动重放操作".into());
            }
            task.cancellable = false;
            task.result = None;
            task.result_available = false;
            task.result_expired = true;
            jobs.insert(
                task.id.clone(),
                Job {
                    task,
                    cancel: Arc::new(AtomicBool::new(false)),
                    completion: Arc::new((Mutex::new(None), Condvar::new())),
                    visible: true,
                    recorded: true,
                    consumers: 0,
                    dedup: None,
                },
            );
        }
        let service = Arc::new(Self {
            manager: Arc::new(Mutex::new(manager)),
            jobs: Mutex::new(jobs),
            scheduler: Mutex::new(Scheduler::default()),
            wake: Condvar::new(),
            events: Mutex::new(None),
            closing: AtomicBool::new(false),
            history_lock: Mutex::new(()),
        });
        {
            let manager = service
                .manager
                .lock()
                .map_err(|_| "Manager state lock failed")?;
            crate::network::configure(&manager.state.settings)?;
        }
        service.persist_history()?;
        Ok(service)
    }

    pub(crate) fn set_events(&self, events: EventSink) {
        *self.events.lock().expect("task events lock") = Some(events);
    }
    pub(crate) fn set_state_events(
        &self,
        events: Arc<dyn Fn(u64) + Send + Sync>,
    ) -> Result<(), String> {
        self.manager
            .lock()
            .map_err(|_| "Manager state lock failed")?
            .set_state_events(events);
        Ok(())
    }
    pub(crate) fn workspace_state(&self) -> Result<Option<Value>, String> {
        self.manager
            .lock()
            .map_err(|_| "Manager state lock failed")?
            .workspace_state()
    }
    pub(crate) fn storage(&self) -> Result<Arc<crate::storage::Storage>, String> {
        Ok(self
            .manager
            .lock()
            .map_err(|_| "Manager state lock failed")?
            .storage
            .clone())
    }
    pub(crate) fn migrate_storage(&self, preview_id: &str) -> Result<(), String> {
        let manager = self
            .manager
            .lock()
            .map_err(|_| "Manager state lock failed")?;
        manager
            .storage
            .migrate(preview_id, |destination| manager.copy_database(destination))
    }
    pub(crate) fn match_external_background(
        self: &Arc<Self>,
        after: Option<String>,
    ) -> std::thread::JoinHandle<()> {
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            let Some(service) = weak.upgrade() else {
                return;
            };
            if let Some(id) = after {
                let _ = service.wait(&id);
            }
            if let Ok(snapshot) = service.snapshot() {
                for skill in snapshot["skills"].as_array().into_iter().flatten() {
                    if service.closing.load(Ordering::Relaxed) {
                        break;
                    }
                    if skill["localPath"].is_string()
                        && skill["repo"].is_string()
                        && skill["sourceId"].is_string()
                        && skill["readOnly"] != true
                    {
                        let Some(commit) = skill["commit"].as_str().filter(|v| v.len() == 40)
                        else {
                            continue;
                        };
                        let inventory=service.start(json!({"action":"get_external_dependencies","skillId":skill["id"],"remoteCommit":commit}),Some("正在核对外部文件来源"),None,false,false,crate::dispatch);
                        let Ok(inventory) = inventory else {
                            continue;
                        };
                        let Some(id) = inventory["id"].as_str() else {
                            continue;
                        };
                        let report = service.wait(id);
                        service.release_compat(id);
                        if let Ok(report) = report {
                            let items:Vec<_>=report["items"].as_array().into_iter().flatten().filter(|item|!matches!(item["referenceKind"].as_str(),Some("command"|"skill_alias"))).map(|item|json!({"id":item["id"],"expectedFingerprint":item["referenceFingerprint"]})).collect();
                            if !items.is_empty() {
                                let _=service.background(json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":skill["id"],"remoteCommit":commit,"items":items}),"正在匹配外部文件来源",crate::dispatch);
                            }
                        }
                    }
                }
            }
        })
    }
    pub(crate) fn store_workspace_state(&self, value: &Value) -> Result<(), String> {
        self.manager
            .lock()
            .map_err(|_| "Manager state lock failed")?
            .store_workspace_state(value)
    }
    pub(crate) fn store_workspace_move(
        &self,
        layout: &Value,
        pane_id: &str,
        window_label: &str,
    ) -> Result<(), String> {
        let _history = self
            .history_lock
            .lock()
            .map_err(|_| "Task history lock failed")?;
        let mut jobs = self.jobs.lock().map_err(|_| "Task registry lock failed")?;
        let previous: Vec<_> = jobs
            .iter()
            .filter(|(_, job)| job.task.origin["paneId"] == pane_id)
            .map(|(id, job)| (id.clone(), job.task.origin.clone()))
            .collect();
        for (id, _) in &previous {
            jobs.get_mut(id).expect("registered task").task.origin["windowLabel"] =
                json!(window_label);
        }
        let records = history_records(&jobs);
        let result = self
            .manager
            .lock()
            .map_err(|_| "Manager state lock failed")?
            .store_workspace_with_history(layout, &records);
        if let Err(error) = result {
            for (id, origin) in previous {
                jobs.get_mut(&id).expect("registered task").task.origin = origin;
            }
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn background(
        self: &Arc<Self>,
        request: Value,
        label: &str,
        dispatch: Dispatch,
    ) -> Result<Value, String> {
        self.start(
            request,
            Some(label),
            Some(&json!({"view":"skills","windowLabel":"application"})),
            false,
            true,
            dispatch,
        )
    }

    pub(crate) fn snapshot(&self) -> Result<Value, String> {
        Ok(self
            .manager
            .lock()
            .map_err(|_| "Manager state lock failed")?
            .snapshot())
    }

    pub(crate) fn control(
        self: &Arc<Self>,
        request: &Value,
        dispatch: Dispatch,
    ) -> Result<Option<Value>, String> {
        Ok(Some(match request["action"].as_str().unwrap_or_default() {
            "get_state" => self.snapshot()?,
            "start_task" => {
                let business = request
                    .get("request")
                    .filter(|v| v.is_object())
                    .ok_or("start_task requires a business request")?;
                let task = self.start(
                    business.clone(),
                    request["label"].as_str(),
                    request.get("origin"),
                    request["foreground"] != false,
                    true,
                    dispatch,
                )?;
                json!({"task":task})
            }
            "get_task" => json!({"task":self.get(text(request,"taskId")?,true)?}),
            "list_tasks" => json!({"tasks":self.list()?}),
            "cancel_task" => json!({"task":self.cancel(text(request,"taskId")?)?}),
            "clear_task_history" => {
                let cleared = self.clear_history()?;
                json!({"tasks":self.list()?,"clearedTaskIds":cleared})
            }
            "prepare_close" => self.prepare_close()?,
            "cancel_close" => {
                self.closing.store(false, Ordering::Relaxed);
                json!({"ready":true})
            }
            _ => return Ok(None),
        }))
    }

    pub(crate) fn execute(
        self: &Arc<Self>,
        request: Value,
        dispatch: Dispatch,
    ) -> Result<Value, String> {
        if let Some(value) = self.control(&request, dispatch)? {
            return Ok(value);
        }
        let task = self.start(request, None, None, true, false, dispatch)?;
        let id = task["id"].as_str().ok_or("Invalid task ID")?;
        let outcome = self.wait(id);
        self.release_compat(id);
        outcome
    }

    fn start(
        self: &Arc<Self>,
        request: Value,
        label: Option<&str>,
        origin: Option<&Value>,
        foreground: bool,
        visible: bool,
        dispatch: Dispatch,
    ) -> Result<Value, String> {
        let action = text(&request, "action")?.to_owned();
        if matches!(
            action.as_str(),
            "start_task"
                | "get_task"
                | "list_tasks"
                | "cancel_task"
                | "clear_task_history"
                | "get_state"
                | "prepare_close"
                | "cancel_close"
        ) {
            return Err("此请求不能作为后台业务任务启动".into());
        }
        if self.closing.load(Ordering::Relaxed) && action != "set_settings" {
            return Err("程序正在保存并关闭，暂不开始新任务".into());
        }
        let (resources, dedup) = {
            let manager = self
                .manager
                .lock()
                .map_err(|_| "Manager state lock failed")?;
            (
                resources_for(&manager, &request)?,
                auto_match_dedup(&manager, &request).or_else(|| dedup_key(&request)),
            )
        };
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let task = Task {
            id: id.clone(),
            action: action.clone(),
            label: redact(label.unwrap_or(&action), &request)
                .chars()
                .take(240)
                .collect(),
            status: "queued".into(),
            stage: "等待执行".into(),
            created_at: now(),
            started_at: None,
            finished_at: None,
            origin: clean_origin(origin, &request),
            progress: Value::Null,
            cancellable: true,
            cancel_requested: false,
            result_available: false,
            result_expired: false,
            error: None,
            result: None,
            resources: resources.iter().map(|r| r.key.clone()).collect(),
        };
        let metadata = task.metadata();
        {
            let mut jobs = self.jobs.lock().map_err(|_| "Task registry lock failed")?;
            if let Some(key) = &dedup {
                if let Some(existing) = jobs.values_mut().find(|job| {
                    !job.task.terminal()
                        && !job.task.cancel_requested
                        && job.dedup.as_ref() == Some(key)
                }) {
                    if visible {
                        existing.visible = true;
                        existing.recorded = true;
                    } else {
                        existing.consumers += 1;
                    }
                    return Ok(existing.task.metadata());
                }
            }
            jobs.insert(
                id.clone(),
                Job {
                    task,
                    cancel: cancel.clone(),
                    completion: Arc::new((Mutex::new(None), Condvar::new())),
                    visible,
                    recorded: visible,
                    consumers: usize::from(!visible),
                    dedup,
                },
            );
        }
        if let Err(error) = self.persist_visible(&id) {
            self.jobs
                .lock()
                .map_err(|_| "Task registry lock failed")?
                .remove(&id);
            return Err(format!("未能保存任务记录，任务未开始：{error}"));
        }
        self.emit(&id);
        let service = self.clone();
        std::thread::spawn(move || {
            service.run(id, request, resources, foreground, cancel, dispatch)
        });
        Ok(metadata)
    }

    fn run(
        self: Arc<Self>,
        id: String,
        request: Value,
        resources: Vec<Resource>,
        foreground: bool,
        cancel: Arc<AtomicBool>,
        dispatch: Dispatch,
    ) {
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value, String> {
                let _resources = self.acquire(&id, resources, foreground, &cancel)?;
                if cancel.load(Ordering::Relaxed) {
                    return Err(CANCELLED.into());
                }
                let read = read_request(&request);
                {
                    let mut jobs = self.jobs.lock().map_err(|_| "Task registry lock failed")?;
                    let task = &mut jobs.get_mut(&id).ok_or("任务不存在")?.task;
                    if cancel.load(Ordering::Relaxed) {
                        return Err(CANCELLED.into());
                    }
                    task.status = "running".into();
                    task.started_at = Some(now());
                    task.stage = if read {
                        "正在读取与核对，可在后台继续"
                    } else {
                        "正在执行，关键写入期间不可中止"
                    }
                    .into();
                    task.cancellable = read;
                }
                self.emit(&id);
                self.persist_visible(&id)?;
                let (mut worker, hook) = {
                    let manager = self
                        .manager
                        .lock()
                        .map_err(|_| "Manager state lock failed")?;
                    let hook = Arc::new(WorkerHook {
                        service: Arc::downgrade(&self),
                        base: Mutex::new(WorkerBase {
                            state: manager.state.clone(),
                            previews: manager.previews.clone(),
                            undo: manager.management_undo.clone(),
                        }),
                        action: request["action"].as_str().unwrap_or_default().to_owned(),
                        request: request.clone(),
                        cancel: cancel.clone(),
                        cancellable: read,
                    });
                    (manager.detached(hook.clone()), hook)
                };
                let progress_service = Arc::downgrade(&self);
                let progress_id = id.clone();
                let result = with_progress(
                    Arc::new(move |stage, completed, total| {
                        if let Some(service) = progress_service.upgrade() {
                            service.change(&progress_id, |task| {
                                if !task.cancel_requested {
                                    task.stage = stage.into();
                                    task.progress = match (completed, total) {
                                        (Some(completed), Some(total)) => {
                                            json!({"completed":completed,"total":total})
                                        }
                                        _ => Value::Null,
                                    };
                                }
                            });
                        }
                    }),
                    || {
                        crate::network::with_task_context(
                            crate::network::TaskContext {
                                foreground,
                                cancel: if read {
                                    cancel.clone()
                                } else {
                                    Arc::new(AtomicBool::new(false))
                                },
                            },
                            || dispatch(&mut worker, &request),
                        )
                    },
                );
                // A consumed preview stays consumed on failure; undo changes made
                // after a successful save are published before a result is exposed.
                hook.publish_ephemeral(&worker)?;
                let mut value = result?;
                if read && cancel.load(Ordering::Relaxed) {
                    return Err(CANCELLED.into());
                }
                rebase_result(&mut value, &self.snapshot()?);
                Ok(value)
            }))
            .unwrap_or_else(|_| Err("后台操作异常终止；请检查当前状态后重试".into()));
        self.finish(&id, outcome, &request, cancel.load(Ordering::Relaxed));
    }

    fn finish(&self, id: &str, outcome: Result<Value, String>, request: &Value, cancelled: bool) {
        let completion = {
            let mut jobs = self.jobs.lock().expect("task registry lock");
            let Some(job) = jobs.get_mut(id) else {
                return;
            };
            job.task.finished_at = Some(now());
            job.task.cancellable = false;
            match &outcome {
                Ok(value) => {
                    job.task.status = if partial(value) {
                        "partial"
                    } else {
                        "succeeded"
                    }
                    .into();
                    job.task.stage = if partial(value) {
                        "操作完成，部分项目需要处理"
                    } else {
                        "操作完成"
                    }
                    .into();
                    if value["autoMatchSummary"].is_object() {
                        let summary = &value["autoMatchSummary"];
                        job.task.stage = format!(
                            "已匹配 {} · 记录修复 {} · 远端就绪 {} · 补齐需预览 {} · 待选择 {} · 缺失 {} · 异常 {}",
                            summary["matched"].as_u64().unwrap_or(0),
                            summary["recordsRepaired"].as_u64().unwrap_or(0),
                            summary["remoteReady"].as_u64().unwrap_or(0),
                            summary["localPreviewRequired"].as_u64().unwrap_or(0),
                            summary["needsSelection"].as_u64().unwrap_or(0),
                            summary["missing"].as_u64().unwrap_or(0),
                            summary["error"].as_u64().unwrap_or(0)
                        );
                    }
                    job.task.result = Some(value.clone());
                    job.task.result_available = true;
                    job.task.progress = result_progress(value);
                }
                Err(error) => {
                    let is_cancel =
                        cancelled && (error == CANCELLED || read_task(&job.task.action));
                    job.task.status = if is_cancel { "cancelled" } else { "failed" }.into();
                    job.task.stage = if is_cancel {
                        "已停止，未开始的操作不再执行"
                    } else {
                        "操作未完成，请查看原因"
                    }
                    .into();
                    job.task.error = Some(redact(error, request));
                }
            }
            job.completion.clone()
        };
        if let Err(error) = self.persist_visible(id) {
            self.change(id, |task| {
                task.error = Some(format!("操作结果已保留在当前会话；历史保存失败：{error}"));
            });
        }
        self.emit(id);
        self.prune();
        *completion.0.lock().expect("task completion lock") = Some(outcome);
        completion.1.notify_all();
    }

    fn acquire(
        self: &Arc<Self>,
        id: &str,
        resources: Vec<Resource>,
        foreground: bool,
        cancel: &AtomicBool,
    ) -> Result<ResourceGuard, String> {
        let mut scheduler = self
            .scheduler
            .lock()
            .map_err(|_| "Task queue lock failed")?;
        scheduler
            .waiting
            .push((id.into(), foreground, resources.clone()));
        loop {
            if cancel.load(Ordering::Relaxed) {
                scheduler.waiting.retain(|(key, _, _)| key != id);
                self.wake.notify_all();
                return Err(CANCELLED.into());
            }
            let index = scheduler
                .waiting
                .iter()
                .position(|(key, _, _)| key == id)
                .ok_or("Task left queue")?;
            let active_conflict = scheduler
                .active
                .values()
                .any(|held| conflicts(held, &resources));
            let waiting_conflict =
                scheduler
                    .waiting
                    .iter()
                    .enumerate()
                    .any(|(other, (key, priority, wants))| {
                        key != id
                            && conflicts(wants, &resources)
                            && ((*priority && !foreground)
                                || (*priority == foreground && other < index))
                    });
            if !active_conflict && !waiting_conflict {
                scheduler.waiting.remove(index);
                scheduler.active.insert(id.into(), resources);
                return Ok(ResourceGuard {
                    service: self.clone(),
                    id: id.into(),
                });
            }
            scheduler = self
                .wake
                .wait_timeout(scheduler, Duration::from_millis(100))
                .map_err(|_| "Task queue wait failed")?
                .0;
        }
    }

    fn wait(&self, id: &str) -> Result<Value, String> {
        let completion = self
            .jobs
            .lock()
            .map_err(|_| "Task registry lock failed")?
            .get(id)
            .ok_or("任务不存在")?
            .completion
            .clone();
        let mut value = completion
            .0
            .lock()
            .map_err(|_| "Task completion lock failed")?;
        while value.is_none() {
            value = completion
                .1
                .wait(value)
                .map_err(|_| "Task completion wait failed")?;
        }
        value.clone().expect("completed task")
    }
    fn release_compat(&self, id: &str) {
        let mut jobs = self.jobs.lock().expect("task registry lock");
        let remove = jobs.get_mut(id).is_some_and(|job| {
            job.consumers -= 1;
            !job.recorded && job.consumers == 0 && job.task.terminal()
        });
        if remove {
            jobs.remove(id);
        }
    }

    fn get(&self, id: &str, result: bool) -> Result<Value, String> {
        let jobs = self.jobs.lock().map_err(|_| "Task registry lock failed")?;
        let job = jobs.get(id).ok_or("操作记录已移除或不存在")?;
        let mut value = if result {
            serde_json::to_value(&job.task).map_err(|e| e.to_string())?
        } else {
            job.task.metadata()
        };
        if !job.visible {
            value["hidden"] = json!(true);
        }
        Ok(value)
    }
    fn list(&self) -> Result<Vec<Value>, String> {
        let jobs = self.jobs.lock().map_err(|_| "Task registry lock failed")?;
        let mut tasks: Vec<_> = jobs
            .values()
            .filter(|job| job.visible)
            .map(|job| job.task.metadata())
            .collect();
        tasks.sort_by(|a, b| b["createdAt"].as_str().cmp(&a["createdAt"].as_str()));
        Ok(tasks)
    }
    fn cancel(&self, id: &str) -> Result<Value, String> {
        {
            let mut jobs = self.jobs.lock().map_err(|_| "Task registry lock failed")?;
            let job = jobs.get_mut(id).ok_or("任务不存在")?;
            if job.task.terminal() {
                return Ok(job.task.metadata());
            }
            if !job.task.cancellable {
                return Err("任务正在执行关键写入，完成后会给出结果".into());
            }
            job.cancel.store(true, Ordering::Relaxed);
            job.task.cancel_requested = true;
            job.task.stage = "正在停止；当前网络请求返回后结束".into();
        }
        self.wake.notify_all();
        self.emit(id);
        self.persist_history()?;
        self.get(id, false)
    }
    fn change(&self, id: &str, change: impl FnOnce(&mut Task)) {
        if let Some(job) = self.jobs.lock().expect("task registry lock").get_mut(id) {
            change(&mut job.task);
        }
        self.emit(id);
    }
    fn emit(&self, id: &str) {
        let task = {
            let jobs = self.jobs.lock().expect("task registry lock");
            let Some(job) = jobs.get(id).filter(|job| job.visible) else {
                return;
            };
            job.task.metadata()
        };
        if let Some(events) = self.events.lock().expect("task events lock").clone() {
            events(json!({"task":task}));
        }
    }
    fn persist_history(&self) -> Result<(), String> {
        let _history = self
            .history_lock
            .lock()
            .map_err(|_| "Task history lock failed")?;
        let records = {
            let jobs = self.jobs.lock().map_err(|_| "Task registry lock failed")?;
            history_records(&jobs)
        };
        self.manager
            .lock()
            .map_err(|_| "Manager state lock failed")?
            .store_task_history(&records)
    }
    fn persist_visible(&self, id: &str) -> Result<(), String> {
        let visible = self
            .jobs
            .lock()
            .map_err(|_| "Task registry lock failed")?
            .get(id)
            .is_some_and(|job| job.visible);
        if visible {
            self.persist_history()
        } else {
            Ok(())
        }
    }
    fn prune(&self) {
        let mut jobs = self.jobs.lock().expect("task registry lock");
        let mut terminal: Vec<_> = jobs
            .iter()
            .filter(|(_, job)| job.recorded && job.task.terminal())
            .map(|(id, job)| (id.clone(), job.task.finished_at.clone()))
            .collect();
        terminal.sort_by(|a, b| b.1.cmp(&a.1));
        for (id, _) in terminal.into_iter().skip(HISTORY_LIMIT) {
            if jobs.get(&id).is_some_and(|job| job.consumers == 0) {
                jobs.remove(&id);
            }
        }
        // Compatibility calls are not user-visible history entries. A completed
        // caller still owns its completion Arc after removal.
        jobs.retain(|_, job| {
            job.recorded
                || !job.task.terminal()
                || job.consumers > 0
                || Arc::strong_count(&job.completion) > 1
        });
    }
    fn clear_history(&self) -> Result<Vec<String>, String> {
        let cleared = {
            let mut jobs = self.jobs.lock().map_err(|_| "Task registry lock failed")?;
            jobs.values_mut()
                .filter_map(|job| {
                    if job.task.terminal() && job.visible {
                        job.visible = false;
                        Some(job.task.id.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        if let Err(error) = self.persist_history() {
            let mut jobs = self.jobs.lock().map_err(|_| "Task registry lock failed")?;
            for id in &cleared {
                if let Some(job) = jobs.get_mut(id) {
                    job.visible = true;
                }
            }
            return Err(error);
        }
        if let Some(events) = self
            .events
            .lock()
            .map_err(|_| "Task events lock failed")?
            .clone()
        {
            events(json!({"clearedTaskIds":cleared,"tasks":self.list()?}));
        }
        Ok(cleared)
    }
    fn prepare_close(&self) -> Result<Value, String> {
        self.closing.store(true, Ordering::Relaxed);
        let running = {
            let mut jobs = self.jobs.lock().map_err(|_| "Task registry lock failed")?;
            jobs.values_mut()
                .filter(|job| !job.task.terminal())
                .map(|job| {
                    if job.task.cancellable {
                        job.cancel.store(true, Ordering::Relaxed);
                        job.task.cancel_requested = true;
                        job.task.stage = "程序正在关闭，等待本次读取停止".into();
                    }
                    (job.task.id.clone(), job.completion.clone())
                })
                .collect::<Vec<_>>()
        };
        self.wake.notify_all();
        for (id, _) in &running {
            self.emit(id);
        }
        for (_, completion) in running {
            let mut value = completion
                .0
                .lock()
                .map_err(|_| "Task completion lock failed")?;
            while value.is_none() {
                value = completion
                    .1
                    .wait(value)
                    .map_err(|_| "Task completion wait failed")?;
            }
        }
        self.persist_history()?;
        Ok(json!({"ready":true}))
    }
}

struct ResourceGuard {
    service: Arc<TaskSystem>,
    id: String,
}
impl Drop for ResourceGuard {
    fn drop(&mut self) {
        if let Ok(mut scheduler) = self.service.scheduler.lock() {
            scheduler.active.remove(&self.id);
        }
        self.service.wake.notify_all();
    }
}

struct WorkerBase {
    state: State,
    previews: HashMap<String, Value>,
    undo: Vec<Value>,
}
struct WorkerHook {
    service: Weak<TaskSystem>,
    base: Mutex<WorkerBase>,
    action: String,
    request: Value,
    cancel: Arc<AtomicBool>,
    cancellable: bool,
}

impl ManagerRuntime for WorkerHook {
    fn save(&self, worker: &Manager) -> Result<(), String> {
        if self.cancellable && self.cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.into());
        }
        let service = self
            .service
            .upgrade()
            .ok_or("Task service is unavailable")?;
        let mut base = self.base.lock().map_err(|_| "Task merge lock failed")?;
        validate_observations(&base.state, &worker.state, &self.action)?;
        let mut manager = service
            .manager
            .lock()
            .map_err(|_| "Manager state lock failed")?;
        let merged = merge_state(&base.state, &worker.state, &manager.state, &self.action)?;
        if matches!(self.action.as_str(), "get_files" | "get_export_plan")
            && merged.skills == manager.state.skills
            && merged.sources == manager.state.sources
            && merged.connections == manager.state.connections
            && merged.settings == manager.state.settings
        {
            base.state = worker.state.clone();
            return Ok(());
        }
        let previous = std::mem::replace(&mut manager.state, merged);
        if let Err(error) = manager.save() {
            manager.state = previous;
            return Err(error);
        }
        base.state = worker.state.clone();
        Ok(())
    }
    fn snapshot(&self) -> Value {
        self.service
            .upgrade()
            .and_then(|service| service.snapshot().ok())
            .unwrap_or(Value::Null)
    }
}

fn validate_observations(before: &State, after: &State, action: &str) -> Result<(), String> {
    if !matches!(
        action,
        "refresh" | "audit_skills" | "auto_associate" | "get_files" | "get_export_plan"
    ) {
        return Ok(());
    }
    for skill in &after.skills {
        let Some(prior) = before
            .skills
            .iter()
            .find(|prior| prior["id"] == skill["id"])
        else {
            continue;
        };
        let audit_changed = prior["runtimeAudit"] != skill["runtimeAudit"]
            && skill["runtimeAudit"]["scopeHashes"].is_object();
        let hashes_changed = prior["lastLocalCheckedAt"] != skill["lastLocalCheckedAt"]
            && skill["localHashes"].is_object();
        if !audit_changed && !hashes_changed {
            continue;
        }
        let Some(path) = skill["localPath"].as_str() else {
            continue;
        };
        let current = crate::local::files(Path::new(path))?;
        if audit_changed {
            if skill["runtimeAudit"]["inventoryHashes"].is_object()
                && skill["runtimeAudit"]["inventoryHashes"] != json!(current)
            {
                return Err(
                    "本地文件在核对期间发生变化；本次过期核对结果未保存，请重新检查".into(),
                );
            }
            let scope = &skill["runtimeAudit"]["scopeHashes"];
            if scope
                .as_object()
                .expect("hash object")
                .iter()
                .any(|(path, hash)| current.get(path).map(String::as_str) != hash.as_str())
            {
                return Err(
                    "本地文件在核对期间发生变化；本次过期核对结果未保存，请重新检查".into(),
                );
            }
        }
        if hashes_changed
            && skill["localHashes"]
                .as_object()
                .expect("hash object")
                .iter()
                .any(|(path, hash)| current.get(path).map(String::as_str) != hash.as_str())
        {
            return Err(
                "本地文件在远端检查期间发生变化；本次过期比较结果未保存，请重新检查".into(),
            );
        }
    }
    Ok(())
}

impl WorkerHook {
    fn publish_ephemeral(&self, worker: &Manager) -> Result<(), String> {
        let service = self
            .service
            .upgrade()
            .ok_or("Task service is unavailable")?;
        let mut base = self.base.lock().map_err(|_| "Task merge lock failed")?;
        let mut manager = service
            .manager
            .lock()
            .map_err(|_| "Manager state lock failed")?;
        if worker.management_undo != base.undo {
            if manager.management_undo != base.undo {
                return Err("管理操作记录已变化，请重新核对后撤销".into());
            }
            manager.management_undo = worker.management_undo.clone();
            base.undo = worker.management_undo.clone();
        }
        if let Some(id) = self.request["previewId"]
            .as_str()
            .or_else(|| self.request["planId"].as_str())
        {
            if base.previews.contains_key(id) && !worker.previews.contains_key(id) {
                manager.previews.remove(id);
            }
        }
        for (id, preview) in &worker.previews {
            if base.previews.get(id) != Some(preview) {
                if manager.previews.get(id).is_some_and(|current| {
                    Some(current) != base.previews.get(id) && current != preview
                }) {
                    return Err("预览已变化，请重新生成".into());
                }
                manager.previews.insert(id.clone(), preview.clone());
            }
        }
        crate::export_plan::trim_previews(&mut manager.previews);
        Ok(())
    }
}

fn merge_state(
    before: &State,
    after: &State,
    current: &State,
    action: &str,
) -> Result<State, String> {
    let mut merged = current.clone();
    merged.skills = merge_records(
        &before.skills,
        &after.skills,
        &current.skills,
        "Skill",
        action,
    )?;
    merged.sources = merge_records(
        &before.sources,
        &after.sources,
        &current.sources,
        "来源",
        action,
    )?;
    merged.connections = merge_records(
        &before.connections,
        &after.connections,
        &current.connections,
        "客户端连接",
        action,
    )?;
    merged.settings = merge_object(
        &before.settings,
        &after.settings,
        &current.settings,
        "设置",
        &["ignoredClientConnections"],
    )?;
    if before.settings.get("ignoredClientConnections")
        != after.settings.get("ignoredClientConnections")
    {
        let old = before.settings["ignoredClientConnections"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let next = after.settings["ignoredClientConnections"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut live = current.settings["ignoredClientConnections"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        live.retain(|entry| !old.contains(entry) || next.contains(entry));
        for entry in next.iter().filter(|entry| !old.contains(entry)) {
            if !live.contains(entry) {
                live.push(entry.clone());
            }
        }
        merged.settings["ignoredClientConnections"] = json!(live);
    }
    Ok(merged)
}

fn merge_records(
    before: &[Value],
    after: &[Value],
    current: &[Value],
    kind: &str,
    action: &str,
) -> Result<Vec<Value>, String> {
    let mut merged = current.to_vec();
    for prior in before {
        let id = &prior["id"];
        let next = after.iter().find(|item| item["id"] == *id);
        let live = merged.iter().position(|item| item["id"] == *id);
        match (next, live) {
            (None, Some(index)) => {
                if merged[index] != *prior {
                    return Err(format!("{kind}记录在任务运行期间发生变化，请重新操作"));
                }
                merged.remove(index);
            }
            (Some(next), Some(index)) if next != prior => {
                if kind == "Skill" && !management_task(action) {
                    for field in [
                        "localPath",
                        "sourceId",
                        "repo",
                        "branch",
                        "remotePath",
                        "runtimeFiles",
                        "externalBindings",
                        "installedCommit",
                        "baselineCommit",
                    ] {
                        if merged[index].get(field) != prior.get(field)
                            && next.get(field) != merged[index].get(field)
                        {
                            return Err(format!(
                                "{kind}的{field}在任务运行期间发生变化，本次过期结果未写入"
                            ));
                        }
                    }
                }
                let protected = if kind == "Skill"
                    && matches!(
                        action,
                        "refresh" | "audit_skills" | "auto_associate" | "add_github"
                    ) {
                    &["categoryId", "favorite", "externalBindings"][..]
                } else {
                    &[]
                };
                if kind == "Skill" && matches!(action, "get_files" | "get_export_plan") {
                    let mut prior = prior.clone();
                    let mut next = next.clone();
                    let live = &merged[index];
                    if same_runtime_audit_proof(&next["runtimeAudit"], &live["runtimeAudit"]) {
                        next["runtimeAudit"] = live["runtimeAudit"].clone();
                    } else if same_runtime_audit_proof(
                        &prior["runtimeAudit"],
                        &live["runtimeAudit"],
                    ) {
                        prior["runtimeAudit"] = live["runtimeAudit"].clone();
                    }
                    merged[index] = merge_object(&prior, &next, live, kind, protected)?;
                } else {
                    merged[index] = merge_object(prior, next, &merged[index], kind, protected)?;
                }
            }
            (Some(next), None) if next != prior => {
                return Err(format!("{kind}记录已被移除，本次结果未写入"))
            }
            _ => {}
        }
    }
    for item in after
        .iter()
        .filter(|item| !before.iter().any(|prior| prior["id"] == item["id"]))
    {
        if let Some(live) = merged.iter().find(|live| live["id"] == item["id"]) {
            if live != item {
                return Err(format!("{kind}记录已存在，请刷新后操作"));
            }
        } else {
            merged.push(item.clone());
        }
    }
    Ok(merged)
}

fn same_runtime_audit_proof(left: &Value, right: &Value) -> bool {
    fn proof(value: &Value) -> Option<Value> {
        if value["method"] != "static_package" {
            return None;
        }
        let mut value = value.clone();
        let record = value.as_object_mut()?;
        record.remove("checkedAt");
        record.remove("invalidatedAt");
        if let Some(dependencies) = record
            .get_mut("externalDependencies")
            .and_then(Value::as_array_mut)
        {
            for dependency in dependencies {
                if let Some(record) = dependency.as_object_mut() {
                    record.remove("checkedAt");
                    for source in ["local", "remote"] {
                        if let Some(evidence) =
                            record.get_mut(source).and_then(Value::as_object_mut)
                        {
                            evidence.remove("checkedAt");
                        }
                    }
                }
            }
        }
        Some(value)
    }
    match (proof(left), proof(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn merge_object(
    before: &Value,
    after: &Value,
    current: &Value,
    kind: &str,
    protected: &[&str],
) -> Result<Value, String> {
    let (Some(prior), Some(next), Some(live)) =
        (before.as_object(), after.as_object(), current.as_object())
    else {
        return if current == before || current == after {
            Ok(after.clone())
        } else {
            Err(format!("{kind}在任务运行期间已变化"))
        };
    };
    let mut merged = live.clone();
    let keys: BTreeSet<_> = prior.keys().chain(next.keys()).collect();
    for key in keys {
        if protected.contains(&key.as_str()) || prior.get(key) == next.get(key) {
            continue;
        }
        if live.get(key) != prior.get(key) && live.get(key) != next.get(key) {
            return Err(format!("{kind}的{key}已变化，本次过期结果未写入"));
        }
        match next.get(key) {
            Some(value) => {
                merged.insert(key.clone(), value.clone());
            }
            None => {
                merged.remove(key);
            }
        }
    }
    Ok(Value::Object(merged))
}

fn rebase_result(value: &mut Value, current: &Value) {
    if value["skills"].is_array() && value["sources"].is_array() && value["connections"].is_array()
    {
        for field in [
            "skills",
            "sources",
            "connections",
            "settings",
            "dataDir",
            "storage",
            "managementUndo",
            "stateRevision",
            "readGeneration",
        ] {
            value[field] = current[field].clone();
        }
    }
    for field in ["stateSnapshot", "snapshot"] {
        if value[field]["skills"].is_array() {
            value[field] = current.clone();
        }
    }
}

fn read_task(action: &str) -> bool {
    action.starts_with("preview_")
        || matches!(
            action,
            "refresh"
                | "audit_skills"
                | "refresh_clients"
                | "get_document"
                | "get_doc_asset"
                | "get_files"
                | "get_export_plan"
                | "read_file"
                | "get_skill_info"
                | "scan_local"
                | "get_cleanup_receipts"
                | "get_external_dependencies"
                | "check_external_dependencies"
                | "test_network"
                | "get_network_settings"
                | "storage_get"
                | "storage_stats"
                | "storage_preview"
                | "storage_open_path"
                | "storage_resolve_default_path"
        )
}
pub(crate) fn read_request(request: &Value) -> bool {
    read_task(request["action"].as_str().unwrap_or_default())
        || (request["action"] == "batch_external_dependencies" && request["operation"] == "recheck")
}
fn management_task(action: &str) -> bool {
    matches!(
        action,
        "create_category"
            | "rename_category"
            | "delete_category"
            | "reorder_categories"
            | "set_category_color"
            | "assign_category"
            | "restore_category_assignments"
            | "set_favorite"
            | "undo_management"
    )
}

fn resources_for(manager: &Manager, request: &Value) -> Result<Vec<Resource>, String> {
    let action = text(request, "action")?;
    let preview = request["previewId"]
        .as_str()
        .or_else(|| request["planId"].as_str())
        .and_then(|id| manager.previews.get(id));
    let skill_id = request["skillId"].as_str().or_else(|| {
        preview.and_then(|p| {
            p["public"]["skillId"]
                .as_str()
                .or_else(|| p["skillId"].as_str())
        })
    });
    let write = !read_request(request)
        && !action.starts_with("preview_")
        && !matches!(action, "open_local" | "open_folder" | "export");
    let management = management_task(action);
    let mut resources = BTreeMap::new();
    let mut add = |key: String, exclusive: bool| {
        resources
            .entry(key)
            .and_modify(|v| *v |= exclusive)
            .or_insert(exclusive);
    };
    if management {
        add("management".into(), true);
    }
    if action == "set_settings" {
        add("settings:display".into(), true);
    }
    if action == "set_network_settings" {
        add("settings:network".into(), true);
    }
    if action.starts_with("storage_") {
        add(
            "settings:storage".into(),
            matches!(action, "storage_apply_defaults" | "storage_default_path"),
        );
    }
    if matches!(action, "set_token" | "delete_token") {
        add(
            format!(
                "credential:{}",
                request["repo"].as_str().unwrap_or_default().to_lowercase()
            ),
            true,
        );
    }
    if action == "scan_local" {
        if let Some(path) = request["path"].as_str() {
            add(format!("directory:{}", path_key(path)), false);
        }
    }
    if action == "add_local" {
        if let Some(paths) = request["paths"].as_array() {
            for path in paths.iter().filter_map(Value::as_str) {
                add(format!("directory:{}", path_key(path)), true);
            }
        }
    }
    if matches!(action, "add_github" | "auto_associate") {
        add("sources:discovery".into(), true);
    }
    let selected: Vec<_> = if matches!(action, "refresh" | "audit_skills") {
        let ids = manager.refresh_selection(request)?;
        manager
            .state
            .skills
            .iter()
            .filter(|skill| {
                ids.as_ref()
                    .is_none_or(|set| skill["id"].as_str().is_some_and(|id| set.contains(id)))
            })
            .collect()
    } else if action == "connect_batch" {
        manager
            .state
            .skills
            .iter()
            .filter(|skill| {
                request["skillIds"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| *id == skill["id"]))
            })
            .collect()
    } else {
        skill_id
            .and_then(|id| manager.state.skills.iter().find(|skill| skill["id"] == id))
            .into_iter()
            .collect()
    };
    if !management {
        for skill in &selected {
            add(
                format!("skill:{}", skill["id"].as_str().unwrap_or_default()),
                write,
            );
            if let Some(path) = skill["localPath"].as_str() {
                add(format!("directory:{}", path_key(path)), write);
            }
        }
    }
    if let Some(path) = preview.and_then(|p| p["targetPath"].as_str()) {
        if action.starts_with("apply_") && !matches!(action, "apply_push" | "apply_upload") {
            add(format!("directory:{}", path_key(path)), true);
        }
    }
    if let Some(id) = request["previewId"].as_str() {
        add(format!("preview:{id}"), true);
    }
    if matches!(action, "apply_push" | "apply_upload") {
        let repo = preview
            .and_then(|p| p["repo"].as_str())
            .or_else(|| selected.first().and_then(|s| s["repo"].as_str()))
            .unwrap_or_default();
        let branch = preview
            .and_then(|p| p["branch"].as_str())
            .or_else(|| selected.first().and_then(|s| s["branch"].as_str()))
            .unwrap_or("main");
        add(format!("branch:{}:{branch}", repo.to_lowercase()), true);
    }
    let connection = request["connectionId"]
        .as_str()
        .and_then(|id| manager.state.connections.iter().find(|c| c["id"] == id))
        .or_else(|| {
            preview
                .and_then(|p| p["connectionId"].as_str())
                .and_then(|id| manager.state.connections.iter().find(|c| c["id"] == id))
        });
    if matches!(
        action,
        "connect"
            | "connect_batch"
            | "disconnect"
            | "remove_connection_record"
            | "preview_disconnect"
            | "apply_disconnect"
            | "refresh_clients"
            | "repair_client_connections"
            | "confirm_client_usage"
            | "refresh"
            | "audit_skills"
    ) {
        if let Some(client) = request["client"]
            .as_str()
            .or_else(|| connection.and_then(|c| c["client"].as_str()))
        {
            add(format!("client:{client}"), write);
        } else {
            for client in ["codex", "claude", "cursor"] {
                add(format!("client:{client}"), write);
            }
        }
        if let Some(path) = request["targetPath"].as_str() {
            add(format!("entry:{}", path_key(path)), write);
        }
        if let Some(paths) = request["targetPaths"].as_object() {
            for path in paths.values().filter_map(Value::as_str) {
                add(format!("entry:{}", path_key(path)), write);
            }
        }
        if let Some(path) = connection.and_then(|connection| connection["path"].as_str()) {
            add(format!("entry:{}", path_key(path)), write);
        }
        // Batch previews and automatic repairs may cover several clients and source folders.
        // Lock each actual record, including source content used by a usage confirmation.
        let mut ids = BTreeSet::new();
        if let Some(id) = request["connectionId"].as_str() {
            ids.insert(id);
        }
        if let Some(items) = request["connectionIds"].as_array() {
            ids.extend(items.iter().filter_map(Value::as_str));
        }
        if let Some(entries) = preview.and_then(|value| value["entries"].as_array()) {
            ids.extend(
                entries
                    .iter()
                    .filter_map(|entry| entry["connectionId"].as_str()),
            );
        }
        for record in &manager.state.connections {
            if !ids.contains(record["id"].as_str().unwrap_or_default())
                && !(matches!(action, "refresh" | "audit_skills" | "refresh_clients")
                    || action == "repair_client_connections" && ids.is_empty())
            {
                continue;
            }
            if let Some(client) = record["client"].as_str() {
                add(format!("client:{client}"), write);
            }
            if let Some(path) = record["path"].as_str() {
                add(format!("entry:{}", path_key(path)), write);
            }
            if let Some(id) = record["skillId"].as_str() {
                add(format!("skill:{id}"), write);
                if let Ok(skill) = manager.skill(id) {
                    if let Some(path) = skill["localPath"].as_str() {
                        add(format!("directory:{}", path_key(path)), write);
                    }
                }
            }
        }
    }
    if action == "export" {
        if let Some(path) = request["destination"].as_str() {
            add(format!("destination:{}", path_key(path)), true);
        }
    }
    Ok(resources
        .into_iter()
        .map(|(key, write)| Resource { key, write })
        .collect())
}

fn path_key(path: &str) -> String {
    let path =
        crate::local::canonical(Path::new(path)).unwrap_or_else(|_| Path::new(path).to_path_buf());
    let value = path.to_string_lossy().replace('/', "\\");
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("Missing {key}"))
}
fn clean_origin(origin: Option<&Value>, request: &Value) -> Value {
    let mut value = Map::new();
    for key in [
        "view",
        "skillId",
        "sourceId",
        "clientId",
        "tab",
        "resultKind",
        "paneId",
        "windowLabel",
    ] {
        if let Some(text) = origin.and_then(|o| o[key].as_str()) {
            value.insert(
                key.into(),
                json!(text.chars().take(512).collect::<String>()),
            );
        }
    }
    if !value.contains_key("skillId") {
        if let Some(id) = request["skillId"].as_str() {
            value.insert("skillId".into(), json!(id));
        }
    }
    Value::Object(value)
}
fn redact(text: &str, request: &Value) -> String {
    let mut value = text.to_owned();
    for field in ["token", "proxyUsername", "proxyPassword"] {
        if let Some(secret) = request[field].as_str().filter(|v| !v.is_empty()) {
            value = value.replace(secret, "[凭据已隐藏]");
        }
    }
    value.chars().take(4000).collect()
}
fn dedup_key(request: &Value) -> Option<String> {
    let action = request["action"].as_str()?;
    if action == "repair_client_connections" {
        let mut ids = request["connectionIds"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        ids.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
        ids.dedup();
        return Some(json!([action, ids]).to_string());
    }
    if !matches!(action, "refresh" | "audit_skills" | "refresh_clients") {
        return None;
    }
    let mut ids = request["skillIds"].as_array().cloned().unwrap_or_default();
    if let Some(id) = request["skillId"].as_str() {
        ids.push(json!(id));
    }
    ids.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    ids.dedup();
    Some(json!([action, request["localOnly"] == true, ids]).to_string())
}
fn auto_match_dedup(manager: &Manager, request: &Value) -> Option<String> {
    if request["action"] != "batch_external_dependencies" || request["operation"] != "auto_match" {
        return None;
    }
    let skill = manager.skill(request["skillId"].as_str()?).ok()?;
    let commit = request["remoteCommit"].as_str()?.to_ascii_lowercase();
    if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut proof = Vec::new();
    for item in request["items"].as_array()? {
        proof.push((item["id"].as_str()?, item["expectedFingerprint"].as_str()?));
    }
    proof.sort_unstable();
    let length = proof.len();
    proof.dedup();
    if proof.len() != length {
        return None;
    }
    Some(
        json!([
            "external-auto-match",
            skill["id"],
            skill["sourceId"],
            skill["repo"].as_str().map(str::to_lowercase),
            skill["branch"],
            skill["remotePath"],
            skill["localPath"].as_str().map(path_key),
            commit,
            proof,
            request["includeRemote"] != false,
            skill["externalBindings"]
        ])
        .to_string(),
    )
}
fn partial(value: &Value) -> bool {
    value["transferResult"]["verificationError"]
        .as_str()
        .is_some()
        || value["transferResult"]["cleanup"]["recordError"]
            .as_str()
            .is_some()
        || value["transferResult"]["cleanup"]["retained"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        || value["auditSummary"]["failed"].as_u64().unwrap_or(0) > 0
        || value["auditSummary"]["needsReview"].as_u64().unwrap_or(0) > 0
        || value["failed"].as_u64().unwrap_or(0) > 0
        || ["needsSelection", "missing", "error"]
            .iter()
            .any(|key| value["autoMatchSummary"][key].as_u64().unwrap_or(0) > 0)
}
fn result_progress(value: &Value) -> Value {
    let total = value["auditSummary"]["checked"].as_u64();
    let total = total.or_else(|| {
        value["connectionResults"]
            .as_array()
            .map(|items| items.len() as u64)
    });
    total
        .map(|total| json!({"completed":total,"total":total}))
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::OnceLock, time::Instant};

    #[test]
    fn durable_metadata_events_use_live_revision_and_workspace_uses_same_database() {
        let (dir, service) = fixture();
        let revisions = Arc::new(Mutex::new(Vec::new()));
        let observed = revisions.clone();
        service
            .set_state_events(Arc::new(move |revision| {
                observed.lock().unwrap().push(revision)
            }))
            .unwrap();
        service
            .execute(
                json!({"action":"set_settings","settings":{"uiZoom":125}}),
                fake_dispatch,
            )
            .unwrap();
        let revision = service.snapshot().unwrap()["stateRevision"]
            .as_u64()
            .unwrap();
        assert_eq!(*revisions.lock().unwrap(), vec![revision]);
        let workspace =
            json!({"revision":2,"windows":[{"label":"main","panes":[],"activePaneId":"home"}]});
        service.store_workspace_state(&workspace).unwrap();
        assert_eq!(service.workspace_state().unwrap(), Some(workspace.clone()));
        let db = rusqlite::Connection::open(dir.path().join("manager.sqlite3")).unwrap();
        let stored: String = db
            .query_row(
                "SELECT payload FROM workspace_state WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), workspace);
        assert_eq!(service.snapshot().unwrap()["settings"]["uiZoom"], 125);
    }

    #[test]
    fn task_origin_retains_stable_pane_identity_when_moved_to_another_window() {
        let (_dir, service) = fixture();
        let task = service
            .start(
                json!({"action":"get_document","skillId":"one"}),
                Some("read"),
                Some(&json!({"paneId":"detail-one","windowLabel":"main"})),
                true,
                true,
                fake_dispatch,
            )
            .unwrap();
        let id = task["id"].as_str().unwrap();
        service.wait(id).unwrap();
        service
            .store_workspace_move(
                &json!({"revision":1,"windows":[]}),
                "detail-one",
                "workspace-second",
            )
            .unwrap();
        let moved = service.get(id, false).unwrap();
        assert_eq!(moved["origin"]["paneId"], "detail-one");
        assert_eq!(moved["origin"]["windowLabel"], "workspace-second");
        assert!(
            service
                .manager
                .lock()
                .unwrap()
                .task_history()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == id
                    && entry["origin"]["windowLabel"] == "workspace-second")
        );
    }

    #[test]
    fn pane_move_rolls_back_layout_and_task_ownership_together_on_history_write_failure() {
        let (dir, service) = fixture();
        let task = service
            .start(
                json!({"action":"get_document","skillId":"one"}),
                None,
                Some(&json!({"paneId":"detail-one","windowLabel":"main"})),
                true,
                true,
                fake_dispatch,
            )
            .unwrap();
        let id = task["id"].as_str().unwrap();
        service.wait(id).unwrap();
        let before = json!({"revision":1,"windows":[{"label":"main"}]});
        service.store_workspace_state(&before).unwrap();
        let db = rusqlite::Connection::open(dir.path().join("manager.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER stop_history BEFORE INSERT ON task_history BEGIN SELECT RAISE(ABORT,'synthetic history failure'); END").unwrap();
        assert!(service
            .store_workspace_move(
                &json!({"revision":2,"windows":[{"label":"workspace-second"}]}),
                "detail-one",
                "workspace-second"
            )
            .is_err());
        assert_eq!(service.workspace_state().unwrap(), Some(before));
        assert_eq!(
            service.get(id, false).unwrap()["origin"]["windowLabel"],
            "main"
        );
        let stored: String = db
            .query_row(
                "SELECT payload FROM task_history WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&stored).unwrap()["origin"]["windowLabel"],
            "main"
        );
    }
    #[test]
    fn history_clear_broadcasts_exact_ids_only_after_durable_success_and_restores_on_failure() {
        let (dir, service) = fixture();
        let id = start(&service, json!({"action":"get_files","skillId":"one"}));
        service.wait(&id).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let listener = events.clone();
        service.set_events(Arc::new(move |event| listener.lock().unwrap().push(event)));
        let db = rusqlite::Connection::open(dir.path().join("manager.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER prevent_history_clear BEFORE DELETE ON task_history BEGIN SELECT RAISE(ABORT,'synthetic failure'); END").unwrap();
        assert!(service.clear_history().is_err());
        assert_eq!(service.list().unwrap().len(), 1);
        assert!(events.lock().unwrap().is_empty());
        db.execute_batch("DROP TRIGGER prevent_history_clear")
            .unwrap();
        assert_eq!(service.clear_history().unwrap(), vec![id.clone()]);
        let event = events.lock().unwrap()[0].clone();
        assert_eq!(event["clearedTaskIds"], json!([id]));
        assert_eq!(event["tasks"], json!([]));
        assert_eq!(service.get(&id, true).unwrap()["hidden"], true);
        assert!(event.get("result").is_none());
    }

    #[test]
    fn external_batch_rechecks_are_reads_and_binding_conflicts_with_export_scope() {
        let (_dir, service) = fixture();
        let manager = service.manager.lock().unwrap();
        let check =
            json!({"action":"batch_external_dependencies","operation":"recheck","skillId":"one"});
        let bind =
            json!({"action":"batch_external_dependencies","operation":"bind","skillId":"one"});
        assert!(read_request(&check));
        assert!(!read_request(&bind));
        let reads = resources_for(&manager, &check).unwrap();
        let writes = resources_for(&manager, &bind).unwrap();
        let export = resources_for(
            &manager,
            &json!({"action":"export","skillId":"one","destination":"synthetic.zip"}),
        )
        .unwrap();
        assert!(conflicts(&writes, &export));
        assert!(!conflicts(&reads, &export));
    }
    #[test]
    fn automatic_matching_deduplicates_identical_proof_across_callers_but_not_new_proof() {
        let (_dir, service) = fixture();
        {
            let mut manager = service.manager.lock().unwrap();
            let skill = &mut manager.state.skills[0];
            skill["sourceId"] = json!("source");
            skill["repo"] = json!("synthetic/repo");
            skill["branch"] = json!("main");
            skill["remotePath"] = json!("one");
        }
        let (hold_id, latch) = new_latch();
        let request = json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"one","remoteCommit":"a".repeat(40),"items":[{"id":"b","expectedFingerprint":"second"},{"id":"a","expectedFingerprint":"first"}],"hold":hold_id});
        let first = service
            .start(
                request.clone(),
                Some("background"),
                None,
                false,
                true,
                fake_dispatch,
            )
            .unwrap();
        entered(&latch);
        let mut reordered = request.clone();
        reordered["items"].as_array_mut().unwrap().reverse();
        let joined = service
            .start(
                reordered,
                Some("opened detail"),
                Some(&json!({"paneId":"detail-one","windowLabel":"main"})),
                true,
                true,
                fake_dispatch,
            )
            .unwrap();
        assert_eq!(first["id"], joined["id"]);
        let mut other_commit = request.clone();
        other_commit["remoteCommit"] = json!("b".repeat(40));
        let distinct = service
            .start(other_commit, None, None, true, true, fake_dispatch)
            .unwrap();
        assert_ne!(first["id"], distinct["id"]);
        let mut other_fingerprint = request.clone();
        other_fingerprint["items"][0]["expectedFingerprint"] = json!("changed");
        let distinct_fingerprint = service
            .start(other_fingerprint, None, None, true, true, fake_dispatch)
            .unwrap();
        assert_ne!(first["id"], distinct_fingerprint["id"]);
        {
            service.manager.lock().unwrap().state.skills[0]["remotePath"] =
                json!("other-association");
        }
        let association = service
            .start(request, None, None, true, true, fake_dispatch)
            .unwrap();
        assert_ne!(first["id"], association["id"]);
        assert_eq!(service.jobs.lock().unwrap().len(), 4);
        release(&latch);
        for task in [first, distinct, distinct_fingerprint, association] {
            let _ = service.wait(task["id"].as_str().unwrap());
        }
    }
    #[test]
    fn scheduler_waits_do_not_release_recorded_or_hidden_callers_compatibility_consumers() {
        for visible in [true, false] {
            let (_dir, service) = fixture();
            let (id, latch) = new_latch();
            let task = service
                .start(
                    json!({"action":"audit_skills","hold":id}),
                    None,
                    None,
                    false,
                    visible,
                    fake_dispatch,
                )
                .unwrap();
            entered(&latch);
            let task_id = task["id"].as_str().unwrap().to_owned();
            let scheduler = service.match_external_background(Some(task_id.clone()));
            // Dense consumers can still register independent reads while the
            // application scheduler waits for an existing audit.
            let other = service
                .start(
                    json!({"action":"get_files","skillId":"two"}),
                    None,
                    None,
                    true,
                    true,
                    fake_dispatch,
                )
                .unwrap();
            service.wait(other["id"].as_str().unwrap()).unwrap();
            release(&latch);
            scheduler.join().unwrap();
            assert_eq!(
                service
                    .jobs
                    .lock()
                    .unwrap()
                    .get(&task_id)
                    .unwrap()
                    .consumers,
                usize::from(!visible)
            );
            assert!(service.get(&task_id, true).is_ok());
            assert!(service.snapshot().is_ok());
            if !visible {
                service.release_compat(&task_id);
                assert!(!service.jobs.lock().unwrap().contains_key(&task_id));
            }
        }
    }
    #[test]
    fn automatic_match_attention_and_errors_are_partial_with_four_counts() {
        fn answer(_: &mut Manager, request: &Value) -> Result<Value, String> {
            Ok(request["answer"].clone())
        }
        let (_dir, service) = fixture();
        for field in ["needsSelection", "missing", "error"] {
            let mut summary = json!({"matched":1,"needsSelection":0,"missing":0,"error":0});
            summary[field] = json!(1);
            let task=service.start(json!({"action":"batch_external_dependencies","operation":"auto_match","skillId":"one","answer":{"autoMatchSummary":summary}}),None,None,true,true,answer).unwrap();
            let id = task["id"].as_str().unwrap();
            service.wait(id).unwrap();
            let task = service.get(id, true).unwrap();
            assert_eq!(task["status"], "partial");
            let stage = task["stage"].as_str().unwrap();
            for label in ["已匹配", "待选择", "缺失", "异常"] {
                assert!(stage.contains(label));
            }
        }
    }

    #[derive(Default)]
    struct LatchState {
        entered: bool,
        released: bool,
    }
    type Latch = Arc<(Mutex<LatchState>, Condvar)>;
    fn latches() -> &'static Mutex<HashMap<String, Latch>> {
        static LATCHES: OnceLock<Mutex<HashMap<String, Latch>>> = OnceLock::new();
        LATCHES.get_or_init(|| Mutex::new(HashMap::new()))
    }
    fn new_latch() -> (String, Latch) {
        let id = uuid::Uuid::new_v4().to_string();
        let latch = Arc::new((Mutex::new(LatchState::default()), Condvar::new()));
        latches().lock().unwrap().insert(id.clone(), latch.clone());
        (id, latch)
    }
    fn entered(latch: &Latch) {
        let limit = Instant::now() + Duration::from_secs(5);
        let mut state = latch.0.lock().unwrap();
        while !state.entered {
            assert!(Instant::now() < limit, "synthetic worker did not enter");
            state = latch
                .1
                .wait_timeout(state, Duration::from_millis(20))
                .unwrap()
                .0;
        }
    }
    fn release(latch: &Latch) {
        latch.0.lock().unwrap().released = true;
        latch.1.notify_all();
    }
    fn hold(id: &str) -> Result<(), String> {
        let latch = latches().lock().unwrap().get(id).unwrap().clone();
        let mut state = latch.0.lock().unwrap();
        state.entered = true;
        latch.1.notify_all();
        while !state.released {
            crate::network::check_cancelled()?;
            state = latch
                .1
                .wait_timeout(state, Duration::from_millis(20))
                .unwrap()
                .0;
        }
        Ok(())
    }
    fn fake_dispatch(manager: &mut Manager, request: &Value) -> Result<Value, String> {
        if management_task(request["action"].as_str().unwrap_or_default()) {
            return crate::categories::handle(manager, request)?
                .ok_or("Unknown management action".into());
        }
        if request["action"] == "set_settings" {
            return crate::dispatch(manager, request);
        }
        if let Some(id) = request["hold"].as_str() {
            hold(id)?;
        }
        if let Some(fields) = request["fields"].as_object() {
            let mut skill = manager.skill(text(request, "skillId")?)?;
            for (field, value) in fields {
                skill[field] = value.clone();
            }
            manager.upsert_skill(skill);
            manager.save()?;
        }
        if let Some(id) = request["preview"].as_str() {
            manager.previews.clear();
            manager.previews.insert(id.into(),json!({"public":{"id":id,"skillId":request["skillId"],"kind":"update","canApply":true},"repo":"synthetic/repo","branch":"main"}));
            return Ok(
                json!({"id":id,"kind":"update","skillId":request["skillId"],"canApply":true}),
            );
        }
        if let Some(id) = request["previewId"].as_str() {
            manager
                .previews
                .remove(id)
                .ok_or("Preview already consumed")?;
            if request["fail"] == true {
                return Err("Synthetic apply failed".into());
            }
        }
        if let Some(id) = request["afterSave"].as_str() {
            hold(id)?;
        }
        if request["fail"] == true {
            return Err(request["error"]
                .as_str()
                .unwrap_or("Synthetic failure")
                .into());
        }
        report_progress("已读取测试数据", Some(1), Some(1));
        Ok(manager.snapshot())
    }
    fn fixture() -> (tempfile::TempDir, Arc<TaskSystem>) {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/tasks-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let dir = tempfile::tempdir_in(parent).unwrap();
        let mut manager = Manager::new(dir.path().into()).unwrap();
        manager.state.skills = vec![
            json!({"id":"one","name":"one","repo":"synthetic/repo","branch":"main","favorite":false,"runtimeFiles":["SKILL.md"],"status":"local_only"}),
            json!({"id":"two","name":"two","repo":"synthetic/repo","branch":"main","favorite":false,"runtimeFiles":["SKILL.md"]}),
            json!({"id":"three","name":"three","repo":"synthetic/repo","branch":"other","runtimeFiles":["SKILL.md"]}),
        ];
        manager.save().unwrap();
        (dir, TaskSystem::new(manager).unwrap())
    }
    fn start(service: &Arc<TaskSystem>, request: Value) -> String {
        service
            .start(
                request,
                Some("测试操作"),
                Some(&json!({"view":"skills","tab":"files"})),
                true,
                true,
                fake_dispatch,
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .into()
    }
    fn task_status(service: &TaskSystem, id: &str, status: &str) {
        let limit = Instant::now() + Duration::from_secs(5);
        loop {
            if service.get(id, false).unwrap()["status"] == status {
                return;
            }
            assert!(
                Instant::now() < limit,
                "expected task status {status}: {:?}",
                service.get(id, true)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn independent_reads_and_management_do_not_wait_for_long_io_or_overwrite_each_other() {
        let (_dir, service) = fixture();
        let (hold_id, latch) = new_latch();
        let a = start(
            &service,
            json!({"action":"audit_skills","skillId":"one","hold":hold_id,"fields":{"status":"synced"}}),
        );
        entered(&latch);
        let began = Instant::now();
        let b = start(&service, json!({"action":"read_file","skillId":"two"}));
        service.wait(&b).unwrap();
        let favorite = start(
            &service,
            json!({"action":"set_favorite","skillId":"one","favorite":true}),
        );
        service.wait(&favorite).unwrap();
        let settings = start(
            &service,
            json!({"action":"set_settings","settings":{"uiZoom":125}}),
        );
        service.wait(&settings).unwrap();
        assert!(began.elapsed() < Duration::from_secs(2));
        let state = service.snapshot().unwrap();
        assert_eq!(state["skills"][0]["favorite"], true);
        assert_eq!(state["settings"]["uiZoom"], 125);
        release(&latch);
        let final_state = service.wait(&a).unwrap();
        assert_eq!(final_state["skills"][0]["favorite"], true);
        assert_eq!(final_state["skills"][0]["status"], "synced");
        assert_eq!(final_state["settings"]["uiZoom"], 125);
        assert_eq!(final_state["managementUndo"]["available"], true);
    }

    #[test]
    fn one_skill_writes_queue_while_an_independent_skill_finishes() {
        let (_dir, service) = fixture();
        let (hold_id, latch) = new_latch();
        let a = start(
            &service,
            json!({"action":"apply_update","skillId":"one","hold":hold_id,"fields":{"version":"a"}}),
        );
        entered(&latch);
        let b = start(
            &service,
            json!({"action":"apply_update","skillId":"one","fields":{"version":"b"}}),
        );
        let c = start(
            &service,
            json!({"action":"apply_update","skillId":"two","fields":{"version":"c"}}),
        );
        service.wait(&c).unwrap();
        assert_eq!(service.get(&b, false).unwrap()["status"], "queued");
        assert!(service.cancel(&a).is_err());
        release(&latch);
        service.wait(&a).unwrap();
        service.wait(&b).unwrap();
        let state = service.snapshot().unwrap();
        assert_eq!(state["skills"][0]["version"], "b");
        assert_eq!(state["skills"][1]["version"], "c");
    }

    #[test]
    fn same_repository_branch_uploads_queue_but_other_branch_does_not() {
        let (_dir, service) = fixture();
        let (hold_id, latch) = new_latch();
        let a = start(
            &service,
            json!({"action":"apply_push","skillId":"one","hold":hold_id}),
        );
        entered(&latch);
        let b = start(&service, json!({"action":"apply_push","skillId":"two"}));
        let c = start(&service, json!({"action":"apply_push","skillId":"three"}));
        service.wait(&c).unwrap();
        assert_eq!(service.get(&b, false).unwrap()["status"], "queued");
        release(&latch);
        service.wait(&a).unwrap();
        service.wait(&b).unwrap();
    }

    #[test]
    fn cancelled_queued_write_never_dispatches_and_running_read_stops_cooperatively() {
        let (_dir, service) = fixture();
        let (hold_id, latch) = new_latch();
        let a = start(
            &service,
            json!({"action":"apply_update","skillId":"one","hold":hold_id}),
        );
        entered(&latch);
        let b = start(
            &service,
            json!({"action":"apply_update","skillId":"one","fields":{"version":"must-not-write"}}),
        );
        service.cancel(&b).unwrap();
        assert!(service.wait(&b).is_err());
        task_status(&service, &b, "cancelled");
        release(&latch);
        service.wait(&a).unwrap();
        assert!(service.snapshot().unwrap()["skills"][0]["version"].is_null());
        let (read_hold, read_latch) = new_latch();
        let read = start(
            &service,
            json!({"action":"read_file","skillId":"two","hold":read_hold}),
        );
        entered(&read_latch);
        service.cancel(&read).unwrap();
        assert!(service.wait(&read).is_err());
        task_status(&service, &read, "cancelled");
    }

    #[test]
    fn save_acknowledgement_is_already_durable_before_worker_continues() {
        let (dir, service) = fixture();
        let (hold_id, latch) = new_latch();
        let task = start(
            &service,
            json!({"action":"apply_update","skillId":"one","fields":{"version":"durable"},"afterSave":hold_id}),
        );
        entered(&latch);
        let independently_read = Manager::new(dir.path().into()).unwrap();
        assert_eq!(independently_read.state.skills[0]["version"], "durable");
        assert!(
            independently_read.snapshot()["stateRevision"]
                .as_u64()
                .unwrap()
                > 1
        );
        release(&latch);
        service.wait(&task).unwrap();
    }

    #[test]
    fn conflicting_binding_or_runtime_scope_rejects_stale_derived_results() {
        let (_dir, service) = fixture();
        let (hold_id, latch) = new_latch();
        let task = start(
            &service,
            json!({"action":"audit_skills","skillId":"one","hold":hold_id,"fields":{"status":"synced"}}),
        );
        entered(&latch);
        {
            let mut manager = service.manager.lock().unwrap();
            manager.state.skills[0]["externalBindings"] = json!({"new":"binding"});
            manager.save().unwrap();
        }
        release(&latch);
        assert!(service
            .wait(&task)
            .unwrap_err()
            .contains("externalBindings"));
        let state = service.snapshot().unwrap();
        assert_eq!(state["skills"][0]["status"], "local_only");
        assert_eq!(state["skills"][0]["externalBindings"]["new"], "binding");
    }

    #[test]
    fn concurrent_previews_are_retained_and_consumption_is_single_use_even_on_failure() {
        let (_dir, service) = fixture();
        let a = start(
            &service,
            json!({"action":"preview_update","skillId":"one","preview":"preview-one"}),
        );
        service.wait(&a).unwrap();
        let b = start(
            &service,
            json!({"action":"preview_update","skillId":"two","preview":"preview-two"}),
        );
        service.wait(&b).unwrap();
        assert_eq!(service.manager.lock().unwrap().previews.len(), 2);
        let apply = start(
            &service,
            json!({"action":"apply_update","previewId":"preview-one","fail":true}),
        );
        assert!(service.wait(&apply).is_err());
        let manager = service.manager.lock().unwrap();
        assert!(!manager.previews.contains_key("preview-one"));
        assert!(manager.previews.contains_key("preview-two"));
    }

    #[test]
    fn color_management_uses_existing_serialization_and_survives_concurrent_read_completion() {
        let (_dir, service) = fixture();
        {
            let mut manager = service.manager.lock().unwrap();
            manager.state.settings["categories"] =
                json!([{"id":"legacy","name":"Legacy","future":{"keep":true}}]);
            manager.save().unwrap();
        }
        let (hold_id, latch) = new_latch();
        let read = start(
            &service,
            json!({"action":"audit_skills","skillId":"one","hold":hold_id,"fields":{"status":"synced"}}),
        );
        entered(&latch);
        let color = start(
            &service,
            json!({"action":"set_category_color","categoryId":"legacy","color":"purple"}),
        );
        service.wait(&color).unwrap();
        let favorite = start(
            &service,
            json!({"action":"set_favorite","skillId":"one","favorite":true}),
        );
        service.wait(&favorite).unwrap();
        release(&latch);
        let state = service.wait(&read).unwrap();
        assert_eq!(state["settings"]["categories"][0]["color"], "purple");
        assert_eq!(
            state["settings"]["categories"][0]["future"],
            json!({"keep":true})
        );
        assert_eq!(state["skills"][0]["favorite"], true);
        assert_eq!(state["skills"][0]["status"], "synced");
        let undo = start(&service, json!({"action":"undo_management"}));
        service.wait(&undo).unwrap();
        let undo = start(&service, json!({"action":"undo_management"}));
        let state = service.wait(&undo).unwrap();
        assert_eq!(
            state["settings"]["categories"],
            json!([{"id":"legacy","name":"Legacy","future":{"keep":true}}])
        );
        assert_eq!(state["skills"][0]["status"], "synced");
        assert!(read_request(
            &json!({"action":"storage_resolve_default_path","kind":"export"})
        ));
        let manager = service.manager.lock().unwrap();
        assert!(resources_for(
            &manager,
            &json!({"action":"storage_resolve_default_path","kind":"export"})
        )
        .unwrap()
        .iter()
        .all(|resource| !resource.write));
    }

    #[test]
    fn management_undo_remains_consistent_after_task_completion() {
        let (_dir, service) = fixture();
        let a = start(
            &service,
            json!({"action":"set_favorite","skillId":"one","favorite":true}),
        );
        let state = service.wait(&a).unwrap();
        assert_eq!(state["managementUndo"]["available"], true);
        let b = start(&service, json!({"action":"undo_management"}));
        let state = service.wait(&b).unwrap();
        assert_eq!(state["skills"][0]["favorite"], false);
        assert_eq!(state["managementUndo"]["available"], false);
    }

    #[test]
    fn repeated_refreshes_join_one_task_and_events_do_not_expose_full_results() {
        let (_dir, service) = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        service.set_events(Arc::new(move |event| captured.lock().unwrap().push(event)));
        let (hold_id, latch) = new_latch();
        let a = start(
            &service,
            json!({"action":"refresh","skillId":"one","localOnly":true,"hold":hold_id}),
        );
        entered(&latch);
        let b = start(
            &service,
            json!({"action":"refresh","skillIds":["one"],"localOnly":true}),
        );
        assert_eq!(a, b);
        release(&latch);
        service.wait(&a).unwrap();
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .all(|event| event["task"].get("result").is_none()));
        assert!(service.get(&a, true).unwrap()["result"]["skills"].is_array());
        assert_eq!(service.list().unwrap().len(), 1);
    }

    #[test]
    fn history_is_bounded_durable_redacted_and_never_replays_expired_results() {
        let (dir, service) = fixture();
        for _ in 0..102 {
            let task = start(&service, json!({"action":"read_file","skillId":"one"}));
            service.wait(&task).unwrap();
        }
        let secret = "synthetic-private-token";
        let task=service.start(json!({"action":"set_token","repo":"synthetic/repo","token":secret,"fail":true,"error":secret}),Some(secret),None,true,true,fake_dispatch).unwrap()["id"].as_str().unwrap().to_owned();
        assert!(service.wait(&task).is_err());
        let saved = service.manager.lock().unwrap().task_history().unwrap();
        assert_eq!(saved.len(), 100);
        assert!(!json!(saved).to_string().contains(secret));
        let restarted = TaskSystem::new(Manager::new(dir.path().into()).unwrap()).unwrap();
        assert_eq!(restarted.list().unwrap().len(), 100);
        assert!(restarted
            .list()
            .unwrap()
            .iter()
            .all(|task| task["resultExpired"] == true && task["resultAvailable"] == false));
        restarted.clear_history().unwrap();
        assert!(restarted.list().unwrap().is_empty());
    }

    #[test]
    fn unfinished_history_becomes_interrupted_on_restart_without_running_business_code() {
        let (dir, service) = fixture();
        let task = json!({"id":"old","action":"apply_push","label":"旧上传","status":"running","stage":"提交中","createdAt":now(),"origin":{"view":"skills","skillId":"one"}});
        service
            .manager
            .lock()
            .unwrap()
            .store_task_history(&[task])
            .unwrap();
        let restarted = TaskSystem::new(Manager::new(dir.path().into()).unwrap()).unwrap();
        let record = restarted.get("old", true).unwrap();
        assert_eq!(record["status"], "interrupted");
        assert_eq!(record["resultExpired"], true);
        assert!(record.get("result").is_none());
        assert_eq!(
            restarted.snapshot().unwrap()["skills"][0]["status"],
            "local_only"
        );
    }

    #[test]
    fn prepare_close_cancels_reads_but_waits_for_critical_write_and_can_be_reversed() {
        let (_dir, service) = fixture();
        let (write_hold, write_latch) = new_latch();
        let write = start(
            &service,
            json!({"action":"apply_update","skillId":"one","hold":write_hold,"fields":{"version":"committed"}}),
        );
        entered(&write_latch);
        let (read_hold, read_latch) = new_latch();
        let read = start(
            &service,
            json!({"action":"read_file","skillId":"two","hold":read_hold}),
        );
        entered(&read_latch);
        let close_service = service.clone();
        let close = std::thread::spawn(move || close_service.prepare_close());
        task_status(&service, &read, "cancelled");
        assert!(!close.is_finished());
        assert!(service
            .start(
                json!({"action":"scan_local"}),
                None,
                None,
                true,
                true,
                fake_dispatch
            )
            .is_err());
        release(&write_latch);
        service.wait(&write).unwrap();
        assert_eq!(close.join().unwrap().unwrap()["ready"], true);
        service
            .control(&json!({"action":"cancel_close"}), fake_dispatch)
            .unwrap();
        let fresh = start(&service, json!({"action":"read_file","skillId":"one"}));
        service.wait(&fresh).unwrap();
    }

    #[test]
    fn get_state_is_pure_and_does_not_discover_or_save_connections() {
        let (_dir, service) = fixture();
        let before = service.snapshot().unwrap();
        let state = service
            .execute(json!({"action":"get_state"}), crate::dispatch)
            .unwrap();
        assert_eq!(state, before);
        assert!(service.list().unwrap().is_empty());
    }

    #[test]
    fn merge_conflicts_are_atomic_and_preserve_unrelated_fields() {
        let (_dir, service) = fixture();
        let before = service.manager.lock().unwrap().state.clone();
        let mut after = before.clone();
        after.skills[0]["status"] = json!("synced");
        after.settings["uiZoom"] = json!(125);
        let mut current = before.clone();
        current.skills[0]["status"] = json!("both_modified");
        current.settings["uiFontSize"] = json!(18);
        assert!(merge_state(&before, &after, &current, "refresh").is_err());
        assert_eq!(current.settings["uiZoom"], 100);
        current.skills[0]["status"] = before.skills[0]["status"].clone();
        let merged = merge_state(&before, &after, &current, "refresh").unwrap();
        assert_eq!(merged.settings["uiZoom"], 125);
        assert_eq!(merged.settings["uiFontSize"], 18);
    }

    #[test]
    fn canonical_directory_aliases_serialize_even_with_different_skill_ids() {
        let (dir, service) = fixture();
        let root = dir.path().join("skill");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("SKILL.md"),
            "---\nname: synthetic\ndescription: test\n---\n",
        )
        .unwrap();
        {
            let mut manager = service.manager.lock().unwrap();
            manager.state.skills[0]["localPath"] = json!(root.to_string_lossy());
            manager.state.skills[1]["localPath"] = json!(root.join(".").to_string_lossy());
            manager.save().unwrap();
        }
        let (hold_id, latch) = new_latch();
        let a = start(
            &service,
            json!({"action":"apply_update","skillId":"one","hold":hold_id}),
        );
        entered(&latch);
        let b = start(
            &service,
            json!({"action":"apply_update","skillId":"two","fields":{"version":"b"}}),
        );
        assert_eq!(service.get(&b, false).unwrap()["status"], "queued");
        release(&latch);
        service.wait(&a).unwrap();
        service.wait(&b).unwrap();
    }

    #[test]
    fn client_repair_and_usage_take_source_locks_while_checks_remain_reads() {
        let (_dir, service) = fixture();
        let mut manager = service.manager.lock().unwrap();
        manager.state.skills[0]["localPath"] = json!("D:/synthetic/skill-one");
        manager.state.skills[1]["localPath"] = json!("D:/synthetic/skill-two");
        manager.state.connections = vec![
            json!({"id":"entry-one","client":"codex","skillId":"one","path":"D:/synthetic/codex/one"}),
            json!({"id":"entry-two","client":"cursor","skillId":"two","path":"D:/synthetic/cursor/two"}),
        ];
        for action in ["repair_client_connections", "confirm_client_usage"] {
            let resources = resources_for(
                &manager,
                &json!({"action":action,"connectionId":"entry-one","connectionIds":["entry-one"]}),
            )
            .unwrap();
            assert!(!read_request(&json!({"action":action})));
            for prefix in ["client:codex", "skill:one", "directory:", "entry:"] {
                assert!(
                    resources
                        .iter()
                        .any(|resource| resource.key.starts_with(prefix) && resource.write),
                    "{action}: {prefix}"
                );
            }
        }
        for action in ["refresh", "audit_skills", "refresh_clients"] {
            let resources =
                resources_for(&manager, &json!({"action":action,"skillId":"one"})).unwrap();
            for name in ["client:codex", "client:cursor", "skill:one", "skill:two"] {
                assert!(
                    resources
                        .iter()
                        .any(|resource| resource.key == name && !resource.write),
                    "{action}: {name}"
                );
            }
            assert!(resources
                .iter()
                .filter(|resource| resource.key.starts_with("directory:"))
                .all(|resource| !resource.write));
        }
        manager.previews.insert(
            "client-preview".into(),
            json!({"entries":[{"connectionId":"entry-one"},{"connectionId":"entry-two"}]}),
        );
        let resources = resources_for(
            &manager,
            &json!({"action":"apply_disconnect","previewId":"client-preview"}),
        )
        .unwrap();
        for name in [
            "client:codex",
            "client:cursor",
            "skill:one",
            "skill:two",
            "preview:client-preview",
        ] {
            assert!(
                resources
                    .iter()
                    .any(|resource| resource.key == name && resource.write),
                "batch preview: {name}"
            );
        }
    }

    #[test]
    fn identical_automatic_repair_requests_coalesce_and_distinct_targets_do_not() {
        let first =
            dedup_key(&json!({"action":"repair_client_connections","connectionIds":["b","a","a"]}));
        assert_eq!(
            first,
            dedup_key(&json!({"action":"repair_client_connections","connectionIds":["a","b"]}))
        );
        assert_ne!(
            first,
            dedup_key(&json!({"action":"repair_client_connections","connectionIds":["a"]}))
        );
    }

    #[test]
    fn same_client_batch_and_remove_record_queue_without_blocking_another_client() {
        let (_dir, service) = fixture();
        {
            let mut manager = service.manager.lock().unwrap();
            manager.state.connections = vec![
                json!({"id":"entry","client":"codex","skillId":"two","path":"D:/synthetic/client"}),
            ];
            manager.save().unwrap();
        }
        let (hold_id, latch) = new_latch();
        let a = start(
            &service,
            json!({"action":"connect_batch","client":"codex","skillIds":["one"],"hold":hold_id}),
        );
        entered(&latch);
        let b = start(
            &service,
            json!({"action":"remove_connection_record","connectionId":"entry"}),
        );
        let c = start(
            &service,
            json!({"action":"connect","client":"claude","skillId":"three"}),
        );
        service.wait(&c).unwrap();
        assert_eq!(service.get(&b, false).unwrap()["status"], "queued");
        release(&latch);
        service.wait(&a).unwrap();
        service.wait(&b).unwrap();
    }

    #[test]
    fn a_failed_durable_save_does_not_acknowledge_or_modify_live_state() {
        let (dir, service) = fixture();
        let before = service.snapshot().unwrap();
        let db = rusqlite::Connection::open(dir.path().join("manager.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER deny_test_save BEFORE UPDATE ON app_state BEGIN SELECT RAISE(ABORT,'synthetic disk failure'); END").unwrap();
        let a = start(
            &service,
            json!({"action":"apply_update","skillId":"one","fields":{"version":"must-not-commit"}}),
        );
        assert!(service
            .wait(&a)
            .unwrap_err()
            .contains("synthetic disk failure"));
        assert_eq!(service.snapshot().unwrap(), before);
        let persisted = Manager::new(dir.path().into()).unwrap();
        assert!(persisted.state.skills[0]["version"].is_null());
    }

    #[test]
    fn external_file_change_rejects_stale_hash_observations_without_holding_live_lock() {
        let (dir, service) = fixture();
        let root = dir.path().join("observed");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("SKILL.md"), "original bytes").unwrap();
        let digest = crate::local::files(&root).unwrap()["SKILL.md"].clone();
        {
            let mut manager = service.manager.lock().unwrap();
            manager.state.skills[0]["localPath"] = json!(root.to_string_lossy());
            manager.save().unwrap();
        }
        let (hold_id, latch) = new_latch();
        let a = start(
            &service,
            json!({"action":"refresh","skillId":"one","hold":hold_id,"fields":{"localHashes":{"SKILL.md":digest},"lastLocalCheckedAt":now(),"status":"synced"}}),
        );
        entered(&latch);
        std::fs::write(root.join("SKILL.md"), "edited in existing editor").unwrap();
        release(&latch);
        assert!(service.wait(&a).unwrap_err().contains("本地文件"));
        assert_eq!(
            service.snapshot().unwrap()["skills"][0]["status"],
            "local_only"
        );
    }

    #[test]
    fn independent_client_ignore_changes_merge_as_set_differences() {
        let (_dir, service) = fixture();
        let before = service.manager.lock().unwrap().state.clone();
        let mut after = before.clone();
        after.settings["ignoredClientConnections"] =
            json!([{"client":"codex","path":"a","target":"one"}]);
        let mut current = before.clone();
        current.settings["ignoredClientConnections"] =
            json!([{"client":"claude","path":"b","target":"two"}]);
        let merged = merge_state(&before, &after, &current, "remove_connection_record").unwrap();
        assert_eq!(
            merged.settings["ignoredClientConnections"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn inventory_read_merges_reject_added_files_and_changed_evidence_with_the_same_timestamp() {
        for action in ["get_files", "get_export_plan"] {
            let (dir, service) = fixture();
            let root = dir.path().join("observed");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("SKILL.md"), "original bytes").unwrap();
            let original = crate::local::files(&root).unwrap();
            let mut before = service.manager.lock().unwrap().state.clone();
            before.skills[0]["localPath"] = json!(root.to_string_lossy());
            before.skills[0]["runtimeAudit"] =
                json!({"checkedAt":"same-timestamp","scopeHashes":original});
            let mut after = before.clone();
            after.skills[0]["runtimeAudit"]["inventoryHashes"] = json!(original);
            assert!(validate_observations(&before, &after, action).is_ok());
            std::fs::write(root.join("new-file.md"), "new inventory member").unwrap();
            assert!(validate_observations(&before, &after, action)
                .unwrap_err()
                .contains("本地文件"));
            let current = crate::local::files(&root).unwrap();
            after.skills[0]["runtimeAudit"]["inventoryHashes"] = json!(current);
            assert!(validate_observations(&before, &after, action).is_ok());
            std::fs::write(root.join("SKILL.md"), "changed required bytes").unwrap();
            assert!(validate_observations(&before, &after, action).is_err());
        }
    }

    #[test]
    fn concurrent_real_inventory_and_export_plan_reads_share_equivalent_audit_evidence() {
        static BARRIER: std::sync::OnceLock<std::sync::Barrier> = std::sync::OnceLock::new();
        fn synchronized_dispatch(manager: &mut Manager, request: &Value) -> Result<Value, String> {
            BARRIER.get().unwrap().wait();
            crate::dispatch(manager, request)
        }
        let (dir, service) = fixture();
        let root = dir.path().join("concurrent-inventory");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("SKILL.md"),
            "---\nname: concurrent-fixture\n---\nUse this skill.\n",
        )
        .unwrap();
        {
            let mut manager = service.manager.lock().unwrap();
            let mut skill = crate::local::draft(&root).unwrap();
            skill["id"] = json!("one");
            manager.upsert_skill(skill);
            manager.save().unwrap();
        }
        BARRIER.get_or_init(|| std::sync::Barrier::new(2));
        for added in [false, true] {
            if added {
                std::fs::write(root.join("optional-native.md"), "New optional file.").unwrap();
            }
            let before = service.snapshot().unwrap();
            let files_service = service.clone();
            let files = std::thread::spawn(move || {
                files_service.execute(
                    json!({"action":"get_files","skillId":"one"}),
                    synchronized_dispatch,
                )
            });
            let plan_service = service.clone();
            let plan = std::thread::spawn(move || {
                plan_service.execute(
                    json!({"action":"get_export_plan","skillId":"one","selectedIds":[]}),
                    synchronized_dispatch,
                )
            });
            let files = files.join().unwrap().unwrap();
            let plan = plan.join().unwrap().unwrap();
            assert_eq!(
                files["files"].as_array().unwrap().len(),
                if added { 2 } else { 1 }
            );
            assert_eq!(plan["canExport"], true);
            let after = service.snapshot().unwrap();
            assert_eq!(
                after["stateRevision"].as_u64().unwrap(),
                before["stateRevision"].as_u64().unwrap() + 1,
                "Equivalent concurrent evidence is persisted once"
            );
            let record = after["skills"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == "one")
                .unwrap();
            assert_eq!(
                record["runtimeAudit"]["inventoryHashes"],
                json!(crate::local::files(&root).unwrap())
            );
            assert!(record["baselineCommit"].is_null());
            assert!(record["installedCommit"].is_null());
            service
                .execute(
                    json!({"action":"get_files","skillId":"one"}),
                    crate::dispatch,
                )
                .unwrap();
            service
                .execute(
                    json!({"action":"get_export_plan","skillId":"one","selectedIds":[]}),
                    crate::dispatch,
                )
                .unwrap();
            assert_eq!(
                service.snapshot().unwrap(),
                after,
                "Unchanged subsequent reads do not rewrite the evidence"
            );
        }
    }

    #[test]
    fn inventory_audit_timestamp_merge_preserves_real_proof_selection_and_binding_conflicts() {
        for action in ["get_files", "get_export_plan"] {
            let (_dir, service) = fixture();
            let mut before = service.manager.lock().unwrap().state.clone();
            before.skills[0]["runtimeAudit"] = json!({"method":"static_package","checkedAt":"old","status":"passed","scopeHashes":{"SKILL.md":"hash"},"inventoryHashes":{"SKILL.md":"hash"},"externalBindingStamp":"binding","proposedScope":["SKILL.md"],"externalDependencies":[{"id":"ref","role":"environment","source":"SKILL.md","checkedAt":"old","local":{"status":"available","checkedAt":"old"}}]});
            let mut after = before.clone();
            after.skills[0]["runtimeAudit"]["checkedAt"] = json!("worker");
            let mut live = before.clone();
            live.skills[0]["runtimeAudit"]["checkedAt"] = json!("live");
            assert!(merge_state(&before, &after, &live, action).is_ok());
            for (field, value) in [
                ("status", json!("failed")),
                ("inventoryHashes", json!({"SKILL.md":"other"})),
                ("scopeHashes", json!({"SKILL.md":"other"})),
                ("externalBindingStamp", json!("changed")),
                ("proposedScope", json!(["SKILL.md", "new.py"])),
            ] {
                let mut changed = live.clone();
                changed.skills[0]["runtimeAudit"][field] = value;
                assert!(
                    merge_state(&before, &after, &changed, action).is_err(),
                    "{field}"
                );
            }
            let mut changed_role = live.clone();
            changed_role.skills[0]["runtimeAudit"]["externalDependencies"][0]["role"] =
                json!("required");
            assert!(merge_state(&before, &after, &changed_role, action).is_err());
            for (field, value) in [
                ("runtimeFiles", json!(["SKILL.md", "README.md"])),
                (
                    "externalBindings",
                    json!([{"id":"new-binding","role":"required"}]),
                ),
            ] {
                let mut changed = live.clone();
                changed.skills[0][field] = value;
                assert!(
                    merge_state(&before, &after, &changed, action).is_err(),
                    "{field}"
                );
            }
        }
    }

    #[test]
    fn compatibility_detail_reads_do_not_emit_history_or_write_its_database_table() {
        let (dir, service) = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        service.set_events(Arc::new(move |event| captured.lock().unwrap().push(event)));
        let db = rusqlite::Connection::open(dir.path().join("manager.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER deny_test_history_write BEFORE DELETE ON task_history BEGIN SELECT RAISE(ABORT,'detail read must not rewrite history'); END").unwrap();
        service
            .execute(json!({"action":"read_file","skillId":"one"}), fake_dispatch)
            .unwrap();
        assert!(events.lock().unwrap().is_empty());
        assert!(service.list().unwrap().is_empty());
    }

    #[test]
    fn clearing_terminal_history_preserves_a_result_until_its_waiting_caller_reads_it() {
        let (_dir, service) = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        service.set_events(Arc::new(move |event| captured.lock().unwrap().push(event)));
        let task = start(
            &service,
            json!({"action":"apply_update","skillId":"one","fields":{"version":"saved-before-clear"}}),
        );
        task_status(&service, &task, "succeeded");
        service.clear_history().unwrap();
        assert!(service.list().unwrap().is_empty());
        assert!(service
            .manager
            .lock()
            .unwrap()
            .task_history()
            .unwrap()
            .is_empty());
        let event_count = events.lock().unwrap().len();
        let retained = service.get(&task, true).unwrap();
        assert_eq!(retained["hidden"], true);
        assert_eq!(
            retained["result"]["skills"][0]["version"],
            "saved-before-clear"
        );
        assert_eq!(
            service.wait(&task).unwrap()["skills"][0]["version"],
            "saved-before-clear"
        );
        service.emit(&task);
        assert_eq!(events.lock().unwrap().len(), event_count);
        assert!(service.list().unwrap().is_empty());
    }

    #[test]
    fn clear_keeps_running_tasks_and_retains_cleared_results_after_later_completions() {
        let (_dir, service) = fixture();
        let completed = start(&service, json!({"action":"read_file","skillId":"one"}));
        service.wait(&completed).unwrap();
        let (hold_id, latch) = new_latch();
        let running = start(
            &service,
            json!({"action":"read_file","skillId":"two","hold":hold_id}),
        );
        entered(&latch);
        service.clear_history().unwrap();
        assert_eq!(service.list().unwrap().len(), 1);
        assert_eq!(service.list().unwrap()[0]["id"], running);
        assert_eq!(
            service
                .manager
                .lock()
                .unwrap()
                .task_history()
                .unwrap()
                .len(),
            1
        );
        release(&latch);
        service.wait(&running).unwrap();
        assert_eq!(service.get(&completed, true).unwrap()["hidden"], true);
        assert_eq!(service.get(&running, true).unwrap()["status"], "succeeded");
        assert_eq!(service.list().unwrap().len(), 1);
    }

    #[test]
    fn cleared_session_results_share_the_same_one_hundred_terminal_memory_bound() {
        let (_dir, service) = fixture();
        let mut ids = Vec::new();
        for index in 0..120 {
            let task = start(&service, json!({"action":"read_file","skillId":"one"}));
            service.wait(&task).unwrap();
            ids.push(task);
            if index == 59 {
                service.clear_history().unwrap();
            }
        }
        assert_eq!(service.jobs.lock().unwrap().len(), 100);
        assert_eq!(service.list().unwrap().len(), 60);
        assert_eq!(
            service
                .manager
                .lock()
                .unwrap()
                .task_history()
                .unwrap()
                .len(),
            60
        );
        assert!(service.get(&ids[0], true).is_err());
        assert_eq!(service.get(&ids[40], true).unwrap()["hidden"], true);
        assert_eq!(service.get(&ids[119], true).unwrap()["status"], "succeeded");
    }

    #[test]
    fn an_invisible_fast_result_is_pinned_before_its_caller_reaches_wait() {
        let (_dir, service) = fixture();
        let task = service
            .start(
                json!({"action":"get_network_settings"}),
                None,
                None,
                true,
                false,
                fake_dispatch,
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        task_status(&service, &task, "succeeded");
        let limit = Instant::now() + Duration::from_secs(5);
        while service
            .jobs
            .lock()
            .unwrap()
            .get(&task)
            .is_some_and(|job| Arc::strong_count(&job.completion) > 1)
        {
            assert!(Instant::now() < limit);
            std::thread::sleep(Duration::from_millis(2));
        }
        let other = start(&service, json!({"action":"read_file","skillId":"two"}));
        service.wait(&other).unwrap();
        assert!(service.wait(&task).unwrap()["skills"].is_array());
        service.release_compat(&task);
        assert!(service.get(&task, true).is_err());
    }

    #[test]
    fn deduplicated_compatibility_callers_each_hold_their_completion_until_read() {
        let (_dir, service) = fixture();
        let (hold_id, latch) = new_latch();
        let first = service
            .start(
                json!({"action":"refresh","skillId":"one","localOnly":true,"hold":hold_id}),
                None,
                None,
                true,
                false,
                fake_dispatch,
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        entered(&latch);
        let second = service
            .start(
                json!({"action":"refresh","skillIds":["one"],"localOnly":true}),
                None,
                None,
                true,
                false,
                fake_dispatch,
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(first, second);
        assert_eq!(service.jobs.lock().unwrap()[&first].consumers, 2);
        release(&latch);
        service.wait(&first).unwrap();
        service.release_compat(&first);
        let other = start(&service, json!({"action":"read_file","skillId":"two"}));
        service.wait(&other).unwrap();
        service.wait(&second).unwrap();
        service.release_compat(&second);
        assert!(service.get(&first, true).is_err());
    }

    #[test]
    fn awaited_compatibility_execution_does_not_accumulate_invisible_results() {
        let (_dir, service) = fixture();
        for _ in 0..40 {
            service
                .execute(json!({"action":"read_file","skillId":"one"}), fake_dispatch)
                .unwrap();
        }
        assert!(service.jobs.lock().unwrap().is_empty());
        assert!(service.list().unwrap().is_empty());
        assert!(service
            .manager
            .lock()
            .unwrap()
            .task_history()
            .unwrap()
            .is_empty());
    }
}
