//! Authoritative, receipt-frozen context for explicit citations. Never a client-supplied excerpt.
use crate::citation::Locator;
use crate::{content::ContentStore, context, exercise_references as exercises, types::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitationContext {
    pub version: u8,
    pub references: Vec<Snapshot>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub source: ManifestItem,
    pub quoted_text: String,
    pub locator: Option<Locator>,
    pub supplemental_context: Vec<Supplement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exercise_state: Option<ExerciseState>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Supplement {
    pub source: ManifestItem,
    pub range: Locator,
    pub requested_range: Locator,
    pub total_chars: usize,
    pub total_bytes: usize,
    pub text: String,
    pub truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExerciseState {
    pub question_id: String,
    pub status: String,
    pub attempt_id: Option<String>,
    pub feedback_entry_id: Option<String>,
}
struct Source {
    manifest: ManifestItem,
    text: String,
}
fn shorten(part: &mut Supplement, keep: usize, tail: bool) {
    let count = part.text.chars().count();
    if keep >= count {
        return;
    }
    part.text = if tail {
        part.range.start_char = part.range.end_char - keep;
        part.text.chars().skip(count - keep).collect()
    } else {
        part.range.end_char = part.range.start_char + keep;
        part.text.chars().take(keep).collect()
    };
    part.source.included_chars = keep;
    part.truncated = true;
}
fn conflict() -> CoreError {
    CoreError::new(
        "CONTEXT_CONFLICT",
        "引用补充上下文与冻结来源不一致，请保留原问题后重新选择。",
    )
}
fn source(id: &str, path: String, version: &str, text: String) -> Source {
    Source {
        manifest: ManifestItem {
            id: id.into(),
            path,
            version: version.into(),
            included_chars: text.chars().count(),
        },
        text,
    }
}
fn exercise_source(p: exercises::ExerciseProjection) -> Source {
    source(
        &p.source_id,
        format!(
            "exercise-part/{}/{}/{}/{}",
            p.document_id, p.question_id, p.part, p.entry_id
        ),
        &p.text_hash,
        p.text,
    )
}
fn same(a: &ManifestItem, b: &ManifestItem) -> bool {
    a.id == b.id && a.path == b.path && a.version == b.version
}
fn located(text: &str, quote: &str, locator: Option<&Locator>) -> Option<Locator> {
    locator.cloned().or_else(|| {
        let mut matches = text.match_indices(quote);
        let (at, _) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        let start = text[..at].chars().count();
        Some(Locator {
            start_char: start,
            end_char: start + quote.chars().count(),
        })
    })
}
fn ordinary(line: &str) -> bool {
    let s = line.trim_start();
    !s.starts_with(['#', '>', '|', '<', '`', '~'])
        && !matches!(s.as_bytes(), [b'-' | b'*' | b'+', b' ' | b'\t', ..])
        && !s.chars().next().is_some_and(|c| c.is_ascii_digit())
}
/// Exact source offsets, blank-line paragraph boundaries, and opaque fenced blocks.
fn window(text: &str, selected: &Locator) -> Locator {
    let mut blocks: Vec<(usize, usize, bool)> = vec![];
    let mut at = 0;
    let mut start = None;
    let mut paragraph = false;
    let mut fence: Option<(char, usize)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        let end = at + line.chars().count();
        let heading = trimmed.trim_start_matches('#');
        if fence.is_none()
            && heading.len() != trimmed.len()
            && trimmed.len() - heading.len() <= 6
            && heading.starts_with(' ')
        {
            if let Some(s) = start.take() {
                blocks.push((s, at, paragraph));
            }
            blocks.push((at, end, false));
            at = end;
            continue;
        }
        if fence.is_none()
            && !trimmed.is_empty()
            && (trimmed.chars().all(|c| c == '=') || trimmed.chars().all(|c| c == '-'))
        {
            blocks.push((start.take().unwrap_or(at), end, false));
            at = end;
            continue;
        }
        // Lists and other block markers can interrupt a paragraph without a blank line.
        if fence.is_none() && paragraph && !ordinary(line) {
            if let Some(s) = start.take() {
                blocks.push((s, at, true));
            }
        }
        if start.is_none() && !trimmed.is_empty() {
            start = Some(at);
            paragraph = ordinary(line);
        }
        let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~');
        let count = marker
            .map(|c| trimmed.chars().take_while(|v| *v == c).count())
            .unwrap_or(0);
        let mut closed = false;
        if let Some((c, n)) = fence {
            if marker == Some(c) && count >= n && trimmed.chars().all(|v| v == c) {
                fence = None;
                closed = true;
            }
        } else if count >= 3 {
            if let Some(s) = start.filter(|s| *s < at) {
                blocks.push((s, at, paragraph));
                start = Some(at);
            }
            fence = Some((marker.unwrap(), count));
            paragraph = false;
        }
        if closed {
            if let Some(s) = start.take() {
                blocks.push((s, end, false));
            }
            at = end;
            continue;
        }
        if trimmed.is_empty() && fence.is_none() {
            if let Some(s) = start.take() {
                blocks.push((s, at, paragraph));
            }
        }
        at = end;
    }
    if let Some(s) = start {
        blocks.push((s, at, paragraph));
    }
    let first = blocks
        .iter()
        .position(|(a, b, _)| *a < selected.end_char && *b > selected.start_char);
    let last = blocks
        .iter()
        .rposition(|(a, b, _)| *a < selected.end_char && *b > selected.start_char);
    match (first, last) {
        (Some(mut a), Some(mut b)) => {
            if blocks[a].2 && a > 0 && blocks[a - 1].2 {
                a -= 1;
            }
            if blocks[b].2 && b + 1 < blocks.len() && blocks[b + 1].2 {
                b += 1;
            }
            Locator {
                start_char: blocks[a].0.min(selected.start_char),
                end_char: blocks[b].1.max(selected.end_char),
            }
        }
        _ => selected.clone(),
    }
}
fn add(
    snapshot: &mut Snapshot,
    source: &Source,
    wanted: Locator,
    tail: bool,
    chars: &mut usize,
    bytes: &mut usize,
) {
    if wanted.start_char == wanted.end_char {
        return;
    }
    let raw = source
        .text
        .chars()
        .skip(wanted.start_char)
        .take(wanted.end_char - wanted.start_char)
        .collect::<Vec<_>>();
    let iter: Box<dyn Iterator<Item = &char>> = if tail {
        Box::new(raw.iter().rev())
    } else {
        Box::new(raw.iter())
    };
    let mut picked = vec![];
    for c in iter {
        if *chars == 0 || *bytes < c.len_utf8() {
            break;
        }
        *chars -= 1;
        *bytes -= c.len_utf8();
        picked.push(*c);
    }
    if tail {
        picked.reverse();
    }
    let range = if tail {
        Locator {
            start_char: wanted.end_char - picked.len(),
            end_char: wanted.end_char,
        }
    } else {
        Locator {
            start_char: wanted.start_char,
            end_char: wanted.start_char + picked.len(),
        }
    };
    let mut manifest = source.manifest.clone();
    manifest.included_chars = picked.len();
    snapshot.supplemental_context.push(Supplement {
        source: manifest,
        range,
        requested_range: wanted,
        total_chars: source.text.chars().count(),
        total_bytes: source.text.len(),
        text: picked.iter().collect(),
        truncated: picked.len() != raw.len(),
    });
}
fn snapshot(
    source: Source,
    quote: &str,
    locator: Option<&Locator>,
    parts: Vec<Source>,
    state: Option<ExerciseState>,
) -> Snapshot {
    let range = located(&source.text, quote, locator);
    let mut manifest = source.manifest.clone();
    manifest.included_chars = quote.chars().count();
    let mut result = Snapshot {
        source: manifest,
        quoted_text: quote.into(),
        locator: range.clone(),
        supplemental_context: vec![],
        exercise_state: state,
    };
    let mut chars = 8000usize.saturating_sub(quote.chars().count());
    let mut bytes = 30000usize.saturating_sub(quote.len());
    let mut ordered = parts
        .into_iter()
        .filter(|p| !same(&p.manifest, &source.manifest))
        .collect::<Vec<_>>();
    ordered.sort_by_key(|p| !p.manifest.path.contains("/question/"));
    let question_count = ordered
        .iter()
        .take_while(|p| p.manifest.path.contains("/question/"))
        .count();
    for question in ordered.iter().take(question_count) {
        add(
            &mut result,
            question,
            Locator {
                start_char: 0,
                end_char: question.text.chars().count(),
            },
            false,
            &mut chars,
            &mut bytes,
        );
    }
    if let Some(range) = range {
        let surrounding = if result.exercise_state.is_some() {
            Locator {
                start_char: 0,
                end_char: source.text.chars().count(),
            }
        } else {
            window(&source.text, &range)
        };
        add(
            &mut result,
            &source,
            Locator {
                start_char: surrounding.start_char,
                end_char: range.start_char,
            },
            true,
            &mut chars,
            &mut bytes,
        );
        add(
            &mut result,
            &source,
            Locator {
                start_char: range.end_char,
                end_char: surrounding.end_char,
            },
            false,
            &mut chars,
            &mut bytes,
        );
    }
    for part in ordered.into_iter().skip(question_count) {
        let end = part.text.chars().count();
        add(
            &mut result,
            &part,
            Locator {
                start_char: 0,
                end_char: end,
            },
            false,
            &mut chars,
            &mut bytes,
        );
    }
    result
}
pub fn freeze(
    store: &mut ContentStore,
    session: &Session,
    inputs: &[context::ContextInjection],
    ai: &[context::AssistantReference],
    exercise: &[exercises::ExerciseReference],
) -> Result<CitationContext> {
    if inputs.len() + ai.len() + exercise.len() > 8 {
        return Err(CoreError::new("LIMIT_EXCEEDED", "附加引用合计最多8份。"));
    }
    crate::course_sessions::validate_injections(store, session, inputs)?;
    crate::assistant_references::validate_references(store, session, ai)?;
    let projected = exercises::validate_references(store, session, exercise)?;
    let mut refs = vec![];
    for input in inputs {
        if let Some(quote) = &input.selection {
            let doc = context::resolve_injection(store, input)?;
            crate::citation::validate_source(&doc.body, quote, input.locator.as_ref())?;
            if !session.section_id.is_empty() {
                let course = crate::library_tree::course_id(store, &session.section_id)?;
                if store.read(&course)?.metadata.kind == "course"
                    && crate::library_tree::course_id(store, &doc.id)? != course
                {
                    return Err(CoreError::new("OUT_OF_SCOPE", "引用来源不属于当前课程。"));
                }
            }
            refs.push(snapshot(
                source(&doc.id, doc.path, &doc.version, doc.body),
                quote,
                input.locator.as_ref(),
                vec![],
                None,
            ));
        }
    }
    for r in ai {
        let entry = session
            .entries
            .iter()
            .find(|e| e.id == r.entry_id)
            .ok_or_else(conflict)?;
        refs.push(snapshot(
            source(
                &r.entry_id,
                format!("ai-answer/{}/{}", session.id, r.entry_id),
                &r.text_hash,
                entry.content.clone(),
            ),
            &r.selection,
            r.locator.as_ref(),
            vec![],
            None,
        ));
    }
    for (r, p) in exercise.iter().zip(projected) {
        let (parts, state) = exercises::context_parts(store, session, r)?;
        refs.push(snapshot(
            exercise_source(p),
            &r.selection,
            r.locator.as_ref(),
            parts.into_iter().map(exercise_source).collect(),
            Some(state),
        ));
    }
    let quoted = refs
        .iter()
        .map(|r| r.quoted_text.chars().count())
        .sum::<usize>();
    if quoted > context::MAX_CONTEXT_CHARS {
        return Err(CoreError::new(
            "LIMIT_EXCEEDED",
            "精确引用超过本轮上下文预算，未截断原文。",
        ));
    }
    let mut remaining = context::MAX_CONTEXT_CHARS - quoted;
    for reference in &mut refs {
        for part in &mut reference.supplemental_context {
            let keep = part.text.chars().count().min(remaining);
            let tail = part.source.id == reference.source.id
                && reference
                    .locator
                    .as_ref()
                    .is_some_and(|r| part.range.end_char <= r.start_char);
            shorten(part, keep, tail);
            remaining -= keep;
        }
    }
    Ok(CitationContext {
        version: 1,
        references: refs,
    })
}
/// Persisted snapshots are data too: verify each frozen source without sampling newer answers.
pub fn validate(
    store: &mut ContentStore,
    session: &Session,
    frozen: &CitationContext,
    inputs: &[context::ContextInjection],
    ai: &[context::AssistantReference],
    exercise: &[exercises::ExerciseReference],
) -> Result<()> {
    let current = freeze(store, session, inputs, ai, exercise)?;
    if frozen.version != 1 || frozen.references.len() != current.references.len() {
        return Err(conflict());
    }
    for (saved, now) in frozen.references.iter().zip(current.references) {
        let origin = if saved.exercise_state.is_some() {
            let path = saved.source.path.split('/').collect::<Vec<_>>();
            let reference = exercise
                .iter()
                .find(|r| path.len() == 5 && r.document_id == path[1] && r.question_id == path[2])
                .ok_or_else(conflict)?;
            Some(crate::sessions::get(store, &reference.session_id)?)
        } else {
            None
        };
        let source_session = origin.as_ref().unwrap_or(session);
        if saved.supplemental_context.len() > context::MAX_CONTEXT_DOCUMENTS
            || saved.quoted_text.chars().count()
                + saved
                    .supplemental_context
                    .iter()
                    .map(|p| p.text.chars().count())
                    .sum::<usize>()
                > 8000
            || saved.quoted_text.len()
                + saved
                    .supplemental_context
                    .iter()
                    .map(|p| p.text.len())
                    .sum::<usize>()
                > 30000
        {
            return Err(conflict());
        }
        if !same(&saved.source, &now.source)
            || saved.source.included_chars != saved.quoted_text.chars().count()
            || saved.quoted_text != now.quoted_text
            || saved.exercise_state.is_some() != now.exercise_state.is_some()
            || serde_json::to_value(&saved.locator)? != serde_json::to_value(&now.locator)?
        {
            return Err(conflict());
        }
        let material = inputs
            .iter()
            .find(|v| v.document_id == saved.source.id)
            .map(|v| context::resolve_injection(store, v))
            .transpose()?;
        for part in &saved.supplemental_context {
            let text = if let Some(doc) = &material {
                if part.source.id != doc.id
                    || part.source.version != doc.version
                    || part.source.path != doc.path
                {
                    return Err(conflict());
                }
                doc.body.clone()
            } else if saved.source.path.starts_with("ai-answer/") {
                if !same(&saved.source, &part.source) {
                    return Err(conflict());
                }
                session
                    .entries
                    .iter()
                    .find(|e| e.id == saved.source.id)
                    .ok_or_else(conflict)?
                    .content
                    .clone()
            } else {
                let path = part.source.path.split('/').collect::<Vec<_>>();
                let original = saved.source.path.split('/').collect::<Vec<_>>();
                if path.len() != 5 || original.len() != 5 || path[..3] != original[..3] {
                    return Err(conflict());
                }
                let state = saved.exercise_state.as_ref().ok_or_else(conflict)?;
                if path[3] != "question" && state.attempt_id.is_none()
                    || path[3] == "attempt" && state.attempt_id.as_deref() != Some(path[4])
                    || path[3] == "feedback" && state.feedback_entry_id.as_deref() != Some(path[4])
                {
                    return Err(conflict());
                }
                let p = exercise_source(exercises::project(
                    store,
                    source_session,
                    path[1],
                    path[2],
                    path[3],
                    Some(path[4]),
                )?);
                if !same(&p.manifest, &part.source) {
                    return Err(conflict());
                }
                p.text
            };
            if part.range.start_char > part.range.end_char
                || part.requested_range.start_char > part.range.start_char
                || part.range.end_char > part.requested_range.end_char
                || part.requested_range.end_char > text.chars().count()
                || part.total_chars != text.chars().count()
                || part.total_bytes != text.len()
                || part.text.chars().count() != part.range.end_char - part.range.start_char
                || part.source.included_chars != part.text.chars().count()
                || part.text
                    != text
                        .chars()
                        .skip(part.range.start_char)
                        .take(part.range.end_char - part.range.start_char)
                        .collect::<String>()
                || part.truncated
                    != (part.range.start_char != part.requested_range.start_char
                        || part.range.end_char != part.requested_range.end_char)
            {
                return Err(conflict());
            }
        }
        if let Some(state) = &saved.exercise_state {
            validate_exercise_state(store, source_session, &saved.source, state)?;
        }
    }
    Ok(())
}
/// A private Focus fingerprint proves the saved bytes, but cannot grant access
/// to an auxiliary attempt/feedback that has since disappeared or been revoked.
/// Quoted references have already passed their existing authority checks.
pub(crate) fn validate_frozen_visibility(
    store: &mut ContentStore,
    frozen: &CitationContext,
    references: &[exercises::ExerciseReference],
) -> Result<()> {
    for snapshot in &frozen.references {
        let Some(state) = &snapshot.exercise_state else {
            continue;
        };
        let reference = references
            .iter()
            .find(|r| {
                snapshot.source.path
                    == format!(
                        "exercise-part/{}/{}/{}/{}",
                        r.document_id, r.question_id, r.part, r.entry_id
                    )
            })
            .ok_or_else(conflict)?;
        let origin = crate::sessions::get(store, &reference.session_id)?;
        validate_exercise_state(store, &origin, &snapshot.source, state)?;
    }
    Ok(())
}
fn validate_exercise_state(
    store: &mut ContentStore,
    session: &Session,
    source: &ManifestItem,
    state: &ExerciseState,
) -> Result<()> {
    let path = source.path.split('/').collect::<Vec<_>>();
    if path.len() != 5
        || path[2] != state.question_id
        || !matches!(state.status.as_str(), "unanswered" | "submitted" | "graded")
        || (state.status == "unanswered") != state.attempt_id.is_none()
        || (state.status == "graded") != state.feedback_entry_id.is_some()
    {
        return Err(conflict());
    }
    if let Some(id) = &state.attempt_id {
        exercises::project(store, session, path[1], path[2], "attempt", Some(id))?;
    } else if state.status != "unanswered" {
        return Err(conflict());
    }
    if let Some(id) = &state.feedback_entry_id {
        let entry = session
            .entries
            .iter()
            .find(|e| &e.id == id)
            .ok_or_else(conflict)?;
        if entry.data.as_ref().and_then(|d| d["attemptId"].as_str()) != state.attempt_id.as_deref()
        {
            return Err(conflict());
        }
        exercises::project(store, session, path[1], path[2], "feedback", Some(id))?;
    }
    Ok(())
}
pub fn saved(entry: &Entry) -> Result<Option<CitationContext>> {
    entry
        .data
        .as_ref()
        .and_then(|v| v.get("citationContext"))
        .map(|v| serde_json::from_value(v.clone()).map_err(|_| conflict()))
        .transpose()
}
/// Only already-delivered exact text ranges qualify, never a summary or a Git file's existence.
fn projection(
    ctx: &Context,
    current: &str,
    source: &ManifestItem,
    range: &Locator,
    text: &str,
) -> Vec<Value> {
    let mut covered = vec![];
    let mut candidate = |id: &str,
                         path: &str,
                         version: &str,
                         span: &Locator,
                         raw: &str,
                         provider: &str,
                         owner: &str,
                         body_start: usize| {
        let start = range.start_char.max(span.start_char);
        let end = range.end_char.min(span.end_char);
        if id == source.id
            && path == source.path
            && version == source.version
            && start < end
            && raw
                .chars()
                .skip(start - span.start_char)
                .take(end - start)
                .eq(text
                    .chars()
                    .skip(start - range.start_char)
                    .take(end - start))
        {
            covered.push((
                start,
                end,
                provider.to_string(),
                owner.to_string(),
                body_start,
            ));
        }
    };
    for m in &ctx.automatic_materials {
        let body = m.text.chars().skip(m.body_start_char).collect::<String>();
        candidate(
            &m.id,
            &source.path,
            &m.version,
            &Locator {
                start_char: 0,
                end_char: body.chars().count(),
            },
            &body,
            "material",
            &m.id,
            m.body_start_char,
        );
    }
    if let Some(c) = &ctx.citation_context {
        for prior in c.references.iter().take_while(|r| r.source.id != current) {
            if let Some(span) = &prior.locator {
                candidate(
                    &prior.source.id,
                    &prior.source.path,
                    &prior.source.version,
                    span,
                    &prior.quoted_text,
                    "citation",
                    &prior.source.id,
                    0,
                );
            }
            for part in &prior.supplemental_context {
                candidate(
                    &part.source.id,
                    &part.source.path,
                    &part.source.version,
                    &part.range,
                    &part.text,
                    "citation",
                    &prior.source.id,
                    0,
                );
            }
        }
    }
    let mut result = vec![];
    let mut cursor = range.start_char;
    while cursor < range.end_char {
        if let Some((_, end, provider, owner, body_start)) = covered
            .iter()
            .filter(|(a, b, _, _, _)| *a <= cursor && *b > cursor)
            .max_by_key(|(_, b, _, _, _)| *b)
        {
            let mut part = json!({"range":{"startChar":cursor,"endChar":end},"textFrom":{"provider":provider,"sourceId":source.id,"citationId":owner,"version":source.version}});
            if provider == "material" {
                part["textFrom"]["materialRange"] =
                    json!({"startChar":cursor+body_start,"endChar":end+body_start});
            }
            result.push(part);
            cursor = *end;
        } else {
            let end = covered
                .iter()
                .filter(|(a, _, _, _, _)| *a > cursor)
                .map(|(a, _, _, _, _)| *a)
                .min()
                .unwrap_or(range.end_char);
            result.push(json!({"range":{"startChar":cursor,"endChar":end},"text":text.chars().skip(cursor-range.start_char).take(end-cursor).collect::<String>()}));
            cursor = end;
        }
    }
    result
}
fn rendered(ctx: &Context, snapshot: &Snapshot, base: &Value, omit: bool) -> Result<String> {
    let mut value = base.clone();
    if let Some(range) = &snapshot.locator {
        let pieces = projection(
            ctx,
            &snapshot.source.id,
            &snapshot.source,
            range,
            &snapshot.quoted_text,
        );
        if pieces.iter().any(|v| v.get("textFrom").is_some()) {
            value.as_object_mut().unwrap().remove("quotedText");
            value["quotedTextProjection"] = json!(pieces);
        }
    }
    value["exerciseState"] = serde_json::to_value(&snapshot.exercise_state)?;
    let mut supplemental = vec![];
    if !omit {
        for part in &snapshot.supplemental_context {
            let mut value = serde_json::to_value(part)?;
            let pieces = projection(
                ctx,
                &snapshot.source.id,
                &part.source,
                &part.range,
                &part.text,
            );
            if pieces.iter().any(|v| v.get("textFrom").is_some()) {
                value.as_object_mut().unwrap().remove("text");
                value["projection"] = json!(pieces);
            }
            supplemental.push(value);
        }
    }
    value["supplementalContext"] = json!(supplemental);
    value["supplementalContextTruncated"] =
        json!(omit || snapshot.supplemental_context.iter().any(|p| p.truncated));
    Ok(serde_json::to_string(&value)?)
}
/// The actual bounded projection is kept separately from the saved user's original snapshot.
pub fn quote(ctx: &mut Context, id: &str, value: Value, available: usize) -> Result<String> {
    if !value.is_object() {
        return Err(CoreError::new(
            "INVALID_CONTEXT_REQUEST",
            "引用投影结构无效。",
        ));
    }
    let Some(index) = ctx
        .citation_context
        .as_ref()
        .and_then(|c| c.references.iter().position(|r| r.source.id == id))
    else {
        return Ok(serde_json::to_string(&value)?);
    };
    let mut snapshot = ctx.citation_context.as_ref().unwrap().references[index].clone();
    let mut sources = ctx
        .manifest
        .iter()
        .chain(&ctx.assistant_reference_manifest)
        .chain(&ctx.exercise_reference_manifest)
        .map(|m| m.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    sources.insert(snapshot.source.id.clone());
    for prior in ctx
        .citation_context
        .as_ref()
        .unwrap()
        .references
        .iter()
        .take(index)
    {
        for part in &prior.supplemental_context {
            if !part.text.is_empty() {
                sources.insert(part.source.id.clone());
            }
        }
    }
    for part in &mut snapshot.supplemental_context {
        if !sources.contains(&part.source.id) && sources.len() >= context::MAX_CONTEXT_DOCUMENTS {
            shorten(part, 0, false);
        } else if !part.text.is_empty() {
            sources.insert(part.source.id.clone());
        }
    }
    let mut omit = false;
    loop {
        let text = rendered(ctx, &snapshot, &value, omit)?;
        let length = text.chars().count();
        if length <= available {
            if snapshot.supplemental_context.iter().any(|p| p.truncated) || omit {
                ctx.truncated = true;
                if !ctx
                    .warnings
                    .iter()
                    .any(|w| w.starts_with("TRUNCATED_CITATION_CONTEXT"))
                {
                    ctx.warnings.push(
                        "TRUNCATED_CITATION_CONTEXT: 补充上下文已按预算缩短；精确选区未改写。"
                            .into(),
                    );
                }
            }
            ctx.citation_context.as_mut().unwrap().references[index] = snapshot;
            return Ok(text);
        }
        if let Some(part) = snapshot
            .supplemental_context
            .iter_mut()
            .rev()
            .find(|p| !p.text.is_empty())
        {
            let count = part.text.chars().count();
            let keep = count.saturating_sub(length - available);
            let tail = part.source.id == snapshot.source.id
                && snapshot
                    .locator
                    .as_ref()
                    .is_some_and(|r| part.range.end_char <= r.start_char);
            shorten(part, keep, tail);
        } else if !omit {
            omit = true;
        } else {
            return Err(CoreError::new(
                "LIMIT_EXCEEDED",
                "上下文预算不足，精确引用未截断，请减少引用。",
            ));
        }
    }
}

pub fn has_quote(ctx: &Context, id: &str) -> bool {
    ctx.citation_context
        .as_ref()
        .is_some_and(|c| c.references.iter().any(|r| r.source.id == id))
}
pub fn inventory(ctx: &Context) -> Vec<Value> {
    let mut result = vec![];
    let mut seen = std::collections::BTreeSet::new();
    if let Some(c) = &ctx.citation_context {
        for reference in &c.references {
            let mut entry = json!({"id":format!("citation:{}",reference.source.id),"kind":"reference","status":"added","sourceId":reference.source.id,"version":reference.source.version,"includedChars":reference.source.included_chars,"reason":"explicit_citation","sourceMarkers":["citation"],"coordinateSpace":"body"});
            if let Some(range) = &reference.locator {
                entry["locator"] = json!(range);
                if projection(
                    ctx,
                    &reference.source.id,
                    &reference.source,
                    range,
                    &reference.quoted_text,
                )
                .iter()
                .any(|p| p["textFrom"]["provider"] == "material")
                {
                    entry["sourceMarkers"] = json!(["citation", "material"]);
                }
            }
            result.push(entry);
            for part in &reference.supplemental_context {
                let id = format!(
                    "supplement:{}:{}:{}",
                    part.source.id, part.range.start_char, part.range.end_char
                );
                if !seen.insert(id.clone()) {
                    continue;
                }
                let material = projection(
                    ctx,
                    &reference.source.id,
                    &part.source,
                    &part.range,
                    &part.text,
                )
                .iter()
                .any(|p| p["textFrom"]["provider"] == "material");
                result.push(json!({"id":id,"kind":"reference","status":"added","sourceId":part.source.id,"version":part.source.version,"includedChars":part.source.included_chars,"range":{"startChar":part.range.start_char,"endChar":part.range.end_char,"complete":part.range.start_char==0&&part.range.end_char==part.total_chars,"totalBytes":part.total_bytes},"requestedRange":part.requested_range,"truncated":part.truncated,"sourceMarkers":if material{vec!["citation","material"]}else{vec!["citation"]},"coordinateSpace":"body","reason":"explicit_supplemental_context"}));
            }
        }
    }
    result
}

#[cfg(test)]
#[path = "citation_context_tests.rs"]
mod tests;

/// Render the existing, validated reference protocol without its storage metadata.
/// Model-facing citations are real inlined text with explicit delimiters (citation-context-v1):
/// a two-line 文件/版本 header, line-rounded context and the quoted original between
/// <引用开始>/<引用结束>. Coordinates never appear in this projection; deduplicated pieces
/// are resolved back to their real source text instead of pointing at it.
pub(crate) fn bridge_references(text: &str, ctx: &Context, focus: Option<&crate::focus::Focus>) -> String {
    /// Quoted originals are capped at 8000 Unicode characters; longer ones are
    /// cut and marked at the tail.
    const QUOTE_CHARS: usize = 8_000;
    const CONTEXT_CHARS: usize = 200;
    const QUOTE_TRUNCATION: &str = "\n（引用超长，已截断）";
    fn path(value: &str, focus: Option<&crate::focus::Focus>) -> String {
        focus.and_then(|f|value.strip_prefix(&format!("courses/{}/",f.course_id))).unwrap_or(value).to_owned()
    }
    fn part_label(part: Option<&str>) -> String {
        match part {
            Some("question") => "练习题目",
            Some("answer") => "已显示答案",
            Some("explanation") => "已显示解析",
            Some("attempt") => "已提交作答",
            Some("feedback") => "批改反馈",
            Some("reference") => "已提交参考",
            _ => "用户选取的练习分区",
        }
        .into()
    }
    fn source_label(value: &str, focus: Option<&crate::focus::Focus>) -> String {
        if value.starts_with("ai-answer/") { return "历史回答".into(); }
        if let Some(rest) = value.strip_prefix("exercise-part/") {
            return part_label(rest.split('/').nth(2));
        }
        path(value, focus)
    }
    fn chars_of(text: &str, start: usize, end: usize) -> String {
        if start >= end || end > text.chars().count() {
            return String::new();
        }
        text.chars().skip(start).take(end - start).collect()
    }
    /// A deduplicated piece must still resolve to its real text, never a placeholder.
    fn piece_text(piece: &Value, ctx: &Context) -> String {
        if let Some(raw) = piece["text"].as_str() {
            return raw.to_owned();
        }
        let from = &piece["textFrom"];
        let (Some(id), Some(version)) = (from["sourceId"].as_str(), from["version"].as_str()) else {
            return String::new();
        };
        if from["provider"] == "material" {
            // textFrom.materialRange addresses the complete material projection, body included.
            if let Some(m) = ctx
                .automatic_materials
                .iter()
                .find(|m| m.id == id && m.version == version)
            {
                let start = from["materialRange"]["startChar"].as_u64().unwrap_or_default() as usize;
                let end = from["materialRange"]["endChar"].as_u64().unwrap_or_default() as usize;
                return chars_of(&m.text, start, end);
            }
            return String::new();
        }
        let start = piece["range"]["startChar"].as_u64().unwrap_or_default() as usize;
        let end = piece["range"]["endChar"].as_u64().unwrap_or_default() as usize;
        let Some(references) = ctx.citation_context.as_ref().map(|c| c.references.as_slice()) else {
            return String::new();
        };
        for prior in references {
            if prior.source.id != id || prior.source.version != version {
                continue;
            }
            if let Some(span) = &prior.locator {
                if span.start_char <= start && end <= span.end_char {
                    return chars_of(&prior.quoted_text, start - span.start_char, end - span.start_char);
                }
            }
            for part in &prior.supplemental_context {
                if part.range.start_char <= start && end <= part.range.end_char {
                    return chars_of(&part.text, start - part.range.start_char, end - part.range.start_char);
                }
            }
        }
        String::new()
    }
    fn pieces_text(pieces: &[Value], ctx: &Context) -> String {
        pieces.iter().map(|p| piece_text(p, ctx)).collect()
    }
    fn inline_text(raw: Option<&str>, projection: Option<&Value>, ctx: &Context) -> String {
        if let Some(raw) = raw {
            return raw.to_owned();
        }
        projection
            .and_then(Value::as_array)
            .map(|pieces| pieces_text(pieces, ctx))
            .unwrap_or_default()
    }
    fn quoted_original(text: &str) -> String {
        if text.chars().count() <= QUOTE_CHARS {
            return text.to_owned();
        }
        let mut cut: String = text.chars().take(QUOTE_CHARS).collect();
        cut.push_str(QUOTE_TRUNCATION);
        cut
    }
    /// Keep whole source lines within the budget, extending outward from the quoted
    /// side; a fragment shorter than a line stays whole, and a single line above the
    /// budget is hard-cut on the side nearest the quote.
    fn line_bounded(text: &str, keep_tail: bool) -> String {
        if text.chars().count() <= CONTEXT_CHARS {
            return text.to_owned();
        }
        let lines: Vec<&str> = text.split('\n').collect();
        let candidates: Vec<&str> = if keep_tail {
            lines.iter().rev().copied().collect()
        } else {
            lines.clone()
        };
        let mut kept: Vec<&str> = Vec::new();
        for line in candidates {
            let width = line.chars().count();
            let used = kept.iter().map(|k| k.chars().count()).sum::<usize>() + kept.len();
            if used + width > CONTEXT_CHARS {
                if kept.is_empty() {
                    return if keep_tail {
                        line.chars().skip(width - CONTEXT_CHARS).collect()
                    } else {
                        line.chars().take(CONTEXT_CHARS).collect()
                    };
                }
                break;
            }
            kept.push(line);
        }
        if keep_tail {
            kept.reverse();
        }
        kept.join("\n")
    }
    fn citation_source_id(ctx: &Context, v: &Value) -> Option<String> {
        match v["type"].as_str() {
            Some("quoted_material") => v["documentId"].as_str().map(str::to_owned),
            Some("quoted_ai_answer") => v["entryId"].as_str().map(str::to_owned),
            Some("quoted_exercise_part") => {
                let path = format!(
                    "exercise-part/{}/{}/{}/{}",
                    v["documentId"].as_str().unwrap_or_default(),
                    v["questionId"].as_str().unwrap_or_default(),
                    v["part"].as_str().unwrap_or_default(),
                    v["entryId"].as_str().unwrap_or_default()
                );
                ctx.manifest
                    .iter()
                    .chain(&ctx.exercise_reference_manifest)
                    .find(|m| m.path == path)
                    .map(|m| m.id.clone())
                    .or_else(|| {
                        ctx.citation_context.as_ref().and_then(|c| {
                            c.references
                                .iter()
                                .find(|r| r.source.path == path)
                                .map(|r| r.source.id.clone())
                        })
                    })
            }
            _ => None,
        }
    }
    fn readable(v: &Value, ctx: &Context, focus: Option<&crate::focus::Focus>) -> String {
        let kind = v["type"].as_str().unwrap_or_default();
        let file = match kind {
            "quoted_material" => ctx.manifest.iter().find(|m|v["documentId"]==m.id).map(|m|path(&m.path,focus)).unwrap_or_else(||"用户材料引用".into()),
            "quoted_ai_answer" => "历史回答".into(),
            _ => part_label(v["part"].as_str()),
        };
        // Prefer the fixed Git checkpoint when one exists; otherwise the content version.
        let version = if kind == "quoted_material" {
            focus.map(|f| f.commit_hash.clone())
                .or_else(|| v["version"].as_str().map(str::to_owned))
                .or_else(|| ctx.manifest.iter().find(|m|v["documentId"]==m.id).map(|m|m.version.clone()))
        } else {
            v["textHash"].as_str().map(str::to_owned)
        }
        .unwrap_or_default();
        // Selection boundaries often swallow leading/trailing newlines; the
        // delimiters must hug the quoted text, inner line structure stays intact.
        let quote = quoted_original(inline_text(v["quotedText"].as_str(), v.get("quotedTextProjection"), ctx).trim_matches(['\n', '\r']));
        let locator = match (v["locator"]["startChar"].as_u64(), v["locator"]["endChar"].as_u64()) {
            (Some(start), Some(end)) => Some((start, end)),
            _ => None,
        };
        let citation_id = citation_source_id(ctx, v);
        let mut before = String::new();
        let mut after = String::new();
        let mut blocks = vec![];
        for supplement in v["supplementalContext"].as_array().into_iter().flatten() {
            let content = inline_text(supplement["text"].as_str(), supplement.get("projection"), ctx);
            if content.is_empty() {
                continue;
            }
            let start = supplement["range"]["startChar"].as_u64();
            let end = supplement["range"]["endChar"].as_u64();
            let same = citation_id.as_deref().is_some_and(|id| supplement["source"]["id"].as_str() == Some(id));
            // Context above/below the quote: same-source ranges around the locator.
            // Without a resolvable source id, exact adjacency to the locator decides.
            let placement = match (locator, start, end, same) {
                (Some((locator_start, _)), Some(_), Some(e), true) if e <= locator_start => 0,
                (Some((_, locator_end)), Some(s), Some(_), true) if s >= locator_end => 1,
                (Some((locator_start, _)), Some(_), Some(e), false) if citation_id.is_none() && e == locator_start => 0,
                (Some((_, locator_end)), Some(s), Some(_), false) if citation_id.is_none() && s == locator_end => 1,
                _ => 2,
            };
            if placement == 0 {
                let trimmed = content.trim_matches(['\n', '\r']);
                if !trimmed.is_empty() {
                    before = if before.is_empty() { trimmed.to_owned() } else { format!("{before}\n{trimmed}") };
                }
            } else if placement == 1 {
                let trimmed = content.trim_matches(['\n', '\r']);
                if !trimmed.is_empty() {
                    after = if after.is_empty() { trimmed.to_owned() } else { format!("{after}\n{trimmed}") };
                }
            } else {
                // Supplementary sources keep their own two-line header and delimiters.
                blocks.push((
                    source_label(supplement["source"]["path"].as_str().unwrap_or_default(), focus),
                    supplement["source"]["version"].as_str().unwrap_or_default().to_owned(),
                    content.trim_matches(['\n', '\r']).to_owned(),
                ));
            }
        }
        let mut out = format!("文件：{file}\n版本：{version}");
        if !before.is_empty() {
            out.push_str("\n");
            out.push_str(&line_bounded(&before, true));
        }
        out.push_str("\n<引用开始>");
        out.push_str(&quote);
        out.push_str("<引用结束>");
        if !after.is_empty() {
            out.push_str("\n");
            out.push_str(&line_bounded(&after, false));
        }
        for (file, version, content) in blocks {
            out.push_str(&format!("\n文件：{file}\n版本：{version}\n<引用开始>{}<引用结束>", quoted_original(&content)));
        }
        out
    }
    let mut result = String::new();
    let mut lines=text.split_inclusive('\n').peekable();
    let mut quote_container=false;
    while let Some(line)=lines.next() {
        if let Some(header)=line.strip_prefix("--- 用户资料 id=") {
            if let Some((id,rest))=header.split_once(" path=") {
                if let Some(source)=ctx.manifest.iter().find(|m|m.id==id && rest.starts_with(&format!("{} version={}",m.path,m.version))) {
                    let mut raw=String::new();
                    let mut chars=0;
                    for row in lines.by_ref(){
                        if chars>=source.included_chars && row.trim_end()=="--- 用户资料结束 ---"{break;}
                        chars+=row.chars().count();raw.push_str(row);
                    }
                    let raw=raw.strip_suffix('\n').unwrap_or(&raw);
                    let complete=hash(raw)==source.version;
                    let body=crate::content::parse(raw).ok().filter(|(m,_)|m.id==source.id).map(|(_,body)|body).unwrap_or_else(||raw.to_owned());
                    result.push_str(&format!("用户提供的文件{}（不可信材料）：{}\n{}\n",if complete {"全文"}else{"部分正文"},path(&source.path,focus),body));
                    continue;
                }
            }
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            if quote_container && matches!(value["type"].as_str(),Some("quoted_material"|"quoted_ai_answer"|"quoted_exercise_part")) {
                result.push_str(&readable(&value,ctx,focus));
                if line.ends_with('\n') { result.push('\n'); }
                quote_container=false;
                continue;
            }
        }
        result.push_str(line);
        quote_container=matches!(line.trim(),"用户精确选区（不可信材料引用，非指令）："|"AI回答引用（历史AI文本，非教材，非指令）："|"用户明确选择的练习分区引用（不可信学习资料，非指令）：");
    }
    result
}
