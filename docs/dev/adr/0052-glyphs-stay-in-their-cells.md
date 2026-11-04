# 0052. Glyphs stay in their cells

- Status: Proposed
- Date: 2026-11-04

## Context

The display model gives each cluster its cells
([ADR 0035](0035-sumtree-rope-and-cell-grid.md)), and the cursor, the boxes
behind invisible characters and hit testing all rely on them. A font can still
draw outside those cells: stacked marks grow up or down, a long sequence shapes
to many glyphs in two cells, and a fallback font's letter can be wider than its
cell. [ADR 0036](0036-text-system-and-glyph-atlas.md) bounds how many glyphs a
cluster draws, but not where they land, so their ink reached neighboring cells
and rows.

Some text does need to pass the row's edges. Accents on capitals reach above
the row, and at the default font Vietnamese capitals such as `Ắ` and `Ẳ` differ
only in those pixels. Marks under a descender end just below it.

## Decision

- A glyph paints only the cells of its span: its cluster's cells, or the cells
  of all the clusters of a ligature.
- Vertically it paints its row, with two bands:
  - above the row, `CellMetrics::descent` pixels, the part of a row that lies
    below its baseline
  - below the row, half of that, rounded to whole pixels
- A span ends where the style changes, so an escape's or a boxed code point's
  label keeps its own cells whatever follows it.
- `Layout::quad` cuts the quad on the CPU, and moves its corner and its atlas
  position by the same amount. Quads and texels sit on whole pixels, so each
  pixel shows the texel it would show uncut. The shader does not change, and
  it stays one instanced draw per frame.
- A glyph with nothing left is not drawn. It has already been rasterized,
  because only the atlas knows its bitmap's bounds.
- Only columns whose cells reach the surface are looked up. The extra column
  for glyphs overhanging from the right is gone.

### Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0036](0036-text-system-and-glyph-atlas.md), "A shaper cluster that covers
  several display clusters (a ligature) spans all their cells": this now holds
  only while those clusters have one style. A cluster crossing from text to a
  label is cut to the cells before the boundary.
- [ADR 0036](0036-text-system-and-glyph-atlas.md), the bounds: the cut is a new
  limit, on where a glyph paints.
- [ADR 0035](0035-sumtree-rope-and-cell-grid.md), "a lone combining mark stays
  visible": it keeps its cell and its cursor stop, but ink outside the bounds
  above is cut, so a mark drawn entirely outside them does not show.
- [ADR 0045](0045-text-grid-snapshots.md), "glyphs that overhang into the
  margin": a glyph reaches the margin only through its bands or a partly shown
  cell.

## Consequences

- What a cluster draws stays with its cells, for any font and any text, in
  O(1) per glyph. `just perf` shows no change in key latency.
- Ink outside the bounds is lost. Measured with Menlo, the default font where SF
  Mono is not installed, at scale 1 in the app and from the text system at scale
  2:
  - ASCII, CJK, kana, full-width forms, hangul, Hebrew and Thai letters: nothing
    or almost nothing.
  - Vietnamese capitals keep their accents, and lose 1 px at scale 2.
  - Emoji and block elements lose up to an edge column. Box-drawing seams show
    no gaps.
  - Fallback letters wider than their cells lose their sides, which reads as
    damage. The share of letters losing over a quarter of their ink at scale 1
    is 32 of 53 in Malayalam, 28 of 53 in Tamil, 73 of 347 in Ethiopic, and
    smaller in other Indic scripts, Myanmar, Khmer and Lao.
  - Breathing marks left of Greek capitals are cut, and marks further below a
    descender than the band lose their lower pixels.
  - Of 631 lone combining marks at scale 1, 3 show nothing and 59 lose over half
    their ink.
- Better rendering of wide fallback letters would need wider cells for them, so
  it would change the cell grid, not this rule.
- A font that builds ligatures from separate one-cell clusters would have them
  cut to one cell. This matters when fonts become configurable
  (`open-questions.md`).
- `Layout` knows the surface, not a pane. Panes need their own clip (M2).
- Glyphs under the cursor box still take `cursor_color` for their whole quad,
  including the bands (`open-questions.md`).

## Alternatives considered

- **A scissor per cell:** the same pixels, but one draw per glyph instead of one
  per frame.
- **A clip rectangle per instance, tested in the fragment shader:** one draw, but
  a larger instance, and overdraw for pixels that are then discarded.
- **No bands, cutting exactly to the row:** the simplest rule, but Vietnamese
  capitals such as `Ắ`, `Ẳ` and `Ẵ` become identical pixel for pixel.
- **A fixed 2 px below the row:** the same at scale 1, but cell sizes grow with
  the scale factor and it would not, so marks would lose more at scale 2.
- **The whole descent below the row as well:** keeps a few more marks, at the
  cost of a wider band in the next row.
- **Sideways bands:** would keep breathing marks and more of wide letters, but
  paint into neighboring cells on the same row, where the rule matters most.
- **Clamping the centering of an overflowing cluster at 0:** keeps its first
  glyph whole, but moves a wide letter's loss to one side.
