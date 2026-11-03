# Architecture

oso is an Emacs-like GUI text editor written in Rust. This document describes
the overall structure. Individual decisions and their rationale live in
[`adr/`](adr/README.md); open topics are tracked in
[`open-questions.md`](open-questions.md).

## Goals, in priority order

1. **Speed.** Opening a file or typing must never stutter. See the budgets in
   [`roadmap.md`](roadmap.md#performance-budgets).
2. **Security.** Opening a file or a repository never executes anything.
   See [`security.md`](security.md).
3. **Extensibility for the developer.** Many commands will be added over time,
   so adding one must be cheap. There is no user scripting
   ([ADR 0001](adr/0001-no-user-scripting.md)).
4. **Fast iteration.** Code changes reach the screen in 2–3 seconds during
   development ([ADR 0002](adr/0002-dev-hot-reload.md)).

The primary user is the author. Others may use oso as-is. Documentation is
kept thorough regardless.

## Platforms

Linux, macOS and Windows are supported by design. macOS is Tier 1 (daily use,
visual QA); Linux and Windows are Tier 2 (CI build and tests). OS-specific code
sits behind traits in `oso-platform`. See [ADR 0003](adr/0003-platform-tiers.md).

## Crate layout

```
crates/
├── oso/            bin, "tip" crate: commands, default keymaps, app glue
├── oso-core/       buffers, selections, display model, command registry,
│                   keymaps, config; no GPU, #![forbid(unsafe_code)]
├── oso-render/     wgpu pipeline, glyph atlas, text-system trait
└── oso-platform/   OS abstractions: text shaping/rasterizing backends, IME,
                    clipboard, filesystem and paths, process spawning, PTY
```

Commands live in the tip crate because it changes most often and rebuilds
fastest. Stable foundations live in library crates. See
[ADR 0004](adr/0004-crate-layout.md).

## Layers

```
OS events (winit)
   │
   ▼
Input: key translation (Command = Meta, Option = Super on macOS), IME
   │
   ▼
Keymap lookup: keymap-at-point > minor modes > major mode > global
   │
   ▼
Command registry: commands are plain functions over host-owned state
   fn(&mut Ctx, Args) -> Result<()>   (run once per command, across all cursors)
   │
   ▼
oso-core state: buffers (rope), per-window selection sets, decoration layers
   │
   ▼
Display model: buffer → folds → inlays → tabs → wraps → blocks → display rows
   │
   ▼
oso-render: lay out only visible rows on a monospace cell grid, draw with wgpu
```

### Text core

- **Rope buffer** built on oso's own `SumTree`, the same tree the display
  model uses. Offset and line conversions take O(log n), byte offsets are the
  primary coordinate, and snapshots are cheap to clone and share with
  background threads. Text is UTF-8 for now. See
  [ADR 0022](adr/0022-text-buffer-and-file-loading.md).
- **Selections** are per window and support multiple cursors. Each selection
  has its own mark. See [ADR 0007](adr/0007-multi-cursor.md).
- **Undo** groups one command's edits across all cursors. Undo history is not
  persisted to disk.

### Command registry

Every command is registered with a name, a docstring, an argument spec and a
function. The registry drives `M-x` completion, `describe-function`,
`describe-key` and the generated command reference. Commands never keep global
state: the host owns all state and passes it in. This keeps commands
testable, restartable and hot-patchable.

A panicking command is caught at the command boundary. The editor reports the
error and keeps running ([ADR 0014](adr/0014-panic-strategy.md)).

### Display model

The display model maps buffer positions to screen rows and back, in O(log n),
through a stack of transforms. It supports folding, virtual text, soft wrap
(off by default), block decorations and variable row heights. See
[`display-model.md`](display-model.md) and
[ADR 0006](adr/0006-display-model.md).

### Text rendering

Text shaping and rasterization use OS-native APIs behind a trait: CoreText on
macOS and DirectWrite on Windows. The Linux backend is still open. Layout uses
a monospace cell grid: half-width characters take one cell and full-width
characters take two. See [ADR 0005](adr/0005-text-rendering.md).

### GPU rendering

wgpu with one backend per OS: Metal, DX12 and Vulkan. There is no GL
fallback. Each frame runs independent passes:

```
Pass 1: background, cursors, selections, underlines   (shape renderer)
Pass 2: glyphs from the atlas                          (text renderer)
Pass 3: images and other content                       (added when needed)
```

The event loop uses `ControlFlow::Wait`. Frames are drawn only on demand:
on input that changes the view, on resize, and when the window becomes visible.
The surface presents with `Fifo` and a frame latency of 1. Fifo is always
supported and never tears, and on-demand drawing means it costs nothing while
idle. Mailbox or Immediate can be revisited once key-to-photon latency is
measured ([ADR 0025](adr/0025-latency-measurement.md)). The adapter is
requested with low power preference, which keeps dual-GPU machines on the
integrated GPU.

Surface errors are mapped to typed recovery actions: an outdated surface is
reconfigured, a lost one is recreated, a timeout retries the frame, and an
occluded window skips it until it is visible again. At most three failed frames
in a row are retried, so a surface that does not recover cannot spin the event
loop. Render errors are logged and are never fatal.

### Views and buffers

| Buffer kind | Backing | Examples |
|---|---|---|
| File | rope loaded from disk | source files, notes |
| Generated | rope produced by a mode, read-only by default | git status, directory listings, compile output, help |
| Terminal | `alacritty_terminal` grid | shell sessions |

A window shows a view. A text view renders a buffer through the display model.
A terminal view renders a cell grid and exposes its content as searchable
text, so isearch, line search and copy work as in text buffers. See
[ADR 0015](adr/0015-terminal.md).

The minibuffer, menus (hydra/transient-style), completion popups and hover
documentation are buffers shown in floating windows. Editing commands work
inside them unchanged. See [ADR 0008](adr/0008-floating-ui.md).

### Background work

The UI thread never blocks on I/O or parsing:

- Files load on a background thread. The first screen is shown as soon as it
  is available, and the buffer stays read-only until loading finishes.
  Invalid UTF-8 and binary files open read-only, with escapes for invalid
  bytes ([ADR 0022](adr/0022-text-buffer-and-file-loading.md)).
- Tree-sitter parsing runs on a background thread with cancellation and
  timeouts. Results replace the syntax decoration layer atomically.
- Search (ripgrep-style), git status and language servers stream results in
  asynchronously.

### Configuration

User configuration is TOML: key bindings, menus, theme, fonts and per-mode
settings. It reloads on file change. Inside repositories, only EditorConfig is
read. See [ADR 0019](adr/0019-configuration.md).

### Persistence

There is no periodic auto-save and there are no backup files. If oso panics,
unsaved buffers are written to a private directory (mode `0700`, files
`0600`), skipping secret-looking files. Saving uses a temporary file, fsync and
rename. See [ADR 0013](adr/0013-persistence.md).

## Security architecture (summary)

- Projects open in **restricted mode** until trusted with `M-x trust-project`.
  Trust is stored in user config, never in the repository
  ([ADR 0009](adr/0009-project-trust.md)).
- Child processes are spawned only through `oso-platform::process`, and a
  lint enforces this. oso contains no network code
  ([ADR 0010](adr/0010-process-and-network-policy.md)).
- Git status is read in-process with `gix`. Commands defined in git config are
  never run ([ADR 0011](adr/0011-git-integration.md)).

Full details are in [`security.md`](security.md).

## Development tooling (dev builds only)

- **Hot reload:** config and shader reload on file change. Code changes
  trigger an incremental build, then a restart with in-memory state handoff
  ([ADR 0002](adr/0002-dev-hot-reload.md)). The `oso dev` supervisor
  (`just dev`) drives the build and restart, and the old instance exits only
  after the new one has imported its state
  ([ADR 0023](adr/0023-control-socket-protocol-and-restart.md)).
- **Control socket:** agents can send keys, run commands, read state and take
  screenshots ([ADR 0016](adr/0016-dev-control-socket.md)). The protocol is
  JSON Lines, and `oso ctl` is the client
  ([ADR 0023](adr/0023-control-socket-protocol-and-restart.md)).
- **Dev profile:** dependencies are optimized and oso is not, to keep the
  tip-crate rebuild within 2 s ([ADR 0024](adr/0024-dev-build-profile.md)).

Both are behind cargo features that release builds never enable, and CI
checks this.
