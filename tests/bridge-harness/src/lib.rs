// Matching public host contracts only; no production source files are changed.
pub mod types {
    pub type Result<T> = std::result::Result<T, CoreError>;
    #[derive(Debug)]
    pub struct CoreError;
    impl CoreError {
        pub fn new(_: &str, _: impl Into<String>) -> Self {
            Self
        }
    }
}
pub mod feishu {
    use serde::{Deserialize, Serialize};
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
    pub struct IdentityStatus {
        pub ready: bool,
        pub reason: String,
    }
}
pub mod reminders {
    use crate::types::Result;
    pub enum ChannelAvailability {
        Enabled,
        Disabled { reason: String },
    }
    pub struct DeliveryReceipt {
        pub external_id: String,
    }
    pub struct Notification {
        pub id: String,
        pub title: String,
    }
    pub trait NotificationChannel: Send + Sync {
        fn name(&self) -> &str;
        fn availability(&self) -> ChannelAvailability;
        fn deliver(&self, notification: &Notification) -> Result<DeliveryReceipt>;
    }
}
#[path = "../../../tools/feishu-bot/bridge.rs"]
pub mod bridge;

#[cfg(test)]
mod tests {
    use super::{bridge::*, feishu::DeliveryState};
    use std::{
        path::Path,
        time::{Duration, Instant},
    };
    fn cfg(receive: bool) -> BotConfig {
        BotConfig {
            app_id: "cli_0123456789abcdef".into(),
            owner_open_id: "ou_owner".into(),
            chat_id: "oc_selected".into(),
            receive_enabled: receive,
            delivery_enabled: false,
        }
    }
    #[test]
    fn private_pipe_lifecycle_offline_no_sdk_and_no_network() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let script = root
            .join("tools/feishu-bot/worker.mjs")
            .canonicalize()
            .unwrap();
        let dir =
            std::env::temp_dir().join(format!("feishu-bridge-offline-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let worker = BotWorker::start(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../runtime/node")),
            &script,
            &dir,
            cfg(false),
            "offline_fixture_only".into(),
        )
        .unwrap();
        let end = Instant::now() + Duration::from_secs(4);
        while matches!(worker.snapshot().state.as_str(),""|"starting") && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(worker.snapshot().state, "receive_disabled");
        assert!(!worker.identity_status().ready);
        assert_eq!(
            worker.send("request_offline", "hello").status,
            DeliveryState::Blocked
        );
        worker.pull_pending().unwrap();
        assert!(worker.take_inbound().is_empty());
        assert!(worker.ack_persisted("malformed").is_err());
        worker.stop();
        assert!(!worker.identity_status().ready);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn rejects_relative_runtime_paths_before_spawn() {
        assert!(BotWorker::start(
            Path::new("node"),
            Path::new("worker.mjs"),
            Path::new("state"),
            cfg(false),
            "fixture".into()
        )
        .is_err());
    }
}
