//! Versioned application instructions, captured before a model run; never user material.
use crate::types::{hash, CoreError, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

static SKILL_DIR: OnceLock<PathBuf> = OnceLock::new();
const MAX_SKILL_BYTES: u64 = 16 * 1024;
/// The fixed bundled allowlist. On-demand listing/loading never discovers other
/// directories; an id outside this list is an authorization rejection.
pub const KNOWN_IDS: &[&str] = &["learning-explore", "learning-exercises", "learning-review"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillVersion {
    pub id: String,
    pub sha256: String,
    pub scope: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillSnapshot {
    pub id: String,
    pub sha256: String,
    pub scope: String,
    pub content: String,
}
impl SkillSnapshot {
    pub fn version(&self) -> SkillVersion {
        SkillVersion {
            id: self.id.clone(),
            sha256: self.sha256.clone(),
            scope: self.scope.clone(),
        }
    }
}
/// Frontmatter metadata only; the on-demand list tool never returns bodies.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillListing {
    pub id: String,
    pub name: String,
    pub description: String,
}

pub fn set_resource_dir(path: PathBuf) -> Result<()> {
    SKILL_DIR
        .set(path)
        .map_err(|_| CoreError::new("SKILL_INVALID", "学习指导路径已设置。"))
}
fn required(mode: &str, tools: &[String]) -> Result<Vec<(&'static str, &'static str)>> {
    let mut items = match mode {
        "question" => vec![("learning-explore", "mode")],
        "practice" => vec![("learning-exercises", "mode")],
        "review" => vec![("learning-review", "mode")],
        "explain" | "feedback" | "grading" | "legacy" => vec![],
        _ => return Err(CoreError::new("SKILL_INVALID", "学习指导模式无效。")),
    };
    if tools.iter().any(|tool| tool == "generate_exercises")
        && !items.iter().any(|(id, _)| *id == "learning-exercises")
    {
        items.push(("learning-exercises", "tool:generate_exercises"));
    }
    Ok(items)
}
fn validate_content(id: &str, content: &str) -> Result<(String, String)> {
    let invalid = || {
        CoreError::new(
            "SKILL_INVALID",
            "学习指导文件格式无效，请恢复随包文件后重试。",
        )
    };
    if content.is_empty() || content.len() as u64 > MAX_SKILL_BYTES || content.contains('\0') {
        return Err(invalid());
    }
    let rest = content.strip_prefix("---\n").ok_or_else(invalid)?;
    let (header, body) = rest.split_once("\n---\n").ok_or_else(invalid)?;
    let mut name = None;
    let mut description = None;
    for line in header.lines() {
        if let Some(value) = line.strip_prefix("name: ") {
            if name.replace(value).is_some() {
                return Err(invalid());
            }
        } else if let Some(value) = line.strip_prefix("description: ") {
            if description.replace(value).is_some() {
                return Err(invalid());
            }
        } else {
            return Err(invalid());
        }
    }
    let description = description.filter(|value| !value.trim().is_empty());
    match (name, description) {
        (Some(name), Some(description)) if name == id && !body.trim().is_empty() => {
            Ok((name.to_owned(), description.to_owned()))
        }
        _ => Err(invalid()),
    }
}
pub fn capture(mode: &str, tools: &[String]) -> Result<Vec<SkillSnapshot>> {
    capture_from(&bundled_dir(), mode, tools)
}
fn bundled_dir() -> PathBuf {
    SKILL_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills"))
}
/// The single path-and-format gate shared by proactive capture and on-demand load.
fn load_validated(root: &Path, id: &str, scope: &str) -> Result<SkillSnapshot> {
    let path = root.join(id).join("SKILL.md");
    let meta = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CoreError::new(
                "SKILL_MISSING",
                "本次需要的学习指导文件缺失，未发送模型请求。",
            )
        } else {
            CoreError::new("SKILL_IO", "学习指导文件暂时无法读取，未发送模型请求。")
        }
    })?;
    if !meta.is_file()
        || meta.file_type().is_symlink()
        || meta.len() > MAX_SKILL_BYTES
        || !fs::canonicalize(&path).is_ok_and(|resolved| resolved.starts_with(&root))
    {
        return Err(CoreError::new(
            "SKILL_INVALID",
            "学习指导文件不是有效的随包文本。",
        ));
    }
    let content = fs::read_to_string(path)
        .map_err(|_| CoreError::new("SKILL_IO", "学习指导文件暂时无法读取，未发送模型请求。"))?;
    validate_content(id, &content)?;
    Ok(SkillSnapshot {
        id: id.into(),
        sha256: hash(&content),
        scope: scope.into(),
        content,
    })
}
pub fn capture_from(directory: &Path, mode: &str, tools: &[String]) -> Result<Vec<SkillSnapshot>> {
    let required = required(mode, tools)?;
    if required.is_empty() {
        return Ok(vec![]);
    }
    let root = fs::canonicalize(directory).map_err(|_| {
        CoreError::new(
            "SKILL_MISSING",
            "本次需要的学习指导文件缺失，未发送模型请求。",
        )
    })?;
    required
        .into_iter()
        .map(|(id, scope)| load_validated(&root, id, scope))
        .collect()
}
/// On-demand metadata for the fixed allowlist. Entries failing the shared gate
/// are omitted, so the listing only advertises guidance that can be loaded.
pub fn catalog_from(directory: &Path) -> Vec<SkillListing> {
    let Ok(root) = fs::canonicalize(directory) else {
        return vec![];
    };
    KNOWN_IDS
        .iter()
        .filter_map(|id| {
            let snapshot = load_validated(&root, id, "on-demand").ok()?;
            let (name, description) = validate_content(id, &snapshot.content).ok()?;
            Some(SkillListing {
                id: (*id).into(),
                name,
                description,
            })
        })
        .collect()
}
pub fn catalog() -> Vec<SkillListing> {
    catalog_from(&bundled_dir())
}
/// On-demand body for one allowlisted id; identical checks to proactive capture.
pub fn load(id: &str) -> Result<SkillSnapshot> {
    let root = fs::canonicalize(bundled_dir()).map_err(|_| {
        CoreError::new(
            "SKILL_MISSING",
            "本次需要的学习指导文件缺失，未发送模型请求。",
        )
    })?;
    load_validated(&root, id, "on-demand")
}
pub fn validate(mode: &str, tools: &[String], snapshots: &[SkillSnapshot]) -> Result<()> {
    let required = required(mode, tools)?;
    if required.len() != snapshots.len() {
        return Err(CoreError::new(
            "SKILL_MISSING",
            "本次学习指导快照不完整，未发送模型请求。",
        ));
    }
    for ((id, scope), snapshot) in required.iter().zip(snapshots) {
        validate_content(id, &snapshot.content)?;
        if snapshot.id != *id
            || snapshot.scope != *scope
            || snapshot.sha256 != hash(&snapshot.content)
        {
            return Err(CoreError::new(
                "SKILL_INVALID",
                "学习指导快照与本次模式或版本不一致。",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write(root: &Path, id: &str, body: &str) {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {id}\ndescription: 明确模式的学习指导\n---\n{body}\n"),
        )
        .unwrap();
    }
    #[test]
    fn mode_and_tool_load_only_their_guidance_without_duplicates() {
        let temp = tempfile::tempdir().unwrap();
        for id in ["learning-explore", "learning-review", "learning-exercises"] {
            write(temp.path(), id, "指导正文");
        }
        let tools = vec!["generate_exercises".into()];
        let explore = capture_from(temp.path(), "question", &tools).unwrap();
        assert_eq!(
            explore.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["learning-explore", "learning-exercises"]
        );
        assert_eq!(explore[1].scope, "tool:generate_exercises");
        let practice = capture_from(temp.path(), "practice", &tools).unwrap();
        assert_eq!(practice.len(), 1);
        assert_eq!(practice[0].scope, "mode");
        let review = capture_from(temp.path(), "review", &[]).unwrap();
        assert_eq!(review.len(), 1);
        assert_eq!(review[0].id, "learning-review");
        assert!(
            capture_from(Path::new("/not-a-skill-root"), "feedback", &[])
                .unwrap()
                .is_empty()
        );
        assert!(capture_from(Path::new("/not-a-skill-root"), "legacy", &[])
            .unwrap()
            .is_empty());
    }
    #[test]
    fn missing_malformed_or_outside_files_never_fall_back() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            capture_from(temp.path(), "question", &[]).unwrap_err().code,
            "SKILL_MISSING"
        );
        write(temp.path(), "learning-explore", "正文");
        fs::write(
            temp.path().join("learning-explore/SKILL.md"),
            "---\nname: wrong\ndescription: 错误\n---\n正文",
        )
        .unwrap();
        assert_eq!(
            capture_from(temp.path(), "question", &[]).unwrap_err().code,
            "SKILL_INVALID"
        );
        #[cfg(unix)]
        {
            fs::remove_file(temp.path().join("learning-explore/SKILL.md")).unwrap();
            std::os::unix::fs::symlink("/etc/hosts", temp.path().join("learning-explore/SKILL.md"))
                .unwrap();
            assert_eq!(
                capture_from(temp.path(), "question", &[]).unwrap_err().code,
                "SKILL_INVALID"
            );
        }
    }
    #[test]
    fn captured_version_remains_fixed_and_cross_mode_or_tampering_fails() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "learning-explore", "第一版指导");
        let old = capture_from(temp.path(), "question", &[]).unwrap();
        write(temp.path(), "learning-explore", "第二版指导");
        let new = capture_from(temp.path(), "question", &[]).unwrap();
        assert_ne!(old[0].sha256, new[0].sha256);
        assert!(old[0].content.contains("第一版"));
        validate("question", &[], &old).unwrap();
        assert_eq!(
            validate("review", &[], &old).unwrap_err().code,
            "SKILL_INVALID"
        );
        let mut changed = old.clone();
        changed[0].content.push('改');
        assert_eq!(
            validate("question", &[], &changed).unwrap_err().code,
            "SKILL_INVALID"
        );
        let mut duplicate = old.clone();
        duplicate.push(old[0].clone());
        assert_eq!(
            validate("question", &[], &duplicate).unwrap_err().code,
            "SKILL_MISSING"
        );
    }
    #[test]
    fn catalog_lists_only_allowlisted_metadata_without_bodies() {
        let temp = tempfile::tempdir().unwrap();
        for id in KNOWN_IDS {
            write(temp.path(), id, "指导正文");
        }
        // An id outside the fixed allowlist is never listed even when present.
        write(temp.path(), "rogue-skill", "未列入清单的内容");
        let listed = catalog_from(temp.path());
        assert_eq!(
            listed.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            KNOWN_IDS
        );
        for item in &listed {
            assert_eq!(item.name, item.id);
            assert!(!item.description.trim().is_empty());
        }
        assert!(!format!("{listed:?}").contains("指导正文"));
        fs::remove_file(temp.path().join("learning-review/SKILL.md")).unwrap();
        assert_eq!(
            catalog_from(temp.path())
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["learning-explore", "learning-exercises"]
        );
        assert!(catalog_from(Path::new("/not-a-skill-root")).is_empty());
    }
    #[test]
    fn on_demand_load_reuses_the_same_gate_as_proactive_capture() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "learning-explore", "按需正文");
        let snapshot = capture_from(temp.path(), "question", &[]).unwrap()[0].clone();
        let loaded = load_validated(
            &fs::canonicalize(temp.path()).unwrap(),
            "learning-explore",
            "on-demand",
        )
        .unwrap();
        assert_eq!(loaded.content, snapshot.content);
        assert_eq!(loaded.sha256, snapshot.sha256);
        #[cfg(unix)]
        {
            fs::remove_file(temp.path().join("learning-explore/SKILL.md")).unwrap();
            std::os::unix::fs::symlink("/etc/hosts", temp.path().join("learning-explore/SKILL.md"))
                .unwrap();
            assert_eq!(
                load_validated(
                    &fs::canonicalize(temp.path()).unwrap(),
                    "learning-explore",
                    "on-demand"
                )
                .unwrap_err()
                .code,
                "SKILL_INVALID"
            );
        }
    }
}
