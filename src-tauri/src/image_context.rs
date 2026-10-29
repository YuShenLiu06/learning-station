//! Freeze explicitly referenced course images; retries validate the original versions.
use crate::{assets, content::ContentStore, types::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs, path::Path};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Reference { id: String, version: String }

pub(crate) fn capture(
    store: &ContentStore, state: &Path, session: &Session, user_id: &str,
    prompt: &str, context: &Context, readable: Option<&[ManifestItem]>,
) -> Result<Vec<Value>> {
    let conflict = || CoreError::new("CONTEXT_CONFLICT", "图片来源或版本已变化，请保留原问题并重新发送。");
    let dir = state.join("image-snapshots");
    match fs::symlink_metadata(&dir) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => return Err(conflict()),
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir_all(&dir)?,
        Err(e) => return Err(e.into()),
    }
    let path = dir.join(format!("{}.json", hash(&format!("{}:{user_id}", session.id))));
    let saved: Option<Vec<Reference>> = match fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_symlink() || !m.is_file() || m.len() > 4096 => return Err(conflict()),
        Ok(_) => Some(serde_json::from_slice(&assets::bounded_read(&path, 4096)?).map_err(|_| conflict())?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    // Retrying an accepted user never discovers newly edited note references.
    let ids: Vec<String> = if let Some(ref saved) = saved {
        if saved.len() > 4 { return Err(conflict()); }
        saved.iter().map(|r| r.id.clone()).collect()
    } else {
        let mut seen = BTreeSet::new();
        crate::context::image_ids(prompt).into_iter()
            .chain(crate::context::image_ids(&context.text))
            .filter(|id| seen.insert(id.clone())).collect()
    };
    let mut references = Vec::new();
    let mut images = Vec::new();
    for id in ids {
        let course = session.course_id.as_deref().ok_or_else(|| CoreError::new("SOURCE_REQUIRED", "请在课程对话中添加图片。"))?;
        let preview = assets::read(store, course, &id)?;
        let Some(data_url) = preview.data_url else { continue; };
        if readable.is_some_and(|items| !items.iter().any(|m| m.id == id && m.version == preview.asset.sha256)) {
            return Err(conflict());
        }
        if images.len() == 4 {
            return Err(CoreError::new("IMAGE_LIMIT", "每次最多发送4张图片，每张最多5MiB。"));
        }
        let (_, data) = data_url.split_once(',').ok_or_else(conflict)?;
        images.push(json!({"type":"image", "mimeType":preview.asset.mime, "data":data, "sourceId":id}));
        references.push(Reference { id, version: preview.asset.sha256 });
    }
    if let Some(saved) = saved {
        if saved != references { return Err(conflict()); }
    } else {
        crate::app::atomic_json(&path, &references)?;
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::Engine, library_tree};
    #[test]
    fn freezes_references_and_rejects_cross_course_or_changed_versions() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut core = engine.lock().unwrap();
        let course = library_tree::create(&mut core.content, "course", "Images", None, "", "image-course-1").unwrap();
        let note = library_tree::create(&mut core.content, "section", "Note", Some(&course.id), "Text", "image-note-1").unwrap();
        let (session, _) = core.start_session(&note.id, false).unwrap();
        let context = crate::context::load(&core.content, &note.id, None).unwrap();
        let image = assets::import(&core.content, &course.id, "one.png", b"\x89PNG\r\n\x1a\n", "image-import-1").unwrap();
        let question = format!("Explain ![one](asset:{})", image.id);
        let readable = crate::context::course_read_manifest(&core.content, &course.id).unwrap();
        let first = capture(&core.content, &core.state_dir, &session, "user-1", &question, &context, Some(&readable)).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0]["mimeType"], "image/png");
        // The original image snapshot is reused even if current note text changes.
        assert_eq!(capture(&core.content, &core.state_dir, &session, "user-1", "changed current text", &context, Some(&readable)).unwrap(), first);
        assert!(capture(&core.content, &core.state_dir, &session, "user-empty", "No image", &context, Some(&readable)).unwrap().is_empty());
        let mut different = readable.clone();
        different.iter_mut().find(|m| m.id == image.id).unwrap().version = "changed".into();
        assert_eq!(capture(&core.content, &core.state_dir, &session, "user-1", &question, &context, Some(&different)).unwrap_err().code, "CONTEXT_CONFLICT");
        let other = library_tree::create(&mut core.content, "course", "Other", None, "", "image-course-2").unwrap();
        let foreign = assets::import(&core.content, &other.id, "two.png", b"\x89PNG\r\n\x1a\n", "image-import-2").unwrap();
        assert!(capture(&core.content, &core.state_dir, &session, "user-foreign", &format!("![](asset:{})", foreign.id), &context, Some(&readable)).is_err());
    }

    #[test]
    fn rejects_more_than_four_images_and_deduplicates_note_and_question() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let mut core = engine.lock().unwrap();
        let course = library_tree::create(&mut core.content, "course", "Images", None, "", "image-limit-course").unwrap();
        let mut references = Vec::new();
        for i in 0..5 {
            let image = assets::import(&core.content, &course.id, "one.png", b"\x89PNG\r\n\x1a\n", &format!("image-limit-{i}")).unwrap();
            references.push(format!("![](asset:{})", image.id));
        }
        let note = library_tree::create(&mut core.content, "section", "Note", Some(&course.id), &references[0], "image-limit-note").unwrap();
        let (session, _) = core.start_session(&note.id, false).unwrap();
        let context = crate::context::load(&core.content, &note.id, None).unwrap();
        let readable = crate::context::course_read_manifest(&core.content, &course.id).unwrap();
        assert_eq!(capture(&core.content, &core.state_dir, &session, "limit-user", &references.join("\n"), &context, Some(&readable)).unwrap_err().code, "IMAGE_LIMIT");
        assert_eq!(capture(&core.content, &core.state_dir, &session, "dedup-user", &references[0], &context, Some(&readable)).unwrap().len(), 1);
        let mut reversed = references[..2].to_vec();
        reversed.sort_by(|a, b| b.cmp(a));
        let ordered = capture(&core.content, &core.state_dir, &session, "order-user", &reversed.join("\n"), &context, Some(&readable)).unwrap();
        assert_eq!(ordered.iter().map(|i| i["sourceId"].as_str().unwrap().to_owned()).collect::<Vec<_>>(), crate::context::image_ids(&reversed.join("\n")));
    }
}
