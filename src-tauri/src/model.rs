use crate::types::{CoreError, Result};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
    time::{Instant, MissedTickBehavior},
};

const MAX_OUTPUT: usize = 1024 * 1024;
const MAX_LINE: usize = 6 * MAX_OUTPUT + 4096;
const MAX_CONFIG: u64 = 16 * 1024;
const MAX_TOOL_CALLS: usize = 8;
const MAX_TOOL_BYTES: usize = 32 * 1024;
const ALL_TOOLS: &[&str] = &[
    "record_question",
    "create_error",
    "query_related_knowledge",
    "create_review_card",
    "create_note",
];
const MODEL_ID: &str = "glm-5.3-flash";
const HARNESS: &str = "pi-agent-core";
const USAGE_GATE: &str = "PLAN_USAGE_UNCONFIRMED";
const KEYCHAIN_SERVICE: &str = "com.yushen.learning-station.pi-coding-plan-key";
const NODE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../runtime/node");
const DOMESTIC: &str = "https://open.bigmodel.cn/api/coding/paas/v4";
const INTERNATIONAL: &str = "https://api.z.ai/api/coding/paas/v4";
static SIDECAR_DIR: OnceLock<PathBuf> = OnceLock::new();

fn harness_name() -> String {
    HARNESS.into()
}
fn model_id() -> String {
    MODEL_ID.into()
}
fn usage_gate() -> String {
    USAGE_GATE.into()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProfileSettings {
    pub profile: String,
    pub configured: bool,
    pub base_url: String,
    pub model: String,
    pub verified: bool,
}
fn default_profiles() -> Vec<ModelProfileSettings> {
    [("bigmodel-coding", DOMESTIC), ("zai-coding", INTERNATIONAL)]
        .into_iter()
        .map(|(profile, url)| ModelProfileSettings {
            profile: profile.into(),
            configured: false,
            base_url: url.into(),
            model: MODEL_ID.into(),
            verified: false,
        })
        .collect()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSettings {
    pub configured: bool,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub verified: bool,
    #[serde(default = "harness_name")]
    pub harness: String,
    #[serde(default)]
    pub active_profile: String,
    #[serde(default = "default_profiles")]
    pub profiles: Vec<ModelProfileSettings>,
    #[serde(default = "usage_gate")]
    pub usage_gate: String,
}
impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            configured: false,
            provider: "glm-pi".into(),
            base_url: String::new(),
            model: model_id(),
            verified: false,
            harness: harness_name(),
            active_profile: String::new(),
            profiles: default_profiles(),
            usage_gate: usage_gate(),
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
    keychain_namespace: String,
    settings: ModelSettings,
}
// Never Serialize credentials or derive Debug: no secrets cross IPC or logs.
pub struct ModelCredentials {
    settings: ModelSettings,
    api_key: String,
}
impl fmt::Debug for ModelCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelCredentials")
            .field("settings", &self.settings)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}
pub type ToolHandler =
    Arc<dyn Fn(ToolCall) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>> + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolPolicy {
    pub mode: String,
    pub allowed_tools: Vec<String>,
    pub max_calls: usize,
}
impl ToolPolicy {
    fn legacy(enabled: bool) -> Self {
        Self {
            mode: "legacy".into(),
            allowed_tools: if enabled {
                ALL_TOOLS[..3].iter().map(|name| (*name).into()).collect()
            } else {
                vec![]
            },
            max_calls: if enabled { 8 } else { 0 },
        }
    }
}
fn validate_tool_policy(policy: &ToolPolicy, has_handler: bool) -> Result<()> {
    let (allowed, maximum): (&[&str], usize) = match policy.mode.as_str() {
        "question" => (
            &["query_related_knowledge", "record_question", "create_note"],
            4,
        ),
        "explain" => (ALL_TOOLS, 6),
        "practice" => (&["query_related_knowledge"], 2),
        "feedback" => (&[], 0),
        "legacy" => (&ALL_TOOLS[..3], 8),
        _ => return Err(sidecar_error("TOOL_PROTOCOL")),
    };
    let unique: std::collections::BTreeSet<_> = policy.allowed_tools.iter().collect();
    if policy.max_calls > maximum
        || policy.max_calls > MAX_TOOL_CALLS
        || unique.len() != policy.allowed_tools.len()
        || policy
            .allowed_tools
            .iter()
            .any(|name| !allowed.contains(&name.as_str()))
        || (!policy.allowed_tools.is_empty() && (policy.max_calls == 0 || !has_handler))
    {
        return Err(sidecar_error("TOOL_PROTOCOL"));
    }
    Ok(())
}
/// Optional Tauri resource-directory integration. This does not start Node or touch Keychain.
pub fn set_sidecar_dir(path: PathBuf) -> Result<()> {
    SIDECAR_DIR
        .set(path)
        .map_err(|_| CoreError::new("PI_RUNTIME", "Pi 组件路径已设置。"))
}
impl ModelStore {
    /// Startup reads only this app's non-secret configuration. No process, Keychain or network.
    pub fn new(state_dir: &Path) -> Result<Self> {
        let config_path = state_dir.join("model-settings.json");
        let settings = match std::fs::metadata(&config_path) {
            Ok(meta) => {
                if meta.len() > MAX_CONFIG {
                    return Err(invalid("模型配置文件过大，请重新配置。"));
                }
                let value: Value = serde_json::from_slice(&std::fs::read(&config_path)?)
                    .map_err(|_| invalid("模型配置文件无效，请重新配置。"))?;
                // Never migrate an old generic API configuration/key into a Coding Plan profile.
                if value.get("harness").and_then(Value::as_str) != Some(HARNESS) {
                    ModelSettings::default()
                } else {
                    let mut saved: ModelSettings = serde_json::from_value(value)
                        .map_err(|_| invalid("模型配置文件无效，请重新配置。"))?;
                    if saved.profiles.len() > 3 {
                        return Err(invalid("模型通道配置无效。"));
                    }
                    let mut ids = std::collections::BTreeSet::new();
                    for profile in &saved.profiles {
                        let endpoint =
                            validate_settings("glm-pi", &profile.base_url, &profile.model, false)?;
                        if profile_id(&endpoint)? != profile.profile
                            || !ids.insert(profile.profile.clone())
                        {
                            return Err(invalid("模型通道配置无效。"));
                        }
                    }
                    if saved.active_profile.is_empty() {
                        saved.configured = false;
                        saved.verified = false;
                        saved.base_url.clear();
                        saved.model = model_id();
                    } else {
                        let profile = saved
                            .profiles
                            .iter()
                            .find(|p| p.profile == saved.active_profile)
                            .ok_or_else(|| invalid("模型通道配置无效。"))?;
                        saved.base_url = profile.base_url.clone();
                        saved.model = profile.model.clone();
                        saved.configured = profile.configured;
                        // External usage remains blocked independently of any saved verification flag.
                        saved.verified = profile.verified && is_fixture(&profile.base_url);
                    }
                    saved.provider = "glm-pi".into();
                    saved.harness = harness_name();
                    saved.usage_gate = usage_gate();
                    saved
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ModelSettings::default(),
            Err(e) => return Err(e.into()),
        };
        let identity = std::fs::canonicalize(state_dir).unwrap_or_else(|_| state_dir.to_path_buf());
        Ok(Self {
            config_path,
            keychain_namespace: crate::types::hash(&identity.to_string_lossy()),
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
        let endpoint = validate_settings(provider.trim(), base_url.trim(), model.trim(), false)?;
        let profile = profile_id(&endpoint)?;
        if api_key.len() > 16384 || api_key.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(invalid("API 密钥格式无效，请重新输入。"));
        }
        if api_key.is_empty() {
            if !self.settings.configured {
                return Err(CoreError::new(
                    "AUTH",
                    "请先输入所选 Coding Plan 通道的 API 密钥。",
                ));
            }
            let saved_endpoint = validate_settings(
                &self.settings.provider,
                &self.settings.base_url,
                &self.settings.model,
                false,
            )?;
            if saved_endpoint != endpoint || self.settings.active_profile != profile {
                return Err(CoreError::new(
                    "AUTH",
                    "通道地址已更改，请输入该区域 Coding Plan 的 API 密钥后保存。",
                ));
            }
        }
        if !api_key.is_empty() {
            self.keychain_entry(&profile)?
                .set_password(api_key)
                .map_err(|_| keychain_error())?;
        }
        let changed = self.settings.base_url != endpoint || !api_key.is_empty();
        let mut next = self.settings();
        next.configured = true;
        next.provider = "glm-pi".into();
        next.base_url = endpoint.clone();
        next.model = model_id();
        next.harness = harness_name();
        next.active_profile = profile.clone();
        next.verified = next.verified && !changed && is_fixture(&endpoint);
        next.usage_gate = usage_gate();
        let item = ModelProfileSettings {
            profile: profile.clone(),
            configured: true,
            base_url: endpoint,
            model: model_id(),
            verified: next.verified,
        };
        if let Some(saved) = next.profiles.iter_mut().find(|p| p.profile == profile) {
            *saved = item;
        } else {
            next.profiles.push(item);
        }
        self.persist(&next)?;
        self.settings = next;
        Ok(self.settings())
    }
    pub fn credentials(&self) -> Result<ModelCredentials> {
        if !self.settings.configured {
            return Err(CoreError::new(
                "NOT_CONFIGURED",
                "请选择 Coding Plan 区域；当前套餐用途尚待确认。",
            ));
        }
        ensure_usage_allowed(&self.settings.base_url)?; // Deliberately before Keychain access.
        let profile = profile_id(&self.settings.base_url)?;
        if profile != self.settings.active_profile {
            return Err(invalid("当前通道配置不一致，请重新保存。"));
        }
        let api_key = self
            .keychain_entry(&profile)?
            .get_password()
            .map_err(|_| CoreError::new("AUTH", "无法读取本应用该区域的密钥，请检查模型设置。"))?;
        ModelCredentials::new(self.settings(), api_key)
    }
    /// Tests only the saved configuration, via the same real Pi Agent as generation.
    pub async fn test(&mut self) -> Result<ModelTest> {
        ensure_usage_allowed(&self.settings.base_url)?; // No key, process or persistence before the gate.
        let credentials = self.credentials()?;
        self.set_verified(false)?;
        run_sidecar(
            credentials,
            vec![],
            Arc::new(AtomicBool::new(false)),
            |_| {},
            "test",
            None,
            ToolPolicy::legacy(false),
        )
        .await?;
        self.set_verified(true)?;
        Ok(ModelTest {
            ok: true,
            message: "已保存配置经 Pi Agent 完成真实模型连接测试。".into(),
        })
    }
    fn set_verified(&mut self, value: bool) -> Result<()> {
        let mut next = self.settings();
        next.verified = value;
        if let Some(profile) = next
            .profiles
            .iter_mut()
            .find(|p| p.profile == next.active_profile)
        {
            profile.verified = value;
        }
        self.persist(&next)?;
        self.settings = next;
        Ok(())
    }
    fn keychain_account(&self, profile: &str) -> String {
        format!("{}:{profile}", self.keychain_namespace)
    }
    fn keychain_entry(&self, profile: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYCHAIN_SERVICE, &self.keychain_account(profile))
            .map_err(|_| keychain_error())
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
        if api_key.is_empty()
            || api_key.len() > 16384
            || api_key.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(CoreError::new("AUTH", "API 密钥格式无效。"));
        }
        Ok(Self { settings, api_key })
    }
}
/// No state lock is held while the separate Pi Agent process streams or settles.
pub async fn generate(
    credentials: ModelCredentials,
    messages: Vec<Value>,
    cancel: Arc<AtomicBool>,
    on_delta: impl FnMut(&str) + Send,
) -> Result<String> {
    run_sidecar(
        credentials,
        messages,
        cancel,
        on_delta,
        "generate",
        None,
        ToolPolicy::legacy(false),
    )
    .await
}
/// Registers only the versioned learning plugin; the handler owns local scope and persistence.
pub async fn generate_with_tools(
    credentials: ModelCredentials,
    messages: Vec<Value>,
    cancel: Arc<AtomicBool>,
    on_delta: impl FnMut(&str) + Send,
    tool_handler: ToolHandler,
) -> Result<String> {
    run_sidecar(
        credentials,
        messages,
        cancel,
        on_delta,
        "generate",
        Some(tool_handler),
        ToolPolicy::legacy(true),
    )
    .await
}
/// The caller supplies a fixed mode policy; Rust and Pi independently enforce its subset.
pub async fn generate_scoped(
    credentials: ModelCredentials,
    messages: Vec<Value>,
    cancel: Arc<AtomicBool>,
    on_delta: impl FnMut(&str) + Send,
    tool_handler: Option<ToolHandler>,
    policy: ToolPolicy,
) -> Result<String> {
    run_sidecar(
        credentials,
        messages,
        cancel,
        on_delta,
        "generate",
        tool_handler,
        policy,
    )
    .await
}
async fn run_sidecar(
    mut credentials: ModelCredentials,
    messages: Vec<Value>,
    cancel: Arc<AtomicBool>,
    mut on_delta: impl FnMut(&str) + Send,
    operation: &str,
    tool_handler: Option<ToolHandler>,
    policy: ToolPolicy,
) -> Result<String> {
    ensure_usage_allowed(&credentials.settings.base_url)?;
    validate_tool_policy(&policy, tool_handler.is_some())?;
    if cancel.load(Ordering::Acquire) {
        return Err(cancelled());
    }
    if messages.len() > 128 {
        return Err(sidecar_error("INVALID_INPUT"));
    }
    let directory = SIDECAR_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../pi-sidecar"));
    let directory = std::fs::canonicalize(directory).map_err(|_| runtime_error())?;
    if !Path::new(NODE_PATH).is_file()
        || !directory.join("sidecar.mjs").is_file()
        || !directory.join("harness.mjs").is_file()
        || !directory.join("learning-plugin.mjs").is_file()
        || !directory
            .join("node_modules/@earendil-works/pi-agent-core/package.json")
            .is_file()
    {
        return Err(runtime_error());
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Input<'a> {
        protocol: u8,
        operation: &'a str,
        settings: &'a ModelSettings,
        api_key: &'a str,
        messages: &'a [Value],
        learning_tools: bool,
        tool_policy: &'a ToolPolicy,
    }
    let mut input = serde_json::to_vec(&Input {
        protocol: 1,
        operation,
        settings: &credentials.settings,
        api_key: &credentials.api_key,
        messages: &messages,
        learning_tools: tool_handler.is_some()
            && operation == "generate"
            && !policy.allowed_tools.is_empty(),
        tool_policy: &policy,
    })
    .map_err(|_| sidecar_error("INVALID_INPUT"))?;
    if input.len() > 4 * MAX_OUTPUT - 1 {
        input.fill(0);
        return Err(sidecar_error("INVALID_INPUT"));
    }
    input.push(b'\n');
    credentials.api_key.clear();
    let mut child = Command::new(NODE_PATH)
        .arg("--permission")
        .arg(format!("--allow-fs-read={}", directory.display()))
        .arg(directory.join("sidecar.mjs"))
        .current_dir(&directory)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "en_US.UTF-8")
        .env("TZ", "UTC")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| runtime_error())?;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(if operation == "test" { 30 } else { 120 });
    let first_deadline = started + Duration::from_secs(30);
    let mut stdin = child.stdin.take().ok_or_else(runtime_error)?;
    let written = await_stage(
        async { stdin.write_all(&input).await.map_err(|_| runtime_error()) },
        &cancel,
        first_deadline,
    )
    .await;
    input.fill(0);
    drop(input);
    if let Err(error) = written {
        stop_child(&mut child, &mut stdin).await;
        return Err(error);
    }
    let result = async {
        let mut stdout = child.stdout.take().ok_or_else(runtime_error)?;
        let mut block = [0u8; 8192];
        let mut line = Vec::new();
        let mut output = String::new();
        let mut ready = false;
        let mut settled = false;
        let mut final_content = None;
        let mut had_response = false;
        let mut tool_attempts = 0usize;
        let mut tool_cache: std::collections::BTreeMap<String, (String, Value)> =
            std::collections::BTreeMap::new();
        loop {
            let stage_deadline = if !had_response {
                first_deadline.min(deadline)
            } else {
                deadline
            };
            let count = await_stage(
                async { stdout.read(&mut block).await.map_err(|_| runtime_error()) },
                &cancel,
                stage_deadline,
            )
            .await?;
            if count == 0 {
                break;
            }
            for byte in &block[..count] {
                if *byte != b'\n' {
                    if line.len() >= MAX_LINE {
                        return Err(sidecar_error("MODEL_RESPONSE"));
                    }
                    line.push(*byte);
                    continue;
                }
                let event: Value =
                    serde_json::from_slice(&line).map_err(|_| sidecar_error("MODEL_RESPONSE"))?;
                line.clear();
                match event.get("type").and_then(Value::as_str) {
                    Some("ready") if !ready && !settled => {
                        if event.get("harness").and_then(Value::as_str) != Some(HARNESS)
                            || event.get("version").and_then(Value::as_str) != Some("1.3.0")
                        {
                            return Err(runtime_error());
                        }
                        if !policy.allowed_tools.is_empty() && operation == "generate" {
                            let expected = json!(policy.allowed_tools);
                            let registered = event
                                .get("plugins")
                                .and_then(Value::as_array)
                                .is_some_and(|plugins| {
                                    plugins.iter().any(|plugin| {
                                        plugin.get("id").and_then(Value::as_str)
                                            == Some("learning-station.learning")
                                            && plugin.get("version").and_then(Value::as_str)
                                                == Some("1.0.0")
                                            && plugin.get("tools") == Some(&expected)
                                    })
                                });
                            if !registered {
                                return Err(runtime_error());
                            }
                        } else if event
                            .get("plugins")
                            .and_then(Value::as_array)
                            .is_some_and(|plugins| !plugins.is_empty())
                        {
                            return Err(runtime_error());
                        }
                        ready = true;
                    }
                    Some("delta") if ready && !settled => {
                        let text = event
                            .get("delta")
                            .and_then(Value::as_str)
                            .ok_or_else(|| sidecar_error("MODEL_RESPONSE"))?;
                        if output.len() + text.len() > MAX_OUTPUT {
                            return Err(sidecar_error("OUTPUT_LIMIT"));
                        }
                        if !text.is_empty() {
                            had_response = true;
                        }
                        output.push_str(text);
                        on_delta(text);
                    }
                    Some("tool_request") if ready && !settled => {
                        let handler = tool_handler
                            .as_ref()
                            .ok_or_else(|| sidecar_error("TOOLS_DISABLED"))?;
                        had_response = true;
                        tool_attempts += 1;
                        if tool_attempts > policy.max_calls || tool_attempts > MAX_TOOL_CALLS {
                            return Err(sidecar_error("TOOL_LIMIT"));
                        }
                        let call = ToolCall {
                            id: event
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .into(),
                            name: event
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .into(),
                            args: event.get("args").cloned().unwrap_or(Value::Null),
                        };
                        if !policy.allowed_tools.contains(&call.name) {
                            return Err(sidecar_error("TOOLS_DISABLED"));
                        }
                        validate_tool_call(&call)?;
                        let identity = serde_json::to_string(&(&call.name, &call.args))
                            .map_err(|_| sidecar_error("TOOL_PROTOCOL"))?;
                        let reply = if let Some((saved_identity, reply)) = tool_cache.get(&call.id)
                        {
                            if saved_identity != &identity {
                                return Err(sidecar_error("TOOL_PROTOCOL"));
                            }
                            reply.clone()
                        } else {
                            let result =
                                await_stage(handler(call.clone()), &cancel, deadline).await;
                            let reply = match result {
                                Ok(value) if value.is_object() => {
                                    json!({"type":"tool_result","id":call.id,"result":value})
                                }
                                Ok(_) => tool_failure(&call.id, "TOOL_PROTOCOL"),
                                Err(error)
                                    if matches!(error.code.as_str(), "CANCELLED" | "TIMEOUT") =>
                                {
                                    return Err(sidecar_error(&error.code))
                                }
                                Err(error) => tool_failure(&call.id, &error.code),
                            };
                            if serde_json::to_vec(&reply)
                                .map_err(|_| sidecar_error("TOOL_PROTOCOL"))?
                                .len()
                                > MAX_TOOL_BYTES
                            {
                                return Err(sidecar_error("TOOL_LIMIT"));
                            }
                            tool_cache.insert(call.id.clone(), (identity, reply.clone()));
                            reply
                        };
                        let mut bytes = serde_json::to_vec(&reply)
                            .map_err(|_| sidecar_error("TOOL_PROTOCOL"))?;
                        bytes.push(b'\n');
                        await_stage(
                            async { stdin.write_all(&bytes).await.map_err(|_| runtime_error()) },
                            &cancel,
                            deadline,
                        )
                        .await?;
                    }
                    Some("done") if ready && !settled => {
                        if event.get("settled").and_then(Value::as_bool) != Some(true)
                            || event.get("stopReason").and_then(Value::as_str) != Some("stop")
                            || event.get("outputBytes").and_then(Value::as_u64)
                                != Some(output.len() as u64)
                            || output.trim().is_empty()
                        {
                            return Err(sidecar_error("MODEL_RESPONSE"));
                        }
                        let text = event
                            .get("finalContent")
                            .and_then(Value::as_str)
                            .ok_or_else(|| sidecar_error("MODEL_RESPONSE"))?;
                        if text.trim().is_empty() || text.len() > MAX_OUTPUT {
                            return Err(sidecar_error("MODEL_RESPONSE"));
                        }
                        final_content = Some(text.to_owned());
                        settled = true;
                    }
                    Some("error") => {
                        return Err(sidecar_error(
                            event
                                .get("code")
                                .and_then(Value::as_str)
                                .unwrap_or("PI_RUNTIME"),
                        ))
                    }
                    _ => return Err(sidecar_error("MODEL_RESPONSE")),
                }
            }
        }
        let status = await_stage(
            async { child.wait().await.map_err(|_| runtime_error()) },
            &cancel,
            deadline,
        )
        .await?;
        if !line.is_empty() || !settled || !status.success() {
            return Err(sidecar_error("MODEL_RESPONSE"));
        }
        final_content.ok_or_else(|| sidecar_error("MODEL_RESPONSE"))
    }
    .await;
    if result.is_err() {
        stop_child(&mut child, &mut stdin).await;
    }
    result
}
fn validate_tool_call(call: &ToolCall) -> Result<()> {
    if call.id.is_empty() || call.id.chars().count() > 256 || call.id.chars().any(char::is_control)
    {
        return Err(sidecar_error("TOOL_PROTOCOL"));
    }
    let (fields, required): (&[(&str, usize)], &[&str]) = match call.name.as_str() {
        "record_question" => (&[("question", 8000), ("selection", 12000)], &["question"]),
        "create_error" => (
            &[
                ("front", 8000),
                ("back", 12000),
                ("cause", 4000),
                ("exerciseId", 128),
                ("tags", 0),
            ],
            &["front", "back"],
        ),
        "query_related_knowledge" => (&[("query", 500), ("knowledgeId", 128)], &[]),
        "create_review_card" => (
            &[("front", 8000), ("back", 12000), ("tags", 0)],
            &["front", "back"],
        ),
        "create_note" => (&[("content", 12000), ("selection", 12000)], &["content"]),
        _ => return Err(sidecar_error("TOOLS_DISABLED")),
    };
    let args = call
        .args
        .as_object()
        .ok_or_else(|| sidecar_error("TOOL_PROTOCOL"))?;
    if serde_json::to_vec(&call.args)
        .map_err(|_| sidecar_error("TOOL_PROTOCOL"))?
        .len()
        > MAX_TOOL_BYTES
    {
        return Err(sidecar_error("TOOL_LIMIT"));
    }
    for (key, value) in args {
        let (_, limit) = fields
            .iter()
            .find(|(field, _)| *field == key.as_str())
            .ok_or_else(|| sidecar_error("TOOL_PROTOCOL"))?;
        if key == "tags" {
            let tags = value
                .as_array()
                .ok_or_else(|| sidecar_error("TOOL_PROTOCOL"))?;
            if tags.len() > 12
                || tags.iter().any(|tag| {
                    tag.as_str()
                        .is_none_or(|s| s.trim().is_empty() || s.chars().count() > 64)
                })
            {
                return Err(sidecar_error("TOOL_PROTOCOL"));
            }
        } else {
            let text = value
                .as_str()
                .ok_or_else(|| sidecar_error("TOOL_PROTOCOL"))?;
            if text.chars().count() > *limit
                || (!matches!(key.as_str(), "selection" | "cause") && text.trim().is_empty())
            {
                return Err(sidecar_error("TOOL_PROTOCOL"));
            }
            if matches!(key.as_str(), "exerciseId" | "knowledgeId") && !crate::types::valid_id(text)
            {
                return Err(sidecar_error("TOOL_PROTOCOL"));
            }
        }
    }
    if required.iter().any(|field| !args.contains_key(*field))
        || (call.name == "query_related_knowledge"
            && !args.contains_key("query")
            && !args.contains_key("knowledgeId"))
    {
        return Err(sidecar_error("TOOL_PROTOCOL"));
    }
    Ok(())
}
fn tool_failure(id: &str, code: &str) -> Value {
    let safe = if !code.is_empty()
        && code.len() <= 64
        && code.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
    {
        code
    } else {
        "TOOL_FAILED"
    };
    json!({"type":"tool_result","id":id,"error":{"code":safe,"message":"学习工具执行失败，未确认完成。"}})
}
async fn stop_child(child: &mut Child, stdin: &mut ChildStdin) {
    let _ = tokio::time::timeout(
        Duration::from_millis(100),
        stdin.write_all(b"{\"type\":\"cancel\"}\n"),
    )
    .await;
    if tokio::time::timeout(Duration::from_millis(300), child.wait())
        .await
        .is_err()
    {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
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
        tokio::select! { biased;
            _ = tokio::time::sleep_until(deadline) => return Err(sidecar_error("TIMEOUT")),
            _ = tick.tick() => { if cancel.load(Ordering::Acquire) { return Err(cancelled()); } },
            result = &mut future => return result,
        }
    }
}
fn validate_settings(
    provider: &str,
    base_url: &str,
    model: &str,
    allow_unconfigured: bool,
) -> Result<String> {
    if !matches!(provider, "glm-pi" | "openai-compatible") || model != MODEL_ID {
        return Err(invalid("本次仅支持 Pi Agent 与 glm-5.3-flash 模型。"));
    }
    if allow_unconfigured && base_url.is_empty() {
        return Ok(String::new());
    }
    if base_url.is_empty()
        || base_url.len() > 2048
        || base_url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(invalid("请选择有效的 Coding Plan 通道地址。"));
    }
    let url = Url::parse(base_url).map_err(|_| invalid("Coding Plan 地址无效。"))?;
    let userinfo = base_url
        .split_once("://")
        .map(|(_, rest)| {
            rest.split(['/', '?', '#'])
                .next()
                .unwrap_or_default()
                .contains('@')
        })
        .unwrap_or(false);
    if userinfo
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("通道地址不能包含账号、查询参数或片段。"));
    }
    let normalized = url.as_str().trim_end_matches('/').to_owned();
    if normalized != DOMESTIC && normalized != INTERNATIONAL && !is_fixture(&normalized) {
        return Err(invalid(
            "仅支持国内或国际 Coding Plan 专用通道；不支持普通按量付费 API。",
        ));
    }
    Ok(normalized)
}
fn is_fixture(endpoint: &str) -> bool {
    Url::parse(endpoint).is_ok_and(|url| {
        url.scheme() == "http"
            && matches!(
                url.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
            )
    })
}
fn profile_id(endpoint: &str) -> Result<String> {
    match endpoint {
        DOMESTIC => Ok("bigmodel-coding".into()),
        INTERNATIONAL => Ok("zai-coding".into()),
        _ if is_fixture(endpoint) => Ok("local-fixture".into()),
        _ => Err(invalid("Coding Plan 通道无效。")),
    }
}
fn ensure_usage_allowed(endpoint: &str) -> Result<()> {
    if is_fixture(endpoint) {
        Ok(())
    } else {
        Err(sidecar_error(USAGE_GATE))
    }
}
fn sidecar_error(code: &str) -> CoreError {
    let (safe_code, message) = match code {
        "PLAN_USAGE_UNCONFIRMED" => (
            USAGE_GATE,
            "Coding Plan 是否允许本学习应用用途尚未确认，暂不能连接或生成。",
        ),
        "AUTH" => ("AUTH", "API 认证失败，请检查所选区域的密钥和模型权限。"),
        "RATE_LIMIT" => ("RATE_LIMIT", "模型服务限流或额度不足，请检查额度后重试。"),
        "TIMEOUT" => ("TIMEOUT", "模型请求超时；已收到的内容已保留。"),
        "CANCELLED" => ("CANCELLED", "生成已停止，已收到的内容已保留。"),
        "NETWORK" => ("NETWORK", "模型连接失败或中断；已收到的内容已保留。"),
        "INVALID_INPUT" => (
            "INVALID_INPUT",
            "模型输入无效或过大，请缩小所选材料后重试。",
        ),
        "INVALID_URL" => (
            "INVALID_URL",
            "通道地址无效或返回重定向，请检查所选官方区域。",
        ),
        "INVALID_MODEL_SETTINGS" => (
            "INVALID_MODEL_SETTINGS",
            "请选择 Coding Plan 区域和 glm-5.3-flash 模型。",
        ),
        "MODEL_HTTP" => (
            "MODEL_HTTP",
            "模型服务拒绝请求，请检查所选区域、密钥和模型权限。",
        ),
        "OUTPUT_LIMIT" => (
            "OUTPUT_LIMIT",
            "模型输出超过限制，已停止；已收到的内容已保留。",
        ),
        "TOOL_LIMIT" => ("TOOL_LIMIT", "学习工具调用超过本次预算，已停止。"),
        "TOOL_PROTOCOL" => ("TOOL_PROTOCOL", "学习工具协议或参数无效，已停止。"),
        "TOOL_FAILED" => ("TOOL_FAILED", "学习工具执行失败，未确认完成。"),
        "TOOLS_DISABLED" => ("TOOLS_DISABLED", "模型请求了未启用的工具，已停止本次生成。"),
        "MODEL_RESPONSE" => (
            "MODEL_RESPONSE",
            "模型未完整返回有效文本；已收到的内容已保留。",
        ),
        _ => (
            "PI_RUNTIME",
            "Pi 运行环境不可用，请检查 Node 24 和学习站组件。",
        ),
    };
    let mut error = CoreError::new(safe_code, message);
    if safe_code == "RATE_LIMIT" {
        error.retryable = true;
    }
    error
}
fn runtime_error() -> CoreError {
    sidecar_error("PI_RUNTIME")
}
fn cancelled() -> CoreError {
    sidecar_error("CANCELLED")
}
fn invalid(message: &str) -> CoreError {
    CoreError::new("INVALID_MODEL_SETTINGS", message)
}
fn keychain_error() -> CoreError {
    CoreError::new(
        "KEYCHAIN",
        "无法保存本应用该区域的密钥，请检查本机钥匙串权限。",
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
    fn settings(endpoint: &str) -> ModelSettings {
        let mut value = ModelSettings::default();
        value.configured = true;
        value.base_url = endpoint.into();
        value.active_profile = profile_id(endpoint).unwrap();
        value
    }
    #[test]
    fn defaults_are_unconfigured_pi_glm_without_selecting_a_region() {
        let directory = tempfile::tempdir().unwrap();
        let store = ModelStore::new(directory.path()).unwrap();
        let settings = store.settings();
        assert!(!settings.configured);
        assert!(settings.base_url.is_empty());
        assert!(settings.active_profile.is_empty());
        assert_eq!(settings.model, MODEL_ID);
        assert_eq!(settings.harness, HARNESS);
        assert_eq!(settings.usage_gate, USAGE_GATE);
        assert_eq!(settings.profiles.len(), 2);
        assert!(!directory.path().join("model-settings.json").exists());
    }
    #[test]
    fn only_coding_endpoints_allowed_and_secret_input_is_not_echoed() {
        for url in [
            "https://api.z.ai/api/paas/v4",
            "https://open.bigmodel.cn/api/paas/v4",
            "https://example.com/v1",
            "http://api.z.ai/api/coding/paas/v4",
            "https://user:fixture-secret@api.z.ai/api/coding/paas/v4",
            "https://@api.z.ai/api/coding/paas/v4",
            "https://api.z.ai/api/coding/paas/v4?key=fixture-secret",
        ] {
            let error = validate_settings("glm-pi", url, MODEL_ID, false).unwrap_err();
            assert!(!error.to_string().contains("fixture-secret"));
        }
        assert_eq!(
            validate_settings(
                "openai-compatible",
                &format!("{DOMESTIC}/"),
                MODEL_ID,
                false
            )
            .unwrap(),
            DOMESTIC
        );
        assert!(validate_settings("glm-pi", INTERNATIONAL, "another-model", false).is_err());
    }
    #[test]
    fn changed_endpoint_empty_key_preserves_settings_without_keychain_access() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ModelStore::new(directory.path()).unwrap();
        store.settings = settings(DOMESTIC);
        let original = serde_json::to_value(store.settings()).unwrap();
        let error = store
            .configure("glm-pi", INTERNATIONAL, MODEL_ID, "")
            .unwrap_err();
        assert_eq!(error.code, "AUTH");
        assert_eq!(serde_json::to_value(store.settings()).unwrap(), original);
        assert!(!store.config_path.exists());
        store.settings = settings("http://localhost:1234/v1");
        assert_eq!(
            store
                .configure("glm-pi", "http://localhost:1234/other", MODEL_ID, "")
                .unwrap_err()
                .code,
            "AUTH"
        );
    }
    #[test]
    fn normalized_same_profile_can_retain_its_own_key_without_keychain_access() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ModelStore::new(directory.path()).unwrap();
        store.settings = settings(DOMESTIC);
        let result = store
            .configure(
                "openai-compatible",
                "https://OPEN.bigmodel.cn:443/api/coding/paas/v4/",
                MODEL_ID,
                "",
            )
            .unwrap();
        assert_eq!(result.provider, "glm-pi");
        assert_eq!(result.active_profile, "bigmodel-coding");
        assert!(!std::fs::read_to_string(&store.config_path)
            .unwrap()
            .contains("apiKey"));
    }
    #[test]
    fn regional_keychain_accounts_are_distinct_and_never_migrated() {
        let directory = tempfile::tempdir().unwrap();
        let store = ModelStore::new(directory.path()).unwrap();
        assert_ne!(
            store.keychain_account("bigmodel-coding"),
            store.keychain_account("zai-coding")
        );
        std::fs::write(&store.config_path, r#"{"configured":true,"provider":"openai-compatible","baseUrl":"https://api.openai.com/v1","model":"legacy","verified":true}"#).unwrap();
        let migrated = ModelStore::new(directory.path()).unwrap();
        assert!(!migrated.settings().configured);
        assert!(migrated.settings().base_url.is_empty());
    }
    #[tokio::test]
    async fn production_gate_blocks_test_credentials_and_generation_before_keychain_or_process() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ModelStore::new(directory.path()).unwrap();
        store.settings = settings(DOMESTIC);
        assert_eq!(store.credentials().unwrap_err().code, USAGE_GATE);
        assert_eq!(store.test().await.unwrap_err().code, USAGE_GATE);
        let credentials =
            ModelCredentials::new(settings(INTERNATIONAL), "fixture-secret".into()).unwrap();
        assert!(!format!("{credentials:?}").contains("fixture-secret"));
        assert_eq!(
            generate(
                credentials,
                vec![],
                Arc::new(AtomicBool::new(false)),
                |_| {}
            )
            .await
            .unwrap_err()
            .code,
            USAGE_GATE
        );
        assert!(!store.config_path.exists());
    }
    fn fixture(finish: bool) -> (ModelCredentials, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                let count = socket.read(&mut buf).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buf[..count]);
            }
            let end = request.windows(4).position(|v| v == b"\r\n\r\n").unwrap() + 4;
            let length = String::from_utf8_lossy(&request[..end])
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                })
                .unwrap();
            while request.len() < end + length {
                let count = socket.read(&mut buf).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buf[..count]);
            }
            let payload: Value = serde_json::from_slice(&request[end..end + length]).unwrap();
            assert_eq!(payload["model"], MODEL_ID);
            assert_eq!(payload["thinking"]["type"], "enabled");
            assert!(payload.get("tools").is_none());
            let body = format!(
                "data: {}\n\n{}",
                json!({"id":"fixture","choices":[{"index":0,"delta":{"content":"Pi fixture 文本"},"finish_reason":null}]}),
                if finish {
                    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"
                } else {
                    ""
                }
            );
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).unwrap();
        });
        (
            ModelCredentials::new(
                settings(&format!("http://{address}/v1")),
                "fixture-secret".into(),
            )
            .unwrap(),
            server,
        )
    }
    #[tokio::test]
    async fn rust_generation_uses_real_pi_agent_sidecar_for_local_fixture() {
        let (credentials, server) = fixture(true);
        let mut deltas = String::new();
        let output = generate(
            credentials,
            vec![json!({"role":"user","content":"fixture only"})],
            Arc::new(AtomicBool::new(false)),
            |delta| deltas.push_str(delta),
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(output, "Pi fixture 文本");
        assert_eq!(output, deltas);
    }
    #[tokio::test]
    async fn rust_sidecar_rejects_truncated_provider_stream() {
        let (credentials, server) = fixture(false);
        let error = generate(
            credentials,
            vec![json!({"role":"user","content":"fixture only"})],
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.code, "MODEL_RESPONSE");
    }
    #[tokio::test]
    async fn pi_tool_round_trip_persists_real_markdown_and_reopens_without_a_model() {
        use crate::{
            app::Engine,
            learning_tools::{self, ToolScope},
            sessions,
        };
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("library");
        let runtime = directory.path().join("runtime");
        let engine = Engine::new(library.clone(), runtime.clone()).unwrap();
        let scope = {
            let mut core = engine.lock().unwrap();
            let section = core
                .create_section(
                    "Pi工具fixture课程",
                    "Since Monday is a starting point.",
                    vec![],
                    vec![],
                )
                .unwrap();
            let (session, context) = core.start_session(&section.id, false).unwrap();
            ToolScope {
                session_id: session.id,
                section_id: section.id,
                run_id: "run_pi_tool_fixture".into(),
                allowed_ids: context.manifest.into_iter().map(|item| item.id).collect(),
            }
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server_library = library.clone();
        let server_session = scope.session_id.clone();
        let server_section = scope.section_id.clone();
        let server_version = engine
            .lock()
            .unwrap()
            .content
            .read(&scope.section_id)
            .unwrap()
            .version;
        let expected_final = json!({"exercises":[{"question":"Since marks what?","format":"exact","answers":["a starting point"],"explanation":"Since Monday is a starting point.","tags":["since"],"sourceIds":[scope.section_id]}],"summary":"fixture"}).to_string();
        let final_answer = expected_final.clone();
        let server = thread::spawn(move || {
            for turn in 0..2 {
                let deadline = std::time::Instant::now() + Duration::from_secs(8);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "Pi fixture did not issue the next turn"
                            );
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => panic!("fixture accept failed"),
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                    let count = socket.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                }
                let head = request.windows(4).position(|v| v == b"\r\n\r\n").unwrap() + 4;
                let length = String::from_utf8_lossy(&request[..head])
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                while request.len() < head + length {
                    let count = socket.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                }
                let payload: Value = serde_json::from_slice(&request[head..head + length]).unwrap();
                let names: Vec<_> = payload["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|tool| tool["function"]["name"].as_str().unwrap())
                    .collect();
                assert_eq!(names, ALL_TOOLS);
                assert_eq!(payload["thinking"]["type"], "enabled");
                let body = if turn == 0 {
                    let first = json!({"choices":[{"index":0,"delta":{"role":"assistant","content":"我先记录一下。"},"finish_reason":null}]});
                    let call = json!({"choices":[{"index":0,"delta":{"tool_calls":[
                        {"index":0,"id":"actual-question-1","type":"function","function":{"name":"record_question","arguments":serde_json::to_string(&json!({"question":"Why since rather than for? fixture-secret","selection":"since Monday"})).unwrap()}},
                        {"index":1,"id":"actual-error-1","type":"function","function":{"name":"create_error","arguments":serde_json::to_string(&json!({"front":"Since or for Monday?","back":"Since marks a starting point.","cause":"fixture suggestion","tags":["since"]})).unwrap()}},
                        {"index":2,"id":"actual-knowledge-1","type":"function","function":{"name":"query_related_knowledge","arguments":serde_json::to_string(&json!({"query":"Since"})).unwrap()}},
                        {"index":3,"id":"actual-card-1","type":"function","function":{"name":"create_review_card","arguments":serde_json::to_string(&json!({"front":"What does since mark?","back":"A starting point.","tags":["since"]})).unwrap()}},
                        {"index":4,"id":"actual-note-1","type":"function","function":{"name":"create_note","arguments":serde_json::to_string(&json!({"content":"Since gives a starting point; for gives a duration.","selection":"since Monday"})).unwrap()}}
                    ]},"finish_reason":null}]});
                    format!("data: {first}\n\ndata: {call}\n\ndata: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\ndata: [DONE]\n\n")
                } else {
                    let result_messages: Vec<_> = payload["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|message| message["role"] == "tool")
                        .collect();
                    assert_eq!(result_messages.len(), 5);
                    let receipts: Vec<Value> = result_messages
                        .iter()
                        .map(|message| {
                            serde_json::from_str(message["content"].as_str().unwrap()).unwrap()
                        })
                        .collect();
                    for result in &receipts {
                        assert_eq!(result["ok"], true);
                        assert_eq!(result["result"]["sessionId"], server_session);
                    }
                    for index in 0..2 {
                        let receipt = &receipts[index]["result"];
                        assert_eq!(receipt["persisted"], true);
                        let markdown = std::fs::read_to_string(
                            server_library.join(receipt["path"].as_str().unwrap()),
                        )
                        .unwrap();
                        assert!(markdown.contains("AI整理，待本人确认"));
                        assert!(!markdown.contains("fixture-secret"));
                        if index == 0 {
                            assert!(markdown.contains("Why since rather than for?"));
                            assert!(markdown.contains("[REDACTED]"));
                        } else {
                            assert!(markdown.contains("Since or for Monday?"));
                            assert!(markdown.contains("fixture suggestion"));
                        }
                    }
                    let query = &receipts[2]["result"];
                    assert_eq!(query["materialRole"], "user-reference");
                    assert_eq!(query["sectionId"], server_section);
                    assert_eq!(query["results"][0]["id"], server_section);
                    assert_eq!(query["results"][0]["version"], server_version);
                    assert!(query["results"][0]["excerpt"]
                        .as_str()
                        .unwrap()
                        .contains("Since Monday is a starting point."));
                    assert_eq!(query["manifest"][0]["id"], server_section);
                    assert_eq!(query["manifest"][0]["version"], server_version);
                    for index in [3, 4] {
                        let receipt = &receipts[index]["result"];
                        assert_eq!(receipt["persisted"], true);
                        let markdown = std::fs::read_to_string(
                            server_library.join(receipt["path"].as_str().unwrap()),
                        )
                        .unwrap();
                        assert!(markdown.contains("AI整理，待本人确认"));
                        if index == 3 {
                            assert!(markdown.contains("What does since mark?"));
                        } else {
                            assert!(markdown
                                .contains("Since gives a starting point; for gives a duration."));
                        }
                    }
                    let response = json!({"choices":[{"index":0,"delta":{"content":final_answer},"finish_reason":"stop"}]});
                    format!("data: {response}\n\ndata: [DONE]\n\n")
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).unwrap();
            }
        });
        let handler_engine = engine.clone();
        let handler_scope = scope.clone();
        let handler: ToolHandler = Arc::new(move |call| {
            let engine = handler_engine.clone();
            let scope = handler_scope.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    learning_tools::execute(&engine, &scope, &call.id, &call.name, call.args)
                })
                .await
                .map_err(|_| CoreError::new("IO", "本地fixture工具执行失败。"))?
            })
        });
        let credentials = ModelCredentials::new(
            settings(&format!("http://{address}/v1")),
            "fixture-secret".into(),
        )
        .unwrap();
        let mut partial = String::new();
        let output = generate_scoped(
            credentials,
            vec![json!({"role":"user","content":"请记录疑问、创建错题、查询关联知识、创建卡片和笔记，并返回练习JSON。"})],
            Arc::new(AtomicBool::new(false)),
            |delta| partial.push_str(delta),
            Some(handler.clone()),
            ToolPolicy { mode:"explain".into(), allowed_tools: ALL_TOOLS.iter().map(|name| (*name).into()).collect(), max_calls:6 },
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(output, expected_final);
        assert!(partial.starts_with("我先记录一下。"));
        assert!(serde_json::from_str::<Value>(&output)
            .unwrap()
            .get("exercises")
            .is_some());
        drop(handler);
        drop(engine);
        let reopened = Engine::new(library, runtime).unwrap();
        assert!(!reopened.state().unwrap().model.configured);
        let mut core = reopened.lock().unwrap();
        let session = sessions::get(&mut core.content, &scope.session_id).unwrap();
        assert_eq!(session.entries.len(), 4);
        let entry = session
            .entries
            .iter()
            .find(|entry| {
                entry
                    .data
                    .as_ref()
                    .is_some_and(|data| data["question"] == true)
            })
            .unwrap();
        assert!(entry.content.contains("Why since rather than for?"));
        let data = entry.data.as_ref().unwrap();
        assert_eq!(data["question"], true);
        assert_eq!(data["origin"], "pi-tool");
        assert_eq!(data["needsConfirmation"], true);
        let error = session
            .entries
            .iter()
            .find(|entry| entry.kind == "error")
            .unwrap();
        let error_data = error.data.as_ref().unwrap();
        assert_eq!(error_data["front"], "Since or for Monday?");
        assert_eq!(error_data["cause"], "");
        assert_eq!(error_data["suggestedCause"], "fixture suggestion");
        assert_eq!(error_data["causeConfirmed"], false);
        assert_eq!(error_data["needsConfirmation"], true);
        assert_eq!(error_data["origin"], "pi-tool");
        let card = session
            .entries
            .iter()
            .find(|entry| {
                entry
                    .data
                    .as_ref()
                    .is_some_and(|data| data["reviewCard"] == true)
            })
            .unwrap();
        assert_eq!(
            card.data.as_ref().unwrap()["front"],
            "What does since mark?"
        );
        assert_eq!(card.data.as_ref().unwrap()["needsConfirmation"], true);
        let note = session
            .entries
            .iter()
            .find(|entry| {
                entry
                    .content
                    .contains("Since gives a starting point; for gives a duration.")
            })
            .unwrap();
        assert_eq!(note.kind, "note");
        assert_ne!(
            note.data.as_ref().unwrap().get("reviewCard"),
            Some(&Value::Bool(true))
        );

        assert_eq!(
            core.content.read(&scope.section_id).unwrap().body,
            "Since Monday is a starting point."
        );
    }
    #[test]
    fn mode_tool_policy_rejects_cross_mode_tools_and_excess_budget() {
        for (mode, allowed, maximum) in [
            (
                "question",
                vec!["query_related_knowledge", "record_question", "create_note"],
                4,
            ),
            ("explain", ALL_TOOLS.to_vec(), 6),
            ("practice", vec!["query_related_knowledge"], 2),
            ("feedback", vec![], 0),
        ] {
            let mut policy = ToolPolicy {
                mode: mode.into(),
                allowed_tools: allowed.into_iter().map(str::to_owned).collect(),
                max_calls: maximum,
            };
            assert!(validate_tool_policy(&policy, true).is_ok());
            policy.max_calls += 1;
            assert!(validate_tool_policy(&policy, true).is_err());
        }
        assert!(validate_tool_policy(
            &ToolPolicy {
                mode: "practice".into(),
                allowed_tools: vec!["create_review_card".into()],
                max_calls: 2
            },
            true
        )
        .is_err());
    }
    #[test]
    fn tool_protocol_rejects_scope_fields_unknown_tools_and_large_arguments() {
        for (name, args) in [
            (
                "record_question",
                json!({"question":"Q","sessionId":"another"}),
            ),
            ("query_related_knowledge", json!({})),
            ("bash", json!({"command":"denied"})),
        ] {
            assert!(validate_tool_call(&ToolCall {
                id: "call-1".into(),
                name: name.into(),
                args
            })
            .is_err());
        }
        let large = ToolCall {
            id: "large".into(),
            name: "record_question".into(),
            args: json!({"question":"中".repeat(8000),"selection":"文".repeat(4000)}),
        };
        assert_eq!(validate_tool_call(&large).unwrap_err().code, "TOOL_LIMIT");
    }
    #[tokio::test]
    async fn tool_handler_is_not_invoked_when_production_usage_is_blocked() {
        let invoked = Arc::new(AtomicBool::new(false));
        let flag = invoked.clone();
        let handler: ToolHandler = Arc::new(move |_| {
            flag.store(true, Ordering::Release);
            Box::pin(async { Ok(json!({"persisted":true})) })
        });
        let credentials =
            ModelCredentials::new(settings(DOMESTIC), "fixture-secret".into()).unwrap();
        assert_eq!(
            generate_with_tools(
                credentials,
                vec![],
                Arc::new(AtomicBool::new(false)),
                |_| {},
                handler
            )
            .await
            .unwrap_err()
            .code,
            USAGE_GATE
        );
        assert!(!invoked.load(Ordering::Acquire));
    }
    #[tokio::test]
    async fn cancellation_and_timeout_are_typed_without_waiting_for_network() {
        let cancel = AtomicBool::new(true);
        assert_eq!(
            await_stage(
                std::future::pending::<Result<()>>(),
                &cancel,
                Instant::now() + Duration::from_secs(1)
            )
            .await
            .unwrap_err()
            .code,
            "CANCELLED"
        );
        assert_eq!(
            await_stage(
                std::future::pending::<Result<()>>(),
                &AtomicBool::new(false),
                Instant::now() + Duration::from_millis(10)
            )
            .await
            .unwrap_err()
            .code,
            "TIMEOUT"
        );
    }
    #[test]
    fn unknown_sidecar_error_is_sanitized() {
        let error = sidecar_error("fixture-secret raw provider body");
        assert_eq!(error.code, "PI_RUNTIME");
        assert!(!format!("{error:?}").contains("fixture-secret"));
    }
}
