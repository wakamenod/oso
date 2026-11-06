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
- **The renderer does not change,** as ADR 0061 meant.
- **Building a `Status` reuses its strings,** so it allocates nothing once they have grown.
  It reads the buffer through the cursor's position and a summary: O(log n) plus a scan of
  one chunk, what drawing the cursor already costs.

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
  stays at the expense of what follows it. The end is cut, so a shared start hides what
  differs in the worst case. Telling such names apart is M2-19's.
- **The marker is Emacs's:** `--`, `**` once modified, `%%` read-only and `%*` read-only and
  modified.
- **The column counts from 0 and the line from 1,** as in Emacs. Both are display rows and
  columns, so M2-25 must keep the line a logical one when rows wrap.
- **The major mode reads `Fundamental`** until modes arrive (M3).
- **`[bidi controls]` warns** for the nine Trojan Source controls (U+202A to U+202E, U+2066 to
  U+2069). LRM, RLM and ALM are left out: they can only tip a neutral character, and
  right-to-left prose has them everywhere. All twelve are still boxed.
- **The count is in `TabSummary`,** so asking costs O(1) and an edit updates it along with the
  rest. Each chunk is scanned once: a vectorized count of the lead byte 0xE2, then its characters
  only if it has one. Plain text pays about what counting line breaks does.
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
The name, the marker, the encoding and loading progress stay, as `get-state` gives them anyway.
The warnings stay too: they tell the person at the screen something they need, and they stay out
of `get-state` with the rest of the modeline.

This costs something. A person editing `.ssh/config` sees no line number, and the text itself is on
the same screen. Showing the position only to the screen, with `get-state` and screenshots withheld,
would also hold today. It is one condition in `push_state`, and the author may prefer it.

### Messages

- **`Editor::message` caps a message at 4096 bytes,** room for the longest path a message names
  (`PATH_MAX` on Linux). A longer one is cut on a character boundary and ended by `…`.
- **`Editor::echo` reaches the echo area alone,** and neither the log nor `*Messages*`. This is how
  "C-x C-z is undefined" and "Quit" get there, so typed keys stay out of the `info` log (ADR 0056). Its
  text shows the keys. Unbound keys still reach the `debug` log as ADR 0056 has it.
- **What a command says when it fails while a secret buffer is shown goes the same way,** whichever
  way it came. "End of buffer" says where the cursor is, and a log or `*Messages*` keeps it. A panic
  still reaches the log, as it names only the command and is a bug to find. This changes ADR 0056's "A
  failed command's message goes to `Editor::message`, whatever invoked it".
- **What was echoed while a secret buffer was shown goes when none is shown,** by any path. A frame
  checks this as it reads the echo area, so a window moved by the socket shows no failure of the buffer
  before. A message from outside a command, such as a finished save, names a file and quotes no text,
  so it stays.
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
- **`*Messages*` in view counts its messages,** since the same message twice leaves the echo area as
  it was. The buffer's revision would do, but a load piece changes that of any buffer.

`Effects::redraw` keeps its meaning for what commands change in the text. An unbound key shows once:
repeating it leaves the `Status` the same, so it draws nothing more.

### `*Messages*` and internal buffers

- **`Buffer::is_internal` marks a buffer the editor keeps for itself.** It is read-only for keys
  (`ReadOnly::Internal`), has no undo history, and is never modified.
- **`Editor::buffers` leaves them out.** So `oso dev`'s check, generation and import, the exit guard
  and the emergency save need nothing new, and a new reader of the user's work cannot forget them. Who
  wants one asks for it by id. This fails safe, where a filter in each reader would not.
- **The editor makes `*Messages*` when it starts,** so a message never finds it missing, and
  `kill_buffer` leaves it. M2-19's `kill-buffer` decides whether a person may.
- **Each message is one row, as the log escapes it,** so a file name cannot forge a line, and a name's
  bidi control does not set off the warning there. The echo area and the modeline draw through
  `text_clusters` instead, which boxes it. Both stop at the 1000 messages `Editor::messages` keeps.
- **A window showing `*Messages*` keeps a cursor at the end there,** as new rows arrive.
- **`view-echo-area-messages` on `C-h e`** shows it, with the last row in view. Since nothing else
  leaves it until `C-x b` (M2-19), the same key goes back to where the window was. So a person can read it,
  and a client of the socket can too.
- **A window on `*Messages*` comes back on `*scratch*`** after a restart, with no change to the state's
  format. It is exported as one more entry with no path. Its text is not carried.
- **An instance whose window is on `*Messages*` refuses an import,** since it has no `*scratch*` in view
  for the state to start from. Its messages alone do not refuse one.

### The band above a one-row pane

**The modeline and the echo area keep losing the band above their row,** as ADR 0061 has it. Their text is
names and short messages, on a row of its own. It needs no renderer change, and a name from disk never paints
on the row above it. Vietnamese capitals lose the top of their accents there, which is checked on screen, as
reported in the pull request. M2-16 and M3 still decide this for their panes.

### The control socket

This is a human gate ([`REVIEW.md`](../../../REVIEW.md)). `get-state` gains `modeline` and `echo`, as ADR 0051
foresaw, and leaves both out while any window shows a secret buffer. Screenshots already refuse then.

### Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0056](0056-typing.md): a failed command's message does not reach the log or `*Messages*` while a
  secret buffer is shown, and a failed or undefined key gets a frame.
- [ADR 0057](0057-frame-layout.md): with two rows, the echo row also holds what the modeline warned of.
- `ui.md`, "Every message is also appended to `*Messages*`": but those above.

## Consequences

- Every text-grid frame now shows the two rows, so every snapshot changes there and nowhere else.
- A key that only changes the echo area now draws a frame, of its cost and no more. A change of position
  shapes the modeline's row again, on each key that moves the cursor; `just perf` is in the pull request.
- `Editor::messages` and `*Messages*` each hold a copy, at most about 8 MiB between them with every message
  at its cap, and 100 KiB or so as they are. `Editor::messages` can go once its readers are tests alone.
- `TabSummary` grows by a word per tree node, and each chunk costs one more scan as it is measured.
- The buffer's id numbers shift by one, as `*Messages*` takes the one after `*scratch*`.
- A person reading `*Messages*` does not find failures from a secret buffer.
- A message shows only as far as the row is wide. `*Messages*` holds the rest, unless it was withheld.
- `oso-core`'s public API gains `Status`, `Editor::status_into`, `Editor::echo`, `Editor::echo_withheld`,
  `Editor::clear_echo`, `Editor::set_typing`, `Editor::toggle_messages`, `Editor::messages_buffer`,
  `Editor::file_changed_on_disk`, `MESSAGES`, `MAX_MESSAGE_BYTES`, `Buffer::is_internal`,
  `Buffer::changed_on_disk`, `ReadOnly::Internal`, `DisplaySnapshot::has_bidi_controls`,
  `TabSummary::bidi_controls` and `cell::is_bidi_control`. `oso-render` gains two `Face` variants.
- M3's splits need a `Status` per window, and modelines that differ between the active window and the others.

## Alternatives considered

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
- **Keeping the band above a one-row pane by painting on the row above:** keeps accents whole, but needs a
  renderer change and an exception to "a glyph stays in its pane", for names and messages alone.
- **`*Messages*` listed by `Editor::buffers`, with each reader skipping it:** the plain reading of ADR 0055, but
  one forgotten filter holds every restart or writes it in an emergency copy.
- **A filter on the secret check only for positional messages:** "Buffer is read-only" says nothing, but
  commands are many and each failure would have to be judged, where a rule for all of them needs no judgment.
- **Hiding the bidi warning for a secret buffer:** it says one thing of the text, and the person needs it most
  there. The socket does not see it.
- **Counting LRM, RLM and ALM too:** one rule for every bidi character, but every Arabic or Hebrew document warns.
- **Removing `Editor::messages`, with `*Messages*` as the only copy:** less memory, but tests and callers
  read messages as strings, and a rope gives them back in pieces.
- **Showing the prefix as the keys typed:** closer to Emacs, but the dispatcher would have to keep them, and `C-u 5`
  says the same as `M-5`.
