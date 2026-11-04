# oso-platform

OS abstractions: the text system backends, and later IME, clipboard,
filesystem and the PTY (ADR 0003, ADR 0004). The dev control socket transport
lives in `control`, and process spawning in `process`, behind the `dev`
feature.

- Depends on no workspace crate. It is the only crate that may use OS
  bindings (`objc2*`, `core-foundation*`, `libc`, `windows*`, ...).
- OS details stay behind its types and traits: no CoreText, Win32 or libc
  types in its public API.
- Every `unsafe` block needs a `// SAFETY:` comment.
- Processes are spawned only through `oso-platform::process` (ADR 0010,
  ADR 0047), behind the `dev` feature until a release feature needs it.
  There is no network code.
- Each OS backend sits in its own `cfg`-gated module. Linux and Windows
  must keep building and passing tests in CI (Tier 2).
- Module layers are in `guardrails/layers.txt` (`[oso-platform]`).
- Every Rust file opens with its responsibility and a `Must not depend on:`
  paragraph (ADR 0041). The test checks names in backticks against the layers.
- Anything `pub` is in the API snapshot (`just public-api`).
