mod audit;
mod categories;
mod checks;
mod clients;
mod export_plan;
mod external;
mod local;
mod local_roots;
mod model;
mod network;
mod remote;
mod storage;
mod tasks;
mod transaction;
mod workspace;

use model::Manager;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};

fn folder_for_skill(m: &Manager, request: &Value) -> Result<PathBuf, String> {
    let skill = m.skill(request["skillId"].as_str().ok_or("Missing skillId")?)?;
    let path = skill["localPath"]
        .as_str()
        .ok_or("This Skill has no local folder yet")?;
    let folder = local::canonical(std::path::Path::new(path))?;
    if !folder.is_dir() {
        return Err("Skill folder is unavailable".into());
    }
    Ok(folder)
}

fn dispatch(m: &mut Manager, request: &Value) -> Result<Value, String> {
    if let Some(value) = local_roots::handle(m, request)? {
        return Ok(value);
    }
    match request["action"].as_str().ok_or("Missing action")? {
        "get_state" => {
            return Ok(m.snapshot());
        }
        "refresh" | "audit_skills" => {
            m.refresh_selection(request)?;
            clients::refresh(m)?;
            m.save()?;
        }
        "set_settings" => {
            let mut settings = m.state.settings.clone();
            if let Some(editor) = request["settings"]["editorPath"].as_str() {
                settings["editorPath"] = json!(editor);
            }
            if let Some(theme) = request["settings"].get("uiTheme") {
                if !matches!(theme.as_str(), Some("light" | "dark" | "eye")) {
                    return Err("Choose light, dark or eye comfort appearance".into());
                }
                settings["uiTheme"] = theme.clone();
            }
            for (name, allowed) in [
                ("libraryMode", &["list", "details", "cards"][..]),
                ("librarySort", &["name", "status", "checked"][..]),
            ] {
                if let Some(value) = request["settings"].get(name) {
                    if !value.as_str().is_some_and(|v| allowed.contains(&v)) {
                        return Err(format!("Invalid {name}"));
                    }
                    settings[name] = value.clone();
                }
            }
            if let Some(value) = request["settings"].get("libraryPageSize") {
                if !matches!(value.as_u64(), Some(20 | 50 | 100)) {
                    return Err("Page size must be 20, 50 or 100".into());
                }
                settings["libraryPageSize"] = value.clone();
            }
            for (name, min, max) in [
                ("uiZoom", 80, 160),
                ("uiFontSize", 14, 22),
                ("sidebarWidth", 200, 420),
            ] {
                if let Some(value) = request["settings"].get(name) {
                    let number = value
                        .as_u64()
                        .filter(|n| (min..=max).contains(n))
                        .ok_or_else(|| format!("{name} must be an integer from {min} to {max}"))?;
                    settings[name] = json!(number);
                }
            }
            if let Some(value) = request["settings"].get("sidebarCollapsed") {
                if !value.is_boolean() {
                    return Err("sidebarCollapsed must be true or false".into());
                }
                settings["sidebarCollapsed"] = value.clone();
            }
            settings["checkIntervalMinutes"] = json!(30);
            let previous = std::mem::replace(&mut m.state.settings, settings);
            if let Err(error) = m.save() {
                m.state.settings = previous;
                return Err(error);
            }
            return Ok(m.snapshot());
        }
        "open_local" => {
            let s = m.skill(request["skillId"].as_str().ok_or("Missing skillId")?)?;
            let path = s["localPath"]
                .as_str()
                .ok_or("This skill has not been installed")?;
            if !std::path::Path::new(path).is_dir() {
                return Err("Skill folder is unavailable".into());
            }
            let editor = m.state.settings["editorPath"].as_str().unwrap_or("");
            #[cfg(windows)]
            let mut command = std::process::Command::new(if editor.is_empty() {
                "explorer.exe"
            } else {
                editor
            });
            #[cfg(not(windows))]
            let mut command = std::process::Command::new(if editor.is_empty() {
                "xdg-open"
            } else {
                editor
            });
            command
                .arg(path)
                .spawn()
                .map_err(|e| format!("Could not open editor/folder: {e}"))?;
            return Ok(json!({"opened":true}));
        }
        "open_folder" => {
            let folder = folder_for_skill(m, request)?;
            #[cfg(windows)]
            let mut command = std::process::Command::new("explorer.exe");
            #[cfg(not(windows))]
            let mut command = std::process::Command::new("xdg-open");
            command
                .arg(&folder)
                .spawn()
                .map_err(|e| format!("Could not open Skill folder: {e}"))?;
            return Ok(json!({"opened":true,"path":folder.to_string_lossy()}));
        }
        _ => {}
    }
    if let Some(v) = categories::handle(m, request)? {
        return Ok(v);
    }
    if let Some(v) = storage::handle(m, request)? {
        return Ok(v);
    }
    if let Some(v) = clients::handle(m, request)? {
        return Ok(v);
    }
    if let Some(v) = audit::handle(m, request)? {
        return Ok(v);
    }
    if let Some(v) = network::handle(m, request)? {
        return Ok(v);
    }
    if let Some(v) = export_plan::handle(m, request)? {
        return Ok(v);
    }
    if let Some(v) = external::handle(m, request)? {
        return Ok(v);
    }
    if let Some(v) = remote::file_request(m, request)? {
        return Ok(v);
    }
    if let Some(v) = local::handle(m, request)? {
        if request["action"] == "add_local" && !m.state.sources.is_empty() {
            return remote::handle(m, &json!({"action":"auto_associate"}))?
                .ok_or_else(|| "Automatic source association is unavailable".into());
        }
        return Ok(v);
    }
    if let Some(v) = remote::handle(m, request)? {
        return Ok(v);
    }
    Err("Unknown action".into())
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn export_inventory_does_not_promote_optional_readme_image_into_default_zip() {
        let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/export-inventory-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let fixture = tempfile::tempdir_in(parent).unwrap();
        let root = fixture.path().join("skill");
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(
            root.join("SKILL.md"),
            "---\nname: inventory\n---\nUse the skill.\n",
        )
        .unwrap();
        std::fs::write(
            root.join("README.md"),
            "![Optional illustration](assets/readme.png)",
        )
        .unwrap();
        std::fs::write(root.join("assets/readme.png"), b"synthetic image fixture").unwrap();
        let mut m = Manager::new(fixture.path().join("state")).unwrap();
        let mut skill = local::draft(&root).unwrap();
        skill["id"] = json!("inventory");
        skill["runtimeConfirmed"] = json!(true);
        skill["runtimeFiles"] = json!(["SKILL.md", "README.md", "assets/readme.png"]);
        m.state.skills.push(skill);
        let files = dispatch(
            &mut m,
            &json!({"action":"get_files", "skillId":"inventory"}),
        )
        .unwrap();
        assert!(files["requiredFiles"]
            .as_array()
            .unwrap()
            .contains(&json!("assets/readme.png")));
        assert_eq!(files["exportRequiredFiles"], json!(["SKILL.md"]));
        assert!(files["exportError"].is_null());
        assert!(root.join("README.md").exists());
    }

    #[test]
    fn source_folder_uses_registered_path_independently_of_editor_and_rejects_unavailable_entries()
    {
        let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/folder-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let fixture = tempfile::tempdir_in(parent).unwrap();
        let folder = fixture.path().join("a folder with spaces");
        std::fs::create_dir(&folder).unwrap();
        let mut m = Manager::new(fixture.path().join("state")).unwrap();
        m.state.settings["editorPath"] = json!("not-an-executable");
        m.state
            .skills
            .push(json!({"id":"local","localPath":folder.to_string_lossy()}));
        m.state.skills.push(json!({"id":"remote","localPath":null}));
        let request = json!({"skillId":"local","path":"ignored-arbitrary-path"});
        assert_eq!(
            folder_for_skill(&m, &request).unwrap(),
            local::canonical(&folder).unwrap()
        );
        assert!(folder_for_skill(&m, &json!({"skillId":"remote"})).is_err());
        assert!(folder_for_skill(&m, &json!({"skillId":"unknown"})).is_err());
        m.state.skills[0]["localPath"] = json!(folder.join("missing").to_string_lossy());
        assert!(folder_for_skill(&m, &request).is_err());
    }

    #[test]
    fn display_preferences_survive_restart_and_invalid_values_do_not_partially_apply() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/settings-tests");
        std::fs::create_dir_all(&root).unwrap();
        let fixture = tempfile::tempdir_in(root).unwrap();
        {
            let mut m = Manager::new(fixture.path().to_path_buf()).unwrap();
            dispatch(&mut m, &json!({"action":"set_settings","settings":{"uiTheme":"dark","uiZoom":125,"uiFontSize":18}})).unwrap();
            let before = m.state.settings.clone();
            for change in [
                json!({"uiTheme":"unknown"}),
                json!({"uiZoom":200}),
                json!({"uiFontSize":9}),
                json!({"uiZoom":100.5}),
            ] {
                assert!(
                    dispatch(&mut m, &json!({"action":"set_settings","settings":change})).is_err()
                );
                assert_eq!(m.state.settings, before);
            }
        }
        let m = Manager::new(fixture.path().to_path_buf()).unwrap();
        assert_eq!(m.state.settings["uiTheme"], "dark");
        assert_eq!(m.state.settings["uiZoom"], 125);
        assert_eq!(m.state.settings["uiFontSize"], 18);
    }

    #[test]
    fn list_preferences_persist_without_resetting_display_and_reject_invalid_values() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/settings-tests");
        std::fs::create_dir_all(&root).unwrap();
        let fixture = tempfile::tempdir_in(root).unwrap();
        {
            let mut m = Manager::new(fixture.path().to_path_buf()).unwrap();
            dispatch(&mut m, &json!({"action":"set_settings","settings":{"uiTheme":"eye","uiZoom":115,"uiFontSize":17}})).unwrap();
            dispatch(&mut m, &json!({"action":"set_settings","settings":{"libraryMode":"details","libraryPageSize":50,"librarySort":"status"}})).unwrap();
            assert_eq!(m.state.settings["uiTheme"], "eye");
            assert_eq!(m.state.settings["uiZoom"], 115);
            let saved = m.state.settings.clone();
            for settings in [
                json!({"libraryMode":"unknown"}),
                json!({"libraryPageSize":0}),
                json!({"libraryPageSize":50.5}),
                json!({"librarySort":"other"}),
            ] {
                assert!(dispatch(
                    &mut m,
                    &json!({"action":"set_settings","settings":settings})
                )
                .is_err());
                assert_eq!(m.state.settings, saved);
            }
        }
        let m = Manager::new(fixture.path().to_path_buf()).unwrap();
        assert_eq!(m.state.settings["libraryMode"], "details");
        assert_eq!(m.state.settings["libraryPageSize"], 50);
        assert_eq!(m.state.settings["librarySort"], "status");
        assert_eq!(m.state.settings["uiFontSize"], 17);
    }

    #[test]
    fn cards_mode_preserves_unknown_settings_and_invalid_combined_writes_are_atomic() {
        let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/settings-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let mut m = Manager::new(directory.path().to_path_buf()).unwrap();
        m.state.settings["futurePreference"] = json!({"keep": [1,2,3]});
        dispatch(
            &mut m,
            &json!({"action":"set_settings","settings":{"libraryMode":"cards"}}),
        )
        .unwrap();
        let saved = m.state.settings.clone();
        assert!(dispatch(
            &mut m,
            &json!({"action":"set_settings","settings":{"libraryMode":"unknown","uiFontSize":22}})
        )
        .is_err());
        assert_eq!(m.state.settings, saved);
        let restored = Manager::new(directory.path().to_path_buf()).unwrap();
        assert_eq!(restored.state.settings["libraryMode"], "cards");
        assert_eq!(
            restored.state.settings["futurePreference"],
            json!({"keep": [1,2,3]})
        );
        assert_eq!(restored.state.settings["uiZoom"], saved["uiZoom"]);
    }

    #[test]
    fn layout_preferences_are_partial_validated_and_roll_back_when_saving_fails() {
        let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/settings-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let fixture = tempfile::tempdir_in(parent).unwrap();
        let mut m = Manager::new(fixture.path().to_path_buf()).unwrap();
        dispatch(
            &mut m,
            &json!({"action":"create_category","name":"Research"}),
        )
        .unwrap();
        let categories = m.state.settings["categories"].clone();
        dispatch(&mut m, &json!({"action":"set_settings","settings":{"uiTheme":"eye","uiZoom":113,"uiFontSize":17}})).unwrap();
        dispatch(&mut m, &json!({"action":"set_settings","settings":{"sidebarWidth":380,"sidebarCollapsed":true}})).unwrap();
        let saved = m.state.settings.clone();
        assert_eq!(saved["categories"], categories);
        assert_eq!(saved["uiTheme"], "eye");
        assert_eq!(saved["uiZoom"], 113);
        assert_eq!(saved["uiFontSize"], 17);
        for settings in [
            json!({"sidebarWidth":199}),
            json!({"sidebarWidth":421}),
            json!({"sidebarWidth":224.5}),
            json!({"sidebarWidth":250,"sidebarCollapsed":"false"}),
            json!({"sidebarCollapsed":null}),
        ] {
            assert!(dispatch(
                &mut m,
                &json!({"action":"set_settings","settings":settings})
            )
            .is_err());
            assert_eq!(m.state.settings, saved);
        }
        drop(m);
        let mut m = Manager::new(fixture.path().to_path_buf()).unwrap();
        assert_eq!(m.state.settings, saved);
        let db = rusqlite::Connection::open(fixture.path().join("manager.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER prevent_insert BEFORE INSERT ON app_state BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;").unwrap();
        assert!(dispatch(&mut m, &json!({"action":"set_settings","settings":{"sidebarWidth":224,"sidebarCollapsed":false,"uiZoom":100}})).is_err());
        assert_eq!(m.state.settings, saved);
    }

    #[test]
    fn selected_refresh_only_checks_requested_local_skills_and_validates_before_changes() {
        let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/selection-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let fixture = tempfile::tempdir_in(parent).unwrap();
        let mut m = Manager::new(fixture.path().join("state")).unwrap();
        let roots: Vec<_> = (0..3)
            .map(|n| {
                let root = fixture.path().join(format!("skill-{n}"));
                std::fs::create_dir_all(&root).unwrap();
                std::fs::write(
                    root.join("SKILL.md"),
                    format!("---\nname: skill-{n}\ndescription: before\n---\nSynthetic.\n"),
                )
                .unwrap();
                root
            })
            .collect();
        dispatch(&mut m, &json!({"action":"add_local","paths":roots})).unwrap();
        let ids: Vec<_> = m
            .state
            .skills
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_string())
            .collect();
        let untouched = m.skill(&ids[2]).unwrap();
        for (n, root) in roots.iter().enumerate() {
            std::fs::write(
                root.join("SKILL.md"),
                format!("---\nname: skill-{n}\ndescription: after\n---\nSynthetic changed.\n"),
            )
            .unwrap();
        }
        dispatch(
            &mut m,
            &json!({"action":"refresh","skillIds":[ids[0],ids[1],ids[0]]}),
        )
        .unwrap();
        assert_eq!(m.skill(&ids[0]).unwrap()["description"], "after");
        assert_eq!(m.skill(&ids[1]).unwrap()["description"], "after");
        assert_eq!(m.skill(&ids[2]).unwrap(), untouched);
        let before = serde_json::to_value(&m.state).unwrap();
        for request in [
            json!({"action":"refresh","skillIds":[]}),
            json!({"action":"refresh","skillIds":[ids[0],"missing"]}),
            json!({"action":"refresh","skillIds":[ids[0]],"skillId":ids[1]}),
            json!({"action":"refresh","skillIds":"not a list"}),
        ] {
            assert!(dispatch(&mut m, &request).is_err());
            assert_eq!(serde_json::to_value(&m.state).unwrap(), before);
        }
    }
}

#[tauri::command]
async fn manager_request(
    state: tauri::State<'_, Arc<tasks::TaskSystem>>,
    workspace: tauri::State<'_, Arc<workspace::WorkspaceSystem>>,
    window: tauri::WebviewWindow,
    mut request: Value,
) -> Result<Value, String> {
    use tauri::Manager;
    let service = state.inner().clone();
    let workspace = workspace.inner().clone();
    let label = window.label().to_owned();
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(value) = workspace.handle(&app, &label, &request)? {
            return Ok(value);
        }
        workspace.inject_origin(&label, &mut request)?;
        service.execute(request, dispatch)
    })
    .await
    .map_err(|_| "Background operation failed".to_string())?
}

fn resolve_data_dir(
    args: &[std::ffi::OsString],
    environment: Option<std::ffi::OsString>,
    executable: Result<PathBuf, String>,
) -> Result<PathBuf, String> {
    if let Some(index) = args.iter().position(|arg| arg == "--data-dir") {
        let value = args
            .get(index + 1)
            .filter(|value| !value.is_empty() && !value.to_string_lossy().starts_with("--"))
            .ok_or("--data-dir requires a directory path")?;
        return Ok(PathBuf::from(value));
    }
    if let Some(value) = environment {
        if value.is_empty() {
            return Err("SKILL_MANAGER_DATA_DIR must not be empty".into());
        }
        return Ok(PathBuf::from(value));
    }

    let executable = executable?;
    let executable_dir = executable
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or("Could not determine the application directory")?;
    let config_path = executable_dir.join("data-directory.txt");
    let config = match std::fs::File::open(&config_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(executable_dir.join("Skill_Manager_Data"));
        }
        Err(error) => {
            return Err(format!("Could not read {}: {error}", config_path.display()));
        }
    };
    use std::io::Read;
    let mut bytes = Vec::new();
    config
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read {}: {error}", config_path.display()))?;
    if bytes.len() > 4096 {
        return Err(format!(
            "{} must be at most 4096 bytes",
            config_path.display()
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| format!("{} must contain a UTF-8 path", config_path.display()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let line = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    if line.contains(['\r', '\n', '\0']) || line.trim().is_empty() {
        return Err(format!(
            "{} must contain one non-empty directory path on a single line",
            config_path.display()
        ));
    }
    let path = PathBuf::from(line.trim());
    if path.is_absolute() {
        Ok(path)
    } else if path.has_root()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::Prefix(_)))
    {
        Err(format!(
            "{} must contain an absolute path or a path relative to the application directory",
            config_path.display()
        ))
    } else {
        Ok(executable_dir.join(path))
    }
}

#[cfg(test)]
mod data_directory_tests {
    use super::*;
    use std::ffi::OsString;

    fn fixture() -> tempfile::TempDir {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/data-directory-tests");
        std::fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }

    #[test]
    fn explicit_paths_take_priority_without_requiring_an_executable_path() {
        let args = [
            OsString::from("app"),
            OsString::from("--data-dir"),
            OsString::from("selected"),
        ];
        assert_eq!(
            resolve_data_dir(&args, Some("environment".into()), Err("unavailable".into())).unwrap(),
            PathBuf::from("selected")
        );
        assert_eq!(
            resolve_data_dir(&[], Some("environment".into()), Err("unavailable".into())).unwrap(),
            PathBuf::from("environment")
        );
    }

    #[test]
    fn sidecar_supports_absolute_and_application_relative_paths() {
        let root = fixture();
        let exe = root.path().join("skill-manager.exe");
        let sidecar = root.path().join("data-directory.txt");
        let resolve = || resolve_data_dir(&[], None, Ok(exe.clone()));
        assert_eq!(resolve().unwrap(), root.path().join("Skill_Manager_Data"));
        std::fs::write(&sidecar, "\u{feff}state/管理数据\r\n").unwrap();
        assert_eq!(resolve().unwrap(), root.path().join("state/管理数据"));
        let absolute = root.path().join("absolute-data");
        std::fs::write(&sidecar, absolute.to_str().unwrap()).unwrap();
        assert_eq!(resolve().unwrap(), absolute);
        std::fs::write(&sidecar, "invalid\nconfiguration").unwrap();
        assert_eq!(
            resolve_data_dir(&[], Some("override".into()), Ok(exe)).unwrap(),
            PathBuf::from("override")
        );
    }

    #[test]
    fn invalid_sidecars_fail_instead_of_silently_selecting_another_database() {
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            b" \t\r\n".to_vec(),
            b"one\ntwo".to_vec(),
            b"one\n\n".to_vec(),
            b"one\rtwo".to_vec(),
            b"one\0two".to_vec(),
            vec![b'a'; 4097],
            vec![0xff],
        ];
        for bytes in cases {
            let root = fixture();
            std::fs::write(root.path().join("data-directory.txt"), bytes).unwrap();
            assert!(resolve_data_dir(&[], None, Ok(root.path().join("app.exe"))).is_err());
            assert!(!root.path().join("Skill_Manager_Data").exists());
        }
        let root = fixture();
        std::fs::create_dir(root.path().join("data-directory.txt")).unwrap();
        assert!(resolve_data_dir(&[], None, Ok(root.path().join("app.exe"))).is_err());
    }

    #[test]
    fn empty_explicit_values_and_unavailable_executable_fail() {
        for args in [
            vec![OsString::from("--data-dir")],
            vec![OsString::from("--data-dir"), OsString::new()],
            vec![OsString::from("--data-dir"), OsString::from("--other")],
        ] {
            assert!(
                resolve_data_dir(&args, Some("fallback".into()), Err("no exe".into())).is_err()
            );
        }
        assert!(resolve_data_dir(&[], Some(OsString::new()), Err("no exe".into())).is_err());
        assert!(resolve_data_dir(&[], None, Err("no exe".into())).is_err());
    }
}

pub fn run() {
    let args: Vec<_> = std::env::args_os().collect();
    let data_dir = match resolve_data_dir(
        &args,
        std::env::var_os("SKILL_MANAGER_DATA_DIR"),
        std::env::current_exe()
            .map_err(|error| format!("Could not determine the application path: {error}")),
    ) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Skill Manager could not initialize: {error}");
            return;
        }
    };
    let source = if args.iter().any(|a| a == "--data-dir") {
        "commandLine"
    } else if std::env::var_os("SKILL_MANAGER_DATA_DIR").is_some() {
        "environment"
    } else if std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("data-directory.txt")))
        .is_some_and(|p| p.exists())
    {
        "legacySidecar"
    } else {
        "applicationDefault"
    };
    let storage = match std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|exe| {
            storage::Storage::load(
                data_dir.clone(),
                &exe,
                source,
                matches!(source, "commandLine" | "environment"),
            )
        }) {
        Ok(storage) => storage,
        Err(error) => {
            eprintln!("Skill Manager could not initialize storage: {error}");
            return;
        }
    };
    let manager = Manager::with_storage(storage.clone()).and_then(|mut m| {
        let recovered = transaction::recover_in(&storage.path("recovery")?)?;
        if !recovered.is_empty() {
            m.state.settings["recoveryMessages"] = json!(recovered);
        }
        Ok(m)
    });
    let service = match manager.and_then(tasks::TaskSystem::new) {
        Ok(service) => service,
        Err(e) => {
            eprintln!("Skill Manager could not initialize: {e}");
            return;
        }
    };
    let mut context = tauri::generate_context!();
    #[cfg(debug_assertions)]
    for window in &mut context.config_mut().app.windows {
        if let Ok(port) = std::env::var("SKILL_MANAGER_DEBUG_PORT")
            .unwrap_or_default()
            .parse::<u16>()
        {
            if port >= 1024 {
                window.additional_browser_args = Some(format!(
                    "--remote-debugging-port={port} --remote-debugging-address=127.0.0.1"
                ));
            }
        }
    }
    let windows: Vec<_> = context
        .config()
        .app
        .windows
        .iter()
        .filter(|w| w.create)
        .cloned()
        .collect();
    for window in &mut context.config_mut().app.windows {
        window.create = false;
    }
    let workspace = match workspace::WorkspaceSystem::new(
        service.clone(),
        storage
            .path("webview")
            .expect("validated WebView directory"),
        windows.first().cloned().unwrap_or_default(),
    ) {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("Skill Manager could not initialize workspace: {error}");
            return;
        }
    };
    let lifecycle = workspace.clone();
    tauri::Builder::default()
        .manage(service.clone())
        .manage(workspace.clone())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![manager_request])
        .setup(move |app| {
            use tauri::Emitter;
            let handle = app.handle().clone();
            service.set_events(Arc::new(move |payload| {
                let event = if payload["clearedTaskIds"].is_array() {
                    "task-history-cleared"
                } else {
                    "task-updated"
                };
                let _ = handle.emit(event, payload);
            }));
            let handle = app.handle().clone();
            service.set_state_events(Arc::new(move |revision| {
                let _ = handle.emit("state-changed", json!({"stateRevision":revision}));
            }))?;
            workspace.restore_windows(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                use tauri::Manager;
                let prevent = window
                    .app_handle()
                    .try_state::<Arc<workspace::WorkspaceSystem>>()
                    .is_some_and(|workspace| {
                        workspace.native_close_requested(window.app_handle(), window.label())
                    });
                // The approved handshake uses destroy(), which does not re-enter this gate.
                if prevent {
                    api.prevent_close();
                }
            }
            if matches!(event, tauri::WindowEvent::Destroyed) {
                use tauri::Manager;
                if let Some(workspace) = window
                    .app_handle()
                    .try_state::<Arc<workspace::WorkspaceSystem>>()
                {
                    workspace.destroyed(window.app_handle(), window.label());
                }
            }
        })
        .build(context)
        .expect("Could not build Skill Manager")
        .run(move |_app, event| {
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = event
            {
                if lifecycle.migration_in_progress() {
                    api.prevent_exit();
                }
            }
        });
}
