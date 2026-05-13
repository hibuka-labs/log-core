use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::LogLevel;

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub level: LogLevel,
    pub module: &'static str,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    pub context: Value,
    pub session_id: Option<String>,
}

impl LogEntry {
    pub fn new(
        level: LogLevel,
        module: &'static str,
        message: impl Into<String>,
        context: Value,
    ) -> Self {
        Self {
            level,
            module,
            message: message.into(),
            timestamp: Utc::now(),
            context,
            session_id: None,
        }
    }

    pub fn with_session_id(mut self, session_id: Option<String>) -> Self {
        self.session_id = session_id;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_new_entry() {
        let entry = LogEntry::new(LogLevel::Info, "test::module", "hello", json!({"key": "value"}));
        assert_eq!(entry.level, LogLevel::Info);
        assert_eq!(entry.module, "test::module");
        assert_eq!(entry.message, "hello");
        assert_eq!(entry.context, json!({"key": "value"}));
        assert!(entry.session_id.is_none());
    }

    #[test]
    fn test_with_session_id() {
        let entry = LogEntry::new(LogLevel::Warn, "mod", "msg", json!(null))
            .with_session_id(Some("sess-1".into()));
        assert_eq!(entry.session_id, Some("sess-1".into()));
    }
}
