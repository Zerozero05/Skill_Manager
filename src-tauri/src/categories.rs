use crate::model::{Manager, State};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const COLORS: [&str; 8] = [
    "blue", "teal", "green", "purple", "orange", "rose", "indigo", "slate",
];

fn requested_color(request: &Value) -> Result<Option<&str>, String> {
    request
        .get("color")
        .map(|value| {
            value
                .as_str()
                .filter(|color| COLORS.contains(color))
                .ok_or_else(|| "Choose one of the eight category colors".into())
        })
        .transpose()
}

// Same read-only identity ordering as categories.ts. Never repair old data on startup.
fn category_colors(list: &[Value]) -> BTreeMap<String, String> {
    let mut result: BTreeMap<String, String> = list
        .iter()
        .filter_map(|category| {
            let color = category["color"]
                .as_str()
                .filter(|color| COLORS.contains(color))?;
            Some((category["id"].as_str()?.to_owned(), color.to_owned()))
        })
        .collect();
    let mut used: BTreeSet<String> = result.values().cloned().collect();
    let mut ids: Vec<&str> = list
        .iter()
        .filter_map(|category| category["id"].as_str())
        .collect();
    ids.sort_unstable();
    let mut overflow = 0;
    for id in ids {
        if result.contains_key(id) {
            continue;
        }
        let color = if let Some(color) = COLORS.iter().find(|color| !used.contains(**color)) {
            *color
        } else {
            let color = COLORS[overflow % COLORS.len()];
            overflow += 1;
            color
        };
        result.insert(id.to_owned(), color.to_owned());
        used.insert(color.to_owned());
    }
    result
}

fn next_color(prior: &BTreeMap<String, String>) -> &'static str {
    COLORS
        .iter()
        .find(|color| !prior.values().any(|value| value == **color))
        .copied()
        .unwrap_or(COLORS[prior.len() % COLORS.len()])
}

fn materialize_colors(list: &mut [Value], prior: &BTreeMap<String, String>) {
    for category in list {
        // Preserve unknown future color values and every unrelated field.
        if category.get("color").is_some_and(|value| !value.is_null()) {
            continue;
        }
        if let Some(color) = category["id"].as_str().and_then(|id| prior.get(id)) {
            category["color"] = json!(color);
        }
    }
}

fn categories(state: &State) -> Result<Vec<Value>, String> {
    match state.settings.get("categories") {
        None => Ok(vec![]),
        Some(value) => value
            .as_array()
            .cloned()
            .ok_or_else(|| "Saved category list is invalid".into()),
    }
}

fn text<'a>(request: &'a Value, name: &str) -> Result<&'a str, String> {
    request[name]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Missing {name}"))
}

fn category_id(value: Option<&Value>, categories: &[Value]) -> Result<Value, String> {
    match value {
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(id)) if categories.iter().any(|category| category["id"] == *id) => {
            Ok(json!(id))
        }
        _ => Err("Choose an existing category or 未分类".into()),
    }
}

fn category_index(categories: &[Value], id: &str) -> Result<usize, String> {
    categories
        .iter()
        .position(|category| category["id"] == id)
        .ok_or_else(|| "Category no longer exists".into())
}

fn category_name(
    request: &Value,
    categories: &[Value],
    except: Option<&str>,
) -> Result<String, String> {
    let name = text(request, "name")?.trim();
    if name.is_empty() {
        return Err("Category name must not be empty".into());
    }
    let folded = name.to_lowercase();
    if ["全部 skills", "未分类", "收藏"].contains(&folded.as_str()) {
        return Err("This category name is reserved".into());
    }
    if categories.iter().any(|category| {
        category["id"].as_str() != except
            && category["name"]
                .as_str()
                .is_some_and(|existing| existing.to_lowercase() == folded)
    }) {
        return Err("A category with this name already exists".into());
    }
    Ok(name.to_owned())
}

fn remember(m: &mut Manager, before: &State, action: &str) {
    let mut changes = Vec::new();
    for prior in &before.skills {
        let Some(after) = m
            .state
            .skills
            .iter()
            .find(|skill| skill["id"] == prior["id"])
        else {
            continue;
        };
        for field in ["categoryId", "favorite"] {
            if prior.get(field) != after.get(field) {
                changes.push(json!({"skillId":prior["id"],"field":field,
                    "before":prior[field],"beforePresent":prior.get(field).is_some(),
                    "after":after[field],"afterPresent":after.get(field).is_some()}));
            }
        }
    }
    let prior_categories = categories(before).unwrap_or_default();
    let next_categories = categories(&m.state).unwrap_or_default();
    let category_change = (prior_categories != next_categories)
        .then(|| json!({"before":prior_categories,"after":next_categories}));
    if changes.is_empty() && category_change.is_none() {
        return;
    }
    let label = match action {
        "create_category" => "创建分类",
        "rename_category" => "重命名分类",
        "delete_category" => "移除分类",
        "reorder_categories" => "调整分类顺序",
        "set_category_color" => "更改分类颜色",
        "set_favorite" => "更改收藏",
        "restore_category_assignments" => "撤销归类",
        _ => "归类 Skill",
    };
    m.management_undo
        .push(json!({"label":label,"categories":category_change,"changes":changes}));
    if m.management_undo.len() > 20 {
        m.management_undo.remove(0);
    }
}

fn undo(m: &mut Manager) -> Result<Value, String> {
    let entry = m
        .management_undo
        .last()
        .cloned()
        .ok_or("没有可撤销的分类或收藏操作")?;
    let mut next = m.state.clone();
    if !entry["categories"].is_null() {
        if json!(categories(&next)?) != entry["categories"]["after"] {
            return Err("分类已发生其他变化，无法安全撤销；现有记录已保留".into());
        }
        next.settings["categories"] = entry["categories"]["before"].clone();
    }
    for change in entry["changes"].as_array().ok_or("Invalid undo record")? {
        let skill = next
            .skills
            .iter_mut()
            .find(|skill| skill["id"] == change["skillId"])
            .ok_or("Skill记录已变化，无法安全撤销；现有记录已保留")?;
        let field = text(change, "field")?;
        let expected = (change["afterPresent"] == true).then_some(&change["after"]);
        if skill.get(field) != expected {
            return Err("分类或收藏已发生其他变化，无法安全撤销；现有记录已保留".into());
        }
        let object = skill.as_object_mut().ok_or("Invalid Skill record")?;
        if change["beforePresent"] == true {
            object.insert(field.into(), change["before"].clone());
        } else {
            object.remove(field);
        }
    }
    let restored_categories = categories(&next)?;
    if next.skills.iter().any(|skill| {
        skill["categoryId"].as_str().is_some_and(|id| {
            !restored_categories
                .iter()
                .any(|category| category["id"] == id)
        })
    }) {
        return Err("分类中已有新的成员，无法安全撤销；请先核对归属".into());
    }
    let previous = std::mem::replace(&mut m.state, next);
    if let Err(error) = m.save() {
        m.state = previous;
        return Err(error);
    }
    m.management_undo.pop();
    Ok(m.snapshot())
}

pub fn handle(m: &mut Manager, request: &Value) -> Result<Option<Value>, String> {
    let action = request["action"].as_str().unwrap_or_default();
    if action == "undo_management" {
        return undo(m).map(Some);
    }
    if !matches!(
        action,
        "create_category"
            | "rename_category"
            | "delete_category"
            | "reorder_categories"
            | "set_category_color"
            | "assign_category"
            | "restore_category_assignments"
            | "set_favorite"
    ) {
        return Ok(None);
    }
    // Validate and change a draft; rejected batches and failed saves cannot leave partial state.
    let mut next = m.state.clone();
    let mut list = categories(&next)?;
    let prior_list = list.clone();
    let prior_colors = category_colors(&prior_list);
    match action {
        "set_favorite" => {
            let id = text(request, "skillId")?;
            let favorite = request["favorite"]
                .as_bool()
                .ok_or("favorite must be true or false")?;
            let skill = next
                .skills
                .iter_mut()
                .find(|skill| skill["id"] == id)
                .ok_or("Skill no longer exists")?;
            skill["favorite"] = json!(favorite);
        }
        "create_category" => {
            let name = category_name(request, &list, None)?;
            let color = requested_color(request)?.unwrap_or_else(|| next_color(&prior_colors));
            list.push(json!({"id":m.id(),"name":name,"color":color}));
        }
        "rename_category" => {
            let id = text(request, "categoryId")?;
            let index = category_index(&list, id)?;
            let name = category_name(request, &list, Some(id))?;
            list[index]["name"] = json!(name);
            if let Some(color) = requested_color(request)? {
                list[index]["color"] = json!(color);
            }
        }
        "set_category_color" => {
            let index = category_index(&list, text(request, "categoryId")?)?;
            let color = requested_color(request)?.ok_or("Missing color")?;
            list[index]["color"] = json!(color);
        }
        "delete_category" => {
            let id = text(request, "categoryId")?;
            let index = category_index(&list, id)?;
            list.remove(index);
            for skill in &mut next.skills {
                if skill["categoryId"] == id {
                    skill["categoryId"] = Value::Null;
                }
            }
        }
        "reorder_categories" => {
            let ids = request["categoryIds"]
                .as_array()
                .ok_or("categoryIds must contain the complete category order")?;
            let mut seen = BTreeSet::new();
            let mut ordered = Vec::with_capacity(ids.len());
            for id in ids {
                let id = id.as_str().ok_or("Invalid category ID")?;
                if !seen.insert(id) {
                    return Err("Category order contains a duplicate".into());
                }
                ordered.push(list[category_index(&list, id)?].clone());
            }
            if ordered.len() != list.len() {
                return Err("categoryIds must contain every category exactly once".into());
            }
            list = ordered;
        }
        "assign_category" => {
            let category = category_id(request.get("categoryId"), &list)?;
            let ids = m
                .refresh_selection(&json!({"skillIds":request["skillIds"]}))?
                .ok_or("Select at least one Skill")?;
            for skill in &mut next.skills {
                if skill["id"].as_str().is_some_and(|id| ids.contains(id)) {
                    skill["categoryId"] = category.clone();
                }
            }
        }
        "restore_category_assignments" => {
            let assignments = request["assignments"]
                .as_array()
                .filter(|values| !values.is_empty())
                .ok_or("Choose category assignments to restore")?;
            let mut seen = BTreeSet::new();
            for assignment in assignments {
                let id = text(assignment, "skillId")?;
                if !seen.insert(id) {
                    return Err("Category assignments contain a duplicate Skill".into());
                }
                let category = category_id(assignment.get("categoryId"), &list)?;
                let skill = next
                    .skills
                    .iter_mut()
                    .find(|skill| skill["id"] == id)
                    .ok_or("Skill no longer exists")?;
                skill["categoryId"] = category;
            }
        }
        _ => unreachable!(),
    }
    if list != prior_list {
        // Record materialization in this operation's before/after pair. Undo restores
        // exact old objects before walking the pre-color history, rather than clearing it.
        materialize_colors(&mut list, &prior_colors);
        next.settings["categories"] = json!(list);
    }
    let previous = std::mem::replace(&mut m.state, next);
    if let Err(error) = m.save() {
        m.state = previous;
        return Err(error);
    }
    remember(m, &previous, action);
    Ok(Some(m.snapshot()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Manager) {
        let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/categories-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let dir = tempfile::tempdir_in(parent).unwrap();
        let mut m = Manager::new(dir.path().to_path_buf()).unwrap();
        m.state.skills = vec![
            json!({"id":"local","localPath":"D:/synthetic/a","runtimeFiles":["SKILL.md"]}),
            json!({"id":"remote","sourceId":"source","remotePath":"a","localPath":null}),
            json!({"id":"linked","sourceId":"source","localPath":"D:/synthetic/b","readOnly":true}),
        ];
        m.state.connections = vec![json!({"skillId":"local","id":"connection"})];
        m.state.sources = vec![json!({"id":"source","repo":"synthetic/repo"})];
        (dir, m)
    }

    fn create(m: &mut Manager, name: &str) -> String {
        handle(m, &json!({"action":"create_category","name":name})).unwrap();
        m.state.settings["categories"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn state(m: &Manager) -> Value {
        serde_json::to_value(&m.state).unwrap()
    }

    #[test]
    fn favorites_validate_atomically_preserve_metadata_and_survive_restart() {
        let (dir, mut m) = fixture();
        let category = create(&mut m, "Research");
        handle(
            &mut m,
            &json!({"action":"assign_category","skillIds":["local"],"categoryId":category}),
        )
        .unwrap();
        let original = state(&m);
        assert!(m.skill("local").unwrap()["favorite"].is_null());
        for request in [
            json!({"action":"set_favorite","skillId":"local"}),
            json!({"action":"set_favorite","skillId":"local","favorite":null}),
            json!({"action":"set_favorite","skillId":"local","favorite":1}),
            json!({"action":"set_favorite","skillId":"local","favorite":"true"}),
            json!({"action":"set_favorite","skillId":"missing","favorite":true}),
            json!({"action":"set_favorite","favorite":true}),
        ] {
            assert!(handle(&mut m, &request).is_err());
            assert_eq!(state(&m), original);
        }
        let snapshot = handle(
            &mut m,
            &json!({"action":"set_favorite","skillId":"local","favorite":true}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(snapshot["skills"][0]["favorite"], true);
        assert_eq!(snapshot["skills"][0]["categoryId"], category);
        assert_eq!(snapshot["connections"], original["connections"]);
        assert_eq!(snapshot["sources"], original["sources"]);
        drop(m);
        let mut restored = Manager::new(dir.path().to_path_buf()).unwrap();
        assert_eq!(restored.skill("local").unwrap()["favorite"], true);
        handle(
            &mut restored,
            &json!({"action":"set_favorite","skillId":"local","favorite":false}),
        )
        .unwrap();
        let mut after = state(&restored);
        after["skills"][0]
            .as_object_mut()
            .unwrap()
            .remove("favorite");
        assert_eq!(after, original);
    }

    #[test]
    fn category_names_are_trimmed_unique_and_renaming_keeps_identity() {
        let (_dir, mut m) = fixture();
        let id = create(&mut m, "  Research  ");
        assert_eq!(m.state.settings["categories"][0]["name"], "Research");
        let before = state(&m);
        for name in [
            "",
            " \t\n",
            "research",
            " RESEARCH ",
            "全部 Skills",
            "全部 SKILLS",
            "未分类",
            "收藏",
        ] {
            assert!(handle(&mut m, &json!({"action":"create_category","name":name})).is_err());
            assert_eq!(state(&m), before);
        }
        let other = create(&mut m, "Work");
        let before = state(&m);
        assert!(handle(
            &mut m,
            &json!({"action":"rename_category","categoryId":id,"name":"work"})
        )
        .is_err());
        assert_eq!(state(&m), before);
        handle(
            &mut m,
            &json!({"action":"rename_category","categoryId":id,"name":" RESEARCH "}),
        )
        .unwrap();
        assert_eq!(
            m.state.settings["categories"][0],
            json!({"id":id,"name":"RESEARCH","color":"blue"})
        );
        assert_eq!(m.state.settings["categories"][1]["id"], other);
    }

    #[test]
    fn category_order_requires_the_full_unique_list_and_survives_restart() {
        let (dir, mut m) = fixture();
        let a = create(&mut m, "A");
        let b = create(&mut m, "B");
        let before = state(&m);
        for ids in [
            json!([]),
            json!([a]),
            json!([a, a]),
            json!([a, "missing"]),
            json!("invalid"),
        ] {
            assert!(handle(
                &mut m,
                &json!({"action":"reorder_categories","categoryIds":ids})
            )
            .is_err());
            assert_eq!(state(&m), before);
        }
        handle(
            &mut m,
            &json!({"action":"reorder_categories","categoryIds":[b,a]}),
        )
        .unwrap();
        drop(m);
        let restored = Manager::new(dir.path().to_path_buf()).unwrap();
        assert_eq!(restored.state.settings["categories"][0]["id"], b);
        assert_eq!(restored.state.settings["categories"][1]["id"], a);
    }

    #[test]
    fn batches_validate_all_members_and_categories_before_applying_and_undo_is_atomic() {
        let (_dir, mut m) = fixture();
        let id = create(&mut m, "Research");
        let other = create(&mut m, "Work");
        let original = state(&m);
        for request in [
            json!({"action":"assign_category","skillIds":["local","missing"],"categoryId":id}),
            json!({"action":"assign_category","skillIds":[],"categoryId":id}),
            json!({"action":"assign_category","skillIds":["local"],"categoryId":"missing"}),
            json!({"action":"assign_category","skillIds":["local"]}),
        ] {
            assert!(handle(&mut m, &request).is_err());
            assert_eq!(state(&m), original);
        }
        handle(&mut m, &json!({"action":"assign_category","skillIds":["local","remote","local"],"categoryId":id})).unwrap();
        handle(
            &mut m,
            &json!({"action":"assign_category","skillIds":["linked"],"categoryId":other}),
        )
        .unwrap();
        let assigned = state(&m);
        for assignments in [
            json!([{"skillId":"local","categoryId":null},{"skillId":"missing","categoryId":null}]),
            json!([{"skillId":"local","categoryId":null},{"skillId":"remote","categoryId":"missing"}]),
            json!([{"skillId":"local","categoryId":null},{"skillId":"local","categoryId":other}]),
        ] {
            assert!(handle(
                &mut m,
                &json!({"action":"restore_category_assignments","assignments":assignments})
            )
            .is_err());
            assert_eq!(state(&m), assigned);
        }
        handle(
            &mut m,
            &json!({"action":"restore_category_assignments","assignments":[
                {"skillId":"local","categoryId":null},{"skillId":"remote","categoryId":other}
            ]}),
        )
        .unwrap();
        assert!(m.skill("local").unwrap()["categoryId"].is_null());
        assert_eq!(m.skill("remote").unwrap()["categoryId"], other);
        assert_eq!(m.skill("linked").unwrap()["categoryId"], other);
        assert_eq!(
            m.state.sources,
            original["sources"].as_array().unwrap().clone()
        );
        assert_eq!(
            m.state.connections,
            original["connections"].as_array().unwrap().clone()
        );
        for skill in &mut m.state.skills {
            skill.as_object_mut().unwrap().remove("categoryId");
        }
        assert_eq!(
            m.state.skills,
            original["skills"].as_array().unwrap().clone()
        );
    }

    #[test]
    fn deleting_a_category_only_clears_its_membership_and_preserves_skills_and_connections() {
        let (dir, mut m) = fixture();
        let a = create(&mut m, "A");
        let b = create(&mut m, "B");
        handle(
            &mut m,
            &json!({"action":"assign_category","skillIds":["local","remote"],"categoryId":a}),
        )
        .unwrap();
        handle(
            &mut m,
            &json!({"action":"assign_category","skillIds":["linked"],"categoryId":b}),
        )
        .unwrap();
        let before = state(&m);
        handle(&mut m, &json!({"action":"delete_category","categoryId":a})).unwrap();
        assert_eq!(
            m.state.settings["categories"],
            json!([{"id":b,"name":"B","color":"teal"}])
        );
        assert!(m.skill("local").unwrap()["categoryId"].is_null());
        assert!(m.skill("remote").unwrap()["categoryId"].is_null());
        assert_eq!(m.skill("linked").unwrap()["categoryId"], b);
        assert_eq!(m.state.skills.len(), 3);
        assert_eq!(
            m.state.sources,
            before["sources"].as_array().unwrap().clone()
        );
        assert_eq!(
            m.state.connections,
            before["connections"].as_array().unwrap().clone()
        );
        let after = state(&m);
        assert!(handle(&mut m, &json!({"action":"delete_category","categoryId":a})).is_err());
        assert_eq!(state(&m), after);
        drop(m);
        assert_eq!(
            state(&Manager::new(dir.path().to_path_buf()).unwrap()),
            after
        );
    }

    #[test]
    fn legacy_database_without_category_or_layout_fields_keeps_its_data() {
        let (dir, mut m) = fixture();
        for field in ["categories", "sidebarWidth", "sidebarCollapsed"] {
            m.state.settings.as_object_mut().unwrap().remove(field);
        }
        m.save().unwrap();
        let old = state(&m);
        drop(m);
        let mut m = Manager::new(dir.path().to_path_buf()).unwrap();
        assert_eq!(state(&m), old);
        let id = create(&mut m, "New category");
        handle(
            &mut m,
            &json!({"action":"assign_category","skillIds":["remote"],"categoryId":id}),
        )
        .unwrap();
        assert_eq!(m.state.skills.len(), 3);
        assert_eq!(m.state.sources, old["sources"].as_array().unwrap().clone());
    }

    #[test]
    fn undo_walks_category_create_rename_order_membership_delete_and_favorite_in_reverse() {
        let (_dir, mut m) = fixture();
        let mut states = vec![state(&m)];
        let a = create(&mut m, "Research");
        states.push(state(&m));
        let b = create(&mut m, "Tools");
        states.push(state(&m));
        for request in [
            json!({"action":"assign_category","skillIds":["local","remote"],"categoryId":a}),
            json!({"action":"rename_category","categoryId":a,"name":"Papers"}),
            json!({"action":"reorder_categories","categoryIds":[b,a]}),
            json!({"action":"set_favorite","skillId":"local","favorite":true}),
            json!({"action":"delete_category","categoryId":a}),
        ] {
            handle(&mut m, &request).unwrap();
            states.push(state(&m));
        }
        states.pop();
        for expected in states.into_iter().rev() {
            handle(&mut m, &json!({"action":"undo_management"})).unwrap();
            assert_eq!(state(&m), expected);
        }
        assert_eq!(m.snapshot()["managementUndo"]["available"], false);
        assert!(handle(&mut m, &json!({"action":"undo_management"})).is_err());
    }

    #[test]
    fn undo_preserves_new_runtime_evidence_and_rejects_conflicting_metadata_without_popping() {
        let (_dir, mut m) = fixture();
        handle(
            &mut m,
            &json!({"action":"set_favorite","skillId":"local","favorite":true}),
        )
        .unwrap();
        m.state.skills[0]["lastLocalCheckedAt"] = json!("new-check-evidence");
        m.state.skills[0]["baselineCommit"] = json!("new-baseline");
        handle(&mut m, &json!({"action":"undo_management"})).unwrap();
        assert_eq!(
            m.state.skills[0]["lastLocalCheckedAt"],
            "new-check-evidence"
        );
        assert_eq!(m.state.skills[0]["baselineCommit"], "new-baseline");
        assert!(m.state.skills[0].get("favorite").is_none());
        handle(
            &mut m,
            &json!({"action":"set_favorite","skillId":"local","favorite":true}),
        )
        .unwrap();
        m.state.skills[0]["favorite"] = json!(false);
        let before = state(&m);
        let depth = m.management_undo.len();
        assert!(handle(&mut m, &json!({"action":"undo_management"})).is_err());
        assert_eq!(state(&m), before);
        assert_eq!(m.management_undo.len(), depth);
    }

    #[test]
    fn undo_is_bounded_skips_noops_and_session_history_is_not_persisted() {
        let (dir, mut m) = fixture();
        for index in 0..25 {
            handle(
                &mut m,
                &json!({"action":"set_favorite","skillId":"local","favorite":index%2==0}),
            )
            .unwrap();
        }
        assert_eq!(m.management_undo.len(), 20);
        handle(
            &mut m,
            &json!({"action":"set_favorite","skillId":"local","favorite":true}),
        )
        .unwrap();
        assert_eq!(m.management_undo.len(), 20);
        let restarted = Manager::new(dir.path().to_path_buf()).unwrap();
        assert!(restarted.management_undo.is_empty());
        assert_eq!(restarted.state.skills[0]["favorite"], true);
        for _ in 0..20 {
            handle(&mut m, &json!({"action":"undo_management"})).unwrap();
        }
        assert!(m.management_undo.is_empty());
    }

    #[test]
    fn undo_save_failure_retains_current_state_and_available_history_for_retry() {
        let (dir, mut m) = fixture();
        create(&mut m, "Research");
        let before = state(&m);
        let depth = m.management_undo.len();
        let db = rusqlite::Connection::open(dir.path().join("manager.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER reject_undo BEFORE INSERT ON app_state BEGIN SELECT RAISE(ABORT, 'synthetic undo save failure'); END;").unwrap();
        assert!(handle(&mut m, &json!({"action":"undo_management"})).is_err());
        assert_eq!(state(&m), before);
        assert_eq!(m.management_undo.len(), depth);
        db.execute_batch("DROP TRIGGER reject_undo").unwrap();
        handle(&mut m, &json!({"action":"undo_management"})).unwrap();
        assert!(categories(&m.state).unwrap().is_empty());
    }

    #[test]
    fn undo_cannot_orphan_a_new_member_registered_after_category_creation() {
        let (_dir, mut m) = fixture();
        let id = create(&mut m, "Research");
        m.state
            .skills
            .push(json!({"id":"new-member","categoryId":id}));
        let before = state(&m);
        assert!(handle(&mut m, &json!({"action":"undo_management"})).is_err());
        assert_eq!(state(&m), before);
    }

    #[test]
    fn database_save_failure_rolls_back_category_and_membership_in_memory_and_on_disk() {
        let (dir, mut m) = fixture();
        let id = create(&mut m, "Research");
        let before = state(&m);
        let db = rusqlite::Connection::open(dir.path().join("manager.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER prevent_insert BEFORE INSERT ON app_state BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;").unwrap();
        for request in [
            json!({"action":"create_category","name":"Work"}),
            json!({"action":"assign_category","skillIds":["local","remote"],"categoryId":id}),
            json!({"action":"delete_category","categoryId":id}),
            json!({"action":"set_favorite","skillId":"remote","favorite":true}),
        ] {
            assert!(handle(&mut m, &request).is_err());
            assert_eq!(state(&m), before);
        }
        drop(m);
        assert_eq!(
            state(&Manager::new(dir.path().to_path_buf()).unwrap()),
            before
        );
    }

    fn legacy_categories(m: &mut Manager) -> Vec<Value> {
        let list = vec![
            json!({"id":"e","name":"E","future":{"keep":[1,2]}}),
            json!({"id":"c","name":"C"}),
            json!({"id":"a","name":"A"}),
            json!({"id":"b","name":"B"}),
            json!({"id":"d","name":"D"}),
        ];
        m.state.settings["categories"] = json!(list);
        m.save().unwrap();
        list
    }

    #[test]
    fn legacy_color_defaults_are_read_only_and_category_edits_keep_other_colors_stable() {
        for request in [
            json!({"action":"rename_category","categoryId":"a","name":"Renamed"}),
            json!({"action":"create_category","name":"New"}),
            json!({"action":"reorder_categories","categoryIds":["a","b","c","d","e"]}),
            json!({"action":"delete_category","categoryId":"a"}),
        ] {
            let (_dir, mut m) = fixture();
            let legacy = legacy_categories(&mut m);
            let original = state(&m);
            let colors = category_colors(&legacy);
            assert_eq!(colors.values().collect::<BTreeSet<_>>().len(), 5);
            assert_eq!(state(&m), original);
            handle(&mut m, &request).unwrap();
            let changed = categories(&m.state).unwrap();
            for prior in &legacy {
                if let Some(category) = changed
                    .iter()
                    .find(|category| category["id"] == prior["id"])
                {
                    let id = prior["id"].as_str().unwrap();
                    assert_eq!(category["color"], colors[id]);
                    assert_eq!(category.get("future"), prior.get("future"));
                }
            }
            handle(&mut m, &json!({"action":"undo_management"})).unwrap();
            assert_eq!(state(&m), original);
        }
    }

    #[test]
    fn first_color_save_preserves_pre_color_undo_chain_and_unknown_extensions() {
        let (_dir, mut m) = fixture();
        let legacy = legacy_categories(&mut m);
        let mut renamed = legacy.clone();
        renamed[0]["name"] = json!("Old rename");
        m.state.settings["categories"] = json!(renamed);
        m.save().unwrap();
        // Exact legacy entry written before category colors existed.
        m.management_undo.push(json!({"label":"重命名分类","categories":{"before":legacy,"after":renamed},"changes":[]}));
        let before_favorite = state(&m);
        handle(
            &mut m,
            &json!({"action":"set_favorite","skillId":"local","favorite":true}),
        )
        .unwrap();
        let before_color = state(&m);
        assert_eq!(m.state.settings["categories"], json!(renamed));
        handle(
            &mut m,
            &json!({"action":"set_category_color","categoryId":"a","color":"rose"}),
        )
        .unwrap();
        assert_eq!(m.state.settings["categories"][2]["color"], "rose");
        assert!(categories(&m.state)
            .unwrap()
            .iter()
            .all(|category| COLORS.contains(&category["color"].as_str().unwrap())));
        assert_eq!(m.management_undo.last().unwrap()["label"], "更改分类颜色");
        handle(&mut m, &json!({"action":"undo_management"})).unwrap();
        assert_eq!(state(&m), before_color);
        handle(&mut m, &json!({"action":"undo_management"})).unwrap();
        assert_eq!(state(&m), before_favorite);
        handle(&mut m, &json!({"action":"undo_management"})).unwrap();
        assert_eq!(m.state.settings["categories"], json!(legacy));
        assert!(m.management_undo.is_empty());
    }

    #[test]
    fn color_validation_and_save_failure_leave_state_and_undo_unchanged() {
        let (dir, mut m) = fixture();
        legacy_categories(&mut m);
        let original = state(&m);
        for request in [
            json!({"action":"set_category_color","categoryId":"a","color":"unknown"}),
            json!({"action":"set_category_color","categoryId":"a","color":null}),
            json!({"action":"set_category_color","categoryId":"a"}),
            json!({"action":"set_category_color","categoryId":"missing","color":"blue"}),
            json!({"action":"create_category","name":"New","color":"unknown"}),
            json!({"action":"rename_category","categoryId":"a","name":"Renamed","color":1}),
        ] {
            assert!(handle(&mut m, &request).is_err());
            assert_eq!(state(&m), original);
            assert!(m.management_undo.is_empty());
        }
        handle(
            &mut m,
            &json!({"action":"rename_category","categoryId":"a","name":"A"}),
        )
        .unwrap();
        assert_eq!(state(&m), original);
        assert!(m.management_undo.is_empty());
        let db = rusqlite::Connection::open(dir.path().join("manager.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER reject_color BEFORE INSERT ON app_state BEGIN SELECT RAISE(ABORT, 'synthetic color save failure'); END;").unwrap();
        assert!(handle(
            &mut m,
            &json!({"action":"set_category_color","categoryId":"a","color":"rose"})
        )
        .is_err());
        assert_eq!(state(&m), original);
        assert!(m.management_undo.is_empty());
        db.execute_batch("DROP TRIGGER reject_color").unwrap();
        handle(
            &mut m,
            &json!({"action":"set_category_color","categoryId":"a","color":"rose"}),
        )
        .unwrap();
        let restarted = Manager::new(dir.path().to_path_buf()).unwrap();
        assert_eq!(restarted.state.settings["categories"][2]["color"], "rose");
    }

    #[test]
    fn explicit_and_future_colors_survive_unrelated_category_edits() {
        let (_dir, mut m) = fixture();
        m.state.settings["categories"] = json!([
            {"id":"a","name":"A","color":"green","future":{"note":true}},
            {"id":"b","name":"B","color":"future-color"},
            {"id":"c","name":"C"}
        ]);
        let before = category_colors(&categories(&m.state).unwrap());
        handle(
            &mut m,
            &json!({"action":"reorder_categories","categoryIds":["c","b","a"]}),
        )
        .unwrap();
        let list = categories(&m.state).unwrap();
        assert_eq!(list[0]["color"], before["c"]);
        assert_eq!(list[1]["color"], "future-color");
        assert_eq!(list[2]["color"], "green");
        assert_eq!(list[2]["future"], json!({"note":true}));
        assert_eq!(category_colors(&list), before);
        handle(
            &mut m,
            &json!({"action":"create_category","name":"Rose","color":"rose"}),
        )
        .unwrap();
        assert_eq!(
            m.state.settings["categories"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["color"],
            "rose"
        );
    }
}
