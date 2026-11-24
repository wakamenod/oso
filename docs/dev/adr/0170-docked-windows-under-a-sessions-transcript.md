# 0170. Docked windows under a session's transcript

- Status: Accepted, amended by 0180 and 0182; "An empty composer's placeholder, as amended" (2026-11-24); "The composer's rules, as amended" (2026-11-24); "The composer's padding and the footer, as amended" (2026-11-24)
- Date: 2026-11-22

## Context

A native session ([ADR 0165](0165-native-claude-sessions-the-route.md)) is read in its buffer,
`*claude: NAME*`, and written to in a composer under it. ADR 0130's design, which ADR 0165 keeps,
puts the composer in "a composer window of 1 to 10 rows under it".
[`native-sessions-ux.md`](../native-sessions-ux.md) adds two more things in the same place:
- the permission card, docked between the transcript and the composer (U1, step 2);
- the to-do strip, pinned above the composer (U3, step 3).

oso has no way to hold a window at a number of rows:

- **Splits keep weights.** A window split off ([ADR 0112](0112-window-splits.md)) is half its
  parent. One made a number of rows tall, as ADR 0167's commit list is (`Splits::split_aside`),
  keeps its share of the rows, not its count, once the frame is resized.
- **A split is a window of its own.** `C-x b` elsewhere leaves the composer showing the old
  session, and `C-x 1` closes it; the session's commands would have to make it again.
- **The closest precedent is the review's draft prompt** (ADR 0122). It grows from 1 to 10 rows
  with its wrapped text (`minibuffer::draft`, `MAX_DRAFT_ROWS`). But it is a prompt: modal, and
  stacked with the agent's and the permission questions.

On 2026-11-22 the author chose docks in the session's own window (Q-C1 (a)). Top to bottom they
are the card, the to-do strip and the composer. `i` in the transcript goes to the composer,
`C-c C-o` comes back, and `C-x o` steps through the docks too (Q-C2 (a)). This ADR designs the
docks.

The facts it builds on (`frame`, `window`, `draw`):

- A tiled window's rect becomes its regions in one place, `window_regions`:
  - gutter, text, a one-row modeline and a divider;
  - its text rows are its `ViewSize`, which scrolling, wrapping and the screen read.
- Each window in the frame's layout is drawn as panes: text, signs, gutter, modeline and
  divider, each with its own id (`pane_id`). Panes on one layer never overlap ([ADR 0061](0061-panes-clipping-and-layers.md)).
- Keys go to the selected window's buffer, and commands act on it. A click selects the window
  whose text is under it. `C-x o` cycles the windows that have cells, the side window apart.

## Decision

### A docked window

- **A docked window is a window of its own,** with its buffer, selections, scroll and wrap,
  that the split tree does not hold. It is laid out inside a tiled window, its host, from rows
  the host gives up at its bottom, above the host's modeline. Everything that works on a
  window works on it: keys, commands, the cursor, soft wrap ([ADR 0104](0104-soft-wrap-drawn-in-view-rows.md)),
  search, the mark.
- **Docks belong to a buffer, not to a window.** The host buffer is the one whose windows get
  them; today only a native session's buffer. It has at most three slots, top to bottom:
  - `Card`: the permission card, step 2's part 2e-2;
  - `Strip`: the to-do strip, step 3;
  - `Composer`.

  The host says what each slot shows, or that it is empty.
- **One window carries a buffer's docks:** the one that shows the host buffer and was selected
  last (D1).
  - If the host buffer leaves that window, its docks go to the next window that shows it.
  - If no window shows it, the docks are hidden.
  - A hidden docked window keeps its selections, size, scroll and text, and comes back as it
    was once the buffer is shown again; the layout neither sizes nor scrolls it meanwhile.
    Killing the host buffer, or the docked buffer, removes the dock.
- **Not in the tree, not carried.** A docked window is not in the split tree or in the `Tiling`
  a restart hands over. After a restart, a session's docks come back when its buffer is shown
  and its host docks them again. A space's tree that comes in or goes out (ADR 0121) leaves
  docked windows in the frame, as it leaves the side window: they go where their host is shown.

### Rows

- **Heights:**
  - The composer is as many rows as its text wraps to, from 1 to 10, as the draft prompt is.
  - The card and the strip are as many rows as their text has, at most 12 and 6
    (native-sessions-ux.md's card, and U3's opened strip).
  - Each is measured again as its text changes, and the host's layout is redone at once.
- **No modeline and no divider** (D3). A composer has a row above and below its text, which its
  rules are drawn in, and the window that carries the docks a footer row under them (amended,
  below).
  - The composer's gutter shows `›` on its first row, in the gutter's face, and nothing on the
    rows below.
  - The card draws its own frame as text, and the strip is one line.
  - The host's modeline stays its own last row, under the docks.
- **When the host is short** (D2):
  - The host keeps at least two rows of text, and the composer at least one.
  - The footer is given up first, then the composer's padding, both rows of it, then the strip
    and the card, from their ends (amended, below). A cut card says so on its last row.
  - A host with less room than that hides its docks. Its modeline then says `docked hidden`,
    and the composer can be reached by making the window taller.
  - A frame that grows gives the rows back.
- **Only tiled windows host.** The side window ([ADR 0121](0121-agent-sessions-and-spaces.md))
  never does: its sessions pane shows no session buffer. A float or a prompt never does
  either.

### Drawing

- **Its own panes:** a docked window is in the frame's layout after its host, with regions of
  its own (gutter and text), and is drawn as any window is, on the tiled layer. Its rect is
  carved from the host's, so no pane overlaps. It takes pane ids of its own.
- **Telling it apart:** a docked window's text is drawn over the float's face, as a prompt's
  answer is ([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md)), so the composer
  reads as a place to type. No new face pair is needed (the rules' and the strip's, as amended, are
  in the theme's pairs). The card's keys as chips (ADR 0075's
  look) are part 2e-2's, and the strip's look step 3's.
- **The cursor:** it is drawn in the selected window only, docked or not. The host's cursor
  shows hollow while the composer is selected, as any unselected window's does.
  - Erratum (2026-11-24, as built): the cursor's style follows the window keys go to, not the
    active modeline, which is the host's while its docked window is selected. As first built the
    two were one, and the composer's cursor was the hollow one.

### Selecting and input

- **`C-x o`** steps through the windows in the layout's order: a host, then its docks top to
  bottom, then the next tiled window. `C-x 1` keeps the selected window's host and that host's
  docks.
- **The tree's commands act on the host,** as they act on the tree's window while the side
  window is selected (ADR 0121), and select it: from a docked window, a split splits its host,
  `C-x 1` keeps the host, `C-x 0` deletes the host (and, in the only window, changes nothing),
  and showing another buffer (`C-x b`, a visit, a review opening) shows it in the host. A docked
  window's own buffer changes only by its host's `set_dock`, which takes no host docked under
  itself and no buffer docked in another slot.
- **Up and down** (`windmove`) step from a host to its docks and back, top to bottom; left and
  right go as the host does.
- **The modeline:** while a docked window is selected, its host's modeline speaks for it, as
  the selected window's, with the frame's segments; a docked window's own warnings stay off
  the echo row.
- **A prompt over a docked window** acts on it, as on any window beneath a prompt. If the docks
  hide while the prompt is open, as when the frame shrinks, the prompt acts on the host
  instead: accepted, as nothing a prompt over the composer does is lost by it.
- **A click** in a docked window's text selects it, as a click selects any window. A wheel
  scrolls the window under the pointer.
- **`Editor::select_window`** takes a docked window's id. A command selects a slot's window by
  its host buffer and slot. Step 2's `i` and `C-c C-o` are such commands, in the native
  session's and the composer's keymaps.
- **Keys go to the selected window's buffer.** The composer's own keymap (part 2a-2) is a
  buffer mode's, as `*Brief*`'s is.
- **The buffer order** (ADR 0171): a docked window's buffer is its host's, not one a person
  goes between. It is never listed (`C-x b`, `M-,`, the buffer list), never rises, and its
  name cannot be taken by a second buffer. Selecting a docked window raises its host's buffer,
  as selecting the window it is part of would.

### Agents and secrets

- A docked window is listed in the agent channel's state as a window with `docked: {host,
  slot}`, and its text follows its buffer's own rules; one whose buffer is withheld from agents
  (ADR 0165), as the composer's and the card's are, is not listed at all. A host's area is its
  whole rect, its docks' rows too, so its size says nothing of how much the person wrote. The
  dev socket lists every docked window.
- An agent's move into a docked window (`other-window`, `windmove`) is undone, as one into the
  side window is, and its `Change` commands are refused while one is selected: what is docked
  under a session is the person's to write.
- A docked window showing a buffer that may hold secrets counts as shown, as any window does.
- **Text from Claude in a dock** (the card in 2e-2, the strip in step 3) must not pass for oso's
  own UI or for the composer: those parts fence it with a mark or face of their own, decided in
  their design, and cap the text the host docks.
- Nothing about docks reaches a log but their kind and rows.

### Public API

The author approved these items on 2026-11-22; a signature that differs much goes to the author
again before the snapshot is regenerated:

- `oso_core::window::DockSlot` (`Card`, `Strip`, `Composer`), with the usual derives.
- `Editor::set_dock(host, slot, Option<BufferId>)`, `Editor::dock_window(host, slot) ->
  Option<WindowId>`, and `Window::docked() -> Option<(BufferId, DockSlot)>`.
- `FrameLayout` lists docked windows with their regions, as it lists tiled ones.

## Questions

The author chose (a) on each, on 2026-11-22, through the space's orchestrator. The options are
kept as they were put.

1. **D1: a session shown in two windows.**
   (a) The one selected last carries the docks; the other shows the transcript only.
   (b) Each window gets docks of its own, all showing the same buffers: two cursors in one
   composer, and the card twice.
2. **D2: a short host.**
   (a) As above: the host keeps two rows and the composer one; the card and the strip give up
   rows first; with less room the docks hide and the modeline says so.
   (b) The host gives up all its rows to the docks before any dock gives up one.
3. **D3: a row between the transcript and the docks.**
   (a) None: the composer's `›` in its gutter and the faces tell them apart.
   (b) A one-row rule in the divider's face above the first dock, which costs a row.
   (a) was chosen. The rules, drawn inside the composer's rows and then in rows of their own around
   its text, are neither option (amended 2026-11-24).

## Changes to accepted ADRs

Once accepted:

- [ADR 0112](0112-window-splits.md), "Regions": a tiled window's rect also gives up rows at
  its bottom, above its modeline, to the windows docked in it. "What each window draws" covers
  docked windows too. `C-x o` steps through a host's docks after the host, and `C-x 1` keeps the
  selected window's host and its docks.
- [ADR 0057](0057-frame-layout.md): a frame's layout also lists docked
  windows, each inside its host's rect.
- [ADR 0061](0061-panes-clipping-and-layers.md): a docked
  window's panes are on the tiled layer, carved from its host's, so the rule that panes on one
  layer never overlap holds.
- [ADR 0165](0165-native-claude-sessions-the-route.md), "The buffers": the composer window is a
  composer dock of the session's buffer, with the card and the strip above it.
- [ADR 0171](0171-one-buffer-order-for-the-editor.md): a docked window's buffer is left out
  of the order's listing, and selecting a docked window raises its host's buffer instead.
- [ADR 0121](0121-agent-sessions-and-spaces.md), the side window's rules ("the tree's commands
  act on the tree's window selected last") hold for a docked window too, with its host; and a
  space's tree that moves leaves docked windows in the frame, as it leaves the side window.

## Consequences

- One mechanism holds the composer (step 2), the card (2e-2) and the strip (step 3); none of
  them is a window the person can lose.
- The composer is an ordinary editable buffer in an ordinary window: everything oso does in a
  buffer works there, and it costs a prompt nothing.
- `window_regions`, the frame's layout, `other_window`, selection and the panes' ids learn about
  docked windows. A host's outer rect runs from its text's top to its modeline's bottom, its
  docks' rows between, so splits, resizes, floats and the draft prompt measure it whole. The
  readers that assume the split tree were checked: the mouse, the channel's and the dev
  socket's state, the restart's carry and the review's hand-over, spaces' trees, the slot of
  ADR 0160 and the pairs of ADR 0122.
- The host's text loses rows while its docks show; a short window hides them rather than
  squeezing the transcript out.
- Docks are not carried over a restart: a composer's text written but not sent is lost when oso
  restarts, as a prompt's is.

## Alternatives considered

- **A split window kept at a number of rows (Q-C1 (b)):** a new kind of child in the split tree.
  It is still a window of its own that `C-x b` and `C-x 1` leave behind, and the card and the
  strip would need windows of their own.
- **A multi-line prompt (Q-C1 (c)):** the draft prompt's sizing at no layout cost, but modal. It
  stacks with the questions an agent or a session's permission asks, and has no place for the
  card or the strip.
- **Rows in the session's own buffer,** as ECC has them: one buffer for transcript and prompt,
  which native-sessions-ux.md section 1 finds clumsy. Scrolling up takes the prompt away, and a
  read-only transcript would need an editable tail.
- **A float at the window's bottom edge** (ADR 0075 at the bottom): floats cover text, are not
  windows, and take no editing.

## An empty composer's placeholder, as amended

Amended on 2026-11-24, the author's choice: the mark and the background alone did not show the
composer as a place to type. A rule above it and a stronger background were the other options, and
neither was taken then; the rules came later the same day (below).

- **While the composer is empty, it says what it is and its keys,** dim, on its first row:
  `Write to Claude · C-c C-c sends · C-c C-o transcript`. The keys are the real ones: the shortest
  that run the two commands in the composer's layers, the user's keys among them, or `M-x` and the
  name. In a narrow window its last parts are left out, whole; `Write to Claude` is cut at the
  window's edge at worst.
- **It is drawn, and is no text:** no byte of it is in the buffer, so nothing that reads, sends,
  moves over or searches the buffer meets it, nor does the agent channel.
- **While its session has a prompt suggestion, it says that instead** (amended 2026-11-24, the
  author's word; ADR 0165, part 3f): `TEXT   (C-c C-s takes it)`, the key read as the hint's are. The
  text is Claude's, so it never shows without oso's words after it, which are spans of their
  own: text alone could pass for the hint. In a narrow window the text is cut with `…` and the
  key stays whole; with no room for the key and 8 cells, the `…` among them, or the whole of a
  shorter text, the usual hint shows, and the key still takes it. It follows the rules below as the hint
  does.
- **Its pane** is the composer's one more, in the prompt's face
  ([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md)), and an exception to "a docked
  window's panes are on the tiled layer" above: it is on a layer just over them and under every
  float, so panes on one layer still never overlap. It has no background, so the current line's
  band and the flash show behind it. It starts in the cell after the cursor's.
- **When it shows:** whether the composer is selected or not, until the buffer holds anything or an
  input method composes there. A composer that a yank turned secret, which sends nothing, shows
  none.

## The composer's rules, as amended

Amended on 2026-11-24, the author's wish (ADR 0165, the look).

- **A composer has a rule above and below it:** a thin line in the prompt's dim colour along its
  first row's top and its last row's bottom, from its window's edge to its edge, its gutter among
  it, which is on the float's background as its text is. It is drawn in the composer's rows and takes none; heights, wraps and presses are as before.
  So it is where a host has no room for the composer's padding; with it, each is in a row of its own
  (amended, below).
- **It is the composer's alone:** a card, the strip, a request's line and any other buffer docked in
  the composer's place have none; a question's answer written there (ADR 0182, R2) will.
- **It is oso's alone:** the composer's text, its escapes, a text being composed and the placeholder
  are cut to inside it, and it crosses the gutter, where no text is.
- **The current line's band is behind it;** the selection, the flash and the cursor are over it, and
  so are the other fills (search's matches, trailing whitespace, dictation's span) and a jump's
  labels. While the composer is empty, its placeholder's pane, which is cut the same, draws the rule
  over the flash.
- **Nothing of it is text:** the agent channel, copying and search see nothing new.
- **Public items,** which the author approved on 2026-11-24: `oso_render::Rules` (`above` and `below`,
  each a face or none; `Rules::NONE`) and `Pane::rules`. They are a pane's, not a view's, as the
  gutter is a pane of its own.
- **What it changes:** [ADR 0061](0061-panes-clipping-and-layers.md): a pane may have lines along its
  edges, in its own cells, and its glyphs are then cut to inside them.
  [ADR 0052](0052-glyphs-stay-in-their-cells.md) and
  [ADR 0063](0063-modeline-echo-area-and-internal-buffers.md): such a pane keeps no band past a line.
  The modeline's band, which is kept, may still reach the rule below the composer over it, with an
  accent of the modeline's text.

## The composer's padding and the footer, as amended

Amended on 2026-11-24, the author's wish: the composer was far too low, and the mode and the model
belong under its lower line, as Claude Code's terminal has them (ADR 0165, the look, part G2). Of half
a row of pixels, a blank row each, one blank row below and the terminal's own way, the author chose
the last, from shots of the app.

- **A composer has a row above and below its text,** its padding, over its window's whole width. Each
  holds one of its rules along its middle: half a row of the float's background within it, half a row
  of the text's beyond it. The rows are the window's and hold no text: its text, wraps, scrolling,
  cursor, presses, band, placeholder and preedit are in its text's cells as before, and a wheel over
  them scrolls the composer. They are not among its 10 rows. An empty composer takes 3 rows, and 4
  with the footer, as the terminal's does.
- **It is the composer's alone,** as its rules are: no card, strip, request's line nor any other
  buffer docked in its place has it; a question's answer written there (ADR 0182, R2) will.
- **The footer:** a host whose buffer has one has a row under its docks, above its modeline, in the
  one window that carries them. The host draws it on the text's background; it is no window and no
  buffer, so nothing that goes to a window, reads, copies or searches a buffer meets it, nor does the
  agent channel. A wheel over it scrolls the host. What it says is ADR 0165's, part G2.
- **The modeline of a window that shows a footer** says a note of its own, without what the footer
  says; every other window of the buffer says the whole, and so does that one while a prompt's float
  covers its footer.
- **A short host** gives up the footer first, then the padding, both rows or none, then the strip and
  the card as before: the footer and the padding are given from what every dock's rows leave over,
  past the host's two. So a tall draft takes their rows before it scrolls. Where there is no padding,
  the rules are in the composer's own rows, as above. What hides the docks is as before. With a strip
  of one row and an empty composer, a host of 7 rows has all, 6 no footer, 5 no padding, and 3 no
  strip. A host of 7 then keeps 2 rows of its own.
- **A card that grows** (ADR 0180) leaves the padding and the footer their rows, whether they show or
  not, so that its room does not depend on its own rows.
- **A request's line in the composer's place** has neither padding nor rules: the transcript grows by
  3 rows as point goes to the line, and shrinks by them as the composer has its place back.
- **Public items,** which the author approved on 2026-11-24: `Editor::set_footer(host, note)`,
  `FrameLayout::footer(host)`, `FrameLayout::padding(docked)`, and
  `oso_render::PaneContent::Rule { ink, above, below }`.
- **What it changes:** [ADR 0061](0061-panes-clipping-and-layers.md): a pane may be one line along its
  middle between two backgrounds. [ADR 0063](0063-modeline-echo-area-and-internal-buffers.md): the
  footer, a pane of one row, keeps its band on the lower rule's row, below its line. [ADR 0165](0165-native-claude-sessions-the-route.md), parts 3a and
  3b; [ADR 0181](0181-remote-control-of-native-sessions.md), "The mark";
  [ADR 0182](0182-a-native-sessions-requests-in-its-conversation.md), "The line".
- **Erratum:** `set_dock` with another buffer in a slot left its window docked nowhere, where it is to
  show that buffer in the same window. It stays docked now.
- **Not chosen:** a footer as a dock of its own (`DockSlot::Footer`), which would be a window that
  `C-x o` goes to and every state lists, with a buffer and a mode. Text drawn half a cell off the
  grid, which would cost one row less, and an offset in every place that turns a cell into pixels.
