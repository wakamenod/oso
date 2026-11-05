# User interface (M2)

This document describes the screen layout and visual behavior that M2
builds. The decision record is [ADR 0026](adr/0026-m2-ui-layout.md); the
theme is in [ADR 0027](adr/0027-theme.md). UI for later features is designed
before each feature is built.

## Layout

```
+--------------------------------------------------------------+
|              +--------------------------------+              |
|              | M-x: find_                     |  minibuffer  |
|              | > find-file                    |  (top center)|
|              |   find-file-other-window       |              |
|              +--------------------------------+              |
| code code code code         | code code code code            |
| code code code   +---------------------+   code code         |
| code code code   |  menu (center)      |   code code         |
| code code code   +---------------------+   code code         |
| code code code code         | code code code code            |
|-modeline (active)-----------|-modeline (inactive)------------|
| echo area: one line for messages                             |
+--------------------------------------------------------------+
```

All floating UI uses the floating windows of
[ADR 0008](adr/0008-floating-ui.md), with a border and padding similar to
posframe.

`oso_core::frame` divides the frame into whole cells
([ADR 0057](adr/0057-frame-layout.md)). The modeline and the echo area take the
two bottom rows. A frame too small for all of it gives up the modeline first and
the echo area next. The pixels left over go below the echo area, in the background
color, so no partly shown row is drawn.

## Minibuffer and completion

- The minibuffer floats at the **top center** of the frame.
- It shows **18 candidate rows** by default (configurable), with fuzzy
  matching (orderless-style) and annotations on the right (marginalia-style).
- **Preview inside the popup.** For commands with a preview, such as buffer
  switching, ripgrep search and line search, the popup splits: candidates on
  the left, a preview of the selected candidate on the right. Commands
  without a preview, such as `M-x`, use a single-column popup.
- The preview is read-only, and it only renders already-open buffers or
  files the trust model allows reading.

## Menus

Hydra/transient-style menus float at the **center** of the frame, as they do
with hydra-posframe today.

## Echo area

- A single line at the bottom of the frame shows messages such as "Saved"
  and errors, including caught command panics.
- The minibuffer no longer lives there, so the line is for messages only.
- Every message is also appended to the `*Messages*` buffer.

## Modeline

- Each window has its own modeline. The active window's modeline is visually
  distinct.
- M2 contents:
  - buffer name and modified marker
  - line and column, plus the cursor count when there are several cursors
  - major mode
  - encoding and line endings
  - loading progress, while a file is loading
  - a warning when the file contains bidirectional control characters
  - a warning when the file changed on disk while the buffer has unsaved
    changes
- M3 adds the project trust indicator.

## Cursor

- A **box** cursor that does **not blink**. It covers its character's
  cells: two for a full-width character, the whole escape for `\xFF`, one
  on a tab (built in M1-6).
- With multiple cursors, secondary cursors use a dimmer color.
- After a large jump or scroll, the cursor position flashes briefly
  (beacon-style).

## isearch

- All matches in view are highlighted, and the current match is emphasized.
- The match count ("3/17") is shown, anzu-style.
- The search string is shown in the echo area.

## Input method (IME)

- Preedit text is drawn **inline at the cursor, underlined**, and pushes the
  text after it along its row. It is never written to the buffer
  ([ADR 0051](adr/0051-m2-plan-decisions.md)). The OS candidate window is placed
  just below the cursor, as in other macOS apps.
- The IME is switched off automatically when entering the minibuffer,
  isearch or a menu, since those use single keys.

## Visual aids

On by default and toggleable from the toggle menu:

- **Whitespace visualization**: full-width spaces shown as `□`, plus trailing
  whitespace and tabs.
- **Cursor flash** after large jumps.
- **Current line highlight.**

Off by default: line numbers.

## Bidirectional and invisible characters

- Bidirectional control characters and invisible characters are drawn as
  their code point in a colored box (for example `U+202E`). They are never
  rendered invisibly. A 1-pixel gap separates neighboring boxes
  ([ADR 0051](adr/0051-m2-plan-decisions.md)).
- A display cluster holds at most 8 marks and 16 characters. Each character past
  that is drawn the same way, but ASCII, which stays text. A new chunk starts the
  limits over ([ADR 0054](adr/0054-limits-on-a-display-cluster.md)).
- A buffer containing bidirectional control characters shows a warning in its
  modeline (Trojan Source, CVE-2021-42574).
