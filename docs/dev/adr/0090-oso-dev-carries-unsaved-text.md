# 0090. `oso dev` carries unsaved text

- Status: Accepted, amended by 0112 and 0119
- Date: 2026-11-08

## Context

Until now any unsaved edit has held every restart of `oso dev`, since the state holds paths
and positions and the new instance reads each file again
([ADR 0047](0047-process-spawning-and-restart-handoff.md),
[ADR 0055](0055-oso-dev-never-loses-unsaved-text.md)). Now that typing, kills and marks
exist, that holds nearly every restart. [ADR 0051](0051-m2-plan-decisions.md) has already
decided the outline: carry the text in pieces under the line limit, with the modified flag,
file information, the save baseline, marks, and the kill ring without entries from secret
buffers. Undo history is not carried, internal buffers are not exported, and a secret buffer
with edits keeps the old instance. This ADR fixes the rest. Build errors also still go only to
the terminal, which an author watching the window does not see.

A response line is limited to 1 MiB ([ADR 0032](0032-control-socket-transport.md)), and the
state passes through sockets in memory only, never through files
([ADR 0023](0023-control-socket-protocol-and-restart.md)).

The author chose these on 2026-11-08, through the coordinator. The author accepted this on
2026-11-09, through the coordinator.

## Decision

### What is carried

- **For each modified buffer that cannot hold secrets:** its text, that it is modified, its
  file information (BOM and line ending), its save baseline (where the file is, and its
  modification time, size and mode), whether a save found the file changed and what it may
  save over, and the file's own spelling. A `*scratch*` with text carries it too.
- **For the window:** the cursors with their goals and the scroll position, as before, and now
  the marks, the mark ring and whether the region shows. A region shown at the restart comes
  back shown.
- **For the editor:** the kill ring, without entries from a buffer that may hold secrets or from
  the clipboard ([ADR 0074](0074-the-kill-ring-and-the-clipboard.md)), and the minibuffer
  histories.
- **Not carried:** undo history ([ADR 0051](0051-m2-plan-decisions.md)), internal buffers,
  and the fingerprints the kill ring keeps of secret entries sent to the clipboard. Those are a
  keyed hash, and the key lives only in the process that made it. Carrying both would let any
  client of the socket test guesses at short secrets offline.
- Buffers without edits are still handed over by path, read again and judged like any file.

### How the text travels

- **The state stays one line, and holds no text.** `export-state` returns `version` 2. Each
  text it carries is an **item**: a buffer's text, a kill entry's pieces one after another, or
  a history's answers one after another. The state gives each item's length, the length of
  each piece or answer in it, and what it belongs to. An `export` number names this export.
- **`export-state` takes an O(1) snapshot** of each item's rope. The pieces come from that
  snapshot, so text typed after the export goes into none of them, and the generation says it
  is there.
- **`export-text`** is a new dev method: `{export, item, offset}` gives the item's text from
  `offset`. A piece is at most 512 KiB, fewer bytes if escaping would take its line past the
  limit, and it ends on a character boundary. It is copied on a background thread, as `get-text`
  is ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)), so the UI thread does O(1)
  work. An export that is not the latest is an error.
- **`import-text`** is a new dev method: `{item, offset, text}` appends a piece to what the new
  instance stages for that item. A piece out of order is an error. The rope and display map are
  built on a background thread, as a load builds them.
- **`import-state`** then takes the state. It changes nothing unless every item has arrived at
  its full length, and a file that cannot begin to load or be judged leaves the instance clean
  again. It replies only once every carried text is in its buffer and every carried path has
  been judged, so that the emergency save covers the text before the old instance goes
  ([ADR 0062](0062-emergency-save-and-the-panic-hook.md)). A crash before the reply keeps the
  old instance and its text. The reply gives the new instance's generation right after the
  import.
- **The supervisor relays one piece at a time,** from the old instance to the new one, and holds
  nothing more. The text is never written to a file. It stays in the two editors' memory and
  passes through the same-user sockets in the control directory, which is `0700`.
- **A snapshot is dropped** at the next export, when an exit for a generation is refused, or
  at the first request 60 s after the export, which `export-text` refuses from then on.
  Staged pieces are dropped once the import takes them or is refused, or at the first
  request 60 s after the last piece. What is dropped is freed on a thread of its own.
- **All the text carried, the kill ring included, is at most 64 MiB.** If the buffers' text is
  more, the restart waits, and the echo area names the largest buffer to save. Kill entries
  fill what the buffers and histories leave, the newest first, and older ones that do not fit
  are dropped, as there is nothing to save to make room. So are those past 10,000 pieces in
  all.

### The new instance

- **Each carried buffer is judged again.** A background thread opens and resolves its path as
  a load does, and reads nothing. Until then the buffer counts as a file not judged yet, so
  the socket withholds it and the emergency save leaves it out. It can be typed into at once.
  Its positions apply at once, since its text is all there.
- **An instance with unsaved text is never closed.** `check-handover` also says whether an
  instance holds text no file does, as `unsaved`. Such an instance can hand over, but the
  supervisor does not close it, beside the old one or as one that got no window. A new instance
  that took over a state holds only what the old one still holds, so it is closed with the
  generation its import reported, and stays once it has been typed into.
- **A reader takes its own version and the one before,** so version 2 reads the version 1 state
  that instances running today write, and a later version 3 will read version 2. Anything else,
  or any bad field, leaves the new instance clean beside the old one, which keeps its text. Its
  echo area says why.

### What still holds a restart

- **A modified buffer that may hold secrets:** a file judged secret, one not judged yet, or one
  a secret kill was yanked into. Its text never leaves the instance. The echo area names it.
- **A buffer holding a recovered emergency copy not saved yet**
  ([ADR 0064](0064-recovering-an-emergency-copy.md)).
- **As before:** a save running, a prompt or a composition open, a buffer that may hold secrets
  shown ([ADR 0079](0079-state-and-restarts-around-a-secret-buffer.md)), and an instance beside
  the old one that has been edited.
- **An undo or a redo of a large step under way**
  ([ADR 0119](0119-undoing-large-steps-in-slices.md)): the handoff carries no history, so the
  undo could not be made after it.

### The generation

- **It covers the kill ring now:** each buffer's id, each modified buffer's content id (as
  before), and the newest kill entry's id and length with the number of entries. So a kill
  between the export and the exit refuses the exit, also one appended to the newest entry.
- A save between the export and the exit changes it too, since the buffer stops being
  modified.
- Marks, cursors, scroll positions and histories do not count. Like a cursor's moves, they
  change all the time while a window is in use, and they put no text at stake.

### Build errors and why a restart waits

- **cargo runs with `--message-format=json-diagnostic-rendered-ansi`.** The supervisor prints
  each diagnostic's `rendered` text to the terminal itself, in color as before.
- **The editor gets only the level, the message, the code and the primary span's file, line and
  column,** never `rendered`, so no escape sequence reaches it. Control characters are escaped
  as in any message. Warnings are not sent, nor rustc's closing count of errors. A build that
  fails before the compiler says anything, such as on a manifest, says only that it failed.
- **The echo area** says, for example,
  `oso dev: build failed: 3 errors; crates/oso/src/x.rs:12:5: mismatched types [E0308]`.
  **`*Messages*`** gets one row per error, the first 20, then `… and N more`.
- **Why a restart waits reaches the old instance's echo area the same way,** for example
  `oso dev: restart held: cannot hand over .env: it has unsaved edits and may hold secrets`.
  While a buffer that may hold secrets is shown, the reason stays the single one of ADR 0079.
- Build errors go to the current instance and to the one beside it, and why a restart waits to
  the old instance, through a new dev method, `dev-message {text}`, of at most 1 KiB.

### Smaller points

- **`get-state` shows nothing new.** Build errors are in `echo` and in `*Messages*`, and a
  carried buffer looks like any modified buffer.
- **Positions waiting for a handed-over file to load stay with that buffer.** If the window goes
  to another buffer meanwhile, such as `*Messages*` with `C-h e`, they are put in the place the
  window keeps for that file ([ADR 0088](0088-switching-and-killing-buffers.md)) once it has
  loaded, and coming back gives them. The wait ends once the window keeps no place for it.

## Changes to accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0023](0023-control-socket-protocol-and-restart.md):
  - "Restored state: everything": the text of modified buffers, marks, the kill ring and the
    histories are restored now. Undo history is still not ([ADR 0051](0051-m2-plan-decisions.md)).
  - The state carries text in pieces through two more methods, `export-text` and
    `import-text`, still in memory only.
  - "If the version does not match … the new instance starts clean": a reader also takes the
    version before its own.
- [ADR 0047](0047-process-spawning-and-restart-handoff.md):
  - "The state holds paths and positions, never text": it now holds the text of modified buffers
    that cannot hold secrets, in pieces, at version 2. A buffer without edits is still read from
    disk.
  - "Until the echo area exists (M2), build errors stay in the terminal": they reach the echo
    area and `*Messages*` now.
  - "with errors going straight to the terminal": the supervisor prints them there itself.
- [ADR 0055](0055-oso-dev-never-loses-unsaved-text.md):
  - "A file with unsaved edits makes `export-state` fail": only one that may hold secrets, one
    holding a recovered copy not saved yet, or carried text over 64 MiB in all.
  - The generation covers the kill ring too, as above, and "a save between the export and the
    exit does not change it" no longer holds: the saved buffer stops being modified.
  - "No generation is ever reported while a file holds edits", which made closing an instance
    safe: one is, and `unsaved` is what keeps such an instance from being closed.
- [ADR 0059](0059-safe-save-and-the-exit-guard.md), "`oso dev`'s handoff reads the file again,
  so a fresh baseline comes with it": a carried buffer brings the baseline it had, and whether a
  save found its file changed.
- [ADR 0071](0071-mark-and-region.md), "A restart drops the marks and the ring": a restart
  carries them, and whether the region shows.
- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md), a history is "never written
  to disk or carried through a restart": `oso dev` carries it to the instance that replaces this
  one. It is still never written to disk.
- [ADR 0074](0074-the-kill-ring-and-the-clipboard.md), "Where an entry came from survives the
  round trip": not through a restart under `oso dev`, which does not carry the fingerprints,
  for text the clipboard has not kept marked concealed.
- [ADR 0079](0079-state-and-restarts-around-a-secret-buffer.md), a file not judged yet "is
  loading, as after a restart, and its positions are the ones it came in with": a carried buffer
  is not judged yet and not loading, and its positions apply at once. It still does not count
  as a secret buffer shown.

## Consequences

- A restart goes through with unsaved edits, and edits in a buffer that may hold secrets are
  the only ones that hold it for good.
- After a restart, undo starts afresh. A carried buffer stays modified until it is saved, even
  if its text is changed back by hand.
- A large unsaved buffer slows the restart: 64 MiB takes two requests per 512 KiB piece.
- After a restart, a secret that went to the clipboard comes back by a yank as ordinary text,
  since its fingerprint is not carried, unless the clipboard kept the concealed mark oso put on
  it. This happens only under `oso dev`.
- A same-user client can read the unsaved text of a buffer that cannot hold secrets through
  `export-text`, as it already can through `get-text`, and can stage text in a new instance, as
  it already can by typing.
- The new instance's echo area says nothing on a restart that worked. The log has the timings.
- The supervisor still runs the code it was started with, so build errors reach the echo area
  only from a `just dev` started after this change. Each message also reaches the instance's
  log, which goes to the same terminal, so the errors show there a second time, one row each.
- Old kill entries can be dropped at a restart, when the buffers' text and newer kills fill
  64 MiB.
- Two instances that each hold unsaved text both stay until the person closes one, as ADR 0055
  has it for two edited ones: an instance left beside the old one after it was typed into holds
  later restarts.

## Alternatives considered

- **Carrying only the text, the flag and file information:** less code, but a kill or a mark
  would still be lost on each build.
- **Carrying undo history:** a much larger handoff, which ADR 0051 rejected.
- **The whole state on one line:** it stops working at about 1 MiB of text.
- **A `0600` temporary file in the control directory:** one write instead of many requests, but
  ADR 0023 keeps the state out of files, and a crash would leave the text on disk.
- **Carrying the secret fingerprints with their key:** a client could then test guesses
  offline. Holding the restart while one exists would hold it for the rest of the session.
- **A cap of 256 MiB, or none:** more text carried, at a restart several times slower.
- **A reader of its own version only:** each change of the format would hold every restart until
  everything is saved.
- **Keeping the old instance until the new one has run for a while:** two windows stay up
  longer, and an editor that crashes later is any editor that crashes, which the emergency save
  covers ([ADR 0062](0062-emergency-save-and-the-panic-hook.md)).
- **Counting marks in the generation:** a window in use would seldom hand over.
- **The whole rendered diagnostics, without escapes, in `*Messages*`:** many rows each, which
  the terminal already shows better.
- **A `get-state` field for the last build:** the echo area already says it.
