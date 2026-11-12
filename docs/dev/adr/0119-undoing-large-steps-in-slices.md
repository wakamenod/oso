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
- **The staged crossing remembers the buffer's revision** and drops itself, saying "Undo or redo
  abandoned", if the buffer changed under it some other way; the rules below leave no such way, so
  this is a guard.
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
  has not changed, and the crossing takes them in at its commit, as if they came just after it
  (see "As built (step 2)").
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
  those two and `Editor::crossing` for the host, which step 2 asks the author for. As built, the
  files and the public items differ; see "As built (step 2)".
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

### As built (step 1, 2026-11-12)

- **`text_perf -- undo` measures undo and redo of large steps:** `!` over 4 million short lines,
  in a wrapped window and in two windows; `!` over a highlighted 2 MiB buffer; query-replace
  sessions answered `y` match by match, over 140 KB and far apart over 100 MB; and 20 commands
  joined at 1000 cursors. The three `!` cases of ADR 0113 stay in `-- replace`, and the 89 MB kill
  in the default run. The `indent-region` command is not on main yet; its case comes with it. A macro replay
  runs in the host, and makes steps like a session answered match by match. The figures for each
  part below came from timers added to the code for a run and taken out again; the part prints
  totals only.
- **Before this step, for the steps `!` makes, a crossing cost in proportion to its splices.**
  On main, the last batches of 1000 splices cost what the first did: about 1.2 ms on short lines,
  1.1 ms in a 2 MiB buffer and 3.1 ms on a 6 MB line, nearly all of it in the rope and the display
  map; merging the changed ranges took microseconds. Undoing `!` over 4 million lines took about
  530–590 ms, and redo about as long. Of that, about 540 ms applied the batches the 64 MiB limit
  kept (434 of 4000), 4 ms followed the edited lines and 15–20 ms was the commit, most of it
  freeing the group crossed. A wrap or a second window added under 4 ms. Highlights added about
  190 ms over 434,000 splices, 430 ns each.
- **A step of many small batches far apart grew faster than the step.** A session answered `y`
  match by match, or a macro replay that edits forward, makes one. Every batch mapped the changed
  ranges, and followed the edited lines, of all the batches before it. On main, undoing 5,000,
  10,000 and 20,000 answers took 48, 180 and 728 ms over 140 KB, and 145 ms, 533 ms and 2.2 s far
  apart over 100 MB: four times as long for twice the answers.
- **Batches that lie apart now apply as one** (`joined_apart` in `buffer/splice.rs`): a batch
  joins the ones before it when it lies wholly before them, or wholly after what they became, with
  a byte between. Undoing a session answered forward then applies one batch, and redoing it does
  too: 5, 11 and 22 ms over 140 KB, and 16, 27 and 47 ms over 100 MB, linear. `!`'s slices join
  the same way wherever a byte separates them, so undoing `!` applies one batch of 434,000
  splices; it costs as before (520–530 ms over 4 million lines, 681 ms highlighted), and 20
  commands joined at 1000 cursors take 29 ms, as before. The text, the content id and every offset
  come out as before. What differs is not visible: the changed ranges are those of the joined
  batch, the wrap splices come in its order, and the rope's chunks may be cut otherwise. The redo
  group holds the joined batches, and `apply_group` returns one step a batch it applied. A single batch is not copied; typing never joins, since one cursor's
  batches touch and many cursors' interleave. A property test checks that a group applied this
  way gives the text, moves offsets and edited lines as its batches one after another, also with
  splices that meet inside a batch; that a cluster start outside the changed ranges it returns is
  still one; and that what it gives back undoes it, and what that gives back redoes it. Another
  checks that batches each apart from the last join into one step, both ways.
- **Left for the next steps:**
  - Batches that neither lie before nor after the ones joined, such as a macro that edits two
    places moving the same way, still map each batch over the ranges before it; step 2 counts
    that work in its slices.
  - A run is joined into a copy of its splices before any applies, a few milliseconds for
    434,000; step 2 builds runs a slice at a time, or reads the batches in place.
  - Undoing the 89 MB kill takes 70 ms, copying its text back at 0.78 ms per MB. 20 commands
    joined at 1000 cursors take 29 ms. Both are sliced in step 2.
  - Freeing the group crossed, and the copy of a run, comes off the UI thread in step 3.

### Step 2 decisions

Put to the author with step 2 on 2026-11-12, the recommended option first; each answer was the
first, (a).

1. **What `undo` and `redo` do.**
   (a) `Editor::undo` and `Editor::redo` keep crossing one step whole, now running the slices back
   to back in one call, and `begin_crossing`, `crossing`, `go_on_crossing` and `cancel_crossing`
   come beside them. Step 2 changes no behaviour of the app; step 3 switches the commands.
   (b) `undo` and `redo` start a crossing and return while it goes on. Every caller changes in
   step 2.
2. **How the end is told.**
   (a) `go_on_crossing` returns `Option<Result<(), EditError>>`: `None` while it goes on, the
   result when it ended, with a new `EditError::Abandoned`.
   (b) An enum of its own for going on, done, cancelled and abandoned.
3. **Who writes the echo line.**
   (a) The editor, from the second slice: `Undoing… 42%`, `Redoing… 42%`, and with a count
   `Undoing… 42% (3 more)`, through `write_crossing_line` from `write_echo_line`, as `!` writes
   `Replacing…`. The host only redraws.
   (b) The host, from `Crossing::done`.
4. **What `done` is.**
   (a) An `f32` share of the current step's work, from 0 to 1.
   (b) Work done and work in all, as counts.
5. **query-replace's `u`.**
   (a) Keeps calling `undo`, crossing at once: its step is a single replacement.
   (b) Starts a crossing too.
6. **What the commit lets go.**
   (a) The commit hands the group crossed, the old rope and the old display map to a value private
   to the crate. Step 3 frees it on a background thread; step 2 drops it where it is dropped today.
   (b) Step 2 frees it off the UI thread already.

### As built (step 2, 2026-11-12)

- **The crossing is in three parts.** `buffer/sliced.rs` applies a group a slice at a time
  (`GroupApply`). `buffer/crossing.rs` holds the clones of the rope and the display map, the copies
  of the highlights and the edited lines it follows, and the commit (`StagedCrossing`).
  `editor/crossing.rs` keeps the crossing under way, follows every window's cursors and top on
  copies, commits each step, and writes the echo line (`Crossing`). `Editor::undo` and
  `Editor::redo` begin a crossing and run its slices back to back. The stage that applied a whole
  group at once, and `joined_apart`, are gone; step 1's tests now run through the sliced apply,
  at random slice and piece sizes.
- **A group counts each batch** (`undo::Counts`: its splices, the bytes they replace and put back,
  and its cost to the history), kept beside the batches as they are recorded and joined. A run is
  then planned from the counts and each batch's first and last splice, in O(batches): what lies
  apart, the order its batches apply in, and what the splices below each take. No run is copied,
  and a run that lies after needs no scan of its splices. The share done is the work applied over
  the group's, so it never goes back.
- **A slice stops after any unit of work:** a splice, a piece of at most 1 MiB of one splice's
  text, or the deletion before the pieces. Each slice applies at least one. A splice longer than a
  piece has its range deleted once and its text put back at increasing offsets; the batch that
  undoes it holds it as one splice, and cursors follow it once its last piece is in.
- **The redo group is the one crossing at once gives.** Each run gives back one batch, however
  the slices cut it, so a crossing builds the same group in any slices. This replaces Q2's batch
  cut in two that becomes two batches, and Q12's redo group with the undo's cuts.
- **What cursors follow is cut only where a byte lies between splices, or two replacements
  meet.** The property tests found that an insertion that meets a splice inside a batch, as one at
  the start of a replacement, can move a cursor otherwise when followed in two parts than in one.
  So a part of a run waits for the next slice when it ends at such a meeting; between batches the
  runs already need a byte. The highlights and the changed ranges of earlier runs follow the same
  parts. The edited lines follow each run once, when it ends, as at once: following them reads
  every edited range, which after a save is one a line `!` changed, so following them every slice
  cost the square of the step.
- **The changed ranges** (where a cursor may no longer be at a cluster's start, ADR 0091) are
  worked out splice by splice in the offsets after the run: where a splice's rebuilt chunks start
  is put in those offsets when the splice below it applies, or at that splice's start if it falls
  inside it, which holds as much or more. A run's ranges join the others when it ends. They may be
  wider than at once; clipping a cursor more is clipping it to where it already is.
- **The budget:** a splice costs 1024 units and the bytes it puts back, and a slice does at most 4
  MiB of them or applies for 3 ms. Highlights cost about 430 ns a splice to follow after the slice
  has applied, so a slice applies for 3 ms less however long the one before went on past its time,
  but at least 1 ms. A slice also counts the following it does: each part handed out costs its
  windows' cursors, the highlights' batch and the earlier runs' changed ranges, and each run that
  ends costs the edited lines, 2 units each. A slice stops after a run when what is left cannot pay
  for the next. The selected window's cursors are not followed when the step restores its own; only
  its ring is. Tests set the work and the piece size with `Editor::set_crossing_slices` and no time,
  so that the slices are the same every run.
- **What may change meanwhile, and what abandons it.** Each slice checks the buffer's revision, its
  history's count of changes (`History::changes`, bumped by every record, join, crossing and clear),
  that the buffer can still be edited and still wraps or not as it did, that the window it began in
  is still selected and that the same windows show the buffer. The editor's commit checks that no
  window's cursors changed, only then, once every slice has applied, as nothing but a command
  changes them and the host holds commands meanwhile; and the buffer's checks all this again and
  commits nothing that has not all applied. A save taking the edited lines, or a scope tracked for
  cleanup or query-replace, also abandons it. What the host may do between turns is taken in at the
  commit instead: a save that ended (what it took is forgotten, or edited again if it failed), a
  cleanup scan that ended, and highlights a report brought or that were dropped, which are mapped
  through the slices' batch; past the log's 4096 splices the crossing's own copy stays, as when the
  report comes after the commit. Either way the highlights of text the step put back are a guess, as
  after any edit, and may differ from those of a report taken before the undo. A window that
  scrolled meanwhile, by a resize or a wrap report, gets the top that followed the crossing.
- **A panic in a slice** drops the crossing and nothing else: the editor holds no crossing while a
  slice runs. No test makes a slice panic, as nothing but a bug can.
- **`EditError::Abandoned` says "Undo or redo abandoned",** as it may be either.
- **`begin_crossing` drops a crossing under way once its first step can begin;** a count of 0, or an
  error, leaves it. `Editor::undo` and `redo` begin one too. `go_on_crossing` with none under way
  returns `Some(Ok(()))`. A step it commits outside a command leaves the wraps' budget and following
  the cursors to the host, as the dispatcher does them after a command; doing them in the editor
  made the top depend on how the step was sliced. It and `!` share `SLICE_TIME`. An error at a later
  step met in the first slice comes back from `begin_crossing`, after the steps before it.
  `Crossing` has a `Debug` that prints only `redo`, `done` and `left`, which the API snapshot shows
  too.
- **Tests:** a property test crosses random histories (two windows, several cursors, marks, soft
  wrap, highlights, joined steps, counts both ways) in slices of random work and pieces of 1 to 23
  bytes, and checks text, content id, revision, every window's cursors, marks, ring, top and
  settled rows, every group of the history, the highlights and the edited lines against crossing at
  once. Another cancels at a random slice and checks that nothing changed. Unit tests cover a kill
  put back in pieces over turns, a count that meets `NoUndo` at a later step, `Abandoned` after an
  edit and after another window is selected, the echo line, and a report and a save that end
  meanwhile.
- **Measured** with `text_perf -- undo`, release build. Each slice is the time
  `begin_crossing` or `go_on_crossing` took; the last one commits.

  | Case | At once, undo / redo | Slices | p50 | p99 | Last | Whole |
  |---|---|---|---|---|---|---|
  | `!` over 4M short lines | 524 / 506 ms | 168 | 3.00 ms | 3.01 ms | 15.8 ms | 517 ms |
  | the same, wrapped | 529 / 512 ms | 167 | 3.00 ms | 3.02 ms | 23.5 ms | 522 ms |
  | the same, two windows | 518 / 503 ms | 165 | 3.00 ms | 3.01 ms | 18.6 ms | 511 ms |
  | `!` over 2 MiB, highlighted | 679 / 677 ms | 219 | 3.00 ms | 3.11 ms | 19.0 ms | 674 ms |
  | 20,000 answers far apart in 100 MB | 48 / 45 ms | 14 | 3.00 ms | 3.06 ms | 5.8 ms | 45 ms |
  | 20 commands joined at 1000 cursors | 29 / 28 ms | 10 | 3.00 ms | 3.01 ms | 2.7 ms | 30 ms |
  | an 89 MB kill | 61 ms | 20 | 2.92 ms | 2.99 ms | 2.2 ms | 55 ms |
  | 2000 runs that meet, 200k edited | 587 / 606 ms | 200 | 3.20 ms | 3.34 ms | 3.4 ms | 620 ms |
  | the same, 100k cursors in a window | 840 / 841 ms | 334 | 2.42 ms | 2.71 ms | 1.2 ms | 842 ms |

  Runs vary by about 10% with other work on the machine. Undoing at once costs what it did after
  step 1, and typing and undoing at 1000 cursors costs what
  it does on main (p99 3.3 ms over 100 MB). Every slice but the last stays within 3 ms and one
  unit. The last is long because it frees the group crossed: with the free left out, it took 1.3 to
  1.7 ms for `!` over 4M lines.
- **Left for step 3:**
  - Freeing what the commit lets go (`buffer::Freed`: the group crossed, the old text, display
    map and highlights, and the wrap splices) off the UI thread; it is dropped in the commit for
    now. A group the history's limit evicts at the commit, and the windows' old wraps, are still
    freed there too. So is a crossing cancelled or abandoned late in a large step, which holds
    nearly a whole group: `C-g` must hand it to the same thread.
  - What a slice does that the work count leaves out, which only the time bounds: planning a run
    reads each batch that joins it, all at once (microseconds for 20,000 batches); a step of many
    runs that do not lie apart follows every earlier run's changed ranges a part at a time; and a
    run that ends follows the edited lines once over all its splices.
  - The host: holding keys and clicks, `C-g`, asking for a slice each turn and redrawing, the
    channel's wait and reply, `get-state`'s `undoing`, the waits of a restart, a reload and a
    replay, and switching the `undo` and `undo-redo` commands to `begin_crossing`.
  - The commit's wrap edit and the clip read every splice's wrap edit and every window's cursors
    once; both are inside the 1.3 to 1.7 ms above.

### Step 3 decisions

Put to the author with step 3's plan on 2026-11-12, the recommended option first; each answer was
the first, (a).

1. **The wraps' budget before a slice outside a command.** (a) `Editor::refill_wraps` becomes
   public, and the host calls it before each slice and `Editor::follow_cursor` after, as the
   dispatcher does around a command. (b) The host opens and closes an undo group around each
   slice. (c) A public wrapper doing all three.
2. **What frees what a crossing lets go.** (a) `Editor::take_crossing_freed`, giving
   `Option<Box<dyn Send>>` with everything since the last call. (b) `Editor::take_freed` gives
   killed buffers and crossings together, changing its signature. (c) A new public type.
3. **The wheel meanwhile.** (a) Summed for each window and scrolled once the keys held with it
   have run, while `!` replaces too. (b) Dropped. (c) Held event by event with the keys.
4. **"Count too large" when the steps cross after the command.** (a) Said at once, and the
   progress line comes first in the echo row while the crossing goes on, so the message shows
   once it ends. (b) Said at once, hiding the progress. (c) A count over 256 is refused before
   any step crosses.
5. **What `C-g` says.** (a) "Undo cancelled" or "Redo cancelled". (b) "Undo cancelled" either way.
6. **The agent's reply when the person cancels.** (a) The error "Undo cancelled by the person"
   (or "Redo"). (b) A result with `"cancelled": true`.
7. **A batch too large to follow in one slice.** (a) The history records a batch of more than
   4096 splices as several of at most 4096. (b) Following such a batch over later slices. (c)
   Leaving it bounded by what the commands make today.

### As built (step 3, 2026-11-12)

- **The commands.** `undo` and `undo-redo` call `Editor::begin_crossing` with a count of at most
  256, and say "Count too large" at once past it (decision 4). They ask for a frame only if a step
  crossed or one goes on, as before. query-replace's `u` still undoes at once.
- **Each turn.** `app/slices.rs` runs, before the loop waits, `!`'s slice, then the undo's, then
  a replay's. Through `Dispatcher::go_on_crossing` the undo's gives the wraps their budget, runs
  a slice, makes the windows follow their cursors and asks for a frame; the emergency copy then
  reads the buffer. When the crossing ends, an error is said and stops a keyboard macro's
  replay, as a failing command does, and the input held meanwhile runs, then the wheel's.
- **A hidden window draws no frame,** and the redraw that would bring the next turn waits
  with it, so a step undone behind other windows went on only when something else woke the
  loop; `!` and a replay did the same. While any of them goes on, the loop now also wakes 16 ms
  after a turn, about a frame at 60 Hz, so a visible window keeps its pace.
- **A panic in a later slice** is caught there, inside the emergency publisher's command scope,
  as `Registry::execute` catches a command's: the editor holds no crossing while a slice runs,
  so it drops the crossing and nothing else, and the echo area says "Undo or redo panicked".
- **Held input** (`dispatch/hold_all.rs`) is held while `!` replaces or a crossing goes on, in one
  queue of at most 256. `C-g` cancels the crossing, says "Undo cancelled" or "Redo cancelled",
  and drops the queue and the wheel held with it. The window's close button is held with the
  keys, past the 256 too, and asks its question after them; so does it now while `!` replaces.
  The wheel is summed for each window and scrolls after the keys, while `!` replaces too; a
  window gone meanwhile scrolls nothing.
- **The echo row** shows the progress line first while a crossing goes on; what a command said
  shows once it ends.
- **What waits.** The channel refuses `run`, `open` and `insert` with "an undo or a redo goes on;
  try again in a moment". A restart, and an `exit` with a generation, are refused with "cannot
  hand over: an undo or a redo is under way", which `oso dev` waits out as it does a macro.
  `Editor::reload_waits` holds a reload, and a replay waits.
- **`get-state`'s `undoing`,** in the channel's state and the dev socket's: `null`,
  `{"redo": false, "done": 0.42}`, or `{"active": true}` while a buffer that may hold secrets is
  shown or is the one undone.
- **The channel's own undo.** If the command leaves a crossing under way, the reply waits
  (`app/agent/undoing.rs`): once it ends, the command's own reply, or the error that ended it;
  "Undo cancelled by the person" (or "Redo") after the person's `C-g`. Past 8 s, under the
  channel's 10 s wait for a reply, it replies that the undo goes on and `state`'s `undoing` says
  how far; the reply to a save waits as long, from the same constant
  (`channel::server::ANSWER_WITHIN`). A connection that goes, or a channel turned off, forgets
  the reply; the undo goes on. The `state` tool of `oso mcp` names `undoing`.
- **Freeing.** The editor keeps what each commit lets go, now with the groups the history's
  limit drops at that commit, and what a crossing cancelled, abandoned or replaced held, until
  the host takes it with `Editor::take_crossing_freed`. `App::apply` hands it to the thread that
  frees a reloaded file's text. The windows' old wraps are still dropped at the commit, and an
  edit still frees the groups the limit drops where it is.
- **Large batches are recorded in parts of at most 4096 splices** (`undo::BATCH_SPLICES`), the
  lowest first, so that they apply from the highest back as the whole did. The text, the content
  id and every offset come out as before, and a crossing at once and in slices cross the same
  group. What may differ is where another window's cursor lands when it sits exactly where two
  parts meet, as it follows the parts one after the other rather than the batch whole. A batch
  that fits is recorded as before, with nothing allocated beside it.
- **A slice hands out what cursors follow up to where it may last be cut.** Step 2 handed out
  the part of a run applied in a slice only when the slice stopped where it could be cut, and
  otherwise kept all of it for the next. Now a slice keeps only what it applied after the last
  place it could be cut.
- **A run joins at most 65,536 splices** (`RUN_SPLICES` in `buffer/sliced.rs`). What ends with a
  run, as the edited lines it follows, reads all of it at once, which step 2 left for this step;
  a large step's batches that lie apart now make several runs, each ending in a slice of its own.
- **Measured** with `text_perf -- undo`, release build, with what a commit lets go dropped
  outside the times as the host frees it:

  | Case | Slices | p50 | p99 | Max | Last | Whole |
  |---|---|---|---|---|---|---|
  | `!` over 4M short lines | 172 | 3.00 ms | 3.16 ms | 3.17 ms | 1.10 ms | 514 ms |
  | the same, wrapped | 169 | 3.00 ms | 3.16 ms | 5.09 ms | 5.09 ms | 509 ms |
  | the same, two windows | 176 | 3.00 ms | 3.16 ms | 3.16 ms | 0.71 ms | 526 ms |
  | `!` over 2 MiB, highlighted | 233 | 3.00 ms | 3.23 ms | 4.13 ms | 0.80 ms | 698 ms |
  | 20,000 answers far apart in 100 MB | 14 | 3.00 ms | 3.01 ms | 3.01 ms | 1.90 ms | 41 ms |
  | 20 commands joined at 1000 cursors | 10 | 3.00 ms | 3.01 ms | 3.01 ms | 2.63 ms | 30 ms |
  | an 89 MB kill | 21 | 2.36 ms | 2.94 ms | 3.01 ms | 1.78 ms | 52 ms |
  | 200k splices in one edit, highlighted, 1000 cursors in a second window | 201 | 3.00 ms | 4.12 ms | 4.18 ms | 3.79 ms | 604 ms |
  | `!` deleting 2M bytes, 1000 a line, highlighted | 280 | 3.00 ms | 4.09 ms | 9.30 ms | 2.35 ms | 844 ms |

  Redo's slices take about what undo's do. The last slice, which commits, took 15.8 to 23.5 ms in step 2, almost
  all of it freeing; in a wrapped window it still takes about 5 to 6 ms, rewrapping the rows in
  view with the command's budget. A run that ends follows the edited lines, which puts p99 at
  3.16 ms where runs end often. The longest slice of the last case was 4.3 to 5.0 ms in three
  more runs; the 9.3 ms came once. Before the cut and run changes above, the "200k splices"
  case's longest slice took 47 ms with the batch kept whole, and the last case's 17 ms.

- **In the running app** (release build with the dev socket, the window open but not focused,
  reading `get-state` every 200 ms), undoing `!` over 3.6 million lines of a 100 MB file took
  4.5 to 4.7 s from `C-z` to the commit, and redoing it 4.5 s, the same with keys held, which
  then ran in order. The slices took about 0.4 s of it: the loop ran about 22 turns a second, and
  the UI thread waited for the next event about 80% of the time, so the wait is the pace of one
  slice a turn, not the work. The echo area said `Undoing… 46%` midway while the text stayed as
  before. Whether to run more than one slice a turn is put to the author.
- **Tests.** Unit tests in `oso-core`: what a commit, a cancel, a replaced and an abandoned
  crossing let go waits to be taken; the progress comes before a message until the end; a
  reload waits; a batch of more than two parts, of replacements that meet and of deletions apart,
  crosses back and forth at once and in slices; a batch is cut into parts each no larger than one,
  lowest first; slices that stop where cursors may not be cut hand out no step longer than a
  batch (it handed out three batches' worth before); and batches past a run's splices make
  several runs that give the text one batch after another gives. Through the `Dispatcher`: keys and a close request
  wait and run in order, `C-g` cancels an undo and a redo and drops the keys, a replay waits for
  an undo its own key began, and the count limit. In the app: the channel's wait, `get-state`'s
  `undoing` beside a secret buffer, and the restart's and the exit's refusals. End to end
  (`e2e/undo.rs`, `e2e/agent.rs`): over 20,000 lines replaced by `!`, keys typed after `C-z`
  run after it, the progress shows, a restart waits for it, `C-g` cancels an undo and a redo, and
  an agent's undo replies when it ends and hears the person's `C-g`.
- **Left:** the windows' old wraps are dropped at the commit; query-replace's `u` undoes at once,
  as its step is one replacement; `!`'s later slices still run outside a panic boundary; the
  input held meanwhile runs in the turn that commits, as after `!`.

## Changes to accepted ADRs

- [ADR 0113](0113-query-replace-keyboard-macros-and-whitespace-cleanup.md): the exception for
  undoing a large `!` ended with step 3; its "As built" points here. The window's close button
  and the wheel now wait while `!` replaces, as keys do.
- [ADR 0053](0053-edit-core.md): a crossing may be staged over several turns, still on clones and
  committed once.
- [ADR 0090](0090-oso-dev-carries-unsaved-text.md): a restart also waits for a crossing, and so
  does an `exit` with a generation.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md): the channel also waits for a crossing,
  `get-state` gains `undoing`, and the reply to its own `undo` waits for the crossing to end.
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
- A caller that begins a crossing must allow one still going on when it returns. `Editor::undo`
  and `Editor::redo` still cross a whole step (step 2 decision 1), as query-replace's `u` needs;
  the `undo` and `undo-redo` commands use `Editor::begin_crossing` since step 3.

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
