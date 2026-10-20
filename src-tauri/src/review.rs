//! Explicit review receipts reuse the existing session journal; no scheduling algorithm.
use crate::{content::ContentStore, sessions, types::*};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::Path,
};
fn states(store: &ContentStore) -> Result<BTreeMap<String, Value>> {
    let mut result: BTreeMap<String, Value> = BTreeMap::new();
    for summary in store
        .summaries()
        .into_iter()
        .filter(|item| item.kind == "entry")
    {
        let doc = store.read(&summary.id)?;
        let Some(data) = doc.metadata.extra.get("data") else {
            continue;
        };
        if data.get("origin").and_then(Value::as_str) != Some("review-state") {
            continue;
        }
        let Some(id) = data
            .get("reviewItemId")
            .and_then(Value::as_str)
            .filter(|id| valid_id(id))
        else {
            continue;
        };
        if !matches!(
            data.get("status").and_then(Value::as_str),
            Some("again" | "understood")
        ) {
            continue;
        }
        let revision = data.get("revision").and_then(Value::as_u64).unwrap_or(0);
        if result
            .get(id)
            .is_none_or(|old| old["revision"].as_u64().unwrap_or(0) < revision)
        {
            result.insert(id.into(), data.clone());
        }
    }
    Ok(result)
}
pub fn list(store: &mut ContentStore, query: &str) -> Result<Vec<Value>> {
    list_for_course(store, query, None)
}
pub fn list_for_course(
    store: &mut ContentStore,
    query: &str,
    course_id: Option<&str>,
) -> Result<Vec<Value>> {
    if query.len() > 1000 || course_id.is_some_and(|id| !valid_id(id)) {
        return Err(CoreError::new("INVALID_INPUT", "复习筛选条件无效。"));
    }
    let tokens: Vec<_> = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let mut items = all(store)?;
    items.retain(|item| {
        course_id.is_none_or(|id| item["courseId"].as_str() == Some(id))
            && tokens
                .iter()
                .all(|t| item.to_string().to_lowercase().contains(t))
    });
    items.truncate(200);
    Ok(items)
}
pub fn all(store: &mut ContentStore) -> Result<Vec<Value>> {
    let mut items = sessions::review_items_all(store)?;
    let state = states(store)?;
    let tree = crate::library_tree::list(store)?;
    for item in &mut items {
        let section_id = item["sectionId"].as_str().unwrap_or("").to_string();
        let node = tree.nodes.iter().find(|n| n.id == section_id);
        // Course chats retain their persisted owner even without a material source.
        let owner = item["courseId"].as_str().or_else(|| node.map(|n| n.course_id.as_str()));
        let course = owner.and_then(|id| tree.nodes.iter().find(|c| c.id == id));
        item["courseId"] = course.map(|n| json!(n.id)).unwrap_or(Value::Null);
        item["courseTitle"] = course.map(|n| json!(n.title)).unwrap_or(Value::Null);
        item["sectionTitle"] = node
            .map(|n| json!(n.title))
            .unwrap_or_else(|| json!("独立学习记录"));
        item["sourceDocumentId"] = if section_id.is_empty() {
            Value::Null
        } else {
            json!(section_id)
        };
        let saved = item["id"].as_str().and_then(|id| state.get(id));
        item["reviewState"] = saved
            .map(|value| json!({"status":value["status"],"reviewedAt":value["reviewedAt"]}))
            .unwrap_or_else(|| json!({"status":"new","reviewedAt":null}));
    }
    Ok(items)
}
pub fn set(store: &mut ContentStore, id: &str, status: &str, request_id: &str) -> Result<Value> {
    if !valid_id(id) || !valid_id(request_id) || !matches!(status, "again" | "understood") {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "复习项目、状态或请求编号无效。",
        ));
    }
    let item = sessions::review_items_all(store)?
        .into_iter()
        .find(|item| item["id"].as_str() == Some(id))
        .ok_or_else(|| CoreError::new("NOT_FOUND", "复习项目已变化，请刷新列表后再记录。"))?;
    let session_id = item["sessionId"]
        .as_str()
        .ok_or_else(|| CoreError::new("INVALID_DOCUMENT", "复习项目缺少来源记录。"))?;
    let session = sessions::get(store, session_id)?;
    if let Some(old) = session
        .entries
        .iter()
        .filter_map(|entry| entry.data.as_ref())
        .find(|data| {
            data.get("origin").and_then(Value::as_str) == Some("review-state")
                && data.get("requestId").and_then(Value::as_str) == Some(request_id)
        })
    {
        if old.get("reviewItemId").and_then(Value::as_str) != Some(id)
            || old.get("status").and_then(Value::as_str) != Some(status)
        {
            return Err(CoreError::new("CONFLICT", "此复习请求编号已用于其他内容。"));
        }
        return Ok(json!({"id":id,"status":status,"reviewedAt":old["reviewedAt"]}));
    }
    let revision = session
        .entries
        .iter()
        .filter_map(|entry| entry.data.as_ref())
        .filter(|data| {
            data.get("origin").and_then(Value::as_str) == Some("review-state")
                && data.get("reviewItemId").and_then(Value::as_str) == Some(id)
        })
        .filter_map(|data| data.get("revision").and_then(Value::as_u64))
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| CoreError::new("LIMIT_EXCEEDED", "复习记录版本已达上限。"))?;
    let stamp = timestamp();
    let entry = Entry {
        id: String::new(),
        kind: "note".into(),
        content: if status == "again" {
            "本人选择：再练一次".into()
        } else {
            "本人选择：已理解".into()
        },
        created_at: stamp.clone(),
        selection: None,
        related_ids: vec![id.into()],
        data: Some(
            json!({"origin":"review-state","reviewItemId":id,"status":status,"reviewedAt":stamp,"revision":revision,"requestId":request_id}),
        ),
    };
    sessions::append(
        store,
        session_id,
        entry,
        None,
        &format!("review_{}", &hash(request_id)[..32]),
    )?;
    Ok(json!({"id":id,"status":status,"reviewedAt":stamp}))
}
pub fn export(
    store: &mut ContentStore,
    ids: &[String],
    destination: &Path,
) -> Result<(String, usize)> {
    if ids.is_empty() || ids.len() > 200 || ids.iter().any(|id| !valid_id(id)) {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "请选择1至200条真实复习项目后导出。",
        ));
    }
    let unique = ids.iter().collect::<BTreeSet<_>>();
    if unique.len() != ids.len() {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "复习导出包含重复项目，请刷新后重试。",
        ));
    }
    let all = sessions::review_items_all(store)?;
    let map = all
        .into_iter()
        .map(|item| (item["id"].as_str().unwrap_or("").to_string(), item))
        .collect::<BTreeMap<_, _>>();
    let quote = |value: &str| format!("\"{}\"", value.replace('"', "\"\""));
    let mut csv = String::from("#separator:comma\n#html:false\n#columns:Front,Back,Tags\n");
    for id in ids {
        let card = map.get(id).ok_or_else(|| {
            CoreError::new(
                "NOT_FOUND",
                "所选复习项目已变化；未导出部分结果，请刷新后重试。",
            )
        })?;
        let tags = card["tags"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.replace(char::is_whitespace, "_"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        csv.push_str(&format!(
            "{},{},{}\n",
            quote(card["front"].as_str().unwrap_or("")),
            quote(card["back"].as_str().unwrap_or("")),
            quote(&tags)
        ));
    }
    fs::create_dir_all(destination)?;
    let path = destination.join(format!(
        "anki-selected-{}.csv",
        uuid::Uuid::new_v4().simple()
    ));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(csv.as_bytes())?;
    file.sync_all()?;
    Ok((path.to_string_lossy().into_owned(), ids.len()))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Engine;
    #[test]
    fn card_course_comes_from_persisted_session_without_inventing_material() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let state = temp.path().join("state");
        let engine = Engine::new(library.clone(), state.clone()).unwrap();
        let (owner, foreign, note, other, chat, legacy, independent) = {
            let mut core = engine.lock().unwrap();
            let mut node = |kind, parent, title| crate::library_tree::create(&mut core.content, kind, title, parent, "", title).unwrap();
            let owner = node("course", None, "card-owner-course");
            let foreign = node("course", None, "card-foreign-course");
            let note = node("section", Some(owner.id.as_str()), "card-owner-note");
            let other = node("section", Some(foreign.id.as_str()), "card-foreign-note");
            let chat = crate::course_sessions::create(&mut core, &owner.id, "card-empty-chat").unwrap();
            let legacy = sessions::create(&mut core.content, &note.id, &[]).unwrap();
            let independent = sessions::create_exploration(&mut core.content).unwrap();
            (owner.id, foreign.id, note.id, other.id, chat.id, legacy.id, independent.id)
        };
        let mut cards = vec![];
        for (session, source) in [(&chat, ""), (&chat, note.as_str()), (&legacy, note.as_str()), (&independent, "")] {
            let call = format!("card-owner-{}", cards.len());
            let scope = crate::learning_tools::ToolScope { session_id: session.clone(), section_id: source.into(), run_id: call.clone(), allowed_ids: vec![] };
            let args = json!({"front":call,"back":"Synthetic only"});
            let receipt = crate::learning_tools::execute(&engine, &scope, &call, "create_review_card", args.clone()).unwrap();
            assert_eq!(receipt["persisted"], true);
            assert_eq!(crate::learning_tools::execute(&engine, &scope, &call, "create_review_card", args).unwrap(), receipt);
            cards.push((receipt["entryId"].as_str().unwrap().to_owned(), source.to_owned(), session != &independent));
        }
        let foreign_scope = crate::learning_tools::ToolScope { session_id: chat.clone(), section_id: other.clone(), run_id: "card-cross-course".into(), allowed_ids: vec![other] };
        assert_eq!(crate::learning_tools::execute(&engine, &foreign_scope, "card-cross-course", "create_review_card", json!({"front":"Foreign card","back":"Must not save"})).unwrap_err().code, "TOOL_SCOPE");
        // Card payload metadata cannot override its owning session's course.
        {
            let mut core = engine.lock().unwrap();
            let doc = core.content.read(&cards[0].0).unwrap();
            let mut metadata = doc.metadata;
            metadata.extra.get_mut("data").unwrap()["courseId"] = json!(foreign);
            core.content.save(&doc.id, &crate::content::serialize(&metadata, &doc.body).unwrap(), &doc.version).unwrap();
        }
        drop(engine);
        let engine = Engine::new(library, state).unwrap();
        let mut core = engine.lock().unwrap();
        core.refresh().unwrap();
        let all = list(&mut core.content, "").unwrap();
        assert_eq!(all.len(), cards.len());
        for (id, source, owned) in cards {
            let item = all.iter().find(|item| item["id"] == id).unwrap();
            assert_eq!(item["courseId"], if owned { json!(owner) } else { Value::Null });
            assert_eq!(item["sourceDocumentId"], if source.is_empty() { Value::Null } else { json!(source) });
        }
        assert_eq!(list_for_course(&mut core.content, "", Some(&owner)).unwrap().len(), 3);
        assert!(list_for_course(&mut core.content, "", Some(&foreign)).unwrap().is_empty());
        assert!(sessions::get(&mut core.content, &chat).unwrap().section_id.is_empty());
    }

    #[test]
    fn exact_export_and_explicit_review_receipts_survive_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let state = temp.path().join("state");
        let engine = Engine::new(library.clone(), state.clone()).unwrap();
        let (session, id1, id2) = {
            let mut core = engine.lock().unwrap();
            let section = core
                .create_section("English", "Source", vec![], vec![])
                .unwrap();
            let session = core.start_session(&section.id, false).unwrap().0;
            let mut ids = vec![];
            for (n, front) in [(1, "First card"), (2, "Second card")] {
                let saved = sessions::append(
                    &mut core.content,
                    &session.id,
                    Entry {
                        id: String::new(),
                        kind: "note".into(),
                        content: front.into(),
                        created_at: String::new(),
                        selection: None,
                        related_ids: vec![],
                        data: Some(
                            json!({"reviewCard":true,"front":front,"back":"Reference","tags":[]}),
                        ),
                    },
                    None,
                    &format!("review-card-{n}"),
                )
                .unwrap();
                ids.push(saved.entries.last().unwrap().id.clone());
            }
            (session, ids[0].clone(), ids[1].clone())
        };
        {
            let mut core = engine.lock().unwrap();
            let first = set(&mut core.content, &id1, "again", "review_one").unwrap();
            assert_eq!(
                set(&mut core.content, &id1, "again", "review_one").unwrap(),
                first
            );
            assert_eq!(
                set(&mut core.content, &id1, "understood", "review_one")
                    .unwrap_err()
                    .code,
                "CONFLICT"
            );
            set(&mut core.content, &id1, "understood", "review_two").unwrap();
            let (path, count) =
                export(&mut core.content, &[id2.clone()], &state.join("exports")).unwrap();
            assert_eq!(count, 1);
            let text = fs::read_to_string(path).unwrap();
            assert!(text.contains("Second card") && !text.contains("First card"));
            assert!(export(&mut core.content, &[], &state.join("exports")).is_err());
            assert!(export(
                &mut core.content,
                &[id1.clone(), "missing".into()],
                &state.join("exports")
            )
            .is_err());
        }
        drop(engine);
        let reopened = Engine::new(library, state).unwrap();
        let mut core = reopened.lock().unwrap();
        core.refresh().unwrap();
        let items = list(&mut core.content, "").unwrap();
        assert_eq!(
            items.iter().find(|item| item["id"] == id1).unwrap()["reviewState"]["status"],
            "understood"
        );
        assert_eq!(
            sessions::get(&mut core.content, &session.id)
                .unwrap()
                .entries
                .iter()
                .filter(|entry| entry
                    .data
                    .as_ref()
                    .and_then(|d| d.get("origin"))
                    .and_then(Value::as_str)
                    == Some("review-state"))
                .count(),
            2
        );
    }
}
