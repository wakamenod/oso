# 0069. The macOS input method

- Status: Proposed
- Date: 2026-11-07

## Context

M2-12 is the last step before the author edits real notes in oso
([roadmap](../roadmap.md)), and most of them are Japanese. Earlier ADRs fixed the outline:
the preedit is drawn inline at the cursor, underlined, pushes the text after it in the
cursor's row only and is never written to the buffer
([ADR 0026](0026-m2-ui-layout.md), [ADR 0051](0051-m2-plan-decisions.md)); commits go
through `Dispatcher::insert_text` ([ADR 0056](0056-typing.md)); `TextView` gains the
preedit and `Rows::read` splices it into the cursor's row
([ADR 0061](0061-panes-clipping-and-layers.md)); `Editor::cursor_area` gives the cursor's
cell in physical pixels ([ADR 0057](0057-frame-layout.md)); and the IME goes off in the
minibuffer, isearch, menus and the list of emergency copies
([ADR 0064](0064-recovering-an-emergency-copy.md), [`ui.md`](../ui.md)).

A spike with winit 0.30.13 and the Apple Japanese input method, at the keyboard, showed
what winit really does ([`open-questions.md`](../open-questions.md), "By M2-12"):

- A composition is `Enabled` (only at the first marked text after a start or a
  `Disabled`), one `Preedit(text, range)` per key, then `Preedit("", None)` and `Commit`.
  The range is in UTF-8 bytes: the caret while typing, the segment being converted
  while converting.
- During a composition no key reaches `KeyboardInput`, `C-g` included. So no binding can
  fire during one, and only mouse clicks, timers and other threads can change what oso
  shows.
- `set_ime_allowed(false)` is enough to turn the IME off, and plain keys then arrive as
  keys. The OS input source stays selected. Turned off during a composition, the OS keeps
  the composition, and the next key after turning on brings it back. winit cannot commit
  or discard one.
- `Disabled` also comes unasked, on the first key after the input source changed. A
  composition can end with `Preedit("", None)` and no `Commit`.
- With the IME allowed and a Japanese source, Option-a starts a composition, and `s-a`
  never arrives, against "Option stays Super" ([ADR 0051](0051-m2-plan-decisions.md)).
- A plain letter starts a composition, so `C-x b` in hiragana would swallow `b`.
- Space arrives as `SPC`, and winit drops the IME's own text when nothing is marked, so a
  full-width space cannot be typed.
- `set_ime_cursor_area` places the candidate window just below the rectangle. It takes
  physical units as `PhysicalPosition`. A call during a composition does not move an open
  candidate window. It costs 20 to 60 µs, about 200 µs during a composition, and about
  3.2 ms before the first frame. `set_ime_allowed` costs under 1 µs, and allowing it before
  the first frame did not change startup.
- Leaving the app during a composition sends only `Focused(false)`. The composition
  survives and continues on return.

## Decision

The author chose each UI choice below on 2026-11-07, from options put to them: how the
segment looks, where the box cursor goes, which cursor shows the preedit, the IME off in
buffers that may hold secrets, in read-only buffers and while a prefix argument reads
digits, `s-SPC` for a full-width space, a restart waiting for a composition, one undo step
per commit, and the preedit metric. Which way Option stays Super waits for their keys.

### The preedit

- **The preedit is the editor's, outside every buffer:** `Editor` holds at most one,
  for the selected window, as its text and an optional byte range. A range that is not
  on character boundaries of the text, or runs past it, is dropped and the text kept.
  Nothing writes it to a buffer, the undo history, an emergency copy, a state handoff or
  a log, and a log says only that a commit happened, nor does the socket's stand-in
  give its text to `Debug` ([ADR 0028](0028-secrets-handling.md)). Short pieces of it may
  stay in the renderer's cache of shaped pieces until the next edit, as any text drawn
  does ([ADR 0068](0068-cut-clusters-short-pieces-the-cursor-box-and-lines-by-direction.md)).
- **It is shown at the primary cursor only.** A commit inserts at every cursor, as
  typing does ([ADR 0007](0007-multi-cursor.md)).
- **Its cells follow the buffer's rules.** It is segmented with `text_clusters`, so
  full-width kana, emoji, half-width kana with marks and control characters take the
  cells and escapes they would in the buffer.
- **It is drawn inline and pushes the rest of the row,** as ADR 0051 says. `TextView`
  gains the preedit, which goes at its cursor. `display_map::row_clusters` gives the
  cursor's row up to the cursor, then the preedit, then the rest with its columns moved by
  the preedit's width, and both `Rows::read` and the text-grid snapshots read the row
  through it. What is pushed past the right edge is cut off, and a tab after the preedit
  keeps the width of its buffer column. The preedit is in a face of its own, `Preedit`,
  so it is shaped apart from the text around it. The cache's shortcut on an unchanged
  revision does not hold for that row while a preedit is shown, nor in the frame after,
  so the row is compared by content as after an edit, and every other row is reused as
  before.
- **It is underlined, and the segment being converted more thickly,** as native macOS
  text views draw it: a rectangle of one line width under each preedit cell, and of two
  under the segment, in the row's own pixels ([ADR 0061](0061-panes-clipping-and-layers.md)).
- **The box cursor goes where the IME's caret is.** While typing at the end of the
  preedit, which is the usual case, that is the cell after it, where the text resumes.
  When the IME moves its caret inside, the box covers the preedit cell there. While
  converting there is no caret, and the box goes after the preedit.
- **The view follows the preedit.** After each preedit event, `window::follow` gets the
  span from the cursor to the box: it scrolls sideways by the fewest columns that show
  the box, and the preedit's start too when both fit. A preedit wider than the window
  shows its end, where typing happens.
- **The first preedit of a composition counts as a key:** `Dispatcher::begin_composition`
  empties the echo area and resets the exit guard, through the same wrapper as a key. Later ones change only the
  preedit and ask for a frame.
- **Each commit is one undo step,** also a single character. A commit goes through a new
  `Dispatcher::commit_text`, the same path as `insert_text` but never asking to join the
  step before. One-character commits are common in Japanese (`、`, `。`, a lone kana),
  and joining them to the letters before would undo a phrase together with what came
  before it.

### The candidate window

- **The cursor's cell goes to the OS after every event,** before the next is read:
  `Editor::cursor_area` as a `PhysicalPosition` and `PhysicalSize`, only when the
  rectangle or the scale factor differs from the last one given. A composition the next
  key starts then opens its candidate window at the cell a command or a commit just
  moved to, also before that frame is drawn, as a call during a composition moves
  nothing. One rule covers motion, scrolling, resizing, a new scale and a finished load.
- **Not before the first frame is presented,** where the first call costs about 3 ms.
  `set_ime_allowed` itself is called as soon as the window exists.
- **Not during a composition,** where a call costs about 200 µs and moves nothing. The
  candidate window stays where the composition started.
- **Not when the cursor is out of view** (`cursor_area` is `None`).
- **Not in a buffer that may hold secrets,** which has the IME off (below).

### When the IME is on

- **One function decides whether the IME is wanted,** from the editor and the dispatcher,
  in one hook after every window event and user event, before the next event is read. It calls `set_ime_allowed` only when the answer changes. The IME is wanted
  unless:
  - the selected buffer shows emergency copies (`Mode::shows_copies`), where letters are
    answers (ADR 0064);
  - the selected buffer is read-only, such as while it loads, a binary file or
    `*Messages*`, so a letter fails at once instead of after a whole composition;
  - the selected buffer may hold secrets, as macOS password fields do;
  - a prefix key is pending (`C-x`), so the next letter reaches the keymap;
  - a prefix argument reads digits or a minus (`C-u`, `M-5`), since digits compose in
    hiragana.

  The minibuffer (M2-16), isearch (M2-20) and menus (M3) add their own condition to this
  function. `Dispatcher::reads_keys` says whether a prefix key or argument waits.
- **Turning off waits for the composition to end.** While a preedit is shown, a change to
  "not wanted" is kept, and applied at the composition's end: an empty preedit, a commit,
  or `Disabled`. Keys cannot cause such a change, as they are swallowed; only a socket
  request or another thread can. Turning on happens at once, and gives the OS the
  cursor's cell if it moved while the IME was off.
- **Any event that leaves a buffer that may hold secrets selected ends a composition,**
  in the same hook, so the IME goes off there at once. Only a socket request or another
  thread can change the buffer during one, and a composition the socket made has no OS
  behind it to end it.
- **The wheel during a composition moves the view only.** The cursors stay, so the
  commit goes where the composition started, and the command it runs brings the view
  back. Moving the cursors to the view, as the wheel does otherwise, would move the
  preedit to another line.
- **A commit goes where typing goes at that moment,** as a key would. If a request
  changed the selected buffer during the composition, the commit goes there, and fails
  as a key would where text is refused.
- **oso keeps its own state** and never reads it back from `Ime` events: whether it
  wants the IME, whether it last allowed it, and whether a composition is open, which is
  whether the editor holds a preedit.
  - `Enabled` changes nothing.
  - `Disabled` ends any composition and drops the preedit, whether oso asked for it or
    not. It does not change what oso wants, and the next composition sends `Enabled`
    again.
  - `Preedit("", None)` with no commit ends the composition and drops the preedit.
- **Focus changes nothing.** On `Focused(false)` during a composition the preedit stays
  drawn, and the composition continues on return, as the OS keeps it.
- **A dev restart waits for a composition to end,** as it waits for a save:
  `check-handover`, and `exit` with a generation, refuse while a preedit is shown, and
  the next build tries again
  ([ADR 0055](0055-oso-dev-never-loses-unsaved-text.md)). Closing the window during a
  composition loses the preedit, which is not buffer text.

### Option as Super

ADR 0051's "Option stays Super" holds. To keep it with a Japanese source:

1. **First, `set_option_as_alt(OptionAsAlt::Both)`** on the window, from winit's
   `WindowExtMacOS`. winit then gives the input method the key without Option's
   character.
2. **If Option-a still composes,** the IME also goes off while Option is held, from
   `ModifiersChanged`, and back when it is released, through the same function and with
   the same wait for a composition to end.

Which one holds is recorded here once the author has tried the keys (open). Neither needs
a new dependency or `unsafe`.

### A full-width space

- **`SPC` stays half-width.** `insert-full-width-space` inserts U+3000 at every cursor,
  with a count, as `self-insert-command` would for that character. Its default key is
  `s-SPC` (Option-Space), if Option arrives as Super with a Japanese source (above). It is
  a default, which M3's user key layer can change
  ([ADR 0051](0051-m2-plan-decisions.md)).

### The control socket and end-to-end flows

- **A dev method, `ime`, stands in for the OS,** since no flow can drive a real input
  method. It takes `{"preedit": TEXT, "range": [START, END]}`, `{"commit": TEXT}`,
  `{"clear": true}` (an empty preedit with no commit) or `{"disabled": true}`. The tip
  crate turns winit's `Ime` into its own event type, and the method feeds that type to
  the same handler, so it needs no winit and stays behind the `dev` feature. A commit
  sends the empty preedit first, as the OS does. While oso has the IME off, the method
  refuses, as the OS would send nothing.
- **`get-state` gains `ime`:** `{"wanted", "allowed", "composing", "preedit", "range",
  "area"}`, where `area` is the last rectangle given to the OS. While a buffer that may
  hold secrets is shown, `preedit`, `range` and `area` are null
  ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md),
  [ADR 0051](0051-m2-plan-decisions.md)). `text` stays the buffer's text, without the
  preedit; `screenshot` shows it.
- **Flows (`just e2e`)** check the seams: a preedit drawn and the text pushed, against a
  screenshot; a commit inserting and one `C-z` taking exactly it out; the IME off in
  `*Emergency Copies*`, after `C-x` and after `C-u`, and on again after them; turning off
  deferred until the composition ends; and a restart refused while composing. Drawing is
  checked in text-grid snapshots ([ADR 0045](0045-text-grid-snapshots.md)), which gain
  marks for the underline and the segment. What only the OS does is a manual check.

### Costs

Against the 8 ms typing budget ([ADR 0056](0056-typing.md)):

- **A key** gains the wanted check, a few comparisons, and `set_ime_allowed` only when the
  answer changes, under 1 µs.
- **A preedit event** copies its text into the editor, follows, and draws a frame in which
  one row is shaped again, as one typed key would cost. Other rows are reused.
- **An event that moved the cursor on the screen** gains one `set_ime_cursor_area`, 20
  to 60 µs, before the frame it asks for. That includes a key that moves the cursor, and
  each wheel or trackpad event of a scroll.
- **A commit** costs as typing the same text does, plus one undo step.
- **The first key after launch** stalls the UI thread for about 21 to 32 ms with the IME
  allowed, against 6 to 15 ms without, once per process, as the OS's input method starts.
  It is under the 100 ms freeze threshold and is accepted.
- `just perf` drives keys through the socket, which skips the OS input method, so its
  typing metrics do not see it. **It gains `ime_preedit_p99_ms`:** preedit updates through
  the `ime` method on the screen of mixed text, reported only, with no threshold, since a
  row with a preedit skips the cache's shortcut on an unchanged revision. This edits the
  benchmark files that define the checks, which the author approved.

### Where it lives

- **`oso-core`:** `display_map::Preedit`, whose cells, caret and segment are worked out
  once through `text_clusters`, and `display_map::row_clusters`, in a private module
  `display_map/preedit.rs`; `motion::cursor_box`; `Editor::preedit` and
  `Editor::set_preedit`, which follow it with `window::follow` as it is; and
  `Editor::keep_undo_step_apart`. No new public module or trait.
- **`oso-render`:** `TextView<'a>`'s preedit field, `Face::Preedit`, the splice in
  `rows`, and the underline rectangles and the cursor box in `scene`, which holds two
  rectangles back for the underlines as it does one for the box.
- **`oso`:** `app/ime.rs`, a concern beside `input`, for the events, the wanted function,
  the deferral and the cursor area; `Dispatcher::commit_text`; the command and its key in
  `commands/edit.rs` and `bindings/edit.rs`; the socket method in `dev`; the preedit
  metric in `examples/perf`.
- No new crate, crate edge, dependency or `unsafe`, and no change to
  `guardrails/layers.txt`. `oso-platform` gains no IME trait: winit's calls are the
  abstraction, and only the tip crate may use winit
  ([ADR 0038](0038-architecture-guardrails.md)).

## Consequences

- Japanese can be typed, converted and undone phrase by phrase. A sentence of several
  commits takes several `C-z`.
- The candidate window opens below where the composition started. When a long preedit
  converts its later clauses, it stays there, not under the segment, as winit gives the
  OS one rectangle.
- In a buffer that may hold secrets, the IME is off and letters arrive as keys, so
  Japanese cannot be typed there.
- After `C-x` or `C-u` the IME is off until the next key, so `C-u` then hiragana needs the
  key after `C-u` to be a command or a digit.
- Read-only buffers and the copies list never show a preedit.
- `s-SPC` is taken.
- The socket gains `ime` and `get-state` a field, which `just release-safety` and the e2e
  flows cover.
- A dev restart can wait on a composition left open.
- Soft wrap (M2-25) must decide what a preedit that pushes past the right edge does on a
  wrapped row.
- The `open-questions.md` item on `dispatch.rs` wrapping `handle_key`, `insert_text` and
  `run_command` the same way is done: one wrapper, `as_input`, serves them,
  `commit_text` and `begin_composition`. The exit guard stays outside it, as a prefix key
  must not reset it.
- The socket's `ime` takes a preedit of at most 64 KiB, so that segmenting one stays
  short. A long preedit keeps an index of every 256th cluster, so a frame that shows its
  end reads only from near there.

## Alternatives considered

- **Writing the preedit into the buffer,** as Emacs's NS port does with an overlay: undo,
  the modified flag, saves and emergency copies would all have to skip it. ADR 0051
  rejected it.
- **Drawing the preedit over the text after it:** rejected in ADR 0051.
- **A preedit at every cursor in view:** shows what each will get, but splices several
  rows, and ADR 0051 says the cursor's row only.
- **A commit at the primary cursor only:** the one place where typing would not reach
  every cursor.
- **Joining a one-character commit to the typing before it,** as ADR 0056 lets a
  one-character input join: a phrase of one kana would undo with the word before it.
- **Calling `set_ime_cursor_area` from the key path alone:** misses scrolling, resizing
  and a finished load.
- **Calling it from the draw path alone, after a presented frame:** costs keys nothing,
  but a composition started before the next frame opens its candidate window at the old
  cell, for the whole composition.
- **Ending a composition on a wheel scroll:** the OS keeps it, and the next key would
  bring it back.
- **Giving the OS the segment's cell during a composition:** the spike saw such a call
  move nothing, at 200 µs each.
- **Turning off at once, even during a composition:** the OS keeps the composition, and
  it comes back at the next key.
- **Committing or discarding the composition before turning off:** winit offers no way,
  and reaching `NSTextInputContext` past winit would need `unsafe` in its view.
- **Switching the input source to ASCII through HIToolbox:** needs FFI and `unsafe`, and
  `set_ime_allowed(false)` already gives plain keys.
- **Reading `Disabled` as oso's own state:** it also comes when the source changes, and
  oso would then never turn the IME on again.
- **The IME on in a buffer that may hold secrets,** with the candidate window following
  the cursor or at a fixed place: Japanese would work there. It is off instead, as macOS
  password fields do.
- **`S-SPC` for a full-width space:** what Japanese IMEs often use, but ADR 0033 keeps
  Shift out of characters, so the key cannot be bound without changing it.
- **`C-x 8 SPC`:** Emacs's prefix for special characters, but Emacs gives that key a
  no-break space.
- **An `oso-platform` IME trait:** nothing else would implement it yet, and the tip crate
  is the only winit user.
