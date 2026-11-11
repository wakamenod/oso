# 0053. Edit core: edits, selection mapping and undo

- Status: Accepted, amended by 0112
- Date: 2026-11-04

## Context

Everything in M2 builds on changing text ([`roadmap.md`](../roadmap.md), M2-1).
`Rope::replace` and `DisplayMap::sync` existed, but nothing connected them: buffers
had no edit method and no modified flag, and selections could not follow an edit.

The forces:

- A panic is caught at the command boundary and the editor keeps running
  ([ADR 0014](0014-panic-strategy.md)), so a half-applied rope, display map and undo
  log must not survive one.
- `Buffer` may not know about windows (`guardrails/layers.txt`), and a buffer can
  show in several windows, each with its own selections
  ([ADR 0007](0007-multi-cursor.md)).
- A keystroke must not cost work proportional to the buffer, and killing a huge
  region should not copy it ([ADR 0051](0051-m2-plan-decisions.md)).
- The commands, keys and dispatcher that call this arrive in M2-4.

## Decision

### Edits

- `Editor::apply_edits(buffer, &[Replacement])` takes byte ranges in the text as it
  is now, usually one per cursor, in any order. A range is clamped as in
  `Rope::replace`. Empty replacements are dropped, and if none is left the call does
  nothing: no revision, no content id, no undo step. A replacement with the same text
  still counts as an edit, as in Emacs.
- **Overlaps merge.** Ranges that overlap, and insertions at one offset, become
  their union, replaced by their texts joined in offset order; replacements with
  one start keep their given order. Ranges that only touch stay apart. Two cursors
  killing words that overlap should delete both, so refusing is the wrong answer.
- They are applied in descending offset order, so each leaves the offsets below it
  valid.
- **Edits are atomic.** The buffer works on clones of the rope and the display map,
  both O(1) to clone, and returns a staged edit. The editor works out every window's
  cursors against it. Only then does anything change, in moves that cannot panic:
  rope, display map, revision, content id, undo record and cursors. Dropping a staged
  edit changes nothing, which is what a panic does.
- A read-only buffer refuses the edit (`EditError::ReadOnly`), as does undo. Buffers
  with invalid bytes are read-only, so the offset-keyed `InvalidBytes` table stays
  valid.
- Edits live on `Editor`, not `Buffer`, since mapping selections needs the windows.
  `apply_edits` maps them itself, so no public accessor for other windows' cursors is
  needed and no caller can forget a window.

### Selection mapping

Every window showing the buffer follows the edit. Following costs O(cursors + edits),
and clipping O(log n + a chunk) per cursor, in every such window. A cursor before an edit keeps its offset, one after it shifts, and a cursor at the end of the
replaced text stays before the text that followed it. The rest depends on the window:

- **The selected window edits.** A cursor at the start of an edit, or inside its
  replaced text, ends after the new text, so a command inserting at each cursor need
  not move them. A command may move them afterward, as `open-line` will.
- **Other windows follow as Emacs markers.** A cursor at the start of an edit stays
  before the new text, and one inside the replaced text moves there.

Mapped cursors are then clipped to a display cluster, because an edit can move those
boundaries, and cursors that met merge, keeping a primary. A cursor clips to the cluster's
start, except a selected-window cursor just after new text, which clips to the cluster's end
if the text joined what follows. Otherwise typing a mark before existing text would put the
next character before what was just typed. So typing a virama before a consonant moves the
cursor past that consonant. Edits clear goal
columns and leave other fields of a selection alone. Marks (M2-13) need their own mapping,
since they are not sorted as cursors are.

### Modified state

A content id (`oso_core::buffer::ContentId`) names a state of the text. Each edit gets a
new one, undo and redo restore the stored one, and ids are never reused. A buffer is
modified when its id differs from the saved one. Safe save reads `Buffer::content_id`
with the text it writes, and calls `Buffer::mark_saved` when the write is done, so edits
made during the save leave the buffer modified. A load is a new saved state with no
history. The same text reached by other edits counts as modified.

### Undo

- **A new private module, `undo`,** below `buffer`. It stores and applies nothing itself.
  `buffer` re-exports its one public type, `ContentId`.
- **A group stores what crosses it:** batches of splices, in offsets of the text they
  apply to, and on each side a content id and the selected window's cursors. Crossing a
  group yields the group that crosses back, so undo and redo are one operation. Removed
  text is a `Rope::slice`, which shares chunks and is O(log n). Inserted text is not
  stored at all until undo takes it out of the rope, so typing keeps almost nothing.
- **Groups are opened by the dispatcher,** which already holds the panic boundary:
  `Editor::begin_undo_group`, then `end_undo_group`, or `end_undo_group_after_panic` for the
  panic path, which never panics. What a panicking command applied stays as one undo step,
  and nothing joins it. Commands do not nest. A `begin` closes a group left open, so a missed
  end costs one step instead of merging every later command. An edit with no group open is a
  step of its own. The group's closing cursors are read at the end, so a command may move
  them after its edits.
- **A command joins** by calling `Editor::allow_undo_join`, for self-inserts. Its edits
  join the previous group if the previous command called it too, edited something and undid
  nothing, the text and the cursors are where that group left them, no redo is pending, and
  fewer than 20 commands joined (`amalgamating-undo-limit`). Any other command in between ends
  the joining.
- **Undo acts on the selected window's buffer.** That window gets the group's stored
  cursors; if the group has none, because the selected window showed another buffer when
  it was made, the cursors follow the text. Other windows follow the text.
- **The bound is in bytes,** 64 MiB per buffer: stored text, plus an estimated cost per
  group, splice and cursor. **It leaves out the group last recorded or crossed,** which always
  stays whatever its size, so peak memory is the bound plus that group. Without this, undoing
  a large paste would drop every older group: it costs nothing recorded, but its text moves to
  the redo stack. Past the bound, redo groups go first, from the furthest, since losing one
  costs less than losing older history, and then the oldest undo groups. Content ids make the
  rest follow: a state dropped from the history cannot come back, so a buffer whose saved state
  was dropped stays modified until it is saved.

### Messages

`Editor::message` keeps the last 1000 messages and writes each to the log, escaped so that
a name in one cannot move a terminal's cursor. It lives on `Editor`, not `Ctx`, because save
completion and recovery offers arrive outside commands. Drawing it is M2-7.

Since messages reach the log, they must never quote buffer text
([ADR 0028](0028-secrets-handling.md)). `Replacement` and the undo types have no `Debug`,
and the `Debug` of `Rope` now shows only its length. Types that hand out text for drawing,
such as `DisplayCluster` and `CellRun`, still print it, so no log line may format them.

`oso-core` now depends on `log`, with the author's approval. It was already in the build
for `oso` and `oso-render`, so no crate is added.

## Consequences

- M2-4 wraps `Registry::execute` in `begin_undo_group` and `end_undo_group`, or
  `end_undo_group_after_panic` when the result is `Panicked`, and commands call `apply_edits`.
  It also gets `From<EditError> for CommandError`.
- The costs of large kills are measured in `crates/oso-core/examples/text_perf.rs`, and
  their open limits, with when each must be settled, are in
  [`open-questions.md`](../open-questions.md). Killing 89 MB of a 100 MB buffer takes about
  1.2 ms, and undoing it about 63 ms, since the slice is copied and measured again. Redoing
  it takes 1.4 ms.
- A keystroke costs about 9 µs on a 100 MB buffer with one cursor, and 15 ms with 1000.
- M2-4 needs a test of the panic path through `Registry::execute`.
- A keyboard macro (M3) that wants one undo step needs groups to nest, which they do not.
- Undo cannot return part of a group, and more than one window cannot have its cursors
  restored by undo. Only the window that undoes gets them.
- A window that is not selected can keep a scroll position past the end of the text. Splits
  (M3) must deal with it.
- History is dropped on the next edit past the bound, on the UI thread: about 2.5 ms for an
  89 MB group. That edit may come right after a kill far over the bound: the kill stays only
  until something newer exists.

## Alternatives considered

- **`Buffer::edit` that maps nothing, with cursors mapped by each command:** no windows in
  the edit, but every command must remember every window.
- **Refusing overlapping edits:** simple, as VS Code does, but two cursors deleting
  overlapping words would fail with a message about a thing the user cannot see.
- **Keeping the first of two overlapping edits:** the other cursor's text is lost silently.
- **Moving every window's cursors past an insertion:** one rule, but typing in one window
  would push the cursor of another, unlike Emacs.
- **Snapshots of the whole rope and display map in each group:** O(1) undo and no re-measuring,
  but a group holds a copy of the tree paths in two trees, far more than a typed character needs,
  and it contradicts ADR 0051.
- **Storing inserted text in the group:** undo would not read the rope, but a paste would stay in
  the history for nothing, as the rope holds it while the edit stands.
- **An open group kept in `Buffer` with explicit begin and end there:** but the cursors before a
  command belong to a window, which a buffer does not know.
- **Taking the saved state's reachability into account in a flag:** content ids need no flag.
- **Counting the newest group, and dropping older ones for it:** a strict total, but undoing a
  large paste would make the next `C-z` fail, and the saved state could become unreachable.
- **Dropping redo groups, even the one just crossed, to keep a strict total:** older history
  stays, but `C-z` then `C-M-z` would fail after a large paste.
- **Opening groups in `Registry::execute`:** it holds the panic boundary too and no caller could
  forget, but text from the IME reaches the dispatcher without a command
  ([ADR 0051](0051-m2-plan-decisions.md)) and needs a group too.
- **Taking messages from the host instead of depending on `log`:** no change to the manifest, but
  nothing would reach the log until M2-4 wired it.
