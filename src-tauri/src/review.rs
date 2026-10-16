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
    let mut items = sessions::review_items(store, query)?;
    let state = states(store)?;
    for item in &mut items {
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
