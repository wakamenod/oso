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

## Windows

- A frame splits into windows stacked (`C-x 2`) or side by side (`C-x 3`), and the new window
  below or at the right is selected; `C-x 0` deletes the selected one, `C-x 1` the others, and `C-x o` selects the next. `C-x +` balances them, and
  `C-x ^`, `C-x }` and `C-x {` resize ([ADR 0112](adr/0112-window-splits.md)).
- Each window has its text area, its gutter and its modeline. Side by side windows are parted
  by a column with a thin line in its middle.
- A window that is not selected shows its cursors hollow, and no region, current-line band,
  isearch matches or flash. Its modeline is in the inactive face. Beneath a prompt, the window
  it came from keeps its region and band, its cursor hollow and its modeline inactive.
- The minibuffer and completion float over the whole tiled area, whatever the layout.
- The wheel scrolls the window under the pointer without selecting it, its gutter and
  modeline included. Over a divider, the echo area or the margins it scrolls the selected
  window, and over a float the window under it.

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
  The wheel scrolls another window too, whose cursors come into its view as without a
  prompt.
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
- The preview is read-only, and it only renders already-open buffers or,
  for a project search, files read from disk that may not hold secrets
  ([ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md)).

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

### A project's files (`C-x p f`)

[ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md):

```
┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃ …/… Find file in oso: rs█                                  ┃
┃▌a.rs                                                      ▐┃
┃ build.rs                                                   ┃
┃ src/lib.rs                                                 ┃
┃ src/main.rs                                                ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

- `project-find-file` opens the centred float, `Find file in oso:`, over the current buffer's
  project, its files as paths from the root. Outside a project the buffer's directory stands in,
  named after its last component; a buffer with no file, the home directory and `/` say
  `Not in a project`. From inside a prompt it says `Not in the minibuffer`.
- `C-u C-x p f` asks `Find file in directory:` first, with `C-x C-f`'s completion from the
  buffer's directory, and lists that directory as a project, named after its last component.
  It lists only directories and links, as Emacs's `read-directory-name` does: `RET` on a row
  goes into it, and on a link that leads elsewhere says `Not a directory: <path>`. What is typed
  is given once the listings that decide it have come, `RET` and `M-RET` waiting until then:
  `Not a directory`, `No such directory` or `Cannot read directory` when they say so. The home
  directory, one that holds it, `/` and a path with `..` in it are refused.
- The float opens empty and the list grows while the project is walked, every 50 ms. The count
  shows `…/…` until the walk ends, and `+` after it when it stopped at a limit: 100,000 files,
  32 MiB of paths and names, 1,000,000 entries read, or 16 MiB of ignore files.
- Rows come in the order of `M-x`'s: the files opened from this project's listing first, then the
  shorter, then by name. Each batch is matched against what is typed as it comes. The row
  selected stays selected as rows come, so `RET` during the walk takes the row shown. Past 5000
  rows a worker adds each batch, and `RET` waits only when it also matches a newer input.
- The files are those `rg --files --hidden` lists: hidden files too, less what `.rgignore`,
  `.ignore`, `.gitignore`, the root's `.git/info/exclude` and the global excludes leave out, read
  only inside the project. Nothing inside `.git`, `.jj`, `.hg` or a directory holding one, which
  is another project, is listed. Links are never followed: a link to a file under the root is
  listed, and so is one that leaves it.
- `RET` visits the file as `C-x C-f` does, so a file that may hold secrets opens as such a buffer.
  There is no preview. `C-g` stops the walk.
- The history is the project's own, in memory, and a listing beside a buffer that may hold
  secrets keeps none.

### Searching a project (`M-s r`, `C-x p g`)

[ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md):

```
┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃ …/… Search oso: fn !test█                                  ┃
┃ src/main.rs:1: fn main()│use std::fmt;                       ┃
┃▌src/lib.rs:3: fn run() ▐│                                    ┃
┃ .env:2      may hold se…│pub ▯n run() {                      ┃
┃ src/lib.rs:40:     xxxx…│    println!("fn");                 ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

- `project-search` opens the centred float, `Search oso:`, over the same project as `C-x p f`,
  with the same outside-a-project rule, and `C-u` asks `Search in directory:` first.
- What is typed is what to look for. It is split at spaces into parts, and a line matches when
  every part does, in any order. A part matches as typed and, if it holds a character an
  expression reads, as that expression too, so `fn\s+main` and `^use` work. `=` before a part
  keeps it as typed, and `!` before one leaves out the lines holding it. No other affix of `M-x`'s
  means anything here, and an affix is read only at a part's start, so `x =` and `done!` are
  text. Case is ignored unless an upper-case letter is typed.
- A search starts once a part that is not excluded has two characters, and each key starts a new
  one; the one before stops at its next file or block. A key that only narrows a search that
  ended without a limit looks again through the lines it found, without reading the files again.
- Rows come while the search runs, a file's together and in line order, at most every 30 ms, and
  never move once shown. Each is `path:line: text`, the path and line in the annotation's face and
  the matches in the match face. The text is at most 512 bytes of the line around its first
  match, `…` where the line goes on, with control and invisible characters boxed. The count shows
  `…/…` while the search runs and `+` once it stopped at a limit: 10,000 rows, or the walk's.
- It searches the files `C-x p f` lists, as they are on disk, and never a link. A line longer
  than 16 MiB skips the rest of its file, and a NUL byte stops a file, as `rg` stops at a binary
  one. A file whose name may hold secrets is searched, but its rows say `path:line` and
  `may hold secrets`, with no text.
- `RET` visits the file as `C-x C-f` does and goes to the line and the match's column once it
  shows, with the place before on the mark ring. `TAB` does nothing, as a row's text would
  replace what is looked for. `C-g` stops the search.
- The float splits as `C-x b`'s does, and the preview shows the selected row's file around its
  line, that line about halfway down, with a hollow cursor at the match, the match in isearch's
  face and the other matches in view in its other face. An open buffer of the file is shown from
  memory, unsaved edits and all; any other file is read from where the search found the line, at
  once, and a row selected before the read comes stops it. The preview never wraps here: it
  scrolls left as far as the match needs to show. A row of a file that may hold secrets says
  `may hold secrets` in its place and nothing of the file is read; a file gone, swapped for a link
  or no longer a regular file shows nothing. The preview opens no buffer and takes no key.
- `C-u`'s `Search in directory:` and `Find file in directory:` list only directories and links,
  as Emacs's `read-directory-name` does. `RET` on a row goes into it. What is typed is given only
  once a listing says it is a directory, or a look says a link leads to one: otherwise the prompt
  stays open and says `Not a directory`, `No such directory` or `Cannot read directory`. `RET` and
  `M-RET` typed before that listing or look has come wait for it.
- The history, `project-search`, keeps what was typed, not the row, and a search beside a buffer
  that may hold secrets keeps none.

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

Hydra/transient-style menus float at the **center** of the tiled area, as they do
with hydra-posframe today ([ADR 0117](adr/0117-menus.md)). Steps 1 to 3 are built: menus
from `config.toml`, the open menu with its float, and the built-in menus. The key list is
planned.

- A head closes the menu unless it says `stay`; switches (`[x]`/`[ ]`) stay and are read by the
  head that runs. A key the menu does not bind is refused with `x is not in this menu: q to quit`
  and drops the prefix argument. `C-g` first ends a half-typed head or the prefix argument, then
  closes one level of submenus; `q` closes one level too unless a head binds it.
- The float is completion's panel at the center of the tiled area. Each key is a chip: the usual
  colour for a head that closes the menu, a red one for a head that stays, and a third for a
  switch, whose `[x]` sits on a chip of its colour while it is on. Titles are in their own colour,
  notes dim and aligned in a column, and a head that cannot run now is dim with no chip. The head
  `RET` runs is marked with a box. `C-u 5-` shows at the end of the title's row while it is typed.
- `C-u`, `M-<digit>` and `M--` build a prefix argument for the next head; plain digits are heads.
- A head that opens a prompt, a search or a replay closes the menu first; no menu opens in a
  prompt (`Not in the minibuffer`). A menu's key ends isearch, query-replace or the layer of
  `C-x TAB` and opens the menu; during a jump it ends the jump and does not run, as any key
  the jump does not bind. The input method is off while a menu is open, and the agent
  channel waits (`the person is in a menu`). `M-x open-menu` opens any menu by name.
- Built-in menus, each with heads only for commands oso has:
  - `toggles` on `C-c t`: whitespace, whitespace cleanup, the cursor flash, truncation, line
    numbers, the current line's band, highlighting, and the theme pair, whose note names the
    theme drawn now. Each toggle shows `[x]` or `[ ]`, and every head closes the menu.
  - `windows`: the arrows resize the window by 3 and stay, `=` balances and stays, `-` and `/`
    split, `d` kills a buffer, `0` deletes the window and `O` the others, `b` switches buffer,
    `,` and `.` go to the previous and next buffer and stay, `f` finds a project's file and `F`
    a file.
  - `jumps`: `t` the timed character jump, `l` and `h` a line, `e` a line's end, `n` a line by
    number.
  - `search`: `f` a project's file, `r` the project's lines.
  The review menu on `C-x v r` comes with ADR 0122. `windows`, `jumps` and `search` have no
  key: `M-x open-menu` opens them, or a key in `config.toml`.
- `[menus.<name>]` in `config.toml` adds a menu, replaces a built-in one of the same name whole,
  or removes it with `false`. A removed menu's default key is then undefined. There are at most
  256 menus, the four built-in ones among them even when removed. For example
  (`config/menus_tests.rs` loads this sample, and changes with it):

  ```toml
  [menus]
  search = false                    # removes the built-in search menu

  [menus.toggles]                   # replaces the built-in toggles; C-c t opens this one
  title = "Toggles"
  [[menus.toggles.groups]]
  title = "Aids"
  heads = [
    ["w", "global-whitespace-mode", "whitespace"],
    ["n", "global-display-line-numbers-mode", "line numbers", { stay = true }],
  ]

  [menus.buffers]                   # adds a menu
  title = "Buffers"
  [[menus.buffers.groups]]
  heads = [
    ["b", "switch-to-buffer", "switch"],
    [",", "previous-buffer", "previous", { stay = true }],
    [".", "next-buffer", "next", { stay = true }],
    ["w", { menu = "windows" }, "windows…"],
  ]

  [keys.global]
  "C-c w" = { menu = "windows" }
  "C-c b" = { menu = "buffers" }
  ```
- Planned (step 6): after a prefix key waits 1.0 s, a panel along the bottom of the frame lists
  the keys that may follow, as which-key does; `C-h` shows it at once and pages it.

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
  distinct, and segments about the frame, such as `Agent`, show only there
  ([ADR 0112](adr/0112-window-splits.md)).
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
- The project, after the name: its root's last component, cut to 24 cells, then
  `restricted` in the dim face, as `Agent` is, while the record of trusted projects
  does not trust it, also while that record is unread or refused. A buffer in no project
  shows neither ([ADR 0114](adr/0114-projects-trust-finding-files-and-searching-a-project.md)).
  `M-x trust-project` asks "Trust ~/src/oso? Its code may run through language servers,
  formatters and git hooks (y or n)", with the verbs `trust` and `keep restricted`. `y` records
  the root in `trusted-projects`, beside `config.toml`, says `Trusted ~/src/oso` and drops
  `restricted`. `M-x untrust-project` takes the line out without asking and says
  `~/src/oso is restricted again`.

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
  `dark` and `light`, or the pair `[theme] pair` names in `config.toml`.
- The choice is remembered across starts, and wins over `[theme] default` until that name
  changes ([ADR 0108](adr/0108-the-user-configuration-file.md)). oso does not follow the OS's
  dark or light appearance.
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

- `C-s` and `C-r` search for the literal string as it is typed, or migemo's pattern for romaji
  (below), with smart case: case matters once the string has an uppercase letter, and `M-c`
  turns that around for the search ([ADR 0082](adr/0082-isearch.md)).
- All matches in view are highlighted in a muted amber, and the current match in a strong one.
- The search string is shown in the echo area, with the count anzu-style: `I-search: foo [3/17]`,
  `[?/…]` while a worker counts, and `[searching…]` while it looks for a far match.
- `C-s`/`C-r` go on to the next match, and round the end after `Failing I-search`. `DEL` goes back
  a step, `C-w` adds the word after the match, `C-y` the newest kill, `M-p`/`M-n` go through the
  search history, and `RET` ends the search there and pushes where it began onto the mark ring.
- `C-g` first takes away a failing part, then goes back where the search began. Any other key
  ends the search and runs.
- `C-M-s` and `C-M-r` search for a regexp in the `regex` crate's syntax, one line at a time, and
  the line says `Regexp I-search:`; in any search they repeat it, as `C-s` and `C-r` do. `M-r`
  turns a search from literal to a regexp or back. A regexp still being typed says
  `[incomplete input]` and a wrong one `[Invalid regexp: …]`, and the cursor stays at the last
  match. Regexps have their own history ([ADR 0116](adr/0116-migemo-and-jumps.md)).
- A string of lowercase letters, `'` and `-` is romaji, and migemo searches for it with its kana
  too: `ka` also finds `か`, `カ`, `ｶ` and `ｋａ`, and an unfinished end every kana it can start,
  so `kan` finds `かな` and `かん`. The line then starts `[migemo]`. Any other character, an
  uppercase letter among them, searches literally, and so does a string with a letter that starts
  no spelling (`struct`). A regexp's search never uses it. `M-m` turns it off or on for the rest
  of a search, and `[search] migemo = false` starts every search with it off
  ([ADR 0116](adr/0116-migemo-and-jumps.md)). With a dictionary the person names in
  `[search] migemo-dictionary` (C/Migemo's `migemo-dict` or an SKK file, in UTF-8), it also
  finds the words whose reading starts with the kana: `kanji` finds `漢字`. oso ships none. The
  file is read at the first search that would use it, and until it is in, searches find kana;
  `*Messages*` then says `Migemo dictionary: 137,025 readings`, or why it was not read.
- Only the primary cursor searches; the others stay. `C->` and `C-<` end the search with the
  match as the region and add a cursor at the next or previous match, and `C-c C-<` puts one on
  every match, with the search's string and case
  ([ADR 0091](adr/0091-multiple-cursor-keys-and-m-y.md)).

## Jumps

- `M-j` (`avy-goto-char-timer`) reads a string on the echo row, `Jump: ka`, and highlights its
  matches in view as isearch's other matches. A romaji string matches by migemo, as isearch's
  does, unless `[search] migemo = false`. `DEL` takes a character away. Half a second after
  the last key, or at `RET`, each match shows a label; with no string yet, it waits
  ([ADR 0116](adr/0116-migemo-and-jumps.md)).
- `M-g f` (`avy-goto-line`) labels each line's start in view at once, and a digit that no label
  takes, typed before any of a label, opens `goto-line` with it. `avy-goto-end-of-line` labels each line's end and has no key. `M-j` in
  isearch (`avy-isearch`) labels the search's matches in the selected window, and ends the search
  at the one chosen, as `RET` does.
- Labels show in every tiled window, nearest the window's cursor first, over the first cells of
  each target in two faces of their own: the key to type next, and the keys after it. A label is
  cut at the row's end and at the next label. Typing a label's keys jumps; its first key keeps the
  labels that start with it. At most 4096 are labelled, and the echo row then says how many
  there were.
- The primary cursor jumps and the others stay. A jump into another window selects it. With a
  region shown, the jump extends it; otherwise the place left is pushed onto the mark ring.
  `C-u` keeps to the selected window.
- At the labels, an action's key acts on the label typed next instead of going there, as avy's
  dispatch keys: `SPC` sets the region from the cursor to it; `W` copies its whole line and `K`
  kills it, and the cursor stays; `Y` copies it and `T` kills it, and each yanks it at every
  cursor as `C-u C-y` does; `c` adds a cursor there, and the primary stays. What is typed of a
  label stays, another action's key takes the place of the first, and the echo row shows the one
  armed: `Jump to a line  [kill line]`. While a string is read, these keys are characters of it.
  An action is one undo step in each buffer it changes; in a read-only buffer it says so and does
  nothing (ADR 0116 step 6).
- `C-g` ends a jump with nothing moved, before anything else it would do. A key that is not a
  label, `DEL` and `RET` among them, says `No such candidate` and does not run; while the string
  is read, a key the jump does not bind ends it silently. A change to the text or a scroll under
  the labels ends the jump, saying which, and the next key types nothing; while the string is
  read, its matches are found again.
- Labels are drawn over everything in their cells, the text of the rows beside them too, and
  under floats. A keyboard macro replays a jump as it was recorded, labels and all.
- `[jump] keys` sets the label keys, `asdfghjl` by default: 2 to 32 distinct printable ASCII
  characters, none of `W K Y T c`. `[jump] timeout` sets the wait in seconds, 0.1 to 5, 0.5 by
  default.

## query-replace

- `M-%` reads the string and its replacement in two prompts, which take Japanese as the prompt
  for a file's name does; an empty string takes the last pair, which the first prompt offers
  ([ADR 0113](adr/0113-query-replace-keyboard-macros-and-whitespace-cleanup.md)).
- It runs from the cursor to the end of the buffer, or within the regions shown, with smart
  case, and the replacement takes the match's case while case folds.
- The echo area says `Query replacing foo with bar: [3/17] (? for help)`, and the matches in
  the scope are highlighted as isearch's are.
- At each match: `y`/`SPC` replace, `n`/`DEL` skip, `!` the rest, `.` this one and end, `,` this
  one and stay, `^` back, `u` undo the last replacement, `q`/`RET` end, `?` the keys. `C-g` ends
  it keeping what it replaced, and any other key ends it and runs.
- The whole session undoes as one step.
- `C-M-%` replaces a regexp's matches, in the `regex` crate's syntax, one line at a time, with
  `$1`, `${name}` and `$0` in the replacement. A regexp that does not compile keeps the prompt
  open, saying why.

## Keyboard macros

- `C-x (` or `<f3>` starts recording, with `Def` on the modeline; `C-x )` ends it; `C-x e` or `<f4>`
  ends a recording and runs the last macro, and `e` right after runs it again. `C-u N` runs it
  N times, and `C-u 0` until something stops it
  ([ADR 0113](adr/0113-query-replace-keyboard-macros-and-whitespace-cleanup.md)).
- A macro holds the keys typed and the text an input method commits. A passphrase prompt
  drops the recording, as does typing in a buffer that may hold secrets, and `C-g` with
  nothing else to do drops it too.
- A replay never answers a question: it does not start while one is asked, and it stops
  before typing into one.
- A replay runs a few milliseconds at each turn, so frames keep coming. A command's error, an
  undefined key, a failing search, a question or a passphrase prompt stops it, and `C-g`
  does. What it changed in a buffer undoes as one step.

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
  isearch, a menu or the list of emergency copies, since those use single keys. In the
  prompt for a file's name (`C-x C-f`, `C-x C-w`), the input source switches to ASCII when it
  opens and back when it closes, and choosing a Japanese input mode there, by `かな` or the
  menu bar, turns the IME on for that prompt; `英数` turns it off again
  ([ADR 0106](adr/0106-the-input-source-turns-japanese-on-in-a-file-name-prompt.md)). It is
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
toggles menu on `C-c t` ([ADR 0097](adr/0097-visual-aids.md), [ADR 0117](adr/0117-menus.md)):

- **Whitespace** (`global-whitespace-mode`):
  - a full-width space shows as `□`, a tab as a dim `»` in its first cell, and the other
    non-ASCII spaces (such as U+00A0) as a dim `·`;
  - trailing spaces and tabs get a fill, except on a line with a cursor;
  - half-width spaces are not marked.
- **Current line** (`global-hl-line-mode`): a band across the text area on the primary
  cursor's row, behind the region and isearch's matches.
- **Cursor flash** (`beacon-mode`): after the cursor moves 5 or more rows, a command
  scrolls the window as far (`C-v`, `M-v`, `C-l`, `M-<`, `M->`), or another buffer or window
  is shown, a band from the cursor to the window's edge holds for 150 ms, then fades out in
  two steps by 0.4 s. The scrolling that follows the cursor and the wheel do not flash. The
  next key ends it.

**Highlighting** (`global-font-lock-mode`, the same argument) is on by default
([ADR 0111](adr/0111-tree-sitter-highlighting.md)). It colours comments, strings, keywords,
types, functions, constants, macros, and Markdown's headings, links and markup, in each
theme's syntax colours, at 7:1 on the background, the current line's band and a float. Under
the region, isearch's matches, trailing whitespace's fill and the cursor flash, text is drawn
in the text's colour. The colours are provisional, until the author's own syntax theme.
Dusk's are pale tints or the text's own, so it looks nearly plain. Off, it drops every buffer's
highlights. A buffer's language is its major mode, from the file's name
on a visit and on `C-x C-w`, and set by hand with `rust-mode`, `markdown-mode`, `toml-mode` and
`fundamental-mode` (`M-x`, no keys), which refuse an internal buffer. Setting a mode again,
the same one too, gives a buffer whose highlighting stopped another try. The modeline names
the mode.

Off by default: **line numbers** (`global-display-line-numbers-mode`). They are absolute, the
current one brighter, in a gutter as wide as the line count's digits plus one, at least
3 digits. Floats keep their place when it appears.

None of these is remembered across starts. Each starts as `[aids]` in `config.toml` says, and
a switch made with its command stands until that setting changes in the file
([ADR 0108](adr/0108-the-user-configuration-file.md)).

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
