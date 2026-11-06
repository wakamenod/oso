# 0061. Panes: drawing a frame as a list with clipping and layers

- Status: Proposed
- Date: 2026-11-05

## Context

The modeline, echo area, region, IME preedit and floating windows all need the
renderer to draw more than one thing ([ADR 0026](0026-m2-ui-layout.md),
[ADR 0008](0008-floating-ui.md)). `RenderState::render` took one `TextView`, with one
`Layout` and one `Rows`, and drew all rectangles before all text:

- An opaque floating window could not hide the text under it.
- A glyph could paint outside the region it belongs to. [ADR 0052](0052-glyphs-stay-in-their-cells.md)
  allows half the descent below a row, which on the last text row lands on the
  modeline's row, reserved by [ADR 0057](0057-frame-layout.md).
- `Rows::begin` drops another buffer's rows, so two views could not share a cache.

## Decision

### The list

`render` and `capture` take `&[Pane]`. A new module, `pane`, holds plain data with no
wgpu types, re-exported at the crate's root:

```rust
pub struct Pane<'a> {
    pub id: PaneId,                 // keeps its shaped rows across frames
    pub region: CellRect,           // the frame's cells it is drawn in
    pub layer: Layer,               // a higher one hides a lower one
    pub background: Option<Face>,   // filled in behind the content
    pub content: PaneContent<'a>,
}

pub enum PaneContent<'a> {
    Text(TextView),                 // rows of a buffer, as before
    Lines(&'a [&'a [Span<'a>]]),    // rows that are no buffer's, as spans
}
```

- **`PaneId`** is the caller's. `Scene` keeps one `Rows` per id and drops it when the
  id leaves the list. A list index would change meaning when a float opens.
- **`Layer`** is a number. `Layer::TILED` is the only one the app uses today. A pane
  above it needs nothing new: M2-16 picks a number.
- **Panes in one layer must not overlap.** They draw together, all their rectangles
  and then all their text. Tiled regions never overlap (ADR 0057). Windows that may
  overlap each other take different layers.
- **Later panes in a layer come first** when a frame runs out of room (see below), so
  what must always show goes after a window's text. Each pane in a list needs its own
  id, which a debug assertion checks.
- **`Span` text goes through the display model,** through the new
  `display_map::text_clusters`, so a control or bidi character in a file name is an
  escape in the modeline too, and never reaches the shaper. A row of its own has no tab
  stops and no next line, so tabs and line breaks are escapes as well, and neither ends
  the row early. The face of an escape's label wins over the span's.
- `Lines` carry no revision. Each row is read and compared with what was shaped, the
  same comparison a buffer's row gets after a change. It stops at the pane's last
  column, and each span is first cut to 64 bytes per column, as much as a display
  cluster can hold ([ADR 0054](0054-limits-on-a-display-cluster.md)).
  A list of short rows costs little and a caller has nothing to get wrong. Large and
  long-lived text belongs in a buffer.

The renderer's half of M2-7 is built here, as the step asked, so that M2-7 needs no
renderer change. The app's list holds the text pane and two empty ones for the modeline
and the echo area, so M2-7 gives them `Lines` and a `background`. It also changes
`panes` in the tip crate to borrow its spans. Its text-grid snapshots can use
`text_clusters` too, so they lay out the same cells. They need no `Face`, so `Face`
stays in `oso-render` unless M2-7 builds its spans in `oso-core`.

### Faces

`Face` is a semantic enum: `Text`, `Escape`, `Boxed` and `Cursor`. Each has a foreground
for coverage glyphs and a background for rectangles, with today's constants until the
theme (M2-23). It replaces passing colors one by one. `PlacedGlyph` and `Grapheme`
hold a `Face` instead of a `CellStyle`, so what a shaper drew as one unit is also cut
where the face changes.

`Face` lives in `oso-render`, because nothing in `oso-core` names one yet. If M3's
highlighting in a library crate needs to, it moves down.

### Clipping: on the CPU

`Layout::quad` already cuts each glyph to its cells on the CPU (ADR 0052). It now also
cuts to the pane's pixels, and rectangles are cut the same way. There is no scissor.

- No state change between panes, so panes in a layer stay one draw.
- It is unit-tested and property-tested with no GPU.
- A pane region larger than the surface, as a stale layout can give for a frame, is
  harmless. A scissor rectangle outside the target is an error in wgpu.
- The cost is four comparisons per glyph, in place of the four against the surface
  that were there.

**A glyph stays in its pane.** The bands of ADR 0052 hold between rows of one pane and
stop at its edges, so a pane's last row loses the band below it and a pane's first row
the band above it, **with one exception: a pane on the frame's top row keeps that band
in the top margin.** No pane owns the margin and tiled panes do not overlap, so nothing
tiled there is painted over, and ADR 0052 keeps this band for real text. Rectangles stay
in the region. M2-7 decides what one-row panes below the text do with the band above
them, and M2-16 and M3 what their panes below the top row do.

### Layers: one draw per layer per pipeline

`Scene` turns the list into a `Vec<Rect>`, a `Vec<GlyphInstance>` and one `LayerDraw`
per layer, each a range of both. `Painter` records them into a single render pass:
for each layer from the back, the rectangles and then the quads. Merging the two
passes costs nothing, since both have the same target. A frame of one layer is two
draws, as before, in one pass instead of two.

**Panes are built front to back, the reverse of how they are drawn.** The ranges let
the buffers hold them in any order. The caps on quads (`MAX_INSTANCES`) and rectangles
(`MAX_RECTS`) and the frame's budget of new glyphs then run out on what is behind and
earlier in the list, not on the modeline or a float. No amount of text in the text
pane can take their place. A full atlas can leave the rebuild short of budget, which
the next frame makes up, the front first again.

### Modules

`scene` holds all of this with no GPU, behind a `GlyphSource` trait inside the crate
that the real atlas and a fake one implement. A full atlas clears and every pane is
built again, so no quad keeps a stale place. `paint` owns the two pipelines and the
scene, and takes a device and a target, so `RenderState` and `capture` share one path
(`prepare`, then `encode`), and a test can run it with no surface.

`guardrails/layers.txt` gets three lines, `paint`, `scene` and `pane`, in this order:
`capture`, `crate`, `paint`, `scene`, `text`, `rows`, `atlas grid shape`, `pane`.

### One mapping from cells to pixels

`PixelGrid::cell(row, column)` is new, and `PixelGrid::rect` uses it. `Layout` takes the
grid from the metrics, and works out the scrolled row and column as integers before
asking it. The region's rows and columns are passed as cells, and never turned into
pixels and back.

### What is drawn in the margins

**The background, and in the top margin the band of the top row.** The column left of
`scroll.left` used to show in the left margin, except when `scroll.left` was a multiple
of `WINDOW_STEP`, since the shaped columns started there. The first column drawn is now
`scroll.left`, and it agrees with the shaped columns at every position. The row above
`scroll.top` is not drawn.

### The partly shown column on the right

**It is not drawn.** The region holds whole cells, so what is left over on the right is
background, like the pixels left over below the echo area (ADR 0057). A column the
cursor can never reach no longer shows, and a wide cluster in the last column shows
the half in the region, as the cursor box already does. Sideways scrolling works in
the same whole columns.

### How the rest will fit

Beyond the renderer's half of M2-7, these are not built. Each adds a little to what
this ADR has:

- **M2-7:** fills the two empty panes, gives them a `background`, and adds a `Face` or two.
- **M2-13, a selection:** a list of rectangles on `TextView`, added to the pane's
  rectangles after its background and before its boxes, in a new `Face`. They are in
  cells, computed in `oso-core`, and cut like the rest.
- **M2-12, preedit:** the preedit stays outside the buffer and is a new field on
  `TextView`, with the text and the range being converted. `Rows::read` splices its run
  into the cursor's row and moves the later columns. The comparison of what was shaped
  then covers it, and the check that skips reading while the revision holds must also
  compare the preedit. The underline is a thin rectangle, in pixels of the row.
- **M2-16, a float:** a pane in a higher layer with a `background`. A float on the
  frame's top row must also decide about the top margin, since its background stops at
  its region and the band above that row is shared. A read-only view, such as a preview,
  makes `TextView::cursor` optional. If a popup comes and goes with
  each key, the scratch space in `Rows` moves to `Scene`, so it is not allocated again.

## Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0052](0052-glyphs-stay-in-their-cells.md): the bands stop at a pane's edges, but
  for the frame's top row. "One instanced draw per frame" becomes one per layer. Columns
  are looked up if they are in the pane, not on the surface. Its note that `Layout`
  knows the surface and not a pane no longer holds.
- [ADR 0057](0057-frame-layout.md): the right side, which it left to this step.
  `TextView::area` is now `Pane::region`, and `text_view` is `panes` in the tip crate.
- [ADR 0045](0045-text-grid-snapshots.md): the renderer no longer draws partly shown
  columns, or glyphs that overhang into the margin other than the band above the top
  row, so the grid's clipping matches what is drawn. No snapshot changes.
- `architecture.md`'s two passes become one, with two pipelines.

## Consequences

- A pane below the frame's top row loses the band above its first row, and every pane
  the band below its last row, so nothing paints on the next pane's row.
- Up to a cell's width less text shows on the right, depending on the window's size.
- A frame's CPU cost goes up a little, on a window of 99 x 32 cells of ASCII in a release
  build (medians of 3 runs, before and after): moving the cursor 87 to 97 µs, scrolling
  115 to 130 µs, and typing, where every row is read again, 200 to 289 µs. Key and typing
  latency show no change at p99. A profile did not pin down the cost of typing, and
  inlining alone moves it by 35 µs, so it stays open until it matters. The first screen
  of a 100 MB file looks 1 to 3 ms later, not yet explained or told apart from noise.
- `Lines` cost what reading their rows costs, each frame. M2-7's modeline is short, and
  `*Messages*` is a buffer. A large float of `Lines` would need a cheap check first.
- A `Scene` holds one `Rows` per pane id, and each is empty until the pane draws text.
- The `oso-render` public API gains `Pane`, `PaneContent`, `PaneId`, `Layer`, `Span` and
  `Face`, and `TextView` loses `area`. `oso-core` gains `PixelGrid::cell`,
  `display_map::text_clusters` and `MAX_CLUSTER_BYTES`.
- Five tests in `paint` draw on a real GPU and read pixels. They are ignored by
  default, since `cargo test` must not need a GPU, and run with `cargo test -p
  oso-render -- --ignored`. CI does not run them.
- Glyphs under the cursor box still take the cursor's color for their whole quad. Layers
  do not change that, so it stays in `open-questions.md`.

## Alternatives considered

- **A scissor per pane:** the same pixels, but one draw per pane per pipeline and a
  state change between them, a GPU to test it, and a rectangle to clamp to the surface.
- **A clip rectangle per instance, tested in the fragment shader:** one draw, but a
  larger instance, and overdraw that is then discarded (as in ADR 0052).
- **Cutting every pane strictly, the top row too:** a simpler rule with nothing in the
  margin, but it gives up what ADR 0052 kept this band for on the first row of every
  file. The author chose the exception after seeing both.
- **Taller rows, or padding inside a pane:** every row or region pays for a few letters,
  and the grid no longer fills the frame.
- **Depth or stencil to hide what is under a float:** saves overdraw, which is a few
  rows of a float, at the cost of a new attachment, and blending still needs the order.
- **Drawing each pane in its own render pass:** simple, but a pass per pane costs more
  on tile-based GPUs, and nothing needs the separation.
- **`Layer` as an enum of `Tiled` and `Floating`:** clearer names, but it has a variant
  with no caller, and M2-16 will know how many it needs.
- **A pane-level list of rectangles in place of `background`:** one mechanism, but only
  the test pane would fill it now. The cursor box needs the cell under it for the
  glyphs' color either way, so it stays on `TextView`.
- **Keeping a stale `Rows` for panes that come and go:** a popup would reopen a little
  faster, for memory held by something not on screen.
- **`Face` in `oso-core`:** the modeline's text could name it if it is built there, at
  the cost of a public module that nothing in `oso-core` uses yet.
