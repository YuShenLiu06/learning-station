use crate::{
    assets,
    content::{self, ContentStore},
    course_project::{course_project_root, create_material},
    library_tree,
    types::{Document, Metadata, Result},
};
use std::{collections::BTreeMap, fs, path::Path};
use tempfile::TempDir;

fn metadata(id: &str, kind: &str, parent: Option<&str>) -> Metadata {
    let mut extra = BTreeMap::new();
    if let Some(parent) = parent {
        extra.insert("parentId".into(), serde_json::json!(parent));
    }
    Metadata {
        id: id.into(),
        title: format!("合成 {id}"),
        kind: kind.into(),
        goals: vec![],
        tags: vec![],
        links: vec![],
        version: 1,
        extra,
    }
}

fn fixture() -> (TempDir, ContentStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = ContentStore::new(dir.path().join("library"), dir.path().join("state")).unwrap();
    (dir, store)
}

fn course(store: &mut ContentStore, id: &str) -> Document {
    create_material(store, metadata(id, "course", None), &format!("课程 {id}\n")).unwrap()
}

fn document_path(course_id: &str, id: &str) -> String {
    format!("courses/{course_id}/materials/documents/{id}.md")
}

// These BUSY sites precede writes. Retry only the same request and payload;
// uncertain IO and all other errors must remain visible to the caller.
fn retry_busy<T>(mut operation: impl FnMut() -> Result<T>) -> Result<T> {
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

fn write_document(path: &Path, id: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        content::serialize(&metadata(id, "section", None), "不得进入材料索引\n").unwrap(),
    )
    .unwrap();
}

#[test]
fn course_project_roots_are_canonical_siblings_with_separate_materials() {
    let (_dir, mut store) = fixture();
    let a = course(&mut store, "course_a");
    let b = course(&mut store, "course_b");
    let a_root = course_project_root(store.root(), &a.id).unwrap();
    let b_root = course_project_root(store.root(), &b.id).unwrap();
    assert!(a_root.is_absolute());
    assert_eq!(
        a_root,
        fs::canonicalize(store.root().join("courses/course_a")).unwrap()
    );
    assert_eq!(
        b_root,
        fs::canonicalize(store.root().join("courses/course_b")).unwrap()
    );
    assert_eq!(a_root.parent(), b_root.parent());
    assert_ne!(a_root, b_root);
    assert_eq!(a.path, document_path(&a.id, &a.id));
    assert_eq!(b.path, document_path(&b.id, &b.id));
    assert_eq!(
        fs::read_to_string(a_root.join("materials/documents/course_a.md")).unwrap(),
        a.content
    );
    assert!(!a_root.join("materials/documents/course_b.md").exists());
    assert!(!b_root.join("materials/documents/course_a.md").exists());
    assert!(!store.root().join("courses/course_a.md").exists());
    assert!(!store.root().join("courses/course_b.md").exists());
}

#[test]
fn child_create_save_and_reopen_keep_one_physical_document() {
    let (dir, mut store) = fixture();
    course(&mut store, "course_a");
    let folder = store
        .create(
            metadata("folder_a", "folder", Some("course_a")),
            "目录\n",
            "courses/folder_a.md",
        )
        .unwrap();
    let child = store
        .create(
            metadata("child_a", "section", Some(&folder.id)),
            "初稿\n",
            "courses/child_a.md",
        )
        .unwrap();
    assert_eq!(folder.path, document_path("course_a", &folder.id));
    assert_eq!(child.path, document_path("course_a", &child.id));
    let changed = content::serialize(&child.metadata, "已保存\n").unwrap();
    let saved = store.save(&child.id, &changed, &child.version).unwrap();
    assert_eq!(saved.path, child.path);
    assert!(!store.root().join("courses/folder_a.md").exists());
    assert!(!store.root().join("courses/child_a.md").exists());
    let mut reopened =
        ContentStore::new(store.root().to_path_buf(), dir.path().join("state")).unwrap();
    reopened.refresh().unwrap();
    let actual = reopened.read(&child.id).unwrap();
    assert_eq!(actual.path, child.path);
    assert_eq!(actual.body, "已保存\n");
    assert_eq!(actual.version, saved.version);
    assert_eq!(
        library_tree::course_id(&reopened, &child.id).unwrap(),
        "course_a"
    );
    assert_eq!(
        reopened
            .summaries()
            .iter()
            .filter(|item| item.id == child.id)
            .count(),
        1
    );
}

#[test]
fn native_file_edit_is_immediately_read_and_stale_save_conflicts() {
    let (_dir, mut store) = fixture();
    let original = course(&mut store, "course_a");
    let native_content = content::serialize(&original.metadata, "原生工具直接编辑\n").unwrap();
    fs::write(store.root().join(&original.path), &native_content).unwrap();
    let current = store.read(&original.id).unwrap();
    assert_eq!(current.body, "原生工具直接编辑\n");
    assert_eq!(current.path, original.path);
    assert_eq!(current.version, content::hash(&native_content));
    assert_ne!(current.version, original.version);
    assert_eq!(
        store
            .save(&original.id, &original.content, &original.version)
            .unwrap_err()
            .code,
        "CONFLICT"
    );
    assert_eq!(
        fs::read_to_string(store.root().join(&original.path)).unwrap(),
        native_content
    );
}

#[test]
fn legacy_flat_and_other_paths_are_read_in_place_without_migration() {
    let (dir, mut store) = fixture();
    let old = store
        .create(
            metadata("legacy_course", "course", None),
            "旧课程\n",
            "courses/legacy_course.md",
        )
        .unwrap();
    let old_child = store
        .create(
            metadata("legacy_child", "section", Some(&old.id)),
            "旧材料\n",
            "archive/lesson.md",
        )
        .unwrap();
    let original_bytes = fs::read(store.root().join(&old.path)).unwrap();
    assert_eq!(
        course_project_root(store.root(), &old.id).unwrap_err().code,
        "COURSE_PROJECT_REQUIRED"
    );
    assert!(!store.root().join("courses/legacy_course").exists());
    let mut reopened =
        ContentStore::new(store.root().to_path_buf(), dir.path().join("state")).unwrap();
    reopened.refresh().unwrap();
    assert_eq!(reopened.read(&old.id).unwrap().path, old.path);
    assert_eq!(
        reopened.read(&old_child.id).unwrap().path,
        "archive/lesson.md"
    );
    assert_eq!(
        fs::read(store.root().join(&old.path)).unwrap(),
        original_bytes
    );
    assert!(!store.root().join(document_path(&old.id, &old.id)).exists());
    assert!(!store
        .root()
        .join(document_path(&old.id, &old_child.id))
        .exists());
}

#[test]
fn root_lookup_requires_existing_project_and_rejects_invalid_ids() {
    let (_dir, store) = fixture();
    assert_eq!(
        course_project_root(store.root(), "missing_course")
            .unwrap_err()
            .code,
        "COURSE_PROJECT_REQUIRED"
    );
    for id in ["", "..", "../escape", "/absolute", "a/b", "a\\b", "a:b"] {
        assert!(
            course_project_root(store.root(), id).is_err(),
            "accepted {id:?}"
        );
    }
    assert!(!store.root().join("courses").exists());
    fs::create_dir_all(store.root().join("courses/empty_course")).unwrap();
    assert_eq!(
        course_project_root(store.root(), "empty_course")
            .unwrap_err()
            .code,
        "COURSE_PROJECT_REQUIRED"
    );
}

#[cfg(unix)]
#[test]
fn root_lookup_rejects_symlinked_course_and_material_directories() {
    use std::os::unix::fs::symlink;
    let (dir, mut store) = fixture();
    course(&mut store, "course_a");
    let outside = dir.path().join("outside");
    fs::create_dir_all(outside.join("materials")).unwrap();
    symlink(&outside, store.root().join("courses/course_link")).unwrap();
    assert!(course_project_root(store.root(), "course_link").is_err());
    let materials = store.root().join("courses/course_a/materials");
    fs::rename(&materials, dir.path().join("original_materials")).unwrap();
    symlink(outside.join("materials"), &materials).unwrap();
    assert!(course_project_root(store.root(), "course_a").is_err());
    assert!(outside
        .join("materials")
        .read_dir()
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn project_assets_use_editable_original_name_and_preserve_import_receipt() {
    let (_dir, mut store) = fixture();
    course(&mut store, "course_a");
    course(&mut store, "course_b");
    let name = "原始%:资料.md";
    let imported = retry_busy(|| {
        assets::import(
            &store,
            "course_a",
            name,
            b"first\n",
            "project_asset_request",
        )
    })
    .unwrap();
    let directory = course_project_root(store.root(), "course_a")
        .unwrap()
        .join("materials/assets/course_a")
        .join(&imported.id);
    let file = directory.join("原始%25%3A资料.md");
    assert_eq!(fs::read(&file).unwrap(), b"first\n");
    let receipt_path = directory.join(".asset.json");
    let receipt = fs::read(&receipt_path).unwrap();
    assert!(!directory.join("metadata.json").exists());
    assert!(!directory.join("content.md").exists());
    assert!(!store.root().join("assets/course_a").exists());
    fs::write(&file, "外部编辑的新资料\n").unwrap();
    let actual = retry_busy(|| assets::read(&store, "course_a", &imported.id)).unwrap();
    assert_eq!(actual.text.as_deref(), Some("外部编辑的新资料\n"));
    assert_eq!(actual.asset.size, "外部编辑的新资料\n".len() as u64);
    assert_eq!(actual.asset.sha256, content::hash("外部编辑的新资料\n"));
    let retried = retry_busy(|| {
        assets::import(
            &store,
            "course_a",
            name,
            b"first\n",
            "project_asset_request",
        )
    })
    .unwrap();
    assert_eq!(retried, imported);
    assert_eq!(fs::read_to_string(&file).unwrap(), "外部编辑的新资料\n");
    assert_eq!(fs::read(&receipt_path).unwrap(), receipt);
    assert!(retry_busy(|| assets::read(&store, "course_b", &imported.id)).is_err());
    assert!(retry_busy(|| assets::list(&store, "course_b"))
        .unwrap()
        .is_empty());
    assert_eq!(
        retry_busy(|| assets::list(&store, "course_a"))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn legacy_assets_keep_original_layout_and_integrity_checks() {
    let (_dir, mut store) = fixture();
    store
        .create(
            metadata("legacy_course", "course", None),
            "旧课程\n",
            "courses/legacy_course.md",
        )
        .unwrap();
    let asset = retry_busy(|| {
        assets::import(
            &store,
            "legacy_course",
            "旧名.txt",
            b"original\n",
            "legacy_asset_request",
        )
    })
    .unwrap();
    let directory = store.root().join("assets/legacy_course").join(&asset.id);
    assert!(directory.join("metadata.json").is_file());
    assert!(!directory.join(".asset.json").exists());
    assert_eq!(
        retry_busy(|| assets::read(&store, "legacy_course", &asset.id))
            .unwrap()
            .text
            .as_deref(),
        Some("original\n")
    );
    fs::write(directory.join("content.txt"), b"tampered\n").unwrap();
    assert_eq!(
        retry_busy(|| assets::read(&store, "legacy_course", &asset.id))
            .unwrap_err()
            .code,
        "INVALID_ASSET"
    );
    assert!(retry_busy(|| assets::import(
        &store,
        "legacy_course",
        "旧名.txt",
        b"original\n",
        "legacy_asset_request"
    ))
    .is_err());
    assert_eq!(
        fs::read(directory.join("content.txt")).unwrap(),
        b"tampered\n"
    );
    assert!(!store
        .root()
        .join("courses/legacy_course/materials")
        .exists());
}

#[test]
fn refresh_indexes_material_documents_but_excludes_private_state_and_attachments() {
    let (_dir, mut store) = fixture();
    course(&mut store, "course_a");
    let root = course_project_root(store.root(), "course_a").unwrap();
    write_document(&root.join(".git/objects/forged.md"), "git_forged");
    write_document(&root.join("sessions/forged.md"), "session_forged");
    write_document(
        &root.join("materials/documents/.receipt.md"),
        "receipt_forged",
    );
    let attachment = content::serialize(
        &metadata("asset_forged", "section", None),
        "附件里的元数据\n",
    )
    .unwrap();
    retry_busy(|| {
        assets::import(
            &store,
            "course_a",
            "带元数据.md",
            attachment.as_bytes(),
            "index_asset_request",
        )
    })
    .unwrap();
    let summaries = store.refresh().unwrap();
    assert_eq!(
        summaries
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["course_a"]
    );
    let tree = library_tree::list(&store).unwrap();
    assert_eq!(tree.nodes.len(), 1);
    assert_eq!(tree.nodes[0].id, "course_a");
    for id in [
        "git_forged",
        "session_forged",
        "receipt_forged",
        "asset_forged",
    ] {
        assert_eq!(store.read(id).unwrap_err().code, "NOT_FOUND");
    }
}

#[test]
fn project_create_is_idempotent_and_cross_project_move_never_reparents() {
    let (_dir, mut store) = fixture();
    let a = library_tree::create_project(&mut store, "甲", "正文", "project_create_a").unwrap();
    let b = library_tree::create_project(&mut store, "乙", "正文", "project_create_b").unwrap();
    let again = library_tree::create_project(&mut store, "甲", "正文", "project_create_a").unwrap();
    assert_eq!(a.path, again.path);
    assert_eq!(a.version, again.version);
    let child = library_tree::create(
        &mut store,
        "section",
        "子",
        Some(&a.id),
        "",
        "project_child_a",
    )
    .unwrap();
    assert!(
        library_tree::move_node(&mut store, &child.id, Some(&b.id), 0, &child.version).is_err()
    );
    assert_eq!(library_tree::course_id(&store, &child.id).unwrap(), a.id);
    retry_busy(|| assets::import(&store, &a.id, "note.txt", b"a", "cross_project_request"))
        .unwrap();
    assert!(retry_busy(|| assets::import(
        &store,
        &b.id,
        "note.txt",
        b"a",
        "cross_project_request"
    ))
    .is_err());
}

fn special_attachment_name_is_supported(name: &str, request: &str) {
    let (_dir, mut store) = fixture();
    course(&mut store, "course_a");
    retry_busy(|| assets::import(&store, "course_a", "seed.txt", b"seed\n", "name_seed")).unwrap();
    let imported = retry_busy(|| assets::import(&store, "course_a", name, b"{}", request));
    let list = retry_busy(|| assets::list(&store, "course_a"));
    let next =
        retry_busy(|| assets::import(&store, "course_a", "next.txt", b"next\n", "name_next"));
    eprintln!(
        "name_bytes={}; import={:?}; list={:?}; next={:?}",
        name.len(),
        imported.as_ref().map(|_| ()).map_err(|e| &e.code),
        list.as_ref().map(|v| v.len()).map_err(|e| &e.code),
        next.as_ref().map(|_| ()).map_err(|e| &e.code)
    );
    let asset = imported.unwrap();
    assert_eq!(asset.file_name, name);
    assert_eq!(
        retry_busy(|| assets::read(&store, "course_a", &asset.id))
            .unwrap()
            .text
            .as_deref(),
        Some("{}")
    );
    assert_eq!(list.unwrap().len(), 2);
    next.unwrap();
    assert_eq!(
        retry_busy(|| assets::import(&store, "course_a", name, b"{}", request)).unwrap(),
        asset
    );
}

#[test]
fn receipt_named_attachment_does_not_poison_the_course() {
    special_attachment_name_is_supported(".asset.json", "receipt_name_request");
    special_attachment_name_is_supported(".ASSET.JSON", "receipt_case_request");
}

#[test]
fn percent_expansion_does_not_poison_the_course() {
    special_attachment_name_is_supported(&format!("{}.txt", "%".repeat(128)), "long_name_request");
}

#[test]
fn native_parent_edit_cannot_grant_a_sibling_course_identity() {
    let (_dir, mut store) = fixture();
    course(&mut store, "course_a");
    course(&mut store, "course_b");
    let folder = create_material(
        &mut store,
        metadata("folder_a", "folder", Some("course_a")),
        "folder\n",
    )
    .unwrap();
    let child = create_material(
        &mut store,
        metadata("child_a", "section", Some("course_a")),
        "keep bytes\n",
    )
    .unwrap();
    let moved = library_tree::move_node(&mut store, &child.id, Some(&folder.id), 0, &child.version)
        .unwrap();
    assert_eq!(moved.path, child.path);
    assert_eq!(
        library_tree::course_id(&store, &child.id).unwrap(),
        "course_a"
    );
    let mut external = moved.metadata.clone();
    external
        .extra
        .insert("parentId".into(), serde_json::json!("course_b"));
    let bytes = content::serialize(&external, &moved.body).unwrap();
    let path = store.root().join(&child.path);
    fs::write(&path, &bytes).unwrap();
    store.refresh().unwrap();
    assert!(library_tree::course_id(&store, &child.id).is_err());
    let tree = library_tree::list(&store).unwrap();
    assert_ne!(
        tree.nodes
            .iter()
            .find(|node| node.id == child.id)
            .unwrap()
            .course_id,
        "course_b"
    );
    assert_eq!(fs::read_to_string(path).unwrap(), bytes);
}

// Reviewer-only synthetic probes, appended solely in the isolated copy.
#[test]
fn review51_legacy_child_cannot_move_into_project_without_a_physical_move() {
    let (_dir, mut store) = fixture();
    let legacy = store
        .create(
            metadata("legacy", "course", None),
            "legacy\n",
            "courses/legacy.md",
        )
        .unwrap();
    let old_child = store
        .create(
            metadata("legacy_child", "section", Some(&legacy.id)),
            "old bytes\n",
            "courses/legacy_child.md",
        )
        .unwrap();
    let project = course(&mut store, "project_b");
    let moved = library_tree::move_node(
        &mut store,
        &old_child.id,
        Some(&project.id),
        0,
        &old_child.version,
    );
    if let Ok(ref document) = moved {
        eprintln!(
            "legacy_move_success=true; logical_course={}; actual_path={}; project_root={}",
            library_tree::course_id(&store, &document.id).unwrap(),
            document.path,
            course_project_root(store.root(), &project.id)
                .unwrap()
                .display()
        );
    }
    assert!(
        moved.is_err(),
        "legacy-to-project reparent leaves material outside the new course's physical root"
    );
    assert_eq!(
        store.read(&old_child.id).unwrap().content,
        old_child.content
    );
    assert_eq!(
        store.read(&old_child.id).unwrap().version,
        old_child.version
    );
}

#[test]
fn review51_old_nested_courses_document_survives_refresh() {
    let (_dir, mut store) = fixture();
    let parent = store
        .create(
            metadata("legacy", "course", None),
            "legacy\n",
            "courses/legacy.md",
        )
        .unwrap();
    let child = store
        .create(
            metadata("old_nested", "section", Some(&parent.id)),
            "keep me\n",
            "courses/legacy_materials/old.md",
        )
        .unwrap();
    let original = fs::read(store.root().join(&child.path)).unwrap();
    store.refresh().unwrap();
    let read = store.read(&child.id);
    eprintln!(
        "old_nested_read_after_refresh={:?}; file_preserved={}",
        read.as_ref()
            .map(|d| d.path.as_str())
            .map_err(|e| e.code.as_str()),
        fs::read(store.root().join(&child.path)).unwrap() == original
    );
    assert!(
        read.is_ok(),
        "a previously accepted legacy path disappears from the document index"
    );
}

#[test]
fn r1close_legacy_anchor_basename_does_not_hide_siblings() {
    let (_dir, mut store) = fixture();
    let root = store
        .create(
            metadata("legacy", "course", None),
            "legacy\n",
            "courses/legacy.md",
        )
        .unwrap();
    let child = store
        .create(
            metadata("old_child", "section", Some(&root.id)),
            "keep\n",
            "courses/legacy_bucket/old.md",
        )
        .unwrap();
    let anchor = "courses/legacy_bucket/materials/documents/legacy_bucket.md";
    store
        .create(
            metadata("unrelated_anchor", "section", Some("legacy")),
            "ordinary legacy\n",
            anchor,
        )
        .unwrap();
    for invalid in [
        metadata("unrelated_anchor", "section", Some("legacy")),
        metadata("wrong_id", "course", None),
        metadata("legacy_bucket", "section", None),
        metadata("legacy_bucket", "course", Some("legacy")),
    ] {
        let bytes = content::serialize(&invalid, "ordinary legacy\n").unwrap();
        fs::write(store.root().join(anchor), &bytes).unwrap();
        store.refresh().unwrap();
        assert_eq!(store.read(&child.id).unwrap().content, child.content);
        assert_eq!(
            fs::read(store.root().join(&child.path)).unwrap(),
            child.content.as_bytes()
        );
        assert_eq!(
            fs::read_to_string(store.root().join(anchor)).unwrap(),
            bytes
        );
    }
}

#[test]
fn review51_legacy_materials_component_does_not_trigger_project_routing() {
    let (_dir, mut store) = fixture();
    let parent = store
        .create(
            metadata("legacy", "course", None),
            "legacy\n",
            "archive/semester/materials/old.md",
        )
        .unwrap();
    let child = store.create(
        metadata("new_child", "section", Some(&parent.id)),
        "new child\n",
        "courses/new_child.md",
    );
    eprintln!(
        "legacy_materials_parent_path={}; child_result={:?}",
        parent.path,
        child
            .as_ref()
            .map(|d| d.path.as_str())
            .map_err(|e| e.code.as_str())
    );
    assert!(child.is_ok(),"only the third path component was used to mistake a legacy location for a physical project");
}

#[test]
fn review51_save_cannot_reassign_a_project_document_to_a_sibling_course() {
    let (_dir, mut store) = fixture();
    course(&mut store, "course_a");
    course(&mut store, "course_b");
    let child = create_material(
        &mut store,
        metadata("child_a", "section", Some("course_a")),
        "a bytes\n",
    )
    .unwrap();
    let mut changed = child.metadata.clone();
    changed
        .extra
        .insert("parentId".into(), serde_json::json!("course_b"));
    let saved = store.save(
        &child.id,
        &content::serialize(&changed, &child.body).unwrap(),
        &child.version,
    );
    let accepted_cross_scope = saved
        .as_ref()
        .is_ok_and(|doc| library_tree::course_id(&store, &doc.id).unwrap() == "course_b");
    eprintln!(
        "save_success={}; course_id={:?}; physical_path={}",
        saved.is_ok(),
        library_tree::course_id(&store, &child.id),
        child.path
    );
    assert!(
        !accepted_cross_scope,
        "save bypasses the new physical-course move guard"
    );
    assert!(saved.is_err());
    assert_eq!(store.read(&child.id).unwrap().content, child.content);
    assert_eq!(store.read(&child.id).unwrap().version, child.version);
}

#[test]
fn review51_project_creation_does_not_report_legacy_path_success() {
    let (_dir, mut store) = fixture();
    let old = library_tree::create(
        &mut store,
        "course",
        "same",
        None,
        "body",
        "review_same_request",
    )
    .unwrap();
    let project = library_tree::create_project(&mut store, "same", "body", "review_same_request");
    eprintln!(
        "legacy_path={}; create_project_return={:?}",
        old.path,
        project
            .as_ref()
            .map(|d| d.path.as_str())
            .map_err(|e| e.code.as_str())
    );
    assert!(
        project.as_ref().is_err()
            || project
                .as_ref()
                .is_ok_and(|d| course_project_root(store.root(), &d.id).is_ok()),
        "create_project returns success while no project exists"
    );
}
