# 0043. No edit approval in auto mode

- Status: Accepted
- Date: 2026-11-04

## Context

[ADR 0021](0021-development-tooling.md) gates edits to dependency manifests,
check definitions and `.claude/` with permission prompts. Those prompts reach
a human only in the default permission mode, so the ADR told agents in auto
mode to ask in the conversation before editing those files. The author works
in auto mode and does not want to be asked about file changes.

## Decision

- **In auto mode, agents do not ask before changing files.** This includes
  the files listed as needing approval in `AGENTS.md`. This replaces the
  "must ask in the conversation" rule of ADR 0021.
- **Agents still ask about decisions:**
  - design changes and ADRs
  - adopting a new dependency
- Outside auto mode, the permission prompts work as before.
- The human gates of [ADR 0018](0018-development-process.md) still apply at
  review time: changes to these files remain visible in the pull request.

## Consequences

- Less interruption, and no more approvals that only repeat a decision
  already made.
- An agent in auto mode can loosen a check without asking. Review is the
  defense: `REVIEW.md`, the reviewer subagents and CI.

## Alternatives considered

- **Keeping the conversation prompt in auto mode:** it interrupts the author
  for edits that review catches anyway.
