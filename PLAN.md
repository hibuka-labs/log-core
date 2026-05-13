# log-core 实现计划

## 1. 定位

统一日志底座。提供 `LogSink` trait + `Logger` 组合器，支持多种后端（终端 / 文件 / 云端 / 组合），调用方不感知底层输出目标。

```
ops-agent / agent-core / db-agent
            │
            ▼
        Logger（组合多个 LogSink）
            │
    ┌───────┼────────┐
    ▼       ▼        ▼
ConsoleSink  FileSink  CloudSink（后续实现）
```

和 data-core 同一模式：**trait 定义在 log-core，业务 crate 只依赖 trait**。

## 2. Crate 结构

```
log-core/
├── Cargo.toml
└── src/
    ├── lib.rs            # pub mod + re-export
    ├── level.rs          # LogLevel 枚举
    ├── entry.rs          # LogEntry 结构体
    ├── sink.rs           # LogSink trait
    ├── logger.rs         # Logger 组合器 (Builder)
    ├── console.rs        # ConsoleSink — stderr 输出
    ├── file.rs           # FileSink — 滚动文件写入
    └── composite.rs      # CompositeSink — 组合多个 sink
```

### 2.1 依赖

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

| 依赖 | 用途 |
|------|------|
| `chrono` | 时间戳 |
| `tokio` | 异步写文件、后续 HTTP 上传 |
| 其他 | 同 data-core |

### 2.2 文件滚动方案

不引入第三方 rolling 库，自己实现最简单的方案：

- 文件名 `ops.log`
- 超过 `max_size`（默认 10MB）时 rename 为 `ops.1.log`
- 保留最近 N 个归档文件（默认 5 个）

## 3. 类型设计

### 3.1 LogLevel

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,   // 纯开发调试
    Info,    // 正常运行时信息
    Warn,    // 非预期但可恢复
    Error,   // 影响用户
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
    /// 写入一条日志
    async fn write(&self, entry: &LogEntry) -> anyhow::Result<()>;

    /// 刷新缓冲区（文件 sink 需要，终端 sink 可以空实现）
    async fn flush(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
```

## 4. Logger 组合器

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

    // 便捷方法
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

## 5. 三（四）个 Sink 实现

### 5.1 ConsoleSink

```rust
// 用 eprintln! 写 stderr，加颜色
// Debug → 灰色，Info → 默认，Warn → 黄色，Error → 红色
// 格式：[2026-05-13 14:30:00] [WARN] [agent::runtime] message {json context}

pub struct ConsoleSink;
```

### 5.2 FileSink

```rust
pub struct FileSink {
    // 内部：tokio::sync::Mutex<File>
    // 滚动逻辑：写入前检查大小，超限时 rotate
}

impl FileSink {
    pub fn new(path: impl Into<PathBuf>, max_size: u64, max_files: usize) -> Self;
}
```

### 5.3 CloudSink（二期实现）

调用方**不感知**背后是哪家云服务。只配置 URL + Key，log-core 内部统一处理。

环境变量约定（`.env`）：

```bash
LOG_CLOUD_URL=https://your-log-service.com/api/v1/logs
LOG_CLOUD_KEY=sk-xxxxxxxx
LOG_CLOUD_MIN_LEVEL=warn     # debug/info/warn/error，默认 warn
```

两种使用方式：

```rust
// 方式 1：零参数，全部从环境变量读
Logger::builder().console().file("ops.log").cloud().build();

// 方式 2：显式传参（适合不依赖 env 的场景）
Logger::builder().console().file("ops.log")
    .cloud_with("https://log.example.com/v1/ingest", "api-key-xxx", LogLevel::Warn)
    .build();
```

CloudSink 内部结构：

```rust
pub struct CloudSink {
    api_url: String,
    api_key: String,
    min_level: LogLevel,
    buffer: Arc<Mutex<Vec<LogEntry>>>,
    client: Option<reqwest::Client>,
}

impl CloudSink {
    /// 从环境变量创建，如果没配则返回 None（表示不上云）
    pub fn from_env() -> Option<Self>;

    /// 显式创建
    pub fn new(api_url: String, api_key: String, min_level: LogLevel) -> Self;
}
```

上报逻辑：
1. `write()` 时如果 level >= min_level，加入缓冲队列
2. 异步批量 POST 到 api_url（批量大小可配，默认 50 条或每 5 秒）
3. 支持重试（最多 3 次，指数退避）
4. 失败则丢弃，不影响本地日志

### 5.4 CompositeSink

```rust
// 实际就是 Logger 的内核 —— Logger 本质就是一个 CompositeSink
// 所以不需要单独的 CompositeSink 类型
// Logger 本身就是组合器，调用方直接 Logger::builder().console().file(...).build()
```

## 6. lib.rs 导出

```rust
pub use level::LogLevel;
pub use entry::LogEntry;
pub use sink::LogSink;
pub use logger::{Logger, LoggerBuilder};
pub use console::ConsoleSink;
pub use file::FileSink;
pub use cloud::CloudSink;
```

## 7. 业务层使用示例

```rust
use log_core::{Logger, LogLevel};
use serde_json::json;

// 开发环境 — 只输出终端
let logger = Logger::builder()
    .console()
    .min_level(LogLevel::Debug)
    .build();

// 产品环境 — 终端 + 文件
let logger = Logger::builder()
    .console()
    .file("ops.log")
    .min_level(LogLevel::Info)
    .build();

// 产品 + 云端上报 — 从 .env 自动读取配置
// .env:  LOG_CLOUD_URL=... LOG_CLOUD_KEY=...
let logger = Logger::builder()
    .console()
    .file("ops.log")
    .cloud()                        // ← 零参数，全部从环境变量读
    .min_level(LogLevel::Info)
    .build();

// 使用
logger.info("agent::runtime", "session created", json!({"session_id": 1}));
logger.warn("agent::ssh", "connection timed out", json!({"host": "1.2.3.4", "timeout_ms": 8000}));
logger.error("agent::plan", "step execution failed", json!({"step": 3, "error": "permission denied"}));

// 也支持 plain message（无 context 时传 null）
logger.info("agent::runtime", "agent started", json!(null));
```

## 8. agent-core 集成方式

agent-core **不需要依赖 log-core**。集成点有两种：

### 方案 A（推荐）：agent-core 不感知日志

agent-core 零变更。ops-agent 在 `run_turn_with_handler` 前后自行打日志：

```rust
logger.info("agent", "turn start", json!({"session_id": id, "input": input}));
runtime.run_turn_with_handler(id, input, handler).await?;
logger.info("agent", "turn complete", json!({"session_id": id}));
```

### 方案 B：agent-core 接受 Option<Logger>

agent-core 的 `AgentConfig` 加一个可选的 `logger: Option<Arc<Logger>>`。如果提供了，runtime 内部自动在关键节点打日志。不提供就不打。

**推荐 A**，保持 agent-core 零依赖。

## 9. 云端上报（二期）

### 设计原则

**调用方不感知云厂商**。配置是运维的事，写在 `.env` 里。log-core 内部读环境变量或接受显式参数，统一 HTTP POST 上报。

| 调用方要做的 | 调用方不需要做的 |
|-------------|----------------|
| `.cloud()` 一行 | 不用管是阿里云 SLS 还是腾讯云 CLS |
| 在 `.env` 配 `LOG_CLOUD_URL` / `LOG_CLOUD_KEY` | 不用管上报格式、重试、批量策略 |

### Builder 接口

```rust
// 零参数 — 全部从环境变量自动读取
Logger::builder().cloud().build();

// 显式传参
Logger::builder().cloud_with(url, key, level).build();
```

Builder 里 `.cloud()` 内部调用 `CloudSink::from_env()`：

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

如果环境变量没配，`from_env()` 返回 `None`，`.cloud()` 就跳过不添加这个 sink（不上云）。

### CloudSink 内部行为

1. `write()` 时如果 `level >= min_level`，加入缓冲队列
2. 异步批量 POST 到 api_url（批量大小可配，默认 50 条或每 5 秒）
3. 支持重试（最多 3 次，指数退避）
4. 请求体为标准 JSON 数组 `[{...entry...}]`
5. 失败则丢弃，不影响本地日志（不阻塞业务）

## 10. 终端输出格式

```
[2026-05-13 14:30:01] [INFO]  [agent::runtime] session created {"session_id":1}
[2026-05-13 14:30:05] [WARN]  [agent::ssh]     connection timed out {"host":"1.2.3.4"}
[2026-05-13 14:30:10] [ERROR] [agent::plan]    step execution failed {"step":3,"error":"..."}
```

## 11. 实现顺序

1. **level.rs** — `LogLevel` 枚举
2. **entry.rs** — `LogEntry` 结构体
3. **sink.rs** — `LogSink` trait
4. **console.rs** — `ConsoleSink`
5. **file.rs** — `FileSink`
6. **logger.rs** — `Logger` + `LoggerBuilder`
7. **lib.rs** — 模块导出
8. **Cargo.toml** — 依赖配置
9. **cargo check + test** — 验证编译
