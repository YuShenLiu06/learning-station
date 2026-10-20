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
    let state_dir = {
        let mut core = engine.lock().map_err(|_| unknown())?;
        if crate::sessions::get(&mut core.content, session_id).is_err() {
            return Err(unknown());
        }
        core.state_dir.clone()
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
    let entry = read_ledger(&ledger_path).map_err(|_| unknown())?;
    let arguments_hash = hash(&serde_json::to_string(&call.args).map_err(|_| unknown())?);
    if entry.name != call.name || entry.arguments_hash != arguments_hash {
        // A ledger exists but for different arguments: that is a conflict with
        // committed evidence, not an unknown outcome.
        return Err(CoreError::new(
            "CONFLICT",
            "原工具记录与本次参数不一致，未重复执行。",
        ));
    }
    let mut result = entry.result;
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
            .filter(|path| valid_relative_path(path));
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
        let mut result = entry.result;
        result["replayed"] = json!(true);
        return Ok(result);
    }
    let mut result = match name {
        "read" => execute_read(engine, scope, decode::<ReadArgs>(args)?),
        "write" => execute_write(&state_dir, scope, decode::<WriteArgs>(args)?),
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
    crate::app::atomic_json(&ledger_path, &entry)?;
    Ok(result)
}

fn execute_read(
    engine: &crate::app::Engine,
    scope: &ToolScope,
    args: ReadArgs
) -> Result<Value> {
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
            let session = crate::sessions::get(&mut core.content, &scope.session_id)
                .map_err(|_| out_of_scope())?;
            if core.content.is_trashed(&scope.session_id) {
                return Err(out_of_scope());
            }
            let course_scope = session.course_id.is_some();
            if course_scope
                && crate::course_sessions::source(&core.content, &session, &source_id)
                    .map_err(|_| out_of_scope())?
                    != source_id
            {
                return Err(out_of_scope());
            }
            core.content.read(&source_id)?
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
            context_item.id == source_id && context_item.version == item.version
        });
        Ok(json!({"sessionId":scope.session_id,"sourceId":source_id,"path":document.path,
            "title":document.metadata.title,"kind":document.metadata.kind,"version":document.version,
            "includedChars":included_chars,"content":content.0,"truncated":content.1,
            "manifestRole":"user-reference","includedInInitialContext":included_in_initial_context,
            "authorizationScope":if run.course_read_manifest.is_some() {"course-read-manifest"} else {"run-context-manifest"},
            "delivery":"tool-result","range":{"startChar":0,"endChar":included_chars,"complete":!content.1,"totalBytes":document.content.len()}}),)
    } else {
        let path = args.path.as_deref().ok_or_else(invalid)?;
        if !valid_relative_path(path) {
            return Err(invalid());
        }
        let files_root = ensure_workspace_files(engine, scope)?;
        let target = resolve_existing(&files_root, path)?;
        let bytes = fs::read(&target).map_err(io)?;
        let content = bounded_text(&String::from_utf8_lossy(&bytes), MAX_READ_BYTES);
        Ok(json!({"sessionId":scope.session_id,"path":path,"bytes":bytes.len(),
            "content":content.0,"truncated":content.1}))
    }
}

fn execute_write(
    state_dir: &Path,
    scope: &ToolScope,
    args: WriteArgs,
) -> Result<Value> {
    if !valid_relative_path(&args.path) || args.content.chars().count() > MAX_CONTENT_CHARS {
        return Err(invalid());
    }
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
        crate::authorized_tools::execute(engine, scope, call, name, args)
    }

    use std::sync::Arc;

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
