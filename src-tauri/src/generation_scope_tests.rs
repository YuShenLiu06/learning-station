use super::*;

#[tokio::test]
async fn course_start_without_focus_keeps_readable_versions_separate_from_explicit_body() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
    let (a, b, first) = {
        let mut c = engine.lock().unwrap();
        let owner = crate::library_tree::create(
            &mut c.content,
            "course",
            "A",
            None,
            "",
            "scope-context-course-a",
        )
        .unwrap();
        let other = crate::library_tree::create(
            &mut c.content,
            "course",
            "B",
            None,
            "",
            "scope-context-course-b",
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
        let readable = context::course_read_manifest(&c.content, &owner.id).unwrap();
        assert!(readable
            .iter()
            .any(|m| m.id == a.id && m.version == a.version));
        assert!(!readable.iter().any(|m| m.id == b.id));
        (a, b, first)
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
    // Source capture happens before that gate.
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
        let (_, ctx) = c.session_context_for(&session.id, Some(""), None).unwrap();
        let readable =
            context::course_read_manifest(&c.content, session.course_id.as_deref().unwrap())
                .unwrap();
        let native = crate::native_session::prepare(
            &c.state_dir,
            &session,
            "scope_native",
            "scope_user",
            readable.iter().map(|item| item.id.clone()).collect(),
            false,
            vec![],
            "",
        )
        .unwrap();
        assert!(native.authorized_source_ids.contains(&a.id));
        assert!(!native.authorized_source_ids.contains(&b.id));
        assert!(ctx.manifest.is_empty());
        assert!(ctx.text.is_empty());
    }
    request.request_id = "project-context-explicit-request".into();
    request.injections.push(context::ContextInjection {
        document_id: a.id.clone(),
        expected_version: a.version.clone(),
        selection: None,
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

#[tokio::test]
async fn course_selection_and_practice_keep_explicit_body_and_reject_other_courses() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
    let (session, source, other) = {
        let mut c = engine.lock().unwrap();
        let course = crate::library_tree::create(
            &mut c.content,
            "course",
            "Course",
            None,
            "",
            "selection-course",
        )
        .unwrap();
        let source = crate::library_tree::create(
            &mut c.content,
            "section",
            "Source",
            Some(&course.id),
            "EXPLICIT_SELECTION_BODY",
            "selection-source",
        )
        .unwrap();
        let other = crate::library_tree::create(
            &mut c.content,
            "section",
            "Other",
            None,
            "CROSS_COURSE_PRIVATE",
            "selection-other",
        )
        .unwrap();
        let session =
            crate::course_sessions::create(&mut c, &course.id, "selection-session").unwrap();
        (session, source, other)
    };
    let mut request = GenerationRequest {
        session_id: session.id.clone(),
        document_id: Some(source.id.clone()),
        mode: "explain".into(),
        selection: "EXPLICIT_SELECTION_BODY".into(),
        prompt: "Explain selection".into(),
        request_id: "scope-selection".into(),
        user_entry_id: None,
        injections: vec![],
        assistant_references: vec![],
        exercise_references: vec![],
    };
    for mode in ["explain", "practice"] {
        request.mode = mode.into();
        request.request_id = format!("scope-{mode}");
        if mode == "practice" {
            request.selection.clear();
        }
        assert_eq!(
            start(engine.clone(), request.clone(), Arc::new(|_| {}))
                .await
                .unwrap_err()
                .code,
            "NOT_CONFIGURED"
        );
        let mut c = engine.lock().unwrap();
        let frozen = sessions::get(&mut c.content, &session.id).unwrap();
        assert!(frozen
            .context_manifest
            .iter()
            .any(|m| m.id == source.id && m.included_chars > 0));
    }
    request.mode = "explain".into();
    request.request_id = "scope-cross-course".into();
    request.injections.push(context::ContextInjection {
        document_id: other.id,
        expected_version: other.version,
        selection: None,
    });
    assert_eq!(
        start(engine.clone(), request, Arc::new(|_| {}))
            .await
            .unwrap_err()
            .code,
        "OUT_OF_SCOPE"
    );
}

#[test]
fn old_runs_keep_frozen_context_authorization_without_inventing_course_scope() {
    let mut run: Run = serde_json::from_value(json!({
        "id":"run_legacy_scope", "sessionId":"legacy_session", "mode":"question", "status":"queued", "content":"", "error":null,
        "createdAt":"", "updatedAt":"", "requestDigest":"legacy", "contextSnapshotVersion":1,
        "contextManifest":[{"id":"legacy_source", "path":"legacy.md", "version":"frozen", "includedChars":1}]
    })).unwrap();
    assert!(run.course_read_manifest.is_none());
    assert_eq!(run.read_scope_manifest()[0].id, "legacy_source");
    run.course_read_manifest = Some(vec![]);
    assert!(run.read_scope_manifest().is_empty());
}
