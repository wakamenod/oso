# 0153. Agents resume sessions

- Status: Accepted
- Date: 2026-11-17

## Context

On 2026-11-17 the Mac logged out and every hosted session stopped. Once oso started again, it
brought them back stopped, as [ADR 0121](0121-agent-sessions-and-spaces.md) has it: no process
starts by itself. The person resumed the space's orchestrator with `C-c a r`
(`session-resume`), but the orchestrator could only start new sessions (`session_start`,
`start_worktree_session`). A new session does not have the stopped one's conversation, so each
worker's context was lost, or the person had to resume each worker by hand.

What holds now:

- **`session-resume` (`Person`)** runs `claude --resume ID` in the session's own buffer, with its
  name and directory, and an orchestrator's or worker's `--add-dir`s made again from its space
  ([ADR 0136](0136-issue-spaces.md)). Its start thread first reads Claude Code's registry and
  refuses a conversation that a live process runs, offering `session-fork`. Five seconds later
  it reads the registry again and warns when two processes run the conversation. Two processes
  on one conversation fork it silently (ADR 0121, "Restore and the fork hazard").
- **ADR 0135** lets a session the person started start sessions without asking, in a trusted
  project, up to `agents_start_limit` live ones each; past that the call asks.
- **ADR 0136** scopes a space's orchestrator to its space's sessions and repositories.

## Decision

The author answered each question below on 2026-11-17, through the space's orchestrator: (a)
for each.

### The tool (Q1, Q3, Q8)

- **`session_resume { session }`**, a session by its id or name from `session_list`, `Change`,
  "asks unless allowed", like `session_start`. Its reply waits until the session runs again,
  or says why not.
- **Only a space's current orchestrator calls it (Q1)**, and only on its space's sessions, which
  are the only ones it sees (ADR 0136, step 4a). Any other session is refused with "only a
  space's orchestrator resumes sessions". A worker keeps its repository's scope and has no
  resume.
- **Which sessions (Q3):** a stopped or failed one that is not archived and still has its
  buffer. Refused, each with its reason: the caller itself, one that runs or is starting, an
  archived one, an orchestrator of the space, current or earlier (`-was-n`), and one whose
  buffer is gone. The person resumes those.
- **The loop guard and the 60 s refusal** count it as any call that changes, with the session's
  id as its target, so a refusal holds for another spelling of the same session.

### Asking (Q2)

- **ADR 0135's rule decides whether it asks:** the setting, `agent-starts-ask`, a caller the
  person started, and the target's project trusted now by an open buffer of it, the session's
  own among them, with the session's directory at or under that project's root. Otherwise it asks `Agent: let session "fix-login" resume session "api" in
  "…/.claude/worktrees/api"? (y allow, n refuse)`. `?` says that the session takes up its own
  conversation and gets no prompt.
- **The limit (Q2):** while it runs, a session resumed without asking counts as one of the
  caller's live unasked sessions, toward `agents_start_limit`. The app keeps this mark in memory
  only. The session keeps its recorded origin: a resume does not change how its conversation
  began, and ADR 0135 already keeps a resumed session's origin. Its parent may be an earlier
  orchestrator, after a handover, so the count cannot go by parent.
- A session resumed after the person's `y`, or by the person's `session-resume`, does not count.
- **At the person's `y`** every rule is checked again: the caller is still its space's
  orchestrator, the session still of that space, and still one a resume takes.

### The resume (Q4, Q6)

- **It is the person's resume:** same buffer, name, directory and space checks, and no baseline.
  It runs behind what the person is doing, as an agent's start does
  ([ADR 0144](0144-agent-starts-with-no-limit-and-in-the-background.md)): its buffer is not
  shown.
- **A conversation that runs in another process is refused (Q4):** "its conversation runs in
  process PID in DIR: stop it there first; oso forks a conversation only for the person". The
  echo area says so too. No fork is offered to the person, and the agent cannot fork.
- **No prompt (Q6):** `claude --resume ID` exactly as the person's. The orchestrator messages the
  session with `SendMessage` once it runs.
- The check made 5 s after a resume still warns, in the echo area, when two processes run the
  conversation.

### What the person sees (Q5)

- A resume without asking: `Agent: session "fix-login" is resuming session "api" in "…",
  without asking` in the echo area and `*Messages*`, and `session_resume: starting without
  asking` then `started` in `*Agent*`. The row keeps its marks.
- The reply says `"asked": false` with ADR 0135's note, or `"asked": true`.

### One at a time (Q7)

- One session per call. After a logout, eight workers take eight calls, within the loop guard,
  and those past the limit ask.

### Code

- `sessions::tools`: the table's ninth tool, `resume_question` and `resume_started`, and
  `resumable`, a pure function over what the app knows of the target, with tests.
- `channel::protocol::sessions`: `Call::Resume { session }` (at most 255 bytes), which waits for
  the person as a start does. `oso mcp` describes it.
- `app/sessions/tools/resume.rs`: the call, `Plan::Resume`, and what runs at the `y` or without
  asking: the rule checked again, then `App::resume_hosted`. `App::resume_hosted` holds the
  body of `session-resume`, which now calls it too.
- `Report::Live` for a tool's resume answers the call and offers no fork
  (`App::session_tool_live`). `App::resume_session` and `App::resume_hosted` are in
  `app/sessions/hosting/resume.rs`, beside `fork.rs`.
- e2e: `space_tools::an_orchestrator_resumes_the_stopped_sessions_of_its_space`.

## Questions

1. **Who may call it.**
   (a) Only a space's current orchestrator, on its space's sessions.
   (b) Also any hosted session, for its own children in its repository.
   (c) Any session, for anything in its scope.
2. **The limit.**
   (a) A resume without asking counts toward the caller's limit while it runs, by a mark kept in
   memory; the session keeps its recorded origin.
   (b) The session's origin becomes `unasked`, recorded, so it asks for its own children and
   shows `· unasked`.
   (c) Resumes do not count.
3. **Which sessions.**
   (a) Stopped or failed ones, not archived, not the caller or an orchestrator.
   (b) As (a), stopped only.
4. **A conversation that runs elsewhere.**
   (a) Refuse the agent; offer the person no fork.
   (b) Refuse the agent, and put the person's fork question.
5. **What the person sees.**
   (a) The echo area, `*Messages*` and `*Agent*`; the row unchanged.
   (b) As (a), and a `· resumed` mark on the row while it runs.
6. **A prompt for the resumed session.**
   (a) None; the orchestrator messages it.
   (b) A short line from oso as its first prompt.
7. **Many at once.**
   (a) One session per call.
   (b) Also `sessions_resume { sessions }`, one question for all.
8. **Where it is recorded.**
   (a) A new ADR that amends 0121, 0135 and 0136.
   (b) An amendment inside ADR 0135.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0121](0121-agent-sessions-and-spaces.md), the tools table: a new row, `session_resume` |
  Change, asks | a stopped session of the orchestrator's space, as `session-resume`.
- ADR 0121, "**`session-resume` (`Person`)** runs `claude --resume ID`": an orchestrator's
  `session_resume` runs it too, under this ADR's rules; a live conversation is refused with no
  fork offered.
- [ADR 0135](0135-agents-start-sessions-without-asking.md), "What it covers":
  "`start_worktree_session` and `session_start` … start without the question": and
  `session_resume`, under this ADR's rules; a session resumed without asking counts toward
  `agents_start_limit` while it runs.
- [ADR 0136](0136-issue-spaces.md), "Session tools in a space": the orchestrator may resume the
  stopped sessions of its space with `session_resume`; "No tool … replaces an orchestrator"
  holds, as no tool resumes an orchestrator.

## Consequences

- After a logout or a crash, the person resumes the orchestrator, and it resumes its workers
  with their conversations, without a question up to the limit when the configuration allows.
- The fork hazard is guarded as for the person: a live conversation is refused, and the race
  the CLI leaves open is warned about 5 s later.
- A prompt injection in an orchestrator can resume its space's stopped sessions, up to the limit
  without asking. Each resumed session takes up a conversation the person had already allowed,
  under the person's Claude Code permissions, and costs tokens. The echo line and
  `agent-starts-ask` let the person see it and stop it.
- The mark that counts a resume toward the limit is kept in memory only, as the brief of an
  unasked start is (ADR 0135, as built).

## Alternatives considered

- **Resume every stopped session when oso starts:** ADR 0121 Q2 refused this. Opening oso must
  run nothing.
- **`session_start` with a `resume` argument:** one tool with two meanings, and a resume takes
  none of a start's arguments.
- **Let the agent fork a live conversation:** a fork makes a second conversation the person did
  not ask for, and the agent cannot tell which process the person meant to keep.
- **Let workers resume their own children:** no case needs it yet, and after a handover the
  parent of a space's workers is an earlier orchestrator.
