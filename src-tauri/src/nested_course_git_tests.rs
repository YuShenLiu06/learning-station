use super::*;
use crate::{app::Engine, focus, types::Entry};
use std::collections::BTreeMap;

fn tree_bytes(root: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for item in fs::read_dir(dir).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    result
}

// The desktop's default library lives inside its source checkout, including
// linked worktrees whose .git is a file. No real library or model is involved.
#[test]
fn nested_course_git_chat_handles_new_flat_and_partially_migrated_courses() {
    for linked_worktree in [false, true] {
        for layout in ["new", "flat", "migrated"] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source");
            fs::create_dir(&source).unwrap();
            git(&source, &["init", "--template=", "."], &[]);
            fs::write(source.join("tracked.txt"), b"source unchanged").unwrap();
            git(&source, &["add", "tracked.txt"], &[]);
            git(&source, &["commit", "-m", "synthetic source"], &[]);
            let checkout = if linked_worktree {
                let path = dir.path().join("linked");
                git(
                    &source,
                    &["worktree", "add", "--detach", path.to_str().unwrap()],
                    &[],
                );
                path
            } else {
                source.clone()
            };
            let source_before = tree_bytes(&source.join(".git"));
            let library = checkout.join("library");
            let state = dir.path().join("state");
            let engine = Engine::new(library.clone(), state.clone()).unwrap();
            let mut core = engine.lock().unwrap();
            let course = "nested_course";
            let note = "nested_note";
            if layout == "new" {
                create_material(&mut core.content, metadata(course, None), "课程").unwrap();
                create_material(&mut core.content, metadata(note, Some(course)), "原始笔记")
                    .unwrap();
            } else {
                core.content
                    .create(metadata(course, None), "课程", "courses/old-course.md")
                    .unwrap();
                core.content
                    .create(
                        metadata(note, Some(course)),
                        "原始笔记",
                        "courses/old-note.md",
                    )
                    .unwrap();
                if layout == "migrated" {
                    let root =
                        crate::course_project::ensure_project(&mut core.content, course).unwrap();
                    assert!(root.join(".migration-backup").is_dir());
                    assert!(!root.join(".git").exists());
                }
            }
            let original = core.content.read(note).unwrap().content;
            let session =
                crate::course_sessions::create(&mut core, course, "nested-chat-request").unwrap();
            if layout == "migrated" {
                // Ordinary chat without Focus reaches this same init boundary.
                let root = crate::native_session::project_root(&core.content, &session)
                    .unwrap()
                    .unwrap();
                assert!(root.join(".git/learning-materials-v2").is_file());
            } else if layout == "flat" {
                assert!(crate::native_session::project_root(&core.content, &session)
                    .unwrap()
                    .is_none());
            }
            let document = core.content.read(note).unwrap();
            let frozen = focus::freeze(
                &mut core,
                focus::FreezeRequest {
                    session_id: session.id.clone(),
                    input: focus::Input {
                        document_id: note.into(),
                        expected_body: document.body,
                        expected_version: Some(document.version),
                        view: "edit".into(),
                        ranges: vec![],
                        question_id: None,
                        asset_id: None,
                    },
                    selection: String::new(),
                    injections: vec![],
                    assistant_references: vec![],
                    exercise_references: vec![],
                },
            )
            .unwrap();
            let entry = Entry {
                id: String::new(),
                kind: "user".into(),
                content: "帮我整理当前笔记".into(),
                created_at: String::new(),
                selection: None,
                related_ids: vec![],
                data: Some(
                    serde_json::json!({"requestedContext":{"documentId":note,"selection":"","injections":[],"focus":frozen}}),
                ),
            };
            let saved = focus::append_entry(
                &mut core,
                &session.id,
                entry.clone(),
                Some(&session.version),
                "nested-question-request",
            )
            .unwrap();
            assert_eq!(saved.entries.len(), 1);
            let fixed = focus::read_document(&core, &frozen, note).unwrap();
            assert_eq!(fixed.content, original);
            let root = course_project_root(&library, course).unwrap();
            assert_eq!(
                git_text(&root, &["rev-parse", "--show-toplevel"], &[]),
                root.to_str().unwrap()
            );
            assert!(!root.join(".git/index").exists());
            assert_eq!(
                crate::native_session::project_root(&core.content, &session).unwrap(),
                Some(root)
            );
            drop(core);
            drop(engine);
            let reopened = Engine::new(library, state).unwrap();
            let mut core = reopened.lock().unwrap();
            let replay = focus::append_entry(
                &mut core,
                &session.id,
                entry,
                Some("stale"),
                "nested-question-request",
            )
            .unwrap();
            assert_eq!(replay.entries.len(), 1);
            let first = crate::versioning::snapshot(&mut core, course).unwrap();
            let second = crate::versioning::snapshot(&mut core, course).unwrap();
            assert_eq!(first.material_tree_oid, second.material_tree_oid);
            assert_eq!(core.content.read(note).unwrap().content, original);
            assert_eq!(
                tree_bytes(&source.join(".git")),
                source_before,
                "source HEAD, refs, index, config and objects changed: {linked_worktree}/{layout}"
            );
        }
    }
}

#[test]
fn nested_course_git_never_adopts_foreign_or_redirected_repositories() {
    for variant in [
        "foreign",
        "git-file",
        "wrong-marker",
        "alternates",
        "commondir",
        "symlink",
        "hardlink",
    ] {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--template=", "."], &[]);
        let source_before = tree_bytes(&dir.path().join(".git"));
        let mut store =
            ContentStore::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        create_material(&mut store, metadata("nested_guard", None), "course").unwrap();
        let root = course_project_root(store.root(), "nested_guard").unwrap();
        let git_dir = root.join(".git");
        match variant {
            "foreign" => {
                git(&root, &["init", "--template=", "."], &[]);
            }
            "git-file" => {
                fs::write(
                    &git_dir,
                    format!("gitdir: {}\n", dir.path().join(".git").display()),
                )
                .unwrap();
            }
            _ => {
                MaterialGit::init_course(store.root(), "nested_guard").unwrap();
            }
        }
        match variant {
            "wrong-marker" => {
                fs::write(git_dir.join("learning-materials-v2"), "another_course").unwrap()
            }
            "alternates" => fs::write(
                git_dir.join("objects/info/alternates"),
                dir.path().join(".git/objects").to_str().unwrap(),
            )
            .unwrap(),
            "commondir" => fs::write(
                git_dir.join("commondir"),
                dir.path().join(".git").to_str().unwrap(),
            )
            .unwrap(),
            #[cfg(unix)]
            "symlink" => {
                fs::remove_file(git_dir.join("config")).unwrap();
                std::os::unix::fs::symlink(dir.path().join(".git/config"), git_dir.join("config"))
                    .unwrap();
            }
            #[cfg(unix)]
            "hardlink" => {
                fs::remove_file(git_dir.join("config")).unwrap();
                fs::hard_link(dir.path().join(".git/config"), git_dir.join("config")).unwrap();
            }
            #[cfg(not(unix))]
            "symlink" | "hardlink" => continue,
            _ => {}
        }
        let before = tree_bytes(&root);
        assert!(
            MaterialGit::init_course(store.root(), "nested_guard").is_err(),
            "adopted {variant}"
        );
        assert_eq!(tree_bytes(&root), before, "modified {variant}");
        assert_eq!(
            tree_bytes(&dir.path().join(".git")),
            source_before,
            "modified source via {variant}"
        );
    }
}
