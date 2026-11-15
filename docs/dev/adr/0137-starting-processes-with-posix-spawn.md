# 0137. Starting processes with `posix_spawn`

- Status: Accepted
- Date: 2026-11-15

## Context

`oso-platform::process::spawn` started every child through the standard library's `Command`.
`Command` uses `posix_spawn` where it can, but a `pre_exec` hook makes it fork, and two kinds of
child had one:

- **A child in a terminal** ([ADR 0120](0120-the-terminal.md)): the hook calls `setsid` and
  `TIOCSCTTY`, sets the job-control signals back to their defaults, empties the signal mask and
  marks every descriptor from 3 up close-on-exec.
- **The new instance `oso-restart` starts** ([ADR 0133](0133-keeping-terminals-over-an-update.md),
  step 2): the hook puts the pair's end at descriptor 3 and marks the rest close-on-exec. A free
  descriptor 3 was held by a file of nothing during the spawn, so that the standard library's
  pipe that reports a failed `exec` could not take it.

A fork of a large, many-threaded process takes the allocator's locks while it copies the address
space, and runs every library's after-fork handlers in the child. One of those, libnotify's on
macOS, aborted terminal children under load until the parent learned to set libnotify up first
(ADR 0120). Other libraries' handlers could do the same.

Which descriptors a child gets was also guarded in two ways. The terminal's and the new instance's
hooks marked the rest close-on-exec in the child. Every other child got whatever this process
held without close-on-exec at that moment. macOS makes several kinds of descriptor without
close-on-exec and marks them a moment later, so ADR 0133 added a gate in
`privacy::inheritance`: a lock with no data that every spawn held shared, and the making of a
socket pair or the receipt of descriptors held alone. It was the one `static` the architecture
test allowed, an exception to [ADR 0042](0042-statics-snapshots-and-render-errors.md), and it
covered only the places that took it.

## Decision

Every child on macOS and Linux is started by `posix_spawn`, called by hand in
`process/unix.rs`, with file actions that name each descriptor the child gets. The one
exception is a child in a terminal on macOS, which still forks. Windows keeps the standard
library's `Command`, as it has no terminal (ConPTY is later) and no descriptor to pass. The author
approved all the recommended options (2026-11-15).

### Only the descriptors the child is given

- **macOS:** `POSIX_SPAWN_CLOEXEC_DEFAULT`. The kernel closes every descriptor the file actions
  do not create as it starts the child. An inherited standard stream is named with
  `posix_spawn_file_actions_addinherit_np`.
- **Linux:** `posix_spawn_file_actions_addclosefrom_np` from 3 (from 4 with a passed
  descriptor), after the file actions that place them. This needs glibc 2.34 or later (Ubuntu
  22.04, Debian 12, RHEL 9 and later), bound directly through `libc`, with no fallback. musl has
  no such call and is not supported: the build stops with an error that says so.
- **The streams:** `Sink::Inherit` keeps this process's own, as `exec` would: on macOS one that
  is close-on-exec, or not open, is left out. A closed input and `Sink::Null` open `/dev/null`
  in the child, and a pipe's end is placed with `dup2`. The ends this process
  keeps become the standard library's `ChildStdin`, `ChildStdout` and `ChildStderr`.
- **A passed descriptor** (`Spawn::pass_fd`) is placed at 3 with `dup2`. A source below 4 is
  first copied above 3, close-on-exec, so that no file action overwrites a later one's source.
  `posix_spawn` reports a failed `exec` itself, so nothing holds descriptor 3 any more.
- **The rest:** the directory comes from `posix_spawn_file_actions_addchdir_np`, a group of the
  child's own from `POSIX_SPAWN_SETPGROUP`, and an empty signal mask from
  `POSIX_SPAWN_SETSIGMASK`. `SIGPIPE` is set to its default with `POSIX_SPAWN_SETSIGDEF`, as the
  standard library sets it, and in a terminal so are the job-control and termination signals
  (ADR 0120).
- **The environment** is exactly the allowlist, `PATH` and the caller's `set`; when two entries
  share a name, the last one wins, as with `Command`.

So a descriptor that this process made without close-on-exec, or that macOS marks only a moment
later, reaches no child. Whatever made it, and on whatever thread, it needs no lock.

### A child in a terminal

- **Linux:** `POSIX_SPAWN_SETSID`, which glibc runs before the file actions, and the slave
  opened by its name at 0 without `O_NOCTTY`, which a session leader with no terminal takes as
  its controlling one. `dup2` then copies it to 1 and 2. The `Slave` keeps its own descriptor
  open until the caller drops it, as before. As the old hook failed when `TIOCSCTTY` did, the
  spawn asks the terminal's session once `posix_spawn` returns (after the child's `exec`), and
  ends, reaps and fails a child whose terminal is another session's. A terminal of no session
  passes (amended 2026-11-15, after an intermittent CI failure). Linux takes the terminal from
  a session as its leader exits, so a short child that has ended before the check leaves it so,
  whereas an open that did not take it leaves it another session's. Failing that child had
  reaped it and lost its output and status. It asks through
  a close-on-exec copy of the master that the `Slave` holds: Linux answers `TIOCGSID` on a
  slave only for the caller's own controlling terminal.
- **macOS forks, as before.** XNU runs the file actions before `POSIX_SPAWN_SETSID`, and only
  the session leader's own `TIOCSCTTY` gives a session a controlling terminal: opening a
  terminal never does. So `posix_spawn` cannot start a child with one. A stress test of 8
  threads × 50 `/bin/sleep` children, with the slave either `dup2`'d or opened by name and
  `POSIX_SPAWN_SETSID` set, left 0 of 400 with a foreground group. A shell can hide this, as
  `/bin/sh` takes a terminal as its own once it runs, so the test that guards it starts `sleep`.
  The child-side hook stays as ADR 0120 built it, behind `#[cfg(target_os = "macos")]` in
  `pty/unix.rs`, without the Linux `close_range` branch, and still sets libnotify up first. On
  this Mac, `fork` returned to the parent in about 0.3 ms at 100 MB, 1 GB and 4 GB of touched
  heap, against about 0.1 ms for `posix_spawn`. So the fork that remains costs little. Its
  after-fork handlers stay a risk, for terminal starts on macOS only.

### `process::Child`

The standard library makes a `Child` only from its own spawn, so `spawn` now returns
`oso_platform::process::Child`, with the methods oso uses: public `stdin`, `stdout` and `stderr`
of the standard library's pipe types, `id`, `wait` (it closes the standard input first),
`try_wait`, `kill` and `wait_with_output` (it reads both outputs at once, one on a scoped thread
of its own). It reports `std::process::ExitStatus` and `Output`. On Unix it waits with `waitpid`
by the child's ID and keeps the status. `kill` does nothing once the child has been waited for,
as its ID may be another process's by then, nor once `waitpid` has said `ECHILD`, which means
something else waited for it. As with the standard library's, dropping it neither
waits nor kills. On Windows it wraps the standard library's.

### The gate goes

`privacy::inheritance` keeps only `INHERITED_FD`. `ControlStream::pair` takes no lock. On macOS,
`recv_with_fds` reads as any blocking read does, honouring the read timeout through the socket's
own, and marks what came close-on-exec right after. The architecture test lets no file declare a
`static` any more, so ADR 0133's exception to ADR 0042 ends.

### Unsafe code and checks

- In `process/unix.rs` and `process/unix/objects.rs`, which holds the file actions and
  attributes as values that destroy themselves, each libc call has its own block with a
  `// SAFETY:` comment: their set-up, additions and destruction (the objects are boxed, so they
  never move), the signal sets, `F_GETFD` before an inherit, `posix_spawn`, `F_DUPFD_CLOEXEC`,
  `waitpid`, `kill`, and `tcgetsid` on Linux. The standard library's `Command` is built in one
  place, `process::command`, for Windows and for the macOS terminal's child.
  `posix_spawn_file_actions_addinherit_np` and `posix_spawn_file_actions_addchdir_np` are
  declared in a local `extern` block on macOS, as `libc` binds neither (libSystem has had both
  since macOS 10.15).
- `clippy.toml` is unchanged. `libc::posix_spawn` stays banned, and `process/unix.rs` silences
  the ban with `#[expect]`. The architecture test's list of files that may do that gains
  `process/unix.rs` and keeps `process/pty/unix.rs`, for the macOS hook.
- **Tests:**
  - No spawn forks on the test's own thread, counted by a `pthread_atfork` handler: plain,
    piped, own group and passed descriptor. A child in a terminal forks once on macOS and never
    on Linux. The test failed for the terminal and the passed descriptor before this change.
  - A child, a child with a passed descriptor and a terminal child each list their open
    descriptors from 0 to 63, and one made without close-on-exec at 200 or above. They see only
    0–2, plus 3 for the passed one.
  - `sleep` in a terminal is its terminal's foreground group as soon as the spawn returns.
  - A passed descriptor at 2, with the standard error piped, is still the child's 3. The test
    fails if a source below 4 is not first copied above.
  - Also tested: the directory and the variables (the last of a name wins), a NUL refused, a
    program removed after it was found failing to start, waiting and killing, a child waited
    for elsewhere never signalled, and both full pipes read at once.
  - The leak test is the reverse of the one it replaces: before, a plain child was expected to
    see a descriptor left open on exec.

## Changes to accepted ADRs

- **ADR 0120, Decision.** "A `Spawn` gains `terminal: Option<&Pty>`, and a `pre_exec` hook
  calls `setsid` and `TIOCSCTTY`" now holds on macOS only. Linux starts a terminal's child with
  `posix_spawn`, as above. "The spawn runs on the terminal's thread, never the UI thread, since
  `fork` of a large process takes milliseconds" still holds, now because the spawn reads the
  file system to build `PATH`. "The child's exit status is the standard library's
  `Child::wait`" is now `process::Child::wait`.
- **ADR 0120, step 2 as built.** "The child-side hook (`pre_exec`, which makes the standard
  library fork) …" now holds on macOS only, without the `close_range` branch. "The architecture
  test allows silencing them in `process.rs` and in `process/pty/unix.rs` … only": it also
  allows `process/unix.rs`, the start by hand. "As a spawn with a terminal forks the whole
  process" holds on macOS only. "`posix_spawn` for terminals … would need an ADR": this is that
  ADR, for Linux; macOS cannot. "One `unsafe` `pre_exec`" is the macOS hook alone.
- **ADR 0133, step 2 as built.** "`Spawn::pass_fd` makes its end of the pair the child's
  descriptor 3 and marks every other one close-on-exec in the child (`process::pass_in`); the
  spawn forks … A free descriptor 3 is held here meanwhile": the end is placed by a file action
  of `posix_spawn`, nothing else reaches the child, nothing forks and nothing holds descriptor 3.
  "Descriptors and children": the gate is removed, as above. With it goes ADR 0133's change
  to ADR 0042, the one `static` it let the architecture test allow: no file may declare one
  now. Under "Not done", "starting the new instance with `posix_spawn` rather than a fork, with
  the terminals' starts" is done, but for terminals on macOS.

## Consequences

- No child of oso's inherits a descriptor by chance, whatever made it. The guard sits in one
  place, the spawn, instead of at each place that makes a descriptor.
- Nothing forks but a terminal's child on macOS, so a spawn copies no address space and runs no
  after-fork handler.
- Linux builds need glibc 2.34 or later, and musl is not supported.
- `spawn`'s callers name `process::Child` instead of `std::process::Child`. The perf harness
  keeps its own `Command` (ADR 0046).
- Windows is unchanged.

## Alternatives considered

- **Keep the gate and the hooks** (ADR 0133). This leaves the forks, and a gate that covers only
  the places that take it.
- **Only terminal and passed-descriptor children by hand; the rest on `Command`.** This leaves
  the other children inheriting what macOS marks late.
- **A spawn function of its own, with its own child type, for the two forking cases**, with
  `spawn` unchanged. That makes two child types and two paths to keep in step.
- **Linux: look up `addclosefrom_np` at run time, falling back to the fork.** It would keep
  older glibc and musl, at the cost of a second path that CI would barely test.
- **Linux: reopen the slave by the descriptor oso holds** (a handle-relative reopen) instead of
  by name. Not now: oso already opens the slave by name.
- **macOS terminals without a fork:** a trampoline, either oso's own binary in a hidden first
  mode or a small helper binary next to it, that runs `TIOCSCTTY` and then `exec`. It needs no
  fork, but costs an extra `exec` of a large binary, or a new binary to build, ship and trust,
  for a fork measured at 0.3 ms.
