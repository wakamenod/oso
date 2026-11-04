# oso-core

Editor logic with no GPU, window or OS dependency: the rope and `SumTree`,
the display model, cursors and selections, the command registry and
keymaps (ADR 0004). See `docs/dev/display-model.md`.

- Depends on no workspace crate. No wgpu, winit or OS bindings.
- `#![forbid(unsafe_code)]`. No `unwrap` or `expect` outside tests: return
  an error, or `#[expect(clippy::expect_used, reason = "...")]` for a real
  invariant.
- No global state. Commands are plain functions over the host-owned
  `Editor`, through `Ctx`, and work on every selection.
- Positions go through the display model; never assume one buffer line per
  screen row.
- Module layers are in `guardrails/layers.txt` (`[oso-core]`). For example,
  `display_map` must not use `command`.
- Anything `pub` is in the API snapshot (`just public-api`). A new public
  trait or module needs an ADR.
- Most logic is tested here with plain `cargo test`, often with `proptest`.
