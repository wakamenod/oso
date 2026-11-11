# 0111. Tree-sitter highlighting

- Status: Accepted
- Date: 2026-11-12

## Context

[ADR 0012](0012-tree-sitter.md) chose native tree-sitter for syntax, with Rust, Markdown and
TOML first, parsing on a background thread with cancellation and timeouts, results replacing a
syntax layer atomically, and fuzzing. [ADR 0006](0006-display-model.md) made decorations layers
of positions that producers own and swap from background threads. M3 builds the first of them.
This ADR designs it before any code.

What changed since those ADRs were written:

- **The display pipeline exists.** Rows on screen are a `window::Screen`, built once a frame,
  each row a part of a logical line with its byte range (`ScreenRow::start`, `end`)
  ([ADR 0104](0104-soft-wrap-drawn-in-view-rows.md)). Edits go through `Editor::apply_edits`,
  on O(1) clones committed together ([ADR 0053](0053-edit-core.md)), and the rope reports the
  chunks an edit rebuilt (`RopeEdit`), while the exact splices are `OffsetEdit`s.
- **`Face` lives in `oso-render`** (`pane.rs`), and a face change is a shaping break
  (`rows.rs`, `break_at`): a row of one face is one call to the shaper. Shaped rows hold a
  face, never a colour, and glyph instances take their colours from the theme each frame, so a
  theme switch shapes nothing again ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)).
  ADR 0061 left `Face` in `oso-render` "until M3's highlighting in a library crate needs it".
- **Themes are data with a contrast test per face** (`theme.rs`): every face declares its pairs
  in a `match` with no wildcard, at 7:1 for text that is read, 4.5:1 for labels and 3:1 for
  marks, and fills and marks keep OKLab distances. ADR 0089: "Syntax faces join the table, and
  its contrast rules, when syntax highlighting comes (M3)."
- **Background work has a pattern:** a lazily spawned thread per producer in the tip crate
  (`app/searches.rs`, `app/wraps.rs`), jobs over O(1) snapshots, superseded by a generation, a
  report through the app's `Waker` as a `UserEvent`, `catch_unwind` around the job.
- **Secret buffers** have a `Verdict`, withheld until the loader judges the file `Ordinary`
  ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md), ADR 0051). A yank can make a
  buffer secret later.
- **`Buffer::mode`** is a closed list (`Fundamental`, `CopyList`, `CopyView`) "until major modes
  arrive in M3". The configuration file reserves per-mode tables (`[keys.rust]`,
  `[modes.markdown]`) ([ADR 0108](0108-the-user-configuration-file.md)).
- **tree-sitter's Rust API changed** since ADR 0012: 0.25 and later drop `set_timeout_micros`
  and the cancellation flag for a progress callback (`ParseOptions`, `QueryCursorOptions`)
  that can stop a parse or a query, and a parser stopped that way resumes where it stopped.
  `parse_with_options` reads the text through a callback in pieces of any length, so the
  rope's chunks feed it with no copy.
- **The local Windows check breaks.** `cargo clippy --target x86_64-pc-windows-msvc` on macOS
  runs build scripts, and the grammars' `build.rs` compile C with `cc`, which finds no MSVC
  headers on a Mac (`stdlib.h` not found; tried on a scratch crate). No dependency compiles C for
  that target today. A stub compiler and archiver (`CC_x86_64_pc_windows_msvc`,
  `AR_x86_64_pc_windows_msvc`) that write empty files let the check through; it checks the Rust
  and compiles no C. CI's Windows job builds with MSVC as before.

### The dependencies

Researched on 2026-11-12 with the `add-dependency` steps, in a scratch copy of the workspace
(`cargo tree`, `cargo deny check`, `cargo audit`; nothing built). Nothing is added by this ADR.

| Crate | Version (≥ 2 weeks old) | Licence | What it builds | Notes |
|---|---|---|---|---|
| `tree-sitter` | `=0.27.1` (2026-10-08) | MIT | `cc` compiles the runtime's `lib.c`; bindings are pre-generated, no `bindgen` or clang | ABI 13–15. `default-features = false, features = ["std"]`; `wasm` (wasmtime) stays off. `#match?` uses `regex::bytes` |
| `tree-sitter-language` | `0.1.9` (through the others) | MIT | nothing on our targets | `#![no_std]`, one newtype |
| `streaming-iterator` | `0.1.9` (through `tree-sitter`) | MIT OR Apache-2.0 | nothing | small |
| `tree-sitter-rust` | `=0.24.2` (2026-03-27) | MIT | `parser.c` 6.5 MB, `scanner.c` 12.6 KB | tree-sitter org; ABI 15 |
| `tree-sitter-md` | `=0.5.4` (2026-10-24) | MIT | block grammar 2.1 MB + 62.6 KB scanner, inline grammar 2.3 MB + 15.8 KB scanner | tree-sitter-grammars; active. Its `parser` feature stays off |
| `tree-sitter-toml-ng` | `=0.7.0` (2024-12-03) | MIT | `parser.c` 134 KB, `scanner.c` 2.6 KB | tree-sitter-grammars; ABI 14; quiet since 2025-07 |

- **Six crates are new to `Cargo.lock`:** `tree-sitter`, `tree-sitter-language`, `streaming-iterator` and the three grammars.
  `cc` 1.2.63, `regex` 1.12.4 and `serde_json` are already locked at versions they accept.
  `cargo deny check` passes (licences, bans, sources, advisories), no new duplicate versions,
  and `cargo audit` reports nothing new.
- **RustSec** lists nothing against these crates. It does list a grammar crate removed from
  crates.io for malicious code (`tree-sitter-pkl`, RUSTSEC-2025-0156): grammar crates are a real
  supply-chain risk (T2), so every version is exact and every `build.rs` is read before a bump.
- **Not used:** `tree-sitter-highlight` parses from scratch over one contiguous slice, with no
  old tree, so it fits neither the rope nor incremental parsing; a loop over `QueryCursor` is
  less code than adapting it. `tree-sitter-toml` (ikatyang) is on tree-sitter 0.20 and
  unchanged since 2021. tree-sitter-md's own `MarkdownParser` parses every inline node again on
  every parse and reuses old inline trees by index, which is wrong after an insertion.
- **Fuzzing upstream:** tree-sitter is not in OSS-Fuzz (attempts closed or open). Upstream runs
  randomized-edit fuzzing in its own CI. The runtime parses and queries with explicit stacks,
  not recursion. Past crashes in the Markdown scanner are fixed; its `realloc` is checked only
  by an `assert`.
- **Compile cost:** about 11 MB of generated C, compiled once with optimization (dependencies
  are optimized in the dev profile, [ADR 0024](0024-dev-build-profile.md)) and cached. The tip
  crate's incremental build does not change.
- **An alternative for TOML with no C:** `toml_parser` 1.1.5 is already locked, through `toml`
  ([ADR 0108](0108-the-user-configuration-file.md)). Its lexer gives tokens with spans, which is
  enough to highlight TOML (Q3).

## Decision

The author chose the open parts on 2026-11-12, through the coordinator. Each is marked with its
question (Q1–Q13), listed under the author's choices with the options not taken.

### Where the code lives (Q1)

- **A new library crate, `oso-syntax`,** holds everything that touches tree-sitter: the
  grammars, the queries, the parsers and the highlighting loop. It depends on `oso-core` and the
  tree-sitter crates, and only the tip crate depends on it:

  ```
  oso
  oso-render  oso-syntax
  oso-core    oso-platform
  ```

  It is `#![forbid(unsafe_code)]`: tree-sitter's Rust API is safe, and the C stays inside the
  dependencies. `deny.toml` lets only `oso-syntax` depend on the tree-sitter crates (`wrappers`),
  as it does for wgpu and winit.
- **`oso-core` gains a public module, `syntax`,** with no tree-sitter in it:
  - `Highlight`, a closed enum of what a range of text is (`Comment`, `String`, `Keyword`, …,
    see Faces). It names no colour.
  - `Highlights`, a `SumTree` of runs, each a length in bytes and an `Option<Highlight>`. It
    follows edits, is O(1) to clone and goes between threads. It answers "the runs from offset
    a to b" with one seek, O(log n), and a step per run.
- **`Buffer` holds the buffer's `Highlights`,** or none, beside its display map, and an edit
  maps them on the same clones, committed together (ADR 0053). Undo, redo and a reload go
  through the same path.
- **The tip crate runs the worker** (`app/syntax.rs`), as it runs isearch's and the wraps'.
- `oso-render` maps a `Highlight` to a `Face` (`Face::of_highlight`). `Face` stays in
  `oso-render`.

### Languages and modes (Q7)

- **A language is a major mode.** `Mode` gains `Rust`, `Markdown` and `Toml`, which name the
  grammar; `Fundamental` has none. These are the names ADR 0108's per-mode tables will use.
- **The mode comes from the file's name only:** `.rs`; `.md` and `.markdown`; `.toml` and the
  names `Cargo.lock`. Extensions are compared ignoring ASCII case. Nothing in the file is read
  to decide: no first line, no shebang, no modeline, no `-*- mode -*-` (T1). A buffer with no
  file, and an internal buffer, are `Fundamental`.
- The mode is set when a file is visited and when `C-x C-w` gives the buffer a new name, as
  Emacs's `set-auto-mode` does.
- **Commands set it by hand:** `rust-mode`, `markdown-mode`, `toml-mode` and
  `fundamental-mode`, reach `Look`. A configured table of names to modes waits for the
  configuration's per-mode tables.
- **`global-font-lock-mode`** turns highlighting off and on for every buffer, with the
  minor-mode argument of [ADR 0097](0097-visual-aids.md), reach `Look`. It is on by default.

### Where parsing runs

- **One worker thread for every buffer,** spawned at the first job, in the tip crate. The
  worker owns each buffer's parse state (`oso_syntax::Document`): its parser, its tree, and the
  `Highlights` it last made. The UI thread never calls into tree-sitter.
- **A job** carries the buffer's id, its mode, its revision, an O(1) snapshot of its rope, the
  splices since the last job (start, old length, new length, as `OffsetEdit`), and the byte
  ranges its windows show. The worker turns the splices into tree-sitter's `InputEdit`s, with
  their points from the old and new snapshots (`Rope::offset_to_point`, already in tree-sitter's
  units), and calls `Tree::edit` for each. Splices are a few words each, so the UI's part is
  O(1) per edit.
- **The UI sends a job after each command that changed a highlighted buffer,** and when a
  window shows a part of it not yet highlighted. The newest job for a buffer replaces one still
  waiting, and its splices join.
- **The parse reads the rope by chunks** through `parse_with_options`, from the snapshot, with
  no copy of the text.
- **An edit does not cancel a running parse.** The parse goes on to its end, and the next job
  starts from its tree: cancelling on every key would never finish a large file while someone
  types. A parse stops for good only when its buffer is killed, changes mode, may now hold
  secrets, or passes its time limit (Q6).
- **The worker works in slices** of at most 20 ms, through the progress callback: a parser
  stopped by it resumes on the next call. Between slices it takes the jobs for buffers a window
  shows before the others, so a large file in the background does not delay typing in the one
  shown.
- **While a file is loading,** the worker parses the first snapshot the loader gives, so the
  first screen has colour soon, and parses again from that tree when the load completes. The
  snapshots in between are skipped.
- **The report** carries the revision it was made for and a snapshot of the new `Highlights`.
  The UI maps it through the splices made since that revision, kept on the buffer in a short
  log, O(log n) each, and swaps it in. A report whose revision has left the log (more than
  64 splices, or a reload) is dropped, and the next job covers it.
- **A frame is asked for** only when a window shows the buffer and the report changed runs in
  its view.
- The worker catches a panic in a job, as the wraps' worker does: that buffer's highlights stop,
  and the others go on.

### Highlighting and the highlight map

- **The worker highlights ranges, not the whole buffer each time.** After a parse, it runs the
  highlight query, with `QueryCursor::set_byte_range`, over:
  1. the ranges the buffer's windows show, extended to whole lines;
  2. the ranges the parse changed (`Tree::changed_ranges`) and the splices' ranges;
  3. once after the first parse, the rest of the buffer, in slices, reporting at most every
     50 ms as the loader does.

  It splices the runs it found into its `Highlights` and reports.
- **Each query has a match limit** (`set_match_limit`) and a time limit per slice (the progress
  callback). A query that hits one leaves its range unhighlighted and goes on.
- **Until a report arrives, the UI's `Highlights` follow the edit:** text inserted inside a run,
  or at its end on the same line, takes that run's highlight, as Emacs's text is rear-sticky; at a
  line's start it takes none. So a character typed in a string is drawn as a string at once, and
  the next report, usually the next frame, puts right what the guess got wrong.
- **Markdown** has two grammars: the block grammar parses the whole buffer, and the inline
  grammar parses each `inline` node on its own, as its injection query says, only in the ranges
  the worker highlights. Inline trees are not kept: they are small, and parsing one again costs
  less than tracking it through edits. Fenced code is not highlighted yet; a later step injects `rust` and `toml` fences (Q9).
- **Injections** other than Markdown's inline grammar wait.

### What a frame costs

- **Colour is applied after shaping (Q4).** A syntax face sets a foreground only. Shaped rows
  are what they are today, with `Face::Text` where the text is plain, and the highlight is
  looked up when glyph instances are built, each frame, as the theme's colours already are:
  for each row in view, one seek in `Highlights` at `ScreenRow::start` and a walk over the runs
  to its end, beside the glyphs. A glyph of `Face::Text` takes its cluster's highlight face; an
  escape, a box, a whitespace mark, the preedit and the glyph under the cursor keep theirs. A
  ligature takes its first cluster's.
- **Nothing is shaped again** when highlights change, and the shaping cache needs no new key or
  invalidation. A report costs one frame of instance building.
- **The expected cost per frame** is one seek per row in view and one step per run: tens of
  microseconds for a full screen of code. **Per key**, the UI thread maps `Highlights` through
  the edit (O(log n)) and posts a job (O(1)). Both are measured in the first step that draws
  colour, with a typing scenario on a large Rust file added to `just perf` (its thresholds are a
  guard file, so the author sets them).
- **The worker's cost per key** is an incremental parse and a query over the changed lines. It
  runs while the frame is drawn and does not count in the key's latency, but it decides how many
  frames show the guess.

### Large files and long lines (Q6)

- **A buffer larger than 8 MiB is not parsed,** and stays plain. A tree costs memory in
  proportion to the text, which step 1 measures; 8 MiB is far above any source file and below
  where that memory matters. The limit becomes a setting with the per-mode tables.
  - Erratum (2026-11-12, as built in step 1, the author's choice): the limit is **2 MiB**. A
    tree costs 35 to 60 times its text, not the little this assumed; see As built.
- **Time limits:** a first parse that passes 5 s of the worker's time, or a later one that passes
  1 s, stops highlighting in that buffer, says so in the echo area and `*Messages*` (with no text
  of the buffer), and the buffer stays plain until its mode is set again.
- **Long lines need no rule of their own.** tree-sitter works in bytes, its repetitions are
  balanced trees, and the queries run over byte ranges: a 10 MB line under the size limit costs
  a seek, not a scan, both on the worker and per frame, where only the row's visible part is
  read. The worker highlights a long line's shown part first, by byte range, not the whole line.
- Our own walks of a tree use `TreeCursor`, never recursion, so a deep tree cannot overflow a
  stack.

### Secret buffers (Q2)

- **A buffer that may hold secrets is not parsed.** No job is sent while `Buffer::is_secret`
  holds, which includes a buffer the loader has not judged yet. When a buffer becomes secret, by
  a yank, its `Highlights` are dropped and the worker drops its document.
- So no tree, no timing and no highlight exists for it, and nothing needs withholding. Such a
  buffer shows plain text. Among the default secret patterns ([ADR 0028](0028-secrets-handling.md)),
  Cargo's `credentials.toml` is the one that would have had colour.
- In every buffer, the worker's messages and logs carry no buffer text, and a parse's time is
  not reported to `take-timings`.

### Faces and themes (Q5, Q10)

- **The highlight classes:**

  | `Highlight` | Token | Captures it takes (prefix match) |
  |---|---|---|
  | `Comment` | `syntax.comment` | `comment`, `comment.documentation` |
  | `String` | `syntax.string` | `string`, `character`, `escape`, `text.literal` |
  | `Keyword` | `syntax.keyword` | `keyword`, `label` |
  | `Type` | `syntax.type` | `type`, `constructor` |
  | `Function` | `syntax.function` | `function`, `function.method` |
  | `Constant` | `syntax.constant` | `constant`, `number`, `boolean` |
  | `Property` | `syntax.property` | `property`, TOML keys |
  | `Macro` | `syntax.macro` | `function.macro`, `attribute` |
  | `Heading` | `syntax.heading` | `text.title` |
  | `Link` | `syntax.link` | `text.uri`, `text.reference` |
  | `Markup` | `syntax.markup` | `text.emphasis`, `text.strong`, list and quote markers |

  Everything else (variables, punctuation, operators) is plain text. A theme maps each token to a
  palette name, and several may share one: the default look stays subdued, as
  [ADR 0027](0027-theme.md) asks.
- **`Face` gains one variant per class,** and `theme.rs` maps each in all three themes.
- **Contrast (Q10):** every syntax face is text that is read, so it meets 7:1 on every fill a
  line of text sits on: the background, the current line, the flash, the region and both search
  fills, as `Text` does today. It keeps the OKLab distances that hold for `Text`: `question` from
  every syntax face, so a question's words never look like buffer text, and the whitespace and
  wrap marks from every syntax face.
- The colours are picked as ADR 0089 picks them: sample frames for each theme, with ratios, and
  the author's choice among candidates.

### Queries and trust (Q8)

- **The queries are oso's own files,** in `crates/oso-syntax/queries/<language>/highlights.scm`,
  written from the grammars' upstream queries (MIT, credited in each file's header) and cut to
  the captures in the table above. They are compiled into the binary (`include_str!`).
- **A query is compiled on the worker,** the first time its language is needed, and kept for the
  session, so it never happens at startup or on the UI thread. Step 1 measures how long the
  Rust query takes to compile.
- **Nothing is loaded at run time:** no grammar, no query, from the project, the configuration
  directory or anywhere else. A grammar is code, and a query decides how much work a file costs,
  so both are part of the binary that was reviewed (T1, T8). The project-trust model
  ([ADR 0009](0009-project-trust.md)) does not change: highlighting a file opened in restricted
  mode runs only oso's own code and its pinned grammars.
- Predicates are limited to what tree-sitter's Rust crate evaluates itself: `#eq?`, `#match?`
  (with `regex`, linear time) and `#any-of?`. A query using any other fails a unit test.

### Running C parsers on untrusted files (Q11)

- **They run in the oso process,** as ADR 0012 accepted. What limits the risk:
  - few grammars, exact versions, and each `build.rs` and scanner diff read before a bump;
  - the size and time limits above, and no parsing of secret buffers;
  - the worker thread, so a slow parse never holds the UI;
  - fuzzing (Q12).
- **What it cannot limit:** a memory error in a grammar's C, or in the runtime, ends oso at once.
  A signal is not a panic, so the emergency save does not run, and unsaved text in every buffer
  is lost. The external scanners, hand-written C, are where that is most likely: Markdown's is the
  largest (62.6 KB).

### Fuzzing (Q12)

- **Property tests in `just ci`,** under the 30 s budget ([ADR 0050](0050-test-quality-checks.md)):
  for each language, random edits applied incrementally give the same highlights as a fresh parse
  of the result; random bytes parse and highlight within the time limits and never panic.
- **Longer runs of the same properties weekly,** in the scheduled workflow that already runs the
  mutation tests.
- `cargo-fuzz` (nightly, `libfuzzer-sys`) is a later step with its own dependency approval.

### How later features use this

- **Comment-aware `C-e`** ([ADR 0030](0030-m2-editing-commands.md)) reads `Highlights` on the UI
  thread: on the cursor's line, it also stops before a trailing run of `Comment` that ends the
  line (before blanks). No tree is needed, and a comment marker inside a string is a string's
  run, not a comment's. Where highlights are stale or absent, `C-e` does what it does today.
- **Indentation** needs the tree, and right after an edit, as `RET` indents a line the worker has
  not parsed yet. It gets its own ADR. This one leaves room for it: a report can carry an O(1)
  copy of the tree (`Tree::clone` bumps a reference count) with its revision.
- Structural motion (`C-M-f`, expand region) and a later language server's ranges can read the
  same tree.

### The local Windows check (Q13)

- The pre-push Windows check sets a stub C compiler and archiver for the MSVC target, which
  write empty files, as a `just` recipe (`just clippy-windows`). It still checks every Rust
  line for Windows; the C is compiled and linked by CI's Windows job, which also runs the tests.

## Changes to accepted ADRs

- **ADR 0012:** "Parse on a background thread with cancellation and timeouts" now reads: a
  parse is stopped by the progress callback, in slices, for a killed buffer, a change of mode,
  a secret verdict or a time limit, and not by a newer edit. "Fuzz the highlighting path"
  holds as property tests now and `cargo-fuzz` later (Q12).
- **ADR 0004 and ADR 0038:** a fourth library crate, `oso-syntax`, between the tip crate and
  `oso-core`. `guardrails/layers.txt` gains it in `[workspace]`, and `syntax` joins
  `[oso-core]` on the line of `display_map`.
- **ADR 0061:** `Face` stays in `oso-render`. `oso-core` names a `Highlight`, which is no face.
- **ADR 0089:** "Syntax faces join the table, and its contrast rules" is the table and the rules
  above.
- **ADR 0030:** comment-aware `C-e` reads the highlight map, not the tree.

## Consequences

- Rust, Markdown and TOML have colour without a key ever waiting for a parse, and changing a
  highlight shapes nothing again.
- A character typed is drawn first with a guess, the run it extends, and corrected by the next
  report. Where the guess is wrong (closing a string, typing `//`), the colour changes a frame or
  two later.
- About 11 MB of C is compiled once per clean build, and six crates join `Cargo.lock`.
- A crash in a grammar's C loses unsaved text, which no measure here prevents (Q11).
- Secret buffers, and files over the size limit, show plain text.
- Highlights hold one `Option<Highlight>` per run of text, and the worker a tree per buffer.
- New public API: `oso_core::syntax` (`Highlight`, `Highlights`), `Mode`'s new variants, and the
  `oso-syntax` crate. The public API snapshots change.
- The pre-push Windows check needs the stub compiler, through a recipe (a `Justfile` change).

## The author's choices

The author chose these on 2026-11-12, through the coordinator, each the option written into the
decision above. The options not taken are listed with their trade-offs.

- **Q1. Where the tree-sitter code lives:** a new crate, `oso-syntax`. Not taken: the tip crate
  (library logic in the crate that changes most, and it links the C), and `oso-core` (the core
  would link 11 MB of C, which its rule against `unsafe` was meant to keep out).
- **Q2. Secret buffers:** never parsed. Not taken: parsed, with highlights shown on screen only
  and kept out of every report (colour in the few secret files with a grammar, more places to
  check).
- **Q3. TOML:** `tree-sitter-toml-ng`, so one engine, one query format and one tree for `C-e` and
  indentation. Not taken: `toml_parser`'s lexer, already in the tree, with no new C but a second
  highlighting path, tokens and no tree.
- **Q4. Syntax faces:** colour only, applied after shaping. Not taken: bold and italic (needs
  those fonts loaded, and a face change would split shaping and reshape rows as highlights
  arrive).
- **Q5. Highlight classes:** the eleven in the table. Not taken: five (Markdown would show only
  headings), and about 25 as Helix's and Neovim's capture names (many more colours to check at
  7:1 in three themes).
- **Q6. Limits:** 8 MiB; 5 s for a first parse, 1 s for a later one. Not taken: 32 MiB (more
  memory per tree), and no size limit (memory unbounded by size).
- **Q7. Modes:** `Mode` gains the languages, the file's name decides, and `rust-mode`,
  `markdown-mode`, `toml-mode` and `fundamental-mode` set it by hand. Not taken: a language field
  apart from `Mode` with no commands, and a shebang or first-line check (it reads the file to
  decide, and no grammar here needs it).
- **Q8. Queries:** oso's own `.scm` files, compiled in, nothing loaded at run time. Not taken: the
  grammar crates' bundled queries as they are (capture names differ by crate, and a bump changes
  the look unreviewed), and queries from the configuration directory (a query decides how much
  work a file costs, and it is a second file format to keep).
- **Q9. Markdown's fenced code:** plain for now, with the inline grammar in. Highlighting fences
  tagged `rust` and `toml` with those grammars is a later step, once the base is settled: the
  fence's info string names a mode, and its content is parsed with that grammar over its range
  (`set_included_ranges`), as the inline grammar is, its runs going into the same `Highlights`.
  Not taken: the block grammar only (no links, emphasis or code spans).
- **Q10. Contrast:** 7:1 on every fill a line sits on, as `Text`. Not taken: 4.5:1 for comments
  (the modus themes' faint comments meet 7:1, so the look does not need it).
- **Q11. The C parsers' process:** in the oso process, as ADR 0012 accepted, with the limits
  above. Not taken: a helper process (`oso parse`) now, where a crash costs only colour but each
  edit crosses a pipe and the helper keeps its own copy of the text, and a helper process only if
  fuzzing finds a crash.
- **Q12. Fuzzing:** property tests in `just ci`, longer runs weekly, `cargo-fuzz` later. Not
  taken: `cargo-fuzz` now, with a nightly toolchain and `libfuzzer-sys` before the first
  highlight.
- **Q13. The pre-push Windows check:** a stub C compiler and archiver for the MSVC target, in a
  `just clippy-windows` recipe. Not taken: `x86_64-pc-windows-gnu` with Homebrew's `mingw-w64`
  (compiles the C, but another target than CI's and a large install), and MSVC's headers through
  `xwin` (a download under Microsoft's licence).
- **The dependencies** are approved as researched: `tree-sitter =0.27.1`, `tree-sitter-rust
  =0.24.2`, `tree-sitter-md =0.5.4` and `tree-sitter-toml-ng =0.7.0`, added in a commit of their
  own with `just deny` and `just audit`.

## As built

### Step 1: parsing

- **The size limit is 2 MiB, not 8 MiB** (the author's choice, 2026-11-12, through the
  coordinator, after comparing other editors' limits). Measured on the author's Mac, a release
  build, with one character typed in the middle of the file; the peak includes the rope, about
  19 MB at 8 MB:

  | Input | First parse | After one key | Peak memory |
  |---|---|---|---|
  | Rust, 1 MiB | 50 ms | 1.5 ms | 46 MB |
  | Rust, 2 MiB | 132 ms | 5.5 ms | 88 MB |
  | Rust, 8 MB | 380 ms | 15 ms | 311 MB |
  | Markdown, 2 MiB (oso's docs) | 545 ms | 145 ms | 133 MB |
  | Markdown, 8 MB | 875 ms | 150 ms | 530 MB |
  | TOML, 2 MiB | 252 ms | 47 ms | 117 MB |
  | TOML, 8 MB | 470 ms | 125 ms | 424 MB |

  At 8 MiB one buffer could take about 0.5 GB; at 2 MiB, about 130 MB at most. A large Markdown
  or TOML file reparses slowly after each key, which shows as highlights a few frames late, not
  as key latency.
- **Compiling the queries** takes 8 ms for Rust's upstream query, 0.5 ms and 1 ms for
  Markdown's block and inline ones, and 0.05 ms for TOML's, on the worker at first use.
- **A clean build compiles the C** in about 6 s of wall time (22 s of CPU) on the author's Mac.
- **The rope needed no new API:** `Rope::chunks_in_range` feeds the parser its chunks.
- **Points need not be exact for the tree:** tree-sitter takes the points of reused nodes from
  where its lexer reads, so an edit's wrong point leaves the tree right. The property test
  catches a wrong byte; a unit test checks the points, which `Tree::changed_ranges` reports.
- **The agent channel** counted every mode but `Fundamental` as one with keys of its own, so it
  would have refused every Rust, Markdown or TOML buffer. It now matches every mode with no
  wildcard, so a new mode says whether it has keys of its own.
- **An incremental parse need not equal a fresh one.** tree-sitter does not promise it once
  error recovery is involved: on texts of 12 KB, the review found a few trees in every 300 rounds
  that differed, all with errors. So the property tests compare a document with the same history
  parsed from one contiguous string, which always matched, and with a fresh parse only where
  neither tree holds an error. Their texts span several of the rope's 1008-byte chunks, and edits
  also come while a parse is stopped part way.
- **Markdown's inline grammar overflows a thread's stack on deeply nested inline links.** The
  review found that about 96 KB of such text in one inline node overflows a 2 MiB stack, and the
  process aborts: no panic, so no emergency save. An 8 MiB stack fails at 1 MiB of it, and 2 MiB
  needed 64 MiB. The size limit does not prevent it, and the Context above was wrong to say the
  runtime never recurses. Rust's and TOML's grammars and Markdown's block grammar parse 2 MiB of
  nesting with no trouble. So:
  - step 3 caps the text of one inline node far below what overflows, measured, and leaves a
    longer one plain;
  - step 4 runs the worker on a thread of its own with a stack size set from those measurements;
  - such text is one of the cases the property tests and their weekly runs feed every grammar.
- **One slice can overshoot its time:** tree-sitter asks the progress callback only every so
  often, and the review measured single slices of 178 ms (Rust), 522 ms (Markdown inline) and
  577 ms (TOML) on crafted nesting. That holds no key, as it is the worker's time, but it delays
  other buffers' jobs by as much.
- **Edits that wait for a stopped parse** are kept in full, one entry a batch, with no bound.
  Step 2, which gives the worker its jobs, merges them.

## Alternatives considered

- **Highlighting the whole buffer after every parse:** simplest, but a query over a large file
  per key; the changed ranges and the view are enough.
- **Highlighting only the view, each frame, on the UI thread** (as Zed and Neovim run queries
  there): no map to keep, but it runs tree-sitter's C on the UI thread, which ADR 0012 rules out.
- **Cancelling a parse on every newer edit:** fresher results on small files, but a large file
  never finishes while someone types.
- **Waiting a few milliseconds for the worker before drawing a key's frame:** no guessed colour,
  but the UI thread would wait on parsing.
- **Inserted text plain until the report:** no wrong guess, but a flicker inside every string
  and comment as one types.
- **`tree-sitter-highlight`:** see the dependencies.
- **tree-sitter-md's `MarkdownParser`:** see the dependencies.
- **A combined inline tree over all of Markdown's inline ranges:** incremental, but an unclosed
  `*` in one paragraph would reach into the next.
- **Pure-Rust parsers per language** (`pulldown-cmark`, `syn`): not incremental, and `syn` has no
  error recovery, so a half-typed line loses the file's colour.
- **Wasm grammars and syntect:** rejected in ADR 0012.
