//! Fixed-object Changes projection. The caller supplies the submission's checkpoint;
//! this module never captures a worktree, commits, or records Pi acceptance.
use crate::{material_git::MaterialGit, types::*};
use serde_json::{json, Value};

/// `source_id` is already authorized by the submission, not inferred from a path.
/// Only text permitted for automatic projection may be supplied (not answer keys).
pub fn prepare(
    git: &MaterialGit,
    course: &str,
    commit: &str,
    previous: Option<&str>,
    focus_previous: Option<&str>,
    focus: Option<(&str, &str)>,
    budget: usize,
) -> Result<Value> {
    let checkpoint = git.checkpoint(course, commit)?;
    let old = previous
        .map(|oid| git.checkpoint(course, oid))
        .transpose()?;
    let mut diff = git.compare(course, previous, commit, None)?;
    if let (Some(old), Some((_, path))) = (focus_previous, focus) {
        let focused_diff = git.compare(course, Some(old), commit, Some(path))?;
        if let Some(change) = focused_diff.changes.into_iter().find(|c| c.path == path) {
            diff.changes.retain(|c| c.path != path);
            diff.changes.push(change);
        }
    }
    let mut changes = Vec::new();
    for change in &diff.changes {
        let mut value = serde_json::to_value(change)?;
        // Raw bytes remain in Git and in the native compare API. A model text
        // projection must not replace them with U+FFFD, base64, or an empty patch.
        value.as_object_mut().unwrap().remove("patchBytes");
        if let Some(bytes) = &change.patch_bytes {
            value["patchUnavailable"] = json!("non_utf8");
            value["patchByteLength"] = json!(bytes.len());
        }
        let readable_commit = if change.status == "D" {
            previous.unwrap_or(commit)
        } else {
            commit
        };
        value["fixedVersion"] =
            json!({"courseId":course,"checkpointCommitOid":readable_commit,"path":change.path});
        if focus.is_some_and(|(_, path)| path == change.path) && (change.patch.is_some() || change.patch_bytes.is_some()) {
            value["patchBaseCommitOid"] = json!(focus_previous);
        }
        changes.push(value);
    }
    let mut focused = Value::Null;
    if let Some((id, path)) = focus {
        if !valid_id(id) {
            return Err(CoreError::new("MATERIAL_SCOPE", "聚焦来源无效。"));
        }
        let file = git
            .files(course, commit)?
            .into_iter()
            .find(|f| f.path == path)
            .ok_or_else(|| CoreError::new("MATERIAL_SCOPE", "聚焦材料不在本次固定版本。"))?;
        let bytes = git.read(course, commit, path)?;
        focused = json!({"id":id,"path":path,"gitCommitOid":commit,"gitBlobOid":file.blob_oid,"totalBytes":bytes.len()});
        let binary = bytes.iter().take(8000).any(|b| *b == 0);
        match String::from_utf8(bytes) {
            Ok(_) if binary => {
                focused["textUnavailable"] = json!("binary");
            }
            Ok(text) => {
                let version = hash(&text);
                focused["version"] = json!(&version);
                focused["textHash"] = json!(version);
                focused["range"] = json!({"startChar":0,"endChar":text.chars().count(),"complete":true,"totalBytes":text.len()});
                focused["text"] = json!(text);
                if let Some(change) = diff
                    .changes
                    .iter()
                    .find(|c| c.path == path && c.patch.is_some())
                {
                    if let Some(old_oid) = focus_previous {
                        if let Some(base) = git
                            .files(course, old_oid)?
                            .into_iter()
                            .find(|f| f.path == path)
                        {
                            focused["gitChange"] = json!({"baseCommitOid":old_oid,"baseBlobOid":base.blob_oid,"unifiedDiff":change.patch});
                        }
                    }
                }
            }
            Err(_) => {
                focused["textUnavailable"] = json!("non_utf8");
            }
        }
    }
    let exclusions_changed = old
        .as_ref()
        .is_none_or(|c| c.excluded_files != checkpoint.excluded_files);
    let value = json!({"checkpoint":checkpoint,"baseCommitOid":previous,"initial":diff.initial,
        "changes":changes,"exclusionsChanged":exclusions_changed,"focused":focused,"complete":true});
    if serde_json::to_vec(&value)?.len() > budget {
        return Err(CoreError::new(
            "LIMIT_EXCEEDED",
            "完整 Git 变化及必要正文超过发送容量；未截断，未确认本次接收基准。",
        ));
    }
    Ok(value)
}

#[cfg(test)]
#[path = "material_changes_tests.rs"]
mod tests;
