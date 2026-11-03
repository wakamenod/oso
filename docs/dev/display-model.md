# Display model

The display model maps buffer positions to what the screen shows. Buffer
content and screen content differ in many ways:

- Folded ranges are hidden.
- Virtual text appears that is not in the file, such as inlay hints.
- Markup is concealed; an org link shows only its description.
- Tabs expand, and long lines may wrap.
- Blocks such as images or diagnostic details sit between lines.

Cursor motion, scrolling, hit testing and rendering all depend on converting
between buffer and display coordinates correctly and quickly. The decision
record is [ADR 0006](adr/0006-display-model.md).

## Transform pipeline

```
Buffer (rope, file content)
  ↓ FoldMap     hide ranges and show a placeholder ("…")
  ↓ InlayMap    insert or replace text for display only (inlay hints, link descriptions)
  ↓ TabMap      expand tabs to cell columns
  ↓ WrapMap     soft wrap at the window width (off by default)
  ↓ BlockMap    insert blocks between rows, possibly taller than one row
Display rows
```

- Each layer converts positions in both directions in O(log n), using
  summarized trees in the style of Zed's `SumTree`. oso implements its own
  `SumTree` in `oso-core`, and the buffer's rope is built on it too
  ([ADR 0022](adr/0022-text-buffer-and-file-loading.md)).
- Layers update incrementally. An edit invalidates only the affected range.
- Only visible rows are laid out and rendered.
- The whole pipeline lives in `oso-core`. It has no GPU dependency and is
  unit- and property-tested.

## Cell grid

Layout uses a monospace cell grid. A half-width grapheme takes one cell and a
full-width grapheme takes two (Unicode East Asian Width). The x position of a
cell is `column × cell_width`, so hit testing and cursor motion need no
shaping. Shaping and rasterization are still used for drawing (emoji, CJK,
ligatures) through the OS-native backend. Proportional fonts are not
supported.

## Row heights

Rows have uniform height by default. The architecture supports rows of
different heights from the start (inline images, larger headings, multi-row
blocks) by keeping a height index in the BlockMap tree, so `y → row` stays
O(log n). This is cheaper than retrofitting variable heights later.

## Soft wrap and motion

Soft wrap is off by default and can be enabled per mode (for example Markdown
or org) or toggled per buffer. `C-n` and `C-p` move by logical lines.

## Decorations

Decorations are positional ranges grouped into **layers**. Each producer owns
one layer:

| Layer owner | Examples | Notes |
|---|---|---|
| Syntax | tree-sitter highlights | computed in the background, swapped atomically |
| Search | isearch matches | |
| Diagnostics | LSP errors and warnings | underline styles and gutter signs |
| Modes | dired file lines, git sections and hunks | carry typed data |
| Selections | regions of every cursor | |

- Anchors shift with edits, so decorations stay attached to the right text.
- Decorations do **not** travel with text on copy and yank. This differs from
  Emacs text properties and is deliberate: it keeps async highlighting simple.
- A semantic decoration can carry arbitrary typed data and a **keymap at
  point**. For example, `s` stages the hunk under the cursor in the git status
  buffer.
- Replace-style decorations (concealment, link descriptions) feed the
  InlayMap.

## Windows, frames and floating windows

- A frame holds a tree of split windows. Tabs (tab-bar style) hold separate
  window trees.
- Each window has its own selection set, so the same buffer in two windows can
  be edited at different places.
- Floating windows are positioned freely above the tree. They show the
  minibuffer, menus, completion popups and hover documentation. Their
  placement is decided per feature.

## Terminal views

A terminal view renders an `alacritty_terminal` cell grid directly, not a
rope. It implements a searchable-text interface so that isearch, line search,
copy and selection work as in text buffers.
