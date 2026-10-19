use std::sync::Arc;

use async_trait::async_trait;
use log_core::{LogEntry, LogLevel, LogSink, Logger};
use serde_json::json;
use tokio::sync::Mutex;

/// Mock cloud-reporting sink — collects logs >= Warn
struct MockCloudSink {
    uploaded: Arc<Mutex<Vec<LogEntry>>>,
}

#[async_trait]
impl LogSink for MockCloudSink {
    async fn write(&self, entry: &LogEntry) -> anyhow::Result<()> {
        if entry.level >= LogLevel::Warn {
            self.uploaded.lock().await.push(entry.clone());
        }
        Ok(())
    }
}

/// Mock local-file sink — collects all logs
struct MockFileSink {
    entries: Arc<Mutex<Vec<LogEntry>>>,
}

#[async_trait]
impl LogSink for MockFileSink {
    async fn write(&self, entry: &LogEntry) -> anyhow::Result<()> {
        self.entries.lock().await.push(entry.clone());
        Ok(())
    }
}

#[tokio::test]
async fn test_realistic_logger_usage() {
    // Simulated: the product-environment Logger = local file + cloud upload (>=Warn)
    let uploaded = Arc::new(Mutex::new(Vec::new()));
    let file_entries = Arc::new(Mutex::new(Vec::new()));

    let cloud = MockCloudSink {
        uploaded: uploaded.clone(),
    };
    let file = MockFileSink {
        entries: file_entries.clone(),
    };

    let logger = Logger::builder()
        .sink(Box::new(file))
        .sink(Box::new(cloud))
        .min_level(LogLevel::Info)
        .build();

    // ------ Simulated normal operation ------
    logger
        .info(
            "agent::session",
            "session created",
            json!({"session_id": 1}),
        )
        .await;
    logger
        .info(
            "agent::ssh",
            "connected to server",
            json!({"host": "192.168.1.1", "latency_ms": 15}),
        )
        .await;

    // ------ Simulated warning ------
    logger
        .warn(
            "agent::ssh",
            "connection slow",
            json!({"host": "192.168.1.1", "latency_ms": 2500}),
        )
        .await;

    // ------ Simulated error ------
    logger
        .error(
            "agent::plan",
            "step execution failed",
            json!({"step": 3, "error": "permission denied", "command": "systemctl restart nginx"}),
        )
        .await;

    // ------ Debug filtered out by min_level ------
    logger
        .debug(
            "agent::internal",
            "debug info",
            json!({"detail": "should not appear"}),
        )
        .await;

    // Verify all logs reached the local file
    {
        let entries = file_entries.lock().await;
        assert_eq!(entries.len(), 4, "file should have all 4 entries");
        assert_eq!(entries[0].module, "agent::session");
        assert_eq!(entries[1].level, LogLevel::Info);
        assert_eq!(entries[2].level, LogLevel::Warn);
        assert_eq!(entries[3].level, LogLevel::Error);
    }

    // Verify only Warn + Error were uploaded to the cloud
    {
        let up = uploaded.lock().await;
        assert_eq!(up.len(), 2, "cloud should only have warn+error");
        assert_eq!(up[0].level, LogLevel::Warn);
        assert_eq!(up[0].message, "connection slow");
        assert_eq!(up[1].level, LogLevel::Error);
        assert_eq!(up[1].message, "step execution failed");
    }
}
