//! Inert course attachments. Original names are metadata only; paths use validated
//! IDs and allowlisted extensions. No asset is executed, decoded, or fetched.

use crate::content::ContentStore;
use crate::types::{CoreError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, Metadata};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

const MAX_TEXT: usize = 1024 * 1024;
const MAX_IMAGE: usize = 5 * 1024 * 1024;
const MAX_PREVIEW: usize = 128 * 1024;
const MAX_COURSE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_ASSETS: usize = 100;
const MAX_METADATA: usize = 16 * 1024;
const MAX_COURSES: usize = 4096;
const LOCK_NAME: &str = ".assets.lock";
static OPERATIONS: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    pub course_id: String,
    pub file_name: String,
    pub mime: String,
    pub size: u64,
    pub sha256: String,
    pub reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetPreview {
    pub asset: Asset,
    pub text: Option<String>,
    pub data_url: Option<String>,
    pub truncated: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredAsset {
    version: u8,
    request_id: String,
    payload_hash: String,
    asset: Asset,
}

struct LockedRoot {
    path: PathBuf,
    // Advisory OS locking is released on process exit, including a crash.
    _lock: File,
}

/// Imports bytes provided by the caller, never a caller-selected system path.
/// Both body and receipt are synced in a private staging directory before its
/// atomic publication under the asset lock. Existing assets are never rewritten.
pub fn import(
    store: &ContentStore,
    course_id: &str,
    file_name: &str,
    bytes: &[u8],
    request_id: &str,
) -> Result<Asset> {
    validate_course(store, course_id)?;
    if !crate::types::valid_id(request_id) {
        return Err(CoreError::new("INVALID_INPUT", "资料导入请求编号无效。"));
    }
    let (extension, mime) = inspect(file_name, bytes)?;
    let id = format!("asset_{}", &digest(request_id.as_bytes())[..32]);
    let asset = Asset {
        id: id.clone(),
        course_id: course_id.into(),
        file_name: file_name.into(),
        mime: mime.into(),
        size: bytes.len() as u64,
        sha256: digest(bytes),
        reference: format!("asset:{id}"),
    };
    let record = StoredAsset {
        version: 1,
        request_id: request_id.into(),
        payload_hash: payload_hash(&asset)?,
        asset,
    };
    let _process = OPERATIONS.lock().map_err(|_| busy())?;
    let (storage, project) = crate::course_project::asset_storage(store, course_id)?;
    let root = lock_root(&storage, true)?.ok_or_else(invalid_state)?;
    // The ID is global even though storage and authorization are course-scoped.
    // Checking only directory existence discloses no other course's contents.
    reject_other_course_request(&root.path, course_id, &id)?;
    for summary in store
        .summaries()
        .into_iter()
        .filter(|s| s.kind == "course" && s.id != course_id)
    {
        let (other, _) = crate::course_project::asset_storage(store, &summary.id)?;
        if fs::symlink_metadata(other.join("assets").join(&summary.id).join(&id)).is_ok() {
            return Err(conflict());
        }
    }
    let course =
        checked_directory(&storage, &["assets", course_id], true)?.ok_or_else(invalid_state)?;
    let path = course.join(&id);
    if fs::symlink_metadata(&path).is_ok() {
        return existing_receipt(&storage, course_id, &id, &record, project);
    }
    let existing = list_course(&storage, course_id, project)?;
    let total: u64 = existing.iter().map(|asset| asset.size).sum();
    if existing.len() >= MAX_ASSETS || total + bytes.len() as u64 > MAX_COURSE_BYTES {
        return Err(limit("每门课程最多保存100份资料，总大小不得超过25MiB。"));
    }
    let name = if project {
        editable_name(file_name)
    } else {
        format!("content.{extension}")
    };
    publish_asset(&course, &id, |staged| {
        write_noclobber(staged, &name, bytes)?;
        write_noclobber(
            staged,
            if project {
                ".asset.json"
            } else {
                "metadata.json"
            },
            &serde_json::to_vec(&record).map_err(|_| invalid_state())?,
        )
    })?;
    Ok(record.asset)
}

pub fn list(store: &ContentStore, course_id: &str) -> Result<Vec<Asset>> {
    validate_course(store, course_id)?;
    let _process = OPERATIONS.lock().map_err(|_| busy())?;
    let (storage, project) = crate::course_project::asset_storage(store, course_id)?;
    let Some(_root) = lock_root(&storage, false)? else {
        return Ok(vec![]);
    };
    list_course(&storage, course_id, project)
}

/// Full, validated inert bytes for a checkpoint; never the truncated preview.
pub(crate) fn checkpoint_bytes(
    store: &ContentStore,
    course_id: &str,
) -> Result<Vec<(Asset, Vec<u8>)>> {
    validate_course(store, course_id)?;
    let _process = OPERATIONS.lock().map_err(|_| busy())?;
    let (storage, project) = crate::course_project::asset_storage(store, course_id)?;
    let Some(_root) = lock_root(&storage, false)? else {
        return Ok(vec![]);
    };
    list_course(&storage, course_id, project)?
        .iter()
        .map(|asset| {
            load_asset(&storage, course_id, &asset.id, project)
                .map(|(record, bytes)| (record.asset, bytes))
        })
        .collect()
}

pub fn read(store: &ContentStore, course_id: &str, asset_id: &str) -> Result<AssetPreview> {
    validate_course(store, course_id)?;
    validate_asset_id(asset_id)?;
    let _process = OPERATIONS.lock().map_err(|_| busy())?;
    let (storage, project) = crate::course_project::asset_storage(store, course_id)?;
    let _root = lock_root(&storage, false)?.ok_or_else(not_found)?;
    let (record, bytes) = load_asset(&storage, course_id, asset_id, project)?;
    let is_image = record.asset.mime.starts_with("image/");
    let (text, data_url, truncated) = if is_image {
        (
            None,
            Some(format!(
                "data:{};base64,{}",
                record.asset.mime,
                base64(&bytes)
            )),
            false,
        )
    } else {
        let text = std::str::from_utf8(&bytes).map_err(|_| invalid_state())?;
        let mut end = text.len().min(MAX_PREVIEW);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        (Some(text[..end].to_owned()), None, end < text.len())
    };
    Ok(AssetPreview {
        asset: record.asset,
        text,
        data_url,
        truncated,
    })
}

/// Context receives the same bounded inert text preview, never raw binary data.
pub fn referenced_text(
    store: &ContentStore,
    course_id: &str,
    asset_id: &str,
) -> Result<(Asset, Option<String>)> {
    let preview = read(store, course_id, asset_id)?;
    Ok((preview.asset, preview.text))
}

fn validate_course(store: &ContentStore, course_id: &str) -> Result<()> {
    if !crate::types::valid_id(course_id)
        || crate::library_tree::course_id(store, course_id)? != course_id
    {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "资料只能归属于有效的顶层课程。",
        ));
    }
    Ok(())
}

fn validate_asset_id(id: &str) -> Result<()> {
    if id.len() != 38 || !id.starts_with("asset_") || !lower_hex(&id[6..], 32) {
        return Err(unsafe_path());
    }
    Ok(())
}

fn extension(file_name: &str) -> Result<String> {
    if file_name.is_empty()
        || file_name.len() > 255
        || file_name.trim().is_empty()
        || file_name.contains(['/', '\\'])
        || file_name.contains("..")
        || file_name.chars().any(char::is_control)
        || !matches!(
            Path::new(file_name)
                .components()
                .collect::<Vec<_>>()
                .as_slice(),
            [Component::Normal(_)]
        )
    {
        return Err(unsafe_path());
    }
    Path::new(file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| {
            CoreError::new(
                "UNSUPPORTED_TYPE",
                "资料必须使用允许的文本、代码或图片扩展名。",
            )
        })
}

fn mime_for(extension: &str) -> Option<(&'static str, bool)> {
    Some(match extension {
        "png" => ("image/png", true),
        "jpg" | "jpeg" => ("image/jpeg", true),
        "gif" => ("image/gif", true),
        "webp" => ("image/webp", true),
        "json" => ("application/json", false),
        "csv" => ("text/csv", false),
        "md" | "markdown" => ("text/markdown", false),
        "txt" | "tsv" | "py" | "js" | "ts" | "tsx" | "jsx" | "css" | "rs" | "c" | "cpp" | "cc"
        | "cxx" | "h" | "hpp" | "java" | "sql" | "yaml" | "yml" | "toml" | "go" | "swift"
        | "kt" | "r" => ("text/plain", false),
        _ => return None,
    })
}

fn inspect(file_name: &str, bytes: &[u8]) -> Result<(String, &'static str)> {
    let extension = extension(file_name)?;
    let (mime, image) = mime_for(&extension).ok_or_else(|| {
        CoreError::new(
            "UNSUPPORTED_TYPE",
            "只允许受控文本、代码以及PNG、JPEG、GIF、WebP图片。",
        )
    })?;
    if bytes.len() > if image { MAX_IMAGE } else { MAX_TEXT } {
        return Err(limit(if image {
            "单张图片不得超过5MiB。"
        } else {
            "单份文本或代码不得超过1MiB。"
        }));
    }
    if image {
        let matches = match mime {
            "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
            "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
            _ => false,
        };
        if !matches {
            return Err(CoreError::new("INVALID_ASSET", "图片内容与扩展名不匹配。"));
        }
    } else {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| CoreError::new("INVALID_ASSET", "文本和代码资料必须是UTF-8。"))?;
        let prefix = text
            .trim_start_matches('\u{feff}')
            .trim_start()
            .chars()
            .take(512)
            .collect::<String>()
            .to_ascii_lowercase();
        if text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            || prefix.starts_with("<!doctype html")
            || prefix.starts_with("<html")
            || prefix.starts_with("<svg")
            || (prefix.starts_with("<?xml") && prefix.contains("<svg"))
            || bytes.starts_with(b"MZ")
            || bytes.starts_with(b"PK\x03\x04")
            || bytes.starts_with(b"%PDF-")
            || bytes.starts_with(b"Rar!")
            || bytes.starts_with(b"7z\xbc\xaf\x27\x1c")
            || bytes.starts_with(b"\x1f\x8b")
            || bytes.starts_with(b"BZh")
            || (bytes.len() >= 262 && &bytes[257..262] == b"ustar")
        {
            return Err(CoreError::new(
                "UNSUPPORTED_TYPE",
                "资料含二进制、主动页面或归档内容，未保存。",
            ));
        }
    }
    Ok((extension, mime))
}

pub(crate) fn editable_name(name: &str) -> String {
    let encoded = name.replace('%', "%25").replace(':', "%3A");
    if encoded == ".asset.json" || encoded.len() > 255 {
        // The caller has already validated the allowlisted extension. Keep the
        // original name in the receipt, and retain hidden-name exclusion.
        let extension = name
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        format!(
            "{}content.{extension}",
            if name.starts_with('.') { "." } else { "" }
        )
    } else {
        encoded
    }
}

fn payload_hash(asset: &Asset) -> Result<String> {
    let bytes = serde_json::to_vec(&(
        &asset.course_id,
        &asset.file_name,
        &asset.mime,
        asset.size,
        &asset.sha256,
    ))
    .map_err(|_| invalid_state())?;
    Ok(digest(&bytes))
}

fn existing_receipt(
    root: &Path,
    course_id: &str,
    id: &str,
    requested: &StoredAsset,
    project: bool,
) -> Result<Asset> {
    let (existing, _) = load_asset(root, course_id, id, project)?;
    if existing.request_id != requested.request_id
        || existing.payload_hash != requested.payload_hash
    {
        return Err(conflict());
    }
    sync_directory(&root.join("assets").join(course_id))?;
    Ok(requested.asset.clone())
}

fn list_course(root: &Path, course_id: &str, project: bool) -> Result<Vec<Asset>> {
    let Some(course) = checked_directory(root, &["assets", course_id], false)? else {
        return Ok(vec![]);
    };
    let mut assets = vec![];
    let mut total = 0_u64;
    for entry in fs::read_dir(&course).map_err(CoreError::io)? {
        let entry = entry.map_err(CoreError::io)?;
        let id = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid_state())?;
        if id.starts_with(".import-") {
            let metadata = fs::symlink_metadata(entry.path()).map_err(CoreError::io)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(unsafe_path());
            }
            continue; // A crash may leave unpublished staging; never expose or delete it.
        }
        validate_asset_id(&id)?;
        if assets.len() >= MAX_ASSETS {
            return Err(limit("课程资料超过100份，未读取或写入更多内容。"));
        }
        let (record, _) = load_asset(root, course_id, &id, project)?;
        total += record.asset.size;
        if total > MAX_COURSE_BYTES {
            return Err(limit("课程资料总大小超过25MiB。"));
        }
        assets.push(record.asset);
    }
    assets.sort_by(|a, b| a.file_name.cmp(&b.file_name).then(a.id.cmp(&b.id)));
    Ok(assets)
}

fn load_asset(
    root: &Path,
    course_id: &str,
    id: &str,
    project: bool,
) -> Result<(StoredAsset, Vec<u8>)> {
    validate_asset_id(id)?;
    let directory =
        checked_directory(root, &["assets", course_id, id], false)?.ok_or_else(not_found)?;
    let receipt = if project {
        ".asset.json"
    } else {
        "metadata.json"
    };
    let metadata = bounded_read(&directory.join(receipt), MAX_METADATA)?;
    let mut record: StoredAsset = serde_json::from_slice(&metadata).map_err(|_| invalid_state())?;
    if record.version != 1
        || !crate::types::valid_id(&record.request_id)
        || record.asset.id != id
        || record.asset.course_id != course_id
        || format!("asset_{}", &digest(record.request_id.as_bytes())[..32]) != id
        || record.asset.reference != format!("asset:{id}")
        || !lower_hex(&record.asset.sha256, 64)
        || record.payload_hash != payload_hash(&record.asset)?
    {
        return Err(invalid_state());
    }
    let ext = extension(&record.asset.file_name)?;
    let (mime, image) = mime_for(&ext).ok_or_else(invalid_state)?;
    if record.asset.mime != mime
        || record.asset.size
            > if image {
                MAX_IMAGE as u64
            } else {
                MAX_TEXT as u64
            }
    {
        return Err(invalid_state());
    }
    let content_name = if project {
        editable_name(&record.asset.file_name)
    } else {
        format!("content.{ext}")
    };
    let mut entries = 0;
    for entry in fs::read_dir(&directory).map_err(CoreError::io)? {
        let name = entry.map_err(CoreError::io)?.file_name();
        if name != receipt && name != content_name.as_str() {
            return Err(invalid_state());
        }
        entries += 1;
        if entries > 2 {
            return Err(invalid_state());
        }
    }
    if entries != 2 {
        return Err(invalid_state());
    }
    let bytes = bounded_read(
        &directory.join(content_name),
        if image { MAX_IMAGE } else { MAX_TEXT },
    )?;
    if !project
        && (bytes.len() as u64 != record.asset.size || digest(&bytes) != record.asset.sha256)
    {
        return Err(invalid_state());
    }
    inspect(&record.asset.file_name, &bytes)?;
    if project {
        record.asset.size = bytes.len() as u64;
        record.asset.sha256 = digest(&bytes);
    }
    Ok((record, bytes))
}

fn reject_other_course_request(root: &Path, course_id: &str, id: &str) -> Result<()> {
    let mut count = 0;
    for entry in fs::read_dir(root).map_err(CoreError::io)? {
        let entry = entry.map_err(CoreError::io)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid_state())?;
        if name == LOCK_NAME {
            continue;
        }
        count += 1;
        if count > MAX_COURSES {
            return Err(limit("课程资料目录超过受控扫描上限。"));
        }
        if !crate::types::valid_id(&name) {
            return Err(unsafe_path());
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(CoreError::io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(unsafe_path());
        }
        if name != course_id && fs::symlink_metadata(entry.path().join(id)).is_ok() {
            return Err(conflict());
        }
    }
    Ok(())
}

fn lock_root(root: &Path, create: bool) -> Result<Option<LockedRoot>> {
    let Some(root) = checked_directory(root, &["assets"], create)? else {
        return Ok(None);
    };
    let path = root.join(LOCK_NAME);
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = match options.open(&path) {
        Ok(file) => {
            file.sync_all().map_err(CoreError::io)?;
            sync_directory(&root)?;
            file
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => checked_file(&path, 0)?,
        Err(error) => return Err(CoreError::io(error)),
    };
    lock.try_lock().map_err(|_| busy())?;
    Ok(Some(LockedRoot {
        path: root,
        _lock: lock,
    }))
}

fn checked_directory(root: &Path, parts: &[&str], create: bool) -> Result<Option<PathBuf>> {
    let root_metadata = fs::symlink_metadata(root).map_err(CoreError::io)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(unsafe_path());
    }
    let mut path = root.to_path_buf();
    for part in parts {
        if !matches!(
            Path::new(part).components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        ) || part.contains(['/', '\\'])
            || part.chars().any(char::is_control)
        {
            return Err(unsafe_path());
        }
        path.push(part);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                match create_private_directory(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(CoreError::io(error)),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(CoreError::io(error)),
            Ok(_) => {}
        }
        let metadata = fs::symlink_metadata(&path).map_err(CoreError::io)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || !fs::canonicalize(&path)
                .map_err(CoreError::io)?
                .starts_with(root)
        {
            return Err(unsafe_path());
        }
    }
    Ok(Some(path))
}

fn checked_file(path: &Path, maximum: usize) -> Result<File> {
    let before = fs::symlink_metadata(path).map_err(CoreError::io)?;
    if before.file_type().is_symlink() || !before.is_file() || before.len() > maximum as u64 {
        return Err(invalid_state());
    }
    let file = File::open(path).map_err(CoreError::io)?;
    let opened = file.metadata().map_err(CoreError::io)?;
    let after = fs::symlink_metadata(path).map_err(CoreError::io)?;
    if after.file_type().is_symlink()
        || !same_file(&before, &opened)
        || !same_file(&after, &opened)
        || !opened.is_file()
        || opened.len() > maximum as u64
    {
        return Err(invalid_state());
    }
    Ok(file)
}

fn same_file(a: &Metadata, b: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev() && a.ino() == b.ino() && b.nlink() == 1
    }
    #[cfg(not(unix))]
    {
        a.len() == b.len() && a.modified().ok() == b.modified().ok()
    }
}

fn bounded_read(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let file = checked_file(path, maximum)?;
    let mut bytes = Vec::new();
    file.take((maximum + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(CoreError::io)?;
    if bytes.len() > maximum {
        return Err(invalid_state());
    }
    Ok(bytes)
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

// The caller holds both application and on-disk asset locks. Publishing the
// same-filesystem directory keeps readers from observing only one of its files.
fn publish_asset(course: &Path, id: &str, write: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let staged = tempfile::Builder::new()
        .prefix(".import-")
        .tempdir_in(course)
        .map_err(CoreError::io)?;
    write(staged.path())?;
    let destination = course.join(id);
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Err(conflict()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(CoreError::io(error)),
    }
    fs::rename(staged.path(), &destination).map_err(CoreError::io)?;
    sync_directory(course)
}

fn write_noclobber(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let metadata = fs::symlink_metadata(directory).map_err(CoreError::io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(unsafe_path());
    }
    let mut temporary = tempfile::NamedTempFile::new_in(directory).map_err(CoreError::io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(CoreError::io)?;
    }
    temporary.write_all(bytes).map_err(CoreError::io)?;
    temporary.as_file().sync_all().map_err(CoreError::io)?;
    temporary
        .persist_noclobber(directory.join(name))
        .map_err(|error| {
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                conflict()
            } else {
                CoreError::io(error.error)
            }
        })?;
    sync_directory(directory)
}

fn sync_directory(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(CoreError::io)?;
    }
    Ok(())
}

fn base64(bytes: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(CHARS[(a >> 2) as usize] as char);
        output.push(CHARS[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            CHARS[(((b & 15) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            CHARS[(c & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn unsafe_path() -> CoreError {
    CoreError::new(
        "UNSAFE_PATH",
        "资料路径、编号或文件名不安全，未读取或写入。",
    )
}
fn invalid_state() -> CoreError {
    CoreError::new("INVALID_ASSET", "资料元数据或内容不完整，未覆盖原文件。")
}
fn conflict() -> CoreError {
    CoreError::new("CONFLICT", "此导入请求编号已用于另一份资料，未覆盖原文件。")
}
fn not_found() -> CoreError {
    CoreError::new("NOT_FOUND", "找不到此课程资料。")
}
fn limit(message: &str) -> CoreError {
    CoreError::new("LIMIT_EXCEEDED", message)
}
fn busy() -> CoreError {
    CoreError::new("BUSY", "资料存储正在使用，请稍后重试。")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, ContentStore, String, String, String) {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ContentStore::new(
            directory.path().join("library"),
            directory.path().join("state"),
        )
        .unwrap();
        let course =
            crate::library_tree::create(&mut store, "course", "课程", None, "", "fixture_course")
                .unwrap();
        let section = crate::library_tree::create(
            &mut store,
            "section",
            "小节",
            Some(&course.id),
            "资料只归属顶层课程。",
            "fixture_section",
        )
        .unwrap();
        let other = crate::library_tree::create(
            &mut store,
            "course",
            "其他课程",
            None,
            "",
            "fixture_other",
        )
        .unwrap();
        (directory, store, course.id, section.id, other.id)
    }

    #[test]
    fn failed_staging_and_crash_leftovers_do_not_publish_or_block_assets() {
        let (_dir, store, course, _, _) = fixture();
        let seed = import(&store, &course, "seed.txt", b"seed", "staging_seed").unwrap();
        let directory = store.root().join("assets").join(&course);
        let id = format!("asset_{}", &digest(b"staging_failed")[..32]);
        let error = publish_asset(&directory, &id, |staged| {
            write_noclobber(staged, "content.txt", b"uncommitted")?;
            Err(CoreError::new("IO", "synthetic receipt write failure"))
        })
        .unwrap_err();
        assert_eq!(error.code, "IO");
        assert!(!directory.join(&id).exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        let pending = directory.join(".import-crashed");
        fs::create_dir(&pending).unwrap();
        fs::write(pending.join("content.txt"), b"unpublished").unwrap();
        assert_eq!(list(&store, &course).unwrap(), vec![seed]);
        let next = import(&store, &course, "next.txt", b"next", "staging_next").unwrap();
        assert_eq!(list(&store, &course).unwrap().len(), 2);
        let error = publish_asset(&directory, &next.id, |_| Ok(())).unwrap_err();
        assert_eq!(error.code, "CONFLICT");
        assert_eq!(
            read(&store, &course, &next.id).unwrap().text.as_deref(),
            Some("next")
        );
        assert!(pending.is_dir()); // Unknown crash data is retained, not silently removed.
    }

    #[test]
    fn unicode_assets_reopen_and_return_real_stable_receipts() {
        let (directory, store, course, _, _) = fixture();
        let asset = import(
            &store,
            &course,
            "复习 资料.md",
            "# 笔记\n\n只作为文本保存。".as_bytes(),
            "import_unicode",
        )
        .unwrap();
        assert_eq!(
            asset.id,
            format!("asset_{}", &digest(b"import_unicode")[..32])
        );
        assert_eq!(asset.reference, format!("asset:{}", asset.id));
        assert_eq!(
            asset.sha256,
            digest("# 笔记\n\n只作为文本保存。".as_bytes())
        );
        assert_eq!(asset.mime, "text/markdown");
        assert_eq!(list(&store, &course).unwrap(), vec![asset.clone()]);
        let preview = read(&store, &course, &asset.id).unwrap();
        assert_eq!(preview.text.as_deref(), Some("# 笔记\n\n只作为文本保存。"));
        assert!(preview.data_url.is_none());
        assert!(!preview.truncated);
        drop(store);
        let mut reopened = ContentStore::new(
            directory.path().join("library"),
            directory.path().join("state"),
        )
        .unwrap();
        reopened.refresh().unwrap();
        assert_eq!(list(&reopened, &course).unwrap(), vec![asset.clone()]);
        assert_eq!(
            import(
                &reopened,
                &course,
                "复习 资料.md",
                "# 笔记\n\n只作为文本保存。".as_bytes(),
                "import_unicode"
            )
            .unwrap(),
            asset
        );
        let (reference, text) = referenced_text(&reopened, &course, &asset.id).unwrap();
        assert_eq!(reference, asset);
        assert_eq!(text, preview.text);
    }

    #[test]
    fn request_identity_includes_course_name_and_contents_without_overwriting() {
        let (_directory, store, course, _, other) = fixture();
        let first = import(
            &store,
            &course,
            "sample.py",
            b"print('hello')\n",
            "same_request",
        )
        .unwrap();
        for (target, name, bytes) in [
            (
                course.as_str(),
                "sample.py",
                b"print('different')\n".as_slice(),
            ),
            (
                course.as_str(),
                "renamed.py",
                b"print('hello')\n".as_slice(),
            ),
            (other.as_str(), "sample.py", b"print('hello')\n".as_slice()),
        ] {
            assert_eq!(
                import(&store, target, name, bytes, "same_request")
                    .unwrap_err()
                    .code,
                "CONFLICT"
            );
        }
        assert_eq!(list(&store, &course).unwrap(), vec![first.clone()]);
        assert_eq!(
            read(&store, &course, &first.id).unwrap().text.as_deref(),
            Some("print('hello')\n")
        );
        assert!(list(&store, &other).unwrap().is_empty());
        assert!(read(&store, &other, &first.id).is_err());
    }

    #[test]
    fn text_preview_respects_utf8_boundary_and_exact_file_limits() {
        let (_directory, store, course, _, _) = fixture();
        let text = "中".repeat(MAX_TEXT / 3);
        let asset = import(
            &store,
            &course,
            "边界.txt",
            text.as_bytes(),
            "utf8_boundary",
        )
        .unwrap();
        let preview = read(&store, &course, &asset.id).unwrap();
        let limited = preview.text.unwrap();
        assert!(preview.truncated);
        assert!(limited.len() <= MAX_PREVIEW);
        assert!(MAX_PREVIEW - limited.len() < 3);
        assert!(text.starts_with(&limited));
        import(
            &store,
            &course,
            "exact.txt",
            &vec![b'a'; MAX_TEXT],
            "exact_text",
        )
        .unwrap();
        assert_eq!(
            import(
                &store,
                &course,
                "too-large.txt",
                &vec![b'a'; MAX_TEXT + 1],
                "too_large_text"
            )
            .unwrap_err()
            .code,
            "LIMIT_EXCEEDED"
        );
        let empty = import(&store, &course, "empty.txt", b"", "empty_text").unwrap();
        assert_eq!(
            read(&store, &course, &empty.id).unwrap().text.as_deref(),
            Some("")
        );
    }

    #[test]
    fn images_require_matching_magic_and_never_supply_context_text() {
        let (_directory, store, course, _, _) = fixture();
        for (index, name, bytes, mime) in [
            (0, "图.PNG", b"\x89PNG\r\n\x1a\n".as_slice(), "image/png"),
            (1, "photo.jpeg", b"\xff\xd8\xff".as_slice(), "image/jpeg"),
            (2, "animation.gif", b"GIF89a".as_slice(), "image/gif"),
            (
                3,
                "photo.webp",
                b"RIFF\x04\x00\x00\x00WEBP".as_slice(),
                "image/webp",
            ),
        ] {
            let asset = import(&store, &course, name, bytes, &format!("image_{index}")).unwrap();
            assert_eq!(asset.mime, mime);
            let preview = read(&store, &course, &asset.id).unwrap();
            assert!(preview.text.is_none());
            assert_eq!(
                preview.data_url,
                Some(format!("data:{mime};base64,{}", base64(bytes)))
            );
            assert!(!preview.truncated);
            assert!(referenced_text(&store, &course, &asset.id)
                .unwrap()
                .1
                .is_none());
        }
        for (name, bytes) in [
            ("fake.png", b"GIF89a".as_slice()),
            ("fake.jpg", b"not an image".as_slice()),
            ("fake.gif", b"\x89PNG\r\n\x1a\n".as_slice()),
            ("fake.webp", b"RIFFonly".as_slice()),
        ] {
            assert_eq!(
                import(&store, &course, name, bytes, "invalid_magic")
                    .unwrap_err()
                    .code,
                "INVALID_ASSET"
            );
        }
    }

    #[test]
    fn course_count_and_total_size_limits_include_existing_assets() {
        let (_directory, store, course, _, other) = fixture();
        let mut first = None;
        for index in 0..MAX_ASSETS {
            let asset = import(
                &store,
                &course,
                &format!("note-{index}.txt"),
                b"x",
                &format!("count_{index}"),
            )
            .unwrap();
            if index == 0 {
                first = Some(asset);
            }
        }
        assert_eq!(list(&store, &course).unwrap().len(), MAX_ASSETS);
        assert_eq!(
            import(&store, &course, "overflow.txt", b"x", "count_overflow")
                .unwrap_err()
                .code,
            "LIMIT_EXCEEDED"
        );
        assert_eq!(
            import(&store, &course, "note-0.txt", b"x", "count_0").unwrap(),
            first.unwrap()
        );
        let mut image = vec![0_u8; MAX_IMAGE];
        image[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        for index in 0..5 {
            import(
                &store,
                &other,
                &format!("image-{index}.png"),
                &image,
                &format!("size_{index}"),
            )
            .unwrap();
        }
        assert_eq!(
            list(&store, &other)
                .unwrap()
                .iter()
                .map(|asset| asset.size)
                .sum::<u64>(),
            MAX_COURSE_BYTES
        );
        assert_eq!(
            import(&store, &other, "overflow.txt", b"x", "size_overflow")
                .unwrap_err()
                .code,
            "LIMIT_EXCEEDED"
        );
        image.push(0);
        assert_eq!(
            import(&store, &other, "oversized.png", &image, "image_oversize")
                .unwrap_err()
                .code,
            "LIMIT_EXCEEDED"
        );
    }

    #[test]
    fn invalid_paths_non_courses_and_unsupported_types_never_create_asset_storage() {
        let (_directory, store, course, section, _) = fixture();
        for target in [section.as_str(), "../outside", "missing_course"] {
            assert!(import(&store, target, "note.txt", b"text", "bad_course").is_err());
        }
        for name in [
            "../note.txt",
            "path/note.txt",
            "path\\note.txt",
            "x..txt",
            "line\n.txt",
            "/absolute.txt",
            "",
        ] {
            assert!(import(&store, &course, name, b"text", "bad_path").is_err());
        }
        for name in [
            "page.html",
            "icon.svg",
            "app.exe",
            "archive.zip",
            "document.pdf",
            "video.mp4",
        ] {
            assert_eq!(
                import(&store, &course, name, b"text", "bad_type")
                    .unwrap_err()
                    .code,
                "UNSUPPORTED_TYPE"
            );
        }
        for bytes in [
            b"\xff\xfe".as_slice(),
            b"a\x00b".as_slice(),
            b"<html>active</html>".as_slice(),
            b"<svg></svg>".as_slice(),
            b"MZnot-code".as_slice(),
        ] {
            assert!(import(&store, &course, "disguised.txt", bytes, "bad_contents").is_err());
        }
        assert!(!store.root().join("assets").exists());
        assert!(read(&store, &course, "../outside").is_err());
    }

    #[test]
    fn code_is_inert_and_saved_with_private_non_executable_permissions() {
        let (directory, store, course, _, _) = fixture();
        let marker = directory.path().join("must-not-exist");
        let code = format!(
            "from pathlib import Path\nPath({:?}).write_text('executed')\n",
            marker.to_string_lossy()
        );
        let asset = import(
            &store,
            &course,
            "sample.py",
            code.as_bytes(),
            "inert_python",
        )
        .unwrap();
        assert_eq!(read(&store, &course, &asset.id).unwrap().text, Some(code));
        assert!(!marker.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let folder = store.root().join("assets").join(&course).join(&asset.id);
            assert_eq!(
                fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
                0o700
            );
            for name in ["metadata.json", "content.py"] {
                assert_eq!(
                    fs::metadata(folder.join(name))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
    }

    #[test]
    fn corrupt_metadata_or_contents_are_never_overwritten_or_hidden_from_quota() {
        let (_directory, store, course, _, _) = fixture();
        let asset = import(
            &store,
            &course,
            "original.txt",
            b"original",
            "original_request",
        )
        .unwrap();
        let folder = store.root().join("assets").join(&course).join(&asset.id);
        let metadata = folder.join("metadata.json");
        let original_metadata = fs::read(&metadata).unwrap();
        fs::write(&metadata, b"broken metadata").unwrap();
        assert!(import(
            &store,
            &course,
            "original.txt",
            b"original",
            "original_request"
        )
        .is_err());
        assert_eq!(fs::read(&metadata).unwrap(), b"broken metadata");
        assert!(import(&store, &course, "new.txt", b"new", "new_request").is_err());
        fs::write(&metadata, original_metadata).unwrap();
        fs::write(folder.join("content.txt"), b"tampered").unwrap();
        assert!(read(&store, &course, &asset.id).is_err());
        assert!(list(&store, &course).is_err());
        assert!(import(&store, &course, "new.txt", b"new", "another_request").is_err());
        assert_eq!(fs::read(folder.join("content.txt")).unwrap(), b"tampered");
        fs::write(&metadata, vec![b'x'; MAX_METADATA + 1]).unwrap();
        assert!(read(&store, &course, &asset.id).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_components_and_files_cannot_read_or_write_outside_the_library() {
        use std::os::unix::fs::symlink;
        let (directory, store, course, _, _) = fixture();
        let outside = directory.path().join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, store.root().join("assets")).unwrap();
        assert!(import(&store, &course, "note.txt", b"text", "root_symlink").is_err());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        fs::remove_file(store.root().join("assets")).unwrap();
        let asset = import(&store, &course, "note.txt", b"original", "safe_asset").unwrap();
        let folder = store.root().join("assets").join(&course).join(&asset.id);
        let content = folder.join("content.txt");
        let target = outside.join("private.txt");
        fs::write(&target, b"outside-content").unwrap();
        fs::remove_file(&content).unwrap();
        symlink(&target, &content).unwrap();
        assert!(read(&store, &course, &asset.id).is_err());
        assert!(import(&store, &course, "note.txt", b"replacement", "safe_asset").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"outside-content");
    }

    #[test]
    fn base64_has_correct_padding_for_all_chunk_lengths() {
        for (input, encoded) in [
            (b"".as_slice(), ""),
            (b"f".as_slice(), "Zg=="),
            (b"fo".as_slice(), "Zm8="),
            (b"foo".as_slice(), "Zm9v"),
            (b"foob".as_slice(), "Zm9vYg=="),
        ] {
            assert_eq!(base64(input), encoded);
        }
    }
}
