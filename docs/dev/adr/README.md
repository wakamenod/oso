# Architecture Decision Records

Each ADR records one decision: its context, the decision, its consequences
and the alternatives considered. ADRs are not rewritten after acceptance. To
change a decision, add a new ADR that supersedes the old one, and update the
old one's status.

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
## Consequences
## Alternatives considered
```
