//! A course's physical workspace. Materials have one editable location; legacy
//! flat libraries are read in place and never implicitly migrated.
use crate::{
    content::ContentStore,
    types::{CoreError, Document, Metadata, Result},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

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

// Only new projects acquire the new layout. Low-level legacy imports keep their
// original paths, including deliberately malformed metadata awaiting repair.
pub(crate) fn routed_path(store: &ContentStore, metadata: &Metadata) -> Result<Option<String>> {
    let Some(parent) = metadata.extra.get("parentId").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let Some(summary) = store.summaries().into_iter().find(|s| s.id == parent) else {
        return Ok(None);
    };
    if summary.path.split('/').nth(2) != Some("materials") {
        return Ok(None);
    }
    let course = crate::library_tree::course_id(store, parent)?;
    course_project_root(store.root(), &course)?;
    Ok(Some(material_path(&course, &metadata.id)?))
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
    if parts.len() >= 3 && parts[0] == "courses" {
        if parts[2] != "materials" {
            return false;
        }
        if parts.len() >= 4 && parts[3] != "documents" {
            return false;
        }
    }
    true
}
fn required() -> CoreError {
    CoreError::new(
        "COURSE_PROJECT_REQUIRED",
        "此课程尚无独立项目目录；未迁移旧材料。",
    )
}
fn unsafe_path() -> CoreError {
    CoreError::new("UNSAFE_PATH", "课程目录或归属无效。")
}

#[cfg(test)]
#[path = "course_project_tests.rs"]
mod tests;
