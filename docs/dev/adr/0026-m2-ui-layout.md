# 0026. M2 UI layout

- Status: Accepted
- Date: 2026-11-03

## Context

M2 is where daily use starts, so the minibuffer, messages, modeline, cursor,
isearch and IME display must be decided before it is built
([ADR 0008](0008-floating-ui.md) fixed only how floating UI is built). The
author's current setup is the starting point:

- a centered floating minibuffer with 18 rows
- centered hydra menus
- a per-window modeline
- a non-blinking box cursor
- a beacon-style cursor flash
- whitespace visualization

Emacs's bottom minibuffer is hard to see.

## Decision

- **Minibuffer:**
  - floats at the top center
  - 18 candidate rows by default, configurable
  - previews are shown inside the popup: candidates on the left, preview on
    the right
- **Menus** float at the center of the frame.
- **Echo area:** one line at the bottom of the frame for messages only. All
  messages are logged to `*Messages*`.
- **Modeline:**
  - one per window; the active one is distinct
  - M2 contents: buffer name and modified marker, line and column (plus the
    cursor count), major mode, encoding and line endings, loading progress,
    and warnings for bidirectional controls and on-disk changes
- **Cursor:** a box that does not blink; secondary cursors are dimmer.
- **isearch:** all matches highlighted, current match emphasized, "n/m"
  count shown, search string in the echo area.
- **IME:**
  - preedit is drawn inline at the cursor and underlined
  - the OS candidate window sits just below the cursor
  - the IME is switched off automatically in the minibuffer, isearch and
    menus
- **Visual aids on by default:**
  - whitespace visualization (full-width space as `□`)
  - cursor flash after large jumps
  - current line highlight
- Line numbers are off by default.
- **Bidirectional and invisible characters** are drawn as boxed code points.
  Buffers containing bidirectional controls show a modeline warning.

Details and a layout sketch are in [`ui.md`](../ui.md).

## Consequences

- Floating UI sits in two fixed places: top center for the minibuffer and
  center for menus. Messages always appear at the bottom.
- The popup preview needs a read-only text view inside a floating window.
  The preview is limited to already-open buffers and files the trust model
  allows reading.
- Per-window modelines must be cheap to redraw, because they update on every
  cursor move.

## Alternatives considered

- **Minibuffer at the frame center (current Emacs setup), at the center of
  the active window, or at the bottom (Emacs default).** Top center was
  chosen.
- **Previewing in the window behind the popup (consult style):** keeps the
  popup small, but moves the real window during selection.
- **Toast notifications for messages:** frees the bottom line, but messages
  are easy to miss.
- **A single global status bar (VS Code style):** less familiar than
  per-window modelines when windows are split.
