# 0011. Git integration

- Status: Accepted
- Date: 2026-11-03

## Context

A git porcelain subset is needed: status, hunk staging and reverting, commit,
push/pull, log/diff/checkout by ref, rebase and conflict resolution. Running
`git` in an untrusted repository can execute commands from its config: for
example `core.fsmonitor`, filter drivers, textconv and external diff.

## Decision

- **Read-only information** (branch, status, diff gutter) is read in-process
  with `gix`. Config-defined commands are never run. Verifying that `gix` can
  be forced to skip them is an open item.
- **Write operations** call the `git` CLI through the process module, only in
  trusted projects ([ADR 0009](0009-project-trust.md)), because `gix` does
  not cover them fully and hooks run.
- Passphrase and credential input never enters history, the kill ring or
  macros.
- The command subset is chosen from the author's actual usage while building
  M4.

## Consequences

- Git status is safe to show in any repository.
- Write operations behave exactly like the user's git, including hooks and
  credential helpers.

## Alternatives considered

- **Shelling out to `git` for everything:** simple, but unsafe in untrusted
  repositories.
- **`gix` for everything:** not feature-complete for write operations.
