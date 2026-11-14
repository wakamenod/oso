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
- **T-parse:** the decoder reads only what came over the pair, from the parent. Before this
  step lands, the decoder is fuzzed on both library builds, as step 11's follow-up did under
  `oso dev`.
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
- [ADR 0090](0090-oso-dev-carries-unsaved-text.md): the unsaved text it carries under `oso dev`
  is carried over `oso-restart` too, with the same limits.

## Consequences

- Release builds gain one spawn of oso itself, from one command, after the file checks. The
  descriptor passing, the carried state and the snapshot decoder ship in release builds.
- The handoff has a second transport to test: the dev socket with the supervisor, and the
  socket pair. The order and the state stay shared.
- `just release-safety` keeps every dev method's name out of release binaries.
- A quit or a crash still ends the terminals. Option C's "quit when idle" (Q5) covers the quit.
- The steps, once accepted: the socket-pair transport and `oso-restart`, then the binary
  checks, then the notice on focus, then the quit (Q5).

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
