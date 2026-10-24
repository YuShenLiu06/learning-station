use crate::{
    assets,
    content::{self, ContentStore},
    course_project::{course_project_root, create_material},
    material_git::{Checkpoint, FrozenMaterials, MaterialDiff, MaterialGit},
    types::{Document, Metadata, Result},
};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
use tempfile::TempDir;

const CHILD: &str = "documents/child_a.md";
fn metadata(id: &str, parent: Option<&str>) -> Metadata {
    let kind = parent.map_or("course", |_| "section");
    serde_json::from_value(serde_json::json!({
        "id": id, "title": id, "kind": kind, "parentId": parent
    }))
    .unwrap()
}
fn retry_busy<T>(mut operation: impl FnMut() -> Result<T>) -> Result<T> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match operation() {
            Err(error) if error.code == "BUSY" && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            result => return result,
        }
    }
}
struct Fixture {
    dir: TempDir,
    store: ContentStore,
    git: MaterialGit,
    root: std::path::PathBuf,
    child: Document,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut store =
            ContentStore::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        create_material(&mut store, metadata("course_a", None), "course a\n").unwrap();
        create_material(&mut store, metadata("course_b", None), "private b\n").unwrap();
        let child =
            create_material(&mut store, metadata("child_a", Some("course_a")), "old\n").unwrap();
        let git = MaterialGit::init_course(store.root(), "course_a").unwrap();
        let root = course_project_root(store.root(), "course_a").unwrap();
        Self {
            dir,
            store,
            git,
            root,
            child,
        }
    }
    fn freeze(&self) -> FrozenMaterials {
        retry_busy(|| FrozenMaterials::capture_project(&self.store, "course_a")).unwrap()
    }
    fn commit(&self, parent: Option<&str>) -> Checkpoint {
        self.git.commit(&self.freeze(), parent).unwrap()
    }
    fn import(&self, name: &str, bytes: &[u8], request: &str) -> assets::Asset {
        retry_busy(|| assets::import(&self.store, "course_a", name, bytes, request)).unwrap()
    }
    fn read(&self, cp: &Checkpoint, path: &str) -> Vec<u8> {
        self.git
            .read("course_a", &cp.checkpoint_commit_oid, path)
            .unwrap()
    }
    fn diff(&self, old: &Checkpoint, new: &Checkpoint, focus: Option<&str>) -> MaterialDiff {
        self.git
            .compare(
                "course_a",
                Some(&old.checkpoint_commit_oid),
                &new.checkpoint_commit_oid,
                focus,
            )
            .unwrap()
    }
}

// Restricted Git plumbing touches only this test's temporary course repository.
fn git(root: &Path, args: &[&str], input: &[u8]) -> Vec<u8> {
    let mut child = Command::new("git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("LC_ALL", "C")
        .env("GIT_AUTHOR_NAME", "Synthetic Test")
        .env("GIT_AUTHOR_EMAIL", "test@localhost")
        .env("GIT_COMMITTER_NAME", "Synthetic Test")
        .env("GIT_COMMITTER_EMAIL", "test@localhost")
        .current_dir(root)
        .args(["--no-pager", "-c", "core.hooksPath=/dev/null"])
        .args(["-c", "core.attributesFile=/dev/null"])
        .args(["-c", "commit.gpgSign=false"])
        .args(["-c", "protocol.allow=never"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "temporary Git failed: {args:?}");
    output.stdout
}
fn git_text(root: &Path, args: &[&str], input: &[u8]) -> String {
    String::from_utf8(git(root, args, input))
        .unwrap()
        .trim()
        .into()
}

#[test]
fn courses_have_independent_normal_git_and_reject_wrong_repository_or_freeze() {
    let f = Fixture::new();
    let first = f.commit(None);
    let b = MaterialGit::init_course(f.store.root(), "course_b").unwrap();
    let b_root = course_project_root(f.store.root(), "course_b").unwrap();
    let b_frozen = retry_busy(|| FrozenMaterials::capture_project(&f.store, "course_b")).unwrap();
    let b_cp = b.commit(&b_frozen, None).unwrap();
    assert_eq!(
        git_text(&f.root, &["rev-parse", "--is-bare-repository"], &[]),
        "false"
    );
    assert!(f.root.join(".git/objects").is_dir() && b_root.join(".git/objects").is_dir());
    assert!(!f.root.join(".git/objects/info/alternates").exists());
    assert!(!b_root.join(".git/objects/info/alternates").exists());
    let tree = format!("{}:materials", first.checkpoint_commit_oid);
    assert_eq!(
        git_text(&f.root, &["rev-parse", &tree], &[]),
        first.material_tree_oid
    );
    assert!(f.git.commit(&b_frozen, None).is_err());
    assert!(f
        .git
        .checkpoint("course_b", &b_cp.checkpoint_commit_oid)
        .is_err());
    assert!(b
        .read("course_a", &first.checkpoint_commit_oid, CHILD)
        .is_err());
    let reopened = MaterialGit::open_course(f.store.root(), "course_a").unwrap();
    assert_eq!(
        reopened
            .checkpoint("course_a", &first.checkpoint_commit_oid)
            .unwrap(),
        first
    );
}

#[test]
fn fixed_reads_and_complete_patch_stats_and_binary_use_physical_material_paths() {
    let f = Fixture::new();
    let first = f.commit(None);
    let large = format!("{}tail-marker\n", "added line\n".repeat(30000));
    fs::write(
        f.store.root().join(&f.child.path),
        content::serialize(&f.child.metadata, &large).unwrap(),
    )
    .unwrap();
    let png = b"\x89PNG\r\n\x1a\n\0binary-fixture";
    let image = f.import("图%:片.png", png, "course_git_image");
    let image_path = format!("assets/course_a/{}/图%25%3A片.png", image.id);
    let next = f.commit(Some(&first.checkpoint_commit_oid));
    let diff = f.diff(&first, &next, Some(CHILD));
    let changed = diff.changes.iter().find(|c| c.path == CHILD).unwrap();
    assert_eq!(changed.status, "M");
    assert_eq!(
        (changed.added, changed.deleted, changed.binary),
        (Some(30001), Some(1), false)
    );
    let patch = changed.patch.as_ref().unwrap();
    assert!(patch.len() > 300000 && patch.contains("+tail-marker\n") && patch.contains("-old\n"));
    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-renames"];
    args.extend(["--no-color", "--patch", "--full-index"]);
    args.extend([
        first.material_tree_oid.as_str(),
        next.material_tree_oid.as_str(),
        "--",
        CHILD,
    ]);
    assert_eq!(patch.as_bytes(), git(&f.root, &args, &[]));
    let binary = diff.changes.iter().find(|c| c.path == image_path).unwrap();
    assert!(
        binary.binary
            && binary.added.is_none()
            && binary.deleted.is_none()
            && binary.patch.is_none()
    );
    assert_eq!(f.read(&first, CHILD), f.child.content.as_bytes());
    assert_eq!(f.read(&next, &image_path), png);
    assert!(f
        .git
        .read("course_b", &next.checkpoint_commit_oid, &image_path)
        .is_err());
    assert!(f.git.read("course_a", "HEAD", CHILD).is_err());
}

#[test]
fn frozen_project_bytes_survive_later_native_document_and_asset_edits() {
    let f = Fixture::new();
    let asset = f.import("资料.txt", b"asset before\n", "course_git_frozen");
    let frozen = f.freeze();
    fs::write(
        f.store.root().join(&f.child.path),
        content::serialize(&f.child.metadata, "native after\n").unwrap(),
    )
    .unwrap();
    let asset_path = format!("assets/course_a/{}/资料.txt", asset.id);
    fs::write(f.root.join("materials").join(&asset_path), b"asset after\n").unwrap();
    let first = f.git.commit(&frozen, None).unwrap();
    assert_eq!(f.read(&first, CHILD), f.child.content.as_bytes());
    assert_eq!(f.read(&first, &asset_path), b"asset before\n");
    let next = f.commit(Some(&first.checkpoint_commit_oid));
    assert_ne!(first.material_tree_oid, next.material_tree_oid);
    assert_eq!(f.read(&next, &asset_path), b"asset after\n");
}

#[test]
fn secrets_caches_nested_git_are_excluded_and_material_symlinks_are_rejected() {
    let f = Fixture::new();
    let first = f.commit(None);
    for path in [
        ".env",
        "auth.json",
        "cache/private.md",
        "materials/cache/private.md",
        "materials/documents/nested/.git/private.md",
    ] {
        let path = f.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"synthetic secret never real\n").unwrap();
    }
    let sensitive = f.import("provider-config.json", b"{}", "course_git_secret");
    let next = f.commit(Some(&first.checkpoint_commit_oid));
    assert_eq!(first.material_tree_oid, next.material_tree_oid);
    assert!(next
        .excluded_files
        .iter()
        .any(|entry| entry.asset_id == sensitive.id));
    #[cfg(unix)]
    {
        let outside = f.dir.path().join("outside.txt");
        fs::write(&outside, b"outside fixture\n").unwrap();
        std::os::unix::fs::symlink(&outside, f.root.join("materials/linked.txt")).unwrap();
        assert!(retry_busy(|| FrozenMaterials::capture_project(&f.store, "course_a")).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"outside fixture\n");
    }
}

#[test]
fn synthetic_transcript_only_commit_preserves_the_material_tree() {
    let f = Fixture::new();
    let first = f.commit(None);
    let entry = b"{\"id\":\"synthetic-native-entry\"}\n";
    let transcript = git_text(&f.root, &["hash-object", "-w", "--stdin"], entry);
    let mut entries = git(&f.root, &["ls-tree", &first.checkpoint_commit_oid], &[]);
    entries.extend_from_slice(format!("100644 blob {transcript}\ttranscript.jsonl\n").as_bytes());
    let tree = git_text(&f.root, &["mktree"], &entries);
    let oid = git_text(
        &f.root,
        &["commit-tree", &tree, "-p", &first.checkpoint_commit_oid],
        b"synthetic transcript only\n",
    );
    git(
        &f.root,
        &["update-ref", "refs/synthetic/transcript", &oid],
        &[],
    );
    let next = f.git.checkpoint("course_a", &oid).unwrap();
    assert_ne!(first.checkpoint_commit_oid, next.checkpoint_commit_oid);
    assert_eq!(first.material_tree_oid, next.material_tree_oid);
    assert!(f.diff(&first, &next, None).changes.is_empty());
    assert_eq!(f.git.stat("course_a", Some(&first.checkpoint_commit_oid), &oid).unwrap(), "");
    fs::write(
        f.root.join("unregistered-chat.jsonl"),
        b"not automatically included",
    )
    .unwrap();
    let after = f.git.commit(&f.freeze(), Some(&oid)).unwrap();
    let transcript_path = format!("{}:transcript.jsonl", after.checkpoint_commit_oid);
    assert_eq!(
        git(&f.root, &["cat-file", "blob", &transcript_path], &[]),
        entry
    );
    assert!(git(
        &f.root,
        &[
            "ls-tree",
            &after.checkpoint_commit_oid,
            "--",
            "unregistered-chat.jsonl"
        ],
        &[]
    )
    .is_empty());
}

#[test]
fn legacy_bare_repository_reopens_for_reads_but_cannot_commit() {
    let dir = tempfile::tempdir().unwrap();
    let mut store =
        ContentStore::new(dir.path().join("library"), dir.path().join("state")).unwrap();
    let doc = store
        .create(metadata("legacy", None), "legacy\n", "courses/legacy.md")
        .unwrap();
    let frozen = retry_busy(|| FrozenMaterials::capture(&store, &doc.id)).unwrap();
    let path = dir.path().join("legacy.git");
    let writer = MaterialGit::create_new(&path).unwrap();
    let first = writer.commit(&frozen, None).unwrap();
    let reader = MaterialGit::open(&path).unwrap();
    let oid = &first.checkpoint_commit_oid;
    let bytes = reader.read(&doc.id, oid, "documents/legacy.md").unwrap();
    assert_eq!(bytes, doc.content.as_bytes());
    let diff = reader.compare(&doc.id, Some(oid), oid, None).unwrap();
    assert!(diff.changes.is_empty());
    assert!(reader.commit(&frozen, Some(oid)).is_err());
    assert!(MaterialGit::open_course(store.root(), &doc.id).is_err());
    // Initial comparison is read-only as well: Git computes the native empty
    // tree identity without mktree/-w on a legacy archive.
    assert!(reader.compare(&doc.id, None, oid, None).unwrap().initial);
    let model_path = reader.repository_path(&doc.id, "documents/legacy.md").unwrap();
    assert_eq!(model_path, "materials/legacy/documents/legacy.md");
    let stat = reader.stat(&doc.id, None, oid).unwrap();
    let empty = git_text(&path, &["hash-object", "-t", "tree", "--stdin"], &[]);
    assert_eq!(stat.as_bytes(), git(&path, &["diff", "--no-ext-diff", "--no-textconv", "--no-color", "--find-renames", "--stat", &empty, oid, "--", "materials/legacy"], &[]));
    assert!(stat.contains(&model_path));
    assert!(reader.repository_path(&doc.id, "../outside.md").is_err());
}

#[test]
fn model_stat_is_native_root_diff_including_rename_binary_delete_and_focused_file() {
    let f = Fixture::new();
    fs::write(f.root.join("materials/rename-before.txt"), b"rename unchanged\n").unwrap();
    fs::write(f.root.join("materials/delete.txt"), b"delete this\n").unwrap();
    fs::write(f.root.join("materials/picture.png"), b"\x89PNG\r\n\x1a\n\0before").unwrap();
    let first = f.commit(None);
    fs::rename(f.root.join("materials/rename-before.txt"), f.root.join("materials/rename-after.txt")).unwrap();
    fs::remove_file(f.root.join("materials/delete.txt")).unwrap();
    fs::write(f.root.join("materials/picture.png"), b"\x89PNG\r\n\x1a\n\0after").unwrap();
    fs::write(f.store.root().join(&f.child.path), content::serialize(&f.child.metadata, "focused change\n").unwrap()).unwrap();
    let frozen = f.freeze();
    // None of these later worktree edits may be sampled by stat or patch.
    fs::write(f.root.join("materials/delete.txt"), b"late unrelated\n").unwrap();
    fs::write(f.root.join("materials/late.txt"), b"late addition\n").unwrap();
    fs::write(f.store.root().join(&f.child.path), content::serialize(&f.child.metadata, "late focus\n").unwrap()).unwrap();
    let next = f.git.commit(&frozen, Some(&first.checkpoint_commit_oid)).unwrap();
    let old = &first.checkpoint_commit_oid;
    let new = &next.checkpoint_commit_oid;
    let stat = f.git.stat("course_a", Some(old), new).unwrap();
    assert_eq!(stat.as_bytes(), git(&f.root, &["diff", "--no-ext-diff", "--no-textconv", "--no-color", "--find-renames", "--stat", old, new, "--", "materials"], &[]));
    assert!(stat.contains("materials/documents/child_a.md"));
    assert!(stat.contains("materials/{rename-before.txt => rename-after.txt}"));
    assert!(stat.contains("materials/picture.png") && stat.contains("Bin"));
    assert!(stat.contains("materials/delete.txt") && stat.contains("deletion"));
    assert!(!stat.contains("late") && !stat.contains("material-exclusions"));
    let patch = f.git.patch("course_a", old, new, CHILD).unwrap();
    assert_eq!(patch, git(&f.root, &["diff", "--no-ext-diff", "--no-textconv", "--no-color", "--no-renames", "--patch", "--full-index", old, new, "--", "materials/documents/child_a.md"], &[]));
    let patch = String::from_utf8(patch).unwrap();
    assert!(patch.contains("+++ b/materials/documents/child_a.md\n") && patch.contains("+focused change\n"));
    assert!(!patch.contains("late focus"));
    assert!(f.git.repository_path("course_b", CHILD).is_err());
    assert!(f.git.stat("course_a", Some("HEAD"), new).is_err());
    assert!(f.git.patch("course_a", old, new, "../outside.md").is_err());
}

#[test]
fn body_patch_uses_real_trees_at_repository_path_without_document_metadata() {
    let f = Fixture::new();
    let path = f.git.repository_path("course_a", CHILD).unwrap();
    let old = "old body\nunchanged\n";
    let new = "new body 😀\nunchanged\n";
    let patch = f.git.compare_bodies(old, new, &path).unwrap();
    let tree = |text: &str| {
        let blob = git_text(&f.root, &["hash-object", "-w", "--stdin"], text.as_bytes());
        let documents = git_text(&f.root, &["mktree"], format!("100644 blob {blob}\tchild_a.md\n").as_bytes());
        let materials = git_text(&f.root, &["mktree"], format!("040000 tree {documents}\tdocuments\n").as_bytes());
        git_text(&f.root, &["mktree"], format!("040000 tree {materials}\tmaterials\n").as_bytes())
    };
    let old_tree = tree(old);
    let new_tree = tree(new);
    assert_eq!(patch.as_bytes(), git(&f.root, &["diff", "--no-ext-diff", "--no-textconv", "--no-color", "--no-renames", &old_tree, &new_tree, "--", &path], &[]));
    assert!(patch.starts_with("diff --git a/materials/documents/child_a.md b/materials/documents/child_a.md\n"));
    assert!(patch.contains("--- a/materials/documents/child_a.md\n") && patch.contains("+++ b/materials/documents/child_a.md\n"));
    assert!(patch.contains("-old body\n+new body 😀\n"));
    assert!(!patch.contains("parentId") && !patch.contains("createRequestDigest"));
    assert!(f.git.compare_bodies(old, new, "/outside.md").is_err());
}

#[test]
fn replaced_or_shared_repository_is_rejected_and_git_failure_keeps_old_receipt() {
    let f = Fixture::new();
    let first = f.commit(None);
    let frozen = f.freeze();
    let refs = f.root.join(".git/refs/checkpoints/course_a");
    let backup = f.root.join(".git/refs/checkpoints/saved");
    fs::rename(&refs, &backup).unwrap();
    fs::write(&refs, b"block ref namespace").unwrap();
    let error = f
        .git
        .commit(&frozen, Some(&first.checkpoint_commit_oid))
        .unwrap_err();
    assert_eq!(error.code, "MATERIAL_GIT");
    fs::remove_file(&refs).unwrap();
    fs::rename(&backup, &refs).unwrap();
    let retry = f
        .git
        .commit(&frozen, Some(&first.checkpoint_commit_oid))
        .unwrap();
    assert_eq!(retry.material_tree_oid, first.material_tree_oid);
    assert_eq!(f.read(&first, CHILD), f.child.content.as_bytes());
    let marker = f.root.join(".git/learning-materials-v2");
    fs::write(&marker, "course_b").unwrap();
    assert!(f
        .git
        .commit(&frozen, Some(&first.checkpoint_commit_oid))
        .is_err());
    assert!(MaterialGit::open_course(f.store.root(), "course_a").is_err());
    fs::write(&marker, "course_a").unwrap();
    fs::write(
        f.root.join(".git/objects/info/alternates"),
        f.root.join(".git/objects").to_str().unwrap(),
    )
    .unwrap();
    assert!(MaterialGit::open_course(f.store.root(), "course_a").is_err());
    fs::remove_file(f.root.join(".git/objects/info/alternates")).unwrap();
    assert_eq!(f.read(&first, CHILD), f.child.content.as_bytes());
}

#[test]
fn unchanged_and_same_course_file_add_delete_use_actual_material_files() {
    let mut f = Fixture::new();
    let image = f.import(
        "diagram.png",
        b"\x89PNG\r\n\x1a\n\0fixture",
        "delete_image_request",
    );
    let first = f.commit(None);
    let same = f.commit(Some(&first.checkpoint_commit_oid));
    assert_eq!(first.material_tree_oid, same.material_tree_oid);
    assert!(f.diff(&first, &same, None).changes.is_empty());
    fs::remove_file(f.store.root().join(&f.child.path)).unwrap();
    let image_path = format!("assets/course_a/{}/diagram.png", image.id);
    fs::remove_file(f.root.join("materials").join(&image_path)).unwrap();
    create_material(
        &mut f.store,
        metadata("new_section", Some("course_a")),
        "new material\n",
    )
    .unwrap();
    let next = f.commit(Some(&same.checkpoint_commit_oid));
    let diff = f.diff(&same, &next, None);
    assert!(diff
        .changes
        .iter()
        .any(|c| c.path == CHILD && c.status == "D" && c.patch.is_none()));
    assert!(diff
        .changes
        .iter()
        .any(|c| c.path == image_path && c.status == "D" && c.binary));
    assert!(diff
        .changes
        .iter()
        .any(|c| c.path == "documents/new_section.md" && c.status == "A" && c.added.is_some()));
    assert_eq!(f.read(&first, CHILD), f.child.content.as_bytes());
}

#[test]
fn trashed_material_is_excluded_until_restore_while_fixed_history_remains_readable() {
    let mut f = Fixture::new();
    let first = f.commit(None);
    let preview = crate::trash::preview(&mut f.store, &f.child.id).unwrap();
    let receipt = crate::trash::trash(
        &mut f.store,
        &f.child.id,
        &preview.version,
        "course_git_trash_child",
    )
    .unwrap();
    assert_eq!(f.read(&first, CHILD), f.child.content.as_bytes());
    assert_eq!(
        fs::read(f.store.root().join(&f.child.path)).unwrap(),
        f.child.content.as_bytes()
    );
    let hidden = f.commit(Some(&first.checkpoint_commit_oid));
    let oid = &hidden.checkpoint_commit_oid;
    let files = f.git.files("course_a", oid).unwrap();
    assert!(!files.iter().any(|file| file.path == CHILD));
    assert!(f.git.read("course_a", oid, CHILD).is_err());
    assert!(f
        .diff(&first, &hidden, Some(CHILD))
        .changes
        .iter()
        .any(|c| c.path == CHILD && c.status == "D"));
    crate::trash::restore(&mut f.store, &receipt.trash_id, "course_git_restore_child").unwrap();
    let restored = f.commit(Some(oid));
    let restored_oid = &restored.checkpoint_commit_oid;
    let files = f.git.files("course_a", restored_oid).unwrap();
    assert!(files.iter().any(|file| file.path == CHILD));
    assert_eq!(f.read(&restored, CHILD), f.child.content.as_bytes());
    assert!(f
        .diff(&hidden, &restored, Some(CHILD))
        .changes
        .iter()
        .any(|c| c.path == CHILD && c.status == "A"));
    assert_eq!(restored.material_tree_oid, first.material_tree_oid);
    assert_eq!(f.read(&first, CHILD), f.child.content.as_bytes());
}

#[test]
fn trash_visibility_requires_a_fresh_ledger_and_hides_copied_managed_ids() {
    let mut f = Fixture::new();
    let first = f.commit(None);
    let mut stale = ContentStore::new(f.store.root().into(), f.dir.path().join("state")).unwrap();
    stale.refresh().unwrap();
    let preview = crate::trash::preview(&mut f.store, &f.child.id).unwrap();
    crate::trash::trash(
        &mut f.store,
        &f.child.id,
        &preview.version,
        "trash_visibility",
    )
    .unwrap();
    let error = FrozenMaterials::capture_project(&stale, "course_a").unwrap_err();
    assert_eq!(error.code, "TRASH_STATE_CHANGED");
    fs::write(
        f.root.join("materials/documents/copied.md"),
        &f.child.content,
    )
    .unwrap();
    fs::rename(
        f.store.root().join(&f.child.path),
        f.root.join("materials/documents/renamed.MD"),
    )
    .unwrap();
    for (stem, extension) in [("lower", "md"), ("mixed", "Md")] {
        fs::write(
            f.root
                .join(format!("materials/documents/renamed_{stem}.{extension}")),
            &f.child.content,
        )
        .unwrap();
    }
    f.store.refresh().unwrap();
    assert_eq!(f.store.read(&f.child.id).unwrap_err().code, "TRASHED");
    let hidden = f.commit(Some(&first.checkpoint_commit_oid));
    let files = f
        .git
        .files("course_a", &hidden.checkpoint_commit_oid)
        .unwrap();
    assert!(!files.iter().any(|file| file.path == CHILD
        || file.path == "documents/copied.md"
        || file.path.starts_with("documents/renamed")));
    assert_eq!(f.read(&first, CHILD), f.child.content.as_bytes());
}

#[path = "nested_course_git_tests.rs"]
mod nested;

#[test]
fn oversized_non_text_materials_do_not_block_valid_text_snapshot_and_focus() {
    let f = Fixture::new();
    let first = f.commit(None);
    let large_paths = ["archive.zip", "book.pdf", "video.mp4", "arbitrary.custom"];
    for path in large_paths {
        let mut file = fs::File::create(f.root.join("materials").join(path)).unwrap();
        file.write_all(&[0xff, 0, 0xfe]).unwrap();
        file.set_len(5 * 1024 * 1024 + 1).unwrap();
    }
    let html = b"<!doctype html><title>valid</title>";
    fs::write(f.root.join("materials/demo.html"), html).unwrap();
    fs::write(f.root.join("materials/notes.custom"), b"valid arbitrary suffix").unwrap();
    let next = f.commit(Some(&first.checkpoint_commit_oid));
    for path in large_paths {
        assert!(next.excluded_files.iter().any(|entry|
            entry.file_name == path && entry.reason == "capacity_exceeded" && entry.asset_id.is_empty()));
        assert!(f.git.read("course_a", &next.checkpoint_commit_oid, path).is_err());
        assert_eq!(fs::metadata(f.root.join("materials").join(path)).unwrap().len(), 5 * 1024 * 1024 + 1);
    }
    assert_eq!(f.read(&next, CHILD), f.child.content.as_bytes());
    assert_eq!(f.read(&next, "demo.html"), html);
    assert_eq!(f.read(&next, "notes.custom"), b"valid arbitrary suffix");
    let focused = f.diff(&first, &next, Some("demo.html"));
    assert!(focused.changes.iter().any(|change| change.path == "demo.html"));
}

#[test]
fn oversized_registered_attachment_keeps_source_id_in_exclusion() {
    let f = Fixture::new();
    let asset = f.import("large.txt", b"small original", "large_asset_source");
    let path = format!("assets/course_a/{}/large.txt", asset.id);
    let first = f.commit(None);
    assert_eq!(f.read(&first, &path), b"small original");
    let file = fs::OpenOptions::new().write(true)
        .open(f.root.join("materials").join(&path)).unwrap();
    file.set_len(5 * 1024 * 1024 + 1).unwrap();
    let next = f.commit(Some(&first.checkpoint_commit_oid));
    let excluded = next.excluded_files.iter().find(|entry| entry.file_name == path).unwrap();
    assert_eq!(excluded.reason, "capacity_exceeded");
    assert_eq!(excluded.asset_id, asset.id);
    assert!(f.git.read("course_a", &next.checkpoint_commit_oid, &path).is_err());
    assert_eq!(f.read(&next, CHILD), f.child.content.as_bytes());
    assert_eq!(f.read(&first, &path), b"small original");
}

#[test]
fn oversized_unregistered_paths_do_not_inherit_asset_source_id() {
    let f = Fixture::new();
    let asset = f.import("original.txt", b"registered", "large_unregistered_paths");
    let paths = [
        "assets/course_a/asset_00000000000000000000000000000000/large.txt".to_owned(),
        format!("assets/course_a/{}/extra.txt", asset.id),
    ];
    for path in &paths {
        let full = f.root.join("materials").join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::File::create(full).unwrap().set_len(5 * 1024 * 1024 + 1).unwrap();
    }
    let checkpoint = f.commit(None);
    for path in &paths {
        let excluded = checkpoint.excluded_files.iter().find(|entry| &entry.file_name == path).unwrap();
        assert_eq!(excluded.reason, "capacity_exceeded");
        assert!(excluded.asset_id.is_empty());
    }
    assert_eq!(f.read(&checkpoint, &format!("assets/course_a/{}/original.txt", asset.id)), b"registered");
}
