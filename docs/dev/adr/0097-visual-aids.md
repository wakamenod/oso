# 0097. Visual aids: whitespace, the current line, a cursor flash and line numbers

- Status: Proposed
- Date: 2026-11-09

## Context

[ADR 0026](0026-m2-ui-layout.md) decided which visual aids M2 has and which are on:

- on by default: whitespace visualization (a full-width space as `□`, trailing whitespace
  and tabs), a cursor flash after large jumps, and a highlight of the current line;
- off by default: line numbers.

`ui.md` adds that they toggle from a toggle menu, which does not exist yet. M2-24 builds
them. Each must say:

- how it looks in the three themes of [ADR 0089](0089-built-in-themes-switched-at-run-time.md);
- how it is drawn without slowing typing or the first frames;
- how it meets the secondary cursors M2-21 is drawing;
- how it meets soft wrap, which comes in M2-25.

The author chose these on 2026-11-09.

## Decision

### Whitespace

- **What is shown:**
  - a full-width space (U+3000) as `□`;
  - a tab as a dim `»` in its first cell, the rest of its cells blank;
  - trailing spaces and tabs on a line as a fill.
  - Half-width spaces are not marked.
- **The cursor's line keeps its trailing whitespace unmarked,** so that typing a space at
  the end of a line does not flash a fill under it. Every cursor's line counts, the
  secondary ones too.
- **The marks come from the display model:**
  - `CellStyle` gains `Whitespace`. The runs of a tab and of U+3000 carry its label, `»` or
    `□`, in the face `Face::Whitespace`, which [ADR 0089](0089-built-in-themes-switched-at-run-time.md)'s
    contrast test checks.
  - The label is only drawn. The buffer's text, `get-text` and copying see the tab or the
    space, as escapes already work ([ADR 0035](0035-sumtree-rope-and-cell-grid.md)).
- **A mark cannot be taken for text:**
  - its face is used by nothing else;
  - a real `»` or `□` in a file is drawn in the text's colour;
  - text-grid snapshots mark a whitespace cell, so the two differ there too
    ([ADR 0045](0045-text-grid-snapshots.md)).
- **The trailing fill** is a list of cell rectangles that `Editor` works out each frame for
  the rows in view, as the region's are ([ADR 0071](0071-mark-and-region.md)). Each
  visible row costs the length of its trailing run. The fill is `Face::TrailingWhitespace`.

### The current line

- **The selected window's primary cursor's row** gets a band the window's width, in
  `Face::CurrentLine`. Secondary cursors get none.
- **It is the back of the fills:** behind isearch's matches and the region
  ([ADR 0082](0082-isearch.md)), so those stay as they are, and behind the boxes and the
  cursors, primary and secondary.
- **It is one rectangle per frame,** from the cursor's display row, with no cache to keep.
- **With soft wrap (M2-25)** the band covers every row of the logical line the cursor is on.

### The cursor flash

- **When:**
  - after the primary cursor moves 5 or more rows in one command;
  - after the window scrolls;
  - after another buffer is shown in the window, or another window is selected.
- **Not when:**
  - typing or editing;
  - isearch's steps, which have their own current match;
  - in a prompt or a float.
- **What:** a band from the cursor to the window's right edge, on the cursor's row, in
  `Face::Flash`, fading out over about 0.4 s. It is a rectangle drawn last among the
  fills, under the text.
- **It ends at the next key,** or when the window is hidden. Only the primary cursor
  flashes.
- **Cost:**
  - While it fades, the app asks for a frame at each display refresh through the event
    loop's wake at a set time (`wake_at`, which both window layers have). This is about
    24 frames at 60 Hz, each drawing what is shaped already.
  - Nothing is reshaped, as only a rectangle's alpha changes.
  - At rest it asks for nothing.
  - A key during the fade ends it first, so typing never waits on it.
- **With soft wrap,** a large jump counts display rows.

### Line numbers

- **Absolute numbers,** right-aligned in a gutter at the window's left. The current line's
  number is in `Face::LineNumberCurrent`, the others in `Face::LineNumber`. Rows past the
  buffer's end are blank.
- **The gutter is a region of the window's layout:**
  - `frame::layout` gives each window a gutter beside its text area while line numbers are
    on ([ADR 0057](0057-frame-layout.md)). Positions, scrolling, the input method's
    rectangles and the cursor keep using the text area, so none of them changes.
  - The gutter's width is the digits of the buffer's line count plus one cell of space,
    at least 3 digits. It changes only when the count gains or loses a digit. The text
    area then loses or gains a column, and the rows in view are shaped again once.
- **Its rows are spans,** like the modeline's ([ADR 0063](0063-modeline-echo-area-and-internal-buffers.md)):
  - numbers are short pieces kept by their text, so a number already shaped costs nothing
    ([ADR 0068](0068-cut-clusters-short-pieces-the-cursor-box-and-lines-by-direction.md));
  - the gutter's spans are built again only when the scroll, the rows in view, the line
    count or the current line change.
- **With soft wrap,** a logical line's number is on its first row, and the rows it
  continues on are blank.

### Turning them on and off

- **Each aid is an `M-x` command** named after its Emacs counterpart:
  - `whitespace-mode`
  - `global-hl-line-mode`
  - `beacon-mode`
  - `display-line-numbers-mode`
- **Scope:** each toggles the aid for every buffer and window, and says in the echo area
  whether it is now on or off.
- **No key is bound.** The toggle menu (M3) will reach them.
- **Nothing is remembered:** every start begins with ADR 0026's defaults, and lasting
  choices belong to M3's configuration. An `oso dev` restart carries them in its handoff,
  as it carries the theme.
- **Reach `Look`:** they change only what is drawn, and nothing on disk
  ([ADR 0085](0085-agent-channel-and-oso-mcp.md)). `get-state` reports which aids are on.

### Where they apply

- They apply to the tiled windows, every buffer they show, internal buffers too.
- Floats (prompts, completion, questions) have none. Their rows are not a buffer the
  person edits.
- In a buffer that may hold secrets they are drawn as in any other. What `get-state` and
  the agent channel withhold there stays withheld: the band's and the gutter's rows are
  positions, and are left out with the cursor's.

### Faces and contrast

- **New faces and their tiers**, added to every theme ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)):

  | Face | Tier |
  |---|---|
  | `Whitespace` | 3:1 on the background and on the current line: marks to notice, not text to read |
  | `TrailingWhitespace` | a fill |
  | `CurrentLine` | a fill; text keeps 7:1 on it, escapes 4.5:1 |
  | `Flash` | a fill |
  | `LineNumber` | 4.5:1 on the background |
  | `LineNumberCurrent` | 7:1 on the background |

- **Telling fills apart:** `CurrentLine` stands apart from the background by the minimum
  OKLab lightness step. The region, isearch's matches and the trailing fill stand apart
  from `CurrentLine`, as from the background.
- **Picking the colours:** they are picked as M2-23's were, from sample frames for each
  theme drawn off screen (`theme_samples`), which gain the aids.

## Consequences

- **Typing costs:** one more rectangle for the band, and the trailing fill's scan of the
  rows in view, each bounded by the screen.
- **The whitespace marks** are part of the shaped row, as escapes are. Turning
  `whitespace-mode` on or off shapes the rows in view again, once.
- **The flash** draws frames only while it fades, and never while a key waits.
- **The gutter** narrows the text area by its width. A buffer that reaches another digit
  of lines reshapes the rows in view once.
- **Code to come:**
  - `CellStyle::Whitespace`, and the labels in the display model;
  - `Editor`'s trailing-whitespace cells and the aids' switches;
  - the gutter in `frame::layout`, and the gutter pane in the app;
  - the flash's timer in the app;
  - six faces in `oso-render::theme`;
  - four commands;
  - snapshot marks for whitespace cells.
  No new crate, crate edge, dependency or `unsafe`.

## Alternatives considered

- **Half-width spaces as `·` too:** shows every space, at the cost of a busy screen.
- **Trailing whitespace marked on the cursor's line too:** the fill would flash under each
  space typed at a line's end.
- **A band on every cursor's line:** with many cursors, the band would cover much of the
  screen and hide which cursor is primary.
- **Line numbers in place of a band:** the current line would show only while numbers are
  on.
- **The cursor box blinking twice:** a smaller sign, and a blink is what ADR 0026's
  non-blinking cursor avoids.
- **Flashing only on a switch or a scroll:** a long `M->` or `M-g g` would not be marked.
- **Relative numbers, or the current line absolute and the rest relative:** every cursor
  motion would change every number in view.
- **A gutter of fixed width:** it wastes columns on small files and cuts numbers past
  9999.
- **Keys bound now:** few keys are free, and the toggle menu is where these belong.
- **Per-buffer switches:** closer to Emacs's minor modes, but more to explain until
  configuration exists.
- **Remembering the switches, as the theme is:** that is configuration's job. The theme is
  remembered because the person picks it, where the aids keep defaults.
