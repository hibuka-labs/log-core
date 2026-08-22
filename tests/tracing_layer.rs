use std::sync::Arc;

use async_trait::async_trait;
use log_core::{LogCoreLayer, LogEntry, LogLevel, LogSink};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;

/// A mock sink that captures written entries for later inspection.
struct MockSink {
    entries: Arc<Mutex<Vec<LogEntry>>>,
    notify: Arc<Notify>,
}

impl MockSink {
    fn new() -> (Self, Arc<Mutex<Vec<LogEntry>>>, Arc<Notify>) {
        let entries = Arc::new(Mutex::new(Vec::new()));
        let notify = Arc::new(Notify::new());
        (
            Self {
                entries: entries.clone(),
                notify: notify.clone(),
            },
            entries,
            notify,
        )
    }
}

#[async_trait]
impl LogSink for MockSink {
    async fn write(&self, entry: &LogEntry) -> anyhow::Result<()> {
        self.entries.lock().await.push(entry.clone());
        self.notify.notify_one();
        Ok(())
    }
}

/// Install a subscriber with a `LogCoreLayer` wrapping `MockSink`, execute `f`,
/// then poll until `expected_count` entries arrive and return them.
async fn collect_events(
    min_level: LogLevel,
    expected_count: usize,
    f: impl FnOnce(),
) -> Vec<LogEntry> {
    let (sink, entries, _notify) = MockSink::new();
    let layer = LogCoreLayer::new(vec![Box::new(sink)], min_level);
    let subscriber = Registry::default().with(layer);
    let guard = tracing::subscriber::set_default(subscriber);

    f();
    drop(guard);

    // Wait for all entries to arrive.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let count = entries.lock().await.len();
        if count >= expected_count {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for log entries: got {count}, expected {expected_count}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let collected = entries.lock().await;
    collected.clone()
}

// ── Layer construction ───────────────────────────────────────────────────

#[tokio::test]
async fn test_console_layer_creation() {
    let layer = LogCoreLayer::console(LogLevel::Info);
    // Just verify it doesn't panic and can be used as a subscriber.
    let subscriber = Registry::default().with(layer);
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::info!("test");
}

#[tokio::test]
async fn test_file_layer_creation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.log");
    let layer = LogCoreLayer::file(path.to_str().unwrap(), LogLevel::Debug)
        .await
        .unwrap();
    let subscriber = Registry::default().with(layer);
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::info!("file test");
}

#[tokio::test]
async fn test_console_and_file_creation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.log");
    let layer = LogCoreLayer::console_and_file(path.to_str().unwrap(), LogLevel::Warn)
        .await
        .unwrap();
    let subscriber = Registry::default().with(layer);
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::warn!("combo test");
}

#[tokio::test]
async fn test_file_layer_bad_path() {
    let result = LogCoreLayer::file("/nonexistent/dir/file.log", LogLevel::Info).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_console_and_file_bad_path() {
    let result = LogCoreLayer::console_and_file("/nonexistent/dir/file.log", LogLevel::Info).await;
    assert!(result.is_err());
}

// ── Event recording ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_info_event_captured() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::info!("hello world");
    })
    .await;

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].level, LogLevel::Info);
    assert_eq!(entries[0].message, "hello world");
    assert!(entries[0].context.is_object());
}

#[tokio::test]
async fn test_error_event_level() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::error!("something broke");
    })
    .await;

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].level, LogLevel::Error);
    assert_eq!(entries[0].message, "something broke");
}

#[tokio::test]
async fn test_warn_event_level() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::warn!("careful");
    })
    .await;

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].level, LogLevel::Warn);
}

#[tokio::test]
async fn test_debug_event_level() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::debug!("debug info");
    })
    .await;

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].level, LogLevel::Debug);
}

#[tokio::test]
async fn test_trace_maps_to_debug() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::trace!("trace info");
    })
    .await;

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].level, LogLevel::Debug);
}

// ── Min-level filtering ──────────────────────────────────────────────────

#[tokio::test]
async fn test_debug_filtered_by_info_min() {
    let entries = collect_events(LogLevel::Info, 1, || {
        tracing::debug!("should be filtered");
        tracing::info!("should pass");
    })
    .await;

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message, "should pass");
}

#[tokio::test]
async fn test_info_filtered_by_warn_min() {
    let entries = collect_events(LogLevel::Warn, 1, || {
        tracing::info!("filtered");
        tracing::warn!("passed");
    })
    .await;

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message, "passed");
}

#[tokio::test]
async fn test_error_passes_when_min_is_error() {
    let entries = collect_events(LogLevel::Error, 1, || {
        tracing::info!("nope");
        tracing::warn!("nope");
        tracing::debug!("nope");
        tracing::error!("yes");
    })
    .await;

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message, "yes");
}

#[tokio::test]
async fn test_all_levels_pass_when_min_is_debug() {
    let entries = collect_events(LogLevel::Debug, 4, || {
        tracing::error!("e");
        tracing::warn!("w");
        tracing::info!("i");
        tracing::debug!("d");
    })
    .await;

    assert_eq!(entries.len(), 4);
}

// ── Field formatting (JsonVisitor) ───────────────────────────────────────

#[tokio::test]
async fn test_string_field_via_debug() {
    // record_debug with a &str produces "\"value\"" which is stored as a JSON string.
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::info!(key = "value", "msg");
    })
    .await;

    let ctx = &entries[0].context;
    let key = ctx.get("key").expect("key field should exist");
    // Tracing macros call record_str for string literal fields.
    assert_eq!(key, &json!("value"));
}

#[tokio::test]
async fn test_bool_field() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::info!(active = true, "msg");
    })
    .await;

    let ctx = &entries[0].context;
    assert_eq!(ctx.get("active"), Some(&Value::Bool(true)));
}

#[tokio::test]
async fn test_i64_field() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::info!(count = 42_i64, "msg");
    })
    .await;

    let ctx = &entries[0].context;
    assert_eq!(
        ctx.get("count"),
        Some(&Value::Number(serde_json::Number::from(42_i64)))
    );
}

#[tokio::test]
async fn test_u64_field() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::info!(size = 100_u64, "msg");
    })
    .await;

    let ctx = &entries[0].context;
    assert_eq!(
        ctx.get("size"),
        Some(&Value::Number(serde_json::Number::from(100_u64)))
    );
}

#[tokio::test]
async fn test_f64_field() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::info!(ratio = 2.71_f64, "msg");
    })
    .await;

    let ctx = &entries[0].context;
    assert_eq!(ctx.get("ratio"), Some(&json!(2.71)));
}

#[tokio::test]
async fn test_multiple_fields() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::info!(name = "alice", age = 30_i64, active = true, "msg");
    })
    .await;

    let ctx = &entries[0].context;
    assert_eq!(ctx.get("name"), Some(&json!("alice")));
    assert_eq!(
        ctx.get("age"),
        Some(&Value::Number(serde_json::Number::from(30_i64)))
    );
    assert_eq!(ctx.get("active"), Some(&Value::Bool(true)));
}

#[tokio::test]
async fn test_message_field_recorded_via_record_str() {
    // The tracing `info!` macro uses record_str for the message field when it's a literal.
    // But in practice, `info!("text")` goes through record_debug.
    // Let's verify the message is captured correctly either way.
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::info!("test message");
    })
    .await;

    assert_eq!(entries[0].message, "test message");
}

#[tokio::test]
async fn test_empty_message_falls_back_to_metadata_name() {
    // When no message field is provided, the visitor's message stays empty,
    // and the layer uses metadata.name() instead.
    // We can emit an event without a message-like field.
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::event!(tracing::Level::INFO, key = "val");
    })
    .await;

    assert_eq!(entries.len(), 1);
    // metadata.name() should be something non-empty (the event site).
    assert!(!entries[0].message.is_empty());
}

// ── strip_quotes behavior ────────────────────────────────────────────────

#[tokio::test]
async fn test_message_strip_quotes() {
    // record_debug with a string like "hello" produces `"hello"` (with quotes).
    // strip_quotes should remove them for the message field.
    // The tracing info!("text") macro goes through record_debug for the message.
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::info!("quoted message");
    })
    .await;

    // The message should NOT have surrounding quotes.
    assert_eq!(entries[0].message, "quoted message");
}

// ── SinkHandle ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_sink_handle_add_sink() {
    let (sink, entries, notify) = MockSink::new();
    let layer = LogCoreLayer::new(vec![], LogLevel::Info);

    // Add sink via handle.
    let handle = layer.sink_handle();
    handle.add_sink(Box::new(sink)).await;

    let subscriber = Registry::default().with(layer);
    let _guard = tracing::subscriber::set_default(subscriber);

    tracing::info!("after add");

    tokio::time::timeout(std::time::Duration::from_secs(2), notify.notified())
        .await
        .expect("timed out");

    let collected = entries.lock().await;
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].message, "after add");
}

#[tokio::test]
async fn test_sink_handle_clone_shares_sinks() {
    let (sink, entries, notify) = MockSink::new();
    let layer = LogCoreLayer::new(vec![Box::new(sink)], LogLevel::Info);
    let handle = layer.sink_handle();
    let _handle2 = handle.clone();

    // Both handles point to the same sink list.
    let subscriber = Registry::default().with(layer);
    let _guard = tracing::subscriber::set_default(subscriber);

    tracing::info!("via clone");

    tokio::time::timeout(std::time::Duration::from_secs(2), notify.notified())
        .await
        .expect("timed out");

    let collected = entries.lock().await;
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].message, "via clone");
}

// ── Metadata target propagation ──────────────────────────────────────────

#[tokio::test]
async fn test_target_propagated_as_module() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::info!(target: "my_crate::submodule", "target test");
    })
    .await;

    assert_eq!(entries[0].module, "my_crate::submodule");
}

// ── Context (JSON object) ────────────────────────────────────────────────

#[tokio::test]
async fn test_context_is_json_object() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::info!("msg");
    })
    .await;

    assert!(entries[0].context.is_object());
    // With no extra fields, context should be an empty object.
    assert_eq!(entries[0].context, json!({}));
}

// ── Multiple sinks ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_multiple_sinks_receive_same_entry() {
    let (sink1, entries1, notify1) = MockSink::new();
    let (sink2, entries2, notify2) = MockSink::new();

    let layer = LogCoreLayer::new(vec![Box::new(sink1), Box::new(sink2)], LogLevel::Info);
    let subscriber = Registry::default().with(layer);
    let _guard = tracing::subscriber::set_default(subscriber);

    tracing::info!("multi");

    tokio::time::timeout(std::time::Duration::from_secs(2), notify1.notified())
        .await
        .expect("timed out on sink1");
    tokio::time::timeout(std::time::Duration::from_secs(2), notify2.notified())
        .await
        .expect("timed out on sink2");

    let c1 = entries1.lock().await;
    let c2 = entries2.lock().await;
    assert_eq!(c1.len(), 1);
    assert_eq!(c2.len(), 1);
    assert_eq!(c1[0].message, "multi");
    assert_eq!(c2[0].message, "multi");
}

// ── Edge case: non-UTF-8 safe debug ──────────────────────────────────────

#[tokio::test]
async fn test_debug_field_with_special_chars() {
    let entries = collect_events(LogLevel::Debug, 1, || {
        tracing::info!(data = "line1\nline2\ttab", "msg");
    })
    .await;

    let ctx = &entries[0].context;
    let val = ctx.get("data").expect("data field should exist");
    // record_debug formats with {:?}, so newlines/tabs become escaped.
    let s = val.as_str().unwrap();
    assert!(s.contains("line1"));
    assert!(s.contains("line2"));
}
