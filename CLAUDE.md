# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Goal

Build a Rust-based intelligent ACP/OpenAI-compatible gateway for local coding LLMs (Qwen, DeepSeek, Ollama), optimized for Zed IDE and coding-agent workflows.

## Architecture: Clean Architecture

The project follows strict Clean Architecture with dependency rules enforced at the crate boundary level. Dependencies flow inward only — outer layers depend on inner layers, never the reverse.

```
┌─────────────────────────────────────────────┐
│  Infrastructure (adapters, drivers, I/O)    │  ← HTTP handlers, DB, Ollama client
├─────────────────────────────────────────────┤
│  Application (use cases, orchestration)     │  ← Routing logic, retry, compression
├─────────────────────────────────────────────┤
│  Domain (entities, ports, value objects)    │  ← Model, Message, Tool, Route traits
└─────────────────────────────────────────────┘
```

### Crate Layout (workspace)

```
nixacp-gateway/
├── Cargo.toml                  # workspace root
├── gateway.toml                # runtime config (Ollama URL, log format, models)
├── crates/
│   ├── domain/                 # pure domain — zero external I/O dependencies
│   │   └── src/
│   │       ├── entities/       # Model, Message, ConversationRequest, StreamChunk
│   │       └── ports/          # traits: LlmBackend, ToolRuntime, Router
│   ├── application/            # use cases, depends only on domain
│   │   └── src/
│   │       ├── chat.rs         # ChatService (Phase 2 adds prompt pipeline here)
│   │       ├── routing/        # multi-model router (Phase 2+)
│   │       ├── reflection/     # retry-on-failure loop (Phase 4)
│   │       ├── compression/    # context window management (Phase 2)
│   │       └── prompt/         # prompt optimization pipeline (Phase 2)
│   ├── infrastructure/         # implements domain ports
│   │   └── src/
│   │       ├── ollama/         # OllamaClient — LlmBackend impl, NDJSON stream
│   │       ├── openai/         # OpenAI-compat passthrough (Phase 7)
│   │       ├── acp/            # ACP protocol adapter (Phase 5)
│   │       └── tools/          # extensible tool runtime (Phase 3)
│   ├── api/                    # HTTP server, Axum router
│   │   └── src/
│   │       ├── openai/         # /v1/chat/completions, /v1/models
│   │       ├── middleware/     # request/response logging (StdAppLog)
│   │       ├── acp/            # ACP endpoints (Phase 5)
│   │       ├── state.rs        # AppState, Config, LogConfig
│   │       ├── error.rs        # AppError → OpenAI error JSON
│   │       └── server.rs       # build_router
│   └── logging/                # Standard Application Log v1.0 types + subscriber
│       └── src/
│           ├── types.rs        # StdAppLog, LogType, LogLevel, LogRequest, LogResponse
│           └── layer.rs        # init_subscriber, LogFormat (json/text)
└── src/
    └── main.rs                 # Tokio runtime, figment config, DI wiring
```

## Key Technical Requirements

| Concern | Approach |
|---|---|
| Async runtime | Tokio (multi-threaded) |
| HTTP server | Axum |
| HTTP client | reqwest with streaming |
| Streaming | Server-Sent Events (SSE) via `axum::response::Sse` |
| Serialization | serde / serde_json |
| Observability | tracing + tracing-subscriber + opentelemetry |
| Config | config-rs or figment (layered: file → env → CLI) |
| Error handling | thiserror in domain/application, anyhow at binary boundary |

## Protocol Support

- **OpenAI-compatible API** — `/v1/chat/completions`, `/v1/models`, `/v1/completions` (streaming + non-streaming)
- **ACP (Agent Communication Protocol)** — message passing for agent-to-agent and Zed IDE integration
- **Tool-call normalization** — translate between OpenAI function-calling format and model-native formats (Qwen, DeepSeek)

## Core Features

- **Multi-model router** — route requests by model name, task type, or capability tag; defined as a `Router` port in domain
- **Reflection retry** — if a response fails a quality check (e.g., malformed tool call, incomplete code), re-submit with reflection prompt; max retries configurable
- **Context compression** — sliding window + summarization to fit context within model's token limit; pluggable strategy via trait
- **Prompt optimization** — transform user prompts for coding tasks (add language hints, strip noise, inject system context)
- **Local-first** — Ollama is the primary backend; no cloud dependency required; enterprise auth optional

## Development Commands

```bash
# Build all crates
cargo build --workspace

# Run the gateway
cargo run -p nixacp-gateway

# Run all tests
cargo test --workspace

# Run tests for a specific crate
cargo test -p domain
cargo test -p application

# Run a single test by name
cargo test -p application routing::tests::test_model_selection

# Lint
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --all

# Check without building
cargo check --workspace
```

## Testing

### Unit tests
- Live in the same file as the code under test (`#[cfg(test)]` module at the bottom).
- Test domain logic and application use-cases in isolation — no I/O, no HTTP, no real Ollama.
- Mock `LlmBackend`, `ToolRuntime`, `SessionStore` ports using simple `struct FakeBackend` impls, not `mockall` — keeps tests readable.
- Run: `cargo test -p domain` / `cargo test -p application`
- Run one test: `cargo test -p application reflection::tests::test_quality_check_incomplete_code`

### Integration tests
- Live in `crates/<crate>/tests/` (Rust integration test convention).
- Use `wiremock` to mock Ollama HTTP endpoints — never require a live Ollama process in CI.
- Test full request/response cycles through the Axum router using `axum::http::Request` + `tower::ServiceExt::oneshot`.
- One test file per feature area: `tests/streaming.rs`, `tests/tool_calls.rs`, `tests/acp_session.rs`, `tests/reflection.rs`.
- Run: `cargo test -p api --test streaming`

### Benchmark tests
- Live in `crates/<crate>/benches/` using `criterion`.
- Required benchmarks (must be kept green before any Phase 5+ memory optimization):
  - `benches/hot_path.rs` — measures allocs/token in the SSE hot path (target: ≤ 2)
  - `benches/message_arena.rs` — `MessageArena::push` and `content_slice` throughput
  - `benches/token_counting.rs` — tiktoken throughput at 4K / 32K / 128K token inputs
  - `benches/json_serialization.rs` — `ConversationRequest` serialization at varying context sizes
- Run: `cargo bench -p infrastructure`
- Run one benchmark: `cargo bench -p infrastructure --bench hot_path`
- Profile with: `cargo bench -p infrastructure -- --profile-time=5` (criterion built-in flamegraph)

### Test commands summary
```bash
cargo test --workspace                        # all unit + integration tests
cargo test -p domain                          # domain unit tests only
cargo test -p application                     # application unit tests only
cargo test -p api --test streaming            # single integration test file
cargo bench --workspace                       # all benchmarks
cargo bench -p infrastructure --bench hot_path
```

## Coding Conventions

- All async functions use `async fn` with Tokio; no blocking calls on the async executor
- Domain traits (`LlmBackend`, `Router`, `ToolRuntime`) are `async_trait` bounds
- Errors: `thiserror` enums in `domain` and `application`; `anyhow::Result` only in `api` and `main`
- SSE chunks must follow the OpenAI delta format: `data: {json}\n\ndata: [DONE]\n\n`
- Tool calls are normalized at the infrastructure layer before entering application logic
- Config is injected via constructor (no global state); use `Arc<Config>` for shared access

## Comment Policy

Write comments only when the **why** is non-obvious. Acceptable comment targets:

- **Hidden constraints** — a platform quirk, a protocol edge case, or an upstream bug that forced an unusual decision (e.g., "Ollama sends a final empty chunk after `done: true` — consume it to avoid a spurious StreamParse error")
- **Subtle invariants** — ordering requirements or aliasing rules that a future reader would need to know before touching the code (e.g., "acquire session lock, snapshot messages, *then* release before calling spawn_blocking — never hold the lock across an await")
- **Workarounds** — a deliberate hack with a ticket or explanation (e.g., "reqwest 0.12 leaks connections on early drop; manually abort the response body here until #1234 is fixed")
- **Non-obvious performance choices** — when the naïve alternative would be measurably worse and the reader might reach for it (e.g., "BytesMut::split_to avoids a memcpy; do not replace with `drain(..pos)` which copies")

**Do not comment:**
- What the code does (well-named functions and variables already say this)
- Standard Rust patterns (`impl From<X> for Y`, `#[derive(Debug)]`, `Arc::clone`)
- Temporary task context ("added for Phase 3", "called from chat handler") — this belongs in commit messages
- The current date, author, or ticket number in source files

**Docstrings:** Only on public API surfaces consumed by other crates — one sentence max. Never multi-paragraph. Avoid restating the function signature.

## Application Logging Standard

All structured logs follow [Standard Application Log v1.0](https://github.com/preedep/standard-app-log/blob/main/README.md). The `crates/logging` crate owns the types and subscriber setup. **PII_LOG is not used.**

### Log types emitted

| LogType      | Where                                              |
|--------------|----------------------------------------------------|
| `APP_LOG`    | Startup / shutdown (`main.rs`)                     |
| `REQ_LOG`    | Every inbound HTTP request (middleware)            |
| `RES_LOG`    | Every HTTP response sent to caller (middleware)    |
| `REQ_EX_LOG` | Every outgoing request to Ollama (`OllamaClient`)  |
| `RES_EX_LOG` | Every response received from Ollama (`OllamaClient`) |

### `[log]` config block (`gateway.toml`)

```toml
[log]
format      = "json"          # "json" | "text"  (default: "json")
level       = "info"          # tracing filter string
app_id      = "nixacp-gateway"
app_version = "0.1.0"
```

Override at runtime: `GATEWAY_LOG_FORMAT=text GATEWAY_LOG_LEVEL=debug cargo run`

### Key rules

- `StdAppLog::emit()` serialises to one JSON line and forwards to `tracing::info!` — works with both JSON and text subscribers.
- Never log full request/response bodies for SSE streams (too large, chunked).
- Never log Ollama message content — only model name and message count (`REQ_EX_LOG`).
- Strip `Authorization` and `Cookie` headers before logging.
- `correlation_id`: read from `X-Correlation-Id` header, or generate a new UUID if absent.
- `request_id`: always a fresh UUID per request; injected into the response as `X-Request-Id`.

## Zed IDE Integration Notes

- The ACP endpoint is the primary integration point for Zed's agent panel
- Streaming must be low-latency; avoid buffering entire responses
- Tool results should round-trip within a single ACP session without re-authentication

## ACP Protocol Reference

- Spec site: https://agentclientprotocol.com — JSON-RPC over stdio (local agents) / HTTP+SSE (remote agents)
- Core concepts: sessions, runs, tool execution, slash commands, workspace metadata
- **Before implementing ACP wire types in Phase 5:** check for the `acp-sdk` Rust crate on crates.io first. Use it if available; only hand-implement from the OpenAPI spec if no SDK exists.
- All ACP wire types live exclusively in `infrastructure/acp` behind the `AcpAdapter`. Changes to the spec are isolated there and never touch `domain` or `application`.
- ACP is actively evolving — the HTTP remote-agent support is explicitly marked "work in progress" on the spec site.

## Roadmap

Development is organized in 7 phases over 6 months. See `docs/ROADMAP.md` for the full roadmap including phase deliverables, exit criteria, and the definition of done.

**Phase summary:**
1. Minimal streaming proxy (Weeks 1–3)
2. Prompt pipeline + compression (Weeks 4–5)
3. Tool calls (Weeks 6–7)
4. Reflection/retry + cancellation hardening (Weeks 8–9)
5. ACP protocol / Zed integration (Weeks 10–11)
6. Observability + performance hot path (Weeks 12–13)
7. MCP + multi-backend + horizontal scale (Weeks 14–26)

## Performance Rules

These rules are non-negotiable. Violating them introduces measurable regressions. See `docs/architecture/performance.md` for the full analysis behind each decision.

### Async / Task model
- The `ReflectionOrchestrator` task IS the stream pump — do not spawn a separate stream-pump task. One task drives `reqwest::bytes_stream()`, forwards chunks to `mpsc::Sender`, and runs quality checks inline.
- Reflection passes are **sequential**, never concurrent. Only one `ConversationRequest` lives in memory at a time per session.
- Use `tokio::task::spawn_blocking` for: tiktoken counting, `serde_json` serialization of payloads > 64 KB, `minijinja` rendering, regex quality checks on responses > 100 KB, and `DashMap::retain` in background tasks. Never run these on the async executor.
- All shell tools use `tokio::process::Command`, never `spawn_blocking(|| std::process::Command::...)`. Only async process handles are cancellation-safe.
- Use `FuturesUnordered` for concurrent tool dispatch, not `tokio::join!`. Results are consumed as they arrive; stragglers are cancelled independently.
- Load `tiktoken_rs` BPE tables once at startup into `OnceLock<Arc<CoreBPE>>`. Never call `get_bpe_from_model()` per-request.

### Cancellation
- Every stream-pump loop must use `tokio::select!` between `cancel_token.cancelled()` and the next chunk. Never rely on `mpsc::SendError` alone to detect client disconnect — Ollama may not send a chunk for 30+ seconds.
- Structure all spawned tasks under a `CancellationToken` tree: `gateway_cancel → request_cancel → orchestrator_cancel → tool_cancel[N]`. Drop-based `StreamGuard` fires cancel and returns the Ollama semaphore permit atomically.
- Wrap `mpsc::Sender::send()` with a 60-second timeout. A permanently-full channel means the client is dead.

### Locks and shared state
- `BackendRegistry` and `ToolRegistry` use `arc_swap::ArcSwap<HashMap<...>>` — never `RwLock`. Readers take a pointer load (~8 ns), never block.
- `AcpSession` conversation history uses `tokio::sync::RwLock<MessageArena>` (async, not `std::sync`). Never hold it across a `.await` point longer than needed.
- Token counting (`TiktokenCounter`) must **never** be called while holding the session lock. Snapshot messages, release lock, count in `spawn_blocking`, reacquire to write.

### Memory
- Message storage uses `MessageArena` (contiguous `String` + `Vec<MessageHeader>`), not `Vec<Message>`. `MessageHeader` is 20 bytes. Snapshots for reflection are `Arc::clone`, never data clones.
- NDJSON line accumulation uses a per-connection `NdjsonAccumulator` (`BytesMut`, 8 KB initial). Call `.reset()` between reflection passes — retains capacity, avoids reallocation.
- Use `smol_str::SmolStr` for content delta fields in `OllamaStreamChunk`. Avoids heap allocation for token deltas ≤ 22 bytes (covers 99%+ of cases).
- SSE event data: `String::with_capacity(80)` + `serde_json::to_writer` into a thread-local `Vec<u8>`. Target: **1 heap allocation per streamed token** in the hot path.
- Per-session hard limits: 8 MB `MessageArena::content`, 4,096 messages, 50 MB total. Enforce on every `push()`; trigger compression at 4,096 messages, hard-reject at 8 MB.

### Concurrency limits
- Gate all Ollama calls with a `tokio::sync::Semaphore` sized to the backend's GPU concurrency (config: `ollama.max_concurrent`, default `2`). This is separate from the reqwest connection pool size (`pool_max_idle_per_host = max_concurrent × 2`).
- Global admission: `Semaphore(150)` at the `AdmissionControl` middleware. On `try_acquire` failure → HTTP 503 with `Retry-After: 10`. No queueing at the gateway level.
- Limit concurrent reflection sessions with `Semaphore(20)`. Sessions that cannot acquire skip reflection and return the direct response.
- `mpsc::channel` capacity: **32** (not 64). Provides backpressure sooner; 32 × ~128 B = 4 KB max per-session buffer.

### Timeouts (non-negotiable hierarchy)
```
Per-full-request   600s  (Axum middleware)
  Per-reflection   120s  (ReflectionOrchestrator)
    Per-backend     90s  (OllamaClient::stream)
      Per-chunk     30s  (stream.next() in pump loop)
    Per-tool        10s  (ToolExecutor)
  Quality check      5s  (QualityChecker)
```
