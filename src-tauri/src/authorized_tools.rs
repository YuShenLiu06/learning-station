//! User-authorized workspace tools (read/write/bash). Every call is validated
//! against the selected tier, the real session, and a per-session workspace
//! under the app state directory; receipts are durably ledgered for replay.
use crate::learning_tools::ToolScope;
use crate::types::{hash, timestamp, CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Hard ceiling on files a session workspace may accumulate.
pub const MAX_WORKSPACE_FILES: usize = 256;
/// Total byte ceiling for one session workspace.
pub const MAX_WORKSPACE_BYTES: u64 = 2 * 1024 * 1024;
/// Same tool-argument transport budget as the core learning tools.
const MAX_TOOL_INPUT_BYTES: usize = 32 * 1024;
const DEFAULT_BASH_TIMEOUT_MS: u64 = 10_000;
const MAX_BASH_TIMEOUT_MS: u64 = 30_000;
const MIN_BASH_TIMEOUT_MS: u64 = 1_000;
/// Per-stream output cap; keeps the worst escaped JSON reply below 32 KiB.
const MAX_STREAM_BYTES: usize = 2_400;
/// Upper bound for one read result before the final JSON fitting pass.
const MAX_READ_BYTES: usize = 20_000;
const MAX_RESULT_BYTES: usize = 30_000;
const MAX_COMMAND_CHARS: usize = 4_000;
const MAX_CONTENT_CHARS: usize = 20_000;
const TIERS: [&str; 4] = ["off", "read", "write", "bash"];

fn tier_rank(tier: &str) -> usize {
    TIERS.iter().position(|item| *item == tier).unwrap_or(0)
}
pub(crate) fn required_rank(name: &str) -> Option<usize> {
    match name {
        "read" => Some(1),
        "write" => Some(2),
        "bash" => Some(3),
        _ => None,
    }
}

/// Recovery reads the durable receipt ledger by the original stable operation
/// ID; it never re-executes a write or command.
pub(crate) fn recover(
    engine: &crate::app::Engine,
    session_id: &str,
    run_id: &str,
    call: &crate::model::ToolCall,
) -> Result<Value> {
    let unknown = || {
        CoreError::new("NATIVE_TOOL_UNKNOWN", "原工具结果尚不可核实；未重复执行。")
    };
    if !crate::types::valid_id(session_id)
        || !crate::types::valid_id(run_id)
        || required_rank(&call.name).is_none()
    {
        return Err(unknown());
    }
    let (state_dir, session) = {
        let mut core = engine.lock().map_err(|_| unknown())?;
        let session = crate::sessions::get(&mut core.content, session_id).map_err(|_| unknown())?;
        (core.state_dir.clone(), session)
    };
    let scope = ToolScope {
        session_id: session_id.into(),
        section_id: String::new(),
        run_id: run_id.into(),
        allowed_ids: vec![],
    };
    let (request_id, _) = crate::learning_tools::operation_id(engine, &scope, &call.id, &call.name, &call.args)
        .map_err(|_| unknown())?;
    let ledger_path = workspace_root(&state_dir, session_id)
        .join("receipts")
        .join(format!("{request_id}.json"));
    let entry = read_ledger(&ledger_path).map_err(|_| course_write_unknown(engine, session_id, &ledger_path))?;
    let arguments_hash = hash(&serde_json::to_string(&call.args).map_err(|_| unknown())?);
    if entry.name != call.name || entry.arguments_hash != arguments_hash {
        // A ledger exists but for different arguments: that is a conflict with
        // committed evidence, not an unknown outcome.
        return Err(CoreError::new(
            "CONFLICT",
            "原工具记录与本次参数不一致，未重复执行。",
        ));
    }
    if call.name == "read" {
        let run = crate::generation::get(engine, run_id)?;
        let read_scope = ToolScope {
            session_id: session_id.into(),
            section_id: run.source_document_id.clone().unwrap_or_else(|| {
                if session.course_id.is_none() { session.section_id.clone() } else { String::new() }
            }),
            run_id: run_id.into(),
            allowed_ids: run.read_scope_manifest().iter().map(|item| item.id.clone()).collect(),
        };
        validate_read_replay(engine, &read_scope, &call.args, &entry.result)?;
    }
    let mut result = proposal_result(engine, entry.result)?;
    if result["status"] == "pending" {
        return Err(CoreError::new("NATIVE_TOOL_UNKNOWN", "建议仍待用户决定，正文未修改；请先审阅后核对原请求。"));
    }
    result["recovered"] = json!(true);
    Ok(result)
}

/// Upper bound of receipt summaries one listing returns.
const MAX_RECEIPT_SUMMARIES: usize = 200;

/// Read-only summary of the durable write/bash receipt ledger of one session's
/// workspace. Exposes only the stable request ID, tool name, status, timestamp
/// and the write's relative path; never arguments, command text or file
/// contents. The workspace is derived from the real session, never a path.
pub(crate) fn list_receipt_summaries(
    engine: &crate::app::Engine,
    session_id: &str,
) -> Result<Value> {
    if !crate::types::valid_id(session_id) {
        return Err(out_of_scope());
    }
    let state_dir = {
        let mut core = engine.lock()?;
        // Only a real session's own workspace is listable.
        crate::sessions::get(&mut core.content, session_id)?;
        core.state_dir.clone()
    };
    let receipts_dir = workspace_root(&state_dir, session_id).join("receipts");
    let directory = match fs::read_dir(&receipts_dir) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // No ledgered tool receipts yet is an honest empty list.
            return Ok(json!({"sessionId": session_id, "receipts": [], "truncated": false}));
        }
        Err(error) => return Err(io(error)),
    };
    let mut summaries = Vec::new();
    for file in directory {
        let file = file.map_err(io)?;
        let name = file.file_name();
        let Some(request_id) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .map(str::to_owned)
        else {
            continue;
        };
        // Structural failures are skipped: an untrusted file must never reach
        // the UI, and one bad file must not hide the rest of the audit trail.
        let Ok(entry) = read_ledger(&file.path()) else {
            continue;
        };
        if !matches!(entry.name.as_str(), "write" | "bash")
            || !crate::types::valid_id(&request_id)
        {
            continue;
        }
        let status = match entry.name.as_str() {
            "write" => {
                if entry.result["persisted"] == json!(true) {
                    "persisted"
                } else {
                    "unverified"
                }
            }
            _ if entry.result["timedOut"] == json!(true) => "timed-out",
            _ => "completed",
        };
        let path = entry
            .result["path"]
            .as_str()
            .filter(|path| valid_relative_path(path) || crate::material_git::check_path(path).is_ok());
        summaries.push(json!({
            "requestId": request_id,
            "tool": entry.name,
            "status": status,
            "createdAt": entry.created_at,
            "path": path,
        }));
    }
    summaries.sort_by(|a, b| {
        let key = |value: &Value| {
            (
                value["createdAt"].as_str().unwrap_or("").to_owned(),
                value["requestId"].as_str().unwrap_or("").to_owned(),
            )
        };
        key(b).cmp(&key(a))
    });
    let truncated = summaries.len() > MAX_RECEIPT_SUMMARIES;
    summaries.truncate(MAX_RECEIPT_SUMMARIES);
    Ok(json!({"sessionId": session_id, "receipts": summaries, "truncated": truncated}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ReadArgs {
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    path: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WriteArgs {
    path: String,
    content: String,
    #[serde(default)]
    expected_version: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BashArgs {
    command: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LedgerEntry {
    name: String,
    arguments_hash: String,
    result: Value,
    created_at: String,
}

/// Entry point for the user-authorized workspace tools. Tier, session, paths
/// and outputs are validated here; durable receipts are ledgered per operation.
pub(crate) fn execute(
    engine: &crate::app::Engine,
    scope: &ToolScope,
    call_id: &str,
    name: &str,
    args: Value,
) -> Result<Value> {
    // Same invariants the core learning tools enforce before any dispatch.
    validate_scope(scope, call_id)?;
    if serde_json::to_vec(&args)?.len() > MAX_TOOL_INPUT_BYTES {
        return Err(limit());
    }
    let Some(required) = required_rank(name) else {
        return Err(not_allowed());
    };
    let run = crate::generation::get(engine, &scope.run_id)?;
    if run.session_id != scope.session_id || run.context_snapshot_version != 1
        || run.source_document_id.as_ref().is_some_and(|id| id != &scope.section_id)
        || !scope.section_id.is_empty() && !run.read_scope_manifest().iter().any(|m| m.id == scope.section_id)
        || scope.allowed_ids.iter().any(|id| !run.read_scope_manifest().iter().any(|m| &m.id == id)) { return Err(out_of_scope()); }
    let state_dir = {
        let mut core = engine.lock()?;
        let session = crate::sessions::get(&mut core.content, &scope.session_id)?;
        if crate::course_sessions::source(&core.content, &session, &scope.section_id).map_err(|_| out_of_scope())? != scope.section_id {
            return Err(out_of_scope());
        }
        core.state_dir.clone()
    };
    let tier = engine.ui_preferences()?.tool_authorization_tier;
    if tier_rank(&tier) < required {
        return Err(not_allowed());
    }
    let (request_id, _) =
        crate::learning_tools::operation_id(engine, scope, call_id, name, &args)?;
    let arguments_hash = hash(&serde_json::to_string(&args)?);
    let receipts = workspace_root(&state_dir, &scope.session_id).join("receipts");
    let ledger_path = receipts.join(format!("{request_id}.json"));
    if let Ok(entry) = read_ledger(&ledger_path) {
        if entry.name != name || entry.arguments_hash != arguments_hash {
            return Err(CoreError::new(
                "CONFLICT",
                "原工具记录与本次参数不一致，未重复执行。",
            ));
        }
        if name == "read" {
            validate_read_replay(engine, scope, &args, &entry.result)?;
        }
        let mut result = proposal_result(engine, entry.result)?;
        result["replayed"] = json!(true);
        return Ok(result);
    }
    // Direct course writes share the same revocation boundary as accepting a
    // proposal. Acquire only after replay (which may itself settle a proposal).
    let _authorization = if name == "write" {
        Some(crate::app::UI_PREFERENCE_WRITE.lock().map_err(|_| CoreError::new("BUSY", "界面设置暂不可用。"))?)
    } else { None };
    if name == "write" && tier_rank(&engine.ui_preferences()?.tool_authorization_tier) < required {
        return Err(not_allowed());
    }
    let outcome = (|| {
    let mut result = match name {
        "read" => execute_read(engine, scope, decode::<ReadArgs>(args)?),
        "write" => execute_write(engine, &state_dir, scope, decode::<WriteArgs>(args)?, &ledger_path, call_id),
        "bash" => execute_bash(&state_dir, scope, decode::<BashArgs>(args)?),
        _ => return Err(not_allowed()),
    }?;
    fit_result_bytes(&mut result)?;
    let entry = LedgerEntry {
        name: name.into(),
        arguments_hash,
        result: result.clone(),
        created_at: timestamp(),
    };
    fs::create_dir_all(&receipts).map_err(io)?;
    if ledger_path.with_extension("pending").exists() {
        crate::course_write_transaction::durable_json(&ledger_path, &entry)?;
    } else {
        crate::app::atomic_json(&ledger_path, &entry)?;
    }
    Ok(result)
    })();
    outcome.map_err(|error| {
        if name == "write" && ledger_path.with_extension("pending").exists() {
            course_write_unknown(engine, &scope.session_id, &ledger_path)
        } else { error }
    })
}

/// Fixed diagnostics only: never forward arbitrary messages, paths, or content
/// from tool errors into model transport or activity cards.
pub(crate) fn write_conflict_feedback(error: &CoreError) -> Option<Value> {
    if error.code != "CONFLICT" { return None; }
    let details = error.details.as_ref()?;
    match details["reason"].as_str()? {
        "write_version_mismatch" => Some(json!({"message":"文件版本不匹配，本次未写入；请重新读取并使用 read 返回的 version，不要使用 Focus 的 Git OID。",
            "details":{"reason":"write_version_mismatch"}})),
        "edit_review_pending" => {
            let proposal = details["proposalId"].as_str().filter(|id| crate::types::valid_id(id))?;
            let course = details["courseId"].as_str().filter(|id| crate::types::valid_id(id))?;
            let status = details["status"].as_str()?;
            let message = match status {
                "pending" => "已有修改建议待审阅，本次未写入；请先接受或拒绝旧建议，再重新读取后修改。",
                "accepting" | "unknown" => "旧建议结果尚未核实，本次未写入；请在学习站核对原建议，不要重复写入。",
                _ => return None,
            };
            Some(json!({"message":message,"details":{"reason":"edit_review_pending","proposalId":proposal,"courseId":course,"status":status}}))
        }
        _ => None,
    }
}

fn course_write_unknown(engine: &crate::app::Engine, session_id: &str, ledger: &Path) -> CoreError {
    let mut error = CoreError::new(
        "NATIVE_TOOL_UNKNOWN",
        "写入结果需核对恢复目录；未重复执行。请保留原请求、旧稿与新稿。",
    );
    let inspect = (|| -> Result<Value> {
        let pending = crate::assets::bounded_read(&ledger.with_extension("pending"), 16 * 1024)?;
        let pending: Value = serde_json::from_slice(&pending)?;
        let course = pending["courseId"].as_str().ok_or_else(invalid)?;
        let path = pending["path"].as_str().ok_or_else(invalid)?;
        if !crate::types::valid_id(course) {
            return Err(invalid());
        }
        let operation = ledger
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(invalid)?;
        let mut core = engine.lock()?;
        let session = crate::sessions::get(&mut core.content, session_id)?;
        let owner = match session.course_id {
            Some(id) => Some(id),
            None => match crate::course_sessions::course(&core.content, &session.section_id) {
                Ok(owner) => owner,
                Err(_)
                    if crate::types::valid_id(&session.section_id)
                        && pending["sessionId"] == session.id
                        && pending["sectionId"] == session.section_id
                        && path == format!("documents/{}.md", session.section_id)
                        && !core.content.is_trashed(&session.section_id)
                        && fs::symlink_metadata(core.content.root().join(format!(
                            "courses/{course}/materials/documents/{}.md",
                            session.section_id
                        )))
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                {
                    Some(course.to_owned())
                }
                Err(error) => return Err(error),
            },
        };
        if owner.as_deref() != Some(course) {
            return Err(out_of_scope());
        }
        if core.content.is_trashed(session_id) || core.content.is_trashed(course) {
            return Err(out_of_scope());
        }
        if core.content.read(course)?.metadata.kind != "course" {
            return Err(out_of_scope());
        }
        crate::material_git::check_path(path)?;
        let relative = format!("courses/{course}/materials/{path}");
        Ok(crate::course_write_transaction::inspect(core.content.root(),&relative,operation)
            .unwrap_or_else(|_|json!({"recoveryPath":crate::course_write_transaction::recovery_path(&relative,operation),"recovery":"inspect-only","state":"unverified"})))
    })();
    if let Ok(details) = inspect {
        if let Some(path) = details["recoveryPath"].as_str() {
            error.message = format!("写入结果需核对，未重复执行。恢复目录（相对资料库）：{path}；请保留 old/new 并先比较现有目标，不覆盖第三份文件。");
        }
        error.details = Some(details);
    }
    error
}

/// Receipts prove an earlier read, not continuing permission to disclose it.
/// Reuse the live read validator, then require the receipt's version to match
/// this run's frozen version as well as the currently visible source.
pub(crate) fn validate_read_replay(
    engine: &crate::app::Engine,
    scope: &ToolScope,
    args: &Value,
    receipt: &Value,
) -> Result<()> {
    if tier_rank(&engine.ui_preferences()?.tool_authorization_tier) < 1 {
        return Err(not_allowed());
    }
    let mut current = execute_read(engine, scope, decode::<ReadArgs>(args.clone())?)?;
    fit_result_bytes(&mut current)?;
    let unchanged = if args.get("sourceId").is_some() {
        current["sourceId"] == receipt["sourceId"] && current["version"] == receipt["version"]
    } else {
        ["sessionId", "path", "version", "bytes", "content", "truncated"].iter().all(|key| current[key] == receipt[key])
    };
    if !unchanged {
        return Err(CoreError::new("CONFLICT", "原读取回执与本轮资料版本不一致，请保留原记录后重新提问。"));
    }
    Ok(())
}

fn execute_read(engine: &crate::app::Engine, scope: &ToolScope, args: ReadArgs) -> Result<Value> {
    let exclusive = args.source_id.is_some() as u8 + args.path.is_some() as u8;
    if exclusive != 1 {
        return Err(invalid());
    }
    if let Some(source_id) = args.source_id {
        if !crate::types::valid_id(&source_id) {
            return Err(out_of_scope());
        }
        let mut allowed: std::collections::BTreeSet<&str> =
            scope.allowed_ids.iter().map(String::as_str).collect();
        allowed.insert(&scope.section_id);
        if !allowed.contains(source_id.as_str()) {
            return Err(out_of_scope());
        }
        // Authorization/version scope is frozen separately from automatic context.
        let run = crate::generation::get(engine, &scope.run_id)?;
        if run.session_id != scope.session_id || run.context_snapshot_version != 1 {
            return Err(out_of_scope());
        }
        let item = run
            .read_scope_manifest()
            .iter()
            .find(|item| item.id == source_id)
            .ok_or_else(out_of_scope)?;
        let document = {
            let mut core = engine.lock()?;
            let session = crate::sessions::get(&mut core.content, &scope.session_id).map_err(|_| out_of_scope())?;
            if core.content.is_trashed(&scope.session_id) { return Err(out_of_scope()); }
            if let Some(focus) = &run.focus {
                let frozen = crate::focus::load(&mut core, &session, focus)?;
                if let Some(path) = frozen.asset_path(&source_id) {
                    let asset = crate::assets::list(&core.content, &focus.course_id)?.into_iter().find(|a| a.id == source_id).ok_or_else(out_of_scope)?;
                    if !matches!(asset.mime.as_str(), "text/plain" | "text/markdown" | "text/csv" | "application/json") {
                        return Err(CoreError::new("UNSUPPORTED_TYPE", "该附件没有受支持的文本正文；图片/PDF仍只提供元数据。"));
                    }
                    let bytes = crate::focus::open_materials(&core, focus)?.read(&focus.course_id, &focus.commit_hash, path)?;
                    let text = std::str::from_utf8(&bytes).map_err(|_| CoreError::new("UNSUPPORTED_TYPE", "该附件没有受支持的文本正文。"))?;
                    if hash(text) != item.version { return Err(CoreError::new("CONFLICT", "资料与本轮固定版本不一致。")); }
                    let (content, truncated) = bounded_text(text, MAX_READ_BYTES);
                    let chars = content.chars().count();
                    let included = run.context_manifest.iter().any(|m| m.id == source_id && m.version == item.version && m.included_chars > 0);
                    return Ok(json!({"sessionId":scope.session_id,"sourceId":source_id,"path":path,"title":path.rsplit('/').next(),"kind":"asset","version":item.version,"includedChars":chars,"content":content,"truncated":truncated,"manifestRole":"user-reference","includedInInitialContext":included,"authorizationScope":if run.course_read_manifest.is_some(){"course-read-manifest"}else{"run-context-manifest"},"delivery":"tool-result","range":{"startChar":0,"endChar":chars,"complete":!truncated,"totalBytes":bytes.len()}}));
                }
                crate::course_sessions::source(&core.content, &session, &source_id).map_err(|_| out_of_scope())?;
                crate::focus::read_document(&core, focus, &source_id)?
            } else {
                if session.course_id.is_some() && crate::course_sessions::source(&core.content, &session, &source_id).map_err(|_| out_of_scope())? != source_id { return Err(out_of_scope()); }
                core.content.read(&source_id)?
            }
        };
        if document.version != item.version {
            return Err(CoreError::new(
                "CONFLICT",
                "关联资料已在读取期间更新，请重新读取。",
            ));
        }
        let content = bounded_text(&document.content, MAX_READ_BYTES);
        let included_chars = content.0.chars().count();
        let included_in_initial_context = run.context_manifest.iter().any(|context_item| {
            context_item.id == source_id && context_item.version == item.version && context_item.included_chars > 0
        });
        Ok(json!({"sessionId":scope.session_id,"sourceId":source_id,"path":document.path,
            "title":document.metadata.title,"kind":document.metadata.kind,"version":document.version,
            "includedChars":included_chars,"content":content.0,"truncated":content.1,
            "manifestRole":"user-reference","includedInInitialContext":included_in_initial_context,
            "authorizationScope":if run.course_read_manifest.is_some() {"course-read-manifest"} else {"run-context-manifest"},
            "delivery":"tool-result","range":{"startChar":0,"endChar":included_chars,"complete":!content.1,"totalBytes":document.content.len()}}),)
    } else {
        let path = args.path.as_deref().ok_or_else(invalid)?;
        if let Some(result) = course_read(engine, scope, path)? { return Ok(result); }
        if !valid_relative_path(path) { return Err(invalid()); }
        let files_root = ensure_workspace_files(engine, scope)?;
        let target = resolve_existing(&files_root, path)?;
        let bytes = fs::read(&target).map_err(io)?;
        let content = bounded_text(&String::from_utf8_lossy(&bytes), MAX_READ_BYTES);
        Ok(json!({"sessionId":scope.session_id,"path":path,"bytes":bytes.len(),
            "content":content.0,"truncated":content.1}))
    }
}

#[path = "course_workspace.rs"]
mod course_workspace;
use course_workspace::{course_read, course_write};
pub(crate) use course_workspace::{proposal_list, proposal_get, proposal_decide, proposal_result, exercise_proposal_decide};

fn execute_write(
    engine: &crate::app::Engine,
    state_dir: &Path,
    scope: &ToolScope,
    args: WriteArgs,
    ledger_path: &Path,
    call_id: &str,
) -> Result<Value> {
    if args.content.chars().count() > MAX_CONTENT_CHARS {
        return Err(invalid());
    }
    if let Some(result) = course_write(engine, scope, &args, ledger_path, call_id)? {
        return Ok(result);
    }
    if !valid_relative_path(&args.path) { return Err(invalid()); }
    let files_root = ensure_workspace_files_at(state_dir, &scope.session_id)?;
    let target = resolve_for_write(&files_root, &args.path)?;
    let (count, bytes) = workspace_usage(&files_root)?;
    let existing = fs::symlink_metadata(&target).map(|meta| meta.len()).ok();
    let new_bytes = args.content.len() as u64;
    if count + usize::from(existing.is_none()) > MAX_WORKSPACE_FILES
        || bytes - existing.unwrap_or(0) + new_bytes > MAX_WORKSPACE_BYTES
    {
        return Err(limit());
    }
    let overwritten = existing.is_some();
    write_atomic(&target, args.content.as_bytes())?;
    Ok(json!({"persisted":true,"sessionId":scope.session_id,"path":args.path,
        "bytes":args.content.len(),"overwritten":overwritten}))
}

fn execute_bash(state_dir: &Path, scope: &ToolScope, args: BashArgs) -> Result<Value> {
    if args.command.trim().is_empty()
        || args.command.chars().count() > MAX_COMMAND_CHARS
        || args
            .timeout_ms
            .is_some_and(|ms| !(MIN_BASH_TIMEOUT_MS..=MAX_BASH_TIMEOUT_MS).contains(&ms))
    {
        return Err(invalid());
    }
    let files_root = ensure_workspace_files_at(state_dir, &scope.session_id)?;
    let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(DEFAULT_BASH_TIMEOUT_MS));
    let mut command = std::process::Command::new("/bin/bash");
    command
        .arg("-c")
        .arg(&args.command)
        .current_dir(&files_root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "en_US.UTF-8")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group so a timeout can stop spawned grandchildren too.
        command.process_group(0);
    }
    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|_| CoreError::new("TOOL_FAILED", "无法启动受控命令执行。"))?;
    let pid = child.id();
    let mut stdout_pipe = child.stdout.take().ok_or_else(runtime)?;
    let mut stderr_pipe = child.stderr.take().ok_or_else(runtime)?;
    let stdout_reader =
        std::thread::spawn(move || read_bounded(&mut stdout_pipe, MAX_STREAM_BYTES));
    let stderr_reader =
        std::thread::spawn(move || read_bounded(&mut stderr_pipe, MAX_STREAM_BYTES));
    let deadline = started + timeout;
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(io)? {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = std::process::Command::new("/bin/kill")
                .args(["-9", &format!("-{pid}")])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            break child.wait().map_err(io)?;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stdout_truncated) = stdout_reader.join().map_err(|_| runtime())?;
    let (stderr, stderr_truncated) = stderr_reader.join().map_err(|_| runtime())?;
    let exit_code = if timed_out { None } else { status.code() };
    Ok(json!({"persisted":true,"sessionId":scope.session_id,
        "exitCode":exit_code,"timedOut":timed_out,
        "stdout":String::from_utf8_lossy(&stdout),"stderr":String::from_utf8_lossy(&stderr),
        "truncated":stdout_truncated || stderr_truncated,
        "durationMs":started.elapsed().as_millis() as u64}))
}

fn ensure_workspace_files(engine: &crate::app::Engine, scope: &ToolScope) -> Result<PathBuf> {
    let state_dir = engine.lock()?.state_dir.clone();
    ensure_workspace_files_at(&state_dir, &scope.session_id)
}

fn ensure_workspace_files_at(state_dir: &Path, session_id: &str) -> Result<PathBuf> {
    if !crate::types::valid_id(session_id) {
        return Err(invalid());
    }
    let files = workspace_root(state_dir, session_id).join("files");
    fs::create_dir_all(&files).map_err(io)?;
    let meta = fs::symlink_metadata(&files).map_err(io)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(runtime());
    }
    let canonical_files = fs::canonicalize(&files).map_err(io)?;
    let canonical_state = fs::canonicalize(state_dir).map_err(io)?;
    if !canonical_files.starts_with(&canonical_state) {
        return Err(runtime());
    }
    Ok(canonical_files)
}

fn workspace_root(state_dir: &Path, session_id: &str) -> PathBuf {
    state_dir.join("workspaces").join(session_id)
}

fn valid_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 128
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains("//")
        && path.split('/').count() <= 4
        && path.split('/').all(|segment| {
            let mut chars = segment.chars();
            !segment.is_empty()
                && segment.len() <= 64
                && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        })
}

fn resolve_existing(files_root: &Path, path: &str) -> Result<PathBuf> {
    let target = files_root.join(path);
    let meta = match fs::symlink_metadata(&target) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CoreError::new("NOT_FOUND", "工作区中没有这个文件。"))
        }
        Err(error) => return Err(io(error)),
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(invalid());
    }
    let canonical = fs::canonicalize(&target).map_err(io)?;
    if !canonical.starts_with(files_root) {
        return Err(invalid());
    }
    Ok(canonical)
}

fn resolve_for_write(files_root: &Path, path: &str) -> Result<PathBuf> {
    let target = files_root.join(path);
    if let Ok(meta) = fs::symlink_metadata(&target) {
        if meta.file_type().is_symlink() {
            return Err(invalid());
        }
        if !meta.is_file() {
            return Err(invalid());
        }
    }
    let parent = target.parent().ok_or_else(invalid)?;
    fs::create_dir_all(parent).map_err(io)?;
    let canonical_parent = fs::canonicalize(parent).map_err(io)?;
    if !canonical_parent.starts_with(files_root) {
        return Err(invalid());
    }
    Ok(canonical_parent.join(target.file_name().ok_or_else(invalid)?))
}

fn workspace_usage(files_root: &Path) -> Result<(usize, u64)> {
    fn walk(directory: &Path, depth: usize, usage: &mut (usize, u64)) -> Result<()> {
        if depth > 8 || usage.0 > 8_192 {
            return Ok(());
        }
        for entry in fs::read_dir(directory).map_err(io)? {
            let entry = entry.map_err(io)?;
            let meta = fs::symlink_metadata(entry.path()).map_err(io)?;
            if meta.is_dir() {
                walk(&entry.path(), depth + 1, usage)?;
            } else {
                usage.0 += 1;
                usage.1 += meta.len();
            }
        }
        Ok(())
    }
    let mut usage = (0, 0);
    walk(files_root, 0, &mut usage)?;
    Ok(usage)
}

fn write_atomic(target: &Path, bytes: &[u8]) -> Result<()> {
    let parent = target.parent().ok_or_else(invalid)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(io)?;
    temp.write_all(bytes).map_err(io)?;
    temp.as_file().sync_all().map_err(io)?;
    temp.persist(target).map_err(|error| io(error.error))?;
    Ok(())
}

fn read_bounded(pipe: &mut impl Read, cap: usize) -> (Vec<u8>, bool) {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4_096];
    let mut truncated = false;
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if data.len() + read > cap {
                    let room = cap - data.len();
                    let boundary = char_boundary(&buffer[..room], room);
                    data.extend_from_slice(&buffer[..boundary]);
                    truncated = true;
                    break;
                }
                data.extend_from_slice(&buffer[..read]);
            }
            Err(_) => break,
        }
    }
    (data, truncated)
}

fn char_boundary(bytes: &[u8], mut end: usize) -> usize {
    while end > 0 && end <bytes.len() && (bytes[end] & 0xC0) == 0x80 {end -= 1;
    }
    end.min(bytes.len())
}

fn bounded_text(value: &str, cap: usize) -> (String, bool) {
    if value.len() <= cap {
        return (value.to_owned(), false);
    }
    (value[..char_boundary(value.as_bytes(), cap)].to_owned(), true,)
}

/// Final guard: keep the serialized reply below the transport budget by
/// truncating long string fields on the Unicode boundary.
fn fit_result_bytes(result: &mut Value) -> Result<()> {
    let mut content_was_truncated = false;
    for _ in 0..8 {
        if serde_json::to_vec(result)?.len() <= MAX_RESULT_BYTES {
            if let Some(content_chars) = result
                .get("content")
                .and_then(Value::as_str)
                .map(|content| content.chars().count())
            {
                if result.get("includedChars").is_some() {
                    result["includedChars"] = json!(content_chars);
                }
                if content_was_truncated {
                    result["truncated"] = json!(true);
                }
                if result.get("range").is_some() {
                    result["range"]["endChar"] = json!(content_chars);
                    result["range"]["complete"] =
                        json!(!result["truncated"].as_bool().unwrap_or(false));
                }
            }
            return Ok(());
        }
        let mut changed = false;
        if let Some(object) = result.as_object_mut() {
            for (key, value) in object.iter_mut() {
                if let Some(text) = value.as_str() {
                    if text.len() > 512 {
                        let (cut, _) = bounded_text(text, text.len() / 2);
                        *value = Value::String(cut);
                        if key == "content" {
                            content_was_truncated = true;
                        }
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return Err(limit());
        }
    }
    Err(limit())
}

fn read_ledger(path: &Path) -> Result<LedgerEntry> {
    let meta = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CoreError::new("NOT_FOUND", "没有原工具回执。")
        } else {
            io(error)
        }
    })?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > 1024 * 1024 {
        return Err(runtime());
    }
    serde_json::from_slice(&fs::read(path).map_err(io)?).map_err(|_| runtime())
}

fn decode<T: serde::de::DeserializeOwned>(args: Value) -> Result<T> {
    serde_json::from_value(args).map_err(|_| invalid())
}

fn validate_scope(scope: &ToolScope, call_id: &str) -> Result<()> {
    if !crate::types::valid_id(&scope.session_id)
        || (!scope.section_id.is_empty() && !crate::types::valid_id(&scope.section_id))
        || !crate::types::valid_id(&scope.run_id)
        || scope.allowed_ids.len() > 32
        || scope.allowed_ids.iter().any(|id| !crate::types::valid_id(id))
        || call_id.is_empty()
        || call_id.len() > 128
        || call_id.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    Ok(())
}

fn invalid() -> CoreError {
    CoreError::new(
        "INVALID_TOOL_INPUT",
        "受控工具参数无效、含未知字段或缺少必填内容，未执行。",
    )
}
fn limit() -> CoreError {
    CoreError::new(
        "LIMIT_EXCEEDED",
        "受控工作区超过文件或容量限额，未执行额外写入。",
    )
}
fn out_of_scope() -> CoreError {
    CoreError::new(
        "TOOL_SCOPE",
        "受控工具只能访问当前会话工作区和本轮已授权来源。",
    )
}
fn not_allowed() -> CoreError {
    CoreError::new(
        "TOOL_NOT_ALLOWED",
        "未启用此工具；需要在设置中提高工具授权档位，默认关闭。",
    )
}
fn runtime() -> CoreError {
    CoreError::new("IO", "受控工作区状态无效，未执行。")
}
fn io(error: std::io::Error) -> CoreError {
    CoreError::io(error)
}

#[cfg(test)]
mod tests {
    use crate::app::{Engine, UiPreferences};
    use crate::learning_tools::ToolScope;
    use crate::types::CoreError;
    use serde_json::{json, Value};
    use std::io::Write;

    fn fixture(tier: &str) -> (tempfile::TempDir, Arc<Engine>, ToolScope) {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(directory.path().join("library"), directory.path().join("state"))
            .unwrap();
        engine
            .save_ui_preferences(UiPreferences {
                tool_authorization_tier: tier.into(),
                ..UiPreferences::default()
            })
            .unwrap();
        let scope = {
            let mut core = engine.lock().unwrap();
            let section = core
                .create_section("当前课程", "Full lesson body about since and for.", vec![], vec![])
                .unwrap();
            let (session, context) = core.start_session(&section.id, false).unwrap();
            // A frozen run manifest is the read tool's source of authorization.
            let run: crate::generation::Run = serde_json::from_value(json!({
                "id":"run_authorized_fixture","sessionId":session.id,"mode":"question","status":"succeeded",
                "content":"","error":null,"createdAt":"2026-10-17T00:00:00Z","updatedAt":"2026-10-17T00:00:00Z",
                "requestDigest":"fixture","contextSnapshotVersion":1,
                "contextManifest":context.manifest
            }))
            .unwrap();
            crate::app::atomic_json(&core.state_dir.join("runs").join(format!("{}.json", run.id)), &run).unwrap();
            engine.runs.lock().unwrap().insert(
                run.id.clone(),
                crate::generation::ManagedRun {
                    run,
                    cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                },
            );
            ToolScope {
                session_id: session.id,
                section_id: section.id,
                run_id: "run_authorized_fixture".into(),
                allowed_ids: context.manifest.into_iter().map(|item| item.id).collect(),
            }
        };
        (directory, engine, scope)
    }

    fn course_fixture(tier: &str) -> (tempfile::TempDir,Arc<Engine>,ToolScope, String, String) {
        let directory = tempfile::tempdir().unwrap();
        let engine = Engine::new(
            directory.path().join("library"),
            directory.path().join("state"),
        )
        .unwrap();
        engine
            .save_ui_preferences(UiPreferences {
                tool_authorization_tier: tier.into(),
                ..UiPreferences::default()
            })
            .unwrap();
        let (scope, source_id, sibling_id) = {
            let mut core = engine.lock().unwrap();
            let course = crate::library_tree::create(
                &mut core.content,
                "course",
                "Synthetic course",
                None,
                "",
                "authorized-read-course",
            )
            .unwrap();
            let source = crate::library_tree::create(
                &mut core.content,
                "section",
                "Synthetic source",
                Some(&course.id),
                "authorized source body",
                "authorized-read-source",
            )
            .unwrap();
            let sibling_course = crate::library_tree::create(
                &mut core.content,
                "course",
                "Synthetic sibling course",
                None,
                "",
                "authorized-read-sibling-course",
            )
            .unwrap();
            let sibling = crate::library_tree::create(
                &mut core.content,
                "section",
                "Sibling source",
                Some(&sibling_course.id),
                "sibling body must stay private",
                "authorized-read-sibling-source",
            )
            .unwrap();
            let session = crate::course_sessions::create(
                &mut core,
                &course.id,
                "authorized-read-course-session",
            )
            .unwrap();
            let read_manifest = crate::context::course_read_manifest(&core.content, &course.id)
                .unwrap()
                .into_iter()
                .filter(|item| item.id == source.id)
                .collect::<Vec<_>>();
            let run: crate::generation::Run = serde_json::from_value(json!({
                "id":"run_course_read_fixture","sessionId":session.id,"mode":"question","status":"succeeded",
                "content":"","error":null,"createdAt":"2026-10-17T00:00:00Z","updatedAt":"2026-10-17T00:00:00Z",
                "requestDigest":"course-read-fixture","contextSnapshotVersion":1,
                "contextManifest":[],"courseReadManifest":read_manifest
            }))
            .unwrap();
            crate::app::atomic_json(
                &core.state_dir.join("runs").join(format!("{}.json", run.id)),
                &run,
            )
            .unwrap();
            engine.runs.lock().unwrap().insert(
                run.id.clone(),
                crate::generation::ManagedRun {
                    run,
                    cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                },
            );
            (
                ToolScope {
                    session_id: session.id,
                    section_id: String::new(),
                    run_id: "run_course_read_fixture".into(),
                    allowed_ids: vec![source.id.clone()],
                },
                source.id,
                sibling.id,
            )
        };
        (directory, engine, scope, source_id, sibling_id)
    }

    fn run(
        engine: &Arc<Engine>,
        scope: &ToolScope, call: &str, name: &str, args: Value,) -> Result<Value, CoreError> {
        // Existing #64 tests exercise the final publication transaction after
        // explicit acceptance; proposal lifecycle is tested separately below.
        let result = crate::authorized_tools::execute(engine, scope, call, name, args)?;
        if result["status"] == "pending" {
            super::proposal_decide(engine, result["courseId"].as_str().unwrap(), result["proposalId"].as_str().unwrap(), true)
        } else { Ok(result) }
    }

    fn exercise_review_fixture() -> (tempfile::TempDir, Arc<Engine>, ToolScope, String, Value) {
        let (dir, engine, mut scope, source, _) = course_fixture("write");
        let (document, original) = {
            let mut core = engine.lock().unwrap();
            let raw = json!({"summary":"synthetic","exercises":[
                {"question":"A or B?\n\nA. one\nB. two","format":"exact","answers":["A"],"explanation":"one","tags":[],"sourceIds":[]},
                {"question":"Unchanged","format":"exact","answers":["same"],"explanation":"same","tags":[],"sourceIds":[]}
            ]});
            let data = crate::exercises::validate(raw, &[], "review-fixture").unwrap();
            let document = crate::exercises::create(&mut core.content, &scope.session_id, &source, data, "review-exercise-create").unwrap();
            let payload = crate::exercise_edits::payload(&mut core.content, &document.id).unwrap();
            (document, payload)
        };
        scope.allowed_ids.push(document.id.clone());
        let mut runs = engine.runs.lock().unwrap();
        let run = &mut runs.get_mut(&scope.run_id).unwrap().run;
        run.course_read_manifest.as_mut().unwrap().push(crate::types::ManifestItem { id:document.id.clone(), path:document.path, version:document.version, included_chars:0 });
        crate::app::atomic_json(&engine.lock().unwrap().state_dir.join("runs").join(format!("{}.json",run.id)), run).unwrap();
        drop(runs);
        (dir,engine,scope,document.id,original)
    }
    #[test]
    fn exercise_review_persists_whole_questions_replays_and_preserves_historical_grading() {
        let (dir,engine,scope,id,original) = exercise_review_fixture();
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let origin_entry = before.metadata.extra["exerciseEntryId"].as_str().unwrap().to_owned();
        let old_entry_bytes = engine.lock().unwrap().content.read(&origin_entry).unwrap().content;
        let mut suggested = original.clone(); suggested["exercises"][0]["answers"] = json!(["B"]); suggested["exercises"][0]["explanation"] = json!("two");
        let args = json!({"path":format!("documents/{id}.md"),"content":suggested.to_string(),"expectedVersion":before.version});
        let pending = super::execute(&engine,&scope,"exercise-review","write",args.clone()).unwrap();
        let course = pending["courseId"].as_str().unwrap(); let proposal = pending["proposalId"].as_str().unwrap();
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().version,before.version);
        let qid = original["exercises"][0]["id"].as_str().unwrap();
        let result = super::exercise_proposal_decide(&engine,course,proposal,qid,true).unwrap();
        assert_eq!(result["persisted"],true); assert_eq!(result["complete"],true);
        let loaded = crate::exercises::get(&mut engine.lock().unwrap().content,&id).unwrap();
        assert_ne!(loaded.exercises["exercises"][0]["id"],qid);
        assert_eq!(loaded.exercises["exercises"][1],original["exercises"][1]);
        assert_eq!(loaded.exercises["exercises"][0]["answers"],json!(["B"]));
        assert_eq!(engine.lock().unwrap().content.read(&origin_entry).unwrap().content,old_entry_bytes);
        let version = loaded.document.version;
        super::exercise_proposal_decide(&engine,course,proposal,qid,true).unwrap();
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().version,version);
        let replay = super::execute(&engine,&scope,"exercise-review","write",args).unwrap(); assert_eq!(replay["persisted"],true);
        let answer = crate::sessions::append(&mut engine.lock().unwrap().content,&scope.session_id,crate::types::Entry { id:String::new(),kind:"attempt".into(),content:"A".into(),created_at:String::new(),selection:None,related_ids:vec![],data:Some(json!({"exerciseId":qid,"exerciseEntryId":origin_entry,"answer":"A"})) },None,"review-old-answer").unwrap();
        assert_eq!(answer.entries.iter().find(|e|e.kind=="feedback").unwrap().data.as_ref().unwrap()["correct"],true);
        drop(engine);
        let reopened = Engine::new(dir.path().join("library"),dir.path().join("state")).unwrap();
        assert_eq!(crate::exercises::get(&mut reopened.lock().unwrap().content,&id).unwrap().document.version,version);
        assert!(super::proposal_list(&reopened,course).unwrap().as_array().unwrap().is_empty());
        let mut core=reopened.lock().unwrap();
        let preview=crate::trash::preview(&mut core.content,&id).unwrap();
        let trashed=crate::trash::trash(&mut core.content,&id,&preview.version,"review-trash").unwrap();
        assert!(core.content.is_trashed(&origin_entry));
        crate::trash::restore(&mut core.content,&trashed.trash_id,"review-restore").unwrap();
        assert_eq!(crate::exercises::get(&mut core.content,&id).unwrap().document.version,version);

    }
    #[test]
    fn exercise_review_rejection_conflict_scope_and_revocation_leave_original() {
        let (_dir,engine,scope,id,original) = exercise_review_fixture();
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let mut suggested = original.clone(); suggested["exercises"][0]["question"] = json!("changed"); suggested["exercises"].as_array_mut().unwrap().pop();
        let args = json!({"path":format!("documents/{id}.md"),"content":suggested.to_string(),"expectedVersion":before.version});
        let mut outside = scope.clone(); outside.allowed_ids.retain(|x|x!=&id);
        assert!(super::execute(&engine,&outside,"outside-exercise","write",args.clone()).is_err());
        let p = super::execute(&engine,&scope,"exercise-mixed","write",args).unwrap(); let course=p["courseId"].as_str().unwrap(); let proposal=p["proposalId"].as_str().unwrap();
        let first=original["exercises"][0]["id"].as_str().unwrap(); let second=original["exercises"][1]["id"].as_str().unwrap();
        super::exercise_proposal_decide(&engine,course,proposal,first,false).unwrap();
        engine.save_ui_preferences(UiPreferences::default()).unwrap();
        assert_eq!(super::exercise_proposal_decide(&engine,course,proposal,second,true).unwrap_err().code,"TOOL_NOT_ALLOWED");
        engine.save_ui_preferences(UiPreferences {tool_authorization_tier:"write".into(),..UiPreferences::default()}).unwrap();
        let edited=crate::content::serialize(&before.metadata,"local edit").unwrap();
        engine.lock().unwrap().content.save(&id,&edited,&before.version).unwrap();
        assert_eq!(super::exercise_proposal_decide(&engine,course,proposal,second,true).unwrap_err().code,"CONFLICT");
        super::exercise_proposal_decide(&engine,course,proposal,second,false).unwrap();
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().body,"local edit");
    }

    #[test]
    fn exercise_review_partial_recovery_and_count_limit_remain_decidable() {
        let (dir,engine,scope,id,original) = exercise_review_fixture();
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let mut suggested = original.clone(); suggested["exercises"].as_array_mut().unwrap().clear();
        suggested["exercises"].as_array_mut().unwrap().push(json!({"question":"New","format":"exact","answers":["new"],"explanation":"new","tags":[],"sourceIds":[]}));
        let p = super::execute(&engine,&scope,"exercise-replace","write",json!({"path":format!("documents/{id}.md"),"content":suggested.to_string(),"expectedVersion":before.version})).unwrap();
        let course=p["courseId"].as_str().unwrap(); let proposal=p["proposalId"].as_str().unwrap();
        let first=original["exercises"][0]["id"].as_str().unwrap(); let second=original["exercises"][1]["id"].as_str().unwrap();
        let early_path=dir.path().join(format!("state/edit-proposals/{course}/{proposal}.json"));
        let mut early:Value=serde_json::from_slice(&std::fs::read(&early_path).unwrap()).unwrap();
        early["exercise"]["changes"].as_array_mut().unwrap().iter_mut().find(|c|c["id"]==first).unwrap()["status"]=json!("accepting");
        crate::course_write_transaction::durable_json(&early_path,&early).unwrap();
        assert_eq!(super::exercise_proposal_decide(&engine,course,proposal,first,true).unwrap_err().code,"NOT_PUBLISHED");
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().version,before.version);
        super::exercise_proposal_decide(&engine,course,proposal,first,true).unwrap();
        let saved=crate::exercises::get(&mut engine.lock().unwrap().content,&id).unwrap();
        let path=dir.path().join(format!("state/edit-proposals/{course}/{proposal}.json"));
        let mut record:Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        record["exercise"]["changes"].as_array_mut().unwrap().iter_mut().find(|c|c["id"]==first).unwrap()["status"]=json!("accepting");
        crate::course_write_transaction::durable_json(&path,&record).unwrap();
        super::exercise_proposal_decide(&engine,course,proposal,first,true).unwrap();
        assert_eq!(crate::exercises::get(&mut engine.lock().unwrap().content,&id).unwrap().document.version,saved.document.version);
        assert_eq!(super::exercise_proposal_decide(&engine,course,proposal,second,true).unwrap_err().code,"INVALID_INPUT");
        assert_eq!(super::proposal_get(&engine,course,proposal).unwrap()["exercise"]["changes"].as_array().unwrap().iter().find(|c|c["id"]==second).unwrap()["status"],"pending");
        super::exercise_proposal_decide(&engine,course,proposal,"new_0",true).unwrap();
        super::exercise_proposal_decide(&engine,course,proposal,second,true).unwrap();
        let loaded=crate::exercises::get(&mut engine.lock().unwrap().content,&id).unwrap();
        assert_eq!(loaded.exercises["exercises"].as_array().unwrap().len(),1);
        assert_eq!(loaded.exercises["exercises"][0]["question"],"New");
    }
    #[test]
    fn exercise_review_rejects_forged_sources_ids_and_unsubmitted_drafts() {
        let (_dir,engine,scope,id,original) = exercise_review_fixture();
        let before=engine.lock().unwrap().content.read(&id).unwrap();
        for field in ["id","sourceIds"] {
            let mut invalid=original.clone(); invalid["exercises"][0][field]=if field=="id" {json!("foreign_question")} else {json!(["unauthorized_source"])};
            assert!(super::execute(&engine,&scope,&format!("forged-{field}"),"write",json!({"path":format!("documents/{id}.md"),"content":invalid.to_string(),"expectedVersion":before.version})).is_err());
        }
        let mut changed=original.clone(); changed["exercises"][0]["question"]=json!("changed");
        let p=super::execute(&engine,&scope,"exercise-draft","write",json!({"path":format!("documents/{id}.md"),"content":changed.to_string(),"expectedVersion":before.version})).unwrap();
        let first=original["exercises"][0]["id"].as_str().unwrap();
        engine.drafts.save(&format!("exercise:{first}"),crate::drafts::DraftData {question:"unsent input".into(),..Default::default()},None).unwrap();
        assert_eq!(super::exercise_proposal_decide(&engine,p["courseId"].as_str().unwrap(),p["proposalId"].as_str().unwrap(),first,true).unwrap_err().code,"CONFLICT");
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().version,before.version);
    }

    #[test]
    #[ignore = "isolated native exercise review GUI fixture"]
    fn create_exercise_review_gui_fixture() {
        let (dir,engine,scope,id,original)=exercise_review_fixture();
        let before=engine.lock().unwrap().content.read(&id).unwrap();
        let mut next=original.clone();
        next["exercises"][0]["question"]=json!("下列哪个选项表示数字二？\n\nA. one\nB. two");
        next["exercises"][0]["answers"]=json!(["B"]);
        next["exercises"][0]["explanation"]=json!("two 表示二，与题目中的数字对应。");
        next["exercises"].as_array_mut().unwrap().push(json!({"question":"新增：用英语写出数字三。","format":"exact","answers":["three"],"explanation":"three 表示三。","tags":[],"sourceIds":[]}));
        let p=super::execute(&engine,&scope,"gui-exercise-review","write",json!({"path":format!("documents/{id}.md"),"content":next.to_string(),"expectedVersion":before.version})).unwrap();
        let root=dir.keep();
        std::fs::create_dir_all("/tmp/exercise-review-evidence").unwrap();
        crate::app::atomic_json(std::path::Path::new("/tmp/exercise-review-evidence/fixture.json"),&json!({"library":root.join("library"),"state":root.join("state"),"documentId":id,"proposal":p})).unwrap();
        println!("EXERCISE_REVIEW_FIXTURE {}",root.display());
    }

    #[test]
    fn raw_files_save_without_review_and_noop_documents_do_not_stage() {
        let (dir, engine, scope, id, _) = course_fixture("write");
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let unchanged = super::execute(&engine, &scope, "unchanged", "write", json!({
            "path":format!("documents/{id}.md"),"content":before.body,"expectedVersion":before.version
        })).unwrap();
        assert_eq!(unchanged["status"], "unchanged");
        assert_eq!(unchanged["persisted"], false);
        let course = unchanged["courseId"].as_str().unwrap();
        for (index, path) in ["notes/a.txt", "notes/a.md", "notes/a.py", "notes/a.docx"].iter().enumerate() {
            let args = json!({"path":path,"content":"original"});
            let call = format!("raw-{index}");
            let saved = super::execute(&engine, &scope, &call, "write", args.clone()).unwrap();
            assert_eq!(saved["persisted"], true);
            assert!(saved["proposalId"].is_null());
            assert_eq!(super::execute(&engine, &scope, &call, "write", args).unwrap()["replayed"], true);
            assert_eq!(super::execute(&engine, &scope, &format!("missing-version-{index}"), "write", json!({"path":path,"content":"changed"})).unwrap_err().code, "CONFLICT");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(dir.path().join(format!("library/courses/{course}/materials/{path}")), std::fs::Permissions::from_mode(0o750)).unwrap();
            }
            let saved = super::execute(&engine, &scope, &format!("replace-{index}"), "write", json!({"path":path,"content":"changed","expectedVersion":saved["version"]})).unwrap();
            assert_eq!(saved["persisted"], true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(dir.path().join(format!("library/courses/{course}/materials/{path}"))).unwrap().permissions().mode() & 0o7777, 0o750);
            }
        }
        assert!(super::proposal_list(&engine, course).unwrap().as_array().unwrap().is_empty());
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().version, before.version);
    }

    #[test]
    fn legacy_raw_pending_is_explicitly_rejectable_without_blocking_new_write() {
        let (dir, engine, scope, id, _) = course_fixture("write");
        let course = engine.lock().unwrap().content.read(&id).unwrap().metadata.extra["parentId"].as_str().unwrap().to_owned();
        let args = json!({"path":"legacy.txt","content":"old proposed text"});
        let (operation, _) = crate::learning_tools::operation_id(&engine,&scope,"legacy-raw","write",&args).unwrap();
        let proposal = json!({"id":operation,"courseId":course,"sessionId":scope.session_id,
            "sectionId":scope.section_id,"runId":scope.run_id,"callId":"legacy-raw","allowedIds":scope.allowed_ids,
            "path":"legacy.txt","expectedVersion":null,"original":"","content":"old proposed text","status":"pending","receipt":null});
        let receipt = json!({"persisted":false,"proposalPersisted":true,"proposalId":operation,"courseId":course,
            "sessionId":scope.session_id,"path":"legacy.txt","status":"pending"});
        crate::course_write_transaction::durable_json(&dir.path().join(format!("state/edit-proposals/{course}/{operation}.json")),&proposal).unwrap();
        let ledger = super::LedgerEntry { name:"write".into(), arguments_hash:crate::types::hash(&serde_json::to_string(&args).unwrap()), result:receipt, created_at:crate::types::timestamp() };
        crate::course_write_transaction::durable_json(&dir.path().join(format!("state/workspaces/{}/receipts/{operation}.json",scope.session_id)),&ledger).unwrap();
        assert_eq!(super::execute(&engine,&scope,"legacy-raw","write",args).unwrap()["status"],"pending");
        let target = dir.path().join(format!("library/courses/{course}/materials/legacy.txt"));
        assert!(!target.exists(), "reading or replaying old pending must not publish");
        assert_eq!(super::execute(&engine,&scope,"new-raw","write",json!({"path":"legacy.txt","content":"new authorized text"})).unwrap()["persisted"],true);
        assert_eq!(super::proposal_decide(&engine,&course,&operation,true).unwrap_err().code,"CONFLICT");
        assert_eq!(super::proposal_decide(&engine,&course,&operation,false).unwrap()["status"],"rejected");
        assert_eq!(std::fs::read_to_string(target).unwrap(),"new authorized text");
    }

    #[test]
    fn write_conflict_identifies_pending_review_and_recovers_after_explicit_rejection() {
        let (_dir, engine, scope, id, _) = course_fixture("write");
        let read = super::execute(&engine,&scope,"read-version","read",json!({"sourceId":id})).unwrap();
        let args = json!({"path":format!("documents/{id}.md"),"content":"proposed body","expectedVersion":read["version"]});
        let pending = super::execute(&engine,&scope,"old-proposal","write",args.clone()).unwrap();
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let blocked = super::execute(&engine,&scope,"new-proposal","write",args.clone()).unwrap_err();
        assert_eq!(blocked.code,"CONFLICT");
        let feedback = super::write_conflict_feedback(&blocked).unwrap();
        assert_eq!(feedback["details"]["reason"],"edit_review_pending");
        assert_eq!(feedback["details"]["proposalId"],pending["proposalId"]);
        assert_eq!(feedback["details"]["status"],"pending");
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().content,before.content);
        let mut wrong = args.clone(); wrong["expectedVersion"] = json!("0".repeat(40));
        let mismatch = super::execute(&engine,&scope,"wrong-oid","write",wrong).unwrap_err();
        assert_eq!(super::write_conflict_feedback(&mismatch).unwrap()["details"]["reason"],"write_version_mismatch");
        super::proposal_decide(&engine,pending["courseId"].as_str().unwrap(),pending["proposalId"].as_str().unwrap(),false).unwrap();
        let next = super::execute(&engine,&scope,"after-reject","write",args).unwrap();
        assert_eq!(next["status"],"pending");
        assert_eq!(next["persisted"],false);
        assert_ne!(next["proposalId"],pending["proposalId"]);
    }

    #[test]
    fn proposals_preserve_original_serialize_and_recover_decisions() {
        let (dir, engine, scope, id, _) = course_fixture("write");
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let args = json!({"path":format!("documents/{id}.md"),"content":"Accepted body","expectedVersion":before.version});
        let pending = super::execute(&engine, &scope, "proposal", "write", args.clone()).unwrap();
        let course = pending["courseId"].as_str().unwrap();
        let proposal = pending["proposalId"].as_str().unwrap();
        assert_eq!(pending["persisted"], false);
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().content, before.content);
        assert_eq!(super::proposal_list(&engine, course).unwrap().as_array().unwrap().len(), 1);
        assert_eq!(super::proposal_get(&engine, course, proposal).unwrap()["original"], before.body);
        let mut changed = args.clone(); changed["content"] = json!("Second");
        assert_eq!(super::execute(&engine, &scope, "second", "write", changed).unwrap_err().code, "CONFLICT");
        let reopened = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        assert_eq!(super::proposal_get(&reopened, course, proposal).unwrap()["status"], "pending");
        let receipt = super::proposal_decide(&engine, course, proposal, true).unwrap();
        assert_eq!(receipt["persisted"], true);
        assert_eq!(super::proposal_decide(&engine, course, proposal, true).unwrap(), receipt);
        assert_eq!(super::proposal_result(&engine, pending.clone()).unwrap(), receipt);
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().body.trim(), "Accepted body");
        let replay = super::execute(&engine, &scope, "proposal", "write", args).unwrap();
        assert_eq!(replay["persisted"], true);
    }

    #[test]
    fn proposal_pre_intent_crash_recovers_only_exact_pending_evidence() {
        let (dir, engine, scope, id, _) = course_fixture("write");
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let pending = super::execute(&engine, &scope, "crash-before-intent", "write", json!({"path":format!("documents/{id}.md"),"content":"Not published","expectedVersion":before.version})).unwrap();
        let course = pending["courseId"].as_str().unwrap(); let proposal = pending["proposalId"].as_str().unwrap();
        let path = dir.path().join(format!("state/edit-proposals/{course}/{proposal}.json"));
        let mut data: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        data["status"] = json!("accepting");
        crate::course_write_transaction::durable_json(&path, &data).unwrap();
        let reopened = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        assert_eq!(super::proposal_get(&reopened, course, proposal).unwrap()["status"], "pending");
        let ledger = dir.path().join(format!("state/workspaces/{}/receipts/{proposal}.json", scope.session_id));
        // Any durable intent keeps an uncertain publication locked.
        std::fs::write(ledger.with_extension("pending"), "{}").unwrap();
        assert_eq!(super::proposal_get(&reopened, course, proposal).unwrap()["status"], "accepting");
        assert_eq!(super::proposal_decide(&reopened, course, proposal, false).unwrap_err().code, "NATIVE_TOOL_UNKNOWN");
        std::fs::remove_file(ledger.with_extension("pending")).unwrap();
        let original_ledger = std::fs::read(&ledger).unwrap();
        let mut entry: Value = serde_json::from_slice(&original_ledger).unwrap();
        entry["argumentsHash"] = json!("different");
        crate::course_write_transaction::durable_json(&ledger, &entry).unwrap();
        assert_eq!(super::proposal_get(&reopened, course, proposal).unwrap()["status"], "accepting");
        std::fs::write(&ledger, original_ledger).unwrap();
        assert_eq!(super::proposal_decide(&reopened, course, proposal, false).unwrap()["status"], "rejected");
        assert_eq!(reopened.lock().unwrap().content.read(&id).unwrap().body, before.body);
    }

    #[test]
    fn proposal_completion_ledger_recovers_interrupted_status_without_republish() {
        let (dir, engine, scope, id, _) = course_fixture("write");
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let pending = super::execute(&engine, &scope, "crash-status", "write", json!({"path":format!("documents/{id}.md"),"content":"Accepted once","expectedVersion":before.version})).unwrap();
        let course = pending["courseId"].as_str().unwrap(); let proposal = pending["proposalId"].as_str().unwrap();
        let receipt = super::proposal_decide(&engine, course, proposal, true).unwrap();
        let path = dir.path().join(format!("state/edit-proposals/{course}/{proposal}.json"));
        let archived = path.parent().unwrap().join("archive").join(path.file_name().unwrap());
        let mut data: Value = serde_json::from_slice(&std::fs::read(&archived).unwrap()).unwrap();
        data["status"] = json!("accepting"); data["receipt"] = Value::Null;
        crate::course_write_transaction::durable_json(&path, &data).unwrap();
        engine.save_ui_preferences(UiPreferences::default()).unwrap();
        assert_eq!(super::proposal_result(&engine, pending.clone()).unwrap(), receipt);
        assert_eq!(super::proposal_decide(&engine, course, proposal, true).unwrap(), receipt);
        assert_eq!(super::proposal_get(&engine, course, proposal).unwrap()["status"], "accepted");
    }

    #[test]
    fn proposal_missing_or_mismatched_initial_ledger_never_publishes() {
        let (dir, engine, scope, id, _) = course_fixture("write");
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let pending = super::execute(&engine, &scope, "missing-ledger", "write", json!({"path":format!("documents/{id}.md"),"content":"Unsafe publication","expectedVersion":before.version})).unwrap();
        let course = pending["courseId"].as_str().unwrap();
        let proposal = pending["proposalId"].as_str().unwrap();
        let ledger = dir.path().join(format!("state/workspaces/{}/receipts/{proposal}.json", scope.session_id));
        let mut entry: Value = serde_json::from_slice(&std::fs::read(&ledger).unwrap()).unwrap();
        std::fs::remove_file(&ledger).unwrap();
        assert_eq!(super::proposal_decide(&engine, course, proposal, true).unwrap_err().code, "NATIVE_TOOL_UNKNOWN");
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().content, before.content);
        assert!(!ledger.with_extension("pending").exists());
        entry["argumentsHash"] = json!("mismatched");
        crate::course_write_transaction::durable_json(&ledger, &entry).unwrap();
        assert_eq!(super::proposal_decide(&engine, course, proposal, true).unwrap_err().code, "CONFLICT");
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().content, before.content);
        assert_eq!(super::proposal_decide(&engine, course, proposal, false).unwrap()["status"], "rejected");
    }

    #[test]
    fn proposal_rejection_after_restart_settles_original_session_and_run() {
        let (dir, engine, scope, id, _) = course_fixture("write");
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let pending = super::execute(&engine, &scope, "cancelled-call", "write", json!({"path":format!("documents/{id}.md"),"content":"Cancelled draft","expectedVersion":before.version})).unwrap();
        let course = pending["courseId"].as_str().unwrap();
        let proposal = pending["proposalId"].as_str().unwrap();
        // Emulate a pre-upgrade proposal and the journal persisted before a
        // native handler was cancelled while awaiting the user's decision.
        let path = dir.path().join(format!("state/edit-proposals/{course}/{proposal}.json"));
        let mut data: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        data.as_object_mut().unwrap().remove("callId");
        crate::course_write_transaction::durable_json(&path, &data).unwrap();
        let native = dir.path().join("state/pi-native").join(crate::types::hash(&scope.session_id));
        std::fs::create_dir_all(&native).unwrap();
        let name = format!("tool-{}.json", crate::types::hash(&serde_json::to_string(&json!([scope.run_id, "cancelled-call"])).unwrap()));
        let args = json!({"path":format!("documents/{id}.md"),"content":"Cancelled draft","expectedVersion":before.version});
        crate::native_session::atomic(&native.join(name), &json!({"version":1,"runId":scope.run_id,"callId":"cancelled-call","name":"write","argsHash":crate::types::hash(&serde_json::to_string(&args).unwrap()),"reply":null})).unwrap();
        let activity_id = format!("tool_{}", &crate::types::hash(&format!("{}\ncancelled-call", scope.run_id))[..32]);
        let activity: crate::generation::ToolActivity = serde_json::from_value(json!({"id":activity_id,"toolCallId":activity_id,"runId":scope.run_id,"sessionId":scope.session_id,"name":"write","status":"running","title":"受控写入","summary":"AI 修改待确认，建议已保留，正文未修改。","startedAt":"2026-10-23T00:00:00Z","updatedAt":"2026-10-23T00:00:00Z","persisted":false,"object":null,"errorCode":null})).unwrap();
        let mut run = crate::generation::get(&engine, &scope.run_id).unwrap();
        run.status = "cancelled".into();
        run.tool_calls.push(activity);
        crate::app::atomic_json(&dir.path().join(format!("state/runs/{}.json", scope.run_id)), &run).unwrap();
        let other = crate::course_sessions::create(&mut engine.lock().unwrap(), course, "different-active-session").unwrap();
        let reopened = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        super::proposal_decide(&reopened, course, proposal, false).unwrap();
        let again = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let run = crate::generation::get(&again, &scope.run_id).unwrap();
        assert_eq!(run.tool_calls.len(), 1);
        assert_eq!(run.tool_calls[0].status, "succeeded");
        assert_eq!(run.tool_calls[0].id, activity_id);
        assert_eq!(run.status, "cancelled");
        assert!(!run.tool_calls[0].persisted);
        assert_eq!(run.tool_calls[0].session_id, scope.session_id);
        let mut core = again.lock().unwrap();
        let original = crate::sessions::get(&mut core.content, &scope.session_id).unwrap();
        assert!(original.entries.iter().any(|entry| entry.data.as_ref().is_some_and(|data| data["toolActivity"]["summary"] == "用户已拒绝建议，正文未修改。")));
        assert!(crate::sessions::get(&mut core.content, &other.id).unwrap().entries.is_empty());
        drop(core);
        assert!(super::proposal_list(&again, course).unwrap().as_array().unwrap().is_empty());
        assert_eq!(super::proposal_get(&again, course, proposal).unwrap()["status"], "rejected");
        assert_eq!(super::proposal_result(&again, pending).unwrap()["status"], "rejected");
    }

    #[test]
    fn proposal_decisions_settle_cancelled_saved_question_retries_only() {
        for accept in [false, true] {
            for legacy in [false, true] {
                let (dir, engine, scope, id, _) = read_replay_fixture();
                engine.save_ui_preferences(UiPreferences { tool_authorization_tier: "write".into(), ..UiPreferences::default() }).unwrap();
                let before = engine.lock().unwrap().content.read(&id).unwrap();
                let args = json!({"path":format!("documents/{id}.md"),"content":"Retry draft","expectedVersion":before.version});
                let pending = super::execute(&engine, &scope, "first-call", "write", args.clone()).unwrap();
                let retry = retry_read_run(&engine, &scope, false);
                let replay = super::execute(&engine, &retry, "retry-call", "write", args.clone()).unwrap();
                assert_eq!(replay["proposalId"], pending["proposalId"]);
                assert_eq!(replay["replayed"], true);
                let course = pending["courseId"].as_str().unwrap();
                let proposal = pending["proposalId"].as_str().unwrap();
                let proposal_path = dir.path().join(format!("state/edit-proposals/{course}/{proposal}.json"));
                if legacy {
                    let mut data: Value = serde_json::from_slice(&std::fs::read(&proposal_path).unwrap()).unwrap();
                    data.as_object_mut().unwrap().remove("callId");
                    crate::course_write_transaction::durable_json(&proposal_path, &data).unwrap();
                }
                let native = dir.path().join("state/pi-native").join(crate::types::hash(&scope.session_id));
                std::fs::create_dir_all(&native).unwrap();
                let mut other_args = args.clone();
                other_args["content"] = json!("Unrelated draft");
                // Real runs share one saved question; journal/activity checkpoints
                // reproduce cancellation while both handlers awaited the decision.
                for (current, call, call_args) in [(&scope, "first-call", &args), (&retry, "retry-call", &args), (&retry, "different-args", &other_args)] {
                    let name = format!("tool-{}.json", crate::types::hash(&serde_json::to_string(&json!([current.run_id, call])).unwrap()));
                    crate::native_session::atomic(&native.join(name), &json!({"version":1,"runId":current.run_id,"callId":call,"name":"write","argsHash":crate::types::hash(&serde_json::to_string(call_args).unwrap()),"reply":null})).unwrap();
                    let activity_id = format!("tool_{}", &crate::types::hash(&format!("{}\n{call}", current.run_id))[..32]);
                    let activity = serde_json::from_value(json!({"id":activity_id,"toolCallId":activity_id,"runId":current.run_id,"sessionId":current.session_id,"name":"write","status":"unknown","title":"受控写入","summary":"等待核对","startedAt":"2026-10-23T00:00:00Z","updatedAt":"2026-10-23T00:00:00Z","persisted":false,"object":null,"errorCode":"NATIVE_TOOL_UNKNOWN"})).unwrap();
                    let mut runs = engine.runs.lock().unwrap();
                    let run = &mut runs.get_mut(&current.run_id).unwrap().run;
                    run.status = "cancelled".into();
                    run.tool_calls.push(activity);
                    crate::app::atomic_json(&dir.path().join(format!("state/runs/{}.json", current.run_id)), run).unwrap();
                }
                // Matching args in a different saved question must stay unknown.
                let mut unrelated = crate::generation::get(&engine, &retry.run_id).unwrap();
                unrelated.id = "run_unrelated_question".into();
                unrelated.user_entry_id = Some("different_saved_question".into());
                unrelated.tool_calls.truncate(1);
                unrelated.tool_calls[0].run_id = unrelated.id.clone();
                crate::app::atomic_json(&dir.path().join(format!("state/runs/{}.json", unrelated.id)), &unrelated).unwrap();
                let name = format!("tool-{}.json", crate::types::hash(&serde_json::to_string(&json!([unrelated.id, "retry-call"])).unwrap()));
                crate::native_session::atomic(&native.join(name), &json!({"version":1,"runId":unrelated.id,"callId":"retry-call","name":"write","argsHash":crate::types::hash(&serde_json::to_string(&args).unwrap()),"reply":null})).unwrap();
                let reopened = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
                let receipt = super::proposal_decide(&reopened, course, proposal, accept).unwrap();
                for current in [&scope, &retry] {
                    let run = crate::generation::get(&reopened, &current.run_id).unwrap();
                    assert_eq!(run.status, "cancelled");
                    assert_eq!(run.tool_calls[0].status, "succeeded");
                    assert_eq!(run.tool_calls[0].persisted, accept);
                }
                assert_eq!(crate::generation::get(&reopened, &retry.run_id).unwrap().tool_calls[1].status, "unknown");
                assert_eq!(crate::generation::get(&reopened, &unrelated.id).unwrap().tool_calls[0].status, "unknown");
                let run_path = dir.path().join(format!("state/runs/{}.json", retry.run_id));
                let checkpoint: Value = serde_json::from_slice(&std::fs::read(&run_path).unwrap()).unwrap();
                assert_eq!(super::proposal_decide(&reopened, course, proposal, accept).unwrap(), receipt);
                assert_eq!(super::proposal_result(&reopened, pending.clone()).unwrap(), receipt);
                let repeated: Value = serde_json::from_slice(&std::fs::read(&run_path).unwrap()).unwrap();
                assert_eq!(repeated["toolCalls"], checkpoint["toolCalls"]);
                assert!(super::proposal_list(&reopened, course).unwrap().as_array().unwrap().is_empty());
                assert!(!proposal_path.exists());
                let mut core = reopened.lock().unwrap();
                core.content.refresh().unwrap();
                assert_eq!(core.content.read(&id).unwrap().body.trim(), if accept { "Retry draft" } else { before.body.trim() });
            }
        }
    }

    #[test]
    fn proposal_terminal_checkpoint_repairs_activity_before_archiving() {
        for accept in [false, true] {
            for previously_archived in [false, true] {
                let (dir, engine, scope, id, _) = course_fixture("write");
                let before = engine.lock().unwrap().content.read(&id).unwrap();
                let pending = super::execute(&engine, &scope, "terminal-checkpoint", "write", json!({"path":format!("documents/{id}.md"),"content":"Final body","expectedVersion":before.version})).unwrap();
                let course = pending["courseId"].as_str().unwrap();
                let proposal = pending["proposalId"].as_str().unwrap();
                let session_path = {
                    let mut core = engine.lock().unwrap();
                    let session = crate::sessions::get(&mut core.content, &scope.session_id).unwrap();
                    core.content.root().join(session.path)
                };
                let run_path = dir.path().join(format!("state/runs/{}.json", scope.run_id));
                let session_checkpoint = std::fs::read(&session_path).unwrap();
                let run_checkpoint = std::fs::read(&run_path).unwrap();
                let receipt = super::proposal_decide(&engine, course, proposal, accept).unwrap();
                let path = dir.path().join(format!("state/edit-proposals/{course}/{proposal}.json"));
                let archive = path.parent().unwrap().join("archive").join(path.file_name().unwrap());
                // Preserve the real durable decision/publication but restore
                // activity records to the pre-settlement crash checkpoint.
                std::fs::write(&session_path, session_checkpoint).unwrap();
                std::fs::write(&run_path, run_checkpoint).unwrap();
                if !previously_archived { std::fs::rename(&archive, &path).unwrap(); }
                let reopened = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
                if previously_archived {
                    // Compatibility with the previous archive-before-settle window.
                    assert_eq!(super::proposal_result(&reopened, pending.clone()).unwrap(), receipt);
                } else {
                    let original_run = std::fs::read(&run_path).unwrap();
                    std::fs::write(&run_path, "broken run checkpoint").unwrap();
                    assert!(super::proposal_list(&reopened, course).is_err());
                    assert!(path.exists(), "failed settlement must remain discoverable");
                    assert!(!archive.exists());
                    std::fs::write(&run_path, original_run).unwrap();
                    assert!(super::proposal_list(&reopened, course).unwrap().as_array().unwrap().is_empty());
                    assert!(!path.exists());
                    assert!(archive.exists());
                }
                let again = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
                let run = crate::generation::get(&again, &scope.run_id).unwrap();
                assert_eq!(run.tool_calls.len(), 1);
                assert_eq!(run.tool_calls[0].status, "succeeded");
                assert_eq!(run.tool_calls[0].persisted, accept);
                let mut core = again.lock().unwrap();
                let session = crate::sessions::get(&mut core.content, &scope.session_id).unwrap();
                assert!(session.entries.iter().any(|entry| entry.data.as_ref().is_some_and(|data| data["toolActivity"]["status"] == "succeeded")));
                core.content.refresh().unwrap();
                assert_eq!(core.content.read(&id).unwrap().body.trim(), if accept { "Final body" } else { before.body.trim() });
            }
        }
    }

    #[test]
    fn upgraded_terminal_activity_replay_archives_without_changing_legacy_digest() {
        for accept in [false, true] {
            let (dir, engine, scope, id, _) = course_fixture("write");
            let before = engine.lock().unwrap().content.read(&id).unwrap();
            let pending = super::execute(&engine, &scope, "legacy-settled", "write", json!({"path":format!("documents/{id}.md"),"content":"Saved once","expectedVersion":before.version})).unwrap();
            let course = pending["courseId"].as_str().unwrap();
            let proposal = pending["proposalId"].as_str().unwrap();
            let receipt = super::proposal_decide(&engine, course, proposal, accept).unwrap();
            // Build the old-release checkpoint: its terminal receipt is already
            // durable, but the process stopped before moving the flat proposal.
            let (entry_path, legacy_bytes) = {
                let mut core = engine.lock().unwrap();
                let session = crate::sessions::get_persisted(&mut core.content, &scope.session_id).unwrap();
                let entry = session.entries.iter().find(|entry| entry.data.as_ref().is_some_and(|data| data["toolActivity"]["status"] == "succeeded")).unwrap();
                let mut doc = core.content.read(&entry.id).unwrap();
                let mut data = entry.data.clone().unwrap();
                let activity = data["toolActivity"].as_object_mut().unwrap();
                activity.remove("userEntryId"); activity.remove("sequence");
                let digest = crate::types::hash(&serde_json::to_string(&json!({"kind":entry.kind,"content":entry.content,"selection":entry.selection,"relatedIds":entry.related_ids,"data":data})).unwrap());
                doc.metadata.extra.insert("data".into(), data);
                doc.metadata.extra.insert("requestDigest".into(), json!(digest));
                let path = core.content.root().join(&doc.path);
                let bytes = crate::content::serialize(&doc.metadata, &doc.body).unwrap().into_bytes();
                std::fs::write(&path, &bytes).unwrap();
                (path, bytes)
            };
            let run_path = dir.path().join(format!("state/runs/{}.json", scope.run_id));
            let mut run: Value = serde_json::from_slice(&std::fs::read(&run_path).unwrap()).unwrap();
            for activity in run["toolCalls"].as_array_mut().unwrap() {
                activity.as_object_mut().unwrap().remove("userEntryId");
                activity.as_object_mut().unwrap().remove("sequence");
            }
            std::fs::write(&run_path, serde_json::to_vec(&run).unwrap()).unwrap();
            let path = dir.path().join(format!("state/edit-proposals/{course}/{proposal}.json"));
            let archive = path.parent().unwrap().join("archive").join(path.file_name().unwrap());
            std::fs::rename(&archive, &path).unwrap();
            drop(engine);
            let reopened = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
            assert!(super::proposal_list(&reopened, course).unwrap().as_array().unwrap().is_empty());
            assert!(!path.exists() && archive.exists());
            assert_eq!(super::proposal_result(&reopened, pending).unwrap(), receipt);
            assert_eq!(std::fs::read(&entry_path).unwrap(), legacy_bytes);
            assert_eq!(crate::generation::get(&reopened, &scope.run_id).unwrap().tool_calls.len(), 1);
            assert_eq!(reopened.lock().unwrap().content.read(&id).unwrap().body.trim(), if accept { "Saved once" } else { before.body.trim() });
        }
    }

    #[test]
    fn proposal_poll_preserves_recycled_session_terminal_until_restore() {
        let (dir, engine, scope, document_id, _) = course_fixture("write");
        let before = engine.lock().unwrap().content.read(&document_id).unwrap();
        let mut args = json!({"path":format!("documents/{document_id}.md"),"content":"old draft","expectedVersion":before.version});
        let terminal = super::execute(&engine, &scope, "terminal-trash", "write", args.clone()).unwrap();
        let course = terminal["courseId"].as_str().unwrap();
        let id = terminal["proposalId"].as_str().unwrap();
        let path = dir.path().join(format!("state/edit-proposals/{course}/{id}.json"));
        let archive = path.parent().unwrap().join("archive").join(path.file_name().unwrap());
        // Crash checkpoint: rejection is durable, but its activity is not settled.
        let mut data: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        data["status"] = json!("rejected");
        crate::course_write_transaction::durable_json(&path, &data).unwrap();
        args["content"] = json!("new draft");
        let pending = super::execute(&engine, &scope, "new-pending", "write", args).unwrap();
        let trash = {
            let mut core = engine.lock().unwrap();
            let preview = crate::trash::preview(&mut core.content, &scope.session_id).unwrap();
            crate::trash::trash(&mut core.content, &scope.session_id, &preview.version, "terminal-session-trash").unwrap()
        };
        let session_path = dir.path().join(format!("library/sessions/{}/session.md", scope.session_id));
        let session_before = std::fs::read(&session_path).unwrap();
        let proposal_before = std::fs::read(&path).unwrap();
        let run_path = dir.path().join(format!("state/runs/{}.json", scope.run_id));
        let run_before = std::fs::read(&run_path).unwrap();
        let reopened = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        for _ in 0..2 {
            let list = super::proposal_list(&reopened, course).unwrap();
            assert_eq!(list.as_array().unwrap().len(), 1);
            assert_eq!(list[0]["id"], pending["proposalId"]);
            assert_eq!(super::proposal_get(&reopened, course, pending["proposalId"].as_str().unwrap()).unwrap()["content"], "new draft");
            assert_eq!(std::fs::read(&session_path).unwrap(), session_before);
            assert_eq!(std::fs::read(&path).unwrap(), proposal_before);
            assert_eq!(std::fs::read(&run_path).unwrap(), run_before);
            assert!(!archive.exists());
            assert!(reopened.lock().unwrap().content.is_trashed(&scope.session_id));
        }
        // A recycled owner does not excuse unrelated corruption.
        std::fs::write(&run_path, "broken checkpoint").unwrap();
        let error = super::proposal_list(&reopened, course).unwrap_err();
        assert_ne!(error.code, "TRASHED");
        assert!(path.exists() && !archive.exists());
        std::fs::write(&run_path, run_before).unwrap();
        {
            let mut core = reopened.lock().unwrap();
            crate::trash::restore(&mut core.content, &trash.trash_id, "terminal-session-restore").unwrap();
        }
        let list = super::proposal_list(&reopened, course).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["id"], pending["proposalId"]);
        assert!(!path.exists() && archive.exists());
        assert_eq!(crate::generation::get(&reopened, &scope.run_id).unwrap().tool_calls[0].status, "succeeded");
        assert_eq!(reopened.lock().unwrap().content.read(&document_id).unwrap().body, before.body);
    }

    #[test]
    fn proposal_poll_migrates_legacy_terminal_files_and_ignores_archive_contents() {
        let (dir, engine, scope, document_id, _) = course_fixture("write");
        let before = engine.lock().unwrap().content.read(&document_id).unwrap();
        let args = json!({"path":format!("documents/{document_id}.md"),"content":"draft","expectedVersion":before.version});
        let pending = super::execute(&engine, &scope, "legacy", "write", args.clone()).unwrap();
        let course = pending["courseId"].as_str().unwrap(); let id = pending["proposalId"].as_str().unwrap();
        let path = dir.path().join(format!("state/edit-proposals/{course}/{id}.json"));
        let mut data: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        data["status"] = json!("rejected");
        data.as_object_mut().unwrap().remove("callId");
        crate::course_write_transaction::durable_json(&path, &data).unwrap();
        assert!(super::proposal_list(&engine, course).unwrap().as_array().unwrap().is_empty());
        let archive = path.parent().unwrap().join("archive");
        assert!(!path.exists());
        assert!(archive.join(format!("{id}.json")).exists());
        // Malformed history cannot slow down or break active polling/staging.
        std::fs::write(archive.join("historical.json"), "invalid history").unwrap();
        assert!(super::proposal_list(&engine, course).unwrap().as_array().unwrap().is_empty());
        assert_eq!(super::proposal_get(&engine, course, id).unwrap()["status"], "rejected");
        assert_eq!(super::execute(&engine, &scope, "fresh", "write", args).unwrap()["status"], "pending");
    }

    #[test]
    fn proposal_publication_serializes_with_both_preference_save_paths() {
        use std::sync::mpsc;
        use std::time::Duration;
        for patch in [false, true] {
            let (_dir, engine, scope, id, _) = course_fixture("write");
            let before = engine.lock().unwrap().content.read(&id).unwrap();
            let pending = super::execute(&engine, &scope, "serialized", "write", json!({"path":format!("documents/{id}.md"),"content":"Accepted before revocation","expectedVersion":before.version})).unwrap();
            let course = pending["courseId"].as_str().unwrap().to_owned();
            let proposal = pending["proposalId"].as_str().unwrap().to_owned();
            // Stop publication at core access, then verify that the decision
            // already owns the same boundary as both settings writers.
            let core = engine.lock().unwrap();
            let deciding = engine.clone(); let c = course.clone(); let p = proposal.clone();
            let (locked_tx, locked_rx) = mpsc::channel();
            let decision = std::thread::spawn(move || {
                super::course_workspace::DECISION_AUTHORIZATION_LOCKED.with(|notification| *notification.borrow_mut() = Some(locked_tx));
                super::proposal_decide(&deciding, &c, &p, true)
            });
            locked_rx.recv_timeout(Duration::from_secs(5)).expect("the deciding thread must acquire the authorization lock");
            assert!(crate::app::UI_PREFERENCE_WRITE.try_lock().is_err(), "decision must hold the authorization lock while waiting for core");
            let saving = engine.clone(); let (tx, rx) = mpsc::channel();
            let revoke = std::thread::spawn(move || {
                let result = if patch {
                    saving.patch_all_ui_preferences(None, None, None, None, None, Some("read".into()))
                } else { saving.save_ui_preferences(UiPreferences::default()) };
                tx.send(result).unwrap();
            });
            assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
            drop(core);
            let receipt = decision.join().unwrap().unwrap();
            rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(); revoke.join().unwrap();
            assert_eq!(receipt["persisted"], true);
            // Already committed results remain queryable at off/read, while a
            // new pending publication is forbidden after revocation succeeds.
            assert_eq!(super::proposal_decide(&engine, &course, &proposal, true).unwrap(), receipt);
            engine.save_ui_preferences(UiPreferences { tool_authorization_tier: "write".into(), ..UiPreferences::default() }).unwrap();
            let latest = engine.lock().unwrap().content.read(&id).unwrap();
            let pending = super::execute(&engine, &scope, "after-revocation", "write", json!({"path":format!("documents/{id}.md"),"content":"Must remain pending","expectedVersion":latest.version})).unwrap();
            engine.patch_all_ui_preferences(None, None, None, None, None, Some(if patch { "read" } else { "off" }.into())).unwrap();
            let next = pending["proposalId"].as_str().unwrap();
            assert_eq!(super::proposal_decide(&engine, &course, next, true).unwrap_err().code, "TOOL_NOT_ALLOWED");
            assert_eq!(super::proposal_decide(&engine, &course, next, false).unwrap()["status"], "rejected");
        }
    }

    #[test]
    fn recycled_document_proposal_remains_readable_and_rejectable() {
        let (_dir, engine, scope, id, _) = course_fixture("write");
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let pending = super::execute(&engine, &scope, "recycled", "write", json!({"path":format!("documents/{id}.md"),"content":"AI body","expectedVersion":before.version})).unwrap();
        let course = pending["courseId"].as_str().unwrap(); let proposal = pending["proposalId"].as_str().unwrap();
        {
            let mut core = engine.lock().unwrap();
            let preview = crate::trash::preview(&mut core.content, &id).unwrap();
            crate::trash::trash(&mut core.content, &id, &preview.version, "proposal-trash").unwrap();
        }
        assert_eq!(super::proposal_get(&engine, course, proposal).unwrap()["content"], "AI body");
        assert!(super::proposal_decide(&engine, course, proposal, true).is_err());
        assert_eq!(super::proposal_decide(&engine, course, proposal, false).unwrap()["status"], "rejected");
        assert!(super::proposal_list(&engine, course).unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn proposals_reject_keep_drafts_and_refuse_stale_or_revoked_acceptance() {
        let (_dir, engine, scope, id, _) = course_fixture("write");
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let args = json!({"path":format!("documents/{id}.md"),"content":"AI body","expectedVersion":before.version});
        let p = super::execute(&engine, &scope, "proposal", "write", args).unwrap();
        let course = p["courseId"].as_str().unwrap(); let proposal = p["proposalId"].as_str().unwrap();
        let latest = engine.lock().unwrap().content.read(&id).unwrap();
        let manual = crate::content::serialize(&latest.metadata, "External body").unwrap();
        std::fs::write(engine.lock().unwrap().content.root().join(&latest.path), manual).unwrap();
        assert_eq!(super::proposal_decide(&engine, course, proposal, true).unwrap_err().code, "CONFLICT");
        assert_eq!(super::proposal_get(&engine, course, proposal).unwrap()["content"], "AI body");
        engine.save_ui_preferences(UiPreferences::default()).unwrap();
        assert_eq!(super::proposal_decide(&engine, course, proposal, true).unwrap_err().code, "TOOL_NOT_ALLOWED");
        let rejected = super::proposal_decide(&engine, course, proposal, false).unwrap();
        assert_eq!(rejected["status"], "rejected");
        assert_eq!(super::proposal_decide(&engine, course, proposal, false).unwrap(), rejected);
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().body.trim(), "External body");
    }

    #[test]
    #[ignore = "creates an isolated native GUI fixture only when explicitly requested"]
    fn create_edit_review_gui_fixture() {
        let (dir, engine, scope, id, _) = course_fixture("write");
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let p = super::execute(&engine, &scope, "gui-proposal", "write", json!({"path":format!("documents/{id}.md"),"content":"# AI 修改预览\n\n这是待确认的学习笔记。\n\n```rust\nlet learning = true;\n```\n\n公式 $a^2+b^2=c^2$ 和 [链接](https://example.com)。","expectedVersion":before.version})).unwrap();
        let root = dir.keep();
        println!("EDIT_REVIEW_FIXTURE root={} document={} proposal={}", root.display(), id, p);
    }

    use std::sync::Arc;

    #[test]
    fn course_intent_directory_sync_failure_does_not_publish() {
        let (dir, engine, scope, id, _) = course_fixture("write");
        let original = engine.lock().unwrap().content.read(&id).unwrap();
        let args = json!({"path":format!("documents/{id}.md"),"content":"model", "expectedVersion":original.version});
        let (op, _) =
            crate::learning_tools::operation_id(&engine, &scope, "sync-failure", "write", &args)
                .unwrap();
        let pending = dir.path().join(format!(
            "state/workspaces/{}/receipts/{op}.pending",
            scope.session_id
        ));
        crate::course_write_transaction::FAIL_RECEIPT_SYNC
            .with(|f| *f.borrow_mut() = Some(pending.clone()));
        let result = run(&engine, &scope, "sync-failure", "write", args);
        crate::course_write_transaction::FAIL_RECEIPT_SYNC.with(|f| *f.borrow_mut() = None);
        assert_eq!(result.unwrap_err().code, "NATIVE_TOOL_UNKNOWN");
        assert!(pending.is_file());
        let evidence: Value = serde_json::from_slice(&std::fs::read(&pending).unwrap()).unwrap();
        assert_eq!(evidence["sessionId"], scope.session_id);
        assert_eq!(evidence["sectionId"], scope.section_id);
        let core = engine.lock().unwrap();
        assert_eq!(core.content.read(&id).unwrap().content, original.content);
        assert!(!core
            .content
            .root()
            .join(crate::course_write_transaction::recovery_path(
                &original.path,
                &op
            ))
            .exists());
    }

    #[test]
    fn course_completion_directory_sync_failure_is_unknown() {
        let (dir, engine, scope, _, _) = course_fixture("write");
        let args = json!({"path":"sync.txt","content":"model"});
        let (op, _) =
            crate::learning_tools::operation_id(&engine, &scope, "completion-sync", "write", &args)
                .unwrap();
        let ledger = dir.path().join(format!(
            "state/workspaces/{}/receipts/{op}.json",
            scope.session_id
        ));
        crate::course_write_transaction::FAIL_RECEIPT_SYNC
            .with(|f| *f.borrow_mut() = Some(ledger.clone()));
        let result = run(&engine, &scope, "completion-sync", "write", args);
        crate::course_write_transaction::FAIL_RECEIPT_SYNC.with(|f| *f.borrow_mut() = None);
        let error = result.unwrap_err();
        assert_eq!(error.code, "NATIVE_TOOL_UNKNOWN");
        assert_eq!(error.details.unwrap()["published.json"]["exists"], true);
        assert!(ledger.is_file());
    }

    #[test]
    fn unsupported_course_write_has_no_intent_or_publication() {
        let (dir, engine, scope, _, _) = course_fixture("write");
        let args = json!({"path":"unsupported.txt","content":"model"});
        let (op, _) =
            crate::learning_tools::operation_id(&engine, &scope, "unsupported", "write", &args)
                .unwrap();
        crate::course_write_transaction::FORCE_UNSUPPORTED.with(|f| f.set(true));
        let result = run(&engine, &scope, "unsupported", "write", args);
        crate::course_write_transaction::FORCE_UNSUPPORTED.with(|f| f.set(false));
        assert_eq!(result.unwrap_err().code, "UNSUPPORTED_PLATFORM");
        assert!(!dir
            .path()
            .join(format!(
                "state/workspaces/{}/receipts/{op}.pending",
                scope.session_id
            ))
            .exists());
        let mut core = engine.lock().unwrap();
        let course = crate::sessions::get(&mut core.content, &scope.session_id)
            .unwrap()
            .course_id
            .unwrap();
        assert!(!core
            .content
            .root()
            .join(format!("courses/{course}/materials/unsupported.txt"))
            .exists());
    }

    #[test]
    fn legacy_missing_section_recovers_only_bound_pending_course() {
        let (dir, engine, scope, id, sibling) = course_fixture("write");
        let (course, original) = {
            let mut core = engine.lock().unwrap();
            let session = core.content.read(&scope.session_id).unwrap();
            let course = session.metadata.extra["courseId"]
                .as_str()
                .unwrap()
                .to_owned();
            crate::course_project::ensure_project(&mut core.content, &course).unwrap();
            let mut metadata = session.metadata.clone();
            metadata.extra.remove("courseId");
            metadata.extra.insert("sectionId".into(), json!(id));
            let content = crate::content::serialize(&metadata, &session.body).unwrap();
            std::fs::write(core.content.root().join(&session.path), &content).unwrap();
            core.content.register(&session.path).unwrap();
            (course, core.content.read(&id).unwrap())
        };
        let call = crate::model::ToolCall {
            id: "legacy_missing".into(),
            name: "write".into(),
            args: json!({"path":format!("documents/{id}.md"),"content":"model","expectedVersion":original.version}),
        };
        let (op, _) =
            crate::learning_tools::operation_id(&engine, &scope, &call.id, &call.name, &call.args)
                .unwrap();
        let ledger = dir.path().join(format!(
            "state/workspaces/{}/receipts/{op}.json",
            scope.session_id
        ));
        let pending = json!({"path":format!("documents/{id}.md"),"courseId":course,"sessionId":scope.session_id,"sectionId":id});
        crate::course_write_transaction::durable_json(&ledger.with_extension("pending"), &pending)
            .unwrap();
        let core = engine.lock().unwrap();
        let recovery = core
            .content
            .root()
            .join(crate::course_write_transaction::recovery_path(
                &original.path,
                &op,
            ));
        std::fs::create_dir_all(&recovery).unwrap();
        std::fs::rename(
            core.content.root().join(&original.path),
            recovery.join("old"),
        )
        .unwrap();
        drop(core);
        let error = super::recover(&engine, &scope.session_id, &scope.run_id, &call).unwrap_err();
        assert_eq!(error.details.unwrap()["old"]["exists"], true);
        assert!(!engine
            .lock()
            .unwrap()
            .content
            .root()
            .join(&original.path)
            .exists());
        for (key, value) in [("sessionId", "different"), ("sectionId", sibling.as_str())] {
            let mut mismatch = pending.clone();
            mismatch[key] = json!(value);
            crate::course_write_transaction::durable_json(
                &ledger.with_extension("pending"),
                &mismatch,
            )
            .unwrap();
            assert!(
                super::course_write_unknown(&engine, &scope.session_id, &ledger)
                    .details
                    .is_none()
            );
        }
        crate::course_write_transaction::durable_json(&ledger.with_extension("pending"), &pending)
            .unwrap();
        let core = engine.lock().unwrap();
        let session = core.content.read(&scope.session_id).unwrap();
        let mut metadata = session.metadata.clone();
        let other = crate::course_sessions::course(&core.content, &sibling)
            .unwrap()
            .unwrap();
        metadata.extra.insert("courseId".into(), json!(other));
        std::fs::write(
            core.content.root().join(&session.path),
            crate::content::serialize(&metadata, &session.body).unwrap(),
        )
        .unwrap();
        drop(core);
        assert!(
            super::course_write_unknown(&engine, &scope.session_id, &ledger)
                .details
                .is_none()
        );
    }

    #[test]
    fn retained_recovery_directories_do_not_consume_material_quota() {
        let (_dir, engine, scope, id, _) = course_fixture("write");
        let mut core = engine.lock().unwrap();
        let course = crate::sessions::get(&mut core.content, &scope.session_id)
            .unwrap()
            .course_id
            .unwrap();
        crate::course_project::ensure_project(&mut core.content, &course).unwrap();
        let root = crate::course_project::course_project_root(core.content.root(), &course)
            .unwrap()
            .join("materials");
        for index in 0..4100 {
            std::fs::create_dir(root.join(format!(".learning-write-test{index}"))).unwrap();
        }
        let retained = root.join(".learning-write-test0/old");
        std::fs::write(&retained, b"retained").unwrap();
        crate::material_git::FrozenMaterials::capture_project(&core.content, &course).unwrap();
        assert_eq!(std::fs::read(&retained).unwrap(), b"retained");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                root.join(format!("documents/{id}.md")),
                root.join(".learning-write-link"),
            )
            .unwrap();
            assert!(
                crate::material_git::FrozenMaterials::capture_project(&core.content, &course)
                    .is_err()
            );
            std::fs::remove_file(root.join(".learning-write-link")).unwrap();
        }
        for index in 0..4097 {
            std::fs::write(root.join(format!("normal{index}.txt")), b"x").unwrap();
        }
        assert_eq!(
            crate::material_git::FrozenMaterials::capture_project(&core.content, &course)
                .unwrap_err()
                .code,
            "LIMIT_EXCEEDED"
        );
    }

    #[test]
    fn course_write_edits_body_with_version_and_keeps_identity_and_receipt() {
        let (_dir, engine, scope, id, sibling) = course_fixture("write");
        let path = format!("documents/{id}.md");
        let read = run(&engine, &scope, "read-body", "read", json!({"path":path})).unwrap();
        assert!(!read["content"].as_str().unwrap().starts_with("---"));
        let original = engine.lock().unwrap().content.read(&id).unwrap();
        // A normal course may already contain a multi-megabyte attachment.
        // It must not consume the old per-session scratch quota.
        let material_root = engine.lock().unwrap().content.root().join(&original.path).parent().unwrap().parent().unwrap().to_path_buf();
        std::fs::write(material_root.join("large.txt"), vec![b'x'; 3 * 1024 * 1024]).unwrap();
        let args = json!({"path":path,"content":"Updated course body","expectedVersion":read["version"]});
        let saved = run(&engine, &scope, "edit-body", "write", args.clone()).unwrap();
        assert_eq!(saved["persisted"], true);
        let updated = engine.lock().unwrap().content.read(&id).unwrap();
        assert_eq!(updated.body.trim(), "Updated course body");
        assert_eq!(updated.metadata.id, original.metadata.id);
        assert_eq!(updated.metadata.extra["parentId"], original.metadata.extra["parentId"]);
        assert_ne!(updated.version, original.version);
        let (operation,_) = crate::learning_tools::operation_id(&engine,&scope,"edit-body","write",&args).unwrap();
        let recovery=engine.lock().unwrap().content.root().join(crate::course_write_transaction::recovery_path(&original.path,&operation));
        assert_eq!(std::fs::read_to_string(recovery.join("old")).unwrap(),original.content);
        assert_eq!(std::fs::read_to_string(recovery.join("new")).unwrap(),updated.content);
        let restored=crate::content::parse(&std::fs::read_to_string(recovery.join("old")).unwrap()).unwrap().0;
        assert_eq!(restored.id,original.id);

        assert_eq!(run(&engine, &scope, "stale-body", "write", args.clone()).unwrap_err().code, "CONFLICT");
        assert_eq!(run(&engine, &scope, "edit-body", "write", args).unwrap()["replayed"], true);
        assert_eq!(run(&engine, &scope, "missing-version", "write", json!({"path":path,"content":"overwrite"})).unwrap_err().code, "CONFLICT");
        assert_eq!(run(&engine, &scope, "cross-course", "write", json!({"path":format!("documents/{sibling}.md"),"content":"x"})).unwrap_err().code, "TOOL_SCOPE");
        assert_eq!(run(&engine, &scope, "create-record", "write", json!({"path":"documents/forged.md","content":"x"})).unwrap_err().code, "TOOL_SCOPE");
    }

    #[test]
    fn course_files_are_readable_versioned_and_unknown_write_never_repeats() {
        let (dir, engine, scope, _, _) = course_fixture("write");
        let args = json!({"path":"notes/summary.md","content":"New course material"});
        let saved = run(&engine, &scope, "new-file", "write", args.clone()).unwrap();
        let course = saved["courseId"].as_str().unwrap();
        let host = dir.path().join(format!("library/courses/{course}/materials/notes/summary.md"));
        assert_eq!(std::fs::read_to_string(&host).unwrap(), "New course material");
        assert!(!dir.path().join(format!("state/workspaces/{}/files/notes/summary.md",scope.session_id)).exists());
        let read = run(&engine, &scope, "read-file", "read", json!({"path":"notes/summary.md"})).unwrap();
        assert_eq!(read["version"], saved["version"]);
        assert_eq!(read["content"], "New course material");
        let changes = crate::versioning::all_stat(&mut engine.lock().unwrap(), course, None).unwrap();
        assert!(changes.iter().any(|change| change.path == "notes/summary.md"));
        let new_args = json!({"path":"notes/summary.md","content":"Second content","expectedVersion":read["version"]});
        run(&engine, &scope, "update-file", "write", new_args).unwrap();
        assert_eq!(run(&engine, &scope, "new-file", "write", args.clone()).unwrap()["replayed"],true);
        assert_eq!(std::fs::read_to_string(&host).unwrap(), "Second content");
        // Model retry with the exact original operation cannot execute after
        // receipt loss, even if the original destination is now absent.
        let (request_id, _) = crate::learning_tools::operation_id(&engine,&scope,"new-file","write",&args).unwrap();
        std::fs::remove_file(dir.path().join(format!("state/workspaces/{}/receipts/{request_id}.json",scope.session_id))).unwrap();
        std::fs::remove_file(&host).unwrap();
        assert_eq!(run(&engine, &scope, "new-file", "write", args).unwrap_err().code,"NATIVE_TOOL_UNKNOWN");
        assert!(!host.exists());
    }

    #[test]
    fn course_text_write_accepts_html_and_arbitrary_suffixes_and_keeps_versions() {
        let (dir, engine, scope, _, _) = course_fixture("write");
        for (i, path) in ["demo/red-black-tree-demo.html", "demo/source.unlisted", "demo/README"].iter().enumerate() {
            let original = "<!doctype html><title>原文</title>";
            let args = json!({"path":path,"content":original});
            let saved = run(&engine, &scope, &format!("suffix-create-{i}"), "write", args.clone()).unwrap();
            assert_eq!(saved["persisted"], true);
            let changes = crate::versioning::all_stat(&mut engine.lock().unwrap(), saved["courseId"].as_str().unwrap(), None).unwrap();
            assert!(changes.iter().any(|change| change.path == *path));
            let read = run(&engine, &scope, &format!("suffix-read-{i}"), "read", json!({"path":path})).unwrap();
            assert_eq!(read["content"], original);
            assert_eq!(read["version"], saved["version"]);
            let update = json!({"path":path,"content":"updated UTF-8 正文","expectedVersion":read["version"]});
            run(&engine, &scope, &format!("suffix-update-{i}"), "write", update).unwrap();
            assert_eq!(run(&engine, &scope, &format!("suffix-stale-{i}"), "write", json!({"path":path,"content":"stale","expectedVersion":read["version"]})).unwrap_err().code, "CONFLICT");
            assert_eq!(run(&engine, &scope, &format!("suffix-create-{i}"), "write", args).unwrap()["replayed"], true);
            let host = dir.path().join(format!("library/courses/{}/materials/{path}", saved["courseId"].as_str().unwrap()));
            assert_eq!(std::fs::read_to_string(host).unwrap(), "updated UTF-8 正文");
        }
    }

    #[test]
    fn course_write_refuses_unreadable_initial_receipt_before_publication() {
        let (dir, engine, scope, id, _) = course_fixture("write");
        let before = engine.lock().unwrap().content.read(&id).unwrap();
        let args = json!({"path":format!("documents/{id}.md"),"content":"must not publish","expectedVersion":before.version});
        let (request, _) = crate::learning_tools::operation_id(&engine,&scope,"receipt-failure","write",&args).unwrap();
        let ledger = dir.path().join(format!("state/workspaces/{}/receipts/{request}.json",scope.session_id));
        let proposal = super::execute(&engine, &scope, "receipt-failure", "write", args).unwrap();
        std::fs::remove_file(&ledger).unwrap();
        std::fs::create_dir_all(&ledger).unwrap();
        assert_eq!(super::proposal_decide(&engine, proposal["courseId"].as_str().unwrap(), proposal["proposalId"].as_str().unwrap(), true).unwrap_err().code,"NATIVE_TOOL_UNKNOWN");
        assert!(!ledger.with_extension("pending").exists());
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().content, before.content);
        assert_eq!(super::proposal_get(&engine, proposal["courseId"].as_str().unwrap(), proposal["proposalId"].as_str().unwrap()).unwrap()["status"], "pending");
    }

    #[test]
    fn course_write_reports_unknown_for_content_save_failure_after_intent() {
        let (dir, engine, scope, id, _) = course_fixture("write");
        let path = format!("documents/{id}.md");
        let read = run(&engine,&scope,"read-before-fail","read",json!({"path":path})).unwrap();
        std::fs::write(dir.path().join("state/history"),"occupied").unwrap();
        let args = json!({"path":path,"content":"attempt","expectedVersion":read["version"]});
        assert_eq!(run(&engine,&scope,"save-failure","write",args).unwrap_err().code,"NATIVE_TOOL_UNKNOWN");
        assert_eq!(engine.lock().unwrap().content.read(&id).unwrap().body,read["content"].as_str().unwrap());
    }

    #[test]
    fn course_write_refuses_incomplete_baselines_including_json_budget_truncation() {
        let (dir, engine, scope, id, _) = course_fixture("write");
        let mut core = engine.lock().unwrap();
        let doc = core.content.read(&id).unwrap();
        let root = crate::course_project::ensure_project(&mut core.content,doc.metadata.extra["parentId"].as_str().unwrap()).unwrap().join("materials");
        let content = crate::content::serialize(&doc.metadata,&"长".repeat(8000)).unwrap();
        core.content.save(&id,&content,&doc.version).unwrap();
        drop(core);
        // Both raw UTF-8 byte truncation and escaped-JSON transport truncation
        // must reject replacement, even with the correct full-file version.
        for (path, body) in [(format!("documents/{id}.md"),None),("long.txt".into(),Some("x".repeat(21000))),("escaped.txt".into(),Some("\t".repeat(18000)))] {
            if let Some(body) = body { std::fs::write(root.join(&path),body).unwrap(); }
            let before = std::fs::read(root.join(&path)).unwrap();
            let read = run(&engine,&scope,&format!("read-{}",path.len()),"read",json!({"path":path})).unwrap();
            assert_eq!(read["truncated"],true);
            let args=json!({"path":path,"content":"prefix","expectedVersion":read["version"]});
            let (request,_) = crate::learning_tools::operation_id(&engine,&scope,"truncate-write","write",&args).unwrap();
            assert_eq!(run(&engine,&scope,"truncate-write","write",args).unwrap_err().code,"TOOL_SIZE_LIMIT");
            assert_eq!(std::fs::read(root.join(&path)).unwrap(),before);
            assert!(!dir.path().join(format!("state/workspaces/{}/receipts/{request}.pending",scope.session_id)).exists());
        }
    }

    #[test]
    fn course_paths_support_unicode_spaces_depth_and_leading_underscore_only_in_courses() {
        let (_dir, engine, scope, _, _) = course_fixture("write");
        let path = "中文目录/含 空格/三层/四层/五层/_总结.txt";
        let written = run(&engine,&scope,"unicode-new","write",json!({"path":path,"content":"original"})).unwrap();
        let read = run(&engine,&scope,"unicode-read","read",json!({"path":path})).unwrap();
        assert_eq!(read["version"],written["version"]);
        run(&engine,&scope,"unicode-edit","write",json!({"path":path,"content":"updated","expectedVersion":read["version"]})).unwrap();
        let receipts = super::list_receipt_summaries(&engine,&scope.session_id).unwrap();
        assert!(receipts["receipts"].as_array().unwrap().iter().any(|r|r["path"]==path));
        let (_dir, legacy, legacy_scope) = fixture("write");
        assert_eq!(run(&legacy,&legacy_scope,"unicode-legacy","write",json!({"path":path,"content":"x"})).unwrap_err().code,"INVALID_TOOL_INPUT");
    }

    #[test]
    fn excluded_material_write_paths_create_neither_file_nor_intent() {
        let (dir, engine, scope, _, _) = course_fixture("write");
        {
            let mut core = engine.lock().unwrap();
            let session = crate::sessions::get(&mut core.content,&scope.session_id).unwrap();
            let root = crate::course_project::ensure_project(&mut core.content,session.course_id.as_ref().unwrap()).unwrap();
            std::fs::create_dir_all(root.join("materials/cache")).unwrap();
            std::fs::create_dir_all(root.join("materials/dist")).unwrap();
        }
        for path in ["cache/summary.md", "Cache/summary.md", "config.json", "dist/summary.txt", "Dist/summary.txt", "notes/auth.json"] {
            let args = json!({"path":path,"content":"{}"});
            let (request, _) = crate::learning_tools::operation_id(&engine,&scope,"excluded-file","write",&args).unwrap();
            assert_eq!(run(&engine,&scope,"excluded-file","write",args).unwrap_err().code,"MATERIAL_SCOPE");
            assert!(!dir.path().join(format!("state/workspaces/{}/receipts/{request}.pending",scope.session_id)).exists());
            let mut core = engine.lock().unwrap();
            let session = crate::sessions::get(&mut core.content,&scope.session_id).unwrap();
            let root = crate::course_project::course_project_root(core.content.root(),session.course_id.as_ref().unwrap()).unwrap();
            assert!(!root.join("materials").join(path).exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn course_paths_reject_symlinks_reserved_paths_and_absolute_paths() {
        let (dir, engine, scope, _, _) = course_fixture("write");
        let saved = run(&engine,&scope,"seed","write",json!({"path":"seed.txt","content":"seed"})).unwrap();
        let root = dir.path().join(format!("library/courses/{}/materials",saved["courseId"].as_str().unwrap()));
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside,root.join("escape")).unwrap();
        for path in ["escape/new/deep.txt", "../outside.txt", "/tmp/outside.txt", ".git/config", "assets/new.txt", "Documents/forged.md", "ASSETS/new.txt"] {
            assert!(run(&engine,&scope,"invalid-path","write",json!({"path":path,"content":"x"})).is_err(),"{path}");
        }
        assert!(!outside.join("new").exists());
        assert!(!root.join("assets/new.txt").exists());
    }

    #[test]
    fn tier_off_rejects_authorized_tools_without_creating_a_workspace() {
        let (directory, engine, scope) = fixture("off");
        for (name, args) in [
            ("read", json!({"sourceId": scope.section_id.clone()})),
            ("write", json!({"path":"a.md","content":"x"})),
            ("bash", json!({"command":"echo hi"})),
        ] {
            let error = run(&engine, &scope, "off-call", name, args).unwrap_err();
            assert_eq!(error.code, "TOOL_NOT_ALLOWED", "{name} must stay off by default");
        }
        assert!(!directory.path().join("state/workspaces").exists());
    }

    #[test]
    fn read_tier_reads_frozen_authorized_sources_only() {
        let (directory, engine, scope) = fixture("read");
        let source = run(&engine, &scope, "read-source", "read", json!({"sourceId": scope.section_id.clone()})).unwrap();
        assert_eq!(source["sessionId"], scope.session_id.clone());
        assert_eq!(source["sourceId"], scope.section_id.clone());
        assert!(source["content"].as_str().unwrap().contains("since and for"));
        assert_eq!(source["truncated"], false);
        assert!(source["version"].is_string(), "version must be the real document version");
        // Not part of this run's authorized context.
        let error = run(&engine, &scope, "read-outside", "read", json!({"sourceId": "unrelated"})).unwrap_err();
        assert_eq!(error.code, "TOOL_SCOPE");
        // Version drift must not silently serve new content.
        {
            let mut core = engine.lock().unwrap();
            let mut document = core.content.read(&scope.section_id).unwrap();
            document.body.push_str(" appended");
            let serialized = crate::content::serialize(&document.metadata, &document.body).unwrap();
            core.content
                .save(&scope.section_id, &serialized, &document.version)
                .unwrap();
        }
        let error = run(&engine, &scope, "read-drift", "read", json!({"sourceId": scope.section_id.clone()})).unwrap_err();
        assert_eq!(error.code, "CONFLICT");
        // Path reads need an existing workspace file.
        let error = run(&engine, &scope, "read-missing", "read", json!({"path": "notes.md"})).unwrap_err();
        assert_eq!(error.code, "NOT_FOUND");
        // Read tier cannot write or execute.
        for (name, args) in [
            ("write", json!({"path":"a.md","content":"x"})),
            ("bash", json!({"command":"echo hi"})),
        ] {
            assert_eq!(run(&engine, &scope, "tier-gap", name, args).unwrap_err().code, "TOOL_NOT_ALLOWED");
        }
        drop(directory);
    }

    #[test]
    fn course_read_scope_is_separate_from_automatic_context_and_versioned() {
        let (_directory, engine, scope, source_id, sibling_id) = course_fixture("read");
        let result = run(
            &engine,
            &scope,
            "course-read-source",
            "read",
            json!({"sourceId":source_id}),
        )
        .unwrap();
        assert!(result["content"]
            .as_str()
            .unwrap()
            .contains("authorized source body"));
        assert_eq!(result["authorizationScope"], "course-read-manifest");
        assert_eq!(result["includedInInitialContext"], false);
        assert_eq!(result["delivery"], "tool-result");
        assert_eq!(
            result["includedChars"],
            result["content"].as_str().unwrap().chars().count()
        );
        assert_eq!(result["truncated"], false);
        assert_eq!(result["range"]["startChar"], 0);
        assert_eq!(result["range"]["endChar"], result["includedChars"]);
        assert_eq!(result["range"]["complete"], true);
        assert!(result["version"].is_string());
        assert!(crate::generation::get(&engine, &scope.run_id)
            .unwrap()
            .context_manifest
            .is_empty());

        let mut sibling_scope = scope.clone();
        sibling_scope.allowed_ids = vec![sibling_id.clone()];
        assert_eq!(
            run(
                &engine,
                &sibling_scope,
                "course-read-sibling",
                "read",
                json!({"sourceId":sibling_id}),
            )
            .unwrap_err()
            .code,
            "TOOL_SCOPE"
        );

        {
            let mut runs = engine.runs.lock().unwrap();
            runs.get_mut(&scope.run_id)
                .unwrap()
                .run
                .course_read_manifest
                .as_mut()
                .unwrap()[0]
                .version = "stale-version".into();
        }
        assert_eq!(
            run(
                &engine,
                &scope,
                "course-read-stale-version",
                "read",
                json!({"sourceId":source_id}),
            )
            .unwrap_err()
            .code,
            "CONFLICT"
        );

        engine
            .save_ui_preferences(UiPreferences {
                tool_authorization_tier: "off".into(),
                ..UiPreferences::default()
            })
            .unwrap();
        assert_eq!(
            run(
                &engine,
                &scope,
                "course-read-tier-off",
                "read",
                json!({"sourceId":source_id}),
            )
            .unwrap_err()
            .code,
            "TOOL_NOT_ALLOWED"
        );
    }

    fn read_replay_fixture() -> (tempfile::TempDir, Arc<Engine>, ToolScope, String, String) {
        let fixture = course_fixture("read");
        let question = {
            let mut core = fixture.1.lock().unwrap();
            let session = crate::sessions::append(&mut core.content, &fixture.2.session_id,
                crate::types::Entry { id: String::new(), kind: "user".into(), content: "Read source N".into(),
                    created_at: crate::types::timestamp(), selection: None, related_ids: vec![], data: None },
                None, "read-replay-saved-question").unwrap();
            session.entries.last().unwrap().id.clone()
        };
        let mut runs = fixture.1.runs.lock().unwrap();
        let run = &mut runs.get_mut(&fixture.2.run_id).unwrap().run;
        run.user_entry_id = Some(question);
        run.status = "failed".into();
        drop(runs);
        fixture
    }

    fn retry_read_run(engine: &Arc<Engine>, scope: &ToolScope, refresh: bool) -> ToolScope {
        let mut next = scope.clone();
        next.run_id = "run_read_retry".into();
        let mut frozen = crate::generation::get(engine, &scope.run_id).unwrap();
        frozen.id = next.run_id.clone();
        if refresh {
            let mut core = engine.lock().unwrap();
            let session = crate::sessions::get(&mut core.content, &scope.session_id).unwrap();
            frozen.course_read_manifest = Some(crate::context::course_read_manifest(
                &core.content, session.course_id.as_deref().unwrap()).unwrap());
            next.allowed_ids = frozen.read_scope_manifest().iter().map(|m| m.id.clone()).collect();
        }
        engine.runs.lock().unwrap().insert(next.run_id.clone(), crate::generation::ManagedRun {
            run: frozen, cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        next
    }

    fn assert_read_replay_boundary(change: &str, cross_run: bool) {
        let (_dir, engine, scope, source, sibling) = read_replay_fixture();
        let args = json!({"sourceId":source});
        let receipt = run(&engine, &scope, "read_before_failure", "read", args.clone()).unwrap();
        let request = crate::learning_tools::operation_id(&engine, &scope, "read_before_failure", "read", &args).unwrap().0;
        let ledger = super::workspace_root(&engine.lock().unwrap().state_dir, &scope.session_id)
            .join("receipts").join(format!("{request}.json"));
        let original = std::fs::read(&ledger).unwrap();
        {
            let mut core = engine.lock().unwrap();
            let document = core.content.read(&source).unwrap();
            match change {
                "move" => {
                    let owner = crate::course_sessions::course(&core.content, &sibling).unwrap().unwrap();
                    crate::library_tree::move_node(&mut core.content, &source, Some(&owner), 0, &document.version).unwrap();
                }
                "trash" => {
                    let preview = crate::trash::preview(&mut core.content, &source).unwrap();
                    crate::trash::trash(&mut core.content, &source, &preview.version, "replay-trash").unwrap();
                }
                "update" => { core.content.save(&source, &document.content.replace("authorized source body", "new version"), &document.version).unwrap(); }
                _ => {}
            }
        }
        // Same persisted question and args after a failed model turn; new runs
        // have a freshly captured directory, without changing the operation ID.
        let retry = if cross_run { retry_read_run(&engine, &scope, true) } else { scope.clone() };
        let replay = run(&engine, &retry, "read_after_failure", "read", args.clone());
        let call = crate::model::ToolCall { id: "read_before_failure".into(), name: "read".into(), args };
        let recovered = crate::native_session::recover(&engine, &scope.session_id, &scope.run_id, &call);
        let retry_recovered = crate::native_session::recover(&engine, &retry.session_id, &retry.run_id, &call);
        assert_eq!(std::fs::read(&ledger).unwrap(), original, "old receipt must remain immutable");
        if change == "unchanged" {
            assert_eq!(replay.unwrap()["version"], receipt["version"]);
            assert_eq!(recovered.unwrap()["version"], receipt["version"]);
            assert_eq!(retry_recovered.unwrap()["version"], receipt["version"]);
        } else {
            let expected = if change == "update" { "CONFLICT" } else { "TOOL_SCOPE" };
            assert_eq!(replay.unwrap_err().code, expected, "{change} cross_run={cross_run}");
            assert_eq!(recovered.unwrap_err().code, expected, "{change} original recovery");
            assert_eq!(retry_recovered.unwrap_err().code, expected, "{change} retry recovery");
        }
    }

    #[test]
    fn read_replay_cross_run_move() { assert_read_replay_boundary("move", true); }
    #[test]
    fn read_replay_cross_run_trash() { assert_read_replay_boundary("trash", true); }
    #[test]
    fn read_replay_same_run_update() { assert_read_replay_boundary("update", false); }
    #[test]
    fn read_replay_cross_run_update() { assert_read_replay_boundary("update", true); }
    #[test]
    fn read_replay_unchanged_cross_run() { assert_read_replay_boundary("unchanged", true); }

    #[test]
    fn read_replay_rechecks_workspace_and_current_tier() {
        for change in ["update", "delete", "tier"] {
            let (_dir, engine, scope) = fixture("write");
            let args = json!({"path":"draft.md"});
            run(&engine, &scope, "workspace_seed", "write", json!({"path":"draft.md","content":"original"})).unwrap();
            run(&engine, &scope, "workspace_read", "read", args.clone()).unwrap();
            let files = super::workspace_root(&engine.lock().unwrap().state_dir, &scope.session_id).join("files");
            match change {
                "update" => std::fs::write(files.join("draft.md"), "changed").unwrap(),
                "delete" => std::fs::remove_file(files.join("draft.md")).unwrap(),
                _ => { engine.save_ui_preferences(UiPreferences { tool_authorization_tier: "off".into(), ..UiPreferences::default() }).unwrap(); }
            }
            assert!(run(&engine, &scope, "workspace_read", "read", args.clone()).is_err(), "{change}");
            assert!(crate::native_session::recover(&engine, &scope.session_id, &scope.run_id,
                &crate::model::ToolCall { id: "workspace_read".into(), name: "read".into(), args }).is_err(), "{change}");
        }
    }

    #[test]
    fn final_fit_updates_read_character_count_and_truncation_flag() {
        let mut result = json!({
            "content":"🙂".repeat(20_000),
            "includedChars":20_000,
            "truncated":false,
            "range":{"startChar":0,"endChar":20_000,"complete":true,"totalBytes":80_000}
        });
        super::fit_result_bytes(&mut result).unwrap();
        assert!(result["truncated"].as_bool().unwrap());
        assert_eq!(
            result["includedChars"],
            result["content"].as_str().unwrap().chars().count()
        );
        assert_eq!(result["range"]["endChar"], result["includedChars"]);
        assert_eq!(result["range"]["complete"], false);
    }

    #[test]
    fn write_tier_confines_files_to_the_session_workspace() {
        let (directory, engine, scope) = fixture("write");
        let written = run(&engine, &scope, "write-note", "write", json!({"path":"notes/summary.md","content":"controlled scratch content"}),).unwrap();
        assert_eq!(written["persisted"], true);
        assert_eq!(written["path"], "notes/summary.md");
        assert_eq!(written["bytes"], "controlled scratch content".len());
        let file = directory
            .path()
            .join("state/workspaces")
            .join(&scope.session_id)
            .join("files/notes/summary.md");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "controlled scratch content");
        let read_back = run(&engine, &scope, "read-back", "read", json!({"path": "notes/summary.md"})).unwrap();
        assert_eq!(read_back["content"], "controlled scratch content");
        assert_eq!(read_back["truncated"], false);
        // Another session never sees this workspace.
        let other = {
            let mut core = engine.lock().unwrap();
            crate::sessions::create(&mut core.content, &scope.section_id, &[]).unwrap()
        };
        let mut foreign = scope.clone();
        foreign.session_id = other.id;
        // A foreign session's scope mismatches its real section binding first.
        let error = run(&engine, &foreign, "foreign-read", "read", json!({"path": "notes/summary.md"})).unwrap_err();
        assert_eq!(error.code, "TOOL_SCOPE");
        // Path validation rejects traversal, absolutes and unsafe segments.
        for bad in ["../escape.md", "/tmp/abs.md", "a/../../b.md", ".hidden", "a/b.md/", "", "notes/../more.md"] {
            let error = run(&engine, &scope, "bad-path", "write", json!({"path": bad, "content": "x"})).unwrap_err();
            assert_eq!(error.code, "INVALID_TOOL_INPUT", "path {bad:?} must be rejected");
        }
        // Write tier cannot execute commands.
        assert_eq!(run(&engine, &scope, "no-bash", "bash", json!({"command":"echo hi"})).unwrap_err().code, "TOOL_NOT_ALLOWED");
    }

    #[test]
    fn write_replays_receipt_and_conflicts_on_changed_arguments() {
        let (_directory, engine, scope) = fixture("write");
        let args = json!({"path":"draft.md","content":"first version"});
        let first = run(&engine, &scope, "same-call", "write", args.clone()).unwrap();
        assert_eq!(first.get("replayed"), None);
        let second = run(&engine, &scope, "same-call", "write", args.clone()).unwrap();
        assert_eq!(second["replayed"], true);
        assert_eq!(first["path"], second["path"]);
        let changed = json!({"path":"draft.md","content":"different content"});
        assert_eq!(run(&engine, &scope, "same-call", "write", changed).unwrap_err().code, "CONFLICT");
        // The conflicting write never landed.
        let current = run(&engine, &scope, "verify", "read", json!({"path": "draft.md"})).unwrap();
        assert_eq!(current["content"], "first version");
    }

    #[test]
    fn workspace_symlinks_cannot_escape_confinement() {
        let (directory, engine, scope) = fixture("write");
        let outside = directory.path().join("outside-secret.txt");
        std::fs::write(&outside, "PRIVATE_OUTSIDE").unwrap();
        let files = directory
            .path()
            .join("state/workspaces")
            .join(&scope.session_id)
            .join("files");
        std::fs::create_dir_all(&files).unwrap();
        std::os::unix::fs::symlink(&outside, files.join("escape.md")).unwrap();
        assert_eq!(
            run(&engine, &scope, "read-link", "read", json!({"path": "escape.md"})).unwrap_err().code,
            "INVALID_TOOL_INPUT"
        );
        assert_eq!(
            run(&engine, &scope, "write-link", "write", json!({"path": "escape.md", "content": "overwrite"})).unwrap_err().code,
            "INVALID_TOOL_INPUT"
        );
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "PRIVATE_OUTSIDE");
    }

    #[test]
    fn bash_tier_runs_bounded_commands_in_the_workspace_only() {
        let (directory, engine, scope) = fixture("bash");
        let hello = run(&engine, &scope, "bash-hello", "bash", json!({"command": "printf hello"})).unwrap();
        assert_eq!(hello["persisted"], true);
        assert_eq!(hello["stdout"], "hello");
        assert_eq!(hello["exitCode"], 0);
        assert_eq!(hello["timedOut"], false);
        // Minimal environment: no HOME/USER, fixed PATH.
        let env = run(&engine, &scope, "bash-env", "bash", json!({"command": "env"})).unwrap();
        let text = env["stdout"].as_str().unwrap();
        assert!(!text.contains("HOME="));
        assert!(!text.contains("USER="));
        assert!(text.contains("PATH=/usr/bin:/bin"));
        // The working directory is the session workspace files root.
        let pwd = run(&engine, &scope, "bash-pwd", "bash", json!({"command": "printf %s \"$PWD\""})).unwrap();
        let working_directory = pwd["stdout"].as_str().unwrap();
        assert!(
            working_directory.contains(&format!("workspaces/{}/files", scope.session_id)),
            "pwd {working_directory}"
        );
        // Writing via bash stays inside the workspace files root.
        run(&engine, &scope, "bash-touch", "bash", json!({"command": "printf data > from-bash.txt"})).unwrap();
        let read_back = run(&engine, &scope, "bash-verify", "read", json!({"path": "from-bash.txt"})).unwrap();
        assert_eq!(read_back["content"], "data");
        // Non-zero exits are honest results, not failures.
        let failed = run(&engine, &scope, "bash-exit", "bash", json!({"command": "exit 3"})).unwrap();
        assert_eq!(failed["exitCode"], 3);
        assert_eq!(failed["persisted"], true);
        // Output is bounded.
        let noisy = run(&engine, &scope, "bash-noisy", "bash", json!({"command": "printf 'x%.0s' $(seq 1 20000)"})).unwrap();
        assert_eq!(noisy["truncated"], true);
        assert!(noisy["stdout"].as_str().unwrap().len() < 20_000);
        // Timeouts stop the process group and stay honest.
        let started = std::time::Instant::now();
        let timeout = run(&engine, &scope, "bash-timeout", "bash", json!({"command": "sleep 5", "timeoutMs": 1000})).unwrap();
        assert_eq!(timeout["timedOut"], true);
        assert!(started.elapsed().as_secs() < 4, "timeout must not wait the full sleep");
        // Replay returns the ledgered receipt without re-execution.
        let replay = run(&engine, &scope, "bash-hello", "bash", json!({"command": "printf hello"})).unwrap();
        assert_eq!(replay["replayed"], true);
        assert_eq!(replay["stdout"], "hello");
        // Invalid arguments never start a process.
        for args in [json!({"command": ""}), json!({"command": "echo", "timeoutMs": 100}), json!({"command": "echo", "timeoutMs": 99000}), json!({"command": "echo hi", "extra": 1})] {
            assert_eq!(run(&engine, &scope, "bash-bad", "bash", args).unwrap_err().code, "INVALID_TOOL_INPUT");
        }
        drop(directory);
    }

    #[test]
    fn bash_receipts_survive_restart_for_recovery() {
        let (directory, engine, scope) = fixture("bash");
        let first = run(&engine, &scope, "stable-call", "bash", json!({"command": "printf stable"})).unwrap();
        drop(engine);
        let reopened = Engine::new(directory.path().join("library"), directory.path().join("state"))
            .unwrap();
        reopened
            .save_ui_preferences(UiPreferences {
                tool_authorization_tier: "bash".into(),
                ..UiPreferences::default()
            })
            .unwrap();
        let replay = run(&reopened, &scope, "stable-call", "bash", json!({"command": "printf stable"})).unwrap();
        assert_eq!(replay["replayed"], true);
        assert_eq!(replay["stdout"], first["stdout"]);
    }

    #[test]
    fn recovery_returns_ledgered_receipt_after_restart_without_reexecuting() {
        let (directory, engine, scope) = fixture("write");
        let call = crate::model::ToolCall {
            id: "stable-recovery".into(),
            name: "write".into(),
            args: json!({"path": "draft.md", "content": "first version"}),
        };
        let first = run(&engine, &scope, &call.id, "write", call.args.clone()).unwrap();
        assert_eq!(first["persisted"], true);
        let target = directory
            .path()
            .join("state/workspaces")
            .join(&scope.session_id)
            .join("files/draft.md");
        // A later local edit proves recovery replays the receipt, not the write.
        std::fs::write(&target, "first version + later edit").unwrap();
        drop(engine);
        let reopened = Engine::new(directory.path().join("library"), directory.path().join("state"))
            .unwrap();
        let recovered = crate::authorized_tools::recover(
            &reopened,
            &scope.session_id,
            "run_authorized_fixture",
            &call,
        )
        .unwrap();
        assert_eq!(recovered["recovered"], true);
        assert_eq!(recovered.get("replayed"), None, "recovery is not a replay");
        assert_eq!(recovered["path"], "draft.md");
        assert_eq!(recovered["bytes"], "first version".len());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "first version + later edit",
            "the ledgered write must not run again"
        );
    }

    #[test]
    fn recovery_conflicts_on_changed_arguments_and_reports_missing_ledger_honestly() {
        let (_directory, engine, scope) = fixture("write");
        let call = crate::model::ToolCall {
            id: "conflict-op".into(),
            name: "write".into(),
            args: json!({"path": "draft.md", "content": "first version"}),
        };
        run(&engine, &scope, &call.id, "write", call.args.clone()).unwrap();
        // Same request ID with different arguments conflicts with the committed record.
        let mut changed = call.clone();
        changed.args = json!({"path": "draft.md", "content": "different content"});
        let error = crate::authorized_tools::recover(
            &engine,
            &scope.session_id,
            "run_authorized_fixture",
            &changed,
        )
        .unwrap_err();
        assert_eq!(error.code, "CONFLICT");
        // The native session recovery path delegates authorized tools and must
        // surface the same verdict, not soften it to an unknown.
        let error = crate::native_session::recover(
            &engine,
            &scope.session_id,
            "run_authorized_fixture",
            &changed,
        )
        .unwrap_err();
        assert_eq!(error.code, "CONFLICT");
        // No ledger at all stays an honest unknown, never a guess.
        let absent = crate::model::ToolCall {
            id: "never-executed".into(),
            name: "bash".into(),
            args: json!({"command": "printf hi"}),
        };
        let error = crate::authorized_tools::recover(
            &engine,
            &scope.session_id,
            "run_authorized_fixture",
            &absent,
        )
        .unwrap_err();
        assert_eq!(error.code, "NATIVE_TOOL_UNKNOWN");
        // Neither path re-executed or touched the ledgered file.
        let current = run(&engine, &scope, "verify", "read", json!({"path": "draft.md"})).unwrap();
        assert_eq!(current["content"], "first version");
    }

    #[test]
    fn receipt_listing_is_read_only_and_shares_no_arguments() {
        let (_directory, engine, scope) = fixture("bash");
        run(&engine, &scope, "list-write", "write", json!({"path": "notes/draft.md", "content": "secret scratch"})).unwrap();
        run(&engine, &scope, "list-bash", "bash", json!({"command": "printf 'hidden command text'"})).unwrap();
        run(&engine, &scope, "list-read", "read", json!({"path": "notes/draft.md"})).unwrap();
        let summary = crate::authorized_tools::list_receipt_summaries(&engine, &scope.session_id).unwrap();
        let receipts = summary["receipts"].as_array().unwrap();
        assert_eq!(receipts.len(), 2, "read receipts stay out of the write/bash audit list");
        let text = serde_json::to_string(&receipts).unwrap();
        assert!(!text.contains("hidden command text"), "command text must never leave the ledger");
        assert!(!text.contains("secret scratch"), "file content must never leave the ledger");
        for receipt in receipts {
            assert!(["write", "bash"].contains(&receipt["tool"].as_str().unwrap()));
            assert!(crate::types::valid_id(receipt["requestId"].as_str().unwrap()));
            assert!(receipt["createdAt"].as_str().is_some_and(|t| !t.is_empty()));
        }
        let write = receipts.iter().find(|r| r["tool"] == "write").unwrap();
        assert_eq!(write["path"], "notes/draft.md");
        assert_eq!(write["status"], "persisted");
        // A session without tool receipts lists empty, and foreign sessions are rejected.
        let other = {
            let mut core = engine.lock().unwrap();
            crate::sessions::create(&mut core.content, &scope.section_id, &[]).unwrap()
        };
        let empty = crate::authorized_tools::list_receipt_summaries(&engine, &other.id).unwrap();
        assert_eq!(empty["receipts"].as_array().unwrap().len(), 0);
        assert!(crate::authorized_tools::list_receipt_summaries(&engine, "missing_session").is_err());
        assert!(crate::authorized_tools::list_receipt_summaries(&engine, "../escape").is_err());
    }

    #[test]
    fn receipt_listing_skips_corrupt_or_unsafe_ledger_files() {
        let (directory, engine, scope) = fixture("write");
        run(&engine, &scope, "skip-real", "write", json!({"path": "draft.md", "content": "kept"})).unwrap();
        let receipts = directory
            .path()
            .join("state/workspaces")
            .join(&scope.session_id)
            .join("receipts");
        std::fs::write(receipts.join("pi_tool_corrupt.json"), "{ not valid json").unwrap();
        std::fs::write(receipts.join("not-a-receipt.txt"), "ignored by suffix").unwrap();
        std::fs::write(receipts.join("pi_tool_oversize.json"), "x".repeat(1024 * 1024 + 1)).unwrap();
        let outside = directory.path().join("outside-secret.txt");
        std::fs::write(&outside, "PRIVATE").unwrap();
        std::os::unix::fs::symlink(&outside, receipts.join("pi_tool_link.json")).unwrap();
        let summary = crate::authorized_tools::list_receipt_summaries(&engine, &scope.session_id).unwrap();
        let list = summary["receipts"].as_array().unwrap();
        assert_eq!(list.len(), 1, "only the structurally valid receipt is listed");
        assert_eq!(list[0]["path"], "draft.md");
        assert_eq!(list[0]["status"], "persisted");
    }

    #[test]
    fn receipt_listing_caps_at_two_hundred_and_marks_truncation() {
        let (directory, engine, scope) = fixture("write");
        let receipts = directory
            .path()
            .join("state/workspaces")
            .join(&scope.session_id)
            .join("receipts");
        std::fs::create_dir_all(&receipts).unwrap();
        for index in 0..201 {
            let entry = super::LedgerEntry {
                name: "write".into(),
                arguments_hash: format!("hash-{index}"),
                result: json!({"persisted": true, "path": format!("f{index}.md")}),
                created_at: format!("2026-10-17T{:02}:{:02}:00.000Z", index / 60, index % 60),
            };
            crate::app::atomic_json(&receipts.join(format!("pi_tool_cap{index:03}.json")), &entry)
                .unwrap();
        }
        let summary = crate::authorized_tools::list_receipt_summaries(&engine, &scope.session_id).unwrap();
        let list = summary["receipts"].as_array().unwrap();
        assert_eq!(list.len(), 200);
        assert_eq!(summary["truncated"], true);
        // Newest first; only the oldest entry falls off the cap.
        assert_eq!(list[0]["createdAt"], "2026-10-17T03:20:00.000Z");
        assert_eq!(list[0]["path"], "f200.md");
        assert_eq!(list[199]["createdAt"], "2026-10-17T00:01:00.000Z");
        assert!(
            !list.iter().any(|item| item["createdAt"] == "2026-10-17T00:00:00.000Z"),
            "the oldest receipt is the truncated one"
        );
    }

    #[test]
    fn authorized_tools_require_a_real_matching_session() {
        let (_directory, engine, scope) = fixture("bash");
        let mut forged = scope.clone();
        forged.session_id = "missing_session".into();
        assert!(run(&engine, &forged, "forged-session", "bash", json!({"command": "echo hi"})).is_err());
        let mut crossed = scope.clone();
        crossed.section_id = "unrelated".into();
        assert_eq!(
            run(&engine, &crossed, "cross-section", "write", json!({"path":"a.md","content":"x"})).unwrap_err().code,
            "TOOL_SCOPE"
        );
    }

    #[test]
    fn write_quota_bounds_disk_usage() {
        let (_directory, engine, scope) = fixture("write");
        for index in 0..crate::authorized_tools::MAX_WORKSPACE_FILES {
            run(&engine, &scope, &format!("quota-{index}"), "write", json!({"path": format!("f{index}.md"), "content": "x"})).unwrap();
        }
        let error = run(&engine, &scope, "quota-over", "write", json!({"path": "over.md", "content": "x"})).unwrap_err();
        assert_eq!(error.code, "LIMIT_EXCEEDED");
        // Replaying an already-ledgered write still works at the quota boundary.
        let replay = run(&engine, &scope, "quota-0", "write", json!({"path": "f0.md", "content": "x"})).unwrap();
        assert_eq!(replay["replayed"], true);
        let _ = std::io::stdout().flush();
    }
}

#[cfg(test)]
mod m2_independent_review {
    use crate::{app::{Engine,UiPreferences}, library_tree, learning_tools::ToolScope, types::*};
    use serde_json::json;
    #[test]
    fn second_note_tools_keep_frozen_scope_receipts_and_sources() {
        let d=tempfile::tempdir().unwrap();
        let e=Engine::new(d.path().join("library"),d.path().join("state")).unwrap();
        e.save_ui_preferences(UiPreferences{tool_authorization_tier:"read".into(),..UiPreferences::default()}).unwrap();
        let scope={
            let mut c=e.lock().unwrap();
            let owner=library_tree::create(&mut c.content,"course","Owner",None,"","tool-owner").unwrap();
            let a=library_tree::create(&mut c.content,"section","A",Some(&owner.id),"A content","tool-note-a").unwrap();
            let b=library_tree::create(&mut c.content,"section","B",Some(&owner.id),"B content","tool-note-b").unwrap();
            let s=c.start_session(&a.id,false).unwrap().0;
            let ctx=c.session_context_for(&s.id,Some(&b.id),None).unwrap().1;
            let run: crate::generation::Run=serde_json::from_value(json!({
                "id":"tool_run_second_note","sessionId":s.id,"sourceDocumentId":b.id,"mode":"question","status":"succeeded",
                "content":"","error":null,"createdAt":timestamp(),"updatedAt":timestamp(),
                "requestDigest":"fixture","contextSnapshotVersion":1,"contextManifest":ctx.manifest
            })).unwrap();
            e.runs.lock().unwrap().insert(run.id.clone(),crate::generation::ManagedRun{run,cancel:std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))});
            ToolScope{session_id:s.id,section_id:b.id,run_id:"tool_run_second_note".into(),allowed_ids:ctx.manifest.into_iter().map(|m|m.id).collect()}
        };
        let r=super::execute(&e,&scope,"read-second-note","read",json!({"sourceId":scope.section_id}));
        println!("authorized read on second note: {:?}",r);
        assert!(r.is_ok(),"authorized source read must follow frozen focus, not session creation anchor");
        e.save_ui_preferences(UiPreferences{tool_authorization_tier:"write".into(),..UiPreferences::default()}).unwrap();
        let written=super::execute(&e,&scope,"write-second-note","write",json!({"path":"answer.txt","content":"synthetic B"})).unwrap();
        assert_eq!(written["persisted"],true);
        assert!(written["proposalId"].is_null());
        assert_eq!(super::execute(&e,&scope,"write-second-note","write",json!({"path":"answer.txt","content":"synthetic B"})).unwrap()["replayed"],true);
        assert!(super::execute(&e,&scope,"write-escape-note","write",json!({"path":"../escape","content":"x"})).is_err());
        e.save_ui_preferences(UiPreferences{tool_authorization_tier:"bash".into(),..UiPreferences::default()}).unwrap();
        assert_eq!(super::execute(&e,&scope,"bash-second-note","bash",json!({"command":"printf synthetic"})).unwrap()["stdout"],"synthetic");
        let invalid={let mut c=e.lock().unwrap();let other=library_tree::create(&mut c.content,"section","Not frozen",None,"other","not-frozen-tool-note").unwrap();ToolScope{section_id:other.id,..scope.clone()}};
        assert_eq!(super::execute(&e,&invalid,"wrong-focus-read","read",json!({"sourceId":invalid.section_id})).unwrap_err().code,"TOOL_SCOPE");
        let mut non_frozen=scope.clone();non_frozen.allowed_ids.push("not_in_manifest".into());
        assert_eq!(super::execute(&e,&non_frozen,"wrong-frozen-write","write",json!({"path":"x.txt","content":"x"})).unwrap_err().code,"TOOL_SCOPE");
        let anchor={let mut c=e.lock().unwrap();crate::sessions::get(&mut c.content,&scope.session_id).unwrap().section_id};
        let old_focus=ToolScope{section_id:anchor,..scope.clone()};
        assert_eq!(super::execute(&e,&old_focus,"non-frozen-course-focus","read",json!({"sourceId":old_focus.section_id})).unwrap_err().code,"TOOL_SCOPE");
        for (name,args) in [
            ("create_note",json!({"content":"Synthetic B note"})),
            ("create_todo",json!({"title":"Synthetic B todo"})),
            ("generate_exercises",json!({"requirements":"B practice","exercises":[{"question":"B?","format":"exact","answers":["B"],"explanation":"B explanation","tags":[],"sourceIds":[scope.section_id]}],"summary":"B"})),
        ] {
            let call=crate::model::ToolCall{id:format!("second-note-{name}"),name:name.into(),args};
            let saved=crate::learning_tools::execute(&e,&scope,&call.id,name,call.args.clone()).unwrap();
            assert_eq!(saved["sectionId"],scope.section_id);
            let recovered=crate::native_session::recover(&e,&scope.session_id,&scope.run_id,&call).unwrap();
            assert_eq!(recovered["sectionId"],scope.section_id);
            assert_eq!(recovered["recovered"],true);
        }
        e.save_ui_preferences(UiPreferences::default()).unwrap();
        assert_eq!(super::execute(&e,&scope,"revoked-second-note","read",json!({"sourceId":scope.section_id})).unwrap_err().code,"TOOL_NOT_ALLOWED");
    }
}
