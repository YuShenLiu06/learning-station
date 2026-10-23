//! Stable application session mapping. Pi owns the transcript; this module owns its binding
//! and an immutable legacy backup. A missing initialized transcript is an error, never a new chat.
use crate::types::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeSession {
    pub directory: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_dir: Option<PathBuf>,
    pub learning_session_id: String,
    pub run_id: String,
    pub user_entry_id: String,
    pub authorized_source_ids: Vec<String>,
    pub protect_history: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_boundary_key: Option<String>,
    pub legacy_records: Vec<Value>,
}
pub(crate) fn project_root(
    store: &crate::content::ContentStore,
    session: &Session,
) -> Result<Option<PathBuf>> {
    let Some(owner) = &session.course_id else {
        return Ok(None);
    };
    // Recycle retains physical files; validate current visibility before handing Pi a cwd.
    store.read(owner)?;
    match crate::course_project::course_project_root(store.root(), owner) {
        Ok(root) => {
            // Reuse the course Git service; Pi's native .agents discovery needs
            // this real project boundary before walking ancestor directories.
            crate::material_git::MaterialGit::init_course(store.root(), owner)?;
            Ok(Some(root))
        }
        Err(error) if error.code == "COURSE_PROJECT_REQUIRED" => Ok(None),
        Err(error) => Err(error),
    }
}
pub fn error() -> CoreError {
    CoreError::new(
        "NATIVE_SESSION_INVALID",
        "原生学习会话缺失或损坏；旧记录仍保留，未静默创建新会话。请保留本机状态目录后恢复备份。",
    )
}
pub fn atomic(path: &Path, value: &Value) -> Result<()> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or_else(error)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&serde_json::to_vec(value)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(CoreError::io)?;
    fs::File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
pub fn prepare(
    state: &Path,
    session: &Session,
    run: &str,
    user: &str,
    authorized: Vec<String>,
    protected: bool,
    records: Vec<Value>,
    markdown: &str,
) -> Result<NativeSession> {
    let root = state.join("pi-native");
    for path in [&root, &root.join(hash(&session.id))] {
        match fs::symlink_metadata(path) {
            Ok(m) if !m.is_dir() || m.file_type().is_symlink() => return Err(error()),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)?,
            Err(e) => return Err(CoreError::io(e)),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    let directory = fs::canonicalize(root.join(hash(&session.id)))?;
    let binding = directory.join("binding.json");
    if binding.exists() {
        let meta = fs::symlink_metadata(&binding)?;
        if meta.file_type().is_symlink() || meta.len() > 16384 {
            return Err(error());
        }
        let data: Value = serde_json::from_slice(&fs::read(&binding)?).map_err(|_| error())?;
        if data["version"] != 1
            || data["learningSessionId"] != session.id
            || data["transcript"] != "transcript.jsonl"
        {
            return Err(error());
        }
        if data["initialized"] == true && !directory.join("transcript.jsonl").is_file() {
            return Err(error());
        }
    } else {
        if fs::read_dir(&directory)?.next().is_some() {
            return Err(error());
        }
        // Snapshot is never used as a fabricated native transcript, and never replaces Markdown.
        {
            use std::io::Write;
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let path = directory.join("legacy-backup.md");
            if !path.exists() {
                let mut file = options.open(&path)?;
                file.write_all(markdown.as_bytes())?;
                file.sync_all()?;
            }
        }
        atomic(
            &directory.join("legacy-backup.json"),
            &json!({"version":1,"session":session,"sourceVersion":session.version}),
        )?;
        atomic(
            &binding,
            &json!({"version":1,"learningSessionId":session.id,"transcript":"transcript.jsonl","initialized":false}),
        )?;
    }
    let initialized: Value = serde_json::from_slice(&fs::read(&binding)?).map_err(|_| error())?;
    let records = if initialized["initialized"] == true {
        vec![]
    } else {
        records
    };
    let history_boundary_key = if session.course_id.is_some() && protected { Some(hash(&serde_json::to_string(&session.context_excluded_history_source_ids)?)) } else { None };
    Ok(NativeSession {
        directory,
        project_root: None,
        agent_dir: None,
        learning_session_id: session.id.clone(),
        run_id: run.into(),
        user_entry_id: user.into(),
        authorized_source_ids: authorized,
        protect_history: protected,
        history_boundary_key,
        legacy_records: records,
    })
}

/// Node permission mode intentionally forbids fsync. Only Rust syncs its exact private
/// session directory; the sidecar supplies no path and cannot extend the filesystem scope.
pub fn sync_directory(directory: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(directory)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(error());
    }
    let entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    if entries.len() > 8192 {
        return Err(error());
    }
    for entry in entries {
        let p = entry.path();
        let meta = fs::symlink_metadata(&p)?;
        if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > 32 * 1024 * 1024 {
            return Err(error());
        }
        fs::File::open(&p)?.sync_all()?;
    }
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

/// Read committed business objects by the original stable operation ID; never execute a tool.
pub fn recover(
    engine: &crate::app::Engine,
    session_id: &str,
    run_id: &str,
    call: &crate::model::ToolCall,
) -> Result<Value> {
    let unknown = || CoreError::new("NATIVE_TOOL_UNKNOWN", "原工具结果尚不可核实；未重复写入。");
    let run = crate::generation::get(engine, run_id).map_err(|_| unknown())?;
    if run.session_id != session_id {
        return Err(unknown());
    }
    // Workspace tools recover durable evidence without repeating effects.
    // Reads additionally revalidate current permission and the frozen version.
    if crate::authorized_tools::required_rank(&call.name).is_some() {
        return crate::authorized_tools::recover(engine, session_id, run_id, call);
    }
    if !crate::generation::task_tool_policy(&run.mode, "off")
        .allowed_tools
        .contains(&call.name)
    {
        return Err(unknown());
    }
    let scope = crate::learning_tools::ToolScope {
        session_id: session_id.into(),
        section_id: String::new(),
        run_id: run_id.into(),
        allowed_ids: vec![],
    };
    let (request, user) =
        crate::learning_tools::operation_id(engine, &scope, &call.id, &call.name, &call.args)?;
    let mut core = engine.lock()?;
    let session = crate::sessions::get(&mut core.content, session_id)?;
    let source = run.source_document_id.as_deref().unwrap_or(if session.course_id.is_none() { &session.section_id } else { "" });
    if call.name == "create_todo" {
        let todo = engine
            .reminders
            .lookup_todo_request(&request)?
            .ok_or_else(unknown)?;
        if todo.session_id.as_deref() != Some(session_id)
            || call.args["title"].as_str().map(str::trim) != Some(todo.title.as_str())
        {
            return Err(unknown());
        }
        return Ok(
            json!({"persisted":true,"todoId":todo.id,"sessionId":session_id,"sectionId":source,"todo":todo,"recovered":true}),
        );
    }
    if matches!(
        call.name.as_str(),
        "query_related_knowledge" | "list_learning_skills" | "load_learning_skill"
    ) {
        // Read-only calls have no durable business object to recover.
        return Err(unknown());
    }
    let id = format!("entry_{}", &hash(&request)[..32]);
    let entry = session
        .entries
        .iter()
        .find(|e| {
            e.id == id
                && e.data.as_ref().is_some_and(|d| match &user {
                    Some(user) => {
                        d["businessUserEntryId"].as_str() == Some(user.as_str())
                            && d["toolArgumentsHash"].as_str()
                                == Some(hash(&serde_json::to_string(&call.args).unwrap()).as_str())
                    }
                    None => d["runId"].as_str() == Some(run_id),
                })
        })
        .ok_or_else(unknown)?;
    if call.name == "generate_exercises" {
        if entry.kind != "exercise" {
            return Err(unknown());
        }
        let doc_id = format!("exdoc_{}", &hash(&request)[..32]);
        let doc = core.content.read(&doc_id)?;
        if doc.metadata.kind != "exercise"
            || doc
                .metadata
                .extra
                .get("originSessionId")
                .and_then(Value::as_str)
                != Some(session_id)
        {
            return Err(unknown());
        }
        let origin = crate::exercises::origin(&core.content, &doc).map_err(|_| unknown())?;
        return Ok(
            json!({"persisted":true,"documentId":doc.id,"exerciseEntryId":id,"sessionId":session_id,"sectionId":origin.source_document_id,"recovered":true}),
        );
    }
    if ![
        "record_question",
        "create_note",
        "create_error",
        "create_review_card",
    ]
    .contains(&call.name.as_str())
    {
        return Err(unknown());
    }
    let doc = core.content.read(&id)?;
    let source = entry.data.as_ref().and_then(|d| d.get("sourceDocumentId")).and_then(Value::as_str).unwrap_or(source);
    Ok(
        json!({"persisted":true,"entryId":id,"sessionId":session_id,"sectionId":source,"path":doc.path,"needsConfirmation":true,"kind":doc.metadata.extra.get("entryKind"),"recovered":true}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::Engine,
        generation::{ManagedRun, Run},
        model::ToolCall,
    };
    use std::sync::{atomic::AtomicBool, Arc};
    fn setup() -> (tempfile::TempDir, Arc<Engine>, Session, String) {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::new(dir.path().join("lib"), dir.path().join("state")).unwrap();
        let (s, md) = {
            let mut c = e.lock().unwrap();
            let d = c
                .create_section("Native fixture", "Public material", vec![], vec![])
                .unwrap();
            let (s, _) = c.start_session(&d.id, false).unwrap();
            let md = c.content.read(&s.id).unwrap().content;
            (s, md)
        };
        (dir, e, s, md)
    }
    #[test]
    fn mapping_backups_are_idempotent_and_missing_binding_never_recreates() {
        let (_dir, e, s, md) = setup();
        let state = e.lock().unwrap().state_dir.clone();
        let n = prepare(
            &state,
            &s,
            "run_first",
            "user_first",
            vec![s.section_id.clone()],
            false,
            vec![],
            &md,
        )
        .unwrap();
        let backup = fs::read(n.directory.join("legacy-backup.md")).unwrap();
        let n2 = prepare(
            &state,
            &s,
            "run_next",
            "user_next",
            vec![],
            false,
            vec![],
            "changed",
        )
        .unwrap();
        assert_eq!(n.directory, n2.directory);
        assert_eq!(
            fs::read(n.directory.join("legacy-backup.md")).unwrap(),
            backup
        );
        fs::remove_file(n.directory.join("binding.json")).unwrap();
        assert_eq!(
            prepare(
                &state,
                &s,
                "run_third",
                "user_third",
                vec![],
                false,
                vec![],
                "changed"
            )
            .err()
            .unwrap()
            .code,
            "NATIVE_SESSION_INVALID"
        );
        assert_eq!(
            fs::read(n.directory.join("legacy-backup.md")).unwrap(),
            backup
        );
    }
    #[test]
    fn symlink_binding_and_missing_initialized_file_are_errors() {
        let (dir, e, s, md) = setup();
        let state = e.lock().unwrap().state_dir.clone();
        let n = prepare(
            &state,
            &s,
            "run_first",
            "user_first",
            vec![],
            false,
            vec![],
            &md,
        )
        .unwrap();
        atomic(&n.directory.join("binding.json"),&json!({"version":1,"learningSessionId":s.id,"transcript":"transcript.jsonl","initialized":true})).unwrap();
        assert!(prepare(
            &state,
            &s,
            "run_next",
            "user_next",
            vec![],
            false,
            vec![],
            &md
        )
        .is_err());
        #[cfg(unix)]
        {
            fs::remove_file(n.directory.join("binding.json")).unwrap();
            std::os::unix::fs::symlink(
                dir.path().join("missing"),
                n.directory.join("binding.json"),
            )
            .unwrap();
            assert!(prepare(
                &state,
                &s,
                "run_next",
                "user_next",
                vec![],
                false,
                vec![],
                &md
            )
            .is_err());
        }
    }
    #[test]
    fn recovery_reads_real_business_receipt_and_never_creates_missing_or_foreign_todo() {
        let (_dir, e, s, _md) = setup();
        let run:Run=serde_json::from_value(json!({"id":"run_fixture","sessionId":s.id,"mode":"question","status":"interrupted","content":"","error":null,"createdAt":timestamp(),"updatedAt":timestamp(),"requestDigest":"fixture","contextManifest":[],"contextSnapshotVersion":1})).unwrap();
        e.runs.lock().unwrap().insert(
            run.id.clone(),
            ManagedRun {
                run: run.clone(),
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        let call = ToolCall {
            id: "call_original".into(),
            name: "create_todo".into(),
            args: json!({"title":"Original only"}),
        };
        let scope = crate::learning_tools::ToolScope {
            session_id: s.id.clone(),
            section_id: s.section_id.clone(),
            run_id: run.id.clone(),
            allowed_ids: vec![],
        };
        let first =
            crate::learning_tools::execute(&e, &scope, &call.id, &call.name, call.args.clone())
                .unwrap();
        let recovered = recover(&e, &s.id, &run.id, &call).unwrap();
        assert_eq!(recovered["todoId"], first["todoId"]);
        assert_eq!(e.reminders.list_todos().unwrap().len(), 1);
        assert!(recover(&e, "foreign", &run.id, &call).is_err());
        let absent = ToolCall {
            id: "missing".into(),
            ..call
        };
        assert_eq!(
            recover(&e, &s.id, &run.id, &absent).unwrap_err().code,
            "NATIVE_TOOL_UNKNOWN"
        );
        assert_eq!(e.reminders.list_todos().unwrap().len(), 1);
    }

    #[test]
    fn new_run_and_tool_ids_for_one_saved_question_do_not_repeat_business_writes() {
        let (_dir, e, s, _md) = setup();
        let user = {
            let mut c = e.lock().unwrap();
            crate::sessions::append(
                &mut c.content,
                &s.id,
                Entry {
                    id: String::new(),
                    kind: "user".into(),
                    content: "Save these learning objects".into(),
                    created_at: String::new(),
                    selection: None,
                    related_ids: vec![],
                    data: None,
                },
                None,
                "saved_question_fixture",
            )
            .unwrap()
            .entries
            .last()
            .unwrap()
            .id
            .clone()
        };
        let mut scopes = vec![];
        for id in ["run_before_restart", "run_after_restart"] {
            let run: Run = serde_json::from_value(json!({"id":id,"sessionId":s.id,"userEntryId":user,
                "mode":"explain","status":"running","content":"","error":null,"createdAt":timestamp(),
                "updatedAt":timestamp(),"requestDigest":"dedup-fixture","contextManifest":[],"contextSnapshotVersion":1})).unwrap();
            e.runs.lock().unwrap().insert(
                id.into(),
                ManagedRun {
                    run,
                    cancel: Arc::new(AtomicBool::new(false)),
                },
            );
            scopes.push(crate::learning_tools::ToolScope {
                session_id: s.id.clone(),
                section_id: s.section_id.clone(),
                run_id: id.into(),
                allowed_ids: vec![],
            });
        }
        for (name, args, key) in [
            ("create_note", json!({"content":"Only one note"}), "entryId"),
            (
                "record_question",
                json!({"question":"Only one question"}),
                "entryId",
            ),
            (
                "create_error",
                json!({"front":"Only one error","back":"Reference"}),
                "entryId",
            ),
            (
                "create_review_card",
                json!({"front":"Only one card","back":"Reference"}),
                "entryId",
            ),
            ("create_todo", json!({"title":"Only one todo"}), "todoId"),
            (
                "generate_exercises",
                json!({"requirements":"One question","summary":"Fixture","exercises":[{
                    "question":"2 + 2?","format":"exact","answers":["4"],"explanation":"Addition","tags":[],"sourceIds":[]
                }]}),
                "documentId",
            ),
        ] {
            let first =
                crate::learning_tools::execute(&e, &scopes[0], "first_call", name, args.clone())
                    .unwrap();
            let second = crate::learning_tools::execute(
                &e,
                &scopes[1],
                "different_call",
                name,
                args.clone(),
            )
            .unwrap();
            assert_eq!(first[key], second[key], "{name}");
            let recovered = recover(
                &e,
                &s.id,
                &scopes[1].run_id,
                &ToolCall {
                    id: "another_call".into(),
                    name: name.into(),
                    args,
                },
            )
            .unwrap();
            assert_eq!(recovered[key], first[key], "{name}");
        }
        let mut c = e.lock().unwrap();
        let saved = crate::sessions::get(&mut c.content, &s.id).unwrap();
        assert_eq!(saved.entries.len(), 6); // one user, four learning entries, one exercise
        assert_eq!(e.reminders.list_todos().unwrap().len(), 1);
        assert_eq!(
            saved
                .entries
                .iter()
                .filter(|e| e.kind == "exercise")
                .count(),
            1
        );
    }
}
