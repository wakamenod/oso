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
  letter held with Control, Meta or Super. A plain capital still types itself. A prefix key
  is found this way too, as in Emacs, so `C-X C-S` saves: the key goes on as the one bound,
  and only a command found with its last key so is shifted. No change to how the window's
  keys are read.
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
  `MarkActive` (`Inactive`, `Active`, `Shifted`), named as Emacs's `mark-active` and
  `get-state`'s field, since a region of the frame is another thing; the `MarkRing`, kept inline
  as an array of 16 so it costs no allocation when the set is cloned; and `push_marks`,
  `pop_mark`, `keep_primary` and `set_mark_active`. Sorting merges overlapping regions while they show. Marks
  follow edits in ascending order, in O(cursors + edits) when they are in order, as setting them
  at the cursors leaves them, and after a sort when they are not, as regions in both directions
  leave them. The ring's 16 are sorted on the stack.
- **`oso_core::frame::region_cells`** gives the frame's cells of a window's regions: one
  rectangle per row of a region in view. Each row finds the regions in its columns in view by a
  binary search on the selections, so a frame converts two offsets per row and two for each
  region a row shows, however long the rows and however many regions are out of view. `Editor::region_cells`
  calls it for the selected window, with the preedit. `Editor::deactivate_mark` is `C-g`'s
  step for the region: it hides the regions, keeps the marks and says whether one was shown,
  so a chain of what `C-g` cancels (M2-16) calls it first and stops there if it returns true. No new module or trait, and no change
  to `guardrails/layers.txt`.
- **`oso-render`:** `TextView::region_cells` holds those cells, and `scene` fills them in
  `Face::Region` after the pane's background, before the boxes, the glyphs and the cursor.
- **`oso-core`'s `editor/follow.rs`,** a private module, holds how cursors and marks follow a
  change, moved out of `editor.rs` with one helper that clips cursors, marks and the ring, which
  the way back from `*Messages*` shares. A message logged to `*Messages*` moves its cursors and
  marks but leaves its region shown, as Emacs's message log does, and a mark at its cursor moves
  with it, so `C-SPC` at the end says "Mark set" with no region over it. Sorting merges only
  while regions show; with them hidden it is the plain sort and merge of cursors that met, as
  before marks.
- **`oso`:** a `mark` module in `commands` and in `bindings`, with `set-mark-command`,
  `exchange-point-and-mark`, `pop-to-mark-command` and `mark-whole-buffer`, `shift_select` for
  the motion and scroll commands, and `keyboard-quit` hiding the region. The dispatcher passes
  `shifted` in `Invocation`. Text-grid snapshots mark a region's cells with `*` on a line
  between `+`.

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
- [ADR 0034](0034-command-registry-and-keymaps.md), a key resolves to a command, a pending prefix,
  an unbound sequence or a cancel: a fifth outcome, `Shifted`, is a command found by a second
  lookup without Shift on the last key, and a prefix found so is pending.
- [ADR 0030](0030-m2-editing-commands.md), "typing over an active region replaces the region":
  until M2-14, typing inserts and ends the region.
- [`display-model.md`](../display-model.md), selections as a layer of decorations: the region is
  rectangles of cells on the text pane, not a decoration layer, which no producer needs yet.
- [ADR 0045](0045-text-grid-snapshots.md), the text grid's notation: a line between `+` marks a
  region's cells with `*`.

## Consequences

- Every motion keeps its cursor's mark. A command that builds a `Selection` anew drops it, so
  motions use `Selection::move_to`.
- `M-<` and `M->` now say "Mark set", which changes four text-grid snapshots by that line.
- A frame with a region shown allocates a small list of rectangles, one per row of it in view.
  Without one it allocates nothing more.
- Two cursors whose regions come to overlap become one, which a person may not expect from
  multiple-cursors.el, where they stay apart.
- `oso dev` loses the marks and the ring at a restart until M2-15.
- `C-SPC` reaches oso only if macOS's "Select the previous input source" shortcut, Control-Space by
  default, is off.
- The `oso-core` public API gains `MarkActive`, `MarkRing`, `MARK_RING_SIZE`, `Selection::mark`,
  `move_to` and `range`, the set's mark methods and `as_slice`, `frame::region_cells`,
  `Editor::region_cells`, `Editor::deactivate_mark`, `KeyPress::unshifted`, `Resolution::Shifted`, `Invocation::shifted`
  and `Ctx::shifted`. `oso-render` gains `Face::Region` and `TextView::region_cells`.

### Costs

Measured in a release build, on a fast machine, through `oso-core` with no window:

- **`frame::region_cells`** on a view of 60 rows by 200 columns: about 2 µs on a 10 MB single
  line, with 1, 100 or 1000 regions along it and the view at its start or 5 MB in, and about
  28 µs on 100 MB of source-like text, with 1 or 1000 regions spread over it.
- **Typing with a mark at every cursor,** which clips each mark after the edit: on 100 MB,
  100 cursors 1.34 to 1.46 ms at p50 and 1000 cursors 16.2 to 16.9 ms; on a 10 MB line,
  100 cursors 1.72 to 2.04 ms and 1000 cursors 15.7 to 19.9 ms. Only marks near an edit can
  need clipping, so this can be cut to those if many cursors with marks become common (M2-21).
- **The ring is copied with every saved set of cursors,** 136 bytes, into each undo group too,
  where an undo does not use it. A group already holds the cursors' `Vec`, so this adds no
  allocation, and the history is bounded in bytes, which counts it.
- The key and frame latencies of `just perf` are in the pull request.

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
