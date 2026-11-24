# Security

This document describes oso's threat model and the measures that follow from
it. oso has no user scripting and no plugins. That removes a large class of
attacks, but it does not remove all of them.

## Threat model

The main scenario is **opening an untrusted repository** (open-source or
third-party code) and having something run because of its contents. Other
threats:

| # | Threat | Examples |
|---|---|---|
| T1 | Content-triggered execution | modelines, file-local variables, project config pointing to executables, `.git/config` `core.fsmonitor`, `.gitattributes` filters, language servers running `build.rs` or proc macros |
| T2 | Supply chain | malicious or compromised crates; `build.rs` and proc macros run on every build, and dev hot reload builds often |
| T3 | Display spoofing | bidirectional control characters (Trojan Source, CVE-2021-42574), invisible characters, homoglyphs |
| T4 | Secrets leaking to disk | auto-save and backup files, session data, crash dumps, persisted undo, clipboard |
| T5 | Unsafe file writes | following symlinks, partial writes, wrong permissions |
| T6 | Process launching | PATH hijacking (on Windows the current directory comes first), URL handlers |
| T7 | Terminal escape sequences | clipboard reads (OSC 52), title or report sequences that echo input back, spoofed hyperlinks (OSC 8), output that asks the editor to run a command |
| T8 | Denial of service | huge files, very long lines, regex backtracking |
| T9 | Agentic development | prompt injection through read content, casually added dependencies or `unsafe`, dev-only entry points leaking into release |
| T10 | Untrusted parsers | the terminal engine; image and PDF decoding (later milestones) |

Out of scope: remote file editing (there is no TRAMP equivalent) and any
network features.

## Principles

1. **Opening never executes.** Opening a file or project triggers no external
   process and no code from the project.
2. **Enforce mechanically.** Rules that a compiler or lint can check are
   checked by tooling, not by convention.
3. **No network code.** oso contains no networking. Network access happens
   only in child processes the user starts explicitly, such as `git push`.
4. **Minimize and audit dependencies.**

## Measures

### Project trust (T1)

- Projects open in **restricted mode**. A modeline indicator shows the
  mode, and no popup asks about it.
- A project is the nearest directory above a file holding `.git`, `.jj` or
  `.hg`, found from metadata alone, following no link, on a thread of its own.
  The walk stops at the home directory, at a directory or marker another user
  owns (root aside) and where the device changes. Windows has no owner or
  device to go by, so there only the home directory (`USERPROFILE`) and a link
  stop it ([ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md)).
- A git worktree takes its main repository's trust. Its `.git` file's claim is
  matched by its words against the trusted roots before anything it names is
  looked up, so a path a repository wrote, a UNC one on Windows among them, is
  looked up only under a root the person trusted. Then the main repository's
  `.git/worktrees/<name>/gitdir` must name the worktree's own `.git` back. Both
  files are read at most 1 KiB, and neither the worktree's place nor its `.git`
  may be a link, looked at again after the reads.
- `M-x trust-project` marks a project as trusted. It asks first, naming the
  root. Only the person's `y` to that question confirms, and it checks that the
  root, by path and identity, is still the one asked about; run any other way,
  the command asks. `M-x untrust-project` takes the line out without asking.
  Both are `Person`, so the agent channel never runs them.
- `C-x p f` walks a project's files in-process on its own thread, `oso-walk`,
  and runs nothing. It reads no directory above the project's root. Each
  directory below the root is opened from its parent by its name, following no
  link, and listed and read from what was opened, never by its path again
  (`oso_platform::file_meta::Directory`); at most 32 stay open, and one let go
  is opened again the same way only if it is still the directory it was, by
  device and inode. A walk enters at most 256 directories deep and 4096 bytes
  of path, and a directory it cannot open or list for a reason other than its
  being gone, refused or no directory ends it with `+`. Links are never followed: a link is resolved by its words,
  each hop looked at from the directory before it, only while every hop stays
  under the root, and one that leaves it is listed without a look. Opening any
  row goes through the loader's verdict, so a file that may hold secrets opens
  as such a buffer. On Windows directories are still reached by their paths.
- `M-s r` and `C-x p g` search the same files in-process, on `oso-grep` and up
  to eight searching threads, and run nothing. A search holds about 70
  descriptors at most: the walk's 32, and the directories of the 16 files
  waiting and the 8 being read, with those files. Like the walk, it never
  searches a root that holds the home directory, by the real place of both, and
  a directory named with `C-u` that is home, holds it, is `/` or has `..` in it
  is refused. A file is opened from its
  directory as the walk opened it, following no link and waiting for nothing,
  and only a regular file is read; a link is never searched, so a link in a
  repository cannot make a search read a file outside it. A line longer than
  16 MiB skips the rest of its file, a NUL byte stops it, and a search keeps at
  most 10,000 rows of at most 512 bytes of text. A file whose name may hold
  secrets, as the path walked or by its path from every place the root leads
  through, as the loader finds them, is searched, but its rows carry no text
  and are never previewed: nothing of it is read for a preview. A root that
  cannot be resolved counts as one that may hold secrets, and the root opened
  must be, by device and inode, the place judged, reached again from `/` a name
  at a time following no link, or nothing under it is read.
  `M-s l` (`consult-line`) searches only the text of the buffer shown, in
  memory, on `oso-lines`, and reads no file; it is `Person`. Its rows are
  that buffer's text even when it may hold secrets, which the person sees
  already, but then nothing of the prompt reaches `get-state` or the
  history, and its preview never shows
  ([ADR 0154](adr/0154-consult-line.md)).
  A preview reads a row's file on `oso-preview` as the search reached it: the
  root by its path with the same home checks, then each directory and the file
  from the one before, following no link and waiting for nothing, a regular
  file only, judged again as the search judges it, from the root opened only if
  it is the place judged. It reads at most 64 KiB before the row's line and
  16 MiB in all, keeps at most 256 lines of at most 4 KiB of the file's bytes,
  and for a diagnostic of `M-g d`, which has its line but no offset, first
  counts line breaks from the file's start within 64 MiB (ADR 0138); it
  opens no buffer, and an open buffer of a file
  that may hold secrets is never shown in it. A file that could not be opened for a
  reason other than its being gone or refused ends the search with `+`. On
  macOS a file a sync service keeps elsewhere is not read, as reading would
  fetch it.
  Nothing of the input or the rows reaches a log, and `Debug` of a request, a
  row or a batch gives a generation and a count only. The lines a search that
  ended without a limit found, at most 16 MiB of them with their paths,
  counted as they are found, stay on `oso-grep` for a search that narrows it.
  The next search that ends replaces them, and they go once no search came for
  30 s. The rows of a list of a language server's places (`M-.`'s several,
  `M-?`'s) read each file's lines the same way, on `oso-xrefs`, within 64 MiB
  of a file and 256 MiB of a list, and show nothing of a file that may hold
  secrets; a file outside the server's root is reached from its own directory,
  so one right in a directory that is or holds the home directory is not read
  (ADR 0138, step 8b).
- The walk reads the project's ignore files itself, as patterns only: only a
  regular file that is not a link, opened without waiting, of at most 1 MiB.
  The person's global excludes may be a link but must be such a file too. They
  can only hide files. The markers' directories and nested projects, counted
  as detection counts them, are never entered. At most 100,000 files and
  32 MiB of paths are listed, and at most 1,000,000 entries, 32 MiB of names
  and 16 MiB of ignore files read. Nothing of the list reaches a log: the
  walker's crates write only warnings
  ([ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md)).
- Trust is stored in `trusted-projects` in oso's configuration directory, by
  the root's path, device and inode (the path alone on Windows), never inside
  the repository. A dev build given `--config FILE` uses the one beside that
  file, and `--no-config` reads none, so nothing is trusted.
- The file is read on a thread of its own, `oso-trust`, with
  `config.toml`'s checks, at start, once a second when it changed, and when oso
  gains the focus or saves. It is refused when anyone else, a group included,
  can change it or the way to it. A refused or broken file trusts nothing,
  `*Messages*` says why, and the commands refuse rather than write over it.
- oso changes the file on that thread: what a person changed is made in the
  file as it is then, read with the same checks, so a line written by hand
  since stays and a file refused since is not written over. It goes through a
  part file of mode `0600`, renamed over the file, only in a directory no one
  else can change, and never over a link, which a dotfiles setup may make.
  Each write is read back, so what the modeline says is what the file says.
  Changes not yet made when oso goes are waited for, a second at most.
- A path in the file holds no control or invisible character, so a line reads
  in another editor as it does in oso.
- The modeline says `restricted` from the record alone, never from a name.
- Trust means "I accept running this project's code on my machine".

| Capability | Restricted | Trusted |
|---|---|---|
| View, edit, save | yes | yes |
| Syntax highlighting (in-process tree-sitter) | yes | yes |
| Git status display (in-process `gix`) | yes | yes |
| Terminal (explicit user action) | yes | yes |
| Finding and searching the project's files (in-process) | yes | yes |
| Language servers | no | yes |
| Formatters, format on save | no | yes |
| Git write operations (commit, push, rebase; hooks run) | no | yes |
| Agent review (the `git` CLI; no hooks or monitor, clean filters run) | no | yes |
| Org babel execution (explicit action only) | no | yes |

- Inside repositories, oso reads only EditorConfig. It never reads settings
  that name commands or executables. EditorConfig sets a file's indentation and tab width
  and nothing else, so it is read in every project, trusted or not. The walk up from the file
  stops where project detection stops (a link, another user's directory, another file system,
  after the home directory), and a `.editorconfig` is read only if it is a regular file, no
  link, this user's or root's, not writable by every user, of at most 64 KiB, opened from its
  directory once that is the one the walk saw, and never in a directory every user may write.
  A visit looks in at most 64 directories and reads at most 32 such files and 1 MiB of them. Parsing is bounded: 1000 sections, a
  glob of 4 KiB with braces 8 deep, 128 bytes for one with a number range, the `regex` crate's
  size limit, and at most 128 sections and 2 KiB of globs compiled a visit
  ([ADR 0115](adr/0115-indentation.md)).
- The user configuration is one per-user file, never searched for elsewhere.
  It names registered commands and values only, is read off the UI thread with
  checks on its owner and modes, and a bad one never stops a start. Its agent
  channel setting starts the channel at launch, and each connection is still
  asked. The channel never saves into oso's configuration directory
  ([ADR 0108](adr/0108-the-user-configuration-file.md)).
- Migemo's dictionary is a file only `[search] migemo-dictionary` in the user configuration
  names, by an absolute path or `~/` and a relative one from an absolute home directory,
  never a path on another computer, as written or as the home directory makes it; nothing in a
  project names one. It is read on `oso-migemo` at the first search that
  would use it, with the configuration file's checks (through links, refused if other users can
  change it or the way to it, a regular file, at most 64 MiB of UTF-8; one a group can change
  is read with a warning). Each word enters a search's regexp through `regex::escape`, with
  bounds on readings, words and a pattern's size, and neither a word nor the path reaches a
  log or `*Messages*` ([ADR 0116](adr/0116-migemo-and-jumps.md)).
- There are no modelines and no file-local variables that execute anything.
  Org links of type `shell:` or `elisp:` are not supported.

### Git (T1)

- Status, branch and diff gutter are read in-process with `gix`. Commands
  defined in config are never run: `core.fsmonitor`, filter and smudge/clean
  drivers, textconv, external diff. Verifying that `gix` can be forced to skip
  all of these is an open item.
- Write operations call the `git` CLI through the process module, and only in
  trusted projects. So does an agent review (diffs, file lists, blobs, and the baseline's
  tree), with revisions resolved to object ids, option-like ones refused,
  `--no-ext-diff --no-textconv`, and `GIT_OPTIONAL_LOCKS=0`
  ([ADR 0122](adr/0122-agent-review.md)). Restricted projects get no review: its thread starts
  only from a root the editor trusts, and stops when that trust is taken back. Every review
  run sets `core.fsmonitor=false` and `core.hooksPath` to an empty private directory, so no
  hook or file system monitor the repository names runs. Clean filters do run, as `git add`
  and a diff of the work tree need them (Git LFS is one): a trusted project's filter programs
  run in its review. No run enters a repository embedded in the work tree, a project of its
  own: diffs ignore a submodule's work tree, and the adds leave every submodule out.
  Untracked files are listed through a copy of the index, so the person's index is never
  written, and a file whose name may hold secrets, or a work tree file with more than one
  name, is listed and never read.
- A review's comments reach an agent only as the person's paste (ADR 0122, step 12): every
  command of it is `Person`, the message is shown first in `*review message*`, where the agent
  channel's `Change` commands are refused, and it goes through `send_to_session`, cleaned and
  not submitted, refused while the session asks for an answer. A file whose name may hold
  secrets, or whose open buffer may, sends its place, never its lines, and a message turned
  secret by a yank is not sent. A hunk's line has its control characters and line separators
  escaped, so that a paste cannot break it into lines that read as another comment. oso puts
  no message in the kill ring or on the clipboard, even with no session to paste into: what
  the person copies from it is theirs.
- A commit ([ADR 0127](adr/0127-source-control-and-github.md), step 1b) runs the repository's
  own hooks, only in a trusted project and only as the person's act: every command of it is
  `Person`, and while a commit's message is in front the agent channel's `Change` commands are
  refused. A hook gets the process module's allowlist, `XDG_CONFIG_HOME`, the SSH agent's and
  GnuPG's variables, and the names `[git] pass_env` lists, which take no `GIT_*` name, with
  `GIT_TERMINAL_PROMPT=0` and `GIT_EDITOR=:`. It has no time limit; git runs in a process group
  of its own, which `C-g` ends, as revoking the project's trust does, and which oso's exit sends
  `SIGTERM`, then `SIGKILL`; once git has ended, its group gets `SIGKILL` before git is reaped,
  so that a hook that ignores `SIGTERM` does not outlive it. A git status's writes to the index have no time limit either, still with no hook,
  and `C-g` stops them. What git and its hooks write goes to
  `*git log: NAME*`, escape sequences and control characters taken out, and that buffer is
  withheld from the agent channel. File names in a message's `#` lines are escaped on one line.
- A push, a pull or a fetch from a git status (ADR 0127, step 2) is the person's act, `Person`,
  in a trusted project, with the repository's hooks and a commit's variables plus the proxies.
  It starts in a session of its own with no controlling terminal (`Spawn::own_session`), so
  that `ssh` or a credential helper cannot ask on the terminal oso was started from, unseen;
  `GIT_TERMINAL_PROMPT=0` stays. A push names its refs in full after `--` and the remote. A
  remote typed or picked in a prompt must be one `git remote` lists; one from the repository's
  configuration is taken as git takes it, and an address is shown with its user and password
  left out. A branch name is refused before git sees it if it could read as anything else, or
  holds a control or bidirectional character. A forced push is only `--force-with-lease` with
  the tip oso read, after a question naming what it drops (or that oso could not tell), and is
  refused if the branch moved since; the question and the prompts take an answer only for the
  status they were asked over, and one closed without it is forgotten. Subjects are shown
  escaped on one line, and the server's own lines stay out of the echo area.
- The graph (ADR 0127, step 4) runs only reads, with a read's git, in a trusted project, on a
  thread of its own: every page bounded, its records parsed as untrusted bytes, ids checked as
  hex, and subjects, authors and ref names escaped on one line. Its commands are `Person`: they
  run git, open a review or write the clipboard. A ref typed for `l o` must not read as an
  option; one picked goes to git as the full ref listed. Its next page is read only for a
  person's key in it, never for an agent's command that moved point, and a project no longer
  trusted has the graph's root revoked at once, so that nothing queued on its thread runs git.
  An agent may read a graph's rows over the channel, as a status's: commit metadata, no file's
  text, but subjects, authors and ref names are anyone's words, and reach it as data. Its keys
  that change the repository (step 4b) run on the run thread with the repository's hooks: a
  detached checkout, a pick and a revert ask first, bound to the graph and its prompt; ids go to
  git whole and a tag's name passes the branch's filter; a checkout, a pick and a revert are
  refused while something is stopped, judged on the run thread, and when oso cannot tell. Only
  a label the push's filter takes is offered to check out. These runs honour `refs/replace/`,
  as the person's own git does, where the graph's reads do not.
- A pull request's review (ADR 0122, step 12b) asks `gh` as the pull request buffers do, from
  the review's thread, only for the person's `p`, `r` or `g` in a trusted project, never on a
  timer. Only the whole commit ids it gives reach git; its base branch reaches git only as a
  plain ASCII branch name, so no refspec of GitHub's can name a ref to write. The side the
  clone lacks is fetched from the branch's remote, else `origin`, with the review's empty
  hooks directory and reads' variables, the proxies, the SSH agent and no controlling
  terminal (`Git::for_fetch`), as objects alone (`--refmap=`, `--no-write-fetch-head`), so no
  ref moves and `FETCH_HEAD` names nothing of a fork's for a later merge; with no tags,
  submodules or upkeep. The review's stop ends the fetch and the wait for `gh`; a fetch still
  running as oso quits goes on, with no bound, until it ends or fails. A failed fetch shows
  git's own last line, never a server's own error or `remote:` text. The work tree stands for the pull request only when
  the head branch checked out holds its head commit, so a branch behind it hides no pushed
  commit, and comments go to a worktree only under the same rule, and to the review's own
  session after a failed refresh. The branch and title are escaped in the echo area, after the
  fixed words.
- Pull requests (ADR 0127, step 5a) are read through `gh`, the person's: in a trusted project
  only, on a thread of the repository's own, `gh` run in the empty private directory, never in
  the repository, with `--repo` naming the repository the branch's remote's address gives, on
  `github.com` or the host `GH_HOST` names only, its token passed by name to that host alone, and
  each run killed after 30 s, the root looked at again before each. The head line is read only
  for the person's `C-x g` or `g`, at most once every 30 s, and after oso's own push or checkout,
  never for an agent's command; a pull request of another head branch is none. What GitHub gives is anyone's words: escaped on
  its rows and cut, never acted on, and a review thread's file is visited only at a plain
  relative path under the root, and `w` copies only a plain `https` address. Every character the
  display boxes is escaped in what an agent reads too. The system's opener
  (`oso_platform::process::open_link`) takes only an `https` address of plain characters, finds
  its program and helpers with the project left out of `PATH`, and oso opens only a pull
  request's own address on the repository's host, after the echo area shows it. Every command
  is `Person`; an agent may read `*pr: #N*` through `text`, as data. Making and merging (step 5a-2) are the person's
  runs on the run thread: `gh pr create` and `gh pr merge` with every value bound to its flag
  and the body on standard input, each value quoted in the log, the branch's remote and the
  project's trust looked at again before `gh` runs. The push before one goes to the remote's
  branch of the same name, never to an upstream of another name, and the pull request follows
  that push alone. A merge comes only after a question naming the pull request, its repository,
  its base, the method and its head; just before it the run thread reads again that it is open
  and into that base, and `--match-head-commit` makes `gh` refuse it if its head moved. A
  question or a message `gh` answers late opens nothing over what the person does now. A pull request's message is
  the person's: an agent's command does not change it. Checks and review threads sent to an
  agent (step 6b) are read again when the person asks, shown in `*review message*` under the
  untrusted line before anything is pasted, every line quoted or fenced and escaped; only the
  person's and the repository's owners', members' and collaborators' comments go in by default,
  the pull request's author no more than another, and a fork's logs are left out; `C-c C-t`
  takes them in above the end of what GitHub said, never among the person's words. Every line is
  escaped as the display boxes it, bidi and invisible characters too, so the preview shows all
  that is pasted. A secret-named file's hunk is not sent. A log is read only of a GitHub Actions
  job of the repository itself, by the run and job its link names, at most 8 a message, each
  within 10 s and 4 MiB. An issue's brief (step 5b-1) is its text
  under the line that tells the agent it is data, every line escaped, cut at 64 KiB, with only
  the comments of its author, of the login `gh` is logged in as and of the repository's owners,
  members and collaborators, each line quoted so that none passes for oso's or the person's, a
  deleted account never counting as the author; the person reads it in `*Brief*` before it is
  sent. An agent may read `*issue: #N*` through `text`, every comment in it, as data, as a pull
  request's. Only an issue's own address opens. Each worktree's pull request (step 5b-2) is
  read with the person's listings, at most once a minute a repository for the tree, never for an
  agent's `worktree_list` or `sessions-refresh` and never when the worker sees the worktrees
  change. A fork's pull request is never taken as a branch's, here or in the status's head line
  and `N m`: its head branch may be named as the person's. A template is read only as a regular
  file under the root, never through a linked directory, at most 64 KiB, never one whose name
  may hold secrets.
- Stashes and branches from a git status (ADR 0127, step 3b) are the person's acts, `Person`,
  with the repository's hooks: a checkout runs `post-checkout` and the filters, as git's own
  does in a trusted project. A stash is named by its commit's id: an apply gives git the id, and
  a pop or a drop reads git's own `stash@{N}` for it again, with a read's git, just before git
  runs, as stashes are shared by every worktree and agent session of the repository; a prompt's
  answer names a stash the prompt listed. The milliseconds between that read and git's run are
  left open. A branch name typed or picked passes the push's filter before git sees it, which
  refuses invisible characters too, a branch or a start picked goes to git as the full ref
  listed, so that no tag of the same name stands in for it, and a start typed must not read as
  an option. A list the run thread gives late opens no prompt unless the person is still in that
  status. Dropping a stash and deleting a branch that is not merged ask
  first, escaped on one line; a forced delete runs only while the branch is on the commit the
  question saw, and only for the status it was asked over. A branch checked out in a worktree
  is never deleted.
- What is stopped in a repository (ADR 0127, step 3a) is read from the files git keeps in the
  work tree's own directory, each a regular file of at most 4 KiB, opened following no link and
  waiting for no writer, as untrusted bytes: an id must be a whole one, 40 or 64 hex digits, and
  nothing read reaches a command line but those ids. The questions escape what git gave on one
  line, as the head does. The reads around a run are a read's (no hook, no lazy fetch, no
  proxy).
  Continuing, skipping and aborting are the person's acts, `Person`, with the repository's
  hooks, `GIT_EDITOR=:` and `GIT_SEQUENCE_EDITOR=:`. A skip or an abort asks first, naming what
  goes; its `y` runs only for the status it was asked over, and the run thread refuses it
  unless what is stopped is the one asked about: its kind, commit, branch, start, progress and
  where it goes back to. A continue runs what the person's own todo list
  holds, `exec` lines too, as `git rebase --continue` in a terminal would. A pull's `-r` and
  `-m` say how in whole (`--rebase`, `--no-rebase --ff --no-edit`), so that no configuration
  makes either interactive.
- Discarding in a git status (ADR 0127, step 1c) asks first, and keeps each file as an
  unreachable object first (`git hash-object -w --no-filters`), but a file that may hold
  secrets, as a loose object can be read more widely than a private file. The record of the
  copies, in oso's state directory, is private and holds ids and paths, never contents. Paths
  from git are deleted or written only as plain names under the project's root, through no
  link and through none of git's own directory, and writing a copy back makes its file there
  with no link followed, and keeps what the file held as another copy first. A file with more
  than one name, or for its owner's eyes only, is not discarded, as its copy could expose it.
- The recent files (ADR 0148), `recent-files` in oso's state directory, are private (`0600`)
  and hold paths and times, never contents. Only a file judged `Ordinary`, after the loader
  has seen where its path leads, goes in, and the list read back is judged by its names again.
  The agent channel neither reads it nor opens a file from it, and nothing on it is looked at
  to list it.
- Saved reviews (ADR 0122, step 6), `reviews/*.review` in oso's state directory, are private
  (`0700`, files `0600`) and hold file contents: the hunk lines of the readings a review's
  comments and marks are on, with the comments' texts, the person's and the agent's. Nothing of
  a file whose name, or name before a rename, may hold secrets is written, nor of one whose
  buffer was seen to hold secrets or to be withheld from agents while a review was open, but its
  comments' places and texts. A file is written whole through a part of its own write and read
  only as a regular file of this user's with one name, at most 16 MiB, its header checked to
  name the project and scope asked, and the model checked whole before it is used. A process
  of the same user, an agent's shell among them, can write the file: what it says (comments'
  authors, decisions and viewed marks, a session review's baseline) is taken as written, as
  the person's other state files are.
- Passphrase and credential prompts never enter minibuffer history, the kill
  ring or keyboard macros.

### Process spawning (T1, T6)

- Only `oso-platform::process` may create processes. It ships in release builds from
  [ADR 0120](adr/0120-the-terminal.md)'s step 2, for the terminal; its PTY is its own. Clippy
  `disallowed_methods` bans `std::process::Command::new` (and its async
  counterparts), `CommandExt::pre_exec` and libc's `fork`, `exec*`, `posix_spawn*`,
  `system` and `popen` everywhere else, and `cargo-deny` bans process-spawning
  helper crates. Clippy `disallowed_types` bans `std::net` sockets, and
  Unix domain sockets outside `oso-platform::control`, which carries the dev control
  socket and the agent channel. That transport is in every build, and an architecture
  check lets only the dev tooling, and the agent channel once it starts, listen on it,
  clean it or write to it ([ADR 0085](adr/0085-agent-channel-and-oso-mcp.md)). The one
  exception is the
  benchmark harness, which never ships
  ([ADR 0046](adr/0046-performance-regression-checks.md)).
- Executables resolve from absolute paths in user config, or from `PATH` with
  the current directory and the project directory removed. Language servers also come from
  oso's built-in table: on `PATH`, in a toolchain's directory under the home (`~/.cargo/bin`,
  `~/go/bin`), or, in a trusted project only, from fixed names under its root, such as
  `node_modules/.bin`, whose link must lead to a regular file of this user's inside the root
  ([ADR 0138](adr/0138-language-servers.md)). Only an `.exe` starts one on Windows.
- A language server starts only for a trusted project, on its own thread: before each start
  and restart, that thread checks again that the trust was not taken back and that the root
  is still the directory trusted, then finds the program. It runs in a process group of its
  own, so that stopping it, after a crash, a broken bound, `lsp-shutdown`, untrusting or a
  quit, ends the `cargo` and build scripts it started too. The group is signalled only
  before the server is reaped, as its number may be a stranger's after. On Windows only the
  server ends. What it sends is bounded as ADR 0138's table says, and `*LSP*` keeps its last
  1 MiB in memory only: method names, ids, sizes and timings, and the server's standard
  error, which may quote code it read, so it never reaches a log line or the disk.
- A server edits a buffer only by an action the person picked from `C-c C-r`'s list (ADR 0138,
  step 13): its edit is read whole within its bounds, applies only to open buffers still at the
  text the server had as its request went out, never to one that may hold secrets, and names no
  file under an automount or with a `..`; one that creates, renames or deletes a file is
  refused, and one of more than 1,000 edits too. Its title is the server's text, shown with its
  controls as spaces and its bidi and invisible characters written out, after what the edit
  changed, and never logged. The agent channel never
  runs the commands, which are `Person`. A file the server has no text of is the buffer that
  holds it, unmodified and at the revision it had, or opens through the loader, unshown, at most
  8 MiB, only after the person's yes to a question naming the files; never outside the server's
  root, lexically or where links lead, in a `.git` directory, or under a name that may hold
  secrets, and a loaded one judged secret, not there, or open under another name refuses the
  edit, and what it opened goes (13b). A command runs only if the server declared it, and its
  `workspace/applyEdit` applies as the action's edit only while it waits; any other gets
  `applied: false`.
- A rename (`M-x lsp-rename`, step 14) is the person's alone, as its commands are `Person`. Its
  edit goes the same way, judged as 13b judges one before and once its files load, so that no
  file that may hold secrets or lies outside the root reaches `*lsp-rename*`, which is withheld
  from agents. Its files open first, unshown and unedited, and the preview is built from those
  texts with the ranges `y` applies, which applies only while every buffer is still at the text
  shown. Every line that changes is shown before and after, what differs in a long line whole,
  its controls and bidi and invisible characters written out; one too large to show, or with
  edits that overlap, is refused, not shown in part. The new name is at most 256 bytes with no
  control, and neither it nor an edit's text is logged.
- Dictation calls TypeWhisper's local API through curl, never a socket of oso's own
  ([ADR 0175](adr/0175-dictation-through-typewhisper.md)): `/usr/bin/curl` or `[dictation]
  curl` by absolute path, with only `-q --config -` as arguments. The address, the token and
  the body go in the configuration on its standard input; the host is `127.0.0.1`, fixed in
  oso, with no proxy, no protocol but `http`, no redirect, a time limit and a size limit. The
  token comes from TypeWhisper's discovery file, read fresh for each use and refused when
  it is a link, not this user's, writable by its group or others, or over 4 KiB; it is a
  redacted type that no argument, log line, message, restart state or agent reaches. The
  commands are `Person`, refused in keyboard macros. A stale file with another local process on
  its port would get the token and type what it answers. TypeWhisper itself puts dictated text
  on the clipboard, unmarked, also from a buffer that may hold secrets.
- A dictation's fix (ADR 0175, step 2) sends the dictated text, `[dictation]
  fix_context_lines` lines on each side (3 unless set, at most 20, and at most 4 KiB a side),
  the mode's name and TypeWhisper's dictionary, capped, to Anthropic through the person's
  `claude -p`, as one JSON object on its standard input that the system prompt calls data. It
  runs in untrusted projects too: `claude` reads nothing of the project, though the text sent
  is the project's. `claude` runs with no tool, no MCP server, no project setting and no
  session kept, in a new private directory, killed at 30 s; a stop before it starts holds.
  Its answer can only replace text inside the span, while the span still holds exactly what
  was sent, in one undo step, at most 1024 replacements; a `right` with a control, an
  invisible or bidi character or a noncharacter, or much longer than its `wrong`, is dropped.
  Nothing of a buffer that may hold secrets is sent, checked at the start and again at the
  answer, and the minibuffer's text is never fixed. The report of a fix is shown in the echo
  area alone, withheld from the dev socket and agents, and never in `*Messages*` or the log.
- Dictation's learning record (ADR 0175, step 3), `dictation/learning.json` and its log in
  oso's state directory, mode `0600` in a private directory, read only as a regular file of
  this user's with one name, at most 256 KiB, hold words of the person's dictations: at most
  40 pairs, gone after 30 days unseen, and the log's last 500 lines. Nothing comes from the
  minibuffer or a buffer that may hold secrets: no span is watched there, and a watch ends,
  learning nothing, when its buffer may hold secrets or anything but the person's own
  commands changes it, an agent's among them. Each fix sends at most 4 KiB of the record
  to Anthropic with the text, only of pairs whose `wrong` is in the text, and never a pair
  learned only from text typed plain (`C-u s-d`), which the person kept from Claude. No agent tool reads the record; `dictation-forget-learned`
  removes it and the log.
- Dictation adds what it learned often enough to TypeWhisper's dictionary (ADR 0175, step 4),
  through the same calls, at most 40 entries, and never a pair learned only from text typed
  plain. TypeWhisper's own settings decide where its dictionary goes: a language model's prompt, a
  cloud recogniser or a sync. oso removes only what TypeWhisper gives exactly as oso added it, read
  again before each change, by the list of that, `dictation/typewhisper.json`, which is kept as the
  record is and written before each change, under a lock. TypeWhisper does not list a disabled
  entry, so one of the same word is written over and then taken for oso's. The learning log's
  buffer is withheld from agents, and its command and the forget are `Person`.
- The environment is sanitized and every spawn is logged in a visible buffer.

### Dependencies (T2, T8)

- `cargo-deny` (`deny.toml`, `just deny`) enforces:
  - advisories, including yanked crates
  - a minimal permissive license allowlist
    ([ADR 0020](adr/0020-license.md))
  - sources (crates.io only)
  - bans on networking crates (reqwest, hyper and similar)
  - bans on backtracking regex engines (fancy-regex, onig, pcre2, regress)
  - bans on process-spawning helper crates (duct, open and similar)
  - all of the above with every feature enabled
- `cargo-audit` (`just audit`) runs in CI, and `Justfile` builds use
  `--locked`.
- The toolchain is pinned in `rust-toolchain.toml`.
- New dependencies require human approval. Agents must justify them: who
  maintains the crate, how much `unsafe` it has, and how many transitive
  dependencies it adds.
- Only the `regex` crate is used for regular expressions, since it runs in
  linear time.
- `oso-core` uses `#![forbid(unsafe_code)]`. Other crates require
  `// SAFETY:` comments (clippy `undocumented_unsafe_blocks`).
- cargo-vet is not used for now.

### Display (T3)

Bidirectional control characters and invisible characters are always shown
as boxed code points, and a buffer containing bidirectional controls shows a
modeline warning ([ADR 0026](adr/0026-m2-ui-layout.md),
[ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)). Control characters
and invalid bytes are shown as escapes (`^M`, `\xFF`) in an escape color. Labels are
shaped apart from the text around them, so that text cannot change what a label reads.
None of these characters reach the text shaper: their labels are shaped as
ASCII instead ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)).
The text system may add characters of its own to what it shapes, such as a ZWNJ
between runs of one direction; they map to no character of the file and draw nothing.
A cluster the renderer shapes only in part, past its budget of characters, shows
in a box and is shaped on its own ([ADR 0068](adr/0068-cut-clusters-short-pieces-the-cursor-box-and-lines-by-direction.md)).
Cells stay in the logical order of the text and no character is shown as its mirrored
form, in every script
([ADR 0066](adr/0066-text-stays-in-logical-order.md)).
A glyph paints only the cells of its cluster, in its row and in narrow bands above
and below the row, and a label's glyphs keep their own cells
([ADR 0052](adr/0052-glyphs-stay-in-their-cells.md)).
Syntax colours only colour glyphs that would be drawn as text: an escape, a box or a
whitespace mark keeps its own colour, and every syntax colour stays apart from the escape
colour and the marks', so highlighted text never passes for a label
([ADR 0111](adr/0111-tree-sitter-highlighting.md)). Syntax colour is advisory: it comes from
a parser's guess, which may differ from the compiler's, and never decides how a character is
shown, boxed or escaped.

### Persistence (T4, T5)

- Dictation's learning record and log (ADR 0175, step 3) keep words of the person's
  dictations and their rewrites on disk, in `dictation/` in oso's state directory, mode
  `0600`: at most 40 pairs, and the log's lines of the last 30 days. A pair whose `right`
  looks like a credential is never kept, and nothing comes from a buffer that may hold
  secrets. `dictation-forget-learned` removes both. What oso added to TypeWhisper's dictionary
  is listed there too (step 4), and that list holds its words until TypeWhisper has answered a
  forget.
- There is no periodic auto-save, no backup files and no persisted undo
  history.
- On a panic outside a command, or when the OS ends the app on macOS, unsaved buffers are written to a
  private directory (`0700`, files `0600`, refused if it is a symlink, another user's or looser, or if another user can change
  anything on the way to it, through every link and directory it passes, in which case nothing is written, and oso says so at startup). On macOS, access lists count: only `deny` entries pass on the directory, what leads to it may not hold an `allow` that changes what is in it, and each copy holds no list before its text goes in, and at startup oso also says of a `deny` that would stop a copy ([ADR 0065](adr/0065-access-lists-on-private-directories.md)). Files matching secret patterns are excluded
  ([ADR 0028](adr/0028-secrets-handling.md) lists the defaults; users can extend them), and their text
  is not even held where the panic hook can reach it. A path that could not be resolved is saved only if no
  name on it, or on the way to where the walk stopped, matches ([ADR 0051](adr/0051-m2-plan-decisions.md)).
  A confirmed exit writes nothing. A kill, a power cut, a force quit or running out of memory
  writes nothing either. A copy is only ever offered back: nothing in that directory may choose a path to
  write to without the person ([ADR 0062](adr/0062-emergency-save-and-the-panic-hook.md)).
- Recovery treats that directory as untrusted input ([ADR 0064](adr/0064-recovering-an-emergency-copy.md)). A path in a `.meta` is
  shown and compared, and no file operation ever uses it, so a copy reaches only the buffer of a file the person opened, on their answer,
  and the disk only on `C-x C-s`. A directory refused by the checks above is not read. Each file must be regular, this user's, with one name,
  and the file that was looked at once open. Reads are bounded: 64 KiB of a `.meta`, 1000 rows of copies and temporary files together, 100,000 names, and a look at what is under 2 GiB.
  A view of a copy is withheld from the socket if any path it names would count as secret, if its buffer's verdict says so, if the copy says it was withheld or does not say, or if nothing says
  whose it is, and text recovered from such a view is withheld in the buffer it goes in. Nothing is deleted unasked but a copy found byte for byte equal to the file the person opened, and a recovered copy once a save of it has reached the disk. A temporary file left by a save or a
  copy is listed once over an hour old, and always withheld, as it may hold a secret buffer's text.
- The OS input method is off in a buffer that may hold secrets, as in macOS password
  fields, and gets no cursor area there. A preedit is in no buffer, copy, handoff or log
  ([ADR 0069](adr/0069-macos-input-method.md)). In a prompt it is on only in a review
  comment's draft, at once, and in a prompt that reads text or completes one once the person
  chooses a Japanese input mode there; never in a passphrase, a number or a question, nor while
  the draft or any buffer shown may hold secrets, and it goes off at once when one comes into
  view ([ADR 0158](adr/0158-japanese-in-prompts-and-review-drafts.md)).
- On macOS, a buffer that may hold secrets gets secure input while it is selected and oso is
  in front with the keyboard, as a password field does: keystrokes stay with oso. The window
  layer holds one enable at a time and releases it when any of that stops, when the loop
  ends and after a panic in the handler
  ([ADR 0073](adr/0073-secure-input-in-secret-buffers.md)).
- On macOS, the input method's process, and the system's other text services that read a
  view the same way, can read up to 2000 bytes of the cursor's line on each side of the
  cursor while the IME is on, from a buffer whose verdict is `Ordinary` only, and never from
  an internal buffer or a prompt: nothing is read from any other. The window layer drops the
  excerpt when the IME goes off and after a panic in the handler. Writing Tools are off for
  the view, and while the IME is off it gives no input context. The excerpt is never logged,
  and the dev socket says only how long it is and where the cursor is in it, and nothing
  while a buffer that may hold secrets is shown. The guard is the buffer's verdict alone:
  text yanked from a secret buffer into an ordinary one goes to the input method from there,
  as it would through the clipboard
  ([ADR 0070](adr/0070-windowing-and-input-without-winit.md),
  [ADR 0077](adr/0077-choices-for-the-native-layers-stage-2.md)).
- On macOS, oso listens for the system's notice that the person chose another keyboard input
  source. Any local process can post that notice, unauthenticated, so oso ignores what it
  carries and reads the selected source from the Text Input Sources itself, only while it is
  in front. It acts on it only in a prompt that reads text or completes one, never while a
  buffer that may hold secrets is shown, and never in a passphrase, a number or a question
  ([ADR 0158](adr/0158-japanese-in-prompts-and-review-drafts.md)). The worst a forged
  notice can do is make that prompt catch up with the source the person really has chosen.
  While such a prompt is open and oso is in front, oso also chooses an ASCII-capable input
  source for the person, and gives theirs back when the prompt closes, oso goes behind or the
  app ends, unless they chose another meanwhile. It changes only which installed source is
  selected, and reads only a source's ID and its ASCII flag
  ([ADR 0106](adr/0106-the-input-source-turns-japanese-on-in-a-file-name-prompt.md)).
- The kill ring syncs with the OS clipboard but is never persisted. oso reads the
  clipboard only on a yank, on its own thread, and while a dictation's text is due looks only
  at its list of types, to drop TypeWhisper's own Command-V
  ([ADR 0175](adr/0175-dictation-through-typewhisper.md)). A kill from a buffer that may hold secrets
  goes there marked concealed for clipboard managers, never shows over the socket, and
  makes the buffer it is yanked into secret, also after a round trip through another app
  ([ADR 0074](adr/0074-the-kill-ring-and-the-clipboard.md)). An image is read only on
  `session-yank-image` and never decoded in oso's process: it is copied into a new `0600` file
  in a `0700` directory under the cache, a TIFF is converted by `/usr/bin/sips` in a child
  process with a time limit, a concealed pasteboard is not read, and the files go when the
  session is archived or after 7 days
  ([ADR 0142](adr/0142-attaching-the-clipboard-s-image-to-a-session.md)). Core
  dumps are disabled with `RLIMIT_CORE` on Unix, soft and hard. Windows has no such limit for a process to set.
  The panic hook prints the location and the stack and never the message, also for a panic caught at the
  command boundary ([ADR 0028](adr/0028-secrets-handling.md), [ADR 0062](adr/0062-emergency-save-and-the-panic-hook.md)).
- `view-lossage` keeps the last 300 keys oso received, in memory only: never saved, logged or
  carried over a restart. A key typed where secrets may be (a buffer that may hold them, a
  passphrase prompt, or a prompt, isearch or jump over such a buffer) is kept as `<hidden>`,
  with nothing of what it ran, and so is a character sent to a terminal's program, which may be
  a password the terminal has not yet seen asked for. A run of hidden keys is one line. A key
  held to run later is kept as `<held>` until it runs, where it is judged. `*Lossage*` is
  withheld from the agent channel, and the channel can run neither `view-lossage` nor
  `describe-key`, which reads no key where secrets may be
  ([ADR 0150](adr/0150-view-lossage-and-describe-key.md)).
- Saving (T5) writes a temporary file beside the real file, follows symbolic links to
  find it, and renames it over that file, so a crash leaves the old file or the whole
  new one, unless the file system cannot sync, which the message then says. The text goes only to where the buffer's file was: if a link or a directory
  on the path has changed since it was read, nothing is written at the new place. The
  temporary file is created with `create_new` and, on Unix, mode `0600`, so it is never a
  link someone planted and nobody else can read it while it fills. It takes the
  old file's mode, less set-user-ID and set-group-ID, and its group, before the rename.
  If the group cannot be kept, that group gets no more than others do. If the owner
  cannot be kept, the save is refused. A file deleted since it was read gets its mode
  back, narrowed if its group could differ. A file changed on disk since it was read and a file the user may not write are
  left as they are.
  Its access list goes with it on macOS, and a POSIX one on Linux: the temporary file holds none while the text goes in, and
  takes the old file's only after it, or what its directory gives if it is new. If a list cannot be carried, the save is
  refused. The quarantine flag goes with it too on macOS, and if it cannot, the save goes on and says so. Other extended attributes are lost, and hard links break
  ([ADR 0059](adr/0059-safe-save-and-the-exit-guard.md), [ADR 0067](adr/0067-access-lists-and-the-quarantine-flag-in-a-save.md)). A secret buffer saves like
  any other: only the emergency save skips them. Messages name the path and never
  quote the text. A kill or a power cut during a save, or leaving past the 10 minutes it
  may hold an exit, can leave the temporary file behind, `0600` or by then the file's own
  mode, with the text in it, also a secret buffer's.
- A file changed on disk is read again as it was first read: opened, followed through its
  links and judged from the file that was opened. A reload never makes a buffer's verdict less
  strict, as undo can bring back what was read under the old one. Neither a load nor a
  reload logs a file's size. The watcher only looks at visited paths and never opens a file
  ([ADR 0083](adr/0083-reloading-files-changed-on-disk.md)).
- Dev state handoff for hot reload goes process-to-process in memory, never
  through files. Dev screenshots are the only dev-only files; see
  [Development](#development-t9).

### Terminal (T7)

Terminal output is untrusted input: a file `cat`ed, a page fetched, a remote host over `ssh`
([ADR 0120](adr/0120-the-terminal.md)).

- Each sequence is allowed, denied or answered from oso's own state, by one policy function per
  effect in `oso-term`, each with a test. A denied one is dropped without a reply. Text a sequence
  carries for oso to show has control, bidi and invisible characters dropped and is cut at 256 bytes.
- OSC 52 clipboard reads are disabled. A write goes to the kill ring, and to the OS clipboard only
  within 2 s of the person's key in that terminal.
- Sequences that echo data back, such as title reporting, OSC 50 and `DECRQSS`, are disabled. Window
  moves and resizes are denied.
- OSC 8 hyperlinks, URLs and `path:line` show the real target before opening: `http` and `https` with
  the OS's opener, `file:` and `path:line` in oso, any other scheme asks.
- Kitty graphics are off until ADR 0120's step 12. Then: direct transmission only, never from a file
  or shared memory, at most 16 MiB and 4096 × 4096 pixels an image and 64 MiB a terminal, decoded as
  step 12 decides (T10). Sixel and iTerm2 images are denied.
- A shell runs an oso command only through oso's own sequence, and only when it carries the
  terminal's random token, the shell is in the foreground, the command is listed in `[terminal]
  commands-from-shell` (empty by default, and never one that changes trust, the agent channel or a
  terminal's reading or input), its one argument is a checked string, and the terminal is selected.
  vterm's OSC 51 is denied.
- The floating terminal's `cd` is the person's typing: the path is quoted, a path with a control
  character is refused, and nothing is typed unless the shell is in the foreground and its prompt is
  empty.
- An agent never types into a terminal, and reads or searches one only where the person allowed it
  for that terminal (`terminal-allow-agent-read`). While the PTY has echo off with line editing on,
  as a password prompt has it, the terminal is treated as secret: the channel waits while it is
  shown, its rows are withheld, secure input is on while it is selected, and input typed there
  drops a keyboard macro's recording. A mouse copy there is a secret kill-ring entry, and no
  mouse report or wheel arrow key (mode 1007) reaches its program. A copy reads concealed cells (SGR 8) as blanks.
  Terminal text never reaches the disk.
- A terminal parses on its own thread, reads at most 64 KiB at a time and queues at most 16 MiB of
  writes, so neither bulk output nor a large paste reaches the UI thread.

### Agent integration (T7, T9)

- Claude Code runs as its official TUI in a terminal view (M4,
  [ADR 0031](adr/0031-claude-code-integration.md)).
- Naming a space is the one run of `claude` with no terminal
  ([ADR 0159](adr/0159-space-new-starts-the-orchestrator-and-display-names.md), step 2): only
  on the person's `space-new`, unless `[spaces] naming = "local"` (any other value counts as
  `local`), and one at a time. The person's own `claude`, found as a session's is, runs
  `--print --safe-mode --tools= --strict-mcp-config --setting-sources=user
  --disable-slash-commands --no-session-persistence --model=haiku` with a fixed system prompt
  and JSON schema, through `oso_platform::process` on a thread of its own, in a session with no
  controlling terminal, in a new private directory of its own under the state directory's
  `space-naming`, removed after it; a directory already there is not used, as a file there
  could be a setting it reads. It takes the names a session's terminal passes on but the agent
  socket and oso's runtime directory, and sets none; its arguments are not logged, and its
  error output is thrown away. The description, which a buffer that may hold secrets cannot
  give, goes on its standard input, never in an argument; at most 64 KiB of its answer are
  read. Its whole session is killed past 20 s, when the space is closed, and at an exit or a
  restart's handoff. The answer is untrusted text: its display name is cleaned as a display name,
  its slug taken only if it is a slug as oso makes them, both made unique, and anything else
  gives way to oso's own names. A description that steers it can only pick the two names. No
  permission option is given: with no tool, nothing is asked.
- A native session's `claude -p` runs only in a trusted project, or in an issue space's own
  directory as below, as Claude Code asks it no trust question of its own ([ADR 0165](adr/0165-native-claude-sessions-the-route.md), Q5): a
  new start at a trusted project's root, and a resume or a fork only at the root of a project
  trusted now, never under the trust the session started with. An open file or terminal of the project decides; with none open, the record of trusted projects
  does, by the root's path on the UI thread and by its path and identity on the start thread,
  as an issue space's member is (part 4a): a directory made again at the path and a link there
  are refused, and a linked worktree resumes only while git's files, read afresh, and the record
  say its main repository is trusted ("Worktrees' resumes"): the start thread walks the root
  again, with nothing run, where part 4a looks and once more just before the spawn. It so
  trusts the worktree's `.claude` settings, hooks and `.mcp.json` as they are then, also at
  `oso-restart`'s resume with no person there; a new start, a fork, an adoption and a move
  into a space in a worktree need an open, trusted file or terminal of it. No native session's
  transcript decides one ("Transcripts count as open"). A record not
  read yet is waited for, at most 5 s with ADR 0173's held starts, then nothing is trusted.
  Until its start thread reports, the UI takes a resume's or fork's trust back once its
  project is no longer trusted, a worktree's once any root has left the record, and the look
  before the spawn refuses it.
  The start thread looks at the root again before the spawn.
  The resume that follows an interrupt by signal is such a resume, judged again then, and none
  starts while the person is asked whether to stop the session, or oso waits to quit or
  restart (step 2c). So is `oso-restart`'s resume of the sessions it stopped (part 4c), which
  runs with no person there: only the ids the parent's state names that the new instance's
  own record has as stopped native sessions, at most 64, in the record's directory and root,
  under the trust judged then, once the instance before has gone, and refused by the start
  thread while the registry has the conversation live.
- A native session in an issue space's own directory runs under the space's gate, never
  under a project's trust ([ADR 0176](adr/0176-native-orchestrators.md)): only in an open
  space's directory that oso made under `[spaces] root`, with the identity it had then, for the
  space's current or an earlier orchestrator, a session of the person's there that oso made
  native there (a native fork of one), or the new orchestrator the person starts for that
  space (`o`, `space-new`, a successor; part O-3); never one an agent started, nor a terminal
  fork of it or an adoption. In a space's directory a new start is native by that gate alone,
  or a terminal. The records it judges by are files agents' sessions can write, as the
  session tools' roles are. Its line adds `--setting-sources=user`, so none of the
  directory's or its parents' hooks, `.mcp.json` servers, commands or `CLAUDE.md` load; the
  space's checks run on the start thread before it, as before every start there. No
  `TrustedRoot` is made for it, so it adds nothing to the session tools' trust and takes no
  baseline. Until its start thread reports, the UI takes the gate back once the space closed,
  its directory changed, the session is no longer the space's (a new orchestrator's start no
  longer its space's) or its buffer is gone, and the look before the spawn refuses it.
- A native session's earlier turns (ADR 0165, part 5a) are read from Claude Code's transcript
  of its conversation once its process is spawned, before its stream: only a regular file of
  this user's with one name under the transcripts' directory, in a project directory that is
  not a link, opened without following a link, whose first line naming a directory names the
  session's; at most its last 128 MiB and 131,072 lines, within 1 s on a thread the start
  thread waits for no longer, lines over 16 MiB not read, and at most 8 MiB and 2000 lines of
  it decoded. Attachment
  lines, which hold the account's email and organisation ids, and the CLI's own records are
  never decoded, and nothing read is logged. A message shows as the person's only where the
  transcript marks it so. Rows that hold the conversation read only what follows the ids of
  their newest messages (part 5b), by the same rules.
- A native session's replies are drawn without most of their Markdown's marks (ADR 0165, "The
  look: Markdown"): what is hidden is marks, a fence's language of at most 16 bytes, and a
  link's web address behind plain words, on a line of words alone, never other text; invisible and bidi characters show as anywhere. A line that could start as a row
  of oso's keeps its marks, or is drawn behind a bar in code; a fence's code can still have a
  tool row's colours, not its place. A table's box is in a class and a colour of its own, inside a
  reply's indent, so it is no card's frame, guide or diff; a cell hides nothing a line would not, and one broken inside a word says so with `↩`,
  and one that would not stand as it measures leaves the table as written, as do the bounds on
  its size. `w` copies the raw text, on the person's key.
- A native session's control layer (ADR 0165, step 2c) writes only `initialize`,
  `interrupt`, `get_usage` (part 3a) and `set_permission_mode` (part 3b), and reads of an
  answer only its id, its success, its error, cut to 256 bytes and logged escaped,
  `initialize`'s slash commands (below), its models, its permission mode and whether Remote
  Control is available, `get_usage`'s cost and two windows, and `remote_control`'s address and
  id (ADR 0181, which nothing asks yet); it answers no request of the CLI's. `SIGINT` goes to the session's
  process alone, never its group, and only on Unix: on Windows it would end the process at
  once and leave its tools running.
- A native session's model and effort level (ADR 0165, part 3c) go on a later process's
  command line, `--model=` and `--effort=`, one argument each, only from the person's own
  `/model` or `/effort`, sent from the composer, and only once the `result` that ends that
  command's turn names it, is no error, and its reply says the CLI set it. What a stream line
  says alone notes nothing: the model or level is the one the person sent, a model one word of
  ASCII letters, digits and `._:/@[]-` that does not start with `-`, a level one of the five
  `--effort` takes; an effort's reply must name the level asked. A forged `result` can at most
  note what the person asked, though it can do so where the CLI refused or capped it: a model
  or level the person's settings would not take on `/model` or `/effort` then goes on the
  command line, where whether the CLI holds it to those settings is not measured. The person's
  own command includes text an agent put in the composer (part 2f escapes only the start of
  what it puts) that the person then sent. Nothing of it is
  in the record across oso's restart; the restart's state carries it for a session the
  restart resumes (part 4c), read again by these rules. Completion over the models and levels shows the CLI's
  text as completion rows show any text, escaped, and puts the one chosen in the composer,
  unsent.
- oso changes a native session's permission mode (ADR 0165, Q4, part 3b) only on the
  person's key: `native-session-cycle-mode` (`S-TAB`, `Person`); no agent or channel method
  sets one, and a plan the person approves with `acceptEdits` or `auto` (part 3d) counts as
  their choice. The CLI changes it too, as approving a plan does. The modes oso sends, in
  `set_permission_mode`, `--permission-mode` and the bridge's `setMode` (only `acceptEdits` or
  `auto`, after a plan), are one type's, which has no value for
  `bypassPermissions` or `dontAsk`, so neither can be sent. A later process of the session is
  given the mode it held, as `--resume` keeps none: the one oso gave its process until that
  process says one, else the one the CLI said last if it is `default` or `plan`, or if the
  person switched to it here, else `default` once the person switched here. The stream is not
  authenticated, as a tool the session runs can write to it: a line that says `acceptEdits`
  or `auto` is shown but never carried unless the person chose it, and once the person
  switched, a later process never falls back to the settings' mode, which may ask less. Only
  the first `initialize` answer that succeeded is read; one after a refusal or a wait that
  ended is still taken, as the CLI speaks the protocol (step 2c). A fork starts as its
  source's next process would. A tier-1 switch whose resume is refused (the project not
  trusted now) is over. A new session gets no mode, and starts as the person's Claude
  Code settings say; so does one that comes back stopped after oso's restart. One that
  `oso-restart` resumes (part 4c) starts in the mode the instance before would have given its
  next process, from the parent's state, taken as the person's choice. A tier-1 switch stops
  only an idle process, and only while a resume may start (no stop asked, no quit or restart
  waiting); the resume is judged under trust then. A mode that is not readable as itself
  (ASCII letters, digits, `-`, `_`, at most 256 bytes) is not shown.
- The slash commands a native session's CLI lists (ADR 0165, step 2d) are the project's text
  as much as the CLI's, as a skill's description is its file's: at most 512 and 8 aliases
  each, each hint and description cut to one line and 256 bytes, a name or alias longer than
  that or holding a space, a control, an invisible or a bidi character dropped, a project's
  marked `not built in`, and shown only in the person's completion rows, which escape controls
  and bidi characters. `/clear`, which would move the conversation to a session id oso's
  record does not follow, is refused at each write, whatever a project's skill takes its name,
  and an agent's brief held in the composer; a session whose conversation moved anyway, as its
  stream's session id says, neither resumes nor forks while oso runs.
- A native session's `claude -p` asks the person about a call through its bridge's
  `permission` tool, named as its `--permission-prompt-tool`
  ([ADR 0165](adr/0165-native-claude-sessions-the-route.md), step 2):
  - The bridge asks on the sessions' socket, `<pid>-session.sock`, a second socket a release
    build listens on without the person's command: from a native session's start until none
    runs or starts, with the agent channel off or on. It lets in only a connection that a
    running native session's bridge makes, as ADR 0121's claim judges it, one per session; it
    never asks the person about a connection, and answers only `permission`, which the agent
    channel refuses. The bridge connects as MCP's `initialize` comes, before the session's
    first turn; a second process that claims a session's slot, as a command the session runs
    can, is refused, and the person is told.
  - Only a bridge started with `--permission`, which only a native session's MCP configuration
    gives, lists the tool; the CLI keeps the prompt tool from the model, as measured.
  - The person is asked about the call as the session's own stream has it, by its id and tool,
    and only if the request's input reads to the same target; a request whose call does not
    wait in the stream within 2 s, or that stops waiting before it is asked, is denied unasked,
    and a call is asked about once. What only the request carries is shown from it, marked so
    where it is a plan: a reason, an edit's strings, a question's text, an `ExitPlanMode`'s
    plan. So a process that passes the claim can show its own text for a waiting call, under
    that call's tool and target; an allow to it runs only that call (step 2e-2).
  - It is asked on the session's permission card (step 2e-2), withheld from agents: oso's title
    and keys around Claude's text, fenced row by row after `│ ` in a face of its own, each of
    Claude's line feeds a row, a long row broken at the card's width as the display measures
    it, the rows it goes on to fenced `┊ `, so that no text of Claude's runs off the card,
    stands as a key row or passes for oso's words; no agent command runs while a card is
    selected, a look included; the title says
    when it does not show all the call does, and `RET` shows the input that runs whole in the
    card. The frame, oso's words and the keys' chips are in faces that only oso's own spans take,
    and what is drawn behind them comes from those, never from what a text holds; an option's
    number and mark are oso's too (ADR 0180). Its keys are `Person`, and answer only from the
    keyboard, after 500 ms with no key and
    since the card became the selected buffer, however point came, or oso's window came in front
    again, or the card's came to show all of it (longer after a prompt an agent opened closed);
    never from a macro, nor from a window that shows less than all of
    the card from its start, unwrapped, or is under 20 cells wide; a held key answers once, `RET`
    and a plan's chords among them. From the session's composer,
    `y`, `n`, `N` and a question's `o` and digits answer only while it is empty and selected, with
    one cursor and no composition, under the same gate, measured since it was so, which looks at
    the card's window, as an agent's looks run there, and only as a
    key that would be typed: never text an input method commits, a held key's repeat, nor what a
    macro replays. Never a plan's `a`, `A` or `e`. Point moves
    to it only from that session's transcript, or its composer with no draft, at once as it
    appears and never in the middle of a key sequence, a prompt, a search, a composition or a
    drag. A question's digit chooses only an option whose every row has been on screen. Whatever goes
    wrong, the bridge denies: a call runs only on the person's `y` or answers, an `allow`
    gives the CLI back the input it sent, and only an `AskUserQuestion`'s takes answers, the
    person's alone, and an `ExitPlanMode`'s the person's edit and mode (part 3d, below). A
    note or an answer holding a yanked secret is refused.
  - A plan's card (part 3d) also approves with a mode after it, `acceptEdits` or `auto`, the
    only two the protocol and the bridge carry: an answer naming any other mode, or a plan
    that is not a string within 64 KiB, denies, and only an `ExitPlanMode`'s `allow` takes
    either, so no other call's answer sets a mode. `auto` lets the CLI's classifier run what it
    judges safe unasked; it is the person's `A` (or `C-c C-A` in the edit), after the card's
    quiet wait, as on any card, and then `y` to a question, as `A` begins many a typed
    sentence. The question is kept for the call it asked about, and its `y` approves only that
    call while it still waits, its edit checked again, and only after the card's quiet moment
    since the question opened, or oso's window came in front again, and since the key before
    it, as a question takes other keys
    without closing; a question closed otherwise is dropped.
    An edit of the plan, `*plan: NAME*`, is the person's draft: withheld from agents, which may
    not change it (`Mode::Plan`, kept as a composer's mode is), approved only by the person's
    keys there, after the same quiet wait as its card, since the edit became the selected buffer,
    never from a macro, and only for the call it was made for. One that differs from Claude's
    plan asks y or n first, saying how many lines differ; that question is as auto's, and its `y`
    approves only the edit as it was when asked. No agent command
    runs while it is selected, a look included, as cursors or a region would steer the
    person's typing into what they approve. It is sent
    only if it differs from Claude's plan, never cut, and never once a yank made it secret.
    While an edit that differs waits, the card's approvals are refused, so that a `y` never
    approves a plan the person changed. Claude's plan text in it is shown as an editor shows
    any text; what goes back unedited is the CLI's own plan, as the card showed it, and an
    edited one goes back whole as the person's, any of Claude's text left in it included.
  - A session's to-do strip (step 3, part 3e), `*todo: NAME*`, is withheld from agents too, and
    fences Claude's text as the card does: each item's text after oso's mark, in a face of its
    own, its control characters as spaces, cut to the strip's width, so that none of it runs
    off the strip or stands where oso's keys are. Its list comes only from the main thread's
    to-do calls' results, matched with their calls, at most 100 items of 256 bytes; oso
    never reads the CLI's task files.
- Its MCP tools reach oso only through the `oso mcp` stdio bridge and the agent channel, a Unix
  socket with a peer user check (and a native session's permission questions through the
  sessions' socket, above); no TCP port is opened. There is no code-execution tool, and
  no way to write a file that could run code later without the person's yes: the channel opens
  only files that exist, saves without asking only buffers the person visited, and never saves
  oso's configuration or trust ([ADR 0085](adr/0085-agent-channel-and-oso-mcp.md)). The threat is
  an agent steered by text it read, and other processes of the same user:
  - The channel is off until the person runs `agent-channel-start`, or `[agent-channel] start =
    true` in `config.toml` starts it at launch. Only the first look at the file starts it, and a
    reload never does; a file that other users in a group can change, or the way to it, does not
    start it ([ADR 0108](adr/0108-the-user-configuration-file.md),
    [ADR 0110](adr/0110-adr-0108-as-built.md)). Each new connection asks the person on its
    first request, however the channel started, from the keyboard only, naming the process as it
    reports itself, and `oso mcp` only when the process runs oso's own program. A connection
    that sends nothing within 2 s is closed unasked and uncounted, so a probe raises no
    question. One connection waits at a time, at most 10 ask a minute, at most 60 connect a
    minute and 2 are open at once (a hosted session's bridge apart, below), and a refusal holds off new connections for 60 s. One let
    in sends at most 30 requests a second. The modeline shows `Agent` while it is on, and
    `*Agent*` lists each request without its text.
  - While a native session's request waits in front of the person (its card, composer, transcript
    or plan's edit selected), `open` shows no file, one it is loading among them, which is
    dropped; and while any card waits, an agent does not switch highlighting.
  - It runs registered commands only, by name, with arguments checked against their spec, and
    no keys. Each command declares a reach: `Look`, `Change` or `Person`. The channel refuses
    `Person` commands: exiting, `M-x`, answering prompts, reading the clipboard, emergency
    copies, passphrases, and later trusting a project, spawning processes, secret buffers and
    secure input. A channel action never starts a process by itself (M4 onward, as nothing that opening or
    saving starts in M3 runs one): what an agent's open would start waits for the person's
    first command in that buffer, and what its save would start, such as a format on save or a
    language server's check, for the person's next save of it. oso's own work, such as the
    whitespace cleanup, runs at once. Each kind of thing opening or saving starts declares
    whether it runs a program, with no default, and the modeline and the agent's reply say what
    waits ([ADR 0118](adr/0118-what-an-agents-open-or-save-starts.md)).
  - It runs nothing while a buffer that may hold secrets is shown, while the person is in a
    prompt, a key sequence, a composition or an isearch (ADR 0082), while a mouse button is
    held, within 1 s of the person's last key or mouse press or release, or while a
    question it opened is open. It does not open a path that matches the secret patterns, and
    shows a file it opened only once it is judged ordinary. Its state withholds what
    `get-state` withholds, and never has keys in progress, prompt input, the kill ring or the
    clipboard.
  - A question it opens starts with `Agent:`, is answered by the person, and takes no answer
    until 500 ms pass with no key. Its kills never reach the OS clipboard, and its text enters
    no prompt history.
  - Screenshots, the dev socket's restart handoff, timings and hot reload stay dev-only. A
    release build's handoff runs only from the person's `oso-restart`, over a socket pair with
    its child ([ADR 0133](adr/0133-keeping-terminals-over-an-update.md)). It starts only the
    binary the instance came from, by the path it had at start, once no one but the person and
    root can change it or a directory above it; the agent channel stops before it. The new
    instance takes its end of the pair only from its parent, reads the terminals' masters only
    once that instance has gone, and restores no library snapshot until the snapshot is on, so
    its decoder reads nothing from the pair.
  - An agent it lets in reads any file oso can read, except files that may hold secrets, and
    edits the files oso has open. Any save of a buffer the person did not visit asks the person.
    `C-x C-f`, `C-x C-w`, `C-x p f`, `M-s r` and `kill-buffer` are the person's only. The channel switches buffers by
    name, but never to one that may hold secrets
    ([ADR 0087](adr/0087-finding-files-writing-under-a-new-name-and-large-files.md),
    [ADR 0088](adr/0088-switching-and-killing-buffers.md)).
- oso writes no hook into Claude Code's settings. When the person sets
  `[sessions] steer_worktrees = true`, a session oso starts gets one `PreToolUse` hook for that
  session only, passed with `--settings`
  ([ADR 0121](adr/0121-agent-sessions-and-spaces.md)); no hook reports a session's state.
  The hook is `oso hook deny-worktree`: it reads at most 64 KiB of standard input, starts
  nothing, and allows what it cannot read. It steers Claude, and is no boundary.
- Remote Control is off in every native session ([ADR 0165](adr/0165-native-claude-sessions-the-route.md),
  as amended): each native start's `--settings` has `disableRemoteControl`, so the CLI refuses a
  request to connect it to claude.ai, whatever the project's settings or the person's
  `remoteControlAtStartup` say. Connected, the conversation would be stored there, and messages
  and mode changes would come from outside oso's window, past oso's rules on who makes them.
  oso never asks for it: the request and the state are there
  ([ADR 0181](adr/0181-remote-control-of-native-sessions.md), part RC-1), with nothing that
  writes it. A terminal's `claude`, the TUI a native session is handed to among
  them, follows the person's own settings. It is a setting of that one process: a `claude` the
  session runs reads its own, and it is no boundary against other processes of the same user.
  - What the stream says of it is read strictly, and a process started blocked is off whatever it
    says: no line turns it on, makes a message the person's, or answers a card. Where a process
    may be connected, a mode no key here chose is shown and never carried to a later process, and
    a request to connect has a random id. The session's address is kept only of claude.ai's
    exact shape, never opened; neither it nor the session's id is logged.
  - The canary: in a process started blocked, an `initialize` that says it is available, or a
    `bridge_state` that says it connects, is said once, in a row and the log, and that
    process's messages and modes are then judged as a connected one's, which only takes away.
    Any process that writes to the stream can make it do so.
- The agents' session tools ([ADR 0121](adr/0121-agent-sessions-and-spaces.md), step 6) act
  only for a session oso started, proven by its bridge, and only within its repository; any
  other connection may only read, within the selected window's project, or ask to move into a
  space. Starting a session asks the person each time, from the window only, after a quiet
  moment, and the process starts on
  the person's `y`, as the person's act; the question quotes and escapes every name in it, and
  `?` shows the whole brief. No tool types into a terminal, answers a session's dialog, passes a
  permission flag or reads Claude Code's token files; a guard test keeps the tools' files from
  the terminal and the process module.
  - A session's project is trusted as an open buffer of it has it now, its own terminal or
    transcript among them (ADR 0132; ADR 0165, "Transcripts count as open"); with none, a job
    runs under what the session started under while that holds, and nothing goes unasked.
  - A space's orchestrator's `session_resume` ([ADR 0153](adr/0153-agents-resume-sessions.md))
    takes only a stopped session of its space that is no orchestrator and not archived. It asks
    as a start does, or goes without asking under ADR 0135's rules, counted toward the limit
    while it runs. A conversation a live process runs is refused, and the agent never forks
    one.
  - A space's orchestrator's `space_add_repo`
    ([ADR 0161](adr/0161-an-orchestrator-adds-a-repository-to-its-space.md)) adds a
    repository's main checkout to its own space, under every check of `space-add-repo`, the
    path also naming it with no link or `..` on the way, and looked at again at the person's
    `y`. It asks as a start does, past no start limit (only the rate below and 16 members), and goes without asking only for a trusted
    repository oso knew before the call, by device and inode; one oso does not trust always
    asks and joins restricted. A running orchestrator reads it only after the person's `/add-dir` paste,
    or, run natively, after their `y` to its resume with it, which oso judges again at each
    event until the session is idle ([ADR 0176](adr/0176-native-orchestrators.md), part O-2).
  - `session_rename` ([ADR 0155](adr/0155-renaming-sessions.md)) asks nothing, as it changes
    metadata only, and only the session tools' rate bounds how many it makes
    ([ADR 0178](adr/0178-a-rate-limit-on-the-session-tools.md)). An agent
    renames itself, the sessions it started and, as a current orchestrator, its space's
    sessions; never an orchestrator, whose name is the address its workers message. A rename
    takes no name another session, a space or an earlier orchestrator has, nor one another
    running session was started with or Claude Code's registry gives it, ASCII case aside.
    A registry name counts only from the entry of the session's own `claude`, and only a clean
    one.
  - `session_archive` ([ADR 0179](adr/0179-agents-archive-sessions.md)) archives a row in
    `session_rename`'s scope, never the caller, an orchestrator, current or earlier, a pinned or
    an archived session; the person's `A` brings it back, and no agent unarchives. A stopped
    session's archive asks nothing. One that runs is stopped first, as `k` stops it, only when
    its screen is settled and its own registry entry, matched to its process, says `idle`, read
    on a thread (one at a time per caller, answered within 5 s) and judged again on the UI
    thread before the stop, with the session's id and the caller's connection; `shell` (a
    background shell still runs) refuses it. The registry half is advisory against an agent of
    the same user, which could write that file, as it could signal the process itself; the
    screen half is oso's own. The stop asks unless the person's `agents_start_without_asking`
    allows it and the session is not shown in the selected window, with no trust asked, as it
    runs nothing, and no start limit, so only the rate bounds it; after a `y` the registry is
    read again, and a connection that goes meanwhile drops the stop, saying so. The stop is the
    watch's (`App::stop_quietly`), outside the tools' files, which the guard test keeps from
    terminals, as a removal's stop is. Its worktree, branch and images stay.
  - An agent's `worktree_remove` asks nothing once every check holds (the author's choice), and
    `worktree_list` runs git and `gh` without a question; both need a trusted project.
  - The person's `n` refuses the same call from a session for 60 s. It is kept by session, so a
    new connection does not reset it, and a branch's refusal by the name git resolved it to.
    No count of calls waits for the person ([ADR 0177](adr/0177-no-loop-guard-on-the-session-tools.md)),
    but a session makes at most 30 calls that ask or change in any 60 s, a sliding window kept
    by session; past that, each call is refused with how long to wait ([ADR 0178](adr/0178-a-rate-limit-on-the-session-tools.md)). Only a call
    the guard lets through counts, even one a later check or job refuses, so a failing removal's
    git and `gh` and a rename's probe of names run at most 30 times a minute per session.
    What else holds a runaway agent: the questions, one at a time; the refusals, which only a
    question's `n` records, so a tool that asks nothing is never refused; `agents_start_limit`
    where set, which caps the unasked sessions running at once, not those started over time;
    and the channel's 30 requests a second per connection, each session's bridge being one.
    A session that changes its target each time asks again after every `n`. With
    `agents_start_limit = "none"`, unasked starts have no limit but the rate. The rate is per
    session: sessions that run away together each get their own.
  - A tool's worktree jobs take the trust of the session's open project as it is at the call,
    and again at the person's `y`, so a project the person no longer trusts runs nothing.
  - `move_to_space` ([ADR 0151](adr/0151-moving-a-session-into-a-space.md)) is the one tool
    that acts for an outside connection, and it asks the person each time. The agent names only
    a space; the conversation, its directory and its process come from the registry entry of
    the bridge's parent, matched to when that parent started as the connection came. The
    question names that process as the system does, since any process of the person's can
    write an entry. Its repository must be trusted, at the call, at the `y` and again before
    the resume, and the question names it when the `y` adds it to the space.
    Nothing starts until the outside process has ended, and the start thread reads the registry
    again, so two processes never run one conversation. The moved session counts as started on
    the person's `y` (ADR 0135), which the question says. oso signals no process for it: the
    bridge of the session that moves sends `SIGTERM` to its own parent only, once oso saw its
    turn end after the reply, and only while that id is still its parent (a pidfd on Linux).
  - `draft_set` ([ADR 0127](adr/0127-source-control-and-github.md), step 6a) asks nothing: it
    writes only the commit message whose newest request, pasted by the person's `C-c C-d`,
    named its 32-bit id, from the session it was pasted into, in that worktree of a trusted
    project,
    within 15 minutes, once, at most 16 KiB cleaned as a paste is, and commits nothing. The
    requests oso pastes into a session are the person's, read before Enter; a fix request holds
    the failed run's output, which the log withholds from the channel, so only the person's
    paste gives it to the agent, and leaves out its lines shaped like a key or a token, saying
    how many. `app/vcs/draft.rs` is one of the tools' files the guard test keeps from
    terminals, and none of them may call `send_to_session`.
- Worktree sessions run git and `gh` only for a trusted project whose main repository is
  trusted too, on `oso-sessions-git` (a removal's `git worktree remove` on
  `oso-sessions-remove`), through `oso-platform::process`
  ([ADR 0121](adr/0121-agent-sessions-and-spaces.md), step 4).
  - Each job checks the project's trust when it runs, and a worktree being made between its
    steps: a project no longer trusted, or whose root is no longer the directory that was
    trusted, runs no git.
  - Every git run has no hook and no file system monitor of the repository's, as a review's
    has, `git worktree add` included. It fetches nothing and is killed after 10 s, except a
    removal's `git worktree remove`: it runs only after every check held, has no time limit,
    and only the person's `worktree-remove-stop` stops it, after a warning, as a kill leaves
    the worktree half removed. Neither an agent nor revoking the project's trust ends it. It
    runs in a session of its own with no controlling terminal, so what it starts cannot wait
    on the terminal oso was started from.
    While it runs, its worktree is held: a second removal of it, its checks and a session
    started in it are refused, and at most 4 removals run at once.
  - A linked worktree is run in only once its claim and the main repository's record of it
    hold, with `GIT_DIR` and `GIT_WORK_TREE` given.
  - `gh` runs in an empty private directory and names the repository with `--repo`, so it runs
    no git in the repository. It is asked only about `github.com` or the host `GH_HOST` names,
    and gets only that host's tokens.
  - `.claude/worktrees` and every copy `.worktreeinclude` makes are created without following a
    link, and the copies are capped, so a repository cannot write outside itself. The
    worktree's place is checked again after git made it, before anything is copied. A source
    file with more than one name is not copied.
  - A removal never passes `--force` and never deletes a branch. The person's `y` runs every
    check again in the job that removes it, and git itself refuses a worktree that changed
    after that. The checks guard against losing work by mistake; they are no boundary against
    an agent that can write the repository's `.git`.
  - `*Brief*` is withheld from the agent channel, as `*Session explain*` is.
- `claude` hosts a Claude Code session in a terminal
  ([ADR 0121](adr/0121-agent-sessions-and-spaces.md)). Its command line is built from parts oso
  controls, never from text an agent gave, and passes no permission option, so a session starts
  in the mode the person's own settings give it; only a native session's later process is
  given `--permission-mode`, the mode it was in or the person switched it to (ADR 0165,
  part 3b), and `--model` and `--effort`, the ones the person chose with `/model` and
  `/effort` (part 3c); and so is the TUI a native session is handed to (`session-switch-mode`,
  part 4d-1), with what the native session held, on its resume alone, never on a new or
  forked terminal's line. Through the login shell, its arguments are
  positional parameters of a fixed script, never script text, and `claude` is found on the
  shell's `PATH` as in any terminal the person opens, outside the session's directory and its
  project, unless `[sessions] claude` names it by absolute path. `[sessions] env` cannot pass `PATH` or the loader's variables. Starting,
  resuming, forking and `session-switch-mode` are `Person`. oso's start brings sessions back
  stopped, and runs nothing.
  - `session-switch-mode` keeps one writer of a conversation (ADR 0165, part 4d-1): the CLI lets
    two processes write one, so the TUI starts only once the stream said the native process
    ended and it was reaped (a stream whose end came with no reap, as when its reader failed,
    has its group killed then, and the switch is given up), its start the resume's registry
    look; a stop asked or made (the person's, a space's close, a worktree's removal), an exit or a
    restart asked meanwhile gives it up too. The TUI is not
    gated by oso's project trust, as no TUI session is: Claude Code asks its own question.
    Back from the TUI (part 4d-2), the native resume is gated as any native resume. A session
    keeps its TUI's `claude`'s process id and start time, read just after the spawn, until its
    end is seen reaped; every native resume of it waits first on its start thread, at most
    10 s, until no process with that id started when it did, as a TUI that ignores `SIGHUP`
    outlives its terminal's 2 s wait for its child, and a kill stops the session before the
    hang-up; else it is refused, and the record still says `tui`. A running TUI whose process
    id is not known does not switch.
  - A resume reads Claude Code's registry first, and refuses while a live process has that id,
    offering a fork. The registry is read on a thread, `<digits>.json` names only, bounded,
    without following links and only the reading user's; oso never reads the `*.key` files.
  - `session-adopt` (`Person`) lists Claude Code's transcripts, `<uuid>.jsonl` names only, on a
    thread, bounded, without following links and only the reading user's with one name. Of each
    it reads only the id, the change time, a name from lines that start as title lines, and the
    top-level `cwd`, found by a scanner that never decodes another value. It never reads what
    was said. A directory from either file is taken only absolute, with no `.` or `..` part.
    Names and directories are escaped before they are shown. Each candidate's text stands for
    one conversation, looked up in the list its prompt showed. The resume is a resume, refused
    while the registry says the conversation runs.
  - A session's `oso mcp` names its session, and is let in without the question only if its
    parent is the `claude` oso started and has not reaped, it runs oso's own program, and no
    other connection holds that session. Such a bridge is outside the channel's cap of 2, one a
    session, and keeps every rate limit. A claim that fails this asks as any connection does.
- A hosted session's state comes from rules over its screen, which a model wrote: untrusted
  text ([ADR 0121](adr/0121-agent-sessions-and-spaces.md)). No state authorises anything, and
  needs input is set only by a rule that knows a dialog; an unknown prompt is idle. The rules
  are oso's, compiled in, and the person's `agent-rules/claude.toml` beside `config.toml`, read
  with its checks and refused when other users, a group included, can change it; never a
  project's, never fetched. They use the `regex` crate only, each pattern with a 128 KiB size
  limit, at most 256 patterns and 128 rules a file, and the screen is cut to 40 rows of 1 KiB,
  so judging one is linear in at most some 41 KiB.
  - A look gets 5 ms, checked between patterns, not inside one: past it the state holds and
    the screen is looked at again later, less and less often.
  - A local rule that is the slowest of 3 late looks in a row is not used until the rules
    reload, and `*Messages*` names it. A bundled rule is never dropped.
  - So a crafted local file can still delay the sessions worker, and its sessions' states, by
    one pattern's run per look, and it costs memory when it loads. The file is the person's
    own, and nothing else waits on that thread.
- Sending to a hosted session (`session-send`, `session-mention`,
  [ADR 0121](adr/0121-agent-sessions-and-spaces.md) step 5) is the person's typing, `Person`:
  the agent channel never writes to a terminal. It goes through one path, which pastes as a
  bracketed paste, cleaned as any paste is, and never sends Enter; a native session's goes
  into its composer at point, unsent ([ADR 0165](adr/0165-native-claude-sessions-the-route.md),
  step 2f).
  - What it pastes is text oso composed from what others wrote, so it is escaped first, as is
    an agent's brief: a leading `/` gets a space, as Claude Code runs a message that starts
    with `/` as a command, and an `@` at the start or after whitespace (U+FEFF included) gets a
    `\`, as Claude Code puts a file an `@PATH` names in the context there, wherever the file is
    (ADR 0121, "Composed text, as amended" and "An agent's brief, as amended"). It is escaped
    once, as the paste will read it: a control character the paste drops, or an invisible one,
    is looked through, and a leading `!` or `#`, a mode of the TUI's input box, gets the space
    too. A worktree session's header and hand-off facts, and `session-mention`'s `@NAME`, are
    such text.
    Text put into a draft, a kept image's reference included, also leaves no `@` of the draft at
    the start of a word; the brief oso composes for an orchestrator's successor, of names agents
    choose, is escaped too. An image kept after the person left the session goes in without
    taking the selected window, and a composer a yank turned secret takes nothing. A kept image's `@PATH` is oso's own and
    goes as it is, as does what the person types. So does the space's `/add-dir PATH` offer to
    its orchestrator's terminal, oso's own command, which only `Command::add_dir` makes, from
    a recorded repository the person trusts, while its path is absolute, in UTF-8 and one
    plain line, with no blank but a space, none at its end and no `@` after one, the command
    within 512 bytes; a native session is never given it (ADR 0165, part 2f, amended
    2026-11-23). Only `claude -p`'s reading was measured;
    the TUI's is taken to be the same.
  - A native session's prompt suggestion is Claude's text, which a file Claude read can
    steer: `C-c C-s` or `M-n` puts it into the composer escaped as composed text, unsent, so a
    suggested `/command` or `@PATH` runs only if the person takes the escape out; after a draft
    it goes with a space between, so that it joins no `/` or `@` of the draft's, and never after
    a command. It shows in the empty composer's placeholder, never without oso's words and key
    after it, so that it cannot pass for oso's hint, and is in no buffer. One holding a line
    break, a control, bidi or invisible character, or a variation selector that draws as
    nothing, is dropped, so that what goes in is one line
    that shows as it is in the composer, scrolled into view; a composer a yank turned secret takes none. The recap of a turn that ended unseen quotes the transcript, so it reaches only
    the echo area, marked `NAME (last reply): …` so that it cannot pass for oso's own message,
    unlogged and left out of what the control socket and the agent channel
    read ([ADR 0165](adr/0165-native-claude-sessions-the-route.md), part 3f).
  - A session that is not running, not judged since it started, asking for input or held by a
    full-screen view is refused, as a paste into a dialog could choose an option. The check
    reads the last verdict, which can be some 100 to 200 ms old when the paste lands, and waits
    for a look judged in time while looks run out of time; a dialog
    that opens in that window receives the paste, which still carries no Enter.
  - A buffer that may hold secrets sends its path and lines only, never its text, and the echo
    area says so. A note goes as typed or yanked. A send is at most 256 KiB.
  - Following a message selects the sender's terminal, so it is `Person` too. Names that other
    programs give in Claude Code's registry are offered for a mention only without spaces,
    control or invisible characters.
  - A message's sender is read from a line of the screen, which any program in the terminal
    can print. It only sets the row's note and the session `session-follow-message` shows, and
    authorises nothing.
  - A language server's diagnostics go as its text, which a dependency's author may write to
    steer the agent ([ADR 0138](adr/0138-language-servers.md), step 11): every control, bidi or
    invisible character in it is written as `\u{…}`. `session-send-diagnostics` sends each
    one's first line; `session-fix-error` puts the whole message, at most 2 KiB, in a fence of
    its own under a line saying a language server wrote it, so that it cannot pass as the lines
    or the ask that follow. Neither sends anything of a buffer that may hold secrets.
- The agent channel's `diagnostics` (ADR 0138, step 11) gives the servers' text to the agent
  escaped as above, the messages, sources and codes and the paths a server names alike, at most
  500 entries and 512 KiB, made off the UI thread. It refuses a buffer that may hold secrets or
  that only the person reads, as `text` does, and leaves such buffers, those an agent's open
  still waits on, and the servers' reports of their files out of a project's. A file no buffer
  holds is judged off the UI thread by where its path really leads, links and all, as the
  loader judges a buffer's, so that a project's `conf.ts` linked to a credentials file gives
  nothing. `state`'s counts follow the same check.
- `state`'s `list` (ADR 0138, step 15) gives the agent the list of a server's places or of a
  project's diagnostics that the person has open, as a frame shows it: the rows shown and the
  preview's rows as drawn, at most the float's rows and 256 bytes a row. It gives nothing while
  a buffer that may hold secrets is shown, and never the person's input itself, though the rows
  shown follow it. A row of a file whose name may hold secrets, or of a buffer the channel may
  not read or is still opening, gives its path and line only, and the preview of one is
  withheld; the rows' paths and texts are escaped as a server's text is. The preview's rows are
  what the person sees, as `text` gives a buffer's rows; a file outside the project shows as the
  person sees it, as the reader withholds those that may hold secrets. `M-g d` leaves out a file
  whose path really leads to one that may hold secrets, judged off the UI thread as the
  `diagnostics` tool judges one. The agent still opens no such list itself.
- `*Session explain*` shows
  screen text and is withheld from the agent channel, which can neither read it nor switch to
  it; what the channel's commands say or kill while it is shown stays out of its reach.
  `session-explain-file`, which reads a file, is `Person`.

- A window docked under a session's transcript, as the composer is
  ([ADR 0170](adr/0170-docked-windows-under-a-sessions-transcript.md)), is the person's: an
  agent's move into one is undone, its `Change` commands are refused while one is selected, and
  one whose buffer is withheld from agents is not listed in the channel's state, where its host
  gives its whole rect so that the composer's height says nothing of what was typed. A docked
  window's buffer changes only by its host's dock, never by a command that shows a buffer.

### Denial of service (T8)

- Files load in the background, and the UI stays responsive on huge files and
  long lines. Only regular files are opened, so a pipe or a device cannot
  block or feed the loader forever. Opening never blocks on a special file: the
  loader opens without waiting and checks the open file
  ([ADR 0087](adr/0087-finding-files-writing-under-a-new-name-and-large-files.md)).
  Directories for `C-x C-f` are listed on worker threads, at most 4, bounded to
  100,000 names each, and no listed file is opened. A file of 2 GiB or more asks
  before it is read, as running out of memory aborts with no emergency save. Rendering shapes only the visible
  columns of a row, and rasterizes a bounded number of new glyphs per frame
  ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)).
- Parsing runs in the background with cancellation and timeouts. Tree-sitter parses no file over
  2 MiB, and stops highlighting a buffer whose first parse passes 5 s of the worker thread's CPU
  time or a later one 1 s, or whose highlighting takes 2 s of it (ADR 0111, as built; ADR 0172).
  The time is the thread's, at most the wall clock's: on a loaded machine the wall time crafted
  text takes to fail grows with the load. A buffer that may hold secrets, including one
  not judged yet, gets no job, and one that turns secret is dropped from the worker after the
  event that made it so: what waited for it never runs. Its
  queries are oso's own, compiled in, with a match limit; nothing is loaded at run time
  ([ADR 0111](adr/0111-tree-sitter-highlighting.md)).
- Opening a review parses text the person never opened: both sides of each changed file, the
  old one from git's history, as documents no buffer owns, which an agent's changes can reach
  with no other step. They go through the same worker, limits, stacks and caught panics as
  buffers, at most 8 a review at once, and only for a file every name of which is judged
  ordinary (`Entry::verdict`, the review's own check before it reads, with
  `Buffer::judge_name`); the scheduler refuses any other verdict. An abort in a grammar's C
  code is not caught, as for a buffer
  ([ADR 0122](adr/0122-agent-review.md), step 9).

### Untrusted parsers (T10)

- Image and PDF support (M4 and later) will use pure-Rust decoders, or isolate
  parsing in a separate process. How the terminal's Kitty images are decoded is ADR 0120's step 12
  ([ADR 0120](adr/0120-the-terminal.md)).
- The terminal engine, libghostty-vt, parses untrusted output in `oso-term`'s FFI: memory-unsafe
  Zig and C++ in oso's process, the second exception after the grammars. It is pinned to a signed
  release with one checked-in patch, so that row copies never abort (step 3 as built), built
  offline, and fuzzed through `oso-term`'s target at each change to its lock (`just fuzz-term`,
  an hour each built ReleaseFast and ReleaseSafe, on the Mac mini, and on Linux; a short run in
  `just test`); a crash in it ends
  oso with no emergency save, as one in a grammar does. Its Kitty images are decoded by Wuffs inside
  it, not by a pure-Rust decoder, unless ADR 0120's step 12 builds it without them
  ([ADR 0120](adr/0120-the-terminal.md), step 1 as built).
- Tree-sitter's grammars are the exception: generated and hand-written C that runs in the oso
  process on untrusted files ([ADR 0012](adr/0012-tree-sitter.md)). They are few, pinned to exact
  versions, with each `build.rs` and scanner read before a bump, and never run on a buffer that
  may hold secrets. One, tree-sitter-yaml, is built from oso's copy in `patches/`, which bounds
  what its scanner serializes; every file of the copy is hashed in a list a guard test checks,
  the directory needs approval as dependencies do, and `cargo audit` does not look a patched
  crate up, so RustSec is checked for it by hand
  ([ADR 0157](adr/0157-highlighting-html-css-json-and-yaml.md)). A memory error in one ends oso with no emergency save
  ([ADR 0111](adr/0111-tree-sitter-highlighting.md)).
- Fuzz targets (`cargo-fuzz`) cover file loading, decoding, highlighting and
  layout. What a language server sends is fuzzed through proptest targets in `oso-lsp`'s
  modules (framing, JSON, URIs, positions; [ADR 0138](adr/0138-language-servers.md)).

### Development (T9)

- The dev control socket and hot reload exist only behind cargo features that
  release builds never enable. `just release-safety` runs in CI and fails if
  a release build enables a `dev` feature, depends on a dev-only crate or
  contains a dev marker string. Each check first proves it can see the dev
  code in a dev build ([ADR 0032](adr/0032-control-socket-transport.md)).
- The control socket ([ADR 0032](adr/0032-control-socket-transport.md)):
  - lives in a private directory (`$XDG_RUNTIME_DIR/oso`, or `oso-<uid>` in
    the temporary directory), created `0700` and refused if it is a symlink,
    owned by another user or has a looser mode, or if other users can
    write to its parent and it is not sticky, and on macOS by the same rule for
    access lists, on the directory and its parent
    ([ADR 0065](adr/0065-access-lists-on-private-directories.md))
  - checks on both ends that the peer runs as the same user (`getpeereid`,
    `SO_PEERCRED`)
  - limits request and response lines to 1 MiB, clients to 4 (each disconnected after
    60 s idle) and `send-keys` to 4096 keys, and never blocks the UI thread
  - is not available on Windows yet
- Processes spawn only through `oso_platform::process`: executables come from
  an absolute path or from `PATH` without relative or missing entries, the
  current directory or the project, children get an allowlisted environment and a closed
  stdin unless the caller pipes it (a review's `git cat-file --batch`), and every spawn is logged. It ships in every build since the terminal
  needs it ([ADR 0120](adr/0120-the-terminal.md)); a child in a terminal gets the
  terminal's slave as its streams and controlling terminal, in a session of its own, and
  neither end of a terminal reaches any other child. One descriptor may be passed, as the
  child's descriptor 3 (`oso-restart`'s pair,
  [ADR 0133](adr/0133-keeping-terminals-over-an-update.md)). On macOS and Linux a child gets
  only its standard streams and that descriptor, however the rest were made, and is started
  with `posix_spawn`, never a fork, but a terminal's child on macOS, whose hook marks every
  other descriptor close-on-exec ([ADR 0137](adr/0137-starting-processes-with-posix-spawn.md)). `oso dev` builds offline,
  so a changed lock file fetches nothing by itself
  ([ADR 0047](adr/0047-process-spawning-and-restart-handoff.md)).
- The state handoff never carries a secret buffer's content, and no position in one
  either, so a restart starts it from the top
  ([ADR 0058](adr/0058-word-motion-prefix-argument-and-secret-positions.md))
  ([ADR 0047](adr/0047-process-spawning-and-restart-handoff.md)). It carries the
  unsaved text of other buffers, and kills that came from neither a secret buffer nor
  the clipboard, in pieces through the dev socket or `oso-restart`'s pair and memory only,
  never a file. A buffer
  that may hold secrets with unsaved edits holds the restart, and the refusal names it
  without quoting it ([ADR 0055](adr/0055-oso-dev-never-loses-unsaved-text.md),
  [ADR 0090](adr/0090-oso-dev-carries-unsaved-text.md)). The new instance judges each
  carried path again and withholds the buffer until then. Minibuffer histories are carried
  too; they never hold an answer given around a secret buffer or a passphrase. The
  fingerprints of secret kills are not carried, so a secret copied to the clipboard before a
  restart yanks back as ordinary text after it, under `oso dev` or `oso-restart`, unless the clipboard
  kept the concealed mark. Build errors reach the editor as
  cargo's message and span, never its rendered text with escape sequences.
- Dev builds trust the same user: any process running as the same user can
  run any command through the socket. Release builds contain none of this, only the agent
  channel above, with its own socket and fewer methods, and the state handoff for the person's
  `oso-restart`, over a socket pair with its own child and none of the dev methods
  ([ADR 0133](adr/0133-keeping-terminals-over-an-update.md)).
  This is reviewed when commands that spawn processes arrive (M3 onward)
  ([ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md)). A sandboxed
  client of the same user is trusted too, so giving it the socket gives it what
  the dev instance can read ([ADR 0051](adr/0051-m2-plan-decisions.md)). `send-keys` also
  edits any buffer it can show, secret or not ([ADR 0056](adr/0056-typing.md)). With
  `open-file` and `save-buffer` a client writes any file the dev instance may write, with no
  prompt of its own tool asking, and outside any sandbox of its own. `save-buffer` writes
  only where the file was, so this gives no way to read a secret buffer
  ([ADR 0059](adr/0059-safe-save-and-the-exit-guard.md)).
- Secret-pattern buffers give no text and no pixels over the socket: while
  one is shown, `screenshot` fails, `get-text` fails and `get-state` returns
  only its metadata, without `cursor`, `scroll`, the window's selections or the modeline and
  echo area ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)), and later
  match counts ([ADR 0051](adr/0051-m2-plan-decisions.md)), and
  without its size while it is modified
  ([ADR 0058](adr/0058-word-motion-prefix-argument-and-secret-positions.md)), or at all for a
  copy being looked at or compared ([ADR 0064](adr/0064-recovering-an-emergency-copy.md)). What a command
  says while one is shown stays out of the log and `*Messages*`, but for a panic, which names
  only the command, and leaves the echo area once the window moves on, and its modeline shows
  no position. The path as opened, each symlink's target and the
  canonical path all count, so a link to a key or through a secret path is
  caught, and a path that cannot be resolved counts as secret. A file counts as
  secret from the moment it is visited until the loading thread has resolved
  its path ([ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md)).
- A pair of windows that scroll together moves nothing in step while either shows a buffer
  that may hold secrets, whatever its verdict was when they were paired, so no position in one
  reaches the other window, which the socket and the agent channel describe
  ([ADR 0122](adr/0122-agent-review.md), step 10).
- While a secret-pattern buffer is shown, `get-state` gives no `modified`, `saving` or size of
  such buffers, and `check-handover`, `export-state` and an `exit` with a generation are refused
  for one reason, whatever the buffers hold. `invalid_bytes` is null for such a buffer. Keys in
  progress, a composition and the prompts that were around one stay out of `get-state` and
  `screenshot` until they end, also once it is gone, and those prompts' answers enter no
  history. A restart does not show such a buffer again from behind an internal one
  ([ADR 0079](adr/0079-state-and-restarts-around-a-secret-buffer.md)).
- `take-timings` records no key if a secret-pattern buffer is shown just before or after
  it or when its frame is presented, and no frames while one is shown, but the first
  frame ([ADR 0060](adr/0060-timings-while-a-secret-buffer-is-shown.md)). It times no load of
  such a buffer, and no first screen drawn while one is shown. While one is shown, the log
  keeps warnings and errors only, from every crate
  ([ADR 0078](adr/0078-load-times-and-log-lines-around-a-secret-buffer.md)).
- Responses are safe to print: DEL, C1 controls, bidi controls and invisible
  characters from a file go out as `\uXXXX`
  ([ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md)).
- Screenshots are a dev-only exception to keeping screen and buffer
  contents off disk. They are written `0600`, without following symlinks,
  into the private directory, and removed on a clean exit or by the next
  dev instance.
- Human approval is required for:
  - changes to `Cargo.toml`, `Cargo.lock`, `deny.toml`, `.claude/` and the
    files that define the checks (`Justfile`, `clippy.toml`,
    `rust-toolchain.toml`, `.cargo/`, `.github/`, gitleaks config).
    `.claude/settings.json` asks before agents edit them, and the pre-commit
    hook asks again before a commit that touches them. In auto mode these
    prompts do not reach a human, so agents must ask in the conversation and
    PR review is the backstop.
  - new `unsafe`
  - changes to process, file-writing or trust code
- Secret scanning runs through GitHub push protection, a gitleaks scan in the
  Claude Code pre-commit hook, and a full-history gitleaks scan in CI. Both
  scans ignore `gitleaks:allow` comments, and CI rejects gitleaks allowlist
  files.
- CI actions are pinned to commits by the workflow lockfile, which GitHub requires
  of every workflow, the workflow token is read-only, and checkout does not persist
  credentials ([ADR 0021](adr/0021-development-tooling.md),
  [ADR 0080](adr/0080-workflow-dependency-lockfile.md)). Tools a step downloads are
  checked against pinned checksums.

## What must never be committed

The repository is public. Never commit secrets, personal paths or environment
details, employer or client names, or descriptions of unfixed vulnerabilities.
Report and fix vulnerabilities through GitHub private security advisories, and
document them only after the fix.
