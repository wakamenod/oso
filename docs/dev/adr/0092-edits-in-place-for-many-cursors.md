# 0092. Edits in place for many cursors

- Status: Accepted
- Date: 2026-11-09

## Context

[ADR 0091](0091-multiple-cursor-keys-and-m-y.md) caps a window at 1000 cursors and asks the edit
core to fit them: a command's splices applied in one pass, through a batched splice in `rope/` and a
batched sync in `display_map/`, chosen between an edit in place inside a leaf and one walk that keeps
the untouched subtrees by measuring both; cursors and marks clipped only near an edit; and the
edit's share of key press to photon at p99 ≤ 4 ms with 1000 cursors, typing and undo, bare and over
regions, on the 100 MB file and the 10 MB single line. If 1000 cursors missed it, the bound went back
to the author.

Before the change, typing at 1000 cursors took 16.75 ms at p50 and 25.8 ms at p99 on 100 MB: each
splice split the rope's tree and the tab map's twice and appended twice, and every cursor and mark
was clipped with two scans of its chunk.

## Decision

The author decided on 2026-11-09, through the coordinator, after the measures below.

- **Only the edit in place was built.** `SumTree::replace_items`, `pub(crate)` in the private module
  `sum_tree/replace.rs`, replaces a run of items in the leaf that holds it, copying only the path to
  that leaf; on the clone a command edits, a node copied for one splice is not copied again for the
  next. A run that spans leaves, or would leave a leaf too small or too large, splits and appends as
  before. `Rope::replace` and `TabMap::sync` call it, one splice at a time, as `buffer/splice.rs`
  drives them; there is no batched splice in `rope/` and no batched sync in `display_map/`.
- **The walk that keeps the untouched subtrees was not built:** the edit in place met the 4 ms share
  on the 100 MB file and on long ASCII lines, and what misses it on the dense line is not the tree
  (below), which a walk would not change.
- **Clipping near edits and a cluster in one scan** are as ADR 0091 says, with the invariant that
  makes them sound written in `display-model.md`: a display cluster depends only on its rope chunk's
  text and where the chunk starts.
- **The bound stays at 1000.** A faster walk over characters that cannot join their neighbours, for
  each rebuilt chunk's summary, is recorded in `open-questions.md` as the next step.
- **No `just perf` scenario of 1000 cursors yet:** its thresholds are guard files. `text_perf --
  cursors` measures the edit's share, and the macmini's `just perf` checks that the existing
  scenarios do not regress.

### Measures

p99 of typing one character at 1000 cursors and of undoing it, with `text_perf -- cursors` on a
MacBook, against the 4 ms share:

| Text | Bare | With marks | Over regions |
|---|---|---|---|
| 100 MB file | 2.8 / 3.0 ms | 2.9 / 3.1 ms | 3.2 / 2.9 ms |
| 10 MB line of minified JSON | 3.3 / 3.3 ms | 3.4 / 3.5 ms | **4.7** / 4.1 ms |
| 10 MB line dense with CJK, emoji and marks | **5.6 / 5.8 ms** | **5.7 / 5.8 ms** | **9.3 / 8.4 ms** |

Before: 23.1 / 17.7 ms bare on 100 MB, and 28.5 / 27.6 ms bare and 36.6 / 42.8 ms over regions on the
dense line. One cursor on 100 MB went from 12.5 to 3.1 µs at p99, and one edit with its display sync
from 21 to 2.3 µs at p50. What remains on the dense line is each rebuilt chunk's tab summary, which
walks the chunk's clusters through the grapheme segmenter.

## Changes to accepted ADRs

The passage below stays in its ADR as written; what holds now follows it. The status of ADR 0091
becomes `Accepted, amended by 0092`.

- [ADR 0091](0091-multiple-cursor-keys-and-m-y.md), "A command's splices are applied in one pass. The
  rope gains a batched splice and the tab map a batched sync, each `pub(crate)`, in its own layer
  (`rope/` and `display_map/` ...), and `buffer/splice.rs` drives both": each splice is applied as
  before, through `SumTree::replace_items`, which edits a leaf in place and shares the copied path
  across a command's splices. The private module is `sum_tree/replace.rs`, not under `rope/`.
- ADR 0091, "if 1000 cursors miss the budget, the bound goes back to the author": they miss the 4 ms
  share on dense mixed-script long lines and over regions on a JSON line, and the author keeps 1000.

## Consequences

- Typing and undo at 1000 cursors fit the share on large files and ASCII long lines; on a long line
  dense with CJK, emoji and marks they take up to 9.3 ms at p99, over a frame at 120 Hz.
- Every edit, also with one cursor, copies less of the tree.
- `sum_tree` gains a second way to edit; a property test holds it to two splits and two appends, and
  to `Vec::splice` over many edits with no clone between.
- Key press to photon with 1000 cursors is not measured by `just perf` until a scenario is agreed.

## Alternatives considered

- **The walk that keeps the untouched subtrees, as ADR 0091 left open:** one primitive for the rope
  and the tab map, but more code in `sum_tree`, and the edit in place already met the share where
  the tree was the cost.
- **A batched splice in `rope/` and a batched sync in `display_map/`:** each would hold a copy of the
  same tree logic; `replace_items` keeps it in `sum_tree` once.
- **A lower bound, such as 500, until the faster walk:** the dense line would fit, but a rename across
  a large file would not reach, and the common cases already fit at 1000.
