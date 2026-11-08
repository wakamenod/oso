# 0083. Reloading files changed on disk

- Status: Proposed
- Date: 2026-11-08

## Context

[ADR 0029](0029-file-changes-on-disk.md) decided that open files are watched and a buffer
with no unsaved edits reloads at once, keeping its cursors, and that a buffer with edits
warns and asks before a save goes over the newer file. It named OS file notifications and a
watcher crate such as `notify` for noticing the change. It left open how a reload goes in,
what undo does across it, where cursors go, what is said, what happens to a file that goes
away or keeps changing, and what a reload means for a buffer that may hold secrets.

Safe save already checks the file against the buffer's baseline before it replaces it
([ADR 0059](0059-safe-save-and-the-exit-guard.md)), the modeline already warns of a file
changed under edits, and a save already asks before it goes over one
([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md)).

## Decision

The author chose these on 2026-11-08, through the coordinator. This amends ADR 0029's
"OS file notifications".

- **Noticing a change: a look once a second, and at once when the window gains the focus.**
  A thread in `loader` `stat`s each visited path, following links. That costs microseconds
  a file and needs no dependency, no `unsafe` and no watch on a directory. A file replaced
  by a rename (git, most editors), a link pointed elsewhere and a file on a network mount all
  show the same way. The thread wakes the UI only when what it saw differs from its last
  report. The window layer reports the focus as `Event::Focused`.
- **What counts as a change:** the modification time or the size differs from the buffer's
  baseline, or a file with another device and inode is there, or nothing is. The baseline's
  stamp holds the device and inode of the file the load opened or the save wrote. The file is
  then read again off the UI thread. If the text is the same, only the baseline changes,
  silently. A change of mode alone only updates the baseline.
- **Safe save compares the same way** (amending
  [ADR 0059](0059-safe-save-and-the-exit-guard.md)): another file renamed in counts as a
  change even with the same time and size, so a save asks before it goes over it. oso's own
  save records the device and inode of the file it renamed in, so the next save does not ask.
  The author chose this on 2026-11-08 as well.
- **The editor decides.** `Editor::files_seen` takes what the looks found and says which
  files to read again. It sets and clears the warnings and says each thing once. The host
  only looks, reads and frees.
- **A buffer with no unsaved edits takes the new text as one undo step.** It is unmodified
  afterwards, since it matches the disk. Undo brings back the old text, and the buffer is
  then modified. A text that cannot be edited, a binary file's or one with invalid UTF-8,
  replaces the old one with no undo, as a load does.
- **Cursors and views:** the loading thread finds the common prefix and suffix of the old and
  new texts. Cursors and marks before and after the change keep their place, and those inside
  it keep their line and column, as far as the new line reaches. Every window showing the
  buffer keeps its top row, then follows its cursor. Regions hide, as after any edit.
- **A buffer with unsaved edits keeps them.** The modeline says `[changed on disk]`, and the
  echo area says once: "NAME changed on disk; your edits are kept". The file seen is recorded
  as the one a save may go over, so `C-x C-s` asks at once, and replaces only that file. If
  undo brings the buffer back to what was read, it reloads at the next check.
- **`revert-buffer` (`C-x x g`, as in Emacs 28)** reads the file again on request. With unsaved
  edits it asks first, "Discard your edits and reload NAME?", and the text read goes in as
  one undo step, so undo brings the edits back. A command only queues the request
  (`Editor::request_revert`), with the state the person agreed to lose; the host reads the
  file. A request waits while a prompt is open or the buffer saves. If the buffer changed
  since the person said yes, nothing goes in, and the echo area says so. A file now binary
  or not valid UTF-8, whose text would replace the edits with no undo, is refused too.
- **A read that fails**, for an error or because its file could not be placed, says so,
  shows `[changed on disk]` with edits or not, and is tried again at the next new look:
  when the file changes again, or when oso comes to the front.
- **The echo area says "Reloaded NAME".** When several reload together, as after a
  checkout, one message says "Reloaded N files: NAMES". A read running for over a second
  does not hold it back.
- **A file gone, or no longer a regular file:** the text stays, the modeline says
  `[deleted on disk]`, and the echo area says so once. A save writes the file again, as it
  does for a file deleted since it was read. A file that comes back counts as a change.
- **Large and changing files:** no size limit. The read builds a new text beside the old, and
  the buffer stays editable meanwhile. Its text goes in only once complete, and only if the
  buffer still holds what the read compared with. An edit made meanwhile drops the read, and
  the buffer is then one with edits whose file changed. One read runs per buffer at a time,
  and `revert-buffer` joins a read already running rather than start another. So a file being
  written to is read again at most about once a second, and a read that hangs is never
  doubled. The text a reload leaves, the one before it or one not put in, is freed on a
  thread of its own, as freeing a large file's text takes longer than a frame.
- **A read whose file could not be placed:** if where the opened file is cannot be told, as
  when a link is switched around the open, its text stays out, as a first load's would be
  refused a save.
- **Secrets:** a reload opens, follows and judges the file exactly as a first load does, from
  the file it opened ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)). The verdict
  never becomes less strict on a reload, as undo can bring back text read under the old one.
  A verdict that becomes stricter is noted as any change of a shown buffer's verdict is
  ([ADR 0079](0079-state-and-restarts-around-a-secret-buffer.md)). Messages name the file
  only, and the reload logs nothing of its size.
- **Waiting:** a reload neither starts nor goes in while a prompt is open
  (`Editor::reload_waits`), as what the prompt's command acts on must not change under it.
  One that finishes then is dropped and read again once the prompt closes, and a
  `revert-buffer` that has to wait is asked for again. A buffer that is still loading or
  saving is not looked at. Once a save ends, its file is looked at again and reported
  whatever it is, and a report from a look made before it is ignored.
- **A buffer still on its first load** needs nothing more: the load's baseline is from its
  own open, so the next look finds the difference.

## Consequences

- No new dependency. A change made while oso has the focus shows within a second; one made
  in another program shows as soon as oso is in front again.
- A file another program writes to all the time is read in full about once a second while
  oso shows it unmodified. A large log costs a background read each time, never the UI.
- The looks go on while oso is in the background, so a window beside a terminal follows
  the files. They cost one `stat` a file a second and wake nothing while nothing changes.
- After a reload, undo can go back past it, which Emacs's `revert-buffer` does not offer.
- A reload holds the old text and the new one, with its display map, until it goes in, more
  than twice the memory for a moment.
- A change that keeps the time, the size and the inode, on a file system with coarse times,
  is missed until the next one. Where the system gives no inode, only the time and size
  count. A file system that gives a file a new inode at each look, as some network and
  FUSE mounts do, makes every look a change: the file is read again, its text found the
  same, and a save asks before it goes over it. Telling such mounts apart is left for when
  someone uses one.
- `Editor::reload_waits` is the one place that says when a reload must wait. A mode of a
  window that a change of text would disturb adds its condition there.

## Alternatives considered

- **`notify` or another watcher crate:** reacts at once, but brings about eight crates and
  per-OS backends, and has to watch directories, since a rename replaces the file watched.
  A second's delay while oso has the focus costs little.
- **Our own kqueue and inotify in `oso-platform`:** no crates, but a new public module and
  `unsafe` bindings on three systems.
- **Clearing undo on a reload:** simpler, but a reload then loses the history of every
  buffer a checkout touches.
- **A line diff to map cursors:** places cursors better between several changed hunks, but
  needs more code and costs more on large files. Prefix and suffix are enough for the common
  case of one change.
- **Making a reloading buffer read-only, as a first load is:** a large file would then block
  editing for the length of every read.
- **The new verdict on a reload, with undo swapping verdicts back:** more state, and a
  mistake there would show a secret text as ordinary.
