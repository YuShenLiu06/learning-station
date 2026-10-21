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
use std::path::Path;
#[derive(Clone)]
pub struct PromptPart {
    pub key: &'static str,
    pub description: &'static str,
    pub value: Value,
}
#[derive(Default)]
pub struct ContextBridge {
    parts: Vec<PromptPart>,
}
pub struct FrozenPrompt {
    pub messages: Vec<Value>,
    pub time: TimeContext,
    pub manifest: Value,
    pub debug_registrations: Option<Value>,
}
impl ContextBridge {
    pub fn add(&mut self, part: &PromptPart) -> Result<()> {
        if part.key.is_empty() || part.description.trim().is_empty() {
            return Err(CoreError::new("INVALID_CONTEXT_REQUEST", "上下文登记缺少来源或说明。"));
        }
        if let Some(old) = self.parts.iter().find(|p| p.key == part.key) {
            if old.description != part.description || old.value != part.value {
                return Err(CoreError::new("CONTEXT_CONFLICT", "同一上下文来源有冲突，未发送模型请求。"));
            }
            return Ok(());
        }
        if self.parts.len() >= 16 {
            return Err(CoreError::new("LIMIT_EXCEEDED", "上下文片段超过预算。"));
        }
        self.parts.push(part.clone());
        Ok(())
    }
    pub fn debug_registrations(&self) -> Value {
        Value::Array(self.parts.iter().enumerate().map(|(position, part)| {
            json!({"key":part.key,"description":part.description,"position":position,"value":part.value})
        }).collect())
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
        let metadata_only_ids: Vec<_> = ctx.manifest.iter().filter(|m| m.included_chars == 0 && !ctx.automatic_materials.iter().any(|a|a.id==m.id)).map(|m|m.id.clone()).collect();
        let (_, references) = projection.provider_texts(ctx)?;
        let references = crate::citation_context::bridge_references(&references, ctx)?;
        let question = if request.prompt.trim().is_empty() { crate::generation::default_request(&request.mode) } else { &request.prompt };
        let mut store = Self::default();
        store.add(&crate::time_context::bridge_part(&time))?;
        store.add(&crate::context::bridge_part(&references))?;
        if let Some(focus) = &request.focus {
            store.add(&crate::focus::bridge_part(focus))?;
        } else {
            store.add(&automatic_context::bridge_part(projection, ctx))?;
        }
        if let Some(intent) = intent {
            store.add(&crate::exploration::bridge_part(intent))?;
        }
        // Input snapshot only: Pi accepts this once via its normal user entry.
        store.add(&PromptPart { key: "question", description: "本轮用户输入（由 Pi 用户消息入口发送）", value: json!(question) })?;
        let mut entries = if request.focus.is_some() { vec![] } else { projection.manifest_entries(ctx) };
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
            if crate::citation_context::has_quote(ctx, &m.id) {
                continue;
            }
            let metadata_only = metadata_only_ids.contains(&m.id);
            entries.push(json!({"id":format!("reference:{}",m.id),"kind":if metadata_only { "material" } else { "reference" },"status":"added","sourceId":m.id,"version":m.version,"includedChars":m.included_chars,"reason":if metadata_only { "material_metadata_only" } else { "explicit_one_shot" }}));

        }
        for input in request
            .injections
            .iter()
            .filter(|i| i.selection.is_some() && auto_ids.contains(i.document_id.as_str()))
        {
            if crate::citation_context::has_quote(ctx, &input.document_id) {
                continue;
            }
            let mut entry = json!({"id":format!("citation:{}",input.document_id),"kind":"reference","status":"added","sourceId":input.document_id,"version":input.expected_version,"includedChars":input.selection.as_ref().unwrap().chars().count(),"reason":"explicit_exact_selection"});
            if let Some(locator) = &input.locator {
                entry["locator"] = json!(locator);
            }
            entries.push(entry);
        }
        entries.extend(crate::citation_context::inventory(ctx));
        for input in request
            .injections
            .iter()
            .filter(|i| i.selection.is_none() && auto_ids.contains(i.document_id.as_str()))
        {
            let m = ctx
                .automatic_materials
                .iter()
                .find(|m| m.id == input.document_id)
                .unwrap();
            entries.push(json!({"id":format!("explicit-full:{}",m.id),"kind":"reference","status":"retained","sourceId":m.id,"version":m.version,"includedChars":m.text.chars().count(),"sourceMarkers":["citation","material"],"reason":"explicit_full_material_already_projected"}));
        }
        if !request.selection.is_empty() {
            entries.push(json!({"id":"selection","kind":"reference","status":"added","sourceId":request.document_id.as_deref().unwrap_or(&session.section_id),"version":hash(&request.selection),"includedChars":request.selection.chars().count(),"reason":"explicit_one_shot"}));
        }
        let mut focus_text = String::new();
        if let Some(focus) = &request.focus {
            let source = focus.asset.as_ref().and_then(|a|a["id"].as_str()).unwrap_or(&focus.document_id);
            for entry in &mut entries {
                if entry["sourceId"] == source {
                    if let Some(markers) = entry["sourceMarkers"].as_array_mut() {
                        if markers.iter().any(|m| m == "material") { markers.push(json!("focus")); }
                    }
                }
            }
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
        let manifest = json!({"version":1,"entries":entries,"projectedChars":projection.text.chars().count()+focus_text.chars().count(),"projectedBytes":projection.text.len()+focus_text.len()});
        let mut envelope = store.assemble()?;
        envelope["contextBridgeManifest"] = manifest.clone();
        envelope["metadataOnlySourceIds"] = json!(metadata_only_ids);
        let content = serde_json::to_string(&envelope)?;
        if content.len() > 2 * 1024 * 1024 {
            return Err(CoreError::new("LIMIT_EXCEEDED", "模型上下文及来源清单超过传输预算。"));
        }
        let messages = vec![json!({"role":"user","content":content})];
        let debug_registrations=crate::request_debug::enabled().then(||store.debug_registrations());
        Ok(FrozenPrompt {
            debug_registrations,
            messages,
            time,
            manifest,
        })
    }
    pub fn assemble(&self) -> Result<Value> {
        let registrations = self.debug_registrations();
        if serde_json::to_vec(&registrations)?.len() > 2 * 1024 * 1024 {
            return Err(CoreError::new("LIMIT_EXCEEDED", "模型上下文超过传输预算，未截断。"));
        }
        Ok(json!({"bridge":registrations}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_none_and_exact_locators_preserve_the_real_bridge_wire() {
        let d = tempfile::tempdir().unwrap();
        let e = crate::app::Engine::new(d.path().join("lib"), d.path().join("state")).unwrap();
        let mut c = e.lock().unwrap();
        let source = c
            .create_section("合成旧材料", "🙂原句", vec![], vec![])
            .unwrap();
        let (session, _) = c.start_session(&source.id, false).unwrap();
        for locator in [Value::Null, json!({"startChar":1,"endChar":3})] {
            let input = serde_json::from_value(json!({"documentId":source.id,"expectedVersion":source.version,"selection":"原句","locator":locator})).unwrap();
            let mut ctx = crate::context::load(&c.content, &source.id, None).unwrap();
            crate::context::inject(&c.content, &mut ctx, &[input]).unwrap();
            let projection =
                ContextBridge::snapshot(&c.state_dir, &c.content, &session, &ctx).unwrap();
            let request = serde_json::from_value(json!({"sessionId":session.id,"mode":"question","prompt":"解释原句","selection":"","requestId":"legacy_locator_fixture","injections":[{"documentId":source.id,"expectedVersion":source.version,"selection":"原句","locator":locator}]})).unwrap();
            let frozen = ContextBridge::freeze(
                &request,
                &ctx,
                &session,
                None,
                &projection,
                TimeContext::capture(),
            )
            .unwrap();
            let item = frozen.manifest["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["reason"] == "explicit_exact_selection")
                .unwrap();
            if locator.is_null() {
                assert!(item.get("locator").is_none());
            } else {
                assert_eq!(item["locator"], locator);
            }
        }
    }
    #[test]
    fn registration_preserves_order_description_full_content_and_conflict() {
        let mut bridge = ContextBridge::default();
        let time = PromptPart { key: "current_time", description: "本轮时间", value: json!({"referenceTime":"2026-10-21T00:00:00Z"}) };
        let reference = PromptPart { key: "references", description: "明确引用", value: json!("正文".repeat(10000)) };
        bridge.add(&time).unwrap(); bridge.add(&reference).unwrap(); bridge.add(&time).unwrap();
        let snapshot = bridge.debug_registrations();
        assert_eq!(snapshot[0]["key"], "current_time");
        assert_eq!(snapshot[1]["description"], "明确引用");
        assert_eq!(snapshot[1]["value"], reference.value);
        assert_eq!(bridge.add(&PromptPart { value: json!("changed"), ..reference }).unwrap_err().code, "CONTEXT_CONFLICT");
        assert!(bridge.assemble().unwrap().get("question").is_none());
    }
}
