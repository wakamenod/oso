# 0115. Indentation

- Status: Accepted
- Date: 2026-11-12

## Context

The roadmap puts indentation in M3 ([roadmap](../roadmap.md)). Earlier ADRs set rules that apply to it,
or left it to M3:

- `TAB` runs `tab-to-tab-stop`: spaces up to the next stop, every `tab-width` cells (8), at each cursor,
  counting the display column of each one ([ADR 0051](0051-m2-plan-decisions.md),
  [ADR 0056](0056-typing.md), [ADR 0101](0101-word-checks-by-offset-and-reading-shared-by-cursors.md)).
  ADR 0056 did not name it `indent-for-tab-command`, "which comes with modes in M3".
- "Indentation never inserts tabs" is one of the author's Emacs habits
  ([ADR 0030](0030-m2-editing-commands.md)); the author's Emacs sets `indent-tabs-mode` to nil.
- Inside repositories, oso reads only EditorConfig, "indentation, line endings and similar"
  ([ADR 0019](0019-configuration.md)). Nothing reads it yet. ADR 0113 left "whether to read
  `.editorconfig`" open.
- `RET` runs `newline`, which inserts the file's line break at every cursor, an undo step of its own
  (ADR 0056). `C-j` is unbound. `C-a` toggles between the line's start and its first non-blank
  (`mwim-beginning-of-code-or-line`, ADR 0030).
- With a region shown, `TAB` does not replace it, "as in Emacs, where it indents"
  ([ADR 0074](0074-the-kill-ring-and-the-clipboard.md)). A yank inserts its text as it is.
- In prompts, `TAB` is `insert-completion`, on the prompt's own keymap layer
  ([ADR 0076](0076-completion-and-m-x.md)). Nothing here changes it.
- Major modes are `Rust`, `Markdown`, `Toml` and `Fundamental`, from the file's name only
  (`Mode::for_path`), and `rust-mode` and the others set one by hand
  ([ADR 0111](0111-tree-sitter-highlighting.md), step 4b).
- **Tree-sitter runs only on two worker threads.** The UI thread never calls it. Buffers that may hold
  secrets, internal buffers and files over 2 MiB are never parsed, and with `global-font-lock-mode`
  off nothing is. A buffer's `Highlights` follow each edit with a guess until the worker's next report
  (ADR 0111). ADR 0111 said indentation "needs the tree, and right after an edit, as `RET` indents a
  line the worker has not parsed yet. It gets its own ADR."
- Whitespace cleanup on save trims the lines edited since the file was read, when
  `[save] trim-whitespace` is on ([ADR 0113](0113-query-replace-keyboard-macros-and-whitespace-cleanup.md)).
- Soft wrap starts a continuation row at the left edge ([ADR 0100](0100-soft-wrap.md)). Indentation
  does not change that.
- Commands act on the selection set, at most 1000 cursors, in one batched edit
  ([ADR 0091](0091-multiple-cursor-keys-and-m-y.md), [ADR 0092](0092-edits-in-place-for-many-cursors.md)).
  At most 16 MiB is inserted by one command (ADR 0056). Keyboard macros replay keys and must do the
  same thing each time (ADR 0113).
- The configuration file reserves per-mode tables such as `[modes.markdown]`
  ([ADR 0108](0108-the-user-configuration-file.md), [ADR 0110](0110-adr-0108-as-built.md)).

A buffer can be 1 GB, and the UI thread must never wait on parsing. So `RET` cannot wait for the
worker's tree, and no command may scan a whole buffer on the UI thread.

### What other editors do

- **Emacs.** `TAB` is `indent-for-tab-command`: with `tab-always-indent` at its default `t` it
  indents the current line, wherever the cursor is; `complete` also completes, and nil inserts a tab
  outside the indentation. With a region active it indents the region. Electric Indent mode, on by
  default since 24.4, indents the line after every `RET`; `C-j` then inserts a newline without
  indenting. `C-M-\` is `indent-region`, `C-x TAB` is `indent-rigidly` (with no count, a transient
  mode where `<left>`/`<right>` move by one column and `S-<left>`/`S-<right>` by a tab stop), `M-m` is
  `back-to-indentation`, `M-i` is `tab-to-tab-stop` and `M-^` is `delete-indentation`. Modes indent
  by their own function: classic `rust-mode` by bracket depth and special cases, `rust-ts-mode` by
  `treesit-simple-indent-rules` over the tree, which Emacs parses on the main thread.
  Sources: [Indentation Commands](https://www.gnu.org/software/emacs/manual/html_node/emacs/Indentation-Commands.html),
  [Indent Convenience](https://www.gnu.org/software/emacs/manual/html_node/emacs/Indent-Convenience.html),
  [Indentation](https://www.gnu.org/software/emacs/manual/html_node/emacs/Indentation.html).
- **Zed.** `auto_indent` is `syntax_aware` (default, from `indents.scm` captures `@indent`, `@start`,
  `@end`, `@outdent`, plus `increase_indent_pattern`/`decrease_indent_pattern` regexps),
  `preserve_indent` or `none`; `auto_indent_on_paste` is on by default. The buffer parses on the main
  thread for up to 1 ms (`sync_parse_timeout`); when that is not enough, `request_autoindent` computes
  on a background task and applies the indentation **after** the edit, once the parse is done.
  Sources: [configuring Zed](https://zed.dev/docs/configuring-zed),
  [language extensions](https://zed.dev/docs/extensions/languages),
  [`buffer.rs`](https://github.com/zed-industries/zed/blob/main/crates/language/src/buffer.rs).
- **Helix.** `indent-heuristic` is `simple` (copy the previous line's indentation), `tree-sitter`
  (from `indents.scm` captures `@indent`, `@outdent`, `@align`, `@extend`, with a `tail` or `all`
  scope) or `hybrid`, the default, which computes the tree's *difference* from an existing line and
  applies it to that line's actual indentation, falling back in the order hybrid, tree-sitter, simple.
  Helix parses synchronously on each edit.
  Sources: [editor config](https://docs.helix-editor.com/editor.html),
  [indent queries](https://docs.helix-editor.com/guides/indent.html).
- **VS Code.** Language configurations give `indentationRules` (`increaseIndentPattern`,
  `decreaseIndentPattern`, `indentNextLinePattern`, `unIndentedLinePattern`) and `onEnterRules`
  (match `beforeText`/`afterText`/`previousLineText`, act `indent`, `outdent`, `indentOutdent`); with
  none, it indents after an open bracket and outdents when a closing one is typed. All regexps over
  lines, on the main thread, with no tree.
  Source: [language configuration guide](https://code.visualstudio.com/api/language-extensions/language-configuration-guide).
- **EditorConfig.** `indent_style` is `tab` or `space`; `indent_size` a number, or `tab` for the tab
  width; `tab_width` defaults to `indent_size`; `unset` removes a property; files are found from the
  file's directory up to one with `root = true`. Unset properties fall back to the editor's.
  Sources: [specification 0.17.2](https://spec.editorconfig.org/), [editorconfig.org](https://editorconfig.org/).

So the editors that use a tree either parse on the main thread (Emacs, Helix, Zed for 1 ms) or
change the text after the key, when a background parse ends (Zed). oso can do neither: ADR 0012 and
ADR 0111 keep tree-sitter off the UI thread, and an edit that lands later would make a keyboard
macro's replay depend on the worker's timing.

## Decision

The author chose on 2026-11-12, through the coordinator, the first option (a) of every question
under [Questions](#questions), which the text below describes, and with them the two new public
modules of `oso-core`, `indent` and `editorconfig`, and their rows in `guardrails/layers.txt`.
The steps are under [Steps](#steps).

### Where the answer comes from (Q1)

- **Indentation is computed on the UI thread, at the key, from the buffer's text and its settings
  alone.** Each mode has rules in `oso-core`, pure functions over the rope with a small lexer of
  their own. They read no tree, no `Highlights` and no worker's report. So:
  - `RET` and `TAB` never wait, and their result does not depend on when the worker last reported:
    a macro's replay, a test and the agent channel get the same indentation each time;
  - buffers that may hold secrets, files over 2 MiB, buffers loading and buffers with font-lock off
    indent like any other. Nothing leaves the UI thread, so nothing needs withholding;
  - adding a language means adding its rules in Rust; a declarative query (`indents.scm`) can come
    later for languages whose rules are not worth writing by hand (Alternatives).
- **Each answer is bounded.** A mode's rules lex forward from an **anchor**, a line known to start
  outside any string or comment: for Rust and TOML, the nearest line above that starts at column 0
  with a top-level item (`fn`, `pub`, `impl`, `mod`, `use`, `struct`, `enum`, `trait`, `const`,
  `static`, `type`, `extern`, `unsafe`, `async`, `macro_rules!`, `#[`, `#!`, `}`; in TOML a `[` table
  header), or the buffer's start. They lex at most 256 KiB back; past that, or at the 1 MiB budget one
  command shares across all its cursors, a cursor falls back to the previous non-blank line's
  indentation, as Helix's `simple`. Cursors share one forward pass in order: each starts from the later
  of its anchor and the cursor before it, carrying the lexer's state, so 1000 cursors in one function
  cost one pass over it. Step 1 measures both bounds and sets them so that `RET` with one cursor costs
  under 0.1 ms and the shared budget under 5 ms.
- **The answer is a column, or "keep".** Keep means: leave the line's indentation as it is (inside a
  multi-line string, where spaces would change the string's value; past the budget for `TAB`).

### The rules per mode

**Rust (Q9).** The lexer knows line and nested block comments, strings and byte strings with escapes,
raw strings with any number of `#`, char literals against lifetimes (`'a'` against `'a`), and the
brackets `{ ( [`. For a line L it finds the innermost bracket still open at L's start:

1. Inside a string or raw string: keep. Inside a block comment: the comment's opening column plus
   one when L starts with `*`, else the previous line's indentation.
2. L starts with the closer of the innermost open bracket (`}`, `)`, `]`, also `} else {` and
   `) -> T {`): the indentation of the line that holds that opener.
3. Otherwise, inside an open bracket: the indentation of the opener's line plus `indent_size`
   (**block indent**, as rustfmt's default `indent_style = "Block"` writes). At top level: 0.
4. **Continuation:** inside `{` or at top level, when the code line before L (skipping blank and
   comment lines) does not end a statement or item, that is, its last token is not `;`, `{`, `}` or
   `,`, and it is not an attribute line (`#[...]`), add `indent_size` once. This covers
   `let x =`, method chains (`.bar()` lines), `Pat =>` with its body on the next line and the bounds
   after `where`. Lines that start with `where` or `{` take no continuation, so a `where` clause and
   the brace after it sit at the item's indentation. Continuation does not stack: a chain of ten
   `.call()` lines stays one level in.

This matches what rustfmt writes, so reindenting rustfmt-formatted code changes nothing; step 1
checks that over every `.rs` file in this repository.

**TOML.** Inside a multi-line array or inline table: the opener's line indentation plus `indent_size`,
and a closing `]` or `}` at the line's start goes back to the opener's line. Inside a multi-line
basic or literal string (`"""`, `'''`): keep. Elsewhere: the indentation of the previous non-blank
line outside any bracket, so a file that indents its tables keeps doing so, and one that does not
stays at column 0.

**Markdown (Q8).** No lexer beyond lines: a list item is `-`, `*`, `+` or a number with `.` or `)`,
then a space; its content column is where the text after the marker starts.

- `RET` indents the new line to the content column of the item it is in, as markdown-mode's
  `markdown-indent-on-enter` does, and to the previous line's indentation elsewhere. Inside a fenced
  code block it copies the previous line's indentation: fenced Rust gets no Rust rules. ADR
  0111's step 7 highlights fenced Rust, but indentation does not read it yet.
- `TAB` cycles the line through the positions it could take, as markdown-mode's
  `markdown-indent-line` does: the content columns of the items above it that it could belong to,
  from the innermost out, then column 0, then the innermost again.
- `M-RET` (`markdown-insert-list-item`) starts a new item after the current one, with the same
  marker and indentation, a number one higher in a numbered list.

**Fundamental (Q7).** `RET` copies the indentation of the line it leaves, or none if that line is
blank, as Emacs's `indent-according-to-mode` does for `indent-relative`. `TAB` stays
`tab-to-tab-stop`, as today.

**What counts as indentation:** spaces and tabs at a line's start, as Emacs's `current-indentation`.
A full-width space (U+3000) is text: a Japanese paragraph's leading `　` is never copied, trimmed or
replaced.

### Commands and keys

| Key | Command | What it does |
|---|---|---|
| `TAB` | `indent-for-tab-command` | In a language mode, reindent each cursor's line (Q2) |
| `RET` | `newline` | Insert a line break and, with `electric-indent-mode` on, indent the new line (Q3) |
| `C-j` | `electric-newline-and-maybe-indent` | A line break with no indentation while `electric-indent-mode` is on, and with it while off (Q6) |
| `C-M-\` | `indent-region` | Reindent every line in each cursor's region (Q11) |
| `C-x TAB` | `indent-rigidly` | Shift each region's lines by a count, or by arrows (Q10) |
| `M-m` | `back-to-indentation` | Move each cursor to its line's first non-blank (Q12) |
| `M-i` | `tab-to-tab-stop` | Today's `TAB`, kept under Emacs's key (Q12) |
| `M-^` | `delete-indentation` | Join the line to the one before, leaving one space, as Emacs (Q12) |
| `M-RET` | `markdown-insert-list-item` | In Markdown only (Q8) |
| | `newline-and-indent` | A line break that always indents, for users who bind it |
| | `electric-indent-mode` | Turn `RET`'s indentation off and on, ADR 0097's minor-mode argument |

- **`TAB` (Q2), `tab-always-indent` t:** in Rust, TOML and Markdown it sets the line's indentation to
  the rule's column, wherever the cursor is on the line. A cursor inside the indentation goes to its
  end; one in the text keeps its place in the text. A line already right does not change. With a
  region shown it runs `indent-region`, as Emacs. Two cursors on one line reindent it once. In
  Fundamental, and in internal buffers, it runs `tab-to-tab-stop`.
- **`RET` (Q3):** with `electric-indent-mode` on (the default), at each cursor: delete the spaces and
  tabs just before the cursor, so the line left has no trailing whitespace, and a line left holding
  only indentation becomes empty; delete those just after it; insert the line break and the new
  line's indentation. The line left is not reindented. A region shown is replaced first, as today.
- **Between brackets (Q4):** a cursor directly between an opener and its closer (`{|}`, `(|)`,
  `[|]`, with only blanks between) gets an extra line: the closer goes to its own line at the
  opener's line indentation, and the cursor to an indented line between.
- **Electric closers (Q5):** in Rust and TOML, typing `}`, `)` or `]` as the first non-blank of a line
  reindents that line, so the closer lands under its opener's line. Typed only: a yank or `insert`
  does not.
- **Undo:** one step per command, as now: `RET` with its indentation is one step, as in Emacs and Zed.
  An electric closer joins the typing around it. The keys of one `C-x TAB` session join into one step.
- **Several cursors:** each cursor's indentation is computed from the text as the cursors before it
  leave it, so `foo(|a, |b)` with `RET` at both gives `b)` under `a,`. The whole command is one
  batched edit, and the 16 MiB cap on what one command inserts counts the indentation too.
- **Reach:** `Change` for the editing commands; `Look` for `back-to-indentation`, as for the other
  motions such as `C-a`, and for `electric-indent-mode`, as for the visual aids.
  The agent channel's rules for buffers that may hold secrets apply as to any edit.
- **The dev restart** hands over `electric-indent-mode`'s flag, beside font-lock's.

### `indent-region` (Q11)

- Acts on the region between each cursor and its mark, shown or not, as Emacs's `r`; with no mark it
  signals "The mark is not set now". Overlapping regions' lines are reindented once. Each changed line
  is one replacement in one batched edit; unchanged lines are not touched, so whitespace cleanup on
  save (ADR 0113) sees only the lines that changed.
- **Bounded on the UI thread:** one pass from the first region's anchor, up to a byte budget step 1
  measures to keep the command under 50 ms (expected around 4 MiB). A larger region signals "Region
  too large to indent" and changes nothing. A 2 MiB source file reindents in one command.
- With a count N, it indents every line to column N, as Emacs.

### `indent-rigidly` (Q10)

- With a count N, it shifts every line of each region by N columns, negative to the left, never past
  column 0, as Emacs. Blank lines stay empty.
- With no count, from a key, it opens a transient layer, as the prefix argument's digits have
  (ADR 0058): `<left>` and `<right>` shift by one column, `S-<left>` and `S-<right>` to the next tab
  stop of `indent_size`, and any other key closes the layer and runs as usual. The region stays shown
  while the layer is open. From the agent channel, with no count, it shifts by `indent_size`.
- `indent-rigidly-left`, `indent-rigidly-right`, `indent-rigidly-left-to-tab-stop` and
  `indent-rigidly-right-to-tab-stop` are commands too, as in Emacs.

### Settings and EditorConfig (Q13–Q16)

- **Each buffer has an indentation setting** (`oso_core::indent::Settings`): `indent_size`,
  `indent_style` (spaces or tabs) and `tab_width`. It comes from, the first that gives a property:
  1. the EditorConfig files that apply to the file;
  2. the configuration's `[modes.<mode>]` table: `indent-size`, `indent-style` (`"space"` or
     `"tab"`), `tab-width`;
  3. the compiled-in default: spaces, `indent_size` 4 in Rust, TOML and Markdown, `tab_width` 8.
- **Tabs (Q14):** with `indent_style = tab`, indentation is as many tabs of `tab_width` as fit, then
  spaces, as the specification says. Without it, indentation is spaces only, as ADR 0030 has it. A
  line reindented takes the buffer's style, whatever it had.
- **`tab_width`** is the buffer's display tab width too, as `DisplayMap` already holds it.
- **`[indent] electric = true`** sets `electric-indent-mode` at start; it applies when its value in the
  file changes, as the other settings do (ADR 0110).
- **No guessing from the file's text** (Q16): a file with no EditorConfig takes the configuration's or
  the default setting. Detecting a file's indentation, as VS Code's `detectIndentation`, can come later.

**Reading EditorConfig (Q13, Q15):**

- On the loader thread, before it builds the display map, so `tab_width` costs the UI nothing. Also
  when `C-x C-w` gives the buffer a new name, on a thread, applied when it comes back.
- From the file's directory up, stopping after a file with `root = true`, after the home directory
  (so `~/.editorconfig` counts), at a change of file system and at a directory another user owns. A
  file is read only if it passes the configuration file's checks (ADR 0108): regular, no link, this
  user's or root's, not writable by every user, at most 64 KiB.
- **The walk agrees with project detection (ADR 0114)** where it can: it starts from the place the
  file's path leads, follows no link, and stops where detection stops, through the same checks, which
  move out of `project.rs` into a function both call, so the two never disagree about which
  directories are "above" a file or whose they are. They differ only in where they end: detection
  ends at the nearest marker, while EditorConfig goes on past a project's root to `root = true` or
  the home directory, as the specification has it, so a `.editorconfig` above a repository still
  counts. The walk runs on the loader thread, not on `oso-project`, because the tab width must be
  known before the display map is built; it reads no marker and decides no trust.
- In every project, trusted or restricted: EditorConfig is declarative and can run nothing, which is
  why ADR 0019 allows it.
- Read once per visit. A changed `.editorconfig` applies to buffers visited after it, as Emacs's
  `editorconfig-mode` does.
- **Only the three indentation properties** are applied in M3. `trim_trailing_whitespace`,
  `insert_final_newline`, `end_of_line`, `charset` and `max_line_length` are parsed and ignored, and
  can be added later, each with the ADR it touches (ADR 0113, ADR 0030, ADR 0022).
- **The parser and matcher are ours (Q15),** a pure function in `oso-core`
  (`oso_core::editorconfig`): INI sections, and globs (`*`, `**`, `?`, `[...]`, `{a,b}`,
  `{n1..n2}`) translated to the `regex` crate's syntax, with a numeric range checked as a number,
  never expanded. Bounds: at most 64 KiB a file, 1000 sections, a glob of 4 KiB, braces nested 8 deep,
  and the `regex` crate's size limit; a section that passes one is skipped with a message in
  `*Messages*`. `indent_size` and `tab_width` are taken from 1 to 32; another value is ignored.
- `M-x describe-indentation` says, in the echo area, the buffer's setting and where each property came
  from (EditorConfig, the configuration, the default), with no path outside the project.

### Where the code lives

- **`oso_core::indent`** (new public module): `Settings`, `Style`, the per-mode rules as functions
  that answer a column or keep for a line, and the bounded lexers, one private module per mode. It
  reads a rope and does no I/O. A match on `Mode` chooses the rules; no new trait.
- **`oso_core::editor::indent`**: the editor's methods the commands call, as `editor/tab.rs` is
  today: reindenting lines at the cursors, `newline` with indentation, the region commands.
- **`oso_core::editorconfig`** (new public module): parsing and matching, no I/O.
- **The tip crate**: commands in `commands/indent.rs`, keys in `bindings/indent.rs`, reading the files
  in the loader (`loader/editorconfig.rs`), settings in `config/settings.rs`.
- `guardrails/layers.txt` gains the two `oso-core` modules: `indent` beside `buffer`, `motion` and
  `search` (it reads the rope and the display map), and `editorconfig` in the bottom row, beside
  `complete` and `project`, using no other module. This ADR is the one the layer rules ask for.

### Tests and measures

- Unit tests per rule, and a property test that the rules never panic and stay within their budget on
  random text, nested brackets 100,000 deep and long lines.
- **Rustfmt's code is a fixed point:** `indent-region` over every `.rs` file in this repository
  changes no line, apart from a short, listed set of cases the rules do not cover.
- EditorConfig: the specification's own test cases for globs and precedence, and a fuzz-style
  property test of the parser and matcher on random input within their bounds.
- `text_perf` cases: `RET` at 1 and 1000 cursors in a 2 MiB Rust file, `TAB` likewise, `indent-region`
  over 2 MiB, and the worst anchor search. A `just perf` typing scenario with `RET` in a Rust file
  (its thresholds are guard files, so the author sets them).
- `just e2e` for the keys (`TAB`, `RET`, `C-x TAB` and its layer) and `verify-in-app`.

## Questions

Put to the author on 2026-11-12, the recommended option first; each answer was the first, (a).

### How indentation is computed

1. **Where the answer comes from.**
   (a) Rules per mode on the UI thread, from the text alone, with their own bounded lexer. No wait,
   the same answer in secret, large and unparsed buffers, and replays that do not depend on the
   worker; but a hand-written heuristic per language, wrong in rare cases a tree would get right.
   (b) The same rules, but reading `Highlights` (the tree's strings and comments, mapped through the
   edits since the last report) instead of lexing. No lexer to write, but the answer depends on how
   fresh the last report is, so a replay can differ, and secret or large buffers still need the lexer.
   (c) The worker computes indentation from the tree (`indents.scm`) and keeps a per-line map in
   `oso-core`, which the UI reads and adjusts by rules for the lines edited since, as Helix's
   `hybrid`. Better answers in odd code, but two systems to keep right, stale right after `{` `RET`,
   and still rules for unparsed buffers.
   (d) Indent with rules at the key, then correct the line when the worker's parse ends, as Zed does.
   The text changes after the key, under the person's typing, and macros replay differently.
2. **`TAB`.**
   (a) In a language mode it reindents the line wherever the cursor is (Emacs's default
   `tab-always-indent` t); with a region shown it indents the region; Fundamental keeps
   `tab-to-tab-stop`, which moves to `M-i` too.
   (b) As (a), but in the text past the indentation it inserts spaces to the next stop
   (`tab-always-indent` nil). Familiar from other editors, but `TAB` mid-line then does two things.
   (c) As VS Code: insert `indent_size` spaces, or shift the region's lines. Simple, but no
   reindenting by key.
3. **`RET`.**
   (a) Indents the new line, removes the whitespace it leaves behind, and does not reindent the line
   it leaves. Electric closers (Q5) fix the common case of a misplaced `}`.
   (b) Also reindents the line it leaves, as Emacs's `electric-indent-mode` does. A line the person
   indented by hand moves when they press `RET` after it.
   (c) No indentation on `RET`; `C-j` indents, as Emacs before 24.4.
4. **`RET` between brackets** (`{|}`).
   (a) Open a line between: the closer on its own line, the cursor indented between, as VS Code, Zed,
   Helix and Emacs's `electric-pair-mode` do.
   (b) As plain Emacs: the closer moves to the new line, the cursor before it.
5. **Electric closers.**
   (a) In Rust and TOML, typing `}`, `)` or `]` first on a line reindents it.
   (b) No; only `RET`, `TAB` and `indent-region` indent.
6. **`C-j`.**
   (a) A line break with no indentation while `electric-indent-mode` is on, as Emacs since 24.4
   (`electric-newline-and-maybe-indent`).
   (b) The same as `RET`.
   (c) Left unbound.
7. **Fundamental mode.**
   (a) `RET` copies the indentation of the line it leaves, as Emacs does; `TAB` stays `tab-to-tab-stop`.
   A full-width space is never indentation.
   (b) `RET` indents nothing in Fundamental, as today.
8. **Markdown.**
   (a) As markdown-mode's defaults, the author's Emacs: `RET` indents to the item's content column,
   `TAB` cycles through the positions a line can take, `M-RET` starts a new item.
   (b) `RET` continues a list with a new marker, and `RET` on an empty item ends the list, as Typora,
   Obsidian and VS Code's Markdown extensions do. Fewer keys, but `RET` then inserts text besides
   whitespace.
9. **Rust's style.**
   (a) Block indent: the opener's line plus `indent_size`, as rustfmt writes.
   (b) Visual alignment where text follows the opener on its line (`foo(a,` then `b` under `a`), as
   classic Emacs `rust-mode` can. rustfmt rewrites such code to block indent anyway.

### Region commands and scope

10. **`indent-rigidly`.**
    (a) As Emacs: a count shifts; with none, a transient layer with arrows, plus the four named
    commands. Needs a small transient layer, as the prefix argument's digits have.
    (b) A count only, and the four named commands for users to bind. No new layer, but `C-x TAB`
    alone shifts by `indent_size` instead.
11. **`indent-region` on a large region.**
    (a) On the UI thread up to a measured budget (about 4 MiB, under 50 ms), refused past it.
    (b) On a worker over a snapshot, applied as one step when it ends if the buffer has not changed
    since, as a formatter would. No limit, but an edit that lands later, and a second path to test.
12. **Other indentation commands in M3.**
    (a) `back-to-indentation` (`M-m`), `tab-to-tab-stop` on `M-i`, `delete-indentation` (`M-^`) and
    `newline-and-indent` by name.
    (b) Only `back-to-indentation`; the rest later.

### Settings and EditorConfig

13. **Reading EditorConfig.**
    (a) In every project, trusted or restricted, on the loader thread, up to `root = true`, the home
    directory, a change of file system or another user's directory, each file passing the
    configuration file's checks.
    (b) Only in trusted projects. Safer in principle, but a restricted project's files then indent
    against its own style, and EditorConfig can run nothing.
    (c) Not in M3: only the configuration's per-mode settings.
14. **`indent_style = tab` and "indentation never inserts tabs".**
    (a) Honour it: a project that asks for tabs gets tabs in its files, and ADR 0030's rule becomes the
    default when nothing asks. Mixing spaces into a tab-indented project would break its style.
    (b) Never insert tabs, whatever EditorConfig says.
15. **The EditorConfig parser.**
    (a) Our own in `oso-core`, globs translated to the `regex` crate, with bounds. No new dependency,
    a few hundred lines, tested against the specification's cases.
    (b) The `ec4rs` crate (an EditorConfig core in Rust). Less code, but a new dependency to vet, and
    its bounds on crafted globs are its own.
    (c) `globset` as a direct dependency (already in the tree under `ignore`) for the globs, our own
    INI reading. Its `{a,b}` and `**` differ from EditorConfig's in details, and it has no numeric ranges.
16. **Configuration and defaults.**
    (a) `[modes.<mode>]` with `indent-size`, `indent-style`, `tab-width`, and `[indent] electric`;
    EditorConfig over the configuration over the defaults (4 spaces, tab width 8); no guessing from the
    file.
    (b) As (a), but the configuration over EditorConfig, so the person's choice always wins. A
    project's style then loses to a personal one.
    (c) As (a), plus guessing `indent_size` and the style from the file's first indented lines when
    nothing else gives them, as VS Code's `detectIndentation`.
17. **Yanked text.**
    (a) Inserted as it is, as Emacs; `C-x C-x` then `C-M-\` reindents it.
    (b) Reindented as it goes in, as Zed's `auto_indent_on_paste`. Convenient for code, but it changes
    text the person copied, also in prose and in strings.

## Steps

One pull request per step, each with `just ci` and its own review; the steps that change keys also run
`just e2e` and `verify-in-app`.

1. **The rules (`oso_core::indent`).** `Settings`, `Style`, the Rust, TOML, Markdown and Fundamental
   rules with their lexers and budgets, as pure functions over a rope; the `layers.txt` row. Unit and
   property tests, the rustfmt fixed point over this repository's `.rs` files, and `text_perf` cases
   that set the 256 KiB, 1 MiB and `indent-region` budgets from measurements. No command changes yet.
2. **`TAB`, `RET` and `C-j`.** `indent-for-tab-command`, `newline` with `electric-indent-mode`,
   `electric-newline-and-maybe-indent`, `newline-and-indent`, the line between brackets, electric
   closers, several cursors, undo, the dev restart's flag, and `[indent] electric` with
   `[modes.<mode>]` in the configuration. A `just perf` scenario with `RET` in a Rust file, report-only
   until the author sets its thresholds.
3. **The region and the small commands.** `indent-region` with its budget and count,
   `indent-rigidly` with its transient layer and the four named commands, `back-to-indentation`,
   `M-i`, `delete-indentation`, and Markdown's `TAB` cycle and `M-RET`.
4. **EditorConfig (`oso_core::editorconfig`).** The parser and matcher with their bounds and the
   specification's cases, and its `layers.txt` row; the stop checks shared with project detection;
   the walk on the loader thread before the display map is built, and again after `C-x C-w`;
   `indent_style`, `indent_size` and `tab_width` applied over the configuration;
   `describe-indentation`.

Steps 1 to 3 need no EditorConfig: until step 4, a buffer's setting is the configuration's or the
default.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it. The status of each ADR named
gains `amended by 0115`.

- [ADR 0030](0030-m2-editing-commands.md), "indentation never inserts tabs": unless EditorConfig's
  `indent_style = tab` applies to the file (Q14).
- [ADR 0051](0051-m2-plan-decisions.md), "TAB inserts spaces to the next tab stop until modes bring
  indentation (M3)": in Rust, TOML and Markdown, `TAB` indents; Fundamental and internal buffers keep
  the spaces, and `M-i` inserts them anywhere.
- [ADR 0056](0056-typing.md), "`tab-to-tab-stop` (`TAB`)": `TAB` is now `indent-for-tab-command`;
  `newline` also indents while `electric-indent-mode` is on.
- [ADR 0111](0111-tree-sitter-highlighting.md), "Indentation needs the tree, and right after an
  edit": indentation reads no tree in M3; its rules lex the text on the UI thread. A report need not
  carry a copy of the tree for it.
- [ADR 0113](0113-query-replace-keyboard-macros-and-whitespace-cleanup.md), "whether to read
  `.editorconfig`": it is read, for its indentation properties only.

## Consequences

- `RET` and `TAB` cost microseconds and never wait, in any buffer, and a keyboard macro that types
  code replays with the same indentation every time.
- Indentation is a heuristic: code that rustfmt would not write, or unusual macros, can indent wrongly;
  `indent-region` then does too, and the person fixes it by hand or with a formatter later.
- A buffer that may hold secrets indents like any other, with nothing sent anywhere.
- A project's `.editorconfig` decides its files' indentation and tab width, also over the person's
  configuration, and can turn tabs on in its files.
- Each new language needs rules written in Rust, until a query-based path exists.
- Follow-up work: a query-based indenter on the worker for `indent-region` in more languages; fenced
  code in Markdown with the language's rules; the other EditorConfig properties; detecting a file's
  indentation; formatting on save with a trusted program.

## Alternatives considered

- **Parsing on the UI thread for a short budget, as Zed's 1 ms:** breaks ADR 0012 and ADR 0111, and a
  crafted file can make one call to tree-sitter run about 1 s (ADR 0111, step 3).
- **Waiting for the worker's tree before answering `RET`:** the UI thread would wait on parsing.
- **Correcting the line when the parse ends (Zed's async path):** see Q1 (d).
- **A tree-based indent map kept by the worker (Helix's hybrid over a stale tree):** see Q1 (c).
- **Indent depth times `indent_size`, from the count of open brackets:** wrong as soon as a file
  indents a block differently; the opener's line's own indentation follows the file.
- **Copying the previous line's indentation everywhere (Helix's `simple`):** right in prose, but no
  help after `{` or before `}`; it stays the fallback past the budget.
- **A trait per mode for indenters:** the list of modes is closed and small; a match keeps it in one
  place, and a trait can come with modes from outside the crate.
- **Expanding EditorConfig's `{n1..n2}` into alternatives:** a crafted `{1..999999999}` would build a
  huge regex; a number checked against the range costs nothing.

## As built

### Step 1: the rules

- **`indent` sits on the row of `undo` and `kill` in `layers.txt`,** not beside `buffer`,
  `motion` and `search` as Where the code lives says (the author's choice, 2026-11-12, through
  the coordinator): it uses only the rope, the display map and `sum_tree`, and below `buffer`,
  `Buffer` can hold its `Settings`.
- **One function per mode, chosen by the caller.** `indent` cannot name `buffer::Mode`, which is
  above it. It has one function per mode with the same signature, `rust`, `toml`, `markdown` and
  `fundamental`, and step 2 chooses one with the match on `Mode`, in `buffer` or in
  `editor::indent`. Inside, the four share one pass through a private trait; there is no public
  trait.
- **The tab width has one owner:** the buffer's `Settings::tab_width`, from 1 to 32, which also
  sets the display map's `TabWidth` (the author's choice, 2026-11-12). Step 2 or step 4 wires it;
  until then the display's tab width stays the default 8, which `Settings::default` matches.
- **The public items:** `Settings` (`new(indent_size, style, tab_width)`, which refuses a value
  outside 1 to `Settings::MAX`, 32; the three getters; `indentation(column)`, the text that
  indents to a column), `Style`, `Answer` (`Column` or `Keep`), `Line` (`Row(row)`, an existing
  line, or `New(offset)`, the line a break at that offset would start), `Budget` (`new`,
  `with_limit(bytes)`, as `motion::WalkBudget` names it, and `is_spent`), the constants
  `LINE_LOOKBACK`, `COMMAND_READ` and `LINE_COST`, the four rule functions, and
  `markdown_stops(rope, settings, row, budget)` for step 3's `TAB` cycle. The lines asked are in
  the order of where they start; one out of order gets the fallback, and one asked twice the
  same answer.
- **The fallback:** past the budget or the lookback, or where the rules cannot tell (inside
  brackets deeper than they keep), a new line takes the indentation of the nearest non-blank
  line before it, looking 128 bytes back, then 1 KiB; an existing line keeps its own, as
  Decision says for `TAB`. What the fallbacks read comes from a reserve of 64 KiB a command apart
  from the budget; past it, a new line gets no indentation. Markdown's existing lines always
  keep: `TAB` cycles through `markdown_stops` instead. `Keep` for a new line means insert no
  indentation and delete no space around the break.
- **No answer past column 4096:** a line that would go deeper keeps its indentation, so that no
  command inserts more than that for one line, whatever the text.
- **Each answer counts for the next:** a line the command reindents, or a new line it starts,
  has the answered indentation for the lines after it. So `foo(|bar(|x))` with `RET` at both
  cursors gives 4, then 8, as the second opener's line is the new one; and a line the command
  moves off column 0 is no anchor, so reindenting pasted code that starts at the margin keeps its
  brackets. A break at a line's start leaves an empty line behind it, so the new line takes no
  indentation in Fundamental and Markdown. A line that starts inside a string, a multi-line one or
  a raw one, or one that a `\` at a line's end goes on, lays out what follows as the line the
  string starts on (step 2 widened this wording; the rule was so from step 1).
- **Rust's continuation follows rustfmt more closely than rule 4.** The repository's fixed point
  needed:
  - continuation inside every bracket, not only `{` and top level, with an opener at a line's end
    ending it: rustfmt indents a chain inside a call's arguments too;
  - a line that starts with `.` sits with the chain's call before it, or one level past the line
    its chain starts on; one that starts with an operator and a space (`&&`, `||`, `+`, `==`) sits
    with the operand before it; any other line that goes on sits with the one before it, if that
    one went on too; after a line of closers only (`})`, `)?`), a chain sits with the closers and
    an operand with the line that opened them; a value on the line after `=` keeps the operands'
    place. So continuation does not stack: ten `.call()` lines, or ten lines of a macro's body
    that end no statement, stay one level in;
  - a `match` guard's operands one level past the `if` line, `| Pat` lines in a `match`'s arms
    with the first pattern, and `else` with no continuation, for `let ... else {`;
  - a `}` that closes a non-empty `{` opened on the same line, as `Foo { a }` or
    `unsafe { f() }`, ends no statement; an empty `{}` does.
- **The shared pass:** every cursor carries the state on, and the pass starts afresh at each
  anchor it passes outside a string or comment. A line within 1 KiB of the one before reads on to
  it, as does the row right after the row before; a farther one looks back for its own anchor, in
  windows from 256 bytes up to 4 KiB, no farther than the line before. When the line before ends
  inside a string or comment, the pass first reads on to where it closes, as a line inside one
  that looks like an anchor proves nothing. 1 KiB measured best of 512 bytes, 1 KiB and 4 KiB for
  1000 cursors: at 4 KiB they read on through the whole file and ran out of budget.
- **What the budget counts:** each byte read once, the lookback's too, and at least `LINE_COST`,
  32 bytes, a line, as reading a line costs about as much as lexing 32 bytes; so short lines are
  bounded in time too. A region of `bytes` and `lines` needs at most
  `bytes + lines * LINE_COST`, plus its first line's lookback. At most 4 KiB of spaces and tabs is
  read at the start of a line asked about.
- **Bounded memory:** the lexer keeps 256 open brackets and counts any deeper; 100,000 brackets
  deep is tested. Nothing recurses. A line longer than one of the rope's chunks is copied once,
  within the budget. The column a block comment opens at is measured once, at the end of its
  line.
- **Measured** with `cargo run --release -p oso-core --example text_perf -- indent` on the
  author's Mac. "Rust" is a 2 MiB file of items as rustfmt writes them, with an anchor every
  ~900 bytes; "impl" is one `impl` of 2 MiB, with no anchor past its first line:

  | Case | p50 | p99 |
  |---|---|---|
  | Rust, `RET` at 1 cursor (5000 random lines; max 39 µs) | 1.5 µs | 3.0 µs |
  | Rust, `RET` at 1000 cursors, one pass | 1.4 ms | 1.4 ms |
  | impl, `RET` at 1000 cursors (the budget runs out) | 1.6 ms | 1.6 ms |
  | impl, one cursor whose anchor is at the lookback (64 KiB lexed) | 153 µs | 171 µs |
  | impl, one cursor with no anchor in the lookback | 23 µs | 35 µs |
  | Markdown, one cursor 64 KiB from the start | 108 µs | 130 µs |
  | A 64 KiB line of `/**/` after a tab and a wide character, `RET` after it | 70 µs | 84 µs |
  | 64 KiB of blank lines, `RET` after them | 103 µs | 127 µs |
  | 2 MiB of blank, `x` or `{` lines, 1000 cursors (the budget runs out) | 0.8–2.0 ms | 2.1 ms |
  | Rust, every line of 2 / 4 / 8 MiB, as `indent-region` asks, within the budget above | 8.6 / 17 / 35 ms | |

- **The bounds set from them:**
  - `LINE_LOOKBACK` is **64 KiB**, not 256 KiB. In this repository the farthest any line is from
    its anchor is 60 KiB (99% within 30 KiB, 90% within 9 KiB), and one line's lexing runs at
    about 400 MiB/s, so the worst one-cursor `RET`, with its anchor at the lookback, costs about
    0.15 ms; at 256 KiB it cost 0.6 ms. A typical `RET` costs a few microseconds, well under
    0.1 ms; only an anchor near the lookback goes over it. Markdown's only anchor is the buffer's
    start, so a list item past 64 KiB into a document falls back; oso's largest document is
    52 KB.
  - `COMMAND_READ` stays **1 MiB**: 1000 cursors in the 2 MiB Rust file stay within it, at
    1.4 ms, and where they cannot, the command stops by 2.1 ms, under the 5 ms asked.
  - **`indent-region`** answers about 230 MiB/s, so 4 MiB takes about 17 ms and 8 MiB 35 ms,
    without the edit. 4 MiB, as Decision expected, leaves room for the batched edit under 50 ms;
    step 3 measures it with the edit and gives `Budget::with_limit` what the region needs.
- **The rustfmt fixed point** runs with `just ci`: reindenting every `.rs` file of the repository
  takes 0.6 s in a debug build. It reads only files under the workspace's root, past directories
  whose names start with `.` and those named `target` or `target-*` (step 2), and follows no link, and it gives each file the budget a
  region of its size needs, so no line falls back. Five files have lines the rules indent
  otherwise, listed in the test with the count and the reason; a listed file that is gone or has
  none fails it:

  | File | Lines | Why |
  |---|---|---|
  | `oso-render/src/rows.rs` | 22 | a `proptest!` body, which rustfmt does not format |
  | `oso/src/dispatch/prompts.rs` | 3 | `\|` patterns inside parentheses sit with the first one |
  | `oso-render/src/lib.rs` | 1 | the same |
  | `oso-platform/src/text/coretext/direction.rs` | 1 | an operand of `&&` inside one of `\|\|` goes one level further |
  | `oso-core/src/syntax/highlights/tests.rs` | 1 | a chain on a tuple goes one level past its closer |

  A change that makes the rules indent another line of the repository otherwise fails the test,
  in `oso-core`, whichever crate the line is in; the fix is in the rules, or a new entry with its
  reason.
- **Left to step 2** (taken up there, below): a crate-private helper for a line's current
  indentation, and asking for the empty line between brackets.

### Step 2: `TAB`, `RET` and `C-j`

The author chose, on 2026-11-12 through the coordinator, the recommended option of each question
step 2's plan put:

- **The empty line between brackets** is asked with a new `Line::Empty(offset)`, which every mode's
  rules answer as a new line with nothing after the break; asked before `Line::New` at the same
  offset, it is the line `New`'s line leaves.
- **`TAB` with a region shown** reindents the cursors' lines, as with none; step 3 makes it run
  `indent-region`.
- **The indentation helper** stays `pub(crate)` (`indent::indentation`, over a line's pieces):
  spaces and tabs only, so `RET` never trims a U+3000, and step 3's `M-m` follows it; `C-a` keeps
  `motion`'s test.
- **A new line inside a block comment** that does not start with `*` takes the indentation of the
  nearest non-blank line above, the comment's text, not of the blank line a break leaves.
- **The `just perf` scenario** `indent_ret_*` (`RET` in the middle of a 1 MiB Rust file) is wired
  into the benchmark with one call in `bench.rs`, reported only; the author sets its thresholds.
- **The public items:** `Line::Empty`, `Buffer::indent_settings` and `set_indent_settings`,
  `Mode::indent_rules`, and on `Editor` `electric_indent`, `set_electric_indent`,
  `indent_for_tab`, `newline`, `indent_typed_closers`, `set_mode_indents` and `mode_indent`.

As built:

- **The tab width has one owner, the display map.** `Buffer::indent_settings` gives the indent
  size and style set on it with the tab width the buffer is shown with; `set_indent_settings`
  changes the size and style only. The tab width comes from the loader, which builds the display
  map with the mode's configured `tab-width` when a file is visited, and with the buffer's own when
  it is read again, so nothing rebuilds a display map, or a window's wrap, on the UI thread. A
  configuration change gives open buffers the new indent size and style at once; a new tab width
  applies to files visited after it (the author's choice). A buffer carried over a dev restart
  keeps the default tab width until visited again.
- **Each mode's indentation is the editor's** (`Editor::set_mode_indents`, `mode_indent`; the
  author's choice): the app sets the table when the configuration applies, which gives every open
  buffer of the person's its mode's indent size and style; a visit, a mode command (`rust-mode`
  and the others) and a new name that changes the mode (`C-x C-w`) give it the same.
- **The file named on the command line** is visited before the configuration's first look, so its
  display map is built with the default tab width of 8 even with a mode's `tab-width` set, while
  its indent size and style follow when the look applies. With `indent-style = "tab"`,
  `tab-width = 4` and `indent-size = 4`, a level there is then four spaces, not a tab, so
  indentation mixes tabs and spaces in that file until it is visited again. A known limit;
  rebuilding the display map, or reading the file again, when the first look changes a mode's
  tab width would remove it.
- **Every buffer added** (`Editor::add_buffer`) takes its mode's indent size and style, unless it
  is internal: a visit, `C-x b` to a new name, a new `*scratch*`.
- **`RET`** (`newline`, `newline-and-indent`, and `C-j`'s `electric-newline-and-maybe-indent`) is
  `Editor::newline(ending, count, indent, limit)`: a shown region goes first; at each cursor the
  spaces and tabs before and after it go, at most 4 KiB each way and never past the next cursor,
  and `count` breaks then the last new line's indentation go in, as one batched edit; inside a
  string, the break alone, with nothing trimmed. A space a combining mark is on stays, as the
  mark's base. All the cursors share one call to the rules, on the text before the edit, so a
  cursor's answer counts the lines the ones before it make. The 16 MiB limit is checked before
  anything changes, with each cursor's indentation counted at its deepest (twice 4096 columns),
  so whether `RET` fits depends only on the count, the line ending and the cursors; past it,
  nothing changes and it signals "Too much text to insert".
- **In Markdown, `RET` keeps a line break of spaces:** two spaces or more after text, at the
  line's end and outside a fenced code block, break the line, as whitespace cleanup on save
  (ADR 0113) keeps them; whether the cursor is in a fence is read in one pass for all such
  cursors. Other blanks before the cursor go, as in other modes.
- **The line between brackets** is opened in Rust and TOML only (the author's choice): in Markdown
  `[|]` is a link's text, and `RET` there breaks the line alone. In TOML only between `[` and
  `]`: an inline table cannot span lines in TOML 1.0, and `(` means nothing in it. Two cursors
  between the same brackets (`{  |  |  }`) open no line between, and the closer goes on the
  second cursor's line; a known limit.
- **The limit counts the deepest indentation** only when `RET` indents: `C-j`, and `RET` with
  `electric-indent-mode` off, reserve nothing for it.
- **`C-j` in a prompt** answers it (`exit-minibuffer`), and **in a search** adds a line break to
  the search string (`isearch-newline`), as in Emacs, so the global `C-j` reaches neither.
- **`TAB`** in Rust and TOML asks the rules for each cursor's line once and replaces only
  indentation that differs. In Markdown it stays `tab-to-tab-stop` until step 3 brings its cycle
  through `markdown_stops`, as in Fundamental mode and internal buffers; `tab-to-tab-stop` keeps
  its name for `M-i` in step 3.
- **A typed closer** reindents its line inside the typing command, so it undoes with the typing;
  `insert` and a yank do not. It is a second batched edit after the typing's.
- **`TAB` moves a cursor** to the end of the indentation only on lines the rules answered a
  column; in a string it stays.
- **Where the code is:** the configuration's indentation is read in `config/indentation.rs`, not
  `config/settings.rs`; `newline` moved from `commands/edit.rs` to `commands/indent.rs`, with the
  keys `TAB`, `RET` and `C-j` in `bindings/indent.rs`.
- **Measured** with `text_perf -- indent`, release, the author's Mac, in the 2 MiB Rust file, the
  whole command through the editor:

  | Case | p50 | p99 |
  |---|---|---|
  | `RET` at 1 cursor | 5.6 µs | 85 µs |
  | `RET` at 1000 cursors (breaks alone: 1.6 ms; the rules alone: 1.5 ms) | 4.1 ms | 4.5 ms |
  | `TAB` at 1000 cursors | 2.9 ms | 3.0 ms |
  | `}` typed at 1000 cursors, each dedenting its line (the typing alone: 0.9 ms) | 3.2 ms | 3.7 ms |

  `RET` at 1000 cursors is a little past ADR 0092's 4 ms share of an edit, and within a frame;
  folding a typed closer's reindent into the typing's edit would save about 0.9 ms of its 2.3 ms.
  Both are left for when a frame needs them.
- **`[indent] electric`** and **`[modes.<mode>]`** with `indent-size`, `indent-style` (`"space"` or
  `"tab"`) and `tab-width`, each 1 to 32, for `rust`, `markdown`, `toml` and `fundamental`. Another
  name in `[modes]` or in a mode's table is reported and skipped. `electric` applies when its value
  in the file changes, and a dev restart's handed-over flag wins at the next look, as the theme's
  does.
- **A bracket opened on a line a string goes on to** takes that line's own indentation, as its
  opener's line: `g("a \⏎     b", vec![⏎1,⏎]);` gives 11 and 7. rustfmt never writes this shape,
  so it stays a limit of the rules.

### Step 3: the region and the small commands

The author chose, on 2026-11-12 through the coordinator, the recommended option of each question
step 3's plan put:

- **`indent-region` with no count in Markdown, Fundamental mode and internal buffers** signals "No
  indentation rules in this mode" and changes nothing; with a count it works in every mode. `TAB`
  with a region shown runs `indent-region` in Rust and TOML only; in Markdown it cycles the
  cursors' lines and in Fundamental mode inserts spaces, as with no region.
- **A line of only spaces and tabs** in a region becomes empty, whether reindented, put at a
  column or shifted; an empty line stays empty, so no command leaves trailing whitespace. Inside a
  string the rules keep it, as they keep any line there (the review of step 3).
- **`indent-rigidly` and `indent-region` with a count** take the same bounds as `indent-region`
  (below), and the 16 MiB limit on what one command inserts, checked before anything changes.
- **Markdown's `TAB` in a fenced code block** inserts spaces to the next tab stop at the cursor,
  as `RET` there copies the line above as Fundamental mode does.
- **`M-RET` outside a list item, or in a fenced code block,** signals "Not in a list" and
  changes nothing; one such cursor stops them all.
- **`M-^`** is Emacs 27's: with a prefix argument it joins the next line to this one, and with a
  region shown it joins every line of the region.
- **`C-g` and any other key** close the layer of `C-x TAB` and run as usual, so `C-g` hides the
  region, as Emacs's `indent-rigidly-map` does.
- **The public items:** `IndentError` (`NoMark`, `TooLarge`, `NoRules`, `NotInList`,
  `TooMuchText`, `Edit`), `Shift` (`Left`, `Right`), and on `Editor` `indent_region`,
  `indent_rigidly`, `indent_rigidly_to_tab_stop`, `indent_rigidly_layer`,
  `set_indent_rigidly_layer`, `back_to_indentation` (which returns whether a cursor moved;
  Shift selects in the command, as for the other motions), `delete_indentation` and
  `markdown_insert_list_item`. The region commands take the most bytes they may insert, as
  `newline` does.

As built:

- **Two bounds, not one budget of 4 MiB** (the author's choice): `indent-region` reads at about
  2.7 ms a MiB of the rules' budget, and each changed line is a splice of the edit, about 1 µs
  (ADR 0092), so a byte budget alone could not keep the command under 50 ms. A command takes at
  most **6 MiB of the rules' budget** (`bytes + lines * LINE_COST`, plus one `LINE_LOOKBACK`),
  checked before anything is read, and changes at most **30,000 lines**, checked before the edit;
  past either it signals "Region too large to indent" and changes nothing. A 2 MiB Rust file
  needs about 4.6 MiB of the budget.
- **Corrected:** "A 2 MiB source file reindents in one command" under `indent-region` (Q11) holds
  only when at most 30,000 of its lines change. A whole 2 MiB file pasted at the margin, 84,000
  lines, is refused, and is reindented in parts. A batched splice for large edits would lift
  this; it is a later ADR (`open-questions.md`).
- **`M-^` over a region** joins at most **15,000 lines** and reads at most **2 MiB** (`bytes +
  lines * LINE_COST`) a command (the author's choice): a join removes a line break, a splice of
  about 2 µs. Past either it signals "Region too large to indent" and changes nothing.
- **The region commands insert at most 4 MiB** a command, under the 16 MiB any command may (the
  author's choice): a deep column over many lines costs the edit about 3 ms a MiB on top of its
  splices, so `C-u 559 C-M-\` over 29,000 lines took 93 ms. Past it they signal "Too much text to
  insert" and change nothing.
- **Corrected:** "The keys of one `C-x TAB` session join into one undo step" (Commands and keys)
  holds while they change at most 30,000 lines in all; the key that would take them past it starts
  a new step (the author's choice), so that undoing one replays no more than about that many
  splices. A session over a small region stays one step.
- **The region's lines are read in one pass** (`editor/indent/leads.rs`): each line's
  indentation, what it is made of, whether it is blank, and where its text ends, for
  `indent-region`, `indent-rigidly` and `M-^` over a region. A seek to each line cost about
  600 ns a line, 50 ms of a 2 MiB file's 61. A line already right is told from what the pass
  noted, with no second read, in either style. All the regions' lines, blank ones too, share one
  call to the rules and one budget, so the pass reads on from each line to the next; a pass that
  runs out of the budget refuses the command, "Region too large to indent", rather than leave
  part of the regions reindented. A line whose indentation is longer than 4 KiB is left as it is.
- **A blank a combining mark is on** stays as the mark's base, in a region's commands and in
  `M-^`, as `RET` keeps it (step 2): the indentation ends before it.
- **A region's lines** are Emacs's: every line that starts before its end, from its start's line
  for the rules (`indent-region-line-by-line`), and from the first line that starts in it for a
  column or a shift (`indent-rigidly`, `indent-region` with a count), so a region that starts
  inside a line leaves that line to them. An empty region has none. Overlapping regions' lines
  count once.
- **The layer of `C-x TAB`** is the editor's, as isearch is, and belongs to the window and buffer
  it opened in: `Keymaps::active` puts its keymap first among the minor layers while it is open
  there and no prefix argument reads. Another buffer shown in that window, by the agent channel
  or a load, leaves the layer behind, and the next command closes it. A file read again waits
  while the layer is open, as it does for isearch, and an edit that no key of the layer made is
  never joined into its step. The dispatcher closes it before any command but the layer's four
  shifts that the person runs, before every command of the agent channel, and on an undefined key
  or `C-g` under a prefix. Closing it joins the edits since it opened into one undo step
  (`Buffer::join_undo_since`, as query-replace does), which goes back to the cursors before it,
  with no limit of keys but the lines they change (below). While it is open, each shift leaves
  the region as shown as it was. `C-x TAB` with no mark signals "The mark is not set now", as
  Emacs's `r` does.
  A dev restart does not hand the layer over.
- **To a tab stop** moves each region's least indented non-blank line to the next or previous
  multiple of `indent_size`, and the region's other lines by as much. Shifting right takes no line
  past column 4096 unless it is there already.
- **Markdown's `TAB`** keeps no state between keys: a line at one of its stops goes to the next,
  wrapping, and a line at none goes to the first, so a keyboard macro replays the same. Every
  cursor's line is read in one pass from the buffer's start (`indent::markdown_lists`,
  crate-private), so past `COMMAND_READ` a line is left as it is, not moved to column 0.
- **`M-RET`** breaks the line at each cursor, the blanks around it gone, and starts the new item
  with the innermost open item's indentation, marker and spaces, the number one higher, `.` or `)`
  kept; when the number grows a digit, one space fewer follows it if one can go, as markdown-mode
  does. On a blank line the item goes on it; with the cursor before an item's marker, the new item
  goes on a line before it, with that item's number. Later items are not renumbered, as in Emacs;
  `M-RET` takes no count yet (`open-questions.md`). It is bound in a Markdown keymap above the
  global one, so in other modes `M-RET` is undefined.
- **A single `M-^`** reads only the ends of its two lines, at most 4 KiB at each, so it works
  beside a line of any length; joins over a region read their lines in one pass.
- **`M-^`'s space** follows `fixup-whitespace`: none when either side is a line's edge (a blank
  line joined, or the joined line blank), after `(`, `[` or `{`, or before `)`, `]` or `}`; a
  full-width space is text. Each cursor goes to its first join, before the space.
- **`M-m`** reads at most 4 KiB of a line's indentation, as `TAB` does.
- **Measured** with `text_perf -- indent`, release, the author's Mac, the whole command:

  | Case | p50 | p99 |
  |---|---|---|
  | `indent-region`, 2 MiB Rust already right (reads only) | 12.5 ms | 12.7 ms |
  | `indent-region`, 30,000 short lines that change | 27.8 ms | 28.0 ms |
  | `indent-region`, 2 MiB right then 30,000 lines that change (the most it takes) | 43.5 ms | 43.6 ms |
  | `indent-region`, 2 MiB at the margin (refused after reading) | 10.8 ms | 11.0 ms |
  | `indent-region`, 2 MiB of tab indentation already right | 10.2 ms | 10.3 ms |
  | `indent-region`, 70,000 lines each before a blank line | 9.9 ms | 10.4 ms |
  | `indent-region`, 1000 regions of 30 lines apart in 5 MiB | 7.5 ms | 7.7 ms |
  | `indent-region` to column 140 over 29,000 lines (4 MiB inserted) | 38.5 ms | 39.0 ms |
  | `indent-rigidly` over 29,000 lines | 30.9 ms | 31.6 ms |
  | `indent-rigidly`, 1000 regions of 30 lines | 38.2 ms | 38.7 ms |
  | `M-^` over 10,000 lines | 20.1 ms | 20.3 ms |
  | `M-^` over 15,000 lines, 2 MiB read (the most it takes) | 43.4 ms | 43.6 ms |
  | Markdown `TAB` at 1000 cursors in a 2 MiB document | 1.7 ms | 1.8 ms |
  | `M-RET` at 1000 cursors | 1.3 ms | 1.3 ms |
