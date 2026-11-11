# oso

The tip crate and binary: commands, default key bindings, the event loop and
the glue between the library crates (ADR 0004). It changes most
often, so it must rebuild within 2 s.

- May depend on every library crate; nothing depends on it. It is the only
  crate that may use winit, and only `src/app/shell.rs` names it: the rest works
  on the events, keys and window calls that file gives (ADR 0070). On macOS,
  `src/app/native.rs` gives the same names over `oso_platform::window`'s own layer
  instead, and neither `shell.rs` nor winit is built.
- Commands are plain functions registered with a docstring and argument
  spec (`add-command` skill). They keep no state of their own. Each feature has
  a module in `src/commands/` and, if it has default keys, one of the same name
  in `src/bindings/`.
- `src/app.rs` holds the app's state and what its concerns share, and `src/app/`
  the concerns: `shell` runs winit, or `native` oso's own layer (ADR 0070), `events` routes, `input`, `ime`, `draw`,
  `loads`, `reloads`, `saves`, `copies`, `clipboard`, `searches`, `wraps` and `flash` do the work, `control`
  answers the dev socket, and `handover` exports and imports the state of an `oso dev` restart.
- `src/emergency.rs` and `src/emergency/` hold the panic hook and the emergency copies
  (ADR 0062), and recovering them (ADR 0064). Their writer sits above `saver`, and nothing
  there may hold text in a message or a log. A path in a `.meta` is never used to touch a file.
- `src/config/` turns the user configuration file's text into the user keymap and settings
  (ADR 0108, ADR 0110). It does no I/O, and only it uses `toml`.
- Keep logic that can be tested without a window in `oso-core`. This crate
  translates events and wires things together.
- Dev-only code (the control socket, `oso ctl`, `oso dev`, timings) stays
  behind the `dev` feature; `just release-safety` checks it.
- `src/channel/` is the agent channel and `oso mcp` (ADR 0085), in every
  build. It has no dev method, and runs only commands whose reach is `Look`
  or `Change`. `src/app/agent.rs` answers it on the UI thread, and
  `src/dispatch/agent.rs` runs its commands.
- End-to-end flows live in `tests/e2e` and run with `just e2e` (ADR 0049). They
  drive a dev build from outside, and start it through `oso_platform::process`.
- Module layers are in `guardrails/layers.txt` (`[oso]`).
- Every Rust file opens with its responsibility and a `Must not depend on:`
  paragraph (ADR 0041). The test checks names in backticks against the layers.
- The repository guard tests live in `tests/` (`workspace_lints.rs`,
  `architecture/`). Tests that need the `Dispatcher` stay inside the crate,
  including the text-grid snapshots in `src/dispatch/grid_tests.rs`
  (ADR 0045).
