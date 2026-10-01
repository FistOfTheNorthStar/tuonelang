# tuo-mcp

The tuonelang agent protocol (`tuo agent --stdio`, see `crates/tuo-agent`)
exposed as [Model Context Protocol](https://modelcontextprotocol.io) tools, so
a coding agent such as Claude Code sees the compiler's queries in its tool list
instead of having to drive a JSON-lines stream by hand.

It is a bridge and nothing more: every tool is one protocol method, the
compiler computes every answer, and one `tuo agent` process lives for the
bridge's whole life so the incremental database is reused across calls. It has
no dependencies — both transports are newline-delimited JSON — and nothing in
the workspace depends on it.

## Setup

The repository's `.mcp.json` registers the server for Claude Code; approve it
when prompted on the next session. It needs a built `tuo`:

```bash
cargo build -p tuo-cli            # produces target/debug/tuo, which the bridge finds
```

Set `TUO_BIN` to use a different binary.

## Tools

| Tool | Protocol method | Use |
|---|---|---|
| `tuo_open`, `tuo_open_file` | `set_document` | Put a document (inline text, or a file from disk) into the session. Call again after every edit. |
| `tuo_check` | `check` | The whole front end over every open document; diagnostics with ranges. |
| `tuo_verify` | `verify` | Static checks plus spec execution; optionally only the specs an edit affects. |
| `tuo_format` | `format` | The canonical formatter. |
| `tuo_diagnostics`, `tuo_symbols` | same names | Per-document diagnostics and module-level symbols. |
| `tuo_type_at`, `tuo_definition`, `tuo_references`, `tuo_signature`, `tuo_members`, `tuo_specs_for` | same names | Positional queries; positions are one-based `line`/`column`. |
| `tuo_available_imports`, `tuo_imports_for_symbol` | same names | Which module exports a name, instead of guessing. |
| `tuo_run_spec` | `run_spec` | Execute specs on the reference interpreter. |
| `tuo_apply_safe_fix` | `apply_safe_fix` | Only compiler-authored, machine-applicable fixes. |
| `tuo_context_at`, `tuo_expected_type_at`, `tuo_visible_symbols_at`, `tuo_valid_members_of`, `tuo_call_signature`, `tuo_expected_syntax_at` | same names | The generation queries: what to write next at a position, with their honesty flags (`complete`, `exhaustive`) passed through untouched. |
| `tuo_cheatsheet` | `tuo cheatsheet` (CLI) | The generated language brief (ADR-0018). |

The agent protocol's own guarantees carry over: responses are deterministic
where the compiler is, an unknown document or malformed position comes back as
a structured error (surfaced as an `isError` tool result), and no LLM is
embedded anywhere.

## Test

```bash
node tools/tuo-mcp/test.mjs
```

drives the bridge over its real stdio transport: initialize, list tools, open a
program, check it, break it and see `R0002`, ask a generation query, provoke
an error, fetch the brief.
