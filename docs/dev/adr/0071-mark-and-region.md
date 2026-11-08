# 0071. The mark and the region

- Status: Proposed
- Date: 2026-11-08

## Context

M2-13 brings the mark and the region, which M2-14's kills, M2-20's isearch and M2-21's
multiple-cursor keys build on ([roadmap](../roadmap.md)). Earlier ADRs fixed the outline:

- each selection of a window has a cursor and its own mark ([ADR 0007](0007-multi-cursor.md));
- `C-,` pops to the previous mark, and `C-g` removes extra cursors first, then deactivates
  the mark ([ADR 0030](0030-m2-editing-commands.md));
- a region is a list of rectangles on `TextView`, in cells computed in `oso-core`, drawn in a
  new `Face` after the pane's background and before its boxes
  ([ADR 0061](0061-panes-clipping-and-layers.md));
- marks need their own mapping through edits, since they are not sorted as cursors are
  ([ADR 0053](0053-edit-core.md));
- `oso dev` carries marks through a restart from M2-15 ([ADR 0051](0051-m2-plan-decisions.md)).

The author's Emacs keeps `transient-mark-mode` and `shift-select-mode` at their defaults,
binds `C-,` to `pop-to-mark-command` and `C-.` to `consult-mark`, and shows the region as a
background colour only (modus-themes, `bg-only`).

## Decision

The author chose each item under "Behaviour", "How it looks", "Several cursors" and "The
control socket" on 2026-11-08, from options put to them, all as recommended, and the region's
tint from screenshots. The rest are defaults they accepted with them.

### Behaviour

- **The region shows only while it is active,** as Emacs's `transient-mark-mode`. `C-SPC`
  shows it, and motion keeps it and grows it. It ends on any edit to the buffer, from any
  window, which includes typing, `DEL`, `C-d`, undo and an input method's commit. It also
  ends on `C-g`, on `C-SPC C-SPC` and when the window shows another buffer. Until M2-14,
  typing with a region inserts and ends it; M2-14 makes it replace the region.
- **`C-SPC`** (and `C-@`) sets the mark at every cursor, shows the region and says "Mark
  set". Repeated at once, it hides the region and keeps the mark ("Mark deactivated"), the
  Emacs way of remembering a place. A third sets it again.
- **`C-x C-x`** swaps every cursor with its mark and shows the region. `C-u C-x C-x` swaps
  and hides it. With no mark on the primary cursor it signals "No mark set in this buffer".
- **The mark ring.** The ring keeps the primary cursor's last 16 marks, the newest first, and
  they follow edits as the marks do. `C-SPC` puts the old mark there, as Emacs's `push-mark`
  does, and so do `M-<` and `M->`, which set the mark where the cursors were and say "Mark
  set". They do not with a plain `C-u`, nor while the region shows, which the jump then
  extends. `C-u C-SPC` and `C-,` go to the mark with one cursor and hide the region. The mark
  then takes the newest one in the ring, and the old one goes last, so repeating `C-,` goes
  back through it. A `C-SPC` straight after `C-u C-SPC` sets the mark
  (`set-mark-command-repeat-pop` nil). `C-.` waits for completion (M2-17). Later steps set the
  mark where Emacs does: isearch's exit (M2-20), yank (M2-14).
- **`C-x h`** (`mark-whole-buffer`) leaves one cursor at the start with its mark at the end,
  shows the region and puts the old mark and the old cursor in the ring.
- **Shift selection,** as Emacs's `shift-select-mode`: a motion with Shift shows a region
  from where it started, and the next motion without Shift hides it. A region shown by
  `C-SPC` stays, and a shifted motion only grows it. The motions are those of `C-f`, `C-b`,
  `M-f`, `M-b`, `C-n`, `C-p`, `C-a`, `C-e`, `<home>`, `<end>`, `M-<`, `M->`, `C-v` and `M-v`, with
  any of their keys: `S-<right>`, `C-S-f` (which arrives as `C-F`), `M-S-f` and so on.
- **Shift translation is in the key resolver.** A key with Shift that is bound to nothing,
  `S-<right>` or `C-F`, resolves to what the key without Shift is bound to, as
  `Resolution::Shifted`, and the command sees `Ctx::shifted`. A key bound with Shift keeps its
  own binding. `KeyPress::unshifted` takes Shift off a named key, and lower-cases an upper-case
  letter held with Control, Meta or Super. A plain capital still types itself. Only a command
  is found this way, not a prefix key. No change to how winit's keys are read.
- **Marks follow edits as Emacs markers,** with `Bias::Left` in every window: text typed at a
  mark goes after it, and a mark in deleted text goes to its start. They are then clipped to
  the start of a display cluster, as the cursors are.
- **Undo puts back the marks with the cursors,** from the undo group, and hides the region.
  The ring stays as it is, so a mark pushed after an edit is not lost by undoing it.
- **A restart drops the marks and the ring** until M2-15 carries them. The state handoff's
  format does not change.

### How it looks

- **Background only, in `Face::Region`:** a muted blue-grey, linear `[0.06, 0.10, 0.20]`
  (about `#48587a`), until the theme (M2-23) maps it to its palette and the contrast check.
  The author picked it from screenshots of three tints, over a darker blue and a neutral
  grey that was close to the modeline's. Text keeps its own face, so moving the region
  reshapes nothing. The escape boxes, the cursor box and a preedit's underline are drawn over
  it.
- **A row whose line break is in the region is filled to the window's right edge,** as
  Emacs's region face with `:extend`. An empty region draws nothing.
- **During a composition the region stays drawn.** The preedit sits at the primary cursor,
  outside the region, and pushes the region's cells after it on that row, as it pushes the
  text. The commit is an edit, so it ends the region. A cancelled composition leaves it.

### Several cursors

- **Each cursor has its own mark** (ADR 0007), and **the window shows all their regions or
  none.** `C-g` and an edit hide them all together.
- **Regions that overlap merge into one selection while they show,** with the union of their
  text, so that a kill in M2-14 never takes text twice. The merged one keeps the direction of
  the primary, if it is one of them. Regions that only touch stay apart, and so do hidden ones,
  which merge only where two cursors meet, as before.
- **`C-u C-SPC`, `C-,` and `C-x h` leave one cursor,** at the primary's mark, as `M-<` does.

### What it is made of

- **`oso_core::selection`:** `Selection::mark`, `Selection::move_to` (moves the cursor and
  keeps the mark, which every motion now uses), and `Selection::range`. `SelectionSet` gains
  `Region` (`Inactive`, `Active`, `Shifted`), the `MarkRing`, kept inline as an array of 16
  so it costs no allocation when the set is cloned, and `push_marks`, `pop_mark`,
  `keep_primary` and `set_region`. Sorting merges overlapping regions while they show. Marks
  follow edits with the cursors, in O(cursors + edits) while they are in order, which setting
  them at the cursors leaves them, and correctly in any order.
- **`oso_core::frame::region_cells`** gives the frame's cells of a window's regions: one
  rectangle per row of a region in view, found by a binary search on the selections, so a
  frame converts two offsets for each region in view and nothing else. `Editor::region_cells`
  calls it for the selected window, with the preedit. No new module or trait, and no change
  to `guardrails/layers.txt`.
- **`oso-render`:** `TextView::region` holds those cells, and `scene` fills them in
  `Face::Region` after the pane's background, before the boxes, the glyphs and the cursor.
- **`oso`:** a `mark` module in `commands` and in `bindings`, with `set-mark-command`,
  `exchange-point-and-mark`, `pop-to-mark-command` and `mark-whole-buffer`, `shift_select` for
  the motion and scroll commands, and `keyboard-quit` hiding the region. The dispatcher passes
  `shifted` in `Invocation`. Text-grid snapshots mark a region's cells with `#` on a line
  between `~`.

### The control socket

This is a human gate ([`REVIEW.md`](../../../REVIEW.md)).

- **`get-state`** gives each selection a `mark` (`{offset, row, column}` or null), the
  top-level `cursor` the primary's `mark`, and each window `mark_active` and `mark_ring`
  (offsets, the newest first).
- **For a buffer that may hold secrets, none of it shows,** neither the marks, nor
  `mark_active`, nor the ring, as `cursor` and `selections` are already left out
  ([ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md)). The region is drawn
  on the screen as in any buffer, and `screenshot` is refused there already
  ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)). `export-state` keeps one
  cursor at the start and no mark. No modeline shows the region's size.

### Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0030](0030-m2-editing-commands.md), the `C-g` chain: `C-g` hides the region now; removing
  extra cursors first comes with the multiple-cursor keys (M2-21).
- [ADR 0053](0053-edit-core.md), "Edits clear goal columns and leave other fields of a selection
  alone": they move the marks too, as markers, and hide the region.
- [ADR 0061](0061-panes-clipping-and-layers.md), "a list of rectangles on `TextView`": they are in
  the frame's cells, as the cursor box's are, from `frame::region_cells`.

## Consequences

- Every motion keeps its cursor's mark. A command that builds a `Selection` anew drops it, so
  motions use `Selection::move_to`.
- `M-<` and `M->` now say "Mark set", which changes four text-grid snapshots by that line.
- A frame with a region shown allocates a small list of rectangles, one per row of it in view.
  Without one it allocates nothing more.
- The cost of a key with a region shown, and of a frame, is in "Costs" below.
- Two cursors whose regions come to overlap become one, which a person may not expect from
  multiple-cursors.el, where they stay apart.
- `oso dev` loses the marks and the ring at a restart until M2-15.
- `C-SPC` reaches oso only if macOS's "Select the previous input source" shortcut, Control-Space by
  default, is off.
- The `oso-core` public API gains `Region`, `MarkRing`, `MARK_RING_SIZE`, `Selection::mark`,
  `move_to` and `range`, the set's mark methods and `as_slice`, `frame::region_cells`,
  `Editor::region_cells`, `KeyPress::unshifted`, `Resolution::Shifted`, `Invocation::shifted`
  and `Ctx::shifted`. `oso-render` gains `Face::Region` and `TextView::region`.

### Costs

To be measured with `just perf` before this is accepted.

## Alternatives considered

- **The region shown until `C-g`, through edits:** unlike Emacs, and M2-14's typing over a region
  would then replace one set long before.
- **`C-SPC` toggling the region whenever it shows:** it could no longer set the mark again while
  a region shows.
- **No ring in this step:** no way back after `M-<` until a later step.
- **Shift selection on the arrows only, or none in M2:** less to build, but `C-S-f` and the other
  Emacs motions would not select as the author's Emacs does.
- **Recoloured text in the region:** the text's face would change at the region's edges, so every
  key that grows the region would reshape its rows, and a ligature could break there
  ([ADR 0061](0061-panes-clipping-and-layers.md) shapes each face on its own).
- **Only the text's cells, or one cell for the line break:** an empty line in the region would
  show nothing, or almost nothing.
- **Hiding the region during a composition,** as macOS text views replace a selection when
  composing starts: a cancelled composition would have to bring it back.
- **Overlapping regions kept apart,** as multiple-cursors.el: M2-14 would need a rule for kills of
  overlapping pieces.
- **`mark_active` shown for a secret buffer:** one more bit that changes with the buffer's text.
- **The ring in `Window`, or in the buffer as Emacs keeps it:** it would need its own mapping
  through edits beside the cursors'. In the selection set it follows edits with them, and the
  window has one set.
- **Shift translation in the tip crate's key reading:** it would have to know the keymaps, and the
  socket's keys would not get it.
