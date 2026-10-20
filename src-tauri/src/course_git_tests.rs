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
