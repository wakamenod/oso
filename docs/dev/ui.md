# User interface (M2)

This document describes the screen layout and visual behavior that M2
builds. The decision record is [ADR 0026](adr/0026-m2-ui-layout.md); the
themes are in [ADR 0027](adr/0027-theme.md) and
[ADR 0089](adr/0089-built-in-themes-switched-at-run-time.md). UI for later features is designed
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
- **Completion** ([ADR 0076](adr/0076-completion-and-m-x.md)) floats at the **center** of the
  frame, unlike the other prompts: 62% of its width, and always as tall as **18 candidate rows**
  and the answer's row (configurable once configuration exists), however many match. The label,
  the answer in its field and a count such as `3/57` share one row. A 5-point border a little
  darker than the fill frames it, with all four corners rounded.

```
│Agenda         ╭────────────────────────────────────────────────────╮        │
│- budget for Q3│                                                    │        │
│- hiring: two e│  M-x ╭ forw█ ───────────────────────────────╮  2/2 │        │
│- office move  │                                                    │        │
│               │   forward-char C-f  Move point N characters (…     │        │
│Decisions      │  ▌forward-word M-f  Move point to the end of t…▐   │        │
│- keep the Frid│                                                    │        │
│               ╰────────────────────────────────────────────────────╯        │
```

- **Matching is orderless's:** words separated by spaces match in any order, each as
  literal text or a regular expression. An affix at a word's start or end picks one style:
  `!` must not match, `,` initials (`,sbk` for `save-buffers-kill-terminal`), `=` literal
  only, `^` at the start, `~` the letters in order, `&` the annotation. Case counts once the
  input has an upper-case letter. What matched is highlighted.
- **The order is vertico's:** the prompt's history first, then shorter, then alphabetical,
  with an exact match of the input on top. Typing never moves a candidate.
- **Each row** has the candidate, its shortest key after it, and a summary at the right end:
  for commands, the docstring's first line (marginalia-style). A narrow row cuts the summary
  first, then drops it, then the key, then cuts the name.
- **Keys:** `C-n`/`C-p` and the arrows move (stopping at the ends), `M-<`/`M->` go to the
  first and last, `C-v`/`M-v` by a page, `TAB` puts the selected candidate in the answer,
  and `RET` gives the selected candidate, or says `No match`.
- **`M-x`** lists every command but those that read their key or belong to a keymap not active
  beneath it, runs the one taken with the prefix argument typed before `M-x` (its label says
  it, `C-u M-x`), and puts the one run last first, so `M-x RET` runs it again.
- A short frame gives up candidate rows first, down to one, then the padding.
- Beside a buffer that may hold secrets, a prompt neither reads nor records history, so its
  order says nothing of earlier answers.
- **Preview inside the popup.** For commands with a preview, such as buffer
  switching, ripgrep search and line search, the popup splits: candidates on
  the left, a preview of the selected candidate on the right. Commands
  without a preview, such as `M-x`, use a single-column popup.
- The preview is read-only, and it only renders already-open buffers or
  files the trust model allows reading.

### Finding files (`C-x C-f`, `C-x C-w`)

[ADR 0087](adr/0087-finding-files-writing-under-a-new-name-and-large-files.md):

- The centred float, `Find file:` (or `Write file:`), starting at the buffer's directory, the home
  directory as `~/`. A worker lists directories; the count shows `…` until it answers.
- Past 5000 names, a worker builds the list and matches each key; the rows shown stay and the
  count shows `…` until it answers, and `RET` or `TAB` meanwhile waits for it. Any other key
  drops that wait, a prefix too, and `C-g` then does nothing more
  ([ADR 0096](adr/0096-completion-on-a-worker-past-5000-candidates.md)).
- Segments before the last are prefixes, and the last one matches as in `M-x`: `~/P/S/o` reaches
  `~/Projects/SideProjects/oso`. `~/` or `/` typed after a `/` starts over, the text before drawn dim.
  Names starting with `.` show only when the last segment starts with `.`.
- A row is the name, `/` after a directory, then its mode, size and age, and `→ target` for a link.
- `TAB` puts the selected path in the answer. `RET` enters a directory and opens a file, or the
  typed path when nothing matches (`(New file)`). `M-RET`, or `C-p` from the first row, takes the text
  as typed.
- A file already open shows its buffer. A file of 2 GiB or more asks
  `big.iso is 3.2 GiB, all held in memory; open it? (y or n)`.
- `C-x C-w` writes the buffer under a new path, asking before it writes over a file, and the buffer
  then visits it.

### Buffers (`C-x b`, `C-x k`, `M-,`, `M-/`, `C-.`)

[ADR 0088](adr/0088-switching-and-killing-buffers.md):

```
┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃ 1/4 Switch to buffer: █                                    ┃
┃▌todo.md  *  2k  Te…▐┃# Todo                                 ┃
┃ notes.md    9k  Te… ┃- budget for Q3                        ┃
┃ *Messages*  1k  Me… ┃▯ hiring: two engineers                ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

- `C-x b` lists buffers, the most recent first and the current one last, so `RET` goes back to the
  previous one. A row is the name, `*` when modified, the size, the mode and the path. A name that
  matches nothing creates a buffer.
- The float keeps its size: the list takes the left 2/5, the preview the rest, showing the selected
  buffer from where the window last was in it. A buffer that may hold secrets is never previewed: the
  preview says `may hold secrets`. A narrow frame drops the preview.
- A window comes back to a buffer where it left it: cursors, marks and scroll.
- Two files with one name show as `notes.md<work>` and `notes.md<home>`.
- `C-x k` reads a buffer, the current first, and asks before killing a modified one. `*Messages*`,
  `*Emergency Copies*` and `*Agent*` are kept. The window then shows the most recent other buffer.
- `M-,` and `M-/` go to the previous and next buffer in the window's order, skipping internal ones.
- `C-.` lists the mark ring, with the buffer around the selected mark in the preview.

## Menus

Hydra/transient-style menus float at the **center** of the frame, as they do
with hydra-posframe today.

## Echo area

- A single line at the bottom of the frame shows messages such as "Saved"
  and errors, including caught command panics, until the next key.
- The minibuffer no longer lives there, so the line is for messages only, and for
  what is being typed: `C-x-` or `C-u 5-`, which comes ahead of any message.
- Every message is also appended to the `*Messages*` buffer, which `C-h e` shows, and until
  `C-x b` leaves. What a command says while a secret buffer is shown stays out of
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
- A copy of a file not open needs `oso PATH` or `C-x C-f`, which starts at the row's path.

## Modeline

- Each window has its own modeline. The active window's modeline is visually
  distinct.
- M2 contents:
  - a warning when the file contains bidirectional control characters
  - a warning when the file changed on disk while the buffer has unsaved
    changes, or when the file is gone from disk
    ([ADR 0083](adr/0083-reloading-files-changed-on-disk.md))
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
- With multiple cursors, every cursor but the primary is drawn as an outline of its box in
  a dimmer colour, the theme's prompt colour, over the glyph as it is; the primary keeps its
  filled box ([ADR 0091](adr/0091-multiple-cursor-keys-and-m-y.md)).
- After a large jump or scroll, the cursor position flashes briefly
  (beacon-style, [ADR 0097](adr/0097-visual-aids.md)).

## Themes

[ADR 0089](adr/0089-built-in-themes-switched-at-run-time.md):

- Three themes: `dark` (near-black, the default), `light`, and `dusk` (dark grey).
- `M-x load-theme` picks one through a completion. `<f5>` (`toggle-theme`) switches between
  `dark` and `light`, a pair that configuration will be able to name (M3).
- The choice is remembered across starts. oso does not follow the OS's dark or light appearance.
- A switch redraws once. In every theme, text that is read keeps 7:1 contrast and secondary
  labels 4.5:1, and a question's label has a colour no buffer text has.

## Mark and region

As Emacs with `transient-mark-mode` and `shift-select-mode`
([ADR 0071](adr/0071-mark-and-region.md)):

- The region is shown only while it is active: from `C-SPC` (or `C-@`), `C-x C-x` or `C-x h`, or a
  motion with Shift, until an edit, `C-g`, `C-SPC C-SPC` or another buffer in the window. A region
  from a motion with Shift ends with the next motion without it.
- It is a background colour only, the theme's `region`: text keeps its own colour,
  and boxes, the cursor and a preedit's underline are drawn over it. A row whose line break is in
  the region is filled to the window's right edge.
- `C-u C-SPC` and `C-,` go back through the mark ring, 16 marks. `M-<` and `M->` set the mark where
  the cursor was, and the echo area says "Mark set".
- With several cursors, each has its own mark, and all regions show or none. Regions that come to
  overlap merge into one.
- During a composition the region stays, and the preedit pushes it as it pushes the text.

## Multiple cursors

As multiple-cursors.el ([ADR 0091](adr/0091-multiple-cursor-keys-and-m-y.md)):

- `C->` and `C-<` add a cursor at the next or previous occurrence of the region, or of the
  word at point, which the first key selects; a string from a word matches whole words only,
  and case always matters. The new cursor's region covers the match, and it becomes the
  primary. There is no wrap: past the end the echo area says "No more matches".
- `C-M->` and `C-M-<` skip: they move the last (first) cursor on to the next occurrence. `M--`
  before `C->` or `C-<` removes the cursor at that end.
- `C-c C-<` puts a cursor on every occurrence in the buffer, and `C-x SPC` one on each line from
  the mark's to the cursor's, at the cursor's column.
- A window has at most 1000 cursors. A key that would pass it adds none, and `C-c C-<` says how
  many occurrences there are.
- A far occurrence is found on a worker thread, and the echo area says `Searching…` meanwhile.
- `C-g` first hides the regions, then leaves the primary cursor alone.

## `M-y`

As consult-yank-pop ([ADR 0091](adr/0091-multiple-cursor-keys-and-m-y.md)):

- `M-y` reads the clipboard as `C-y` does, then lists the kill ring in the centred float,
  the newest first, each text once. A row is the first 1 KiB of a kill, a line break drawn
  as `⤶` in the annotation's face, and says how many lines and pieces it has. The selected kill's lines show beside
  the list.
- A kill from a buffer that may hold secrets is listed with only its line count, no text and
  no preview, and anything typed hides it. It is never compared with another kill.
- `RET` yanks the kill picked as `C-y` would, in place of what a yank just inserted, and
  puts it first in the ring.
- In a prompt and in isearch, `M-y` right after a yank puts the next older kill in its place,
  passing secret ones.

## isearch

- `C-s` and `C-r` search for the literal string as it is typed, with smart case: case matters
  once the string has an uppercase letter, and `M-c` turns that around for the search
  ([ADR 0082](adr/0082-isearch.md)).
- All matches in view are highlighted in a muted amber, and the current match in a strong one.
- The search string is shown in the echo area, with the count anzu-style: `I-search: foo [3/17]`,
  `[?/…]` while a worker counts, and `[searching…]` while it looks for a far match.
- `C-s`/`C-r` go on to the next match, and round the end after `Failing I-search`. `DEL` goes back
  a step, `C-w` adds the word after the match, `C-y` the newest kill, `M-p`/`M-n` go through the
  search history, and `RET` ends the search there and pushes where it began onto the mark ring.
- `C-g` first takes away a failing part, then goes back where the search began. Any other key
  ends the search and runs.
- Only the primary cursor searches; the others stay. `C->` and `C-<` end the search with the
  match as the region and add a cursor at the next or previous match, and `C-c C-<` puts one on
  every match, with the search's string and case
  ([ADR 0091](adr/0091-multiple-cursor-keys-and-m-y.md)).

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

On by default, each toggled by an `M-x` command for every buffer, which a prefix argument
turns on (positive) or off (zero or negative) as Emacs's minor modes do, and from the
toggle menu once it exists ([ADR 0097](adr/0097-visual-aids.md)):

- **Whitespace** (`global-whitespace-mode`):
  - a full-width space shows as `□`, a tab as a dim `»` in its first cell, and the other
    non-ASCII spaces (such as U+00A0) as a dim `·`;
  - trailing spaces and tabs get a fill, except on a line with a cursor;
  - half-width spaces are not marked.
- **Current line** (`global-hl-line-mode`): a band across the text area on the primary
  cursor's row, behind the region and isearch's matches.
- **Cursor flash** (`beacon-mode`): after the cursor moves 5 or more rows, a command
  scrolls the window as far (`C-v`, `M-v`, `C-l`, `M-<`, `M->`), or another buffer or window
  is shown, a band from the cursor to the window's edge fades out over about 0.4 s. The
  scrolling that follows the cursor and the wheel do not flash. The next key ends it.

Off by default: **line numbers** (`global-display-line-numbers-mode`). They are absolute, the
current one brighter, in a gutter as wide as the line count's digits plus one, at least
3 digits. Floats keep their place when it appears.

None of these is remembered across starts until configuration exists (M3).

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
