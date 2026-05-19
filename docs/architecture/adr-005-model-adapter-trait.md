# ADR-005 — ModelAdapter Trait: Per-Model Plugin for Prompt and Quirks

**Status:** Accepted  
**Date:** 2026-05-19  
**Relates to:** [ADR-003](adr-003-tool-strategy-native-plus-mcp.md), [ADR-004](adr-004-tool-naming-convention.md)

---

## Context

Before this ADR, model-specific behaviour was scattered across two files with hardcoded `if model.contains("qwen")` / `if model.contains("deepseek")` branches:

| File | Hardcoded behaviour |
|---|---|
| `application/prompt/quirks.rs` — `ModelQuirksTransformer` | Strip `<\|im_start\|>` for Qwen; strip `<think>` for DeepSeek |
| `application/prompt/system_prompt.rs` — `SystemPromptBuilder` | Same system prompt for every model regardless of its preferred instruction style |
| `api/src/state.rs` | Static tool-name list and example JSON baked into a format string |

Adding support for a new LLM (e.g. Llama 3, Mistral, Phi-3) required editing all three files, with no compile-time guarantee that the new model's behaviour was complete.

---

## Decision

Introduce a `ModelAdapter` trait in `domain/ports/model_adapter.rs`. It is the single extension point for all per-model behaviour:

```rust
pub trait ModelAdapter: Send + Sync {
    /// True if this adapter handles the given model name string.
    fn matches(&self, model: &str) -> bool;

    /// Strip model-specific injection tokens from message content.
    /// Default: identity (no cleaning).
    fn clean_content(&self, text: String) -> String { text }

    /// Tool-call instructions injected into the system prompt for this model.
    /// Receives the workspace root and list of available tool names.
    fn tool_instructions(&self, workspace_root: &str, tools: &[&str]) -> String;

    /// Minimum max_tokens to use on tool-loop requests for this model.
    /// Default: 4096 (safe floor for all known models).
    fn tool_max_tokens(&self) -> u32 { 4096 }
}
```

A `ModelAdapterRegistry` (in `application/prompt/adapter_registry.rs`) holds a `Vec<Arc<dyn ModelAdapter>>` and dispatches by first match. `ModelQuirksTransformer` and `SystemPromptBuilder` both delegate to the registry.

Built-in adapters live in `infrastructure/adapters/`:

| Adapter | Matches | Behaviour |
|---|---|---|
| `QwenAdapter` | `"qwen"` / `"Qwen"` | Strips `<\|im_start\|>` / `<\|im_end\|>`; Qwen-tuned tool instructions |
| `DeepSeekAdapter` | `"deepseek"` / `"DeepSeek"` | Strips `<think>…</think>`; DeepSeek-tuned tool instructions |
| `DefaultAdapter` | everything else | No-op cleaning; generic tool instructions |

`DefaultAdapter` is always registered last so unrecognised models still work.

---

## How to add a new LLM

1. Create `crates/infrastructure/src/adapters/<model>.rs`
2. Implement `ModelAdapter` for your struct
3. Register it in `AppState::new()` in `api/src/state.rs` before `DefaultAdapter`

No changes to domain, application, or any existing adapter are required.

---

## Rationale

| Option | Problem |
|---|---|
| Keep `if model.contains(...)` branches | Every new model edits 2–3 files; no compile-time completeness check; grows unboundedly |
| Trait per concern (separate QuirksTrait, PromptTrait) | Two traits to implement per model; harder to register atomically |
| Single `ModelAdapter` trait (chosen) | One file per model; one registration call; default impls cover 90% of models |

The trait's `clean_content` and `tool_instructions` methods cover the two concrete needs today. `tool_max_tokens` is included because some models (e.g. small quantised variants) need a different floor than 4096.

---

## Consequences

- `ModelQuirksTransformer` becomes a thin delegator: it calls `registry.adapter_for(model).clean_content(text)` instead of branching on model name.
- `SystemPromptBuilder` gains an `Arc<ModelAdapterRegistry>` field; `apply()` calls `adapter.tool_instructions(workspace_root, tools)` and appends the result.
- `api/state.rs` no longer contains the hardcoded tool-name string; it builds the tool list dynamically from `ToolLoopOrchestrator::tool_names()` and passes it to `SystemPromptBuilder`.
- All existing tests for `ModelQuirksTransformer` remain valid — they test the same observable behaviour, now routed through the adapter.
- The `DefaultAdapter` is a correct fallback: its `tool_instructions` produces the same generic JSON example that `state.rs` previously hardcoded.
