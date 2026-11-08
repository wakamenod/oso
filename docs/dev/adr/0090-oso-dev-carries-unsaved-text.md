# 0090. `oso dev` carries unsaved text

- Status: Proposed
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

The author chose these on 2026-11-08, through the coordinator.

## Decision

### What is carried

- **For each modified buffer that cannot hold secrets:** its text, that it is modified, its
  file information (BOM and line ending), its save baseline (where the file is, and its
  modification time, size and mode), whether a save found the file changed and what it may
  save over, and the file's own spelling. A modified `*scratch*` carries its text too.
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
  text it carries, whether a buffer, a kill ring piece or a history, is an **item**: the state
  gives its length and what it belongs to, and an `export` number names this export.
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
  its full length. It replies only once every carried text is in its buffer, so a crash before
  the reply keeps the old instance and its text.
- **The supervisor relays one piece at a time,** from the old instance to the new one, and holds
  nothing more. The text is never written to a file. It stays in the two editors' memory and
  passes through the same-user sockets in the control directory, which is `0700`.
- **A snapshot is dropped** at the exit, at the next export, or 60 s after the export. Staged
  pieces are dropped once the import is done or refused, or 60 s after the last piece.
- **All the text carried, the kill ring included, is at most 64 MiB.** Above that the restart
  waits, and the echo area says which buffer to save.

### The new instance

- **Each carried buffer is judged again.** A background thread opens and resolves its path as
  a load does, and reads nothing. Until then the buffer counts as a file not judged yet, so
  the socket withholds it. It can be typed into at once. Its positions apply at once, since its
  text is all there.
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

### The generation

- **It covers the kill ring now:** each buffer's id, each modified buffer's content id (as
  before), and the newest kill entry's id with the number of entries. So a kill between the
  export and the exit refuses the exit.
- A save between the export and the exit changes it too, since the buffer stops being
  modified.
- Marks, cursors, scroll positions and histories do not count. Like a cursor's moves, they
  change all the time while a window is in use, and they put no text at stake.

### Build errors and why a restart waits

- **cargo runs with `--message-format=json-diagnostic-rendered-ansi`.** The supervisor prints
  each diagnostic's `rendered` text to the terminal itself, in color as before.
- **The editor gets only the level, the message, the code and the primary span's file, line and
  column,** never `rendered`, so no escape sequence reaches it. Control characters are escaped
  as in any message. Warnings are not sent.
- **The echo area** says, for example,
  `oso dev: build failed: 3 errors; crates/oso/src/x.rs:12:5: mismatched types [E0308]`.
  **`*Messages*`** gets one row per error, the first 20, then `… and N more`.
- **Why a restart waits reaches the echo area the same way,** for example
  `oso dev: restart held: notes.txt has unsaved edits and may hold secrets`. While a buffer that
  may hold secrets is shown, the reason stays the single one of ADR 0079.
- They go to the current instance and to the one beside it, through a new dev method,
  `dev-message {text}`, of at most 1 KiB.

### Smaller points

- **`get-state` shows nothing new.** Build errors are in `echo` and in `*Messages*`, and a
  carried buffer looks like any modified buffer.
- **Positions waiting for a handed-over file to load stay with that buffer.** If the window goes
  to an internal buffer, such as with `C-h e`, they are applied to the place it goes back to
  once the file has loaded. Showing another buffer ends the wait.

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
  - The generation covers the kill ring too, as above.
- [ADR 0071](0071-mark-and-region.md), "A restart drops the marks and the ring": a restart
  carries them, and whether the region shows.

## Consequences

- A restart goes through with unsaved edits, and edits in a buffer that may hold secrets are
  the only ones that hold it for good.
- After a restart, undo starts afresh. A carried buffer stays modified until it is saved, even
  if its text is changed back by hand.
- A large unsaved buffer slows the restart: 64 MiB takes two requests per 512 KiB piece.
- After a restart, a secret that went to the clipboard comes back by a yank as ordinary text,
  since its fingerprint is not carried. This happens only under `oso dev`.
- A same-user client can read the unsaved text of a buffer that cannot hold secrets through
  `export-text`, as it already can through `get-text`, and can stage text in a new instance, as
  it already can by typing.
- The new instance's echo area says nothing on a restart that worked. The log has the timings.
- The supervisor still runs the code it was started with, so build errors reach the echo area
  only from a `just dev` started after this change.

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
