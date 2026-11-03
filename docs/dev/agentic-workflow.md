# Agentic development workflow

oso is developed with Claude Code. This document describes how agents work
on the project and where humans stay in the loop. The decision record is
[ADR 0018](adr/0018-development-process.md).

## Rules: enforce, review, then document

Agents can miss rules buried in long documents. Rules are therefore handled
in three tiers:

1. **Enforce mechanically** whatever tooling can check: clippy lints,
   `cargo-deny`, hooks, permission settings and CI.
2. **Review** what needs judgment, using [`REVIEW.md`](../../REVIEW.md) and
   reviewer subagents.
3. **Document** intent and rationale in [`AGENTS.md`](../../AGENTS.md), these
   docs and ADRs.

When an agent makes a mistake, add a rule to prevent it from happening again.
Use the first tier that can catch it. Keep `AGENTS.md` short and prune it
regularly.

## Files

```
AGENTS.md                   always loaded: priorities, hard rules, workflow
crates/*/AGENTS.md          crate-specific rules (planned)
REVIEW.md                   review checklist
docs/dev/                   design docs, ADRs, open questions
.claude/settings.json       permissions and hooks (planned)
.claude/skills/             repeatable procedures (planned)
.claude/agents/             reviewer subagents (planned)
```

## Planned tooling

### Permissions and hooks

- Ask for human approval before editing `Cargo.toml`, `Cargo.lock`,
  `deny.toml` or `.claude/`, and before running `cargo add`.
- Pre-commit hook: run `just ci` and block the commit if it fails.
- Post-edit hook on `.rs` files: run `cargo fmt` (nothing heavier).
- Pre-commit secret scan with gitleaks.

### Skills

- `add-command`: implement the function, register it with a docstring and
  argument spec, add a key binding, write unit tests and an end-to-end test,
  and regenerate the command reference.
- `verify-in-app`: start a dev build, drive it over the control socket and
  check screenshots.
- `adr`: record a decision with the standard template.
- `add-dependency`: research maintenance, `unsafe` usage, transitive
  dependencies and `cargo-deny` results, report back, and wait for approval.

### Reviewer subagents

Read-only `security-reviewer` and `perf-reviewer` agents follow the matching
sections of `REVIEW.md`. They run in parallel after implementation, and their
findings are fixed before human review.

## Flow of a feature

```
plan → implement in a worktree → tests → verify in the app
→ reviewer subagents → PR → 3-OS CI → human gate review → merge
```

Small chores may go directly to `main`.

## Human gates

The author reviews only these, and everything else passes on agent review:

- design decisions (ADRs)
- new dependencies
- new `unsafe`
- security-sensitive code: process spawning, file writing, the trust model,
  the control socket
- merges to `main`

## Documentation language

- Everything in the repository is English: code, comments, identifiers,
  commits and docs.
- User documentation is English by default, with a Japanese edition.
- The repository is public. See
  [`security.md`](security.md#what-must-never-be-committed) for what must
  never be committed.
