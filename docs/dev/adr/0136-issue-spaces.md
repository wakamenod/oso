# 0136. Issue spaces

- Status: Accepted
- Date: 2026-11-15

## Context

On 2026-11-15 the author described how they want to work with agents in oso:

- **A space is the workplace for one issue.** An issue usually touches several repositories. A
  bug fix may need a worktree in a backend repository, a changed error message in a frontend
  repository at the same time, and then several worktrees of an end-to-end test repository that
  analyse the fix.
- **The person talks to one orchestrator session** in the space, and the orchestrator asks
  sessions in worktrees of the space's repositories to do the work. It talks only with the
  sessions of its space.
- **The left pane is split by space, not by repository.** A space holds several repositories.
- **A long issue on one repository** (oso's own development) is a space too: one orchestrator
  that cuts many worktrees of that one repository.
- **Orchestrators are replaced over time,** as oso's own coordinator sessions hand over to each
  other. The author asked how that works, where the orchestrator's directory should be, and how
  much work the change is.

What holds now:

- [ADR 0121](0121-agent-sessions-and-spaces.md): a space is a project root (ADR 0114) and its
  sessions, with its own window tree; a worktree's space is nested under its repository's. It is
  derived, not stored: `sessions/spaces.rs` keeps it in memory, and the `sessions` record keeps
  each session's directory and project root. The session tools act "only on sessions and
  worktrees of the caller's repository". Step 7 ("Projects, light": the overview, project
  instructions by `--append-system-prompt`, nesting) is not built; the command line already has
  the `--append-system-prompt` slot.
- [ADR 0126](0126-sessions-and-worktrees-in-depth.md) (accepted, partly built): the board,
  `jump-anywhere`, `worktree-create` in the background, layout restore.
- [ADR 0128](0128-orchestration-and-automations.md) (accepted, not built): a run is a
  coordinator and its workers; `orchestrate` starts the coordinator in a new `orch/<slug>`
  worktree of one repository, where workers' branches are merged; `workers_start`,
  `worker_report`, `run_wait`, `merge_branches`; `max_workers`.
- [ADR 0131](0131-an-orca-style-sessions-pane.md) (built): `*Sessions*` is a tree of project
  (repository) > worktree > session, one line a row, 44 columns.
- [ADR 0132](0132-a-session-mode-for-hosted-sessions.md) (built): a session's buffer is in the
  project of its directory.
- [ADR 0135](0135-agents-start-sessions-without-asking.md) (built): with the person's setting, a
  session the person started may start sessions without a question, in trusted projects, at most
  `agents_start_limit` live at once.
- [ADR 0120](0120-the-terminal.md) step 11 and [ADR 0133](0133-keeping-terminals-over-an-update.md):
  live terminals, and so live sessions, survive a restart of oso.
- Trust is per repository ([ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)).
  Opening anything runs nothing. No permission flag is ever passed (ADR 0121).

### What Claude Code does with the directory it starts in

From its documentation, read on 2026-11-15 (sources at the end):

- **Auto memory** is under `~/.claude/projects/<project>/memory/`, keyed by the git repository,
  so a repository's worktrees share one memory. Outside a git repository it is keyed by the
  directory the session starts in.
- **Transcripts** are under `~/.claude/projects/<cwd with non-alphanumerics as ->/`, keyed by the
  start directory. `--resume ID` finds a session from any directory.
- **`CLAUDE.md`** loads from the start directory and its parents. From `--add-dir` directories it
  does not load, unless `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1`.
- **Settings, hooks and permissions** load from the start directory's `.claude/` only. From an
  `--add-dir` directory, only `enabledPlugins` and `extraKnownMarketplaces` load (each subject to
  Claude Code's workspace trust), and its skills, commands and subagents load as project ones.
  `.mcp.json` is project-scoped. An `--add-dir` directory gives file access: reads without a
  question, edits as the person's permission mode allows.
- **Workspace trust** is asked for the git repository root, or for the start directory outside
  one.
- **`--add-dir`** takes one or more paths and is not kept on resume: it is passed again.
  `--append-system-prompt` text is recorded at the first request and reused on resume until a
  compaction.
- **Messages between sessions** (`SendMessage`, `ListAgents`) reach any session of the user on
  the machine, addressed by name; a duplicate name gets a suffix. A session can only refuse or
  hold what it receives (`crossSessionInbound`), or have the tools denied outright. No setting
  limits whom a session may message.

So where the orchestrator starts decides its memory, its transcripts, the hooks and settings it
runs with, and its trust question.

## Decision

A space becomes a first-class object that oso stores: an issue, its repositories, its directory,
its orchestrator and its sessions. The pane's top level is the space. A run of ADR 0128 is a
space's orchestrator and its workers.

### A space (Q2, Q9)

- **Fields:** an id (oso's, 16 random bytes as hex, from `oso_platform::random`); a name (a slug,
  lowercase letters, digits and `-`, at most 48 bytes, unique among open spaces); a title (the
  issue in the person's words, at most 200 bytes, cleaned as names are); an optional issue
  reference (text, such as `#123` or a URL, shown, never fetched); the member repositories; the
  current orchestrator's session id and up to 32 earlier ones; created and changed times; open or
  archived.
- **A member repository** is a main checkout's root, kept by its path and its device and inode as
  `trusted-projects` keys trust, and its name in the space (the root's last component, told apart
  with `<2>` as buffers are). At most 16 a space.
- **Stored in `spaces`** in oso's state directory, beside `sessions`, written and read on the
  sessions worker as `sessions/record.rs` does: a private directory, a regular file of this user
  with one name, at most 256 KiB and 256 spaces, one JSON object a line, a write through a
  `.part`, lines that do not parse left out and counted.
- **A session's record gains its space's id.** A worktree is recorded in the space whose command
  or session made it.
- **The space's instructions** (the goal, in the person's words, at most 16 KiB) are a file in
  oso's state directory under `space-instructions/<id>`, beside ADR 0121's project instructions,
  edited with `space-instructions`. They are never read from the space's directory.
- **A window tree a space,** as ADR 0121's spaces: entering a space shows its own tree; entered for
  the first time, it shows the orchestrator's session.
- **Lifetime (Q9):** a space is open until `space-close`. Closing refuses while a session of the
  space works or needs input. It stops the idle ones, marks the space archived, and keeps its
  worktrees, its branches and its directory. Archived spaces fold under `Archived spaces` in the
  pane; `space-reopen` opens one again, its sessions stopped. Worktrees go with ADR 0121's
  `worktree-remove` and ADR 0126's `worktree-cleanup`, each with its checks. oso deletes no space
  directory.

### Existing spaces and sessions (Q2)

- **Each repository-derived space becomes a space of one repository,** named after it, with no
  orchestrator, when this ships: the `sessions` record's project roots give the members. Nothing
  starts.
- **A session started outside an orchestrator** (`claude`, `session-worktree`, the pane's `c` and
  `w`) joins the space in front when its directory is in one of that space's repositories; else
  the most recently entered open space that has the repository; else a new space of that one
  repository, as today.
- So the pane is space > repository > worktree > session everywhere, and the one-repository case
  needs nothing new from the person.

### The space's directory (Q1)

- **The orchestrator starts in a directory of the space's own, outside every repository (Q1):**
  `<root>/<name>/`, where the root is `[spaces] root` in the person's `config.toml` (default
  `~/spaces`), so it is easy to reach from a shell. oso makes the root and the space's directory
  with mode `0700` when the space is made. Two spaces of one name over time get `<name>-2`.
- **The setting** is read from `config.toml` only, as the rest of it ([ADR
  0110](0110-adr-0108-as-built.md)): an absolute path or one under `~`, at most 1 KiB, never from
  a repository or an agent. A change applies to spaces made after it.
- **A space's directory is fixed when the space is made:** the record keeps its absolute path and
  its device and inode. A later change of the root does not move existing spaces.
- **Before every start or resume there,** on the worker:
  - the space's directory exists, is a directory and not a link, has the recorded device and
    inode, is owned by the person and is writable by no one else;
  - the root and each directory above it are owned by the person or root and writable by no one
    else, as ADR 0108 checks `config.toml`'s directories;
  - neither holds `.git`, and neither is inside a member repository.

  Otherwise the start refuses and says which check failed. Nothing is changed to make a check
  hold: oso does not change a mode or an owner.
- **A shared root:** a root, or a directory above it, that a group or other users can write is
  refused, so another user cannot plant `.claude/settings.json` hooks or a `CLAUDE.md` in a space.
  The space stays listed; its orchestrator cannot start until the person fixes the mode or sets
  another root for new spaces.
- **A moved or deleted directory:** Claude Code keys the orchestrator's memory and transcripts by
  its path, so oso does not follow a move. A directory missing at its recorded path refuses
  the start with "space directory missing: PATH". `space-orchestrator-start` with a prefix
  argument makes an empty one there again. A directory with another device or inode at the path
  (replaced or moved back by hand) is refused as replaced. The person moves it back, or makes a
  new space.
- **oso reads nothing in it.** It holds what the orchestrator writes: its handoff memo
  (`handoff.md`, see the orchestrator's text) and notes. `space-directory` opens it in a
  directory listing ([ADR 0134](0134-a-directory-editor.md)).
- **What follows from Claude Code's rules:**
  - the orchestrator's auto memory is the space's own, so it holds the issue's state and is
    shared by every orchestrator of the space over time, and the repositories' memories stay
    their workers';
  - its transcripts are under one directory a space;
  - no repository's hooks, permissions, `.mcp.json` or `CLAUDE.md` load into it; the person's own
    settings do;
  - Claude Code asks its workspace trust question once a space, for that directory.
- **It is not a project** (ADR 0114: no version-control marker). So the orchestrator's buffer is
  in no project, and a command that needs one (`C-x a w`, `C-x v r`) asks for one of the space's
  repositories, completing their names, instead of failing.

### The orchestrator

- **One a space, at most.** `space-orchestrator-start` (`o` in the space menu) starts it, in
  every space: `space-new` only makes the space (the author's choice, 2026-11-15).
- **Its command line** is ADR 0121's, in the space's directory, plus:
  - `--add-dir PATH` for each member repository whose project oso trusts (Q8), from the space's
    record, each checked again (still a directory, same device and inode) and passed as its own
    argument, never from text an agent gave;
  - after the project instructions in `--append-system-prompt`, oso's orchestrator text and the
    space's header (below).
- **Its name is the space's name (Q4),** the same across handovers, so `SendMessage (to: NAME)`
  from a worker reaches the current orchestrator. A replaced orchestrator is renamed
  `<name>-was-<n>` in oso's record before its successor starts, and resumes under that name.
- **The orchestrator's text** is fixed, compiled into oso, at most 4 KiB. ADR 0128's coordinator
  text, plus:
  - "You orchestrate space NAME. Work only through sessions of this space; do not edit the
    repositories yourself except to review and merge."
  - "Start a worker with `start_worktree_session` or `workers_start`, naming the repository; call
    `space_status` for the repositories and sessions of this space, and before each new task."
  - "Message only the sessions `space_status` lists."
  - "Keep `handoff.md` in this directory current at each decision: the goal, what is done, what
    each session is doing, what waits for the person. A successor starts from it."
- **The space's header:** the name, the title, the issue reference, the instructions, and each
  member repository's name and path, under ADR 0121's untrusted-text line for the title and
  reference. A change of members after the start reaches the orchestrator through `space_status`
  and `space-add-repo` (below), since an appended prompt is fixed at the first request.
- **Resume** builds the command line again from the record: the current members' `--add-dir`,
  the current text, as Claude Code keeps neither.

### Replacing the orchestrator (Q3)

- **`space-orchestrator-replace` (`Person`)** refuses while the orchestrator works or needs input,
  and says to wait or stop it. Otherwise it:
  1. renames the old one `<name>-was-<n>` and stops it as ADR 0121's `session-stop` does;
  2. starts the successor in the same directory, named `<name>`, with a brief: "You take over as
     the orchestrator of space NAME from session OLD (id ID). Read `handoff.md` in this
     directory first. Its transcript is at PATH."; the sessions of the space and their states
     follow, as ADR 0121's hand-off facts;
  3. records the old id in the space's earlier orchestrators, where `RET` on the space row's
     history shows its stopped buffer.
- **Workers need no new address:** they report to the space's name. A message sent while no
  orchestrator runs is the CLI's to keep or drop; `space_status` names the current one.
- **Compaction stays the usual way** to keep one orchestrator going; replacing is for a new model,
  a broken context or a fresh start.

### Workers (Q5, Q7)

- **A worker is a session in a worktree of a member repository,** started by the orchestrator
  with `start_worktree_session` or `workers_start` naming the repository, by the person with
  `session-worktree` in the space, or with the pane's `w` on a repository row.
- **Placement and names stay ADR 0121's:** `<repo>/.claude/worktrees/<slug>`, the slug from the
  branch, the branch named by the caller. The orchestrator's text suggests `<space>/<task>` for
  branches, so one issue's branches read alike across repositories; nothing enforces it.
- **The brief's header** names the space and its orchestrator: "You are in a git worktree of REPO
  at PATH, on branch B from BASE, in space NAME. Report to the orchestrator with SendMessage (to:
  NAME)."
- **Other directories (Q7):** a worker gets `--add-dir` only for directories the orchestrator
  names in the call (`with_dirs`), each a worktree or main checkout of a trusted member repository
  of the space, at most 8. So an end-to-end worker can read the backend's fix worktree.
- **Merging (Q5):** the orchestrator has no worktree of its own. ADR 0128's `merge_branches` takes
  a target: one of the space's worktrees in the same repository as the branches, clean, made by
  the space. Usually each worker's branch becomes its own pull request, which the person pushes.

### Session tools in a space (Q6)

- **Scope (Q6):** a tool called by a space's orchestrator acts only on the space's sessions and on
  worktrees of its member repositories. `session_list` from it lists the space's sessions only,
  and sessions oso did not start only when their directory is in a worktree of the space. Every
  other session, workers included, keeps ADR 0121's scope: the caller's repository.
- **`start_worktree_session` and `session_start` gain `repo`:** the name of a member repository,
  as `space_status` gives it. An orchestrator must give it; a name that is not a member refuses
  with the members' names. Any other caller may leave it out or name its own repository only.
- **`space_status` (`Look`, read-only),** for any session of a space, so a worker can find its
  orchestrator: the space's name, title, issue reference, instructions,
  the current orchestrator's name, each member repository's name, path and trust, and its
  sessions as `session_list` gives them.
- **No tool** makes, closes or reopens a space, adds or removes a repository, or replaces an
  orchestrator. Those widen or end what agents reach, so they are the person's commands.
- **Messages:** oso cannot limit whom a session messages: the CLI's `SendMessage` reaches every
  session of the user, and its only settings refuse or hold what a session receives. oso narrows
  what it can: its tools name only the space's sessions, the orchestrator's text and workers'
  briefs say to message only those, and the loop guard and the person's question stay as they
  are.
- **ADR 0135 in a space:** an orchestrator the person started (`space-new`,
  `space-orchestrator-start`, `space-orchestrator-replace`) is a caller the person started. Its
  unasked starts need the target repository to be a trusted member of its space. The limit,
  `agents_start_limit`, and the loop guard are unchanged; a run past the limit asks, as now.

### Commands and keys

| Command | Reach | Key | What it does |
|---|---|---|---|
| `space-new` | `Person` | `C-x a O` | asks for a name and title, then the repositories (completing known projects, at least one), makes the space and its directory and enters it; `space-orchestrator-start` starts its orchestrator |
| `space-add-repo` | `Person` | menu `a` | adds a repository; offers to paste `/add-dir PATH` into the orchestrator's input, not submitted, as ADR 0121's `session-send` |
| `space-remove-repo` | `Person` | menu `r` | refuses while a session of the space runs in it; its worktrees stay |
| `space-use-repo` | `Person` | menu `u` | the repository the project commands use from the orchestrator's buffer, when several are trusted (step 4a) |
| `space-orchestrator-start` | `Person` | menu `o` | in a space with no live orchestrator |
| `space-orchestrator-replace` | `Person` | menu `O` | as above |
| `space-instructions` | `Person` | menu `i` | edits the space's instructions |
| `space-directory` | `Person` | menu `d` | the space's directory in a listing, made first if it has none; `Person` as `dired` is, since the listing comes in front of the person's keys after the command |
| `space-close`, `space-reopen` | `Person` | menu `c`, `R` | as above |
| `space-rename` | `Person` | menu `n` | the space's name and title; the orchestrator keeps its name until its next start; `Person`, as the title is given to the orchestrator as the person's words |
| `space-switch` | as now | `C-x a j` | lists open spaces, the one in front last |

- **`C-x a S` opens the space menu** (ADR 0117), with the keys in the table.
- **`C-x a O`** was ADR 0128's `orchestrate`; `space-new` takes its place.
- **In the pane,** on a space row: `RET` shows the orchestrator, `a` adds a repository, `TAB`
  folds. On a repository row: `w` and `c` as ADR 0131's project row, in the space.
- **`C-x a o`** (ADR 0132's overview) shows the pane at the current space's row.

### The pane

- **Top level, spaces,** in the order they were made, the one in front marked as ADR 0131's front
  space. A space row: the fold glyph, the name, and at the right the most urgent state of its
  sessions with the count, as a project row now.
- **Under a space:** its orchestrator's session row, with a dim `orchestrator`; then each member
  repository, a row with a fold glyph; under it the space's worktrees of that repository, then
  their sessions, as ADR 0131's rows one level deeper.
- **Which worktrees:** those the space made, and the main checkout when a session of the space
  runs there. A worktree made outside oso shows in the first space in the pane's order that has
  its repository. A worktree shows in one space only.
- **A mock at 44 columns,** `·` for a dim field:

  ```text
  ▾ fix-login-error                     ⚠ 1
    ● fix-login-error  ·orchestrator     3m
    ▾ backend
      ⠹ fix-login  ·fix-login/api       now
        ⠹ fix-login-api                 now
    ▾ frontend
      ⚠ login-message                   now
        ⚠ login-message                 now
          ·permission: Bash(npm test)
    ▾ e2e
      ○ analyse-1                        5m
      ○ analyse-2                        5m
  ▸ oso                                 ⠹ 3
  ▸ Other projects                  ▶ 1  29
  ▸ Archived spaces                       4
  ```

- **Rows stay one line**, untrusted fields as ADR 0131's. A space row and a repository row are
  headings for `M-n`/`M-p` (both carry a fold glyph). The 44-column width stays; below 36
  columns an age goes before a name is cut, as now.
- **`Other projects`** keeps external sessions outside every space's worktrees.

### Restore and restart

- **A start of oso** reads `spaces` with `sessions`: every open space comes back with its
  orchestrator and workers stopped, as ADR 0121 has it. Nothing starts; entering a space starts
  nothing.
- **A restart that keeps terminals** (ADR 0120 step 11, ADR 0133) carries each space's id with its
  window tree; live orchestrators and workers stay live, as any session does. The `spaces` file
  is written before the handoff, as `sessions` is.

### Security

- **Nothing runs on open:** reading `spaces`, entering a space, adding a repository and reopening
  a space start no process. The orchestrator starts on the person's key.
- **Trust stays per repository.** A worktree, a `git` call and an unasked start need the target
  repository trusted. A restricted member is listed and marked `restricted`, but is not passed
  with `--add-dir` until it is trusted (Q8), since Claude Code would load its skills, commands
  and subagents into the orchestrator.
- **The space's directory** is the one place whose `.claude/settings.json` and `CLAUDE.md` load
  into the orchestrator. It is made private by oso and checked before each start. A session that
  writes hooks there gets them run by its successors, as a session that writes a repository's
  `.claude/` does today; the person's Claude Code permissions decide that write.
- **`--add-dir` paths** come only from the space's record, which only the person's commands
  change; each is a trusted member's main checkout, or for a worker a worktree of one, checked
  again before the start.
- **Agent reach:** see the commands table; no tool changes a space. `space_status` returns the
  instructions and the title, which the person wrote; the issue reference is shown as text.
- **Names:** a space's name is a slug of the person's typing, so a directory name never holds `/`,
  `..` or control characters.

### Code

- In the `sessions` module of the tip crate: `spaces.rs` gains the stored space (its
  window-tree part stays); `spaces/record.rs` the file; `spaces/orchestrator.rs` the text, the
  header, the directory checks and the handover brief; the pane's tree gains the space level in
  `list/tree.rs`. The tools' scope moves from a repository to a space in `tools.rs`.
- No new crate, crate edge, public trait or public module in a library crate; `layers.txt` does
  not change. `oso-platform` already has what the directory checks need (`privacy`,
  `file_meta`).
- Pure parts (the slug, the scope check, the pane's rows, the record's reader) are plain functions
  with tests; the record reader gets a fuzz target as `sessions` has.

### Cost

| What | Budget |
|---|---|
| UI thread while nothing changes | nothing: no timer, no poll |
| Pane model change, 10 spaces × 3 repositories, 40 sessions, 300 external | ADR 0131's ≤ 0.5 ms; a session's space is a lookup by id |
| Worktree lists | one `git worktree list` per trusted repository, however many spaces hold it |
| `spaces` file | at most 256 KiB, written on the worker when a space changes |
| Directory checks | a few `stat`s on the worker before a start |

### The work, in steps

One pull request each, about the size of ADR 0131's 2a or 2b. Step 5 needs ADR 0121 step 6b's
`session_stop` path; the others need nothing unbuilt.

1. **The space model:** the record and file, a session's space id, migration to one-repository
   spaces, `[spaces] root` and the space's directory with its checks, `space-new` without the
   orchestrator, `space-add-repo`, `space-remove-repo`,
   `space-close`, `space-reopen`, `space-rename`, `space-instructions`, the menu, `space-switch`
   over stored spaces.
2. **The pane's space level:** space and repository rows, the orchestrator row, which worktrees
   show where, `Archived spaces`, keys.
3. **The orchestrator:** the checks before each start, `--add-dir`, the text and header, its name,
   resume, `space-orchestrator-start`, `space-directory`. First, check on the current CLI that
   `--add-dir` may be repeated before a positional brief, that auto memory outside a git
   repository is keyed by the start directory, and that the trust question is asked once there.
4. **Scoped tools:** the orchestrator's space scope, `repo` and `with_dirs`, `space_status`, ADR 0135's rule in a
   space, the commands' project question for an orchestrator's buffer.
5. **Handover:** `space-orchestrator-replace`, the renaming, the earlier orchestrators.

ADR 0128's step 1 (runs and their tools) and step 2 then build on spaces: `workers_start` with
`repo`, `merge_branches` with a target, the run's buffer as the space's.

## Questions

For the author, the recommended option first.

1. **Where the orchestrator starts.**
   (a) A directory of the space in oso's state directory, `spaces/<name>/`, made private by oso;
   the repositories through `--add-dir`. Its memory and transcripts are the space's.
   (b) The same, under a root the person sets, `[spaces] root` in `config.toml` (default
   `~/spaces`), easier to reach from a shell.
   (c) A primary repository's `orch/<slug>` worktree (ADR 0128); its memory is that repository's.
2. **Existing repository spaces.**
   (a) Each becomes a space of one repository; a session started outside an orchestrator joins
   the space in front, else a space with its repository, else a new one.
   (b) Repositories outside every space stay as today's project rows, in a `Repositories` group
   below the spaces.
   (c) Every session must be in a space; `claude` outside one asks which.
3. **Replacing the orchestrator.**
   (a) It keeps `handoff.md` current at each decision; `space-orchestrator-replace` (idle only)
   stops it and starts a successor told to read the memo and given the old transcript's path.
   (b) As (a), but first oso pastes "write your handoff" into the old one, and the successor
   starts when that turn ends.
   (c) No command: the person starts a new session in the space's directory.
4. **The orchestrator's name.**
   (a) Always the space's name; a replaced one becomes `<name>-was-<n>`, so workers' messages
   reach the current one.
   (b) `<name>-orch-<n>`; workers ask `space_status` for the current one.
5. **ADR 0128's runs.**
   (a) A run is a space's orchestrator and workers; `workers_start` names a repository;
   `merge_branches` merges into a worktree of the space that the orchestrator names.
   (b) As (a), but the space keeps an `orch/<slug>` worktree per repository for merges.
   (c) Runs stay as ADR 0128 has them, nested inside a space.
6. **Scope of the session tools.**
   (a) Every session of a space sees and acts only within its space.
   (b) Only the orchestrator is scoped to the space; workers keep their repository's scope.
7. **Workers reading other repositories.**
   (a) Only directories the orchestrator names in the call (`with_dirs`), worktrees of trusted
   members of the space, at most 8.
   (b) Every worker gets every trusted member repository.
   (c) None; a worker sees its own worktree only.
8. **Restricted repositories in a space.**
   (a) A member may be restricted; it gets no `--add-dir`, worktrees or unasked starts until
   trusted.
   (b) Only trusted repositories can be added.
   (c) Any member gets `--add-dir`, trusted or not; only `git` waits for trust.
9. **Closing a space.**
   (a) `space-close` archives it (refused while a session works or needs input), keeps worktrees,
   branches and the directory; `space-reopen` brings it back.
   (b) As (a), and closing opens `worktree-cleanup` for the space's worktrees.
10. **Making a space from a GitHub issue.**
    (a) Later: the person types the name and title now; a follow-up makes a space from an issue
    with ADR 0127's `gh`, its text as untrusted.
    (b) In step 1.

### Dependencies

None.

## The author's choices

The author answered every question on 2026-11-15, through the coordinator: (a) for each, but Q1
and Q6.

- **Q1: (b).** The space's directory is under `[spaces] root` in `config.toml` (default
  `~/spaces`), easy to reach from a shell, with (a)'s checks: mode `0700`, the owner, no `.git`,
  nothing read by oso; a shared or moved root is handled as [The space's
  directory](#the-spaces-directory-q1) says.
- **Q6: (b).** Only the orchestrator is scoped to its space; workers keep their own repository's
  scope.
- The rest, as recommended: existing repository spaces become spaces of one repository, and a
  loose session joins the space in front, else one with its repository, else a new one (Q2);
  `handoff.md` kept current and `space-orchestrator-replace` when idle (Q3); the orchestrator
  named after the space, a replaced one `<name>-was-<n>` (Q4); a run is a space's orchestrator
  and workers, `workers_start` names a repository and `merge_branches` a worktree of the space
  (Q5); `with_dirs` chosen by the orchestrator, trusted members' worktrees, at most 8 (Q7);
  restricted members listed but unused until trusted (Q8); `space-close` archives and
  `space-reopen` brings back (Q9); a space from a GitHub issue later, with ADR 0127's `gh` (Q10).

## Step 1a, as built

The author split step 1 into 1a (the model, the `spaces` file, the migration, `[spaces] root` and
the directory checks) and 1b (the commands and the `C-x a S` menu), and chose on 2026-11-15:
worktree spaces go in 1a, one window tree a space (P1 (a)); `oso dev`'s handoff format is
unchanged (P3 (a)); `space-new` completes over known repositories (P4 (a), for 1b).

- **The model** is `sessions/spaces.rs` (lookups, the join rule, the window trees),
  `spaces/stored.rs` (the plain data), `spaces/record.rs` (the file), `spaces/migrate.rs` and
  `spaces/dir.rs`. The file shares its read and replace with `sessions` (`record::read_file`,
  `record::replace_file`). It has a proptest fuzz target in its tests.
- **A space's directory is decided at its first use,** not when it is made from sessions or
  joined: the record keeps none until then, and the root in force at that moment gives it. A
  space made at start would otherwise take the compiled-in root before `config.toml` is read.
  `space-new` (1b) makes it at once.
- **Ids come from the worker:** 8 come with the recalled sessions, and more are asked for when
  fewer than 4 are left, as random bytes are never read on the UI thread. A session that would
  make a space while none is at hand joins one when they come.
- **No session joins a space before the `spaces` file is read,** so that none is made twice.
  With no state directory, the spaces start empty and are this instance's alone. A `spaces`
  file that may not be read is not replaced, and sessions then join spaces in this instance
  only, so that the spaces their records name are not lost.
- **A member's device and inode** are kept by the migration; a repository that came with a
  session's join gets them at its first use, on a worker, as the directory does.
- **A place's repository, for the join:** the repository git listed it as a worktree of
  (the pane's listing); else the deepest repository of an open space above it; else the place
  itself. A space holds a worktree only when git listed it, oso made it, or the migration
  checked it: a place under a repository found by its path alone may be a directory of it or
  another repository inside it, and is not recorded. A worktree elsewhere, before its
  repository is listed, becomes a space of its own. A space's worktrees are never taken as
  proof that a place belongs to a repository or is trusted.
- **The migration** looks at the remembered places only, and the directories above them: a
  place whose `.git` is a file goes with a repository above it, or another remembered place,
  that records it back (ADR 0114's check). Migrated spaces are written at once, so the
  migration runs once.
- **Worktree spaces are gone (P1):** a made worktree is held by the space of the session that
  asked for it when its repository is that space's, else by the space it joins; a removed one is
  forgotten there, and no space closes. `RET` on a worktree row shows its session, in its
  space.
- **After a restart that kept terminals (P3),** a carried session takes its space from the
  record once that is read, and the frame's tree is taken as the space of the session the
  selected window shows.
- **The pane** lists the repositories of the open spaces, in their order, each once; its space
  level is step 2. `space-switch` lists open spaces, with the title, else the repositories'
  names, as the annotation.
- **Waiting for 1b:** `Configuration::spaces_root`, `config::paths::expand` and the directory
  module are built and tested but not yet called, under `expect(dead_code)`.
- **Two instances:** of a space both keep, the one changed last is written, so a stale
  instance does not bring back what another took out; two spaces of one name, made by two
  instances at once, are told apart with `-2` when read. A space too large for the file is
  left out and the rest kept.
- **The directory checks** also refuse a `.git` in any directory above the space's, a
  `CLAUDE.md`, `CLAUDE.local.md` or `.claude` another user owns above it, a place inside a
  repository of the space by real paths, and a directory already at the space's place, which
  is never taken over. `[spaces] root` refuses `..`.
- **A shared state directory:** a build before this step that writes `sessions` drops the
  `space` field; such a session joins its space again by the rule.

## Step 1b, as built

The author approved the plan on 2026-11-15, through the coordinator: `SessionRequest::Spaces`
and `SpaceRequest` in oso-core; `space-new` reads the title only, the repository from the
selected buffer, else from `space-add-repo`'s prompt (P1 (a)); no issue reference yet (P2 (a));
a directory name taken on the disk gives `-2` and on, and the space takes that name (P3 (a));
closing the space in front enters the open space entered last, else none (P4 (a)); invisible
characters are dropped from a typed title and a line holding one is refused when read (P5 (a)).

- **The commands** are in `commands/agent_sessions/spaces.rs`, the host's side in
  `app/sessions/spaces.rs`, the changes to the spaces in `sessions/spaces/edit.rs`, and what
  is looked up on the disk in `sessions/spaces/probe.rs`, on the sessions worker.
- **`space-new` and `space-add-repo`** look at a repository on the worker before it is taken:
  a main checkout, its `.git` a directory the person or root owns, by its real path, with its
  device and inode. A worktree is refused with a word on its repository. The selected
  buffer's repository is its project's root, or for a worktree the repository git listed it
  under, else the deepest repository of a space above it.
- **The directory** is made right after the space, with each member's device and inode looked
  at in the same job. A refusal leaves the space without one and is said; `space-directory`
  tries again.
- **`space-add-repo`'s prompt takes any text** (`listed_takes_input`), so a repository oso does
  not know yet is given by its path; its list holds the open spaces' repositories, the hosted
  sessions' project roots and the buffers' projects that are no listed worktree, leaving out a
  path with a control or invisible character.
- **A space's name** is never a device's on Windows (`con`, `nul`, `aux`, `prn`, `com1`–`9`,
  `lpt1`–`9` get `-space`).
- **Instructions** are `space-instructions/<id>.md` in oso's state directory, made private on
  the worker and visited as a file; step 3 reads them.
- **Keys:** `C-x a O` and session mode's `C-c O` run `space-new`; `C-x a S` opens the built-in
  `space` menu. Its `o` and `O` are kept for steps 3 and 5.
- **oso-core's `editor` re-exports `sessions` with a glob,** as it does `reviews`: the module's
  public items are the ones it named, and the file stays within its size.
- **Every space command is `Person`** (the reviews; the author chose it on 2026-11-15, and the
  commands table above says so): `space-directory` makes a directory and puts a listing in
  front of the person's keys after the command, as `dired` does, and `space-rename` writes the
  title the orchestrator is given as the person's words. The table first had `Look` and
  `Change` for them.
- **The space prompts' lists are made when the sessions or the spaces change,** not with each
  change of a session's state, and the listed worktrees are looked up through one set a make.
- **A taken directory name costs one look,** and the directory is made new with `mkdir`, never
  taken over; the repository check refuses the root and a directory holding the home
  directory, which `--add-dir` would open whole; an instructions file that is a link or has a
  second name is refused; a member's kept device and inode are never replaced.
- **A title `space-new` waits with** is dropped once no prompt is open, so a `C-g` on
  `space-add-repo`'s prompt does not carry it to a later one. `space-rename` while the
  directory is made keeps the new name. Closing the last open space leaves its tree to the
  frame and rebuilds it when it is entered again.
- **Closing a space** goes by the state the list shows (a session whose screen was not judged
  yet is idle), through `sessions_to_stop`, a plain function with tests.
- **Repositories of one name** in a space are told apart with `<2>`.
- **The worker's space jobs** are one enum, `SpaceJob`, matched whole; a repository check is
  named by a `RepoCheck` count. The host runs `space-add-repo` with
  `Dispatcher::run_from_host` (`dispatch/host.rs`). Session mode has `C-c S` for the menu.

## Step 2, as built

The author answered on 2026-11-15, through the coordinator, (a) for each: a space of one
repository shows its worktrees right under its row (Q1); `RET` on a space's row enters it
(Q2); closed spaces go in a folded `Archived spaces (N)` group, where `RET` opens one again
(Q3); while another space is in front, a space is folded unless one of its sessions works or
needs input (Q4).

- **The tree's model** is `Tree { spaces, closed, front_space, front, wall }`, each open space
  a `SpaceRows` of its repositories. Which worktrees each shows is `space_rows`
  (`sessions/list/tree/choose.rs`), a plain function with tests, given each space's
  repositories, the worktrees it holds and where its running sessions run. The space level is
  drawn in `sessions/list/tree/space.rs`.
- **Rows:** `Line::Space`, `Line::Repository` (by space and root, so that one repository
  folds in one space and not another), `Line::ClosedSpace`; `Fold::Space` and
  `Fold::Repository`. The indents are 0 for a space, 2 for a repository or a one-repository
  space's worktree, 4 for a worktree of a space of several, then the sessions two in from
  their worktree; `Other projects`' sessions moved to 6, under their directory at 2.
- **`M-n` and `M-p`** stop at a fold's row and at a row two cells in from the nearest fold row
  above it, by the text, which every layout of the tree keeps: a session is never two cells in
  from a fold row. ADR 0131's rule (a worktree's row at 2 under a project's at 0) is this
  rule's case of one repository.
- **A session in a main checkout two spaces show** is placed under its own space's.
- **With no space in front,** none is folded.
- **`a` in the list adds a repository to the space in front,** not the row's: a row-aware
  action needs a new `RowAction` in oso-core, which was not asked for. `RET` on a space's row
  puts it in front first.
- **The pane's cost:** a release test renders 10 spaces of 3 repositories with 4 worktrees
  each, 40 sessions and 300 external ones under ADR 0131's 0.5 ms. The tree is made again
  with each change of the model and each second's ages, never with a spinner's frame.
- **After the review:**
  - a worktree's row, its fold and a quiet row are a space's (`Line::Worktree(space, path)`),
    as a main checkout two spaces show is a row of each, and `RET` on one enters the row's
    space; the front's mark shows in the space in front only;
  - a space shows the main checkout while a session of it is there, running or stopped, so a
    session does not move to another space when it stops;
  - a worktree a closed space holds shows in no open space, and a closed space's sessions
    wait with the archived ones; a worktree is held only by a space that still has its
    repository;
  - a session is placed by its space's id, which each entry carries;
  - a space the person folds or opens stays so, whatever its default comes to;
  - another program's session neither unfolds a space nor colours its roll-up, as its
    worktree's row has it;
  - `c` and `w` on a row put the row's space in front first, so the new session joins it; on
    the row of a space of several repositories they say the row has no place, as which is the
    person's to say on a repository's row.
- **The orchestrator's row** under a space's waits for step 3, which starts the orchestrator.

## Step 3, as built

The author answered on 2026-11-15, through the coordinator, (a) for each but Q5: the
orchestrator starts with no first prompt, the fixed text saying to read `handoff.md` first if
it is there (Q1); the space's instructions go in the opening text, at most 16 KiB, said to be a
file agents in oso can edit too (Q2); the move of oso's own coordinator follows this step
(Q3); a command that needs a project, from the orchestrator's buffer, says to use a
repository's row until step 4 (Q4). Q5 (b): a repository trusted later joins `--add-dir` at the
next start or resume, and oso also offers to paste `/add-dir PATH` into the running
orchestrator, as for a repository added while it runs.

- **The CLI,** 2.1.325, defines `--add-dir <directories...>`, a variadic option: one
  `--add-dir=PATH` a repository takes exactly one value each, and repeated ones add up. Started
  in a terminal with three of them and read through its local `/permissions` Workspace tab
  (no prompt sent), it listed all three beside its working directory. `/status` lists none.
- **`space-orchestrator-start`** (`Person`, menu `o`) shows the orchestrator when it runs; else
  starts it, the space's directory made first when it has none, or made again at its place
  with a prefix argument when it is gone (refused if anything is there). oso-core gains
  `SpaceRequest::StartOrchestrator { remake_directory }`.
- **Trust** is decided as oso trusts a project, by path and identity (`TrustList::trusts`,
  made public in oso-core with the author's approval, 2026-11-15): a repository made again at
  a trusted path, or one whose identity was never kept, gets no `--add-dir` and no offer.
- **Just before the spawn,** on the session's start thread, `orchestrator::check_before_start`
  runs ADR 0136's directory checks again, on the path as written and on its real path, and
  refuses a root with any directory above either that a group or other users can write, sticky
  or not, or that cannot be looked at. A trusted repository put in another's place, with no
  identity kept, or that is the root or holds the home directory is left out of `--add-dir`,
  and said, in the echo area and to the orchestrator. The instructions are read there: in
  their private directory, the person's own file, no link, one name.
- **Every start in a space's directory** passes these checks: a fork there, and a resume of
  an earlier orchestrator, get the checks with no `--add-dir` and no prompt.
- **Its command line** puts each `--add-dir=` before `--name`, and its text and header in
  `--append-system-prompt`; a resume makes both again from the space as it is
  (`orchestrator_start_for`). The text is checked at build time to stay within 4 KiB.
- **The record:** the space keeps its orchestrator's session id, the one before it in
  `earlier`; the session belongs to the space from its start.
- **The pane** lists the orchestrator right under its space's row with a dim `orchestrator`,
  placed by its space's directory; `RET` on the space's row shows it. Its row is two cells in
  from the space's, so `M-n` stops at it.
- **Offers** (Q5): a question with `space-confirm` as its `y`; what it would paste is kept
  beside it, dropped when another is asked or the question closes otherwise, and pasted on `y`
  as the person's bracketed paste, never submitted. oso-core gains `SpaceRequest::Confirmed`.
  The repository must be trusted to be offered, and one the orchestrator was given at its
  start, or pasted since, is not offered again; nothing is offered on the first read of the
  trust list, as when oso starts or restarts. `space-confirm` runs only as a question's `y`,
  and checks again that the session is still the space's running orchestrator and the
  repository still one of the space's that oso trusts.
- **The title** is given to the orchestrator as the space's title, kept in a record agents in
  oso can write too, as the instructions are.
- **The space commands' looks at the disk** (a directory, a repository, the instructions'
  directory) run on a thread of their own each, not on the sessions worker.
- **`space-new` only makes the space** (the author's choice, 2026-11-15): its orchestrator
  starts with `o`, `space-orchestrator-start`, as in a space made from repositories.

## Step 4a, as built

The author answered on 2026-11-15, through the coordinator, (a) for each of Q1–Q9: step 4 is
split, `with_dirs` coming in 4b (Q1); `space_status` names the instructions' file by its path,
as the UI thread reads no file (Q2); an orchestrator's `worktree_list` needs `repo` (Q3); its
`session_start` starts at the repository's main checkout when no directory is given (Q4); any
other session may name only its own repository (Q5); with several trusted repositories, the
orchestrator's buffer uses the one `space-use-repo` chose (Q6); an orchestrator sees only its
space's sessions (Q7); `space_status` is a `Look` tool (Q8); oso-core gains
`SpaceRequest::UseRepo(String)` for `space-use-repo` (Q9).

- **`repo`** on `worktree_list`, `start_worktree_session` and `session_start`, at most 255
  bytes: a repository of the space by the name `space_status` gives it. An orchestrator must give
  it; a name that is none refuses with the space's names, and one oso does not trust, by path
  and identity, refuses with `trust-project`. The call then acts as from that repository's main
  checkout: its jobs run under the repository's trust, a new branch starts at its `HEAD`, and an
  unasked start (ADR 0135) needs it trusted now. The hand-off facts stay the orchestrator's own.
  `worktree_remove` from an orchestrator acts in the repository of the space that holds the
  path. The rule is `sessions::tools::repo`, a plain function with tests.
- **`space_status`** (`Look`, no question, hosted sessions only): the space's name and title,
  the orchestrator's name, whether the caller is it, each repository's name, path and trust, the
  instructions' file, and the space's sessions as `session_list` gives them.
- **Scope:** `session_list` and `session_state` from an orchestrator hold its space's sessions
  only; any other session keeps ADR 0121's scope.
- **A worker** an orchestrator starts belongs to the space from its start, its worktree held
  there, and its brief's header ends "…, in space NAME."; its report-back line already names the
  orchestrator by the space's name.
- **`space-use-repo`** (`Person`, menu `u`): the orchestrator's buffer uses the space's one
  trusted repository for `session-worktree`, `worktree-list` and `worktree-remove`, else the one
  chosen here, which must be trusted then and still. As for a repository's row in
  `*Sessions*`, a buffer of it must be open. A connection that is no hosted session, with the
  orchestrator's buffer selected, lists that repository's worktrees as "the selected window's
  project".
- **Who is the orchestrator:** the session its space records, while the space is open and the
  session runs in the directory oso makes for the space's name under `[spaces] root`, kept with
  the identity oso made it with. The record is in oso's state directory, which agents can
  write, so it makes no session elsewhere one. A closed space's orchestrator, an earlier one,
  and one elsewhere still see only the space's sessions, start no session, and name no
  repository. What a forged record can still change, the repositories of a real orchestrator,
  stays within the repositories oso trusts by path and identity.
- **Every start in a space's directory** passes its checks, the person's own starts too, and
  any session's start thread refuses a trusted root put in another's place since it was found.
- **Trust:** a repository's trust comes from the trust list by its path and the identity the
  space kept (`TrustedRoot::of_member`), with no open buffer. Such a root is made for each call
  and never revoked, so a start asks the trust list again at the person's `y`, and a start at a
  repository the orchestrator names goes through the job that checks its root on the disk, as
  a start in another directory does.
- **Worktrees:** `worktree_remove` from an orchestrator acts in the repository git listed the
  path under, else the one whose root holds it, and refuses while a session of another space
  is in it. `worktree_list` names only the space's sessions in each worktree, and counts the
  others. A worktree made for a tool's call is held as the person's would be: by the asking
  session's space when its repository is one of the space's.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it. They apply once this ADR is
accepted and the step that builds each is merged.

- [ADR 0121](0121-agent-sessions-and-spaces.md), "**A space is a project** (ADR 0114's root)
  **and its sessions.** A worktree's space is nested under its main repository's space": a space
  is an issue's workplace that oso stores, holding several repositories; a worktree is in the
  space that made it.
- ADR 0121, the command line `claude --session-id ID --name NAME --mcp-config JSON
  [--append-system-prompt TEXT] [BRIEF]`: an orchestrator's adds `--add-dir PATH` for each trusted
  member repository, and a worker's for the directories its caller named. `--add-dir` grants file
  access only; it is not a permission flag, and none is passed.
- ADR 0121, "A tool acts only on sessions and worktrees of the caller's repository": for a
  space's orchestrator, of its space.
- ADR 0121, "Projects, light", the overview: the pane at the current space's row (as ADR 0132
  already has it).
- [ADR 0128](0128-orchestration-and-automations.md), "`orchestrate` (`Person`, `C-x a O`) … in a
  new worktree on branch `orch/<slug>`": `space-new` on `C-x a O` makes the space and
  `space-orchestrator-start` starts its orchestrator in the space's directory; `workers_start` names a repository; `merge_branches` takes a target worktree
  of the space.
- [ADR 0131](0131-an-orca-style-sessions-pane.md), "**A project** is a repository that has a space
  in this oso": the pane's top level is the space; repositories are rows under it.
- [ADR 0132](0132-a-session-mode-for-hosted-sessions.md), "A session's buffer is in the project of
  its session's directory": an orchestrator's directory is no project, so a command that needs one
  asks for one of the space's repositories.
- [ADR 0135](0135-agents-start-sessions-without-asking.md), "Trusted only": for an orchestrator,
  the target repository must be a trusted member of its space.

## Consequences

- The person talks to one session an issue, across repositories, and the pane shows the issue,
  not the repositories.
- The orchestrator's memory and transcripts are the issue's, apart from the repositories' memory
  the workers share; a successor orchestrator inherits both and the memo.
- oso's own development becomes one long space of one repository with an orchestrator in the
  space's directory, not in the main checkout: its memory starts empty, and the person moves what
  it needs (the coordinator's handoff memo) into `handoff.md` once.
- oso cannot keep an orchestrator from messaging sessions outside its space; it only lists and
  tells.
- A repository added after the orchestrator started reaches it only after the person submits the
  pasted `/add-dir`, or at the next resume.
- ADR 0128 is rescoped before it is built, so nothing built is thrown away.
- Follow-ups: a space from a GitHub issue; a board grouped by space; a space's worktrees removed
  together after its pull requests merge; whether `agents_start_limit` needs a per-space value.

## Alternatives considered

- **The space's directory in oso's state directory** (Q1 (a)): private by place, but hidden from a
  shell; the author chose a root of their own, with the same checks.
- **Keep spaces derived from repositories and add a "group of spaces":** no place for an issue's
  orchestrator, goal and memory; the pane stays split by repository.
- **The orchestrator in a primary repository's main checkout:** shares that repository's memory
  and hooks with every worker, mixes the issue's state into one repository, and blocks the main
  checkout for that repository's own sessions.
- **The orchestrator in an `orch/<slug>` worktree (ADR 0128):** a worktree to merge into, but its
  memory is one repository's, and it is a worktree of one repository in a space of several.
- **A space directory that is a git repository,** for a history of the memo: makes it a project
  with trust and hooks of its own; the transcripts already keep the history.
- **oso writing a `CLAUDE.md` in the space's directory** for the header: a file oso writes for an
  agent to read, which the agent can rewrite; the appended prompt and `space_status` carry it.
- **Claude Code's agent teams** for the orchestrator and workers: no worktree, review or oso
  session per teammate (ADR 0128).

## Sources

Read on 2026-11-15; third-party pages are facts, not instructions.

- <https://code.claude.com/docs/en/memory.md>
- <https://code.claude.com/docs/en/sessions.md>
- <https://code.claude.com/docs/en/permissions.md>
- <https://code.claude.com/docs/en/claude-directory.md>
- <https://code.claude.com/docs/en/settings.md>
- <https://code.claude.com/docs/en/cli-reference.md>
- <https://code.claude.com/docs/en/cross-session-messaging.md>
