# 0178. A rate limit on the session tools

- Status: Accepted
- Date: 2026-11-23

## Context

[ADR 0177](0177-no-loop-guard-on-the-session-tools.md) removed the loop guard: 20 counted
calls, then a wait until the person typed in the session. No count of calls bounds a session
now. A session that loops, or that a prompt injection steers, can call the session tools up to
the channel's 30 requests a second, one in flight, on its bridge's connection. Its Consequences
list what that leaves open: a removal that fails runs git, and `gh` on the person's GitHub
token, on every call; a prepare job of `start_worktree_session` or `session_start` runs git
before any question or limit; and `session_rename` asks nothing, so nothing refuses it.

The author asked for a follow-up (ADR 0177, Consequences): a per-session rate limit on calls
that ask or change, one that never needs the person to type, that bounds runaway loops only
and is never felt in normal use.

The busiest normal use is an orchestrator's round over its space. With eight workers it
removes eight worktrees, starts or resumes eight sessions and renames eight. That is 24 calls
if all of it falls in one minute. In practice they come in bursts of about eight.

## Decision

The author accepted this ADR on 2026-11-23, through the orchestrator session.

- **At most 30 calls that ask or change in any 60 s, per calling session.** The window slides:
  a call's place frees 60 s after the call came. `sessions::tools::CALLS_A_MINUTE` is 30 and
  `RATE_WINDOW` is 60 s. There is no setting.
- **Which calls count:** every call `Guard::admit` lets through. Those are the tools that ask
  or change (`worktree_remove`, `start_worktree_session`, `session_start`, `session_resume`,
  `session_rename`, `space_add_repo`, `draft_set`) from a hosted session, and `move_to_space`
  by its conversation's id. A call counts once it is admitted, even when a later check or its
  job refuses it. A failing removal's git and `gh` run is what this bounds.
- **Which calls do not count:** the reading tools (`worktree_list` keeps its own 6 a minute);
  a call refused before the guard (another agent's question waits, the person is busy, a scope
  or lookup error); and a call the person's 60 s refusal holds back at the guard. The person's
  refusal is checked first and says what it said before. A refusal found only after the guard
  counts, as any later check does: the person's refusal of the branch or repository a job
  resolved, and another agent's question found waiting by a start that might have gone
  without asking but asks after all.
- **Kept by session, never by connection,** as the refusals are: a bridge that connects again
  keeps its window.
- **The refusal says how long to wait:** "this session made 30 calls that ask or change in the
  last minute; wait N s and try again; do not reach its effect another way meanwhile", with N
  the whole seconds, rounded up, until the oldest call leaves the window.
- **The MCP server's instructions** say: "A session makes at most 30 session tool calls that
  ask or change a minute; past that, a call says how long to wait: wait that long, then try
  again, and do not reach its effect another way meanwhile." A test checks that they name
  `CALLS_A_MINUTE`.
- **Nothing the person does is limited.** The person's commands do not go through the session
  tools, and nothing waits for the person to type.
- **Memory:** each session keeps at most 30 instants. Past 256 sessions, a new one first drops
  those with no call in the window.

## Changes to accepted ADRs

- [ADR 0177](0177-no-loop-guard-on-the-session-tools.md), Decision, "**No count of calls.** A
  session's session tools are not counted and not refused for how many calls it has made": no
  count waits for the person, but a session makes at most 30 calls that ask or change in any
  minute. Its Consequences "A runaway agent is no longer bounded by oso" and "`session_rename`
  and calls that fail can repeat without end": they repeat at most 30 times a minute per
  session. Its rejected alternative "**A count by time**" is the decision here, at the author's
  request. Its Follow-up is this ADR.
- ADR 0177, Decision, "**The MCP server's instructions** … No tool description or brief
  mentions a limit of calls": the instructions name the rate; no tool description or brief
  does.
- ADR 0177, Consequences, "What still holds it:": the rate of 30 calls a minute per session
  holds it too. "What that leaves open", "a session the person started can start sessions in
  a trusted project with no limit at all" and "start and remove worktree sessions in turn
  without end": at most 30 such calls a minute. "`sessions::tools::Guard` keeps only the
  refusals": it keeps the refusals and each session's window, in `sessions::tools::guard`.
- ADR 0177, "Changes to accepted ADRs": each passage it gives as what holds now for
  [ADR 0121](0121-agent-sessions-and-spaces.md) (`sessions::tools` holds the table of reach,
  the refusals and the questions' wording), [ADR 0127](0127-source-control-and-github.md)
  (`draft_set` is not counted), [ADR 0128](0128-orchestration-and-automations.md) (no call is
  counted), [ADR 0135](0135-agents-start-sessions-without-asking.md) (with `"none"` nothing
  stops it), [ADR 0144](0144-agent-starts-with-no-limit-and-in-the-background.md) (with
  `"none"`, no limit at all, no such bound), [ADR 0151](0151-moving-a-session-into-a-space.md)
  (the person's refusals are kept by the conversation's id),
  [ADR 0153](0153-agents-resume-sessions.md) (resumes are not counted),
  [ADR 0155](0155-renaming-sessions.md) (nothing counts a rename, without end) and
  [ADR 0161](0161-an-orchestrator-adds-a-repository-to-its-space.md) (no limit beyond 16
  members): each such call counts toward the rate; `sessions::tools` holds the rate too, and
  `move_to_space`'s window is kept by the conversation's id, as its refusals are. No count
  waits for the person.

## Consequences

- An orchestrator's round over eight workers stays well within the limit, and nothing waits
  for the person. A message from another device still never leaves a session stuck.
- A runaway session's session tools go from about 30 calls a second to 30 a minute. The other
  bounds of ADR 0177 hold as they were: the questions, the 60 s refusal, `agents_start_limit`
  and the channel's own rate.
- The limit is per session. Several sessions that run away together each get 30 a minute.
- An agent past the limit is told how long to wait. The MCP instructions say a tool oso
  refused is a decision. This one says to wait and try again, and, as for a decision, not to
  reach the same effect another way meanwhile.
- `move_to_space` checks for another agent's question before the guard, as `admit_tool`
  does, so that refusal does not count. Its look at the asker runs before the guard, one at a
  time, as it did before.
- The code is small: `Guard`, now in `sessions::tools::guard`, keeps a window per session
  beside the refusals, and `Refusal::Rate` says the wait.
- Follow-up: `worktree_list`'s 6 a minute is a second sliding window, kept in `app`. It could
  move beside `Guard` and share its window.

## Alternatives considered

- **20 a minute:** an eight-worker round of removes, starts and renames comes close to it.
- **60 a minute or more:** a failing removal's `gh` calls from one session come near
  GitHub's hourly limit.
- **A limit on each tool** (as `worktree_list`'s 6 a minute): a table of numbers to tune, and
  a loop that moves between tools gets the sum.
- **Count every call, refused or not:** a session that retries while another agent's question
  waits would use up its window doing nothing.
- **A setting:** one more number for a bound that should never be felt; it can come as a new
  ADR if real use reaches it.
