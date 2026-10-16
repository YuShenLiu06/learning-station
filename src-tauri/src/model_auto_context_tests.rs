use super::*;
use crate::{
    app::Engine,
    context,
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
fn course_body() -> String {
    (0..180)
        .map(|n| format!("Line {n:03}: Original public course statement with a stable example.\n"))
        .collect()
}
fn socket_body(socket: &mut std::net::TcpStream) -> Value {
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(8)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    let (start, len) = loop {
        let n = socket.read(&mut chunk).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(at) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            let h = String::from_utf8_lossy(&bytes[..at]);
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
    while bytes.len() < start + len {
        let n = socket.read(&mut chunk).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
    }
    serde_json::from_slice(&bytes[start..start + len]).unwrap()
}
#[tokio::test]
async fn context_bridge_real_pi_wire_replays_full_diff_once_per_tool_loop_and_failed_retry() {
    let dir = tempfile::tempdir().unwrap();
    let e = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
    let (doc, sid) = {
        let mut c = e.lock().unwrap();
        let d = c
            .create_section("Course fixture", &course_body(), vec![], vec![])
            .unwrap();
        let (s, _) = c.start_session(&d.id, false).unwrap();
        (d.id, s.id)
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let mut packets = Vec::new();
        for index in 0..8 {
            let deadline = std::time::Instant::now() + Duration::from_secs(45);
            let (mut socket, _) = loop {
                match listener.accept() {
                    Ok(v) => break v,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            };
            let payload = socket_body(&mut socket);
            packets.push(payload);
            let body = if index < 2 {
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":format!("read_{index}"),"type":"function","function":{"name":"query_related_knowledge","arguments":"{\"query\":\"Original public course\"}"}}]},"finish_reason":"tool_calls"}]})
                )
            } else {
                format!(
                    "data: {}\n\n{}",
                    json!({"choices":[{"index":0,"delta":{"content":"Fixture answer"},"finish_reason":if index==5 {Value::Null}else{json!("stop")}}]}),
                    if index == 5 { "" } else { "data: [DONE]\n\n" }
                )
            };
            let response=format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            socket.write_all(response.as_bytes()).unwrap();
        }
        packets
    });
    let mut projections = Vec::new();
    let mut sizes = Vec::new();
    let mut failed_projection = String::new();
    for round in 0..6 {
        if round >= 2 && round != 4 {
            let body = if round == 2 {
                course_body().replace("Line 080: Original", "Line 080: Revised")
            } else if round == 3 {
                course_body()
                    .replace("Line 080: Original", "Line 080: Revised")
                    .replace("Line 090: Original", "Line 090: Retried")
            } else {
                course_body().replace(
                    "Original public course statement",
                    "Completely rewritten new content",
                )
            };
            let mut c = e.lock().unwrap();
            let d = c.content.read(&doc).unwrap();
            c.content
                .save(
                    &doc,
                    &crate::content::serialize(&d.metadata, &body).unwrap(),
                    &d.version,
                )
                .unwrap();
        }
        let prompt = format!("Fixture question {round}");
        let (s, ctx, projection, user) = {
            let mut c = e.lock().unwrap();
            let saved = sessions::append(
                &mut c.content,
                &sid,
                Entry {
                    id: String::new(),
                    kind: "user".into(),
                    content: prompt.clone(),
                    created_at: timestamp(),
                    selection: None,
                    related_ids: vec![],
                    data: Some(json!({"mode":"question"})),
                },
                None,
                &format!("bridge_user_{round}"),
            )
            .unwrap();
            let user = saved.entries.last().unwrap().id.clone();
            let (s, mut ctx) = c.session_context(&sid, None).unwrap();
            context::attach_history(&mut ctx, &s, Some(&user));
            let p = ContextBridge::snapshot(&c.state_dir, &c.content, &s, &ctx).unwrap();
            (s, ctx, p, user)
        };
        let request = GenerationRequest {
            session_id: sid.clone(),
            mode: "question".into(),
            selection: String::new(),
            prompt,
            request_id: format!("bridge_op_{round}"),
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
        let sent = frozen.messages[0]["content"].as_str().unwrap();
        let json: Value = serde_json::from_str(sent).unwrap();
        assert!(sent.find("\"learningHistory\"").unwrap() < sent.find("\"context\"").unwrap());
        assert!(sent.find("\"context\"").unwrap() < sent.rfind("\"question\"").unwrap());
        assert_eq!(json["contextBridgeManifest"], frozen.manifest);
        assert_eq!(json["manifest"], json!(ctx.manifest));
        assert!(json["turnState"]["turn"]["referenceTime"]
            .as_str()
            .is_some());
        if round == 3 {
            failed_projection = json["context"].as_str().unwrap().into();
        }
        if round == 4 {
            assert_eq!(json["context"], failed_projection);
        }
        projections.push(json.clone());
        let legacy = crate::generation::messages_with_intent(&request, &ctx, &s, None);
        let previous_envelope = legacy.last().unwrap()["content"].as_str().unwrap().len();
        sizes.push(json!({"round":round,"mode":projection.stats["updates"][0]["mode"],"newMaterialBytes":projection.stats["appendedBytes"],"projectedMaterialBytes":projection.text.len(),"previousSingleFullMaterialBytes":ctx.text.len(),"userEnvelopeBytes":sent.len(),"previousSingleFullEnvelopeBytes":previous_envelope,"estimatedTokensAt4BytesPerToken":sent.len().div_ceil(4)}));
        let scope = ToolScope {
            session_id: sid.clone(),
            section_id: doc.clone(),
            run_id: format!("bridge_run_{round}"),
            allowed_ids: ctx.manifest.iter().map(|m| m.id.clone()).collect(),
        };
        let handler_engine = e.clone();
        let handler: ToolHandler = Arc::new(move |call| {
            let e = handler_engine.clone();
            let scope = scope.clone();
            Box::pin(
                async move { learning_tools::execute(&e, &scope, &call.id, &call.name, call.args) },
            )
        });
        let key_path = dir
            .path()
            .join("state/automatic-context")
            .join(format!("{sid}.json"));
        let prior = std::fs::read(&key_path).ok();
        let result = generate_scoped(
            ModelCredentials::new(
                ModelSettings {
                    provider: "custom".into(),
                    protocol: PROTOCOL.into(),
                    base_url: endpoint.clone(),
                    model: "fixture-model".into(),
                    max_output_tokens: Some(8192),
                    ..ModelSettings::default()
                },
                "synthetic-bridge-not-real".into(),
            )
            .unwrap(),
            frozen.messages,
            Arc::new(AtomicBool::new(false)),
            |_| {},
            Some(handler),
            ToolPolicy {
                mode: "question".into(),
                allowed_tools: vec!["query_related_knowledge".into()],
                max_calls: 2,
            },
            crate::learning_skills::capture("question", &["query_related_knowledge".into()])
                .unwrap(),
            Some(RequestContext {
                time: frozen.time,
                exploration: None,
            }),
        )
        .await;
        if round == 3 {
            assert_eq!(result.unwrap_err().code, "MODEL_INCOMPLETE");
            assert_eq!(std::fs::read(&key_path).ok(), prior);
        } else {
            let text = result.unwrap();
            let mut c = e.lock().unwrap();
            let saved = sessions::append(
                &mut c.content,
                &sid,
                Entry {
                    id: String::new(),
                    kind: "assistant".into(),
                    content: text,
                    created_at: timestamp(),
                    selection: None,
                    related_ids: vec![],
                    data: Some(json!({"contextBridgeManifest":frozen.manifest})),
                },
                None,
                &format!("bridge_answer_{round}"),
            )
            .unwrap();
            projection
                .commit(&c.state_dir, &saved, &saved.entries.last().unwrap().id)
                .unwrap();
        }
    }
    let packets = server.join().unwrap();
    assert_eq!(packets.len(), 8);
    for (round, packet_index) in [0usize, 3, 4, 5, 6, 7].into_iter().enumerate() {
        sizes[round]["actualApiPayloadBytes"] =
            json!(serde_json::to_vec(&packets[packet_index]).unwrap().len());
    }
    // Within a live tool loop the original bridge user envelope is present exactly once.
    let first_user = &packets[0]["messages"][1];
    for packet in &packets[..3] {
        let matching = packet["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "user" && m["content"] == first_user["content"])
            .count();
        assert_eq!(matching, 1);
        assert_eq!(packet["max_tokens"], 8192);
    }
    let tools = packets[2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .collect::<Vec<_>>();
    assert_eq!(tools.len(), 2);
    for tool in tools {
        let t = tool["content"].as_str().unwrap();
        assert!(t.contains("Original public course statement"));
        assert!(!t.contains("unifiedDiff"));
        assert!(!t.contains("readcache"));
    }
    assert_eq!(sizes[0]["mode"], "full");
    assert_eq!(sizes[1]["mode"], "unchanged");
    assert_eq!(sizes[1]["newMaterialBytes"], 0);
    assert_eq!(sizes[2]["mode"], "diff");
    assert_eq!(sizes[3]["mode"], "diff");
    assert_eq!(sizes[4]["mode"], "diff");
    assert_eq!(sizes[5]["mode"], "full");
    if let Ok(path) = std::env::var("LEARNING_STATION_CONTEXT_EVIDENCE") {
        let path = Path::new(&path);
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("request-projections.json"),serde_json::to_vec_pretty(&json!({"syntheticOnly":true,"actualPinnedPiSdk":true,"apiPackets":packets,"requestProjections":projections})).unwrap()).unwrap();
        std::fs::write(path.join("byte-comparison.json"),serde_json::to_vec_pretty(&json!({"measurements":sizes,"tokenEstimate":"UTF-8 bytes/4 rounded up, illustrative only; not model tokenizer or service usage","baseline":"pre-change stateless request sent one current full material, not accumulated duplicate copies"})).unwrap()).unwrap();
    }
}
