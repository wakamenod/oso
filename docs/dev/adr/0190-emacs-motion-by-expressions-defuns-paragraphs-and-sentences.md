# 0190. Emacs's motion by expressions, defuns, paragraphs and sentences

- Status: Accepted
- Date: 2026-11-29

## Context

The author asked on 2026-11-25 for every standard Emacs cursor-motion command, with none left
out: the commands of the Emacs manual's Moving Point, Words, Expressions, Defuns, Paragraphs,
Sentences, Pages, Marks, Scrolling and Transpose sections. oso had the motions by character,
word, line and buffer end ([ADR 0030](0030-m2-editing-commands.md),
[ADR 0040](0040-buffers-loading-motion-and-scrolling.md),
[ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md)), `M-m`, `C-v`, `M-v`,
`C-l`, `goto-line` and the mark commands of [ADR 0071](0071-mark-and-region.md). It had nothing
that knows brackets, strings or comments.

- Commands run on the UI thread and must not block on parsing. Syntax trees live only on the
  syntax worker ([ADR 0111](0111-tree-sitter-highlighting.md)). What the main thread has is a
  buffer's `Highlights`: runs over the whole buffer up to `syntax::MAX_BYTES` (2 MiB), each with
  what it is (`Comment`, `String`, ...), found in O(log n) and mapped through edits until the
  next report. `C-e` already reads comments from them.
- The indentation rules ([ADR 0115](0115-indentation.md), [ADR 0164](0164-indenting-the-web-languages-and-go.md))
  lex strings and comments themselves, but only forward from an anchor line.
- A motion that reads text has a budget ([ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md),
  [ADR 0101](0101-word-checks-by-offset-and-reading-shared-by-cursors.md)): 4 MiB a command, so
  that no key passes 100 ms.
- `C-M-n` and `C-M-p` move between diagnostics ([ADR 0138](0138-language-servers.md), Q15). In
  Emacs they are `forward-list` and `backward-list`. The author decided Emacs's meaning wins.

The author answered Q1 to Q4 below on 2026-11-29, through the space's orchestrator.

## Decision

### The commands and their keys

`*` marks what oso has; the rest is new. Each key is Emacs's default.

| Group | Keys and commands |
| --- | --- |
| Point | `*C-f C-b C-n C-p C-a C-e M-f M-b M-< M-> M-g g`, `*M-m`; `M-r` `move-to-window-line-top-bottom`; `M-g c` `goto-char`; `M-g TAB` `move-to-column`; `C-x C-n` `set-goal-column` |
| Aliases | `C-<left>`, `M-<left>` `backward-word`; `C-<right>`, `M-<right>` `forward-word`; `C-<home>`, `C-<end>` `beginning-of-buffer`, `end-of-buffer` |
| Expressions | `C-M-f`, `C-M-<right>` `forward-sexp`; `C-M-b`, `C-M-<left>` `backward-sexp`; `C-M-n` `forward-list`; `C-M-p` `backward-list`; `C-M-u`, `C-M-<up>` `backward-up-list`; `up-list` (no key); `C-M-d`, `C-M-<down>` `down-list`; `C-M-k` `kill-sexp`; `C-M-DEL` `backward-kill-sexp`; `C-M-@`, `C-M-SPC` `mark-sexp`; `C-M-t` `transpose-sexps` |
| Defuns | `C-M-a`, `C-M-<home>` `beginning-of-defun`; `C-M-e`, `C-M-<end>` `end-of-defun`; `C-M-h` `mark-defun`; `C-M-l` `reposition-window`; `M-g i` `imenu` |
| Paragraphs | `M-{`, `C-<up>` `backward-paragraph`; `M-}`, `C-<down>` `forward-paragraph`; `M-h` `mark-paragraph` |
| Sentences | `M-a` `backward-sentence`; `M-e` `forward-sentence`; `M-k` `kill-sentence`; `C-x DEL` `backward-kill-sentence` |
| Pages | `C-x [` `backward-page`; `C-x ]` `forward-page`; `C-x C-p` `mark-page` |
| Marks | `*C-SPC C-@ C-x C-x C-, C-x h`; `M-@` `mark-word`; `C-x C-SPC`, `C-x C-@` `pop-global-mark` |
| Scrolling | `*C-v M-v C-l`; `C-M-v`, `M-<next>` `scroll-other-window`; `C-M-S-v`, `M-<prior>` `scroll-other-window-down`; `M-<home>`, `M-<end>` `beginning-of-buffer-other-window`, `end-of-buffer-other-window`; `C-M-S-l` `recenter-other-window`; `C-x <` `scroll-left`; `C-x >` `scroll-right`; `scroll-up-line`, `scroll-down-line` (no keys) |
| Killing | `M-z` `zap-to-char`; `zap-up-to-char` (no key) |
| Transpose | `C-t` `transpose-chars`; `M-t` `transpose-words`; `C-x C-t` `transpose-lines` |

- **The diagnostics move to `M-g n` and `M-g p` (Q1),** with `M-g M-n` and `M-g M-p`: Emacs's
  `next-error` keys, beside `M-g d`'s list. `C-M-n` and `C-M-p` become `forward-list` and
  `backward-list`.
- **Left out:** `C-x l` `count-lines-page` (it counts and does not move), visual-line motion
  (`C-n` and `C-p` stay on logical lines in a wrapped buffer, as `toggle-truncate-lines` says), `narrow-to-defun` (oso has
  no narrowing), and `show-paren-mode`, which is a display, not a motion.
- **No clash with the author's `config.toml`:** no key above uses `s-`, the `M-s` prefix or
  `C-c r`. Inside isearch `M-r` stays `isearch-toggle-regexp`, as in Emacs, and the mode-local
  `M-n`, `M-p`, `M-v`, `M-<` and `M->` stay. `C-x SPC` stays `edit-lines`.
- **macOS may take some chords first:** `C-M-SPC` (Emoji & Symbols), `C-M-d` (Look Up) and the
  `C-` arrows (Mission Control and Spaces, on by default). oso has no menu
  ([ADR 0051](0051-m2-plan-decisions.md)), so `M-h`, `M-z` and `C-M-f` reach it. Each step checks
  its chords in the app; `C-M-@` is the key `mark-sexp` documents, and `M-{`, `M-}` the
  paragraph keys.

### Expressions: what a sexp is

- **A sexp,** read from point: space, punctuation and comments are skipped; then it is a string,
  a bracket group (`()`, `[]`, `{}`) with what it holds, or a run of symbol characters. A
  symbol character is a display cluster whose first character is a letter or a digit of any
  script, or `_`, with no break where the script changes; and in a mode's own way, `$` in
  JavaScript and TypeScript, `-` in CSS, HTML and YAML. `<` and `>` are not brackets but in HTML, where a tag `<…>` outside strings and
  comments is one, as Emacs's `sgml-mode` reads it. Motion by elements is left for later.
- **Strings and comments come from the buffer's highlights** where it has them: a `String` run
  is one sexp, from its first byte to its last, and a `Comment` run counts as space. A run is
  passed with one seek, whatever its length. Highlights lag an edit until the next report; until
  then the text an edit inserted takes the guess of [ADR 0111](0111-tree-sitter-highlighting.md).
  A template literal with `${}` is several runs, so a sexp from its backquote ends at the `${`:
  accepted.
- **With no highlights** (Swift, LaTeX, Fundamental, a buffer past 2 MiB, or before the first
  report) each line is lexed from its own start, with the mode's table: Swift's `"` strings and
  `//` comments, LaTeX's `%` comments; Fundamental has brackets only. A block comment or a string
  that spans lines is not seen there.
- **The worker's tree is not asked.** A motion would wait on another thread, which a key and a
  keyboard macro cannot, and the UI thread never blocks on parsing.
- **Errors are Emacs's words:** "Containing expression ends prematurely" when a closer comes
  first, "Unbalanced parentheses" when a bracket has no match or there is no list to go up out
  of, and "Don't have two things to transpose". A scan that would read more than its budget
  leaves that cursor where it was and says "Count too large": half a sexp is no place to stop.
- **Running out of sexps** goes to the buffer's edge with no error, as Emacs's `forward-sexp`
  and `forward-list` do.
- **Out of a string:** `backward-up-list` and `up-list` from inside a string go to its start or
  past its end first, as Emacs's interactive `escape-strings` does.

### Defuns (Q2)

- **First, by the line a defun starts on,** at any indentation, outside strings and comments:
  - Rust: `fn`, `struct`, `enum`, `union`, `trait`, `impl`, `mod`, `const`, `static`, `type`
    and `macro_rules!`, after any of `pub`, `pub(…)`, `async`, `unsafe`, `extern "…"` and
    `const`; not `use`, nor a `mod` that ends with `;`.
  - Go: `func`, `type`, `var`, `const`.
  - JavaScript and TypeScript: `function`, `class`, `interface`, `export`, `async function`, and
    `const x = (…) =>`.
  - Swift: `func`, `class`, `struct`, `enum`, `extension`, `protocol`.
  - CSS: a rule's line that ends with `{`. TOML: a `[table]`. YAML and JSON: a key at the top
    level. Markdown: a heading. LaTeX: `\section` and its kin, and `\begin{`.
  - Fundamental: `(` or `{` at column 0, as Emacs's `open-paren-in-column-0` does.
- **A defun ends** in the languages of braces after the bracket group from its line's first `{`,
  or at a `;` before any; elsewhere before the next defun of the same or a higher level (a
  Markdown section before the next heading of its level or above). `end-of-defun` goes to the
  start of the line after.
- **Then the worker's ranges (step 7).** With each highlights report the worker sends the ranges
  its grammar's `@definition.*` captures (the tags queries) give, mapped through edits as
  highlights are. Where a buffer has them they win, so a JavaScript class's methods and Rust's
  `impl` items are found; elsewhere the heads above stay. `imenu` lists these ranges.
- `C-M-h` takes in the comment just above the defun, as Emacs's `mark-defun` does.
  `reposition-window` puts the defun's first line (or its comment) at the window's top, unless
  point would leave the window, and then point's line at the bottom.

### Paragraphs, sentences and pages (Q3)

- **A paragraph** ends at a blank line (only spaces and tabs) or a form feed. In Markdown,
  headings and fence lines also stand apart.
- **A sentence ends** at `.`, `?`, `!` or `…`, after any closing `"`, `'`, `)`, `]`, `”`, `’`,
  `»`, then one space or a line's end (Q3: `sentence-end-double-space` off), and at `。`, `！`,
  `？` or `．` with no space after. A paragraph's end ends a sentence too.
- **A page** ends at a form feed.

### Counts, negative arguments and cursors

- **Every motion moves each cursor on its own** ([ADR 0007](0007-multi-cursor.md)),
  in order with the budget they share (`WalkBudget::for_cursors`), and cursors that meet merge.
  The first error shows after all have moved, as the motions by character do.
- **N repeats, and a negative N goes the other way.** Besides:
  - `down-list` with a negative N goes backward into the list before point, and stops before its
    closer.
  - `transpose-sexps`, `-words`, `-chars` and `-lines`: N drags what is before point past N more;
    0 swaps what ends at point with what ends at the mark.
  - `move-to-window-line-top-bottom` with no argument goes to the middle row, then the top, then
    the bottom on repeats; N is row N from the top, and a negative N counts from the bottom.
  - `goto-char` reads N, 1-based characters, from the prompt "Goto char: " or the prefix
    argument. `move-to-column` reads N display cells from "Move to column: " or the prefix
    argument, at each cursor's line, and stops at its end.
  - `scroll-other-window` with no argument scrolls a screen, as `C-v` does; N is lines.
  - `scroll-left` and `scroll-right` with no argument scroll the window's width less 2 columns.
- **One cursor is left,** from the primary, by `goto-char`, `move-to-window-line-top-bottom`,
  `pop-global-mark`, `mark-defun`, `reposition-window` and `imenu`, as `goto-line` does. The
  others work at every cursor.
- **`set-goal-column`** keeps the primary's display column for its buffer, as Emacs's
  buffer-local `goal-column`; `C-n` and `C-p` then go to it. `C-u C-x C-n` clears it. It says
  "Goal column N (use C-u C-x C-n to unset it)". A restart drops it.
- **Scrolling another window** scrolls the next one in `C-x o`'s order, and moves its cursors
  into view, as `C-v` does. With one window it says "No other window to scroll".
  `scroll-left` and `scroll-right` do the same by columns, and nothing while the buffer wraps.

### The region and the marks ([ADR 0071](0071-mark-and-region.md))

- **The new motions keep an active region and grow it, and select with Shift,** as Emacs's
  `^p` commands do.
- **`beginning-of-defun` and `end-of-defun` set the mark** where the cursors were, with "Mark
  set", when no region shows and the command does not repeat. No other motion does.
- **The mark commands** (`mark-word`, `mark-sexp`, `mark-paragraph`, `mark-defun`, `mark-page`)
  set every cursor's mark, put the old one in the ring and show the region. Repeated, or while
  a region shows, they move the mark N more objects on. `mark-paragraph` and `mark-defun` move
  point to the start first.
- **The global mark ring,** as Emacs's `global-mark-ring`: the editor keeps 16 places, a buffer
  and an offset each. A mark pushed in a buffer other than the newest place's adds a place. The
  places follow edits as a window's jumps do (`window::Jumps`), a killed buffer's places go,
  `Debug` shows only how many there are, and `get-state` and `export-state` never show a place
  in a buffer that may hold secrets ([ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md)).
  `pop-global-mark` shows the newest place in the selected window and moves it to the ring's
  end.
- **The kills** (`kill-sexp`, `backward-kill-sexp`, `kill-sentence`, `backward-kill-sentence`,
  `zap-to-char`, `zap-up-to-char`) kill at every cursor, and a kill right after a kill appends,
  as `kill-word` does.

### What the agent channel may run ([ADR 0085](0085-agent-channel-and-oso-mcp.md))

- The motions, the mark commands, the scrolls, `set-goal-column` and `imenu` are `Look`.
- The kills and transposes are `Change`. `zap-to-char` and `zap-up-to-char` read their character
  as `ArgSpec::CountAndChar`, which the channel passes as text.

### Where `sexp` sits ([ADR 0038](0038-architecture-guardrails.md))

- `sexp` is a new top-level module of `oso-core`, on the layer of `window` and `word` in
  `guardrails/layers.txt`: it may use `buffer`, `motion`, `selection`, `display_map`, `syntax`
  and `rope`, and `editor`, `frame` and the tip crate may use it. It reads a display snapshot and
  a buffer's highlights, and does no I/O.
- Paragraphs, sentences and pages are private modules of `motion`, which needs no new layer.

### Public items

New public items, as the steps add them:

- **`oso_core::sexp`, a new module:**
  - `pub enum Table { Fundamental, Rust, Go, Script, Css, Json, Html, Markdown, Toml, Yaml, Latex, Swift }`
  - `pub struct Syntax<'a>`, with
    `pub fn new(display: &'a DisplaySnapshot, highlights: Option<&'a Highlights>, table: Table) -> Self`
  - `pub enum ScanError { EndsPrematurely, Unbalanced, NothingToTranspose, TooFar }`, with
    `pub fn message(self) -> &'static str`
  - `pub fn forward_sexp(syntax: &Syntax<'_>, selection: &mut Selection, n: isize, budget: &mut WalkBudget) -> Result<(), ScanError>`,
    and `forward_list`, `up_list`, `down_list` with the same signature
  - `pub fn sexp_end(syntax: &Syntax<'_>, offset: usize, n: isize, budget: &mut WalkBudget) -> Result<usize, ScanError>`
  - `pub fn transpose_ranges(syntax: &Syntax<'_>, offset: usize, n: isize, budget: &mut WalkBudget) -> Result<(Range<usize>, Range<usize>), ScanError>`
  - `pub fn beginning_of_defun(syntax: &Syntax<'_>, selection: &mut Selection, n: isize, budget: &mut WalkBudget) -> Result<(), Edge>`,
    and `end_of_defun` with the same signature
  - `pub fn defun_bounds(syntax: &Syntax<'_>, offset: usize, budget: &mut WalkBudget) -> Option<Range<usize>>`
- **`sexp::Table::of(mode: Mode) -> Table`** chooses a buffer's table. It sits in `sexp`, as
  `buffer` cannot use `sexp`: `sexp` uses `motion`, which shares `buffer`'s layer.
- **`oso_core::motion`**, from private modules `paragraph`, `sentence` and `page`, each
  `(display: &DisplaySnapshot, selection: &mut Selection, n: isize, budget: &mut WalkBudget) -> Result<(), Edge>`:
  `pub fn forward_paragraph` (with `markdown: bool` after `display`), `pub fn forward_sentence`,
  `pub fn forward_page`; and
  `pub fn paragraph_bounds(display: &DisplaySnapshot, offset: usize, markdown: bool, budget: &mut WalkBudget) -> Range<usize>`.
- **`oso_core::rope::Rope`:** `pub fn char_to_offset(&self, chars: usize) -> usize`,
  `pub fn offset_to_char(&self, offset: usize) -> usize`, and
  `pub fn reversed_chunks_in_range(&self, range: Range<usize>) -> RevChunks<'_>`.
- **`oso_core::command`:** `ArgSpec::CountAndChar { prompt: &'static str }`, read with
  `pub fn char_answer(&self) -> Option<char>` on `Ctx`.
- **The global mark ring:** `pub struct GlobalMarks` in `editor`, read with
  `Editor::global_marks(&self) -> &GlobalMarks`; it is changed only inside `oso-core`.
- **The goal column:** `Buffer::goal_column(&self) -> Option<usize>` and
  `Buffer::set_goal_column(&mut self, cells: Option<usize>)`.
- **Step 7:** `oso_core::syntax::Defuns`, sorted ranges mapped through `Splice` as `Highlights`
  are, with `pub fn new() -> Self`, `pub fn around(&self, offset: usize) -> Option<Range<usize>>`,
  `pub fn before(&self, offset: usize) -> Option<Range<usize>>`,
  `pub fn after(&self, offset: usize) -> Option<Range<usize>>` and
  `pub fn splice(&mut self, splices: &[Splice])`; the worker's report carries them.

A change to one of these names or signatures goes back to the author first.

### Steps

Each is its own pull request, with its tests first. Step 1 lands first; steps 2 to 6 and 8 can be
built at once after it; step 7 follows step 3, and step 9 follows step 7. New commands go in new
files where they can (`commands/sexp.rs`, `commands/text_motion.rs`, and their `bindings/`), so
that the steps touch few lines in common. Each step updates `docs/dev/ui.md` for its keys.

1. **The scanner (`P1-core`).** `oso_core::sexp` with `Table`, `Syntax`, `ScanError`,
   `forward_sexp`, `forward_list`, `up_list`, `down_list`, `sexp_end` and `transpose_ranges`;
   `Table::of`; `Rope::reversed_chunks_in_range`; `sexp` in `guardrails/layers.txt`. No command
   and no key. Tests: a table of
   cases for each mode, with `|` for point (strings holding brackets, comments, raw and template
   strings, a Japanese run, an HTML tag, a combining mark after `)`, with highlights and without,
   highlights after a splice); property tests that forward then backward over balanced text comes
   back, that `up_list` after `down_list` does, that nothing panics and every stop is a cluster's
   start; crafted costs (1 MiB of `(`, 1 MiB of `""`, 2 MiB with no closer) within the budget.
2. **The expression commands.** `forward-sexp`, `backward-sexp`, `forward-list`,
   `backward-list`, `backward-up-list`, `up-list`, `down-list`, `kill-sexp`,
   `backward-kill-sexp`, `mark-sexp` and `transpose-sexps`, with `C-M-f/b/n/p/u/d/k/DEL/@/SPC/t`
   and the `C-M-` arrows; the diagnostics move to `M-g n`, `M-g p`, `M-g M-n`, `M-g M-p`, and ADR
   0138's key table follows. Tests: several cursors, N, negative N, Shift, a repeated
   `mark-sexp`, kills that append, the errors. Checked in the app: `C-M-SPC` and `C-M-d`.
3. **Defuns by their lines.** `beginning-of-defun`, `end-of-defun`, `defun_bounds`,
   `mark-defun` and `reposition-window`, with `C-M-a`, `C-M-e`, `C-M-<home>`, `C-M-<end>`,
   `C-M-h`, `C-M-l`; the heads of each mode above. Tests: each mode's heads, nested items, "Mark
   set" only when no region shows and the command does not repeat, the comment `C-M-h` takes in.
4. **Paragraphs, sentences and pages.** `forward-paragraph`, `backward-paragraph`,
   `mark-paragraph`, `forward-sentence`, `backward-sentence`, `kill-sentence`,
   `backward-kill-sentence`, `forward-page`, `backward-page`, `mark-page`, with `M-{`, `M-}`,
   `C-<up>`, `C-<down>`, `M-h`, `M-a`, `M-e`, `M-k`, `C-x DEL`, `C-x [`, `C-x ]`, `C-x C-p`.
   Tests: blank lines with spaces, Markdown headings and fences, English and Japanese sentence
   ends, one space, a sentence cut by a paragraph's end, a form feed.
5. **Other windows and the screen.** `move-to-window-line-top-bottom`, `scroll-other-window`,
   `scroll-other-window-down`, `beginning-of-buffer-other-window`,
   `end-of-buffer-other-window`, `recenter-other-window`, `scroll-left`, `scroll-right`,
   `scroll-up-line`, `scroll-down-line`, with `M-r`, `C-M-v`, `C-M-S-v`, `M-<next>`, `M-<prior>`,
   `M-<home>`, `M-<end>`, `C-M-S-l`, `C-x <`, `C-x >`. Tests: the `M-r` cycle, one window, the
   other window's cursors kept in view, a wrapping buffer.
6. **Point by number, and the goal column.** `goto-char`, `move-to-column`, `set-goal-column`,
   with `M-g c`, `M-g TAB`, `C-x C-n`; the aliases `C-<left>`, `C-<right>`, `M-<left>`,
   `M-<right>`, `C-<home>`, `C-<end>`; `Rope::char_to_offset`, `Rope::offset_to_char`,
   `Buffer::goal_column`. Tests: characters of several bytes, a column inside a tab or a wide
   character, a goal column kept by `C-n` and cleared by `C-u C-x C-n`.
7. **Defuns from the worker, and `imenu`.** `syntax::Defuns` from each grammar's tags query on
   the worker, carried with the highlights and mapped through edits; step 3's commands read them
   first; `imenu` on `M-g i` lists them with a preview, as `consult-line` does. Tests: JavaScript
   class methods, Rust `impl` items, ranges after an edit, crafted costs on the worker.
8. **Marks and zapping.** `mark-word` on `M-@`; `GlobalMarks` and `pop-global-mark` on
   `C-x C-SPC` and `C-x C-@`; `zap-to-char` on `M-z` and `zap-up-to-char`, with
   `ArgSpec::CountAndChar`. Tests: the ring follows edits, loses a killed buffer's places, shows
   counts only and no secret buffer's place; a repeated `M-@`; zapping several cursors, a
   negative N, a character not found.
9. **Transposes.** `transpose-chars`, `transpose-words` and `transpose-lines` on `C-t`, `M-t`
   and `C-x C-t`, as Emacs does at a line's end and at the buffer's edges. Tests: clusters,
   Japanese words, several cursors, N and 0.

## Step 1, as built

Built on 2026-11-29; the author approved the changes through the space's orchestrator.

- **No `Rope::reversed_chunks_in_range`.** The scanner reads a display snapshot, not a rope, so
  it reads backward through crate-private helpers, `DisplaySnapshot::text_before` and
  `Highlights::runs_before`. No new public item does it.
- **`Syntax`'s `Debug` is written by hand** and shows its table and whether it has highlights,
  as a display snapshot has no `Debug`.
- **A highlighted string or comment costs what it holds, at most `SEEK_COST`.** A full budget
  of 4 MiB reads in 16 to 21 ms in a release build, forward or backward, with highlights,
  lexed lines or neither.
- **A line longer than 64 KiB is all code** where lines are lexed, so a lexed line has a
  bounded cost.
- **`up_list` leaves a comment as it leaves a string,** as its first level.

## Step 2, as built

- **The commands are in `commands/sexp.rs` and their keys in `bindings/sexp.rs`;** no new public
  item. `kill-sexp` and `backward-kill-sexp` kill through `kill.rs`'s helper, so a kill after a
  kill appends.
- **`mark-sexp`:** a cursor whose scan fails keeps its mark; if every cursor's fails, nothing
  changes, as in Emacs, where the error comes before `push-mark`. Extending with no argument
  moves the mark away from point, as Emacs's `(if (< (mark) (point)) -1 1)`.
- **`transpose-sexps`:** every scan error but the budget's says "Don't have two things to
  transpose", as Emacs's interactive `transpose-sexps` gives one message for any scan error. With
  0, point and the mark each go with their expression, as Emacs's `exchange-point-and-mark`
  after the swap leaves them. A cursor whose swap overlaps an earlier cursor's swaps nothing, so
  that no text is swapped twice. The text copied across cursors is at most `MAX_INSERT`
  (16 MiB), past which it says "Too much text to transpose" and changes nothing.
- **`C-M-p` in a terminal** stays `terminal-float-cycle-position` while the floating terminal
  shows ([ADR 0120](0120-the-terminal.md)): that layer is above the global keymap.

## Step 4, as built

Built on 2026-11-29.

- **Emacs's stops.** `forward-paragraph` stops at the first separating line after the paragraph,
  `backward-paragraph` at the last one before it, and from a paragraph's first line at the one
  just above. `forward-sentence` stops after the closers, `backward-sentence` after the space.
  `forward-page` from a line's start passes a form feed there, as Emacs's does.
- **No message at the buffer's edge,** as Emacs's commands give none. The functions still fail
  with `Edge` when a step starts there.
- **A step past the budget does not move:** the cursor stays where the step before left it, and
  the command says "Count too large", as a sexp does. Lines and sentences are read a character
  at a time at the cost of their bytes, with no seek per line: a paragraph motion crosses about
  4 MiB, and a sentence motion, which reads its paragraph to find its end and then searches it,
  a paragraph of about 2 MiB. A kill whose step ran out kills nothing at that cursor.
- **Sentences read paragraphs as Fundamental's,** as `forward_sentence` has no Markdown flag: a
  heading with no blank line after it starts its paragraph's first sentence.
- **Fences are not followed across lines:** a line that starts with `#` inside a fenced block
  counts as a heading.
- **`mark-page` is Emacs's:** with N it marks the page N pages away, and point goes to the page's
  start. Again, or while a region shows, it moves the mark N pages on, one with no N.

## Step 8, as built

Built on 2026-11-29.

- **A push reaches the global ring through the selection set.** Marks are pushed at many places
  through `SelectionSet::push_marks` and `push_primary_mark`; each now notes that it pushed, in a
  crate-private flag that equality leaves out. When a command ends, the editor takes it from
  each tiled window's set and from the place the window last left, the selected window last, so
  a push just before a switch to another buffer counts too. A place is the primary cursor's mark.
- **`pop-global-mark` pushes no mark and keeps the region as it was,** as Emacs's `goto-char`
  does. The agent is refused a place in a buffer it may not be shown, by the channel's one check
  (`unshowable`) before anything moves, a buffer with keys of its own among them. The
  ring is not carried by a restart, as a window's jumps are not; `get-state` lists its places in
  `global_marks`, leaving out those in a buffer that may hold secrets, and the field while one
  is shown.
- **`ArgSpec::CountAndChar` reads in a text prompt with no history, which the dispatcher ends
  as soon as typed text puts a character in it,** as `read-char-from-minibuffer` ends. No new
  `minibuffer::Kind`. `RET` on it empty runs nothing. A key typed there does not break a run of
  kills, so `M-z` right after a kill appends to it.
- **A zap matches characters as Emacs does, with `case-fold-search` on,** and ends what it kills
  at a display cluster's edge. It reads at most 4 MiB across its cursors, its own bound in the
  tip crate, as `kill-line`'s blanks are; a backward zap reads 64 KiB at a time. A character
  yanked into the prompt from a secret is refused, as query-replace refuses such a string
  (ADR 0113): where the kill ends would tell it, and the kill would hold it unmarked.
- **New public items beyond the list above:** `Editor::pop_global_mark`, and `GlobalMarks::len`,
  `is_empty` and `iter`, which `get-state` reads.

## Changes to accepted ADRs

- [ADR 0138](0138-language-servers.md), "Keys (Q15)" and the key table: `lsp-next-diagnostic`
  and `lsp-previous-diagnostic` move from `C-M-n` and `C-M-p` to `M-g n` and `M-g p` (and
  `M-g M-n`, `M-g M-p`).

## Consequences

- oso has every motion of Emacs's manual sections named above but those left out, with Emacs's
  keys.
- Expressions are as good as the highlights: right in the highlighted languages once a report
  has come, line by line elsewhere. Strings and comments past 2 MiB are not known.
- Defuns by lines miss what has no keyword (a JavaScript method) until step 7.
- The author's fingers for the diagnostics change from `C-M-n` and `C-M-p` to `M-g n` and
  `M-g p`.
- Some `C-M-` chords and the `C-` arrows may not reach oso on macOS while the system keeps them.
- A benchmark of `C-M-f` across a 2 MiB buffer would change the protected perf bench, so it is
  its own pull request, asked for first.

## Alternatives considered

- **Asking the worker's tree for each sexp:** exact, but the command would wait on another
  thread, and a keyboard macro could not replay it.
- **The indentation lexers for strings and comments:** they read only forward from an anchor,
  so a backward motion would read from the anchor before the place it seeks, which it does not
  know.
- **Brackets only, everywhere:** a bracket in a string or comment would break the match in
  every language.
- **Defuns at column 0 only, as Emacs's fallback:** misses every method inside an `impl` or a
  class.
- **Keeping the diagnostics on `C-M-n` and `C-M-p`:** the author chose Emacs's meaning. `M-n` and
  `M-p` (Flymake's suggestion) and `C-c ! n` and `C-c ! p` (Flycheck's) were offered and not
  taken.
- **Two spaces after a sentence, Emacs's default:** most text oso edits, this repository's
  among it, puts one.
