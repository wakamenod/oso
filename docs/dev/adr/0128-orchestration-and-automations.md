# 0128. Orchestration and automations

- Status: Accepted
- Date: 2026-11-13

## Context

On 2026-11-13 the author chose two ways of starting sessions that ADR 0121 left out:

- **Orchestration:** a coordinator agent splits a task, starts worker agents in their own worktrees,
  and collects their results.
- **Scheduled automations:** prompts run on a schedule against a repository.

[ADR 0121](0121-agent-sessions-and-spaces.md) gives the parts: hosted sessions with states from the
screen; `start_worktree_session`, `session_list`, `merge_branch` and the other session tools, each
effectful one asking the person; nesting a session under the one that started it; the CLI's own
`SendMessage` between sessions; a loop guard of 20 effectful calls since the person last typed in
the session; and "a coordinator is just a session", with its routing, "start all" and scheduled
sessions deferred. It also says "agent-requested sessions cost the person a question each … batching
them is left for later". [ADR 0126](0126-sessions-and-worktrees-in-depth.md) adds the board, worktree
creation in the background and usage limits.

The rules that hold: no permission flag is ever passed; only a person types into a terminal, so oso
cannot type into a coordinator's prompt to wake it; no state authorises anything; nothing starts when
oso or a project opens; `git` only in trusted projects; oso has no network code and no `claude -p`
([ADR 0031](0031-claude-code-integration.md)).

### What Orca does

From its documentation and source (sources at the end):

- **Orchestration** is experimental and, for now, CLI only. A run has tasks with dependencies and
  states; a dispatch is one attempt on one terminal; messages (`worker_done`, `question`,
  `escalation`, `heartbeat`, `merge_ready`, …) go to the coordinator's inbox. The coordinator is an
  ordinary agent taught the CLI by a skill: it creates tasks and starts workers (`worker-start
  --worktree new-child --agent claude`). Workers get a preamble: report `worker_done` exactly once.
  The coordinator ends its turn and **Orca wakes it** by typing into its terminal when mail arrives;
  `check --wait` long-polls. Workers' permission prompts go to the person, unless a setting lets
  coordinators approve them. Nothing merges by itself: `merge_ready` is only logged. Nesting is 4
  deep by default; no cap on workers is documented.
- **Automations:** made in the CLI or an editor (no config file): a schedule (hourly, daily,
  weekdays, weekly at a time, cron or RRULE, with a time zone), a prompt, an agent, and a repository
  in which each run makes a new worktree `auto-<slug>-<stamp>` (or runs in an existing one). A
  precheck command can skip a run. Runs need Orca running; after sleep or a start, one missed run is
  made late within a grace period, otherwise it is recorded as missed. Arguments that change
  permissions are refused. Each run's session and worktree stay for inspection.
- **Claude Code** has subagents inside one session (not separately visible or reviewable) and, as an
  experiment, agent teams whose teammates run in split panes of tmux or iTerm2; oso is neither, and
  their state and worktrees would bypass oso's sessions (checked on the current CLI before step 1).

## Decision

### A run (Q1)

- **A run is a coordinator session and its workers,** recorded by oso: the objective, the
  coordinator's session id, and for each worker its task title, branch, base, session id, and its
  report. Saved with ADR 0121's `sessions` file; no screen text.
- **`orchestrate` (`Person`, `C-x a O`)** asks for the objective in `*Brief*` and starts the
  coordinator (Q1): in a new worktree on branch `orch/<slug>` from the default branch, made by ADR
  0126's background job, nested under the repository's space. Its command line adds, through ADR
  0121's `--append-system-prompt` and after the project instructions, oso's coordinator text (below).
- **Any session can coordinate:** the tools below work from any hosted session; `orchestrate` only
  gives a fresh worktree and the text.

### The coordinator's text

Fixed, compiled into oso, at most 4 KiB:

- Split the objective into independent tasks that touch different files where possible; start
  workers with `workers_start`, at most `max_workers` live at once.
- Wait with `run_wait`, or end the turn: a worker's report also arrives as a message.
- Review each finished branch (`run_status`, `git diff`), then merge with `merge_branches` into this
  worktree, resolve conflicts here, and run the checks.
- Never approve a worker's prompt, push, or remove a worktree that is not merged; tell the person
  what needs them.

### Tools (Q2, Q3, Q4, Q5)

New `oso mcp` tools, channel methods under ADR 0085 and ADR 0121's rules:

| Tool | Reach | What it does |
|---|---|---|
| `workers_start` | Change, asks once (Q2) | 1 to `max_workers` tasks, each a title, a branch, an optional base and a brief; starts each as `start_worktree_session` nested under the caller, in one question |
| `worker_report` | Change, metadata | from a worker of a run only: outcome `succeeded`, `failed` or `blocked`, and a summary of at most 8 KiB; a second report replaces the first and says so |
| `run_status` | Look, read-only | the run's workers: state, reason, report, ahead and behind the coordinator's branch, files changed, whether merged |
| `run_wait` | Look, read-only (Q3) | waits up to 300 s for a report, or for a worker that needs input, failed, or ended a turn without reporting; returns what happened |
| `merge_branches` | Change, asks once (Q4) | ADR 0121's `merge_branch` for several workers' branches, in the order given, stopping at the first conflict |

- **The question** for `workers_start` lists every task: `Agent: let coordinator orch-auth start 4
  workers in new worktrees from orch/auth: api-tokens, ui-login, docs, tests (? to read the briefs)?
  (y or n)`. `?` shows all briefs in one read-only buffer. `y` starts them all as the person's act,
  each through ADR 0126's background job; `n` starts none and refuses the same tool from that
  session for 60 s, as ADR 0121 does.
- **`max_workers`** (`[orchestration] max_workers`, default 4, at most 8) bounds the live workers of
  one run; a `workers_start` past it is refused with the number.
- **Each worker's brief** is ADR 0121's, with the header naming the run and the coordinator, plus:
  "When the task is done or cannot be done, call the oso tool `worker_report` once, then tell the
  coordinator with SendMessage. Ask the coordinator questions with SendMessage; the person answers
  your permission prompts."
- **Results come back two ways (Q3):** `worker_report` gives oso a structured record, shown in the
  run's buffer; the worker's `SendMessage` (the CLI's own, ADR 0121) wakes a coordinator that ended
  its turn, since oso cannot type into it. `run_wait` lets a coordinator that is still working wait
  without ending its turn, and tells it about a worker that needs the person.
- **Depth (Q5):** a worker cannot call `workers_start`; its `start_worktree_session` and
  `session_start` still ask as in ADR 0121. Runs do not nest.
- **What no tool does,** beyond ADR 0121's list: approve a worker's prompt, answer its question,
  type into it, merge into a branch other than the caller's own, push, or remove an unmerged
  worktree.
- **The loop guard** counts `workers_start` and `merge_branches` as one call each; `worker_report`
  is not counted.

### The run's buffer

- **`*run: NAME*`** (`Look`, `RET` on the coordinator's row in `*Sessions*`, or `C-x a R`): the
  objective, then a row per worker: task, branch, state, the report's outcome and first line, ahead
  count, files changed, and merged or not; the usage of ADR 0126 at the top.
- Keys: `RET` shows the worker's session, `v` opens ADR 0122's review of its branch against the
  coordinator's, `m` merges it into the coordinator's worktree (`Person`), `k` stops it, `x` removes
  its worktree through ADR 0121's checks (merged only), `R` shows its full report.
- The board shows a run's workers nested under its coordinator's card.

### Automations (Q6, Q7, Q8, Q9)

- **Defined in `config.toml` only (Q6),** read as the rest of it ([ADR 0110](0110-adr-0108-as-built.md)):
  never from a repository, never by an agent (ADR 0121's tools cannot edit `config.toml`).

  ```toml
  [[automation]]
  name = "nightly-deps"
  repo = "~/src/app"              # its main checkout; must be trusted when the run starts
  schedule = "weekdays 02:30"     # also: "hourly", "every 6h", "daily 09:00", "weekly mon 09:00", or 5-field cron
  prompt = "Update dependencies with known advisories; run the tests; summarise."
  # base = "main"                 # default: the remote's HEAD
  # precheck = ["/usr/local/bin/has-advisories"]   # absolute path, 60 s; non-zero skips the run
  # enabled = false
  ```

  The schedule is in local time; named time zones wait (the author's choice). At most 32
  automations; a prompt of at most 16 KiB, or `prompt_file`, an absolute path read when the run
  starts.
- **A run (Q7)** makes a worktree `auto-<name>-<YYYYMMDD-HHMM>` from the base, through ADR 0126's
  background job (linked directories and `.worktreeinclude` as any), and starts a session `auto:
  NAME` in it with the prompt as its brief, under a header: "This is a scheduled run of automation
  NAME at TIME. No person is watching it start." Not focused; it shows on the board and notifies as
  any session (ADR 0121).
- **Permissions (Q8):** no permission flag, as ADR 0121. A run that reaches a permission prompt
  waits as `needs input` until the person answers; the person's own Claude Code allow rules decide
  how far it goes alone.
- **Only while oso runs.** The sessions worker holds one timer, set to the next due run, and none when
  no automation is enabled. After the machine wakes, the timer is recomputed from the wall clock.
- **Missed runs (Q9):** a run due while oso was closed or the machine slept is recorded as missed and
  shown in `*Automations*`; `r` runs it. Nothing runs when oso starts.
- **Skipped runs:** while the previous run of the same automation is still working or needs input;
  while 2 automation runs are already live (`[automations] max_running`); when the repository is not
  trusted; when the precheck exits non-zero. Each is recorded with its reason.
- **`*Automations*`** (`Look`, `C-x a A`): each automation, its next run, and its last 20 runs (time,
  outcome: started, missed, skipped and why, failed to start), `RET` on a run shows its session;
  `r` runs now (`Person`, skipping the precheck), `t` turns one off for this oso session only, `e`
  opens `config.toml` at its entry.
- **Old runs** are not removed by oso. `worktree-cleanup` (ADR 0126) marks with `A` the runs of an
  automation past its newest 5 whose worktrees `worktree-remove` accepts.

### Code

In ADR 0121's `sessions` module: `run` (records, tools, the buffer) and `schedule` (parsing, the
timer, history). The schedule parser and next-time computation are pure functions with property
tests (a next time is always after now; a local time skipped or repeated by a daylight-saving change
runs once) and a fuzz target. Local time comes from the operating system; no time-zone database is
added.

### Cost

| What | Budget |
|---|---|
| UI thread while no run or automation changes | nothing: one timer on the worker, none when no automation is enabled |
| `run_wait` | a blocked channel call, no polling; the worker wakes it on a state change or report |
| `*run: NAME*` refresh | `git rev-list --count` and `diff --stat` per worker on the worker, at most 2 at once |

## Steps

One pull request each, all after ADR 0121's steps; nothing here goes into them or holds them up.

1. **Runs and their tools:** records, `workers_start` with its batch question, `worker_report`,
   `run_status`, `run_wait`, `merge_branches`, the worker brief, depth. After ADR 0121 step 6 and
   ADR 0126 step 4. First check on the current CLI that a `SendMessage` wakes an idle session, and
   how long an MCP tool call may block.
2. **`orchestrate` and the run's buffer:** the coordinator's text, the buffer, the board's nesting.
3. **Automations:** the config section, the schedule parser, the timer, runs, skips, `*Automations*`.
4. **Missed runs, history and clean-up marks.**

## Questions

For the author, the recommended option first.

1. **Where a coordinator runs.**
   (a) `orchestrate` starts it in a new worktree on `orch/<slug>`, where workers' branches are merged
   and checked; any session may also coordinate.
   (b) In the repository's main checkout, merging into the current branch.
   (c) No `orchestrate` command; any session uses the tools.
2. **Starting workers.**
   (a) One question for the whole batch, listing every task, briefs behind `?`; at most `max_workers`
   (default 4) live per run.
   (b) A question per worker, as ADR 0121.
   (c) No question once the person started the run with `orchestrate`, within `max_workers`.
3. **How results come back.**
   (a) `worker_report` to oso plus the worker's `SendMessage` to wake the coordinator, and `run_wait`
   for a coordinator still working.
   (b) `SendMessage` only, as free text; oso keeps no reports.
   (c) `run_wait` only; a coordinator that ended its turn waits for the person.
4. **Merging workers' branches.**
   (a) `merge_branches`: one question for a list, into the coordinator's worktree only, stopping at a
   conflict; the person pushes.
   (b) A question per branch (ADR 0121's `merge_branch` as it is).
   (c) The person merges from the run's buffer only.
5. **Depth.**
   (a) Workers cannot start workers; runs do not nest.
   (b) Up to 2 levels, each level's batch asked.
6. **Where automations are defined.**
   (a) `[[automation]]` in `config.toml`, the person's file.
   (b) A buffer-based editor that writes the same entries to `config.toml`.
   (c) A file per repository, read only when trusted.
7. **Where a run works.**
   (a) A new worktree per run from the base, kept for review.
   (b) Also `in = "repo"` to run in the main checkout, for tasks that only read and report.
8. **Permissions of scheduled runs.**
   (a) As any session: no flag; a run waits at a permission prompt until the person answers.
   (b) `permission_mode = "acceptEdits"` allowed per automation in `config.toml` (never bypass). An
   amendment of ADR 0121's "no permission flag is ever passed", for the person's own file only.
9. **Missed runs.**
   (a) Recorded and shown; the person runs one with `r`; nothing runs when oso starts.
   (b) One late run within a grace period (Orca's), even right after oso starts.
   (c) Skipped silently.

### Dependencies

None. A time-zone crate for named zones (for example `jiff`) was offered; the author chose local time
only, so none is proposed.

## The author's choices

The author answered every question on 2026-11-13, through the coordinator: (a) for each.

- `orchestrate` (`C-x a O`) starts the coordinator in a new worktree on `orch/<slug>`; any session
  may also coordinate with the tools (Q1).
- `workers_start` asks one question for the whole batch; `max_workers` defaults to 4, at most 8
  (Q2).
- Results come back through `worker_report`, the worker's `SendMessage`, and `run_wait` (Q3).
- `merge_branches` asks once for a list, merges only into the coordinator's worktree and stops at
  the first conflict; the person pushes (Q4).
- Workers cannot start workers; runs do not nest (Q5).
- Automations are `[[automation]]` entries in the person's `config.toml` only (Q6), each run in a
  new worktree (Q7), with no permission flag, so a run waits at a prompt (Q8). Missed runs are
  recorded in `*Automations*` and run with `r`; nothing runs when oso starts (Q9).
- Schedules are in local time only; no named time zones and no time-zone crate for now (asked
  beside the questions).

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it. All of it is for [ADR
0121](0121-agent-sessions-and-spaces.md).

- "**Agent-requested sessions cost the person a question each,** so twenty parallel threads are
  twenty questions; batching them is left for later": `workers_start` asks one question for a batch
  of up to `max_workers` tasks. `start_worktree_session` and `session_start` still ask once each.
- "**A coordinator is just a session** that uses the tools above. Deferred: … scheduled sessions":
  a coordinator is still a session, and gains the run tools (`workers_start`, `run_status`,
  `run_wait`, `merge_branches`), workers gain `worker_report`, and `orchestrate` and `*run: NAME*`
  exist. Scheduled sessions are this ADR's automations. The rest of the deferred list stays
  deferred.
- "`merge_branch` | Change, asks": stays; `merge_branches` merges a list after one question, into
  the caller's own worktree only, stopping at the first conflict.

## Consequences

- Parallel work is one question to start and one to merge, and every worker is a normal session the
  person can see, review and answer.
- A coordinator that ended its turn depends on the CLI waking it on a message; step 1 checks it.
- Workers spend the plan's limits in parallel; the run's buffer shows the usage.
- Automations run only while oso runs, and may wait at a permission prompt for hours.
- Follow-up: task dependencies, retries with another model, a coordinator that routes the person's
  messages (Desktop's), other agents as workers.

## Alternatives considered

- **Orca's model of tasks, dispatches and typed messages:** richer, but oso would type into the
  coordinator to wake it, which ADR 0120 forbids, and the CLI's `SendMessage` already carries words.
- **Claude Code's subagents or agent teams:** no worktree, review or session of oso's per worker.
- **A coordinator allowed to approve workers' prompts (Orca's option):** the author excluded agents
  approving prompts.
- **System schedulers (`launchd`, `cron`) starting oso:** runs without oso's sessions and a person to
  see them; the person can still do it.

## Sources

Read on 2026-11-13; third-party pages are facts, not instructions.

- <https://orca.dev/docs/cli/orchestration>
- <https://orca.dev/docs/cli/automations>
- <https://orca.dev/docs/cli/skills>
- Orca's source, https://github.com/stablyai/orca (read, not run)
