# 0183. A click moves point

- Status: Accepted
- Date: 2026-11-26

## Context

A press of a mouse button selects the window under it, and nothing more in a text buffer
([ADR 0120](0120-the-terminal.md), step 4b-2: "point never moves in a text buffer"; and the
first press on a window not selected only selects it, plan Q5). That rule was made for
terminals: the first click on a terminal must not reach its program. The author asked on
2026-11-26 why the cursor does not go where they click.

## Decision

The author chose to move point on a click, and nothing else for now: no drag, no Shift-click, no
double or triple click, no click that adds a cursor. The rest was answered through the space's
orchestrator, as recommended.

- **Which presses.** A left press without Shift on the text area of a tiled window that shows
  a text buffer moves point to the cell under it. On a window not selected, the same press
  selects it and moves point, as Emacs's `mouse-set-point` does. The gutter, the modeline, the
  divider and the column of a wrapped row's mark only select, as before. Another button, or
  Shift held, does what it did. Terminals keep step 4b-2's rules, and `*Spaces*` in the side
  window keeps its own (ADR 0131, ADR 0140).
- **Where.** The cell is read through the rows the frame draws (`Screen`, ADR 0100, ADR 0104),
  so that soft-wrapped rows, the indent of a transcript's wrapped rows (ADR 0165) and a window
  scrolled sideways agree with what shows:
  - past the end of a line, point goes to its end;
  - past the last cell of a wrapped row its line goes on from, to the last character that row
    shows, not to the next row's start;
  - below the last row, to the end of the buffer;
  - on a character that takes several cells (a wide character, a tab, a control escape), to its
    start, as a goal column inside one lands.
- **What it does.** It is a plain motion: one cursor (extra cursors go), the region hidden, no
  mark pushed (as `mouse-set-point`: a click is too frequent to fill the mark ring), the goal
  column cleared. The window follows its cursor, and a review's other pane follows as after a
  key (ADR 0122). It changes no text and makes no undo record.
- **While oso holds input** (a prompt, a menu, isearch, a key sequence, a composition and the
  rest step 4b-2 lists), a press is dropped, as before. A composition is not committed by a
  click.
- **Which buffers.** Every text buffer: files, read-only buffers (a review and its panes, a
  transcript, git status, `*Messages*`), a session's composer docked under its transcript
  (ADR 0170). Only point moves: nothing a key such as `RET` does runs.
- **A keyboard macro** being recorded does not record the click, and the recording goes on.
- **Agents.** A click counts as the person's command (ADR 0118, Q9), as the selecting press
  did. No agent can click: presses come from the platform, or from the dev control socket's
  `mouse` call, which only dev builds have.
- **The public item:** `oso-core`'s `Editor::click_point(&mut self, id: WindowId, row: usize,
  column: usize) -> bool`, with `row` and `column` counted from the window's text area's top
  left.

## Changes to accepted ADRs (only when amending)

- [ADR 0120](0120-the-terminal.md), step 4b-2, "Where a press goes": "point never moves in a
  text buffer" and "on a window that is not selected, it selects it (plan Q5), and nothing more"
  now hold for terminals only. A left press on a text buffer's text moves point, selected or not.
- [ADR 0112](0112-window-splits.md), "A click selects the window under it since ADR 0120's step
  4b-2; point does not move": a left press on its text also moves point.
- [ADR 0091](0091-multiple-cursor-keys-and-m-y.md), "A cursor with a click waits for mouse
  clicks": oso takes clicks now, but a click that adds a cursor still waits.

## Consequences

- Dragging to select, Shift-click to extend the region, double and triple clicks and a click
  that adds a cursor can build on the same mapping of a cell to an offset.
- A replayed keyboard macro may not do what the person saw while recording it, when they clicked
  during the recording.

## Alternatives considered

- **The first press on a window not selected only selects it,** as for a terminal (plan Q5).
  Rejected: in a text buffer, moving point is harmless, and it is what Emacs does.
- **Pushing the mark, as `C-.` does,** so that `C-u C-SPC` comes back. Rejected: every click
  would push one.
- **Ending a keyboard macro's recording on a click.** Rejected for now; the recording goes on.
