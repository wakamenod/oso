# 0014. Panic strategy

- Status: Accepted
- Date: 2026-11-03

## Context

Commands will be added constantly, and some will have bugs. A bug in one
command should not take down the editor.

## Decision

- Keep `panic = "unwind"` in all profiles.
- Catch panics at the command boundary, report them in the echo area and a
  log buffer, and keep running.
- A panic outside the command boundary triggers the emergency save
  ([ADR 0013](0013-persistence.md)) before exiting.

## Consequences

- A buggy command costs one error message, not the session.
- State touched by a panicking command may be partially updated. Commands
  should stage changes before applying them where practical.
- Binaries are slightly larger than with `abort`.

## Alternatives considered

- **`panic = "abort"`:** smaller binaries, but every command bug becomes a
  crash.
