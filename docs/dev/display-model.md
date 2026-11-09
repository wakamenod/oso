# Display model

The display model maps buffer positions to what the screen shows. Buffer
content and screen content differ in many ways:

- Folded ranges are hidden.
- Virtual text appears that is not in the file, such as inlay hints.
- Markup is concealed; an org link shows only its description.
- Tabs expand, and long lines may wrap.
- Blocks such as images or diagnostic details sit between lines.

Cursor motion, scrolling, hit testing and rendering all depend on converting
between buffer and display coordinates correctly and quickly. The decision
record is [ADR 0006](adr/0006-display-model.md).

## Transform pipeline

```
Buffer (rope, file content)
  ↓ FoldMap     hide ranges and show a placeholder ("…")
  ↓ InlayMap    insert or replace text for display only (inlay hints, link descriptions)
  ↓ TabMap      expand tabs to cell columns
  ↓ WrapMap     soft wrap at the window width (off by default)
  ↓ BlockMap    insert blocks between rows, possibly taller than one row
Display rows
```

- Each layer converts positions in both directions in O(log n), using
  summarized trees in the style of Zed's `SumTree`. oso implements its own
  `SumTree` in `oso-core`, and the buffer's rope is built on it too
  ([ADR 0022](adr/0022-text-buffer-and-file-loading.md),
  [ADR 0035](adr/0035-sumtree-rope-and-cell-grid.md)).
- Layers update incrementally. An edit invalidates only the affected range:
  the rope reports the chunks an edit replaced (`RopeEdit`), and each layer
  replaces its items for them. The buffer does this on clones and swaps them in
  together ([ADR 0053](adr/0053-edit-core.md)), so the layers never disagree.
- Only visible rows are laid out and rendered.
- The whole pipeline lives in `oso-core`. It has no GPU dependency and is
  unit- and property-tested.

### What exists today

M1-4 built only what the viewer needs (`oso_core::display_map`):

- **TabMap.** A `SumTree` whose items mirror the rope's chunks and share
  their text. Each item's summary holds its line breaks, its bidi controls and
  the column function of its last line, so the column of any offset is found in
  O(log n) plus a scan of one chunk, even on a 10 MB line full of tabs, and
  whether a buffer holds a bidi control in O(1). The tab width is a parameter;
  changing it rebuilds only the TabMap. A chunk read often keeps checkpoints in its
  lines longer than 128 bytes, cluster starts every 128 bytes where a scan can begin
  in the state a scan from the chunk's start has there, so a scan in a long line reads
  at most one stride ([ADR 0099](adr/0099-checkpoints-in-long-lines.md)).
- **`DisplayMap` and `DisplaySnapshot`.** The API callers use: offset ↔
  `DisplayPoint` (row, cell column) with a `Bias` for positions inside wide
  clusters, row widths, and the clusters of a row from any column, for
  rendering. A snapshot is O(1) to take and can go to another thread.
- **`DisplayCluster::for_each_run`** says what a cluster puts on the grid. The
  renderer and the text-grid snapshots both draw through it
  ([ADR 0045](adr/0045-text-grid-snapshots.md)).
- **Rows are buffer lines**, because soft wrap is off by default.
- **Invalid bytes.** The TabMap holds the buffer's invalid-byte table
  (`oso_core::decode::InvalidBytes`), its only copy: the buffer reads it
  from the display map. The U+FFFD of an invalid byte is a
  display cluster of its own, `\xNN` in 4 cells, even where a grapheme
  cluster would join it to a neighbor
  ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)).
- **Cursor stops** are display clusters (`oso_core::motion`), so motion,
  hit testing and drawing agree on every cluster. An edit can move those
  boundaries, for example a combining mark joining the letter before it, so the
  cursors and marks in the chunks an edit rebuilt are clipped to a cluster start
  after it ([ADR 0053](adr/0053-edit-core.md), [ADR 0091](adr/0091-multiple-cursor-keys-and-m-y.md)).
  Clipping no others is sound because **a display cluster depends only on the text of
  its rope chunk and where that chunk starts**: the rope rebuilds the chunks around an
  edit out to grapheme boundaries, and the clusters of every other chunk stay as they
  were. FoldMap and InlayMap must keep this, or say what an edit changed beyond its
  chunks.
- FoldMap and InlayMap will go below the TabMap; its items then mirror
  InlayMap chunks. WrapMap and BlockMap will go above it; a display point
  then differs from a TabMap point. There are no placeholder layers.

## Cell grid

Layout uses a monospace cell grid. A half-width grapheme takes one cell and a
full-width grapheme takes two (Unicode East Asian Width). The x position of a
cell is `column × cell_width`, so hit testing and cursor motion need no
shaping. Shaping and rasterization are still used for drawing (emoji, CJK,
ligatures) through the OS-native backend. Proportional fonts are not
supported.

The rules in detail ([ADR 0035](adr/0035-sumtree-rope-and-cell-grid.md)):

- A grapheme cluster takes 1 or 2 cells, unless it goes past the limits below.
  Emoji sequences take 2.
- Ambiguous-width characters take 1 cell.
- Bidi controls and invisible characters (ZWSP, ZWNJ, tag characters,
  U+2028) are boxed code points such as `U+202E`, one cell per label
  character. One that would extend a cluster is split out, except inside
  emoji sequences and after non-ASCII text.
- Every other cluster takes at least 1 cell, so a lone combining mark stays
  visible.
- A display cluster holds at most 8 marks after its first character and 16
  characters. Each character past that, to the end of the grapheme cluster, is
  a boxed code point, but ASCII, which stays text. A new chunk starts the limits
  over ([ADR 0054](adr/0054-limits-on-a-display-cluster.md)). Past the renderer's
  budget of characters per column, a cluster of several characters shows its first one
  in a box ([ADR 0068](adr/0068-cut-clusters-short-pieces-the-cursor-box-and-lines-by-direction.md)).
- Control characters are shown as escapes, as in Emacs: `^@` to `^_`, `^?`
  (2 cells) and `\200` to `\237` (4 cells). A lone `\r` is shown as `^M`.
- An invalid byte of the file is shown as `\xNN` in 4 cells, in the escape
  color, so it is not mistaken for text that reads `\x41`.
- A tab advances to the next tab stop. `\n` and `\r\n` end the row. With whitespace
  shown, a tab's first cell carries a dim `»`, a full-width space a `□` and the other
  non-ASCII spaces a `·`, as labels in the whitespace face that only drawing sees. Whether
  a row carries them is an argument to `row_clusters`, never set for `text_clusters`
  ([ADR 0097](adr/0097-visual-aids.md)).
- Text that is no buffer's, such as a modeline's, is one row: there a tab and a line
  break are escapes as well (`display_map::text_clusters`).
- Cells stay in the logical order of the text, left to right, in every script, and no
  character is shown as its mirrored form. oso has no right-to-left display
  ([ADR 0066](adr/0066-text-stays-in-logical-order.md)).

A glyph paints only the cells of its cluster and only its row. When the shaper
draws several clusters as one ligature, its glyph paints the cells of all of
them. Above the row it also keeps the part of a row that lies below the
baseline, in pixels, because accents on capitals reach there and letters can
differ only there. Below the row it keeps half that, where marks under a
descender end. Anything further out is cut, including the sides of a fallback
font's letter wider than its cells
([ADR 0052](adr/0052-glyphs-stay-in-their-cells.md)). These bands stop at a pane's
edges, but for the band above the frame's top row
([ADR 0061](adr/0061-panes-clipping-and-layers.md)) and above the modeline and the echo
row ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)).

## Row heights

Rows have uniform height by default. The architecture supports rows of
different heights from the start (inline images, larger headings, multi-row
blocks) by keeping a height index in the BlockMap tree, so `y → row` stays
O(log n). This is cheaper than retrofitting variable heights later.

## Soft wrap and motion

Soft wrap is off by default and can be enabled per mode (for example Markdown
or org) or toggled per buffer. `C-n` and `C-p` move by logical lines.

## Decorations

Decorations are positional ranges grouped into **layers**. Each producer owns
one layer:

| Layer owner | Examples | Notes |
|---|---|---|
| Syntax | tree-sitter highlights | computed in the background, swapped atomically |
| Search | isearch matches | built in M2-20 as rectangles of the view's cells, worked out each frame ([ADR 0082](adr/0082-isearch.md)) |
| Diagnostics | LSP errors and warnings | underline styles and gutter signs |
| Modes | dired file lines, git sections and hunks | carry typed data |
| Selections | regions of every cursor | built in M2-13 as rectangles of the frame's cells, not as a layer ([ADR 0071](adr/0071-mark-and-region.md)) |

- Anchors shift with edits, so decorations stay attached to the right text.
- Decorations do **not** travel with text on copy and yank. This differs from
  Emacs text properties and is deliberate: it keeps async highlighting simple.
- A semantic decoration can carry arbitrary typed data and a **keymap at
  point**. For example, `s` stages the hunk under the cursor in the git status
  buffer.
- Replace-style decorations (concealment, link descriptions) feed the
  InlayMap.

## Windows, frames and floating windows

- A frame holds a tree of split windows. Tabs (tab-bar style) hold separate
  window trees.
- Each window has its own selection set, so the same buffer in two windows can
  be edited at different places.
- Floating windows are positioned freely above the tree. They show the
  minibuffer, menus, completion popups and hover documentation. Their
  placement is decided per feature.
- `oso_core::frame` lays out a frame in whole cells: the window's text area and
  modeline, and the echo area. `frame::float_regions` places a float over them, outside the
  layout: a prompt's, attached to the top edge at the center, whose window goes after the tiled
  ones and is the selected one while it is open
  ([ADR 0072](adr/0072-floating-windows-the-minibuffer-and-questions.md),
  [ADR 0075](adr/0075-a-float-as-a-panel-attached-to-the-top-edge.md)). Its panel, field and key
  chips are rounded boxes the renderer draws in pixels around those cells. `Editor` holds the layout,
  and a window's `ViewSize` is its text area, so scrolling and the cursor stop above
  the reserved rows ([ADR 0057](adr/0057-frame-layout.md)).
- `Editor::status_into` says what the modeline and the echo area read, and the app draws
  those as spans ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)).
- `frame::PixelGrid` maps cells to physical pixels. The renderer draws a list of
  panes, each in its region, apart from the band above the frame's top row and above the
  modeline and the echo row ([ADR 0061](adr/0061-panes-clipping-and-layers.md),
  [ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)), and `Editor::cursor_area` gives
  the input method the cursor's rectangle.
- An input method's preedit is no buffer's and needs no InlayMap: `Editor` holds it, and
  `display_map::row_clusters` splices it into the cursor's row, moving what follows by its
  cells. The renderer and the text-grid snapshots both read that row through it, and
  `motion::cursor_box` places the cursor box on the preedit's caret or after it
  ([ADR 0069](adr/0069-macos-input-method.md)).

## Terminal views

A terminal view renders an `alacritty_terminal` cell grid directly, not a
rope. It implements a searchable-text interface so that isearch, line search,
copy and selection work as in text buffers.
