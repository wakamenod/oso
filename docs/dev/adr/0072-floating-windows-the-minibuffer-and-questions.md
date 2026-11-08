# 0072. Floating windows, the minibuffer and questions

- Status: Proposed
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

Marked "(to ask)": a UI choice put to the author, with the recommended option written here. The options and
mock-ups are in the pull request. The rest follows from accepted ADRs, or is a design choice the author may
still change.

### A float

- **A float is a window of its own over the tiled ones.** It has a buffer, a selection set with one cursor,
  and a scroll, as any window does. `Editor` holds the floats apart from the tiled windows. While one is
  open it is the selected window, so commands act on its buffer, and the tiled window it came from is the
  *window beneath*.
- **Its place is a pure function,** `frame::float_region(size, place, rows)`, from the frame's size, a
  place (`TopCenter` now, `Center` for M3's menus) and the rows it wants, to a `CellRect`. `FrameLayout`
  keeps its `Copy` and holds no floats, as ADR 0057 wanted. The region is worked out again each frame,
  for a few comparisons. `Editor::cursor_area`, `text_view` and `set_frame_size`'s check on a taller view
  look the selected window's region up instead of assuming the tiled one.
- **It is drawn as panes in two layers.** `Layer::FLOAT` (16) holds its chrome, a pane over its whole
  region with a background and a border. `Layer::FLOAT_CONTENT` (17) holds what is inside it: the prompt,
  the input, and from M2-17 the candidates and the preview, which never overlap one another. Menus take
  32 and 33 in M3, above the minibuffer. The gaps leave room without renumbering.
- **The border is a line of whole physical pixels** (2, as vertico-posframe's), drawn inside the outer
  ring of the region's cells. The rest of that ring is padding. `Pane` gains `border: Option<Face>`,
  four rectangles cut like the others. This is ADR 0061's "pane-level list of rectangles" in the one form
  that has a caller.
- **The look: a border and one cell of padding on every side (to ask).** A prompt is 3 rows tall: the
  ring, the input row, the ring. About 8 px of padding shows left and right of the text, and most of a
  row above and below it.
- **Content panes keep their bands** above and below, into the float's padding, through the same
  `Layout` option that keeps the modeline's band (ADR 0063), extended to the band below. The padding is the
  float's own, so nothing else is drawn there, and accents on the input's capitals are not cut.
- **Where: one row below the top of the frame (to ask).** Row 0 of the window beneath stays in view above
  the float. A float never sits on the frame's top row, so the margin question of ADR 0061 does not
  arise, and the renderer needs no change for it.
- **Width: 60% of the frame's columns, at least 40, and never closer than 2 columns to an edge (to ask).**
  It does not change while typing, nor with the length of the prompt. M2-19's preview commands ask for a
  wider one.
- **Height: what the float holds.** In M2-16 that is the input row, and up to two more rows for a prompt
  too long for one. From M2-17 the candidate rows come below the input, so the input row stays where it
  is as their number changes, which is why the minibuffer grows down from the top.
- **A small frame takes what fits:** the padding rows go first, then the gap row above, then the border,
  and a frame of one row has the float over that row. A float is never taller than the text area, so the
  modeline and the echo row stay in view.
- **Faces:** `Face::Float`, `Face::FloatBorder` and `Face::Prompt`, with constants until the theme
  (M2-23), which gives the float a background apart from the text's and checks its contrast.

### The minibuffer

- **A prompt is a float showing an internal buffer that takes typing.** ADR 0063's `is_internal` is split:
  *internal* now means only that the buffer is not the person's work, so `Editor::buffers` still leaves it
  out, and every reader of the user's work (`oso dev`'s check, the exit guard, the emergency save, the
  handoff) needs nothing new. *Read-only* is its own flag again, which `*Messages*` keeps.
- **Each prompt gets a new buffer, dropped when it closes.** No text, undo or cursor carries from one prompt
  to the next, and a passphrase does not stay in a buffer kept for reuse. The buffer has undo, bounded
  as any buffer's, and is never modified for the exit guard.
- **The prompt text is not in the buffer.** It is a `Lines` pane left of the input, in `Face::Prompt`. So
  the buffer's text is the answer, `C-a` goes to its start, and history holds answers only. A prompt
  longer than the width less 20 columns breaks onto a second and third row at whole clusters, and is cut
  with `…` after that. The input goes on its last row.
- **The input is one row and scrolls sideways** as any window does (`window::follow`) (to ask). Soft wrap
  (M2-25) may change this.
- **Commands work in it unchanged** (ADR 0008), with three rules:
  - A command that changes which buffer a window shows (`C-h e`, `C-h r`, later `C-x b`) refuses in a float:
    "Not in the minibuffer". It acts on the window beneath only through commands meant for that, which come
    with M2-17 and M2-19.
  - The float keeps one cursor. M2-21's keys that add cursors refuse there.
  - A command that needs a file, such as `C-x C-s`, fails there as on any buffer with none.
- **Keys:** a keymap in the major-mode layer of the minibuffer's buffer, as the emergency copies have one
  (ADR 0064), over the global keymap:

  | Key | Command | |
  |---|---|---|
  | `RET` | `exit-minibuffer` | Gives the input to what asked. |
  | `C-g` | `keyboard-quit` | The chain below, which closes the prompt. |
  | `M-p` | `previous-history-element` | The prompt's history, older. |
  | `M-n` | `next-history-element` | Newer, back to what was being typed. |

  `TAB` is unbound until M2-17 gives it completion. `C-n` and `C-p` move as in any one-line buffer until
  M2-17 gives them candidates. `ESC` is not a quit key: the author's `Esc` sends 英数.
- **Modal:** while a prompt is open, keys go to it alone. There is one tiled window until M3, so there is
  nowhere to switch to, and M3 decides whether `C-x o` may leave a prompt open, as Emacs allows.
- **The wheel scrolls the window beneath and leaves its cursors (to ask),** as during a composition
  (ADR 0069), so a person can read the text while answering without moving where the command acts.

### History

- **Each prompt names a history, or none.** A history is a list in `Editor`, in memory only, never written
  to disk or carried through a restart (ADR 0013, ADR 0051). `M-p` and `M-n` replace the input with an
  entry, and `M-n` past the newest gives back what was typed.
- **At most 100 entries, as Emacs's `history-length`, and an entry again moves to the front (to ask).** An
  answer over 4096 bytes is not recorded, so an entry costs at most what a message does (ADR 0063).
- **Nothing a passphrase prompt reads enters any history.**
- **A prompt opened while a buffer that may hold secrets is shown records nothing either (to ask).** Until
  M2-20's search strings and M3's query-replace there is no prompt whose answer quotes the text, but the
  rule fails closed now, so those steps need not remember it. It costs `M-p` in such a buffer.

### Reading a command's arguments

- **A command declares what it reads, and the dispatcher reads it before running it,** as Emacs's
  `interactive` does. `ArgSpec` gains `Number { prompt, history }` and `Text { prompt, history }` now,
  and M2-17 to M2-19 add `Command`, `File` and `Buffer`. The command stays a plain function: it never
  waits.
- **The dispatcher keeps the pending command:** its id, its prefix argument and the answers so far. On `RET`
  it checks the answer against the spec. A number that does not parse keeps the prompt open and says why,
  in the float. Once all are read it runs the command, through the same `execute` and command boundary as
  a key, so undo, the messages around a secret buffer and the exit guard see one command.
- **`Args` stays `Copy`.** A command reads its answers through `ctx`, as `self-insert-command` reads its
  text through `ctx.input()` (ADR 0051), so no command's signature changes.
- **A prefix argument can stand for the answer,** where the spec says so: `C-u 42 M-g g` goes to line 42
  with no prompt, as in Emacs.
- **`run-command` gives the answers in its request,** checked against the same spec, and refuses a command
  whose arguments it lacks, naming them. It never opens a prompt that waits for a person. A flow tests the
  prompt with `send-keys`.
- `M-x` waits for M2-17. It reads a command's name, then that command's arguments the same way.

### Questions

- **A command that has to ask ends, and leaves a question** through `ctx.ask(Question)`: the text, and the
  command to run on `y`, with an argument saying what was true when it asked. `n` runs a second command if
  the question names one, and otherwise nothing. No closure is kept, so commands keep no state, and the
  answer is a command like any other.
- **The command run on `y` checks again.** If more is at risk than the question said, such as a buffer that
  became modified while it waited, it asks again instead of going on.
- **One key answers (to ask):** `y` or `n`. `C-g` cancels, which for a yes/no question is the same as `n`
  but says "Quit". Unlike Emacs's `y-or-n-p`, `SPC`, `DEL` and `RET` do not answer: they are the keys
  most often typed in prose, and a stray one should not decide. Any other key says `Answer y or n` in the
  float and leaves the question open. A held key never answers: its repeats are dropped, as for the
  emergency copies' letters (`app/input.rs`).
- **A question shows in the same float as a prompt, at the top center (to ask),** with `(y or n)` at the
  end of its text, in `Face::Prompt`. Its keymap is modal, like the copies list's letters: no key reaches
  the global keymap.
- **A question opens only from a command,** run by a key, `M-x` or the socket, never from a background
  result such as a finished save. Otherwise a `y` typed in prose could answer a question that appeared
  under the person's fingers. A background result says what it found and which key asks: a save that finds
  the file changed on disk says so, and the next `C-x C-s` asks.
- **The first questions (to ask):**
  - The exit guard: `C-x C-c` and the close button, with a buffer modified or a save running, ask
    `2 modified buffers and 1 save running; exit without saving? (y or n)`, and `y` exits as a repeat does
    today. This replaces ADR 0059's repeat. The close button pressed again while the question is open does
    nothing more. A Dock quit stays ADR 0062's and ADR 0070's.
  - Saving over a file changed on disk: `C-x C-s` on a buffer known to conflict (its modeline says
    `[changed on disk]`) asks `notes.md changed on disk since it was read; save over it? (y or n)`. This is
    the way out ADR 0059 left to this step.
  - `goto-line` on `M-g g` and `M-g M-g`, reading a number, as the first command whose arguments are read.

### Recursive prompts

- **A prompt can open from a prompt (to ask).** `C-x C-c` in a prompt asks its question, M2-17's embark-like
  actions open prompts from candidates, and a question can come from a command run in a prompt. Emacs
  refuses this by default (`enable-recursive-minibuffers`), which would leave the exit guard with no way to
  ask.
- **Only the innermost shows, at the same place, with its depth first: `[2]` (to ask).** The others stay
  open beneath, hidden, and come back as it closes. Up to 8 deep; one more refuses, saying so.
- **The innermost takes the keys, and `C-g` closes it alone.**

### The `C-g` chain

**Each `C-g` does one thing, the first that applies, and says "Quit":**

1. a composition, once oso has its own view (ADR 0070): discards it. With winit no key arrives during one;
2. a pending prefix key or prefix argument: cancels it, as `KeyResolver` does today;
3. an active region in the selected window, on every cursor (M2-13);
4. the innermost prompt or question: closes it, and runs nothing;
5. isearch (M2-20), its own two steps;
6. extra cursors (M2-21);
7. otherwise nothing more.

Steps 3 to 7 are one function in `oso-core`, `Editor::quit`, which `keyboard-quit` calls in every keymap.
The minibuffer's `C-g` is that same command, so M2-13, M2-20 and M2-21 add their step in its place and
nowhere else. A region in a prompt goes before the prompt, as with Emacs's `delete-selection-mode`.

### The echo area and messages

- **The echo row stays at the bottom, for messages and for keys in progress** (ADR 0063). No prompt is
  drawn there.
- **A message from a command run by a key typed in a prompt shows in the float (to ask),** at the end of
  the input row, in brackets: `Go to line: 9999 [Past the end]`. It goes at the next key, as the echo area's
  does, and reaches `*Messages*` and the log by the same rules. A message from elsewhere, such as a
  finished save, goes to the echo row as always.
- Closing a prompt with `C-g` echoes "Quit", which reaches neither the log nor `*Messages*`.

### What the window beneath shows

- **Its text, its modeline and its updates, as before.** A load's progress and a finished save still show.
- **Its cursor is drawn hollow (to ask),** as Emacs draws a window that is not selected, so the person sees
  where the command will act. `TextView`'s cursor gains a style.
- **A float may cover its cursor, and the view does not move (to ask).** A prompt is 3 rows near the top,
  and a view that jumped on every prompt would be worse. M3's query-replace, whose question is about the
  text at the cursor, may revisit this.

### The input method

- **`wants_ime` gains one condition: a float is selected,** as `ui.md` and ADR 0026 decided. Turning off
  waits for a composition's end (ADR 0069), or commits it under ADR 0070. A key that opens a prompt cannot
  come during a composition, so only the socket or another thread could open one then.
- **When the prompt closes, the same function decides again** for the window beneath. Nothing else is kept.
- **No prompt in M2-16 reads text a person types in Japanese:** `goto-line` reads digits and questions read
  one letter. M2-18's file names may need Japanese. Whether a key such as かな turns the IME on for one
  prompt is decided then, after trying at the keyboard whether that key reaches oso while the IME is off.
- `Editor::cursor_area` gives the float's cursor when it is selected, so a candidate window, once one opens
  in a prompt, goes below the input.

### Passphrases and secrets

- **A passphrase prompt is a buffer that may hold secrets** (`Verdict::Secret`). So every rule for such a
  buffer applies with nothing new: `get-state` leaves out its text and positions, screenshots are refused
  while it shows, keys typed in it leave no timings (ADR 0060), messages around it stay out of the log and
  `*Messages*`, and the IME is off.
- **On top of that:** it records no history, has no undo, so no copy of the text stays in an undo record,
  and is dropped as it closes. Its kills reach neither the kill ring nor the OS clipboard (M2-14 builds
  that), while a yank into it works, as pasting from a password manager needs. Keyboard macros (M3) do not
  record what is typed in it.
- **It shows one `*` per character (to ask),** as Emacs's `read-passwd` does.
- **What memory keeps:** the rope's chunks are freed without being overwritten. `oso-core` has no `unsafe`,
  and a write of zeros before a free can be optimized away without a volatile write. Core dumps are already
  off ([ADR 0028](0028-secrets-handling.md)). Overwriting needs `zeroize` or a small `unsafe` in another
  crate, which the first caller decides.
- **No M2 command reads a passphrase.** M4's git is the first. M2-16 builds the kind and its rules, with
  tests in `oso-core` and a dev-only command for the flows (to ask).
- **Any prompt, while a buffer that may hold secrets is shown,** leaves its text, its prompt and its
  cursor out of `get-state`, as the echo area and the modeline are left out then (ADR 0063). A search
  string or a line number can say something of the text.

### The control socket and end-to-end flows

This is a human gate ([`REVIEW.md`](../../../REVIEW.md)).

- **`get-state` gains `prompts`:** for each open one from the outside in, `depth`, `kind` (`read`,
  `question` or `passphrase`), `prompt`, `text`, `cursor` and `region` in cells. `prompt`, `text` and
  `cursor` are null under the rules above.
- **`run-command` takes the answers** as `{"name": "goto-line", "args": {"line": 42}}`. It keeps taking a
  count for `ArgSpec::Count`.
- **`send-keys` drives prompts** as a person's keys do.
- **A dev restart waits for every prompt to close (to ask),** as it waits for a composition (ADR 0069):
  `check-handover` and a generation's `exit` refuse while one is open, and say why. A prompt is no unsaved
  work, but closing it under the person loses what they were doing.
- **Flows (`just e2e`):** `M-g g 4 2 RET` moves to line 42 and closes the prompt; `C-g` closes it and leaves
  the cursor; `M-p` brings back the last answer; the exit guard's question, `n` stays and `y` exits; a
  question refuses `SPC` and a held `y` answers once; the IME off in a prompt and back on after it; a
  question inside a prompt and `C-g` twice; a passphrase prompt withheld from `get-state` and a screenshot;
  a restart refused while a prompt is open. Drawing goes in text-grid snapshots
  ([ADR 0045](0045-text-grid-snapshots.md)), which draw a float's border with box characters, and a pixel
  test checks that a float hides the text under it.

### Costs

Against the 8 ms typing budget ([ADR 0056](0056-typing.md)):

- **Opening a prompt** adds two or three panes. Their ids are new, so each gets a `Rows` and shapes its one
  row: about what one typed key costs, a few hundred microseconds. The window beneath keeps its id, so its
  rows are reused as they are, and closing the prompt reshapes nothing.
- **A key typed in a prompt** reshapes the input row and reuses every row of the window beneath, whose
  revision has not changed. That is less than typing in a file.
- **The rows under a float are still drawn,** at most the float's area again in pixels. ADR 0061 accepted
  this overdraw.
- **Every batch compares the `Status`,** which gains the prompts' state: a few fields.
- **`M-p`** copies at most 4096 bytes. **`get-state`** is unchanged for a client that asks nothing new.
- `just perf` gains `prompt_type_p99_ms`, keys typed into an open prompt over the screen of mixed text,
  reported only, with no threshold. This edits the benchmark files that define the checks, which needs the
  author's approval.

### What this leaves room for

- **M2-17:** candidates as a `Lines` pane below the input, in `Layer::FLOAT_CONTENT`, the float's height
  growing to 1 + 18 rows. Annotations as spans at the right. The selected candidate's background needs a
  rectangle per row in `Lines`, which M2-17 adds. `M-x` through `ArgSpec::Command`. `C-n` and `C-p` on
  candidates. Candidates from a worker thread (consult's ripgrep) update the float through the `Status`
  comparison, as a load's progress does.
- **M2-18 and M2-19:** `ArgSpec::File` and `ArgSpec::Buffer`. The preview as a read-only `TextView` pane
  right of the candidates, with ADR 0061's optional cursor, in a wider float.
- **Embark-like actions:** recursive prompts.
- **M3:** menus at `Place::Center` in layers 32 and 33, migemo in the minibuffer with the IME off, and
  splits, where the window beneath is the one selected before the prompt.

### Where it lives

- **`oso-core`:** a new public module, `minibuffer`, with the prompt stack, `Prompt`, `Question` and the
  histories, on `frame`'s line of `guardrails/layers.txt`: below `editor`, above `window` and `buffer`.
  `frame::float_region`, `Editor::quit`, `ArgSpec`'s new variants and `Ctx`'s answers. The split of
  `is_internal`.
- **`oso-render`:** `Pane::border`, three `Face` variants, the band below in `Layout`, a cursor style on
  `TextView`, and `Layer::FLOAT` and `Layer::FLOAT_CONTENT`.
- **`oso`:** the minibuffer's keymaps and commands in `commands/minibuffer.rs` and
  `bindings/minibuffer.rs`; `goto-line`; the dispatcher's pending command; the exit guard's and the save's
  questions; the panes in `app/draw.rs`; the condition in `app/ime.rs`; the socket's fields in `dev`.
- No new crate, crate edge, dependency or `unsafe`. One new public module and no new public trait.

## Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0034](0034-command-registry-and-keymaps.md): `ArgSpec` describes arguments to read, and the dispatcher
  reads them before the command runs. `run-command` takes answers. `C-g` in a prompt is `keyboard-quit`,
  whose chain is in `Editor::quit`.
- [ADR 0057](0057-frame-layout.md): floats are placed by `frame::float_region`, outside `FrameLayout`.
- [ADR 0059](0059-safe-save-and-the-exit-guard.md): the exit guard asks instead of waiting for a repeat, and a
  conflicting save can go on once asked.
- [ADR 0061](0061-panes-clipping-and-layers.md): `Pane` gains a border, and panes inside a float keep the
  bands above and below them.
- [ADR 0063](0063-modeline-echo-area-and-internal-buffers.md): `is_internal` no longer means read-only and no
  undo, and a message from a key typed in a prompt shows in the float.
- `ui.md`: where the prompt sits, how it looks, and questions.

## Consequences

- Commands ask through the dispatcher or a question and never wait, so a command can be tested as before,
  with its answers given.
- A key that opens a prompt draws a frame with two or three new panes, and closing it draws the window
  beneath unchanged.
- Row 0 of the text shows above a prompt, and rows 1 to 3 are hidden under it.
- `y` answers a question at once. `SPC` and `RET` do not, unlike in Emacs.
- `C-x C-c` with unsaved work asks instead of waiting for a repeat. A person used to repeating it will see
  a question.
- A buffer that changed on disk can be saved over once asked.
- `M-p` gives nothing in a prompt opened from a buffer that may hold secrets.
- Japanese cannot be typed in a prompt until M2-18 decides otherwise.
- A dev restart can wait on a prompt left open.
- `oso-core`'s public API gains the `minibuffer` module, `frame::float_region`, `Editor::quit` and
  `ArgSpec`'s variants; `oso-render`'s gains `Pane::border`, `Layer`'s two constants, three `Face`
  variants and the cursor style.

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
