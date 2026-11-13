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
  the concerns: `shell` runs winit, or `native` oso's own layer (ADR 0070), `events` routes, `input`, `ime`, `draw`, `configuration`,
  `loads`, `reloads`, `saves`, `copies`, `clipboard`, `searches`, `migemo`, `slices`, `walks`, `greps`, `wraps`, `flash`, `jumps`, `reviews` and `terminals` do the work, `control`
  answers the dev socket, and `handover` exports and imports the state of an `oso dev` restart.
- `src/emergency.rs` and `src/emergency/` hold the panic hook and the emergency copies
  (ADR 0062), and recovering them (ADR 0064). Their writer sits above `saver`, and nothing
  there may hold text in a message or a log. A path in a `.meta` is never used to touch a file.
- `src/config/` reads the user configuration file and turns its text into the user keymap and
  settings (ADR 0108, ADR 0110), and reads and writes the record of trusted projects beside it
  (`config/trust.rs`, ADR 0114). `config/checked.rs` reads each such file, and one the
  configuration names (migemo's dictionary, ADR 0116), with ADR 0108's checks. Its parsing does
  no I/O. Only it and `sessions::rules`, which
  parses the agent rules files (ADR 0121), use `toml`, which review keeps: no guard test checks
  it.
- `src/terminal.rs` and `src/terminal/` host terminals (ADR 0120): which keys go to a program
  (`route`), what a terminal starts (`shell`), the screen the UI keeps (`grid`, `colors`) and
  the start thread (`hosted`). A hosted session's terminal finds its shell with
  `hosted::find_shell` and is put in its buffer by `app/terminals/start.rs`, as a terminal's. `oso-term` owns the PTY, the engine and the policy; the
  dispatcher holds each terminal's key-reading state (`dispatch/terminal.rs`), and
  `app/terminals.rs` carries out what commands and keys ask. Terminal text never reaches a
  log, the disk or the agent channel.
- `src/sessions.rs` and `src/sessions/` are agent sessions (ADR 0121): the rules that read a
  hosted session's state from its screen, the tracker, and the `oso-sessions` worker. Their
  patterns use `regex`, compiled with size limits; the screen is untrusted text. Hosted
  sessions get ready and start their terminal on `oso-session-start` (`start`), with a command
  line built by pure functions (`command`, `launch`); Claude Code's registry (`registry`) and
  oso's record (`record`) are read off the UI thread, and a bridge's claim is judged by one pure
  function (`bridge`). `session-adopt` reads Claude Code's transcripts (`transcripts`) only
  for ids, names, times and directories, on its own `oso-session-adopt-list` thread (`adopt`).
  `app/sessions.rs` and `app/sessions/` carry out what the worker and the start thread find.
- `src/review.rs` runs git and diffs for an agent review on `oso-review` (ADR 0122), only for
  a trusted project's root, and git only through `oso_platform::process`. `review/view.rs`
  lays out a review buffer's whole text on `oso-review-view`, one thread per review, and
  `review/count.rs` counts the review menu's scopes on `oso-review-count`. A review buffer's
  keys are listed in `commands::review::KEYS`, which `?` reads, and bound by
  `bindings/review.rs`.
- `src/walk.rs` walks a project's files on `oso-walk` (ADR 0114). Only it uses `ignore`, which
  review keeps: no guard test checks it. `src/grep.rs` searches them on `oso-grep` over the same
  walk, and `src/grep/preview.rs` reads a row's file for the preview on `oso-preview`.
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
