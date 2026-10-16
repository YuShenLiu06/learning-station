//! AnkiConnect on one fixed loopback port. Never launches Anki or reads its collection/config.
use crate::{
    app::{atomic_json, Engine},
    review,
    types::*,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
static WRITE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const MODEL: &str = "LearningStationBasicV1";
pub struct Anki {
    dir: PathBuf,
    client: reqwest::Client,
    url: String,
}
fn err(code: &str, msg: &str) -> CoreError {
    CoreError::new(code, msg)
}
impl Anki {
    pub fn new(state_dir: &Path) -> Result<Self> {
        Self::at(state_dir, "http://127.0.0.1:8765")
    }
    fn at(state_dir: &Path, url: &str) -> Result<Self> {
        Ok(Self {
            dir: state_dir.join("anki"),
            url: url.into(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(8))
                .build()
                .map_err(|_| err("ANKI_OFFLINE", "本机Anki连接不可用。"))?,
        })
    }
    async fn call(&self, action: &str, params: Value) -> Result<Value> {
        let mut response = self
            .client
            .post(&self.url)
            .json(&json!({"action":action,"version":6,"params":params}))
            .send()
            .await
            .map_err(|_| {
                err(
                    "ANKI_OFFLINE",
                    "未连接本机Anki。请打开已安装的Anki并确认AnkiConnect运行，再点击重试。",
                )
            })?;
        if !response.status().is_success() {
            return Err(err("ANKI_HTTP", "AnkiConnect未接受请求，未确认操作。"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| err("ANKI_RESPONSE", "Anki响应不完整。"))?
        {
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err(err("ANKI_RESPONSE", "Anki响应过大，请减少牌组卡片范围。"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|_| err("ANKI_RESPONSE", "Anki返回了无法识别的数据。"))?;
        if !v.is_object() || v.get("result").is_none() || v.get("error").is_none() {
            return Err(err("ANKI_RESPONSE", "AnkiConnect协议版本不兼容。"));
        }
        if !v["error"].is_null() {
            let msg = v["error"].as_str().unwrap_or("").to_lowercase();
            if msg.contains("key") || msg.contains("auth") || msg.contains("permission") {
                return Err(err(
                    "ANKI_AUTH_REQUIRED",
                    "AnkiConnect要求单独授权或API密钥；学习站尚未获得此权限，未读取插件密钥。",
                ));
            }
            return Err(err(
                "ANKI_API",
                "Anki拒绝了操作，请在Anki中检查牌组和卡片状态。",
            ));
        }
        Ok(v["result"].clone())
    }
    async fn decks(&self) -> Result<Value> {
        let decks = self.call("deckNamesAndIds", json!({})).await?;
        let m = decks
            .as_object()
            .filter(|m| m.len() <= 2000)
            .ok_or_else(|| err("ANKI_RESPONSE", "Anki牌组列表无效。"))?;
        Ok(json!(m
            .iter()
            .filter_map(|(n, id)| id.as_u64().map(|i| json!({"id":i,"name":n})))
            .collect::<Vec<_>>()))
    }
    pub async fn check(&self) -> Value {
        match self.call("version", json!({})).await {
            Ok(version) if version.as_u64().is_some_and(|v| v >= 6) => match self.decks().await {
                Ok(decks) => {
                    json!({"connected":true,"code":"CONNECTED","message":"已连接本机AnkiConnect。","version":version,"decks":decks})
                }
                Err(e) => json!({"connected":false,"code":e.code,"message":e.message,"decks":[]}),
            },
            Ok(_) => {
                json!({"connected":false,"code":"ANKI_VERSION","message":"AnkiConnect版本不兼容。","decks":[]})
            }
            Err(e) => json!({"connected":false,"code":e.code,"message":e.message,"decks":[]}),
        }
    }
    async fn deck(&self, name: &str) -> Result<()> {
        if name.is_empty() || name.len() > 1000 || name.chars().any(char::is_control) {
            return Err(err("INVALID_INPUT", "请选择真实Anki牌组。"));
        }
        if self
            .decks()
            .await?
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["name"].as_str() == Some(name))
        {
            Ok(())
        } else {
            Err(err("NOT_FOUND", "Anki牌组不存在，请刷新列表。"))
        }
    }
    async fn card(&self, id: u64) -> Result<Value> {
        if id == 0 {
            return Err(err("INVALID_INPUT", "Anki卡片编号无效。"));
        }
        let cards = self.call("cardsInfo", json!({"cards":[id]})).await?;
        let c = cards
            .as_array()
            .and_then(|a| a.first())
            .filter(|c| c["cardId"].as_u64() == Some(id))
            .ok_or_else(|| err("NOT_FOUND", "Anki卡片不存在。"))?;
        for field in ["question", "answer"] {
            if c[field].as_str().is_none_or(|s| s.len() > 128 * 1024) {
                return Err(err("ANKI_RESPONSE", "卡片内容无效或过长。"));
            }
        }
        Ok(c.clone())
    }
    fn token(c: &Value) -> String {
        format!(
            "av_{}",
            hash(
                &json!([
                    c["cardId"],
                    c["mod"],
                    c["reps"],
                    c["lapses"],
                    c["queue"],
                    c["due"],
                    c["question"],
                    c["answer"]
                ])
                .to_string()
            )
        )
    }
    fn mapping(&self) -> Result<BTreeMap<String, Value>> {
        read_json(&self.dir.join("sources.json")).map(|v| v.unwrap_or_default())
    }
    pub async fn due(&self, deck: Option<&str>) -> Result<Value> {
        let mut query = "is:due -is:suspended".to_string();
        if let Some(d) = deck {
            self.deck(d).await?;
            query.push_str(&format!(
                " deck:\"{}\"",
                d.replace('\\', "\\\\").replace('"', "\\\"")
            ));
        }
        let found = self.call("findCards", json!({"query":query})).await?;
        let ids = found
            .as_array()
            .ok_or_else(|| err("ANKI_RESPONSE", "到期卡清单无效。"))?;
        let total = ids.len();
        let selected = ids.iter().take(25).cloned().collect::<Vec<_>>();
        if selected.is_empty() {
            return Ok(json!({"cards":[],"total":total,"mode":"anki"}));
        }
        let cards = self.call("cardsInfo", json!({"cards":selected})).await?;
        let mapping = self.mapping()?;
        let mut result = vec![];
        for c in cards
            .as_array()
            .ok_or_else(|| err("ANKI_RESPONSE", "到期卡数据无效。"))?
        {
            let id = c["cardId"]
                .as_u64()
                .ok_or_else(|| err("ANKI_RESPONSE", "到期卡编号无效。"))?;
            let question = c["question"]
                .as_str()
                .filter(|s| s.len() <= 128 * 1024)
                .ok_or_else(|| err("ANKI_RESPONSE", "到期卡内容过长。"))?;
            let key = c["note"].as_u64().unwrap_or(0).to_string();
            result.push(json!({"cardId":id,"deckName":c["deckName"],"question":question,"reviewToken":Self::token(c),"nextReviews":c["nextReviews"],"source":mapping.get(&key).cloned().unwrap_or(Value::Null)}));
        }
        Ok(json!({"cards":result,"total":total,"mode":"anki"}))
    }
    async fn checked_card(&self, id: u64, token: &str) -> Result<Value> {
        if !valid_id(token) {
            return Err(err("INVALID_INPUT", "复习版本无效。"));
        }
        let c = self.card(id).await?;
        if Self::token(&c) != token {
            return Err(err(
                "ANKI_CARD_CHANGED",
                "此卡已在Anki中变化，请刷新后再复习，未写入旧结果。",
            ));
        }
        Ok(c)
    }
    pub async fn reveal(&self, id: u64, token: &str) -> Result<Value> {
        let c = self.checked_card(id, token).await?;
        atomic_json(
            &self.dir.join("revealed").join(format!("{token}.json")),
            &json!({"cardId":id,"token":token}),
        )?;
        Ok(json!({"cardId":id,"answer":c["answer"]}))
    }
    fn op_path(&self, request: &str) -> Result<PathBuf> {
        if !valid_id(request) || request.len() < 8 || request.len() > 128 {
            return Err(err("INVALID_INPUT", "Anki请求编号无效。"));
        }
        Ok(self
            .dir
            .join("operations")
            .join(format!("{}.json", hash(request))))
    }
    fn read_operation(&self, request: &str) -> Result<Option<Value>> {
        let path = self.op_path(request)?;
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(CoreError::io(e)),
            Ok(m) if m.file_type().is_symlink() || !m.is_file() || m.len() > 128 * 1024 => {
                return Err(err("INVALID_STATE", "Anki操作回执不可安全读取，未重发。"))
            }
            _ => {}
        }
        let v: Value = serde_json::from_slice(&fs::read(path)?)?;
        let fields = [
            "requestId",
            "status",
            "cardId",
            "noteIds",
            "digest",
            "error",
        ];
        if v.as_object()
            .is_none_or(|o| o.keys().any(|k| !fields.contains(&k.as_str())))
            || v["requestId"].as_str() != Some(request)
            || !matches!(
                v["status"].as_str(),
                Some("unknown" | "failed" | "succeeded")
            )
            || v["digest"]
                .as_str()
                .is_none_or(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
            || (v.get("cardId").is_none() && v.get("noteIds").is_none())
            || v.get("cardId").is_some_and(|id| id.as_u64().is_none())
            || v.get("noteIds").is_some_and(|ids| {
                ids.as_array().is_none_or(|a| {
                    a.len() > 100 || a.iter().any(|n| n.as_u64().is_none_or(|id| id == 0))
                })
            })
        {
            return Err(err(
                "INVALID_STATE",
                "Anki操作回执格式无效，不能确认成功或重新发送。",
            ));
        }
        if v["status"] == "succeeded"
            && v.get("noteIds")
                .is_some_and(|ids| ids.as_array().unwrap().is_empty())
        {
            return Err(err("INVALID_STATE", "Anki成功回执缺少真实编号。"));
        }
        Ok(Some(v))
    }
    pub fn operation(&self, request: &str) -> Result<Value> {
        self.read_operation(request)?
            .ok_or_else(|| err("NOT_FOUND", "尚未记录该Anki操作。"))
    }
    fn old(&self, request: &str, digest: &str) -> Result<Option<Value>> {
        let old = self.read_operation(request)?;
        if old
            .as_ref()
            .is_some_and(|v| v["digest"].as_str() != Some(digest))
        {
            return Err(err("CONFLICT", "Anki请求编号已用于另一操作。"));
        }
        Ok(old)
    }
    fn save_op(&self, request: &str, v: &Value) -> Result<()> {
        atomic_json(&self.op_path(request)?, v)
    }
    fn failed(
        &self,
        request: &str,
        digest: &str,
        mut fields: Value,
        error: CoreError,
    ) -> Result<Value> {
        if let Some(old) = self.old(request, digest)? {
            return Ok(old);
        }
        fields["requestId"] = json!(request);
        fields["digest"] = json!(digest);
        fields["status"] = json!("failed");
        fields["error"] = json!(error);
        self.save_op(request, &fields)?;
        Ok(fields)
    }
    pub async fn answer(&self, id: u64, token: &str, ease: u8, request: &str) -> Result<Value> {
        let _guard = WRITE_LOCK.lock().await;
        let digest = hash(&json!(["answer", id, token, ease]).to_string());
        if let Some(old) = self.old(request, &digest)? {
            return Ok(old);
        }
        let preflight = async {
            if !(1..=4).contains(&ease) {
                return Err(err("INVALID_INPUT", "请选择1至4档复习结果。"));
            }
            if !valid_id(token) || !token.starts_with("av_") {
                return Err(err("INVALID_INPUT", "复习版本无效。"));
            }
            let revealed: Value = read_json(
                &self.dir.join("revealed").join(format!("{token}.json")),
            )?
            .ok_or_else(|| err("INVALID_STATE", "请先显示本张卡的答案，再选择复习结果。"))?;
            if revealed["cardId"].as_u64() != Some(id) {
                return Err(err("CONFLICT", "答案与卡片不匹配。"));
            }
            self.checked_card(id, token).await?;
            Ok::<(), CoreError>(())
        }
        .await;
        if let Err(error) = preflight {
            return self.failed(request, &digest, json!({"cardId":id}), error);
        }
        let mut op = json!({"requestId":request,"status":"unknown","cardId":id,"digest":digest});
        self.save_op(request, &op)?;
        match self
            .call(
                "answerCards",
                json!({"answers":[{"cardId":id,"ease":ease}]}),
            )
            .await
        {
            Ok(v) if v == json!([true]) => op["status"] = json!("succeeded"),
            Ok(_) => {
                op["status"] = json!("failed");
                op["error"] = json!({"code":"ANKI_REJECTED","message":"Anki未确认本次复习。"});
            }
            Err(e) => op["error"] = json!(e),
        }
        self.save_op(request, &op)?;
        Ok(op)
    }
    pub async fn add(&self, items: &[Value], deck: &str, request: &str) -> Result<Value> {
        let _guard = WRITE_LOCK.lock().await;
        let ids = items.iter().map(|v| v["id"].clone()).collect::<Vec<_>>();
        let digest = hash(&json!(["add", ids, deck]).to_string());
        if let Some(old) = self.old(request, &digest)? {
            return Ok(old);
        }
        if items.is_empty() || items.len() > 100 {
            return self.failed(
                request,
                &digest,
                json!({"noteIds":[]}),
                err("INVALID_INPUT", "每次选择1至100条本机复习卡。"),
            );
        }
        if let Err(error) = self.deck(deck).await {
            return self.failed(request, &digest, json!({"noteIds":[]}), error);
        }
        let mut op = json!({"requestId":request,"status":"unknown","noteIds":[],"digest":digest});
        self.save_op(request, &op)?;
        let attempt = match tokio::time::timeout(
            Duration::from_secs(90),
            self.add_inner(items, deck, &mut op, request),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(err(
                "ANKI_TIMEOUT",
                "写入等待超时，部分结果可能已保存；请先核对，系统不自动重发。",
            )),
        };
        if let Err(e) = attempt {
            op["error"] = json!(e);
        } else {
            op["status"] = json!("succeeded");
        }
        self.save_op(request, &op)?;
        Ok(op)
    }
    async fn add_inner(
        &self,
        items: &[Value],
        deck: &str,
        op: &mut Value,
        request: &str,
    ) -> Result<()> {
        let names = self.call("modelNames", json!({})).await?;
        if !names
            .as_array()
            .is_some_and(|a| a.iter().any(|n| n == MODEL))
        {
            self.call("createModel",json!({"modelName":MODEL,"inOrderFields":["Front","Back","Source"],"css":".card{font-family:system-ui;font-size:20px;text-align:left}","cardTemplates":[{"Name":"学习站复习","Front":"{{Front}}","Back":"{{FrontSide}}<hr id=answer>{{Back}}<hr>{{Source}}"}]})).await?;
        }
        if self
            .call("modelFieldNames", json!({"modelName":MODEL}))
            .await?
            != json!(["Front", "Back", "Source"])
        {
            return Err(err(
                "ANKI_MODEL_CONFLICT",
                "Anki中同名学习站模板字段已改变，未覆盖现有模板。",
            ));
        }
        let mut mapping = self.mapping()?;
        let mut note_ids = vec![];
        for item in items {
            let id = item["id"]
                .as_str()
                .filter(|id| valid_id(id))
                .ok_or_else(|| err("INVALID_INPUT", "本机卡编号无效。"))?;
            let tag = format!("learning_station_{}", &hash(id)[..32]);
            let existing = self
                .call("findNotes", json!({"query":format!("tag:{tag}")}))
                .await?;
            let found = existing
                .as_array()
                .ok_or_else(|| err("ANKI_RESPONSE", "Anki去重结果无效。"))?;
            let nid = if found.is_empty() {
                let source = source(item);
                let note = json!({"deckName":deck,"modelName":MODEL,"fields":{"Front":html(item["front"].as_str().unwrap_or("")),"Back":html(item["back"].as_str().unwrap_or("")),"Source":html(&source.to_string())},"options":{"allowDuplicate":false},"tags":["learning_station",tag]});
                self.call("addNote", json!({"note":note}))
                    .await?
                    .as_u64()
                    .ok_or_else(|| err("ANKI_RESPONSE", "Anki未返回真实笔记编号。"))?
            } else if found.len() == 1 {
                found[0]
                    .as_u64()
                    .ok_or_else(|| err("ANKI_RESPONSE", "Anki笔记编号无效。"))?
            } else {
                return Err(err(
                    "ANKI_DUPLICATE",
                    "Anki中发现多个同源笔记，请先在Anki核对，未新增副本。",
                ));
            };
            mapping.insert(nid.to_string(), source(item));
            atomic_json(&self.dir.join("sources.json"), &mapping)?;
            note_ids.push(nid);
            op["noteIds"] = json!(note_ids);
            self.save_op(request, op)?;
        }
        Ok(())
    }
}
fn source(item: &Value) -> Value {
    json!({"reviewItemId":item["id"],"sessionId":item["sessionId"],"sectionId":if item["sectionId"].as_str()==Some(""){Value::Null}else{item["sectionId"].clone()},"sectionTitle":item["sectionTitle"],"courseId":item["courseId"],"courseTitle":item["courseTitle"]})
}
fn html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\n', "<br>")
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CoreError::io(e)),
    }
}
pub fn for_engine(e: &Engine) -> Result<Anki> {
    Anki::new(&e.lock()?.state_dir)
}
pub fn selected(e: &Engine, ids: &[String]) -> Result<Vec<Value>> {
    if ids.is_empty()
        || ids.len() > 100
        || ids.iter().any(|id| !valid_id(id))
        || ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len()
    {
        return Err(err("INVALID_INPUT", "请选择1至100张真实本机卡。"));
    }
    let mut c = e.lock()?;
    c.refresh()?;
    let all = review::all(&mut c.content)?;
    ids.iter()
        .map(|id| {
            all.iter()
                .find(|v| v["id"].as_str() == Some(id))
                .cloned()
                .ok_or_else(|| err("NOT_FOUND", "所选卡片已变化，请刷新后重试。"))
        })
        .collect()
}
pub async fn add_selected(
    e: &Engine,
    ids: Vec<String>,
    deck: &str,
    request: &str,
) -> Result<Value> {
    let a = for_engine(e)?;
    let digest = hash(&json!(["add", ids, deck]).to_string());
    if let Some(old) = a.old(request, &digest)? {
        return Ok(old);
    }
    let items = match selected(e, &ids) {
        Ok(items) => items,
        Err(error) => {
            let _g = WRITE_LOCK.lock().await;
            return a.failed(request, &digest, json!({"noteIds":[]}), error);
        }
    };
    a.add(&items, deck, request).await
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    fn server(replies: Vec<Value>) -> (String, std::thread::JoinHandle<Vec<Value>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let mut calls = vec![];
            for reply in replies {
                let (mut s, _) = l.accept().unwrap();
                let mut raw = vec![];
                let end;
                loop {
                    let mut b = [0; 1024];
                    let n = s.read(&mut b).unwrap();
                    raw.extend_from_slice(&b[..n]);
                    if let Some(p) = raw.windows(4).position(|p| p == b"\r\n\r\n") {
                        end = p + 4;
                        break;
                    }
                }
                let header = String::from_utf8_lossy(&raw[..end]);
                let n = header
                    .lines()
                    .find_map(|l| {
                        l.to_lowercase()
                            .strip_prefix("content-length:")
                            .map(|s| s.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                while raw.len() < end + n {
                    let mut b = [0; 1024];
                    let k = s.read(&mut b).unwrap();
                    raw.extend_from_slice(&b[..k]);
                }
                calls.push(serde_json::from_slice(&raw[end..end + n]).unwrap());
                let body = json!({"result":reply,"error":null}).to_string();
                write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
            calls
        });
        (url, h)
    }
    fn card() -> Value {
        json!({"cardId":1,"note":2,"deckName":"测试","question":"front","answer":"back","mod":1,"reps":0,"lapses":0,"queue":2,"due":1,"nextReviews":["1m","6m","1d","4d"]})
    }
    #[tokio::test]
    async fn real_http_due_hides_back_and_answer_is_once() {
        let c = card();
        let (url, h) = server(vec![
            json!([1]),
            json!([c.clone()]),
            json!([c.clone()]),
            json!([c]),
            json!([true]),
        ]);
        let d = tempfile::tempdir().unwrap();
        let a = Anki::at(d.path(), &url).unwrap();
        let due = a.due(None).await.unwrap();
        assert!(due["cards"][0].get("answer").is_none());
        let t = due["cards"][0]["reviewToken"].as_str().unwrap();
        assert_eq!(a.reveal(1, t).await.unwrap()["answer"], "back");
        let r = a.answer(1, t, 3, "anki-answer-0001").await.unwrap();
        assert_eq!(r["status"], "succeeded");
        assert_eq!(a.answer(1, t, 3, "anki-answer-0001").await.unwrap(), r);
        let calls = h.join().unwrap();
        assert_eq!(calls.len(), 5);
        assert_eq!(calls.last().unwrap()["action"], "answerCards");
    }
    #[tokio::test]
    async fn changed_card_refuses_stale_write() {
        let mut c = card();
        let t = Anki::token(&c);
        c["reps"] = json!(1);
        let (url, h) = server(vec![json!([c])]);
        let d = tempfile::tempdir().unwrap();
        let a = Anki::at(d.path(), &url).unwrap();
        assert_eq!(a.reveal(1, &t).await.unwrap_err().code, "ANKI_CARD_CHANGED");
        assert_eq!(h.join().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn offline_is_not_connected_and_unknown_is_not_replayed() {
        let d = tempfile::tempdir().unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        drop(l);
        let a = Anki::at(d.path(), &url).unwrap();
        assert_eq!(a.check().await["connected"], false);
        let digest = hash(&json!(["answer", 1, "token", 3]).to_string());
        let op = json!({"requestId":"unknown-op-1","status":"unknown","cardId":1,"digest":digest});
        a.save_op("unknown-op-1", &op).unwrap();
        assert_eq!(
            a.answer(1, "token", 3, "unknown-op-1").await.unwrap()["status"],
            "unknown"
        );
    }
    #[test]
    fn template_fields_escape_untrusted_html() {
        assert_eq!(
            html("<script>x</script>\n&"),
            "&lt;script&gt;x&lt;/script&gt;<br>&amp;"
        );
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[tokio::test]
    async fn definite_preflight_failure_has_durable_failed_receipt_without_network() {
        let d = tempfile::tempdir().unwrap();
        let a = Anki::at(d.path(), "http://127.0.0.1:1").unwrap();
        let op = a
            .answer(1, "../../not-allowed", 3, "anki-invalid-request")
            .await
            .unwrap();
        assert_eq!(op["status"], "failed");
        assert_eq!(a.operation("anki-invalid-request").unwrap(), op);
        assert_eq!(
            a.answer(1, "../../not-allowed", 3, "anki-invalid-request")
                .await
                .unwrap(),
            op
        );
    }
    #[tokio::test]
    async fn missing_local_card_is_failed_not_permanently_unknown() {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::new(d.path().join("lib"), d.path().join("state")).unwrap();
        let r = add_selected(
            &e,
            vec!["entry_missing".into()],
            "不存在的牌组",
            "anki-missing-local-card",
        )
        .await
        .unwrap();
        assert_eq!(r["status"], "failed");
        assert_eq!(
            for_engine(&e)
                .unwrap()
                .operation("anki-missing-local-card")
                .unwrap(),
            r
        );
    }
    #[test]
    fn unreadable_or_corrupt_receipt_is_not_not_found() {
        let d = tempfile::tempdir().unwrap();
        let a = Anki::at(d.path(), "http://127.0.0.1:1").unwrap();
        assert_eq!(
            a.operation("not-received-yet").unwrap_err().code,
            "NOT_FOUND"
        );
        let p = a.op_path("corrupt-op-1").unwrap();
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "{bad").unwrap();
        assert_eq!(
            a.operation("corrupt-op-1").unwrap_err().code,
            "INVALID_JSON"
        );
    }
}

#[cfg(test)]
mod receipt_integrity_tests {
    use super::*;
    #[test]
    fn valid_json_with_missing_or_invented_status_stays_unknown_to_client() {
        let d = tempfile::tempdir().unwrap();
        let a = Anki::at(d.path(), "http://127.0.0.1:1").unwrap();
        let p = a.op_path("invalid-status-1").unwrap();
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        for status in ["maybe-sent", "succeeded"] {
            fs::write(&p,json!({"requestId":"invalid-status-1","status":status,"digest":"a".repeat(64),"noteIds":[]}).to_string()).unwrap();
            assert_eq!(
                a.operation("invalid-status-1").unwrap_err().code,
                "INVALID_STATE"
            );
        }
    }
}
