//! Recoverable, ledger-only deletion. Markdown and asset bytes are never moved,
//! overwritten or removed by this module. Call under the application's Core lock.
use crate::{content::ContentStore, library_tree, types::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Component, Path},
    time::SystemTime,
};

const MAX_LEDGER_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RECORDS: usize = 4096;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub trash_id: String,
    pub id: String,
    pub title: String,
    pub kind: String,
    pub node_count: usize,
    pub session_count: usize,
    pub created_at: String,
    pub restored_at: Option<String>,
    pub status: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub node_count: usize,
    pub session_count: usize,
    pub version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Snapshot {
    id: String,
    path: String,
    version: String,
    kind: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    receipt: Receipt,
    parent_id: Option<String>,
    snapshots: Vec<Snapshot>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Operation {
    kind: String,
    target: String,
    digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ledger {
    schema_version: u8,
    records: BTreeMap<String, Record>,
    operations: BTreeMap<String, Operation>,
}
impl Default for Ledger {
    fn default() -> Self {
        Self {
            schema_version: 1,
            records: BTreeMap::new(),
            operations: BTreeMap::new(),
        }
    }
}
struct Loaded {
    ledger: Ledger,
    fingerprint: Option<String>,
    stamp: Option<(u64, SystemTime)>,
}
#[derive(Default)]
pub(crate) struct Visibility {
    pub ids: BTreeSet<String>,
    pub paths: BTreeSet<String>,
    pub stamp: Option<(u64, SystemTime)>,
}

pub fn hidden_ids(store: &ContentStore) -> Result<BTreeSet<String>> {
    Ok(load_visibility(store.state_dir())?.ids)
}
pub fn list(store: &ContentStore) -> Result<Vec<Receipt>> {
    let ledger = load(store.state_dir())?.ledger;
    let mut items: Vec<_> = ledger
        .records
        .into_values()
        .map(|record| record.receipt)
        .collect();
    items.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then(a.trash_id.cmp(&b.trash_id))
    });
    Ok(items)
}
pub fn preview(store: &mut ContentStore, id: &str) -> Result<Preview> {
    let (preview, _, _) = collect(store, id)?;
    Ok(preview)
}
pub fn trash(
    store: &mut ContentStore,
    id: &str,
    expected_version: &str,
    request_id: &str,
) -> Result<Receipt> {
    validate_request(request_id)?;
    if !valid_id(id) {
        return Err(invalid("课程节点编号无效。"));
    }
    let loaded = load(store.state_dir())?;
    store.sync_trash_visibility()?;
    let op_key = operation_key("trash", request_id);
    let digest = hash(&serde_json::to_string(
        &json!({"id":id,"version":expected_version}),
    )?);
    if let Some(receipt) = replay(&loaded.ledger, &op_key, "trash", &digest)? {
        return Ok(receipt);
    }
    let (preview, snapshots, parent_id) = collect(store, id)?;
    if preview.version != expected_version {
        return Err(CoreError::new(
            "CONFLICT",
            "待回收的节点或学习记录已变化，请重新确认回收范围。",
        ));
    }
    let mut ledger = loaded.ledger.clone();
    let trash_id = format!("trash_{}", &hash(request_id)[..32]);
    if ledger.records.contains_key(&trash_id) {
        return Err(CoreError::new("CONFLICT", "回收编号已被另一操作使用。"));
    }
    let receipt = Receipt {
        trash_id: trash_id.clone(),
        id: preview.id,
        title: preview.title,
        kind: preview.kind,
        node_count: preview.node_count,
        session_count: preview.session_count,
        created_at: timestamp(),
        restored_at: None,
        status: "trashed".into(),
    };
    ledger.records.insert(
        trash_id.clone(),
        Record {
            receipt: receipt.clone(),
            parent_id,
            snapshots,
        },
    );
    ledger.operations.insert(
        op_key,
        Operation {
            kind: "trash".into(),
            target: trash_id,
            digest,
        },
    );
    let record = ledger.records.get(&receipt.trash_id).unwrap();
    let sync_error = commit(store.state_dir(), &loaded, &ledger, || {
        preflight(store, record).map(|_| ()).map_err(|_| {
            CoreError::new(
                "CONFLICT",
                "待回收的原文件在提交前变化，请重新确认回收范围。",
            )
        })
    })?;
    store.sync_trash_visibility()?;
    if let Some(error) = sync_error {
        return Err(error);
    }
    Ok(receipt)
}
pub fn restore(store: &mut ContentStore, trash_id: &str, request_id: &str) -> Result<Receipt> {
    validate_request(request_id)?;
    if !valid_trash_id(trash_id) {
        return Err(invalid("回收编号无效。"));
    }
    let loaded = load(store.state_dir())?;
    store.sync_trash_visibility()?;
    let op_key = operation_key("restore", request_id);
    let digest = hash(&serde_json::to_string(&json!({"trashId":trash_id}))?);
    if let Some(receipt) = replay(&loaded.ledger, &op_key, "restore", &digest)? {
        return Ok(receipt);
    }
    let record = loaded
        .ledger
        .records
        .get(trash_id)
        .ok_or_else(|| CoreError::new("NOT_FOUND", "找不到这条回收记录。"))?
        .clone();
    let mut documents = Vec::new();
    if record.receipt.status == "trashed" {
        if record
            .parent_id
            .as_ref()
            .is_some_and(|parent| store.is_trashed(parent))
        {
            return Err(CoreError::new(
                "PARENT_TRASHED",
                "请先恢复所属父节点，再恢复这项内容。",
            ));
        }
        documents = preflight(store, &record)?;
        // All originals are rechecked before the single visibility commit; there
        // is no per-file restore step that could leave a partially restored tree.
        let checked = preflight(store, &record)?;
        if checked
            .iter()
            .map(|doc| (&doc.id, &doc.version))
            .ne(documents.iter().map(|doc| (&doc.id, &doc.version)))
        {
            return Err(restore_conflict());
        }
        documents = checked;
    }
    let mut ledger = loaded.ledger.clone();
    let updated = ledger.records.get_mut(trash_id).unwrap();
    if updated.receipt.status == "trashed" {
        updated.receipt.status = "restored".into();
        updated.receipt.restored_at = Some(timestamp());
    }
    let receipt = updated.receipt.clone();
    ledger.operations.insert(
        op_key,
        Operation {
            kind: "restore".into(),
            target: trash_id.into(),
            digest,
        },
    );
    verify_restore_dependencies(&documents, &ledger)?;
    let sync_error = commit(store.state_dir(), &loaded, &ledger, || {
        if record.receipt.status == "trashed" {
            documents = preflight(store, &record)?;
            verify_restore_dependencies(&documents, &ledger)?;
        }
        Ok(())
    })?;
    store.sync_trash_visibility()?;
    store.install_restored(documents);
    if let Some(error) = sync_error {
        return Err(error);
    }
    Ok(receipt)
}

fn collect(store: &mut ContentStore, id: &str) -> Result<(Preview, Vec<Snapshot>, Option<String>)> {
    if !valid_id(id) {
        return Err(invalid("课程节点编号无效。"));
    }
    // Include unopened sessions/entries, not just the last-opened course cache.
    store.refresh()?;
    let root = store.read(id)?;
    let tree = library_tree::list(store)?;
    if !tree.nodes.iter().any(|node| node.id == id) {
        return collect_session(store, &root);
    }
    let mut ids = BTreeSet::from([id.to_owned()]);
    loop {
        let additions: Vec<_> = tree
            .nodes
            .iter()
            .filter(|node| {
                node.parent_id
                    .as_ref()
                    .is_some_and(|parent| ids.contains(parent))
            })
            .map(|node| node.id.clone())
            .filter(|id| !ids.contains(id))
            .collect();
        if additions.is_empty() {
            break;
        }
        ids.extend(additions);
    }
    let node_count = ids.len();
    let mut documents = BTreeMap::new();
    let mut sessions = BTreeSet::new();
    let summaries = store.summaries();
    for summary in &summaries {
        if ids.contains(&summary.id) || summary.kind == "session" {
            let document = store.read(&summary.id)?;
            if ids.contains(&document.id) {
                documents.insert(document.id.clone(), document);
            } else if document
                .metadata
                .extra
                .get("courseId")
                .or_else(|| document.metadata.extra.get("sectionId"))
                .and_then(Value::as_str)
                .is_some_and(|owner| ids.contains(owner))
            {
                sessions.insert(document.id.clone());
                documents.insert(document.id.clone(), document);
            }
        }
    }
    // A moved exercise may still own its ledger in this course's shared session.
    // Do not hide that ledger while another live course still depends on it.
    for node in tree.nodes.iter().filter(|n| n.kind == "exercise" && !ids.contains(&n.id)) {
        let doc = store.read(&node.id)?;
        if doc.metadata.extra.get("originSessionId").and_then(Value::as_str).is_some_and(|id| sessions.contains(id)) {
            return Err(CoreError::new("SOURCE_IN_USE", "其他课程仍有练习依赖原账本，未收纳此课程。请先处理这些练习或将来源材料移回。"));
        }
    }
    let mut entries = BTreeMap::new();
    let origins: Vec<_> = documents
        .values()
        .filter(|doc| doc.metadata.kind == "exercise")
        .filter_map(|doc| {
            Some((
                doc.metadata
                    .extra
                    .get("originSessionId")?
                    .as_str()?
                    .to_owned(),
                doc.metadata
                    .extra
                    .get("exerciseEntryId")?
                    .as_str()?
                    .to_owned(),
            ))
        })
        .collect();
    let origin_sessions: BTreeSet<_> = origins
        .iter()
        .map(|(session, _)| session.as_str())
        .collect();
    for summary in summaries
        .into_iter()
        .filter(|summary| summary.kind == "entry")
    {
        let document = store.read(&summary.id)?;
        if document
            .metadata
            .extra
            .get("sessionId")
            .and_then(Value::as_str)
            .is_some_and(|session| sessions.contains(session))
        {
            documents.insert(document.id.clone(), document);
        } else if document.metadata.extra.get("data").is_some_and(|data| {
            (document.metadata.extra.get("entryKind").and_then(Value::as_str) == Some("error") || data["reviewCard"] == true)
                && data.get("sourceDocumentId").and_then(Value::as_str).is_some_and(|source| ids.contains(source))
        }) { documents.insert(document.id.clone(), document);
        } else if document
            .metadata
            .extra
            .get("sessionId")
            .and_then(Value::as_str)
            .is_some_and(|session| origin_sessions.contains(session))
        {
            entries.insert(document.id.clone(), document);
        }
    }
    collect_exercise_entries(store, &origins, &entries, &mut documents)?;
    let snapshots: Vec<_> = documents
        .into_values()
        .map(|doc| Snapshot {
            id: doc.id,
            path: doc.path,
            version: doc.version,
            kind: doc.metadata.kind,
        })
        .collect();
    let version = hash(&serde_json::to_string(&snapshots)?);
    let parent_id = root
        .metadata
        .extra
        .get("parentId")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .map(str::to_owned);
    Ok((
        Preview {
            id: root.id,
            title: root.metadata.title,
            kind: root.metadata.kind,
            node_count,
            session_count: sessions.len(),
            version,
        },
        snapshots,
        parent_id,
    ))
}
// A course conversation is not a tree node; it enters the same recoverable
// ledger together with its whole entries directory. Section-anchored and
// exploration sessions keep their existing node-attached lifecycle.
fn collect_session(
    store: &mut ContentStore,
    root: &Document,
) -> Result<(Preview, Vec<Snapshot>, Option<String>)> {
    if root.metadata.kind != "session" {
        return Err(invalid("只能回收课程树节点或课程对话。"));
    }
    let owner = root
        .metadata
        .extra
        .get("courseId")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("只能回收课程对话；独立探索与小节会话随所属内容回收。"))?
        .to_owned();
    if !library_tree::list(store)?
        .nodes
        .iter()
        .any(|node| node.id == owner && node.kind == "course")
    {
        return Err(CoreError::new(
            "TRASHED",
            "所属课程不在当前目录，请先恢复课程再处理对话。",
        ));
    }
    // Materialized exercises keep their journal in this session's entries;
    // hiding that ledger would break live exercise views.
    for node in library_tree::list(store)?
        .nodes
        .iter()
        .filter(|node| node.kind == "exercise")
    {
        let doc = store.read(&node.id)?;
        if doc
            .metadata
            .extra
            .get("originSessionId")
            .and_then(Value::as_str)
            == Some(root.id.as_str())
        {
            return Err(CoreError::new(
                "SOURCE_IN_USE",
                "此对话仍被练习用作作答账本，请先处理这些练习；原对话与练习均保留。",
            ));
        }
    }
    let mut documents = BTreeMap::new();
    documents.insert(root.id.clone(), root.clone());
    for summary in store.summaries().into_iter().filter(|s| s.kind == "entry") {
        let document = store.read(&summary.id)?;
        if document
            .metadata
            .extra
            .get("sessionId")
            .and_then(Value::as_str)
            == Some(root.id.as_str())
        {
            documents.insert(document.id.clone(), document);
        }
    }
    let snapshots: Vec<_> = documents
        .into_values()
        .map(|doc| Snapshot {
            id: doc.id,
            path: doc.path,
            version: doc.version,
            kind: doc.metadata.kind,
        })
        .collect();
    let version = hash(&serde_json::to_string(&snapshots)?);
    Ok((
        Preview {
            id: root.id.clone(),
            title: root.metadata.title.clone(),
            kind: "session".into(),
            node_count: 0,
            session_count: 1,
            version,
        },
        snapshots,
        Some(owner),
    ))
}
// A materialized exercise is a child of its source material, but its journal
// lives in that material's session. Only exact structured relationships within
// the recorded origin session extend its deletion scope; arbitrary note links
// or prose never make another note part of an exercise.
fn collect_exercise_entries(
    store: &ContentStore,
    origins: &[(String, String)],
    entries: &BTreeMap<String, Document>,
    documents: &mut BTreeMap<String, Document>,
) -> Result<()> {
    for (session_id, exercise_entry_id) in origins {
        if documents.contains_key(exercise_entry_id) || store.is_trashed(exercise_entry_id) {
            continue;
        }
        let belongs = |doc: &Document| {
            doc.metadata.extra.get("sessionId").and_then(Value::as_str) == Some(session_id.as_str())
        };
        let kind = |doc: &Document| {
            doc.metadata
                .extra
                .get("entryKind")
                .and_then(Value::as_str)
                .unwrap_or("note")
                .to_owned()
        };
        let original = entries
            .get(exercise_entry_id)
            .filter(|doc| belongs(doc) && kind(doc) == "exercise")
            .ok_or_else(|| {
                CoreError::new(
                    "INVALID_DOCUMENT",
                    "练习的原始记录或会话归属无效，未回收任何内容。",
                )
            })?;
        let questions: BTreeSet<_> = original
            .metadata
            .extra
            .get("data")
            .and_then(|data| data.get("exercises"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|question| question.get("id").and_then(Value::as_str))
            .filter(|id| valid_id(id))
            .map(str::to_owned)
            .collect();
        let mut selected = BTreeMap::from([(original.id.clone(), "exercise".to_owned())]);
        loop {
            let mut additions = Vec::new();
            for doc in entries
                .values()
                .filter(|doc| belongs(doc) && !selected.contains_key(&doc.id))
            {
                let entry_kind = kind(doc);
                // A second exercise is an independent object even in the same session.
                if entry_kind == "exercise" {
                    continue;
                }
                let Some(data) = doc.metadata.extra.get("data") else {
                    continue;
                };
                let references = |field: &str, expected: &str| {
                    data.get(field)
                        .and_then(Value::as_str)
                        .is_some_and(|id| selected.get(id).is_some_and(|kind| kind == expected))
                };
                let related = data
                    .get("exerciseId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| questions.contains(id))
                    || references("exerciseEntryId", "exercise")
                    || references("attemptId", "attempt")
                    || references("sourceErrorId", "error")
                    || (data.get("origin").and_then(Value::as_str) == Some("review-state")
                        && references("reviewItemId", "error"));
                if related {
                    additions.push((doc.id.clone(), entry_kind));
                }
            }
            if additions.is_empty() {
                break;
            }
            selected.extend(additions);
        }
        for id in selected.keys() {
            documents.insert(id.clone(), entries[id].clone());
        }
    }
    Ok(())
}
fn preflight(store: &ContentStore, record: &Record) -> Result<Vec<Document>> {
    let mut documents = Vec::new();
    for snapshot in &record.snapshots {
        let document = store
            .read_relative_raw(&snapshot.path)
            .map_err(|_| restore_conflict())?;
        if document.id != snapshot.id
            || document.path != snapshot.path
            || document.version != snapshot.version
            || document.metadata.kind != snapshot.kind
        {
            return Err(restore_conflict());
        }
        documents.push(document);
    }
    Ok(documents)
}
// Check the proposed visibility, not the current hidden set: dependencies restored
// in this same record are allowed. No hidden ledger content is read or restored here.
fn verify_restore_dependencies(documents: &[Document], ledger: &Ledger) -> Result<()> {
    let remaining = visibility(ledger);
    for document in documents.iter().filter(|doc| doc.metadata.kind == "exercise") {
        let field = |key: &str| document.metadata.extra.get(key).and_then(Value::as_str)
            .filter(|id| valid_id(id)).ok_or_else(restore_conflict);
        let session = field("originSessionId")?;
        let entry = field("exerciseEntryId")?;
        for (dependency, path) in [
            (session, format!("sessions/{session}/session.md")),
            (entry, format!("sessions/{session}/entries/{entry}.md")),
        ] {
            if !remaining.ids.contains(dependency) && !remaining.paths.contains(&path) { continue; }
            let blocked = ledger.records.values().find(|record| record.receipt.status == "trashed"
                && record.snapshots.iter().any(|item| item.id == dependency || item.path == path))
                .ok_or_else(corrupt)?;
            return Err(CoreError::new("DEPENDENCY_TRASHED", format!(
                "原练习账本仍在回收站，请先恢复“{}”（{}）后重试；本项仍保留在回收站。",
                blocked.receipt.title, blocked.receipt.id,
            )).details(json!({"documentId":document.id,"originSessionId":session,
                "exerciseEntryId":entry,"dependencyId":dependency,
                "blockingTrashId":blocked.receipt.trash_id,"restoreTargetId":blocked.receipt.id,
                "restoreTargetKind":blocked.receipt.kind})));
        }
    }
    Ok(())
}
fn operation_key(kind: &str, request_id: &str) -> String {
    format!("op_{}", hash(&format!("{kind}\n{request_id}")))
}
fn replay(ledger: &Ledger, key: &str, kind: &str, digest: &str) -> Result<Option<Receipt>> {
    if let Some(operation) = ledger.operations.get(key) {
        if operation.kind != kind || operation.digest != digest {
            return Err(CoreError::new(
                "CONFLICT",
                "防重复编号已被另一份操作参数使用。",
            ));
        }
        return Ok(Some(
            ledger
                .records
                .get(&operation.target)
                .ok_or_else(corrupt)?
                .receipt
                .clone(),
        ));
    }
    Ok(None)
}
fn visibility(ledger: &Ledger) -> Visibility {
    let mut visibility = Visibility::default();
    for record in ledger
        .records
        .values()
        .filter(|record| record.receipt.status == "trashed")
    {
        visibility
            .ids
            .extend(record.snapshots.iter().map(|snapshot| snapshot.id.clone()));
        visibility.paths.extend(
            record
                .snapshots
                .iter()
                .map(|snapshot| snapshot.path.clone()),
        );
    }
    visibility
}
pub(crate) fn load_visibility(state_dir: &Path) -> Result<Visibility> {
    let loaded = load(state_dir)?;
    let mut visible = visibility(&loaded.ledger);
    visible.stamp = loaded.stamp;
    Ok(visible)
}
pub(crate) fn ledger_stamp(state_dir: &Path) -> Result<Option<(u64, SystemTime)>> {
    match fs::symlink_metadata(state_dir.join("library-trash.json")) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.len() > MAX_LEDGER_BYTES
            {
                return Err(corrupt());
            }
            Ok(Some((
                metadata.len(),
                metadata.modified().map_err(CoreError::io)?,
            )))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CoreError::io(error)),
    }
}
fn read_bytes(state_dir: &Path) -> Result<Option<Vec<u8>>> {
    if ledger_stamp(state_dir)?.is_none() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    fs::File::open(state_dir.join("library-trash.json"))
        .map_err(CoreError::io)?
        .take(MAX_LEDGER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(CoreError::io)?;
    if bytes.len() as u64 > MAX_LEDGER_BYTES {
        return Err(corrupt());
    }
    Ok(Some(bytes))
}
fn load(state_dir: &Path) -> Result<Loaded> {
    let before = ledger_stamp(state_dir)?;
    let bytes = read_bytes(state_dir)?;
    let after = ledger_stamp(state_dir)?;
    if before != after {
        return Err(CoreError::new(
            "TRASH_STATE_CHANGED",
            "回收记录在读取期间变化，请刷新后重试。",
        ));
    }
    let Some(bytes) = bytes else {
        return Ok(Loaded {
            ledger: Ledger::default(),
            fingerprint: None,
            stamp: after,
        });
    };
    let ledger: Ledger = serde_json::from_slice(&bytes).map_err(|_| corrupt())?;
    validate_ledger(&ledger)?;
    Ok(Loaded {
        ledger,
        fingerprint: Some(hash(&String::from_utf8(bytes).map_err(|_| corrupt())?)),
        stamp: after,
    })
}

fn validate_ledger(ledger: &Ledger) -> Result<()> {
    if ledger.schema_version != 1
        || ledger.records.len() > MAX_RECORDS
        || ledger.operations.len() > 4 * MAX_RECORDS
    {
        return Err(corrupt());
    }
    let mut active_ids = BTreeSet::new();
    let mut active_paths = BTreeSet::new();
    for (id, record) in &ledger.records {
        let receipt = &record.receipt;
        if id != &receipt.trash_id
            || !valid_trash_id(id)
            || !valid_id(&receipt.id)
            || !receipt_kind(&receipt.kind)
            || receipt.title.trim().is_empty()
            || if receipt.kind == "session" {
                receipt.node_count != 0 || receipt.session_count != 1
            } else {
                receipt.node_count == 0
            }
            || record.snapshots.len() > 4096
            || !matches!(receipt.status.as_str(), "trashed" | "restored")
            || (receipt.status == "trashed") != receipt.restored_at.is_none()
            || record.parent_id.as_ref().is_some_and(|id| !valid_id(id))
        {
            return Err(corrupt());
        }
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let mut nodes = 0;
        let mut sessions = 0;
        for snapshot in &record.snapshots {
            let path = Path::new(&snapshot.path);
            if !valid_id(&snapshot.id)
                || !sha(&snapshot.version, 64)
                || !ids.insert(snapshot.id.clone())
                || !paths.insert(snapshot.path.clone())
                || snapshot.path.is_empty()
                || snapshot.path.contains(['\\', ':'])
                || path.is_absolute()
                || path
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
                || !path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("md"))
                || !(node_kind(&snapshot.kind)
                    || matches!(snapshot.kind.as_str(), "session" | "entry"))
            {
                return Err(corrupt());
            }
            if node_kind(&snapshot.kind) {
                nodes += 1;
            }
            if snapshot.kind == "session" {
                sessions += 1;
            }
            if receipt.status == "trashed"
                && (!active_ids.insert(snapshot.id.clone())
                    || !active_paths.insert(snapshot.path.clone()))
            {
                return Err(corrupt());
            }
        }
        if !ids.contains(&receipt.id)
            || !record
                .snapshots
                .iter()
                .any(|snapshot| snapshot.id == receipt.id && snapshot.kind == receipt.kind)
            || nodes != receipt.node_count
            || sessions != receipt.session_count
        {
            return Err(corrupt());
        }
    }
    for (key, operation) in &ledger.operations {
        if !key.strip_prefix("op_").is_some_and(|value| sha(value, 64))
            || !matches!(operation.kind.as_str(), "trash" | "restore")
            || !sha(&operation.digest, 64)
            || !ledger.records.contains_key(&operation.target)
        {
            return Err(corrupt());
        }
    }
    Ok(())
}
/// None means the atomic commit and directory sync succeeded. A directory-sync
/// error still reports failure, but callers install the committed visibility so
/// an acknowledgment failure cannot reopen the hidden files in memory.
fn commit(
    state_dir: &Path,
    loaded: &Loaded,
    ledger: &Ledger,
    verify_originals: impl FnOnce() -> Result<()>,
) -> Result<Option<CoreError>> {
    // The persistent lock inode is never renamed or removed. Across processes
    // it serializes fingerprint comparison and replacement of the ledger.
    let _lock = lock_ledger(state_dir)?;
    validate_ledger(ledger)?;
    let bytes = serde_json::to_vec_pretty(ledger)?;
    if bytes.len() as u64 > MAX_LEDGER_BYTES {
        return Err(CoreError::new(
            "LIMIT_EXCEEDED",
            "回收记录达到容量上限，请保留原文件并联系维护。",
        ));
    }
    let mut temp = tempfile::NamedTempFile::new_in(state_dir).map_err(CoreError::io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(CoreError::io)?;
    }
    temp.write_all(&bytes).map_err(CoreError::io)?;
    temp.as_file().sync_all().map_err(CoreError::io)?;
    verify_originals()?;
    let current = read_bytes(state_dir)?
        .map(|bytes| String::from_utf8(bytes).map(|text| hash(&text)))
        .transpose()
        .map_err(|_| corrupt())?;
    if current != loaded.fingerprint {
        return Err(CoreError::new(
            "CONFLICT",
            "回收记录已在其他位置更改，请刷新后重试。",
        ));
    }
    temp.persist(state_dir.join("library-trash.json"))
        .map_err(|error| CoreError::io(error.error))?;
    #[cfg(unix)]
    {
        return Ok(fs::File::open(state_dir)
            .and_then(|directory| directory.sync_all())
            .err()
            .map(CoreError::io));
    }
    #[cfg(not(unix))]
    {
        Ok(None)
    }
}
fn lock_ledger(state_dir: &Path) -> Result<fs::File> {
    let path = state_dir.join("library-trash.lock");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(corrupt())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(CoreError::io(error)),
    }
    let mut create = fs::OpenOptions::new();
    create.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        create.mode(0o600);
    }
    let file = match create.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Do not create through an existing symlink. Opening an existing
            // lock performs no writes; validate its actual inode before locking.
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(CoreError::io)?
        }
        Err(error) => return Err(CoreError::io(error)),
    };
    let actual = file.metadata().map_err(CoreError::io)?;
    let named = fs::symlink_metadata(&path).map_err(CoreError::io)?;
    if !actual.is_file() || !named.is_file() || named.file_type().is_symlink() {
        return Err(corrupt());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if actual.dev() != named.dev() || actual.ino() != named.ino() {
            return Err(corrupt());
        }
    }
    file.try_lock().map_err(|error| match error {
        fs::TryLockError::WouldBlock => {
            CoreError::new("BUSY", "另一进程正在更新回收记录，请稍后重试。")
        }
        fs::TryLockError::Error(error) => CoreError::io(error),
    })?;
    Ok(file)
}
fn node_kind(kind: &str) -> bool {
    matches!(
        kind,
        "course" | "folder" | "section" | "knowledge" | "exploration" | "exercise"
    )
}
/// A session record targets one course conversation instead of a tree node.
fn receipt_kind(kind: &str) -> bool {
    node_kind(kind) || kind == "session"
}
fn sha(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn valid_trash_id(id: &str) -> bool {
    id.strip_prefix("trash_")
        .is_some_and(|value| sha(value, 32))
}
fn validate_request(id: &str) -> Result<()> {
    if id.len() < 8 || id.len() > 128 || id.chars().any(char::is_control) {
        Err(invalid("操作缺少有效防重复编号。"))
    } else {
        Ok(())
    }
}
fn invalid(message: &str) -> CoreError {
    CoreError::new("INVALID_INPUT", message)
}
fn corrupt() -> CoreError {
    CoreError::new(
        "TRASH_LEDGER_INVALID",
        "回收记录损坏或路径不安全；未恢复为未删除视图，请先保留运行数据。",
    )
}
fn restore_conflict() -> CoreError {
    CoreError::new(
        "TRASH_CONFLICT",
        "回收内容的原文件缺失或已在外部修改；未恢复任何节点，原文件保持原样。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{exercises, review, sessions};
    fn fixture() -> (
        tempfile::TempDir,
        ContentStore,
        Document,
        Document,
        String,
        String,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mut store =
            ContentStore::new(dir.path().join("library"), dir.path().join("runtime")).unwrap();
        let course = library_tree::create(
            &mut store,
            "course",
            "Course",
            None,
            "Course material",
            "trash-fixture-course",
        )
        .unwrap();
        let child = library_tree::create(
            &mut store,
            "section",
            "Lesson",
            Some(&course.id),
            "ORIGINAL_SECTION_BODY",
            "trash-fixture-child",
        )
        .unwrap();
        let session = sessions::create(&mut store, &child.id, &[]).unwrap();
        let saved = sessions::append(
            &mut store,
            &session.id,
            Entry {
                id: String::new(),
                kind: "note".into(),
                content: "SAVED_LEARNING_NOTE".into(),
                created_at: String::new(),
                selection: None,
                related_ids: vec![child.id.clone()],
                data: None,
            },
            None,
            "trash-fixture-note",
        )
        .unwrap();
        let entry = saved.entries[0].id.clone();
        (dir, store, course, child, session.id, entry)
    }
    #[test]
    fn subtree_sessions_entries_assets_and_original_ids_restore_byte_for_byte() {
        let (dir, mut store, course, child, session, entry) = fixture();
        fs::create_dir_all(store.root().join("assets/course")).unwrap();
        let asset = store.root().join("assets/course/source.bin");
        fs::write(&asset, b"\x00ASSET_BYTES\xff").unwrap();
        let versions: BTreeMap<_, _> = store
            .summaries()
            .into_iter()
            .map(|item| {
                let doc = store.read(&item.id).unwrap();
                (doc.id, doc.version)
            })
            .collect();
        let scope = preview(&mut store, &course.id).unwrap();
        assert_eq!(scope.node_count, 2);
        assert_eq!(scope.session_count, 1);
        let receipt = trash(
            &mut store,
            &course.id,
            &scope.version,
            "trash-course-request",
        )
        .unwrap();
        assert_eq!(receipt.status, "trashed");
        assert_eq!(list(&store).unwrap().len(), 1);
        for id in [&course.id, &child.id, &session, &entry] {
            assert_eq!(store.read(id).unwrap_err().code, "TRASHED");
        }
        assert_eq!(
            store.read_relative(&child.path).unwrap_err().code,
            "TRASHED"
        );
        assert_eq!(store.register(&child.path).unwrap_err().code, "TRASHED");
        assert_eq!(
            store
                .save(&child.id, &child.content, &child.version)
                .unwrap_err()
                .code,
            "TRASHED"
        );
        assert!(store.summaries().is_empty());
        assert!(library_tree::list(&store).unwrap().nodes.is_empty());
        assert_eq!(fs::read(&asset).unwrap(), b"\x00ASSET_BYTES\xff");
        drop(store);
        let mut reopened =
            ContentStore::new(dir.path().join("library"), dir.path().join("runtime")).unwrap();
        assert!(reopened.is_trashed(&child.id));
        reopened.refresh().unwrap();
        assert!(reopened.summaries().is_empty());
        assert_eq!(
            trash(
                &mut reopened,
                &course.id,
                &scope.version,
                "trash-course-request"
            )
            .unwrap(),
            receipt
        );
        let restored = restore(&mut reopened, &receipt.trash_id, "restore-course-request").unwrap();
        assert_eq!(restored.status, "restored");
        assert_eq!(
            restore(&mut reopened, &receipt.trash_id, "restore-course-request").unwrap(),
            restored
        );
        // Simulate losing the successful restore acknowledgment and restarting.
        drop(reopened);
        let mut reopened =
            ContentStore::new(dir.path().join("library"), dir.path().join("runtime")).unwrap();
        assert_eq!(
            restore(&mut reopened, &receipt.trash_id, "restore-course-request").unwrap(),
            restored
        );
        reopened.refresh().unwrap();
        for (id, version) in versions {
            assert_eq!(reopened.read(&id).unwrap().version, version);
        }
        assert_eq!(fs::read(&asset).unwrap(), b"\x00ASSET_BYTES\xff");
        assert_eq!(
            sessions::get(&mut reopened, &session).unwrap().entries[0].related_ids,
            vec![child.id]
        );
    }
    #[test]
    fn preview_hash_detects_new_children_and_modified_learning_entries() {
        let (_dir, mut store, course, child, _session, entry) = fixture();
        let before = preview(&mut store, &course.id).unwrap();
        library_tree::create(
            &mut store,
            "section",
            "New child",
            Some(&course.id),
            "new",
            "trash-new-child",
        )
        .unwrap();
        assert_eq!(
            trash(&mut store, &course.id, &before.version, "trash-stale-scope")
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        let before = preview(&mut store, &course.id).unwrap();
        let note = store.read(&entry).unwrap();
        store
            .save(
                &entry,
                &note
                    .content
                    .replace("SAVED_LEARNING_NOTE", "EXTERNALLY_CHANGED_NOTE"),
                &note.version,
            )
            .unwrap();
        assert_eq!(
            trash(&mut store, &course.id, &before.version, "trash-stale-entry")
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert!(list(&store).unwrap().is_empty());
        assert!(!store.is_trashed(&child.id));
    }
    #[test]
    fn changed_or_missing_original_prevents_the_entire_restore_atomically() {
        let (_dir, mut store, course, child, _session, _entry) = fixture();
        let scope = preview(&mut store, &course.id).unwrap();
        let receipt = trash(
            &mut store,
            &course.id,
            &scope.version,
            "trash-conflict-original",
        )
        .unwrap();
        let path = store.root().join(&child.path);
        fs::write(
            &path,
            child
                .content
                .replace("ORIGINAL_SECTION_BODY", "EXTERNAL_EDIT"),
        )
        .unwrap();
        let ledger_before = fs::read(store.state_dir().join("library-trash.json")).unwrap();
        assert_eq!(
            restore(&mut store, &receipt.trash_id, "restore-external-edit")
                .unwrap_err()
                .code,
            "TRASH_CONFLICT"
        );
        assert_eq!(
            fs::read(store.state_dir().join("library-trash.json")).unwrap(),
            ledger_before
        );
        assert!(store.is_trashed(&course.id));
        assert!(store.is_trashed(&child.id));
        fs::remove_file(&path).unwrap();
        assert_eq!(
            restore(&mut store, &receipt.trash_id, "restore-missing-file")
                .unwrap_err()
                .code,
            "TRASH_CONFLICT"
        );
        assert_eq!(
            fs::read(store.state_dir().join("library-trash.json")).unwrap(),
            ledger_before
        );
        fs::write(path, &child.content).unwrap();
        assert_eq!(
            restore(&mut store, &receipt.trash_id, "restore-repaired-file")
                .unwrap()
                .status,
            "restored"
        );
    }
    #[test]
    fn independent_child_recycle_requires_parent_restore_and_keeps_its_lifecycle() {
        let (_dir, mut store, course, child, _session, _entry) = fixture();
        let scope = preview(&mut store, &child.id).unwrap();
        let child_receipt = trash(
            &mut store,
            &child.id,
            &scope.version,
            "trash-independent-child",
        )
        .unwrap();
        let scope = preview(&mut store, &course.id).unwrap();
        assert_eq!(scope.node_count, 1);
        assert_eq!(scope.session_count, 0);
        let parent_receipt = trash(
            &mut store,
            &course.id,
            &scope.version,
            "trash-independent-parent",
        )
        .unwrap();
        assert_eq!(
            restore(&mut store, &child_receipt.trash_id, "restore-child-first")
                .unwrap_err()
                .code,
            "PARENT_TRASHED"
        );
        restore(&mut store, &parent_receipt.trash_id, "restore-parent-first").unwrap();
        assert!(store.is_trashed(&child.id));
        restore(&mut store, &child_receipt.trash_id, "restore-child-second").unwrap();
        assert!(!store.is_trashed(&child.id));
        assert_eq!(
            restore(&mut store, &child_receipt.trash_id, "restore-parent-first")
                .unwrap_err()
                .code,
            "CONFLICT"
        );
    }
    #[test]
    fn corrupt_or_symlink_ledger_fails_closed_without_touching_originals() {
        let (dir, mut store, course, child, _session, _entry) = fixture();
        fs::write(store.state_dir().join("library-trash.json"), b"{broken").unwrap();
        assert!(store.refresh().is_err());
        assert!(list(&store).is_err());
        assert!(store.summaries().is_empty());
        assert!(store.read(&course.id).is_err());
        assert!(ContentStore::new(dir.path().join("library"), dir.path().join("runtime")).is_err());
        assert_eq!(
            fs::read_to_string(store.root().join(&child.path)).unwrap(),
            child.content
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let ledger = store.state_dir().join("library-trash.json");
            fs::remove_file(&ledger).unwrap();
            let outside = dir.path().join("outside-ledger.json");
            fs::write(&outside, b"{}").unwrap();
            symlink(&outside, &ledger).unwrap();
            assert!(list(&store).is_err());
            assert!(store.refresh().is_err());
            assert_eq!(fs::read(&outside).unwrap(), b"{}");
        }
    }
    #[test]
    fn exercise_child_hides_only_its_exact_journal_closure_and_restores_original_bytes() {
        let (dir, mut store, _course, section, session, unrelated_note) = fixture();
        let mut exercises = Vec::new();
        for index in 0..2 {
            let created = exercises::create_manual(
                &mut store,
                &session,
                &section.id,
                exercises::ManualExercise {
                    question: format!("Question {index}"),
                    format: "exact".into(),
                    answers: vec!["correct".into()],
                    explanation: "Reference explanation".into(),
                },
                &format!("exercise-trash-manual-{index}"),
            )
            .unwrap();
            let exercise_id = created.exercises["exercises"][0]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let exercise_entry_id = created.document.metadata.extra["exerciseEntryId"]
                .as_str()
                .unwrap()
                .to_owned();
            sessions::append(&mut store, &session, Entry { id: String::new(), kind: "attempt".into(),
                content: "wrong".into(), created_at: String::new(), selection: None,
                related_ids: vec![exercise_entry_id.clone()],
                data: Some(json!({"exerciseId":exercise_id,"exerciseEntryId":exercise_entry_id,"answer":"wrong"})) },
                None, &format!("exercise-trash-attempt-{index}")).unwrap();
            let error = sessions::get(&mut store, &session)
                .unwrap()
                .entries
                .into_iter()
                .find(|entry| {
                    entry.kind == "error"
                        && entry.data.as_ref().unwrap()["exerciseId"] == exercise_id
                })
                .unwrap();
            review::set(
                &mut store,
                &error.id,
                "understood",
                &format!("exercise_trash_review_{index}"),
            )
            .unwrap();
            exercises.push((created.document, exercise_id, error.id));
        }
        // Plain user notes are independent even if linked to the deleted error.
        let saved = sessions::append(
            &mut store,
            &session,
            Entry {
                id: String::new(),
                kind: "note".into(),
                content: "Independent user note".into(),
                created_at: String::new(),
                selection: None,
                related_ids: vec![exercises[0].2.clone()],
                data: None,
            },
            None,
            "exercise-unrelated-linked-note",
        )
        .unwrap();
        let independent_note = saved
            .entries
            .iter()
            .find(|entry| entry.content == "Independent user note")
            .unwrap()
            .id
            .clone();
        let before: BTreeMap<_, _> = store
            .summaries()
            .into_iter()
            .map(|item| {
                let doc = store.read(&item.id).unwrap();
                (doc.id, (doc.path, doc.content))
            })
            .collect();
        let first_entry = exercises[0].0.metadata.extra["exerciseEntryId"]
            .as_str()
            .unwrap()
            .to_owned();
        let scope = preview(&mut store, &exercises[0].0.id).unwrap();
        assert_eq!((scope.node_count, scope.session_count), (1, 0));
        let receipt = trash(
            &mut store,
            &exercises[0].0.id,
            &scope.version,
            "exercise-child-trash-request",
        )
        .unwrap();
        assert!(!store.is_trashed(&session));
        assert!(!store.is_trashed(&section.id));
        assert!(!store.is_trashed(&unrelated_note));
        assert!(!store.is_trashed(&independent_note));
        assert!(store.is_trashed(&first_entry));
        assert!(store.is_trashed(&exercises[0].2));
        let active = sessions::get(&mut store, &session).unwrap();
        assert_eq!(
            active
                .entries
                .iter()
                .filter(|entry| entry.kind == "exercise")
                .count(),
            1
        );
        for entry in active
            .entries
            .iter()
            .filter(|entry| matches!(entry.kind.as_str(), "attempt" | "feedback" | "error"))
        {
            assert_eq!(entry.data.as_ref().unwrap()["exerciseId"], exercises[1].1);
        }
        let items = review::list(&mut store, "").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], exercises[1].2);
        assert_eq!(items[0]["reviewState"]["status"], "understood");
        let record = &load(store.state_dir()).unwrap().ledger.records[&receipt.trash_id];
        assert_eq!(record.snapshots.len(), 6); // child, exercise, attempt, feedback, error, review receipt
        drop(store);
        let mut store =
            ContentStore::new(dir.path().join("library"), dir.path().join("runtime")).unwrap();
        assert_eq!(
            sessions::get(&mut store, &session)
                .unwrap()
                .entries
                .iter()
                .filter(|entry| entry.kind == "exercise")
                .count(),
            1
        );
        restore(
            &mut store,
            &receipt.trash_id,
            "exercise-child-restore-request",
        )
        .unwrap();
        store.refresh().unwrap();
        for (id, (path, content)) in before {
            assert_eq!(store.read(&id).unwrap().content, content);
            assert_eq!(
                fs::read_to_string(store.root().join(path)).unwrap(),
                content
            );
        }
        assert_eq!(review::list(&mut store, "").unwrap().len(), 2);
    }

    #[test]
    fn course_conversation_recovers_with_history_and_neighbor_chats_stay_visible() {
        let (dir, mut store, course, child, _session, _entry) = fixture();
        let first = sessions::create_course(
            &mut store,
            "",
            &[],
            "chat_trash_first",
            &course.id,
            &[],
        )
        .unwrap();
        let second = sessions::create_course(
            &mut store,
            "",
            &[],
            "chat_trash_second",
            &course.id,
            &[],
        )
        .unwrap();
        let saved = sessions::append(
            &mut store,
            &first.id,
            Entry {
                id: String::new(),
                kind: "note".into(),
                content: "FIRST_CHAT_NOTE".into(),
                created_at: String::new(),
                selection: None,
                related_ids: vec![],
                data: None,
            },
            None,
            "trash-session-first-note",
        )
        .unwrap();
        let entry = saved.entries[0].id.clone();
        let stale = preview(&mut store, &first.id).unwrap();
        sessions::append(
            &mut store,
            &first.id,
            Entry {
                id: String::new(),
                kind: "note".into(),
                content: "SECOND_CHAT_NOTE".into(),
                created_at: String::new(),
                selection: None,
                related_ids: vec![],
                data: None,
            },
            None,
            "trash-session-second-note",
        )
        .unwrap();
        assert_eq!(
            trash(&mut store, &first.id, &stale.version, "trash-session-stale")
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        let scope = preview(&mut store, &first.id).unwrap();
        assert_eq!(scope.kind, "session");
        assert_eq!(scope.node_count, 0);
        assert_eq!(scope.session_count, 1);
        let receipt = trash(
            &mut store,
            &first.id,
            &scope.version,
            "trash-session-request",
        )
        .unwrap();
        assert_eq!(receipt.kind, "session");
        assert_eq!(
            trash(&mut store, &first.id, &scope.version, "trash-session-request").unwrap(),
            receipt
        );
        assert_eq!(store.read(&first.id).unwrap_err().code, "TRASHED");
        assert_eq!(store.read(&entry).unwrap_err().code, "TRASHED");
        assert!(store.read(&child.id).is_ok());
        assert!(!store.is_trashed(&second.id));
        assert_eq!(
            sessions::get(&mut store, &second.id).unwrap().id,
            second.id
        );
        let course_scope = preview(&mut store, &course.id).unwrap();
        assert_eq!(course_scope.node_count, 2);
        // The fixture's section-anchored session plus the surviving chat; the
        // hidden conversation must not be counted or collected again.
        assert_eq!(course_scope.session_count, 2);
        drop(store);
        let mut reopened =
            ContentStore::new(dir.path().join("library"), dir.path().join("runtime")).unwrap();
        assert!(reopened.is_trashed(&first.id));
        let restored = restore(
            &mut reopened,
            &receipt.trash_id,
            "restore-session-request",
        )
        .unwrap();
        assert_eq!(restored.status, "restored");
        assert_eq!(
            restore(
                &mut reopened,
                &receipt.trash_id,
                "restore-session-request"
            )
            .unwrap(),
            restored
        );
        let back = sessions::get(&mut reopened, &first.id).unwrap();
        assert_eq!(back.title, first.title);
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.entries[0].content, "FIRST_CHAT_NOTE");
    }
    #[test]
    fn conversation_trash_waits_for_dependent_exercise_ledgers_and_restore_orders_dependencies() {
        let (_dir, mut store, course, child, _session, _entry) = fixture();
        let chat = sessions::create_course(
            &mut store,
            "",
            &[],
            "chat_trash_dep",
            &course.id,
            &[],
        )
        .unwrap();
        let created = exercises::create_manual(
            &mut store,
            &chat.id,
            &child.id,
            exercises::ManualExercise {
                question: "Trash dependency question?".into(),
                format: "exact".into(),
                answers: vec!["answer".into()],
                explanation: "Synthetic explanation".into(),
            },
            "trash-session-exercise",
        )
        .unwrap();
        assert_eq!(
            preview(&mut store, &chat.id).unwrap_err().code,
            "SOURCE_IN_USE"
        );
        let exercise_scope = preview(&mut store, &created.document.id).unwrap();
        let exercise_receipt = trash(
            &mut store,
            &created.document.id,
            &exercise_scope.version,
            "trash-session-exercise-node",
        )
        .unwrap();
        let chat_scope = preview(&mut store, &chat.id).unwrap();
        let chat_receipt = trash(
            &mut store,
            &chat.id,
            &chat_scope.version,
            "trash-session-chat-after-exercise",
        )
        .unwrap();
        assert_eq!(
            restore(
                &mut store,
                &exercise_receipt.trash_id,
                "restore-exercise-first"
            )
            .unwrap_err()
            .code,
            "DEPENDENCY_TRASHED"
        );
        restore(&mut store, &chat_receipt.trash_id, "restore-chat-first").unwrap();
        restore(
            &mut store,
            &exercise_receipt.trash_id,
            "restore-exercise-second",
        )
        .unwrap();
        assert!(store.read(&created.document.id).is_ok());
        assert_eq!(
            exercises::get(&mut store, &created.document.id)
                .unwrap()
                .session
                .id,
            chat.id
        );
    }

    #[test]
    fn ledger_os_lock_prevents_other_store_commit_and_rejects_stale_fingerprint() {
        let (dir, mut store, course, child, _session, _entry) = fixture();
        let scope = preview(&mut store, &course.id).unwrap();
        let stale = load(store.state_dir()).unwrap();
        let lock = lock_ledger(store.state_dir()).unwrap();
        let mut other =
            ContentStore::new(dir.path().join("library"), dir.path().join("runtime")).unwrap();
        assert_eq!(
            trash(
                &mut other,
                &course.id,
                &scope.version,
                "trash-lock-busy-request"
            )
            .unwrap_err()
            .code,
            "BUSY"
        );
        assert!(list(&store).unwrap().is_empty());
        assert_eq!(store.read(&child.id).unwrap().content, child.content);
        drop(lock);
        let receipt = trash(
            &mut store,
            &course.id,
            &scope.version,
            "trash-lock-success-request",
        )
        .unwrap();
        let committed = fs::read(store.state_dir().join("library-trash.json")).unwrap();
        assert_eq!(
            commit(store.state_dir(), &stale, &stale.ledger, || Ok(()))
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert_eq!(
            fs::read(store.state_dir().join("library-trash.json")).unwrap(),
            committed
        );
        assert_eq!(list(&store).unwrap()[0], receipt);
        let lock = lock_ledger(store.state_dir()).unwrap();
        assert_eq!(
            restore(&mut store, &receipt.trash_id, "restore-lock-busy-request")
                .unwrap_err()
                .code,
            "BUSY"
        );
        assert!(store.is_trashed(&course.id));
        drop(lock);
        restore(
            &mut store,
            &receipt.trash_id,
            "restore-lock-success-request",
        )
        .unwrap();
    }
}
