# 0100. Soft wrap

- Status: Accepted, amended by 0102, 0103, 0104 and 0125
- Date: 2026-11-09

## Context

M2-25 brings soft wrap, the last step of M2 ([ADR 0051](0051-m2-plan-decisions.md)). Long
paragraphs of Japanese prose are hard to read by scrolling sideways.

[ADR 0006](0006-display-model.md) puts a WrapMap above the TabMap. It decides that wrapping is
off by default, turned on per mode or by a toggle, and that `C-n` and `C-p` move by logical
lines. It does not say which object owns each layer.

What stands in the way today:

- Every reader of a display row takes it for a buffer line: motion, word motion, `edit-lines`, the
  frame's cell rectangles, the renderer's rows, isearch's highlights and the input method's
  rectangle.
- A window's top is a display row, and so is everything that keeps a view:
  - a place ([ADR 0088](0088-switching-and-killing-buffers.md));
  - isearch's view for `C-g` ([ADR 0082](0082-isearch.md));
  - `oso dev`'s handoff ([ADR 0047](0047-process-spawning-and-restart-handoff.md)).
- The display map belongs to the buffer, but the width belongs to the window, and two windows on
  one buffer can be of different widths.
- The budgets hold with wrapping on ([`roadmap.md`](../roadmap.md)):
  - key press to photon at p99 ≤ 8 ms, also with 1000 cursors
    ([ADR 0091](0091-multiple-cursor-keys-and-m-y.md));
  - no freeze on a 10 MB line, a 100 MB file or a 1 GB one;
  - no freeze when the width changes.
- Several ADRs left a point for M2-25:
  - the band, the numbers and the flash of [ADR 0097](0097-visual-aids.md);
  - `TAB`'s columns ([ADR 0056](0056-typing.md));
  - `C-a`, `C-e` and `kill-line` on logical lines
    ([ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md),
    [ADR 0074](0074-the-kill-ring-and-the-clipboard.md));
  - the modeline's line ([ADR 0063](0063-modeline-echo-area-and-internal-buffers.md));
  - a preedit at the right edge ([ADR 0069](0069-macos-input-method.md)).

## Decision

The author chose each point marked (author) on 2026-11-09, the recommended option each time but
the first.

### Turning it on

- **Off by default, for every buffer (author).** `toggle-truncate-lines` turns it on or off for
  the current buffer, on `C-x x t` as in Emacs 28. Nothing turns it on by file name; modes do that
  from M3, as ADR 0006 says.
- **Its argument is Emacs's (author):**
  - with no prefix argument, it toggles;
  - a positive one truncates, which turns wrapping off;
  - zero or a negative one wraps.

  This is the reverse of ADR 0097's aids, whose positive argument turns them on. Given a count,
  the agent channel's call is idempotent either way. It says in the echo area whether long lines
  are now truncated, as Emacs does. The spec is `ArgSpec::Count`, read through `Args::raw`.
- **Its reach is `Look`** ([ADR 0085](0085-agent-channel-and-oso-mcp.md)): it changes how text is
  shown, never the text, a file or the clipboard, and needs no person.
- **The setting is the buffer's; the width is each window's.** Two windows on one wrapped buffer
  each wrap at their own width.
- `oso dev` carries each buffer's setting to the instance that replaces it (ADR 0047). This is a
  new field of the handoff, and so a new `STATE_VERSION`.

### Where a row breaks

- **At the window's text width (author), less one column for the continuation mark.** A gutter
  takes its own columns beside it (ADR 0097).
- **Below 8 text columns, nothing wraps,** and no mark shows. The window is too narrow to read
  wrapped text in, and a row of a single wide cluster would never end.
- **Only between display clusters,** so cursors, regions and matches never split one.
- **Break rules (author)** of oso's own, with no new dependency:
  - **Text with spaces** breaks after a run of U+0020.
  - **CJK** breaks between any two clusters, and between CJK and other text.
  - **Never a break opportunity:** U+00A0, U+202F and U+2007 (no-break spaces), and inside a word.
    A word longer than a row breaks at the last cluster that fits.
  - **Kinsoku** uses the core sets of JIS X 4051:
    - a row starts with none of `、。，．・：；？！ー）］｝〕〉》」』】` or the small kana, nor with
      ASCII `,.:;?!)]}` after CJK;
    - a row ends with none of `（［｛〔〈《「『【`, nor with `([{` before CJK;
    - a break that would break a rule moves back, by up to two clusters, which then start the next
      row. If two are not enough, it breaks where it would have.
- **The spaces at a break (author):**
  - The U+0020 that fit stay at the end of their row.
  - One more may take the mark's column: the mark draws in its cell, and a cursor, region or match
    on that space draws there too.
  - Any spaces left start the next row.
  - U+3000 and tabs never take the mark's column, so ADR 0097's `□` and `»` always show where the
    character is.

  So every cluster has a cell, and a view row's width is bounded.
- **A cluster that does not fit** goes to the next row whole. That covers a wide character, a tab,
  and an escape box such as `\xNN` ([ADR 0040](0040-buffers-loading-motion-and-scrolling.md)). The
  cells it leaves at the end of its row stay blank. A cluster wider than a whole row gets a row of
  its own and is cut at the edge, as today.
- **A tab keeps the width it has on its logical line.** The TabMap expands tabs before rows wrap,
  so a tab's cells count from the logical line's start, as `TAB` counts them (ADR 0056).
- **What a break depends on:** the clusters from its row's start to the edge, plus two clusters
  of lookahead for kinsoku. A row's start depends on the row before it. This bounds what an edit
  can change (below).

### How wrapped rows look

- **A continuation row starts at the left edge (author),** with no indent.
- **A mark, `↩`, in the last column of each row that continues (author).**
  - It is in `Face::WrapMark`, a new face in ADR 0097's 3:1 tier for marks to notice, used by
    nothing else ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)).
  - It cannot pass for text: text never takes that column but for one hanging space, and a
    literal U+21A9 is in the text's face. Text-grid snapshots mark the cell with `m`
    ([ADR 0045](0045-text-grid-snapshots.md)).
- **ADR 0097 holds,** in view rows:
  - the current line's band covers every view row of the logical line;
  - a logical line's number is on its first view row, and its continuation rows are blank;
  - the flash counts a jump in view rows;
  - the trailing fill is drawn only on a logical line's last view row, as only there do spaces
    come before its break.
- **Nothing scrolls sideways while a buffer is wrapped.** The left column stays 0, and the
  sideways wheel does nothing.

### Motion stays on logical lines

- **`C-n`, `C-p`, `C-a`, `C-e`, `kill-line`, `edit-lines` (`C-x SPC`) and `TAB` work on logical
  lines (author),** as ADR 0006, 0051, 0056, 0058 and 0074 say. `C-n` keeps its goal column in the
  logical line's display columns. So in a wrapped paragraph, `C-n` goes to the next paragraph.
- **Word motion is unchanged:** the end of a view row that wraps is not the end of a word.
- **Scrolling is by view rows.**
  - `C-v` and `M-v` move a screenful, keeping 2 rows of context, and `C-v N` moves N rows.
  - `C-l`'s recenter cycle (ADR 0058) and the scroll by a count count view rows.
  - The view follows the cursor's view row, and the wheel moves by view rows.
  - Mouse hits (M3) go through the wrap.
- **The modeline's line and column stay logical** (ADR 0063).

### The WrapMap

- **A window holds the wrap of its buffer at its width.** It is made from the buffer's display
  snapshot. The display map stays the buffer's, so a `DisplayPoint` keeps meaning a logical line
  and a cell column in it, and every reader of one is right as it is.
  - A **view row** is a row on the screen: the offset where it starts, and the cell column of its
    logical line where it starts. Unwrapped, each logical line is one view row.
  - Only what draws or scrolls reads view rows. That is the renderer, the frame's cell rectangles,
    the window's scroll, isearch's highlights and the input method's rectangle.
- **A `SumTree` of breaks with bounded items.** An item is either a run of logical lines that do
  not wrap, or at most 64 breaks within one TabMap chunk. Its summary counts view rows and bytes,
  so a view row and an offset convert both ways in O(log n).
  - A 10 MB line at 100 columns holds about 100,000 breaks in about 1,600 items. An edit copies
    the path to the items it changes, not the line.
  - A break costs at most 8 bytes, so a wrap takes at most 8 bytes per view row that continues.
    100 MB of one-line paragraphs at 40 columns comes to about 2.5 million breaks, about 20 MB a
    window. `text_perf` measures it, and a 1 GB file too (below).
- **Whether a line fits** is found by reading its clusters, on the worker or for the rows in view.
  The TabMap gains no field for it (the checkpoints of ADR 0099, PR #123, make a scan from mid-line
  cheap). A line's width in the TabMap's summary would need a later ADR.

### A window's top

- **A window's top is an offset, wrapped or not:** the start of its first view row. Unwrapped, that
  is a line's start. The left column stays a column.
  - It follows edits as a marker that stays before an insertion at it
    ([ADR 0053](0053-edit-core.md)).
  - It snaps back to the start of its view row after every rewrap, edit, reload or report, so the
    first row in view always starts where a row starts.
- **Everything that keeps a view keeps the offset,** in place of the top row:
  - `Scroll`, and `Window::set_top`, `follow` and its reach;
  - `Editor::restore_positions`;
  - a place (ADR 0088), and isearch's view for `C-g` (ADR 0082);
  - `C-x b`'s preview, which wraps the previewed buffer at the preview's width;
  - the scroll by a count;
  - the check whether a load touched the rows in view;
  - `oso dev`'s handoff;
  - the agent channel's state (below).

### Edits

- **Every window's wrap follows each edit with the buffer.** `Editor` rewraps every window that
  shows the buffer on clones, and commits them in the same step that swaps the buffer's rope and
  display map in, which cannot panic (ADR 0053). So a window's wrap and its buffer never disagree.
- **A command's splices rewrap in one pass,** ascending, over the ranges the edit rebuilt
  (`changed`, [ADR 0092](0092-edits-in-place-for-many-cursors.md)), after all of them are applied.
- **Where a rewrap starts:**
  - Clusters change only in the rebuilt chunks (ADR 0092), so it starts at the view row before the
    one that holds the rebuilt chunks' start.
  - It starts further back while those rows are shorter than the rules' reach, two clusters.
- **Where it may stop,** when a new break equals an old one:
  - past the end of the rebuilt chunks and the rules' lookahead;
  - at the same offset;
  - with the same column modulo the tab width;
  - after a cluster of the same class for the rules.

  From there every break is as before.
- **What the UI thread does is bounded** by the rows in view and a byte budget, 256 KiB a command,
  as a key's is (ADR 0091).
  - Text with spaces resyncs within a row or two of an edit.
  - In CJK or text with no spaces, the breaks after an edit all shift and may not resync before the
    line's end. Past the rows in view and the budget, the old breaks are kept, shifted by the
    edit's length, as a provisional wrap of the line's stale tail.
  - The stale tail goes to the worker. Its breaks fall in unchanged text, so they stay on cluster
    starts; rows there may be a little short or long until the worker's report.
- **A large edit is a width change.** Past the budget, the edit's lines are rewrapped as after a
  width change, with the rows in view at once and the rest on the worker. That covers a 16 MiB
  yank, a reload's span ([ADR 0083](0083-reloading-files-changed-on-disk.md)), a replace from the
  agent's 1 MiB insert, and undoing a large kill.

### The worker

- **A width change, a load, a large edit and a stale tail wrap on a worker,** as isearch's and
  completion's work does ([ADR 0082](0082-isearch.md),
  [ADR 0096](0096-completion-on-a-worker-past-5000-candidates.md)).
  - The editor hands out a `WrapJob` for a snapshot, a width and the range to wrap, and the app
    runs it on its thread.
  - ADR 0097's gutter gaining a digit, or line numbers turning on, is a width change.
- **One job a window at a time.** A new job supersedes the running one: its generation goes up, and
  the old one stops at its next check, as a `SearchJob` does. So a resize drag runs one job, for
  the last width.
- **Edits since the snapshot:** the window keeps the rebuilt ranges of the commands since, up to
  64 commands or 1 MiB.
  - A report applies them, as an edit's rewrap does, and then replaces what it covers.
  - Past that bound, the report is dropped and the job runs again from the current snapshot, as
    isearch drops a stale answer.
- **A resize during a load:** the part jobs at the old width are dropped. One job wraps what has
  loaded at the new width, and the parts follow in order.

### Before a report arrives

- **A provisional wrap starts from any cluster start.** Rows are wrapped forward from an anchor:
  the window's top, or the cursor's offset backed up by at most 64 KiB to a cluster start, a space
  break or a line's start, whichever is nearest.
  - So after a toggle with the cursor 5 MB into a line, the cursor's row is found within the
    bound, and the view shows it.
  - Rows before the top come from the same anchor, as `M-v` and `C-l` need them.
- **A provisional row count** covers what is not yet wrapped: each such logical line counts its
  cells over the width, rounded up, read from what the scan has, or its bytes when the scan has not
  reached it.
  - Scroll bounds, `M->`, `C-v`'s end check and the wheel's stop read it.
  - So does `C-x b`'s preview, which centres its row from the top.
  - It is right once the report arrives.
- **The rows in view are wrapped on the UI thread** from the top, within the view's rows and the
  byte budget.
  - Hanging spaces are bounded, so a view row reads at most its cells' clusters.
  - A width change draws its next frame at once.
  - If the budget runs out before the view is full, the rest of the view shows its logical lines
    unwrapped, cut at the edge, until the report.
- **When a report arrives,** the top snaps to the start of the view row that holds the top's
  offset. The rows in view may break differently, in CJK every one of them, but the text at the
  top stays in the first row.

### The input method on a wrapped row

- **A preedit wraps as if it were inserted (author).** Its logical line is laid out again with the
  preedit in it, from where an edit there would start a rewrap (above) to the end of the rows in
  view, on the UI thread. So a preedit that starts with `。` at a row's start pulls the row before
  it as committing it will, and the text does not jump on commit.
- Rows after it in view move down, nothing past the view is rewrapped, and committing makes an
  ordinary edit.
- The input method's rectangle is the cursor's cell in that layout (ADR 0069).

### The renderer and the frame

- **Rows are kept as ADR 0051 says: by their place on the screen, compared by content.**
  - A cached row is reused when the bytes it covers and its start column match. For a view row,
    the start column is the logical line's column where the row starts.
  - Whether it ends at a break, and so draws the mark, is compared too.
  - Nothing in an entry is a buffer offset (`architecture.md`).
- **The frame's cell rectangles read view rows:**
  - the cursor's, and the other cursors' (ADR 0091);
  - the region's ([ADR 0071](0071-mark-and-region.md)), and isearch's matches (ADR 0082).

  A range that goes on past a view row fills to that row's last cell, the mark's column
  included when a hanging space is in the range.

### The sockets and secrets

- **The dev socket:**
  - `get-state`'s `text.rows` and `get-text` give view rows;
  - `scroll` gives the top's offset and view row;
  - each buffer says `wrapped`.
  `just release-safety` and the e2e flows cover it.
- **The agent channel** ([ADR 0085](0085-agent-channel-and-oso-mcp.md)):
  - its `text` stays in logical lines, as it reads the buffer;
  - its state's `first_row` stays the logical line that holds the top's offset;
  - its state's rows are logical lines, so an agent's view does not depend on a window's width;
  - each buffer gains `wrapped`, a new field in the versioned schema.
- **Secrets:**
  - Where rows break says how long words are, so a buffer that may hold secrets withholds the
    top's offset and view row as it withholds its other positions (ADR 0051, ADR 0058).
  - `Debug` on `WrapJob`, `WrapReport`, `WrapSnapshot`, `ViewRow` and `Window` prints no offsets,
    as `SearchReport`'s does.
  - The worker reads the text in process, keeps no copy past the job and logs nothing of it
    ([ADR 0028](0028-secrets-handling.md)).

### Where it lives, and in what order

- **`oso-core`:**
  - `display_map/wrap_map.rs`, a new public module. It exports `WrapSnapshot`, `ViewRow`,
    `WrapJob` and `WrapReport`; `WrapMap` itself is `pub(crate)`, held only by `Window`.
  - The rules are in a private `display_map/wrap_map/breaks.rs`.
  - `Window` gains its wrap and an offset top, `Scroll` and `Place` change, and `Buffer` gains
    whether it wraps.
  - `frame.rs` and `editor/view.rs` read view rows.
- **`oso-render`:** draws view rows, and gains `Face::WrapMark` and its colour in each theme.
- **`oso`:**
  - `toggle-truncate-lines` in `commands/view.rs` and its key in `bindings/view.rs`;
  - the job's thread in `app/`, as `app/searches.rs` runs isearch's;
  - the dev socket's, the channel's and the handoff's fields.
- The API snapshots show what changes.
- **Order:**
  1. The WrapMap, its rules and `text_perf`, after ADR 0099 (PR #123)
     merges. Its result does not depend on the checkpoints, which are exact; its cost
     does.
  2. The offset top, the window's wrap and the toggle.
  3. The renderer, the frame, isearch and the input method.
  4. The worker.

### Tests and measures

- **A property test** compares the WrapMap with a plain greedy wrap from the line's start. It runs
  over random text, widths and edits, with CJK, marks, tabs, wide characters, escapes, invalid
  bytes, no-break spaces and kinsoku among them:
  - the wrap after each command equals a wrap from scratch, once its stale tail is done;
  - a report with the edits since applied equals the wrap of the current text;
  - a provisional wrap from an anchor equals the real one from the first row that starts at a
    real break.
- **Unit tests for each rule:**
  - the spaces at a break and the one that hangs;
  - U+3000 and tabs, which never hang;
  - no-break spaces;
  - a word longer than a row;
  - each kinsoku set, and the two-cluster limit;
  - a cluster wider than a row;
  - a tab across the edge;
  - the 8-column floor.
- **Unit tests for the top:** the top through edits, reloads, places, isearch's `C-g` and the
  handoff; and supersession and the stale-report rerun of jobs.
- **Text-grid snapshots** (ADR 0045):
  - a wrapped paragraph of Japanese and one of English;
  - the mark and a hanging space;
  - a region and a match over a break;
  - a preedit at the edge;
  - the gutter once ADR 0097's is built.
- **A `just e2e` flow:** `C-x x t` on a long line, `get-state`'s view rows, `C-n` to the next
  logical line, and a resize through the socket where it can.
- **`text_perf`:**
  - wrapping 100 MB of source and of prose, and 1 GB, on the worker, with the memory of each;
  - wrapping a 10 MB line of each kind: with spaces, CJK, and no spaces;
  - typing in a wrapped 10 MB line of each kind, at 1 and at 1000 cursors;
  - the UI thread's share of a width change and of a large edit, which stays under a frame.
- **`just perf`** keeps its scenarios with wrapping off. A wrapped one waits for the author's word,
  since its thresholds are guard files.

## Changes to accepted ADRs

The passages below stay in their ADR as written; what holds now follows each. The status of each
is now `Accepted, amended by … 0100`.

- [ADR 0035](0035-sumtree-rope-and-cell-grid.md), "A `DisplayPoint` is a row and a cell column.
  Rows are buffer lines": they stay logical lines. What a window shows is in view rows, which the
  window's WrapMap gives.
- [ADR 0040](0040-buffers-loading-motion-and-scrolling.md):
  - "a `SelectionSet` and its scroll position (top row, left column)": its top's offset and left
    column;
  - "long lines scroll sideways by the fewest columns": not in a wrapped buffer, where the left
    column stays 0.
- [ADR 0045](0045-text-grid-snapshots.md): the marks table gains `m`, the wrap mark's cell.
- [ADR 0088](0088-switching-and-killing-buffers.md), a place's "the top row and the left column":
  the top's offset and the left column.

ADR 0006's order of layers holds. It names no owner for them, and this ADR puts the WrapMap in the
window. Whose the BlockMap is, the buffer's or the window's, is decided when it comes.

## Consequences

- Prose reads without scrolling sideways, and motion, kills and `TAB` behave as they do unwrapped.
- In a wrapped paragraph, `C-n` jumps the whole paragraph; moving within it takes `C-f`, `M-f` or
  isearch. Emacs's default, which moves by screen rows, was not chosen.
- Each window keeps a wrap of each wrapped buffer it shows, so two windows on one buffer pay twice.
- After a width change, or an edit in a long CJK line, rows can be provisional for a moment: some
  short or long in a stale tail, or the view's rows reflowing when the report arrives.
- The renderer, the frame's rectangles, the scroll, places, the handoff and the dev socket's rows
  all change; unwrapped they behave as today.
- The kinsoku rules cover Japanese only. Scripts that need more than spaces and cluster breaks,
  such as Thai, wrap at clusters until a later ADR.
- `toggle-truncate-lines`' argument runs the other way from ADR 0097's aids.

## Alternatives considered

- **On for `.md`, `.txt` and `.org` until modes:** the author chose the toggle alone.
- **On everywhere by default:** contrary to ADR 0006, and code reads better unwrapped.
- **A positive argument turning wrapping on, or a second command for it:** the author kept
  Emacs's meaning.
- **UAX #14 in full through a crate:** a new dependency and its tables, for what Japanese prose and
  spaces need.
- **Breaks at any cluster only:** splits English words.
- **Spaces that never hang, or that all hang past the edge:** the first starts rows with a space
  whenever a word ends at the edge. The second leaves a cursor on a hanging space with no cell, and
  makes a row of spaces as long as the line.
- **`C-n` by screen rows, as Emacs's `line-move-visual`, or everything by screen rows, as
  `visual-line-mode`:** contrary to ADR 0006 and 0051, and to `kill-line` and `TAB`.
- **No wrap past 1 MB a line, or past 100 MB a buffer:** the author wants every line wrapped.
- **Continuation rows indented as their line, or by 2 more:** prose reads best from the edge, and a
  view row's column would depend on its line's indent.
- **No mark, or a mark at a continuation row's start:** a wrap would look like a line break, or the
  mark would take a text column.
- **At most 80 columns, or a toggle for it:** the window's width suits a frame the person sizes; a
  fill column can come with M3's configuration.
- **A preedit cut at the edge until it is committed, or shown in a float:** text typed at a row's
  end would be hidden, or shown away from where it goes.
- **The WrapMap in the buffer's display map:** one width per buffer, so two windows on it could not
  differ, and every reader of a `DisplayPoint` would have to tell logical rows from view rows.
- **One wrap per buffer, keyed by width and tab width, shared by windows of the same width:** saves
  memory only when two windows on one buffer have one width, which splits rarely give, at the cost
  of a shared cache and its eviction.
- **One item per wrapped line:** a 10 MB line would be one item of 100,000 breaks, copied whole on
  every edit.
- **Rewrapping a whole logical line on each edit, or resyncing at the first equal break:** the
  first costs about 70 ms a key on a 10 MB CJK line, and 1000 times that with 1000 cursors. The
  second stops at the row before the edit, before anything changed.
- **A width change rewrapped on the UI thread:** a 100 MB file would freeze for a large part of a
  second.
- **A top kept as a view row number:** every rewrap above the view would move what is shown, and a
  report would have to find a row number first.
- **A row cache keyed by a view row's offset:** ADR 0051 compares rows by content, so that undo,
  reload and edits above need no record of what changed.
