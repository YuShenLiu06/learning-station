//! Markdown is the source of truth. The in-memory index contains paths and summaries only.
use crate::types::{CoreError, Document, DocumentSummary, Metadata, Result};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub const MAX_DOCUMENT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SCAN_FILES: usize = 4096;
const MAX_SCAN_DIRECTORIES: usize = 4096;

#[derive(Clone)]
struct IndexEntry {
    relative: String,
    summary: DocumentSummary,
}

pub struct ContentStore {
    root: PathBuf,
    state_dir: PathBuf,
    index: BTreeMap<String, IndexEntry>,
}

impl ContentStore {
    /// Creates directories, but deliberately does not scan the library on startup.
    pub fn new(root: PathBuf, state_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root).map_err(CoreError::io)?;
        fs::create_dir_all(&state_dir).map_err(CoreError::io)?;
        let root = fs::canonicalize(root).map_err(CoreError::io)?;
        let state_dir = fs::canonicalize(state_dir).map_err(CoreError::io)?;
        Ok(Self {
            root,
            state_dir,
            index: BTreeMap::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Refresh is transactional: a scan failure does not discard the previous index.
    /// Invalid/non-course Markdown and symlinks are ignored. Scan work is bounded.
    pub fn refresh(&mut self) -> Result<Vec<DocumentSummary>> {
        let mut pending = vec![self.root.clone()];
        let mut files_seen = 0;
        let mut directories_seen = 0;
        let mut index = BTreeMap::new();
        while let Some(dir) = pending.pop() {
            directories_seen += 1;
            if directories_seen > MAX_SCAN_DIRECTORIES {
                return Err(CoreError::new(
                    "LIMIT_EXCEEDED",
                    "库目录数量超过扫描上限（4096）",
                ));
            }
            let mut entries = Vec::new();
            for entry in fs::read_dir(&dir).map_err(CoreError::io)? {
                entries.push(entry.map_err(CoreError::io)?.path());
                if entries.len() > MAX_SCAN_FILES {
                    return Err(CoreError::new(
                        "LIMIT_EXCEEDED",
                        "单目录条目超过扫描上限（4096）",
                    ));
                }
            }
            entries.sort();
            for path in entries {
                // State may be placed inside the library by a custom installation.
                // History remains private state and must never become course documents.
                if path.starts_with(self.state_dir.join("history")) {
                    continue;
                }
                let meta = fs::symlink_metadata(&path).map_err(CoreError::io)?;
                if meta.file_type().is_symlink() {
                    continue;
                }
                if meta.is_dir() {
                    pending.push(path);
                    continue;
                }
                if !meta.is_file() {
                    continue;
                }
                files_seen += 1;
                if files_seen > MAX_SCAN_FILES {
                    return Err(CoreError::new(
                        "LIMIT_EXCEEDED",
                        "库文件数量超过扫描上限（4096）",
                    ));
                }
                if path
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("md"))
                    != Some(true)
                    || meta.len() > MAX_DOCUMENT_BYTES
                {
                    continue;
                }
                let relative =
                    relative_string(path.strip_prefix(&self.root).map_err(|_| unsafe_path())?)?;
                let document = match self.read_relative(&relative) {
                    Ok(document) => document,
                    Err(error)
                        if matches!(error.code.as_str(), "INVALID_DOCUMENT" | "LIMIT_EXCEEDED") =>
                    {
                        continue
                    }
                    Err(error) => return Err(error),
                };
                if index.contains_key(&document.id) {
                    return Err(CoreError::new(
                        "DUPLICATE_ID",
                        format!("重复的文档 ID：{}", document.id),
                    ));
                }
                index.insert(
                    document.id.clone(),
                    IndexEntry {
                        relative,
                        summary: summary(&document),
                    },
                );
            }
        }
        self.index = index;
        Ok(self.summaries())
    }

    pub fn summaries(&self) -> Vec<DocumentSummary> {
        let mut items: Vec<_> = self
            .index
            .values()
            .map(|entry| entry.summary.clone())
            .collect();
        items.sort_by(|a, b| a.path.cmp(&b.path));
        items
    }

    pub fn read(&self, id: &str) -> Result<Document> {
        validate_id(id)?;
        let entry = self
            .index
            .get(id)
            .ok_or_else(|| CoreError::new("NOT_FOUND", format!("找不到文档：{id}")))?;
        let document = self.read_relative(&entry.relative)?;
        if document.id != id {
            return Err(CoreError::new(
                "CONFLICT",
                "文件中的稳定 ID 已在外部修改，请刷新课程库",
            ));
        }
        Ok(document)
    }

    pub fn read_relative(&self, relative: &str) -> Result<Document> {
        let path = self.checked_path(relative, false)?;
        let file = fs::File::open(&path).map_err(CoreError::io)?;
        let metadata = file.metadata().map_err(CoreError::io)?;
        if !metadata.is_file() {
            return Err(unsafe_path());
        }
        if metadata.len() > MAX_DOCUMENT_BYTES {
            return Err(too_large());
        }
        let mut bytes = Vec::new();
        file.take(MAX_DOCUMENT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(CoreError::io)?;
        if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
            return Err(too_large());
        }
        let content = String::from_utf8(bytes)
            .map_err(|_| CoreError::new("INVALID_DOCUMENT", "Markdown 文件必须为 UTF-8"))?;
        let (frontmatter, body) = parse(&content)?;
        let modified: DateTime<Utc> = metadata.modified().map_err(CoreError::io)?.into();
        Ok(Document {
            id: frontmatter.id.clone(),
            path: relative_string(path.strip_prefix(&self.root).map_err(|_| unsafe_path())?)?,
            metadata: frontmatter,
            body,
            version: hash(&content),
            content,
            modified_at: modified.to_rfc3339(),
        })
    }

    pub fn create(
        &mut self,
        mut metadata: Metadata,
        body: &str,
        relative: &str,
    ) -> Result<Document> {
        validate_metadata(&metadata)?;
        if self.index.contains_key(&metadata.id) {
            return Err(CoreError::new("ALREADY_EXISTS", "稳定 ID 已存在"));
        }
        metadata.version = metadata.version.max(1);
        let content = serialize(&metadata, body)?;
        let target = self.checked_path(relative, true)?;
        if target.exists() {
            return Err(CoreError::new("ALREADY_EXISTS", "目标文件已存在"));
        }
        self.atomic_write(&target, &content, None)?;
        self.register(relative)
    }

    /// Saves only if the current bytes still match the version read by the editor.
    pub fn save(&mut self, id: &str, content: &str, expected_version: &str) -> Result<Document> {
        let current = self.read(id)?;
        if current.version != expected_version {
            return Err(conflict());
        }
        let (mut metadata, body) = parse(content)?;
        if metadata.id != id {
            return Err(CoreError::new(
                "INVALID_DOCUMENT",
                "保存时不能更改文档稳定 ID",
            ));
        }
        // Older clients must not silently drop extension metadata they do not understand.
        for (key, value) in &current.metadata.extra {
            metadata
                .extra
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        metadata.version = current
            .metadata
            .version
            .checked_add(1)
            .ok_or_else(|| CoreError::new("LIMIT_EXCEEDED", "文档版本号已达上限"))?;
        let content = serialize(&metadata, &body)?;
        let target = self.checked_path(&current.path, false)?;
        self.atomic_write(&target, &content, Some(expected_version))?;
        self.register(&current.path)
    }

    /// Registers a file written by another local subsystem without scanning the library.
    pub fn register(&mut self, relative: &str) -> Result<Document> {
        let document = self.read_relative(relative)?;
        if self
            .index
            .get(&document.id)
            .is_some_and(|entry| entry.relative != document.path)
        {
            return Err(CoreError::new("DUPLICATE_ID", "稳定 ID 已由其他文件使用"));
        }
        self.index
            .retain(|_, entry| entry.relative != document.path);
        self.index.insert(
            document.id.clone(),
            IndexEntry {
                relative: document.path.clone(),
                summary: summary(&document),
            },
        );
        Ok(document)
    }

    pub fn forget(&mut self, id: &str) {
        self.index.remove(id);
    }

    fn checked_path(&self, relative: &str, allow_missing: bool) -> Result<PathBuf> {
        if relative.is_empty() || relative.contains('\\') || relative.contains(':') {
            return Err(unsafe_path());
        }
        let path = Path::new(relative);
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(unsafe_path());
        }
        if path
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x.eq_ignore_ascii_case("md"))
            != Some(true)
        {
            return Err(CoreError::new(
                "INVALID_DOCUMENT",
                "课程文件必须使用 .md 扩展名",
            ));
        }
        let mut target = self.root.clone();
        if self
            .root
            .join(path)
            .starts_with(self.state_dir.join("history"))
        {
            return Err(unsafe_path());
        }
        let components: Vec<_> = path.components().collect();
        for (index, component) in components.iter().enumerate() {
            target.push(component.as_os_str());
            match fs::symlink_metadata(&target) {
                Ok(meta) => {
                    if meta.file_type().is_symlink() {
                        return Err(unsafe_path());
                    }
                    if index + 1 < components.len() && !meta.is_dir() {
                        return Err(unsafe_path());
                    }
                    let canonical = fs::canonicalize(&target).map_err(CoreError::io)?;
                    if !canonical.starts_with(&self.root) {
                        return Err(unsafe_path());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_missing => {}
                Err(error) => return Err(CoreError::io(error)),
            }
        }
        Ok(target)
    }

    /// Immutable, content-addressed originals live in application state, outside
    /// the course index. A failed backup must prevent replacement of the source.
    fn snapshot(&self, document: &Document) -> Result<()> {
        let mut directory = self.state_dir.clone();
        for component in ["history", document.id.as_str()] {
            directory.push(component);
            match fs::create_dir(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(CoreError::io(error)),
            }
            let metadata = fs::symlink_metadata(&directory).map_err(CoreError::io)?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || !fs::canonicalize(&directory)
                    .map_err(CoreError::io)?
                    .starts_with(&self.state_dir)
            {
                return Err(CoreError::io("Unsafe history directory"));
            }
        }
        let path = directory.join(format!("{}.md", document.version));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // Do not overwrite any existing SHA-addressed object, including a
                // malformed or partial object left by an interrupted earlier write.
                let metadata = fs::symlink_metadata(&path).map_err(CoreError::io)?;
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.len() != document.content.len() as u64
                    || fs::read(&path).map_err(CoreError::io)? != document.content.as_bytes()
                {
                    return Err(CoreError::io("Existing history snapshot is invalid"));
                }
                return Ok(());
            }
            Err(error) => return Err(CoreError::io(error)),
        };
        if let Err(error) = file
            .write_all(document.content.as_bytes())
            .and_then(|_| file.sync_all())
        {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(CoreError::io(error));
        }
        #[cfg(unix)]
        fs::File::open(&directory)
            .and_then(|dir| dir.sync_all())
            .map_err(CoreError::io)?;
        Ok(())
    }

    fn atomic_write(&self, target: &Path, content: &str, expected: Option<&str>) -> Result<()> {
        if content.len() as u64 > MAX_DOCUMENT_BYTES {
            return Err(too_large());
        }
        let parent = target.parent().ok_or_else(unsafe_path)?;
        fs::create_dir_all(parent).map_err(CoreError::io)?;
        // Recheck containment after creating any parent directories.
        let relative =
            relative_string(target.strip_prefix(&self.root).map_err(|_| unsafe_path())?)?;
        self.checked_path(&relative, expected.is_none())?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(CoreError::io)?;
        if expected.is_some() {
            temporary
                .as_file()
                .set_permissions(fs::metadata(target).map_err(CoreError::io)?.permissions())
                .map_err(CoreError::io)?;
        }
        temporary
            .write_all(content.as_bytes())
            .map_err(CoreError::io)?;
        temporary.as_file().sync_all().map_err(CoreError::io)?;
        if let Some(expected) = expected {
            let latest = self.read_relative(&relative)?;
            if latest.version != expected {
                return Err(conflict());
            }
            self.snapshot(&latest)?;
            // Snapshot IO may take time; keep external-edit detection immediately
            // before replacement as well as before creating the snapshot.
            if self.read_relative(&relative)?.version != expected {
                return Err(conflict());
            }
            temporary
                .persist(target)
                .map_err(|error| CoreError::io(error.error))?;
        } else {
            temporary.persist_noclobber(target).map_err(|error| {
                if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                    CoreError::new("ALREADY_EXISTS", "目标文件已存在")
                } else {
                    CoreError::io(error.error)
                }
            })?;
        }
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(CoreError::io)?;
        Ok(())
    }
}

pub fn hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(CoreError::new(
            "INVALID_DOCUMENT",
            "稳定 ID 只能包含字母、数字、下划线和短横线（最长128字节）",
        ));
    }
    Ok(())
}

fn validate_metadata(metadata: &Metadata) -> Result<()> {
    validate_id(&metadata.id)?;
    if metadata.title.trim().is_empty()
        || !matches!(
            metadata.kind.as_str(),
            "section" | "knowledge" | "session" | "entry"
        )
    {
        return Err(CoreError::new(
            "INVALID_DOCUMENT",
            "文档需要标题和有效类型（section、knowledge、session、entry）",
        ));
    }
    for link in &metadata.links {
        validate_id(link)?;
    }
    // Known fields cannot be shadowed by flattened extension metadata.
    let reserved: BTreeSet<&str> = ["id", "title", "kind", "goals", "tags", "links", "version"]
        .into_iter()
        .collect();
    if metadata
        .extra
        .keys()
        .any(|key| reserved.contains(key.as_str()))
    {
        return Err(CoreError::new(
            "INVALID_DOCUMENT",
            "扩展元数据不能覆盖保留字段",
        ));
    }
    Ok(())
}

pub fn parse(content: &str) -> Result<(Metadata, String)> {
    if content.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(too_large());
    }
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let first_newline = content.find('\n').ok_or_else(invalid_frontmatter)?;
    if content[..first_newline].trim_end_matches('\r') != "```json" {
        return Err(invalid_frontmatter());
    }
    let rest = &content[first_newline + 1..];
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "```" {
            let metadata: Metadata = serde_json::from_str(&rest[..offset])
                .map_err(|_| CoreError::new("INVALID_DOCUMENT", "顶部 JSON 元数据格式无效"))?;
            validate_metadata(&metadata)?;
            let body = rest[offset + line.len()..]
                .strip_prefix("\r\n")
                .or_else(|| rest[offset + line.len()..].strip_prefix('\n'))
                .unwrap_or(&rest[offset + line.len()..])
                .to_owned();
            return Ok((metadata, body));
        }
        offset += line.len();
    }
    Err(invalid_frontmatter())
}

pub fn serialize(metadata: &Metadata, body: &str) -> Result<String> {
    validate_metadata(metadata)?;
    let json = serde_json::to_string_pretty(metadata).map_err(|_| invalid_frontmatter())?;
    let content = format!("```json\n{json}\n```\n\n{body}");
    if content.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(too_large());
    }
    Ok(content)
}

fn summary(document: &Document) -> DocumentSummary {
    DocumentSummary {
        id: document.id.clone(),
        path: document.path.clone(),
        title: document.metadata.title.clone(),
        kind: document.metadata.kind.clone(),
        tags: document.metadata.tags.clone(),
        version: document.version.clone(),
        modified_at: document.modified_at.clone(),
    }
}

fn relative_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(|text| text.replace('\\', "/"))
        .ok_or_else(unsafe_path)
}
fn unsafe_path() -> CoreError {
    CoreError::new(
        "UNSAFE_PATH",
        "仅允许课程库内的相对 Markdown 路径，不能包含符号链接或上级目录",
    )
}
fn invalid_frontmatter() -> CoreError {
    CoreError::new(
        "INVALID_DOCUMENT",
        "Markdown 顶部需要完整的 json 围栏元数据",
    )
}
fn too_large() -> CoreError {
    CoreError::new("LIMIT_EXCEEDED", "Markdown 文件超过 2 MiB 上限")
}
fn conflict() -> CoreError {
    CoreError::new(
        "CONFLICT",
        "文件已在其他位置修改，请刷新后比较差异；当前输入未写入磁盘",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata(id: &str) -> Metadata {
        serde_json::from_value(json!({"id":id,"title":"测试","kind":"section","goals":[],"tags":[],"links":[],"version":1,"custom":{"keep":true}})).unwrap()
    }
    fn store() -> (tempfile::TempDir, ContentStore) {
        let dir = tempfile::tempdir().unwrap();
        let store =
            ContentStore::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        (dir, store)
    }

    #[test]
    fn startup_does_not_scan_and_refresh_reads_markdown() {
        let (_dir, mut store) = store();
        let created = store
            .create(metadata("lesson"), "# 内容\n", "nested/lesson.md")
            .unwrap();
        let reopened = ContentStore::new(store.root.clone(), store.root.join(".state")).unwrap();
        assert!(reopened.summaries().is_empty());
        assert_eq!(store.refresh().unwrap().len(), 1);
        assert_eq!(store.read("lesson").unwrap().content, created.content);
    }

    #[test]
    fn save_preserves_extension_metadata_and_rejects_stale_version() {
        let (_dir, mut store) = store();
        let first = store
            .create(metadata("lesson"), "原始", "lesson.md")
            .unwrap();
        let edited = first.content.replace("原始", "本地修改");
        let saved = store.save("lesson", &edited, &first.version).unwrap();
        assert_eq!(saved.metadata.version, 2);
        assert_eq!(saved.metadata.extra["custom"], json!({"keep": true}));
        assert_eq!(
            store
                .save("lesson", &edited, &first.version)
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        fs::write(
            store.root.join("lesson.md"),
            saved.content.replace("本地修改", "外部修改"),
        )
        .unwrap();
        assert_eq!(
            store
                .save("lesson", &edited, &saved.version)
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert_eq!(store.read("lesson").unwrap().body, "外部修改");
    }

    #[test]
    fn save_snapshots_original_bytes_once_and_stale_save_creates_no_snapshot() {
        let (_dir, mut store) = store();
        let original = store
            .create(metadata("lesson"), "原始正文", "lesson.md")
            .unwrap();
        let saved = store
            .save(
                "lesson",
                &original.content.replace("原始正文", "新正文"),
                &original.version,
            )
            .unwrap();
        let history = store.state_dir.join("history/lesson");
        let snapshot = history.join(format!("{}.md", original.version));
        assert_eq!(fs::read_to_string(&snapshot).unwrap(), original.content);
        assert_eq!(fs::read_dir(&history).unwrap().count(), 1);
        assert_eq!(
            store
                .save("lesson", &original.content, &original.version)
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert_eq!(store.read("lesson").unwrap().content, saved.content);
        assert!(!history.join(format!("{}.md", saved.version)).exists());
        let external = saved.content.replace("新正文", "外部正文");
        fs::write(store.root.join("lesson.md"), &external).unwrap();
        assert_eq!(
            store
                .save("lesson", &original.content, &saved.version)
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert_eq!(
            fs::read_to_string(store.root.join("lesson.md")).unwrap(),
            external
        );
        assert!(!history.join(format!("{}.md", hash(&external))).exists());
        assert_eq!(fs::read_dir(&history).unwrap().count(), 1);
        // Reusing the same original SHA must preserve the existing snapshot bytes.
        store.snapshot(&original).unwrap();
        assert_eq!(fs::read_to_string(&snapshot).unwrap(), original.content);
        assert_eq!(fs::read_dir(&history).unwrap().count(), 1);
    }

    #[test]
    fn snapshot_failure_preserves_current_course_file() {
        let (_dir, mut store) = store();
        let original = store
            .create(metadata("lesson"), "原始正文", "lesson.md")
            .unwrap();
        fs::write(store.state_dir.join("history"), "blocked by a file").unwrap();
        assert_eq!(
            store
                .save(
                    "lesson",
                    &original.content.replace("原始正文", "新正文"),
                    &original.version
                )
                .unwrap_err()
                .code,
            "IO"
        );
        assert_eq!(store.read("lesson").unwrap().content, original.content);
    }

    #[test]
    fn history_inside_library_is_never_indexed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        let mut store = ContentStore::new(root.clone(), root.join(".state")).unwrap();
        let original = store
            .create(metadata("lesson"), "原始正文", "lesson.md")
            .unwrap();
        store
            .save(
                "lesson",
                &original.content.replace("原始正文", "新正文"),
                &original.version,
            )
            .unwrap();
        assert_eq!(store.refresh().unwrap().len(), 1);
        assert_eq!(
            store
                .read_relative(&format!(".state/history/lesson/{}.md", original.version))
                .unwrap_err()
                .code,
            "UNSAFE_PATH"
        );
    }

    #[test]
    fn extension_metadata_survives_an_older_client() {
        let (_dir, mut store) = store();
        let first = store
            .create(metadata("lesson"), "正文", "lesson.md")
            .unwrap();
        let mut old_client = first.metadata.clone();
        old_client.extra.clear();
        let content = serialize(&old_client, "新正文").unwrap();
        let saved = store.save("lesson", &content, &first.version).unwrap();
        assert_eq!(saved.metadata.extra["custom"], json!({"keep": true}));
    }

    #[test]
    fn rejects_traversal_absolute_paths_invalid_ids_and_duplicate_creates() {
        let (_dir, mut store) = store();
        for relative in [
            "../escape.md",
            "/tmp/escape.md",
            "a/../../escape.md",
            "C:\\escape.md",
        ] {
            assert_eq!(
                store.read_relative(relative).unwrap_err().code,
                "UNSAFE_PATH"
            );
        }
        assert!(store.create(metadata("bad/id"), "", "bad.md").is_err());
        store.create(metadata("lesson"), "", "lesson.md").unwrap();
        assert_eq!(
            store
                .create(metadata("other"), "", "lesson.md")
                .unwrap_err()
                .code,
            "ALREADY_EXISTS"
        );
    }

    #[test]
    fn refresh_skips_invalid_and_oversized_files() {
        let (_dir, mut store) = store();
        store.create(metadata("valid"), "正文", "valid.md").unwrap();
        fs::write(store.root.join("README.md"), "# 不是课程").unwrap();
        fs::write(
            store.root.join("huge.md"),
            vec![b'x'; MAX_DOCUMENT_BYTES as usize + 1],
        )
        .unwrap();
        assert_eq!(store.refresh().unwrap().len(), 1);
        assert_eq!(
            store.read_relative("huge.md").unwrap_err().code,
            "LIMIT_EXCEEDED"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_read_or_write_outside_library() {
        use std::os::unix::fs::symlink;
        let (dir, mut store) = store();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(
            outside.join("secret.md"),
            serialize(&metadata("outside"), "private").unwrap(),
        )
        .unwrap();
        symlink(&outside, store.root.join("linked")).unwrap();
        symlink(outside.join("secret.md"), store.root.join("secret.md")).unwrap();
        assert_eq!(
            store.read_relative("linked/secret.md").unwrap_err().code,
            "UNSAFE_PATH"
        );
        assert_eq!(
            store.read_relative("secret.md").unwrap_err().code,
            "UNSAFE_PATH"
        );
        assert_eq!(
            store
                .create(metadata("new"), "", "linked/new.md")
                .unwrap_err()
                .code,
            "UNSAFE_PATH"
        );
        assert!(store.refresh().unwrap().is_empty());
    }

    #[test]
    fn unicode_crlf_and_fenced_code_in_body_round_trip() {
        let content = "```json\r\n{\"id\":\"unicode\",\"title\":\"中文\",\"kind\":\"section\",\"goals\":[],\"tags\":[],\"links\":[],\"version\":1}\r\n```\r\n\r\n你好🌍\n```rust\nlet x = 1;\n```\n";
        let (meta, body) = parse(content).unwrap();
        assert_eq!(meta.id, "unicode");
        assert_eq!(body, "你好🌍\n```rust\nlet x = 1;\n```\n");
        assert_eq!(parse(&serialize(&meta, &body).unwrap()).unwrap().1, body);
    }
}
