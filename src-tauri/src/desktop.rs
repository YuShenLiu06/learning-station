use crate::{
    app::{Engine, LibraryState, UiPreferences},
    context,
    generation::{self, GenerationRequest, Run, Sink},
    model::{ModelSettings, ModelTest},
    reminders::{Notification, Todo},
    sessions,
    types::*,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{Emitter, Manager, State};

async fn work<T: Send + 'static>(
    engine: Arc<Engine>,
    f: impl FnOnce(&Engine) -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(move || f(&engine))
        .await
        .map_err(|_| CoreError::new("INTERNAL", "本次操作中断，输入仍保留，请重试。"))?
}
#[tauri::command]
async fn get_app_state(state: State<'_, Arc<Engine>>) -> Result<LibraryState> {
    let e = state.inner().clone();
    work(e, |e| e.state()).await
}
#[tauri::command]
async fn refresh_library(
    app: tauri::AppHandle,
    state: State<'_, Arc<Engine>>,
) -> Result<Vec<DocumentSummary>> {
    let docs = work(state.inner().clone(), |e| e.lock()?.refresh()).await?;
    let _ = app.emit("library_changed", json!({"documents":docs}));
    Ok(docs)
}
#[tauri::command]
async fn create_section(
    state: State<'_, Arc<Engine>>,
    title: String,
    body: String,
    goals: Option<Vec<String>>,
    tags: Option<Vec<String>>,
) -> Result<Document> {
    work(state.inner().clone(), move |e| {
        e.lock()?.create_section(
            &title,
            &body,
            goals.unwrap_or_default(),
            tags.unwrap_or_default(),
        )
    })
    .await
}
#[tauri::command]
async fn import_markdown(
    state: State<'_, Arc<Engine>>,
    content: String,
    filename: Option<String>,
) -> Result<Document> {
    let _ = filename;
    work(state.inner().clone(), move |e| e.lock()?.import(&content)).await
}
#[tauri::command]
async fn open_document(state: State<'_, Arc<Engine>>, id: String) -> Result<Document> {
    work(state.inner().clone(), move |e| e.lock()?.open(&id)).await
}
#[tauri::command]
async fn save_document(
    state: State<'_, Arc<Engine>>,
    id: String,
    content: String,
    expected_version: String,
) -> Result<Document> {
    work(state.inner().clone(), move |e| {
        e.lock()?.content.save(&id, &content, &expected_version)
    })
    .await
}
#[tauri::command]
async fn start_session(
    state: State<'_, Arc<Engine>>,
    section_id: String,
    new_session: Option<bool>,
) -> Result<Value> {
    work(state.inner().clone(), move |e| {
        let (session, context) = e
            .lock()?
            .start_session(&section_id, new_session.unwrap_or(false))?;
        Ok(json!({"session":session,"context":context}))
    })
    .await
}
#[tauri::command]
async fn start_exploration(
    state: State<'_, Arc<Engine>>,
    section_id: Option<String>,
    associate_current_section: bool,
    fresh: Option<bool>,
) -> Result<Value> {
    work(state.inner().clone(), move |e| {
        let mut core = e.lock()?;
        let (session, context) = core.start_exploration(
            section_id.as_deref(),
            associate_current_section,
            fresh.unwrap_or(false),
        )?;
        let last_run_id = core
            .preferences
            .last_run_by_session
            .get(&session.id)
            .cloned();
        Ok(json!({"session":session,"context":context,"lastRunId":last_run_id}))
    })
    .await
}
#[tauri::command]
async fn load_session_context(
    state: State<'_, Arc<Engine>>,
    session_id: String,
    selection: Option<String>,
) -> Result<Context> {
    work(state.inner().clone(), move |e| {
        Ok(e.lock()?
            .session_context(&session_id, selection.as_deref())?
            .1)
    })
    .await
}
#[tauri::command]
async fn get_ui_preferences(state: State<'_, Arc<Engine>>) -> Result<UiPreferences> {
    work(state.inner().clone(), |e| e.ui_preferences()).await
}
#[tauri::command]
async fn save_ui_preferences(
    state: State<'_, Arc<Engine>>,
    font_size: Option<u8>,
    assistant_width: Option<u16>,
) -> Result<UiPreferences> {
    work(state.inner().clone(), move |e| {
        e.patch_ui_preferences(font_size, assistant_width)
    })
    .await
}
#[tauri::command]
async fn get_session(state: State<'_, Arc<Engine>>, id: String) -> Result<Session> {
    work(state.inner().clone(), move |e| {
        sessions::get(&mut e.lock()?.content, &id)
    })
    .await
}
#[tauri::command]
async fn append_entry(
    state: State<'_, Arc<Engine>>,
    session_id: String,
    entry: Entry,
    expected_version: String,
    request_id: String,
) -> Result<Session> {
    work(state.inner().clone(), move |e| {
        sessions::append(
            &mut e.lock()?.content,
            &session_id,
            entry,
            Some(&expected_version),
            &request_id,
        )
    })
    .await
}
#[tauri::command]
async fn load_context(
    state: State<'_, Arc<Engine>>,
    section_id: String,
    selection: Option<String>,
) -> Result<Context> {
    work(state.inner().clone(), move |e| {
        context::load(&e.lock()?.content, &section_id, selection.as_deref())
    })
    .await
}
#[tauri::command]
async fn start_generation(
    app: tauri::AppHandle,
    state: State<'_, Arc<Engine>>,
    session_id: String,
    mode: String,
    selection: String,
    prompt: String,
    request_id: String,
    user_entry_id: Option<String>,
) -> Result<Value> {
    let sink: Sink = Arc::new(move |event| {
        let _ = app.emit("generation", event);
    });
    let run = generation::start(
        state.inner().clone(),
        GenerationRequest {
            session_id,
            mode,
            selection,
            prompt,
            request_id,
            user_entry_id,
        },
        sink,
    )
    .await?;
    Ok(json!({"runId":run.id}))
}
#[tauri::command]
async fn cancel_generation(state: State<'_, Arc<Engine>>, run_id: String) -> Result<Value> {
    let run = generation::cancel(state.inner(), &run_id)?;
    Ok(json!({"status":run.status}))
}
#[tauri::command]
async fn get_run(state: State<'_, Arc<Engine>>, run_id: String) -> Result<Run> {
    let e = state.inner().clone();
    work(e, move |e| generation::get(e, &run_id)).await
}
#[tauri::command]
async fn get_model_settings(state: State<'_, Arc<Engine>>) -> Result<ModelSettings> {
    Ok(state
        .model_summary
        .read()
        .map_err(|_| CoreError::new("BUSY", "模型配置暂不可用。"))?
        .clone())
}
#[tauri::command]
async fn configure_model(
    state: State<'_, Arc<Engine>>,
    provider: String,
    protocol: Option<String>,
    base_url: String,
    model: String,
    api_key: String,
) -> Result<ModelSettings> {
    let mut store = state.model.lock().await;
    let result = store.configure_protocol(
        &provider,
        protocol.as_deref().unwrap_or("openai-completions"),
        &base_url,
        &model,
        &api_key,
    )?;
    *state
        .model_summary
        .write()
        .map_err(|_| CoreError::new("BUSY", "模型配置暂不可用。"))? = result.clone();
    Ok(result)
}
#[tauri::command]
async fn set_model(state: State<'_, Arc<Engine>>, model: String) -> Result<ModelSettings> {
    state.set_model(&model).await
}
#[tauri::command]
async fn test_model(state: State<'_, Arc<Engine>>) -> Result<ModelTest> {
    let mut store = state.model.lock().await;
    let result = store.test().await;
    *state
        .model_summary
        .write()
        .map_err(|_| CoreError::new("BUSY", "模型配置暂不可用。"))? = store.settings();
    result
}
#[tauri::command]
async fn check_feishu_status() -> crate::feishu::IdentityStatus {
    crate::feishu::check_status().await
}
#[tauri::command]
async fn test_feishu_notification(
    state: State<'_, Arc<Engine>>,
    request_id: String,
) -> Result<crate::feishu::DeliveryOutcome> {
    let state_dir = state.lock()?.state_dir.clone();
    crate::feishu::test_notification(&state_dir, &request_id).await
}
#[tauri::command]
async fn create_todo(
    state: State<'_, Arc<Engine>>,
    title: String,
    due_at: Option<String>,
    reminder_enabled: Option<bool>,
    session_id: Option<String>,
    request_id: String,
) -> Result<Todo> {
    work(state.inner().clone(), move |e| {
        if let Some(session_id) = &session_id {
            sessions::get(&mut e.lock()?.content, session_id)?;
        }
        e.reminders.create_todo(
            &title,
            due_at.as_deref(),
            reminder_enabled.unwrap_or(false),
            session_id.as_deref(),
            &request_id,
        )
    })
    .await
}
#[tauri::command]
async fn list_todos(state: State<'_, Arc<Engine>>) -> Result<Vec<Todo>> {
    work(state.inner().clone(), |e| e.reminders.list_todos()).await
}
#[tauri::command]
async fn set_todo_done(state: State<'_, Arc<Engine>>, id: String, done: bool) -> Result<Todo> {
    work(state.inner().clone(), move |e| {
        e.reminders.set_done(&id, done)
    })
    .await
}
#[tauri::command]
async fn list_notifications(state: State<'_, Arc<Engine>>) -> Result<Vec<Notification>> {
    work(state.inner().clone(), |e| e.reminders.list_notifications()).await
}
#[tauri::command]
async fn mark_notification_read(state: State<'_, Arc<Engine>>, id: String) -> Result<Notification> {
    work(state.inner().clone(), move |e| e.reminders.mark_read(&id)).await
}
#[tauri::command]
async fn export_anki_csv(state: State<'_, Arc<Engine>>, session_id: String) -> Result<Value> {
    work(state.inner().clone(), move |e| {
        let mut core = e.lock()?;
        let destination = core.content.root().join("exports");
        let (path, count) = sessions::export_csv(&mut core.content, &session_id, &destination)?;
        Ok(json!({"path":path,"count":count,"synced":false}))
    })
    .await
}
#[tauri::command]
async fn list_review_items(
    state: State<'_, Arc<Engine>>,
    query: Option<String>,
) -> Result<Vec<Value>> {
    work(state.inner().clone(), move |e| {
        sessions::review_items(&mut e.lock()?.content, query.as_deref().unwrap_or(""))
    })
    .await
}

#[tauri::command]
async fn get_ui_draft(
    state: State<'_, Arc<Engine>>,
    key: String,
) -> Result<Option<crate::drafts::UiDraft>> {
    work(state.inner().clone(), move |e| e.drafts.get(&key)).await
}
#[tauri::command]
async fn save_ui_draft(
    state: State<'_, Arc<Engine>>,
    key: String,
    data: crate::drafts::DraftData,
    expected_version: Option<String>,
) -> Result<crate::drafts::UiDraft> {
    work(state.inner().clone(), move |e| {
        e.drafts.save(&key, data, expected_version.as_deref())
    })
    .await
}
#[tauri::command]
async fn set_todo_reminder(
    state: State<'_, Arc<Engine>>,
    id: String,
    reminder_enabled: bool,
) -> Result<Todo> {
    work(state.inner().clone(), move |e| {
        e.reminders.set_reminder_enabled(&id, reminder_enabled)
    })
    .await
}

static RUN_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
#[tauri::command]
async fn record_ui_metric(
    state: State<'_, Arc<Engine>>,
    name: String,
    value_ms: f64,
) -> Result<Value> {
    if ![
        "shell-interactive",
        "section-ready",
        "editor-ready",
        "page-switch",
        "ime-commit",
        "stream-input",
    ]
    .contains(&name.as_str())
        || !value_ms.is_finite()
        || !(0.0..=1_000_000.0).contains(&value_ms)
    {
        return Err(CoreError::new("INVALID_INPUT", "性能记录无效。"));
    }
    let elapsed = RUN_START
        .get()
        .map(|t| t.elapsed().as_secs_f64() * 1000.0)
        .unwrap_or_default();
    work(state.inner().clone(), move |e| {
        let path = e.lock()?.state_dir.join("performance.jsonl");
        let recorded = crate::app::append_metric(&path, &json!({"name":name,"valueMs":value_ms,"mainToMetricMs":elapsed,"timestamp":timestamp(),"version":env!("CARGO_PKG_VERSION"),"arch":std::env::consts::ARCH}))?;
        Ok(json!({"recorded":recorded}))
    }).await
}

pub fn run() {
    let _ = RUN_START.set(std::time::Instant::now());
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(not(debug_assertions))]
            crate::model::set_sidecar_dir(app.path().resource_dir()?.join("pi-sidecar"))?;
            let home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .ok_or("无法定位本机用户目录")?;
            let library = std::env::var_os("LEARNING_STATION_LIBRARY")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join("opt/learning-station/library"));
            let state_dir = std::env::var_os("LEARNING_STATION_STATE")
                .map(std::path::PathBuf::from)
                .unwrap_or(app.path().app_data_dir()?);
            let engine = Engine::new(library, state_dir)?;
            app.manage(engine.clone());
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // The UI explicitly refreshes after rendering its cached bootstrap.
                // Do not race that first read by holding the core lock for a startup scan.
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
                loop {
                    interval.tick().await;
                    let clone = engine.clone();
                    if let Ok(notifications) = work(clone, |e| e.reminders.tick()).await {
                        for notification in notifications {
                            let _ = handle.emit("notification", notification);
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_state,
            refresh_library,
            create_section,
            import_markdown,
            open_document,
            save_document,
            start_session,
            start_exploration,
            load_session_context,
            get_ui_preferences,
            save_ui_preferences,
            get_session,
            append_entry,
            load_context,
            start_generation,
            cancel_generation,
            get_run,
            get_model_settings,
            configure_model,
            test_model,
            check_feishu_status,
            test_feishu_notification,
            set_model,
            create_todo,
            list_todos,
            set_todo_done,
            list_notifications,
            mark_notification_read,
            export_anki_csv,
            list_review_items,
            get_ui_draft,
            save_ui_draft,
            set_todo_reminder,
            record_ui_metric
        ])
        .run(tauri::generate_context!())
        .expect("学习站启动失败，请保留Markdown数据并检查本机权限。");
}
