# 0007. Multiple cursors

- Status: Accepted
- Date: 2026-11-03

## Context

Multiple cursors are wanted. Retrofitting them changes every command, so the
decision has to be made before commands are written.

## Decision

- Each window owns a **selection set**: one or more selections, each with a
  cursor and its own mark. The same buffer in two windows has independent
  selection sets, as with Emacs point.
- The command API operates on the selection set. A single cursor is a set of
  one.
- Ways to add cursors:
  - the next or previous occurrence of the word or region
  - a rectangle
  - all search matches
  - modifier + mouse click
- **Kill and yank are per cursor.** Killing with N cursors stores N pieces as
  one kill-ring entry. Yanking with N cursors pastes the matching piece at
  each one. When the counts differ, the pieces are joined with newlines.
- **Undo** treats one command's edits across all cursors as one step.

## Consequences

- Every command must be written for selection sets from the start. Helper
  functions cover the common "for each cursor" pattern.
- Emacs single-region semantics are a special case of the general model.

## Alternatives considered

- **Single cursor, as in Emacs:** simpler, but multiple cursors would be very
  costly to add later.
- **Selections shared per buffer:** loses the Emacs habit of editing two
  places in one buffer from two windows.
