# 0151. Moving a session into a space at its request

- Status: Accepted, amended by 0159 and 0177
- Date: 2026-11-17

## Context

The author runs Claude Code sessions in another editor's terminal, outside oso. They want to
prepare a space in oso, tell such a session "move to oso's space X", and have its conversation
go on as a session hosted in space X, with the outside process gone.

What is fixed already:

- [ADR 0121](0121-agent-sessions-and-spaces.md): a hosted session is a `claude` in an oso
  terminal. Two processes on one conversation fork it, so every resume first reads Claude
  Code's registry and refuses a live id. "Adopting a session started outside oso, as built":
  `session-adopt` resumes a stopped conversation in the directory it ran in, since a resume
  finds its transcript only there. Step 6: a connection that is no hosted session's bridge may
  only read (the author's choice), and no tool stops or archives its caller.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md): the person lets each outside connection in.
  The channel reads the peer process and its parent from the system.
- [ADR 0136](0136-issue-spaces.md): a space has member repositories. "No tool makes, closes
  or reopens a space, adds or removes a repository". A space's orchestrator runs in the
  space's own directory.
- [ADR 0135](0135-agents-start-sessions-without-asking.md): a session started by the person,
  or on the person's `y` (`Origin::Answered`), may start sessions without asking when the
  configuration allows.

What an agent cannot do: end its own host. A live process cannot move between programs.
The conversation can move: Claude Code keeps it by its id, and oso can resume that id once
the outside process has ended.

## Decision

The author chose these on 2026-11-17, through the coordinator: the bridge stops its own
parent once its turn ends, with `/exit` as the fallback (Q1); the person's `y` also adds the
repository to the space, and the session never becomes the orchestrator (Q2); a repository
oso does not trust is refused (Q3); the moved session counts as started on the person's `y`,
and the question says so (Q4); a stopped session of oso's record is resumed in its own buffer
(Q5); no first prompt is typed (Q6); Windows has the manual stop only (Q7); and a new ADR
(Q8). `oso_platform::process::terminate_parent` was approved for step 2.

### The tool

- **`move_to_space { space }`**, a session tool of `oso mcp` and a method of the channel.
  `Reach::Change`. It asks every time (`Asks::Each`). ADR 0135's setting never lets it go
  unasked.
- **Only an outside connection may call it**, one the person let in. A hosted session's bridge
  is told it is oso's already.
- **The agent gives only the space's name.** It must be an open space's name exactly, at most
  48 bytes. The conversation's id and directory come from Claude Code's registry, never from
  the agent.

### Who asks

An outside session's `oso mcp` is a child of that session's `claude`, as Claude Code starts its
stdio MCP servers. So:

- The connecting process must run oso's own executable, and the system must name its parent.
  The server reads when that parent started as the connection comes in, off the UI thread, as
  it does for a hosted session's bridge.
- On a thread of its own (`oso-session-move`), oso reads the registry entry named by that
  parent's process id, `<pid>.json`, with ADR 0121's reader and bounds. The entry must name a
  conversation and a directory, its `kind`, if given, must be `interactive`, and its
  `procStart` (or `startedAt`) must match when the parent started. The process with that id
  now must have started then too, within the registry's 2 s: Linux counts a start from boot,
  which moves when the clock is set.
- The entry's directory must be absolute with no `.` or `..` part, as `session-adopt` holds
  it, and must be its own real path, so that the space's repository and the resume agree.
- Its project is found as ADR 0114 finds a file's, from that directory up. A linked worktree
  counts only when its main repository is trusted and records it back. The repository's main
  checkout is checked as `space-add-repo` checks one (ADR 0136).

Any of these failing refuses the call and says which. A registry under another
`CLAUDE_CONFIG_DIR` than oso's is not found: "oso cannot find this session in Claude Code's
registry".

### What is checked before the question

- The space is open. An unknown name is refused without listing the person's spaces. A closed
  one says to reopen it with `space-reopen`.
- oso trusts the repository, by its path and identity as ADR 0136 keys a member's. Otherwise
  the call is refused: the person trusts it with `trust-project`, then the agent asks again.
- oso does not run that conversation. A stopped session of oso's record with that id is
  resumed in its own buffer (Q5), and only when it ran in the same directory and is no
  space's orchestrator.
- A space holds at most 16 repositories, as before.
- The loop guard and the person's refusals are kept by the conversation's id, as an outside
  caller has no hosted session. Only one move waits at a time, and no other agent's question
  may be open.

### The question

`Agent: move session "NAME" ("claude", pid 812, in DIR, outside oso) into space X, adding
repository R to it? It stops there first, then resumes here with your settings and may start
sessions as yours do. (y or n)`

- `, adding repository R to it` is there only when R is not a member yet.
- `"claude"` is the parent's name as the system gives it, not the registry's word: any process
  of the person's can start `oso mcp` and write an entry, and the question names what runs.
- NAME and DIR come from the registry, so each is escaped and cut as ADR 0121's names are.
- It is asked as every agent's question is: from the keyboard only, after 500 ms with no key.
  `?` shows the conversation's id, its directory, the repository and a worktree whole, up to
  1024 characters, the space, the space a held worktree leaves, and that background work in
  the old process ends with it.

### After the person's `y`

1. Trust is checked again. The repository joins the space if it is not a member, and a linked
   worktree is held by the space, which takes it from a space that held it.
2. The tool replies. The reply tells the agent to end its turn now with one short line, and to
   call no more tools: the line says the session stops there once the turn ends, and to type
   `/exit` if it does not. That line is the note the other editor's terminal keeps, since oso
   cannot write there.
3. An internal buffer `*moving: NAME*` says what oso waits for. Killing it cancels the move:
   it is killed as a person's buffer is, as a rename's preview is (`Mode::SessionMove`).
4. A thread watches the outside process every 500 ms, by its id and start time. At 120 s the
   echo area says: "NAME still runs outside oso: type /exit there; oso resumes it once it
   stops". At 5 minutes oso gives up: nothing is resumed, and the echo area says to stop it
   there and use `session-adopt`. An oso exit or `oso-restart` during the wait drops the move
   the same way. What the echo area says names the session, so none of it is logged.
5. Once the process has ended, oso checks again that it trusts the repository and that X is
   open and holds it; otherwise nothing is resumed, and the echo area says why. Then it resumes
   the conversation as `session-adopt` does: in the
   directory it ran in, under the name it would give it, through the start thread, which reads
   the registry again and refuses a live id. Its origin is `Answered`, and its space is X.
   No prompt is typed. It is shown in the selected window when X is the space in front.
   Otherwise entering X shows it. The echo area says "Moved NAME into space X". A stopped
   session of oso's record resumes in its buffer the same way, its origin `Answered` now.

### Stopping the outside process (step 2)

- The bridge stops its own parent, never another process; oso watches and signals nothing.
  `channel` sits below `sessions` (ADR 0038), so the bridge does not read the registry: the
  app does, with ADR 0121's reader (the author's choice, 2026-11-18).
- Once the reply to its own `move_to_space` has gone to the agent, `oso mcp` calls the
  channel's `move_wait` on the same connection, so nothing new is asked. It is no tool: no
  agent calls it. The app holds it, for the bridge of the session that moves only, once.
- The wait's thread, which watches the process every 500 ms, also reads its registry entry
  until 120 s. The turn has ended when the entry, still that process's and that
  conversation's, says `idle` with a `statusUpdatedAt` later than the reply, the same on two
  readings in a row: a prompt the person typed as the turn ended starts the next turn at
  once. An entry that does not say when its status changed never counts, so a status from
  before the reply stops nothing.
- `move_wait` is held once and answered once. A turn that ended before the bridge asked
  answers nothing later, as the session may have gone on since. While the bridge waits, any
  request of the agent's cuts the wait, as a cancel does, and is answered: the turn has not
  ended then.
- Then `move_wait` is answered with the process's id. The bridge sends it `SIGTERM` through
  `oso_platform::process::terminate_parent(parent)`, which signals only while that id is
  the bridge's own parent: once a parent ends, its child is given to another and its id can
  be reused. On Linux a pidfd taken between two checks holds the process; on macOS the check
  and the signal follow at once, and a parent that ends keeps its id until it is reaped. It
  never sends `SIGKILL`, and never to the first process.
- If the turn does not end, for example at a permission prompt, nothing is stopped: at 120 s
  `move_wait` is answered with why not, and the person's `/exit` applies. A cancel, a give-up
  or any other end of the wait answers it the same way.
- Windows has no such stop: the bridge holds nothing there, and the move waits for the
  person's `/exit`.

### Where it lands

- In its own directory, as a session of space X under that repository. No new worktree is
  made: its work is in that checkout, and its transcript is found only there.
- Never as the orchestrator, since an orchestrator runs in the space's own directory.
- Not pinned.

### What changes with the move

- oso's command line is used, so the session's settings are the person's: no permission flag
  is passed (ADR 0121), and the flags the outside run had (`--model`, `--add-dir`, other MCP
  servers, a permission mode) are not carried. oso does not read the outside process's
  arguments.
- The moved session gets the hosted session tools. With `Origin::Answered`, ADR 0135's
  setting lets it start sessions without asking, which the question says.

### Security

- The process started is `claude --resume ID` with oso's own command line, in the directory
  the registry entry of the verified parent names, on the person's `y`, as the person's act.
- The agent's text reaches only the space lookup. Registry text in the question is escaped
  and cut. `*Agent*` logs the tool and its outcome, never a name or a directory.
- An outside conversation that was told to move, for example by text it read, gains the
  hosted tools only through the person's `y` to a question that says so.
- Any process of the person's can write the registry. A forged entry can name only a
  conversation and a directory of the person's own. The question shows the name, the
  directory and the process as the system names it, and `?` the conversation's id; a process
  that can rewrite the entry could edit the transcript as well.
- The bridge's parent is read as the connection comes, and its start time a moment later, as
  the question about the connection is asked. A parent that ended between the two, its id
  given to another `claude` meanwhile, would be taken for that one; the question would name
  that session.

### Code

- `sessions/moving.rs`: the look on its thread, the wait's steps and the words, with no `app`.
- `app/sessions/tools/moving.rs`: the call, its checks and its question, as the other tools'
  are; `app/sessions/hosting/moving.rs`: the wait and the resume.
- `channel::Event::Asks` carries the parent's start time, and the app keeps each outside
  connection's peer while it is open.
- `oso_core::buffer::Mode::SessionMove`, the waiting buffer's mode, which the person may kill.
- Step 2: `oso_platform::process::terminate_parent` (`process/parent.rs`, Unix); the
  channel's `move_wait` (`channel::protocol::Op::MoveWait`, held up to 130 s); the registry
  watch on the wait's thread (`sessions::moving::idle`) and the held reply in
  `app/sessions/hosting/moving.rs`; and the bridge's side in `channel/mcp/moving.rs`.

### The work, in steps

1. The tool, its checks, the question, the wait for the person's `/exit`, and the resume into
   the space. Where the system gives no start time, as on Windows now, the call is refused.
2. The bridge stops its parent once the turn has ended.

## Changes to accepted ADRs

Each passage below stays in its ADR as written; what holds now follows it. The statuses of
ADRs 0085, 0121, 0135 and 0136 gain `amended by 0151`.

- [ADR 0121](0121-agent-sessions-and-spaces.md), step 6, "Any other connection may only read,
  within the selected window's project": an outside connection may also call
  `move_to_space`, which asks the person each time.
- ADR 0121, "What no tool does", "never stops or archives the caller itself": in step 2,
  `move_to_space`'s bridge stops its own `claude` once the person agreed and oso saw its turn
  end.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "Each new connection asks the person": the app
  keeps what the system said of each connection let in (its process, its parent, and when
  that parent started) while it is open. In step 2 the channel gains `move_wait`, a method
  no tool names, which only the bridge of a session that moves is answered.
- [ADR 0136](0136-issue-spaces.md), "No tool makes, closes or reopens a space, adds or removes
  a repository": `move_to_space` adds the moved session's repository on the person's `y` to a
  question that names it.
- [ADR 0135](0135-agents-start-sessions-without-asking.md), "An outside connection still has
  no starting tools, and no setting gives it any": an outside connection's `move_to_space`
  resumes its own conversation in oso on the person's `y`, never unasked, and the session it
  becomes counts as started on that `y`.

## Consequences

- A session started outside oso moves into a space with one request and one `y`, and in step
  1 one `/exit`. The conversation is never run by two processes: oso resumes only after the
  old process is gone, and the start thread checks again.
- Outside connections gain their first tool that acts. It always asks, and starts nothing
  until the old process has ended.
- The moved session runs with the person's settings, not its old flags.
- Not verified on the real CLI: that the registry's `status` turns `idle` when a turn ends,
  and that `SIGTERM` after an idle turn leaves the transcript whole. The end-to-end flows use a
  fake `claude`. The author tries one move after step 2.

## Alternatives considered

- **oso signals the outside process by its id.** It is not oso's child, so its id may be
  given to another process between the look and the signal.
- **The person always types `/exit`.** Simpler, but the author asked for the session to stop
  by itself. It stays as the fallback.
- **Refuse a repository that is not a member.** Keeps ADR 0136 as it was, but costs the person
  a `space-add-repo` first; the author chose to add it on the `y`.
- **Land as the space's orchestrator.** An orchestrator runs in the space's directory, and a
  resume must run where the conversation ran.
- **A new worktree.** The conversation's work is in its checkout, and its transcript would not
  be found from another directory.
- **A first prompt after the resume.** It would spend a turn without the person.
