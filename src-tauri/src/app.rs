use crate::{
    content::{self, ContentStore},
    context,
    model::{ModelSettings, ModelStore},
    reminders::Ledger,
    sessions::{self, Preferences},
    types::*,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, RwLock},
};

pub struct Core {
    pub content: ContentStore,
    pub preferences: Preferences,
    pub state_dir: PathBuf,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryState {
    pub root: String,
    pub documents: Vec<DocumentSummary>,
    pub active_document_id: Option<String>,
    pub active_session_id: Option<String>,
    pub last_run_id: Option<String>,
    pub model: ModelSettings,
    pub indexing: bool,
    pub feishu: Value,
    pub anki: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UiPreferences {
    pub font_size: u8,
    #[serde(default = "default_assistant_width")]
    pub assistant_width: u16,
}
impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            font_size: 16,
            assistant_width: default_assistant_width(),
        }
    }
}
fn default_assistant_width() -> u16 {
    346
}
static UI_PREFERENCE_WRITE: Mutex<()> = Mutex::new(());
impl UiPreferences {
    fn validate(&self) -> Result<()> {
        if !(12..=24).contains(&self.font_size) {
            return Err(CoreError::new(
                "INVALID_INPUT",
                "字号请选择12至24之间的整数。",
            ));
        }
        if !(280..=960).contains(&self.assistant_width) {
            return Err(CoreError::new(
                "INVALID_INPUT",
                "助教栏宽度请选择280至960之间的整数。",
            ));
        }
        Ok(())
    }
}

pub struct Engine {
    pub core: Mutex<Core>,
    pub model: tokio::sync::Mutex<ModelStore>,
    pub model_summary: RwLock<ModelSettings>,
    pub reminders: Ledger,
    pub drafts: crate::drafts::DraftStore,
    pub runs: Mutex<BTreeMap<String, crate::generation::ManagedRun>>,
}

impl Engine {
    pub async fn set_model(&self, model: &str) -> Result<crate::model::ModelSettings> {
        let mut store = self.model.lock().await;
        if self
            .runs
            .lock()
            .map_err(|_| CoreError::new("BUSY", "生成状态暂不可用。"))?
            .values()
            .any(|run| ["queued", "running"].contains(&run.run.status.as_str()))
        {
            return Err(CoreError::new(
                "BUSY",
                "生成中不能切换模型，请先停止并等待结束。",
            ));
        }
        let settings = store.set_model(model)?;
        *self
            .model_summary
            .write()
            .map_err(|_| CoreError::new("BUSY", "模型配置暂不可用。"))? = settings.clone();
        Ok(settings)
    }
    pub fn new(library_root: PathBuf, state_dir: PathBuf) -> Result<Arc<Self>> {
        fs::create_dir_all(&state_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700))?;
        }
        let preferences: Preferences = match fs::read_to_string(state_dir.join("preferences.json"))
        {
            Ok(text) => serde_json::from_str(&text).map_err(|_| {
                CoreError::new(
                    "INVALID_STATE",
                    "恢复信息无法读取，原Markdown仍保留。请先备份运行数据再修复。",
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Preferences::default(),
            Err(e) => return Err(CoreError::io(e)),
        };
        let mut content = ContentStore::new(library_root, state_dir.clone())?;
        // Reopen only the last known document and its bounded links; never scan on startup.
        if let Some(active) = &preferences.active_document_id {
            let mut pending = vec![active.clone()];
            let mut seen = std::collections::BTreeSet::new();
            while let Some(id) = pending.pop() {
                if seen.len() >= 32 || !seen.insert(id.clone()) {
                    continue;
                }
                if let Some(path) = preferences.document_paths.get(&id) {
                    if let Ok(doc) = content.register(path) {
                        pending.extend(doc.metadata.links);
                    }
                }
            }
        }
        let model = ModelStore::new(&state_dir)?;
        let settings = model.settings();
        let reminders = Ledger::new(&state_dir)?;
        let drafts = crate::drafts::DraftStore::new(&state_dir)?;
        Ok(Arc::new(Self {
            core: Mutex::new(Core {
                content,
                preferences,
                state_dir,
            }),
            model: tokio::sync::Mutex::new(model),
            model_summary: RwLock::new(settings),
            reminders,
            drafts,
            runs: Mutex::new(BTreeMap::new()),
        }))
    }
    pub fn lock(&self) -> Result<MutexGuard<'_, Core>> {
        self.core
            .lock()
            .map_err(|_| CoreError::new("BUSY", "学习文件暂时繁忙，请稍后重试。"))
    }
    pub fn ui_preferences(&self) -> Result<UiPreferences> {
        let path = self.lock()?.state_dir.join("ui-preferences.json");
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(UiPreferences::default())
            }
            Err(error) => Err(CoreError::io(error)),
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > 4096 => {
                Err(CoreError::new(
                    "INVALID_STATE",
                    "字号设置文件无效，请保留原文件后重试。",
                ))
            }
            Ok(_) => {
                let value: UiPreferences = serde_json::from_slice(&fs::read(path)?)?;
                value.validate()?;
                Ok(value)
            }
        }
    }
    pub fn save_ui_preferences(&self, value: UiPreferences) -> Result<UiPreferences> {
        let _guard = UI_PREFERENCE_WRITE
            .lock()
            .map_err(|_| CoreError::new("BUSY", "界面设置暂不可用。"))?;
        value.validate()?;
        let path = self.lock()?.state_dir.join("ui-preferences.json");
        atomic_json(&path, &value)?;
        Ok(value)
    }
    pub fn patch_ui_preferences(
        &self,
        font_size: Option<u8>,
        assistant_width: Option<u16>,
    ) -> Result<UiPreferences> {
        let _guard = UI_PREFERENCE_WRITE
            .lock()
            .map_err(|_| CoreError::new("BUSY", "界面设置暂不可用。"))?;
        let mut value = self.ui_preferences()?;
        if font_size.is_none() && assistant_width.is_none() {
            return Ok(value);
        }
        if let Some(size) = font_size {
            value.font_size = size;
        }
        if let Some(width) = assistant_width {
            value.assistant_width = width;
        }
        value.validate()?;
        let path = self.lock()?.state_dir.join("ui-preferences.json");
        atomic_json(&path, &value)?;
        Ok(value)
    }
    pub fn state(&self) -> Result<LibraryState> {
        let core = self.lock()?;
        let model = self
            .model_summary
            .read()
            .map_err(|_| CoreError::new("BUSY", "模型设置暂不可用。"))?
            .clone();
        Ok(LibraryState {
            root: core.content.root().to_string_lossy().into_owned(),
            documents: core.summaries(),
            active_document_id: core.preferences.active_document_id.clone(),
            active_session_id: core.preferences.active_session_id.clone(),
            last_run_id: core
                .preferences
                .active_session_id
                .as_ref()
                .and_then(|id| core.preferences.last_run_by_session.get(id))
                .cloned(),
            model,
            indexing: false,
            feishu: json!({"enabled":false,"status":"disabled","message":"飞书提醒未启用；应用退出或Mac休眠时不能准时发送。"}),
            anki: json!({"connected":false,"mode":"csv","message":"仅CSV导出，未同步Anki。"}),
        })
    }
}

impl Core {
    pub fn summaries(&self) -> Vec<DocumentSummary> {
        self.content
            .summaries()
            .into_iter()
            .filter(|d| d.kind == "section" || d.kind == "knowledge")
            .collect()
    }
    pub fn refresh(&mut self) -> Result<Vec<DocumentSummary>> {
        self.content.refresh()?;
        for doc in self.summaries() {
            self.preferences.document_paths.insert(doc.id, doc.path);
        }
        self.save_preferences()?;
        Ok(self.summaries())
    }
    pub fn open(&mut self, id: &str) -> Result<Document> {
        let doc = self.content.read(id)?;
        self.preferences.active_document_id = Some(id.into());
        self.preferences
            .document_paths
            .insert(doc.id.clone(), doc.path.clone());
        self.save_preferences()?;
        Ok(doc)
    }
    pub fn create_section(
        &mut self,
        title: &str,
        body: &str,
        goals: Vec<String>,
        tags: Vec<String>,
    ) -> Result<Document> {
        if title.trim().is_empty()
            || title.len() > 500
            || body.len() > 1_500_000
            || goals.len() > 30
            || tags.len() > 30
        {
            return Err(CoreError::new(
                "INVALID_INPUT",
                "请填写标题；标题最多500字节，材料最多1.5MB，目标与标签各最多30个。",
            ));
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let meta = Metadata {
            id: id.clone(),
            title: title.trim().into(),
            kind: "section".into(),
            goals,
            tags,
            links: vec![],
            version: 1,
            extra: BTreeMap::new(),
        };
        let doc = self
            .content
            .create(meta, body, &format!("courses/{id}.md"))?;
        self.preferences.active_document_id = Some(id);
        self.preferences
            .document_paths
            .insert(doc.id.clone(), doc.path.clone());
        self.save_preferences()?;
        Ok(doc)
    }
    pub fn import(&mut self, content: &str) -> Result<Document> {
        if content.len() > 1_500_000 {
            return Err(CoreError::new("LIMIT_EXCEEDED", "单份材料最多1.5MB。"));
        }
        let (meta, body) = content::parse(content)?;
        if meta.kind != "section" && meta.kind != "knowledge" {
            return Err(CoreError::new(
                "INVALID_INPUT",
                "只能导入课程小节或知识点；学习记录请放回原库。",
            ));
        }
        let id = meta.id.clone();
        let doc = self
            .content
            .create(meta, &body, &format!("courses/{id}.md"))?;
        self.preferences.active_document_id = Some(id);
        self.preferences
            .document_paths
            .insert(doc.id.clone(), doc.path.clone());
        self.save_preferences()?;
        Ok(doc)
    }
    pub fn start_session(
        &mut self,
        section_id: &str,
        new_session: bool,
    ) -> Result<(Session, Context)> {
        let context = context::load(&self.content, section_id, None)?;
        let session = if !new_session {
            self.preferences
                .session_by_section
                .get(section_id)
                .cloned()
                .map(|id| sessions::get(&mut self.content, &id))
                .transpose()?
        } else {
            None
        };
        let session = match session {
            Some(session) => session,
            None => sessions::create(&mut self.content, section_id, &context.manifest)?,
        };
        self.preferences.active_document_id = Some(section_id.into());
        self.preferences.active_session_id = Some(session.id.clone());
        self.preferences
            .session_by_section
            .insert(section_id.into(), session.id.clone());
        self.save_preferences()?;
        Ok((session, context))
    }
    pub fn start_exploration(
        &mut self,
        section_id: Option<&str>,
        associate: bool,
        fresh: bool,
    ) -> Result<(Session, Context)> {
        if associate {
            let id = section_id.filter(|id| !id.is_empty()).ok_or_else(|| {
                CoreError::new("INVALID_INPUT", "请先选择要关联的小节，或关闭关联。")
            })?;
            return self.start_session(id, fresh);
        }
        let previous = if fresh {
            None
        } else {
            self.preferences.exploration_session_id.clone()
        };
        let session = match previous {
            Some(id) => {
                let session = sessions::get(&mut self.content, &id)?;
                if !session.section_id.is_empty() {
                    return Err(CoreError::new(
                        "INVALID_STATE",
                        "独立探索恢复信息不匹配，未带入课程材料。",
                    ));
                }
                session
            }
            None => sessions::create_exploration(&mut self.content)?,
        };
        self.preferences.exploration_session_id = Some(session.id.clone());
        self.save_preferences()?;
        Ok((
            session,
            Context {
                text: String::new(),
                manifest: vec![],
                warnings: vec![],
                truncated: false,
            },
        ))
    }
    pub fn session_context(
        &mut self,
        session_id: &str,
        selection: Option<&str>,
    ) -> Result<(Session, Context)> {
        let session = sessions::get(&mut self.content, session_id)?;
        let context = if session.section_id.is_empty() {
            if selection.is_some_and(|text| !text.is_empty()) {
                return Err(CoreError::new(
                    "INVALID_INPUT",
                    "独立探索不带入当前小节选句，请清除选句或开启小节关联。",
                ));
            }
            Context {
                text: String::new(),
                manifest: vec![],
                warnings: vec![],
                truncated: false,
            }
        } else {
            context::load(&self.content, &session.section_id, selection)?
        };
        Ok((session, context))
    }
    pub fn save_preferences(&self) -> Result<()> {
        atomic_json(&self.state_dir.join("preferences.json"), &self.preferences)
    }
}

static METRIC_APPEND_LOCK: Mutex<()> = Mutex::new(());

/// Keep concurrent startup metrics as whole JSONL records, never formatter fragments.
pub fn append_metric(path: &std::path::Path, value: &Value) -> Result<bool> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    let _guard = METRIC_APPEND_LOCK
        .lock()
        .map_err(|_| CoreError::new("BUSY", "性能记录暂不可用。"))?;
    if fs::metadata(path).is_ok_and(|meta| meta.len() > 1_000_000) {
        return Ok(false);
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(&bytes)?;
    Ok(true)
}

pub fn atomic_json(path: &std::path::Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::new("IO", "运行数据路径无效。"))?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temp.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| CoreError::io(e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_metrics_append_complete_json_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("performance.jsonl");
        let threads: Vec<_> = (0..8)
            .map(|worker| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for sequence in 0..25 {
                        assert!(append_metric(
                            &path,
                            &json!({"worker":worker,"sequence":sequence})
                        )
                        .unwrap());
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let bytes = fs::read_to_string(path).unwrap();
        let records: std::collections::BTreeSet<_> = bytes
            .lines()
            .map(|line| {
                let value: Value = serde_json::from_str(line).unwrap();
                (
                    value["worker"].as_u64().unwrap(),
                    value["sequence"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(bytes.lines().count(), 200);
        assert_eq!(records.len(), 200);
    }
    #[test]
    fn open_session_recovers_after_engine_restart_and_course_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("library");
        let state = dir.path().join("runtime");
        let engine = Engine::new(lib.clone(), state.clone()).unwrap();
        let mut c = engine.lock().unwrap();
        let doc = c
            .create_section(
                "English",
                "She has studied for three years.",
                vec!["for/since".into()],
                vec![],
            )
            .unwrap();
        let (session, context) = c.start_session(&doc.id, false).unwrap();
        assert_eq!(context.manifest.len(), 1);
        sessions::append(
            &mut c.content,
            &session.id,
            Entry {
                id: String::new(),
                kind: "note".into(),
                content: "为什么用for？".into(),
                created_at: String::new(),
                selection: None,
                related_ids: vec![],
                data: None,
            },
            Some(&session.version),
            "real-note-request",
        )
        .unwrap();
        assert_eq!(c.content.read(&doc.id).unwrap().content, doc.content);
        drop(c);
        drop(engine);
        let engine = Engine::new(lib, state).unwrap();
        let mut c = engine.lock().unwrap();
        c.refresh().unwrap();
        let (resumed, _) = c.start_session(&doc.id, false).unwrap();
        assert_eq!(resumed.id, session.id);
        assert_eq!(resumed.entries.len(), 1);
    }
}

#[cfg(test)]
mod iteration_tests {
    use super::*;

    #[test]
    fn independent_exploration_has_no_course_context_and_resumes_separately() {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("library");
        let state = dir.path().join("state");
        let engine = Engine::new(library.clone(), state.clone()).unwrap();
        let (course_id, linked_id, free_id) = {
            let mut core = engine.lock().unwrap();
            let course = core
                .create_section("English", "PRIVATE_COURSE_MATERIAL", vec![], vec![])
                .unwrap();
            let (linked, _) = core.start_session(&course.id, false).unwrap();
            let (free, context) = core
                .start_exploration(Some(&course.id), false, false)
                .unwrap();
            assert!(free.section_id.is_empty());
            assert!(context.text.is_empty());
            assert!(context.manifest.is_empty());
            assert_eq!(
                core.preferences.active_document_id.as_deref(),
                Some(course.id.as_str())
            );
            assert_eq!(
                core.preferences.active_session_id.as_deref(),
                Some(linked.id.as_str())
            );
            sessions::append(
                &mut core.content,
                &free.id,
                Entry {
                    id: String::new(),
                    kind: "user".into(),
                    content: "独立探索的未解问题".into(),
                    created_at: String::new(),
                    selection: None,
                    related_ids: vec![],
                    data: None,
                },
                Some(&free.version),
                "free-exploration-question",
            )
            .unwrap();
            assert_eq!(
                core.session_context(&free.id, Some("OLD_SELECTION"))
                    .unwrap_err()
                    .code,
                "INVALID_INPUT"
            );
            assert!(core
                .session_context(&free.id, None)
                .unwrap()
                .1
                .manifest
                .is_empty());
            (course.id, linked.id, free.id)
        };
        drop(engine);
        let engine = Engine::new(library, state).unwrap();
        let mut core = engine.lock().unwrap();
        let (free, context) = core
            .start_exploration(Some(&course_id), false, false)
            .unwrap();
        assert_eq!(free.id, free_id);
        assert_eq!(free.entries.len(), 1);
        assert!(context.text.is_empty());
        assert_eq!(
            core.start_session(&course_id, false).unwrap().0.id,
            linked_id
        );
    }

    #[test]
    fn actual_session_manifest_tracks_versions_and_unlinked_rejects_sources() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut core = engine.lock().unwrap();
        let document = core
            .create_section("English", "Original", vec![], vec![])
            .unwrap();
        let (session, _) = core.start_session(&document.id, false).unwrap();
        let content = crate::content::serialize(&document.metadata, "Actually changed").unwrap();
        let document = core
            .content
            .save(&document.id, &content, &document.version)
            .unwrap();
        let (_, context) = core.session_context(&session.id, None).unwrap();
        assert_eq!(context.manifest[0].version, document.version);
        let updated =
            sessions::set_context_manifest(&mut core.content, &session.id, &context.manifest)
                .unwrap();
        assert_eq!(updated.context_manifest[0].version, document.version);
        let free = core.start_exploration(None, false, false).unwrap().0;
        assert!(
            sessions::set_context_manifest(&mut core.content, &free.id, &context.manifest).is_err()
        );
        let mut doc = core.content.read(&free.id).unwrap();
        doc.metadata.links.push(document.id);
        let bytes = crate::content::serialize(&doc.metadata, &doc.body).unwrap();
        core.content.save(&free.id, &bytes, &doc.version).unwrap();
        assert_eq!(
            sessions::get(&mut core.content, &free.id).unwrap_err().code,
            "INVALID_DOCUMENT"
        );
    }

    #[test]
    fn font_preferences_persist_and_invalid_size_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("library");
        let state = dir.path().join("state");
        let engine = Engine::new(lib.clone(), state.clone()).unwrap();
        assert_eq!(engine.ui_preferences().unwrap().font_size, 16);
        engine
            .save_ui_preferences(UiPreferences {
                font_size: 20,
                ..UiPreferences::default()
            })
            .unwrap();
        let old = fs::read(state.join("ui-preferences.json")).unwrap();
        assert_eq!(
            engine
                .save_ui_preferences(UiPreferences {
                    font_size: 11,
                    ..UiPreferences::default()
                })
                .unwrap_err()
                .code,
            "INVALID_INPUT"
        );
        assert_eq!(fs::read(state.join("ui-preferences.json")).unwrap(), old);
        drop(engine);
        assert_eq!(
            Engine::new(lib, state)
                .unwrap()
                .ui_preferences()
                .unwrap()
                .font_size,
            20
        );
    }
}

#[cfg(test)]
mod width_preference_tests {
    use super::*;
    #[test]
    fn old_font_settings_and_concurrent_width_patches_preserve_each_other() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let state = temp.path().join("state");
        let engine = Engine::new(library.clone(), state.clone()).unwrap();
        fs::write(state.join("ui-preferences.json"), r#"{"fontSize":18}"#).unwrap();
        assert_eq!(engine.ui_preferences().unwrap().assistant_width, 346);
        let a = engine.clone();
        let b = engine.clone();
        let t1 = std::thread::spawn(move || a.patch_ui_preferences(Some(20), None).unwrap());
        let t2 = std::thread::spawn(move || b.patch_ui_preferences(None, Some(600)).unwrap());
        t1.join().unwrap();
        t2.join().unwrap();
        assert_eq!(
            engine.ui_preferences().unwrap(),
            UiPreferences {
                font_size: 20,
                assistant_width: 600
            }
        );
        let old = fs::read(state.join("ui-preferences.json")).unwrap();
        assert!(engine.patch_ui_preferences(None, Some(279)).is_err());
        assert!(engine.patch_ui_preferences(None, Some(961)).is_err());
        assert_eq!(fs::read(state.join("ui-preferences.json")).unwrap(), old);
        drop(engine);
        assert_eq!(
            Engine::new(library, state)
                .unwrap()
                .ui_preferences()
                .unwrap(),
            UiPreferences {
                font_size: 20,
                assistant_width: 600
            }
        );
    }
}
