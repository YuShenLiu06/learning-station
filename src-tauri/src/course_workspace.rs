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
    let target = target(&core, &course, path)?;
    let doc = document(&core, scope, &course, path)?;
    let (text, version) = if let Some(doc) = doc {
        (doc.body, doc.version)
    } else {
        let (bytes, version) = crate::versioning::read_versioned(&mut core, &course, path)?;
        crate::assets::inspect(
            target
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(invalid)?,
            &bytes,
        )?;
        let text = String::from_utf8(bytes).map_err(|_| invalid())?;
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
    let Some(course) = course(&mut core, scope)? else {
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
        let (bytes, version) = crate::versioning::read_versioned(&mut core, &course, &args.path)?;
        Some((String::from_utf8(bytes).map_err(|_| invalid())?, version))
    } else {
        None
    };
    if let Some((body, version)) = &baseline {
        if read_result(scope, &course, &args.path, body, version)?["truncated"] == true {
            return Err(CoreError::new(
                "TOOL_LIMIT",
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
        crate::assets::inspect(
            target
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(invalid)?,
            args.content.as_bytes(),
        )?;
        args.content.clone()
    };
    let operation = ledger
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    let published_version = if doc.is_none() {
        Some(crate::versioning::write_version(
            &mut core,
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
