use super::*;
use crate::{
    assets,
    content::{self, ContentStore},
    library_tree,
    material_git::{Checkpoint, FrozenMaterials},
    types::Document,
};

struct Fixture {
    _dir: tempfile::TempDir,
    store: ContentStore,
    git: MaterialGit,
    course: Document,
    a: Document,
    b: Document,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut store =
            ContentStore::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let course = library_tree::create_project(&mut store, "Synthetic course", "Outline", "changes_course").unwrap();
        let a = library_tree::create(
            &mut store,
            "section",
            "A",
            Some(&course.id),
            &"stable\n".repeat(100),
            "changes_a",
        )
        .unwrap();
        let b = library_tree::create(
            &mut store,
            "section",
            "B",
            Some(&course.id),
            "B first\n",
            "changes_b",
        )
        .unwrap();
        let git = MaterialGit::init_course(store.root(), &course.id).unwrap();
        Self {
            _dir: dir,
            store,
            git,
            course,
            a,
            b,
        }
    }
    fn checkpoint(&self, parent: Option<&str>) -> Checkpoint {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let frozen = loop {
            match FrozenMaterials::capture_project(&self.store, &self.course.id) {
                Err(e) if e.code == "BUSY" && std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                r => break r.unwrap(),
            }
        };
        self.git.commit(&frozen, parent).unwrap()
    }
    fn project(
        &self,
        new: &Checkpoint,
        old: Option<&Checkpoint>,
        focus: Option<(&str, &str)>,
        budget: usize,
    ) -> Result<Value> {
        prepare(
            &self.git,
            &self.course.id,
            &new.checkpoint_commit_oid,
            old.map(|c| c.checkpoint_commit_oid.as_str()),
            old.map(|c| c.checkpoint_commit_oid.as_str()),
            focus,
            budget,
        )
    }
    fn asset_path(&self, checkpoint: &Checkpoint, id: &str) -> String {
        self.git.files(&self.course.id, &checkpoint.checkpoint_commit_oid).unwrap().into_iter().find(|file| file.path.split('/').any(|part| part == id)).unwrap().path
    }
    fn save(&mut self, id: &str, body: &str) {
        let doc = self.store.read(id).unwrap();
        self.store
            .save(
                id,
                &content::serialize(&doc.metadata, body).unwrap(),
                &doc.version,
            )
            .unwrap();
    }
}
const BUDGET: usize = 512 * 1024;

#[test]
fn initial_summary_is_not_a_body_and_switching_focus_reads_the_fixed_blob() {
    let mut f = Fixture::new();
    let a_path = format!("documents/{}.md", f.a.id);
    let b_path = format!("documents/{}.md", f.b.id);
    let first = f.checkpoint(None);
    let value = f
        .project(&first, None, Some((&f.a.id, &a_path)), BUDGET)
        .unwrap();
    let b = value["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["path"] == b_path)
        .unwrap();
    assert!(b["patch"].is_null());
    assert!(!serde_json::to_string(b).unwrap().contains("B first"));
    let id = f.b.id.clone();
    f.save(&id, "B +111\n");
    let second = f.checkpoint(Some(&first.checkpoint_commit_oid));
    let value = f
        .project(&second, Some(&first), Some((&f.a.id, &a_path)), BUDGET)
        .unwrap();
    assert_eq!(value["changes"][0]["status"], "M");
    assert!(value["changes"][0]["patch"].is_null());
    f.save(&id, "latest worktree must not leak\n");
    let focused = f
        .project(&second, Some(&second), Some((&f.b.id, &b_path)), BUDGET)
        .unwrap();
    assert_eq!(focused["changes"], json!([]));
    assert!(focused["focused"]["text"]
        .as_str()
        .unwrap()
        .contains("B +111"));
    assert!(!focused.to_string().contains("latest worktree"));
    assert_eq!(focused["focused"]["range"]["complete"], true);
}

#[test]
fn native_patch_is_complete_and_budget_failure_does_not_silently_clip() {
    let mut f = Fixture::new();
    let path = format!("documents/{}.md", f.a.id);
    let first = f.checkpoint(None);
    let id = f.a.id.clone();
    f.save(&id, &"changed 🧪\n".repeat(17000));
    let second = f.checkpoint(Some(&first.checkpoint_commit_oid));
    let native = String::from_utf8(f.git.patch(&f.course.id, &first.checkpoint_commit_oid, &second.checkpoint_commit_oid, &path).unwrap()).unwrap();
    let value = f
        .project(&second, Some(&first), Some((&f.a.id, &path)), BUDGET)
        .unwrap();
    assert_eq!(value["focused"]["gitChange"]["unifiedDiff"], json!(native));
    assert!(value["changes"][0].get("patch").is_none());
    assert!(value["focused"]["gitChange"]["unifiedDiff"]
        .as_str()
        .unwrap()
        .contains("+changed 🧪"));
    assert_eq!(
        f.project(&second, Some(&first), Some((&f.a.id, &path)), 1000)
            .unwrap_err()
            .code,
        "LIMIT_EXCEEDED"
    );
    assert_eq!(
        f.project(&second, Some(&first), Some((&f.a.id, &path)), BUDGET)
            .unwrap(),
        value
    );
}

#[test]
fn attachments_exclusions_binary_and_non_utf8_patch_are_auditable_without_lossy_text() {
    let f = Fixture::new();
    let first = f.checkpoint(None);
    let jpeg = include_bytes!("../../tests/fixtures/material-no-nul-prefix.jpg");
    let image = assets::import(
        &f.store,
        &f.course.id,
        "diagram:1.jpg",
        jpeg,
        "changes_jpeg",
    )
    .unwrap();
    let text = assets::import(
        &f.store,
        &f.course.id,
        "notes.txt",
        b"new attachment\n",
        "changes_text",
    )
    .unwrap();
    let excluded = assets::import(
        &f.store,
        &f.course.id,
        "credentials.json",
        b"synthetic only",
        "changes_excluded",
    )
    .unwrap();
    let second = f.checkpoint(Some(&first.checkpoint_commit_oid));
    let path = f.asset_path(&second, &image.id);
    let value = f
        .project(&second, Some(&first), Some((&image.id, &path)), BUDGET)
        .unwrap();
    let change = value["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["path"] == path)
        .unwrap();
    assert_eq!(change["patchUnavailable"], "non_utf8");
    assert!(change["patchByteLength"].as_u64().unwrap() > jpeg.len() as u64);
    assert!(change.get("patchBytes").is_none());
    assert_eq!(value["focused"]["textUnavailable"], "non_utf8");
    assert_eq!(
        f.git
            .read(&f.course.id, &second.checkpoint_commit_oid, &path)
            .unwrap(),
        jpeg
    );
    assert!(value["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["path"] == f.asset_path(&second, &text.id)));
    let excluded_file = value["checkpoint"]["excludedFiles"].as_array().unwrap().iter()
        .find(|item| item["assetId"] == excluded.id && item["fileName"].as_str().unwrap().ends_with("credentials.json")).unwrap();
    assert_eq!(excluded_file["reason"], "sensitive_file_name");
    assert!(value["checkpoint"]["excludedFiles"].as_array().unwrap().iter().any(|item| item["assetId"] == ""));
    assert!(!value.to_string().contains('�'));
    assert_eq!(
        f.project(&second, Some(&second), None, BUDGET).unwrap()["exclusionsChanged"],
        false
    );
}

#[test]
fn deletion_binary_and_cross_course_fail_closed() {
    let mut f = Fixture::new();
    let first = f.checkpoint(None);
    let id = f.a.id.clone();
    f.save(&id, "binary\0payload");
    let second = f.checkpoint(Some(&first.checkpoint_commit_oid));
    let path = format!("documents/{}.md", f.a.id);
    let value = f
        .project(&second, Some(&first), Some((&f.a.id, &path)), BUDGET)
        .unwrap();
    assert_eq!(value["changes"][0]["binary"], true);
    assert!(value["changes"][0]["patch"].is_null());
    assert_eq!(value["focused"]["textUnavailable"], "binary");
    std::fs::remove_file(f.store.root().join(&f.b.path)).unwrap();
    f.store.refresh().unwrap();
    let third = f.checkpoint(Some(&second.checkpoint_commit_oid));
    let value = f.project(&third, Some(&second), None, BUDGET).unwrap();
    assert_eq!(value["changes"][0]["status"], "D");
    assert_eq!(
        value["changes"][0]["fixedVersion"]["checkpointCommitOid"],
        second.checkpoint_commit_oid
    );
    assert!(prepare(
        &f.git,
        "other_course",
        &third.checkpoint_commit_oid,
        None,
        None,
        None,
        BUDGET
    )
    .is_err());
    assert!(prepare(&f.git, &f.course.id, "HEAD", None, None, None, BUDGET).is_err());
}

#[test]
fn focused_text_assets_keep_real_identity_content_version_and_complete_fixed_bytes() {
    let f = Fixture::new();
    let bytes = "同一附件正文😀\r\n".as_bytes();
    let a = assets::import(
        &f.store,
        &f.course.id,
        "first:lesson.txt",
        bytes,
        "changes_asset_a",
    )
    .unwrap();
    let b = assets::import(
        &f.store,
        &f.course.id,
        "second.txt",
        bytes,
        "changes_asset_b",
    )
    .unwrap();
    assert_ne!(a.id, b.id);
    let cp = f.checkpoint(None);
    for asset in [a, b] {
        let path = f.asset_path(&cp, &asset.id);
        let value = f
            .project(&cp, None, Some((&asset.id, &path)), BUDGET)
            .unwrap();
        let source = &value["focused"];
        assert_eq!(source["id"], asset.id);
        assert_eq!(source["path"], path);
        assert_eq!(source["modelPath"], format!("materials/{path}"));
        assert!(source["gitChange"].is_null());
        assert_eq!(source["version"], asset.sha256);
        assert_eq!(source["textHash"], source["version"]);
        assert_eq!(source["text"].as_str().unwrap().as_bytes(), bytes);
        assert_eq!(source["range"]["complete"], true);
        assert_eq!(source["range"]["totalBytes"], bytes.len());
        assert_ne!(source["gitBlobOid"], source["version"]);
        assert_eq!(source["gitCommitOid"], cp.checkpoint_commit_oid);
    }
}

#[test]
fn focused_text_asset_patch_keeps_repository_prefix_and_frozen_bytes() {
    let f = Fixture::new();
    let asset = assets::import(&f.store, &f.course.id, "notes:lesson.txt", b"before text\n", "asset-native-patch").unwrap();
    let first = f.checkpoint(None);
    let path = f.asset_path(&first, &asset.id);
    let root = crate::course_project::course_project_root(f.store.root(), &f.course.id).unwrap();
    std::fs::write(root.join("materials").join(&path), b"after text\n").unwrap();
    let second = f.checkpoint(Some(&first.checkpoint_commit_oid));
    std::fs::write(root.join("materials").join(&path), b"unfrozen text\n").unwrap();
    let value = f.project(&second, Some(&first), Some((&asset.id, &path)), BUDGET).unwrap();
    let model_path = format!("materials/{path}");
    assert_eq!(value["focused"]["modelPath"], model_path);
    assert_eq!(value["focused"]["text"], "after text\n");
    let patch = value["focused"]["gitChange"]["unifiedDiff"].as_str().unwrap();
    assert!(patch.starts_with(&format!("diff --git a/{model_path} b/{model_path}\n")));
    assert!(patch.contains(&format!("+++ b/{model_path}\n")));
    assert!(patch.contains("-before text\n+after text\n"));
    assert!(!value.to_string().contains("unfrozen text"));
}

#[test]
fn course_notification_old_and_focus_body_old_compare_independently() {
    let mut f = Fixture::new();
    let path = format!("documents/{}.md", f.a.id);
    let first = f.checkpoint(None);
    let id = f.a.id.clone();
    f.save(&id, "A at intermediate checkpoint\n");
    let second = f.checkpoint(Some(&first.checkpoint_commit_oid));
    f.save(&id, "A final fixed version\n");
    let third = f.checkpoint(Some(&second.checkpoint_commit_oid));
    let value = prepare(&f.git, &f.course.id, &third.checkpoint_commit_oid, Some(&second.checkpoint_commit_oid), Some(&first.checkpoint_commit_oid), Some((&f.a.id, &path)), BUDGET).unwrap();
    let native = String::from_utf8(f.git.patch(&f.course.id, &first.checkpoint_commit_oid, &third.checkpoint_commit_oid, &path).unwrap()).unwrap();
    assert_eq!(value["baseCommitOid"], second.checkpoint_commit_oid);
    assert_eq!(value["stat"], f.git.stat(&f.course.id, Some(&second.checkpoint_commit_oid), &third.checkpoint_commit_oid).unwrap());
    assert_eq!(value["focused"]["gitChange"]["baseCommitOid"], first.checkpoint_commit_oid);
    assert_eq!(value["changes"][0]["patchBaseCommitOid"], first.checkpoint_commit_oid);
    assert_eq!(value["focused"]["gitChange"]["unifiedDiff"], json!(native));
    assert!(value["changes"][0].get("patch").is_none());
    let missing = prepare(&f.git, &f.course.id, &third.checkpoint_commit_oid, Some(&second.checkpoint_commit_oid), None, Some((&f.a.id, &path)), BUDGET).unwrap();
    assert!(missing["focused"]["gitChange"].is_null());
    assert!(missing["changes"][0]["patch"].is_null());
    assert!(missing["focused"]["text"].as_str().unwrap().contains("A final fixed version"));
}

#[test]
fn focus_paths_and_body_patch_use_frozen_root_paths_for_text_and_metadata_only_views() {
    use crate::{app::Engine, focus::{FreezeRequest, Input}};
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
    let mut core = engine.lock().unwrap();
    let course = library_tree::create_project(&mut core.content, "C", "", "path-course").unwrap();
    let note = library_tree::create(&mut core.content, "section", "A", Some(&course.id), "old body\n", "path-note").unwrap();
    let sid = core.start_session(&note.id, false).unwrap().0.id;
    let request = |core: &crate::app::Core, id: &str, view: &str, asset: Option<String>| {
        let doc = core.content.read(id).unwrap();
        FreezeRequest {
            session_id: sid.clone(),
            input: Input { document_id: id.into(), expected_body: doc.body, expected_version: Some(doc.version), view: view.into(), ranges: vec![], question_id: None, asset_id: asset },
            selection: String::new(), injections: vec![], assistant_references: vec![], exercise_references: vec![],
        }
    };
    let r = request(&core, &note.id, "read", None);
    let first = crate::focus::freeze(&mut core, r).unwrap();
    let session = crate::sessions::get(&mut core.content, &sid).unwrap();
    let initial = for_focus(&mut core, &session, &first, None, None).unwrap();
    let path = format!("materials/documents/{}.md", note.id);
    assert_eq!(initial["focusPath"], path);
    assert_eq!(initial["focused"]["modelPath"], path);
    assert_eq!(initial["focused"]["text"], "old body\n");
    assert!(initial["focused"]["gitChange"].is_null());
    // Git may abbreviate the generated long ID in its native default stat width.
    assert!(!initial["stat"].as_str().unwrap().is_empty());
    core.content.save(&note.id, &content::serialize(&note.metadata, "new body\n").unwrap(), &note.version).unwrap();
    let r = request(&core, &note.id, "read", None);
    let second = crate::focus::freeze(&mut core, r).unwrap();
    let doc = core.content.read(&note.id).unwrap();
    core.content.save(&note.id, &content::serialize(&doc.metadata, "late workspace\n").unwrap(), &doc.version).unwrap();
    let value = for_focus(&mut core, &session, &second, Some(&first.commit_hash), Some(&first.commit_hash)).unwrap();
    assert_eq!(value["focused"]["text"], "new body\n");
    let patch = value["focused"]["gitChange"]["unifiedDiff"].as_str().unwrap();
    assert!(patch.starts_with(&format!("diff --git a/{path} b/{path}\n")));
    assert!(patch.contains("-old body\n+new body\n"));
    assert!(!patch.contains("createRequestDigest") && !patch.contains("parentId") && !value.to_string().contains("late workspace"));
    let unchanged = for_focus(&mut core, &session, &second, Some(&second.commit_hash), Some(&second.commit_hash)).unwrap();
    assert_eq!(unchanged["stat"], "");
    assert!(unchanged["focused"]["gitChange"].is_null());
    assert_eq!(unchanged["focused"]["text"], "new body\n");
    let exercise = crate::exercises::create_manual(&mut core.content, &sid, &note.id, crate::exercises::ManualExercise {
        question: "Q?".into(), format: "exact".into(), answers: vec!["PRIVATE_KEY_BODY".into()], explanation: "PRIVATE_REASON_BODY".into(),
    }, "path-exercise").unwrap();
    let r = request(&core, &exercise.document.id, "exercise", None);
    let focus = crate::focus::freeze(&mut core, r).unwrap();
    let value = for_focus(&mut core, &session, &focus, Some(&second.commit_hash), None).unwrap();
    assert_eq!(value["focusPath"], format!("materials/documents/{}.md", exercise.document.id));
    assert!(value["focused"].is_null());
    assert!(!value.to_string().contains("PRIVATE_KEY_BODY") && !value.to_string().contains("PRIVATE_REASON_BODY"));
    let image = crate::material_git::retry_busy(|| assets::import(&core.content, &course.id, "picture.png", b"\x89PNG\r\n\x1a\n\0image", "path-image")).unwrap();
    let r = request(&core, &note.id, "asset", Some(image.id));
    let focus = crate::focus::freeze(&mut core, r).unwrap();
    assert_eq!(focus.r#type, "image");
    let value = for_focus(&mut core, &session, &focus, Some(&second.commit_hash), None).unwrap();
    assert_eq!(value["focusPath"], format!("materials/{}", focus.path));
    assert!(value["focused"].is_null());
    let doc = core.content.read(&note.id).unwrap();
    core.content.save(&note.id, &content::serialize(&doc.metadata, "binary\0body").unwrap(), &doc.version).unwrap();
    let r = request(&core, &note.id, "read", None);
    let binary = crate::focus::freeze(&mut core, r).unwrap();
    assert!(binary.binary);
    let value = for_focus(&mut core, &session, &binary, None, None).unwrap();
    assert_eq!(value["focusPath"], path);
    assert!(value["focused"].is_null());
}

#[test]
fn git_binary_classification_is_authoritative_even_when_utf8_contains_no_nul() {
    let f = Fixture::new();
    let path = format!("documents/{}.md", f.a.id);
    let root = crate::course_project::course_project_root(f.store.root(), &f.course.id).unwrap();
    std::fs::create_dir_all(root.join(".git/info")).unwrap();
    std::fs::write(root.join(".git/info/attributes"), format!("{path} -diff\n")).unwrap();
    let checkpoint = f.checkpoint(None);
    let native = f.git.compare(&f.course.id, None, &checkpoint.checkpoint_commit_oid, None).unwrap();
    assert!(native.changes.iter().find(|c| c.path == path).unwrap().binary);
    let value = f.project(&checkpoint, None, Some((&f.a.id, &path)), BUDGET).unwrap();
    assert_eq!(value["focused"]["textUnavailable"], "binary");
    assert!(value["focused"]["text"].is_null());
}
