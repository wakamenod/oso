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
[ADR 0008](adr/0008-floating-ui.md). A prompt's float is a panel attached to the top
edge, close to VS Code's Quick Input
([ADR 0075](adr/0075-a-float-as-a-panel-attached-to-the-top-edge.md)).

`oso_core::frame` divides the frame into whole cells
([ADR 0057](adr/0057-frame-layout.md)). The modeline and the echo area take the
two bottom rows. A frame too small for all of it gives up the modeline first and
the echo area next. The pixels left over go below the echo area and at the right edge,
in the background color, so no partly shown row or column is drawn
([ADR 0061](adr/0061-panes-clipping-and-layers.md)).

## Minibuffer and completion

- The minibuffer **hangs from the top edge** of the text area, centered, over its first rows
  ([ADR 0072](adr/0072-floating-windows-the-minibuffer-and-questions.md),
  [ADR 0075](adr/0075-a-float-as-a-panel-attached-to-the-top-edge.md)). It is a panel only a
  little lighter than the text, with its bottom corners rounded, a soft shadow and a hairline.
- It takes **60% of the frame's columns**, at least 40 and never closer than 2 to an edge,
  with 2 columns of padding at its sides. It does not change while typing.
- The label has rows of its own. Below it, after a row of padding, the answer is typed in a
  field with a thin accent outline, and a row of padding ends the panel.

```
┌────────────┬──────────────────────────────────────┬────────────┐
│# Meeting no│  Go to line:                         │            │
│            │                                      │            │
│Agenda      │ [42█                 [Past the end]] │            │
│- budget for╰──────────────────────────────────────╯            │
│▯ hiring: two engineers                                         │
```

The panel's top is the window's edge, square and with no hairline; only its bottom corners
are rounded.

- The answer is one row and scrolls sideways. A long label breaks after a space onto at most
  three rows. A message from a key typed in the prompt shows at the end of its row, in
  brackets, until the next key.
- A short frame gives up the padding below, then the row between the label and the answer,
  then the label's last rows: it keeps its first, the last of them cut with `…`. With one row
  of text, the label and the answer share it.
- The window beneath keeps its text and modeline, and its cursor is drawn **hollow**. The
  float may cover it, and the view does not move. The wheel scrolls it, and its cursors stay.
- Keys: `RET` gives the answer, `C-g` closes the prompt, `M-p` and `M-n` walk its history,
  and editing keys work as in any buffer. A history keeps 100 answers in memory, and an
  answer given again moves to the front. A prompt opened while a buffer that may hold secrets
  is shown keeps none.
- A prompt can open from a prompt, 8 deep. Only the innermost shows, its depth first: `[2]`.
- **Questions** show in the same float, with their keys as chips on the last row, each with
  what it does: `[y] exit  [n] stay`. Only `y` and `n` answer; `SPC`,
  `DEL` and `RET` do not, unlike in Emacs, and any other key says `Answer y or n`. A held
  key answers once. `C-x C-c` and the close button ask before exiting with unsaved work, and
  `C-x C-s` asks before saving over a file that changed on disk.
- **A passphrase** shows one `*` per character, keeps no undo and enters no history.
- `C-g` does one thing at a time: a pending prefix, then the region, then the innermost
  prompt.
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

## Mark and region

As Emacs with `transient-mark-mode` and `shift-select-mode`
([ADR 0071](adr/0071-mark-and-region.md)):

- The region is shown only while it is active: from `C-SPC` (or `C-@`), `C-x C-x` or `C-x h`, or a
  motion with Shift, until an edit, `C-g`, `C-SPC C-SPC` or another buffer in the window. A region
  from a motion with Shift ends with the next motion without it.
- It is a background colour only, a muted blue-grey until the theme: text keeps its own colour,
  and boxes, the cursor and a preedit's underline are drawn over it. A row whose line break is in
  the region is filled to the window's right edge.
- `C-u C-SPC` and `C-,` go back through the mark ring, 16 marks. `M-<` and `M->` set the mark where
  the cursor was, and the echo area says "Mark set".
- With several cursors, each has its own mark, and all regions show or none. Regions that come to
  overlap merge into one.
- During a composition the region stays, and the preedit pushes it as it pushes the text.

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
- Option keys stay Super with a Japanese input source. Pressed during a composition, an
  Option key ends it and what was being composed is lost.

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
