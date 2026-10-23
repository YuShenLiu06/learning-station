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
    "create_todo",
    "generate_exercises",
    "list_learning_skills",
    "load_learning_skill",
];
/// User-selectable tool authorization tiers, ordered by strength. The default
/// is "off": read/write/bash stay unregistered unless the user opts in.
pub const TOOL_AUTHORIZATION_TIERS: [&str; 4] = ["off", "read", "write", "bash"];
pub const MAX_OUTPUT_TOKENS: u32 = 131_072;
pub const MIN_INPUT_RESERVE: u32 = 1_024;
pub fn default_output_tokens(provider: &str) -> u32 {
    if provider == "custom" {
        4_096
    } else {
        32_768
    }
}
pub fn parse_limits(
    max_output: &Value,
    context_window: Option<&Value>,
) -> Result<(u32, Option<u32>)> {
    let integer = |v: &Value| {
        v.as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| {
                CoreError::new(
                    "INVALID_MODEL_LIMITS",
                    "token 上限须为整数，上下文容量未知时请留空。",
                )
            })
    };
    let max = integer(max_output)?;
    let context = context_window
        .filter(|v| !v.is_null())
        .map(integer)
        .transpose()?;
    validate_limits(max, context)?;
    Ok((max, context))
}
fn validate_limits(max_output: u32, context_window: Option<u32>) -> Result<()> {
    if !(1..=MAX_OUTPUT_TOKENS).contains(&max_output)
        || context_window.is_some_and(|window| {
            !(1_024..=2_000_000).contains(&window)
                || max_output > window.saturating_sub(MIN_INPUT_RESERVE)
        })
    {
        return Err(CoreError::new("INVALID_MODEL_LIMITS", "输出上限须为 1–131072 的整数；上下文容量可留空，填写时须为 1024–2000000，且至少为输入预留 1024 token。"));
    }
    Ok(())
}
const MODEL_ID: &str = "glm-5.3-flash";
const HARNESS: &str = "pi-agent-core";
const PROTOCOL: &str = "openai-completions";
const ANTHROPIC: &str = "anthropic-messages";
const USAGE_GATE: &str = "PLAN_USAGE_UNCONFIRMED";
const KEYCHAIN_SERVICE: &str = "com.yushen.learning-station.pi-api-key";
const NODE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../runtime/node");
static NODE_RESOURCE: OnceLock<PathBuf> = OnceLock::new();
pub fn node_path() -> PathBuf {
    NODE_RESOURCE
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(NODE_PATH))
}
pub fn set_node_path(path: PathBuf) -> Result<()> {
    if !path.is_absolute() || !path.is_file() {
        return Err(runtime_error());
    }
    NODE_RESOURCE.set(path).map_err(|_| runtime_error())
}
const DOMESTIC: &str = "https://open.bigmodel.cn/api/coding/paas/v4";
const INTERNATIONAL: &str = "https://api.z.ai/api/coding/paas/v4";
static SIDECAR_DIR: OnceLock<PathBuf> = OnceLock::new();

fn harness_name() -> String {
    HARNESS.into()
}
fn protocol_name() -> String {
    PROTOCOL.into()
}
fn coding_provider() -> String {
    "glm-pi".into()
}
fn usage_gate() -> String {
    USAGE_GATE.into()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProfileSettings {
    pub profile: String,
    #[serde(default = "coding_provider")]
    pub provider: String,
    #[serde(default = "protocol_name")]
    pub protocol: String,
    pub configured: bool,
    pub base_url: String,
    pub model: String,
    pub verified: bool,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub context_window: Option<u32>,
}
fn default_profiles() -> Vec<ModelProfileSettings> {
    [("bigmodel-coding", DOMESTIC), ("zai-coding", INTERNATIONAL)]
        .into_iter()
        .map(|(profile, url)| ModelProfileSettings {
            profile: profile.into(),
            provider: coding_provider(),
            protocol: protocol_name(),
            configured: false,
            base_url: url.into(),
            model: MODEL_ID.into(),
            verified: false,
            max_output_tokens: None,
            context_window: None,
        })
        .collect()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSettings {
    pub configured: bool,
    pub provider: String,
    #[serde(default = "protocol_name")]
    pub protocol: String,
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
    #[serde(default, skip_deserializing)]
    pub request_url: String,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub context_window: Option<u32>,
}
impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            configured: false,
            provider: "custom".into(),
            protocol: protocol_name(),
            base_url: String::new(),
            model: String::new(),
            verified: false,
            harness: harness_name(),
            active_profile: String::new(),
            profiles: default_profiles(),
            usage_gate: String::new(),
            request_url: String::new(),
            max_output_tokens: None,
            context_window: None,
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
    #[cfg(test)]
    fixture_credentials: Option<(ModelSettings, String)>,
}
// Never Serialize credentials or derive Debug: no secrets cross IPC or logs.
pub struct ModelCredentials {
    settings: ModelSettings,
    api_key: String,
    diagnostic_path: Option<PathBuf>,
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
    /// User-selected authorization tier carried from local preferences; the
    /// model side never supplies or raises it.
    #[serde(default = "default_authorization")]
    pub authorization: String,
}
fn default_authorization() -> String {
    "off".into()
}
/// Cumulative tools unlocked by each authorization tier.
pub fn authorization_tools(authorization: &str) -> Result<&'static [&'static str]> {
    Ok(match authorization {
        "off" => &[],
        "read" => &["read"],
        "write" => &["read", "write"],
        "bash" => &["read", "write", "bash"],
        _ => return Err(sidecar_error("TOOL_PROTOCOL")),
    })
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
            authorization: "off".into(),
        }
    }
}
// question/explain are compatibility labels for the same interactive task.
// Keep task construction and validation on one Rust allowlist.
fn task_tools(mode: &str) -> Result<(&'static [&'static str], usize)> {
    Ok(match mode {
        "question" | "explain" => (ALL_TOOLS, 6),
        "practice" => (
            &[
                "query_related_knowledge",
                "list_learning_skills",
                "load_learning_skill",
            ],
            2,
        ),
        "review" => (&["query_related_knowledge"], 2),
        "feedback" | "grading" => (&[], 0),
        "legacy" => (&ALL_TOOLS[..3], 8),
        _ => return Err(sidecar_error("TOOL_PROTOCOL")),
    })
}
pub(crate) fn task_tool_policy(mode: &str, authorization: &str) -> ToolPolicy {
    let (names, max_calls) = task_tools(mode).unwrap_or((&[], 0));
    let mut policy = ToolPolicy {
        mode: mode.into(),
        allowed_tools: names.iter().map(|name| (*name).into()).collect(),
        max_calls,
        authorization: authorization.into(),
    };
    if matches!(mode, "question" | "explain") {
        if let Ok(extra) = authorization_tools(authorization) {
            policy
                .allowed_tools
                .extend(extra.iter().map(|name| (*name).into()));
        }
    }
    policy
}
fn validate_tool_policy(policy: &ToolPolicy, has_handler: bool) -> Result<()> {
    let (allowed, maximum) = task_tools(&policy.mode)?;
    let authorized = authorization_tools(&policy.authorization)?;
    // Tier tools ride on top of the interactive exploration modes only.
    let tier_applies = matches!(policy.mode.as_str(), "question" | "explain");
    let unique: std::collections::BTreeSet<_> = policy.allowed_tools.iter().collect();
    if policy.max_calls > maximum
        || policy.max_calls > MAX_TOOL_CALLS
        || unique.len() != policy.allowed_tools.len()
        || policy.allowed_tools.iter().any(|name| {
            !allowed.contains(&name.as_str())
                && !(tier_applies && authorized.contains(&name.as_str()))
        })
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
                    if saved.profiles.len() > 4 {
                        return Err(invalid("模型通道配置无效。"));
                    }
                    let mut ids = std::collections::BTreeSet::new();
                    for profile in &saved.profiles {
                        validate_limits(
                            profile
                                .max_output_tokens
                                .unwrap_or_else(|| default_output_tokens(&profile.provider)),
                            profile.context_window,
                        )?;
                        let endpoint = validate_protocol_settings(
                            &profile.provider,
                            &profile.protocol,
                            &profile.base_url,
                            &profile.model,
                            false,
                        )?;
                        if settings_profile(&profile.provider, &endpoint)? != profile.profile
                            || !ids.insert(profile.profile.clone())
                        {
                            return Err(invalid("模型通道配置无效。"));
                        }
                    }
                    if saved.active_profile.is_empty() {
                        saved.configured = false;
                        saved.verified = false;
                        saved.base_url.clear();
                        saved.model.clear();
                        saved.provider = "custom".into();
                        saved.protocol = protocol_name();
                        saved.max_output_tokens = None;
                        saved.context_window = None;
                    } else {
                        let profile = saved
                            .profiles
                            .iter()
                            .find(|p| p.profile == saved.active_profile)
                            .ok_or_else(|| invalid("模型通道配置无效。"))?;
                        saved.provider = canonical_provider(&profile.provider)?.into();
                        saved.protocol = profile.protocol.clone();
                        saved.base_url = profile.base_url.clone();
                        saved.model = profile.model.clone();
                        saved.configured = profile.configured;
                        saved.max_output_tokens = profile.max_output_tokens;
                        saved.context_window = profile.context_window;
                        // External usage remains blocked independently of any saved verification flag.
                        saved.verified =
                            profile.verified && !usage_blocked(&saved.provider, &profile.base_url);
                    }
                    saved.harness = harness_name();
                    saved.usage_gate = if usage_blocked(&saved.provider, &saved.base_url) {
                        usage_gate()
                    } else {
                        String::new()
                    };
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
            #[cfg(test)]
            fixture_credentials: None,
        })
    }
    #[cfg(test)]
    pub(crate) fn use_local_fixture(&mut self, endpoint: String) {
        assert!(endpoint.starts_with("http://127.0.0.1:"));
        self.fixture_credentials = Some((ModelSettings {
            configured: true, provider: "custom".into(), protocol: PROTOCOL.into(),
            model: "offline-fixture".into(), base_url: endpoint, active_profile: "custom".into(),
            ..ModelSettings::default()
        }, "synthetic-local-key".into()));
    }
    pub fn settings(&self) -> ModelSettings {
        let mut settings = self.settings.clone();
        settings.request_url = request_url(&settings.protocol, &settings.base_url);
        settings.max_output_tokens = Some(
            settings
                .max_output_tokens
                .unwrap_or_else(|| default_output_tokens(&settings.provider)),
        );
        settings
    }
    /// Local non-secret preference only: no credential access or model request.
    pub fn set_limits(
        &mut self,
        max_output_tokens: u32,
        context_window: Option<u32>,
    ) -> Result<ModelSettings> {
        if !self.settings.configured {
            return Err(CoreError::new(
                "NOT_CONFIGURED",
                "请先保存供应商连接，再设置输出上限。",
            ));
        }
        validate_limits(max_output_tokens, context_window)?;
        let mut next = self.settings();
        let profile = next
            .profiles
            .iter_mut()
            .find(|p| p.profile == next.active_profile)
            .ok_or_else(|| invalid("模型通道配置无效。"))?;
        profile.max_output_tokens = Some(max_output_tokens);
        profile.context_window = context_window;
        next.max_output_tokens = Some(max_output_tokens);
        next.context_window = context_window;
        self.persist(&next)?;
        self.settings = next;
        Ok(self.settings())
    }
    // Limits may be reused only for the same endpoint/protocol/provider binding.
    fn limits_for_binding(
        &self,
        profile: &str,
        provider: &str,
        protocol: &str,
        endpoint: &str,
    ) -> (Option<u32>, Option<u32>) {
        self.settings
            .profiles
            .iter()
            .find(|p| {
                p.profile == profile
                    && p.provider == provider
                    && p.protocol == protocol
                    && p.base_url == endpoint
            })
            .map(|p| (p.max_output_tokens, p.context_window))
            .unwrap_or((None, None))
    }
    /// A local model-name change never reads or writes credentials or calls a server.
    pub fn set_model(&mut self, model: &str) -> Result<ModelSettings> {
        if !self.settings.configured {
            return Err(CoreError::new(
                "NOT_CONFIGURED",
                "请先在供应商设置保存地址、协议和密钥。",
            ));
        }
        let model = model.trim();
        validate_protocol_settings(
            &self.settings.provider,
            &self.settings.protocol,
            &self.settings.base_url,
            model,
            false,
        )?;
        if self.settings.model == model {
            return Ok(self.settings());
        }
        let mut next = self.settings();
        next.model = model.into();
        next.verified = false;
        if let Some(profile) = next
            .profiles
            .iter_mut()
            .find(|p| p.profile == next.active_profile)
        {
            profile.model = model.into();
            profile.verified = false;
        }
        self.persist(&next)?;
        self.settings = next;
        Ok(self.settings())
    }
    pub fn configure(
        &mut self,
        provider: &str,
        base_url: &str,
        model: &str,
        api_key: &str,
    ) -> Result<ModelSettings> {
        self.configure_protocol(provider, PROTOCOL, base_url, model, api_key)
    }
    /// Save only explicit user settings and an app-owned Keychain secret; never sends a request.
    pub fn configure_protocol(
        &mut self,
        provider: &str,
        protocol: &str,
        base_url: &str,
        model: &str,
        api_key: &str,
    ) -> Result<ModelSettings> {
        let provider = canonical_provider(provider.trim())?;
        let protocol = protocol.trim();
        let model = model.trim();
        let endpoint =
            validate_protocol_settings(provider, protocol, base_url.trim(), model, false)?;
        let profile = settings_profile(provider, &endpoint)?;
        if protocol == ANTHROPIC && api_key.contains("sk-ant-oat") {
            return Err(invalid(
                "Anthropic 自定义接入仅支持 API key，不使用 OAuth 凭据。",
            ));
        }
        if api_key.len() > 16384 || api_key.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(invalid("API 密钥格式无效，请重新输入。"));
        }
        if api_key.is_empty() {
            if !self.settings.configured {
                return Err(CoreError::new("AUTH", "请先输入此通道的 API 密钥。"));
            }
            let saved_endpoint = validate_protocol_settings(
                &self.settings.provider,
                &self.settings.protocol,
                &self.settings.base_url,
                &self.settings.model,
                false,
            )?;
            if saved_endpoint != endpoint
                || self.settings.protocol != protocol
                || canonical_provider(&self.settings.provider)? != provider
                || self.settings.active_profile != profile
            {
                return Err(CoreError::new(
                    "AUTH",
                    "通道地址或协议已更改，请输入新通道的 API 密钥后保存。",
                ));
            }
        }
        if !api_key.is_empty() {
            self.keychain_entry(&endpoint, protocol)?
                .set_password(api_key)
                .map_err(|_| keychain_error())?;
        }
        let changed = self.settings.base_url != endpoint
            || self.settings.model != model
            || self.settings.protocol != protocol
            || self.settings.provider != provider
            || !api_key.is_empty();
        let mut next = self.settings();
        next.configured = true;
        next.provider = provider.into();
        next.protocol = protocol.into();
        next.base_url = endpoint.clone();
        next.model = model.into();
        next.harness = harness_name();
        next.active_profile = profile.clone();
        next.verified = next.verified && !changed && !usage_blocked(provider, &endpoint);
        next.usage_gate = if usage_blocked(provider, &endpoint) {
            usage_gate()
        } else {
            String::new()
        };
        let (max_output_tokens, context_window) =
            self.limits_for_binding(&profile, provider, protocol, &endpoint);
        next.max_output_tokens = max_output_tokens;
        next.context_window = context_window;
        let item = ModelProfileSettings {
            profile: profile.clone(),
            provider: provider.into(),
            protocol: protocol.into(),
            configured: true,
            base_url: endpoint,
            model: model.into(),
            verified: next.verified,
            max_output_tokens,
            context_window,
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
        #[cfg(test)]
        if let Some((settings, key)) = &self.fixture_credentials {
            return ModelCredentials::new(settings.clone(), key.clone());
        }
        if !self.settings.configured {
            return Err(CoreError::new(
                "NOT_CONFIGURED",
                "请先保存模型通道、协议、地址、模型和 API 密钥。",
            ));
        }
        ensure_usage_allowed(&self.settings)?; // Deliberately before Keychain access.
        let profile = settings_profile(&self.settings.provider, &self.settings.base_url)?;
        if profile != self.settings.active_profile {
            return Err(invalid("当前通道配置不一致，请重新保存。"));
        }
        let api_key = self
            .keychain_entry(&self.settings.base_url, &self.settings.protocol)?
            .get_password()
            .map_err(|_| CoreError::new("AUTH", "无法读取本应用此通道的密钥，请检查模型设置。"))?;
        let mut credentials = ModelCredentials::new(self.settings(), api_key)?;
        credentials.diagnostic_path = Some(
            self.config_path
                .with_file_name("model-response-diagnostic.json"),
        );
        Ok(credentials)
    }
    /// Tests only the saved configuration, via the same real Pi Agent as generation.
    pub async fn test(&mut self) -> Result<ModelTest> {
        ensure_usage_allowed(&self.settings)?; // No key, process or persistence before the gate.
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
            vec![],
            None,
        )
        .await?;
        self.set_verified(true)?;
        Ok(ModelTest {
            ok: true,
            message: "已按保存的协议、地址和模型完成连接测试。".into(),
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
    fn keychain_account(&self, endpoint: &str, protocol: &str) -> String {
        let origin = Url::parse(endpoint)
            .map(|url| url.origin().ascii_serialization())
            .unwrap_or_default();
        format!(
            "{}:{}",
            self.keychain_namespace,
            crate::types::hash(&format!("{protocol}\n{origin}"))
        )
    }
    fn keychain_entry(&self, endpoint: &str, protocol: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYCHAIN_SERVICE, &self.keychain_account(endpoint, protocol))
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
    pub fn override_model(&mut self, model: &str) -> Result<()> {
        validate_protocol_settings(
            &self.settings.provider,
            &self.settings.protocol,
            &self.settings.base_url,
            model,
            false,
        )?;
        self.settings.model = model.into();
        Ok(())
    }
    fn new(mut settings: ModelSettings, api_key: String) -> Result<Self> {
        let max_output = settings
            .max_output_tokens
            .unwrap_or_else(|| default_output_tokens(&settings.provider));
        validate_limits(max_output, settings.context_window)?;
        settings.max_output_tokens = Some(max_output);
        validate_protocol_settings(
            &settings.provider,
            &settings.protocol,
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
        Ok(Self {
            settings,
            api_key,
            diagnostic_path: None,
        })
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
        vec![],
        None,
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
        vec![],
        None,
    )
    .await
}
pub type ToolRecoveryHandler = Arc<
    dyn Fn(String, ToolCall) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>> + Send + Sync,
>;
pub type OutputValidator = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;
pub type ContextObserver = Arc<dyn Fn(Value) -> Result<()> + Send + Sync>;
pub type CompactionObserver = Arc<dyn Fn() + Send + Sync>;
pub type MaterialChangesHandler = Arc<dyn Fn(Option<String>, Option<String>) -> Result<Value> + Send + Sync>;
#[derive(Clone)]
pub struct RequestContext {
    pub debug_identity: Option<Value>,
    pub debug_registrations: Option<Value>,
    pub material_changes: Option<MaterialChangesHandler>,
    pub time: crate::time_context::TimeContext,
    pub exploration: Option<crate::exploration::ExplorationContext>,
    pub native_session: Option<crate::native_session::NativeSession>,
    pub context_observer: Option<ContextObserver>,
    pub tool_recovery: Option<ToolRecoveryHandler>,
    pub output_validator: Option<OutputValidator>,
    pub compaction_observer: Option<CompactionObserver>,
}
impl RequestContext {
    fn capture() -> Self {
        Self {
            debug_identity: None,
            debug_registrations: None,
            material_changes: None,
            time: crate::time_context::TimeContext::capture(),
            exploration: None,
            native_session: None,
            context_observer: None,
            tool_recovery: None,
            output_validator: None,
            compaction_observer: None,
        }
    }
}
/// A completed, non-aborted compaction may have dropped earlier tool results
/// from the effective context; per-run guidance dedup must not keep pointing
/// at bodies the model can no longer see.
fn compaction_clears_dedup(event: &Value) -> bool {
    event.get("event").and_then(Value::as_str) == Some("compaction_end")
        && event.get("aborted").and_then(Value::as_bool) != Some(true)
}
/// The caller supplies a fixed mode policy; Rust and Pi independently enforce its subset.
pub async fn generate_scoped(
    credentials: ModelCredentials,
    messages: Vec<Value>,
    cancel: Arc<AtomicBool>,
    on_delta: impl FnMut(&str) + Send,
    tool_handler: Option<ToolHandler>,
    policy: ToolPolicy,
    skills: Vec<crate::learning_skills::SkillSnapshot>,
    request_context: Option<RequestContext>,
) -> Result<String> {
    run_sidecar(
        credentials,
        messages,
        cancel,
        on_delta,
        "generate",
        tool_handler,
        policy,
        skills,
        request_context,
    )
    .await
}
fn configured_agent_dir(sidecar_dir: &Path) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    if let Some(path) = std::env::var_os("PI_CODING_AGENT_DIR") {
        let path = PathBuf::from(path);
        if path == Path::new("~") {
            return home;
        }
        if let Ok(rest) = path.strip_prefix("~/") {
            return home.join(rest);
        }
        return if path.is_absolute() {
            path
        } else {
            sidecar_dir.join(path)
        };
    }
    home.join(".pi/agent")
}

/// Read-only SDK resource paths for native project discovery. Project root is recursive;
/// outside it, only known ancestor instruction filenames and user resource directories pass.
fn native_project_read_allowlist(
    project_root: &Path,
    agent_dir: &Path,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut paths = vec![project_root.to_path_buf()];
    let context_names = [
        "AGENTS.override.md",
        "AGENTS.md",
        "AGENTS.MD",
        "CLAUDE.md",
        "CLAUDE.MD",
    ];
    let mut dir = project_root.to_path_buf();
    loop {
        for name in context_names {
            paths.push(dir.join(name));
        }
        let parent = match dir.parent() {
            Some(parent) if parent != dir => parent,
            _ => break,
        };
        dir = parent.to_path_buf();
    }
    for name in [
        "SYSTEM.md",
        "APPEND_SYSTEM.md",
        "AGENTS.override.md",
        "AGENTS.md",
        "AGENTS.MD",
        "CLAUDE.md",
        "CLAUDE.MD",
    ] {
        paths.push(agent_dir.join(name));
    }
    for name in ["skills", "prompts", "themes", "extensions"] {
        paths.push(agent_dir.join(name));
    }
    if let Some(home) = home.filter(|home| home.is_absolute()) {
        paths.push(home.join(".agents/skills"));
    }
    paths.sort();
    paths.dedup();
    paths
}

#[cfg(test)]
mod native_project_scope_tests {
    use super::native_project_read_allowlist;
    use std::path::Path;

    #[test]
    fn project_scope_does_not_grant_sibling_course_or_user_auth_file() {
        let root = Path::new("/fixture/library/courses/course-a");
        let agent = Path::new("/fixture/home/.pi/agent");
        let home = Path::new("/fixture/home");
        let paths = native_project_read_allowlist(root, agent, Some(home));

        assert!(paths.contains(&root.to_path_buf()));
        assert!(paths.contains(&agent.join("skills")));
        assert!(paths.contains(&home.join(".agents/skills")));
        assert!(
            !paths.contains(&Path::new("/fixture/library/courses/course-b/notes.md").to_path_buf())
        );
        assert!(!paths.contains(&agent.join("auth.json")));
    }
}

async fn run_sidecar(
    mut credentials: ModelCredentials,
    messages: Vec<Value>,
    cancel: Arc<AtomicBool>,
    mut on_delta: impl FnMut(&str) + Send,
    operation: &str,
    tool_handler: Option<ToolHandler>,
    policy: ToolPolicy,
    skills: Vec<crate::learning_skills::SkillSnapshot>,
    request_context: Option<RequestContext>,
) -> Result<String> {
    let mut request_context = request_context.unwrap_or_else(RequestContext::capture);
    request_context.time.validate()?;
    ensure_usage_allowed(&credentials.settings)?;
    let debug_attempt = uuid::Uuid::new_v4().to_string();
    let mut debug_fallback=request_context.debug_identity.clone().unwrap_or_else(||json!({}));
    let debug_run = request_context.native_session.as_ref().map(|n|n.run_id.clone()).unwrap_or_else(||debug_attempt.clone());
    if debug_fallback["sessionId"].is_null(){debug_fallback["sessionId"]=json!(request_context.native_session.as_ref().map(|n|n.learning_session_id.clone()).unwrap_or_else(||format!("utility:{}",operation)));}
    if debug_fallback["runId"].is_null(){debug_fallback["runId"]=json!(debug_run);}
    debug_fallback["attempt"]=json!(debug_attempt); debug_fallback["operation"]=json!(operation);
    let mut debug_epoch = None;
    let mut debug_metadata = Value::Null;
    let diagnostic_path = credentials.diagnostic_path.take();
    validate_tool_policy(&policy, tool_handler.is_some())?;
    crate::learning_skills::validate(&policy.mode, &policy.allowed_tools, &skills)?;
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
    if let Some(native) = request_context.native_session.as_mut() {
        if native.project_root.is_some() {
            native.agent_dir.get_or_insert_with(|| configured_agent_dir(&directory));
        }
    }
    if !node_path().is_file()
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
        debug_protocol: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        debug_registrations: Option<&'a Value>,
        operation: &'a str,
        settings: &'a ModelSettings,
        api_key: &'a str,
        messages: &'a [Value],
        learning_tools: bool,
        tool_policy: &'a ToolPolicy,
        learning_skills: &'a [crate::learning_skills::SkillSnapshot],
        time_context: &'a crate::time_context::TimeContext,
        exploration_context: Option<&'a crate::exploration::ExplorationContext>,
        native_session: Option<&'a crate::native_session::NativeSession>,
        output_validation: bool,
        material_changes: bool,
    }
    let mut sidecar_input = Input {
        protocol: 1,
        debug_protocol: true,
        debug_registrations: request_context.debug_registrations.as_ref(),
        operation,
        settings: &credentials.settings,
        api_key: &credentials.api_key,
        messages: &messages,
        learning_tools: tool_handler.is_some()
            && operation == "generate"
            && !policy.allowed_tools.is_empty(),
        tool_policy: &policy,
        learning_skills: &skills,
        time_context: &request_context.time,
        exploration_context: request_context.exploration.as_ref(),
        native_session: request_context.native_session.as_ref(),
        output_validation: request_context.output_validator.is_some(),
        material_changes: request_context.material_changes.is_some(),
    };
    let mut input = serde_json::to_vec(&sidecar_input).map_err(|_| sidecar_error("INVALID_INPUT"))?;
    if input.len()>4*MAX_OUTPUT-1 && sidecar_input.debug_registrations.is_some() {
        input.fill(0);sidecar_input.debug_registrations=None;
        input=serde_json::to_vec(&sidecar_input).map_err(|_|sidecar_error("INVALID_INPUT"))?;
    }
    if input.len() > 4 * MAX_OUTPUT - 1 {
        input.fill(0);
        return Err(sidecar_error("INVALID_INPUT"));
    }
    input.push(b'\n');
    credentials.api_key.clear();
    let mut command = Command::new(node_path());
    command
        .arg("--permission")
        .arg(format!("--allow-fs-read={}", directory.display()));
    if let Some(native) = &request_context.native_session {
        command
            .arg(format!("--allow-fs-read={}", native.directory.display()))
            .arg(format!("--allow-fs-write={}", native.directory.display()));
        if let Some(project_root) = &native.project_root {
            let canonical_root =
                std::fs::canonicalize(project_root).map_err(|_| runtime_error())?;
            if !project_root.is_absolute()
                || &canonical_root != project_root
                || !std::fs::symlink_metadata(&canonical_root)
                    .map(|meta| meta.is_dir() && !meta.file_type().is_symlink())
                    .unwrap_or(false)
            {
                return Err(runtime_error());
            }
            let agent_dir = native.agent_dir.as_ref().ok_or_else(runtime_error)?;
            if !agent_dir.is_absolute() {
                return Err(runtime_error());
            }
            let home = std::env::var_os("HOME").map(PathBuf::from);
            for path in native_project_read_allowlist(&canonical_root, agent_dir, home.as_deref()) {
                command.arg(format!("--allow-fs-read={}", path.display()));
            }
        }
    }
    let mut child = command
        .arg(directory.join("sidecar.mjs"))
        .current_dir(&directory)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "en_US.UTF-8")
        .env(
            "TZ",
            request_context.time.time_zone.as_deref().unwrap_or("UTC"),
        )
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
        let mut material_change_requests = 0usize;
        let mut output_validations = 0usize;
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
                    Some("debug_probe") if ready && !settled => {
                        let (epoch,meta)=crate::request_debug::gate(&debug_fallback);
                        debug_epoch=epoch; debug_metadata=meta;
                        stdin.write_all(format!("{}\n",json!({"type":"tool_result","id":event["id"],"result":{"epoch":epoch}})).as_bytes()).await.map_err(|_|runtime_error())?;
                    }
                    Some("debug_request" | "debug_status") if ready && !settled => {
                        if let Some(epoch)=debug_epoch.filter(|epoch|event["epoch"].as_u64()==Some(*epoch)) {
                            if let Some(id)=event["id"].as_str().filter(|id|id.len()<=64) {
                                crate::request_debug::capture(epoch,&format!("{}:{}",debug_attempt,id),&debug_metadata,&event);
                            }
                        }
                    }
                    Some("ready") if !ready && !settled => {
                        if event
                            .get("timeContext")
                            .and_then(|v| v.get("referenceTime"))
                            .and_then(Value::as_str)
                            != Some(request_context.time.reference_time.as_str())
                            || event.get("timeContext").and_then(|v| v.get("timeZone"))
                                != Some(&json!(request_context.time.time_zone))
                        {
                            return Err(CoreError::new(
                                "TIME_CONTEXT_INVALID",
                                "模型请求时间回执不一致，未确认生成。",
                            ));
                        }
                        if !skills.is_empty()
                            && event.get("learningSkills")
                                != Some(&json!(skills
                                    .iter()
                                    .map(|skill| skill.version())
                                    .collect::<Vec<_>>()))
                        {
                            return Err(sidecar_error("SKILL_INVALID"));
                        }
                        if let Some(native)=&request_context.native_session {
                            if event["nativeSession"]["version"]!="1.3.0"||event["nativeSession"]["engine"]!="pi-coding-agent"||event["nativeSession"]["persistent"]!=true||event["nativeSession"]["learningSessionId"]!=native.learning_session_id {return Err(runtime_error());}
                        }
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
                                                == Some("1.2.0")
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
                    Some("material_changes") if ready && !settled => {
                        material_change_requests += 1;
                        if material_change_requests > 8 { return Err(sidecar_error("TOOL_LIMIT")); }
                        let handler=request_context.material_changes.as_ref().ok_or_else(||sidecar_error("TOOLS_DISABLED"))?;
                        let id=event["id"].as_str().filter(|s|crate::types::valid_id(s)).ok_or_else(||sidecar_error("TOOL_PROTOCOL"))?;
                        let previous=match &event["baseCommitOid"] {Value::Null=>None,Value::String(oid)=>Some(oid.clone()),_=>return Err(sidecar_error("TOOL_PROTOCOL"))};
                        let focus_previous=match &event["focusBaseCommitOid"] {Value::Null=>None,Value::String(oid)=>Some(oid.clone()),_=>return Err(sidecar_error("TOOL_PROTOCOL"))};
                        let result=handler(previous,focus_previous)?;
                        let mut bytes=serde_json::to_vec(&json!({"type":"tool_result","id":id,"result":result}))?;bytes.push(b'\n');
                        await_stage(async{stdin.write_all(&bytes).await.map_err(|_|runtime_error())},&cancel,deadline).await?;
                    }
                    Some("session_sync") if !settled => {
                        let native=request_context.native_session.as_ref().ok_or_else(||sidecar_error("TOOLS_DISABLED"))?;
                        let id=event["id"].as_str().filter(|s|crate::types::valid_id(s)).ok_or_else(||sidecar_error("TOOL_PROTOCOL"))?;
                        crate::native_session::sync_directory(&native.directory)?;
                        let mut bytes=serde_json::to_vec(&json!({"type":"tool_result","id":id,"result":{"synced":true}}))?;bytes.push(b'\n');
                        await_stage(async{stdin.write_all(&bytes).await.map_err(|_|runtime_error())},&cancel,deadline).await?;
                        had_response=true;
                    }
                    Some("tool_recover") if !settled => {
                        let native=request_context.native_session.as_ref().ok_or_else(||sidecar_error("TOOLS_DISABLED"))?;
                        let recovery=request_context.tool_recovery.as_ref().ok_or_else(||sidecar_error("NATIVE_TOOL_UNKNOWN"))?;
                        tool_attempts+=1;if tool_attempts>MAX_TOOL_CALLS{return Err(sidecar_error("TOOL_LIMIT"));}
                        let original=event["recoveryRunId"].as_str().ok_or_else(||sidecar_error("TOOL_PROTOCOL"))?;
                        if !crate::types::valid_id(original){return Err(sidecar_error("TOOL_PROTOCOL"));}
                        let call=ToolCall{id:event["id"].as_str().unwrap_or_default().into(),name:event["name"].as_str().unwrap_or_default().into(),args:event["args"].clone()};
                        validate_tool_call(&call)?;
                        let recovered=await_stage(recovery(original.into(),call.clone()),&cancel,deadline).await;
                        let reply = match recovered {
                            Ok(value) => json!({"type":"tool_result","id":call.id,"result":value}),
                            Err(error) if call.name == "read" && matches!(error.code.as_str(),
                                "TOOL_SCOPE" | "CONFLICT" | "TOOL_NOT_ALLOWED" | "TRASHED" | "NOT_FOUND") =>
                                json!({"type":"tool_result","id":call.id,"error":{"code":error.code,"message":error.message}}),
                            Err(error) => return Err(error),
                        };
                        let path=native.directory.join(format!("tool-{}.json",crate::types::hash(&serde_json::to_string(&json!([original,call.id]))?)));
                        if call.name != "read" || !path.exists() {
                            crate::native_session::atomic(&path,&json!({"version":1,"runId":original,"callId":call.id,"name":call.name,"argsHash":crate::types::hash(&serde_json::to_string(&call.args)?),"reply":reply}))?;
                        }
                        let mut bytes=serde_json::to_vec(&reply)?;bytes.push(b'\n');
                        await_stage(async{stdin.write_all(&bytes).await.map_err(|_|runtime_error())},&cancel,deadline).await?;
                        had_response=true;
                    }
                    Some("validate_output") if ready && !settled && policy.mode == "practice" => {
                        output_validations += 1;
                        if output_validations > 3 { return Err(sidecar_error("TOOL_LIMIT")); }
                        let validator = request_context.output_validator.as_ref().ok_or_else(|| sidecar_error("TOOLS_DISABLED"))?;
                        let id = event["id"].as_str().filter(|s| crate::types::valid_id(s)).ok_or_else(|| sidecar_error("TOOL_PROTOCOL"))?;
                        let content = event["content"].as_str().filter(|s| s.len() <= 256*1024).ok_or_else(|| sidecar_error("OUTPUT_LIMIT"))?;
                        let result = match validator(content) {
                            Ok(()) => json!({"valid":true}),
                            Err(error) => json!({"valid":false,"recoverable":error.details.as_ref().is_some_and(|d| d["recoverable"]==true),"message":error.message}),
                        };
                        let mut bytes = serde_json::to_vec(&json!({"type":"tool_result","id":id,"result":result}))?; bytes.push(b'\n');
                        await_stage(async {stdin.write_all(&bytes).await.map_err(|_|runtime_error())}, &cancel, deadline).await?;
                    }
                    Some("context_manifest") if ready && !settled => {
                        let manifest=event.get("manifest").cloned().ok_or_else(||sidecar_error("MODEL_RESPONSE"))?;
                        if manifest["version"]!=1 || manifest["entries"].as_array().is_none_or(|a|a.len()>256) {return Err(sidecar_error("MODEL_RESPONSE"));}
                        if let Some(observer)=&request_context.context_observer {observer(manifest)?;}
                    }
                    Some("session_lifecycle") if ready && !settled => {
                        if !matches!(event["event"].as_str(),Some("compaction_start"|"compaction_end")){return Err(sidecar_error("MODEL_RESPONSE"));}
                        if compaction_clears_dedup(&event) {
                            if let Some(observer)=&request_context.compaction_observer {observer();}
                        }
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
                        let receipt_path=request_context.native_session.as_ref().map(|n|n.directory.join(format!("tool-{}.json",crate::types::hash(&serde_json::to_string(&json!([n.run_id,call.id])).unwrap()))));
                        let receipt_record=|reply:Value|json!({"version":1,"runId":request_context.native_session.as_ref().map(|n|&n.run_id),"callId":call.id,"name":call.name,"argsHash":crate::types::hash(&serde_json::to_string(&call.args).unwrap()),"reply":reply});
                        if let Some(path)=&receipt_path {crate::native_session::atomic(path,&receipt_record(Value::Null))?;}
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
                                Err(error) => {
                                    let mut reply = tool_failure(&call.id, &error.code);
                                    // Only validator-produced diagnostics: never echo model values or arbitrary IO errors.
                                    if error.code == "INVALID_MODEL_OUTPUT" && error.details.as_ref().is_some_and(|d| d["field"].is_string()) {
                                        reply["error"]["message"] = json!(error.message);
                                    }
                                    reply
                                },
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
                        if let Some(path)=&receipt_path {crate::native_session::atomic(path,&receipt_record(reply.clone()))?;}
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
                        if let Some(diagnostic) =
                            event.get("diagnostic").and_then(safe_response_diagnostic)
                        {
                            record_response_diagnostic(
                                diagnostic_path.as_deref(),
                                "OK",
                                &diagnostic,
                            );
                        }
                        settled = true;
                    }
                    Some("error") => {
                        let mut error = sidecar_error(
                            event
                                .get("code")
                                .and_then(Value::as_str)
                                .unwrap_or("PI_RUNTIME"),
                        );
                        if let Some(diagnostic) =
                            event.get("diagnostic").and_then(safe_response_diagnostic)
                        {
                            record_response_diagnostic(
                                diagnostic_path.as_deref(),
                                &error.code,
                                &diagnostic,
                            );
                            error.details = Some(json!({"response":diagnostic}));
                        }
                        return Err(error);
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
    crate::request_debug::finish_attempt(&debug_attempt, result.as_ref().err().map(|error|error.code.as_str()));
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
    if call.name == "list_learning_skills" {
        if !call
            .args
            .as_object()
            .is_some_and(|object| object.is_empty())
        {
            return Err(sidecar_error("TOOL_PROTOCOL"));
        }
        return Ok(());
    }
    if call.name == "load_learning_skill" {
        let a = call
            .args
            .as_object()
            .ok_or_else(|| sidecar_error("TOOL_PROTOCOL"))?;
        if a.keys().any(|k| k != "skillId")
            || a.len() != 1
            || a.get("skillId")
                .and_then(Value::as_str)
                .is_none_or(|id| id.is_empty() || id.chars().count() > 128)
        {
            return Err(sidecar_error("TOOL_PROTOCOL"));
        }
        return Ok(());
    }
    if call.name == "generate_exercises" {
        let a = call
            .args
            .as_object()
            .ok_or_else(|| sidecar_error("TOOL_PROTOCOL"))?;
        if a.keys()
            .any(|k| !["requirements", "exercises", "summary"].contains(&k.as_str()))
            || a.get("requirements")
                .and_then(Value::as_str)
                .is_none_or(|s| s.trim().is_empty() || s.chars().count() > 8000)
            || a.get("summary")
                .and_then(Value::as_str)
                .is_none_or(|s| s.chars().count() > 10000)
            || a.get("exercises")
                .and_then(Value::as_array)
                .is_none_or(|a| a.is_empty() || a.len() > 5)
            || serde_json::to_vec(&call.args)?.len() > MAX_TOOL_BYTES
        {
            return Err(sidecar_error("TOOL_PROTOCOL"));
        }
        return Ok(());
    }
    if matches!(call.name.as_str(), "read" | "write" | "bash") {
        // Shape validation only; the authorization tier and workspace
        // confinement are enforced by authorized_tools at execution.
        let a = call
            .args
            .as_object()
            .ok_or_else(|| sidecar_error("TOOL_PROTOCOL"))?;
        if serde_json::to_vec(&call.args)?.len() > MAX_TOOL_BYTES {
            return Err(sidecar_error("TOOL_LIMIT"));
        }
        let text = |key: &str| a.get(key).and_then(Value::as_str);
        let protocol = || sidecar_error("TOOL_PROTOCOL");
        match call.name.as_str() {
            "read" => {
                if a.keys().any(|k| !["sourceId", "path"].contains(&k.as_str()))
                    || a.contains_key("sourceId") == a.contains_key("path")
                {
                    return Err(protocol());
                }
                if let Some(id) = text("sourceId") {
                    if !crate::types::valid_id(id) {
                        return Err(protocol());
                    }
                }
                if let Some(path) = text("path") {
                    if path.is_empty() || path.chars().count() > 128 {
                        return Err(protocol());
                    }
                }
            }
            "write" => {
                if a.get("expectedVersion").is_some_and(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 128)) { return Err(protocol()); }
                if a.keys().any(|k| !["path", "content", "expectedVersion"].contains(&k.as_str())) {
                    return Err(protocol());
                }
                match (text("path"), text("content")) {
                    (Some(path), Some(content)) => {
                        if path.is_empty()
                            || path.chars().count() > 128
                            || content.trim().is_empty()
                            || content.chars().count() > 20000
                        {
                            return Err(protocol());
                        }
                    }
                    _ => return Err(protocol()),
                }
            }
            "bash" => {
                if a.keys().any(|k| !["command", "timeoutMs"].contains(&k.as_str())) {
                    return Err(protocol());
                }
                match text("command") {
                    Some(command) => {
                        if command.trim().is_empty() || command.chars().count() > 4000 {
                            return Err(protocol());
                        }
                    }
                    None => return Err(protocol()),
                }
                if let Some(timeout) = a.get("timeoutMs") {
                    if timeout.as_u64().is_none_or(|ms| !(1000..=30000).contains(&ms)) {
                        return Err(protocol());
                    }
                }
            }
            _ => unreachable!(),
        }
        return Ok(());
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
        "create_todo" => (
            &[
                ("title", 500),
                ("dueAt", 64),
                ("relativeMinutes", 0),
                ("reminderEnabled", 0),
            ],
            &["title"],
        ),
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
        if key == "relativeMinutes" {
            if value.as_u64().is_none_or(|n| !(1..=525600).contains(&n)) {
                return Err(sidecar_error("TOOL_PROTOCOL"));
            }
        } else if key == "reminderEnabled" {
            if value.as_bool().is_none() {
                return Err(sidecar_error("TOOL_PROTOCOL"));
            }
        } else if key == "tags" {
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
            if key == "dueAt" && chrono::DateTime::parse_from_rfc3339(text).is_err() {
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
        || (call.name == "create_todo"
            && args.get("reminderEnabled").and_then(Value::as_bool) == Some(true)
            && !args.contains_key("dueAt")
            && !args.contains_key("relativeMinutes"))
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
fn canonical_provider(provider: &str) -> Result<&str> {
    match provider {
        "custom" => Ok("custom"),
        "glm-pi" | "openai-compatible" => Ok("glm-pi"),
        _ => Err(invalid("请选择自定义 API 或 GLM Coding Plan 通道。")),
    }
}
#[cfg(test)]
fn validate_settings(
    provider: &str,
    base_url: &str,
    model: &str,
    allow_unconfigured: bool,
) -> Result<String> {
    validate_protocol_settings(provider, PROTOCOL, base_url, model, allow_unconfigured)
}
fn validate_protocol_settings(
    provider: &str,
    protocol: &str,
    base_url: &str,
    model: &str,
    allow_unconfigured: bool,
) -> Result<String> {
    let provider = canonical_provider(provider)?;
    if ![PROTOCOL, ANTHROPIC].contains(&protocol) || (provider == "glm-pi" && protocol != PROTOCOL)
    {
        return Err(invalid("自定义服务支持 OpenAI Chat Completions 或 Anthropic Messages；Coding Plan 仅支持 OpenAI 协议。"));
    }
    if allow_unconfigured && base_url.is_empty() && model.is_empty() {
        return Ok(String::new());
    }
    if model.is_empty()
        || model.len() > 256
        || model.chars().any(char::is_control)
        || (provider == "glm-pi" && model != MODEL_ID)
    {
        return Err(invalid(
            "请输入有效模型 ID（最多 256 字节）；Coding Plan 仅支持 glm-5.3-flash。",
        ));
    }
    if base_url.is_empty()
        || base_url.len() > 2048
        || base_url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(invalid("请输入有效 API 地址。"));
    }
    let url = Url::parse(base_url).map_err(|_| invalid("API 地址无效。"))?;
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
    if protocol == ANTHROPIC
        && (url.path().trim_end_matches('/').ends_with("/v1")
            || url.path().trim_end_matches('/').ends_with("/messages"))
    {
        return Err(invalid("Anthropic 基础地址请填根地址或代理前缀，不含末尾 /v1 或 /messages；实际会追加 /v1/messages?beta=true。"));
    }
    if url.host_str().is_none() || (url.scheme() != "https" && !is_fixture(&normalized)) {
        return Err(invalid("API 地址必须使用 HTTPS；仅本机测试可使用 HTTP。"));
    }
    if provider == "glm-pi"
        && normalized != DOMESTIC
        && normalized != INTERNATIONAL
        && !is_fixture(&normalized)
    {
        return Err(invalid(
            "GLM Coding Plan 仅支持国内或国际专用地址。自定义 API 请另选自定义通道。",
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
fn settings_profile(provider: &str, endpoint: &str) -> Result<String> {
    if canonical_provider(provider)? == "custom" {
        Ok("custom".into())
    } else {
        profile_id(endpoint)
    }
}
fn usage_blocked(provider: &str, endpoint: &str) -> bool {
    if is_fixture(endpoint) {
        return false;
    }
    provider != "custom"
        || Url::parse(endpoint).is_ok_and(|url| {
            matches!(url.host_str(), Some("open.bigmodel.cn" | "api.z.ai"))
                && url.path().starts_with("/api/coding/")
        })
}
fn ensure_usage_allowed(settings: &ModelSettings) -> Result<()> {
    validate_protocol_settings(
        &settings.provider,
        &settings.protocol,
        &settings.base_url,
        &settings.model,
        false,
    )?;
    if usage_blocked(&settings.provider, &settings.base_url) {
        Err(sidecar_error(USAGE_GATE).details(json!({"category":"local_policy","reason":"coding_plan_endpoint","stage":"before_credentials","requestSent":false})))
    } else {
        Ok(())
    }
}
fn sidecar_error(code: &str) -> CoreError {
    let (safe_code, message) = match code {
        "PLAN_USAGE_UNCONFIRMED" => (
            USAGE_GATE,
            "本地用途限制：当前地址是 Coding Plan 专用通道，应用用途适用性尚未确认，未发送模型请求。这不是服务器连接失败。可保留配置等待确认，或自行配置具有普通 API 权限的地址；程序不会自动更换通道。",
        ),
        "AUTH" => ("AUTH", "API 认证失败，请检查所选区域的密钥和模型权限。"),
        "RATE_LIMIT" => ("RATE_LIMIT", "模型服务限流或额度不足，请检查额度后重试。"),
        "TIMEOUT" => ("TIMEOUT", "模型请求超时；已收到的内容已保留。"),
        "CANCELLED" => ("CANCELLED", "生成已停止，已收到的内容已保留。"),
        "NETWORK" => ("NETWORK", "模型连接失败或中断；已收到的内容已保留。"),
        "NATIVE_SESSION_INVALID" => ("NATIVE_SESSION_INVALID","原生学习会话缺失或损坏；旧学习记录仍保留，未静默创建新会话。"),
        "NATIVE_SESSION_BUSY" => ("NATIVE_SESSION_BUSY","本学习会话仍有请求在执行，请稍后核对原请求。"),
        "NATIVE_TOOL_UNKNOWN" => ("NATIVE_TOOL_UNKNOWN","原工具调用回执尚未核实；未重复执行写入，请保留原请求编号。"),
        "INVALID_INPUT" => (
            "INVALID_INPUT",
            "模型输入无效或过大，请缩小所选材料后重试。",
        ),
        "INVALID_URL" => (
            "INVALID_URL",
            "通道地址无效或返回重定向，请检查所填 API 地址。",
        ),
        "INVALID_MODEL_SETTINGS" => (
            "INVALID_MODEL_SETTINGS",
            "请检查所选 API 协议、地址和模型。",
        ),
        "INVALID_MODEL_LIMITS" => ("INVALID_MODEL_LIMITS", "输出或上下文 token 上限无效，请在供应商设置中检查。"),
        "MODEL_HTTP" => (
            "MODEL_HTTP",
            "模型服务拒绝请求，请检查所选区域、密钥和模型权限。",
        ),
        "MODEL_ENDPOINT_PAGE" => ("MODEL_ENDPOINT_PAGE", "服务地址返回了网页，未得到模型 API 响应。请核对基础地址是否需要 /v1，并查看已保存的最终请求 URL；应用未自动改地址。"),
        "MODEL_FORMAT" => ("MODEL_FORMAT", "服务返回的格式不符合所选流式协议，请核对协议和最终请求 URL；未把它当作完整答案。"),
        "MODEL_LENGTH" => ("MODEL_LENGTH", "模型因输出 token 上限而结束（思考也可能占用此预算）；可用正文已保留。可在供应商设置调整输出上限，不会自动重试。"),
        "MODEL_INCOMPLETE" => ("MODEL_INCOMPLETE", "模型流在有效结束标记前结束；可用正文已保留，未当作完整答案。"),
        "MODEL_THINKING_ONLY" => ("MODEL_THINKING_ONLY", "模型仅返回思考内容，没有最终正文；思考内容未当作学习答案。"),
        "MODEL_EMPTY" => ("MODEL_EMPTY", "模型声明结束，但没有返回有效正文。"),
        "OUTPUT_LIMIT" => (
            "OUTPUT_LIMIT",
            "模型输出超过限制，已停止；已收到的内容已保留。",
        ),
        "TOOL_LIMIT" => ("TOOL_LIMIT", "学习工具调用超过本次预算，已停止。"),
        "LIMIT_EXCEEDED" => ("LIMIT_EXCEEDED", "完整材料变化及必要正文超过发送容量；未截断，原问题与版本保留。"),
        "CONTEXT_CONFLICT" => ("CONTEXT_CONFLICT", "聚焦正文与固定材料类型不一致，正文未发送；请保留原问题并重新核对聚焦材料。"),
        "TIME_CONTEXT_INVALID" => ("TIME_CONTEXT_INVALID","请求时间或时区无效，未发送模型请求。"),
        "SKILL_MISSING" => ("SKILL_MISSING", "本次学习指导缺失，未发送模型请求。"),
        "SKILL_INVALID" => ("SKILL_INVALID", "学习指导版本或模式校验失败，未发送模型请求。"),
        "TOOL_PROTOCOL" => ("TOOL_PROTOCOL", "学习工具协议或参数无效，已停止。"),
        "TOOL_FAILED" => ("TOOL_FAILED", "学习工具执行失败，未确认完成。"),
        "TOOLS_DISABLED" => ("TOOLS_DISABLED", "模型请求了未启用的工具，已停止本次生成。"),
        "MODEL_RESPONSE" => (
            "MODEL_RESPONSE",
            "模型响应未通过完整性校验；可用正文已保留，未当作完整答案。",
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
    if safe_code == USAGE_GATE {
        error.details = Some(
            json!({"category":"local_policy","reason":"coding_plan_endpoint","requestSent":false}),
        );
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
        "无法保存本应用此通道的密钥，请检查本机钥匙串权限。",
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

    #[test]
    fn only_completed_compaction_clears_guidance_dedup() {
        assert!(compaction_clears_dedup(&json!({"event":"compaction_end"})));
        assert!(!compaction_clears_dedup(&json!({"event":"compaction_end","aborted":true})));
        assert!(!compaction_clears_dedup(&json!({"event":"compaction_start"})));
        assert!(!compaction_clears_dedup(&json!({"event":"tool_request"})));
        assert!(!compaction_clears_dedup(&json!({})));
    }
    fn settings(endpoint: &str) -> ModelSettings {
        let mut value = ModelSettings::default();
        value.configured = true;
        value.provider = "glm-pi".into();
        value.model = MODEL_ID.into();
        value.base_url = endpoint.into();
        value.active_profile = profile_id(endpoint).unwrap();
        value
    }
    #[test]
    fn exercise_tool_summary_counts_unicode_characters_not_bytes() {
        for summary in ["汉".repeat(4000), "🧠".repeat(6000)] {
            let call = ToolCall {
                id: "unicode-call".into(),
                name: "generate_exercises".into(),
                args: json!({"requirements":"BST", "summary":summary,"exercises":[{}]}),
            };
            assert!(validate_tool_call(&call).is_ok());
        }
        let call = ToolCall {
            id: "unicode-call".into(),
            name: "generate_exercises".into(),
            args: json!({"requirements":"BST", "summary":"a".repeat(10001),"exercises":[{}]}),
        };
        assert!(validate_tool_call(&call).is_err());
    }
    #[test]
    fn defaults_are_unconfigured_custom_without_guessing_endpoint_or_model() {
        let directory = tempfile::tempdir().unwrap();
        let store = ModelStore::new(directory.path()).unwrap();
        let settings = store.settings();
        assert!(!settings.configured);
        assert!(settings.base_url.is_empty());
        assert!(settings.active_profile.is_empty());
        assert_eq!(settings.provider, "custom");
        assert_eq!(settings.protocol, PROTOCOL);
        assert!(settings.model.is_empty());
        assert_eq!(settings.harness, HARNESS);
        assert!(settings.usage_gate.is_empty());
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
            store.keychain_account(DOMESTIC, PROTOCOL),
            store.keychain_account(INTERNATIONAL, PROTOCOL)
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
    #[test]
    fn custom_settings_validate_explicit_protocol_url_and_model_without_keychain() {
        assert_eq!(
            validate_protocol_settings(
                "custom",
                PROTOCOL,
                "https://API.example.com:443/custom/v1/",
                "user/model",
                false
            )
            .unwrap(),
            "https://api.example.com/custom/v1"
        );
        for url in [
            "http://example.com/v1",
            "file:///tmp/fixture-secret",
            "https://user:fixture-secret@example.com/v1",
            "https://@example.com/v1",
            "https://example.com/v1?",
            "https://example.com/v1#",
            "https://example.com/v1?key=fixture-secret",
        ] {
            let error =
                validate_protocol_settings("custom", PROTOCOL, url, "my-model", false).unwrap_err();
            assert!(!error.to_string().contains("fixture-secret"));
        }
        for protocol in ["", "unsupported-protocol", "openai-responses"] {
            assert!(validate_protocol_settings(
                "custom",
                protocol,
                "https://example.com/v1",
                "my-model",
                false
            )
            .is_err());
        }
        for model in ["".to_owned(), "x".repeat(257), "invalid\nmodel".into()] {
            assert!(validate_protocol_settings(
                "custom",
                PROTOCOL,
                "https://example.com/v1",
                &model,
                false
            )
            .is_err());
        }
        let mut selected = ModelSettings::default();
        selected.base_url = "https://example.com/v1".into();
        selected.model = "my-model".into();
        assert!(ensure_usage_allowed(&selected).is_ok());
        selected.base_url = DOMESTIC.into();
        assert_eq!(
            ensure_usage_allowed(&selected).unwrap_err().code,
            USAGE_GATE
        );
    }
    #[test]
    fn custom_save_and_reopen_are_nonsecret_and_address_changes_require_a_new_key() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ModelStore::new(directory.path()).unwrap();
        store.settings.configured = true;
        store.settings.base_url = "https://example.com/custom/v1".into();
        store.settings.model = "previous-model".into();
        store.settings.active_profile = "custom".into();
        store.settings.verified = true;
        let before = serde_json::to_value(store.settings()).unwrap();
        let error = store
            .configure_protocol(
                "custom",
                PROTOCOL,
                "https://example.com/other/v1",
                "user/model",
                "",
            )
            .unwrap_err();
        assert_eq!(error.code, "AUTH");
        assert_eq!(serde_json::to_value(store.settings()).unwrap(), before);
        assert!(!store.config_path.exists());
        let saved = store
            .configure_protocol(
                "custom",
                PROTOCOL,
                "https://EXAMPLE.com:443/custom/v1/",
                "user/model",
                "",
            )
            .unwrap();
        assert!(!saved.verified);
        assert!(saved.usage_gate.is_empty());
        let reopened = ModelStore::new(directory.path()).unwrap();
        assert_eq!(reopened.settings().provider, "custom");
        assert_eq!(reopened.settings().protocol, PROTOCOL);
        assert_eq!(reopened.settings().model, "user/model");
        assert!(!std::fs::read_to_string(&store.config_path)
            .unwrap()
            .contains("apiKey"));
        assert_ne!(
            store.keychain_account("https://example.com/v1", PROTOCOL),
            store.keychain_account("https://other.example/v1", PROTOCOL)
        );
        assert_ne!(
            store.keychain_account("https://example.com/v1", PROTOCOL),
            store.keychain_account("https://example.com/v1", "future-protocol")
        );
        assert_eq!(
            store.keychain_account("https://EXAMPLE.com:443/v1", PROTOCOL),
            store.keychain_account("https://example.com/other", PROTOCOL)
        );
    }
    #[tokio::test]
    async fn custom_real_pi_local_fixture_preserves_model_and_rejects_truncation() {
        for finish in [true, false] {
            let (credentials, server) = fixture(finish, true);
            let mut partial = String::new();
            let result = generate(
                credentials,
                vec![json!({"role":"user","content":"fixture only"})],
                Arc::new(AtomicBool::new(false)),
                |delta| partial.push_str(delta),
            )
            .await;
            if finish {
                assert_eq!(result.unwrap(), "Pi fixture 文本");
                assert_eq!(partial, "Pi fixture 文本");
            } else {
                assert_eq!(result.unwrap_err().code, "MODEL_INCOMPLETE");
            }
            server.join().unwrap();
        }
    }
    #[tokio::test]
    async fn request_debug_real_stdio_boundary_on_off_and_failed_response() {
        use crate::request_debug;
        for (enabled,complete) in [(false,true),(true,true),(true,false)] {
            request_debug::set(enabled);
            let (credentials,server)=fixture(complete,true);
            let identity=json!({"courseId":"debug-course","sessionId":"debug-session","runId":"debug-run","requestId":"debug-request","retry":true});
            let result=generate_scoped(credentials,vec![json!({"role":"user","content":"debug-wire-boundary-synthetic"})],Arc::new(AtomicBool::new(false)),|_|{},None,ToolPolicy::legacy(false),vec![],Some(RequestContext{debug_identity:Some(identity.clone()),debug_registrations:enabled.then(||json!([{"key":"actual_provider","lifetime":"OneShot","position":"References","value":"wire registration"}])),..RequestContext::capture()})).await;
            let snapshot=request_debug::snapshot();
            request_debug::set(false);
            server.join().unwrap();
            let records:Vec<_>=snapshot["records"].as_array().unwrap().iter().filter(|r|r["body"].to_string().contains("debug-wire-boundary-synthetic")).collect();
            if !enabled {assert!(records.is_empty());continue;}
            assert_eq!(records.len(),1,"actual stdio record missing: {snapshot}");
            assert_eq!(records[0]["body"]["model"],"user-selected/model-2026");
            assert_eq!(records[0]["body"]["max_tokens"],6144);
            assert_eq!(records[0]["headers"]["authorization"],"[REDACTED]");
            assert_eq!(records[0]["status"],"HTTP 200");
            assert_eq!(records[0]["registrations"][0]["key"],"actual_provider");
            for key in ["courseId","sessionId","runId","requestId","retry"]{assert_eq!(records[0]["identity"][key],identity[key]);}
            assert!(!records[0].to_string().contains("fixture-secret"));
            assert_eq!(records[0]["runOutcome"],if complete {"complete"} else {"MODEL_INCOMPLETE"});
            assert_eq!(result.is_ok(),complete);
        }
    }
    fn fixture(finish: bool, custom: bool) -> (ModelCredentials, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(45);
            let (mut socket, _) = loop {
                match listener.accept() {
                    Ok(socket) => break socket,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("local fixture accept deadline: {error}"),
                }
            };
            // macOS accepted sockets inherit O_NONBLOCK from the bounded listener.
            socket.set_nonblocking(false).unwrap();
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
            if custom {
                assert_eq!(payload["model"], "user-selected/model-2026");
                assert!(payload.get("thinking").is_none());
                assert!(payload.get("reasoning_effort").is_none());
            } else {
                assert_eq!(payload["model"], MODEL_ID);
                assert_eq!(payload["thinking"]["type"], "enabled");
            }
            assert!(payload.get("tools").is_none());
            assert_eq!(payload["max_tokens"], 6_144);
            assert!(payload.get("max_completion_tokens").is_none());
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
        let mut selected = settings(&format!("http://{address}/v1"));
        selected.max_output_tokens = Some(6_144);
        selected.context_window = None;
        if custom {
            selected.provider = "custom".into();
            selected.model = "user-selected/model-2026".into();
            selected.active_profile = "custom".into();
            selected.usage_gate.clear();
        }
        (
            ModelCredentials::new(selected, "fixture-secret".into()).unwrap(),
            server,
        )
    }
    #[tokio::test]
    async fn rust_generation_uses_real_pi_agent_sidecar_for_local_fixture() {
        let (credentials, server) = fixture(true, false);
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
        let (credentials, server) = fixture(false, false);
        let error = generate(
            credentials,
            vec![json!({"role":"user","content":"fixture only"})],
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await
        .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.code, "MODEL_INCOMPLETE");
    }
    #[tokio::test]
    async fn authorized_write_tool_round_trips_through_the_real_sidecar_without_internal_tier_notice() {
        use crate::{
            app::{Engine, UiPreferences},
            authorized_tools,
            learning_tools::ToolScope,
        };
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("library");
        let runtime = directory.path().join("runtime");
        let engine = Engine::new(library.clone(), runtime.clone()).unwrap();
        engine
            .save_ui_preferences(UiPreferences {
                tool_authorization_tier: "write".into(),
                ..UiPreferences::default()
            })
            .unwrap();
        let scope = {
            let mut core = engine.lock().unwrap();
            let section = core
                .create_section("受控工具fixture", "Workspace tool fixture body.", vec![], vec![])
                .unwrap();
            let (session, context) = core.start_session(&section.id, false).unwrap();
            let run: crate::generation::Run = serde_json::from_value(json!({
                "id":"run_authorized_wire","sessionId":session.id,"sourceDocumentId":section.id,
                "mode":"question","status":"running","content":"","error":null,
                "createdAt":crate::types::timestamp(),"updatedAt":crate::types::timestamp(),"requestDigest":"fixture",
                "contextSnapshotVersion":1,"contextManifest":context.manifest
            })).unwrap();
            engine.runs.lock().unwrap().insert(run.id.clone(), crate::generation::ManagedRun {
                run, cancel: Arc::new(AtomicBool::new(false)),
            });
            ToolScope {
                session_id: session.id,
                section_id: section.id,
                run_id: "run_authorized_wire".into(),
                allowed_ids: context.manifest.into_iter().map(|item| item.id).collect(),
            }
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server_session = scope.session_id.clone();
        let workspace_file = runtime
            .join("workspaces")
            .join(&scope.session_id)
            .join("files/summary.md");
        let server_workspace_file = workspace_file.clone();
        let server = thread::spawn(move || {
            for turn in 0..2 {
                let deadline = std::time::Instant::now() + Duration::from_secs(8);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(std::time::Instant::now() < deadline);
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => panic!("fixture accept failed"),
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
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
                // The write tier registers read/write on top of the question set.
                assert_eq!(
                    names,
                    [
                        "query_related_knowledge",
                        "record_question",
                        "create_note",
                        "create_todo",
                        "generate_exercises",
                        "read",
                        "write"
                    ]
                );
                assert!(!payload.to_string().contains("工具授权档位"));
                let body = if turn == 0 {
                    let first = json!({"choices":[{"index":0,"delta":{"role":"assistant","content":"我把结论写入工作区。"},"finish_reason":null}]});
                    let call = json!({"choices":[{"index":0,"delta":{"tool_calls":[
                        {"index":0,"id":"authorized-write-1","type":"function","function":{"name":"write","arguments":serde_json::to_string(&json!({"path":"summary.md","content":"fixture scratch conclusion"})).unwrap()}}
                    ]},"finish_reason":null}]});
                    format!("data: {first}\n\ndata: {call}\n\ndata: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\ndata: [DONE]\n\n")
                } else {
                    let tool_messages: Vec<_> = payload["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|message| message["role"] == "tool")
                        .collect();
                    assert_eq!(tool_messages.len(), 1);
                    let receipt: Value =
                        serde_json::from_str(tool_messages[0]["content"].as_str().unwrap()).unwrap();
                    assert_eq!(receipt["ok"], true);
                    assert_eq!(receipt["result"]["persisted"], true);
                    assert_eq!(receipt["result"]["sessionId"], server_session);
                    assert_eq!(receipt["result"]["path"], "summary.md");
                    assert_eq!(
                        std::fs::read_to_string(&server_workspace_file).unwrap(),
                        "fixture scratch conclusion"
                    );
                    let response = json!({"choices":[{"index":0,"delta":{"content":"已写入会话工作区。"},"finish_reason":"stop"}]});
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
                    // Production dispatch: workspace tools run in their own module.
                    if matches!(call.name.as_str(), "read" | "write" | "bash") {
                        authorized_tools::execute(&engine, &scope, &call.id, &call.name, call.args)
                    } else {
                        crate::learning_tools::execute(
                            &engine, &scope, &call.id, &call.name, call.args,
                        )
                    }
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
        let allowed: Vec<String> = [
            "query_related_knowledge",
            "record_question",
            "create_note",
            "create_todo",
            "generate_exercises",
            "read",
            "write",
        ]
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
        let output = generate_scoped(
            credentials,
            vec![json!({"role":"user","content":"请把结论写入会话工作区文件。"})],
            Arc::new(AtomicBool::new(false)),
            |delta| partial.push_str(delta),
            Some(handler.clone()),
            ToolPolicy {
                mode: "question".into(),
                allowed_tools: allowed.clone(),
                max_calls: 4,
                authorization: "write".into(),
            },
            crate::learning_skills::capture("question", &allowed).unwrap(),
            None,
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(output, "已写入会话工作区。");
        assert!(partial.starts_with("我把结论写入工作区。"));
        assert_eq!(
            std::fs::read_to_string(&workspace_file).unwrap(),
            "fixture scratch conclusion"
        );
        // The library course itself stays untouched by workspace writes.
        let mut core = engine.lock().unwrap();
        assert_eq!(
            core.content.read(&scope.section_id).unwrap().body,
            "Workspace tool fixture body."
        );
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
                        {"index":4,"id":"actual-note-1","type":"function","function":{"name":"create_note","arguments":serde_json::to_string(&json!({"content":"Since gives a starting point; for gives a duration.","selection":"since Monday"})).unwrap()}},
                        {"index":5,"id":"actual-todo-1","type":"function","function":{"name":"create_todo","arguments":serde_json::to_string(&json!({"title":"复习 since 与 for","dueAt":"2000-01-01T08:00:00+08:00"})).unwrap()}}
                    ]},"finish_reason":null}]});
                    format!("data: {first}\n\ndata: {call}\n\ndata: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\ndata: [DONE]\n\n")
                } else {
                    let result_messages: Vec<_> = payload["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|message| message["role"] == "tool")
                        .collect();
                    assert_eq!(result_messages.len(), 6);
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
                    let todo = &receipts[5]["result"];
                    assert_eq!(todo["persisted"], true);
                    assert_eq!(todo["todo"]["title"], "复习 since 与 for");
                    assert_eq!(todo["todo"]["sessionId"], server_session);
                    assert_eq!(todo["todo"]["reminderEnabled"], false);
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
            vec![json!({"role":"user","content":"请记录疑问、创建错题、查询关联知识、创建卡片和笔记、创建复习待办但不提醒，并返回练习JSON。"})],
            Arc::new(AtomicBool::new(false)),
            |delta| partial.push_str(delta),
            Some(handler.clone()),
            ToolPolicy { mode:"explain".into(), allowed_tools: ALL_TOOLS.iter().map(|name| (*name).into()).collect(), max_calls:6, authorization:"off".into() },
            crate::learning_skills::capture("explain", &ALL_TOOLS.iter().map(|name| (*name).into()).collect::<Vec<String>>()).unwrap(),
            None,
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
        let todos = reopened.reminders.list_todos().unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].title, "复习 since 与 for");
        assert!(!todos[0].reminder_enabled);
        assert!(reopened.reminders.tick().unwrap().is_empty());
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
            ("question", ALL_TOOLS.to_vec(), 6),
            ("explain", ALL_TOOLS.to_vec(), 6),
            (
                "practice",
                vec![
                    "query_related_knowledge",
                    "list_learning_skills",
                    "load_learning_skill",
                ],
                2,
            ),
            ("review", vec!["query_related_knowledge"], 2),
            ("feedback", vec![], 0),
            ("grading", vec![], 0),
        ] {
            let mut policy = ToolPolicy {
                mode: mode.into(),
                allowed_tools: allowed.into_iter().map(str::to_owned).collect(),
                max_calls: maximum,
                authorization: "off".into(),
            };
            assert!(validate_tool_policy(&policy, true).is_ok());
            policy.max_calls += 1;
            assert!(validate_tool_policy(&policy, true).is_err());
        }
        assert!(validate_tool_policy(
            &ToolPolicy {
                mode: "practice".into(),
                allowed_tools: vec!["create_review_card".into()],
                max_calls: 2,
                authorization: "off".into()
            },
            true
        )
        .is_err());
    }
    #[test]
    fn authorization_tiers_extend_only_question_and_explain_policies() {
        let policy = |mode: &str, tools: &[&str], authorization: &str| ToolPolicy {
            mode: mode.into(),
            allowed_tools: tools.iter().map(|name| (*name).to_owned()).collect(),
            max_calls: 4,
            authorization: authorization.into(),
        };
        assert!(validate_tool_policy(&policy("question", &["read"], "read"), true).is_ok());
        assert!(validate_tool_policy(&policy("explain", &["read", "write", "bash"], "bash"), true).is_ok());
        assert!(validate_tool_policy(&policy("explain", &["read", "write"], "write"), true).is_ok());
        // A tier never unlocks stronger tools than selected.
        assert!(validate_tool_policy(&policy("question", &["write"], "read"), true).is_err());
        assert!(validate_tool_policy(&policy("explain", &["bash"], "write"), true).is_err());
        assert!(validate_tool_policy(&policy("question", &["read"], "off"), true).is_err());
        // Other modes gain nothing from any tier.
        for mode in ["practice", "review", "feedback", "grading", "legacy"] {
            assert!(validate_tool_policy(&policy(mode, &["read"], "bash"), true).is_err());
        }
        // Unknown tiers are rejected outright.
        assert!(validate_tool_policy(&policy("question", &[], "shell"), true).is_err());
    }
    #[test]
    fn authorized_tool_calls_pass_shape_validation_with_tier_enforced_elsewhere() {
        for (name, args) in [
            ("read", json!({"sourceId":"knowledge_1"})),
            ("read", json!({"path":"notes/summary.md"})),
            ("write", json!({"path":"notes/summary.md","content":"hello"})),
            ("bash", json!({"command":"printf hi"})),
            ("bash", json!({"command":"printf hi","timeoutMs":1500})),
        ] {
            assert!(
                validate_tool_call(&ToolCall {
                    id: "call-1".into(),
                    name: name.into(),
                    args
                })
                .is_ok(),
                "{name} shape must pass protocol validation"
            );
        }
        for (name, args) in [
            ("read", json!({})),
            ("read", json!({"sourceId":"a","path":"b"})),
            ("read", json!({"sourceId":"../../etc/passwd"})),
            ("read", json!({"sourceId":"ok","extra":1})),
            ("write", json!({"path":"a.md"})),
            ("write", json!({"content":"x"})),
            ("write", json!({"path":"","content":"x"})),
            ("bash", json!({"command":"   "})),
            ("bash", json!({"command":"echo","timeoutMs":100})),
            ("bash", json!({"command":"echo","timeoutMs":99000})),
            ("bash", json!({"command":"echo","extra":1})),
        ] {
            assert!(
                validate_tool_call(&ToolCall {
                    id: "call-1".into(),
                    name: name.into(),
                    args
                })
                .is_err(),
                "{name} invalid shape must be rejected"
            );
        }
    }
    #[test]
    fn create_todo_reminder_flag_needs_boolean_and_a_time() {
        for args in [
            json!({"title":"T","reminderEnabled":false}),
            json!({"title":"T","dueAt":"2026-10-18T10:00:00+08:00","reminderEnabled":true}),
            json!({"title":"T","relativeMinutes":5,"reminderEnabled":true}),
        ] {
            assert!(
                validate_tool_call(&ToolCall {
                    id: "call-1".into(),
                    name: "create_todo".into(),
                    args
                })
                .is_ok()
            );
        }
        for args in [
            json!({"title":"T","reminderEnabled":true}),
            json!({"title":"T","reminderEnabled":"yes"}),
            json!({"title":"T","reminderEnabled":1}),
        ] {
            assert!(
                validate_tool_call(&ToolCall {
                    id: "call-1".into(),
                    name: "create_todo".into(),
                    args
                })
                .is_err()
            );
        }
    }
    #[test]
    fn tool_protocol_rejects_scope_fields_unknown_tools_and_large_arguments() {
        for (name, args) in [
            (
                "record_question",
                json!({"question":"Q","sessionId":"another"}),
            ),
            ("query_related_knowledge", json!({})),
            ("shell", json!({"command":"denied"})),
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

#[cfg(test)]
mod policy_feedback_tests {
    use super::*;
    #[test]
    fn custom_coding_endpoint_reports_local_policy_before_credential_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ModelStore::new(dir.path()).unwrap();
        store.settings.provider = "custom".into();
        store.settings.protocol = PROTOCOL.into();
        store.settings.base_url = DOMESTIC.into();
        store.settings.model = MODEL_ID.into();
        store.settings.configured = true;
        store.settings.active_profile = "custom".into();
        let error = store.credentials().unwrap_err();
        assert_eq!(error.code, USAGE_GATE);
        assert_eq!(error.details.as_ref().unwrap()["category"], "local_policy");
        assert_eq!(
            error.details.as_ref().unwrap()["stage"],
            "before_credentials"
        );
        assert_eq!(error.details.as_ref().unwrap()["requestSent"], false);
        assert!(error.message.contains("本地用途限制"));
        assert!(!error.retryable);
    }
    #[test]
    fn ordinary_custom_target_does_not_inherit_a_saved_coding_plan_gate() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::new(dir.path()).unwrap();
        let mut settings = ModelSettings::default();
        settings.provider = "custom".into();
        settings.base_url = "https://api.example.com/v1".into();
        settings.model = "user-selected-model".into();
        settings.configured = true;
        settings.active_profile = "custom".into();
        settings.usage_gate = USAGE_GATE.into();
        settings.profiles.push(ModelProfileSettings {
            profile: "custom".into(),
            provider: "custom".into(),
            protocol: PROTOCOL.into(),
            configured: true,
            base_url: settings.base_url.clone(),
            model: settings.model.clone(),
            verified: false,
            max_output_tokens: None,
            context_window: None,
        });
        store.persist(&settings).unwrap();
        let reopened = ModelStore::new(dir.path()).unwrap().settings();
        assert_eq!(reopened.provider, "custom");
        assert_eq!(reopened.base_url, "https://api.example.com/v1");
        assert!(reopened.usage_gate.is_empty());
        assert!(ensure_usage_allowed(&reopened).is_ok());
    }
}

fn request_url(protocol: &str, base_url: &str) -> String {
    if base_url.is_empty() {
        return String::new();
    }
    format!(
        "{}{}",
        base_url.trim_end_matches('/'),
        if protocol == ANTHROPIC {
            "/v1/messages?beta=true"
        } else {
            "/chat/completions"
        }
    )
}
/// Only typed, bounded metadata crosses the sidecar boundary; raw response fields never do.
fn safe_response_diagnostic(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    let mut result = serde_json::Map::new();
    result.insert("version".into(), json!(1));
    for (key, allowed) in [
        ("operation", &["test", "generate"][..]),
        ("protocol", &[PROTOCOL, ANTHROPIC][..]),
        (
            "requestPath",
            &[
                "/chat/completions",
                "/v1/chat/completions",
                "/v1/messages?beta=true",
                "custom-base/chat/completions",
                "custom-base/v1/messages?beta=true",
            ][..],
        ),
        (
            "contentType",
            &["sse", "html", "json", "plain", "other", "missing"][..],
        ),
        (
            "finishReason",
            &[
                "stop",
                "end",
                "length",
                "tool_calls",
                "function_call",
                "content_filter",
                "network_error",
                "end_turn",
                "max_tokens",
                "stop_sequence",
                "tool_use",
                "refusal",
                "missing",
                "other",
            ][..],
        ),
    ] {
        if let Some(text) = object
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| allowed.contains(text))
        {
            result.insert(key.into(), json!(text));
        }
    }
    for key in [
        "requests",
        "httpStatus",
        "maxTokens",
        "transportBytes",
        "providerEvents",
        "textChars",
        "thinkingChars",
        "textEvents",
        "thinkingEvents",
        "usageOutputTokens",
        "usageReasoningTokens",
    ] {
        if let Some(number) = object
            .get(key)
            .and_then(Value::as_u64)
            .filter(|number| *number <= 1_000_000_000)
        {
            result.insert(key.into(), json!(number));
        }
    }
    for key in ["sawDone", "cleanEof", "messageStop", "invalidDelta"] {
        if let Some(flag) = object.get(key).and_then(Value::as_bool) {
            result.insert(key.into(), json!(flag));
        }
    }
    Some(Value::Object(result))
}
fn record_response_diagnostic(path: Option<&Path>, code: &str, diagnostic: &Value) {
    if let Some(path) = path {
        let _ = crate::app::atomic_json(
            path,
            &json!({"schemaVersion":1,"recordedAt":crate::types::timestamp(),"code":code,"response":diagnostic}),
        );
    }
}

#[cfg(test)]
mod configuration_response_tests {
    use super::*;
    #[test]
    fn protocols_derive_exact_request_targets_without_guessing_v1() {
        assert_eq!(
            request_url(PROTOCOL, "https://example.invalid"),
            "https://example.invalid/chat/completions"
        );
        assert_eq!(
            request_url(PROTOCOL, "https://example.invalid/v1/"),
            "https://example.invalid/v1/chat/completions"
        );
        assert_eq!(
            request_url(ANTHROPIC, "https://example.invalid/proxy"),
            "https://example.invalid/proxy/v1/messages?beta=true"
        );
        assert!(validate_protocol_settings(
            "custom",
            ANTHROPIC,
            "https://example.invalid",
            "user-model",
            false
        )
        .is_ok());
        for path in ["/v1", "/v1/", "/v1/messages", "/messages"] {
            assert!(validate_protocol_settings(
                "custom",
                ANTHROPIC,
                &format!("https://example.invalid{path}"),
                "user-model",
                false
            )
            .is_err());
        }
        assert!(
            validate_protocol_settings("glm-pi", ANTHROPIC, DOMESTIC, MODEL_ID, false).is_err()
        );
    }
    #[test]
    fn model_only_change_preserves_binding_and_never_needs_keychain() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = ModelStore::new(temp.path()).unwrap();
        let mut settings = ModelSettings::default();
        settings.configured = true;
        settings.provider = "custom".into();
        settings.protocol = ANTHROPIC.into();
        settings.base_url = "https://example.invalid/proxy".into();
        settings.model = "old-model".into();
        settings.active_profile = "custom".into();
        settings.verified = true;
        settings.profiles.push(ModelProfileSettings {
            profile: "custom".into(),
            provider: "custom".into(),
            protocol: ANTHROPIC.into(),
            configured: true,
            base_url: settings.base_url.clone(),
            model: settings.model.clone(),
            verified: true,
            max_output_tokens: None,
            context_window: None,
        });
        store.persist(&settings).unwrap();
        store.settings = settings.clone();
        let changed = store.set_model("new-model").unwrap();
        assert_eq!(changed.base_url, settings.base_url);
        assert_eq!(changed.protocol, settings.protocol);
        assert_eq!(changed.active_profile, settings.active_profile);
        assert!(!changed.verified);
        let old = std::fs::read(&store.config_path).unwrap();
        assert!(store.set_model("bad\nmodel").is_err());
        assert_eq!(std::fs::read(&store.config_path).unwrap(), old);
        store.set_model("new-model").unwrap();
        assert_eq!(std::fs::read(&store.config_path).unwrap(), old);
        assert_eq!(
            ModelStore::new(temp.path()).unwrap().settings().model,
            "new-model"
        );
    }
    #[test]
    fn response_metadata_cannot_persist_headers_content_or_unknown_stop_text() {
        let value = json!({"httpStatus":200,"contentType":"html","requestPath":"/chat/completions","finishReason":"raw-secret","textChars":0,"thinkingChars":12,"content":"raw-secret","authorization":"raw-secret","headers":{"key":"raw-secret"},"maxTokens":999999999999u64});
        let diagnostic = safe_response_diagnostic(&value).unwrap();
        assert_eq!(diagnostic["httpStatus"], 200);
        assert!(diagnostic.get("maxTokens").is_none());
        assert!(!diagnostic.to_string().contains("raw-secret"));
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("diagnostic.json");
        record_response_diagnostic(Some(&path), "MODEL_ENDPOINT_PAGE", &diagnostic);
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("raw-secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

#[cfg(test)]
#[path = "model_time_tests.rs"]
mod time_intent_tests;

#[cfg(test)]
mod model_limit_tests {
    use super::*;
    fn configured(dir: &Path) -> ModelStore {
        let mut store = ModelStore::new(dir).unwrap();
        let mut settings = ModelSettings {
            configured: true,
            provider: "custom".into(),
            protocol: PROTOCOL.into(),
            base_url: "https://example.invalid/v1".into(),
            model: "fixture-model".into(),
            verified: true,
            active_profile: "custom".into(),
            ..ModelSettings::default()
        };
        settings.profiles.push(ModelProfileSettings {
            profile: "custom".into(),
            provider: settings.provider.clone(),
            protocol: settings.protocol.clone(),
            configured: true,
            base_url: settings.base_url.clone(),
            model: settings.model.clone(),
            verified: true,
            max_output_tokens: None,
            context_window: None,
        });
        store.persist(&settings).unwrap();
        store.settings = settings;
        store
    }
    #[test]
    fn legacy_limits_are_effective_defaults_and_context_capacity_stays_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let store = configured(dir.path());
        let mut value = serde_json::to_value(&store.settings).unwrap();
        value.as_object_mut().unwrap().remove("maxOutputTokens");
        value.as_object_mut().unwrap().remove("contextWindow");
        for profile in value["profiles"].as_array_mut().unwrap() {
            profile.as_object_mut().unwrap().remove("maxOutputTokens");
            profile.as_object_mut().unwrap().remove("contextWindow");
        }
        crate::app::atomic_json(&store.config_path, &value).unwrap();
        let reopened = ModelStore::new(dir.path()).unwrap();
        assert_eq!(reopened.settings().max_output_tokens, Some(4096));
        assert_eq!(reopened.settings().context_window, None);
        let glm = ModelCredentials::new(
            ModelSettings {
                provider: "glm-pi".into(),
                base_url: "http://127.0.0.1:1/v1".into(),
                model: MODEL_ID.into(),
                ..ModelSettings::default()
            },
            "fixture-key".into(),
        )
        .unwrap();
        assert_eq!(glm.settings.max_output_tokens, Some(32768));
        assert_eq!(glm.settings.context_window, None);
    }
    #[test]
    fn limits_save_without_credentials_preserves_binding_and_verification_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = configured(dir.path());
        let before = store.settings();
        let saved = store.set_limits(8192, Some(32768)).unwrap();
        assert_eq!(saved.max_output_tokens, Some(8192));
        assert_eq!(saved.context_window, Some(32768));
        assert_eq!(
            (
                saved.provider,
                saved.protocol,
                saved.base_url,
                saved.model,
                saved.verified
            ),
            (
                before.provider,
                before.protocol,
                before.base_url,
                before.model,
                before.verified
            )
        );
        let reopened = ModelStore::new(dir.path()).unwrap().settings();
        assert_eq!(reopened.max_output_tokens, Some(8192));
        assert_eq!(reopened.context_window, Some(32768));
        assert!(!std::fs::read_to_string(&store.config_path)
            .unwrap()
            .contains("apiKey"));
        assert_eq!(store.set_limits(8192, None).unwrap().context_window, None);
    }
    #[test]
    fn invalid_limits_do_not_modify_disk_or_memory_and_unconfigured_never_saves() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = configured(dir.path());
        let disk = std::fs::read(&store.config_path).unwrap();
        let memory = serde_json::to_value(store.settings()).unwrap();
        for (max, context) in [
            (0, None),
            (131073, None),
            (1, Some(1023)),
            (1, Some(2000001)),
            (8192, Some(9215)),
        ] {
            assert_eq!(
                store.set_limits(max, context).unwrap_err().code,
                "INVALID_MODEL_LIMITS"
            );
            assert_eq!(std::fs::read(&store.config_path).unwrap(), disk);
            assert_eq!(serde_json::to_value(store.settings()).unwrap(), memory);
        }
        for max in [
            json!(-1),
            json!(1.5),
            json!("8192"),
            json!(null),
            json!(4294967296u64),
        ] {
            assert_eq!(
                parse_limits(&max, None).unwrap_err().code,
                "INVALID_MODEL_LIMITS"
            );
        }
        for context in [json!(-1), json!(3.5), json!("unknown")] {
            assert_eq!(
                parse_limits(&json!(1), Some(&context)).unwrap_err().code,
                "INVALID_MODEL_LIMITS"
            );
        }
        assert_eq!(
            parse_limits(&json!(1), Some(&Value::Null)).unwrap(),
            (1, None)
        );
        assert!(validate_limits(131072, Some(132096)).is_ok());
        let other = tempfile::tempdir().unwrap();
        let mut blank = ModelStore::new(other.path()).unwrap();
        assert_eq!(
            blank.set_limits(8192, None).unwrap_err().code,
            "NOT_CONFIGURED"
        );
        assert!(!blank.config_path.exists());
    }
    #[test]
    fn limits_follow_saved_binding_and_model_change_keeps_budget() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = configured(dir.path());
        store.set_limits(8192, Some(32768)).unwrap();
        let selected = store.set_model("another-fixture-model").unwrap();
        assert_eq!(selected.max_output_tokens, Some(8192));
        let same = store
            .configure_protocol(
                "custom",
                PROTOCOL,
                "https://example.invalid/v1",
                "third-model",
                "",
            )
            .unwrap();
        assert_eq!(same.max_output_tokens, Some(8192));
        assert_eq!(same.context_window, Some(32768));
        assert_eq!(
            store.limits_for_binding("custom", "custom", PROTOCOL, "https://else.invalid/v1"),
            (None, None)
        );
        assert_eq!(
            store.limits_for_binding("custom", "custom", ANTHROPIC, "https://example.invalid/v1"),
            (None, None)
        );
        assert_eq!(
            store.limits_for_binding("custom", "glm-pi", PROTOCOL, "https://example.invalid/v1"),
            (None, None)
        );
        let mut credentials =
            ModelCredentials::new(store.settings(), "fixture-only".into()).unwrap();
        store.set_limits(16384, None).unwrap();
        credentials.override_model("grading-fixture-model").unwrap();
        assert_eq!(credentials.settings.max_output_tokens, Some(8192));
        assert_eq!(credentials.settings.context_window, Some(32768));
    }
    #[tokio::test]
    async fn grading_queued_and_running_block_limit_changes_before_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let e =
            crate::app::Engine::new(dir.path().join("library"), dir.path().join("state")).unwrap();
        for status in ["queued", "running"] {
            let run = serde_json::from_value(json!({"id":"grade_fixture","sessionId":"session_fixture","attemptId":"attempt_fixture", "status":status,"result":null,"error":null,"model":"fixture","updatedAt":"fixture","digest":"fixture"})).unwrap();
            e.grades.lock().unwrap().insert(
                "grade_fixture".into(),
                crate::grading::ManagedGrade {
                    run,
                    cancel: Arc::new(AtomicBool::new(false)),
                },
            );
            assert_eq!(
                e.set_model_limits(8192, None).await.unwrap_err().code,
                "BUSY"
            );
        }
        e.grades.lock().unwrap().clear();
        assert_eq!(
            e.set_model_limits(8192, None).await.unwrap_err().code,
            "NOT_CONFIGURED"
        );
        assert!(!dir.path().join("state/model-settings.json").exists());
    }
}

#[cfg(test)]
#[path = "model_auto_context_tests.rs"]
mod automatic_context_wire_tests;

#[cfg(test)]
#[path = "model_native_session_tests.rs"]
mod native_session_tests;
