# 0147. Highlighting Go, TypeScript and JavaScript

- Status: Accepted
- Date: 2026-11-17

## Context

[ADR 0111](0111-tree-sitter-highlighting.md) highlights Rust, Markdown and TOML with
tree-sitter, on two worker threads, within a size limit and time limits, and never in a buffer
that may hold secrets. [ADR 0138](0138-language-servers.md) added the modes Go, TypeScript, TSX
and JavaScript (with JSX), chosen by the file's name, so that they can have a language server.
They have no highlighting, and ADR 0138 lists "highlighting for the new modes" as a follow-up.
The author writes Go and React (TypeScript, TSX and JSX) every day.

What is already in place, and does not change:

- **The worker and its schedule** know nothing of a language: a buffer goes to the worker when
  `Buffer::can_highlight` holds, shown buffers go first, and every parse and query runs in
  slices of 20 ms under ADR 0111's limits (2 MiB, 5 s for a first parse and 1 s for a later one,
  2 s of highlighting, 256 matches in progress, 1 KiB read by a predicate, 16 KiB pieces).
- **The highlight classes** are eleven (`Comment`, `String`, `Keyword`, `Type`, `Function`,
  `Constant`, `Property`, `Macro`, `Heading`, `Link`, `Markup`), each with a face and colours in
  every theme, checked for contrast.
- **Markdown's fenced code** is parsed with the grammar its info string names, among those
  compiled in (ADR 0111, step 7), and a language server's hover text is Markdown, highlighted
  as its mode says (ADR 0138, step 10). gopls sends its code in fences tagged `go`, and
  typescript-language-server in fences tagged `typescript`.

### The dependencies

Researched on 2026-11-17 with the `add-dependency` steps, in a scratch copy of the workspace
(`cargo tree`, `cargo deny check`, `cargo audit`; nothing built there). The C was compiled on its
own, outside the workspace, to measure it.

| Crate | Version | Licence | What it builds | Notes |
|---|---|---|---|---|
| `tree-sitter-go` | `=0.25.0` (2025-08-29) | MIT | `parser.c` 1.57 MB, no external scanner | tree-sitter org; ABI 15 |
| `tree-sitter-javascript` | `=0.25.0` (2025-09-01) | MIT | `parser.c` 2.86 MB, `scanner.c` 10.6 KB | tree-sitter org; ABI 15; JSX is part of the grammar |
| `tree-sitter-typescript` | `=0.23.2` (2024-11-11) | MIT | two grammars, TypeScript and TSX, each a `parser.c` of about 8.7 MB, and one 10 KB scanner they share | tree-sitter org; ABI 14; the newest release, the repository's last commit 2025-01 |

- **Three crates are new to `Cargo.lock`,** and nothing else: each needs only `cc` and
  `tree-sitter-language`, already locked. No duplicate versions. `cargo deny check` passes and
  `cargo audit` reports nothing new. RustSec lists nothing against them.
- **`unsafe`:** one `LanguageFn::from_raw` per grammar. **`build.rs`:** `cc` compiles the
  `parser.c` and `scanner.c` files as C11 (Go and JavaScript add `/utf-8` on MSVC;
  TypeScript's does not, and its C is all ASCII). Both scanners are stateless: they allocate
  nothing, serialize nothing, and each loop ends at the end of the text.
- **Cost:** with `clang -O2`, each file compiles in under half a second. The objects hold about
  3.5 MB of parse tables (Go 0.22 MB, JavaScript 0.41 MB, TypeScript 1.41 MB, TSX 1.44 MB;
  Rust's is 1.1 MB), so the release binary grows from about 18.7 MB to about 22 MB.
- **Windows:** `just clippy-windows`'s stub compiler serves any crate that builds C with `cc`,
  so the recipe does not change. CI's Windows job compiles the C with MSVC.
- **Not used:** Helix's queries (MPL-2.0) and nvim-treesitter's (Apache-2.0), which are richer
  than upstream's but under other licences, and need changes for oso's classes all the same.

## Decision

The author chose every recommendation on 2026-11-17, through the coordinator. Each is marked
with its question (Q1–Q8).

### Grammars and modes (Q1, Q2)

- **The three crates, at the versions above,** each an exact version, with each `build.rs` and
  scanner diff read before a bump (T2), and listed in `deny.toml` as used only by `oso-syntax`.
- **`Language` gains `Go`, `JavaScript`, `TypeScript` and `Tsx`.** The mode picks the grammar,
  as before: `Mode::Go` is Go; `Mode::JavaScript` (`.js`, `.mjs`, `.cjs`, `.jsx`) is
  JavaScript, whose grammar parses JSX; `Mode::TypeScript` (`.ts`, `.mts`, `.cts`) is
  TypeScript, and `Mode::Tsx` (`.tsx`) is TSX. TypeScript and TSX are two grammars, as TSX
  cannot have TypeScript's `<T>value` casts. LaTeX and Swift stay without one.
- **The binary grows by about 3.5 MB** (Q2), the price of four grammars.

### Queries (Q3, Q4, Q6)

- **oso's own files, as ADR 0111 Q8 has them:** written from each grammar's upstream query
  (MIT, credited in the file's header), cut to the eleven classes, with oso's own patterns where
  upstream's are thin, and only `#eq?`, `#match?` and `#any-of?`. Upstream JavaScript's
  `#is-not? local` goes. Emacs's go-ts-mode and typescript-ts-mode were read for how things
  look; nothing is copied from them.
- **JavaScript, TypeScript and TSX share parts** (Q3): an `ecma` part every one of the three
  grammars can compile, a `jsx` part and a `typescript` part. JavaScript's query is ecma and jsx,
  TypeScript's is ecma and typescript, TSX's all three. A unit test compiles each against its
  grammar. Compared by their node types, JavaScript's grammar has two kinds of node TSX's
  lacks (`field_definition` and `using_declaration`), which the ecma part leaves out, and
  TypeScript's has none of JSX's.
- **What each class takes:**
  - Go: keywords and labels are `Keyword`; types are `Type`; functions and methods, where
    declared and where called, builtins too, are `Function`; names declared with `const`,
    numbers, `true`, `false`, `nil` and `iota` are `Constant`; fields, and a struct literal's
    keys, are `Property`; strings, raw strings and runes are `String`. Package names and
    variables stay plain.
  - JavaScript and TypeScript: keywords, `this` and `super` are `Keyword`; types, and
    capitalised names, are `Type`; functions and methods, declared and called, are `Function`;
    properties are `Property`; strings, template strings and regular expressions are `String`,
    while the code in a template's `${…}` is highlighted as code; numbers, `true`, `false`,
    `null`, `undefined` and all-caps names are `Constant`; decorators are `Macro`, as Rust's
    attributes are.
  - **JSX** (Q4): a tag's name is `Function`, near Emacs's tsx-ts-mode, which draws tags with
    the face of a function call; an attribute's name is `Property`; a tag's text stays plain. No
    class is added.
- **No injections** (Q6): a tagged template's text stays a `String`, a regular expression one
  `String`, a JSDoc comment one `Comment`. ADR 0111's "Injections other than Markdown's inline
  grammar and its fenced code wait" holds.

### Fenced code (Q5)

- **More info strings name a grammar,** by the same rule (the first word, ignoring ASCII case,
  at most 16 bytes): `go` and `golang` are Go; `js`, `javascript`, `jsx`, `mjs` and `cjs` are
  JavaScript; `ts`, `typescript`, `mts` and `cts` are TypeScript; `tsx` is TSX. So a language
  server's hover shows its code in colour.
- **A fence's grammar's query is compiled when a fence of it is first met** on a thread, not
  with Markdown's, so a Markdown file compiles only the queries of the fences it has. This now
  holds for Rust's and TOML's too (below). The 16 KiB cap on a fence does not change.

### Long lines (Q9)

Asked after step 2's review, which found that 2 MiB of minified JavaScript, an ordinary file,
takes about 200 MB highlighted, against the "about 130 MB at most" ADR 0111 chose its size limit
on. The author chose (2026-11-17, through the coordinator):

- **A text with a line longer than 16 KiB is not parsed, in every grammar,** and stays plain, as
  one past the size limit does. Such lines are minified or generated code. The echo area says
  "Highlighting stopped in NAME: a line is longer than 16 KiB", and, as after any failure,
  setting the mode again gives it another try.
- 16 KiB is a piece's size (ADR 0111), so a highlighted line is never cut into pieces. Measured
  on what was at hand (As built), source lines stay far below it, and minified bundles' run to
  hundreds of KB. VS Code stops tokenizing a line at 20,000 characters
  (`editor.maxTokenizationLineLength`); bytes are what the worker reads, so the limit counts
  bytes.
- Not taken: accepting 200 MB for a minified buffer and amending ADR 0111's figure; a lower size
  limit for JavaScript, TypeScript and TSX only, which would leave long lines in other grammars.

### What does not change

- ADR 0111's other limits, the secret rules (a buffer that may hold secrets, or is not judged yet, is
  never parsed, whatever its mode), the worker's schedule and on-screen-first order.
- Comment-aware `C-e` reads the highlights' `Comment` runs, so it works in the new modes as
  they are highlighted. Indentation ([ADR 0115](0115-indentation.md)) does not change.

### Tests and measuring (Q7)

- For each grammar: its captures, its query compiled with only the allowed predicates, the
  property tests (an incremental parse is the same history fed one string; highlights in slices
  are highlights in one call), random text and crafted nesting on a small stack, and its fences.
- **`just perf` does not change** (Q7): what a key costs on the UI thread, mapping the
  highlights and posting a job, and a frame's seek per row, do not depend on the language, and
  its `syntax` phase gates them on Rust. The worker's time is measured with oso-syntax's
  `syntax_perf` example and written under As built.

### Steps (Q8)

1. This ADR, and Go.
2. JavaScript, TypeScript and TSX, with the shared query parts and their fences.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it. The status of each ADR named
gains `amended by 0147`.

- [ADR 0111](0111-tree-sitter-highlighting.md), "Fenced code whose info string names Rust or
  TOML is parsed with that grammar the same way" and step 7's info strings: Go, JavaScript,
  TypeScript and TSX too, by the names above.
- ADR 0111, step 7: "A Markdown document now compiles Rust's and TOML's queries too, at its
  first highlight on each thread": a fence's grammar's query is compiled when a fence of it is
  first met on the thread.
- ADR 0111, "A buffer larger than 8 MiB is not parsed" (2 MiB, by its erratum): nor is one
  with a line longer than 16 KiB (Q9). Its "at 2 MiB, about 130 MB at most" holds for source;
  minified text, which held about 200 MB, is no longer parsed.
- [ADR 0138](0138-language-servers.md), "They have no highlighting until their grammars come":
  Go has it from step 1, and JavaScript, TypeScript and TSX from step 2. LaTeX and Swift still
  have none.

## Consequences

- Go, TypeScript, TSX, JavaScript and JSX files are in colour, and so is their code in
  Markdown and in a language server's hover.
- About 22 MB more C is compiled once per clean build, twice what ADR 0111's grammars brought;
  the release binary grows by about 3.5 MB.
- Four more grammars' C runs in oso's process on untrusted files (ADR 0111 Q11). Go's has no
  external scanner, and the others' scanners keep no state; the limits and the fuzzing hold for
  them as for the first three.
- tree-sitter-typescript has had no release since 2024-11. A grammar that stops being kept
  stays at its pinned version until one is chosen to replace it.
- Follow-ups: highlighting for LaTeX and Swift; injections, such as CSS in a tagged template,
  which need grammars oso does not have.

## Alternatives considered

- **Helix's or nvim-treesitter's queries:** richer, but MPL-2.0 and Apache-2.0, and they still
  need changing for oso's eleven classes (Q3).
- **A file per language with no shared parts:** each of JavaScript's patterns three times,
  to be kept in step by hand (Q3).
- **A `Tag` class for JSX:** a new face and colours in three themes, each with contrast tests,
  for one kind of token (Q4). Tags as `Type`: reads like a component's name, but `div` is no
  type.
- **Only `go`, `js`, `ts`, `tsx` and `jsx` in fences** (Q5): typescript-language-server's hover
  says `typescript`, and `golang` and `javascript` are common.
- **Injections** in templates, regular expressions and JSDoc (Q6): each needs a grammar oso does
  not have, and ADR 0111 leaves injections for later.
- **A TSX phase in `just perf`**, reported or gated (Q7): the UI's cost does not depend on the
  language, and a new gate needs new thresholds.
- **An As built note in ADR 0111 instead of an ADR** (Q8): it adds dependencies and public API,
  which need an ADR, and ADR 0111 is long already. **One pull request, or three:** Go goes alone
  first, as it is the smallest and has no scanner, and the three that share queries go
  together.

## As built

### Step 1: Go

- `tree-sitter-go =0.25.0` was added on its own commit, with its `deny.toml` entry.
- **`Language::Go`** (public, in the API snapshot), `Mode::Go` → Go, and `Buffer::can_highlight`
  allows `Mode::Go`. `queries/go/highlights.scm` is written from upstream's, as above; a struct
  literal's keys are `Property` by an anchored pattern (`keyed_element . literal_element`), so a
  map literal's identifier keys are too. A call through `index_expression`
  (`f[int]()`) is a function.
- **Fences:** `go` and `golang`. The fence's query is compiled when `highlight_piece` widens a
  Markdown piece to a fence of that grammar, one query a call. The piece then ends as
  `Piece::Compiled` and is done again, unjudged, so a piece with fences of several new languages
  runs past its slice by one compile at most (the perf review of step 2): the review found that, timed as the piece's own work, a compile (Rust's
  takes 9.7 ms, and more on a slower machine or in a debug build) could halve pieces for
  nothing or fail the whole document as too slow. Each query is tried once a thread, so a piece
  ends so at most once a language. A fence whose query did not compile stays plain, as a fence of
  a language oso does not inject does; before, a broken Rust or TOML query, which only a broken
  build has, stopped the whole Markdown document as `Incompatible`. A Markdown document's first
  highlight now compiles only Markdown's own queries and the injection query.
- `Language::ALL` and `Kind::ALL` (crate-private) list every grammar and query once; the
  queries' table takes its length from them, and a test checks each query has a place of its
  own, so a language added in step 2 cannot be left out of the tests.
- **Long lines (Q9),** in `document/long_line.rs`: the worker reads the whole text once, before
  the first parse (1.6 ms for 2 MiB, release), and each trusted edit checks only the lines it
  touched, so joining two lines past the limit stops the document too. Splices that do not map
  the text have it read again. A line counts its bytes, without its line break;
  `Failure::LongLine` is new and public. Tests of crafted text on one line build their documents
  with the check lifted (`Document::with_long_lines`, tests only), so they still reach the
  grammar. Longest lines measured, in bytes:

  | Files | How many | Longest line, 99th percentile | Longest | Past 16 KiB |
  |---|---|---|---|---|
  | oso's Rust | 1,383 | 596 | 987 | none |
  | oso's Markdown docs | 157 | 550 | 1,276 | none |
  | npm's JavaScript | 1,118 | 402 | 1,714 | none |
  | TypeScript sources of installed tools | 1,563 | 633 | 32,334 | 4 tests with one long string each |
  | Go's standard library | 8,076 | 445 | 1,375,317 | a 17.6 KB test, and two generated tables |
  | Minified JavaScript | 15 | 954,533 | 1,604,877 | all |

- **Two lists say which modes are highlighted:** `oso-core`'s `Buffer::can_highlight`, which
  cannot see `oso-syntax`, and `Language::of`. A test in the tip crate checks they agree for
  every mode a file can have, so a mode moved in one alone does not stay plain unnoticed.
- **Measured** with `syntax_perf -- go`, release, on the author's Mac, on 2 MiB of Go's standard
  library (`net/http` and `go/types`), while other builds ran:

  | Input, 2 MiB | Parse | Parse and highlight | In 20 ms slices | After one key | Peak memory |
  |---|---|---|---|---|---|
  | Go | 108–184 ms | 162–269 ms | 9–13, the longest 20.0–21.1 ms | 3.4–5.2 ms | 71 MB |
  | Rust (oso-core, for comparison) | 95 ms | 182 ms | | | 87 MB |

  Peak memory is a process's highest resident set with one document highlighted to its end in
  slices, the rope and the queries too (`syntax_perf` holds two documents at once, so its peak
  counts two). Rust's matches ADR 0111's 88 MB.

  Go's query compiles in 1.5 ms (Rust's 9.7 ms, TOML's 0.09 ms). Crafted texts of 2 MB, a
  few characters repeated (`(`, `{`, `func(`, `[`, `a.`, `x{`, `if a{`, `map[`, `*`, `<-`),
  reach the 5 s limit of a first parse and stay plain, as the same texts do in Rust's grammar;
  `/*` and `` ` `` repeated parse in 0.15 s and 4.2 s and stop at the highlighting limits.
  `syntax_perf -- fenced` (oso-core's Rust, every 40 lines in a `rust` fence) now compiles
  its queries afresh for the run in slices, so the lazy compile falls in one: 625–648 ms in one
  call, 34 slices, the longest 20.1 ms, the same runs. 2 MiB of oso's ADRs as Markdown: 698 ms,
  38 slices, the longest 20.0 ms.
- **The property tests' seeds and pieces** gained Go's at the end of each list, so that the
  others keep their indices. The two seeds saved in `proptest-regressions` (#313) draw other
  cases now that the lists are longer; the bug they found keeps its own test
  (`a_fence_cut_short_at_its_end_is_highlighted_again`).
- **Tests:** Go's captures and its trailing comments as one run each; line breaks plain; a parse
  of Go; Go in the property tests' seeds and pieces (`func `, `go` as a fence's language), in
  the random and crafted texts (`func(` added to the atoms); `go` and `Golang` fences, and
  `gopher` staying plain; a fence's query compiled only at its first fence, and a piece that
  compiles one ending as `Compiled` and then done; and in `oso-core`,
  which modes can be highlighted, and that a Go buffer that may hold secrets cannot.

### Step 2: JavaScript, TypeScript and TSX

- `tree-sitter-javascript =0.25.0` and `tree-sitter-typescript =0.23.2` were added on their own
  commit, with their `deny.toml` entries.
- **`Language::JavaScript`, `Language::TypeScript` and `Language::Tsx`** (public), from
  `Mode::JavaScript`, `Mode::TypeScript` and `Mode::Tsx`, which `Buffer::can_highlight` now
  allows. LaTeX and Swift are the modes of ADR 0138 left without a grammar.
- **The queries** are three files, `queries/ecma`, `queries/jsx` and `queries/typescript`,
  joined at build time (`concat!` of `include_str!`): JavaScript's is jsx then ecma,
  TypeScript's typescript then ecma, TSX's typescript, jsx, then ecma. The specific parts go
  first, as of two patterns on one node the first wins: a component's tag (`<Button>`) is a
  function, not a capitalised name's type. Beyond the Decision's list:
  - a template string's backticks, text and escapes are `String`, and its `${` and `}` plain,
    so the code inside is drawn as code;
  - a decorator's `@` and name are `Macro`, its arguments as code;
  - a `#!` line is `Comment`; private names (`#items`) are properties, or functions where
    declared or called as methods;
  - TypeScript's predefined types (`string`, `number`, `void`, ...) are `Type`, an enum's
    members `Constant`, and a signature's name (in an interface, an abstract method, an
    overload) `Function`;
  - a tag's name in a member expression (`<Menu.Item>`) is a function in both parts.
- **Fences:** `js`, `javascript`, `jsx`, `mjs` and `cjs`; `ts`, `typescript`, `mts` and `cts`;
  `tsx`.
- **Measured** with `syntax_perf`, release, on the author's Mac, while other builds ran. Inputs
  were packages installed on that machine: npm's own JavaScript, TypeScript sources from
  another tool's packages, and its one `.tsx` file (12 KB) repeated:

  | Input, 2 MiB | Parse | Parse and highlight | In 20 ms slices | After one key | Peak memory |
  |---|---|---|---|---|---|
  | JavaScript | 111 ms | 191 ms | 9, the longest 20.0 ms | 8.0 ms | 148 MB |
  | TypeScript | 82 ms | 154 ms | 9, the longest 20.0 ms | 5.0 ms | 130 MB |
  | TSX (the `.tsx` file repeated) | 62 ms | 121 ms | 6, the longest 20.0 ms | 1.1 ms | 97 MB |
  | TSX (the TypeScript sources) | 103 ms | 186 ms | 9, the longest 20.0 ms | 25 ms | 132 MB |
  | Minified JavaScript, 1.9 MB | 231 ms | 478 ms | 26, the longest 20.2 ms | 22 ms | 400 MB |

  **A minified file is the costly case:** its tokens are dense, so its tree, and its 600,000
  runs, take about three times the memory of source at the same size. 2 MiB of it is the most
  memory one highlighted buffer takes in any grammar; the size limit bounds it.
- **Compiling the queries** takes 7.6–12 ms for JavaScript's, 13–18 ms for TypeScript's and
  17–21 ms for TSX's (Rust's 6.7–16 ms in the same runs), about a slice. A buffer's own query is
  compiled before its first parse, as in ADR 0111; a fence's ends its piece unjudged (step 1),
  so neither is counted against a piece.
- **Tests:** what the three share drawn alike in each (`assert_ecma`), JSX's tags and
  attributes, TypeScript's types, keywords, enums, signatures and decorators, a cast in
  TypeScript, a generic component in TSX; comments as one run each; line breaks plain; a
  parse of each; their fences, and other words (`node`, `react`) staying plain; a seed of each
  in the property tests, with `tsx`, a tag and `${` among the pieces; and the three in the
  random and crafted texts (`${` and a backtick, a tag, an arrow function added to the atoms).
  oso-syntax's tests take about 4 s now, from about 1 s.
