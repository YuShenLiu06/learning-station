//! Pure file addresses and raw disk stat. "Current" is resolved only here, at
//! freeze time; every downstream caller passes explicit addresses instead.
use crate::{app::Core, types::*};
use serde::{Deserialize, Serialize};
use std::{fs, path::Component, time::UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileAddr {
    /// Stable identity: documentId or assetId.
    pub id: String,
    /// Repository-relative path for course files; disk-relative path otherwise.
    pub path: String,
    /// markdown | text | image | pdf | exercise | unknown.
    pub kind: String,
    /// Owning course; empty for course-less files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub course: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileMetadata {
    pub path: String,
    pub kind: String,
    pub size_bytes: u64,
    pub mtime: u64,
}

fn conflict() -> CoreError {
    CoreError::new("CONTEXT_CONFLICT", "当前聚焦文件无法解析，原请求保留；请重新核对。")
}

fn unsafe_path() -> CoreError {
    CoreError::new("UNSAFE_PATH", "文件地址路径无效。")
}

/// The only place "which file is the user looking at" is answered. The address
/// is complete at return; neither interface re-reads application state later.
/// Unrecognized kinds still return an address (kind=unknown), never an error.
pub fn nowfile(
    core: &mut Core,
    session_id: &str,
    document_id: &str,
    asset_id: Option<&str>,
    view: &str,
) -> Result<FileAddr> {
    if !["read", "edit", "exercise", "asset"].contains(&view) || core.content.is_trashed(session_id) {
        return Err(conflict());
    }
    core.content.refresh()?;
    let session = crate::sessions::get(&mut core.content, session_id)?;
    crate::course_sessions::source(&core.content, &session, document_id)?;
    let document = core.content.read(document_id)?;
    let course = crate::course_sessions::course(&core.content, document_id)?;
    if let Some(asset_id) = asset_id {
        let course = course.as_ref().ok_or_else(conflict)?;
        let asset = crate::assets::list(&core.content, course)?
            .into_iter()
            .find(|a| a.id == asset_id)
            .ok_or_else(|| CoreError::new("NOT_FOUND", "当前课程没有这份附件。"))?;
        let kind = if asset.mime.starts_with("image/") {
            "image"
        } else if asset.mime == "application/pdf" {
            "pdf"
        } else if asset.mime.starts_with("text/") || asset.mime == "application/json" {
            "text"
        } else {
            "unknown"
        };
        return Ok(FileAddr {
            id: asset.id.clone(),
            path: format!("assets/{}/{}/{}", course, asset.id, crate::assets::editable_name(&asset.file_name)),
            kind: kind.into(),
            course: Some(course.clone()),
        });
    }
    let kind = match document.metadata.kind.as_str() {
        "exercise" => "exercise",
        "course" | "folder" | "section" | "knowledge" | "exploration" => "markdown",
        _ => "unknown",
    };
    Ok(FileAddr {
        id: document.id.clone(),
        path: match &course {
            Some(_) => format!("documents/{}.md", document.id),
            None => document.path,
        },
        kind: kind.into(),
        course,
    })
}

/// Direct std::fs stat of the addressed file. No cache, no state, no version.
pub fn metadata(core: &Core, addr: &FileAddr) -> Result<FileMetadata> {
    let host = disk_path(core, addr)?;
    let meta = fs::symlink_metadata(&host)?;
    let mtime = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(FileMetadata { path: addr.path.clone(), kind: addr.kind.clone(), size_bytes: meta.len(), mtime })
}

/// Resolves an address to its host file: the course project's materials tree
/// for course files, the library root for course-less disk paths. Shared with
/// the versioning content-hash backend so resolution exists in one place only.
pub(crate) fn disk_path(core: &Core, addr: &FileAddr) -> Result<std::path::PathBuf> {
    let host = match &addr.course {
        Some(course) => crate::course_project::course_project_root(core.content.root(), course)?
            .join("materials")
            .join(checked(&addr.path)?),
        None => core.content.root().join(checked(&addr.path)?),
    };
    let meta = fs::symlink_metadata(&host)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(CoreError::new("NOT_FOUND", "文件地址没有对应的普通文件。"));
    }
    Ok(host)
}

fn checked(relative: &str) -> Result<&str> {
    if relative.is_empty()
        || relative.contains('\\')
        || relative.chars().any(char::is_control)
        || std::path::Path::new(relative)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(unsafe_path());
    }
    Ok(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Engine;

    #[test]
    fn metadata_stats_the_addressed_disk_file_directly() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut c = e.lock().unwrap();
        let course = crate::library_tree::create_project(&mut c.content, "C", "", "metadata-course").unwrap();
        let note = crate::library_tree::create(&mut c.content, "section", "A", Some(&course.id), "元数据正文\n\n末段", "metadata-note").unwrap();
        let sid = c.start_session(&note.id, false).unwrap().0.id;
        let addr = nowfile(&mut c, &sid, &note.id, None, "read").unwrap();
        assert_eq!(addr.kind, "markdown");
        assert_eq!(addr.path, format!("documents/{}.md", note.id));
        assert_eq!(addr.course.as_deref(), Some(course.id.as_str()));
        let host = disk_path(&c, &addr).unwrap();
        let stat = fs::metadata(&host).unwrap();
        let meta = metadata(&c, &addr).unwrap();
        assert_eq!(meta.size_bytes, stat.len());
        assert_eq!(meta.mtime, stat.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_secs());
        assert_eq!(meta.kind, "markdown");
        // A course-less file keeps a disk-relative path under the library root.
        let loose = crate::library_tree::create(&mut c.content, "section", "Loose", None, "无课程正文", "metadata-loose").unwrap();
        let sid = c.start_session(&loose.id, false).unwrap().0.id;
        let addr = nowfile(&mut c, &sid, &loose.id, None, "read").unwrap();
        assert_eq!(addr.course, None);
        assert_eq!(addr.path, format!("courses/{}.md", loose.id));
        assert!(metadata(&c, &addr).unwrap().size_bytes > 0);
    }

    #[test]
    fn asset_addresses_carry_mime_kinds_and_unknown_kinds_still_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut c = e.lock().unwrap();
        let course = crate::library_tree::create_project(&mut c.content, "C", "", "metadata-asset-course").unwrap();
        let note = crate::library_tree::create(&mut c.content, "section", "A", Some(&course.id), "锚点正文", "metadata-asset-note").unwrap();
        let sid = c.start_session(&note.id, false).unwrap().0.id;
        let asset = crate::material_git::retry_busy(|| {
            crate::assets::import(&c.content, &course.id, "note.txt", b"attached text", "metadata-asset")
        })
        .unwrap();
        let addr = nowfile(&mut c, &sid, &note.id, Some(&asset.id), "asset").unwrap();
        assert_eq!(addr.kind, "text");
        assert_eq!(addr.path, format!("assets/{}/{}/{}", course.id, asset.id, crate::assets::editable_name(&asset.file_name)));
        assert_eq!(addr.course.as_deref(), Some(course.id.as_str()));
        assert_eq!(metadata(&c, &addr).unwrap().size_bytes, "attached text".len() as u64);
        let mut foreign = addr.clone();
        foreign.kind = "not-a-kind".into();
        assert_eq!(metadata(&c, &foreign).unwrap().kind, "not-a-kind");
        let mut escaped = addr;
        escaped.path = "../escape.md".into();
        assert_eq!(metadata(&c, &escaped).unwrap_err().code, "UNSAFE_PATH");
    }
}
