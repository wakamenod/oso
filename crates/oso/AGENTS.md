# oso

The tip crate and binary: commands, default key bindings, the winit event
loop and the glue between the library crates (ADR 0004). It changes most
often, so it must rebuild within 2 s.

- May depend on every library crate; nothing depends on it. It is the only
  crate that may use winit.
- Commands are plain functions registered with a docstring and argument
  spec (`add-command` skill). They keep no state of their own.
- Keep logic that can be tested without a window in `oso-core`. This crate
  translates events and wires things together.
- Dev-only code (the control socket, `oso ctl`, `oso dev`) stays behind the
  `dev` feature; `just release-safety` checks it.
- Module layers are in `guardrails/layers.txt` (`[oso]`).
- Every Rust file opens with its responsibility and a `Must not depend on:`
  paragraph (ADR 0041). The test checks names in backticks against the layers.
- The repository guard tests live in `tests/` (`workspace_lints.rs`,
  `architecture/`). Tests that need the `Dispatcher` stay inside the crate,
  including the text-grid snapshots in `src/dispatch/grid_tests.rs`
  (ADR 0045).
