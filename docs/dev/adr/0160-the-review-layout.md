# 0160. The review layout

- Status: Accepted, amended by 0163, 0167 and 0169
- Date: 2026-11-19

## Context

During a review ([ADR 0122](0122-agent-review.md)) the author jumps to the source with `RET`,
opens the old side with `C-u RET` and talks to an agent session. Step 3 part three put the source
in "another window, else one split below", as Emacs's `diff-goto-source` does; the old side goes
there too, and the message of comments `C-c C-c` makes takes the selected window. Each of these
finds its own place, so windows pile up while reviewing, and nothing puts back the windows from
before the review.

The pieces to fix this exist. [ADR 0121](0121-agent-sessions-and-spaces.md) takes a whole tree of
tiled windows out of the frame and puts it back (`Editor::swap_window_tree`), with every window's
cursors, views, places and pair kept: each space keeps its own tree, and `space-zoom` keeps the
tree from before it. [ADR 0112](0112-window-splits.md) left rules for where a buffer is shown
(Emacs's `display-buffer`) for later. `C-x v r` opened the review menu ([ADR 0117](0117-menus.md)).

The author asked for a fixed layout: the review at the left, one slot at the right, one key back
from the slot to the review, and the windows from before restored on `q`, as magit's quit and
ediff's restore do. A later step, `review-ask`, sends questions to a space's orchestrator and shows
its answer; the slot must be able to show the orchestrator's terminal for it.

The author chose (a) on each question below on 2026-11-19, through the coordinator.

## Decision

### The layout

- **The review layout** is the frame's tree made of the review at the left and one window at its
  right, the **slot**, side by side. The slot is two fifths of the row, the review three (Q4),
  resized with `C-x }` and `C-x {` as any window, and kept so while the layout lives.
- **The slot shows one thing at a time (Q7):** the source `RET` goes to, the side `M-RET` or
  `C-u RET` shows,
  the `*review message*` `C-c C-c` makes, and later the orchestrator's terminal `review-ask` shows,
  each in place of the last. It first shows what the selected window showed as the layout was
  made, else the buffer that window showed last that is no review's, never one that may hold
  secrets or is withheld from agents, which would come back on screen unasked (ADR 0044); else
  `*Messages*`.
- **Every person's open of a review shows it in the layout (Q2):** the menu's scopes,
  `review-resume`, a review opened from a git status, a graph or a pull request. While the frame
  is the layout, another review opened goes to its left, selected, from wherever the person is.
  A review a restart opens again shows in its window, as before. Agents open no review.
- **Outside the layout,** `RET`, `M-RET` and `C-u RET` keep step 3's rule.

### One key (Q1)

`review-layout` (`Person`: it takes the person's whole tree out and shows a review, whose looks
run git) is bound to `C-x v r`, and any key may bind it:

| Where | What it does |
|---|---|
| no review open, or with a prefix argument | opens the review menu, at once as the command ends, as a menu key opens it, so that a keyboard macro's next key goes to it |
| a review open, no layout | shows the layout of the review the selected window showed last of the selected buffer's project, else the one of that project opened last, else the one the window showed last of any project, else the one opened last |
| in the slot, or any window but the review's | selects the review again where the person left it: the window a jump was asked in last, else the layout's |
| in the review, its panes or its files pane | restores the windows from before the layout |

`q` in the review restores too, and so does `C-x k` of the layout's review (Q8). `C-x o` still
goes through the windows.

- **Only the person's `q` restores.** `review-quit` is `Change`, so an agent may bury a review
  as before; restoring the whole tree would choose where the person's next keys land, as
  `review-layout` and `space-zoom` would, which are `Person`. So an agent's `q` asks for no
  restore (`Editor::runs_for_agent`), and the layout stays.
- **The restore comes after the key:** the host carries out the request once the dispatcher
  ends, as it does `space-zoom`'s, so keys sent with `q` in one batch, or after it in a keyboard
  macro's replay, run in the window `q` buried before the windows come back.
- **The menu's prefix argument:** `C-u` before `C-x v r` now means the menu, so it goes to no
  head; a prefix typed inside the menu still builds one for the next head (ADR 0117, Q15).

### What is kept (Q3)

- **The tree from before** is the frame's whole tree at the moment the layout is made from
  outside it, taken out as `space-zoom` takes its tree, and never taken again while the layout
  lives: opening another review there keeps it. Restoring puts it back exactly, every window's
  cursors and view as they were; the layout's tree is dropped, and the buffers opened meanwhile
  stay open, their edits kept. As for a space's tree, a cursor in the kept tree is clipped to the
  text as it is when it comes back, not carried through edits made meanwhile.
- **The layout's windows are known by their names (`WindowId`), never by their places,** so that
  a window the person moved, split or resized is still found:
  - A slot split by hand keeps its name; a slot deleted is made again by the next jump, at the
    right of the review's window, a pair's whole, two fifths of the row.
  - `C-x 1`, `C-x 0` and `C-x b` do not end the layout. The way back selects any window showing
    the review; with none, a layout is made again in place of the frame's tree, keeping the tree
    from before.
- **Several reviews:** one layout a tree. Its left shows one review at a time.
- **Spaces:** each space's tree keeps the trees behind it (ADR 0121), now a stack: `space-zoom`'s
  and the layout's. Switching spaces takes the layout with its space. A zoom inside the layout
  zooms back out to it; restoring from inside a zoom drops the zoom and gives back the tree from
  before the layout; a zoom made before it is given back still zoomed. While zoomed inside the
  layout, `RET` follows step 3's rule.

### The two panes and the files pane (Q5, Q6)

- **The pair takes the layout's left,** side by side or stacked as step 11's judge has it by the
  left's own width, the slot staying at the right. On a narrow frame that means stacked or one
  stream; a later layout toggle (the slot at the bottom) would give the panes the whole width.
- **The files pane** is unchanged: 40 columns at the left of the review's area, taken from the
  review, so the frame is files, review, slot.

### Restarts (Q9)

A restart hands over only the tree in the frame (ADR 0121, as built): the tree from before is
not carried. The frame comes back as the layout's windows, the review opening again in its window
(ADR 0122, Q-H), and the state's review entry gains an optional `slot`, the slot's place among the
state's windows, so that the frame is still the layout. With nothing to restore, `q` and
`C-x v r` in the review then leave the review's window alone, showing what it showed behind the
review. A reader before this ADR leaves `slot` out.

### Public items

- `ReviewRequest::Layout { menu: bool }`, which `review-layout` asks: the host keeps the layout
  with the spaces' trees, so it says what the key does where the person is.
- `ReviewRequest::Quit`, which `review-quit` asks after it buried the review, for the person
  only.

The author approved both on 2026-11-19, through the coordinator.

The rest is in the tip crate: `sessions::spaces::kept` (the stack of kept trees and the layout's
entry, `space-zoom` moved there) and `app/reviews/slot.rs` (the layout's choices, free functions
over the editor and the stack). Nothing new is public in `oso_core::frame` or `oso_core::window`.

## Changes to accepted ADRs

- [ADR 0122](0122-agent-review.md):
  - Step 3 part three, "The window (Q2, Q3)": in the review layout, the source and the sides go
    to its slot; outside it the rule stands.
  - "The menu", `C-x v r`, and the Keys' "`q` bury": `C-x v r` runs `review-layout`, which opens
    the menu with no review open or with `C-u`; `q` restores the windows from before the layout
    when the frame is one, and buries the review otherwise.
  - Step 12a, "`*review message*` opens in the selected window": in the layout, in its slot,
    selected; sending or dropping it selects the review again.
  - Step 6, Q-H: the restart's review entry gains an optional `slot`.
  - `review-kill` and `C-x k` of the layout's review put back the windows from before it.
  - Step 3's "`review-quit` … is `Change`" holds: an agent's `q` buries the review, and only the
    person's restores the windows.
- [ADR 0117](0117-menus.md), "Default keys (Q25)", and [ADR 0143](0143-agent-keys-on-c-c-a-and-an-agents-menu.md)'s
  restatement of them: `C-x v r` is no longer one of the built-in menus' default keys; the review
  menu keeps its name, `open-menu` and `config.toml` reach it, and `review-layout` opens it.
  ADR 0117, "The prefix argument (Q15)": a prefix before `C-x v r` opens the menu and goes to no
  head.
- [ADR 0112](0112-window-splits.md), "rules for where a buffer is shown … are left for later": the
  review layout's slot is the first such rule, for a review's jumps and message only.
- [ADR 0121](0121-agent-sessions-and-spaces.md), `space-zoom`'s one saved tree: each space keeps a
  stack of trees behind its own, the zoom's and the review layout's, so that the two nest.

## Consequences

- Reviewing no longer adds windows: everything the review shows goes to one slot, and one key
  goes back and forth and restores.
- The review is three fifths of the frame. On the end-to-end window of 99 columns it is 59 wide;
  the panes there are stacked, and side by side only on a frame of about 270 columns or more.
  The author may want a toggle later (a ratio, or the slot at the bottom): the layout's ratio is
  one constant, and its windows are found by name. The slot's share is set from the frame's
  regions in the tip crate (`fit_slot`); a toggle that puts the slot below would rather have
  `oso-core` set a window's share of its run, a public item to approve then.
- The review benchmark (`crates/oso/examples/perf/bench/review.rs`, unchanged, as it is a check
  file) now opens its review in the layout, so its first screen, keys and panes are measured in
  a review three fifths wide, its panes stacked. Its numbers are recorded with no threshold.
- `review-ask` (a later step) shows the orchestrator's terminal through the same slot,
  `App::show_in_review_slot(buffer, select)`, without selecting it, so the answer shows beside
  the diff while the person stays in it.
- A restart loses the tree from before, as it loses a zoom and the other spaces' trees. Carrying
  kept trees across a restart, all three together, is left for later (open-questions).
- Every e2e flow that opened a review now sees two windows; the flows were updated, and one
  flow walks the layout.

## Alternatives considered

- **`C-x v r` stays the menu, with a head for the layout and a separate key back** (Q1 (b)): two
  keys to learn for one place, and the menu would open on the way back.
- **Only `C-x v r` makes the layout; the menu opens reviews in the selected window** (Q2 (b)):
  the windows would pile up again whenever a review is opened from the menu.
- **Keep only the shape, the buffers and the positions, and rebuild them** (Q3 (b)): more code for
  the same result, and a pair or a window's places would be lost.
- **A ratio of 1:1 or 2:1** (Q4 (b), (c)): the review is what is read most; 3:2 keeps it wide
  while a source file of 80 columns still fits the slot on a laptop's frame.
- **The slot below the panes while two panes show, or hidden** (Q5 (b), (c)): the layout would
  change shape under the person; the later toggle covers it.
- **The files pane at the top of the slot's column** (Q6 (b)): the diff would keep its width, but
  the pane would no longer sit beside the review it lists, as step 6b built it.
- **The draft stays in the selected window** (Q7 (b)): it would cover the review being commented
  on.
- **The layout ends once no window shows its review** (Q8 (b)): a `C-x b` in the review's window
  would lose the person's tree from before.
- **Carry the tree from before across a restart** (Q9 (b)): exact, but it changes the handoff's
  format, which is best done once for the spaces' trees, a zoom and this together.
