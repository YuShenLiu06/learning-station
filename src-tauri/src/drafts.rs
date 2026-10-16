//! Device-local editor input. These JSON records never become course Markdown.
use crate::app::atomic_json;
use crate::types::{hash, timestamp, CoreError, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_BODY_BYTES: usize = 1536 * 1024;
const MAX_QUESTION_CHARS: usize = 30_000;
const MAX_TITLE_CHARS: usize = 500;
const MAX_DRAFT_BYTES: usize = 4 * 1024 * 1024;

/// Only these editor fields may cross the draft IPC boundary. Model settings and
/// API key fields are deliberately absent and unknown fields are rejected.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct DraftData {
    pub body: String,
    pub question: String,
    pub import_title: String,
    pub import_body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UiDraft {
    pub key: String,
    pub data: DraftData,
    pub version: String,
    pub updated_at: String,
}

pub struct DraftStore {
    directory: PathBuf,
    // Serialize compare-and-write operations made through the application's store.
    writes: Mutex<()>,
}

impl DraftStore {
    /// Creates a private runtime directory without scanning existing draft files.
    pub fn new(state_dir: &Path) -> Result<Self> {
        fs::create_dir_all(state_dir).map_err(CoreError::io)?;
        let state_dir = fs::canonicalize(state_dir).map_err(CoreError::io)?;
        let directory = state_dir.join("drafts");
        match fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(CoreError::io(error)),
        }
        let metadata = fs::symlink_metadata(&directory).map_err(CoreError::io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(CoreError::new("IO", "本机草稿目录无效，输入仍保留。"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .map_err(CoreError::io)?;
        }
        Ok(Self {
            directory,
            writes: Mutex::new(()),
        })
    }

    pub fn get(&self, key: &str) -> Result<Option<UiDraft>> {
        let path = self.path(key)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(CoreError::io(error)),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CoreError::new("IO", "本机草稿文件无效，输入仍保留。"));
        }
        if metadata.len() > MAX_DRAFT_BYTES as u64 {
            return Err(too_large());
        }
        let file = fs::File::open(&path).map_err(CoreError::io)?;
        let mut bytes = Vec::new();
        file.take(MAX_DRAFT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(CoreError::io)?;
        if bytes.len() > MAX_DRAFT_BYTES {
            return Err(too_large());
        }
        let draft: UiDraft = serde_json::from_slice(&bytes).map_err(|_| invalid_draft())?;
        validate_data(&draft.data)?;
        if draft.key != key
            || draft.version != draft_version(&draft.key, &draft.data, &draft.updated_at)?
        {
            return Err(invalid_draft());
        }
        Ok(Some(draft))
    }

    /// None permits an upsert. With a version, the saved record must still match.
    pub fn save(
        &self,
        key: &str,
        data: DraftData,
        expected_version: Option<&str>,
    ) -> Result<UiDraft> {
        let path = self.path(key)?;
        validate_data(&data)?;
        let _guard = self
            .writes
            .lock()
            .map_err(|_| CoreError::new("BUSY", "草稿保存暂不可用，输入仍保留。"))?;
        let current = self.get(key)?;
        if let Some(expected) = expected_version {
            if current.as_ref().map(|draft| draft.version.as_str()) != Some(expected) {
                return Err(CoreError::new(
                    "CONFLICT",
                    "草稿已在其他位置更新，当前输入仍保留。",
                ));
            }
        }
        let updated_at = timestamp();
        let version = draft_version(key, &data, &updated_at)?;
        let draft = UiDraft {
            key: key.to_owned(),
            data,
            version,
            updated_at,
        };
        // Count the same pretty JSON encoding used by atomic_json, including
        // escaped control characters and record metadata.
        let bytes = serde_json::to_vec_pretty(&draft).map_err(|_| invalid_draft())?;
        if bytes.len() > MAX_DRAFT_BYTES {
            return Err(too_large());
        }
        atomic_json(&path, &draft)?;
        Ok(draft)
    }

    fn path(&self, key: &str) -> Result<PathBuf> {
        if key.is_empty()
            || key.len() > 160
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-'))
        {
            return Err(CoreError::new(
                "INVALID_DRAFT_KEY",
                "草稿标识只允许字母、数字、冒号、下划线和短横线（最长160字节）。",
            ));
        }
        Ok(self.directory.join(format!("{}.json", hash(key))))
    }
}

fn validate_data(data: &DraftData) -> Result<()> {
    if data.body.len() > MAX_BODY_BYTES
        || data.import_body.len() > MAX_BODY_BYTES
        || data.question.chars().count() > MAX_QUESTION_CHARS
        || data.import_title.chars().count() > MAX_TITLE_CHARS
    {
        return Err(too_large());
    }
    Ok(())
}

fn draft_version(key: &str, data: &DraftData, updated_at: &str) -> Result<String> {
    let canonical = serde_json::to_string(&(key, data, updated_at)).map_err(|_| invalid_draft())?;
    Ok(hash(&canonical))
}
fn too_large() -> CoreError {
    CoreError::new(
        "LIMIT_EXCEEDED",
        "草稿过大：正文各限1.5MiB、问题限30000字符、标题限500字符，整体限4MiB。",
    )
}
fn invalid_draft() -> CoreError {
    CoreError::new("INVALID_DRAFT", "本机草稿格式无效，当前输入仍保留。")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roundtrip_reopens_with_private_file_and_enforces_size_limits() {
        let directory = tempfile::tempdir().unwrap();
        let store = DraftStore::new(directory.path()).unwrap();
        assert!(store.get("section:lesson-1").unwrap().is_none());
        let data = DraftData {
            body: "尚未保存到课程的正文🌍".into(),
            question: "这里为什么成立？".into(),
            import_title: "待导入".into(),
            import_body: "# 待导入内容".into(),
        };
        let saved = store.save("section:lesson-1", data.clone(), None).unwrap();
        let reopened = DraftStore::new(directory.path()).unwrap();
        assert_eq!(reopened.get("section:lesson-1").unwrap().unwrap(), saved);
        assert_eq!(saved.data, data);
        let path = directory
            .path()
            .join("drafts")
            .join(format!("{}.json", hash("section:lesson-1")));
        assert!(path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let excessive = DraftData {
            body: "x".repeat(MAX_BODY_BYTES + 1),
            ..DraftData::default()
        };
        assert_eq!(
            store.save("big", excessive, None).unwrap_err().code,
            "LIMIT_EXCEEDED"
        );
        let excessive = DraftData {
            question: "问".repeat(MAX_QUESTION_CHARS + 1),
            ..DraftData::default()
        };
        assert_eq!(
            store.save("big", excessive, None).unwrap_err().code,
            "LIMIT_EXCEEDED"
        );
        let excessive = DraftData {
            import_title: "题".repeat(MAX_TITLE_CHARS + 1),
            ..DraftData::default()
        };
        assert_eq!(
            store.save("big", excessive, None).unwrap_err().code,
            "LIMIT_EXCEEDED"
        );
        // JSON escaping can exceed the record budget before field byte budgets.
        let excessive = DraftData {
            body: "\u{0001}".repeat(MAX_BODY_BYTES),
            ..DraftData::default()
        };
        assert_eq!(
            store.save("big", excessive, None).unwrap_err().code,
            "LIMIT_EXCEEDED"
        );
        assert!(store.get("big").unwrap().is_none());
    }

    #[test]
    fn rejects_unknown_api_key_fields_instead_of_persisting_settings() {
        for name in ["apiKey", "api_key", "model", "baseUrl"] {
            let mut value = json!({"body":"草稿"});
            value[name] = json!("must not enter drafts");
            assert!(serde_json::from_value::<DraftData>(value).is_err());
        }
        let defaults: DraftData = serde_json::from_value(json!({"body":"草稿"})).unwrap();
        assert!(defaults.question.is_empty());
        assert!(defaults.import_title.is_empty());
        assert!(defaults.import_body.is_empty());
    }

    #[test]
    fn invalid_keys_cannot_traverse_or_select_arbitrary_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = DraftStore::new(directory.path()).unwrap();
        for key in [
            "",
            "../secret",
            "/tmp/secret",
            "section/lesson",
            "a\\b",
            "中文",
            ".",
            "..",
        ] {
            assert_eq!(store.get(key).unwrap_err().code, "INVALID_DRAFT_KEY");
            assert_eq!(
                store
                    .save(key, DraftData::default(), None)
                    .unwrap_err()
                    .code,
                "INVALID_DRAFT_KEY"
            );
        }
        assert_eq!(
            store.get(&"x".repeat(161)).unwrap_err().code,
            "INVALID_DRAFT_KEY"
        );
        assert_eq!(
            fs::read_dir(directory.path().join("drafts"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn stale_version_does_not_overwrite_newer_draft_or_create_absent_record() {
        let directory = tempfile::tempdir().unwrap();
        let store = DraftStore::new(directory.path()).unwrap();
        let first = store
            .save(
                "lesson:1",
                DraftData {
                    body: "第一稿".into(),
                    ..DraftData::default()
                },
                None,
            )
            .unwrap();
        let second = store
            .save(
                "lesson:1",
                DraftData {
                    body: "第二稿".into(),
                    ..DraftData::default()
                },
                Some(&first.version),
            )
            .unwrap();
        assert_ne!(first.version, second.version);
        assert_eq!(
            store
                .save("lesson:1", first.data, Some(&first.version))
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert_eq!(store.get("lesson:1").unwrap().unwrap(), second);
        assert_eq!(
            store
                .save("absent", DraftData::default(), Some("stale"))
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert!(store.get("absent").unwrap().is_none());
    }
}
