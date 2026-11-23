# 0177. No loop guard on the session tools

- Status: Accepted
- Date: 2026-11-23

## Context

[ADR 0121](0121-agent-sessions-and-spaces.md) gave the agents' session tools a loop guard. A
hosted session may make at most 20 counted tool calls, every tool that asks or changes, until
the person types in its terminal. Past that, each counted call is refused with "waits for the
person to type in this session". Keys, text and pastes the person sends to that terminal reset
the count. A `y` to a question does not. The MCP server's instructions told agents "After 20
such calls the tools wait until the person types in your session".

Later ADRs leaned on it as the bound of a run: [ADR 0135](0135-agents-start-sessions-without-asking.md)
(Q9), [ADR 0144](0144-agent-starts-with-no-limit-and-in-the-background.md) (`"none"` for
"no limit but the loop guard") and [ADR 0136](0136-issue-spaces.md). The tools of
[ADR 0127](0127-source-control-and-github.md), [ADR 0128](0128-orchestration-and-automations.md),
[ADR 0151](0151-moving-a-session-into-a-space.md), [ADR 0153](0153-agents-resume-sessions.md),
[ADR 0155](0155-renaming-sessions.md) and [ADR 0161](0161-an-orchestrator-adds-a-repository-to-its-space.md)
each count toward it.

In use it only gets in the way. An orchestrator that starts, resumes and renames workers
reaches 20 calls in one run of work. A message the person sends from another device reaches
the session without going through its terminal, so it never resets the count, and the session
stops until the person comes back to oso to type in it.

## Decision

The author decided on 2026-11-23, through the orchestrator session, to remove the loop guard
entirely, and accepted this ADR the same day.

- **No count of calls.** A session's session tools are not counted and not refused for how
  many calls it has made. `sessions::tools::MAX_CALLS`, `Refusal::Loop` and the per-session
  count go.
- **Nothing resets anything when the person types.** The terminals no longer tell the session
  tools about the person's input (`person_typed_in_session` goes).
- **The person's 60 s refusal is unchanged.** An `n` or `C-g` to a question refuses the same
  tool with the same target from the same session for 60 s, kept by session (or by
  conversation, for `move_to_space`), and `was_refused` still looks up a resolved target. Only
  a question's answer records a refusal, so a tool that never asks is never refused this way.
- **The MCP server's instructions** drop "After 20 such calls the tools wait until the person
  types in your session". No tool description or brief mentions a limit of calls.
- **Every other rule holds:** each tool's reach, its questions, the caller rule, trust, the
  checks before a question, the start limit (`[sessions] agents_start_limit`) where set, and
  the agent channel's own bounds (ADR 0085: 30 requests a second per connection, one in
  flight).

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0121](0121-agent-sessions-and-spaces.md), "The bridge and the channel", "**The loop
  guard:** a session may cause at most 20 effectful tool calls … since the person last typed in
  its terminal": no count of calls holds. The 60 s refusal that follows stays.
- ADR 0121, step 6, "`sessions::tools` holds the table of reach, the loop guard, the refusals
  and the questions' wording": it holds the table of reach, the refusals and the questions'
  wording.
- ADR 0121, step 6, "**The loop guard** counts each call of `worktree_remove`,
  `start_worktree_session` and `session_start` by session …", and "the loop guard's reset from
  the terminal's input": neither holds. The step list's "the loop guard" in "Agent tools" is
  dropped.
- ADR 0121, step 6, "The server's instructions say … and that the tools wait for the person
  after 20 calls": they no longer say it.
- [ADR 0135](0135-agents-start-sessions-without-asking.md), Limits, "the loop guard (at most
  20 effectful calls since the person last typed in the calling session)", and "**The loop
  guard and the 60 s refusal are unchanged (Q9)**": an unasked start is not counted; the 60 s
  refusal holds. So do the author's choices' "The loop guard is unchanged (Q9)" and the as-built
  "Every check and the loop guard run before it": every check runs before it. Its rejected
  alternative "**No limit**" said one injected session could start
  sessions until the loop guard stops it; with `"none"` nothing stops it now (below).
- [ADR 0144](0144-agent-starts-with-no-limit-and-in-the-background.md), "the loop guard (Q9):
  at most 20 effectful calls since the person last typed in the caller", and "It also takes
  `"none"`, for no limit but the loop guard": with `"none"`, unasked starts have no limit at
  all. So do the author's "up to 20 starts each time the person types in a caller with
  `"none"`", its Changes bullet "With `"none"`, up to 20 each time the person types in that
  session", and its Consequences "can start up to 20 sessions each time the person types in
  it": with `"none"` there is no such bound. Its "The loop guard already bounds a run" no
  longer holds.
- [ADR 0128](0128-orchestration-and-automations.md), "a loop guard of 20 effectful calls since
  the person last typed", and "**The loop guard** counts `workers_start` and `merge_branches` as
  one call each": no call is counted.
- [ADR 0127](0127-source-control-and-github.md), `draft_set` "counted by the loop guard": it
  is not counted. It asks nothing, so no refusal holds it back either.
- [ADR 0136](0136-issue-spaces.md), "the loop guard and the person's question stay as they
  are" and "`agents_start_limit`, and the loop guard are unchanged": the person's question and
  `agents_start_limit` stay; there is no loop guard.
- [ADR 0151](0151-moving-a-session-into-a-space.md), "The loop guard and the person's refusals
  are kept by the conversation's id": the person's refusals are.
- [ADR 0153](0153-agents-resume-sessions.md), "**The loop guard and the 60 s refusal** count it
  as any call that changes", and "eight workers take eight calls, within the loop guard": the
  60 s refusal applies to a resume that asks; resumes are not counted.
- [ADR 0155](0155-renaming-sessions.md), "It counts toward the loop guard, by the session's id,
  once the session is found and the name is clean, before the clash check, so that probing which
  names are taken counts too", and "up to the loop guard": nothing counts a rename, and a
  session can rename the sessions it may rename without end.
- [ADR 0161](0161-an-orchestrator-adds-a-repository-to-its-space.md), "**No limit beyond the
  loop guard and 16 members (Q4)**", its answer "(a) None beyond the loop guard and 16 members",
  and "**The loop guard and the 60 s refusal** count it as any call that changes": no limit
  beyond 16 members; the 60 s refusal applies to an add that asks.

## Consequences

- An orchestrator, or any session, can run as many session tool calls as its work takes, and
  a message from another device does not leave it stuck until the person types in oso.
- **A runaway agent is no longer bounded by oso.** A session that loops, or one a prompt
  injection steers, can call the session tools without end. What still holds it:
  - every tool that asks puts a question to the person, one agent question at a time;
  - the person's `n` refuses the same call (tool and target) from that session for 60 s;
  - `[sessions] agents_start_limit`, where it is a number, caps how many unasked sessions a
    caller has running at once, not how many it starts over time: a session that ends, or
    whose worktree is removed, frees its place. A session started without asking still asks
    for its own starts (ADR 0135's one level);
  - the channel's 30 requests a second, one in flight, per connection. Each hosted session's
    bridge is a connection of its own, so the total grows with the sessions.
- **What that leaves open:**
  - A stream of questions: a session that changes the target each time (another branch,
    another directory) asks again right after each `n`, and while its question is open other
    agents' questions wait. The person stops it with `agent-starts-ask` (for unasked starts),
    by stopping the channel, or by ending the session; there is no "refuse everything from
    this session".
  - With `agents_start_without_asking` on and `agents_start_limit = "none"`, a session the
    person started can start sessions in a trusted project with no limit at all, each working
    under the person's Claude Code permissions and costing tokens. With a number, it can start
    and remove worktree sessions in turn without end.
  - A tool that asks nothing is never refused, as only a question's answer records a refusal.
    `session_rename` and calls that fail can repeat without end; `draft_set` stays bounded by
    the requests the person pasted, `space_add_repo` by 16 members, and a removal by the
    worktrees there are.
- The code is smaller: `sessions::tools::Guard` keeps only the refusals.
- Docs: `docs/dev/security.md`, `architecture.md`, `ui.md` and `docs/user/daily-use.md` drop
  the 20 calls.
- **Follow-up (the author, 2026-11-23):** a per-session rate limit on calls that ask or change
  (for example 30 a minute) that never needs the person to type, to bound runaway loops only,
  in a separate PR with its own ADR.

## Alternatives considered

- **Reset the count on a message from any device**, not only on input to the terminal: oso
  does not see messages that reach Claude Code another way, so it cannot tell them from the
  agent's own work.
- **A higher count** (say 200): still stops a long orchestrator run at an arbitrary point, and
  still needs the person at oso to go on.
- **A count by time** (at most N calls a minute): a different guard with a number of its own
  to tune; the author wanted none.
- **Keep the guard, off by a setting**: one more setting for a guard the author does not want;
  it can come back as a new ADR if runaway runs show up.
