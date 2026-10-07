use async_trait::async_trait;

use crate::LogEntry;

// `async_trait` rewrites the async fns into `Pin<Box<dyn Future …>>` and stamps
// `#[must_use]` on them; the boxed Future is already `must_use`, so clippy 1.99+
// (double_must_use) rejects the macro output. Allow on the trait covers the
// expansion. `unknown_lints` keeps pre-1.99 toolchains from rejecting the name.
#[allow(unknown_lints, clippy::double_must_use)]
#[async_trait]
pub trait LogSink: Send + Sync {
    async fn write(&self, entry: &LogEntry) -> anyhow::Result<()>;

    async fn flush(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
