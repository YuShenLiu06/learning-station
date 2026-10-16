use crate::{
    content::{self, ContentStore},
    context,
    model::{ModelSettings, ModelStore},
    reminders::Ledger,
    sessions::{self, Preferences},
    types::*,
};
use serde::Serialize;
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

pub struct Engine {
    pub core: Mutex<Core>,
    pub model: tokio::sync::Mutex<ModelStore>,
    pub model_summary: RwLock<ModelSettings>,
    pub reminders: Ledger,
    pub drafts: crate::drafts::DraftStore,
    pub runs: Mutex<BTreeMap<String, crate::generation::ManagedRun>>,
}

impl Engine {
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
    pub fn save_preferences(&self) -> Result<()> {
        atomic_json(&self.state_dir.join("preferences.json"), &self.preferences)
    }
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
