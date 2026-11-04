# 0041. Module responsibility headers

- Status: Accepted
- Date: 2026-11-04

## Context

[ADR 0038](0038-architecture-guardrails.md), point 7, requires every source
file to start with a `//!` comment that states the module's responsibility
and what it must not depend on, and a test that fails on files without one.
[ADR 0039](0039-architecture-guardrail-mechanics.md) deferred this to M1-10b.
Neither says what the comment looks like or how the test reads it.

`guardrails/layers.txt` already enforces the order of modules and crates. A
header that only repeated it would add nothing and could drift. What the
rules cannot express is the conceptual boundary: the rope knows nothing of
cells and display rows, and a command does not know its keys.

## Decision

### Format

- The file starts with a `//!` comment. Its first paragraph states the
  module's responsibility.
- Its second paragraph starts with `Must not depend on:` and states the
  boundary. It may wrap, and may explain the boundary in a few words. Further
  paragraphs are free.
- The boundary is written in concepts and names that help a reader, not as a
  copy of the layers. It does not have to list everything the layers forbid.
- A name in backticks is a claim the check can test. A lower module that a
  file must still avoid is a conceptual boundary: write it as a concept, not
  a name. For example, `command.rs` says "key presses". The claim must still
  be true, and review checks it.

For example:

```rust
//! The text of a buffer: a rope of UTF-8 chunks on a [`SumTree`]
//! (ADR 0022, ADR 0035).
//!
//! Must not depend on: `decode`, `display_map`, cells or display rows.
```

### Scope

Every `.rs` file of every crate under `crates/`, including `tests/` and
`examples/`, as ADR 0038 says "every source file". The size limit leaves out
`tests/` because it measures product code. A header has the same value there:
the architecture test's own modules have boundaries too. Build output, hidden
directories and symbolic links are skipped, as in the other checks. WGSL
shaders are not covered.

### The check

A new `headers` module in `crates/oso/tests/architecture` fails, naming the
file, when:

- the file does not start with a `//!` line
- the first paragraph is missing, or is the boundary
- the second paragraph does not start with the marker
- the text after the marker has no letter or digit
- the boundary paragraph has an unmatched or doubled backtick
- a name in backticks in the boundary paragraph is listed in the layers that
  apply to the file, and those layers do not forbid it for that file

The layers that apply are:

- `[workspace]`, with the file's crate as the node
- for a file under `src/`, its crate's section, with the file's top-level
  module as the node, or `crate` for `lib.rs` and `main.rs`

Forbidden means on the same line or on a higher line, as in the layer check,
except the file's own node, which fails. A name is read as code spells it:
`crate::buffer`, `display_map::cell`, `buffer.rs` and `oso_core` all name their
node. A name that the layers do not list, such as `wgpu` or a type, is not
checked.

`layers.txt` stays the source of truth. The check never requires a header to
name a module, so a change to the layers fails only the headers whose claims
it makes false.

## Consequences

- Every file states its boundary where an agent reads first, and
  `rg -A1 'Must not depend on:' crates` lists all of them.
- A claim in backticks cannot drift from the rules. Conceptual claims can, and
  stay a review item for `arch-reviewer`.
- Headers count as lines for the size limit. `rope.rs` and `sum_tree.rs` are
  at their allowlisted size, so their comments were tightened to fit.
- Every new file costs a few header lines. This is intended, and a module
  whose boundary is hard to state is worth a second look.
- A word in backticks that happens to equal a module name, such as `text`, is
  read as that module. The failure message tells the author to correct it.

## Alternatives considered

- **A full list of forbidden modules, compared with `layers.txt`:** no drift,
  but long in low layers (`rope` would list 11 modules), and adding a module
  would force an edit in every file below it. It also leaves no place for the
  conceptual boundary.
- **Checking only that the marker is present:** simplest, but a named module
  could drift from the rules without anyone noticing.
- **A boundary only in the crate's `AGENTS.md`:** too far from the file, and
  it cannot say anything about one module.
