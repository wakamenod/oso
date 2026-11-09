# 0102. Soft wrap: what building it changed

- Status: Accepted
- Date: 2026-11-10

## Context

Building [ADR 0100](0100-soft-wrap.md)'s WrapMap, its budget and its worker (PR #129) measured
what the ADR had estimated, and a review probed it. Some of the ADR's figures and passages did
not hold. The rules, the motion and everything a person sees stay as ADR 0100 decided.

## Decision

The author approved the budget on 2026-11-10, through the coordinator. The rest follows from
the measurements.

- **The budget is 128 KiB of rows a command, each splice charged 2 KiB more,** in place of
  256 KiB.
  - Before it reads a row, a splice's rewrap seeks into a long line several times, about 35 µs.
  - CJK costs about 36 ns a byte to wrap, ASCII about 15.
  - 256 KiB came to about 16 ms for a thousand cursors; this budget comes to about 4 to 5 ms.
- **A command's splices are rewrapped in the order they were applied,** each with the display
  snapshots from before and after it, in place of one ascending pass over the rebuilt ranges.
  - The edit core syncs the display map once a splice, so each snapshot costs one O(1) clone.
  - Each splice's rewrap is then checked against a wrap from scratch on its own.
  - Once the budget is spent, the rest of the command is joined in one go: what it changed,
    each splice's range taken back through those applied before it, becomes one provisional
    span and one stale range. So a thousand cursors pay for the budget, not for each cursor.
- **An item holds at most 64 rows, of one logical line or several,** in place of 64 breaks within
  one TabMap chunk, which rows do not align with.
  - A line that wraps starts an item of rows, which takes the lines around it until 16 that fit
    come in a row.
  - A run of lines that fit is an item of at most 256 lines.
- **A row in an item of rows costs about 9 bytes,** items and summaries counted, in place of
  8 bytes a break. Measured with `text_perf -- wrap`:
  - 100 MB of prose at 40 columns takes about 22 MB a window;
  - 100 MB of source takes about 46 MB at 40 columns and 1 MB at 80;
  - a 10 MB line takes about 1 MB;
  - 1 GB of source at 80 columns takes about 7 MB, and 26 s on the worker.
- **A line of printable ASCII and tabs that fits is found from its bytes,** with no cluster read.
- **Columns are compared modulo the tab width only when a tab follows on the line.**
  - With none, a row's cells do not depend on where it starts.
  - The next row with a tab is found once a rewrap and kept, so a long line is walked once.
- **A space hangs into the mark's column only where the row may break after it,** or before more
  spaces, which start the next row. So a no-break space or kinsoku after it holds.
- **The kinsoku sets** take, besides JIS X 4051's core sets:
  - at a row's start: the iteration marks `々ゝゞヽヾ`, `〜`, `‼`, the closing quotes `”’`, and the
    halfwidth `｡｣､･ｧｨｩｪｫｬｭｮｯｰ`;
  - at a row's end: `“‘｢`.
- **The worker comes second in the build order,** before anything shows wrapped rows.
  - Each edit rewraps the chunks the rope rebuilt around it, about 700 bytes.
  - So without the budget, a thousand cursors would rewrap about 700 KB a key.

## Changes to accepted ADRs

The passages below stay in their ADR as written; what holds now follows each. ADR 0100's status
becomes `Accepted, amended by 0102`.

- [ADR 0100](0100-soft-wrap.md), "a byte budget, 256 KiB a command, as a key's is (ADR 0091)":
  128 KiB of rows a command, each splice charged 2 KiB more.
- ADR 0100, "A command's splices rewrap in one pass, ascending, over the ranges the edit rebuilt
  (`changed`, …), after all of them are applied": in the order they were applied, each with its
  snapshots, and the rest joined in one go once the budget is spent.
- ADR 0100, "at most 64 breaks within one TabMap chunk": at most 64 rows, of one line or several;
  a run of at most 256 lines that fit.
- ADR 0100, "A break costs at most 8 bytes, so a wrap takes at most 8 bytes per view row that
  continues. 100 MB of one-line paragraphs at 40 columns comes to about 2.5 million breaks, about
  20 MB a window": about 9 bytes a row in an item of rows, with the figures above.
- ADR 0100, "Whether a line fits is found by reading its clusters": from its bytes when it is
  printable ASCII and tabs.
- ADR 0100, "with the same column modulo the tab width": when a tab follows on the line.
- ADR 0100, "One more may take the mark's column": where the row may break after it, or before
  more spaces.
- ADR 0100, "Kinsoku uses the core sets of JIS X 4051": and the characters above.
- ADR 0100, "Order: … 2. The offset top, the window's wrap and the toggle. 3. The renderer, the
  frame, isearch and the input method. 4. The worker.": the worker second, then the offset top,
  then the renderer.

## Consequences

- A command at a thousand cursors on a 10 MB line costs about 6.5 ms with words or no spaces and
  11 ms with CJK, the edit core's 5 ms included.
- Two windows on one wrapped buffer, or a narrow window on source, take tens of MB.
- A window's rows can be provisional for a frame or more after a large command, until the worker
  reports.

## Alternatives considered

- **Keep 256 KiB:** about 16 ms for a thousand cursors, two frames.
- **One ascending pass over the rebuilt ranges, with only the snapshots from before and after the
  command:** saves a clone a splice, but every old position must be mapped through every splice,
  and each splice's rewrap could no longer be checked on its own.
- **One item per wrapped line:** a line that wraps once costs about 160 bytes a continuing row.
