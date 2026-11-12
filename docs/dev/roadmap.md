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
- **M1-7b** (done): the rest of the control socket and the restart-and-restore
  loop.
  - `run-command`, the full `get-state`, `get-text` and the secret-pattern
    rules ([ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md))
  - `oso_platform::process`, `export-state` and `import-state`, and `oso dev`
    with `just dev` ([ADR 0047](adr/0047-process-spawning-and-restart-handoff.md))
- **M1-8** (done): performance measurement in CI
  ([ADR 0024](adr/0024-dev-build-profile.md),
  [ADR 0025](adr/0025-latency-measurement.md),
  [ADR 0046](adr/0046-performance-regression-checks.md)). Startup still misses
  its budget; see `open-questions.md`.
- **M1-9**: test infrastructure from the test pyramid
  ([ADR 0037](adr/0037-test-pyramid.md)), in three steps, each started as
  soon as its prerequisite merges:
  - **M1-9a** (done): weekly mutation testing, coverage reports and the 30 s
    test time budget ([ADR 0050](adr/0050-test-quality-checks.md)).
  - **M1-9b** (done): the text-grid snapshot harness
    ([ADR 0045](adr/0045-text-grid-snapshots.md)).
  - **M1-9c** (done): automated end-to-end tests over the control socket
    (`just e2e`, and CI on Linux with lavapipe and on macOS;
    [ADR 0049](adr/0049-end-to-end-test-harness.md)).
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

M2 is built in this order. Each step is one PR, and a step starts as soon as
what it depends on has merged, with at most three running at once. The
decisions behind this plan are in
[ADR 0051](adr/0051-m2-plan-decisions.md). The UI is in [`ui.md`](ui.md)
([ADR 0026](adr/0026-m2-ui-layout.md)), and command semantics are in
[ADR 0030](adr/0030-m2-editing-commands.md). Command = Meta and Option = Super
throughout.

**The base.** Text can change only once all of this holds.

- **In parallel:**
  - **M2-0** (done): make room, with no change in behavior. Commands and bindings
    in one module per feature, `app.rs` split into input, drawing, loads and
    events, and `Window::scroll` private.
  - **M2-1**: the edit core in `oso-core`: edits applied atomically across all
    cursors, selections mapped in every window, the modified flag, linear
    undo/redo with one step per command, and the message store
    ([ADR 0007](adr/0007-multi-cursor.md)).
  - **M2-3**: shaped rows compared by content, so editing shows no stale glyphs
    and reshapes only display rows whose text changed. A line break reshapes the
    rows below it.
- **M2-2** (after M2-1): `oso dev` holds a restart while a buffer is modified, and
  never closes or kills an instance that has been edited
  ([ADR 0047](adr/0047-process-spawning-and-restart-handoff.md),
  [ADR 0055](adr/0055-oso-dev-never-loses-unsaved-text.md)).
- **M2-4** (after M2-0 to M2-3): typing: self-insert, `RET`, `DEL`, `C-d`, `TAB`,
  `C-z`/`C-M-z`, and a typing metric in the benchmarks
  ([ADR 0056](adr/0056-typing.md)). It removes the dev method `insert-text`, whose
  flow moves to `send-keys`. A throwaway branch that logs IME events runs alongside
  it, and its findings go to M2-12.

**The screen, and keeping work safe.**

- **In parallel, after M2-4:**
  - **M2-5**: `oso_core::frame`, the pure layout of a frame into regions, and
    text-grid snapshots that compose them. The modeline and echo rows are
    reserved.
  - **M2-8**: safe save (`C-x C-s`) on a background thread, and a guard on exiting
    with unsaved work ([ADR 0013](adr/0013-persistence.md),
    [ADR 0029](adr/0029-file-changes-on-disk.md),
    [ADR 0059](adr/0059-safe-save-and-the-exit-guard.md)).
  - **M2-9**: word motion with Japanese rules, `C-a`/`C-e` as in mwim, `track-eol`,
    `C-u` and `M-<digit>`, `C-l`, and the horizontal wheel
    ([ADR 0058](adr/0058-word-motion-prefix-argument-and-secret-positions.md)).
- **M2-6** (after M2-5, and after the work in progress in the renderer's text pass
  merges): a list of panes drawn with clipping and layers, the base of everything
  below that draws ([ADR 0008](adr/0008-floating-ui.md)). It decides the partly shown
  column on the right ([ADR 0057](adr/0057-frame-layout.md)).
- **M2-7** (after M2-6): modeline, echo area, `*Messages*` and internal buffers,
  which `oso dev`'s check, generation and import must skip
  ([ADR 0055](adr/0055-oso-dev-never-loses-unsaved-text.md)). A load that grows rows
  below the view no longer asks for a frame, so the modeline's loading progress must
  ask for its own, and a frame too small for a modeline needs a place for its warnings
  ([ADR 0057](adr/0057-frame-layout.md)).
- **M2-10** (after M2-8): emergency save on a panic or an OS-initiated exit, the
  panic hook, which must ignore a failed write since `oso dev`'s instances can outlive
  their terminal ([ADR 0055](adr/0055-oso-dev-never-loses-unsaved-text.md)), and no core
  dumps ([ADR 0013](adr/0013-persistence.md),
  [ADR 0014](adr/0014-panic-strategy.md), [ADR 0028](adr/0028-secrets-handling.md)).
- **M2-11** (after M2-7 and M2-10): recovering an emergency copy
  ([ADR 0064](adr/0064-recovering-an-emergency-copy.md)). M2-12 must switch the IME off in its list.
- **M2-12** (after M2-4 and M2-6): the macOS IME: inline preedit that pushes the
  text after it, the candidate window, one undo step per commit, and off in the
  minibuffer, isearch and menus.

**The author can edit real notes without risking them once M2-12 has merged,** with
M2-4, M2-7, M2-8, M2-10 and M2-11. Files open only as `oso FILE`, and `oso dev` holds
restarts while text is unsaved. There is no kill and yank, opening files from inside,
search or auto-revert yet. Everyday use starts after M2-14 and M2-18.

**Everyday editing.**

- **M2-13** (after M2-6): mark and region, drawn as rectangles in the window's pane
  ([ADR 0071](adr/0071-mark-and-region.md)).
- **M2-14** (after M2-13): kill ring, with `M-d` and `M-DEL` as kills, `C-w`, `M-w`, `C-k`, `C-y`, typing over a
  region, per-cursor kills and the OS clipboard, on macOS first
  ([ADR 0074](adr/0074-the-kill-ring-and-the-clipboard.md)).
- **M2-15** (after M2-2, M2-7, M2-13 and M2-14): `oso dev` carries unsaved text, and
  sends build errors to the echo area
  ([ADR 0023](adr/0023-control-socket-protocol-and-restart.md),
  [ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md),
  [ADR 0090](adr/0090-oso-dev-carries-unsaved-text.md)).
- **M2-16** (after M2-7): floating windows and minibuffer input, reading a command's
  arguments, yes/no prompts and the `C-g` chain ([ADR 0008](adr/0008-floating-ui.md)).
- **M2-17** (after M2-16): completion: the orderless matcher, 18 candidate rows,
  annotations, history and `M-x`.
- **M2-18** (after M2-8 and M2-17): `C-x C-f` with path-segment completion on a
  worker thread, `C-x C-w`, asking before opening files of 2 GiB or more, and opening
  that never blocks on a special file
  ([ADR 0087](adr/0087-finding-files-writing-under-a-new-name-and-large-files.md)).
- **M2-19** (after M2-17): `C-x b` with in-popup preview, `kill-buffer`, distinct
  names for files with one name, `M-,`/`M-/`, a window's place in each buffer, and `C-.`
  ([ADR 0088](adr/0088-switching-and-killing-buffers.md)).
- **M2-20** (after M2-7 and M2-13): isearch (`C-s`/`C-r`) with smart case, match
  highlighting and a count from the background.
- **M2-21** (after M2-14, M2-17, M2-19 and M2-20): multiple-cursor keys (`C->`, `C-<`, their skips,
  `C-c C-<`, `C-x SPC` and the same keys in isearch), at most 1000 cursors with the edits
  batched, and `M-y` from the kill ring in the centred float with a preview
  ([ADR 0007](adr/0007-multi-cursor.md), [ADR 0091](adr/0091-multiple-cursor-keys-and-m-y.md)).
- **M2-22** (after M2-8 and M2-16): reloading files changed on disk
  ([ADR 0029](adr/0029-file-changes-on-disk.md)).
- **M2-23** (after M2-13, M2-17 and M2-20, and after the work in the text pass merges):
  three built-in themes, `dark`, `light` and `dusk`, switched at run time with `load-theme`
  and `<f5>` and remembered across starts, with the contrast check for every theme, text
  gamma, and gaps between invisible boxes ([ADR 0027](adr/0027-theme.md),
  [ADR 0089](adr/0089-built-in-themes-switched-at-run-time.md)).
- **M2-24** (after M2-23): visual aids: whitespace, current line, cursor flash and
  line numbers ([ADR 0097](adr/0097-visual-aids.md)).
- **M2-25** (after M2-9 and M2-24): soft wrap, with the WrapMap
  ([ADR 0006](adr/0006-display-model.md)).

**The agent channel, pulled forward from M4** ([ADR 0085](adr/0085-agent-channel-and-oso-mcp.md)),
alongside the steps above, after M2-16:

- **AC-1**: a reach on every command, and the agent channel in release builds: `run`, `open`,
  `state`, `text`, `commands` and `cancel`, `ArgSpec::Text` and `insert`, the connection question,
  the `Agent` segment and `*Agent*`, and the change to `just release-safety`.
- **AC-2** (after AC-1): `oso mcp`, the stdio bridge, with the tools `state`, `text`, `commands`,
  `look`, `change` and `open`, and then `cancel` ([ADR 0093](adr/0093-a-cancel-tool-in-oso-mcp.md)).
- Each later step gives its commands a reach, such as `C-x C-f`, `C-x b` and window splits.

### M3 — Coding

- Loading the TOML configuration, with a user key layer above the global keymap
  ([ADR 0019](adr/0019-configuration.md),
  [ADR 0051](adr/0051-m2-plan-decisions.md)).
  A setting starts the agent channel at launch
  ([ADR 0085](adr/0085-agent-channel-and-oso-mcp.md)).
- Decided: what an agent's open or save starts waits in that buffer for the person, from M4
  ([ADR 0118](adr/0118-what-an-agents-open-or-save-starts.md)). Nothing in M3 has to wait: what
  opening or saving starts in M3 runs in oso's own process, and the release build cannot spawn.
- Tree-sitter highlighting, starting with Rust, Markdown and TOML
  ([ADR 0111](adr/0111-tree-sitter-highlighting.md)). Later, Markdown's fenced code tagged
  `rust` or `toml` is highlighted with those grammars.
- Window splits ([ADR 0112](adr/0112-window-splits.md)), indentation
  ([ADR 0115](adr/0115-indentation.md)), project trust model.
- Project file finding and asynchronous ripgrep-style search, in-process
  ([ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md)).
- Migemo-style romaji search for Japanese text, and avy-style jumps
  ([ADR 0116](adr/0116-migemo-and-jumps.md)).
- The menu component, porting the author's hydra menus.
- query-replace (`M-%`), keyboard macros, whitespace cleanup on save.
- Comment-aware `C-e` using tree-sitter's highlights
  ([ADR 0030](adr/0030-m2-editing-commands.md), [ADR 0111](adr/0111-tree-sitter-highlighting.md)).

### M4 — Self-hosting (develop oso in oso)

- First, with the first program that opening or saving starts (format on save or language
  servers): what an agent's open or save starts waits for the person in that buffer, a `waits`
  modeline segment, and the agent told so
  ([ADR 0118](adr/0118-what-an-agents-open-or-save-starts.md)).
- Language servers: diagnostics, go to definition, completion.
- A git-porcelain subset: status, staging hunks, commit, push/pull, log,
  conflict resolution.
- Compile buffer (running `just ci`), format on save.
- The terminal, replacing a terminal app
  ([ADR 0120](adr/0120-the-terminal.md)): a PTY of oso's own, the engine chosen by
  a time-boxed comparison, a thread per terminal, semi-char keys with `C-s` search
  and copy mode, the escape-sequence policy, project terminals (`C-x p m`,
  `C-x p M`), a floating terminal (`` C-` ``), shell commands that run listed oso
  commands, Kitty images, the seam for the compile buffer, and terminals that
  survive `oso dev`'s restart. OSC 7501 status comes with sessions (ADR 0121).
- Claude Code integration ([ADR 0031](adr/0031-claude-code-integration.md)):
  the official `claude` TUI in a terminal view, hooks for session identity
  and state, review and worktree tools on the `oso mcp` bridge, session
  list, notifications, worktree sessions, and restore via `--resume`.

### After M4

- Directory editor (dired-like), org subset.
- Terminal: ConPTY on Windows, Sixel and iTerm2 images
  ([ADR 0120](adr/0120-the-terminal.md)).
- A wiki/GTD client, to be considered.
- Tabs and multiple frames.
- Inline images and a PDF viewer.
- Each of these includes only the commands the author actually uses.

The UI of each feature (for example, where the minibuffer appears) is
discussed and decided before that feature is built.
