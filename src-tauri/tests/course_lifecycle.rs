use learning_station_core::{app::{Core, Engine}, content, course_sessions, exercise_references, exercises::{self, ManualExercise}, library_tree, review, sessions, trash, types::*};
use serde_json::json;

fn node(c: &mut Core, kind: &str, parent: Option<&str>, key: &str) -> Document {
    library_tree::create(&mut c.content, kind, key, parent, key, &format!("review-probe-{key}")).unwrap()
}
fn ex(c: &mut Core, sid: &str, source: &str, key: &str) -> exercises::ExerciseDocument {
    exercises::create_manual(&mut c.content, sid, source, ManualExercise {
        question: "Synthetic question".into(), format: "exact".into(), answers: vec!["right".into()], explanation: "Synthetic explanation".into()
    }, key).unwrap()
}
fn setup() -> (tempfile::TempDir, std::sync::Arc<Engine>) {
    let t = tempfile::tempdir().unwrap();
    let e = Engine::new(t.path().join("library"), t.path().join("state")).unwrap(); (t,e)
}

#[test]
fn deleting_first_note_should_keep_other_note_course_chat() {
    let (_t,e)=setup(); let mut c=e.lock().unwrap();
    let owner=node(&mut c,"course",None,"trash-owner");
    let a=node(&mut c,"section",Some(&owner.id),"trash-note-a");
    let b=node(&mut c,"section",Some(&owner.id),"trash-note-b");
    let sid=c.start_session(&a.id,false).unwrap().0.id;
    assert_eq!(c.start_session(&b.id,false).unwrap().0.id,sid);
    let p=trash::preview(&mut c.content,&a.id).unwrap();
    println!("first-note trash preview: session_count={}",p.session_count);
    trash::trash(&mut c.content,&a.id,&p.version,"delete-first-note").unwrap();
    let result=c.start_session(&b.id,false);
    println!("open surviving note after first-note delete: {:?}",result.as_ref().map(|s|&s.0.id));
    assert!(result.is_ok(),"surviving course note must retain shared chat");
}

#[test]
fn review_of_second_note_should_preserve_second_note_source() {
    let (_t,e)=setup(); let mut c=e.lock().unwrap();
    let owner=node(&mut c,"course",None,"review-owner");
    let a=node(&mut c,"section",Some(&owner.id),"review-note-a");
    let b=node(&mut c,"section",Some(&owner.id),"review-note-b");
    let sid=c.start_session(&a.id,false).unwrap().0.id;
    c.start_session(&b.id,false).unwrap();
    let item=ex(&mut c,&sid,&b.id,"review-exercise-b");
    let qid=item.exercises["exercises"][0]["id"].as_str().unwrap();
    sessions::append(&mut c.content,&sid,Entry { id:"".into(),kind:"attempt".into(),content:"wrong".into(),created_at:"".into(),selection:None,related_ids:vec![],data:Some(json!({"exerciseId":qid,"answer":"wrong"})) },None,"review-wrong-answer").unwrap();
    let items=review::all(&mut c.content).unwrap();
    println!("review source={}, actual exercise source={}",items[0]["sourceDocumentId"],b.id);
    assert_eq!(items[0]["sourceDocumentId"],b.id);
}

#[test]
fn moved_note_exercise_reference_should_remain_readable_without_cross_course_history() {
    let (_t,e)=setup(); let mut c=e.lock().unwrap();
    let owner=node(&mut c,"course",None,"move-owner-a");
    let other=node(&mut c,"course",None,"move-owner-b");
    let a=node(&mut c,"section",Some(&owner.id),"move-note-a");
    let sid=c.start_session(&a.id,false).unwrap().0.id;
    let item=ex(&mut c,&sid,&a.id,"move-exercise-a");
    let qid=item.exercises["exercises"][0]["id"].as_str().unwrap();
    let fresh=c.content.read(&a.id).unwrap();
    library_tree::move_node(&mut c.content,&a.id,Some(&other.id),0,&fresh.version).unwrap();
    let new=c.start_session(&item.document.id,false).unwrap().0;
    assert_ne!(new.id,sid);
    assert!(exercises::get(&mut c.content,&item.document.id).is_ok());
    let r=exercise_references::load(&mut c.content,&sid,&item.document.id,qid,"question",None);
    println!("reference from moved valid exercise: {:?}",r.as_ref().map(|p|&p.source_id));
    assert!(r.is_ok(),"valid moved exercise should allow read-only reference");
}

#[test]
fn forged_source_metadata_should_not_override_real_exercise_ledger() {
    let (_t,e)=setup(); let mut c=e.lock().unwrap();
    let owner=node(&mut c,"course",None,"spoof-owner");
    let a=node(&mut c,"section",Some(&owner.id),"spoof-note-a");
    let b=node(&mut c,"section",Some(&owner.id),"spoof-note-b");
    let s=c.start_session(&a.id,false).unwrap().0;
    let item=ex(&mut c,&s.id,&a.id,"spoof-exercise-a");
    let doc=c.content.read(&item.document.id).unwrap();
    let mut m=doc.metadata; m.extra.insert("sourceDocumentId".into(),json!(b.id));
    c.content.save(&doc.id,&content::serialize(&m,&doc.body).unwrap(),&doc.version).unwrap();
    let r=course_sessions::source(&c.content,&s,&doc.id);
    println!("forged page source accepted: {:?}; real ledger source={}",r,item.exercises["sourceDocumentId"]);
    assert!(r.is_err(),"edited page metadata cannot change exercise ledger's source");
}

#[test]
fn concurrent_course_initialization_has_one_stable_id() {
    let (_t,e)=setup();
    let (a,b)={let mut c=e.lock().unwrap();let owner=node(&mut c,"course",None,"concurrent-owner");
        (node(&mut c,"section",Some(&owner.id),"concurrent-note-a").id,node(&mut c,"section",Some(&owner.id),"concurrent-note-b").id)};
    let gate=std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles:Vec<_>=(0..8).map(|i|{let e=e.clone();let gate=gate.clone();let id=if i%2==0{a.clone()}else{b.clone()};
        std::thread::spawn(move||{gate.wait();e.lock().unwrap().start_session(&id,false).unwrap().0.id})}).collect();
    let ids:std::collections::BTreeSet<_>=handles.into_iter().map(|h|h.join().unwrap()).collect();
    assert_eq!(ids.len(),1);println!("8 concurrent callers: one stable course session");
}

#[test]
fn wrong_course_focus_and_exercise_source_are_rejected_without_answers_in_context() {
    let (_t,e)=setup();let mut c=e.lock().unwrap();
    let owner=node(&mut c,"course",None,"reject-owner-a");let foreign=node(&mut c,"course",None,"reject-owner-b");
    let a=node(&mut c,"section",Some(&owner.id),"reject-note-a");let b=node(&mut c,"section",Some(&foreign.id),"reject-note-b");
    let s=c.start_session(&a.id,false).unwrap().0;let item=ex(&mut c,&s.id,&a.id,"reject-exercise-a");
    let ctx=c.session_context_for(&s.id,Some(&item.document.id),None).unwrap().1;
    assert!(!ctx.text.contains("Synthetic explanation"));
    assert!(!ctx.text.contains(&item.document.id));
    assert_eq!(c.session_context_for(&s.id,Some(&b.id),None).unwrap_err().code,"OUT_OF_SCOPE");
    let doc=c.content.read(&item.document.id).unwrap();let mut m=doc.metadata;m.extra.insert("sourceDocumentId".into(),json!(b.id));
    c.content.save(&doc.id,&content::serialize(&m,&doc.body).unwrap(),&doc.version).unwrap();
    assert_eq!(c.session_context_for(&s.id,Some(&doc.id),None).unwrap_err().code,"CONTEXT_CONFLICT");
    println!("wrong-course focus/forged source rejected; exercise answers not injected");
}

#[test]
fn restoring_trashed_anchor_recovers_original_course_id_and_entries() {
    let (_t,e)=setup();let mut c=e.lock().unwrap();let owner=node(&mut c,"course",None,"restore-owner");
    let a=node(&mut c,"section",Some(&owner.id),"restore-note-a");let b=node(&mut c,"section",Some(&owner.id),"restore-note-b");
    let s=c.start_session(&a.id,false).unwrap().0;
    sessions::append(&mut c.content,&s.id,Entry{id:"".into(),kind:"note".into(),content:"RECOVER_SYNTHETIC_ENTRY".into(),created_at:"".into(),selection:None,related_ids:vec![],data:None},None,"recover-synthetic-entry").unwrap();
    let p=trash::preview(&mut c.content,&a.id).unwrap();let receipt=trash::trash(&mut c.content,&a.id,&p.version,"restore-delete-anchor").unwrap();
    assert_eq!(c.start_session(&b.id,false).unwrap().0.id,s.id);
    trash::restore(&mut c.content,&receipt.trash_id,"restore-original-anchor").unwrap();
    let r=c.start_session(&b.id,false).unwrap().0;assert_eq!(r.id,s.id);assert!(r.entries.iter().any(|x|x.content=="RECOVER_SYNTHETIC_ENTRY"));
    println!("restoring anchor recovers shared session and original entry; bytes not destroyed");
}

#[test]
fn trashing_each_note_keeps_chat_and_trashing_course_restores_exact_identity() {
    let (_t,e)=setup(); let mut c=e.lock().unwrap();
    let owner=node(&mut c,"course",None,"lifecycle-owner");
    let a=node(&mut c,"section",Some(&owner.id),"lifecycle-a");
    let b=node(&mut c,"section",Some(&owner.id),"lifecycle-b");
    let s=c.start_session(&a.id,false).unwrap().0;
    let xa=ex(&mut c,&s.id,&a.id,"lifecycle-ex-a");
    let xb=ex(&mut c,&s.id,&b.id,"lifecycle-ex-b");
    for source in [&a,&b] {
        sessions::append(&mut c.content,&s.id,Entry { id:"".into(),kind:"note".into(),content:source.id.clone(),created_at:"".into(),selection:None,related_ids:vec![],data:Some(json!({"reviewCard":true,"front":source.id,"back":"card","sourceDocumentId":source.id})) },None,&format!("card-{}",source.id)).unwrap();
    }
    let before=sessions::get(&mut c.content,&s.id).unwrap();
    let native=learning_station_core::native_session::prepare(&c.state_dir,&before,"lifecycle_run","lifecycle_user",vec![],false,vec![],"").unwrap();
    for (removed,kept) in [(&a,&b),(&b,&a)] {
        let p=trash::preview(&mut c.content,&removed.id).unwrap(); assert_eq!(p.session_count,0);
        let receipt=trash::trash(&mut c.content,&removed.id,&p.version,&format!("trash-{}",removed.id)).unwrap();
        let live=c.start_session(&kept.id,false).unwrap().0;
        assert_eq!(live.id,s.id);
        assert!(learning_station_core::exploration::request_context(&c.content,&live,"Continue",&[]).unwrap().is_none());
        assert!(review::all(&mut c.content).unwrap().iter().all(|r|r["sourceDocumentId"]!=removed.id));
        trash::restore(&mut c.content,&receipt.trash_id,&format!("restore-{}",removed.id)).unwrap();
    }
    let p=trash::preview(&mut c.content,&owner.id).unwrap(); assert_eq!(p.session_count,1);
    let receipt=trash::trash(&mut c.content,&owner.id,&p.version,"trash-whole-course").unwrap();
    assert!(c.start_session(&b.id,false).is_err());
    trash::restore(&mut c.content,&receipt.trash_id,"restore-whole-course").unwrap();
    let resumed=c.start_session(&b.id,false).unwrap().0; assert_eq!(resumed.id,s.id);
    assert_eq!(resumed.entries.len(),before.entries.len());
    assert_eq!(exercises::get(&mut c.content,&xa.document.id).unwrap().session.id,s.id);
    assert_eq!(exercises::get(&mut c.content,&xb.document.id).unwrap().session.id,s.id);
    assert_eq!(learning_station_core::native_session::prepare(&c.state_dir,&resumed,"restored_run","restored_user",vec![],false,vec![],"").unwrap().directory,native.directory);
}

#[test]
fn review_sources_follow_real_questions_and_manual_cards_after_moving_material() {
    let (_t,e)=setup();let mut c=e.lock().unwrap();
    let owner=node(&mut c,"course",None,"cards-owner");let other=node(&mut c,"course",None,"cards-other");
    let a=node(&mut c,"section",Some(&owner.id),"cards-a");let b=node(&mut c,"section",Some(&owner.id),"cards-b");
    let s=c.start_session(&a.id,false).unwrap().0;let x=ex(&mut c,&s.id,&b.id,"cards-ex-b");
    let q=x.exercises["exercises"][0]["id"].as_str().unwrap();
    sessions::append(&mut c.content,&s.id,Entry{id:"".into(),kind:"attempt".into(),content:"wrong".into(),created_at:"".into(),selection:None,related_ids:vec![],data:Some(json!({"exerciseId":q,"answer":"wrong"}))},None,"cards-wrong-attempt").unwrap();
    sessions::append(&mut c.content,&s.id,Entry{id:"".into(),kind:"note".into(),content:"manual B".into(),created_at:"".into(),selection:None,related_ids:vec![],data:Some(json!({"reviewCard":true,"front":"manual B","back":"answer","sourceDocumentId":b.id}))},None,"cards-manual-b").unwrap();
    let fresh=c.content.read(&b.id).unwrap();library_tree::move_node(&mut c.content,&b.id,Some(&other.id),0,&fresh.version).unwrap();
    let cards=review::all(&mut c.content).unwrap();assert_eq!(cards.len(),2);
    assert!(cards.iter().all(|r|r["sourceDocumentId"]==b.id&&r["courseId"]==other.id));
    let part=exercise_references::load(&mut c.content,&s.id,&x.document.id,q,"question",None).unwrap();
    let r=exercise_references::ExerciseReference{locator:None,session_id:s.id.clone(),document_id:x.document.id.clone(),exercise_entry_id:part.exercise_entry_id,question_id:q.into(),part:"question".into(),entry_id:part.entry_id,text_hash:part.text_hash,selection:part.text};
    assert!(exercise_references::validate_references(&mut c.content,&s,&[r.clone()]).is_err());
    let target=c.start_session(&x.document.id,false).unwrap().0;
    assert!(exercise_references::validate_references(&mut c.content,&target,&[r]).is_ok());
    assert!(target.entries.is_empty());
    sessions::append(&mut c.content,&s.id,Entry{id:"".into(),kind:"attempt".into(),content:"right".into(),created_at:"".into(),selection:None,related_ids:vec![],data:Some(json!({"exerciseId":q,"answer":"right"}))},None,"cards-moved-attempt").unwrap();
    assert_eq!(exercises::get(&mut c.content,&x.document.id).unwrap().session.id,s.id);
}

#[test]
fn editable_page_cannot_rebind_source_origin_or_question_entry() {
    let (_t,e)=setup();let mut c=e.lock().unwrap();let owner=node(&mut c,"course",None,"integrity-owner");
    let a=node(&mut c,"section",Some(&owner.id),"integrity-a");let b=node(&mut c,"section",Some(&owner.id),"integrity-b");
    let s=c.start_session(&a.id,false).unwrap().0;let x=ex(&mut c,&s.id,&a.id,"integrity-ex-a");let y=ex(&mut c,&s.id,&b.id,"integrity-ex-b");
    let original=c.content.read(&x.document.id).unwrap();
    for (key,value) in [("sourceDocumentId",json!(b.id)),("originSessionId",json!("another_session")),("exerciseEntryId",y.document.metadata.extra["exerciseEntryId"].clone())] {
        let current=c.content.read(&original.id).unwrap();let mut meta=original.metadata.clone();meta.extra.insert(key.into(),value);
        c.content.save(&original.id,&content::serialize(&meta,&original.body).unwrap(),&current.version).unwrap();
        assert!(exercises::get(&mut c.content,&original.id).is_err());
        assert!(c.session_context_for(&s.id,Some(&original.id),None).is_err());
    }
    let current=c.content.read(&original.id).unwrap();
    let mut rebound=y.document.metadata.clone();rebound.id=original.id.clone();
    c.content.save(&original.id,&content::serialize(&rebound,&original.body).unwrap(),&current.version).unwrap();
    assert_eq!(exercises::get(&mut c.content,&original.id).unwrap_err().code,"CONTEXT_CONFLICT");
    let current=c.content.read(&original.id).unwrap();c.content.save(&original.id,&original.content,&current.version).unwrap();
    assert_eq!(course_sessions::source(&c.content,&s,&original.id).unwrap(),a.id);
}

#[test]
fn exercise_leaf_move_and_recycling_an_in_use_origin_fail_before_mutation() {
    let (_t,e)=setup();let mut c=e.lock().unwrap();
    let owner=node(&mut c,"course",None,"move-guard-owner");let target=node(&mut c,"course",None,"move-guard-target");
    let a=node(&mut c,"section",Some(&owner.id),"move-guard-a");let b=node(&mut c,"section",Some(&target.id),"move-guard-b");
    let s=c.start_session(&a.id,false).unwrap().0;let x=ex(&mut c,&s.id,&a.id,"move-guard-exercise");
    let fresh=c.content.read(&x.document.id).unwrap();
    assert_eq!(library_tree::move_node(&mut c.content,&fresh.id,Some(&b.id),0,&fresh.version).unwrap_err().code,"INVALID_HIERARCHY");
    assert_eq!(c.content.read(&fresh.id).unwrap().version,fresh.version);
    let fresh=c.content.read(&a.id).unwrap();library_tree::move_node(&mut c.content,&a.id,Some(&target.id),0,&fresh.version).unwrap();
    let legacy=exercises::materialize_legacy(&mut c.content,&s.id,x.document.metadata.extra["exerciseEntryId"].as_str().unwrap()).unwrap();
    assert_eq!(exercises::get(&mut c.content,&legacy.id).unwrap().exercises["sourceDocumentId"],a.id);
    assert_eq!(trash::preview(&mut c.content,&owner.id).unwrap_err().code,"SOURCE_IN_USE");
    assert!(sessions::get(&mut c.content,&s.id).is_ok());
    assert!(exercises::get(&mut c.content,&x.document.id).is_ok());
}
