use crate::types::{CoreError, Result};
use futures_util::StreamExt;
use reqwest::{header, Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fmt,
    future::Future,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::time::{Instant, MissedTickBehavior};

const MAX_OUTPUT: usize = 1024 * 1024;
const MAX_CONFIG: u64 = 16 * 1024;
const KEYCHAIN_SERVICE: &str = "com.yushen.learning-station.model-api-key";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSettings {
    pub configured: bool,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub verified: bool,
}
impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            configured: false,
            provider: "openai-compatible".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: String::new(),
            verified: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTest {
    pub ok: bool,
    pub message: String,
}

pub struct ModelStore {
    config_path: PathBuf,
    keychain_account: String,
    settings: ModelSettings,
}

// Deliberately neither Serialize nor derived Debug: credentials never cross IPC or logs.
pub struct ModelCredentials {
    settings: ModelSettings,
    api_key: String,
    client: Client,
}
impl fmt::Debug for ModelCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelCredentials")
            .field("settings", &self.settings)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

impl ModelStore {
    /// Startup reads only this application's non-secret settings; no Keychain/network access.
    pub fn new(state_dir: &Path) -> Result<Self> {
        let config_path = state_dir.join("model-settings.json");
        let settings = match std::fs::metadata(&config_path) {
            Ok(meta) => {
                if meta.len() > MAX_CONFIG {
                    return Err(invalid("模型配置文件过大，请重新配置。"));
                }
                let saved: ModelSettings = serde_json::from_slice(&std::fs::read(&config_path)?)
                    .map_err(|_| invalid("模型配置文件无效，请重新配置。"))?;
                validate_settings(
                    &saved.provider,
                    &saved.base_url,
                    &saved.model,
                    !saved.configured,
                )?;
                saved
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ModelSettings::default(),
            Err(e) => return Err(e.into()),
        };
        let identity = std::fs::canonicalize(state_dir).unwrap_or_else(|_| state_dir.to_path_buf());
        Ok(Self {
            config_path,
            keychain_account: crate::types::hash(&identity.to_string_lossy()),
            settings,
        })
    }

    pub fn settings(&self) -> ModelSettings {
        self.settings.clone()
    }

    pub fn configure(
        &mut self,
        provider: &str,
        base_url: &str,
        model: &str,
        api_key: &str,
    ) -> Result<ModelSettings> {
        let provider = provider.trim();
        let model = model.trim();
        let endpoint = validate_settings(provider, base_url.trim(), model, false)?;
        if api_key.len() > 16 * 1024 || api_key.chars().any(char::is_control) {
            return Err(invalid("API 密钥格式无效，请重新输入。"));
        }
        // Retain this app's credential only for the exact same normalized endpoint.
        // A different path can represent a different provider, even on the same host.
        if api_key.is_empty() {
            if !self.settings.configured {
                return Err(CoreError::new("AUTH", "请先输入 API 密钥。"));
            }
            let saved_endpoint = validate_settings(
                &self.settings.provider,
                &self.settings.base_url,
                &self.settings.model,
                false,
            )?;
            if saved_endpoint != endpoint {
                return Err(CoreError::new(
                    "AUTH",
                    "API 地址已更改，请输入该通道的 API 密钥后保存。",
                ));
            }
        }
        if !api_key.is_empty() {
            self.keychain_entry()?
                .set_password(api_key)
                .map_err(|_| keychain_error())?;
        }
        let changed = self.settings.provider != provider
            || self.settings.base_url != endpoint
            || self.settings.model != model
            || !api_key.is_empty();
        let next = ModelSettings {
            configured: true,
            provider: provider.into(),
            base_url: endpoint,
            model: model.into(),
            verified: self.settings.verified && !changed,
        };
        self.persist(&next)?;
        self.settings = next;
        Ok(self.settings())
    }

    pub fn credentials(&self) -> Result<ModelCredentials> {
        if !self.settings.configured {
            return Err(CoreError::new(
                "NOT_CONFIGURED",
                "请先配置模型和 API 密钥。",
            ));
        }
        let api_key = self.keychain_entry()?.get_password().map_err(|_| {
            CoreError::new(
                "AUTH",
                "无法读取本应用的 API 密钥，请在模型设置中重新输入。",
            )
        })?;
        if api_key.is_empty() {
            return Err(CoreError::new("AUTH", "API 密钥为空，请重新输入。"));
        }
        ModelCredentials::new(self.settings(), api_key)
    }

    /// Test only the saved configuration. UI edits must be saved with `configure` first.
    pub async fn test(&mut self) -> Result<ModelTest> {
        // Clear stale success even when Keychain access fails.
        self.settings.verified = false;
        self.persist(&self.settings)?;
        let credentials = self.credentials()?;
        let cancel = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_secs(30);
        let request = credentials.request(json!({"model": credentials.settings.model, "messages": [{"role":"user","content":"Reply with OK."}], "stream":false, "max_tokens":8}));
        let response = await_stage(
            async { request.send().await.map_err(network_error) },
            &cancel,
            deadline,
        )
        .await?;
        check_status(response.status())?;
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        loop {
            let next = await_stage(
                async { stream.next().await.transpose().map_err(network_error) },
                &cancel,
                deadline,
            )
            .await?;
            let Some(chunk) = next else { break };
            if bytes.len() + chunk.len() > MAX_OUTPUT {
                return Err(protocol("模型测试响应过大。"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| protocol("模型返回了无效的 JSON，请检查兼容接口地址。"))?;
        let nonempty = value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty());
        if !nonempty {
            return Err(protocol("接口未返回有效文本，请检查模型名称和接口兼容性。"));
        }
        let mut next = self.settings();
        next.verified = true;
        self.persist(&next)?;
        self.settings = next;
        Ok(ModelTest {
            ok: true,
            message: "已保存的模型配置连接成功，模型已返回真实响应。".into(),
        })
    }

    fn keychain_entry(&self) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYCHAIN_SERVICE, &self.keychain_account).map_err(|_| keychain_error())
    }
    fn persist(&self, settings: &ModelSettings) -> Result<()> {
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = self
            .config_path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let bytes =
            serde_json::to_vec_pretty(settings).map_err(|_| invalid("模型配置无法保存。"))?;
        let result = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temp, &self.config_path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result.map_err(CoreError::io)
    }
}

impl ModelCredentials {
    fn new(settings: ModelSettings, api_key: String) -> Result<Self> {
        validate_settings(
            &settings.provider,
            &settings.base_url,
            &settings.model,
            false,
        )?;
        // Never follow redirects with credentials, even to a different path on the same host.
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| CoreError::new("NETWORK", "无法初始化模型网络连接。"))?;
        Ok(Self {
            settings,
            api_key,
            client,
        })
    }
    fn request(&self, body: Value) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}/chat/completions", self.settings.base_url))
            .bearer_auth(&self.api_key)
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&body)
    }
}

/// Runs outside the main state lock. Deltas emitted before an error are retained by the caller.
pub async fn generate(
    mut credentials: ModelCredentials,
    messages: Vec<Value>,
    cancel: Arc<AtomicBool>,
    mut on_delta: impl FnMut(&str) + Send,
) -> Result<String> {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(120);
    let first_deadline = started + Duration::from_secs(30);
    let request = credentials
        .request(json!({"model": credentials.settings.model, "messages":messages, "stream":true}));
    // The builder owns a copy of the auth header. Release the original as soon as possible.
    credentials.api_key.clear();
    let response = await_stage(
        async { request.send().await.map_err(network_error) },
        &cancel,
        first_deadline,
    )
    .await?;
    check_status(response.status())?;
    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut output = String::new();
    loop {
        let stage_deadline = if output.is_empty() {
            first_deadline
        } else {
            deadline
        };
        let next = await_stage(
            async { stream.next().await.transpose().map_err(network_error) },
            &cancel,
            stage_deadline,
        )
        .await?;
        let Some(chunk) = next else {
            return Err(protocol(
                "模型连接已结束，但缺少完整结束标记；已收到的内容已保留，可重试。",
            ));
        };
        // Process each complete event in order, retaining earlier deltas if a later event fails.
        for byte in chunk {
            if cancel.load(Ordering::Acquire) {
                return Err(cancelled());
            }
            if let Some(event) = decoder.push(byte)? {
                match parse_event(&event)? {
                    StreamEvent::Delta { text, finished } => {
                        if output.len() + text.len() > MAX_OUTPUT {
                            return Err(protocol(
                                "模型输出超过 1 MiB，已停止；已收到的内容已保留。",
                            ));
                        }
                        if !text.is_empty() {
                            output.push_str(&text);
                            on_delta(&text);
                        }
                        if finished {
                            if cancel.load(Ordering::Acquire) {
                                return Err(cancelled());
                            }
                            return finish_output(output);
                        }
                    }
                    StreamEvent::Done => {
                        if cancel.load(Ordering::Acquire) {
                            return Err(cancelled());
                        }
                        return finish_output(output);
                    }
                }
            }
        }
    }
}

fn finish_output(output: String) -> Result<String> {
    if output.trim().is_empty() {
        Err(protocol("模型未返回任何文本，请检查模型设置后重试。"))
    } else {
        Ok(output)
    }
}

async fn await_stage<F, T>(future: F, cancel: &AtomicBool, deadline: Instant) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    tokio::pin!(future);
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => return Err(CoreError::new("TIMEOUT", "模型请求超时；已收到的内容已保留，可重试。")),
            _ = tick.tick() => { if cancel.load(Ordering::Acquire) { return Err(cancelled()); } },
            result = &mut future => return result,
        }
    }
}

fn validate_settings(
    provider: &str,
    base_url: &str,
    model: &str,
    allow_empty_model: bool,
) -> Result<String> {
    if provider != "openai-compatible" {
        return Err(invalid("暂只支持 OpenAI 兼容的 API 服务。"));
    }
    if base_url.is_empty() || base_url.len() > 2048 || base_url.chars().any(char::is_control) {
        return Err(invalid("API 地址无效或过长。"));
    }
    if model.len() > 256
        || (!allow_empty_model && model.is_empty())
        || model.chars().any(char::is_control)
    {
        return Err(invalid("请输入有效的模型名称（最多 256 字节）。"));
    }
    let url = Url::parse(base_url).map_err(|_| invalid("请输入完整的 HTTPS API 地址。"))?;
    let has_userinfo = base_url
        .split_once("://")
        .map(|(_, rest)| {
            rest.split(['/', '?', '#'])
                .next()
                .unwrap_or_default()
                .contains('@')
        })
        .unwrap_or(false);
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || url.host_str().is_none()
        || has_userinfo
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "API 地址必须使用 HTTPS（本机 localhost 可用 HTTP），且不能包含账号、查询参数或片段。",
        ));
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

#[derive(Default)]
struct SseDecoder {
    line: Vec<u8>,
    data: Vec<u8>,
    saw_data: bool,
    after_cr: bool,
}
impl SseDecoder {
    fn push(&mut self, byte: u8) -> Result<Option<Vec<u8>>> {
        if self.after_cr {
            self.after_cr = false;
            if byte == b'\n' {
                return Ok(None);
            }
        }
        if byte == b'\r' || byte == b'\n' {
            self.after_cr = byte == b'\r';
            return self.finish_line();
        }
        if self.line.len() + self.data.len() >= MAX_OUTPUT {
            return Err(protocol("模型流事件过大，已停止接收。"));
        }
        self.line.push(byte);
        Ok(None)
    }
    fn finish_line(&mut self) -> Result<Option<Vec<u8>>> {
        let line = std::mem::take(&mut self.line);
        if line.is_empty() {
            if !self.saw_data {
                return Ok(None);
            }
            self.saw_data = false;
            self.data.pop(); // SSE joins multiple data fields with LF, removing the last LF.
            return Ok(Some(std::mem::take(&mut self.data)));
        }
        if line == b"data" || line.starts_with(b"data:") {
            let content = if line.len() == 4 {
                &[][..]
            } else {
                let rest = &line[5..];
                rest.strip_prefix(b" ").unwrap_or(rest)
            };
            if self.data.len() + content.len() + 1 > MAX_OUTPUT {
                return Err(protocol("模型流事件过大，已停止接收。"));
            }
            self.data.extend_from_slice(content);
            self.data.push(b'\n');
            self.saw_data = true;
        }
        Ok(None)
    }
}

enum StreamEvent {
    Delta { text: String, finished: bool },
    Done,
}
fn parse_event(data: &[u8]) -> Result<StreamEvent> {
    if data == b"[DONE]" {
        return Ok(StreamEvent::Done);
    }
    let value: Value = serde_json::from_slice(data)
        .map_err(|_| protocol("模型返回了无效的流事件，请检查 API 兼容性。"))?;
    if value.get("error").is_some() {
        return Err(protocol("模型服务在生成中返回错误；已收到的内容已保留。"));
    }
    let choice = value.pointer("/choices/0");
    let text = choice
        .and_then(|v| v.pointer("/delta/content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let finished = match choice.and_then(|v| v.get("finish_reason")) {
        None | Some(Value::Null) => false,
        Some(Value::String(reason)) if !reason.is_empty() => true,
        _ => return Err(protocol("模型返回了无效的结束标记；已收到的内容已保留。")),
    };
    Ok(StreamEvent::Delta { text, finished })
}
fn check_status(status: reqwest::StatusCode) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    let (code, message) = match status.as_u16() {
        401 | 403 => ("AUTH", "API 认证失败，请检查密钥和模型访问权限。"),
        429 => (
            "RATE_LIMIT",
            "模型服务限流或额度不足，请检查额度后稍后重试。",
        ),
        300..=399 => (
            "INVALID_URL",
            "API 地址返回重定向。为保护密钥，请直接填写最终 API 地址。",
        ),
        408 | 504 => ("TIMEOUT", "模型服务响应超时，请稍后重试。"),
        500..=599 => ("NETWORK", "模型服务暂时不可用，请稍后重试。"),
        _ => (
            "MODEL_HTTP",
            "模型服务拒绝请求，请检查 API 地址和模型名称。",
        ),
    };
    let mut error = CoreError::new(code, message);
    if code == "RATE_LIMIT" {
        error.retryable = true;
    }
    Err(error)
}
fn network_error(error: reqwest::Error) -> CoreError {
    if error.is_timeout() {
        CoreError::new("TIMEOUT", "模型请求超时；已收到的内容已保留，可重试。")
    } else {
        CoreError::new(
            "NETWORK",
            "模型网络连接失败或中断；已收到的内容已保留，请检查网络后重试。",
        )
    }
}
fn invalid(message: &str) -> CoreError {
    CoreError::new("INVALID_MODEL_SETTINGS", message)
}
fn protocol(message: &str) -> CoreError {
    CoreError::new("MODEL_RESPONSE", message)
}
fn cancelled() -> CoreError {
    CoreError::new("CANCELLED", "生成已停止，已收到的内容已保留。")
}
fn keychain_error() -> CoreError {
    CoreError::new(
        "KEYCHAIN",
        "无法保存本应用的 API 密钥，请检查本机钥匙串权限后重试。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn settings(base_url: &str) -> ModelSettings {
        ModelSettings {
            configured: true,
            provider: "openai-compatible".into(),
            base_url: base_url.into(),
            model: "fixture-model".into(),
            verified: false,
        }
    }
    fn fixture(status: &str, body: &str) -> (ModelCredentials, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let handle = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                let count = socket.read(&mut buf).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..count]);
            }
            // Read the complete request before closing, avoiding a TCP reset from an unread body.
            let header_end = request.windows(4).position(|v| v == b"\r\n\r\n").unwrap() + 4;
            let length = String::from_utf8_lossy(&request[..header_end])
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            while request.len() < header_end + length {
                let count = socket.read(&mut buf).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..count]);
            }
            // Fixture credential, never a user credential or an actual Keychain access.
            assert!(String::from_utf8_lossy(&request).contains("Bearer fixture-secret"));
            socket.write_all(response.as_bytes()).unwrap();
        });
        (
            ModelCredentials::new(
                settings(&format!("http://{address}/v1")),
                "fixture-secret".into(),
            )
            .unwrap(),
            handle,
        )
    }
    #[test]
    fn settings_startup_has_no_secret_or_keychain_access() {
        let directory = tempfile::tempdir().unwrap();
        let store = ModelStore::new(directory.path()).unwrap();
        assert!(!store.settings().configured);
        assert!(!directory.path().join("model-settings.json").exists());
        assert!(!serde_json::to_string(&store.settings())
            .unwrap()
            .contains("apiKey"));
    }
    #[test]
    fn changing_endpoint_requires_a_new_key_without_touching_keychain_or_settings() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ModelStore::new(directory.path()).unwrap();
        store.settings = settings("https://example.com/v1");
        store.settings.verified = true;
        let original = serde_json::to_value(store.settings()).unwrap();
        for endpoint in [
            "https://other.example/v1",
            "https://example.com/another-provider/v1",
        ] {
            let error = store
                .configure("openai-compatible", endpoint, "fixture-model", "")
                .unwrap_err();
            assert_eq!(error.code, "AUTH");
            assert_eq!(serde_json::to_value(store.settings()).unwrap(), original);
            assert!(!directory.path().join("model-settings.json").exists());
        }
    }
    #[test]
    fn equivalent_normalized_endpoint_can_retain_the_saved_key_without_keychain_access() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ModelStore::new(directory.path()).unwrap();
        store.settings = settings("https://example.com/v1");
        store.settings.verified = true;
        let updated = store
            .configure(
                "openai-compatible",
                "https://EXAMPLE.com:443/v1/",
                "fixture-model",
                "",
            )
            .unwrap();
        assert_eq!(updated.base_url, "https://example.com/v1");
        assert!(updated.configured);
        assert!(updated.verified);
        let saved = std::fs::read_to_string(directory.path().join("model-settings.json")).unwrap();
        assert!(!saved.contains("apiKey"));
    }
    #[test]
    fn reject_unsafe_urls_without_echoing_input() {
        for url in [
            "http://example.com/v1",
            "ftp://localhost/v1",
            "https://user:fixture-secret@example.com/v1",
            "https://@example.com/v1",
            "https://example.com/v1?key=fixture-secret",
            "https://example.com/#fixture-secret",
            "file:///tmp/key",
        ] {
            let error = validate_settings("openai-compatible", url, "model", false).unwrap_err();
            assert!(!error.to_string().contains("fixture-secret"));
        }
        assert!(validate_settings(
            "openai-compatible",
            "https://example.com/v1/",
            "model",
            false
        )
        .is_ok());
        assert!(
            validate_settings("openai-compatible", "http://[::1]:8080/v1", "model", false).is_ok()
        );
        assert!(validate_settings("unsupported", "https://example.com", "model", false).is_err());
    }
    #[test]
    fn sse_handles_every_byte_boundary_utf8_crlf_and_multiline() {
        let input = ": ping\r\ndata: {\r\ndata: \"choices\":[{\"delta\":{\"content\":\"你好🙂\"},\"finish_reason\":null}]}\r\n\r\ndata: [DONE]\r\n\r\n";
        let mut decoder = SseDecoder::default();
        let events: Vec<_> = input
            .as_bytes()
            .iter()
            .filter_map(|b| decoder.push(*b).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        match parse_event(&events[0]).unwrap() {
            StreamEvent::Delta { text, finished } => {
                assert_eq!(text, "你好🙂");
                assert!(!finished);
            }
            _ => panic!("missing delta"),
        }
        assert!(matches!(
            parse_event(&events[1]).unwrap(),
            StreamEvent::Done
        ));
    }
    #[test]
    fn parser_rejects_invalid_and_oversized_events() {
        assert!(parse_event(b"not json").is_err());
        let mut decoder = SseDecoder::default();
        for _ in 0..MAX_OUTPUT {
            decoder.push(b'x').unwrap();
        }
        assert!(decoder.push(b'x').is_err());
        assert!(finish_output("  ".into()).is_err());
    }
    #[tokio::test]
    async fn streams_real_fixture_content_and_requires_end_marker() {
        let (credentials, server) = fixture("200 OK", "data: {\"choices\":[{\"delta\":{\"content\":\"fixture 文本\"},\"finish_reason\":null}]}\n\ndata: [DONE]\n\n");
        assert!(!format!("{credentials:?}").contains("fixture-secret"));
        let mut deltas = String::new();
        let output = generate(
            credentials,
            vec![json!({"role":"user","content":"fixture"})],
            Arc::new(AtomicBool::new(false)),
            |delta| deltas.push_str(delta),
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(output, "fixture 文本");
        assert_eq!(output, deltas);
        let (credentials, server) = fixture(
            "200 OK",
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
        );
        let mut partial = String::new();
        let error = generate(
            credentials,
            vec![],
            Arc::new(AtomicBool::new(false)),
            |delta| partial.push_str(delta),
        )
        .await
        .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.code, "MODEL_RESPONSE");
        assert_eq!(partial, "partial");
    }
    #[tokio::test]
    async fn finish_reason_alone_is_a_valid_end_marker() {
        let (credentials, server) = fixture("200 OK", "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\n");
        assert_eq!(
            generate(
                credentials,
                vec![],
                Arc::new(AtomicBool::new(false)),
                |_| {}
            )
            .await
            .unwrap(),
            "done"
        );
        server.join().unwrap();
    }
    #[tokio::test]
    async fn errors_never_echo_server_body_or_fixture_key() {
        for (status, code) in [
            ("401 Unauthorized", "AUTH"),
            ("403 Forbidden", "AUTH"),
            ("429 Too Many Requests", "RATE_LIMIT"),
            ("302 Found", "INVALID_URL"),
        ] {
            let (credentials, server) = fixture(status, "fixture-secret server body");
            let error = generate(
                credentials,
                vec![],
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
            .await
            .unwrap_err();
            server.join().unwrap();
            assert_eq!(error.code, code);
            assert!(!format!("{error:?}").contains("fixture-secret"));
        }
    }
    #[tokio::test]
    async fn cancellation_interrupts_pending_work_and_timeout_is_typed() {
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            signal.store(true, Ordering::Release);
        });
        let started = Instant::now();
        let error = await_stage(
            std::future::pending::<Result<()>>(),
            &cancel,
            started + Duration::from_secs(10),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "CANCELLED");
        assert!(started.elapsed() < Duration::from_secs(1));
        let error = await_stage(
            std::future::pending::<Result<()>>(),
            &AtomicBool::new(false),
            Instant::now() + Duration::from_millis(10),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "TIMEOUT");
    }
}
