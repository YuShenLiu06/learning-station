//! Read-only browser for a registered course's materials. Never follows links.
use crate::{
    content::ContentStore,
    types::{CoreError, Result},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseFile {
    pub id: String,
    pub path: String,
    pub name: String,
    pub directory: bool,
    pub parent_id: Option<String>,
    pub document_id: Option<String>,
}
fn denied() -> CoreError {
    CoreError::new("OUT_OF_SCOPE", "此文件不在课程材料的可访问范围内。")
}
fn visible(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    !n.starts_with('.')
        && !matches!(
            n.as_str(),
            "node_modules"
                | "target"
                | "cache"
                | "__pycache__"
                | "secrets"
                | "credentials"
                | "credentials.json"
                | "secrets.json"
                | "id_rsa"
                | "id_ed25519"
        )
        && ![".pem", ".key", ".p12", ".pfx"]
            .iter()
            .any(|s| n.ends_with(s))
}
fn root(store: &ContentStore, course: &str) -> Result<PathBuf> {
    if store.is_trashed(course) || store.read(course)?.metadata.kind != "course" {
        return Err(denied());
    }
    Ok(crate::course_project::course_project_root(store.root(), course)?.join("materials"))
}
fn id(course: &str, path: &str) -> String {
    format!("file-{:x}", Sha256::digest(format!("{course}/{path}")))
}
pub fn resolve(store: &ContentStore, course: &str, relative: &str) -> Result<PathBuf> {
    let root = root(store, course)?;
    if relative.is_empty() || relative.contains('\\') {
        return Err(denied());
    }
    let mut path = root.clone();
    for part in Path::new(relative).components() {
        let Component::Normal(name) = part else {
            return Err(denied());
        };
        if !name.to_str().is_some_and(visible) {
            return Err(denied());
        }
        path.push(name);
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(denied());
        }
    }
    if !path.canonicalize()?.starts_with(root)
        || store.is_path_trashed(&format!("courses/{course}/materials/{relative}"))?
    {
        return Err(denied());
    }
    Ok(path)
}
pub fn list(store: &ContentStore, course: &str) -> Result<Vec<CourseFile>> {
    let root = match root(store, course) {
        Ok(root) => root,
        Err(e) if e.code == "COURSE_PROJECT_REQUIRED" => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let documents = crate::library_tree::list(store)?.nodes;
    let mut result = vec![];
    fn walk(
        store: &ContentStore,
        course: &str,
        root: &Path,
        dir: &Path,
        depth: usize,
        docs: &[crate::library_tree::LibraryNode],
        out: &mut Vec<CourseFile>,
    ) -> Result<()> {
        if depth > 24 {
            return Err(CoreError::new(
                "FILE_TREE_LIMIT",
                "课程目录超过 24 层，请在 Finder 中整理后重试。",
            ));
        }
        let mut entries = fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !visible(&name) {
                continue;
            }
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() || !(meta.is_file() || meta.is_dir()) {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| denied())?
                .to_string_lossy()
                .replace('\\', "/");
            let full = format!("courses/{course}/materials/{relative}");
            if store.is_path_trashed(&full)? {
                continue;
            }
            let doc = docs.iter().find(|d| d.path == full);
            if doc.is_some_and(|d| d.kind == "course") {
                continue;
            }
            if out.len() >= 10000 {
                return Err(CoreError::new(
                    "FILE_TREE_LIMIT",
                    "课程文件超过 10000 项，请在 Finder 中整理后重试。",
                ));
            }
            let parent = Path::new(&relative)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| id(course, &p.to_string_lossy()));
            out.push(CourseFile {
                id: id(course, &relative),
                path: relative,
                name,
                directory: meta.is_dir(),
                parent_id: parent,
                document_id: doc.map(|d| d.id.clone()),
            });
            if meta.is_dir() {
                walk(store, course, root, &entry.path(), depth + 1, docs, out)?;
            }
        }
        Ok(())
    }
    walk(store, course, &root, &root, 0, &documents, &mut result)?;
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lists_raw_files_and_rejects_escape_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let mut store =
            ContentStore::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let metadata=serde_json::from_value(serde_json::json!({"id":"course","title":"课程","kind":"course","goals":[],"tags":[],"links":[],"version":1})).unwrap();
        crate::course_project::create_material(&mut store, metadata, "").unwrap();
        let material = root(&store, "course").unwrap();
        fs::create_dir_all(material.join("demo")).unwrap();
        fs::write(material.join("demo/example.html"), "<html/>").unwrap();
        fs::write(material.join("notes.txt"), "test").unwrap();
        fs::write(material.join(".env"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path(), material.join("escape")).unwrap();
        let files = list(&store, "course").unwrap();
        assert!(files
            .iter()
            .any(|f| f.path == "demo/example.html" && f.parent_id.is_some()));
        assert!(files.iter().any(|f| f.path == "notes.txt"));
        assert!(!files.iter().any(|f| f.path == ".env" || f.path == "escape"));
        assert!(resolve(&store, "course", "../state").is_err());
        assert!(resolve(&store, "course", ".env").is_err());
        assert!(resolve(&store, "course", "/tmp").is_err());
        #[cfg(unix)]
        assert!(resolve(&store, "course", "escape").is_err());
        assert!(resolve(&store, "course", "notes.txt").is_ok());
    }
    #[test]
    fn filters_private_files_without_hiding_user_formats() {
        for name in [
            ".git",
            ".env",
            "secrets",
            "credentials.json",
            "id_rsa",
            "private.pem",
            "cache",
        ] {
            assert!(!visible(name));
        }
        for name in ["演示.html", "资料.txt", "课堂", "示例.zip"] {
            assert!(visible(name));
        }
    }
}
