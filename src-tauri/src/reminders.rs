//! Runtime-only reminder ledger. Pass the app data directory, never the Markdown library.
//! `tick` commits reminders before returning them; restore the in-app inbox with
//! `list_notifications` after a restart. No channel is invoked by this module.

use crate::types::{CoreError, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use uuid::Uuid;

const TODO_COLUMNS: &str = "id,title,due_at,session_id,done,created_at,reminder_enabled";
const NOTIFICATION_COLUMNS: &str = "id,todo_id,title,due_at,is_read,delivery_status";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Todo {
    pub id: String,
    pub title: String,
    pub due_at: Option<String>,
    #[serde(default)]
    pub reminder_enabled: bool,
    pub session_id: Option<String>,
    pub done: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: String,
    pub todo_id: String,
    pub title: String,
    pub due_at: String,
    pub read: bool,
    /// External delivery only: pending, sent, failed, or unknown. In-app read
    /// state is independent. Version 1 never attempts external delivery.
    pub delivery_status: String,
}

/// Future external adapters must expose their actual configuration state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ChannelAvailability {
    Enabled,
    Disabled { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryReceipt {
    pub external_id: String,
}

/// Calling an adapter is an explicit future operation, separate from `tick`.
/// Implementations must not report success unless delivery is confirmed. An
/// ambiguous network outcome must be persisted as `unknown` before any retry.
pub trait NotificationChannel: Send + Sync {
    fn name(&self) -> &str;
    fn availability(&self) -> ChannelAvailability;
    fn deliver(&self, notification: &Notification) -> Result<DeliveryReceipt>;
}

#[derive(Debug, Default)]
pub struct FeishuChannel;

impl NotificationChannel for FeishuChannel {
    fn name(&self) -> &str {
        "feishu"
    }

    fn availability(&self) -> ChannelAvailability {
        ChannelAvailability::Disabled {
            reason: "飞书未连接；当前仅提供站内提醒。".into(),
        }
    }

    fn deliver(&self, _notification: &Notification) -> Result<DeliveryReceipt> {
        Err(CoreError::new(
            "CHANNEL_DISABLED",
            "飞书未连接，未发送任何消息；请使用站内提醒。",
        ))
    }
}

pub struct Ledger {
    connection: Mutex<Connection>,
}

impl Ledger {
    pub fn new(state_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(state_dir).map_err(CoreError::io)?;
        let mut connection =
            Connection::open(state_dir.join("notifications.sqlite3")).map_err(storage_error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(storage_error)?;
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .map_err(storage_error)?;
        migrate_schema(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn create_todo(
        &self,
        title: &str,
        due_at: Option<&str>,
        reminder_enabled: bool,
        session_id: Option<&str>,
        request_id: &str,
    ) -> Result<Todo> {
        let title = title.trim();
        validate_text(title, "title", 500)?;
        validate_text(request_id, "requestId", 200)?;
        if let Some(session_id) = session_id {
            validate_text(session_id, "sessionId", 200)?;
        }
        let (canonical_due, due_seconds, due_nanos) = normalize_due(due_at)?;
        if reminder_enabled && canonical_due.is_none() {
            return Err(CoreError::new(
                "INVALID_INPUT",
                "启用提醒前请先设置提醒时间。",
            ));
        }
        let payload = serde_json::to_vec(&(title, &canonical_due, reminder_enabled, session_id))
            .map_err(|_| CoreError::new("INTERNAL", "无法编码待办请求。"))?;
        let payload_hash = format!("{:x}", Sha256::digest(payload));
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let prior: Option<(String, String, i64)> = transaction
            .query_row(
                "SELECT payload_hash,todo_id,payload_version FROM reminder_requests WHERE request_id=?1",
                [request_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(storage_error)?;
        if let Some((prior_hash, todo_id, payload_version)) = prior {
            // Preserve v1 hashes exactly. Migration conservatively turns off old
            // reminders, so a legacy replay cannot implicitly enable delivery.
            let matches = if payload_version == 1 && !reminder_enabled {
                let legacy_payload = serde_json::to_vec(&(
                    title,
                    canonical_due.as_deref().unwrap_or(""),
                    session_id,
                ))
                .map_err(|_| CoreError::new("INTERNAL", "无法编码待办请求。"))?;
                prior_hash == format!("{:x}", Sha256::digest(legacy_payload))
            } else {
                payload_version == 2 && prior_hash == payload_hash
            };
            if !matches {
                return Err(CoreError::new(
                    "CONFLICT",
                    "requestId 已用于不同的待办内容，请为新请求使用新的 requestId。",
                ));
            }
            let todo = get_todo(&transaction, &todo_id)?;
            transaction.commit().map_err(storage_error)?;
            return Ok(todo);
        }
        let todo = Todo {
            id: Uuid::new_v4().to_string(),
            title: title.into(),
            due_at: canonical_due,
            reminder_enabled,
            session_id: session_id.map(str::to_owned),
            done: false,
            created_at: timestamp(Utc::now()),
        };
        transaction
            .execute(
                "INSERT INTO reminder_todos
                 (id,title,due_at,due_seconds,due_nanos,session_id,done,created_at,reminder_enabled)
                 VALUES (?1,?2,?3,?4,?5,?6,0,?7,?8)",
                params![
                    todo.id,
                    todo.title,
                    todo.due_at,
                    due_seconds,
                    due_nanos,
                    todo.session_id,
                    todo.created_at,
                    todo.reminder_enabled
                ],
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "INSERT INTO reminder_requests(request_id,payload_hash,todo_id,payload_version) VALUES(?1,?2,?3,2)",
                params![request_id, payload_hash, todo.id],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(todo)
    }

    pub fn list_todos(&self) -> Result<Vec<Todo>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {TODO_COLUMNS} FROM reminder_todos ORDER BY done,due_seconds,due_nanos,id"
            ))
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], todo_from_row)
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    pub fn set_done(&self, id: &str, done: bool) -> Result<Todo> {
        validate_text(id, "id", 200)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        if transaction
            .execute(
                "UPDATE reminder_todos SET done=?2 WHERE id=?1",
                params![id, done],
            )
            .map_err(storage_error)?
            == 0
        {
            return Err(CoreError::new("NOT_FOUND", "找不到待办。"));
        }
        let todo = get_todo(&transaction, id)?;
        transaction.commit().map_err(storage_error)?;
        Ok(todo)
    }

    /// Stops future reminder creation. Already persisted notification history
    /// and any external message already sent cannot be retracted here.
    pub fn set_reminder_enabled(&self, id: &str, enabled: bool) -> Result<Todo> {
        validate_text(id, "id", 200)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let todo = get_todo(&transaction, id)?;
        if enabled && normalize_due(todo.due_at.as_deref())?.0.is_none() {
            return Err(CoreError::new(
                "INVALID_INPUT",
                "启用提醒前请先设置提醒时间。",
            ));
        }
        transaction
            .execute(
                "UPDATE reminder_todos SET reminder_enabled=?2 WHERE id=?1",
                params![id, enabled],
            )
            .map_err(storage_error)?;
        let todo = get_todo(&transaction, id)?;
        transaction.commit().map_err(storage_error)?;
        Ok(todo)
    }

    /// Returns only newly persisted reminders. Existing unread reminders remain
    /// available through `list_notifications`, including after process restarts.
    pub fn tick(&self) -> Result<Vec<Notification>> {
        self.tick_at(Utc::now())
    }

    fn tick_at(&self, now: DateTime<Utc>) -> Result<Vec<Notification>> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let due_todos = {
            let mut statement = transaction
                .prepare(&format!(
                    "SELECT {TODO_COLUMNS} FROM reminder_todos t
                     WHERE done=0 AND reminder_enabled=1 AND due_at IS NOT NULL AND due_at<>'' AND
                       (due_seconds<?1 OR (due_seconds=?1 AND due_nanos<=?2))
                       AND NOT EXISTS (SELECT 1 FROM reminder_notifications n
                           WHERE n.todo_id=t.id AND n.due_at=t.due_at)
                     ORDER BY due_seconds,due_nanos,id"
                ))
                .map_err(storage_error)?;
            let rows = statement
                .query_map(
                    params![now.timestamp(), now.timestamp_subsec_nanos()],
                    todo_from_row,
                )
                .map_err(storage_error)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(storage_error)?
        };
        let mut notifications = Vec::with_capacity(due_todos.len());
        for todo in due_todos {
            let notification = Notification {
                id: Uuid::new_v4().to_string(),
                todo_id: todo.id,
                title: todo.title,
                due_at: todo.due_at.ok_or_else(|| {
                    CoreError::new("STORAGE", "提醒记录缺少时间，请检查本机数据库。")
                })?,
                read: false,
                delivery_status: "pending".into(),
            };
            transaction
                .execute(
                    "INSERT INTO reminder_notifications
                     (id,todo_id,title,due_at,is_read,delivery_status,created_at)
                     VALUES (?1,?2,?3,?4,0,'pending',?5)",
                    params![
                        notification.id,
                        notification.todo_id,
                        notification.title,
                        notification.due_at,
                        timestamp(now)
                    ],
                )
                .map_err(storage_error)?;
            notifications.push(notification);
        }
        transaction.commit().map_err(storage_error)?;
        Ok(notifications)
    }

    pub fn list_notifications(&self) -> Result<Vec<Notification>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {NOTIFICATION_COLUMNS} FROM reminder_notifications
                 ORDER BY created_at DESC,id"
            ))
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], notification_from_row)
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    pub fn mark_read(&self, id: &str) -> Result<Notification> {
        validate_text(id, "id", 200)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        if transaction
            .execute(
                "UPDATE reminder_notifications SET is_read=1 WHERE id=?1",
                [id],
            )
            .map_err(storage_error)?
            == 0
        {
            return Err(CoreError::new("NOT_FOUND", "找不到提醒。"));
        }
        let notification = transaction
            .query_row(
                &format!("SELECT {NOTIFICATION_COLUMNS} FROM reminder_notifications WHERE id=?1"),
                [id],
                notification_from_row,
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(notification)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| CoreError::new("STORAGE", "提醒存储暂不可用，请重启应用后重试。"))
    }
}

// SQLite cannot remove NOT NULL with ALTER COLUMN. Rebuild only the parent
// table in a single transaction with foreign keys temporarily disabled, then
// verify all preserved children before commit. A failed migration rolls back.
fn migrate_schema(connection: &mut Connection) -> Result<()> {
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .map_err(storage_error)?;
    let migration = (|| -> Result<()> {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let version: i64 = transaction
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(storage_error)?;
        if version > 2 {
            return Err(CoreError::new(
                "UNSUPPORTED_VERSION",
                "提醒数据库由更新版本创建，请使用更新版本打开。",
            ));
        }
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='reminder_todos')",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if exists && !has_column(&transaction, "reminder_todos", "reminder_enabled")? {
            transaction
                .execute_batch(
                    "CREATE TABLE reminder_todos_v2 (
                        id TEXT PRIMARY KEY,
                        title TEXT NOT NULL,
                        due_at TEXT,
                        due_seconds INTEGER NOT NULL,
                        due_nanos INTEGER NOT NULL,
                        session_id TEXT,
                        done INTEGER NOT NULL DEFAULT 0 CHECK(done IN (0,1)),
                        created_at TEXT NOT NULL,
                        reminder_enabled INTEGER NOT NULL DEFAULT 0 CHECK(reminder_enabled IN (0,1)),
                        CHECK(reminder_enabled=0 OR (due_at IS NOT NULL AND due_at<>''))
                     );
                     INSERT INTO reminder_todos_v2
                        (id,title,due_at,due_seconds,due_nanos,session_id,done,created_at,reminder_enabled)
                        SELECT id,title,NULLIF(TRIM(due_at),''),due_seconds,due_nanos,session_id,done,created_at,0
                        FROM reminder_todos;
                     DROP TABLE reminder_todos;
                     ALTER TABLE reminder_todos_v2 RENAME TO reminder_todos;",
                )
                .map_err(storage_error)?;
        }
        transaction
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS reminder_todos (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    due_at TEXT,
                    due_seconds INTEGER NOT NULL,
                    due_nanos INTEGER NOT NULL,
                    session_id TEXT,
                    done INTEGER NOT NULL DEFAULT 0 CHECK(done IN (0,1)),
                    created_at TEXT NOT NULL,
                    reminder_enabled INTEGER NOT NULL DEFAULT 0 CHECK(reminder_enabled IN (0,1)),
                    CHECK(reminder_enabled=0 OR (due_at IS NOT NULL AND due_at<>''))
                 );
                 CREATE INDEX IF NOT EXISTS reminder_todos_due
                    ON reminder_todos(done,reminder_enabled,due_seconds,due_nanos);
                 CREATE TABLE IF NOT EXISTS reminder_requests (
                    request_id TEXT PRIMARY KEY,
                    payload_hash TEXT NOT NULL,
                    todo_id TEXT NOT NULL REFERENCES reminder_todos(id),
                    payload_version INTEGER NOT NULL DEFAULT 2 CHECK(payload_version IN (1,2))
                 );
                 CREATE TABLE IF NOT EXISTS reminder_notifications (
                    id TEXT PRIMARY KEY,
                    todo_id TEXT NOT NULL REFERENCES reminder_todos(id),
                    title TEXT NOT NULL,
                    due_at TEXT NOT NULL,
                    is_read INTEGER NOT NULL DEFAULT 0 CHECK(is_read IN (0,1)),
                    delivery_status TEXT NOT NULL DEFAULT 'pending'
                        CHECK(delivery_status IN ('pending','sent','failed','unknown')),
                    created_at TEXT NOT NULL,
                    UNIQUE(todo_id,due_at)
                 );",
            )
            .map_err(storage_error)?;
        if !has_column(&transaction, "reminder_requests", "payload_version")? {
            transaction
                .execute_batch(
                    "ALTER TABLE reminder_requests ADD COLUMN payload_version INTEGER NOT NULL DEFAULT 1
                     CHECK(payload_version IN (1,2));",
                )
                .map_err(storage_error)?;
        }
        let broken_reference: Option<String> = transaction
            .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
            .optional()
            .map_err(storage_error)?;
        if broken_reference.is_some() {
            return Err(CoreError::new(
                "STORAGE",
                "提醒数据库关联校验失败，迁移已取消。",
            ));
        }
        transaction
            .execute_batch("PRAGMA user_version=2;")
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    })();
    let restore = connection
        .execute_batch("PRAGMA foreign_keys=ON;")
        .map_err(storage_error);
    migration?;
    restore
}

fn has_column(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    // Only internal static table names are passed to this helper.
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(storage_error)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(storage_error)?;
    for name in names {
        if name.map_err(storage_error)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn normalize_due(due_at: Option<&str>) -> Result<(Option<String>, i64, u32)> {
    let Some(value) = due_at else {
        return Ok((None, 0, 0));
    };
    if value.trim().is_empty() {
        return Ok((None, 0, 0));
    }
    if value.len() > 64 {
        return Err(CoreError::new("INVALID_INPUT", "dueAt 过长。"));
    }
    let due = DateTime::parse_from_rfc3339(value)
        .map_err(|_| CoreError::new("INVALID_INPUT", "dueAt 必须是带时区的 RFC3339 日期。"))?
        .with_timezone(&Utc);
    Ok((
        Some(timestamp(due)),
        due.timestamp(),
        due.timestamp_subsec_nanos(),
    ))
}

fn get_todo(connection: &Connection, id: &str) -> Result<Todo> {
    connection
        .query_row(
            &format!("SELECT {TODO_COLUMNS} FROM reminder_todos WHERE id=?1"),
            [id],
            todo_from_row,
        )
        .optional()
        .map_err(storage_error)?
        .ok_or_else(|| CoreError::new("NOT_FOUND", "找不到待办。"))
}

fn todo_from_row(row: &Row<'_>) -> rusqlite::Result<Todo> {
    Ok(Todo {
        id: row.get(0)?,
        title: row.get(1)?,
        due_at: row.get(2)?,
        session_id: row.get(3)?,
        done: row.get(4)?,
        created_at: row.get(5)?,
        reminder_enabled: row.get(6)?,
    })
}

fn notification_from_row(row: &Row<'_>) -> rusqlite::Result<Notification> {
    Ok(Notification {
        id: row.get(0)?,
        todo_id: row.get(1)?,
        title: row.get(2)?,
        due_at: row.get(3)?,
        read: row.get(4)?,
        delivery_status: row.get(5)?,
    })
}

fn validate_text(value: &str, field: &str, max_chars: usize) -> Result<()> {
    if value.trim().is_empty()
        || value.len() > max_chars * 4
        || value.chars().count() > max_chars
        || value.contains('\0')
    {
        return Err(CoreError::new(
            "INVALID_INPUT",
            &format!("{field} 不能为空，且长度不得超过 {max_chars} 个字符。"),
        ));
    }
    Ok(())
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn storage_error(_error: rusqlite::Error) -> CoreError {
    // Do not expose local paths or database contents through IPC error text.
    CoreError::new(
        "STORAGE",
        "无法读写提醒数据库，请检查数据目录权限与可用空间。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("learning-reminders-{}", Uuid::new_v4())))
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn plain_todos_persist_complete_and_never_create_reminders() {
        let dir = TestDirectory::new();
        let todo;
        {
            let ledger = Ledger::new(&dir.0).unwrap();
            todo = ledger
                .create_todo("没有提醒的待办", None, false, Some("session"), "plain")
                .unwrap();
            assert_eq!(todo.due_at, None);
            assert!(!todo.reminder_enabled);
            assert_eq!(
                ledger
                    .create_todo(
                        "没有提醒的待办",
                        Some(" \t"),
                        false,
                        Some("session"),
                        "plain"
                    )
                    .unwrap(),
                todo
            );
            assert!(ledger.tick().unwrap().is_empty());
            assert!(ledger.list_notifications().unwrap().is_empty());
        }
        {
            let ledger = Ledger::new(&dir.0).unwrap();
            assert_eq!(ledger.list_todos().unwrap(), vec![todo.clone()]);
            assert!(ledger
                .tick_at(at("2999-01-01T00:00:00Z"))
                .unwrap()
                .is_empty());
            assert!(ledger.set_done(&todo.id, true).unwrap().done);
            assert!(ledger.tick().unwrap().is_empty());
        }
        let ledger = Ledger::new(&dir.0).unwrap();
        let saved = ledger.list_todos().unwrap().remove(0);
        assert_eq!(saved.id, todo.id);
        assert_eq!(saved.due_at, None);
        assert!(!saved.reminder_enabled);
        assert!(saved.done);
        assert!(!ledger.set_done(&todo.id, false).unwrap().done);
        assert!(ledger.tick().unwrap().is_empty());
        assert!(ledger
            .tick_at(at("2999-01-01T00:00:00Z"))
            .unwrap()
            .is_empty());
        assert!(ledger.list_notifications().unwrap().is_empty());
    }

    #[test]
    fn a_due_time_does_not_enable_reminders_and_disabled_state_survives_restart() {
        let dir = TestDirectory::new();
        let todo;
        {
            let ledger = Ledger::new(&dir.0).unwrap();
            todo = ledger
                .create_todo(
                    "有时间的待办",
                    Some("2000-01-01T08:00:00+08:00"),
                    false,
                    None,
                    "timed",
                )
                .unwrap();
            assert_eq!(
                todo.due_at.as_deref(),
                Some("2000-01-01T00:00:00.000000000Z")
            );
            assert!(!todo.reminder_enabled);
            assert!(ledger.tick().unwrap().is_empty());
            assert!(
                ledger
                    .set_reminder_enabled(&todo.id, true)
                    .unwrap()
                    .reminder_enabled
            );
            assert!(
                !ledger
                    .set_reminder_enabled(&todo.id, false)
                    .unwrap()
                    .reminder_enabled
            );
            assert!(ledger.tick().unwrap().is_empty());
        }
        let ledger = Ledger::new(&dir.0).unwrap();
        assert_eq!(ledger.list_todos().unwrap(), vec![todo.clone()]);
        assert!(ledger.tick().unwrap().is_empty());
        ledger.set_reminder_enabled(&todo.id, true).unwrap();
        assert_eq!(ledger.tick().unwrap().len(), 1);
        ledger.set_reminder_enabled(&todo.id, false).unwrap();
        assert!(ledger.tick().unwrap().is_empty());
        // Closing reminders does not erase the user's existing inbox history.
        assert_eq!(ledger.list_notifications().unwrap().len(), 1);
        ledger.set_reminder_enabled(&todo.id, true).unwrap();
        assert!(ledger.tick().unwrap().is_empty());
    }

    #[test]
    fn enabling_requires_a_valid_time_and_enabled_is_part_of_request_identity() {
        let dir = TestDirectory::new();
        let ledger = Ledger::new(&dir.0).unwrap();
        for (index, due) in [None, Some(""), Some(" \t")].into_iter().enumerate() {
            let error = ledger
                .create_todo("缺少时间", due, true, None, &format!("invalid-{index}"))
                .unwrap_err();
            assert_eq!(error.code, "INVALID_INPUT");
        }
        let plain = ledger
            .create_todo("普通待办", None, false, None, "plain")
            .unwrap();
        assert_eq!(
            ledger
                .set_reminder_enabled(&plain.id, true)
                .unwrap_err()
                .code,
            "INVALID_INPUT"
        );
        assert!(ledger
            .list_todos()
            .unwrap()
            .iter()
            .all(|todo| !todo.reminder_enabled));
        assert_eq!(
            ledger
                .create_todo("非法日期", Some("not-a-date"), false, None, "invalid-date")
                .unwrap_err()
                .code,
            "INVALID_INPUT"
        );
        ledger
            .create_todo("有时间", Some("2000-01-01T00:00:00Z"), false, None, "timed")
            .unwrap();
        assert_eq!(
            ledger
                .create_todo("有时间", Some("2000-01-01T00:00:00Z"), true, None, "timed")
                .unwrap_err()
                .code,
            "CONFLICT"
        );
        assert!(ledger.tick().unwrap().is_empty());
    }

    fn legacy_database(dir: &Path, version: i64) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let connection = Connection::open(dir.join("notifications.sqlite3")).unwrap();
        connection.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE reminder_todos (
                id TEXT PRIMARY KEY,title TEXT NOT NULL,due_at TEXT NOT NULL,
                due_seconds INTEGER NOT NULL,due_nanos INTEGER NOT NULL,
                session_id TEXT,done INTEGER NOT NULL DEFAULT 0 CHECK(done IN (0,1)),created_at TEXT NOT NULL
             );
             CREATE INDEX reminder_todos_due ON reminder_todos(done,due_seconds,due_nanos);
             CREATE TABLE reminder_requests (
                request_id TEXT PRIMARY KEY,payload_hash TEXT NOT NULL,
                todo_id TEXT NOT NULL REFERENCES reminder_todos(id)
             );
             CREATE TABLE reminder_notifications (
                id TEXT PRIMARY KEY,todo_id TEXT NOT NULL REFERENCES reminder_todos(id),
                title TEXT NOT NULL,due_at TEXT NOT NULL,is_read INTEGER NOT NULL DEFAULT 0,
                delivery_status TEXT NOT NULL DEFAULT 'pending',created_at TEXT NOT NULL,
                UNIQUE(todo_id,due_at)
             );"
        ).unwrap();
        let due = timestamp(at("2000-01-01T00:00:00Z"));
        for (id, title, date, done) in [
            ("legacy_due", "旧提醒", due.as_str(), false),
            ("legacy_plain", "旧待办", "", true),
            ("legacy_notified", "旧已提醒", due.as_str(), false),
        ] {
            connection.execute(
                "INSERT INTO reminder_todos(id,title,due_at,due_seconds,due_nanos,session_id,done,created_at)
                 VALUES(?1,?2,?3,?4,0,NULL,?5,?6)",
                params![id, title, date, if date.is_empty() { 0 } else { at(&due).timestamp() }, done, due],
            ).unwrap();
        }
        let old_payload = serde_json::to_vec(&("旧提醒", &due, None::<&str>)).unwrap();
        let old_hash = format!("{:x}", Sha256::digest(old_payload));
        connection.execute(
            "INSERT INTO reminder_requests(request_id,payload_hash,todo_id) VALUES('legacy_request',?1,'legacy_due')",
            [&old_hash],
        ).unwrap();
        let plain_payload = serde_json::to_vec(&("旧待办", "", None::<&str>)).unwrap();
        connection.execute(
            "INSERT INTO reminder_requests(request_id,payload_hash,todo_id) VALUES('legacy_plain_request',?1,'legacy_plain')",
            [format!("{:x}", Sha256::digest(plain_payload))],
        ).unwrap();
        connection.execute(
            "INSERT INTO reminder_notifications(id,todo_id,title,due_at,is_read,delivery_status,created_at)
             VALUES('legacy_notification','legacy_notified','旧已提醒',?1,1,'sent',?1)",
            [&due],
        ).unwrap();
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
        old_hash
    }

    #[test]
    fn v2_migration_preserves_records_and_hashes_while_disabling_old_reminders() {
        for old_version in [0, 1] {
            let dir = TestDirectory::new();
            let old_hash = legacy_database(&dir.0, old_version);
            {
                let ledger = Ledger::new(&dir.0).unwrap();
                let todos = ledger.list_todos().unwrap();
                assert_eq!(todos.len(), 3);
                assert!(todos.iter().all(|todo| !todo.reminder_enabled));
                let plain = todos.iter().find(|todo| todo.id == "legacy_plain").unwrap();
                assert_eq!(plain.due_at, None);
                assert!(plain.done);
                assert!(ledger.tick().unwrap().is_empty());
                let notifications = ledger.list_notifications().unwrap();
                assert_eq!(notifications.len(), 1);
                assert_eq!(notifications[0].id, "legacy_notification");
                assert_eq!(notifications[0].delivery_status, "sent");
                assert!(notifications[0].read);
                let replay = ledger
                    .create_todo(
                        "旧提醒",
                        Some("2000-01-01T08:00:00+08:00"),
                        false,
                        None,
                        "legacy_request",
                    )
                    .unwrap();
                assert_eq!(replay.id, "legacy_due");
                assert_eq!(
                    ledger
                        .create_todo("旧待办", None, false, None, "legacy_plain_request")
                        .unwrap()
                        .id,
                    "legacy_plain"
                );
                assert_eq!(
                    ledger
                        .create_todo(
                            "旧提醒",
                            Some("2000-01-01T00:00:00Z"),
                            true,
                            None,
                            "legacy_request"
                        )
                        .unwrap_err()
                        .code,
                    "CONFLICT"
                );
                {
                    let connection = ledger.lock().unwrap();
                    let (hash, version): (String, i64) = connection.query_row(
                        "SELECT payload_hash,payload_version FROM reminder_requests WHERE request_id='legacy_request'", [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    ).unwrap();
                    assert_eq!(hash, old_hash);
                    assert_eq!(version, 1);
                    assert_eq!(
                        connection
                            .query_row("SELECT COUNT(*) FROM reminder_requests", [], |row| row
                                .get::<_, i64>(0))
                            .unwrap(),
                        2
                    );
                    assert_eq!(
                        connection
                            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                            .unwrap(),
                        2
                    );
                    assert_eq!(
                        connection
                            .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
                            .unwrap(),
                        1
                    );
                }
                let new_plain = ledger
                    .create_todo("新待办", None, false, None, "new_plain")
                    .unwrap();
                assert_eq!(new_plain.due_at, None);
                ledger.set_reminder_enabled("legacy_due", true).unwrap();
                assert_eq!(ledger.tick().unwrap().len(), 1);
            }
            let ledger = Ledger::new(&dir.0).unwrap();
            assert!(
                ledger
                    .list_todos()
                    .unwrap()
                    .iter()
                    .find(|todo| todo.id == "legacy_due")
                    .unwrap()
                    .reminder_enabled
            );
            assert!(ledger.tick().unwrap().is_empty());
            assert_eq!(ledger.list_notifications().unwrap().len(), 2);
        }
    }

    #[test]
    fn failed_migration_rolls_back_schema_and_restores_foreign_keys() {
        let dir = TestDirectory::new();
        let old_hash = legacy_database(&dir.0, 1);
        let mut connection = Connection::open(dir.0.join("notifications.sqlite3")).unwrap();
        connection.execute_batch(
            "PRAGMA foreign_keys=OFF;
             INSERT INTO reminder_notifications(id,todo_id,title,due_at,is_read,delivery_status,created_at)
             VALUES('orphan','missing','orphan','2000-01-01T00:00:00Z',0,'pending','2000-01-01T00:00:00Z');"
        ).unwrap();
        assert_eq!(migrate_schema(&mut connection).unwrap_err().code, "STORAGE");
        assert!(!has_column(&connection, "reminder_todos", "reminder_enabled").unwrap());
        assert!(!has_column(&connection, "reminder_requests", "payload_version").unwrap());
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM reminder_todos", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload_hash FROM reminder_requests WHERE request_id='legacy_request'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            old_hash
        );
    }

    #[test]
    fn repeated_ticks_and_reopened_ledger_do_not_duplicate_notifications() {
        let dir = TestDirectory::new();
        let todo;
        let notification;
        {
            let ledger = Ledger::new(&dir.0).unwrap();
            todo = ledger
                .create_todo(
                    "复习",
                    Some("2000-01-01T00:00:00Z"),
                    true,
                    Some("session"),
                    "a",
                )
                .unwrap();
            let first = ledger.tick().unwrap();
            assert_eq!(first.len(), 1);
            notification = first[0].clone();
            assert_eq!(notification.todo_id, todo.id);
            assert_eq!(notification.delivery_status, "pending");
            assert!(ledger.tick().unwrap().is_empty());
            assert!(ledger.mark_read(&notification.id).unwrap().read);
        }
        let ledger = Ledger::new(&dir.0).unwrap();
        assert_eq!(ledger.list_todos().unwrap(), vec![todo]);
        assert!(ledger.tick().unwrap().is_empty());
        let notifications = ledger.list_notifications().unwrap();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].id, notification.id);
        assert!(notifications[0].read);
    }

    #[test]
    fn restart_catches_up_overdue_todos_and_skips_completed_ones() {
        let dir = TestDirectory::new();
        {
            let ledger = Ledger::new(&dir.0).unwrap();
            ledger
                .create_todo("漏过的提醒", Some("2000-01-01T00:00:00Z"), true, None, "a")
                .unwrap();
            let done = ledger
                .create_todo("已完成", Some("2000-01-01T00:00:00Z"), true, None, "b")
                .unwrap();
            ledger.set_done(&done.id, true).unwrap();
            ledger
                .create_todo("未来", Some("2999-01-01T00:00:00Z"), true, None, "c")
                .unwrap();
        }
        let ledger = Ledger::new(&dir.0).unwrap();
        let reminders = ledger.tick().unwrap();
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].title, "漏过的提醒");
    }

    #[test]
    fn request_ids_are_durable_and_conflict_on_different_content() {
        let dir = TestDirectory::new();
        let original;
        {
            let ledger = Ledger::new(&dir.0).unwrap();
            original = ledger
                .create_todo("复习", Some("2000-01-01T08:00:00+08:00"), true, None, "a")
                .unwrap();
            let replay = ledger
                .create_todo("复习", Some("2000-01-01T00:00:00Z"), true, None, "a")
                .unwrap();
            assert_eq!(original, replay);
        }
        let ledger = Ledger::new(&dir.0).unwrap();
        assert_eq!(
            ledger
                .create_todo("复习", Some("2000-01-01T00:00:00Z"), true, None, "a")
                .unwrap(),
            original
        );
        let error = ledger
            .create_todo("不同内容", Some("2000-01-01T00:00:00Z"), true, None, "a")
            .unwrap_err();
        assert_eq!(serde_json::to_value(error).unwrap()["code"], "CONFLICT");
        assert_eq!(ledger.list_todos().unwrap().len(), 1);
    }

    #[test]
    fn changing_completion_does_not_resend_an_existing_reminder() {
        let dir = TestDirectory::new();
        let ledger = Ledger::new(&dir.0).unwrap();
        let todo = ledger
            .create_todo("复习", Some("2000-01-01T00:00:00Z"), true, None, "a")
            .unwrap();
        assert_eq!(ledger.tick().unwrap().len(), 1);
        assert!(ledger.set_done(&todo.id, true).unwrap().done);
        assert!(!ledger.set_done(&todo.id, false).unwrap().done);
        assert!(ledger.tick().unwrap().is_empty());
    }

    #[test]
    fn due_comparison_preserves_subsecond_precision() {
        let dir = TestDirectory::new();
        let ledger = Ledger::new(&dir.0).unwrap();
        ledger
            .create_todo(
                "精确时间",
                Some("2026-01-01T00:00:00.500000001Z"),
                true,
                None,
                "a",
            )
            .unwrap();
        assert!(ledger
            .tick_at(at("2026-01-01T00:00:00.500000000Z"))
            .unwrap()
            .is_empty());
        assert_eq!(
            ledger
                .tick_at(at("2026-01-01T00:00:00.500000001Z"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn independent_connections_share_deduplication() {
        let dir = TestDirectory::new();
        let a = Ledger::new(&dir.0).unwrap();
        let b = Ledger::new(&dir.0).unwrap();
        let first = a
            .create_todo("复习", Some("2000-01-01T00:00:00Z"), true, None, "a")
            .unwrap();
        assert_eq!(
            first,
            b.create_todo("复习", Some("2000-01-01T00:00:00Z"), true, None, "a")
                .unwrap()
        );
        assert_eq!(a.tick().unwrap().len(), 1);
        assert!(b.tick().unwrap().is_empty());
        assert_eq!(b.list_notifications().unwrap().len(), 1);
    }

    #[test]
    fn validates_input_and_never_claims_feishu_is_connected() {
        let dir = TestDirectory::new();
        let ledger = Ledger::new(&dir.0).unwrap();
        assert!(ledger
            .create_todo(" ", Some("2000-01-01T00:00:00Z"), true, None, "a")
            .is_err());
        assert!(ledger
            .create_todo("复习", Some("2026-01-01"), true, None, "a")
            .is_err());
        assert!(ledger
            .create_todo("复习", Some("2000-01-01T00:00:00Z"), true, None, "")
            .is_err());
        assert!(ledger
            .create_todo(
                &"字".repeat(501),
                Some("2000-01-01T00:00:00Z"),
                true,
                None,
                "a"
            )
            .is_err());
        assert!(matches!(
            FeishuChannel.availability(),
            ChannelAvailability::Disabled { .. }
        ));
        let todo = ledger
            .create_todo("复习", Some("2000-01-01T00:00:00Z"), true, None, "ok")
            .unwrap();
        let notification = ledger.tick().unwrap().remove(0);
        assert_eq!(notification.todo_id, todo.id);
        let error = FeishuChannel.deliver(&notification).unwrap_err();
        assert_eq!(
            serde_json::to_value(error).unwrap()["code"],
            "CHANNEL_DISABLED"
        );
        assert_eq!(
            ledger.list_notifications().unwrap()[0].delivery_status,
            "pending"
        );
    }
}
