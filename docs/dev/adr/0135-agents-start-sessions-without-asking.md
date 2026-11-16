# 0135. Agents start sessions without asking

- Status: Accepted, amended by 0136, 0144
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
- The record of a session keeps how it started (`key`, `answered`, `unasked`), so the
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

## The author's choices

The author answered every question on 2026-11-15, through the coordinator: (a) for each. So the
setting is the boolean `[sessions] agents_start_without_asking` (Q1). It covers
`start_worktree_session` and `session_start`, and `workers_start` once ADR 0128 builds it (Q2). It
reaches only callers the person started, by a key or a `y` (Q3), in trusted projects only (Q4). At
most 4 live unasked sessions per caller, `agents_start_limit` 1 to 8, and past that the call asks
(Q5). The setting is read at oso's start only (Q6). Each start is said in the echo area,
`*Messages*` and the `*Agent*` log, the row is marked and the brief is behind `session-explain`
(Q7). `agent-starts-ask` and `agent-starts-allow` hold until oso restarts (Q8). The loop guard is
unchanged (Q9), and the registry entry records how a session started (Q10).

## As built

Built on 2026-11-15 in one step. The coordinator approved the plan, and the author the two points
below and the oso-core variant.

- **The brief is kept in memory only (the author's choice, 2026-11-15).** oso's session record
  (`sessions::record`, not Claude Code's registry) caps 256 sessions at 256 KiB and a brief may
  be 64 KiB, so the record keeps only `origin`, written for every session: `person`, `answered`,
  `unasked`. A line without it is the person's, unless it has a parent: such a line, or one
  stripped of its origin, reads as `unknown`, as does a value oso does not know, and neither may
  start sessions without asking. A line written afresh by someone with the person's account
  could still claim `person`; that is the same residual as `config.toml`'s. `Hosted::unasked_brief`
  keeps the brief, as the session got it (oso's header and the hand-off facts included), until oso
  exits. `session-explain` says first "Started by session "api-fix" without asking, as the
  configuration allows; its brief is at the end.", then oso's explanation, then the brief, so that
  no brief reads as oso's text; "It had no brief." for none; after a restart "…; its brief is not
  kept across a restart.". The worker's explanation carries the session's key, so two explains
  in flight never swap their headers.
- **The row's mark is `· unasked` (the author's choice, 2026-11-15),** after the place and the pin,
  in place of "started by api-fix without asking": a row is already nested under the session
  that started it. In the tree, where a row under its worktree shows no place, the mark shows
  alone.
- **`SessionRequest::AgentStarts(bool)` is a new oso-core public variant (the author's OK,
  2026-11-15).** Commands reach the app only through the editor's requests, and the setting lives
  in the app, so `agent-starts-ask` and `agent-starts-allow` need one; the snapshot gains one
  line.
- **Where it decides:** `sessions::tools::starts_without_asking`, a pure function over the tool
  (only those whose table entry is `Asks::UnlessAllowed`: `start_worktree_session` and
  `session_start`), the setting and the commands, the caller's origin, the trust and the
  caller's live unasked sessions (running ones, ones being got ready and worktrees being made for
  them), at the point where the question would open. Every check and the loop guard run before
  it, as for a question. Another agent's open question does not hold back a call that may start
  without asking; it does if the call asks after all. `app/sessions/tools/starting.rs` holds the
  decision's inputs and what runs once a call may, on `y` or without asking.
- **`oso dev`'s restart** ([ADR 0120](0120-the-terminal.md), step 11) carries each running
  session's origin in its state, read as the record's is; a brief is not carried, so
  `session-explain` says it is not kept.
- **Trust is taken as it is now (found in review):** a `session_start` in the caller's own
  directory starts without asking only if an open buffer has the project trusted now, the
  session's own among them (ADR 0132), never by the trust the session began with, which the
  person may have taken back. A worktree, or another directory, is looked at by a job against the
  record of trusted projects as it is now, as before.
- **Origins (Q3):** `session-resume` keeps a session's recorded origin, so a resumed unasked one
  still asks for its children; `session-fork`, `session-adopt` and `session-worktree` are the
  person's keys and start sessions of the person's. A question from a caller that started
  without asking says "It started without asking itself.", as a `y` lets its child start
  sessions without asking in turn.
- **The launch:** `InForce::changes` gives the setting on the launch's look only, off from a file
  a group can change ("config.toml: [sessions] agents_start_without_asking not applied: other users
  in a group can change it"), and a later change says "… = true applies from the next start", or
  "agents_start_limit = N applies from the next start". With it on, oso says "Sessions started by
  agents start without asking, as config.toml says (agent-starts-ask stops it)".
- **What is said:** each start's line goes to the echo area and `*Messages*` once its start is
  under way, after the worktree's "Making the worktree for x for an agent…" and "made" lines, so
  that the echo area ends on it: `Agent: session "repo" is starting a session in a new worktree
  "…" on new branch "x" from "main", without asking (brief: 1 line; C-x a e explains)`. `*Agent*`
  logs "start_worktree_session: starting without asking" and then "started". Every reply that
  starts a session says `"asked"`; one without asking adds a note. The tools' descriptions tell
  agents so.
- **e2e:** `session_tools::a_worktree_session_starts_without_asking_up_to_the_limit_and_asks_past_it`,
  `session_tools::after_agent_starts_ask_a_start_asks_again` and
  `session_tools::a_session_in_its_own_directory_starts_without_asking_until_the_project_is_untrusted`.
  A caller that itself started without asking, which needs a second bridge, and the counts of
  sessions being got ready, are unit tests of the rule only.

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
