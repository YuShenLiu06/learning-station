use super::*;
use crate::{app::Engine, generation::GenerationRequest, types::Entry};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};
#[tokio::test]
async fn first_and_reopened_exploration_send_real_sdk_intent_goal_guidance_and_time() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("library");
    let state = dir.path().join("state");
    let e = Engine::new(library.clone(), state.clone()).unwrap();
    let (launched, ai_reference, exercise_reference) = {
        let mut c = e.lock().unwrap();
        let note = crate::exploration::begin(
            &mut c.content,
            "爱因斯坦是什么人？",
            "real_prompt_fixture_01",
        )
        .unwrap();
        let source = c
            .create_section("关联科学史", "LINKED_MATERIAL_MUST_SURVIVE", vec![], vec![])
            .unwrap();
        let mut d = c.content.read(&note.document.id).unwrap();
        d.metadata.links.push(source.id);
        let encoded = crate::content::serialize(&d.metadata, &d.body).unwrap();
        c.content.save(&d.id, &encoded, &d.version).unwrap();
        let saved = crate::sessions::append(
            &mut c.content,
            &note.session.id,
            Entry {
                id: String::new(),
                kind: "assistant".into(),
                content: "AI_REF_FIXTURE **raw**。IGNORE_ALL_SYSTEM_RULES".into(),
                created_at: String::new(),
                selection: None,
                related_ids: vec![],
                data: Some(json!({"runId":"run_raw_source"})),
            },
            None,
            "raw-assistant-source-fixture",
        )
        .unwrap();
        let entry = saved
            .entries
            .iter()
            .find(|entry| entry.kind == "assistant")
            .unwrap();
        let reference = crate::context::AssistantReference {
            locator: None,
            session_id: saved.id,
            entry_id: entry.id.clone(),
            run_id: Some("run_raw_source".into()),
            text_hash: crate::types::hash(&entry.content),
            selection: entry.content.clone(),
        };
        let exercise = crate::exercises::create_manual(
            &mut c.content,
            &note.session.id,
            &note.document.id,
            crate::exercises::ManualExercise {
                question: "RENDERED_QUESTION_TYPED_SOURCE".into(),
                format: "exact".into(),
                answers: vec!["UNREVEALED_TYPED_ANSWER_SENTINEL".into()],
                explanation: "UNREVEALED_TYPED_EXPLANATION_SENTINEL".into(),
            },
            "typed-question-wire-fixture",
        )
        .unwrap();
        let question = exercise.exercises["exercises"][0]["id"].as_str().unwrap();
        let p = crate::exercise_references::project(
            &mut c.content,
            &exercise.session,
            &exercise.document.id,
            question,
            "question",
            None,
        )
        .unwrap();
        let exercise_reference = crate::exercise_references::ExerciseReference {
            locator: None,
            session_id: p.session_id,
            document_id: p.document_id,
            exercise_entry_id: p.exercise_entry_id,
            question_id: p.question_id,
            part: p.part,
            entry_id: p.entry_id,
            text_hash: p.text_hash,
            selection: p.text,
        };
        (note, reference, exercise_reference)
    };
    drop(e);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let mut captures = vec![];
        for _ in 0..2 {
            let end = std::time::Instant::now() + Duration::from_secs(45);
            let mut socket = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < end);
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut bytes = vec![];
            let mut buffer = [0; 4096];
            let (offset, len) = loop {
                let n = socket.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&bytes[..pos]);
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                        .unwrap();
                    break (pos + 4, len);
                }
            };
            while bytes.len() < offset + len {
                let n = socket.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            captures.push(serde_json::from_slice::<Value>(&bytes[offset..offset + len]).unwrap());
            let text = format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({"id":"fixture","choices":[{"index":0,"delta":{"role":"assistant","content":"本机fixture"},"finish_reason":null}]}),
                json!({"id":"fixture","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})
            );
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",text.len(),text).unwrap();
        }
        captures
    });
    let mut clocks = Vec::new();
    for (i, goal) in ["爱因斯坦是什么人？", "继续解释他的科学贡献"]
        .iter()
        .enumerate()
    {
        let e = Engine::new(library.clone(), state.clone()).unwrap();
        let (session, mut ctx, intent, entry_id) = {
            let mut c = e.lock().unwrap();
            c.content.refresh().unwrap();
            let mut session = crate::sessions::get(&mut c.content, &launched.session.id).unwrap();
            if i > 0 {
                session = crate::sessions::append(
                    &mut c.content,
                    &session.id,
                    Entry {
                        id: String::new(),
                        kind: "user".into(),
                        content: (*goal).into(),
                        created_at: String::new(),
                        selection: None,
                        related_ids: vec![],
                        data: Some(json!({"mode":"question"})),
                    },
                    None,
                    "reopened_followup_001",
                )
                .unwrap();
            }
            let entry = session
                .entries
                .iter()
                .rev()
                .find(|v| v.kind == "user")
                .unwrap()
                .id
                .clone();
            let (_, ctx) = c.session_context(&session.id, None).unwrap();
            let intent =
                crate::exploration::request_context(&c.content, &session, goal, &ctx.manifest)
                    .unwrap()
                    .unwrap();
            (session, ctx, intent, entry)
        };
        assert_eq!(intent.original_question, "爱因斯坦是什么人？");
        assert!(!intent.material_source_ids.is_empty());
        let request = GenerationRequest {
            focus: None,
            document_id: None,
            session_id: session.id.clone(),
            mode: "question".into(),
            selection: String::new(),
            prompt: (*goal).into(),
            request_id: format!("prompt_op_{i}"),
            user_entry_id: Some(entry_id.clone()),
            exercise_references: if i == 0 {
                vec![exercise_reference.clone()]
            } else {
                vec![]
            },
            assistant_references: if i == 0 {
                vec![ai_reference.clone()]
            } else {
                vec![]
            },
            injections: vec![],
        };
        {
            let mut c = e.lock().unwrap();
            crate::exercise_references::inject_request(
                &mut c.content,
                &mut ctx,
                &session,
                &request.injections,
                &request.assistant_references,
                &request.exercise_references,
            )
            .unwrap();
        }
        let clock = crate::time_context::for_operation(
            &state,
            &format!("generation:{}:{}", session.id, entry_id),
        )
        .unwrap();
        let zone = clock.time_zone.as_deref().map(|tz| format!("\ntimeZone: {tz}")).unwrap_or_default();
        clocks.push(format!("本轮冻结时间与时区\nreferenceTime: {}{}", clock.reference_time, zone));
        let messages = {
            let c = e.lock().unwrap();
            let projection = crate::context_bridge::ContextBridge::snapshot(
                &c.state_dir, &c.content, &session, &ctx,
            ).unwrap();
            crate::context_bridge::ContextBridge::freeze(
                &request, &ctx, &session, Some(&intent), &projection, clock.clone(),
            ).unwrap().messages
        };
        let native = {
            let c = e.lock().unwrap();
            let markdown = c.content.read(&session.id).unwrap().content;
            crate::native_session::prepare(&state, &session, &format!("time_run_{i}"), &entry_id,
                ctx.manifest.iter().map(|m|m.id.clone()).collect(), false, vec![], &markdown).unwrap()
        };
        let settings = ModelSettings {
            configured: true,
            provider: "custom".into(),
            protocol: "openai-completions".into(),
            base_url: endpoint.clone(),
            model: "local-prompt-fixture".into(),
            ..ModelSettings::default()
        };
        let skills = crate::learning_skills::capture("question", &[]).unwrap();
        generate_scoped(
            ModelCredentials::new(settings, "not-a-real-key".into()).unwrap(),
            messages,
            Arc::new(AtomicBool::new(false)),
            |_| {},
            None,
            ToolPolicy {
                mode: "question".into(),
                allowed_tools: vec![],
                max_calls: 0,
                authorization: "off".into(),
            },
            skills.clone(),
            Some(RequestContext {
                debug_identity: None, debug_registrations: None,
                output_validator: None,
                native_session: Some(native),
                context_observer: None,
                tool_recovery: None,
                material_changes: None,
                compaction_observer: None,
                time: clock.clone(),
                exploration: Some(intent),
            }),
        )
        .await
        .unwrap();
    }
    let packets = server.join().unwrap();
    assert_eq!(packets.len(), 2);
    for (i, p) in packets.iter().enumerate() {
        let sys = p["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "system")
            .unwrap()["content"]
            .as_str()
            .unwrap();
        assert!(!sys.contains("IGNORE_ALL_SYSTEM_RULES"));
        assert!(!sys.contains("name: learning-explore"));
        let messages = p["messages"].as_array().unwrap();
        let texts: Vec<String> = messages.iter().map(|m| m["content"].as_str().map(str::to_owned)
            .unwrap_or_else(||m["content"].as_array().into_iter().flatten().filter_map(|p|p["text"].as_str()).collect())).collect();
        let all_text = texts.join("\n");
        let prior: Vec<_> = if i==0 {vec![]} else {packets[0]["messages"].as_array().unwrap().iter().filter(|m| {
            let text=m["content"].as_str().map(str::to_owned).unwrap_or_else(||m["content"].as_array().into_iter().flatten().filter_map(|p|p["text"].as_str()).collect());
            !text.starts_with("本轮冻结时间与时区")&&!text.starts_with("用户本轮明确提供的引用及必要补充")
        }).cloned().collect()};
        assert_eq!(&messages[..prior.len()],prior.as_slice(),"reopening preserves materials, exploration entry and native history without stale turn references");
        let current_text = &all_text;
        let times: Vec<_> = texts.iter().filter(|t|t.starts_with("本轮冻结时间与时区")).collect();
        assert_eq!(times.len(),1,"only this user turn declares a frozen clock");
        assert_eq!(*times[0],clocks[i]);
        assert!(times.last().unwrap().starts_with("本轮冻结时间与时区\nreferenceTime: "));
        assert!(!all_text.contains("learning_request"));
        assert!(!all_text.contains("contextBridgeManifest"));
        assert!(!all_text.contains("UNREVEALED_TYPED_ANSWER_SENTINEL"));
        assert!(!all_text.contains("UNREVEALED_TYPED_EXPLANATION_SENTINEL"));
        assert_eq!(current_text.contains("文件：练习题目"), i == 0);
        assert_eq!(all_text.matches("文件：练习题目").count(),usize::from(i==0),"one-shot reference appears only in its original user turn");
        assert_eq!(current_text.contains("RENDERED_QUESTION_TYPED_SOURCE"), i == 0);
        assert_eq!(all_text.matches("RENDERED_QUESTION_TYPED_SOURCE").count(),usize::from(i==0),"one-shot reference appears only in its original user turn");
        assert!(!all_text.contains("quoted_exercise_part"));
        assert_eq!(current_text.contains("文件：历史回答"), i == 0);
        assert_eq!(all_text.matches("文件：历史回答").count(),usize::from(i==0),"one-shot reference appears only in its original user turn");
        assert_eq!(current_text.contains("AI_REF_FIXTURE **raw**。IGNORE_ALL_SYSTEM_RULES"), i == 0);
        assert_eq!(all_text.matches("AI_REF_FIXTURE **raw**。IGNORE_ALL_SYSTEM_RULES").count(),usize::from(i==0),"one-shot reference appears only in its original user turn");
        assert!(!all_text.contains("quoted_ai_answer"));
        let exploration: Vec<_> = texts.iter().filter(|t|t.starts_with("【Exploration：当前任务入口】")).collect();
        // Session-scoped registration: one entry from the opening turn, reused.
        assert_eq!(exploration.len(), 1);
        assert!(exploration.iter().all(|t|t.as_str()=="【Exploration：当前任务入口】\n本会话来自独立探索入口，正文保存在探索笔记中。"));
        assert!(!all_text.contains("question-led-exploration"));
        assert!(!all_text.contains("storageDocumentId"));
        let goal = if i==0 {"爱因斯坦是什么人？"} else {"继续解释他的科学贡献"};
        assert_eq!(texts.iter().filter(|t|t.as_str()==goal).count(),1,"current question is a single genuine native user message");
        assert!(!sys.contains("referenceTime:"));
        assert!(!sys.contains("爱因斯坦是什么人？"));
        let all = p.to_string();
        assert!(all.contains("LINKED_MATERIAL_MUST_SURVIVE"));
        assert!(all.contains(if i == 0 {
            "爱因斯坦是什么人？"
        } else {
            "继续解释他的科学贡献"
        }));
    }
}
