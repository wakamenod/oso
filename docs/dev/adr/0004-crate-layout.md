# 0004. Workspace crate layout

- Status: Accepted, amended by 0111
- Date: 2026-11-03

## Context

Incremental builds must stay fast for hot reload
([ADR 0002](0002-dev-hot-reload.md)). Core logic should be testable without a
GPU, and `unsafe` should be confined. Tools like subsecond can patch only the
tip crate.

## Decision

```
crates/oso           bin (tip): commands, default keymaps, app glue
crates/oso-core      buffers, selections, display model, registry, keymaps, config
                     no GPU dependency; #![forbid(unsafe_code)]
crates/oso-render    wgpu pipeline, glyph atlas, text-system trait
crates/oso-platform  OS abstractions: text backends, IME, clipboard, fs, process, PTY
```

- Code that changes often (commands) lives in the tip crate. Stable
  foundations live in libraries.
- Rust edition 2024. Release builds use LTO and `codegen-units = 1`.

## Consequences

- Command edits rebuild only the small tip crate.
- `oso-core` is pure logic, which suits property tests and agents.
- Changes in library crates need a full restart; restart-and-restore handles
  this.

## Alternatives considered

- **Fewer crates (bin + core):** simpler at first, but mixes GPU and OS code
  with logic.
- **One crate per feature (git, terminal, org):** possible later when
  features grow. Too early now.
