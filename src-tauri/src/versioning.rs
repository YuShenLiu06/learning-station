//! The single version exit: snapshots, version identity, change statistics,
//! diffs and fixed-version reads. Focus/Changes build on these interfaces;
//! no other module opens a Git or content-hash path for materials.
use crate::{app::Core, file_metadata::{self, FileAddr}, material_git::{FrozenMaterials, MaterialDiff, MaterialGit}, types::*};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotInfo {
    pub commit_hash: String,
    pub material_tree_oid: String,
    pub excluded_files: Vec<crate::material_git::ExcludedFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileStat {
    pub path: String,
    /// Git name-status letter: A/M/D (rename detection disabled; a move is D + A).
    pub status: String,
    /// None for Git's binary '-' numstat, never a made-up count.
    pub added: Option<u64>,
    pub deleted: Option<u64>,
    pub binary: bool,
}

/// The whole fixed text when the caller has no base (first sight, lost base or
/// over budget), otherwise the focused file's complete Git patch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffPayload {
    Full(String),
    Diff(String),
}

fn scope() -> CoreError {
    CoreError::new("MATERIAL_SCOPE", "文件不在该课程的固定版本中。")
}
fn text_unavailable() -> CoreError {
    CoreError::new("CONTEXT_CONFLICT", "聚焦材料不是可比较的文本，不能生成差量。")
}

/// One freeze equals one snapshot: a flat course gets its physical project
/// here first, then the existing capture → init → commit chain records the
/// checkpoint exactly as freeze does.
pub fn snapshot(core: &mut Core, course: &str) -> Result<SnapshotInfo> {
    let (_, cp) = record(core, course)?;
    Ok(SnapshotInfo { commit_hash: cp.checkpoint_commit_oid, material_tree_oid: cp.material_tree_oid, excluded_files: cp.excluded_files })
}

// Course-level "current" is always a freshly recorded checkpoint, so every
// comparison uses fixed OIDs instead of the live worktree.
fn record(core: &mut Core, course: &str) -> Result<(MaterialGit, crate::material_git::Checkpoint)> {
    core.content.refresh()?;
    crate::course_project::ensure_project(&mut core.content, course)?;
    let captured = FrozenMaterials::capture_project(&core.content, course)?;
    let git = MaterialGit::init_course(core.content.root(), course)?;
    let cp = git.commit(&captured, None)?;
    Ok((git, cp))
}

/// Git blob OID for course files, content sha256 for course-less files; the
/// caller never knows which backend produced the string.
pub fn version_hash(core: &mut Core, addr: &FileAddr) -> Result<String> {
    match &addr.course {
        Some(course) => {
            let (git, cp) = record(core, course)?;
            git.files(course, &cp.checkpoint_commit_oid)?
                .into_iter()
                .find(|f| f.path == addr.path)
                .map(|f| f.blob_oid)
                .ok_or_else(scope)
        }
        None => Ok(format!("{:x}", Sha256::digest(std::fs::read(file_metadata::disk_path(core, addr)?)?))),
    }
}

/// Single-file change statistics; no patch body is materialized.
pub fn file_stat(core: &mut Core, course: &str, path: &str, base: Option<&str>) -> Result<Option<FileStat>> {
    let (git, cp) = record(core, course)?;
    Ok(change_of(&git.compare(course, base, &cp.checkpoint_commit_oid, None)?, path).map(stat_of))
}

/// Course-level change list; no diff is materialized.
pub fn all_stat(core: &mut Core, course: &str, base: Option<&str>) -> Result<Vec<FileStat>> {
    let (git, cp) = record(core, course)?;
    Ok(git.compare(course, base, &cp.checkpoint_commit_oid, None)?.changes.into_iter().map(stat_of).collect())
}

/// Full current text without a base, otherwise the focused file's patch. A
/// file unchanged since the base yields an empty patch, not a resend.
pub fn file_diff(core: &mut Core, course: &str, path: &str, base: Option<&str>) -> Result<DiffPayload> {
    let (git, cp) = record(core, course)?;
    let commit = &cp.checkpoint_commit_oid;
    match base {
        None => Ok(DiffPayload::Full(String::from_utf8(git.read(course, commit, path)?).map_err(|_| text_unavailable())?)),
        Some(base) => match change_of(&git.compare(course, Some(base), commit, Some(path))?, path) {
            Some(change) => change.patch.map(DiffPayload::Diff).ok_or_else(text_unavailable),
            None => git.files(course, commit)?.iter().any(|f| f.path == path)
                .then(|| DiffPayload::Diff(String::new()))
                .ok_or_else(scope),
        },
    }
}

/// Complete course diff: every text change carries its full patch, binary
/// changes keep Git's bare verdict, never a lossy re-encoding.
pub fn all_diff(core: &mut Core, course: &str, base: Option<&str>) -> Result<MaterialDiff> {
    let (git, cp) = record(core, course)?;
    let commit = &cp.checkpoint_commit_oid;
    let mut diff = git.compare(course, base, commit, None)?;
    for change in &mut diff.changes {
        // Unfocused compares carry statistics only; fetch each focused patch.
        if !change.binary && change.patch.is_none() && change.patch_bytes.is_none() {
            if let Some(focused) = change_of(&git.compare(course, base, commit, Some(&change.path))?, &change.path) {
                change.patch = focused.patch;
                change.patch_bytes = focused.patch_bytes;
            }
        }
    }
    Ok(diff)
}

/// Fixed-version read; `version` is a full checkpoint commit OID.
pub fn read(core: &Core, course: &str, path: &str, version: &str) -> Result<Vec<u8>> {
    MaterialGit::open_course(core.content.root(), course)?.read(course, version, path)
}

/// Returns bytes and their blob identity from one immutable checkpoint.
/// A live filesystem read followed by version_hash would mix two revisions.
pub fn read_versioned(core: &mut Core, course: &str, path: &str) -> Result<(Vec<u8>, String)> {
    let (git, cp) = record(core, course)?;
    read_versioned_at(&git, course, path, &cp.checkpoint_commit_oid)
}

fn read_versioned_at(git: &MaterialGit, course: &str, path: &str, commit: &str) -> Result<(Vec<u8>, String)> {
    let version = git.files(course, commit)?.into_iter()
        .find(|file| file.path == path).ok_or_else(scope)?.blob_oid;
    Ok((git.read(course, commit, path)?, version))
}

fn change_of(diff: &MaterialDiff, path: &str) -> Option<crate::material_git::FileChange> {
    diff.changes.iter().find(|c| c.path == path).cloned()
}

fn stat_of(change: crate::material_git::FileChange) -> FileStat {
    FileStat { path: change.path, status: change.status, added: change.added, deleted: change.deleted, binary: change.binary }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Engine;

    fn project_setup(name: &str) -> (tempfile::TempDir, std::sync::Arc<Engine>, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut c = e.lock().unwrap();
        let course = crate::library_tree::create_project(&mut c.content, "C", "", &format!("{name}-course")).unwrap();
        let note = crate::library_tree::create(&mut c.content, "section", "A", Some(&course.id), &format!("{name} 正文\n\n末段"), &format!("{name}-note")).unwrap();
        drop(c);
        (dir, e, course.id, note.id)
    }

    #[test]
    fn versioned_read_keeps_bytes_and_identity_when_disk_changes_after_capture() {
        let (_dir, e, course, _) = project_setup("versioned-read");
        let mut core = e.lock().unwrap();
        let root = crate::course_project::course_project_root(core.content.root(), &course).unwrap();
        let path = root.join("materials/summary.txt");
        std::fs::write(&path, "original bytes").unwrap();
        let (git, cp) = record(&mut core, &course).unwrap();
        let expected = git.files(&course, &cp.checkpoint_commit_oid).unwrap().into_iter().find(|f| f.path == "summary.txt").unwrap().blob_oid;
        std::fs::write(&path, "external edit not yet shown").unwrap();
        let (bytes, version) = read_versioned_at(&git, &course, "summary.txt", &cp.checkpoint_commit_oid).unwrap();
        assert_eq!(bytes, b"original bytes");
        assert_eq!(version, expected);
        let (_, latest) = read_versioned(&mut core, &course, "summary.txt").unwrap();
        assert_ne!(latest, version);
    }

    // Hand-written flat layout: library root courses/<id>.md files carrying the
    // stable course and document ids inside their metadata.
    fn flat_doc(id: &str, kind: &str, parent: Option<&str>, body: &str) -> String {
        let parent = match parent { Some(p) => format!("\"{p}\""), None => "null".into() };
        format!("```json\n{{\"id\":\"{id}\",\"title\":\"{id}\",\"kind\":\"{kind}\",\"goals\":[],\"tags\":[],\"links\":[],\"version\":1,\"parentId\":{parent},\"order\":0}}\n```\n\n{body}")
    }

    #[test]
    fn flat_course_snapshot_succeeds_and_a_second_call_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let library = dir.path().join("library");
        std::fs::create_dir_all(library.join("courses")).unwrap();
        let course = "node_flat_versioning_course";
        let child = "node_flat_versioning_child";
        std::fs::write(library.join(format!("courses/{course}.md")), flat_doc(course, "course", None, "课程根")).unwrap();
        std::fs::write(library.join(format!("courses/{child}.md")), flat_doc(child, "section", Some(course), "平铺正文\n\n末段")).unwrap();
        let mut c = e.lock().unwrap();
        c.content.refresh().unwrap();
        let first = snapshot(&mut c, course).unwrap();
        let second = snapshot(&mut c, course).unwrap();
        assert_eq!(first.material_tree_oid, second.material_tree_oid);
        assert_eq!(first.excluded_files, second.excluded_files);
        assert!(!first.commit_hash.is_empty());
        // The migrated materials are readable at the first fixed version.
        let bytes = read(&c, course, &format!("documents/{child}.md"), &first.commit_hash).unwrap();
        assert!(String::from_utf8(bytes).unwrap().contains("平铺正文"));
    }

    #[test]
    fn version_hash_uses_git_blobs_for_courses_and_sha256_without_one() {
        let (_dir, e, course, note) = project_setup("hash");
        let mut c = e.lock().unwrap();
        let addr = FileAddr { id: note.clone(), path: format!("documents/{note}.md"), kind: "markdown".into(), course: Some(course.clone()) };
        let snap = snapshot(&mut c, &course).unwrap();
        let expected = MaterialGit::open_course(c.content.root(), &course).unwrap()
            .files(&course, &snap.commit_hash).unwrap()
            .into_iter().find(|f| f.path == addr.path).unwrap().blob_oid;
        assert!(expected.len() == 40);
        assert_eq!(version_hash(&mut c, &addr).unwrap(), expected);
        let loose = crate::library_tree::create(&mut c.content, "section", "Loose", None, "无课程正文", "hash-loose").unwrap();
        let addr = FileAddr { id: loose.id.clone(), path: loose.path.clone(), kind: "markdown".into(), course: None };
        let bytes = std::fs::read(c.content.root().join(&loose.path)).unwrap();
        assert_eq!(version_hash(&mut c, &addr).unwrap(), format!("{:x}", Sha256::digest(&bytes)));
    }

    #[test]
    fn file_diff_without_base_returns_full_text_and_base_compares_return_patches() {
        let (_dir, e, course, note) = project_setup("diff");
        let path = format!("documents/{note}.md");
        let mut c = e.lock().unwrap();
        match file_diff(&mut c, &course, &path, None).unwrap() {
            DiffPayload::Full(text) => assert!(text.contains("diff 正文")),
            DiffPayload::Diff(_) => panic!("first sight must be full text"),
        }
        let base = snapshot(&mut c, &course).unwrap();
        match file_diff(&mut c, &course, &path, Some(&base.commit_hash)).unwrap() {
            DiffPayload::Diff(patch) => assert!(patch.is_empty(), "unchanged file must not resend"),
            DiffPayload::Full(_) => panic!("a registered base must produce a patch"),
        }
        let d = c.content.read(&note).unwrap();
        c.content.save(&note, &crate::content::serialize(&d.metadata, "NEW diff 正文").unwrap(), &d.version).unwrap();
        match file_diff(&mut c, &course, &path, Some(&base.commit_hash)).unwrap() {
            DiffPayload::Diff(patch) => { assert!(patch.contains("+NEW diff 正文")); assert!(patch.contains("-diff 正文")); }
            DiffPayload::Full(_) => panic!("an edited file against its base must produce a patch"),
        }
        let stat = file_stat(&mut c, &course, &path, Some(&base.commit_hash)).unwrap().unwrap();
        assert_eq!(stat.status, "M");
        assert!(!stat.binary);
        assert!(stat.added.unwrap() > 0 && stat.deleted.unwrap() > 0);
        let all = all_stat(&mut c, &course, Some(&base.commit_hash)).unwrap();
        assert!(all.iter().any(|s| s.path == path && s.status == "M"));
        let diff = all_diff(&mut c, &course, Some(&base.commit_hash)).unwrap();
        let change = diff.changes.iter().find(|ch| ch.path == path).unwrap();
        assert!(change.patch.as_deref().unwrap().contains("+NEW diff 正文"));
    }
}
