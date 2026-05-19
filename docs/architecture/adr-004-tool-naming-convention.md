# ADR-004 — Tool Naming Convention: `verb_noun`

**Status:** Accepted  
**Date:** 2026-05-19

## Context

The gateway's built-in tools were originally named with inconsistent conventions:

| Old name   | Pattern    |
|------------|------------|
| `file_read`  | noun_verb  |
| `search`     | bare verb  |
| `find`       | bare verb  |
| `list_dir`   | verb_abbrev|

When adding `write_file` and `patch_file`, naming inconsistency became a problem: models trained on OpenAI-style function calling, MCP server conventions, and our own system prompt examples all need a single predictable pattern to follow.

## Decision

All built-in tool names use **`verb_noun` snake_case**:

| Tool             | Action  | Object      |
|------------------|---------|-------------|
| `read_file`      | read    | file        |
| `write_file`     | write   | file        |
| `patch_file`     | patch   | file        |
| `search_files`   | search  | files       |
| `find_files`     | find    | files       |
| `list_directory` | list    | directory   |

MCP tools added via `[[tools.mcp]]` in `gateway.toml` keep their server-assigned names and are not renamed — only gateway-native tools follow this convention.

## Rationale

- **Model discoverability:** `verb_noun` reads naturally as an instruction (`read_file`, `write_file`). Models generate fewer wrong-format calls when the name itself implies the action.
- **Consistency with MCP ecosystem:** Most MCP servers (git, fetch, filesystem) already use `verb_noun` or `verb_object` naming. Aligning reduces cognitive mismatch for models switching between native and MCP tools.
- **Alphabetic grouping:** All file-operating tools sort together under `*_file` and `*_files` in the model's tool list — the model sees related tools adjacent.
- **No ambiguity with bare verbs:** `search` could mean search the web, search the DB, search the workspace. `search_files` is unambiguous.

## Consequences

- All string literals naming tools (tool loop, normalizer tests, system prompt, integration tests) must use the new names. A grep for the old names (`file_read`, `list_dir`, `find"`, `"search"`) should return zero hits in production code after this change.
- The system prompt rule `NEVER output {"function_name":...}` remains unchanged — the P5 normalizer format is still supported for models that emit it.
- Future native tools MUST follow `verb_noun`. Any deviation requires a new ADR.
