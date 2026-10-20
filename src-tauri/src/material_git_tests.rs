use super::*;
use crate::{assets, library_tree, types::Document};
use tempfile::TempDir;

#[path = "material_git_regressions.rs"]
mod regressions;

pub(crate) fn retry_busy<T>(mut operation: impl FnMut() -> Result<T>) -> Result<T> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match operation() {
            Err(error) if error.code == "BUSY" && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            result => return result,
        }
    }
}

fn node(store: &mut ContentStore, parent: Option<&str>, body: &str, request: &str) -> Document {
    let kind = if parent.is_some() {
        "section"
    } else {
        "course"
    };
    library_tree::create(store, kind, "合成节点", parent, body, request).unwrap()
}

struct Fixture {
    dir: TempDir,
    store: ContentStore,
    course: Document,
    other: Document,
    child: Document,
    git: MaterialGit,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut store =
            ContentStore::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let course = node(&mut store, None, "first\n", "fixture_course");
        let other = node(&mut store, None, "private-other\n", "fixture_other");
        let child = node(&mut store, Some(&course.id), "old\n", "fixture_child");
        let git = MaterialGit::create_new(&dir.path().join("materials.git")).unwrap();
        Self {
            dir,
            store,
            course,
            other,
            child,
            git,
        }
    }
    fn freeze(&self) -> FrozenMaterials {
        retry_busy(|| FrozenMaterials::capture(&self.store, &self.course.id)).unwrap()
    }
    fn commit(&self, parent: Option<&str>) -> Checkpoint {
        self.git.commit(&self.freeze(), parent).unwrap()
    }
    fn diff(
        &self,
        old: Option<&Checkpoint>,
        new: &Checkpoint,
        focus: Option<&str>,
    ) -> MaterialDiff {
        self.git
            .compare(
                &self.course.id,
                old.map(|cp| cp.checkpoint_commit_oid.as_str()),
                &new.checkpoint_commit_oid,
                focus,
            )
            .unwrap()
    }
    fn read(&self, cp: &Checkpoint, path: &str) -> Vec<u8> {
        self.git
            .read(&self.course.id, &cp.checkpoint_commit_oid, path)
            .unwrap()
    }
    fn import(&self, name: &str, bytes: &[u8], request: &str) -> assets::Asset {
        // Import's BUSY sites precede asset writes. Keep the same payload and
        // request ID; other errors (including uncertain IO) are never retried.
        retry_busy(|| assets::import(&self.store, &self.course.id, name, bytes, request)).unwrap()
    }
    fn child_path(&self) -> String {
        format!("documents/{}.md", self.child.id)
    }
    fn save_child(&mut self, body: &str) {
        let content = crate::content::serialize(&self.child.metadata, body).unwrap();
        self.child = self
            .store
            .save(&self.child.id, &content, &self.child.version)
            .unwrap();
    }
}

#[test]
fn first_unchanged_and_reopen_use_reachable_git_objects() {
    let f = Fixture::new();
    let first = f.commit(None);
    let files = f
        .git
        .files(&f.course.id, &first.checkpoint_commit_oid)
        .unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(f.read(&first, &f.child_path()), f.child.content.as_bytes());
    let initial = f.diff(None, &first, None);
    assert!(initial.initial);
    assert!(initial
        .changes
        .iter()
        .all(|c| c.status == "A" && c.patch.is_none()));
    let second = f.commit(Some(&first.checkpoint_commit_oid));
    assert_ne!(first.checkpoint_commit_oid, second.checkpoint_commit_oid);
    assert_eq!(first.material_tree_oid, second.material_tree_oid);
    assert!(f.diff(Some(&first), &second, None).changes.is_empty());
    f.git.git(&["gc", "--prune=now"], &[]).unwrap();
    let reopened = MaterialGit::open(&f.dir.path().join("materials.git")).unwrap();
    assert_eq!(
        reopened
            .checkpoint(&f.course.id, &first.checkpoint_commit_oid)
            .unwrap(),
        first
    );
    assert!(reopened
        .read(&f.other.id, &first.checkpoint_commit_oid, &f.child_path())
        .is_err());
}

#[test]
fn text_add_delete_modify_and_large_focused_patch_are_complete() {
    let mut f = Fixture::new();
    let doomed = node(&mut f.store, Some(&f.course.id), "doomed\n", "doomed_1");
    let first = f.commit(None);
    let old_content = f.child.content.clone();
    let large = format!("{}tail-marker\n", "added line\n".repeat(30000));
    f.save_child(&large);
    fs::remove_file(f.store.root().join(&doomed.path)).unwrap();
    f.store.forget(&doomed.id);
    let added = node(&mut f.store, Some(&f.course.id), "new\n", "fixture_added");
    let next = f.commit(Some(&first.checkpoint_commit_oid));
    let result = f.diff(Some(&first), &next, Some(&f.child_path()));
    let changed = result
        .changes
        .iter()
        .find(|c| c.path == f.child_path())
        .unwrap();
    assert_eq!(changed.status, "M");
    let patch = changed.patch.as_ref().unwrap();
    assert!(patch.len() > 300000 && patch.contains("+tail-marker\n") && patch.contains("-old\n"));
    let full_git = f
        .git
        .git(
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--no-color",
                "--patch",
                "--full-index",
                &first.material_tree_oid,
                &next.material_tree_oid,
                "--",
                &f.child_path(),
            ],
            &[],
        )
        .unwrap();
    assert_eq!(patch.as_bytes(), full_git);
    assert!(result
        .changes
        .iter()
        .any(|c| c.path == format!("documents/{}.md", added.id)
            && c.status == "A"
            && c.added.unwrap() > 0
            && c.patch.is_none()));
    assert!(result
        .changes
        .iter()
        .any(|c| c.path == format!("documents/{}.md", doomed.id)
            && c.status == "D"
            && c.deleted.unwrap() > 0));
    assert_eq!(f.read(&first, &f.child_path()), old_content.as_bytes());
    assert!(f
        .git
        .read(
            &f.course.id,
            &next.checkpoint_commit_oid,
            &format!("documents/{}.md", doomed.id)
        )
        .is_err());
}

#[test]
fn attachments_include_full_text_binary_and_file_level_deletion() {
    let f = Fixture::new();
    let first = f.commit(None);
    let text = format!("{}complete-tail", "x".repeat(150000));
    let attachment = f.import("资料.txt", text.as_bytes(), "fixture_text");
    let png = b"\x89PNG\r\n\x1a\n\0binary-fixture";
    let image = f.import("图.png", png, "fixture_image");
    let binary_path = format!("attachments/{}/图.png", image.id);
    let next = f.commit(Some(&first.checkpoint_commit_oid));
    let result = f.diff(Some(&first), &next, Some(&binary_path));
    let binary = result
        .changes
        .iter()
        .find(|c| c.path == binary_path)
        .unwrap();
    assert!(binary.binary && binary.added.is_none() && binary.patch.is_none());
    assert_eq!(f.read(&next, &binary_path), png);
    assert_eq!(
        f.read(&next, &format!("attachments/{}/资料.txt", attachment.id)),
        text.as_bytes()
    );
    // Replace only synthetic fixture storage and metadata.
    let image_dir = f
        .store
        .root()
        .join(format!("assets/{}/{}", f.course.id, image.id));
    fs::remove_dir_all(&image_dir).unwrap();
    let replaced = f.import("图.png", b"\x89PNG\r\n\x1a\n\0changed", "fixture_image");
    assert_eq!(replaced.id, image.id);
    let changed = f.commit(Some(&next.checkpoint_commit_oid));
    let result = f.diff(Some(&next), &changed, Some(&binary_path));
    assert_eq!(result.changes[0].status, "M");
    assert!(result.changes[0].binary && result.changes[0].patch.is_none());
    fs::remove_dir_all(&image_dir).unwrap();
    let deleted = f.commit(Some(&changed.checkpoint_commit_oid));
    let result = f.diff(Some(&changed), &deleted, None);
    assert_eq!(result.changes[0].status, "D");
    assert!(result.changes[0].binary);
}

#[test]
fn frozen_bytes_survive_concurrent_writes_and_other_courses_stay_isolated() {
    let mut f = Fixture::new();
    let frozen = f.freeze();
    let old = f.child.content.clone();
    let path = f.store.root().join(&f.child.path);
    node(&mut f.store, Some(&f.other.id), "other\n", "other_ch");
    let done = std::sync::atomic::AtomicBool::new(false);
    let checkpoint = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                fs::write(&path, b"concurrent edit").unwrap();
            }
        });
        let checkpoint = f.git.commit(&frozen, None).unwrap();
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        checkpoint
    });
    assert_eq!(f.read(&checkpoint, &f.child_path()), old.as_bytes());
    fs::write(&path, old).unwrap();
    let unchanged = f.commit(Some(&checkpoint.checkpoint_commit_oid));
    assert_eq!(checkpoint.material_tree_oid, unchanged.material_tree_oid);
}

#[test]
fn secrets_configs_caches_source_and_transcripts_are_excluded() {
    let f = Fixture::new();
    let first = f.commit(None);
    for name in [".env", "auth.json", "credentials.json", "models.json"] {
        fs::write(f.store.root().join(name), b"fixture-secret-never-real").unwrap();
    }
    for dir in [".git", "cache", "node_modules", "src", "pi-native"] {
        fs::create_dir(f.store.root().join(dir)).unwrap();
        fs::write(
            f.store.root().join(dir).join("private.md"),
            b"fixture-private",
        )
        .unwrap();
    }
    f.import("provider-config.json", b"{}", "fixture_config");
    retry_busy(|| {
        assets::import(
            &f.store,
            &f.other.id,
            "other.txt",
            b"private-other",
            "fixture_other_asset",
        )
    })
    .unwrap();
    let next = f.commit(Some(&first.checkpoint_commit_oid));
    assert_eq!(first.material_tree_oid, next.material_tree_oid);
}

#[test]
fn future_transcript_only_commit_changes_commit_identity_not_material_identity() {
    let f = Fixture::new();
    let first = f.commit(None);
    let materials = f
        .git
        .tree_entries(&[("tree", &first.material_tree_oid, &f.course.id)])
        .unwrap();
    let transcript = f
        .git
        .text(
            &["hash-object", "-w", "--stdin"],
            b"{\"id\":\"pi-entry-stable\"}\n",
        )
        .unwrap();
    let pi = f
        .git
        .tree_entries(&[("blob", &transcript, "session-stable.jsonl")])
        .unwrap();
    let root = f
        .git
        .tree_entries(&[
            ("tree", &materials, "materials"),
            ("tree", &pi, "pi-native"),
        ])
        .unwrap();
    let oid = f
        .git
        .text(
            &["commit-tree", &root, "-p", &first.checkpoint_commit_oid],
            b"synthetic future transcript only\n",
        )
        .unwrap();
    let next = f.git.checkpoint(&f.course.id, &oid).unwrap();
    assert_ne!(first.checkpoint_commit_oid, next.checkpoint_commit_oid);
    assert_eq!(first.material_tree_oid, next.material_tree_oid);
    assert!(f
        .git
        .compare(&f.course.id, Some(&first.checkpoint_commit_oid), &oid, None)
        .unwrap()
        .changes
        .is_empty());
}

#[test]
fn failed_git_ref_write_preserves_old_checkpoint_and_retry_recovers() {
    let mut f = Fixture::new();
    let first = f.commit(None);
    f.save_child("second\n");
    let frozen = f.freeze();
    let refs = f.git.directory.join("refs/checkpoints").join(&f.course.id);
    let backup = refs.with_extension("saved");
    fs::rename(&refs, &backup).unwrap();
    fs::write(&refs, b"block ref creation").unwrap();
    assert_eq!(
        f.git
            .commit(&frozen, Some(&first.checkpoint_commit_oid))
            .unwrap_err()
            .code,
        "MATERIAL_GIT"
    );
    assert_eq!(
        f.git
            .checkpoint(&f.course.id, &first.checkpoint_commit_oid)
            .unwrap(),
        first
    );
    fs::remove_file(&refs).unwrap();
    fs::rename(backup, refs).unwrap();
    let next = f
        .git
        .commit(&frozen, Some(&first.checkpoint_commit_oid))
        .unwrap();
    assert_ne!(first.material_tree_oid, next.material_tree_oid);
    assert!(!f.diff(Some(&first), &next, None).changes.is_empty());
}

#[test]
fn malicious_paths_oids_and_foreign_repositories_are_rejected() {
    let f = Fixture::new();
    let first = f.commit(None);
    for path in [
        "../secret",
        "/etc/passwd",
        "a/../../b",
        ".git/config",
        "a\\b",
        "a:b",
        "a\nb",
        ":(glob)*",
        "a//b",
    ] {
        assert!(f
            .git
            .read(&f.course.id, &first.checkpoint_commit_oid, path)
            .is_err());
        assert!(f
            .git
            .compare(&f.course.id, None, &first.checkpoint_commit_oid, Some(path))
            .is_err());
    }
    for oid in ["HEAD", "--help", "abc", "HEAD^{tree}"] {
        assert!(f.git.checkpoint(&f.course.id, oid).is_err());
    }
    assert!(MaterialGit::create_new(&f.git.directory).is_err());
    assert!(MaterialGit::open(f.dir.path()).is_err());
    let source = f.dir.path().join("software");
    fs::create_dir(&source).unwrap();
    fs::create_dir(source.join(".git")).unwrap();
    assert!(MaterialGit::create_new(&source.join("materials.git")).is_err());
    assert!(!source.join("materials.git").exists());
}

#[test]
#[cfg(unix)]
fn material_symlinks_and_git_hooks_never_run() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let f = Fixture::new();
    let path = f.store.root().join(&f.child.path);
    fs::remove_file(&path).unwrap();
    let outside = f.dir.path().join("outside.md");
    fs::write(&outside, &f.child.content).unwrap();
    symlink(&outside, &path).unwrap();
    assert!(FrozenMaterials::capture(&f.store, &f.course.id).is_err());
    fs::remove_file(&path).unwrap();
    fs::write(&path, &f.child.content).unwrap();
    symlink(f.dir.path(), f.store.root().join("assets")).unwrap();
    assert!(FrozenMaterials::capture(&f.store, &f.course.id).is_err());
    fs::remove_file(f.store.root().join("assets")).unwrap();
    fs::create_dir_all(f.git.directory.join("hooks")).unwrap();
    let marker = f.dir.path().join("executed");
    for name in ["pre-commit", "reference-transaction", "external-diff"] {
        let script = f.git.directory.join("hooks").join(name);
        fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(script, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let external = f.git.directory.join("hooks/external-diff");
    f.git
        .git(
            &["config", "diff.external", external.to_str().unwrap()],
            &[],
        )
        .unwrap();
    let first = f.commit(None);
    f.diff(None, &first, Some(&f.child_path()));
    assert!(!marker.exists());
}
