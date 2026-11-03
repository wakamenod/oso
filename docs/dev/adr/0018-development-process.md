# 0018. Development process

- Status: Accepted
- Date: 2026-11-03

## Context

All development is done with Claude Code. Human review time is the scarce
resource. See [`agentic-workflow.md`](../agentic-workflow.md).

## Decision

- **Features:** worktree branch → PR → CI on all three OSes → human gate
  review → merge.
- **Small chores** may go directly to `main`.
- **Human gates:**
  - ADRs and design decisions
  - new dependencies
  - new `unsafe`
  - security-sensitive code
  - merges to `main`
- Everything else passes on agent review: reviewer subagents plus
  `REVIEW.md`.
- Rules are enforced mechanically where possible, then by review, then by
  documentation.
- `just ci` must pass before every commit, and a hook enforces it.
- CI runs on GitHub Actions: Linux, macOS and Windows.

## Consequences

- The author reviews only what matters most.
- The tooling (hooks, skills, reviewer agents) must be built early. It is
  planned as part of M1.

## Alternatives considered

- **Reviewing every diff by hand:** too slow for agentic development.
- **Committing everything to `main`:** loses CI on all platforms before merge.
