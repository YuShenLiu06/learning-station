use super::*;
use crate::{
    app::Engine,
    context_bridge::ContextBridge,
    generation::GenerationRequest,
    learning_tools::{self, ToolScope},
    sessions,
    types::*,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};
fn receive(s: &mut std::net::TcpStream) -> Value {
    s.set_nonblocking(false).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let mut b = vec![];
    let mut block = [0; 8192];
    let (start, len) = loop {
        let n = s.read(&mut block).unwrap();
        assert!(n > 0);
        b.extend_from_slice(&block[..n]);
        if let Some(at) = b.windows(4).position(|x| x == b"\r\n\r\n") {
            let h = String::from_utf8_lossy(&b[..at]);
            let len = h
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap();
            break (at + 4, len);
        }
    };
    while b.len() < start + len {
        let n = s.read(&mut block).unwrap();
        b.extend_from_slice(&block[..n]);
    }
    serde_json::from_slice(&b[start..start + len]).unwrap()
}
#[tokio::test]
async fn real_rust_node_pi_native_restart_roles_tool_pairs_and_course_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = Engine::new(dir.path().join("lib"), dir.path().join("state")).unwrap();
    let (doc, sid, other) = {
        let mut c = e.lock().unwrap();
        let d = c
            .create_section("Native source", "MATERIAL_NATIVE_STABLE", vec![], vec![])
            .unwrap();
        let (s, _) = c.start_session(&d.id, false).unwrap();
        let o = c
            .create_section("Other course", "OTHER_COURSE_SOURCE", vec![], vec![])
            .unwrap();
        let (os, _) = c.start_session(&o.id, false).unwrap();
        (d.id, s.id, os.id)
    };
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    server.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/v1", server.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let mut packets = vec![];
        for index in 0..6 {
            let until = std::time::Instant::now() + Duration::from_secs(45);
            let (mut socket, _) = loop {
                match server.accept() {
                    Ok(v) => break v,
                    Err(x)
                        if x.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < until =>
                    {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(x) => panic!("fixture {x}"),
                }
            };
            packets.push(receive(&mut socket));
            let delta = if index == 0 {
                json!({"tool_calls":[{"index":0,"id":"native_call","type":"function","function":{"name":"create_note","arguments":"{\"content\":\"Durable native note\"}"}}]})
            } else {
                json!({"content":match index{1=>"FIRST_NATIVE_REPLY",2=>"SECOND_NATIVE_REPLY",3=>"OTHER_NATIVE_REPLY",_=>"MODEL_CHANGED_REPLY"}})
            };
            let finish = if index == 0 { "tool_calls" } else { "stop" };
            let body = format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
            );
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).unwrap();
        }
        packets
    });
    let inventory = Arc::new(std::sync::Mutex::new(vec![]));
    let mut first_native = None;
    let mut first_user = String::new();
    for round in 0..4 {
        if round == 1 {
            e = Engine::new(dir.path().join("lib"), dir.path().join("state")).unwrap();
            e.lock().unwrap().content.refresh().unwrap();
        }
        let active = if round == 2 { &other } else { &sid };
        let prompt = if round == 0 {
            format!("QUESTION_NATIVE_0 {}", "x".repeat(6000))
        } else {
            format!("QUESTION_NATIVE_{round}")
        };
        let (s, ctx, projection, user, md, state) = {
            let mut c = e.lock().unwrap();
            let saved = sessions::append(
                &mut c.content,
                active,
                Entry {
                    id: String::new(),
                    kind: "user".into(),
                    content: prompt.clone(),
                    created_at: timestamp(),
                    selection: None,
                    related_ids: vec![],
                    data: None,
                },
                None,
                &format!("native_wire_user_{round}"),
            )
            .unwrap();
            let user = saved.entries.last().unwrap().id.clone();
            let (s, ctx) = c.session_context(active, None).unwrap();
            let p = ContextBridge::snapshot(&c.state_dir, &c.content, &s, &ctx).unwrap();
            let md = c.content.read(&s.id).unwrap().content;
            (s, ctx, p, user, md, c.state_dir.clone())
        };
        if round == 0 {
            first_user = user.clone();
        }
        let run_id = format!("native_wire_run_{round}");
        let tracked:crate::generation::Run=serde_json::from_value(json!({"id":run_id,"sessionId":active,"userEntryId":user,"mode":"question","status":"succeeded","content":"fixture","error":null,"createdAt":timestamp(),"updatedAt":timestamp(),"requestDigest":"native-fixture","contextManifest":ctx.manifest,"contextSnapshotVersion":1})).unwrap();
        std::fs::create_dir_all(state.join("runs")).unwrap();
        crate::app::atomic_json(&state.join("runs").join(format!("{run_id}.json")), &tracked)
            .unwrap();
        let native = crate::native_session::prepare(
            &state,
            &s,
            &run_id,
            &user,
            ctx.manifest.iter().map(|m| m.id.clone()).collect(),
            false,
            vec![],
            &md,
        )
        .unwrap();
        if round == 0 {
            first_native = Some(native.directory.clone());
        }
        let request = GenerationRequest {
            session_id: active.clone(),
            mode: "question".into(),
            selection: String::new(),
            prompt: prompt.clone(),
            request_id: format!("native_wire_op_{round}"),
            user_entry_id: Some(user),
            injections: vec![],
            assistant_references: vec![],
            exercise_references: vec![],
        };
        let frozen = ContextBridge::freeze(
            &request,
            &ctx,
            &s,
            None,
            &projection,
            crate::time_context::TimeContext::capture(),
        )
        .unwrap();
        let scope = ToolScope {
            session_id: active.clone(),
            section_id: s.section_id.clone(),
            run_id,
            allowed_ids: ctx.manifest.iter().map(|m| m.id.clone()).collect(),
        };
        let e2 = e.clone();
        let handler: ToolHandler = Arc::new(move |call| {
            let e = e2.clone();
            let scope = scope.clone();
            Box::pin(
                async move { learning_tools::execute(&e, &scope, &call.id, &call.name, call.args) },
            )
        });
        let inv = inventory.clone();
        let output = generate_scoped(
            ModelCredentials::new(
                ModelSettings {
                    provider: "custom".into(),
                    protocol: PROTOCOL.into(),
                    base_url: endpoint.clone(),
                    model: if round == 3 {
                        "changed-fixture"
                    } else {
                        "fixture-model"
                    }
                    .into(),
                    max_output_tokens: Some(2048),
                    ..ModelSettings::default()
                },
                "fake-native-key-not-real".into(),
            )
            .unwrap(),
            frozen.messages,
            Arc::new(AtomicBool::new(false)),
            |_| {},
            Some(handler),
            ToolPolicy {
                mode: "question".into(),
                allowed_tools: vec!["create_note".into()],
                max_calls: 2,
                authorization: "off".into(),
            },
            crate::learning_skills::capture("question", &["create_note".into()]).unwrap(),
            Some(RequestContext {
                output_validator: None,
                time: frozen.time,
                exploration: None,
                native_session: Some(native),
                context_observer: Some(Arc::new(move |m| {
                    inv.lock().unwrap().push(m);
                    Ok(())
                })),
                tool_recovery: None,
                compaction_observer: None,
            }),
        )
        .await
        .unwrap();
        let mut c = e.lock().unwrap();
        let saved = sessions::append(
            &mut c.content,
            active,
            Entry {
                id: String::new(),
                kind: "assistant".into(),
                content: output,
                created_at: timestamp(),
                selection: None,
                related_ids: vec![],
                data: None,
            },
            None,
            &format!("native_wire_answer_{round}"),
        )
        .unwrap();
        projection
            .commit_native(&state, &saved, &saved.entries.last().unwrap().id)
            .unwrap();
    }
    // Simulate loss before the native tool result and its IPC receipt were durably received.
    // The Rust business write remains committed; recovery must inspect it, never execute again.
    let native_dir = first_native.clone().unwrap();
    let transcript = native_dir.join("transcript.jsonl");
    let raw = std::fs::read_to_string(&transcript).unwrap();
    let lines = raw.lines().collect::<Vec<_>>();
    let at = lines
        .iter()
        .position(|line| {
            let v: Value = serde_json::from_str(line).unwrap();
            v["type"] == "message" && v["message"]["role"] == "toolResult"
        })
        .unwrap();
    std::fs::write(&transcript, format!("{}\n", lines[..at].join("\n"))).unwrap();
    let receipt = native_dir.join(format!(
        "tool-{}.json",
        hash(&serde_json::to_string(&json!(["native_wire_run_0", "native_call"])).unwrap())
    ));
    std::fs::remove_file(receipt).unwrap();
    let (s, ctx, projection, md, state) = {
        let mut c = e.lock().unwrap();
        let (s, ctx) = c.session_context(&sid, None).unwrap();
        let p = ContextBridge::snapshot(&c.state_dir, &c.content, &s, &ctx).unwrap();
        let md = c.content.read(&sid).unwrap().content;
        (s, ctx, p, md, c.state_dir.clone())
    };
    let prompt = format!("QUESTION_NATIVE_0 {}", "x".repeat(6000));
    let request = GenerationRequest {
        session_id: sid.clone(),
        mode: "question".into(),
        selection: String::new(),
        prompt,
        request_id: "native_recover_op".into(),
        user_entry_id: Some(first_user.clone()),
        injections: vec![],
        assistant_references: vec![],
        exercise_references: vec![],
    };
    let frozen = ContextBridge::freeze(
        &request,
        &ctx,
        &s,
        None,
        &projection,
        crate::time_context::TimeContext::capture(),
    )
    .unwrap();
    let native = crate::native_session::prepare(
        &state,
        &s,
        "native_recover_run",
        &first_user,
        ctx.manifest.iter().map(|m| m.id.clone()).collect(),
        false,
        vec![],
        &md,
    )
    .unwrap();
    let restored = e.clone();
    let restore_sid = sid.clone();
    let recovery: ToolRecoveryHandler = Arc::new(move |original, call| {
        let e = restored.clone();
        let sid = restore_sid.clone();
        Box::pin(async move { crate::native_session::recover(&e, &sid, &original, &call) })
    });
    let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = writes.clone();
    let forbidden: ToolHandler = Arc::new(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(CoreError::new(
                "INTERNAL",
                "Recovery must not execute writes",
            ))
        })
    });
    generate_scoped(
        ModelCredentials::new(
            ModelSettings {
                provider: "custom".into(),
                protocol: PROTOCOL.into(),
                base_url: endpoint.clone(),
                model: "fixture-model".into(),
                ..ModelSettings::default()
            },
            "fake-native-key-not-real".into(),
        )
        .unwrap(),
        frozen.messages,
        Arc::new(AtomicBool::new(false)),
        |_| {},
        Some(forbidden),
        ToolPolicy {
            mode: "question".into(),
            allowed_tools: vec!["create_note".into()],
            max_calls: 2,
            authorization: "off".into(),
        },
        crate::learning_skills::capture("question", &["create_note".into()]).unwrap(),
        Some(RequestContext {
            output_validator: None,
            time: frozen.time,
            exploration: None,
            native_session: Some(native),
            context_observer: None,
            tool_recovery: Some(recovery),
            compaction_observer: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    let packets = handle.join().unwrap();
    assert_eq!(packets.len(), 6);
    for packet in &packets {
        assert!(!serde_json::to_string(packet)
            .unwrap()
            .contains("/pi-native/"));
        for tool in packet["tools"].as_array().unwrap() {
            assert_eq!(tool["function"]["name"], "create_note");
        }
    }
    let second = serde_json::to_string(&packets[2]).unwrap();
    assert!(second.contains("FIRST_NATIVE_REPLY"));
    assert!(second.contains("QUESTION_NATIVE_0"));
    assert!(second.contains(&"x".repeat(6000)));
    assert!(!second.contains("learningHistory"));
    assert!(packets[2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool"));
    assert!(!serde_json::to_string(&packets[3])
        .unwrap()
        .contains("FIRST_NATIVE_REPLY"));
    assert_eq!(packets[4]["model"], "changed-fixture");
    assert!(inventory.lock().unwrap().iter().any(|m| m["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["kind"] == "material" && i["status"] == "retained")));
    let restored_packet = serde_json::to_string(&packets[5]).unwrap();
    assert!(restored_packet.contains("native_call"));
    assert!(restored_packet.contains("recovered"));
    assert!(!restored_packet.contains("No result provided"));
    let p = first_native.unwrap();
    let raw = std::fs::read_to_string(p.join("transcript.jsonl")).unwrap();
    assert!(!raw.contains("fake-native-key-not-real"));
    assert!(raw.contains("\"role\":\"toolResult\""));
    assert!(raw.contains("\"type\":\"toolCall\""));
    assert!(p.join("legacy-backup.md").is_file());
    let mut c = e.lock().unwrap();
    let s = sessions::get(&mut c.content, &sid).unwrap();
    assert_eq!(
        s.entries
            .iter()
            .filter(|e| e.content.contains("Durable native note"))
            .count(),
        1
    );
    assert_eq!(doc, s.section_id);
    if let Ok(target) = std::env::var("LEARNING_STATION_NATIVE_EVIDENCE") {
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(Path::new(&target).join("actual-packets.json"),serde_json::to_vec_pretty(&json!({"syntheticOnly":true,"realRustNodePinnedPi":true,"packets":packets,"manifests":*inventory.lock().unwrap()})).unwrap()).unwrap();
    }
}

#[tokio::test]
async fn rust_node_pi_validates_final_json_and_corrects_before_any_persistence() {
    for unauthorized in [false, true] {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", server.local_addr().unwrap());
        let expected = if unauthorized { 1 } else { 2 };
        let handle = thread::spawn(move || {
            let mut packets = vec![];
            for index in 0..expected {
                let until = std::time::Instant::now() + Duration::from_secs(15);
                let (mut socket, _) = loop {
                    match server.accept() {
                        Ok(v) => break v,
                        Err(x)
                            if x.kind() == std::io::ErrorKind::WouldBlock
                                && std::time::Instant::now() < until =>
                        {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(x) => panic!("fixture {x}"),
                    }
                };
                packets.push(receive(&mut socket));
                let content = json!({"summary":"合成练习","exercises":[{"question":if index==0 && !unauthorized {" "} else {"解释概念"},"format":"self","answers":["合成答案"],"tags":[],"sourceIds":if unauthorized {json!(["unauthorized_source"])} else {json!([])}}]}).to_string();
                let body = format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({"choices":[{"index":0,"delta":{"content":content},"finish_reason":"stop"}]})
                );
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).unwrap();
            }
            packets
        });
        let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = checks.clone();
        let mut context = RequestContext::capture();
        context.output_validator = Some(Arc::new(move |text| {
            observed.fetch_add(1, Ordering::SeqCst);
            let data = serde_json::from_str(text)
                .map_err(|_| crate::exercises::field_error("json", "必须为JSON"))?;
            crate::exercises::validate(data, &[], "validation-run").map(|_| ())
        }));
        let result = generate_scoped(
            ModelCredentials::new(
                ModelSettings {
                    provider: "custom".into(),
                    protocol: PROTOCOL.into(),
                    base_url: endpoint,
                    model: "fixture-model".into(),
                    ..ModelSettings::default()
                },
                "synthetic-validation-key".into(),
            )
            .unwrap(),
            vec![json!({"role":"user","content":"生成合成练习"})],
            Arc::new(AtomicBool::new(false)),
            |_| {},
            None,
            ToolPolicy {
                mode: "practice".into(),
                allowed_tools: vec![],
                max_calls: 0,
                authorization: "off".into(),
            },
            crate::learning_skills::capture("practice", &[]).unwrap(),
            Some(context),
        )
        .await
        .unwrap();
        assert_eq!(checks.load(Ordering::SeqCst), expected);
        let packets = handle.join().unwrap();
        if unauthorized {
            assert!(result.contains("unauthorized_source"));
        } else {
            assert!(serde_json::to_string(&packets[1])
                .unwrap()
                .contains("exercises[1].question"));
            assert!(crate::exercises::validate(
                serde_json::from_str(&result).unwrap(),
                &[],
                "final-result"
            )
            .is_ok());
        }
    }
}

/// Issue 3/4 at the real Rust↔Node wire: an invalid generate_exercises call gets an
/// INVALID_MODEL_OUTPUT field receipt inside the same native Agent loop, the model
/// corrects once, and exactly one durable exercise write survives.
#[tokio::test]
async fn rust_node_pi_exercise_tool_receipt_corrects_invalid_output_in_same_native_loop() {
    let dir = tempfile::tempdir().unwrap();
    let e = Engine::new(dir.path().join("lib"), dir.path().join("state")).unwrap();
    let (s, ctx, user, md, state) = {
        let mut c = e.lock().unwrap();
        let d = c
            .create_section("Synthetic source", "SYNTHETIC_MATERIAL_STABLE", vec![], vec![])
            .unwrap();
        let (s, ctx) = c.start_session(&d.id, false).unwrap();
        let saved = sessions::append(
            &mut c.content,
            &s.id,
            Entry {
                id: String::new(),
                kind: "user".into(),
                content: "请生成一道合成练习".into(),
                created_at: timestamp(),
                selection: None,
                related_ids: vec![],
                data: None,
            },
            None,
            "exercise_wire_user",
        )
        .unwrap();
        let user = saved.entries.last().unwrap().id.clone();
        let md = c.content.read(&s.id).unwrap().content;
        (s, ctx, user, md, c.state_dir.clone())
    };
    let sid = s.id.clone();
    let run_id = "exercise_wire_run";
    let tracked: crate::generation::Run = serde_json::from_value(json!({"id":run_id,"sessionId":sid,"userEntryId":user,"mode":"question","status":"succeeded","content":"fixture","error":null,"createdAt":timestamp(),"updatedAt":timestamp(),"requestDigest":"exercise-fixture","contextManifest":ctx.manifest,"contextSnapshotVersion":1})).unwrap();
    std::fs::create_dir_all(state.join("runs")).unwrap();
    crate::app::atomic_json(&state.join("runs").join(format!("{run_id}.json")), &tracked).unwrap();
    e.runs.lock().unwrap().insert(
        run_id.into(),
        crate::generation::ManagedRun {
            run: tracked,
            cancel: Arc::new(AtomicBool::new(false)),
        },
    );
    let authorized: Vec<String> = ctx.manifest.iter().map(|m| m.id.clone()).collect();
    let native =
        crate::native_session::prepare(&state, &s, run_id, &user, authorized.clone(), false, vec![], &md)
            .unwrap();
    let projection = {
        let c = e.lock().unwrap();
        ContextBridge::snapshot(&state, &c.content, &s, &ctx).unwrap()
    };
    let request = GenerationRequest {
        session_id: sid.clone(),
        mode: "question".into(),
        selection: String::new(),
        prompt: "请生成一道合成练习".into(),
        request_id: "exercise_wire_op".into(),
        user_entry_id: Some(user),
        injections: vec![],
        assistant_references: vec![],
        exercise_references: vec![],
    };
    let frozen =
        ContextBridge::freeze(&request, &ctx, &s, None, &projection, crate::time_context::TimeContext::capture())
            .unwrap();
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    server.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/v1", server.local_addr().unwrap());
    let wire_section = authorized[0].clone();
    let handle = thread::spawn(move || {
        let mut packets = vec![];
        for index in 0..3 {
            let until = std::time::Instant::now() + Duration::from_secs(45);
            let (mut socket, _) = loop {
                match server.accept() {
                    Ok(v) => break v,
                    Err(x)
                        if x.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < until =>
                    {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(x) => panic!("fixture {x}"),
                }
            };
            packets.push(receive(&mut socket));
            // Whitespace-only question text passes every schema and preflight gate;
            // only Rust's validator rejects it with a field-level diagnostic.
            let exercise_call = |id: &str, question: &str| {
                json!({"tool_calls":[{"index":0,"id":id,"type":"function","function":{
                    "name":"generate_exercises",
                    "arguments":serde_json::to_string(&json!({"requirements":"合成练习要求","summary":"合成总结","exercises":[{
                        "question":question,"format":"exact","answers":["合成参考答案"],"tags":[],"sourceIds":[wire_section]
                    }]})).unwrap()
                }}]})
            };
            let (delta, finish_reason) = if index == 0 {
                (exercise_call("exercise_bad", "   "), "tool_calls")
            } else if index == 1 {
                let seen = serde_json::to_string(&packets).unwrap();
                assert!(seen.contains("INVALID_MODEL_OUTPUT"));
                assert!(seen.contains("exercises[1].question"), "diagnostic missing");
                (exercise_call("exercise_good", "解释合成概念"), "tool_calls")
            } else {
                let seen = serde_json::to_string(&packets).unwrap();
                assert!(seen.contains("exdoc_"), "persisted receipt missing");
                (json!({"content":"已按真实回执保存合成练习。"}), "stop")
            };
            let payload = json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish_reason}]});
            let body = format!("data: {payload}\n\ndata: [DONE]\n\n");
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).unwrap();
        }
        packets
    });
    let scope = ToolScope {
        session_id: sid.clone(),
        section_id: s.section_id.clone(),
        run_id: run_id.into(),
        allowed_ids: authorized,
    };
    let e2 = e.clone();
    let handler: ToolHandler = Arc::new(move |call| {
        let e = e2.clone();
        let scope = scope.clone();
        Box::pin(
            async move { learning_tools::execute(&e, &scope, &call.id, &call.name, call.args) },
        )
    });
    let output = generate_scoped(
        ModelCredentials::new(
            ModelSettings {
                provider: "custom".into(),
                protocol: PROTOCOL.into(),
                base_url: endpoint,
                model: "fixture-model".into(),
                ..ModelSettings::default()
            },
            "synthetic-exercise-key-not-real".into(),
        )
        .unwrap(),
        frozen.messages,
        Arc::new(AtomicBool::new(false)),
        |_| {},
        Some(handler),
        ToolPolicy {
            mode: "question".into(),
            allowed_tools: vec!["generate_exercises".into()],
            max_calls: 4,
            authorization: "off".into(),
        },
        crate::learning_skills::capture("question", &["generate_exercises".into()]).unwrap(),
        Some(RequestContext {
            output_validator: None,
            time: frozen.time,
            exploration: None,
            native_session: Some(native.clone()),
            context_observer: None,
            tool_recovery: None,
            compaction_observer: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(output, "已按真实回执保存合成练习。");
    let packets = handle.join().unwrap();
    for packet in &packets {
        let wire = serde_json::to_string(packet).unwrap();
        assert!(!wire.contains("synthetic-exercise-key-not-real"));
        for tool in packet["tools"].as_array().unwrap() {
            assert_eq!(tool["function"]["name"], "generate_exercises");
        }
    }
    let receipt = |call_id: &str| {
        serde_json::from_str::<Value>(&std::fs::read_to_string(native.directory.join(format!(
            "tool-{}.json",
            hash(&serde_json::to_string(&json!([run_id, call_id])).unwrap())
        )))
        .unwrap())
        .unwrap()
    };
    let bad = receipt("exercise_bad");
    assert_eq!(bad["reply"]["error"]["code"], "INVALID_MODEL_OUTPUT");
    assert!(bad["reply"]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("exercises[1].question"));
    let good = receipt("exercise_good");
    assert_eq!(good["reply"]["result"]["persisted"], true);
    let document_id = good["reply"]["result"]["documentId"].as_str().unwrap().to_owned();
    assert!(document_id.starts_with("exdoc_"));
    let mut c = e.lock().unwrap();
    let saved = sessions::get(&mut c.content, &sid).unwrap();
    let exercise_entries: Vec<_> = saved
        .entries
        .iter()
        .filter(|entry| entry.kind == "exercise")
        .collect();
    assert_eq!(exercise_entries.len(), 1, "failed call must not persist");
    let data = exercise_entries[0].data.as_ref().unwrap();
    assert_eq!(data["requirements"], "合成练习要求");
    assert!(data["exercises"][0].get("explanation").is_none());
    assert!(crate::exercises::get(&mut c.content, &document_id)
        .unwrap()
        .document
        .body
        .contains("尚未提供解析。"));
}

