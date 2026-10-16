//! Exercise Markdown is a rendered view. Grading reads the original session entry data.
use crate::{content::ContentStore, sessions, types::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExerciseDocument {
    pub document: Document,
    pub session: Session,
    pub exercises: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualExercise {
    pub question: String,
    pub format: String,
    pub answers: Vec<String>,
    pub explanation: String,
}
fn invalid() -> CoreError {
    CoreError::new(
        "INVALID_MODEL_OUTPUT",
        "练习结构、字段长度或来源无效；收到的内容已保留。",
    )
}
fn conflict() -> CoreError {
    CoreError::new("CONFLICT", "请求编号已关联另一份练习，未覆盖原记录。")
}
fn text(value: Option<&Value>, minimum: usize, maximum: usize) -> Result<&str> {
    value
        .and_then(Value::as_str)
        .filter(|s| s.trim().len() >= minimum && s.len() <= maximum && !s.contains('\0'))
        .ok_or_else(invalid)
}
fn known(value: &Value, fields: &[&str]) -> Result<()> {
    if value
        .as_object()
        .is_none_or(|object| object.keys().any(|key| !fields.contains(&key.as_str())))
    {
        return Err(invalid());
    }
    Ok(())
}
/// Strict model-facing JSON: no Markdown fence, invented IDs, or transport metadata.
pub fn validate(mut data: Value, manifest: &[ManifestItem], namespace: &str) -> Result<Value> {
    known(&data, &["exercises", "summary"])?;
    if !valid_id(namespace) {
        return Err(invalid());
    }
    let list = data
        .get_mut("exercises")
        .and_then(Value::as_array_mut)
        .ok_or_else(invalid)?;
    for (index, question) in list.iter_mut().enumerate() {
        known(
            question,
            &[
                "question",
                "format",
                "answers",
                "explanation",
                "tags",
                "sourceIds",
                "rubric",
            ],
        )?;
        question["id"] = json!(format!("q_{}_{}", &hash(namespace)[..32], index));
        question["answerMatch"] = json!("trim");
    }
    validate_stored(&data, manifest, false)?;
    Ok(data)
}
fn validate_stored(data: &Value, manifest: &[ManifestItem], legacy: bool) -> Result<()> {
    if serde_json::to_vec(data)?.len() > 256 * 1024 || manifest.len() > 64 {
        return Err(invalid());
    }
    let fields: &[&str] = if legacy {
        &[
            "exercises",
            "summary",
            "contextManifest",
            "promptVersion",
            "runId",
            "sourceDocumentId",
            "requirements",
            "manual",
        ]
    } else {
        &[
            "exercises",
            "summary",
            "contextManifest",
            "promptVersion",
            "runId",
            "sourceDocumentId",
            "requirements",
        ]
    };
    known(data, fields)?;
    if legacy && data.get("manual").is_some_and(|value| !value.is_boolean()) {
        return Err(invalid());
    }
    if !legacy {
        text(data.get("summary"), 0, 10000)?;
    } else if data.get("summary").is_some() {
        text(data.get("summary"), 0, 10000)?;
    }
    let sources: BTreeSet<_> = manifest.iter().map(|item| item.id.as_str()).collect();
    if sources.len() != manifest.len() || sources.iter().any(|id| !valid_id(id)) {
        return Err(invalid());
    }
    let list = data
        .get("exercises")
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty() && list.len() <= 5)
        .ok_or_else(invalid)?;
    let mut ids = BTreeSet::new();
    for question in list {
        known(
            question,
            &[
                "id",
                "question",
                "format",
                "answers",
                "explanation",
                "tags",
                "sourceIds",
                "rubric",
                "answerMatch",
            ],
        )?;
        if question
            .get("answerMatch")
            .is_some_and(|v| v.as_str() != Some("trim"))
        {
            return Err(invalid());
        }
        let id = text(question.get("id"), 1, 128)?;
        if !valid_id(id) || !ids.insert(id) {
            return Err(invalid());
        }
        text(question.get("question"), 1, 20000)?;
        if let Some(rubric) = question.get("rubric") {
            crate::grading::validate_rubric(rubric)?;
        }
        if !matches!(
            question.get("format").and_then(Value::as_str),
            Some("exact" | "self")
        ) {
            return Err(invalid());
        }
        let answers = question
            .get("answers")
            .and_then(Value::as_array)
            .filter(|list| !list.is_empty() && list.len() <= 10)
            .ok_or_else(invalid)?;
        for answer in answers {
            text(Some(answer), 1, 10000)?;
        }
        if !legacy || question.get("explanation").is_some() {
            text(question.get("explanation"), 0, 20000)?;
        }
        if !legacy || question.get("tags").is_some() {
            let tags = question
                .get("tags")
                .and_then(Value::as_array)
                .filter(|list| list.len() <= 12)
                .ok_or_else(invalid)?;
            for tag in tags {
                text(Some(tag), 1, 256)?;
                if tag.as_str().unwrap().chars().count() > 64 {
                    return Err(invalid());
                }
            }
        }
        if !legacy || question.get("sourceIds").is_some() {
            let ids = question
                .get("sourceIds")
                .and_then(Value::as_array)
                .filter(|list| list.len() <= 32)
                .ok_or_else(invalid)?;
            let mut unique = BTreeSet::new();
            for id in ids {
                let id = id.as_str().ok_or_else(invalid)?;
                if !sources.contains(id) || !unique.insert(id) {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(())
}
fn stored_manifest(data: &Value, fallback: &[ManifestItem]) -> Result<Vec<ManifestItem>> {
    match data.get("contextManifest") {
        Some(value) => serde_json::from_value(value.clone()).map_err(|_| invalid()),
        None => Ok(fallback.to_vec()),
    }
}
fn request(request_id: &str) -> Result<()> {
    if request_id.len() < 8 || request_id.len() > 128 || request_id.chars().any(char::is_control) {
        return Err(CoreError::new("INVALID_INPUT", "请求缺少有效防重复编号。"));
    }
    Ok(())
}
fn source(store: &ContentStore, session: &Session, id: &str) -> Result<Document> {
    let document = store.read(id)?;
    if session.section_id != id
        || !matches!(
            document.metadata.kind.as_str(),
            "section" | "knowledge" | "exploration"
        )
    {
        return Err(CoreError::new(
            "OUT_OF_SCOPE",
            "练习必须属于原会话的学习文档，不能转存到其他文档。",
        ));
    }
    Ok(document)
}
fn question_links(data: &Value) -> Vec<String> {
    data["exercises"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|question| question["sourceIds"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
fn literal(value: &str) -> String {
    let longest = value
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}text\n{value}\n{fence}")
}
/// Core rendering is deliberately independent from any frontend JSON/body parser.
pub fn render(data: &Value) -> Result<String> {
    let list = data["exercises"].as_array().ok_or_else(invalid)?;
    let mut body = String::from(
        "# 练习\n\n题目、参考答案与解析保存在原会话的结构化练习记录中；本页是阅读视图。\n",
    );
    if let Some(summary) = data["summary"].as_str().filter(|s| !s.trim().is_empty()) {
        body.push_str(&format!("\n## 练习说明\n\n{}\n", literal(summary)));
    }
    for (index, question) in list.iter().enumerate() {
        let answers = question["answers"]
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(|answer| answer.as_str().ok_or_else(invalid))
            .collect::<Result<Vec<_>>>()?;
        body.push_str(&format!("\n## 第 {} 题\n\n题目编号：`{}`\n\n### 题干\n\n{}\n\n### 参考答案\n\n{}\n\n### 解析\n\n{}\n\n### 来源\n\n", index + 1, question["id"].as_str().ok_or_else(invalid)?, literal(question["question"].as_str().ok_or_else(invalid)?), literal(&answers.join("\n")), literal(question["explanation"].as_str().unwrap_or("尚未提供解析。"))));
        let links = question["sourceIds"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(|id| format!("- [[{id}]]"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        body.push_str(&if links.is_empty() {
            "未声明材料来源。".into()
        } else {
            links.join("\n")
        });
        body.push('\n');
    }
    Ok(body)
}
fn read_document(store: &mut ContentStore, id: &str) -> Result<Document> {
    match store.read(id) {
        Ok(document) => Ok(document),
        Err(error) if error.code == "NOT_FOUND" => {
            store.refresh()?;
            store.read(id)
        }
        Err(error)
            if error.code == "IO"
                && store
                    .summaries()
                    .iter()
                    .find(|item| item.id == id)
                    .is_some_and(|item| {
                        std::fs::symlink_metadata(store.root().join(&item.path))
                            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                    }) =>
        {
            store.refresh()?;
            store.read(id)
        }
        Err(error) => Err(error),
    }
}
fn materialize(
    store: &mut ContentStore,
    session: &Session,
    source_document: &Document,
    entry_id: &str,
    data: &Value,
    request_id: &str,
) -> Result<Document> {
    let id = format!("exdoc_{}", &hash(request_id)[..32]);
    let digest = hash(&serde_json::to_string(
        &json!({"sessionId":session.id,"sourceDocumentId":source_document.id,"exerciseEntryId":entry_id,"data":data}),
    )?);
    match read_document(store, &id) {
        Ok(document) => {
            if document.metadata.kind != "exercise"
                || document
                    .metadata
                    .extra
                    .get("requestDigest")
                    .and_then(Value::as_str)
                    != Some(&digest)
                || document
                    .metadata
                    .extra
                    .get("originSessionId")
                    .and_then(Value::as_str)
                    != Some(&session.id)
                || document
                    .metadata
                    .extra
                    .get("exerciseEntryId")
                    .and_then(Value::as_str)
                    != Some(entry_id)
            {
                return Err(conflict());
            }
            return Ok(document);
        }
        Err(error) if error.code == "NOT_FOUND" => {}
        Err(error) => return Err(error),
    }
    let mut extra = BTreeMap::new();
    extra.insert("parentId".into(), json!(source_document.id));
    extra.insert("originSessionId".into(), json!(session.id));
    extra.insert("exerciseEntryId".into(), json!(entry_id));
    extra.insert("sourceDocumentId".into(), json!(source_document.id));
    extra.insert("requestDigest".into(), json!(digest));
    let metadata = Metadata {
        id: id.clone(),
        title: format!("{} · 练习", source_document.metadata.title),
        kind: "exercise".into(),
        goals: vec![],
        tags: vec![],
        links: question_links(data),
        version: 1,
        extra,
    };
    store.create(metadata, &render(data)?, &format!("exercises/{id}.md"))
}
/// Append is the durable transaction unit; a retry repairs a missing child without changing its entry.
pub fn create(
    store: &mut ContentStore,
    session_id: &str,
    source_document_id: &str,
    mut data: Value,
    request_id: &str,
) -> Result<Document> {
    request(request_id)?;
    let session = sessions::get(store, session_id)?;
    let source_document = source(store, &session, source_document_id)?;
    let child_id = format!("exdoc_{}", &hash(request_id)[..32]);
    match read_document(store, &child_id) {
        Ok(child)
            if child.metadata.kind != "exercise"
                || child
                    .metadata
                    .extra
                    .get("originSessionId")
                    .and_then(Value::as_str)
                    != Some(session_id) =>
        {
            return Err(conflict())
        }
        Ok(_) => {}
        Err(error) if error.code == "NOT_FOUND" => {}
        Err(error) => return Err(error),
    }
    let entry_id = format!("entry_{}", &hash(request_id)[..32]);
    if store.read(&entry_id).is_ok_and(|entry| {
        entry
            .metadata
            .extra
            .get("sessionId")
            .and_then(Value::as_str)
            != Some(session_id)
    }) {
        return Err(conflict());
    }
    let existing_entry = session
        .entries
        .iter()
        .any(|entry| entry.id == entry_id && entry.kind == "exercise");
    let manifest = stored_manifest(&data, &session.context_manifest)?;
    let authorized: BTreeSet<_> = session
        .context_manifest
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    if !existing_entry
        && manifest
            .iter()
            .any(|item| !authorized.contains(item.id.as_str()))
    {
        return Err(CoreError::new(
            "OUT_OF_SCOPE",
            "练习来源不在当前会话的材料白名单中。",
        ));
    }
    if data.get("contextManifest").is_none() {
        data["contextManifest"] = json!(manifest);
    }
    data["sourceDocumentId"] = json!(source_document_id);
    validate_stored(&data, &manifest, false)?;
    let ids: BTreeSet<_> = data["exercises"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["id"].as_str().unwrap())
        .collect();
    if session
        .entries
        .iter()
        .filter(|entry| entry.kind == "exercise" && entry.id != entry_id)
        .filter_map(|entry| entry.data.as_ref())
        .filter_map(|data| data["exercises"].as_array())
        .flatten()
        .filter_map(|question| question["id"].as_str())
        .any(|id| ids.contains(id))
    {
        return Err(conflict());
    }
    let saved = sessions::append(
        store,
        session_id,
        Entry {
            id: String::new(),
            kind: "exercise".into(),
            content: render(&data)?,
            created_at: String::new(),
            selection: None,
            related_ids: question_links(&data),
            data: Some(data.clone()),
        },
        None,
        request_id,
    )?;
    materialize(
        store,
        &saved,
        &source_document,
        &entry_id,
        &data,
        request_id,
    )
}
pub fn get(store: &mut ContentStore, document_id: &str) -> Result<ExerciseDocument> {
    let document = read_document(store, document_id)?;
    if document.metadata.kind != "exercise" {
        return Err(CoreError::new("INVALID_INPUT", "请选择练习文档。"));
    }
    let session_id = document
        .metadata
        .extra
        .get("originSessionId")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .ok_or_else(invalid)?;
    let entry_id = document
        .metadata
        .extra
        .get("exerciseEntryId")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .ok_or_else(invalid)?;
    let session = sessions::get(store, session_id)?;
    let entry = session
        .entries
        .iter()
        .find(|entry| entry.id == entry_id && entry.kind == "exercise")
        .ok_or_else(|| CoreError::new("NOT_FOUND", "原练习记录缺失，未把页面正文当作评分依据。"))?;
    let exercises = entry.data.clone().ok_or_else(invalid)?;
    validate_stored(
        &exercises,
        &stored_manifest(&exercises, &session.context_manifest)?,
        true,
    )?;
    Ok(ExerciseDocument {
        document,
        session,
        exercises,
    })
}
pub fn create_manual(
    store: &mut ContentStore,
    session_id: &str,
    source_document_id: &str,
    input: ManualExercise,
    request_id: &str,
) -> Result<ExerciseDocument> {
    let session = sessions::get(store, session_id)?;
    let data = validate(
        json!({"exercises":[{"question":input.question,"format":input.format,"answers":input.answers,"explanation":input.explanation,"tags":[],"sourceIds":[]}],"summary":"手动创建练习"}),
        &session.context_manifest,
        &format!("manual_{}", &hash(request_id)[..32]),
    )?;
    let document = create(store, session_id, source_document_id, data, request_id)?;
    get(store, &document.id)
}
/// Explicit migration only. Existing question IDs and original Markdown receipts remain untouched.
pub fn materialize_legacy(
    store: &mut ContentStore,
    session_id: &str,
    exercise_entry_id: &str,
) -> Result<Document> {
    let session = sessions::get(store, session_id)?;
    let source_document = source(store, &session, &session.section_id)?;
    let entry = session
        .entries
        .iter()
        .find(|entry| entry.id == exercise_entry_id && entry.kind == "exercise")
        .ok_or_else(|| CoreError::new("NOT_FOUND", "找不到原练习记录。"))?;
    let data = entry.data.clone().ok_or_else(invalid)?;
    validate_stored(
        &data,
        &stored_manifest(&data, &session.context_manifest)?,
        true,
    )?;
    let request_id = format!(
        "legacy_{}",
        &hash(&format!("{session_id}\n{exercise_entry_id}"))[..32]
    );
    materialize(
        store,
        &session,
        &source_document,
        exercise_entry_id,
        &data,
        &request_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, ContentStore, Session, Document) {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ContentStore::new(
            directory.path().join("library"),
            directory.path().join("state"),
        )
        .unwrap();
        let document = store
            .create(
                Metadata {
                    id: "section_ex".into(),
                    title: "Since 与 for".into(),
                    kind: "section".into(),
                    goals: vec![],
                    tags: vec![],
                    links: vec![],
                    version: 1,
                    extra: BTreeMap::new(),
                },
                "Since Monday is a starting point.",
                "course/source.md",
            )
            .unwrap();
        let manifest = vec![ManifestItem {
            id: document.id.clone(),
            path: document.path.clone(),
            version: document.version.clone(),
            included_chars: document.body.chars().count(),
        }];
        let session = sessions::create(&mut store, &document.id, &manifest).unwrap();
        (directory, store, session, document)
    }
    fn raw() -> Value {
        json!({"exercises":[{"question":"for or since three years?","format":"exact","answers":["for"],"explanation":"Three years is a duration.","tags":["time"],"sourceIds":["section_ex"]}],"summary":"Time expressions"})
    }
    fn payload(session: &Session, namespace: &str) -> Value {
        validate(raw(), &session.context_manifest, namespace).unwrap()
    }
    #[test]
    fn strict_structure_rejects_unknown_fields_lengths_and_untrusted_sources_before_write() {
        let (_directory, mut store, session, source) = fixture();
        for change in 0..9 {
            let mut data = raw();
            match change {
                0 => data["shell"] = json!("denied"),
                1 => data["exercises"][0]["extra"] = json!(true),
                2 => data["exercises"][0]["id"] = json!("forged"),
                3 => data["exercises"][0]["sourceIds"] = json!(["not_in_manifest"]),
                4 => data["exercises"][0]["tags"] = json!(["x".repeat(65)]),
                5 => data["exercises"][0]["answers"] = json!([]),
                6 => data["exercises"][0]["explanation"] = json!("x".repeat(20001)),
                7 => data["summary"] = json!(12),
                _ => data["exercises"][0]["sourceIds"] = json!("section_ex"),
            }
            assert!(
                validate(data, &session.context_manifest, "run_strict").is_err(),
                "case {change}"
            );
        }
        let mut data = payload(&session, "run_valid");
        data["contextManifest"] =
            json!([{"id":"invented_source","path":"secret.md","version":"v1","includedChars":1}]);
        assert!(create(
            &mut store,
            &session.id,
            &source.id,
            data,
            "invalid-source-request"
        )
        .is_err());
        assert!(sessions::get(&mut store, &session.id)
            .unwrap()
            .entries
            .is_empty());
    }
    #[test]
    fn child_creation_retry_conflict_missing_child_repair_and_original_entry_are_durable() {
        let (directory, mut store, session, source) = fixture();
        let data = payload(&session, "run_child");
        let document = create(
            &mut store,
            &session.id,
            &source.id,
            data.clone(),
            "exercise-child-request",
        )
        .unwrap();
        assert_eq!(document.metadata.kind, "exercise");
        assert_eq!(document.metadata.extra["parentId"], source.id);
        assert_eq!(document.metadata.links, vec![source.id.clone()]);
        for heading in ["### 题干", "### 参考答案", "### 解析", "### 来源"] {
            assert!(document.body.contains(heading));
        }
        let original = std::fs::read_to_string(store.root().join(&source.path)).unwrap();
        assert_eq!(
            create(
                &mut store,
                &session.id,
                &source.id,
                data.clone(),
                "exercise-child-request"
            )
            .unwrap()
            .version,
            document.version
        );
        let mut changed = data.clone();
        changed["exercises"][0]["answers"] = json!(["since"]);
        assert_eq!(
            create(
                &mut store,
                &session.id,
                &source.id,
                changed,
                "exercise-child-request"
            )
            .unwrap_err()
            .code,
            "CONFLICT"
        );
        let entries = sessions::get(&mut store, &session.id).unwrap().entries;
        assert_eq!(entries.len(), 1);
        let entry_bytes = store.read(&entries[0].id).unwrap().content;
        std::fs::remove_file(store.root().join(&document.path)).unwrap();
        store.refresh().unwrap();
        let repaired = create(
            &mut store,
            &session.id,
            &source.id,
            data,
            "exercise-child-request",
        )
        .unwrap();
        assert_eq!(repaired.id, document.id);
        assert_eq!(store.read(&entries[0].id).unwrap().content, entry_bytes);
        assert_eq!(
            std::fs::read_to_string(store.root().join(&source.path)).unwrap(),
            original
        );
        drop(store);
        let mut reopened = ContentStore::new(
            directory.path().join("library"),
            directory.path().join("state"),
        )
        .unwrap();
        let loaded = get(&mut reopened, &document.id).unwrap();
        assert_eq!(loaded.session.id, session.id);
        assert_eq!(loaded.session.entries.len(), 1);
        assert_eq!(
            loaded.exercises["exercises"][0]["id"],
            entries[0].data.as_ref().unwrap()["exercises"][0]["id"]
        );
    }
    #[test]
    fn moving_child_and_editing_rendered_body_does_not_change_question_ids_or_grading_answers() {
        let (directory, mut store, session, source) = fixture();
        let document = create(
            &mut store,
            &session.id,
            &source.id,
            payload(&session, "run_move"),
            "exercise-move-request",
        )
        .unwrap();
        let question_id = get(&mut store, &document.id).unwrap().exercises["exercises"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut metadata = document.metadata.clone();
        metadata
            .extra
            .insert("parentId".into(), json!("different_display_parent"));
        metadata.extra.insert("order".into(), json!(42));
        let edited =
            crate::content::serialize(&metadata, "# Edited view\n\n参考答案：since\n").unwrap();
        store
            .save(&document.id, &edited, &document.version)
            .unwrap();
        let moved = "moved/exercise-renamed.md";
        std::fs::create_dir_all(store.root().join("moved")).unwrap();
        std::fs::rename(store.root().join(&document.path), store.root().join(moved)).unwrap();
        store.refresh().unwrap();
        let loaded = get(&mut store, &document.id).unwrap();
        assert_eq!(loaded.document.path, moved);
        assert_eq!(loaded.exercises["exercises"][0]["answers"], json!(["for"]));
        assert_eq!(
            sessions::create(&mut store, &document.id, &[]).unwrap().id,
            session.id
        );
        let answered = sessions::append(
            &mut store,
            &session.id,
            Entry {
                id: String::new(),
                kind: "attempt".into(),
                content: "for".into(),
                created_at: String::new(),
                selection: None,
                related_ids: vec![],
                data: Some(json!({"exerciseId":question_id,"answer":"for"})),
            },
            None,
            "exercise-move-answer",
        )
        .unwrap();
        let feedback = answered
            .entries
            .iter()
            .find(|entry| entry.kind == "feedback")
            .unwrap();
        assert_eq!(feedback.data.as_ref().unwrap()["correct"], true);
        drop(store);
        let mut reopened = ContentStore::new(
            directory.path().join("library"),
            directory.path().join("state"),
        )
        .unwrap();
        assert_eq!(
            get(&mut reopened, &document.id)
                .unwrap()
                .session
                .entries
                .iter()
                .filter(|entry| entry.kind == "attempt")
                .count(),
            1
        );
    }
    #[test]
    fn manual_and_explicit_legacy_materialization_preserve_stable_ids_and_zero_automatic_migration()
    {
        let (_directory, mut store, session, source) = fixture();
        let manual = create_manual(
            &mut store,
            &session.id,
            &source.id,
            ManualExercise {
                question: "What is a duration?".into(),
                format: "self".into(),
                answers: vec!["a length of time".into()],
                explanation: "Compare a time point.".into(),
            },
            "manual-exercise-request",
        )
        .unwrap();
        assert!(manual.exercises["exercises"][0]["id"]
            .as_str()
            .unwrap()
            .starts_with("q_"));
        let saved = sessions::append(&mut store, &session.id, Entry { id:String::new(),kind:"exercise".into(),content:"Legacy prose remains unchanged.".into(),created_at:String::new(),selection:None,related_ids:vec![],data:Some(json!({"manual":true,"exercises":[{"id":"q_legacy","question":"Since Monday?","format":"exact","answers":["since"]}]})) }, None, "legacy-exercise-request").unwrap();
        assert_eq!(
            store
                .summaries()
                .iter()
                .filter(|item| item.kind == "exercise")
                .count(),
            1
        );
        let entry_id = saved
            .entries
            .iter()
            .find(|entry| entry.content == "Legacy prose remains unchanged.")
            .unwrap()
            .id
            .clone();
        let bytes = store.read(&entry_id).unwrap().content;
        let migrated = materialize_legacy(&mut store, &session.id, &entry_id).unwrap();
        assert_eq!(
            materialize_legacy(&mut store, &session.id, &entry_id)
                .unwrap()
                .id,
            migrated.id
        );
        let loaded = get(&mut store, &migrated.id).unwrap();
        assert_eq!(loaded.exercises["exercises"][0]["id"], "q_legacy");
        assert_eq!(loaded.exercises["manual"], true);
        let mut model_payload = raw();
        model_payload["manual"] = json!(true);
        assert!(validate(
            model_payload,
            &session.context_manifest,
            "run_unknown_manual"
        )
        .is_err());
        assert_eq!(store.read(&entry_id).unwrap().content, bytes);
        assert_eq!(
            sessions::get(&mut store, &session.id)
                .unwrap()
                .entries
                .len(),
            2
        );
    }
}
