# 0068. What a row's bounds show, and room for the cursor box

- Status: Proposed
- Date: 2026-11-07

## Context

The renderer bounds what a row costs in three places, and each has to keep the
screen honest about the file:

- **The budget of characters per column** ([ADR 0040](0040-buffers-loading-motion-and-scrolling.md),
  [ADR 0054](0054-limits-on-a-display-cluster.md)). Past it, a cluster is shaped by
  its first character, so what that leaves out has to show as left out.
- **The caps on a frame** ([ADR 0061](0061-panes-clipping-and-layers.md)). Within a
  pane, the cursor box is filled after the boxes it goes over, so it is the
  rectangle a full frame leaves out.
- **The split by direction** ([ADR 0066](0066-text-stays-in-logical-order.md)). Each
  run of one direction was a line of its own, so a row cost a line per change of
  direction.

## Decision

### Past the budget, a cut cluster is boxed

- Past the budget, a cluster of one character is shaped as before.
- A cluster of several characters is shaped by its first one, in `Face::Boxed`, with a
  box behind it. The face change starts a piece of its own, so nothing past the budget
  forms with what is around it.
- A label comes one character a run, so it is never cut.
- The bound stays the same: at most `CHARS_PER_COLUMN` characters per column, one more
  per column past it, and one cluster.

### One rectangle is held for the cursor box

- While a text pane draws its rows, one rectangle is held back, and the cursor box takes
  it. Its boxes and background cannot take the cursor box's place.
- Panes in front still come first ([ADR 0061](0061-panes-clipping-and-layers.md)).

### On macOS, the runs of one direction are shaped as one line

- The split by direction is the same as in ADR 0066: the same graphemes go to level 1.
- The runs of each direction are put together in one line, at that direction's level,
  each after a ZWNJ (U+200C). ZWNJ keeps letters from joining and ligatures from forming
  across it, so each run shapes as it would alone. Its glyphs map to no byte of the row
  and are dropped.
- So a call to `TextSystem::shape` makes at most two lines, however often the row
  changes direction. A row of one direction is one line, as before.
- The glyphs of each run are moved so that the runs follow one another in the order of
  the text, as if each had been shaped on its own.

## Consequences

- What is past the budget shows as cut, in the face of boxed code points, for any
  font, color glyphs included. Real text stays within the budget, so it does not change.
- A row past the budget that alternates cut and whole clusters is shaped in more
  pieces, as a row of alternating escapes already is (ADR 0061).
- The cursor box shows on any screen that has room for one more rectangle than its
  front panes take.
- Shaping a row that changes direction costs less, and its cost no longer grows with
  the number of changes. In a release build with Menlo at scale 2, on one machine, the
  median of 3 rounds of the best of 15 runs of 50 calls, in µs per row before and after:
  Hebrew prose with punctuation and brackets (128 columns) 99 and 79, Hebrew in code
  (128) 55 and 31, a row changing direction at every letter 451 and 183 at 192
  columns and 756 and 302 at 320. Rows of one direction are within noise: ASCII 3.3 and
  2.4, Japanese 8.8 and 6.1, Hebrew words 70 and 75, Arabic words 52 and 55.
- A row changing direction at every letter of 320 columns still costs about twice a row
  of Hebrew words, since every run is shaped. A cold screen of 60 such rows takes about
  18 ms, inside the 50 ms of a first screen. A key that reshapes a whole screen of
  right-to-left text still misses 8 ms at wide windows, as ADR 0066 says.
- The positions within a cluster are what they were. Across runs of one direction
  shaped together, kerning between runs is gone, which the cell grid never used.
- `TextSystem` does not change.

## Alternatives considered

- **A budget of lines per row, passed through `TextSystem::shape`, with the rest at
  level 0:** bounds the cost, but changes the public trait, and level 0 is what
  ADR 0066 found loses letters in pointed Hebrew in Courier New.
- **A space between runs:** the same glyphs, but a space comes from the primary font,
  so each run starts a new font run in the fallback font, and the worst row cost about a
  third more than with ZWNJ.
- **Showing a cut cluster's first character in the escape face, with no box:** the color
  says it is cut, but a color glyph such as an emoji does not take a face's color.
- **Labels of the cut characters:** shows exactly what is there, but a cluster's
  label takes more cells than the cluster, so the row's columns would no longer
  match the display model.
- **Filling the cursor box first:** it would go under the boxes it covers.
