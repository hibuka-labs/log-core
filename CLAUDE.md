# CLAUDE.md — log-core

Unified logging foundation: LogSink trait + Logger combinator.

## Rules

### Dependencies
- `Cargo.toml` uses **pure version deps** (no `path`).
- To debug: temporarily add `path`, **DO NOT commit**.

### Publishing
1. Bump version → commit → push → `cargo publish --registry crates-io`

### Downstream
- [ ] phi-agent (if used)
