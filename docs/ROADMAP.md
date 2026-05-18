# nixacp-gateway Roadmap

> **North star:** A local-first, Zed-native intelligent gateway that makes Qwen/DeepSeek/Ollama feel as capable as GPT-4 for coding — with reflection, tool use, and ACP streaming — running entirely on the developer's machine.

**Timeline:** 7 months | **Architecture reference:** `CLAUDE.md` | **Performance reference:** `docs/architecture/performance.md` | **Tool strategy:** `docs/architecture/adr-003-tool-strategy-native-plus-mcp.md`

---

## Milestone Calendar

| Month | Phases | Theme | Key Deliverable |
|---|---|---|---|
| 1 | 1 + 2 | Foundation | Streaming proxy live; Zed can chat via OpenAI-compat API |
| 2 | 3 | Tool Calls | Agentic loops with file read + search tools |
| 3 | 4 + Perf 2–3 | Intelligence | Reflection/retry + cancellation hardening |
| 4 | 5 | ACP | Full Zed IDE integration via ACP protocol |
| 5 | 6 + Perf 4–5 | Production | Observability, admission control, 1-alloc hot path |
| 6 | 7a + 7b | Tools Expansion | Native built-ins (write, edit, bash) + MCP client |
| 7 | 7c + Perf 6–7 | Scale | Multi-backend routing, HTTP/2, horizontal scaling |

---

## Phase 1 — Minimal Streaming Proxy ✅ DONE

**Duration:** Weeks 1–3 | **Perf:** Perf 1 (correct baseline)

**Entry criteria:** Cargo workspace initialized; Ollama reachable (remote or local).

**Deliverables:**
- Workspace `Cargo.toml` with 5 crates: `domain`, `application`, `infrastructure`, `api`, `logging`
- `domain`: `Message`, `Role`, `ConversationRequest`, `StreamChunk`, `LlmBackend` port trait, `BackendError`
- `infrastructure/ollama`: `OllamaClient` — HTTP POST `/api/chat`, NDJSON stream via `BytesMut`/`unfold`
- `api`: `POST /v1/chat/completions` (SSE + non-streaming), `GET /v1/models`, request/response logging middleware
- `logging`: [Standard Application Log v1.0](https://github.com/preedep/standard-app-log/blob/main/README.md) — `StdAppLog`, `APP_LOG` / `REQ_LOG` / `RES_LOG` / `REQ_EX_LOG` / `RES_EX_LOG` (PII_LOG excluded); configurable `json` / `text` format via `[log]` config section
- `src/main.rs`: figment config, DI wiring, custom Tokio runtime (physical cores, 512 KB stacks)
- `gateway.toml`: layered config overridable via `GATEWAY_*` env vars

**Key files:**
```
crates/domain/src/entities/{message,model,conversation,stream_chunk}.rs
crates/domain/src/ports/llm_backend.rs
crates/infrastructure/src/ollama/{client,stream,types}.rs
crates/api/src/{server,state,error,openai/chat,openai/models,middleware/logging}.rs
crates/logging/src/{types,layer}.rs
src/main.rs  |  gateway.toml
```

**Exit criteria — all met:**
- `curl -N localhost:8080/v1/chat/completions` streams SSE tokens from remote Ollama ✅
- Zed IDE receives a response via OpenAI-compat endpoint ✅
- `cargo test --workspace` passes (19 tests) ✅

---

## Phase 2 — Prompt Pipeline + Context Compression ✅ DONE

**Duration:** Weeks 4–5 | **Perf:** none (correctness only)

**Entry criteria:** Phase 1 exit criteria met.

**Deliverables:**
- `domain/ports`: `TokenCounter`, `ContextCompressor` traits
- `infrastructure/token_counter`: `TiktokenCounter` — `cl100k_base`, `OnceLock<Arc<CoreBPE>>` at startup (never per-request)
- `application/prompt`: `PromptPipeline`, `SystemPromptBuilder`, `ModelQuirksTransformer` (strip Qwen `<|im_start|>`, DeepSeek `<think>` leakage)
- `application/compression`: `CompressionService`, `SlidingWindowCompressor`
- Pipeline + compression wired into `ChatService.prepare()` before every backend call
- Real token counting wired into `complete()` (prompt_tokens + completion_tokens)

**Exit criteria — all met:**
- Conversation with 200+ messages stays within context window ✅ (compression_boundary_200_messages test)
- Qwen injection tokens stripped from user input ✅ (strips_qwen_tokens test)
- `cargo test --workspace` passes (36 tests, 0 warnings) ✅ (130 total after Phase 3)

---

## Phase 3 — Tool Calls ✅ DONE

**Duration:** Weeks 6–7 | **Perf:** none (correctness only)

**Entry criteria:** Phase 2 exit criteria met. ✅

**Deliverables:**
- `domain/entities`: `ToolDefinition`, `ToolCall`, `ToolResult` ✅ (3.1)
- `domain/ports`: `ToolRuntime` trait ✅ (3.1)
- `infrastructure/tools`: `ToolCallNormalizer` (OpenAI JSON + Qwen XML + DeepSeek markdown + bare JSON) ✅ (3.2)
- `infrastructure/ollama`: Ollama wire types extended with `OllamaToolCall`, `OllamaFunction` ✅ (3.2)
- `infrastructure/tools`: `ToolRegistry`, `ToolExecutor` (10s timeout, `FuturesUnordered`) ✅ (3.3)
- Built-in tools: `FileReadTool` (workspace root confinement via canonicalize, `../` rejected) ✅ (3.3)
- Built-in tools: `SearchTool` (ripgrep via `tokio::process::Command`, optional sub-path) ✅ (3.3)
- `application/tool_loop`: `ToolLoopOrchestrator` — detect → dispatch → inject → resubmit, max 5 passes ✅ (3.4)
- `api/openai`: extend wire types + route requests with `tools` through orchestrator ✅ (3.5)
- `wiremock` integration test: 3-tool-call agentic loop ✅ (3.5)

**Exit criteria — all met:**
- Gateway completes a 3-tool-call agentic loop end-to-end ✅
- `FileReadTool` returns `ToolError::Unauthorized` on `../` traversal ✅
- `cargo test -p infrastructure` passes (57 tests) ✅
- `cargo test -p application` passes (20 tests, includes 6 tool_loop tests) ✅
- `cargo test -p api --test tool_calls` passes (3 integration tests) ✅
- `cargo test --workspace` passes (134 tests, 0 warnings) ✅

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

## Phase 7a — Native Tools Expansion ⚙️ IN PROGRESS

**Duration:** Weeks 14–15 | **Perf:** none (correctness only)  
**Decision:** [ADR-003](architecture/adr-003-tool-strategy-native-plus-mcp.md)

**Entry criteria:** Phase 6 exit criteria met.

**Deliverables:**

*New native tools:*
- `infrastructure/tools/find.rs` — `FindTool` ✅: recursive file search by glob pattern with workspace confinement; `max_depth` support
- `infrastructure/tools/find.rs` — `ListTool` ✅: immediate directory listing, `[dir]/[file]` prefixes, sorted output
- `infrastructure/tools/file_write.rs` — `FileWriteTool`: create or overwrite a file inside workspace root; rejects paths outside workspace via `canonicalize`
- `infrastructure/tools/file_edit.rs` — `FileEditTool`: apply a unified diff patch to an existing file; uses `similar` crate for patch application
- `infrastructure/tools/bash.rs` — `BashTool`: allowlist-only shell execution; denies `;`, `|`, `&&`, `||`, `>`, `<`, `` ` ``, `$(...)`; 30-second timeout; working directory locked to workspace root

*Tool infrastructure (completed as part of 7a):*
- `domain/ports/tool_runtime.rs` — `ToolRuntime::definition()` ✅: every tool advertises its own JSON Schema; gateway injects all tool definitions on every request (Zed-sent tools ignored)
- `api/openai/chat.rs` + `responses.rs` ✅: tool loop result streamed as SSE (was returning `application/json`, invisible to Zed UI)
- `gateway.toml` — `workspace_root` ✅: configurable workspace confinement root; included in system prompt so model uses relative paths
- `infrastructure/ollama/client.rs` — `domain_messages()` ✅: assistant messages with tool calls now serialize `tool_calls` to Ollama; previously sent empty content with no `tool_calls` field causing Ollama to stall (502) on pass 2
- `application/prompt/system_prompt.rs` ✅: tool-use rules appended to existing Zed system message (was silently skipped when Zed sent its own system message, leaving model with no tool instructions)
- `api/openai/chat.rs` ✅: `apply_pipeline()` called before tool loop so system prompt injection reaches the model (tool loop bypasses `ChatService.prepare()`)
- `src/main.rs` — `GATEWAY_WORKSPACE_ROOT` env var ✅: figment config now correctly maps `GATEWAY_WORKSPACE_ROOT` → `workspace_root` (was broken by `split("_")` mapping it to nested key `workspace.root`); startup log confirms active workspace root

*Two-registry plumbing:*
- `application/tool_loop`: `ToolLoopOrchestrator` gains `McpToolRegistry` field (empty for now); resolution order: native first, MCP second
- `domain/ports`: `ToolRegistry` trait extracted so both registries share the same lookup interface

*Config:*
- `gateway.toml`: `[tools.bash]` block — `enabled = true/false`, `allowlist = [...]`
- `api/state.rs`: parse `ToolsConfig`, wire `BashTool` only when `enabled = true`

*Tests:*
- Unit tests for each new tool: path traversal rejection, allowlist enforcement, diff apply ✅ (15 tests for FindTool/ListTool)
- Integration test `tests/native_tools.rs`: write → read → bash loop via Axum router + wiremock

**Key files:**
```
crates/infrastructure/src/tools/{find,file_write,file_edit,bash}.rs
crates/domain/src/ports/tool_runtime.rs
crates/application/src/tool_loop/mod.rs
gateway.toml
crates/api/tests/native_tools.rs
```

**Exit criteria:**
- `FindTool` returns `ToolError::Unauthorized` on paths outside workspace ✅
- `ListTool` returns `ToolError::Unauthorized` on paths outside workspace ✅
- Tool loop response streamed as SSE to Zed ✅
- `workspace_root` configurable in `gateway.toml` ✅
- `GATEWAY_WORKSPACE_ROOT` env var correctly overrides `workspace_root` at runtime ✅
- Tool definitions injected with system prompt visible to model (Zed system message preserved + appended) ✅
- Zed agent chat: `list_dir` tool executes and returns directory listing end-to-end ✅
- `FileWriteTool` creates and overwrites files; rejects `../` traversal
- `FileEditTool` applies a valid unified diff; returns error on malformed patch
- `BashTool` executes `cargo --version`; rejects `rm -rf /` and `cat /etc/passwd | grep root`
- `cargo test --workspace` passes (0 warnings) ✅ (175 tests)

---

## Phase 7b — MCP Client Integration

**Duration:** Weeks 16–17 | **Perf:** none (correctness only)  
**Decision:** [ADR-003](architecture/adr-003-tool-strategy-native-plus-mcp.md)

**Entry criteria:** Phase 7a exit criteria met.

**Deliverables:**

*MCP client infrastructure:*
- `infrastructure/tools/mcp/client.rs` — `McpStdioClient`: spawn MCP server process, keep alive for gateway lifetime, restart once on failure; uses `rmcp` from the ACP SDK (already a transitive dep via `agent-client-protocol`)
- `infrastructure/tools/mcp/registry.rs` — `McpToolRegistry`: implements `ToolRegistry` port; lists tools from all connected MCP servers; dispatches calls by tool name prefix (`<server-name>/<tool>`)
- `infrastructure/tools/mcp/config.rs` — parse `[[tools.mcp]]` entries from `gateway.toml`

*Wiring:*
- `application/tool_loop`: `McpToolRegistry` populated from config and passed to orchestrator
- `api/state.rs`: spawn MCP child processes at startup; register `McpToolRegistry` shutdown in `CancellationToken` tree

*Config (`gateway.toml`):*
```toml
[[tools.mcp]]
name    = "git"
command = "npx"
args    = ["-y", "@modelcontextprotocol/server-git", "/workspace"]

[[tools.mcp]]
name    = "fetch"
command = "npx"
args    = ["-y", "@modelcontextprotocol/server-fetch"]
```

*Tests:*
- Unit: `McpToolRegistry` resolves tool names, handles server crash + restart
- Integration `tests/mcp_tools.rs`: mock MCP server (stdio), gateway calls it, result injected into conversation

**Key files:**
```
crates/infrastructure/src/tools/mcp/{client,registry,config}.rs
crates/api/src/state.rs
gateway.toml
crates/api/tests/mcp_tools.rs
```

**Exit criteria:**
- Gateway starts with `[[tools.mcp]]` entries and spawns MCP child processes
- Tool call routed to MCP server; result injected into conversation and resubmitted to model
- MCP server crash triggers one restart; second crash returns `ToolError::ExecutionFailed`
- `cargo test -p api --test mcp_tools` passes
- `cargo test --workspace` passes (0 warnings)

---

## Phase 7c — Multi-Backend, HTTP/2, Horizontal Scale

**Duration:** Weeks 18–26 | **Perf:** Perf 6 (HTTP/2 + pool tuning), Perf 7 (Redis session store)

**Entry criteria:** Phase 7b exit criteria met.

**Deliverables:**
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
| `BashTool` allowlist bypass via argument injection | Medium | High | Strip shell metacharacters before exec; use `tokio::process::Command` with explicit arg array (never shell string); test with adversarial inputs |
| MCP server version incompatibility (`rmcp` API drift) | Medium | Medium | Pin `rmcp` version; confine all MCP wiring to `infrastructure/tools/mcp/` — blast radius is one module |
| MCP child process leak on gateway crash | Low | Medium | Register MCP process handles in `CancellationToken` tree; use `kill_on_drop(true)` on `Child` |

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
- [ ] Native tools: `FindTool`, `FileWriteTool`, `FileEditTool`, `BashTool` — all workspace-confined and tested
- [ ] `BashTool` rejects shell metacharacter injection attempts
- [ ] MCP tool round-trip: gateway calls external MCP server, result injected into conversation
- [ ] MCP child process cleaned up within 2s of gateway shutdown
