# 0125. A rewrap starts at a row that reads the edit

- Status: Accepted
- Date: 2026-11-13

## Context

[ADR 0100](0100-soft-wrap.md) says where the rewrap after an edit starts:

> It starts at the view row before the one that holds the rebuilt chunks' start. It starts
> further back while those rows are shorter than the rules' reach, two clusters.

The preedit is laid out from the same place (ADR 0100, "The input method on a wrapped row"), with
the cursor in place of the rebuilt chunks' start.

The rule assumes that a row reads at most two clusters past its end. That does not hold. To decide
where a row breaks, the rules read on from the row's start to the first cluster that does not fit,
and one more for a hanging space. Kinsoku then moves the break back from there. So a row that broke
at its last break before a stretch with none reads the whole stretch. For example, at 10 columns
`aaあ。。  。` wraps as `aa`, `あ。。  ` and `。`: no row may start with `。`, even after
spaces. A `b` typed before the last `。` lets a row start after the spaces, and the last space
hangs. So `aa` takes everything up to the `b`. The rewrap started at `あ`, a row too late, and kept
`aa` on its own:

- **A preedit** showed `aa` alone while composing, and the text jumped on commit. A property test
  found this.
- **An edit** did the same when the rope's rebuilt chunks started at such a point. The rows stayed
  wrong until a later rewrap covered them.

The author chose to fix the rule for both, through the coordinator, on 2026-11-13.

## Decision

- **A rewrap also starts further back while a row before reads the edit's start:** while the
  cluster before it starts within that row's width from the row's start.
  - That cluster then fits the row, or is the first that does not. In both cases, the row reads the
    cluster at the edit's start.
  - A zero-cell cluster before the edit's start is passed over, and the cluster before it is used.
    That steps back over more rows, never fewer.
- **The short-row rule stays,** so a rewrap starts at the same row as before or an earlier one.
- **The preedit's layout follows,** as it starts where an edit at the cursor would start its
  rewrap.

## Changes to accepted ADRs

- **[ADR 0100](0100-soft-wrap.md), "Where a rewrap starts":** "It starts further back while those
  rows are shorter than the rules' reach, two clusters" now continues: "or while a row before reads
  the rebuilt chunks' start, the cluster before it starting within the row's width".

## Consequences

- An edit or a preedit lays out the same rows as a wrap from scratch, also after a row that broke
  early.
- The rows stepped back over start within one row's width of the edit's start, so the extra work
  is at most one row's width of short rows on the edit's line.
- The wrap tests now edit at exact offsets as well. Before, every edit started at a rebuilt chunk's
  start, which in a short text is the buffer's start, so no row before the edit was ever kept.

## Alternatives considered

- **Fix only the preedit's start:** leaves edits that start at such a point wrong. Those are rare,
  but their rows stay wrong.
- **Rewrap from the logical line's start:** ADR 0100 turned this down for long lines.
