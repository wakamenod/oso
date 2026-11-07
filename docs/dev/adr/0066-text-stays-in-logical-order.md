# 0066. Text stays in logical order

- Status: Proposed
- Date: 2026-11-07

## Context

The grid lays out a row's clusters in the logical order of the text: a column is
`column × cell_width`, and cursor motion, hit testing and the snapshots all read it that
way ([ADR 0035](0035-sumtree-rope-and-cell-grid.md)). Real right-to-left display would
need a visual order in the display model, for motion, selection and hit testing too.
Nothing in oso has one, and nothing planned needs one soon.

Left alone, an OS shaper runs the Unicode bidirectional algorithm over what it is given.
That algorithm decides order and direction for a whole paragraph, which a cell grid in
logical order has no use for. Right-to-left scripts still need their direction to shape:
in Courier New, the font macOS falls back to for Hebrew from Menlo, points on a letter come
out right only at an odd embedding level.

## Decision

These hold for every backend:

- **oso does not do right-to-left display.** Text stays in logical order, left to right,
  in every script. It may be hard to read, and its order on screen is the file's. A later
  ADR can reverse this.
- **Cells stay in logical order.** `TextSystem::shape` names each glyph's cluster and the
  caller places it by that, so the order glyphs come in cannot change the order of the cells.
- **No character is shown as its mirrored form.**
- **Right-to-left scripts are shaped with their direction,** so their marks and joining
  work. How is the backend's own.
- **The handling of bidi controls does not change.** They are boxed and never reach the
  shaper, and a buffer with one shows the modeline warning
  ([ADR 0063](0063-modeline-echo-area-and-internal-buffers.md)). These hold for reasons of
  their own, whatever the shaper does.

On macOS:

- **No bidirectional algorithm runs.** `CTTypesetter` makes each line with its embedding
  level forced, and no bidi control is added to the shaped text.
- **A row is split by the direction of its scripts, inside the backend, and only between
  the graphemes of the text the shaper is given.** Each glyph still goes to its cluster.
  - A grapheme is right to left if each of its characters is in the blocks of right-to-left
    scripts (U+0590 to U+08FF, U+FB1D to U+FDFF, U+FE70 to U+FEFF, U+10800 to U+10FFF and
    U+1E800 to U+1EFFF) or extends the letter before it, and one of them is in the blocks.
    It is shaped at level 1.
  - Any other grapheme is shaped at level 0, so a character prepended to one with a mirrored
    form does not bring it to level 1.
  - A space and ASCII punctuation with no mirrored form stay in the run they are in.
  - A row with none of those blocks is one call at level 0, with no scan of its graphemes.
  - Nothing that reaches level 1 has a mirrored form: neither those blocks, the characters
    that extend a letter, nor that punctuation (`Bidi_Mirrored` and
    `GraphemeBreakProperty` in Unicode 17). So what can be mirrored is always at level 0.
- **`kCTTypesetterOptionAllowUnboundedLayout` is set.** What bounds a row stays as it was,
  the budget of characters per column in `oso-render`
  ([ADR 0040](0040-buffers-loading-motion-and-scrolling.md),
  [ADR 0054](0054-limits-on-a-display-cluster.md)).

### Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0036](0036-text-system-and-glyph-atlas.md), "`shape(row)`: glyphs in visual order":
  each glyph still names its cluster, but a row is no longer in one visual order. Only the
  glyphs within a level 1 run come in the opposite order of their clusters, and `Placer`
  sorts them by column as it did.
- [ADR 0036](0036-text-system-and-glyph-atlas.md), font fallback: `CTLine` runs still give
  it, from a `CTLine` that a typesetter makes.
- [ADR 0061](0061-panes-clipping-and-layers.md), "A row of one face ... stays one string":
  it is one call to `TextSystem::shape`, and the backend can split it by direction.

## Consequences

- Right-to-left text reads left to right, letter by letter. Hebrew keeps its points, and
  Arabic keeps its joining forms and lam-alef, as they were before this ADR.
- The public API does not change, only the doc of `TextSystem::shape`. `coretext.rs` has
  more `unsafe` blocks, each with a `// SAFETY:` comment. `objc2-core-foundation` gains the
  feature `CFNumber`, and `objc2-core-text` the feature `CTTypesetter`. `oso-platform` now
  uses `unicode-segmentation`, the version `oso-core` has. No crate is added.
- Shaping costs about the same on a row of one direction, and more on a row that changes
  direction often. In a release build with Menlo at scale 2, on one machine, the median of 3
  rounds of the best of 15 runs of 50 calls, in µs per row before and after: ASCII 128
  characters 3.2 and 3.3, Japanese 64 characters 6.4 and 6.3, Hebrew words 70 and 73, Arabic
  words 63 and 67, pointed Hebrew 88 and 91, and Hebrew prose with punctuation and brackets 67
  and 105. These are for a row that changed: shaped rows are cached
  ([ADR 0051](0051-m2-plan-decisions.md)).
- `just perf` holds no right-to-left text, so it does not see those rows. Adding such a
  file is open (`open-questions.md`).
- The tests are in `coretext.rs`, against the real shaper, and on Menlo and what it falls
  back to. SF Mono, the first default where it is installed, was not on the machine tested.
- Linux and Windows have no text yet. Their backends have to keep these rules too
  (`open-questions.md`).
- If a second backend needs the same split, `is_right_to_left` moves to `text/mod.rs`.

## Alternatives considered

- **Level 0 for everything:** the simplest rule, with one kind of run. In Courier New, the
  second mark on a letter then joins the next letter, whose glyph is credited to the cell
  before it and cut there. Arabic loses its joining forms too.
- **A writing direction attribute of left-to-right override on `CTLine`:** the same glyphs
  as level 0 in every case tried, so the same result. In a C test at 13 pt it took 3.75 µs
  against 1.55 µs for level 0 on a row of 128 ASCII characters, the commonest row there is.
- **`kCTTypesetterOptionDisableBidiProcessing`:** the same glyphs as level 0, and deprecated
  since macOS 10.8.
- **Breaking pieces around characters that have a mirrored form, with the algorithm still
  running:** keeps today's shaping, but needs a table of those 554 characters that must stay
  complete, and a character it misses is mirrored. Splitting by direction needs five ranges,
  and the forced level says what cannot happen outside them.
- **A table of marks that stay in a run:** no new use of `unicode-segmentation`, but such a
  table has to hold every mark of every script, or a mark goes to another line than its letter.
- **Shaping each right-to-left cluster on its own:** the same letters and marks, but no
  joining, and a shaping call per cluster on every such row.
- **Passing a direction through `TextSystem::shape`, or splitting in `oso-render`:** direction
  is an input to the shaper, and the caller needs only the cluster and the position in it. The
  split depends only on the text, so the row cache's comparison stays valid, and no public
  trait changes.
- **Bidi controls added to the shaped text:** cluster offsets would have to be mapped back,
  and those are characters oso otherwise keeps from the shaper.
- **Real right-to-left display:** a visual order through the display model, motion,
  selection and hit testing. A large change no milestone asks for.
