# 0013. Persistence: emergency save only

- Status: Accepted
- Date: 2026-11-03

## Context

Periodic auto-save, backup files and persisted undo history all write buffer
contents to disk. If a buffer holds secrets, those copies may also end up in
system or cloud backups. The author already runs Emacs with auto-save, backups
and lock files off. A young oso will crash more often than a mature editor.

## Decision

- No periodic auto-save, no backup files (`file~`), no lock files and no
  persisted undo history.
- **Emergency save on panic:** unsaved buffers are written to a private
  directory (`0700`, files `0600`), skipping secret-looking files. When such a
  file is opened later, oso offers to recover it.
- **Safe save:** write a temporary file in the same directory, fsync it,
  rename it over the target and preserve permissions.

## Consequences

- Secrets reach disk only in the rare event of a crash.
- Power loss or OS-level kills can lose unsaved work.

## Alternatives considered

- **Emacs-style periodic auto-save to a private directory:** more resilient,
  but writes contents to disk regularly.
- **No recovery at all:** the safest option, but early oso bugs would cost
  work.
