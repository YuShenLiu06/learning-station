//! Course routing is separate from the document frozen for one request.
//! Legacy conversations remain independent, backed up and never stitched into Pi.
use crate::{
    app::{atomic_json, Core},
    content::ContentStore,
    library_tree, sessions,
    types::*,
};
use serde_json::json;
use std::{fs, path::Path};

pub fn course(store: &ContentStore, document: &str) -> Result<Option<String>> {
    let root = library_tree::course_id(store, document)?;
    Ok((store.read(&root)?.metadata.kind == "course").then_some(root))
}

pub fn source(store: &ContentStore, session: &Session, document: &str) -> Result<String> {
    if document.is_empty() && (session.course_id.is_some() || session.section_id.is_empty()) {
        return Ok(String::new());
    }
    if store.is_trashed(document) {
        return Err(CoreError::new("TRASHED", "当前材料已在回收站。"));
    }
    let doc = store.read(document)?;
    let source = if doc.metadata.kind == "exercise" {
        crate::exercises::origin(store, &doc)?.source_document_id
    } else {
        document.to_owned()
    };
    let material = store.read(&source)?;
    if store.is_trashed(&source)
        || !matches!(
            material.metadata.kind.as_str(),
            "section" | "knowledge" | "exploration"
        )
    {
        return Err(scope());
    }
    if let Some(owner) = &session.course_id {
        if course(store, document)?.as_ref() != Some(owner)
            || course(store, &source)?.as_ref() != Some(owner)
        {
            return Err(scope());
        }
    } else if source != session.section_id {
        return Err(scope());
    }
    Ok(source)
}

fn scope() -> CoreError {
    CoreError::new("OUT_OF_SCOPE", "材料不属于本课程会话，未带入其他课程。")
}

// Retain already-seen course history when Focus changes. This does not inject
// those files or enlarge ToolScope.allowed_ids / read-tool authorization.
pub fn history_sources(
    store: &ContentStore,
    session: &Session,
    current: &[ManifestItem],
) -> Result<Vec<String>> {
    let mut ids: std::collections::BTreeSet<_> = current.iter().map(|m| m.id.clone()).collect();
    if let Some(owner) = &session.course_id {
        ids.extend(
            library_tree::list(store)?
                .nodes
                .into_iter()
                .filter(|n| &n.course_id == owner)
                .map(|n| n.id),
        );
        ids.extend(crate::assets::list(store, owner)?.into_iter().map(|a| a.id));
    }
    Ok(ids.into_iter().collect())
}

pub fn validate_injections(store: &ContentStore, session: &Session, inputs: &[crate::context::ContextInjection]) -> Result<()> {
    if session.course_id.is_some() {
        for input in inputs {
            source(store, session, &input.document_id)?;
        }
    }
    Ok(())
}

/// Omitted material on a course chat means no automatic note context.
pub fn request_document<'a>(session: &'a Session, document: Option<&'a str>) -> &'a str {
    document.unwrap_or(if session.course_id.is_some() { "" } else { &session.section_id })
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseSessions {
    pub course_id: String,
    pub active_session_id: Option<String>,
    pub sessions: Vec<DocumentSummary>,
}

fn check_course(core: &mut Core, owner: &str) -> Result<()> {
    if core.content.is_trashed(owner) {
        return Err(CoreError::new("TRASHED", "课程已在回收站，请先恢复。"));
    }
    let document = match core.content.read(owner) {
        Err(error) if error.code == "NOT_FOUND" => { core.content.refresh()?; core.content.read(owner)? }
        result => result?,
    };
    if document.metadata.kind != "course"
        || course(&core.content, owner)?.as_deref() != Some(owner)
    {
        return Err(scope());
    }
    Ok(())
}

pub fn list(core: &mut Core, owner: &str) -> Result<CourseSessions> {
    core.content.refresh()?;
    check_course(core, owner)?;
    let mut items = vec![];
    for item in core.content.summaries().into_iter().filter(|d| d.kind == "session") {
        let doc = core.content.read(&item.id)?;
        if doc.metadata.extra.get("courseId").and_then(|v| v.as_str()) == Some(owner) {
            items.push(item);
        }
    }
    items.sort_by(|a, b| b.modified_at.cmp(&a.modified_at).then(a.id.cmp(&b.id)));
    let selected = core.preferences.active_session_by_course.get(owner)
        .or_else(|| core.preferences.session_by_course.get(owner));
    let active_session_id = selected.filter(|id| items.iter().any(|s| &s.id == *id)).cloned();
    Ok(CourseSessions { course_id: owner.into(), active_session_id, sessions: items })
}

pub fn select(core: &mut Core, owner: &str, id: &str) -> Result<Session> {
    check_course(core, owner)?;
    let session = sessions::get(&mut core.content, id)?;
    if core.content.is_trashed(id) || session.course_id.as_deref() != Some(owner) {
        return Err(scope());
    }
    core.preferences.active_session_by_course.insert(owner.into(), id.into());
    core.preferences.active_session_id = Some(id.into());
    core.save_preferences()?;
    Ok(session)
}

pub fn create(core: &mut Core, owner: &str, request: &str) -> Result<Session> {
    core.content.refresh()?;
    check_course(core, owner)?;
    if request.len() < 8 || request.len() > 128 || request.chars().any(char::is_control) {
        return Err(CoreError::new("INVALID_INPUT", "新对话缺少有效防重复编号。"));
    }
    // Global request identity rejects reuse in another course instead of reassigning it.
    let id = format!("chat_{}", &hash(request)[..32]);
    sessions::create_course(&mut core.content, "", &[], &id, owner, &[])?;
    select(core, owner, &id)
}

pub fn rename(core: &mut Core, owner: &str, id: &str, title: &str, expected: &str) -> Result<Session> {
    check_course(core, owner)?;
    let session = sessions::get(&mut core.content, id)?;
    if session.course_id.as_deref() != Some(owner) { return Err(scope()); }
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 100 || title.chars().any(char::is_control) {
        return Err(CoreError::new("INVALID_INPUT", "对话名称请填写1到100个字符，不含控制字符。"));
    }
    if session.title == title { return Ok(session); }
    if session.version != expected {
        return Err(CoreError::new("CONFLICT", "对话已更新，请刷新后重试；未覆盖原记录。"));
    }
    let doc = core.content.read(id)?;
    let mut metadata = doc.metadata;
    metadata.title = title.into();
    let heading = format!("# {}\n", session.title);
    let body = doc.body.strip_prefix(&heading)
        .map(|rest| format!("# {title}\n{rest}"))
        .unwrap_or(doc.body);
    core.content.save(id, &crate::content::serialize(&metadata, &body)?, &doc.version)?;
    sessions::get(&mut core.content, id)
}

pub fn open(core: &mut Core, document: &str, owner: &str) -> Result<(Session, Context)> {
    let available = list(core, owner)?;
    let selected = core.preferences.active_session_by_course.get(owner)
        .or_else(|| core.preferences.session_by_course.get(owner)).cloned()
        .or_else(|| (available.sessions.len() == 1).then(|| available.sessions[0].id.clone()));
    let result = if let Some(id) = selected {
        let session = sessions::get(&mut core.content, &id)?;
        if session.course_id.as_deref() != Some(owner) { return Err(scope()); }
        core.session_context_for(&id, Some(document), None)?
    } else if available.sessions.is_empty() {
        open_legacy(core, document, owner)?
    } else {
        return Err(CoreError::new("SESSION_SELECTION_REQUIRED", "本课程有多个对话，请先选择要继续的对话；原记录均保留。"));
    };
    if result.0.id == format!("course_{}", &hash(owner)[..32]) {
        core.preferences.session_by_course.entry(owner.into()).or_insert_with(|| result.0.id.clone());
    }
    select(core, owner, &result.0.id)?;
    core.preferences.active_document_id = Some(document.into());
    core.save_preferences()?;
    Ok(result)
}

fn open_legacy(core: &mut Core, document: &str, owner: &str) -> Result<(Session, Context)> {
    let id = core
        .preferences
        .session_by_course
        .get(owner)
        .cloned()
        .unwrap_or_else(|| format!("course_{}", &hash(owner)[..32]));
    let session = match sessions::get(&mut core.content, &id) {
        Ok(session) => session,
        Err(_)
            if !core.preferences.session_by_course.contains_key(owner)
                && fs::symlink_metadata(
                    core.content
                        .root()
                        .join(format!("sessions/{id}/session.md")),
                )
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            core.content.refresh()?;
            let mut legacy = vec![];
            for item in core
                .content
                .summaries()
                .into_iter()
                .filter(|d| d.kind == "session")
            {
                let doc = core.content.read(&item.id)?;
                let anchor = doc
                    .metadata
                    .extra
                    .get("sectionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !anchor.is_empty()
                    && course(&core.content, anchor).ok().flatten().as_deref() == Some(owner)
                {
                    legacy.push(item.id);
                }
            }
            legacy.sort();
            backup(core, owner, &legacy)?;
            let focus = if core.content.read(document)?.metadata.kind == "exercise" {
                crate::exercises::origin(&core.content, &core.content.read(document)?)?.source_document_id
            } else {
                document.to_owned()
            };
            let ctx = crate::context::load(&core.content, &focus, None)?;
            sessions::create_course(
                &mut core.content,
                &focus,
                &ctx.manifest,
                &id,
                owner,
                &legacy,
            )?
        }
        Err(e) => return Err(e),
    };
    if session.course_id.as_deref() != Some(owner) {
        return Err(scope());
    }
    let result = core.session_context_for(&session.id, Some(document), None)?;
    core.preferences
        .session_by_course
        .insert(owner.into(), session.id.clone());
    core.preferences.active_document_id = Some(document.into());
    core.preferences.active_session_id = Some(session.id);
    core.save_preferences()?;
    Ok(result)
}

fn backup(core: &Core, owner: &str, legacy: &[String]) -> Result<()> {
    if legacy.is_empty() {
        return Ok(());
    }
    let parent = core.state_dir.join("course-session-backups");
    fs::create_dir_all(&parent)?;
    if fs::symlink_metadata(&parent)?.file_type().is_symlink() {
        return Err(scope());
    }
    let destination = parent.join(hash(owner));
    if destination.exists() {
        let meta = fs::symlink_metadata(&destination)?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(scope());
        }
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(destination.join("complete.json"))?)?;
        if saved["courseId"] != owner || saved["legacySessionIds"] != json!(legacy) {
            return Err(CoreError::new(
                "CONFLICT",
                "旧会话清单与已有备份不一致；原记录仍保留，未切换课程会话。",
            ));
        }
        return Ok(());
    }
    let temp = tempfile::tempdir_in(&parent)?;
    atomic_json(&temp.path().join("preferences.json"), &core.preferences)?;
    for id in legacy {
        copy(
            &core.content.root().join("sessions").join(id),
            &temp.path().join("sessions").join(id),
            0,
        )?;
        let native = core.state_dir.join("pi-native").join(hash(id));
        if native.join("active.lock").try_exists()? {
            return Err(CoreError::new(
                "BUSY",
                "旧会话仍有运行锁；请先核对并停止原运行，再建立课程共享对话。",
            ));
        }
        if native.try_exists()? {
            copy(&native, &temp.path().join("pi-native").join(hash(id)), 0)?;
        }
    }
    atomic_json(
        &temp.path().join("complete.json"),
        &json!({"version":1,"courseId":owner,"legacySessionIds":legacy}),
    )?;
    fs::rename(temp.path(), &destination)?;
    fs::File::open(&parent)?.sync_all()?;
    Ok(())
}

fn copy(from: &Path, to: &Path, depth: usize) -> Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if depth > 3 || meta.file_type().is_symlink() {
        return Err(scope());
    }
    if meta.is_dir() {
        fs::create_dir_all(to)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(to, fs::Permissions::from_mode(0o700))?;
        }
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy(&entry.path(), &to.join(entry.file_name()), depth + 1)?;
        }
        fs::File::open(to)?.sync_all()?;
    } else if meta.is_file() && meta.len() <= 32 * 1024 * 1024 {
        fs::copy(from, to)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(to, fs::Permissions::from_mode(0o600))?;
        }
        fs::File::open(to)?.sync_all()?;
    } else {
        return Err(scope());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::Engine,
        exercises::{self, ManualExercise},
        native_session,
    };
    fn node(core: &mut Core, kind: &str, parent: Option<&str>, name: &str) -> Document {
        library_tree::create(
            &mut core.content,
            kind,
            name,
            parent,
            name,
            &format!("course-test-{name}"),
        )
        .unwrap()
    }
    fn exercise(core: &mut Core, session: &str, doc: &str) -> exercises::ExerciseDocument {
        exercises::create_manual(
            &mut core.content,
            session,
            doc,
            ManualExercise {
                question: "Synthetic question?".into(),
                format: "exact".into(),
                answers: vec!["answer".into()],
                explanation: "Synthetic explanation".into(),
            },
            &format!("exercise-{doc}"),
        )
        .unwrap()
    }
    fn question(content: &str, document: &str, injections: Vec<crate::context::ContextInjection>) -> Entry {
        Entry {
            id: String::new(), kind: "user".into(), content: content.into(),
            created_at: timestamp(), selection: None, related_ids: vec![],
            data: Some(json!({"mode":"question","requestedContext":{
                "documentId":document,"selection":"","injections":injections
            }})),
        }
    }

    #[test]
    fn material_free_chats_keep_selection_history_and_names_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        let state = dir.path().join("state");
        let engine = Engine::new(root.clone(), state.clone()).unwrap();
        let (owner, first, second, version) = {
            let mut c = engine.lock().unwrap();
            let owner = node(&mut c, "course", None, "empty-course");
            assert!(list(&mut c, &owner.id).unwrap().sessions.is_empty());
            let first = create(&mut c, &owner.id, "create-empty-chat-one").unwrap();
            let second = create(&mut c, &owner.id, "create-empty-chat-two").unwrap();
            assert_ne!(first.id, second.id);
            assert_eq!(first.course_id.as_deref(), Some(owner.id.as_str()));
            assert!(first.section_id.is_empty());
            assert!(first.context_manifest.is_empty());
            assert_eq!(create(&mut c, &owner.id, "create-empty-chat-one").unwrap().id, first.id);
            let saved = sessions::append(&mut c.content, &first.id, question("Only chat one", "", vec![]), None, "empty-chat-first-question").unwrap();
            assert_eq!(saved.entries[0].data.as_ref().unwrap()["sourceDocumentId"], "");
            assert!(select(&mut c, &owner.id, &second.id).unwrap().entries.is_empty());
            let renamed = rename(&mut c, &owner.id, &first.id, "  主题 一  ", &saved.version).unwrap();
            assert_eq!(renamed.title, "主题 一");
            assert_eq!(renamed.entries.len(), 1);
            assert_eq!(rename(&mut c, &owner.id, &first.id, "stale edit", &saved.version).unwrap_err().code, "CONFLICT");
            assert!(rename(&mut c, &owner.id, &first.id, "bad\nname", &renamed.version).is_err());
            let chosen = select(&mut c, &owner.id, &first.id).unwrap();
            assert_eq!(list(&mut c, &owner.id).unwrap().sessions.len(), 2);
            assert_eq!(c.preferences.active_document_id, None);
            (owner.id, first.id, second.id, chosen.version)
        };
        let boot = engine.state().unwrap();
        assert_eq!(boot.last_session_id.as_deref(), Some(first.as_str()));
        assert_eq!(boot.last_course_id.as_deref(), Some(owner.as_str()));
        drop(engine);
        let engine = Engine::new(root, state).unwrap();
        let boot = engine.state().unwrap();
        assert_eq!(boot.last_session_id.as_deref(), Some(first.as_str()));
        assert_eq!(boot.last_course_id.as_deref(), Some(owner.as_str()));
        let mut c = engine.lock().unwrap();
        // Selection also works before the caller requests a library scan/list.
        assert_eq!(select(&mut c, &owner, &first).unwrap().id, first);
        let menu = list(&mut c, &owner).unwrap();
        assert_eq!(menu.sessions.len(), 2);
        assert_eq!(menu.active_session_id.as_deref(), Some(first.as_str()));
        let resumed = select(&mut c, &owner, &first).unwrap();
        assert_eq!(resumed.version, version);
        assert_eq!(resumed.title, "主题 一");
        assert_eq!(resumed.entries[0].content, "Only chat one");
        assert!(select(&mut c, &owner, &second).unwrap().entries.is_empty());
    }

    #[test]
    fn fixed_chat_and_native_files_are_retained_when_creating_and_selecting_other_chats() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut c = engine.lock().unwrap();
        let owner = node(&mut c, "course", None, "retained-course");
        let a = node(&mut c, "section", Some(&owner.id), "retained-note-a");
        let b = node(&mut c, "section", Some(&owner.id), "retained-note-b");
        let fixed = c.start_session(&a.id, false).unwrap().0;
        let fixed = sessions::append(&mut c.content, &fixed.id, question("Old chat history", &a.id, vec![]), None, "retained-old-question").unwrap();
        let bytes = fs::read(c.content.root().join(&fixed.path)).unwrap();
        let native = native_session::prepare(&c.state_dir, &fixed, "old_run", "old_user", vec![], false, vec![], "").unwrap();
        let transcript = native.directory.join("transcript.jsonl");
        fs::write(&transcript, "{\"type\":\"session\",\"version\":3,\"id\":\"synthetic-fixed\"}\n").unwrap();
        let original_native = fs::read(&transcript).unwrap();
        let binding = fs::read(native.directory.join("binding.json")).unwrap();
        let new = create(&mut c, &owner.id, "retained-new-chat").unwrap();
        let other_native = native_session::prepare(&c.state_dir, &new, "new_run", "new_user", vec![], false, vec![], "").unwrap();
        assert_ne!(native.directory, other_native.directory);
        assert!(new.entries.is_empty());
        assert!(new.legacy_session_ids.is_empty());
        assert_eq!(c.start_session(&b.id, false).unwrap().0.id, new.id);
        assert!(c.session_context(&new.id, None).unwrap().1.manifest.is_empty());
        assert!(c.session_context(&fixed.id, None).unwrap().1.manifest.is_empty());
        assert!(c.session_context_for(&fixed.id, Some(""), None).unwrap().1.text.is_empty());
        assert_eq!(c.session_context_for(&new.id, Some(&b.id), None).unwrap().1.manifest[0].id, b.id);
        let reader = c.preferences.active_document_id.clone();
        let resumed = select(&mut c, &owner.id, &fixed.id).unwrap();
        assert_eq!(c.preferences.active_document_id, reader);
        assert_eq!(resumed.entries[0].content, "Old chat history");
        assert_eq!(fs::read(c.content.root().join(&fixed.path)).unwrap(), bytes);
        assert_eq!(fs::read(&transcript).unwrap(), original_native);
        assert_eq!(fs::read(native.directory.join("binding.json")).unwrap(), binding);
        assert_eq!(list(&mut c, &owner.id).unwrap().sessions.len(), 2);
        assert_eq!(c.preferences.session_by_course[&owner.id], fixed.id);
    }

    #[test]
    fn course_scope_rejects_foreign_selection_sources_and_frozen_injections() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut c = engine.lock().unwrap();
        let owner = node(&mut c, "course", None, "scope-course");
        let a = node(&mut c, "section", Some(&owner.id), "scope-note");
        let foreign = node(&mut c, "course", None, "foreign-course");
        let b = node(&mut c, "section", Some(&foreign.id), "foreign-note");
        let chat = create(&mut c, &owner.id, "scope-own-chat").unwrap();
        let foreign_chat = create(&mut c, &foreign.id, "scope-foreign-chat").unwrap();
        assert_eq!(select(&mut c, &owner.id, &foreign_chat.id).unwrap_err().code, "OUT_OF_SCOPE");
        assert_eq!(create(&mut c, &foreign.id, "scope-own-chat").unwrap_err().code, "CONFLICT");
        assert_eq!(rename(&mut c, &foreign.id, &chat.id, "steal", &chat.version).unwrap_err().code, "OUT_OF_SCOPE");
        assert_eq!(c.session_context_for(&chat.id, Some(&b.id), None).unwrap_err().code, "OUT_OF_SCOPE");
        let input = crate::context::ContextInjection { document_id: b.id.clone(), expected_version: b.version, selection: None };
        let mut ctx = c.session_context(&chat.id, None).unwrap().1;
        assert_eq!(crate::exercise_references::inject_request(&mut c.content, &mut ctx, &chat, &[input.clone()], &[], &[]).unwrap_err().code, "OUT_OF_SCOPE");
        assert_eq!(sessions::append(&mut c.content, &chat.id, question("Forged quote", "", vec![input.clone()]), None, "scope-forged-quote").unwrap_err().code, "OUT_OF_SCOPE");
        assert!(sessions::get(&mut c.content, &chat.id).unwrap().entries.is_empty());
        let mut forged_source = question("Invented anchor", "", vec![]);
        forged_source.data.as_mut().unwrap()["sourceDocumentId"] = json!(a.id);
        assert_eq!(sessions::append(&mut c.content, &chat.id, forged_source, None, "scope-invented-anchor").unwrap_err().code, "CONTEXT_CONFLICT");
        let own = crate::context::ContextInjection { document_id: a.id.clone(), expected_version: a.version, selection: Some("scope-note".into()) };
        crate::exercise_references::inject_request(&mut c.content, &mut ctx, &chat, &[own], &[], &[]).unwrap();
        assert_eq!(ctx.manifest[0].id, a.id);
        let independent = sessions::create_exploration(&mut c.content).unwrap();
        let mut empty = c.session_context(&independent.id, None).unwrap().1;
        crate::exercise_references::inject_request(&mut c.content, &mut empty, &independent, &[input], &[], &[]).unwrap();
        assert_eq!(empty.manifest[0].id, b.id);
        let exercise = exercise(&mut c, &chat.id, &a.id);
        assert_eq!(source(&c.content, &foreign_chat, &exercise.document.id).unwrap_err().code, "OUT_OF_SCOPE");
        assert_eq!(exercises::get(&mut c.content, &exercise.document.id).unwrap().session.id, chat.id);
    }

    #[test]
    fn material_free_chat_tools_save_without_fabricating_a_source() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let chat = {
            let mut c = engine.lock().unwrap();
            let owner = node(&mut c, "course", None, "tool-course");
            create(&mut c, &owner.id, "tool-course-chat").unwrap()
        };
        let scope = crate::learning_tools::ToolScope {
            session_id: chat.id.clone(), section_id: "".into(), run_id: "run_empty_tool".into(), allowed_ids: vec![],
        };
        let receipt = crate::learning_tools::execute(&engine, &scope, "record_empty", "record_question", json!({"question":"A course question"})).unwrap();
        assert_eq!(receipt["persisted"], true);
        let card = crate::learning_tools::execute(&engine, &scope, "card_empty", "create_review_card", json!({"front":"No materials yet","back":"Synthetic only"})).unwrap();
        assert_eq!(card["persisted"], true);
        let items = crate::review::list_for_course(&mut engine.lock().unwrap().content, "No materials yet", chat.course_id.as_deref()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["sourceDocumentId"], serde_json::Value::Null);
        let saved = sessions::get(&mut engine.lock().unwrap().content, &chat.id).unwrap();
        assert!(saved.entries[0].related_ids.is_empty());
        assert!(saved.section_id.is_empty());
        assert_eq!(crate::learning_tools::execute(&engine, &scope, "practice_empty", "generate_exercises", json!({"requirements":"Practice","summary":"","exercises":[]})).unwrap_err().code, "SOURCE_REQUIRED");
    }

    #[tokio::test]
    async fn practice_without_a_source_is_refused_before_model_access() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let session = {
            let mut c = engine.lock().unwrap();
            let owner = node(&mut c, "course", None, "practice-course");
            create(&mut c, &owner.id, "practice-no-source").unwrap()
        };
        let request = crate::generation::GenerationRequest {
            document_id: Some("".into()), session_id: session.id.clone(), mode: "practice".into(),
            selection: "".into(), prompt: "Create exercises".into(), request_id: "practice-empty-source".into(),
            user_entry_id: None, injections: vec![], assistant_references: vec![], exercise_references: vec![],
        };
        assert_eq!(crate::generation::start(engine.clone(), request, std::sync::Arc::new(|_| {})).await.unwrap_err().code, "SOURCE_REQUIRED");
        assert!(sessions::get(&mut engine.lock().unwrap().content, &session.id).unwrap().entries.is_empty());
    }

    #[test]
    fn shared_course_routes_keep_frozen_source_and_native_binding_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        let state = dir.path().join("state");
        let engine = Engine::new(root.clone(), state.clone()).unwrap();
        let (a, b, other, owner, sid, native) = {
            let mut core = engine.lock().unwrap();
            let owner = node(&mut core, "course", None, "owner");
            let a = node(&mut core, "section", Some(&owner.id), "note-a");
            let b = node(&mut core, "section", Some(&owner.id), "note-b");
            let foreign = node(&mut core, "course", None, "foreign");
            let other = node(&mut core, "section", Some(&foreign.id), "other-note");
            let session = core.start_session(&a.id, false).unwrap().0;
            assert_eq!(core.start_session(&b.id, false).unwrap().0.id, session.id);
            assert_ne!(
                core.start_session(&other.id, false).unwrap().0.id,
                session.id
            );
            let frozen = core
                .session_context_for(&session.id, Some(&b.id), None)
                .unwrap()
                .1;
            let allowed = history_sources(&core.content, &session, &frozen.manifest).unwrap();
            assert!(allowed.contains(&a.id) && allowed.contains(&b.id));
            assert!(!allowed.contains(&other.id));
            core.start_session(&a.id, false).unwrap();
            assert_eq!(frozen.manifest[0].id, b.id);
            assert_eq!(
                core.session_context_for(&session.id, Some(&other.id), None)
                    .unwrap_err()
                    .code,
                "OUT_OF_SCOPE"
            );
            let item = exercise(&mut core, &session.id, &b.id);
            assert_eq!(
                core.start_session(&item.document.id, false).unwrap().0.id,
                session.id
            );
            assert_eq!(item.document.metadata.extra["sourceDocumentId"], b.id);
            let n = native_session::prepare(
                &state,
                &session,
                "course_run",
                "course_user",
                vec![],
                false,
                vec![],
                "",
            )
            .unwrap();
            (a.id, b.id, other.id, owner.id, session.id, n.directory)
        };
        drop(engine);
        let engine = Engine::new(root, state.clone()).unwrap();
        let mut core = engine.lock().unwrap();
        core.content.refresh().unwrap();
        assert_eq!(core.start_session(&a, false).unwrap().0.id, sid);
        assert_eq!(core.start_session(&b, false).unwrap().0.id, sid);
        assert_ne!(core.start_session(&other, false).unwrap().0.id, sid);
        core.preferences.session_by_course.clear();
        core.save_preferences().unwrap();
        let session = core.start_session(&b, false).unwrap().0;
        assert_eq!(session.id, sid);
        assert_eq!(core.preferences.session_by_course[&owner], sid);
        assert_eq!(
            native_session::prepare(
                &state,
                &session,
                "retry_run",
                "same_user",
                vec![],
                false,
                vec![],
                ""
            )
            .unwrap()
            .directory,
            native
        );
    }
    #[test]
    fn legacy_histories_are_backed_up_without_merging_and_exercise_ledger_stays_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut core = engine.lock().unwrap();
        let owner = node(&mut core, "course", None, "legacy-owner");
        let a = node(&mut core, "section", Some(&owner.id), "legacy-a");
        let b = node(&mut core, "section", Some(&owner.id), "legacy-b");
        let old_a = sessions::create(&mut core.content, &a.id, &[]).unwrap();
        let old_b = sessions::create(&mut core.content, &b.id, &[]).unwrap();
        let ex = exercise(&mut core, &old_b.id, &b.id);
        let original = fs::read(core.content.root().join(&old_b.path)).unwrap();
        let native = core.state_dir.join("pi-native").join(hash(&old_b.id));
        fs::create_dir_all(&native).unwrap();
        fs::write(
            native.join("transcript.jsonl"),
            "synthetic legacy transcript\n",
        )
        .unwrap();
        let session = core.start_session(&a.id, false).unwrap().0;
        assert_eq!(session.legacy_session_ids.len(), 2);
        assert!(session.entries.is_empty());
        assert_ne!(session.id, old_a.id);
        assert_ne!(session.id, old_b.id);
        assert_eq!(
            fs::read(core.content.root().join(&old_b.path)).unwrap(),
            original
        );
        let backup = core
            .state_dir
            .join("course-session-backups")
            .join(hash(&owner.id));
        assert_eq!(fs::read(backup.join(&old_b.path)).unwrap(), original);
        assert_eq!(
            fs::read(
                backup
                    .join("pi-native")
                    .join(hash(&old_b.id))
                    .join("transcript.jsonl")
            )
            .unwrap(),
            b"synthetic legacy transcript\n"
        );
        assert_eq!(
            core.start_session(&ex.document.id, false).unwrap().0.id,
            session.id
        );
        assert_eq!(
            exercises::get(&mut core.content, &ex.document.id)
                .unwrap()
                .session
                .id,
            old_b.id
        );
        let question_id = ex.exercises["exercises"][0]["id"].as_str().unwrap();
        let p = crate::exercise_references::load(
            &mut core.content,
            &old_b.id,
            &ex.document.id,
            question_id,
            "question",
            None,
        )
        .unwrap();
        let r = crate::exercise_references::ExerciseReference {
            session_id: old_b.id,
            document_id: ex.document.id,
            exercise_entry_id: p.exercise_entry_id,
            question_id: question_id.into(),
            part: "question".into(),
            entry_id: p.entry_id,
            text_hash: p.text_hash,
            selection: p.text,
        };
        assert!(
            crate::exercise_references::validate_references(&mut core.content, &session, &[r])
                .is_ok()
        );
    }
    #[cfg(unix)]
    #[test]
    fn failed_backup_does_not_switch_route_or_touch_original_history() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut core = engine.lock().unwrap();
        let owner = node(&mut core, "course", None, "blocked-owner");
        let a = node(&mut core, "section", Some(&owner.id), "blocked-note");
        let old = sessions::create(&mut core.content, &a.id, &[]).unwrap();
        let native = core.state_dir.join("pi-native").join(hash(&old.id));
        fs::create_dir_all(&native).unwrap();
        fs::write(native.join("active.lock"), "fixture").unwrap();
        assert_eq!(core.start_session(&a.id, false).unwrap_err().code, "BUSY");
        assert!(!core.preferences.session_by_course.contains_key(&owner.id));
        fs::remove_file(native.join("active.lock")).unwrap();
        std::os::unix::fs::symlink(dir.path(), native.join("escape")).unwrap();
        assert!(core.start_session(&a.id, false).is_err());
        fs::remove_file(native.join("escape")).unwrap();
        assert!(core.start_session(&a.id, false).is_ok());
        assert!(sessions::get(&mut core.content, &old.id).is_ok());
    }
}
