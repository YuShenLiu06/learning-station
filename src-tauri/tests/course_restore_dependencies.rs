use learning_station_core::{app::{Core,Engine}, exercises::{self,ManualExercise}, library_tree, sessions, trash, types::*};
fn node(c:&mut Core,kind:&str,parent:Option<&str>,key:&str)->Document {library_tree::create(&mut c.content,kind,key,parent,key,key).unwrap()}
fn ex(c:&mut Core,s:&str,d:&str,key:&str)->exercises::ExerciseDocument {exercises::create_manual(&mut c.content,s,d,ManualExercise{question:"Synthetic question?".into(),format:"exact".into(),answers:vec!["right".into()],explanation:"Synthetic explanation".into()},key).unwrap()}

#[test]
fn restoring_moved_course_must_not_report_success_with_hidden_origin_ledger() {
    let t=tempfile::tempdir().unwrap();let e=Engine::new(t.path().join("library"),t.path().join("state")).unwrap();let mut c=e.lock().unwrap();
    let a=node(&mut c,"course",None,"restore-ledger-owner-a");let b=node(&mut c,"course",None,"restore-ledger-owner-b");
    let n=node(&mut c,"section",Some(&a.id),"restore-ledger-note");let s=c.start_session(&n.id,false).unwrap().0;
    let x=ex(&mut c,&s.id,&n.id,"restore-ledger-exercise");
    let n=c.content.read(&n.id).unwrap();library_tree::move_node(&mut c.content,&n.id,Some(&b.id),0,&n.version).unwrap();
    c.start_session(&x.document.id,false).unwrap();
    assert_eq!(trash::preview(&mut c.content,&a.id).unwrap_err().code,"SOURCE_IN_USE");
    let p=trash::preview(&mut c.content,&b.id).unwrap();let tb=trash::trash(&mut c.content,&b.id,&p.version,"restore-ledger-trash-b").unwrap();
    let p=trash::preview(&mut c.content,&a.id).unwrap();let ta=trash::trash(&mut c.content,&a.id,&p.version,"restore-ledger-trash-a").unwrap();
    let restored=trash::restore(&mut c.content,&tb.trash_id,"restore-ledger-only-b");
    let readable=exercises::get(&mut c.content,&x.document.id);
    println!("restore target B={:?}; original exercise readable={:?}",restored.as_ref().map(|r|&r.status),readable.as_ref().map(|r|&r.session.id));
    let safe=restored.is_err() || readable.is_ok();
    if restored.is_ok() && readable.is_err() {
        trash::restore(&mut c.content,&ta.trash_id,"restore-ledger-fix-owner-a").unwrap();
        assert_eq!(exercises::get(&mut c.content,&x.document.id).unwrap().session.id,s.id);
        println!("restoring original course A makes the same exercise/ledger IDs readable again; no byte loss observed");
    }
    assert!(safe,"restored course must remain blocked until its original exercise ledger is restored, or restore the dependency safely");
}


fn recycle(c: &mut Core, id: &str, request: &str) -> trash::Receipt {
    let preview = trash::preview(&mut c.content, id).unwrap();
    trash::trash(&mut c.content, id, &preview.version, request).unwrap()
}

#[test]
fn course_and_material_restore_reject_atomically_then_reuse_the_original_request() {
    for whole_course in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let engine = Engine::new(temp.path().join("library"), temp.path().join("state")).unwrap();
        let mut c = engine.lock().unwrap();
        let a = node(&mut c, "course", None, "dependency-origin-course");
        let b = node(&mut c, "course", None, "dependency-target-course");
        let n = node(&mut c, "section", Some(&a.id), "dependency-source-note");
        let origin = c.start_session(&n.id, false).unwrap().0;
        let x = ex(&mut c, &origin.id, &n.id, "dependency-source-exercise");
        let question = x.exercises["exercises"][0]["id"].as_str().unwrap();
        sessions::append(&mut c.content, &origin.id, Entry {
            id: "".into(), kind: "attempt".into(), content: "wrong".into(), created_at: "".into(),
            selection: None, related_ids: vec![], data: Some(serde_json::json!({"exerciseId":question,"answer":"wrong"})),
        }, None, "dependency-saved-attempt").unwrap();
        let fresh = c.content.read(&n.id).unwrap();
        library_tree::move_node(&mut c.content, &n.id, Some(&b.id), 0, &fresh.version).unwrap();
        let target = c.start_session(&x.document.id, false).unwrap().0;
        let saved = sessions::get(&mut c.content, &origin.id).unwrap();
        assert!(saved.entries.iter().any(|entry| entry.kind == "feedback"));
        let mappings: Vec<_> = [&saved, &target].into_iter().map(|session| {
            let mapping = learning_station_core::native_session::prepare(
                &c.state_dir, session, "dependency_native_run", "dependency_native_user", vec![], false, vec![], "",
            ).unwrap();
            let bytes = std::fs::read(mapping.directory.join("binding.json")).unwrap();
            (session.id.clone(), mapping.directory, bytes)
        }).collect();
        let originals: Vec<_> = [a.id.as_str(), b.id.as_str(), n.id.as_str(), x.document.id.as_str(), origin.id.as_str(), target.id.as_str()]
            .into_iter().chain(saved.entries.iter().map(|entry| entry.id.as_str())).map(|id| {
                let document = c.content.read(id).unwrap();
                (document.path, document.content.into_bytes())
            }).collect();
        let target_id = if whole_course { &b.id } else { &n.id };
        let target_receipt = recycle(&mut c, target_id, "dependency-trash-target");
        let origin_receipt = recycle(&mut c, &a.id, "dependency-trash-origin");
        let ledger_path = c.state_dir.join("library-trash.json");
        let ledger = std::fs::read(&ledger_path).unwrap();
        for _ in 0..2 {
            let error = trash::restore(&mut c.content, &target_receipt.trash_id, "dependency-restore-target").unwrap_err();
            assert_eq!(error.code, "DEPENDENCY_TRASHED");
            let details = error.details.unwrap();
            assert_eq!(details["originSessionId"], origin.id);
            assert_eq!(details["restoreTargetId"], a.id);
            assert_eq!(details["blockingTrashId"], origin_receipt.trash_id);
            assert_eq!(std::fs::read(&ledger_path).unwrap(), ledger);
            assert!(c.content.is_trashed(target_id));
            assert!(c.content.is_trashed(&origin.id));
            assert!(exercises::get(&mut c.content, &x.document.id).is_err());
        }
        // The same failure and recovery semantics survive an application restart.
        drop(c); drop(engine);
        let engine = Engine::new(temp.path().join("library"), temp.path().join("state")).unwrap();
        let mut c = engine.lock().unwrap();
        c.refresh().unwrap(); // Normal startup scan; Engine::new intentionally starts with an empty index.
        assert_eq!(trash::restore(&mut c.content, &target_receipt.trash_id, "dependency-restore-target").unwrap_err().code, "DEPENDENCY_TRASHED");
        assert_eq!(std::fs::read(&ledger_path).unwrap(), ledger);
        trash::restore(&mut c.content, &origin_receipt.trash_id, "dependency-restore-origin").unwrap();
        assert!(c.content.is_trashed(target_id));
        let restored = trash::restore(&mut c.content, &target_receipt.trash_id, "dependency-restore-target").unwrap();
        assert_eq!(restored.status, "restored");
        assert_eq!(trash::restore(&mut c.content, &target_receipt.trash_id, "dependency-restore-target").unwrap(), restored);
        assert_eq!(trash::restore(&mut c.content, &target_receipt.trash_id, "dependency-restore-again").unwrap(), restored);
        assert_eq!(exercises::get(&mut c.content, &x.document.id).unwrap().session.id, origin.id);
        assert_eq!(serde_json::to_value(sessions::get(&mut c.content, &origin.id).unwrap().entries).unwrap(), serde_json::to_value(&saved.entries).unwrap());
        assert_eq!(c.start_session(&x.document.id, false).unwrap().0.id, target.id);
        for (path, bytes) in originals {
            assert_eq!(std::fs::read(c.content.root().join(path)).unwrap(), bytes);
        }
        for (id, directory, bytes) in mappings {
            let session = sessions::get(&mut c.content, &id).unwrap();
            let mapping = learning_station_core::native_session::prepare(
                &c.state_dir, &session, "dependency_native_after", "dependency_native_user_after", vec![], false, vec![], "",
            ).unwrap();
            assert_eq!(mapping.directory, directory);
            assert_eq!(std::fs::read(directory.join("binding.json")).unwrap(), bytes);
        }
    }
}

#[test]
fn all_external_origin_ledgers_must_be_restored_before_target_becomes_visible() {
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::new(temp.path().join("library"), temp.path().join("state")).unwrap();
    let mut c = engine.lock().unwrap();
    let target = node(&mut c, "course", None, "many-target-course");
    let mut origins = vec![];
    for index in 0..2 {
        let owner = node(&mut c, "course", None, &format!("many-origin-course-{index}"));
        let source = node(&mut c, "section", Some(&owner.id), &format!("many-origin-note-{index}"));
        let session = c.start_session(&source.id, false).unwrap().0;
        let exercise = ex(&mut c, &session.id, &source.id, &format!("many-origin-exercise-{index}"));
        let source = c.content.read(&source.id).unwrap();
        library_tree::move_node(&mut c.content, &source.id, Some(&target.id), index, &source.version).unwrap();
        c.start_session(&exercise.document.id, false).unwrap();
        origins.push((owner, session, exercise));
    }
    let target_receipt = recycle(&mut c, &target.id, "many-trash-target");
    let receipts: Vec<_> = origins.iter().enumerate().map(|(i, (owner, _, _))| recycle(&mut c, &owner.id, &format!("many-trash-origin-{i}"))).collect();
    for (i, receipt) in receipts.iter().enumerate() {
        let error = trash::restore(&mut c.content, &target_receipt.trash_id, "many-restore-target").unwrap_err();
        assert_eq!(error.code, "DEPENDENCY_TRASHED");
        assert!(c.content.is_trashed(&target.id));
        trash::restore(&mut c.content, &receipt.trash_id, &format!("many-restore-origin-{i}")).unwrap();
    }
    trash::restore(&mut c.content, &target_receipt.trash_id, "many-restore-target").unwrap();
    for (_, session, exercise) in origins {
        assert_eq!(exercises::get(&mut c.content, &exercise.document.id).unwrap().session.id, session.id);
    }
}

#[test]
fn restoring_a_course_with_its_own_exercise_ledger_needs_no_other_restore() {
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::new(temp.path().join("library"), temp.path().join("state")).unwrap();
    let mut c = engine.lock().unwrap();
    let owner = node(&mut c, "course", None, "local-ledger-course");
    let note = node(&mut c, "section", Some(&owner.id), "local-ledger-source");
    let session = c.start_session(&note.id, false).unwrap().0;
    let exercise = ex(&mut c, &session.id, &note.id, "local-ledger-exercise");
    let receipt = recycle(&mut c, &owner.id, "local-ledger-trash");
    let restored = trash::restore(&mut c.content, &receipt.trash_id, "local-ledger-restore").unwrap();
    assert_eq!(restored.status, "restored");
    assert_eq!(exercises::get(&mut c.content, &exercise.document.id).unwrap().session.id, session.id);
}

#[test]
fn an_entry_hidden_in_a_separate_exercise_record_is_also_a_restore_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::new(temp.path().join("library"), temp.path().join("state")).unwrap();
    let mut c = engine.lock().unwrap();
    let owner = node(&mut c, "course", None, "entry-dependency-course");
    let source = node(&mut c, "section", Some(&owner.id), "entry-dependency-source");
    let other = node(&mut c, "section", Some(&owner.id), "entry-dependency-other");
    let session = c.start_session(&source.id, false).unwrap().0;
    let exercise = ex(&mut c, &session.id, &source.id, "entry-dependency-exercise");
    let entry = exercise.document.metadata.extra["exerciseEntryId"].as_str().unwrap();
    let view = exercises::materialize_legacy(&mut c.content, &session.id, entry).unwrap();
    library_tree::move_node(&mut c.content, &view.id, Some(&other.id), 0, &view.version).unwrap();
    let entry_receipt = recycle(&mut c, &view.id, "entry-dependency-trash-view");
    let source_receipt = recycle(&mut c, &source.id, "entry-dependency-trash-source");
    assert!(!c.content.is_trashed(&session.id));
    let error = trash::restore(&mut c.content, &source_receipt.trash_id, "entry-dependency-restore-source").unwrap_err();
    assert_eq!(error.code, "DEPENDENCY_TRASHED");
    let details = error.details.unwrap();
    assert_eq!(details["dependencyId"], entry);
    assert_eq!(details["blockingTrashId"], entry_receipt.trash_id);
    assert!(c.content.is_trashed(&source.id));
    trash::restore(&mut c.content, &entry_receipt.trash_id, "entry-dependency-restore-view").unwrap();
    trash::restore(&mut c.content, &source_receipt.trash_id, "entry-dependency-restore-source").unwrap();
    for document in [&exercise.document.id, &view.id] {
        assert_eq!(exercises::get(&mut c.content, document).unwrap().session.id, session.id);
    }
}
