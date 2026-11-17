# oso-lsp

The language server client (ADR 0138). Step 2 built the parts that spawn nothing: `framing`,
`json`, `uri`, `position` and `limits`. Step 3 added `protocol`, the messages of a server's
life as plain types, and `server`, its threads: the session thread (`server/session.rs`,
with `run.rs` and `incoming.rs`), the pipe threads (`pipes.rs`), the crash and flood rules
(`policy.rs`) and what is reported to the UI (`outbox.rs`). Step 4 added the documents a
server knows (`server/documents.rs`, with `session/sync.rs`): a whole text never goes past
half the writer's queue, and a document with a lone carriage return or over the bound is kept
from the server. Step 6 added diagnostics: reading `publishDiagnostics` (`protocol/published.rs`),
the reports a session keeps (`server/diagnosed.rs`), and the texts each document keeps to place a
late report against the version it names and map it forward (`server/documents.rs`). Step 6b
hands the UI the kept report of each file no open document holds (`Diagnosed::File`, shared),
with an empty one for each the store lets go, for a file once its document opens, and
`Diagnosed::Forgotten` at a run's end; the outbox keeps the latest of each document and file.
Step 8 asks about documents (`server/asks.rs`, `session/asks.rs`) and reads definitions
(`protocol/located.rs`); step 10 asks for a description, `textDocument/hover`, and reads it
within its bounds (`protocol/hover.rs`); step 13 asks for code actions over a range with the
diagnostics there as the server gave them, and reads an answer, a resolve and a workspace edit
whole or not at all (`protocol/actions.rs`, `protocol/workspace_edit.rs`), placed on the
session thread in the texts the server had as the request went out (`session/actions.rs`,
`session/edits.rs`), as the values the UI takes (`server/actions.rs`, `server/edits.rs`); the
last answer's actions are kept as the server gave them, but for the edits read, for a resolve
and a command's arguments. Step 13b runs a declared command and hands the UI the server's
`workspace/applyEdit` while it waits, whose answer waits for the UI (`session/apply.rs`).

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
- The UI's asks (step 8's definitions and references, step 9's completions and resolves) go to
  one request per document and kind, a newer one cancelling the older, each with its kind's
  wait and its `AskContext` (`server/asks.rs`, `server/session/asks.rs`); an answer's places
  and ranges are placed in the text the server has, a completion's range kept only around the
  place asked about, and a resolve's edits in the text it was asked in. The last completions'
  items are kept, as the server gave them, only while it resolves.
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
