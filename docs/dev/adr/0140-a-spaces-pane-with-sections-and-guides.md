# 0140. A spaces pane with sections and guides

- Status: Accepted
- Date: 2026-11-16

## Context

The left pane of [ADR 0131](0131-an-orca-style-sessions-pane.md), `*Sessions*`, became a tree of
spaces, repositories, worktrees and sessions with [ADR 0136](0136-issue-spaces.md). The author uses
it daily and raised three problems (2026-11-16):

1. **The structure is unclear.** Nothing tells a space's row from a repository's, a worktree's or
   a session's.
2. **They did not find that the pane takes the cursor.** `C-x a s` selects it, then `n`/`p`,
   `RET` and `TAB` work, but nothing shows that or how.
3. **`*Sessions*` does not name what it shows.**

The pane was run with a dev build and a realistic state: a space of one repository with its main
checkout, three worktrees and five sessions in mixed states, a second space of three
repositories, external sessions and an archived one. It showed:

- **Containers look alike.** A space, a repository, a quiet row, `Other projects` and `Archived`
  are each `▾ name`; only the indent differs.
- **Worktrees and sessions look alike.** A worktree's row begins with its sessions' rolled-up
  state mark, as a session's row begins with its own; two cells of indent are the only
  difference. A worktree's only session often has the worktree's name, so the pair reads as
  one row twice (`○ refactor-auth` over `○ refactor-auth`).
- **Indent means different things.** A space of one repository shows its worktrees right under
  its row (ADR 0136, Q1), so two cells in is a worktree there and a repository in a space of
  several.
- **An empty repository row** is an open fold with nothing under it.
- **Focus does not show.** The point's band is drawn the same whether the pane is selected or
  not; only the modeline's colour changes, and its text was cut (`*Sessions*  tree · ? key`).
  `C-x a s` in the pane hides it rather than going back.
- oso's text faces are colours only (no bold), but rows can take a background, as the current
  line's band and a review's bands do.

The design was offered as three options at 44 columns: sections and guides, Orca's two-line cards,
and a minimal change. Orca's sidebar ([ADR 0131](0131-an-orca-style-sessions-pane.md#what-orca-does))
tells its levels apart mostly by form: a header bar for a host, an icon for a repository, a card
for a worktree, small rows for agents.

## Decision

The pane becomes `*Spaces*`. Each level of the tree gets its own form, as in this mock at 44
columns:

```text
 Spaces                        ⚠ 1  ✗ 1  ◆ 1
 ▾ webapp ────────────────────────────── ⚠ 1
   ▾ main  primary                     ✗  2m
     ├ ✗ webapp-2                         2m
     │   Migrate the build
     └ ⠹ Wire the new settings page       3m
   ▾ fix-login                         ⚠  2m
     └ ⚠ Fix the login redirect           2m
         Do you want to proceed?
   ▾ add-search                        ◆  2m
     ├ ◆ Add search to the header         2m
     └ ○ search-perf-check               14m
   ▸ refactor-auth  (1)                ○  2m
 ▾ checkout-flow-v2 ──────────────────── ▶ 1
   ● orchestrator                         1m
   ▾ api/                                ○ 2
     ▸ main  primary  (1)              ○  2h
     ▾ checkout-api                    ○ now
       └ ○ checkout-api                  now
   ▸ notes/
 ▸ Other projects ─────────────────── ▶ 1  2
 ▸ Archived ────────────────────────────── 1
```

### Rows (Q1, Q2)

- **One left column for the cursor.** Every row of the tree begins one cell in. The cell is the
  point's bar's (see [Focus](#focus-q3-q4)).
- **A title row first:** `Spaces`, and at the right the most urgent state over every session of
  oso's with its count, then the next two, as the modeline's marks. Its name is in the space's
  face.
- **A space's row is a section header:** the fold glyph, the name in the space's face (the front
  face while it is in front), a `─` rule out to the roll-up, and a band behind the whole row
  (`Face::SessionHeader`). `Other projects`, `Archived` and `Archived spaces` are drawn the same,
  their names in the text's colour.
- **A repository's row** (in a space of several) is the fold glyph and the name with a `/`, in the
  repository's face, and the roll-up at the right. One with nothing to show is folded (`▸`), with
  no roll-up.
- **A worktree's row** begins with the fold glyph, then the name, its branch dim where it differs
  (the main checkout: its branch and a dim `primary`), and a folded worktree's session count,
  `(1)`. Its sessions' rolled-up mark moves to the right, two cells before its age, in the
  state's face; it spins as before. A worktree with no session is `▸`, with no mark.
- **A session's row** begins with a dim tree guide, `├` or `└` (the last of its worktree), then
  its mark, its label and its age. A row under a session that is not the last continues the guide
  with `│`, so a reason line and a nested session hang under it. The orchestrator's rows (ADR
  0136) and archived sessions keep their places, with no guide.
- **A session's label (Q2)** is its name, or its title (the terminal's, without a spinner, as its
  recap is) when its name is its worktree's name. External sessions keep their names.
- **Faces:** the space's name `Highlight::SessionSpace`, a repository's `Highlight::
  SessionRepository`; guides, rules, branches, counts and ages `SessionQuiet`, as now. Each is
  checked at 4.5:1 on the text's background, the header band and the point's band, as the state
  faces are.
- **The front worktree** (ADR 0131's "space in front") has a faint background on its row and its
  sessions' (`Face::SessionFront` stays its name's colour, `Face::SessionFrontTint` the
  background), as Orca's selected card.
- The by-state list (`s`) keeps its rows, one cell in.

### Motion

- **A row's mark** is its first character past the indent and the guides (`│├└`).
- **`n`/`p`** stop at a row whose mark is a fold glyph or a state mark, as now.
- **`M-n`/`M-p`** stop at a row whose mark is a fold glyph: a space, a repository, a worktree or a
  group. Every container has one now, so the rule that a worktree is two cells in from a fold row
  goes, and with it the orchestrator's stop (it is a session, reached with `n`).

### Focus (Q3, Q4)

- **`C-x a s` and `C-c s` toggle focus (Q3):** they show and select the pane, select it when it is
  shown, and go back to the tree's window selected last when it is selected, as `C-x o` does from
  it. `q` in the pane goes back too. **`C-u C-x a s` hides the pane.**
- **The selected pane (Q4)** draws the point's row in an accent band (`Face::SessionPoint`) and a
  `▌` in the row's first cell (`Face::SessionPointBar`; as built, a one-cell fill). **The pane
  not selected** draws no band for point; only the front worktree's tint shows.
- **Modeline:** the pane's says `Spaces` and the grouping, then the hints: `C-x a s to enter`
  while not selected, `q back · TAB fold · ? keys` while selected. It no longer says the buffer's
  name or `%%`, as ADR 0131 meant: its note is all it shows.

### Keys and mouse (Q5, Q6)

| Key | Does |
|---|---|
| `q` | back to the tree's window selected last (`sessions`) |
| `S-TAB` | cycle every fold: spaces only, then down to worktrees, then everything (`sessions-cycle-folds`) |
| `<right>` | open a folded row; on an open one, go to its first row (`sessions-open`) |
| `<left>` | fold an open row; on a session or a folded row, go to the row it is under (`sessions-close`) |

- **`S-TAB`** starts from the deepest level any open row shows, and goes on round. It sets the
  folds it passes as the person's, as `TAB` does.
- **A click on the pane not selected (Q5)** selects it and moves point to the row, and acts on
  nothing else. A click on the selected pane acts as `RET`, as now. A click on a fold glyph folds,
  selected or not. The wheel scrolls, as now.
- New commands and their reach (ADR 0085): `sessions-cycle-folds`, `sessions-open` and
  `sessions-close` are `Look`, as `TAB` and `n` are. `sessions` stays `Person`.

### Name (Q7, Q8)

- **The buffer is `*Spaces*` (Q7).** Every text that names the pane says so: echo messages,
  command docs, `?`, the user's documentation.
- **The commands keep their names (Q8):** `sessions`, `sessions-next` and the rest. `sessions`'
  doc says what it does now.

### Built where

- The tree's rows in `sessions/list/tree.rs`, `tree/space.rs` and `list.rs`'s session row; the
  label rule beside them. The title row in the tree's renderer.
- The bands (header, front tint, point) and the bar in the app's drawing of the side window
  (`app/draw/tiled.rs`), from the rendered list's line kinds and the window's screen rows, as
  fills and a label beside the ones it draws now. oso-core's `current_line_cells` is unchanged:
  the app draws its rows in the point's face while the pane is selected and leaves them out
  otherwise.
- The modeline's note set by the app when the pane's selection changes.
- **oso-core's public API gains** two `Highlight` variants, and `SessionRequest::CycleFolds`, one
  line, for `S-TAB` (the host knows each level's folds; the list's text does not hold the folded
  rows). `oso-render`'s `Face` gains `SessionHeader`, `SessionFrontTint`, `SessionPoint`,
  `SessionPointBar`, `SessionSpace` and `SessionRepository`, each in every built-in theme.
- No new crate, crate edge, module or trait in a library crate.

### Cost

- Nothing new runs while nothing changes. The bands are computed with the frame from the rows in
  view, as a review's are. A change of selection sets the modeline's note and the title row once.
- The tree's render stays within ADR 0131's 0.5 ms for 20 worktrees, 40 sessions and 300 external
  sessions folded; its release-only test keeps it there.

## The author's choices

The author answered on 2026-11-16, through the coordinator, the recommended option for each:
sections and guides (Q1); a session's title only when its name is its worktree's (Q2); `C-x a s`
toggles focus, `q` returns, `C-u C-x a s` hides (Q3); an accent band and `▌` while selected, no
band otherwise, and modeline hints (Q4); a click on the pane not selected selects and moves point
(Q5); `S-TAB` and `<left>`/`<right>` (Q6); `*Spaces*` (Q7); the `sessions-*` names kept (Q8).

## As built

Built on 2026-11-16 in one pull request with this ADR. The author approved each point below that
differs from the decision, through the coordinator, after seeing the pane in the running app.

- **The bar at point is a one-cell fill** in an accent (`Face::SessionPointBar`, the palette's
  azure), not a `▌` glyph: a window's labels draw ASCII keys only, and a fill needs no change to
  the renderer. Every row still leaves its first cell to it.
- **The selected pane's modeline** says `Spaces · q back · TAB fold · ? keys`; beside the
  `Agent` marks a 44-column modeline cuts its end, so `q back` comes first.
- **Where it is built:** the rows in `sessions/list/row.rs` (a session's), `list/tree.rs` (the
  title and sections), `tree/worktree.rs` (a worktree's row and its hung sessions) and
  `tree/space.rs`; the bands in `app/draw/spaces.rs`, beside `tiled.rs`; the note in
  `app/sessions/watch/view.rs`, set before each frame. `oso-render`'s map from highlights to
  faces moved to a private `pane/highlights.rs`, to keep `pane.rs` under 600 lines.
- **A session's title** for its label is its terminal's, cleaned as a recap is and without its
  spinner, read when the list is made.
- Verified in a dev build: the guides and rules are one cell wide in the default font, and the
  rows stay aligned.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0131](0131-an-orca-style-sessions-pane.md), "`*Sessions*` becomes a tree of projects,
  worktrees and sessions": the buffer is `*Spaces*`.
- ADR 0131, "**Worktree row:** two cells of indent, the most urgent state's mark of its sessions
  … the worktree's directory name": a worktree's row begins with a fold glyph; the rolled-up mark
  is at the right, before its age.
- ADR 0131, "**Session row:** four cells of indent (six when nested), its mark, its name": a
  session's row begins with a tree guide, then its mark and its label (the name, or the title
  when the name is its worktree's).
- ADR 0131, "**The pane's modeline** names the grouping and the help key, `Sessions · tree · ?
  keys`": `Spaces`, the grouping and the hints for entering or leaving, by whether it is selected.
- ADR 0131, "**The row at point is highlighted** across the pane's width": only while the pane is
  selected, in the point's accent face, with an accent bar in the row's first cell.
- ADR 0131, "**The space in front** … has a faint background": built, on the front worktree's
  rows.
- ADR 0131, "Keys and mouse", `M-n`/`M-p` "next / previous worktree or project row": the next
  row with a fold glyph; and "a click moves point to the row and does what `RET` does there" (as
  built, "It acts whether the list was selected or not (Q2)"): on the pane not selected, a click
  selects it and moves point only.
- ADR 0131, step 2b as built, "a heading is a row whose mark is a fold's, or a row at a
  worktree's indent under a project's": a heading is a row whose mark is a fold's.
- [ADR 0136](0136-issue-spaces.md), "A space row: the fold glyph, the name, and at the right the
  most urgent state of its sessions with the count": also a `─` rule to the roll-up and a band.
- ADR 0136, "each member repository, a row with a fold glyph": its name with a `/`.
- ADR 0136, step 3 as built, "Its row is two cells in from the space's, so `M-n` stops at it": the
  orchestrator's row is a session's; `n` stops at it, `M-n` does not.
- [ADR 0121](0121-agent-sessions-and-spaces.md), via the `sessions` command, "select it if it is
  shown; hide it if it is selected": go back to the tree's window if it is selected; hide it with
  a prefix argument.

## Consequences

- Each level has its own form: a band and a rule for a space, a `/` for a repository, a fold glyph
  for a worktree, a guide and a state mark for a session. Indent no longer has to carry it.
- The person sees when the pane has their keys, how to enter it, and how to leave.
- The pane loses one column to the bar's cell.
- The `M-n` rule is simpler and no longer depends on indents.
- e2e flows and tests that name `*Sessions*` or match its rows change with it.
- Follow-up: a key to select the pane from anywhere without the `C-x a` prefix, if `C-x a s`
  turns out to be too long; agents' icons in the mark's place, when ADR 0031's adapters come.

## Alternatives considered

- **Orca's two-line worktree cards,** with the session's title on every session row: closest to
  Orca, but about 30% more rows, and the author chose one line a row in ADR 0131 (Q2).
- **The minimal change** (a rule on space rows, `/` on repositories, a fold glyph on worktrees, no
  guides, no bands): cheap, but a session is still told from a worktree's next row by indent.
- **Glyphs for each level** (a branch sign for a worktree, a folder for a repository): the
  monospaced fonts oso uses lack most of them, and a fallback font may draw them two cells wide.
- **Hints only in the modeline,** with no change to the band: the band that showed whether or not
  the pane was selected is what hid the focus.
- **Keeping `C-x a s` as show, select or hide, and adding `q`:** the person who presses the key
  again to leave loses the pane.
- **Renaming the commands to `spaces-*`:** churn in keys and documents for a name only the docs
  show; the author kept them.
