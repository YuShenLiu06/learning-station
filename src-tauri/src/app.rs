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
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: u16,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub expanded_node_ids: Vec<String>,
}
impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            font_size: 16,
            assistant_width: default_assistant_width(),
            sidebar_width: default_sidebar_width(),
            sidebar_collapsed: false,
            expanded_node_ids: vec![],
        }
    }
}
fn default_assistant_width() -> u16 {
    346
}
fn default_sidebar_width() -> u16 {
    240
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
        if !(180..=480).contains(&self.sidebar_width) {
            return Err(CoreError::new(
                "INVALID_INPUT",
                "目录栏宽度请选择180至480之间的整数。",
            ));
        }
        if self.expanded_node_ids.len() > 4096
            || self.expanded_node_ids.iter().any(|id| !valid_id(id))
        {
            return Err(CoreError::new(
                "INVALID_INPUT",
                "目录展开状态包含无效编号或过多项目。",
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
    pub bot: crate::feishu_bot::BotManager,
    pub grades: Mutex<BTreeMap<String, crate::grading::ManagedGrade>>,
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
        let bot = crate::feishu_bot::BotManager::new(state_dir.clone());
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
            bot,
            grades: Mutex::new(BTreeMap::new()),
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
            Ok(meta)
                if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > 600 * 1024 =>
            {
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
        self.patch_all_ui_preferences(font_size, assistant_width, None, None, None)
    }
    pub fn patch_all_ui_preferences(
        &self,
        font_size: Option<u8>,
        assistant_width: Option<u16>,
        sidebar_collapsed: Option<bool>,
        expanded_node_ids: Option<Vec<String>>,
        sidebar_width: Option<u16>,
    ) -> Result<UiPreferences> {
        let _guard = UI_PREFERENCE_WRITE
            .lock()
            .map_err(|_| CoreError::new("BUSY", "界面设置暂不可用。"))?;
        let mut value = self.ui_preferences()?;
        if font_size.is_none()
            && assistant_width.is_none()
            && sidebar_collapsed.is_none()
            && expanded_node_ids.is_none()
            && sidebar_width.is_none()
        {
            return Ok(value);
        }
        if let Some(size) = font_size {
            value.font_size = size;
        }
        if let Some(width) = assistant_width {
            value.assistant_width = width;
        }
        if let Some(width) = sidebar_width {
            value.sidebar_width = width;
        }
        if let Some(collapsed) = sidebar_collapsed {
            value.sidebar_collapsed = collapsed;
        }
        if let Some(ids) = expanded_node_ids {
            value.expanded_node_ids = ids;
        }
        value.validate()?;
        let path = self.lock()?.state_dir.join("ui-preferences.json");
        atomic_json(&path, &value)?;
        Ok(value)
    }
    pub fn state(&self) -> Result<LibraryState> {
        let mut core = self.lock()?;
        core.content.sync_trash_visibility()?;
        let model = self
            .model_summary
            .read()
            .map_err(|_| CoreError::new("BUSY", "模型设置暂不可用。"))?
            .clone();
        Ok(LibraryState {
            root: core.content.root().to_string_lossy().into_owned(),
            documents: core.summaries(),
            active_document_id: core
                .preferences
                .active_document_id
                .clone()
                .filter(|id| !core.content.is_trashed(id)),
            active_session_id: core
                .preferences
                .active_session_id
                .clone()
                .filter(|id| !core.content.is_trashed(id)),
            last_run_id: core
                .preferences
                .active_session_id
                .as_ref()
                .filter(|id| !core.content.is_trashed(id))
                .and_then(|id| core.preferences.last_run_by_session.get(id))
                .cloned(),
            model,
            indexing: false,
            feishu: self.bot.settings().map(|mut v|{let snapshot=self.bot.snapshot().ok();v["enabled"]=v["deliveryEnabled"].clone();v["status"]=json!(snapshot.as_ref().map(|s|s.state.as_str()).unwrap_or("failed"));v["message"]=json!(snapshot.as_ref().map(|s|s.reason.as_str()).unwrap_or("机器人状态读取失败；学习记录仍可使用。"));v}).unwrap_or_else(|_|json!({"enabled":false,"status":"failed","message":"机器人配置读取失败，未连接；本机学习功能可继续使用。"})),
            anki: json!({"connected":false,"mode":"anki","code":"UNCHECKED","message":"请在复习页检查本机Anki连接；CSV仅为备用导出。","decks":[]}),
        })
    }
}

impl Core {
    pub fn summaries(&self) -> Vec<DocumentSummary> {
        self.content
            .summaries()
            .into_iter()
            .filter(|d| {
                matches!(
                    d.kind.as_str(),
                    "course" | "folder" | "section" | "knowledge" | "exploration" | "exercise"
                )
            })
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
        self.import_at(content, None, None)
    }
    /// Import retains authored stable IDs/links and commits the parent in the same Markdown write.
    pub fn import_at(
        &mut self,
        content: &str,
        parent_id: Option<&str>,
        request_id: Option<&str>,
    ) -> Result<Document> {
        if content.len() > 1_500_000 {
            return Err(CoreError::new("LIMIT_EXCEEDED", "单份材料最多1.5MB。"));
        }
        let (mut meta, body) = content::parse(content)?;
        if !matches!(meta.kind.as_str(), "section" | "knowledge" | "exploration") {
            return Err(CoreError::new(
                "INVALID_INPUT",
                "只能导入学习材料或探索正文；练习与学习记录请保留原库归属。",
            ));
        }
        let id = meta.id.clone();
        if let Some(parent) = parent_id {
            meta.extra.insert("parentId".into(), json!(parent));
        }
        let parent = match meta.extra.get("parentId") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) if valid_id(value) => Some(value.clone()),
            _ => return Err(CoreError::new("INVALID_INPUT", "父节点编号无效。")),
        };
        if let Some(parent) = &parent {
            crate::library_tree::course_id(&self.content, parent)?;
            let mut next = Some(parent.clone());
            let mut depth = 1;
            while let Some(current) = next {
                if current == id {
                    return Err(CoreError::new(
                        "INVALID_HIERARCHY",
                        "导入文档不能把自身作为祖先。",
                    ));
                }
                depth += 1;
                if depth > 12 {
                    return Err(CoreError::new("LIMIT_EXCEEDED", "课程层级最多12层。"));
                }
                let doc = self.content.read(&current)?;
                if doc.metadata.kind == "exercise" {
                    return Err(CoreError::new("INVALID_HIERARCHY", "练习不能容纳子文档。"));
                }
                next = doc
                    .metadata
                    .extra
                    .get("parentId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
        }
        let digest = hash(&serde_json::to_string(
            &json!({"content":content,"parentId":parent_id}),
        )?);
        if let Some(request) = request_id {
            if request.len() < 8 || !valid_id(request) {
                return Err(CoreError::new(
                    "INVALID_INPUT",
                    "导入请求缺少有效防重复编号。",
                ));
            }
            let dir = self.state_dir.join("import-receipts");
            match fs::symlink_metadata(&dir) {
                Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                    return Err(CoreError::new("UNSAFE_PATH", "导入回执目录不可安全使用。"))
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&dir)?;
                }
                Err(e) => return Err(CoreError::io(e)),
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
            }
            let path = dir.join(format!("{}.json", hash(request)));
            let receipt = json!({"documentId":id,"digest":digest});
            if !path.exists() {
                let mut temp = tempfile::NamedTempFile::new_in(&dir)?;
                temp.write_all(&serde_json::to_vec(&receipt)?)?;
                temp.as_file().sync_all()?;
                match temp.persist_noclobber(&path) {
                    Ok(_) => {}
                    Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(CoreError::io(error.error)),
                }
            }
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 4096 {
                return Err(CoreError::new(
                    "INVALID_STATE",
                    "导入回执不可安全读取，未覆盖原文件。",
                ));
            }
            let saved: Value = serde_json::from_slice(&fs::read(&path)?)?;
            if saved != receipt {
                return Err(CoreError::new(
                    "CONFLICT",
                    "该导入请求编号已用于其他材料，原文件未更改。",
                ));
            }
            meta.extra.insert("importRequestId".into(), json!(request));
            meta.extra.insert("importDigest".into(), json!(digest));
            match self.content.read(&id) {
                Ok(doc)
                    if doc
                        .metadata
                        .extra
                        .get("importRequestId")
                        .and_then(Value::as_str)
                        == Some(request)
                        && doc
                            .metadata
                            .extra
                            .get("importDigest")
                            .and_then(Value::as_str)
                            == Some(&digest) =>
                {
                    return Ok(doc)
                }
                Ok(_) => {
                    return Err(CoreError::new(
                        "ALREADY_EXISTS",
                        "相同文档ID已存在，未覆盖原材料。",
                    ))
                }
                Err(error) if error.code == "NOT_FOUND" => {}
                Err(error) => return Err(error),
            }
        }
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
    pub fn set_document_links(
        &mut self,
        id: &str,
        links: Vec<String>,
        expected_version: &str,
    ) -> Result<Document> {
        let mut doc = self.content.read(id)?;
        if !matches!(
            doc.metadata.kind.as_str(),
            "section" | "knowledge" | "exploration"
        ) {
            return Err(CoreError::new(
                "INVALID_INPUT",
                "只能为学习材料或探索正文设置关联资料。",
            ));
        }
        let unique = links.iter().collect::<std::collections::BTreeSet<_>>();
        if links.len() > 16
            || unique.len() != links.len()
            || links.iter().any(|link| !valid_id(link) || link == id)
        {
            return Err(CoreError::new(
                "INVALID_INPUT",
                "请显式选择最多16份不重复的其他学习材料。",
            ));
        }
        for link in &links {
            let source = self.content.read(link)?;
            if !matches!(
                source.metadata.kind.as_str(),
                "section" | "knowledge" | "exploration"
            ) {
                return Err(CoreError::new(
                    "INVALID_INPUT",
                    "不能把练习答案、目录或系统记录设为自动学习资料。",
                ));
            }
        }
        doc.metadata.links = links;
        let content = content::serialize(&doc.metadata, &doc.body)?;
        self.content.save(id, &content, expected_version)
    }
    pub fn start_session(
        &mut self,
        section_id: &str,
        new_session: bool,
    ) -> Result<(Session, Context)> {
        if self.content.read(section_id)?.metadata.kind == "exercise" {
            let item = crate::exercises::get(&mut self.content, section_id)?;
            let (_, context) = self.session_context(&item.session.id, None)?;
            self.preferences.active_document_id = Some(section_id.into());
            self.preferences.active_session_id = Some(item.session.id.clone());
            self.preferences
                .session_by_section
                .insert(section_id.into(), item.session.id.clone());
            self.save_preferences()?;
            return Ok((item.session, context));
        }
        let context = context::load(&self.content, section_id, None)?;
        let session = if new_session {
            None
        } else if let Some(id) = self.preferences.session_by_section.get(section_id).cloned() {
            Some(sessions::get(&mut self.content, &id)?)
        } else {
            // A committed note/session can survive a crash before UI preferences.
            // Reuse real persisted ownership instead of creating an empty duplicate.
            self.content.refresh()?;
            let mut candidates: Vec<_> = self
                .content
                .summaries()
                .into_iter()
                .filter(|document| document.kind == "session")
                .collect();
            candidates.sort_by(|a, b| b.modified_at.cmp(&a.modified_at).then(a.id.cmp(&b.id)));
            let mut previous = None;
            for candidate in candidates {
                let document = self.content.read(&candidate.id)?;
                if document
                    .metadata
                    .extra
                    .get("sectionId")
                    .and_then(Value::as_str)
                    == Some(section_id)
                {
                    previous = Some(sessions::get(&mut self.content, &candidate.id)?);
                    break;
                }
            }
            previous
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
                assistant_reference_manifest: vec![],
                history_manifest: vec![],
                excluded_history_source_ids: vec![],
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
        let mut context = if session.section_id.is_empty() {
            if selection.is_some_and(|text| !text.is_empty()) {
                return Err(CoreError::new(
                    "INVALID_INPUT",
                    "独立探索不带入当前小节选句，请清除选句或开启小节关联。",
                ));
            }
            Context {
                assistant_reference_manifest: vec![],
                history_manifest: vec![],
                excluded_history_source_ids: vec![],
                text: String::new(),
                manifest: vec![],
                warnings: vec![],
                truncated: false,
            }
        } else {
            context::load(&self.content, &session.section_id, selection)?
        };
        context::protect_trashed_history(&mut context, &session, &self.content.trashed_ids());
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
                assistant_width: 600,
                ..UiPreferences::default()
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
                assistant_width: 600,
                ..UiPreferences::default()
            }
        );
    }
}

#[cfg(test)]
mod navigation_preferences_tests {
    use super::*;
    #[test]
    fn navigation_patches_preserve_width_font_and_legacy_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let state = temp.path().join("state");
        let engine = Engine::new(library.clone(), state.clone()).unwrap();
        fs::write(
            state.join("ui-preferences.json"),
            r#"{"fontSize":18,"assistantWidth":420}"#,
        )
        .unwrap();
        assert!(!engine.ui_preferences().unwrap().sidebar_collapsed);
        engine
            .patch_all_ui_preferences(
                None,
                None,
                Some(true),
                Some(vec!["course_a".into(), "folder_b".into()]),
                None,
            )
            .unwrap();
        engine.patch_ui_preferences(Some(20), None).unwrap();
        let saved = engine.ui_preferences().unwrap();
        assert_eq!(saved.font_size, 20);
        assert_eq!(saved.assistant_width, 420);
        assert!(saved.sidebar_collapsed);
        assert_eq!(saved.expanded_node_ids.len(), 2);
        let before = fs::read(state.join("ui-preferences.json")).unwrap();
        assert!(engine
            .patch_all_ui_preferences(None, None, None, Some(vec!["../bad".into()]), None)
            .is_err());
        assert_eq!(before, fs::read(state.join("ui-preferences.json")).unwrap());
        drop(engine);
        assert_eq!(
            Engine::new(library, state)
                .unwrap()
                .ui_preferences()
                .unwrap(),
            saved
        );
    }
}

#[cfg(test)]
mod sidebar_width_tests {
    use super::*;
    #[test]
    fn sidebar_width_defaults_and_independent_patches_keep_old_preferences() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let state = temp.path().join("state");
        let engine = Engine::new(library.clone(), state.clone()).unwrap();
        fs::write(state.join("ui-preferences.json"),
            r#"{"fontSize":18,"assistantWidth":420,"sidebarCollapsed":true,"expandedNodeIds":["folder_a"]}"#).unwrap();
        assert_eq!(engine.ui_preferences().unwrap().sidebar_width, 240);
        let a = engine.clone();
        let b = engine.clone();
        let font = std::thread::spawn(move || a.patch_ui_preferences(Some(20), None).unwrap());
        let width = std::thread::spawn(move || {
            b.patch_all_ui_preferences(None, None, None, None, Some(360))
                .unwrap()
        });
        font.join().unwrap();
        width.join().unwrap();
        let saved = engine.ui_preferences().unwrap();
        assert_eq!(saved.sidebar_width, 360);
        assert_eq!(saved.font_size, 20);
        assert_eq!(saved.assistant_width, 420);
        assert!(saved.sidebar_collapsed);
        assert_eq!(saved.expanded_node_ids, vec!["folder_a"]);
        let bytes = fs::read(state.join("ui-preferences.json")).unwrap();
        for invalid in [179, 481] {
            let error = engine
                .patch_all_ui_preferences(None, None, None, None, Some(invalid))
                .unwrap_err();
            assert_eq!(error.code, "INVALID_INPUT");
            assert_eq!(fs::read(state.join("ui-preferences.json")).unwrap(), bytes);
        }
        drop(engine);
        assert_eq!(
            Engine::new(library, state)
                .unwrap()
                .ui_preferences()
                .unwrap(),
            saved
        );
    }
}

#[cfg(test)]
mod nested_import_tests {
    use super::*;
    #[test]
    fn json_import_preserves_identity_links_and_parent_in_one_receipted_write() {
        let temp = tempfile::tempdir().unwrap();
        let engine = Engine::new(temp.path().join("library"), temp.path().join("state")).unwrap();
        let mut core = engine.lock().unwrap();
        let course = crate::library_tree::create(
            &mut core.content,
            "course",
            "Course",
            None,
            "",
            "import_course_001",
        )
        .unwrap();
        let folder = crate::library_tree::create(
            &mut core.content,
            "folder",
            "Folder",
            Some(&course.id),
            "",
            "import_folder_001",
        )
        .unwrap();
        let original="```json\n{\"id\":\"import_original_id\",\"title\":\"Authored\",\"kind\":\"section\",\"links\":[\"related_material\"]}\n```\n\nAuthored body";
        let doc = core
            .import_at(original, Some(&folder.id), Some("import_request_001"))
            .unwrap();
        assert_eq!(doc.id, "import_original_id");
        assert_eq!(doc.metadata.links, vec!["related_material"]);
        assert_eq!(doc.metadata.extra["parentId"], folder.id);
        assert_eq!(
            core.import_at(original, Some(&folder.id), Some("import_request_001"))
                .unwrap()
                .version,
            doc.version
        );
        let altered = original.replace("Authored body", "Different body");
        assert_eq!(
            core.import_at(&altered, Some(&folder.id), Some("import_request_001"))
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert_eq!(core.content.read(&doc.id).unwrap().version, doc.version);
        assert!(core
            .import_at(original, Some(&folder.id), Some("other_import_request"))
            .is_err());
    }
}

#[cfg(test)]
mod linked_exploration_tests {
    use super::*;
    #[test]
    fn explicit_links_selection_and_unlink_use_actual_versions_without_other_courses() {
        let temp = tempfile::tempdir().unwrap();
        let engine = Engine::new(temp.path().join("library"), temp.path().join("state")).unwrap();
        let mut core = engine.lock().unwrap();
        let material = core
            .create_section("Chosen material", "CHOSEN_SOURCE_MARKER", vec![], vec![])
            .unwrap();
        let foreign = core
            .create_section("Other course", "FOREIGN_SOURCE_MARKER", vec![], vec![])
            .unwrap();
        let explore = crate::library_tree::create(
            &mut core.content,
            "exploration",
            "My exploration",
            None,
            "OWN_EXPLORATION_MARKER",
            "exploration_scope_001",
        )
        .unwrap();
        let session = core.start_session(&explore.id, false).unwrap().0;
        let (_, before) = core
            .session_context(&session.id, Some("EXPLICIT_SELECTION_MARKER"))
            .unwrap();
        assert!(
            before.text.contains("OWN_EXPLORATION_MARKER")
                && before.text.contains("EXPLICIT_SELECTION_MARKER")
        );
        assert!(
            !before.text.contains("CHOSEN_SOURCE_MARKER")
                && !before.text.contains("FOREIGN_SOURCE_MARKER")
        );
        let linked = core
            .set_document_links(&explore.id, vec![material.id.clone()], &explore.version)
            .unwrap();
        let (_, context) = core.session_context(&session.id, None).unwrap();
        assert!(
            context.text.contains("CHOSEN_SOURCE_MARKER")
                && !context.text.contains("FOREIGN_SOURCE_MARKER")
        );
        assert_eq!(
            context
                .manifest
                .iter()
                .find(|item| item.id == material.id)
                .unwrap()
                .version,
            material.version
        );
        core.set_document_links(&explore.id, vec![], &linked.version)
            .unwrap();
        let (_, unlinked) = core.session_context(&session.id, None).unwrap();
        assert!(
            !unlinked.text.contains("CHOSEN_SOURCE_MARKER")
                && !unlinked.text.contains("FOREIGN_SOURCE_MARKER")
        );
        assert!(core
            .set_document_links(&foreign.id, vec![foreign.id.clone()], &foreign.version)
            .is_err());
    }
}

#[cfg(test)]
mod exercise_context_guard_tests {
    use super::*;
    #[test]
    fn exercise_routing_uses_origin_and_direct_links_never_read_reference_answers() {
        let temp = tempfile::tempdir().unwrap();
        let engine = Engine::new(temp.path().join("library"), temp.path().join("state")).unwrap();
        let mut core = engine.lock().unwrap();
        let source = core
            .create_section(
                "Question source",
                "SOURCE_WITHOUT_REFERENCE",
                vec![],
                vec![],
            )
            .unwrap();
        let session = core.start_session(&source.id, false).unwrap().0;
        let item = crate::exercises::create_manual(
            &mut core.content,
            &session.id,
            &source.id,
            crate::exercises::ManualExercise {
                question: "Try first".into(),
                format: "self".into(),
                answers: vec!["HIDDEN_REFERENCE_MARKER".into()],
                explanation: "HIDDEN_EXPLANATION_MARKER".into(),
            },
            "context_guard_exercise",
        )
        .unwrap();
        assert!(item.document.body.contains("HIDDEN_REFERENCE_MARKER"));
        assert_eq!(
            context::load(&core.content, &item.document.id, None)
                .unwrap_err()
                .code,
            "EXERCISE_CONTEXT_RESTRICTED"
        );
        let (actual, ctx) = core.start_session(&item.document.id, false).unwrap();
        assert_eq!(actual.id, session.id);
        assert!(!ctx.text.contains("HIDDEN_REFERENCE_MARKER"));
        let mut metadata = source.metadata.clone();
        metadata.links = vec![item.document.id.clone()];
        core.content
            .save(
                &source.id,
                &content::serialize(&metadata, &source.body).unwrap(),
                &source.version,
            )
            .unwrap();
        let safe = context::load(&core.content, &source.id, None).unwrap();
        assert!(safe
            .warnings
            .iter()
            .any(|warning| warning.starts_with("EXERCISE_CONTEXT_OMITTED")));
        assert!(
            !safe.text.contains("HIDDEN_REFERENCE_MARKER")
                && !safe.text.contains("HIDDEN_EXPLANATION_MARKER")
        );
    }
}

#[cfg(test)]
mod exploration_and_trash_recovery_tests {
    use super::*;

    #[test]
    fn exploration_reopens_its_committed_question_without_preference_mapping() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("library");
        let state = directory.path().join("state");
        let engine = Engine::new(library.clone(), state.clone()).unwrap();
        let question = "Why is for used with three years?";
        let saved = {
            let mut core = engine.lock().unwrap();
            crate::exploration::begin(&mut core.content, question, "recovery_question_001").unwrap()
        };
        drop(engine); // Equivalent to loss before the command saves open/session preferences.
        let reopened = Engine::new(library, state).unwrap();
        let mut core = reopened.lock().unwrap();
        core.content.refresh().unwrap();
        assert!(!core
            .preferences
            .session_by_section
            .contains_key(&saved.document.id));
        let (session, _) = core.start_session(&saved.document.id, false).unwrap();
        assert_eq!(session.id, saved.session.id);
        assert_eq!(
            session
                .entries
                .iter()
                .filter(|entry| entry.kind == "user")
                .count(),
            1
        );
        assert_eq!(
            session
                .entries
                .iter()
                .find(|entry| entry.id == saved.user_entry_id)
                .unwrap()
                .content,
            question
        );
        core.content.refresh().unwrap();
        assert_eq!(
            core.content
                .summaries()
                .iter()
                .filter(|document| document.kind == "session")
                .count(),
            1
        );
    }

    #[test]
    fn trashed_open_ids_are_hidden_and_restore_recovers_original_preferences() {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(
            directory.path().join("library"),
            directory.path().join("state"),
        )
        .unwrap();
        let (document_id, session_id, receipt) = {
            let mut core = engine.lock().unwrap();
            let document = core
                .create_section("English", "for three years", vec![], vec![])
                .unwrap();
            let (session, _) = core.start_session(&document.id, false).unwrap();
            let preview = crate::trash::preview(&mut core.content, &document.id).unwrap();
            let receipt = crate::trash::trash(
                &mut core.content,
                &document.id,
                &preview.version,
                "hide_open_record_001",
            )
            .unwrap();
            (document.id, session.id, receipt)
        };
        let state = engine.state().unwrap();
        assert!(state.active_document_id.is_none());
        assert!(state.active_session_id.is_none());
        assert!(!state
            .documents
            .iter()
            .any(|document| document.id == document_id));
        {
            let mut core = engine.lock().unwrap();
            crate::trash::restore(
                &mut core.content,
                &receipt.trash_id,
                "restore_open_record_001",
            )
            .unwrap();
        }
        let state = engine.state().unwrap();
        assert_eq!(
            state.active_document_id.as_deref(),
            Some(document_id.as_str())
        );
        assert_eq!(
            state.active_session_id.as_deref(),
            Some(session_id.as_str())
        );
    }
}
