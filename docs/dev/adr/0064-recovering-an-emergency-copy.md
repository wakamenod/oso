# 0064. Recovering an emergency copy

- Status: Proposed
- Date: 2026-11-07

## Context

[ADR 0013](0013-persistence.md) says oso offers to recover a copy when its file is opened, and
[ADR 0062](0062-emergency-save-and-the-panic-hook.md) fixes the format and what a reader must keep to: a copy is only
ever offered, nothing in the directory may choose a path to write to without the person, at most 64 KiB of a `.meta` is read, a first
line it does not know offers nothing, and the text is never parsed. [ADR 0059](0059-safe-save-and-the-exit-guard.md) leaves
`.oso-save-*.tmp` files to this step, and `open-questions.md` asks when a copy goes.

What makes this hard is the order of the roadmap:

- Files open only as `oso FILE`. There is no minibuffer or yes/no prompt until M2-16, no `M-x` until M2-17, no `C-x C-f` until M2-18 and no `C-x b` until M2-19.
- M2-7 ([ADR 0063](0063-modeline-echo-area-and-internal-buffers.md)) brings what can be used: the modeline's warnings, the echo area, `*Messages*`, and internal
  buffers, which are read-only, never modified, and left out of `Editor::buffers`. So the exit guard, `oso dev`'s checks and the emergency save skip them with no code of their own.
- The echo area empties at the next key, so a message alone can be missed.
- The UI thread must not wait on any of this. A home directory may be on a stuck network mount.
- The directory is private, but a backup, a sync tool or a mistake can put anything there. Its contents are untrusted input.

## Decision

### What the author chose

The author chose among the alternatives below on 2026-11-07:

- The offer: a notice, the `[emergency copy]` warning, and `*Emergency Copies*` on `C-h r`.
- Recovering: `r` looks, and a second `r` recovers.
- After recovery: a copy goes when a save of that buffer ends with the disk's confirmation, and undoing the recovery withdraws this.
- Declining: `d d` deletes at once.
- A copy nobody answers is never removed, and a notice comes at each start.
- Comparing: a line diff with `similar` 3.2.1, `default-features = false` with only `std`, on the tip crate, if `cargo tree`, `cargo deny check`, `cargo audit` and the rebuild check pass. The limits under "What a person can do" go with it.
- A copy of a file not open: the person runs `oso PATH`, and the row shows the path.
- `.oso-save-*.tmp` files: listed with look and delete, nothing removed unasked, and a look always withheld from the socket.
- A copy that would count as secret now is offered and withheld from the socket, a copy equal to its file goes unasked with a row in `*Messages*`, and a file that has gone comes back as `0600`, which the message says.

### The rule everything rests on

**A path in a `.meta` is shown and compared, and no file operation ever uses it.** It is not opened, not `stat`ed and not resolved. oso only touches:

- The emergency directory itself, once the checks of `PrivateDir` pass.
- A file the person opened, and the directory that file is in.

So a copy can reach a file only if the person named that file themselves, and it reaches only that file's buffer, and only if they answer. Writing it to the file takes a `C-x C-s` after that. A hostile `.meta` can say what
it likes and at worst shows a misleading row.

### How a copy is offered

Three things, none of which takes a key from the person:

- **A notice,** through `Editor::message`, so it also stays in `*Messages*`: `notes.txt has an emergency copy, from 12 minutes ago: C-h r`, or with no match, `2 emergency copies wait: C-h r`. It is said once per run, and again only for a copy not said before.
- **A modeline warning,** `[emergency copy]`, on a buffer whose file has a copy nobody has answered. It lasts until they answer, so that a missed notice costs nothing. It costs O(1), a flag on `Buffer`, as `[changed on disk]` does.
- **A list,** the internal buffer `*Emergency Copies*`, which `C-h r` shows at any time. All answers are given there, with single keys, so one place serves a file just opened, a file opened long ago, a copy of a buffer with no file, several copies of one file, and a copy of a file not open at all. Nothing else opens it: the window stays on the file, and typing goes to the file.

```text
3 emergency copies.  v look  = compare  r recover  d delete  g refresh  q back

  12 min ago  panic  1234 B  file unchanged  /home/me/notes/todo.txt
  2 days ago  exit    420 B  not open        /home/me/notes/plan.txt
  2 days ago  panic    12 B  no file         *scratch*
  3 days ago  ?        88 B  unknown         1a2b3c4d5e6f7a8b-1790000000-77-0.txt
```

- The newest come first, and each row is one copy. The cursor starts on the first row of the file the window was on.
- A row shows the whole path, never only the name, since the person is deciding from it. Names and paths pass through the escaping `*Messages*` uses (ADR 0063), so one row stays one row, and a path is cut at 1 KiB, with the cut shown.
- What a row says of the file:

  | Word | Meaning | Answers |
  |---|---|---|
  | `file unchanged` | The file's stamp is the one the metadata records. The copy is the file plus what was typed | look, compare, recover, delete |
  | `file changed` | The stamp differs, or a file appeared where the buffer had none | look, compare, recover, delete |
  | `file gone` | The metadata records a file, and none is there | look, recover, delete |
  | `no file` | The buffer had no file, and a buffer of that name is open | look, recover, delete |
  | `not open` | The metadata names a file no buffer visits | look, delete |
  | `leads elsewhere` | The buffer's path matches and its location does not | look, delete |
  | `recovered` | Its text is in the buffer and has not been saved | look, delete |
  | `damaged` | Its `.txt` is not the size `bytes` says, or is 2 GiB or more | look (under 2 GiB), delete |
  | `unknown` | No usable `.meta` | look, delete |
  | `left by a save` | A leftover temporary file (below) | look, delete |

### What a person can do

- **Look (`v`, `RET`):** the text fills an internal buffer named `*copy of notes.txt*` from a loading thread, with progress in the modeline. `q` goes back to the list, and `q` there goes back to where the window was, as `C-h e` does for `*Messages*`.
- **Compare (`=`):** a line diff of the file on disk against the copy, in an internal buffer `*compare notes.txt*`, built on a thread. A row with no file to compare says so. Either side over 64 MiB or over 1 million lines gives what is known instead: both sizes, and that they differ. The line limit is what bounds memory, as the diff keeps about 32 bytes per line of both sides, and running out of memory aborts with no emergency save. A diff that takes over 5 s has given up and reads as one replacement of nearly everything, so that too gives what is known instead, and says why. On the author's Mac that takes about 100,000 changed lines in a million, or 50,000 in 100,000, while 10,000 in a million take 1.7 s and 10,000 in 20,000 take 0.6 s. It uses `Algorithm::RawMyers`, whose splits halve the work, so that its recursion stays shallow: a stack overflow would also abort with no emergency save, and `Algorithm::Myers`'s heuristic splits promise no such thing. The thread catches a panic, and then says the compare failed.
- **Recover (`r`, then `r`):** the first `r` on a row looks at the copy and says `Press r again to put this text in notes.txt`. The second, in that view, puts it in the buffer. So nobody recovers a text they have not been shown, and no request has to outlive a command, as the second `r` needs the load to have finished.
  - It replaces the buffer's whole text as **one undo step**, and the window shows that buffer with the cursor at the start. The buffer is modified. **Nothing is written:** that takes `C-x C-s`.
  - It costs O(1): the rope and the display map were built off the UI thread for the view, and a clone of each is O(1). Undoing it costs what undoing a large kill does today, about 0.7 ms per MB (`open-questions.md` already holds that for M2-14).
  - It is refused, with the reason, while the buffer is loading, read-only, being saved or modified. Saving or undoing first is what is left to do, so that a person's typing is not swapped out by a key.
  - It is refused for a view with invalid UTF-8 or a NUL byte, and for a `damaged` or `unknown` row. An editable buffer's text cannot look like that, so such a copy has been changed since it was written.
  - With the row at `file changed`, the message says that saving replaces what is there now. The buffer's baseline is the file as just read, so the save goes through: the person was told, and compared.
  - **BOM and line ending** stay the buffer's, from the file just read. With no file there, they come from the metadata, where only `true` and `false`, and `lf` and `crlf`, count.
  - **A file that has gone** comes back with mode `0600`, through ADR 0059's rule for a deleted file: the buffer takes a baseline with a file of that mode. The metadata records no mode, and a private file must not come back as what the umask gives. A file meant to be shared needs a `chmod`, and the message says so.
- **Delete (`d`, then `d`):** declining deletes. The first `d` says what the second does, through `Invocation::repeats`, the same way as a repeated `C-x C-c`, so any other key cancels it. With several cursors, it takes each cursor's row.
- **No answer** is an answer too: the copy stays and is offered next time.
- **Refresh (`g`):** reads the directory again, for what another instance has written.

A copy that is `not open` needs `oso PATH` first, or later `C-x C-f`. The row gives the path, and matching does the rest.

### Matching a copy to a buffer

This runs on the scan's thread, on bytes, with no decoding. For each buffer that visits a file and has a baseline, `write::encoded` of its own path and location is compared with the metadata:

1. **The location is equal:** a match, whatever path led there. So a link and its target find each other.
2. **The metadata has no location, and the path is equal:** a match. That load could not tell where the file was, and this one can.
3. **The path is equal and the location differs:** no match, and the row says `leads elsewhere`. ADR 0059's "the text goes only where the file was" holds across a crash too.
4. **Neither is equal:** no match.

A buffer with no baseline matches nothing, as it could not be saved anyway. A copy with neither `path` nor `location` matches a no-file buffer of the same `name`, which today means `*scratch*`, and then only if that buffer is empty and unmodified.

**The hash in a stem is not used.** The list needs every `.meta` anyway, and a hash can only find a candidate, which this then compares. The stem must fit `<16 hex>-<digits>-<digits>-<digits>`, and any other name is left alone: not read, not listed, not deleted.

**What `file-bytes` and `file-modified` say** comes from comparing them with `FileStamp::same_content`, which is ADR 0059's rule for a change. They are untrusted, so they only choose a word on a row. No decision is made for the person from them. A `file-bytes` with no `file-modified` reads as changed.

**A copy equal to its file is removed, not offered.** That is what a save finishing at the moment of a crash leaves, which ADR 0062 foresaw. If `bytes`, plus the BOM if the file starts with one, equals the file's length, the thread compares the two as streams, 1 MiB at a time. Only a matched copy is compared, so only a file the person opened is read. Removing it loses nothing, since the file holds that very text. `*Messages*` says so, with the stem.

**Several copies of one file** are several rows. Each is answered alone, as an older one may hold text the newer one does not. The modeline warns while any is left.

**Spellings on file systems that ignore case or normalize names.** `loader::location` makes the directory canonical and keeps the last name as typed, so `oso NOTES.txt` and `oso notes.txt` get different locations for one file on default macOS and Windows volumes. **The location takes the file's own spelling:** if something is there, `location` also canonicalizes the file itself. On APFS, `fs::canonicalize` gave the stored bytes for another case and for both normalizations, in the name and in a directory on the way. This also serves M2-18, which must find the buffer already visiting a file, and the save's comparison is unaffected, as both sides come from `location`. A new file keeps its spelling, as nothing says otherwise. A copy that still goes unmatched is in the list as `not open`, where its path shows the spelling to open it by. Comparing files by identity would need a `stat` of a path from metadata, against the rule above.

### Untrusted contents

**The directory.** The same checks as `PrivateDir::open`, creating nothing, come first, through a new `PrivateDir::existing`: `Ok(None)` if it is missing, and an error for a symlink, another owner, a looser mode, an access list ([ADR 0065](0065-access-lists-on-private-directories.md)), or anything on the way another user can change. **A directory that is refused is not read at all,** and the message names it. Its contents could be anyone's. A read-only one is fine to read, which is why this is not `PrivateDir::check`, and deleting there simply fails with a message.

**The scan is bounded.** `read_dir` stops at 100,000 names. The newest 1000 stems, by the seconds in the name, get their `.meta` read, and a last row says how many more there are.

**Each file** must be a regular file by `symlink_metadata`, before anything opens it, as opening a pipe blocks. It must be this user's, with one hard link, as a hard link is a name for some other file's text. Once open, the handle must show the same device and inode, with `loader`'s `same_file`, so that a swap in between is refused. A `.meta` that fails this makes the stem `unknown`, and a text that fails it makes the stem `damaged`. Neither is read.

**A `.meta`:**

- At most 64 KiB is read. More than that is `unknown`.
- The first line must be exactly `oso-emergency-copy 1`, or the copy is `unknown`, and nothing but a look and a delete is offered.
- Each other line is `key value`, in printable ASCII. A key not known is ignored, as ADR 0062 says. A known key twice, a number that does not parse, or a bad `%XX` makes the copy `unknown`. `bytes` is required.
- `why` and `written` only reach a row. A `why` not known shows as `?`, and a `written` in the future shows as it is.
- The parser takes bytes and returns a value. A test feeds it anything, and checks that what `meta_text` writes parses back.

**A `.txt`:**

- A look reads no more than its size at the check, and never 2 GiB or more, where ADR 0051 wants to ask and M2-18 brings the asking. Running out of memory aborts with no emergency save, so this is the one case that says to read the file some other way, and gives its path.
- `damaged` is decided from the size, before reading. So a `.txt` far bigger than its `bytes` costs nothing until someone looks, and then at most the limit.
- The text goes through the same `Builder` as any file. It is displayed by the same rules, and nothing in it is run or interpreted.

**A `.txt` with no `.meta`** is a whole text from a crash between the two renames, or a copy being written by another instance at this moment. So it is `unknown` only when its modification time is over an hour old, and hidden until then.

### When copies and temporary files go

Nothing goes without either the person's answer or proof that nothing is lost.

- **After recovery:** when a save of that buffer ends with the disk's confirmation, so that the text is safe first. A kill between the recovery and the save must not lose it, so the copy stays until then. A save the disk did not confirm (`Unsynced`) keeps it. Exiting without saving keeps it, and it is offered again.
- **Undoing a recovery withdraws it.** Each undo and redo compares the content with the states recorded on either side of the recovery, so the row goes back and a later save does not remove a copy the person turned down. A save that ends with the buffer modified again keeps the copy too, until one that holds what is in the buffer.
- **Once declined:** at once, after the second `d`. The `.meta` goes first, so that a failure halfway leaves an `unknown` text and never an offer with nothing behind it.
- **After some time: never, for a copy nobody has answered.** ADR 0062's promise is that a panic loses no text, and the person may not open that file for months. The notice comes at each start, so that they stay visible. Backups may hold them, as ADR 0062 says.
- **An equal copy:** as soon as it is found to be equal.
- **Two instances:** one deleting what another still lists is a message in the second, `That copy is gone`. Deleting what is already gone is not an error.

**`.oso-save-*.tmp` in the emergency directory** is what a kill left of a copy being written, and can be the only trace of that text. Once its modification time is over an hour old, so that no running writer is touched, it is a row `left by a save`: look and delete, never recover. The hook waits at most 5 minutes in a sync, so an hour leaves room. The name does not help: its number is the sub-second part of the time, not an age.

**`.oso-save-*.tmp` beside a file** is the same, with one more thing to fear: it may hold a secret buffer's text, and its name does not say whose it was.

- When a file finishes loading, a thread reads that file's directory, stopping at 100,000 names, for names of exactly that form that are regular, this user's and over an hour old. Only the directory of a file the person opened is read.
- Each is a row, in the same list, with its full path. **Nothing removes one unasked:** it may be the newest text there is, after a kill during a save.
- **A look at one always counts as secret** (`Verdict::Unresolved` at the least), so that the socket gets no text or pixels, whatever its name says.
- `d d` removes it as a copy goes. Its text cannot be recovered, as nothing says whose it is.

### Secrets

- **A copy that would count as secret now,** because its path has moved, a link has changed, or patterns have grown, **is offered like any other.** It is the person's text, and the copy exists either way. Deleting it unasked could lose the only copy. Refusing a look would leave a person unable to decide.
- **A view or a compare buffer takes the strictest verdict there is:** the matcher over the metadata's decoded `path` and `location`, which is pure and touches no file, plus the verdict of the buffer it matched. A row with neither path nor location, or `unknown`, is `Unresolved`, which is withheld. This needs `Buffer::judge_copy`, as these buffers have no path of their own, and both of today's ways of judging call a buffer with no path `Ordinary`. Paths are decoded lossily for this, which keeps every name a pattern can match and needs no `unsafe`. So `get-text` and `get-state` give no text, `screenshot` refuses, and the messages around it stay out of the log and `*Messages*`, all by what ADR 0044 and ADR 0063 already do.
- **Recovery keeps the target's verdict.** A secret buffer that recovers text gets no new copy at the next panic, since its entry holds no text (ADR 0062).
- **Messages, the log and rows** hold stems, names, paths, sizes and ages, as any save message does, and never text. Line numbers of a difference appear only in a compare buffer, which is withheld as above. No message says where two texts differ.
- **The list itself is not withheld.** It holds what `get-state` already says of any buffer: names, paths and sizes.
- **No new control method.** `*Emergency Copies*` and its views are buffers, and reach `get-text`, `show-buffer` and `send-keys` as `*Messages*` does. A window on any of them hands over as a window on `*Messages*` does (ADR 0063), so what `place_behind_messages` does for one buffer covers them all, and an import refuses in the same way.

### Never blocking the UI thread

- **At startup,** the thread that already runs `PrivateDir::check` also scans, so the UI thread gains no spawn, and the first frame does not wait. A stuck home directory leaves that thread stuck and no one waiting.
- **When a load finishes,** one thread matches, compares for equality and looks beside the file. That costs one spawn per file opened, 8 to 14 µs by ADR 0059's figure for a save. Nothing is started for a key.
- **A scan hands over** finished rows: facts, and the list's text as a `LoadSnapshot`, so that showing it is O(1) through `apply_load`. An answer starts a new scan rather than editing rows.
- **The key path gains** a look at the selected buffer's mode to choose the `major` layer, and the modeline one flag. `just perf` checks that, by the usual rule.
- **Views** load on a loader thread, and are dropped on one when left, as `open-questions.md` already asks of a killed buffer. Compares and deletions run on threads, and report through `UserEvent::Copies`.

### Commands and keys

`commands/recovery.rs` and `bindings/recovery.rs`, by the `add-command` skill. Commands see only `ctx.editor`, so they queue a `CopyAction`, as `save-buffer` queues a job, and `App::apply` takes them after the command. `Effects` stays for plain flags (ADR 0059).

| Key | Where | Command | Docstring's first line |
|---|---|---|---|
| `C-h r` | global | `recover-session` | Show the emergency copies waiting for an answer, or go back from them. |
| `v`, `RET` | list | `emergency-copy-view` | Show the text of the copy on this row, read-only. |
| `=` | list, view | `emergency-copy-compare` | Show how the copy on this row differs from its file. |
| `r` | list, view | `emergency-copy-recover` | Put the copy's text in its file's buffer, once it has been shown. |
| `d` | list | `emergency-copy-delete` | Delete the copy on this row, when repeated. |
| `g` | list | `emergency-copies-refresh` | Read the directory of emergency copies again. |
| `q` | list, view, compare | `emergency-copies-back` | Go from a copy to the list, or from the list to where the window was. |

- All take `ArgSpec::None`. `recover-session` takes Emacs's name. The others have no Emacs command, as Emacs recovers through dired and a prompt.
- **`Layers::major` is used for the first time.** `Buffer` gains a small `Mode`, `Fundamental` or `EmergencyCopies`, and the dispatcher picks the keymap by it. `bindings::recovery_keymap` builds it, as `prefix_keymaps` builds its own. Letters not bound there reach the global text command, which refuses with `Buffer is read-only`.
- `C-n`, `C-p` and the rest of motion work as in any buffer. A command reads rows from the selection set: `d` takes every cursor's row, and the others take the primary cursor's, as they change what the window shows.
- **M2-16 and later** do not replace this. `y-or-n` may come to stand for `d d`, `M-x` reaches the same commands, M2-18 may fill in a `not open` row's path for `C-x C-f`, and `C-x b` removes the need for `q`. A key's meaning does not change.
- **M2-12 must count this mode** with the minibuffer, isearch and menus when it turns the IME off (`ui.md`), since with it on, `r` starts a composition.

### Where it lives

- **No new crate, crate edge, public trait or public module, and no change to `guardrails/layers.txt`.** ADR 0038 asks for nothing beyond this record.
- **The tip crate's `emergency` module grows,** since reader and writer must agree on a format that only they know:
  - `emergency/copies.rs`: the scan, which shares `write::encoded`, and `emergency/meta.rs`: the parser, which shares the header.
  - `emergency/offer.rs`: rows, matching and what each answer may do, with no window, tested as `saver::Leaving` is.
  - `emergency/compare.rs`: equality and the line diff.
  - None of this is reachable from `hook` or `publish`, which stay as they are. `emergency` already sits above `saver` and `loader`, and needs nothing else. The paragraph at the top of `emergency.rs` and `crates/oso/AGENTS.md` get updated: text from a copy may be held here, on its way to a buffer, and still never in a message or a log.
- **`app/copies.rs`** is a new concern beside `loads` and `saves`. It holds what `App` adds, `emergency::Offer`, and takes in `UserEvent::Copies`.
- **`loader`** gains `spawn_copy`, the same thread with a byte limit and with no resolving or baseline, and the change to `location`. `same_file` becomes visible to the crate.
- **`oso-core`,** in a private `editor/copies.rs` and `buffer/recovery.rs`, with new public API and no new public module. `editor` gains `CopyAction`, `row_text`, `copy_buffer_name` and, on `Editor`, `queue_copy_action`, `take_copy_actions`, `add_internal_buffer`, `remove_internal_buffer`, `toggle_copies`, `copies_back`, `leave_copies`, `has_copies_list`, `set_copies_text`, `set_copies_cursor`, `cursor_line` and `replace_text_from`. `place_behind_messages` becomes `place_behind_internal`, which goes through both ways back. `buffer` gains `Mode` and, on `Buffer`, `mode`, `begin_loading`, `set_copy_waits`, `judge_copy`, `set_file_format`, `expect_a_file_that_has_gone`, `recovery_stands` and `forget_recovery`. `Buffer::internal` stays crate-private. `ReadOnly::Internal` keeps its meaning.
- **`oso-platform`** gains `PrivateDir::existing` and `privacy::is_this_users`, in the API snapshot. `is_this_users` is what a file beside another file is checked by, where no private directory's owner can say.
- **A dependency:** `similar` 3.2, with `default-features = false` and only `std`, on the tip crate, approved by the author on the condition above. It has no dependencies of its own that way, no `unsafe` and no build script. `insta` keeps 2.7.0 in tests, so `Cargo.lock` holds two versions, and only 3.2 reaches a release build. 2.7.0 alone would add nothing to `Cargo.lock`, but its compaction pass is quadratic in the number of edits and runs past any deadline, which 3.2.1 fixed.

### The gates this touches

- **File writing ([`REVIEW.md`](../../../REVIEW.md)):** this is the first code that removes files, and `loader::location` is what a save compares. Removal takes only an exact name that a scan found to be this user's and regular, after an answer or a proof of equality.
- **The control socket:** no new method, but `export-state` and `import-state` learn about these buffers.

## Tests

**Units and properties,** at the lowest layer that can say it:

- The parser: pseudo-random bytes and lines, as the tip crate has no `proptest`, with no panic and no value from a bad line; `meta_text` round trips; a 64 KiB limit; a first line of another version; a key not known; a key twice.
- Matching: a table over the four cases, a link and its target, a no-file buffer, `*scratch*` with text in it, and a location that differs under one path.
- What a stamp says: unchanged, changed, gone, appeared, and no `file-modified`.
- Each file kind in a scan: a symlink, a pipe, a directory, a hard link, another owner where a test can arrange it, a wrong stem, 100,000 names and a `.txt` bigger than `bytes`.
- Removal: `.meta` first; a stem only as the scan found it; gone already; a save that was not confirmed keeps the copy; undo and redo of a recovery.
- `Editor::recover_text`: one undo step, undo and redo, modified, cursors in every window, refused when modified or loading, and O(1) by checking that the rope is shared.
- The compare's limits: 64 MiB, 1 million lines, the deadline with a pair that has no lines in common, and a panic on its thread.
- `loader::location`: another case on the running system's file system, where it ignores case, and another normalization on macOS, `#[cfg]`-gated.
- Commands through the registry, and keys through a `Dispatcher`: `r r`, `d d` with a key between, and `q` back through all levels.
- Text-grid snapshots: the list with each kind of row, a hostile name, a modeline with the warning, and a frame too small for a modeline.

**End-to-end flows** (ADR 0049), each with a real copy made by the flows #40 added unless it says otherwise:

- **A real panic, then recovery:** type, `dev-panic`, start again on the file. The echo and the modeline offer it; `C-h r r r` puts the text back; the file is untouched until `C-x C-s`; the copy goes after the save; the log holds no typed text.
- **An end nobody agreed to,** with `dev-end-unasked`: a copy `why exit` is offered, and a confirmed exit without saving keeps it for next time.
- **Decline:** `d` alone keeps it, `d d` removes both files.
- **A file changed since,** and **a file gone since:** the rows say so, recovery still works, the save replaces in the first and, in the second, recreates with mode `0600`.
- **`*scratch*`:** copied, listed as `no file`, recovered into an empty `*scratch*`.
- **Secret:** a hand-made copy naming a `.env` path. Look at it: `get-text` errors, `screenshot` refuses, `get-state` has no text, and the log holds no fake secret.
- **A hostile directory,** hand-made, with old modification times set: a `.meta` naming another file with a `.txt` far over its `bytes`, one of over 64 KiB, a `.txt` with no `.meta`, a symlink and a pipe. The instance starts as fast as ever and draws. No file outside the directory changes, and the file the `.meta` names is not read.
- **A refused directory (`0755`):** nothing is read, and it says so.
- **An equal copy,** hand-made: gone after opening the file, and nothing offered.
- **Leftovers,** hand-made, beside the file and in the directory: an old one listed and deletable, a young one left alone.

`just e2e` runs these, and the `verify-in-app` skill checks the screen, as this adds what a person sees. `read_copies`' assertion of two files each stays true for all but the hostile flow, which uses its own.

## Consequences

- The author can keep real notes with a way back, before M2-16, through one key to remember, which the notice gives.
- A person who runs `oso` right after a crash gets a notice, and can lose nothing by ignoring it. A person who deletes one has to mean it.
- A copy of a file not open takes a restart with `oso PATH` until M2-18. This is the price of never using a path from metadata.
- Copies nobody answers stay on disk until they do, and a person is reminded at each start. That text may reach system backups for as long as it stays.
- `*Emergency Copies*` brings the first buffer keymap, single-letter keys and a second internal buffer family. M2-16's split of internal buffers (ADR 0063) must keep it read-only.
- `C-h e`'s way back, `Place`, now has a second user, until M2-19 removes both.
- A recovered file that had gone comes back as `0600`, even if it was once shared.
- Undoing a recovery of a large copy takes as long as undoing a kill of that size on the UI thread, which `open-questions.md` already holds.
- A file with a copy shows no different content until someone asks, so a person who misses the warning and edits has two texts. The copy stays, and saving does not remove it unless it was recovered.
- A `.oso-save-*.tmp` beside a file now causes a row until someone answers, in every instance opening a file there. A directory shared with a stopped save may show one for good, until `d d`.
- Reading a directory costs up to 100,000 names, once per file opened, on a thread.
- Docs to update with the code: `ui.md`, `security.md`, `architecture.md`, `roadmap.md`, `open-questions.md` (the two "By M2-11" items go), `crates/oso/AGENTS.md`, the ADR table, and the public API snapshots.

## Alternatives considered

**How to offer:**

- **Prefix keys only, `C-c r r`, `C-c r =`, `C-c r d`:** about a third less code and no list, but only the newest copy of the file in view is reachable. A copy of `*scratch*`, or of a file not open, is reachable only outside oso, and several copies take turns unseen.
- **A modal question in the echo row, as `y-or-n-p`, through `Layers::minor`:** cannot be missed, but it arrives when the scan finishes, while the person may already be typing, so a letter meant for the file answers it. It also leaves the same cases out, and needs its own key handling that M2-16 replaces.
- **Opening the list by itself when the file has a copy:** cannot be missed, for a key, but the window shows something other than the file asked for, on a run where the copy may be old and known. The modeline warning covers missing it.
- **Waiting for M2-16's prompt:** less UI to design twice, but M2-11 then blocks real notes (M2-12) on floating windows.
- **A command line, `oso --recover`:** works with no UI, but reading text is what a person needs to decide, and that is the GUI.

**How to answer:**

- **`r` once:** one key fewer, but a person can recover a text they have not seen, and a load must finish behind a request that outlives its command.
- **`d` once:** one key fewer, for what cannot be undone.
- **Recovering into a modified buffer, with undo as the way back:** fewer refusals, but one key swaps out what was just typed.
- **Recovering as a new buffer with no file:** nothing can be aimed wrongly, but with no `C-x C-w` or `C-x b` it cannot be saved or left today.
- **`o` to open a row's file from the list:** saves a restart, but metadata then chooses what oso opens, even with a key. Decoding it to a path also needs care on Windows. M2-18 can do this through a prompt the person reads.

**Compare:**

- **A diff of our own (prefix and suffix trimmed, then a bounded Myers),** about 150 lines with property tests: no dependency, but one more algorithm to own, and M4's git work will want one anyway. This is the fallback if the dependency is refused.
- **No diff:** sizes and a look at each text. Nothing to build, and enough while the file is unchanged, but it gives no help in the case where a person most needs it, a file changed since.
- **Comparing with the buffer rather than the file:** the buffer equals the file unless edited, and recovering already refuses a modified buffer.

**Going:**

- **Deleting on recovery:** the simplest, but a kill before the save loses the text.
- **Keeping a declined copy for 7 days, hidden:** a way back from a mistake, but the person said no, and this is text on disk that they believe is gone.
- **Removing unanswered copies after 30 or 90 days:** less text in backups and no pile-up, but a person returning to a file after that loses the text with no word.
- **Deleting leftovers beside a file by age:** tidy, but it may be the newest text there is, and a directory scan would delete from wherever a file was opened.
- **Ignoring leftovers beside a file:** no directory read, but a secret's text can sit in a project and reach a commit.
- **Only saying a count of leftovers:** less UI, but a person then has to find them by hand.

**Matching:**

- **Matching by hash alone:** ADR 0062 forbids trusting it.
- **Comparing the file's identity to find other spellings:** catches them all, but needs a `stat` of a path from metadata, and says nothing once the file is gone.
- **Folding case and normalization when comparing:** needs Unicode tables and a guess at what the file system does, and two real files on a case-sensitive volume would match.

**Elsewhere:**

- **A new top-level `recovery` module:** a clearer name, but needs a layer change, and splits a format that only two files should know.
- **The state in `oso-core`:** commands could read it without a queue, but parsing and files stay out of that crate, so it would hold a copy of what the tip crate knows.
- **A hash check of the text, in the `.meta`:** would catch a changed copy, but a format change and a pass over the text, for a directory that only this user writes.
- **Adding `file-mode` to the `.meta`:** a file that has gone comes back as it was, within version 1, but the value is untrusted, so it could only narrow, and `0600` already does.
