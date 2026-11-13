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
├── oso/            bin, "tip" crate: commands, default keymaps, config,
│                   app glue
├── oso-core/       buffers, selections, display model, command registry,
│                   keymaps; no GPU, #![forbid(unsafe_code)]
├── oso-render/     wgpu pipeline, glyph atlas
├── oso-syntax/     tree-sitter, its C grammars and oso's queries; only the
│                   tip crate uses it
├── oso-term/       the terminal: libghostty-vt behind an engine trait, the
│                   escape-sequence policy, a thread per terminal and its
│                   snapshots; the key encoder and search from steps 4 and 5;
│                   only the tip crate uses it (ADR 0120)
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
depends on `oso-core` and `oso-platform`, `oso-syntax` on `oso-core`, and neither of
those two depends on another workspace crate. Only `oso-render` uses wgpu, only
`oso-platform` uses OS bindings, and only `oso-syntax` uses tree-sitter
([ADR 0111](adr/0111-tree-sitter-highlighting.md)). From
[ADR 0120](adr/0120-the-terminal.md)'s step 3, `oso-term` depends on `oso-core` and
`oso-platform`, and only the tip crate uses it. It alone links libghostty-vt, which Zig 0.17
builds from the release `crates/oso-term/ghostty-vt.lock` pins: run `just ghostty-vt` once to
fetch and check its tarballs, with Zig 0.17 and minisign installed. All of its `unsafe` is in
`engine/ghostty`'s `ffi` and `raw`. These rules, and the allowed layers of
modules inside each crate, are listed in
[`guardrails/layers.txt`](../../guardrails/layers.txt) and enforced by
`crates/oso/tests/architecture`
([ADR 0038](adr/0038-architecture-guardrails.md),
[ADR 0039](adr/0039-architecture-guardrail-mechanics.md)).

Only the tip crate uses winit, and in it only `app/shell.rs`: the rest of the app
works on oso's own events, keys and window calls, `oso_platform::window`'s, so that a
window layer of oso's own can replace winit there
([ADR 0070](adr/0070-windowing-and-input-without-winit.md)). On macOS that layer is the
window system since stage 3: `oso_platform::window` runs `NSApplication`'s loop with a view
of its own, and `app/native.rs` gives the app the same loop, window and waker over it.
winit is built only for Windows and Linux. The
tip crate owns the window and hands `oso-render` a `wgpu::DisplayAndWindowHandle` and the
size in pixels; the renderer reports back when it needs a redraw instead of calling into
the window.

## Layers

```
OS events (oso_platform::window through app/native.rs on macOS,
           winit through app/shell.rs elsewhere)
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
  scroll position. While its buffer wraps (M2-25) it also holds its wrap of
  the buffer, which it drops when it shows another, and its top is an offset
  ([ADR 0100](adr/0100-soft-wrap.md), [ADR 0103](adr/0103-soft-wrap-in-a-window.md)).
  The app runs the wraps' jobs on one thread, as it runs isearch's. A frame holds a tree of
  windows, split side by side or stacked, each with its own selection set, scroll, wrap and places
  over a shared buffer ([ADR 0112](adr/0112-window-splits.md)). See
  [ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md). A pure function,
  `oso_core::frame::layout`, divides the frame into whole cells: each
  window's text area, with a gutter for line numbers at its left, above its modeline row, a
  divider column between side by side windows, and the echo area ([ADR 0057](adr/0057-frame-layout.md), [ADR 0097](adr/0097-visual-aids.md), [ADR 0112](adr/0112-window-splits.md)).
- **Selections** are per window and support multiple cursors
  (`oso_core::selection::SelectionSet`). Each selection has a cursor, a
  goal column for vertical motion and its own mark. The set says whether the
  regions show, for all of them, and keeps the primary's mark ring. See
  [ADR 0007](adr/0007-multi-cursor.md) and [ADR 0071](adr/0071-mark-and-region.md).
  Every window's cursors follow an edit of the buffer it shows, and those in the
  chunks the edit rebuilt are clipped to display clusters (`display-model.md`). In the selected window a cursor at an insertion ends after
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
  command reads is limited by a `WalkBudget`, which several cursors share
  ([ADR 0058](adr/0058-word-motion-prefix-argument-and-secret-positions.md),
  [ADR 0101](adr/0101-word-checks-by-offset-and-reading-shared-by-cursors.md)). After every
  command the dispatcher scrolls the window by the fewest rows and columns
  that show the cursor (`scroll-conservatively` 101). `C-v`, `M-v` and the
  mouse wheel set the window's top row with `Window::set_top`, the one
  place that moves cursors left behind into view. `Editor::restore_positions`
  puts back cursors and a view saved together and then follows the cursor.
  The top is an offset, which follows edits as a marker; scrolling counts view
  rows, which are logical lines unless the buffer wraps, while motion stays on
  logical lines ([ADR 0100](adr/0100-soft-wrap.md),
  [ADR 0103](adr/0103-soft-wrap-in-a-window.md)). A
  selected window that gets taller also pulls its top row back by at most the rows
  it gained, to fill empty rows below the last one, so one that was short at the
  end of a buffer shows its text again. `Editor::scroll_view_sideways`, for the wheel,
  moves the view and leaves the cursor, even out of view, until the next command in
  that window or one that moves it; `Editor::scroll_view` leaves it too beneath a
  prompt or a composition (ADR 0069, ADR 0072). The wheel scrolls the window under the pointer
  without selecting it: the app turns the pointer's pixel into a cell with
  `PixelGrid::cell_at`, and `Editor::wheel_window` into a window
  ([ADR 0112](adr/0112-window-splits.md)).
- **Undo** is linear (`oso_core::undo`, [ADR 0030](adr/0030-m2-editing-commands.md)):
  an edit after an undo discards the redo groups. A group holds one command's
  edits across all cursors, and the selected window's cursors before and after.
  The dispatcher opens it before a command and closes it after, and on the panic
  path. Consecutive self-inserts join one group, up to 20. Removed text is kept as
  rope slices, so killing a huge region copies nothing. History is bounded in
  bytes, 64 MiB per buffer, not counting the group last recorded or crossed, which
  always stays. It is not persisted to disk. A batch of more than 4096 splices is
  kept in parts. An undo or a redo crosses a slice a turn on clones and commits each
  step once ([ADR 0119](adr/0119-undoing-large-steps-in-slices.md)): the command runs
  the first slice, `app/slices.rs` the rest, back to back and drawn as the share moves
  on, keys and the wheel wait as for `!` and `C-g` cancels it, and what each commit lets
  go is freed off the UI thread.
- **The kill ring** (`oso_core::kill`, [ADR 0074](adr/0074-the-kill-ring-and-the-clipboard.md))
  is owned by `Editor`. A kill takes one piece per cursor as rope slices, merging ranges
  that overlap, so it copies nothing on the UI thread, and appends to the newest entry
  when the command before also killed. It keeps 120 entries and 64 MiB, and a yank
  inserts at most 16 MiB. An entry killed from a buffer that may hold secrets is secret,
  and yanking it makes the target buffer secret (`Verdict::Yanked`).
- **isearch** (`Editor::isearch`, [ADR 0082](adr/0082-isearch.md)) is a mode of the selected
  window, with its own keymap in place of the others. The matcher (`oso_core::search`, private to
  the crate) runs Knuth–Morris–Pratt over the rope's chunks, or a regexp one line at a time
  (`search/regexp`, [ADR 0116](adr/0116-migemo-and-jumps.md)). A romaji string searches for
  `oso_core::migemo`'s pattern of its kana, a regexp the matcher compiles; `migemo` is pure, on
  the crate's lowest layer, and the editor keeps the last 32 compiled (`editor/migemo.rs`, which
  jumps share). Its `Dictionary` is built from a file's text, one sorted table over the text
  itself, and gives the words of the readings a string's kana start, at most 4096 a pattern,
  which a prompt's components share;
  the editor reads no file, but says when a search wanted a dictionary, and the host's
  `oso-migemo` thread (`app/migemo.rs`) reads the one `[search] migemo-dictionary` names, checked
  as `config.toml` is, and hands the table over, kept for the session. A project's search and
  completion's `%` compile their components' patterns with it themselves
  (`migemo::compile_with_budget`), each keeping a component the input before had. Each key
  searches 256 KiB, and a regexp's 3 ms counted from the key's start, a
  compile on the key included, on the UI thread, and one worker thread of the host's
  (`app/searches.rs`) goes on with a far match, a long line and a regexp too large to compile on
  a key, and counts the matches over an O(1) snapshot, superseded by the next job through a
  generation. A frame searches only the rows in view for the highlights. A search ends when its window shows another
  buffer or a prompt opens.
- **Jumps by label** (`Editor::jump`, `oso_core::jump`,
  [ADR 0116](adr/0116-migemo-and-jumps.md)) are a mode of the selected window too, with their own
  keymap. A key finds the candidates in every tiled window's rows in view, within 256 KiB a window
  and 1 MiB a jump, its matches within a key's 3 ms, nearest the window's primary cursor first, and labels them with avy's tree; a
  frame draws only those found. The tip crate runs the timer that shows the labels
  (`app/jumps.rs`). An action's key at the labels acts on the target instead
  (`editor/jump/actions.rs`), through the kill ring's own kill and yank, so its kills reach the
  clipboard as any kill does.
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

- `oso_core::keymap::Keymap` maps a key press to a command, a menu of `oso_core::menu`
  ([ADR 0117](adr/0117-menus.md)) or a nested prefix keymap. Bindings refer to commands by id, resolved from names
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
  emergency copies and a prompt. The prompt for a file's name has the window choose an ASCII
  input source while it is open (`Window::set_ascii_input`), and allows the IME once the person
  chooses a Japanese input mode there, which oso's macOS layer hears from the system as
  `Event::InputSource`, never while a buffer that may hold secrets is shown
  ([ADR 0106](adr/0106-the-input-source-turns-japanese-on-in-a-file-name-prompt.md)). Turning
  off during a composition commits it first, where
  typing goes then, on oso's own layer if it would go in as typed there
  (`Dispatcher::types_text`), and otherwise waits for it to end, as always on winit, which
  cannot commit one ([ADR 0069](adr/0069-macos-input-method.md),
  [ADR 0077](adr/0077-choices-for-the-native-layers-stage-2.md)). The app gives the window
  the cursor's cell when it moves and, during a composition, where it is drawn: its first
  cell, and where each of its characters is (`Editor::preedit_layout`). On oso's own layer
  the view only keeps them, and answers the input method's question about a range with that
  place, so the candidate window follows the segment being converted. While the IME is off
  it gives no input context, so no text service reads it, and Writing Tools are off for it.
  While the IME is on, the app also gives it an excerpt of the cursor's line, at most 2000
  bytes each side of the cursor (`Editor::cursor_excerpt`), from a buffer whose verdict is
  `Ordinary` and not internal, and none in a prompt; again when the buffer, its text or
  verdict, or the cursor changed (`Editor::excerpt_stamp`). The view answers the input
  method's questions about text and ranges in that excerpt's coordinates, with the
  composition at the cursor, and drops the excerpt when the IME goes off.
- `Layers` holds the active keymaps, highest priority first: keymap at
  point, minor modes, major mode, the user's, global. The first keymap that binds the
  whole key sequence, or leaves it undefined, decides. So far the global layer exists, a
  minor mode layer of digits while a prefix argument is typed, another while the layer of
  `C-x TAB` is open, whose arrows shift the region until another key closes it
  ([ADR 0115](adr/0115-indentation.md)), a major mode layer in the buffers that show emergency
  copies, in a prompt and in a Markdown buffer (`M-RET`), and a layer for the user's keys, which
  the configuration file will fill ([ADR 0108](adr/0108-the-user-configuration-file.md)). A question's keys
  take the global layer's place, so that no other key reaches a command. While a jump is on, its
  keymap comes before every other but a question's ([ADR 0116](adr/0116-migemo-and-jumps.md)).
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
rows are buffer lines. Soft wrap (M2-25) keeps a `DisplayPoint`'s row a
logical line and adds each window's WrapMap, whose view rows are what the
window shows ([ADR 0100](adr/0100-soft-wrap.md)). See
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
  columns, widened to 64-column steps, are shaped. The rows in view are a
  `window::Screen`, built once a frame, which the renderer, the frame's cells,
  the visual aids, isearch's highlights, the input method's rectangle, the dev
  socket and the text-grid snapshots all read: each a part of a logical line, so
  that soft wrap's view rows and unwrapped lines are drawn alike
  ([ADR 0104](adr/0104-soft-wrap-drawn-in-view-rows.md)). Shaping is cached by
  a row's logical line and the column of it where the row starts. An entry is
  reused without looking at the buffer while the revision holds, its columns
  and its wrap mark are as they were. After a change, the row's shaped columns are read again
  and it is reused if the shaper's input is the same: the text and each
  cluster's column, cells and face. That is what "the bytes it covers and
  its start column" in [ADR 0051](adr/0051-m2-plan-decisions.md) comes to,
  because the rope's bytes alone cannot tell invalid bytes, tab widths or a
  cluster that starts before the first column apart. It replaces the offset
  check in [ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md) too.
  Nothing in an entry is a buffer offset, so an edit leaves the rows below
  it alone wherever they now are in the buffer, and undo, reload and skipped
  frames need no record of what changed. Rows wrapped above the view change no
  row's key. A row's clusters count their columns from its first one, and a row
  with no entry takes the one its place on the screen had for the same line, so
  the rows after a character typed in a wrapped line, which move along it, shape
  nothing again. A row that went to another line, after a line break or a deleted
  line, is shaped again. A row that continues ends with the wrap mark `↩`, a
  piece of its own in `Face::WrapMark` ([ADR 0100](adr/0100-soft-wrap.md)).
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
The surface presents with `Fifo` and a frame latency of 2, which gives it three
drawables ([ADR 0124](adr/0124-three-drawables.md)). Fifo is always
supported and never tears, and on-demand drawing means it costs nothing while
idle. The third drawable lets a key that draws twice, its own frame and then a
worker's result, leave the next key's frame a free drawable, where with two that
frame waited for the compositor on the UI thread. The adapter is
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
| Terminal | `oso-term` cell grid, owned by the terminal's thread | shells, project terminals, the floating terminal, compile output |

A window shows a view. A text view renders a buffer through the display model.
A terminal view renders a cell grid that the terminal's thread publishes as
snapshots, at most one in flight, so the UI thread never parses, searches or
touches the PTY. `C-s` searches the screen and scrollback on that thread, and
copy mode (`C-c C-j`) freezes them into a read-only buffer for every other
text command. The PTY is `oso-platform::process::pty`, spawned through the
process module's rules ([ADR 0015](adr/0015-terminal.md),
[ADR 0120](adr/0120-the-terminal.md)).

The minibuffer, completion popups and hover documentation are buffers shown in
floating windows, and editing commands work inside them unchanged. See
[ADR 0008](adr/0008-floating-ui.md). A menu (hydra/transient-style) is a float with no window or
buffer, whose heads act on the window beneath ([ADR 0117](adr/0117-menus.md)). The open menus are
the dispatcher's, a stack over the set the first came from, and their keys come first while one
is open; the app draws the one on top over the tiled panes. A command never
waits for an answer: the dispatcher reads what its `ArgSpec` asks for in a prompt
before running it, and a command that has to ask leaves a question whose `y` runs a
command of its own, which checks again. `C-g` does one thing at a time, in
`Editor::quit` ([ADR 0072](adr/0072-floating-windows-the-minibuffer-and-questions.md)). A prompt that
completes keeps its candidates in the editor (`oso_core::complete`): orderless matching in an
order fixed as it opens, matched once each command ends, on the UI thread up to 5000 candidates
and on a worker past that, in a float at the center of the frame.
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
- A file's project is found on a thread of its own once the loader knows where its path leads
  (`crates/oso/src/project.rs`): the metadata of `.git`, `.jj` and `.hg` from its directory up,
  following no link and stopping at the home directory, another user's directory and a change
  of file system. A worktree's `.git` file is read, and what it names is kept by its words;
  only when those words name a trusted root is the main repository's record of the worktree
  read, by the record's spelling of that root, to check the claim. The editor keeps a table of
  projects that buffers point into (`oso_core::project`), and each request carries the trusted
  roots of its moment, so the thread never reads the record itself
  ([ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md)).
- A project's files for `C-x p f` are walked on a thread of their own, `oso-walk`
  (`crates/oso/src/walk.rs`, glue in `app/walks.rs`): a depth-first walk of its own, each
  directory opened from its parent by name and listed from the descriptor
  (`oso_platform::file_meta::Directory`, at most 32 held, at most 256 deep, `walk/tree.rs`),
  matching
  with a stack of the `ignore` crate's gitignore matchers in ripgrep's order, built from ignore
  files it reads itself (regular, no link, no wait, at most 1 MiB, none above the root). Hidden
  files are in, the markers and nested projects out, links are not followed, at most 100,000
  files and 32 MiB of paths listed, 1,000,000 entries and 16 MiB of ignore files read. A host
  timer adds what waits once a walk has been quiet for 250 ms. Paths come back in batches every 50 ms with the listing's
  generation (`UserEvent::Walked`), and a newer listing or the prompt closing moves a shared
  generation on, which stops the walk. The editor merges each batch into the list
  (`editor/walks.rs`, `complete/merge.rs`): sorted on its own and put in place by binary search,
  and only its paths matched. Up to 5000 rows that runs on the UI thread, moving the list rather
  than copying it; past that a merge job on `oso-complete` does, and batches that come while it
  runs wait for the next, so a slow job is never started over
  ([ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md)).
- A project's search for `M-s r` runs on `oso-grep` (`crates/oso/src/grep.rs`, glue in
  `app/greps.rs`): it walks with `walk::walk_files` and hands each file, with its open directory,
  over a bounded queue of 16 to `min(cores - 1, 8)` searching threads, at least one
  (`grep/files.rs`). A searcher reads 256 KiB blocks cut at a line's end (`grep/read.rs`) and
  matches them with its own copy of `oso_core::project::LineQuery`, completion's orderless
  components over each line on the `regex` crate's byte engine: the longest component that is
  not excluded is found through the block by its earliest end, and only the line of that end is
  matched whole. A file's rows go together into the search's batch, which `oso-grep` sends at
  most every 30 ms in batches of at most 1000 rows (`UserEvent::Grepped`, glue in
  `app/greps.rs`), with the request's generation, which every thread reads before each file and
  each block. The editor (`editor/project_search.rs`) puts the rows after those shown,
  never filtering them by the input, and each changed input asks for a new search; one that
  narrows a whole search's input sends that search's generation, and `oso-grep` looks through
  the lines it kept, at most 16 MiB, instead of walking
  ([ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md)).
- The selected row's preview is read on `oso-preview` (`grep/preview.rs`): the editor asks once
  per row selected (`Editor::take_preview_request`), with the row's byte offset, unless the row's
  file may hold secrets or an open buffer of it shows; the thread reads the newest request
  waiting, opening the file from the root a directory at a time, and builds an
  `oso_core::editor::Excerpt`, the lines around the row with their display model, which comes
  back as `UserEvent::Previewed`. The editor shows it as `Preview::Excerpt`, drawn as a text pane
  of its own whose revision is the excerpt's, or an open buffer of the same file by device and
  inode in its place (`editor/project_search/preview.rs`).
- Directories for `C-x C-f` are listed on worker threads (`crates/oso/src/lister.rs`),
  one per directory and at most 4, and their names come back as a snapshot with the
  input's generation, which the prompt takes only if it is still the newest
  ([ADR 0087](adr/0087-finding-files-writing-under-a-new-name-and-large-files.md)).
- A prompt's list past 5000 candidates is built and matched on a thread of its own
  (`app/completions.rs`), from a job `oso_core::complete` makes over `Arc` snapshots, superseded
  by the next through a generation. The rows shown stay and the count says `…` until the result
  comes, and the dispatcher holds `RET` and `TAB` until then (`dispatch/held.rs`); in a prompt
  for a directory it holds `RET` and `M-RET` until the listing or the look at a link that decides
  the answer has come
  ([ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md)). A look at a
  row patches it, with no new list, and a long list replaced or closed is dropped on that thread
  ([ADR 0096](adr/0096-completion-on-a-worker-past-5000-candidates.md)).
- Files save on a background thread (`crates/oso/src/saver.rs`), from an O(1)
  snapshot of the rope. The thread frees what only the snapshot held, then sends
  its report ([ADR 0059](adr/0059-safe-save-and-the-exit-guard.md)).
- The event loop's user event is an enum: loader wake-ups, save reports, the watcher's
  reports and what threads find of emergency copies in every build, and control socket requests in dev builds.
- Tree-sitter parsing runs on two worker threads of the tip crate (`app/syntax.rs`), each on an
  8 MiB stack, which share one schedule of every buffer's document (`oso_syntax::Scheduler`):
  shown buffers first, and at most one thread on a buffer no window shows, so a slice that runs
  long never holds the shown ones. The UI thread never calls tree-sitter, and nothing under the
  lock makes or frees a document; the UI sends a job after each command and drops a buffer that
  turned secret at once. A buffer's highlights are a map of runs
  in `oso-core` (`syntax::Highlights`) that follows edits with the buffer, and the worker's
  reports replace it, mapped through the edits made since. Colour is applied when glyph
  instances are built, so a highlight never reshapes a row, and a report draws a frame only
  if a window shows runs it changed. Markdown's inline text and its fenced Rust and TOML are
  parsed again, with their own grammars, wherever the worker highlights. Buffers that may
  hold secrets and files over 2 MiB are not parsed
  ([ADR 0111](adr/0111-tree-sitter-highlighting.md)). The same schedule takes documents no
  buffer owns, texts that never change such as a review's sides, opened with the verdict of
  where they came from (a text that may hold secrets is refused). They run only where buffers
  no window shows do, at most 4 hold a tree at once, and a finished one gives its tree to a
  thread to free ([ADR 0122](adr/0122-agent-review.md), step 8).
- A hosted agent session's state is read from its screen on `oso-sessions`
  (`crates/oso/src/sessions/`, glue in `app/sessions.rs`), never on the UI thread
  ([ADR 0121](adr/0121-agent-sessions-and-spaces.md)). `sessions::screen::Evidence` is the last 40
  non-empty rows, the title and the OSC 9;4 progress, cut to fixed sizes; `sessions::rules`
  compiles the bundled `claude.toml` and the local `agent-rules/claude.toml` over it; and
  `sessions::engine::evaluate` gives the first rule that holds, highest priority first, within a
  deadline of 5 ms checked between matchers, past which the state holds. A tracker
  per session (`sessions::state::Tracker`) turns verdicts into the taxonomy with the debounce and
  the 3 s grace period, from instants it is given, so its tests need no clock. The thread judges
  a session's screen only after it changed, at most 10 times a second, and wakes on its own only
  while a debounce or grace period waits. It sends the UI thread only changes
  (`UserEvent::Sessions`). ADR 0121's step 3 feeds it a hosted session's screens; step 2 built
  it against recorded screens, and `session-explain-file` explains a saved one in
  `*Session explain*`, a buffer the agent channel cannot read.
- A hosted session (`claude`, ADR 0121 step 1) gets ready on a short `oso-session-start`
  thread of its own (`sessions/start.rs`): the id from `/dev/urandom`, the directory and
  `[sessions] claude` checked, Claude Code's registry read before a resume and again 5 s after
  (`sessions/registry.rs`, liveness by `oso_platform::process::start_time`), the shell found, and
  in a trusted project ADR 0122's baseline, waited for at most 2 s. Its command line is built by
  pure functions (`sessions/command.rs`, `sessions/launch.rs`). The same thread starts its
  `oso_term::Terminal`, with the shell ADR 0120's terminal finds (`terminal::hosted::find_shell`)
  and that terminal's environment, and the app installs it in the buffer as it installs a
  terminal's (`app/terminals/start.rs`). The app's table of hosted sessions (`sessions/hosted.rs`, glue in
  `app/sessions/hosting.rs`) holds each session's buffer and its `claude`'s process id
  (`oso_term::Terminal::pid`); the set oso remembers, `sessions` in the state directory
  (`sessions/record.rs`), is read and written on `oso-sessions`. A bridge's claim of a session is
  judged on the UI thread by one pure function (`sessions/bridge.rs`) when the channel's server
  asks (`Event::Claims`).
- Worktree sessions (`crates/oso/src/sessions/worktree/`, glue in `app/sessions/worktrees.rs`,
  [ADR 0121](adr/0121-agent-sessions-and-spaces.md) step 4) run as jobs on `oso-sessions-git`,
  at most 2 threads, beside the sessions worker so that its rules never wait on git.
  - The jobs list branches, make a worktree (`create.rs`: brief and hand-off facts, `git
    worktree add`, `.worktreeinclude`'s copies), list worktrees row by row (`checks::look`,
    rendered by `list.rs`), and check and remove one (`checks.rs`, `merged.rs`).
  - Every git run is ADR 0122's runner (`review::git`); `gh` runs through its bounded
    collector.
  - Each job takes the project's trust when it is asked for (`jobs::Trust`), checks it again
    before it runs, and refuses a repository whose main checkout is not trusted too. What
    touches the disk, the new worktree's trust included, is made on the job's thread.
  - A job reads the repository's remotes, main branches, GitHub repository and the registry's
    sessions once (`facts.rs`). A listing sends every row without `gh`, then what `gh` says of
    the rows that need it (`Report::Merged`); a newer listing stops an older one.
  - What only the UI thread knows (unsaved buffers, unsent briefs, hosted sessions) goes with
    each job as a `Snapshot`, and is checked again before the removal's question and its `y`,
    whose job runs every check again before it removes.
  - The dispatcher waits for the branches before `session-worktree`'s prompt opens, as it
    waits for the clipboard (`dispatch/branches.rs`); `app/sessions/worktrees.rs` asks for them
    (`sync_branch_wait`), and the listing goes ahead of other jobs.
- Project search (in-process and ripgrep-style,
  [ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md)),
  git status and language servers stream results in asynchronously.
- An agent review's git and diffing run on a thread per review, `oso-review`
  (`crates/oso/src/review.rs`), started only from a trusted project's root. It lists the
  scope's files with `git diff --raw -z` (untracked files through a copy of the
  index, in a private `reviews/tmp/` under the state directory), reads old sides through one
  long-lived `git cat-file --batch` fed on a piped standard input, reads work tree files from
  the top a directory at a time following no link, and diffs each file with `similar` into
  `oso_core::review`'s hunks. A file whose signature did not change is not read again, and
  shown files go first. Each listing, the readings after it and the `Read` that ends them
  share a generation; refreshes asked for meanwhile are answered as one. Before each request
  the root's device and inode are looked at again, and git never looks above the root
  ([ADR 0122](adr/0122-agent-review.md)).
- Each review also has a view thread, `oso-review-view` (`crates/oso/src/review/view.rs`),
  which lays out each file's section and builds the review buffer's whole text through
  `load::Builder`, compared with the text before, and builds the next only once the UI put
  the last in, so one text is in flight; with nothing changed it lays out nothing. The UI
  thread (`app/reviews.rs`) puts each text in at O(1), in order, while no prompt holds it, and
  frees what it replaces, the text, rows and highlights (`ReviewLeftover`), off the UI
  thread. The review menu's counts come from at most two `oso-review-count` threads
  (`review/count.rs`), an epoch per opening of the menu. A review buffer's added and removed
  lines, headers and cards are bands the window fills behind its text for the rows in view
  (`Editor::review_cells`).

### Configuration

User configuration is TOML: key bindings, menus, theme, fonts and per-mode
settings. It reloads on file change. Inside repositories, only EditorConfig is
read. See [ADR 0019](adr/0019-configuration.md).

- One file, `config.toml` in `$XDG_CONFIG_HOME/oso/` or `~/.config/oso/`
  (`%APPDATA%\oso\` on Windows). oso never writes it, and `--no-config` starts
  without it, without the record of trusted projects, and without the local agent rules,
  so the bundled rules apply.
- A thread in the tip crate's `config` (`config::Reader`) reads, parses and builds it at
  start and when a `stat` once a second, on focus or after a save sees it change, and the UI
  thread swaps the result in between keys (`app/configuration.rs`), dropping the old keymap
  on another thread. A bad file leaves the last good one, with messages.
- The record of trusted projects, `trusted-projects` beside the file, has a thread of its own
  (`config::TrustFile`, `oso-trust`): it reads the record with the file's checks, refusing a
  group too, and makes what `trust-project` and `untrust-project` changed in the file as it is
  then, through a part file renamed over it, and reads it back. The editor holds the record
  (`Editor::set_trust_list`) and asks it in `Editor::project_trusted`
  ([ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md)).
- `config::parse` is a pure function from the file's text to the user keymap, the menus
  in force, the settings and the problems, with their lines. The user keymap also leaves
  undefined each default key of a built-in menu the file removes
  ([ADR 0117](adr/0117-menus.md)). A syntax error rejects the file; a bad entry is skipped
  and the rest applies. See [ADR 0110](adr/0110-adr-0108-as-built.md).
- User keys are a keymap directly above the global one, which can also leave a
  key undefined. A setting applies at start and when its value in the file
  changes. See [ADR 0108](adr/0108-the-user-configuration-file.md).
- The app keeps the settings in force (`config::InForce`: the file's, or the compiled-in
  value of each it leaves out) and applies only those a look changes, so a choice made at run
  time stands. The theme shown last is remembered with the configured default then, in
  `oso/theme`, and wins at start while that default is the file's.
- The local agent rules file, `agent-rules/claude.toml` beside the file, is read on the same
  thread with the same checks and at most 256 KiB, compiled there over the bundled rules
  (`config/agent_rules.rs`), and handed to the sessions worker for every session at once; a
  file that fails to parse keeps the rules in force. Its format is in
  [agent-rules.md](agent-rules.md) ([ADR 0121](adr/0121-agent-sessions-and-spaces.md)).
- `[sessions]` says how a hosted session's `claude` starts: `claude`, its absolute path, and
  `env`, the names of more variables a session takes from oso's environment; the start thread
  reads them as they are in force when the session starts
  ([ADR 0121](adr/0121-agent-sessions-and-spaces.md), step 1). `steer_worktrees` puts the
  steering hook into a session's `--settings` (step 4).
- Indentation ([ADR 0115](adr/0115-indentation.md)): each buffer has an indentation setting,
  property by property from the EditorConfig files that apply to it, then the configuration's
  `[modes.<mode>]`, then the defaults, and knows where each came from (`describe-indentation`).
  The loader reads the EditorConfig files from the file's directory up, through the stop checks
  it shares with project detection (`project::upward`), before it builds the display map, and a
  thread reads them again after `C-x C-w`; parsing and matching are pure functions in
  `oso_core::editorconfig`. Indentation itself is computed on the UI thread at the key by
  per-mode rules in `oso_core::indent`, from the text alone, within a budget: no tree and no
  worker.

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
  ([ADR 0009](adr/0009-project-trust.md),
  [ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md)).
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
    `channel::wire`, which the dev socket shares, and no dev method.
  - `oso mcp` (`channel/mcp.rs`) is a subcommand of the oso binary that opens no window: MCP's
    JSON-RPC on stdin and stdout, with the tools `state`, `text`, `commands`, `look`,
    `change`, `open` and `cancel`, which takes back a question the agent's change opened
    (ADR 0093). It connects on the first tool call, asks the channel's `version`
    first, and connects again after oso restarts. It looks through every instance's socket
    only while it knows none to use, at most once each 5 s, since each look counts toward each
    instance's connections a minute. stdin is read on a thread of its own
    (`channel/mcp/stdin.rs`): the end of stdin, or a cancel naming the call in flight, closes
    that call's connection, a cancelled request that still waits never runs, and at most 8
    lines wait, so a cancel is never stuck behind them. To use it with Claude Code:
    `claude mcp add --scope user oso -- /absolute/path/to/oso mcp`, then `M-x
    agent-channel-start` in oso, and `y` to the question when Claude Code first calls a tool.
  - `open` visits a file that exists, unshown, through `app/agent/open.rs`, and shows it at
    its line once the loader has judged it ordinary and read it; one judged otherwise is
    dropped. A file a buffer already holds, by its path at once or by device, inode, time and
    size once loaded, is shown in that buffer instead, if the channel may show it
    (`dispatch::unshowable`, which `switch-to-buffer` asks too), and that buffer keeps its
    mark (ADR 0094). Until its open is done, a buffer the channel is opening is left out of
    `state`, `text`, `switch-to-buffer` and the dev restart's handoff. `text` copies rows or the region off the UI thread (`channel::text`). A buffer
    the channel opened is marked, and `save-buffer` run for the agent asks the person before
    saving it. A save a command run for the agent queued, or the person's `y` to its question,
    goes through `saver::spawn_for_agent`, which refuses it on the saving thread, right before
    it writes, when a name it writes through or where that leads is in oso's configuration
    directory or is what an entry of it leads to, by canonical path or by device and inode
    (ADR 0108). The `run` reply waits for that save's end, up to 8 s, then says it still
    saves. `ArgSpec::Text` and `insert` take text, and `switch-to-buffer` a buffer's name that
    `C-x b` lists: `Dispatcher::run_for_agent` resolves it once and shows that buffer, unless it
    may hold secrets, is still loading or has keys of its own, such as `*Emergency Copies*`
    (ADR 0094, ADR 0095); `dispatch::args` maps a given argument
    for the dev socket and the channel alike. A count, which repeats a command's work, is at
    most 10,000 either way, as ADR 0044 has it; a number such as a line is a value, and goes
    as given. A reload of a file the channel opened checks it as `open` does, and a reload
    that finds the file elsewhere makes the agent's save of it ask. An `oso dev` restart keeps
    the mark: the buffer's handoff entry says `"opened_by_agent": true`, only when it is.
  - What opening or saving starts runs for the channel only when it is oso's own work in its
    process, such as the final newline and the whitespace cleanup. From M4, each kind of thing
    opening or saving starts declares `Runs::InOso` or `Runs::Program` in `oso-core`; a
    `Program` kind for an agent's open or save marks the buffer instead, and the person's next
    command there, or next save of it, lets it run (ADR 0118). Nothing in M3 is `Program`.
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
