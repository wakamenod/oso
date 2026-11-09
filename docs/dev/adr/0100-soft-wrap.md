# 0100. Soft wrap

- Status: Proposed
- Date: 2026-11-09

## Context

M2-25 brings soft wrap, the last step of M2
([ADR 0051](0051-m2-plan-decisions.md)). Long paragraphs of Japanese prose are hard to read by
scrolling sideways. [ADR 0006](0006-display-model.md) puts a WrapMap above the TabMap. It also
decides that wrapping is off by default, turned on per mode or by a toggle, and that `C-n` and
`C-p` move by logical lines.

What stands in the way today:

- Every reader of a display row takes it for a buffer line: motion, word motion, `edit-lines`, the
  frame's cell rectangles, the renderer's row cache, isearch's highlights and the input method's
  rectangle.
- The display map belongs to the buffer, but the width belongs to the window. Two windows on one
  buffer can be of different widths.
- The budgets hold with wrapping on:
  - key press to photon at p99 ≤ 8 ms ([`roadmap.md`](../roadmap.md));
  - no freeze on a 10 MB line or a 100 MB file;
  - no freeze when the width changes.
- Several ADRs left a point for M2-25:
  - the band and the line numbers of [ADR 0097](0097-visual-aids.md);
  - the cursor flash's jumps in display rows, also ADR 0097;
  - `TAB`'s columns ([ADR 0056](0056-typing.md));
  - `C-a`, `C-e` and `kill-line` on logical lines ([ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md),
    [ADR 0074](0074-the-kill-ring-and-the-clipboard.md));
  - the modeline's line ([ADR 0063](0063-modeline-echo-area-and-internal-buffers.md));
  - a preedit at the right edge ([ADR 0069](0069-macos-input-method.md)).

## Decision

The author chose each point below on 2026-11-09, the recommended option every time except the
first.

### Turning it on

- **Off by default, for every buffer.** `toggle-truncate-lines` turns it on or off for the current
  buffer, on `C-x x t` as in Emacs 28. Nothing turns it on by file name. Modes do that from M3,
  as ADR 0006 says.
- **The setting is the buffer's; the width is each window's.** Two windows on one wrapped buffer
  each wrap at their own width.
- The modeline gives no word for it. Wrapped rows show it.
- The command's reach is `Look` ([ADR 0085](0085-agent-channel-and-oso-mcp.md)): it changes how
  text is shown, not the text.
- `oso dev` carries each buffer's setting to the instance that replaces it
  ([ADR 0047](0047-process-spawning-and-restart-handoff.md)).

### Where a row breaks

- **At the window's text width, less one column,** which holds the continuation mark. A gutter
  takes its own columns beside it ([ADR 0097](0097-visual-aids.md)).
- **Only between display clusters,** so cursors, regions and matches never split one.
- **Text in Latin and other scripts that use spaces breaks after a space.** The spaces at a break
  stay at the end of its row, past the edge if need be, and are not drawn there. A word longer
  than a row breaks at the last cluster that fits.
- **CJK text breaks between any two clusters**, and between CJK and other text.
- **Basic kinsoku**, the core sets of JIS X 4051, as rules of oso's own, with no new dependency:
  - a row starts with none of `、。，．・：；？！ー）］｝〕〉》」』】` or the small kana, nor their
    ASCII counterparts `,.:;?!)]}` after CJK;
  - a row ends with none of `（［｛〔〈《「『【`, nor `([{` before CJK;
  - a break that would break a rule moves back, up to two clusters, so those clusters start the
    next row. Past two it breaks where it would have.
- **A cluster that does not fit** goes to the next row whole: a wide character, a tab or an
  escape box such as `\xNN` ([ADR 0040](0040-buffers-loading-motion-and-scrolling.md)). The cells
  it leaves at the end of its row stay blank. A cluster wider than a whole row gets a row of its
  own and is cut at the edge, as today.
- **A tab keeps the width it has on its logical line.** The TabMap expands tabs before rows wrap,
  so a tab's cells count from the logical line's start, as `TAB` counts them (ADR 0056).

### How wrapped rows look

- **A continuation row starts at the left edge,** with no indent.
- **A faint mark, `↩`, in the last column of each row that continues,** in a new face,
  `Face::WrapMark`, with the line numbers' colour. The text never uses that column.
- **ADR 0097 holds as written:**
  - the current line's band covers every row of the logical line;
  - a logical line's number is on its first row, and the rows it continues on are blank;
  - the flash counts a jump in display rows.
- **Nothing scrolls sideways while a buffer is wrapped.** The window's left column stays 0, and
  the sideways wheel does nothing.

### Motion stays on logical lines

- **`C-n`, `C-p`, `C-a`, `C-e`, `kill-line`, `edit-lines` (`C-x SPC`) and `TAB` work on logical
  lines,** as ADR 0006, 0051, 0056, 0058 and 0074 say. `C-n` keeps its goal column in the logical
  line's display columns. So in a wrapped paragraph, `C-n` goes to the next paragraph.
- **Word motion is unchanged:** the end of a row that wraps is not the end of a word.
- **Scrolling is by display rows.**
  - `C-v` and `M-v` move a screenful of rows, keeping 2 rows of context.
  - `C-l` centres the cursor's row.
  - The view follows the cursor's row, and the wheel moves by rows.
- **The modeline's line and column stay logical** (ADR 0063).

### The WrapMap

- **A window holds a wrap of its buffer at its width,** made from the buffer's display snapshot.
  The display map stays the buffer's, so its `DisplayPoint` keeps meaning a logical line and a
  column, and every reader of it is right as it is.
  - A **view row** is one row on the screen: a logical row and the offset where the view row
    starts. Only what draws or scrolls reads view rows: the renderer, the frame's cell rectangles,
    the view's scroll, isearch's highlights and the input method's rectangle.
  - The pipeline of ADR 0006 holds: the WrapMap is above the TabMap, and the BlockMap goes above
    it later, in the window too.
- **A `SumTree` of wrap points,** where each item is a run of logical rows that do not wrap, or the
  breaks of one row that does. Its summary counts view rows and buffer bytes, so a view row and an
  offset convert both ways in O(log n).
  - A line that fits costs one entry in a run. Whether it fits is found by reading its clusters,
    on the worker or for the rows in view; the TabMap gains no field for it. A line's width in
    the TabMap's summary would need a later ADR.
  - A 10 MB line at 100 columns holds about 100,000 breaks, about 2 MB.
- **A window's top is an offset,** the start of its first view row. A rewrap therefore keeps the
  text in view where it was, and never has to find a row number first.
- **An edit rewraps only near itself.** The wrap is redone from the row before the one that holds
  the edit, until a new break falls where an old one was with the same state. After that every
  break is as before. Typing in a wrapped 10 MB line costs a row or two, not the line.
- **A width change and a load wrap on a worker,** as isearch's and completion's do
  ([ADR 0082](0082-isearch.md), [ADR 0096](0096-completion-on-a-worker-past-5000-candidates.md)):
  - The editor hands out a `WrapJob` for a snapshot and a width, and the app runs it on a thread.
    The report comes back for the snapshot's revision. The edits made since are applied to it as
    above, and then it replaces the window's wrap.
  - Meanwhile, the rows in view are wrapped on the UI thread from the window's top, within the
    view's rows and a bound in bytes. So a width change draws its next frame at once.
  - When the report arrives, the top snaps to the start of the view row that holds it. The text
    in view can move by up to a row, once.
  - Rows past what is wrapped show as they come, on the frame after the report.
  - A load wraps each appended part on the worker, as ADR 0022 loads it. Until that part is done,
    the rows from it in view are wrapped as above.
- **No limit on a line's length or a buffer's size.** A 10 MB line and a 100 MB file are wrapped
  whole, on the worker.
- **A buffer that may hold secrets is wrapped as any other.** The worker reads its text in
  process, keeps no copy past the job, and logs nothing of it ([ADR 0028](0028-secrets-handling.md)).

### The input method on a wrapped row

- **A preedit wraps as if it were inserted:** its logical line is laid out again with the preedit
  in it, from the cursor's view row to the end of the rows in view, on the UI thread. Rows after it
  in view move down. Nothing is rewrapped past the view, and committing makes an ordinary edit.
- The input method's rectangle is the cursor's cell in that layout
  ([ADR 0069](0069-macos-input-method.md)).

### What reads view rows

- **The renderer** draws view rows, and its row cache is keyed by a view row's start offset and
  the buffer's revision, in place of the display row.
- **The frame's cell rectangles:**
  - the cursor's;
  - the other cursors' ([ADR 0091](0091-multiple-cursor-keys-and-m-y.md));
  - the region's ([ADR 0071](0071-mark-and-region.md));
  - isearch's matches (ADR 0082).

  A range that goes on past a view row fills to that row's end, whether a line break or a wrap
  ends it.
- **The dev socket:**
  - `get-state`'s `text.rows` and `get-text` give view rows in view;
  - `scroll` gives the top's offset and view row;
  - each buffer says `wrapped`.
  `just release-safety` and the e2e flows cover the change.
- **The agent channel's `text`** stays in logical lines, as it reads the buffer.

### Where it lives

- **`oso-core`:**
  - `display_map/wrap_map.rs`, a new public module, with `WrapMap`, `WrapSnapshot`, `ViewRow`,
    `WrapJob` and `WrapReport`, and the break rules in a private `display_map/wrap_map/breaks.rs`;
  - `Window` gains its wrap and an offset top;
  - `Buffer` gains whether it wraps;
  - `frame.rs` and `editor/view.rs` read view rows.
- **`oso-render`:** draws view rows, and gains `Face::WrapMark` and its theme colour
  ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)).
- **`oso`:**
  - `toggle-truncate-lines` in `commands/view.rs` and its key in `bindings/view.rs`;
  - the job's thread in `app/`, as `app/searches.rs` runs isearch's;
  - the dev socket's fields.
- The API snapshots show the new public items.

### Tests and measures

- **A property test** compares the WrapMap with a plain greedy wrap from the line's start:
  - over random text, widths and edits;
  - CJK, marks, tabs, wide characters, escapes, invalid bytes and kinsoku among them;
  - the wrap after each edit equals a wrap from scratch;
  - a job's report with the edits since applied equals the wrap of the current text.
- **Unit tests for each rule:**
  - spaces at a break;
  - a word longer than a row;
  - each kinsoku set, and the two-cluster limit;
  - a cluster wider than a row;
  - a tab across the edge;
  - the mark's column.
- **Text-grid snapshots** ([ADR 0045](0045-text-grid-snapshots.md)):
  - a wrapped paragraph of Japanese and one of English;
  - the mark;
  - a region and a match over a break;
  - a preedit at the edge;
  - the gutter once ADR 0097's is built.
- **A `just e2e` flow:** `C-x x t` on a long line, `get-state`'s view rows, `C-n` to the next
  logical line, and a resize through the socket where it can.
- **`text_perf`:**
  - wrapping 100 MB of source and of prose;
  - wrapping a 10 MB line;
  - typing in a wrapped 10 MB line at 1 and 100 cursors;
  - the UI thread's share of a width change, which stays under a frame;
  - the memory of each.
- **`just perf`** keeps its scenarios with wrapping off. A wrapped one waits for the author's word,
  since its thresholds are guard files.

## Changes to accepted ADRs

The passages below stay in their ADR as written; what holds now follows each. Once this ADR is
accepted, the status of each becomes `Accepted, amended by … 0100`.

- [ADR 0006](0006-display-model.md), "buffer → FoldMap → InlayMap → TabMap → WrapMap → BlockMap →
  display rows": the order holds. The WrapMap and the BlockMap belong to the window, since their
  rows depend on its width, while the layers below stay the buffer's.
- [ADR 0035](0035-sumtree-rope-and-cell-grid.md), "A `DisplayPoint` is a row and a cell column.
  Rows are buffer lines": they stay buffer lines. What a window shows is in view rows, which the
  WrapMap gives.
- [ADR 0040](0040-buffers-loading-motion-and-scrolling.md), "long lines scroll sideways by the
  fewest columns": not in a wrapped buffer, where the left column stays 0.

## Consequences

- Prose reads without scrolling sideways, and motion, kills and `TAB` behave as they do unwrapped.
- In a wrapped paragraph, `C-n` jumps the whole paragraph. Moving within it takes `C-f`, `M-f`
  or isearch. Emacs's default, which moves by screen rows, was not chosen.
- Every window keeps a wrap of each wrapped buffer it shows, so two windows on one buffer pay twice.
- A width change can move the text in view by up to a row once, when the worker's wrap arrives.
- The renderer, the frame's rectangles, the view's scroll and the dev socket's rows all change to
  view rows. Unwrapped, every logical row is one view row, so they behave as today.
- The kinsoku rules are oso's own and cover Japanese only. Other scripts that need more than spaces
  and cluster breaks (Thai, for example) wrap at clusters until a later ADR.
- `Face::WrapMark` is a new face in every theme.

## Alternatives considered

- **On for `.md`, `.txt` and `.org` until modes:** the author chose the toggle alone.
- **On everywhere by default:** contrary to ADR 0006, and code reads better unwrapped.
- **UAX #14 in full through a crate:** a new dependency, and its tables, for what Japanese prose
  and spaces need.
- **Breaks at any cluster only:** splits English words.
- **`C-n` by screen rows, as Emacs's `line-move-visual`, or everything by screen rows, as
  `visual-line-mode`:** contrary to ADR 0006 and 0051, and to `kill-line` and `TAB`.
- **No wrap past 1 MB a line, or past 100 MB a buffer:** the author wants every line wrapped.
- **Continuation rows indented as their line, or by 2 more:** prose reads best from the edge, and
  the column of a view row would depend on its line's indent.
- **No mark, or a mark at a continuation row's start:** a wrap would look like a line break, or
  the mark would take a text column.
- **At most 80 columns, or a toggle for it:** the window's width suits a frame the person sizes;
  a fixed fill column can come with M3's configuration.
- **A preedit cut at the edge until it is committed, or shown in a float:** text typed at a row's
  end would be hidden, or away from where it goes.
- **The WrapMap in the buffer's display map:** one width per buffer, so two windows on it could
  not differ, and every reader of a `DisplayPoint` would have to tell logical rows from view rows.
- **Rewrapping a whole logical line on each edit:** a 10 MB line would cost tens of milliseconds a
  key.
- **A width change rewrapped on the UI thread:** a 100 MB file would freeze for a large part of a
  second.
- **A top kept as a view row number:** every rewrap above the view would move what is shown.
