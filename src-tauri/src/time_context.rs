//! Non-secret, durable time anchors. Reusing a user entry reuses its parsing clock.
use crate::{
    app::atomic_json,
    types::{hash, CoreError, Result},
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, sync::Mutex};
static ANCHORS: Mutex<()> = Mutex::new(());
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeContext {
    pub reference_time: String,
    pub time_zone: Option<String>,
}
pub fn system_time_zone() -> Option<String> {
    let zone = match std::env::var("TZ") {
        Ok(value) => value.trim_start_matches(':').to_string(),
        Err(_) => iana_time_zone::get_timezone().ok()?,
    };
    if zone.len() <= 100
        && (zone == "UTC" || zone.contains('/'))
        && !zone.starts_with('/')
        && !zone.contains("..")
        && zone
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b))
    {
        Some(zone)
    } else {
        None
    }
}
impl TimeContext {
    pub fn capture() -> Self {
        Self {
            reference_time: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            time_zone: system_time_zone(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if !self.reference_time.ends_with('Z')
            || self.reference_time.len() > 32
            || DateTime::parse_from_rfc3339(&self.reference_time).is_err()
            || self.time_zone.as_ref().is_some_and(|s| {
                s.is_empty()
                    || s.len() > 100
                    || s.contains("..")
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b))
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}
pub fn for_operation(state: &Path, key: &str) -> Result<TimeContext> {
    let _guard = ANCHORS
        .lock()
        .map_err(|_| CoreError::new("BUSY", "请求时间暂不可用。"))?;
    let dir = state.join("time-anchors");
    match fs::symlink_metadata(&dir) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => return Err(invalid()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&dir)?;
        }
        Err(e) => return Err(CoreError::io(e)),
    }
    let path = dir.join(format!("{}.json", hash(key)));
    match fs::symlink_metadata(&path) {
        Ok(m) => {
            if m.file_type().is_symlink() || !m.is_file() || m.len() > 2048 {
                return Err(invalid());
            }
            let saved: TimeContext =
                serde_json::from_slice(&fs::read(&path)?).map_err(|_| invalid())?;
            saved.validate()?;
            Ok(saved)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let time = TimeContext::capture();
            atomic_json(&path, &time)?;
            Ok(time)
        }
        Err(e) => Err(CoreError::io(e)),
    }
}
pub fn resolve_due(
    context: Option<&TimeContext>,
    due: Option<&str>,
    relative: Option<u32>,
) -> Result<Option<String>> {
    let Some(minutes) = relative else {
        return due
            .map(|s| {
                DateTime::parse_from_rfc3339(s)
                    .map(|d| {
                        d.with_timezone(&Utc)
                            .to_rfc3339_opts(SecondsFormat::Secs, true)
                    })
                    .map_err(|_| invalid())
            })
            .transpose();
    };
    if !(1..=525600).contains(&minutes) {
        return Err(invalid());
    }
    let ctx = context.ok_or_else(invalid)?;
    ctx.validate()?;
    let target = DateTime::parse_from_rfc3339(&ctx.reference_time)
        .map_err(|_| invalid())?
        .checked_add_signed(chrono::Duration::minutes(minutes.into()))
        .ok_or_else(invalid)?;
    if let Some(given) = due {
        if DateTime::parse_from_rfc3339(given).map_err(|_| invalid())? != target {
            return Err(CoreError::new(
                "TIME_CONTEXT_CONFLICT",
                "相对截止时间与原请求时间不一致，未保存待办。",
            ));
        }
    }
    Ok(Some(
        target
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    ))
}
fn invalid() -> CoreError {
    CoreError::new(
        "TIME_CONTEXT_INVALID",
        "请求时间或时区不可安全读取；未按猜测的当地时间保存。",
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_deadlines_cross_midnight_and_offsets_are_verified() {
        let c = TimeContext {
            reference_time: "2026-10-16T15:55:00Z".into(),
            time_zone: Some("Asia/Shanghai".into()),
        };
        assert_eq!(
            resolve_due(Some(&c), Some("2026-10-17T00:05:00+08:00"), Some(10))
                .unwrap()
                .unwrap(),
            "2026-10-16T16:05:00Z"
        );
        assert!(resolve_due(Some(&c), Some("2026-10-17T00:15:00+08:00"), Some(10)).is_err());
        assert!(resolve_due(None, None, Some(10)).is_err());
        assert!(resolve_due(Some(&c), Some("2026-10-17T00:05:00"), None).is_err());
    }
    #[test]
    fn daylight_saving_elapsed_time_is_not_a_fixed_local_clock_addition() {
        for (reference, due) in [
            ("2026-03-08T06:55:00Z", "2026-03-08T03:05:00-04:00"),
            ("2026-11-01T05:55:00Z", "2026-11-01T01:05:00-05:00"),
        ] {
            let c = TimeContext {
                reference_time: reference.into(),
                time_zone: Some("America/New_York".into()),
            };
            assert!(resolve_due(Some(&c), Some(due), Some(10)).is_ok());
        }
    }
    #[test]
    fn same_logical_operation_reopens_same_anchor_and_corruption_never_recaptures() {
        let d = tempfile::tempdir().unwrap();
        let a = for_operation(d.path(), "session:user1").unwrap();
        let saved = TimeContext {
            reference_time: "2026-01-01T00:00:00Z".into(),
            time_zone: Some("Etc/UTC".into()),
        };
        let path = d
            .path()
            .join("time-anchors")
            .join(format!("{}.json", hash("session:user1")));
        atomic_json(&path, &saved).unwrap();
        assert_eq!(for_operation(d.path(), "session:user1").unwrap(), saved);
        assert_ne!(for_operation(d.path(), "session:user2").unwrap(), saved);
        fs::write(&path, "broken").unwrap();
        assert!(for_operation(d.path(), "session:user1").is_err());
        assert!(a.reference_time.ends_with('Z'));
    }
}
