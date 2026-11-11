# 0104. Soft wrap drawn in view rows

- Status: Accepted
- Date: 2026-11-10

## Context

[ADR 0100](0100-soft-wrap.md) decides soft wrap. [ADR 0102](0102-soft-wrap-what-building-it-changed.md)
and [ADR 0103](0103-soft-wrap-in-a-window.md) record building its WrapMap and putting it in the
window. Until this step, a wrapped buffer scrolled by view rows but drew its logical lines, cut at
the edge, from the one that held the top (`Window::drawn_top`). This step makes everything that
draws read view rows, as ADR 0100's "How wrapped rows look", "The renderer and the frame" and
"The input method on a wrapped row" say, and binds `C-x x t`.

ADR 0100 left some of this open:

- **What keys the renderer's cache.** ADR 0100 says rows are kept "by their place on the
  screen". The cache kept them by display row, so scrolling reused them. Kept by place, a
  scroll of one row would shape every row again.
- **How far a range past a view row fills:** to the last cell of text, or to the edge before the
  mark.
- **How a provisional row draws.** Before a report, a row near the view may be a whole logical
  line, or the part of a line before an anchor that rows were wrapped from (ADR 0103), which
  goes on in the next row and may be wider than the window.
- **Whether the view follows a preedit that wraps** past the window's last row.
- **The input method's rectangle** for characters of a preedit on rows after its first.
- **Line numbers.** ADR 0097's gutter did not exist when ADR 0100 was written.

## Decision

The author chose each point marked (author) on 2026-11-10, through the coordinator, the
recommended option each time, and approved the public API in direction.

### The rows a frame shows

- **One list of the rows in view, built once a frame,** read by everything that draws:
  - the renderer;
  - the frame's cells of the cursor, the other cursors, the regions and isearch's matches;
  - the visual aids and the gutter;
  - the input method's rectangle;
  - the dev socket's rows;
  - the text-grid snapshots.

  So they agree on every row.
- **A screen row is part of a logical line:** the cell columns of that line from where it starts
  to where the next row starts.
  - It also holds the buffer offsets of both ends, for the searches over cursors and matches.
  - Unwrapped, each logical line is one row, and the window's left column says which columns
    show.
  - Its `Debug` prints no offset or column (ADR 0100).
- **What a row draws:**
  - A row that continues draws its text in the wrap's width, and the mark in the column after.
  - A row that does not continue, provisional ones included, draws up to the window's edge.
- **Building it reads the rows in view:** a seek to the top, then one conversion a row.
- **`Window::drawn_top` goes away,** and nothing that draws counts logical lines.

### Provisional rows and rows wider than the width (author)

- **A provisional whole logical line** draws cut at the window's edge, with no mark, as ADR 0100
  says.
- **A row draws the mark only while it fits** in the wrap's width, or in that and the mark's
  column when the cell there is a U+0020 that hangs. Any other row that continues draws to the
  window's edge with no mark, as a row that does not wrap, until it is wrapped again. That is:
  - a row wrapped at a width the window has since left, as after a resize or a gutter that
    gained a digit, while the command's budget is spent;
  - the part of a line before an anchor rows were wrapped from, which goes on in the next row.
- So the mark's column shows the mark, or a space hanging there, or text of a row with no
  mark.
- **The preedit's line is laid out from the cursor's own row** when the row before it is wider
  than that, so the layout stays within its rows.

### The narrowest wrap (author)

- **Below 10 text columns, nothing wraps,** where ADR 0100 said 8. A row then holds the widest
  display cluster, a box of 8 cells for a character past a cluster's limits
  ([ADR 0054](0054-limits-on-a-display-cluster.md)), and the mark's column after it.

### The renderer (author)

- **A shaped row is kept by its logical line and its start column.** Neither is a buffer offset.
  - The cache reuses a row without reading it while the buffer's revision holds, and its key,
    the column where the next row starts, whether it continues and the columns shaped are the
    same, with no preedit on it.
  - Otherwise it reads the row and compares what it would shape, as ADR 0051 says.
  - So a report or rows wrapped near the view above the top, which change row numbers, shape
    nothing again.
  - A row's clusters count their columns from its first cluster's, and a row with no entry
    takes the one its place on the screen had in the last frame, if that was of the same
    logical line and no row of the frame has its key, and is read and compared with it. So
    the rows after a character typed in a wrapped line, which move a cell along it with the
    same text, shape nothing again; only rows whose text changed do. A page scrolled away
    keeps its rows for coming back.
  - Past six screens of rows and 64 more, the rows of lines more than a screen from the view
    go, and then, if that is not enough, as for a long wrapped line, all the frame did not
    draw: as many as rows were kept before, so a jump to a buffer's end and back finds its
    first page still shaped.
- **The mark is part of the shaped row,** a piece of its own in `Face::WrapMark`, so whether a
  row continues is compared as its content is. A hanging space is left out of what is shaped,
  and the mark draws in its cell.
- **`Face::WrapMark`** is in ADR 0097's 3:1 tier, with its colour in each theme, and used by
  nothing else.

### The frame's cells (author)

- **A range that goes on past a view row fills to the row's text width,** the cell before the
  mark's column, and also takes the mark's column when the range holds a hanging space there.
- A cursor, a region or a match on a hanging space draws in the mark's cell.

### The input method

- **The preedit's logical line is laid out again with the preedit in it,** as ADR 0100 says.
  - It starts where an edit at the cursor would start its rewrap, and reads a row's clusters at
    a time, for at most a screen and a few rows.
  - It is done only while the cursor's row is in view.
  - Its rows' columns count the preedit's cells, as `row_clusters` draws them, and the rows after
    it in view move down.
- **The view follows the caret's row in that layout while composing (author),** so a preedit
  that wraps past the last row scrolls the view a row, as an unwrapped one scrolls it sideways.
- **The input method's rectangle stays one (author):** the cursor's cell in that layout, with
  the characters' x kept in the text area. Each character's row for the input method waits for
  a later change, recorded in `open-questions.md`.

### The gutter (author)

- ADR 0097's line numbers come first, and this step draws them in view rows: a line's number on
  its first row, none on the rows it continues on.
- **A gutter that gains a digit in a command** narrows the text area within the command's
  budget, which its edits share, as ADR 0103 says of the rows near the view. A frame laid out
  again for it starts no budget; a resize does.
- **The gutter does not narrow while its buffer is edited,** as ADR 0105 left to M2-25: it
  widens at once, and fits the line count again when a buffer is shown or the aids change.
  So a newline typed and deleted at 1000 lines does not wrap the rows again at each key.

### The C-x b preview

- **The preview wraps the previewed buffer at the preview's width,** as ADR 0100 says. No window
  shows that buffer, so the editor keeps the preview's wrap:
  - it is made provisional, in O(log n), when the preview changes buffer, width or text;
  - its rows near the preview's start are wrapped within each command's budget;
  - no job wraps the rest, as nothing scrolls a preview.

### The sockets

- **The dev socket's `get-state` `text.rows` and `get-text` give view rows.** `text.start_row` is
  the top's view row.
- The agent channel keeps logical lines (ADR 0100).

### The key

- **`toggle-truncate-lines` is on `C-x x t`,** in `bindings/view.rs`.

### The public API

The exact items are in the pull request and the API snapshot. In outline, with no new public
module:

- in `window`, `Screen` and `ScreenRow`, and `Screen::show_lines` for a view no window holds,
  such as a theme's sample;
- `Window::wrap_of`, a window's wrap only while it follows its buffer's text;
- `Editor::screen`, `Editor::preview_screen` and `Editor::prompt_screen`, the last for a
  prompt's answer, and `Editor::text_screen`, the rows with no preedit, for the dev socket;
- `oso_render::Face::WrapMark`;
- `frame`'s cell functions and the editor's cell methods taking a `&Screen` in place of a
  `ViewTop` and a preedit;
- `TextView` holding the screen in place of `scroll`;
- `Window::drawn_top` removed.

## Changes to accepted ADRs

The passages below stay in their ADR as written; what holds now follows each. ADR 0100's status
becomes `Accepted, amended by 0102, 0103 and 0104`, and ADR 0103's and ADR 0105's
`Accepted, amended by 0104`.

- [ADR 0100](0100-soft-wrap.md), "Rows are kept as ADR 0051 says: by their place on the screen,
  compared by content": by their logical line and start column, compared by content.
- ADR 0100, "A range that goes on past a view row fills to that row's last cell, the mark's
  column included when a hanging space is in the range": to the cell before the mark's column,
  the row's text width, and the mark's column with a hanging space.
- ADR 0100, "`WrapMap` itself is `pub(crate)`, held only by `Window`": and by the editor for
  the `C-x b` preview's buffer, which no window shows.
- ADR 0100, "If the budget runs out before the view is full, the rest of the view shows its
  logical lines unwrapped, cut at the edge, until the report": and a row wider than the wrap's
  width and the mark's column, as the part of a line before an anchor, draws as one, with no
  mark.
- ADR 0100, "Below 8 text columns, nothing wraps": below 10.
- ADR 0100, "The view follows the cursor's view row": while composing, the caret's view row with
  the preedit laid out in its line.
- [ADR 0103](0103-soft-wrap-in-a-window.md), "Until the renderer draws view rows, a frame draws
  logical lines from the one that holds the top, and `Window::drawn_top` says which": a frame
  draws view rows, and `Window::drawn_top` is gone.
- ADR 0103, "`text.rows` stay logical lines until the renderer's step": they are view rows.
- ADR 0103, "`toggle-truncate-lines` is on `M-x` until the renderer draws view rows": it is on
  `C-x x t`.
- [ADR 0105](0105-m2-24-part-2-the-flash-and-line-numbers.md), "M2-25 keeps the gutter from
  narrowing while the buffer is edited. That is deferred to M2-25", and "refilling every wrap
  map when the gutter's width changes, are left to M2-25 step 4": the gutter keeps its width
  while its buffer is edited, and a width it gains in a command narrows the text area within
  the command's budget.
- ADR 0103's public API, "in `oso-render`, `TextView::scroll` as a `ViewTop`", "`frame`'s cell
  functions … taking a `ViewTop`" and `Window::drawn_top`: as listed above.

## Consequences

- A wrapped buffer draws its view rows, with the mark, and every part of a frame agrees on them.
- Building the rows in view costs a seek and a conversion a row, once a frame.
- Rows wrapped above the view, by a report or near the view, shape nothing again.
- A preedit at a row's end wraps as it will once committed, and the view follows its caret.
- The input method places its candidate window by the cursor's cell, even for characters of a
  preedit on a later row.
- The preview keeps a wrap of its own while a wrapped buffer is previewed.

## Alternatives considered

- **Shaped rows kept by their view row:** a report or rows wrapped near the view above the top
  change every number, and shape the screen again.
- **Kept by their place on the screen:** every scroll shapes the screen again.
- **A range past a break filled only to its last cell of text:** a region over a wrap would look
  ragged where a wide character moved to the next row.
- **The part of a line before an anchor cut at the wrap's width, with the mark,** as first
  chosen: text past the width would show on no row while the mark said the row went on.
- **A row wrapped at an old width cut at the new one:** the same.
- **Narrowing a window resets its rows to one a logical line:** a resize would lose the rows
  already wrapped near the view.
- **8 text columns, as ADR 0100 said:** the widest cluster would reach the mark's column.
- **Only the cursor's row followed while composing:** a preedit that wraps past the last row
  would compose out of view.
- **Each character's row for the input method now:** changes `Editor::preedit_layout` and the
  native layer's input method, for a candidate window a row off.
- **The gutter built in this step:** the visual aids' own step builds it first.
