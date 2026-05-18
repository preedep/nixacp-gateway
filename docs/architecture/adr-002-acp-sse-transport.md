# ADR-002 — ACP Session Updates via Server-Sent Events

**Date:** 2026-05-18  
**Status:** Accepted  
**Deciders:** @preedep

---

## Context

Phase 5 delivered `POST /acp` which handles `session/prompt` but returns only `{"stopReason":"end_turn"}` — the LLM answer is invisible to the caller. The ACP spec sends streaming content back to the client as **`session/update` notifications** over a separate channel, not in the JSON-RPC response body.

Two options were evaluated for the notification transport:

1. **Response-body streaming** — return a streaming HTTP response from `session/prompt`, embedding notification lines in the body before the final JSON-RPC result.
2. **Server-Sent Events (SSE) on a separate endpoint** — client opens `GET /acp/events?session_id=<id>` before prompting; agent pushes `session/update` notifications there.

---

## Decision

Use **SSE on `GET /acp/events`** (option 2).

Each `session/update` notification is one SSE `data:` line containing a JSON-RPC 2.0 notification object:

```
data: {"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"...","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hello"}}}}

```

The `session/prompt` JSON-RPC response (`stopReason`) is still returned synchronously in the HTTP response body.

---

## Rationale

| Factor | Response-body streaming | SSE endpoint |
|---|---|---|
| ACP spec alignment | Non-standard | Matches spec intent for remote agents |
| Zed compatibility | Unknown | Spec-compliant; Zed implements this path |
| Multiple subscribers | Not possible | `broadcast::Sender` allows fan-out |
| Retrofit cost | Low | Low — adds one route, one channel per session |
| Backpressure | Implicit (HTTP) | `broadcast::Sender` with capacity 64 |

---

## Implementation

- `AcpSessionStore` gains a `broadcast::Sender<String>` per session (capacity 64).
- `GET /acp/events?session_id=<id>` subscribes via `broadcast::Receiver`, streams JSON-encoded `session/update` notification lines as SSE events.
- `session/prompt` handler: opens a streaming Ollama request, pushes each delta as a `session/update` / `AgentMessageChunk` to the broadcast channel, then returns `PromptResponse` once the stream ends.
- SSE event format: `data: <json>\n\n` (standard SSE). No `id:` field needed; the session ID is inside the JSON.

## Consequences

- Clients **must** open `GET /acp/events?session_id=<id>` before sending `session/prompt` to receive streaming content.
- If no SSE subscriber is connected, notifications are silently dropped (broadcast with no receivers). The `session/prompt` still returns correctly.
- Phase 7 can layer a persistent SSE connection manager and replay missed events.
