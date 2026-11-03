# 0036. Text system, glyph atlas and the default font

- Status: Proposed
- Date: 2026-11-03

## Context

[ADR 0005](0005-text-rendering.md) decided OS-native shaping and
rasterizing behind a text-system trait, a monospace cell grid and a GPU
glyph atlas. It names `oso-platform` as the home of the trait, while
[ADR 0004](0004-crate-layout.md) lists the trait under `oso-render`. M1-5
builds the macOS backend, and needs concrete answers for:

- the CoreText binding crate
- the shape of the trait, and which crate places glyphs on the grid
- how `oso-render` gets glyphs without depending on CoreText or winit
- the atlas design
- the default font until configuration exists
  ([ADR 0019](0019-configuration.md))

## Decision

### Binding crate

- `objc2-core-text` and `objc2-core-graphics` 0.3, with only the features
  used, as macOS-only dependencies of `oso-platform`.
  `objc2-core-foundation` is already in the tree through wgpu's Metal
  backend, so two crates are new.
- All `unsafe` code of the text system is in
  `oso-platform/src/text/coretext.rs`, each block with a `// SAFETY:`
  comment.

### The trait

- `oso_platform::text::TextSystem`:
  - `metrics()`: the cell width and height, ascent and descent, in whole
    physical pixels
  - `shape(row)`: glyphs in visual order, each with its font and glyph id,
    the byte offset of its cluster, and its pen position and advance
  - `rasterize(glyph)`: a coverage bitmap (one byte per pixel) for text, or
    premultiplied RGBA for color fonts (emoji)
- `oso_platform::text::load(families, size, scale)` loads the first
  installed family at the window's scale factor, so all sizes are physical
  pixels and glyphs are rasterized at the resolution they are shown at. A
  scale factor change loads the font again.
- Font fallback is the OS's: on macOS, `CTLine` runs use the system cascade
  list for the primary font.
- Systems without a backend return `Unsupported`. The app then uses
  `NullTextSystem`: a grid with cells estimated from the font size, and no
  text. Linux and Windows build, run and pass CI this way (Tier 2,
  [ADR 0003](0003-platform-tiers.md)).

### Placing glyphs on the grid

- `oso-render` places glyphs, not the backends. A glyph's cluster is
  snapped to the display cluster of `oso_core::display_map` that contains
  it, so drawing and cursor motion use the same widths
  ([ADR 0035](0035-sumtree-rope-and-cell-grid.md)). A shaper cluster that
  covers several display clusters (a ligature) spans all their cells.
- A cluster's glyphs keep their offsets from the font, and the cluster is
  centered in its cells. Full-width glyphs from a fallback font whose
  advance is not exactly two cells sit in the middle of them.
- Cell width and height are rounded to whole pixels, and quads sit on whole
  pixels, so glyphs stay crisp.

### How `oso-render` gets glyphs

- `oso-render` depends on `oso-platform` and takes a
  `Box<dyn TextSystem>` from the tip crate. It does not depend on winit or
  CoreText.
- The tip crate chooses the font and the scale factor; the renderer asks
  for shaping and rasterizing only on a cache miss.
- This settles ADR 0004 and ADR 0005: the trait lives in `oso-platform`,
  and `oso-render` uses it.

### Glyph atlas and pass 2

- Two textures: R8 coverage for text and RGBA8 sRGB for color glyphs.
- Glyphs are packed into shelves and never move. A full texture doubles,
  up to 8192 px or the device limit, and keeps its contents, so a glyph is
  rasterized once while its font is loaded.
- A frame that still does not fit clears the atlas and is built once more,
  so a full atlas never leaves glyphs invisible.
- Rows are shaped when they first become visible and kept until their text
  or the font changes. A frame turns only the glyphs inside the viewport
  into quads. One instanced draw per frame, with `textureLoad` and no
  sampler.
- Bounds against hostile text and fonts: at most 32 glyphs per cluster,
  2^18 quads per frame (within the device's buffer limit), 1024 px per
  glyph side, and font sizes clamped to 4 to 256 pt.

### Cursor

- The cursor is a box of the real cell size
  ([ADR 0026](0026-m2-ui-layout.md)). On a full-width character it covers
  both cells, as in Emacs, and the glyph under it takes the background
  color.

### Default font

- `SF Mono`, then `Menlo`, at 13 pt, as constants in the tip crate until
  configuration exists. CoreText finds SF Mono by name only where it is
  installed as a font (Apple's developer download); otherwise Menlo, which
  ships with every macOS, is used.

## Consequences

- Text renders on macOS with native quality, CJK and emoji fallback, and
  color emoji. Measured on the author's Mac (release build):
  - the font loads in about 1 ms
  - the first frame's 76 glyphs are shaped and rasterized in about 4 ms
  - the first frame comes about 50 ms after window creation, within the
    100 ms budget; the first launch of a freshly built binary took about
    200 ms, with Metal's shader cache cold
  - cursor motion rasterizes nothing, and the app idles at 0% CPU
- Linux and Windows show the cursor on the grid but no text until their
  backends exist.
- Another backend (DirectWrite, Linux) only shapes and rasterizes; grid
  placement, the atlas and the cursor are shared.
- Text is blended in linear space on the sRGB surface, so light text on a
  dark background looks slightly heavier than in native macOS apps. Revisit
  with the theme ([ADR 0027](0027-theme.md)).
- Left for M1-6, when buffers feed rows:
  - shape only the visible column window of a row, not the whole row, so a
    10 MB line does not block the UI thread
  - key the shaping cache by buffer line, not by screen row, so scrolling
    does not reshape every row
  - cap new rasterizations per frame and batch uploads, so a page of new
    CJK glyphs does not stall one frame
  - draw boxed code points and control escapes from the display model
    (`ui.md`, ADR 0035) instead of shaping the raw characters
- The atlas never evicts single glyphs. A session with very many distinct
  glyphs clears it when full and rasterizes the visible ones again.

## Alternatives considered

- **`core-text` and `core-graphics` (servo):** mature, but they add four to
  five duplicate versions of crates already in the tree, and a proc macro.
- **Backends place glyphs on the grid:** keeps placement next to shaping,
  but every backend would need its own copy of the display model's widths,
  and drawing and cursor motion could disagree.
- **The tip crate hands bitmaps to `oso-render`:** keeps `oso-render` free
  of `oso-platform`, but moves atlas misses into the crate that changes most
  often, and needs a callback or two passes per frame.
- **One RGBA atlas for everything:** one texture and bind, but four times
  the memory for text, which is almost all of it.
- **Re-rasterizing on atlas growth, instead of copying:** simpler, but a
  stall the size of the whole atlas each time it grows.
- **Fractional cell widths:** closer to the font's advance, but glyphs
  would need subpixel positioning to stay crisp, and caching per subpixel
  offset.
- **AppKit's monospaced system font for SF Mono:** always available, but
  needs `objc2-app-kit`, a much larger dependency.
