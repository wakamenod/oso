# 0068. Cut clusters, short pieces, the cursor box and lines by direction

- Status: Accepted
- Date: 2026-11-07

## Context

The renderer bounds what a row costs in three places, and each has to keep the
screen honest about the file:

- **The budget of characters per column** ([ADR 0040](0040-buffers-loading-motion-and-scrolling.md),
  [ADR 0054](0054-limits-on-a-display-cluster.md)). Past it, a cluster is shaped by
  its first character, so what that leaves out has to show as left out.
- **The caps on a frame** ([ADR 0061](0061-panes-clipping-and-layers.md)). Within a
  pane, the cursor box is filled after the background and the boxes it goes over.
- **The split by direction** ([ADR 0066](0066-text-stays-in-logical-order.md)). Each
  run of one direction was a line of its own, so a row cost a line per change of
  direction.

A row is also shaped in pieces wherever the face changes (ADR 0061), and a piece that
comes back, such as a label, a name on the modeline or a single character, was shaped
again each time.

These are three decisions and a cache, taken together because they come from one pass
over the same code. A later ADR can replace any one of them.

## Decision

### Past the budget, a cut cluster is boxed and shaped alone

- Past the budget, a cluster of one character is shaped as before.
- A cluster of several characters is shaped by its first one, in `Face::Boxed`, with a
  box behind it. It is a piece of its own, and so is what follows it, so nothing forms
  with what is around it, a label or another cut cluster included.
- Cut clusters side by side share one box.
- A label comes one character a run, so it is never cut.
- The bound stays the same: at most `CHARS_PER_COLUMN` characters per column, one more
  per column past it, and one cluster.

### Short pieces are kept by their text

- When a row is shaped in pieces, a piece of up to 64 bytes is kept by its text, up to
  1024 of them per pane, and shaped again only when the font changes. A shaper's output
  depends only on its text and the font, so a kept piece is what shaping it would give.
- Kept pieces hold buffer text, so they go at each revision, when the pane shows another
  buffer, and when its cached rows are dropped ([ADR 0028](0028-secrets-handling.md)).
- A row of one piece is shaped as before, and the rows themselves are still cached by
  display row (ADR 0051).

### One rectangle is held for the cursor box

- While a text pane whose cursor box shows fills its background and draws its rows, one
  rectangle is held back, and the cursor box takes it. Neither its background nor its boxes can take the
  cursor box's place.
- Panes in front still come first ([ADR 0061](0061-panes-clipping-and-layers.md)).

### On macOS, the runs of one direction are shaped as one line

- The split by direction is the same as in ADR 0066: the same graphemes go to level 1.
- The runs of each direction are put together in one line, at that direction's level,
  each after a ZWNJ (U+200C). ZWNJ keeps letters from joining and ligatures from forming
  across it, so each run shapes as it would alone. The ZWNJ is the backend's own: it maps
  to no byte of the row, and its glyphs are dropped.
- So a call to `TextSystem::shape` makes at most two lines, however often the row
  changes direction. A row of one direction is one line, as before.
- The glyphs of each run are moved so that the runs follow one another in the order of
  the text, as if each had been shaped on its own.
- If a joined line does not come out as its runs would alone, its runs are shaped apart,
  a line each, as before this ADR. It has not: a separator's glyph has an advance, or the
  first grapheme of a run has no glyph, which it always has alone.
- The logic of the split moves to `text/coretext/direction.rs`, a private module.

## Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0040](0040-buffers-loading-motion-and-scrolling.md) and
  [ADR 0054](0054-limits-on-a-display-cluster.md), "the renderer still shapes clusters by
  their first character": a cluster of several characters is shaped by its first one in
  a box, as a piece of its own.
- [ADR 0061](0061-panes-clipping-and-layers.md), `Face::Boxed`: it is also the face of a
  cut cluster. The caps: a text pane holds one rectangle back for its cursor box.
- [ADR 0061](0061-panes-clipping-and-layers.md), "each span, and each face within it, is
  shaped on its own": a cut cluster is too, and a short piece is shaped once.
- [ADR 0066](0066-text-stays-in-logical-order.md), the macOS split: runs of one direction
  share a line.
- [ADR 0066](0066-text-stays-in-logical-order.md) and
  [ADR 0036](0036-text-system-and-glyph-atlas.md), the order of glyphs from `shape`: the
  glyphs of the left-to-right line come before those of the right-to-left one. Only the
  doc of `TextSystem::shape` changes.
- [ADR 0066](0066-text-stays-in-logical-order.md) kept characters oso adds out of the
  shaped text. The ZWNJ is one, but it maps to nothing of the row and draws nothing, so
  the cells and their clusters are as ADR 0066 has them.

## Consequences

- What is past the budget shows as cut, in the face of boxed code points, for any font,
  color glyphs included. Real text stays within the budget, so it does not change.
- The cursor box shows whenever the frame has room for one more rectangle than the panes
  in front take.
- Shaping one row of real text, in a release build with Menlo at scale 2, on one
  machine, the median of 3 rounds of the best of 15 runs of 50 calls, in µs before and
  after: Hebrew prose with punctuation and brackets (128 columns) 99 and 79, Hebrew in code
  (128) 55 and 31. Rows of one direction run the same code as before; the runs gave ASCII
  3.3 and 2.4, Japanese 8.8 and 6.1, Hebrew words 70 and 75 and Arabic words 52 and 55,
  which is the spread of the runs.
- What a row costs past these rules is bounded as they say: at most two lines per call to
  the shaper, a cut cluster shaped alone, and short pieces shaped once.
- A row that changes direction checks its joined lines, at the cost of sorting the
  clusters of its glyphs.
- A key that reshapes a whole screen of right-to-left text still misses 8 ms at wide
  windows, as ADR 0066 says.
- Kept pieces take up to 1024 short strings and their glyphs per pane.
- The positions within a cluster are what they were. Across runs of one direction shaped
  together, kerning between runs is gone, which the cell grid never used.
- The tests check, against the real shaper, that runs shaped together give each cluster
  the glyphs and advances it gets alone, with no fallback, for Hebrew, Arabic, Latin,
  flags, emoji sequences, Hangul jamo, Devanagari and Thai on both sides of a run of the
  other direction.
- `line_up` finds each glyph's run by a binary search, twice, which is a few percent of a
  row that changes direction. It is left so.

## Alternatives considered

- **A budget of lines per row, passed through `TextSystem::shape`, with the rest at
  level 0:** bounds the cost, but changes the public trait, and level 0 is what
  ADR 0066 found loses letters in pointed Hebrew in Courier New.
- **A space between runs:** the same glyphs, but a space comes from the primary font,
  so each run starts a new font run in the fallback font, which costs more.
- **Showing a cut cluster's first character in the escape face, with no box:** the color
  says it is cut, but a color glyph such as an emoji does not take a face's color.
- **Cut clusters together in one piece:** fewer calls to the shaper, but what is cut
  could form with its neighbors.
- **Labels of the cut characters:** shows exactly what is there, but a cluster's
  label takes more cells than the cluster, so the row's columns would no longer
  match the display model.
- **A box per cut cluster:** one rectangle per cell on such rows, which runs into the
  frame's cap sooner.
- **Filling the cursor box first:** it would go under the boxes it covers.
