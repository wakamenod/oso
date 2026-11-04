# Roadmap

## Performance budgets

These budgets are measured from M1 onward, and regressions fail CI.

| Metric | Budget |
|---|---|
| Key press to photon | p99 ≤ 8 ms (one frame at 120 Hz) |
| Startup to first frame | ≤ 100 ms |
| Open a 100 MB file | first screen ≤ 50 ms, rest loads in the background |
| 1 GB file, or a 10 MB single line | no freeze while scrolling or typing |
| Dev incremental build of the tip crate | ≤ 2 s |

## Milestones

### M0 — done

A cursor rectangle drawn with wgpu that moves with the arrow keys.

### M1 — Viewer

M1 is built in this order. Each step is one PR.

- **M1-0** (done): development tooling
  ([ADR 0021](adr/0021-development-tooling.md)).
- **M1-1** (done): quick fixes to the current code:
  - `ControlFlow::Wait` instead of `Poll`
  - an explicit present mode and frame latency 1
  - one backend per OS
  - a low-power adapter
  - typed surface-error handling
- **M1-2** (done): workspace split into four crates
  ([ADR 0004](adr/0004-crate-layout.md)).
  - Only each OS's wgpu backend is compiled in, with
    `default-features = false` and per-target backend features
    ([ADR 0003](adr/0003-platform-tiers.md)).
- **M1-7a** (done): dev control socket foundation, so that agents can check
  their own work in the app from here on
  ([ADR 0016](adr/0016-dev-control-socket.md),
  [ADR 0023](adr/0023-control-socket-protocol-and-restart.md),
  [ADR 0032](adr/0032-control-socket-transport.md),
  [ADR 0033](adr/0033-key-presses-and-notation.md)):
  - the socket (`0700` directory, peer user check), JSON Lines and `oso ctl`
  - `screenshot`, and `send-keys` injecting key events into the app
  - the key and modifier type in `oso-core`, which M1-3 builds on
  - a CI check that release builds contain no dev-only code
  - Windows is deferred: `oso ctl` reports that it is not supported there
    yet. Its transport is the point to look again at the Linux-only API
    snapshots ([ADR 0042](adr/0042-statics-snapshots-and-render-errors.md)).
- **In parallel** after M1-7a:
  - **M1-3** (done): command registry and keymaps with prefix keys
    ([ADR 0034](adr/0034-command-registry-and-keymaps.md)).
  - **M1-4** (done): `SumTree`, rope and display model skeleton
    ([`display-model.md`](display-model.md),
    [ADR 0022](adr/0022-text-buffer-and-file-loading.md),
    [ADR 0035](adr/0035-sumtree-rope-and-cell-grid.md)).
  - **M1-5** (done): OS-native text rendering on macOS
    ([ADR 0036](adr/0036-text-system-and-glyph-atlas.md)): CoreText behind
    the text-system trait, a glyph atlas and pass 2, and a box cursor on
    the cell grid. Linux and Windows draw no text until their backends
    exist. A temporary sample is drawn until M1-6.
- **M1-6** (done): asynchronous, progressive file loading; scrolling and
  cursor motion: `C-f/b/n/p`, `C-a/e`, `M-<`/`M->`, `C-v`/`M-v`
  ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)):
  - `oso [FILE]`; buffers, windows and selection sets in `oso-core`
  - a loading thread that decodes and builds the rope and display map,
    with `\xNN` for invalid bytes
  - motion by display cluster, conservative scrolling, the mouse wheel
  - buffer text drawn through the display model; the temporary sample is
    gone
- **M1-7b**: the rest of the control socket (`run-command`, the full
  `get-state`, `export-state` and `import-state`) and the restart-and-restore
  loop (`oso dev`).
- **M1-8** (done): performance measurement in CI
  ([ADR 0024](adr/0024-dev-build-profile.md),
  [ADR 0025](adr/0025-latency-measurement.md),
  [ADR 0044](adr/0044-performance-regression-checks.md)). Startup still misses
  its budget; see `open-questions.md`.
- **M1-9**: test infrastructure from the test pyramid
  ([ADR 0037](adr/0037-test-pyramid.md)), in three steps, each started as
  soon as its prerequisite merges:
  - **M1-9a** (right after M1-10a merges, since both change CI and the
    `Justfile`): weekly mutation testing, coverage reports and the 30 s test
    time budget.
  - **M1-9b** (right after M1-6 merges, since it snapshots real buffer
    views): the text-grid snapshot harness.
  - **M1-9c** (after M1-7b, which completes `get-state`): automated
    end-to-end tests over the control socket (`just e2e`, and Linux CI with
    lavapipe).
- **M1-10a** (done): architecture guardrails
  ([ADR 0038](adr/0038-architecture-guardrails.md),
  [ADR 0039](adr/0039-architecture-guardrail-mechanics.md)):
  - dependency-direction rules between crates and modules
  - public API snapshots
  - size limits with a ratchet allowlist
  - design-rule lints
  - the `arch-reviewer` subagent and the PR template with the ADR check
  - crate `AGENTS.md` files
  - the weekly design health check
- **M1-10b** (done): module responsibility headers in every source file and
  the test that requires them
  ([ADR 0041](adr/0041-module-responsibility-headers.md)), the ban on `static`
  items and the render errors without wgpu types
  ([ADR 0042](adr/0042-statics-snapshots-and-render-errors.md)).

### M2 — Editor (start using oso for text and notes)

- Insertion and deletion; Japanese input through the macOS IME, switched off
  automatically in the minibuffer and isearch.
- Undo/redo, safe save and emergency save on panic.
- Kill ring, mark and region, multiple cursors.
- isearch (`C-s`/`C-r`).
- Minibuffer with fuzzy matching and preview: `M-x`, `C-x C-f`, `C-x b`, `C-g`.
- The UI from [`ui.md`](ui.md): top-center minibuffer with in-popup
  preview, echo area, per-window modelines, isearch match count, inline IME
  preedit, visual aids, and the dark/light theme
  ([ADR 0026](adr/0026-m2-ui-layout.md), [ADR 0027](adr/0027-theme.md)).
- Reloading files changed on disk ([ADR 0029](adr/0029-file-changes-on-disk.md)).
- Editing command semantics from [ADR 0030](adr/0030-m2-editing-commands.md):
  the author's key customizations, linear undo/redo (`C-z`/`C-M-z`), `M-y`
  from the kill ring, Japanese word motion, multiple-cursor keys,
  path-segment file completion and smart-case isearch.
- Command = Meta, Option = Super.

### M3 — Coding

- Tree-sitter highlighting, starting with Rust, Markdown and TOML.
- Window splits, indentation, project trust model.
- Project file finding and asynchronous ripgrep search.
- Migemo-style romaji search for Japanese text, and avy-style jumps.
- The menu component, porting the author's hydra menus.
- query-replace (`M-%`), keyboard macros, whitespace cleanup on save.
- Comment-aware `C-e` using tree-sitter
  ([ADR 0030](adr/0030-m2-editing-commands.md)).

### M4 — Self-hosting (develop oso in oso)

- Language servers: diagnostics, go to definition, completion.
- A git-porcelain subset: status, staging hunks, commit, push/pull, log,
  conflict resolution.
- Compile buffer (running `just ci`), format on save.
- Terminal view for hosting agents: PTY, `alacritty_terminal` rendering,
  input, searchable text, escape-sequence policy, OSC 7501 status
  ([ADR 0015](adr/0015-terminal.md)).
- Claude Code integration ([ADR 0031](adr/0031-claude-code-integration.md)):
  the official `claude` TUI in a terminal view, hooks for session identity
  and state, the `oso mcp` bridge for review and worktree tools, session
  list, notifications, worktree sessions, and restore via `--resume`.

### After M4

- Directory editor (dired-like), org subset.
- Pop-up terminal and the terminal as the backend for compile.
- A wiki/GTD client, to be considered.
- Tabs and multiple frames.
- Inline images and a PDF viewer.
- Each of these includes only the commands the author actually uses.

The UI of each feature (for example, where the minibuffer appears) is
discussed and decided before that feature is built.
