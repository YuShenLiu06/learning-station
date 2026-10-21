//! Volatile debug records only. No filesystem, DB, telemetry, or logging sink.
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
const MAX_RECORDS: usize = 40;
const MAX_BYTES: usize = 4 * 1024 * 1024;
#[derive(Default)]
pub struct DebugState {
    enabled: bool,
    active_session: Option<String>,
    last_sequences: Vec<(String,u64)>,
    epoch: u64,
    sequence: u64,
    records: Vec<Value>,
    metadata: Vec<(String, Value)>,
    dropped: usize,
}
impl DebugState {
    fn set(&mut self, enabled: bool) {
        self.enabled=enabled; self.epoch+=1;
        self.records.clear(); self.metadata.clear(); self.last_sequences.clear(); self.dropped=0;
    }
    fn snapshot(&self) -> Value { json!({"enabled":self.enabled,"records":self.records,"dropped":self.dropped}) }
    fn capture(&mut self, epoch: u64, key: &str, meta: &Value, event: &Value) {
        if !self.enabled || epoch!=self.epoch { return; }
        if event["type"]=="debug_status" {
            if let Some(record)=self.records.iter_mut().find(|r|r["captureId"]==key) {
                record["status"]=event["status"].clone();
            }
            return;
        }
        // The sidecar redacts before IPC; never accept arbitrary unbounded events.
        if event.to_string().len()>768*1024 { self.dropped+=1; return; }
        let sid=meta["sessionId"].as_str().unwrap_or("");
        let previous=self.last_sequences.iter().find(|(id,_)|id==sid).map(|(_,seq)|*seq);
        self.sequence+=1;
        self.last_sequences.retain(|(id,_)|id!=sid);
        if self.last_sequences.len()>=80 {self.last_sequences.remove(0);}
        self.last_sequences.push((sid.into(),self.sequence));
        let mut record=event.clone();
        record["sequence"]=json!(self.sequence); record["captureId"]=json!(key);
        record["previousSequence"]=json!(previous);
        record["identity"]=meta.clone();
        record["identity"]["model"]=record["body"]["model"].clone(); record["status"]=json!("sending");
        self.records.push(record);
        while self.records.len()>MAX_RECORDS || self.records.iter().map(|r|r.to_string().len()).sum::<usize>()>MAX_BYTES {
            self.records.remove(0); self.dropped+=1;
        }
    }
}
fn state() -> &'static Mutex<DebugState> { static STATE:OnceLock<Mutex<DebugState>>=OnceLock::new(); STATE.get_or_init(||Mutex::new(DebugState::default())) }
pub fn set(enabled: bool) -> Value { let mut s=state().lock().unwrap_or_else(|e|e.into_inner()); s.set(enabled); s.snapshot() }
pub fn snapshot() -> Value { state().lock().unwrap_or_else(|e|e.into_inner()).snapshot() }
pub fn clear_session(session: &str) {
    let mut s=state().lock().unwrap_or_else(|e|e.into_inner());
    s.epoch+=1;
    s.records.retain(|r|r["identity"]["sessionId"]!=session);
    s.metadata.retain(|(_,m)|m["sessionId"]!=session);
    s.last_sequences.retain(|(id,_)|id!=session);
    if s.active_session.as_deref()==Some(session) {s.active_session=Some(String::new());}
}
pub fn enter_session(session: &str) { state().lock().unwrap_or_else(|e|e.into_inner()).active_session=Some(session.into()); }
pub fn register(run: &str, mut metadata: Value) {
    let mut s=state().lock().unwrap_or_else(|e|e.into_inner());
    if !s.enabled { return; }
    metadata["retry"]=json!(s.metadata.iter().any(|(id,m)|id!=run && m["sessionId"]==metadata["sessionId"] && m["userEntryId"]==metadata["userEntryId"] && !metadata["userEntryId"].is_null()));
    if s.metadata.len()>=80 { s.metadata.remove(0); }
    s.metadata.push((run.into(),metadata));
}
pub fn gate(run: &str, fallback: &Value) -> (Option<u64>, Value) {
    let s=state().lock().unwrap_or_else(|e|e.into_inner());
    let registered=s.metadata.iter().rev().find(|(id,_)|id==run).map(|(_,m)|m.clone()).unwrap_or(Value::Null);
    let mut meta=fallback.clone();
    if let Some(fields)=registered.as_object(){for (key,value) in fields {meta[key]=value.clone();}}
    let sid=meta["sessionId"].as_str().unwrap_or("");
    let visible=s.active_session.as_deref().is_none_or(|active|active==sid) || sid.starts_with("utility:");
    ((s.enabled && visible).then_some(s.epoch),meta)
}
pub fn capture(epoch:u64,key:&str,meta:&Value,event:&Value) { state().lock().unwrap_or_else(|e|e.into_inner()).capture(epoch,key,meta,event); }
pub fn finish_attempt(attempt:&str,error:Option<&str>) {
    let mut s=state().lock().unwrap_or_else(|e|e.into_inner());
    for record in s.records.iter_mut().filter(|r|r["identity"]["attempt"]==attempt) {record["runOutcome"]=json!(error.unwrap_or("complete"));}
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn volatile_gate_limits_and_late_events() {
        let mut s=DebugState::default(); let e=json!({"type":"debug_request","body":{"messages":["fixture"]}});
        s.capture(0,"a",&json!({"sessionId":"a"}),&e); assert!(s.records.is_empty());
        s.set(true); let epoch=s.epoch;
        for i in 0..45 {s.capture(epoch,&i.to_string(),&json!({"sessionId":"a"}),&e);}
        assert_eq!(s.records.len(),40); assert_eq!(s.dropped,5);
        s.set(false); s.capture(epoch,"late",&Value::Null,&e); assert!(s.records.is_empty()); assert!(s.metadata.is_empty());
        s.set(true); s.capture(epoch,"old",&Value::Null,&e); assert!(s.records.is_empty());
    }
}
