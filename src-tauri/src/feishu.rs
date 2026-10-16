//! Explicit, owner-only Feishu CLI adapter. This module never schedules work,
//! enables delivery, retries a send, logs credentials, or opens an OAuth flow.

use crate::types::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
        let bytes =
            read_bounded_file(Path::new(VERIFIED_CHANNEL), 64 * 1024, false).map_err(|_| {
                CoreError::new(
                    "CHANNEL_DISABLED",
                    "已验证的飞书本人通道缺失或不可安全读取。",
                )
            })?;
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

const TEST_MESSAGE: &str = "学习站测试通知：这是一次由你主动触发的本机连接测试。";
const MAX_OUTBOX_BYTES: usize = 16 * 1024;
static TEST_NOTIFICATION_LOCK: Mutex<()> = Mutex::const_new(());

/// A read-only identity check for an explicit user action. It never logs in or
/// sends a message, and unavailable CLI/configuration always reports false.
pub async fn check_status() -> IdentityStatus {
    match FeishuCliChannel::from_verified_file() {
        Ok(channel) => channel.check_identity().await,
        Err(_) => IdentityStatus {
            ready: false,
            reason: "已验证的飞书本人通道不可用；未发送消息。".into(),
        },
    }
}

/// Send only the fixed connection-test text after an explicit user click. The
/// persisted reservation is Unknown before any CLI invocation. Reusing a request
/// ID returns that record even after a crash, so ambiguous delivery is never retried.
pub async fn test_notification(state_dir: &Path, request_id: &str) -> Result<DeliveryOutcome> {
    test_notification_with(state_dir, request_id, |text, key| async move {
        match FeishuCliChannel::from_verified_file() {
            Ok(channel) => channel.send_explicitly_enabled(&text, &key, true).await,
            Err(_) => DeliveryOutcome::new(
                DeliveryState::Blocked,
                "已验证的飞书本人通道不可用；未发送消息。",
            ),
        }
    })
    .await
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TestOutboxRecord {
    version: u8,
    request_id: String,
    outcome: DeliveryOutcome,
}

async fn test_notification_with<F, Fut>(
    state_dir: &Path,
    request_id: &str,
    send: F,
) -> Result<DeliveryOutcome>
where
    F: FnOnce(String, String) -> Fut,
    Fut: std::future::Future<Output = DeliveryOutcome>,
{
    if !(8..=128).contains(&request_id.len())
        || !request_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(CoreError::new(
            "INVALID_INPUT",
            "测试请求编号须为8至128位字母、数字、短横线或下划线。",
        ));
    }
    let _guard = TEST_NOTIFICATION_LOCK.lock().await;
    let directory = private_outbox_directory(state_dir)?;
    let digest = crate::types::hash(request_id);
    let path = directory.join(format!("{digest}.json"));
    if let Some(previous) = read_outbox_record(&path, request_id)? {
        return Ok(previous.outcome);
    }
    let mut record = TestOutboxRecord {
        version: 1,
        request_id: request_id.into(),
        outcome: DeliveryOutcome::new(
            DeliveryState::Unknown,
            "测试通知已开始，结果尚未确认；请先核实飞书消息，系统不会自动重发。",
        ),
    };
    let reservation = outbox_tempfile(&directory, &record)?;
    match reservation.persist_noclobber(&path) {
        Ok(_) => sync_directory(&directory)?,
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            return read_outbox_record(&path, request_id)?
                .map(|previous| previous.outcome)
                .ok_or_else(outbox_error);
        }
        Err(error) => return Err(CoreError::io(error.error)),
    }
    let pending = record.outcome.clone();
    record.outcome = send(TEST_MESSAGE.into(), format!("learning_test_{digest}")).await;
    // Even an injected/future sender cannot turn an unconfirmed response into Sent.
    if record.outcome.status == DeliveryState::Sent
        && record
            .outcome
            .message_id
            .as_deref()
            .is_none_or(|id| id.trim().is_empty())
    {
        record.outcome = pending.clone();
    }
    let finish = (|| -> Result<()> {
        // Reject a replaced symlink/oversized record rather than overwriting it.
        read_outbox_record(&path, request_id)?.ok_or_else(outbox_error)?;
        let saved = outbox_tempfile(&directory, &record)?;
        saved
            .persist(&path)
            .map_err(|error| CoreError::io(error.error))?;
        sync_directory(&directory)
    })();
    if finish.is_err() {
        // The durable pre-send reservation remains authoritative if recording
        // the final result fails. Never encourage an automatic send retry.
        return Ok(pending);
    }
    Ok(record.outcome)
}

fn private_outbox_directory(state_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(state_dir).map_err(CoreError::io)?;
    require_directory(state_dir)?;
    let path = state_dir.join("feishu-test-outbox");
    match std::fs::create_dir(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(CoreError::io(error)),
    }
    require_directory(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .map_err(CoreError::io)?;
    }
    Ok(path)
}

fn require_directory(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(CoreError::io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CoreError::new(
            "UNSAFE_PATH",
            "飞书测试记录目录不能是符号链接或普通文件。",
        ));
    }
    Ok(())
}

fn read_bounded_file(path: &Path, maximum: usize, private: bool) -> Result<Vec<u8>> {
    let before = std::fs::symlink_metadata(path).map_err(CoreError::io)?;
    if before.file_type().is_symlink() || !before.is_file() || before.len() > maximum as u64 {
        return Err(outbox_error());
    }
    let file = std::fs::File::open(path).map_err(CoreError::io)?;
    let opened = file.metadata().map_err(CoreError::io)?;
    let after = std::fs::symlink_metadata(path).map_err(CoreError::io)?;
    if after.file_type().is_symlink() || !after.is_file() || opened.len() > maximum as u64 {
        return Err(outbox_error());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Compare the opened descriptor before reading any bytes, so swapping
        // the checked path for a symlink cannot expose another file's contents.
        if before.dev() != opened.dev()
            || before.ino() != opened.ino()
            || after.dev() != opened.dev()
            || after.ino() != opened.ino()
            || (private && opened.mode() & 0o077 != 0)
        {
            return Err(outbox_error());
        }
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut bytes = Vec::new();
    file.take((maximum + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(CoreError::io)?;
    if bytes.len() > maximum {
        return Err(outbox_error());
    }
    Ok(bytes)
}

fn read_outbox_record(path: &Path, request_id: &str) -> Result<Option<TestOutboxRecord>> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(CoreError::io(error)),
        Ok(_) => {}
    }
    let record: TestOutboxRecord =
        serde_json::from_slice(&read_bounded_file(path, MAX_OUTBOX_BYTES, true)?)
            .map_err(|_| outbox_error())?;
    if record.version != 1
        || record.request_id != request_id
        || (record.outcome.status == DeliveryState::Sent
            && record
                .outcome
                .message_id
                .as_deref()
                .is_none_or(|id| id.trim().is_empty()))
    {
        return Err(outbox_error());
    }
    Ok(Some(record))
}

fn outbox_tempfile(directory: &Path, record: &TestOutboxRecord) -> Result<tempfile::NamedTempFile> {
    require_directory(directory)?;
    let bytes = serde_json::to_vec(record).map_err(|_| outbox_error())?;
    if bytes.len() > MAX_OUTBOX_BYTES {
        return Err(outbox_error());
    }
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(CoreError::io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(CoreError::io)?;
    }
    file.write_all(&bytes).map_err(CoreError::io)?;
    file.as_file().sync_all().map_err(CoreError::io)?;
    Ok(file)
}

fn sync_directory(directory: &Path) -> Result<()> {
    require_directory(directory)?;
    std::fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(CoreError::io)
}

fn outbox_error() -> CoreError {
    CoreError::new(
        "INVALID_STATE",
        "飞书测试记录不可安全读取；为防止重复发送，未重试。",
    )
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

    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn confirmed_fixture() -> DeliveryOutcome {
        DeliveryOutcome {
            status: DeliveryState::Sent,
            message_id: Some("om_fixture_confirmed".into()),
            reason: "fixture confirmed".into(),
        }
    }

    #[tokio::test]
    async fn test_message_is_fixed_and_duplicate_requests_reopen_without_resending() {
        let directory = tempfile::tempdir().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let sent_count = count.clone();
        let first = test_notification_with(
            directory.path(),
            "request_sent_01",
            move |text, key| async move {
                sent_count.fetch_add(1, Ordering::SeqCst);
                assert_eq!(text, "学习站测试通知：这是一次由你主动触发的本机连接测试。");
                assert_eq!(
                    key,
                    format!("learning_test_{}", crate::types::hash("request_sent_01"))
                );
                confirmed_fixture()
            },
        )
        .await
        .unwrap();
        let reopened_path = directory.path().to_path_buf();
        let second = test_notification_with(&reopened_path, "request_sent_01", |_, _| async {
            panic!("persisted requests must never invoke the sender again")
        })
        .await
        .unwrap();
        assert_eq!(first, confirmed_fixture());
        assert_eq!(second, first);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let path = directory
            .path()
            .join("feishu-test-outbox")
            .join(format!("{}.json", crate::types::hash("request_sent_01")));
        assert_eq!(
            read_outbox_record(&path, "request_sent_01")
                .unwrap()
                .unwrap()
                .outcome,
            first
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(directory.path().join("feishu-test-outbox"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[tokio::test]
    async fn simultaneous_double_clicks_send_only_once() {
        let directory = tempfile::tempdir().unwrap();
        let count = AtomicUsize::new(0);
        let send = |_, _| async {
            count.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            confirmed_fixture()
        };
        let (a, b) = tokio::join!(
            test_notification_with(directory.path(), "request_double_01", send),
            test_notification_with(directory.path(), "request_double_01", send)
        );
        assert_eq!(a.unwrap(), b.unwrap());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unknown_and_unconfirmed_results_are_durable_and_never_retried() {
        let directory = tempfile::tempdir().unwrap();
        for (request, outcome) in [
            (
                "request_unknown_01",
                DeliveryOutcome::new(DeliveryState::Unknown, "fixture timeout"),
            ),
            (
                "request_no_id_01",
                DeliveryOutcome::new(DeliveryState::Sent, "fixture missing receipt"),
            ),
        ] {
            let first =
                test_notification_with(directory.path(), request, |_, _| async move { outcome })
                    .await
                    .unwrap();
            assert_eq!(first.status, DeliveryState::Unknown);
            let reopened_path = directory.path().to_path_buf();
            let replay = test_notification_with(&reopened_path, request, |_, _| async {
                panic!("ambiguous outcomes must not be retried")
            })
            .await
            .unwrap();
            assert_eq!(replay, first);
        }
    }

    #[tokio::test]
    async fn cancellation_after_reservation_reopens_as_unknown_without_resending() {
        let directory = tempfile::tempdir().unwrap();
        let owned_path = directory.path().to_path_buf();
        let (started, wait_started) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            test_notification_with(&owned_path, "request_cancel_01", move |_, _| async move {
                started.send(()).unwrap();
                std::future::pending::<DeliveryOutcome>().await
            })
            .await
        });
        wait_started.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let reopened =
            test_notification_with(directory.path(), "request_cancel_01", |_, _| async {
                panic!("an interrupted send must never be automatically retried")
            })
            .await
            .unwrap();
        assert_eq!(reopened.status, DeliveryState::Unknown);
        assert!(reopened.message_id.is_none());
    }

    #[tokio::test]
    async fn invalid_request_ids_are_rejected_before_storage_or_sending() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unused");
        for request in [
            "".into(),
            "short".into(),
            "../outside".into(),
            "has spaces".into(),
            "x".repeat(129),
        ] {
            let result = test_notification_with(&path, &request, |_, _| async {
                panic!("invalid requests must never invoke sender")
            })
            .await;
            assert_eq!(result.unwrap_err().code, "INVALID_INPUT");
        }
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn corrupt_oversized_or_mismatched_history_never_triggers_a_send() {
        let directory = tempfile::tempdir().unwrap();
        let outbox = private_outbox_directory(directory.path()).unwrap();
        for (request, bytes) in [
            ("request_corrupt_01", b"not-json".to_vec()),
            ("request_oversize_01", vec![b'x'; MAX_OUTBOX_BYTES + 1]),
            (
                "request_mismatch_01",
                serde_json::to_vec(&TestOutboxRecord {
                    version: 1,
                    request_id: "another_request".into(),
                    outcome: confirmed_fixture(),
                })
                .unwrap(),
            ),
        ] {
            let path = outbox.join(format!("{}.json", crate::types::hash(request)));
            std::fs::write(&path, bytes).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
            assert!(
                test_notification_with(directory.path(), request, |_, _| async {
                    panic!("unsafe history must not trigger a send")
                })
                .await
                .is_err()
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_directories_and_records_are_rejected_without_sending() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), directory.path().join("feishu-test-outbox")).unwrap();
        assert!(
            test_notification_with(directory.path(), "request_dirlink_01", |_, _| async {
                panic!("symlink directories must not trigger sends")
            })
            .await
            .is_err()
        );
        std::fs::remove_file(directory.path().join("feishu-test-outbox")).unwrap();
        let outbox = private_outbox_directory(directory.path()).unwrap();
        let target = outside.path().join("external.json");
        std::fs::write(&target, b"must-not-read-or-overwrite").unwrap();
        symlink(
            &target,
            outbox.join(format!(
                "{}.json",
                crate::types::hash("request_filelink_01")
            )),
        )
        .unwrap();
        assert!(
            test_notification_with(directory.path(), "request_filelink_01", |_, _| async {
                panic!("symlink records must not trigger sends")
            })
            .await
            .is_err()
        );
        assert_eq!(
            std::fs::read(target).unwrap(),
            b"must-not-read-or-overwrite"
        );
    }

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
