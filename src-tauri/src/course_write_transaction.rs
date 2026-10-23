//! Recoverable course-tool publication. This binds directory objects, not an
//! adversarial same-UID filesystem namespace. No automatic retry or rollback.
use crate::types::{CoreError, Result};
use std::path::{Path, PathBuf};

pub(crate) fn recovery_path(relative: &str, operation: &str) -> PathBuf {
    Path::new(relative)
        .parent()
        .unwrap_or(Path::new(""))
        .join(format!(".learning-write-{operation}"))
}
fn unknown(path: &Path) -> CoreError {
    CoreError::new(
        "NATIVE_TOOL_UNKNOWN",
        "写入需核对恢复目录；旧稿、新稿和第三方文件均未自动覆盖或重放。",
    )
    .details(serde_json::json!({"recoveryPath":path,"recovery":"inspect-only"}))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod platform {
    use super::*;
    use rustix::fd::OwnedFd;
    use rustix::fs::{self as fs, Mode, OFlags, RenameFlags, CWD};
    use std::io::{Read, Write};
    use std::path::Component;
    const DIR: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);
    const FILE: OFlags = OFlags::RDONLY
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC)
        .union(OFlags::NONBLOCK);
    fn io(e: impl std::fmt::Display) -> CoreError {
        CoreError::io(e)
    }
    fn directory(parent: &OwnedFd, name: &std::ffi::OsStr, create: bool) -> Result<OwnedFd> {
        if create {
            match fs::mkdirat(parent, name, Mode::RWXU) {
                Ok(()) => fs::fsync(parent).map_err(io)?,
                Err(rustix::io::Errno::EXIST) => {}
                Err(e) => return Err(io(e)),
            }
        }
        fs::openat(parent, name, DIR, Mode::empty()).map_err(io)
    }
    fn open_parent(root: &Path, relative: &str, create: bool) -> Result<OwnedFd> {
        if !root.is_absolute() {
            return Err(io("absolute root required"));
        }
        let mut fd = fs::openat(CWD, "/", DIR, Mode::empty()).map_err(io)?;
        for component in root.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => fd = directory(&fd, name, false)?,
                _ => return Err(io("invalid root")),
            }
        }
        for component in Path::new(relative)
            .parent()
            .unwrap_or(Path::new(""))
            .components()
        {
            let Component::Normal(name) = component else {
                return Err(io("invalid relative path"));
            };
            fd = directory(&fd, name, create)?;
        }
        Ok(fd)
    }
    fn read(fd: &OwnedFd, name: &std::ffi::OsStr) -> Result<Option<Vec<u8>>> {
        let file = match fs::openat(fd, name, FILE, Mode::empty()) {
            Ok(file) => file,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        let mut file = std::fs::File::from(file);
        if !file.metadata()?.is_file() {
            return Err(io("not regular"));
        }
        let mut bytes = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(crate::content::MAX_DOCUMENT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > crate::content::MAX_DOCUMENT_BYTES {
            return Err(io("file too large"));
        }
        Ok(Some(bytes))
    }
    fn write(fd: &OwnedFd, name: &str, bytes: &[u8]) -> Result<()> {
        let file = fs::openat(
            fd,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(io)?;
        let mut file = std::fs::File::from(file);
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
    pub(super) fn publish(
        root: &Path,
        relative: &str,
        expected: Option<&[u8]>,
        content: &[u8],
        operation: &str,
        mut checkpoint: impl FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        crate::material_git::check_path(relative)?;
        if !crate::types::valid_id(operation) {
            return Err(io("invalid operation"));
        }
        let recovery = recovery_path(relative, operation);
        let name = Path::new(relative)
            .file_name()
            .ok_or_else(|| io("missing name"))?;
        let parent = open_parent(root, relative, true)?;
        if read(&parent, name)?.as_deref() != expected {
            return Err(CoreError::new("CONFLICT", "文件已变化，未开始替换。"));
        }
        checkpoint("checked")?;
        let recovery_name = recovery.file_name().unwrap();
        fs::mkdirat(&parent, recovery_name, Mode::RWXU).map_err(|_| unknown(&recovery))?;
        let txn = directory(&parent, recovery_name, false)?;
        let outcome = (|| {
            write(&txn, "new", content)?;
            write(
                &txn,
                "manifest.json",
                &serde_json::to_vec(
                    &serde_json::json!({"version":1,"path":relative,"operation":operation,"hadOriginal":expected.is_some(),"newHash":crate::types::hash(std::str::from_utf8(content).map_err(io)?)}),
                )?,
            )?;
            fs::fsync(&txn).map_err(io)?;
            fs::fsync(&parent).map_err(io)?;
            checkpoint("prepared")?;
            if expected.is_some() {
                fs::renameat_with(&parent, name, &txn, "old", RenameFlags::NOREPLACE)
                    .map_err(io)?;
                fs::fsync(&txn).map_err(io)?;
                fs::fsync(&parent).map_err(io)?;
                checkpoint("old-moved")?;
                if read(&txn, std::ffi::OsStr::new("old"))?.as_deref() != expected {
                    return Err(unknown(&recovery));
                }
            }
            checkpoint("before-publish")?;
            // Keep `new` in the recovery directory even after publication.
            write(&txn, "publish", content)?;
            fs::fsync(&txn).map_err(io)?;
            fs::renameat_with(&txn, "publish", &parent, name, RenameFlags::NOREPLACE)
                .map_err(io)?;
            fs::fsync(&parent).map_err(io)?;
            fs::fsync(&txn).map_err(io)?;
            checkpoint("published")?;
            let current = open_parent(root, relative, false)?;
            let original = fs::fstat(&parent).map_err(io)?;
            let current = fs::fstat(&current).map_err(io)?;
            if original.st_dev != current.st_dev || original.st_ino != current.st_ino {
                return Err(unknown(&recovery));
            }
            write(&txn, "published.json", b"{\"published\":true}")?;
            fs::fsync(&txn).map_err(io)?;
            Ok(())
        })();
        if outcome.is_err() {
            // Best effort only: preserve old/new, never replace a third version.
            if let Ok(Some(bytes)) = read(&txn, std::ffi::OsStr::new("old")) {
                if write(&txn, "restore", &bytes).is_ok() {
                    let _ =
                        fs::renameat_with(&txn, "restore", &parent, name, RenameFlags::NOREPLACE);
                    let _ = fs::fsync(&parent);
                    let _ = fs::fsync(&txn);
                }
            }
        }
        outcome.map_err(|_: CoreError| unknown(&recovery))
    }
    pub(super) fn inspect(
        root: &Path,
        relative: &str,
        operation: &str,
    ) -> Result<serde_json::Value> {
        let recovery = recovery_path(relative, operation);
        let parent = open_parent(root, relative, false)?;
        let txn = directory(&parent, recovery.file_name().unwrap(), false)?;
        let mut state = serde_json::json!({"recoveryPath":recovery,"recovery":"inspect-only"});
        for name in ["manifest.json", "old", "new", "published.json"] {
            // Do not return private document bodies, only identities and existence.
            state[name] = match read(&txn, std::ffi::OsStr::new(name))? {
                Some(bytes) => {
                    serde_json::json!({"exists":true,"bytes":bytes.len(),"hash":crate::types::hash(std::str::from_utf8(&bytes).map_err(io)?)})
                }
                None => serde_json::json!({"exists":false}),
            };
        }
        state["target"] = match read(&parent, Path::new(relative).file_name().unwrap())? {
            Some(bytes) => {
                serde_json::json!({"exists":true,"bytes":bytes.len(),"hash":crate::types::hash(std::str::from_utf8(&bytes).map_err(io)?)})
            }
            None => serde_json::json!({"exists":false}),
        };
        Ok(state)
    }
}

pub(crate) fn publish(
    root: &Path,
    relative: &str,
    expected: Option<&[u8]>,
    content: &[u8],
    operation: &str,
) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    return platform::publish(root, relative, expected, content, operation, |_| Ok(()));
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (root, relative, expected, content, operation);
        Err(CoreError::new(
            "UNSUPPORTED_PLATFORM",
            "此平台尚未提供可恢复的课程写入。",
        ))
    }
}
pub(crate) fn inspect(root: &Path, relative: &str, operation: &str) -> Result<serde_json::Value> {
    crate::material_git::check_path(relative)?;
    if !crate::types::valid_id(operation) {
        return Err(unknown(Path::new("")));
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    return platform::inspect(root, relative, operation);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (root, relative, operation);
        Err(unknown(Path::new("")))
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        std::fs::write(root.join("note.txt"), b"original").unwrap();
        (d, root)
    }
    #[test]
    fn preexisting_new_target_and_stale_version_never_start_transaction() {
        let (_d, root) = fixture();
        for expected in [None, Some(b"stale".as_slice())] {
            assert_eq!(
                publish(&root, "note.txt", expected, b"model", "preflight")
                    .unwrap_err()
                    .code,
                "CONFLICT"
            );
            assert!(!root.join(recovery_path("note.txt", "preflight")).exists());
        }
        assert_eq!(std::fs::read(root.join("note.txt")).unwrap(), b"original");
    }
    #[test]
    fn final_window_external_replacement_is_retained_and_restored_without_clobber() {
        let (_d, root) = fixture();
        let result = platform::publish(
            &root,
            "note.txt",
            Some(b"original"),
            b"model",
            "race",
            |stage| {
                if stage == "prepared" {
                    std::fs::write(root.join("external.tmp"), b"external-new")?;
                    std::fs::rename(root.join("external.tmp"), root.join("note.txt"))?;
                }
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err().code, "NATIVE_TOOL_UNKNOWN");
        let recovery = root.join(recovery_path("note.txt", "race"));
        assert_eq!(
            std::fs::read(recovery.join("old")).unwrap(),
            b"external-new"
        );
        assert_eq!(std::fs::read(recovery.join("new")).unwrap(), b"model");
        assert_eq!(
            std::fs::read(root.join("note.txt")).unwrap(),
            b"external-new"
        );
    }
    #[test]
    fn third_version_created_after_old_move_survives_failed_publication() {
        let (_d, root) = fixture();
        assert!(platform::publish(
            &root,
            "note.txt",
            Some(b"original"),
            b"model",
            "third",
            |stage| {
                if stage == "before-publish" {
                    std::fs::write(root.join("note.txt"), b"third")?;
                }
                Ok(())
            }
        )
        .is_err());
        let recovery = root.join(recovery_path("note.txt", "third"));
        assert_eq!(std::fs::read(root.join("note.txt")).unwrap(), b"third");
        assert_eq!(std::fs::read(recovery.join("old")).unwrap(), b"original");
        assert_eq!(std::fs::read(recovery.join("new")).unwrap(), b"model");
    }
    #[test]
    fn replaced_parent_symlink_cannot_redirect_any_publication() {
        let (_d, root) = fixture();
        std::fs::create_dir(root.join("notes")).unwrap();
        std::fs::write(root.join("notes/note.txt"), b"original").unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("note.txt"), b"sentinel").unwrap();
        let result = platform::publish(
            &root,
            "notes/note.txt",
            Some(b"original"),
            b"model",
            "parent",
            |stage| {
                if stage == "checked" {
                    std::fs::rename(root.join("notes"), root.join("moved"))?;
                    std::os::unix::fs::symlink(outside.path(), root.join("notes"))?;
                }
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err().code, "NATIVE_TOOL_UNKNOWN");
        assert_eq!(
            std::fs::read(outside.path().join("note.txt")).unwrap(),
            b"sentinel"
        );
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(root.join("moved/note.txt")).unwrap(),
            b"model"
        );
    }
    #[test]
    fn each_interrupted_stage_is_inspectable_and_never_replays_over_third_version() {
        for stage in ["prepared", "old-moved", "published"] {
            let (_d, root) = fixture();
            assert!(platform::publish(
                &root,
                "note.txt",
                Some(b"original"),
                b"model",
                "interrupted",
                |point| {
                    if point == stage {
                        return Err(CoreError::new("IO", "injected interruption"));
                    }
                    Ok(())
                }
            )
            .is_err());
            std::fs::write(root.join("note.txt"), b"third-after-interruption").unwrap();
            let before = inspect(&root, "note.txt", "interrupted").unwrap();
            assert_eq!(before["new"]["exists"], true);
            assert_eq!(before, inspect(&root, "note.txt", "interrupted").unwrap());
            assert!(publish(
                &root,
                "note.txt",
                Some(b"third-after-interruption"),
                b"retry",
                "interrupted"
            )
            .is_err());
            assert_eq!(
                std::fs::read(root.join("note.txt")).unwrap(),
                b"third-after-interruption"
            );
        }
    }
    #[test]
    fn abrupt_interruptions_keep_durable_layout_without_running_error_recovery() {
        for stage in ["prepared", "old-moved", "published"] {
            let (_d, root) = fixture();
            let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                platform::publish(
                    &root,
                    "note.txt",
                    Some(b"original"),
                    b"model",
                    "abrupt",
                    |point| {
                        if point == stage {
                            panic!("simulated process interruption");
                        }
                        Ok(())
                    },
                )
            }));
            assert!(stopped.is_err());
            let status = inspect(&root, "note.txt", "abrupt").unwrap();
            assert_eq!(status["manifest.json"]["exists"], true);
            assert_eq!(status["new"]["exists"], true);
            assert_eq!(status["old"]["exists"], stage != "prepared");
            assert_eq!(status["target"]["exists"], stage != "old-moved");
            assert_eq!(status, inspect(&root, "note.txt", "abrupt").unwrap());
        }
    }
    #[test]
    fn symlink_replacing_original_is_preserved_without_following_it() {
        let (_d, root) = fixture();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("sentinel"), b"private").unwrap();
        assert!(platform::publish(
            &root,
            "note.txt",
            Some(b"original"),
            b"model",
            "link",
            |stage| {
                if stage == "prepared" {
                    std::fs::remove_file(root.join("note.txt"))?;
                    std::os::unix::fs::symlink(
                        outside.path().join("sentinel"),
                        root.join("note.txt"),
                    )?;
                }
                Ok(())
            }
        )
        .is_err());
        let recovery = root.join(recovery_path("note.txt", "link"));
        assert!(std::fs::symlink_metadata(recovery.join("old"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(recovery.join("new")).unwrap(), b"model");
        assert_eq!(
            std::fs::read(outside.path().join("sentinel")).unwrap(),
            b"private"
        );
    }

    #[test]
    fn success_preserves_separate_new_copy_and_original_object() {
        let (_d, root) = fixture();
        publish(&root, "note.txt", Some(b"original"), b"model", "success").unwrap();
        std::fs::write(root.join("note.txt"), b"later-in-place-edit").unwrap();
        let recovery = root.join(recovery_path("note.txt", "success"));
        assert_eq!(std::fs::read(recovery.join("new")).unwrap(), b"model");
        assert_eq!(std::fs::read(recovery.join("old")).unwrap(), b"original");
        assert!(
            inspect(&root, "note.txt", "success").unwrap()["published.json"]["exists"]
                .as_bool()
                .unwrap()
        );
    }
}
