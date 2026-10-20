//! One request's view and authorized materials. Git identifies bytes, never Pi acceptance.
use crate::{app::{atomic_json, Core}, automatic_context::MaterialSnapshot, content, context, material_git::{FrozenMaterials, MaterialGit}, types::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet}, fs};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Focus {
    pub id: String,
    pub course_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    pub document_id: String,
    pub source_document_id: String,
    pub path: String,
    pub r#type: String,
    pub commit_hash: String,
    pub material_tree_oid: String,
    pub excluded_files: Vec<crate::material_git::ExcludedFile>,
    pub document_version: String,
    pub view: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub binary: bool,
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
fn is_false(value: &bool) -> bool { !*value }
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
        Ok(json!({"start":r.start,"end":r.end,"startChar":body[..offset(r.start).unwrap()].chars().count(),"endChar":body[..offset(r.end).unwrap()].chars().count(),"totalChars":body.chars().count(),"totalBytes":body.len(),"text":text}))
    }).collect()
}
// Remove only automatically supplied bodies by trusted Context byte spans, never text matching.
fn metadata_only(ctx: &mut Context, binary_ids: &BTreeSet<String>) -> Result<()> {
    let mut text = String::new(); let mut cursor = 0; let mut retained = Vec::new();
    for mut source in std::mem::take(&mut ctx.automatic_materials) {
        let start = source.start; let end = source.end;
        if start < cursor || end < start || end > ctx.text.len() || !ctx.text.is_char_boundary(start) || !ctx.text.is_char_boundary(end) { return Err(conflict()); }
        text.push_str(&ctx.text[cursor..start]);
        if binary_ids.contains(&source.id) {
            for item in ctx.manifest.iter_mut().filter(|m| m.id == source.id) { item.included_chars = 0; }
            ctx.warnings.push(format!("BINARY_MATERIAL_METADATA_ONLY: {} Git检查点判为二进制，自动材料仅保留元数据。",source.id));
        } else {
            source.start = text.len(); text.push_str(&ctx.text[start..end]); source.end = text.len(); retained.push(source);
        }
        cursor = end;
    }
    text.push_str(&ctx.text[cursor..]); ctx.text = text; ctx.automatic_materials = retained; Ok(())
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
    let binary_paths: BTreeSet<_> = git.compare(&course, None, &cp.checkpoint_commit_oid, None)?.changes.into_iter().filter(|f|f.binary).map(|f|f.path).collect();
    let assets: BTreeMap<_, _> = crate::assets::list(&core.content, &course)?.into_iter().map(|a| Ok((a.id.clone(), crate::material_git::attachment_path(&a.id, &a.file_name)?))).collect::<Result<_>>()?;
    // Verify every included source against the same fixed checkpoint, including explicit sources.
    for m in &ctx.manifest {
        let path = assets.get(&m.id).cloned().unwrap_or_else(|| format!("documents/{}.md", m.id));
        let bytes = git.read(&course, &cp.checkpoint_commit_oid, &path)?;
        if content::hash(std::str::from_utf8(&bytes).map_err(|_| conflict())?) != m.version { return Err(conflict()); }
    }
    let binary_ids = ctx.automatic_materials.iter().filter(|m| binary_paths.contains(&assets.get(&m.id).cloned().unwrap_or_else(||format!("documents/{}.md",m.id)))).map(|m|m.id.clone()).collect();
    metadata_only(&mut ctx, &binary_ids)?;
    let raw = git.read(&course, &cp.checkpoint_commit_oid, &format!("documents/{}.md", i.document_id))?;
    if content::hash(std::str::from_utf8(&raw).map_err(|_| conflict())?) != document.version { return Err(conflict()); }
    let mut focus = Focus { id: String::new(), course_id: course.clone(), session_id: session.id.clone(), document_id: i.document_id.clone(), source_document_id: crate::course_sessions::source(&core.content, &session, &i.document_id)?, path: format!("documents/{}.md", i.document_id), r#type: "markdown".into(), commit_hash: cp.checkpoint_commit_oid, material_tree_oid: cp.material_tree_oid, excluded_files: cp.excluded_files, document_version: document.version, view: i.view.clone(), binary: binary_paths.contains(&format!("documents/{}.md",i.document_id)), paragraphs: vec![], exercise: None, asset: None };
    if document.metadata.kind == "exercise" && i.asset_id.is_none() {
        if i.view != "exercise" && i.view != "edit" { return Err(conflict()); }
        focus.r#type = "exercise".into();
        if i.view == "exercise" || i.question_id.is_some() {
        let origin = crate::exercises::get(&mut core.content, &i.document_id)?;
        let questions = origin.exercises["exercises"].as_array().ok_or_else(conflict)?;
        let (number, question) = questions.iter().enumerate().find(|(_, q)| i.question_id.as_deref().is_none_or(|id| q["id"].as_str() == Some(id))).ok_or_else(conflict)?;
        let id = question["id"].as_str().ok_or_else(conflict)?;
        let attempt = origin.session.entries.iter().rev().find(|e| e.kind == "attempt" && e.data.as_ref().is_some_and(|d| d["exerciseId"] == id));
        let graded = attempt.is_some_and(|a| origin.session.entries.iter().any(|e| e.kind == "feedback" && e.data.as_ref().is_some_and(|d| d["attemptId"] == a.id)));
        focus.exercise = Some(json!({"questionId":id,"number":number+1,"answered":attempt.is_some(),"graded":graded}));
        }
        // Exercise Markdown contains reference answers: never auto-project its visible text.
    } else if let Some(asset_id) = &i.asset_id {
        if i.view != "asset" { return Err(conflict()); }
        let asset = crate::assets::list(&core.content, &course)?.into_iter().find(|a| &a.id == asset_id).ok_or_else(conflict)?;
        focus.path = assets.get(asset_id).ok_or_else(conflict)?.clone(); focus.binary = binary_paths.contains(&focus.path);
        let bytes = git.read(&course, &focus.commit_hash, &focus.path)?;
        if format!("{:x}", Sha256::digest(&bytes)) != asset.sha256 { return Err(conflict()); }
        focus.r#type = if asset.mime.starts_with("image/") { "image" } else if asset.mime == "application/pdf" { "pdf" } else { "text" }.into();
        focus.document_version = asset.sha256;
        focus.asset = Some(json!({"id":asset.id,"mime":asset.mime,"size":asset.size}));
        // Binary previews provide identity only; text attachments use the same bounded ranges.
        if focus.r#type == "text" && !focus.binary { focus.paragraphs = paragraphs(std::str::from_utf8(&bytes).map_err(|_| conflict())?, &i.ranges)?; }
        if !ctx.manifest.iter().any(|m| m.id == *asset_id) {
            if ctx.manifest.len() >= crate::context::MAX_CONTEXT_DOCUMENTS { return Err(CoreError::new("LIMIT_EXCEEDED", "聚焦附件超过来源预算。")); }
            ctx.manifest.push(ManifestItem { id: asset_id.clone(), path: focus.path.clone(), version: focus.document_version.clone(), included_chars: focus.paragraphs.iter().filter_map(|p|p["text"].as_str()).map(|s|s.chars().count()).sum() });
        }

    } else {
        if i.view != "read" && i.view != "edit" { return Err(conflict()); }
        if !focus.binary { focus.paragraphs = paragraphs(&document.body, &i.ranges)?; }
    }
    let mut record = Frozen { focus, materials: ctx.automatic_materials.clone(), context: ctx, assets, requested: context::RequestedContext { focus: None, document_id: i.document_id.clone(), selection: request.selection, injections: request.injections, assistant_references: request.assistant_references, exercise_references: request.exercise_references } };
    record.focus.id = hash(&serde_json::to_string(&record)?);
    atomic_json(&directory(core)?.join(format!("{}.json", record.focus.id)), &record)?;
    Ok(record.focus)
}
pub fn load(core: &mut Core, session: &Session, focus: &Focus) -> Result<Frozen> {
    if !focus.session_id.is_empty() && focus.session_id != session.id { return Err(conflict()); }
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
    pub fn asset_path(&self, id: &str) -> Option<&str> { self.assets.get(id).map(String::as_str) }
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
    use crate::material_git::retry_busy;
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
        let mut c = e.lock().unwrap();
        let owner = crate::library_tree::course_id(&c.content, &b).unwrap();
        let asset = retry_busy(|| crate::assets::import(&c.content, &owner, "lesson.txt", b"Frozen attachment", "focus-text-asset")).unwrap();
        let d = c.content.read(&b).unwrap(); c.content.save(&b, &content::serialize(&d.metadata, &format!("{}\n\n[Text]({})", d.body, asset.reference)).unwrap(), &d.version).unwrap();
        let r = request(&c, &sid, &b); let f = freeze(&mut c, r).unwrap();
        let session = crate::sessions::get(&mut c.content, &sid).unwrap(); let ctx = load(&mut c, &session, &f).unwrap().context;
        let allowed_ids = ctx.manifest.iter().map(|m| m.id.clone()).collect();
        let run: Run = serde_json::from_value(json!({"focus":f,"id":"run_focus_fixture","sessionId":sid,"mode":"explain","status":"cancelled","content":"","createdAt":"fixture","updatedAt":"fixture","requestDigest":"fixture","contextSnapshotVersion":1,"contextManifest":ctx.manifest})).unwrap();
        e.runs.lock().unwrap().insert(run.id.clone(), ManagedRun { run, cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)) });
        let d = c.content.read(&b).unwrap(); c.content.save(&b, &content::serialize(&d.metadata, "NEW WORKSPACE").unwrap(), &d.version).unwrap(); drop(c);
        let scope = ToolScope { session_id: sid, section_id: b.clone(), run_id: "run_focus_fixture".into(), allowed_ids };
        let read = crate::authorized_tools::execute(&e, &scope, "read-old", "read", json!({"sourceId":b})).unwrap();
        assert!(read["content"].as_str().unwrap().contains("原文😀")); assert!(!read["content"].as_str().unwrap().contains("NEW WORKSPACE"));
        let query = crate::learning_tools::execute(&e, &scope, "query-old", "query_related_knowledge", json!({"query":"原文"})).unwrap();
        assert!(query["results"][0]["excerpt"].as_str().unwrap().contains("原文😀"));
        let attachment = crate::authorized_tools::execute(&e, &scope, "read-asset", "read", json!({"sourceId":asset.id})).unwrap();
        assert_eq!(attachment["content"], "Frozen attachment"); assert_eq!(attachment["version"], asset.sha256);
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
        let image = retry_busy(|| crate::assets::import(&c.content, &owner, "test.png", &bytes, "focus-image")).unwrap();
        retry_busy(|| crate::assets::import(&c.content, &owner, "auth.json", b"{}", "focus-excluded")).unwrap();
        let mut r = request(&c, &sid, &a); r.input.view = "asset".into(); r.input.asset_id = Some(image.id.clone()); r.input.ranges.clear();
        let f = freeze(&mut c, r).unwrap(); assert_eq!(f.r#type, "image"); assert_eq!(f.document_version, image.sha256); assert!(f.paragraphs.is_empty()); assert_eq!(f.excluded_files.len(), 1);
        assert_eq!(f.excluded_files[0].file_name, "auth.json"); assert!(!serde_json::to_string(&f).unwrap().contains("iVBOR"));
        let exercise = crate::exercises::create_manual(&mut c.content, &sid, &a, crate::exercises::ManualExercise { question:"Q?".into(), format:"exact".into(), answers:vec!["hidden".into()], explanation:"Reason".into() }, "focus-image-exercise").unwrap();
        let mut r = request(&c, &sid, &exercise.document.id); r.input.view = "asset".into(); r.input.asset_id = Some(image.id); r.input.ranges.clear();
        let f = freeze(&mut c, r).unwrap(); assert_eq!(f.r#type, "image"); assert_eq!(f.document_id, exercise.document.id);

    }
    #[test]
    fn focused_attachment_inventory_records_identity_partial_ranges_and_deduplicates_existing_sources() {
        let (_dir, e, a, _b, sid) = setup(); let mut c = e.lock().unwrap();
        let owner = crate::library_tree::course_id(&c.content, &a).unwrap();
        let bytes = "prefix\nVISIBLE😀\ntail".as_bytes();
        let asset = retry_busy(|| crate::assets::import(&c.content, &owner, "focused.txt", bytes, "focus-inventory-one")).unwrap();
        let other = retry_busy(|| crate::assets::import(&c.content, &owner, "same.txt", bytes, "focus-inventory-two")).unwrap();
        let mut r = request(&c, &sid, &a); r.input.view = "asset".into(); r.input.asset_id = Some(asset.id.clone()); r.input.ranges = vec![Range{start:7,end:16}];
        let f = freeze(&mut c, r).unwrap(); let session = crate::sessions::get(&mut c.content, &sid).unwrap(); let ctx = load(&mut c, &session, &f).unwrap().context;
        let item = ctx.manifest.iter().find(|m|m.id==asset.id).unwrap(); assert_eq!(item.included_chars,8); assert_eq!(item.version,asset.sha256);
        let req = serde_json::from_value(json!({"focus":f,"documentId":a,"sessionId":sid,"mode":"explain","selection":"","prompt":"Explain","requestId":"focus-inventory"})).unwrap();
        let projection = crate::automatic_context::prepare_native(&c.state_dir,&session,&ctx);
        let prompt = crate::context_bridge::ContextBridge::freeze(&req,&ctx,&session,None,&projection,crate::time_context::for_operation(&c.state_dir,"focus-inventory").unwrap()).unwrap();
        let envelope:Value = serde_json::from_str(prompt.messages[0]["content"].as_str().unwrap()).unwrap();
        let entry = envelope["contextBridgeManifest"]["entries"].as_array().unwrap().iter().find(|e|e["id"]=="focus:0").unwrap();
        assert_eq!(entry["sourceId"],asset.id); assert_eq!(entry["includedChars"],8); assert_eq!(entry["range"]["startChar"],7); assert_eq!(entry["range"]["endChar"],15); assert_eq!(entry["range"]["complete"],false);
        let d = c.content.read(&a).unwrap(); c.content.save(&a,&content::serialize(&d.metadata,&format!("{}\n\n[Text]({})",d.body,asset.reference)).unwrap(),&d.version).unwrap();
        let mut r = request(&c,&sid,&a); r.input.view="asset".into();r.input.asset_id=Some(asset.id.clone());r.input.ranges.clear();
        let quoted = freeze(&mut c,r).unwrap(); let quoted_ctx=load(&mut c,&session,&quoted).unwrap().context;assert_eq!(quoted_ctx.manifest.iter().filter(|m|m.id==asset.id).count(),1);
        let mut r = request(&c,&sid,&a);r.input.view="asset".into();r.input.asset_id=Some(other.id.clone());r.input.ranges.clear();
        let distinct=freeze(&mut c,r).unwrap();let distinct_ctx=load(&mut c,&session,&distinct).unwrap().context;assert!(distinct_ctx.manifest.iter().any(|m|m.id==other.id));assert_ne!(f.asset,distinct.asset);
    }
    #[test]
    fn frozen_focus_cannot_cross_independent_sessions_in_the_same_course() {
        let (_dir,e,a,_b,sid) = setup(); let mut c=e.lock().unwrap();
        let owner=crate::library_tree::course_id(&c.content,&a).unwrap();
        let second=crate::sessions::create_course(&mut c.content,&a,&[],"focus_other_chat",&owner,&[]).unwrap();
        let r=request(&c,&sid,&a);let f=freeze(&mut c,r).unwrap();
        let own=crate::sessions::get(&mut c.content,&sid).unwrap();assert!(load(&mut c,&own,&f).is_ok());
        assert_eq!(load(&mut c,&second,&f).err().map(|e|e.code),Some("CONTEXT_CONFLICT".into()));
        let r=request(&c,&second.id,&a);let other=freeze(&mut c,r).unwrap();assert_ne!(f.id,other.id);
        assert!(load(&mut c,&second,&other).is_ok());
    }
    #[test]
    fn git_binary_automatic_materials_are_metadata_only_but_authorized_active_read_remains_text() {
        let (_dir, e, a, b, sid) = setup();
        e.save_ui_preferences(UiPreferences { tool_authorization_tier: "read".into(), ..UiPreferences::default() }).unwrap();
        let mut c = e.lock().unwrap(); let d = c.content.read(&a).unwrap();
        let mut metadata = d.metadata; metadata.links = vec![b.clone()];
        c.content.save(&a, &content::serialize(&metadata, "Visible\0SYNTHETIC_BINARY_FOCUS_MARKER\n").unwrap(), &d.version).unwrap();
        let r = request(&c, &sid, &a); let f = freeze(&mut c, r).unwrap(); assert!(f.binary); assert!(f.paragraphs.is_empty());
        assert!(open_materials(&c).unwrap().compare(&f.course_id,None,&f.commit_hash,Some(&f.path)).unwrap().changes[0].binary);
        let session = crate::sessions::get(&mut c.content,&sid).unwrap(); let frozen = load(&mut c,&session,&f).unwrap(); let ctx = &frozen.context;
        assert_eq!(ctx.manifest.iter().find(|m|m.id==a).unwrap().included_chars,0);
        assert_eq!(ctx.automatic_materials.len(),1); assert_eq!(ctx.automatic_materials[0].id,b);
        let kept = &ctx.automatic_materials[0]; assert!(ctx.text[kept.start..kept.end].contains(&kept.text));
        let req = serde_json::from_value(json!({"focus":f,"documentId":a,"sessionId":sid,"mode":"explain","selection":"","prompt":"Synthetic question","requestId":"binary-focus"})).unwrap();
        let projection = crate::automatic_context::prepare_native(&c.state_dir,&session,ctx);
        let packet = crate::context_bridge::ContextBridge::freeze(&req,ctx,&session,None,&projection,crate::time_context::for_operation(&c.state_dir,"binary-focus").unwrap()).unwrap();
        let envelope:Value = serde_json::from_str(packet.messages[0]["content"].as_str().unwrap()).unwrap();
        assert!(!serde_json::to_string(&envelope).unwrap().contains("SYNTHETIC_BINARY_FOCUS_MARKER"));
        assert_eq!(envelope["metadataOnlySourceIds"],json!([a])); assert_eq!(envelope["nativeMaterials"][0]["id"],b);
        assert!(envelope["nativeMaterials"][0]["text"].as_str().unwrap().contains("原文😀"));
        let run:Run = serde_json::from_value(json!({"id":"run_binary_focus","focus":f,"sessionId":sid,"mode":"explain","status":"cancelled","content":"","createdAt":"fixture","updatedAt":"fixture","requestDigest":"fixture","contextSnapshotVersion":1,"contextManifest":ctx.manifest})).unwrap();
        let scope = ToolScope { session_id:sid,section_id:a.clone(),run_id:run.id.clone(),allowed_ids:ctx.manifest.iter().map(|m|m.id.clone()).collect() };
        e.runs.lock().unwrap().insert(run.id.clone(),ManagedRun { run,cancel:std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)) });
        let d = c.content.read(&a).unwrap(); c.content.save(&a,&content::serialize(&d.metadata,"NEW WORKSPACE").unwrap(),&d.version).unwrap(); drop(c);
        let active = crate::authorized_tools::execute(&e,&scope,"read-binary-markdown","read",json!({"sourceId":a})).unwrap();
        assert!(active["content"].as_str().unwrap().contains("SYNTHETIC_BINARY_FOCUS_MARKER")); assert!(!active["content"].as_str().unwrap().contains("NEW WORKSPACE"));
    }
    #[test]
    fn read_rejects_all_image_mimes_including_complete_valid_utf8_gif() {
        let (_dir,e,a,_b,sid) = setup();
        e.save_ui_preferences(UiPreferences { tool_authorization_tier:"read".into(),..UiPreferences::default() }).unwrap();
        let mut c = e.lock().unwrap(); let owner = crate::library_tree::course_id(&c.content,&a).unwrap();
        // Complete 1x1 GIF with a 256-color table; C7 80 is valid UTF-8. Encoding does not make it text.
        let mut gif = b"GIF89a".to_vec(); gif.extend_from_slice(&[1,0,1,0,0xc7,0x80,0]); gif.extend_from_slice(&[0;768]);
        gif.extend_from_slice(&[0x2c,0,0,0,0,1,0,1,0,0,2,2,0x44,1,0,0x3b]); assert!(std::str::from_utf8(&gif).is_ok());
        // Other entries are importer-accepted signature fixtures for the MIME gate, not decode tests.
        let fixtures = [("boundary.gif",gif),("boundary.webp",b"RIFF\0\0\0\0WEBP".to_vec()),("boundary.png",b"\x89PNG\r\n\x1a\n".to_vec()),("boundary.jpg",vec![0xff,0xd8,0xff])];
        for (index,(name,bytes)) in fixtures.into_iter().enumerate() {
            let asset = retry_busy(|| crate::assets::import(&c.content,&owner,name,&bytes,&format!("focus-mime-{index}"))).unwrap();
            let mut r = request(&c,&sid,&a); r.input.view="asset".into(); r.input.asset_id=Some(asset.id.clone()); r.input.ranges.clear();
            let f = freeze(&mut c,r).unwrap(); assert_eq!(f.r#type,"image"); assert!(f.paragraphs.is_empty());
            let session = crate::sessions::get(&mut c.content,&sid).unwrap(); let ctx = load(&mut c,&session,&f).unwrap().context;
            let run:Run = serde_json::from_value(json!({"id":format!("run_image_{index}"),"focus":f,"sessionId":sid,"mode":"explain","status":"running","content":"","createdAt":"fixture","updatedAt":"fixture","requestDigest":"fixture","contextSnapshotVersion":1,"contextManifest":ctx.manifest})).unwrap();
            let scope = ToolScope { session_id:sid.clone(),section_id:a.clone(),run_id:run.id.clone(),allowed_ids:vec![asset.id.clone()] };
            e.runs.lock().unwrap().insert(run.id.clone(),ManagedRun { run,cancel:std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)) }); drop(c);
            assert_eq!(retry_busy(|| crate::authorized_tools::execute(&e,&scope,&format!("read-image-{index}"),"read",json!({"sourceId":asset.id}))).unwrap_err().code,"UNSUPPORTED_TYPE");
            c=e.lock().unwrap();
        }
    }
}
