# oso-lsp

The language server client (ADR 0138). Step 2 built the parts that spawn nothing: `framing`,
`json`, `uri`, `position` and `limits`, and the fake server in `examples/fake_server/`.

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
- The fake server is an example, so it never ships. Its replies are built by hand, framed
  by `framing::encode`, so a test of the client checks the client's reading of them. Its script's format is in
  `examples/fake_server/script.rs`; `test = true` in `Cargo.toml` runs that file's tests.
- Every Rust file opens with its responsibility and a `Must not depend on:` paragraph (ADR
  0041). Anything `pub` is in the API snapshot (`just public-api`).
