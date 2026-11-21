# 0164. Indenting HTML, CSS, JSON, YAML, JavaScript, TypeScript and Go

- Status: Accepted
- Date: 2026-11-21

## Context

[ADR 0115](0115-indentation.md) indents on the UI thread, at the key, by rules per mode in
`oso_core::indent`: pure functions over the rope with lexers of their own, which read forward
from an anchor line within a bounded reach (64 KiB back from a line, 1 MiB a command) and fall
back to the line before's indentation past it. Rust, TOML and Markdown have rules; every other
mode keeps the line before's indentation, and `TAB` there moves to the next tab stop.

[ADR 0157](0157-highlighting-html-css-json-and-yaml.md) gave HTML, CSS, JSON and YAML modes and
highlighting, and left their indentation for later (its Q9). JavaScript, TypeScript, TSX and Go
([ADR 0138](0138-language-servers.md), ADR 0147) have no rules either. The author writes React
in TypeScript and TSX, and Go, every day.

## Decision

The author chose on 2026-11-21, through the coordinator: Q1–Q3, Q5, Q6, Q8 and Q10 as
recommended; Q4 as recommended (YAML as Emacs's yaml-mode); Q7 as recommended, and, asked
after, the same for JavaScript, TypeScript and Go; Q9 against the recommendation: JavaScript,
TypeScript, TSX and Go in this change too, not later.

### One lexer for the languages of brackets and tags

`oso_core::indent::code` reads CSS, JSON, JavaScript and TypeScript (JSX among them), Go and
HTML in one lexer with a dialect each, through ADR 0115's shared pass. For a line:

1. **Inside a multi-line string** (a template literal, a Go raw string, a quoted attribute) or
   the text of `<pre>` or `<textarea>`: keep. **Inside a block comment** (`/* */`): a line that
   starts with `*` goes one past the comment's opening column, any other to the line before's
   indentation, as Rust's rule 1; inside HTML's `<!-- -->`, the line before's.
2. **A line that starts with a closer** goes to the indentation of its opener's line: `}`, `)`
   or `]` in code; `</` to the line of the element it names (or the innermost, for a name the
   line does not hold whole); a start tag's `>` or `/>` to the tag's line.
3. **Otherwise, inside a bracket or an element:** one level past its opener's line (block indent,
   as Prettier and gofmt write). At top level: 0.
4. **JavaScript and Go, a statement that goes on:** when the code line before ended with an
   operator at the same depth, or this one starts with `.`, `?.`, `?`, `:`, `&&`, `||`, `??` or
   `|`, one level past the statement's first line; the continuation does not stack, so a chain's
   calls sit together. In Go, a block opened on such a line (`if a &&` then `b {`) counts from
   the statement, as gofmt indents its body.
5. **Labels:** in JavaScript a `case` sits one level in from its `switch`'s line and the lines
   under it one more, as Prettier writes; `default:` counts as a label only after a `case` in
   the same braces, so an object's `default:` key is a key. In Go, `case` and `default:` sit with
   the `switch`, as gofmt writes.

What each dialect reads:

- **CSS:** strings, `/* */`, and `{ ( [`. **JSON:** strings, `//` and `/* */` in every JSON
  file (Q5: tsconfig and VS Code's files have them, and tree-sitter-json reads them), `{ [`.
- **JavaScript and TypeScript:** strings with escapes; template literals over lines, whose
  `${...}` is code; `//` and `/* */`; regular expressions: a `/` starts one where an operand can
  start (after an operator, a `(`, a keyword such as `return`, at a statement's start) and it
  closes on its line; anywhere else, or unclosed, it divides. **JSX** (in JavaScript and TSX, not
  TypeScript): a `<` where an operand can start, then a letter or `>`, starts an element; its
  text is text (an apostrophe opens no string), its `{...}` code.
- **Go:** strings and runes with escapes, raw strings over lines, `//` and `/* */`.
- **HTML:** elements by their tags, void elements (`<br>`, `<img>`, ...) and `/>` opening nothing,
  comments, doctype; an end tag closes the element it names and what is open inside it, as a
  browser closes an unclosed `<p>`, and one that closes nothing does nothing. `<script>`'s text is
  read as JavaScript and `<style>`'s as CSS, one level in from their tag's line, up to
  `</script` or `</style` wherever it comes, as a browser reads it.

**Anchors** (a line at column 0 the pass may start from): CSS rules and at-rules; JSON's
document bracket only; JavaScript's and TypeScript's top-level declarations (`import`, `export`,
`function`, `class`, `const`, ..., `@`, `}`); Go's (`func`, `type`, `var`, `const`, `import`,
`package`, `}`, `)`); HTML's tags at column 0.

### YAML (Q4)

YAML's indentation is its structure, so no line is reindented by rules: `indent-region` refuses
as in Fundamental, and an existing line keeps its indentation. As Emacs's yaml-mode:

- **`RET`:** after a line that ends with `:` (past a comment, outside quotes), a block scalar's
  `|` or `>` with its indicators, the new line goes one level past the line's content, past its
  `- ` markers (so after `- with:` it nests under `with`); after a `-` alone, to the item's
  content column; after any other line, to that line's indentation.
- **`TAB`:** cycles the line through its stops, as Markdown's does: the deepest the line before
  allows, then each level out to 0, then the deepest again. No state is kept between keys.

### Keys (Q1–Q3)

- **`TAB` and `indent-region`** reindent by these rules in CSS, JSON, HTML, JavaScript,
  TypeScript, TSX and Go, as in Rust and TOML, within ADR 0115's bounds.
- **Typed closers (Q1):** `}`, `)` or `]` typed first on a line reindents it, in those modes;
  in HTML, JavaScript and TSX, the `/` of a `</` that starts the line too, so an end tag goes
  under its element's line before its name is typed.
- **`RET` between a pair (Q2):** `{|}`, `(|)` and `[|]` in CSS, JavaScript, TypeScript and Go,
  `{|}` and `[|]` in JSON, open an indented line between with the closer on its own line; so does
  a start tag directly before the end tag of the same element, `<li>|</li>`, in HTML, JavaScript
  and TSX.

### Past the reach (Q6)

A new line past the reach or the budget took the line before's indentation (ADR 0115). In
these languages it now also goes a level in after a line that ends with an opener (`{`, `[`,
`(`, or a start tag that stays open) and a level out when what follows it starts with a closer.
A large JSON file has no anchor past its first line, so this is how `RET` goes there. An
existing line past the reach still keeps its indentation.

### Defaults (Q7, Q8)

- **Two spaces a level** in HTML, CSS, JSON, YAML, JavaScript, TypeScript and TSX (Prettier's);
  **a tab a level** in Go (gofmt's: indent size 8, tabs, tab width 8); four spaces elsewhere, as
  before. `Mode::default_indent` holds them, for the editor and for the configuration's
  compiled-in values; `[modes.<mode>]` and EditorConfig override them as before.
- **YAML is always indented with spaces** (Q8): a tab cannot indent YAML, so a buffer in YAML
  mode whose setting says tabs indents with spaces of the same size.

### What does not change

ADR 0115's bounds and fallback for existing lines, the shared pass, the UI thread (no tree, no
worker), the commands and keys; Rust's, TOML's, Markdown's and Fundamental's rules; LaTeX and
Swift keep the line before's indentation.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it. The status of each ADR named
gains `amended by 0164`.

- [ADR 0115](0115-indentation.md), "The rules per mode", and "`TAB` (Q2) ... in Rust, TOML and
  Markdown it sets the line's indentation to the rule's column ... In Fundamental ... it runs
  `tab-to-tab-stop`": the rules above for CSS, JSON, HTML, JavaScript, TypeScript, TSX and Go,
  whose `TAB` reindents; YAML's `TAB` cycles. The modes of ADR 0138 other than LaTeX and Swift no
  longer keep the line before's indentation.
- ADR 0115, "Electric closers (Q5): in Rust and TOML": in the modes above too, and the `/` of a
  `</` in HTML, JavaScript and TSX.
- ADR 0115, "Between brackets (Q4)", as built "in Rust and TOML only": in the modes and pairs
  above too.
- ADR 0115, "past that ... a cursor falls back to the previous non-blank line's indentation":
  in the modes above, a level more after an opener and one less before a closer (Q6).
- ADR 0115, the default of four spaces, `Settings::default`: per mode, as above (Q7); YAML always
  spaces (Q8).
- [ADR 0157](0157-highlighting-html-css-json-and-yaml.md), Q9, "The four modes keep the line
  before's indentation": they have the rules above.

## Consequences

- Typing in a React component, a Go function, a stylesheet, a page or a workflow file indents as
  their formatters write them, and `C-M-\` reindents such files by the same rules.
- One more lexer of hand-written rules to keep; a formatter's layout it does not know (Prettier
  breaking a long condition inside `if (`, a JSX attribute on the tag's line with the rest under
  it) indents by the bracket rule and stays as typed when it is not reindented.
- A regular expression is told from a division by the token before it, so a `/` after `)` (as in
  `if (x) /re/.test(y)`) is a division and what follows on its line is read as code; the line
  after is not affected, as a regular expression cannot span lines.
- A JSX element in a `.js` or `.tsx` file is told from a comparison the same way; TypeScript's
  `.ts` has none, so its generics and comparisons never open one.

## Alternatives considered

- **Leaving JavaScript, TypeScript and Go for later** (Q9, the recommendation): the author writes
  them every day.
- **A tree-based indenter** (Zed's `indents.scm`): needs the worker's tree at the key, which
  ADR 0115 Q1 ruled out.
- **Rules relative to the line before only** (VS Code's `indentationRules`): no state, but wrong
  after a multi-line string, comment or tag; used here only past the reach (Q6).
- **YAML reindented by its structure, or by the previous line only** (Q4): a reindent can change
  what a YAML file means; copying the line before misses the common `key:` then its block.
- **Four spaces everywhere** (Q7): no formatter of these languages writes that.

## As built

- **Where the code lives:** `oso_core::indent::code` (the rules, the dialects and their anchors)
  and `code/scan.rs` (reading a line in a dialect), `indent::yaml`, and in the editor
  `editor/indent/yaml.rs` (YAML's `TAB`). New public items: `indent::css`, `json`, `html`,
  `javascript`, `typescript`, `go`, `yaml`, `yaml_stops`, and `Mode::default_indent` and
  `Mode::spaces_only`. `Mode::indent_rules` names the new rules; TSX's are JavaScript's.
- **The fallback** is the lexer's: `Lexer::relative`, by default the line before's
  indentation, is told the line before and what the new line starts with.
- **Tests:** text as Prettier and gofmt write it is a fixed point of each language (CSS, JSON with
  comments, TypeScript with chains, unions and ternaries, a `switch`, JSX, Go with a raw string
  and a condition over lines, an HTML page with void elements, a comment, a style, a script and a
  start tag over lines); template literals, regular expressions, `<pre>`, unclosed elements; new
  lines; the answer past the reach; the keys in the editor (`TAB`, typed closers and `</`, `RET`
  between pairs and tags, YAML's `RET` and `TAB`, the defaults and YAML's spaces); and the new
  rules in the random-text property tests.
- **Measured** with `cargo run --release -p oso-core --example text_perf -- indent`, on the
  author's Mac, 2 MiB of each language as its formatter writes it:

  | Input, 2 MiB | `RET` at 1 cursor, p50 / p99 / max | `RET` at 1000 cursors, p50 | Every line |
  |---|---|---|---|
  | CSS | 0.5 / 0.9 / 34 µs | 0.49 ms | 11.5 ms |
  | JSON, one document | 47 / 145 / 207 µs | 1.65 ms (budget spent) | 10.9 ms |
  | TSX | 1.3 / 2.8 / 37 µs | 1.36 ms | 10.7 ms |
  | Go | 0.9 / 1.4 / 30 µs | 0.75 ms | 11.3 ms |
  | HTML, one page | 26 / 150 / 210 µs | 1.88 ms (budget spent) | 9.6 ms |
  | Rust (ADR 0115, same run) | 1.8 / 4.1 / 43 µs | 1.82 ms | 10.5 ms |

  A JSON document or an HTML page whose content is all inside its first element has no anchor
  past its first lines, so a key reads back to the 64 KiB reach and falls back, as Rust's single
  `impl` does (its "lex from the reach" 191–291 µs in the same run). Crafted text, one `RET`:
  2 MiB of `{` lines 52 / 329 µs, of `[` 20 / 32 µs, of `<a>` lines 0.5 / 0.8 µs, of template
  literals opening `${` 0.49 / 0.77 ms, of JSX tags with `{(` 0.19 / 1.23 ms, p50 / p99.
