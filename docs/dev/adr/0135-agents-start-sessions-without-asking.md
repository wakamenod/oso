# 0135. Agents start sessions without asking

- Status: Proposed
- Date: 2026-11-15

## Context

A coordinator is a session oso hosts that starts other sessions with the session tools
([ADR 0121](0121-agent-sessions-and-spaces.md)): `start_worktree_session` makes a worktree and
starts a session in it, and `session_start` starts one in a directory of the caller's repository.
Both are "Change, asks": each call opens an `Agent:` question, and the process starts on the
person's `y`, as the person's act. ADR 0121 saw the cost: twenty parallel threads are twenty
questions. ADR 0128's `workers_start` (accepted, not built) asks once for a run of up to 8.

The author answers these questions many times a day, for coordinators they started themselves,
and chose (2026-11-15) a setting in `config.toml` that lets agents start sessions without the
question.

What the question guards today, and what still guards it without one:

- **Starting a process** is `Person` by ADR 0085's rule ("anything that spawns a process"); ADR
  0121 lets a session tool ask instead, and the person's `y` starts it.
- **A worktree runs git,** whose hooks are the repository's code (`post-checkout`), so worktree
  tools need a trusted project (ADR 0114). `session_start` in the caller's own directory runs no
  git, and works in a restricted project.
- **The new session's own permissions** are untouched: oso passes no permission option, so each
  session asks the person in its own terminal as their Claude Code settings say (ADR 0121).
- **Who may call:** only a proven bridge of a session oso hosts has these tools; an outside
  connection may read sessions and worktrees only.
- **Limits:** the loop guard (at most 20 effectful calls since the person last typed in the
  calling session), the 60 s refusal after a `n`, the brief's 64 KiB, and ADR 0085's rate limits.
- **What a prompt injection in a session could do** with no question: start sessions with briefs
  of its choosing, which then work under the person's Claude Code permissions and cost tokens.
  A session with `Bash` can also write `config.toml` under the person's account, so a setting
  that an agent can turn on for itself, and that takes effect at once, would remove the question
  for everyone.

## Decision

### The setting (Q1, Q6)

- **`[sessions] agents_start_without_asking = true`** in the person's `config.toml` (default
  `false`). It is read only from that file (ADR 0108): no repository file, project setting or
  environment variable sets it, and no tool or command writes it (ADR 0085, ADR 0121: no tool
  edits `config.toml`).
- **It applies from oso's start only (Q6),** as `[agent-channel] start` does (ADR 0110): a look
  that finds it changed says "config.toml: [sessions] agents_start_without_asking = true applies
  from the next start", so a session that writes the file cannot use it in the oso that runs it.
  At start, with it on, the echo area and `*Messages*` say "Sessions started by agents start
  without asking, as config.toml says".
- **A file that other users in a group can change does not apply it,** nor one whose way to it
  they can change, as for `[agent-channel] start`; `*Messages*` says so and every other setting
  applies.

### What it covers (Q2, Q3, Q4)

- **`start_worktree_session` and `session_start` (Q2)** start without the question when every
  rule below holds; otherwise they ask exactly as now. `session_stop`, `session_archive`,
  `merge_branch` and the read tools that ask are unchanged. ADR 0128's `workers_start` follows
  the same rules when it is built, as one call for its run.
- **The caller is a session the person started (Q3):** by a key (`claude`, `session-worktree`,
  `session-resume`, `session-adopt` and the like) or by answering `y` to a question. A session
  started without asking asks for its own children, so the setting reaches one level, not a tree.
- **Trusted only (Q4):** the caller's project is trusted, and for `session_start` in another
  directory, that directory's repository is the caller's and trusted too. `session_start` in the
  caller's own restricted directory asks, as now.
- **Every check that runs before the question still runs** (the branch, a branch checked out
  elsewhere, the directory, the brief's size); a failing check refuses with its reason, as now.

### Limits (Q5, Q9)

- **At most 4 live sessions started without asking per caller (Q5),**
  `[sessions] agents_start_limit` (1 to 8, read with the setting). Past it, the call asks, as it
  does with the setting off. A stopped or archived session no longer counts.
- **The loop guard and the 60 s refusal are unchanged (Q9):** an unasked start counts as one of
  the 20 calls since the person last typed in the caller; a call that falls back to asking and
  is refused is refused for 60 s, as now. Past 20, the call is refused with "waits for the person
  to type in this session".

### What the person sees (Q7, Q8)

- **Each unasked start is said (Q7)** in the echo area and `*Messages*`: `Agent: session api-fix
  started session x in worktree .claude/worktrees/x on new branch x from main, without asking
  (brief: 42 lines; C-x a e explains)`. The `*Agent*` log has the call, as for any request.
- **`*Sessions*` marks the row** "started by api-fix without asking", and `session-explain`
  (`C-x a e`, `e`) on it shows the brief as `?` would have.
- **`agent-starts-ask` (`Person`, Q8)** makes these calls ask again until oso restarts, and
  `agent-starts-allow` takes that back; the start line at launch names the first.
- **The tool's reply** says "started without asking, as the person's config.toml allows", so the
  caller can tell the person what it did.

### Agents

- **Reach:** the tools stay `Change`. The process starts as the person's standing choice in their
  own file, not as an agent's act; nothing else in ADR 0085's `Person` rule changes.
- **An outside connection** still has no starting tools, and no setting gives it any.

### Code

- `config`: the two settings, applied at start only, and the group check.
- `sessions::tools`: `admit` decides "asks" or "starts" from the setting, the caller's origin, the
  trust and the count, as one pure function with tests.
- `app/sessions/tools/asks.rs`: on "starts", run the plan the `y` would run, and say the line.
- The registry entry of a session keeps how it started (`key`, `answered`, `unasked`), so the
  caller rule and the row's mark survive a restart.
- e2e: a fake coordinator's `start_worktree_session` starts with no question when set, asks
  past the limit, and asks from a session started without asking.

## Questions

1. **The setting.**
   (a) `[sessions] agents_start_without_asking = true`, a boolean, `false` by default.
   (b) `[sessions] agent_starts = "ask" | "allow"`, so a later value (such as "allow in worktrees
   only") fits.
   (c) A list of tools, `agents_start_tools = ["start_worktree_session"]`.
2. **Which tools.**
   (a) `start_worktree_session` and `session_start`, and `workers_start` when it is built.
   (b) `start_worktree_session` only: a worktree keeps the new session's changes apart.
   (c) Both, but not `workers_start`, which keeps its one question.
3. **Which callers.**
   (a) A session the person started, by a key or a `y`; one started without asking asks for its
   children.
   (b) Any hosted session, the limit of Q5 bounding the tree.
   (c) Only sessions the person marks, with a new `session-allow-starts` (`Person`).
4. **Trust.**
   (a) Trusted projects only; `session_start` in the caller's own restricted directory asks.
   (b) Trusted projects, and `session_start` in the caller's own directory anywhere, as it runs
   no git.
5. **A limit.**
   (a) At most 4 live unasked sessions per caller (`agents_start_limit`, 1 to 8); past it, ask.
   (b) The same limit, but past it refuse.
   (c) No limit but the loop guard.
6. **When it applies.**
   (a) From oso's start only, as `[agent-channel] start`; a later change says it applies at the
   next start.
   (b) At once, on every look at the file.
7. **How a start is said.**
   (a) The echo area and `*Messages*`, the `*Agent*` log, and a mark on the row with the brief
   behind `session-explain`.
   (b) As (a), and asking for attention (a Dock bounce), as a session that needs input does.
   (c) `*Messages*` and the row only, so the echo area stays quiet.
8. **Turning it off for now.**
   (a) `agent-starts-ask` / `agent-starts-allow` (`Person`), until oso restarts.
   (b) Only by editing `config.toml` and restarting.
9. **The loop guard.**
   (a) Unchanged: an unasked start is one of the 20 calls, and the 60 s refusal holds.
   (b) A lower count for unasked starts: at most 8 since the person typed in the caller.
10. **Where the start is recorded.**
    (a) In the registry entry, so a restored session keeps its origin and mark.
    (b) In memory only: after a restart every restored session counts as the person's.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0121](0121-agent-sessions-and-spaces.md), the tools table, "`session_start` | Change,
  asks" and "`start_worktree_session` | Change, asks": they start without asking under this
  ADR's rules, when the person's `config.toml` sets `agents_start_without_asking`.
- ADR 0121, "**The process starts on the person's `y`, as the person's act.**": or on the
  person's standing setting, under this ADR's rules.
- ADR 0121, "Agent-requested sessions cost the person a question each": not when the setting
  allows it.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "anything that spawns a process" is `Person`, as
  ADR 0121 amended it ("a session tool may ask the person, and the process starts on the person's
  `y`"): or starts without asking under this ADR.
- [ADR 0128](0128-orchestration-and-automations.md), "`workers_start` | Change, asks once": or
  starts without asking under this ADR's rules, as one call.

## Consequences

- A coordinator the person started runs its threads with no question each, in trusted projects,
  up to the limit, and each start is said where the person looks.
- Every start still goes through the same checks, and each new session asks for its own
  permissions as before: the setting removes the question, not the new session's prompts.
- A prompt injection in a session the person started can start up to the limit of sessions with
  briefs of its choosing, which cost tokens and work under the person's Claude Code permissions;
  the line in the echo area, the row's mark and `agent-starts-ask` are how the person sees and
  stops it.
- An agent that writes `config.toml` changes nothing until the person restarts oso, and the
  change is said.
- Follow-ups: the registry's new field; the e2e flow; `workers_start` when ADR 0128 is built.

## Alternatives considered

- **Remember a `y` for the rest of the session ("always for this caller")**: no setting, but a
  question still per coordinator, and a remembered answer is easy to give by habit. The author
  chose a setting.
- **Batch the questions** (one question for several starts): `workers_start` does this for runs;
  it still asks, and a coordinator's starts come one at a time.
- **Allow it for outside connections too**: they have no proven session, so nothing bounds or
  attributes what they start.
- **Apply a change at once**: a session that writes `config.toml` through `Bash` could lift the
  question for itself in the same oso.
- **No limit**: one injected session could start sessions until the loop guard stops it, twenty
  at a time.
