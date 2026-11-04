# 0040. Buffers, file loading, cursor motion and scrolling

- Status: Accepted
- Date: 2026-11-03

## Context

M1-6 connects the command registry ([ADR 0034](0034-command-registry-and-keymaps.md)),
the rope and display model ([ADR 0035](0035-sumtree-rope-and-cell-grid.md))
and CoreText rendering ([ADR 0036](0036-text-system-and-glyph-atlas.md)),
so that oso opens a real file and moves and scrolls through it. Some
behavior was decided with the author before the work started:

- `C-f`/`C-b` move by grapheme cluster.
- Each invalid UTF-8 byte becomes U+FFFD in the rope, with its value in a
  side table, and shows as `\xNN`. Such buffers and binary files are
  read-only and never saved. Decoding streams, so a sequence split between
  reads is not invalid.
- `oso [FILE]`; a missing file opens an empty buffer; a directory or an
  unreadable file is logged and falls back to `*scratch*`.
- Scrolling is conservative (`scroll-conservatively` 101), `C-v`/`M-v`
  keep 2 lines of context, and long lines scroll sideways by the fewest
  columns.

[ADR 0022](0022-text-buffer-and-file-loading.md) and
[ADR 0030](0030-m2-editing-commands.md) fix the rest of the semantics
(progressive loading, BOM, line endings, goal column, `track-eol`). This
ADR records the decisions the implementation needed beyond those.

## Decision

### Buffers, windows and selections

- `Editor` owns the buffers, as `(BufferId, Buffer)`, and the windows.
  There is one window until splits (M3). Every window shows a buffer that
  exists; killing a shown buffer shows `*scratch*`, created again if
  needed.
- A `Buffer` holds its name, its path, the rope, the display map, the
  invalid bytes, the file information (BOM, line ending, binary) and its
  loading progress. Its read-only reason is computed: binary, then invalid
  UTF-8, then loading.
- A `Window` holds its buffer, a `SelectionSet` and its scroll position
  (top row, left column), and the rows and columns it shows in full.
- A `SelectionSet` is never empty, sorted by cursor, with no two cursors
  at one offset, and has a primary selection that the window scrolls to.
  A `Selection` is a cursor (a byte offset at the start of a display
  cluster) and a goal for vertical motion. The mark comes in M2.
- `SelectionSet::update` runs a motion on every selection, merges cursors
  that meet, and reports whether a cursor moved and the first error. A
  cursor that hits a buffer edge moves as far as it can, as in Emacs.
- Commands keep `fn(&mut Ctx, Args)` and reach the state through
  `ctx.editor.current_mut()`: the selected window and its buffer.

### Module layout

These are new public modules of `oso-core`, a structural change under
[ADR 0038](0038-architecture-guardrails.md):

- `buffer`, `selection`, `window` and `editor` (rewritten): the state
  above. `cursor` is removed.
- `decode` and `load`: decoding and building a buffer's content, with no
  I/O; the loading thread and file access are in the tip crate.
- `motion`: motion over a display snapshot.

They depend on `rope`, `display_map` and `sum_tree`, never on `command`
or `keymap`. `oso-render` gains a private `rows` module, the shaping
cache, and the tip crate a `loader` module. No crate edge changes.

`guardrails/layers.txt` ([ADR 0039](0039-architecture-guardrail-mechanics.md))
places them, highest first:

- `oso-core`: `keymap`, `command`, `editor`, `window`, then `buffer` and
  `motion`, then `load` and `selection`, then `display_map` (which now
  uses `decode`), then `decode`, `key` and `rope`, then `sum_tree`.
- `oso-render`: `rows` between `text` and `atlas grid shape`.
- `oso`: `loader` in the lowest layer.

### Invalid bytes

- `oso_core::decode::Decoder` decodes pieces of any size. It carries at
  most 3 bytes between pieces (an incomplete sequence, or the start of the
  file while the BOM is undecided), so its output is the same however the
  file is split. Bytes still incomplete at the end are invalid.
- Invalid bytes are grouped as `std` reports them, and each byte becomes
  its own U+FFFD.
- The first line break decides the line ending, LF without one. A NUL in
  the first 8 KB makes the file binary.
- `InvalidBytes` is a `SumTree` of blocks: the bytes found in one decoded
  piece, each packed as a 24-bit offset in the block and the byte. That
  is 4 bytes per invalid byte, O(1) to clone and O(log n) to look up.
- The display map holds the table. Its cluster scan makes the U+FFFD of
  an invalid byte a display cluster of its own,
  `CellKind::InvalidByte(byte)`, 4 cells wide, even where a grapheme
  cluster would join it to a neighbor. A U+FFFD that was valid in the
  file is ordinary text in 1 cell.

### Motion

- Motion steps over display clusters: grapheme clusters, tabs, `\r\n`,
  escapes, boxed invisible characters and invalid bytes. Each is one
  cursor stop.
- The goal column is in display cells. A goal inside a wide character or
  a tab lands at its start, where the box cursor covers it (Emacs's
  visual line motion; its logical `move-to-column` lands after it).
- Vertical motion past the last line moves to the end of the buffer, and
  before the first to its start, and signals, as Emacs's `next-line`
  does. The goal stays for the next vertical move; every other motion
  clears it.
- `M-<` and `M->` take Emacs's numeric argument (N/10 of the way, then
  the next line start). `C-a` and `C-e` take a count of lines.
- The cursor box covers its cluster's cells, but 1 cell on a tab, as
  Emacs draws it with `x-stretch-cursor` off.

### Scrolling

- After every command, the dispatcher scrolls the selected window by the
  fewest rows and columns that show the primary cursor, as Emacs's
  redisplay does. Commands do not scroll for it.
- `C-v` scrolls by the window's full rows less 2, or by N lines with a
  numeric argument, negative to reverse. It signals "End of buffer" when
  the new top would pass the last row, and then does not move. `M-v`
  stops at the first row and signals "Beginning of buffer" only when
  already there. A cursor that would leave the window moves to the start
  of its nearest row.
- The mouse wheel scrolls 3 rows a notch; trackpad pixels add up to
  rows. Wheel scrolling stops once the last row is at the bottom (a view
  past that, after `C-v`, stays), and moves cursors into view as `C-v`
  does. Horizontal wheel scrolling is not handled yet.

### Loading files

- `oso [FILE]` opens one file; `--` ends options. Only regular files are
  opened, checked before opening, since opening a pipe blocks. Nothing in
  the file is run or interpreted.
- A loading thread per file reads 64 KB at a time, up to the size seen
  at open, so a file that keeps growing (a log) is not read forever. A
  `load::Builder` decodes each piece, appends it to the rope, extends the
  invalid bytes and syncs the display map, all on that thread. A panic
  there fails the load instead of leaving the buffer loading.
- Snapshots (rope, display map, invalid bytes: each O(1)) go to a
  one-entry slot. The thread wakes the UI with a user event only when the
  slot was empty. The UI takes the latest snapshot when woken and before
  each frame. The first snapshot follows the first read; later ones come
  at most every 50 ms; the last one is complete.
- The UI redraws for a snapshot only if the window shows that buffer and
  reaches its old end.
- Dropping the `LoadHandle` cancels the load. A missing file ends as an
  empty, editable buffer; any other failure is logged and kills the
  buffer.
- The event loop's user event is `UserEvent::Load(BufferId)` in every
  build, and `UserEvent::Control` only with the `dev` feature.

### Rendering buffer text

- Each frame draws the visible rows from the window's top row and left
  column.
- Shaping is cached by display row. An entry records the columns it
  shaped, the buffer offsets they cover and the buffer's revision. It is
  reused without looking at the buffer while the revision holds, and
  after a change while the offsets match; text loaded there changes them.
  Switching buffers or fonts drops the cache, and rows far from the view
  are evicted.
- A row is shaped from the left edge rounded down to a multiple of 64 to
  past the right edge one step later, so a 10 MB line costs one screen,
  and sideways scrolling reshapes once per 64 columns.
- At most 16 characters of a cluster are shaped, and past 4 characters
  per shaped column a row's clusters are shaped by their first character
  only. Real text never reaches either limit; a base with thousands of
  combining marks costs a bounded amount.
- The frame is prepared (shaping, rasterizing, uploads) before the
  drawable is acquired, so that work overlaps the wait for it.
- Rope chunks hold at most 1008 bytes instead of 1024, so a chunk and its
  `Arc` counts fit a 1 KB allocation; this keeps ADR 0035's "at most
  1 KB" and cut a 1 GB file from 1.7 GB to 1.4 GB.
- Control characters, invalid bytes and invisible characters are never
  passed to the shaper. Their escapes (`^M`, `\xFF`, `U+202E`) are shaped
  as ASCII labels in an escape color; invisible characters also get a
  box. A tab is shaped as a space.
- A frame rasterizes at most 64 new glyphs, and requests another frame
  for the rest. Screenshots rasterize everything. New glyphs are uploaded
  at the end of the frame, one write per run of neighbors on an atlas
  shelf.

## Consequences

- oso opens files of any size without blocking the UI; see the numbers in
  the M1-6 pull request.
- All motion and scrolling rules are pure functions in `oso-core`
  (`motion`, `window`), tested without a window.
- A binary file of random bytes takes about 3 bytes in the rope and 4 in
  the table per invalid byte. There is no size limit on opening yet.
- A screen of hostile text (a base and 1 KB of combining marks in every
  cell) still takes about 36 ms to segment and shape on first view.
- Search (M2) must skip U+FFFD at offsets in `InvalidBytes`.
- Editing (M2) must invalidate shaped rows by edit range; the offset check
  alone misses an edit that keeps a row's length.
- A screen of new CJK glyphs fills in over a few frames instead of
  stalling one.
- `get-state` reports the buffer, cursor and scroll position, never buffer
  text.

## Alternatives considered

- **Invalid-byte information in the TabMap items:** no separate tree, but
  a source fact would live in a display layer and be lost when the tab
  width changes.
- **A plain `Vec` of invalid offsets:** simpler, but O(n) to snapshot and
  16 bytes per entry.
- **Private-use code points for invalid bytes:** no table, but they are
  real characters that a file can contain.
- **A channel carrying every snapshot:** simpler, but a busy UI queues
  them, and the first frame cannot show content that arrived while the
  window opened.
- **Scrolling inside each command:** explicit, but every command that
  moves the cursor would have to remember it.
- **Landing after a wide character on vertical motion,** as Emacs's
  logical `move-to-column`: the box cursor would jump right of the goal.
- **Shaping whole rows:** simplest, but a 10 MB line would stall the
  frame.
- **A time budget for rasterizing:** adapts to glyph cost, but makes
  frames depend on timing and tests flaky.
