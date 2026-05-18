# ADR-003 — Tool Strategy: Native Built-ins + MCP for Extended Tools

**Date:** 2026-05-18  
**Status:** Accepted  
**Deciders:** @preedep

---

## Context

The gateway's tool system (Phase 3) currently provides two built-in tools: `file_read` and `search`
(ripgrep). For the gateway to support agentic coding workflows comparable to Claude Code, it needs
a richer set: shell execution, file write/edit, directory listing, git operations, web fetch, and
user-defined tools.

Two pure approaches were considered:

1. **Native only** — hand-build every tool in `infrastructure/tools/`
2. **MCP only** — delegate all tool execution to external MCP servers

A third approach emerged from review: a **mix**, where coding essentials are native and everything
else is delegated to MCP.

---

## Decision

Use a **two-registry architecture**:

```
ToolLoopOrchestrator
  ├── NativeToolRegistry   ← built-in, always available, sandboxed
  └── McpToolRegistry      ← pluggable, optional, Phase 7
```

Both registries implement the existing `ToolRuntime` port from `domain`. The orchestrator resolves
a tool name by checking native first, then MCP.

### Native tools (built-in, always-on)

| Tool | Rationale |
|---|---|
| `file_read` | Already done; path-traversal protection via `canonicalize` |
| `file_write` | Core coding workflow; needs workspace sandboxing |
| `file_edit` | Diff/patch apply; needs workspace sandboxing |
| `search` | Already done; ripgrep via `tokio::process::Command` |
| `find` / `ls` | Trivial, no external dependency, used on every request |
| `bash` (restricted) | Coding agents need shell; **allowlist-only** — deny pipes, redirects, shell metacharacters |

Native tools have no MCP round-trip latency, no external process to manage, and the security
policy is controlled entirely by the gateway.

### MCP tools (pluggable, optional)

| Tool category | Example MCP server |
|---|---|
| Git operations | `@modelcontextprotocol/server-git` |
| Web fetch / browser | `@modelcontextprotocol/server-fetch` |
| Database queries | `@modelcontextprotocol/server-postgres` |
| Custom / user-defined | Any MCP server the user brings |

MCP is the right boundary for tools maintained by third parties and for user-extensible workflows.
The gateway acts as an MCP **client** (not server) for these.

---

## Rationale

| Factor | Native only | MCP only | Mix (chosen) |
|---|---|---|---|
| Latency | Best — no IPC | MCP round-trip per call | Native: zero IPC; MCP: only when needed |
| Security control | Full | Depends on MCP server | Full for critical tools; delegated for optional |
| Maintenance | High — own every tool | Low — upstream maintains servers | Low — only maintain the small core |
| Extensibility | Low — code change required | High — add server in config | High — MCP handles extensibility |
| Dependency risk | None | MCP server availability | Minimal — native tools have no deps |
| Coding workflow coverage | Must build everything | Already exists in MCP ecosystem | Core built-in; extras for free via MCP |

---

## `bash` tool security policy

`bash` is the highest-risk native tool. The policy:

- **Allowlist** of safe programs configurable in `gateway.toml` under `[tools.bash]`
- Default allowlist: `cargo`, `rustc`, `npm`, `node`, `python`, `python3`, `pip`, `git`
- **Deny** if the command string contains: `;`, `|`, `&&`, `||`, `>`, `<`, `` ` ``, `$(...)`
- Working directory is always the workspace root; `cd` outside workspace is rejected
- Timeout: 30 seconds (same as `Per-chunk` level in the timeout hierarchy)

---

## Architectural impact

### Phase 7 additions (planned)

```
crates/infrastructure/src/tools/
  ├── file_write.rs      ← new native tool
  ├── file_edit.rs       ← new native tool
  ├── find.rs            ← new native tool
  ├── bash.rs            ← new native tool (allowlist enforced)
  └── mcp/
      ├── client.rs      ← MCP stdio/HTTP client
      ├── registry.rs    ← McpToolRegistry (implements ToolRuntime port)
      └── config.rs      ← parse [[tools.mcp]] entries from gateway.toml
```

### `gateway.toml` additions

```toml
[tools.bash]
enabled   = true
allowlist = ["cargo", "rustc", "npm", "node", "python", "python3", "git"]

[[tools.mcp]]
name    = "git"
command = "npx"
args    = ["-y", "@modelcontextprotocol/server-git", "/workspace"]

[[tools.mcp]]
name    = "fetch"
command = "npx"
args    = ["-y", "@modelcontextprotocol/server-fetch"]
```

### Resolution order

1. Look up tool name in `NativeToolRegistry`
2. If not found, look up in `McpToolRegistry`
3. If not found in either, return `ToolError::NotFound` (existing behavior)

---

## Consequences

- `ToolLoopOrchestrator` gains a second registry field; resolution order is deterministic (native
  wins on name collision).
- Native tools share the existing 10-second `ToolExecutor` timeout; `bash` uses 30 seconds
  (configurable).
- MCP servers are spawned as child processes on first use and kept alive for the gateway lifetime;
  a failed MCP server is retried once before the tool returns `ToolError::ExecutionFailed`.
- Adding a new native tool requires a code change; adding a new MCP tool requires only a
  `gateway.toml` entry — no code change.
- The `[[tools.mcp]]` config section is introduced in Phase 7. Until then, `McpToolRegistry`
  is registered but empty; native tools work as today.
- Related: [[adr-001-acp-sdk]] — the ACP SDK (`rmcp`) exposes an MCP server capability flag;
  Phase 7 wires the `McpToolRegistry` through that same `rmcp` infrastructure.

---

## Implementation notes (Phase 7a partial delivery)

The following was delivered ahead of the full Phase 7a schedule:

**`ToolRuntime::definition()`** — added to the `ToolRuntime` trait in `domain/ports/tool_runtime.rs`.
Each tool now returns its own `ToolDefinition` with a complete JSON Schema. The gateway injects
all registered tool definitions into every request regardless of what Zed sends — Zed's own editor
tools (`edit_file`, `create_file`, etc.) are silently dropped.

**Tool loop SSE streaming** — `tool_loop_response()` in `api/openai/chat.rs` and
`responses_tool_loop()` in `api/openai/responses.rs` now emit the final answer as SSE
(`text/event-stream`) instead of a JSON blob. This is required for Zed's chat UI to render
the result.

**`workspace_root` config** — `gateway.toml` now has a top-level `workspace_root` key.
All tools are confined to this path. The system prompt includes the root so the model uses
relative paths rather than absolute paths from Zed's file context.

**`domain_messages()` tool_calls serialization fix** — `OllamaClient::domain_messages()` now
serializes `Message.tool_calls` onto the outgoing `OllamaChatMessage`. Previously, assistant
messages carrying tool calls were sent with empty `content` and no `tool_calls` field; Ollama
could not correlate the subsequent tool-result messages and stalled until timeout (502 on pass 2).

**System prompt injection for Zed requests** — `SystemPromptBuilder::apply()` now appends the
gateway's tool-use rules to an existing system message rather than skipping injection when one is
present. Zed always sends its own system message, so the previous behavior left the model with no
tool instructions. The appended rules are visible in the `prompt_eval_count` token count (increased
from 3435 to 3618+ on first call).

**Pipeline applied before tool loop** — `chat_completions()` in `api/openai/chat.rs` now calls
`state.chat.apply_pipeline()` before handing the request to `ToolLoopOrchestrator`. The tool loop
calls `backend.complete()` directly, bypassing `ChatService.prepare()`, so the pipeline transform
must be applied at the call site.

**`GATEWAY_WORKSPACE_ROOT` env var fix** — `src/main.rs` previously used
`Env::prefixed("GATEWAY_").split("_")` for all env vars. The `split("_")` caused
`GATEWAY_WORKSPACE_ROOT` to be mapped to the nested key `workspace.root` instead of the flat
`workspace_root` field — so the env var was silently ignored and tools used the gateway binary's
working directory as the workspace root. Fixed by splitting only the known nested keys
(`log_level`, `log_format`, `server_host`, `server_port`) and mapping `GATEWAY_WORKSPACE_ROOT`
separately via `Env::map`.

**Normalizer Priority 5 — `function_name`/`function_arg` format** — Some Qwen2.5-coder variants
emit `{"function_name":"tool","function_arg":{...}}` instead of the standard
`{"name":"tool","arguments":{...}}`. Added as Priority 5 in `ToolCallNormalizer` so the tool
executes rather than returning the raw JSON as the model's response text. The system prompt
also now explicitly forbids this format by name.

**`max_tokens` floor on tool-loop requests** — `chat_completions()` sets `max_tokens` to
`max(requested, 4096)` before passing the request to `ToolLoopOrchestrator`. Ollama's default
`num_predict` is 128 when `max_tokens` is absent, which is enough for approximately 2 lines of
output — too short for any real file content.

**`file_read` fallback content append** — `ToolLoopOrchestrator` now tracks the last successful
`file_read` result. If the model's final response is shorter than the file content it received,
the raw file content is appended after the model's intro sentence. This is a reliable fallback
for the common case where a 14B model summarises file contents instead of reproducing them.
The fallback is `file_read`-specific and only fires on successful reads; it does not affect
`list_dir`, `search`, or `find` responses.
