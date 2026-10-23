//! A course's physical workspace. Materials have one editable location. Legacy
//! flat courses are read in place and reach the project layout only through the
//! explicit ensure_project() migration on the first freeze write path.
use crate::{
    content::ContentStore,
    types::{CoreError, Document, Metadata, Result},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

const MIGRATION_STAGING: &str = ".migration-staging";
const MIGRATION_BACKUP: &str = ".migration-backup";
// Mirror the asset service's private receipt/body caps: exceeding either means
// the legacy layout is not what this migration understands.
const MAX_MIGRATION_RECEIPT: usize = 16 * 1024;
const MAX_MIGRATION_ASSET_BODY: usize = 5 * 1024 * 1024;

pub fn course_project_root(library_root: &Path, course_id: &str) -> Result<PathBuf> {
    if !crate::types::valid_id(course_id) {
        return Err(unsafe_path());
    }
    let root = library_root.canonicalize()?;
    let mut path = root.clone();
    for part in ["courses", course_id, "materials"] {
        path.push(part);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(required()),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(unsafe_path());
        }
        if !path.canonicalize()?.starts_with(&root) {
            return Err(unsafe_path());
        }
    }
    Ok(path.parent().unwrap().canonicalize()?)
}

pub fn create_material(
    store: &mut ContentStore,
    metadata: Metadata,
    body: &str,
) -> Result<Document> {
    let relative = if metadata.kind == "course" {
        if metadata.extra.get("parentId").is_some_and(|v| !v.is_null()) {
            return Err(unsafe_path());
        }
        material_path(&metadata.id, &metadata.id)?
    } else {
        routed_path(store, &metadata)?.ok_or_else(required)?
    };
    store.create(metadata, body, &relative)
}

pub(crate) fn material_path(course: &str, id: &str) -> Result<String> {
    if !crate::types::valid_id(course) || !crate::types::valid_id(id) {
        return Err(unsafe_path());
    }
    Ok(format!("courses/{course}/materials/documents/{id}.md"))
}

/// Idempotent: a course that already owns a project layout returns its root
/// unchanged; a legacy flat course migrates into `courses/<id>/materials/` on
/// the first call and then returns that root. Stable IDs, bytes and therefore
/// content versions survive verbatim. Documents map by frontmatter ID to
/// `documents/<id>.md`; legacy attachments map from
/// `assets/<course>/<assetId>/{metadata.json, content.*}` to
/// `materials/assets/<course>/<assetId>/{.asset.json, <escapedName>}`.
///
/// New copies are written to hidden `.migration-staging/` first; only after a
/// full read-back does each old file move to `.migration-backup/` inside the
/// course root. Originals are never silently deleted, and an interruption
/// never leaves two indexable copies of one document — a retry resumes from
/// the staging directory and completes the migration.
pub fn ensure_project(store: &mut ContentStore, course_id: &str) -> Result<PathBuf> {
    if !crate::types::valid_id(course_id) {
        return Err(unsafe_path());
    }
    let library_root = store.root().to_path_buf();
    let course_root = library_root.join("courses").join(course_id);
    let staging = course_root.join(MIGRATION_STAGING);
    let root_target = material_path(course_id, course_id)?;
    let mut root = store.read(course_id);
    if root
        .as_ref()
        .is_err_and(|error| matches!(error.code.as_str(), "NOT_FOUND" | "IO"))
    {
        // An interrupted migration can leave the root only in staging while a
        // stale index still names the vanished flat file; refresh and retry.
        root = store.refresh().and_then(|_| store.read(course_id));
    }
    let root = match root {
        Ok(document) => Some(document),
        Err(error) => {
            if staged_root_bytes(&staging, course_id)?.is_none() {
                return Err(error);
            }
            None
        }
    };
    if root
        .as_ref()
        .is_some_and(|document| document.path == root_target)
    {
        // Already a physical project: no writes and no index churn.
        return course_project_root(&library_root, course_id);
    }
    // Members resolve by loose parentId attribution, mirroring the tree view.
    // Trashed members keep their recorded paths so trash restore preflight
    // still finds the originals, and documents inside another physical course
    // never move with this one.
    let mut documents: BTreeMap<String, DocMigration> = BTreeMap::new();
    for summary in store.summaries() {
        if !visible_kind(&summary.kind) {
            continue;
        }
        let Some(document) = loose_member(store, &summary.id, course_id)? else {
            continue;
        };
        if project_course(store, &document.path, &document.id)?
            .is_some_and(|owner| owner != course_id)
        {
            continue;
        }
        let target = material_path(course_id, &document.id)?;
        documents.insert(
            document.id.clone(),
            DocMigration {
                bytes: document.content.into_bytes(),
                old: (document.path != target).then_some(document.path.clone()),
                staged: false,
            },
        );
    }
    merge_staged_documents(&staging, &mut documents)?;
    let mut assets: BTreeMap<String, AssetMigration> = BTreeMap::new();
    if root.is_some() {
        // The indexed flat root means legacy attachments are the visible copies.
        for asset in crate::assets::list(store, course_id)? {
            let legacy = library_root.join("assets").join(course_id).join(&asset.id);
            let receipt =
                regular_file_bytes(&legacy.join("metadata.json"), MAX_MIGRATION_RECEIPT)?
                    .ok_or_else(unsafe_path)?;
            let (old_body_name, body) = sole_body_file(&legacy, "metadata.json")?;
            assets.insert(
                asset.id.clone(),
                AssetMigration {
                    receipt,
                    body,
                    body_name: crate::assets::editable_name(&asset.file_name),
                    old: Some(legacy),
                    old_body_name: Some(old_body_name),
                    staged: false,
                },
            );
        }
    }
    merge_staged_assets(&staging, course_id, &mut assets)?;
    // Precheck every destination before writing anything: an occupied or
    // mismatched target aborts with the flat originals untouched.
    if let Ok(metadata) = fs::symlink_metadata(&course_root) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(unsafe_path());
        }
    }
    for (id, unit) in &documents {
        let final_path = library_root.join(material_path(course_id, id)?);
        if let Some(existing) =
            regular_file_bytes(&final_path, crate::content::MAX_DOCUMENT_BYTES as usize)?
        {
            if existing != unit.bytes {
                return Err(migration_conflict());
            }
        }
    }
    for (id, unit) in &assets {
        let final_dir = asset_final_dir(&library_root, course_id, id);
        if let Some(files) = asset_directory(&final_dir)? {
            if files.len() != 2
                || files.get(".asset.json").map(|v| v.as_slice()) != Some(unit.receipt.as_slice())
                || files.get(&unit.body_name).map(|v| v.as_slice()) != Some(unit.body.as_slice())
            {
                return Err(migration_conflict());
            }
        }
    }
    // Stage every copy inside the hidden directory, receipt written last for
    // assets so a partial write is recognizable and redoable on retry.
    for (id, unit) in &documents {
        if unit.staged {
            continue;
        }
        let staged = ensure_directory(
            &library_root,
            &format!("courses/{course_id}/{MIGRATION_STAGING}/documents"),
        )?
        .join(format!("{id}.md"));
        durable_write(&staged, &unit.bytes)?;
    }
    for (id, unit) in &assets {
        if unit.staged {
            continue;
        }
        let dir = ensure_directory(
            &library_root,
            &format!("courses/{course_id}/{MIGRATION_STAGING}/assets/{course_id}/{id}"),
        )?;
        durable_write(&dir.join(&unit.body_name), &unit.body)?;
        durable_write(&dir.join(".asset.json"), &unit.receipt)?;
    }
    for (id, unit) in &documents {
        let staged = staging.join("documents").join(format!("{id}.md"));
        if let Some(bytes) =
            regular_file_bytes(&staged, crate::content::MAX_DOCUMENT_BYTES as usize)?
        {
            if bytes != unit.bytes {
                return Err(migration_conflict());
            }
        }
    }
    for (id, unit) in &assets {
        let staged_dir = staging.join("assets").join(course_id).join(id);
        if let Some(files) = asset_directory(&staged_dir)? {
            if files.get(".asset.json").map(|v| v.as_slice()) != Some(unit.receipt.as_slice())
                || files.get(&unit.body_name).map(|v| v.as_slice()) != Some(unit.body.as_slice())
            {
                return Err(migration_conflict());
            }
        }
    }
    // Flip assets first, then documents with the course anchor last, so the
    // old visible copy always leaves the indexable tree before its verified
    // replacement appears.
    if !assets.is_empty() {
        ensure_directory(
            &library_root,
            &format!("courses/{course_id}/materials/assets/{course_id}"),
        )?;
    }
    for (id, unit) in &assets {
        let final_dir = asset_final_dir(&library_root, course_id, id);
        let staged_dir = staging.join("assets").join(course_id).join(id);
        if let Some(legacy) = &unit.old {
            if fs::symlink_metadata(legacy).is_ok() {
                let body_name = unit.old_body_name.as_deref().ok_or_else(unsafe_path)?;
                if regular_file_bytes(&legacy.join("metadata.json"), MAX_MIGRATION_RECEIPT)?
                    != Some(unit.receipt.clone())
                    || regular_file_bytes(&legacy.join(body_name), MAX_MIGRATION_ASSET_BODY)?
                        != Some(unit.body.clone())
                {
                    return Err(migration_conflict());
                }
                let backup_target = ensure_directory(
                    &library_root,
                    &format!("courses/{course_id}/{MIGRATION_BACKUP}/assets/{course_id}"),
                )?
                .join(id);
                if fs::symlink_metadata(&backup_target).is_ok() {
                    return Err(migration_conflict());
                }
                fs::rename(legacy, &backup_target).map_err(CoreError::io)?;
            }
        }
        if fs::symlink_metadata(&final_dir).is_ok() {
            let _ = fs::remove_dir_all(&staged_dir);
        } else {
            fs::rename(&staged_dir, &final_dir).map_err(CoreError::io)?;
        }
    }
    for (id, unit) in &documents {
        if id.as_str() == course_id {
            continue;
        }
        flip_document(&library_root, course_id, &staging, id, unit)?;
    }
    if let Some(unit) = documents.get(course_id) {
        flip_document(&library_root, course_id, &staging, course_id, unit)?;
    }
    let project = course_project_root(&library_root, course_id)?;
    store.refresh()?;
    for (id, unit) in &documents {
        let document = store.read(id)?;
        if document.path != material_path(course_id, id)?
            || document.content.as_bytes() != unit.bytes.as_slice()
        {
            return Err(migration_conflict());
        }
    }
    let listed: BTreeSet<String> = crate::assets::list(store, course_id)?
        .into_iter()
        .map(|asset| asset.id)
        .collect();
    if listed != assets.keys().cloned().collect::<BTreeSet<_>>() {
        return Err(migration_conflict());
    }
    // Empty staging scaffolding is removed; leftovers of an interrupted run
    // stay for manual inspection instead of being deleted.
    let _ = fs::remove_dir(staging.join("documents"));
    let _ = fs::remove_dir(staging.join("assets").join(course_id));
    let _ = fs::remove_dir(staging.join("assets"));
    let _ = fs::remove_dir(&staging);
    Ok(project)
}

struct DocMigration {
    bytes: Vec<u8>,
    /// Flat relative path still holding the visible copy, if any.
    old: Option<String>,
    staged: bool,
}

struct AssetMigration {
    receipt: Vec<u8>,
    body: Vec<u8>,
    /// Final editable file name inside the project asset directory.
    body_name: String,
    /// Legacy directory still holding the visible copy, if any.
    old: Option<PathBuf>,
    old_body_name: Option<String>,
    staged: bool,
}

fn visible_kind(kind: &str) -> bool {
    matches!(
        kind,
        "course" | "folder" | "section" | "knowledge" | "exploration" | "exercise"
    )
}

// Loose parentId attribution, mirroring the tree view: chains that cannot
// reach this course (missing or cyclic parents) leave their documents
// untouched rather than failing the whole migration.
fn loose_member(store: &ContentStore, id: &str, course_id: &str) -> Result<Option<Document>> {
    let document = match store.read(id) {
        Ok(document) => document,
        Err(_) => return Ok(None),
    };
    let mut visited = BTreeSet::new();
    let mut cursor = document.id.clone();
    loop {
        if !visited.insert(cursor.clone()) || visited.len() > crate::library_tree::MAX_DEPTH {
            return Ok(None);
        }
        let doc = match store.read(&cursor) {
            Ok(doc) => doc,
            Err(_) => return Ok(None),
        };
        match doc
            .metadata
            .extra
            .get("parentId")
            .and_then(|value| value.as_str())
            .filter(|parent| crate::types::valid_id(parent))
        {
            Some(parent) => cursor = parent.to_owned(),
            None => return Ok((doc.id == course_id).then_some(document)),
        }
    }
}

fn staged_root_bytes(staging: &Path, course_id: &str) -> Result<Option<Vec<u8>>> {
    let path = staging.join("documents").join(format!("{course_id}.md"));
    let Some(bytes) = regular_file_bytes(&path, crate::content::MAX_DOCUMENT_BYTES as usize)?
    else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| unsafe_path())?;
    let (metadata, _) = crate::content::parse(text)?;
    if metadata.id != course_id
        || metadata.kind != "course"
        || metadata
            .extra
            .get("parentId")
            .is_some_and(|value| !value.is_null())
    {
        return Err(unsafe_path());
    }
    Ok(Some(bytes))
}

fn merge_staged_documents(
    staging: &Path,
    documents: &mut BTreeMap<String, DocMigration>,
) -> Result<()> {
    let directory = staging.join("documents");
    let Some(entries) = sub_entries(&directory)? else {
        return Ok(());
    };
    for (name, is_dir) in entries {
        if is_dir || !crate::content::is_markdown_path(Path::new(&name)) {
            return Err(unsafe_path());
        }
        let bytes =
            regular_file_bytes(&directory.join(&name), crate::content::MAX_DOCUMENT_BYTES as usize)?
                .ok_or_else(unsafe_path)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| unsafe_path())?;
        let (metadata, _) = crate::content::parse(text)?;
        if !visible_kind(&metadata.kind) {
            return Err(unsafe_path());
        }
        match documents.get_mut(&metadata.id) {
            Some(unit) => {
                if unit.bytes != bytes {
                    return Err(migration_conflict());
                }
                unit.staged = true;
            }
            None => {
                documents.insert(
                    metadata.id.clone(),
                    DocMigration {
                        bytes,
                        old: None,
                        staged: true,
                    },
                );
            }
        }
    }
    Ok(())
}

fn merge_staged_assets(
    staging: &Path,
    course_id: &str,
    assets: &mut BTreeMap<String, AssetMigration>,
) -> Result<()> {
    let parent = staging.join("assets").join(course_id);
    let Some(entries) = sub_entries(&parent)? else {
        return Ok(());
    };
    for (id, is_dir) in entries {
        if !is_dir {
            return Err(unsafe_path());
        }
        let dir = parent.join(&id);
        let Some(files) = asset_directory(&dir)? else {
            return Err(unsafe_path());
        };
        let Some(receipt) = files.get(".asset.json").cloned() else {
            // An interrupted write (body first, receipt last) left scratch only.
            fs::remove_dir_all(&dir).map_err(CoreError::io)?;
            continue;
        };
        if files.len() != 2 {
            return Err(unsafe_path());
        }
        let body_name = files
            .keys()
            .find(|name| name.as_str() != ".asset.json")
            .cloned()
            .ok_or_else(unsafe_path)?;
        let body = files[&body_name].clone();
        match assets.get_mut(&id) {
            Some(unit) => {
                if unit.receipt != receipt || unit.body != body || unit.body_name != body_name {
                    return Err(migration_conflict());
                }
                unit.staged = true;
            }
            None => {
                assets.insert(
                    id,
                    AssetMigration {
                        receipt,
                        body,
                        body_name,
                        old: None,
                        old_body_name: None,
                        staged: true,
                    },
                );
            }
        }
    }
    Ok(())
}

fn flip_document(
    library_root: &Path,
    course_id: &str,
    staging: &Path,
    id: &str,
    unit: &DocMigration,
) -> Result<()> {
    let final_path =
        ensure_directory(library_root, &format!("courses/{course_id}/materials/documents"))?
            .join(format!("{id}.md"));
    if let Some(old) = &unit.old {
        let old_path = library_root.join(old);
        if fs::symlink_metadata(&old_path).is_ok() {
            // Refuse to proceed if the visible copy changed since collection.
            if regular_file_bytes(&old_path, crate::content::MAX_DOCUMENT_BYTES as usize)?
                != Some(unit.bytes.clone())
            {
                return Err(migration_conflict());
            }
            let backup_target = ensure_directory(
                library_root,
                &format!("courses/{course_id}/{MIGRATION_BACKUP}/documents"),
            )?
            .join(format!("{id}.md"));
            if fs::symlink_metadata(&backup_target).is_ok() {
                return Err(migration_conflict());
            }
            fs::rename(&old_path, &backup_target).map_err(CoreError::io)?;
        }
    }
    let staged = staging.join("documents").join(format!("{id}.md"));
    if fs::symlink_metadata(&final_path).is_ok() {
        // The verified copy already occupies the destination; drop our scratch.
        let _ = fs::remove_file(&staged);
    } else {
        fs::rename(&staged, &final_path).map_err(CoreError::io)?;
    }
    Ok(())
}

fn asset_final_dir(library_root: &Path, course_id: &str, asset_id: &str) -> PathBuf {
    library_root
        .join(format!("courses/{course_id}/materials/assets/{course_id}"))
        .join(asset_id)
}

/// Creates each missing component and refuses symlinks or non-directories;
/// `library_root` must already be canonical.
fn ensure_directory(library_root: &Path, relative: &str) -> Result<PathBuf> {
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(unsafe_path());
    }
    let mut path = library_root.to_path_buf();
    for component in relative.components() {
        path.push(component.as_os_str());
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(unsafe_path());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Err(error) = fs::create_dir(&path) {
                    if error.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(CoreError::io(error));
                    }
                }
            }
            Err(error) => return Err(CoreError::io(error)),
        }
    }
    if !path.canonicalize().map_err(CoreError::io)?.starts_with(library_root) {
        return Err(unsafe_path());
    }
    Ok(path)
}

fn durable_write(target: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
    {
        Ok(file) => file,
        Err(error) => return Err(CoreError::io(error)),
    };
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(target);
        return Err(CoreError::io(error));
    }
    drop(file);
    #[cfg(unix)]
    fs::File::open(target.parent().ok_or_else(unsafe_path)?)
        .and_then(|directory| directory.sync_all())
        .map_err(CoreError::io)?;
    Ok(())
}

/// Regular-file bytes, or None when absent; symlinks and directories are unsafe.
fn regular_file_bytes(path: &Path, maximum: usize) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(unsafe_path());
            }
            Ok(Some(crate::assets::bounded_read(path, maximum)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => return Err(CoreError::io(error)),
    }
}

/// Directory listing as name → is_dir, rejecting symlinks; None when absent.
fn sub_entries(directory: &Path) -> Result<Option<BTreeMap<String, bool>>> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(unsafe_path());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(CoreError::io(error)),
    }
    let mut entries = BTreeMap::new();
    for entry in fs::read_dir(directory).map_err(CoreError::io)? {
        let entry = entry.map_err(CoreError::io)?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(CoreError::io)?;
        if metadata.file_type().is_symlink() {
            return Err(unsafe_path());
        }
        entries.insert(
            entry
                .file_name()
                .into_string()
                .map_err(|_| unsafe_path())?,
            metadata.is_dir(),
        );
    }
    Ok(Some(entries))
}

/// The single non-receipt regular file of a legacy or staged asset directory.
fn sole_body_file(legacy: &Path, receipt_name: &str) -> Result<(String, Vec<u8>)> {
    let entries = sub_entries(legacy)?.ok_or_else(unsafe_path)?;
    let mut body_name = None;
    let mut count = 0;
    for (name, is_dir) in entries {
        if is_dir {
            return Err(unsafe_path());
        }
        count += 1;
        if name != receipt_name && body_name.replace(name).is_some() {
            return Err(unsafe_path());
        }
    }
    if count != 2 {
        return Err(unsafe_path());
    }
    let body_name = body_name.ok_or_else(unsafe_path)?;
    let body = regular_file_bytes(&legacy.join(&body_name), MAX_MIGRATION_ASSET_BODY)?
        .ok_or_else(unsafe_path)?;
    Ok((body_name, body))
}

fn asset_directory(directory: &Path) -> Result<Option<BTreeMap<String, Vec<u8>>>> {
    let Some(entries) = sub_entries(directory)? else {
        return Ok(None);
    };
    if entries.values().any(|is_dir| *is_dir) {
        return Err(unsafe_path());
    }
    let mut files = BTreeMap::new();
    for name in entries.keys() {
        let maximum = if name == ".asset.json" {
            MAX_MIGRATION_RECEIPT
        } else {
            MAX_MIGRATION_ASSET_BODY
        };
        files.insert(
            name.clone(),
            regular_file_bytes(&directory.join(name), maximum)?.ok_or_else(unsafe_path)?,
        );
    }
    Ok(Some(files))
}

// Only new projects acquire the new layout. Low-level legacy imports keep their
// original paths, including deliberately malformed metadata awaiting repair.
pub(crate) fn routed_path(store: &ContentStore, metadata: &Metadata) -> Result<Option<String>> {
    let Some(parent) = metadata.extra.get("parentId").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let parent = match store.read(parent) {
        Ok(document) => document,
        Err(error) if error.code == "NOT_FOUND" => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(course) = project_course(store, &parent.path, &parent.id)? else {
        return Ok(None);
    };
    if crate::library_tree::course_id(store, &parent.id)? != course {
        return Err(unsafe_path());
    }
    course_project_root(store.root(), &course)?;
    Ok(Some(material_path(&course, &metadata.id)?))
}

// Recognize the full physical layout and its actual root document, not a
// coincidental component called "materials" in an existing legacy path.
pub(crate) fn project_course(
    store: &ContentStore,
    relative: &str,
    id: &str,
) -> Result<Option<String>> {
    let Some((course, _)) = relative
        .strip_prefix("courses/")
        .and_then(|p| p.split_once('/'))
    else {
        return Ok(None);
    };
    if !crate::types::valid_id(course) || relative != material_path(course, id)? {
        return Ok(None);
    }
    if course != id {
        match store.read(course) {
            Ok(root) if root.path == material_path(course, course)? => {}
            Ok(_) => return Ok(None),
            Err(error) if error.code == "NOT_FOUND" => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    Ok(Some(course.into()))
}

// Shared by create/save, so Markdown metadata edits and tree actions cannot
// claim a new physical course while keeping the only file in the old location.
pub(crate) fn validate_placement(
    store: &ContentStore,
    relative: &str,
    metadata: &Metadata,
) -> Result<()> {
    let course = project_course(store, relative, &metadata.id)?;
    let parent = metadata
        .extra
        .get("parentId")
        .filter(|value| !value.is_null());
    if course.as_deref() == Some(metadata.id.as_str()) {
        return if metadata.kind == "course" && parent.is_none() {
            Ok(())
        } else {
            Err(unsafe_path())
        };
    }
    let target = match parent.and_then(|value| value.as_str()) {
        Some(id) => match store.read(id) {
            Ok(document) => Some(document),
            Err(error) if error.code == "NOT_FOUND" && course.is_none() => None,
            Err(error) => return Err(error),
        },
        None => None,
    };
    let target_course = target
        .as_ref()
        .map(|doc| project_course(store, &doc.path, &doc.id))
        .transpose()?
        .flatten();
    if course.is_some() || target_course.is_some() {
        if course != target_course || metadata.kind == "course" {
            return Err(unsafe_path());
        }
        if let Some(parent) = target {
            if Some(crate::library_tree::course_id(store, &parent.id)?) != course {
                return Err(unsafe_path());
            }
        }
    }
    Ok(())
}

pub(crate) fn asset_storage(store: &ContentStore, course: &str) -> Result<(PathBuf, bool)> {
    let document = store.read(course)?;
    if document.path == material_path(course, course)? {
        Ok((
            course_project_root(store.root(), course)?.join("materials"),
            true,
        ))
    } else {
        Ok((store.root().to_path_buf(), false))
    }
}

pub(crate) fn indexable(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let parts: Vec<_> = relative.iter().filter_map(|p| p.to_str()).collect();
    if parts.iter().any(|p| p.starts_with('.')) {
        return false;
    }
    // courses/<id>/ is reserved for project layout. Only managed documents
    // enter the document index; raw assets and Pi state stay inert.
    if parts.len() >= 3 && parts[0] == "courses" && has_project_anchor(root, parts[1]) {
        if parts[2] != "materials" {
            return false;
        }
        if parts.len() >= 4 && parts[3] != "documents" {
            return false;
        }
    }
    true
}
fn has_project_anchor(root: &Path, course: &str) -> bool {
    let Ok(root) = course_project_root(root, course) else {
        return false;
    };
    let directory = root.join("materials/documents");
    let document = directory.join(format!("{course}.md"));
    if !matches!(fs::symlink_metadata(directory), Ok(m) if !m.file_type().is_symlink() && m.is_dir())
    {
        return false;
    }
    let Ok(bytes) =
        crate::assets::bounded_read(&document, crate::content::MAX_DOCUMENT_BYTES as usize)
    else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return false;
    };
    let Ok((metadata, _)) = crate::content::parse(text) else {
        return false;
    };
    metadata.id == course
        && metadata.kind == "course"
        && !metadata
            .extra
            .get("parentId")
            .is_some_and(|value| !value.is_null())
}
fn required() -> CoreError {
    CoreError::new(
        "COURSE_PROJECT_REQUIRED",
        "此课程尚无独立项目目录；未迁移旧材料。",
    )
}
fn migration_conflict() -> CoreError {
    CoreError::new(
        "CONFLICT",
        "迁移课程项目时发现文件已变化或目标位置被占用，原文件未改动；请核对后重试。",
    )
}
fn unsafe_path() -> CoreError {
    CoreError::new("UNSAFE_PATH", "课程目录或归属无效。")
}

#[cfg(test)]
#[path = "course_project_tests.rs"]
mod tests;
