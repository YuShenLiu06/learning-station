//! Structured whole-question edits. The immutable session ledger remains grading authority.
use crate::{content::ContentStore, exercises, sessions, types::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuestionChange {
    pub id: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub status: String,
    pub operation: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Review {
    pub document_id: String,
    pub version: String,
    pub changes: Vec<QuestionChange>,
}
fn conflict() -> CoreError { CoreError::new("CONFLICT", "练习版本已变化；原题与建议保留，请重新读取后提出修改。") }
pub(crate) fn payload(store: &mut ContentStore, id: &str) -> Result<Value> {
    let loaded = exercises::get(store, id)?;
    Ok(json!({"exercises":loaded.exercises["exercises"]}))
}
pub(crate) fn prepare(store: &mut ContentStore, id: &str, content: &str, proposal: &str) -> Result<Review> {
    let loaded = exercises::get(store, id)?;
    let proposed: Value = serde_json::from_str(content).map_err(|_| CoreError::new("INVALID_INPUT", "练习修改请写入 read 返回的 {exercises:[...]} 结构；选项写在 question 中，整题保留 id，新增题省略 id。"))?;
    if proposed.as_object().is_none_or(|o| o.len() != 1 || !o.contains_key("exercises")) { return Err(conflict()); }
    let old = loaded.exercises["exercises"].as_array().ok_or_else(conflict)?;
    let next = proposed["exercises"].as_array().filter(|a| !a.is_empty() && a.len() <= 5).ok_or_else(|| CoreError::new("INVALID_INPUT", "建议题集必须包含1至5道题。"))?;
    let mut data = loaded.exercises.clone();
    let mut seen = std::collections::BTreeSet::new();
    let mut changes = Vec::new();
    let mut normalized = Vec::new();
    for (index, question) in next.iter().enumerate() {
        let before = if let Some(id) = question.get("id") {
            let id = id.as_str().ok_or_else(conflict)?;
            if !seen.insert(id.to_owned()) { return Err(conflict()); }
            Some(old.iter().find(|q| q["id"].as_str() == Some(id)).ok_or_else(conflict)?.clone())
        } else { None };
        let key = before.as_ref().and_then(|q| q["id"].as_str()).map(str::to_owned).unwrap_or_else(|| format!("new_{index}"));
        let operation = format!("exedit_{}", &hash(&format!("{proposal}:{key}"))[..32]);
        let mut after = question.clone();
        if !after.is_object() { return Err(conflict()); }
        if before.as_ref() != Some(question) { after["id"] = json!(format!("q_{}", &hash(&operation)[..32])); }
        after["answerMatch"] = json!("trim");
        normalized.push(after.clone());
        if before.as_ref() != Some(question) {
            changes.push(QuestionChange { id:key, before, after:Some(after), status:"pending".into(), operation });
        }
    }
    for question in old {
        let id = question["id"].as_str().ok_or_else(conflict)?;
        if !seen.contains(id) {
            changes.push(QuestionChange { id:id.into(), before:Some(question.clone()), after:None, status:"pending".into(), operation:format!("exedit_{}", &hash(&format!("{proposal}:{id}"))[..32]) });
        }
    }
    data["exercises"] = json!(normalized);
    exercises::validate_stored(&data, &exercises::stored_manifest(&data, &loaded.session.context_manifest)?, true)?;
    if changes.is_empty() { return Err(CoreError::new("INVALID_INPUT", "题集没有修改，无需创建审阅。")); }
    Ok(Review { document_id:id.into(), version:loaded.document.version, changes })
}
/// Build and validate before recording write intent, including intermediate 1..5 bounds.
pub(crate) fn next(store: &mut ContentStore, review: &Review, index: usize) -> Result<Value> {
    let loaded = exercises::get(store, &review.document_id)?;
    if loaded.document.version != review.version { return Err(conflict()); }
    let change = &review.changes[index];
    let mut data = loaded.exercises;
    let questions = data["exercises"].as_array_mut().ok_or_else(conflict)?;
    if let Some(before) = &change.before {
        let position = questions.iter().position(|q| q == before).ok_or_else(conflict)?;
        match &change.after { Some(after) => questions[position] = after.clone(), None => { questions.remove(position); } }
    } else if let Some(after) = &change.after { questions.push(after.clone()); }
    if questions.is_empty() || questions.len() > 5 { return Err(CoreError::new("INVALID_INPUT", "每次保存须保留1至5道题：请先接受其他新增或删除，再处理此题；本题仍待审阅。")); }
    exercises::validate_stored(&data, &exercises::stored_manifest(&data, &loaded.session.context_manifest)?, true)?;
    Ok(data)
}
pub(crate) fn publish(store: &mut ContentStore, review: &Review, index: usize, mut data: Value) -> Result<Document> {
    let doc = store.read(&review.document_id)?;
    if doc.version != review.version { return Err(conflict()); }
    let origin = exercises::origin(store, &doc)?;
    let original = doc.metadata.extra.get("originalExerciseEntryId").cloned().unwrap_or_else(|| json!(origin.entry_id));
    let operation = &review.changes[index].operation;
    data["revisionDocumentId"] = json!(doc.id);
    data["originalExerciseEntryId"] = original.clone();
    let body = exercises::render(&data)?;
    // Append may be replayed only with identical request digest; original questions and attempts are never changed.
    sessions::append_exercise_revision(store, &origin.session_id, Entry {
        id:String::new(), kind:"exercise".into(), content:body.clone(), created_at:String::new(),
        selection:None, related_ids:vec![], data:Some(data),
    }, operation)?;
    let mut metadata = doc.metadata.clone();
    metadata.extra.insert("originalExerciseEntryId".into(), original);
    metadata.extra.insert("exerciseEntryId".into(), json!(format!("entry_{}", &hash(operation)[..32])));
    metadata.extra.insert("exerciseEditOperation".into(), json!(operation));
    store.save_course_tool(&doc.id, &crate::content::serialize(&metadata, &body)?, &doc.version, operation)
}
