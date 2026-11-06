# 0059. Safe save and the exit guard

- Status: Accepted
- Date: 2026-11-05

## Context

[ADR 0013](0013-persistence.md) settled on a temporary file, fsync and a rename, and
[ADR 0029](0029-file-changes-on-disk.md) on a check of the modification time right before
the rename. [ADR 0053](0053-edit-core.md) brought `content_id` and `mark_saved`. Nothing wrote
a file yet, `C-x C-c` and the window's close button exited at once, and the loader dropped the
metadata of the file it opened.

This is what lets the author trust oso with real notes, so losing or damaging a file is the
failure to design against. The UI thread must not wait on any of it, and a command has no
threads or files of its own ([ADR 0034](0034-command-registry-and-keymaps.md)).

## Decision

### The order of a write

On a thread of its own, each step before the next:

1. **Follow symbolic links** one at a time to the file itself, up to 40. A link stays a link, and
   a link to a file that does not exist creates that file. A relative target counts from the
   link's directory, and nothing is normalized by hand, so the system walks `..` as it would for
   the link. Nothing about secrets is judged here: a secret buffer saves like any other, since
   only the emergency save skips them ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)).
2. **Check the baseline** (below), so that a path now leading elsewhere writes nothing there, and a
   file already changed costs no write. If the file exists, also open it for writing and close it. A rename needs only the directory, so without
   this a save would replace a file the user made read-only or may not write. Opening writes
   nothing and changes no time.
3. **Create a temporary file** with `create_new` (`O_EXCL`) and mode `0600`, in the directory of the
   file itself, so that the rename stays on one file system. The name is
   `.oso-save-<process>-<nanoseconds>-<attempt>.tmp`, trying the next attempt if it exists. It is
   hidden and says what it is. It holds nothing of the file's name, which could make it too long
   for the system, and `create_new` means it is never a link someone left there.
4. **Write** the BOM if the file had one, then the rope's chunks through a 1 MiB buffer. Line endings
   are already in the rope.
5. **Set the attributes, fsync and close.** One fsync after the attributes, so that a crash cannot leave
   the new file in place with mode `0600`. If the file system says it cannot sync at all, the save goes on
   and the message says a power cut could lose the text and any file it replaced. `std` makes `sync_all` a bare `F_FULLFSYNC` on macOS,
   with no fallback, and some network and FUSE mounts refuse it, so otherwise nothing could be saved there. Any
   other error fails the save. The stamp to record next is read here, by the temporary file's name, after the
   close, as Windows may settle the modification time only then, and a size other than what was written fails
   the save.
6. **Check the baseline again,** right before the rename. A file whose mode or owner changed during the
   write counts as changed too, since the rename would undo a `chmod`, and so does one that came back
   after being gone at step 2. The stamp to record next has by then been read, so that a change made just
   after the rename still counts as a change at the next save.
7. **Rename, then fsync the directory.** If that fsync fails, the file has still been replaced, so the
   save counts as done and the message adds that a power cut could bring back what was there. Some network
   file systems cannot do it at all.

The temporary file goes in the real directory of the file's location, so a link changed during the write does
not lose track of it. It is removed when anything fails, also on the way out of a panic, by a guard that only
the rename disarms. Up to the rename the file is untouched, and after it the file holds the whole new text. If the
file system cannot sync, that holds only as far as the file system can be told: the size is checked after the
close, as `std` does not report a failed close, and the message says a power cut could lose the text and any file it replaced.

### The baseline and the conflict rule

- `Buffer` holds a `Baseline`: its **location**, the real place the path led (links followed, and the part of the
  directory that exists made canonical), and the **file** there: its modification time, size and mode, or nothing if
  there was none. The loader finds these for the file it opened, the stamp from the open file itself, and hands them
  over before the first update, as it does the resolved path. `oso dev`'s handoff reads the file again, so a fresh
  baseline comes with it.
- **The location must hold the opened file.** It is worked out after the open, so a link switched in between would pair
  one file's text with another place. The loader checks that the location holds the very file it opened, by device and
  inode on Unix. `std` keeps a file's identity unstable on Windows, so there it compares the stamp. If they differ, or nothing is
  there, no baseline is recorded, and a message says the file cannot be saved.
- A buffer has no baseline until the host says, and without one a save refuses, and says that where the file leads could
  not be told. So a bug that loses it cannot replace a file.
- **The text goes only where the file was.** A path that now leads elsewhere, because a link or a directory on the way
  changed, writes nothing at the new place, and the message says so. This holds even if nothing is there, so what
  follows about deleted files cannot redirect a buffer. It also keeps a secret buffer's text where it was judged
  ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)). A directory made since a new file was visited is the
  same place.
- **The check is for inequality,** not "newer": a backup restored with an older time is a change too. The mode does not
  count: `chmod` changes no text.
- **A change keeps the file.** Nothing is written over it, and the temporary file goes. The buffer stays modified, and a
  message says why. A file that appears where there was none is a change, and so is anything that is not a regular file
  with the baseline's time and size.
- **A deleted file is not a conflict.** It holds nothing to keep, and refusing would leave the text with no way to be
  saved. Emacs does the same. The file is written again in the same place, with its mode as last read or written, narrowed
  as for a lost group below, since nothing says which group it had. A `chmod` made since is not known.

### Attributes of the replaced file

A rename puts a new file in the old one's place, so nothing carries over by itself.

- **Mode:** kept, including `0600` and the executable bits, less set-user-ID and set-group-ID, which a write
  also clears, and the sticky bit. A file that never existed gets what the umask gives, also one with a name
  that may hold secrets, as any other program would. `std` cannot read the umask, so a
  second temporary file, created with mode `0666`, is looked at and removed.
- **Owner:** kept if it already matches, or if `fchown` works, which means running as root. Otherwise **the
  save is refused,** as a save must not take someone else's file over silently. A shared file in a group can
  therefore be edited but not saved, until M2-16 can ask.
- **Group:** kept if the user may set it. If not, the save goes on, and that group gets no more access than
  everyone else, since the group bits were meant for the old group. A file moved out of `/tmp` on macOS is
  often in a group its owner is not in, so refusing would hurt, and its usual `0644` does not change.
- **Extended attributes and ACLs: lost.** `std` has no way to read or copy them, so what they allowed or denied
  no longer applies, and only the mode does. On macOS that includes Finder tags and the quarantine flag, and on
  Linux `security.selinux` falls back to the directory's default. This is recorded in `open-questions.md`.
- **Hard links: broken, and this is accepted.** Other names keep the old text. Nothing is lost, as both texts exist.
  Detecting it would be platform code for a rare case, and refusing would make those files unsaveable.
- The inode and the creation time are new.

### Rules for saving

`save-buffer` (`C-x C-s`) decides in this order, and refuses without changing anything:

- **Read-only** buffers (binary, invalid UTF-8, loading) are refused, with the reason. They can never be
  saved ([ADR 0022](0022-text-buffer-and-file-loading.md)).
- **A save of this buffer is running:** a second one is **refused, not queued.** A queued save would have to take
  its snapshot and its final newline at some later moment, outside any command and any undo group. Refusing
  needs one flag, and leaves the buffer modified with a message that says so. On an SSD a human rarely sees
  it, since a 1 GB file takes a fraction of a second. On a slow disk or a network file system it can last for
  seconds or more.
- **Unmodified** says `(No changes need to be saved)` and writes nothing, also for a new file left untouched.
  This comes before the next rule, as in Emacs, so `C-x C-s` in an untouched `*scratch*` has nothing to do.
  It is a message and not a failure.
- **Except a file that has gone.** An unmodified buffer that read a file may hold the only copy of it, as Emacs knows.
  Whether the file is there needs the disk, so the command queues a job that writes only if nothing is at the file's
  location, and otherwise reports that there is nothing to save. It adds no final newline: the text goes back as it was. If
  the file has gone and the path also leads elsewhere now, the message says so instead.
- **No file** says `Buffer has no file name`. `C-x C-w` comes in M2-18.

Then:

- **The final newline** ([ADR 0030](0030-m2-editing-commands.md)) is an edit of its own, made before the snapshot, so
  the buffer matches the disk. It uses the file's line ending, is one undo step, and is skipped for an empty
  buffer. **Cursors at the end stay before it,** as Emacs's do, so typing after a save continues the line.
- `mark_saved` takes the content id of the snapshot, so edits made while a save runs stay modified, and an undo
  made while it runs leaves the buffer modified once it ends.
- A missing directory is an error, and none is created. Emacs asks, and prompts come later.
- Messages go through `Editor::message`: `Wrote PATH`, or what went wrong, naming the path and never the text.
  Drawing them is M2-7.

### Where things live

- **A command cannot start a thread, so it queues.** `Editor::start_save` makes the checks and the edit and adds a
  `SaveJob`, which holds an O(1) snapshot. `App::apply`, which already runs after every command, takes the jobs
  and starts them. This is also the seam for tests: `saver::run` does a job with no thread, and `saver::apply` takes
  its report in. A panic on the thread is caught and reported, so a buffer cannot stay "saving".
- **A job is not an `Effects` request.** `Effects` ([ADR 0034](0034-command-registry-and-keymaps.md)) is `Copy`, and compares in
  every command test, while a job holds a rope and a path. So requests that carry data wait on `Editor`, and `Effects` stays for
  plain flags.
- **What an exit under way does is decided in `saver::Leaving`,** with no window, and `app/saves.rs` only acts on it. So every case
  has a test through the `Dispatcher`.
- `saver` is a new top-level module of the tip crate, in a layer of its own just above `loader` in `guardrails/layers.txt`,
  since the two must agree on how a path resolves and on what is recorded of a file. `app/saves.rs`
  sits beside `app/loads.rs`, and `UserEvent::Save` carries the report, which holds no text.
- **The `cfg(unix)` file code stays in `saver` for now.** [ADR 0003](0003-platform-tiers.md) puts OS-specific code behind
  `oso-platform`. This is the first in a release build, and all of it is `std`: modes, `fchown` and syncing a directory, each with a
  portable fallback. It stays here until `libc` or a second user arrives. M2-10 is that user, for the temporary file's guard, and
  needs either a layer for it or a move to an `oso_platform` file-system module, with its own ADR. Saving is the only code that
  writes files, so nothing yet gains from the split.
- `oso-core` gains no public module. `editor/save.rs` and `buffer/baseline.rs` are private, and their types are
  re-exported from `editor` and `buffer`. `decode::BOM` becomes public.

### The exit guard

- `C-x C-c` and `WindowEvent::CloseRequested` refuse once while any buffer is modified **or a save is running**, with a
  message naming how many: `2 modified buffers and 1 save running; repeat to exit anyway`. A running save counts
  separately, since an undo can leave a buffer unmodified while the disk is about to change.
- **Repeating the same action confirms.** The keys do not confirm the button, or the other way around.
- **Any other key oso turns into a key press or text resets it:** a command, even one that fails, typed text, an unbound
  key or a cancelled prefix. The `C-x` of the repeat does not, as a pending prefix has not decided anything yet. The
  dispatcher sees this by comparing the refused action before and after each command. A mouse wheel does not reset it,
  and neither does a key that reaches no dispatcher, such as a dead key or one with no Emacs name.
- **A save that ends with the buffer still modified resets it,** whether it failed or the text moved on while it ran, since
  the refusal no longer says what is at risk. An undo during a save can leave a buffer looking unmodified at the refusal
  and differing from the disk only once the save ends. A save that leaves nothing at risk does not need to: the next try
  exits at once.
- **A save still running at a confirmed exit gets to finish.** The user asked for it, and a process that ends mid-write
  would leave its temporary file behind. The window is hidden, and the event loop keeps running until the last save has
  reported. It has to keep running: a window closed or hidden after the loop stops stays on the screen, frozen, as a slow
  save on macOS showed. In that time the instance counts as exiting: input is ignored and every control request is refused, as
  [ADR 0055](0055-oso-dev-never-loses-unsaved-text.md) asks.
- **A failed save takes the exit back.** The person left believing the text would reach the disk, and with a conflict or a
  full disk it did not. So the window returns with the message, the buffer stays modified, and the guard starts over,
  at once, whatever other saves still run.
- **So does a buffer that comes to differ from the disk while the exit waits.** After an undo during a save, a buffer looks
  unmodified, and the refusal speaks only of the save. Once it lands, the disk holds the text that was undone, and what the person
  sees exists only in this process. They never agreed to lose that. So the exit remembers which buffers were modified when it was
  agreed, and any other that is modified when a save ends takes it back. A buffer that was already modified, and was counted in the
  refusal, does not.
- **After 10 minutes a person gets the window back too,** as nobody can tell where the text is, with a message that says so.
  The save may still be running, so saving that buffer stays refused until it ends. Exiting again is guarded as usual,
  and then does not wait for that save.
- **`exit` over the socket goes whatever happens,** as ADR 0055 defines it. It waits for a running save too, for at most the 10
  minutes, and a failure logs nothing beyond the message at the `info` level. An exit the OS makes (Dock quit, logout) does not
  take this path, and is M2-10's.
- A deleted file does not count for the guard, as telling needs the disk. A person can exit with the only copy of such a text
  until M2-22, recorded in `open-questions.md`.
- While an exit is under way, the window's size, its scale and the modifier keys still count, though no input does, so that it
  returns as it should if taken back. It asks for attention then, and does not take the keyboard from another app, as it may be
  minutes later and the person may be typing a password elsewhere.
- The state lives on `Editor`, like the rest of what commands read. The proper prompt replaces this in M2-16.
- `exit` over the socket stays as [ADR 0055](0055-oso-dev-never-loses-unsaved-text.md) defines it. winit's `exiting` (Dock
  quit, logout) is M2-10.

### The control socket

- **`check-handover`, `export-state` and a guarded `exit` refuse while a save runs.** The buffer may look unmodified after
  an undo, while the file a new instance would read is about to change. This adds to ADR 0055.
- **`save-buffer` writes only to where the file was,** so it gives a client no way to read a secret buffer's text
  elsewhere. `C-x C-w` (M2-18) must keep to that.
- **`get-state` gains `modified` and `saving`** for each buffer. They tell nothing new: `bytes` and a refused handover already
  gave the first away.
- **Dev builds trust the same user to write, as they do to read.** This extends that trust and settles what
  [ADR 0056](0056-typing.md) and `open-questions.md` left to M2-8. There is no dev-only list of places it may write. `open-file`, `send-keys` and `save-buffer`
  together can write any file the dev instance may write, secret or not. That includes files whose edits an agent's tool would
  ask about, such as the check definitions in `AGENTS.md` and git hooks, and a sandboxed client given the socket can write outside its
  sandbox. Refusing would mean telling keys from the socket from keys from the window, which `send-keys` is there not to do
  ([ADR 0034](0034-command-registry-and-keymaps.md)). Release builds have no socket. Not giving a client the socket is how
  to confine it, and agents following `verify-in-app` use it on test files only.

## Consequences

- On the author's Mac, on an SSD, `C-x C-s` on a 1 GB buffer costs the UI thread about 20 µs for the command, about 45
  µs if it adds the final newline, and 8 to 14 µs to start the thread. The whole save takes 150 to 170 ms, and keys typed meanwhile
  took 0.3 to 6.4 ms from the socket to a presented frame. With `trace` logging to a file on the same disk, one key per save took 25 to 30 ms:
  the UI thread's log write waits for the save's fsync. No benchmark tracks any of this yet, so these numbers come from one script run
  for the pull request.
- **A conflict, or a load that could not tell where its file is, has no way out yet.** Until M2-16 can ask and M2-22 can reload, or M2-18 can write elsewhere, such a buffer
  cannot be saved, and `oso dev` holds restarts for it. For a conflict, moving the changed file away and saving again is what is
  left. For a load that could not tell, and for a file owned by someone else, nothing is.
- A window remains between the last check and the rename, about as long as a few system calls, in which another program's write
  is lost, or a link change is not seen. `renameat2` with `RENAME_NOREPLACE` could close it for a new file, but is not in `std`.
- A change that keeps both the size and the modification time goes unseen. Time stamps are nanoseconds on APFS and about 4 ms on
  ext4, but a program can set them. Adding the inode would catch a rename by another safe saver, if this ever matters.
- A kill signal or a power cut during a save, or leaving after the 10 minutes, can leave a `.oso-save-*.tmp` beside the file, mode
  `0600` or by then the file's own, holding the buffer's text or part of it, also a secret buffer's. Nothing cleans those up yet, and
  they look like what they are. `open-questions.md` holds this for M2-11.
- A directory the user may enter but not write stops a save even for a file in it that they may write. A rename needs the
  directory, and writing in place is exactly what this design does not do.
- Saving replaces the file, so a program watching an inode rather than a path sees it as deleted. `oso dev`'s watcher is not affected,
  and neither is `tail -F`.
- Two buffers visiting one file can each pass their check while the other writes. The last rename wins, and each file written is
  whole. Only `oso FILE` and dev's `open-file` can make that happen yet.
- `just perf`'s `stop` sends `C-x C-c` twice now, since benchmarks leave buffers modified.
- **Until M2-7 draws messages, a refused exit, a conflict and a failed save reach only the log.** A close button that seems dead, or a
  save that seems to work, are what a user sees then, so real notes wait for M2-7. A failed save asks for no frame, as a failed key
  does, which M2-7 must change.
- M2-10 can share the temporary file's guard. It must not reuse `start_save`, which edits.
- Text can no longer be lost by a single `C-x C-c`, as ADR 0055 noted it could until now. A bare `exit` over the socket still loses it,
  on purpose.

## Alternatives considered

- **Writing in place, or a backup then in place:** keeps the inode, hard links and all attributes, but a crash in the middle leaves a
  damaged file, which is the one thing this is designed against.
- **Falling back to writing in place when the owner cannot be kept,** as Vim does: makes shared files saveable, at the price of a code
  path with none of these guarantees, used only in the rare case nobody tests. M2-16 can ask.
- **Taking the file over silently when the owner differs,** as Emacs's `file-precious-flag` does: the old owner may lose access with no
  word about it.
- **Refusing when the group cannot be kept:** the safest reading, but it refuses ordinary files on macOS over a difference that gives
  nobody anything.
- **Comparing only time and size, and treating any missing file as deleted:** simple, but a link pointed at a missing path since the
  load would send the buffer there.
- **The file's identity (`dev` and `ino`) instead of its location:** catches a swapped file, but says nothing once the file is gone,
  and only on Unix.
- **Refusing to save over a hard-linked file:** the file could then not be saved at all until prompts exist.
- **`libc` or a crate for `xattr` and ACLs:** `libc` is dev-only until M2-10, and no dependency is added here.
- **The file's name in the temporary name (`.notes.txt.oso`),** as many editors do: easier to recognize, but a name near the limit of 255
  bytes could then not be saved.
- **A random temporary name:** needs a dependency or `/dev/urandom`. `create_new` makes a collision harmless, and the name is not a secret.
- **Setting the mode after the fsync,** as first planned: a crash after the rename could leave the file at `0600`, or a second fsync would be
  needed. Nothing reads the file until the rename, so nothing depends on the other order.
- **Setting the mode after the rename:** the file would have the wrong mode, visibly, for a moment.
- **Taking the new baseline by a `stat` after the rename:** a change made in between would be taken for the save's own.
- **Treating a `chmod` during the write as harmless:** the rename would quietly undo it, and `chmod 600` is usually done for a reason.
- **The baseline in `FileInfo`:** that is `Copy` data the decoder fills in, with nothing about the disk, and it would come through
  `LoadSnapshot` from a builder that never sees the file.
- **Counting a deleted file as a conflict:** consistent, but the text would have no way out, for a file with nothing to protect.
- **Queueing a second save:** see above.
- **A cancel flag, so that a confirmed exit drops a running save:** the user would lose a save they asked for, for a wait that is usually
  milliseconds.
- **Waiting without a limit at exit:** a stuck network file system would leave a process with no window, for good.
- **A dev-only list of places the socket may write:** would confine a client, but `send-keys` would then differ from a key,
  and a list is one more thing to keep up for a tool only the same user can reach.
- **Exiting anyway when a save fails after a confirmed exit:** simpler, and the message did say "exit anyway", but the person
  left with the text in a save they had asked for, and the text would be gone.
- **Waiting after the event loop has ended:** simpler, with no state for leaving, but the window stays on the screen, frozen,
  until the save ends, and macOS may report the app as not responding.
- **A limit on the time with no progress, rather than in all:** never abandons a slow save, but needs a shared counter and its own
  allowance for fsync, which reports no progress, for a case 10 minutes already covers.
- **Requests in `Effects`:** one channel for everything a command asks of the host, but it could no longer be `Copy`, and every
  command test compares it.
- **Guarding exit on `Effects::exit` in the dispatcher:** no command would have to know, but `run-command` would report success for an exit that
  did not happen, and M2-16's prompt belongs in the command anyway.
- **A guard in the dispatcher's state:** would make several `&self` methods `&mut self`, and the close button goes through the editor
  anyway.
- **Letting the close button confirm `C-x C-c`:** a stray click after a refused key would lose the text.
