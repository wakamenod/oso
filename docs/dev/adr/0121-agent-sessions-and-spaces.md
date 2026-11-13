# 0121. Agent sessions and spaces

- Status: Accepted, amended by 0126 and 0128
- Date: 2026-11-12

## Context

M4 is "develop oso in oso" ([roadmap](../roadmap.md)). The author runs every Claude Code session
through their previous Emacs setup today, and wants oso to host that whole workflow. oso hosts the
official interactive `claude` TUI in an oso terminal ([ADR 0031](0031-claude-code-integration.md)):
no `-p`, no Agent SDK. This ADR is the second of three M4 ADRs:
[ADR 0120](0120-the-terminal.md) is the terminal, [ADR 0122](0122-agent-review.md) is reviewing
what an agent changed, and this one is everything around a hosted session.

What the author wants, in their words summarised:

- **They lose track of sessions.** Several run at once, in several worktrees, and one that waits
  for an answer can wait for an hour unseen.
- **"Ask that session to do X."** The session should be found by name and messaged.
- **No stutter.** The previous setup ran everything on one thread: parsing, drawing, `git`, the
  MCP tools, a spinner timer that once locked the editor for good in a 27-hour session.
- **Worktrees made and cleaned up safely,** and ideas from Claude Code Desktop, above all its
  Projects: one conversation that starts and tracks threads.
- **Agents may do almost everything, except:** approve their own permission prompts or connection
  questions, trust a project, type into terminals, read secret or echo-off text, push or delete
  irreversibly, and edit oso's own configuration.

What is decided already:

- **[ADR 0120](0120-the-terminal.md):** a terminal is an oso buffer with its own
  thread. Only a person types into a PTY; the agent channel never writes to one. An agent reads a
  terminal's text only where the person allowed it, per terminal. Echo off is a secret. OSC 9,
  777, 9;4 and the bell show inside oso only. It gives this ADR an in-process host API: rows by
  range or the last N, the title, OSC 9;4 progress, OSC 7's directory, OSC 133's marks, bell and
  notification events, whether the terminal is shown and selected, the foreground process group
  and its program, and extra environment for a child. The `claude` command, its MCP
  configuration, starting the agent channel, hooks and `--resume` were left to this ADR.
- **[ADR 0122](0122-agent-review.md)** leaves three seams to this ADR: deliver a message to a
  session, learn that a turn ended, and say which review the calling agent's tools act on.
- **[ADR 0085](0085-agent-channel-and-oso-mcp.md):** the agent channel, off by default, each command
  `Look`, `Change` or `Person`; anything that spawns a process is `Person`; each connection asks
  the person; questions the channel causes are answered from the keyboard only, after 500 ms with
  no key, labelled `Agent:`.
- **[ADR 0118](0118-what-an-agents-open-or-save-starts.md):** what an agent's open or save starts
  waits for the person. **[ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md):**
  projects, worktrees belong with their main repository for trust, and `git` write operations run
  only in a trusted project ([ADR 0011](0011-git-integration.md)). **[ADR 0112](0112-window-splits.md):**
  windows. **[ADR 0089](0089-built-in-themes-switched-at-run-time.md):** faces.
- **oso has no network code and no file-watching crate.** Changes on disk are found by a thread that
  `stat`s ([ADR 0083](0083-reloading-files-changed-on-disk.md)).

The forces, in the order of the project's priorities:

- **Speed.** Nothing about sessions may run on the UI thread: reading the registry, matching
  screens, `git`, `gh`, transcripts. Twenty sessions must cost the UI thread nothing while none
  changes state.
- **Security.** A session's screen, title, transcript and messages are untrusted text, written by
  a model steered by whatever it read. A state shown wrongly must never authorise anything. An agent
  must not reach, through session tools, the powers ADR 0085 and 0120 keep from it.
- **Extensibility.** Claude Code's TUI changes often. What oso reads from it must be data the person
  can fix the same day, without a release of oso.

### What Claude Code offers a host

Read from its documentation and from what a running CLI writes (2.1.29x–2.1.32x):

- **Flags:** `--session-id <uuid>` (documented) fixes a new session's id; `--resume <id>` continues
  one, and `--fork-session` copies it under a new id; `--name` names it; `--mcp-config` adds MCP
  servers for one session; `--settings` adds a settings file or JSON for one session;
  `--append-system-prompt` adds text to the system prompt; a positional argument is the first
  prompt.
- **The session registry:** each running CLI writes `~/.claude/sessions/<pid>.json` (moved by
  `CLAUDE_CONFIG_DIR`) and removes it when it ends: `pid`, `sessionId`, `cwd`, `startedAt`,
  `procStart`, `name`, `kind`, `status` (`busy`, `idle`, `shell`), `statusUpdatedAt` and
  `messagingSocketPath`. Undocumented. Beside them, `<pid>.<hash>.key` files hold messaging
  tokens. A CLI killed with `SIGKILL` leaves its file behind.
- **No lock:** a second `--resume` of a live session forks the conversation silently.
- **Cross-session messaging** (on by default since 2.1.224): the CLI's own `ListAgents` and
  `SendMessage` tools, addressed by session name, over a per-session inbox socket. A message is
  labelled as from another session, cannot answer a permission prompt or count as consent, and is
  subject to the receiver's `crossSessionInbound` setting. `@name` in the TUI's prompt mentions a
  session. The socket's line format is not documented.
- **`claude agents --json`:** the documented state of *background* sessions: `state` (working,
  blocked, done, failed, stopped) and `waitingFor` (permission prompt, input needed, sandbox
  request, worker request, dialog open). It is a process per call, about 0.2 s, and sessions run in
  an oso terminal are not background sessions.
- **Hooks** (documented): `SessionStart` gives the session id and transcript path; others fire on
  prompts, tools, permission requests and the end of a turn.
- **The transcript,** `~/.claude/projects/<slug>/<sessionId>.jsonl`, is undocumented and changes
  between versions.
- **The screen:** the title starts with a spinner while working and `✳` when idle; OSC 9;4 progress
  is set during a turn and cleared after; permission and question dialogs have fixed footers.

### What others do

- **The author's previous Emacs setup** drove `claude -p` and parsed its stream, so it knew each
  state exactly. Around it: herdr-style *Spaces* (a project or a worktree under its repository, each
  with its own window layout), a 28-column sidebar of spaces and sessions with state marks, a
  modeline count of waiting requests and a jump to the next one, notifications that go to the
  desktop only while the editor is not focused, restore of the session set *stopped*, the registry
  watched to see outside sessions and to refuse a forking resume, `start_worktree_session` (a new
  worktree, a space and a session given a brief with hand-off facts), and `list_worktrees` and
  `remove_worktree` with full safety checks. An unmerged branch added `@session:` completion,
  incoming messages drawn with their sender, and the report-back address in briefs. Its pains were
  the single thread and timers per visual effect.
- **herdr** hosts agents' TUIs in PTYs and reads each one's state from the screen: per-agent TOML
  rules on the live bottom of the screen, the title's spinner and OSC 9;4, with a debounce, a
  strict "blocked" that only a known dialog sets, `explain` to show which rule matched, and local
  rule files that hot-reload. Through v0.6.6 it took Claude's state from hooks; after stale states
  (a subagent's `Stop` made the parent idle, post-tool hooks revived idle panes, interrupts left
  `working`, stale permission reports kept panes blocked) v0.6.7 kept a `SessionStart` hook for
  identity only and now removes the old ones. Its notifications fire only for panes not shown, and
  only if the state is the same after a delay of 1 s. "Done" means finished and not yet seen.
- **Claude Code Desktop** groups sessions by state (Pinned, Needs input, Working, Ready for review,
  Completed) with unread marks, pins, sections, and sessions nested under the one that started them.
  Its Projects run a coordinator conversation that starts *threads*, with an Overview pane by state,
  project instructions and a project memory. About 25 session actions are open to Claude, each
  behind an "Allow Claude to …?" card, with a cap of 20 messages to other sessions before the person
  types again. It notifies only for sessions not being viewed, with separate toggles for
  permission, question and completion. It warns when a second session starts in a folder, honours
  `.worktreeinclude`, and its one-line recaps and state classifier cost model calls.

## Decision

The author accepted this on 2026-11-13, through the coordinator: the first option (a) of each
question under [Questions](#questions), but Q14 (see [The author's choices](#the-authors-choices)).

### A session (Q1, Q20, Q22)

- **A session is a hosted `claude` in an oso terminal, plus its identity:** the session id, a name,
  the directory it runs in (a repository or a worktree), the transcript's path once found, the
  space it belongs to, and the session that started it, if any.
- **`claude` (`Person`)** starts a session in the selected buffer's project root, else its file's
  directory, as a new terminal does, else the directory oso started in. With a prefix argument it asks for a name and a directory. The session's terminal is a
  buffer named `*claude: NAME*`.
- **Identity (Q1):** oso makes the id itself, a version 4 UUID from 16 random bytes of the system's
  (`oso_platform::random`, `/dev/urandom` on Unix), and
  passes `--session-id`. So oso knows the id before the CLI writes anything, and needs no hook for
  it. The name is passed with `--name`: the project's name, then `-2`, `-3` for more in it. The
  registry entry for the child's process id gives the inbox socket's path and the CLI's own status;
  the transcript is found by its id on a worker, once.
- **The command line** is built from parts oso controls, never from text an agent gave:
  `claude --session-id ID --name NAME --mcp-config JSON [--append-system-prompt TEXT] [BRIEF]`.
  - `--mcp-config` names one server, `oso`: oso's own executable by absolute path, with
    `mcp --pid <oso's pid>`. The person's other MCP servers still load.
  - No permission flag is ever passed: not `--permission-mode`, not `--dangerously-skip-permissions`,
    not `--allowedTools`. A session starts in the mode the person's own settings give it, whoever
    asked for it.
  - `[sessions] claude` in `config.toml` may name the executable by absolute path; otherwise the
    login shell finds `claude` on its `PATH`.
- **How it starts (Q22):** through 0120's terminal, as the login shell would run it, so the
  person's `PATH`, `nvm` and proxies apply: `$SHELL -l -c 'exec "$0" "$@"' claude ARGS…`. The
  arguments are positional parameters, never text inside the script, so nothing in a name or a brief
  is parsed by the shell. This holds for `sh`, `bash`, `zsh`, `dash` and `ksh`; for any other shell
  (`fish`, `nu`), oso runs `claude` directly with 0120's environment and `[sessions] env`. The
  spawn goes through `oso-platform::process`, logged by name as 0120 logs it.
- **Extra environment** through 0120's host API: `OSO_SESSION=<id>`, so the bridge can name its
  session (see "The bridge and the channel").
- **The `claude` command turns the agent channel on (Q20)** when it is off, and the echo area says
  so: the person asked for a session whose `oso mcp` needs it.
- **A restricted project can host a session.** It is the person's command, as any terminal is
  (ADR 0120), and Claude Code asks its own workspace trust question. What needs `git` (worktrees, the
  branch in the list, ready for review) waits for trust.
- **When `claude` exits,** the terminal keeps its last screen, read-only, and the session becomes
  `stopped` or `failed` (see "States"). `session-resume` in that buffer resumes it.

### Spaces (Q8)

- **A space is a project** (ADR 0114's root) **and its sessions.** A worktree's space is nested under
  its main repository's space, by the claim ADR 0114 checks.
- **A space keeps its own window tree (Q8).** `space-switch` saves the frame's window tree in the
  space it leaves and shows the one it enters, as a tab would. A space entered for the first time
  shows its last session beside the last file buffer of that project. `space-zoom` toggles the
  selected window alone, as `C-x 1` followed by a restore. Preset layouts wait.
- **The modeline names the space** where it names the project.

### Restore and the fork hazard (Q2)

- **oso remembers its sessions** in `sessions` in oso's state directory, written by the saver on a
  worker when the set changes: for each, the id, name, directory, space, parent, pin and archive
  marks, and when it last changed state. No screen text and no transcript text.
- **A start of oso brings sessions back stopped.** Each is a row in the list and a terminal buffer
  that says `stopped: session-resume to continue`. No process starts, as ADR 0120 has it for
  terminals, and as opening a project never executes anything.
- **`session-resume` (`Person`)** runs `claude --resume ID` with the same command line otherwise.
  First, on a worker, it reads the registry for a live process with that `sessionId`, checking each
  entry's `pid` and `procStart` against the process table. If one is live, it refuses, naming the
  process and its directory, and offers `session-fork` instead: `--resume ID --fork-session`, which
  copies the conversation under a new id. Five seconds after a resume, the registry is read again;
  two live processes with one id give a warning in the echo area and the list, since there is no
  lock to prevent the race.
- **Sessions oso did not start** (another terminal app, another editor, a background session) are
  read from the registry and listed under their directory's space as `external`, with the
  registry's `busy` or `idle` only. oso cannot show their screen; it can mention them, message them
  through a session, and refuses to resume them while they are live.

### States (Q3, Q4, Q5)

**One taxonomy, everywhere** (list, modeline, notifications, tools), in this order of urgency:

| State | Meaning |
|---|---|
| **needs input** | a known dialog is open: `permission`, `question`, `plan`, `elicitation` (MCP), `dialog` (other known dialogs). The reason is carried. |
| **failed** | the process ended with a non-zero status or a signal oso did not send |
| **working** | a turn is under way |
| **ready for review** | the turn ended and the session's directory has changes since its baseline that no review has shown (Q5) |
| **done** | the turn ended and the person has not seen the session since |
| **idle** | the turn ended and has been seen; or the TUI waits at its prompt |
| **stopped** | not running: exited cleanly, stopped by the person, or restored |

- **Marks beside states:** `pinned`, `archived`, `external`, and an age, "waiting 12 min", since the
  state was entered. A space shows the most urgent state of its sessions.
- **Seen:** a session becomes seen once its terminal is shown in the selected window of a focused oso
  window for 2 s. Reading it through a tool does not count. Only a real turn's end makes a session
  `done`: not its start, a resume or a `/clear`.

**Where states come from (Q3):**

- **For a session oso hosts, the screen is the authority,** as herdr has it after leaving hooks:
  rules over 0120's host API, evaluated on the sessions worker, never on the UI thread.
  - The evidence: the last 40 non-empty rows of the live screen (never the person's scrolled view or
    copy mode), the title, and OSC 9;4 progress, each with when it changed.
  - **A rule** has a state, a priority, a reason (for needs input), a region (`title`, `progress`,
    `bottom(N)`, `prompt_box`, `after_last_rule`, `above_prompt_box`), matchers (`contains`,
    `regex`, `line_regex`) combined with `any`, `all` and `not`, and an optional `hold` that keeps
    the last state while a full-screen view of the TUI is open (its transcript viewer, a picker).
  - **Needs input is strict:** only a rule that recognises a known dialog sets it. An unknown prompt
    falls back to idle, and `session-explain` says so. No state, whatever its source, authorises
    anything: not a paste, not an answer, not an action of an agent.
  - **Debounce:** working to idle without a visible idle sign needs 3 matches 100 ms apart, at most
    700 ms; a session's first 3 s are a grace period with no transitions but to needs input.
  - **While echo is off,** rows are withheld (ADR 0120); the state keeps its last value and the
    reason says `echo off`.
- **The process** gives failed and stopped: its exit status from the terminal's thread.
- **The registry's `status`** gives `busy` or `idle` for external sessions only. For hosted ones it
  is not mixed into the state; `session-explain` shows it beside the rule's verdict, as a check.
- **No hook reports state.** Neither does OSC 7501, a draft that nothing current emits.

**The rules file (Q4):**

- **Rules ship with oso** (`claude.toml`, compiled in) **and can be overridden** by
  `agent-rules/claude.toml` in oso's configuration directory, read as `config.toml` is
  ([ADR 0110](0110-adr-0108-as-built.md)): on the config thread at start and when it changes, through
  `privacy::who_can_change`, at most 256 KiB, refused when anyone else can change it. A rule in the
  local file replaces the bundled one of the same name; `disable = true` removes one.
- **Never from a project,** and never fetched: a repository cannot change what oso tells the person
  about an agent working in it. oso has no network code, so a fix for a new TUI is the local file
  first and a release after.
- **Patterns use the `regex` crate,** compiled once per load with a size limit of 1 MiB each; a rule
  that fails to compile is skipped and named in `*Messages*`.
- **`session-explain` (`Look`)** shows, in `*Session explain*`, the state, the matched rule and its
  source (bundled or local), the region text it matched, the debounce, the registry's status, and a
  fallback reason when no rule matched. `session-explain-file` explains a saved screen text, for
  writing a rule. Region text is the session's screen, so the buffer is the person's only: the
  channel cannot read it unless that terminal allows agent reads.
- **Tests** check the engine with synthetic rules and the bundled rules against recorded, scrubbed
  screens of the TUI; a new TUI's screens are added as fixtures when a rule changes.

**Ready for review (Q5)** reuses ADR 0122's session baseline: when a turn ends in a trusted
project, the worker asks `git diff --quiet <baseline>` against a fresh snapshot of the working tree;
changes make the session `ready for review` until a review of that session is opened. In a
restricted project the state is not used.

### The seams to ADR 0122

- **Deliver:** a message for a session goes as the person's bracketed paste into its terminal (see
  "The person sends to a session").
- **Turns:** "a turn ended" is a transition from working to needs input, done or idle, after the
  debounce. "A tool that writes finished" is not given; ADR 0122's own watching covers it.
- **Sessions:** a session started in a trusted project takes its baseline on the worker before
  `claude` spawns, waiting at most 2 s, after which it spawns and the baseline follows. The review an
  agent's tools act on is its session's, named by the bridge.

### Seeing what needs you (Q6, Q7, Q9, Q10, Q11, Q12)

- **`*Sessions*` (Q6)** is an internal buffer: sessions grouped by state, needs input first, then
  failed, working, ready for review, done, idle, stopped; pinned sessions first within their group;
  sessions nested under the session that started them, a parent row saying "2 nested need input".
  Each row: state mark, name, space, branch (trusted projects only), age, and the recap.
  - `sessions` shows it in a side window 32 columns wide at the frame's left, which `C-x o` and
    `other-window` skip and which never becomes selected by a session's own commands; `sessions`
    again hides it. Keys: `RET` show, `n`/`p`, `TAB` fold, `g` refresh, `s` group by state or by
    space, `P` pin, `A` archive, `r` resume, `k` stop, `w` new worktree session, `e` explain.
  - **`session-switch`** is the same list in a completion float ([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md)),
    with filters by state and search over names, spaces, branches and directories.
- **The global mark:** the selected window's modeline shows `⚠ 2` while two sessions need input, and
  `● 3` while three are done or ready for review. `session-next-waiting` shows the session that has
  needed input longest; `session-next-done` the oldest done.
- **Session buffers** show their state in their modeline, with the reason and the age. A session not
  shown that becomes `needs input` or `failed` blinks its mark three times, then stays; no timer runs
  for a state that does not change.
- **Keys (Q7):** the prefix `C-x a`, which stays oso's inside a terminal (ADR 0120's semi-char mode
  keeps `C-x`): `C-x a c` `claude`, `C-x a s` `sessions`, `C-x a b` `session-switch`, `C-x a n`
  next waiting, `C-x a d` next done, `C-x a j` `space-switch`, `C-x a z` zoom, `C-x a w` worktree
  session, `C-x a o` the repository's overview, `C-x a r` resume, `C-x a e` explain, `C-x a m`
  send to session, `C-x a @` mention.
- **The recap (Q12)** is one line, at most 80 characters, from deterministic signals only, no model
  call: for needs input, the dialog's question line; for working, the TUI's status line (its
  spinner's text); otherwise the title. It is cleaned as 0120 cleans a title. It is screen text, so
  the session tools give it to an agent only for a terminal that allows agent reads.

**Notifications (Q9, Q10, Q11):**

- **When (Q10):** a session enters needs input, done, ready for review or failed, is not shown in a
  focused oso window, and is still in that state 1 s later. Each kind has its toggle in
  `[sessions.notify]`: `needs_input`, `done`, `failed` on by default, `ready_for_review` off. Several
  within 2 s are one notification ("3 sessions need input").
- **How (Q9):** a new module, `oso-platform::notify`, with one function, `attention(kind, title,
  body)`:
  - **In oso always:** the modeline mark and an echo-area line naming the session.
  - **macOS:** a Dock bounce (`requestUserAttention`, which oso's window layer already calls), once
    for done and repeated until oso is focused for needs input. Banners in Notification Center need
    `UNUserNotificationCenter`, which works only for an app bundle, so they come when oso ships as
    one, through `objc2-user-notifications` (a new dependency, step 3 or later).
  - **Linux:** the window's urgency hint, through oso's window layer. Desktop banners over D-Bus
    wait, as they need either a D-Bus crate or a process per notification.
  - Text in a notification is the session's name and its state, never screen text.
- **Focusing oso** after a notification shows the session it was about, as herdr's
  `open_notification_target`: `session-next-waiting` does it with one key.

### Worktree sessions (Q13, Q14, Q15, Q16)

All of this needs a trusted project ([ADR 0011](0011-git-integration.md)): `git worktree add`
runs the repository's hooks. Every `git` and `gh` call runs on a worker, through
`oso-platform::process`, with `GIT_TERMINAL_PROMPT=0`, a time limit of 10 s each, and nothing
fetched.

- **Where (Q13):** `<repo>/.claude/worktrees/<slug>`, the slug made from the branch as herdr makes
  it (lowercase, `/` to `-`, other characters dropped, at most 64 bytes). This is where Claude Code
  and the previous setup put them, so a repository's ignore files already cover it.
- **`session-worktree` (`Person`, `C-x a w`)** asks for a branch (completing local branches; a new
  one starts at `HEAD`, or at a base given with a prefix argument) and a brief, written in a buffer
  `*Brief*` and sent with `C-c C-c`. It creates the worktree, copies `.worktreeinclude`'s files,
  opens its space nested under the repository, and starts a session with the brief as its first
  prompt. A branch already checked out elsewhere is refused.
- **The brief** gets a header and hand-off facts, as the previous setup's:
  - "You are in a git worktree of REPO at PATH, on branch B from BASE. This work was handed to you by
    session NAME; report back with SendMessage (to: NAME)." The report-back line appears only when
    the starting session is live and has a name.
  - The starting session's transcript path, its plan files if any, and `git status --porcelain` of
    its uncommitted changes, which did not come along.
  - The files it touched, read from its transcript on the worker, when the transcript parses; else
    the line is left out. The transcript is undocumented, so this is best effort.
  - The brief and facts are at most 64 KiB, passed as one argument.
- **`.worktreeinclude` (Q16)** is honoured as Claude Code honours it: a file in the main checkout
  that matches its patterns (gitignore syntax, read with the `ignore` crate already used by the
  walker) *and* that git ignores is copied into the new worktree. Regular files only, no links
  followed, nothing outside the repository, at most 1,000 files and 256 MiB, mode kept. A
  repository that has no `.worktreeinclude` copies nothing.
- **Two sessions in one directory:** starting a session where a live one already runs (hosted or
  in the registry) warns, "Another session runs in this directory. A worktree keeps their changes
  apart: w for a worktree, RET to go on", and does not refuse.
- **`worktree-list` (`Look`)** shows, for each linked worktree of the repository: path, branch,
  dirty and untracked counts, whether the branch is merged and how that was found, the sessions in
  it and their states, sessions of other projects whose directory is in it, oso buffers in it with
  unsaved changes, its space, and whether `worktree-remove` would accept it or why not.
- **`worktree-remove` (`Person`)** removes a worktree only when every check holds, run on the worker
  in this order:
  1. it is a linked worktree of this repository, not the main one;
  2. `git status --porcelain --untracked-files=all` is empty;
  3. no oso buffer under it has unsaved changes, and no `*Brief*` for it is unsent;
  4. no session in it is working, needs input or has a background task, and no external session's
     directory is in it;
  5. its branch is merged: either the reflog shows a commit made on the branch and its tip is an
     ancestor of `refs/remotes/<remote>/{main,master,develop}` or of the remote's `HEAD`; or `gh pr
     list --head BRANCH --state merged --json` shows a merged pull request into such a branch whose
     merge commit contains the tip, which covers squash merges. No remote, or no `gh` when ancestry
     is not enough, refuses.

  Then it stops the idle sessions in it, closes its space, and runs `git worktree remove PATH`,
  never `--force`. **It never deletes the branch.** The person's command asks first; the question
  names what git ignores and will be deleted with it (`target/`, `node_modules/`,
  `.claude/settings.local.json`). An agent's `worktree_remove` does not ask (Q14), and its reply
  names the same.
- **Steering Claude away from its own worktree tools (Q15):** Claude's `EnterWorktree` and a `Bash`
  `git worktree add` bypass oso's sessions and spaces. oso cannot intercept a tool call of the
  hosted TUI. When the person turns on `[sessions] steer_worktrees = true`, a session oso starts
  gets, through `--settings`, a `PreToolUse` hook for `EnterWorktree` and `Bash` that denies with a
  reason ("use the oso tool start_worktree_session; it makes the worktree, a space and a session")
  when the call is `EnterWorktree` or its command contains `git worktree add`. The hook is `oso hook
  deny-worktree`, oso's own executable, reading the hook's JSON from stdin and starting nothing.
  Nothing is written into Claude Code's settings files, and it is off until the person sets it.
  The MCP server's instructions say the same in words.

### Messages between sessions (Q17, Q18)

- **Agents message each other with the CLI's own `SendMessage` and `ListAgents` (Q18).** oso
  relays nothing, adds no transport and holds no token: it never reads the registry's `*.key`
  files. The CLI's own protections apply: its inbound setting, its rate limits, and its rule that
  a message cannot answer a permission prompt.
- **What oso adds:**
  - `session-mention` (`C-x a @`) completes the live sessions, hosted and external, from the
    registry, and inserts `@NAME`: as the person's bracketed paste into a session's terminal, or as
    text into an oso buffer such as `*Brief*`. Duplicate names get the CLI's own suffix.
  - **Incoming messages** show in the receiving TUI as `› Message from @NAME: …`. A rule of kind
    `message` matches that line; `@NAME` becomes a link (0120's links) that shows that session, and
    the receiving session's row says "message from NAME" until seen.
  - **The report-back address** in briefs (above).

### The person sends to a session (Q17)

- **A bracketed paste into the session's terminal (Q17),** as ADR 0122 delivers review comments:
  cleaned as 0120 cleans any paste, and **not submitted**. The person reads it in the TUI's input
  box and presses Enter. It is the person's typing, from the person's command.
- **Refused while the session needs input:** a paste into a permission dialog could choose an
  option. The echo area says "NAME is asking for permission; answer it first".
- **`session-send` (`Person`, `C-x a m`)** sends the region, or with no region asks for a note in
  the minibuffer, to a session picked by completion (default: the session of this space shown last).
- **Context forms,** built on the UI thread from the buffer as pure functions, then pasted:
  - `@region`: the path relative to the session's directory, the lines, and the text in a fence
    longer than any run of backticks in it;
  - `@file:L-L`: the path and line range only, which Claude Code reads itself;
  - `@diagnostics`: the language server's diagnostics for the buffer or the region (M4), as
    `path:line: severity: message`;
  - `session-fix-error` (`Person`): the diagnostic at point, its 20 lines around, and "Fix this
    error.".
  - A buffer that may hold secrets sends its path and lines, never its text. At most 256 KiB.

### The bridge and the channel (Q19, Q20, Q21)

- **The bridge knows its session:** `oso mcp` passes `OSO_SESSION` when it connects. The channel
  believes it only if the bridge's parent or grandparent process is that session's `claude`, whose
  process id oso holds as its child, so the id cannot be reused while oso has not reaped it.
  Otherwise the connection has no session, as an outside one.
- **A bridge of a session oso started connects without the question (Q19),** since the person
  started that session with that MCP configuration. The modeline's `Agent` mark and `*Agent*` log
  it as any connection. A bridge from any other process asks as ADR 0085 has it.
- **Session tools,** in `oso mcp`, each a channel method with its reach. Read tools answer while
  the channel waits for the person, as `state` does.

| Tool | Reach | What it does |
|---|---|---|
| `session_list` | Look, read-only | id, name, space, directory, state, reason, age, pin, parent, `external`; the recap only for a terminal that allows agent reads; never the asking session's own transcript path |
| `session_state` | Look, read-only | one session's fields above, and the review it has (ADR 0122) |
| `session_transcript` | Look, asks (Q21) | the last turns of a session's transcript: prompts and replies as text, no tool inputs or results, at most 64 KiB |
| `session_search` | Look, asks (Q21) | a regex over prompts and replies of this repository's transcripts, at most 50 hits of 200 bytes |
| `session_start` | Change, asks | a new session in this project, or `start_worktree_session` below |
| `start_worktree_session` | Change, asks | a branch, an optional base and a brief: as `session-worktree`, nested under the caller |
| `worktree_list` | Look, read-only | as `worktree-list` |
| `worktree_remove` | Change (Q14) | as `worktree-remove`: no question once every check holds; refused with the failing check otherwise |
| `session_stop` | Change, asks | `SIGTERM` to the session's `claude`, `SIGHUP` to its group after 5 s |
| `session_archive` | Change, asks | stops it if running, hides it from the list; its worktree is kept |
| `merge_branch` | Change, asks | `git merge --no-edit BRANCH` into the caller's worktree, which must be clean; on conflict, leaves the conflict and lists the files |
| `session_rename`, `session_pin`, `session_mark_read` | Change | metadata only |

- **"Asks" is an `Agent:` question** under ADR 0085's rules (keyboard only, after 500 ms with no key,
  `C-g` refuses). It names the caller and says what will run: `Agent: let session api-fix start a
  session in worktree .claude/worktrees/x on new branch x from main, with a brief of 42 lines (? to
  read it)? (y or n)`. `?` opens the brief in a read-only buffer. **The process starts on the
  person's `y`, as the person's act.** No tool asks to run anything in a terminal, to type, or to
  answer a prompt.
- **What no tool does:** answer or dismiss any session's dialog; send keys or text to a terminal;
  read terminal rows the person did not allow; pass a permission flag; trust a project; push; delete
  a branch; `--force` anything; edit `config.toml`, `trusted-projects`, `agent-rules/` or Claude
  Code's settings files; read the registry's `*.key` files. A tool acts only on sessions and
  worktrees of the caller's repository, and never stops or archives the caller itself.
- **The loop guard:** a session may cause at most 20 effectful tool calls (every "asks" and
  `Change` tool above, `worktree_remove` included) since the person last typed in its terminal; past that, each is refused with
  "waits for the person to type in this session". A `n` or `C-g` refuses the same tool with the same
  target from that session for 60 s, without asking again, and the reply says the person refused it.
- **Never route around a denial:** the MCP server's instructions say that a refused tool is the
  person's decision, not an obstacle, and that the agent must not reach the same effect another way
  (a `Bash` `git worktree add`, `claude` in a shell, `git push`).
- **Agent-requested sessions cost the person a question each,** so twenty parallel threads are twenty
  questions; batching them is left for later (see Projects).

### Projects, light (Q23)

A **project** in Desktop's sense, without a coordinator model:

- **The overview:** `*Sessions*` filtered to one repository (`C-x a o`), with its worktree sessions
  nested, grouped by state.
- **Project instructions:** `project-instructions` (`Person`) edits a file oso keeps in its state
  directory for the repository (by its root's device and inode, as `trusted-projects` keys trust),
  at most 16 KiB. Each session oso starts in that repository or its worktrees gets it through
  `--append-system-prompt`. It is never read from the repository itself; `CLAUDE.md` already is.
- **Shared memory** is Claude Code's own: its auto memory is kept per repository, so worktree
  sessions share it. oso adds nothing; step 7 checks that on the current CLI.
- **Nesting:** a session started by another, through `start_worktree_session` or `session_start`,
  or from a session's space by the person, records that parent and is listed under it.
- **A coordinator is just a session** that uses the tools above. Deferred: a coordinator that routes
  the person's messages to threads, suggested-thread cards with "start all", pull-request next-step
  buttons, a "landing" state, archiving after a week, the files threads produced, scheduled
  sessions, and anything cloud or remote.

### Code, crates and layers (Q24)

- **No new crate (Q24).** A new module, `sessions`, in the tip crate, beside `channel` and
  `project`, with submodules for the registry, the rules engine, states, worktrees, spaces and
  briefs. The rules engine is plain functions over text and gets a fuzz target. `layers.txt` gains
  `sessions` in `[oso]`, below `app` and beside `commands`, using `channel`, `project` and `walk`.
- **`oso-platform` gains `notify`,** a new public module (this ADR is its record), and
  `process::start_time(pid)` for the registry's liveness check (`proc_pidinfo` on macOS,
  `/proc/<pid>/stat` on Linux).
- **`oso-core`'s frame** gains saving and restoring a window tree, for spaces, and a window flag that
  `other-window` skips, for the side window. No new module.
- **One sessions worker thread,** `oso-sessions`, holds the registry poll, the rules, the transcript
  reads and the queue of `git` and `gh` jobs (at most 2 at once). It sends the UI thread only
  changes: a session's state, a list row, a finished job.

### Cost

| What | Budget |
|---|---|
| UI thread while no session changes state | nothing: no timer, no redraw |
| UI thread per state change | ≤ 0.5 ms, including the list's rows |
| Registry poll on the worker | one `stat` of the directory a second; files re-read only when changed, at most 256 files of 64 KiB |
| Rules per session on the worker | ≤ 0.2 ms an evaluation, at most 10 a second, only after the screen or title changed |
| 20 working sessions | ≤ 5% of one core on the worker |
| `worktree-list` for 20 worktrees | first rows ≤ 1 s; each `git` call ≤ 10 s or abandoned |
| Transcript tail read | the last 1 MiB of the file at most |
| `session_search` | ≤ 2 s over 1 GiB of transcripts, pre-filtered by bytes before parsing lines |
| Memory | ≤ 8 KiB a session, plus the worker's 40 rows a session |

New `just perf` flows: 20 hosted sessions printing at once (UI thread per frame stays within ADR
0120's budget), and a state change to photon.

## Steps

One pull request each. Each step updates the docs it changes.

1. **Sessions:** `claude`, identity, the command line, the login-shell start, the registry reader and
   liveness, restore stopped, `session-resume` and `session-fork` with the fork check, the bridge's
   session and its connection rule, and ADR 0122's seams. Unit tests with a fake `claude`.
2. **States:** the rules engine and its fuzz target, the bundled rules and recorded screens, the
   local file and its reload, `session-explain`, the debounce, the taxonomy, seen, ready for review,
   the modeline marks, `session-next-waiting`.
3. **Attention:** `*Sessions*`, the side window, `session-switch`, spaces and their window trees, the
   recap, `oso-platform::notify`, the keys, end-to-end flows and `just perf` flows.
4. **Worktrees:** `session-worktree`, briefs and hand-off facts, `.worktreeinclude`, the
   shared-directory warning, `worktree-list`, `worktree-remove`, and the steering hook.
5. **Sending:** `session-send`, the context forms, `session-fix-error`, `session-mention`, incoming
   message links.
6. **Agent tools:** the session tools, their questions, the loop guard, the 60 s refusal, the
   server instructions.
7. **Projects, light:** the overview, project instructions, nesting, and the check of shared memory.

Step 3 may add `objc2-user-notifications`, through the `add-dependency` skill first, only if oso
ships as an app bundle by then. No other step adds a dependency: `toml`, `regex` and `ignore` are
already used.

## Questions

For the author, the recommended option first. Each is answered on its own.

### Sessions

1. **How oso learns a session's identity.**
   (a) oso makes the id and passes `--session-id` and `--name`; the registry, by the child's process
   id, gives the socket and status. Documented flags, no hook, known before the CLI writes anything.
   (b) A `SessionStart` hook reports the id and the transcript's path, as ADR 0031 planned.
   Documented too, but a hook must be installed, and a resume or `/clear` changes what it reports.
   (c) The registry alone, by process id. Nothing to pass, but undocumented, and a changed file
   format loses every session's identity.
2. **Restore and resume.**
   (a) Sessions come back stopped; `session-resume` checks the registry and refuses a live session,
   offering a fork (`--fork-session`). Nothing starts by itself; the race left by the CLI's missing
   lock is narrowed and warned about.
   (b) Resume them all when oso starts, staggered, as herdr does. Back where you were at once, but a
   start of oso runs programs, against ADR 0120's rule, and races any copy still running elsewhere.
   (c) Like (a), but a live session may be resumed after a `y`. Lets the person override; a wrong
   `y` forks the conversation silently.

### States

3. **Where a hosted session's state comes from.** The key decision.
   (a) The screen: local-overridable rules over rows, title and OSC 9;4, as herdr does now; the
   process for failed and stopped; the registry's status only for external sessions. One authority
   per session. Breaks when the TUI changes until a rule is fixed, which the local file and
   `session-explain` make a same-day fix.
   (b) Hooks for prompts, tools, permissions and `Stop`, as ADR 0031 planned. Documented and exact
   when they fire, but herdr left them after stale states (subagents' `Stop`, interrupts, cancelled
   permissions), and they need an install step.
   (c) `claude agents --json`, polled. Documented with a `waitingFor` reason, but it covers only
   background sessions, which a session in a terminal is not, and costs a process per poll.
   (d) Screen and hooks combined, the newest winning. More evidence, but two authorities disagreeing
   is how the stale states arose.
4. **The rules file.**
   (a) Bundled rules plus a local override file beside `config.toml`, reloaded on change, never read
   from a project; `session-explain` shows what matched. A TUI change is fixed locally the same day.
   (b) Bundled only. Simpler, but every TUI change waits for a release of oso.
5. **"Ready for review".**
   (a) The turn ended and the directory changed since the session's baseline (ADR 0122), until a
   review of it is opened. Deterministic and local; trusted projects only, and one `git diff` per
   turn's end.
   (b) A pull request is open for the branch (`gh`), as Desktop. Matches a PR workflow; misses work
   not yet pushed, and calls `gh`.
   (c) No such state; done and idle only. Simplest; a finished session with changes looks like one
   with none.

### Seeing what needs you

6. **The sessions list.**
   (a) `*Sessions*` in a 32-column side window that `C-x o` skips, plus `session-switch` as a
   completion float. The previous setup's sidebar, always in view; needs a skip flag on windows.
   (b) The float and an ordinary `*Sessions*` buffer only. No new window kind; nothing in view while
   working.
7. **The prefix key.**
   (a) `C-x a`. Works inside a terminal, since 0120 keeps `C-x` oso's; Emacs uses it for abbrevs,
   which oso does not have.
   (b) `C-c c`, as the previous setup. Familiar, but inside a terminal 0120 sends `C-c c` to the
   program, so session keys would not work from a session.
   (c) A Hyper chord. Free everywhere, but only where the keyboard has Hyper.
8. **Spaces and windows.**
   (a) Each space keeps its own window tree, swapped by `space-switch`, with zoom; preset layouts
   later. The previous setup's model without a tab bar; `oso-core` learns to swap a frame's tree.
   (b) No layouts: a space is a group in the list, and switching shows the session in the selected
   window. Less to build; the person rebuilds splits by hand.
   (c) The six preset layouts too. Parity; more to build before it is known which ones are used.
9. **OS notifications.**
   (a) `oso-platform::notify`: Dock bounce on macOS and the urgency hint on Linux now; banners when
   oso is an app bundle (`objc2-user-notifications`, a new dependency then). No process per
   notification; no banner text until then.
   (b) Banners now through `osascript` and `notify-send`, run through the process module. Banners
   at once, but a process per notification, text quoted into AppleScript, and a click opens Script
   Editor, not oso.
   (c) Inside oso only. Nothing new; nothing reaches the person away from oso.
10. **When to notify.**
    (a) A session not shown in a focused oso window, still in the state after 1 s; needs input,
    done and failed on, ready for review off; bursts in 2 s merged. Desktop's and herdr's rule.
    (b) Also for a session shown, when no key reached oso for 2 minutes. Catches a person away from
    a focused window; more notifications while reading.
11. **The modeline count.**
    (a) In the selected window's modeline: `⚠ N` needs input, `● N` done or ready for review.
    (b) Needs input only. Quieter; done sessions are found only in the list.
12. **The recap line.**
    (a) From the screen and title only: the dialog's question, the status line, or the title. No
    model call, nothing written to disk; shown to agents only where the terminal allows reads.
    (b) The last reply's first line from the transcript. Says what was done, not what is happening;
    the transcript is undocumented.
    (c) None. Nothing to break; rows show only the state.

### Worktrees

13. **Where worktrees go.**
    (a) `<repo>/.claude/worktrees/<slug>`, as Claude Code and the previous setup. Ignored already;
    inside the repository, so a recursive search outside oso can see them.
    (b) A directory outside the repository, as herdr's `~/.herdr/worktrees/<repo>/<slug>`. The
    repository stays clean; paths differ from what Claude Code makes.
14. **`worktree_remove` from an agent.**
    (a) Asks each time, after every check. A removal deletes ignored files for good.
    (b) No question once every check holds, as the previous setup. Faster cleanup; an agent deletes
    `target/` and `node_modules/` without the person seeing.
15. **Steering Claude away from `EnterWorktree` and `git worktree add`.**
    (a) A `PreToolUse` hook passed per session with `--settings`, only when the person sets
    `[sessions] steer_worktrees`; nothing written into Claude Code's settings. Explicit and
    reversible; only sessions oso starts get it.
    (b) `oso integration install claude` writes the hook into Claude Code's user settings after
    showing the change, as ADR 0031 planned. Covers sessions started anywhere; edits the person's
    settings file.
    (c) Words only, in the MCP server's instructions. No hook; in auto mode Claude may still make a
    worktree oso does not know.
16. **`.worktreeinclude`.**
    (a) Honoured as Claude Code does: files both matched and ignored are copied, with caps and no
    links. Worktrees get `.env` and local settings; secrets are copied to a second place.
    (b) Ignored. Nothing copied; a new worktree may not build or run until the person copies files.

### Messages

17. **How the person's text reaches a session.**
    (a) A bracketed paste into its terminal, not submitted, refused while it needs input. The
    person's own typing; seen before Enter; no undocumented format.
    (b) The CLI's inbox socket. Arrives as a message even while the session works; it is labelled
    as from another session, so it cannot count as the person's consent, its line format is not
    documented, and the token file must be read.
18. **How agents message each other.**
    (a) The CLI's own `SendMessage` and `ListAgents`; oso relays nothing and adds mentions, links
    and report-back lines. The CLI's own limits apply; oso cannot count those messages.
    (b) Also an oso `session_send` tool writing to the inbox socket. oso could count and log
    messages; a second path to keep in step with an undocumented format.

### The bridge and tools

19. **A bridge's connection question.**
    (a) A bridge whose parent is a `claude` oso started connects without the question; others ask.
    Amends ADR 0085; twenty sessions do not mean twenty questions.
    (b) Every connection asks, as ADR 0085 has it. No change; a question per session started.
20. **The channel and the `claude` command.**
    (a) `claude` turns the channel on when it is off, and says so. One command for the common case.
    (b) It asks first. One more question.
    (c) It leaves the channel alone; `oso mcp` fails until the person runs `agent-channel-start`.
21. **Reading other sessions' transcripts and searching them.**
    (a) Asks once per connection, then allowed for it; prompts and replies only, never tool inputs
    or results, capped. A transcript holds what the agent read, secrets included, mostly in tool
    results.
    (b) No question: it is `Look`. Simplest; any session reads every other's conversation.
    (c) Not offered. Safest; "what did that session decide" needs the person.

### Projects and structure

22. **How `claude` starts.**
    (a) Through the login shell, `$SHELL -l -c 'exec "$0" "$@"'` with positional arguments; other
    shells run `claude` directly. The person's `PATH` and tools apply; no argument is parsed by the
    shell.
    (b) `claude` directly, with 0120's environment and `[sessions] env`. No shell; the person's
    shell setup (version managers, proxies) is missing unless copied into `config.toml`.
23. **Projects.**
    (a) The light version: a repository overview, instructions kept by oso and passed with
    `--append-system-prompt`, Claude Code's own memory, and nesting; a coordinator is just a session
    with the tools. Useful now; Desktop's routing and suggested threads wait.
    (b) Deferred to after M4. Less now; sessions are not grouped by what started them.
24. **Where the code lives.**
    (a) A `sessions` module in the tip crate, `oso-platform::notify`, and small additions to
    `oso-core`'s frame. No new crate; the tip crate grows.
    (b) A new crate, `oso-sessions`, between the tip crate and `oso-core`/`oso-platform`, for the
    registry, rules and worktree checks. Isolated and faster to test; a new crate and edge, and the
    terminal host API stays in the tip crate anyway.

### Dependencies (for the `add-dependency` skill, not decided here)

- `objc2-user-notifications` for banners on macOS, only once oso ships as an app bundle (Q9 a).
- Nothing else: `toml`, `regex` and `ignore` are already dependencies of the tip crate, and the
  UUID comes from `oso_platform::random`, which reads `/dev/urandom`.

## The author's choices

The author answered every question on 2026-11-13, through the coordinator: (a) for each, but Q14.

- **Q14: (b).** An agent's `worktree_remove` asks no question once every check holds, as in the
  previous setup. The checks themselves are unchanged: clean, nothing unsaved or unsent, no busy
  session, the branch merged, never `--force`, never the branch. The reply names the ignored files
  deleted with the worktree. The person's own `worktree-remove` still asks.
- The rest, as recommended: oso's own id with `--session-id` and `--name` (Q1); restore stopped,
  refuse a live resume, offer a fork (Q2); state from screen rules (Q3) with a local override and
  `session-explain` (Q4); ready for review from the baseline (Q5); the side window and the float
  (Q6); `C-x a` (Q7); a window tree per space and zoom (Q8); Dock bounce and urgency now, banners
  with an app bundle (Q9); notifying only for sessions not shown, after 1 s (Q10); `⚠ N` and `● N`
  (Q11); recaps from the screen and title (Q12); `<repo>/.claude/worktrees/<slug>` (Q13); the
  per-session steering hook behind `[sessions] steer_worktrees` (Q15); `.worktreeinclude`
  honoured (Q16); a bracketed paste, not submitted (Q17); the CLI's `SendMessage` (Q18); no
  question for a bridge of a `claude` oso started (Q19); `claude` turns the channel on (Q20);
  transcripts asked once per connection (Q21); the login shell (Q22); the light projects (Q23); a
  `sessions` module and `oso-platform::notify`, no new crate (Q24).

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0031](0031-claude-code-integration.md), "Hooks: SessionStart reports the session ID and
  transcript path. Other hook events report state": the id is oso's own, passed with
  `--session-id`; state comes from the screen; no hook reports state. "OSC 7501 status sequences":
  not used. "Hook installation is explicit: `oso integration install claude`": the one hook oso
  offers is passed per session with `--settings`, when the person sets `[sessions]
  steer_worktrees`; nothing writes Claude Code's settings files.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "Each new connection asks the person": except a
  bridge whose parent is a `claude` oso started. "anything that spawns a process" is `Person`: a
  session tool may ask the person, and the process starts on the person's `y`. "A channel action
  never starts a process by itself": except an agent's `worktree_remove`, which runs `git worktree
  remove` without a question once every check above holds (Q14). "At most 2 clients": a hosted
  session's proven bridge is outside the 2 (the author's choice, see "Step 1, as built").
- [ADR 0122](0122-agent-review.md), "Turns: … with ADR 0121, the Stop and PostToolUse hooks": a turn's end is the state's
  transition; a write tool's end is not given.

## Step 1, as built

Step 1 was built on 2026-11-13, after step 2 and on ADR 0120 step 4a's terminal. The author
answered its questions through the coordinator, (a) for each, and the coordinator approved these
points:

- **A hosted session's bridge does not count toward the channel's cap of 2 connections (the
  author's choice, 2026-11-13).** Each session's `oso mcp` keeps its connection open for its
  life, so with ADR 0085's cap a third session would get no oso tools. The rule:
  - A connection proven to be a hosted session's bridge is let in without the question and
    outside the cap.
  - Each session holds at most one such connection; a second claim of the same session asks as
    any connection does, and counts.
  - The rate limits stay for every connection: 30 requests a second, 60 connections and 10
    questions a minute.
- **The proof is the parent only, not "parent or grandparent".** A one-time run of the real CLI
  (2.1.322) showed that `claude` starts a stdio MCP server itself, with no process between. So a
  claim is believed only if the connecting process runs oso's own program, its parent is the
  `claude` that oso holds as its child (its process id from the terminal's start until it is
  reaped), the id it claims is that session's, the session's terminal has not ended, and no other
  connection holds that session. It is one pure function, `sessions::bridge::admit`, and the UI
  thread decides with it (`Event::Claims`). The same run showed that both `claude`'s environment
  and the configuration's `env` reach the server, and that the `oso` server given with
  `--mcp-config` is the one started.
- **The claim:** `oso mcp` reads `OSO_SESSION` and sends it as `{"session": ID}` in its first
  request, `version`, at most 64 bytes. A claim that fails proof is dropped, and the connection
  asks and has no session.
- **The command line** uses `--flag=value` for every option, and `--` before a brief, since
  `--mcp-config` takes several values and would take a brief as one. A fork passes oso's new id
  with `--session-id`, which the CLI accepts only together with `--fork-session`. A test keeps the
  line to these flags only.
- **The login-shell wrapper** is `SHELL -l -c SCRIPT CLAUDE ARGS…` for a shell whose file name is
  `sh`, `bash`, `zsh`, `dash` or `ksh`; tests run it through each one installed, with `$(…)`,
  backticks, `;`, newlines, quotes, a leading `-` and non-UTF-8 bytes in the arguments. The
  script runs `[sessions] claude` as given. Else it looks for `claude` on the `PATH` the login
  shell set, as oso's own look does when the shell is not one of these: only in absolute
  entries, and in none inside the session's directory, compared with links resolved.
  The shell is ADR 0120's: `[terminal] shell`, then `$SHELL`, then the account's, then `/bin/sh`.
- **The registry reader** (`sessions::registry`) reads only names of the form `<digits>.json` in
  `$CLAUDE_CONFIG_DIR/sessions`, else `~/.claude/sessions`: at most 256 files of 64 KiB, opened
  without following a link, and only the reading user's. oso never reads the `*.key` files.
  Whether a session runs cannot be told, and a resume is refused with the fork offered, when the
  directory has more entry files than are read, when oso's `CLAUDE_CONFIG_DIR` is not an
  absolute path, or when there is no home. A
  process is live if `process::start_time` of its pid is within 2 s of the entry's `procStart`
  (the start as C `ctime` text in UTC), or, when that does not parse, within 10 s of `startedAt`
  (ms since the epoch). A process whose start cannot be told counts as live, so a resume is
  refused rather than risked: an entry with neither field, and a pid the system gives no start
  time for, though it may run. Step 1 reads it only before and 5 s after a resume; the poll comes
  with step 3's list.
- **`oso-platform` gained `process::start_time(pid: u32) -> io::Result<Option<SystemTime>>`,**
  with 3 `unsafe` blocks, each with its `// SAFETY:` comment: `proc_pidinfo` and reading its
  result on macOS, and `sysconf(_SC_CLK_TCK)` on Linux. `Ok(None)` is no such process; an error
  is a process the system does not say the start of, such as another user's, or any process on
  a system it does not support, and the registry reader counts it as live.
- **`oso-platform` gained, in the first follow-up, `privacy::home() -> Option<PathBuf>` and a
  module `random` with `fill(bytes: &mut [u8]) -> io::Result<()>`, with no `unsafe`** (the
  author's approval, 2026-11-13), so that the tip crate reads neither the environment's home nor
  `/dev/urandom` itself. `home` is `$HOME` on Unix and `%USERPROFILE%` on Windows, only an
  absolute path, from the environment and not the disk; the registry's `~/.claude` and the app's
  `~` both take it, so a relative or empty `HOME` is no home for either. `random::fill` reads
  `/dev/urandom` on Unix and is `ErrorKind::Unsupported` elsewhere for now, where a session
  refuses to start with that error. `random` is on the lowest line of `[oso-platform]` in
  `guardrails/layers.txt`. One exception is intentional: `oso-term`'s build script still reads
  `HOME` for its vendor cache, as a build script cannot use `oso-platform` without a
  build-dependency, which is not worth it there.
- **`oso-term` gained `Terminal::pid(&self) -> Option<u32>`:** the child's process id from its
  start until it is reaped, cleared before the reap, so a reused id is never given.
- **The remembered set** is `sessions` in oso's state directory, JSON lines of id, name,
  directory, project root, parent, pin and archive marks, last change and the baseline's tree:
  at most 256 sessions and 256 KiB, private, refused if a link or another user's, written to a
  part file and renamed. A write reads the file again first and keeps the ids this instance never
  knew, so two instances do not drop each other's sessions. It keeps the sessions that changed
  last, and stops before a line that would take the file past 256 KiB, so every file written
  can be read back. The table oso holds keeps 256 too: past that, the sessions that do not run
  go, archived ones first, then unpinned ones, the oldest change first; a buffer whose session
  went says `forgotten` in its modeline. It is read and written on the
  `oso-sessions` worker (`Job::Recall`, `Job::Remember`).
- **The start thread** (`oso-session-start`, one per start) makes the id, checks the directory and
  `[sessions] claude`, reads the registry before a resume and 5 s after it, finds the shell, and
  in a trusted project takes ADR 0122's baseline, waiting at most 2 s; past that, `claude` starts
  and the baseline follows. Only `claude` takes a baseline, and only when the session runs at
  the root of the selected buffer's trusted project: a resume, a fork, and `C-u claude` in
  another directory take none. It then starts the session's `oso_term::Terminal` itself, so the UI
  thread never waits on the spawn. The shell is found by the same function as a terminal's
  (`terminal::hosted::find_shell`), and the app puts the terminal in its buffer with the same code
  (`app/terminals/start.rs`).
- **The environment** is ADR 0120's terminal environment, `[sessions] env`'s names,
  `OSO_SESSION`, and always `CLAUDE_CONFIG_DIR` when oso has it: the session's registry entry is
  then in the directory oso reads. A login profile that sets `CLAUDE_CONFIG_DIR` itself, which oso
  started from the Dock does not see, moves the session's registry where oso does not look, and a
  resume then finds no live process for it. Set the same value where oso is started too.
- **The start baseline is the project's review baseline:** when it comes, in time or late, the
  review takes it as if `B` had been pressed (ADR 0122), and the review menu shows when it was
  taken. The review's index copies for it are in the same state directory as the review's own.
  Its root is revoked with the project's trust, as a review's is.
- **A refused resume's fork question forks that session,** whichever buffer is selected when `y`
  is answered. A start that is refused turns the agent channel off again if that start turned it
  on and no session runs, starts or holds a connection, unless the person or the configuration
  asked for the channel since. A bridge's claim whose answer comes after the channel's server
  stopped waiting holds no session.
- **`C-u claude` reads a directory, then a name,** each with its own history. `oso-core` gained
  `ArgSpec::DirectoryAndTextOnPrefix { directory, directory_history, prompt, history }` and
  `Ctx::directory_and_text(&self) -> Option<(&str, &str)>`, and the new
  `SessionRequest::{Start { name, directory }, Resume(BufferId), Fork(BufferId)}`. With no
  prefix, nothing is read.
- **Keys:** `C-x a c` (`claude`) and `C-x a r` (`session-resume`); the rest of `C-x a` comes
  with step 3. `claude`, `session-resume` and `session-fork` are `Person`.
- **`C-x k` on a session's buffer archives the session:** it is not brought back at the next
  start, and step 3's list shows and restores it. Its `claude` is hung up, and the session counts
  as stopped, not failed. Any other end is stopped for status 0 and failed otherwise; the buffer
  keeps the last screen and its modeline says `stopped: session-resume to continue`, or `failed
  (exited 3): …`.
- **ADR 0122's seams in step 1:** the baseline at start, and "which review is this agent's" as
  the connection's session. Delivering comments as a paste and "a turn ended" wait for step 3,
  which feeds screens to the states: until oso reads a session's screen it cannot refuse a paste
  into an open dialog, so a delivery stays in the kill ring.
- **Configuration:** `[sessions] claude`, the absolute path of `claude`, and `[sessions] env`,
  at most 64 names of variables a session takes from oso's environment besides ADR 0120's; `PATH`,
  `LD_*`, `DYLD_*`, an empty name and one with `=` are refused.
- **Layers:** `sessions` is on its own line in `guardrails/layers.txt`, above
  `commands dev grep keys loader terminal text`, so it starts its terminals itself. Only `app` and
  `config` use it.
- **`dispatch/prompts/replace.rs`** holds `query-replace`'s prompts, moved out of `prompts.rs`,
  which the directory and name prompt took past the file size limit.
- **Not verified on the CLI:** that the registry entry's `sessionId` and `name` are the ones oso
  passed. The one real run ended before the entry was read, and no other real run is allowed;
  the end-to-end flows check the fields with a fake `claude`.

## Step 2, as built

Step 2 was built on 2026-11-13, before step 1 and before ADR 0120's terminal, against screens
recorded from Claude Code 2.1.322. The coordinator approved these points:

- **`regex` became a dependency of the tip crate.** It is the same version and features as
  `oso-core`'s, so the tree gains no crate. The dependency line above was wrong: only `toml` and
  `ignore` were the tip crate's.
- **The fuzz target is a proptest generator** (`sessions/fuzz_tests.rs`). It makes rules files
  and screens, and runs in `just ci`. The tip crate has no library target that cargo-fuzz could
  link; a cargo-fuzz harness can share the generators once `oso-term` has one.
- **A deadline of 5 ms for each look at a screen.** The rules check it between matchers. A
  crafted local file can be slow even within the format's limits: past the deadline, the state
  holds with the reason "rules took too long", and the same screen is judged again after the
  longer wait. `*Messages*` says so after two late looks in a row at one session's screen,
  once for each load of the rules, naming the slowest rule; `*Session explain*` shows it. The
  deadline is the wall clock's, so it is best-effort: a busy machine can pass it once.
  - The Cost table's 0.2 ms is still the bundled rules' budget; the deadline only bounds a
    local file.
  - A session whose looks keep running out of time is judged less often: the wait doubles
    each time, up to 5 s.
  - A local rule that is the slowest of 3 late looks in a row is not used until the rules
    load again, and `*Messages*` names it once. A bundled rule is never dropped: its cost is
    bounded and tested, and a busy machine must not turn a dialog off.
  - **Each pattern's size limit is 128 KiB,** not the 1 MiB above, and its lazy DFA's cache
    the same: the deadline is checked between patterns, not inside one.
  - The worker takes every queued job before it judges, so screens that queued up are judged
    once. A wake-up of the debounce gives the last verdict again, without judging the same
    screen twice.
- **`hold` is a state a rule gives**, not a flag beside one. `session-explain-file` is
  `Person`: the sessions worker reads the file, at most 256 KiB.
- **The format** is in [agent-rules.md](../agent-rules.md). It has the regions listed above,
  with no `whole_recent`, since `bottom(40)` is the same.
- **`oso-core` gained `editor::SessionRequest`, `Editor::request_session`,
  `Editor::take_session_requests`, `Editor::set_internal_text`,
  `Buffer::withhold_from_agents` and `Buffer::withheld_from_agents`.**
  - `*Session explain*` is withheld from agents: the channel's state gives neither its text nor
    its cursors, `text` refuses it, and the channel cannot switch to it. Unlike a secret, it
    changes nothing else: the channel still acts while it is shown, and secure input, the
    person's kills, emergency copies and the modeline are as they were.
  - A command the channel runs while it is shown says what it says only in the echo area,
    which the channel does not read then, and its kills from it count as secret. So the
    channel cannot copy the text where it could read it.
  - The app keeps the buffer by its id, never by its name, and fills only an internal
    buffer: one of that name that anyone made stays theirs. `C-x b` makes no new buffer of a
    name an internal buffer holds.
  - ADR 0120's per-terminal read permission (`terminal-allow-agent-read`) can reuse the same
    flag.
- **What the recorded screens showed.** The title stayed `✳ …` throughout the recording. So a
  turn is told by the `esc to interrupt` footer and the status line; the title's spinner and
  OSC 9;4 stay as further signs.
  - The transcript viewer hides a dialog that waits behind it, and its footer then starts
    with `dialog waiting`. A rule gives needs input (`dialog`) for that footer; the viewer's
    hold ranks below every needs-input rule.
  - `after_last_rule` is empty when no rule row is on the screen, as `prompt_box` is: model
    output with no prompt box below it is never a dialog.
  - Each view that holds (help, the model and resume pickers, settings) is told by its own
    footer. A footer that only offers Esc says nothing, since an unknown dialog has one too and
    must stay idle.
  - A test keeps every bundled needs-input rule above every hold rule. `*Messages*` warns
    when a local file ranks a hold rule above a needs-input rule.
- **`Evidence` has no change timestamps.** The tracker takes the instants it is given, and the
  worker the instant a look arrives; ADR 0120's host API need not stamp them.
- **The engine's echo-off hold is its own signal.** Claude Code's TUI runs in raw mode, with
  ECHO and ICANON both clear. So if the sessions engine held on plain "ECHO clear", every
  session's state would hold for good. The engine needs a narrower signal, such as a password
  prompt's shape: ECHO clear while ICANON is set.
  - What counts as secret in a terminal is ADR 0120's ("while the PTY does not echo", Q17).
    Raw-mode password prompts depend on it, and its step 4 settles it with the author.
  - This step does not narrow it. The engine's signal may differ from it, and is chosen with
    the host API.
- **The feed for step 3.** Today the app owns the worker's one sender, which is not `Clone`.
  Step 3 should give each terminal's thread a feed of its own that clones, taking only
  `Screen` and `Exited` jobs, so that screens never pass through the UI thread.
  The worker's queue has no bound today, as only the app sends to it. Once terminals send
  screens, it needs one: a session's newest screen replaces its last, so a bounded queue
  loses nothing a judgement needs.
- **Left to step 3, which has live sessions:**
  - `session-explain` on a running session;
  - the modeline marks;
  - `session-next-waiting`;
  - the 2 s timer that marks a session seen;
  - the `git diff` call for ready for review.

  The tracker already takes seen and the call's answer.

## Step 3, as built

Step 3 was built on 2026-11-13, on top of ADR 0120's terminal view (step 4a) and this ADR's
step 1. The author chose the recommended answer to each of its questions; the coordinator
approved these points:

- **The side window is outside the window tree** (`editor/sessions/side.rs`): one window at
  the frame's left, 32 columns wide, with a divider. `tiling()`, a space's tree, a restart's
  layout and `C-x +` never see it. `C-x o` skips it; a click and `windmove-left` reach it.
  While it is selected, the tree's commands (`C-x 2`, `C-x 3`, `C-x 1`, a resize, `M-,` and
  `M-/`, `*Messages*` and the list of emergency copies with their way back, and showing a
  buffer, `show_in` the side window too) act on the tree's window selected last, and select
  it, so that no command shows anything in it. `C-x 0` in it hides it, and `C-x 1` keeps it.
  `C-x 0` in a tree's window never selects it, and the side window goes if the buffer it
  shows does.
  - The agent channel never selects it, nor a window whose buffer has keys of its own: after
    an agent's command, the dispatcher selects again the window it had, and says why, as it
    does for a terminal (ADR 0120); if that window has no room left, a window or buffer with
    the ordinary keys is put in front. An agent's `delete-window` is refused in the side
    window, and while another tree's window shows such a buffer. An external session's row
    says `external` first, and its name loses what draws nothing.
  - It shows by itself while oso has a session that is not archived. `C-x a s` shows and
    selects it when hidden, selects it when shown, and hides it when selected. Once the
    person hides it, by `C-x a s` or `C-x 0`, it stays hidden for that oso instance.
- **Spaces swap an opaque `WindowTree`** (`editor/sessions/trees.rs`): each window keeps its
  cursors, scroll, kept places and ADR 0122 pair. `swap_window_tree` refuses while a prompt
  is open and gives the tree back. `space-zoom` swaps in a tree of one window like the
  selected one, and back. Showing a session of another space switches to that space first.
  A space is a project root; worktrees' spaces get their repository with step 4. A zoom made
  before the first session is the first space's, and a space entered with nothing to show
  takes the frame's tree with its zoom. The buffer a space left keeps as its last is the
  tree's window's, never `*Sessions*`.
- **What the editor keeps for sessions is one field, `Sessions`**, in
  `editor/sessions.rs`: the requests, the side window, the modeline's marks
  (`SessionMarks`, drawn as `⚠ N` and `● N` on the selected window's modeline only) and the
  lists the host gives. `editor.rs` is at its size limit, so nothing of this is there.
- **`ArgSpec::Listed` completes over a list the host gave** with `Editor::set_listed`, read
  back with `Editor::listed_candidates(&self, list: &str) -> Arc<[Candidate]>`, a public item
  beyond the plan's list. `session-switch` lists sessions by buffer name, annotated with
  their state, space and recap, so `&needs` finds those that need input; `space-switch` lists
  spaces. The agent channel cannot answer a `Listed` prompt: its candidates hold recaps.
- **Recaps come from the rules**: an optional `recap` pattern on each rule, and
  `recap_region` for where it looks (see [agent-rules.md](../agent-rules.md)). The worker
  finds the recap after each verdict, within its own 5 ms deadline, and sends `Out::Recap`
  only when it changed. A recap that cannot be found leaves the title, without the Braille
  spinner a CLI puts before it while it works, so that the list is not drawn again each time
  the spinner turns.
- **`*Sessions*` is an internal buffer, withheld from agents** (step 2's flag), in
  `Mode::Sessions`. While it is shown, the channel's commands answer in the echo area only.
  It is rendered only while shown, to the window's width, with point kept on its row. Ages
  move on only with a frame drawn anyway, and with `g`: there is no timer of their own.
- **Notices:** a state that `[sessions.notify]` names, of a session not in front of the
  person, becomes due 1 s later if it still holds. The first notice goes at once; those due
  within the next 2 s are merged into one line ("3 sessions need input"). A notice is said in
  the echo area and asks for attention: on macOS `oso-platform::notify` (a Dock bounce,
  repeated for needs input and failed), on Linux and Windows winit's urgency, critical for
  those two. The marks blink 3 times after a session that is not shown needs input or fails.
  The text names sessions and states only.
- **A session counts as seen** after 2 s in the selected window of a focused oso window with
  no prompt open. A turn's end in a trusted project, with a baseline, takes the tree as it is
  now on a review's thread (ADR 0122, at most 2 at once) and tells the worker whether it
  differs from the baseline: that is `git diff --quiet` done with the review's own git. A
  review of the session scope tells the worker its project's sessions were reviewed.
- **The screen feed goes through the UI thread** (Q6), not from each terminal's thread as
  step 2 proposed: after a session's snapshot is merged into ADR 0120's kept grid, its last
  rows, title and OSC 9;4 progress go to the worker at most every 100 ms, and only when
  something changed, never while the person has scrolled back. The worker keeps them in a
  slot of one screen per session, so a busy worker never queues more than one each.
- **Other programs' sessions:** once oso has a session, the worker looks at Claude Code's
  registry once a second: a listing and a `stat` of each `<pid>.json`, read again with step
  1's reader only when one changed, and each process's start time. Live entries that are not
  oso's own are listed as working or idle, never shown. Their names are cleaned as recaps
  are.
- **`k` in the list hangs up a session's terminal**, asking first while it works or needs
  input. The question is the app's, as only the app knows the state; its yes runs
  `session-stop` again, which finds it answered.
- **What differs from the decision above.** `sessions` shows, selects and hides in three
  steps rather than toggling. The side window and spaces' trees are two new modules under
  `editor/sessions/`, not a window flag. `session-switch` searches names, spaces and recaps
  (in their annotation), not directories. The list where `ArgSpec::Listed` reads its
  candidates is kept in the same `Sessions` field, as sessions are its only lists so far.
- **Reaches, the author's choice (2026-11-13):** what puts a session's terminal or
  `*Sessions*` in front of the person's keys is `Person`, as the host selects the window after
  the command: `sessions`, `session-next-waiting`, `session-next-done`, `space-switch`,
  `space-zoom` and the list's `sessions-show`. So are `sessions-pin` and `sessions-archive`,
  which change which sessions ask for the person. `session-switch` (whose list the channel
  cannot answer), `session-explain` and the list's moving, folding, regrouping, refreshing
  and explaining stay `Look`.
- **The views are made once a batch:** a change from the worker marks them stale, and they
  are made once before the event loop waits; what the person asked is made at once. The list's
  text is set only when it changed, and a frame is asked for only when the list, the marks or
  a modeline note changed. Spaces are placed again only when the table changed.
- **The list's untrusted lines never begin with a mark:** a space or recap that does loses
  its leading blanks and marks, so that it never reads as a row, and `n`/`p` never stop on it.
- **A restart leaves the side window out** of the handed-over windows, as the layout is the
  tree's; it comes back by itself while sessions exist. `oso dev`'s handoff hands over only
  the tree in the frame: the other spaces' trees and a zoom are lost, and the spaces start
  again from their last buffers. A project no longer trusted revokes
  the root of a running look for a session's changes, as it does a review's.
- **The registry poll runs while a session of oso's runs,** and stops when none does. It looks
  at most at 1,024 names, and reads a file again only when it changed. A look made before
  the poll stopped is dropped, and a poll started again tells what it finds anew.
- **Not done in step 3:** the branch in each row (it needs git on a trusted project, and
  comes with step 4's worktrees); finding a session's trusted project needs one of its
  buffers open. Echo off is still open: `Evidence::echo_off` holds a session's state, but
  nothing feeds it yet. It waits for ADR 0120 step 4b's echo signal, which must tell a
  password prompt from the raw mode every TUI runs in (see "Step 2, as built").

## Consequences

- A session needs no hook and no change to Claude Code's settings; its identity and MCP server come
  from flags oso passes.
- State depends on the TUI's look. A TUI release can show wrong states until a rule is fixed, and
  `session-explain` and the local file make that fix the person's to do. A wrong state never
  authorises anything.
- Nothing about sessions runs on the UI thread, and nothing runs while nothing changes.
- Agents can start, stop and archive sessions only through a question the person answers. They
  remove a finished worktree without one, but only when every check holds, never with `--force`,
  and never deleting its branch; its ignored files go with it. They cannot type, answer prompts,
  push or delete branches.
- External sessions are visible but read-only to oso.
- Follow-up: banners when oso is an app bundle; preset layouts; Desktop's coordinator routing;
  Windows (registry paths, named pipes, ConPTY). `open-questions.md`'s M4 items on hooks and on
  which worktree tools join `oso mcp` are answered here, and the roadmap's M4 line follows.

## Alternatives considered

- **Hosting `claude -p` and parsing its stream,** as the previous setup: exact states, but ADR 0031
  rejected it.
- **A pty host process or a background server,** as herdr: sessions outlive oso, but a second
  process and protocol; ADR 0120's handoff covers `oso dev`'s restarts.
- **A model that classifies states or writes recaps,** as Desktop: better words, but plan usage, a
  network path oso does not have, and a second agent reading every screen.
- **oso typing an Enter after a paste, or into a dialog, for an agent:** ADR 0120 forbids it; it is
  how an agent would answer its own permission prompts.
- **Remote rule updates,** as herdr's manifests: oso has no network code.
