//! Course routing for the existing authorized read/write tools. The private
//! receipt ledger and Bash cwd remain session-scoped.
use super::*;
use crate::app::Core;

fn conflict() -> CoreError {
    CoreError::new(
        "CONFLICT",
        "文件版本已变化；请重新读取后再写入，原文件保留。",
    )
}

fn course(core: &mut Core, scope: &ToolScope) -> Result<Option<String>> {
    core.content.refresh()?;
    let session = crate::sessions::get(&mut core.content, &scope.session_id)?;
    if core.content.is_trashed(&scope.session_id) {
        return Err(out_of_scope());
    }
    let owner = match session.course_id {
        Some(id) => Some(id),
        None if scope.section_id.is_empty() => None,
        None => crate::course_sessions::course(&core.content, &scope.section_id)?,
    };
    if let Some(id) = &owner {
        core.content.read(id)?;
        crate::course_project::ensure_project(&mut core.content, id)?;
    }
    Ok(owner)
}

// Reject every symlink component before creating directories. Canonicalizing
// only after create_dir_all could already have created directories outside.
fn target(core: &Core, course: &str, path: &str) -> Result<PathBuf> {
    if path.chars().count() > 128 {
        return Err(invalid());
    }
    let root =
        crate::course_project::course_project_root(core.content.root(), course)?.join("materials");
    crate::material_git::validate_material_write_path(&root, path)?;
    let mut target = root;
    for part in path.split('/') {
        target.push(part);
        match fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(out_of_scope()),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e)),
        }
    }
    if core
        .content
        .is_path_trashed(&format!("courses/{course}/materials/{path}"))?
    {
        return Err(out_of_scope());
    }
    Ok(target)
}

fn document(
    core: &Core,
    scope: &ToolScope,
    course: &str,
    path: &str,
) -> Result<Option<crate::types::Document>> {
    let first = path.split('/').next().unwrap_or("");
    if (first.eq_ignore_ascii_case("documents") && first != "documents")
        || first.eq_ignore_ascii_case("assets")
    {
        return Err(out_of_scope());
    }
    if let Some(id) = path
        .strip_prefix("documents/")
        .and_then(|p| p.strip_suffix(".md"))
    {
        if scope.section_id != id && !scope.allowed_ids.iter().any(|allowed| allowed == id) {
            return Err(out_of_scope());
        }
        let doc = core.content.read(id)?;
        if doc.path != format!("courses/{course}/materials/{path}")
            || !matches!(
                doc.metadata.kind.as_str(),
                "section" | "knowledge" | "exploration"
            )
        {
            return Err(out_of_scope());
        }
        return Ok(Some(doc));
    }
    // Business document creation and attachment metadata stay with their
    // existing services. No raw frontmatter can register a new identity.
    if path.starts_with("documents/") || path.starts_with("assets/") {
        return Err(out_of_scope());
    }
    Ok(None)
}

pub(super) fn course_read(
    engine: &crate::app::Engine,
    scope: &ToolScope,
    path: &str,
) -> Result<Option<Value>> {
    let mut core = engine.lock()?;
    let Some(course) = course(&mut core, scope)? else {
        return Ok(None);
    };
    target(&core, &course, path)?;
    let doc = document(&core, scope, &course, path)?;
    let (text, version) = if let Some(doc) = doc {
        (doc.body, doc.version)
    } else {
        let (bytes, version) = crate::versioning::read_versioned(&mut core, &course, path)?;
        let text = crate::material_git::validate_material_text(&bytes)?.to_owned();
        (text, version)
    };
    Ok(Some(read_result(scope, &course, path, &text, &version)?))
}

fn read_result(
    scope: &ToolScope,
    course: &str,
    path: &str,
    text: &str,
    version: &str,
) -> Result<Value> {
    let (content, truncated) = bounded_text(text, MAX_READ_BYTES);
    let mut result = json!({"sessionId":scope.session_id,"courseId":course,"path":path,
        "version":version,"bytes":text.len(),"content":content,"truncated":truncated});
    fit_result_bytes(&mut result)?;
    Ok(result)
}

pub(super) fn course_write(
    engine: &crate::app::Engine,
    scope: &ToolScope,
    args: &WriteArgs,
    ledger: &Path,
) -> Result<Option<Value>> {
    let mut core = engine.lock()?;
    course_write_locked(&mut core, scope, args, ledger, false)
}

fn course_write_locked(
    core: &mut Core,
    scope: &ToolScope,
    args: &WriteArgs,
    ledger: &Path,
    accepted: bool,
) -> Result<Option<Value>> {
    let Some(course) = course(core, scope)? else {
        return Ok(None);
    };
    crate::course_write_transaction::ensure_supported()?;
    let pending = ledger.with_extension("pending");
    if pending.exists() {
        return Err(CoreError::new(
            "NATIVE_TOOL_UNKNOWN",
            "原写入结果尚不可核实；未重复执行。",
        ));
    }
    let root =
        crate::course_project::course_project_root(core.content.root(), &course)?.join("materials");
    crate::material_git::validate_material_write_path(&root, &args.path)?;
    let target = target(&core, &course, &args.path)?;
    let doc = document(&core, scope, &course, &args.path)?;
    let existing = fs::symlink_metadata(&target).ok();
    if existing.as_ref().is_some_and(|m| !m.is_file()) {
        return Err(invalid());
    }
    let baseline = if let Some(doc) = &doc {
        Some((doc.body.clone(), doc.version.clone()))
    } else if existing.is_some() {
        let (bytes, version) = crate::versioning::read_versioned(core, &course, &args.path)?;
        Some((
            crate::material_git::validate_material_text(&bytes)?.to_owned(),
            version,
        ))
    } else {
        None
    };
    if let Some((body, version)) = &baseline {
        if read_result(scope, &course, &args.path, body, version)?["truncated"] == true {
            return Err(CoreError::new(
                "TOOL_SIZE_LIMIT",
                "原文件超出完整读取范围，不能用前缀覆盖；请在编辑器中修改，原文件保留。",
            ));
        }
    }
    let current_version = baseline.as_ref().map(|(_, version)| version.clone());
    if args.expected_version != current_version {
        return Err(conflict());
    }
    let content = if let Some(doc) = &doc {
        crate::content::serialize(&doc.metadata, &args.content)?
    } else {
        // A raw Markdown material must not become a business record on refresh.
        if crate::content::parse(&args.content).is_ok() {
            return Err(out_of_scope());
        }
        crate::material_git::validate_material_text(args.content.as_bytes())?;
        args.content.clone()
    };
    let operation = ledger
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    if !accepted {
        return stage(
            core,
            scope,
            &course,
            args,
            baseline
                .as_ref()
                .map(|(body, _)| body.as_str())
                .unwrap_or(""),
            operation,
        )
        .map(Some);
    }
    let published_version = if doc.is_none() {
        Some(crate::versioning::write_version(
            core,
            &course,
            content.as_bytes(),
        )?)
    } else {
        None
    };
    // Course files use the existing per-document / material validation above.
    // The 2 MiB scratch-workspace quota must not count the user's whole course.
    // Durable intent closes the write→receipt interruption gap. Unknown writes
    // are never replayed; recovery reads the original completed ledger only.
    let session = crate::sessions::get(&mut core.content, &scope.session_id)?;
    crate::course_write_transaction::durable_json(
        &pending,
        &json!({"path":args.path,"courseId":course,"expectedVersion":args.expected_version,
            "sessionId":session.id,"sectionId":session.section_id}),
    )?;
    let version = if let Some(doc) = doc {
        core.content
            .save_course_tool(&doc.id, &content, &doc.version, operation)?
            .version
    } else {
        crate::course_write_transaction::publish(
            &root,
            &args.path,
            baseline.as_ref().map(|(body, _)| body.as_bytes()),
            content.as_bytes(),
            operation,
        )?;
        published_version.unwrap()
    };
    Ok(Some(
        json!({"persisted":true,"sessionId":scope.session_id,"courseId":course,
        "path":args.path,"version":version,"bytes":content.len(),"overwritten":existing.is_some()}),
    ))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Proposal {
    id: String,
    course_id: String,
    session_id: String,
    section_id: String,
    run_id: String,
    allowed_ids: Vec<String>,
    path: String,
    expected_version: Option<String>,
    original: String,
    content: String,
    status: String,
    receipt: Option<Value>,
}

fn proposal_path(core: &Core, course: &str, id: &str) -> Result<PathBuf> {
    if !crate::types::valid_id(course) || !crate::types::valid_id(id) {
        return Err(invalid());
    }
    Ok(core
        .state_dir
        .join("edit-proposals")
        .join(course)
        .join(format!("{id}.json")))
}
fn read_proposal(core: &Core, course: &str, id: &str) -> Result<Proposal> {
    let path = proposal_path(core, course, id)?;
    let mut p: Proposal = serde_json::from_slice(&crate::assets::bounded_read(&path, 256 * 1024)?)?;
    if p.id != id || p.course_id != course {
        return Err(out_of_scope());
    }
    // A crash before durable write intent cannot have published anything. Only
    // reopen this window with the exact original pending operation evidence;
    // missing/mismatched ledgers and any intent remain unknown.
    if p.status == "accepting" {
        let ledger = workspace_root(&core.state_dir, &p.session_id)
            .join("receipts")
            .join(format!("{id}.json"));
        let no_intent = fs::symlink_metadata(ledger.with_extension("pending"))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
        if no_intent {
            if let Ok(entry) = read_ledger(&ledger) {
                let mut args =
                    json!({"path":p.path,"content":p.content,"expectedVersion":p.expected_version});
                let with_version = hash(&serde_json::to_string(&args)?);
                if p.expected_version.is_none() {
                    args.as_object_mut().unwrap().remove("expectedVersion");
                }
                let without_version = hash(&serde_json::to_string(&args)?);
                if entry.name == "write"
                    && (entry.arguments_hash == with_version
                        || entry.arguments_hash == without_version)
                    && entry.result["persisted"] == false
                    && entry.result["status"] == "pending"
                    && entry.result["proposalId"] == p.id
                    && entry.result["courseId"] == p.course_id
                    && entry.result["sessionId"] == p.session_id
                    && entry.result["path"] == p.path
                {
                    p.status = "pending".into();
                }
            }
        }
    }
    Ok(p)
}
fn check_course(core: &mut Core, id: &str) -> Result<()> {
    core.content.refresh()?;
    if !crate::types::valid_id(id)
        || core.content.is_trashed(id)
        || core.content.read(id)?.metadata.kind != "course"
    {
        return Err(out_of_scope());
    }
    Ok(())
}
fn stage(
    core: &mut Core,
    scope: &ToolScope,
    course: &str,
    args: &WriteArgs,
    original: &str,
    id: &str,
) -> Result<Value> {
    let path = proposal_path(core, course, id)?;
    if path.exists() {
        return Ok(proposal_receipt(&read_proposal(core, course, id)?));
    }
    let directory = path.parent().ok_or_else(invalid)?;
    fs::create_dir_all(directory).map_err(io)?;
    for item in fs::read_dir(directory).map_err(io)? {
        let item = item.map_err(io)?;
        let other: Proposal =
            serde_json::from_slice(&crate::assets::bounded_read(&item.path(), 256 * 1024)?)?;
        if other.path == args.path
            && matches!(other.status.as_str(), "pending" | "accepting" | "unknown")
        {
            return Err(CoreError::new(
                "CONFLICT",
                "该文件已有待审阅建议，请先接受或拒绝；正文未修改。",
            ));
        }
    }
    let p = Proposal {
        id: id.into(),
        course_id: course.into(),
        session_id: scope.session_id.clone(),
        section_id: scope.section_id.clone(),
        run_id: scope.run_id.clone(),
        allowed_ids: scope.allowed_ids.clone(),
        path: args.path.clone(),
        expected_version: args.expected_version.clone(),
        original: original.into(),
        content: args.content.clone(),
        status: "pending".into(),
        receipt: None,
    };
    crate::course_write_transaction::durable_json(&path, &p)?;
    Ok(proposal_receipt(&p))
}
fn proposal_receipt(p: &Proposal) -> Value {
    p.receipt.clone().unwrap_or_else(|| json!({"persisted":false,"proposalPersisted":true,"proposalId":p.id,"courseId":p.course_id,"sessionId":p.session_id,"path":p.path,"status":p.status,"message":"建议已保留，正文未修改；等待用户决定，不得依赖建议内容继续执行。"}))
}

/// Lightweight polling: no proposal body is read until its file stamp changes.
pub(crate) fn proposal_list(engine: &crate::app::Engine, course: &str) -> Result<Value> {
    let mut core = engine.lock()?;
    check_course(&mut core, course)?;
    let dir = core.state_dir.join("edit-proposals").join(course);
    let entries = match fs::read_dir(dir) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!([])),
        Err(e) => return Err(io(e)),
    };
    let mut result = Vec::new();
    for item in entries {
        let item = item.map_err(io)?;
        let id = item
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_owned();
        let meta = item.metadata().map_err(io)?;
        if crate::types::valid_id(&id) {
            result.push(
                json!({"id":id,"stamp":format!("{:?}-{}",meta.modified().map_err(io)?,meta.len())}),
            );
        }
    }
    Ok(json!(result))
}
pub(crate) fn proposal_get(engine: &crate::app::Engine, course: &str, id: &str) -> Result<Value> {
    let mut core = engine.lock()?;
    check_course(&mut core, course)?;
    let p = read_proposal(&core, course, id)?;
    Ok(
        json!({"id":p.id,"courseId":p.course_id,"path":p.path,"original":p.original,"content":p.content,"status":p.status,"expectedVersion":p.expected_version}),
    )
}
pub(crate) fn proposal_result(engine: &crate::app::Engine, result: Value) -> Result<Value> {
    let Some(id) = result["proposalId"].as_str() else {
        return Ok(result);
    };
    let course = result["courseId"].as_str().ok_or_else(invalid)?;
    let core = engine.lock()?;
    let p = read_proposal(&core, course, id)?;
    if matches!(p.status.as_str(), "accepting" | "unknown") {
        let ledger = workspace_root(&core.state_dir, &p.session_id)
            .join("receipts")
            .join(format!("{}.json", p.id));
        if let Ok(entry) = read_ledger(&ledger) {
            if entry.result["persisted"] == true {
                return Ok(entry.result);
            }
        }
        drop(core);
        return Err(course_write_unknown(engine, &p.session_id, &ledger));
    }
    Ok(proposal_receipt(&p))
}
pub(crate) fn proposal_decide(
    engine: &crate::app::Engine,
    course_id: &str,
    id: &str,
    accept: bool,
) -> Result<Value> {
    // Acceptance does not grant write permission. Reject remains available after revocation.
    if accept && tier_rank(&engine.ui_preferences()?.tool_authorization_tier) < 2 {
        return Err(not_allowed());
    }
    let mut core = engine.lock()?;
    check_course(&mut core, course_id)?;
    let mut p = read_proposal(&core, course_id, id)?;
    if p.status != "pending" {
        if matches!(p.status.as_str(), "accepting" | "unknown") {
            let ledger = workspace_root(&core.state_dir, &p.session_id)
                .join("receipts")
                .join(format!("{id}.json"));
            if let Ok(entry) = read_ledger(&ledger) {
                if entry.result["persisted"] == true {
                    p.status = "accepted".into();
                    p.receipt = Some(entry.result);
                    crate::course_write_transaction::durable_json(
                        &proposal_path(&core, course_id, id)?,
                        &p,
                    )?;
                    return Ok(proposal_receipt(&p));
                }
            }
            drop(core);
            return Err(course_write_unknown(engine, &p.session_id, &ledger));
        }
        return Ok(proposal_receipt(&p));
    }
    let path = proposal_path(&core, course_id, id)?;
    if !accept {
        p.status = "rejected".into();
        p.receipt = Some(
            json!({"persisted":false,"status":"rejected","proposalId":id,"courseId":course_id,"path":p.path,"message":"用户已拒绝建议，正文未修改。"}),
        );
        crate::course_write_transaction::durable_json(&path, &p)?;
        return Ok(proposal_receipt(&p));
    }
    let scope = ToolScope {
        session_id: p.session_id.clone(),
        section_id: p.section_id.clone(),
        run_id: p.run_id.clone(),
        allowed_ids: p.allowed_ids.clone(),
    };
    if course(&mut core, &scope)?.as_deref() != Some(course_id) {
        return Err(out_of_scope());
    }
    let args = WriteArgs {
        path: p.path.clone(),
        content: p.content.clone(),
        expected_version: p.expected_version.clone(),
    };
    let ledger = workspace_root(&core.state_dir, &p.session_id)
        .join("receipts")
        .join(format!("{id}.json"));
    p.status = "accepting".into();
    crate::course_write_transaction::durable_json(&path, &p)?;
    match course_write_locked(&mut core, &scope, &args, &ledger, true) {
        Ok(Some(receipt)) => {
            p.status = "accepted".into();
            p.receipt = Some(receipt);
        }
        outcome => {
            p.status = if ledger.with_extension("pending").exists() {
                "unknown"
            } else {
                "pending"
            }
            .into();
            crate::course_write_transaction::durable_json(&path, &p)?;
            let error = outcome.err().unwrap_or_else(out_of_scope);
            if p.status == "unknown" {
                drop(core);
                return Err(course_write_unknown(engine, &p.session_id, &ledger));
            }
            return Err(error);
        }
    }
    // Completion ledger survives interruption before proposal status sync.
    let completion = (|| -> Result<()> {
        let mut entry = read_ledger(&ledger)?;
        entry.result = proposal_receipt(&p);
        crate::course_write_transaction::durable_json(&ledger, &entry)?;
        crate::course_write_transaction::durable_json(&path, &p)
    })();
    if completion.is_err() {
        drop(core);
        return Err(course_write_unknown(engine, &p.session_id, &ledger));
    }
    Ok(proposal_receipt(&p))
}
