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
  what it does: `[y] exit  [n] stay`. Only `y` and `n` answer, and `w` where a question has a
  third answer (below); `SPC`, `DEL` and `RET` do not, unlike in Emacs, and any other key says
  `Answer y or n`. A held
  key answers once. `C-x C-c` and the close button ask before exiting with unsaved work, and
  `C-x C-s` asks before saving over a file that changed on disk.
- **A third answer**, `w` (`minibuffer::Third`, whose key is always `w`), comes after `n` on
  the few questions that have one: the label ends `(y, n or w)`, and another key says
  `Answer y, n or w`. `C-x C-c` with a session's turn under way or a job running in a shell
  names them, up to three each, and offers `[y] exit  [n] stay  [w] wait`. `w` exits once
  every terminal has been idle for 2 seconds; the echo area says what it waits for, and that
  `C-x C-c n` cancels, which works from a terminal too. `C-g` cancels where oso has the keys;
  a terminal sends it to its program (ADR 0120). If more is at risk once idle than the
  question named, or a review comment is not sent, nothing is asked: the wait stops and the echo area says why
  ([ADR 0133](adr/0133-keeping-terminals-over-an-update.md), step 5).
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
  only, `^` at the start, `~` the letters in order, `&` the annotation, `%` migemo: romaji
  after it also matches its kana, and the words of a migemo dictionary, so `%kan` finds
  `かんじ.md`, and `漢字.md` with a dictionary. Kana a file name holds decomposed, as macOS
  writes them, match too ([ADR 0116](adr/0116-migemo-and-jumps.md)). The words of one
  pattern are shared by every `%` word of an input, in order, so a third or later word whose
  readings give many may match its kana alone. Case counts once the input has an upper-case
  letter. What matched is highlighted.
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

- The centred float, `Find file:` (or `Write file:`), starting at the directory the buffer stands
  in, the home directory as `~/` ([ADR 0141](adr/0141-the-directory-a-buffer-stands-in.md)): its
  file's, a listing's, a git status's or review's project root, a session's, a terminal's (where
  it started, then what its program last said through OSC 7), the side pane's row at point, else
  the one it was made in, else where oso started. A worker lists directories; the count shows `…`
  until it answers.
- Past 5000 names, a worker builds the list and matches each key; the rows shown stay and the
  count shows `…` until it answers, and `RET` or `TAB` meanwhile waits for it. Any other key
  drops that wait, a prefix too, and `C-g` then does nothing more
  ([ADR 0096](adr/0096-completion-on-a-worker-past-5000-candidates.md)).
- Segments before the last are prefixes, and the last one matches as in `M-x`: `~/P/S/o` reaches
  `~/Projects/SideProjects/oso`. `~/` or `/` typed after a `/` starts over, the text before drawn dim.
  Names starting with `.` show only when the last segment starts with `.`.
- A row is the name, `/` after a directory, then its mode, size and age, and `→ target` for a link.
- While what is typed names a directory (nothing after its last `/`), it is selected, as vertico's
  default does: `RET` lists that directory, as dired does, so `C-x C-f RET` lists the buffer's.
  `C-n` goes to the first name, `TAB` puts it in, and typing a name selects the first match.
- `TAB` puts the selected path in the answer. `RET` on a row enters a directory and opens a file,
  or the typed path when nothing matches (`(New file)`). `M-RET`, or `C-p` from the first row, takes
  the text as typed.
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
  named after its last component, the directory it stands in for a buffer with no file; the home
  directory and `/` say `Not in a project`. From inside a prompt it says `Not in the minibuffer`.
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
  expression reads, as that expression too, so `fn\s+main` and `^use` work. A part that is
  romaji, as isearch reads it, also matches its kana and a migemo dictionary's words, unless
  `[search] migemo = false` ([ADR 0116](adr/0116-migemo-and-jumps.md)). The words of one
  pattern are shared by every such part, in order, so a third or later part whose readings
  give many may match its kana alone. `=` before a part
  keeps it as typed, and `!` before one leaves out the lines holding it, its kana too. No other affix of `M-x`'s
  means anything here, and an affix is read only at a part's start, so `x =` and `done!` are
  text. Case is ignored unless an upper-case letter is typed.
- A search starts once a part that is not excluded has two characters, and each key starts a new
  one; the one before stops at its next file or block. A key that only narrows a search that
  ended without a limit looks again through the lines it found, without reading the files again:
  a part made longer at its end, a romaji one too while every kana form and word of the longer
  part holds one of the shorter's, as when typing `n`, `na`, `nam`, `name`.
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

### Searching the buffer's lines (`M-s l`)

[ADR 0154](adr/0154-consult-line.md), as consult's `consult-line`:

```
┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃ 3/3 Go to line: need█                                      ┃
┃▌ 40: let needle = 1; ▐│  fn main() {                        ┃
┃  97: needle()         │      let ▯eedle = 1;                 ┃
┃   2: // needle        │      println!("{}", needle);        ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

- `consult-line`, on `M-s l` and the `search` menu's `l`, opens the centred float, `Go to line:`,
  over the buffer the window shows. In a terminal it says to search in copy mode, `C-c C-j`.
- What is typed is read as `project-search` reads it: parts in any order, `=` and `!`, an
  expression where one reads, smart case, and romaji also matching its kana. Every key searches,
  one character too; with nothing typed, or only `!` parts, every line that is not empty is
  listed.
- The rows start at the cursor's line and go to the end, then round from the first line to the
  one before the cursor's, so the first row, which is selected, is the nearest match at or after
  the cursor. Each is the line's number, padded to the widest, in the annotation's face, then the
  line around its first match, as `M-s r` shows one. They come while the search runs and never
  move; past 10,000 the count says `+`.
- The preview shows the buffer around the selected row's match, from memory, as `C-.` does, with
  a hollow cursor at the match and the other matches in view highlighted.
- `RET` leaves one cursor at the match, with the place before as the mark and the old mark on the
  ring. `C-g` closes the float; the cursor never moved.
- A buffer that may hold secrets lists its lines, but its preview says `may hold secrets`, and
  the history, `consult-line`, keeps nothing of what was typed.

### Buffers (`C-x b`, `C-x k`, `M-,`, `M-/`, `C-.`)

[ADR 0088](adr/0088-switching-and-killing-buffers.md), [ADR 0148](adr/0148-recent-files.md):

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
- After the buffers, `C-x b` lists the recent files no buffer visits, each its path from `~/`
  marked `file`; their preview says `not open`, and `RET` opens one
  ([ADR 0148](adr/0148-recent-files.md)).

### Recent files (`C-x C-r`)

[ADR 0148](adr/0148-recent-files.md): `Open recent file:` in the centred float, with no preview.
A row is the file's path from `~/`, marked `open` when a buffer visits it, the most recent first
and the current buffer's file last, so `RET` opens the one before. Only a listed file is taken. A
file that is gone says `~/notes/a.md no longer exists` and leaves the list.

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
  - `review`, which `review-layout` on `C-x v r` opens with no review open or with `C-u`
    (ADR 0122, ADR 0160): a review of each scope and the session's baseline, with each scope's
    count of files; see [Reviews](#reviews).
  - `space` on `S` in the Agents menu (ADR 0136): the space in front's repositories,
    instructions, directory and name, and closing and reopening.
  - `commit` on `c` in a git status (ADR 0127): its switches and kinds; see
    [Committing](#committing).
  - `push` on `P`, `pull` on `F` (with `- r` and `- m` from step 3a) and `fetch` on `f` in a
    git status (ADR 0127, step 2); see [Remotes](#remotes).
  - `stash` on `z` and `branch` on `b` in a git status (ADR 0127, step 3b); see
    [Stashes and branches](#stashes-and-branches).
  - `log` on `l` in a git status and in a graph (ADR 0127, step 4); see [Graph](#graph).
  - `agents` on `C-c a` ([ADR 0143](adr/0143-agent-keys-on-c-c-a-and-an-agents-menu.md)), in
    every buffer and every terminal: the agent session commands, in groups Start (`c w r a`),
    Go to (`s b n d f`), Talk (`m @ e`), Space (`j z O`, `S` the space menu) and Session
    (`K P A v`, `V` the review menu). Outside a session's buffer, `P` and `A` are dim.
  `windows`, `jumps` and `search` have no key: `M-x open-menu` opens them, or a key in
  `config.toml`.
- `[menus.<name>]` in `config.toml` adds a menu, replaces a built-in one of the same name whole,
  or removes it with `false`. A removed menu's default key is then undefined. There are at most
  256 menus, the seven built-in ones among them even when removed. For example
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

## Reviews

An agent's changes are reviewed in a review buffer ([ADR 0122](adr/0122-agent-review.md)),
`*review: <scope> <project>*`, opened from the `review` menu (`C-x v r` with no review open, or
`C-u C-x v r`) in the review layout (below): `D` since the session's
baseline, `w` uncommitted, `u` unstaged, `s` staged, `b` the branch against its base (`C-u b`
names it), `c` a commit, `r` a range `FROM..TO`, `p` a pull request by its number (step 12b),
and `B` takes a baseline, keeping the menu open.
Each head's note counts the files it would show, as they come; the line under the title names the
project and how long ago the baseline was taken. Only a trusted project runs git.

- The buffer is one stream: a title, then each file's header (status, path, `+n −m`), its notes
  and its hunks, `-` and `+` lines on bands of their own. Files read later show `reading…`, and
  the modeline says `reading n/m` until every file is read.
- `n`/`p` go to the next or previous hunk, `N`/`P` to the next or previous file, each with a
  count; `g` reads the files again, those in view first; `q` goes back; `?` lists the keys.
- Comments show as cards under their line, a bar in the author's face, `You  #7` or
  `Agent  #12 · new` while unread, with replies further in. `c` comments on a line or a hunk's
  header, or replies on a card; `e` edits your comment; `k` removes your comment with its thread,
  which `C-y` yanks back as text, each comment after a line `You  #7` or `Agent  #12` (a thread
  the agent wrote in stays in oso's kill ring, off the clipboard); `r` resolves a thread or opens
  it again; `a` folds agent cards to their first line; `{`/`}` go to the previous or next comment.
  A file's header counts its comments, the agent's and the unread. Outdated comments show above
  the file's first hunk, and a file no longer changed keeps its comments at the end, under
  `no longer changed  <path>`.
- A comment is typed in a draft in the top float, which grows with its text to 10 rows and
  wraps: `RET` breaks a line, `C-c C-c` keeps it, `C-g` drops it into oso's kill ring, off the
  clipboard. It takes Japanese at once, as a buffer does. A draft that holds secret text, or whose line or comment is gone, is not kept: it
  stays open, and `C-g` keeps its text.
- An agent's comment counts as read once you have had it whole on screen, unfolded, after a key
  of yours.
- `C-c C-c` gathers your comments that were not sent, outside threads you resolved, in the
  order the review shows them, into `*review message*` (ADR 0122, step 12): a line saying what
  to do, the scope, then each comment's `## path  L42 (new)`, its hunk in a `diff` fence (cut to
  30 lines around the comment past 60), the comment it replies to, and `Comment: …`. The echo
  area names the session it goes to. The buffer is yours: an agent may read it, never change it.
  There `C-c C-c` pastes it into the session, as `session-send` does, not submitted, and `C-u
  C-c C-c` picks the session by completion; `C-c C-k` drops it and shows the review again. Once pasted, the comments it
  held are drawn dimmed, but any changed meanwhile; one message waits at a time.
- **In a space** whose orchestrator is recorded ([ADR 0163](adr/0163-reviews-in-a-space.md)),
  the message goes to the orchestrator unless `C-u C-c C-c` picked a session, its first line
  naming the review's worktree; with the orchestrator stopped, it stays, and the echo area names
  the key that resumes or starts it.
- `Q` (`review-ask`) reads a question in the prompt and sends it at once, submitted, to the
  space's orchestrator (outside a space with one, to the review's session): a line saying what it
  is, the review's scope and worktree, then `## path  L42 (new)` and the hunk at point, the lines
  the region spans or an unchanged line, in a `diff` fence, and `Question: …`. The orchestrator's
  terminal shows in the layout's slot, the review staying selected. It goes to the orchestrator
  only for a review of one of the space's repositories or worktrees. Claude Code's input box is
  read from the screen: empty, the question is pasted and Enter goes as a key of its own once the
  box shows the folded paste alone (150 ms to 2 s after it), else it stays unsubmitted; with text
  typed, it is pasted, not submitted; with no box (a dialog, a picker, bash or memory mode),
  nothing is pasted. A refused question shows the terminal and says why; a stopped orchestrator
  is offered to be resumed (`y`), the question held for it two minutes, its file judged for
  secrets again before it goes.
- **Starting one in a space** whose orchestrator is recorded (ADR 0163): `review-layout`'s menu
  asks for the repository by completion over the space's trusted ones, only with two or more,
  then opens `review-start` (`p` a pull request, `w` a worktree by completion, `.` this
  checkout), whose line names the repository; `w` and `.` open the review menu for the worktree,
  its line and counts that worktree's. `w` is unavailable, saying why, while the repository has
  one worktree. `p` says "Finding #N's worktree…" and opens the review on the worktree its head
  is checked out in, else on the main checkout.
- `C-x k` closes the review, asking first while comments of yours that were not sent, or a draft,
  would be lost; a review not shown is shown before the question.
- **The review layout** ([ADR 0160](adr/0160-the-review-layout.md)): a review a person opens
  takes the frame as the review at the left, three fifths, and the slot at the right, two
  fifths. The slot shows one thing at a time: the source `RET` goes to, the side `C-u RET` shows,
  the message `C-c C-c` makes and the session `Q` asks (ADR 0163), each in place of the last. `C-x v r` (`review-layout`) goes
  back from the slot to the review where the person left it, and in the review puts back the
  windows from before the layout, exactly, as `q` and closing the review do. The two panes take
  the layout's left, the files pane the review's left. The windows are found by name, so a slot
  deleted by hand comes back with the next jump; each space keeps its own layout, and a zoom
  nests inside it. A restart keeps the layout but not the windows from before it.
- **A pull request a commit at a time** ([ADR 0167](adr/0167-reviewing-a-pull-request-a-commit-at-a-time.md)):
  `M-n` and `M-p` in a pull request's review step through its commits, each reviewed alone in
  the layout's left with its message under the title; merges and commits that change nothing
  are stepped over. `l` shows the commit list above the review: the whole pull request, then a
  row a commit with `▶` for the one shown, `✓`/`◐` for its hunks viewed, `●N` for its comments
  not sent and `↑` for one not pushed; `RET` shows a row's review, `SPC` shows it and stays.
  `C-c C-c` in any of them sends the comments of the whole and of each commit, a group each.
- In a restricted project the commands and the menu's heads say "Reviewing runs git, which this
  project is not trusted to run: M-x trust-project".

## Directory listings

`C-x d` reads a directory and `C-x C-j` lists the one of the buffer's file, with point on the
file, in a listing buffer named after the directory, `crates/`, as Emacs's dired shows it
([ADR 0134](adr/0134-a-directory-editor.md)). Listing runs nothing and opens no file.

- The first line is the directory, the home directory as `~`. Each row is the mode as `ls -l`
  writes it, the owner and group only when someone else owns a row, the size as `ls -h` (`-` for
  a directory), the time as `ls` and the name, a link's target after `->`; directories first,
  then by name or, with `s`, newest first. Backups ending in `~` are left out. `(` shows names
  only. The modeline says `Dired by name` or `Dired by date`.
- Names are in faces by kind: directories, links, broken links, files someone may run, and pipes,
  sockets and devices; the details are dim. A line break in a name shows as `\n`.
- A large directory shows what it has read every 50 ms, under `reading… N entries so far`; one
  that cannot be read says why.
- `n`/`p` (and `SPC`) go by rows to the name, `RET` (or `f`) opens a file or lists a directory in
  the same buffer, `o` does it in another window, `^` lists the directory above with point on
  the one left, `g` lists it again, `w` copies the names at the cursors (`0 w` their absolute
  paths), `q` shows the buffer before. `C-x k` kills a listing, and `C-x b` goes back to one.
- A shown listing is listed again within a second of its directory changing, each window's
  point on the name it was on.
- `m` marks the row at point and moves down, `u` unmarks and moves down, `DEL` moves up and
  unmarks, each with a count, or over the rows a shown region covers; `U` unmarks every row and
  `t` toggles every mark. A mark is a `*` before the row, in the warning's colour with the row's
  name, and the modeline counts them: `Dired by name  3 marked`. Marks stay over a refresh.
- `C` copies and `R` moves the marked rows, or the row at point, to a path read from the
  directory of the next window's listing (`C-u`: this one's). `D` deletes them after a question
  that names them, and `+` makes a directory with its parents. Before writing over anything, a
  question names what is there. Past half a second the echo area counts what is done, and `C-g`
  stops after the file in hand.

## Git status

`C-x g` shows the git status of the selected window's project in `*git: NAME*`, as magit's
status buffer ([ADR 0127](adr/0127-source-control-and-github.md), step 1a). It runs git, so only in
a trusted project; a restricted one says "The status runs git, which this project is not trusted
to run: M-x trust-project".

- The first lines are the head, `Head:     main 1a2b3c4 Subject`, the upstream with ahead and
  behind, and `Push:` when git pushes elsewhere; then, while a merge, rebase, cherry-pick,
  revert or `git am` is stopped (step 3a), what it is, such as `Rebasing: main onto 1a2b3c4, 2
  of 5; stopped at 5d6e7f8 Subject`, and a line naming its keys (see
  [What is stopped](#what-is-stopped)). Then the sections in magit's order, each
  `Heading (N)`: unmerged paths, untracked files, unstaged changes and staged changes. A file's
  row is its status letter, its path (a rename as `old -> new`) and its lines added and removed.
  Then the commits (step 2): `Unpushed to origin/main`, `Unpulled from origin/main`, at most
  256 each, and the 10 `Recent commits`, each row a short id and a subject; `RET` on one opens
  its review. They are read from what the last fetch left: a refresh fetches nothing.
- `TAB` on a file shows its hunks under it, as git's patch has them, in the diff faces; on a
  section's heading it folds the section (`…`); on a hunk it hides its file's hunks. A file that
  may hold secrets, an untracked or unmerged file and a submodule show why they have no hunks
  instead, and stage as a whole.
- `s` stages and `u` unstages what each cursor is on: a section's files, a file, or a hunk alone;
  a region stages each heading it holds. `S` stages every unstaged change of tracked files,
  `U` unstages every staged one. A write to the index has no time limit, and `C-g` stops it; it
  is refused while a commit runs. `$` shows the log of git's runs. A region inside a hunk
  stages or unstages only the lines it holds (step 1c). A hunk git no longer accepts is
  refused, which reads the status again and says so. A hunk's changed lines mark the words that
  changed, on the refined backgrounds.
- `k` discards what each cursor is on, lines too, after a question that names it: unstaged
  changes from the work tree, staged ones from the index and the work tree, untracked files
  deleted (at most 100 files and 64 MiB). Each file is kept first as one of git's objects for
  two weeks, but one that may hold secrets, which the question says; `M-x git-discarded` lists
  them in `*git discarded: NAME*`, where `RET` writes one back after a question, keeping what
  the file holds then; `g` reads the list again and `q` leaves it. An unmerged path, a
  submodule, a staged rename, a file with unsaved edits, a file with more than one name or for
  its owner's eyes only, more than 1000 files kept, and a discard while a commit runs are
  refused, before the question.
- `n`/`p` go to the next and previous heading, `M-n`/`M-p` to the next and previous of the same
  depth, `^` to the section holding point's. `RET` visits the file, at the line a hunk's line
  shows. `g` reads it again, `q` shows the buffer before, `?` lists the keys. `C-x k` kills a
  status, and `C-x b` goes back to one.
- It is read again when it becomes shown, on `g`, after each staging, and when a file of the
  repository is saved while it shows, each window's point kept on what it was on. What an agent
  does reads nothing: its save, or a status its command left shown, says `stale: g reads it
  again` in the modeline until the person's next key in it.

### Committing

`c` in a status opens the commit menu (step 1b): `- n` `--no-verify`, `- s` `--signoff` and
`- e` `--allow-empty` flip their switches, then `c` commits, `a` amends, `e` extends the last
commit with what is staged and keeps its message, and `w` rewords it alone. With point on a
commit of Unpushed or Recent commits, `f` makes a `fixup!` commit of what is staged for it at
once, and `s` opens a message for a `squash!` one (step 2); oso does not rebase, so `git rebase
-i --autosquash` in a shell folds them in.

- A commit, an amend and a reword open `*commit: NAME*`: the message, then `#` lines with the
  branch and the staged files, which are left out. Past 50 characters on the summary, past 72
  on a body line, and a second line that is not empty, show in the warning's colour. `C-c C-c`
  commits, `C-c C-k` cancels, and `M-p`/`M-n` bring back the last 20 messages committed or
  cancelled.
- git runs the repository's hooks, with no time limit: the echo area counts the seconds, `$` in
  the status shows `*git log: NAME*` with what git and its hooks wrote, and `C-g` there or in the
  status, or `k` in the log, stops it (`SIGTERM`, then `SIGKILL` after 5 s). A commit that fails
  keeps its message and says `commit failed (hook pre-commit): $ shows the output`, adding `;
  C-c C-f asks the session to fix it` when an agent session runs in the worktree.
- The worktree's agent session (step 6a): `C-c C-d` in the message pastes it a request for a
  draft, which it gives back with `draft_set`, above the `#` lines, the person's own text kept
  below as `#` lines. `C-c C-f` in a failed commit's message, or `F` in the log, pastes it the
  failed run's output to fix. Each paste shows the session, not submitted: the person reads it
  and presses Enter there.
- Nothing staged, an amend before the first commit, an empty message, a second message while
  one is open, or a second commit while one runs, is refused before git runs. `C-x C-c` and
  `oso-restart` ask while git commits.
- A hook gets oso's allowlist of variables, `XDG_CONFIG_HOME`, `SSH_AUTH_SOCK`, `GNUPGHOME` and
  `GPG_AGENT_INFO`,
  and the names `[git] pass_env` lists, passed on from oso's own environment; `PATH`, `LD_*`,
  `DYLD_*` and git's own `GIT_*` are refused there.

### Remotes

From a status (step 2), each run as a commit's: no time limit, the repository's hooks, `$` for
its output and `C-g` to stop it, one at a time; the echo area shows git's progress, and the
status is read again after.

- `P` opens the push menu: `- f` pushes with `--force-with-lease`, after a question that names
  the commits it would drop there, and `- n` skips the pre-push hook; then `p` pushes the branch
  to where git pushes it (`@{push}`), `u` to its upstream, and `e` elsewhere, read as
  `REMOTE/BRANCH`. With no push target or no upstream, a prompt asks for the remote, and `u`
  sets the one picked as the upstream. Never a plain `--force`. A plan that comes back once the person
  has left the status, or opened another prompt, opens nothing and says to press `P` again.
- `F u` pulls the upstream, fast-forward only; a branch gone apart from it is refused. With the
  pull menu's `- r` it rebases onto the upstream (`git pull --rebase`), and with `- m` merges
  it, a fast-forward when it can be and else a merge commit with git's message (`git pull
  --no-rebase --ff --no-edit`); both on is refused (step 3a). A pull that stops on a conflict
  says so, and the status shows it stopped. `f u` fetches the upstream's remote and `f a`
  every remote, pruning what each deleted.
- git runs with no terminal: a password, a passphrase or a key it cannot get fails at once, and
  the echo area says to set up a credential helper or `ssh-agent`, or to push from a terminal.
  The proxy variables (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`, either case) are
  passed on.

### What is stopped

While a merge, rebase, cherry-pick, revert or `git am` is stopped (step 3a), from a pull or a
terminal, the head says what and where, and the conflicted files are under Unmerged paths:
`RET` visits one with its markers, and `s` stages it once resolved.

- `C-c C-c` (`git-continue`) goes on: `git rebase --continue` and the like, with the
  repository's hooks and the message git prepared, as a run of its own. It is refused while
  unmerged paths are left, naming `s`.
- `C-c C-s` (`git-skip`) skips the commit it stopped at, or `git am`'s patch, after a
  question that names the commit; a merge has no skip.
- `C-c C-k` (`git-abort`) aborts, after a question that says what goes and where `HEAD` goes
  back to.
- A `y` runs only for the status it was asked over, and only if what is stopped is still what
  the question was asked over: the same commit, branch, start and step.
- A bisect is shown only: `git bisect reset` in a shell ends it.
- While one is stopped, the commit menu's heads and a pull are refused, naming `C-c C-c` and
  `C-c C-k`; staging, discarding, pushing and fetching stay.

### Stashes and branches

From a status (step 3b), each run as a commit's, with the repository's hooks and `$` for its
output.

- **Stashes** show after the staged changes, `Stashes (N)`, each row `stash@{N} On BRANCH:
  message`, every stash of the repository, as the worktrees share them. `RET` on one opens its
  review, the changes it holds against where it was made. `z` opens the stash menu:
  - `z` stashes the changes after a prompt for its message, empty for git's own, and `- u`
    takes the untracked files too;
  - `a` applies the stash at point and keeps it, `p` pops it, and `k` drops it after a question
    that names it. With point on none, a prompt asks which.
  - A stash is acted on by its id: an apply gives git that id, and a pop or a drop finds git's
    own `stash@{N}` for it just before git runs, so that one another worktree pushed meanwhile
    does not take its place; one gone is refused. A prompt's answer names a stash it listed. The
    echo area names a stash by its subject. One that does not apply cleanly leaves its files
    under Unmerged paths, and a pop keeps it.
- **Branches:** `b` opens the branch menu, each head reading its branch in a prompt, local
  branches before the remotes':
  - `b` checks one out; a remote's makes a local branch that tracks it.
  - `c` makes a branch and checks it out, and `n` makes one only: first where (`HEAD`, a branch,
    or any revision typed), then its name.
  - `m` renames one, the branch checked out first.
  - `k` deletes one: a branch merged into its upstream, or `HEAD` with none, goes at once; one
    that is not asks first, naming the commits only it holds, or saying oso could not tell, and
    goes only if it has not moved since. One checked out in another worktree is refused, naming
    `worktree-remove`; the one checked out here is not listed.
  - A prompt opens only while the person is still in that status with no prompt open; else the
    echo area says to ask again. A branch or a start picked goes to git as the ref listed, a
    remote's in full.
- While something is stopped, checking out, making a branch to check out, applying and popping
  are refused, naming `C-c C-c` and `C-c C-k`.

### Graph

`C-x v l`, or `l l` in a status, shows `*graph: NAME*`: the branch's commits in git's
topological order, children before their parents, one row each (ADR 0127, step 4).

- A row is the lanes in box-drawing cells, each lane in a colour of its own, the commit as `●`;
  then the short id, the refs on it (`HEAD`, branches, remote branches, tags), the subject, the
  author and the date, `YYYY-MM-DD`.
- The `l` menu, in a status or a graph: `l` the branch checked out, `a` every ref, `o` another
  ref, read in a prompt over the branches, `HEAD` first, or any revision typed.
- In a graph: `RET` reviews the commit at point, `w` copies its id, `a` shows every ref or the
  branch again, `g` reads it again, `n` and `p` move a line, `q` leaves it.
- What a graph's keys do with a commit (step 4b): `m` marks it, and `RET` with two marked reviews
  the range between them; `c` checks out the branch on its line, asking which if there are
  several, or the commit, detached, after a question; `b` makes a branch there and `t` a tag,
  `C-u t` an annotated one; `C` cherry-picks the marked commits, oldest first, or the one at
  point, and `V` reverts it, each after a question. A merge's revert is refused. While
  something is stopped, `c`, `C` and `V` are refused, and a pick or a revert that stops is the
  status's to go on with.
- It reads 2,000 commits at a time, the next as point nears the end, up to 50,000, then says a
  narrower scope shows the rest. At most 16 lanes are drawn, `┄` past them.
- Pull requests (step 5a), through `gh`: the status's head says `PR:       #12 TITLE  open,
  approved  ✓ 12 ✗ 1 ● 2`, `stale` if it is not the commit checked out. `N` opens the pull
  request menu: `v` shows `*pr: NAME*` (its state, branches, description, checks, review
  threads, unresolved first, and the conversation), `k` the same at its checks, `o` opens it in
  the browser after the echo area shows its address, `l` lists the open ones. In those buffers
  `RET` visits a thread's file at its line, shows a check's address, or shows a listed one; `o`
  opens, `w` copies an address, `g` reads it again, and `r` reviews the pull request (ADR
  0122, step 12b), `*review: pull request #N <project>*`, the echo area saying `Reviewing #N,
  its head commit shows · BRANCH · TITLE` (or `checked out here, so the work tree shows`),
  and `Fetching #N's commits from REMOTE: closing the review stops it` while it fetches.
- Making and merging (step 5a-2): `N c` opens `*pull request: BRANCH*`, the branch's commit's
  message or its commits' subjects above the template, its first line the title; `C-c C-c` makes
  it, asking first to push a branch with no upstream or ahead of it, `C-c C-k` cancels, `M-p`
  and `M-n` walk the messages kept. `- d` makes a draft, `- b` reads the base. `N m` opens the
  merge menu: `s`, `m` or `r` after a question naming the pull request, its base, the method and
  its head; `- d` deletes the branch on the remote.
- Each worktree's open pull request (step 5b-2) shows on its `*Worktrees*` row and beside its
  branch in the `*Spaces*` tree, as `PR #12 open, approved ✓ 3 ✗ 0 ● 1`.
- In `*pr: #N*` (step 6b), `x` sends the check on the line to an agent session, `X` every
  failing one with the logs of at most 8 Actions jobs, `c` the review thread on the line and
  `C` every unresolved one, through `*review message*`: GitHub's words under a line saying they
  are data, quoted or fenced, the comments of others than you and the repository's owners,
  members and collaborators left out and counted, as a fork's logs are; `C-c C-t` there takes
  them in above the end of what GitHub said.
- Issues (step 5b-1): `C-x v i`, or `N i`, shows `*issues: NAME*`; `RET` shows `*issue: #N*` with
  its comments; `w` opens `*Brief*` for a worktree session on `issue-N-SLUG`, the issue's text
  under a line telling the agent it is data; `o` opens it, `y` copies its address, `g` reads
  again.

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
- `C-h l` shows the last 300 keys oso received in `*Lossage*`, each with what it ran, and
  `C-h k` reads a key sequence and says in `*Help*` what it runs, without running it. Keys typed
  where secrets may be show as `<hidden>`, as do characters sent to a terminal
  ([ADR 0150](adr/0150-view-lossage-and-describe-key.md)).

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
- The space in front, after the project, in the selected window's alone: its display name,
  cut to 20 cells, in the dim face, as `restricted` is
  ([ADR 0159](adr/0159-space-new-starts-the-orchestrator-and-display-names.md)).

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
  takes, typed before any of a label, opens `goto-line` with it. `avy-goto-end-of-line` labels each line's end and has no key but `e` in the `jumps` menu. `M-j` in
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
- Keys a terminal sends to its program are not recorded ([ADR 0120](adr/0120-the-terminal.md)).

## Terminals

[ADR 0120](adr/0120-the-terminal.md); step 4a so far.

- `M-x terminal` switches to the shell terminal still running that was selected last, or starts
  one; with `C-u`, it always starts one. A session's `*claude: NAME*` buffer is never the one it
  switches to, so from a session it gives a shell
  ([ADR 0156](adr/0156-terminal-switches-only-to-shells.md)). Buffers are `*terminal*`, `*terminal*<2>`
  and on. `char` in char mode, `exited N` once the program ends, and then the program's title
  in quotes follow `Terminal` in the modeline. An ended terminal keeps its last screen.
- It starts `[terminal] shell` (an absolute path), else `$SHELL` if absolute, else the account's
  shell, else `/bin/sh`, in the selected buffer's project root, else the directory it stands in
  (from a terminal, the one that terminal started in), else the home directory. A restricted project opens one too: what runs in it is the person's.
  `*Messages*` says `Started <shell> in <dir>`.
- `[terminal] login = true` (the default) starts it as a login shell (`-l`). With `false`, the
  shell is interactive but not a login shell, and starts with oso's own environment: from the
  Dock that is the GUI's short `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`) unless its startup files
  set one. bash then reads `~/.bashrc` and not `~/.bash_profile` or `~/.profile`; zsh reads
  `~/.zshenv` and `~/.zshrc` but not `~/.zprofile`; fish reads `config.fish` either way.
- The environment is the process module's allowlist, `SHELL`, `TERM=xterm-256color`,
  `COLORTERM=truecolor`, `TERM_PROGRAM=oso`, `TERM_PROGRAM_VERSION`, `SSH_AUTH_SOCK`, and the
  names `[terminal] env` lists, passed on from oso's own environment. `PATH`, `LD_*`, `DYLD_*`
  and the names a terminal sets itself are refused there, and reported.
- `[terminal] scrollback` keeps 0 to 100,000 lines above the screen, 10,000 by default; the
  library frees them a page at a time, so a few hundred fewer may be kept. The wheel scrolls
  them; the next key sent to the program goes back to the bottom.
- **Semi-char mode**, the default, sends every key to the program but `C-x` and `M-x`, the
  `C-c` prefix, `C-q`, `C-y` and every key with Super (ADR 0143), and every key while a prompt, a
  search or a key sequence of oso's is open. `C-c C-c` sends `C-c`; `C-c C-k` switches to char
  mode; `C-c a` opens the Agents menu; `C-c` and any other key sends both, the global and the user's `C-c` bindings among them: in a terminal, `C-c` is the
  terminal's. `C-q` sends the next key as it is, oso's own among them. Text the input method
  commits goes as a key does.
- **Char mode** sends every key but `C-c C-k`, which comes back to semi-char mode; `C-c C-c`
  sends one `C-c` there too, and `C-c` with any other key sends both.
- Keys are encoded on the terminal's thread against the program's modes, in xterm's forms, or
  the kitty keyboard protocol's once the program pushes it, so `S-RET` reaches a program that
  reads it. `M-` is sent as Alt, with an `ESC` before the key.
- `C-y` pastes the newest kill, or what another app put on the clipboard; `C-u N C-y` the Nth.
  Control characters but tab and new line are removed first, and line breaks of every kind
  become new lines. With bracketed paste on, the text goes between its brackets. With it off,
  each new line is sent as `RET`, and text of several lines asks `Paste N lines into a program
  that may run them?` first; `keep` drops it.
- The input method composes at the program's cursor, and its candidate window opens at that
  cell even while the program hides the cursor; the committed text goes to the program.
- The PTY takes the size of the window that last selected the terminal; other windows showing
  it clip or pad the same grid.
- `C-x k` on a terminal whose program still runs asks first, as `C-x C-c` does for every such
  terminal. Under `oso dev`, a running terminal holds the restart, until terminals are handed
  over: run long-lived programs in an oso that is not rebuilt.
- The agent channel reads no terminal's text, types nothing into one, and never selects one:
  switching to a terminal, or to a list with keys of its own such as `*Spaces*`, is the
  person's.

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
- The IME is switched off automatically in isearch, a jump, query-replace's answers, a menu,
  the list of emergency copies and a question, since those use single keys, and in the prompts
  that read a number or a passphrase. In
  a prompt that reads text or completes one (`C-x C-f`, `M-x`, a session's note, a stash's
  message and the others), the input source switches to ASCII when it opens and back when it
  closes, and choosing a Japanese input mode there, by `かな` or the menu bar, turns the IME on
  for that prompt; `英数` turns it off again
  ([ADR 0106](adr/0106-the-input-source-turns-japanese-on-in-a-file-name-prompt.md),
  [ADR 0158](adr/0158-japanese-in-prompts-and-review-drafts.md)). A review comment's draft has
  the IME on as a buffer does, with the input source left as it was. It is
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
on a visit and on `C-x C-w`, and set by hand with `rust-mode`, `markdown-mode`, `toml-mode`,
`go-mode`, `typescript-mode`, `tsx-mode`, `javascript-mode`, `html-mode`, `css-mode`,
`json-mode`, `yaml-mode`, `latex-mode`, `swift-mode` and `fundamental-mode` (`M-x`, no keys),
which refuse an internal buffer. Go, TypeScript, TSX and JavaScript are highlighted too (ADR
0147), and HTML, CSS, JSON and YAML (ADR 0157): an HTML page's `<script>` is JavaScript, or
JSON when its `type` says so, and its `<style>` CSS, up to 16 KiB an element. LaTeX and Swift
have no highlighting yet: they are
there for their language servers (ADR 0138). Setting a mode again, the same one too, gives a
buffer whose highlighting stopped another try. A file over 2 MiB, or with a line longer than
16 KiB (minified or generated code), stays plain (ADR 0147). The modeline names
the mode.

Off by default: **line numbers** (`global-display-line-numbers-mode`). They are absolute, the
current one brighter, in a gutter as wide as the line count's digits plus one, at least
3 digits. Floats keep their place when it appears.

In a buffer a language server follows, the gutter's first cell is a **sign column**, there with
line numbers off too and before the first report, so the text does not move. A line a diagnostic
starts on has a bar a third of a cell wide in the most severe one's colour: red for an error,
amber for a warning, blue for information. Under its range, a wavy line in that colour; code the
server calls unnecessary is faded, keeping its colours, and deprecated code is struck through.
Hints get no line, sign or count. The modeline shows `lsp` and the counts, `E2 W5 I1`, each in
its colour ([ADR 0138](adr/0138-language-servers.md)).

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

## Agent sessions

- `C-c a c` (`claude`) starts Claude Code's own TUI in a terminal buffer, `*claude: NAME*`, in
  the selected buffer's project root, else the directory it stands in, as a new terminal does, else
  the directory oso started in. NAME is the directory's
  name, then `-2`, `-3` for more sessions there. `C-u C-c a c` asks for the directory, then the
  name. The agent channel turns on if it was off, and the echo area says so: the session's own
  oso tools connect without asking, and any other agent still asks
  ([ADR 0121](adr/0121-agent-sessions-and-spaces.md)).
- oso remembers its sessions. At the next start, each comes back as a buffer with no process,
  `stopped: session-resume to continue`; nothing runs until the person asks.
- `C-c a r` (`session-resume`), in a stopped session's buffer, continues its conversation in the
  same buffer. If Claude Code runs that conversation elsewhere, it refuses, names the process and
  its directory, and asks whether to fork instead; `y` forks that session, whichever buffer is
  selected by then. `session-fork` copies the conversation into a
  new session and buffer.
- `C-x k` on a session's buffer archives the session: it is not brought back at the next start.
- A session's buffer has session mode ([ADR 0132](adr/0132-a-session-mode-for-hosted-sessions.md)):
  its modeline says `Session` in place of `Terminal`, and `C-c C-<letter>` runs the Agents menu's
  head of that letter where the terminal and Emacs leave `C-<letter>` free (ADR 0143): `C-c C-n`,
  `C-c C-d`, `C-c C-b`, `C-c C-r`, `C-c C-a`, `C-c C-w`, `C-c C-e`, `C-c C-f`, `C-c C-z` and
  `C-c C-v`, with `C-c C-p` for the pane (`s`), as `C-c C-s` is the live search. `C-c a` opens the
  menu, for `c`, `m`, `j`, `O`, `S`, `K`, `P`, `A`, `D` and `V`; `C-c C-v` is `review-layout`
  since ADR 0163, `C-c a D` the session's review. `C-c @` mentions, `C-c +` and `C-c -`
  allow and deny the agent channel's reads, `C-c C-u` is a prefix argument, `C-c C-y` pastes an
  `@` reference to the clipboard's image, kept in a private file
  ([ADR 0142](adr/0142-attaching-the-clipboard-s-image-to-a-session.md)), and `C-c ?` says the
  keys in the echo area. After `C-c`, only the session's and
  the terminal's keys are read, and the user's `[keys.session]` over them, never the global keys
  or `[keys.global]`; any other key, or one `[keys.session]` sets to `false`, says
  `C-c g is undefined (C-c C-c sends C-c)` and sends nothing. `C-z` says
  `C-z would suspend Claude Code, and nothing here resumes it; C-q C-z sends it`. In char mode
  every key but `C-c C-k` goes to Claude Code, as in any terminal. The buffer is in the project
  of its session's directory, whose name its modeline shows.
- `C-c a w` (`session-worktree`), in a trusted project whose main repository is trusted too,
  starts a session in a new git worktree.
  - It reads a branch, completing the project's local branches; a new name is a new branch from
    `HEAD`, or from a base that `C-u C-c a w` reads after it. The echo area says "Listing
    branches…" until they come; another command, or `C-g`, drops the prompt.
  - The brief is written in `*Brief*`: `C-c C-c` sends it, `C-c C-k` drops it.
  - The worktree goes in `.claude/worktrees/<branch's slug>`. The files that `.worktreeinclude`
    names and git ignores, such as `.env`, are copied in.
  - The session gets the brief as its first prompt, after a header that says where it is. From
    a session's buffer, the brief also names that session to report back to, with its
    transcript, plan files, uncommitted changes and the files it touched.
- Starting a session where another one runs asks first: a worktree keeps their changes apart.
- `M-x worktree-list` shows `*Worktrees*`: each linked worktree with its branch, uncommitted
  and untracked files, whether its branch is merged and how that was found, its sessions and
  their states, unsaved buffers, its space, and whether it can be removed. `gh` is asked last, only for a branch that was
  pushed and that its remote's main branches do not hold. `g` lists again, and `D` removes the
  one at point. A worktree whose removal runs says `removing…` instead. A row that is clean and
  merged makes its sessions that were ready for review idle: their work needs no review. An
  agent's `worktree_list` does so only for the sessions of its own space.
- `M-x worktree-remove` removes the worktree of the selected buffer, or of the row at point,
  only when nothing would be lost. Each of these must hold, checked in this order:
  - it is a linked worktree of this repository;
  - it has nothing uncommitted or untracked;
  - no buffer in it is unsaved and no brief for it is unsent;
  - no session in it is busy: one oso hosts is busy unless its state is idle, done or ready for
    review and Claude Code's registry calls it idle, in an entry whose start time matches its
    process; none oso did not start runs in it, the registry was read in full, and each such
    session there has a directory oso can read;
  - its branch is merged, into a remote's main branch or through a merged pull request that
    `gh` knows of.

  It then asks, naming what git ignores there and will delete with it, such as `target/`. `y`
  runs every check again before it removes the worktree. It never uses `--force`, and the branch
  is kept. git has no time limit there, as a large `target/` takes time to delete. Once git
  removed it, the sessions that were in it are archived, of every space, and the echo area ends
  "; N sessions in it archived": else each would show under the repository's main checkout,
  which holds the gone path. As oso starts, a stopped session whose directory is gone from
  under the main checkout of one of the spaces' repositories is archived too, and `*Messages*`
  says how many.
- `M-x worktree-remove-stop` stops a running removal: the row's at point in `*Worktrees*`,
  else the only one. Before git runs, nothing is removed. Once git may be deleting, it asks
  first, as the worktree is then left half removed for you to remove by hand.
- `C-c a a` (`session-adopt`) brings a session started outside oso into oso. It completes over
  the sessions oso does not host: those Claude Code says run (`runs as process N: stop it there
  first`), then those that stopped in the last 30 days, newest first, each with its age and
  directory. A picked one is resumed in a new buffer, `*claude: NAME*`, in the directory it ran
  in, under the name it had. A live one is refused until it is stopped where it runs, and `y`
  forks it instead. If it was working when it stopped, the echo area says to ask it to go on.
- A session started outside oso that calls `move_to_space`
  ([ADR 0151](adr/0151-moving-a-session-into-a-space.md)) asks, as every agent question does:
  `Agent: move session "NAME" ("claude", pid N, in "DIR", outside oso) into space "X", adding
  repository "R" to it? …`, the process named as the system names it, and `adding …` only for
  a repository the space lacks; `?` shows the conversation's id and the whole paths. On `y`
  an internal buffer `*moving: NAME*` says what oso waits for, and the echo area says the
  session stops where it runs once its turn ends, or to type `/exit` there; its bridge stops
  it then. At 120 s the echo area asks for `/exit`; at 5 minutes, or when the space
  closed or lost the repository meanwhile, oso resumes nothing and says to use
  `session-adopt`. Killing `*moving: NAME*` with `C-x k` cancels. Once the outside process has
  ended, `*claude: NAME*` resumes in its directory, in space X, and the echo area says
  `Moved "NAME" into space "X"`. None of these lines is logged.
- `C-c a /` (`session-search`) finds a session by what was said in it
  ([ADR 0145](adr/0145-searching-past-agent-conversations.md)). It opens a centred float,
  `Search conversations (all):`, over every Claude Code conversation on this machine, of every
  project; `C-c C-s` in the float moves to the space in front's (`(space NAME)`), then the
  current project's (`(project NAME)`), then back, and `C-u C-c a /` starts in the next one.
  The input is `project-search`'s: words in any order, `!` to exclude, regular expressions,
  smart case and migemo. A row is one prompt or reply that holds every word:

  ```text
  ● oso-2 · oso-2 · 3m › …can worktree trust survive a rename?…
  ◌ Fix the login redirect · webapp · 12d ‹ …the trust record keys by inode…
  ```

  `●` runs here, `○` is stopped here, `⊘` runs in another program, `◌` is not oso's; then the
  name, the place (`repository/worktree`, `(gone)` once its directory is), the age, and `›` for
  a prompt or `‹` for a reply. Rows come newest conversation first, at most 50 a conversation.
  The preview shows the messages around the row, wrapped. `RET` shows a session oso hosts and
  adopts any other, as `C-c a a` does; one whose directory is gone is not resumed. `C-RET`
  reads the row's conversation whole in `*conversation: NAME*`, read-only and withheld from
  agents, where `n` and `p` go from message to message, `r` goes to the session, `f` forks it,
  `g` reads it again and `q` quits; `M-RET` forks it at once. Only prompts
  and replies are searched, never tool inputs or results, thinking or attachments. What was
  read is held in memory only, for 5 minutes after the last search, and no agent gets it.
- `config.toml`:

  ```toml
  [sessions]
  claude = "/usr/local/bin/claude"   # absolute; else the login shell finds it on PATH
  env = ["HTTPS_PROXY", "NODE_EXTRA_CA_CERTS"]
  steer_worktrees = true             # sessions oso starts are steered to C-c a w
  pane_width = 44                    # *Spaces*' pane, 24 to 80 columns
  mode = "native"                    # new sessions in trusted projects run claude -p (ADR 0165)
  ```

  With `mode = "native"` (ADR 0165, step 1), a new session in a trusted project runs `claude -p`
  and shows its conversation in `*claude: NAME*` as oso's own rows instead of a terminal: the
  person's messages after `› `, replies after `● `, thinking as `∴ Thought for N s`, each tool
  call as one line with what came of it, an edit as a diff, and each turn's time and cost. A
  start with no brief opens `*Brief*` for its first message: `C-c C-c` starts the session,
  `C-c C-k` drops it. Its state comes from its stream. In its buffer, `TAB` folds or unfolds
  the row at point (a call of an open row of several on its own), `RET` shows a call's whole
  result read-only in `*result: NAME*`, withheld from agents, `n`/`p` move between the person's
  messages and `]`/`[` between tool rows; session mode's `C-c` keys work there too, but
  `C-c +` and `C-c -`, as it has no screen for an agent to read. In this step
  it takes no message after
  its first, and a send, a paste, `session-explain` or a resume to it says so (`session-fork`
  continues it in a terminal); elsewhere, an agent's
  start with no brief, and a restricted project, keep the terminal.

  `claude` is started as the login shell would start it, so its `PATH`, version managers and
  proxies apply; set `claude` when the shell is not `sh`, `bash`, `zsh`, `dash` or `ksh`. `env`
  names at most 64 more variables a session takes from oso's environment; `PATH`, `LD_*` and
  `DYLD_*` are refused. A session never gets a permission option: it starts in the mode Claude
  Code's own settings give it. With `steer_worktrees`, a session oso starts gets a hook that
  denies Claude's own `EnterWorktree` and `git worktree add`, pointing it to `C-c a w`; nothing
  is written into Claude Code's settings files.
- A session gets oso's `CLAUDE_CONFIG_DIR`. If a login profile sets its own, set the same value
  where oso starts too: oso reads Claude Code's registry there to refuse resuming a session that
  runs elsewhere.
- Which states ask for attention, in `config.toml`:

  ```toml
  [sessions.notify]
  needs_input = true        # the defaults
  done = true
  failed = true
  ready_for_review = false
  ```

  A session that enters one of these while it is not shown in the selected window of a focused
  oso window, and is still in it 1 s later, gets an echo-area line naming it, and on macOS a
  Dock bounce: repeated until oso is focused for needs input and failed, once otherwise. On
  Linux and Windows, the window's urgency hint. Several due within 2 s are one line ("3
  sessions need input"). A notification names the session and its state, never screen text.
- `*Spaces*` lists every session in a side window at the frame's left, 44 columns wide unless
  `[sessions] pane_width` (24 to 80) says otherwise, and narrower, down to 24, where the frame
  has no room for that beside the tree's narrowest window; below 24 it takes no cells. `<` and
  `>` in it make it 4 columns narrower or wider, times the count, within 24 to 80, for this
  oso: never written to the configuration, and back to `pane_width` after an `oso dev`
  restart; a changed `pane_width` replaces it. Under 36 columns a row drops its age before its
  name is cut below 12 cells. It is shown
  by itself while oso has a session that is not archived. Each session takes one line: its
  state's mark (⚠ needs input, ✗ failed, ▶ working, ◆ ready for review, ● done, ○ idle,
  ■ stopped), its name, cut in the middle when long so that its end shows, its space and
  branch where there is room, and its age at the right: the time since it last changed state,
  `now` under a minute. A session that needs input or failed takes a second line, its reason:
  the dialog's question. Marks take their state's face: needs input and failed red, working
  amber, ready for review and done green, idle and stopped grey; a working mark of a session
  oso hosts turns through ten Braille frames, 8 a second, while it is in view, unless
  `[sessions] spinner = false`. A session oso hosts that needs input blinks while it is in
  view, a beat every 625 ms: on every other beat its mark and name are drawn inverse, the
  text's background on a red fill, and so is the `⚠` its worktree's row and its space's row
  roll up, folded or not; the session shown in the selected window of a focused oso window,
  which is in front of the person, does not blink, and `[sessions] blink = false` keeps every
  row still (ADR 0140, "Waiting rows blink, as amended"). While the pane is selected, the row at point has an accent
  band and an accent bar in its first cell, which every row leaves free; while it is not, no band
  marks point ([ADR 0140](adr/0140-a-spaces-pane-with-sections-and-guides.md)). Its modeline
  says `Spaces`, the grouping, and `C-c a s to enter` (the keys of `sessions` as bound, ADR 0143)
  or `q back · TAB fold · ? keys`. The
  list is a tree ([ADR 0131](adr/0131-an-orca-style-sessions-pane.md), ADR 0140) under a
  `Spaces` title row with the most urgent states of oso's sessions: each space, a section
  row on a band, `▾ name ─── ⚠ 1`; in a space of several repositories, each repository,
  `▾ name/`; then its worktrees, the main checkout first (its branch and `primary`), then the
  newest made, as git lists them in a trusted project, each beginning with its fold, with its
  branch where it differs from its directory's name, and at the right its sessions' most
  urgent mark and its newest session's age; under each worktree its sessions on guides
  (`├`, `└`, `│`), the most urgent first, nested ones under the session that started them,
  each called by its title when its name is its worktree's. The worktree whose space is in
  front has its name in an accent colour and a faint band behind its rows. A worktree with no
  session of oso's running is folded, with its count; one with no session and no change for a
  week waits in a folded `N quiet` row. A worktree git
  makes outside oso shows within a second or two. Sessions run outside oso are listed under
  their worktree, grey; those in no project of the tree go in a folded `Other projects`, by
  repository. Archived sessions come last, folded. `s` shows the list grouped by state
  instead, pinned ones first, and back. The list is withheld from agents: while it is shown,
  the agent channel's commands answer in the echo area only.
  - `C-c a s` (`sessions`): shows and selects the list if hidden, selects it if shown, and
    goes back to the tree's window selected last if it is selected, as `q` in it does; with
    `C-u`, hides it. Hidden by the person, by `C-u C-c a s` or `C-x 0`, it stays hidden until
    asked for again. `C-x o` skips it, and so does `C-x 0` in another window; a click or
    `windmove-left` reaches it. While it is selected, `C-x 2`, `C-x 3`, `C-x 1`, `M-,`, `M-/`,
    `*Messages*` and showing a buffer act on the window selected before it, and select it.
  - The commands that show a session or the list, and pinning and archiving, need a person at
    the keyboard: the agent channel cannot run them.
  - In the list: `RET` shows the session, `n`/`p` move between rows, `M-n`/`M-p` between
    spaces, repositories, worktrees and groups (the rows with a fold), `TAB` folds, `S-TAB`
    folds to the spaces, then opens to the worktrees, then opens everything, `<right>` opens a
    folded row or goes into an open one, `<left>` folds an open row or goes to the row it is
    under, `q` goes back to the tree's window, `g`
    renders again with fresh ages and lists each project's worktrees again, `s` groups by
    state instead, `P` pins, `A` archives or
    brings back, `r` resumes, `k` hangs up (asking first while it works or needs input), `e`
    explains its state, `?` says what the marks mean and the keys, `/` is `session-switch`.
  - On a worktree's row, `RET` enters its space, or opens `C-x C-f`'s prompt in it when no
    space is there; on a project's or a group's row it folds; `k` says that `worktree-remove`
    removes worktrees.
  - `c` (`claude`) and `w` (`session-worktree`) take the row at point for where they are, as
    do `C-c a c` and `C-c a w` while the list is selected: a project's row is its main
    checkout and `HEAD`; a worktree's row is that worktree and its branch, or `HEAD` and a note
    when it is on none; a session's row is its worktree, and `w` makes it the new session's
    parent. `w` reads the branch of the row's repository, under the trust of a buffer in it.
  - A left click on the list's text moves point to the row under it; on the selected list it
    then does what `RET` does, and on a list not selected it selects it and nothing more; on a
    fold's mark it folds. Presses wait while a prompt or a key sequence holds input, as
    everywhere.
- `C-c a b` (`session-switch`) completes over sessions by buffer name; their state, space and
  recap are searched too, so `&needs` finds those that need input. `C-c a n` shows the session
  that has needed input longest, `C-c a d` the one done longest, `C-c a e` explains the shown
  session's state in `*Session explain*`.
- With `[sessions] agents_start_without_asking`, a session the person started starts sessions with
  no question in a trusted project, up to `agents_start_limit` each, or with no limit for `"none"`
  but the loop guard ([ADR 0135](adr/0135-agents-start-sessions-without-asking.md),
  [ADR 0144](adr/0144-agent-starts-with-no-limit-and-in-the-background.md)), and a space's
  orchestrator adds a trusted repository oso already knew to its space with no question and no
  limit ([ADR 0161](adr/0161-an-orchestrator-adds-a-repository-to-its-space.md)). A session an agent's
  tool starts, asked or not, starts in the background: the selected window keeps its buffer. Once its start is under way, the
  echo area says `Agent: session "repo" is starting a session in a new worktree … without asking
  (brief: 1 line; C-c a e explains)`, the row's place ends in `· unasked` (alone under its worktree in the tree), and
  `*Session explain*` starts with who started it and ends with its brief, or says that the brief
  is not kept across a restart. `agent-starts-ask` and `agent-starts-allow` turn the question back on and off
  until oso restarts.
- A space is a project with its sessions, and keeps its own windows. `C-c a j` (`space-switch`)
  switches to another; showing a session of another space switches to it first. A space
  entered for the first time shows its last file and its last session side by side; one
  `space-new` just made shows `*scratch*` alone until its orchestrator starts there
  ([ADR 0159](adr/0159-space-new-starts-the-orchestrator-and-display-names.md)).
  `C-c a z` (`space-zoom`) shows the selected window alone, and back. Under `oso dev`, a
  restart keeps only the windows in front: the other spaces' windows go, and a zoomed
  window stays alone.
- The selected window's modeline counts `⚠ N` sessions that need input and `● N` done or
  ready for review. A session's own modeline says its state and age, `needs input
  (permission) 3m`, before its title. A session counts as seen after 2 s in the selected window
  of a focused oso window: done becomes idle.
- Sessions of Claude Code run outside oso, from its registry, are listed too, as working or
  idle, their whole row grey and their marks still, aged from when the registry says their
  status last changed; they are never shown.
- `C-c a m` (`session-send`) pastes the region into a session read with completion, the
  session of this space shown last first. Each region goes as its path relative to the
  session's directory, its lines and its text in a fence. With no region it reads a one-line
  note and sends that. `C-u C-c a m` sends the place only, `path:10-20`, which the agent reads
  itself; with no region, each cursor's line, then the note if one is typed. A buffer that may
  hold secrets sends its place only. A region over 256 KiB is refused, and the echo area
  suggests `C-u`.
  - The paste is a bracketed paste and is not submitted: the session is shown and selected in
    its space, and the person reads the text in its input box and presses Enter.
  - A session that needs input (a permission dialog, a question, a plan) is refused until the
    person answers it, as a paste could choose an option. So are a session that has not
    started yet, one showing a full-screen view such as a picker or the settings, and one at
    a password prompt.
- `C-c a @` (`session-mention`) reads a live session's name, hosted or external, and gives it as
  `@NAME`: inserted at each cursor in an oso buffer such as `*Brief*`, or pasted into the session
  whose terminal is selected. An agent addresses that session by the name with Claude Code's
  own `SendMessage`.
- When a session receives a message from another, its `*Spaces*` row says "message from NAME"
  until the session is seen. `C-c a f` (`session-follow-message`), in that session's terminal,
  shows the sender. The sender is read from the screen, so it points to a session but does not
  prove who wrote.
