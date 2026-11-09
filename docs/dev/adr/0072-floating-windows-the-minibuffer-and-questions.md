# 0072. Floating windows, the minibuffer and questions

- Status: Accepted, amended by 0085, 0076 and 0090
- Date: 2026-11-08

## Context

M2-16 builds floating windows, minibuffer input, reading a command's arguments, yes/no questions and
the `C-g` chain ([roadmap](../roadmap.md)). Earlier ADRs fixed the outline and left choices here:

- [ADR 0008](0008-floating-ui.md): the minibuffer is a buffer in a floating window, and editing
  commands work in it unchanged. [ADR 0026](0026-m2-ui-layout.md): it floats at the top center, as the
  author's vertico-posframe does today, because Emacs's minibuffer at the bottom is hard to see.
  Menus float at the center.
- [ADR 0057](0057-frame-layout.md): floats go above the frame's regions and may cover them. `FrameLayout`
  holds no floats. `Editor::cursor_area` and `text_view` assume the selected window is the tiled one.
- [ADR 0061](0061-panes-clipping-and-layers.md): a float is a pane in a higher layer with a `background`.
  M2-16 picks the layer numbers. A float on the frame's top row must decide about the top margin (8 px
  above row 0), as its background stops at its region and the band above row 0 is shared. Panes below
  the top row lose the band above their first row, and M2-16 decides what its own do.
- [ADR 0063](0063-modeline-echo-area-and-internal-buffers.md): `Buffer::is_internal` also means read-only
  with no undo, and the minibuffer's buffer is internal but takes typing.
- [ADR 0059](0059-safe-save-and-the-exit-guard.md): the exit guard is "repeat to exit anyway" until a
  prompt replaces it. A buffer whose file changed on disk cannot be saved until something can ask.
- [ADR 0069](0069-macos-input-method.md): the minibuffer adds its condition to `wants_ime`.
  [ADR 0070](0070-windowing-and-input-without-winit.md), accepted on its branch, keeps floats as panes inside oso's one window, and
  has `C-g` discard a composition once oso has its own view.
- [`security.md`](../security.md): passphrase prompts never enter minibuffer history, the kill ring or
  keyboard macros.
- [ADR 0034](0034-command-registry-and-keymaps.md): commands are plain functions with no state of their
  own, and `Args` is a small `Copy` value. A command cannot stop halfway and wait for a key, as Emacs's
  `read-string` does by running a recursive edit on the C stack: the UI thread must not wait.

M2-17 (completion, orderless, 18 candidate rows, annotations, `M-x`), M2-18 (`C-x C-f`), M2-19 (`C-x b`
with a preview inside the popup), M2-20 (isearch), M2-21 (multiple cursors), M3's menus and migemo, and
the author's consult and embark habits all build on this step.

vertico-posframe 0.9.2, which the author uses, shows the minibuffer's own buffer, prompt and candidates in
a child frame, with a 2-pixel border, at least 62% of the frame's width and fitted to its content. Its
default place is the frame's center, and its top-center handler puts the frame flush with the top edge.
It shrinks the real minibuffer window to nothing. Each recursive minibuffer gets its own posframe, with a
border color per depth.

## Decision

The author accepted this ADR on 2026-11-08, with the control socket's new fields, `minibuffer`'s
line in `guardrails/layers.txt`, the public API it adds (among it `Bands` and `Pane::bands`) and the
reported-only `prompt_type_p99_ms`.

The author chose each UI choice below on 2026-11-08, from options put to them with text-grid
mock-ups: where the float sits and how it looks, its width, the window beneath, which keys answer a
question and where it shows, the exit guard, the first questions, nested prompts, messages in the
float, the history and secret buffers, passphrases, a restart while a prompt is open, the wheel, a
covered cursor, long input and the IME. They are marked "(chosen)" where they appear.

### A float

- **A prompt is a window of its own over the tiled ones.** Its window goes after the tiled windows in
  `Editor::windows`, and is the selected one while it is open, so commands act on its buffer unchanged
  (ADR 0008). The tiled window it came from is the *window beneath*: `Editor::tiled` gives it, and the
  modeline, the wheel and `get-state` speak of it.
- **Its place is a pure function,** `frame::float_regions(layout, rows)`: from the frame's layout and
  the rows of content, the outer cells with the ring and the inner cells. `FrameLayout` keeps its `Copy`
  and holds no floats, as ADR 0057 wanted. `minibuffer::layout` places the label, the answer and a
  message in it, wrapping the label once. `Editor` keeps the result for the innermost prompt, worked
  out again whenever the frame, the prompt or its message changes (`relayout_prompt`), so a frame or an
  event only reads it. `Editor::cursor_area` and `set_frame_size` know the prompt's window, and
  `Editor::prompt_windows` lists the prompts' windows.
- **One row below the top of the frame (chosen),** so row 0 of the window beneath stays in view above
  it. A float off the frame's top row leaves the margin question of ADR 0061 alone, so the renderer
  needed no change for it. A frame too short for the row above puts the float on row 0, where ADR
  0061's rule holds as it is: the band of the hidden row 0 may show in the margin above the border.
- **60% of the frame's columns, at least 40, and never closer than 2 columns to an edge (chosen).** It
  does not change while typing or with the length of the prompt.
- **A border and one cell of padding on every side (chosen).** The ring is one cell, and the border is
  a line along its outer edge, twice a thin line's width (2 px on cells 20 px high, about 2 points on a
  Retina display), so a prompt is 3 rows tall. `Pane::border` draws the four sides inside the region.
- **A small frame gives up the row above first, then the ring.** A float is never taller than the text
  area, so the modeline and the echo row stay in view, and a frame of one row has the float over it.
  A label taller than the float keeps its last row, with the answer or `(y or n)`.
- **Two layers.** `Layer::FLOAT` (16) holds the float's background and border, one pane over all of it.
  `Layer::FLOAT_CONTENT` (17) holds what is inside, in panes that never overlap one another: up to two of
  the label, the answer and a message, with fixed ids 3 to 7 after the tiled 0 to 2. **Panes in a float
  draw only inside it:** each pane says which bands of ADR 0052 its glyphs keep past its region
  (`Pane::bands`), and a pane in a float keeps one only where its region borders on the padding, so
  that no glyph draws over another pane of the float or outside it. A float with no ring, in a small
  frame, keeps none. Rows of one pane keep their bands between them, as any pane's do. M3's menus
  take layers above these, and give their panes bands as they decide.
- **Faces:** `Face::Float` (the float's background and its text), `Face::FloatBorder` and
  `Face::Prompt` (the label), with constants until the theme (M2-23), which checks their contrast.

### The minibuffer

- **Each prompt gets a new internal buffer, dropped as it closes,** with its window. No text, undo or
  cursor carries from one prompt to the next. *Internal* now means only that the buffer is not the
  person's work, so `Editor::buffers` still leaves it out and the exit guard, the emergency save and
  the handoff need nothing new. A prompt's buffer takes typing and undo (`Buffer::prompt`), while
  `*Messages*` stays read-only (`ReadOnly::Internal`).
- **The label is not in the buffer,** so the buffer's text is the answer, `C-a` goes to its start and
  history holds answers only. It is drawn in its own panes. A label longer than the float's width less
  20 columns breaks after a space, or within a word longer than a row, onto at most three rows, and is
  cut with `…` after that. A question takes the whole width, as nothing is typed.
- **The answer is one row and scrolls sideways (chosen),** as any window does.
- **Commands work in it unchanged** (ADR 0008), with one rule: a command that changes which buffer a
  window shows (`C-h e`, `C-h r`, later `C-x b`) refuses there, "Not in the minibuffer", and the
  editor's `toggle_messages` and `toggle_copies` do nothing while a prompt is open, so the rule holds
  in `oso-core` whatever calls them. A command that
  needs a file fails there as on any buffer with none. What shows a buffer from outside a command, such as the
  socket's `open-file` and `show-buffer`, shows it in the window beneath, and a load there asks for
  its frames as before.
- **Keys:** a keymap in the major-mode layer while a prompt that reads an answer is selected, over the
  global keymap:

  | Key | Command | |
  |---|---|---|
  | `RET` | `exit-minibuffer` | Gives the answer to what asked. A bad number stays and says so. |
  | `C-g` | `keyboard-quit` | The chain below, which closes the prompt. |
  | `M-p` | `previous-history-element` | The prompt's history, older. |
  | `M-n` | `next-history-element` | Newer, and back to what was typed. |

  `TAB` is unbound until M2-17 gives it completion, and `C-n` and `C-p` move as in any one-row buffer
  until M2-17 gives them candidates. `ESC` is no quit key.
- **Modal:** while a prompt is open, keys go to it. There is one tiled window until M3.
- **The wheel scrolls the window beneath and leaves its cursors (chosen),** as during a composition.

### History

- **Each prompt names a history, or none.** A history is a list in `Editor`, in memory only, never
  written to disk or carried through a restart (ADR 0013, ADR 0051).
- **At most 100 answers, and an answer given again moves to the front (chosen).** An answer over 4096
  bytes, or an empty one, is not recorded.
- **A prompt opened while a buffer that may hold secrets is shown records nothing (chosen),** and walks
  no history, and neither does one answered while such a buffer is shown beneath it. A passphrase never
  enters one.

### Reading a command's arguments

- **A command declares what it reads, and the dispatcher reads it before running it,** as Emacs's
  `interactive` does. `ArgSpec` gains `Number { prompt, history }` and `Passphrase { prompt }`. M2-17 to
  M2-19 add what they read. The command stays a plain function and never waits.
- **The dispatcher keeps the waiting command** with its prefix argument and the prompt's id
  (`PromptId`, which `Editor::open_prompt` returns and `Outcome::Answered` carries). When
  that prompt is answered, the command runs with the answer through the same `execute` as a key, so
  undo, the messages around a secret buffer and the emergency save see one command. A prompt that closes
  with no answer drops what waited for it.
- **`Args` stays `Copy`:** a command reads its answer through `ctx.answer()`, or `ctx.number()` for a
  number, parsed as the prompt checked it (`minibuffer::parse_number`), as typing reads its text
  through `ctx.input()` (ADR 0051), so no command's signature changes.
- **A number typed as a prefix argument stands for the answer:** `C-u 42 M-g g` and `M-4 M-2 M-g g` go
  to line 42 with no prompt. `C-u` alone says nothing of a number and opens the prompt.
- **`run-command` gives the answer in its `arg`,** as it gives a count, and refuses a command that
  reads a number without one, or a passphrase at all. It never opens a prompt that waits for a person to
  read an argument. A flow tests the prompt with `send-keys`.
- `goto-line` on `M-g g` and `M-g M-g` is the first such command. As `M-<` does, it sets the mark
  where the cursors were (ADR 0071). `M-x` waits for M2-17.

### Questions

- **A command that has to ask ends, and leaves a question** through `Editor::ask`: its text, and the
  name of the command `y` runs, confirmed. `n` runs nothing. A name that is no command is a bug, which
  the log and the echo area say. No closure is kept, so commands keep no
  state, and the answer runs a command like any other, which reads `ctx.confirmed()`.
- **The command `y` runs checks again,** and asks again if more is at risk than the question said.
- **Only `y` and `n` answer (chosen);** `C-g` closes the question and runs nothing. `SPC`, `DEL` and
  `RET` do not answer, unlike Emacs's `y-or-n-p`, as prose types them most. Any other key says
  `Answer y or n` in the float and leaves the question open. A question's keymap takes the global
  keymap's place, so no other key reaches a command, and text from an input method answers nothing
  and says `Answer y or n` too. A held key answers once: its repeats are dropped, as for the
  emergency copies' letters.
- **A question shows in the same float at the top center (chosen),** ending in `(y or n)`.
- **A question opens only from a command,** run by a key or by the socket, never from a background
  result such as a finished save, so a `y` typed in prose cannot answer a question that appeared under
  the person's fingers. A background result says what it found and which key asks. **A question a
  control socket request opened is answered over the socket:** keys from the window say so and leave
  it open, but `C-g`, so an agent's question cannot take a person's typing as its answer. The app tells
  such a question by its id, newer than any prompt open before the request.
- **The exit guard asks (chosen):** `C-x C-c` and the close button, with a buffer modified or a save
  running, ask `2 modified buffers and 1 save running; exit without saving? (y or n)`. `y` exits as the
  repeat of ADR 0059 did, unless a save has since failed or left its buffer modified, or a buffer the
  question did not cover is at risk, when it asks again: the person agrees to lose those buffers and
  no other. The close button runs the same command through the dispatcher
  (`Dispatcher::close_requested`), dropping a prefix key or argument in progress as a key would, and
  pressed while the question is open does nothing more.
  `ExitAction` goes.
- **Saving over a file changed on disk asks (chosen).** A save that finds the regular file changed
  keeps it, as before, and remembers what it found there. Its message ends "C-x C-s asks to save over
  it". The next `C-x C-s` asks `notes.md changed on disk since it was read; save over it? (y or n)`, and
  `y` saves with what that save found as the baseline, so the file is replaced only if it is still that
  file, and only for the buffer the question was asked of. A change made after the question is kept,
  and asked about again. A file gone meanwhile comes back with the mode it had when read, not the one
  found. The name in the question is cut to 40 cells first, so that the question's own words and its
  `(y or n)` always show. A later failed save that finds no regular file there forgets the file found,
  and an unmodified buffer asks nothing: its save writes only if the file has gone. A path that now leads elsewhere,
  or a file that is no regular file, is refused as before. A new baseline forgets what was found.

### Nested prompts

- **A prompt can open from a prompt (chosen),** up to 8 deep. `C-x C-c` in a prompt asks its question,
  and M2-17's embark-like actions will need this. One more says `Too many prompts open` in the float.
- **Only the innermost shows, at the same place, with its depth first: `[2]`.** The others wait,
  hidden, as they were.
- **The innermost takes the keys, and `C-g` closes it alone.**

### The `C-g` chain

**Each `C-g` does one thing, the first that applies, and says "Quit":**

1. a composition, under ADR 0070's own view: discards it;
2. a pending prefix key or prefix argument: cancels it, in `KeyResolver`, as before;
3. an active region in the selected window, on every cursor (M2-13);
4. the innermost prompt or question: closes it, and runs nothing;
5. isearch (M2-20), its own two steps;
6. extra cursors (M2-21);
7. otherwise nothing more.

Steps 3 to 7 are one function, `Editor::quit`, which returns what it did (`Quit::Region`,
`Quit::Prompt` or `Quit::Nothing`). `keyboard-quit` is a thin
call into it in every keymap, the prompt's too, so M2-13, M2-20 and M2-21 add their step there and
nowhere else.

### The echo area and messages

- **The echo row stays at the bottom, for messages and keys in progress** (ADR 0063).
- **A message from a command run in a prompt shows in the float (chosen),** at the end of the answer's
  row, in brackets: `Go to line: 42 [Beginning of buffer]`. It leaves the answer at least 8 columns,
  and one that does not fit is cut with `…]`. It goes at the next key, as the echo area's does, and
  reaches `*Messages*` and the log by the same rules. A message from elsewhere goes to the echo row.
  So "Quit" from closing a nested prompt shows in the outer one, and from the outermost on the echo row.

### What the window beneath shows

- **Its text, its modeline and its updates, as before.**
- **Its cursor is drawn hollow (chosen),** an outline over glyphs as they are (`CursorStyle::Hollow`),
  and it shows no preedit.
- **A float may cover its cursor, and the view does not move (chosen).**

### The input method

- **`wants_ime` gains one condition: a prompt is selected (chosen),** as `ui.md` and ADR 0026 decided.
  Turning off waits for a composition to end, or commits it under ADR 0070.
- **When the prompt closes, the same function decides again** for the window beneath.
- **No prompt in M2-16 reads text in Japanese:** `goto-line` reads digits and questions one letter.
  Whether a key such as かな turns the IME on for one prompt is decided with M2-18's file names, after
  trying at the keyboard whether that key reaches oso while the IME is off.
- `Editor::cursor_area` gives the prompt's cursor while one is selected.

### Passphrases and secrets

- **A passphrase prompt's buffer may hold secrets** (`Verdict::Passphrase`), so every rule for such a
  buffer applies with nothing new: `get-state` leaves out its text and positions and the modeline and the
  echo area, screenshots are refused, keys typed there leave no timings (ADR 0060), messages around it
  stay out of the log and `*Messages*`, and the IME is off.
- **On top of that,** it keeps no undo record, so no copy of the text stays in one, enters no history,
  and is dropped as it closes, as a buffer that may hold secrets going off the screen. The command that
  gets it runs with its messages kept out of the log and `*Messages*`, and its echo out of `get-state`
  (`Editor::command_reads_passphrase`), so an error that quotes the answer goes no further than the
  screen. While a passphrase prompt is open, also beneath another prompt, `run-command` runs only
  `keyboard-quit`, as a reply could say something of what is typed there. A message its command says
  in a prompt is withheld from `get-state` there too. M2-14's kills from it must reach neither the kill ring nor the OS
  clipboard, while a yank into it works. M3's keyboard macros must not record what is typed there.
  `Debug` of what carries an answer (`Invocation`, `Outcome`, `Prompt`) prints none of it.
- **It shows one `*` per character (chosen),** in view at the cursor (`PaneContent::Masked`), counted
  by `minibuffer::mask` from the rope's summaries, which the renderer and the text-grid snapshots share.
- **The rope's chunks are freed without being overwritten.** `oso-core` has no `unsafe`, and a write
  of zeros before a free can be optimized away without a volatile write. Core dumps are already off
  (ADR 0028). Overwriting needs `zeroize` or a small `unsafe` elsewhere, which the first caller (M4's
  git) decides.
- **Built now (chosen),** with a dev-only command, `dev-read-passphrase` on `<f34>`, for the tests and
  the flows. Nothing else reads a passphrase until M4.
- **Any prompt, while a buffer that may hold secrets is shown,** leaves its label, its answer, its
  cursor and its message out of `get-state`, as a search string or a line number can say something of
  the text.

### The control socket and end-to-end flows

This is a human gate ([`REVIEW.md`](../../../REVIEW.md)), which the author approved in direction.

- **`get-state` gains `prompts`:** for each open one from the outside in, `depth`, `kind` (`number`,
  `passphrase` or `question`), `prompt`, `text` (cut at 4096 bytes), `cursor`, `message`, and `region`
  for the innermost. Its `buffer`, `text`, `cursor` and `windows` stay the tiled window's, and so
  does `get-text`.
- **`run-command`** takes the answer as `arg`, as above.
- **A dev restart waits for every prompt to close (chosen):** `check-handover`, `export-state` and a
  generation's `exit` refuse while one is open, as for a composition.
- **Flows (`just e2e`)** check the seams: `M-g g 4 2 RET` and the state on the way, `C-g`, the exit
  question answered `n` and then `y`, the IME off in a prompt and on again after it, a passphrase
  withheld from `get-state` and a screenshot, and a restart refused while a prompt is open. Drawing is in
  text-grid snapshots, which give a float's cells after the frame, its ring as a box, and mark the
  hollow cursor beneath with `o`; that a float hides what is under it is the scene tests'.

### Costs

Against the 8 ms typing budget ([ADR 0056](0056-typing.md)):

- **Opening a prompt** adds two to four panes, each with a new id and so a new `Rows`, and shapes their
  rows: about what one typed key costs. The window beneath keeps its id, so its rows are reused as
  they are, and closing the prompt reshapes nothing.
- **A key typed in a prompt** reshapes the answer's row and reuses every row of the window beneath,
  whose revision has not changed: less than typing in a file.
- **The rows under a float are still drawn,** at most the float's area again in pixels, which ADR 0061
  accepted.
- `just perf` gains `prompt_type_p99_ms`, keys typed into an open prompt, reported only, with no
  threshold (approved).

### What this leaves room for

- **M2-17:** candidates as rows below the answer in the same float, which grows down so the answer's
  row stays where it is, `M-x` through a new `ArgSpec`, and `C-n`/`C-p` on candidates. The selected
  candidate needs a background per row in `PaneContent::Lines`. Candidates from a worker thread update
  the float through the `Status` comparison, as a load's progress does.
- **M2-18 and M2-19:** their `ArgSpec`s, and the preview as a read-only `TextView` pane right of the
  candidates, in a wider float.
- **Embark-like actions:** nested prompts. **M3:** menus at the center in layers above these.

### Where it lives

- **`oso-core`:** a new public module, `minibuffer` (kinds, prompts, outcomes, histories and the layout
  inside a float), on a line of its own in `guardrails/layers.txt` between `editor` and `frame`;
  `editor/prompts.rs` (opening, closing, answers, history and `Editor::quit`); `frame::float_regions`;
  `ArgSpec`'s variants and `Ctx::answer`/`Ctx::confirmed`; `Buffer::prompt`; `Verdict::Passphrase`;
  `Editor::start_save_over` and `Editor::may_save_over`.
- **`oso-render`:** `Pane::border`, `Pane::bands` and `Bands`, `CursorStyle`, `PaneContent::Masked`, three `Face` variants,
  `Layer::FLOAT` and `Layer::FLOAT_CONTENT`, and the band below in `Layout`.
- **`oso`:** `commands/minibuffer.rs` and `bindings/minibuffer.rs`; `goto-line`; `dispatch/prompts.rs`
  (the waiting commands and what an answer runs); the panes in `app/draw/float.rs`; the condition in
  `app/ime.rs`; the socket's field in `dev`.
- No new crate, crate edge, dependency, `unsafe` or public trait.

## Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0034](0034-command-registry-and-keymaps.md): `ArgSpec` also describes an argument to read, which
  the dispatcher reads before the command runs, and `Invocation` carries the answer and whether a
  question confirmed it. `run-command` gives a number as the answer. `C-g` in a prompt is
  `keyboard-quit`, whose chain is in `Editor::quit`.
- [ADR 0057](0057-frame-layout.md): floats are placed by `frame::float_regions`, outside `FrameLayout`,
  and the selected window may be a prompt's.
- [ADR 0059](0059-safe-save-and-the-exit-guard.md): the exit guard asks instead of waiting for a repeat,
  and keys no longer reset it; `ExitAction` goes. A save that found its file changed can go over it once
  asked.
- [ADR 0061](0061-panes-clipping-and-layers.md): `Pane` gains a border, a view's cursor a style and
  content a masked row, and panes inside a float keep the bands above and below them.
- [ADR 0063](0063-modeline-echo-area-and-internal-buffers.md): `is_internal` no longer means read-only and
  no undo, and a message from a command run in a prompt shows in the float.
- [ADR 0069](0069-macos-input-method.md): a key held in a question answers once, as in the copies list.
- `ui.md`: where the prompt sits, how it looks, and questions.

## Consequences

- Commands ask through the dispatcher or a question and never wait, so a command is tested as before,
  with its answer given.
- A key that opens a prompt draws a frame with two to four new panes, and closing it draws the window
  beneath unchanged.
- Row 0 of the text shows above a prompt, and rows 1 to 3 are hidden under it.
- `y` answers a question at once. `SPC` and `RET` do not, unlike in Emacs.
- `C-x C-c` with unsaved work asks instead of waiting for a repeat, and the close button asks the same.
- A buffer whose file changed on disk can be saved over once asked, which the open question for M2-16 in
  `open-questions.md` wanted. A file owned by another user, or with hard links, still cannot be saved.
- `M-p` gives nothing in a prompt opened from a buffer that may hold secrets, also for `goto-line`.
- Japanese cannot be typed in a prompt until M2-18 decides otherwise.
- A dev restart can wait on a prompt left open.
- `get-state` keeps describing the tiled window while a prompt is open, and lists only tiled windows.
- `oso-core`'s public API gains the `minibuffer` module, `frame::float_regions` and `FloatRegions`,
  `Editor`'s prompt methods, `Editor::quit` and `Quit`, `Editor::tiled`, `Editor::start_save_over`,
  `Editor::may_save_over`, `Editor::command_reads_passphrase`, `Editor::echo_withheld`,
  `SaveRefused::ChangedOnDisk`, `ArgSpec`'s two variants, `Ctx::answer`, `Ctx::number`,
  `Ctx::confirmed`, `Invocation`'s two fields and `Verdict::Passphrase`, and loses `ExitAction` and
  `Editor::exit_attempt`. `oso-render`'s gains `Pane::border`, `Pane::bands`, `Bands`, `CursorStyle`, `Masked`, `Layer`'s two
  constants and three `Face` variants.

## Alternatives considered

- **A float flush with the top row, its background also covering the margin above it:** looks attached to
  the window's top, at the cost of the one place where a pane paints outside its region.
- **Flush with the top row and nothing more:** the cheapest, but the accents of the hidden row 0 can show
  in the margin above the border.
- **A border with no padding rows:** a one-row prompt, but the border runs along the input's top and bottom
  pixels, and accents and descenders touch it.
- **No border, only a background:** the lightest, but it reads as a float only as far as the theme's two
  backgrounds differ.
- **A fixed 80 columns:** steady across frames, but too narrow on a wide frame for M2-19's preview.
- **The frame's whole width, as a bar over row 0:** the most room for a path, but it is Emacs's minibuffer
  moved to the top, not the float of ADR 0026.
- **Width fitted to the content, as posframe does above its floor:** the float would grow and shrink while
  typing and as candidates change.
- **A prompt that grows in rows for a long input:** shows all of a path, but the candidate rows below it
  would move.
- **All messages on the echo row, also while a prompt is open:** one place for messages, but the bottom of
  the frame is where the author does not look while answering at the top.
- **The window beneath unchanged, or dimmed:** unchanged shows two solid cursors; dimming needs blending
  and makes the text harder to read while answering.
- **Scrolling the window beneath so the float never covers its cursor:** the cursor always shows, but the
  view jumps with every prompt.
- **Emacs's `y-or-n-p`, with `SPC` and `DEL`:** familiar, but the space bar is what prose types most.
- **`yes RET` for questions that can lose text,** as Emacs's `yes-or-no-p`: harder to answer by accident,
  and slower every time. The exit guard today needs two deliberate key sequences, which `C-x C-c y` keeps.
- **Questions at the center, where menus go, with their keys spelled out:** clearer for a new reader, but
  a second place to look.
- **Questions on the echo row:** Emacs's place, which ADR 0026 moved away from.
- **Keeping the repeat for the exit guard:** nothing to build, but the message says less than a question.
- **Asking per buffer, as Emacs's `save-some-buffers`:** saves from the guard itself, but each save is a
  background job (ADR 0059) that can fail after the answer, and the exit would have to wait for each.
- **Refusing a prompt from a prompt, as Emacs does by default:** simpler, but the exit guard could not ask
  from a prompt, and embark-like actions need it.
- **Showing every open prompt, the inner a row lower:** shows the context, but covers more text, and two
  inputs look alike.
- **A new prompt closing the one before:** loses what was being typed.
- **Commands that wait for an answer, as `async` functions or with a closure kept for the answer:** reads
  like Emacs Lisp, but a closure keeps state, which the hard rules forbid, and an `async` command needs an
  executor on the UI thread.
- **`Args` holding the answers, with a lifetime:** explicit, but every command's signature changes.
- **`run-command` opening the prompt when answers are missing:** a socket client could leave the instance
  waiting for a person.
- **One buffer per depth, emptied and reused, as Emacs's ` *Minibuf-1*`:** saves an allocation per prompt,
  but text and undo can outlive the prompt.
- **The prompt text as read-only text in the buffer, as in Emacs:** one pane fewer, but every command has
  to step around it, and history would have to strip it.
- **Keeping duplicates in history, as Emacs's default:** cheaper by a scan of 100 entries, and `M-p` steps
  through repeats.
- **History in a prompt opened from a secret buffer, unless the prompt reads a passphrase:** convenient,
  but M2-20 and M3 would have to remember to exclude search strings.
- **The IME on in prompts that read free text:** Japanese file names and search strings would work, but
  ADR 0026 chose off, and migemo (M3) searches Japanese from romaji.
- **Nothing shown while a passphrase is typed, as `ssh` does:** reveals no length to someone watching, but
  a person cannot tell whether a key arrived.
- **Building the passphrase kind with its first caller (M4):** less code now, but its rules touch history,
  the kill ring and the socket, which are cheapest to fix while they are being built.
- **A restart that closes an open prompt:** never holds a restart, but takes the prompt away mid-answer.
- **The wheel doing nothing while a prompt is open:** simpler, but a person cannot look further in the
  text while answering.
- **One layer for a float, with the border as a pane around the content:** the content panes would overlap
  the chrome, which one layer forbids.
