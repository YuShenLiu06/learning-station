//! Explicit local bot lifecycle. No CLI migration, secret discovery, auto-start or automatic send.
use crate::{
    app::atomic_json,
    feishu::{DeliveryOutcome, DeliveryState, IdentityStatus},
    feishu_ws::{BotConfig, BotSnapshot, BotWorker},
    reminders::{Ledger, Notification},
    types::*,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

const SERVICE: &str = "com.yushen.learning-station.feishu-bot";
static RESOURCE: OnceLock<PathBuf> = OnceLock::new();
pub fn set_resource_dir(path: PathBuf) -> Result<()> {
    RESOURCE
        .set(path)
        .map_err(|_| CoreError::new("INVALID_STATE", "机器人资源目录已设置。"))
}
fn resource() -> PathBuf {
    RESOURCE
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tools/feishu-bot"))
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Saved {
    config: BotConfig,
    secret_stored: bool,
    #[serde(default)]
    delivery_since: Option<String>,
}
pub struct BotManager {
    dir: PathBuf,
    worker: Mutex<Option<Arc<BotWorker>>>,
    changes: Mutex<()>,
    last_error: Mutex<Option<String>>,
}
impl BotManager {
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            dir: state_dir.join("feishu-bot"),
            worker: Mutex::new(None),
            changes: Mutex::new(()),
            last_error: Mutex::new(None),
        }
    }
    fn saved(&self) -> Result<Option<Saved>> {
        match fs::read(self.dir.join("settings.json")) {
            Ok(bytes) => {
                let s: Saved = serde_json::from_slice(&bytes)?;
                validate(&s.config)?;
                Ok(Some(s))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(CoreError::io(e)),
        }
    }
    fn account(&self, config: &BotConfig) -> String {
        hash(
            &json!([
                self.dir.to_string_lossy(),
                config.app_id,
                config.owner_open_id,
                config.chat_id
            ])
            .to_string(),
        )
    }
    fn key(&self, config: &BotConfig) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, &self.account(config)).map_err(|_| secret_error())
    }
    pub fn settings(&self) -> Result<Value> {
        let stored = self.saved()?;
        let root = resource();
        let available = crate::model::node_path().is_file()
            && root.join("worker.mjs").is_file()
            && root
                .join("node_modules/@larksuiteoapi/node-sdk/package.json")
                .is_file();
        Ok(
            json!({"configured":stored.is_some(),"appId":stored.as_ref().map(|s|s.config.app_id.as_str()).unwrap_or(""),"ownerOpenId":stored.as_ref().map(|s|s.config.owner_open_id.as_str()).unwrap_or(""),"chatId":stored.as_ref().map(|s|s.config.chat_id.as_str()).unwrap_or(""),"receiveEnabled":stored.as_ref().is_some_and(|s|s.config.receive_enabled),"deliveryEnabled":stored.as_ref().is_some_and(|s|s.config.delivery_enabled),"secretStored":stored.as_ref().is_some_and(|s|s.secret_stored),"runtimeAvailable":available,"runtimeReason":if available{"Node与官方SDK资源已在位；尚未证明已连接飞书。"}else{"固定Node或受审SDK资源缺失；未安装服务、未请求授权。"}}),
        )
    }
    pub fn configure(&self, config: BotConfig, secret: Option<String>) -> Result<Value> {
        let _g = self.changes.lock().map_err(|_| busy())?;
        validate(&config)?;
        if self
            .worker
            .lock()
            .map_err(|_| busy())?
            .as_ref()
            .is_some_and(|w| !w.snapshot().process_exited)
        {
            return Err(CoreError::new("BUSY", "请先停止机器人连接再修改配置。"));
        }
        let old = self.saved()?;
        let same = old.as_ref().is_some_and(|s| {
            s.config.app_id == config.app_id
                && s.config.owner_open_id == config.owner_open_id
                && s.config.chat_id == config.chat_id
                && s.secret_stored
        });
        let supplied = secret.filter(|s| !s.is_empty());
        if !same && supplied.is_none() {
            return Err(CoreError::new(
                "AUTH",
                "首次配置或变更应用/目标时，请在本机输入新App Secret。",
            ));
        }
        if let Some(s) = supplied {
            if s.len() > 16384 || s.chars().any(|c| c.is_control() || c.is_whitespace()) {
                return Err(secret_error());
            }
            self.key(&config)?
                .set_password(&s)
                .map_err(|_| secret_error())?;
        }
        let delivery_since = if config.delivery_enabled {
            old.as_ref()
                .filter(|s| s.config.delivery_enabled)
                .and_then(|s| s.delivery_since.clone())
                .or_else(|| Some(timestamp()))
        } else {
            None
        };
        atomic_json(
            &self.dir.join("settings.json"),
            &Saved {
                config,
                secret_stored: true,
                delivery_since,
            },
        )?;
        self.settings()
    }
    pub fn start(&self) -> Result<BotSnapshot> {
        let _g = self.changes.lock().map_err(|_| busy())?;
        if let Some(w) = self.worker.lock().map_err(|_| busy())?.as_ref() {
            if !w.snapshot().process_exited {
                return Ok(display_snapshot(w.snapshot()));
            }
        }
        let saved = self.saved()?.ok_or_else(|| {
            CoreError::new("CHANNEL_DISABLED", "请先保存本人专用应用和唯一聊天配置。")
        })?;
        if !saved.config.receive_enabled && !saved.config.delivery_enabled {
            return Err(CoreError::new(
                "CHANNEL_DISABLED",
                "接收和发送均关闭，请先明确启用需要的能力。",
            ));
        }
        if self.settings()?["runtimeAvailable"] != true {
            return Err(CoreError::new(
                "BOT_RUNTIME_UNAVAILABLE",
                "固定Node或SDK资源缺失，未连接飞书。",
            ));
        }
        let secret = self
            .key(&saved.config)?
            .get_password()
            .map_err(|_| secret_error())?;
        let worker = Arc::new(BotWorker::start(
            &crate::model::node_path(),
            &resource().join("worker.mjs"),
            &self.dir.join("runtime"),
            saved.config,
            secret,
        )?);
        let snapshot = display_snapshot(worker.snapshot());
        *self.worker.lock().map_err(|_| busy())? = Some(worker);
        Ok(snapshot)
    }
    pub fn stop(&self) -> Result<BotSnapshot> {
        let _g = self.changes.lock().map_err(|_| busy())?;
        if let Some(w) = self.worker.lock().map_err(|_| busy())?.take() {
            w.stop();
            return Ok(display_snapshot(w.snapshot()));
        }
        self.snapshot()
    }
    pub fn snapshot(&self) -> Result<BotSnapshot> {
        if let Some(w) = self.worker.lock().map_err(|_| busy())?.as_ref() {
            let mut s = w.snapshot();
            if let Some(error) = self.last_error.lock().map_err(|_| busy())?.as_ref() {
                s.reason = error.clone();
            }
            return Ok(display_snapshot(s));
        }
        let configured = self.saved()?.is_some();
        Ok(BotSnapshot {
            state: if configured {
                "stopped"
            } else {
                "unconfigured"
            }
            .into(),
            reason: if configured {
                "尚未启动机器人；保存配置不会连接或发送。"
            } else {
                "未配置本人专用飞书机器人；站内提醒可继续使用。"
            }
            .into(),
            counts: json!({}),
            process_exited: true,
            output_closed: true,
        })
    }
    pub fn identity(&self) -> IdentityStatus {
        match self.worker.lock() {
            Ok(w) => w
                .as_ref()
                .map(|w| w.identity_status())
                .unwrap_or(IdentityStatus {
                    ready: false,
                    reason: "机器人尚未启动；未使用旧CLI通道。".into(),
                }),
            Err(_) => IdentityStatus {
                ready: false,
                reason: "机器人状态不可用。".into(),
            },
        }
    }
    pub fn send(&self, key: &str, text: &str) -> DeliveryOutcome {
        match self.worker.lock().ok().and_then(|w| w.clone()) {
            Some(w) => w.send(key, text),
            None => DeliveryOutcome {
                status: DeliveryState::Blocked,
                message_id: None,
                reason: "机器人未启动，未发送消息。".into(),
            },
        }
    }
    pub fn dispatch(&self, ledger: &Ledger) -> Result<Vec<Notification>> {
        use crate::reminders::NotificationChannel;
        let Some(w) = self.worker.lock().map_err(|_| busy())?.clone() else {
            return Ok(vec![]);
        };
        let channel = crate::feishu_ws::BotNotificationChannel(w.clone());
        if !matches!(
            channel.availability(),
            crate::reminders::ChannelAvailability::Enabled
        ) {
            return Ok(vec![]);
        }
        let saved = self.saved()?.ok_or_else(busy)?;
        let Some(since) = saved
            .delivery_since
            .filter(|_| saved.config.delivery_enabled)
        else {
            return Ok(vec![]);
        };
        let mut result = vec![];
        for n in ledger.pending_deliveries(&since)? {
            if !ledger.reserve_delivery(&n.id)? {
                continue;
            }
            let receipt = match channel.deliver(&n) {
                Ok(r) => DeliveryOutcome {
                    status: DeliveryState::Sent,
                    message_id: Some(r.external_id),
                    reason: "飞书已确认发送。".into(),
                },
                Err(e) => DeliveryOutcome {
                    status: if e.code == "CHANNEL_UNKNOWN" {
                        DeliveryState::Unknown
                    } else if e.code == "CHANNEL_DISABLED" {
                        DeliveryState::Blocked
                    } else {
                        DeliveryState::Failed
                    },
                    message_id: None,
                    reason: e.message,
                },
            };
            ledger.finish_delivery(&n.id, &receipt)?;
            let mut updated = n;
            updated.delivery_status = match receipt.status {
                DeliveryState::Sent if receipt.message_id.is_some() => "sent",
                DeliveryState::Blocked | DeliveryState::Failed => "failed",
                _ => "unknown",
            }
            .into();
            result.push(updated);
        }
        Ok(result)
    }
    pub fn poll(&self, ledger: &Ledger) -> Result<Vec<Notification>> {
        let Some(w) = self.worker.lock().map_err(|_| busy())?.clone() else {
            return Ok(vec![]);
        };
        let mut delivered = vec![];
        w.pull_pending()?;
        for incoming in w.take_inbound() {
            match ledger.import_feishu_message(
                &incoming.ticket,
                &incoming.message_id,
                &incoming.text,
            ) {
                Ok((notification, is_new)) => {
                    w.ack_persisted(&incoming.ticket)?;
                    if is_new {
                        delivered.push(notification)
                    }
                }
                Err(e) => {
                    *self.last_error.lock().map_err(|_| busy())? =
                        Some("接收消息未能写入站内收件箱，未确认接收；原记录保留等待恢复。".into());
                    return Err(e);
                }
            }
        }
        Ok(delivered)
    }
}
fn display_snapshot(mut s: BotSnapshot) -> BotSnapshot {
    let message = match s.reason.as_str() {
        "awaiting_worker" => "正在启动机器人运行时，尚未连接。",
        "websocket_handshake_confirmed" => "飞书事件长连接已建立；发送仍以HTTP消息回执为准。",
        "websocket_reconnected" => "飞书事件长连接已恢复。",
        "websocket_disconnected" => "事件连接已断开，尚未确认恢复。",
        "sdk_backoff" => "SDK正在按退避策略重连，已有本机记录保留。",
        "sdk_unavailable" | "sdk_version_unsupported" | "node_version_unsupported" => {
            "机器人运行时版本或SDK资源不可用，未自动安装。"
        }
        "websocket_connection_failed" => "事件连接失败，请核对本人应用配置与飞书权限。",
        "configured" if s.state == "receive_disabled" => {
            "事件接收未启用；HTTP发送需独立启用和确认回执。"
        }
        "configured" => "配置已读取，正在等待事件连接。",
        "shutdown_requested" => "正在停止机器人。",
        "worker_exiting" => "机器人已退出。",
        "worker_output_closed" | "runtime_failure" => {
            "机器人运行中断，尚未确认的发送不会自动重发。"
        }
        "inbound_persistence_failed" => "事件未能持久保存，尚未确认接收。",
        "invalid_configuration_or_protocol" | "invalid_protocol" => {
            "机器人配置或本机通信协议无效，已停止。"
        }
        _ => return s,
    };
    s.reason = message.into();
    s
}
fn busy() -> CoreError {
    CoreError::new("BUSY", "机器人配置暂忙，请稍后重试。")
}
fn secret_error() -> CoreError {
    CoreError::new(
        "KEYCHAIN",
        "无法保存或读取学习站专用机器人凭据；请检查本机Keychain权限，未读取其他应用凭据。",
    )
}
fn validate(c: &BotConfig) -> Result<()> {
    for (s, p) in [
        (&c.app_id, "cli_"),
        (&c.owner_open_id, "ou_"),
        (&c.chat_id, "oc_"),
    ] {
        if !s.starts_with(p)
            || s.len() <= p.len()
            || s.len() > 128
            || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(CoreError::new(
                "INVALID_INPUT",
                "请填写有效App ID、本人open_id和唯一聊天chat_id。",
            ));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_is_disabled_without_credentials_or_cli() {
        let t = tempfile::tempdir().unwrap();
        let m = BotManager::new(t.path().to_path_buf());
        assert_eq!(m.settings().unwrap()["configured"], false);
        assert_eq!(m.snapshot().unwrap().state, "unconfigured");
        assert!(!m.identity().ready);
        assert_eq!(
            m.send("test-00000001", "test").status,
            DeliveryState::Blocked
        );
    }
    #[test]
    fn invalid_and_changed_bindings_without_secret_never_touch_keychain() {
        let t = tempfile::tempdir().unwrap();
        let m = BotManager::new(t.path().to_path_buf());
        let mut c = BotConfig {
            app_id: "cli_test".into(),
            owner_open_id: "ou_owner".into(),
            chat_id: "oc_chat".into(),
            receive_enabled: false,
            delivery_enabled: false,
        };
        assert_eq!(m.configure(c.clone(), None).unwrap_err().code, "AUTH");
        c.chat_id = "oc_../outside".into();
        assert_eq!(m.configure(c, None).unwrap_err().code, "INVALID_INPUT");
        assert!(!m.dir.join("settings.json").exists());
    }
}
