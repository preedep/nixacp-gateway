# Performance Architecture Reference

> Actionable rules live in `CLAUDE.md § Performance Rules`. This doc explains **why**.

---

## Bottlenecks

| # | Bottleneck | Worst case (100 sessions) | Fix |
|---|---|---|---|
| A.1 | Tokio task contention | 6,400 live tasks in full reflection | Orchestrator = stream pump (no extra task). Physical core count for `worker_threads`. |
| A.2 | Session `Mutex` held during tiktoken | 50 ms lock on 128K token count | Snapshot → release lock → `spawn_blocking` count → reacquire to write |
| A.3 | 5 allocs per SSE chunk | 7,500 allocs/sec at 15 tok/s | `SmolStr` + thread-local `Vec<u8>` → **1 alloc/token** |
| A.4 | `serde_json::to_string` on 512 KB request | 5–50 ms on async executor | `to_writer` into pre-alloc `Vec<u8>`; serialize > 64 KB in `spawn_blocking` |
| A.5 | 100 TCP connections for 100 SSE sessions | HOL blocking risk | Enable HTTP/2 (single highest-impact change) |
| A.6 | `ConversationRequest` clone per reflection | 6 MB × 3 passes × 100 sessions = 1.8 GB | `MessageArena` + `Arc::clone` snapshot = 8 bytes |
| A.7 | `tokio::join!` waits for slowest tool | 1 slow tool blocks entire pass | `FuturesUnordered` + `tokio::process::Command` + 10s timeout |
| A.8 | Reflection amplification | 3 × 10 = 30 backend calls/request | `Semaphore(20)` for concurrent reflections; adaptive early exit |
| A.9 | Summarization locks session | 100 sessions × 50 ms = 5 s of contention | Snapshot under brief read-lock; summarize outside lock |
| A.10 | Session memory leak (stale) | OOM after 24h of IDE reconnections | `DashMap::retain` every 60s; hard cap `max_sessions = 500` |
| A.11 | 100 SSE channels + stacks | ~83 MB baseline | Acceptable; `mpsc::channel(32)` not 64 |
| A.12 | reqwest pool exhaustion | Default 10 idle conns for 100 sessions | `pool_max_idle = max_concurrent × 2`; Ollama `Semaphore` |
| A.13 | Slow LLM holds all state open | 250s per response | SSE keep-alive (15s); `Semaphore(max_concurrent)` |
| A.14 | Cancel lag: stream-pump detects disconnect only on next chunk | 30s resource hold after client drops | `select! { cancel.cancelled() / stream.next() }` in pump loop |
| A.15 | Full `mpsc` channel from dead client | Permanent resource hold | `SO_KEEPALIVE` (30/10/3); 60s `send` timeout |

---

## Async Runtime

### Runtime config
```rust
Builder::new_multi_thread()
    .worker_threads(num_cpus::get_physical()) // no HT
    .max_blocking_threads(64)
    .thread_stack_size(512 * 1024)
    .enable_all()
```

### Task topology (per request)
```
Handler task  ──mpsc::channel(32)──▶  Axum SSE
     │
Orchestrator task (= stream pump)
     │  drives reqwest bytes_stream() inline
     └─ FuturesUnordered [tool_0..tool_N]  (tokio::spawn each)
```

### CancellationToken tree
```
gateway_cancel
└── request_cancel
    └── orchestrator_cancel
        ├── backend_call_cancel
        └── tool_cancel[N]
```

Drop-based `StreamGuard`: cancel fires + Ollama semaphore permit released atomically on drop.

### Blocking operation audit

| Operation | Resolution |
|---|---|
| BPE table load (`get_bpe_from_model`) | `OnceLock` at startup — never per-request |
| Token counting | `spawn_blocking` |
| `minijinja::render` | `spawn_blocking` |
| `serde_json` > 64 KB | `spawn_blocking` |
| Shell tool | `tokio::process::Command` (never `spawn_blocking + std::Command`) |
| Regex on response > 100 KB | `spawn_blocking` |
| `DashMap::retain` | `spawn_blocking` in background task |

### Timeout hierarchy
```
600s  full request      (Axum middleware)
 120s  reflection pass   (ReflectionOrchestrator)
   90s  backend call      (OllamaClient::stream)
    30s  per-chunk         (stream.next loop)
   10s  per-tool          (ToolExecutor)
    5s  quality check     (QualityChecker)
```

### Graceful shutdown order
1. Drop `TcpListener` — stop accepting
2. `gateway_cancel.cancel()` — cancel background tasks + all request children
3. `tracker.wait()` with 30s deadline — drain in-flight
4. `spawn_blocking(|| shutdown_tracer_provider())` — flush OTEL

---

## Memory Strategy

### Type selection

| Data | Type |
|---|---|
| reqwest body | `bytes::Bytes` (zero-copy) |
| NDJSON accumulation | `BytesMut` (`split_to` = zero-copy) |
| Tool args (pre-validated) | `Box<RawValue>` |
| Session message content | `Arc<str>` |
| Transformer strings | `Cow<'_, str>` |
| Model/tool name keys | `Arc<str>` via `StringInterner` |
| SSE data field | `String::with_capacity(80)` |
| Ollama request body | `Vec<u8>` pre-alloc 512 KB |

### MessageArena layout
```rust
pub struct MessageArena {
    content: String,          // all text contiguous — 1 heap alloc for all messages
    headers: Vec<MessageHeader>,
}

#[repr(C, align(8))]
struct MessageHeader {        // 20 bytes per message (vs ~200 for Vec<Message>)
    role:          Role,      // u8
    content_start: u32,
    content_len:   u32,
    token_count:   u32,       // cached; filled by background spawn_blocking
    tool_call_count: u8,
    _pad: [u8; 3],
}
```
**3× memory reduction** vs `Vec<Message>`. Snapshot = `Arc::clone` = 8 bytes, not a data clone.

### Hot path (1 alloc per token)

| Step | Alloc? |
|---|---|
| `reqwest Bytes` | 0 — zero-copy |
| `BytesMut::extend_from_slice` | 0 — existing capacity |
| `split_to().freeze()` | 0 — zero-copy split |
| `SmolStr` ≤ 22 bytes | 0 — inline storage |
| `serde_json::to_writer` → thread-local `Vec<u8>` | 0 — reused capacity |
| `String::from_utf8_unchecked` | **1** — required by axum `Event::data` |
| `mpsc::send` | 0 — moves into channel |

### Session memory limits

| Resource | Hard limit | Enforcement |
|---|---|---|
| `MessageArena::content` | 8 MB | Every `push()` |
| Message count | 4,096 | Every `push()` — triggers compression |
| Total session | 50 MB | Background eviction (60s) |
| Active sessions | 500 | New session creation check |

### 100-session memory budget

| Resource | Per session | × 100 |
|---|---|---|
| Task stacks (handler + orchestrator) | 384 KB | 38.4 MB |
| TCP socket buffers | 320 KB | 32 MB |
| `mpsc::channel(32)` | 4 KB | 400 KB |
| `NdjsonAccumulator` | 8 KB | 800 KB |
| `MessageArena` (50 messages) | ~50 KB | 5 MB |
| **Baseline total** | | **~77 MB** |
| At 128K context | ~614 KB | **~139 MB** |

---

## Scalability Model

### Connection pool (per backend)
```
reqwest pool_max_idle = max_concurrent × 2
Semaphore(max_concurrent)  ← gates actual GPU slots, not pool
```

| Hardware | `max_concurrent` | `pool_max_idle` |
|---|---|---|
| M2 Pro 18 GB | 2 | 4 |
| M2 Ultra 96 GB | 4 | 8 |
| RTX 4090 24 GB | 3 | 6 |

### Horizontal scaling

| Component | Stateless? | Multi-instance strategy |
|---|---|---|
| HTTP handler | Yes | Round-robin |
| OpenAI `/v1/chat/completions` | Yes | Round-robin |
| ACP sessions | No | Session affinity via consistent hash on `X-Acp-Session-Id` |
| Rate limiter | No | Redis (`deadpool_redis`) |
| Circuit breaker | No | Per-instance (converges independently) |

`SessionStore` trait: `LocalSessionStore` (DashMap) for Phase 1; `RedisSessionStore` for Phase 7.

### Backpressure chain
```
Slow Zed client
→ TCP recv buffer full → hyper poll_write Pending
→ Sse stream not polled → ReceiverStream not polled
→ mpsc::channel(32) full → Sender::send parks orchestrator
→ reqwest not polled → reqwest buffer full
→ kernel TCP window shrinks → Ollama send pauses
```
Fully automatic via TCP flow control. No app intervention needed.

### Load shedding
```rust
match semaphore.try_acquire() {
    Ok(_)  => next.run(req).await,
    Err(_) => (503, [("Retry-After","10")], json_error("capacity_exceeded")).into_response(),
}
```
Global: `Semaphore(150)`. Degraded mode: skip reflection when < 10% permits for > 30s.

### SLOs

| Metric | P50 | P95 | P99 |
|---|---|---|---|
| TTFB (first streaming token) | 800ms | 2,000ms | 4,000ms |
| TTFB (non-streaming) | 2ms | 10ms | 25ms |
| Gateway overhead | 5ms | 20ms | 50ms |
| Chunk → SSE write | 100µs | 500µs | 2ms |
| Session lookup | 1µs | 5µs | 20µs |
| Graceful shutdown drain | 5s | 20s | 30s |

### Implementation phase order

| Phase | Focus | Goal |
|---|---|---|
| 1 | Correct baseline | End-to-end correctness |
| 2 | Cancel + timeouts | No resource leaks |
| 3 | `ArcSwap` + async `RwLock` | Eliminate lock-induced spikes |
| 4 | Semaphores + admission control | Overload protection |
| 5 | `MessageArena` + `SmolStr` + buffer reuse | 1-alloc hot path |
| 6 | HTTP/2 + pool tuning + `SO_SNDBUF` | 100-session SSE throughput |
| 7 | `RedisSessionStore` + consistent hash | Horizontal scale |
