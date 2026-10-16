//! The Pi agent may append explicitly attributed learning records and inspect a
//! bounded set of already-authorized sources. It never receives filesystem access.
use crate::{app::Engine, context, sessions, types::*};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;

const MAX_TOOL_INPUT_BYTES: usize = 32 * 1024;
const MAX_TOOL_RESULT_BYTES: usize = 32 * 1024;
const MAX_RESULTS: usize = 5;
const MAX_EXCERPT_BYTES: usize = 2000;
const AI_NOTICE: &str = "AI整理，待本人确认";

/// Constructed by generation from the real session and context manifest, never
/// accepted from model arguments. There is intentionally no path or write scope.
#[derive(Debug, Clone)]
pub struct ToolScope {
    pub session_id: String,
    pub section_id: String,
    pub run_id: String,
    pub allowed_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RecordQuestion {
    question: String,
    #[serde(default)]
    selection: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateError {
    front: String,
    back: String,
    #[serde(default)]
    cause: Option<String>,
    #[serde(default)]
    exercise_id: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateReviewCard {
    front: String,
    back: String,
    #[serde(default)]
    tags: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateNote {
    content: String,
    #[serde(default)]
    selection: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct QueryKnowledge {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    knowledge_id: Option<String>,
}

enum Operation {
    Record(RecordQuestion),
    Error(CreateError),
    Card(CreateReviewCard),
    Note(CreateNote),
    Query(QueryKnowledge),
}

/// Synchronous local execution: the caller may put this on spawn_blocking. No
/// network or model lock is used; the core lock covers only the bounded local IO.
pub fn execute(
    engine: &Engine,
    scope: &ToolScope,
    call_id: &str,
    name: &str,
    args: Value,
) -> Result<Value> {
    validate_scope(scope, call_id)?;
    if serde_json::to_vec(&args)?.len() > MAX_TOOL_INPUT_BYTES {
        return Err(limit());
    }
    let operation = match name {
        "record_question" => {
            let value: RecordQuestion = decode(args)?;
            bounded(&value.question, 1, 8000)?;
            if let Some(selection) = &value.selection {
                bounded(selection, 0, 12000)?;
            }
            Operation::Record(value)
        }
        "create_error" => {
            let value: CreateError = decode(args)?;
            bounded(&value.front, 1, 8000)?;
            bounded(&value.back, 1, 12000)?;
            if let Some(cause) = &value.cause {
                bounded(cause, 0, 4000)?;
            }
            validate_tags(&value.tags)?;
            if value.exercise_id.as_ref().is_some_and(|id| !valid_id(id)) {
                return Err(invalid());
            }
            Operation::Error(value)
        }
        "create_review_card" => {
            let value: CreateReviewCard = decode(args)?;
            bounded(&value.front, 1, 8000)?;
            bounded(&value.back, 1, 12000)?;
            validate_tags(&value.tags)?;
            Operation::Card(value)
        }
        "create_note" => {
            let value: CreateNote = decode(args)?;
            bounded(&value.content, 1, 20000)?;
            if let Some(selection) = &value.selection {
                bounded(selection, 0, 12000)?;
            }
            Operation::Note(value)
        }
        "query_related_knowledge" => {
            let value: QueryKnowledge = decode(args)?;
            if let Some(query) = &value.query {
                bounded(query, 1, 500)?;
            }
            if value.knowledge_id.as_ref().is_some_and(|id| !valid_id(id)) {
                return Err(invalid());
            }
            if value.query.is_none() && value.knowledge_id.is_none() {
                return Err(invalid());
            }
            Operation::Query(value)
        }
        _ => return Err(CoreError::new("TOOL_NOT_ALLOWED", "未启用此学习工具。")),
    };
    let mut core = engine.lock()?;
    let session = sessions::get(&mut core.content, &scope.session_id)?;
    if session.section_id != scope.section_id {
        return Err(out_of_scope());
    }
    let source = core.content.read(&scope.section_id)?;
    if !matches!(source.metadata.kind.as_str(), "section" | "knowledge") {
        return Err(out_of_scope());
    }
    if let Operation::Query(query) = operation {
        return query_knowledge(&core.content, scope, query);
    }
    // A model call ID is opaque. Only its hash participates in the deterministic
    // request ID; it never becomes a filename or a relative path.
    let request_id = format!(
        "pi_tool_{}",
        hash(&serde_json::to_string(&(&scope.run_id, call_id))?)
    );
    let entry_id = format!("entry_{}", &hash(&request_id)[..32]);
    let mut existing_error_id = None;
    let entry = match operation {
        Operation::Record(value) => Entry {
            id: String::new(),
            kind: "note".into(),
            content: format!("{AI_NOTICE} · 待解问题\n\n{}", value.question),
            created_at: String::new(),
            selection: value.selection,
            related_ids: vec![scope.section_id.clone()],
            data: Some(
                json!({"question":true,"origin":"pi-tool","needsConfirmation":true,"runId":scope.run_id}),
            ),
        },
        Operation::Card(value) => {
            let back = format!("{AI_NOTICE}\n\n{}", value.back);
            Entry {
                id: String::new(),
                kind: "note".into(),
                content: format!("{AI_NOTICE} · 复习卡\n\n{}\n\n{}", value.front, back),
                created_at: String::new(),
                selection: None,
                related_ids: vec![scope.section_id.clone()],
                data: Some(
                    json!({"reviewCard":true,"front":value.front,"back":back,"tags":value.tags,
                    "origin":"pi-tool","needsConfirmation":true,"runId":scope.run_id}),
                ),
            }
        }
        Operation::Note(value) => Entry {
            id: String::new(),
            kind: "note".into(),
            content: format!("{AI_NOTICE} · 学习笔记\n\n{}", value.content),
            created_at: String::new(),
            selection: value.selection,
            related_ids: vec![scope.section_id.clone()],
            data: Some(json!({"origin":"pi-tool","needsConfirmation":true,"runId":scope.run_id})),
        },
        Operation::Error(value) => {
            if let Some(exercise_id) = &value.exercise_id {
                let belongs = session
                    .entries
                    .iter()
                    .filter(|entry| entry.kind == "exercise")
                    .filter_map(|entry| entry.data.as_ref())
                    .filter_map(|data| data.get("exercises"))
                    .filter_map(Value::as_array)
                    .flatten()
                    .any(|exercise| {
                        exercise.get("id").and_then(Value::as_str) == Some(exercise_id.as_str())
                    });
                if !belongs {
                    return Err(CoreError::new(
                        "NOT_FOUND",
                        "当前学习记录中不存在该练习，未写入错题。",
                    ));
                }
            }
            // Preserve the outcome of a previous execution of this exact call.
            // Otherwise a retry of a newly-created error would see itself as an
            // existing error and incorrectly change its kind/digest to a note.
            let previous_call = session.entries.iter().find(|entry| entry.id == entry_id);
            existing_error_id = if let Some(previous) = previous_call {
                previous.data.as_ref().and_then(|data| {
                    (data.get("origin").and_then(Value::as_str) == Some("pi-tool")
                        && data.get("errorSuggestion").and_then(Value::as_bool) == Some(true))
                    .then(|| {
                        data.get("sourceErrorId")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .flatten()
                })
            } else {
                value.exercise_id.as_ref().and_then(|exercise_id| {
                    session
                        .entries
                        .iter()
                        .rev()
                        .find(|entry| {
                            entry.kind == "error"
                                && entry
                                    .data
                                    .as_ref()
                                    .and_then(|data| data.get("exerciseId"))
                                    .and_then(Value::as_str)
                                    == Some(exercise_id.as_str())
                        })
                        .map(|entry| entry.id.clone())
                })
            };
            let suggested_cause = value.cause.unwrap_or_default();
            let back = if suggested_cause.trim().is_empty() {
                format!("{AI_NOTICE}\n\n{}", value.back)
            } else {
                format!(
                    "{AI_NOTICE}\n\n{}\n\n建议错因（待确认）：{}",
                    value.back, suggested_cause
                )
            };
            let mut data = json!({"front":value.front,"back":back,"cause":"","suggestedCause":suggested_cause,
                "tags":value.tags,"origin":"pi-tool","needsConfirmation":true,"causeConfirmed":false,"runId":scope.run_id});
            if let Some(exercise_id) = value.exercise_id {
                data["exerciseId"] = json!(exercise_id);
            }
            let mut related_ids = vec![scope.section_id.clone()];
            if let Some(source_id) = &existing_error_id {
                data["errorSuggestion"] = json!(true);
                data["sourceErrorId"] = json!(source_id);
                related_ids.push(source_id.clone());
            }
            Entry {
                id: String::new(),
                // Review and CSV select the newest error for an exercise. A
                // pending AI suggestion must never displace an existing card or
                // hide the user's confirmed cause/reference answer.
                kind: if existing_error_id.is_some() {
                    "note"
                } else {
                    "error"
                }
                .into(),
                content: format!("{AI_NOTICE}\n\n{}\n\n{}", value.front, back),
                created_at: String::new(),
                selection: None,
                related_ids,
                data: Some(data),
            }
        }
        Operation::Query(_) => unreachable!(),
    };
    let saved = sessions::append(
        &mut core.content,
        &scope.session_id,
        entry,
        Some(&session.version),
        &request_id,
    )?;
    if !saved.entries.iter().any(|entry| entry.id == entry_id) {
        return Err(CoreError::new(
            "IO",
            "学习记录尚未确认写入，不能报告保存成功。",
        ));
    }
    // Read the actual persisted document to supply a real library-relative path.
    let document = core.content.read(&entry_id)?;
    let mut receipt = json!({"persisted":true,"entryId":entry_id,"sessionId":scope.session_id,
        "sectionId":scope.section_id,"path":document.path,"needsConfirmation":true,
        "kind":document.metadata.extra.get("entryKind")});
    if let Some(source_id) = existing_error_id {
        receipt["existingErrorId"] = json!(source_id);
    }
    Ok(receipt)
}

fn query_knowledge(
    store: &crate::content::ContentStore,
    scope: &ToolScope,
    query: QueryKnowledge,
) -> Result<Value> {
    let mut allowed: BTreeSet<&str> = scope.allowed_ids.iter().map(String::as_str).collect();
    allowed.insert(&scope.section_id);
    if query
        .knowledge_id
        .as_ref()
        .is_some_and(|id| !allowed.contains(id.as_str()))
    {
        return Err(out_of_scope());
    }
    let context = context::load(store, &scope.section_id, None)?;
    if query
        .knowledge_id
        .as_ref()
        .is_some_and(|id| !context.manifest.iter().any(|item| &item.id == id))
    {
        return Err(out_of_scope());
    }
    let needle = query.query.as_deref().map(str::to_lowercase);
    let mut results = Vec::<Value>::new();
    let mut manifest = Vec::<ManifestItem>::new();
    let mut truncated = context.truncated;
    for item in context.manifest {
        if !allowed.contains(item.id.as_str())
            || query.knowledge_id.as_ref().is_some_and(|id| id != &item.id)
        {
            continue;
        }
        let document = store.read(&item.id)?;
        if !matches!(document.metadata.kind.as_str(), "section" | "knowledge") {
            continue;
        }
        if document.version != item.version {
            return Err(CoreError::new(
                "CONFLICT",
                "关联资料已在查询期间更新，请重新查询。",
            ));
        }
        // Search and expose only the prefix included by the bounded context
        // loader; a query cannot reach omitted material beyond its budget.
        let included: String = document.content.chars().take(item.included_chars).collect();
        let lower = included.to_lowercase();
        if needle
            .as_ref()
            .is_some_and(|needle| !lower.contains(needle))
        {
            continue;
        }
        if results.len() == MAX_RESULTS {
            truncated = true;
            break;
        }
        let excerpt = excerpt(&included, query.query.as_deref());
        let count = excerpt.chars().count();
        results.push(json!({"id":document.id,"path":document.path,"title":prefix(&document.metadata.title,500),
            "kind":document.metadata.kind,"version":document.version,"excerpt":excerpt}));
        manifest.push(ManifestItem {
            id: item.id,
            path: item.path,
            version: item.version,
            included_chars: count,
        });
        if serde_json::to_vec(&json!({"results":results,"manifest":manifest}))?.len()
            > MAX_TOOL_RESULT_BYTES - 1024
        {
            results.pop();
            manifest.pop();
            truncated = true;
            break;
        }
    }
    Ok(
        json!({"sectionId":scope.section_id,"sessionId":scope.session_id,"results":results,"manifest":manifest,
        "truncated":truncated,"materialRole":"user-reference"}),
    )
}

fn validate_scope(scope: &ToolScope, call_id: &str) -> Result<()> {
    if !valid_id(&scope.session_id)
        || !valid_id(&scope.section_id)
        || !valid_id(&scope.run_id)
        || scope.allowed_ids.len() > 32
        || scope.allowed_ids.iter().any(|id| !valid_id(id))
        || call_id.is_empty()
        || call_id.len() > 128
        || call_id.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    Ok(())
}
fn decode<T: serde::de::DeserializeOwned>(args: Value) -> Result<T> {
    serde_json::from_value(args).map_err(|_| invalid())
}
fn validate_tags(tags: &[String]) -> Result<()> {
    if tags.len() > 12 {
        return Err(limit());
    }
    for tag in tags {
        bounded(tag, 1, 64)?;
    }
    Ok(())
}
fn bounded(value: &str, minimum: usize, maximum: usize) -> Result<()> {
    if value.len() > maximum {
        return Err(limit());
    }
    if minimum > 0 && value.trim().len() < minimum {
        return Err(invalid());
    }
    Ok(())
}
fn prefix(value: &str, max_bytes: usize) -> String {
    let mut boundary = value.len().min(max_bytes);
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_owned()
}
fn excerpt(value: &str, query: Option<&str>) -> String {
    // Prefer the exact authored query when present. Case-insensitive matching
    // still returns a real prefix, never a synthesized answer or summary.
    let start = query.and_then(|query| value.find(query)).unwrap_or(0);
    prefix(&value[start..], MAX_EXCERPT_BYTES)
}
fn invalid() -> CoreError {
    CoreError::new(
        "INVALID_TOOL_INPUT",
        "学习工具参数无效、含未知字段或缺少必填内容，未执行。",
    )
}
fn limit() -> CoreError {
    CoreError::new(
        "LIMIT_EXCEEDED",
        "学习工具输入或结果超过受控预算，未执行额外写入。",
    )
}
fn out_of_scope() -> CoreError {
    CoreError::new(
        "TOOL_SCOPE",
        "学习工具只能操作当前会话并查询本轮已授权的关联材料。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, sync::Arc};

    fn fixture() -> (tempfile::TempDir, Arc<Engine>, ToolScope, String) {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(
            directory.path().join("library"),
            directory.path().join("runtime"),
        )
        .unwrap();
        let (scope, original) = {
            let mut core = engine.lock().unwrap();
            let section = core
                .create_section("当前课程", "This is the current lesson.", vec![], vec![])
                .unwrap();
            core.create_section("另一课程", "Other private lesson.", vec![], vec![])
                .unwrap();
            for (id, body) in [
                ("knowledge", "Related knowledge about since and for."),
                ("unrelated", "OUTSIDE_SCOPE_PRIVATE_TEXT"),
            ] {
                let metadata = Metadata {
                    id: id.into(),
                    title: id.into(),
                    kind: "knowledge".into(),
                    goals: vec![],
                    tags: vec![],
                    links: vec![],
                    version: 1,
                    extra: BTreeMap::new(),
                };
                core.content
                    .create(metadata, body, &format!("knowledge/{id}.md"))
                    .unwrap();
            }
            let mut metadata = section.metadata.clone();
            metadata.links = vec!["knowledge".into()];
            let content = crate::content::serialize(&metadata, &section.body).unwrap();
            let updated = core
                .content
                .save(&section.id, &content, &section.version)
                .unwrap();
            core.refresh().unwrap();
            let (session, context) = core.start_session(&section.id, false).unwrap();
            let scope = ToolScope {
                session_id: session.id,
                section_id: section.id,
                run_id: "run_tool_fixture".into(),
                allowed_ids: context.manifest.into_iter().map(|item| item.id).collect(),
            };
            (scope, updated.content)
        };
        (directory, engine, scope, original)
    }

    #[test]
    fn question_is_real_markdown_after_reopen_without_model_or_course_changes() {
        let (directory, engine, scope, original) = fixture();
        assert!(!engine.state().unwrap().model.configured);
        let saved = execute(
            &engine,
            &scope,
            "question-call",
            "record_question",
            json!({"question":"Why use since here?","selection":"since Monday"}),
        )
        .unwrap();
        assert_eq!(saved["persisted"], true);
        let relative = saved["path"].as_str().unwrap();
        assert!(!std::path::Path::new(relative).is_absolute());
        let file =
            std::fs::read_to_string(directory.path().join("library").join(relative)).unwrap();
        assert!(file.contains("Why use since here?"));
        assert!(file.contains(AI_NOTICE));
        assert_eq!(
            engine
                .lock()
                .unwrap()
                .content
                .read(&scope.section_id)
                .unwrap()
                .content,
            original
        );
        drop(engine);
        let reopened = Engine::new(
            directory.path().join("library"),
            directory.path().join("runtime"),
        )
        .unwrap();
        let mut core = reopened.lock().unwrap();
        let session = sessions::get(&mut core.content, &scope.session_id).unwrap();
        assert_eq!(session.entries.len(), 1);
        let entry = &session.entries[0];
        assert_eq!(entry.kind, "note");
        assert_eq!(entry.data.as_ref().unwrap()["question"], true);
        assert_eq!(entry.data.as_ref().unwrap()["needsConfirmation"], true);
        assert_eq!(entry.selection.as_deref(), Some("since Monday"));
    }

    #[test]
    fn retried_call_is_idempotent_and_changed_args_conflict() {
        let (_directory, engine, scope, _) = fixture();
        // Opaque call IDs may look like paths; only a digest enters request/file IDs.
        let first = execute(
            &engine,
            &scope,
            "../../opaque-call",
            "record_question",
            json!({"question":"Why?"}),
        )
        .unwrap();
        let second = execute(
            &engine,
            &scope,
            "../../opaque-call",
            "record_question",
            json!({"question":"Why?"}),
        )
        .unwrap();
        assert_eq!(first, second);
        assert!(!first["path"].as_str().unwrap().contains(".."));
        assert_eq!(
            execute(
                &engine,
                &scope,
                "../../opaque-call",
                "record_question",
                json!({"question":"Different?"})
            )
            .unwrap_err()
            .code,
            "CONFLICT"
        );
        let mut core = engine.lock().unwrap();
        assert_eq!(
            sessions::get(&mut core.content, &scope.session_id)
                .unwrap()
                .entries
                .len(),
            1
        );
    }

    #[test]
    fn cross_section_scope_and_foreign_exercise_are_rejected() {
        let (_directory, engine, scope, _) = fixture();
        let mut foreign_scope = scope.clone();
        foreign_scope.section_id = "unrelated".into();
        assert_eq!(
            execute(
                &engine,
                &foreign_scope,
                "cross-section",
                "record_question",
                json!({"question":"Write outside?"})
            )
            .unwrap_err()
            .code,
            "TOOL_SCOPE"
        );
        assert_eq!(
            execute(
                &engine,
                &scope,
                "foreign-exercise",
                "create_error",
                json!({"front":"Q","back":"A","exerciseId":"foreign_question"})
            )
            .unwrap_err()
            .code,
            "NOT_FOUND"
        );
        let mut core = engine.lock().unwrap();
        assert!(sessions::get(&mut core.content, &scope.session_id)
            .unwrap()
            .entries
            .is_empty());
    }

    #[test]
    fn errors_preserve_ai_attribution_and_validate_current_exercise() {
        let (_directory, engine, scope, original) = fixture();
        {
            let mut core = engine.lock().unwrap();
            sessions::append(&mut core.content, &scope.session_id, Entry { id:String::new(),kind:"exercise".into(),content:"Use since or for?".into(),created_at:String::new(),selection:None,related_ids:vec![],
                data:Some(json!({"exercises":[{"id":"current_question","question":"Use since or for?","answers":["since"],"format":"exact"}]})) },None,"fixture-exercise").unwrap();
        }
        let result = execute(&engine,&scope,"error-call","create_error",json!({"front":"Since or for?","back":"Since refers to the starting point.","cause":"May have confused time point and duration.","exerciseId":"current_question","tags":["time"]})).unwrap();
        let retried = execute(&engine,&scope,"error-call","create_error",json!({"front":"Since or for?","back":"Since refers to the starting point.","cause":"May have confused time point and duration.","exerciseId":"current_question","tags":["time"]})).unwrap();
        assert_eq!(result, retried);
        let mut core = engine.lock().unwrap();
        let saved = sessions::get(&mut core.content, &scope.session_id).unwrap();
        let entry = saved
            .entries
            .iter()
            .find(|entry| entry.id == result["entryId"].as_str().unwrap())
            .unwrap();
        let data = entry.data.as_ref().unwrap();
        assert_eq!(entry.kind, "error");
        assert_eq!(data["cause"], "");
        assert_eq!(data["causeConfirmed"], false);
        assert_eq!(data["needsConfirmation"], true);
        assert!(data["suggestedCause"]
            .as_str()
            .unwrap()
            .contains("May have"));
        assert!(data["back"].as_str().unwrap().contains(AI_NOTICE));
        assert!(!data["back"].as_str().unwrap().contains("我的错因"));
        assert_eq!(
            core.content.read(&scope.section_id).unwrap().content,
            original
        );
        drop(core);
        assert_eq!(
            execute(
                &engine,
                &scope,
                "general-error",
                "create_error",
                json!({"front":"General error","back":"A suggestion without an exercise"})
            )
            .unwrap()["persisted"],
            true
        );
    }

    #[test]
    fn pi_card_and_note_persist_reopen_deduplicate_and_export_only_the_card() {
        let (directory, engine, scope, original) = fixture();
        let card_args = json!({"front":"Since Monday","back":"Use since for a starting point.","tags":["time","since"]});
        let note_args = json!({"content":"ORDINARY_NOTE_NOT_A_CARD: compare since with for.","selection":"since Monday"});
        let card = execute(
            &engine,
            &scope,
            "review-card-call",
            "create_review_card",
            card_args.clone(),
        )
        .unwrap();
        let note = execute(
            &engine,
            &scope,
            "ordinary-note-call",
            "create_note",
            note_args.clone(),
        )
        .unwrap();
        assert_eq!(card["persisted"], true);
        assert_eq!(card["kind"], "note");
        assert_eq!(note["persisted"], true);
        assert_eq!(note["kind"], "note");
        assert_eq!(
            execute(
                &engine,
                &scope,
                "review-card-call",
                "create_review_card",
                card_args.clone()
            )
            .unwrap(),
            card
        );
        assert_eq!(
            execute(
                &engine,
                &scope,
                "ordinary-note-call",
                "create_note",
                note_args.clone()
            )
            .unwrap(),
            note
        );
        let mut changed_card = card_args.clone();
        changed_card["back"] = json!("Different card answer");
        let mut changed_note = note_args.clone();
        changed_note["content"] = json!("Different note");
        assert_eq!(
            execute(
                &engine,
                &scope,
                "review-card-call",
                "create_review_card",
                changed_card
            )
            .unwrap_err()
            .code,
            "CONFLICT"
        );
        assert_eq!(
            execute(
                &engine,
                &scope,
                "ordinary-note-call",
                "create_note",
                changed_note
            )
            .unwrap_err()
            .code,
            "CONFLICT"
        );
        {
            let mut core = engine.lock().unwrap();
            let session = sessions::get(&mut core.content, &scope.session_id).unwrap();
            assert_eq!(session.entries.len(), 2);
            let saved_card = session
                .entries
                .iter()
                .find(|entry| entry.id == card["entryId"].as_str().unwrap())
                .unwrap();
            assert_eq!(saved_card.data.as_ref().unwrap()["front"], "Since Monday");
            assert!(saved_card.data.as_ref().unwrap()["back"]
                .as_str()
                .unwrap()
                .contains(AI_NOTICE));
            let saved_note = session
                .entries
                .iter()
                .find(|entry| entry.id == note["entryId"].as_str().unwrap())
                .unwrap();
            assert!(saved_note
                .data
                .as_ref()
                .unwrap()
                .get("reviewCard")
                .is_none());
            assert_eq!(saved_note.selection.as_deref(), Some("since Monday"));
            assert!(saved_note.content.contains(AI_NOTICE));
            let review = sessions::review_items(&mut core.content, "").unwrap();
            assert_eq!(review.len(), 1);
            assert_eq!(review[0]["source"], "AI复习（待确认）");
            assert_eq!(review[0]["front"], "Since Monday");
            let (path, count) = sessions::export_csv(
                &mut core.content,
                &scope.session_id,
                &directory.path().join("pi-card-export"),
            )
            .unwrap();
            let csv = std::fs::read_to_string(path).unwrap();
            assert_eq!(count, 1);
            assert!(csv.contains("Since Monday"));
            assert!(csv.contains(AI_NOTICE));
            assert!(!csv.contains("ORDINARY_NOTE_NOT_A_CARD"));
            assert_eq!(
                core.content.read(&scope.section_id).unwrap().content,
                original
            );
        }
        drop(engine);
        let reopened = Engine::new(
            directory.path().join("library"),
            directory.path().join("runtime"),
        )
        .unwrap();
        assert_eq!(
            execute(
                &reopened,
                &scope,
                "review-card-call",
                "create_review_card",
                card_args
            )
            .unwrap(),
            card
        );
        assert_eq!(
            execute(
                &reopened,
                &scope,
                "ordinary-note-call",
                "create_note",
                note_args
            )
            .unwrap(),
            note
        );
        let mut core = reopened.lock().unwrap();
        let review = sessions::review_items(&mut core.content, "").unwrap();
        assert_eq!(review.len(), 1);
        assert_eq!(review[0]["source"], "AI复习（待确认）");
        // An existing manual card remains explicitly attributed to its author.
        sessions::append(
            &mut core.content,
            &scope.session_id,
            Entry {
                id: String::new(),
                kind: "note".into(),
                content: "Manual card".into(),
                created_at: String::new(),
                selection: None,
                related_ids: vec![],
                data: Some(
                    json!({"reviewCard":true,"front":"Manual front","back":"Manual reference"}),
                ),
            },
            None,
            "manual-card-attribution",
        )
        .unwrap();
        let review = sessions::review_items(&mut core.content, "").unwrap();
        assert_eq!(review.len(), 2);
        assert_eq!(
            review
                .iter()
                .find(|item| item["front"] == "Manual front")
                .unwrap()["source"],
            "手动复习"
        );
    }

    #[test]
    fn ai_suggestion_keeps_confirmed_error_and_reference_in_review_and_csv() {
        let (directory, engine, scope, original) = fixture();
        let (human_error_id, review_before, csv_before) = {
            let mut core = engine.lock().unwrap();
            sessions::append(&mut core.content, &scope.session_id, Entry {
                id:String::new(),kind:"exercise".into(),content:"Choose since or for.".into(),created_at:String::new(),selection:None,related_ids:vec![],
                data:Some(json!({"exercises":[{"id":"confirmed_question","question":"Choose since or for.","answers":["since"],"format":"exact","explanation":"CORRECT_REFERENCE: since starts at a point in time."}]})),
            },None,"manual-exercise-protection").unwrap();
            let attempted = sessions::append(
                &mut core.content,
                &scope.session_id,
                Entry {
                    id: String::new(),
                    kind: "attempt".into(),
                    content: "for".into(),
                    created_at: String::new(),
                    selection: None,
                    related_ids: vec![],
                    data: Some(json!({"exerciseId":"confirmed_question","answer":"for"})),
                },
                None,
                "manual-wrong-attempt",
            )
            .unwrap();
            let prior_error_id = attempted
                .entries
                .iter()
                .find(|entry| entry.kind == "error")
                .unwrap()
                .id
                .clone();
            let corrected = sessions::append(&mut core.content, &scope.session_id, Entry {
                id:String::new(),kind:"error".into(),content:"本人补充的错因".into(),created_at:String::new(),selection:None,related_ids:vec![prior_error_id],
                data:Some(json!({"exerciseId":"confirmed_question","front":"My original question","back":"Must retain original reference","cause":"HUMAN_CONFIRMED: confused duration and time point.","causeConfirmed":true,"tags":["confirmed"]})),
            },None,"human-cause-confirmation").unwrap();
            let human_error = corrected
                .entries
                .iter()
                .find(|entry| {
                    entry.kind == "error"
                        && entry
                            .data
                            .as_ref()
                            .and_then(|data| data.get("causeConfirmed"))
                            .and_then(Value::as_bool)
                            == Some(true)
                })
                .unwrap();
            let id = human_error.id.clone();
            let review = sessions::review_items(&mut core.content, "").unwrap();
            let (path, count) = sessions::export_csv(
                &mut core.content,
                &scope.session_id,
                &directory.path().join("before-export"),
            )
            .unwrap();
            assert_eq!(count, 1);
            (id, review, std::fs::read_to_string(path).unwrap())
        };
        assert!(csv_before.contains("HUMAN_CONFIRMED"));
        assert!(csv_before.contains("CORRECT_REFERENCE"));
        let args = json!({"front":"AI replacement question","back":"AI replacement reference","cause":"AI suggested alternative cause","exerciseId":"confirmed_question","tags":["suggested"]});
        let saved = execute(
            &engine,
            &scope,
            "suggest-existing-error",
            "create_error",
            args.clone(),
        )
        .unwrap();
        assert_eq!(saved["persisted"], true);
        assert_eq!(saved["kind"], "note");
        assert_eq!(saved["existingErrorId"], human_error_id);
        let retried = execute(
            &engine,
            &scope,
            "suggest-existing-error",
            "create_error",
            args.clone(),
        )
        .unwrap();
        assert_eq!(saved, retried);
        let mut changed = args;
        changed["cause"] = json!("Different suggestion using the same call ID");
        assert_eq!(
            execute(
                &engine,
                &scope,
                "suggest-existing-error",
                "create_error",
                changed
            )
            .unwrap_err()
            .code,
            "CONFLICT"
        );
        let mut core = engine.lock().unwrap();
        let session = sessions::get(&mut core.content, &scope.session_id).unwrap();
        let suggestion = session
            .entries
            .iter()
            .find(|entry| entry.id == saved["entryId"].as_str().unwrap())
            .unwrap();
        assert_eq!(suggestion.kind, "note");
        assert!(suggestion.related_ids.contains(&human_error_id));
        assert!(suggestion.content.contains(AI_NOTICE));
        let data = suggestion.data.as_ref().unwrap();
        assert_eq!(data["origin"], "pi-tool");
        assert_eq!(data["errorSuggestion"], true);
        assert_eq!(data["sourceErrorId"], human_error_id);
        assert_eq!(data["cause"], "");
        assert_eq!(data["causeConfirmed"], false);
        let human = session
            .entries
            .iter()
            .find(|entry| entry.id == human_error_id)
            .unwrap();
        assert!(human.data.as_ref().unwrap()["cause"]
            .as_str()
            .unwrap()
            .contains("HUMAN_CONFIRMED"));
        assert_eq!(
            session
                .entries
                .iter()
                .filter(|entry| entry.kind == "note")
                .count(),
            1
        );
        assert_eq!(
            sessions::review_items(&mut core.content, "").unwrap(),
            review_before
        );
        let (path, count) = sessions::export_csv(
            &mut core.content,
            &scope.session_id,
            &directory.path().join("after-export"),
        )
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(std::fs::read_to_string(path).unwrap(), csv_before);
        assert_eq!(
            core.content.read(&scope.section_id).unwrap().content,
            original
        );
    }

    #[test]
    fn query_returns_only_real_authorized_context_with_versions_and_no_writes() {
        let (_directory, engine, mut scope, original) = fixture();
        let response = execute(
            &engine,
            &scope,
            "query-call",
            "query_related_knowledge",
            json!({"query":"since"}),
        )
        .unwrap();
        assert_eq!(response["results"].as_array().unwrap().len(), 1);
        assert_eq!(response["results"][0]["id"], "knowledge");
        assert_eq!(
            response["manifest"][0]["version"],
            engine
                .lock()
                .unwrap()
                .content
                .read("knowledge")
                .unwrap()
                .version
        );
        assert!(response["results"][0]["excerpt"]
            .as_str()
            .unwrap()
            .contains("since"));
        assert!(!response.to_string().contains("OUTSIDE_SCOPE_PRIVATE_TEXT"));
        assert_eq!(
            execute(
                &engine,
                &scope,
                "query-outside",
                "query_related_knowledge",
                json!({"knowledgeId":"unrelated"})
            )
            .unwrap_err()
            .code,
            "TOOL_SCOPE"
        );
        // Even a broad allowed-ID list cannot make an unrelated source part of the current context.
        scope.allowed_ids.push("unrelated".into());
        assert_eq!(
            execute(
                &engine,
                &scope,
                "query-not-linked",
                "query_related_knowledge",
                json!({"knowledgeId":"unrelated"})
            )
            .unwrap_err()
            .code,
            "TOOL_SCOPE"
        );
        scope.allowed_ids.retain(|id| id != "knowledge");
        let restricted = execute(
            &engine,
            &scope,
            "query-filter",
            "query_related_knowledge",
            json!({"query":"since"}),
        )
        .unwrap();
        assert!(restricted["results"].as_array().unwrap().is_empty());
        let mut core = engine.lock().unwrap();
        assert!(sessions::get(&mut core.content, &scope.session_id)
            .unwrap()
            .entries
            .is_empty());
        assert_eq!(
            core.content.read(&scope.section_id).unwrap().content,
            original
        );
    }

    #[test]
    fn unknown_fields_paths_tools_and_excessive_input_never_write() {
        let (_directory, engine, scope, _) = fixture();
        for (name, args) in [
            (
                "create_review_card",
                json!({"front":"F","back":"B","path":"../../outside.md"}),
            ),
            ("create_note", json!({"content":"N","reviewCard":true})),
            (
                "record_question",
                json!({"question":"Q","path":"../../outside.md"}),
            ),
            (
                "create_error",
                json!({"front":"Q","back":"A","causeConfirmed":true}),
            ),
            (
                "query_related_knowledge",
                json!({"knowledgeId":"../private"}),
            ),
            ("query_related_knowledge", json!({"path":"/tmp/private"})),
            ("record_question", json!({"question":"Q","apiKey":"secret"})),
        ] {
            assert_eq!(
                execute(&engine, &scope, "malicious-call", name, args)
                    .unwrap_err()
                    .code,
                "INVALID_TOOL_INPUT"
            );
        }
        assert_eq!(
            execute(&engine, &scope, "arbitrary-tool", "write_file", json!({}))
                .unwrap_err()
                .code,
            "TOOL_NOT_ALLOWED"
        );
        assert_eq!(
            execute(
                &engine,
                &scope,
                &"x".repeat(129),
                "record_question",
                json!({"question":"Q"})
            )
            .unwrap_err()
            .code,
            "INVALID_TOOL_INPUT"
        );
        assert_eq!(
            execute(
                &engine,
                &scope,
                "large",
                "record_question",
                json!({"question":"x".repeat(8001)})
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
        assert_eq!(
            execute(
                &engine,
                &scope,
                "many-tags",
                "create_error",
                json!({"front":"Q","back":"A","tags":vec!["tag";13]})
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
        let mut core = engine.lock().unwrap();
        assert!(sessions::get(&mut core.content, &scope.session_id)
            .unwrap()
            .entries
            .is_empty());
    }
}
