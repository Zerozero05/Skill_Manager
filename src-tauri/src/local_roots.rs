//! Explicit directory sources discover Skills in place. No runtime file is copied.
use crate::{local, model::{now, Manager, State}};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs, path::{Path, PathBuf}};

pub(crate) fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    if cfg!(windows) { text.to_lowercase() } else { path.to_string_lossy().into_owned() }
}

fn strings(request: &Value, field: &str) -> Result<Vec<String>, String> {
    request[field].as_array().ok_or_else(|| format!("{field} 必须是列表"))?.iter()
        .map(|value| value.as_str().filter(|text| !text.is_empty()).map(str::to_owned).ok_or_else(|| format!("{field} 包含无效项目"))).collect()
}
fn link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)] { use std::os::windows::fs::MetadataExt; metadata.file_attributes() & 0x400 != 0 }
    #[cfg(not(windows))] { metadata.file_type().is_symlink() }
}
fn visit(path: &Path, depth: usize, candidates: &mut BTreeSet<PathBuf>, errors: &mut Vec<String>) {
    if depth > 32 || local::managed_path(path) { return; }
    if path.join("SKILL.md").is_file() {
        if local::readonly_reason(path).is_none() { candidates.insert(path.to_owned()); }
        return;
    }
    let entries = match fs::read_dir(path) { Ok(entries) => entries, Err(error) => { errors.push(format!("目录暂不可用 {}：{error}",path.display())); return; } };
    for entry in entries {
        let entry = match entry { Ok(entry) => entry, Err(error) => {errors.push(error.to_string());continue;} };
        if matches!(entry.file_name().to_string_lossy().as_ref(), ".git" | ".skill-manager" | ".skill-manager-state.json") { continue; }
        match fs::symlink_metadata(entry.path()) {
            Ok(metadata) if metadata.is_dir() && !link(&metadata) => visit(&entry.path(),depth+1,candidates,errors),
            Ok(_) => {},
            Err(error) => errors.push(format!("无法读取 {}：{error}",entry.path().display())),
        }
    }
}

#[derive(Default)]
pub(crate) struct Discovery { pub imported: usize, pub errors: Vec<String>, pub changed_ids: Vec<String> }

fn registration_ready(path:&Path)->Result<(),String>{
    let file=path.join("SKILL.md");
    if fs::metadata(&file).map_err(|error|error.to_string())?.len()>1024*1024{return Err("SKILL.md 超出可识别范围".into());}
    let content=fs::read_to_string(file).map_err(|error|error.to_string())?;
    let mut lines=content.trim_start_matches('\u{feff}').lines();
    if lines.next().map(str::trim)!=Some("---"){return Err("等待完整的 Skill 声明".into());}
    let mut yaml=String::new();let mut closed=false;
    for line in lines.by_ref(){if matches!(line.trim(),"---"|"..."){closed=true;break;}yaml.push_str(line);yaml.push('\n');}
    if !closed{return Err("等待 Skill 声明保存完成".into());}
    let declaration:Value=serde_yaml::from_str(&yaml).map_err(|_|"等待有效的 Skill 声明")?;
    if !["name","description"].iter().all(|field|declaration[field].as_str().is_some_and(|value|!value.trim().is_empty())) || !lines.any(|line|!line.trim().is_empty()) {return Err("等待名称、用途及正文保存完成".into());}
    Ok(())
}

/// Existing folders are recognized by identity before opening SKILL.md or
/// hashing bytes. A no-change round only enumerates directory names.
pub(crate) fn discover(manager: &mut Manager, request: &Value) -> Result<Discovery, String> {
    let root_ids: Option<BTreeSet<String>> = request.get("rootIds").map(|_| strings(request,"rootIds").map(|values|values.into_iter().collect())).transpose()?;
    let changed: Vec<PathBuf> = request["changedPaths"].as_array().into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from).collect();
    let mut result=Discovery::default();
    let mut candidates=BTreeSet::new();
    for source in &manager.state.local_roots {
        if source["enabled"] == false || root_ids.as_ref().is_some_and(|ids| !source["id"].as_str().is_some_and(|id|ids.contains(id))) { continue; }
        let Some(path)=source["path"].as_str() else {continue;};
        let root=match local::canonical(Path::new(path)) {Ok(root)=>root,Err(error)=>{result.errors.push(error);continue;}};
        if source["resolvedPath"].as_str().is_some_and(|path|path_key(Path::new(path))!=path_key(&root)) {
            result.errors.push(format!("目录的真实目标已改变，请重新添加：{path}")); continue;
        }
        if changed.is_empty() { visit(&root,0,&mut candidates,&mut result.errors); }
        else {
            for event_path in &changed {
                let event=local::canonical(event_path).unwrap_or_else(|_|event_path.clone());
                if !event.starts_with(&root) {continue;}
                // Unknown file events can only create a Skill through its
                // entry file. Do not rescan a whole source for an unrelated
                // download, database WAL, or cache write beneath the source.
                let folder=if event.is_dir(){event}else if event.file_name().is_some_and(|name|name.to_string_lossy().eq_ignore_ascii_case("SKILL.md")){event.parent().unwrap_or(&root).to_owned()}else{continue;};
                visit(&folder,0,&mut candidates,&mut result.errors);
            }
        }
    }
    let mut known:BTreeSet<String>=manager.state.skills.iter().filter_map(|skill|skill["localPath"].as_str()).map(|path|local::canonical(Path::new(path)).map(|path|path_key(&path)).unwrap_or_else(|_|path_key(Path::new(path)))).collect();
    let ignored:BTreeSet<_>=manager.state.ignored_local_paths.iter().map(|path|path_key(Path::new(path))).collect();
    for path in candidates {
        let path=match local::canonical(&path){Ok(path)=>path,Err(error)=>{result.errors.push(error);continue;}};
        let key=path_key(&path);
        if known.contains(&key)||ignored.contains(&key){continue;}
        if let Err(error)=registration_ready(&path){result.errors.push(format!("Skill 尚未写入完整 {}：{error}",path.display()));continue;}
        match local::draft(&path) {
            Ok(mut skill) if skill["readOnly"]!=true && skill["managed"]!=true => {
                let id=manager.id();skill["id"]=json!(id);crate::audit::refresh_local_audit(&mut skill,None);
                manager.upsert_skill(skill);known.insert(key);result.imported+=1;result.changed_ids.push(id);
            },
            Ok(_)=>{},
            Err(error)=>result.errors.push(format!("Skill 尚未写入完整 {}：{error}",path.display())),
        }
    }
    Ok(result)
}

fn persist(manager:&mut Manager, previous:State)->Result<Value,String>{
    if serde_json::to_value(&manager.state).ok()!=serde_json::to_value(&previous).ok() {
        if let Err(error)=manager.save(){manager.state=previous;return Err(error);}
    }
    Ok(manager.snapshot())
}

pub(crate) fn handle(manager:&mut Manager,request:&Value)->Result<Option<Value>,String>{
    let action=request["action"].as_str().unwrap_or_default();
    if !matches!(action,"set_check_mode"|"add_local_roots"|"update_local_root"|"remove_local_roots"|"restore_local_ignores"|"check_now"|"discover_local"){return Ok(None);}
    let previous=manager.state.clone();
    if action=="discover_local" {
        if request["automatic"]==true && (manager.state.check_mode!="auto" || request["checkGeneration"].as_u64()!=Some(manager.state.check_generation)){return Err("自动检查设置已改变，本次旧检查已停止".into());}
        let discovery=discover(manager,request)?;
        let previous_errors=previous.check_summary["rootErrors"].as_array().cloned().unwrap_or_default();
        let current_errors:Vec<Value>=discovery.errors.iter().map(|error|json!(error)).collect();
        let changed=discovery.imported>0||previous_errors!=current_errors;
        let summary=json!({"newLocal":discovery.imported,"newRemote":0,"associated":0,"needsAttention":0,"errors":discovery.errors.len(),"rootErrors":discovery.errors,"changed":changed,"checkedAt":now()});
        if changed{manager.state.check_summary=summary.clone();}
        let mut value=persist(manager,previous)?;value["checkSummary"]=summary;value["discoveredSkillIds"]=json!(discovery.changed_ids);return Ok(Some(value));
    }
    match action {
        "set_check_mode"=>{
            let mode=request["mode"].as_str().filter(|mode|matches!(*mode,"auto"|"manual")).ok_or("请选择自动检查或手动检查")?;
            if manager.state.check_mode!=mode {manager.state.check_mode=mode.into();manager.state.check_generation+=1;}
        },
        "add_local_roots"=>{
            for path in strings(request,"paths")? {
                let canonical=local::canonical(Path::new(&path))?;
                if !canonical.is_dir(){return Err("请选择本地目录".into());}
                let key=path_key(&canonical);
                if !manager.state.local_roots.iter().any(|root|root["resolvedPath"].as_str().is_some_and(|path|path_key(Path::new(path))==key)) {
                    let name=canonical.file_name().unwrap_or_default().to_string_lossy();
                    manager.state.local_roots.push(json!({"id":manager.id(),"name":name,"path":path,"resolvedPath":canonical.to_string_lossy(),"enabled":true}));manager.state.check_generation+=1;
                }
            }
            if request.get("excludedPaths").is_some() {
                for path in strings(request,"excludedPaths")? {
                    let canonical=local::canonical(Path::new(&path))?;
                    if !manager.state.local_roots.iter().any(|root|root["resolvedPath"].as_str().is_some_and(|root|canonical.starts_with(root))) {return Err("忽略项目必须属于已选择目录".into());}
                    let key=path_key(&canonical);
                    if !manager.state.ignored_local_paths.iter().any(|path|path_key(Path::new(path))==key){manager.state.ignored_local_paths.push(canonical.to_string_lossy().into_owned());manager.state.check_generation+=1;}
                }
            }
            if request.get("selectedPaths").is_some() {
                for path in strings(request,"selectedPaths")? {
                    let canonical=local::canonical(Path::new(&path))?;
                    if !manager.state.local_roots.iter().any(|root|root["resolvedPath"].as_str().is_some_and(|root|canonical.starts_with(root))) {return Err("所选 Skill 必须属于已选择目录".into());}
                    let key=path_key(&canonical);
                    let old=manager.state.ignored_local_paths.len();
                    manager.state.ignored_local_paths.retain(|path|path_key(Path::new(path))!=key);
                    if old!=manager.state.ignored_local_paths.len(){manager.state.check_generation+=1;}
                    if !manager.state.skills.iter().filter_map(|skill|skill["localPath"].as_str()).any(|path|local::canonical(Path::new(path)).is_ok_and(|path|path_key(&path)==key)) {
                        let mut skill=local::draft(&canonical)?;skill["id"]=json!(manager.id());crate::audit::refresh_local_audit(&mut skill,None);manager.upsert_skill(skill);
                    }
                }
            }
        },
        "update_local_root"=>{
            let id=request["rootId"].as_str().ok_or("缺少目录标识")?;
            let root=manager.state.local_roots.iter_mut().find(|root|root["id"]==id).ok_or("该目录已移除")?;
            let before=root.clone();
            if let Some(name)=request.get("name"){let name=name.as_str().map(str::trim).filter(|name|!name.is_empty()&&name.chars().count()<=80).ok_or("目录名称应为1至80个字")?;root["name"]=json!(name);}
            if let Some(enabled)=request.get("enabled"){if !enabled.is_boolean(){return Err("无效的目录开关".into());}root["enabled"]=enabled.clone();}
            if *root!=before{manager.state.check_generation+=1;}
        },
        "remove_local_roots"=>{let ids=strings(request,"rootIds")?;manager.state.local_roots.retain(|root|!root["id"].as_str().is_some_and(|id|ids.iter().any(|value|value==id)));if manager.state.local_roots!=previous.local_roots{manager.state.check_generation+=1;}},
        "restore_local_ignores"=>{let paths=strings(request,"paths")?;manager.state.ignored_local_paths.retain(|path|!paths.iter().any(|value|path_key(Path::new(value))==path_key(Path::new(path))));if manager.state.ignored_local_paths!=previous.ignored_local_paths{manager.state.check_generation+=1;}},
        "check_now"=>{
            if request["automatic"]==true && (manager.state.check_mode!="auto" || request["checkGeneration"].as_u64()!=Some(manager.state.check_generation)){return Err("自动检查设置已改变，本次旧检查已停止".into());}
            let selected_only=request["automatic"]!=true && (request["skillIds"].is_array()||request["skillId"].is_string()) && !request["rootIds"].is_array();
            let discovery=if request["skipLocalDiscovery"]==true || request["remoteOnly"]==true || selected_only {Discovery::default()} else {discover(manager,request)?};
            let mut remote_request=request.clone();
            if request["rootIds"].is_array() && !remote_request["skillIds"].is_array() {
                let ids=strings(request,"rootIds")?;
                let roots:Vec<_>=manager.state.local_roots.iter().filter(|root|root["id"].as_str().is_some_and(|id|ids.iter().any(|value|value==id))).filter_map(|root|root["resolvedPath"].as_str()).collect();
                let selected:Vec<_>=manager.state.skills.iter().filter(|skill|skill["localPath"].as_str().is_some_and(|path|{
                    let path=path_key(Path::new(path));let separator=if cfg!(windows){"\\"}else{"/"};roots.iter().any(|root|{let root=path_key(Path::new(root));path==root||path.starts_with(&(root+separator))})
                })).map(|skill|skill["id"].clone()).collect();
                remote_request["skillIds"]=json!(selected);remote_request["discoveryOnly"]=json!(true);
            }
            if let Some(ids)=remote_request["skillIds"].as_array_mut(){for id in &discovery.changed_ids{if !ids.iter().any(|value|value==id){ids.push(json!(id));}}}
            if request["discoveryOnly"]==true && !remote_request["skillIds"].is_array(){remote_request["skillIds"]=json!(discovery.changed_ids);}
            // Discovery is visible immediately; slow/offline GitHub must never
            // postpone registration of an already valid local Skill.
            if discovery.imported>0 {manager.save()?;}
            let mut value=if remote_request["discoveryOnly"]==true && remote_request["skillIds"].as_array().is_some_and(Vec::is_empty){manager.snapshot()}else{crate::remote::check(manager,&remote_request)?};
            let mut summary=value["checkSummary"].as_object().cloned().unwrap_or_default();
            summary.insert("imported".into(),json!(discovery.imported));
            summary.insert("newLocal".into(),json!(discovery.imported));
            let remote_errors=summary.get("errors").and_then(Value::as_u64).unwrap_or(0);
            summary.insert("errors".into(),json!(remote_errors+discovery.errors.len() as u64));
            summary.insert("rootErrors".into(),json!(discovery.errors));
            let previous_errors=previous.check_summary["rootErrors"].as_array().cloned().unwrap_or_default();
            let current_errors:Vec<Value>=discovery.errors.iter().map(|error|json!(error)).collect();
            let changed=manager.state.skills!=previous.skills || manager.state.sources!=previous.sources || previous_errors!=current_errors;
            summary.insert("changed".into(),json!(changed));
            if changed {summary.insert("checkedAt".into(),json!(now()));manager.state.check_summary=json!(summary);}
            persist(manager,previous)?;
            value=manager.snapshot();value["checkSummary"]=json!(summary);return Ok(Some(value));
        },
        _=>unreachable!(),
    }
    Ok(Some(persist(manager,previous)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture()->tempfile::TempDir{let parent=Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("work/local-root-tests");fs::create_dir_all(&parent).unwrap();tempfile::tempdir_in(parent).unwrap()}
    fn skill(path:&Path,name:&str){fs::create_dir_all(path).unwrap();fs::write(path.join("SKILL.md"),format!("---\nname: {name}\ndescription: synthetic\n---\nUse it.\n")).unwrap();}
    #[test]fn roots_discover_new_in_place_and_preserve_ignored_and_overlapping_sources(){
        let fixture=fixture();let root=fixture.path().join("skills");skill(&root.join("one"),"one");let mut manager=Manager::new(fixture.path().join("state")).unwrap();
        handle(&mut manager,&json!({"action":"add_local_roots","paths":[root]})).unwrap();let first=discover(&mut manager,&json!({})).unwrap();assert_eq!(first.imported,1);assert_eq!(manager.state.skills.len(),1);
        let nested=root.join("nested");fs::create_dir_all(&nested).unwrap();handle(&mut manager,&json!({"action":"add_local_roots","paths":[nested]})).unwrap();skill(&nested.join("two"),"two");assert_eq!(discover(&mut manager,&json!({})).unwrap().imported,1);assert_eq!(discover(&mut manager,&json!({})).unwrap().imported,0);
        let omitted=root.join("omitted");skill(&omitted,"omitted");manager.state.ignored_local_paths.push(local::canonical(&omitted).unwrap().to_string_lossy().into_owned());assert_eq!(discover(&mut manager,&json!({})).unwrap().imported,0);assert_eq!(manager.state.skills.len(),2);
        handle(&mut manager,&json!({"action":"restore_local_ignores","paths":[omitted]})).unwrap();assert_eq!(discover(&mut manager,&json!({})).unwrap().imported,1);assert_eq!(manager.state.skills.len(),3);
    }
    #[test]fn pause_remove_unavailable_and_system_sources_never_remove_registered_files(){
        let fixture=fixture();let root=fixture.path().join("skills");skill(&root.join("one"),"one");skill(&root.join(".system/internal"),"internal");let mut manager=Manager::new(fixture.path().join("state")).unwrap();handle(&mut manager,&json!({"action":"add_local_roots","paths":[root]})).unwrap();assert_eq!(discover(&mut manager,&json!({})).unwrap().imported,1);
        let id=manager.state.local_roots[0]["id"].clone();handle(&mut manager,&json!({"action":"update_local_root","rootId":id,"enabled":false})).unwrap();skill(&root.join("two"),"two");assert_eq!(discover(&mut manager,&json!({})).unwrap().imported,0);
        handle(&mut manager,&json!({"action":"update_local_root","rootId":id,"enabled":true})).unwrap();fs::rename(&root,fixture.path().join("offline")).unwrap();assert!(!discover(&mut manager,&json!({})).unwrap().errors.is_empty());assert_eq!(manager.state.skills.len(),1);
        handle(&mut manager,&json!({"action":"remove_local_roots","rootIds":[id]})).unwrap();assert_eq!(manager.state.skills.len(),1);assert!(fixture.path().join("offline/one/SKILL.md").exists());
    }
    #[test]fn old_states_get_default_mode_and_manual_mode_survives_restart(){
        let fixture=fixture();let mut manager=Manager::new(fixture.path().join("state")).unwrap();assert_eq!(manager.state.check_mode,"auto");handle(&mut manager,&json!({"action":"set_check_mode","mode":"manual"})).unwrap();let reopened=Manager::new(fixture.path().join("state")).unwrap();assert_eq!(reopened.state.check_mode,"manual");assert_eq!(reopened.state.check_generation,1);
    }
    #[test]fn automatic_discovery_waits_for_complete_new_skill_declarations(){
        let fixture=fixture();let root=fixture.path().join("skills");let folder=root.join("new");fs::create_dir_all(&folder).unwrap();fs::write(folder.join("SKILL.md"),"---\nname: new\n").unwrap();let mut manager=Manager::new(fixture.path().join("state")).unwrap();handle(&mut manager,&json!({"action":"add_local_roots","paths":[root]})).unwrap();assert_eq!(discover(&mut manager,&json!({})).unwrap().imported,0);fs::write(folder.join("SKILL.md"),"").unwrap();assert_eq!(discover(&mut manager,&json!({})).unwrap().imported,0);skill(&folder,"new");assert_eq!(discover(&mut manager,&json!({})).unwrap().imported,1);
    }
}
