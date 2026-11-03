# 0029. File changes on disk and editing while loading

- Status: Accepted
- Date: 2026-11-03

## Context

Files change on disk while open, for example after `git checkout` or when
another tool writes them. The author relies on Emacs's global auto-revert.
[ADR 0022](0022-text-buffer-and-file-loading.md) made buffers read-only while
they load, and left open whether M2 should allow editing during loading.

## Decision

- **Watch open files** with OS file notifications and detect changes
  immediately.
  - If the buffer has no unsaved changes, reload it automatically and keep
    cursor positions (mapped through the edit where possible).
  - If the buffer has unsaved changes, do not reload. Show a modeline
    warning, and ask for confirmation before saving over the newer file.
  - Saving through safe save re-checks the file's modification time right
    before the rename.
- **Buffers stay read-only while loading**, as in M1. Ordinary files load in
  an instant. Only huge files are affected.
- The file-watching dependency (for example `notify`) goes through the normal
  dependency approval ([ADR 0010](0010-process-and-network-policy.md)).

## Consequences

- Behaves like Emacs auto-revert, but reacts at once instead of polling.
- A new dependency, with platform-specific watcher backends on all three
  OSes.
- Editing a huge file has to wait until it finishes loading.

## Alternatives considered

- **Checking only on focus and before save:** no dependency, but changes go
  unnoticed while the window has focus.
- **Notification only, never reloading:** safe, but tedious after every
  checkout.
- **Editing the already-loaded part while loading:** complicates appending
  the rest, undo and anchors, for little gain.
