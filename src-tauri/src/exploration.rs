//! Persist the launch question before generation. Stable document, session and
//! entry IDs repair an interrupted launch without relying on UI preferences.
use crate::{content::ContentStore, library_tree, sessions, types::*};
use serde::{Deserialize, Serialize};
use serde_json::json;

const FOLDER_REQUEST_ID: &str = "learning-station-exploration-folder-v1";
const MAX_QUESTION_BYTES: usize = 30_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorationNote {
    pub document: Document,
    pub session: Session,
    pub user_entry_id: String,
    pub folder_id: String,
}

/// The caller holds its normal Core/content lock. This function never touches
/// model configuration, credentials, active-document preferences or networking.
pub fn begin(
    store: &mut ContentStore,
    question: &str,
    request_id: &str,
) -> Result<ExplorationNote> {
    validate(question, request_id)?;
    store.sync_trash_visibility()?;
    let folder_id = format!("node_{}", &hash(FOLDER_REQUEST_ID)[..32]);
    if store.is_trashed(&folder_id) {
        return Err(folder_trashed());
    }
    let folder = library_tree::create(store, "folder", "探索", None, "", FOLDER_REQUEST_ID)
        .map_err(|error| {
            if error.code == "TRASHED" {
                folder_trashed()
            } else {
                error
            }
        })?;
    if folder.id != folder_id
        || folder.metadata.kind != "folder"
        || folder.metadata.title != "探索"
        || folder
            .metadata
            .extra
            .get("parentId")
            .is_some_and(|parent| !parent.is_null())
    {
        return Err(CoreError::new(
            "CONFLICT",
            "固定探索目录的结构已改变，未改写或新建替代目录。",
        ));
    }
    let note_request = note_request_id(request_id);
    let document = library_tree::create(
        store,
        "section",
        &question_title(question),
        Some(&folder_id),
        question,
        &note_request,
    )?;
    // An existing note may have been edited or moved after creation. Its initial
    // createRequestDigest protects request identity; never restore old body text.
    let session_id = session_id(request_id);
    let session = sessions::create_with_id(store, &document.id, &[], &session_id)?;
    let entry_request = entry_request_id(request_id);
    let user_entry_id = format!("entry_{}", &hash(&entry_request)[..32]);
    let saved = sessions::append(
        store,
        &session.id,
        Entry {
            id: String::new(),
            kind: "user".into(),
            content: question.into(),
            created_at: String::new(),
            selection: None,
            related_ids: vec![],
            data: Some(
                json!({"clientMessageId":request_id,"mode":"question","origin":"exploration-entry"}),
            ),
        },
        Some(&session.version),
        &entry_request,
    )?;
    if !saved
        .entries
        .iter()
        .any(|entry| entry.id == user_entry_id && entry.kind == "user")
    {
        return Err(CoreError::new(
            "IO",
            "探索问题尚未确认写入，重试将补齐同一份记录。",
        ));
    }
    Ok(ExplorationNote {
        document,
        session: saved,
        user_entry_id,
        folder_id,
    })
}

fn validate(question: &str, request_id: &str) -> Result<()> {
    if question.trim().is_empty()
        || question.len() > MAX_QUESTION_BYTES
        || !(8..=128).contains(&request_id.len())
        || !valid_id(request_id)
    {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "请输入非空问题（最多30000字节）和8至128位有效请求编号。",
        ));
    }
    Ok(())
}

fn question_title(question: &str) -> String {
    let line = question
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(question);
    let title: String = line
        .trim()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(80)
        .collect();
    if title.trim().is_empty() {
        "新的探索".into()
    } else {
        title
    }
}
fn note_request_id(request: &str) -> String {
    format!("exploration_note_{}", hash(request))
}
fn session_id(request: &str) -> String {
    format!("exploration_session_{}", &hash(request)[..32])
}
fn entry_request_id(request: &str) -> String {
    format!("exploration_user_{}", hash(request))
}
fn folder_trashed() -> CoreError {
    CoreError::new(
        "EXPLORATION_FOLDER_TRASHED",
        "探索目录已在回收站中；请先恢复该目录，再开始探索。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, ContentStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = ContentStore::new(
            directory.path().join("library"),
            directory.path().join("state"),
        )
        .unwrap();
        (directory, store)
    }

    fn counts(store: &mut ContentStore) -> (usize, usize, usize, usize) {
        store.refresh().unwrap();
        let documents = store.summaries();
        (
            documents.iter().filter(|doc| doc.kind == "folder").count(),
            documents.iter().filter(|doc| doc.kind == "section").count(),
            documents.iter().filter(|doc| doc.kind == "session").count(),
            documents.iter().filter(|doc| doc.kind == "entry").count(),
        )
    }

    #[test]
    fn begin_preserves_original_question_and_is_idempotent_without_model_state() {
        let (directory, mut store) = fixture();
        let old = library_tree::create(
            &mut store,
            "exploration",
            "旧探索",
            None,
            "LEGACY_PRIVATE_MATERIAL",
            "legacy_exploration_fixture",
        )
        .unwrap();
        let question = "  \n为什么这里用for？  \n请保留我的原问题。\n";
        let first = begin(&mut store, question, "exploration_begin_001").unwrap();
        assert_eq!(first.document.metadata.kind, "section");
        assert_eq!(first.document.body, question);
        assert!(first.document.metadata.links.is_empty());
        assert_eq!(first.document.metadata.extra["parentId"], first.folder_id);
        let folder = store.read(&first.folder_id).unwrap();
        assert_eq!(folder.metadata.kind, "folder");
        assert_eq!(folder.metadata.title, "探索");
        assert!(folder.metadata.extra["parentId"].is_null());
        assert_eq!(first.session.section_id, first.document.id);
        assert!(first.session.context_manifest.is_empty());
        let session_document = store.read(&first.session.id).unwrap();
        assert_eq!(
            session_document.metadata.extra["associateCurrentSection"],
            true
        );
        assert_eq!(
            session_document.metadata.links,
            vec![first.document.id.clone()]
        );
        assert_eq!(first.session.entries.len(), 1);
        let entry = &first.session.entries[0];
        assert_eq!(entry.id, first.user_entry_id);
        assert_eq!(entry.kind, "user");
        assert_eq!(entry.content, question);
        assert!(entry.selection.is_none());
        assert!(entry.related_ids.is_empty());
        assert_eq!(
            entry.data.as_ref().unwrap(),
            &json!({"clientMessageId":"exploration_begin_001","mode":"question","origin":"exploration-entry"})
        );
        let replay = begin(&mut store, question, "exploration_begin_001").unwrap();
        assert_eq!(replay.document.id, first.document.id);
        assert_eq!(replay.session.id, first.session.id);
        assert_eq!(replay.user_entry_id, first.user_entry_id);
        assert_eq!(counts(&mut store), (1, 1, 1, 1));
        assert_eq!(store.read(&old.id).unwrap().content, old.content);
        assert!(!directory.path().join("state/model.json").exists());
        assert!(!directory.path().join("state/preferences.json").exists());
        assert_eq!(
            begin(&mut store, "同一个请求换了问题", "exploration_begin_001")
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert_eq!(store.read(&first.document.id).unwrap().body, question);
        assert_eq!(counts(&mut store), (1, 1, 1, 1));
    }

    #[test]
    fn restart_with_empty_index_and_no_preferences_reuses_note_session_and_entry() {
        let (directory, mut store) = fixture();
        let first = begin(
            &mut store,
            "问题保留在本地，而不是只保留在输入框。",
            "exploration_reopen_001",
        )
        .unwrap();
        drop(store);
        let mut reopened = ContentStore::new(
            directory.path().join("library"),
            directory.path().join("state"),
        )
        .unwrap();
        assert!(reopened.summaries().is_empty());
        assert!(!directory.path().join("state/preferences.json").exists());
        let replay = begin(
            &mut reopened,
            "问题保留在本地，而不是只保留在输入框。",
            "exploration_reopen_001",
        )
        .unwrap();
        assert_eq!(replay.document.id, first.document.id);
        assert_eq!(replay.session.id, first.session.id);
        assert_eq!(replay.user_entry_id, first.user_entry_id);
        assert_eq!(replay.session.entries.len(), 1);
        assert_eq!(counts(&mut reopened), (1, 1, 1, 1));
    }

    #[test]
    fn retries_repair_both_note_only_and_session_without_entry_after_restart() {
        for after_session in [false, true] {
            let (directory, mut store) = fixture();
            let request = "exploration_partial_001";
            let question = "  原问题已经保存，即使下一步写入失败。  ";
            let stable_session = session_id(request);
            let blocker = if after_session {
                let parent = store.root().join("sessions").join(&stable_session);
                fs::create_dir_all(&parent).unwrap();
                parent.join("entries")
            } else {
                store.root().join("sessions")
            };
            fs::write(&blocker, b"fixture: prevent the next stage").unwrap();
            assert!(begin(&mut store, question, request).is_err());
            let note_id = format!("node_{}", &hash(&note_request_id(request))[..32]);
            let note = store.read(&note_id).unwrap();
            assert_eq!(note.body, question);
            assert_eq!(
                store
                    .root()
                    .join(format!("sessions/{stable_session}/session.md"))
                    .exists(),
                after_session
            );
            // Preferences are deliberately absent. The repair path must use the
            // operation's stable IDs rather than the last active UI session.
            assert!(!directory.path().join("state/preferences.json").exists());
            fs::remove_file(&blocker).unwrap();
            drop(store);
            let mut reopened = ContentStore::new(
                directory.path().join("library"),
                directory.path().join("state"),
            )
            .unwrap();
            let repaired = begin(&mut reopened, question, request).unwrap();
            assert_eq!(repaired.document.id, note_id);
            assert_eq!(repaired.session.id, stable_session);
            assert_eq!(repaired.session.entries.len(), 1);
            assert_eq!(repaired.session.entries[0].content, question);
            assert_eq!(counts(&mut reopened), (1, 1, 1, 1));
        }
    }

    #[test]
    fn recycled_fixed_folder_is_not_recreated_and_restoring_it_allows_retry() {
        let (_directory, mut store) = fixture();
        let first = begin(&mut store, "第一个探索问题", "exploration_trash_folder_001").unwrap();
        let preview = crate::trash::preview(&mut store, &first.folder_id).unwrap();
        let receipt = crate::trash::trash(
            &mut store,
            &first.folder_id,
            &preview.version,
            "trash_exploration_folder",
        )
        .unwrap();
        let files_before = fs::read_dir(store.root().join("courses")).unwrap().count();
        assert_eq!(
            begin(&mut store, "另一个探索问题", "exploration_trash_folder_002")
                .unwrap_err()
                .code,
            "EXPLORATION_FOLDER_TRASHED"
        );
        assert_eq!(
            fs::read_dir(store.root().join("courses")).unwrap().count(),
            files_before
        );
        assert!(store.is_trashed(&first.folder_id));
        crate::trash::restore(&mut store, &receipt.trash_id, "restore_exploration_folder").unwrap();
        let replay = begin(&mut store, "第一个探索问题", "exploration_trash_folder_001").unwrap();
        assert_eq!(replay.document.id, first.document.id);
        assert_eq!(replay.session.id, first.session.id);
        assert_eq!(replay.user_entry_id, first.user_entry_id);
        let second = begin(&mut store, "另一个探索问题", "exploration_trash_folder_002").unwrap();
        assert_eq!(second.folder_id, first.folder_id);
        assert_ne!(second.document.id, first.document.id);
        assert_eq!(counts(&mut store), (1, 2, 2, 2));
    }

    #[test]
    fn recycled_note_cannot_be_bypassed_by_reusing_the_same_operation() {
        let (_directory, mut store) = fixture();
        let first = begin(
            &mut store,
            "回收后应先恢复原探索",
            "exploration_trash_note_001",
        )
        .unwrap();
        let preview = crate::trash::preview(&mut store, &first.document.id).unwrap();
        let receipt = crate::trash::trash(
            &mut store,
            &first.document.id,
            &preview.version,
            "trash_exploration_note",
        )
        .unwrap();
        let files_before = fs::read_dir(store.root().join("courses")).unwrap().count();
        assert_eq!(
            begin(
                &mut store,
                "回收后应先恢复原探索",
                "exploration_trash_note_001"
            )
            .unwrap_err()
            .code,
            "TRASHED"
        );
        assert_eq!(
            fs::read_dir(store.root().join("courses")).unwrap().count(),
            files_before
        );
        assert!(store.is_trashed(&first.document.id));
        crate::trash::restore(&mut store, &receipt.trash_id, "restore_exploration_note").unwrap();
        let replay = begin(
            &mut store,
            "回收后应先恢复原探索",
            "exploration_trash_note_001",
        )
        .unwrap();
        assert_eq!(replay.user_entry_id, first.user_entry_id);
        assert_eq!(counts(&mut store), (1, 1, 1, 1));
    }

    #[test]
    fn retry_keeps_later_note_edits_and_shared_ordinary_session() {
        let (_directory, mut store) = fixture();
        let first = begin(&mut store, "最初的问题", "exploration_edit_001").unwrap();
        let edited =
            crate::content::serialize(&first.document.metadata, "本人后来编辑的笔记正文。\n")
                .unwrap();
        store
            .save(&first.document.id, &edited, &first.document.version)
            .unwrap();
        let replay = begin(&mut store, "最初的问题", "exploration_edit_001").unwrap();
        assert_eq!(replay.document.body, "本人后来编辑的笔记正文。\n");
        assert_eq!(replay.session.id, first.session.id);
        assert_eq!(replay.session.section_id, first.document.id);
        assert_eq!(replay.session.entries[0].content, "最初的问题");
        assert_eq!(counts(&mut store), (1, 1, 1, 1));
    }

    #[test]
    fn validates_byte_limit_and_request_ids_before_creating_anything() {
        let (_directory, mut store) = fixture();
        for (question, request) in [
            (" \n".into(), "valid_request".into()),
            ("x".repeat(MAX_QUESTION_BYTES + 1), "valid_request".into()),
            ("问题".into(), "short".into()),
            ("问题".into(), "../outside".into()),
            ("问题".into(), "x".repeat(129)),
        ] {
            assert_eq!(
                begin(&mut store, &question, &request).unwrap_err().code,
                "INVALID_INPUT"
            );
        }
        assert!(!store.root().join("courses").exists());
        let exact = "问".repeat(MAX_QUESTION_BYTES / 3);
        let result = begin(&mut store, &exact, &"x".repeat(128)).unwrap();
        assert_eq!(result.document.body, exact);
        assert!(result.document.metadata.title.len() <= 320);
        assert_eq!(counts(&mut store), (1, 1, 1, 1));
    }
}
