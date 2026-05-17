# nixacp-gateway Roadmap

> **North star:** A local-first, Zed-native intelligent gateway that makes Qwen/DeepSeek/Ollama feel as capable as GPT-4 for coding — with reflection, tool use, and ACP streaming — running entirely on the developer's machine.

**Timeline:** 6 months | **Architecture reference:** `CLAUDE.md` | **Performance reference:** `docs/architecture/performance.md`

---

## Milestone Calendar

| Month | Phases | Theme | Key Deliverable |
|---|---|---|---|
| 1 | 1 + 2 | Foundation | Streaming proxy live; Zed can chat via OpenAI-compat API |
| 2 | 3 | Tool Calls | Agentic loops with file read + search tools |
| 3 | 4 + Perf 2–3 | Intelligence | Reflection/retry + cancellation hardening |
| 4 | 5 | ACP | Full Zed IDE integration via ACP protocol |
| 5 | 6 + Perf 4–5 | Production | Observability, admission control, 1-alloc hot path |
| 6 | 7 + Perf 6–7 | Scale | MCP, multi-backend, HTTP/2, horizontal scaling |

---

## Phase 1 — Minimal Streaming Proxy

**Duration:** Weeks 1–3 | **Perf:** Perf 1 (correct baseline)

**Entry criteria:** Cargo workspace initialized; Ollama running locally.

**Deliverables:**
- Workspace `Cargo.toml` with 4 crates: `domain`, `application`, `infrastructure`, `api`
- `domain`: `Message`, `Role`, `ConversationRequest`, `StreamChunk`, `LlmBackend` port trait
- `infrastructure/ollama`: `OllamaClient` — HTTP POST `/api/chat`, NDJSON stream parsing
- `api`: `POST /v1/chat/completions` (SSE + non-streaming), `GET /v1/models`
- `src/main.rs`: figment config, DI wiring, Tokio runtime, graceful shutdown skeleton
- Basic `tracing_subscriber` console output

**Key files:**
```
crates/domain/src/entities/{message,model,conversation,stream_chunk}.rs
crates/domain/src/ports/llm_backend.rs
crates/infrastructure/src/ollama/{client,stream,types}.rs
crates/api/src/{server,state,openai/chat,openai/models}.rs
src/main.rs  |  gateway.toml
```

**Exit criteria:**
- `curl -N localhost:8080/v1/chat/completions` streams tokens from Ollama
- Zed IDE receives a response via OpenAI-compat endpoint
- `cargo test -p domain && cargo test -p infrastructure` pass

---

## Phase 2 — Prompt Pipeline + Context Compression

**Duration:** Weeks 4–5 | **Perf:** none (correctness only)

**Entry criteria:** Phase 1 exit criteria met.

**Deliverables:**
- `domain/ports`: `TokenCounter`, `ContextCompressor` traits
- `infrastructure/token_counter`: `TiktokenCounter` — `cl100k_base`, `OnceLock<Arc<CoreBPE>>` at startup (never per-request)
- `application/prompt`: `PromptPipeline`, `SystemPromptBuilder`, `ModelQuirksTransformer` (strip Qwen `<|im_start|>`, DeepSeek `<think>` leakage)
- `application/compression`: `CompressionService`, `SlidingWindowCompressor`
- Pipeline + compression wired into request path before every backend call

**Exit criteria:**
- Conversation with 200+ messages stays within context window (no 400 from Ollama)
- Qwen injection tokens stripped from user input
- `cargo test -p application` passes including compression boundary test

---

## Phase 3 — Tool Calls

**Duration:** Weeks 6–7 | **Perf:** none (correctness only)

**Entry criteria:** Phase 2 exit criteria met.

**Deliverables:**
- `domain/entities`: `ToolDefinition`, `ToolCall`, `ToolResult`
- `domain/ports`: `ToolRuntime` trait
- `infrastructure/tools`: `ToolCallNormalizer` (OpenAI JSON + Qwen XML + DeepSeek markdown fence), `ToolRegistry`, `ToolExecutor`
- Built-in tools: `FileReadTool` (workspace allowlist, path traversal rejected), `SearchTool` (ripgrep)
- Tool call loop in streaming path: `finish_reason: tool_calls` → concurrent dispatch via `FuturesUnordered` → inject `Role::Tool` messages → re-submit
- `wiremock` integration test: 3-tool-call agentic loop

**Exit criteria:**
- Gateway completes a 3-tool-call agentic loop end-to-end
- `FileReadTool` returns `ToolError::Unauthorized` on `../` traversal
- `cargo test -p api --test tool_calls` passes

---

## Phase 4 — Reflection / Retry + Cancellation Hardening

**Duration:** Weeks 8–9 | **Perf:** Perf 2 (cancellation + timeouts), Perf 3 (ArcSwap + async RwLock)

**Entry criteria:** Phase 3 exit criteria met.

**Deliverables:**

*Reflection:*
- `application/reflection`: `QualityChecker` chain — `IncompleteCodeCheck`, `MalformedToolCallCheck`, `EmptyResponseCheck`, `TruncatedResponseCheck`
- `ReflectionOrchestrator`: max 3 passes, sequential (one `ConversationRequest` in memory at a time)
- `CircuitBreaker` per backend (AtomicU8 state, 5 failures → open, 60s timeout)
- `application/compression`: `SummarizationCompressor`

*Cancellation hardening:*
- `CancellationToken` tree: `gateway → request → orchestrator → tool[N]`
- `select! { cancel.cancelled() / stream.next() }` in every stream-pump loop
- `StreamGuard` (drop → cancel + return Ollama semaphore permit)
- 60s timeout on `mpsc::send`; `SO_KEEPALIVE` (30/10/3) on accepted sockets

*Lock contention:*
- `BackendRegistry` + `ToolRegistry` → `arc_swap::ArcSwap`
- `AcpSession` history → `tokio::sync::RwLock<MessageArena>` (async, never blocking executor)
- Timeout hierarchy from `CLAUDE.md` enforced at all layers

**Exit criteria:**
- Deliberately truncated response triggers reflection; complete output produced
- Client disconnect mid-stream → Ollama connection closed within 2s
- `cargo test -p application --test reflection` passes
- No task leak under drop test

---

## Phase 5 — ACP Protocol (Zed Integration)

**Duration:** Weeks 10–11 | **Perf:** none (correctness focus)

**Entry criteria:** Phase 4 exit criteria met. Check for `acp-sdk` Rust crate on crates.io before implementing wire types; use SDK if available, otherwise implement from the OpenAPI spec at https://agentclientprotocol.com.

**Deliverables:**
- `infrastructure/acp`: `AcpSession`, `AcpSessionStore` (DashMap + 30-min TTL eviction), `AcpAdapter` (ACP ↔ domain type mapping), message codec

**ACP endpoints:**
| Endpoint | Purpose |
|---|---|
| `POST /acp/v1/sessions` | Create session; store `WorkspaceMetadata` from Zed handshake |
| `POST /acp/v1/sessions/{id}/runs` | Start a run (≈ chat completions) |
| `GET /acp/v1/sessions/{id}/runs/{rid}/events` | SSE stream of run events |
| `POST /acp/v1/sessions/{id}/runs/{rid}/tool-results` | Inject client-side tool results |

- `application/prompt`: `CodeContextInjector` reads `AcpSession.workspace_meta` (open files, language, git root)
- Session eviction background task: `DashMap::retain` in `spawn_blocking` every 60s
- `X-Acp-Session-Id` header propagated in all ACP responses

**Exit criteria:**
- Zed IDE agent panel: chat works, streaming works, workspace file context injected in system prompt
- Session evicted after idle TTL (verified with shortened TTL in test)
- `cargo test -p api --test acp_session` passes

---

## Phase 6 — Observability, Hardening, Performance Hot Path

**Duration:** Weeks 12–13 | **Perf:** Perf 4 (admission control), Perf 5 (1-alloc hot path)

**Entry criteria:** Phase 5 exit criteria met.

**Deliverables:**

*Observability:*
- `api/middleware`: `TraceLayer` (OTLP → Jaeger), `MetricsLayer` (Prometheus `/metrics`), `RateLimitLayer` (governor), `AdmissionControl` (`Semaphore(150)`, 503 + `Retry-After: 10`)
- `GET /health`, `GET /health/live`, `GET /health/ready`
- Structured JSON logging (`tracing_subscriber::fmt::json()`)

*Concurrency gates:*
- Ollama: `Semaphore(config.ollama.max_concurrent)` in `OllamaClient`
- Reflection: `Semaphore(20)`

*Memory hot path:*
- `MessageArena` replaces `Vec<Message>` (20-byte `MessageHeader`, contiguous `content: String`)
- `SmolStr` for `OllamaStreamChunk::content` (inline ≤ 22 bytes → 0 heap allocs for 99% of token deltas)
- `NdjsonAccumulator` per-connection `BytesMut` with `reset()` between reflection passes
- Thread-local `Vec<u8>` (capacity 128 bytes) for SSE `serde_json::to_writer`

*Security:*
- `ApiKey` newtype: redacting `Display`/`Debug`, `subtle::ConstantTimeEq`
- Backend tokens in `secrecy::SecretString`

*Benchmarks (all must pass):*
- `benches/hot_path.rs` — ≤ 2 allocs per token delta
- `benches/message_arena.rs` — `push` + `content_slice` throughput
- `benches/token_counting.rs` — tiktoken at 4K / 32K / 128K tokens
- `benches/json_serialization.rs` — `ConversationRequest` at varying context sizes

**Exit criteria:**
- `cargo bench -p infrastructure --bench hot_path` shows ≤ 2 allocs/token
- `/metrics` returns all defined Prometheus metrics
- `/health/ready` returns 503 when all backends unhealthy
- No API key or backend credential appears in any log output

---

## Phase 7 — MCP, Multi-Backend, Horizontal Scale

**Duration:** Weeks 14–26 | **Perf:** Perf 6 (HTTP/2 + pool tuning), Perf 7 (Redis session store)

**Entry criteria:** Phase 6 exit criteria met; SLOs verified under load.

**Deliverables:**
- `infrastructure/tools`: `McpToolRuntime` — connect to MCP servers, expose tools via `ToolRuntime` port
- `application/routing`: `RoundRobinStrategy`, `LeastLoadStrategy`, capability-based routing
- `infrastructure/openai_proxy`: `OpenAiProxyClient` (cloud fallback)
- Admin API: `GET/POST/DELETE /admin/backends`
- `BackendHealthPoller` background task (30s per backend)
- HTTP/2: `axum-server` + `rustls` + `h2`; `reqwest` with `tcp_nodelay(true)`, `SO_SNDBUF 256KB`, `pool_max_idle = max_concurrent × 2`
- `SessionStore` trait: `LocalSessionStore` (existing) + `RedisSessionStore` (`deadpool_redis`)
- Consistent-hash `X-Acp-Session-Id` routing (see `docs/architecture/performance.md § D.7`)

**Exit criteria:**
- P95 TTFB < 2,000ms at 20 concurrent sessions (Ollama mock)
- Gateway overhead P95 < 20ms
- 100 concurrent SSE sessions < 256 MB RSS
- `RedisSessionStore` passes all `acp_session` integration tests
- MCP tool round-trip: gateway calls external MCP server, injects result into conversation
- `oha -c 100 -z 60s` load test completes with < 1% 5xx

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| ACP spec evolves mid-project | High | Medium | All ACP wire types behind `AcpAdapter` in `infrastructure/acp`; spec changes isolated there |
| Ollama tool call schema changes | Medium | High | `ToolCallNormalizer` is the single translation point; add version header sniffing to `OllamaClient` |
| tiktoken-rs approximation causes over-compression | Medium | Low | Use `cl100k_base` conservatively; document in `TiktokenCounter` that it over-estimates for Qwen/DeepSeek |
| Zed ACP client behaviour diverges from spec | High | High | Test against actual Zed binary in Phase 5 — not just mock unit tests |
| `MessageArena` migration breaks reflection loop | Medium | High | Keep `Vec<Message>` fallback behind `config.experimental.message_arena = false` during Phase 6 transition |

---

## Definition of Done

All items must be true before the project is considered complete:

- [ ] Zed IDE agent panel: chat, file context, tool use, streaming — all working via ACP
- [ ] `POST /v1/chat/completions` works with streaming and tool calls (OpenAI-compat)
- [ ] Reflection/retry: ≥ 95% success rate on truncated-response test suite
- [ ] 100 concurrent SSE sessions < 256 MB RSS
- [ ] P95 TTFB < 2,000ms at 20 concurrent sessions (Ollama mock)
- [ ] Gateway overhead P95 < 20ms
- [ ] Hot path: ≤ 2 allocs/token (`cargo bench`)
- [ ] `cargo test --workspace` fully green (unit + integration + benchmarks)
- [ ] OTEL traces visible in Jaeger; all Prometheus metrics on `/metrics`
- [ ] Client disconnect cleans up within 2s (Ollama connection closed)
- [ ] No credentials in logs; `FileReadTool` path traversal rejected
