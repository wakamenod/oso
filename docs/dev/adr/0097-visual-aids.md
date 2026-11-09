# 0097. Visual aids: whitespace, the current line, a cursor flash and line numbers

- Status: Accepted
- Date: 2026-11-09

## Context

[ADR 0026](0026-m2-ui-layout.md) decided which visual aids M2 has and which are on:

- on by default: whitespace visualization (a full-width space as `□`, trailing whitespace
  and tabs), a cursor flash after large jumps, and a highlight of the current line;
- off by default: line numbers.

`ui.md` adds that they toggle from a toggle menu, which does not exist yet. M2-24 builds
them. Each must say:

- how it looks in the three themes of [ADR 0089](0089-built-in-themes-switched-at-run-time.md);
- how it is drawn without slowing typing, the first frames or a 10 MB line;
- how it meets the secondary cursors M2-21 is drawing;
- how it meets soft wrap, which comes in M2-25.

The author chose these on 2026-11-09: what whitespace shows, the trailing fill off on a
cursor's line, the band on the primary cursor only, the flash's look and triggers,
absolute line numbers, the gutter's width, `M-x` toggles for every buffer with no keys,
and nothing remembered. The rest is settled here.

## Decision

### The switches

- Four switches, which `Editor` holds and every window reads:
  - whitespace
  - the current line
  - the flash
  - line numbers
- **Commands** named after Emacs's global modes:
  - `global-whitespace-mode`
  - `global-hl-line-mode`
  - `beacon-mode`
  - `global-display-line-numbers-mode`

  Emacs's buffer-local names (`whitespace-mode`, `display-line-numbers-mode`) are left
  for per-buffer switches with configuration (M3).
- **The minor-mode argument,** as Emacs reads it:
  - with no prefix argument, a command toggles;
  - a positive one turns the aid on;
  - zero or a negative one turns it off.

  So the agent channel's calls, which give a count, are idempotent. The spec is
  `ArgSpec::Count`, read through `Args::raw`.
- **What a command does beyond the switch:** it says in the echo area whether the aid is
  now on or off. No key is bound; the toggle menu (M3) will reach them.
- **Nothing is remembered across starts:** every start begins with ADR 0026's defaults,
  and lasting choices belong to M3's configuration.
- **Reach `Look`:** they change only what is drawn, and nothing on disk
  ([ADR 0085](0085-agent-channel-and-oso-mcp.md)).
  - Erratum (2026-11-09, the author's choice): `global-whitespace-mode` is `Person`. Its
    marks show lookalike spaces, so the agent channel may not turn them off before the
    person reads a file. The other three stay `Look`.
- **`get-state`** reports which aids are on.
- **An `oso dev` restart** carries the switches in its handoff:
  - a handoff without them starts from the defaults, so `STATE_VERSION` stays;
  - they are set before positions are restored, so that the gutter is laid out first.

### Whitespace

- **What is shown:**
  - a full-width space (U+3000) as `□`, across its two cells;
  - a tab as a dim `»` in its first cell, the rest of its cells blank;
  - the other non-ASCII spaces (U+00A0, U+2000 to U+200A, U+202F and U+205F) as a dim
    `·`, as they look like a space and can hide in code;
  - trailing whitespace as a fill.
  - A half-width space is not marked.
- **What counts as trailing:** spaces, tabs and U+3000 before a line's break, `\n` or
  `\r\n`, or before the end of the buffer.
  - A lone `\r` is an escape, not whitespace, so it ends a trailing run.
  - The non-ASCII spaces above are not trailing whitespace: they are marked where they are.
- **A line with a cursor keeps its trailing whitespace unmarked,** so that typing a space at
  its end does not flash a fill under it. Every cursor counts, the secondary ones too.
- **The marks are labels drawn from the display model:**
  - `CellStyle` gains `Whitespace`. Whether a row's clusters carry the labels is an
    argument to `row_clusters` and `DisplayCluster::for_each_run`, not state of the
    TabMap, as `display_map` knows nothing of the editor's switches.
  - With the switch off, a tab's run is a space across its cells, as now.
  - The argument is always off for `text_clusters`, so no `□` or `»` shows in a modeline
    or a float, and inside a preedit, which is drawn as it is composed.
  - A cluster whose first character is U+3000 shows `□`. A mark or a variation selector
    after it follows the cluster rules that exist ([ADR 0054](0054-limits-on-a-display-cluster.md)).
  - The labels are only drawn. The buffer's text, `get-text` and copying see the tab or
    the space, as with escapes ([ADR 0035](0035-sumtree-rope-and-cell-grid.md)).
- **A mark cannot pass for text:**
  - its face, `Face::Whitespace`, is used by nothing else, and the contrast test keeps it
    apart from every face text is drawn in;
  - `□` takes two cells, where a real U+25A1 takes one in the text's colour;
  - text-grid snapshots mark a whitespace cell.
- **Edges:** a tab cut at the window's left edge by horizontal scrolling shows its
  remaining cells blank, as the `»` is in its first cell.
- **The rows cache keys on the switch,** so a toggle shapes the rows in view again on the
  next frame. It does not wait for an edit: `Rows` reuses a row only while what it would
  shape is the same.
- **The trailing fill** is the list of cells `Editor` works out each frame for the rows in
  view, as the region's are ([ADR 0071](0071-mark-and-region.md)). Its cost is bounded:
  - for each row whose line ends, it walks back from the line's break over whitespace.
    A walk backward over a line's characters is added to the display map for this;
  - each row's walk may read at most 512 characters, and a frame's at most 16 384 in all,
    as `motion::WalkBudget` bounds a motion;
  - a row whose trailing run does not end within the budget gets no fill. A 10 MB line
    of spaces is drawn as fast as one of letters, unmarked.
  - Erratum (2026-11-09, as built): the walk lives in `Editor`, not the display map. It
    takes each row's bounds from the display model (`motion::line_start` and `line_end`),
    fills only a row that ends its line, and reads the text back from there a few bytes at
    a time, so it counts display rows and stays right with soft wrap.
  - The fill is `Face::TrailingWhitespace`.

### The current line

- **The band:** on the primary cursor's row of the tiled window, across its text area, in
  `Face::CurrentLine`.
  - A prompt being selected does not move it, so `M-x` leaves it where it was.
  - Secondary cursors get none.
  - The gutter gets none: there, the current line's number shows it.
- **It is the back of the fills:** behind isearch's matches and the region
  ([ADR 0082](0082-isearch.md)), so those stay as they are, and behind the boxes and the
  cursors, primary and secondary.
- **It is one rectangle per frame,** from the cursor's display row, with no cache to keep.
- **With soft wrap (M2-25)** the band covers every row of the logical line the cursor is on.

### The cursor flash

- **The trigger is in `oso-core`:** a function of the tiled window's view before and after
  each command or key that changes it, so it is tested with no clock. The app keeps only
  the clock.
- **It flashes when:**
  - the primary cursor moves 5 or more display rows in one command, an undo and `C-g` out
    of isearch, back to where the search began, included;
  - the window is scrolled by a command that scrolls on purpose: `C-v`, `M-v`, `C-l`,
    `M-<`, `M->`;
  - another buffer is shown in the window, or another window is selected.
- **It does not flash for:**
  - the scrolling that only follows the cursor, a row or a few, as `C-n` and typing do;
  - the wheel;
  - typing and editing;
  - isearch's steps, which have their own current match;
  - opening or closing a prompt. The command a prompt's answer runs counts, so `M-g g`
    flashes once its line is far.
- **What it draws:** a band from the cursor to the text area's right edge, on the cursor's
  row, in `Face::Flash`, its strength falling from full to none over about 0.4 s.
  - `TextView` gains the flash's cells and strength, and the scene draws the face's
    background at that strength, last among the fills and under the text.
  - Faces live in `pane`, and their colours in the theme.
- **It ends at the next key,** or when the window is hidden. Only the primary cursor
  flashes.
- **Its cost:**
  - While it fades, the app asks for a frame every 16 ms, about 25 frames, each drawing
    what is shaped already. Nothing is reshaped.
  - At rest it asks for nothing. A key during the fade ends it first, so typing never
    waits on it.
- **One place sets the event loop's wake:** the earliest of every deadline the app keeps,
  the flash's next frame and the saves' idle wait among them. `wake_at` holds one
  deadline, so two places setting it would cancel each other.
- **With soft wrap,** a large jump counts display rows.

### Line numbers

- **What:** absolute numbers, right-aligned. The current line's number is in
  `Face::LineNumberCurrent`, the others in `Face::LineNumber`. Rows past the buffer's end
  are blank.
- **The gutter is a region of the window's layout:**
  - It sits at the window's left, beside its text area ([ADR 0057](0057-frame-layout.md)).
  - Its width is the sum of its columns. Line numbers are one column; signs, such as a
    diagnostic's, can be another later.
  - The line-number column is the digits of the buffer's line count plus one cell of
    space, at least 3 digits.
- **What keeps using the text area:** positions, scrolling, the cursor, the input
  method's rectangles and the band. None of them changes.
- **The width changes only with the digits:** when the line count gains or loses a digit,
  by an edit, a load or another buffer shown, the text area loses or gains a column. The
  rows in view are then shaped again once.
- **The gutter's rows are spans,** built each frame by `Editor::gutter_into` in `oso-core`,
  as `status_into` builds the modeline's. So snapshots see the gutter, and soft wrap needs
  no cache.
  - Numbers are short pieces kept by their text, so a number already shaped costs nothing
    ([ADR 0068](0068-cut-clusters-short-pieces-the-cursor-box-and-lines-by-direction.md)).
- **With soft wrap,** a logical line's number is on its first row, and the rows it
  continues on are blank.

### Where they apply

- **Tiled windows only:** every buffer shown there, internal buffers too. Floats (prompts,
  completion, questions) have none, as their rows are not a buffer the person edits.
- **Floats keep their place:** they centre on the window's whole area, gutter and text,
  not on its text area. So turning line numbers on, or a load that adds a digit, does not
  move or narrow a prompt or the completion list.
- **In a buffer that may hold secrets** they are drawn as in any other. What `get-state`
  and the agent channel withhold there stays withheld:
  - the band's and the gutter's rows are positions, and are left out with the cursor's;
  - `get-state` reports a window's columns as gutter and text together, so the gutter's
    width does not say how many lines a secret buffer has.

### Faces and contrast

- **Six new faces,** added to every theme ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)):

  | Face | What it must reach |
  |---|---|
  | `Whitespace` | 3:1 on the background, `CurrentLine`, `TrailingWhitespace`, the region, both search fills and `Flash` at full strength; apart from every face text is drawn in by the OKLab distance |
  | `TrailingWhitespace` | a fill |
  | `CurrentLine` | a fill; text keeps 7:1 on it, and escapes and boxes 4.5:1 |
  | `Flash` | a fill; text keeps 7:1 on it at full strength, and it stands apart from `CurrentLine` |
  | `LineNumber` | 4.5:1 on the background |
  | `LineNumberCurrent` | 7:1 on the background, apart from `LineNumber` |

  The 3:1 is a new tier: marks to notice, not text to read.
- **Telling fills apart:** `CurrentLine` stands apart from the background by the minimum
  OKLab lightness step. The region, isearch's matches and the trailing fill stand apart
  from `CurrentLine`, as from the background.
- **Picking the colours:** as M2-23's were. The sample frames drawn off screen
  (`theme_samples`) gain the aids, and the author picks from them.
- **Snapshots** show each aid ([ADR 0045](0045-text-grid-snapshots.md)):
  - the band, the trailing fill and the flash on the fills line below a row;
  - the gutter as its own column;
  - a whitespace cell on the marks line.

  The harness starts with the aids off, so existing snapshots keep their look, and each
  aid's scenarios turn it on.

## Changes to accepted ADRs

**ADR 0035** (SumTree rope and cell grid): "a space across a tab's cells" holds with
whitespace off. With it on, a tab's run is a `Whitespace` label, `»` then spaces, and
U+3000 and the other non-ASCII spaces carry theirs.

**ADR 0045** (Text-grid snapshots):
- the marks table gains `w`, a whitespace label's cells;
- the fills line, which marks the region and isearch's matches, gains `l` for the current
  line's band, `t` for the trailing fill and `f` for the flash;
- a window's rows begin with the gutter's text, then `┆`, when line numbers are on.

**ADR 0057** (Frame layout):
- `frame::layout` takes the gutter's width with the frame's size, and gives each window a
  gutter beside its text area. The text area's column is then not 0.
- The layout changes when the gutter's width does, which an edit, a load or another
  buffer shown can do. `Editor` lays it out again then, as on a new frame size.
- Floats centre on the window's whole area, not its text area.

**ADR 0089** (Built-in themes):
- the contrast test gains a 3:1 tier, for marks such as whitespace's;
- the sample frames gain the visual aids.

## Consequences

- **Typing costs:**
  - one more rectangle for the band;
  - the trailing fill's walk over the rows in view, bounded per row and per frame.
- **The whitespace marks** are part of the shaped row, as escapes are. A toggle shapes the
  rows in view again, once.
- **The flash** draws frames only while it fades, and never while a key waits.
- **The gutter** narrows the text area by its width. A buffer that reaches another digit of
  lines reshapes the rows in view once. Floats do not move.
- **Code to come:**
  - the switches, the flash's trigger, `gutter_into` and the trailing cells in `oso-core`,
    with a backward walk over a line in `display_map`;
  - the whitespace labels and their argument in `display_map`;
  - the gutter in `frame::layout`;
  - the gutter pane, the flash's clock and the single wake in the app;
  - six faces, with the flash's strength in `TextView`;
  - four commands;
  - snapshot notation.
  No new crate, crate edge, dependency or `unsafe`.

## Alternatives considered

- **Half-width spaces as `·` too:** shows every space, at the cost of a busy screen.
- **Leaving the other non-ASCII spaces unmarked:** they look like a space, and are what a
  lookalike in code would use.
- **Trailing whitespace marked on the cursor's line too:** the fill would flash under each
  space typed at a line's end.
- **Scanning each line's bytes to its end for the trailing fill:** a 10 MB line would cost
  10 MB each frame.
- **A band on every cursor's line:** with many cursors, it would cover much of the screen
  and hide which cursor is primary.
- **A band across the gutter too:** the current number would need 7:1 on the band as well,
  and the number already marks the line there.
- **Line numbers in place of a band:** the current line would show only while numbers are
  on.
- **The cursor box blinking twice:** a smaller sign, and a blink is what ADR 0026's
  non-blinking cursor avoids.
- **Flashing on any scroll, or only on a switch or a scroll:** the first would flash at each
  `C-n` past the window's edge and each wheel step. The second would miss a long `M->` or
  `M-g g`.
- **Relative numbers, or the current line absolute and the rest relative:** every cursor
  motion would change every number in view.
- **A gutter of fixed width:** it wastes columns on small files and cuts numbers past 9999.
- **A cache of the gutter's spans:** it would need soft wrap's rows to stay right, and
  building a screen of short spans costs less than keeping one.
- **Floats centred on the text area:** turning line numbers on, or a load past 999 lines,
  would move a prompt while it is open.
- **Emacs's buffer-local names for global switches:** the names would mean something else
  once configuration adds per-buffer switches.
- **Keys bound now:** few keys are free, and the toggle menu is where these belong.
- **Per-buffer switches:** closer to Emacs's minor modes, but more to explain until
  configuration exists.
- **Remembering the switches, as the theme is:** that is configuration's job. The theme is
  remembered because the person picks it, where the aids keep defaults.
