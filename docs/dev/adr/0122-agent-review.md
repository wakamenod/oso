# 0122. Agent review

- Status: Accepted, amended by 0121
- Date: 2026-11-12

## Context

The roadmap's M4 puts "review and worktree tools on the `oso mcp` bridge" under the Claude Code
integration ([ADR 0031](0031-claude-code-integration.md)), and ADR 0031 has oso own "review of
changes with comments sent back to the session as a bracketed paste". This ADR designs that
review. It is one of three M4 ADRs: ADR 0120 (the terminal, Proposed
on PR #198), ADR 0121 (sessions and spaces, not written yet) and this one.

The author reviews agent changes every day in their previous Emacs setup, and wants to finish and
improve that inside oso. There, a person reads what an agent changed, comments on lines and hunks,
sends the comments to the agent as one prompt, and the review follows as the agent edits more.
What it taught:

- **Everything ran on the UI thread.** `git` ran synchronously; blob reads, fontification and the
  MCP tools ran on the main loop. A 57-file review took 1.7 s to open, 0.7 s of it fontifying,
  and a re-read 0.7 to 0.8 s. Batching blob reads through one `git cat-file --batch` took them
  from 0.8 s to 0.05 s.
- **Timers froze it.** A repeating refresh timer froze the editor once; the fix was one one-shot
  timer that re-arms while input is pending, and refreshing only reviews on screen.
- **The re-anchoring held.** Comments are kept apart from the text and carried to their line on
  every refresh by the shape of each reading, not by guessing from nearby text (see "Comments").
  A comment whose line went is marked outdated, and comes back if the line does.
- **Agent tours were slow:** a tour that walked the hunks one tool call at a time took 13 to 35 s.
- **What it lacked:** per-hunk stage, revert or "viewed"; a review that survives a restart (an
  unmerged branch); line carrying when jumping to the source from the unified view; a size cap on
  the prompt sent.

What oso has, and does not have:

- No git integration yet. [ADR 0011](0011-git-integration.md) reads git's read-only information
  in-process with `gix` (not yet a dependency) and runs the `git` CLI for writes only in trusted
  projects, since running `git` in an untrusted repository can run commands from its config.
- Projects open restricted until `trust-project` ([ADR 0009](0009-project-trust.md),
  [ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)).
- Only `oso-platform::process` spawns, with an allowlisted environment and standard input closed
  ([ADR 0047](0047-process-spawning-and-restart-handoff.md)). It is dev-only today; ADR 0120's
  step 2 ships it in release builds.
- The agent channel and `oso mcp` ship, off by default, and every command declares a reach
  ([ADR 0085](0085-agent-channel-and-oso-mcp.md)): anything that spawns a process or needs a person
  at the keyboard is `Person`. Nothing reaches a terminal's PTY from the channel (ADR 0120).
- `similar` is a release dependency of the tip crate, for comparing an emergency copy
  ([ADR 0064](0064-recovering-an-emergency-copy.md)), with a deadline and size bounds.
- Tree-sitter highlighting runs on two workers, per buffer, up to 2 MiB
  ([ADR 0111](0111-tree-sitter-highlighting.md)). Windows split
  ([ADR 0112](0112-window-splits.md)). The display model has no BlockMap yet.

Measured in the oso repository (905 files) on the author's Mac: `git diff --raw --numstat -z` over
347 changed files takes 70 ms; a baseline snapshot through a throwaway index takes 80 ms, and the
previous setup measured 25 ms for 206 files once the index's stat cache is copied in.

### What others do

- **The author's previous Emacs setup:** described above. Scopes from a menu (session changes,
  uncommitted, staged, unstaged, branch against base, a commit or range, a PR through `gh`). Line
  and hunk comments, re-anchored by shape, outdated kept and still sent. Sent comments stay, dimmed.
  A files pane with a filter. Eight MCP tools, all auto-allowed; the agent cannot write or edit the
  person's comments, but may remove them. No accept or reject.
- **hunk** (a terminal review viewer for agent changes): every input becomes one review document
  in one store that the UI, the agent API and extensions all change through named intents; the
  renderer keeps only geometry. File identity is by path, content identity apart. Watch mode
  debounces (200 ms quiet, 1 s at most), checks a cheap signature off the render path, and keeps
  the viewport, the selection, notes and an unchanged file's draft across reloads. Agent notes
  start hidden; feedback is pull-only; nothing stages or reverts; notes die with the window. Its
  triage example drops a hunk's decision when the hunk's content changes, "rather than silently
  transferring a decision to changed code". It refuses refs that look like options.
- **Claude Code Desktop's diff:** line comments collected into one batch and sent together; for
  the agent's own findings, "walk through findings", "fix this one" and "apply all".
- **magit:** hunk and line staging from the diff (`s`, `u`, `k` to discard, with a question), and
  `RET` visits the file at the line. Refinement of the current hunk, or of all.
- **GitHub:** per-file "Viewed" checkboxes that clear when the file changes, outdated comments
  kept on the old diff, threads with resolve, suggestions applied from a comment, and "Start a
  review" batching comments until one submit.

## Decision

### One review model (Q1)

- **A review is host-owned state, changed only through commands.** Every key, every agent tool and
  every refresh goes through the same functions, as hunk's store does and as oso's commands are
  plain functions over host-owned state. The model holds no geometry; windows keep their own
  places.
- **What a review holds:**
  - its scope and the project (root, git directory);
  - its files, in order: path, previous path for a rename, status, mode, whether binary, too large
    or withheld, and per file a *reading*: the old side's blob id, the new side's signature, and
    its hunks;
  - each hunk's lines (side, number, text) and its *content key* (see "Decisions");
  - comments and their threads;
  - decisions (staged, reverted, viewed), keyed by content;
  - the tour, if an agent gave one;
  - the generation of the last refresh, and which files are stale.
- **File identity is the path** (and the previous path for a rename), never content, so a file
  the agent rewrites is still the same file and keeps its comments. Content identity is the
  reading's signature, kept apart.
- **Several reviews may be open**, one per scope and project. Each has one review buffer.

### What can be reviewed (Q4, Q5, Q22)

| Scope | Old side | New side |
|---|---|---|
| Session changes | the baseline tree taken when the session started | a fresh snapshot of the working tree |
| Uncommitted | `HEAD` (the empty tree on an unborn branch) | the working tree, untracked files included |
| Staged | `HEAD` | the index |
| Unstaged | the index | the working tree |
| Branch against base | `merge-base BASE HEAD` | the working tree |
| A commit or a range | `X^` or `FROM^` (the empty tree for a root commit) | `X` or `TO` |
| A pull request | the PR's base commit | its head commit |

- **The session baseline** is a tree written through a throwaway index, so the person's index and
  stash are untouched: copy the real index (keeping its modification time, so its stat cache
  applies and a racily clean entry is not missed), then with `GIT_INDEX_FILE` set to the copy,
  `git add -A --` and `git write-tree`. The copy is removed after. The tree id is kept with the
  review (see "Persistence").
- **The baseline is taken** when a session starts, through the session seam below. Until ADR 0121
  exists, the `claude` command of ADR 0121 takes it when it starts an agent in a trusted project,
  and `review-mark-baseline` takes it by hand (Q5).
- **The baseline is not referenced (Q4):** git keeps an unreachable tree for `gc.pruneExpire`, two
  weeks by default. A review whose baseline is gone says so and offers the uncommitted scope.
- **A pull request** is found with `gh pr view N --json number,title,baseRefOid,headRefOid,…` (and
  listed with `gh pr list`). Commits missing locally are fetched with `git fetch --no-tags
  --recurse-submodules=no --refmap= REMOTE pull/N/head` into `FETCH_HEAD` only, no ref of the
  person's moved. When the PR's branch is the one checked out, the scope is branch against base
  instead, so the working tree shows. Posting to GitHub is not in this ADR.
- **The menu** (`C-x v r`, a float of choices as in ADR 0117 once it lands, a completing
  prompt until then) offers each scope with its file count, counted on a worker and
  filled in as each count comes. It remembers the last choice per project.

### Running git and gh

- **Only in trusted projects, only through `oso-platform::process`, never on the UI thread.** Each
  review has one thread that runs its git commands in order; counts for the menu run on a pool of
  at most 2.
- **The arguments are oso's.** Every run starts `git --no-pager -c core.quotePath=false`, and diff
  runs add `--no-ext-diff --no-textconv --no-color`. A revision a person typed or an agent gave is
  refused when it starts with `-` or holds a control character, then resolved to an object id with
  `git rev-parse --verify --quiet --end-of-options REV^{commit}`; only ids reach later commands.
  Paths always follow `--`. A PR number is digits only.
- **The environment** is the allowlist plus values oso sets: `GIT_OPTIONAL_LOCKS=0` (so a read
  never takes the index lock from the agent's own `git`), `GIT_TERMINAL_PROMPT=0`, and for `gh`
  `GH_PROMPT_DISABLED=1`, `GH_NO_UPDATE_NOTIFIER=1` and `NO_COLOR=1`. `GH_TOKEN` and `GH_HOST` are
  passed by name only when set.
- **What git reports is read as bytes:** the file list comes from `git diff --raw --numstat -z`
  (with `-M` for renames), paths NUL-separated, never parsed from patch headers. Old-side blobs
  come through one long-lived `git cat-file --batch` per review, fed object ids on its standard
  input. Every read is bounded (see "Cost and limits"), and a run past 10 s is killed and reported.
- So `oso-platform::process` gains a piped standard input and variables set by value. Both are
  amendments to ADR 0047 below.

### Restricted projects (Q3)

- **A restricted project gets no review.** `C-x v r` says "reviewing runs git, which this project
  is not trusted to run: `M-x trust-project`", and the agent tools answer the same. Nothing runs.
- Comments, decisions and saved reviews of a project that becomes restricted again stay on disk
  and come back when it is trusted.
- When `gix` lands for ADR 0011's in-process reads, an in-process review of the uncommitted,
  staged and commit scopes in restricted projects is a separate decision (open question).

### Diffing (Q2)

- **oso diffs in-process, with `similar`.** git gives the file list and the old blobs; the new
  side is the working-tree file, read as project search's preview reads one (from the root a
  directory at a time, no link followed, regular files only, no wait), or the blob for a commit,
  the index or a PR. The diff runs on a worker, per file, with `similar`'s Myers diff and a
  deadline, and groups into hunks with no context lines (as the previous setup's `-U0`); context
  is shown from the sides on demand.
- Why in-process: each file is diffed on its own, so a refresh re-diffs only files whose signature
  changed and on-screen files go first; the same sides feed intra-line refinement, highlighting
  and re-anchoring without a second read; and no patch text from git is parsed. Hunks need not be
  git's: staging builds its own patch (see "Decisions").
- **A file's signature** is the old blob id plus the new side's blob id, or, for a working-tree
  file, its device, inode, size and modification time (and its blob id when `git diff --raw`
  gives one). A file whose signature did not change is not read again.

### The views (Q6, Q7, Q8, Q9)

- **The review buffer** is a generated, read-only buffer, `*review: <scope> <project>*`, with a
  major mode of its own (`Review`) and its keymap. It is one continuous stream: each file's
  header (status, path, `+n −m`, comment counts, decisions), then its hunks, as the previous setup
  and hunk show it. The text is built on the review's worker as a rope with its display map
  (`oso_core::load::Builder`), so the UI thread does no O(n) work, and a refresh replaces one
  file's section as one edit.
- **The layout follows the width (Q6, the author's choice).** A review takes an area of the frame:
  one window, or a pair of windows (see Q7). The area shows the two sides side by side when its
  width is 160 columns or more, as hunk does, and the unified stream when it is narrower.
  - **Two panes** are the old side and the new side as two generated buffers in a pair of windows
    that scroll together: moving in one moves the other to the aligned row. Rows are aligned by
    filler rows the generated text holds, marked by a decoration that motion skips and comments
    refuse (Q7). Each pane is then at least 80 columns.
  - **The width is judged only when the review opens and when the area's width changes:** a split,
    a closed window, a resized frame or a new font size. Never while scrolling, moving or
    refreshing. A change is judged once the width has stayed the same for 100 ms, so dragging a
    frame's edge switches at most once.
  - **Hysteresis:** the area turns side by side at 160 columns or more and unified below 150. Between
    the two it keeps what it shows, so a width near the line does not flip back and forth.
  - **Switching keeps the place:** the cursor and each window's first row are carried by anchor
    (file, side, line), as a refresh carries them. A line only one side has goes to the nearest row
    of the other.
  - **`|` toggles** between unified and two panes (stacked when the area is narrower than 160
    columns), and from then on the review keeps the person's choice through resizes. `C-u |` turns
    the automatic layout back on.
- **Intra-line refinement:** within each change block of a hunk, removed and added lines are
  paired in order and diffed by words with `similar`, on the worker, on-screen first, drawn in
  `diff.refine-removed` and `diff.refine-added`. Only blocks of at most 100 lines a side and lines
  of at most 1 KiB are refined.
- **Syntax highlighting (Q8):** each side of a file is highlighted whole by oso's tree-sitter
  workers, as a document no buffer owns, then its runs are projected onto the review's rows.
  When the file is open in a buffer whose text is the new side, that buffer's highlights are used.
  On-screen files go first. Rows draw in the diff faces until highlights come, which recolour them
  without moving anything (ADR 0111). ADR 0111's limits hold: no file over 2 MiB, nothing that may
  hold secrets.
- **Comments are rows in the review buffer (Q9):** a card under the line or hunk it is on, built
  from the model, with the author's label drawn by oso in its own face and the text indented under
  it. A comment's text cannot start a card or a label, so an agent cannot write something that
  looks like the person's comment. Cards are read-only rows like filler rows.
- **The files pane:** `s` shows a narrow window on the left listing the files: status, `+n −m`,
  comment counts as the person's and the agent's, `!` for outdated, a mark for viewed and for
  decided hunks. `RET` goes to the file in the review. `/` filters the review by path, or by
  comment text; hidden files keep their comments and the header says how many are hidden.
- **Jump to the source:** `RET` on a new-side line opens the file at that line, carried through
  edits made since the reading: the line is shifted through the hunks between the review's new
  side and the file's current text (the open buffer's, or the file's), counted on a worker for a
  file over 1 MiB. An old-side line goes to where it was removed. A line inside a changed span
  goes to the span's start. `C-u RET` opens the old side as a read-only buffer.
- **Faces** join the theme table with their contrast rules ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)):
  `diff.added`, `diff.removed`, `diff.refine-added`, `diff.refine-removed`, `diff.hunk-header`,
  `diff.file-header`, `review.comment-person`, `review.comment-agent`, `review.outdated`,
  `review.sent` and `review.decided`.

### Comments and re-anchoring (Q10, Q11)

- **A comment** has an id (never reused), an author (`Person` or `Agent`), a path, a side (old or
  new) and a line, or a whole hunk; the line's text (at most 4 KiB kept, with a hash beyond); the
  shape of the reading it was last placed in; its text; the comment it replies to; and its state.
- **States:** a draft (being typed), unsent, sent (dimmed, still shown), resolved (folded to one
  line), and outdated, which combines with the others. An agent's comment is also unread or read.
- **Re-anchoring is the previous setup's algorithm, ported.** Of each reading of a file, a shape
  is kept: the lines the diff takes out of the old side and puts into the new, by number, and the
  old side's blob id. The new side is then a sequence in which a line the old side had is its
  number there (an identity, not a text) and a line put in is its text.
  - Two readings against the same old side share every old line neither took out; those match by
    number. Only the lines put in between two shared lines are matched, by text, with each other
    (an LCS over those stretches).
  - When the old side changed (a commit under the uncommitted scope, a moved branch), the old
    blobs are diffed and old line numbers carried through those hunks first. When the old sides
    are unknown, lines match by text and order alone. When nothing can be said, nothing matches.
  - A line comment moves only to a line whose text is its own. A hunk comment carries each changed
    line of the hunk; if half or more went, it is outdated, else the hunk with most of them wins.
  - Nothing is guessed from surrounding text: a blank line or a lone brace reads like a hundred
    others.
- **Outdated comments are kept and shown** above the file's first hunk, with the line they were
  on, and are still sent. One comes back to its line if the line comes back.
- **The person comments** with `c` on a line or a hunk header, in a draft buffer in a float under
  the line, `C-c C-c` to keep and `C-g` to drop (the draft stays in the kill ring). `c` on an
  agent's comment starts a reply. `e` edits the person's own comment, which makes it unsent again;
  `k` removes it; `r` resolves a thread. `{` and `}` move between comments.
- **Agent comments are always shown (Q10),** in their own face and labelled with the agent's name,
  and the modeline shows how many are unread. `a` folds every agent card to its first line; it
  never hides them.
- **Who may change what (Q11):** an agent adds comments, replies to any comment, and edits,
  resolves and removes its own. It never edits, resolves or removes the person's. A reply takes
  its parent's anchor.

### Decisions: accept, reject, viewed (Q12, Q13, Q19)

- **Per hunk:** `a` accepts (stages), `x` rejects (reverts), `v` marks viewed. On a file header
  they apply to every hunk of the file. In the staged scope, `x` unstages.
- **A decision is keyed by the hunk's content:** its path and the exact text of its removed and
  added lines, not their numbers. A hash finds candidates and the text decides (Q19). When the
  agent rewrites the hunk, no hunk has that content any more, so the decision drops, and the file
  header says "2 decisions dropped: their hunks changed" until the person looks. A decision is
  never carried to changed code.
- **Accept stages exactly the hunk shown:** oso builds a patch from the review's hunk (with the
  context lines read from the old side) and runs `git apply --cached --unidiff-zero` with it on
  standard input. If the file moved on since the reading, the index still gets what the person
  saw, or git refuses and the review says so.
- **Reject edits the file through a buffer (Q13):** the file is opened (or its buffer used), the
  hunk's new-side lines are checked to be where the reading put them, and replaced by its
  old-side lines as one undo step, then saved by the safe save
  ([ADR 0059](0059-safe-save-and-the-exit-guard.md)). If they are not there, it refuses: "that
  hunk changed; look again". A buffer with unsaved edits asks first.
- **Undo:** `u` in the review undoes the last decision: unstage what was staged (`git apply
  --cached -R` with the same patch), put back what was reverted (the reverse edit, checked the
  same way), clear a viewed mark. The review keeps its own list of decisions, at most 1,000.
- **Progress** shows in the modeline: "7 of 12 hunks decided, 3 viewed".
- **Reach:** accept, reject, unstage and their undo spawn git or write files at the person's
  word, so they are `Person`. Viewed is the person's record of what they read, so it is `Person`
  too. No agent tool decides.

### Refresh while the agent edits (Q17)

- **What marks a review stale:**
  - a reviewed file's signature changes, by a look at the reviewed files' metadata once a second
    while the review is shown, as `loader::Watcher` looks at open files, and when the window gains
    the focus;
  - a `git diff --raw` signature that changes (files added or gone, the index or `HEAD` moved),
    checked every 2 s while the review is shown, on the review's thread;
  - a save in oso of a file under the project;
  - the agent finishing a turn or a tool that writes, through the turn seam below.
- **Debounced:** a refresh starts after 200 ms with no new mark, or 1 s after the first, as hunk's.
  One refresh runs at a time per review; marks during it make one more.
- **Off the UI thread, on-screen first:** the review's worker re-reads the file list, re-diffs
  only files whose signature changed, starting with those any window shows, re-anchors their
  comments, drops decisions whose content went, and sends each file's new section with the
  refresh's generation. A review no window shows is only marked stale, and refreshes when shown.
- **It never moves what the person has:**
  - each window keeps its first row by anchor (file, side, line, and offset), not by row number,
    and its cursors the same way; a cursor whose line went goes to the nearest line that stayed;
  - a draft is a buffer of its own, so a refresh never touches its text; if its line goes, it
    stays open and says it will be kept as outdated;
  - the files pane keeps its selection and filter;
  - nothing takes the focus, and no window scrolls to show a change. A hunk that changed since the
    person looked at it shows a `changed` mark in its header until they move onto it.

### Sending comments to the agent (Q16, Q21)

- **`C-c C-c` in the review** gathers every unsent comment of the person's, in review order, and
  shows the message in `*review message*` first, editable. `C-c C-c` there sends it; `C-g` sends
  nothing. Both are `Person`.
- **The message** begins "Review comments on the changes below. Please act on each of them." and
  names the scope. Each comment follows, as the previous setup wrote it:

  ````
  ## src/foo.rs  L42 (new)          (or L40-L48 for a hunk; "(outdated)" when it is)
  ```diff
  @@ -40,3 +40,7 @@
  …the hunk…
  ```
  In reply to the agent's #12: …
  Comment: …
  ````

  The fence grows longer than any run of backticks in the hunk. A hunk longer than 60 lines is cut
  to the 30 lines around the comment, saying so. The message is at most 256 KiB; past that the
  rest of the comments stay unsent, and the preview says how many.
- **After sending,** the comments are marked sent and stay visible, dimmed. The next send carries
  only new ones. An edited comment is unsent again.
- **Where it goes** is the delivery seam below: a bracketed paste into the session's terminal,
  sanitized as ADR 0120 sanitizes any paste, and not submitted, so the person reads it in the TUI
  and presses Enter. Until the terminal exists, the message goes to the kill ring and the preview
  says to paste it.
- Comments on a file that may hold secrets go with the file and line, without the hunk.

### The seams to ADR 0120 and 0121

These sit in the tip crate's `review` module as small traits with one implementation each until
the other ADRs land, so review is built and tested from git and files alone:

- **Deliver:** where a message goes. Now the kill ring; with ADR 0120, the bracketed paste into a
  terminal the person picks (the one `claude` started, by default); with ADR 0121, the session the
  review belongs to.
- **Turns:** "the agent finished a turn" and "a tool that writes finished", which mark the review
  stale. Now nothing (the watching above covers it); with ADR 0121, the Stop and PostToolUse hooks.
- **Sessions:** "a session started in this worktree" (take the baseline) and "which review is this
  agent's" (for the tools). Now the `claude` command and `review-mark-baseline`, and the review
  shown, else the one used last; with ADR 0121, the session's id carried by the bridge.

### Agent tools in `oso mcp` (Q14, Q15, Q20)

New channel methods, each a tool of `oso mcp`, with its reach as ADR 0085 counts it. Read tools
answer while the channel waits for the person, as `state` does; the others wait as `run` does.

| Tool | Reach | What it does |
|---|---|---|
| `review_list` | Look, read-only | the open reviews: id, scope, project, counts, whether stale |
| `review_read` | Look, read-only | one review's structure: files, hunk ids and ranges, `+/−`, decisions, comment counts; patch text only when asked for, per file or hunk, at most 256 KiB a call |
| `review_comments` | Look, read-only | comments with id, author, place, text, replies, state; by author, file or state |
| `review_navigate` | Look | moves a review window to a file, hunk, line or comment, or the next or previous comment, without taking the focus |
| `review_comment` | Change | adds a comment on a line or hunk, or a reply; a batch of at most 100, checked whole before any is added; a line not in the diff is refused, naming the lines that are |
| `review_edit_comment` | Change | edits, resolves or removes one of the agent's own comments |
| `review_tour` | Change | gives an ordered tour (see below) |
| `review_open` | asks the person | asks to open a review of a scope (Q15) |

- **There is no tool to accept, reject, stage, revert or mark viewed** (Q12). An agent that thinks
  a hunk should go says so in a comment.
- **`review_open` asks (Q15):** opening runs git, which the channel never does by itself, so it
  opens an `Agent:` question as ADR 0085's saves do: "Agent: open a review of uncommitted changes in
  oso? (y or n)", from the keyboard only, after 500 ms with no key. `y` opens it in a window beside
  the selected one, without selecting it.
- **The tour (Q20):** one call gives at most 50 stops, each a place and a note of at most 4 KiB.
  The stops become agent comments of the kind `tour`, in order. The modeline shows `Tour 2/7`;
  `n` and `p` step. At a stop, `f` adds the person's comment "Fix this." there (editable), and `F`
  adds it at every stop not yet answered: Claude Code Desktop's "fix this one" and "apply all",
  sent as any comments are.
- **Withheld:** a file that may hold secrets is listed by path and counts only; its lines are
  never in `review_read`, `review_comments` or a tour. While a review shows such a file's lines,
  the channel waits as for a secret buffer.
- **Instructions:** the bridge's server instructions say to open a review only when the person
  asks, to read structure first and patch text only when needed, to batch comments, and not to
  comment on every hunk. They come from the running oso, so they match its build.

### Secrets and display

- A file whose path matches the secret patterns shows in the review as a header with its counts
  and "may hold secrets: `RET` to show". Showing it is `Person`, makes the review buffer one that
  may hold secrets while shown, and its lines are never highlighted, saved with the review or put
  in a message.
- Paths, file text and comment text are drawn by the same rules as any buffer's: invalid bytes as
  `\xNN`, bidirectional and invisible characters shown
  ([`ui.md`](../ui.md#bidirectional-and-invisible-characters), [ADR 0066](0066-text-stays-in-logical-order.md)).

### Persistence (Q18)

- **A review is saved as it changes,** on a thread, to `reviews/` in the per-user state directory
  ([ADR 0108](0108-the-user-configuration-file.md)), a private directory (mode `0700`, files `0600`,
  with access lists counted as for emergency copies). One file per project and scope, JSON Lines
  through `serde_json`: the scope, the baseline tree id, comments with their shapes, decisions with
  their content, viewed marks, the tour, the filter and the place.
- **It comes back** when the same scope of the same project opens again, re-anchored to the new
  reading, and `review-resume` lists saved reviews. Killing a review with unsent comments says
  "3 unsent comments kept; `review-resume` brings them back".
- **Bounds:** at most 16 MiB a review; a review not opened for 30 days is removed. No line of a
  file that may hold secrets is stored.

### Cost and limits

Budgets, measured on the author's Mac by a new `review` scenario of the performance harness (its
thresholds need the author's approval):

- **A 57-file, 5,000-line review shows its first screen within 100 ms** of `C-x v r` (as "startup
  to first frame"), git's runs included, and is whole (every file diffed, refined and highlighted)
  within 500 ms, on workers.
- **The UI thread** spends at most 2 ms swapping in one file's section and never more than 4 ms in
  one turn; keys in a review meet the 8 ms key-to-photon budget.
- **A refresh after one file changed** costs one `git diff --raw` and that file's diff, and shows
  within 100 ms of the debounce ending. UI work is proportional to that file's rows.

Limits for crafted input, each saying what it hides:

- **Files:** at most 10,000 listed and 32 MiB of `--raw` output; past that the review says how
  many more there are.
- **A side:** at most 8 MiB and 200,000 lines is diffed, with a 1 s deadline per file; past any of
  these the file shows its sizes and "too large to show: `RET` opens it".
- **Binary:** a NUL in the first 8,000 bytes (git's rule) or `-` in `--numstat` shows the sizes
  only. A submodule shows its commit ids. A link shows its target's text, as git does, and is never
  followed.
- **Lines:** a line over 64 KiB is drawn as any long line is (ADR 0099); it is not refined, and a
  comment keeps 4 KiB of its text and a hash.
- **The review's text** is at most 64 MiB; past that, files collapse to their headers and open one
  at a time.
- **Comments:** at most 10,000 a review and 64 KiB each. **Agent input:** as the channel's, 1 MiB a
  request.
- **`gh` output** is at most 1 MiB, parsed with `serde_json`.

### Crates and layers (Q1)

- **`oso_core::review`** (new module, pure, no `unsafe`, no I/O): the model, the commands' state
  changes, shapes and re-anchoring, content keys, the files pane's and filter's state. The LCS that
  re-anchoring needs is handed in as a function by the caller, so `oso-core` takes no new
  dependency. It sits on `guardrails/layers.txt`'s `oso-core` line with `project`, and
  `editor` holds the open reviews.
- **`crates/oso/src/review/`** (new tip-crate module): the git and `gh` runner, the diff worker
  (`similar`), the seams, persistence, and the channel methods' bodies. A new line entry beside
  `walk` in `[oso]`.
- **`oso-syntax`** gains documents no buffer owns, for the old side and files not open (new public
  API, this ADR; its items are approved at step 8).
- **`oso_core::window` and `oso_core::editor`** gain a pair of windows that scroll together (new
  public API, this ADR; its items were approved at step 10). `oso_core::frame` keeps the pair in
  its tree with nothing public.
- **`oso-platform::process`** gains a piped standard input and variables set by value.
- **No new crate, no new crate edge, no new dependency** on the recommended options.

### Keys

In a review buffer: `n`/`p` hunk, `N`/`P` file, `{`/`}` comment, `c` comment, `e` edit, `k`
remove, `r` resolve, `a` accept, `x` reject, `v` viewed, `u` undo a decision, `s` files pane, `/`
filter, `|` two panes, `RET` source, `g` refresh now, `C-c C-c` send, `q` bury. `C-x v r` opens the
menu. The exact keys are settled in step 3 with the author.

## Steps

One PR per step; each passes `just ci` and, where it touches the window, `just e2e`.

1. **The model and re-anchoring** in `oso_core::review`, with property tests ported from the
   previous setup's cases. No UI.
2. **Git and diffing:** the process module's additions (after ADR 0120 step 2 ships it; that step
   already added variables set by value, `Spawn::set`, so only a piped standard input is left), the
   review's thread, the scopes (session, uncommitted, staged, unstaged, branch, commit, range), the
   baseline, `similar` diffs and the limits. Unit tests against temporary repositories.
3. **The unified view:** the review buffer, its mode and keys, the menu, comments and drafts,
   jumping to the source, refinement, the `review` performance scenario.
4. **Refresh:** watching, debouncing, on-screen first, keeping places and drafts; end-to-end flows
   that edit files under an open review.
5. **Decisions:** accept, reject, viewed, undo, progress.
6. **The files pane, the filter and persistence.**
7. **Agent tools:** the channel methods, the `oso mcp` tools and instructions, the tour, guard tests
   for authorship and reach.
8. **Documents no buffer owns, in `oso-syntax`** (Q8): parsing and highlighting a text that is not a
   buffer's, scheduled with the shown buffers, under ADR 0111's limits and secret rules. A new
   public API of `oso-syntax`; its items (expected: a document id apart from a buffer id, opening a
   document from a rope snapshot and a language, closing it, and its reports) are listed for the
   author's approval when this step starts.
9. **Highlighting the sides in the review:** projecting each side's runs onto the review's rows,
   on-screen first, and reusing an open buffer's highlights.
10. **A pair of windows that scroll together, in ADR 0112's tree** (Q7): two windows that move by
    a row map their owner gives, closed and resized together. A new public API of
    `oso_core::window` and `oso_core::editor`, with nothing public in `oso_core::frame`; its items
    (making and breaking a pair, the row map's type, and how a pair is laid out and saved across
    a restart) were approved when this step started (see "As built").
11. **The two-pane view and the automatic layout** (Q6): the filler rows, the 160 and 150 column
    rules, judging the width only on a change of it, and `|`.
12. **Sending:** the message and its preview, and the Deliver seam's terminal implementation once
   ADR 0120 lands. PRs through `gh` come here too.

## Questions

Recommended option first.

1. **Where the model lives.** (a) `oso_core::review`, pure, with the LCS handed in; git, diffing and
   workers in the tip crate: unit-testable, no new dependency in `oso-core`, one new module each
   side. (b) The same, with `similar` added to `oso-core`: simpler code, but a new dependency edge
   for the core. (c) All in the tip crate: no new core module, but the model's tests need the tip
   crate and agent tools and keys share less.
2. **Diff engine.** (a) In-process `similar` over blobs from `git cat-file --batch` and files read
   by oso; git gives the file list: per-file refresh, one read feeds refinement, highlighting and
   anchoring, no patch parsing; hunks may differ from `git diff`'s. (b) Parse `git diff` output:
   git's exact hunks, but every refresh re-runs and re-parses the whole diff, and paths in headers
   need unquoting. (c) `gix` for all of it: works in restricted projects, but a large new
   dependency, and ADR 0011's check that it skips config commands is still open.
3. **Restricted projects.** (a) No review; say why and point to `trust-project`: no git runs, as
   ADR 0011 requires. (b) A review only of files open in oso against their text on disk: no git,
   but a different feature with the same name. (c) In-process reads with `gix` now: a review
   everywhere, at the cost of (2c).
4. **Keeping the baseline from gc.** (a) Unreferenced, with git's two-week grace, and said when
   lost: writes nothing in the person's refs. (b) A ref under `refs/oso/`: never lost, but writes
   into the person's repository, shows in `git log --all` and must be cleaned up.
5. **When the baseline is taken before ADR 0121.** (a) By the `claude` command in a trusted project,
   and by `review-mark-baseline`: automatic for the main case. (b) Only by the command: nothing
   runs that the person did not ask for, but easy to forget.
6. **Default layout.** (a) Unified, `|` for two panes: one stream fits any width and matches the
   previous setup's default. (b) Side by side when the window is 160 columns or wider, as hunk does:
   better on a wide screen, but the layout changes with the window.
7. **Aligning two panes.** (a) Filler rows in the generated text, and two windows that scroll
   together: works without a BlockMap; needs a window pairing in ADR 0112's tree. (b) Wait for the
   BlockMap: cleaner, but the two-pane view waits for it. (c) No alignment, as ediff: simplest, but
   the previous setup found scrolling one side alone confusing.
8. **Highlighting the sides.** (a) Whole sides on the tree-sitter workers, as documents no buffer
   owns, on-screen first: correct inside strings and comments; new `oso-syntax` API and up to two
   parses a file. (b) Only the hunks' text: cheap, but wrong where a hunk starts inside a string or
   comment. (c) None at first: fastest to build, but the author's review had it.
9. **Comment cards.** (a) Read-only rows in the review buffer: works now and is searchable. (b)
   BlockMap blocks: no rows in the text, but waits for the BlockMap. (c) Only a mark and a float on
   demand: compact, but comments are not read in place.
10. **Agent comments.** (a) Always shown, with an unread count; `a` folds them: nothing the agent
    wrote is missed. (b) Hidden until `a`, as hunk does: quieter, but notes go unseen.
11. **What an agent may do with the person's comments.** (a) Reply only: the person's words and
    their state stay theirs. (b) Also remove one it dealt with, as the previous setup allows: tidier,
    but an agent steered by what it read can make a request disappear. (c) Also resolve: less
    noise, and still visible, but the person no longer decides what is done.
12. **Agent and decisions.** (a) Never: accept, reject and viewed are `Person`; the agent comments.
    (b) A per-review setting that lets the agent stage (never revert): faster for trusted work, but
    a staged hunk is one commit away from history. (c) Let the agent mark viewed: harmless to files,
    but "viewed" stops meaning the person read it.
13. **How reject reverts.** (a) An edit through the file's buffer, saved safely, checked against the
    reading: undoable in the buffer and the review, and works with unsaved edits. (b) `git apply -R`
    on the working tree: simpler, but bypasses the buffer and the safe save, and an open buffer
    then reloads.
14. **Reach of the agent's comment tools.** (a) `Change`, separate tools, not marked read-only: the
    agent's own permission rules can allow reading and keep asking for writing. (b) `Look`: comments
    change no file, but a person who allowed "look" would allow writing into their review.
15. **May an agent open a review?** (a) Through an `Agent:` question: one keypress, nothing runs
    without the person. (b) Never; the agent asks in chat and the person opens it: no question, but
    slower. (c) Freely: smoothest, but against ADR 0085's rule that a channel action never starts a
    process.
16. **The preview before sending.** (a) Always: the person sees exactly what goes into the
    terminal. (b) Only with `C-u`, as the previous setup: one key less, but a paste is unseen.
17. **Watching for changes.** (a) Polling the reviewed files' metadata each second and a
    `git diff --raw` signature every 2 s while shown, plus saves and the turn seam: no dependency.
    (b) Native file events (`notify`, a new dependency): quicker to notice, but a new crate, and
    large trees need many watches. (c) Only saves, the turn seam and `g`: cheapest, but edits made
    from a shell go unseen.
18. **Persistence.** (a) Saved automatically, kept 30 days: nothing is lost on a restart. (b) Saved
    on request: fewer files, but a crash loses comments. (c) None, as hunk: simplest, the most
    common loss its users report.
19. **Decision identity.** (a) The exact text of the hunk, found by a hash: a decision can never
    move to changed code. (b) A 64-bit hash only: smaller, but a crafted hunk could match it.
20. **The tour.** (a) One call with all stops; the person steps, with "fix this" and "fix all": no
    round trip per stop. (b) The agent navigates stop by stop: it can adapt, but each stop costs a
    turn (13 to 35 s per tour measured).
21. **What the message carries.** (a) Each comment with its hunk, cut past 60 lines, at most
    256 KiB: the agent acts without another tool call. (b) Comments with places only: smaller, but
    the agent must read the diff again.
22. **Pull requests.** (a) Fetch missing commits into `FETCH_HEAD` only, in trusted projects, as a
    `Person` command: no ref of the person's moves. (b) Only commits already present: no network
    through git at all, but most PRs need a fetch first.

### Dependencies

None on the recommended options. The alternatives that would add one: `gix` (2c, 3c), `similar` in
`oso-core` (1b) and `notify` (17b). Each would go through the `add-dependency` skill and the
author's approval.

## The author's choices

The author answered every question on 2026-11-12, through the coordinator: (a) for each, but Q6.

- **Q6: (b).** Side by side automatically when the review's area is wide enough, unified otherwise,
  and `|` still toggles. The threshold is 160 columns, with unified again below 150, and the width
  is judged only when it changes, never while scrolling (see "The views").
- Q7 (a) and Q8 (a) add a pair of windows to ADR 0112's tree and documents no buffer owns to
  `oso-syntax`. Each is a step of its own (steps 8 and 10), whose public API is listed for approval
  when it starts.
- The rest, as recommended: the model in `oso_core::review` (Q1); in-process `similar` (Q2); no
  review in restricted projects (Q3); the baseline unreferenced (Q4) and taken by the `claude`
  command and `review-mark-baseline` (Q5); comments as rows (Q9), agent comments always shown (Q10),
  the agent replying only to the person's comments (Q11); accept, reject and viewed for the person
  only (Q12); reject through the buffer (Q13); comment tools `Change` (Q14); `review_open` through
  an `Agent:` question (Q15); always a preview (Q16); polling (Q17); automatic persistence for 30
  days (Q18); decisions by exact text (Q19); a tour in one call with "fix this" and "fix all" (Q20);
  hunks in the message (Q21); PR commits fetched into `FETCH_HEAD` only (Q22).

## Changes to accepted ADRs

- [ADR 0011](0011-git-integration.md), "Read-only information (branch, status, diff gutter) is
  read in-process with `gix`": a review's diffs and file lists are read by the `git` CLI through
  the process module, only in trusted projects, as write operations are. The diff gutter and
  status in restricted projects stay as ADR 0011 says.
- [ADR 0047](0047-process-spawning-and-restart-handoff.md), "Standard input is closed": a caller
  may pipe standard input; and "the names the caller gives": a caller may also set variables by
  value. Each spawn's log line names the variables set, without their values.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "What the channel can do" and the `oso mcp` tool
  list: the review methods and tools above join them, with the reach given for each.
- [ADR 0111](0111-tree-sitter-highlighting.md), "the worker a tree per buffer": the workers also
  parse documents no buffer owns, a review's sides, under the same limits and secret rules.
- [ADR 0112](0112-window-splits.md), the window tree: it gains a pair of windows that scroll
  together and are closed and resized together.

## Consequences

- The review is the first feature to run git, so it carries the git safety rules (option-like
  revisions refused, ids only, `--end-of-options`, `--no-ext-diff --no-textconv`, no locks taken)
  for the porcelain that follows in M4.
- A review never stalls typing: git, reading, diffing, refinement and highlighting run on workers,
  and the UI thread swaps in a file's section at a time.
- Restricted projects get no review until they are trusted, or until an in-process path exists.
- An agent can read the review, point at places, comment, reply and give a tour, but cannot
  decide, change the person's comments, or run git without the person's `y`.
- Review state is written to the state directory, and holds code text from the reviewed files.
- Two new public APIs wait for the author's approval at the start of their steps: documents no
  buffer owns in `oso-syntax` (step 8) and a pair of windows in `oso_core::window` and
  `oso_core::editor` (step 10, approved 2026-11-13).
- Follow-up: the BlockMap if comments move there, ADR 0121's session and turn seams, posting to
  GitHub, and in-process review with `gix`.

## Alternatives considered

- **A read-only viewer, as hunk:** simpler and safe, but the author wants accept and reject, and
  hunk's own users ask for them.
- **Comments hidden in a side buffer, not in the diff:** the comments would be read apart from the
  code they are about.
- **Sending each comment as it is written:** no batching, and nothing to look over first.
- **Pull-only feedback, as hunk:** the person has to tell the agent to read their notes.
- **Re-anchoring by nearby text (fuzzy):** a blank line or a brace matches anywhere, and a comment
  lands on the wrong line silently.
- **Rebuilding the whole review on each refresh:** the previous setup's 0.7 s re-read; cost must
  follow what changed.
- **Running `git diff` in restricted projects with config commands turned off by `-c`:** git's
  config can name more programs than oso can list, which is why ADR 0011 keeps git out of
  untrusted repositories.
- **A review in the terminal, by running hunk or a similar tool in a PTY:** a second UI with its
  own keys, and its state out of oso's commands and channel.

## As built

### Step 1: the model and re-anchoring

`oso_core::review` (`review.rs`, with `shape`, `map`, `carry`, `comment` and `decision` under
it), on `project`'s row of `guardrails/layers.txt`. It uses only `std`. The author approved its
public items as listed in the plan (with `Applied::marks_changed` added later, item 12), and
answered its questions (a), then those of its reviews:

1. **Paths are bytes** as git gives them with `-z` (`&[u8]`, `/` on every OS), so a name that is
   not UTF-8 goes through unchanged.
2. **No per-edit numbering.** The previous setup counted lines that share a number, for diffs made
   by joining a patch per edit. oso diffs each file against one old side, so a number names one
   line, and that case is not ported.
3. **A commented line is compared whole.** This changes the rule in "Comments and re-anchoring":
   in memory, a comment keeps the reading its line is in (the one it was last placed in), and
   its line there is compared with the line it is carried to, on the worker. The first 4 KiB, the
   length and a hash of the rest (`MAX_KEPT_LINE`) are only what a saved review keeps (step 6).
4. **Removing a comment removes its replies.** The person removes any comment; an agent removes
   only its own, and is refused when a reply is the person's. Both edit only their own comments.
   **An agent resolves or reopens** only its own comment, and only while no reply in its thread
   is the person's.
5. **`editor` holds no review yet.** It gets one in step 3, when a command first needs it.
6. **The nearest line that stayed** (where a cursor goes when its line went) waits for step 4.
7. **Hunks must have an old line between them,** as any diff with no context gives them, and end
   below the last 32-bit number. `Shape::new` checks the order and the numbering, and refuses the
   rest.
8. **A bad matcher result is thrown out** for its stretch: pairs out of range, out of order or of
   unequal lines match nothing there.
9. **Hunks with one text keep their own decisions:** the first of them in one reading goes to the
   first in the next, and so on. A decision stays keyed by content, and stays on the hunk shown.
10. **A file keeps at most 16 earlier readings** (`MAX_KEPT_READINGS`) for its outdated comments
    to come back by. Comments on older ones stay outdated for good and let their reading go.
11. **A file put in against a blob carries nothing.** When one reading's old side is no file and
    the other's is a blob (a new file once committed, or a file deleted and put back), no line
    of the one is a line of the other, and lines that read alike are not taken for the same: the
    comments on them go outdated. Two readings of a file put in share their empty old side.
12. **Applying says two things apart:** `again`, when comments were added since the snapshot
    (they are outdated until carried, and not counted), and `marks_changed`, when decisions or
    viewed marks changed meanwhile and the current ones were carried in their place.
13. **A hunk comment's lines are checked like a line comment's:** each counts only where its
    text is its own.
14. **A line's bytes include its terminator** (`\n`, and a `\r` before it), so a change of line
    ending or of the newline at the end of the file is a change of the line.
15. **Later steps add:** `Review::take_released` in step 4, so the host lets go of old readings
    off the UI thread; and in step 7, a check that an agent's comment names the reading it was
    read in, and an agent identity (`open-questions.md`). The approved names stay.

What was built:

- **A `Shape`** is a reading's old side (`Base`: a blob, no file, or unknown) and its hunks with no
  context, checked by `Shape::new`. `line` and `hunk_at` find the lines a diff shows by halving.
- **The `LineMap`** is the previous setup's algorithm, but it walks the hunks' edges instead of
  each line. Whether an old line is shared changes only at an edge of a hunk of either reading, or
  of the hunks between the old sides. So the shared lines come as runs between the sorted edges,
  and only the lines put in between two runs go to the matcher. The cost follows the hunks and the
  lines they put in, not the file's length. A property test checks it against a line-by-line
  reference of the previous setup's algorithm, over same, moved and new old sides.
- **The LCS is a closure,** `FnMut(&[&[u8]], &[&[u8]]) -> Vec<(usize, usize)>`, given only the
  lines put in on each side, and it must bound its own work (the tip crate's diff has a deadline).
  The hunks between two old blobs come in as `BaseHunk`s when `base_pair` names two different
  blobs, and nothing is carried when the caller has none.
- **Carrying runs in three parts:** `Review::carry` takes a snapshot on the UI thread (a reference
  to each comment's reading, and the marks when a hunk is marked). `Carry::run`, on a worker,
  builds one map per reading carried from, places each anchor once (a thread's replies share it),
  hashes the new hunks and carries the marks. `Review::apply` writes the result by id. A snapshot
  taken before another reading was applied changes nothing and says `again`.
- **Decisions** are a mark per hunk of the current reading, so a lookup is an index. The marks are
  carried by a hash of each hunk's removed and added lines (the review's `RandomState`), and the
  text decides. Viewed is kept apart from accepted and rejected. Marks whose text left the reading,
  or whose file left the review, are counted in `dropped` until `clear_dropped`.
- **Who calls what:** `decide`, `set_viewed`, `mark_sent`, `mark_read` and `clear_dropped` take no
  author: they are the host's, at the person's word, and no agent tool reaches them (step 7).
- **Tests:** the previous setup's cases (lines put in above, look-alike lines, a moved old side
  and its shift tables, an old side that is the same or appears, matching by text, a new file
  matched whole), and properties: a reading maps to itself; the map equals the reference; a line
  carried lands on a line with its own text, in order; a lying matcher never carries a line to
  another text; a hunk comment is outdated exactly when half its lines went; and no agent command
  changes or removes the person's comments, nor reuses an id. Unit tests cover identical hunks,
  snapshots older than a reading, the kept-readings limit, numbers near the last one, a comment
  on a removed line, and a new file once committed.

### Step 2: git and diffing

`crates/oso/src/review.rs` and its modules `git`, `batch`, `id`, `list`, `scope`, `index`,
`side`, `diff`, `read` and `work`, on the `[oso]` line of `guardrails/layers.txt` with
`project` and `walk`. `oso-platform::process` gained `Source { Closed, Piped }` and
`Spawn::stdin`, refused with a terminal, and `oso_core::project::Project` gained `identity`;
the author approved both as listed. `similar` was already a dependency of the tip crate
(ADR 0064), so no dependency changed. The author answered the plan's questions and those of
its reviews (a):

1. **Untracked files are listed through a copy of the index:** the index is copied, keeping
   its modification time, into a private `reviews/tmp/` under the state directory, then
   `git add -A -N --` marks the untracked files as to be added in the copy, which writes no
   file's contents into the repository, and `git diff --raw -z -M --no-abbrev OLD --` runs
   with that copy. The new side is then read from disk, as "Diffing" says, not from a
   snapshot tree. Copies are removed after each run, and those an earlier run left behind are
   swept when a review's thread starts, once they are an hour old. The index is copied only
   if it is a regular file, up to 128 MiB.
2. **Which scopes list untracked files:** uncommitted, session and branch. Unstaged stays the
   index against the tracked files, as git means it.
3. **No `--numstat`.** This changes "What git reports" and "Binary" above: the list is
   `--raw` alone, a file's line counts come from its hunks, and a side is binary only by a
   NUL in its first 8,000 bytes, whatever the repository's attributes say.
4. **`cat-file --batch`,** as written above. A blob past 8 MiB is not read: that process is
   killed and the next read starts another. A thread of its own reads the answers.
5. **Myers' diff** as `similar` gives it, with git's work limit, within 1 s per file. It
   recurses, needing up to half a MiB of stack at 200,000 lines, so the review's thread has a
   16 MiB stack. `RawMyers` was measured too: it gives up at the deadline on inputs Myers
   diffs in milliseconds.
6. **Bounds:** besides "Cost and limits", a request spends at most 5 s reading and diffing
   files, shown files first; files past it say so and are read by the next refresh. A review
   holds at most 64 MiB of sides and hunk lines; files past it show their sizes. Carrying a
   file's comments spends at most 1 s, and says when it was cut short.
7. **The environment** also passes `XDG_CONFIG_HOME` by name, so git finds the person's own
   configuration and ignore file when they moved it, and sets `GIT_NO_LAZY_FETCH=1`,
   `GIT_NO_REPLACE_OBJECTS=1`, and `GIT_CEILING_DIRECTORIES` to the root's parent. The
   baseline's run adds `LC_ALL=C`. This adds to "Running git and gh". A root whose parent's
   path holds the list separator (`:`, or `;` on Windows) cannot be a ceiling, so it is refused
   before git runs. On Windows, a path handed to git loses its verbatim `\\?\` prefix, which
   git cannot take.
8. **Trust by construction:** a review's thread starts only from a `TrustedRoot`, which only
   `Editor::project_trusted` gives, with the root's device and inode as the project was
   found. Before each request they are looked at again, and a root that is no longer that
   directory runs nothing. git never looks above the root, and a work tree whose top is not
   the root is refused. A `TrustedRoot`'s clones share a revocation: once revoked, the thread
   refuses every later request and stops the running one between files. Step 3 revokes the
   root of a review whose project is no longer trusted (`untrust-project`).
9. **The baseline** runs `git add -A --ignore-errors --`, so a file git cannot read is left
   out, and the log says at least how many, not which. It writes the untracked files'
   contents into the repository as unreferenced objects, as any `git add` does; leaving out
   files whose names may hold secrets is a follow-up.
10. **A merge commit** is read against its first parent. A commit whose parent a shallow
    clone lacks is refused.
11. **Carrying runs on the review's thread:** `Request::Carry` runs step 1's `Carry::run` with
    `similar` as the matcher and old blobs read through `cat-file`.
12. **No command yet:** `review-mark-baseline` and the menu come with step 3.
13. **No hook and no file system monitor run.** Every run adds `-c core.fsmonitor=false` and
    `-c core.hooksPath=` an empty private directory, `reviews/tmp/hooks/` (refused if it is not
    empty); git hands both to a git it starts in an embedded repository. Without that private
    directory no git runs. Clean filters still run, as `git add` and a diff of the work tree
    need them (Git LFS is one), so a trusted project's filter programs run in a review. This
    adds to "Running git and gh".
14. **No run enters an embedded repository,** which is a project of its own: diff runs add
    `--ignore-submodules=dirty`, which `.gitmodules` cannot override, and both adds leave out
    each submodule the index copy names (`git ls-files -s -z`, read for mode `160000`), as
    `:(exclude,top,literal)PATH` pathspecs on standard input. An embedded repository the index
    does not name yet is added as a submodule with nothing run inside it. So a submodule shows
    its commit, never its work tree's changes.
15. **A work tree file with more than one name is withheld,** judged on the file opened, as
    the agent channel refuses one (ADR 0085): another of its names may hold secrets.
16. **A conflicted file is listed once,** as conflicted, and neither side is read: git lists
    it again for a stage it compares, and that record is dropped.

What was built:

- **One thread per review,** `oso-review`, takes `Open`, `Refresh`, `Baseline` and `Carry`
  requests in order and hands reports to the event loop: the listing with its generation,
  each changed file's reading, then `Read`. Refreshes waiting one after another are answered
  as one; a newer request, or the reviewer's end, stops the running one between files, and
  the next reads the rest. A failure names its request. A request that panics reports a
  failure and starts afresh with the same scope.
- **Each run** is `git --no-pager -c core.quotePath=false` in the work tree's top; its output
  is read on threads of its own up to a limit per run, standard error up to 4 KiB, and a run
  past 10 s is killed and left, so a process git started that holds the output cannot hold
  the review. Revisions are checked, then resolved with
  `rev-parse --verify --quiet --end-of-options`; only ids go further. Each refresh resolves
  the scope again, as `HEAD` or a branch may have moved.
- **A reading** is a file's entry (paths, status, modes, ids) and its content: both sides and
  a `Shape`, or binary, too large, out of time, withheld (a name, or a name before a rename,
  that may hold secrets, or a work tree file with more than one name: never read),
  conflicted, a submodule, or unreadable. Its signature is the old
  blob and modes, and the new blob or, for a work tree file, its `FileStamp`, or a link's
  target. A refresh reads only files whose signature changed, the caller's shown files first,
  and lets go of files no longer listed. A file that could not be read, did not fit or ran out
  of time is read again next time.
- **Work tree files** are opened from the top a directory at a time, following no link, as
  the preview opens them; a link's side is its target's text. The top is the trusted root, so
  it is not judged again for the home directory as the preview's root is.
- **Tests** run git in temporary repositories through the process module: every scope, a root
  commit, an unborn branch, the baseline and a lost one, refreshing after `HEAD` moved, the
  shown files first, a stopped request, what a review holds, secrets (also by a rename's old
  name), binaries, attributes that would hide a change, large files and links, refused
  revisions, a root inside another repository, a root put in the trusted one's place, a
  revoked root, a root git cannot stop above, a merge commit, a shallow clone's oldest commit,
  a conflicted file, a file with a second name, planted hooks, monitors and an embedded
  repository's clean filter that must not run, carrying a comment across a moved old side,
  the thread's end, and that the person's index is left as it was. `cat-file`'s standard error
  goes nowhere, so a warning cannot end it.

### Step 3: the unified view, part one

Step 3 lands in three PRs, as the author chose: the review buffer, its threads, the menu, the
motions, the faces and the `review` scenario first; comments and drafts second; jumping to the
source and refinement third. The author approved the plan's public items and answered its
questions (a) on 2026-11-13, and approved two items added while building:
`command::ArgSpec::TextOnPrefix`, text read only with a prefix argument, and
`Editor::set_review_reading`. What the first PR built:

- **A view thread per review,** `oso-review-view`, beside step 2's `oso-review`. It keeps each
  file's latest reading and lays out each file's section, again only for a file read again,
  then builds the whole text through `oso_core::load::Builder` and compares it with the text
  before. The UI thread puts it in at O(1), carrying every cursor and window top through the
  span that changed, with no undo, as a file read again is put in; the text it replaces is
  freed off the UI thread, and a text the same as the one before is not sent at all. Texts go
  in in order, and wait while a prompt is open, as a reload does. The text is at most 64 MiB:
  files past it show their header and a note.
- **Carrying a file with no comment to place** runs on the view thread, as it is hashing
  alone. When applying a carry asks for another, it runs again only for the file's current
  reading; a newer reading has its own on the way.
- **One review per project and scope,** the session scope whatever its baseline, and a branch,
  commit or range by the revision given. Opening one that is open shows it in the selected
  window and reads it again. A baseline is taken on a thread of its own and kept for the
  session; keeping it across restarts is step 6's.
- **The menu** is the built-in `review` menu on `C-x v r` (ADR 0117), with a describe function:
  heads `D` session, `w` uncommitted, `u` unstaged, `s` staged, `b` branch, `c` commit, `r`
  range, and `B` to take a baseline, which keeps the menu open. **The heads `p` (a pull
  request) and `S` (the session comments go to) are left out, not hidden, until step 12 and
  ADR 0121's registry,** as the author agreed. `C-u b` names the branch's base; without it the
  base found is used: the branch's upstream, else `origin/HEAD`'s branch, else `main` or
  `master`. `RET` takes the last scope reviewed, else uncommitted.
- **The menu's counts** come from at most two `oso-review-count` threads, listing as a review
  lists and reading no side, only for a trusted project, the session scope only with a
  baseline. Each opening is an epoch: a new one, or the menu closing, drops the counts waiting
  and stops those running between git runs. Each note keeps one width whatever its count
  says, so the float never moves. A project with only `.jj` or `.hg` shows `?` counts, and a
  review of it fails with git's message.
- **The line under the menu's title** names the project and its root, and the baseline's age
  as **"N min ago",** with no local time of day, as the author chose: oso has no helper for
  the local time.
- **Commands,** each with its reach: `review-session`, `review-uncommitted`, `review-unstaged`,
  `review-staged`, `review-branch`, `review-commit`, `review-range`, `review-mark-baseline`,
  `review-refresh` and `review-kill` are `Person`, as they run git; `review-next-hunk`,
  `review-previous-hunk`, `review-next-file`, `review-previous-file` and `review-keys` are
  `Look`; `review-quit` is `Change`. The motions take a count and move every cursor, by the
  buffer's rows, never by its text.
- **Keys in a review buffer:** `n`/`p` hunk, `N`/`P` file, `g` refresh, `q` bury, and `?`
  shows `*Review keys*`. The comment keys and `RET` come with the next two PRs.
- **Closing:** `C-x k` in a review buffer, and leaving oso, ask first while comments of the
  person's that were not sent would be lost.
- **The modeline** says how many agent comments are unread and, while a request reads,
  `reading n/m`.
- **A project no longer trusted** has its review's root revoked: its thread runs no more git,
  and the review says `M-x trust-project, then g`. `g` once it is trusted again starts the
  review over from a new root.
- **Display:** a path keeps to its row; a newline in it shows as `␤`, and a control character
  or a bidirectional mark is boxed and named in the modeline, as in any buffer. The title and
  each note are one line, a newline or carriage return in them a space.
- **Faces:** nine join the themes with their contrast rules: `diff.added`, `diff.removed`,
  `diff.refine-added`, `diff.refine-removed`, `diff.hunk-header`, `diff.file-header`,
  `review.comment-person`, `review.comment-agent` and `review.outdated`; `review.sent` and
  `review.decided` come with steps 12 and 5. Added and removed lines, headers and cards are
  bands drawn behind the text of the rows in view, as the current line's band is, and syntax
  fills; refined words are bands too, but not syntax fills, so text over them keeps the text
  colour. How syntax faces read over the bands is checked in step 9.
- **The `review` scenario** (`examples/perf/bench/review.rs`) reviews a repository of 57 files
  with some 5,000 lines changed: `review_open_first_screen_ms`, `review_open_whole_ms`,
  `review_key_p99_ms` (paced `n`, `p`, `N`, `P`) and `review_refresh_ms` (`g` after one file
  changed). It is recorded with no threshold yet; dev builds report each frame that shows a
  review's new text for it.
- **Tests:** the sections' text and rows, the view thread, the menu's notes, the motions and
  the commands; text-grid snapshots of a review buffer (its bands marked, every note, paths
  that would pass for another row); the glue's choices; and an end-to-end flow that trusts a
  project, opens a review over the window's keys, moves, refreshes and quits.

### Step 8: documents no buffer owns

The author approved the plan's API and its nine answers, each the recommended one (2026-11-13,
through the coordinator).

- **The API** is three methods of `oso_syntax::Scheduler<Key>` and one enum (and `Doomed`,
  below). The key stays generic, and the tip crate keeps buffers and documents apart with a key
  of its own (step 9):
  - `open(key, language, verdict, text, shown) -> Result<(), Refused>` opens a text that never
    changes, to parse and highlight whole, `shown` first. It is O(1), and nothing is parsed or
    freed in it. A key that has a slot is replaced, as a job in a new language replaces one. A
    refused open also closes a document open under the key, so a refresh refused as secret
    leaves nothing of the old text running.
  - `show(key, shown)` says what windows show of an open document. `close(key)` drops a
    document at once, as `drop_buffer` drops a buffer: what waited never runs, and a slice
    running on it gives back nothing. Both leave a buffer's key alone, and a job sent for a
    document's key replaces it with a buffer.
  - `Refused` is `MayHoldSecrets`, `TooLarge` (past ADR 0111's 2 MiB) or `Full`. `Full` is the
    one to try again once another document has closed.
  - Reports are `Report<Key>`, with epoch 0 and revision 0, since the text never changes. So a
    report of an earlier text under the same key looks the same as one of the new text, and the
    caller opens each text under a key it never used before (step 9).
- **Secrets:** `open` takes the source's `Verdict` and refuses anything but `Ordinary`, the
  same test `Buffer::is_secret` makes. So a text not judged yet, or whose path could not be
  resolved, is refused too, as a buffer not judged yet is. No tree, timing or highlight exists
  for such a text. The caller judges the source: every path a review side names (the old and the
  new path of a rename), or `Buffer::verdict` for a buffer's snapshot. Once a source turns
  secret, the caller closes the document. A slice already running on it ends, as a dropped
  buffer's does, and gives back nothing. Judging a path from outside `oso-core` needs a public
  form of `secret::judge`, left to step 3 or 9 (`open-questions.md`).
- **One lane for documents:** a document, shown or not, runs only in the lane that background
  buffers use, which one thread at a time has. So a shown buffer always has a thread, even
  while a slice on crafted text runs long. In that lane, shown documents go first, then
  background buffers, then documents not shown. Among documents, one that holds a tree goes
  before one that has none, and a paused document keeps its place in the queue rather than
  going to the back, so that a started tree finishes and is freed.
- **Bounds:** at most 1024 documents are open. Each slot holds a rope handle and the shown
  ranges, and every checkout scans the slots under the lock the UI thread takes: with 1023
  documents and 16 buffers open, a checkout and its give-back cost about 5 µs together, and an
  open and a close about 0.8 µs (release, the author's Mac). At most 4 documents hold a tree at
  once, and a document that would start a fifth waits for one to finish or close.
- **A shown document waiting for room** (the author's choice after the review, Q10 (a)): while
  a shown document cannot start because 4 hold a tree, hidden documents that hold one go before
  background buffers, so that one finishes and makes room. Under the plain order, the shown
  side would wait behind every background buffer, for as long as they had work. Not taken:
  letting up to 2 shown documents pass the cap, and keeping the plain order as a known limit.
- **A checkout whose slot went** (closed, replaced or dropped while a thread has it) still holds
  the lane, and its tree still counts, until it is given back, a slice that panicked too.
  Otherwise the next document could take the second thread while that slice runs, and a fifth
  tree could start.
- **A checkout keeps the lane it was taken in** (the coordinator's review): a background buffer
  that a window shows during its slice still holds the lane until it is given back, as a shown
  buffer whose window went already did (ADR 0111, step 4a). Before, it let a document take the
  second thread meanwhile.
- **What is freed goes to a thread** (the author's choice after the coordinator's review,
  Q11 (a)): `Scheduler::take_doomed` returns `Doomed` (new, public: an opaque value with
  `is_empty`, which frees what it holds when dropped), not `Vec<Document>`. It holds the
  documents, and the text that waited in a slot that went: closed, replaced, refused or failed
  before it started. The schedule may have held such a text alone, and freeing 2 MiB of it
  takes about 55 µs (release, the author's Mac), so it is not freed under the lock. The pool's
  `open` keeps an O(1) copy of the text, so a refused text is freed after the lock too. Not
  taken: a second method for the texts beside `take_doomed`, and freeing them under the lock
  as a known limit.
- **A finished document gives its tree to the list to free** when it is done, and a thread
  frees it with nothing locked. Its slot stays until `close`, holding no document, text or
  shown ranges. The caller keeps the highlights from the last report.
- **The tip crate's pool is generic over its key** (`Pool<Key>`, `Orders<Key>`), with
  `open`, `show` and `close`, which lock only for the schedule's bookkeeping. The app keeps
  `Pool<BufferId>`, so nothing opens a document until step 9. Until then, the three methods
  are expected to be dead code outside tests. The code is in `oso-syntax`'s private module
  `worker/unowned.rs`.
- **What step 9 takes on:**
  - a key never used before for each text;
  - closing a document whose source turns secret;
  - not opening again a text whose document failed or was lost;
  - calling `show` only when what windows show changed, since each call wakes the threads;
  - opening many sides in one turn under one lock, or spread over turns;
  - measuring the 500 ms "whole" budget with documents on one lane.
- **Tests:**
  - Refusals: every verdict that is withheld, the size limit and the open cap.
  - A refused open closes what was open under the key.
  - The count of open documents through every way a slot goes.
  - A checkout whose document closed keeps the lane and its tree until it is given back, or
    until its panic is reported.
  - A background buffer shown during its slice keeps the lane.
  - The text of a document that never started goes to `Doomed` when it is closed, replaced,
    refused or replaced by a job.
  - Highlights equal a fresh document's.
  - A finished document frees its tree and keeps an empty slot.
  - Closing before, during and after a slice.
  - The lane, its order (a document that holds a tree before one that waited longer) and the
    tree cap, and a shown document waiting for room letting a started one finish first.
  - Nothing is made or freed where documents open, show or close.
  - In the tip crate, two real threads: a crafted document that fails as too slow, beside a
    shown buffer that is highlighted first, and a second document that waits for it.

### Step 10: a pair of windows that scroll together

A pair is two tiled windows whose buffer lines line up by a row map their owner gives, closed and
resized together, in ADR 0112's tree. It is generic: nothing in it knows of a review, whose
two-pane view and filler rows come in step 11. The author approved the public items as listed
in the plan, and answered its questions (a), on 2026-11-13:

1. **The row map is a type,** `window::RowMap`: aligned runs of lines, `(first's line, second's
   line, lines)`, in order and apart on both sides (`RowMap::new`, refused with `BadRowMap`), or
   every line the same line (`RowMap::same`), which a review's filler rows give. `second_of` and
   `first_of` find a line on the other side by halving the runs. A line only one side has goes
   line by line into the other side's gap, its last line taking the rest, or to the line after
   the run before it when the other side has none there.
2. **It maps buffer lines,** display rows before any wrap, so each window wraps at its own width
   and the map stays true.
3. **The other window's cursor follows.** After every command (`Editor::follow_cursor`), when
   the selected window is in a pair and moved since its windows were last in step, the other
   window's top goes to the start of the line aligned with the selected one's top, its left
   column with the selected one's, and its primary cursor, alone, to the line aligned with the
   selected one's cursor, at the same display column clamped to that line, with the selected
   one's goal kept. Then it scrolls as little as shows its cursor, which a window a row shorter
   or wrapped narrower may need. When the other window's top is on the aligned line already, it
   stays, so a long wrapped line keeps the rows it shows. Not while a prompt is open, as ADR
   0112's windows follow: a pair made, turned or given a map beneath a prompt moves in step
   after the first command once it closes.
   The wheel (`scroll_view`) moves the other window's view with the one it turns, and its
   cursors into that view as the wheel moves any window's, but those of the window beneath a
   prompt and of one with a preedit; sideways (`scroll_view_sideways`), its left column alone.
   **Nothing moves in step while either window shows a buffer that may hold secrets,** checked
   at each step as a verdict can change after pairing, so no position in one is carried into a
   window the dev socket and the agent channel describe (ADR 0044, ADR 0079).
4. **A pair resizes as one,** its windows kept the same, as one child of the run around it,
   which `balance-windows` balances with the rest.
5. **`C-x 2` and `C-x 3` in either window split the pair's area:** the new window goes below or
   at the right of the whole pair, outside it.
6. **Showing another buffer in either window breaks the pair,** whatever shows it (`C-x b`,
   killing its buffer, a restart's `show_in`): both windows stay, as windows of their own, merged
   into the run around them when it runs their way.
7. **The handoff stays at version 4.** A pair goes over as two windows of their own in the same
   cells, and its owner pairs as it opens once more: it shows one side, then calls
   `pair_window`. What becomes of the windows left over when a review is restored is step 6's
   to decide; pairing two windows that exist already would need a new public item, such as
   `pair_windows`, for the author's approval then (the coordinator's call, 2026-11-13).
8. **A frame too small for every window hides both windows of a pair together,** never the
   selected window's pair. This holds but where the area could keep no window otherwise: in an
   area smaller than both windows need (a frame of one row, say), one window of the pair shows
   alone, as a run's first does, since an area always keeps a window: the selected window
   when the pair holds it, and otherwise the first.

What was built:

- **`Editor::pair_window(way, buffer, map)`** splits the selected tiled window into a pair, the
  new window at the right or below showing `buffer`, and refuses with `NoPair`: `TooSmall`
  where a split would be, `Paired` for a window in a pair already, `NoBuffer`. The selected
  window stays selected and leads at once. `pair_of` names the other window, `set_row_map`
  gives a new map and moves the windows in step from the selected one, or the first,
  `turn_pair` lays a pair side by side or stacked (`TooSmall` where either window would be
  smaller than a split leaves), and `unpair(keep)` closes the other window, `keep` taking the
  pair's cells, and its selection if the other had it.
- **`C-x 0` in either window deletes both,** and selects the window selected most recently
  before them; a pair that is the whole frame stays, as the only window does. `C-x 1` keeps
  both. `C-x o` and `windmove` go through them as through any window.
- **The tree** (`frame::Splits`) marks a pair's run, crate-private. A pair is never merged into
  a run of its way, so a pair side by side sits in a run side by side as one child. Nothing in
  `frame` is public for it. A pair is a level of the tree of its own, so a layout that holds
  pairs reaches the handoff's 16 levels (`Tiling::MAX_DEPTH`), past which a restart waits, a
  level sooner than the same windows unpaired would.
- **Beneath a prompt, a pair holding the window beneath stays:** `unpair` returns false and
  `delete_window` is refused, as for the sole window, so that what the prompt acts on keeps its
  window and buffer. The window commands refuse inside a prompt anyway (ADR 0112); this keeps
  the API from moving a prompt's window for any other caller.
- **No change to the app:** the host calls `follow_cursor` and the wheel's methods as before,
  and draws each window as before, so no end-to-end flow changed in this step.
- **Cost:** a command's end looks the pair up among the pairs, and a window that did not move
  since leads nothing. Moving in step is two lookups in the map, a seek for each line and one
  view set, and the other window's rows are shaped again only when its view moved. A map is
  built where its owner builds it, off the UI thread for a large one, as `RowMap::new` checks
  every run; giving one to the editor moves it. In a release build, over two texts of 1 GB and
  a map of a million runs, the end of a command costs microseconds more with a pair, and so
  does a character on a 10 MB line both windows wrap.
- **Tests:** the tree's property test also pairs, turns, breaks and unpairs, and checks that a
  pair shows together and shares its cells; the map's runs map exactly and back, and keep lines
  in order; on the editor, moving, scrolling a screen, isearch, the wheel up, down and
  sideways, a prompt and a preedit, a short line and the end of the text, a wrapped window,
  closing, splitting, breaking, unpairing, turning, the handoff, and a buffer that may hold
  secrets in either window, from the start or later; and a property test that, whatever is
  done to the windows, the editor's pairs are the tree's and show what they were paired with.
  Two ignored tests time moving in step in a release build: over two texts of 1 GB and a map
  of a million runs, and on a 10 MB line both windows wrap.

