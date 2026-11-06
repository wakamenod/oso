# 0062. The emergency save, the panic hook and no core dumps

- Status: Proposed
- Date: 2026-11-06

## Context

[ADR 0013](0013-persistence.md) writes unsaved buffers on a panic, [ADR 0014](0014-panic-strategy.md) says a panic
outside the command boundary triggers that before exiting, and [ADR 0028](0028-secrets-handling.md) says core dumps are
off and panic messages hold no text. [ADR 0051](0051-m2-plan-decisions.md) settled that the hook prints the location and
the stack and never the message, that an OS-initiated exit writes copies too, and that an unresolved path is told
apart from a secret name. [ADR 0059](0059-safe-save-and-the-exit-guard.md) left open how this shares the temporary file.
What remains is how it can work at all:

- A panic hook runs in whatever state the panic left, on a thread that may hold anything. It cannot read the editor.
- A panic inside a hook aborts at once, whatever tries to catch it, so a hook cannot protect itself from its own bug.
- A `Drop` that panics while unwinding aborts before any `catch_unwind` runs, so the copies must come before unwinding.
- A terminal nobody reads blocks a write to stderr for good ([ADR 0055](0055-oso-dev-never-loses-unsaved-text.md)).
- `clippy.toml` and the guard tests ban global state, so the hook must own what it reads.
- Typing has a budget of 8 ms, and this must not move it.

## Decision

### The hook

- `std::panic::set_hook` with a closure holding an `Arc` of the shared state: what was last published, the UI thread's
  `ThreadId`, an `in_command` flag and what has been agreed to (below). It is installed once, in `main`, before the event
  loop. `oso ctl` and `oso dev` keep the default hook, as neither holds a buffer: `oso ctl` passes on only what it prints anyway.
- It decides by where the panic happened:
  - **On the UI thread, outside a command:** writes the copies, then prints the location, the stack and where the copies
    are.
  - **Under `Registry::execute`:** prints the location only. The panic is caught at the boundary and the editor runs on,
    so nothing is saved.
  - **On another thread:** prints the location and the stack. The loader and the saver catch their own.
- **It never sees the message.** The function that does the work is passed only the location.
- **Copies are written on a thread of their own, and each buffer under its own `catch_unwind`.** That is the only way a
  panic in the writer can cost one buffer and not all of them, as a panic inside the hook itself would abort. If no thread can
  start, it writes on the panicking thread, where a panic does cost the rest. The hook waits as long as the writer gets
  somewhere: it ticks every megabyte, and after 30 s without one the hook goes on. A sync says nothing until it is done, however
  far it has got, so it has an allowance of its own, 5 minutes, which covers 2 GiB at 10 MB/s. So the biggest buffer on a slow disk keeps its
  copy, and a stuck network home directory still cannot hang a process that is ending for good. What is given up on leaves its temporary file.
- **The stack is captured after the copies too,** as a capture walks the stack and takes a lock of `std`'s, either of which must not stand before them.
- **Printing comes after, also on a thread of its own, and is waited for at most 5 s.** A caught panic does not wait at all, as
  the editor runs on and must not wait for a terminal. A failed write is ignored. If no thread can start, nothing is printed, as printing here could
  hang. The stack is captured on the panicking thread and resolved on the printing one. A `stty tostop` stops the whole process,
  which no thread can help, and the copies are already on disk by then.
- **A second panic under one command** comes from a `Drop` during the unwinding of the first. The process then aborts before the
  boundary can catch anything, so the hook writes the copies, as it would outside a command.
- `try_lock` on the state. Only the UI thread ever writes it, and only moves happen under the lock, so a panic
  while building what is published leaves the lock free and the last published state whole. If it were ever held, the hook says
  so and writes nothing rather than wait on its own thread. A poisoned lock is used as it is.

### What is published

- `Publisher`, on the UI thread, keeps for each buffer whose text may be only here an entry: an O(1) rope snapshot, its
  `ContentId`, and an `Arc` of what does not change with the text: name, path, location, BOM and line ending. So a key
  clones no path. A buffer counts if it is modified **or a save of it is running**. A save may or may not reach the disk, and
  an undo can make that buffer look unmodified while its file is about to differ.
- **After each command that did not panic**, from `Dispatcher::execute`, and after a save reports or a load's event. Never on
  the frame path: `poll_loads` runs before a frame and publishes nothing, as a buffer still loading is not at risk.
- The check walks the buffers and compares `ContentId`, modified, saving and the verdict. That is the small per-key
  work that grows with the number of buffers. A buffer that did not change takes no lock, and one that did finds its entry by a binary
  search.
- **A panicking command's state is held back until a command ends normally.** That holds for every way of publishing: a save's report and a load take in none of it,
  and only the next command that ends does, as the person has then seen that text and gone on from it. **The one exception is a save that command started:** that text was
  asked to reach the disk and is on its way, so the job's own text is published, and the buffer counts as being saved. A save already running lets nothing through. The facts
  stay true throughout: whether a save runs, whether the text may be copied at all, and a buffer that now holds what its file does is no longer at risk. A buffer with nothing
  published yet gets nothing. `Editor::queued_saves` is new public API for this.
- **A secret buffer's entry holds no text.** Only the fact of it is shared, so that the hook can say one was left out.
- Neither the hook nor the exit reads the editor. An exit writes what was last published, for the same reason.

### The verdict

- `Buffer::is_secret`'s flag becomes `oso_core::buffer::Verdict`: `Unjudged`, `Ordinary`, `Unresolved` and `SecretName`.
  Each use asks its own question: `is_withheld` for the control socket ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)), where
  only `Ordinary` passes, and `may_be_copied` for the copies, where `Ordinary` and `Unresolved` do. `Unjudged` gets no copy, though a
  buffer still unjudged is loading and cannot be modified. M2-14 adds a variant for a tainting yank, and `judge_secret` and `judge_unresolved` must then
  not clear it. It also decides whether a kill from an `Unresolved` buffer taints, which `is_secret` would answer by accident.
- **The names a failed walk had already passed count,** through `Buffer::judge_unresolved`. ADR 0051 says "no name on the path":
  a link to nothing that goes through `~/.aws` has such a name, though the path as opened does not.

### The exit

- winit's `ApplicationHandler::exiting` runs on every exit, so what tells them apart is what the app has agreed to,
  `saver::Agreement`. `Leaving` keeps it in a cell the panic hook reads too, on every change, so no path through the app can leave it stale:
  - **`Nothing`:** nobody asked. Everything at risk is written. On macOS `applicationWillTerminate` leads here for a Dock
    quit or a logout.
  - **`ModifiedButNotSaves`:** an exit was confirmed and is waiting for saves. Only the buffers being saved are written, as
    nobody agreed to lose that text, and an end now may stop the write.
  - **`Everything`:** the app has called `event_loop.exit()`, through one method. `C-x C-c` confirmed, the close button confirmed and
    the socket's `exit` all end here, and write nothing. That includes a save that outlived an exit by 10 minutes, as the person was then told that the buffer may not
    be on disk, and that exiting again will not wait for it.
- It writes on the UI thread's call, waiting as a hook does, as the process is going.
- **A save running at that moment:** its thread may finish or may die with the process, and nobody can tell which. Its buffer gets a copy
  of the state the editor holds. At worst the copy equals the file, which recovery can see. The save's own temporary file may stay
  behind, as ADR 0059 says of a kill.
- A state already written is not written again, so an exit after a panic, or a second panic, adds nothing. A later state gets a copy
  of its own.

### The directory

- `$XDG_STATE_HOME/oso/emergency` or `~/.local/state/oso/emergency`, `~/Library/Application Support/oso/emergency` on macOS,
  `%LOCALAPPDATA%\oso\emergency` on Windows. Only absolute values count. It is worked out once at startup from the environment,
  with no I/O, and created only when a copy is about to be written.
- `oso_platform::privacy::state_home` says where per-user state goes, and `PrivateDir` makes the directory: mode `0700` for it and what it creates on the way,
  refusing a symlink, another owner or a looser mode, or anything on the way another user can change, which is checked all the way to the root. A home directory
  that its group can write therefore gets no copies, and the report says why. A looser mode is refused and not repaired, as something may already have read what is
  there. The walk goes one name at a time as the system would, through every link and directory it passes, whether on the written path or not, and each link's owner counts too.
  A link to nothing on the way is refused, and so is a path with `..`. It runs before anything is created, so a refused place gets nothing new, and again after, for what it judged only then. `control`
  uses the same checks of one directory, and has its own walk, as a dev tool's. Windows has no modes, and relies on the per-user place, whose access list the directory inherits.
- If it cannot be made safe, nothing is written, and the report says so and names it.
- **This is also found out at startup,** by `PrivateDir::check` on a thread of its own, which creates nothing, and said with `Editor::message` and in the log at the `error` level, which reaches
  a terminal with no `RUST_LOG`, as nothing shows messages until M2-7, and finding out at the crash would lose all the text. A
  group-writable directory on the way is refused even if that group holds only this user, as with user-private groups and `umask 002`. Knowing that it does means reading the group and user
  databases, through more `unsafe` and whatever name service they use, and being wrong costs the copies' safety. So the person is told at startup, and can `chmod g-w` it.
- In dev builds `--emergency-dir DIR`, as the first argument, puts it elsewhere. The end-to-end harness always passes it, so that a
  test never touches real copies.

### The format

- A copy is two files with one stem, each written to a temporary name with `create_new` and mode `0600`, synced and renamed:
  - **`<stem>.txt`:** the buffer's text exactly, with no BOM, no final newline added and no conversion. Anyone can open it.
  - **`<stem>.meta`:** renamed in last, so that its presence means the copy is whole.
- **The stem is `<hash>-<seconds>-<process>-<attempt>`,** the first with nothing under it. The hash is 64-bit FNV-1a of the real place the
  path led, or the path if that was not told, or the buffer's name if it has no file. FNV-1a because `DefaultHasher` may change
  with the toolchain and M2-11 must find these later. No text from a path is in a name, where it could be too long for the
  system.
- **The metadata** is ASCII, one `key value` per line, after a line naming the format and its version:

  ```text
  oso-emergency-copy 1
  name notes.txt
  path /home/me/link%20to/notes.txt
  location /home/me/real/notes.txt
  bom false
  line-ending lf
  file-bytes 1200
  file-modified 1789999000.123456789
  bytes 1234
  written 1790000000
  why panic
  ```

  A line is left out when a buffer has nothing to say there: no `path` or `location` without a file, and no `file-*` if nothing was there. Names and paths are
  percent-encoded byte by byte, so any name fits on a line and no terminal sees its control characters. `file-bytes` and `file-modified` are the file as the
  buffer last read or wrote it, so that recovery can tell by the rule of [ADR 0059](0059-safe-save-and-the-exit-guard.md) whether it has changed since. `why` is
  `panic` or `exit`, and `written` is in seconds since the Unix epoch.
- **What M2-11 must keep to:** a copy is only ever offered, and nothing in this directory may choose a path to write to without the person.
  The hash in a name is only a hint: it tries the hash of the file's location and of its path, and compares its own encoded path or location with the metadata, so it need
  not trust or even decode what is there. It reads at most 64 KiB of a `.meta`, offers nothing for a first line it does not know, and ignores keys it does not know within
  a version. The text is never parsed. A `.txt` with no `.meta` is unknown, and a `.oso-save-*.tmp` in this directory is what a kill left of a copy being written. On a file system
  that ignores case or normalizes names, two spellings of one file have different hashes, which is M2-11's to settle.
- This writes nothing else, and no text in any message or log. File names of copies are fine to print. The hook prints the directory
  and the count, and how many were left out. Echo-area messages are M2-7 and recovery is M2-11.

### Core dumps

- `oso_platform::privacy::disable_core_dumps` sets `RLIMIT_CORE` to 0, soft and hard, at startup on Unix, before any text can be here. A failure
  is logged. Windows has no such limit for a process to set, so it does nothing there. This also covers `oso dev`'s instances, which start the same way.
- **`libc` moves from a dev-only dependency to a release dependency of `oso-platform`, on Unix.** It is already in the tree through
  winit and wgpu, so `Cargo.lock` gains nothing. This adds one `unsafe` block, for `setrlimit`, and `geteuid`'s, moved from
  `control`, now reaches release builds too.

### Where it lives

- `emergency` is a new top-level module of the tip crate, in a layer of its own above `saver` in `guardrails/layers.txt`, below `bindings`
  and `dispatch`, which marks the command boundary. It shares `saver::Temporary` and its syncing, and so `.oso-save-` names here too. This answers
  [ADR 0059](0059-safe-save-and-the-exit-guard.md) by the first of its two ways, a layer. **`saver`'s `cfg(unix)` file code stays in the tip crate:** it is all `std`, and only the
  directory's checks need `libc`, which went to `privacy`. Copying ACLs or extended attributes would be the next thing to need `libc` there, and would call for the file-system module
  in `oso-platform` then. Saving and copies are still the only code that writes files.
- `oso_platform::privacy` is a new public module, below `control` in the layers: `disable_core_dumps`, `state_home` and `PrivateDir`.
- `oso_core::buffer::Verdict`, `Buffer::verdict` and `Buffer::judge_unresolved` are new public API. `secret` stays a private module.
- The dev tools are the control methods `dev-panic` and `dev-end-unasked` and the command `dev-panic`, behind the `dev` feature. `dev-panic`
  puts the selected buffer's text in its panic message, which is what shows that the message goes nowhere.

### Where this extends earlier decisions

- [ADR 0013](0013-persistence.md): copies on an OS-initiated exit too (from ADR 0051), and the format and directory here.
- [ADR 0014](0014-panic-strategy.md): "before exiting" means before unwinding.
- [ADR 0028](0028-secrets-handling.md): the location, the stack and never the message, also for a caught panic, where the default hook
  printed the message ([ADR 0034](0034-command-registry-and-keymaps.md) noted that).
- [ADR 0044](0044-control-socket-trust-and-secret-buffers.md): the flag on `Buffer` becomes a verdict, and the emergency save asks `may_be_copied` where it said `is_secret`.

## Consequences

- A panic no longer loses text, except in a buffer with a secret name. Nor does an exit the OS asks of the app on macOS, the Tier 1 system, where a Dock quit
  or a logout reaches `exiting`. A kill signal, power loss, a force quit from the Dock and running out of memory, which aborts with no hook, still lose it. On Tier 2 systems a logout
  may not reach `exiting`: winit on Windows does not handle the end of a session, and on Linux one usually comes as a signal, or as a close request, which the exit guard refuses.
- Text reaches the disk outside a save in those two moments, in a directory only the user can read. Copies stay there until M2-11 deals with them,
  or the user removes them, and they may be picked up by system backups, as any file in the home directory is.
- Each key that changes a buffer costs one rope clone and an uncontended lock, and each command a walk over the buffers. The pull request has the measurements.
- A panic's message is gone from all output, also where it would help. The location and the stack have to do, and `RUST_BACKTRACE` no longer matters for a panic.
- A file whose name matches a secret pattern can still lose its work, as decided. The hook says how many it left out.
- The stack holds source paths and function names, which is what ADR 0051 asks for.
- A second thread stuck printing keeps the stderr lock, so that later log lines block as they always have under a stopped terminal (ADR 0055).
- `C-x C-w` (M2-18) changes a buffer's path with no text change, and must make the entry's identity new. M2-14 must keep a taint through both ways of judging.
- `.txt` and `.meta` as two files means a crash between the renames leaves a whole text with no metadata. It is still there for a person, and M2-11 can treat it as unknown.

## Alternatives considered

- **`catch_unwind` around the buffers inside the hook:** cannot work, as a panic in a hook aborts at once.
- **Writing after unwinding, with `catch_unwind` at the top of `main`:** a `Drop` that panics during unwinding aborts first, and winit's own catch on macOS sits between.
- **Reading the editor from the hook:** it may be in the middle of anything, and borrowing it needs a global. Publishing is what lets the hook own its state.
- **Publishing a panicking command's state at the next publish of any kind:** a save's report would then take in text the person has not seen, depending on timing.
- **Holding a whole panicking command's state back, saves included:** a save it started would have no copy if the OS then ended the app, though that text was on its way to the disk.
- **Counting a group as safe when its number equals the user's:** the usual shape of a user-private group, but only a convention, and it would pass a real group of that number.
- **`ArcSwap`, or two slots flipped by an atomic, so that a panic with the lock held still leaves a state to read:** a new dependency or more code, for a case that only moves can reach,
  and they cannot panic.
- **Marking changed buffers in `Editor`, so that a key costs O(1) with any number of buffers:** `buffer_mut` hands out buffers, so anything could change one without marking it, and a missed mark
  loses text without a sign. The walk costs nanoseconds per buffer.
- **A header in the same file:** one rename instead of two, but the text would not be the text, and M2-11 would parse a file that may be huge and that it cannot trust to tell it where the header ends.
- **Keeping the BOM in the copy:** the bytes would match a saved file, but the metadata says it, and the copy opens as the person's text.
- **`$XDG_STATE_HOME` on macOS too:** easier to test, but not where a Mac app keeps state. `--emergency-dir` serves the tests.
- **Reusing `ControlDir`:** it is behind the `dev` feature, and holds what a socket needs.
- **A signal handler for `SIGTERM`:** a handler may do almost nothing, and the rope cannot be walked in one. ADR 0051 accepts losing text to a kill.
- **Writing on a timer, or at intervals:** that is the auto-save ADR 0013 rejected.
