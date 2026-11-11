# 0012. Native tree-sitter for syntax

- Status: Accepted, amended by 0111
- Date: 2026-11-03

## Context

The original design planned syntect, which uses regex-based TextMate grammars.
Syntect is slow on large files. Tree-sitter parses incrementally and is fast,
but its grammars are C code that parses untrusted file content.

## Decision

- Use **native tree-sitter**, as Zed, Helix and Neovim do.
- Ship only the grammars actually used, pinned to exact versions. Start with
  Rust, Markdown and TOML.
- Parse on a background thread with cancellation and timeouts. Results
  replace the syntax decoration layer atomically.
- Fuzz the highlighting path.

## Consequences

- Fast incremental highlighting that never blocks typing.
- C grammar code runs in-process. This is an accepted risk, mitigated by
  pinning, a small grammar set and fuzzing.

## Alternatives considered

- **syntect:** pure Rust, but slower.
- **Tree-sitter grammars as Wasm (wasmtime):** sandboxed, but wasmtime is a
  large dependency and would increase supply-chain risk.
