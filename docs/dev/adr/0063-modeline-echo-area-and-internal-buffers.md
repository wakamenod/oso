# 0063. The modeline, the echo area and internal buffers

- Status: Proposed
- Date: 2026-11-06

## Context

[ADR 0061](0061-panes-clipping-and-layers.md) left two empty panes for the modeline and
the echo area, and [`ui.md`](../ui.md) says what they show. Several ADRs left choices to
this step:

- [ADR 0057](0057-frame-layout.md): where warnings go in a frame too small for a modeline,
  and a load that grows rows below the view no longer asks for a frame, so its progress
  must ask for its own.
- [ADR 0061](0061-panes-clipping-and-layers.md): what one-row panes below the text do with
  the band above their first row.
- [ADR 0056](0056-typing.md): a failed key leaves a message and asks for no frame, and an
  undefined key leaves nothing, since messages reach the log and typed keys must not.
- [ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md): nothing shows that
  a prefix argument is being typed.
- [ADR 0051](0051-m2-plan-decisions.md) and [ADR 0055](0055-oso-dev-never-loses-unsaved-text.md):
  `*Messages*` must not hold a restart.
- `open-questions.md`: a message has no cap on its length, and the bidi warning needs a
  count the TabMap's summary does not keep.

## Decision

### What the two panes say

- **`Editor::status_into` fills in a `Status`,** in `editor/status.rs`: plain strings for
  the warnings, the marker with the name, the rest of the modeline, and the echo area. It
  is pure, so it is tested with no window, and text-grid snapshots draw the same strings.
  `Face` stays in `oso-render`, as ADR 0061 expected: `oso-core` does not need to name one.
- **The tip crate gives them faces.** `panes` becomes `with_panes`, which borrows the
  strings as spans and calls back with the list. `Face` gains `Modeline` and `Warning`. Only
  the modeline's pane has a `background`, as ADR 0057 wants nothing to mark the leftover
  pixels below the echo area.
- **The renderer's drawing changes in one place,** the band below. ADR 0061 meant it not to change at all.
- **Building a `Status` reuses its strings,** so once they have grown it allocates only the tree
  cursor a position needs. It reads the buffer through that position and a summary: O(log n) plus a
  scan of one chunk, what drawing the cursor already costs.

### The modeline

From the left: warnings, the marker and name, loading progress, line and column, the number
of cursors if there are several, the major mode, the encoding and the line endings.

```
[bidi controls] ** notes.md  Loading 34%  12:5 (2 cursors)  Fundamental  UTF-8 LF
```

- **Warnings come first,** so that no name can push them off the row. A span's budget of
  characters (ADR 0061) means a long name cannot take their budget either.
- **The name gets the cells the rest leaves,** and at least 20, cut after a whole cluster and
  ended by `…`. So in a usual window nothing is pushed out, and in a narrow one the name
  stays at the expense of what follows it.
- **The marker is Emacs's:** `--`, `**` once modified, `%%` read-only and `%*` read-only and
  modified.
- **The column counts from 0 and the line from 1,** as in Emacs. Both are display rows and
  columns, so M2-25 must keep the line a logical one when rows wrap.
- **The major mode reads `Fundamental`** until modes arrive (M3).
- **`[bidi controls]` warns** for the nine Trojan Source controls (U+202A to U+202E, U+2066 to
  U+2069). LRM, RLM and ALM are left out: they can only tip a neutral character, and
  right-to-left prose has them everywhere. All twelve are still boxed.
- **The count is in `TabSummary`,** so asking costs O(1) and an edit updates it along with the
  rest. Each chunk is scanned once: a search for the lead byte 0xE2, then its characters only if it
  has one. Japanese text rarely has that byte, and English with dashes or curly quotes pays about
  1 µs per chunk. The count should describe the text, so when the FoldMap or InlayMap goes below the
  TabMap it moves to the rope's summary, as hidden text would stop counting and virtual text would
  start. `Buffer::has_bidi_controls` is what callers ask, so they do not change.
- **`[changed on disk]` warns** when a save found the file changed or elsewhere, while the buffer
  is modified. A save that gets through clears it. It knows no more than a save found out until
  M2-22 brings the watcher.

### What is below three rows

**With two rows, warnings go first on the echo row.** The echo area outlasts the modeline for
the same reason errors do: there is one row to say what is wrong. **With one row, they have no
place,** and neither do messages. The frame is one row of text, and the boxes in it still mark
every control in view. Drawing over the text in a layer above could hide the cursor, for a
window nobody works in.

### A secret buffer's modeline

**It says `secret` where the position goes, and no number of cursors.** ADR 0058 withholds these
from `get-state`, and a modeline that says them would be a new way out if anything ever copies it.
The name, the marker, the encoding and loading progress stay.
The warnings stay too: they tell the person at the screen something they need, and they stay out
of `get-state` with the rest of the modeline. This is the author's choice. It costs the line number
in such a file, with the text itself on the same screen.

### Messages

- **`Editor::message` caps a message at 4096 bytes,** and its row in `*Messages*` and line in the log
  after escaping, room for what it says and the longest path most systems have. A longer one is cut on a
  character boundary and ended by `…`.
- **`Editor::echo` reaches the echo area alone,** and neither the log nor `*Messages*`. This is how
  "C-x C-z is undefined" and "Quit" get there, so typed keys stay out of the `info` log (ADR 0056). Its
  text shows the keys. Unbound keys still reach the `debug` log as ADR 0056 has it.
- **A message from a command run around a secret buffer goes the same way,** whatever the command and
  whichever way it came, and if the command took that buffer off the screen it goes nowhere. "End of
  buffer" says where the cursor is, and a log or `*Messages*` keeps it. So the rule is in `Editor::message`.
  The dispatcher marks where a command begins and ends (`Editor::begin_command`, `Editor::end_command`), and
  what the beginning records holds whatever the command does to that buffer. What a panic leaves names only
  the command, and comes after the end is marked, as it is a bug to find. This changes ADR 0056's "A failed
  command's message goes to `Editor::message`, whatever invoked it" in what it reaches.
- **What was echoed around a secret buffer goes as the window moves on,** for good, so that it does not
  come back with the next such buffer. A frame also checks this as it reads the echo area, for a verdict that
  changes under it. A message from outside a command, such as a finished save, names a file and quotes no
  text, so it stays.
- **A message says what happened before it names a path,** "Not saved: the file changed on disk…: path", so
  that a long path cannot hide it, on the echo row or at the cap. So save failures read differently. A save
  the disk did not confirm begins "Wrote the file, but", as the buffer then counts as saved.
- **A panic while adding a row to `*Messages*` is not caught.** `Editor::message` runs outside the command
  boundary of ADR 0014, also just after a panic it reports, and now edits a rope. So such a panic does what any
  panic outside a command does: ADR 0062's hook writes the emergency copies and the process ends. The log and the
  echo area are set first. The edit is made on clones of a small buffer, so this takes a bug in the rope.
- **The next key empties the echo area,** as in Emacs, and so does `run-command`. The wheel does not.
- **What is being typed comes first,** ahead of any message, as Emacs does while a key sequence is
  under way: `C-x-`, `C-u-`, `C-u 16-`, `C-u 5 C-x-`. The dispatcher owns it and tells the editor after
  each key, so that a frame only reads the editor. A number is shown as a number however it was typed.

### Asking for a frame

**After every batch of events, the app compares the `Status` with the one last drawn or asked for,**
and asks for a frame if they differ (`about_to_wait`). A failed command, a pending key, a finished save, a
message from another thread and a load's progress all go through this, so none needs a request of its
own and a new one cannot forget. It costs one build of a `Status` per batch.

- **A load asks only when its number changes,** at most about 100 times for a file of known size.
- **What was asked for counts as drawn,** so a window that draws nothing, such as a hidden one, is
  not asked again and again.
- **`*Messages*` in view counts by its revision,** since the same message twice leaves the echo area as
  it was. No load changes that buffer, so this asks only for a new row.

`Effects::redraw` keeps its meaning for what commands change in the text. An unbound key shows once:
repeating it leaves the `Status` the same, so it draws nothing more.

### `*Messages*` and internal buffers

- **`Buffer::is_internal` marks a buffer the editor keeps for itself.** It is read-only for keys
  (`ReadOnly::Internal`), has no undo history, and is never modified. M2-16 must split this, as the
  minibuffer's buffer is internal too (ADR 0051) and must take typing and undo (ADR 0008).
- **`Editor::buffers` leaves them out.** So `oso dev`'s check, generation and import, the exit guard
  and the emergency save need nothing new, and a new reader of the user's work cannot forget them. Who
  wants one asks for it by id. This fails safe, where a filter in each reader would not.
- **The editor makes `*Messages*` when it starts,** so a message never finds it missing, and
  `kill_buffer` leaves it. M2-19's `kill-buffer` decides whether a person may.
- **Each message is one row, as the log escapes it,** so a file name cannot forge a line, and a name's
  bidi control does not set off the warning there. The echo area and the modeline draw through
  `text_clusters` instead, which boxes it. It keeps the newest 1000, and is the only copy.
- **A window showing `*Messages*` keeps a cursor at the end there,** as new rows arrive, and the view
  follows it, for a message from outside a command too.
- **`view-echo-area-messages` on `C-h e`** shows it, with the last row in view. Since nothing else
  leaves it until `C-x b` (M2-19), the same key goes back to where the window was. So a person can read it,
  and a client of the socket can too.
- **A window on `*Messages*` hands over where it goes back to,** since no key could reach that again otherwise
  until `C-x b` (M2-19). Positions in a secret buffer are withheld as anywhere (ADR 0058). With nothing to go
  back to, it comes back on `*scratch*`, as one more entry with no path. The state's format does not change,
  and the text of `*Messages*` is not carried.
- **An instance whose window is on `*Messages*` refuses an import,** since it has no `*scratch*` in view
  for the state to start from. Its messages alone do not refuse one.

### The band above the modeline and the echo row

**The modeline and the echo row keep the band above them,** as accents on capitals need it (ADR 0052). That is
a tiled pane of `PaneContent::Lines` with one row. The modeline's rectangle stays in its row. This is a small
change to the renderer (`Layout::with_band_above`, chosen in `draw_pane`), against ADR 0061's expectation. A text
pane and a float keep ADR 0061's rule, and M2-16 and M3 decide for theirs.

### The control socket

This is a human gate ([`REVIEW.md`](../../../REVIEW.md)). `get-state` gains `modeline` and `echo`, as ADR 0051
foresaw, and leaves both out while any window shows a secret buffer. Screenshots already refuse then.

### Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0026](0026-m2-ui-layout.md), the echo area is "for messages only" and all reach `*Messages*`: it also
  shows keys in progress, undefined keys and "Quit", and what is said around a secret buffer stays out.
- [ADR 0051](0051-m2-plan-decisions.md), internal buffers "are not exported": a window on one with nothing to go
  back to is exported as an entry with no path, which holds nothing of it.
- [ADR 0053](0053-edit-core.md), edits go through `apply_edits` with an undo record: an internal buffer's
  go around it, with none.
- [ADR 0055](0055-oso-dev-never-loses-unsaved-text.md), "M2-7 must teach them to skip internal buffers": they
  need teaching nothing, as `Editor::buffers` skips them. `import-state` also refuses with the window on one.
- [ADR 0056](0056-typing.md): a failed command's message does not reach the log or `*Messages*` while a
  secret buffer is shown, and a failed or undefined key gets a frame. So the `<f35>` trip draws a frame on its
  first press, after the reply it measures, and never again.
- [ADR 0057](0057-frame-layout.md): with two rows, the echo row also holds what the modeline warned of.
- [ADR 0059](0059-safe-save-and-the-exit-guard.md): save failures say what happened before they name the path.
- [ADR 0061](0061-panes-clipping-and-layers.md): "a glyph stays in its pane" gains a second exception, and
  the renderer's half of this step was not all built there.
- `ui.md`, "Every message is also appended to `*Messages*`": but those above.

## Consequences

- Every text-grid frame now shows the two rows, so every snapshot changes there and nowhere else.
- A key that only changes the echo area now draws a frame, of its cost and no more. A change of position
  shapes the modeline's row again, on each key that moves the cursor; `just perf` is in the pull request.
- `*Messages*` holds at most about 4 MiB, with every message at its cap, and 50 KiB or so as they are.
- `TabSummary` grows by a word, kept twice for each chunk: about 17 MiB on a 1 GB file, or 1.6%. Each chunk costs
  one more scan as it is measured.
- The buffer's id numbers shift by one, as `*Messages*` takes the one after `*scratch*`.
- A key that changes only the echo area leaves no latency sample, as before, though it now has a frame.
- `get-state` can name a window's buffer that its `buffers` leaves out. M2-19 decides whether `C-x b` lists
  `*Messages*`.
- `C-h e` going back, and the `Place` behind it, are one window's, and go with M2-19, once `C-x b` leaves
  the buffer. M3's splits would need them per window otherwise.
- A person reading `*Messages*` does not find failures from a secret buffer, but a panic, which names only
  the command.
- A message shows only as far as the row is wide. `*Messages*` holds the rest, unless it was withheld.
- `Editor::messages_buffer` has a caller only under the `dev` feature and in tests, since tests read messages
  from the buffer.
- `oso-core`'s public API gains `Status`, `Editor::status_into`, `Editor::echo`, `Editor::clear_echo`,
  `Editor::begin_command`, `Editor::end_command`, `Editor::set_keys_in_progress`, `Editor::toggle_messages`,
  `Editor::messages_buffer`, `Editor::place_behind_messages`, `Editor::file_changed_on_disk`,
  `Buffer::is_internal` and `ReadOnly::Internal`, and loses `Editor::messages`. `oso-render` gains two `Face`
  variants.
- M3's splits need a `Status` per window, and modelines that differ between the active window and the others.

## Alternatives considered

- **Catching a panic while adding a row:** one row lost in place of the session, but ADR 0062's hook has by then
  written copies and said that oso panicked, and the editor would run on after it. Calling it a command for the hook
  needs what the tip crate holds, from every caller of `Editor::message`.

- **The position in a pane of its own,** so that a cursor's move shapes a few characters and not the row: it
  saves a little, by estimate 50 to 110 µs per key, for regions worked out from cell widths each frame. Reusing
  unchanged pieces in `Rows::keep` would be simpler, if `just perf` ever asks.
- **`Editor::user_buffers` for the name of what `Editor::buffers` now does:** says what it filters, at the cost of
  breaking every reader, including PR #40's emergency save, for what a doc comment says.
- **The secret rule in the dispatcher's arm for a failed command:** one place, but a command that calls
  `Editor::message` itself would pass it by.
- **Telling a running command by its open undo group:** no new state, but the rule would then depend on the order
  of calls, and on looking up a buffer the command may have killed.

- **A flag or a request from each thing that changes the status:** cheaper per batch, but messages come from
  commands, threads and the host, and each new one would be a chance to forget. A failed key with no frame is
  how this started.
- **Comparing only a sequence number and a few fields:** cheaper by little, and every field added must be
  remembered.
- **Spans built in `oso-core`, with `Face` moved down:** one place for text and color, at the cost of a public
  module that only the tip crate reads.
- **Warnings reserved at the right edge:** keeps the name first, but needs their width known before laying out
  the rest, and a narrow window then cuts the name to nothing.
- **Warnings on the text row of a frame of one row, in a layer above:** it could hide the cursor, and it
  would be code for a window nobody can work in.
- **Losing the band above a one-row pane, as ADR 0061 has it:** no renderer change, and a glyph stays in its pane
  with no exception, but accents on capitals lose their tops there.
- **`*Messages*` listed by `Editor::buffers`, with each reader skipping it:** the plain reading of ADR 0055, but
  one forgotten filter holds every restart or writes it in an emergency copy.
- **A filter on the secret check only for positional messages:** "Buffer is read-only" says nothing, but
  commands are many and each failure would have to be judged, where a rule for all of them needs no judgment.
- **Hiding the bidi warning for a secret buffer:** it says one thing of the text, and the person needs it most
  there. The socket does not see it.
- **Counting LRM, RLM and ALM too:** one rule for every bidi character, but every Arabic or Hebrew document warns.
- **Keeping `Editor::messages` as a second copy, as strings:** handy for tests, which now read the buffer's rows,
  and nothing else would read it.
- **Showing the prefix as the keys typed:** closer to Emacs, but the dispatcher would have to keep them, and `C-u 5`
  says the same as `M-5`.
