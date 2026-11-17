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
each row `path:line: text` with a preview of its place; `RET` goes there. A file outside any
project that they lead to, such as the standard library's, joins the server until its buffer is
killed. `C-M-i` completes the word before point in a popup under it, as Corfu does, and so does
typing three letters of a word and pausing, or one of the server's characters such as `.`: typing
narrows it, `C-n` and `C-p` move (from the last back to the first), `TAB` inserts, `RET`
inserts once you have moved, and `C-g` or any other key closes it. An item that needs a `use`
line adds it as it goes in; when that line is out of view, the echo area says which it is. The rest comes in later
steps.

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

A session is Claude Code running in an oso terminal, in a buffer named `*claude: NAME*`. oso
tracks its state (needs input, working, done, stopped and so on), and gives it oso's own tools
through `oso mcp`.

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
| `O` | `space-new` | Make a space for an issue, of the selected buffer's repository. |
| `S` | the space menu | Add or remove a repository, instructions, directory, rename, close, reopen. |
| `K` | `session-stop` | Stop this session. It asks first while the session works or needs input. |
| `P` | `session-pin` | Pin or unpin this session; dim outside a session's buffer. |
| `A` | `session-archive` | Archive this session, or bring it back; dim outside a session's buffer. |
| `v` | `review-session` | Review what changed in this project since its baseline. |
| `V` | the review menu | The same menu as `C-x v r`. |

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
| `C-c C-w` | Start a session in a new worktree of this session's project. |
| `C-c C-e`, `C-c C-f` | Explain this session's state; show who sent it its newest message. |
| `C-c C-z` | Zoom the selected window. |
| `C-c C-v` | Review what changed in its project since its baseline. |
| `C-c @` | Insert a session's name as `@NAME`. |
| `C-c +`, `C-c -` | Let the agent channel read its screen, or take that back. |
| `C-c C-u` | A prefix argument for the next key, as `C-u` elsewhere: `C-c C-u C-c a m` sends the place only. |
| `C-c C-y` | Attach the image on the clipboard: oso keeps it in a private file and pastes `@` and its path into the prompt, for you to press Enter. A TIFF is converted to PNG. macOS only for now. |
| `C-c ?` | Say these keys in the echo area. |

The other heads have no such key, as the terminal or Emacs keeps theirs: `C-c a c` starts another
session, `C-c a m` sends, `C-c a j` switches spaces, and `C-c a K`, `C-c a P` and `C-c a A` stop,
pin and archive this session.

`C-c C-c` sends Claude Code a `Ctrl-C`, and `C-c C-k` switches char mode, where every key goes to
Claude Code. Any other `C-c` key that is not listed here sends nothing. To change these keys, see
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

A session starts in the mode your own Claude Code settings give it: oso passes no permission
option. It starts the agent channel, which its tools need.

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
asks. Marks are coloured by what they ask of you:
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
and asks the desktop for your attention.

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
| `A` | Archive, or bring an archived session back. |
| `r` | Resume a stopped session. |
| `k` | Stop the session. It asks first while the session works or needs input. |
| `e` | Explain its state. |
| `?` | Say what the marks mean, and these keys. |

`C-c a c` and `C-c a w` in the list work as `c` and `w`. `C-x o` skips the list. `C-x k` in a
session's buffer stops and archives the session.

### Spaces

A space is the workplace of one issue: its repositories, the worktrees made in it, their
sessions, and its own windows. oso keeps spaces from one start to the next. `C-c a j` switches
spaces, and the windows of the space you leave are kept for when you come back. Showing a
session of another space switches to that space. A worktree made in a space stays in it. A
session started in a repository joins the space in front when that space has the repository,
else the space you were in last that has it, else a new space of that repository. Each
repository you used before spaces came becomes a space of its own. The modeline names the
space.

`C-c a O` makes a space for an issue: type its title, and the space is named after it, with the
selected buffer's repository; with none, it asks for one. Each space gets a directory of its
own, `~/spaces/<name>` (set `[spaces] root` in `config.toml` to put them elsewhere), where its
orchestrator will work. oso makes it readable by you alone, and refuses a root that other users
can write.

`C-c a S` opens the space menu for the space in front:

| Key | Command | What it does |
|---|---|---|
| `a` | `space-add-repo` | Add a repository, by completion or its path; it must be a main checkout. |
| `r` | `space-remove-repo` | Take a repository out; refused while a session of the space runs in it. |
| `u` | `space-use-repo` | Choose the repository that `session-worktree`, `worktree-list` and `worktree-remove` use from the orchestrator's buffer, when the space has several trusted ones. |
| `i` | `space-instructions` | Edit the goal the space's orchestrator is given. |
| `d` | `space-directory` | List the space's directory. |
| `n` | `space-rename` | Give the space a new title, and a name made from it. |
| `c` | `space-close` | Archive the space; refused while one of its sessions works or needs input. |
| `R` | `space-reopen` | Open a closed space again. |
| `o` | `space-orchestrator-start` | Start the space's orchestrator, the session you talk to about the issue, in the space's directory, with each trusted repository as a working directory. `C-u` makes a space directory that is gone again at its place. |
| `O` | `space-orchestrator-replace` | Replace the orchestrator by a new one that takes over from it, once it is idle: the old one is stopped and kept as `NAME-was-N`, and the new one is told to read `handoff.md` first. Asks first when there is no `handoff.md`. |

The orchestrator is told the space's repositories, its title and its instructions, and to
keep a `handoff.md` in its directory for whoever takes over. Its row is right under its
space's, and `RET` on the space's row shows it. A repository added to the space, or trusted,
while it runs is offered to it: `y` pastes `/add-dir PATH` into its prompt, not sent. In
Claude Code, `/permissions` and its Workspace tab list the directories it was given.

The orchestrator works through sessions: its tools name a repository of the space with
`repo`, and it sees the space's sessions only. A worker it starts belongs to the space, and
its brief says to report to the orchestrator by the space's name. `space_status` gives any
session of the space the space, its repositories with their trust, and its sessions. With
`with_dirs`, the orchestrator lets a worker also read another trusted repository of the space or
one of its worktrees; the question you answer lists them.

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
those of the last 30 days. Pick one, and oso resumes it in a new session buffer. Stop it in the
other program first: oso cannot move a running process, so it refuses a live one and offers a
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
sessions there, never forces, and never deletes the branch. While a removal runs, its row in
`*Worktrees*` says `removing…`. `M-x worktree-remove-stop` stops it: before git runs, nothing is
removed; once git may be deleting, it asks first, as the worktree is then left half removed.

## Review changes

`C-x v r` opens the review menu for the current file's project:

| Key | Reviews |
|---|---|
| `D` | What changed since the baseline (see below) |
| `w` | Uncommitted changes, untracked files included |
| `u` | Unstaged changes |
| `s` | Staged changes |
| `b` | The branch against its base |
| `c` | One commit |
| `r` | A range of commits, `FROM..TO` |
| `B` | Take a new baseline now |

Each is a command too, such as `M-x review-uncommitted`. Reviews run git, so the project must
be trusted. `C-c a c` takes a baseline when it starts a session at the root of a trusted
project, so `D` shows what that session changed.

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
| `\|` | Two panes, the old side and the new side lined up, or one stream again |
| `g` | Read the files again now |
| `C-c C-c` | Gather your comments not sent yet into a message to the session |
| `q` | Bury the review |
| `?` | List these keys |

A comment opens as a draft at the top: `RET` breaks a line, `C-c C-c` keeps it, and `C-g` drops
it into the kill ring. `C-x k` closes the review, and asks first while it holds comments you
have not sent.

`C-c C-c` in the review gathers your comments that were not sent, outside threads you resolved,
into `*review message*`: each with its file, its line or hunk and the hunk's lines. Read it and
change it as you like, then `C-c C-c` there pastes it into the agent session that runs in the
review's worktree, without pressing Enter: read it in the session and press Enter there.
`C-u C-c C-c` picks another session, which the review then keeps; with no session the message
stays, and the echo area says how to pick or start one. `C-c C-k` drops the message. A message
holding text yanked from a secret is not sent.
Sent comments are drawn dimmed, and an edit makes one unsent again. A session that is asking you
something takes no paste: answer it first, and the message waits.

`|` shows the review as two panes that scroll together: the old side at the left and the new side
at the right when the review's area is 160 columns wide or more, else one above the other.
Blank rows keep the panes lined up; the cursor skips them. A comment shows in the pane of its
side, a hunk's in the new side's. `|` again, or `q`, goes back to one window.

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
| `N` | The pull request menu, through `gh`: `v` the branch's pull request, `k` its checks, `o` open it in the browser, `c` create one (`- d` a draft, `- b` into a base you name), `m` merge it, `l` the open ones |
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
| `worktree_list` | List the repository's worktrees, as `M-x worktree-list` does. |
| `worktree_remove` | Remove a finished worktree, under the same rules as `M-x worktree-remove`. |
| `space_status` | Read the session's issue space: its repositories, their trust, the orchestrator's name, its sessions. |
| `draft_set` | Give back the commit message draft `C-c C-d` asked for. It goes in your message buffer; nothing is committed. |

A tool that starts a session asks you in oso first. The question names the session that asks
and what would start. `y` allows it, `n` or `C-g` refuses it, and `?` shows the whole brief. A refusal
holds for the same request for a minute. After 20 such calls, a session's tools wait until you
type in its terminal. To let sessions you started start sessions without the question, see
[Session settings](#session-settings).

Claude Code run in a plain `M-x terminal`, or outside oso, is not a session. If you gave it
`oso mcp` as an MCP server yourself, it reaches oso only after `M-x agent-channel-start`, and
each connection asks you first. Of the session tools, it gets only the reading ones.

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
agents_start_without_asking = true   # let your sessions start sessions with no question (off by default)
agents_start_limit = 4               # how many such sessions each may run at once, 1 to 8, or "none"
pane_width = 52                      # the *Spaces* pane's columns, 24 to 80 (44 by default)

[sessions.notify]                    # which states ask for your attention
needs_input = true
done = true
failed = true
ready_for_review = false
```

`steer_worktrees` gives each new session a hook that turns down Claude Code's own worktree
tools and points the agent to `start_worktree_session`, so its worktrees show in oso.
`spinner = false` stops the working marks from turning, and with them the only timer the list
runs. `pane_width` is where the pane starts; `<` and `>` in it change it for this oso only.
The `[sessions.notify]` values shown are the defaults.

`agents_start_without_asking` lets a session you started, with a key or a `y`, start sessions
through `start_worktree_session` and `session_start` without asking you, in a trusted project
only, up to `agents_start_limit` running sessions each; past that it asks again. With
`agents_start_limit = "none"` no limit makes it ask, but a session can still start at most 20
sessions each time you type in it. A session started this way asks for its own. A session an
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

## Developing beside it

In the repository, run:

```sh
just dev
```

This runs a dev build that rebuilds and restarts when the source changes, and keeps its state.
The installed oso has no dev features: `oso dev` and `oso ctl` work only in a dev build.

Both share your configuration file and your per-user state, so a change to
`config.toml` reaches both.

## Where things live

- Configuration: `~/.config/oso/config.toml`, or `$XDG_CONFIG_HOME/oso/config.toml` when that is
  set. oso never writes this file.
- Trusted projects: `trusted-projects`, next to `config.toml`. oso writes it when you trust or
  untrust a project.
- State that oso keeps by itself (emergency copies, the theme chosen last, the sessions to bring
  back):
  `~/Library/Application Support/oso` on macOS, `$XDG_STATE_HOME/oso` or `~/.local/state/oso` on
  Linux.
- Images you attach to a session with `C-c C-y`: `~/Library/Caches/oso/images` on macOS,
  `$XDG_CACHE_HOME/oso/images` or `~/.cache/oso/images` on Linux, one directory a session. They
  go when the session is archived, or after a week.

## Update

```sh
git pull
just install
```

The running oso keeps the old binary. Quit it and start `oso` again when convenient.

Quitting ends oso's terminals and its sessions' `claude` processes. The next start brings each
session back stopped, with its buffer and its place in `*Spaces*`, and starts nothing. Press
`C-c a r` in its buffer, or `r` on its row, to resume it. A session you archived stays away.
