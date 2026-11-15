# 0133. Keeping terminals and sessions over an update of the installed oso

- Status: Accepted
- Date: 2026-11-15

## Context

[ADR 0120](0120-the-terminal.md)'s step 11 carries running terminals over `oso dev`'s restart.
The old instance stops reading each PTY and writes libghostty-vt's snapshot of it. It then
hands each master to the new instance with `SCM_RIGHTS`, through `oso dev`'s supervisor, which
relays the masters only once the old instance has exited. Hosted sessions keep their identity
and their bridge ([ADR 0121](0121-agent-sessions-and-spaces.md)). All of it is behind the `dev`
feature, as everything on the dev control socket is
([ADR 0047](0047-process-spawning-and-restart-handoff.md),
[ADR 0085](0085-agent-channel-and-oso-mcp.md): "the restart handoff … stay dev-only").

Agents now run in the installed oso, the release build: an agent that coordinates others is a
hosted `claude` session there. Quitting or updating that oso ends every terminal and session. A
session can be resumed (`session-resume`, `claude --resume`), but the turn under way is lost,
and so are shells, their jobs and their scrollback. Updating is the common case. An install puts
a new binary in place, and the running instance keeps the old one until it quits.

ADR 0120 rejected "a separate pty host process, as VS Code and herdr have" because `SCM_RIGHTS`
covered "the restart that matters now". This ADR weighs the options again for the installed
build, with security first.

### What is to be kept

- **An update:** the running oso is replaced by the binary just installed, and its terminals and
  sessions run on, with the turn under way unbroken. This is the case that matters.
- **A quit and a later start:** only a process outside oso can keep terminals alive while no oso
  runs.
- **A crash:** the emergency save keeps the text today
  ([ADR 0062](0062-emergency-save-and-the-panic-hook.md)), and terminals end.

### The threats

- **T-exec, running a binary the person did not mean to run.** An update starts a program at
  once, without the person starting it from the Dock or a shell. Whatever can replace the file
  before the update runs as the person, with every terminal's master and the carried state.
- **T-socket, a same-user process joining the handoff.** On the dev socket any same-user client
  may call methods, and step 11 gives terminals only to the parent process. A release build has
  no dev socket and must not gain one.
- **T-parse, the decoder.** The new instance parses the old one's snapshots in Zig, which is not
  memory-safe. What it parses must come only from the old instance.
- **T-keep, what a long-lived holder keeps.** A process that holds PTYs while no oso runs holds
  every program's input and output. Anything that can reach it can read or type into every
  terminal.
- **T-agent, an agent restarting oso.** A restart is the person's to make. It moves every
  terminal and session, and an agent that could trigger one could make the person's next keys
  go elsewhere ([ADR 0085](0085-agent-channel-and-oso-mcp.md)'s reach `Person`).

## Decision

Step 11's handoff ships in release builds behind a command the person runs, over a private
socket pair, after the new binary is checked (option A, Q1). With it, a quit offers to wait for
running work (Q5).

### The command (Q2)

- **`oso-restart`** (reach `Person`, no default key, `M-x`) starts the installed binary as a
  child, through `oso-platform::process`. The child gets one end of a `socketpair` as a
  descriptor it inherits and an argument that says it takes over. No socket listens: only the
  two processes hold the pair.
- **When a new binary is in place,** the running instance notices on focus (it compares the
  file's identity with its own) and says in the echo area that `oso-restart` would move to it.
  It never restarts by itself.

### Checking the new binary (Q3)

- It is the file the running instance came from (its `current_exe`, resolved).
- That file and each directory above it are owned by the person or root and writable by no one
  else, as [ADR 0108](0108-the-user-configuration-file.md) checks the configuration.
- On macOS, when the running binary carries a team identity in its signature, the new one must
  carry the same.
- Anything else, and the command says why and runs nothing.

### What travels (Q4)

- Step 11's order and contents, with [ADR 0090](0090-oso-dev-carries-unsaved-text.md)'s
  unsaved text, kills, histories and windows, over the pair instead of the dev socket and the
  supervisor. There is no `export-text` method and no listening socket, and none of the dev
  methods reach a release binary: `just release-safety` keeps checking for their names, and the
  release handoff has names of its own.
- A snapshot is written only when the new binary names the same library, as in step 11.
  Otherwise the formatter's escape sequences are written.
- The old instance exits once the new one has imported. The new one reads the PTYs only once
  the old one has gone.
- **Failures:** if the new binary does not start, refuses the version or does not import within
  10 s, it is stopped, and the old instance keeps everything and reads again. If the new one
  crashes after the old one has gone, the terminals are lost (their masters close), as in a
  crash today.

### Security

- **T-socket** does not arise: there is no socket.
- **T-parse:** the decoder reads only what came over the pair, from the parent. oso's third
  patch to libghostty-vt (its own pull request) gives a restored page the same capacity a live
  one has, and oso refuses a snapshot larger than the terminal's size and scrollback allow
  before it is decoded. Until the library snapshot can be turned on, the carried state is escape
  sequences, which replay through the live terminal. A later patch (oso's fourth) closes the
  remaining cases where a restored terminal reflows differently from a live one; only then does
  the snapshot turn on, and only after the decoder is fuzzed on both library builds until a
  restored terminal stops on no input a live terminal fed the same input does not also stop on.
  Because the snapshot always comes from the parent over a private channel, hardening the
  decoder against a damaged-but-well-framed snapshot is tracked as later, lower-priority work
  (the author's choice, 2026-11-15).
- **T-agent:** the command is `Person`, and the agent channel stops before the handoff, as an
  exit stops it. Each bridge follows its session to the new instance as under `oso dev`.
- **T-exec** is bounded by the checks above but not removed. A process that can write the
  binary's path as the person could also wait for the next launch. The update makes such a
  binary run sooner; it does not make it possible.
- **T-keep** does not arise: nothing holds a PTY while no oso runs.

## Questions

For the author, through the coordinator, with the recommended option first.

1. **The approach.**
   (a) A: the handoff in release builds, behind `oso-restart`, over a private socket pair.
   (b) B: a pty host process that outlives oso.
   (c) C: resume only, with "quit when idle" and sessions resumed at start.
2. **Who may restart.**
   (a) Only the person, through `oso-restart` (`Person`). oso says when a new binary is in
   place and never restarts by itself.
   (b) Also oso itself, once a new binary is in place and every session is idle.
3. **Checking the new binary.**
   (a) The running binary's own path, owned by the person or root and writable by no one else
   up to the root; on macOS also the same team identity when the running binary has one.
   (b) The same path only.
4. **What travels.**
   (a) Everything step 11 and ADR 0090 carry: terminals, sessions, unsaved text, kills,
   histories and windows.
   (b) Terminals and sessions only; a modified buffer holds the restart.
5. **Quitting with work under way** (with any option).
   (a) `C-x C-c` lists the sessions with a turn under way and the running programs, and offers
   "quit when idle".
   (b) As today: it asks once, with the unsaved text.

## The author's choices

The author answered every question on 2026-11-15, through the coordinator: (a) for each. So step
11's handoff ships in release builds behind `oso-restart` (`Person`), over a private socket pair,
with no listening socket and no dev method in a release binary (Q1). Only the person restarts,
and oso says when a new binary is in place (Q2). The new binary is the running one's own path,
owned by the person or root with no other writer up to the root, and on macOS it has the same
team identity when the running one has one (Q3). Everything step 11 and ADR 0090 carry travels
(Q4). `C-x C-c` lists the turns under way and the running programs, and offers "quit when idle"
(Q5).

## Changes to accepted ADRs

- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "Screenshots, the restart handoff, timings and
  hot reload stay dev-only": the dev control socket's handoff stays dev-only. A release build
  has its own handoff over a private socket pair, run only by `oso-restart`.
- [ADR 0120](0120-the-terminal.md), "A release build has no handoff, and its terminals end with
  it": a release build carries its terminals over `oso-restart`. They still end with a quit or
  a crash.
- [ADR 0120](0120-the-terminal.md), step 11 as built, "All of it is behind the `dev`
  features": the descriptors' passing, a PTY master from a descriptor and `oso-term`'s handoff
  API are in every build, and `oso-term` has no `dev` feature (step 2, as built). The dev
  socket's methods stay dev-only.
- [ADR 0090](0090-oso-dev-carries-unsaved-text.md): the unsaved text it carries under `oso dev`
  is carried over `oso-restart` too, with the same limits.
- [ADR 0047](0047-process-spawning-and-restart-handoff.md), "Inheriting the whole environment"
  rejected: `oso-restart` gives its own binary every variable of its own that `process` lets a
  child have by name, all but `PATH` and the loader's, so that the new instance and the shells
  it starts see what this one did. Every other spawn keeps the allowlist.
- [ADR 0042](0042-statics-snapshots-and-render-errors.md), no statics: one `static` is allowed,
  the gate in `oso-platform`'s `privacy::inheritance`, a lock that holds no data (step 2, as
  built).

## Consequences

- Release builds gain one spawn of oso itself, from one command, after the file checks. The
  descriptor passing, the carried state and the snapshot decoder ship in release builds.
- The handoff has a second transport to test: the dev socket with the supervisor, and the
  socket pair. The order and the state stay shared.
- `just release-safety` keeps every dev method's name out of release binaries.
- A quit or a crash still ends the terminals. Option C's "quit when idle" (Q5) covers the quit.
- The steps: first the library patch and the fuzzing that T-parse asks for (its first pull
  request turns the snapshot back on under `oso dev`), then the socket-pair transport and
  `oso-restart`, then the binary checks, then the notice on focus, then the quit (Q5). The
  release handoff was to land only after the first; the author chose on 2026-11-15 to ship it
  before the snapshot turns on (see "Step 2, as built").

## Alternatives considered

- **B, a pty host process (`oso-ptyd`)** that oso starts on first need. It holds every PTY and
  their terminal engines (or the raw bytes and a replay buffer), and outlives any oso: an
  update, a quit, a crash. oso connects to it, draws its snapshots and sends its input.
  - It isolates the parser: terminal output, which is untrusted, is parsed outside the process
    that holds secret buffers' text. An exploit of libghostty-vt reaches the host, not oso's
    memory. This is its strongest security argument.
  - T-keep is its cost. While no oso runs, the host holds every terminal. It needs a socket and
    so its own authentication: the same user, plus a secret oso keeps. A file of mode 0600 in
    the private directory is readable by any same-user process, so it proves little; the macOS
    keychain or a descriptor inherited from oso's first start do better. Every process of the
    person's that can reach the host can read and type into every terminal, which today only
    oso's own memory allows.
  - A second process and a protocol to keep in step with oso. Both sides have versions over an
    update: the host is the old binary until it restarts, which brings the problem back for the
    host itself. There is a hop per output byte or per snapshot, and macOS does not expect an
    app's helper to outlive it.
  - Rejected for now: the listening host and its own update problem cost and risk more than the
    update case needs. It deserves its own ADR if crashes or the parser's isolation become the
    priority.
- **C, resume only, made better.**
  - At a quit or an update, oso lists the sessions with a turn under way and the terminals
    running a program, and offers to wait for the turns to end ("quit when idle").
  - At the next start, hosted sessions come back stopped (as today). With a new setting,
    `[sessions] resume_on_start = true`, oso resumes each session it remembers as running, with
    `--resume`, once the person has been asked at that start.
  - Shells come back stopped, in their last directory (OSC 7), with their scrollback lost.
  - Nothing new ships beyond a start of `claude` the person agreed to, as `session-resume` is
    today. It costs little.
  - Rejected as the whole answer: the turn under way and the shells' jobs are still lost. Its
    "quit when idle" is kept as Q5.
- **Restarting by itself once every session is idle (Q2 (b)):** no command to remember, but an
  update would then start a program without the person asking, which T-exec makes the person's
  call.

## Step 2, as built

Built on 2026-11-15. The author approved the plan, its questions (the recommended option for
each), the new public items and the guarded edits through the coordinator.

- **Before the snapshot (the author's choice, 2026-11-15).** The release handoff ships while
  every terminal still travels as the formatter's escape sequences (ADR 0120, step 11 as built):
  the screen in use and its scrollback come across, and a full-screen program redraws its
  screen; its other screen and a sequence it had not finished do not. While the snapshot is off,
  a terminal whose entry says `snapshot` is not restored and ends failed, so the decoder reads
  nothing that came over the pair (T-parse), and no terminal keeps the 2 MiB of an unfinished
  sequence that only the snapshot carries.
- **Where it lives.** The state, its texts, the terminals' entries, the descriptors and the
  requests (`Op`) moved from `dev/handoff` to `crates/oso/src/carry/`, on a layer of its own
  under `dev`, so `oso dev`'s supervisor and the pair ask the app the same requests and the app
  answers both with one implementation. `oso-term`'s `dev` feature is gone: the handoff API is in
  every build. Descriptor passing and a PTY master from a descriptor left `oso-platform`'s `dev`
  feature too.
- **The command.** `oso-restart` (`Person`, no key) asks the host through
  `TerminalRequest::Restart`. The host refuses as `check-handover` with terminals does (a buffer
  that may hold secrets shown, a password prompt, a terminal starting, more than 64, text or a
  restart under way), then stops the agent channel and starts the `oso-restart` thread.
- **The binary, until step 3.** The path is the running binary's, resolved when the instance
  started: Linux names a replaced binary `… (deleted)` from then on. On the thread, before
  anything else, the file and every directory above it must be writable by no one but the person
  and root, by `privacy::who_can_change` (ADR 0108's checks, by mode and owner), and the file
  must be an executable one that runs as the person, not setuid or setgid; otherwise the command
  says why and runs nothing. The team identity on macOS is
  step 3's.
- **The start.** The child is started through `oso_platform::process`, with `--take-over`, and
  `--no-config` if this instance read none (a dev build passes its own options too). Its
  standard input is closed and its output goes where this instance's goes. It runs in a process
  group of its own. It gets this instance's variables by name, all but `PATH` (the module's own)
  and the loader's. `Spawn::pass_fd` makes its end of the pair the child's descriptor 3 and marks
  every other one close-on-exec in the child (`process::pass_in`); the spawn forks, on the
  thread. A free descriptor 3 is held here meanwhile, so that the standard library's pipe that
  reports a failed start cannot be the one the child's end replaces.
- **The new instance** takes descriptor 3 first of all, while it has one thread:
  `ControlStream::inherited` marks it close-on-exec and checks that it is a Unix domain socket
  that this process inherited. Its other end must be its parent's: otherwise `--take-over` exits
  with status 2 and opens nothing. Once its window is up, its `oso-carried` thread speaks first.
- **The order** (`carry::going`, tested with fakes):
  1. The new instance says `hello`, with the state versions it reads and its terminal library,
     within 10 s; one that does not, or that cannot read this state's version, is killed and
     reaped.
  2. The UI thread exports the state with its terminals; each text goes as `stage-piece` lines,
     then `take-state`, answered once the new instance's files are judged. Each answer is due
     within 10 s; a failure until then kills the new instance, and this one reads its terminals
     again, turns the agent channel back on if it was on, and says why.
  3. The UI thread hands over a second descriptor of each master; they go with `take-masters`,
     and the new instance answers that it holds them, within 5 s.
  4. The UI thread exits if its generation is still the export's, letting the terminals go with
     nothing hung up; otherwise, and after a failure since the import, the new instance is told
     to `stay` out of it. It gives up the terminals it took, whose screens stay, and closes if it
     was not changed since its import.
  5. The new instance attaches the masters only once the pair has ended and its parent is no
     longer the instance before (looked for every 50 ms, for 10 s; otherwise they are given up).
     So one instance reads each PTY at a time.
- **Two orders, one state.** `dev/restart.rs` (the supervisor's) and `carry::going` (the
  instance going's) are two orders over the same state, the same requests and one piece relay
  (`carry::pieces::relay`); a change to one is a change to the other. The app answers both as
  it answers the parent: `Passed::parent` means the asker may have terminals, which over the
  pair, and for the instance's own restart thread, it may.
- **The agent channel** is held off in both instances while the restart is under way: a start
  asked for meanwhile, by the person or by a session that starts or comes back, happens once
  the restart is over. The instance going turns it back on if it was on and it stays.
- **The exit** is waited for with no time limit once asked: the new instance is told to stay
  only once the instance going has refused, so the two never both close.
- **The pair** carries one JSON object a line, at most the dev socket's line, descriptors only
  with `take-masters`. Its names are its own: `just release-safety` now also checks that no
  release binary holds the names of `export-text`, `import-state`, `hand-over-terminals` or
  `import-terminals`, nor of the dev build's hook `OSO_DEV_RESTART_FAIL`. A reason from the
  other end is cut to 200 characters with no control
  character before it is said.
- **Descriptors and children.** macOS makes a socket pair, and takes descriptors from a message,
  close-on-exec only a moment later. A gate in `oso-platform`'s `privacy::inheritance`, a lock
  that holds no data, is held shared by every spawn and alone around those two moments, so none
  of oso's children starts inside one. It is the one `static` the architecture test allows (an
  exception to ADR 0042). A read that takes descriptors on macOS waits for its bytes first, then
  reads without waiting inside the gate.
- **Bridges** follow as under `oso dev`: the new instance writes `<pid>-follows`, which the
  bridge reads in every build now.
- **New public items:** `oso-platform`'s `ControlStream::{pair, inherited}`, `AsFd` for
  `ControlStream`, `control::INHERITED_FD`, `process::PassedFd`, `Spawn::pass_fd` and
  `process::may_pass`, which the restart's filter of its variables reads;
  `oso-core`'s `TerminalRequest::Restart`; `oso-term`'s handoff API leaves the `dev` feature.
- **Tests.** The order's failures (`carry::going`), the new instance's side (`carry::coming`),
  the pair's lines, the binary's checks, a passed descriptor and a refused snapshot have unit
  tests. End-to-end flows (`tests/e2e/terminal/oso_restart.rs`): a shell goes on, the same
  process; a password prompt holds the restart; a new instance that ends before it speaks, and
  an exit refused once the new one holds everything, leave the terminal here, reading again
  (a dev build's `OSO_DEV_RESTART_FAIL` stops the restart at those points).
- **Not done:** the macOS team identity, and access lists on the binary's way, in step 3; the
  notice on focus (step 4, done; see below); quitting when idle (step 5); the snapshot's return (ADR 0120, step
  11's library patches); starting the new instance with `posix_spawn` rather than a fork, with
  the terminals' starts. `oso-restart` run in an instance `oso dev` supervises starts a child the
  supervisor does not know of; `oso dev`'s own restart is the one to use there.

## Step 4, as built

Built on 2026-11-15. The author approved the plan and its questions through the coordinator,
with two changes: the notice is given in every build, `oso dev`'s included, and it says no more
than was checked (below).

- **The identity.** `main` takes the identity of the binary's path next to the path itself,
  before the event loop (the path `oso-restart` starts): its device and inode, modification
  time and length. A thread of its own, `oso-binary` (`carry::notice`), started with the app,
  tells later files at that path from it. An install that renames a new file over the old one,
  or writes the old one again, changes it.
- **On focus.** Each time the window gains focus, the UI thread asks the thread for a look and
  waits for nothing; a look asked for while one waits is that one. The thread stats the path
  and, for an identity it has not checked yet, runs `oso-restart`'s own checks on the file
  (`binary::check`). It reports a file that is not the one this instance started from.
- **Said once.** The echo area says "The file oso started from has changed: M-x oso-restart
  would start it, with terminals and sessions", or that `oso-restart` would refuse it and why,
  once per identity. Until step 3 checks the signature, nothing tells a new oso from another
  file the person's own processes put there, so it does not say that a new oso is installed;
  step 3 may. Nothing is said while a restart is under way, or in a new instance until it has
  taken over: a later focus says it. A path with no file says nothing. oso never restarts by
  itself (Q2).
- **The notice does not bind the restart.** `oso-restart` checks and starts the file at the
  path when it runs, which may have changed again since it was said.
- **Where it runs.** On Unix, where `oso-restart` runs. Under `oso dev` the binary changes with
  each build, and the notice says so too, though `oso dev`'s own restart is the one to use
  there.
- **The limit (for step 3).** The path is the one resolved at start (Q3). An install that puts
  the new binary in another directory and moves a link to it, as a package manager that keeps
  one directory per version does, leaves the old file in place: neither the notice nor
  `oso-restart` sees the new binary.
- **Driving it.** The dev socket's `window-focus` is the window's focus event with all that
  follows it, the binary's look among it; `focus` still sets only whether the window has the
  keys.
- **Tests.** The thread finds a file renamed over the binary on each look, with why
  `oso-restart` would refuse one it cannot run or others can change, says nothing of the file
  at start or of a missing one, and ends with its looker; the notice says each identity once
  (`carry::notice`).
