// Included in authorized_tools::tests to reuse the real authorized course fixture.
fn partial_fixture() -> (tempfile::TempDir, Arc<Engine>, ToolScope, String, Value) {
    let (dir, engine, scope, id, _) = course_fixture("write");
    let before = engine.lock().unwrap().content.read(&id).unwrap();
    let body = "# 原文😀\r\n\r\n第一段\r\n\r\n不变的中间段\r\n\r\n最后一段\r\n";
    let saved = engine
        .lock()
        .unwrap()
        .content
        .save(
            &id,
            &crate::content::serialize(&before.metadata, body).unwrap(),
            &before.version,
        )
        .unwrap();
    let pending = super::execute(&engine,&scope,"partial","write",json!({"path":format!("documents/{id}.md"),"content":body.replace("第一段","修改第一段").replace("最后一段","修改最后一段"),"expectedVersion":saved.version})).unwrap();
    (dir, engine, scope, id, pending)
}
fn partial_get(engine: &Engine, p: &Value) -> Value {
    super::proposal_get(
        engine,
        p["courseId"].as_str().unwrap(),
        p["proposalId"].as_str().unwrap(),
    )
    .unwrap()
}
fn partial_decide(engine: &Engine, p: &Value, h: &str, accept: bool) -> Result<Value, CoreError> {
    super::text_proposal_decide(
        engine,
        p["courseId"].as_str().unwrap(),
        p["proposalId"].as_str().unwrap(),
        h,
        accept,
    )
}
#[test]
fn partial_review_accept_one_restart_reject_other_preserves_exact_text_and_receipts() {
    let (dir, engine, scope, id, p) = partial_fixture();
    let preview = partial_get(&engine, &p);
    let hunks = preview["textReview"]["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 2);
    assert_eq!(hunks[0]["before"], "第一段\r\n\r\n");
    let accepted = partial_decide(&engine, &p, "h0", true).unwrap();
    assert_eq!(accepted["persisted"], true);
    let saved = engine.lock().unwrap().content.read(&id).unwrap();
    assert_eq!(
        saved.body,
        preview["original"]
            .as_str()
            .unwrap()
            .replace("第一段", "修改第一段")
    );
    let reopened = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
    assert_eq!(
        partial_get(&reopened, &p)["textReview"]["hunks"][0]["status"],
        "accepted"
    );
    assert_eq!(partial_decide(&reopened, &p, "h0", true).unwrap(), accepted);
    assert_eq!(
        partial_decide(&reopened, &p, "h1", false).unwrap()["complete"],
        true
    );
    assert_eq!(
        reopened.lock().unwrap().content.read(&id).unwrap().content,
        saved.content
    );
    assert_eq!(
        partial_decide(&reopened, &p, "h0", false).unwrap(),
        accepted,
        "opposite retry cannot undo a saved decision"
    );
    let final_receipt = super::proposal_result(&reopened, p.clone()).unwrap();
    assert_eq!(final_receipt["acceptedCount"], 1);
    assert_eq!(final_receipt["rejectedCount"], 1);
    let replay=super::execute(&reopened,&scope,"partial","write",json!({"path":preview["path"],"content":preview["content"],"expectedVersion":preview["expectedVersion"]})).unwrap();
    let mut replay = replay;
    assert_eq!(
        replay.as_object_mut().unwrap().remove("replayed"),
        Some(json!(true))
    );
    assert_eq!(replay, final_receipt);
}
#[test]
fn partial_review_conflicts_revocation_and_pure_rejection_never_write() {
    for cause in ["external", "revoked", "draft"] {
        let (_dir, engine, _, id, p) = partial_fixture();
        partial_get(&engine, &p);
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let expected = match cause {
            "external" => {
                engine
                    .lock()
                    .unwrap()
                    .content
                    .save(
                        &id,
                        &crate::content::serialize(&before.metadata, "外部改稿").unwrap(),
                        &before.version,
                    )
                    .unwrap();
                "CONFLICT"
            }
            "revoked" => {
                engine
                    .save_ui_preferences(UiPreferences {
                        tool_authorization_tier: "read".into(),
                        ..UiPreferences::default()
                    })
                    .unwrap();
                "TOOL_NOT_ALLOWED"
            }
            _ => {
                engine
                    .drafts
                    .save(
                        &format!("section:{id}"),
                        crate::drafts::DraftData {
                            body: "未保存草稿".into(),
                            ..Default::default()
                        },
                        None,
                    )
                    .unwrap();
                "CONFLICT"
            }
        };
        assert_eq!(
            partial_decide(&engine, &p, "h0", true).unwrap_err().code,
            expected
        );
        let current = engine.lock().unwrap().content.read(&id).unwrap();
        assert_eq!(
            partial_get(&engine, &p)["textReview"]["hunks"][0]["status"],
            "pending"
        );
        assert_eq!(
            partial_decide(&engine, &p, "h0", false).unwrap()["persisted"],
            false
        );
        assert_eq!(
            partial_decide(&engine, &p, "h1", false).unwrap()["persisted"],
            false
        );
        assert_eq!(
            engine.lock().unwrap().content.read(&id).unwrap().content,
            current.content
        );
    }
}
#[test]
fn partial_review_unknown_only_inspects_original_operation_and_never_replays() {
    let (dir, engine, scope, id, p) = partial_fixture();
    partial_get(&engine, &p);
    let accepted = partial_decide(&engine, &p, "h0", true).unwrap();
    let course = p["courseId"].as_str().unwrap();
    let proposal = p["proposalId"].as_str().unwrap();
    let path = dir
        .path()
        .join(format!("state/edit-proposals/{course}/{proposal}.json"));
    let mut stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored["textReview"]["hunks"][0]["status"] = json!("accepting");
    stored["textReview"]["version"] = stored["expectedVersion"].clone();
    crate::course_write_transaction::durable_json(&path, &stored).unwrap();
    let reopened = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
    partial_get(&reopened, &p);
    let before = reopened.lock().unwrap().content.read(&id).unwrap();
    assert_eq!(
        partial_decide(&reopened, &p, "h1", false).unwrap_err().code,
        "NATIVE_TOOL_UNKNOWN"
    );
    assert_eq!(partial_decide(&reopened, &p, "h0", true).unwrap(), accepted);
    assert_eq!(
        reopened.lock().unwrap().content.read(&id).unwrap().content,
        before.content
    );
    // Simulate publication checkpoint saved but completion ledger interrupted.
    crate::course_write_transaction::durable_json(&path, &stored).unwrap();
    let operation = stored["textReview"]["hunks"][0]["operation"]
        .as_str()
        .unwrap();
    let ledger = dir.path().join(format!(
        "state/workspaces/{}/receipts/{operation}.json",
        scope.session_id
    ));
    std::fs::remove_file(&ledger).unwrap();
    assert_eq!(
        partial_decide(&reopened, &p, "h0", true).unwrap()["persisted"],
        true
    );
    assert_eq!(
        reopened.lock().unwrap().content.read(&id).unwrap().content,
        before.content
    );
    // Missing receipt plus a third-party edit cannot be called a completed save.
    crate::course_write_transaction::durable_json(&path, &stored).unwrap();
    std::fs::remove_file(&ledger).unwrap();
    reopened
        .lock()
        .unwrap()
        .content
        .save(
            &id,
            &crate::content::serialize(&before.metadata, "第三方内容").unwrap(),
            &before.version,
        )
        .unwrap();
    assert_eq!(
        partial_decide(&reopened, &p, "h0", true).unwrap_err().code,
        "NATIVE_TOOL_UNKNOWN"
    );
    assert_eq!(
        reopened.lock().unwrap().content.read(&id).unwrap().body,
        "第三方内容"
    );
}
#[test]
fn partial_review_legacy_pending_migrates_but_unknown_does_not() {
    let (dir, engine, scope, _, p) = partial_fixture();
    let course = p["courseId"].as_str().unwrap();
    let id = p["proposalId"].as_str().unwrap();
    let path = dir
        .path()
        .join(format!("state/edit-proposals/{course}/{id}.json"));
    let mut stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored["status"] = json!("unknown");
    crate::course_write_transaction::durable_json(&path, &stored).unwrap();
    assert!(partial_get(&engine, &p)["textReview"].is_null());
    assert!(partial_decide(&engine, &p, "h0", true).is_err());
    stored["status"] = json!("pending");
    crate::course_write_transaction::durable_json(&path, &stored).unwrap();
    assert_eq!(
        partial_get(&engine, &p)["textReview"]["hunks"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // A pre-intent interruption is reopened explicitly, never auto-published.
    stored = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored["textReview"]["hunks"][0]["status"] = json!("accepting");
    stored["textReview"]["hunks"][0]["operation"] = json!("textedit_not_started");
    crate::course_write_transaction::durable_json(&path, &stored).unwrap();
    assert_eq!(
        partial_decide(&engine, &p, "h0", true).unwrap_err().code,
        "NOT_PUBLISHED"
    );
    assert_eq!(
        partial_get(&engine, &p)["textReview"]["hunks"][0]["status"],
        "pending"
    );
    assert!(!dir
        .path()
        .join(format!(
            "state/workspaces/{}/receipts/textedit_not_started.pending",
            scope.session_id
        ))
        .exists());
}
#[test]
#[ignore = "creates an isolated native QA fixture"]
fn create_partial_edit_review_gui_fixture() {
    let (dir, engine, _, id, p) = partial_fixture();
    partial_get(&engine, &p);
    let root = dir.keep();
    let doc = engine.lock().unwrap().content.read(&id).unwrap();
    crate::app::atomic_json(std::path::Path::new("/tmp/partial-edit-review-fixture.json"),&json!({"library":root.join("library"),"state":root.join("state"),"documentId":id,"documentPath":root.join("library").join(doc.path),"proposal":p})).unwrap();
}
