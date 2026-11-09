# 0103. Soft wrap in a window

- Status: Proposed
- Date: 2026-11-10

## Context

[ADR 0100](0100-soft-wrap.md) decides soft wrap, and [ADR 0102](0102-soft-wrap-what-building-it-changed.md)
records what building its WrapMap, budget and worker changed (PR #129). The next step puts the
WrapMap in the window:

- the top as an offset;
- the window's wrap, and the buffer's setting;
- `toggle-truncate-lines`;
- the job's thread;
- every edit, undo, reload and load kept in step.

The renderer, the frame's cells, isearch's cells and the input method read view rows in the step
after.

ADR 0100 left some of this open, and building it changed some:

- **Before a report arrives,** ADR 0100 wraps rows forward from an anchor and keeps a
  provisional row count for what is not yet wrapped. That count would have to be kept for every
  line, and only scrolling near the view reads it.
- **The top** "snaps back to the start of its view row after every rewrap, edit, reload or
  report". A provisional row is a whole logical line, so a top snapped to one goes back to its
  line's start: a window that comes back to a place 5 MB into a line would show the line's first
  row.
- **Loads and reloads** hand the buffer a whole new text, with no splice a wrap could follow.
- **Each WrapMap's job generations start at 0,** so one window's report could pass another's
  check.

## Decision

The author chose each point marked (author) on 2026-11-10, through the coordinator, and approved
the public API in outline.

### The top

- **A window keeps its top as an offset and finds its row when it reads it.** It shows from the
  start of the view row that holds the offset. Nothing writes the offset back to that start, but
  scrolling, which sets it to a row's start.
  - So a rewrap, a report or a provisional row never moves what the window keeps.
  - The first row in view still always starts where a row starts.
- **It follows edits as a marker that stays before an insertion at it, wrapped or not.** In a
  buffer that does not wrap, an edit above the view no longer moves the text in view down, as
  Emacs's `window-start` does not (author).
- **`Scroll` holds `start`, the offset, and `left`.** The field is renamed from `top`, so that
  every reader of the old row had to be visited (the coordinator chose the name).
  - A place, isearch's view for `C-g`, `restore_positions` and the handoff keep a `Scroll`.
  - The rules of scrolling and the frame's cells count rows in a `ViewTop`: a row and a left
    column.
  - Until the renderer draws view rows, a frame draws logical lines from the one that holds the
    top, and `Window::drawn_top` says which.

### Before a report arrives

- **No provisional row count (author).** Until a job reports, every row it has not wrapped counts
  as one row a logical line, as a provisional row does.
- **The rows near the view are wrapped on the UI thread,** where they are provisional and not yet
  wrapped there:
  - the cursor's row;
  - the rows in view and a screen below them;
  - the rows a command or the wheel scrolls to.
- **Wrapping goes forward from an anchor:**
  - the start of the row that holds what is wanted, if it is at most 64 KiB back;
  - otherwise past a run of spaces near that point;
  - otherwise a cluster start there.
  It joins the old rows at the next boundary past them, with one provisional row between, which
  is left stale for the job.
- **The rows wrapped there stay provisional for the job,** since a row wrapped from an anchor is
  real only from a real break on, but they are not wrapped again before the report.
- **It shares the command's budget with the command's edits (author),** 128 KiB and 2 KiB a call
  (ADR 0102).
  - A command, a wheel scroll, a resize and a report each start a budget.
  - After a thousand cursors spend it, the view can stay provisional until the report, as ADR 0102
    already says.

### The window and the buffer

- **The buffer holds the setting, and each tiled window shows it at its own width.** A prompt's
  window never wraps.
- **A window drops its wrap when it shows another buffer, and makes it again when it comes back
  (author).** Its place keeps the top's offset.
- **A change that no splice led to starts the wrap over:**
  - a load's part;
  - a reload, which brings a display map of its own;
  - an emergency copy put back;
  - any change the window did not follow, which it finds by the buffer's revision.

  Starting over makes the rows provisional, wraps the rows near the view and wants a job for the
  whole buffer. A load's next part supersedes that job, so a wrapped buffer that loads is wrapped
  in full once its last part is in.
- **Edits, undo and redo, `*Messages*` and recovery hand each window's wrap their splices,** each
  with the display before and after it. The splices are collected only while the buffer wraps.
- **Each window follows them on a clone of its wrap,** put in with the buffer's text in ADR 0053's
  step, which cannot panic.

### The jobs

- **A job holds a weak reference to its map's generation, and a report names its map.**
  - A map takes only its own reports, and a clone of a map counts as the same map.
  - A job whose map is gone stops at its next check: after a toggle off, or when its window shows
    another buffer.
- **The app runs the jobs on one thread, `oso-wrap`,** as isearch's run on theirs. A job that
  panics leaves its window's rows provisional, and the thread goes on.

### The command

- **`toggle-truncate-lines` is on `M-x` until the renderer draws view rows,** and `C-x x t` comes
  with that step (author). Until then a wrapped buffer draws its logical lines, cut at the edge,
  while scrolling counts view rows.
- It acts on the tiled window's buffer, and says "Truncate long lines enabled" or "… disabled" in
  the echo area and `*Messages*`, as Emacs does.

### The sockets and the handoff

- **The agent channel's state** gains `wrapped` for each buffer. `first_row` and the rows stay
  logical lines. While a buffer that may hold secrets is shown, its `wrapped` is withheld, as a
  person toggles it there (ADR 0079). The state's version stays 1: a new field is a compatible change, and its version
  goes up only for incompatible ones (ADR 0085).
- **The dev socket's state** gives the top's offset as `scroll.start` and its view row as
  `scroll.top`, and `wrapped` for each buffer. The offset and the row are withheld for a buffer
  that may hold secrets, as its other positions are, and its `wrapped` while one is shown. `text.rows` stay logical lines until the
  renderer's step.
- **The handoff's state is version 3:**
  - the top is an offset, `scroll.start`;
  - a buffer that wraps says `"wraps": true`.

  It still reads versions 1 and 2, whose top is a row, as the start of that logical line. A row
  top still waiting for its file is exported as `scroll.top`, a row, so that a second restart
  reads it right. An imported top goes to the start of the cluster it is in.
- **`Debug` on a `Window` prints no offsets** (ADR 0100).

### The public API

The items are listed in the pull request and the API snapshot:

- in `window`:
  - `Scroll::start`;
  - `ViewTop`;
  - `Window::wrap`, `row_count`, `top_row`, `cursor_row` and `drawn_top`;
- in `editor`:
  - `Editor::set_wraps`, `take_wrap_jobs`, `take_wrap_report`, `loaded`, `top_row`,
    `scroll_lines` and `recenter`;
  - `Preview::Buffer`'s `start`;
- `Buffer::wraps`;
- in `oso-render`, `TextView::scroll` as a `ViewTop`;
- `frame`'s cell functions and `window::follow` taking a `ViewTop`.

## Changes to accepted ADRs

The passages below stay in their ADR as written; what holds now follows each. ADR 0100's status
becomes `Accepted, amended by 0102 and 0103`.

- [ADR 0100](0100-soft-wrap.md), "It snaps back to the start of its view row after every rewrap,
  edit, reload or report, so the first row in view always starts where a row starts": the
  window shows from the start of the row that holds it, and keeps the offset as it is.
- ADR 0100, "When a report arrives, the top snaps to the start of the view row that holds the
  top's offset": the view shows from that row's start, and the offset stays.
- ADR 0100, "A provisional row count covers what is not yet wrapped … Scroll bounds, `M->`,
  `C-v`'s end check and the wheel's stop read it": none is kept. Until the report, a logical
  line not yet wrapped counts one row, and the rows near the view, which those read, are wrapped
  at once.
- ADR 0100, "Rows before the top come from the same anchor": and from the start of the row `M-v`
  or the wheel goes to.
- ADR 0100, "The rows in view are wrapped on the UI thread from the top, within the view's rows
  and the byte budget": within what is left of the command's budget, which its edits share.
- ADR 0100, "A resize during a load: the part jobs at the old width are dropped. One job wraps
  what has loaded at the new width, and the parts follow in order": each part starts the wrap
  over, and the last part's job wraps it all.
- ADR 0100, "`toggle-truncate-lines` … on `C-x x t`": on `M-x` until the renderer draws view rows.
- ADR 0100, "each buffer gains `wrapped`, a new field in the versioned schema": with the state's
  version unchanged, as the field is compatible.

## Consequences

- A window that comes back to a place deep in a long line shows it at once, and the report
  changes at most where the rows in view break.
- Turning wrapping on in a window on a 10 MB line costs the UI thread about 0.7 ms, and a narrower
  window about 0.2 ms (`text_perf -- wrap`, a MacBook). The job then wraps the line in about
  110 ms.
- Once the report is in, a key at one cursor in a wrapped 10 MB line of words costs about 24 µs at
  p50, and 89 µs in CJK. A thousand cursors add about 2.5 ms to the 3 to 6 ms the edit costs
  truncated.
- Before the report, a key in a long provisional line costs up to the budget, about 2 ms, since
  its rewrap reads into the provisional row. So does a line that repeats one short piece of text,
  whose shifted breaks may never meet the old ones; its rest waits for the worker.
- A wrapped buffer that loads in many parts is wrapped in full only after its last part. Until
  then, the rows near the view are wrapped.
- In unwrapped buffers, an edit above the view, by another cursor or the agent, keeps the text in
  view where it was.
- Until the renderer's step, a wrapped buffer draws its logical lines cut at the edge; that is why
  the toggle waits there for its key.

## Alternatives considered

- **ADR 0100's provisional row count:** only scrolling near the view reads it, where the rows are
  wrapped anyway, and it would have to be kept for every line.
- **Snapping the stored top to its row's start:** a provisional row is a whole line, and a
  realized row from an anchor is not yet a real one, so each snap could lose the place by a row
  or by a whole line.
- **A second budget for the rows near the view:** up to twice a command's cost, two frames at a
  thousand cursors.
- **Keeping each buffer's wrap in the window's place:** tens of MB for each wrapped buffer a
  window once showed.
- **Following a load's parts as splices:** the snapshots a load hands over can skip parts, and
  past 4 MiB of stale ranges a job wraps the whole buffer anyway (ADR 0102).
- **Binding `C-x x t` now:** a wrapped buffer would scroll by view rows while it draws logical
  lines until the next step.
- **Version 2 of the agent channel's state for `wrapped`:** the field is compatible, and an
  agent that does not know it reads the state as before.
