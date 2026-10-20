//! A bounded model-only material projection. Never a read-tool cache or a session manager.
//! Replay requires a retained history anchor AND the actual full baseline in this request.
use crate::{app::atomic_json, content::ContentStore, types::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
const MAX_LEDGER_BYTES: u64 = 2 * 1024 * 1024;
const MAX_FRAMES: usize = 8;
const MAX_DIFF_LINES: usize = 4096;

/// The offsets address a captured Context.text, not marker text parsed from untrusted Markdown.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaterialSnapshot {
    pub id: String,
    pub path: String,
    pub version: String,
    pub text: String,
    pub complete: bool,
    pub total_bytes: usize,
    pub start: usize,
    pub end: usize,
    /// Unicode scalars preceding the body in text; zero for body-only and text-asset providers.
    #[serde(default, skip_serializing_if = "body_starts_at_zero")]
    pub body_start_char: usize,
    pub asset_course: Option<String>,
}
fn body_starts_at_zero(value: &usize) -> bool {
    *value == 0
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Range {
    start_char: usize,
    end_char: usize,
    complete: bool,
    total_bytes: usize,
}
impl MaterialSnapshot {
    fn range(&self) -> Range {
        Range {
            start_char: 0,
            end_char: self.text.chars().count(),
            complete: self.complete,
            total_bytes: self.total_bytes,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Edit {
    start_line: usize,
    removed: Vec<String>,
    added: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Frame {
    Full {
        version: String,
        hash: String,
        text: String,
    },
    Diff {
        version: String,
        hash: String,
        base_hash: String,
        edits: Vec<Edit>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Chain {
    id: String,
    path: String,
    version: String,
    range: Range,
    frames: Vec<Frame>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ledger {
    schema_version: u8,
    session_id: String,
    anchor_id: String,
    prefix_hash: String,
    anchor_projection_hash: String,
    chains: Vec<Chain>,
}
/// No user history is rewritten; this record is committed only after a settled saved answer.
pub struct Prepared {
    pub text: String,
    pub stats: Value,
    chains: Vec<Chain>,
}
fn lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_owned).collect()
}
fn full(s: &MaterialSnapshot) -> Chain {
    Chain {
        id: s.id.clone(),
        path: s.path.clone(),
        version: s.version.clone(),
        range: s.range(),
        frames: vec![Frame::Full {
            version: s.version.clone(),
            hash: hash(&s.text),
            text: s.text.clone(),
        }],
    }
}
fn replay(chain: &Chain) -> Option<String> {
    if chain.frames.is_empty() || chain.frames.len() > MAX_FRAMES {
        return None;
    }
    let mut text = String::new();
    for (index, frame) in chain.frames.iter().enumerate() {
        match frame {
            Frame::Full {
                hash: h, text: t, ..
            } if index == 0 && hash(t) == *h => text = t.clone(),
            Frame::Diff {
                hash: h,
                base_hash,
                edits,
                ..
            } if index > 0 && hash(&text) == *base_hash => {
                let mut old = lines(&text);
                let mut previous_end = 0;
                for e in edits {
                    let end = e.start_line.checked_add(e.removed.len())?;
                    if e.start_line < previous_end
                        || end > old.len()
                        || old.get(e.start_line..end)? != e.removed.as_slice()
                    {
                        return None;
                    }
                    previous_end = end;
                }
                for e in edits.iter().rev() {
                    old.splice(
                        e.start_line..e.start_line + e.removed.len(),
                        e.added.clone(),
                    );
                }
                text = old.concat();
                if hash(&text) != *h {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(text)
}
fn prefix_hash(session: &Session, anchor: &str) -> Option<String> {
    let index = session.entries.iter().position(|e| e.id == anchor)?;
    Some(hash(
        &serde_json::to_string(&session.entries[..=index]).ok()?,
    ))
}
fn ledger_path(state: &Path, session: &str, create: bool) -> Option<PathBuf> {
    if !valid_id(session) {
        return None;
    }
    let dir = state.join("automatic-context");
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            fs::create_dir(&dir).ok()?;
        }
        _ => return None,
    }
    let path = dir.join(format!("{session}.json"));
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => None,
        _ => Some(path),
    }
}
fn load(state: &Path, session: &Session, ctx: &Context, native: bool) -> Option<Ledger> {
    // A clipped/compacted history must not make a local-only baseline appear retained.
    if ctx.excluded_history_source_ids.len() > 0
        || (!native
            && ctx
                .warnings
                .iter()
                .any(|w| w.starts_with("TRUNCATED_HISTORY")))
    {
        return None;
    }
    let path = ledger_path(state, &session.id, false)?;
    if fs::metadata(&path).ok()?.len() > MAX_LEDGER_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_LEDGER_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_LEDGER_BYTES {
        return None;
    }
    let saved: Ledger = serde_json::from_slice(&bytes).ok()?;
    let anchor = ctx
        .history_manifest
        .iter()
        .find(|m| m.id == saved.anchor_id);
    if saved.schema_version != 1
        || saved.session_id != session.id
        || saved.chains.len() > 32
        || prefix_hash(session, &saved.anchor_id)? != saved.prefix_hash
        || (!native
            && anchor.map(|a| a.version.as_str()) != Some(saved.anchor_projection_hash.as_str()))
    {
        return None;
    }
    // Explicit future compaction/boundary entries also invalidate, even if an old anchor remains.
    let index = session
        .entries
        .iter()
        .position(|e| e.id == saved.anchor_id)?;
    if session.entries[index + 1..].iter().any(|e| {
        e.kind == "compaction"
            || e.data
                .as_ref()
                .and_then(|d| d.get("contextBoundary"))
                .and_then(Value::as_bool)
                == Some(true)
    }) {
        return None;
    }
    Some(saved)
}
fn patch(old: &str, new: &str, version: &str) -> Option<Frame> {
    let a = lines(old);
    let b = lines(new);
    let width = b.len() + 1;
    // Bounded line LCS keeps disjoint metadata/body changes separate without a new dependency.
    if a.len().max(b.len()) > MAX_DIFF_LINES || (a.len() + 1).checked_mul(width)? > 1_000_000 {
        return None;
    }
    let mut table = vec![0u16; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i * width + j] = if a[i] == b[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut edits = Vec::new();
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            i += 1;
            j += 1;
            continue;
        }
        let start = i;
        let mut removed = Vec::new();
        let mut added = Vec::new();
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && a[i] == b[j] {
                break;
            }
            if j < b.len()
                && (i == a.len() || table[i * width + j + 1] > table[(i + 1) * width + j])
            {
                added.push(b[j].clone());
                j += 1;
            } else {
                removed.push(a[i].clone());
                i += 1;
            }
        }
        edits.push(Edit {
            start_line: start,
            removed,
            added,
        });
    }
    if edits.is_empty() {
        return None;
    }
    let frame = Frame::Diff {
        version: version.into(),
        hash: hash(new),
        base_hash: hash(old),
        edits,
    };
    if serde_json::to_vec(&frame).ok()?.len() * 2 >= old.len().max(new.len()) {
        return None;
    }
    Some(frame)
}
fn unified(id: &str, edits: &[Edit]) -> String {
    let mut result = format!("--- a/{id}\n+++ b/{id}\n");
    let mut offset = 0isize;
    for e in edits {
        let new_start = (e.start_line as isize + offset) as usize;
        result.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            e.start_line + usize::from(!e.removed.is_empty()),
            e.removed.len(),
            new_start + usize::from(!e.added.is_empty()),
            e.added.len()
        ));
        for (marker, ls) in [('-', &e.removed), ('+', &e.added)] {
            for line in ls {
                result.push(marker);
                result.push_str(line);
                if !line.ends_with('\n') {
                    result.push_str("\n\\ No newline at end of file\n");
                }
            }
        }
        offset += e.added.len() as isize - e.removed.len() as isize;
    }
    result
}
fn rendered(chain: &Chain) -> String {
    let frames=chain.frames.iter().map(|f|match f {
        Frame::Full{version,hash,text}=>json!({"kind":"full","version":version,"textHash":hash,"text":text}),
        Frame::Diff{version,hash,base_hash,edits}=>json!({"kind":"diff","version":version,"baseHash":base_hash,"textHash":hash,"unifiedDiff":unified(&chain.id,edits)}),
    }).collect::<Vec<_>>();
    format!(
        "\n{}\n",
        json!({"type":"automatic_material_projection","id":chain.id,"path":chain.path,"version":chain.version,"range":chain.range,"frames":frames})
    )
}
fn replace(ctx: &Context, chains: &[Chain]) -> Option<String> {
    let mut result = String::new();
    let mut cursor = 0;
    for (source, chain) in ctx.automatic_materials.iter().zip(chains) {
        if source.start < cursor
            || source.end > ctx.text.len()
            || !ctx.text.is_char_boundary(source.start)
            || !ctx.text.is_char_boundary(source.end)
        {
            return None;
        }
        result.push_str(&ctx.text[cursor..source.start]);
        result.push_str(&rendered(chain));
        cursor = source.end;
    }
    result.push_str(&ctx.text[cursor..]);
    Some(result)
}
/// Verify captured versions while the Core lock still excludes app writes. External saves during
/// capture fail closed; edits after this check belong to the next request, never a mixed diff.
pub fn verify_snapshot(store: &ContentStore, ctx: &Context) -> Result<()> {
    for source in &ctx.automatic_materials {
        let (version, text) = if let Some(course) = &source.asset_course {
            let (asset, text) = crate::assets::referenced_text(store, course, &source.id)?;
            (asset.sha256, text.unwrap_or_default())
        } else {
            let doc = store.read(&source.id)?;
            (doc.version, doc.content)
        };
        if version != source.version || !text.starts_with(&source.text) {
            return Err(CoreError::new(
                "CONTEXT_CONFLICT",
                "资料在上下文快照期间已变化，请重试；未发送混合版本或错误 diff。",
            ));
        }
    }
    Ok(())
}
pub fn prepare(state: &Path, session: &Session, ctx: &Context) -> Prepared {
    prepare_mode(state, session, ctx, false)
}
pub fn prepare_native(state: &Path, session: &Session, ctx: &Context) -> Prepared {
    prepare_mode(state, session, ctx, true)
}
fn prepare_mode(state: &Path, session: &Session, ctx: &Context, native: bool) -> Prepared {
    let previous = load(state, session, ctx, native);
    let mut bases: BTreeMap<String, Chain> = previous
        .into_iter()
        .flat_map(|l| l.chains)
        .map(|c| (c.id.clone(), c))
        .collect();
    let mut chains = Vec::new();
    let mut updates = Vec::new();
    let mut appended = 0;
    for source in &ctx.automatic_materials {
        let mut chain = full(source);
        let mut mode = "full";
        let mut reason = "baseline_unavailable";
        if source.complete {
            if let Some(base) = bases
                .remove(&source.id)
                .filter(|b| b.range.complete && b.path == source.path)
            {
                if let Some(old) = replay(&base) {
                    if old == source.text
                        && base.range == source.range()
                        && base.version == source.version
                    {
                        chain = base;
                        mode = "unchanged";
                        reason = "retained_full_and_chain";
                    } else if base.frames.len() < MAX_FRAMES {
                        if let Some(delta) = patch(&old, &source.text, &source.version) {
                            chain = base;
                            chain.version = source.version.clone();
                            chain.range = source.range();
                            chain.frames.push(delta);
                            if rendered(&chain).len() <= rendered(&full(source)).len() * 3 / 2 {
                                mode = "diff";
                                reason = "small_change";
                            } else {
                                chain = full(source);
                                reason = "chain_budget";
                            }
                        } else {
                            reason = "large_change";
                        }
                    } else {
                        reason = "chain_limit";
                    }
                } else {
                    reason = "invalid_baseline";
                }
            }
        } else {
            reason = "partial_range_reload";
        }
        let bytes = if mode == "unchanged" {
            0
        } else {
            let mut last = chain.clone();
            last.frames = vec![chain.frames.last().unwrap().clone()];
            rendered(&last).len()
        };
        appended += bytes;
        updates.push(json!({"id":source.id,"mode":mode,"reason":reason,"range":source.range(),"appendedBytes":bytes}));
        chains.push(chain);
    }
    let projected = replace(ctx, &chains).unwrap_or_else(|| ctx.text.clone());
    // Do not grow the existing context-character budget to retain a diff chain.
    if projected.chars().count() > crate::context::MAX_CONTEXT_CHARS
        || projected.len() > MAX_LEDGER_BYTES as usize / 2
    {
        return Prepared {
            text: ctx.text.clone(),
            stats: json!({"version":1,"mode":"full","reason":"projection_budget","appendedBytes":ctx.text.len(),"wireBytes":ctx.text.len()}),
            chains: vec![],
        };
    }
    Prepared {
        stats: json!({"version":1,"updates":updates,"appendedBytes":appended,"wireBytes":projected.len(),"previousFullBytes":ctx.text.len()}),
        text: projected,
        chains,
    }
}
impl Prepared {
    /// Separate versioned automatic documents from the existing one-shot material text.
    pub fn native_materials(&self, ctx: &Context) -> Vec<Value> {
        ctx.automatic_materials
            .iter()
            .enumerate()
            .map(|(index, s)| {
                let mut value: Value = self
                    .chains
                    .get(index)
                    .and_then(|c| serde_json::from_str(&rendered(c)).ok())
                    .unwrap_or(json!({"frames":[]}));
                value["id"] = json!(s.id);
                value["version"] = json!(s.version);
                value["range"] = json!(s.range());
                value["text"] = json!(s.text);
                value["textHash"] = json!(hash(&s.text));
                value["bodyStartChar"] = json!(s.body_start_char);
                value
            })
            .collect()
    }
    pub fn commit_native(&self, state: &Path, session: &Session, anchor: &str) -> Result<()> {
        let path = ledger_path(state, &session.id, true)
            .ok_or_else(|| CoreError::new("IO", "资料投影基准不可保存。"))?;
        let saved = Ledger {
            schema_version: 1,
            session_id: session.id.clone(),
            anchor_id: anchor.into(),
            prefix_hash: prefix_hash(session, anchor)
                .ok_or_else(|| CoreError::new("INVALID_STATE", "资料锚点缺失。"))?,
            anchor_projection_hash: String::new(),
            chains: self.chains.clone(),
        };
        if serde_json::to_vec(&saved)?.len() as u64 <= MAX_LEDGER_BYTES {
            atomic_json(&path, &saved)?;
        }
        Ok(())
    }
    pub fn provider_texts(&self, ctx: &Context) -> Result<(String, String)> {
        let mut materials = String::new();
        let mut references = String::new();
        let mut cursor = 0;
        for (i, s) in ctx.automatic_materials.iter().enumerate() {
            if s.start < cursor
                || s.end > ctx.text.len()
                || !ctx.text.is_char_boundary(s.start)
                || !ctx.text.is_char_boundary(s.end)
            {
                return Err(CoreError::new(
                    "INVALID_STATE",
                    "上下文分区快照无效，未发送请求。",
                ));
            }
            references.push_str(&ctx.text[cursor..s.start]);
            if self.chains.len() == ctx.automatic_materials.len() {
                materials.push_str(&rendered(&self.chains[i]));
            } else {
                materials.push_str(&ctx.text[s.start..s.end]);
            }
            cursor = s.end;
        }
        references.push_str(&ctx.text[cursor..]);
        Ok((materials, references))
    }
    pub fn manifest_entries(&self, ctx: &Context) -> Vec<Value> {
        ctx.automatic_materials.iter().enumerate().map(|(i,s)| {
            let update=&self.stats["updates"][i];
            let mode=update["mode"].as_str().unwrap_or("full");
            let status=match mode {"unchanged"=>"retained","diff"=>"updated",
                _ if update["reason"].as_str().is_some_and(|r|r!="baseline_unavailable") || self.stats["reason"]=="projection_budget"=>"reloaded",_=>"added"};
            json!({"id":s.id,"kind":"material","status":status,"sourceId":s.id,"version":s.version,
                "textHash":hash(&s.text),"includedChars":s.text.chars().count(),"range":s.range(),"bodyStartChar":s.body_start_char,"coordinateSpace":if s.body_start_char==0{"body"}else{"serialized-document"},
                "reason":update["reason"].as_str().or(self.stats["reason"].as_str()).unwrap_or("full_baseline")})
        }).collect()
    }
    pub fn commit(&self, state: &Path, session: &Session, anchor: &str) -> Result<()> {
        let path = ledger_path(state, &session.id, true)
            .ok_or_else(|| CoreError::new("IO", "资料投影基准不可保存，下次会重新载入正文。"))?;
        let (history, truncated) = crate::generation::quoted_history(session, None);
        if truncated {
            return Ok(());
        }
        let item = history
            .iter()
            .find(|v| v["id"].as_str() == Some(anchor))
            .ok_or_else(|| {
                CoreError::new("INVALID_STATE", "本次投影锚点未保留，下次重新载入正文。")
            })?;
        let saved = Ledger {
            schema_version: 1,
            session_id: session.id.clone(),
            anchor_id: anchor.into(),
            prefix_hash: prefix_hash(session, anchor)
                .ok_or_else(|| CoreError::new("INVALID_STATE", "投影锚点缺失。"))?,
            anchor_projection_hash: hash(&serde_json::to_string(item)?),
            chains: self.chains.clone(),
        };
        if serde_json::to_vec(&saved)?.len() as u64 > MAX_LEDGER_BYTES {
            return Ok(());
        }
        atomic_json(&path, &saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::Engine, content, context, sessions};
    use std::sync::Arc;
    fn body() -> String {
        (0..180)
            .map(|n| {
                format!("Line {n:03}: Original public course statement with a stable example.\n")
            })
            .collect()
    }
    fn setup() -> (tempfile::TempDir, Arc<Engine>, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        let (doc, session) = {
            let mut c = e.lock().unwrap();
            let d = c.create_section("Course", &body(), vec![], vec![]).unwrap();
            let (s, _) = c.start_session(&d.id, false).unwrap();
            (d.id, s.id)
        };
        (dir, e, doc, session)
    }
    fn capture(e: &Engine, sid: &str) -> (Session, Context, Prepared) {
        let mut c = e.lock().unwrap();
        let (s, mut ctx) = c.session_context(sid, None).unwrap();
        context::attach_history(&mut ctx, &s, None);
        verify_snapshot(&c.content, &ctx).unwrap();
        let p = prepare(&c.state_dir, &s, &ctx);
        (s, ctx, p)
    }
    fn answer(e: &Engine, sid: &str, p: &Prepared, key: &str) -> Session {
        let mut c = e.lock().unwrap();
        let saved = sessions::append(
            &mut c.content,
            sid,
            Entry {
                id: String::new(),
                kind: "assistant".into(),
                content: "A saved answer.".into(),
                created_at: String::new(),
                selection: None,
                related_ids: vec![],
                data: None,
            },
            None,
            key,
        )
        .unwrap();
        p.commit(&c.state_dir, &saved, &saved.entries.last().unwrap().id)
            .unwrap();
        saved
    }
    fn edit(e: &Engine, id: &str, body: &str) {
        let mut c = e.lock().unwrap();
        let d = c.content.read(id).unwrap();
        let encoded = content::serialize(&d.metadata, body).unwrap();
        c.content.save(id, &encoded, &d.version).unwrap();
    }
    fn mode(p: &Prepared) -> &str {
        p.stats["updates"][0]["mode"].as_str().unwrap_or("full")
    }
    #[test]
    fn first_unchanged_and_small_edits_replay_the_actual_full_base_without_appending_unchanged() {
        let (_d, e, id, sid) = setup();
        let (_, _, first) = capture(&e, &sid);
        assert_eq!(mode(&first), "full");
        assert!(first.text.contains("Original public course"));
        answer(&e, &sid, &first, "answer_1");
        let (_, _, same) = capture(&e, &sid);
        assert_eq!(mode(&same), "unchanged");
        assert_eq!(same.stats["appendedBytes"], 0);
        assert_eq!(same.chains[0].frames.len(), 1);
        edit(
            &e,
            &id,
            &body().replace("Line 080: Original", "Line 080: Updated"),
        );
        // Multiple personal edits before sending coalesce into this one final snapshot.
        edit(
            &e,
            &id,
            &body().replace("Line 080: Original", "Line 080: Final revised"),
        );
        let (_, ctx, changed) = capture(&e, &sid);
        assert_eq!(mode(&changed), "diff");
        assert_eq!(changed.chains[0].frames.len(), 2);
        assert_eq!(
            replay(&changed.chains[0]).unwrap(),
            ctx.automatic_materials[0].text
        );
        assert!(changed.text.contains("Final revised"));
        assert!(!changed.text.contains("Line 080: Updated"));
        assert!(changed.text.contains("\"kind\":\"full\""));
        assert!(changed.text.contains("unifiedDiff"));
    }
    #[test]
    fn broad_change_missing_corrupt_base_and_missing_effective_anchor_fall_back_to_full() {
        let (dir, e, id, sid) = setup();
        let (_, _, first) = capture(&e, &sid);
        answer(&e, &sid, &first, "answer_1");
        edit(
            &e,
            &id,
            &body().replace(
                "Original public course statement",
                "Entirely replaced unrelated paragraph",
            ),
        );
        let (s, ctx, big) = capture(&e, &sid);
        assert_eq!(mode(&big), "full");
        assert_eq!(big.stats["updates"][0]["reason"], "large_change");
        let mut clipped = ctx.clone();
        clipped.history_manifest.clear();
        assert_eq!(
            mode(&prepare(&dir.path().join("state"), &s, &clipped)),
            "full"
        );
        clipped = ctx.clone();
        clipped
            .warnings
            .push("TRUNCATED_HISTORY: effective history compacted".into());
        assert_eq!(
            mode(&prepare(&dir.path().join("state"), &s, &clipped)),
            "full"
        );
        let path = ledger_path(&dir.path().join("state"), &sid, false).unwrap();
        fs::write(&path, "broken JSON").unwrap();
        assert_eq!(mode(&capture(&e, &sid).2), "full");
        fs::remove_file(path).unwrap();
        assert_eq!(mode(&capture(&e, &sid).2), "full");
    }
    #[test]
    fn new_session_branch_boundary_and_history_crop_never_inherit_local_baselines() {
        let (dir, e, id, sid) = setup();
        let (_, _, first) = capture(&e, &sid);
        let saved = answer(&e, &sid, &first, "answer_1");
        let (_, ctx, _) = capture(&e, &sid);
        let mut branch = saved.clone();
        branch.entries[0].content = "Changed branch".into();
        assert_eq!(
            mode(&prepare(&dir.path().join("state"), &branch, &ctx)),
            "full"
        );
        let mut boundary = saved.clone();
        let mut note = boundary.entries[0].clone();
        note.id = "boundary".into();
        note.kind = "compaction".into();
        boundary.entries.push(note);
        assert_eq!(
            mode(&prepare(&dir.path().join("state"), &boundary, &ctx)),
            "full"
        );
        let other = {
            let mut c = e.lock().unwrap();
            c.start_session(&id, true).unwrap().0.id
        };
        assert_eq!(mode(&capture(&e, &other).2), "full");
        for n in 0..20 {
            answer(&e, &sid, &first, &format!("history_{n}"));
        }
        let (_, cropped, p) = capture(&e, &sid);
        assert!(cropped
            .warnings
            .iter()
            .any(|w| w.starts_with("TRUNCATED_HISTORY")));
        assert_eq!(mode(&p), "full");
    }
    #[test]
    fn truncated_range_does_not_claim_full_or_use_a_diff_for_unseen_content() {
        let (_dir, e, id, sid) = setup();
        edit(&e, &id, &format!("{}UNSEEN_SUFFIX", "x".repeat(90_000)));
        let (_, ctx, p) = capture(&e, &sid);
        assert!(!ctx.automatic_materials[0].complete);
        assert!(ctx.truncated);
        assert!(!p.text.contains("UNSEEN_SUFFIX"));
        assert!(!p.text.contains("unifiedDiff"));
        answer(&e, &sid, &p, "partial_1");
        let (_, _, next) = capture(&e, &sid);
        assert_eq!(mode(&next), "full");
        assert!(!next.text.contains("UNSEEN_SUFFIX"));
        // Explicit partial scope also stays partial if it fits a smaller synthetic capture.
        let mut small = ctx;
        small.text = "prefix".into();
        let s = &mut small.automatic_materials[0];
        s.text = "prefix".into();
        s.start = 0;
        s.end = 6;
        let s = e.lock().unwrap().session_context(&sid, None).unwrap().0;
        let result = prepare(&e.lock().unwrap().state_dir, &s, &small);
        assert!(result.text.contains("\"complete\":false"));
        assert_eq!(result.stats["updates"][0]["reason"], "partial_range_reload");
    }
    #[test]
    fn snapshot_versions_remain_frozen_and_a_change_during_capture_is_rejected() {
        let (_dir, e, id, sid) = setup();
        let (_, ctx, frozen) = capture(&e, &sid);
        let old = ctx.automatic_materials[0].version.clone();
        edit(
            &e,
            &id,
            &body().replace("Line 080: Original", "Line 080: CONCURRENT"),
        );
        let c = e.lock().unwrap();
        assert_eq!(
            verify_snapshot(&c.content, &ctx).unwrap_err().code,
            "CONTEXT_CONFLICT"
        );
        assert_eq!(ctx.automatic_materials[0].version, old);
        assert!(!frozen.text.contains("CONCURRENT"));
        drop(c);
        let (_, next, _) = capture(&e, &sid);
        assert!(next.automatic_materials[0].text.contains("CONCURRENT"));
    }
    #[test]
    fn link_aliases_deduplicate_and_manual_one_shot_sources_never_enter_replay() {
        let (_dir, e, id, sid) = setup();
        let mut c = e.lock().unwrap();
        let other = c
            .create_section("Manual", "MANUAL_ONCE_ONLY", vec![], vec![])
            .unwrap();
        let d = c.content.read(&id).unwrap();
        let mut meta = d.metadata;
        meta.links = vec![id.clone(), id.clone()];
        c.content
            .save(
                &id,
                &content::serialize(&meta, &d.body).unwrap(),
                &d.version,
            )
            .unwrap();
        let (s, mut ctx) = c.session_context(&sid, None).unwrap();
        assert_eq!(ctx.automatic_materials.len(), 1);
        context::inject(
            &c.content,
            &mut ctx,
            &[context::ContextInjection {
                locator: None,
                document_id: other.id,
                expected_version: other.version,
                selection: None,
            }],
        )
        .unwrap();
        assert_eq!(ctx.automatic_materials.len(), 1);
        let p = prepare(&c.state_dir, &s, &ctx);
        assert!(p.text.contains("MANUAL_ONCE_ONLY"));
        drop(c);
        answer(&e, &sid, &p, "manual_answer");
        let (_, _, next) = capture(&e, &sid);
        assert!(!next.text.contains("MANUAL_ONCE_ONLY"));
    }
    #[test]
    fn preparing_or_failing_a_send_does_not_advance_persisted_state_and_retry_is_deterministic() {
        let (dir, e, id, sid) = setup();
        let (_, _, first) = capture(&e, &sid);
        assert!(!dir.path().join("state/automatic-context").exists());
        answer(&e, &sid, &first, "answer_1");
        let path = ledger_path(&dir.path().join("state"), &sid, false).unwrap();
        let old = fs::read(&path).unwrap();
        edit(
            &e,
            &id,
            &body().replace("Line 080: Original", "Line 080: After failure"),
        );
        let (s, ctx, failed) = capture(&e, &sid);
        // No successful saved answer -> no commit. A new authorized attempt reuses only the old base.
        let retry = prepare(&dir.path().join("state"), &s, &ctx);
        assert_eq!(failed.text, retry.text);
        assert_eq!(fs::read(path).unwrap(), old);
        assert_eq!(failed.chains[0].frames.len(), 2);
        assert_eq!(retry.chains[0].frames.len(), 2);
    }
    #[test]
    fn active_query_tool_always_returns_actual_plaintext_again_even_after_projection_was_committed()
    {
        let (_dir, e, id, sid) = setup();
        let (_, _, first) = capture(&e, &sid);
        answer(&e, &sid, &first, "answer_1");
        let scope = crate::learning_tools::ToolScope {
            session_id: sid,
            section_id: id.clone(),
            run_id: "run_read_fixture".into(),
            allowed_ids: vec![id.clone()],
        };
        for call in ["read_one", "read_two"] {
            let result = crate::learning_tools::execute(
                &e,
                &scope,
                call,
                "query_related_knowledge",
                json!({"knowledgeId":id}),
            )
            .unwrap();
            let text = result["results"][0]["excerpt"].as_str().unwrap();
            assert!(text.contains("Original public course"));
            assert!(!text.contains("unifiedDiff"));
            assert!(!text.contains("unchanged"));
        }
    }
    #[test]
    fn diff_roundtrips_unicode_blank_lines_newline_and_insertion_removal() {
        for (old, new) in [
            ("原文\n\n尾行", "原文\n改动\n尾行\n"),
            ("a\nb\nc\n", "a\nc\n"),
            ("a\n", "a\nb\n"),
        ] {
            // Pad unchanged context so the useful-diff threshold is met.
            let old = format!("{}{old}", "stable padding line\n".repeat(100));
            let new = format!("{}{new}", "stable padding line\n".repeat(100));
            let d = patch(&old, &new, "v2").unwrap();
            let c = Chain {
                id: "doc".into(),
                path: "doc.md".into(),
                version: "v2".into(),
                range: Range {
                    start_char: 0,
                    end_char: new.chars().count(),
                    complete: true,
                    total_bytes: new.len(),
                },
                frames: vec![
                    Frame::Full {
                        version: "v1".into(),
                        hash: hash(&old),
                        text: old,
                    },
                    d,
                ],
            };
            assert_eq!(replay(&c).unwrap(), new);
        }
    }
}
