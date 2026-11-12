# 0119. Undoing large steps in slices

- Status: Accepted
- Date: 2026-11-12

## Context

One `C-z` can block the UI thread for a long time. [ADR 0113](0113-query-replace-keyboard-macros-and-whitespace-cleanup.md)
recorded it as an exception the author accepted on 2026-11-12: undoing a large query-replace `!`
"applies every slice's batch on the UI thread, at once: about 200 ms over 100 MB with 100,000
sparse matches, 590 ms over 4 million short lines, and 1.7 s over 2 million matches on one 6 MB
line". The author asked for a design that undoes every large step in slices, not only `!`'s: a
keyboard macro's replay, whitespace cleanup on save, `indent-region`
([ADR 0115](0115-indentation.md)), and anything else that makes a large history step.

### How the history works today

- **Undo is linear,** one step per command across all cursors; an edit after an undo discards the
  redo groups ([ADR 0030](0030-m2-editing-commands.md), `oso_core::undo`). A `Group` holds the
  batches that go back, last first, and the two `Side`s it joins: a `ContentId` and, if the
  selected window showed the buffer, its cursors. Crossing a group applies its batches and yields
  the group that crosses back, so undo and redo are one operation.
- **An edit, an undo or a redo is staged on clones, then committed** by moves that cannot panic
  ([ADR 0053](0053-edit-core.md)). `Buffer::stage_crossing` clones the rope and the display map,
  both O(1) to clone and `Arc`-shared, applies the whole group there (`apply_group`), and
  `Editor::cross` maps every window's cursors and views through the steps, then commits. Nothing
  reaches the buffer until the commit.
- **A batch is applied in descending offset order** (`apply_marking`), so each splice leaves the
  offsets below it valid. A batch can therefore be cut anywhere into a suffix applied first and a
  prefix applied after, and both stay exact.
- **Removed text is kept as rope slices,** so a kill copies nothing. But putting it back copies it:
  `Option<Rope>::replace_in` turns the slice into a `String` and the display map measures it again,
  about 0.7 ms per MB (63 ms to undo an 89 MB kill), listed in
  [open questions](../open-questions.md). One splice can be as costly as a million small ones.
- **What a crossing costs grows with the step:** the splices, the bytes they put back, the ranges
  where clusters may have changed (`apply_group` merges them again after every batch, so this part
  may grow faster than the step), other windows' cursors and views mapped batch by batch, the
  whitespace cleanup's edited ranges (`Edited::follow`, once per batch) and the soft wraps' splices.
  Which of these makes the 1.7 s is not measured yet.
- **The history is bounded at 64 MiB per buffer, not counting the newest group,** which always
  stays whatever its size. Redo groups go first, then the oldest undo groups.
- **`History::join_since` joins the groups made since a mark into one** (ADR 0113): `!`'s slices,
  a macro's replay, and query-replace's answers. It keeps every batch as it was, so a joined step
  holds many batches of at most 1000 splices. When the limit dropped the oldest of them, what is
  left is joined whole.
- **Steps that are large today:** `!` (a slice of at most 1000 matches a turn, joined at the end),
  a macro's replay (each command's step, joined), whitespace cleanup on save (one batch over the
  edited lines), `delete-trailing-whitespace` (one batch over the buffer), `indent-region` (one
  batched edit over up to about 4 MiB, each changed line a splice, so about 100,000 splices in
  one batch), typing at up to 1000 cursors (one batch of 1000, joined up to 20 times), a large
  kill or yank (one splice of many bytes), and undoing an emergency copy's recovery (ADR 0064,
  the whole text).
- **What waits for long work today:** `!` holds keys, up to 256, and runs them once it ends;
  `C-g` stops it and drops them (`dispatch/hold_all.rs`). A macro replay waits while `!`'s slices
  run. The agent channel waits while isearch, a query-replace session or a macro is on. A reload
  waits while a prompt, isearch or a session is open (`Editor::reload_waits`,
  [ADR 0083](0083-reloading-files-changed-on-disk.md)). A dev restart waits for those and for a
  macro ([ADR 0090](0090-oso-dev-carries-unsaved-text.md)). The restart handoff carries unsaved
  text but no undo history: after it, undo starts afresh.
- **Saves and the emergency copy read O(1) snapshots.** A save marks saved the content id of its
  snapshot, and exit and a restart are refused while one runs
  ([ADR 0059](0059-safe-save-and-the-exit-guard.md)). The emergency publisher takes a snapshot
  after each command that did not panic, and after each slice of `!`
  ([ADR 0062](0062-emergency-save-and-the-panic-hook.md)).
- **`undo` and `undo-redo` are `Change`** ([ADR 0085](0085-agent-channel-and-oso-mcp.md)), so the
  agent channel can undo a large step. They take a count, crossing up to 256 groups one at a time.
- **Highlights follow the splice log** ([ADR 0111](0111-tree-sitter-highlighting.md)): each
  revision's splices go in a log of at most 64 revisions and 4096 splices, so a report from the
  syntax worker can be mapped to now, and an undo's batches are composed into one. A batch past
  4096 splices empties the log, and the highlights still follow it: one batch of 100,000 splices
  costs 57 to 66 ms there alone.
- **Soft wraps** rewrap a large edit as a width change: the rows in view at once, the rest on the
  wrap worker, which covers undoing a large kill ([ADR 0100](0100-soft-wrap.md),
  [ADR 0102](0102-soft-wrap-what-building-it-changed.md)).
- **Budgets:** a key press reaches the screen within 8 ms at p99
  ([ADR 0025](0025-latency-measurement.md)), of which the edit's share is 4 ms
  ([ADR 0091](0091-multiple-cursor-keys-and-m-y.md)); undo at 1000 cursors keeps it, 3.0 ms on
  100 MB ([ADR 0092](0092-edits-in-place-for-many-cursors.md)). A motion stops at 4 MiB, about
  50 ms, "under the 100 ms that counts as a freeze", and undo crosses at most 256 steps
  ([ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md)). Work that goes on over
  turns stops each slice at 3 ms, as `!` does: `App::before_waiting` calls `replace_slice`, which
  asks for a redraw, and the redraw brings the next turn.

### What other editors do

- **Emacs** undoes synchronously: `primitive-undo` pops `buffer-undo-list` entries in a plain loop
  until the next boundary, so undoing a `replace-regexp` over a large buffer freezes it until done.
  `undo-amalgamate-change-group` only removes boundaries, making one larger step, as
  `History::join_since` does. When one command's record passes `undo-outer-limit` (24,000,000
  bytes), `undo-outer-limit-truncate` discards the buffer's whole undo list with a warning
  (`undo-limit` and `undo-strong-limit` are 160,000 and 240,000)
  ([`lisp/simple.el`](https://github.com/emacs-mirror/emacs/blob/master/lisp/simple.el),
  [`src/undo.c`](https://github.com/emacs-mirror/emacs/blob/master/src/undo.c)).
- **VS Code** compresses each element's text changes; `SingleModelEditStackElement.undo()` hands
  them to the model's `_applyUndo` as one synchronous batch on the renderer thread. The undo
  service is asynchronous only for confirmation across files. No size limit
  ([`editStack.ts`](https://github.com/microsoft/vscode/blob/main/src/vs/editor/common/model/editStack.ts),
  [`textModel.ts`](https://github.com/microsoft/vscode/blob/main/src/vs/editor/common/model/textModel.ts),
  [`undoRedoService.ts`](https://github.com/microsoft/vscode/blob/main/src/vs/platform/undoRedo/common/undoRedoService.ts)).
- **Zed** keeps transactions of edit ids; undo bumps counts in an `UndoMap` and applies an
  `UndoOperation` to the CRDT, flipping fragments visible or hidden, synchronously on the
  foreground thread in time proportional to the fragments touched
  ([`crates/text/src/text.rs`](https://github.com/zed-industries/zed/blob/main/crates/text/src/text.rs),
  [`undo_map.rs`](https://github.com/zed-industries/zed/blob/main/crates/text/src/undo_map.rs)).
- **Helix** keeps a revision tree; each revision stores its transaction and the inversion built at
  commit, and undo applies it synchronously, with no size limit
  ([`helix-core/src/history.rs`](https://github.com/helix-editor/helix/blob/master/helix-core/src/history.rs),
  [`helix-view/src/document.rs`](https://github.com/helix-editor/helix/blob/master/helix-view/src/document.rs)).
- **Vim** bounds the history by `'undolevels'` (1000) and swaps line ranges back in one pass
  (`u_undoredo`); it can interrupt saving a large change for undo, not undoing it
  ([`src/undo.c`](https://github.com/vim/vim/blob/master/src/undo.c)).

None of them slices an undo; they keep it fast enough by compact records, or bound it by
dropping history. oso's rule that the UI thread never blocks has no precedent to copy
here, but the edit core's stage-then-commit already gives what slicing needs: a crossing that
touches nothing until its last move.

## Decision

### A crossing is a value that goes on (Q1, Q2, Q3)

- **Every undo and redo is a resumable crossing.** `Buffer::stage_crossing` becomes a value that
  holds the clones, the batches not yet applied, the place inside the current batch and splice,
  the inverse built so far and each window's cursors and views mapped so far. `go_on(budget)`
  applies work until the budget is spent or the group is crossed.
- **No threshold (Q1).** The command runs the first slice at once. A crossing that ends within it
  commits in the same command, exactly as today, so small undos do not change at all. One that
  does not end goes on at the next turns of the loop. The budget is 3 ms of time, as `!`'s slice
  has, and a count of work: each splice costs a fixed amount plus the bytes it puts back, so a test
  can set a small count and see the same slices every run.
- **A slice cuts anywhere (Q2):** between batches; inside a batch, between splices, since a batch
  applies from its last splice back; and inside one splice's text, putting back at most 1 MiB of
  it a slice at increasing offsets. A batch cut in two becomes two batches in the inverse group,
  which crosses back the same; a splice cut in pieces stays one splice in the inverse.
- **On the UI thread, one slice a turn (Q3),** as `!` replaces. No thread, no channel, and the
  `Editor` keeps the crossing under way, as it keeps a query-replace session; the host asks it to
  go on at each turn and redraws the echo row. The crossing holds only `Arc`-shared values, so a
  later change can hand it to a worker without changing what it is.

### Atomic, and seen as before until it commits (Q4)

- **The buffer is not touched until the crossing ends.** Its text, content id, revision, history,
  highlights and edited ranges stay as they were, and every window draws the text as it was. The
  commit at the end is the moves `commit_crossing` makes today, with the cursors and views already
  mapped, so it cannot leave the buffer half undone.
- So a save, the emergency save after a panic, a crash, a dev restart's handoff, `get-state` and
  the syntax worker all see the state before the undo, which is a state the buffer really had.
- **The staged crossing remembers the buffer's revision** and drops itself, saying "Undo
  abandoned", if the buffer changed under it some other way; the rules below leave no such way,
  so this is a guard.
- **A panic in a slice** drops the crossing and nothing else; the buffer is as before.
- **A save already running when the undo starts** writes its snapshot and marks that content id
  saved, so the buffer after the undo is modified, as when an undo follows a save today.

### What the person sees and may do (Q5, Q6, Q7, Q8)

- **The echo row says `Undoing… 42%`** (or `Redoing…`) from the second turn, by the work count, so
  an undo that takes one or two slices shows nothing (Q8).
- **Keys are held, as for `!` (Q5),** in every window, and run in order once the crossing ends. A
  held key goes into a macro's recording when it runs, once, as ADR 0113 has it. Mouse clicks and
  wheel events are held with them. A replay waits while a crossing goes on.
- **`C-g` cancels it (Q6):** the clones are dropped, the history stays as it was, and the echo row
  says "Undo cancelled". The buffer never changed, so nothing is half undone. The keys held before
  the `C-g` are dropped, as `!`'s are.
- **A count (Q7):** `C-u 5 C-z` crosses groups one after another, each committed when it ends, so
  the buffer is always at a state of the history; `C-g` cancels only the one under way and drops
  the rest of the count. `C-z` held down queues as any held key does, up to the hold's limit.
- **Saving the buffer mid-undo** is a held key, so `C-x C-s` saves the text after the undo. The
  agent's save waits, as below.
- **The window's close button and `C-x C-c`** wait for the crossing like keys; the exit guard
  then asks as usual. A quit the system forces sees the state before the undo.
- **A reload of the file waits** until the crossing ends, as it waits for isearch.

### The agent channel, restarts and highlights (Q9, Q10, Q11)

- **The channel's `undo` and `undo-redo` run as the person's do (Q9),** sliced, and the reply
  comes when the crossing ends or is cancelled, saying which. Other channel commands wait while a
  crossing goes on, as they wait for a macro. The person's `C-g` cancels the agent's undo too, so
  holding keys never takes the keyboard from the person.
- **`get-state` gains `undoing`:** `null`, or `{"redo": false, "done": 0.42}`, and only
  `{"active": true}` around a buffer that may hold secrets, as ADR 0113 hides `replace`.
- **A dev restart waits for the crossing to end (Q10).** Cancelling would be safe but the handoff
  carries no history, so the person could never make that undo after the restart.
- **Highlights get the whole crossing as one revision at the commit (Q11).** The crossing maps
  its own copy of the highlights slice by slice, as it maps cursors, so the commit only moves it
  in; past the log's 4096 splices the log is emptied as for any large batch, and the worker parses
  the new text. Reports for the old revision that come mid-undo are taken as usual, since the buffer
  has not changed, and the crossing's copy takes them too.
- **Soft wraps** take a large crossing as a large edit: the rows in view at the commit, the rest
  on the wrap worker (ADR 0100).

### Cursors and redo (Q12)

- **The selected window gets the cursors of the side it returns to,** as today, and every other
  window showing the buffer keeps its cursors mapped through the steps, slice by slice, so the
  commit maps nothing. Keys are held, so the selected window cannot change meanwhile.
- **Redo is the same crossing the other way (Q12).** The inverse built while undoing is the redo
  group, with the cuts the undo made; redoing slices it the same way. No forward record is kept
  beside it.

### Memory (Q13)

- **The group stays whole until the commit,** so that `C-g` costs nothing; the inverse is built
  beside it. The peak is about twice the step's record, plus the chunks the clone rebuilt, and lasts
  only while the crossing goes on. The step is the newest group, which the 64 MiB limit already lets
  stay whatever its size.
- **What the commit lets go is freed off the UI thread,** on the thread that frees a killed
  buffer's trees ([ADR 0088](0088-switching-and-killing-buffers.md)): the group crossed, the old
  rope and display map. Dropping an 89 MB group takes about 2.5 ms today (ADR 0053), and a group
  of millions of rope slices more.

### Steps that slice well (Q14)

- **Nothing is required of a producer:** the crossing cuts anywhere, so `!`, a replay, whitespace
  cleanup and `indent-region` keep making their steps as now. `join_since` keeps the batches as
  they were and merges none.
- **A group keeps its counts:** its splices and the bytes they put back, summed as batches are
  recorded and joined, so the first slice knows the whole and the echo row its share.
- **The forward pass is each producer's own business:** `!` already slices it, `indent-region` is
  bounded at about 50 ms by ADR 0115, and a cleanup larger than 256 KiB scans on a worker
  (ADR 0113).

### When (Q15)

- Now in M3, in the three steps below, the first of which changes no behaviour.

### Where it lives

- `oso_core::undo` gains the counts. `buffer/edit.rs` turns `StagedCrossing` into the resumable
  crossing, and `buffer/splice.rs` gains a resumable `apply_group`, which merges the changed ranges
  once per slice, not once per batch. `editor/undo_group.rs` keeps the crossing under way, with
  `Editor::go_on_crossing` and `Editor::cancel_crossing`. All of it is private to the crate except
  those two and `Editor::crossing` for the host, which step 2 asks the author for.
- `crates/oso` holds the keys, cancels on `C-g`, asks for the next slice each turn as for `!`,
  waits the channel, the replay, the reload and the restart, and adds `undoing` to `get-state`.

## Questions

Put to the author on 2026-11-12, the recommended option first; each answer was the first, (a).

### When and how it slices

1. **What makes an undo large.**
   (a) Nothing decided ahead: every crossing runs its first slice at once, by a 3 ms budget and a
   count of work (splices and bytes), and only one that does not end there goes on. Small undos do
   not change; tests set the count.
   (b) A fixed size read from the group before it starts (say 4096 splices or 4 MiB put back). One
   path or the other is known ahead, but a group under it on a long line can still pass the budget
   (1000 splices take 5.2 ms on a 6 MB line).
   (c) The time the forward pass took, kept with the group. Follows the machine, but the forward
   and backward passes cost differently, and a joined step was made over many turns.
2. **Where a slice may cut.**
   (a) Anywhere: between batches, between the splices of a batch, and inside one splice's text at
   1 MiB pieces. Every step slices, `indent-region`'s single batch and a large kill's single splice
   too.
   (b) Between batches only, and producers keep batches at 1000 splices. Simpler, but
   `indent-region` and the cleanup must split theirs, and a large kill's undo stays one slice.
   (c) As (b), and first make putting back a rope slice O(log n) by sharing its chunks (the open
   question). Fixes the kill's copy, but the display map still measures the text, and `rope.rs` is
   at its size limit.
3. **Where it runs.**
   (a) On the UI thread, a slice a turn, on clones, as `!` does; one commit at the end. One path,
   the one every undo already takes, and deterministic tests.
   (b) On a worker over the O(1) snapshot, committed when it reports. The UI thread is free for the
   whole time, and the total is shorter by the frames between slices, but it is a second path for
   cursors, views, wraps and highlights, and a new worker.
   (c) The first slice on the UI thread, the rest on a worker. The cost of both.

### What the person sees and may do

4. **What the buffer shows meanwhile.**
   (a) The state before the undo, until one commit at the end. Atomic: a save, a crash, a restart
   and the channel only ever see a state the history has, and `C-g` costs nothing.
   (b) The undo as it goes, each slice committed. The person sees it move, but the buffer is half
   undone between slices, on disk if saved and in an emergency copy, and `C-g` leaves it so.
5. **Keys and the mouse meanwhile.**
   (a) Held in every window and run in order after, as for `!`; clicks and the wheel too.
   (b) Keys for other buffers run; only this buffer's are held. Typing goes on elsewhere, but a key
   that switches buffer, or a command that edits several, needs a rule each.
   (c) As (a), but the wheel scrolls the text as it was meanwhile.
6. **`C-g`.**
   (a) Cancels: nothing undone, the history as it was, "Undo cancelled".
   (b) Stops where it is and keeps what was undone, as `C-g` keeps what `!` replaced. Only
   possible with 4 (b).
   (c) Ignored until the undo ends. Simple, but the person cannot stop a 2 s wait.
7. **A count and a held `C-z`.**
   (a) Each group commits as it ends; `C-g` cancels the one under way and drops the rest of the
   count; a held `C-z` queues as other held keys do.
   (b) The whole count is one crossing, committed once. Atomic over the count, but it builds every
   group's inverse before any commits.
8. **What it says.**
   (a) `Undoing… 42%` (or `Redoing…`) in the echo row from the second turn, as `!` says
   `Replacing…`.
   (b) Nothing; keys are held silently.
   (c) A mark in the modeline.

### The agent channel and restarts

9. **The channel's `undo`.**
   (a) Runs as the person's, sliced; the reply waits for the end and says if it was cancelled.
   Other channel commands wait meanwhile, and `get-state` gains `undoing` (`{"active": true}` only
   for a buffer that may hold secrets).
   (b) Refused for a large step, with an error the agent can read. The channel never holds the
   person's keys, but an agent cannot undo its own large edit.
   (c) Replies at once that it started; the agent reads `get-state` until it ends. No reply waits,
   but every tool must poll.
10. **A dev restart mid-undo.**
    (a) Waits for the crossing to end, as for `!` and a macro; at most a few seconds.
    (b) Cancels it and carries the text as it was. Faster, but the handoff carries no history, so
    the undo is lost for good.

### Highlights, redo, memory and producers

11. **Highlights of a large crossing.**
    (a) Followed slice by slice on the crossing's own copy, so the commit only moves them in and
    the text keeps its colours. Step 1 measures what copying the highlights costs.
    (b) Dropped at the commit for a crossing past the log, and drawn plain until the worker
    reports. Nothing to follow, but a large undo flashes the buffer uncoloured.
12. **Redo.**
    (a) The same crossing the other way: the inverse built while undoing is the redo group, cut as
    the undo cut it.
    (b) Keep the step's forward record beside it, so redo applies it without building. Redo needs
    no building, but every step keeps its text twice.
13. **Memory while it runs.**
    (a) The group stays whole until the commit and the inverse is built beside it, about twice the
    step for the time it runs; what the commit lets go is freed off the UI thread.
    (b) Consume the group as the inverse grows. Half the peak, but `C-g` must then rebuild the group
    or cannot cancel.
    (c) As (a), but refuse to undo a step past a size, as Emacs's `undo-outer-limit` drops one.
    Bounded, but the person loses an undo they had.
14. **What producers must do.**
    (a) Nothing: the crossing cuts anywhere, `join_since` keeps batches, and the group counts its
    splices and bytes as they are recorded.
    (b) Keep batches at 1000 splices and texts at 1 MiB, each producer cutting its own (needed with
    2 (b)).

### When

15. **When it is built.**
    (a) Now in M3, in three steps: measure and remove the costs that grow faster than the step,
    then the resumable crossing in `oso-core`, then the host.
    (b) The measurement step now, the rest later in M3 when the queue is shorter.
    (c) After M3; ADR 0113's exception stands until then.

## Steps

Each step is its own PR. Step 1 touches only the history's internals in `oso-core`; steps 2 and 3
wait for the coordinator's go.

1. **Measure where the time goes,** with `text_perf` cases for the three `!` cases above, an
   `indent-region` over 4 MiB, a macro replay of 1000 commands, and undoing an 89 MB kill: per
   batch and per part (rope, display map, changed ranges, other windows, edited ranges,
   highlights, wraps).
   Remove what grows faster than the step, such as merging the changed ranges once per slice. No
   behaviour changes.
2. **The resumable crossing in `oso-core`,** with the counts, the cuts inside batches and splices,
   `go_on_crossing`, `cancel_crossing` and the commit; property tests that a crossing in slices of
   any size gives the same text, cursors, content id, history and redo group as one at once, and
   that cancelling at any point changes nothing. The public API it needs, `Editor::go_on_crossing`,
   `Editor::cancel_crossing` and `Editor::crossing`, is asked of the author with this step.
3. **The host:** holding keys, `C-g`, the echo row, the channel's wait and reply, `get-state`, the
   restart's wait, the reload's wait, the replay's wait, freeing off the UI thread; e2e for holding,
   `C-g` and the restart; `verify-in-app` on the 100 MB file.

## Changes to accepted ADRs

- [ADR 0113](0113-query-replace-keyboard-macros-and-whitespace-cleanup.md): the exception for
  undoing a large `!` ends once step 3 lands; its "As built" gains a pointer here.
- [ADR 0053](0053-edit-core.md): a crossing may be staged over several turns, still on clones and
  committed once.
- [ADR 0090](0090-oso-dev-carries-unsaved-text.md): a restart also waits for a crossing.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md): the channel also waits for a crossing, and
  `get-state` gains `undoing`.
- [ADR 0083](0083-reloading-files-changed-on-disk.md): `Editor::reload_waits` also waits for a
  crossing.
- [ADR 0053](0053-edit-core.md)'s "undo cannot return part of a group" stays true: a cancelled
  crossing returns none of it.

## Consequences

- No undo or redo blocks the UI thread past its slice, whatever made the step.
- A large undo still takes as long in all, a little longer for the frames between slices, and the
  person waits with keys held, as for `!`.
- The buffer is never half undone, so saving, crashing and restarting need no new rule.
- A large crossing holds about twice the step's memory while it runs.
- `Editor::undo` no longer always ends the crossing before it returns; callers that undo inside a
  command (a count, query-replace's `u`, a replay's `C-z`) must allow a crossing still going on.

## Alternatives considered

- **Undo in slices on the live buffer** (4 (b)): what Emacs does when `C-g` interrupts it. It
  shows progress in the text, but every save, crash and restart between slices would need a rule
  for a state the history does not have.
- **Keeping a whole-text state in each large group,** so that crossing it swaps the rope and the
  display map in O(1) (the open question for an emergency copy). Fast for one step, but each large
  step would keep a copy of the whole text's tree, the cursors and marks of other windows would
  lose their places, and the highlights' log could not follow.
- **Refusing to record a step past a size,** as Emacs's `undo-outer-limit`: bounded, but `C-z` after
  `!` would then do nothing.
- **A worker for the crossing** (3 (b)): kept possible, since the crossing is built from shared
  values; chosen against for one path now.
