use crate::content::ContentStore;
use crate::types::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    #[serde(default)]
    pub session_by_course: BTreeMap<String, String>,
    #[serde(default)]
    pub active_session_by_course: BTreeMap<String, String>,
    pub active_document_id: Option<String>,
    pub active_session_id: Option<String>,
    #[serde(default)]
    pub last_run_by_session: BTreeMap<String, String>,
    #[serde(default)]
    pub session_by_section: BTreeMap<String, String>,
    #[serde(default)]
    pub document_paths: BTreeMap<String, String>,
    #[serde(default)]
    pub exploration_session_id: Option<String>,
}

fn metadata(id: &str, title: &str, kind: &str) -> Metadata {
    Metadata {
        id: id.into(),
        title: title.into(),
        kind: kind.into(),
        goals: vec![],
        tags: vec![],
        links: vec![],
        version: 1,
        extra: BTreeMap::new(),
    }
}

pub fn create(
    store: &mut ContentStore,
    section_id: &str,
    manifest: &[ManifestItem],
) -> Result<Session> {
    let section = store.read(section_id)?;
    if section.metadata.kind == "exercise" {
        // Exercise views always resume their source session; never create an unrelated attempt ledger.
        return Ok(crate::exercises::get(store, section_id)?.session);
    }
    create_with_id(
        store,
        section_id,
        manifest,
        &uuid::Uuid::new_v4().simple().to_string(),
    )
}
/// A deterministic session ID is a durable receipt; retries recover its canonical Markdown
/// path even before the library index is refreshed, without changing an existing session.
pub fn create_with_id(
    store: &mut ContentStore,
    section_id: &str,
    manifest: &[ManifestItem],
    id: &str,
) -> Result<Session> {
    create_scoped(store, section_id, manifest, id, None, &[])
}
pub fn create_course(store: &mut ContentStore, section_id: &str, manifest: &[ManifestItem], id: &str, course: &str, legacy: &[String]) -> Result<Session> {
    create_scoped(store, section_id, manifest, id, Some(course), legacy)
}
fn create_scoped(store: &mut ContentStore, section_id: &str, manifest: &[ManifestItem], id: &str, course: Option<&str>, legacy: &[String]) -> Result<Session> {
    if !valid_id(id) || (!section_id.is_empty() && !valid_id(section_id))
        || (section_id.is_empty() && course.is_none()) {
        return Err(CoreError::new("INVALID_INPUT", "会话或来源编号无效。"));
    }
    if store.is_trashed(id) || store.is_trashed(section_id) {
        return Err(CoreError::new(
            "TRASHED",
            "原会话已在回收站，请先恢复；未重复创建。",
        ));
    }
    let path = format!("sessions/{id}/session.md");
    match fs::symlink_metadata(store.root().join(&path)) {
        Ok(_) => {
            let existing = store.register(&path)?;
            if existing.id != id
                || existing.metadata.kind != "session"
                || existing
                    .metadata
                    .extra
                    .get("sectionId")
                    .and_then(Value::as_str)
                    != Some(section_id)
                || existing.metadata.extra.get("courseId").and_then(Value::as_str) != course
            {
                return Err(CoreError::new(
                    "CONFLICT",
                    "固定会话编号已被另一份内容使用，未覆盖原记录。",
                ));
            }
            return get(store, id);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let section = store.read(if section_id.is_empty() { course.unwrap() } else { section_id })?;
    if !(section_id.is_empty() && course.is_some() && section.metadata.kind == "course") && !matches!(
        section.metadata.kind.as_str(),
        "section" | "knowledge" | "exploration"
    ) {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "请选择课程小节、知识点或探索文档开始学习。",
        ));
    }
    match store.read(id) {
        Ok(_) => {
            return Err(CoreError::new(
                "CONFLICT",
                "固定会话编号已由其他文件使用，未覆盖原记录。",
            ))
        }
        Err(error) if error.code == "NOT_FOUND" => {}
        Err(error) => return Err(error),
    }
    let mut meta = metadata(
        id,
        &format!("{} · 学习记录", section.metadata.title),
        "session",
    );
    if let Some(course) = course {
        meta.title = if section_id.is_empty() {
            let count = store.summaries().iter().filter(|d| d.kind == "session")
                .filter(|d| store.read(&d.id).is_ok_and(|d| d.metadata.extra.get("courseId").and_then(Value::as_str) == Some(course))).count();
            format!("新对话 {}", count + 1)
        } else { format!("{} · 课程对话", store.read(course)?.metadata.title) };
        meta.extra.insert("courseId".into(), json!(course));
    }
    meta.extra.insert("legacySessionIds".into(), json!(legacy));
    meta.links = if section_id.is_empty() { vec![] } else { vec![section_id.into()] };
    meta.extra.insert("sectionId".into(), json!(section_id));
    meta.extra
        .insert("associateCurrentSection".into(), json!(!section_id.is_empty()));
    meta.extra.insert("createdAt".into(), json!(timestamp()));
    meta.extra.insert("contextManifest".into(), json!(manifest));
    let body=format!("# {}\n\n本次学习记录。每条提问、练习和作答存于同目录 entries/ 的 Markdown 文件；课程正文不被改写。\n",meta.title);
    store.create(meta, &body, &path)?;
    get(store, id)
}

/// Independent exploration has no source document or inherited links.
pub fn create_exploration(store: &mut ContentStore) -> Result<Session> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut meta = metadata(&id, "独立探索 · 学习记录", "session");
    meta.extra.insert("sectionId".into(), json!(""));
    meta.extra
        .insert("associateCurrentSection".into(), json!(false));
    meta.extra.insert("createdAt".into(), json!(timestamp()));
    meta.extra.insert("contextManifest".into(), json!([]));
    store.create(
        meta,
        "# 独立探索\n\n未关联课程小节。只有本次探索的提问和记录进入上下文。\n",
        &format!("sessions/{id}/session.md"),
    )?;
    get(store, &id)
}

pub fn get(store: &mut ContentStore, id: &str) -> Result<Session> {
    if !valid_id(id) {
        return Err(CoreError::new("INVALID_INPUT", "会话编号无效。"));
    }
    // Direct paths make resume independent of a full-library scan.
    let relative = format!("sessions/{id}/session.md");
    let doc = store.register(&relative)?;
    if doc.id != id || doc.metadata.kind != "session" {
        return Err(CoreError::new(
            "INVALID_DOCUMENT",
            "会话文件内容或编号无效。",
        ));
    }
    let section_id = doc
        .metadata
        .extra
        .get("sectionId")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::new("INVALID_DOCUMENT", "学习记录缺少小节编号。"))?
        .to_string();
    if section_id.is_empty()
        && doc.metadata.extra.get("courseId").and_then(Value::as_str).is_none()
        && (doc
            .metadata
            .extra
            .get("associateCurrentSection")
            .and_then(Value::as_bool)
            != Some(false)
            || !doc.metadata.links.is_empty()
            || doc
                .metadata
                .extra
                .get("contextManifest")
                .and_then(Value::as_array)
                .is_none_or(|items| !items.is_empty()))
    {
        return Err(CoreError::new(
            "INVALID_DOCUMENT",
            "独立探索的来源信息不一致，未加载任何课程材料。",
        ));
    }
    let dir = store.root().join(format!("sessions/{id}/entries"));
    let mut entries = vec![];
    let mut versions = vec![doc.version.clone()];
    if dir.exists() {
        if fs::symlink_metadata(&dir)?.file_type().is_symlink() {
            return Err(CoreError::new(
                "UNSAFE_PATH",
                "学习记录目录不能是符号链接。",
            ));
        }
        let mut paths = vec![];
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let m = fs::symlink_metadata(&path)?;
            if m.is_file()
                && !m.file_type().is_symlink()
                && path.extension().is_some_and(|s| s == "md")
            {
                paths.push(path);
            }
            if paths.len() > 1000 {
                return Err(CoreError::new(
                    "LIMIT_EXCEEDED",
                    "本次学习记录超过1000条，请开启新会话。",
                ));
            }
        }
        paths.sort();
        for path in paths {
            let relative = path
                .strip_prefix(store.root())
                .map_err(CoreError::io)?
                .to_string_lossy()
                .replace('\\', "/");
            let doc = match store.register(&relative) {
                Ok(document) => document,
                Err(error) if error.code == "TRASHED" => continue,
                Err(error) => return Err(error),
            };
            if doc.metadata.kind != "entry"
                || doc.metadata.extra.get("sessionId").and_then(Value::as_str) != Some(id)
            {
                return Err(CoreError::new(
                    "INVALID_DOCUMENT",
                    "学习记录归属不匹配，请核对外部编辑。",
                ));
            }
            versions.push(doc.version);
            entries.push(Entry {
                id: doc.id,
                kind: doc
                    .metadata
                    .extra
                    .get("entryKind")
                    .and_then(Value::as_str)
                    .unwrap_or("note")
                    .to_string(),
                content: doc.body,
                created_at: doc
                    .metadata
                    .extra
                    .get("createdAt")
                    .and_then(Value::as_str)
                    .unwrap_or(&doc.modified_at)
                    .to_string(),
                selection: doc
                    .metadata
                    .extra
                    .get("selection")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                related_ids: doc.metadata.links,
                data: doc
                    .metadata
                    .extra
                    .get("data")
                    .cloned()
                    .filter(|v| !v.is_null()),
            });
        }
    }
    entries.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    let created = doc
        .metadata
        .extra
        .get("createdAt")
        .and_then(Value::as_str)
        .unwrap_or(&doc.modified_at)
        .to_string();
    let updated = entries
        .last()
        .map(|e| e.created_at.clone())
        .unwrap_or_else(|| doc.modified_at.clone());
    Ok(Session {
        course_id: doc.metadata.extra.get("courseId").and_then(Value::as_str).map(str::to_owned),
        legacy_session_ids: serde_json::from_value(doc.metadata.extra.get("legacySessionIds").cloned().unwrap_or(json!([])))?,
        id: id.into(),
        section_id,
        title: doc.metadata.title,
        path: doc.path,
        version: hash(&versions.join("\n")),
        created_at: created,
        updated_at: updated,
        entries,
        context_exercise_reference_manifest: serde_json::from_value(
            doc.metadata
                .extra
                .get("contextExerciseReferenceManifest")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )?,
        context_assistant_reference_manifest: serde_json::from_value(
            doc.metadata
                .extra
                .get("contextAssistantReferenceManifest")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )?,
        context_history_manifest: serde_json::from_value(
            doc.metadata
                .extra
                .get("contextHistoryManifest")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )?,
        context_excluded_history_source_ids: serde_json::from_value(
            doc.metadata
                .extra
                .get("contextExcludedHistorySourceIds")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )?,
        context_warnings: serde_json::from_value(
            doc.metadata
                .extra
                .get("contextWarnings")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )?,
        context_truncated: serde_json::from_value(
            doc.metadata
                .extra
                .get("contextTruncated")
                .cloned()
                .unwrap_or_else(|| json!(false)),
        )?,
        context_manifest: serde_json::from_value(
            doc.metadata
                .extra
                .get("contextManifest")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )?,
    })
}

/// Store the exact manifest used by the next generation, not a stale startup snapshot.
pub fn set_context_manifest(
    store: &mut ContentStore,
    session_id: &str,
    manifest: &[ManifestItem],
) -> Result<Session> {
    let session = get(store, session_id)?;
    let mut context = Context {
        citation_context: None,
        automatic_materials: vec![],
        text: String::new(),
        manifest: manifest.to_vec(),
        exercise_reference_manifest: vec![],
        assistant_reference_manifest: vec![],
        history_manifest: vec![],
        excluded_history_source_ids: vec![],
        warnings: vec![],
        truncated: false,
    };
    crate::context::protect_trashed_history(&mut context, &session, &store.trashed_ids());
    set_context_snapshot(store, session_id, &context)
}

pub fn set_context_snapshot(
    store: &mut ContentStore,
    session_id: &str,
    context: &Context,
) -> Result<Session> {
    set_context_snapshot_explicit(store, session_id, context, false)
}
pub fn set_context_snapshot_explicit(
    store: &mut ContentStore,
    session_id: &str,
    context: &Context,
    explicit: bool,
) -> Result<Session> {
    let session = get(store, session_id)?;
    let excluded: std::collections::BTreeSet<_> =
        context.excluded_history_source_ids.iter().collect();
    if excluded.len() != context.excluded_history_source_ids.len()
        || excluded.len() > 4096
        || excluded.iter().any(|id| !valid_id(id))
    {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "排除的历史来源编号无效或过多。",
        ));
    }
    if session.course_id.is_none() && session.section_id.is_empty() && !context.manifest.is_empty() && !explicit {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "独立探索不能写入课程来源。",
        ));
    }
    if context.history_manifest.iter().any(|item| {
        !item
            .path
            .starts_with(&format!("learning-history/{session_id}/"))
            || !session.entries.iter().any(|entry| entry.id == item.id)
    }) {
        return Err(CoreError::new(
            "OUT_OF_SCOPE",
            "历史上下文不属于本次会话，未写入来源。",
        ));
    }
    if context.assistant_reference_manifest.len() > 4
        || context.assistant_reference_manifest.iter().any(|item| {
            item.path != format!("ai-answer/{session_id}/{}", item.id)
                || !session.entries.iter().any(|entry| {
                    entry.id == item.id
                        && entry.kind == "assistant"
                        && hash(&entry.content) == item.version
                })
                || item.included_chars == 0
                || item.included_chars > 8000
        })
    {
        return Err(CoreError::new(
            "OUT_OF_SCOPE",
            "AI回答来源不属于本次会话或版本已变化，未写入来源。",
        ));
    }
    crate::exercise_references::validate_manifest(
        store,
        &session,
        &context.exercise_reference_manifest,
    )?;
    let mut doc = store.read(session_id)?;
    doc.metadata.extra.insert(
        "contextExerciseReferenceManifest".into(),
        json!(context.exercise_reference_manifest),
    );
    doc.metadata.extra.insert(
        "contextAssistantReferenceManifest".into(),
        json!(context.assistant_reference_manifest),
    );
    doc.metadata
        .extra
        .insert("contextManifest".into(), json!(context.manifest));
    doc.metadata.extra.insert(
        "contextHistoryManifest".into(),
        json!(context.history_manifest),
    );
    doc.metadata.extra.insert(
        "contextExcludedHistorySourceIds".into(),
        json!(context.excluded_history_source_ids),
    );
    doc.metadata
        .extra
        .insert("contextWarnings".into(), json!(context.warnings));
    doc.metadata
        .extra
        .insert("contextTruncated".into(), json!(context.truncated));
    let content = crate::content::serialize(&doc.metadata, &doc.body)?;
    store.save(session_id, &content, &doc.version)?;
    get(store, session_id)
}

fn validate_entry(entry: &Entry) -> Result<()> {
    if ![
        "user",
        "assistant",
        "exercise",
        "attempt",
        "feedback",
        "error",
        "note",
    ]
    .contains(&entry.kind.as_str())
    {
        return Err(CoreError::new("INVALID_INPUT", "不支持的学习记录类型。"));
    }
    if entry.content.trim().is_empty()
        || entry.content.len() > 1_000_000
        || entry.selection.as_ref().is_some_and(|s| s.len() > 30000)
    {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "请输入内容，单条记录最多1MB，选段最多3万字节。",
        ));
    }
    if entry.data.as_ref().is_some_and(|data| {
        !data.is_object() || serde_json::to_vec(data).map_or(true, |bytes| bytes.len() > 512 * 1024)
    }) {
        return Err(CoreError::new("INVALID_INPUT", "学习记录结构无效或过大。"));
    }
    if entry.related_ids.len() > 64 || entry.related_ids.iter().any(|s| !valid_id(s)) {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "关联记录编号无效或数量过多。",
        ));
    }
    crate::context::requested_context(entry, None)?;
    Ok(())
}

pub fn append(
    store: &mut ContentStore,
    session_id: &str,
    entry: Entry,
    expected: Option<&str>,
    request_id: &str,
) -> Result<Session> {
    append_citation(store, session_id, entry, expected, request_id, None)
}
pub(crate) fn append_citation(
    store: &mut ContentStore,
    session_id: &str,
    mut entry: Entry,
    expected: Option<&str>,
    request_id: &str,
    citation: Option<&crate::citation_context::CitationContext>,
) -> Result<Session> {
    if request_id.len() < 8 || request_id.len() > 128 {
        return Err(CoreError::new("INVALID_INPUT", "请求缺少防重复编号。"));
    }
    validate_entry(&entry)?;
    let old = get(store, session_id)?;
    // Timestamp and client-supplied ID do not affect idempotency; the caller may retry after a lost reply.
    let digest = hash(&serde_json::to_string(
        &json!({"kind":entry.kind,"content":entry.content,"selection":entry.selection,"relatedIds":entry.related_ids,"data":entry.data}),
    )?);
    let id = format!("entry_{}", &hash(request_id)[..32]);
    if let Some(previous) = old.entries.iter().find(|e| e.id == id) {
        let previous_doc = store.read(&previous.id)?;
        let previous_digest = previous_doc
            .metadata
            .extra
            .get("requestDigest")
            .and_then(Value::as_str)
            .unwrap_or("");
        if previous_digest != digest {
            return Err(CoreError::new(
                "CONFLICT",
                "防重复编号已被另一份内容使用，请核对后重试。",
            ));
        }
        if previous.kind == "attempt" {
            let previous = previous.clone();
            grade_exact(store, session_id, &previous, request_id)?;
            return get(store, session_id);
        }
        return Ok(old);
    }
    if entry.kind == "user"
        && entry
            .data
            .as_ref()
            .is_some_and(|d| d.get("citationContext").is_some())
    {
        return Err(CoreError::new(
            "INVALID_CONTEXT_REQUEST",
            "补充引用快照只能由来源校验后生成，不能由客户端提供。",
        ));
    }
    if let Some(frozen) = crate::context::requested_context(&entry, old.course_id.is_none().then_some(old.section_id.as_str()))? {
        let source = crate::course_sessions::source(store, &old, &frozen.document_id)?;
        crate::course_sessions::validate_injections(store, &old, &frozen.injections)?;
        if entry.data.as_ref().and_then(|d| d.get("sourceDocumentId")).and_then(Value::as_str).is_some_and(|id| id != source) {
            return Err(CoreError::new("CONTEXT_CONFLICT", "问题的冻结来源与当前材料不一致，未改变原请求。"));
        }
        entry.data.as_mut().unwrap()["sourceDocumentId"] = json!(source);

        crate::assistant_references::validate_references(
            store,
            &old,
            &frozen.assistant_references,
        )?;
        crate::exercise_references::validate_references(store, &old, &frozen.exercise_references)?;
        let citation = if let Some(citation) = citation {
            // Internal callers supply the already verified same-request Context. Focus
            // may refer to fixed Git bytes after the editable workspace has changed.
            citation.clone()
        } else {
            crate::citation_context::freeze(
                store,
                &old,
                &frozen.injections,
                &frozen.assistant_references,
                &frozen.exercise_references,
            )?
        };
        if !citation.references.is_empty(){entry.data.as_mut().unwrap()["citationContext"] = serde_json::to_value(citation)?;}
        validate_entry(&entry)?;
    }
    if entry.kind == "exercise" {
        let questions = entry
            .data
            .as_ref()
            .and_then(|data| data.get("exercises"))
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty() && items.len() <= 5)
            .ok_or_else(|| CoreError::new("INVALID_INPUT", "练习需要有效题目列表。"))?;
        let mut ids = std::collections::BTreeSet::new();
        for question in questions {
            let id = question
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| valid_id(id))
                .ok_or_else(|| CoreError::new("INVALID_INPUT", "练习缺少稳定题目编号。"))?;
            if !ids.insert(id) || question_exists(&old, id) {
                return Err(CoreError::new(
                    "CONFLICT",
                    "题目编号已存在，未覆盖原题和作答。",
                ));
            }
        }
    }
    if entry.kind == "attempt" {
        let exercise_id = entry
            .data
            .as_ref()
            .and_then(|d| d.get("exerciseId"))
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::new("INVALID_INPUT", "请先选择对应练习再提交作答。"))?;
        if !valid_id(exercise_id)
            || !old
                .entries
                .iter()
                .filter(|entry| entry.kind == "exercise")
                .filter_map(|e| e.data.as_ref())
                .filter_map(|d| d.get("exercises"))
                .filter_map(Value::as_array)
                .flatten()
                .any(|q| q.get("id").and_then(Value::as_str) == Some(exercise_id))
        {
            return Err(CoreError::new(
                "NOT_FOUND",
                "找不到对应题目，作答输入仍保留，请重新选择。",
            ));
        }
    }
    if entry.kind == "attempt" {
        let data = entry.data.as_ref().unwrap();
        let exercise_id = data["exerciseId"].as_str().unwrap();
        if let Some(entry_id) = data.get("exerciseEntryId") {
            if !entry_id.as_str().is_some_and(|id| {
                old.entries.iter().any(|entry| {
                    entry.id == id
                        && entry.kind == "exercise"
                        && entry
                            .data
                            .as_ref()
                            .and_then(|data| data.get("exercises"))
                            .and_then(Value::as_array)
                            .is_some_and(|questions| {
                                questions.iter().any(|question| {
                                    question.get("id").and_then(Value::as_str) == Some(exercise_id)
                                })
                            })
                })
            }) {
                return Err(CoreError::new(
                    "CONFLICT",
                    "题目与练习记录不匹配，未保存作答。",
                ));
            }
        }
        if data.get("answer").is_some_and(|answer| {
            answer
                .as_str()
                .is_none_or(|text| text.trim().is_empty() || text.len() > 30000)
        }) {
            return Err(CoreError::new("INVALID_INPUT", "作答内容无效或过长。"));
        }
    }
    if entry.kind == "feedback" {
        validate_feedback(&old, &entry)?;
    }
    if entry.kind == "error" {
        if let Some(exercise_id) = entry
            .data
            .as_ref()
            .and_then(|data| data.get("exerciseId"))
            .and_then(Value::as_str)
        {
            if !valid_id(exercise_id) || !question_exists(&old, exercise_id) {
                return Err(CoreError::new(
                    "NOT_FOUND",
                    "错题引用的题目不属于此会话，未保存。",
                ));
            }
        }
    }
    if expected.is_some_and(|v| v != old.version) {
        return Err(CoreError::new(
            "CONFLICT",
            "学习记录已在另一处更新。输入仍保留，请重新载入后再保存。",
        ));
    }
    if entry.kind == "error"
        && entry.data.as_ref().is_some_and(|data| {
            data.get("origin").and_then(Value::as_str) == Some("pi-tool")
                || data.get("needsConfirmation").and_then(Value::as_bool) == Some(true)
        })
    {
        if let Some(data) = entry.data.as_mut() {
            if let Some(cause) = data
                .get("cause")
                .and_then(Value::as_str)
                .filter(|cause| !cause.is_empty())
                .map(str::to_string)
            {
                data["suggestedCause"] = json!(cause);
                data["cause"] = json!("");
            }
            data["causeConfirmed"] = json!(false);
            let exercise_id = data.get("exerciseId").and_then(Value::as_str);
            if let Some(previous) = old.entries.iter().rev().find(|previous| {
                previous.kind == "error"
                    && exercise_id.is_some()
                    && previous
                        .data
                        .as_ref()
                        .and_then(|data| data.get("exerciseId"))
                        .and_then(Value::as_str)
                        == exercise_id
            }) {
                entry.kind = "note".into();
                data["sourceErrorId"] = json!(previous.id);
                data["errorSuggestion"] = json!(true);
            }
        }
    }
    // A personal correction refines a historical error without discarding its reference answer.
    if entry.kind == "error" {
        if let Some(data) = entry.data.as_mut() {
            let human_cause = data.get("origin").and_then(Value::as_str) != Some("pi-tool")
                && data.get("needsConfirmation").and_then(Value::as_bool) != Some(true);
            if let (Some(exercise_id), Some(cause)) = (
                data.get("exerciseId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                data.get("cause")
                    .and_then(Value::as_str)
                    .filter(|s| human_cause && !s.trim().is_empty())
                    .map(str::to_string),
            ) {
                data["causeConfirmed"] = json!(true);
                if let Some(previous) = old.entries.iter().rev().find(|e| {
                    e.kind == "error"
                        && e.data
                            .as_ref()
                            .and_then(|d| d.get("exerciseId"))
                            .and_then(Value::as_str)
                            == Some(&exercise_id)
                }) {
                    if let Some(prior) = &previous.data {
                        let reference = prior
                            .get("referenceBack")
                            .or_else(|| prior.get("back"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        data["referenceBack"] = json!(reference);
                        data["back"] = json!(format!("{reference}\n\n我的错因：{cause}"));
                        if prior.get("front").is_some() {
                            data["front"] = prior["front"].clone();
                        }
                        if data
                            .get("tags")
                            .and_then(Value::as_array)
                            .is_none_or(|a| a.is_empty())
                        {
                            data["tags"] = prior.get("tags").cloned().unwrap_or_else(|| json!([]));
                        }
                        data["sourceErrorId"] = json!(previous.id);
                        data["causeConfirmed"] = json!(true);
                        if let Some(suggestion) = prior.get("suggestedCause") {
                            data["suggestedCause"] = suggestion.clone();
                        }
                        if let Some(origin) = prior.get("origin") {
                            data["sourceOrigin"] = origin.clone();
                        }
                    }
                }
            }
        }
    }
    if entry.kind == "error" || entry.data.as_ref().is_some_and(|d| d["reviewCard"] == true) {
        let data = entry.data.as_ref().cloned().unwrap_or_else(|| json!({}));
        let mut source = record_source(&old, &data);
        if !source.is_empty() {
            if data.get("exerciseId").is_none() || data["origin"] == "pi-tool" {
                source = crate::course_sessions::source(store, &old, &source)?;
            }
            entry.data.get_or_insert_with(|| json!({}))["sourceDocumentId"] = json!(source);
        }
    }
    entry.id = id;
    // Millisecond ties must not let a lexicographically larger old attempt become "latest".
    let newest = old
        .entries
        .iter()
        .filter_map(|entry| chrono::DateTime::parse_from_rfc3339(&entry.created_at).ok())
        .map(|stamp| stamp.timestamp_millis())
        .max()
        .unwrap_or(i64::MIN);
    let millis = chrono::Utc::now()
        .timestamp_millis()
        .max(newest.saturating_add(1));
    entry.created_at = chrono::DateTime::from_timestamp_millis(millis)
        .ok_or_else(|| CoreError::new("INVALID_STATE", "学习记录时间无效。"))?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut meta = metadata(&entry.id, &entry.kind, "entry");
    meta.links = entry.related_ids.clone();
    meta.extra.insert("sessionId".into(), json!(session_id));
    meta.extra.insert("entryKind".into(), json!(entry.kind));
    meta.extra
        .insert("createdAt".into(), json!(entry.created_at));
    meta.extra.insert("requestDigest".into(), json!(digest));
    if let Some(selection) = &entry.selection {
        meta.extra.insert("selection".into(), json!(selection));
    }
    if let Some(data) = &entry.data {
        meta.extra.insert("data".into(), data.clone());
    }
    let relative = format!("sessions/{session_id}/entries/{}.md", entry.id);
    store.create(meta, &entry.content, &relative)?;
    // The committed entry is the transaction unit. A crash after this point resumes from Markdown.
    if entry.kind == "attempt" {
        grade_exact(store, session_id, &entry, request_id)?;
    }
    get(store, session_id)
}

fn question_exists(session: &Session, exercise_id: &str) -> bool {
    session
        .entries
        .iter()
        .filter(|entry| entry.kind == "exercise")
        .filter_map(|entry| entry.data.as_ref())
        .filter_map(|data| data.get("exercises"))
        .filter_map(Value::as_array)
        .flatten()
        .any(|question| question.get("id").and_then(Value::as_str) == Some(exercise_id))
}
fn validate_feedback(session: &Session, entry: &Entry) -> Result<()> {
    let data = entry
        .data
        .as_ref()
        .ok_or_else(|| CoreError::new("INVALID_INPUT", "反馈必须关联题目和本次作答。"))?;
    let exercise_id = data
        .get("exerciseId")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .ok_or_else(|| CoreError::new("INVALID_INPUT", "反馈缺少有效题目编号。"))?;
    let attempt_id = data
        .get("attemptId")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .ok_or_else(|| CoreError::new("INVALID_INPUT", "反馈缺少有效作答编号。"))?;
    if !question_exists(session, exercise_id) {
        return Err(CoreError::new("NOT_FOUND", "反馈题目不属于当前会话。"));
    }
    let latest = session.entries.iter().rev().find(|candidate| {
        candidate.kind == "attempt"
            && candidate
                .data
                .as_ref()
                .and_then(|data| data.get("exerciseId"))
                .and_then(Value::as_str)
                == Some(exercise_id)
    });
    if latest.is_none_or(|attempt| attempt.id != attempt_id) {
        return Err(CoreError::new(
            "CONFLICT",
            "反馈不对应此题最新作答，未把历史反馈套用到新答案。",
        ));
    }
    Ok(())
}

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .trim_matches(|c: char| ".。!！".contains(c))
        .to_string()
}

fn answer_matches(question: &Value, reference: &str, answer: &str) -> bool {
    if question["answerMatch"].as_str() == Some("trim") {
        reference.trim() == answer.trim()
    } else {
        normalize(reference) == normalize(answer)
    }
}
fn grade_exact(
    store: &mut ContentStore,
    session_id: &str,
    attempt: &Entry,
    request_id: &str,
) -> Result<()> {
    let data = attempt
        .data
        .as_ref()
        .ok_or_else(|| CoreError::new("INVALID_INPUT", "作答缺少练习关联，请重新选择练习。"))?;
    let exercise_id = data
        .get("exerciseId")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::new("INVALID_INPUT", "缺少题目编号。"))?;
    let session = get(store, session_id)?;
    let exercise = session
        .entries
        .iter()
        .filter(|entry| entry.kind == "exercise")
        .filter_map(|e| e.data.as_ref())
        .filter_map(|d| d.get("exercises"))
        .filter_map(Value::as_array)
        .flatten()
        .find(|q| q.get("id").and_then(Value::as_str) == Some(exercise_id))
        .cloned()
        .ok_or_else(|| {
            CoreError::new(
                "NOT_FOUND",
                "找不到原练习，作答已保存，可添加笔记或重新生成反馈。",
            )
        })?;
    let answer = data
        .get("answer")
        .and_then(Value::as_str)
        .unwrap_or(&attempt.content);
    let reference: Vec<String> = exercise
        .get("answers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let exact = exercise.get("format").and_then(Value::as_str) == Some("exact");
    let correct = exact.then(|| {
        reference
            .iter()
            .any(|a| answer_matches(&exercise, a, answer))
    });
    let explanation = exercise
        .get("explanation")
        .and_then(Value::as_str)
        .unwrap_or("");
    let content = match correct { Some(true) => format!("答案正确。\n\n{explanation}"), Some(false) => format!("本次答案未匹配参考答案。\n\n参考：{}\n\n{explanation}\n\n错因待本人确认，可补充或修正。", reference.join(" / ")), None => "作答已保存，等待AI批改；尚未判定对错。".into() };
    let feedback = Entry {
        id: String::new(),
        kind: "feedback".into(),
        content,
        created_at: timestamp(),
        selection: None,
        related_ids: vec![attempt.id.clone()],
        data: Some(
            json!({"exerciseId":exercise_id,"attemptId":attempt.id,"correct":correct,"method":if exact{"exact"}else{"pending"}}),
        ),
    };
    append(
        store,
        session_id,
        feedback,
        None,
        &format!("feedback-{}", &hash(request_id)[..32]),
    )?;
    if correct == Some(false) {
        let error_request = format!("error-{}", &hash(request_id)[..32]);
        let error_id = format!("entry_{}", &hash(&error_request)[..32]);
        if session.entries.iter().any(|entry| entry.id == error_id) {
            return Ok(());
        }
        // A later wrong answer cannot displace an existing reference/confirmed cause in review or CSV.
        let previous_error = session.entries.iter().rev().find(|entry| {
            entry.kind == "error"
                && entry
                    .data
                    .as_ref()
                    .and_then(|data| data.get("exerciseId"))
                    .and_then(Value::as_str)
                    == Some(exercise_id)
        });
        let mut error = Entry {
            id: String::new(),
            kind: "error".into(),
            content: format!(
                "{}\n\n我的答案：{answer}\n\n参考：{}\n\n错因：待本人确认。",
                exercise
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or("练习"),
                reference.join(" / ")
            ),
            created_at: timestamp(),
            selection: None,
            related_ids: vec![attempt.id.clone()],
            data: Some(
                json!({"exerciseId":exercise_id,"attemptId":attempt.id,"cause":"","tags":exercise.get("tags").cloned().unwrap_or_else(||json!([])),"front":exercise.get("question"),"back":format!("{}\n{}",reference.join(" / "),explanation)}),
            ),
        };
        if let Some(previous) = previous_error {
            error.kind = "note".into();
            error.related_ids.push(previous.id.clone());
            if let Some(data) = error.data.as_mut() {
                data["sourceErrorId"] = json!(previous.id);
                data["errorSuggestion"] = json!(true);
                data["origin"] = json!("exact-grader");
                data["needsConfirmation"] = json!(true);
            }
        }
        append(store, session_id, error, None, &error_request)?;
    }
    Ok(())
}

pub fn export_csv(
    store: &mut ContentStore,
    session_id: &str,
    destination: &Path,
) -> Result<(String, usize)> {
    get(store, session_id)?;
    let cards = review_items_all(store)?
        .into_iter()
        .filter(|v| v["sessionId"].as_str() == Some(session_id))
        .collect::<Vec<_>>();
    let mut lines = String::from("#separator:comma\n#html:false\n#columns:Front,Back,Tags\n");
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let count = cards.len();
    for card in cards {
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
        lines.push_str(&format!(
            "{},{},{}\n",
            quote(card["front"].as_str().unwrap_or("")),
            quote(card["back"].as_str().unwrap_or("")),
            quote(&tags)
        ));
    }
    fs::create_dir_all(destination)?;
    let path = destination.join(format!(
        "anki-{session_id}-{}.csv",
        chrono::Utc::now().format("%Y%m%d-%H%M%S%3f")
    ));
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(lines.as_bytes())?;
    file.sync_all()?;
    Ok((path.to_string_lossy().into_owned(), count))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_resume_dedup_and_external_edit_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let mut store =
            ContentStore::new(temp.path().join("library"), temp.path().join("state")).unwrap();
        store
            .create(
                metadata("section_a", "英语", "section"),
                "For a period; since a starting point.",
                "courses/a.md",
            )
            .unwrap();
        let session = create(&mut store, "section_a", &[]).unwrap();
        let entry = Entry {
            id: String::new(),
            kind: "note".into(),
            content: "since 与 for 的区别？".into(),
            created_at: String::new(),
            selection: None,
            related_ids: vec![],
            data: None,
        };
        let saved = append(
            &mut store,
            &session.id,
            entry.clone(),
            Some(&session.version),
            "request-12345",
        )
        .unwrap();
        assert_eq!(saved.entries.len(), 1);
        assert_eq!(
            append(
                &mut store,
                &session.id,
                entry.clone(),
                Some(&session.version),
                "request-12345"
            )
            .unwrap()
            .entries
            .len(),
            1
        );
        assert_eq!(
            append(
                &mut store,
                &session.id,
                entry,
                Some(&session.version),
                "request-67890"
            )
            .unwrap_err()
            .code,
            "CONFLICT"
        );
        let mut reopened =
            ContentStore::new(temp.path().join("library"), temp.path().join("state")).unwrap();
        let resumed = get(&mut reopened, &session.id).unwrap();
        assert_eq!(resumed.entries[0].content, "since 与 for 的区别？");
    }
}

pub fn review_items(store: &mut ContentStore, query: &str) -> Result<Vec<Value>> {
    if query.len() > 1000 {
        return Err(CoreError::new("INVALID_INPUT", "搜索主题过长，请缩短。"));
    }
    let tokens = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut items = review_items_all(store)?
        .into_iter()
        .filter(|item| {
            let search = item.to_string().to_lowercase();
            tokens.iter().all(|token| search.contains(token))
        })
        .collect::<Vec<_>>();
    items.truncate(200);
    Ok(items)
}
// Exercise ownership comes from the original question entry, never the session anchor.
fn record_source(session: &Session, data: &Value) -> String {
    if let Some(question) = data.get("exerciseId").and_then(Value::as_str) {
        if let Some(exercise) = session.entries.iter().filter(|e| e.kind == "exercise").find(|e| e.data.as_ref().and_then(|d| d.get("exercises")).and_then(Value::as_array).is_some_and(|qs| qs.iter().any(|q| q["id"].as_str() == Some(question)))) {
            return exercise.data.as_ref().and_then(|d| d.get("sourceDocumentId")).and_then(Value::as_str).unwrap_or(if session.course_id.is_none() { &session.section_id } else { "" }).to_owned();
        }
    }
    data.get("sourceDocumentId").and_then(Value::as_str).filter(|id| valid_id(id))
        .unwrap_or(if session.course_id.is_none() { &session.section_id } else { "" }).to_owned()
}
/// Complete bounded-library candidates for exact-ID operations, independent from UI search limits.
pub fn review_items_all(store: &mut ContentStore) -> Result<Vec<Value>> {
    let summaries = store.summaries();
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    let mut session_cache = BTreeMap::new();
    for summary in summaries.into_iter().filter(|s| s.kind == "entry") {
        let doc = store.read(&summary.id)?;
        let kind = doc
            .metadata
            .extra
            .get("entryKind")
            .and_then(Value::as_str)
            .unwrap_or("");
        let data = doc
            .metadata
            .extra
            .get("data")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if kind != "error"
            && !(kind == "note" && data.get("reviewCard").and_then(Value::as_bool) == Some(true))
        {
            continue;
        }
        let front = data
            .get("front")
            .and_then(Value::as_str)
            .unwrap_or(&doc.body)
            .to_string();
        let back = data
            .get("back")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let session_id = doc
            .metadata
            .extra
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let section_id = if valid_id(&session_id) {
            if !session_cache.contains_key(&session_id) { if let Ok(session) = get(store, &session_id) { session_cache.insert(session_id.clone(), session); } }
            session_cache.get(&session_id).map(|session| record_source(session, &data)).unwrap_or_default()
        } else { String::new() };
        let key = if kind == "error" {
            format!(
                "{}:{}",
                session_id,
                data.get("exerciseId")
                    .and_then(Value::as_str)
                    .unwrap_or(&doc.id)
            )
        } else {
            doc.id.clone()
        };
        let stamp = doc
            .metadata
            .extra
            .get("createdAt")
            .cloned()
            .unwrap_or_else(|| json!(doc.modified_at));
        let item = json!({"id":doc.id,"front":front,"back":back,"sectionId":section_id,"source":if kind=="error"{"错题"}else if data.get("origin").and_then(Value::as_str)==Some("pi-tool"){"AI复习（待确认）"}else{"手动复习"},"sessionId":session_id,"tags":data.get("tags").cloned().unwrap_or_else(||json!([])),"createdAt":stamp});
        if grouped.get(&key).is_none_or(|old| {
            old["createdAt"].as_str().unwrap_or("") <= item["createdAt"].as_str().unwrap_or("")
        }) {
            grouped.insert(key, item);
        }
    }
    let mut items = grouped.into_values().collect::<Vec<_>>();
    items.sort_by(|a, b| b["createdAt"].as_str().cmp(&a["createdAt"].as_str()));
    Ok(items)
}

#[cfg(test)]
mod loop_tests {
    use super::*;
    fn entry(kind: &str, content: &str, data: Option<Value>) -> Entry {
        Entry {
            id: String::new(),
            kind: kind.into(),
            content: content.into(),
            created_at: String::new(),
            selection: None,
            related_ids: vec![],
            data,
        }
    }
    #[test]
    fn fixture_exercise_answer_correction_recall_and_csv_are_durable_without_changing_course() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let state = temp.path().join("state");
        let mut store = ContentStore::new(library.clone(), state.clone()).unwrap();
        let original = store
            .create(
                metadata("section_loop", "英语测试材料", "section"),
                "She has studied English for three years.",
                "courses/loop.md",
            )
            .unwrap();
        let session = create(&mut store, "section_loop", &[]).unwrap();
        let exercise = entry(
            "exercise",
            "测试fixture，不是AI生成",
            Some(
                json!({"exercises":[{"id":"q_fixture","question":"for还是since：three years?","format":"exact","answers":["for"],"explanation":"three years是一段时长。","tags":["for-since"]}]}),
            ),
        );
        let saved = append(
            &mut store,
            &session.id,
            exercise,
            None,
            "fixture-exercise-001",
        )
        .unwrap();
        let attempt = entry(
            "attempt",
            "since",
            Some(json!({"exerciseId":"q_fixture","answer":"since"})),
        );
        let answered = append(
            &mut store,
            &session.id,
            attempt.clone(),
            Some(&saved.version),
            "fixture-attempt-001",
        )
        .unwrap();
        assert_eq!(answered.entries.len(), 4);
        assert_eq!(
            answered
                .entries
                .iter()
                .filter(|e| e.kind == "error")
                .count(),
            1
        );
        let retry = append(
            &mut store,
            &session.id,
            attempt,
            Some(&saved.version),
            "fixture-attempt-001",
        )
        .unwrap();
        assert_eq!(retry.entries.len(), 4);
        let corrected = entry(
            "error",
            "把一段时长误当成起点",
            Some(
                json!({"exerciseId":"q_fixture","cause":"把一段时长误当成起点","front":"题目","back":"错因","tags":[]}),
            ),
        );
        append(
            &mut store,
            &session.id,
            corrected.clone(),
            None,
            "fixture-correction-001",
        )
        .unwrap();
        append(
            &mut store,
            &session.id,
            corrected,
            None,
            "fixture-correction-001",
        )
        .unwrap();
        let cards = review_items(&mut store, "for-since").unwrap();
        assert_eq!(cards.len(), 1);
        assert!(cards[0]["back"].as_str().unwrap().contains("for"));
        assert!(cards[0]["back"].as_str().unwrap().contains("误当成起点"));
        assert_eq!(
            store.read("section_loop").unwrap().content,
            original.content
        );
        drop(store);
        let mut reopened = ContentStore::new(library, state).unwrap();
        reopened.refresh().unwrap();
        let (path, count) =
            export_csv(&mut reopened, &session.id, &temp.path().join("exports")).unwrap();
        assert_eq!(count, 1);
        assert!(fs::read_to_string(path).unwrap().contains("误当成起点"));
    }
}

#[cfg(test)]
mod integrity_tests {
    use super::*;
    fn entry(kind: &str, content: &str, data: Value) -> Entry {
        Entry {
            id: String::new(),
            kind: kind.into(),
            content: content.into(),
            created_at: String::new(),
            selection: None,
            related_ids: vec![],
            data: Some(data),
        }
    }
    fn fixture() -> (tempfile::TempDir, ContentStore, Session) {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ContentStore::new(
            directory.path().join("library"),
            directory.path().join("state"),
        )
        .unwrap();
        store
            .create(
                metadata("section_integrity", "Time", "section"),
                "For is a duration.",
                "course/integrity.md",
            )
            .unwrap();
        let session = create(&mut store, "section_integrity", &[]).unwrap();
        let session = append(&mut store, &session.id, entry("exercise", "Fixture questions", json!({"exercises":[{"id":"q_integrity","question":"Three years?","format":"exact","answers":["for"],"explanation":"A duration.","tags":["time"]},{"id":"q_other","question":"Monday?","format":"self","answers":["since"],"explanation":"A time point.","tags":["time"]}]})), None, "integrity-questions").unwrap();
        (directory, store, session)
    }
    #[test]
    fn feedback_is_bound_to_latest_attempt_of_the_same_question_and_retries_preserve_history() {
        let (_directory, mut store, session) = fixture();
        let first = append(
            &mut store,
            &session.id,
            entry(
                "attempt",
                "since",
                json!({"exerciseId":"q_integrity","answer":"since"}),
            ),
            None,
            "integrity-first-answer",
        )
        .unwrap();
        let first_attempt = first
            .entries
            .iter()
            .find(|entry| entry.kind == "attempt")
            .unwrap()
            .id
            .clone();
        let second = append(
            &mut store,
            &session.id,
            entry(
                "attempt",
                "for",
                json!({"exerciseId":"q_integrity","answer":"for"}),
            ),
            None,
            "integrity-second-answer",
        )
        .unwrap();
        let latest = second
            .entries
            .iter()
            .rev()
            .find(|entry| entry.kind == "attempt")
            .unwrap();
        assert_ne!(latest.id, first_attempt);
        let latest_feedback = second
            .entries
            .iter()
            .filter(|entry| entry.kind == "feedback")
            .find(|entry| entry.data.as_ref().unwrap()["attemptId"] == latest.id)
            .unwrap();
        assert_eq!(
            latest_feedback.data.as_ref().unwrap()["exerciseId"],
            "q_integrity"
        );
        assert_eq!(latest_feedback.data.as_ref().unwrap()["correct"], true);
        for (question, attempt) in [
            ("q_integrity", first_attempt.as_str()),
            ("q_other", latest.id.as_str()),
            ("q_integrity", "missing_attempt"),
        ] {
            let error = append(
                &mut store,
                &session.id,
                entry(
                    "feedback",
                    "Must be rejected",
                    json!({"exerciseId":question,"attemptId":attempt,"correct":true}),
                ),
                None,
                &format!("invalid-feedback-{question}-{attempt}"),
            )
            .unwrap_err();
            assert_eq!(error.code, "CONFLICT");
        }
        let retry = append(
            &mut store,
            &session.id,
            entry(
                "attempt",
                "since",
                json!({"exerciseId":"q_integrity","answer":"since"}),
            ),
            None,
            "integrity-first-answer",
        )
        .unwrap();
        assert_eq!(retry.entries.len(), second.entries.len());
        assert_eq!(
            retry
                .entries
                .iter()
                .filter(|entry| entry.kind == "attempt")
                .count(),
            2
        );
    }
    #[test]
    fn subsequent_wrong_answers_and_ai_suggestions_never_replace_personal_cause_or_reference_card()
    {
        let (_directory, mut store, session) = fixture();
        append(
            &mut store,
            &session.id,
            entry(
                "attempt",
                "since",
                json!({"exerciseId":"q_integrity","answer":"since"}),
            ),
            None,
            "cause-first-answer",
        )
        .unwrap();
        append(&mut store, &session.id, entry("error", "My confirmed explanation", json!({"exerciseId":"q_integrity","cause":"I confused a duration with a point.","front":"Changed front must not replace reference","back":"Discard this supplied back","tags":[]})), None, "cause-human-confirmation").unwrap();
        let before = review_items(&mut store, "time").unwrap();
        assert_eq!(before.len(), 1);
        assert!(before[0]["back"].as_str().unwrap().contains("for"));
        assert!(before[0]["back"].as_str().unwrap().contains("I confused"));
        let answered = append(
            &mut store,
            &session.id,
            entry(
                "attempt",
                "on",
                json!({"exerciseId":"q_integrity","answer":"on"}),
            ),
            None,
            "cause-second-answer",
        )
        .unwrap();
        assert!(answered.entries.iter().any(|entry| entry.kind == "note"
            && entry
                .data
                .as_ref()
                .is_some_and(|data| data["origin"] == "exact-grader")));
        append(&mut store, &session.id, entry("error", "Pending AI alternative", json!({"exerciseId":"q_integrity","cause":"AI alternative","front":"AI front","back":"AI alternative back","origin":"pi-tool","needsConfirmation":true,"tags":["time"]})), None, "cause-ai-pending-suggestion").unwrap();
        let after = review_items(&mut store, "time").unwrap();
        assert_eq!(after[0]["id"], before[0]["id"]);
        assert_eq!(after[0]["back"], before[0]["back"]);
        let saved = get(&mut store, &session.id).unwrap();
        let pending = saved
            .entries
            .iter()
            .find(|entry| entry.content == "Pending AI alternative")
            .unwrap();
        assert_eq!(pending.kind, "note");
        assert_eq!(pending.data.as_ref().unwrap()["cause"], "");
        assert_eq!(
            pending.data.as_ref().unwrap()["suggestedCause"],
            "AI alternative"
        );
    }
    #[test]
    fn spoofed_note_questions_duplicate_ids_foreign_entry_and_error_targets_are_rejected() {
        let (_directory, mut store, session) = fixture();
        append(
            &mut store,
            &session.id,
            entry(
                "note",
                "Not a question record",
                json!({"exercises":[{"id":"q_spoofed","answers":["yes"]}]}),
            ),
            None,
            "spoofed-note-record",
        )
        .unwrap();
        for (kind, data) in [
            ("attempt", json!({"exerciseId":"q_spoofed","answer":"yes"})),
            ("attempt", json!({"exerciseId":"../escape","answer":"yes"})),
            (
                "attempt",
                json!({"exerciseId":"q_integrity","exerciseEntryId":"other_entry","answer":"for"}),
            ),
            (
                "error",
                json!({"exerciseId":"foreign_question","cause":"Must not save"}),
            ),
            (
                "exercise",
                json!({"exercises":[{"id":"q_integrity","question":"Duplicate"}]}),
            ),
        ] {
            assert!(append(
                &mut store,
                &session.id,
                entry(kind, "Invalid target", data),
                None,
                &format!("invalid-{}-{}", kind, uuid::Uuid::new_v4().simple())
            )
            .is_err());
        }
        assert!(!get(&mut store, &session.id)
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.kind == "attempt"));
    }
}

#[cfg(test)]
mod review_limit_tests {
    use super::*;
    #[test]
    fn exact_id_candidates_include_old_cards_beyond_the_display_limit() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ContentStore::new(
            directory.path().join("library"),
            directory.path().join("state"),
        )
        .unwrap();
        store
            .create(
                metadata("section_review_limit", "Time", "section"),
                "Reference.",
                "course/review-limit.md",
            )
            .unwrap();
        let session = create(&mut store, "section_review_limit", &[]).unwrap();
        for index in 0..201 {
            let id = format!("review_card_{index:03}");
            let mut meta = metadata(&id, "Card", "entry");
            meta.extra.insert("sessionId".into(), json!(session.id));
            meta.extra.insert("entryKind".into(), json!("note"));
            meta.extra.insert(
                "createdAt".into(),
                json!(
                    (chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap()
                        + chrono::Duration::seconds(index))
                    .to_rfc3339()
                ),
            );
            meta.extra.insert("data".into(),json!({"reviewCard":true,"front":if index==0{"needle_oldest"}else{"Question"},"back":"Reference","tags":[]}));
            store
                .create(
                    meta,
                    "Review fixture",
                    &format!("sessions/{}/entries/{id}.md", session.id),
                )
                .unwrap();
        }
        let visible = review_items(&mut store, "").unwrap();
        assert_eq!(visible.len(), 200);
        assert!(!visible.iter().any(|item| item["id"] == "review_card_000"));
        assert_eq!(
            review_items(&mut store, "needle_oldest").unwrap()[0]["id"],
            "review_card_000"
        );
        let all = review_items_all(&mut store).unwrap();
        assert_eq!(all.len(), 201);
        assert!(all.iter().any(|item| item["id"] == "review_card_000"));
    }
}

#[cfg(test)]
mod fixed_session_tests {
    use super::*;
    #[test]
    fn fixed_session_reopens_without_index_or_preferences_and_conflicts_never_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("library");
        let state = directory.path().join("state");
        let mut store = ContentStore::new(library.clone(), state.clone()).unwrap();
        store
            .create(
                metadata("fixed_source", "Source", "section"),
                "Original source.",
                "course/source.md",
            )
            .unwrap();
        store
            .create(
                metadata("other_source", "Other", "section"),
                "Other source.",
                "course/other.md",
            )
            .unwrap();
        let first = create_with_id(&mut store, "fixed_source", &[], "fixed_session_id").unwrap();
        let bytes = fs::read(store.root().join(&first.path)).unwrap();
        assert_eq!(
            create_with_id(&mut store, "fixed_source", &[], "fixed_session_id")
                .unwrap()
                .version,
            first.version
        );
        assert_eq!(
            create_with_id(&mut store, "other_source", &[], "fixed_session_id")
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert_eq!(fs::read(store.root().join(&first.path)).unwrap(), bytes);
        drop(store);
        let mut reopened = ContentStore::new(library, state).unwrap();
        assert!(reopened.summaries().is_empty());
        let resumed =
            create_with_id(&mut reopened, "fixed_source", &[], "fixed_session_id").unwrap();
        assert_eq!(resumed.id, first.id);
        assert_eq!(resumed.section_id, "fixed_source");
        assert_eq!(resumed.version, first.version);
        assert_eq!(fs::read(reopened.root().join(&first.path)).unwrap(), bytes);
        reopened
            .create(
                metadata("occupied_document", "Unrelated", "section"),
                "Never overwrite.",
                "sessions/colliding_session/session.md",
            )
            .unwrap();
        assert_eq!(
            create_with_id(&mut reopened, "fixed_source", &[], "colliding_session")
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert_eq!(
            reopened.read("occupied_document").unwrap().body,
            "Never overwrite."
        );
    }
}

#[cfg(test)]
mod general_exact_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn new_cross_subject_exact_preserves_symbol_case_and_decimal_points() {
        let q = json!({"answerMatch":"trim"});
        assert!(!answer_matches(&q, "m", "M"));
        assert!(!answer_matches(&q, ".5", "5"));
        assert!(answer_matches(&q, ".5", " .5 "));
        assert!(answer_matches(&json!({}), "for", "FOR!"));
    }
}
