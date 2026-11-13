# 0126. Sessions and worktrees in depth

- Status: Accepted
- Date: 2026-11-13

## Context

[ADR 0121](0121-agent-sessions-and-spaces.md) decided hosted `claude` sessions, their states, the
`*Sessions*` side window, spaces, worktree sessions and the agents' session tools. On 2026-11-13 the
author chose more from Orca's agent development environment, to make oso their daily place for
parallel agents:

- **Sessions:** a board of sessions by state (Needs You / Working / Done); a jump across worktrees
  and their buffers; Claude's rate-limit use and reset times in the modeline; "a UI for parallel
  worktrees", in their words.
- **Worktrees:** a create dialog (a name, a base that is a branch, a commit, a remote branch or a pull
  request, the agent to start, made in the background with progress and a retry); each worktree's
  layout restored; removal and clean-up with confirmation, warnings for unmerged branches, and a
  view of each worktree's size and git state; shared files for new worktrees (linked ignored
  directories such as `node_modules`, and `.worktreeinclude` copies).

Orchestration and scheduled sessions are [ADR 0128](0128-orchestration-and-automations.md); pull
requests and issues are [ADR 0127](0127-source-control-and-github.md), which this ADR shows per
worktree.

What holds already: ADR 0121's rules (no permission flag; the screen is the state's authority; only
a person types into a terminal; `git` only in trusted projects, on the sessions worker, through
`oso-platform::process`); [ADR 0120](0120-the-terminal.md)'s terminals and handoff; [ADR
0117](0117-menus.md)'s menus; [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md)'s
floats; oso has no network code.

### What Orca does

From its documentation and source (sources at the end):

- **The board** (experimental): columns Needs You, Working and Done, and an optional Idle column for
  agents silent about 30 minutes. A card shows the agent, the conversation's name, a state glyph,
  the last message, the worktree, the age and the pull request's state; a click shows the live
  terminal. State comes from the terminal's title and from hooks Orca writes into Claude Code's
  settings, which post to a local port.
- **`Cmd-J`** searches worktrees (hidden ones too), pull request titles and numbers, and every open
  tab. With no query it lists the 6 recent sessions, needs-you first, on `Cmd-1`…`Cmd-6`;
  `Shift-Enter` opens a worktree in a new split; no match offers "Create worktree" with the typed
  name.
- **Usage:** Claude Code's status line input (`rate_limits.five_hour` and `seven_day`, each a used
  percentage and a reset time) through a status line Orca installs only when the person has none;
  as a fallback, Anthropic's OAuth usage endpoint with Claude's own token every 15 minutes. It shows
  the use, the reset times and a warning past 80%.
- **Creating a worktree:** a name that may link a pull request or an issue; "branch from" the base,
  a local branch, a commit or a remote branch (fetched); the agent to start, or a blank terminal.
  The dialog closes at once; `git fetch` and `git worktree add` run in the background with a
  progress row; a failure shows the error and Retry. A setup script from the repository runs only
  after the person approves its contents.
- **Layout restore:** per worktree, the split tree, the focused tab and terminal scrollback; agents
  keep running across quitting, since a daemon owns the terminals.
- **Removal:** a confirmation, then the directory and the branch; Force Delete when git refuses;
  branches that may hold unmerged commits are kept, and a "Review N branches" note offers to delete
  them. A resource view lists each worktree's status, activity, size and git state.
- **Shared files:** `orca.yaml`'s `worktree.sharedDirectories` links existing ignored directories
  from the main checkout; `.worktreeinclude` copies literal paths (APFS clones where possible), at
  most 2 GiB and 50,000 entries a worktree.
- **Parallel worktrees:** no view of its own; the documentation suggests dragging tabs into splits
  and watching the board.

## Decision

### The board (Q1)

- **`session-board` (`Look`, `C-x a B`)** shows `*Sessions*` as columns in the selected window, and
  `B` in `*Sessions*` toggles the same. Columns, most urgent first:

  | Column | ADR 0121 states |
  |---|---|
  | Needs you | needs input, failed |
  | Working | working |
  | Done | ready for review, done |
  | Idle | idle, and stopped when `S` shows them |

- **A card** is 3 rows: the state mark, the name and the age; the branch and its pull request's state
  (ADR 0127's per-worktree read, trusted projects only); the recap (ADR 0121's, under the same rule for
  agents). A worktree being created shows as a card with its progress (below).
- **Keys:** `n`/`p` within a column, `f`/`b` across, `RET` shows the session, and the list's keys
  (`r`, `k`, `P`, `A`, `e`, `w`) act on the card at point.
- **Width:** each column at least 28 cells; a window narrower than four columns drops Idle, then
  falls back to the list. The board is the same buffer, so it costs the list's update budget.

### The jump (Q2)

- **`jump-anywhere` (`Look`, `C-x a j`, Q2)** is a completion float over every space and what is in
  it: linked worktrees (with no space yet, too), their sessions, terminals, file buffers, reviews and
  status buffers, each row naming its space and branch, and the branch's pull request number and title
  where ADR 0127 has read them. ADR 0121's `space-switch` stays, as this list filtered to spaces.
- **With no input,** the 6 most recent sessions, needs input first, on `M-1`…`M-6`.
- **`RET`** enters the space and shows the buffer; `C-RET` shows it in a new split instead; no match
  offers "create worktree NAME" (`C-RET` on the offer), which opens the create menu with the name.
- Matching is the completion float's, on its worker past 5,000 candidates ([ADR
  0096](0096-completion-on-a-worker-past-5000-candidates.md)).

### Usage and limits (Q3)

- **Where the numbers come from:** Claude Code's status line input carries `rate_limits.five_hour`
  and `rate_limits.seven_day`, each with `used_percentage` (0–100) and `resets_at` (Unix seconds),
  for Pro and Max subscribers after the first reply; and `context_window.used_percentage` per
  session. No other documented source exists (`/usage` is interactive only). oso reads no OAuth token
  and calls no endpoint.
- **How (Q3):** when `[sessions] usage = true`, a session oso starts gets, through ADR 0121's
  `--settings`, a `statusLine` whose command is oso's own executable: `oso statusline`. It reads the
  JSON from standard input (at most 64 KiB), keeps only the numbers above and the session id, writes
  them to `usage/<session id>` in oso's state directory (mode `0600`, written to a temporary name and
  renamed), and then:
  - runs the person's own status line command, if `[sessions] statusline` names one by absolute
    path, giving it the same input and printing its output, so the person keeps their status line;
  - or prints a short line of its own (the model and the context use).
  It opens no socket and starts nothing else. Claude Code runs a status line only after its own
  workspace trust, so the numbers appear only there.
- **The sessions worker** `stat`s `usage/` with its registry poll (once a second) and reads a file
  only when it changed. The account's limits are the newest report of any session.
- **The modeline** of the selected window shows `5h 42% ↻14:00 · 7d 18%` while a report is less than
  6 hours old: the warning face from 80%, the error face from 95%; nothing for API-key users, whose
  input has no `rate_limits`. A session's own modeline shows `ctx 63%`.
- **Nothing is written into Claude Code's settings files.** The status line exists only for sessions
  oso starts, and only when the person turns it on.

### Parallel worktrees (Q4)

- **`*Sessions*` groups by worktree** too: `s` cycles state, space, worktree. A worktree row shows
  its branch, ahead and behind its base, dirty and untracked counts, its pull request and checks,
  and its sessions nested.
- **`session-grid` (`Look`, `C-x a g`)** shows up to 6 sessions' terminals of the current repository
  side by side, in a window tree of its own (2 × 1 up to 3 × 2 by the frame's size), needs input
  first, then working. It is a space's window tree like any other: `C-x a z` zooms one tile, `C-x a
  g` again returns to the space shown before. A tile is a window on the session's terminal buffer; a
  terminal shown in two windows draws once per window from the same snapshot (ADR 0120).
- The grid is ADR 0121's deferred "preset layouts", limited to this one.

### Creating a worktree (Q5, Q6, Q7)

- **`worktree-create` (`Person`, `C-x a w`)** opens a menu (ADR 0117) with options that take values,
  shown in the menu as transient shows them, each with a default so that `RET` alone works:
  - `n` name (the slug of ADR 0121 makes the directory; default: asked first);
  - `b` branch to create (default: the name);
  - `f` from (Q5): the default branch (the remote's `HEAD`, else `main`), a local branch, a remote
    branch, a revision, a pull request (`#123`) or an issue (`i45`), completed from local refs and,
    for `#` and `i`, from ADR 0127's lists;
  - `a` agent (Q6): `claude` with a brief (default), `claude` with no brief, a plain terminal, or
    nothing;
  - `p` parent space (default: the repository's, or the session's that is selected).
  `RET` asks for the brief in `*Brief*` when the agent takes one, as ADR 0121 does, then closes.
  ADR 0121's `session-worktree` becomes this menu with its defaults.
- **A pull request** (Q5) whose head is in the same repository fetches that branch and creates a
  tracking branch of the same name; one from a fork creates `pr-123` at `refs/pull/123/head`,
  fetched into `FETCH_HEAD` (ADR 0122's fetch), with no upstream. **An issue** names the branch
  `issue-45-<slug of its title>` and prefills the brief with the issue's text under ADR 0127's
  untrusted-text line.
- **It runs in the background (Q7),** as a job of the sessions worker, in steps, each shown on a
  progress row in `*Sessions*` and the board: fetch (only when the base is remote, a pull request,
  or not yet local), `git worktree add`, shared links, `.worktreeinclude` copies with a count, the
  space, the session. Fetch and `git worktree add` have no 10 s limit (a large checkout and its
  hooks take longer); they show the elapsed time, and `k` on the row cancels.
- **A failed step** keeps the row with git's message: `r` retries from that step, `k` removes what
  was made (the worktree only if this job made it and nothing was written in it since; the branch only
  if this job created it and it has no commits of its own), `RET` shows the full output.
- **No setup script runs.** oso runs nothing a repository names when a worktree is made, beyond git's
  own hooks in a trusted project. The person's own `[worktrees] after_create` command, in
  `config.toml`, is run in the new worktree's project terminal (ADR 0120) when set, so it shows and
  can be stopped; it is never read from the repository.

### Shared files (Q8)

- **Linked directories (Q8):** `[worktrees] link = ["node_modules", ".venv"]` in `config.toml`, for
  all repositories or per repository (`[worktrees."~/src/app"]`). Each entry is a relative path, no
  `..`, not `.git`; it is linked only when, in the main checkout, it exists, is a directory, and git
  ignores it. The new worktree gets a symbolic link to the main checkout's directory, unless that
  path already exists there. At most 100 entries.
- **Copies:** `.worktreeinclude` as ADR 0121 decided (gitignore patterns, ignored files only, regular
  files, at most 1,000 files and 256 MiB). On macOS a copy is a `clonefile` where the volume allows
  it, so a large copy costs no space; else a plain copy.
- **The removal question** names linked directories as links: removing the worktree removes the link,
  never the main checkout's directory. A check before `git worktree remove` makes sure of it.
- A shared `target/` for Rust is not recommended (Cargo's lock serialises builds across worktrees);
  the manual says so.

### Layout restore (Q9)

- **Each space's window tree is saved** with ADR 0121's `sessions` file: the splits and their sizes,
  and in each window a file buffer by its path and line, a session's terminal by its id, a project
  terminal by its project, a review or status buffer by its scope. No text, no scrollback.
- **When oso starts,** spaces are restored with their trees when first entered: files are opened
  (opening runs nothing), sessions come back stopped as ADR 0121 has them, project terminals show
  `stopped: RET to start`. Nothing starts by itself.
- `oso dev`'s restart keeps live terminals through ADR 0120's handoff, so the tree comes back with
  them running.

### Removing and cleaning up (Q10, Q11)

- **`worktree-cleanup` (`Person`, `C-x a k`)** is `worktree-list` (ADR 0121) as a buffer with marks,
  as a directory editor's: each row adds the **size** on disk (counted on the worker, links not
  followed, `target/` and other ignored directories shown apart), the **last activity** (the newest
  of the index, `HEAD`'s reflog and its sessions' last change of state), the pull request and its
  state (ADR 0127), and whether `worktree-remove` would accept it. `d` marks, `u` unmarks, `x`
  removes the marked ones after one question that lists them; `D` marks every row that
  `worktree-remove` accepts and whose pull request is merged or closed.
- **Unmerged and dirty worktrees (Q10):** ADR 0121's checks stay for agents and for the plain
  `worktree-remove`. The person may remove one that fails check 2 (uncommitted changes) or 5 (not
  merged) only with `C-u x` (or `C-u M-x worktree-remove`), after a question that lists the
  uncommitted files and the commits only that branch holds, and answered by typing `yes`. Before it,
  the uncommitted state is kept as a commit object with `git stash create` (tracked changes) and
  `git hash-object -w` (untracked files of at most 1 MiB, up to 1,000), listed in `*git discarded*`
  (ADR 0127). Checks 1, 3 and 4 (the main worktree, unsaved buffers, a busy session) still refuse.
  Then `git worktree remove --force PATH`, the only `--force` oso passes.
- **The branch (Q11):** the removal question offers `b` to delete the branch too: `git branch -d`,
  which git refuses for an unmerged branch; then oso says so, keeps the branch, and lists it under
  "kept branches" in `worktree-cleanup`, where `C-u d` on it runs `git branch -D` after naming the
  commits it would lose. An agent never deletes a branch (ADR 0121).

### Code

All in ADR 0121's `sessions` module of the tip crate; `oso statusline` is a subcommand of oso's own
executable beside `oso mcp` and `oso hook`. The board and the grid use `oso-core`'s window tree and
ADR 0121's frame additions; nothing new is public in a library crate. Pure parts (the board's
layout, the usage file's reader, the link checks) are plain functions with tests; the usage reader
gets a fuzz target.

### Cost

| What | Budget |
|---|---|
| UI thread per board update | the list's: ≤ 0.5 ms per state change |
| Usage | one `stat` a second with the registry; a file of at most 4 KiB read on change |
| `oso statusline` | ≤ 5 ms of its own before the person's command; no network, no socket |
| Size of a worktree | on the worker, at most 1 s a worktree, then shown as "≥ N" |
| `jump-anywhere` | the completion float's budgets |

## Steps

One pull request each, after ADR 0121's steps they build on.

**How this lands beside ADR 0121.** ADR 0121's steps are built as that ADR decides, and nothing in
this ADR goes into them or waits on it: step 3 builds `*Sessions*`, spaces and their window trees,
`C-x a j` as `space-switch`; step 4 builds `session-worktree` with its question, the 10 s limit and
`worktree-remove` without `--force`. Every change below comes after, in this ADR's own pull
requests, and replaces or extends what those steps built: `C-x a j` and `C-x a w` are rebound, the
limit is lifted for creating, `--force` and branch deletion are added for the person. The only
thing to keep in mind while building ADR 0121 is that its window trees and `session-worktree` are
extended later, not thrown away.

1. **The board and grouping by worktree** (after 0121 steps 3 and 4: the rows need `worktree-list`).
2. **The jump** (after 0121 step 3).
3. **Usage:** `oso statusline`, the usage files, the modeline segment (after 0121 step 1, which
   passes `--settings`).
4. **Creating worktrees:** the menu, bases, pull requests and issues (with ADR 0127 step 6), the
   background job and retry, `after_create`, linked directories and clones (after 0121 step 4).
5. **Removing and cleaning up:** `worktree-cleanup`, sizes, forced removal with its copies, kept
   branches (after 0121 step 4).
6. **Layout restore and the grid** (after 0121 step 3).

## Questions

For the author, the recommended option first.

1. **The board.**
   (a) Four columns: Needs you (needs input, failed), Working, Done (ready for review, done), Idle;
   a card of 3 rows; the same buffer as `*Sessions*`, toggled with `B`.
   (b) Three columns as Orca's, idle sessions hidden.
   (c) No board; `*Sessions*` grouped by state is enough.
2. **The jump.**
   (a) `C-x a j` becomes `jump-anywhere` over worktrees, sessions, terminals, files, reviews and
   pull requests; `space-switch` stays as its spaces-only filter; `M-1`…`M-6` for recent sessions.
   (b) A separate key (`s-j`, as Orca's `Cmd-J`), `C-x a j` kept as `space-switch`.
3. **Usage and limits.**
   (a) Opt in with `[sessions] usage = true`: oso's status line for sessions it starts, which records
   the numbers and then runs the person's own status line command named in `[sessions] statusline`.
   (b) No `--settings`: the person adds `oso statusline --tee` to their own status line in Claude
   Code's settings by hand.
   (c) No usage display.
4. **"A UI for parallel worktrees".**
   (a) `*Sessions*` grouped by worktree with git and pull request state, and `session-grid` with up
   to 6 sessions' terminals side by side.
   (b) Grouping by worktree only.
   (c) The grid only.
5. **Bases a worktree can start from.**
   (a) The default branch, a local branch, a remote branch, a revision, a pull request, an issue.
   (b) Without issues.
   (c) Branches and revisions only; pull requests later.
6. **Agents the create menu offers.**
   (a) `claude` with a brief, `claude` without one, a plain terminal, nothing. Other agents' CLIs
   wait for ADR 0031's adapters.
   (b) `claude` with a brief only, as ADR 0121.
7. **Creating in the background.**
   (a) A job with steps, progress on a row, retry from the failed step, cancel, and clean-up of only
   what the job made.
   (b) In the foreground with a progress echo; a failure leaves what was made for the person.
8. **Where linked directories are named.**
   (a) In the person's `config.toml`, for all repositories or per repository; only existing ignored
   directories are linked.
   (b) In a file in the repository (as `orca.yaml`), read only in trusted projects. Shared with the
   team, but a repository decides what links into its worktrees.
   (c) Both.
9. **Layout restore.**
   (a) Window trees saved per space with buffers by path, sessions by id; restored when a space is
   first entered, nothing started.
   (b) Also restore when oso starts, all spaces at once.
   (c) No restore across restarts.
10. **Removing a worktree that is dirty or not merged.**
   (a) Only by the person, with `C-u`, after typing `yes` to a question that lists what is lost; the
   uncommitted state kept as unreachable objects first; agents and the plain command still refuse.
   (b) As ADR 0121: never; the person cleans it by hand first.
   (c) A plain `y` instead of typing `yes`.
11. **The branch when a worktree is removed.**
   (a) Offered: `git branch -d`; an unmerged branch is kept and listed, and deleted with `-D` only
   from `worktree-cleanup` after naming its commits.
   (b) Never deleted, as ADR 0121.
   (c) Deleted with the worktree, as Orca, `-d` only.

### Dependencies

None. `clonefile` is a libc call on macOS through the `libc` crate oso already uses.

## The author's choices

The author answered every question on 2026-11-13, through the coordinator: (a) for each.

- The board: four columns, cards of 3 rows, the same buffer as `*Sessions*` toggled with `B`, and
  `C-x a B` (Q1).
- `C-x a j` is `jump-anywhere`, with `space-switch` as its spaces-only filter; with no input the 6
  recent sessions on `M-1`…`M-6`; `C-RET` in a split; no match offers "create worktree NAME" (Q2).
- Usage by opt-in `[sessions] usage = true`: oso's status line passed with `--settings`, which
  records the numbers and then runs the person's own status line command; nothing written into
  Claude Code's settings files (Q3).
- `*Sessions*` grouped by worktree, and `session-grid` (`C-x a g`, up to 6, needs input first) with
  `C-x a z` to zoom (Q4).
- Every base, pull requests and issues too: a same-repository pull request as a tracking branch, a
  fork's as `pr-123`; an issue as `issue-45-<slug>` with the brief prefilled (Q5).
- The create menu offers `claude` with a brief, `claude` without one, a plain terminal, or nothing
  (Q6).
- Creating is a background job in steps: `r` retries from the failed step, `k` cancels and removes
  only what the job made; fetch and `git worktree add` have no 10 s limit (Q7).
- Linked directories are named in the person's `config.toml`, for all repositories or per
  repository, and only existing ignored directories are linked; `.worktreeinclude` copies are APFS
  clones where the volume allows (Q8).
- Each space's window tree is restored when the space is first entered; sessions come back stopped
  (Q9).
- A dirty or unmerged worktree is removed only by the person, with `C-u x` in `worktree-cleanup` (or
  `C-u M-x worktree-remove`) after typing `yes`; the uncommitted state is kept as unreachable objects
  for git's two weeks; agents and the plain command still refuse (Q10).
- The removal question offers `git branch -d`; an unmerged branch is kept and listed, and `-D` runs
  only from `worktree-cleanup` after naming its commits; agents never delete a branch (Q11).

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it. All of it is for [ADR
0121](0121-agent-sessions-and-spaces.md), and none of it changes what ADR 0121's own steps build
(see Steps).

- "Every `git` and `gh` call runs on a worker, through `oso-platform::process`, with
  `GIT_TERMINAL_PROMPT=0`, a time limit of 10 s each, and nothing fetched": `worktree-create` may
  fetch its base (a remote branch, a pull request, a revision not yet local), and its fetch and `git
  worktree add` have no time limit; they show the elapsed time and `k` cancels them. Every other
  call keeps the rule.
- "`session-worktree` (`Person`, `C-x a w`) asks for a branch (completing local branches; a new one
  starts at `HEAD`, or at a base given with a prefix argument) and a brief": `C-x a w` is
  `worktree-create`, a menu whose defaults make the same worktree with `RET`; `session-worktree` is
  that menu with its defaults. Creating runs as a background job with retry and cancel.
- "Then it stops the idle sessions in it, closes its space, and runs `git worktree remove PATH`,
  never `--force`. **It never deletes the branch.**": still so for agents and for the plain
  `worktree-remove`. The person's `C-u x` in `worktree-cleanup`, or `C-u M-x worktree-remove`, may
  remove a worktree that fails check 2 or 5, after typing `yes`, with `git worktree remove --force
  PATH`, the uncommitted state kept as unreachable objects first. The removal question offers `git
  branch -d`; `git branch -D` runs only from `worktree-cleanup` after naming the commits it loses.
- "`C-x a j` `space-switch`": `C-x a j` is `jump-anywhere`; `space-switch` stays as a command, the
  jump filtered to spaces. New keys: `C-x a B` the board, `C-x a g` the grid, `C-x a k`
  `worktree-cleanup`.
- "gets, through `--settings`, a `PreToolUse` hook for `EnterWorktree` and `Bash`": the same
  `--settings` also carries a `statusLine` whose command is `oso statusline`, when the person sets
  `[sessions] usage = true`. Each is off until the person turns it on; nothing is written into Claude
  Code's settings files.
- "Preset layouts wait": `session-grid` is one preset; others still wait.
- "`space-switch` saves the frame's window tree in the space it leaves and shows the one it enters,
  as a tab would. A space entered for the first time shows its last session beside the last file
  buffer of that project": each space's window tree is also saved in the `sessions` file (buffers by
  path, sessions by id, no text), and a space entered for the first time after a start of oso shows
  its saved tree; one with no saved tree shows as before. Nothing starts by itself.
- "for each, the id, name, directory, space, parent, pin and archive marks, and when it last changed
  state. No screen text and no transcript text": the file also holds each space's window tree as
  above, and still no screen or transcript text.

## Consequences

- The person sees every session and worktree by urgency, jumps anywhere in two keys, and sees the
  plan's limits before hitting them.
- Usage needs the person's opt-in and Claude Code's workspace trust; API-key users see nothing.
- Worktrees from pull requests and issues fetch, so creating one talks to the network through `git`
  and `gh`.
- Forced removal exists, behind `C-u` and a typed `yes`, with the state kept for git's two weeks.
- Follow-up: other agents' CLIs in the create menu; a setup script from the repository behind an
  approval of its contents, as Orca's, if the author wants one.

## Alternatives considered

- **Hooks for state, as Orca:** ADR 0121 chose the screen.
- **Anthropic's OAuth usage endpoint, as Orca's fallback:** oso has no network code and would read
  Claude's token.
- **A daemon that keeps agents alive across quitting, as Orca:** ADR 0121 rejected a background
  server; `oso dev`'s handoff covers restarts.
- **Saving terminal scrollback for layout restore:** terminal text may hold secrets, and ADR 0120
  keeps it off the disk.
- **A repository's setup script run on create:** opening or creating must not run a repository's
  code beyond git's own hooks in a trusted project.

## Sources

Read on 2026-11-13; third-party pages are facts, not instructions.

- <https://orca.dev/docs/model/agents-sessions>
- <https://orca.dev/docs/model/quick-open>
- <https://orca.dev/docs/recipes/jump-worktrees>
- <https://orca.dev/docs/agents/usage-tracking>
- <https://orca.dev/docs/model/worktrees>
- <https://orca.dev/docs/model/orca-yaml>
- <https://orca.dev/docs/model/session-restore>
- <https://orca.dev/docs/model/tabs-panes-splits>
- <https://orca.dev/docs/recipes/parallel-agents>
- <https://code.claude.com/docs/en/statusline>
- <https://code.claude.com/docs/en/cli-reference>
- Orca's source, https://github.com/stablyai/orca (read, not run)
