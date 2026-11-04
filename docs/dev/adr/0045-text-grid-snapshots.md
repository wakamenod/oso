# 0045. Text-grid snapshots

- Status: Accepted
- Date: 2026-11-04

## Context

[ADR 0037](0037-test-pyramid.md) requires text-grid snapshots for every feature
that changes what is drawn, using `insta` if approved
([ADR 0010](0010-process-and-network-policy.md)). It leaves open how the grid
is built, where the tests live and how CI is kept from accepting a snapshot.

A snapshot protects the product only if it shares the renderer's logic. Part of
that logic was already in `oso-core`: cell widths, tab stops, clusters and the
cursor's width. The rest was private to `oso-render`, in `Rows::shape`: what
each cluster puts on the grid. `oso` is a binary crate, so `crates/oso/tests/`
cannot reach the `Dispatcher`.

## Decision

### What is shared

- `DisplayCluster::for_each_run` in `oso_core::display_map` says what a cluster puts on
  the grid: its text, a blank across a tab's cells, or an escape's label with
  one `CellRun` per cell, each with a `CellStyle`.
- `Rows::shape` and the text grid both go through it, including for the boxes
  behind invisible characters. Both take the cursor from
  `Window::cursor_point`, and its width from `motion::cursor_cells`.
- There is no new module and no change to `guardrails/layers.txt`.

### Where the grid and the tests live

- The grid and its scenarios are test code in
  `crates/oso/src/dispatch/grid_tests.rs`, a `#[cfg(test)]` child of
  `dispatch`. Being a child, it needs no change to the layers.
- The scenarios sit in a `#[cfg(test)] mod` in that file, so they do not count
  toward the size limit. The grid and its helpers do.
- They use neither `oso-render` nor a GPU, and nothing from them reaches a
  release build.
- If something other than tests needs the grid, such as the control socket,
  it moves to `oso-core` then, with its own decision.

### The format

Each row of the window is one line between `│`, padded to the window's width.
Below a row that needs it, a line between `:` marks its cells in ASCII:

| Mark | Meaning |
|---|---|
| `^` | the cursor box, over any other mark, as it is drawn on top |
| `<>` | the two cells of a wide cluster |
| `~` | a cell of an escape (`^M`, `\xFF`) |
| `#` | a cell of a boxed invisible character (`U+202E`) |

- `▌` and `▐` stand for a wide cluster that an edge of the window cuts in half.
  They are the grid's notation: the renderer draws part of the glyph.
- Marks show what the row alone cannot: a flag split into two 1-cell clusters
  keeps its text and its width, and `^M` as an escape reads the same as `^M`
  as text. They also do not depend on the reader's font.
- Only the primary cursor is marked, because only it is drawn. The grid follows
  when the renderer draws every cursor (M2).
- A snapshot holds one frame per step of a scenario, under a `keys:` line, and
  its description holds the input with invisible characters escaped.
- A later modeline and minibuffer become more lines below the rows.

### `insta` and CI

- `insta` 1.49 with default features off, as a dev-dependency of `oso` only. It
  adds `insta` and `similar` to `Cargo.lock`.
- `just test`, and so `just ci`, sets `INSTA_UPDATE=no` and
  `INSTA_FORCE_PASS=0`. These override the environment and any `insta` config
  file, so a new or changed snapshot fails and nothing is written.
- `just test` also fails on a pending `*.snap.new` or `*.pending-snap` file.
- `just snapshots` is the one recipe that rewrites snapshots. `cargo-insta` is
  not needed: the review is `git diff`, like code (ADR 0037).
- `.gitattributes` checks out `*.snap` with LF on every OS. Widths come from
  compiled-in Unicode tables, never from a font, so snapshots are the same on
  every OS.

## Consequences

- A change to widths, escapes, tabs, scrolling or the cursor box fails on all
  three OSes without a GPU.
- Snapshots do not cover what happens after the cells: `Layout::cell` and
  `Layout::quad`, glyph placement and colors, the shaping budget, and
  the rest of `text_view` in `app.rs`. These stay with layers 4 and 5
  (ADR 0037).
- The grid's own clipping to the window is test code. The renderer draws
  whatever is on the surface, including partly shown rows and columns and
  glyphs that overhang into the margin, so only pixels show that (layer 4).
- `oso-core`'s public API gains `CellRun`, `CellStyle`,
  `DisplayCluster::for_each_run` and `Window::cursor_point`.
- `insta` spawns processes inside test binaries: `cargo metadata` to find the
  workspace, `rustfmt` when printing a failure and a diff tool if one is
  configured. This is outside the product, so the rule that processes spawn
  only through `oso-platform::process` is not affected.
- A plain `cargo test` writes a `.snap.new` file on a failure, as is useful
  while developing. `just ci` then fails until it is accepted or deleted.
- Scenario tests sit inside the crate, unlike the `crates/*/tests/` that
  `testing.md` first described.

## Alternatives considered

- **A grid in `oso-core` as a public module:** reusable from the control socket,
  but it adds a layer and public API for code the product does not use.
- **Rendering through `oso-render` with a fake text system:** it would exercise
  more of the renderer, but it ties the grid to shaping and glyph placement, and
  needs the crate where it can be avoided.
- **A library target in `oso`, with tests in `crates/oso/tests/`:** a larger
  change to the tip crate, whose rebuild time is budgeted, for no gain in what
  the tests can see.
- **A cursor mark inside the row (`█`, as `testing.md` first sketched):** it hides
  the character under the cursor and shifts or replaces cells. A mark line shows
  both, and the width of the box.
- **Marks only for the cursor:** shorter, but it misses the changes above.
- **Inline snapshots:** handy beside the test, but ADR 0037 expects snapshot
  files, and files keep emoji and wide text out of Rust source.
- **A hand-written golden file comparison:** no new dependency, but diffs and
  updates would be ours to maintain, for a dependency that adds two crates.
