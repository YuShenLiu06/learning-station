use super::*;

#[tokio::test]
async fn course_start_without_focus_keeps_readable_versions_separate_from_explicit_body() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
    let (a, b, first, second, root) = {
        let mut c = engine.lock().unwrap();
        let owner = crate::library_tree::create_project(
            &mut c.content,
            "A",
            "",
            "project-context-course-a",
        )
        .unwrap();
        let other = crate::library_tree::create_project(
            &mut c.content,
            "B",
            "",
            "project-context-course-b",
        )
        .unwrap();
        let a = crate::library_tree::create(
            &mut c.content,
            "section",
            "Note A",
            Some(&owner.id),
            "COURSE_A_BODY_NOT_OPENING_CONTEXT",
            "project-context-note-a",
        )
        .unwrap();
        let b = crate::library_tree::create(
            &mut c.content,
            "section",
            "Note B",
            Some(&other.id),
            "COURSE_B_BODY_PRIVATE",
            "project-context-note-b",
        )
        .unwrap();
        let first =
            crate::course_sessions::create(&mut c, &owner.id, "project-context-chat-one").unwrap();
        let second =
            crate::course_sessions::create(&mut c, &owner.id, "project-context-chat-two").unwrap();
        let root = crate::native_session::project_root(&c.content, &first)
            .unwrap()
            .unwrap();
        assert!(root.join(".git/learning-materials-v2").is_file());
        assert_eq!(
            root,
            crate::course_project::course_project_root(c.content.root(), &owner.id).unwrap()
        );
        let readable = context::course_read_manifest(&c.content, &owner.id).unwrap();
        assert!(readable
            .iter()
            .any(|m| m.id == a.id && m.version == a.version));
        assert!(!readable.iter().any(|m| m.id == b.id));
        (a, b, first, second, root)
    };
    let mut request = GenerationRequest {
        session_id: first.id.clone(),
        document_id: Some(a.id.clone()),
        mode: "explain".into(),
        selection: String::new(),
        prompt: "Ordinary chat with no Focus provider".into(),
        request_id: "project-context-first-request".into(),
        user_entry_id: None,
        injections: vec![],
        assistant_references: vec![],
        exercise_references: vec![],
    };
    // No model is configured: the test cannot touch Keychain or a real endpoint.
    // Source capture and the production native binding happen before that gate.
    assert_eq!(
        start(engine.clone(), request.clone(), Arc::new(|_| {}))
            .await
            .unwrap_err()
            .code,
        "NOT_CONFIGURED"
    );
    {
        let mut c = engine.lock().unwrap();
        let session = sessions::get(&mut c.content, &first.id).unwrap();
        assert!(session.context_manifest.is_empty());
        let one = crate::native_session::prepare(
            &c.state_dir,
            &session,
            "run_one",
            "user_one",
            vec![],
            false,
            vec![],
            "",
        )
        .unwrap();
        let two = crate::native_session::prepare(
            &c.state_dir,
            &second,
            "run_two",
            "user_two",
            vec![],
            false,
            vec![],
            "",
        )
        .unwrap();
        assert_ne!(one.directory, two.directory);
        assert_eq!(
            crate::native_session::project_root(&c.content, &second).unwrap(),
            Some(root)
        );
    }
    request.request_id = "project-context-explicit-request".into();
    request.injections.push(context::ContextInjection {
        document_id: a.id.clone(),
        expected_version: a.version.clone(),
        selection: None,
        locator: None,
    });
    assert_eq!(
        start(engine.clone(), request.clone(), Arc::new(|_| {}))
            .await
            .unwrap_err()
            .code,
        "NOT_CONFIGURED"
    );
    let mut c = engine.lock().unwrap();
    let session = sessions::get(&mut c.content, &first.id).unwrap();
    assert_eq!(session.context_manifest.len(), 1);
    assert_eq!(session.context_manifest[0].id, a.id);
    assert!(session.context_manifest[0].included_chars > 0);
    let (_, mut ctx) = c.session_context_for(&first.id, Some(""), None).unwrap();
    context::inject(&c.content, &mut ctx, &request.injections).unwrap();
    let projection =
        crate::context_bridge::ContextBridge::snapshot(&c.state_dir, &c.content, &session, &ctx)
            .unwrap();
    let frozen = crate::context_bridge::ContextBridge::freeze(
        &request,
        &ctx,
        &session,
        None,
        &projection,
        crate::time_context::TimeContext::capture(),
    )
    .unwrap();
    let wire: Value =
        serde_json::from_str(frozen.messages[0]["content"].as_str().unwrap()).unwrap();
    assert!(wire["nativeMaterials"].as_array().unwrap().is_empty());
    assert!(wire["nativeReferences"]
        .as_str()
        .unwrap()
        .contains("COURSE_A_BODY_NOT_OPENING_CONTEXT"));
    assert!(!wire.to_string().contains("COURSE_B_BODY_PRIVATE"));
    assert!(wire.get("courseReadManifest").is_none());
    let readable =
        context::course_read_manifest(&c.content, first.course_id.as_deref().unwrap()).unwrap();
    let run: Run = serde_json::from_value(json!({"id":"run_explicit_scope","sessionId":first.id,"mode":"explain","status":"queued","content":"","error":null,"createdAt":"","updatedAt":"","requestDigest":"fixture","contextManifest":[],"courseReadManifest":readable,"contextSnapshotVersion":1})).unwrap();
    assert!(run.context_manifest.is_empty());
    assert!(run
        .read_scope_manifest()
        .iter()
        .any(|m| m.id == a.id && m.version == a.version));
    assert!(!run.read_scope_manifest().iter().any(|m| m.id == b.id));
    assert!(run.context_bridge_manifest.is_none());
}
