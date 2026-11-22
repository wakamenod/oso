# 0169. The review slot keeps the source above the session

- Status: Accepted
- Date: 2026-11-22

## Context

The review layout ([ADR 0160](0160-the-review-layout.md)) has the review at the left and one
window at its right, the slot, which "shows one thing at a time (Q7)": the source `RET` goes to,
the side `C-u RET` shows, the `*review message*` `C-c C-c` makes, and the session's terminal
`review-ask` (`Q`) shows ([ADR 0163](0163-reviews-in-a-space.md), "Shown in the slot"), each in
place of the last.

So a question asked about a line the person has open in the slot replaces that source with the
session's terminal. The person reads the answer without the code it is about, and a `RET` to
the source then hides the answer. The author wants to see both.

The author chose on 2026-11-22, through the coordinator: the slot splits top and bottom, the
source on top and the session below, the diff at the left unchanged; a later `RET` changes only
the top part, a later `Q` reuses the bottom, and the review stays selected, as before. The
author chose (a) on each question below.

## Decision

### The split

- **`Q` never covers what the slot shows (Q1):** when `review-ask` shows a session (its terminal,
  or a native session's buffer) while the slot shows anything other than that session or
  `*Messages*` (a source, the old side, the message, what the slot showed as the layout was made
  or made again, or a buffer the person put there), the slot splits top and bottom. What it
  showed stays on top, in the slot; the session goes to the new window below, the **ask
  window**. While the slot shows `*Messages*` or that session already, the session takes the
  whole slot, as before.
- **The review stays selected,** as before; the ask window is not selected.
- **A later `Q` reuses the ask window,** whichever session it shows then.
- **A later `RET`, `C-u RET` or `C-c C-c` takes the top alone,** the slot, as before, the ask
  window kept.
- **A jump while the session fills the slot (Q2):** when `Q` came first, so that the session
  fills the slot, a later `RET`, `C-u RET` or `C-c C-c` splits it the same way: the session
  moves below, and the top takes the source.
- **Too short to split:** the session shows in the slot in place of what it showed, as before;
  and a jump or a message while the session fills the slot replaces it there, as before.

### Sizes (Q4)

The slot's column keeps its two fifths of the row. The split takes its height in halves, the
top half the slot's, as `C-x 2` splits; `C-x ^` and the other resize keys change it as any
window's.

### The windows by name

The ask window is known by its name (`WindowId`), never by its place, as the layout's other
windows are (ADR 0160, Q3):

- **The ask window deleted by hand:** the next `Q` splits the slot again.
- **The slot deleted by hand, the ask window still there:** the next `RET`, `C-u RET` or
  `C-c C-c` splits the ask window; its top half becomes the slot and takes the source, the
  session staying below. Too short to split, the ask window becomes the whole slot again. The
  same holds while the person is in the slot itself, which a jump never takes (ADR 0160).
- **A window that shows a review is never the ask window.**
- **The way back** from the ask window to the review is `review-layout`'s row "in the slot, or
  any window but the review's", and session mode's `C-c C-v` (ADR 0163, Q11), unchanged.
- **The layout's end** (`q`, `C-x k` of its review, `review-layout` in the review) drops the
  layout's whole tree, the ask window with it, unchanged. A layout made again (no window shows
  its review) has no ask window.

### Restarts (Q3)

The state's review entry gains an optional `ask` beside `slot`, the ask window's place among
the state's windows, written only with a slot, never the slot's or one of the review's own
places. A restart carries it, so a `Q` after the restart reuses that window rather than
splitting the slot a third time. The session is what the ask window, else the slot, shows as the
review opens again, while that is a session's, so that a session filling the slot still goes
below on the next jump (Q2). A reader before this ADR leaves `ask` out; a state without it
carries no ask window, as before.

### Public items

None in a library crate. In the tip crate: `ReviewKept` gains `ask`, the ask window, and
`asked`, the session's buffer `review-ask` showed last, so that a jump while it fills the slot
keeps it below; `carry::reviews::{Shown, ReviewEntry}` gain `ask`. `App::show_in_review_slot`
keeps its signature: a buffer shown there not selected, which only `review-ask` asks for, goes
through the split; one shown selected, the message, goes to the slot as before.

## Changes to accepted ADRs

- [ADR 0160](0160-the-review-layout.md):
  - "The slot shows one thing at a time (Q7)": the slot shows one thing at a time, except that
    the session `review-ask` shows goes below what it shows, as above.
  - "Restarts (Q9)": the state's review entry gains an optional `ask` as well as `slot`.
  - Consequences, "`review-ask` (a later step) shows the orchestrator's terminal through the same
    slot, … so the answer shows beside the diff": below the slot's source while there is one.
- [ADR 0163](0163-reviews-in-a-space.md), "Shown in the slot": the session's terminal shows in the
  slot's lower half while the slot shows something other than it or `*Messages*`, and in the
  whole slot otherwise; not selected, as before. Its "Changes to accepted ADRs" entry for ADR
  0160's Q7 is changed the same way.

## Consequences

- The answer and the code it is about show together, and stepping through sources with `RET`
  leaves the answer on screen.
- The slot's source has half the rows it had while a session shows below it. On the end-to-end
  window the slot is about 40 columns wide; the halves are about a third of a laptop screen's
  height each.
- The ask window is shorter than the whole slot, but its program keeps the size of the window
  that selected its terminal last until the person selects the ask window, as with an unselected
  slot before (a terminal is sized by the window that owns it).
- After `C-x 1` in the review, the next `RET` makes the slot again, and the next `Q` splits it.
- Both halves can show the session for a while: when the message on top is sent or dropped, the
  top shows what it showed before it, the session, as any window does when its buffer goes; and
  a jump that splits the slot and is then refused leaves the session on top. The next jump puts
  a source there.

## Alternatives considered

- **Split only below what a review put in the slot** (Q1 (b)): what the slot showed as the layout
  was made, the file the person was in, would be replaced, so whether `Q` hides the slot would
  depend on how its buffer got there.
- **A `RET` replaces a session that fills the slot** (Q2 (b)), as before: the answer would go as
  soon as the person looked at the code it names.
- **Leave the ask window out of the restart** (Q3 (b)): a `Q` after a restart would split the slot
  again, three windows high. The state's entry gains one optional field, as `slot` was added.
- **Source two fifths, session three; or source three, session two** (Q4 (b), (c)): an answer is
  often longer than a hunk, but either is a resize away, and halves are what `C-x 2` gives.
- **"As amended" sections in ADR 0160 and 0163** (README's narrow amendment): the change touches
  two rules of ADR 0160 (the slot and the restart's entry) and one of ADR 0163, so it is a new
  ADR.
- **An explicit argument to `App::show_in_review_slot` saying "a question's session"**: clearer at
  the call sites, but it would change `review-ask`'s delivery code while it was being changed
  elsewhere; "not selected" is `review-ask`'s alone today and can become an argument later.
