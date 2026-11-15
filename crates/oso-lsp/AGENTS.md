# oso-lsp

The language server client (ADR 0138). Step 2 built the parts that spawn nothing: `framing`,
`json`, `uri`, `position` and `limits`. Step 3 added `protocol`, the messages of a server's
life as plain types, and `server`, its threads: the session thread (`server/session.rs`,
with `run.rs` and `incoming.rs`), the pipe threads (`pipes.rs`), the crash and flood rules
(`policy.rs`) and what is reported to the UI (`outbox.rs`).

- May depend on `oso-core` and `oso-platform` only; only the tip crate depends on it. Module
  layers are in `guardrails/layers.txt` (`[oso-lsp]`). No new dependency: JSON is
  `serde_json` through `Value`, with its nesting limit on.
- Everything a server sends is untrusted. Failures are values, never panics; no `unwrap` or
  `expect` outside tests, and no `unsafe` (`#![forbid(unsafe_code)]`).
- Nothing grows without a bound in `limits.rs`. Each bound of ADR 0138's table is there,
  and each constant's doc names the step that enforces it. The part that enforces one reads
  it from there, never a number of its own, and gets a test at the bound and past it, and a
  fuzz target (a proptest `fuzz` in the module's `tests.rs`) that arbitrary input never
  breaks it.
- A position a server sends is clamped, never trusted; a URI is read only if it names a
  local absolute path in normal form. The rules for Windows are plain functions of text, so
  macOS and Linux test them too.
- Nothing here writes a message's contents to a log line or the disk: errors say where and
  what kind, never what a message held.
- A server spawns only through `oso-platform::process`, in a process group of its own, on its
  session thread, after the caller's `Gate` said yes and named the program. The UI thread only
  sends events and takes the outbox; nothing here blocks it.
- The fake server is a binary of the tip crate's dev builds, `crates/oso/tests/fake_lsp_server/`,
  so it never ships (`just release-safety` checks that). Its replies are built by hand, framed
  by `framing::encode`, so a test of the client checks the client's reading of them. Its
  script's format is in its `script.rs`. A server's life against it is tested in
  `crates/oso/tests/lsp_server.rs`, where cargo builds it; this crate's own tests need no
  process.
- Every Rust file opens with its responsibility and a `Must not depend on:` paragraph (ADR
  0041). Anything `pub` is in the API snapshot (`just public-api`).
