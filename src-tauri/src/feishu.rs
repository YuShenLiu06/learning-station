//! Explicit, owner-only Feishu CLI adapter. This module never schedules work,
//! enables delivery, retries a send, logs credentials, or opens an OAuth flow.

use crate::types::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::process::Command;

const CLI: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../tools/local/lark-cli");
const VERIFIED_CHANNEL: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../.state/verified-feishu-channel.json");

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryState {
    Sent,
    Blocked,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryOutcome {
    pub status: DeliveryState,
    pub message_id: Option<String>,
    pub reason: String,
}

impl DeliveryOutcome {
    fn new(status: DeliveryState, reason: &str) -> Self {
        Self {
            status,
            message_id: None,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityStatus {
    pub ready: bool,
    pub reason: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifiedChannel {
    app_id: String,
    recipient_user_open_id: String,
}

/// The only recipient comes from the existing verified local channel file.
/// Do not expose this struct's owner or constructor inputs as UI recipient fields.
#[derive(Clone)]
pub struct FeishuCliChannel {
    app_id: String,
    owner_open_id: String,
}

impl FeishuCliChannel {
    pub fn from_verified_file() -> Result<Self> {
        let bytes = std::fs::read(VERIFIED_CHANNEL)
            .map_err(|_| CoreError::new("CHANNEL_DISABLED", "未找到已验证的飞书本人通道。"))?;
        if bytes.len() > 64 * 1024 {
            return Err(CoreError::new(
                "CHANNEL_DISABLED",
                "已验证的飞书通道配置无效。",
            ));
        }
        let channel: VerifiedChannel = serde_json::from_slice(&bytes)
            .map_err(|_| CoreError::new("CHANNEL_DISABLED", "已验证的飞书通道配置无效。"))?;
        if !valid_identifier(&channel.app_id, "cli_")
            || !valid_identifier(&channel.recipient_user_open_id, "ou_")
        {
            return Err(CoreError::new(
                "CHANNEL_DISABLED",
                "已验证的飞书本人身份配置无效。",
            ));
        }
        Ok(Self {
            app_id: channel.app_id,
            owner_open_id: channel.recipient_user_open_id,
        })
    }

    /// A read-only check using the verified app profile. It never logs in.
    pub async fn check_identity(&self) -> IdentityStatus {
        let args = vec![
            "--profile".into(),
            self.app_id.clone(),
            "whoami".into(),
            "--as".into(),
            "bot".into(),
        ];
        match run_cli(&args, Duration::from_secs(10)).await {
            Invocation::Completed(output) => {
                let ready = output.status.success()
                    && serde_json::from_slice::<Value>(&output.stdout)
                        .ok()
                        .and_then(|value| value.get("ok").and_then(Value::as_bool))
                        == Some(true);
                IdentityStatus {
                    ready,
                    reason: if ready {
                        "飞书机器人身份已验证；发送仍需显式启用。"
                    } else {
                        "无法验证飞书机器人身份；未发送消息，请先修复本机 CLI 授权。"
                    }
                    .into(),
                }
            }
            Invocation::Unavailable => IdentityStatus {
                ready: false,
                reason: "本机飞书 CLI 不可用；未发送消息。".into(),
            },
            Invocation::TimedOut | Invocation::Unconfirmed => IdentityStatus {
                ready: false,
                reason: "飞书身份验证超时；未尝试发送。".into(),
            },
        }
    }

    /// Call only after explicit user enablement. The coordinator must first
    /// commit a unique sourceId/version/dueTime job and its fixed idempotency key.
    /// Persist `Unknown` without retrying it automatically. No queue is enabled here.
    pub async fn send_explicitly_enabled(
        &self,
        text: &str,
        idempotency_key: &str,
        enabled_by_user: bool,
    ) -> DeliveryOutcome {
        if !enabled_by_user {
            return DeliveryOutcome::new(DeliveryState::Blocked, "飞书发送尚未启用；未发送消息。");
        }
        if text.trim().is_empty() || text.len() > 16_000 || text.contains('\0') {
            return DeliveryOutcome::new(
                DeliveryState::Blocked,
                "飞书消息为空或过长；未发送消息。",
            );
        }
        if idempotency_key.is_empty()
            || idempotency_key.len() > 128
            || !idempotency_key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return DeliveryOutcome::new(DeliveryState::Blocked, "发送幂等键无效；未发送消息。");
        }
        let identity = self.check_identity().await;
        if !identity.ready {
            return DeliveryOutcome::new(DeliveryState::Blocked, &identity.reason);
        }
        let args = vec![
            "--profile".into(),
            self.app_id.clone(),
            "im".into(),
            "+messages-send".into(),
            "--as".into(),
            "bot".into(),
            "--user-id".into(),
            self.owner_open_id.clone(),
            "--text".into(),
            text.into(),
            "--idempotency-key".into(),
            idempotency_key.into(),
            "--json".into(),
        ];
        match run_cli(&args, Duration::from_secs(20)).await {
            Invocation::Completed(output) => classify_send(output.status.success(), &output.stdout),
            Invocation::Unavailable => {
                DeliveryOutcome::new(DeliveryState::Blocked, "无法启动本机飞书 CLI；未尝试发送。")
            }
            Invocation::TimedOut | Invocation::Unconfirmed => DeliveryOutcome::new(
                DeliveryState::Unknown,
                "发送超时，结果未知；请先核实飞书消息，系统不会自动重发。",
            ),
        }
    }
}

enum Invocation {
    Completed(Output),
    Unavailable,
    TimedOut,
    Unconfirmed,
}

async fn run_cli(args: &[String], duration: Duration) -> Invocation {
    let mut command = Command::new(CLI);
    command
        .args(args)
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return Invocation::Unavailable,
    };
    // Once spawned, any wait/read failure might follow a successful send.
    match tokio::time::timeout(duration, child.wait_with_output()).await {
        Ok(Ok(output)) => Invocation::Completed(output),
        Ok(Err(_)) => Invocation::Unconfirmed,
        Err(_) => Invocation::TimedOut,
    }
}

fn classify_send(successful_exit: bool, stdout: &[u8]) -> DeliveryOutcome {
    let Ok(value) = serde_json::from_slice::<Value>(stdout) else {
        return DeliveryOutcome::new(
            DeliveryState::Unknown,
            "无法确认发送结果；请先核实飞书消息，系统不会自动重发。",
        );
    };
    if successful_exit && value.get("ok").and_then(Value::as_bool) == Some(true) {
        if let Some(message_id) = value
            .pointer("/data/message_id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
        {
            return DeliveryOutcome {
                status: DeliveryState::Sent,
                message_id: Some(message_id.into()),
                reason: "飞书已确认发送。".into(),
            };
        }
    }
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        // Only inspect machine-readable error identifiers. Do not echo CLI text
        // or response bodies, which may contain credentials or user content.
        let code = value
            .pointer("/error/code")
            .or_else(|| value.get("code"))
            .map(|code| match code {
                Value::String(code) => code.to_ascii_lowercase(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        let category = value
            .pointer("/error/category")
            .or_else(|| value.pointer("/error/type"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let identifier = format!("{code} {category}");
        if ["timeout", "network", "transport", "interrupt", "cancel"]
            .iter()
            .any(|term| identifier.contains(term))
        {
            return DeliveryOutcome::new(
                DeliveryState::Unknown,
                "发送的网络结果未知；请先核实飞书消息，系统不会自动重发。",
            );
        }
        if [
            "auth",
            "credential",
            "token",
            "login",
            "permission",
            "forbidden",
            "999916",
        ]
        .iter()
        .any(|term| identifier.contains(term))
        {
            return DeliveryOutcome::new(
                DeliveryState::Blocked,
                "飞书身份或权限不可用；请先修复本机授权。",
            );
        }
        if !code.is_empty() {
            return DeliveryOutcome::new(
                DeliveryState::Failed,
                "飞书明确拒绝了发送请求；请检查消息与通道配置。",
            );
        }
    }
    DeliveryOutcome::new(
        DeliveryState::Unknown,
        "飞书未返回可确认的发送凭据；请先核实消息，系统不会自动重发。",
    )
}

fn valid_identifier(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() > prefix.len()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirms_sent_only_with_success_exit_ok_and_message_id() {
        let confirmed = br#"{"ok":true,"data":{"message_id":"om_test"}}"#;
        assert_eq!(classify_send(true, confirmed).status, DeliveryState::Sent);
        assert_eq!(
            classify_send(false, confirmed).status,
            DeliveryState::Unknown
        );
        assert_eq!(
            classify_send(true, br#"{"ok":true,"data":{}}"#).status,
            DeliveryState::Unknown
        );
        assert_eq!(
            classify_send(true, b"not json").status,
            DeliveryState::Unknown
        );
    }

    #[test]
    fn distinguishes_rejection_authentication_and_ambiguous_transport() {
        assert_eq!(
            classify_send(false, br#"{"ok":false,"error":{"code":"INVALID_MESSAGE"}}"#).status,
            DeliveryState::Failed
        );
        assert_eq!(
            classify_send(false, br#"{"ok":false,"error":{"code":"AUTH_REQUIRED"}}"#).status,
            DeliveryState::Blocked
        );
        assert_eq!(
            classify_send(false, br#"{"ok":false,"error":{"code":"NETWORK_TIMEOUT"}}"#).status,
            DeliveryState::Unknown
        );
        assert_eq!(
            classify_send(false, br#"{"ok":false}"#).status,
            DeliveryState::Unknown
        );
    }

    #[tokio::test]
    async fn disabled_channel_never_invokes_cli() {
        let channel = FeishuCliChannel {
            app_id: "invalid_on_purpose".into(),
            owner_open_id: "invalid_on_purpose".into(),
        };
        let result = channel
            .send_explicitly_enabled("test", "fixed_key", false)
            .await;
        assert_eq!(result.status, DeliveryState::Blocked);
    }
}
