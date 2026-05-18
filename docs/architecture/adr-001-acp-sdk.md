# ADR-001 — Use `agent-client-protocol` SDK for ACP Implementation

**Date:** 2026-05-18  
**Status:** Accepted  
**Deciders:** @preedep

---

## Context

Phase 5 requires implementing the Agent Client Protocol (ACP) so Zed IDE can connect to the gateway via its agent panel. The CLAUDE.md spec says:

> Before implementing ACP wire types in Phase 5: check for the `acp-sdk` Rust crate on crates.io first. Use it if available; only hand-implement from the OpenAPI spec if no SDK exists.

Two options were evaluated:

1. **Hand-implement** wire types from the ACP OpenAPI spec at agentclientprotocol.com
2. **Use the official Rust SDK** (`agent-client-protocol` + `agent-client-protocol-schema`)

---

## Decision

Use the official SDK crates:

```toml
agent-client-protocol        = "0.12.1"
agent-client-protocol-schema = "0.13.2"
```

---

## Rationale

| Factor | Hand-implement | Use SDK |
|---|---|---|
| Wire types | ~400 LOC to write + maintain | Zero — schema crate provides all types |
| Protocol correctness | Risk of drift from spec | SDK tracks spec by design |
| MSRV compatibility | N/A | Verified: compiles clean on Rust 1.86 |
| Dependency surface | None | Adds ~8 transitive deps (all already used: tokio, serde, uuid, futures) |
| ACP spec stability | Spec is actively evolving | SDK absorbs breaking changes behind a versioned API |
| Zed compatibility | Must track Zed's version manually | SDK is from the same org that maintains the spec |

The SDK was published 2026-05-17 (one day before this decision), is at 0.12.1, and compiles cleanly with our Rust 1.86 toolchain and existing dependency graph.

---

## Consequences

- **All ACP wire types** come from `agent-client-protocol-schema`. Never define duplicate structs.
- **`infrastructure/acp`** holds the `AcpAdapter` (SDK `Agent` trait impl) and `AcpSessionStore`.
- **`api/acp`** holds the HTTP handler that bridges Axum ↔ SDK transport.
- If the SDK introduces a breaking change, the blast radius is confined to `infrastructure/acp` and `api/acp` — domain and application layers are unaffected.
- The SDK uses `rmcp` for MCP server support; we expose that capability flag as `false` in Phase 5 (Phase 7 wires it up).
