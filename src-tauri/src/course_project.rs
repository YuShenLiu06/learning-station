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
fn unsafe_path() -> CoreError {
    CoreError::new("UNSAFE_PATH", "课程目录或归属无效。")
}

#[cfg(test)]
#[path = "course_project_tests.rs"]
mod tests;
