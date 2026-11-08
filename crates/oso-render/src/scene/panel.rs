//! A float's panel as rounded boxes: the panel, its field, its selected candidate and its keys, in pixels around
//! their cells, colored by their faces and sized by a [`PanelStyle`] (ADR 0075). Each box's
//! quad is cut to the pane's region here, on the CPU, as everything else a frame draws is
//! (ADR 0061), so the GPU draws nothing outside it.
//!
//! Must not depend on: `paint`, `capture` or a font backend.

use oso_core::frame::{CellRect, PixelGrid, PixelRect};

use crate::pane::{Face, Panel, PanelStyle};
use crate::shape::Rounded;

/// One point in pixels: the cell's height over 16, so that what is sized in points follows
/// the font's size as well as the display's scale.
pub(super) fn point(cell_height: f32) -> f32 {
    cell_height / 16.0
}

/// `size` points in whole pixels, at least one.
fn whole(size: f32, pt: f32) -> f32 {
    (size * pt).round().max(1.0)
}

/// The rounded boxes of `panel`, in a pane over `region` with cells of `cell` pixels. On the
/// frame's top row the panel and the region reach up to the surface's edge, over the margin.
pub(super) fn boxes(
    panel: &Panel<'_>,
    region: CellRect,
    cell: [f32; 2],
    style: &PanelStyle,
) -> impl Iterator<Item = Rounded> {
    let grid = PixelGrid::new(cell[0], cell[1]);
    let pt = point(cell[1]);
    let clip = to_edge(grid.rect(region), region.row == 0);
    let attached = panel.cells.row == 0;
    let fill = to_edge(grid.rect(panel.cells), attached);
    let radius = style.radius * pt;
    let top = if attached { 0.0 } else { radius };
    let hairline = whole(style.hairline, pt);
    let (shadow, down, sigma) = fit_shadow(style, pt, fill, clip);
    let body = Rounded {
        rect: corners(fill),
        radii: [top, top, radius, radius],
        fill: Face::Float.background(),
        outline: Face::FloatBorder.background(),
        shadow,
        params: [hairline, down, sigma, 0.0],
        bounds: [0.0; 4],
    }
    .cut(3.0 * sigma + hairline + 1.0, down, clip);
    let field = panel
        .field
        .filter(|field| field.rows > 0 && field.columns > 0)
        .and_then(|field| {
            let outline = whole(style.field_outline, pt);
            let [across, up] = style.field_reach.map(|reach| (reach * pt).round());
            // The box, its outline included, reaches past the answer's cells only into the
            // room the layout gives it.
            let room = grid.rect(panel.field_room);
            let cells = grid.rect(field);
            let outer = cut(
                PixelRect {
                    x: cells.x - across,
                    y: cells.y - up,
                    width: cells.width + 2.0 * across,
                    height: cells.height + 2.0 * up,
                },
                room,
            )?;
            let inside = PixelRect {
                x: outer.x + outline,
                y: outer.y + outline,
                width: (outer.width - 2.0 * outline).max(0.0),
                height: (outer.height - 2.0 * outline).max(0.0),
            };
            Rounded {
                rect: corners(inside),
                radii: [style.field_radius * pt; 4],
                fill: Face::Field.background(),
                outline: Face::Field.foreground(),
                shadow: [0.0; 4],
                params: [outline, 0.0, 0.0, 0.0],
                bounds: [0.0; 4],
            }
            .cut(outline + 1.0, 0.0, clip)
        });
    let keys = panel
        .keys
        .iter()
        .filter(|key| key.rows > 0 && key.columns > 0)
        .filter_map(move |key| {
            let cells = grid.rect(*key);
            let [across, up] = style.key_inset.map(|inset| (inset * pt).round());
            Rounded {
                rect: [
                    cells.x + across,
                    cells.y + up,
                    (cells.width - 2.0 * across).max(0.0),
                    (cells.height - 2.0 * up).max(0.0),
                ],
                radii: [style.key_radius * pt; 4],
                fill: Face::Key.background(),
                outline: [0.0; 4],
                shadow: [0.0; 4],
                params: [0.0; 4],
                bounds: [0.0; 4],
            }
            .cut(1.0, 0.0, clip)
        });
    let selected = panel
        .selected
        .and_then(|row| selected_box(row, &grid, pt, style, fill, clip));
    body.into_iter().chain(field).chain(selected).chain(keys)
}

/// The selected candidate's `row`, a little wider than its cells, kept inside the panel's
/// `fill` and cut to `clip` (ADR 0076).
fn selected_box(
    row: CellRect,
    grid: &PixelGrid,
    pt: f32,
    style: &PanelStyle,
    fill: PixelRect,
    clip: PixelRect,
) -> Option<Rounded> {
    if row.rows == 0 || row.columns == 0 {
        return None;
    }
    let across = (style.selected_reach * pt).round();
    let cells = grid.rect(row);
    let wide = cut(
        PixelRect {
            x: cells.x - across,
            width: cells.width + 2.0 * across,
            ..cells
        },
        fill,
    )?;
    Rounded {
        rect: corners(wide),
        radii: [style.selected_radius * pt; 4],
        fill: Face::Selected.background(),
        outline: [0.0; 4],
        shadow: [0.0; 4],
        params: [0.0; 4],
        bounds: [0.0; 4],
    }
    .cut(1.0, 0.0, clip)
}

/// The shadow's color, offset down and sigma below `fill`. With less room above `clip`'s
/// bottom than it reaches, it shrinks and fades to fit, so it ends before the pane does rather
/// than in a hard line along its edge.
fn fit_shadow(
    style: &PanelStyle,
    pt: f32,
    fill: PixelRect,
    clip: PixelRect,
) -> ([f32; 4], f32, f32) {
    let (sigma, down) = (style.shadow_sigma * pt, style.shadow_down * pt);
    let room = (clip.y + clip.height - (fill.y + fill.height)).max(0.0);
    let fit = (room / (3.0 * sigma + down)).min(1.0);
    let mut shadow = style.shadow;
    shadow[3] *= fit;
    (shadow, down * fit, sigma * fit)
}

/// `rect` up to the surface's top edge if `attached`.
fn to_edge(rect: PixelRect, attached: bool) -> PixelRect {
    if attached {
        PixelRect {
            y: 0.0,
            height: rect.height + rect.y,
            ..rect
        }
    } else {
        rect
    }
}

fn corners(rect: PixelRect) -> [f32; 4] {
    [rect.x, rect.y, rect.width, rect.height]
}

/// What stays of `rect` inside `within`, if anything.
fn cut(rect: PixelRect, within: PixelRect) -> Option<PixelRect> {
    let x = rect.x.max(within.x);
    let y = rect.y.max(within.y);
    let right = (rect.x + rect.width).min(within.x + within.width);
    let bottom = (rect.y + rect.height).min(within.y + within.height);
    (right > x && bottom > y).then_some(PixelRect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

impl Rounded {
    /// The same with its quad: the box and `margin` pixels around it, `down` more below,
    /// cut to `clip`. `None` if nothing of it is left.
    fn cut(self, margin: f32, down: f32, clip: PixelRect) -> Option<Self> {
        let [x, y, width, height] = self.rect;
        let quad = PixelRect {
            x: x - margin,
            y: y - margin,
            width: width + 2.0 * margin,
            height: height + 2.0 * margin + down,
        };
        let bounds = cut(quad, clip)?;
        Some(Self {
            bounds: [
                bounds.x,
                bounds.y,
                bounds.x + bounds.width,
                bounds.y + bounds.height,
            ],
            ..self
        })
    }
}
