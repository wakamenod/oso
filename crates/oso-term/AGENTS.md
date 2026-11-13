# oso-term

The terminal model (ADR 0120): libghostty-vt behind the crate-private `Engine` trait, oso's
escape-sequence policy, and a thread per terminal that owns its PTY, its child and its engine.

- May depend on `oso-core` and `oso-platform` only; only the tip crate depends on it. Module
  layers are in `guardrails/layers.txt` (`[oso-term]`).
- It parses untrusted output in memory-unsafe code. All `unsafe` is in `engine/ghostty/ffi.rs`,
  `raw.rs` and `raw/callbacks.rs` (the architecture test checks it): one operation per block,
  each with its `// SAFETY:` comment. Everything else is safe code over `raw`. No `unwrap` or
  `expect` outside tests.
- A callback runs inside the library's parse: it only pushes onto the sink, never calls back
  into a function that writes the terminal, and catches any panic.
- Every struct, constant and packed bit position in `ffi` and `ghostty::layout` is checked
  against the library's own manifest (`manifest_tests`). Add a declaration and its check
  together. The key modifier bits, `#define`s the manifest leaves out, are checked through the
  encoder instead (`keys` tests).
- The policy is one function per effect in `policy.rs`, and each row of ADR 0120's table has a
  test in `policy/tests.rs` that feeds the sequence. A new sequence gets its row and its test.
- Input is encoded on the terminal's thread, against the modes the program has by then: keys
  by the library's encoder, pastes and focus reports by `input.rs`, safe code. A paste keeps no
  control character but TAB and LF, so it cannot end its own bracket. Input that sends anything
  scrolls the view to the bottom; a focus report does not.
- What a sequence carries for oso to show goes through `policy::text`. Nothing here writes
  terminal text to a log line or the disk.
- Nothing grows without a bound in `limits.rs`. The input queue counts what a key or a paste
  may take until the thread has encoded it.
- The library is pinned in `ghostty-vt.lock` and listed in `third-party.md`. A bump changes
  both, re-reads each linked package, and is fuzzed again (`just fuzz-term`, ReleaseFast and
  ReleaseSafe, an hour each, on the Mac mini) before it lands. `build.rs` never downloads;
  `just ghostty-vt` fetches and checks the tarballs.
- Terminals exist on Unix only for now: on Windows nothing is built and `Terminal::start`
  returns `Unsupported`.
- Every Rust file opens with its responsibility and a `Must not depend on:` paragraph (ADR
  0041). Anything `pub` is in the API snapshot (`just public-api`).
