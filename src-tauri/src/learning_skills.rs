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
fn validate_content(id: &str, content: &str) -> Result<()> {
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
    if name != Some(id)
        || description.is_none_or(|value| value.trim().is_empty())
        || body.trim().is_empty()
    {
        return Err(invalid());
    }
    Ok(())
}
pub fn capture(mode: &str, tools: &[String]) -> Result<Vec<SkillSnapshot>> {
    let directory = SKILL_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills"));
    capture_from(&directory, mode, tools)
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
        .map(|(id, scope)| {
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
            let content = fs::read_to_string(path).map_err(|_| {
                CoreError::new("SKILL_IO", "学习指导文件暂时无法读取，未发送模型请求。")
            })?;
            validate_content(id, &content)?;
            Ok(SkillSnapshot {
                id: id.into(),
                sha256: hash(&content),
                scope: scope.into(),
                content,
            })
        })
        .collect()
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
}
