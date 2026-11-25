# Using oso daily

Install a release build of oso for daily work, and keep developing oso with `just dev` beside
it. The installed oso is a copy: rebuilding the repository does not change it while it runs.

A terminal in oso holds `oso dev`'s restarts until the restart handoff exists. So run agent
sessions and other long-lived programs in the installed oso, not in `oso dev`.

## Prerequisites

macOS or Linux. Windows is not supported yet.

- The Rust toolchain from `rust-toolchain.toml`, `just`, and a C compiler for the syntax
  grammars (the Xcode Command Line Tools on macOS).
- Zig 0.17, the version `crates/oso-term/ghostty-vt.lock` names, on `PATH` (or set `ZIG` to it).
- `curl` and `minisign`.

Then, once, from the repository:

```sh
just ghostty-vt
```

It fetches the libghostty-vt release that the terminal is built on, and checks its signature and
sha256. The tarballs go to `~/.cache/oso/vendor` (or `$OSO_VENDOR_DIR`). Run it again when the
lock file changes.

## Install

```sh
just install
```

This builds the release binary, as `just release` does, with no dev features, and puts it at
`~/.local/bin/oso`. Give another directory as `just install DIR`. The binary is copied under a
temporary name and then renamed, so an oso that is already running keeps working. The recipe
prints where the binary went and its version and commit, and says if the directory is not on
`PATH`. It never edits your shell profile.

## Run

```sh
oso            # or: oso FILE
```

Start it from a shell. `oso --no-config` starts without your configuration file.

## Trust a project

oso opens any project restricted: none of its code runs. Worktree sessions and reviews run git
there, so they wait until you trust the project. A session itself runs in a restricted project
too, and Claude Code asks its own trust question.

Visit a file of the project, then run `M-x trust-project` and answer `y`. It works on the project
of the current buffer's file, or of a session's own buffer, so run it from one of those, not from
a plain terminal or `*Spaces*`.
A git worktree is trusted while its main repository is. `M-x untrust-project` takes it back.

## Language servers

In a trusted project, visiting a file starts its language server for the project, if it is
installed: rust-analyzer (found on `PATH` or in `~/.cargo/bin`), gopls (or in `~/go/bin`), the
project's own TypeScript server in `node_modules`, or `tsc --lsp`, texlab, sourcekit-lsp, taplo
and marksman. One that is not installed is said once. The modeline says `lsp` after the project's name: dim while it starts,
`lsp 34%` while it indexes, `lsp!` if it stopped after crashing three times. `*LSP*` says what
the servers did. `M-x lsp-restart` starts the buffer's server again, `M-x lsp-shutdown` stops it
until `M-x lsp-start`, and untrusting the project stops it. In a buffer a server follows,
`M-.` goes to the definition at point, `M-,` goes back, and `C-M-,` forward again; each window
keeps its own history. With several definitions, and for `M-?`, the references, a list opens,
each row `path:line: text` with a preview of its place; `RET` goes there. An agent can look at
the list you have open, its rows and the preview's lines, but never opens one itself. A file outside any
project that they lead to, such as the standard library's, joins the server until its buffer is
killed. `C-M-i` completes the word before point in a popup under it, as Corfu does, and so does
typing three letters of a word and pausing, or one of the server's characters such as `.`: typing
narrows it, `C-n` and `C-p` move (from the last back to the first), `TAB` inserts, `RET`
inserts once you have moved, and `C-g` or any other key closes it. An item that needs a `use`
line adds it as it goes in; when that line is out of view, the echo area says which it is.
`C-c C-r` lists the server's code actions for the region, or for point, such as a fix for the
error there or a refactoring, the one the server prefers selected; `RET` applies the one picked
to the buffers it changes, each as one step to undo, and the echo area says what changed. One the
server disabled is faded, with its reason. An action that changes a file no buffer holds asks
first, `apply to N files, M not open?`; `y` opens them, unshown and unsaved, and applies it. An
action that runs a command of the server's applies what the server then changes.
`M-x lsp-rename` renames what is at point, as Eglot's `eglot-rename`: the prompt names the old
name, which `M-n` gives, and `*lsp-rename*` then shows every line the rename changes, in every
file, as it is and as it would be. `y` applies it, each buffer's edits one step to undo, opening
the files no buffer holds unshown; nothing is saved, and `C-x s` saves. `n`, `q` or `C-g` leaves
everything as it was, and `C-g` while the server works stops it. It has no key, as `C-c` and a
letter are yours to bind.

A server can be changed in `config.toml`:

```toml
[lsp]
start = "manual"                   # only lsp-start starts one

[lsp.servers.rust-analyzer]
command = ["/opt/rust-analyzer/bin/rust-analyzer"]

[lsp.servers.rust-analyzer.options]
check.command = "clippy"
```

## Agent sessions

A session is Claude Code run by oso, in a buffer named `*claude: NAME*`. In a project you
trust, or a space's orchestrator, it is a native session: its conversation shows as oso's own
rows, and you write to it in a composer under them (see [Native sessions](#native-sessions)). In a restricted project, or
with `mode = "tui"` in [Session settings](#session-settings), it is Claude Code's own TUI in an
oso terminal. oso tracks its state (needs input, working, done, stopped and so on), and gives
it oso's own tools through `oso mcp`.

`C-c a` opens the Agents menu, in every buffer and in every terminal. Press the key of a head;
`q` closes the menu, and `C-u` before `C-c a`, or inside the menu, goes to the head you press.

| Key | Command | What it does |
|---|---|---|
| `c` | `claude` | Start a session in the current project. With `C-u`, it asks for a directory and a name. |
| `w` | `session-worktree` | Start a session in a new git worktree (see below). |
| `r` | `session-resume` | Resume the stopped session whose buffer this is. |
| `a` | `session-adopt` | Bring a session started outside oso into oso. |
| `s` | `sessions` | Show and select the `*Spaces*` pane, or go back from it; with `C-u`, hide it. |
| `b` | `session-switch` | Show a session, picked by name. |
| `n` | `session-next-waiting` | Show the session that has waited for input longest. |
| `d` | `session-next-done` | Show the session that has been done longest. |
| `f` | `session-follow-message` | Show the session that sent this one its newest message. |
| `m` | `session-send` | Paste the region, or a note, into a session. |
| `!` | `session-send-diagnostics` | Paste the language server's errors and warnings of the region, or of the buffer, into a session. |
| `x` | `session-fix-error` | Ask a session to fix the error at point: its message, the 10 lines either side, and "Fix this error.". |
| `@` | `session-mention` | Insert a session's name as `@NAME`. |
| `e` | `session-explain` | Explain why this session has the state it shows. |
| `j` | `space-switch` | Switch to another space. |
| `z` | `space-zoom` | Show the selected window alone, and back. |
| `O` | `space-new` | Make a space for what you describe, of the selected buffer's repository, and start its orchestrator. |
| `S` | the space menu | Add or remove a repository, instructions, directory, rename, close, reopen. |
| `K` | `session-stop` | Stop this session. It asks first while the session works or needs input. |
| `P` | `session-pin` | Pin or unpin this session; dim outside a session's buffer. |
| `A` | `session-archive` | Archive this session, or bring it back; dim outside a session's buffer. |
| `R` | `session-rename` | Rename this session, or the one on the `*Spaces*` row: the prompt holds its name. |
| `t` | `session-switch-mode` | Hand this native session to Claude Code's TUI, in a terminal, or take a TUI session back to native, once it is idle. |
| `v` | `review-layout` | The review layout, as `C-x v r`: from the layout's slot, back to the review. |
| `D` | `review-session` | Review what changed in this project since its baseline. |
| `V` | the review menu | The review menu, as `C-x v r` opens it with no review open. |

So `C-c a r` resumes a session, and `C-c a S` opens the space menu.

Inside a session's own buffer, the modeline says `Session`, and the most used heads are `C-c` and
the head's letter with Control, `C-c C-n` for `n`:

| Key | What it does |
|---|---|
| `C-c a` | The Agents menu, as anywhere. |
| `C-c C-n`, `C-c C-d` | The session that waited longest, the one done longest. |
| `C-c C-b` | Show a session, picked by name. |
| `C-c C-p` | The `*Sessions*` pane (`s` in the menu; `C-c C-s` is kept for the terminal's search). |
| `C-c C-r`, `C-c C-a` | Resume, adopt. |
| `C-c C-t` | Take this TUI session back to native (`session-switch-mode`). |
| `C-c C-w` | Start a session in a new worktree of this session's project. |
| `C-c C-e`, `C-c C-f` | Explain this session's state; show who sent it its newest message. |
| `C-c C-z` | Zoom the selected window. |
| `C-c C-v` | The review layout, as `C-x v r`: from the layout's slot, back to the review. `C-c a D` reviews what changed in its project since its baseline. |
| `C-c @` | Insert a session's name as `@NAME`; in a native session's conversation, into its composer. |
| `C-c +`, `C-c -` | Let the agent channel read a TUI session's screen, or take that back. |
| `C-c C-u` | A prefix argument for the next key, as `C-u` elsewhere: `C-c C-u C-c a m` sends the place only. |
| `C-c C-y` | Attach the image on the clipboard: oso keeps it in a private file and pastes `@` and its path into the prompt, or a native session's composer, for you to send. A TIFF is converted to PNG. macOS only for now. |
| `C-c ?` | Say these keys in the echo area. |

The other heads have no such key, as the terminal or Emacs keeps theirs: `C-c a c` starts another
session, `C-c a m` sends, `C-c a j` switches spaces, and `C-c a K`, `C-c a P`, `C-c a A` and `C-c a R` stop,
pin, archive and rename this session.

In a native session, `C-c C-c` sends your message and `C-c C-k` interrupts the turn (see
[Native sessions](#native-sessions)). In a TUI session's terminal, `C-c C-c` sends Claude Code
a `Ctrl-C`, and `C-c C-k` switches char mode, where every key goes to Claude Code. Any other
`C-c` key that is not listed here sends nothing. To change these keys, see
[Session settings](#session-settings). `C-z` would suspend
Claude Code with nothing to resume it, so oso keeps it back; `C-q C-z` sends it. Every other key
Claude Code uses reaches it as you type it, but `C-x` and `C-y`, which stay oso's: `C-q C-x C-e`
sends Claude Code's `Ctrl-X Ctrl-E`, and `C-q C-y` its own yank.

In any terminal, `C-c a` is oso's, and so is every key with Super (Option on macOS, the logo key elsewhere), which programs
rarely read; `C-q` and the key sends it to the program. So a key of your own with Option, or with
a keyboard's Hyper key, works in terminals too. Hyper (Shift, Command, Control and Option
together) reads as `C-M-s-` and an upper-case letter:

```toml
[keys.global]
"C-M-s-A" = { menu = "agents" }    # Hyper-a opens the Agents menu
"C-x a" = { menu = "agents" }      # C-x a, as oso had it before
```

A session's buffer belongs to the project its session runs in, so `C-x v r`, `C-x p f` and
`C-c a w` work from it.

A TUI session starts in the permission mode your own Claude Code settings give it: oso passes
it no permission option. A native session shows its mode under its composer, and `S-TAB` in its
buffer or composer changes it. A session starts the agent channel, which its tools need.

### The spaces pane

`*Spaces*` shows in a window 44 columns wide at the frame's left while you have sessions;
a frame with no room for that narrows it, down to 24 columns. It is a
tree: each space, its repositories, the worktrees the space uses of each (the main checkout
first, then the newest), and the sessions in each, one line each. Each level has a form of its
own: a space's row is a section header on a band, with a rule to its most urgent state; a
repository's name ends with `/`; a worktree's row starts with its fold (`▾` open, `▸` folded)
and has its sessions' most urgent mark at the right; a session hangs from its worktree on a
guide (`├`, `└`) and starts with its own mark. A session named after its worktree shows its
title (the conversation's topic) instead. A space of one repository
lists its worktrees right under its own row. While another space is in front, a space is
folded unless one of its sessions works or needs you; closed spaces wait in a folded
`Archived spaces` group. Each line has a mark, a name, and how long since it last changed state (`now` under
a minute). A worktree's mark is its most urgent session's. Worktrees made with git outside oso
show too, and those untouched for a week wait in a folded `quiet` row. `s` groups the
sessions by state instead. A session that needs you takes a second line with the question it
asks, and a stopped one that could not resume one with the reason. Marks are coloured by what they ask of you:
red needs you, amber works (its mark turns while it is in view), green is done, grey is idle.
Sessions run outside oso are listed too, all in grey: under their worktree, or in a folded
`Other projects` group. The worktree you are in has its name in an accent colour and a faint
band behind its rows.

`C-c a s` (or `C-c C-p` in a session's terminal) moves you into the pane; pressing it again, or
`q`, takes you back to the window you came from, and `C-u C-c a s` hides the pane. While the
pane has your keys, the row at point has an accent band and an accent bar at its left edge, and the
pane's modeline says `q back · TAB fold · ? keys`; otherwise it says `C-c a s to enter`, or the
key you bound `sessions` to. A
click on the pane moves you into it at that row; a second click, or `RET`, acts on the row. A
click on a `▾` or `▸` folds.
The
modeline shows `⚠ N` while sessions need input and `● N` while sessions are done or ready for
review. When a session needs you and you are looking elsewhere, oso says so in the echo area
and asks the desktop for your attention. In the pane, a session waiting for your answer (a
permission, a question, a plan) blinks: its mark and name, and the `⚠` of its worktree and its
space, even folded, until you answer or show it.

In `*Spaces*`:

| Key | What it does |
|---|---|
| `RET` | Show the session on this row. On a space, go to it; on a closed one, open it again. On a worktree, show its session, or go to its space, or find a file in it when it has neither. On a repository or a group, fold it. |
| `n`, `p` | Next and previous row. |
| `M-n`, `M-p` | Next and previous space, repository, worktree or group. |
| `TAB` | Fold or unfold a space, a repository, a worktree or a group. |
| `S-TAB` | Fold everything to the spaces, then open down to the worktrees, then open everything, in turn. |
| `<right>`, `<left>` | Open a folded row, or go into an open one; fold an open row, or go to the row this one is under. |
| `q` | Back to the window you came from. |
| `a` | Add a repository to the space in front. |
| `c` | Start a session in this row's worktree: a project's main checkout, the worktree, or the session's worktree. |
| `w` | Start a session in a new worktree of this row's project. From a worktree, the new branch starts at its branch; from a session, it is also that session's child. |
| `/` | Switch to a session by name, as `C-c a b`. |
| `<`, `>` | The pane 4 columns narrower or wider, from 24 to 80, for this oso. |
| `g` | Refresh. |
| `s` | The tree, or grouped by state. |
| `P` | Pin or unpin: pinned sessions come first. |
| `A` | Archive, or bring an archived session back. On a space, close it, as `C-c a S` `c` does, and go to `Archived spaces`; on a closed one, `RET` opens it again. |
| `r` | Resume a stopped session. |
| `k` | Stop the session. It asks first while the session works or needs input. |
| `R` | Rename the session: the prompt holds its name, `M-p` empties it. |
| `e` | Explain its state. |
| `?` | Say what the marks mean, and these keys. |

`C-c a c` and `C-c a w` in the list work as `c` and `w`. `C-x o` skips the list. `C-x k` in a
session's buffer stops and archives the session.

`R` renames a session in oso and its buffer at once, to a name no other session or space has. A
running Claude Code keeps the name other sessions message it by until you resume it; type
`/rename NEW` in a TUI session's terminal to change that now. A space's orchestrator keeps the space's slug,
which never changes; `space-rename` changes the name you see.

### Spaces

A space is the workplace of one issue: its repositories, the worktrees made in it, their
sessions, and its own windows. oso keeps spaces from one start to the next. `C-c a j` switches
spaces, and the windows of the space you leave are kept for when you come back. Showing a
session of another space switches to that space. A worktree made in a space stays in it. A
session started in a repository joins the space in front when that space has the repository,
else the space you were in last that has it, else a new space of that repository. Each
repository you used before spaces came becomes a space of its own. The selected window's
modeline names the space in front.

`C-c a O` makes a space: describe what it is for, and the space is made with the selected
buffer's repository (with none, it asks for one), entered, and its orchestrator started there;
the buffer you were in stays in its own space. A space has two names: a display name, which
`*Spaces*`, the modeline and `C-c a j` show and you can change at any time to any text, wide
characters and emoji too; and a slug, lower-case ASCII, which never changes. The slug names the
space's directory, `~/spaces/<slug>` (set `[spaces] root` in `config.toml` to put them
elsewhere), its orchestrator, which other sessions message by that name, and the space as agents
name it. Both are made from the description: your own `claude` summarizes it into the two names, with no
tool and nothing kept, in a few seconds; where it cannot, the slug comes from the
description's first English words, else the repository's name, and the display name from its
start. Set `[spaces] naming = "local"` in `config.toml` to skip `claude`. oso makes the directory readable by you
alone, and refuses a root that other users can write. When the directory or the start is
refused, the space stays, and `o` in the space menu starts its orchestrator once you fixed what
the echo area said.

`C-c a S` opens the space menu. Its keys act on the space in front, but for `c` and `R`, which
read a space:

| Key | Command | What it does |
|---|---|---|
| `a` | `space-add-repo` | Add a repository, by completion or its path; it must be a main checkout. |
| `r` | `space-remove-repo` | Take a repository out; refused while a session of the space runs in it. |
| `u` | `space-use-repo` | Choose the repository that `session-worktree`, `worktree-list` and `worktree-remove` use from the orchestrator's buffer, when the space has several trusted ones. |
| `i` | `space-instructions` | Edit the goal the space's orchestrator is given. |
| `d` | `space-directory` | List the space's directory. |
| `n` | `space-rename` | Give the space a new display name, unlike every other space's: the prompt holds its name now. Its slug, directory and orchestrator's name stay. |
| `c` | `space-close` | Close a space, by completion, the one in front first: `RET` alone closes it. Its idle sessions are stopped; its worktrees, branches and directory stay. Refused while one of its sessions works or needs input. Closing another leaves the one in front as it is. |
| `R` | `space-reopen` | Open a closed space again. |
| `o` | `space-orchestrator-start` | Start the space's orchestrator again, the session you talk to about the work, in the space's directory, with each trusted repository as a working directory. `C-u` makes a space directory that is gone again at its place. |
| `O` | `space-orchestrator-replace` | Replace the orchestrator by a new one that takes over from it, once it is idle: the old one is stopped and kept as `NAME-was-N`, and the new one is told to read `handoff.md` first. Asks first when there is no `handoff.md`. |

The orchestrator is told the space's repositories, its description and its instructions, and to
keep a `handoff.md` in its directory for whoever takes over. Its row is right under its
space's, and `RET` on the space's row shows it. A repository added to the space, or trusted,
while it runs is offered to it: `y` pastes `/add-dir PATH` into its prompt, not sent; a
native orchestrator is resumed with it instead, once its turn ends. In
Claude Code, `/permissions` and its Workspace tab list the directories it was given.

A new orchestrator, whether `C-c a O`, `o` or `O` starts it, is a
[native session](#native-sessions) unless `mode = "tui"` in
[Session settings](#session-settings): with no message, its composer is selected for your
first one, and a successor's brief is its first message. It asks no trust question, and it
reads your own Claude Code settings alone: none of the space directory's `.claude/` settings,
hooks, MCP servers, commands or `CLAUDE.md`, nor a `CLAUDE.md` in `~` or `~/spaces`, which a
TUI orchestrator reads. Put what every orchestrator should know in the space's instructions
(`i`) or your own settings. Its memory is the space's, as a TUI one's is. An orchestrator
already running as Claude Code's TUI stays one until you switch it (`C-c C-t`) or replace it.

The orchestrator works through sessions: its tools name a repository of the space with
`repo`, and it sees the space's sessions only. A worker it starts belongs to the space, and
its brief says to report to the orchestrator by the space's name. `space_status` gives any
session of the space the space, its repositories with their trust, and its sessions. With
`with_dirs`, the orchestrator lets a worker also read another trusted repository of the space or
one of its worktrees; the question you answer lists them.

Reviews go through the orchestrator too. Ask it to review a worker's change, and say how: the
viewpoints (security, design, performance, spec, tests). It starts each reviewer with
`session_start` in the worker's worktree, with no "Start here anyway?" question (when oso asks
you about the start at all, the question says another session runs there), and told not to
edit, commit or push; that is its brief, not something oso enforces. It checks their reports
against the code, merges and ranks them, and tells you what holds and what it dropped. When you
ask it about code under review, it reads the code first and cites `file:line`, and says when an
answer is the implementer's.

### Send work to a session

Select a region and press `C-c a m`, then pick the session. The region goes as its path, its
lines and its text. With `C-u`, only the path and lines go (`path:10-20`), and the agent reads
the file itself. With no region, oso asks for a one-line note. The paste is not submitted:
oso shows the session, and you read the paste and press Enter. A session that is asking a
question is refused until you answer it.

In a buffer a language server follows, `C-c a !` sends its errors, warnings and notes, one line
each (`path:line: severity: message`): those in the region, or the whole buffer's. `C-c a x`
sends the most severe one at point, the server's message in a fence of its own, its 10 lines
either side and "Fix this error.". Hints never go, and neither does anything of a buffer that
may hold secrets.

`C-c a @` gives a session's name as `@NAME`, which an agent uses to message that session.

### Adopt a session started elsewhere

`C-c a a` lists the Claude Code sessions oso does not host: those still running elsewhere, then
those of the last 30 days. Pick one, and oso resumes it in a new session buffer: natively if
it ran at the root of a project you trust, as a new session starts, else in a terminal. Stop it
in the other program first: oso cannot move a running process, so it refuses a live one and offers a
fork instead. If the session was in the middle of a turn when it stopped, ask it to go on.

## Worktree sessions

A worktree session works on its own branch in its own directory, so parallel sessions do not
change the same files. The project must be trusted, and so must its main repository.

1. With a file of the project selected, press `C-c a w`.
2. Give a branch: an existing local one, or a new name. A new branch starts at `HEAD`. With
   `C-u`, oso asks for its base too.
3. Write the task in `*Brief*`. `C-c C-c` sends it, and `C-c C-k` drops it.

oso makes the worktree in `.claude/worktrees/` of the repository and starts a session there with
the brief as its first prompt. It copies the git-ignored files that `.worktreeinclude` names,
such as a local `.env`. No hook of the repository runs, so run any setup the repository needs
yourself, in the worktree's terminal.

`M-x worktree-list` lists the repository's worktrees in `*Worktrees*`: each one's branch, its
changes, whether its branch is merged, and its sessions. In it, `g` lists them again and `D`
removes the worktree at point. `M-x worktree-remove` removes a worktree only when:

- it has no uncommitted or untracked files,
- no buffer under it has unsaved changes and no brief for it is unsent,
- no session in it is busy,
- and its branch is merged. A pull request merged on GitHub counts, when `gh` is set up. A
  branch with no commit of its own does not.

It asks first, naming the git-ignored files that go with the worktree. It stops the idle
sessions there, never forces, and never deletes the branch. Once git has removed the worktree,
the sessions that were in it are archived, and the echo area says how many: they wait under
`Archived` in `*Spaces*`, where `A` brings one back. As oso starts, it also archives the stopped
sessions whose directory is gone from inside one of your spaces' repositories, such as a
worktree removed outside oso, and `*Messages*` says how many. While a removal runs, its row in
`*Worktrees*` says `removing…`. `M-x worktree-remove-stop` stops it: before git runs, nothing is
removed; once git may be deleting, it asks first, as the worktree is then left half removed.

## Review changes

`C-x v r` opens the review menu for the current file's project while no review is open, and
`C-u C-x v r` always:

| Key | Reviews |
|---|---|
| `D` | What changed since the baseline (see below) |
| `w` | Uncommitted changes, untracked files included |
| `u` | Unstaged changes |
| `s` | Staged changes |
| `b` | The branch against its base |
| `c` | One commit |
| `r` | A range of commits, `FROM..TO` |
| `p` | A pull request, by its number, through `gh` |
| `B` | Take a new baseline now |

Each is a command too, such as `M-x review-uncommitted`. Reviews run git, so the project must
be trusted. `C-c a c` takes a baseline when it starts a session at the root of a trusted
project, so `D` shows what that session changed.

In a space with an orchestrator, `C-x v r` (with no review open, or `C-u C-x v r`) first asks
which of the space's trusted repositories to review, only when it has two or more, with the
selected file's repository offered by `M-n`. Then a small menu asks what of it:

| Key | Reviews |
|---|---|
| `p` | A pull request, by its number: it opens on the worktree its head is checked out in, so `RET` goes into that worktree's files |
| `w` | A worktree, by completion, then the review menu above for it |
| `.` | This checkout: the selected file's worktree of the repository, else its main checkout, then the review menu |

So the menu works from the orchestrator's terminal too, which is in no repository.

A review opens in the review layout: the review at the left, and one window at the right, the
slot, which shows one thing at a time. `RET` shows the source there, `C-u RET` the old side,
and `C-c C-c` the message of your comments, each in place of the last, so windows do not pile
up. `C-x v r` is the one key for the rest:

- in the slot, it goes back to the review, where you were;
- in the review, it puts back the windows you had before the review, exactly, as `q` does;
- with a review open and no layout, it shows the layout again.

The slot is two fifths of the frame; `C-x }` and `C-x {` resize it. Each space keeps its own
layout, and `C-c a z` zooms inside it. `|` shows the two panes at the layout's left.

In a review buffer:

| Key | What it does |
|---|---|
| `n`, `p` | Next and previous hunk |
| `N`, `P` | Next and previous file |
| `c` | Comment on the line or hunk at point, or reply to the comment at point |
| `e` | Edit your comment at point |
| `k` | Remove your comment at point, with its replies |
| `r` | Resolve the thread, or open it again |
| `}`, `{` | Next and previous comment |
| `a` | Fold the agent's comments, or show them whole |
| `A` | Accept the hunk at point, or the file's on its header: stage it |
| `x` | Reject it: put its old lines back in the file and save it (in the staged review, unstage it) |
| `v` | Mark it viewed, or clear the mark |
| `u` | Undo the last decision |
| `\|` | Two panes, the old side and the new side lined up, or one stream again |
| `M-n`, `M-p` | In a pull request's review: its next and previous commit, each reviewed alone |
| `l` | In a pull request's review: its commit list, above the review |
| `g` | Read the files again now |
| `C-c C-c` | Gather your comments not sent yet into a message to the session, or to the space's orchestrator |
| `Q` | Ask the space's orchestrator about the hunk or line at point, or the region's lines |
| `q` | Put back the windows from before the review layout, or bury the review |
| `?` | List these keys |

`A` and `x` work in a review of your work tree or index, not of a commit, branch or range, and
on a modified file's hunks, not a new, deleted, renamed, binary or linked file's. `x` asks first
when the file's buffer holds unsaved edits, as it saves the whole buffer. A hunk's header says
what you decided, the file's header how many of its hunks you decided, and the modeline how far
you are; a file says when decisions dropped because the agent changed their hunks.

A review shown reads its files again by itself when they change: when the agent saves, ends a
turn, or edits from a shell, and when you save. Your cursor stays on its line. A hunk new since
the last reading, or of a file new to the review, says `· changed` until your cursor has been
in it. `g` reads now.

A pull request reads a commit at a time too. `M-n` in its review shows its first commit alone,
with the commit's message above its files, and `M-n` and `M-p` go on to the next and the
previous commit, and from the first back to the whole pull request. Merges and commits that
change no file are stepped over. Each commit's review keeps its own comments and viewed marks;
`C-c C-c` in any of them gathers the comments of the whole pull request and of each commit into
one message, a group each, and tells the agent that a commit's lines are as that commit left
them. `l` shows the commit list above the review: the whole pull request first, then a row a
commit, `▶` on the one shown, `✓` once every hunk of it is viewed (`◐` for some), `●N` for its
comments not sent, and `↑` for a commit not pushed yet. `RET` there shows a row's review, `SPC`
shows it and stays in the list, and `l` or `q` closes the list.

Pasting a pull request's comments records its head as your last review of it, and so does
`M-x review-checkpoint`. Once the head moves on, after a push or a rebase, the commit list's
second row is `Since last review`: `RET` there reviews only what changed since, its new commits
marked `new`, and `M-n` steps to the first of them. Set the mark on one commit's row and go to
another's, and `RET` reviews that range of commits.

A comment opens as a draft at the top: `RET` breaks a line, `C-c C-c` keeps it, and `C-g` drops
it into the kill ring. `C-x k` closes the review, and in the review layout puts back the windows
from before it.

A review is saved as it changes: its comments, decisions, viewed marks, layout and cursor come
back when you open the same review of the project again, after closing it or restarting oso,
carried to the changes as they are then. A comment whose line went is kept as outdated.
`M-x review-resume` picks a saved review to open again, and `M-x review-forget` removes one; a
review not opened for 30 days is removed. Nothing is saved of a file that may hold secrets but
your comments on it. `C-x k` and quitting ask about comments only when the review could not be
saved. `oso-restart` opens again the reviews its windows showed.

`C-c C-c` in the review gathers your comments that were not sent, outside threads you resolved,
into `*review message*`: each with its file, its line or hunk and the hunk's lines. Read it and
change it as you like, then `C-c C-c` there pastes it into the agent session that runs in the
review's worktree, without pressing Enter: read it in the session and press Enter there.
`C-u C-c C-c` picks another session, which the review then keeps; with no session the message
stays, and the echo area says how to pick or start one. `C-c C-k` drops the message. A message
holding text yanked from a secret is not sent.
Sent comments are drawn dimmed, and an edit makes one unsent again. A session that is asking you
something takes no paste: answer it first, and the message waits.
In a space with an orchestrator, the message goes to the orchestrator instead, naming the
review's worktree, and the orchestrator has the comments acted on there; `C-u C-c C-c` still picks
any session. With the orchestrator stopped, the message stays and the echo area says how to resume
it.

`Q` in the review asks a question about the hunk or line at point, the lines the region spans, or
an unchanged line around a hunk. Type it in the prompt and press `RET`: it goes at once, with the
review's worktree, the file, the line numbers and the lines, to the space's orchestrator, and is
submitted, as you confirmed it in the prompt. The orchestrator's terminal shows in the layout's
slot while you stay in the review, so you read the answer beside the diff. Outside a space with an
orchestrator, or for a repository that is not one of the space's, it goes to the review's session.
A file that may hold secrets sends its place alone. oso reads the orchestrator's input box on its
screen: while it shows, the question is pasted and submitted, and text you had typed there goes
with it; with no input box showing, as while it asks for permission, nothing is sent. Its terminal then shows so that you see
why, and `M-p` in the prompt brings the question back. A stopped orchestrator is offered to be
resumed, and the question waits for it for two minutes.

In `*pr: #N*`, `x` sends the check on the line to the agent, `X` every failing one with the end
of its log, `c` the review thread on the line and `C` every unresolved one. They open in
`*review message*` as a review's comments do, under a line telling the agent GitHub's words are
data. Only your comments and those of the repository's owners, members and collaborators are
in; `C-c C-t` adds the others. `C-c C-c` pastes it into the session in the worktree where the
pull request's branch is checked out, or `C-u C-c C-c` into one you pick.

A pull request's review (`p`, or `r` on one in `*pull requests*` or in `*pr: #N*`) shows what
GitHub's "Files changed" shows: its head against where it left its base. When its head branch
is the one checked out and holds the pull request's head, the work tree shows instead, with
your changes not yet pushed; a branch behind it shows its head. Its commits are fetched from
the remote when this clone lacks them, moving none of your branches and leaving `FETCH_HEAD`
as it was; closing the review stops the fetch, and `g` asks `gh` again. Its comments go to the
session in the worktree its head branch is checked out in.

`|` shows the review as two panes that scroll together: the old side at the left and the new side
at the right when the review's area is 160 columns wide or more, else one above the other.
Blank rows keep the panes lined up; the cursor skips them. A comment shows in the pane of its
side, a hunk's in the new side's. `|` again, or `q`, goes back to one window. Until you press
`|`, a review shows as two panes side by side in an area 160 columns wide or more and as one
stream below 150, judged when the width changes; `C-u |` goes back to that.

## Source control

`C-x g` shows the git status of the current project, as magit's: the head, the untracked,
unstaged and staged files, then the unpushed, unpulled and recent commits. It runs git, so the
project must be trusted.

| Key | Does |
|---|---|
| `TAB` | Show a file's hunks, or fold a section |
| `s` / `u` | Stage or unstage what point is on: a section, a file, a hunk, or a region's lines |
| `k` | Discard it, after a question; a copy is kept for two weeks (`M-x git-discarded`) |
| `c` | The commit menu: `c` commit, `a` amend, `e` extend, `w` reword, and `f` fixup or `s` squash for the commit at point |
| `P` | The push menu: `p` where git pushes, `u` the upstream, `e` elsewhere; `- f` forces with a lease, after a question |
| `F u` | Pull the upstream, fast-forward only; `- r` rebases onto it, `- m` merges it |
| `C-c C-c` | Go on with a merge, rebase, cherry-pick, revert or `git am` that stopped |
| `C-c C-s` / `C-c C-k` | Skip the commit it stopped at, or abort it, after a question |
| `z` | The stash menu: `z` stash, `a` apply, `p` pop, `k` drop the stash at point; `- u` takes untracked files |
| `b` | The branch menu: `b` check out, `c` make and check out, `n` make, `m` rename, `k` delete |
| `l` | The log menu: `l` the graph of the branch, `a` of every ref, `o` of another; `C-x v l` from anywhere |
| `N` | The pull request menu, through `gh`: `v` the branch's pull request, `k` its checks, `o` open it in the browser, `c` create one (`- d` a draft, `- b` into a base you name), `m` merge it, `l` the open ones, `i` the open issues (`C-x v i` from anywhere; there `w` starts a worktree session from one); `r` in a pull request's buffer, or on one in the list, reviews its changes |
| `f u` / `f a` | Fetch the upstream's remote, or every remote |
| `RET` | Visit the file, or review the commit at point |
| `$` | Show what git's runs wrote; `C-g` stops one |
| `g` / `q` / `?` | Read it again, leave it, list the keys |

In the graph, `RET` reviews the commit at point, `w` copies its id, `a` shows every ref or the
branch again, `g` reads it again, `n` and `p` move a line, and `q` leaves it. `m` marks a commit,
and `RET` with two marked reviews the range; `c` checks out the branch on the line, `b` makes a
branch there, `t` a tag, `C` cherry-picks the marked commits or the one at point, and `V`
reverts it, asking first where it matters.

When a merge or a rebase stops on a conflict, the status says so under its head and lists the
files under Unmerged paths: resolve each, `s` stages it, and `C-c C-c` goes on. Commits run the
repository's hooks, with no time limit. A push, a pull or a fetch can ask for no
password or passphrase: set up a credential helper or `ssh-agent`, or push from a terminal.

## What an agent in oso can do

A session oso started connects to oso without a question. Its `oso mcp` server gives it these
tools besides oso's editing ones:

| Tool | What it does |
|---|---|
| `session_list`, `session_state` | Read the sessions oso hosts and their states. |
| `session_start` | Start a session in the repository or one of its worktrees. Asks you first. |
| `start_worktree_session` | Start a session in a new worktree, with a brief. Asks you first. |
| `session_resume` | For a space's orchestrator: resume a stopped session of its space with its conversation, as `C-c a r` does. Asks you first. |
| `session_rename` | Rename its own session or one it started; a space's orchestrator also its space's sessions. Asks nothing, and the echo area says it. |
| `session_archive` | Archive a session it started; a space's orchestrator also its space's sessions, never an orchestrator or a pinned one. A stopped session is archived with no question. One that runs is stopped first only when it is idle, done or ready for review and Claude Code says its process is idle, with no shell left running in the background; that stop asks you first. Its worktree is kept, and `A` under `Archived` brings the row back. |
| `space_add_repo` | For a space's orchestrator: add a repository's main checkout to its space, as the space menu's `a` does. Asks you first, unless the setting below allows it for a trusted repository oso already knows; any other always asks, and one oso does not trust joins restricted. Once it is trusted, oso offers to paste `/add-dir PATH` into the orchestrator, or to resume a native one with it. |
| `worktree_list` | List the repository's worktrees, as `M-x worktree-list` does. |
| `worktree_remove` | Remove a finished worktree, under the same rules as `M-x worktree-remove`. |
| `space_status` | Read the session's issue space: its repositories, their trust, the orchestrator's name, its sessions. |
| `draft_set` | Give back the commit message draft `C-c C-d` asked for. It goes in your message buffer; nothing is committed. |

A tool that starts a session, or adds a repository to a space, asks you in oso first. The question names the session that asks
and what would start. `y` allows it, `n` or `C-g` refuses it, and `?` shows the whole brief of a start. A refusal
holds for the same request for a minute. To let sessions you started start sessions without the question, see
[Session settings](#session-settings).

To stop a session that runs away, each session makes at most 30 calls a minute of the tools
that ask or change something. Past that, oso tells the agent how long to wait. Normal work,
even an orchestrator's round over a whole space, stays well under it, and nothing you do is
limited.

Claude Code run in a plain `M-x terminal`, or outside oso, is not a session. If you gave it
`oso mcp` as an MCP server yourself, it reaches oso only after `M-x agent-channel-start`, and
each connection asks you first. Of the session tools, it gets the reading ones, and
`move_to_space`.

### Moving a session into a space

To move a Claude Code session started outside oso into one of oso's spaces, open the space in
oso, then tell the session "move to oso's space NAME". Its `move_to_space` asks you in oso: the
question names the session, the directory it runs in, and the repository it adds to the space
if the space does not have it yet. The repository must be one oso trusts.

On `y`, the session ends its turn and stops by itself where it runs. If it does not (on
Windows, or when its turn does not end, as at a permission prompt), type `/exit` there. Once
that process has ended, oso resumes the conversation in the same directory, in that space, as
`session-adopt` does (natively at the root of a project you trust), with your own settings:
flags the outside run had, such as `--model` or a permission mode, are not carried.
While oso waits, a buffer `*moving: NAME*` says so; kill it with `C-x k` to cancel. If the
process still runs after 5 minutes, or you close the space or stop trusting the repository
meanwhile, oso resumes nothing; use `session-adopt` once it has stopped.

## Claude Code in a plain terminal

`M-x terminal` opens a terminal with your shell.

While a program in a terminal asks for a password (`sudo`, `ssh`), the modeline says `secret`:
the agent channel waits while that terminal is shown, and on macOS secure input is on while it
is selected. An agent reads a terminal only after you run `M-x terminal-allow-agent-read` in it
(the modeline says `agent reads`), and never while it asks for a password;
`M-x terminal-deny-agent-read` takes that back.

## Session settings

In `config.toml`:

```toml
[sessions]
claude = "/usr/local/bin/claude"  # the executable, by absolute path; else found on PATH
env = ["AWS_PROFILE"]                # more variables a session takes from oso's environment
steer_worktrees = true               # point the agent to start_worktree_session (off by default)
spinner = false                      # keep working marks in *Spaces* still (they spin by default)
blink = false                        # keep waiting sessions in *Spaces* still (they blink by default)
agents_start_without_asking = true   # let your sessions start sessions, and add known repositories, with no question (off by default)
agents_start_limit = 4               # how many such sessions each may run at once, 1 to 8, or "none"
prompt_history = false               # keep what you send native sessions in no file (kept by default)
pane_width = 52                      # the *Spaces* pane's columns, 24 to 80 (44 by default)
mode = "tui"                         # new sessions host Claude Code's TUI in a terminal ("native" by default, in trusted projects)

[sessions.notify]                    # which states ask for your attention
needs_input = true
done = true
failed = true
ready_for_review = false
```

`steer_worktrees` gives each new session a hook that turns down Claude Code's own worktree
tools and points the agent to `start_worktree_session`, so its worktrees show in oso.
`spinner = false` stops the working marks from turning, and `blink = false` stops waiting
sessions from blinking; with both, the list runs no timer. `prompt_history = false` also leaves the file there as it is (`M-x native-prompt-history-clear` empties it); it is off at once, and on again from oso's next start.
`pane_width` is where the pane starts; `<` and `>` in it change it for this oso only.
The `[sessions.notify]` values shown are the defaults.

`mode = "tui"` starts new sessions, forks, adopted ones and new orchestrators as Claude Code's TUI in a terminal,
as oso did before [native sessions](#native-sessions), the default. A session oso hosts keeps
the mode it ran in when you resume it or move it into a space, and a TUI session carried over
`oso-restart` stays one.

`agents_start_without_asking` lets a session you started, with a key or a `y`, start sessions
through `start_worktree_session` and `session_start`, and a space's orchestrator resume a stopped
session of its space through `session_resume`, without asking you, in a trusted project
only, up to `agents_start_limit` running sessions each; past that it asks again. It also lets a
space's orchestrator add a trusted repository oso already knows (one `space-add-repo` would
offer) to its space through `space_add_repo`, with no start limit; any other
repository asks. It lets `session_archive` stop an idle session without asking too, whatever
the project's trust, with no start limit, and the echo area says each one. With
`agents_start_limit = "none"` no limit makes it ask: a session may start as many sessions as it
asks for, within its 30 calls a minute of the tools that ask or change. A session started this way asks for its own. A session an
agent starts, whether it asked or not, stays in the background: it appears in the pane and the
modeline marks, and the window you are in keeps its buffer. Each start is said in the echo area
and `*Messages*`, its row in `*Spaces*` says `unasked`, and `C-c a e` on it shows who started it
and its brief, until oso
restarts. `M-x agent-starts-ask` brings the question back until oso restarts, and
`M-x agent-starts-allow` takes that back. The setting is read when oso starts, so a change applies
from the next start, and it does not apply from a file other users in a group can change. Each new
session still asks you for its own permissions, as your Claude Code settings say.

`[keys.session]` changes the keys of a session's own buffer, as `[keys.global]` does elsewhere:

```toml
[keys.session]
"C-c g" = "sessions"                 # C-c g shows the list
"C-c C-p" = false                    # C-c C-p does nothing
"C-c R" = { menu = "review" }        # C-c R opens the review menu
```

Only keys that start with `C-c` reach it, and `C-c C-c` and `C-c C-k` stay as they are. `C-c ?`
says the keys with your changes.

## Native sessions

Unless `mode = "tui"`, a new session in a trusted project, or a space's new orchestrator,
shows its conversation in `*claude: NAME*` as oso's own rows. In a restricted project it runs in a terminal, where Claude
Code asks its own trust question. A start with no brief starts at once, with the composer
under the conversation selected for your first message; a session you end before writing one
leaves nothing behind. Your messages are on a green band after `〉`, what an agent started the session with on a violet one after `@  from`, Claude's replies two columns in, and
each tool call one row: `▸` folded, `▾` open, `·` with nothing to open, `…` running (`waiting for you` while it
asks), `✗` failed or denied. A subagent's rows, and the calls of an opened row of several, have
a `│` at the left. Only oso's marks stand in the first two columns, so nothing Claude writes passes
for one. An edit shows open, its first 40 lines numbered, and an open result its first
12 lines; each turn's time and cost is at the right. In the session's buffer, `TAB` folds
or unfolds the row at point, `RET` shows a tool call's whole result in `*result: NAME*`, which
agents cannot read, `w` copies a reply as the Markdown Claude wrote, `n` and `p` move between
your messages, and `]` and `[` between tool rows. Replies show without their Markdown's marks,
and a table as a box as wide as the window lets it, at most 100 columns, once its last row came (one
with too many columns for the window stays as written);
`M-w` copies what shows.
Long lines wrap there, in the composer and in the results; `C-x x t` in one of them cuts them at
the window's edge instead.
The `C-c` keys of a session's terminal work there too, except `C-c +` and `C-c -`. Claude
Code's Remote Control is off in native sessions until you turn it on for one; in the TUI
(`C-c a t`), your own settings decide. A note in the transcript says so if a claude says that it is
available all the same.

`M-x native-remote-control`, in a native session's buffer or composer while it is idle, connects it
to claude.ai, once you answered the question; the buffer beside it says what is sent. You then
follow it, send it messages and stop it from a browser or your phone; Claude's questions are
answered in oso alone. A mode set there can let calls run that oso would ask about: the modeline
shows it, and no later start takes it. `⇄` marks the session in the modeline and in `*Spaces*`.
The command again turns it off: claude ends and resumes, after a turn under way or 60 s at the
latest, and at once at a second press. claude.ai keeps what it has. `M-x
native-remote-control-address` copies the session's address there. `oso-restart` connects it again;
nothing else does. Sessions an agent started, and orchestrators, are not connected, nor one whose
git remote's address holds a password or a user name (but `git@`).

You write the next messages in the composer under the conversation, which agents cannot read; a thin line is above and below it.
The row under it says the permission mode, with the key that changes it, and the model. It changes
once `claude` says that it did. In a window too short for the row, the modeline says both.
While the composer is empty it says in grey what it is and its keys. `i` goes there and `C-c C-o` back, `C-c C-c` sends, and `M-p` and `M-n` bring back what you
sent, from any native session, also before a restart; past the newest, what you were writing comes
back. `C-c C-r` lists it all (`Past prompt:`), with the whole message beside, and `RET` puts the one
you choose where point is, not sent. The list is one for all sessions, so it goes across your projects
and spaces: what you wrote in one shows in another's. oso keeps the last 200 in a private file, in
clear text, as Claude Code keeps its conversations; never one from a composer that holds a secret
oso knows of. `M-x native-prompt-history-clear` takes them all out, and `prompt_history = false` in
[Session settings](#session-settings) keeps them in no file. A message shows at once at the end of the conversation, grey after `…`, until Claude says it has
it. If Claude's turn ends without that, it stays grey after `✗`; if `claude` itself ends first, the
conversation says so and the message is back in your composer if that is empty. After a turn, Claude's guess at your next message shows there in grey instead, with
`(C-c C-s takes it)` after it. `C-c C-s`, in the composer or the conversation, puts it at the
end of what you have written there, for you to edit or send, with a leading `/` or an `@`
escaped as below; after a command (`/`, `!` or `#` first), or an `@"` not closed, it puts nothing. `M-n` in an empty
composer puts it in too. When you come back to a session that finished
while you were elsewhere, the echo area says the first sentence of its last reply, as `NAME (last reply): …`. A message you send while Claude is working waits in a queue, grey at the end of the
conversation, and goes when the turn ends. On a queued message, `RET` takes it back into the
composer, `e` edits it where it is (`C-c C-c` puts it back, `C-c C-k` leaves it as it was), `d`
drops it, and `M-<up>` and `M-<down>` move it; `C-c C-q` goes to the queue. `C-c C-k`, in the
composer or the conversation, interrupts the turn under way; what is queued then waits until
you send again (`C-c C-c` with an empty composer sends the next). `C-u C-c C-c` interrupts the
turn and sends your message first, ahead of the queue. What oso sends a native session, with
`session-send`, a review's `C-c C-c`, a commit's draft or an orchestrator's offer, goes into
its composer where point is, for you to read and send; `C-c C-y` there puts the clipboard's
image in as `@` and its path. A review's `Q` question goes at once, or waits behind the turn
under way. Text oso pastes or puts in, in any session, has a `/` it starts with and an `@`
that starts a word escaped (` /`, `\@`), so that Claude reads it as text, not as a command or
a file to attach. Typing `/` in an empty composer, or
`C-c /`, lists the session's slash commands with what each takes; `RET` puts one in the
composer. `/clear` is refused there: start a new session instead. If the session
stops, what is queued stays there: take a message back with `RET`, or drop it with `d`. oso does not
restart while a composer or a queue holds a message: send, clear or drop it first. While a
native session has a turn under way, asks you something or is starting, or while git or a
file copy, move or delete runs,
`M-x oso-restart` waits for it: the echo area names what it waits for, nothing queued is sent
meanwhile, and the restart begins once all of it has been idle for 2 seconds. `C-g` or
`C-x C-c` cancels the wait. The new oso shows each conversation again in the windows that showed
it, at its end, with its composer selected if it was. `M-x session-resume` takes
a stopped session up again in its own buffer, and `M-x session-fork` copies it into a new one;
both start at once, with the composer ready. They need a project you trust, though no file of
it needs to be open; in a git worktree of a repository you trust, a resume needs none open
either, and a fork does. Trusting a repository so trusts its worktrees' Claude settings, hooks
and `.mcp.json`, as they are when a session resumes, also after `M-x oso-restart`. Otherwise the
resume says why, and the fork runs in a terminal. One that `M-x oso-restart` could not resume
says why in its conversation and under its row in `*Spaces*`.

When Claude asks to run something or to change a file, it asks at the end of the conversation: an
amber title (`⚠ Bash asks to run`) over all of it, the whole command, or the change as a diff, with its
tabs and full-width spaces marked. A line under the conversation has its keys. If the composer is empty, or you are at the
conversation's end, you are put on the line, which takes the empty composer's place. What you type in
the composer just then stays a draft there, and you with it; typed at the conversation's end it does
nothing. With a draft, with the
composer chosen (`i`) or with a Japanese input source you stay where you are, and the line says
`Bash asks to run   C-c C-j  answers it`: `C-c C-j` goes there (`C-x o` and a click do too). Clear the
draft, and half a second later you are put on the line. A long one shows its first rows. There `y` allows it once, but only once all of it was on screen, half a second
each page: `SPC` goes on, and `y` before that shows what is left. `C-u y` allows it unread, after a
question that says how much that is. `n` denies it, `N` denies it with a note Claude is told, `RET`
shows its input, and `C-c C-o` leaves it waiting. The line takes keys once you have typed none for half a
second, so that what you were typing never answers it; its keys are dim until then.

A question or a plan opens a framed card between the conversation and the composer, under the to-do
line if there is one, and the cursor moves to it at once, unless you are writing
in the composer (then `C-x o` from the conversation goes to it, past the to-do line): Claude's own words are inside the frame, on rows that
start with `│`, and the keys on its last row. The card takes the same half second. A question from
Claude shows all it asks and its options numbered: type the number, or `o` to answer in your own
words. A plan is approved with `y`. A question or a plan too long for the window shows a page and
how many rows there are; `SPC` goes on and `DEL` back, and an option can be chosen once it was on
screen. In the composer, while it is empty, `y`, `n`, `N` and a question's `o` and numbers answer
the card too, but not a call; once you have written something, every key is text. Elsewhere, the session shows
that it needs you, as a terminal session does.

When Claude keeps a to-do list, it shows as one line above the composer and what asks you something, `▸ Tasks (2/5)` and the
item it works on; `C-c C-t`, in the conversation or the composer, opens it to a row an item
(`[x]` done, `[/]` under way, `[ ]` not started), at most five, and folds it back. It goes once every item is done and the turn ends. Claude Code gives a
session its to-do tools only when you turn them on, in `~/.claude/settings.json`:

```json
{ "env": { "CLAUDE_CODE_ENABLE_TODO_TOOLS": "1" } }
```

## Dictation

oso dictates through [TypeWhisper](https://github.com/TypeWhisper/typewhisper-mac), which
records and recognises your speech on your Mac. Turn on its API server in TypeWhisper's settings.

- `s-d` (Option-D) starts a dictation where you are: a buffer, the minibuffer or a session's
  composer. The modeline says `● REC` while it records. `s-d` again stops it; the modeline says
  `…` until the text comes, and the text goes in where you started, at each cursor, in one
  undo step.
- Then Claude fixes what TypeWhisper misheard, such as a technical term written in kana. The
  text shows on a tinted band and the modeline says `fix` until the fix lands, as one more
  undo step: `C-z` once brings back what TypeWhisper heard. Fixes Claude is less sure of are
  only shown in the echo area. If you edit the dictated text meanwhile, the fix is skipped.
  `C-u s-d` types the text plain, with no fix. In a session's composer, `C-c C-c` while the
  modeline says `fix` waits for the fix and then sends the fixed text; `C-c C-k` drops the
  wait, and so does editing the message meanwhile.
- The fix runs your own `claude` (`[sessions] claude`), which sends the dictated text, a few
  lines around it and TypeWhisper's dictionary to Anthropic. It never runs for the minibuffer
  or a buffer that may hold secrets. `M-x dictation-cancel` stops a fix under way.
- oso learns, quietly, from each fix you keep or undo, and from your own small rewrites of the
  text within a few minutes, such as a misheard word you retype. Later fixes are told what it
  learned, so the same mishearing is fixed the same way. What you rewrite in text typed with
  `C-u s-d` is kept too, but never sent to Claude. It keeps at most 40 pairs, forgets one
  after 30 days unseen, and learns nothing from the minibuffer or a buffer that may hold
  secrets. `M-x dictation-forget-learned` forgets it all.
- A pair learned three times over two days or more goes to TypeWhisper's own dictionary, so that
  TypeWhisper gets it right itself: a term for a word it hears as kana, such as リアクト for React,
  or a correction. oso keeps at most 40 entries there and removes one that no dictation
  through oso used for 30 days. It never changes or removes an entry you added, and one of its
  own that you change, disable or delete is yours: oso does not add it again. One limit:
  TypeWhisper does not show oso entries you disabled, so a disabled entry for the same word is
  replaced. `M-x dictation-forget-learned` also removes what oso added, the next time you dictate
  if TypeWhisper is not running.
- What was learned from `C-u s-d` alone never goes to TypeWhisper's dictionary. TypeWhisper's own
  settings decide where its dictionary goes: a workflow that uses a cloud language model, a cloud
  recogniser or its sync can take it, with what oso added, off your Mac.
- `M-x dictation-learning-log` shows what was learned and added, and when, newest first.
- If you changed that buffer's text meanwhile, or the place is gone, the text goes to the kill
  ring instead, and `C-y` yanks it. While an input method composes there, the text waits.
- `M-x dictation-cancel` drops a dictation and its text.
- TypeWhisper pastes the same text itself with Command-V, which oso reads as `M-v`. oso drops
  an `M-v` while the text is due and TypeWhisper's copy is on the clipboard, a few seconds at
  most. TypeWhisper leaves the text on the clipboard unless its own settings put the clipboard
  back, also from a buffer that may hold secrets.
- Dictation never types into a terminal, a passphrase or a question, and does not run in a
  keyboard macro. It works only with TypeWhisper running: oso never starts it.

`[dictation]` in `config.toml` sets it up; each entry is optional:

```toml
[dictation]
# TypeWhisper's discovery file: this is where its release build keeps it.
discovery = "~/Library/Application Support/TypeWhisper/api-discovery.json"
# The curl oso runs to call TypeWhisper.
curl = "/usr/bin/curl"
# A TypeWhisper workflow to dictate in, by its id.
# workflow = "0F3C2B1A-1234-4ABC-9DEF-0123456789AB"
# The longest a dictation records, in seconds: 5 to 1800, 120 unless set.
max_seconds = 300
# The fix's model and effort, `sonnet` and `low` unless set.
fix_model = "sonnet"
fix_effort = "low"
# The lines on each side of the text the fix sees: 0 to 20, 3 unless set.
fix_context_lines = 3
# The least confidence a fix is made with: low, medium or high, medium unless set.
fix_min_confidence = "low"
```

## Developing beside it

In the repository, run:

```sh
just dev
```

This runs a dev build that rebuilds and restarts when the source changes, and keeps its state.
The installed oso has no dev features: `oso dev` and `oso ctl` work only in a dev build.

Both share your configuration file and your per-user state, so a change to
`config.toml` reaches both.

## Recent files

`C-x C-r` opens a file you visited recently, the most recent first, as Emacs's `recentf-open`
does. The file you are in comes last, so `C-x C-r RET` opens the one before. `C-x b` lists the
same files after the buffers, marked `file`, as consult-buffer does.

oso keeps the last 100 files you opened or looked at, by their paths. It never keeps a file
whose name may hold secrets, such as `.env` or anything under `~/.ssh`, and it never keeps a
file's text. A file you choose that is gone leaves the list. An agent cannot read the list.
To empty it, delete `recent-files` in the state directory below.

## Where things live

- Configuration: `~/.config/oso/config.toml`, or `$XDG_CONFIG_HOME/oso/config.toml` when that is
  set. oso never writes this file.
- Trusted projects: `trusted-projects`, next to `config.toml`. oso writes it when you trust or
  untrust a project.
- State that oso keeps by itself (emergency copies, the theme chosen last, the sessions to bring
  back, the recent files):
  `~/Library/Application Support/oso` on macOS, `$XDG_STATE_HOME/oso` or `~/.local/state/oso` on
  Linux.
- Images you attach to a session with `C-c C-y`: `~/Library/Caches/oso/images` on macOS,
  `$XDG_CACHE_HOME/oso/images` or `~/.cache/oso/images` on Linux, one directory a session. They
  go when you archive the session, or after a week; a session an agent archived keeps them
  until the week ends.

## Update

```sh
git pull
just install
```

The running oso keeps the old binary. Quit it and start `oso` again when convenient.

Quitting ends oso's terminals and its sessions' `claude` processes. The next start brings each
session back stopped, with its buffer and its place in `*Spaces*`, and starts nothing. Press
`C-c a r` in its buffer, or `r` on its row, to resume it. A session you archived stays away.
