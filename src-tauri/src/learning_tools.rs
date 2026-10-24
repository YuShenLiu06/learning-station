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
struct CreateTodo {
    title: String,
    #[serde(default)]
    due_at: Option<String>,
    #[serde(default)]
    relative_minutes: Option<u32>,
    /// Optional explicit reminder opt-in; absence keeps reminders off.
    #[serde(default)]
    reminder_enabled: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct QueryKnowledge {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    knowledge_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LoadSkill {
    skill_id: String,
}

enum Operation {
    Record(RecordQuestion),
    Error(CreateError),
    Card(CreateReviewCard),
    Note(CreateNote),
    Todo(CreateTodo),
    Query(QueryKnowledge),
    Exercises(Value),
    ListSkills,
    LoadSkill(String),
}

/// A model can retry a write with a new run/call ID after interruption. Anchor
/// current writes to the real saved question and exact arguments instead. Old
/// runs without a user binding retain their original operation IDs.
pub(crate) fn operation_id(
    engine: &Engine,
    scope: &ToolScope,
    call_id: &str,
    name: &str,
    args: &Value,
) -> Result<(String, Option<String>)> {
    let user = match crate::generation::get(engine, &scope.run_id) {
        Ok(run) => {
            if run.session_id != scope.session_id {
                return Err(out_of_scope());
            }
            run.user_entry_id
        }
        Err(error) if error.code == "NOT_FOUND" => None,
        Err(error) => return Err(error),
    };
    let digest = match &user {
        Some(user) => hash(&serde_json::to_string(&json!([
            "saved-question-v1",
            scope.session_id,
            user,
            name,
            args
        ]))?),
        None => hash(&serde_json::to_string(&(&scope.run_id, call_id))?),
    };
    Ok((format!("pi_tool_{digest}"), user))
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
    let arguments_hash = hash(&serde_json::to_string(&args)?);
    let (request_id, business_user) = operation_id(engine, scope, call_id, name, &args)?;
    let operation = match name {
        "generate_exercises" => {
            let object = args.as_object().ok_or_else(invalid)?;
            if object
                .keys()
                .any(|k| !["requirements", "exercises", "summary"].contains(&k.as_str()))
            {
                return Err(invalid());
            }
            // The declared schema, the sidecar preflight and the correction notice
            // sent back to the model all count characters; measuring bytes here
            // would reject valid Chinese requirements after every character-based
            // gate already accepted them.
            let requirements = args["requirements"].as_str().ok_or_else(invalid)?;
            if requirements.trim().is_empty() {
                return Err(invalid());
            }
            if requirements.chars().count() > 8000 {
                return Err(limit());
            }
            Operation::Exercises(args)
        }
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
            bounded(&value.content, 1, 12000)?;
            if let Some(selection) = &value.selection {
                bounded(selection, 0, 12000)?;
            }
            Operation::Note(value)
        }
        "create_todo" => {
            let value: CreateTodo = decode(args)?;
            bounded(&value.title, 1, 500)?;
            if let Some(due_at) = &value.due_at {
                bounded(due_at, 1, 64)?;
                chrono::DateTime::parse_from_rfc3339(due_at).map_err(|_| invalid())?;
            }
            if value
                .relative_minutes
                .is_some_and(|n| !(1..=525600).contains(&n))
            {
                return Err(invalid());
            }
            if value.reminder_enabled == Some(true)
                && value.due_at.is_none()
                && value.relative_minutes.is_none()
            {
                return Err(CoreError::new(
                    "INVALID_TOOL_INPUT",
                    "启用提醒需要同时提供提醒时间。",
                ));
            }
            Operation::Todo(value)
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
        "list_learning_skills" => {
            // No parameters and no path surface: the catalog is fixed and bundled.
            if !args.as_object().is_some_and(|object| object.is_empty()) {
                return Err(invalid());
            }
            Operation::ListSkills
        }
        "load_learning_skill" => {
            let value: LoadSkill = decode(args)?;
            if !valid_id(&value.skill_id) {
                return Err(invalid());
            }
            Operation::LoadSkill(value.skill_id)
        }
        _ => return Err(CoreError::new("TOOL_NOT_ALLOWED", "未启用此学习工具。")),
    };
    // Guidance tools are read-only: the idempotency ledger lookup above has no
    // bearing on them, so they return before any business write path.
    if let Operation::ListSkills = operation {
        return Ok(json!({"sessionId":scope.session_id,"runId":scope.run_id,
            "skills":crate::learning_skills::catalog()}));
    }
    if let Operation::LoadSkill(id) = &operation {
        return load_skill(engine, scope, id);
    }
    if name == "generate_exercises" && scope.section_id.is_empty() {
        return Err(CoreError::new("SOURCE_REQUIRED", "请先选择材料再生成练习；未自动指定材料来源。"));
    }
    let explicit_run = if scope.section_id.is_empty() && !scope.allowed_ids.is_empty()
        || name == "generate_exercises"
    {
        let all = engine.runs.lock().map_err(|_| out_of_scope())?;
        let run = all
            .get(&scope.run_id)
            .map(|m| m.run.clone())
            .ok_or_else(out_of_scope)?;
        if run.session_id != scope.session_id
            || run.context_snapshot_version != 1
            || scope
                .allowed_ids
                .iter()
                .any(|id| !run.read_scope_manifest().iter().any(|m| &m.id == id))
        {
            return Err(out_of_scope());
        }
        Some(run)
    } else {
        None
    };
    let request_clock = if let Operation::Todo(ref value) = operation {
        if value.relative_minutes.is_some() || value.reminder_enabled == Some(true) {
            let run = crate::generation::get(engine, &scope.run_id)?;
            if run.session_id != scope.session_id {
                return Err(out_of_scope());
            }
            Some(crate::time_context::TimeContext {
                reference_time: run.reference_time.ok_or_else(invalid)?,
                time_zone: run.time_zone,
            })
        } else {
            None
        }
    } else {
        None
    };
    let focus_run = if matches!(&operation, Operation::Query(_)) { match crate::generation::get(engine, &scope.run_id) { Ok(run) => Some(run), Err(e) if e.code == "NOT_FOUND" => None, Err(e) => return Err(e) } } else { None };
    let mut core = engine.lock()?;
    let session = sessions::get(&mut core.content, &scope.session_id)?;
    if crate::course_sessions::source(&core.content, &session, &scope.section_id).map_err(|_| out_of_scope())? != scope.section_id {
        return Err(out_of_scope());
    }
    if scope.section_id.is_empty() && session.course_id.is_none() {
        // Independent exploration must be explicit in the persisted session;
        // an empty or forged scope cannot silently detach an associated session.
        let document = core.content.read(&scope.session_id)?;
        if document
            .metadata
            .extra
            .get("sectionId")
            .and_then(Value::as_str)
            != Some("")
            || document
                .metadata
                .extra
                .get("associateCurrentSection")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err(out_of_scope());
        }
    } else if !scope.section_id.is_empty() {
        let source = core.content.read(&scope.section_id)?;
        if !matches!(source.metadata.kind.as_str(), "section" | "knowledge") {
            return Err(out_of_scope());
        }
    }
    if let Operation::Query(query) = operation {
        let frozen = if let Some(run) = &focus_run {
            if run.session_id != scope.session_id { return Err(out_of_scope()); }
            if let Some(focus) = &run.focus {
                if run.context_snapshot_version != 1 || focus.source_document_id != scope.section_id
                    || scope.allowed_ids.iter().any(|id| !run.read_scope_manifest().iter().any(|m| &m.id == id)) {
                    return Err(out_of_scope());
                }
            }
            run.focus.as_ref().map(|f| crate::focus::load(&mut core, &session, f)).transpose()?
        } else { None };
        return query_knowledge(&core, &session, scope, query, frozen.as_ref());
    }
    if let Some(user) = &business_user {
        if !session
            .entries
            .iter()
            .any(|e| e.id == *user && e.kind == "user")
        {
            return Err(out_of_scope());
        }
        let entry_id = format!("entry_{}", &hash(&request_id)[..32]);
        if let Some(previous) = session.entries.iter().find(|e| e.id == entry_id) {
            let data = previous.data.as_ref().ok_or_else(out_of_scope)?;
            if data["businessUserEntryId"].as_str() != Some(user.as_str())
                || data["toolArgumentsHash"].as_str() != Some(arguments_hash.as_str())
            {
                return Err(CoreError::new(
                    "CONFLICT",
                    "原工具记录与本次参数不一致，未重复写入。",
                ));
            }
            if name == "generate_exercises" {
                let document = crate::exercises::create(
                    &mut core.content,
                    &session.id,
                    &scope.section_id,
                    data.clone(),
                    &request_id,
                )?;
                return Ok(
                    json!({"persisted":true,"documentId":document.id,"exerciseEntryId":entry_id,
                    "sessionId":session.id,"sectionId":scope.section_id,"replayed":true}),
                );
            }
            let document = core.content.read(&entry_id)?;
            let mut receipt = json!({"persisted":true,"entryId":entry_id,"sessionId":session.id,
                "sectionId":scope.section_id,"path":document.path,"needsConfirmation":true,
                "kind":document.metadata.extra.get("entryKind"),"replayed":true});
            if let Some(source) = data.get("sourceErrorId") {
                receipt["existingErrorId"] = source.clone();
            }
            return Ok(receipt);
        }
    }
    if let Operation::Exercises(args) = &operation {
        let run = explicit_run.as_ref().ok_or_else(out_of_scope)?;
        if run.session_id != scope.session_id {
            return Err(out_of_scope());
        }
        let mut data = crate::exercises::validate(
            json!({"exercises":args["exercises"],"summary":args["summary"]}),
            &run.context_manifest,
            &request_id,
        )?;
        data["contextManifest"] = json!(run.context_manifest);
        data["runId"] = json!(scope.run_id);
        data["promptVersion"] = json!("learning-v2");
        data["requirements"] = args["requirements"].clone();
        if let Some(user) = &business_user {
            data["businessUserEntryId"] = json!(user);
            data["toolArgumentsHash"] = json!(arguments_hash);
        }
        let document = crate::exercises::create_from_snapshot(
            &mut core.content,
            &session.id,
            &scope.section_id,
            data,
            &request_id,
            Some(&run.context_manifest),
        )?;
        return Ok(
            json!({"persisted":true,"documentId":document.id,"exerciseEntryId":document.metadata.extra.get("exerciseEntryId"),"sessionId":session.id,"sectionId":scope.section_id}),
        );
    }
    if let Operation::Todo(value) = operation {
        let due = crate::time_context::resolve_due(
            request_clock.as_ref(),
            value.due_at.as_deref(),
            value.relative_minutes,
        )?;
        let reminder_enabled = value.reminder_enabled.unwrap_or(false);
        if reminder_enabled {
            // Validate the reminder time against the durable run clock in the
            // user's timezone context; a past or unparsable time is rejected.
            let clock = request_clock.as_ref().ok_or_else(invalid)?;
            let due_value = chrono::DateTime::parse_from_rfc3339(
                due.as_deref()
                    .ok_or_else(|| CoreError::new("INVALID_TOOL_INPUT", "启用提醒需要提醒时间。"))?,
            )
            .map_err(|_| invalid())?;
            let reference =
                chrono::DateTime::parse_from_rfc3339(clock.reference_time.as_str())
                    .map_err(|_| invalid())?;
            if due_value <= reference {
                return Err(CoreError::new(
                    "INVALID_TOOL_INPUT",
                    "提醒时间需要晚于当前时间。",
                ));
            }
        }
        let todo = engine.reminders.create_todo(
            &value.title,
            due.as_deref(),
            reminder_enabled,
            Some(&session.id),
            &request_id,
        )?;
        return Ok(
            json!({"persisted":true,"todoId":todo.id,"sessionId":session.id,
            "sectionId":scope.section_id,"dueAt":todo.due_at,"referenceTime":request_clock.as_ref().map(|c|&c.reference_time),"timeZone":request_clock.as_ref().and_then(|c|c.time_zone.as_ref()),"todo":todo}),
        );
    }
    let section_related_ids = if scope.section_id.is_empty() {
        vec![]
    } else {
        vec![scope.section_id.clone()]
    };
    let entry_id = format!("entry_{}", &hash(&request_id)[..32]);
    let mut existing_error_id = None;
    let mut entry = match operation {
        Operation::Record(value) => Entry {
            id: String::new(),
            kind: "note".into(),
            content: format!("{AI_NOTICE} · 待解问题\n\n{}", value.question),
            created_at: String::new(),
            selection: value.selection,
            related_ids: section_related_ids.clone(),
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
                related_ids: section_related_ids.clone(),
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
            related_ids: section_related_ids.clone(),
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
            let mut related_ids = section_related_ids.clone();
            if let Some(source_id) = &existing_error_id {
                data["errorSuggestion"] = json!(true);
                data["sourceErrorId"] = json!(source_id);
                if !scope.section_id.is_empty() {
                    related_ids.push(source_id.clone());
                }
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
        Operation::Query(_) | Operation::Todo(_) | Operation::Exercises(_)
        | Operation::ListSkills | Operation::LoadSkill(_) => unreachable!(),
    };
    if let Some(data) = entry.data.as_mut() { data["sourceDocumentId"] = json!(scope.section_id); }
    if let Some(user) = &business_user {
        let data = entry.data.as_mut().ok_or_else(invalid)?;
        data["businessUserEntryId"] = json!(user);
        data["toolArgumentsHash"] = json!(arguments_hash);
    }
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

/// JSON escaping can inflate control characters roughly six-fold, so a 16KiB
/// body could burst the 32KiB wire budget. Measure the serialized reply with a
/// headroom for the tool_result envelope; an oversized body fails this one
/// call honestly instead of tripping the whole-run TOOL_SIZE_LIMIT afterwards.
fn within_result_budget(reply: &Value) -> bool {
    serde_json::to_vec(reply).is_ok_and(|bytes| bytes.len() + 1024 <= MAX_TOOL_RESULT_BYTES)
}
/// On-demand guidance loading. Only the fixed bundled allowlist is readable;
/// anything else is an authorization rejection without touching the disk. The
/// dedup ledger hangs on the current run: a repeated load returns a short
/// summary instead of a second body, while a new run loads the full text again.
fn load_skill(engine: &Engine, scope: &ToolScope, id: &str) -> Result<Value> {
    if !crate::learning_skills::KNOWN_IDS.contains(&id) {
        return Err(out_of_scope());
    }
    // Guidance already delivered for this run — mode-injected into the system
    // prompt or tool-scoped into a tool description — needs no second body.
    if let Ok(run) = crate::generation::get(engine, &scope.run_id) {
        if let Some(version) = run.learning_skills.iter().find(|version| version.id == id) {
            return Ok(json!({"sessionId":scope.session_id,"runId":scope.run_id,"skillId":id,
                "sha256":version.sha256,"alreadyInjected":true,
                "notice":"该学习指导已随本次模式或工具注册注入指导，正文无需重复加载。"}));
        }
    }
    let mut loaded = engine
        .run_loaded_skills
        .lock()
        .map_err(|_| out_of_scope())?;
    if let Some(sha256) = loaded
        .get(&scope.run_id)
        .and_then(|skills| skills.get(id))
    {
        return Ok(json!({"sessionId":scope.session_id,"runId":scope.run_id,"skillId":id,
            "sha256":sha256,"alreadyLoaded":true,
            "notice":"该学习指导本轮已加载，正文见先前的工具结果。"}));
    }
    let snapshot = crate::learning_skills::load(id)?;
    let reply = json!({"sessionId":scope.session_id,"runId":scope.run_id,"skillId":id,
        "sha256":snapshot.sha256,"content":snapshot.content});
    if !within_result_budget(&reply) {
        return Err(CoreError::new(
            "SKILL_INVALID",
            "学习指导正文超出工具结果预算，本次未返回。",
        ));
    }
    loaded
        .entry(scope.run_id.clone())
        .or_default()
        .insert(id.to_owned(), snapshot.sha256.clone());
    Ok(reply)
}

fn query_knowledge(
    core: &crate::app::Core,
    session: &Session,
    scope: &ToolScope,
    query: QueryKnowledge,
    frozen: Option<&crate::focus::Frozen>,
) -> Result<Value> {
    let store = &core.content;
    if scope.section_id.is_empty() {
        return Ok(
            json!({"sectionId":"","sessionId":scope.session_id,"results":[],"manifest":[],
            "truncated":false,"materialRole":"user-reference"}),
        );
    }
    let mut allowed: BTreeSet<&str> = scope.allowed_ids.iter().map(String::as_str).collect();
    allowed.insert(&scope.section_id);
    if query
        .knowledge_id
        .as_ref()
        .is_some_and(|id| !allowed.contains(id.as_str()))
    {
        return Err(out_of_scope());
    }
    // An authorized catalog is a candidate list, never evidence that its text was sent.
    let catalog = frozen.and_then(|f| f.course_read_manifest.as_ref());
    let (candidates, mut truncated) = if let Some(catalog) = catalog {
        (catalog.clone(), false)
    } else {
        let context = if let Some(frozen) = frozen { frozen.context.clone() } else { context::load(store, &scope.section_id, None)? };
        (context.manifest, context.truncated)
    };
    if query
        .knowledge_id
        .as_ref()
        .is_some_and(|id| !candidates.iter().any(|item| &item.id == id))
    {
        return Err(out_of_scope());
    }
    let needle = query.query.as_deref().map(str::to_lowercase);
    let mut results = Vec::<Value>::new();
    let mut manifest = Vec::<ManifestItem>::new();
    let mut remaining = context::MAX_CONTEXT_CHARS;
    let mut scanned = 0;
    for item in candidates {
        if !allowed.contains(item.id.as_str())
            || query.knowledge_id.as_ref().is_some_and(|id| id != &item.id)
        {
            continue;
        }
        if frozen.is_some() {
            let visible = !frozen.is_some_and(|f| f.asset_path(&item.id).is_some())
                && crate::course_sessions::source(store, session, &item.id).is_ok_and(|id| id == item.id)
                && store.read(&item.id).is_ok_and(|doc| matches!(doc.metadata.kind.as_str(), "section" | "knowledge"));
            if !visible {
                if query.knowledge_id.is_some() { return Err(out_of_scope()); }
                continue;
            }
        }
        if catalog.is_some() && (scanned == context::MAX_CONTEXT_DOCUMENTS || remaining == 0) {
            truncated = true;
            break;
        }
        let document = if let Some(frozen) = frozen { crate::focus::read_document(core, &frozen.focus, &item.id)? } else { store.read(&item.id)? };
        if !matches!(document.metadata.kind.as_str(), "section" | "knowledge") {
            continue;
        }
        if document.version != item.version {
            return Err(CoreError::new(
                "CONFLICT",
                "关联资料已在查询期间更新，请重新查询。",
            ));
        }
        // Catalog zeroes describe prior delivery, not a read budget. Read a fixed
        // bounded prefix; legacy contexts retain their already bounded ranges.
        let limit = if catalog.is_some() { remaining } else { item.included_chars };
        let included: String = document.content.chars().take(limit).collect();
        if catalog.is_some() {
            scanned += 1;
            remaining -= included.chars().count();
            truncated |= included.len() < document.content.len();
        }
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
        || (!scope.section_id.is_empty() && !valid_id(&scope.section_id))
        || !valid_id(&scope.run_id)
        || scope.allowed_ids.len() > 32
        || scope.allowed_ids.iter().any(|id| !valid_id(id))
        || call_id.is_empty()
        || call_id.chars().count() > 256
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
    // The declared schema, the sidecar preflight and the model-side validation
    // all count Unicode characters; measuring bytes here would reject long
    // Chinese content after every character-based gate already accepted it.
    if value.chars().count() > maximum {
        return Err(limit());
    }
    if minimum > 0 && value.trim().chars().count() < minimum {
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

    fn independent_fixture() -> (tempfile::TempDir, Arc<Engine>, ToolScope, String, String) {
        let (directory, engine, associated, original) = fixture();
        let independent_id = "independent_session_fixture";
        {
            let mut core = engine.lock().unwrap();
            let mut extra = BTreeMap::new();
            extra.insert("sectionId".into(), json!(""));
            extra.insert("associateCurrentSection".into(), json!(false));
            extra.insert("createdAt".into(), json!(timestamp()));
            extra.insert("contextManifest".into(), json!([]));
            core.content
                .create(
                    Metadata {
                        id: independent_id.into(),
                        title: "独立探索".into(),
                        kind: "session".into(),
                        goals: vec![],
                        tags: vec![],
                        links: vec![],
                        version: 1,
                        extra,
                    },
                    "独立探索记录。",
                    &format!("sessions/{independent_id}/session.md"),
                )
                .unwrap();
            sessions::get(&mut core.content, independent_id).unwrap();
        }
        let scope = ToolScope {
            session_id: independent_id.into(),
            section_id: String::new(),
            run_id: "run_independent_fixture".into(),
            allowed_ids: vec![],
        };
        (directory, engine, scope, associated.section_id, original)
    }

    #[test]
    fn pi_todos_really_persist_reopen_deduplicate_and_never_enable_reminders() {
        let (directory, engine, scope, original) = fixture();
        let plain_args = json!({"title":"稍后复习 since 与 for"});
        let timed_args = json!({"title":"明天再比较用法","dueAt":"2000-01-01T08:00:00+08:00"});
        let plain = execute(
            &engine,
            &scope,
            "plain-todo",
            "create_todo",
            plain_args.clone(),
        )
        .unwrap();
        let timed = execute(
            &engine,
            &scope,
            "timed-todo",
            "create_todo",
            timed_args.clone(),
        )
        .unwrap();
        assert_eq!(plain["persisted"], true);
        assert_eq!(plain["todoId"], plain["todo"]["id"]);
        assert_eq!(plain["sessionId"], scope.session_id);
        assert_eq!(plain["sectionId"], scope.section_id);
        assert_eq!(plain["todo"]["sessionId"], scope.session_id);
        assert!(plain["todo"]["dueAt"].is_null());
        assert_eq!(timed["todo"]["dueAt"], "2000-01-01T00:00:00.000000000Z");
        for receipt in [&plain, &timed] {
            assert_eq!(receipt["todo"]["reminderEnabled"], false);
        }
        assert_eq!(
            execute(
                &engine,
                &scope,
                "plain-todo",
                "create_todo",
                plain_args.clone()
            )
            .unwrap(),
            plain
        );
        for changed in [
            json!({"title":"另一待办"}),
            json!({"title":"稍后复习 since 与 for","dueAt":"2000-01-02T00:00:00Z"}),
        ] {
            assert_eq!(
                execute(&engine, &scope, "plain-todo", "create_todo", changed)
                    .unwrap_err()
                    .code,
                "CONFLICT"
            );
        }
        assert_eq!(engine.reminders.list_todos().unwrap().len(), 2);
        assert!(engine.reminders.tick().unwrap().is_empty());
        assert!(engine.reminders.list_notifications().unwrap().is_empty());
        {
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
        drop(engine);
        let reopened = Engine::new(
            directory.path().join("library"),
            directory.path().join("runtime"),
        )
        .unwrap();
        assert_eq!(
            execute(&reopened, &scope, "plain-todo", "create_todo", plain_args).unwrap(),
            plain
        );
        assert_eq!(
            execute(&reopened, &scope, "timed-todo", "create_todo", timed_args).unwrap(),
            timed
        );
        assert_eq!(reopened.reminders.list_todos().unwrap().len(), 2);
        assert!(reopened.reminders.tick().unwrap().is_empty());
    }

    #[test]
    fn failed_todo_storage_is_an_error_without_a_success_receipt() {
        let (directory, engine, scope, _) = fixture();
        let connection =
            rusqlite::Connection::open(directory.path().join("runtime/notifications.sqlite3"))
                .unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_tool_todo BEFORE INSERT ON reminder_todos
             BEGIN SELECT RAISE(ABORT,'test-storage-failure'); END;",
            )
            .unwrap();
        assert!(execute(
            &engine,
            &scope,
            "failed-todo",
            "create_todo",
            json!({"title":"不能假称保存"})
        )
        .is_err());
        assert!(engine.reminders.list_todos().unwrap().is_empty());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM reminder_requests", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let mut core = engine.lock().unwrap();
        assert!(sessions::get(&mut core.content, &scope.session_id)
            .unwrap()
            .entries
            .is_empty());
    }

    #[test]
    fn todo_arguments_cannot_override_reminders_ownership_or_scope() {
        let (_directory, engine, scope, _) = fixture();
        for args in [
            json!({"title":"T","reminderEnabled":true}),
            json!({"title":"T","sessionId":"another_session"}),
            json!({"title":"T","sectionId":"another_section"}),
            json!({"title":"T","path":"../../outside.md"}),
            json!({"title":"T","dueAt":""}),
            json!({"title":"T","dueAt":"2000-01-01T00:00:00"}),
            json!({"title":"T","dueAt":"not-a-date"}),
            json!({"title":" "}),
        ] {
            assert_eq!(
                execute(&engine, &scope, "invalid-todo", "create_todo", args)
                    .unwrap_err()
                    .code,
                "INVALID_TOOL_INPUT"
            );
        }
        // An explicit opt-out is a legitimate legacy-compatible call.
        let opted_out = execute(
            &engine,
            &scope,
            "explicit-off-todo",
            "create_todo",
            json!({"title":"T","reminderEnabled":false}),
        )
        .unwrap();
        assert_eq!(opted_out["todo"]["reminderEnabled"], false);
        let mut foreign = scope.clone();
        foreign.section_id = "unrelated".into();
        assert_eq!(
            execute(
                &engine,
                &foreign,
                "cross-section-todo",
                "create_todo",
                json!({"title":"T"})
            )
            .unwrap_err()
            .code,
            "TOOL_SCOPE"
        );
        foreign = scope.clone();
        foreign.session_id = "missing_session".into();
        assert!(execute(
            &engine,
            &foreign,
            "missing-session-todo",
            "create_todo",
            json!({"title":"T"})
        )
        .is_err());
        let todos = engine.reminders.list_todos().unwrap();
        assert_eq!(todos.len(), 1);
        assert!(!todos[0].reminder_enabled);
    }

    #[test]
    fn todo_idempotency_cannot_reuse_another_real_sessions_receipt() {
        let (_directory, engine, scope, _) = fixture();
        let args = json!({"title":"复习"});
        let first = execute(&engine, &scope, "same-call", "create_todo", args.clone()).unwrap();
        let other_session = {
            let mut core = engine.lock().unwrap();
            sessions::create(&mut core.content, &scope.section_id, &[]).unwrap()
        };
        let mut other = scope.clone();
        other.session_id = other_session.id;
        assert_eq!(
            execute(&engine, &other, "same-call", "create_todo", args.clone())
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        other.run_id = "another_run".into();
        let second = execute(&engine, &other, "same-call", "create_todo", args).unwrap();
        assert_ne!(first["todoId"], second["todoId"]);
        assert_eq!(second["todo"]["sessionId"], other.session_id);
        assert_eq!(engine.reminders.list_todos().unwrap().len(), 2);
    }

    #[test]
    fn independent_exploration_tools_do_not_inherit_current_section_sources() {
        let (_directory, engine, scope, current_section, original) = independent_fixture();
        for (call_id, name, args) in [
            (
                "independent-note",
                "create_note",
                json!({"content":"我想单独探索一个问题"}),
            ),
            (
                "independent-question",
                "record_question",
                json!({"question":"这个概念是什么？"}),
            ),
            (
                "independent-card",
                "create_review_card",
                json!({"front":"F","back":"B"}),
            ),
            (
                "independent-error",
                "create_error",
                json!({"front":"Q","back":"A"}),
            ),
        ] {
            let receipt = execute(&engine, &scope, call_id, name, args).unwrap();
            assert_eq!(receipt["persisted"], true);
            assert_eq!(receipt["sectionId"], "");
        }
        for args in [
            json!({"query":"since"}),
            json!({"knowledgeId":"unrelated"}),
            json!({"knowledgeId":current_section}),
        ] {
            let result = execute(
                &engine,
                &scope,
                "independent-query",
                "query_related_knowledge",
                args,
            )
            .unwrap();
            assert_eq!(result["results"], json!([]));
            assert_eq!(result["manifest"], json!([]));
            assert_eq!(result["truncated"], false);
            assert!(!result.to_string().contains("OUTSIDE_SCOPE_PRIVATE_TEXT"));
            assert!(!result.to_string().contains("current lesson"));
        }
        let todo = execute(
            &engine,
            &scope,
            "independent-todo",
            "create_todo",
            json!({"title":"整理独立探索结论"}),
        )
        .unwrap();
        assert_eq!(todo["sectionId"], "");
        assert_eq!(todo["todo"]["sessionId"], scope.session_id);
        assert_eq!(todo["todo"]["reminderEnabled"], false);
        let mut core = engine.lock().unwrap();
        let session = sessions::get(&mut core.content, &scope.session_id).unwrap();
        assert!(session.section_id.is_empty());
        assert!(session.context_manifest.is_empty());
        assert_eq!(session.entries.len(), 4);
        for entry in &session.entries {
            assert!(entry.related_ids.is_empty());
            assert!(core
                .content
                .read(&entry.id)
                .unwrap()
                .metadata
                .links
                .is_empty());
        }
        assert_eq!(
            core.content.read(&current_section).unwrap().content,
            original
        );
    }

    #[test]
    fn independent_scope_requires_explicit_persisted_optout_and_empty_allowed_ids() {
        let (_directory, engine, scope, current_section, _) = independent_fixture();
        let mut contaminated = scope.clone();
        contaminated.allowed_ids.push(current_section.clone());
        for (name, args) in [
            ("query_related_knowledge", json!({"query":"since"})),
            ("create_todo", json!({"title":"T"})),
            ("create_note", json!({"content":"N"})),
        ] {
            assert_eq!(
                execute(&engine, &contaminated, "contaminated-scope", name, args)
                    .unwrap_err()
                    .code,
                "TOOL_SCOPE"
            );
        }
        for associate in [None, Some(json!(true))] {
            {
                let core = engine.lock().unwrap();
                let doc = core.content.read(&scope.session_id).unwrap();
                let mut meta = doc.metadata;
                if let Some(value) = associate {
                    meta.extra.insert("associateCurrentSection".into(), value);
                } else {
                    meta.extra.remove("associateCurrentSection");
                }
                let content = crate::content::serialize(&meta, &doc.body).unwrap();
                // Simulate an external edit: the normal save API deliberately
                // preserves omitted extension keys from older clients.
                std::fs::write(core.content.root().join(&doc.path), content).unwrap();
            }
            assert!(execute(
                &engine,
                &scope,
                "implicit-independent",
                "create_todo",
                json!({"title":"T"})
            )
            .is_err());
        }
        assert!(engine.reminders.list_todos().unwrap().is_empty());
        {
            let mut core = engine.lock().unwrap();
            let doc = core.content.read(&scope.session_id).unwrap();
            let mut meta = doc.metadata;
            meta.extra
                .insert("associateCurrentSection".into(), json!(false));
            let content = crate::content::serialize(&meta, &doc.body).unwrap();
            core.content
                .save(&scope.session_id, &content, &doc.version)
                .unwrap();
        }
        let mut associated_scope = scope.clone();
        associated_scope.section_id = current_section;
        assert_eq!(
            execute(
                &engine,
                &associated_scope,
                "forged-associated",
                "create_note",
                json!({"content":"N"})
            )
            .unwrap_err()
            .code,
            "TOOL_SCOPE"
        );
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
    fn exercise_requirements_are_counted_in_characters_not_bytes() {
        let (_directory, engine, scope, _) = fixture();
        // generate_exercises only runs against a frozen run manifest.
        {
            let manifest: Vec<ManifestItem> = scope
                .allowed_ids
                .iter()
                .map(|id| ManifestItem {
                    id: id.clone(),
                    path: "fixture.md".into(),
                    version: "v1".into(),
                    included_chars: 10,
                })
                .collect();
            let run: crate::generation::Run = serde_json::from_value(json!({
                "id": scope.run_id, "sessionId": scope.session_id, "mode": "question",
                "status": "succeeded", "content": "", "error": null,
                "createdAt": "2026-10-16T15:55:00Z", "updatedAt": "2026-10-16T15:55:00Z",
                "requestDigest": "fixture", "contextManifest": manifest,
                "contextSnapshotVersion": 1,
            }))
            .unwrap();
            engine.runs.lock().unwrap().insert(
                run.id.clone(),
                crate::generation::ManagedRun {
                    run,
                    cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                },
            );
        }
        let args = json!({
            "requirements": "汉".repeat(4000),
            "summary": "合成练习摘要",
            "exercises": [{"question": "合成题目", "format": "exact", "answers": ["合成答案"], "tags": [], "sourceIds": []}],
        });
        let saved = execute(
            &engine,
            &scope,
            "chinese-requirements",
            "generate_exercises",
            args.clone(),
        )
        .unwrap();
        assert_eq!(saved["persisted"], true);
        let retried = execute(
            &engine,
            &scope,
            "chinese-requirements",
            "generate_exercises",
            args,
        )
        .unwrap();
        assert_eq!(retried["documentId"], saved["documentId"]);
        let over = json!({
            "requirements": "汉".repeat(8001),
            "summary": "合成练习摘要",
            "exercises": [{"question": "合成题目", "format": "exact", "answers": ["合成答案"], "tags": [], "sourceIds": []}],
        });
        assert_eq!(
            execute(
                &engine,
                &scope,
                "over-long-requirements",
                "generate_exercises",
                over
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
    }

    #[test]
    fn record_question_fields_are_counted_in_characters_not_bytes() {
        let (_directory, engine, scope, _) = fixture();
        // 8000 Chinese characters pass every character-based gate but weigh
        // 24000 UTF-8 bytes, which the old byte measurement rejected.
        let saved = execute(
            &engine,
            &scope,
            "chinese-question",
            "record_question",
            json!({"question": "问".repeat(8000)}),
        )
        .unwrap();
        assert_eq!(saved["persisted"], true);
        let with_selection = execute(
            &engine,
            &scope,
            "chinese-selection",
            "record_question",
            json!({"question": "短问题", "selection": "选".repeat(9000)}),
        )
        .unwrap();
        assert_eq!(with_selection["persisted"], true);
        assert_eq!(
            execute(
                &engine,
                &scope,
                "over-long-question",
                "record_question",
                json!({"question": "问".repeat(8001)})
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
        assert_eq!(
            execute(
                &engine,
                &scope,
                "over-long-selection",
                "record_question",
                json!({"question": "Q", "selection": "s".repeat(12001)})
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
    }

    #[test]
    fn create_error_fields_are_counted_in_characters_not_bytes() {
        let (_directory, engine, scope, _) = fixture();
        let saved = execute(
            &engine,
            &scope,
            "chinese-error",
            "create_error",
            json!({
                "front": "题".repeat(2700),
                "back": "解".repeat(3000),
                "cause": "因".repeat(1400),
                "tags": ["时间", "标".repeat(22)],
            }),
        )
        .unwrap();
        assert_eq!(saved["persisted"], true);
        assert_eq!(
            execute(
                &engine,
                &scope,
                "over-long-error-front",
                "create_error",
                json!({"front": "题".repeat(8001), "back": "解"})
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
        assert_eq!(
            execute(
                &engine,
                &scope,
                "over-long-error-cause",
                "create_error",
                json!({"front": "题", "back": "解", "cause": "因".repeat(4001)})
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
        assert_eq!(
            execute(
                &engine,
                &scope,
                "over-long-error-tag",
                "create_error",
                json!({"front": "题", "back": "解", "tags": ["标".repeat(65)]})
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
    }

    #[test]
    fn review_card_fields_are_counted_in_characters_not_bytes() {
        let (_directory, engine, scope, _) = fixture();
        let saved = execute(
            &engine,
            &scope,
            "chinese-card",
            "create_review_card",
            json!({
                "front": "面".repeat(2700),
                "back": "背".repeat(3000),
                "tags": ["标".repeat(22)],
            }),
        )
        .unwrap();
        assert_eq!(saved["persisted"], true);
        assert_eq!(
            execute(
                &engine,
                &scope,
                "over-long-card-back",
                "create_review_card",
                json!({"front": "面", "back": "b".repeat(12001)})
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
    }

    #[test]
    fn note_fields_are_counted_in_characters_not_bytes() {
        let (_directory, engine, scope, _) = fixture();
        // The schema layer caps content at 12000 characters while execution
        // used a 20000-byte budget, so this note cleared every character
        // gate and still hit a generic LIMIT_EXCEEDED.
        let saved = execute(
            &engine,
            &scope,
            "chinese-note",
            "create_note",
            json!({"content": "笔".repeat(10000)}),
        )
        .unwrap();
        assert_eq!(saved["persisted"], true);
        assert_eq!(
            execute(
                &engine,
                &scope,
                "over-long-note",
                "create_note",
                json!({"content": "n".repeat(12001)})
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
    }

    #[test]
    fn todo_title_and_due_at_are_counted_in_characters() {
        let (_directory, engine, scope, _) = fixture();
        // The schema caps titles at 500 characters; execution used to keep a
        // wider 2000-byte budget, so the boundary must follow the schema.
        let saved = execute(
            &engine,
            &scope,
            "chinese-todo",
            "create_todo",
            json!({"title": "办".repeat(500)}),
        )
        .unwrap();
        assert_eq!(saved["persisted"], true);
        assert_eq!(
            execute(
                &engine,
                &scope,
                "over-long-todo-title",
                "create_todo",
                json!({"title": "办".repeat(501)})
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
        assert_eq!(
            execute(
                &engine,
                &scope,
                "over-long-todo-due-at",
                "create_todo",
                json!({"title": "办", "dueAt": "x".repeat(65)})
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
    }

    #[test]
    fn knowledge_query_is_counted_in_characters_not_bytes() {
        let (_directory, engine, scope, _) = fixture();
        // 500 Chinese characters are 1500 bytes; the old byte budget
        // rejected them after every character gate accepted the call.
        let found = execute(
            &engine,
            &scope,
            "chinese-query",
            "query_related_knowledge",
            json!({"query": "查".repeat(500)}),
        )
        .unwrap();
        assert!(found["results"].is_array());
        assert_eq!(
            execute(
                &engine,
                &scope,
                "over-long-query",
                "query_related_knowledge",
                json!({"query": "查".repeat(501)})
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
    }

    #[test]
    fn call_id_length_is_counted_in_characters_not_bytes() {
        let (_directory, engine, scope, _) = fixture();
        // The wire preflight accepts call IDs up to 256 characters; the old
        // 128-byte execution gate rejected longer opaque IDs afterwards.
        let saved = execute(
            &engine,
            &scope,
            &"调".repeat(200),
            "record_question",
            json!({"question": "长编号调用"}),
        )
        .unwrap();
        assert_eq!(saved["persisted"], true);
        assert_eq!(
            execute(
                &engine,
                &scope,
                &"x".repeat(257),
                "record_question",
                json!({"question": "超长编号调用"})
            )
            .unwrap_err()
            .code,
            "INVALID_TOOL_INPUT"
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
            ("list_learning_skills", json!({"path":"/tmp/private"})),
            ("load_learning_skill", json!({})),
            ("load_learning_skill", json!({"skillId":"../../skills/secret"})),
            ("load_learning_skill", json!({"skillId":"ok","extra":1})),
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
                &"x".repeat(257),
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

    #[test]
    fn skill_catalog_lists_only_allowlisted_metadata_without_bodies() {
        let (_directory, engine, scope, _) = fixture();
        let listed = execute(
            &engine,
            &scope,
            "list-call",
            "list_learning_skills",
            json!({}),
        )
        .unwrap();
        let skills = listed["skills"].as_array().unwrap();
        assert_eq!(
            skills
                .iter()
                .map(|item| item["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            crate::learning_skills::KNOWN_IDS
        );
        for item in skills {
            assert_eq!(item["name"].as_str().unwrap(), item["id"].as_str().unwrap());
            assert!(!item["description"].as_str().unwrap().trim().is_empty());
        }
        // Metadata only: no frontmatter or body may leak through the listing.
        assert!(!listed.to_string().contains("---\\n"));
        assert!(!listed.to_string().contains("正文"));
    }

    #[test]
    fn skill_load_returns_the_body_and_out_of_list_ids_are_authorization_rejections() {
        let (_directory, engine, scope, _) = fixture();
        let loaded = execute(
            &engine,
            &scope,
            "load-call",
            "load_learning_skill",
            json!({"skillId":"learning-review"}),
        )
        .unwrap();
        let content = loaded["content"].as_str().unwrap();
        assert!(content.starts_with("---\nname: learning-review\n"));
        assert_eq!(loaded["sha256"].as_str().unwrap(), hash(content));
        assert_eq!(loaded["skillId"], "learning-review");
        let format = execute(
            &engine, &scope, "exercise-format-load", "load_learning_skill",
            json!({"skillId":"learning-exercise-markdown"}),
        ).unwrap();
        let body = format["content"].as_str().unwrap();
        assert_eq!(format["skillId"], "learning-exercise-markdown");
        assert_eq!(format["sha256"].as_str().unwrap(), hash(body));
        assert_eq!(body, include_str!("../../skills/learning-exercise-markdown/SKILL.md"));
        // An id outside the fixed allowlist never reaches the disk.
        for (id, code) in [
            ("not-a-skill", "TOOL_SCOPE"),
            ("learning-explore/../learning-review", "INVALID_TOOL_INPUT"),
            ("Skills", "TOOL_SCOPE"),
        ] {
            assert_eq!(
                execute(
                    &engine,
                    &scope,
                    "load-out",
                    "load_learning_skill",
                    json!({"skillId":id})
                )
                .unwrap_err()
                .code,
                code,
                "{id}"
            );
        }
    }

    #[test]
    fn repeated_load_in_one_run_returns_a_notice_but_a_new_run_loads_again() {
        let (_directory, engine, scope, _) = fixture();
        let first = execute(
            &engine,
            &scope,
            "first-load",
            "load_learning_skill",
            json!({"skillId":"learning-explore"}),
        )
        .unwrap();
        assert!(first["content"].is_string());
        let second = execute(
            &engine,
            &scope,
            "second-load",
            "load_learning_skill",
            json!({"skillId":"learning-explore"}),
        )
        .unwrap();
        assert_eq!(second["alreadyLoaded"], true);
        assert_eq!(second["sha256"], first["sha256"]);
        assert!(second.get("content").is_none());
        // A different run is fresh effective context and gets the body again.
        let mut fresh = scope.clone();
        fresh.run_id = "run_skill_reload".into();
        let third = execute(
            &engine,
            &fresh,
            "third-load",
            "load_learning_skill",
            json!({"skillId":"learning-explore"}),
        )
        .unwrap();
        assert!(third["content"].is_string());
        assert_eq!(third["sha256"], first["sha256"]);
    }

    #[test]
    fn guidance_already_injected_for_the_mode_is_not_loaded_again() {
        let (_directory, engine, scope, _) = fixture();
        let run: crate::generation::Run = serde_json::from_value(json!({
            "id": scope.run_id, "sessionId": scope.session_id, "mode": "question",
            "status": "running", "content": "", "error": null,
            "createdAt": "2026-10-17T00:00:00Z", "updatedAt": "2026-10-17T00:00:00Z",
            "requestDigest": "fixture", "contextManifest": [], "contextSnapshotVersion": 1,
            "learningSkills": [
                {"id":"learning-explore","sha256":"ab".repeat(32),"scope":"mode"},
                {"id":"learning-exercises","sha256":"cd".repeat(32),"scope":"tool:generate_exercises"}
            ],
        }))
        .unwrap();
        engine.runs.lock().unwrap().insert(
            run.id.clone(),
            crate::generation::ManagedRun {
                run,
                cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
        );
        let loaded = execute(
            &engine,
            &scope,
            "injected-load",
            "load_learning_skill",
            json!({"skillId":"learning-explore"}),
        )
        .unwrap();
        assert_eq!(loaded["alreadyInjected"], true);
        assert_eq!(loaded["sha256"], "ab".repeat(32));
        assert!(loaded.get("content").is_none());
        // Tool-scoped guidance rides in the tool description every turn, so a
        // second body would duplicate the text already in effective context.
        let exercises = execute(
            &engine,
            &scope,
            "exercises-load",
            "load_learning_skill",
            json!({"skillId":"learning-exercises"}),
        )
        .unwrap();
        assert_eq!(exercises["alreadyInjected"], true);
        assert_eq!(exercises["sha256"], "cd".repeat(32));
        assert!(exercises.get("content").is_none());
        // Guidance the run never received still loads its full body.
        let review = execute(
            &engine,
            &scope,
            "review-load",
            "load_learning_skill",
            json!({"skillId":"learning-review"}),
        )
        .unwrap();
        assert!(review["content"].is_string());
    }

    #[test]
    fn compaction_clears_the_run_dedup_ledger_so_guidance_loads_again() {
        let (_directory, engine, scope, _) = fixture();
        let first = execute(
            &engine,
            &scope,
            "pre-compaction-load",
            "load_learning_skill",
            json!({"skillId":"learning-explore"}),
        )
        .unwrap();
        assert!(first["content"].is_string());
        let notice = execute(
            &engine,
            &scope,
            "repeat-load",
            "load_learning_skill",
            json!({"skillId":"learning-explore"}),
        )
        .unwrap();
        assert_eq!(notice["alreadyLoaded"], true);
        // This mirrors the compaction observer wired in generation.rs: a
        // completed compaction may drop earlier tool results, so the ledger
        // must stop pointing the model at bodies it can no longer see.
        engine
            .run_loaded_skills
            .lock()
            .unwrap()
            .remove(&scope.run_id);
        let after = execute(
            &engine,
            &scope,
            "post-compaction-load",
            "load_learning_skill",
            json!({"skillId":"learning-explore"}),
        )
        .unwrap();
        assert!(after["content"].is_string());
        assert_eq!(after["sha256"], first["sha256"]);
    }

    #[test]
    fn control_character_inflation_stays_inside_the_tool_result_budget() {
        // A 16KiB body of control characters is legal on disk yet serializes
        // to roughly six times its size; the gate must fail this single call
        // honestly instead of letting the wire budget abort the whole run.
        let inflated = json!({"content": "\u{1}".repeat(16 * 1024)});
        assert!(serde_json::to_vec(&inflated).unwrap().len() > MAX_TOOL_RESULT_BYTES);
        assert!(!within_result_budget(&inflated));
        let normal = json!({"content": "普通指导正文"});
        assert!(within_result_budget(&normal));
    }
}
#[cfg(test)]
mod time_todo_tests {
    use super::*;
    #[test]
    fn relative_todo_is_verified_against_durable_run_clock_and_replay_does_not_duplicate() {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::new(d.path().join("library"), d.path().join("state")).unwrap();
        let session = {
            let mut c = e.lock().unwrap();
            let doc = c
                .create_section("fixture", "timing", vec![], vec![])
                .unwrap();
            c.start_session(&doc.id, false).unwrap().0
        };
        let run:crate::generation::Run=serde_json::from_value(json!({"id":"run_relative","sessionId":session.id,"mode":"question","status":"succeeded","content":"","error":null,"createdAt":"2026-10-16T15:55:00Z","updatedAt":"2026-10-16T15:55:00Z","requestDigest":"fixture","contextManifest":[],"referenceTime":"2026-10-16T15:55:00Z","timeZone":"Asia/Shanghai"})).unwrap();
        e.runs.lock().unwrap().insert(
            run.id.clone(),
            crate::generation::ManagedRun {
                run: run.clone(),
                cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
        );
        let scope = ToolScope {
            session_id: session.id,
            section_id: session.section_id,
            run_id: run.id,
            allowed_ids: vec![],
        };
        let args =
            json!({"title":"跨日待办","relativeMinutes":10,"dueAt":"2026-10-17T00:05:00+08:00"});
        let a = execute(&e, &scope, "same-clock", "create_todo", args.clone()).unwrap();
        let b = execute(&e, &scope, "same-clock", "create_todo", args).unwrap();
        assert_eq!(a["todoId"], b["todoId"]);
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(a["dueAt"].as_str().unwrap()).unwrap(),
            chrono::DateTime::parse_from_rfc3339("2026-10-16T16:05:00Z").unwrap()
        );
        assert_eq!(a["referenceTime"], "2026-10-16T15:55:00Z");
        assert_eq!(e.reminders.list_todos().unwrap().len(), 1);
        assert!(!e.reminders.list_todos().unwrap()[0].reminder_enabled);
        assert!(execute(
            &e,
            &scope,
            "wrong-clock",
            "create_todo",
            json!({"title":"Bad","relativeMinutes":10,"dueAt":"2026-10-17T00:15:00+08:00"})
        )
        .is_err());
        assert_eq!(e.reminders.list_todos().unwrap().len(), 1);
    }
}

#[cfg(test)]
mod todo_reminder_tool_tests {
    use super::*;
    fn reminder_fixture() -> (tempfile::TempDir, std::sync::Arc<Engine>, ToolScope) {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::new(d.path().join("library"), d.path().join("state")).unwrap();
        let session = {
            let mut c = e.lock().unwrap();
            let doc = c.create_section("fixture", "reminders", vec![], vec![]).unwrap();
            c.start_session(&doc.id, false).unwrap().0
        };
        let run: crate::generation::Run = serde_json::from_value(json!({
            "id":"run_reminder_tool","sessionId":session.id,"mode":"question","status":"succeeded",
            "content":"","error":null,"createdAt":"2026-10-16T15:55:00Z","updatedAt":"2026-10-16T15:55:00Z",
            "requestDigest":"fixture","contextManifest":[],"referenceTime":"2026-10-16T15:55:00Z","timeZone":"Asia/Shanghai"
        })).unwrap();
        e.runs.lock().unwrap().insert(run.id.clone(), crate::generation::ManagedRun{
            run,
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        let scope = ToolScope {
            session_id: session.id,
            section_id: session.section_id,
            run_id: "run_reminder_tool".into(),
            allowed_ids: vec![],
        };
        (d, e, scope)
    }

    #[test]
    fn todo_reminder_opt_in_schedules_once_and_stays_honest_without_any_delivery() {
        let (_d, e, scope) = reminder_fixture();
        // Legacy calls without the flag keep reminders off.
        let plain = execute(&e, &scope, "legacy-call", "create_todo", json!({"title":"复习"})).unwrap();
        assert_eq!(plain["todo"]["reminderEnabled"], false);
        // Explicit opt-in with a relative time uses the durable run clock.
        let args = json!({"title":"带提醒复习","relativeMinutes":10,"reminderEnabled":true});
        let timed = execute(&e, &scope, "timed-call", "create_todo", args.clone()).unwrap();
        assert_eq!(timed["todo"]["reminderEnabled"], true);
        assert_eq!(timed["dueAt"], "2026-10-16T16:05:00.000000000Z");
        assert_eq!(timed["referenceTime"], "2026-10-16T15:55:00Z");
        // Idempotent replay returns the same receipt without a second todo.
        let replay = execute(&e, &scope, "timed-call", "create_todo", args).unwrap();
        assert_eq!(replay["todoId"], timed["todoId"]);
        // The due time has passed the real clock, so scheduling fires exactly once.
        assert_eq!(e.reminders.tick().unwrap().len(), 1);
        assert_eq!(e.reminders.tick().unwrap().len(), 0);
        let notices = e.reminders.list_notifications().unwrap();
        assert_eq!(notices.len(), 1);
        // Honest state: scheduled locally as pending, never claimed as sent.
        assert_eq!(notices[0].delivery_status, "pending");
        // A failed delivery stays auditable and recoverable; nothing is re-sent here.
        let pending = e.reminders.pending_deliveries("2000-01-01T00:00:00Z").unwrap();
        assert_eq!(pending.len(), 1);
        assert!(e.reminders.reserve_delivery(&pending[0].id).unwrap());
        e.reminders.finish_delivery(&pending[0].id, &crate::feishu::DeliveryOutcome {
            status: crate::feishu::DeliveryState::Failed,
            message_id: None,
            reason: "synthetic failure".into(),
        }).unwrap();
        let after = e.reminders.list_notifications().unwrap();
        assert_eq!(after[0].delivery_status, "failed");
        // Enabling without a time, or with a time already past, is rejected.
        assert_eq!(
            execute(&e, &scope, "no-time", "create_todo", json!({"title":"T","reminderEnabled":true})).unwrap_err().code,
            "INVALID_TOOL_INPUT"
        );
        assert_eq!(
            execute(&e, &scope, "past-time", "create_todo", json!({"title":"T","dueAt":"2026-10-16T15:00:00Z","reminderEnabled":true})).unwrap_err().code,
            "INVALID_TOOL_INPUT"
        );
        // Old behavior is unchanged: a past due time without the flag stays allowed and off.
        let compat = execute(&e, &scope, "compat-call", "create_todo", json!({"title":"旧调用","dueAt":"2000-01-01T00:00:00Z"})).unwrap();
        assert_eq!(compat["todo"]["reminderEnabled"], false);
        assert_eq!(e.reminders.tick().unwrap().len(), 0);
        assert_eq!(e.reminders.list_todos().unwrap().len(), 3);
    }
}
