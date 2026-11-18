# 0157. Highlighting HTML, CSS, JSON and YAML

- Status: Accepted
- Date: 2026-11-18

## Context

[ADR 0111](0111-tree-sitter-highlighting.md) highlights Rust, Markdown and TOML with tree-sitter,
on two worker threads, within a size limit and time limits, and never in a buffer that may hold
secrets. [ADR 0147](0147-highlighting-go-typescript-and-javascript.md) added Go, JavaScript,
TypeScript and TSX the same way. An `.html` file opens in Fundamental mode, with no colour, and
so do CSS, JSON and YAML files, which the author edits beside them.

What is already in place, and does not change: the worker and its schedule, the limits (2 MiB,
5 s for a first parse and 1 s for a later one, 2 s of highlighting, 256 matches in progress,
1 KiB read by a predicate, 16 KiB pieces, no line longer than 16 KiB), the eleven highlight
classes and their faces, and the secret rules.

Markdown is the only language whose tree holds text another grammar parses: its inline text and
its fenced code (ADR 0111, step 7). A piece widens to each fence of a language oso knows, at most
16 KiB, parses its content with that grammar, and paints it over the block grammar's captures; a
fence's query is compiled at its first fence (ADR 0147). HTML holds code the same way, in its
`<script>` and `<style>` elements.

### The dependencies

Researched on 2026-11-18 with the `add-dependency` steps, in a scratch copy of the workspace
(`cargo tree`, `cargo deny check`, `cargo audit`; nothing built there). The C was compiled on its
own, outside the workspace, to measure it.

| Crate | Version | Licence | What it builds | Notes |
|---|---|---|---|---|
| `tree-sitter-html` | `=0.23.2` (2024-11-11) | MIT | `parser.c` 66 KB, `scanner.c` 11 KB | tree-sitter org; ABI 14; the scanner keeps a stack of open tags |
| `tree-sitter-css` | `=0.25.0` (2025-09-28) | MIT | `parser.c` 504 KB, `scanner.c` 3.5 KB | tree-sitter org; ABI 15; the scanner keeps no state |
| `tree-sitter-json` | `=0.24.8` (2024-11-11) | MIT | `parser.c` 28 KB, no external scanner | tree-sitter org; ABI 14; comments are part of the grammar |
| `tree-sitter-yaml` | `=0.7.2` (2025-10-07) | MIT | `parser.c` 1.3 MB, `scanner.c` 51 KB | tree-sitter-grammars org; ABI 14; the scanner keeps stacks of indentation |

- **Four crates are new to `Cargo.lock`,** and nothing else: each needs only `cc` and
  `tree-sitter-language`, already locked. No duplicate versions. `cargo deny check` passes and
  `cargo audit` reports nothing new. Each is the newest release.
- **`unsafe`:** one `LanguageFn::from_raw` per grammar. **`build.rs`:** `cc` compiles the C as
  C11, with `/utf-8` on MSVC. HTML's scanner copies a custom element's name to the heap and
  serializes its stack of tags within tree-sitter's buffer, checking the room before each write.
- **YAML's scanner needs a fix** to bound what it serializes to tree-sitter's buffer, which no
  release carries yet (Q10).
- **Cost:** each file compiles in at most 0.11 s with `clang -O2`; the objects hold about
  0.4 MB in all (YAML 0.2 MB, CSS 0.14 MB, HTML 24 KB, JSON 8 KB), so the release binary grows
  by about 0.4 MB, against the 3.5 MB of ADR 0147's.
- **Windows:** `just clippy-windows`'s stub compiler serves any crate that builds C with `cc`,
  so the recipe does not change. CI's Windows job compiles the C with MSVC.
- **Not used:** Helix's and nvim-treesitter's queries (MPL-2.0 and Apache-2.0), as in ADR 0147.

## Decision

The author chose on 2026-11-18, through the coordinator: Q1 against the recommendation (all four
languages in this pull request), Q2–Q8 as recommended, Q9 with indentation as a later pull
request, and Q10 as option B.

### Languages and modes (Q1, Q6)

- **The four crates, at the versions above,** each an exact version, each `build.rs` and scanner
  diff read before a bump (T2), and listed in `deny.toml` as used only by `oso-syntax`.
- **`Language` gains `Html`, `Css`, `Json` and `Yaml`, and `Mode` the modes of the same names,**
  by the file's name only: `.html` and `.htm` are HTML, `.css` CSS, `.json` and `.jsonc` JSON,
  `.yaml` and `.yml` YAML, ignoring ASCII case. `.xhtml`, `.vue`, `.svelte`, `.scss` and `.less`
  stay Fundamental: their languages are not these grammars'. `html-mode`, `css-mode`,
  `json-mode` and `yaml-mode` set them by hand, as the other mode commands do.
- **No language server comes with them.** The configuration can name the modes (`html`, `css`,
  `json`, `yaml`), and a server set for one is told the protocol's language name (`html`,
  `css`, `json`, `jsonc` for a `.jsonc` file, `yaml`).

### HTML's embedded code (Q2, Q3)

- **A `<script>` is JavaScript, JSON or plain by its `type`** (Q2): JavaScript without one, or
  with an empty one, `module`, `text/javascript`, `application/javascript`, `text/ecmascript`,
  `application/ecmascript`, `text/jsx` or `text/babel` (JavaScript's grammar parses JSX); JSON
  with `importmap`, `speculationrules`, `application/json`, `application/ld+json` or
  `text/json`; plain with any other, such as a template's. **A `<style>` is CSS** without a
  `type` or with `text/css`, and plain otherwise. The value is read with ASCII white space
  trimmed and ignoring ASCII case, at most 32 bytes, and the first `type` counts, as in a
  browser.
- **Found and painted as a fence is:** an HTML piece widens to the whole of each such element it
  meets, its text is parsed with its grammar and painted over HTML's captures, and an element
  that no longer holds code (a `type` typed in, its size past the cap) is highlighted again to
  its end. The element's grammar's query is compiled at its first element, so a page with no
  `<style>` never compiles CSS's.
- **An element longer than 16 KiB, its tags too, keeps its text plain**, as a fence does: a
  piece that widens to it stays small.
- **Only `<script>` and `<style>` content** (Q3): a `style="…"` or `onclick="…"` attribute is a
  string, Markdown's raw HTML stays as it is, and code is embedded one level deep only: an `html`
  fence in Markdown is HTML, and its `<script>` stays plain.

### Queries (Q4, Q5)

oso's own files, as ADR 0111 Q8 has them: written from each grammar's upstream query (MIT,
credited in the file's header), cut to the eleven classes, with only `#eq?`, `#match?` and
`#any-of?`. No class is added. Nothing is captured inside a comment, so that `C-e` keeps
reading a comment's run whole.

- **HTML** (Q4): a tag's name is `Function`, as JSX's is (ADR 0147) and as Emacs's
  html-ts-mode draws it; an attribute's name `Property`; its value, quoted or not, `String`;
  `<!DOCTYPE …>` `Keyword`; a character reference (`&amp;`) `Constant`; a comment `Comment`.
  Text, the angle brackets and an end tag that closes nothing stay plain.
- **CSS** (Q5): at-rules, `and`, `or`, `not`, `only`, `from`, `to` and `!important` are
  `Keyword`; a tag selector `Function`, as the HTML tag it names; class, id, attribute,
  pseudo-class and pseudo-element names `Type`; property and media feature names, and custom
  properties where they are used (`var(--c)`), `Property`; numbers with their units and colours
  (`#fff`) `Constant`; strings and `url(…)`'s address `String`; function names `Function`. Plain
  values such as `red` or `auto` stay plain.
- **JSON:** an object's key, its quotes too, is `Property`; other strings `String`; numbers,
  `true`, `false` and `null` `Constant`; comments `Comment`.
- **YAML:** a mapping's key is `Property`, a number or a boolean too; other scalars, plain or
  quoted or a block's, `String`, unless numbers, booleans, null or dates, which are `Constant`;
  a tag (`!!str`) `Type`; an anchor or an alias `Macro`; directives, `---` and `...` `Keyword`;
  comments `Comment`.

### Fenced code (Q6)

By ADR 0147's rule (the info string's first word, ignoring ASCII case, at most 16 bytes):
`html`, `htm` and `xhtml` are HTML, `css` CSS, `json` and `jsonc` JSON, `yaml` and `yml` YAML.

### Indentation, long lines and limits (Q7, Q8, Q9)

- **The four modes keep the line before's indentation** ([ADR 0115](0115-indentation.md)'s
  Fundamental rules), as the modes of ADR 0138 do, and can be set in the configuration's
  `[modes.html]` and the like. Rules of their own (CSS and JSON by braces and brackets, HTML by
  tags, YAML by the line before) are a later pull request (Q9).
- **ADR 0147's long-line rule holds** (Q8): a text with a line longer than 16 KiB is not parsed,
  in HTML too, where inline SVG and `data:` addresses make such lines more often.
- ADR 0111's other limits, the secret rules and the worker's schedule do not change.

### oso's copy of tree-sitter-yaml (Q10)

- **tree-sitter-yaml 0.7.2 is built from `patches/tree-sitter-yaml/`**, wired through
  `[patch.crates-io]` and excluded from the workspace: the published `.crate` unpacked unchanged
  on its own commit (its sha256 the one crates.io's index gives), then one line of
  `src/scanner.c` changed on the next, bounding what the scanner serializes so that it stays
  within tree-sitter's buffer. `patches/README.md` records the source and what to do at a bump:
  drop the copy once a release carries the change.
- **`cargo audit` does not look a patched crate up,** as it has no registry source in
  `Cargo.lock`: RustSec is checked for it by hand at a bump.

### Tests and measuring (Q7 of ADR 0147)

For each grammar: its captures, its query compiled with only the allowed predicates, comments as
one run, line breaks plain, a parse of it, the property tests (an incremental parse is the same
history fed one string; highlights in slices are highlights in one call) with a seed of each and
an HTML page of embedded code, and the random and crafted texts. For HTML: which `type` names
which language, the cap, an edit to a start tag, and the lazy compile. Fences of each. YAML's
deep nesting, in a file and in a fence. `just perf` does not change, as in ADR 0147: oso-syntax's
`syntax_perf` takes `html`, `css`, `json` and `yaml`.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it. The status of each ADR named
gains `amended by 0157`.

- [ADR 0111](0111-tree-sitter-highlighting.md), "Injections other than Markdown's inline grammar
  and its fenced code wait": HTML's `<script>` and `<style>` elements are parsed with
  JavaScript's, JSON's or CSS's grammar as their `type` says, as fences are.
- ADR 0111, "Fenced code whose info string names Rust or TOML is parsed with that grammar the
  same way", and step 7's info strings, as ADR 0147 amended them: HTML, CSS, JSON and YAML too, by
  the names above.
- [ADR 0147](0147-highlighting-go-typescript-and-javascript.md), Q6, "No injections": it holds for
  JavaScript's templates, regular expressions and JSDoc; HTML embeds JavaScript, as above.
- ADR 0147, its dependencies "each an exact version", from crates.io: tree-sitter-yaml is built
  from oso's copy until a release carries oso's change (Q10).

## Consequences

- HTML, CSS, JSON and YAML files are in colour, an HTML page's scripts and styles too, and so is
  their code in Markdown.
- About 2 MB more C is compiled once per clean build; the release binary grows by about 0.4 MB.
- Four more grammars' C runs in oso's process on untrusted files (ADR 0111 Q11). HTML's and
  YAML's scanners keep state; the limits and the fuzzing hold for them as for the others, and
  oso keeps a fix of its own to YAML's until upstream has one.
- oso now keeps a third-party crate's source in its tree, which a bump must rebase or drop.
- tree-sitter-html and tree-sitter-json have had no release since 2024-11, as tree-sitter-typescript
  has not.
- Follow-ups: indentation for the four modes (Q9); `style` attributes and Markdown's raw HTML as
  HTML or CSS, if wanted.

## Alternatives considered

- **HTML and CSS now, JSON and YAML later** (Q1, the recommendation): a smaller pull request; the
  author wanted the four at once.
- **Every `<script>` as JavaScript** (Q2), as upstream's injection query has it: a template's or a
  JSON block's text would be JavaScript's errors.
- **Attributes and Markdown's raw HTML as code, or code nested more than one level** (Q3): each a
  further parse a piece for little.
- **Tags as `Keyword` or `Type`** (Q4): unlike JSX's, which ADR 0147 made functions.
- **One class for every CSS selector** (Q5): `a.button:hover` would read as one word.
- **A tag-aware indentation now** (Q7, Q9): its own work and tests; it comes after.
- **For HTML only, parsing past a long line** (Q8): a new mechanism, against an accepted rule.
- **Leaving YAML out until upstream fixes its scanner, or a check before parsing YAML** (Q10): the
  first loses YAML for an unknown time, and the second cannot bound the scanner's state exactly.

## As built

- `tree-sitter-html`, `tree-sitter-css` and `tree-sitter-json` were added on a commit each, and
  `tree-sitter-yaml` on two (the copy, then oso's change), each with its `deny.toml` entry.
- **The modes:** `Mode::Html`, `Mode::Css`, `Mode::Json` and `Mode::Yaml` (public, in the API
  snapshot), each highlightable by `Buffer::can_highlight` and named in the configuration's
  modes, so `Editor::set_mode_indents` takes 14 modes now. `Language::Html`, `Language::Css`,
  `Language::Json` and `Language::Yaml` (public).
- **The injection queries:** `Kind::Injections` names its host now, Markdown or HTML
  (crate-private). HTML's finds `script_element` and `style_element`; `document/embedded.rs`
  reads the start tag's `type` with a `TreeCursor` and copies at most 64 bytes of a value, and
  `widen` gives the same `Nested` texts Markdown's does, so the painting, the lazy compile and
  the repainting of an element that no longer holds code are Markdown's.
- **YAML's tree may end before its text** after an error, which no other grammar's does: the
  property test that a parse ends in the newest text compares a YAML tree's end with a fresh
  parse's. The text past it stays plain.
- **Measured** with `syntax_perf`, release, on the author's Mac. Inputs: 2 MiB of the Rust book's
  HTML; the same with a 1.7 KB script and a 2 KB style every 40 lines; the CSS, JSON and YAML
  files of installed tools and crates, YAML's joined as documents of one stream:

  | Input, 2 MiB | Parse | Parse and highlight | In 20 ms slices | After one key | Peak memory |
  |---|---|---|---|---|---|
  | HTML | 43–59 ms | 93–112 ms | 5, the longest 20.0 ms | 0.24–0.27 ms | 61 MB |
  | HTML with scripts and styles | 28 ms | 193 ms | 10, the longest 20.0 ms | 0.60 ms | 58 MB |
  | CSS | 79 ms | 120 ms | 6, the longest 20.0 ms | 9.3 ms | 107 MB |
  | JSON | 48 ms | 88 ms | 5, the longest 20.0 ms | 1.4 ms | 100 MB |
  | YAML | 34 ms | 57 ms | 3, the longest 20.0 ms | 0.36–0.41 ms | 84 MB |
  | Rust (oso-core, for comparison) | 89 ms | 170 ms | 9, the longest 20.0 ms | | 167 MB |

  Peak memory is the process's highest resident set as `syntax_perf` runs, which holds two
  documents of the text at once; Rust's is measured the same way. Crafted texts of 2 MB, a few
  characters repeated (unclosed tags, an element of code a line, deep YAML nesting), are
  highlighted, or reach a first parse's 5 s or the highlighting limits and stay plain, as the
  same kinds of text do in Rust's grammar.
