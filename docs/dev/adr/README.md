# Architecture Decision Records

Each ADR records one decision: its context, the decision, its consequences
and the alternatives considered.

## Changing an accepted ADR

**A new ADR** is needed for a new decision, for reversing or replacing a
decision, and for any change to a module, crate or layer (ADR 0038):

- **Superseding** replaces the whole decision. The old ADR's status becomes
  `Superseded by NNNN`.
- **Amending** changes part of it. The new ADR has a `Changes to accepted
  ADRs` section that quotes or names each passage it changes and says what
  holds now. The old ADR's status becomes `Accepted, amended by NNNN` (more
  numbers are added with `and` or commas).

**In place**, in the accepted ADR itself, only these:

- **As built:** what a step of the decision turned out to be when it was
  built, with the choices the author made on the way and the date. One
  `## As built` section with a subsection per step, or one section per step
  (`## Step 2, as built`, or `#### As built (step 2, YYYY-MM-DD)` under the
  step). See ADRs 0121, 0122 and 0136.
- **A narrow amendment of one rule:** a section titled `## <What changed>,
  as amended`. It opens with `Amended on YYYY-MM-DD`, says whose choice it
  was and the problem it fixes, then gives the rule that holds now. The
  status line names it, after any ADR numbers: `Accepted, amended by 0143;
  "Waiting rows blink, as amended" (2026-11-21)`. See ADRs 0121 and 0140.
- **A correction or wording fix:** an `Erratum (YYYY-MM-DD, …)` bullet under
  the passage it corrects, which stays as it was. See ADRs 0089 and 0111.

Text above these sections is not rewritten; a later section says what holds
now. If an in-place change would reverse the decision or touch a module,
crate or layer, it needs a new ADR instead.

## Listing

The ADRs are the numbered files in this directory, and each one's `Status:` line is
the only record of its status. `just adrs` lists them with their status, and
`just adrs Proposed` lists the ones still waiting. There is no table here: every pull
request that added a row conflicted with every other.

## Template

```markdown
# NNNN. Title

- Status: Proposed | Accepted | Accepted, amended by NNNN | Superseded by NNNN
- Date: YYYY-MM-DD

## Context
## Decision
## Changes to accepted ADRs (only when amending)
## Consequences
## Alternatives considered
```
