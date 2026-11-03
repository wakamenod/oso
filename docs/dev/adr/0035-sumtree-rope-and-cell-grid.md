# 0035. SumTree, rope chunks and the cell grid

- Status: Proposed
- Date: 2026-11-03

## Context

[ADR 0022](0022-text-buffer-and-file-loading.md) decided that oso builds
its own `SumTree`, and builds the rope and the display-model layers on it.
[ADR 0005](0005-text-rendering.md) and [ADR 0006](0006-display-model.md)
decided the cell grid and the layered display model. M1-4 implements them,
and that needs more concrete rules:

- the chunk size and the chunk boundaries
- what the summaries hold
- how cell widths are measured
- how a column on a 10 MB line is found without scanning the line

The budgets are the first screen of a 100 MB file within 50 ms, no freeze
on a 1 GB file or a 10 MB line, and O(log n) mapping in both directions
([`roadmap.md`](../roadmap.md#performance-budgets)).

## Decision

### SumTree

- A persistent B-tree. Each node holds 8 to 16 children (or items); only
  the root may hold fewer. Every node caches the summary of its subtree,
  and leaves cache the summaries of their items.
- Nodes are immutable and shared through `Arc`. Edits copy only the path
  they change, `clone()` is O(1), and a tree is `Send + Sync`.
- A tree is sought by any dimension accumulated from summaries. A cursor
  seeks in O(log n) and steps to the next or previous item in O(1)
  amortized. `append` and `split` cost O(log n).
- Summaries take no context parameter. A summary that depends on a setting,
  such as the tab width, carries the setting itself.

### Rope

- Chunks hold at most 1 KB of UTF-8 and end on grapheme cluster
  boundaries. A character, a `\r\n` pair or an emoji sequence is never split
  across chunks. Only a cluster longer than a whole chunk is split, between
  characters; such clusters occur only in contrived text.
- An edit rebuilds the chunks it touches, and takes in neighboring chunks
  (up to 8 on each side) until both edges of the rebuilt range are cluster
  boundaries. It returns the replaced chunk range (`RopeEdit`), which display
  layers use to follow the edit.
- The rope's summary holds bytes, characters, line breaks and the bytes
  after the last line break. It holds no display data.
- Line breaks are `\n` and `\r\n`, counted as one. A lone `\r` is not a line
  break (ADR 0022).
- Out-of-range offsets are clamped to the text, and an offset inside a
  character moves to its start. No method panics.

### Cell grid

- A grapheme cluster takes the East Asian Width of its text
  (`unicode-width`), clamped to 1 to 2 cells. Emoji sequences take 2 cells.
- **Ambiguous-width characters take 1 cell.** Width 2 may become a setting
  later.
- **Every cluster takes at least 1 cell.** Zero-width characters such as
  ZWSP and bidi controls stay visible, and the cursor can stop on them.
- **Control characters are shown as escapes, as in Emacs.** C0 and DEL use
  caret notation in 2 cells (`^@`, `^M`, `^?`), and C1 uses an octal escape
  in 4 cells (`\200`). A lone `\r` is shown as `^M`.
- A tab advances to the next multiple of the tab width (1 to 256, default
  8).

### TabMap

- The TabMap is its own `SumTree`. Its items mirror the rope's chunks and
  share their text through `Arc`.
- Each item's summary holds its bytes, its line breaks and the *column
  function* of its last line. The column function says how the column at
  the end depends on the column at the start: either `c + lead`, or, after
  a tab, `next_tab_stop(c + lead) + after_tab`. Column functions compose,
  so the column of any offset is found in O(log n) plus a scan of one
  chunk. Tabs on a long line cost nothing extra.
- The TabMap follows each `RopeEdit` by replacing its items for the
  rebuilt chunks. Changing the tab width rebuilds the TabMap in O(n). The
  rope does not change.
- A `DisplayPoint` is a row and a cell column. Rows are buffer lines,
  because soft wrap is off by default. FoldMap and InlayMap will sit below
  the TabMap, and WrapMap and BlockMap above it. None of them exists yet,
  and there are no empty placeholder layers.

## Consequences

- The rope stays a plain text structure, and the display model owns
  everything that depends on fonts or settings.
- Measured on the author's Mac (release build, `crates/oso-core/examples/text_perf.rs`):
  - building a rope from 100 MB takes about 35 ms
  - offset ↔ line conversions take under 1 µs
  - column mapping on a 10 MB line takes about 3 µs per query, the same as
    on a 1 MB line
- Building the TabMap measures every cluster: about 0.5 s for 100 MB of
  mixed text. M1-6 must build it in the background as the file loads,
  like the rope.
- The TabMap keeps a second tree, of about one item per 1 KB of text.
- Text with clusters longer than 1 KB is measured piece by piece, so such a
  cluster is wider than 2 cells. Mappings stay consistent.
- `unicode-segmentation` and `unicode-width` are new dependencies of
  `oso-core`. Both are on Unicode 17.

## Alternatives considered

- **Cell widths in the rope's summary:** one tree instead of two, but the
  rope would depend on the tab width, and changing it would rebuild the
  buffer.
- **A context parameter on summaries (as in Zed):** general, but every tree
  operation would need the context passed in. Carrying the tab width inside
  the summary is enough.
- **Scanning the line for each column query, with a cap on tab expansion:**
  simple, but O(line length) on long lines, or wrong columns past the cap.
- **Chunk boundaries at character boundaries only:** simpler edits, but
  clusters split between chunks would get wrong widths, and `\r\n` could be
  split.
- **Smaller chunks (128 bytes, as in Zed):** shorter scans within a chunk,
  but eight times as many items and nodes, and slower building.
- **Zero width for invisible characters:** what terminals do, but it hides
  bidi controls (Trojan Source) and leaves cursor positions with no cell.
