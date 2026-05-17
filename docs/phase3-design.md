# Phase 3 — Tool Calls: Design & Sub-Plan

## Overview

Phase 3 adds agentic tool-call capability to the gateway: the model can request tool execution, the gateway dispatches the tools, injects results back into the conversation, and re-submits — forming a loop until the model produces a final text response.

**Duration:** Weeks 6–7  
**Depends on:** Phase 2 complete ✅

---

## Total Design

### Data flow

```
Client (Zed / curl)
  │
  │  POST /v1/chat/completions  { tools: [...], messages: [...] }
  ▼
api::openai::chat_completions
  │
  │  ConversationRequest { tool_definitions, messages }
  ▼
application::tool_loop::ToolLoopOrchestrator
  │
  ├─► ChatService::complete_raw()        ← single backend turn (non-streaming)
  │       │
  │       │  ConversationResponse { tool_calls: Some([...]) }
  │       ▼
  │   detect finish_reason == ToolCalls?
  │       │ YES
  │       ▼
  │   ToolExecutor::dispatch_all()       ← FuturesUnordered, concurrent
  │       │
  │       │  Vec<ToolResult>
  │       ▼
  │   inject Role::Tool messages into conversation
  │       │
  │       └─► loop (max 5 passes)
  │
  │   finish_reason == Stop → return final ConversationResponse
  │
  ▼
SSE stream or JSON response back to client
```

### Tool call format normalization

Ollama/Qwen/DeepSeek each emit tool calls differently. The normalizer runs **inside `OllamaClient`** before returning `ConversationResponse` to the application layer. The domain layer only ever sees normalized `ToolCall` structs.

| Model family | Raw format | Example |
|---|---|---|
| OpenAI-native / Ollama structured | JSON in `message.tool_calls[]` | `{"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"src/main.rs\"}"}}`  |
| Qwen2.5 (fallback) | XML in `message.content` | `<tool_call>{"name":"read_file","arguments":{"path":"src/main.rs"}}</tool_call>` |
| DeepSeek (fallback) | Markdown fence in `message.content` | ` ```json\n{"name":"read_file","arguments":{"path":"src/main.rs"}}\n``` ` |

The normalizer tries OpenAI format first (from `message.tool_calls`), then Qwen XML, then DeepSeek markdown.

### Built-in tools

| Tool | Input | Output | Security |
|---|---|---|---|
| `file_read` | `path: String` | File contents as string | Path must be within `workspace_root`; `../` rejected with `ToolError::Unauthorized` |
| `search` | `query: String, path: Option<String>` | Matching lines with file+line refs | Runs `ripgrep` via `tokio::process::Command`; timeout 10s |

### Architecture: where each piece lives

```
domain/
  entities/
    tool.rs               ← ToolDefinition, ToolCall, ToolResult, ToolError (NEW)
  ports/
    tool_runtime.rs       ← ToolRuntime trait (NEW)

infrastructure/
  tools/
    mod.rs                ← re-exports
    normalizer.rs         ← ToolCallNormalizer: raw Ollama → ToolCall (NEW)
    registry.rs           ← ToolRegistry: name → Arc<dyn Tool> (NEW)
    executor.rs           ← ToolExecutor: dispatch + timeout (NEW)
    file_read.rs          ← FileReadTool (NEW)
    search.rs             ← SearchTool via ripgrep (NEW)

application/
  tool_loop.rs            ← ToolLoopOrchestrator: detect→dispatch→inject→resubmit (NEW)
  chat.rs                 ← add stream_with_tools() and complete_with_tools() (MODIFIED)

api/
  openai/
    types.rs              ← extend ChatCompletionRequest + ChunkDelta for tool_calls (MODIFIED)
    chat.rs               ← route to tool_loop when tools present (MODIFIED)

infrastructure/
  ollama/
    types.rs              ← add OllamaToolCall, OllamaFunction fields (MODIFIED)
    client.rs             ← call normalizer after complete(), pass tool_definitions (MODIFIED)
```

---

## Sub-Plans

### Phase 3.1 — Domain entities + ports
**Goal:** Define the normalized tool types and `ToolRuntime` trait. No I/O. All other sub-phases depend on this.

Files created:
- `crates/domain/src/entities/tool.rs`
- `crates/domain/src/ports/tool_runtime.rs`

Files modified:
- `crates/domain/src/entities/mod.rs` — add `pub mod tool`
- `crates/domain/src/ports/mod.rs` — add `pub mod tool_runtime`
- `crates/domain/src/entities/message.rs` — add `tool_call_id` field to `Message` for `Role::Tool` messages
- `crates/domain/src/entities/conversation.rs` — add `tool_definitions: Vec<ToolDefinition>` to `ConversationRequest`, `tool_calls: Option<Vec<ToolCall>>` to `ConversationResponse`

Tests: serde round-trip for all new types.

---

### Phase 3.2 — Infrastructure: Ollama types + normalizer
**Goal:** Extend Ollama wire types to carry tool_calls; implement `ToolCallNormalizer` that converts raw Ollama/Qwen/DeepSeek output into normalized `ToolCall` structs.

Files created:
- `crates/infrastructure/src/tools/mod.rs`
- `crates/infrastructure/src/tools/normalizer.rs`

Files modified:
- `crates/infrastructure/src/ollama/types.rs` — add `OllamaToolCall`, `OllamaFunction`, `tool_calls` field to `OllamaChatChunk`/`OllamaGenerateRequest`
- `crates/infrastructure/src/ollama/client.rs` — pass `tool_definitions` into request body; call normalizer on response
- `crates/infrastructure/src/lib.rs` — add `pub mod tools`

Tests: unit tests for each normalizer format (OpenAI JSON, Qwen XML, DeepSeek markdown, malformed input).

---

### Phase 3.3 — Infrastructure: ToolRegistry + built-in tools
**Goal:** `FileReadTool` and `SearchTool` implementations behind a `ToolRegistry`. Security enforcement for path traversal.

Files created:
- `crates/infrastructure/src/tools/registry.rs`
- `crates/infrastructure/src/tools/executor.rs`
- `crates/infrastructure/src/tools/file_read.rs`
- `crates/infrastructure/src/tools/search.rs`

Tests:
- `FileReadTool` reads a file within workspace root ✅
- `FileReadTool` returns `ToolError::Unauthorized` on `../` ✅
- `FileReadTool` returns `ToolError::Unauthorized` on absolute path outside root ✅
- `SearchTool` returns matches (uses real ripgrep or a mock command) ✅
- `ToolExecutor` 10s timeout fires correctly ✅

---

### Phase 3.4 — Application: ToolLoopOrchestrator
**Goal:** The re-submission loop. Detects tool calls in the response, dispatches via `FuturesUnordered`, injects `Role::Tool` messages, resubmits. Max 5 passes.

Files created:
- `crates/application/src/tool_loop.rs`

Files modified:
- `crates/application/src/chat.rs` — add `complete_with_tools()` that delegates to `ToolLoopOrchestrator`
- `crates/application/src/lib.rs` — add `pub mod tool_loop`

Tests:
- 1-tool-call loop: single call dispatched, result injected, final response returned ✅
- 3-tool-call loop (sequential passes): 3 distinct calls across passes ✅
- Max passes exceeded: returns last response as-is after 5 passes ✅
- Tool error: `ToolError::Unauthorized` injected as error tool result, loop continues ✅

---

### Phase 3.5 — API: Wire types + routing
**Goal:** Extend OpenAI-compat types to carry `tools` and `tool_calls`; route requests that include `tools` through `ToolLoopOrchestrator`; update `AppState` DI.

Files modified:
- `crates/api/src/openai/types.rs` — add `tools`, `tool_choice` to request; `tool_calls` to `ChatMessage`; `tool_calls` delta to `ChunkDelta`
- `crates/api/src/openai/chat.rs` — branch: if `req.tools.is_some()` → `complete_with_tools()`, else existing path
- `crates/api/src/state.rs` — wire `ToolRegistry`, `ToolExecutor`, `ToolLoopOrchestrator` into `AppState`
- `crates/api/Cargo.toml` — add `application` tool_loop re-export (already dep)

Tests: curl-testable — send a request with `tools` field, verify the gateway calls the tool and returns the final answer.

---

## Key Design Decisions

### Why tool loop lives in `application`, not `infrastructure`
The loop logic (detect → dispatch → inject → resubmit) is pure orchestration — no I/O of its own. Infrastructure only implements the tools and normalizer. This keeps the dependency rule clean.

### Why `FuturesUnordered` for dispatch
Multiple tool calls in a single turn can be executed concurrently. `FuturesUnordered` consumes results as they arrive and doesn't wait for stragglers unnecessarily.

### Why max 5 passes
Prevents infinite loops if the model keeps generating tool calls. After 5 passes, the last response (even if it contains tool_calls) is returned as the final answer.

### Streaming with tool calls
For Phase 3, **tool calls use the non-streaming (`complete_with_tools`) path internally** even when the client requests `stream: true`. The SSE response is sent only after the full tool loop completes. This is the same approach used by OpenAI's own API for tool-heavy interactions. True streaming of tool call deltas is deferred to Phase 4+.

### `Role::Tool` message format
```
Message {
    role: Role::Tool,
    content: vec![ContentPart::Text(result_json)],
    tool_call_id: Some("call_abc123"),   // matches the original ToolCall.id
}
```
The `tool_call_id` field links each result back to the call that produced it — required by the OpenAI spec and by Ollama's structured tool call format.

### Security: `FileReadTool` workspace root
Configured at startup via `gateway.toml`:
```toml
[tools]
workspace_root = "/home/user/myproject"   # FileReadTool is confined to this tree
```
Path validation: canonicalize the requested path, then check it starts with `canonicalize(workspace_root)`. Both `../escape` and `/etc/passwd` are rejected.

---

## gateway.toml additions (Phase 3.5)

```toml
[tools]
workspace_root = "."      # FileReadTool root; "." = gateway's working directory
ripgrep_bin    = "rg"     # SearchTool binary; must be on PATH
```

---

## Exit criteria (all sub-phases)

- [x] 3.1: `cargo test -p domain` — all tool entity serde round-trips pass ✅
- [x] 3.2: `cargo test -p infrastructure` — normalizer handles all 3 formats + malformed ✅
- [x] 3.3: `cargo test -p infrastructure` — FileReadTool path traversal rejected; SearchTool returns results ✅
- [ ] 3.4: `cargo test -p application` — 3-pass tool loop test passes
- [ ] 3.5: `cargo test --workspace` — full suite green; curl with `tools` field works end-to-end
