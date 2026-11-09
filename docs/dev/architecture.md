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
├── oso-render/     wgpu pipeline, glyph atlas
└── oso-platform/   OS abstractions: the text-system trait and its
                    shaping/rasterizing backends, the window layer,
                    clipboard, filesystem and paths, process spawning, PTY
```

Commands live in the tip crate because it changes most often and rebuilds
fastest. Stable foundations live in library crates. See
[ADR 0004](adr/0004-crate-layout.md).

Files are read with portable `std::fs` calls in the tip crate's
loader ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)),
and opened through `oso_platform::file_meta::open_for_reading`, which does
not wait on a special file
([ADR 0087](adr/0087-finding-files-writing-under-a-new-name-and-large-files.md)).
What needs `libc` stays in `oso-platform`. Saving uses `std`'s Unix file calls in the tip crate
([ADR 0059](adr/0059-safe-save-and-the-exit-guard.md)), and for what a file
holds beyond its mode, and for opening it without waiting,
`oso_platform::file_meta`
([ADR 0065](adr/0065-access-lists-on-private-directories.md),
[ADR 0067](adr/0067-access-lists-and-the-quarantine-flag-in-a-save.md)).

Dependencies point one way: `oso` depends on the libraries, `oso-render`
depends on `oso-core` and `oso-platform`, and neither of those depends on
another workspace crate. Only `oso-render` uses wgpu, and only
`oso-platform` uses OS bindings. These rules, and the allowed layers of
modules inside each crate, are listed in
[`guardrails/layers.txt`](../../guardrails/layers.txt) and enforced by
`crates/oso/tests/architecture`
([ADR 0038](adr/0038-architecture-guardrails.md),
[ADR 0039](adr/0039-architecture-guardrail-mechanics.md)).

Only the tip crate uses winit, and in it only `app/shell.rs`: the rest of the app
works on oso's own events, keys and window calls, `oso_platform::window`'s, so that a
window layer of oso's own can replace winit there
([ADR 0070](adr/0070-windowing-and-input-without-winit.md)). On macOS that layer exists
behind the `native-window` feature, off by default: `oso_platform::window` runs
`NSApplication`'s loop with a view of its own, and `app/native.rs` gives the app the same
loop, window and waker over it. The
tip crate owns the window and hands `oso-render` a `wgpu::DisplayAndWindowHandle` and the
size in pixels; the renderer reports back when it needs a redraw instead of calling into
the window.

## Layers

```
OS events (winit, through app/shell.rs)
   │
   ▼
Input: key translation into oso_core::key::KeyPress
       (Command = Meta, Option = Super on macOS; ADR 0033), IME
   │
   ▼
Keymap lookup: keymap-at-point > minor modes > major mode > global
   │           (the key resolver keeps a pending prefix such as C-x)
   ▼
Command registry: commands are plain functions over host-owned state
   fn(&mut Ctx, Args) -> Result<(), CommandError>
   (run once per command, across all cursors)
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

- **Rope buffer** built on oso's own `SumTree` (`oso_core::sum_tree`), the
  same tree the display model uses. The rope (`oso_core::rope`) holds UTF-8
  chunks of up to 1 KB that end on grapheme cluster boundaries. Offset and
  line conversions take O(log n), byte offsets are the primary coordinate,
  and snapshots are cheap to clone and share with background threads. Text
  is UTF-8 for now. See
  [ADR 0022](adr/0022-text-buffer-and-file-loading.md) and
  [ADR 0035](adr/0035-sumtree-rope-and-cell-grid.md).
- **Buffers and windows** live in `oso_core::editor::Editor`. A buffer
  (`oso_core::buffer`) holds the rope, its display map (which holds the
  invalid bytes), its file information, its loading progress and its undo
  history. A window
  (`oso_core::window`) shows one buffer with its own selection set and
  scroll position. There is one window until splits (M3). See
  [ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md). A pure function,
  `oso_core::frame::layout`, divides the frame into whole cells: its size is the
  window's text area, above a modeline row and the echo area
  ([ADR 0057](adr/0057-frame-layout.md)).
- **Selections** are per window and support multiple cursors
  (`oso_core::selection::SelectionSet`). Each selection has a cursor, a
  goal column for vertical motion and its own mark. The set says whether the
  regions show, for all of them, and keeps the primary's mark ring. See
  [ADR 0007](adr/0007-multi-cursor.md) and [ADR 0071](adr/0071-mark-and-region.md).
  Every window's cursors follow an edit of the buffer it shows, and are clipped
  to display clusters. In the selected window a cursor at an insertion ends after
  it; in others it stays before, as an Emacs marker. Marks follow as markers in
  every window, and an edit hides the regions.
- **Edits** go through `Editor::apply_edits`, but for an internal buffer's, which have no undo
  ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)), and a recovered emergency copy's whole text
  (`Editor::replace_text_from`, [ADR 0064](adr/0064-recovering-an-emergency-copy.md)): one replacement per cursor, for
  one buffer, applied in descending offset order. Overlapping ranges become
  their union. The rope, display map, undo record, revision and every window's
  cursors change together: the work is done on clones, O(1) to make, and
  committed by moves that cannot panic, so a panic caught at the command boundary
  leaves nothing half applied. Read-only buffers refuse edits. See
  [ADR 0053](adr/0053-edit-core.md).
- **Motion and scrolling** are pure functions in `oso_core::motion`,
  `oso_core::word` and `oso_core::window`. Motion steps over display clusters, and what a
  command reads is limited by a `WalkBudget`
  ([ADR 0058](adr/0058-word-motion-prefix-argument-and-secret-positions.md)). After every
  command the dispatcher scrolls the window by the fewest rows and columns
  that show the cursor (`scroll-conservatively` 101). `C-v`, `M-v` and the
  mouse wheel set the window's top row with `Window::set_top`, the one
  place that moves cursors left behind into view. `Editor::restore_positions`
  puts back cursors and a view saved together and then follows the cursor. A
  selected window that gets taller also pulls its top row back by at most the rows
  it gained, to fill empty rows below the last one, so one that was short at the
  end of a buffer shows its text again. `Editor::scroll_view_sideways`, for the wheel,
  is the one public method that moves the view and leaves the cursor, even out of
  view, until the next command.
- **Undo** is linear (`oso_core::undo`, [ADR 0030](adr/0030-m2-editing-commands.md)):
  an edit after an undo discards the redo groups. A group holds one command's
  edits across all cursors, and the selected window's cursors before and after.
  The dispatcher opens it before a command and closes it after, and on the panic
  path. Consecutive self-inserts join one group, up to 20. Removed text is kept as
  rope slices, so killing a huge region copies nothing. History is bounded in
  bytes, 64 MiB per buffer, not counting the group last recorded or crossed, which
  always stays. It is not persisted to disk.
- **The kill ring** (`oso_core::kill`, [ADR 0074](adr/0074-the-kill-ring-and-the-clipboard.md))
  is owned by `Editor`. A kill takes one piece per cursor as rope slices, merging ranges
  that overlap, so it copies nothing on the UI thread, and appends to the newest entry
  when the command before also killed. It keeps 120 entries and 64 MiB, and a yank
  inserts at most 16 MiB. An entry killed from a buffer that may hold secrets is secret,
  and yanking it makes the target buffer secret (`Verdict::Yanked`).
- **isearch** (`Editor::isearch`, [ADR 0082](adr/0082-isearch.md)) is a mode of the selected
  window, with its own keymap in place of the others. The matcher (`oso_core::search`, private to
  the crate) runs Knuth–Morris–Pratt over the rope's chunks. Each key searches 256 KiB on the UI
  thread, and one worker thread of the host's (`app/searches.rs`) goes on with a far match and counts
  the matches over an O(1) snapshot, superseded by the next job through a generation. A frame
  searches only the rows in view for the highlights. A search ends when its window shows another
  buffer or a prompt opens.
- **The clipboard** (`oso_platform::clipboard`) runs on two threads of its own: one owns
  the system's clipboard, and one answers each read within 500 ms. The host hands it what
  a command killed, joined there with line breaks, and `yank` waits for its read through
  the dispatcher, which holds the keys that come meanwhile. oso reads it only on a yank.
  macOS uses `NSPasteboard`; Linux and Windows have none until their window layers.
- **Modified state** compares content ids. Each edit gets a new id, undo and redo
  restore the stored one, and a buffer is modified when its id differs from the
  saved one. So undoing to the saved state is unmodified, and a buffer whose saved
  state left the history stays modified until it is saved.
- **Messages** (`Editor::message`) are written to the log, so they must not quote buffer
  text. The echo area shows the newest until the next key, and `*Messages*`, an internal
  buffer, keeps the newest 1000. The dispatcher marks where a command begins and ends, and
  inside that span a message around a secret buffer reaches only the echo area. `Editor::echo` reaches the
  echo area alone. `Editor::buffers` leaves internal buffers out, so whatever goes through
  a user's work does too ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)).

### Command registry

Every command is registered with a name, a docstring, an argument spec, a reach and a
function. The reach says whether the agent channel may run it
([ADR 0085](adr/0085-agent-channel-and-oso-mcp.md)). The registry drives `M-x` completion,
`describe-function`, `describe-key`, the control socket's `run-command`, the agent channel and
the generated command reference. Commands never keep global state: the host owns all state and
passes it in. This keeps commands testable, restartable and hot-patchable.
See [ADR 0034](adr/0034-command-registry-and-keymaps.md).

- `oso_core::command`: `Command`, `Registry`, and `Ctx`, which lends a
  command the host's `Editor` and collects its requests to the host
  (redraw, exit). A request that carries data waits on the `Editor`, as a save
  does. `Args` carries the numeric prefix argument, which the dispatcher holds between
  keys ([ADR 0058](adr/0058-word-motion-prefix-argument-and-secret-positions.md)).
- Commands live in the tip crate, one module per feature under
  `crates/oso/src/commands/`, each with a `const` table. `TABLES` in `mod.rs`
  lists the tables. Registering a name twice is an error, and the tests build
  the full registry.
- A command asks for a redraw only when it changed something visible.

A panicking command is caught at the command boundary
(`Registry::execute`). The editor reports the error and keeps running
([ADR 0014](adr/0014-panic-strategy.md)).

### Keymaps

- `oso_core::keymap::Keymap` maps a key press to a command or to a nested
  prefix keymap. Bindings refer to commands by id, resolved from names
  once, when a keymap is built from a table in Emacs notation
  ([ADR 0033](adr/0033-key-presses-and-notation.md)). Default bindings are
  in `crates/oso/src/bindings/`, with a table for each module of commands.
- A keymap may have a text command, which runs for a key that types a character
  (no Control, Meta or Super) and that it does not bind. The command reads the
  character from `ctx.input()`. Other text goes to the same command through
  `Dispatcher::insert_text`, and an input method's commit through
  `Dispatcher::commit_text`, which keeps it an undo step of its own
  ([ADR 0051](adr/0051-m2-plan-decisions.md), [ADR 0056](adr/0056-typing.md),
  [ADR 0069](adr/0069-macos-input-method.md)).
- The input method is the window layer's, reported through `app/shell.rs` on winit or
  `app/native.rs` on oso's own layer, and handled in `app/ime.rs`
  ([ADR 0070](adr/0070-windowing-and-input-without-winit.md)). oso keeps its own state of
  it, and turns it off where the next key is read as a key or cannot type: a pending
  prefix or prefix argument, a read-only buffer, one that may hold secrets, the list of
  emergency copies and a prompt. Turning off during a composition commits it first, where
  typing goes then, on oso's own layer if it would go in as typed there
  (`Dispatcher::types_text`), and otherwise waits for it to end, as always on winit, which
  cannot commit one ([ADR 0069](adr/0069-macos-input-method.md),
  [ADR 0077](adr/0077-choices-for-the-native-layers-stage-2.md)). The app gives the window
  the cursor's cell when it moves and, during a composition, where it is drawn: its first
  cell, and where each of its characters is (`Editor::preedit_layout`). On oso's own layer
  the view only keeps them, and answers the input method's question about a range with that
  place, so the candidate window follows the segment being converted. While the IME is off
  it gives no input context, so no text service reads it, and Writing Tools are off for it.
- `Layers` holds the active keymaps, highest priority first: keymap at
  point, minor modes, major mode, global. The first keymap that binds the
  whole key sequence decides. So far the global layer exists, a minor mode layer of
  digits while a prefix argument is typed, and a major mode layer in the buffers that show
  emergency copies and in a prompt. A question's keys take the global layer's place, so
  that no other key reaches a command.
- `KeyResolver` keeps the pending prefix between key presses: `C-x` waits,
  `C-x C-c` runs, an unbound sequence is reported and clears the prefix,
  and `C-g` cancels a pending prefix.
- The tip crate's `Dispatcher` wires the resolver to the registry. Keys from
  the window and from the control socket's `send-keys` both go through it.

### Display model

The display model maps buffer positions to screen rows and back, in O(log n),
through a stack of transforms. It supports folding, virtual text, soft wrap
(off by default), block decorations and variable row heights. Today only
the TabMap exists (`oso_core::display_map`): it expands tabs, measures
grapheme clusters on the cell grid and shows invalid bytes as `\xNN`, and
rows are buffer lines. See
[`display-model.md`](display-model.md),
[ADR 0006](adr/0006-display-model.md) and
[ADR 0035](adr/0035-sumtree-rope-and-cell-grid.md). The M2 screen layout is in
[`ui.md`](ui.md).

### Text rendering

Text shaping and rasterization use OS-native APIs behind the `TextSystem`
trait in `oso-platform`: CoreText on macOS, DirectWrite on Windows later. The
Linux backend is still open. Until a backend exists, Linux and Windows use a
null text system: the cursor is drawn on the grid, and no text. See
[ADR 0005](adr/0005-text-rendering.md) and
[ADR 0036](adr/0036-text-system-and-glyph-atlas.md).

- **Fonts** are loaded at the window's scale factor, so sizes are physical
  pixels; a scale change loads them again. Fallback for CJK and emoji is the
  OS's cascade list. The default font is SF Mono, then Menlo, at 13 pt,
  until configuration exists.
- **Shaping** returns glyphs with their cluster's byte offset and pen
  position. `oso-render` places them on the cell grid with the display
  model's cluster widths, so drawing and cursor motion agree: half-width
  clusters take one cell, full-width two, and a column's x is
  `column × cell_width`. Cells are whole pixels. A glyph is placed by its cluster, so
  cells stay in the logical order of the text, and no character is shown as its mirrored
  form ([ADR 0066](adr/0066-text-stays-in-logical-order.md)).
- **Rasterizing** gives coverage bitmaps for text and premultiplied RGBA for
  color emoji. They are cached in a glyph atlas (one texture per format,
  packed in shelves, doubling when full), so a glyph is rasterized once
  while its font is loaded.
- **Per frame**, only visible rows are drawn, and of each only the visible
  columns, widened to 64-column steps, are shaped. Shaping is cached by
  display row. An entry is reused without looking at the buffer while the
  revision holds. After a change, the row's shaped columns are read again
  and it is reused if the shaper's input is the same: the text and each
  cluster's column, cells and face. That is what "the bytes it covers and
  its start column" in [ADR 0051](adr/0051-m2-plan-decisions.md) comes to,
  because the rope's bytes alone cannot tell invalid bytes, tab widths or a
  cluster that starts before the first column apart. It replaces the offset
  check in [ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md) too.
  Nothing in an entry is a buffer offset, so an edit leaves the rows below
  it alone wherever they now are in the buffer, and undo, reload and skipped
  frames need no record of what changed. A row that went to another display
  row, after a line break or a deleted line, is shaped again.
  Scrolling reshapes only rows that come into view, and a 10 MB line costs
  one screen. Only glyphs inside the pane become quads. Each pane has its own
  cache, by a `PaneId` that the caller gives it
  ([ADR 0061](adr/0061-panes-clipping-and-layers.md)).
- **Clipping** is done on the CPU. A glyph's quad is cut to its cluster's cells, its
  row with the bands of [ADR 0052](adr/0052-glyphs-stay-in-their-cells.md), and its
  pane, moving the atlas position along with the corner.
- **Colours** come from the current theme, which the app hands the renderer each frame
  ([ADR 0089](adr/0089-built-in-themes-switched-at-run-time.md)). Shaped rows and rasters
  hold none, and the text shader corrects coverage by the foreground's luminance with the
  theme's gamma terms, so a switch shapes and rasterizes nothing again.
- **Text that is no buffer's,** such as a modeline row, comes as `Span`s with a `Face`
  and goes through `display_map::text_clusters`, so its control characters, tabs and
  line breaks are escapes.
- **Pieces:** a row is shaped in pieces wherever the face changes, and at each span, so
  nothing forms across them. A row of one face is one call to the shaper, which may split
  it by direction ([ADR 0066](adr/0066-text-stays-in-logical-order.md)) into at most two
  lines ([ADR 0068](adr/0068-cut-clusters-short-pieces-the-cursor-box-and-lines-by-direction.md)).
- **Escapes** for control characters, invalid bytes and invisible
  characters come from the display model and are shaped as ASCII labels
  in an escape color; the raw characters never reach the shaper
  (security.md, T3). Invisible characters also get a box, as do characters
  past a display cluster's limits
  ([ADR 0054](adr/0054-limits-on-a-display-cluster.md)) and clusters cut past the
  budget of characters, each shaped on its own
  ([ADR 0068](adr/0068-cut-clusters-short-pieces-the-cursor-box-and-lines-by-direction.md)).
  Pieces of up to 64 bytes are kept by their text, so a label or a name is shaped once.
- **New glyphs** are rasterized at most 64 per frame, and the frame asks
  for another; uploads are batched per run of neighbors on an atlas
  shelf. See [ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md).

### GPU rendering

wgpu with one backend per OS: Metal, DX12 and Vulkan. Only that backend is
compiled in, through per-target wgpu features, and there is no GL fallback.

A frame is a list of panes, each with a region of cells, a layer and what it shows
([ADR 0061](adr/0061-panes-clipping-and-layers.md)). One render pass draws the layers
from the back, each with one draw per pipeline, whatever the number of panes and
glyphs:

```
for each layer, back to front:
  rectangles: backgrounds, boxes, cursors, selections   (shape renderer)
  rounded boxes: a float's panel, field and key chips   (shape renderer, one quad each)
  glyphs from the atlas                                 (text renderer)
```

A layer with no rounded box draws none, so a frame with no float costs nothing more
([ADR 0075](adr/0075-a-float-as-a-panel-attached-to-the-top-edge.md)).

`scene` builds those ranges with no GPU, and `paint` uploads and records them, for the
window and for a screenshot alike. Panes are built front to back, so the caps on quads
and rectangles and the budget of new glyphs run out behind what is in front. A text pane
holds one rectangle back for its cursor box, so nothing of its own takes its place. A frame is
clipped to pixels on the CPU, so there is no scissor; a rounded box's quad is cut to its
pane there too. Only the background is drawn in the margins and in what is left over of a
cell at the right and bottom, apart from the band that glyphs on the frame's top row keep
above it, and a float's panel, which covers the top margin above it.

The event loop uses `ControlFlow::Wait`. Frames are drawn only on demand:
on input that changes the view, on resize, when the window becomes visible, and when
what the modeline or the echo area says has changed, which the app compares after each
batch of events ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)).
The surface presents with `Fifo` and a frame latency of 1. Fifo is always
supported and never tears, and on-demand drawing means it costs nothing while
idle. Mailbox or Immediate can be revisited once key-to-photon latency is
measured ([ADR 0025](adr/0025-latency-measurement.md)). The adapter is
requested with low power preference, which keeps dual-GPU machines on the
integrated GPU.

Surface errors are mapped to typed recovery actions: an outdated surface is
reconfigured, a lost one is recreated, a timeout retries the frame, and an
occluded window skips it until it is visible again: the renderer says the frame is owed, and
oso's own macOS window layer looks at the window again every 150 ms until it shows, so the
frame does not wait for a notice that may never come
([ADR 0070](adr/0070-windowing-and-input-without-winit.md)). At most three failed frames
in a row are retried, so a surface that does not recover cannot spin the event
loop. Render errors are logged and are never fatal.

### Views and buffers

| Buffer kind | Backing | Examples |
|---|---|---|
| File | rope loaded from disk | source files, notes |
| Generated | rope produced by a mode, read-only by default | git status, directory listings, compile output, help |
| Internal | rope the editor keeps for itself, not the user's work | `*Messages*`, `*Emergency Copies*` and a copy being looked at |
| Terminal | `alacritty_terminal` grid | shell sessions |

A window shows a view. A text view renders a buffer through the display model.
A terminal view renders a cell grid and exposes its content as searchable
text, so isearch, line search and copy work as in text buffers. See
[ADR 0015](adr/0015-terminal.md).

The minibuffer, menus (hydra/transient-style), completion popups and hover
documentation are buffers shown in floating windows. Editing commands work
inside them unchanged. See [ADR 0008](adr/0008-floating-ui.md). A command never
waits for an answer: the dispatcher reads what its `ArgSpec` asks for in a prompt
before running it, and a command that has to ask leaves a question whose `y` runs a
command of its own, which checks again. `C-g` does one thing at a time, in
`Editor::quit` ([ADR 0072](adr/0072-floating-windows-the-minibuffer-and-questions.md)). A prompt that
completes keeps its candidates in the editor (`oso_core::complete`): orderless matching in an
order fixed as it opens, matched once each command ends, on the UI thread up to 5000 candidates,
in a float at the center of the frame.
`M-x` reads a command's name through `ArgSpec::Command`, and the dispatcher runs that command
with `M-x`'s prefix argument ([ADR 0076](adr/0076-completion-and-m-x.md)).

### Background work

The UI thread never blocks on I/O or parsing:

- Files load on a background thread (`crates/oso/src/loader.rs`). It reads
  64 KB at a time and builds the rope, the display map and the
  invalid-byte table there (`oso_core::load::Builder`), so the UI does no
  O(n) work. O(1) snapshots go to a one-entry slot: the first after the
  first read, then at most one per 50 ms, then the complete one. The thread
  wakes the UI with a user event, and the UI takes the latest snapshot
  when woken and before each frame. The buffer stays read-only until
  loading finishes. Invalid UTF-8 and binary files stay read-only, with
  `\xNN` for invalid bytes
  ([ADR 0022](adr/0022-text-buffer-and-file-loading.md),
  [ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)).
- Open files are looked at once a second, and when the window gains the focus, by a
  thread that only `stat`s them (`loader::Watcher`). A file that changed under a buffer
  with no unsaved edits is read again on a loading thread, which compares the new text
  with the old one there. The UI puts it in as one undo step and maps every window's
  cursors through the one changed span. A buffer with edits keeps them and warns. A reload
  waits while a prompt is open (`Editor::reload_waits`)
  ([ADR 0029](adr/0029-file-changes-on-disk.md),
  [ADR 0083](adr/0083-reloading-files-changed-on-disk.md)).
- Directories for `C-x C-f` are listed on worker threads (`crates/oso/src/lister.rs`),
  one per directory and at most 4, and their names come back as a snapshot with the
  input's generation, which the prompt takes only if it is still the newest
  ([ADR 0087](adr/0087-finding-files-writing-under-a-new-name-and-large-files.md)).
- Files save on a background thread (`crates/oso/src/saver.rs`), from an O(1)
  snapshot of the rope. The thread frees what only the snapshot held, then sends
  its report ([ADR 0059](adr/0059-safe-save-and-the-exit-guard.md)).
- The event loop's user event is an enum: loader wake-ups, save reports, the watcher's
  reports and what threads find of emergency copies in every build, and control socket requests in dev builds.
- Tree-sitter parsing runs on a background thread with cancellation and
  timeouts. Results replace the syntax decoration layer atomically.
- Search (ripgrep-style), git status and language servers stream results in
  asynchronously.

### Configuration

User configuration is TOML: key bindings, menus, theme, fonts and per-mode
settings. It reloads on file change. Inside repositories, only EditorConfig is
read. See [ADR 0019](adr/0019-configuration.md).

### Persistence

There is no periodic auto-save and there are no backup files. If oso panics
outside a command or, on macOS, the OS ends it, unsaved buffers are written to a private
directory (mode `0700`, files `0600`, and on macOS access lists count, through
`oso_platform::file_meta`), skipping secret-looking files. See
[ADR 0013](adr/0013-persistence.md), [ADR 0051](adr/0051-m2-plan-decisions.md),
[ADR 0062](adr/0062-emergency-save-and-the-panic-hook.md) and
[ADR 0065](adr/0065-access-lists-on-private-directories.md).

The panic hook cannot read the editor, so the UI thread publishes to it (`emergency::Publisher`): after each
command, a save's report and a load's event, an O(1) rope snapshot of each buffer that is modified or
being saved, for only what changed. What a panicking command left is held back until a command ends normally, but for a save it started. The hook owns an `Arc` of that, and nothing is global. It tells where the panic
happened: outside a command on the UI thread it writes the copies before unwinding, on threads of its
own so that a panic in the writer costs one buffer and a blocked stderr holds nothing up. `exiting` writes the same
way unless the app agreed to the end (`saver::Agreement`). A copy is `<stem>.txt`, the exact text, and `<stem>.meta`, which says where it
belongs and is renamed in last. Recovery only ever offers one ([ADR 0064](adr/0064-recovering-an-emergency-copy.md)): threads read the directory, `emergency::Offer` holds what they found and what comes of an answer, commands only queue what a person asked, and a path in a `.meta` is never used to touch a file.

Saving never touches the file until the whole new text is safely beside it. A
command checks the buffer, ensures its final newline as an edit, and queues a job
with an O(1) snapshot of the rope (`Editor::start_save`). The host takes the job
after the command and runs it on a thread (`saver`): a temporary file in the file's
own directory, the file's owner, access list and mode, fsync, a check that the file on disk is
where and what the buffer last read or wrote (its real location, modification time and
size, and its device and inode where the system has them, the baseline on `Buffer`), rename, and fsync of the directory. A file changed by
anything else is kept, and the buffer stays modified. A symbolic link stays a link, and hard links
break. `C-x C-c` and the window's close button ask before exiting while a buffer is modified
or a save runs, and on oso's own macOS layer so does a person's request to quit, from the Dock
or another app, while the end of the session is never refused
([ADR 0084](adr/0084-a-persons-quit-asks-as-c-x-c-c-does.md)), and a save still running at exit gets to finish. A save that found its file
changed asks before going over it, and replaces it only if it is still what that save found. See
[ADR 0059](adr/0059-safe-save-and-the-exit-guard.md) and
[ADR 0072](adr/0072-floating-windows-the-minibuffer-and-questions.md).

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
  - It polls the sources, and spawns cargo and the new binary through
    `oso-platform::process`.
  - The state holds paths and positions, and the new instance reads every file
    without edits from disk ([ADR 0047](adr/0047-process-spawning-and-restart-handoff.md)).
    The text of modified buffers, the marks, the kill ring and the histories are
    carried as items the state names by length: `export-text` gives them in pieces
    from an O(1) snapshot, the supervisor relays one piece at a time, and
    `import-text` builds them off the UI thread. Undo history is not carried
    ([ADR 0051](adr/0051-m2-plan-decisions.md),
    [ADR 0090](adr/0090-oso-dev-carries-unsaved-text.md)).
  - A modified buffer that may hold secrets holds the restart, as a running save
    does, and the supervisor asks before it starts anything. Why a restart waits, and
    a failed build's errors, reach the echo area and `*Messages*` through
    `dev-message`. An instance goes only when it says so itself: the supervisor
    never kills, the `exit` it sends carries the generation of the export, and an
    instance whose text has changed since refuses. If the old one refuses, the new
    one goes unless it has been edited too, and then both stay. Instances run in
    process groups of their own, so Ctrl-C ends the supervisor and leaves their
    windows ([ADR 0055](adr/0055-oso-dev-never-loses-unsaved-text.md)).
- **Control socket:** agents can send keys, run commands, read state and take
  screenshots ([ADR 0016](adr/0016-dev-control-socket.md)). The protocol is
  JSON Lines, and `oso ctl` is the client
  ([ADR 0023](adr/0023-control-socket-protocol-and-restart.md)).
  - The transport lives in `oso-platform::control`: a Unix domain socket in a
    private `0700` directory, with a peer user check. Windows is not
    supported yet ([ADR 0032](adr/0032-control-socket-transport.md)).
  - Background threads read and parse requests. They reach the UI thread as
    user events, through the app's `Waker`, and replies return over a channel. Keys from the
    socket take the same path as keys from the window.
  - `screenshot` renders the frame again offscreen (`oso-render`, `dev`
    feature). Readback, PNG encoding and the file write run on a background
    thread.
  - `run-command` runs a command through the same registry as keys.
    `get-state` copies only what the selected window shows, and `get-text`
    works from an O(1) snapshot on a background thread, checking its limit
    before copying, so the UI thread's work does not depend on the buffer's
    size.
  - Buffers that may hold secrets give no text and no pixels. The loading
    thread follows the path, and `Buffer::is_secret` holds the verdict, so
    the UI thread never touches the file system for this
    ([ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md)). While one is shown,
    `get-state` leaves out what an edit or a save there changes and the keys and prompts around
    it, and no restart is handed over
    ([ADR 0079](adr/0079-state-and-restarts-around-a-secret-buffer.md)).
  - `take-timings` reports key latency, startup and load times to benchmarks,
    and `open-file` visits a file with the window up, so that opening can be
    timed apart from startup
    ([ADR 0046](adr/0046-performance-regression-checks.md)). Keys and frames around a
    buffer that may hold secrets stay out of them
    ([ADR 0060](adr/0060-timings-while-a-secret-buffer-is-shown.md)), and so do its loads. While
    one is shown, the log keeps warnings and errors only
    ([ADR 0078](adr/0078-load-times-and-log-lines-around-a-secret-buffer.md)).
- **Agent channel:** the one control entry point in release builds, off until the person
  runs `agent-channel-start`, for agents such as Claude Code through `oso mcp`
  ([ADR 0085](adr/0085-agent-channel-and-oso-mcp.md)).
  - `crates/oso/src/channel/` serves `<pid>-agent.sock` in the same private directory through
    `oso-platform::control`, which is built in every build. Its methods are `version`, `state`,
    `commands`, `run`, `cancel`, `open` and `text`, over the JSON Lines framing in
    `channel::wire`, which the dev socket shares, and no dev method. `oso mcp` comes next
    (AC-2).
  - `open` visits a file that exists, unshown, through `app/agent/open.rs`, and shows it at
    its line once the loader has judged it ordinary and read it; one judged otherwise is
    dropped. `text` copies rows or the region off the UI thread (`channel::text`). A buffer
    the channel opened is marked, and `save-buffer` run for the agent asks the person before
    saving it. `ArgSpec::Text` and `insert` take text; `dispatch::args` maps a given argument
    for the dev socket and the channel alike. A count, which repeats a command's work, is at
    most 10,000 either way, as ADR 0044 has it; a number such as a line is a value, and goes
    as given. A reload of a file the channel opened checks it as `open` does, and a reload
    that finds the file elsewhere makes the agent's save of it ask. An `oso dev` restart keeps
    the mark: the buffer's handoff entry says `"opened_by_agent": true`, only when it is.
  - Each `Command` has a `reach` (`Look`, `Change`, `Person`), and the channel refuses `Person`
    commands. `Dispatcher::run_for_agent` checks the argument against the spec and runs the
    command inside `Editor::for_agent`, so the prompts it opens are the agent's.
  - Each connection waits for the person's yes, asked on its first request: one that sends
    nothing within 2 s is closed unasked, as probes do. Threads read and wait, and
    `app/agent.rs` asks and answers on the UI thread. The channel runs nothing while a buffer
    that may hold secrets is shown, while the person is in a prompt, a key sequence or a
    composition or typed in the last second, or while its own question is open. A question it
    causes is answered from the window. Its kills stay in oso's kill ring.
  - The connection question names the process as it reports itself, and its parent, from
    `ControlStream::peer`.
- **Dev profile:** dependencies are optimized and oso is not, to keep the
  tip-crate rebuild within 2 s ([ADR 0024](adr/0024-dev-build-profile.md)).

Hot reload and the control socket are behind cargo features (`dev` in each
crate, enabled through `oso/dev`) that release builds never enable. The agent channel is not:
it ships, off by default. CI checks
this with `just release-safety`
([ADR 0032](adr/0032-control-socket-transport.md)).
