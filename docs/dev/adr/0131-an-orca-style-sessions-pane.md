# 0131. An Orca-style sessions pane

- Status: Accepted
- Date: 2026-11-14

## Context

The author uses oso daily now, with the coordinator session running inside it. The `*Sessions*`
side window of [ADR 0121](0121-agent-sessions-and-spaces.md) (step 3, 32 columns at the frame's
left) is hard to read. The coordinator looked at it on 2026-11-14 with one hosted session and 29
sessions of Claude Code started elsewhere, and found, worst first:

1. **It lists every `claude` on the machine.** Step 3 lists each live entry of Claude Code's
   registry. "Idle 29" was almost all sessions of another editor and of other projects; one row was
   this project's. Nothing separates this project from the rest.
2. **Each session takes three lines** (name and age; place; recap) below 60 columns, so about 27
   fit in a tall window.
3. **"external"** (started outside oso) is unclear, and is on almost every row.
4. **The space shows on some external rows and not others** (`external · oso`): only a directory
   that a space of this oso holds gets a name.
5. **Every age read `6m`.** An external session's age is the time since oso first saw it, as the
   registry's own `statusUpdatedAt` is not read.
6. **A working session's recap is the TUI's spinner word** ("Jitterbugging…"), which means
   nothing.
7. **Names are cut at the end,** so `chore-survey-review-lifecy…` and `chore-survey-review-split-…`
   lose what tells them apart.
8. **No overview of projects.** The list groups by state, or by space with `s`; nothing shows what
   each project and worktree is doing.
9. **Marks are unexplained.** `▶` and `○` have no legend. The "box" on the `Idle` heading is the
   side window's hollow cursor drawn over `▾` (point was on that line), which reads as a second
   kind of fold glyph.
10. **Sessions oso cannot act on** (running in another program) are mixed with those it can.

The author chose (2026-11-14) to base the pane on **Orca**'s left sidebar, rather than herdr's two
panels or Claude Desktop's single list with sections. Small parts of those may be borrowed where
they fix the issues above (herdr's rolled-up state and Desktop's nested sessions and read or unread
marks are already in ADR 0121).

What holds already:

- ADR 0121: one state taxonomy everywhere (needs input, failed, working, ready for review, done,
  idle, stopped), from the screen, on the `oso-sessions` worker; spaces (a project and its
  sessions, a worktree's space nested under its repository's); the side window outside the window
  tree, skipped by `C-x o`; `*Sessions*` withheld from agents; the registry poll once a second.
- [ADR 0126](0126-sessions-and-worktrees-in-depth.md) (accepted, not built yet): the board (`B`),
  grouping by worktree (`s` cycles state, space, worktree), `session-grid`, `jump-anywhere` on `C-x
  a j`, `worktree-create` on `C-x a w`, `worktree-cleanup` on `C-x a k`.
- [ADR 0127](0127-source-control-and-github.md) (accepted, not built): a worktree's pull request
  from one `gh pr list` per repository, in trusted projects.
- `git` and `gh` run only through `oso-platform::process`, on a worker, in a trusted project
  ([ADR 0011](0011-git-integration.md), [ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)).
  oso has no network code. Nothing about sessions runs on the UI thread, and nothing runs while
  nothing changes.

### What Orca does

From its source at commit `c1b9074c` (2026-11-14) and its documentation, read, not run (ADR
0126 has more on Orca's board, jump and worktrees):

- **Rows.** A project header (icon, bold name, a menu and `+`; no count, no roll-up). Under it, a
  worktree card of two lines: a state mark, the title (bold while unread) and a `primary` badge on
  the main checkout; then the branch, with small icons for a linked issue, notes, ports and the pull
  request, coloured by its state or its checks (the number only in the tooltip). No diff counts and
  no age on the card. Under a card, an agent row of one line: state mark, agent icon, the
  conversation's name, a preview of the last tool or reply, the model, and the age. Two agents or
  more fold into one summary row, "3 agents · 2 working, 1 done".
- **States.** Working is a spinner, needs input an amber `?`, done a green check (a blue dot while
  unseen), failed a red dot, idle a faint grey dot. A worktree's mark is its agents' worst state:
  needs input, failed, working, …, done, idle. States come from hooks Orca installs in Claude Code's
  settings, which post to a local port, and from the terminal title's spinner.
- **Ages:** `now` under a minute, then `Nm`, `Nh`, `Nd`; from the finish for a finished agent, else
  from the start; refreshed every 30 s.
- **Order.** Projects in an order the person drags. Worktrees by "Recent" by default (the last
  terminal start or exit, an agent finishing or asking, a bell; a new worktree is lifted for 5
  minutes), the primary first; other sorts are by attention, name, project or by hand. A re-sort
  settles over 3 s and waits while the pointer is over the list. Pinned worktrees go in a section at
  the top.
- **Folding and many worktrees.** Nothing is folded by default; folds are kept. The list is
  virtualised, with no limit found. Group by project, state, pull request or none; filters hide
  sleeping worktrees and others. Worktrees made outside Orca are found with `git worktree list` but
  hidden until the person shows them. A worktree with no live terminal is "sleeping": a grey dot,
  its title and branch. A plain shell gets no row of its own.
- **Selecting** a card shows that worktree's own layout in the main area; the card gets a tinted
  background and border. Clicking an agent row focuses its pane. Keys: `Cmd-K` the palette (search
  over names, branches, repositories, pull requests, issues), `Cmd-Shift-Up`/`Down` the previous or
  next worktree, `Cmd-1`…`9` by position, `Cmd-B` hides the sidebar.
- **Width:** 280 px by default, 220 to 500, resizable; about 36 cells of its 13 px text.
- **Git and pull requests** come from the `git` and `gh` command-line tools, polled every 1 to 15
  minutes by the PR's state. The sidebar shows only the branch and the PR's icon; staging, commits
  and diff counts are in a source-control panel on the right.
- **Sessions started outside Orca** are not in the sidebar: the hooks drop events from terminals
  Orca did not start. A separate history panel lists past sessions from their transcripts and offers
  to resume them.
- **Remote machines** get a header each (out of scope: oso has no network code).

## Decision

`*Sessions*` becomes a tree of projects, worktrees and sessions, as Orca's sidebar, in the same side
window, buffer and mode. What ADR 0121 and 0126 decided about the side window (outside the window
tree, skipped by `C-x o`, shown while oso has a session, withheld from agents), the states, the
board and the jump stays.

### What the pane shows (Q1, Q3)

- **A project** is a repository that has a space in this oso: one of its sessions runs or is
  restored, or the person entered it with `space-switch` or `jump-anywhere`. Its row is the
  repository's name.
- **Under a project, its worktrees (Q3):** the main checkout first, marked `primary`, then the
  linked worktrees. In a trusted project they come from `git worktree list --porcelain` (ADR 0121
  step 4's reader), so a worktree made outside oso shows too. In a restricted project, only the
  directories where its sessions run show, with no branch.
- **Under a worktree, its sessions:** hosted ones (running, or stopped after a restore), then, dim,
  the external sessions whose directory is in that worktree. A session started by another nests
  under it, as in ADR 0121.
- **Other projects (Q1):** every external session whose directory is in no project of the pane goes
  in one group at the bottom, `Other projects`, folded, with a count. Opened, it lists a row per
  directory (the nearest repository root above the session's directory, else the directory),
  folded, and their sessions under them.
- **Archived** sessions stay in a last group, folded, as now.
- **The pane's modeline** names the grouping and the help key, `Sessions · tree · ? keys`, instead
  of the buffer's name and position.

### Rows at 44 columns (Q2, Q7, Q10)

Each row is one line (Q2). The width is 44 columns (Q7). A mock, with `·` for a dim field:

```text
▾ oso                                   ⚠ 1
  ● main  ·primary                       3m
    ● oso-bb                             3m
    ○ ·oso-2                             2h
  ⚠ m4-sidebar-design                   now
    ⚠ m4-sidebar-design-c3              now
      ·permission: Bash(git push origin…
  ○ fix-prompt-flake  ·fix/prompt-test   1d
▾ notes                                 ■ 2
  ■ main  ·primary  (2)                  2d
▸ Other projects                  ▶ 1  29
▸ Archived                                4
```

- **Project row:** the fold glyph (`▾` open, `▸` folded), the name, and at the right the most
  urgent state of its sessions with how many are in it. A folded project shows the same.
- **Worktree row:** two cells of indent, the most urgent state's mark of its sessions (herdr's
  rolled-up state; nothing for a worktree with none), the worktree's directory name, its branch dim
  when it differs from that name (the main checkout: its branch and a dim `primary`), and at the
  right the newest change of state among its sessions. A folded worktree adds its session count,
  `(3)`.
- **Session row:** four cells of indent (six when nested), its mark, its name, and at the right its
  age. External sessions are dim, with the same marks, so that what oso can show and what it can
  only mention look apart (issue 10). No `external` word.
- **A second line, only for a session that needs input or failed (Q2):** its reason, dim (the
  dialog's question, or the exit status). No other session gets one.
- **Long names are cut in the middle (Q10),** keeping the last third: `chore-survey-r…lifecycle-hook`.
  Branches too. Every untrusted field (names, branches, directories, reasons) keeps ADR 0121's rule:
  cleaned, cut in cells as drawn, never beginning with a mark.

### Marks and colours, one taxonomy (Q5)

- **The glyphs stay ADR 0121's:** `⚠` needs input, `✗` failed, `▶` working, `◆` ready for review,
  `●` done, `○` idle, `■` stopped. Folding uses only `▾` and `▸`, on project and group rows.
- **Each state gets a face,** shared by the pane, the board, the modeline marks (`⚠ N`, `● N`), a
  session's own modeline and `session-switch`'s annotations: needs input and failed in the error
  colour, working in the warning colour, ready for review and done in the success colour, idle and
  stopped dim. Themes define them ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)).
- **A working mark spins (Q5),** as Orca's: see [The spinner](#the-spinner-q5-q11q14). Every
  other mark is still.
- **The row at point is highlighted** across the pane's width, and the side window draws no
  cursor of its own, so a box over a glyph never reads as a mark (issue 9).
- **The space in front** (the worktree the frame shows, and its sessions) has a faint background,
  as Orca's selected card: the pane says where the person is.
- **`?` in the pane** shows the legend of marks and the keys in the echo area, or a help float when
  it does not fit.

### The spinner (Q5, Q11–Q14)

The author chose a spinning working mark (Q5 (b)). It is the one timer the pane runs, and it runs
only while it has something to turn.

- **The glyphs (Q12):** the ten Braille frames `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` in the working colour, in the place of
  `▶`. Braille characters are one cell wide as oso counts cells, so the row never moves.
  `n`/`p` and the "never begins with a mark" rule count these frames as marks, beside
  `ROW_MARKS`.
- **The rate (Q11):** one frame every 125 ms, 8 a second.
- **What spins (Q13):** a hosted working session's row, and a worktree row whose rolled-up mark is
  working. External sessions' rows stay a still, dim `▶`: oso cannot show them, and motion would
  pull the eye to rows the person cannot act on. A project row, the board's cards (in a tree
  window), `session-switch` and the modeline marks stay still.
- **When it runs:** only while the side window shows `*Sessions*`, at least one spinning row is in
  view (not folded, not scrolled away), and oso's window is not minimised. Otherwise no timer is
  set. All rows share one phase, from one deadline on the event loop's wait, as ADR 0121's blink
  of the modeline's marks does (`sessions/attention.rs`); no thread of its own.
- **What a tick does:** it replaces the one cell of each spinning row in view with the next frame,
  a one-character edit each, so that only those rows are shaped again (as rows whose marks
  changed are, [display model](../display-model.md)), and asks for one frame. The list's model,
  its order and its other text are not touched, and nothing goes to the worker.
- **Switched off (Q14):** `[sessions] spinner = false` in `config.toml` gives a still `▶`, and so
  no timer. The system's "reduce motion" setting is not read: that needs a new function in
  `oso-platform` for one spinner.

### Ordering and folding (Q4)

- **Projects:** in the order their spaces were added, as `space-switch` lists them.
- **Worktrees:** the main checkout first, then the linked ones, newest made first (Q4). Rows do not
  move when a state changes: urgency shows through marks, colours, the project row's roll-up, the
  modeline's `⚠ N` and `C-x a n`.
- **Sessions in a worktree:** most urgent first, in the taxonomy's order, then by start; pinned
  first, as ADR 0121.
- **Folding by default:** projects open; a worktree open while it has a running session and folded
  otherwise; worktrees with no session and no change for 7 days in a folded `N quiet` row at the end
  of their project (Q3); `Other projects` and `Archived` folded. What the person folds or opens
  stays so for this oso instance.

### Ages (Q6)

- **An age is the time since the session last changed state** (Q6): a hosted session's as now;
  an external session's from the registry's `statusUpdatedAt`, cut to now when it lies in the
  future, and none when it is missing. A worktree's is its newest session's; a project shows none.
- **Format:** `now` under a minute, then `4m`, `3h`, `2d`, as Orca. So a row's text changes at most
  once a minute, and ages move only with a frame drawn anyway, as now.

### Recaps (Q8)

- **No spinner words, anywhere (Q8).** A working session's recap is the terminal's title (the
  topic the TUI gives the conversation), without its spinner, not the status line. The bundled
  rules drop their working `recap` patterns; a local rule may still set one.
- **Where recaps show:** the pane's second line for needs input and failed only; the board's cards
  and `session-switch` for every session, as ADR 0126 and 0121 have them.

### Git and pull request state (Q9)

- **The pane shows a worktree's branch only** (Q9), as Orca's sidebar does. Ahead and behind,
  dirty counts, the pull request and its checks stay in the board's cards, `jump-anywhere`, and
  `worktree-cleanup` (ADR 0126 and 0127).
- **The worktree list** is one `git worktree list --porcelain` per trusted project, on ADR 0121's
  git pool (at most 2 jobs, 10 s each), with `GIT_TERMINAL_PROMPT=0`: when the project enters the
  pane, after oso makes or removes a worktree, on `g`, and when the sessions worker's existing 1 s
  poll sees the change time of the repository's `worktrees` directory in its git directory change
  (one `stat` a project). No `gh`, no fetch.

### Keys and mouse

The pane's keys, as ADR 0121's, by the row at point:

| Key | Project row | Worktree row | Session row |
|---|---|---|---|
| `RET` | fold | enter its space, or find a file in it when it has none | show the session |
| `TAB` | fold | fold | fold its nested sessions |
| `n` / `p` | next / previous row | | |
| `M-n` / `M-p` | next / previous worktree or project row | | |
| `c` | `claude` in the main checkout | `claude` in this worktree | `claude` in its worktree |
| `w` | `worktree-create` for this repository | the same, from this worktree's branch | the same, parent this session |
| `k` | — | says that `C-x a k` removes worktrees | stop (ADR 0121) |
| `r`, `P`, `A`, `e` | — | — | resume, pin, archive, explain |
| `s` | tree, or grouped by state (ADR 0121's list, one line a session) | | |
| `B` | the board (ADR 0126) | | |
| `/` | `jump-anywhere` (ADR 0126), Orca's search | | |
| `g` | refresh: ages and the worktree lists | | |
| `<` / `>` | the pane 4 columns narrower or wider, for this oso | | |
| `?` | the legend and keys | | |

- **Mouse:** a click moves point to the row and does what `RET` does there; a click on a fold glyph
  folds; the wheel scrolls.
- ADR 0126's third grouping (by worktree) is the tree; `s` toggles tree and state.

### Width (Q7)

- **44 columns by default** (Q7). Orca's sidebar is about 36 cells of its text, but its rows are
  two lines and its branch has a line of its own; oso puts mark, name, branch and age on one line.
  `[sessions] pane_width` in
  `config.toml` sets it, 24 to 80. `<` and `>` change it for this oso instance. Under 36 columns a
  session's age goes before its name is cut below 12 cells.

### How it updates

- **The model is built on the sessions worker,** not the UI thread: the worker already holds the
  external sessions and the states; it gains the worktree lists from the git pool and maps each
  external session's directory to a project and worktree (paths compared component by component,
  after `canonicalize` on the worker). The UI thread receives a changed model only, as `Out::`
  messages do now, and renders it as `sessions/list.rs` does: plain data in, text and line map out,
  with the faces as highlight spans (as review buffers carry `Highlight::DiffAdded`).
- **Rendered only while shown,** to the window's width, only when the model, the folds, the width
  or a minute's age text changed; the text is set only when it differs, as now.
- **Modules:** the tree's model and renderer in `sessions/pane.rs` and `sessions/pane/rows.rs`
  beside `list.rs` (whose by-state renderer stays for `s`), the directory mapping in
  `sessions/worker/places.rs`. No new crate, no new crate edge, no new public module or trait in a
  library crate: `oso-core` gains `Highlight` variants for the state faces and a mode flag to draw
  the point's row highlighted instead of a cursor. Each file stays under 600 lines.

### Agents and secrets

- `*Sessions*` stays an internal buffer withheld from agents; its new text (branches, directories,
  titles) is withheld with it. No session tool gains fields.
- New commands and their reach (ADR 0085): `sessions-new-here` (`c`) is `Person`, as it spawns
  `claude`; `sessions-next-group`/`-previous-group` (`M-n`/`M-p`), `sessions-help` (`?`) and
  `sessions-width` (`<`, `>`) are `Look`, as the list's moving and folding are; `RET` on a worktree
  row is `Person`, as `sessions-show` is (it changes what is in front of the person's keys).
- A secret buffer's text never reaches the pane: rows hold names, branches, directory names, states,
  ages and the reason a dialog shows, as now.
- External sessions' directories come from the registry, which any process of the person's can
  write: a wrong directory only misplaces a dim row, which authorises nothing, as a state never
  does.

### Cost

| What | Budget |
|---|---|
| UI thread while no session changes state and no row spins | nothing: no timer, no redraw |
| One spinner tick, 20 spinning rows in view | ≤ 0.2 ms on the UI thread before the frame; the frame shapes only those rows |
| Spinner, sustained, 8 ticks a second, 20 rows | ≤ 2% of one core for the UI thread and the frame together on the reference Mac; a frame's p99 stays within the key-to-photon budget, so typing in a tree window is not slowed |
| UI thread per model change | ≤ 0.5 ms with 20 worktrees, 40 sessions and 300 external sessions folded |
| Worker per registry poll | ADR 0121's, plus one `stat` per project for its worktree list |
| Worktree list | one `git worktree list` per trusted project per change, ≤ 10 s or abandoned |
| Directory mapping | one `canonicalize` per external session when its directory changes |
| Memory | ≤ 256 bytes a worktree beyond ADR 0121's per session |

The `just perf` flow ADR 0121 named (20 hosted sessions printing at once) gains 300 external
registry entries and 20 worktrees; typing in another window stays within its budget.

## Steps

One pull request each, after ADR 0121's steps it builds on. ADR 0126's step 1 (the board and
grouping by worktree) is built after step 2, on this model.

1. **Fixes inside the current list:** ages from `statusUpdatedAt` (`now`, minutes, hours, days);
   no spinner recaps (the bundled rules); names cut in the middle; one line a session at any width,
   with the reason line for needs input and failed; no `external` word, external rows dim; the
   state faces in the list and the modeline marks; the point's row highlighted; `?`; the spinner
   and `[sessions] spinner`, with a `just perf` flow of 20 spinning rows while typing in a tree
   window. These fix issues 2–7 and 9 on their own.
2. **The tree:** projects, worktrees from `git worktree list`, the directory mapping on the worker,
   `Other projects`, folding defaults, ordering, the space in front, the pane's modeline, the keys
   (`c`, `M-n`/`M-p`, `RET` on a worktree), the mouse. Fixes issues 1, 8 and 10.
3. **Width:** 44 columns, `[sessions] pane_width`, `<` and `>`; the `just perf` flow.

## Questions

For the author, the recommended option first. Each stands on its own.

1. **Sessions of other projects and outside oso.**
   (a) Projects of this oso as trees; an external session whose directory is in one of them is
   listed dim under its worktree; every other external session goes in one folded `Other projects
   (N)` group at the bottom, by project inside.
   (b) The same, but external sessions are never under a project: all in a bottom group `Outside
   oso (N)`, by project inside.
   (c) Not listed at all: only this oso's own sessions. `session-mention` and the tools still see
   them.
2. **Lines a row takes.**
   (a) One line for project, worktree and session rows; the worktree's branch on its line, dim,
   only when it differs from the directory's name; a second dim line only for a session that needs
   input or failed, with its reason.
   (b) Orca's two-line worktree rows (name, then branch, always); sessions on one line.
   (c) One line everywhere; the reason shows in the echo area when point is on the row.
3. **Which worktrees show.**
   (a) Every worktree of the repository (from `git worktree list`, trusted projects only); those with
   no session and no change for 7 days go in a folded `N quiet` row at the end of the project.
   (b) Every worktree, none folded away.
   (c) Only worktrees with a session.
4. **Order of worktrees.**
   (a) Stable: the main checkout first, then newest made first; rows never move on a state change,
   and urgency shows through marks, colours and the project's roll-up.
   (b) By recent activity (the newest change of state first).
   (c) Needs input first, then recent activity.
5. **Marks and colours.**
   (a) ADR 0121's glyphs kept; a face per state shared by the pane, board, modeline marks and
   `session-switch` (error colour for needs input and failed, warning for working, success for
   ready for review and done, dim for idle and stopped); external rows dim; no animation; `?`
   gives a legend.
   (b) As (a), with a spinning mark for working sessions, as Orca: a timer redraws the pane while any
   session works.
   (c) Glyphs only, as now, plus the `?` legend.
6. **What an age means.**
   (a) Time since the session's last change of state: hosted as now, external from the registry's
   `statusUpdatedAt`; `now` under a minute.
   (b) Time since the session started.
   (c) As (a) for hosted sessions; no age for external ones.
7. **Pane width.**
   (a) 44 columns by default, `[sessions] pane_width` (24–80), and `<`/`>` in the pane for this
   oso instance.
   (b) 40 columns, the setting only.
   (c) 32 as now, the setting only.
8. **Recaps.**
   (a) No spinner word anywhere: a working session's recap is the terminal's title (the
   conversation's topic); the pane shows a recap only for needs input and failed; the board and
   `session-switch` show every recap.
   (b) No recap at all for a working session.
   (c) Keep the status line as the working recap, as now.
9. **Git and pull requests in the pane.**
   (a) The branch only, as Orca's sidebar; ahead and behind, dirty counts, the pull request and
   checks stay in the board, the jump and `worktree-cleanup`.
   (b) Also a short pull request mark on the worktree row, `#123 ✓`, once ADR 0127 reads it.
   (c) All of ADR 0126's worktree row: branch, ahead and behind, dirty and untracked, pull request
   and checks.
10. **Long names.**
    (a) Cut in the middle, keeping the last third: `chore-survey-r…lifecycle-hook`.
    (b) Cut at the end, as now.
    (c) Under a worktree, drop the worktree's name from the front of a session's name (`…-c3`),
    cut at the end.

### The spinner (after Q5 (b))

11. **The spinner's rate.**
    (a) 8 frames a second (125 ms), close to Claude Code's own spinner; about 2% of a core at most.
    (b) 4 frames a second (250 ms): half the cost, visibly steppier.
    (c) 2 frames a second: a slow pulse more than a spin.
12. **The spinner's glyphs.**
    (a) The ten Braille frames `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`, one cell wide, in the working colour.
    (b) Quarter circles `◐◓◑◒`: four frames, heavier.
    (c) `▶` and `▷` in turn: a pulse that keeps today's glyph.
13. **What spins.**
    (a) Hosted working sessions' rows and worktree rows whose rolled-up mark is working; external
    sessions stay a still, dim `▶`; project rows, the board, `session-switch` and the modeline stay
    still.
    (b) As (a), and external working sessions spin too.
    (c) Session rows only; worktree rows show a still `▶`.
14. **Turning it off.**
    (a) `[sessions] spinner = false` in `config.toml`; the system's "reduce motion" setting is not
    read.
    (b) As (a), and on macOS the system's "reduce motion" setting stops it too (a new function in
    `oso-platform`).
    (c) No setting.

### Dependencies

None.

## The author's choices

The author answered every question on 2026-11-14, through the coordinator: (a) for each, but Q5.

- **Q5: (b).** As (a), and a working mark spins, as Orca's. The timer must stay cheap: one shared
  tick that changes only the marks' cells, stopped when nothing spins, within the render budget,
  with its rate and cost given here (see [The spinner](#the-spinner-q5-q11q14) and Cost). Q11–Q14
  are the questions this raised; the author answered them the same day, (a) for each: 8 frames a
  second (Q11); the ten Braille frames (Q12); hosted working sessions and rolled-up worktree rows
  spin, external rows stay a still, dim `▶` (Q13); `[sessions] spinner = false` only, the
  system's "reduce motion" not read (Q14).
- The rest, as recommended: other projects' and outside sessions dim under their worktree, the
  rest in a folded `Other projects (N)` (Q1); one line a row, a reason line for needs input and
  failed (Q2); every worktree, quiet ones folded away after 7 days (Q3); a stable order (Q4);
  ages since the last change of state, `now` under a minute (Q6); 44 columns, `[sessions]
  pane_width`, `<` and `>` (Q7); no spinner words, the title as a working recap (Q8); the branch
  only (Q9); names cut in the middle (Q10).

## Step 1, as built

Built on 2026-11-14 and 15 in two pull requests: 1a, the text (#260), and 1b, the faces, the
band and the spinner. The coordinator accepted each point below that differs from the decision.

- **One line a session, at any width** (`sessions/list.rs`): mark, name cut in the middle
  (`cut_middle`, the last third of the room kept), place where at least 6 cells are left, age
  at the right edge. The 60-column layout with a recap column is gone. A second line holds the
  reason for needs input and failed, and "N nested need input" for a parent whose nested
  sessions do. `age` gives `now` under a minute; `age_of`, which `session-adopt` uses for
  "12s ago", keeps seconds.
- **External ages** come from the registry's `statusUpdatedAt` (`registry::Entry::
  status_updated_at`, `External::since`), cut to now. **An entry without it** (an older CLI)
  is aged from oso's first look, as before, rather than showing no age: `Entry::since` is not
  optional, and the case is rare.
- **The word `external`** stayed in 1a, at the start of the place, until 1b's quiet face could
  tell such a row apart; 1b removed it. An external session's place is its space's name, else
  its directory's last name, cleaned as a name is.
- **No spinner words:** the bundled `status-line` rule lost its `recap`, so a working session's
  recap falls to its title (`agent-rules.md`).
- **`?` is `sessions-help` (`Look`)**: the legend and the keys in the echo area, bound in the
  list's own bindings.
- **Faces:** `oso-core`'s `Highlight` gained `SessionAlert`, `SessionWorking`, `SessionDone` and
  `SessionQuiet`, and `oso-render`'s `Face` the same four. Red is the terminal's colour 9, amber
  the warning's, green the terminal's colour 10. Quiet text uses a prompt's dim colour: a
  completion's annotation colour fell short of 4.5:1 on dusk's band (4.48). Each is checked at
  4.5:1 on the text's background and on the band, and the alert and done faces on both
  modelines, which now draw `⚠ N` and `● N` in them.
- **Highlights reach the list** through `Editor::set_internal_highlights`, a new public function
  (the coordinator's approval, 2026-11-15): host-made highlights on an internal buffer in
  `Mode::Sessions`, as a review's are, refused unless they cover the text.
- **The band:** `Editor::current_line_cells` also gives the side window's row at point while it
  shows `*Sessions*`, whatever the visual aids say. No new public item.
- **The spinner** (`sessions/spinner.rs`, pure; `app/sessions/watch/view.rs`): one deadline beside
  the attention's, 125 ms a frame, from the instant it started. A tick renders the list again
  with the next frame and sets its text whole, rather than editing one cell. `oso-render`'s
  shaping cache reuses every row whose text did not change, so only the spinning rows are
  shaped again, and no edit API was needed. It runs only while the side window shows the list,
  a spinning row is in view, and oso's window is not occluded (`Event::Occluded`), and stops
  otherwise. `[sessions] spinner = false` keeps every mark still.
- **The `just perf` flow** is `sessions_spinning_frame_p99_ms` and `sessions_spinning_ui_ms_per_s`
  in `bench/sessions.rs`: the 20 sessions of the printing flow, each showing a working screen,
  over 5 s. It reports, with no threshold, and measures frames rather than typing, which the
  other flows time. `bench.rs` did not change.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0121](0121-agent-sessions-and-spaces.md), "`*Sessions*` (Q6) is an internal buffer: sessions
  grouped by state, … Each row: state mark, name, space, branch (trusted projects only), age, and
  the recap": `*Sessions*` is a tree of projects, worktrees and sessions, one line a row; grouping
  by state stays behind `s`; the recap shows in the pane only for needs input and failed.
- ADR 0121, "`sessions` shows it in a side window 32 columns wide": 44 columns, set by `[sessions]
  pane_width`.
- ADR 0121, "Keys: `RET` show, `n`/`p`, `TAB` fold, `g` refresh, `s` group by state or by space,
  …": the keys above; `s` toggles tree and state.
- ADR 0121, "for working, the TUI's status line (its spinner's text)": the terminal's title.
- ADR 0121, "Sessions oso did not start … are read from the registry and listed under their
  directory's space as `external`": listed dim under their worktree when it is in a project of the
  pane, else under `Other projects`; no `external` word; their age from `statusUpdatedAt`.
- ADR 0121, "Marks beside states: `pinned`, `archived`, `external`, and an age, "waiting 12 min",
  since the state was entered": the age as above, `now`, `4m`, `3h`, `2d`.
- ADR 0121, "A session not shown that becomes `needs input` or `failed` blinks its mark three
  times, then stays; no timer runs for a state that does not change", and the cost line "UI
  thread while no session changes state: nothing: no timer, no redraw": also a timer of 8 ticks a
  second while the side window shows a spinning working row in view, unless `[sessions] spinner =
  false`.
- [ADR 0126](0126-sessions-and-worktrees-in-depth.md), "`*Sessions*` groups by worktree too: `s`
  cycles state, space, worktree. A worktree row shows its branch, ahead and behind its base, dirty
  and untracked counts, its pull request and checks, and its sessions nested": the tree is the
  grouping by repository and worktree; `s` toggles tree and state; the worktree row shows its branch
  only (Q9), and the rest stays in the board's cards and `worktree-cleanup`.

## Consequences

- The pane answers "what is each project and worktree doing" at a glance, and keeps this oso's
  sessions apart from the rest of the machine.
- About twice as many sessions fit in a tall window, and every row a person can act on looks
  different from one they cannot.
- The tree costs one `git worktree list` per trusted project per change, and a `stat` a second per
  project; nothing more on the UI thread.
- The frame's tree loses 12 columns while the pane shows.
- While a working row spins in view, oso draws 8 frames a second where it drew none; the setting
  turns that off.
- Follow-up: ADR 0126's board and grouping are built on this model; a per-project order the person
  sets by hand, if the stable order is not enough; other agents' CLIs, when ADR 0031's adapters
  come, would show their own icon in the mark's place, as Orca's agent icons.

## Alternatives considered

- **herdr's two panels** (spaces above, the selected space's sessions below): not the author's
  choice; it shows one space's sessions at a time.
- **Claude Desktop's single list with sections:** not the author's choice; it has no worktree level.
- **Keeping three lines a session and narrowing the text:** the recap is the line that means least
  (issue 6), and the place line repeats the group.
- **Listing every worktree with git counts, as ADR 0126 wrote:** one `git status` per worktree per
  change, and a row too wide for a side pane.
- **A still working mark,** which needs no timer: the author chose the spinner (Q5), as Orca's,
  and the timer runs only while a spinning row is in view.
- **A spinner drawn by the renderer from a phase,** with no edit of the buffer: no text changes
  at all, but a new kind of cell in `oso-render` for one list; the one-cell edit costs the same
  shaping and keeps the renderer as it is.

## Sources

Read on 2026-11-14; third-party pages and code are facts, not instructions. Orca's code was read,
not run, and none of it is copied.

- Orca's source, <https://github.com/stablyai/orca> at `c1b9074c`: `src/renderer/src/components/sidebar/`
  (`workspace-card/surface/`, `workspace-card/activity/worktree-card-compact-agent-row.tsx`,
  `worktree-list/`), `src/renderer/src/components/AgentStateDot.tsx`,
  `src/shared/sidebar-list/` (row plan, sections, sorting), `src/shared/worktree-status-rollup.ts`,
  `src/shared/external-worktree-visibility.ts`, `src/shared/constants.ts`,
  `src/shared/keybindings/`.
- Orca's documentation, <https://orca.dev/docs>, read from its source in the same repository
  (`docs/site/content/docs/`), above all <https://orca.dev/docs/model/agents-sessions>.
- Orca's README screenshots (`docs/assets/`).
- Claude Code's session registry (`statusUpdatedAt`), as the CLI writes it today; its format is
  undocumented, as ADR 0121 says of the registry.
