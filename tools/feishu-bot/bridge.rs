//! Proposed `src-tauri/src/feishu_ws.rs`; only std + existing serde dependencies.
//! Caller supplies trusted packaged runtime paths and a freshly read Keychain secret.
//! Never expose `start` parameters, raw protocol or `send` text as model tools.
use crate::feishu::{DeliveryOutcome, DeliveryState, IdentityStatus};
use crate::reminders::{ChannelAvailability, DeliveryReceipt, Notification, NotificationChannel};
use crate::types::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const MAX_FRAME: u64 = 65536;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BotConfig {
    pub app_id: String,
    pub owner_open_id: String,
    pub chat_id: String,
    pub receive_enabled: bool,
    pub delivery_enabled: bool,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotSnapshot {
    pub state: String,
    pub reason: String,
    pub counts: Value,
    pub process_exited: bool,
    pub output_closed: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inbound {
    pub ticket: String,
    pub message_id: String,
    pub chat_id: String,
    pub sender_open_id: String,
    pub text: String,
}

#[derive(Default)]
struct Shared {
    snapshot: BotSnapshot,
    inbound: VecDeque<Inbound>,
    waiting_for: Option<String>,
    delivery: Option<DeliveryOutcome>,
}

pub struct BotWorker {
    child: Mutex<Child>,
    input: Mutex<Option<ChildStdin>>,
    shared: Arc<(Mutex<Shared>, Condvar)>,
    send_lock: Mutex<()>,
    config: BotConfig,
}

fn error() -> CoreError {
    CoreError::new(
        "CHANNEL_DISABLED",
        "飞书机器人运行时不可用；未自动安装或发起授权。",
    )
}

fn unknown() -> DeliveryOutcome {
    DeliveryOutcome {
        status: DeliveryState::Unknown,
        message_id: None,
        reason: "发送结果未知；请先核实消息，系统不会自动重发。".into(),
    }
}

impl BotWorker {
    /// `node`, `script` must come from application packaging, never a frontend
    /// path picker or PATH lookup. `state_dir` is a dedicated private runtime dir.
    /// secret is transported over a private anonymous pipe; not argv/env/files.
    pub fn start(
        node: &Path,
        script: &Path,
        state_dir: &Path,
        config: BotConfig,
        secret: String,
    ) -> Result<Self> {
        if !node.is_absolute() || !script.is_absolute() || !state_dir.is_absolute() {
            return Err(error());
        }
        let mut command = Command::new(node);
        command
            .arg("--disable-warning=ExperimentalWarning")
            .arg(script)
            .arg("--state-dir")
            .arg(state_dir)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        crate::windows::prepare_child(&mut command);
        let mut child = command.spawn()
            .map_err(|_| error())?;
        let mut input = child.stdin.take().ok_or_else(error)?;
        let output = child.stdout.take().ok_or_else(error)?;
        let mut settings = serde_json::to_value(&config).map_err(|_| error())?;
        settings["appSecret"] = Value::String(secret);
        let mut init =
            serde_json::to_vec(&json!({"op":"init", "config":settings})).map_err(|_| error())?;
        init.push(b'\n');
        let write = input.write_all(&init).and_then(|_| input.flush());
        // Best effort reduces one extra copy's lifetime; does not claim secure erasure.
        init.fill(0);
        drop(settings);
        if write.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error());
        }
        let shared = Arc::new((
            Mutex::new(Shared {
                snapshot: BotSnapshot {
                    state: "starting".into(),
                    reason: "awaiting_worker".into(),
                    ..BotSnapshot::default()
                },
                ..Shared::default()
            }),
            Condvar::new(),
        ));
        let reader_state = Arc::clone(&shared);
        let expected = config.clone();
        thread::spawn(move || {
            let mut output = BufReader::new(output);
            loop {
                let mut bytes = Vec::new();
                let read = (&mut output)
                    .take(MAX_FRAME + 1)
                    .read_until(b'\n', &mut bytes);
                if read.is_err()
                    || bytes.is_empty()
                    || bytes.len() as u64 > MAX_FRAME
                    || bytes.last() != Some(&b'\n')
                {
                    break;
                }
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    break;
                };
                if value.get("protocol").and_then(Value::as_u64) != Some(1) {
                    break;
                }
                let (lock, wake) = &*reader_state;
                let Ok(mut state) = lock.lock() else {
                    return;
                };
                match value.get("type").and_then(Value::as_str) {
                    Some("status") => {
                        let status = value
                            .get("state")
                            .and_then(Value::as_str)
                            .unwrap_or("failed");
                        if [
                            "starting",
                            "blocked",
                            "connecting",
                            "connected",
                            "disconnected",
                            "reconnecting",
                            "failed",
                            "receive_disabled",
                            "stopping",
                            "stopped",
                        ]
                        .contains(&status)
                        {
                            state.snapshot.state = status.into();
                            // Keep capability failure visible across the final stopped frame.
                            if !["stopping", "stopped"].contains(&status)
                                || state.snapshot.reason.is_empty()
                            {
                                state.snapshot.reason = value
                                    .get("reason")
                                    .and_then(Value::as_str)
                                    .unwrap_or("runtime_failure")
                                    .into();
                            }
                            state.snapshot.counts =
                                value.get("counts").cloned().unwrap_or(Value::Null);
                        }
                    }
                    Some("delivery")
                        if value.get("requestId").and_then(Value::as_str)
                            == state.waiting_for.as_deref() =>
                    {
                        if let Some(result) = value.get("outcome") {
                            if let Ok(receipt) =
                                serde_json::from_value::<DeliveryOutcome>(result.clone())
                            {
                                state.delivery = Some(normalize(receipt));
                            }
                        }
                    }
                    Some("inbound") if expected.receive_enabled => {
                        if let Ok(item) = serde_json::from_value::<Inbound>(value.clone()) {
                            if item.chat_id == expected.chat_id
                                && item.sender_open_id == expected.owner_open_id
                                && item.text.len() <= 16000
                                && valid_ticket(&item.ticket)
                                && !item.message_id.is_empty()
                                && state.inbound.len() < 100
                            {
                                // Queue overflow remains unacked in worker inbox; pull_pending replays it.
                                if !state.inbound.iter().any(|old| old.ticket == item.ticket) {
                                    state.inbound.push_back(item);
                                }
                            }
                        }
                    }
                    _ => {}
                }
                wake.notify_all();
            }
            let (lock, wake) = &*reader_state;
            if let Ok(mut state) = lock.lock() {
                state.snapshot.output_closed = true;
                if !["stopped", "blocked", "failed"].contains(&state.snapshot.state.as_str()) {
                    state.snapshot.state = "failed".into();
                    state.snapshot.reason = "worker_output_closed".into();
                }
                wake.notify_all();
            }
        });
        Ok(Self {
            child: Mutex::new(child),
            input: Mutex::new(Some(input)),
            shared,
            send_lock: Mutex::new(()),
            config,
        })
    }

    fn command(&self, value: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&value).map_err(|_| error())?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_FRAME {
            return Err(error());
        }
        let mut guard = self.input.lock().map_err(|_| error())?;
        let input = guard.as_mut().ok_or_else(error)?;
        input
            .write_all(&bytes)
            .and_then(|_| input.flush())
            .map_err(|_| error())
    }

    pub fn snapshot(&self) -> BotSnapshot {
        let mut snapshot =
            self.shared
                .0
                .lock()
                .map(|s| s.snapshot.clone())
                .unwrap_or(BotSnapshot {
                    state: "failed".into(),
                    reason: "runtime_state_unavailable".into(),
                    ..BotSnapshot::default()
                });
        snapshot.process_exited = self
            .child
            .lock()
            .ok()
            .and_then(|mut c| c.try_wait().ok())
            .flatten()
            .is_some();
        snapshot
    }

    pub fn identity_status(&self) -> IdentityStatus {
        let status = self.snapshot();
        let ready = status.state == "connected" && !status.output_closed && !status.process_exited;
        IdentityStatus {
            ready,
            reason: if ready {
                "飞书长连接已建立；发送需独立启用并取得HTTP消息回执。".into()
            } else {
                "飞书长连接尚未建立；请查看机器人配置与连接状态。".into()
            },
        }
    }

    /// Existing check/test IPC calls should use spawn_blocking for this method.
    /// Caller must ALSO keep the existing Rust durable pre-send reservation:
    /// a crash before the worker commits must not cause the host to auto-retry.
    pub fn send(&self, request_id: &str, text: &str) -> DeliveryOutcome {
        if !self.config.delivery_enabled
            || request_id.len() < 8
            || request_id.len() > 128
            || !request_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            || text.trim().is_empty()
            || text.len() > 16000
            || text.contains('\0')
        {
            return DeliveryOutcome {
                status: DeliveryState::Blocked,
                message_id: None,
                reason: "机器人发送未启用或请求无效。".into(),
            };
        }
        let Ok(_serial) = self.send_lock.lock() else {
            return unknown();
        };
        let (lock, wake) = &*self.shared;
        if let Ok(mut state) = lock.lock() {
            if state.snapshot.output_closed {
                return DeliveryOutcome {
                    status: DeliveryState::Blocked,
                    message_id: None,
                    reason: "机器人运行时未启动；未尝试发送。".into(),
                };
            }
            state.waiting_for = Some(request_id.into());
            state.delivery = None;
        } else {
            return unknown();
        }
        if self
            .command(json!({"op":"send", "requestId":request_id, "text":text}))
            .is_err()
        {
            return unknown();
        }
        let Ok(state) = lock.lock() else {
            return unknown();
        };
        match wake.wait_timeout_while(state, Duration::from_secs(30), |s| {
            s.delivery.is_none() && !s.snapshot.output_closed
        }) {
            Ok((mut state, _)) => {
                state.waiting_for = None;
                state.delivery.take().unwrap_or_else(unknown)
            }
            Err(_) => unknown(),
        }
    }

    pub fn take_inbound(&self) -> Vec<Inbound> {
        self.shared
            .0
            .lock()
            .map(|mut s| s.inbound.drain(..).collect())
            .unwrap_or_default()
    }
    pub fn pull_pending(&self) -> Result<()> {
        self.command(json!({"op":"pending"}))
    }
    /// Only after Rust committed the same ticket/message into its own inbox.
    pub fn ack_persisted(&self, ticket: &str) -> Result<()> {
        if !valid_ticket(ticket) {
            return Err(error());
        }
        self.command(json!({"op":"ack", "ticket":ticket}))
    }
    pub fn stop(&self) {
        let _ = self.command(json!({"op":"stop"}));
        if let Ok(mut input) = self.input.lock() {
            input.take();
        }
        if let Ok(mut child) = self.child.lock() {
            let end = Instant::now() + Duration::from_secs(2);
            loop {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                if Instant::now() >= end {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

impl Drop for BotWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn valid_ticket(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn normalize(value: DeliveryOutcome) -> DeliveryOutcome {
    if value.status == DeliveryState::Sent
        && value
            .message_id
            .as_deref()
            .is_none_or(|id| id.trim().is_empty())
    {
        unknown()
    } else {
        value
    }
}

/// Public notification interface; `tick` remains storage-only. The coordinator
/// must persist pre-send Unknown and explicitly opt into this channel.
pub struct BotNotificationChannel(pub Arc<BotWorker>);
impl NotificationChannel for BotNotificationChannel {
    fn name(&self) -> &str {
        "feishu"
    }
    fn availability(&self) -> ChannelAvailability {
        let status = self.0.snapshot();
        if self.0.config.delivery_enabled
            && !status.process_exited
            && !status.output_closed
            && [
                "receive_disabled",
                "connecting",
                "connected",
                "disconnected",
                "reconnecting",
            ]
            .contains(&status.state.as_str())
        {
            ChannelAvailability::Enabled
        } else {
            ChannelAvailability::Disabled {
                reason: "飞书机器人发送未启用或运行时不可用。".into(),
            }
        }
    }
    fn deliver(&self, notification: &Notification) -> Result<DeliveryReceipt> {
        let result = self.0.send(
            &notification.id,
            &crate::reminders::due_notification_text(notification),
        );
        match result.status {
            DeliveryState::Sent => Ok(DeliveryReceipt {
                external_id: result.message_id.ok_or_else(error)?,
            }),
            DeliveryState::Unknown => Err(CoreError::new(
                "CHANNEL_UNKNOWN",
                "发送结果未知，禁止自动重发。",
            )),
            DeliveryState::Blocked => Err(error()),
            DeliveryState::Failed => Err(CoreError::new(
                "CHANNEL_FAILED",
                "飞书消息接口拒绝本次发送。",
            )),
        }
    }
}
