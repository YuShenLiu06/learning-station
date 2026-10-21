//! Decode only known bridge transport containers. Material/code strings stay literal.
use crate::types::{CoreError, Result};
use serde_json::{json, Value};
use std::{cmp::Ordering, path::Path};

fn container(value: &Value) -> Value {
    value.as_str().and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(|v| v.is_object() || v.is_array()).unwrap_or_else(|| value.clone())
}
fn decoded_content(value: &Value) -> Value {
    let mut decoded = container(value);
    // These fields are JSON-in-string protocol envelopes, never frame.text, code or question.
    if let Some(object) = decoded.as_object_mut() {
        for key in ["context", "nativeReferences"] {
            if let Some(value) = object.get_mut(key) { *value = container(value); }
        }
    }
    decoded
}
fn position_cmp(a: &Value, b: &Value) -> Ordering {
    let paths = |v: &Value| v["path"].as_array().cloned().unwrap_or_default();
    let (a_path, b_path) = (paths(a), paths(b));
    for (a, b) in a_path.iter().zip(&b_path) {
        let order = match (a.as_u64(), b.as_u64()) {
            (Some(a), Some(b)) => a.cmp(&b),
            _ => {
                let rank = |v: &Value| if v == "system" { String::new() } else { v.as_str().unwrap_or("").to_owned() };
                rank(a).cmp(&rank(b))
            }
        };
        if order != Ordering::Equal { return order; }
    }
    a_path.len().cmp(&b_path.len()).then_with(|| a["start"].as_u64().unwrap_or(0).cmp(&b["start"].as_u64().unwrap_or(0)))
}
fn at(body: &Value, source: &Value) -> Option<Value> {
    let mut value = body;
    for key in source["path"].as_array()? {
        value = if let Some(index) = key.as_u64() { value.get(index as usize)? } else { value.get(key.as_str()?)? };
    }
    if let Some(start) = source["start"].as_u64() {
        // Source offsets originate in JavaScript String.slice (UTF-16), not Rust byte offsets.
        let text: Vec<_> = value.as_str()?.encode_utf16().collect();
        let end = source["end"].as_u64().unwrap_or(text.len() as u64) as usize;
        if start as usize > end || end > text.len() { return None; }
        return String::from_utf16(&text[start as usize..end]).ok().map(Value::String);
    }
    Some(value.clone())
}
fn location(source: &Value) -> Value {
    let mut value = json!({"path":source["path"]});
    for key in ["start", "end"] { if let Some(item) = source.get(key) { value[key] = item.clone(); } }
    value
}
fn snapshot_entries(items: Option<&Vec<Value>>, entries: &[Value]) -> Vec<Value> {
    items.into_iter().flatten().map(|item| {
        let mut input = item.clone();
        if let Some(object) = input.as_object_mut() { if let Some(value) = object.remove("value") { object.insert("content".into(), value); } }
        let positions: Vec<_> = entries.iter().filter(|entry| entry.get("registrationKey").is_some()
            && entry["registrationKey"] == item["key"]
            && item.get("userEntryId").is_none_or(|id| entry.get("userEntryId")==Some(id))
            && item.get("runId").is_none_or(|id| entry.get("runId")==Some(id)))
            .map(|entry| entry["finalPosition"].clone()).collect();
        input["finalLocations"] = json!(positions);
        input["locationStatus"] = json!(if positions.is_empty() {"没有已记录的匹配位置；冻结值不是最终实发"} else {"按 key 与已有回合标识关联；同一 key 可有多个片段，不表示逐值一一对应"});
        input
    }).collect()
}
pub(super) fn document(record: &Value) -> Result<Value> {
    if record["complete"] != true { return Err(CoreError::new("DEBUG_INCOMPLETE", "请求体不完整，无法导出完整桥内容；请开启调试后重新采集较小请求。")); }
    let sources = record["sources"].as_array().filter(|s| !s.is_empty())
        .ok_or_else(|| CoreError::new("DEBUG_INCOMPLETE", "未记录最终来源定位，无法导出桥内容；请开启调试后重新发送问题。"))?;
    let mut sources = sources.clone();
    sources.sort_by(position_cmp);
    let questions = sources.iter().filter(|s|s["registrationKey"]=="question").count();
    let mut entries = Vec::new();
    let mut unknown = Vec::new();
    for source in &sources {
        let value = at(&record["body"], source);
        if source["origin"] == "unknown" && value.as_ref().is_none_or(|v| v.as_str().is_none_or(|text| !text.trim().is_empty())) { unknown.push(location(source)); }
        let current_question = source["registrationKey"] == "question" && match source.get("userEntryId") {
            Some(id) => record["identity"].get("userEntryId")==Some(id),
            None => match source.get("runId") { Some(id)=>record["identity"].get("runId")==Some(id),None=>questions==1 }
        };
        if source["origin"] != "bridge" && !current_question { continue; }
        let content = value.ok_or_else(|| CoreError::new("DEBUG_INCOMPLETE", "来源位置与请求体不一致，未导出；请重新采集。"))?;
        let decoded = if source["origin"] == "bridge" && source["registrationKey"] != "references" { decoded_content(&content) } else { content };
        let mut entry = json!({"sourceKey":source.get("registrationKey").unwrap_or(&source["name"]),"name":source["name"],"origin":source["origin"],"registered":source.get("registrationKey").is_some(),"finalPosition":location(source),"content":decoded});
        for key in ["registrationKey", "description", "sourcePath", "runId", "userEntryId"] { if let Some(value) = source.get(key) { entry[key] = value.clone(); } }
        entries.push(entry);
    }
    if entries.is_empty() { return Err(CoreError::new("DEBUG_INCOMPLETE", "本请求没有可定位的桥片段，无法导出桥内容。")); }
    let registrations = record["registrations"].as_array();
    let projections = record["projectionRegistrations"].as_array();
    let complete = |items: Option<&Vec<Value>>| items.is_some_and(|items| !items.is_empty() && items.iter().all(|item| item.get("value").is_some() && item.get("omitted").is_none()));
    let registration_complete = complete(registrations);
    let projection_complete = complete(projections);
    Ok(json!({
        "schemaVersion":2,"scope":"ContextBridge（不含完整 Pi 历史、系统提示或工具定义）","redacted":true,
        "complete":registration_complete && projection_complete && unknown.is_empty(),
        "request":{"sequence":record["sequence"],"time":record["time"],"phase":record["phase"],"protocol":record["protocol"],"identity":record["identity"]},
        "registrationSnapshot":{"complete":registration_complete,"status":if registration_complete {"完整冻结输入；数组顺序不是实发顺序"} else {"不完整：快照缺失或省略；缺失内容无法导出，未从最终请求反推补造"},"entries":snapshot_entries(registrations,&entries)},
        "projectionSnapshot":{"complete":projection_complete,"status":"Pi 可见的注册值；不是最终 wire 顺序。同一 key 可多值、多位置；最终内容与位置见 finalProjection.entries","entries":snapshot_entries(projections,&entries)},
        "finalProjection":{"captureComplete":true,"attributionComplete":unknown.is_empty(),"status":"全部已绑定桥片段及本轮问题；保留旧基准片段自身的 run/user 标识，不将原生历史用户消息混入桥导出；按最终消息及片段顺序排列","entries":entries,"unattributedLocations":unknown},
        "snapshotOmitted":record["snapshotOmitted"],
        "decoding":"只解析已知桥容器；引用原文、材料代码与问题中的字面反斜线 n 不二次解码。"
    }))
}
pub fn decode(sequence: u64) -> Result<Value> {
    let state = super::state().lock().unwrap_or_else(|e| e.into_inner());
    let record = state.records.iter().find(|r| r["sequence"] == sequence)
        .ok_or_else(|| CoreError::new("DEBUG_EXPIRED", "记录已清除或淘汰；请重新采集后导出。"))?;
    document(record)
}
pub fn save(sequence: u64, state_dir: &Path) -> Result<Value> {
    // Build from the same sanitized capture as the decode view, never renderer-supplied JSON.
    let document = decode(sequence)?;
    let path = state_dir.join("exports").join(format!("context-bridge-{}-{}.json", sequence, uuid::Uuid::new_v4()));
    crate::app::atomic_json(&path, &document)?;
    Ok(json!({"path":path,"complete":document["complete"]}))
}

#[cfg(test)] mod tests {
    use super::*;
    fn record() -> Value {
        let body = json!({"messages":(0..12).map(|i| json!({"content":format!("message {i}")})).collect::<Vec<_>>(),"headers":{"authorization":"NEVER_EXPORT"}});
        json!({"sequence":7,"complete":true,"body":body,"identity":{"sessionId":"s","runId":"r","attempt":"a"},"registrations":[{"key":"question","value":"hi"}],"sources":[{"path":["messages",10,"content"],"origin":"bridge","name":"later"},{"path":["messages",2,"content"],"origin":"bridge","name":"earlier"}]})
    }
    #[test] fn export_is_nested_complete_ordered_and_never_contains_headers() {
        let mut r = record();
        let text = "first\nsecond\\n".repeat(1500);
        r["body"]["messages"][2]["content"] = json!(json!({"context":json!({"quote":"a\nb"}).to_string(),"frames":[{"text":text,"code":"{\"literal\":\"\\n\"}"}]}).to_string());
        let doc = document(&r).unwrap();
        let serialized = serde_json::to_string_pretty(&doc).unwrap();
        let parsed: Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["finalProjection"]["entries"][0]["finalPosition"]["path"][1],2);
        assert_eq!(parsed["finalProjection"]["entries"][0]["content"]["frames"][0]["text"],text);
        assert_eq!(parsed["finalProjection"]["entries"][0]["content"]["frames"][0]["code"],"{\"literal\":\"\\n\"}");
        assert_eq!(parsed["finalProjection"]["entries"][0]["content"]["context"]["quote"],"a\nb");
        assert!(!serialized.contains("NEVER_EXPORT")); assert!(serialized.contains("sessionId"));
        assert!(parsed["registrationSnapshot"]["entries"][0].get("description").is_none());
    }
    #[test] fn multiple_focus_fragments_keep_identity_and_exclude_native_history() {
        let mut r=record();
        r["identity"]["userEntryId"]=json!("current-user");
        let literal=r#"{"code":"\\n"}"#;
        r["body"]["messages"][0]["content"]=json!("\n  ");
        r["body"]["messages"][1]["content"]=json!("OLD_NATIVE_QUESTION");
        r["body"]["messages"][2]["content"]=json!(json!({"documentId":"fixture"}).to_string());
        r["body"]["messages"][3]["content"]=json!(literal);
        r["body"]["messages"][10]["content"]=json!(json!({"frames":[{"text":"COMPLETE_FOCUS_TAIL","code":literal}]}).to_string());
        r["body"]["messages"][11]["content"]=json!("CURRENT_QUESTION");
        r["sources"]=json!([
            {"path":["messages",10,"content"],"origin":"bridge","registrationKey":"focus","description":"保留全文","runId":"old-run","userEntryId":"old-user","name":"material"},
            {"path":["messages",1,"content"],"origin":"user","registrationKey":"question","userEntryId":"old-user","name":"user"},
            {"path":["messages",2,"content"],"origin":"bridge","registrationKey":"focus","description":"本轮位置","runId":"r","userEntryId":"current-user","name":"focus"},
            {"path":["messages",0,"content"],"origin":"unknown","name":"blank"},
            {"path":["messages",3,"content"],"origin":"bridge","registrationKey":"references","name":"references"},
            {"path":["messages",11,"content"],"origin":"user","registrationKey":"question","userEntryId":"current-user","name":"user"}
        ]);
        r["registrations"]=json!([{"key":"focus","description":"冻结聚焦材料","value":{"documentId":"fixture"}}]);
        r["projectionRegistrations"]=json!([{"key":"focus","description":"保留全文","runId":"old-run","userEntryId":"old-user","value":{"frames":[]}},{"key":"focus","description":"本轮位置","runId":"r","userEntryId":"current-user","value":{"documentId":"fixture"}}]);
        let doc=document(&r).unwrap();let entries=doc["finalProjection"]["entries"].as_array().unwrap();
        assert_eq!(entries.len(),4);assert_eq!(entries.iter().filter(|e|e["registrationKey"]=="focus").count(),2);
        assert_eq!(entries[0]["description"],"本轮位置");assert_eq!(entries[1]["content"],literal);
        assert_eq!(entries[2]["runId"],"old-run");assert_eq!(entries[2]["content"]["frames"][0]["code"],literal);
        assert_eq!(entries[3]["content"],"CURRENT_QUESTION");assert_eq!(doc["complete"],true);
        assert!(!doc.to_string().contains("OLD_NATIVE_QUESTION"));
        assert_eq!(doc["projectionSnapshot"]["entries"][0]["finalLocations"][0]["path"][1],10);
    }
    #[test] fn explicit_save_matches_decode_is_private_and_survives_capture_clear() {
        let dir = tempfile::tempdir().unwrap();
        let r = record();
        {
            let mut state = super::super::state().lock().unwrap();
            state.set(true); state.records.push(r);
        }
        let expected = decode(7).unwrap();
        let result = save(7, dir.path()).unwrap();
        let path = Path::new(result["path"].as_str().unwrap());
        let bytes = std::fs::read(path).unwrap();
        let parsed: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed, expected);
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        super::super::set(false);
        assert!(decode(7).is_err()); assert!(path.exists());
    }
    #[test] fn missing_snapshot_and_unknown_are_explicit_and_incomplete_body_is_rejected() {
        let mut r = record(); r["registrations"] = Value::Null;
        assert_eq!(document(&r).unwrap()["complete"],false);
        r["registrations"] = json!([{"key":"turn","omitted":"8 KiB"}]);
        assert_eq!(document(&r).unwrap()["registrationSnapshot"]["complete"],false);
        r["sources"][0]["origin"] = json!("unknown");
        assert_eq!(document(&r).unwrap()["finalProjection"]["attributionComplete"],false);
        r["complete"] = json!(false); assert!(document(&r).is_err());
    }
    #[test] fn utf16_fragments_and_registered_question_preserve_original_text() {
        let mut r = record(); r["body"]["messages"][2]["content"] = json!("😀{\"a\":1}end");
        r["sources"][1]["start"] = json!(2); r["sources"][1]["end"] = json!(9);
        r["sources"][0]["origin"] = json!("user"); r["sources"][0]["registrationKey"] = json!("question");
        r["body"]["messages"][10]["content"] = json!("{\"do_not_parse_question\":1}");
        let doc = document(&r).unwrap();
        assert_eq!(doc["finalProjection"]["entries"][0]["content"],json!({"a":1}));
        assert!(doc["finalProjection"]["entries"][1]["content"].is_string());
        assert_eq!(doc["registrationSnapshot"]["entries"][0]["finalLocations"][0]["path"][1],10);
    }
}
