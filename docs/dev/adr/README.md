# Architecture Decision Records

Each ADR records one decision: its context, the decision, its consequences
and the alternatives considered. ADRs are not rewritten after acceptance. To
change a decision, add a new ADR:

- **Superseding** replaces the whole decision. The old ADR's status becomes
  `Superseded by NNNN`.
- **Amending** changes part of it. The new ADR has a `Changes to accepted
  ADRs` section that quotes or names each passage it changes and says what
  holds now. The old ADR's status becomes `Accepted, amended by NNNN` (more
  numbers are added with `and` or commas).

In the old ADR, only the status line changes. Its text stays as it was.

The ADRs are the numbered files in this directory, and each one's `Status:` line is
the only record of its status. `just adrs` lists them with their status, and
`just adrs Proposed` lists the ones still waiting. There is no table here: every pull
request that added a row conflicted with every other.

## Template

```markdown
# NNNN. Title

- Status: Proposed | Accepted | Superseded by NNNN
- Date: YYYY-MM-DD

## Context
## Decision
## Changes to accepted ADRs (only when amending)
## Consequences
## Alternatives considered
```
