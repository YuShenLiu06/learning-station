//! Course routing for the existing authorized read/write tools. The private
//! receipt ledger and Bash cwd remain session-scoped.
use super::*;
use crate::{app::Core, file_metadata::FileAddr};

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
    if !valid_relative_path(path) {
        return Err(invalid());
    }
    let root =
        crate::course_project::course_project_root(core.content.root(), course)?.join("materials");
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

fn addr(course: &str, path: &str) -> FileAddr {
    FileAddr {
        id: String::new(),
        path: path.into(),
        kind: "text".into(),
        course: Some(course.into()),
    }
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
        let bytes = crate::assets::bounded_read(&target, MAX_WORKSPACE_BYTES as usize)?;
        crate::assets::inspect(
            target
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(invalid)?,
            &bytes,
        )?;
        let text = String::from_utf8(bytes).map_err(|_| invalid())?;
        let version = crate::versioning::version_hash(&mut core, &addr(&course, path))?;
        (text, version)
    };
    let (content, truncated) = bounded_text(&text, MAX_READ_BYTES);
    Ok(Some(
        json!({"sessionId":scope.session_id,"courseId":course,"path":path,
        "version":version,"bytes":text.len(),"content":content,"truncated":truncated}),
    ))
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
    let pending = ledger.with_extension("pending");
    if pending.exists() {
        return Err(CoreError::new(
            "NATIVE_TOOL_UNKNOWN",
            "原写入结果尚不可核实；未重复执行。",
        ));
    }
    let target = target(&core, &course, &args.path)?;
    let doc = document(&core, scope, &course, &args.path)?;
    let existing = fs::symlink_metadata(&target).ok();
    if existing.as_ref().is_some_and(|m| !m.is_file()) {
        return Err(invalid());
    }
    let current_version = if let Some(doc) = &doc {
        Some(doc.version.clone())
    } else if existing.is_some() {
        Some(crate::versioning::version_hash(
            &mut core,
            &addr(&course, &args.path),
        )?)
    } else {
        None
    };
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
    // Course files use the existing per-document / material validation above.
    // The 2 MiB scratch-workspace quota must not count the user's whole course.
    // Durable intent closes the write→receipt interruption gap. Unknown writes
    // are never replayed; recovery reads the original completed ledger only.
    fs::create_dir_all(ledger.parent().ok_or_else(invalid)?).map_err(io)?;
    crate::app::atomic_json(
        &pending,
        &json!({"path":args.path,"expectedVersion":args.expected_version}),
    )?;
    let version = if let Some(doc) = doc {
        core.content.save(&doc.id, &content, &doc.version)?.version
    } else {
        let parent = target.parent().ok_or_else(invalid)?;
        fs::create_dir_all(parent).map_err(io)?;
        // Recheck after parent creation and immediately before the write.
        let checked = self::target(&core, &course, &args.path)?;
        if checked != target {
            return Err(out_of_scope());
        }
        if current_version.is_some()
            && crate::versioning::version_hash(&mut core, &addr(&course, &args.path))?
                != current_version.clone().unwrap()
        {
            return Err(conflict());
        }
        if current_version.is_none() && target.exists() {
            return Err(conflict());
        }
        write_atomic(&target, content.as_bytes())?;
        fs::File::open(parent)
            .and_then(|d| d.sync_all())
            .map_err(io)?;
        crate::versioning::version_hash(&mut core, &addr(&course, &args.path))?
    };
    Ok(Some(
        json!({"persisted":true,"sessionId":scope.session_id,"courseId":course,
        "path":args.path,"version":version,"bytes":content.len(),"overwritten":existing.is_some()}),
    ))
}
