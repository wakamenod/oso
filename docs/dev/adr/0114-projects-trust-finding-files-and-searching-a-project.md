# 0114. Projects, their trust, finding their files and searching them

- Status: Accepted
- Date: 2026-11-12

## Context

The roadmap's M3 holds "project trust model" and "project file finding and asynchronous ripgrep
search". Emacs's `project-find-file` and consult's `consult-ripgrep` are the models. All three need
the same idea first: which directory a file's project is.

What is fixed already:

- [ADR 0009](0009-project-trust.md): projects open in restricted mode, shown in the modeline
  without a popup, and `M-x trust-project` trusts one. Trust lives in the person's configuration,
  never in the repository. Restricted mode allows viewing, editing, saving, in-process
  highlighting and git status, and a terminal; trusted mode adds language servers, formatters,
  git write operations and org babel. Nothing executes on open.
- [ADR 0010](0010-process-and-network-policy.md) and [ADR 0047](0047-process-spawning-and-restart-handoff.md):
  only `oso_platform::process` spawns. It finds a program by an absolute path or on `PATH`
  without the current directory, the project and anything under it, clears the environment to
  an allowlist, closes standard input and logs each spawn. It is behind the `dev` feature, and
  `just release-safety` fails if its marker string is in a release binary. Only the `regex`
  crate matches regular expressions.
- [ADR 0011](0011-git-integration.md): git's read-only information comes from `gix` in-process,
  because running `git` in an untrusted repository can run commands from its config.
- [ADR 0028](0028-secrets-handling.md) and [ADR 0044](0044-control-socket-trust-and-secret-buffers.md):
  files whose names match the secret patterns open as buffers that may hold secrets, judged on
  the loader thread from every path on the way. `Buffer::name_may_hold_secrets` judges a path
  by its name alone.
- [ADR 0076](0076-completion-and-m-x.md) and [ADR 0096](0096-completion-on-a-worker-past-5000-candidates.md):
  completion runs in a centred float with orderless components and smart case. Past 5000
  candidates a worker, `oso-complete`, builds and matches the list. A list holds at most 100,000
  names, for memory.
- [ADR 0087](0087-finding-files-writing-under-a-new-name-and-large-files.md): `C-x C-f` lists
  directories on worker threads and opens nothing to list them.
- [ADR 0088](0088-switching-and-killing-buffers.md): a command with a preview splits the float,
  the list on the left and the preview on the right. A buffer that may hold secrets is never
  previewed.
- [ADR 0082](0082-isearch.md): isearch matches on the UI thread within a budget and on a worker
  past it, with smart case. Nothing of the string reaches a log.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md): each command declares a reach. Trusting a
  project and anything that spawns a process are `Person`, and the channel never saves oso's
  configuration or trust files. `find-file` is `Person`, since it opens a prompt.
- [ADR 0108](0108-the-user-configuration-file.md) and [ADR 0110](0110-adr-0108-as-built.md):
  one configuration file in oso's configuration directory, read off the UI thread only when
  no other user can change it or the way to it. No repository file is read as configuration.
  The channel never saves into the configuration directory.

Measured on the author's MacBook (Apple silicon, warm file cache), with a prototype that walks
with the `ignore` crate and matches with `regex::bytes`, against `rg` 15.1 with `--no-config`:

| Work | `rg` | Prototype |
|---|---|---|
| Listing 45,941 files, 917 MB (`--files --hidden`) | 0.105 s | 0.105 s |
| `unsafe impl`, 35,045 matching lines | 0.54–0.56 s | 0.51–0.54 s |
| `fn \w+_new\(` | 0.54–0.67 s | 0.51–0.59 s |
| `AKIA[0-9A-Z]{16}`, one match | 0.54–0.56 s | 0.51 s |
| `(?i)deprecated`, 15,690 lines | 0.54 s | 0.51–0.53 s |
| The oso repository: listing / `unsafe impl` | 0.014 s / 0.036 s | 0.015 s / 0.029 s |

Both listed the same 1,833 files of the oso repository, `.git` included, which both must leave
out. Reading the files costs most of the time, and the `regex` crate is the engine `rg` uses.

## Decision

The author chose (a) on every question below on 2026-11-12, through the coordinator. Q16 does
not apply, as the search runs in-process. The author also approved the `ignore` crate and what it
adds to the lock: `globset`, `crossbeam-deque` and `crossbeam-epoch`. The text names the question
each part answers.

### What a project is

- **A project is a directory holding a version-control marker: `.git` (a directory, or a file as
  in a worktree or submodule), `.jj` or `.hg` (Q3).** Its root is the nearest such directory
  above the file, so a submodule or a repository cloned inside another is its own project. A
  file with no marker above it is in no project.
- **Detection runs nothing and reads no file but a `.git` file.** It asks the metadata of each
  directory and of the three names in it, from the file's directory up, following no link: a
  link is no marker. It stops at the home directory, which is never a project, at a directory
  or marker another user owns (root aside), as git's `safe.directory` does, and where the file
  system changes, as git does by default. It starts from the place the file's path leads, once
  the loader has found it, and runs on a thread of its own, `oso-project`, so neither the
  file's text nor the UI thread waits for it; a slow or hung mount delays only other projects.
- **The editor keeps a table of the projects its buffers are in:** root, a name (the root's last
  component, shown as any label is, with controls boxed) and whether it is trusted. A buffer
  points into it. A buffer with no file, such as `*scratch*`, is in no project.
- **A worktree belongs with its main repository for trust (Q4).** A root whose `.git` is a file
  of at most 1 KiB, whose `gitdir:` line names `<main>/.git/worktrees/<name>` once `.` and `..`
  are worked out by the words alone, claims `<main>`. Detection keeps the claim and looks up
  nothing it names. The claim counts only when `<main>` is trusted and that directory's own
  `gitdir` file names this root's `.git` back, which step 2 checks within the trusted
  repository, never following a link at the worktree's place. Anything else, a submodule
  (`.git/modules/…`) among them, is a project of its own.

### Trust

- **Restricted mode, the default, allows everything M3 builds:** viewing, editing, saving,
  highlighting, git status, EditorConfig, finding a project's files and searching them. With the
  recommended search (Q1) nothing of it starts a process. Trusted mode adds, from M4, what runs a
  project's code: language servers, formatters, format on save, git write operations and org
  babel, as ADR 0009 says.
- **Trust never makes oso read a project's settings.** A program that runs for a trusted project is
  one the person's configuration names. A repository can only hide files from listings and
  searches, through its ignore files.
- **The record is a file oso writes, `trusted-projects`, in oso's configuration directory (Q6),**
  beside `config.toml`, which oso still never writes. Each line is a root's absolute path and
  the root directory's device and inode (Q5):

  ```
  # Projects oso trusts to run their code. Written by oso; delete a line to stop trusting.
  /Users/me/src/oso	dev=16777230 ino=48211907
  ```

  A root whose path is not UTF-8 or holds a control character cannot be trusted, and the command
  says so. Windows, which the author does not use yet, records the path alone.
- **A project is trusted when its root's path and its device and inode are both in the file.** A
  directory deleted and cloned again at the same path, or a file synced to another machine, is
  restricted until trusted again. The modeline says which.
- **The file is read as `config.toml` is** ([ADR 0110](0110-adr-0108-as-built.md)): on the
  `oso-config` thread at start and when it changes, through `privacy::who_can_change`, at most
  256 KiB of UTF-8. **It is refused when anyone else can change it or the way to it, a group
  included,** since it decides what runs. A refused or broken file trusts nothing, and
  `*Messages*` says why. A line it cannot read is skipped and counted.
- **`trust-project` (`Person`)** asks, from the keyboard only, "Trust ~/src/oso? Its code may run
  through language servers, formatters and git hooks (y or n)", naming the root of the selected
  window's buffer, and on `y` writes the file on a worker by the saver's safe save, mode 0600.
  **`untrust-project` (`Person`)** removes the line without asking. Both refuse in a buffer with
  no project. Neither is ever run by the channel (ADR 0085); its save guard already covers the
  configuration directory.
- **The modeline shows the project (Q8):** its name, and `restricted` in the dim face while it is
  not trusted. A buffer in no project shows nothing.
- **All of this is built in M3 (Q7),** before anything is gated by it, so that M4's first spawn
  only asks `Editor::project_trusted`. That spawn looks at the root's device and inode again on
  its worker, as the record may be older than the buffer.

### Which files belong to a project

Listing and searching walk the same files, in-process, with the `ignore` crate (Q1, Q2):

- **Hidden files are included (Q9),** so `.github/`, `.gitignore` and `.claude/` are found. The
  `.git`, `.jj` and `.hg` directories never are.
- **Ignore files apply as they do for `rg`:** `.gitignore` inside a git repository,
  `.git/info/exclude`, the person's global excludes, `.ignore` and `.rgignore`. They are read as
  patterns only.
- **A directory that holds a marker is another project and is not entered,** so every file listed
  is one whose project is this one.
- **Links are not followed.** A link to a file is listed, as opening it goes through the loader's
  full verdict, but it is not searched, so a link in a repository cannot make a search read
  `~/.ssh/id_rsa`.
- **At most 100,000 files are walked (Q13),** for both. Past that the walk stops, and the count
  says `+`.

### Finding a file: `project-find-file`

- **`C-x p f` (Q15), `Person`.** It opens the completion float with the label `Find file in oso:`
  and the project's files as paths relative to its root. Outside a project, see Q10.
- **Listing runs on a worker and streams.** A walker thread sends the paths it has found every
  50 ms, and the list grows while the person types. Up to 5000 they are matched on the UI thread,
  past that on `oso-complete`, as ADR 0096 says. A batch that arrives matches only its own paths
  and merges them in, so the UI thread's share of a batch stays under 1 ms. The count shows `…`
  until the walk ends, and the selected row stays selected as rows arrive, as a result does in
  ADR 0096.
- **The order is ADR 0076's:** history first, then length, then name. The history is the files
  opened from this command, per project.
- **The list is walked again each time the command runs.** There is no cache to go stale, and the
  file system's own cache makes a second walk cheap.
- **`RET` visits the file** as `C-x C-f` does: the loader judges it, so a file that matches the
  secret patterns opens as a buffer that may hold secrets. Its name is listed like any other.
- **No preview,** as `C-x C-f` has none.
- `C-g`, closing the prompt or a newer listing stops the walker at its next directory.

### Searching: `project-search`

- **`M-s r` and `C-x p g` (Q15), `Person`.** The float's label is `Search oso:`. Outside a
  project, see Q10.
- **The input is ADR 0076's orderless components, matched against each line (Q12):** a line
  matches when every component does, in any order. A component is a literal or a regular
  expression as in completion, `!` excludes lines, and smart case holds over the whole input as
  in isearch. A search starts once a component that is not excluded has 2 characters.
- **It runs in-process (Q1), on a thread of its own, `oso-grep`,** which walks and hands files to
  `min(cores - 1, 8)` searching threads, at least one. Each key starts a new search with a new
  generation, which every thread reads before each file and each block; a superseded search
  stops there and reports nothing. An input that only narrows the last one, by ADR 0076's rule,
  filters that search's results instead of walking again, if that search ended without a limit.
- **A file is read in blocks of 256 KiB cut at a line's end.** A line longer than that grows the
  block up to 16 MiB; past that the rest of the file is skipped. A NUL byte stops the file, as
  `rg` stops at a binary one, keeping the rows found before it. Text is read as UTF-8, with
  invalid bytes shown as buffers show them.
- **Each row is `path:line: text`,** the path relative to the root, in the annotation face, and
  the matches in the match face. The text is at most 512 bytes of the line, around its first
  match, cut at character boundaries; a `\r` before the line's end is dropped. Controls, bidi
  controls and invisible characters are boxed, as in a buffer.
- **Rows stream in as `oso-grep` finds them,** in batches at most every 30 ms. A file's rows come
  together and in line order; files come in the order the threads finish them, and rows never
  move once shown. The count shows `…` while the search runs.
- **A search keeps at most 10,000 rows (Q13).** It stops there, and the count says `10000+`.
- **The preview splits the float as ADR 0088's does,** and shows the selected row's file around
  its line with the match highlighted. An open buffer of that file is shown from memory. Any other
  file is read on a worker, up to the line and the rows after it, at most 16 MiB, and a newer
  selection cancels the read. The preview is read-only and opens no buffer.
- **A file that may hold secrets is searched, but its text never reaches the screen (Q11).** Its
  rows say `path:line` and `may hold secrets`, with no text, and it is never previewed. The
  verdict is `Buffer::name_may_hold_secrets` on the path walked, which links do not reach.
- **`RET` visits the file at the line and the match's column,** through the loader as above, and
  pushes the old place on the mark ring. `C-g` stops the search and closes the float.
- **Unsaved edits are not searched:** the files on disk are, as `rg` does.
- **Nothing of the input or the rows reaches a log or `*Messages*`.** `Debug` of a job or a report
  gives its generation only. A search opened beside a buffer that may hold secrets keeps no
  history, as any prompt does there.

### The agent channel

- **`project-find-file`, `project-search`, `trust-project` and `untrust-project` are `Person`.**
  The channel never sees a prompt's candidates or rows, as ADR 0085's `state` already holds.
- **The channel gets no search of its own now (Q14).** An agent such as Claude Code has its own
  search over the same files with its own permission rules.

## Questions

Put to the author through the coordinator, the recommended option first. The author chose (a) on
each; Q16 does not apply.

1. **How to search and list:** (a) in-process: a walker with ignore rules and the `regex`
   crate, on worker threads, for both listing and searching; as fast as `rg` in the table
   above, nothing to install, works when oso starts from the Dock, no process in release builds
   until M4, components in any order, and a file that may hold secrets is judged before it is
   read; costs the walker's dependencies (Q2) and about 600 lines oso maintains. (b) `rg` for
   both, through the process module; the roadmap's wording and `rg`'s maturity (encodings,
   compressed files, its edge cases), but spawning ships in release builds now, `rg` must be
   installed and found (Q16), its output must be parsed, and the input is one regular
   expression. (c) In-process listing, `rg` for searching: two sets of ignore rules that can
   disagree, and both costs.
2. **The walker (if Q1 (a) or (c)):** (a) the `ignore` crate, 0.4.25 (ripgrep's own walker,
   Unlicense or MIT, no `unsafe`, no `build.rs`). It adds `ignore`, `globset`, `crossbeam-deque`
   and `crossbeam-epoch` to the lock; the last two hold `unsafe`. The rest of its tree is in the
   lock already. Its globs use `regex-automata`, the `regex` crate's own engine. (b) oso's own
   walker and gitignore matcher on the `regex` crate: no new dependency, but gitignore's rules
   (negation, anchoring, `**`, nested files, global excludes) are subtle, and a slip lists files
   git ignores or hides ones it does not. (c) Tracked files only, from the git index through
   `gix` (ADR 0011), and no search until then: what `git ls-files` gives, but `gix` is a large
   dependency not yet added, untracked files are missing, and a project that is not git gets
   nothing.
3. **Project markers:** (a) `.git`, `.jj` and `.hg`, the nearest one winning; (b) those and build
   files (`Cargo.toml`, `package.json`, `go.mod`): a crate inside a Cargo workspace becomes its
   own project, which splits the oso repository into one project per crate; (c) (a) now, and a list of extra
   markers in `config.toml` later, as Emacs's `project-vc-extra-root-markers` (an empty
   `.project` file marking a directory with no version control).
4. **Nested projects and trust:** (a) a git worktree takes its main repository's trust, checked
   both ways as above, and anything else nested is its own project; the author's worktrees under
   `.claude/worktrees/` need no step each. (b) Every root is trusted on its own: one
   `trust-project` per worktree. (c) Everything under a trusted root is trusted, submodules and
   repositories cloned inside it too: fewer steps, but a vendored or test repository gets trust it
   was never given.
5. **What identifies a trusted project:** (a) its root's path and the directory's device and
   inode: a clone at the same path after a delete, or a synced file on another machine, is
   restricted again; a backup restored in place needs `trust-project` again. (b) The path alone,
   as VS Code does: simpler, but a different repository at a trusted path runs its code.
   (c) The path and the git remote URL: survives a fresh clone, but reading `.git/config` for it
   reads what an attacker writes, and a fork can claim the same URL.
6. **Where the record lives:** (a) `trusted-projects` in the configuration directory: ADR 0009's
   "user config", covered by the channel's save guard already, read with `config.toml`'s checks,
   and in the person's dotfiles if they sync that directory, where (a) of Q5 makes it harmless.
   (b) The state directory, where oso keeps what it writes itself (ADR 0108): the channel's guard
   and the read checks would have to be extended to it, and ADR 0009 would change.
7. **When to build trust:** (a) all of it in M3: detection, the file, the two commands and the
   modeline; nothing is gated until M4, but the indicator and the commands are lived with first.
   (b) Detection now; the file, the commands and the indicator with M4's first spawn, so no code
   exists before something needs it.
8. **The modeline:** (a) the project's name, then `restricted` in the dim face until it is
   trusted; (b) the name, and `R` in the dim face: narrower; (c) the name only, and a mark only
   once trusted, since restricted is the normal state: ADR 0009 asks for an indicator of
   restricted mode.
9. **Hidden files:** (a) listed and searched, VCS directories never; as `git ls-files` with
   untracked files, so `.github/workflows` is found, and `.env` too when nothing ignores it.
   (b) Left out, as `rg`'s default: `.github` and `.claude` are not found.
10. **Outside a project:** (a) the buffer's directory stands in for the root, except the home
    directory and `/`, which say "Not in a project"; `C-u` asks for a directory first, with
    `C-x C-f`'s completion, as `consult-ripgrep` does. (b) "Not in a project" always, and `C-u`
    asks. (c) Always ask for a directory outside a project.
11. **Files that may hold secrets in a search:** (a) searched, their rows showing the path and the
    line with no text, never previewed; `RET` opens them as buffers that may hold secrets.
    (b) Never read, and the count says how many were skipped: nothing of them is touched, but a
    search for a key in `testdata/server.key` finds nothing. (c) Shown as any file: a screen
    share or a screenshot shows them.
12. **What the search input means:** (a) completion's orderless components, matched per line in
    any order, with `!`; (b) one regular expression for the whole input, as `rg`; (c) consult's
    `#regexp#filter`: the first part to the search, the rest filtering its rows.
13. **Limits:** (a) 100,000 files walked, 10,000 rows, 512 bytes of a line, 16 MiB for one line;
    (b) 250,000 files and 50,000 rows: larger repositories in full, about 2.5 times the memory;
    (c) (a) as defaults that `config.toml` can change later.
14. **The agent channel:** (a) no search for the agent now; it has its own. (b) A channel method
    and an `oso mcp` tool `search`, `Look`: in-process only, at most 200 rows of 256 bytes, files
    that may hold secrets never read, refused while one is shown. (c) (b) and a `files` method
    listing a project's files.
15. **Keys:** (a) `C-x p f` for `project-find-file`, `M-s r` and `C-x p g` for `project-search`;
    (b) `C-x p f` and `C-x p g` only, as project.el; (c) none bound, `M-x` only, the person binds
    them in `config.toml`.
16. **Finding `rg` (only if Q1 (b) or (c)):** (a) an absolute path in `[programs] rg`, else `PATH`
    without the project, each refused if others can change it; (b) `PATH` only: an oso started
    from the Dock finds no Homebrew `rg`; (c) the configured path only: one setting to write, and
    nothing found by accident.

## Changes to accepted ADRs

Each passage below stays in its ADR as written; what holds now follows it. When this ADR is
accepted, the status of ADR 0009 becomes `Accepted, amended by 0114`, and of ADR 0088
`Accepted, amended by 0100 and 0114`.

- [ADR 0009](0009-project-trust.md), "Trust is stored in user config, never in the repository":
  in `trusted-projects` in oso's configuration directory, by path, device and inode, written only
  by `trust-project` and `untrust-project`. A git worktree takes its main repository's trust.
- [ADR 0009](0009-project-trust.md), "Restricted mode allows …": also finding a project's files
  and searching them.
- [ADR 0088](0088-switching-and-killing-buffers.md), "the preview is read-only and renders only
  buffers already open": `project-search`'s preview also shows a file read from disk on a worker,
  never one that may hold secrets, and opens no buffer.
- `ui.md`, "only renders already-open buffers or files the trust model allows reading": a search's
  preview reads any file but one that may hold secrets, in either mode.

## Consequences

- Finding and searching work in every project, trusted or not, and start no process. Nothing in
  M3 needs the process module in a release build.
- Trust exists and is shown before it gates anything. M4's language servers, formatters and git
  write operations ask `Editor::project_trusted` and look at the root again before they spawn.
- Files placed inside a trusted project without a marker of their own, such as an archive
  unpacked there, are that project's and share its trust. That is what trusting a directory means;
  ADR 0009's wording already implies it.
- A worktree is recognised as git recognises it: by the main repository's record of where
  the worktree is. A directory made later at the place of a worktree removed without
  `git worktree prune` is that worktree to git, and takes the main repository's trust here
  too. The author accepted this on 2026-11-12, through the coordinator, as git's own
  behaviour; `git worktree prune` clears such records.
- A project's ignore files can hide its files from a listing and a search. They can do nothing
  else.
- A walk of a very large tree stops at 100,000 files, so the rest of such a project is not listed
  or searched. The count says so.
- A slow or hung file system stalls the walker's threads, never the UI.
- oso maintains a line searcher of its own, on the `regex` crate, and the walker's dependencies.
- `oso-core` gains a public module `project` (detection's rule, the trust record's format, a
  search's query, its line matching and its job and report, as `complete` has), and the tip crate
  modules `project` and `walk`, placed in `guardrails/layers.txt`, a guard file.

## As built

### Step 1: detection

- **Windows has no owner or device to go by,** so there the walk up stops only at the home
  directory (`USERPROFILE`) and at a link. `USERPROFILE` is also what `~` stands for there.

### Step 2: the record and the commands

- **The record has a thread of its own, `oso-trust`** (`config/trust.rs`), not `oso-config`. It
  reads with `config.toml`'s checks (`watch::read`), and refuses a group as well. A write never
  waits behind a parse of `config.toml` on its large stack, and a panic in one thread leaves the
  other going. The thread's shell (its state, wake, poke and timed wait) copies `watch`'s.
- **oso writes the record with a writer of its own, not the saver's safe save.** The saver
  works on a buffer, its baseline and its emergency copy. The record is a short file of oso's
  own, changed by commands, never from a buffer.
  - The editor queues a `TrustChange`, and the thread makes it in the file as it is then, read
    again with the full checks. A line written by hand since stays, and a file refused since is
    not written over.
  - It writes a part file of mode `0600`, named with the process's id, and renames it over the
    file. Just before the rename it checks that the file is still what it read; if another
    writer replaced it, the change is made again on what it holds, up to three times.
  - The directory is synced after the rename, as the saver syncs it.
  - It writes only in a directory no one else can change, made with mode `0700` if missing
    once the nearest existing one passes, and never over a link, as a dotfiles setup makes.
  - A part file older than a minute, of this user and another process, is taken for one a
    crash left and removed.
  - Each write is read back, so the editor holds what the file says.
- **The UI thread waits up to 1 s at exit** for changes not yet made in the file, as
  `untrust-project` already said it was done. The author accepted this on 2026-11-12, through
  the coordinator, as an exception to "never block the UI thread" for exit alone. A quit the OS
  forces does not wait.
- **Windows records the path alone,** as `std` keeps a directory's identity unstable there. A
  line of the other form for the system, an identity on Windows or none elsewhere, trusts
  nothing, so it is skipped and counted.
- **A path in the record holds no control or invisible character,** so that a line reads in
  another editor as it does in oso.
- **A dev build given `--config FILE` uses the record beside that file,** so the end-to-end
  flows never touch the person's. `--no-config` reads none, and trusts nothing.
- **`trust-project` is its own `y`,** as `revert-buffer` is: only the dispatcher's run of a
  question's yes sets `confirmed`, so a key, `M-x`, run-command, the dev socket or the agent
  channel only ask. On `y` the buffer's project root, by path and identity, must still be the
  one the question named.
- **The worktree check matches the claim with `same_place`:** component by component, a
  verbatim `\\?\` prefix being the plain one, so a Windows worktree matches what git writes.
  The main repository's record of the worktree is read by the trust record's spelling of the
  root, and the main repository is looked at before and after the read.
- **A change to the record asks for a project again** only for buffers in a worktree claiming a
  root whose line came or went, and for files whose project has not come yet, whose answer may
  have been checked against the record before.

### Step 3: finding a file

- **`C-u`, which asks for a directory first (Q10), comes with step 4,** shared with
  `project-search` (the author's choice, 2026-11-12). The history of `project-find-file` is per
  project, as decided above.
- **oso reads the ignore files itself, and the `ignore` crate only matches them.** The walk is a
  depth-first walk of oso's own (`walk.rs`) with a stack of the crate's gitignore matchers in
  `rg`'s order: `.rgignore`, `.ignore`, then, inside a git repository, `.gitignore`, the root's
  `.git/info/exclude` and the person's global excludes. The deepest directory's file decides
  within each kind. The author chose this on 2026-11-12, after review.
  - An ignore file is read only if it is a regular file that is not a link, opened without
    waiting (`oso_platform::file_meta::open_regular_here`), of at most 1 MiB. None above the
    project's root is read.
  - A worktree's root, whose `.git` is a file, reads no `info/exclude`.
  - The global excludes file may be a link, as it is the person's own, but must be a regular
    file of at most 1 MiB too. Finding it, the crate reads the git configuration files that
    name it: `$GIT_CONFIG_GLOBAL` or `~/.gitconfig`, `$XDG_CONFIG_HOME/git/config` and
    `/etc/gitconfig`, the person's or root's own.
- **A marker counts as detection counts it,** read from the directory's own listing: a `.git`
  directory or file, a `.jj` or `.hg` directory, never a link, owned by this user or root. The
  walk keeps a copy of detection's rule, as `walk` and `project` share a layer, with tests on
  both sides that keep the two alike.
- **Links are resolved by their words.** Each name on the way is looked at without following
  it, and a link's target is read and taken by its words, only while every hop stays under the
  root. A link that leads to a file under the root is listed; one that leaves the root is listed
  without a look, and the loader judges it when it is opened.
- **Limits.** A walk lists at most 100,000 files (Q13) and 32 MiB of paths, reads at most 32 MiB
  of names, 1,000,000 entries, ignored ones and the names looked at for links included, and
  16 MiB of ignore files in all. Past any of them it stops, and the count says `+`. The author
  approved the last ones on 2026-11-12. Of the ignore files, only a directory's own end the walk
  there: the root's `info/exclude` and the global excludes, read before the walk starts, give no
  patterns when they are longer than what is left, and the walk goes on (corrected in step 4).
- **Outside a project,** the directory is that of the place the file's path leads to. A buffer
  with no file, the home directory, a directory that holds it (by its words in the editor, and
  by its real place on the walker's thread) and `/` say `Not in a project`. A root whose path is
  not UTF-8 is refused, and so is a second listing while one is open.
- **One thread, `oso-walk`, serves every walk,** and a walk stops at its next entry once its
  generation is passed, sooner than the next directory said above. A batch goes when the next
  entry comes, at most every 50 ms; a host timer adds the rows waiting once a walk has sent
  nothing for 250 ms.
- **Merging.** A batch is sorted on its own, put in place by binary search and matched alone.
  Up to 1000 paths into a list of at most 5000 merge on the UI thread; anything else goes to a
  merge job on `oso-complete`. Past 5000 a job waits until the paths waiting are an eighth of
  the list, the walk ends or goes quiet. A job that fails is tried once more; if that fails too,
  the walk stops and the list shows what it has, with `+`. Measured in a release build, the UI
  thread's share of a batch stayed under 0.75 ms in the worst cases tried.
- **Changes to accepted ADRs.** [ADR 0076](0076-completion-and-m-x.md) keeps the rows that stay
  from moving: a list that grows now puts rows above the selected one, which stays selected,
  and an exact match that comes is selected unless a row was moved to.
  [ADR 0096](0096-completion-on-a-worker-past-5000-candidates.md) lets a newer job supersede the
  one running: a walk's batches instead wait for the next merge job, while an edit still
  supersedes it. A merge job that only adds rows holds no `RET` or `TAB`.
- **The history key is `<history>:<root>`,** kept on the prompt inside the crate, as
  `minibuffer::Kind` stays as it is. The `oso dev` restart carries at most 64 histories, each
  named in at most 64 bytes, those named alone first, so a long-rooted project's history is
  left out of a restart rather than refusing it.
- **`files_pending` and `files_cut` cover a project's walk,** so the float and the dev socket's
  state need nothing new. `ignore` and `globset` log only warnings and errors.
- **The walk took 0.20 s for 46,234 files,** against 0.09 s for `rg --files --hidden` and 0.19 s
  for it on one thread, on the author's Mac, warm.

### Step 4: searching a project

- **`C-u` asks for a directory first,** for `project-find-file` and `project-search`, with
  `C-x C-f`'s completion from the buffer's directory: `Find file in directory:` and
  `Search in directory:`. The directory named stands for the project, named after its last
  component. The home directory, one that holds it and `/` are refused, as outside a project
  (`Editor::listable_root`).
- **The walk lists each directory from its parent.** A directory below the root is opened from the
  one it is in by its name, following no link, and listed and read from what was opened, never by
  its path again (`oso_platform::file_meta::Directory`: `openat` with `O_NOFOLLOW`, then
  `fdopendir`; the root is opened by its path). A process may hold only a few hundred
  descriptors, so at most 32 directories stay open, the root and the deepest. One let go is opened
  again from the nearest held, name by name in the same way, only if it is still the directory it
  was by device and inode, and each name counts as an entry read; otherwise the rest of its
  entries are left. Ignore files, the root's `.git/info/exclude` and the markers' owners are read
  from the directory that holds them in the same way, and a link's hops are looked at from the
  directory before each. Windows still reaches directories by their paths.
  - A marker this user cannot open, such as a `.git` of mode `000`, is no marker to the walk,
    though detection, which only looks at it, counts it. Git cannot use such a repository either.
- **Components.** Only `!` and `=` are affixes, and only at a component's start: code often ends in
  `=` or `!`. `^`, `$` and the rest are an expression's, so `^use` is a line's start. This differs
  from ADR 0076, whose `,`, `~`, `^` and `&` mean initialism, flex, prefix and annotation, which
  mean nothing for a line; the author chose it on 2026-11-12. A component that holds a character
  an expression reads matches where either the expression or the literal does, compiled into one
  `regex::bytes` expression with completion's limits, multi-line and CRLF-aware.
- **Matching a block.** The first component that is not excluded is looked for through the whole
  block, and only a line it is found on is matched against every component, so a block costs one
  pass of its most likely component. The match that starts first is the row's.
- **The search's own copy of the split.** `project` and `complete` share a layer, so
  `project::LineQuery` keeps its own copy of completion's split at spaces and its limits:
  1024 bytes of input and 32 components.
- **Threads.** One `oso-grep` thread serves every search in turn and walks with `walk::walk_files`,
  the walk `project-find-file` uses. It hands each file that is not a link, with its directory
  held open, over a queue of at most 32 to `min(cores - 1, 8)` searching threads, at least one,
  started with the first search and kept. A file's rows go into the search's batch together, and
  `oso-grep` sends the batch at most every 30 ms, at the walk's next entry, then every 30 ms while
  the files handed out finish. With no searching thread, `oso-grep` reads each file itself.
- **Reading.** A file is opened from its directory, following no link and waiting for nothing,
  and read only if it is a regular file. A block holds at most 16 MiB, the longest line read, so a
  longer line skips the rest of its file. Each block is looked at once for a NUL and a line break.
- **Rows.** A row's text is at most 512 bytes around its first match, from the line's start when
  the match is within them and from 64 bytes before it otherwise, cut at characters, with `…`
  where the line goes on. Each invalid byte is one U+FFFD, as a buffer reads it, and the column
  `RET` goes to counts characters the same way. A file that may hold secrets is judged by
  `Buffer::name_may_hold_secrets` on the root joined with the path walked.
- **Narrowing.** A search that ended without a limit leaves the lines it found on `oso-grep`, at
  most 16 MiB of them, secret ones too, which never leave it but as rows. The editor keeps the
  generation and the query of the last such search, and a request whose input narrows that one,
  by ADR 0076's rule, names it; `oso-grep` then matches only those lines, if they are the ones it
  kept, and walks otherwise. A narrowing search that is superseded leaves them kept.
- **The list.** The search's prompt is a completion prompt in the given order, whose input never
  filters its rows: each changed input empties the list and starts a search, and rows are put
  after those shown, so the selected row stays. The editor also stops a search at 10,000 rows.
  `files_pending` and `files_cut` cover the search too, and the count is one cell wider while a
  list is cut, so its `+` always shows.
- **`RET`** gives the file's whole path and keeps the place for the command
  (`Editor::take_search_hit`), which visits it with `Editor::request_visit_at`: once the buffer
  has the line, or the whole file, the window showing it pushes its old place on the mark ring
  and goes to the line and column, each as far as the text goes. A visit that asks the large-file
  question first loses the place. The history, `project-search`, is one for every search and
  keeps the input.
- **Public items beyond the plan:** `project::LineHit` and
  `LineQuery::matches_line`, `file_meta::{Entries, Entry}` and `Directory::kind_of`, and
  `Editor::search_highlights`, through which the float draws a row's path and matches.
- **The preview is step 5.**

## Alternatives considered

- **`rg` through the process module (Q1 (b)):** what the roadmap named, and `rg`'s handling of
  encodings and compressed files. It would have needed:

  - **The process module ships in release builds,** and `just release-safety` changes from
    "no spawn marker" to "no dev spawn": a guard file, with its own approval.
  - **Where `rg` comes from (Q16):** the absolute path in a new `[programs] rg = "…"` table of
    `config.toml`, else `Executable::from_path("rg", Some(root))`. Either is refused when anyone
    but the person and root can change it or the way to it (`who_can_change`), or when it lies
    under any project root oso has seen. An oso started from the Dock has only the system's
    `PATH`, so a Homebrew `rg` needs the setting.
  - **The arguments are fixed and the input is one argument:** `--no-config --json --hidden
    --glob !.git --glob !.jj --glob !.hg --no-follow --max-columns 512 --max-columns-preview
    --max-filesize 64M --smart-case --regexp <input> -- .`, run in the root. `--no-config` and the
    allowlisted environment keep out `RIPGREP_CONFIG_PATH`, which could add `--pre` and run a
    program. No shell is involved; `--regexp` keeps an input starting with `-` from being an
    option. The input is one regular expression: rg cannot match components in any order without
    PCRE2.
  - **Its output is untrusted text.** The release build has no JSON parser, so either `serde_json`
    joins it, or oso reads `--null` text output (`path\0line:column:text\n`) and finds the matches
    again with the `regex` crate. Output is cut at 16 MiB and 10,000 rows, and the process is killed
    on a newer key, `C-g` or 30 s, and waited for on a thread.
  - **The command is `Person`, from the window only,** and is allowed in a restricted project: it
    runs the person's program, not the project's code.
- **ripgrep's libraries in-process (`grep-searcher`, `grep-regex`, `ignore`):** `rg`'s behaviour
  exactly, with encodings and binary detection, but `encoding_rs` and `memmap2` come with them,
  and the measurements show the plain `regex` crate already keeps up.
- **A prompt on opening an untrusted project (VS Code):** ADR 0009 rejected it.
- **Trust of a parent directory (all of `~/work`):** fewer steps, but a clone into it is trusted
  without a look. It can come later as a separate entry kind.
- **Caching a project's file list between runs:** faster to show, but stale without a watcher, and
  a walk takes 0.1 s at 46,000 files.
- **Sorting a search's rows by path when it ends:** a stable order, but rows the person is looking
  at would move.
- **Debouncing keys before a search:** consult does, for a process per key. A superseded
  in-process search stops at its next file, so a key needs no wait.
