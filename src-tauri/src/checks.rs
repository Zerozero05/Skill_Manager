//! One backend scheduler serves every window. Native directory events only
//! enqueue scoped reads; all state changes retain TaskSystem's merge checks.
use crate::{local_roots::path_key, tasks::TaskSystem};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    time::{Duration, Instant, SystemTime},
};

const DEBOUNCE: Duration = Duration::from_millis(850);
const MAX_DEBOUNCE: Duration = Duration::from_millis(1600);
const REMOTE_INTERVAL: Duration = Duration::from_secs(60);
const FALLBACK_INTERVAL: Duration = Duration::from_secs(5);

fn watched_paths(config: &Value) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = config["roots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|root| root["enabled"] != false)
        .filter_map(|root| root["resolvedPath"].as_str().map(PathBuf::from))
        .chain(
            config["skills"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|skill| skill["localPath"].as_str().map(PathBuf::from)),
        )
        .collect();
    paths.sort_by_key(|path| path.components().count());
    let mut roots = Vec::new();
    let separator = if cfg!(windows) { "\\" } else { "/" };
    for path in paths {
        let key = path_key(&path);
        if !roots.iter().any(|root: &PathBuf| {
            let root = path_key(root);
            key == root || key.starts_with(&(root + separator))
        }) {
            roots.push(path);
        }
    }
    roots
}

fn dirty_ids(config: &Value, paths: &BTreeSet<PathBuf>) -> Vec<Value> {
    config["skills"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|skill| {
            skill["localPath"].as_str().is_some_and(|root| {
                paths.iter().any(|path| {
                    let root = path_key(Path::new(root));
                    let path = path_key(path);
                    let separator = if cfg!(windows) { "\\" } else { "/" };
                    path == root
                        || path.starts_with(&(root.clone() + separator))
                        || root.starts_with(&(path + separator))
                })
            })
        })
        .map(|skill| skill["id"].clone())
        .collect()
}

fn metadata_stamp(root: &Path) -> BTreeMap<String, (u64, Option<SystemTime>)> {
    fn visit(
        path: &Path,
        root: &Path,
        depth: usize,
        values: &mut BTreeMap<String, (u64, Option<SystemTime>)>,
    ) {
        if depth > 32 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            #[cfg(windows)]
            let is_link = {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let is_link = metadata.file_type().is_symlink();
            if is_link {
                continue;
            }
            if metadata.is_dir() {
                if !matches!(
                    entry.file_name().to_string_lossy().as_ref(),
                    ".git" | ".skill-manager"
                ) {
                    visit(&path, root, depth + 1, values);
                }
            } else if metadata.is_file() {
                values.insert(
                    path.strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned(),
                    (metadata.len(), metadata.modified().ok()),
                );
            }
        }
    }
    let mut values = BTreeMap::new();
    visit(root, root, 0, &mut values);
    values
}

pub(crate) fn start(service: &Arc<TaskSystem>) {
    let weak = Arc::downgrade(service);
    std::thread::spawn(move || {
        let (sender, receiver) = mpsc::channel::<Result<Vec<PathBuf>, ()>>();
        let mut previous_generation = None;
        let mut previous_paths = Vec::<PathBuf>::new();
        let mut pending = BTreeSet::new();
        let mut first_event = None;
        let mut last_event = None;
        let mut remote_checked = Instant::now();
        let mut fallback_checked = Instant::now();
        let mut stamps = BTreeMap::new();
        let mut fallback = false;
        #[cfg(windows)]
        let mut watcher: Option<notify::RecommendedWatcher> = None;
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let Some(service) = weak.upgrade() else {
                break;
            };
            let Ok(config) = service.check_configuration() else {
                continue;
            };
            if config["mode"] != "auto" {
                service.cancel_automatic();
                #[cfg(windows)]
                {
                    drop(watcher.take());
                }
                previous_generation = None;
                previous_paths.clear();
                pending.clear();
                first_event = None;
                last_event = None;
                stamps.clear();
                while receiver.try_recv().is_ok() {}
                continue;
            }
            let generation = config["generation"].as_u64().unwrap_or(0);
            let paths = watched_paths(&config);
            let changed = previous_generation != Some(generation);
            if changed || paths != previous_paths {
                pending.clear();
                first_event = None;
                last_event = None;
                #[cfg(windows)]
                {
                    use notify::Watcher;
                    drop(watcher.take());
                    let sender = sender.clone();
                    watcher =
                        notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                            match event {
                                Ok(event)
                                    if !matches!(event.kind, notify::EventKind::Access(_)) =>
                                {
                                    let _ = sender.send(Ok(event.paths));
                                }
                                Ok(_) => {}
                                Err(_) => {
                                    let _ = sender.send(Err(()));
                                }
                            }
                        })
                        .ok();
                    fallback = watcher.is_none();
                    if let Some(watcher) = watcher.as_mut() {
                        for path in &paths {
                            if watcher
                                .watch(path, notify::RecursiveMode::Recursive)
                                .is_err()
                            {
                                fallback = true;
                            }
                        }
                    }
                }
                #[cfg(not(windows))]
                {
                    let _ = &sender;
                    fallback = true;
                }
                previous_paths = paths;
                previous_generation = Some(generation);
                stamps.clear();
                if fallback {
                    for skill in config["skills"].as_array().into_iter().flatten() {
                        if let Some(path) = skill["localPath"].as_str() {
                            stamps.insert(path.to_owned(), metadata_stamp(Path::new(path)));
                        }
                    }
                }
                if changed {
                    let _ = service.automatic_local_check(
                        json!({"automatic":true,"checkGeneration":generation,"full":true}),
                    );
                    remote_checked = Instant::now();
                    fallback_checked = Instant::now();
                }
            }
            while let Ok(event) = receiver.try_recv() {
                match event {
                    Ok(paths) => {
                        if first_event.is_none() {
                            first_event = Some(Instant::now());
                        }
                        last_event = Some(Instant::now());
                        for path in paths {
                            if pending.len() < 4096 {
                                pending.insert(path);
                            }
                        }
                    }
                    Err(()) => {
                        fallback = true;
                    }
                }
            }
            let due = last_event.is_some_and(|time| time.elapsed() >= DEBOUNCE)
                || first_event.is_some_and(|time| time.elapsed() >= MAX_DEBOUNCE);
            if due && !pending.is_empty() {
                let ids = dirty_ids(&config, &pending);
                let _=service.automatic_local_check(json!({"automatic":true,"checkGeneration":generation,"skillIds":ids,"changedPaths":pending}));
                pending.clear();
                first_event = None;
                last_event = None;
            }
            if fallback && fallback_checked.elapsed() >= FALLBACK_INTERVAL {
                let mut dirty = BTreeSet::new();
                for skill in config["skills"].as_array().into_iter().flatten() {
                    if let Some(path) = skill["localPath"].as_str() {
                        let current = metadata_stamp(Path::new(path));
                        if stamps
                            .get(path)
                            .is_some_and(|previous| previous != &current)
                        {
                            dirty.insert(PathBuf::from(path));
                        }
                        stamps.insert(path.to_owned(), current);
                    }
                }
                let ids = dirty_ids(&config, &dirty);
                let _ = service.automatic_local_check(
                    json!({"automatic":true,"checkGeneration":generation,"skillIds":ids}),
                );
                fallback_checked = Instant::now();
            }
            if remote_checked.elapsed() >= REMOTE_INTERVAL {
                let _=service.automatic_check(json!({"action":"check_now","automatic":true,"checkGeneration":generation,"remoteOnly":true}));
                remote_checked = Instant::now();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn events_target_only_affected_skills_and_roots_are_minimal() {
        let config = json!({"roots":[{"resolvedPath":"/skills","enabled":true},{"resolvedPath":"/paused","enabled":false}],"skills":[{"id":"a","localPath":"/skills/a"},{"id":"b","localPath":"/skills/b"},{"id":"c","localPath":"/independent/c"}]});
        assert_eq!(
            watched_paths(&config),
            vec![PathBuf::from("/skills"), PathBuf::from("/independent/c")]
        );
        assert_eq!(
            dirty_ids(
                &config,
                &BTreeSet::from([PathBuf::from("/skills/a/SKILL.md")])
            ),
            vec![json!("a")]
        );
    }
}
