# 0184. Continue a review where it was left

- Status: Accepted
- Date: 2026-11-26

## Context

The author often leaves a review for other work: another file, another space, a quit. Coming back
meant the review menu again, then finding the place in the diff. `q` buried the review and dropped
the layout's state; `C-x k` and a quit kept only the saved comments, the view's choices and the
cursor's line ([ADR 0122](0122-agent-review.md), step 6). The author asked on 2026-11-26 for one
key that brings everything back as it was: the diff, its scroll, the slot and the session below
it, also across a quit. The answers came through the space's orchestrator.

## Decision

### The key

- **The review key is `C-c a v`**, the agents menu's `v`, `review-layout`. `C-c a V`, its `V`,
  stays the review menu, and `C-c C-v` in a session's buffer stays `review-layout`.
  [ADR 0122](0122-agent-review.md) and [ADR 0160](0160-the-review-layout.md) bound it to `C-x v`
  then `r` too; that binding is removed, and every ADR and doc that named it names `C-c a v`, or
  `C-c a V` where the review menu is meant, with a line saying the key changed here.
- **`C-u` reaches the head.** The agents menu gives its head the prefix argument (ADR 0117, Q15):
  `C-u C-c a v` and `C-c a C-u v` open the review menu, as `C-u` before the old key did. In a
  space whose orchestrator is recorded, that is the start that asks for the repository first
  ([ADR 0163](0163-reviews-in-a-space.md)).
- **No new command, head or public item.** `ReviewRequest::Layout { menu }` covers it: `menu` is
  the prefix argument alone, and only its doc text changes.

### Left

A review is **left** as the person opens it, and each time it stops being what the selected
window shows: `q`, `C-x k`, `C-c a v`'s restore, a space switch that takes its layout out of the
frame, another buffer or window. The review in front is left again as oso quits or restarts
(`hand_review_saves`), and as `C-c a v` continues, so that a review shown where the key is
pressed, outside a layout, is the one laid out, as the old second row of ADR 0160's table did. A
review a restart opens again, or a step opens unseen, keeps the stamp it was saved with. Each leaving stamps the time, in milliseconds, and the id of the space in
front when the review was last seen there, as the spaces record names it: in memory at once,
and in the saved review's header at its next save, which a stamp alone makes due. The review
left last is the one of the greatest stamp, then opened last, over the reviews open, the one
closed last in this run, and those saved.

Stamping on each change of the selected window's review, rather than on each of the commands
the plan named, gives the same review for each of them, and needs no command to tell the host
which review it buried.

### Continue

`C-c a v` with no layout in front continues the review left last:

- **Its space first,** if that space is still open; otherwise it continues in the space in front,
  and says so.
- **Open:** if its layout is kept in that space's trees and in front, the review is only selected
  again, where it was, out of a zoom made inside the layout first; else it is shown as an open
  shows it, in a layout made for it, its cursor and the rows above it put back.
- **Closed, or from before a quit:** it opens from its saved file as `review-resume` opens it,
  only in a project oso knows and trusts; else the same message as `review-resume`, and nothing
  runs. The cursor and the rows above it go back once the text shows them.
- **A commit stepped to from a pull request** ([ADR 0167](0167-reviewing-a-pull-request-a-commit-at-a-time.md))
  is its own review, so it continues as that commit. If its open fails, as when a force-push
  took the commit, the pull request's review opens instead: it is the person's own open, so
  asking `gh` is allowed (ADR 0122, step 12b).
- **Nothing left or saved:** the review menu opens. While the saved reviews are not listed yet,
  the first moments after a start, the open reviews decide, and with none the menu opens and
  the log says why.

### What comes back

The first step brings back the review (its root and scope), the cursor's row and the rows
above it (a new optional `above` in the saved place, and `left` and `space` in the header; a
file saved before reads as it did, with a few rows above), the view's choices step 6 keeps, and
the layout as a plain layout, the slot showing what `first_slot_buffer` chooses.

The second step keeps the layout's own state, in memory as the layout leaves the frame (`q`,
`C-c a v`'s restore, a space switch) and in the saved view as an optional `layout` (a file saved
before has none; one not read is left out alone, and a slot of a kind not known shows nothing):

- **The slot's width and the split below it,** each in thousandths of what it shares, put back
  held to a tenth from either edge; a width the windows refuse stays at two fifths. The split
  below is put back while a window is below the slot, which the next step's session gives.
- **Which side was selected:** the review or the slot.
- **What the slot showed:**
  - *A source file*, at its line, column and rows above, as far as the text goes now. Put back,
    it is judged again: its name, the paths held as secret in this run, and its open buffer. An
    open buffer goes in at once. One not open is opened unshown, as a plain visit with one name,
    as the agent channel opens a file, and goes in once it loaded, if it opens as no secret and
    the slot still shows what it was given meanwhile. A file gone says so, and no new file opens.
  - *A read-only side*, at its line: its buffer if it is still open, else asked of the review's
    thread as `M-RET` asks it, after a quit once that file's text shows, or once every file was
    read; what the jump refuses says why.
  - *The review message:* the message waiting, else made again from the comments not sent, as
    `review-send` makes it; what was typed in a message is not kept across its buffer's end.
  - *Nothing*, else: the slot shows what `first_slot_buffer` chooses.
  - The store's thread judges the slot's file again as it writes, as it judges the review's.

The third step keeps the session below the slot
([ADR 0169](0169-the-review-slot-keeps-the-source-above-the-session.md)), by its record name,
as an optional `session` in the saved layout: the session the window below the slot shows,
else the one `review-ask` showed in the slot itself, never one whose buffer may hold secrets or
is withheld from agents. Put back, after what the slot showed:

- **Live and hosted:** its buffer goes below the slot, in the window there or one split off,
  else in the slot too small to split, and the split below goes back to its share.
- **Ended,** or no longer hosted: the message "session <name> ended: M-x session-resume", and
  nothing is spawned. One still starting, or whose buffer may hold secrets now, is left out
  and nothing is said.

**A restart** carries the layout's windows, their sizes, the selection and the session below
the slot (ADR 0160, Q9; ADR 0169). Once the review's text shows, the slot is given back what
its saved file kept it showing only where the windows carried lack it, as a read-only side or
the review message, which no window carries; the review's own cursor and rows above go back as
for any saved review. Quitting and restarting stamp the review in front, as "Left" says.

**Only under the review's root, from a saved file.** A file the slot showed, as a saved
review's layout names it, after a quit, `C-x k` or a restart, is put back only if its path lies
under the review's root, judged by its words, below the root and with no `..`, as the review's
own files are joined to it; else nothing opens, the slot shows what `first_slot_buffer`
chooses, and the echo area says "The file the slot showed is outside the review's root, so it
is not shown again". A layout kept in memory, made by the person in this run, puts back any
file it showed, after the same secret checks.

**Never kept or shown:** a buffer that may hold secrets, is withheld from agents, or is among the
paths held as secret while a review was open. A review buffer that may hold secrets gives no
place. On a later put-back of a slot's file, the secret name check runs again before the visit.

### The key's table

[ADR 0160](0160-the-review-layout.md)'s "One key (Q1)" table is replaced:

| Where | What `C-c a v` does |
|---|---|
| with a prefix argument | opens the review menu, at once as the command ends (as before) |
| in the review, its panes or its files pane, while laid out | restores the windows from before the layout; the review is left |
| in the slot, or any window of the layout but the review's | selects the review again where the person left it (as before) |
| anywhere else, a review left or saved | continues the review left last: its space first, then the review as it was left, from memory or from its saved file |
| anywhere else, no review ever left or saved | opens the review menu |

Its old second row, which laid out the review the selected window showed last, goes: opening a
review is a leaving, so a review open but never left is still continued.

- **A keyboard macro.** With `C-u` the menu still opens at once, as the command ends, so a
  macro's next key goes to it. The menu for "nothing left or saved" is opened by the host after
  the command, as the host already opened it in a space with an orchestrator, so a macro's next
  key there may come before it.

### What stays

- The tree from before the layout is not carried across processes (ADR 0160, Q9). After a
  continue in a new process, `q` leaves the review's window alone.
- An agent reaches none of this: `review-layout` is `Person`, and an agent's `q` asks for no
  restore.
- Beneath a prompt the key says so and changes nothing; a saved file that cannot be read says so
  as before, and `review-forget` removes it.

## Changes to accepted ADRs

- [ADR 0122](0122-agent-review.md), step 6: the header gains `left` and `space`, the saved place
  `above`, optional keys at the same version; a stamp of leaving alone is saved.
- [ADR 0160](0160-the-review-layout.md): the key is `C-c a v` and its table is the one above;
  `q` keeps the review continued as it was, from memory.
- [ADR 0167](0167-reviewing-a-pull-request-a-commit-at-a-time.md): the review continued is the
  commit stepped to; a commit gone opens the pull request's review.
- [ADR 0169](0169-the-review-slot-keeps-the-source-above-the-session.md): the session below the
  slot is kept by its record name; an ended one is only named.
- ADRs 0117, 0122, 0127, 0132, 0136, 0143, 0160, 0162 and 0163: the key's name only.

## Consequences

- One key leaves and continues a review, in any space and across a quit.
- Each event looks at the selected window's review; the place is taken again only once its
  cursor, view or text moved, at O(log of rows).
- Saved reviews are written once more as they are left, which a write of the same bytes skips.
- Nothing runs git as oso starts: continuing a saved review checks trust first, as
  `review-resume` does, and the person's key is what opens it.

## Alternatives considered

- **A new command, or a new menu head, for continue.** Rejected by the author: one key does
  everything.
- **Keep the old key, `C-x v` then `r`, beside `C-c a v`.** Rejected: two keys for one command, and `C-x v` is the
  source-control prefix.
- **Stamp only on the named commands.** It needs `q` to tell the host which review it buried,
  a change of `ReviewRequest::Quit`, a public item; the selected window's change covers them.
