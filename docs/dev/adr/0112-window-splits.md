# 0112. Window splits

- Status: Accepted
- Date: 2026-11-12

## Context

The M3 roadmap lists window splits. Today a frame shows one tiled window, and above it the floats
of a prompt or completion ([ADR 0008](0008-floating-ui.md), [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md),
[ADR 0076](0076-completion-and-m-x.md)) and the echo area below it
([ADR 0026](0026-m2-ui-layout.md), [ADR 0057](0057-frame-layout.md)). Several decisions were made
with splits in mind and left for this step:

- [ADR 0026](0026-m2-ui-layout.md): a modeline per window, the active one distinct.
- [ADR 0057](0057-frame-layout.md): "Splits divide its part of the frame, and each keeps a text
  area and a modeline", and `cursor_area` must look up the selected window's regions.
- [display-model.md](../display-model.md): a frame holds a tree of split windows, each with its
  own selection set.
- [ADR 0063](0063-modeline-echo-area-and-internal-buffers.md): a `Status` per window.
- [ADR 0053](0053-edit-core.md): a window that is not selected can keep a scroll position past the end
  of the text.
- [ADR 0100](0100-soft-wrap.md) and [ADR 0103](0103-soft-wrap-in-a-window.md): each window wraps
  at its own width, and a copy of a window gets a wrap generation and a job of its own.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md): splits come with a reach for each command.
- [ADR 0088](0088-switching-and-killing-buffers.md): a buffer a window never showed starts where
  any window last left it, "one window until M3".

The code is half ready:

- `Editor` keeps `windows: Vec<Window>` and `selected: usize`. Prompts push windows after the tiled
  one, and `Editor::tiled` is the window beneath them.
- Edits, undo and loads already move the cursors and the top of every window that shows the buffer
  (`follow.rs`, `wraps.rs`), and `kill_buffer` already visits every window.
- `Window` is `Clone`, and a clone drops its wrap.
- Much else reads "the" window: `frame::layout` gives one `WindowRegions`; the gutter's floor, the
  way back of `C-h e`, the band, the flash's sample, isearch's cells, the region's cells,
  `status_into`, the wheel and the IME's rectangle all use `tiled()` or `frame.window`. Some state
  names a window by index (`Marking`, `LastYank`, `ViewSample`), which a deleted window would
  shift.
- The app draws four fixed panes (`PaneId` 0, 1, 2 and 13 for the gutter), and the renderer keeps
  one cache of shaped rows per pane id.
- The dev socket and the agent channel list every tiled window, but describe the rows, the cursor
  and the region of `tiled()` only.
- The restart handoff refuses a state with more than one window.
- The platform's wheel event carries no position, and there is no mouse click at all.

## Decision

The author chose the recommended option on each question on 2026-11-12, through the
coordinator, and asked to revisit none of the points decided without a question. Each point
marked (question N) was one of those choices, listed under [Questions](#questions).

### The window tree

- **A frame holds a tree.** A leaf is a window. A node is a *run* of two or more children laid
  out the same way: side by side (`Across`) or stacked (`Down`). A run never has a child that is a
  run of the same way; such a child is merged into it.
- **Each child of a run keeps a weight,** not a size in cells (question 1). `frame::layout` turns
  the weights into whole cells with the largest remainder, so the cells add up to the run's. A
  frame that is resized keeps the proportions, and resizing back gives the same cells.
- **Splitting a window** in the way of its run adds a sibling right after it. Otherwise the window
  becomes a run of two. Either way the two halves share the window's weight, the new one taking the
  smaller half when the cells are odd. Other windows do not change.
- **Deleting a window** gives its weight to its sibling before it, or after it when it is the first.
  A run left with one child is replaced by that child, merged into its parent if they run the same
  way.
- **Smallest window (question 1):**
  - A split is refused when either half would have fewer than 2 text rows above its modeline, or
    fewer than 10 text columns, ADR 0104's narrowest wrap, beside the narrowest gutter: 4
    columns while line numbers are on, none while they are off. It says "Window too small for
    splitting". Resizing stops at the same size.
  - A window's own gutter never counts there. It grows with its buffer's line count, which a
    buffer that may hold secrets in another window must not give away through where a split or
    a resize stops (ADR 0097). A buffer of 1000 lines or more has a wider gutter, so a window
    of it at the smallest size keeps fewer than 10 text columns, and does not wrap until it is
    wider (ADR 0104).
  - When the frame shrinks, every window keeps at least 1 text row and its modeline, and 1 text
    column. Its gutter goes first, as ADR 0097 has it for a narrow frame.
  - When even that does not fit, the windows that do not fit get no cells and are not drawn, those
    last in the tree first and the selected one never. They come back as the frame grows.
    `other-window` skips them.
- **A frame of one window lays out as today,** small frames included (ADR 0057).
- **Windows have ids.** `WindowId` is never reused within a session, so a deleted window cannot
  be mistaken for another. State that names a window (`Marking`, `LastYank`, the flash's sample,
  a prompt's window beneath) names it by id. The tree holds ids; `Editor` keeps the windows.
  Prompts' windows also get ids, and stay out of the tree.

### Regions

- `FrameLayout` gives a `WindowRegions` for each window in the tree, in tree order (left to right,
  top to bottom), and the echo area. It loses `Copy`, and its storage is reused across frames.
- **Each window keeps its own text area, gutter and modeline** (ADR 0057). Its modeline is its
  bottom row, across its gutter and text.
- **Side by side windows are parted by a divider (question 4):** one column at the right of each
  window that has a window to its right, taken from that window's cells. It draws a vertical line
  one pixel wide at the middle of the column, in a new `Face::Divider`, the full height of the
  window, its modeline included. A window that is the right-most has none.
- **Stacked windows need no divider:** the modeline above parts them.
- **The echo area stays one row across the frame.**
- **The tiled area** is everything above the echo area. Floats are placed over it (below).

### Commands and keys

The new commands are in `crates/oso/src/commands/windows.rs`, bound in `bindings/windows.rs`
(question 2):

| Keys | Command | What it does |
|---|---|---|
| `C-x 2` | `split-window-below` | splits the selected window into two stacked windows |
| `C-x 3` | `split-window-right` | splits it into two side by side |
| `C-x 0` | `delete-window` | deletes the selected window |
| `C-x 1` | `delete-other-windows` | deletes every other window |
| `C-x o` | `other-window` | selects the next window; a count goes that many, a negative one back |
| `C-x +` | `balance-windows` | gives every child of every run the same weight |
| `C-x ^` | `enlarge-window` | makes the window a count of rows taller; negative is shorter |
| `C-x }` | `enlarge-window-horizontally` | a count of columns wider |
| `C-x {` | `shrink-window-horizontally` | a count of columns narrower |
| none | `shrink-window` | a count of rows shorter |
| none | `split-window-below-and-select`, `split-window-right-and-select` | split, then select the new window |
| none | `windmove-left`, `windmove-right`, `windmove-up`, `windmove-down` | select the window in that direction |

- **A split keeps the original window selected (question 3),** as Emacs's do. The new one is below
  or to the right. `…-and-select` select the new one, for people who bind them in place of
  `C-x 2` and `C-x 3` in their configuration ([ADR 0108](0108-the-user-configuration-file.md)).
- **What the new window starts with (question 5):** the same buffer, the primary cursor and its
  goal, the mark, the mark ring and whether the region shows, the scroll, the places
  ([ADR 0088](0088-switching-and-killing-buffers.md)) and the way back of `C-h e`. Not the other
  cursors. Its wrap is made at its own width with a generation and a job of its own (ADR 0103).
- **`other-window` goes through the tree's order** and wraps around. `windmove-*` select the
  window that touches the selected one on that side at the row or column of its primary cursor,
  or say "No window left from selected window", as Emacs does.
- **`delete-window` selects the window selected most recently before it.** On the only window it
  says "Attempt to delete the sole window". `delete-other-windows` on the only window does
  nothing.
- **Resizing takes cells from the neighbours on that side of the window,** nearest first, none below
  the smallest window. When nothing can give, it says "Cannot enlarge the window" or "Cannot
  shrink the window". Resizing sets weights from the cells laid out, so it stays in proportion
  after a frame resize.
- **Every window command refuses inside a prompt,** "Not in the minibuffer"
  (`commands::not_in_prompt`, ADR 0072), and ends isearch as any command not its own does.
- **Selecting another window flashes,** as ADR 0097 has it.
- `C-x 4` (`switch-to-buffer-other-window`, `find-file-other-window`, `kill-buffer-and-window`),
  layout undo as Emacs's `winner-mode`, repeating `C-x o` with `o` alone, and rules for where a
  buffer is shown (Emacs's `display-buffer`) are left for later and recorded in
  `open-questions.md`. `C-h e` and `C-h r` keep showing their buffer in the selected window.

### Each window over a shared buffer

- **A window keeps its own** buffer, selection set (every cursor, mark and the mark ring), scroll,
  view size, places, wrap, gutter width floor and the way back of `C-h e`. These move into
  `Window` from `Editor` where they are still single (`gutter_floor`, `way_back`).
- **The buffer keeps** its text, display map, undo history, revision and the wrap setting, as
  today. `toggle-truncate-lines` acts on the selected window's buffer, so every window that shows
  it wraps or stops, each at its own width (ADR 0103).
- **An edit in one window moves the others' cursors and tops as markers,** as `follow.rs` already
  does. Undo restores cursors only in the window that undoes (ADR 0053).
- **Every window keeps its primary cursor in view.** The selected one follows it in the command,
  as today. Another window whose cursor an edit moved out of its view, or whose top an edit moved
  past the end of the text, follows its cursor when the command ends. This settles ADR 0053's
  scroll past the end.
- **A buffer a window never showed** starts where the window selected most recently that showed
  it left it, or its start (ADR 0088).
- **Killing a buffer** sends each window that showed it back through its own places, as today.
  No window is deleted.
- **A prompt's window beneath is the selected tiled window.** `Editor::tiled` keeps its meaning:
  the selected window, or the one beneath the prompts.
- **isearch runs in the selected window,** and its matches are drawn there only.
- **The `C-x b` preview reads the selected window's places,** as it reads the window's today.

### What each window draws (question 6)

- **The selected window** draws as the one window does today.
- **A window that is not selected,** as in Emacs:
  - its cursors are hollow, the secondary ones dimmer, as beneath a prompt (ADR 0072);
  - no region, no current-line band, no isearch matches and no flash;
  - whitespace marks, the trailing fill, line numbers and wrap marks, as in the selected one;
  - its modeline in a new `Face::ModelineInactive`, with the same segments about its own buffer and
    cursor.
- **Segments about the frame,** such as the agent channel's `Agent` (ADR 0085), show only in the
  selected window's modeline.
- **Beneath a prompt every tiled window is drawn as not selected.**
- **The gutter is per window:** its width comes from its own buffer's line count and its own
  floor, so two windows can have gutters of different widths (ADR 0097, ADR 0104).
- **The new faces** join ADR 0097's tiers: `ModelineInactive`'s text keeps 4.5:1 on its
  background, and `Divider` keeps 3:1 on the background, in every theme
  ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)).

### The minibuffer and floats (question 7)

- **Floats are placed over the tiled area, not over a window.** A prompt's panel hangs from the
  tiled area's top edge, 3/5 of its columns (ADR 0075), and completion centres on it, 62% of its
  columns and 19 rows (ADR 0076). So they stay where they are whatever the layout, and a completion
  float has its rows also over a short window.
- **They may cover any window's text and modeline,** as they cover the one window's today. The
  panel's shadow and hairline stay inside the tiled area, and never reach the echo row.

### Secret buffers, the input method and secure input (questions 8 and 9)

- **"A buffer that may hold secrets is shown" means in any window,** as `Editor::shows_secret`
  already checks. So with such a buffer in any window, as today with it in the one:
  - the dev socket withholds its fields and refuses `screenshot`, `check-handover` and
    `export-state` (ADR 0044, ADR 0079);
  - keys in progress, a composition and timings are left out (ADR 0060, ADR 0079);
  - a prompt opened then gives no label or answer, and keeps no history (ADR 0079);
  - **the agent channel refuses `run`, `open` and `insert` (question 9)** and answers `state`
    with that buffer's fields withheld (ADR 0085).
- **The input method and secure input follow the selected window's buffer (question 8),** as
  they do today: keys go only there. So with `.env` in one window and a source file selected in
  the other, Japanese input works, and secure input is off; selecting the `.env` window turns the
  input method off and secure input on in the same hook after the event (ADR 0069, ADR 0073).
  An open composition is ended before another window is selected, as before a buffer change.

### The agent channel and the dev socket

- **Reach:** every window command is `Look` (question 10). None edits text or a file, and the
  person can undo any of them with keys.
- **The channel's `state`** keeps its version 1, as the new fields are compatible (ADR 0085):
  - each window gains `id` and `area`, its cells as `row`, `column`, `rows` and `columns`, gutter,
    text and modeline together, so a secret buffer's gutter still says nothing of its lines
    (ADR 0097);
  - `windows` lists the tiled windows in tree order;
  - the rows, the region of `text` and `open` are the selected tiled window's, as they are the one
    window's today.
- **The dev socket's `get-state`** gives each window `id` and `area` too. Its top-level `window`,
  `text`, `cursor` and `scroll` describe the selected tiled window. `get-text` reads it.
- **Text-grid snapshots** ([ADR 0045](0045-text-grid-snapshots.md)) compose every window. A
  divider column is `┃`; inactive modelines are between `-` in place of `=`.

### The restart handoff

- **The handoff's state goes to version 4.** It carries every tiled window, as it carries the one
  today (ADR 0090): buffer, cursors with goals, marks, the mark ring, whether the region shows,
  and the scroll; and now:
  - `layout`: the tree, as nested `{"across": [...]}` or `{"down": [...]}` of
    `{"weight": n, "window": i}` or `{"weight": n, "run": {...}}`, at most 16 deep;
  - `selected`: the index of the selected window, as today.
- **A reader takes version 4 and version 3** (ADR 0090), a version 3 state as one window.
- Places stay out of the handoff, as ADR 0088 has it. Each window's internal buffer falls back
  as the one window's does (ADR 0063, ADR 0079).
- A state that does not fit its line, with many windows of many cursors, is refused as one too
  large is today, and the restart waits.

### Mouse focus, later in M3

Clicking and dragging come later in M3. This step leaves room for them:

- **`FrameLayout::hit(row, column)`** says which window a cell is in, and whether it is its text,
  gutter, modeline or divider. A click will select through it, and a drag on a divider or a
  modeline will resize through the same weights.
- **The wheel scrolls the window under the pointer, without selecting it (question 11),** as
  Emacs's `mouse-wheel-follow-mouse` does. The platform's wheel event gains the pointer's
  position, in physical pixels, from `NSEvent`'s location on the native layer and winit's last
  cursor position; the app turns it into a cell with `PixelGrid`. Beneath a prompt it scrolls the
  window under the pointer as it scrolls the one beneath today (ADR 0072).

### Cost

The budgets ([roadmap](../roadmap.md)) are a key's p99 to photon ≤ 8 ms and no freeze on a 1 GB
file or a 10 MB line.

- **Building the rows in view** (ADR 0104) costs a seek and a conversion a row for each window.
  The windows' rows add up to at most the frame's, so N windows cost about one full-frame window
  and N seeks.
- **The modelines:** a `Status` per window, built and compared after each batch of events
  (ADR 0063), a few microseconds each.
- **The renderer:** three panes a window (text, gutter, modeline). Panes in one layer draw
  together, so the draw calls do not grow. Each pane id keeps its shaped rows, and pane ids come
  from the window's id, so a window keeps its rows when another is deleted, and a deleted window's
  are dropped.
  - A split, a delete, a resize or a balance changes widths, so the rows in view are shaped again
    once, as after a frame resize.
  - Two windows on one buffer keep a cache each. An edit reshapes its rows in each window that
    shows them, about twice one window's cost.
- **Wrapping:** each wrapped window keeps its own `WrapMap` (ADR 0103).
  - An edit's splices are followed by every window that wraps the buffer. **All windows share
    the command's budget** of 128 KiB (ADR 0103), the selected window first; past it the other
    windows' rows near their views stay provisional until their jobs report.
  - The jobs queue on the one `oso-wrap` thread, so a second window wrapping a 10 MB line doubles
    the background time, about 110 ms more, and costs the UI thread nothing.
  - Each wrap holds its own rows: two windows wrapping 100 MB of prose at different widths hold
    two trees.
- **No cap on the number of windows** beyond the smallest window: a 400 by 120 cell frame holds about
  a thousand. The channel's 30 requests a second bound how fast an agent can make them.
- **Measured before the steps merge:** `just perf` gains a key's latency with four windows (two
  on one buffer, one of them wrapped) against one, which must stay within the budget, and a frame
  with 16 windows, reported only.

### Code

- **`oso-core`:**
  - `window::WindowId`;
  - `frame::Splits`, the tree, with its splits, deletes, resizes and balance as pure functions of
    the tree and the cells, tested without an editor; `frame::Way` (`Across`, `Down`);
  - `frame::layout` taking the tree; `FrameLayout::windows` and `FrameLayout::hit`;
    `WindowRegions::divider`;
  - `Editor`'s window methods: `split_window`, `delete_window`, `delete_other_windows`,
    `select_window`, `other_window`, `windmove`, `resize_window` and `balance_windows`, with
    `frame::Side` and the refusals `frame::TooSmall`, `editor::SoleWindow`,
    `editor::NoWindow` and `editor::CannotResize`; `screen_of`, and `status_into`,
    `gutter_into` and the cell functions taking a window's id. The tree's order is
    `FrameLayout::windows`;
  - no new public module: `Splits` and `Way` go in `frame`, `WindowId` in `window`.
- **As built, step 2:** the window commands were registered only in tests until step 3 drew
  every window and bound their keys, so the app never showed a frame with windows it did not
  draw. `windmove-up` and `windmove-down` say "No window up from selected window" and "…down…",
  as Emacs's `windmove` does.
- **`oso-render`:** `Face::ModelineInactive` and `Face::Divider`, and `PaneContent::Divider`, a
  line one pixel wide down the middle of its column.
- **As built, step 3:** each window's panes take ids from its `WindowId` (`WindowId::get`): 16
  and up, four a window, below them the echo area's and the floats'. A composition ends when
  another tiled window is selected, whatever selected it, and stays when a prompt opens over
  it (ADR 0077).
- **`Agent` stays in sight (the author's choice, 2026-11-12):** a prompt's float can cover the
  selected window's modeline, which then sits mid-frame. While one does, the bottom-most
  window's modeline, the left-most on the bottom row, says `Agent` too (ADR 0085).
- **`oso-platform`:** the wheel event's position.
- **`oso`:** the commands and bindings, panes per window, the sockets, the handoff's version 4 and
  the wheel.

## Questions

Put to the author through the coordinator, the recommended option first. The author chose (a) on
each.

1. **How sizes are kept, and the smallest window.**
   - (a) A weight for each child, turned into cells at each layout; a split refused below 2 text
     rows or 10 text columns; at least 1 row and 1 column when the frame shrinks. A frame resize
     keeps the proportions exactly, and resizing back gives the same cells.
   - (b) Cells, with a frame resize spreading the difference over the windows, as Emacs does.
     Sizes set by hand stay exact, but rounding drifts over many resizes.
   - (c) As (a), with Emacs's smallest window: 4 rows with the modeline, 10 columns. Fewer windows
     fit in a split, and a 4-row window shows 3 rows of text.
2. **Which commands come now.**
   - (a) `C-x 2`, `3`, `0`, `1`, `o`, `+`, `^`, `}`, `{`, `shrink-window`, the `…-and-select`
     splits and `windmove-*` with no keys. `C-x 4`, layout undo and repeating `C-x o` later.
   - (b) (a) with `C-x 4 b`, `C-x 4 C-f` and `C-x 4 0`, which need where a buffer is shown to be
     decided now.
   - (c) Only `C-x 2`, `3`, `0`, `1` and `o`; resizing and balance in a later step.
3. **Which window is selected after `C-x 2` and `C-x 3`.**
   - (a) The original, as in Emacs, with `split-window-below-and-select` and
     `split-window-right-and-select` to bind in the configuration file.
   - (b) The new one, by default. Differs from Emacs's keys; the original is one `C-x o` away.
4. **How side by side windows are parted.**
   - (a) A column at the right of the left window, with a one-pixel line in its middle in
     `Face::Divider`. Costs a column of text a window.
   - (b) The same column filled in the inactive modeline's background, as a bar. Easier to see,
     heavier.
   - (c) No column, only each window's modeline. Saves the column, but text of two windows can
     meet with no gap.
5. **What a new window starts with.**
   - (a) The primary cursor, its mark and the mark ring, the region's state, the scroll, the places
     and the way back of `C-h e`; not the other cursors. An edit at a thousand cursors moves only
     one cursor in the new window.
   - (b) A copy of everything, every cursor included. Both windows then follow every edit with all
     their cursors.
   - (c) The primary cursor and the scroll only, with no mark or places. Simpler, but `C-x b` in the
     new window has no order of its own.
6. **What a window that is not selected draws.**
   - (a) As in Emacs: hollow cursors, no region, band, isearch matches or flash, an inactive
     modeline, and frame segments such as `Agent` only on the selected modeline.
   - (b) Everything as the selected one, but hollow cursors and an inactive modeline. Regions in
     two windows on one buffer then look alike.
7. **Where the minibuffer and completion float in a split frame.**
   - (a) Over the tiled area, as over the one window today. They never move with the layout, and
     completion keeps its 19 rows over a short window.
   - (b) Over the selected window. Closer to where the person looks, but a short or narrow window
     gives completion few rows, and floats jump when another window is selected.
8. **The input method and secure input with a secret buffer in a window not selected.**
   - (a) They follow the selected window's buffer, as today: keys go only there.
   - (b) The input method off and secure input on while any window shows one. Safer against a key
     sent to the wrong window, but no Japanese input anywhere while `.env` is open beside.
9. **The agent channel while a window not selected shows a secret buffer.**
   - (a) As today: `run`, `open` and `insert` refused while any window shows one. Safe, but the
     agent waits for as long as the person keeps such a buffer open beside.
   - (b) Refused only while the selected window shows one, with every command that would select
     that window or show that buffer refused too. ADR 0085 rejected this per-command check; with
     splits each new command would need it.
10. **The reach of the window commands.**
    - (a) All `Look`: an agent can split, select, resize and delete windows. Deleting a window
      loses no text.
    - (b) Splitting, selecting and resizing `Look`; `delete-window` and `delete-other-windows`
      `Person`, so an agent cannot close the person's layout.
11. **The wheel in a split frame.**
    - (a) Scrolls the window under the pointer without selecting it, as Emacs does. The platform's
      wheel event gains a position, in this work.
    - (b) Scrolls the selected window until mouse focus comes later in M3. No platform change now,
      but the wheel over another window scrolls the wrong one.

## Changes to accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0057](0057-frame-layout.md):
  - "`window` is the one tiled window until splits arrive (M3)": `FrameLayout::windows` gives
    every window's regions, and the layout takes the window tree.
  - "`Editor::cursor_area` and `text_view` pair the selected window with `FrameLayout::window`":
    with the selected window's own regions.
  - A side by side window gives a column to a divider.
- [ADR 0063](0063-modeline-echo-area-and-internal-buffers.md): a `Status` per window, and frame
  segments in the selected window's modeline only.
- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md):
  - "There is one tiled window until M3": the window beneath is the selected tiled window, and
    every tiled window is drawn as not selected beneath a prompt.
  - "The wheel scrolls the window beneath": the window under the pointer.
- [ADR 0075](0075-a-float-as-a-panel-attached-to-the-top-edge.md), the panel "hangs from the top
  edge of the text area" and its region "is the text area of the window beneath": of the tiled
  area.
- [ADR 0076](0076-completion-and-m-x.md), "At the center of the frame's text area" and "62% of the
  text area's columns": of the tiled area.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md): `state`'s windows gain `id` and `area`.
- [ADR 0088](0088-switching-and-killing-buffers.md), "A buffer the window never showed starts where
  any window last left it (one window until M3)": where the window selected most recently that
  showed it left it.
- [ADR 0090](0090-oso-dev-carries-unsaved-text.md) and [ADR 0103](0103-soft-wrap-in-a-window.md):
  the handoff is version 4, with every window and the layout, and reads version 3.
- [ADR 0097](0097-visual-aids.md):
  - "The band: on the primary cursor's row of the tiled window": of the selected window only.
  - "Floats keep their place: they centre on the window's whole area": on the tiled area.
- [ADR 0053](0053-edit-core.md), "A window that is not selected can keep a scroll position past the end
  of the text. Splits (M3) must deal with it": it follows its cursor when the command ends.

## Consequences

- The person can split, select, resize and delete windows with Emacs's keys, and an agent can too.
- Each window keeps its own place in a shared buffer, and a buffer shown twice costs about twice
  the shaping of its edited rows and, wrapped, two wraps.
- A side by side window loses a column to its divider.
- While a secret buffer is open in any window the agent channel waits, as today with it in the one
  window.
- Floats stay where they are whatever the layout.
- The handoff carries the layout.
- The wheel reaches the window under the pointer; clicks and divider drags come with mouse focus.
- `FrameLayout` stops being `Copy`.
- `oso-core` gains `WindowId`, `Splits`, `Way` and window methods on `Editor`, in existing modules.

## Alternatives considered

- **A binary tree, each node split in two:** simpler to build, but three windows side by side
  nest, and balance and resize no longer treat them as one run, as Emacs does.
- **Windows by index, as today:** deleting one shifts the rest, and every state that names a
  window has to be fixed up.
- **A tiling layout that places windows itself:** not Emacs, and the person loses control of
  sizes.
- **One shaped-row cache per buffer, shared by its windows:** an edit in a buffer shown twice
  would shape once, but rows depend on each window's width and left column, and the cache's
  eviction would follow two views. Left until measuring says it is needed.
- **A wrap budget per window:** each window wraps near its view at once, but a key with four
  wrapped windows could cost four budgets.
- **Splits inside a prompt or float:** floats are not in the tree, and a prompt is modal.
- **Deleting the windows that show a killed buffer:** Emacs keeps them, and a window is the
  person's layout.
