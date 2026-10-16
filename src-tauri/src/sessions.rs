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
    if section.metadata.kind != "section" && section.metadata.kind != "knowledge" {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "请选择课程小节或知识点开始学习。",
        ));
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let stamp = timestamp();
    let mut meta = metadata(
        &id,
        &format!("{} · 学习记录", section.metadata.title),
        "session",
    );
    meta.links = vec![section_id.to_string()];
    meta.extra.insert("sectionId".into(), json!(section_id));
    meta.extra
        .insert("associateCurrentSection".into(), json!(true));
    meta.extra.insert("createdAt".into(), json!(stamp));
    meta.extra.insert("contextManifest".into(), json!(manifest));
    let path = format!("sessions/{id}/session.md");
    let body = format!("# {}\n\n本次学习记录。每条提问、练习和作答存于同目录 entries/ 的 Markdown 文件；课程正文不被改写。\n", meta.title);
    store.create(meta, &body, &path)?;
    get(store, &id)
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
            let doc = store.register(&relative)?;
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
        id: id.into(),
        section_id,
        title: doc.metadata.title,
        path: doc.path,
        version: hash(&versions.join("\n")),
        created_at: created,
        updated_at: updated,
        entries,
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
    if session.section_id.is_empty() && !manifest.is_empty() {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "独立探索不能写入课程来源。",
        ));
    }
    let mut doc = store.read(session_id)?;
    doc.metadata
        .extra
        .insert("contextManifest".into(), json!(manifest));
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
    if entry.related_ids.len() > 64 || entry.related_ids.iter().any(|s| !valid_id(s)) {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "关联记录编号无效或数量过多。",
        ));
    }
    Ok(())
}

pub fn append(
    store: &mut ContentStore,
    session_id: &str,
    mut entry: Entry,
    expected: Option<&str>,
    request_id: &str,
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
    if entry.kind == "attempt" {
        let exercise_id = entry
            .data
            .as_ref()
            .and_then(|d| d.get("exerciseId"))
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::new("INVALID_INPUT", "请先选择对应练习再提交作答。"))?;
        if !old
            .entries
            .iter()
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
    if expected.is_some_and(|v| v != old.version) {
        return Err(CoreError::new(
            "CONFLICT",
            "学习记录已在另一处更新。输入仍保留，请重新载入后再保存。",
        ));
    }
    // A personal correction refines a historical error without discarding its reference answer.
    if entry.kind == "error" {
        if let Some(data) = entry.data.as_mut() {
            if let (Some(exercise_id), Some(cause)) = (
                data.get("exerciseId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                data.get("cause")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string),
            ) {
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
                    }
                }
            }
        }
    }
    entry.id = id;
    entry.created_at = timestamp();
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

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .trim_matches(|c: char| ".。!！".contains(c))
        .to_string()
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
    let correct = exact.then(|| reference.iter().any(|a| normalize(a) == normalize(answer)));
    let explanation = exercise
        .get("explanation")
        .and_then(Value::as_str)
        .unwrap_or("");
    let content = match correct { Some(true) => format!("答案正确。\n\n{explanation}"), Some(false) => format!("本次答案未匹配参考答案。\n\n参考：{}\n\n{explanation}\n\n错因待本人确认，可补充或修正。", reference.join(" / ")), None => format!("作答已保存，开放题尚未自动判分。\n\n参考：{}\n\n{explanation}\n\n可发起AI反馈或自行记录理解。", reference.join(" / ")) };
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
        let error = Entry {
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
        append(
            store,
            session_id,
            error,
            None,
            &format!("error-{}", &hash(request_id)[..32]),
        )?;
    }
    Ok(())
}

pub fn export_csv(
    store: &mut ContentStore,
    session_id: &str,
    destination: &Path,
) -> Result<(String, usize)> {
    get(store, session_id)?;
    let cards = review_items(store, "")?
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
        .map(str::to_string)
        .collect::<Vec<_>>();
    let summaries = store.summaries();
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
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
            store
                .read_relative(&format!("sessions/{session_id}/session.md"))
                .ok()
                .and_then(|d| {
                    d.metadata
                        .extra
                        .get("sectionId")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
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
    let mut items = grouped
        .into_values()
        .filter(|item| {
            let search = item.to_string().to_lowercase();
            tokens.iter().all(|t| search.contains(t))
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| b["createdAt"].as_str().cmp(&a["createdAt"].as_str()));
    items.truncate(200);
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
