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
the echo area next. The pixels left over go below the echo area and at the right edge,
in the background color, so no partly shown row or column is drawn
([ADR 0061](adr/0061-panes-clipping-and-layers.md)).

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
  and errors, including caught command panics, until the next key.
- The minibuffer no longer lives there, so the line is for messages only, and for
  what is being typed: `C-x-` or `C-u 5-`, which comes ahead of any message.
- Every message is also appended to the `*Messages*` buffer, which `C-h e` shows, and until
  `C-x b` (M2-19) leaves. What a command says while a secret buffer is shown stays out of
  `*Messages*` and the log, and goes from the line once the window moves on. An
  undefined key and "Quit" reach only the line
  ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)).

## Emergency copies

After a crash, oso offers what it saved ([ADR 0064](adr/0064-recovering-an-emergency-copy.md)). Nothing
takes a key from the person:

- A notice in the echo area and `*Messages*`: `notes.txt has an emergency copy, from 12 min ago: C-h r`.
- `[emergency copy]` first on the modeline of a buffer whose file has one, until it is answered.
- `C-h r` shows `*Emergency Copies*`, one row per copy, the newest first, and goes back from it.

```
3 emergency copies.  v look  = compare  r recover  d delete  g refresh  q back

todo.txt        12 min ago  panic    1234 B  file unchanged   /home/me/notes/todo.txt
plan.txt        2 days ago  exit      420 B  not open         /home/me/notes/plan.txt
1a2b3c4d5e6f…   3 days ago  ?          88 B  unknown          /state/oso/emergency/1a2b3c4d5e6f7a8b-1790000000-77-0.txt
```

- A row starts with the file's name, taken from its path, as a window cuts a long row at its edge. Names line up, and take at most 32 cells. The whole path comes last.
- `v` or `RET` shows the copy's text, read-only, and `=` a line diff with its file. `q` goes back.
- `r` shows the text, and `r` again there puts it in the file's buffer as one undo step. Nothing is
  written until `C-x C-s`, and the copy goes once that has reached the disk.
- `d` says what it does, and `d` again deletes that copy for good. Held down, a letter here answers once.
- A copy of a file not open needs `oso PATH`, or later `C-x C-f`. Its row gives the path.

## Modeline

- Each window has its own modeline. The active window's modeline is visually
  distinct.
- M2 contents:
  - a warning when the file contains bidirectional control characters
  - a warning when the file changed on disk while the buffer has unsaved
    changes
  - a warning when an emergency copy of the file waits for an answer
  - buffer name and modified marker
  - loading progress, while a file is loading
  - line and column, plus the cursor count when there are several cursors
  - major mode
  - encoding and line endings
- The warnings come first, so that no name can push them off the row. A long name is
  cut to what the rest leaves.
- A buffer that may hold secrets shows `secret` for the line, the column and the cursor
  count, and a file not yet judged shows none of them.
- A frame of two rows has no modeline, and its warnings go first on the echo row. A frame of
  one row has no place for them ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)).
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
  ([ADR 0051](adr/0051-m2-plan-decisions.md)). The segment being converted has a
  thicker underline, as in other macOS apps. With several cursors, the preedit shows at
  the primary one, and the commit goes to every cursor
  ([ADR 0069](adr/0069-macos-input-method.md)).
- The cursor box goes where the input method's caret is: usually just after the
  preedit, on the text it pushed.
- The OS candidate window is placed just below the cursor, as in other macOS apps.
- Each commit undoes as one step.
- The IME is switched off automatically when entering the minibuffer,
  isearch, a menu or the list of emergency copies, since those use single keys. It is
  also off while a prefix key (`C-x`) or a prefix argument (`C-u`, `M-5`) waits for the
  next key, in read-only buffers and in buffers that may hold secrets. It goes off only
  once a composition has ended.
- `SPC` types a half-width space with a Japanese input source too. Option-Space
  (`s-SPC`, `insert-full-width-space`) types a full-width one.

## Visual aids

On by default and toggleable from the toggle menu:

- **Whitespace visualization**: full-width spaces shown as `□`, plus trailing
  whitespace and tabs.
- **Cursor flash** after large jumps.
- **Current line highlight.**

Off by default: line numbers.

## Bidirectional and invisible characters

- Text stays in logical order, left to right, in every script, so right-to-left text
  can be hard to read, and its order on screen is the file's
  ([ADR 0066](adr/0066-text-stays-in-logical-order.md)).
- Bidirectional control characters and invisible characters are drawn as
  their code point in a colored box (for example `U+202E`). They are never
  rendered invisibly. A 1-pixel gap separates neighboring boxes
  ([ADR 0051](adr/0051-m2-plan-decisions.md)).
- A display cluster holds at most 8 marks and 16 characters. Each character past
  that is drawn the same way, but ASCII, which stays text. A new chunk starts the
  limits over ([ADR 0054](adr/0054-limits-on-a-display-cluster.md)).
- A buffer containing bidirectional control characters (the nine of Trojan Source,
  CVE-2021-42574) shows a warning in its modeline.
