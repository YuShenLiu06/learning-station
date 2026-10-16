//! A QA-copy-only resource prevents UI automation relaunches from opening real user state.
//! Normal release bundles never contain this marker and retain the normal entry path.
use crate::types::{CoreError, Result};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    schema_version: u32,
    library: PathBuf,
    state: PathBuf,
}
fn invalid() -> CoreError {
    CoreError::new(
        "INVALID_QA_ISOLATION",
        "隔离验收目录无效；已停止启动，未打开用户学习库。",
    )
}
pub fn locations(executable: &Path) -> Result<Option<(PathBuf, PathBuf)>> {
    let Some(macos) = executable
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "MacOS"))
    else {
        return Ok(None);
    };
    let Some(contents) = macos
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "Contents"))
    else {
        return Ok(None);
    };
    let path = contents.join("Resources/.qa-isolation.json");
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid()),
    };
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 4096 {
        return Err(invalid());
    }
    let marker: Marker =
        serde_json::from_slice(&fs::read(path).map_err(|_| invalid())?).map_err(|_| invalid())?;
    if marker.schema_version != 1
        || marker
            .library
            .file_name()
            .is_none_or(|name| name != "library")
        || marker.state.file_name().is_none_or(|name| name != "state")
        || marker.library.parent() != marker.state.parent()
    {
        return Err(invalid());
    }
    let parent = marker.library.parent().ok_or_else(invalid)?;
    if !parent.is_absolute()
        || parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_none_or(|name| !name.starts_with("learning-station-full-qa-"))
    {
        return Err(invalid());
    }
    let temp = fs::canonicalize("/tmp").map_err(|_| invalid())?;
    let canonical_parent = fs::canonicalize(parent).map_err(|_| invalid())?;
    if canonical_parent.parent() != Some(temp.as_path()) {
        return Err(invalid());
    }
    for folder in [&marker.library, &marker.state] {
        if fs::symlink_metadata(folder)
            .map_err(|_| invalid())?
            .file_type()
            .is_symlink()
        {
            return Err(invalid());
        }
        let canonical = fs::canonicalize(folder).map_err(|_| invalid())?;
        if canonical.parent() != Some(canonical_parent.as_path()) || !canonical.is_dir() {
            return Err(invalid());
        }
    }
    Ok(Some((
        fs::canonicalize(marker.library).map_err(|_| invalid())?,
        fs::canonicalize(marker.state).map_err(|_| invalid())?,
    )))
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn normal_bundle_has_no_override_and_marked_relaunch_is_forced_to_private_tmp() {
        let app = tempfile::tempdir().unwrap();
        let exe = app
            .path()
            .join("Preview.app/Contents/MacOS/learning-station");
        let resources = exe.parent().unwrap().parent().unwrap().join("Resources");
        fs::create_dir_all(&resources).unwrap();
        assert!(locations(&exe).unwrap().is_none());
        let root = PathBuf::from("/tmp").join(format!(
            "learning-station-full-qa-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let library = root.join("library");
        let state = root.join("state");
        fs::create_dir_all(&library).unwrap();
        fs::create_dir(&state).unwrap();
        let marker = resources.join(".qa-isolation.json");
        fs::write(
            &marker,
            serde_json::to_vec(&json!({"schemaVersion":1,"library":library,"state":state}))
                .unwrap(),
        )
        .unwrap();
        let (a, b) = locations(&exe).unwrap().unwrap();
        assert_eq!(a, fs::canonicalize(&library).unwrap());
        assert_eq!(b, fs::canonicalize(&state).unwrap());
        fs::write(&marker,serde_json::to_vec(&json!({"schemaVersion":1,"library":"./local-development-path","state":state})).unwrap()).unwrap();
        assert_eq!(locations(&exe).unwrap_err().code, "INVALID_QA_ISOLATION");
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn symlink_or_broken_marker_never_falls_back() {
        use std::os::unix::fs::symlink;
        let app = tempfile::tempdir().unwrap();
        let exe = app.path().join("QA.app/Contents/MacOS/learning-station");
        let resources = exe.parent().unwrap().parent().unwrap().join("Resources");
        fs::create_dir_all(&resources).unwrap();
        let marker = resources.join(".qa-isolation.json");
        symlink("missing", &marker).unwrap();
        assert!(locations(&exe).is_err());
        fs::remove_file(&marker).unwrap();
        fs::write(&marker, b"broken json").unwrap();
        assert!(locations(&exe).is_err());
    }
}
