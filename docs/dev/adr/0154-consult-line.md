# 0154. consult-line

- Status: Accepted
- Date: 2026-11-18

## Context

The author's Emacs binds `M-s l` to consult's `consult-line`: a prompt lists the current buffer's
lines that match what is typed, with orderless matching, the selected line previews, `RET` goes
there and `C-g` returns. In their search menu it is the `l` head. oso had no such command:

- [ADR 0117](0117-menus.md) built the `search` menu with the heads oso had, project search and
  finding a project's file, and said the rest "join as their commands come".
- [ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md) searches a project's
  files in a prompt whose rows stream in from the `oso-grep` threads, matched by
  `LineQuery` (orderless components, `=`, `!`, smart case), with a preview that splits the float.
  [ADR 0116](0116-migemo-and-jumps.md) made its romaji components migemo's.
  [ADR 0145](0145-searching-past-agent-conversations.md) reused that prompt for conversations.
- `consult-mark` (`C-.`, [ADR 0088](0088-switching-and-killing-buffers.md)) lists a buffer's
  marks through completion, with a preview of the buffer from memory, never of one that may hold
  secrets.
- [ADR 0120](0120-the-terminal.md) lists "a line search with a list, as consult-line, for buffers
  and terminals" as a follow-up.

## Decision

The author answered Q1 to Q12 below on 2026-11-18, through the space's orchestrator: (a) for
each.

### The command and its keys (Q1, Q11)

- **`consult-line`**, `Person`, as `consult-mark` and `consult-yank-pop` are named. It reads its
  argument as `ArgSpec::BufferLines { history: "consult-line" }`. The agent channel never runs it:
  its rows are the buffer's text.
- **`M-s l`** by default, beside `M-s r`, and the built-in `search` menu gains a `Buffer` group
  before `Project` with `l` (`line`). The author's `config.toml` binds `M-s` to that menu, which
  hides the default `M-s l` there; `l` reaches it through the menu.
- It refuses inside a prompt ("Not in the minibuffer"), and in a terminal's buffer, whose lines
  are its screen's: "A terminal's lines are searched in copy mode, C-c C-j" (Q12).

### The search (Q2, Q5, Q8)

- **It is a third source of the project search's prompt**, beside a project's files and
  conversations: its input, its streaming rows, their highlights, the count and the float are
  ADR 0114's. Only what is searched, the rows' order and labels, the preview and `RET` are its
  own.
- **The input is read as a project's search reads it (Q2):** orderless components in any order,
  `=` keeps one literal, `!` leaves lines out, an expression where one reads, smart case, and a
  romaji component as migemo's pattern, unless `[search] migemo = false`.
- **Every input searches (Q5)**, one character too. The empty input, or one of `!` components
  only, lists every line that is not empty (and not excluded).
- **Each search reads a snapshot of the buffer's text** (a rope's clone, O(1)) on a thread of its
  own, `oso-lines`, so the UI thread copies and matches nothing of the text (Q8). The text is
  read in blocks of 256 KiB cut at a line's end and matched with `LineQuery::find_in_block`, as a
  file's are; a line longer than 16 MiB is matched in pieces of that size and shows as one row at
  most, its first piece that matches. Each piece is matched alone, so `!` leaves such a line out
  only from the pieces that hold it, and `^`, `$` are a piece's ends: accepted, as such a line is
  crafted or a machine's. A newer input moves the generation, which the search reads at each
  block, and a narrowed search every 1024 lines or 256 KiB read.
- **At most 10,000 rows**, past which the count says `10000+`.
- **The first row goes as soon as it is found**, so the selected row shows at once; **the rest go
  at most every 30 ms**, looked at between blocks too, and with the end. A search that ends at
  once sends two batches, the second with up to 9,999 rows: the editor takes each batch in one
  append, whose cost grows with the whole list, so fewer and larger batches cost it less than
  `M-s r`'s batches of 1000.
- **An input that narrows the one before** (ADR 0114's rule), whose search ended without a limit
  on the same revision of the buffer, looks only through the lines that search found, in a text
  of 8 MiB or more, a line longer than 16 MiB in the same pieces as a whole search. Below that,
  finding each of those lines costs more than looking through the whole text again (5 ms
  against under 1 ms for 5000 lines, measured).

### The list (Q3, Q4)

- **A row is the line's number, padded to the widest the buffer has, `: `, and the line's text**
  around its first match, at most 512 bytes, as a project's search shows one (Q3). The number is
  in the annotation's face and the matches in the match face.
- **The rows go from the cursor's line to the end, then from the first line to the one before
  the cursor's (Q4)**, as consult does with `consult-line-start-from-top` off. The first row, the
  nearest match at or after the cursor, is selected, and rows never move once shown, so the row
  selected does not jump as more come, and the cap keeps the lines after the cursor.

### The preview, `RET` and `C-g` (Q6, Q7)

- **The preview splits the float as `C-.`'s does (Q7)** and shows the buffer from memory, the
  row's match with a hollow cursor and the query's matches in view highlighted, as a project's
  search's preview does. No file is read.
- **`RET` (Q6)** leaves one cursor at the row's match, at a character's start, with the region
  hidden, and sets the mark where the cursor was, which puts the old mark in the ring, as `C-.`
  does. A match an expression starts inside a character, as `(?-u:\xA9)` in `é`, is at that
  character's start, in the preview too. Should the buffer have changed since the search, as a
  reload or the agent can change it, it goes to the start of the line of the row's number; once
  the buffer has gone, `RET` says "No line was picked" and the history keeps nothing of the row.
- **`C-g`** closes the float. The window was never moved, so the cursor stays.

### A buffer that may hold secrets (Q9) and the history (Q10)

- **Its rows show**, as the person sees that buffer already. **The preview never does**: it says
  `may hold secrets`, as every preview (ADR 0088). `get-state` shows nothing of the prompt, its
  label, input and rows, as for every prompt beside such a buffer (ADR 0072, ADR 0076), and
  migemo keeps nothing of its input in its cache.
- **The history is `consult-line`'s own (Q10)**, of what was typed. A search of a buffer that may
  hold secrets, or beside one, records nothing.
- Nothing of the input, the text or the rows reaches a log: `Debug` gives a generation at most.

### Code

- `oso-core`: `editor/project_search/lines.rs`, a private module, holds the source. The prompt's
  state names its source as one value, `Source::{Files, Conversations, Lines}`, which each step
  matches on. Public: `ArgSpec::BufferLines`; `Editor::open_line_search`,
  `take_line_search_request`, `lines_found` and `go_to_line_hit`; `LineSearchRequest` (its
  `generation` and `run`), `LineRow` (opaque, carried from `run` to `lines_found`) and
  `LinesEnd`, which `run` gives: `Stopped`, or `Ended` with whether it was cut, kept apart from
  `lines_found`'s `Option<WalkEnd>`, where `None` means more to come. `LineQuery` gains
  crate-private `lists_every_line`, `matches_listed` and `is_too_long`, and `project` a
  crate-private `row_shown`, a row's text without the column, which costs time linear in the
  line. `go_to_listed_mark` and `go_to_line_hit` share `go_to_keeping_mark`.
- `oso`: `grep/lines.rs`, the `oso-lines` thread (`LineSearcher`, `FoundLines`), which `Grep`
  holds beside `oso-grep`; glue in `app/greps.rs`, the event `UserEvent::LinesFound`,
  `commands/lines.rs`, `bindings/lines.rs`, and the dispatcher's `read_buffer_lines`.
- No crate, crate edge, public trait or layer rule changes.

### Costs measured

Release build, Apple M-series, `lines_costs` and `lines_stop_costs` (ignored by default).

- **The UI thread:** opening the prompt 0.01 to 0.3 ms; a key under 0.8 ms; taking in a whole
  search's rows, the first batch and the rest, 0.45 ms at most for 10,000 rows; the preview of a
  frame under 1 µs.
- **`oso-lines`, a million lines of 70 MiB:** every line 1 ms (cut at 10,000); a literal
  matching once 42 ms; one matching every line 1 ms (cut); no match 27 ms; an expression 46 ms;
  romaji 27 ms; an exclusion alone 46 ms. An input that narrows one with 11 rows: under 0.1 ms.
- **At 8 MiB, where narrowing starts:** narrowing and the whole search cost about the same,
  3.8 ms against 4.2 ms from 5,000 rows and 5.6 ms against 3.9 ms from 9,300.
- **Crafted:** one 64 MiB line, 38 ms with no input and 3 ms for a literal; 200 lines of 1 MiB,
  110 ms with no input and 31 ms narrowed; a million empty lines 2.4 ms.
- **Stopping once superseded:** 6.5 ms within one 64 MiB line (a 16 MiB piece), 0.2 ms in a
  narrowed search of 1 MiB lines.

## Questions

Each was answered (a), the recommendation.

1. **The name:** (a) `consult-line`, as `consult-mark`. (b) `search-line`.
2. **The matching:** (a) a project's search's, migemo by isearch's rule. (b) completion's, migemo
   only with `%`. (c) no migemo.
3. **A row:** (a) the line number and the text, with the match highlighted. (b) no line number.
4. **The order and the selection:** (a) from the cursor's line round to it, the first row
   selected. (b) the buffer's order, the first match at or after the cursor selected, which jumps
   as rows come and which the cap can leave out.
5. **No input:** (a) every line. (b) nothing until two characters, as a project's search.
6. **`RET`:** (a) at the match, the place before on the mark ring. (b) at the line's start.
7. **The preview:** (a) the float's right half, as `C-.`. (b) the window itself, as Emacs, put
   back by `C-g`.
8. **Large buffers:** (a) always on a thread, with generations, narrowing and the cap. (b)
   synchronously up to 1 MiB.
9. **A buffer that may hold secrets:** (a) as `C-.`: rows shown, no preview, nothing to
   `get-state` or the history. (b) refuse. (c) rows without text, as a project's search.
10. **The history:** (a) its own. (b) isearch's.
11. **Keys:** (a) the menu's `l` and `M-s l`. (b) the menu only.
12. **Terminals:** (a) buffers only now; a live screen's later. (b) now.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0117](0117-menus.md), "**`search`** (the author's search menu's heads oso has today:
  project search and finding a project's file; the rest join as their commands come)": and
  `consult-line` on `l`, in a `Buffer` group before `Project`.
- [ADR 0120](0120-the-terminal.md), the terminal's search, "A list of matching lines with a
  preview, as consult-line, comes when oso has one for buffers": oso has one for buffers now; a
  terminal's frozen screen gets it through copy mode, and the live screen's waits.
- ADR 0120, Consequences, the follow-up "a line search with a list, as consult-line, for buffers
  and terminals": buffers have it now, a terminal through copy mode; the live screen's stays a
  follow-up.

## Consequences

- `M-s l` and the menu's `l` search the buffer as the author's Emacs does, with the same prompt,
  rows and preview as `M-s r`.
- The project search's prompt state now has three sources, one `Source` value. A new one adds a
  variant to it, to the request and to the rows' place, an arm to each match on them, and its
  own module, as this did.
- A frozen terminal, through copy mode, is searched as any buffer; the live screen is not.
- Follow-ups: the live terminal screen's line search (ADR 0120); `[migemo]` in the label, which
  neither search shows while isearch does.

## Alternatives considered

- **Completion over every line, as `C-.` lists marks:** each open would copy every line into a
  candidate on the UI thread, over 8 ms past a few MiB, and migemo would need `%`.
- **A prompt of its own, apart from the project search's:** it would duplicate the streaming
  rows, the highlights, the count and the preview's cells.
- **The `oso-grep` thread:** it would wait behind a project's walk, and its batches carry files'
  rows; a thread of its own keeps both simple.
- **Previewing in the window, as Emacs:** the float covers most of the window, and putting the
  view back on `C-g` would be new state for every way a prompt closes.
