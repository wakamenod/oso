# 0101. Word checks by offset, and reading shared by cursors

- Status: Accepted
- Date: 2026-11-10

## Context

Word motion and the checks behind the keys that add cursors read display clusters by display
point ([ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md),
[ADR 0091](0091-multiple-cursor-keys-and-m-y.md)). Going back, they read a window of 128 columns
at a time and step back by it. Each check of a match converts offsets to points and back. On a
long line that is a seek and a scan of part of a chunk for every step, and checkpoints
([ADR 0099](0099-checkpoints-in-long-lines.md)) shortened the scan but kept the steps.

Two costs grow with the number of cursors:
- **Word motion at several cursors** gives each cursor a stop's worth of reading, 128 KiB, about
  2 ms on a long run with no word boundary. The 4 MiB a command reads is shared by all cursors,
  so 16 cursors on such runs take about 17 to 27 ms, and 64 take 33 to 54 ms before 32 of them
  have moved.
- **`TAB` at several cursors on one row** walks every cluster between each cursor and the next,
  with no budget ([ADR 0056](0056-typing.md)), to know where a tab between them now starts. Two
  cursors on a 10 MB row take 22 to 40 ms, and 100 take 46 to 82 ms.

[ADR 0099](0099-checkpoints-in-long-lines.md) left open whether the keys that add cursors should
check words in offsets, for a later ADR to decide. This is that ADR.

The author decided on 2026-11-10, each the recommended option:
1. Word checks read by offset, with the same rules and the same answers when nothing runs out.
   The display-point code stays as a test oracle.
2. Word motion, kill-word and word selection at several cursors share one command's reading of
   about 8 ms. One cursor keeps what it has.
3. `TAB` at several cursors on one row gets each later cursor's column from the TabMap in
   O(log n), with no walk between cursors.
4. Prototype and measure first, then write this ADR with the numbers.

## Decision

### Reading clusters by offset

- **The TabMap gives display clusters by offset,** forward from one and backward from one, with
  each cluster's start, text and kind and no columns: what word motion reads. They are
  crate-internal: `TabMap::pieces_after` and `pieces_before`, in `display_map/tab_map/reader.rs`,
  which `DisplaySnapshot` forwards. They cross chunks and line breaks, and give the clusters
  `clusters(point)` gives: invalid bytes, runs past a cluster's limits and boxed invisibles alike.
- **A scan starts where clusters are as a scan from the chunk's start finds them:** two ASCII
  characters but CR LF, a checkpoint, or the chunk's start. `cluster_at` shares that choice.
  Going back, it reads a stretch of about a stride (128 bytes) at a time and gives the stretch's
  clusters from the last.
- **Word motion and the word checks read this way:** whether a word is at an offset, may start or
  may end there, `M-f` and `M-b`'s stops, and the look back past marks. The rules are ADR 0058's,
  and the answers are the same as by display point whenever the reading does not run out.
- **The display-point code stays as a test oracle** (`word/by_points.rs`, test-only). Property
  tests check that both answer alike at every character boundary, over rich text with invalid bytes
  and runs past the limits, and over words. `clusters(point)` stays for drawing and line motion.

### What a word motion charges

- **Bytes read in order, as before, and a seek (512) for each stretch read backward** and for each
  line break crossed. A stretch replaces ADR 0058's window of a row read backward.
- **One cursor keeps 4 MiB a command and 128 KiB a stop** (ADR 0058).
- **Several cursors share 512 KiB a command,** about 7 ms at the slowest bytes word motion reads,
  about 13 ns each, within the 8 ms of a key. `WalkBudget::for_cursors(n)` gives it. It holds for
  `M-f`, `M-b`, `M-d`, `M-DEL` and selecting the word at each cursor before `C->` or `C-<`.
  Other commands at several cursors, such as `C-f` and `C-d` with a count, keep ADR 0058's 4 MiB.
- **Each cursor's seeks to where it starts come apart from the bytes:** a pool of 3 entry seeks a
  cursor, 3n at n cursors, as finding the word around a cursor takes one forward and two back.
  The first seek of a stop, and the first stretch a stop reads back, take one from the pool while
  it lasts, and only after that 512 bytes. Charged as bytes, those seeks alone would spend the
  512 KiB at about a thousand cursors on any text, though on most text one costs far less: at
  1000 cursors on 10 MB of source, `M-f` moved 884 cursors and `M-b` 462, where all of them move
  now. A count at one cursor has no pool, as before, and a command that has read all its bytes
  takes no more seeks from it.
- **The cursors spend it in order.** A motion leaves a cursor it runs out on where it got to, and
  a kill there kills nothing; both signal "Count too large", as ADR 0058 says of a command's
  budget. Selecting words reads the primary's word first, as it is the query, and a cursor the
  budget runs out on selects nothing; the key goes on with the primary's word.
- **The caps on the checks of a match stay** (`AFTER_WORD`, `BEFORE_WORD` in
  `editor/cursors/find.rs`). A check still charges only what it reads. With a seek charged for
  each stretch read backward, the longest word a query holds (1024 four-byte letters) uses most of
  `BEFORE_WORD`, so the caps should not shrink. A test checks that word is still found whole both
  ways.

### `TAB` at several cursors on one row

- **Each later cursor's column comes from `DisplaySnapshot::column_after(from, column, to)`:** the
  column at `to` once `from`, a cluster's start on the same line before it, is at `column`. It
  composes the column functions of the chunks wholly between from their summaries, in O(log n),
  and scans the chunks the two ends are in. What lies between keeps its cells, and a tab reaches
  the next stop from where it now starts, as before.
- **It counts columns from the logical line's start,** not from a view row's, so `TAB` stays
  logical when rows wrap, as soft wrap needs ([ADR 0100](0100-soft-wrap.md)), and the window's
  code can rely on it.
- **Whether a cursor is on the line of the one before** comes from the rope's line count, in
  O(log n) with no scan of clusters; only the first cursor on a line reads its column.
- **The walk between cursors goes,** and `TAB` needs no budget.

### Public API

`WalkBudget::for_cursors` and `DisplaySnapshot::column_after` are new and public, as the API
snapshot shows. No new module or trait is public. `SumTree::summary_in`, crate-internal, gives
the summary of a run of items without copying a node, for `column_after`.

## Changes to accepted ADRs

- [ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md), "Going somewhere else in
  the text costs 512: a line break, a window of a row read backward, the start of a word": a
  stretch of about a stride read backward by offset, in place of a window of a row.
- ADR 0058, "A command has 4 MiB across all its cursors and its count": word motion, kill-word
  and word selection at several cursors share 512 KiB, with 3 entry seeks a cursor apart from
  them; at one cursor they keep 4 MiB, and other commands keep 4 MiB across their cursors.
- [ADR 0056](0056-typing.md), "On a row with several cursors it counts what the earlier ones
  insert, walking what lies between … That costs the clusters between two cursors on one row": it
  takes each later cursor's column from `column_after`, in O(log n) and a scan of two chunks,
  and walks nothing. "Wrapped rows (M2-25) must count from the line's start": it does, as it
  counts logical columns.
- [ADR 0091](0091-multiple-cursor-keys-and-m-y.md), "With no region shown, the word around each
  cursor (ADR 0058's word) is selected first, with the regions shown": the cursors share one
  command's reading, the primary's first, and one it runs out on selects nothing. The checks that
  a match is a whole word read display clusters by offset, with the same rules and answers.

## Consequences

Measured with `text_perf -- words` in a release build on a MacBook, main against this change,
p50; runs differ by about 5% from one to the next:

| | main | this change |
|---|--:|--:|
| `M-f`, one cursor, 100 MB of source | 1.33 µs | 0.33 µs |
| `M-f` / `M-b`, one cursor, mixed 10 MB line | 12.4 / 11.9 µs | 0.25 / 1.5 µs |
| `M-f`, one cursor, 10 MB of empty lines | 900 µs | 1.2 µs |
| `M-f`, one cursor, 10 MB run of `x` / `д` | 1.04 / 1.73 ms | 1.19 / 1.72 ms |
| `M-b`, one cursor, 10 MB run of `x` / `д` | 614 µs / 1.47 ms | 179 µs / 1.07 ms |
| `M-f` with a count past what a command reads, 10 MB of `a ` | 7.2 ms | 0.57 ms |
| `M-f`, 64 cursors, 10 MB of source | 76 µs | 14 µs |
| `M-f` / `M-b`, 1000 cursors, 10 MB of source | 1.26 / 1.74 ms | 0.24 / 1.25 ms |
| `M-f` / `M-b`, 64 cursors, mixed 10 MB line | 131 / 278 µs | 12 / 84 µs |
| `M-f` / `M-b`, 1000 cursors, mixed 10 MB line | 2.13 / 4.45 ms | 0.22 / 1.40 ms |
| `M-f`, 16 cursors, 10 MB run of `x` | 16.4 ms | 4.3 ms (5 move) |
| `M-f`, 1000 cursors, 10 MB run of `x` | 32.8 ms (32 move) | 4.3 ms (5 move) |
| `M-f`, 16 cursors, 10 MB run of `д` | 26.9 ms | 6.8 ms (5 move) |
| `M-f`, 1000 cursors, 10 MB run of `д`, p99 | 53.1 ms (32 move) | 6.9 ms (5 move) |
| `M-b`, 1000 cursors, 10 MB run of `д`, p99 | 23.2 ms | 1.6 ms |
| `TAB`, 2 / 16 cursors on one mixed 10 MB line | 40.4 / 76.0 ms | 20 / 155 µs |
| `TAB`, 100 / 1000 cursors on one mixed 10 MB line | 81.2 / 96.2 ms | 0.99 / 10.5 ms |
| `TAB`, 100 / 1000 cursors on a 10 MB run of `x` | 45.2 / 54.3 ms | 0.54 / 5.7 ms |

- **A word key at many cursors stays within a frame,** whatever the text. On ordinary text every
  cursor moves, up to the 1000 a window may have; reading by offset makes it several times faster,
  and one cursor too. A run with no boundary still costs one cursor what it did, as a stop still
  reads its 128 KiB.
- **On runs with no word boundary, only the first few cursors move** in one key, about five, and
  the command signals "Count too large". The next key moves the next ones. One cursor behaves as
  before.
- **The pool's seeks are not charged as bytes,** so a word key can do at most `MAX_CURSORS` × 3
  seeks more than its 512 KiB pay for, each a descent of the tree and a scan of at most a chunk.
  A command that has run out of bytes takes no more of them.
- **`TAB` at 1000 cursors on one long line** spends most of its time applying the edits, about
  5 ms of the 10.5 ms, which is the edit core's (ADR 0092); the columns take about 3.5 ms, a scan
  of up to two chunks a cursor, as `TAB` makes new chunks there that have no checkpoints yet.
- **How far one stop goes back on a run with no boundary changes,** as a stretch is a stride of
  bytes, or reaches back to a checkpoint, where a window was 128 columns: about 23 KiB of a run of
  `x` where it was 26 KiB, and about 86 KiB of a run of `д` where it was 43 KiB. Forward is
  unchanged, at 128 KiB.
- **Soft wrap's budget and this one would add up** if one command both moved over words and edited
  at many cursors: soft wrap's `COMMAND_BUDGET` (ADR 0100), 128 KiB of rows a command and 2 KiB a
  splice, about 4 to 5 ms on the UI thread, and these 512 KiB, about 7 ms. Together they would pass
  a frame, so such a command would have to split one between them. None does today.
- **Two ways of reading clusters** live side by side: by offset for words, by point for drawing and
  line motion. The oracle tests keep their answers the same.

## Alternatives considered

- **Checking words in display points, as before:** each check and each step back is a seek and a
  conversion, and multi-cursor `M-f` on ordinary text costs several times more.
- **A stop's worth for each cursor, with no shared bound:** 64 cursors on boundary-less runs take
  over 50 ms, and 1000 would take seconds.
- **Charging each cursor's entry seeks to the 512 KiB:** at about a thousand cursors they alone
  spend it, and later cursors on ordinary text stop and signal.
- **A budget that grows with the cursors,** 512 KiB or 1.5 KiB a cursor, whichever is more: at
  1000 cursors the bytes it lets a key read on boundary-less runs take about 25 ms.
- **A smaller stop at several cursors, so that every cursor moves a little:** each key would move
  every cursor a shorter piece, and a run of 128 KiB would take more keys at each cursor than at
  one. Moving a few cursors a whole stop keeps one cursor's behaviour per cursor.
- **Capping `TAB`'s walk with a budget:** later cursors would land off their stops on long rows.
  The summaries already hold what the walk computes.
- **Shrinking `AFTER_WORD` and `BEFORE_WORD`:** they cap rather than charge, and reading by offset
  needs most of `BEFORE_WORD` for the longest query, so a smaller cap would reject whole words.
