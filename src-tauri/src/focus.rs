//! One request's view and authorized materials. Git identifies bytes, never Pi acceptance.
use crate::{app::{atomic_json, Core}, automatic_context::MaterialSnapshot, content, context, material_git::{FrozenMaterials, MaterialGit}, types::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Focus {
    pub id: String,
    pub course_id: String,
    pub document_id: String,
    pub source_document_id: String,
    pub path: String,
    pub r#type: String,
    pub commit_hash: String,
    pub material_tree_oid: String,
    pub excluded_files: Vec<crate::material_git::ExcludedFile>,
    pub document_version: String,
    pub view: String,
    pub paragraphs: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exercise: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset: Option<Value>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    pub document_id: String,
    pub expected_body: String,
    pub expected_version: Option<String>,
    pub view: String,
    pub ranges: Vec<Range>,
    pub question_id: Option<String>,
    pub asset_id: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range { pub start: usize, pub end: usize }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FreezeRequest {
    pub session_id: String,
    pub input: Input,
    pub selection: String,
    pub injections: Vec<context::ContextInjection>,
    pub assistant_references: Vec<context::AssistantReference>,
    pub exercise_references: Vec<crate::exercise_references::ExerciseReference>,
}
#[derive(Serialize, Deserialize)]
pub struct Frozen {
    pub focus: Focus,
    pub context: Context,
    materials: Vec<MaterialSnapshot>,
    assets: BTreeMap<String, String>,
    requested: context::RequestedContext,
}
fn conflict() -> CoreError { CoreError::new("CONTEXT_CONFLICT", "聚焦材料或冻结快照不一致，原问题保留；请重新核对。") }
fn directory(core: &Core) -> Result<std::path::PathBuf> {
    let path = core.state_dir.join("focus");
    match fs::create_dir(&path) { Ok(()) => {}, Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}, Err(e) => return Err(e.into()) }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() { return Err(conflict()); }
    Ok(path)
}
pub fn open_materials(core: &Core) -> Result<MaterialGit> { MaterialGit::open(&core.state_dir.join("material-git")) }
fn repository(core: &Core) -> Result<MaterialGit> {
    let path = core.state_dir.join("material-git");
    match fs::symlink_metadata(&path) {
        Ok(_) => MaterialGit::open(&path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => MaterialGit::create_new(&path),
        Err(e) => Err(CoreError::io(e)),
    }
}
fn paragraphs(body: &str, ranges: &[Range]) -> Result<Vec<Value>> {
    if ranges.len() > 16 { return Err(conflict()); }
    let offset = |wanted| { let mut n = 0; for (i, c) in body.char_indices() { if n == wanted { return Some(i); } n += c.len_utf16(); } (n == wanted).then_some(body.len()) };
    let mut chars = 0;
    let mut previous = 0;
    ranges.iter().map(|r| {
        if r.start < previous || r.end < r.start { return Err(conflict()); }
        let text = body.get(offset(r.start).ok_or_else(conflict)?..offset(r.end).ok_or_else(conflict)?).ok_or_else(conflict)?;
        chars += text.chars().count(); previous = r.end;
        if chars > 8000 { return Err(conflict()); }
        Ok(json!({"start":r.start,"end":r.end,"text":text}))
    }).collect()
}
pub fn freeze(core: &mut Core, request: FreezeRequest) -> Result<Focus> {
    let i = &request.input;
    if !["read", "edit", "exercise", "asset"].contains(&i.view.as_str()) { return Err(conflict()); }
    core.content.refresh()?;
    let (session, mut ctx) = core.session_context_for(&request.session_id, Some(&i.document_id), Some(&request.selection))?;
    let document = core.content.read(&i.document_id)?;
    if document.body != i.expected_body || i.expected_version.as_ref().is_some_and(|v| v != &document.version) { return Err(conflict()); }
    let course = crate::course_sessions::course(&core.content, &i.document_id)?.ok_or_else(conflict)?;
    crate::exercise_references::inject_request(&mut core.content, &mut ctx, &session, &request.injections, &request.assistant_references, &request.exercise_references)?;
    ctx.history_manifest.clear();
    let frozen = FrozenMaterials::capture(&core.content, &course)?;
    let git = repository(core)?;
    let cp = git.commit(&frozen, None)?;
    let assets: BTreeMap<_, _> = crate::assets::list(&core.content, &course)?.into_iter().map(|a| Ok((a.id.clone(), crate::material_git::attachment_path(&a.id, &a.file_name)?))).collect::<Result<_>>()?;
    // Verify every included source against the same fixed checkpoint, including explicit sources.
    for m in &ctx.manifest {
        let path = assets.get(&m.id).cloned().unwrap_or_else(|| format!("documents/{}.md", m.id));
        let bytes = git.read(&course, &cp.checkpoint_commit_oid, &path)?;
        if content::hash(std::str::from_utf8(&bytes).map_err(|_| conflict())?) != m.version { return Err(conflict()); }
    }
    let raw = git.read(&course, &cp.checkpoint_commit_oid, &format!("documents/{}.md", i.document_id))?;
    if content::hash(std::str::from_utf8(&raw).map_err(|_| conflict())?) != document.version { return Err(conflict()); }
    let mut focus = Focus { id: String::new(), course_id: course.clone(), document_id: i.document_id.clone(), source_document_id: crate::course_sessions::source(&core.content, &session, &i.document_id)?, path: format!("documents/{}.md", i.document_id), r#type: "markdown".into(), commit_hash: cp.checkpoint_commit_oid, material_tree_oid: cp.material_tree_oid, excluded_files: cp.excluded_files, document_version: document.version, view: i.view.clone(), paragraphs: vec![], exercise: None, asset: None };
    if document.metadata.kind == "exercise" {
        if i.view != "exercise" && i.view != "edit" { return Err(conflict()); }
        focus.r#type = "exercise".into();
        let origin = crate::exercises::get(&mut core.content, &i.document_id)?;
        let questions = origin.exercises["exercises"].as_array().ok_or_else(conflict)?;
        let (number, question) = questions.iter().enumerate().find(|(_, q)| i.question_id.as_deref().is_none_or(|id| q["id"].as_str() == Some(id))).ok_or_else(conflict)?;
        let id = question["id"].as_str().ok_or_else(conflict)?;
        let attempt = origin.session.entries.iter().rev().find(|e| e.kind == "attempt" && e.data.as_ref().is_some_and(|d| d["exerciseId"] == id));
        let graded = attempt.is_some_and(|a| origin.session.entries.iter().any(|e| e.kind == "feedback" && e.data.as_ref().is_some_and(|d| d["attemptId"] == a.id)));
        focus.exercise = Some(json!({"questionId":id,"number":number+1,"answered":attempt.is_some(),"graded":graded}));
        // Exercise Markdown contains reference answers: never auto-project its visible text.
    } else if let Some(asset_id) = &i.asset_id {
        if i.view != "asset" { return Err(conflict()); }
        let asset = crate::assets::list(&core.content, &course)?.into_iter().find(|a| &a.id == asset_id).ok_or_else(conflict)?;
        focus.path = assets.get(asset_id).ok_or_else(conflict)?.clone();
        let bytes = git.read(&course, &focus.commit_hash, &focus.path)?;
        if format!("{:x}", Sha256::digest(&bytes)) != asset.sha256 { return Err(conflict()); }
        focus.r#type = if asset.mime.starts_with("image/") { "image" } else if asset.mime == "application/pdf" { "pdf" } else { "text" }.into();
        focus.document_version = asset.sha256;
        focus.asset = Some(json!({"id":asset.id,"mime":asset.mime,"size":asset.size}));
        // Binary previews provide identity only; text attachments use the same bounded ranges.
        if focus.r#type == "text" { focus.paragraphs = paragraphs(std::str::from_utf8(&bytes).map_err(|_| conflict())?, &i.ranges)?; }
    } else {
        if i.view != "read" && i.view != "edit" { return Err(conflict()); }
        focus.paragraphs = paragraphs(&document.body, &i.ranges)?;
    }
    let mut record = Frozen { focus, materials: ctx.automatic_materials.clone(), context: ctx, assets, requested: context::RequestedContext { focus: None, document_id: i.document_id.clone(), selection: request.selection, injections: request.injections, assistant_references: request.assistant_references, exercise_references: request.exercise_references } };
    record.focus.id = hash(&serde_json::to_string(&record)?);
    atomic_json(&directory(core)?.join(format!("{}.json", record.focus.id)), &record)?;
    Ok(record.focus)
}
pub fn load(core: &mut Core, session: &Session, focus: &Focus) -> Result<Frozen> {
    if focus.id.len() != 64 || !focus.id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) { return Err(conflict()); }
    let path = directory(core)?.join(format!("{}.json", focus.id));
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 4 * 1024 * 1024 { return Err(conflict()); }
    let mut record: Frozen = serde_json::from_slice(&fs::read(path)?)?;
    if record.focus != *focus { return Err(conflict()); }
    record.focus.id.clear();
    if hash(&serde_json::to_string(&record)?) != focus.id { return Err(conflict()); }
    record.focus.id = focus.id.clone();
    if crate::course_sessions::source(&core.content, session, &focus.document_id)? != focus.source_document_id { return Err(conflict()); }
    if crate::course_sessions::course(&core.content, &focus.document_id)?.as_ref() != Some(&focus.course_id) { return Err(conflict()); }
    let cp = open_materials(core)?.checkpoint(&focus.course_id, &focus.commit_hash)?;
    if cp.material_tree_oid != focus.material_tree_oid || cp.excluded_files != focus.excluded_files { return Err(conflict()); }
    for m in &record.context.manifest {
        if record.assets.contains_key(&m.id) {
            if !crate::assets::list(&core.content, &focus.course_id)?.iter().any(|a| a.id == m.id) { return Err(conflict()); }
        } else { crate::course_sessions::source(&core.content, session, &m.id)?; }
    }
    crate::assistant_references::validate_references(&core.content, session, &record.requested.assistant_references)?;
    crate::exercise_references::validate_references(&mut core.content, session, &record.requested.exercise_references)?;
    if let Some(asset) = &focus.asset {
        if !crate::assets::list(&core.content, &focus.course_id)?.iter().any(|a| Some(a.id.as_str()) == asset["id"].as_str()) { return Err(conflict()); }
    }
    record.context.automatic_materials = record.materials.clone();
    context::protect_trashed_history(&mut record.context, session, &core.content.trashed_ids());
    Ok(record)
}
impl Frozen {
    pub fn matches(&self, request: &crate::generation::GenerationRequest) -> Result<bool> {
        Ok(self.requested.document_id == request.document_id.as_deref().unwrap_or_default() && self.requested.selection == request.selection && serde_json::to_value(&self.requested.injections)? == serde_json::to_value(&request.injections)? && serde_json::to_value(&self.requested.assistant_references)? == serde_json::to_value(&request.assistant_references)? && serde_json::to_value(&self.requested.exercise_references)? == serde_json::to_value(&request.exercise_references)?)
    }
}
pub fn read_document(core: &Core, focus: &Focus, id: &str) -> Result<Document> {
    if !valid_id(id) { return Err(conflict()); }
    let text = String::from_utf8(open_materials(core)?.read(&focus.course_id, &focus.commit_hash, &format!("documents/{id}.md"))?).map_err(|_| conflict())?;
    let (metadata, body) = content::parse(&text)?;
    Ok(Document { id: id.into(), path: format!("courses/{id}.md"), version: content::hash(&text), content: text, metadata, body, modified_at: String::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::{Engine, UiPreferences}, generation::{Run, ManagedRun}, learning_tools::ToolScope};
    fn setup() -> (tempfile::TempDir, std::sync::Arc<Engine>, String, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut c = e.lock().unwrap();
        let course = crate::library_tree::create(&mut c.content, "course", "C", None, "", "focus-course").unwrap();
        let a = crate::library_tree::create(&mut c.content, "section", "A", Some(&course.id), "原文😀\n\n末段", "focus-note-a").unwrap();
        let b = crate::library_tree::create(&mut c.content, "section", "B", Some(&course.id), "原文😀\n\n末段", "focus-note-b").unwrap();
        let sid = c.start_session(&a.id, false).unwrap().0.id;
        drop(c); (dir, e, a.id, b.id, sid)
    }
    fn request(c: &Core, sid: &str, id: &str) -> FreezeRequest {
        let d = c.content.read(id).unwrap();
        FreezeRequest { session_id: sid.into(), input: Input { document_id: id.into(), expected_body: d.body, expected_version: Some(d.version), view: "read".into(), ranges: vec![Range { start: 0, end: 4 }], question_id: None, asset_id: None }, selection: String::new(), injections: vec![], assistant_references: vec![], exercise_references: vec![] }
    }
    #[test]
    fn frozen_bytes_and_view_survive_edits_navigation_and_engine_reopen() {
        let (dir, e, a, b, sid) = setup();
        let mut c = e.lock().unwrap(); let r = request(&c, &sid, &a); let f = freeze(&mut c, r).unwrap();
        assert_eq!(f.paragraphs[0]["text"], "原文😀");
        let other = request(&c, &sid, &b); let fb = freeze(&mut c, other).unwrap();
        assert_ne!(f.document_id, fb.document_id); assert_ne!(f.path, fb.path);
        let d = c.content.read(&a).unwrap(); c.content.save(&a, &content::serialize(&d.metadata, "NEW WORKSPACE").unwrap(), &d.version).unwrap();
        assert_eq!(c.start_session(&b, false).unwrap().0.id, sid);
        let session = crate::sessions::get(&mut c.content, &sid).unwrap();
        let old = load(&mut c, &session, &f).unwrap(); assert!(!old.context.text.contains("NEW WORKSPACE"));
        assert_eq!(read_document(&c, &f, &a).unwrap().body, "原文😀\n\n末段");
        let projection = crate::automatic_context::prepare_native(&c.state_dir, &session, &old.context);
        let request: crate::generation::GenerationRequest = serde_json::from_value(json!({"focus":f,"documentId":a,"sessionId":sid,"mode":"explain","selection":"","prompt":"Explain","requestId":"focus-request"})).unwrap();
        assert!(old.matches(&request).unwrap());
        let prompt = crate::context_bridge::ContextBridge::freeze(&request, &old.context, &session, None, &projection, crate::time_context::for_operation(&c.state_dir, "focus-test").unwrap()).unwrap();
        let envelope: Value = serde_json::from_str(prompt.messages[0]["content"].as_str().unwrap()).unwrap();
        assert_eq!(envelope["focus"], json!(f)); assert_eq!(envelope["activeDocumentId"], a);
        drop(c); drop(e);
        let reopened = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut c = reopened.lock().unwrap(); c.content.refresh().unwrap();
        let session = crate::sessions::get(&mut c.content, &sid).unwrap();
        assert_eq!(load(&mut c, &session, &f).unwrap().focus, f);
    }
    #[test]
    fn fixed_read_and_query_use_the_same_run_checkpoint_after_workspace_edits() {
        let (_dir, e, _a, b, sid) = setup();
        e.save_ui_preferences(UiPreferences { tool_authorization_tier: "read".into(), ..UiPreferences::default() }).unwrap();
        let mut c = e.lock().unwrap(); let r = request(&c, &sid, &b); let f = freeze(&mut c, r).unwrap();
        let session = crate::sessions::get(&mut c.content, &sid).unwrap(); let ctx = load(&mut c, &session, &f).unwrap().context;
        let run: Run = serde_json::from_value(json!({"focus":f,"id":"run_focus_fixture","sessionId":sid,"mode":"explain","status":"cancelled","content":"","createdAt":"fixture","updatedAt":"fixture","requestDigest":"fixture","contextSnapshotVersion":1,"contextManifest":ctx.manifest})).unwrap();
        e.runs.lock().unwrap().insert(run.id.clone(), ManagedRun { run, cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)) });
        let d = c.content.read(&b).unwrap(); c.content.save(&b, &content::serialize(&d.metadata, "NEW WORKSPACE").unwrap(), &d.version).unwrap(); drop(c);
        let scope = ToolScope { session_id: sid, section_id: b.clone(), run_id: "run_focus_fixture".into(), allowed_ids: vec![b.clone()] };
        let read = crate::authorized_tools::execute(&e, &scope, "read-old", "read", json!({"sourceId":b})).unwrap();
        assert!(read["content"].as_str().unwrap().contains("原文😀")); assert!(!read["content"].as_str().unwrap().contains("NEW WORKSPACE"));
        let query = crate::learning_tools::execute(&e, &scope, "query-old", "query_related_knowledge", json!({"knowledgeId":b})).unwrap();
        assert!(query["results"][0]["excerpt"].as_str().unwrap().contains("原文😀"));
    }
    #[test]
    fn exercise_focus_contains_question_state_without_reference_answers() {
        let (_dir, e, a, _b, sid) = setup(); let mut c = e.lock().unwrap();
        let exercise = crate::exercises::create_manual(&mut c.content, &sid, &a, crate::exercises::ManualExercise { question: "Question?".into(), format: "exact".into(), answers: vec!["HIDDEN_SECRET_ANSWER".into()], explanation: "EXPLANATION_SECRET".into() }, "focus-manual-exercise").unwrap();
        let mut r = request(&c, &sid, &exercise.document.id); r.input.view = "exercise".into(); r.input.ranges.clear();
        let f = freeze(&mut c, r).unwrap(); assert_eq!(f.exercise.as_ref().unwrap()["number"], 1); assert_eq!(f.exercise.as_ref().unwrap()["answered"], false); assert!(f.paragraphs.is_empty());
        let session = crate::sessions::get(&mut c.content, &sid).unwrap(); let ctx = load(&mut c, &session, &f).unwrap().context;
        assert!(!serde_json::to_string(&f).unwrap().contains("HIDDEN_SECRET")); assert!(!ctx.text.contains("HIDDEN_SECRET"));
    }
    #[test]
    fn invalid_ranges_foreign_courses_and_tampered_snapshots_fail_closed() {
        let (_dir, e, a, _b, sid) = setup(); let mut c = e.lock().unwrap();
        let mut r = request(&c, &sid, &a); r.input.ranges[0].end = 3; assert!(freeze(&mut c, r).is_err()); // split emoji
        let r = request(&c, &sid, &a); let mut f = freeze(&mut c, r).unwrap();
        let session = crate::sessions::get(&mut c.content, &sid).unwrap(); f.view = "edit".into(); assert!(load(&mut c, &session, &f).is_err());
        let foreign = crate::library_tree::create(&mut c.content, "course", "Other", None, "", "focus-other-course").unwrap();
        let x = crate::library_tree::create(&mut c.content, "section", "X", Some(&foreign.id), "X", "focus-other-note").unwrap();
        let r = request(&c, &sid, &x.id); assert!(freeze(&mut c, r).is_err());
    }
    #[test]
    fn image_focus_is_metadata_only_and_preserves_checkpoint_exclusions() {
        let (_dir, e, a, _b, sid) = setup(); let mut c = e.lock().unwrap();
        let owner = crate::library_tree::course_id(&c.content, &a).unwrap();
        let bytes = vec![137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 31, 0, 3, 3, 2, 0, 239, 163, 87, 151, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130];
        let image = crate::assets::import(&c.content, &owner, "test.png", &bytes, "focus-image").unwrap();
        crate::assets::import(&c.content, &owner, "auth.json", b"{}", "focus-excluded").unwrap();
        let mut r = request(&c, &sid, &a); r.input.view = "asset".into(); r.input.asset_id = Some(image.id.clone()); r.input.ranges.clear();
        let f = freeze(&mut c, r).unwrap(); assert_eq!(f.r#type, "image"); assert_eq!(f.document_version, image.sha256); assert!(f.paragraphs.is_empty()); assert_eq!(f.excluded_files.len(), 1);
        assert_eq!(f.excluded_files[0].file_name, "auth.json"); assert!(!serde_json::to_string(&f).unwrap().contains("iVBOR"));
    }
}
