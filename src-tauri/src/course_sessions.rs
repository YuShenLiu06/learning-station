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
    if document.is_empty() && session.section_id.is_empty() {
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

pub fn open(core: &mut Core, document: &str, owner: &str) -> Result<(Session, Context)> {
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
