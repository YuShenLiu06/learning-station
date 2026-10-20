//! Regressions for independent review round 1; all files and remotes are synthetic.
use super::*;

fn replace_document(path: &Path, bytes: &str) {
    let pending = path.with_extension("pending");
    fs::write(&pending, bytes).unwrap();
    fs::rename(pending, path).unwrap();
}

#[test]
fn atomic_reparent_between_resolution_and_storage_keeps_the_resolved_bytes() {
    // Deterministic schedule, with both repeated and one-time atomic replacement.
    for repeat in [true, false] {
        let f = Fixture::new();
        let path = f.store.root().join(&f.child.path);
        let mut metadata = f.child.metadata.clone();
        metadata
            .extra
            .insert("parentId".into(), serde_json::json!(f.other.id));
        let foreign = crate::content::serialize(&metadata, "B_ONLY_PRIVATE_BODY\n").unwrap();
        let mut switched = false;
        let result = FrozenMaterials::capture_with(&f.store, &f.course.id, |id| {
            if id == f.child.id && (repeat || !switched) {
                replace_document(&path, &f.child.content);
                let chain = library_tree::ancestry(&f.store, id)?;
                // The old implementation separately read B here after resolving A.
                replace_document(&path, &foreign);
                switched = true;
                return Ok(chain);
            }
            library_tree::ancestry(&f.store, id)
        });
        if repeat {
            let frozen = result.unwrap();
            let cp = f.git.commit(&frozen, None).unwrap();
            assert_eq!(f.read(&cp, &f.child_path()), f.child.content.as_bytes());
        } else {
            assert_eq!(result.unwrap_err().code, "CONFLICT");
        }
    }
}

#[test]
fn ancestry_version_changes_within_a_capture_are_conflicts() {
    let f = Fixture::new();
    let path = f.store.root().join(&f.course.path);
    let changed = crate::content::serialize(&f.course.metadata, "changed ancestor\n").unwrap();
    let result = FrozenMaterials::capture_with(&f.store, &f.course.id, |id| {
        let chain = library_tree::ancestry(&f.store, id)?;
        if id == f.course.id {
            replace_document(&path, &changed);
        }
        Ok(chain)
    });
    assert_eq!(result.unwrap_err().code, "CONFLICT");
}

#[test]
fn missing_oid_never_invokes_a_configured_promisor_transport() {
    let f = Fixture::new();
    let marker = f.dir.path().join("transport-executed");
    for (key, value) in [
        ("remote.synthetic.promisor", "true".to_owned()),
        (
            "remote.synthetic.url",
            format!("ext::/usr/bin/touch {}", marker.display()),
        ),
        ("protocol.ext.allow", "always".to_owned()),
    ] {
        f.git.git(&["config", key, &value], &[]).unwrap();
    }
    let first = f.commit(None);
    assert_eq!(
        f.git
            .checkpoint(&f.course.id, &"1".repeat(40))
            .unwrap_err()
            .code,
        "MATERIAL_GIT"
    );
    assert!(!marker.exists());
    assert_eq!(
        f.git
            .checkpoint(&f.course.id, &first.checkpoint_commit_oid)
            .unwrap(),
        first
    );
}

#[test]
fn teaching_names_survive_and_exclusions_are_readable_from_the_commit() {
    let f = Fixture::new();
    let original = f.commit(None);
    for (name, request) in [
        ("tokenization.txt", "nlp_lesson"),
        ("author-notes.md", "author_notes"),
    ] {
        let asset = f.import(name, b"ordinary teaching material\n", request);
        let cp = f.commit(Some(&original.checkpoint_commit_oid));
        let path = attachment_path(&asset.id, name).unwrap();
        assert_eq!(f.read(&cp, &path), b"ordinary teaching material\n");
        assert!(cp.excluded_files.is_empty());
    }
    let before = f.commit(None);
    let secret = f.import("credentials-local.json", b"{}", "excluded_secret");
    let config = f.import("provider-config.json", b"{}", "excluded_config");
    let excluded = f.commit(Some(&before.checkpoint_commit_oid));
    assert_eq!(before.material_tree_oid, excluded.material_tree_oid);
    assert_eq!(excluded.excluded_files.len(), 2);
    for asset in [secret, config] {
        assert!(excluded
            .excluded_files
            .iter()
            .any(|file| file.asset_id == asset.id
                && file.file_name == asset.file_name
                && file.reason == "sensitive_file_name"));
        assert!(f
            .git
            .read(
                &f.course.id,
                &excluded.checkpoint_commit_oid,
                &attachment_path(&asset.id, &asset.file_name).unwrap()
            )
            .is_err());
    }
    let reopened = MaterialGit::open(&f.git.directory).unwrap();
    assert_eq!(
        reopened
            .checkpoint(&f.course.id, &excluded.checkpoint_commit_oid)
            .unwrap(),
        excluded
    );
    assert!(f
        .git
        .checkpoint(&f.course.id, &before.checkpoint_commit_oid)
        .unwrap()
        .excluded_files
        .is_empty());
}

#[test]
fn existing_colon_and_percent_names_have_distinct_readable_safe_paths() {
    let f = Fixture::new();
    let first = f.commit(None);
    let colon = f.import("lesson:one.txt", b"colon name\n", "colon_lesson");
    let percent = f.import("lesson%3Aone.txt", b"literal percent\n", "percent_lesson");
    let cp = f.commit(Some(&first.checkpoint_commit_oid));
    let colon_path = attachment_path(&colon.id, &colon.file_name).unwrap();
    let percent_path = attachment_path(&percent.id, &percent.file_name).unwrap();
    assert!(colon_path.ends_with("/lesson%3Aone.txt"));
    assert!(percent_path.ends_with("/lesson%253Aone.txt"));
    assert_ne!(
        attachment_path("same_id", "lesson:one.txt").unwrap(),
        attachment_path("same_id", "lesson%3Aone.txt").unwrap()
    );
    let files = f
        .git
        .files(&f.course.id, &cp.checkpoint_commit_oid)
        .unwrap();
    assert!(files.iter().any(|file| file.path == colon_path));
    assert_eq!(f.read(&cp, &colon_path), b"colon name\n");
    assert_eq!(f.read(&cp, &percent_path), b"literal percent\n");
    let diff = f.diff(Some(&first), &cp, Some(&colon_path));
    assert!(diff
        .changes
        .iter()
        .find(|change| change.path == colon_path)
        .unwrap()
        .patch
        .as_ref()
        .unwrap()
        .contains("+colon name"));
}

#[test]
fn valid_jpeg_non_utf8_patch_is_complete_and_preserves_other_changes() {
    let f = Fixture::new();
    let first = f.commit(None);
    // Independently decoded 2x2 JPEG with a valid COM segment; not a screenshot.
    let jpeg = include_bytes!("../../tests/fixtures/material-no-nul-prefix.jpg");
    assert!(!jpeg[..8192].contains(&0));
    let image = f.import("diagram.jpg", jpeg, "jpeg_lesson");
    let text = f.import("notes.txt", b"keep this change\n", "jpeg_other_text");
    let cp = f.commit(Some(&first.checkpoint_commit_oid));
    let path = attachment_path(&image.id, &image.file_name).unwrap();
    let diff = f.diff(Some(&first), &cp, Some(&path));
    let change = diff
        .changes
        .iter()
        .find(|change| change.path == path)
        .unwrap();
    assert!(!change.binary && change.patch.is_none());
    let raw = change.patch_bytes.as_ref().unwrap();
    assert!(std::str::from_utf8(raw).is_err());
    let expected = f
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
                &cp.material_tree_oid,
                "--",
                &path,
            ],
            &[],
        )
        .unwrap();
    assert_eq!(*raw, expected);
    let text_path = attachment_path(&text.id, &text.file_name).unwrap();
    assert!(diff.changes.iter().any(|change| change.path == text_path
        && change.status == "A"
        && change.patch.is_none()
        && change.patch_bytes.is_none()));
    assert_eq!(f.read(&cp, &path), jpeg);
}
