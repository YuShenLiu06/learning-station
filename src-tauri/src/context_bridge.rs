//! Project providers validate authorization and freeze content; this store only orders, deduplicates
//! and bounds their model projection. It does not read files, manage sessions, or execute tools.
use crate::{
    automatic_context::{self, Prepared},
    content::ContentStore,
    generation::GenerationRequest,
    time_context::TimeContext,
    types::*,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifetime {
    Stable,
    Turn,
    Versioned,
    OneShot,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Position {
    Stable,
    History,
    Materials,
    References,
    State,
    Question,
}
#[derive(Clone)]
pub struct PromptPart {
    pub key: &'static str,
    pub lifetime: Lifetime,
    pub position: Position,
    pub value: Value,
}
/// Providers supply already-authorized, already-frozen values and declare their lifetime/position.
pub trait ContextProvider {
    fn provide(&self) -> Result<PromptPart>;
}
impl ContextProvider for PromptPart {
    fn provide(&self) -> Result<PromptPart> {
        Ok(self.clone())
    }
}
#[derive(Default)]
pub struct ContextBridge {
    parts: BTreeMap<&'static str, PromptPart>,
}
pub struct FrozenPrompt {
    pub messages: Vec<Value>,
    pub time: TimeContext,
    pub manifest: Value,
}
impl ContextBridge {
    pub fn add(&mut self, provider: &dyn ContextProvider) -> Result<()> {
        let part = provider.provide()?;
        let valid = match part.position {
            Position::Stable | Position::State => part.value.is_object(),
            Position::History => part.value.is_array(),
            Position::Materials | Position::References | Position::Question => {
                part.value.is_string()
            }
        };
        if !valid {
            return Err(CoreError::new(
                "INVALID_CONTEXT_REQUEST",
                "上下文提供器的结构无效。",
            ));
        }

        if let Some(old) = self.parts.get(part.key) {
            if old.position != part.position
                || old.lifetime != part.lifetime
                || old.value != part.value
            {
                return Err(CoreError::new(
                    "CONTEXT_CONFLICT",
                    "同一上下文来源有冲突，未发送模型请求。",
                ));
            }
            return Ok(());
        }
        if self.parts.len() >= 16 {
            return Err(CoreError::new("LIMIT_EXCEEDED", "上下文片段超过预算。"));
        }
        self.parts.insert(part.key, part);
        Ok(())
    }
    pub fn snapshot(
        state: &Path,
        store: &ContentStore,
        session: &Session,
        ctx: &Context,
    ) -> Result<Prepared> {
        automatic_context::verify_snapshot(store, ctx)?;
        Ok(automatic_context::prepare_native(state, session, ctx))
    }
    pub fn freeze(
        request: &GenerationRequest,
        ctx: &Context,
        session: &Session,
        intent: Option<&crate::exploration::ExplorationContext>,
        projection: &Prepared,
        time: TimeContext,
    ) -> Result<FrozenPrompt> {
        time.validate()?;
        let legacy = crate::generation::messages_current(request, ctx, session, intent);
        let raw = legacy
            .last()
            .and_then(|m| m["content"].as_str())
            .unwrap_or_default();
        let mut metadata: serde_json::Map<String, Value> = serde_json::from_str(raw)?;
        metadata.remove("learningHistory");
        metadata.remove("historyManifest");
        metadata.remove("context");
        metadata.remove("userGoal");
        if let Some(document) = &request.document_id { metadata.insert("activeDocumentId".into(), json!(document)); }
        if let Some(focus) = &request.focus { metadata.insert("focus".into(), json!(focus)); }
        let metadata_only_ids: Vec<_> = ctx.manifest.iter().filter(|m| m.included_chars == 0 && !ctx.automatic_materials.iter().any(|a|a.id==m.id)).map(|m|m.id.clone()).collect();
        if !metadata_only_ids.is_empty() { metadata.insert("metadataOnlySourceIds".into(), json!(metadata_only_ids)); }
        let question = metadata.remove("question").unwrap_or(json!(""));
        let (materials, references) = projection.provider_texts(ctx)?;
        let mut store = Self::default();
        for part in [
            PromptPart {
                key: "stable",
                lifetime: Lifetime::Stable,
                position: Position::Stable,
                value: json!({"type":"learning_request","materialRole":"user-reference","projectionVersion":1}),
            },
            PromptPart {
                key: "materials",
                lifetime: Lifetime::Versioned,
                position: Position::Materials,
                value: json!(materials),
            },
            PromptPart {
                key: "references",
                lifetime: Lifetime::OneShot,
                position: Position::References,
                value: json!(references.clone()),
            },
            PromptPart {
                key: "turn",
                lifetime: Lifetime::Turn,
                position: Position::State,
                value: json!({"referenceTime":time.reference_time,"timeZone":time.time_zone,"metadata":metadata,"automaticProjection":projection.stats}),
            },
            PromptPart {
                key: "question",
                lifetime: Lifetime::Turn,
                position: Position::Question,
                value: question,
            },
        ] {
            store.add(&part)?;
        }
        let mut entries = projection.manifest_entries(ctx);
        let auto_ids = ctx
            .automatic_materials
            .iter()
            .map(|m| m.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for m in ctx
            .manifest
            .iter()
            .filter(|m| !auto_ids.contains(m.id.as_str()))
            .chain(&ctx.assistant_reference_manifest)
            .chain(&ctx.exercise_reference_manifest)
        {
            let metadata_only = metadata_only_ids.contains(&m.id);
            entries.push(json!({"id":format!("reference:{}",m.id),"kind":if metadata_only { "material" } else { "reference" },"status":"added","sourceId":m.id,"version":m.version,"includedChars":m.included_chars,"reason":if metadata_only { "material_metadata_only" } else { "explicit_one_shot" }}));
        }
        if !request.selection.is_empty() {
            entries.push(json!({"id":"selection","kind":"reference","status":"added","sourceId":request.document_id.as_deref().unwrap_or(&session.section_id),"version":hash(&request.selection),"includedChars":request.selection.chars().count(),"reason":"explicit_one_shot"}));
        }
        let mut focus_text = String::new();
        if let Some(focus) = &request.focus {
            let source = focus.asset.as_ref().and_then(|a|a["id"].as_str()).unwrap_or(&focus.document_id);
            if focus.paragraphs.is_empty() { entries.push(json!({"id":"focus","kind":"turn","status":"added","sourceId":source,"version":focus.document_version,"includedChars":0,"reason":"focus_metadata_only"})); }
            for (index, part) in focus.paragraphs.iter().enumerate() {
                let text = part["text"].as_str().unwrap_or_default(); focus_text.push_str(text);
                let mut entry = json!({"id":format!("focus:{index}"),"kind":"turn","status":"added","sourceId":source,"version":focus.document_version,"includedChars":text.chars().count(),"reason":"focus_visible_body"});
                if part["startChar"].is_u64() && part["endChar"].is_u64() && part["totalBytes"].is_u64() {
                    entry["range"] = json!({"startChar":part["startChar"],"endChar":part["endChar"],"totalBytes":part["totalBytes"],"complete":part["startChar"]==0 && part["endChar"]==part["totalChars"]});
                }
                entries.push(entry);
            }
        }
        entries.push(json!({"id":"request-time","kind":"turn","status":"added","version":hash(&serde_json::to_string(&time)?),"includedChars":serde_json::to_string(&time)?.chars().count(),"reason":"frozen_current_turn"}));
        entries.push(json!({"id":"context-protocol","kind":"stable","status":"retained","version":"1","includedChars":serde_json::to_string(&store.parts["stable"].value)?.chars().count(),"reason":"projection_protocol"}));
        let manifest = json!({"version":1,"entries":entries,"projectedChars":projection.text.chars().count()+focus_text.chars().count(),"projectedBytes":projection.text.len()+focus_text.len()});
        let mut messages = store.assemble()?;
        let content = messages[0]["content"].as_str().unwrap_or_default();
        // The exact same immutable inventory is transmitted, stored on Run, and shown by the UI.
        let at = content
            .rfind(",\"question\":")
            .ok_or_else(|| CoreError::new("INVALID_STATE", "本轮问题位置无效。"))?;
        let actual = format!(
            "{},\"contextBridgeManifest\":{},\"nativeMaterials\":{},\"nativeReferences\":{}{}",
            &content[..at],
            serde_json::to_string(&manifest)?,
            serde_json::to_string(&projection.native_materials(ctx))?,
            serde_json::to_string(&references)?,
            &content[at..]
        );
        if actual.len() > 2 * 1024 * 1024 {
            return Err(CoreError::new(
                "LIMIT_EXCEEDED",
                "模型上下文及来源清单超过传输预算。",
            ));
        }
        messages[0]["content"] = json!(actual);
        Ok(FrozenPrompt {
            messages,
            time,
            manifest,
        })
    }
    pub fn assemble(&self) -> Result<Vec<Value>> {
        let mut history = Vec::new();
        let mut material = String::new();
        let mut references = String::new();
        let mut metadata = serde_json::Map::new();
        let mut states = serde_json::Map::new();
        let mut question = Value::Null;
        let mut sorted = self.parts.values().collect::<Vec<_>>();
        sorted.sort_by_key(|p| (p.position, p.key));
        for p in sorted {
            match p.position {
                Position::Stable => {
                    if let Some(values) = p.value.as_object() {
                        for (k, v) in values {
                            metadata.insert(k.clone(), v.clone());
                        }
                    }
                }
                Position::History => {
                    if let Some(items) = p.value.as_array() {
                        history.extend(items.clone());
                    }
                }
                Position::Materials => material.push_str(p.value.as_str().unwrap_or_default()),
                Position::References => references.push_str(p.value.as_str().unwrap_or_default()),
                Position::State => {
                    if let Some(values) = p.value["metadata"].as_object() {
                        for (k, v) in values {
                            metadata.insert(k.clone(), v.clone());
                        }
                    }
                    let mut state = p.value.as_object().cloned().unwrap_or_default();
                    state.remove("metadata");
                    state.remove("automaticProjection");
                    states.insert(p.key.to_string(), Value::Object(state));
                }
                Position::Question => {
                    if !question.is_null() && question != p.value {
                        return Err(CoreError::new("CONTEXT_CONFLICT", "本次问题来源冲突。"));
                    }
                    question = p.value.clone();
                }
            }
        }
        let context = format!("{material}{references}");
        if context.chars().count() > crate::context::MAX_CONTEXT_CHARS
            || serde_json::to_string(&history)?.chars().count() > 20_000
        {
            return Err(CoreError::new(
                "LIMIT_EXCEEDED",
                "本次模型上下文超过已有预算，未静默丢弃正文基准。",
            ));
        }
        let metadata = serde_json::to_string(&metadata)?;
        let inside = &metadata[1..metadata.len() - 1];
        let history_field = if history.is_empty() {
            String::new()
        } else {
            format!("\"learningHistory\":{},", serde_json::to_string(&history)?)
        };
        let text = format!(
            "{{{}\"context\":{},{},\"automaticProjection\":{},\"turnState\":{},\"question\":{}}}",
            history_field,
            serde_json::to_string(&context)?,
            inside,
            serde_json::to_string(
                &self
                    .parts
                    .get("turn")
                    .map(|p| p.value["automaticProjection"].clone())
                    .unwrap_or(Value::Null)
            )?,
            serde_json::to_string(&states)?,
            serde_json::to_string(&question)?
        );
        if text.len() > 2 * 1024 * 1024 {
            return Err(CoreError::new("LIMIT_EXCEEDED", "模型上下文超过传输预算。"));
        }
        Ok(vec![json!({"role":"user","content":text})])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_turn_versioned_and_one_shot_parts_deduplicate_or_fail_before_assembly() {
        let mut store = ContextBridge::default();
        for (key, lifetime, position) in [
            ("stable", Lifetime::Stable, Position::Stable),
            ("turn", Lifetime::Turn, Position::State),
            ("materials", Lifetime::Versioned, Position::Materials),
            ("references", Lifetime::OneShot, Position::References),
        ] {
            let part = PromptPart {
                key,
                lifetime,
                position,
                value: if matches!(position, Position::Stable | Position::State) {
                    json!({"value":"frozen"})
                } else {
                    json!("frozen")
                },
            };
            store.add(&part).unwrap();
            store.add(&part).unwrap();
        }
        assert_eq!(store.parts.len(), 4);
        assert_eq!(
            store
                .add(&PromptPart {
                    key: "materials",
                    lifetime: Lifetime::Versioned,
                    position: Position::Materials,
                    value: json!("changed late")
                })
                .unwrap_err()
                .code,
            "CONTEXT_CONFLICT"
        );
    }
    #[test]
    fn new_turn_provider_keeps_its_frozen_value_and_budget_failure_never_clips_a_baseline() {
        let mut bridge = ContextBridge::default();
        for p in [
            PromptPart {
                key: "stable",
                lifetime: Lifetime::Stable,
                position: Position::Stable,
                value: json!({"type":"learning_request"}),
            },
            PromptPart {
                key: "learningHistory",
                lifetime: Lifetime::Turn,
                position: Position::History,
                value: json!([]),
            },
            PromptPart {
                key: "exam-state",
                lifetime: Lifetime::Turn,
                position: Position::State,
                value: json!({"goal":"frozen local goal"}),
            },
            PromptPart {
                key: "question",
                lifetime: Lifetime::Turn,
                position: Position::Question,
                value: json!("actual question"),
            },
        ] {
            bridge.add(&p).unwrap();
        }
        let sent = bridge.assemble().unwrap();
        let v: Value = serde_json::from_str(sent[0]["content"].as_str().unwrap()).unwrap();
        assert_eq!(v["turnState"]["exam-state"]["goal"], "frozen local goal");
        bridge
            .add(&PromptPart {
                key: "materials",
                lifetime: Lifetime::Versioned,
                position: Position::Materials,
                value: json!("x".repeat(60001)),
            })
            .unwrap();
        assert_eq!(bridge.assemble().unwrap_err().code, "LIMIT_EXCEEDED");
    }
}
