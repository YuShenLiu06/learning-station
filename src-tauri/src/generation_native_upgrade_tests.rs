//! All storage, credentials and HTTP responses are synthetic. Each turn uses
//! generation::start -> permission-restricted Node -> the locked real Pi SDK.
use super::*;
use std::{io::{Read, Write}, net::TcpListener, process::Command, thread, time::{Duration, Instant}};

fn server(turns: usize) -> (String, std::sync::mpsc::Sender<()>, thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let (stop, stopped) = std::sync::mpsc::channel();
    let handle = thread::spawn(move || {
        let mut captured = vec![];
        for turn in 0..if turns == 1 { usize::MAX } else { turns } {
            // For unknown outcomes, listen until the second generation has
            // ended; any accidental model continuation is answered and counted.
            let deadline = Instant::now() + Duration::from_secs(45);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && turns == 1 && turn > 0 && stopped.try_recv().is_ok() => return captured,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                    Err(e) => panic!("localhost fixture accept: {e}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = vec![];
            let mut buffer = [0; 4096];
            while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).unwrap(); assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
            }
            let end = request.windows(4).position(|v| v == b"\r\n\r\n").unwrap() + 4;
            let length: usize = String::from_utf8_lossy(&request[..end]).lines().find_map(|line| {
                line.split_once(':').filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse().unwrap())
            }).unwrap();
            while request.len() < end + length {
                let count = socket.read(&mut buffer).unwrap(); assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
            }
            captured.push(serde_json::from_slice(&request[end..end + length]).unwrap());
            let answer = if turn == 0 { "UPGRADE_HISTORY_MARKER" } else { "fixture answer" };
            let body = format!("data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"index":0,"delta":{"role":"assistant","content":answer},"finish_reason":null}]}),
                json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}));
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        captured
    });
    (endpoint, stop, handle)
}

async fn turn_outcome(engine: &Arc<Engine>, session: &str, number: usize) -> Run {
    let queued = start(engine.clone(), GenerationRequest {
        focus: None,
        session_id: session.into(), document_id: None, mode: "question".into(),
        prompt: format!("Synthetic upgrade turn {number}"), selection: String::new(),
        request_id: format!("upgrade-request-{number}"), user_entry_id: None,
        injections: vec![], exercise_references: vec![], assistant_references: vec![],
    }, Arc::new(|_| {})).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let run = get(engine, &queued.id).unwrap();
        if !matches!(run.status.as_str(), "queued" | "running") {
            return run;
        }
        assert!(Instant::now() < deadline, "native turn deadline");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn turn(engine: &Arc<Engine>, session: &str, number: usize) -> Run {
    let run = turn_outcome(engine, session, number).await;
    assert_eq!(run.status, "succeeded", "{:?}", run.error);
    run
}

async fn upgrade(compacted: bool) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
    let (endpoint, stop, server) = server(3);
    engine.model.lock().await.use_local_fixture(endpoint);
    let (session, source, old_ids, legacy_ids, native_dir) = {
        let mut core = engine.lock().unwrap();
        let owner = crate::library_tree::create(&mut core.content, "course", "Owner", None, "", "upgrade-owner").unwrap();
        let folder = crate::library_tree::create(&mut core.content, "folder", "Folder", Some(&owner.id), "", "upgrade-folder").unwrap();
        let source = crate::library_tree::create(&mut core.content, "section", "Source", Some(&folder.id), "synthetic body", "upgrade-source").unwrap();
        let session = crate::course_sessions::create(&mut core, &owner.id, "upgrade-session").unwrap();
        let exercise = crate::exercises::create_manual(&mut core.content, &session.id, &source.id,
            crate::exercises::ManualExercise { question: "Synthetic?".into(), format: "exact".into(), answers: vec!["yes".into()], explanation: "fixture".into() }, "upgrade-exercise").unwrap();
        let old_ids = crate::course_sessions::history_sources(&core.content, &session, &[]).unwrap();
        let legacy_ids = vec![owner.id, folder.id, exercise.document.id];
        assert!(legacy_ids.iter().all(|id| old_ids.contains(id)));
        let native_dir = core.state_dir.join("pi-native").join(hash(&session.id));
        (session.id, source.id, old_ids, legacy_ids, native_dir)
    };
    let first = turn(&engine, &session, 1).await;
    assert!(legacy_ids.iter().all(|id| !first.read_scope_manifest().iter().any(|m| &m.id == id)));
    // Recreate the exact pre-M2 scope using Rust's retained history_sources
    // and the public locked SDK, without fabricating transcript IDs/messages.
    let script = r#"import {SessionManager} from '@earendil-works/pi-coding-agent';
const [directory, ids, compact] = process.argv.slice(1);
const manager = SessionManager.open(directory+'/transcript.jsonl', directory, directory);
manager.appendCustomEntry('learning-station-scope', {sourceIds: JSON.parse(ids)});
if(compact === 'true') {
 const kept = manager.appendCustomMessageEntry('fixture-kept', 'permitted marker', false, {});
 manager.appendCompaction('UPGRADE_SUMMARY_MARKER', kept, 1000);
}"#;
    let seeded = Command::new(model::node_path()).args(["--input-type=module", "-e", script])
        .arg(&native_dir).arg(serde_json::to_string(&old_ids).unwrap()).arg(compacted.to_string())
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../pi-sidecar")).output().unwrap();
    assert!(seeded.status.success(), "{}", String::from_utf8_lossy(&seeded.stderr));
    let old_transcript = fs::read(native_dir.join("transcript.jsonl")).unwrap();
    let second = turn(&engine, &session, 2).await;
    assert!(legacy_ids.iter().all(|id| !second.read_scope_manifest().iter().any(|m| &m.id == id)));
    {
        let mut core = engine.lock().unwrap();
        let preview = crate::trash::preview(&mut core.content, &source).unwrap();
        crate::trash::trash(&mut core.content, &source, &preview.version, "upgrade-revoke-source").unwrap();
    }
    turn(&engine, &session, 3).await;
    let _ = stop.send(());
    let requests = server.join().unwrap();
    let marker = if compacted { "UPGRADE_SUMMARY_MARKER" } else { "UPGRADE_HISTORY_MARKER" };
    assert!(requests[1]["messages"].to_string().contains(marker), "upgrade silently excluded authorized history: {}", requests[1]["messages"]);
    assert!(!requests[2]["messages"].to_string().contains(marker), "true revocation must exclude old history/summary");
    assert!(fs::read(native_dir.join("transcript.jsonl")).unwrap().starts_with(&old_transcript), "raw historical entries remain intact");
}

#[tokio::test]
async fn native_scope_upgrade_retains_legacy_history_and_revokes_real_source() { upgrade(false).await; }
#[tokio::test]
async fn native_scope_upgrade_retains_legacy_summary_and_revokes_real_source() { upgrade(true).await; }

async fn pending_read(cached: bool, unknown_run: Option<&str>) {
    let missing_run = unknown_run.is_some();
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        engine.save_ui_preferences(crate::app::UiPreferences {
            tool_authorization_tier: "read".into(), ..Default::default()
        }).unwrap();
        let (endpoint, stop, server) = server(if missing_run { 1 } else { 2 });
        engine.model.lock().await.use_local_fixture(endpoint);
        let (session, source, native) = {
            let mut core = engine.lock().unwrap();
            let course = crate::library_tree::create(&mut core.content, "course", "Owner", None, "", "pending-owner").unwrap();
            let source = crate::library_tree::create(&mut core.content, "section", "Source", Some(&course.id), "PENDING_PRIVATE_BODY", "pending-source").unwrap();
            let session = crate::course_sessions::create(&mut core, &course.id, "pending-session").unwrap();
            let native = core.state_dir.join("pi-native").join(hash(&session.id));
            (session.id, source.id, native)
        };
        let first = turn(&engine, &session, 1).await;
        let scope = crate::learning_tools::ToolScope { session_id: session.clone(), run_id: first.id.clone(),
            section_id: String::new(), allowed_ids: first.read_scope_manifest().iter().map(|m| m.id.clone()).collect() };
        let args = json!({"sourceId":source});
        let receipt = crate::authorized_tools::execute(&engine, &scope, "pending_read", "read", args.clone()).unwrap();
        let request = crate::learning_tools::operation_id(&engine, &scope, "pending_read", "read", &args).unwrap().0;
        let ledger = engine.lock().unwrap().state_dir.join("workspaces").join(&session)
            .join("receipts").join(format!("{request}.json"));
        let original_ledger = fs::read(&ledger).unwrap();
        let call_receipt = native.join(format!("tool-{}.json", hash(&json!([first.id,"pending_read"]).to_string())));
        if cached {
            crate::native_session::atomic(&call_receipt, &json!({"callId":"pending_read","name":"read",
                "argsHash":hash(&args.to_string()),"reply":{"result":receipt}})).unwrap();
        }
        let original_native = cached.then(|| fs::read(&call_receipt).unwrap());
        let script = r#"import {SessionManager} from '@earendil-works/pi-coding-agent';
const [directory, run, args] = process.argv.slice(1);
const manager = SessionManager.open(directory+'/transcript.jsonl', directory, directory);
manager.appendMessage({role:'assistant',content:[{type:'toolCall',id:'pending_read',name:'read',arguments:JSON.parse(args)}],api:'openai-completions',provider:'learning-custom',model:'offline-fixture',usage:{input:0,output:0},stopReason:'toolUse',timestamp:Date.now()});
manager.appendCustomEntry('learning-station-tool',{runId:run,callId:'pending_read'});"#;
        let seeded = Command::new(model::node_path()).args(["--input-type=module", "-e", script])
            .arg(&native).arg(&first.id).arg(args.to_string())
            .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../pi-sidecar")).output().unwrap();
        assert!(seeded.status.success(), "{}", String::from_utf8_lossy(&seeded.stderr));
        {
            let mut core = engine.lock().unwrap();
            let document = core.content.read(&source).unwrap();
            core.content.save(&source, &document.content.replace("PENDING_PRIVATE_BODY", "updated body"), &document.version).unwrap();
        }
        let transcript = fs::read(native.join("transcript.jsonl")).unwrap();
        if missing_run {
            engine.runs.lock().unwrap().remove(&first.id);
            let path = engine.lock().unwrap().state_dir.join("runs").join(format!("{}.json", first.id));
            if unknown_run == Some("missing") { fs::remove_file(path).unwrap(); }
            else { fs::write(path, b"{damaged-run-evidence").unwrap(); }
        }
        let outcome = turn_outcome(&engine, &session, 2).await;
        let _ = stop.send(());
    let requests = server.join().unwrap();
        assert_eq!(fs::read(&ledger).unwrap(), original_ledger);
        if let Some(bytes) = original_native { assert_eq!(fs::read(&call_receipt).unwrap(), bytes); }
        if missing_run {
            assert_eq!(outcome.error.as_ref().map(|error| error.code.as_str()), Some("NATIVE_TOOL_UNKNOWN"));
            assert_eq!(requests.len(), 1, "missing evidence must not call the model again");
            assert_eq!(fs::read(native.join("transcript.jsonl")).unwrap(), transcript,
                "unknown read must remain unpaired without fabricating a negative result");
        } else {
            assert_eq!(outcome.status, "succeeded", "{:?}", outcome.error);
            let messages = requests[1]["messages"].as_array().unwrap();
            assert!(messages.iter().any(|m| m["role"] == "tool" && m["content"].as_str().is_some_and(|text| text.contains("CONFLICT"))));
            assert!(!requests[1]["messages"].to_string().contains("PENDING_PRIVATE_BODY"));
        }
}

#[tokio::test]
async fn native_pending_read_denial_pairs_real_rust_reply_without_rewriting_receipts() {
    for cached in [false, true] { pending_read(cached, None).await; }
}

#[tokio::test]
async fn native_pending_read_missing_run_without_cache_stays_unknown() { pending_read(false, Some("missing")).await; }
#[tokio::test]
async fn native_pending_read_missing_run_with_cache_stays_unknown() { pending_read(true, Some("missing")).await; }
#[tokio::test]
async fn native_pending_read_corrupt_run_without_cache_stays_unknown() { pending_read(false, Some("corrupt")).await; }
#[tokio::test]
async fn native_pending_read_corrupt_run_with_cache_stays_unknown() { pending_read(true, Some("corrupt")).await; }

#[tokio::test]
async fn native_exploration_context_reaches_first_and_reopened_provider_requests_only() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("library");
    let state = dir.path().join("state");
    let mut engine = Engine::new(library.clone(), state.clone()).unwrap();
    let (launch, ordinary) = {
        let mut core = engine.lock().unwrap();
        let launch = crate::exploration::begin(
            &mut core.content, "EXPLORATION_ORIGINAL_QUESTION", "native-exploration-launch",
        ).unwrap();
        // A supported legacy course avoids external project resource discovery;
        // this regression tests scenario routing, not the resource loader.
        let owner = crate::library_tree::create(
            &mut core.content, "course", "Ordinary course", None, "", "native-ordinary-course",
        ).unwrap();
        crate::library_tree::create(
            &mut core.content, "section", "Unread note", Some(&owner.id),
            "ORDINARY_NOTE_MUST_NOT_AUTO_LOAD", "native-ordinary-note",
        ).unwrap();
        let ordinary = crate::course_sessions::create(
            &mut core, &owner.id, "native-ordinary-conversation",
        ).unwrap();
        (launch, ordinary)
    };
    let native = state.join("pi-native").join(hash(&launch.session.id)).join("transcript.jsonl");
    let (endpoint, stop, provider) = server(3);
    let mut first_transcript = vec![];
    let goals = ["EXPLORATION_ORIGINAL_QUESTION", "EXPLORATION_FOLLOWUP_GOAL", "ORDINARY_NEW_QUESTION"];
    let mut runs = vec![];
    for (round, goal) in goals.iter().enumerate() {
        if round > 0 {
            // Drop all in-memory runs/model state. Reopen the same business and
            // exact Pi session through production routing, not a test-built spec.
            drop(engine);
            engine = Engine::new(library.clone(), state.clone()).unwrap();
            engine.lock().unwrap().content.refresh().unwrap();
        }
        engine.model.lock().await.use_local_fixture(endpoint.clone());
        let session = if round == 2 { &ordinary.id } else { &launch.session.id };
        let queued = start(engine.clone(), GenerationRequest {
            focus: None, session_id: session.clone(), document_id: None,
            // Even a question request must not turn an ordinary course into exploration.
            mode: "question".into(), prompt: (*goal).into(), selection: String::new(),
            request_id: format!("native-exploration-turn-{round}"),
            user_entry_id: (round == 0).then(|| launch.user_entry_id.clone()),
            injections: vec![], exercise_references: vec![], assistant_references: vec![],
        }, Arc::new(|_| {})).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(45);
        let finished = loop {
            let run = get(&engine, &queued.id).unwrap();
            if !matches!(run.status.as_str(), "queued" | "running") { break run; }
            assert!(Instant::now() < deadline, "native exploration turn deadline");
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(finished.status, "succeeded", "{:?}", finished.error);
        assert!(finished.context_bridge_manifest.is_some(), "real Pi projection must return its inventory");
        if round == 0 {
            first_transcript = fs::read(&native).unwrap();
        } else if round == 1 {
            assert!(fs::read(&native).unwrap().starts_with(&first_transcript),
                "reopening must retain the exact native transcript");
        }
        runs.push(finished);
    }
    let _ = stop.send(());
    let requests = provider.join().unwrap();
    assert_eq!(requests.len(), 3);
    let text = |message: &Value| message["content"].as_str().map(str::to_owned)
        .unwrap_or_else(|| message["content"].as_array().unwrap().iter()
            .filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join(""));
    for (round, packet) in requests.iter().enumerate() {
        let messages = packet["messages"].as_array().unwrap();
        let system = text(messages.iter().find(|message| message["role"] == "system").unwrap());
        let turn = messages.iter().map(&text)
            .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(|value| value["type"] == "learning_request").last()
            .expect("provider must receive the actual native learning turn");
        if round < 2 {
            let expected = json!({
                "origin":"exploration-entry", "originalUserEntryId":launch.user_entry_id,
                "scenario":"question-led-exploration", "originalQuestion":goals[0],
                "userGoal":goals[round], "storageDocumentId":launch.document.id,
                "materialSourceIds":[],
            });
            assert_eq!(turn["explorationContext"], expected, "ContextBridge/native turn lost exploration context");
            let system_context = system.lines().rev().find_map(|line| serde_json::from_str::<Value>(line).ok())
                .expect("provider system prompt must carry the exploration object, not only turn metadata");
            assert_eq!(system_context, expected, "Rust/model/native system wiring lost the frozen exploration context");
            assert!(system.contains("从用户问题发起的新探索"));
            assert!(system.contains("不是教材缺失"));
            assert_eq!(serde_json::to_value(&runs[round].exploration_context).unwrap(), expected);
            assert!(!packet.to_string().contains("ORDINARY_NOTE_MUST_NOT_AUTO_LOAD"));
        } else {
            assert!(runs[round].exploration_context.is_none());
            assert!(turn["explorationContext"].is_null());
            assert!(!system.contains("从用户问题发起的新探索"));
            for private in [goals[0], goals[1], "UPGRADE_HISTORY_MARKER", "ORDINARY_NOTE_MUST_NOT_AUTO_LOAD"] {
                assert!(!packet.to_string().contains(private), "ordinary course inherited {private}");
            }
            assert!(runs[round].context_manifest.is_empty());
        }
    }
    assert!(requests[1]["messages"].as_array().unwrap().iter()
        .any(|message| message["role"] == "user" && text(message) == goals[0]),
        "restored request must retain the original real native user");
    assert!(requests[1]["messages"].to_string().contains("UPGRADE_HISTORY_MARKER"));
    let entries: Vec<Value> = String::from_utf8(fs::read(&native).unwrap()).unwrap()
        .lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(entries[0]["id"], hash(&launch.session.id));
    assert_eq!(entries.iter().filter(|entry| entry["type"] == "message" && entry["message"]["role"] == "user").count(), 2);
    assert!(entries.iter().any(|entry| entry["type"] == "custom_message"
        && entry["customType"] == "learning-station-turn"
        && entry["content"].as_str().unwrap_or("").contains(goals[1])));
}
