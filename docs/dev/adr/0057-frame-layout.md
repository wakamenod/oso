# 0057. The layout of a frame into regions

- Status: Accepted, amended by 0097 and 0112
- Date: 2026-11-05

## Context

The modeline, echo area, IME preedit, region and floating windows all need the
screen divided into regions ([ADR 0026](0026-m2-ui-layout.md),
[ADR 0008](0008-floating-ui.md)), and M2-6 draws a list of panes on top of that.
Until now the renderer drew one view over the whole surface:

- `RenderState::text_area` subtracted private margins and nothing else, so no row
  was kept for a modeline or an echo area.
- A partly shown row was drawn at the bottom, below the rows that scrolling
  counts.
- M2-12 needs the cursor's cell in physical pixels for `set_ime_cursor_area`, and
  only the renderer knew the margins.
- Text-grid snapshots showed the selected window only.

## Decision

### Regions

`oso_core::frame` is a new public module, above `window` and below `editor`.
`frame::layout` is a pure function from a frame's size in whole cells to its
regions: the window's text area, its modeline and the echo area.

- From the top: text, one modeline row, one echo row.
- **A small frame gives up the modeline first and the echo area next.** Three rows
  or more hold all three, two rows hold text and echo, and one row holds text. The
  echo area outlasts the modeline because errors appear there.
- The regions never overlap, and their rows add up to the frame's.
- Floating windows are not in the layout yet. M2-16 adds them, above the other regions
  and free to cover them, once it knows what a feature needs to ask for a place.
- `window` is the one tiled window until splits arrive (M3). Splits divide its part
  of the frame, and each keeps a text area and a modeline.

### Who holds what

- `Editor` holds the layout. `Editor::set_frame_size` sets it, and gives every
  window its text area as its `ViewSize`, so the two cannot disagree. Scrolling,
  `C-v`, the wheel and following the cursor work with that size and need no change.
- `Editor::set_view_size` stays, for tests that think in text rows. It lays out the
  frame that holds that text area. It can go once the branches adding to those test
  helpers have landed (`open-questions.md`).

### Pixels

`frame::PixelGrid` maps cells to physical pixels, and it holds the margins, which
leave `oso-render`. It takes the cell's width and height as plain numbers, because
`oso-core` cannot see `oso_platform::text::CellMetrics`. Layout logic belongs in
`oso-core`, and the renderer draws what it is given:

- `RenderState::frame_size` replaces `text_area`, and `RenderState::pixel_grid`
  hands over the grid.
- `TextView::area` says which cells a view is drawn in. Its rows stop at the area's
  bottom, and clipping to the region comes with M2-6. The cursor box comes from the
  same `frame::cursor_rect` as the rectangle below.
- `Editor::cursor_area(grid)` gives the selected window's primary cursor as a
  physical rectangle, or `None` when it is out of view, for `set_ime_cursor_area`
  in M2-12. It covers the part of the cursor's cells in view. No `Ime` call and no winit are
  involved yet.

### Leftover pixels

- The grid holds whole cells only, and starts at the top-left margin. **The pixels
  left over at the bottom go below the echo area,** in the background color. The
  echo area has no background of its own, so nothing marks them.
- The reserved rows therefore end on whole cells, and no partly shown row is drawn.
- **The right side does not change:** the partly shown column is still drawn. Nothing
  sits to its right, and M2-6 decides this with each pane's clip.

### Snapshots

- This extends [ADR 0045](0045-text-grid-snapshots.md). The grid composes the regions
  from `Editor::frame`, from the top. Text rows stay between `│`, the modeline's row
  is between `=` and the echo area's between `"`.
- The size given to `check` stays the text area's, so an existing scenario keeps its
  scrolling. `check_frame` takes a whole frame, for scenarios about the frame itself.

## Consequences

- The app compares the renderer's frame size with the editor's after each frame, so a
  surface the renderer configured again on its own does not wait for `Resized`.
- A window shows two rows fewer. They are blank until M2-7.
- `Editor::cursor_area` and `text_view` pair the selected window with
  `FrameLayout::window`, which is right only while the selected window is the tiled
  one. M2-16's minibuffer and M3's splits must look up the selected window's regions.
- Every text-grid frame gains two lines. Branches in progress need `just snapshots`
  after merging this.
- A surface too short for one whole row shows no text and no cursor, where it used to
  show part of a row. `ViewSize` stays at least 1, so commands still work.
- Clipping each region to its own cells is M2-6's, with the list of panes.
- Below three rows there is no modeline, so what it shows has no place. M2-7 decides
  where its warnings go then.
- `window::reaches` no longer counts a partly shown row, so a load that grows only
  the rows below the view asks for one frame fewer. M2-7 must ask for a frame when
  the modeline's loading progress changes.
- [ADR 0045](0045-text-grid-snapshots.md) says the renderer draws partly shown rows.
  That no longer holds for rows at the bottom, and still holds for columns on the right.
- `oso-core`'s public API gains the `frame` module, and `oso-render`'s `text_area` is
  replaced. Pixel rectangles are `f32`, as the renderer's are. M2-12 passes them to
  winit as physical values.

## Alternatives considered

- **Reserving rows only in the renderer:** quick, but scrolling and the snapshots would
  not see them, and the layout would need a GPU to test.
- **Layout owned by the tip crate, with `Editor` keeping only `ViewSize`:** no new
  editor state, but two sources of truth for one size, and snapshots could not read
  the frame from the editor they already take.
- **The pixel rectangle computed in the renderer:** it knows the metrics, but layout
  logic would sit in the crate that should only draw, and it needs a GPU to test.
- **Margins kept in `oso-render`, with the core taking them as arguments:** the renderer
  keeps a layout constant, and every caller repeats it.
- **Leftover pixels above the modeline, or as a partly shown row:** the modeline would
  sit flush with the bottom, but the rows would no longer share one grid, and a partly
  shown row is one the cursor can never reach.
- **Stretching the bottom rows to fill the leftover:** no strip at the bottom, but
  rows of different heights break the cell grid.
- **The modeline outlasting the echo area:** it keeps the buffer's name and warnings
  in a frame of two rows, but errors would have nowhere to go.
- **Dropping `set_view_size`:** one way in, but it would touch every test helper while
  other branches add to them.
- **A list of floating rectangles in the layout now:** it would always be empty, cost
  `FrameLayout` its `Copy` and add to the public API. A rectangle does not say which
  window it holds, and a resize rebuilds the layout, so M2-16 would replace it anyway.
- **A trait or an enum of regions:** more to change when M2-16 and M3 know what they
  need, and nothing uses them now.
