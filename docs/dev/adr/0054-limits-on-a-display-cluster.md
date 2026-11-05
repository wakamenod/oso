# 0054. Limits on a display cluster

- Status: Accepted
- Date: 2026-11-05

## Context

A display cluster could hold a whole rope chunk, about 1 KB, in one cell
([ADR 0035](0035-sumtree-rope-and-cell-grid.md)), and the renderer shaped at most
16 of its characters ([ADR 0040](0040-buffers-loading-motion-and-scrolling.md)).
The columns of a row bounded how many clusters it reads, not how much text each
of them holds. Since shaped rows are compared by content ([ADR 0051](0051-m2-plan-decisions.md)), every
visible row is read after each change to the buffer, so the length of a cluster
counts on the typing path.

The longest real clusters tested hold 8 marks and 10 characters, so limits at or
above those leave real text as it is. What is past a limit must stay visible.

## Decision

- A display cluster holds at most **8 marks** after its first character, and at
  most **16 characters**.
- A mark is a character that takes no cells or that extends the character before
  it (`unicode-width` and `unicode-segmentation` say which). Spacing marks and
  emoji modifiers count, and no new dependency or table is needed.
- Each character past a limit, to the end of the grapheme cluster, is its own
  display cluster: its code point in a box (`U+0301`), `CellKind::PastLimit`, one
  cell per label character. It is a cursor stop, as other boxed characters are.
  An invisible character stays `CellKind::Invisible`, which looks the same.
- ASCII past a limit stays text. It can only follow a prepended character, and
  it reads the same.
- The next grapheme cluster starts over. So does a new chunk, where a grapheme
  cluster longer than a chunk is split ([ADR 0035](0035-sumtree-rope-and-cell-grid.md)).
- Whether a character is past a limit depends on the clusters before it, so a pass
  of `Clusters` that resumes in a line takes that from the scan that found where to
  resume, at no extra cost.
- Past its budget of characters per column the renderer still shapes clusters by
  their first character, and it still draws at most 32 glyphs per span.
- This is display only. The buffer, saving and search keep the text as it is.

### Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0035](0035-sumtree-rope-and-cell-grid.md), a cluster takes 1 to 2 cells: a
  cluster past the limits takes more, as one longer than a chunk already did.
- [ADR 0040](0040-buffers-loading-motion-and-scrolling.md), "At most 16 characters of
  a cluster are shaped": the renderer no longer cuts a cluster, because the display
  model has already limited it. What never reaches the shaper now includes
  characters past the limits.
- [ADR 0045](0045-text-grid-snapshots.md), the `#` mark: it now marks these boxes too.

## Consequences

- The columns of a row bound how many clusters it reads, and each holds a limited
  number of characters. Finding where a cluster ends still reads its grapheme
  cluster, at most one chunk, and what it reads is not read again for the clusters
  that follow in it.
- Characters past a limit are shown as code points and can be reached by the
  cursor, and their row is wider.
- What real text shows does not change. Tests keep whole: decomposed Vietnamese,
  Hindi, Kannada, Khmer, Hebrew with marks, a long Tibetan stack (8 marks), an emoji
  sequence of 10 characters and 6 marks, and a tag flag (6 marks).
- Rare text can go past a limit, such as stacks written to test fonts.
- Reading a screen of real text costs about the same: up to 4% more on ASCII and 3%
  on Japanese, up to about 8 µs, and no change beyond noise on Hindi and emoji. A
  cursor step costs up to about 0.2 µs more.
- `CellKind` has a new variant, which is visible in the API snapshot.

## Alternatives considered

- **Splitting at the limits with no boxes:** no new kind, but the pieces then run
  together into one long run of marks for the shaper, and stack marks on nothing.
- **More marks (16), or only a limit on characters:** keeps more stacks whole, but
  no real text was found that needs it.
- **Fewer marks (4 or 6):** rows shape faster, but this is close to what Hebrew and
  Tibetan use.
- **Lowering the budget to 3 characters per column:** cuts rows of emoji with skin
  tones and tag flags, which take 3.5, to their first characters.
- **Shaping at most 8 or 10 characters of a cluster:** a small change, but what is
  past it is not drawn, and 8 cuts real emoji sequences.
- **Reading only rows whose chunks changed:** draws the same, but rows that move and
  first views cost as much as before.
- **Cutting a row after a number of bytes, with a mark:** bounds reading, but what
  is cut cannot be read at all.
