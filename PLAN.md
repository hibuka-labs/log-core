# log-core Implementation Plan

## 1. Purpose

Unified logging foundation. Provides a `LogSink` trait + `Logger` combinator supporting multiple backends (terminal / file / cloud / composite), so callers don't need to know the underlying output target.

```
ops-agent / agent-core / db-agent
            │
            ▼
        Logger (composes multiple LogSinks)
            │
    ┌───────┼────────┐
    ▼       ▼        ▼
ConsoleSink  FileSink  CloudSink (future)
```

Same pattern as data-core: **trait defined in log-core, business crates only depend on the trait**.

## 2. Crate Structure

```
log-core/
├── Cargo.toml
└── src/
    ├── lib.rs            # pub mod + re-export
    ├── level.rs          # LogLevel enum
    ├── entry.rs          # LogEntry struct
    ├── sink.rs           # LogSink trait
    ├── logger.rs         # Logger combinator (Builder)
    ├── console.rs        # ConsoleSink — stderr output
    ├── file.rs           # FileSink — rolling file writer
    └── composite.rs      # CompositeSink — compose multiple sinks
```

### 2.1 Dependencies

```toml
[dependencies]
anyhow = "1"
async-trait = "0.1"
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "=1.52.2", features = ["full"] }

[dev-dependencies]
tempfile = "3"
```

| Dependency | Purpose |
|------------|---------|
| `chrono` | Timestamps |
| `tokio` | Async file writes, future HTTP uploads |
| Other | Same as data-core |

### 2.2 File Rolling Strategy

No third-party rolling library — implement the simplest approach:

- File name `ops.log`
- When exceeding `max_size` (default 10 MB), rename to `ops.1.log`
- Keep the most recent N archive files (default 5)

## 3. Type Design

### 3.1 LogLevel

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,   // Development debugging only
    Info,    // Normal runtime information
    Warn,    // Unexpected but recoverable
    Error,   // Affects users
}
```

### 3.2 LogEntry

```rust
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub level: LogLevel,
    pub module: &'static str,
    pub message: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub context: serde_json::Value,
    pub session_id: Option<String>,
}
```

### 3.3 LogSink trait

```rust
#[async_trait]
pub trait LogSink: Send + Sync {
    /// Write a log entry
    async fn write(&self, entry: &LogEntry) -> anyhow::Result<()>;

    /// Flush buffer (needed by file sink; console sink can be a no-op)
    async fn flush(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
```

## 4. Logger Combinator

```rust
pub struct Logger {
    sinks: Vec<Box<dyn LogSink>>,
    min_level: LogLevel,
}

impl Logger {
    pub fn builder() -> LoggerBuilder { ... }

    pub async fn log(&self, level: LogLevel, module: &'static str, message: impl Into<String>, context: Value) {
        if level < self.min_level { return; }
        let entry = LogEntry {
            level, module,
            message: message.into(),
            timestamp: chrono::Utc::now(),
            context,
            session_id: None,
        };
        for sink in &self.sinks {
            let _ = sink.write(&entry).await;
        }
    }

    // Convenience methods
    pub async fn debug(&self, module: &'static str, msg: &str, ctx: Value) { ... }
    pub async fn info(&self, module: &'static str, msg: &str, ctx: Value)  { ... }
    pub async fn warn(&self, module: &'static str, msg: &str, ctx: Value)  { ... }
    pub async fn error(&self, module: &'static str, msg: &str, ctx: Value) { ... }
}
```

### 4.1 Builder

```rust
pub struct LoggerBuilder {
    sinks: Vec<Box<dyn LogSink>>,
    min_level: LogLevel,
}

impl LoggerBuilder {
    pub fn new() -> Self { ... }
    pub fn min_level(mut self, level: LogLevel) -> Self { ... }
    pub fn console(mut self) -> Self { ... }
    pub fn file(mut self, path: &str) -> Self { ... }
    pub fn sink(mut self, sink: Box<dyn LogSink>) -> Self { ... }
    pub fn build(self) -> Logger { ... }
}
```

## 5. Three (Four) Sink Implementations

### 5.1 ConsoleSink

```rust
// Write to stderr via eprintln!, with colors
// Debug → gray, Info → default, Warn → yellow, Error → red
// Format: [2026-05-13 14:30:00] [WARN] [agent::runtime] message {json context}

pub struct ConsoleSink;
```

### 5.2 FileSink

```rust
pub struct FileSink {
    // Internal: tokio::sync::Mutex<File>
    // Rolling logic: check size before write, rotate if exceeded
}

impl FileSink {
    pub fn new(path: impl Into<PathBuf>, max_size: u64, max_files: usize) -> Self;
}
```

### 5.3 CloudSink (Phase 2)

Callers **don't know which cloud provider** is behind it. Just configure URL + Key — log-core handles everything internally.

Environment variable convention (`.env`):

```bash
LOG_CLOUD_URL=https://your-log-service.com/api/v1/logs
LOG_CLOUD_KEY=sk-xxxxxxxx
LOG_CLOUD_MIN_LEVEL=warn     # debug/info/warn/error, default warn
```

Two usage modes:

```rust
// Mode 1: zero args — all from environment variables
Logger::builder().console().file("ops.log").cloud().build();

// Mode 2: explicit args (for env-less scenarios)
Logger::builder().console().file("ops.log")
    .cloud_with("https://log.example.com/v1/ingest", "api-key-xxx", LogLevel::Warn)
    .build();
```

CloudSink internals:

```rust
pub struct CloudSink {
    api_url: String,
    api_key: String,
    min_level: LogLevel,
    buffer: Arc<Mutex<Vec<LogEntry>>>,
    client: Option<reqwest::Client>,
}

impl CloudSink {
    /// Create from env vars; returns None if not configured (no cloud upload)
    pub fn from_env() -> Option<Self>;

    /// Explicit creation
    pub fn new(api_url: String, api_key: String, min_level: LogLevel) -> Self;
}
```

Upload logic:
1. On `write()`, if level >= min_level, add to buffer queue
2. Async batch POST to api_url (batch size configurable, default 50 entries or every 5 seconds)
3. Retry support (max 3 attempts, exponential backoff)
4. Discard on failure — does not affect local logging

### 5.4 CompositeSink

```rust
// Essentially the kernel of Logger — Logger IS a CompositeSink
// So no separate CompositeSink type is needed
// Logger itself is the combinator: Logger::builder().console().file(...).build()
```

## 6. lib.rs Exports

```rust
pub use level::LogLevel;
pub use entry::LogEntry;
pub use sink::LogSink;
pub use logger::{Logger, LoggerBuilder};
pub use console::ConsoleSink;
pub use file::FileSink;
pub use cloud::CloudSink;
```

## 7. Business-Layer Usage Examples

```rust
use log_core::{Logger, LogLevel};
use serde_json::json;

// Development — terminal only
let logger = Logger::builder()
    .console()
    .min_level(LogLevel::Debug)
    .build();

// Production — terminal + file
let logger = Logger::builder()
    .console()
    .file("ops.log")
    .min_level(LogLevel::Info)
    .build();

// Production + cloud — auto-read from .env
// .env:  LOG_CLOUD_URL=... LOG_CLOUD_KEY=...
let logger = Logger::builder()
    .console()
    .file("ops.log")
    .cloud()                        // ← zero args, all from env
    .min_level(LogLevel::Info)
    .build();

// Usage
logger.info("agent::runtime", "session created", json!({"session_id": 1}));
logger.warn("agent::ssh", "connection timed out", json!({"host": "1.2.3.4", "timeout_ms": 8000}));
logger.error("agent::plan", "step execution failed", json!({"step": 3, "error": "permission denied"}));

// Also supports plain message (pass null when no context)
logger.info("agent::runtime", "agent started", json!(null));
```

## 8. agent-core Integration

agent-core **does not need to depend on log-core**. Two integration points:

### Option A (recommended): agent-core is log-agnostic

Zero changes to agent-core. ops-agent logs before/after `run_turn_with_handler`:

```rust
logger.info("agent", "turn start", json!({"session_id": id, "input": input}));
runtime.run_turn_with_handler(id, input, handler).await?;
logger.info("agent", "turn complete", json!({"session_id": id}));
```

### Option B: agent-core accepts Option<Logger>

Add optional `logger: Option<Arc<Logger>>` to agent-core's `AgentConfig`. If provided, runtime auto-logs at key points internally.

**Recommend A**, keeping agent-core zero-dependency.

## 9. Cloud Upload (Phase 2)

### Design Principle

**Callers are cloud-provider-agnostic**. Configuration is an ops concern, set in `.env`. log-core reads env vars or accepts explicit params, handles HTTP POST uniformly.

| Caller's responsibility | Not caller's responsibility |
|-------------------------|----------------------------|
| `.cloud()` — one line | Which cloud provider (Aliyun SLS / Tencent CLS / ...) |
| Set `LOG_CLOUD_URL` / `LOG_CLOUD_KEY` in `.env` | Upload format, retry, batching strategy |

### Builder Interface

```rust
// Zero args — all from env
Logger::builder().cloud().build();

// Explicit args
Logger::builder().cloud_with(url, key, level).build();
```

Inside Builder, `.cloud()` calls `CloudSink::from_env()`:

```rust
impl CloudSink {
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("LOG_CLOUD_URL").ok()?;
        let key = std::env::var("LOG_CLOUD_KEY").ok()?;
        let level = std::env::var("LOG_CLOUD_MIN_LEVEL")
            .ok()
            .and_then(|s| match s.as_str() {
                "debug" => Some(LogLevel::Debug),
                "info"  => Some(LogLevel::Info),
                "error" => Some(LogLevel::Error),
                _       => Some(LogLevel::Warn),
            })
            .unwrap_or(LogLevel::Warn);
        Some(Self::new(url, key, level))
    }
}
```

If env vars are not set, `from_env()` returns `None`, and `.cloud()` skips adding this sink (no cloud upload).

### CloudSink Internal Behavior

1. On `write()`, if level >= min_level, add to buffer queue
2. Async batch POST to api_url (batch size configurable, default 50 entries or every 5 seconds)
3. Retry support (max 3 attempts, exponential backoff)
4. Request body: standard JSON array `[{...entry...}]`
5. Discard on failure — does not block business logic

## 10. Terminal Output Format

```
[2026-05-13 14:30:01] [INFO]  [agent::runtime] session created {"session_id":1}
[2026-05-13 14:30:05] [WARN]  [agent::ssh]     connection timed out {"host":"1.2.3.4"}
[2026-05-13 14:30:10] [ERROR] [agent::plan]    step execution failed {"step":3,"error":"..."}
```

## 11. Implementation Order

1. **level.rs** — `LogLevel` enum
2. **entry.rs** — `LogEntry` struct
3. **sink.rs** — `LogSink` trait
4. **console.rs** — `ConsoleSink`
5. **file.rs** — `FileSink`
6. **logger.rs** — `Logger` + `LoggerBuilder`
7. **lib.rs** — Module exports
8. **Cargo.toml** — Dependency config
9. **cargo check + test** — Verify compilation
