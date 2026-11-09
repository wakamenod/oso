# 0099. Checkpoints in long lines

- Status: Accepted
- Date: 2026-11-09

## Context

The TabMap finds the display point of an offset, the offset of a point, and the display cluster
around an offset in O(log n) plus a scan of one chunk
([ADR 0035](0035-sumtree-rope-and-cell-grid.md)). The scan starts at the chunk's start or its last
line break, and for a cluster also at the nearest two ASCII characters, and segments every cluster
on the way, up to 1008 bytes. On short lines that is a few bytes. On a long line it is up to a
whole chunk on every call.

Some callers read the same chunk many times:
- word motion steps back cluster by cluster;
- the keys that add cursors check each match they find against its clusters and words
  ([ADR 0091](0091-multiple-cursor-keys-and-m-y.md));
- clipping after an edit looks at clusters around each cursor.

On a long line each of those reads pays the whole scan again.

## Decision

The author decided this on 2026-11-09, through the coordinator.

- **A chunk keeps checkpoints in its long lines.** A checkpoint is a cluster start inside the
  chunk. One is taken at the first cluster start at or past every 128 bytes from the line's start
  or the checkpoint before. Only a line longer than 128 bytes gets any.
- **A checkpoint holds what a scan there needs:**
  - its offset in the chunk;
  - the line breaks before it in the chunk;
  - the column function from its line's start, or from the chunk's start on the chunk's first line;
  - the offset before which characters were past a display cluster's limits there.

  It takes 16 bytes.
- **Scans start at the last checkpoint before what they look for**, when it is past the line's
  start. This covers an offset's point, a point's offset, the clusters of a row from a column, and
  the cluster around an offset. The cluster around an offset first looks for two ASCII characters
  within a stride before it, from the bytes alone, and needs no checkpoint when it finds them.
  Once a chunk has checkpoints, a scan in one of its long lines reads at most a stride and a
  cluster, plus what the segmenter looks back over.
- **Exact by construction:**
  - At a checkpoint the scan keeps the whole chunk as context, as a display cluster can start
    inside a grapheme cluster. It also starts in the state the scan from the chunk's start had
    there, so it finds the same clusters after it.
  - Elsewhere it reads from a line's start or an ASCII pair, as before.
  - A display cluster still depends only on its chunk's text and where the chunk starts
    ([ADR 0092](0092-edits-in-place-for-many-cursors.md)).
  - Clusters and words follow UAX #29 as before. No rule is loosened.
- **Made on demand, once a chunk has been read 8 times.** A chunk makes its checkpoints in the
  scan that reads it the eighth time, and keeps them for good, shared by every snapshot that holds
  the chunk.
  - Typing reads a chunk a few times between two edits, and an edit makes a new chunk, so typing
    never makes them.
  - A search that reads one chunk thousands of times makes them after its first few reads.
  - Loading and editing cost what they did, and only chunks read over and over hold checkpoints.
- **The budgets of what a key may read stay as they are.** A capped check makes the same
  decisions and stops at the same step. Once its chunk has checkpoints, a seek reads less than
  `SEEK_COST` charges for it.
- **A `OnceLock` in each chunk, the repo's first exception to the rule against them.** The rule
  bans `OnceLock` as a way to keep global state (ADR 0038). Here each TabMap item keeps its own,
  so that its checkpoints are made once and shared by every snapshot that holds the item, on any
  thread, with no lock on a read once they exist. Nothing outside the item reaches it, and it goes
  with the item. It is written in ADR 0038's exception form, an
  `#[expect(clippy::disallowed_types, reason = "...")]` at each use, citing this ADR.

## Changes to accepted ADRs

- [ADR 0035](0035-sumtree-rope-and-cell-grid.md), "the column of any offset is found in O(log n)
  plus a scan of one chunk": on a line longer than 128 bytes, once its chunk has checkpoints, the
  scan is of at most a stride and a cluster, plus what the segmenter looks back over.

## Consequences

Measured with `text_perf` in a release build on a MacBook, main against this change; runs differ
by about 5% from one to the next:

| | main | with checkpoints |
|---|--:|--:|
| `DisplayMap::new`, 100 MB of source | 31.4 ms | 31.4 ms |
| first screen, 64 KB | 153 µs | 159 µs |
| an edit and its sync, 100 MB, p50 | 2.25 µs | 2.25 µs |
| typing at 1 cursor, 1 MB, p50 | 1.58 µs | 1.67 µs |
| typing at 100 cursors on a 10 MB line, p50 | 490 µs | 507 µs |
| drawing 60 rows | 27 µs | 27 µs |
| offset to display point, random, on a 1 MB line | 3.5 µs | 0.93 µs |
| `M-b` on a 10 MB line, p50 | 16.5 µs | 11.8 µs |
| `M-f` with a count past what a key reads, 10 MB of `a ` | 36 ms | 7.2 ms |

One `M-f` on a long line reads its chunk too few times to make checkpoints, and costs what it did
(12.2 µs and 12.5 µs): that is the price of leaving typing as it was.

- **Memory, for 100 MB of 10 KB lines** (peak resident size):
  - the per-chunk fields add 7 MB (242 to 249 MB);
  - if every chunk is read often enough to make its checkpoints, they add 20 MB (to 262 MB).
- **Every chunk is 32 bytes larger,** 96 bytes in place of 64, for the checkpoints it may make
  and the count of reads before them.
- **What reads one long line over and over,** a search or a motion with a large count, reads about
  a fifth of what it did. Whether the keys that add cursors should check words in offsets rather
  than in display points is left open (see `open-questions.md`).

## Alternatives considered

- **Checkpoints made with every chunk, as it is loaded or edited:** loading long lines took 13%
  longer, and typing in a long line twice as long, as every key rebuilt the edited chunk's.
- **Made at the first read:** one `M-f` on a long line would be six times faster, but typing in a
  long line paid the same as above, moved from the edit to the read after it.
- **A stride of 64 or 32 bytes:** 1.3 and 1.6 times faster again, for twice and four times the
  memory.
- **Smaller chunks:** rejected in ADR 0035 for building time and the number of items, and they
  would cost every buffer, not only long lines.
- **Checking words in offsets instead of display points:** would take out the windows the word
  checks read, but changes those checks. A later ADR decides it.
- **A per-chunk first boundary only:** a chunk's start already is one, so it would change nothing.
