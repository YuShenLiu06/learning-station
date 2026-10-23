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
pub(crate) fn field_error(path: &str, reason: &str) -> CoreError {
    CoreError::new(
        "INVALID_MODEL_OUTPUT",
        format!("练习校验失败：{path} {reason}；收到的内容已保留。"),
    )
    .details(json!({"field":path,"reason":reason,"recoverable":!reason.contains("本轮授权")}))
}
fn text<'a>(
    value: Option<&'a Value>,
    minimum: usize,
    maximum: usize,
    path: &str,
) -> Result<&'a str> {
    let s = value
        .and_then(Value::as_str)
        .ok_or_else(|| field_error(path, "必须是文本"))?;
    let count = s.chars().count();
    if s.trim().chars().count() < minimum {
        return Err(field_error(path, "不能为空或只有空白"));
    }
    if count > maximum {
        return Err(field_error(
            path,
            &format!("共{count}个字符，最多{maximum}个字符"),
        ));
    }
    if s.contains('\0') {
        return Err(field_error(path, "不能包含空字符"));
    }
    Ok(s)
}
fn known(value: &Value, fields: &[&str]) -> Result<()> {
    if value
        .as_object()
        .is_none_or(|object| object.keys().any(|key| !fields.contains(&key.as_str())))
    {
        return Err(field_error("schema", "必须是对象且不能包含未声明字段"));
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
        .ok_or_else(|| field_error("exercises", "必须是数组"))?;
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
        )
        .map_err(|_| {
            field_error(
                &format!("exercises[{}]", index + 1),
                "必须是对象且不能包含未声明字段",
            )
        })?;
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
            "businessUserEntryId",
            "toolArgumentsHash",
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
            "businessUserEntryId",
            "toolArgumentsHash",
        ]
    };
    known(data, fields)?;
    if legacy && data.get("manual").is_some_and(|value| !value.is_boolean()) {
        return Err(invalid());
    }
    if !legacy {
        text(data.get("summary"), 0, 10000, "summary")?;
    } else if data.get("summary").is_some() {
        text(data.get("summary"), 0, 10000, "summary")?;
    }
    let sources: BTreeSet<_> = manifest.iter().map(|item| item.id.as_str()).collect();
    if sources.len() != manifest.len() || sources.iter().any(|id| !valid_id(id)) {
        return Err(invalid());
    }
    let list = data
        .get("exercises")
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty() && list.len() <= 5)
        .ok_or_else(|| field_error("exercises", "必须包含1至5道练习"))?;
    let mut ids = BTreeSet::new();
    for (index, question) in list.iter().enumerate() {
        let path = format!("exercises[{}]", index + 1);
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
        let id = text(question.get("id"), 1, 128, &format!("{path}.id"))?;
        if !valid_id(id) || !ids.insert(id) {
            return Err(invalid());
        }
        text(
            question.get("question"),
            1,
            20000,
            &format!("{path}.question"),
        )?;
        if let Some(rubric) = question.get("rubric") {
            crate::grading::validate_rubric(rubric).map_err(|_| {
                field_error(&format!("{path}.rubric"), "评分标准结构、编号或分值无效")
            })?;
        }
        if !matches!(
            question.get("format").and_then(Value::as_str),
            Some("exact" | "self")
        ) {
            return Err(field_error(&format!("{path}.format"), "必须是exact或self"));
        }
        let answers = question
            .get("answers")
            .and_then(Value::as_array)
            .filter(|list| !list.is_empty() && list.len() <= 10)
            .ok_or_else(|| field_error(&format!("{path}.answers"), "必须包含1至10个答案"))?;
        for (answer_index, answer) in answers.iter().enumerate() {
            text(
                Some(answer),
                1,
                10000,
                &format!("{path}.answers[{}]", answer_index + 1),
            )?;
        }
        if question.get("explanation").is_some() {
            text(
                question.get("explanation"),
                0,
                20000,
                &format!("{path}.explanation"),
            )?;
        }
        if !legacy || question.get("tags").is_some() {
            let tags = question
                .get("tags")
                .and_then(Value::as_array)
                .filter(|list| list.len() <= 12)
                .ok_or_else(|| field_error(&format!("{path}.tags"), "必须是最多12项的数组"))?;
            for tag in tags {
                text(Some(tag), 1, 64, &format!("{path}.tags"))?;
            }
        }
        if !legacy || question.get("sourceIds").is_some() {
            let ids = question
                .get("sourceIds")
                .and_then(Value::as_array)
                .filter(|list| list.len() <= 32)
                .ok_or_else(|| field_error(&format!("{path}.sourceIds"), "必须是最多32项的数组"))?;
            let mut unique = BTreeSet::new();
            for id in ids {
                let id = id.as_str().ok_or_else(|| {
                    field_error(&format!("{path}.sourceIds"), "来源编号必须是文本")
                })?;
                if !sources.contains(id) {
                    return Err(field_error(
                        &format!("{path}.sourceIds"),
                        "含本轮授权来源清单以外的编号",
                    ));
                }
                if !unique.insert(id) {
                    return Err(field_error(&format!("{path}.sourceIds"), "来源编号重复"));
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
    if crate::course_sessions::source(store, session, id)? != id
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
    data: Value,
    request_id: &str,
) -> Result<Document> {
    create_from_snapshot(store, session_id, source_document_id, data, request_id, None)
}
pub(crate) fn create_from_snapshot(
    store: &mut ContentStore, session_id: &str, source_document_id: &str,
    mut data: Value, request_id: &str, frozen: Option<&[ManifestItem]>,
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
    let authorized: BTreeSet<_> = frozen.unwrap_or(&session.context_manifest)
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
pub struct ExerciseOrigin {
    pub session_id: String,
    pub entry_id: String,
    pub source_document_id: String,
}
/// The page is an editable view; only its verified immutable ledger mapping is provenance.
pub fn origin(store: &ContentStore, document: &Document) -> Result<ExerciseOrigin> {
    let bad = || CoreError::new("CONTEXT_CONFLICT", "练习页面与原账本的来源映射不一致，请保留页面并重新打开原练习。").details(json!({"documentId":document.id}));
    let field = |key: &str| document.metadata.extra.get(key).and_then(Value::as_str)
        .filter(|id| valid_id(id)).map(str::to_owned).ok_or_else(&bad);
    if document.metadata.kind != "exercise" { return Err(bad()); }
    let session_id = field("originSessionId")?;
    let entry_id = field("exerciseEntryId")?;
    let native_page = entry_id.strip_prefix("entry_").map(|suffix| format!("exdoc_{suffix}"));
    let legacy_request = format!("legacy_{}", &hash(&format!("{session_id}\n{entry_id}"))[..32]);
    let legacy_page = format!("exdoc_{}", &hash(&legacy_request)[..32]);
    if native_page.as_deref() != Some(&document.id) && legacy_page != document.id { return Err(bad()); }
    let owner = store.read_relative(&format!("sessions/{session_id}/session.md"))?;
    let entry = store.read_relative(&format!("sessions/{session_id}/entries/{entry_id}.md"))?;
    if owner.id != session_id || owner.metadata.kind != "session" || entry.id != entry_id
        || entry.metadata.kind != "entry" || entry.metadata.extra.get("sessionId").and_then(Value::as_str) != Some(&session_id)
        || entry.metadata.extra.get("entryKind").and_then(Value::as_str) != Some("exercise") { return Err(bad()); }
    let data = entry.metadata.extra.get("data").ok_or_else(&bad)?;
    let source_document_id = data.get("sourceDocumentId").or_else(|| owner.metadata.extra.get("sectionId"))
        .and_then(Value::as_str).filter(|id| valid_id(id)).ok_or_else(&bad)?.to_owned();
    let digest = hash(&serde_json::to_string(&json!({"sessionId":session_id,"sourceDocumentId":source_document_id,"exerciseEntryId":entry_id,"data":data}))?);
    if document.metadata.extra.get("sourceDocumentId").and_then(Value::as_str) != Some(&source_document_id)
        || document.metadata.extra.get("requestDigest").and_then(Value::as_str) != Some(&digest) { return Err(bad()); }
    Ok(ExerciseOrigin { session_id, entry_id, source_document_id })
}
pub fn get(store: &mut ContentStore, document_id: &str) -> Result<ExerciseDocument> {
    let document = read_document(store, document_id)?;
    if document.metadata.kind != "exercise" {
        return Err(CoreError::new("INVALID_INPUT", "请选择练习文档。"));
    }
    let provenance = origin(store, &document)?;
    let session = sessions::get(store, &provenance.session_id)?;
    let entry = session
        .entries
        .iter()
        .find(|entry| entry.id == provenance.entry_id && entry.kind == "exercise")
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
    source(store, &session, source_document_id)?;
    let manifest = crate::context::load(store, source_document_id, None)?.manifest;
    let mut data = validate(
        json!({"exercises":[{"question":input.question,"format":input.format,"answers":input.answers,"explanation":input.explanation,"tags":[],"sourceIds":[]}],"summary":"手动创建练习"}),
        &manifest,
        &format!("manual_{}", &hash(request_id)[..32]),
    )?;
    data["contextManifest"] = json!(manifest);
    let document = create_from_snapshot(store, session_id, source_document_id, data, request_id, Some(&manifest))?;
    get(store, &document.id)
}
/// Explicit migration only. Existing question IDs and original Markdown receipts remain untouched.
pub fn materialize_legacy(
    store: &mut ContentStore,
    session_id: &str,
    exercise_entry_id: &str,
) -> Result<Document> {
    let session = sessions::get(store, session_id)?;
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
    let source_id = data.get("sourceDocumentId").and_then(Value::as_str).unwrap_or(&session.section_id);
    let source_document = store.read(source_id)?;
    if !matches!(source_document.metadata.kind.as_str(), "section" | "knowledge" | "exploration") { return Err(invalid()); }
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
    fn unicode_contract_and_safe_field_diagnostics() {
        let (_directory, mut store, session, source) = fixture();
        for (index, text) in ["汉".repeat(4000), "🧠".repeat(6000), "a".repeat(10000)]
            .iter()
            .enumerate()
        {
            let mut data = raw();
            data["summary"] = json!(text);
            data["exercises"][0]["answers"] = json!([text]);
            let request = format!("unicode-request-{index}");
            let valid = validate(data, &session.context_manifest, &request).unwrap();
            let first =
                create(&mut store, &session.id, &source.id, valid.clone(), &request).unwrap();
            let retry = create(&mut store, &session.id, &source.id, valid, &request).unwrap();
            assert_eq!(first.id, retry.id);
        }
        for (field, value, expected) in [
            ("answers", json!(["汉".repeat(10001)]), "answers[1]"),
            ("question", json!("  \n  "), "question"),
            (
                "sourceIds",
                json!(["private-source-not-to-echo"]),
                "sourceIds",
            ),
            (
                "sourceIds",
                json!(["section_ex", "section_ex"]),
                "sourceIds",
            ),
        ] {
            let mut data = raw();
            data["exercises"][0][field] = value;
            let error = validate(data, &session.context_manifest, "diagnostic-run").unwrap_err();
            assert_eq!(error.code, "INVALID_MODEL_OUTPUT");
            assert!(error.message.contains(expected));
            assert!(!error.message.contains("private-source-not-to-echo"));
        }
        assert_eq!(
            sessions::get(&mut store, &session.id)
                .unwrap()
                .entries
                .len(),
            3
        );
    }
    #[test]
    fn file_paths_and_history_only_ids_never_become_exercise_sources() {
        let manifest = vec![ManifestItem {
            id: "section_ex".into(),
            path: "courses/course_one/materials/documents/section_ex.md".into(),
            version: "frozen_version".into(), included_chars: 20,
        }];
        for id in ["materials/documents/section_ex.md", "courses/course_one/materials/documents/section_ex.md", "../documents/section_ex.md", "history_only", "invented"] {
            let mut data = raw();
            data["exercises"][0]["sourceIds"] = json!([id]);
            let error = validate(data, &manifest, "run_sources").unwrap_err();
            assert_eq!(error.code, "INVALID_MODEL_OUTPUT");
            assert_eq!(error.details.unwrap()["recoverable"], false);
        }
        assert!(validate(raw(), &manifest, "run_sources").is_ok());
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

#[cfg(test)]
mod issue_four_regression {
    use super::*;
    #[test]
    fn three_questions_with_two_missing_explanations_preserve_absence_and_validate_sources() {
        // Synthetic text with the reported structure; no private course text or source ID.
        let source = "synthetic_dialectic";
        let manifest = vec![ManifestItem {
            id: source.into(),
            path: "synthetic.md".into(),
            version: "v1".into(),
            included_chars: 10,
        }];
        let criteria = |points: [u32; 4]| json!({"criteria": points.iter().enumerate().map(|(i,p)| json!({"id":format!("c{i}"),"label":"概念", "description":"说明判断依据", "maxPoints":p})).collect::<Vec<_>>()});
        let data = json!({"summary":"概念练习","exercises":[
            {"question":"概念辨析", "format":"exact", "answers":["保留与克服"],"explanation":"合成解析", "tags":[],"sourceIds":[source]},
            {"question":"解释概念", "format":"self", "answers":["合成参考"],"rubric":criteria([20,30,30,20]),"tags":[],"sourceIds":[source]},
            {"question":"迁移应用", "format":"self", "answers":["合成参考"],"rubric":criteria([25,25,25,25]),"tags":[],"sourceIds":[source]}
        ]});
        let valid = validate(data.clone(), &manifest, "reported-shape").unwrap();
        assert!(valid["exercises"][1].get("explanation").is_none());
        assert!(valid["exercises"][2].get("explanation").is_none());
        assert_eq!(render(&valid).unwrap().matches("尚未提供解析。").count(), 2);
        assert!(validate(data.clone(), &[], "no-authorization")
            .unwrap_err()
            .message
            .contains("sourceIds"));
        let mut wrong = data;
        wrong["exercises"][1]["explanation"] = json!(42);
        assert!(validate(wrong, &manifest, "bad-type")
            .unwrap_err()
            .message
            .contains("exercises[2].explanation"));
    }
}

/// The packaged learning-exercises guidance is a live contract: its fenced practice-output JSON
/// samples must pass the real model-facing validator, with tool-input pairs staying a distinct shape.
#[cfg(test)]
mod skill_sample_contracts {
    use super::*;
    use std::{fs, path::Path};
    fn guidance_json_samples() -> Vec<Value> {
        let content = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills/learning-exercises/SKILL.md"),
        )
        .expect("packaged SKILL.md must exist next to the crate");
        let mut samples = Vec::new();
        let mut block: Option<String> = None;
        for line in content.lines() {
            let trimmed = line.trim();
            if block.is_none() {
                if trimmed == "```json" {
                    block = Some(String::new());
                }
                continue;
            }
            if trimmed == "```" {
                let text = block.take().expect("closed fence keeps its buffer");
                samples.push(serde_json::from_str(text.trim()).expect("fenced sample is JSON"));
                continue;
            }
            let buffer = block.as_mut().expect("open fence keeps its buffer");
            buffer.push_str(line);
            buffer.push('\n');
        }
        assert!(block.is_none(), "SKILL.md leaves no unclosed json fence");
        samples
    }
    #[test]
    fn practice_output_samples_validate_with_covering_manifest_and_reject_boundary_abuse() {
        let samples = guidance_json_samples();
        // Tool inputs carry requirements; practice products must not, proving the pair stays split.
        let products: Vec<&Value> = samples
            .iter()
            .filter(|sample| sample.get("requirements").is_none())
            .collect();
        let tool_inputs: Vec<&Value> = samples
            .iter()
            .filter(|sample| sample.get("requirements").is_some())
            .collect();
        assert_eq!(products.len(), 2);
        assert_eq!(tool_inputs.len(), 2);
        assert_eq!(
            products
                .iter()
                .map(|product| product["exercises"][0]["format"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["exact", "self"]
        );
        for input in &tool_inputs {
            assert!(
                validate((*input).clone(), &[], "skill-sample-contract")
                    .unwrap_err()
                    .message
                    .contains("schema"),
                "tool input must not double as a practice product"
            );
        }
        // One fixture manifest covers every demo sourceId the samples declare.
        let mut sources = BTreeSet::new();
        for product in &products {
            for question in product["exercises"].as_array().unwrap() {
                for id in question["sourceIds"].as_array().unwrap() {
                    sources.insert(id.as_str().unwrap().to_owned());
                }
            }
        }
        assert!(sources.contains("src_demo_material") && sources.contains("src_demo_note"));
        let manifest: Vec<ManifestItem> = sources
            .iter()
            .map(|id| ManifestItem {
                id: id.clone(),
                path: format!("{id}.md"),
                version: "v1".into(),
                included_chars: 100,
            })
            .collect();
        for product in &products {
            let valid = validate((*product).clone(), &manifest, "skill-sample-contract").unwrap();
            for question in valid["exercises"].as_array().unwrap() {
                assert!(question["id"].as_str().unwrap().starts_with("q_"));
                assert_eq!(question["answerMatch"], "trim");
            }
        }
        assert!(products[1]["exercises"][0].get("rubric").is_some());
        // Boundary abuse is rejected: a source outside this round's authorized manifest...
        let mut hijacked = (*products[0]).clone();
        hijacked["exercises"][0]["sourceIds"] = json!(["src_not_in_manifest"]);
        let error = validate(hijacked, &manifest, "skill-sample-negative").unwrap_err();
        assert_eq!(error.code, "INVALID_MODEL_OUTPUT");
        assert!(error.message.contains("sourceIds"));
        assert!(!error.message.contains("src_not_in_manifest"));
        // ...and an overlong field beyond the fixed length contract.
        let mut overlong = (*products[0]).clone();
        overlong["exercises"][0]["tags"] = json!(["x".repeat(65)]);
        let error = validate(overlong, &manifest, "skill-sample-negative").unwrap_err();
        assert_eq!(error.code, "INVALID_MODEL_OUTPUT");
        assert!(error.message.contains("tags"));
    }
}
