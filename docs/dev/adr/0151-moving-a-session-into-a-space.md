# 0151. Moving a session into a space at its request

- Status: Accepted
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
  conversation and a directory, and its `procStart` (or `startedAt`) must match when the
  parent started. The process with that id now must have started then too.
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
  resumed in its own buffer (Q5), and only when it ran in the same directory.
- A space holds at most 16 repositories, as before.
- The loop guard and the person's refusals are kept by the conversation's id, as an outside
  caller has no hosted session. Only one move waits at a time, and no other agent's question
  may be open.

### The question

`Agent: move session "NAME" (claude, pid 812, in DIR, outside oso) into space X, adding
repository R to it? It stops there first, then resumes here with your settings and may start
sessions as yours do. (y or n)`

- `, adding repository R to it` is there only when R is not a member yet.
- NAME and DIR come from the registry, so each is escaped and cut as ADR 0121's names are.
- It is asked as every agent's question is: from the keyboard only, after 500 ms with no key.
  `?` shows the conversation's id, its directory, the repository, the space, and that
  background work in the old process ends with it.

### After the person's `y`

1. Trust is checked again. The repository joins the space if it is not a member, and a linked
   worktree is held by the space.
2. The tool replies. The reply tells the agent to end its turn now with one short line, and to
   call no more tools: in step 1, the line tells the person to type `/exit` there. That line
   is the note the other editor's terminal keeps, since oso cannot write there.
3. An internal buffer `*moving: NAME*` says what oso waits for. Killing it cancels the move.
4. A thread watches the outside process every 500 ms, by its id and start time. At 120 s the
   echo area says: "NAME still runs outside oso: type /exit there; oso resumes it once it
   stops". At 5 minutes oso gives up: nothing is resumed, and the echo area says to stop it
   there and use `session-adopt`. An oso exit during the wait drops the move the same way.
5. Once the process has ended, oso resumes the conversation as `session-adopt` does: in the
   directory it ran in, under the name it would give it, through the start thread, which reads
   the registry again and refuses a live id. Its origin is `Answered`, and its space is X.
   No prompt is typed. It is shown in the selected window when X is the space in front.
   Otherwise entering X shows it. The echo area says "Moved NAME into space X".

### Stopping the outside process (step 2)

- The bridge stops its own parent, never another process. Once the reply to its own
  `move_to_space` said the person agreed, `oso mcp` watches its parent's registry entry every
  250 ms, for at most 120 s, until its `status` is `idle`, that is, until the turn has ended.
  Then, if its parent is still the process the entry names and started when it says, it sends
  `SIGTERM` to it through `oso_platform::process::terminate_parent`. It never sends
  `SIGKILL`.
- A parent's process id cannot be given to another process while its child, the bridge,
  runs, so this is safer than oso signalling a process it did not start.
- If the turn does not end, for example at a permission prompt, nothing is stopped, and
  step 1's `/exit` at 120 s applies.
- Windows has no such stop, so the move waits for the person's `/exit` there.

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
  conversation and a directory of the person's own, and the question shows both.

### Code

- `sessions/moving.rs`: the look on its thread, the wait's steps and the words, with no `app`.
- `app/sessions/tools/moving.rs`: the call, its checks and its question, as the other tools'
  are; `app/sessions/hosting/moving.rs`: the wait and the resume.
- `channel::Event::Asks` carries the parent's start time, and the app keeps each outside
  connection's peer while it is open.
- Step 2: `oso_platform::process::terminate_parent` (Unix), and the bridge's watch in
  `channel/mcp`.

### The work, in steps

1. The tool, its checks, the question, the wait for the person's `/exit`, and the resume into
   the space. This works on every system.
2. The bridge stops its parent once the turn has ended.

## Changes to accepted ADRs

Each passage below stays in its ADR as written; what holds now follows it. The statuses of
ADRs 0085, 0121 and 0136 gain `amended by 0151`.

- [ADR 0121](0121-agent-sessions-and-spaces.md), step 6, "Any other connection may only read,
  within the selected window's project": an outside connection may also call
  `move_to_space`, which asks the person each time.
- ADR 0121, "What no tool does", "never stops or archives the caller itself": in step 2,
  `move_to_space`'s bridge stops its own `claude` once the person agreed and its turn ended.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "Each new connection asks the person": the app
  keeps what the system said of each connection let in (its process, its parent, and when
  that parent started) while it is open.
- [ADR 0136](0136-issue-spaces.md), "No tool makes, closes or reopens a space, adds or removes
  a repository": `move_to_space` adds the moved session's repository on the person's `y` to a
  question that names it.

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
