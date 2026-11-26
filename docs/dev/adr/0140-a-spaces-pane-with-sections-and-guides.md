# 0140. A spaces pane with sections and guides

- Status: Accepted, amended by 0143 and 0159; "Waiting rows blink, as amended" (2026-11-21); "`A` on a space's row, as amended" (2026-11-24); "The session shown, as amended" (2026-11-26)
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

## Waiting rows blink, as amended

Amended on 2026-11-21, with the author's choices through the coordinator. The author could not
tell from the pane that a session was waiting for them, at a permission prompt or a question. Its
row had a red `⚠`, one still cell, while working rows spun beside it, so the eye went to them;
and a folded space or worktree said so only with a `⚠ N` at its right edge. The modeline's mark
blinks three times as a session not shown enters needs input (ADR 0121), then stands still. The
author's Emacs setup blinks a waiting session's tab and sidebar row, inverse, until it is
answered.

- **Only needs input blinks** (Q1): `permission`, `question`, `plan`, `elicitation` and `dialog`,
  a session of oso's whose dialog wants the person's answer. Failed, done, ready for review and
  idle stand still. Another program's session never blinks, as it never spins: oso cannot show
  it. Nor does an archived one, which does not spin either.
- **Its mark and name blink inverse** (Q2): on the lit beat they are drawn in the text's
  background colour on a fill of the alert's red, `Face::SessionAlertLit` over
  `Highlight::SessionAlertLit`; on the other beat as before, the red `⚠` and the plain name. The
  text never changes, so `n`/`p`, the cut, point and the row's width stay as they are, and a beat
  shapes nothing again: it sets the buffer's highlights alone, and the frame draws the fill.
  Text is drawn inverse only where its fill is: a roll-up cut at a narrow width keeps its face.
- **Not in front of the person** (Q4): the session shown in the selected window of a focused oso
  window, which is looking at its dialog, does not blink, nor do its roll-ups for it. While a
  prompt is open, no session is in front of the person (ADR 0121's "seen"), so that one blinks
  too, as the prompt has their keys. A change of the session in front draws the list again at
  once while one needs input and the list could blink.
- **625 ms a beat** (Q3), lit then not, from the first beat lit, for as long as it needs input:
  five of the spinner's frames, so that while both run a beat falls on a frame and wakes nothing
  more.
- **The roll-ups blink too** (Q5), folded or open: a worktree's rolled-up `⚠` at its row's right,
  and a space's `⚠ N` at the end of its rule, on the same beat, while a session in them blinks.
  So a folded space still shows that a session in it waits. A repository's row in a space of
  several has no roll-up, and so nothing to blink; the title row's counts, `Other projects`,
  `Archived` and, grouped by state, the `Needs input` header stand still too.
- **`[sessions] blink = false`** (Q6) keeps them still, and so sets no deadline for them; like
  `spinner`, it applies at once. The system's "reduce motion" setting is not read, as for the
  spinner (ADR 0131, Q14).
- **The modeline is unchanged** (Q7): its `⚠ N` still blinks three times as a session not shown
  enters needs input.
- **One clock** (`sessions/spinner.rs`'s `Clock`) keeps the spinner's frames and the blink's
  beats from one epoch, with one deadline on the event loop's wait: the next frame while a
  spinning row is in view, else the next beat while a blinking row is. While neither is in
  view, while oso's window is hidden, or with both off, no deadline is set: nothing runs while
  nothing waits. A render reads it as a `Beat`: the frame, whether the beat is lit, and the
  session in front of the person.
- **Built in** `sessions/list.rs` (`Beat`, `Rendered::blinking`, `SpaceBands::lit`, the cells
  drawn inverse), `list/row.rs`, `tree/worktree.rs` and `tree/space.rs`; the fill in
  `app/draw/spaces.rs` and `tiled.rs`, over the point's band, by the cells of the line each
  screen row shows; the face and its contrast pair in `oso-render`, on its own fill at 4.5:1 in
  every built-in theme, and listed among the fills a highlight keeps its face on. oso-core's
  public API gains the one `Highlight` variant; `oso-render`'s `Face` gains the one face. No new
  crate, crate edge, module or trait. A span the list cuts with its text now keeps whole
  characters only, as a cut may put its `…` where a span began.
- **Cost.** A beat renders the list, as a spinner's tick does, and sets its highlights alone. In
  the tree, with twenty waiting rows of forty and their roll-ups, it costs what a tick of twenty
  spinning rows does, within a tenth, and so stays inside ADR 0131's 0.2 ms where the tick does;
  grouped by state, each waiting row has its reason's line too, and a beat costs about a sixth
  more. Release-only tests take both in turn and hold those ratios. At most 1.6 beats a second
  while nothing spins, a fifth of the spinner's rate, inside ADR 0131's sustained budget; while
  it spins, a beat adds no wakeup.
- **Tests.** Unit tests cover the beats and the deadline; the rows and roll-ups that blink,
  folded or not, and on both beats; those that do not (in front of the person, another
  program's, other states, the switch off); the same text on both beats; that whatever is drawn
  inverse is on its fill at every width from 1 to 59; the fill's cells cut to the window; the
  setting; the cost; and, in `oso-render`, that a lit beat draws the glyphs inverse on their fill
  and neither beat shapes anything again. An end-to-end flow checks in the running app that a
  waiting session in front of the person stands still, blinks once it is not, and stands still
  once it is answered.
- **Changes to accepted ADRs**, each passage staying as written: [ADR
  0131](0131-an-orca-style-sessions-pane.md), "A working mark spins (Q5) … Every other mark is
  still": a waiting session's mark and name blink, and the roll-ups of one; and "The spinner
  (Q5, Q11–Q14) … It is the one timer the pane runs": the spinner and the blink share that one
  timer.

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

## `A` on a space's row, as amended

Amended on 2026-11-24, with the author's choices through the coordinator: `A` on a space's row did
nothing, and now closes that space, asking nothing
([ADR 0136](0136-issue-spaces.md), "A chosen space is closed, as amended").

## The session shown, as amended

Amended on 2026-11-26, with the author's choices through the coordinator. The author could not
tell from the pane which session was open: the front worktree's rows had a faint tint, and point's
band showed only while the pane was selected. And a space not in front sometimes showed open.

- **The session shown**: the row of the session the tree's window shows (`Editor::tree_window`:
  its terminal, or a native session's transcript, or its composer, card or call shown in a window
  of its own) has its own band (`Face::SessionShown`), stronger than the front worktree's tint,
  and its label in the front's colour (`Highlight::SessionFront`), whether the pane is selected or
  not. One row at most: other windows' sessions, a file shown, or a row hidden by a fold mark
  nothing. While the pane is selected, point's band is drawn over it.
- **The session shown's rows open**: each time the session the tree's window shows changes, its
  space, its repository, its worktree and the session it is nested under open, so its row is in
  sight. One folded again by hand while it stays shown stays folded.
- **A space not in front is folded** (amends [ADR 0136](0136-issue-spaces.md), step 2, Q4): each
  time the space in front changes, every other space folds, whatever `TAB` or `S-TAB` chose, and
  a session working or waiting in it no longer opens it: its row's roll-up shows that. One opened
  by hand stays open until the space in front changes again.
- **With no space in front**, as before, none is folded.
- **Colours:** the band is a muted purple, the hue of the front's name, apart from point's blue and
  clearer than the tint's grey: dark `#2a2140`, light `#e9dcf5`, dusk `#5a4a72`. Every session
  face is checked at 4.5:1 on it, as on the other bands.
- **Built where:** the list (`sessions/list.rs`, `row.rs`, `tree.rs`) takes the session shown with
  `List::set_shown` and marks its rows in `SpaceBands::shown`; the app finds it
  (`App::shown_session`) and draws the band under point's (`app/draw/spaces.rs`, `tiled.rs`).
  `oso-render`'s `Face` gains `SessionShown`. No new crate, edge, module or trait.
- **Changes to accepted ADRs**, each passage staying as written: this ADR's "**The pane not
  selected** draws no band for point; only the front worktree's tint shows": only the front
  worktree's tint and the session shown's band show.
