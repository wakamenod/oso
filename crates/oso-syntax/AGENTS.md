# oso-syntax

Syntax highlighting with tree-sitter: the grammars, oso's own queries, the parsers and the
highlighting loop (ADR 0012, ADR 0111). See `docs/dev/display-model.md`.

- May depend on `oso-core` only. It is the only crate that may use tree-sitter and its
  grammars (`deny.toml`, `crates/oso/tests/architecture`); only the tip crate depends on it.
- `#![forbid(unsafe_code)]`: tree-sitter's Rust API is safe, and the C stays in the
  dependencies. No `unwrap` or `expect` outside tests.
- Nothing here runs on the UI thread. The tip crate's worker calls it, and the UI reads only
  what it reports, through `oso-core` types. tree-sitter types stay out of the public API.
- Every parse and every query is bounded: a time slice, the time limits and the size limit
  of ADR 0111. A callback given to tree-sitter must never panic, as it runs under C.
- Walk a tree with a `TreeCursor`, never by recursion, so a deep tree cannot overflow a stack.
- No buffer text in a log line or an error.
- Grammar versions are exact. Before a bump, read its `build.rs` and its external scanner's
  diff (T2 in `docs/dev/security.md`).
- Module layers are in `guardrails/layers.txt` (`[oso-syntax]`).
- Every Rust file opens with its responsibility and a `Must not depend on:` paragraph (ADR
  0041). Anything `pub` is in the API snapshot (`just public-api`).
