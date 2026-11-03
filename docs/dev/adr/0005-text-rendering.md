# 0005. OS-native text rendering on a monospace cell grid

- Status: Accepted
- Date: 2026-11-03

## Context

Japanese text, emoji and font fallback must look right. Hit testing and
cursor motion must be fast. The author uses one monospace font whose
full-width glyphs are exactly two half-width cells wide.

## Decision

- **Shaping and rasterization use OS-native APIs from the start:** CoreText
  on macOS and DirectWrite on Windows, behind a text-system trait in
  `oso-platform`. The Linux backend is open (cosmic-text or
  FreeType + HarfBuzz).
- **Layout uses a monospace cell grid:**
  - half-width graphemes take one cell
  - full-width graphemes take two (Unicode East Asian Width)
  - a column's x position is `column × cell_width`
- Proportional fonts are not supported.
- Rasterized glyphs are cached in a GPU texture atlas.

## Consequences

- Best rendering quality and font fallback on each OS.
- Hit testing, cursor motion and the display model need no shaping, so they
  are fast and simple.
- There are three backends to maintain instead of one.
- No book-style proportional rendering for Markdown or org.

## Alternatives considered

- **cosmic-text everywhere, with native backends later:** less work at first,
  but weaker font fallback and rendering quality than native APIs.
- **Proportional layout:** flexible, but every position lookup needs shaping.
