//! Pass 2: text drawn as glyph quads on the cell grid, from the atlas
//! (ADR 0005, ADR 0036).
//!
//! Must not depend on: `capture` or a font backend. The GPU side of text lives
//! here; shaping and assigning glyphs to cells do not.
//!
//! - Only visible rows are drawn, and of each only the visible columns,
//!   in steps of [`WINDOW_STEP`] columns, shaped through the cache in
//!   `rows`. A 10 MB line costs what a screen of it does.
//! - Glyphs are rasterized once, into the atlas, within a per-frame budget.
//! - Each glyph's quad is cut to its cluster's cells and its row (see
//!   [`Layout::quad`], ADR 0052), on the CPU, so this stays one instanced draw.

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use oso_core::display_map::{CellStyle, DisplayPoint, DisplaySnapshot};
use oso_core::window::Scroll;
use oso_platform::text::{CellMetrics, GlyphFormat, TextSystem};

use wgpu::util::DeviceExt;

use crate::atlas::{AtlasGlyph, GlyphAtlas, RasterBudget};
use crate::grid::PlacedGlyph;
use crate::rows::Rows;
use crate::shape::Uniforms;

/// Columns shaped beyond the visible ones on each side, and the step the
/// shaped window moves by.
pub(crate) const WINDOW_STEP: usize = 64;

/// Instances the buffer holds before it first grows.
const INITIAL_INSTANCES: usize = 1024;

/// The most quads in one frame. A screen of text needs far fewer; this
/// bounds the instance buffer when text stacks many glyphs in a cell.
const MAX_INSTANCES: usize = 1 << 18;

/// [`MAX_INSTANCES`], or fewer if the device's buffers cannot hold them.
fn max_instances(device: &wgpu::Device) -> usize {
    let limit = device.limits().max_buffer_size / std::mem::size_of::<GlyphInstance>() as u64;
    MAX_INSTANCES.min(usize::try_from(limit).unwrap_or(usize::MAX))
}

/// One glyph quad, as the shader reads it.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GlyphInstance {
    position: [f32; 2],
    size: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    kind: u32,
}

impl GlyphInstance {
    const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x2,
        2 => Float32x2,
        3 => Float32x4,
        4 => Uint32,
    ];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// What stays of `start..start + len` inside `min..max`, along one axis: the
/// pixels cut from its start and the pixels left, or `None` if none are.
///
/// Quads and their texels both sit on whole pixels, so moving a quad's corner
/// and its atlas position by the same amount shows the same texels in the
/// same places.
fn clip_axis(start: f32, len: f32, min: f32, max: f32) -> Option<(f32, f32)> {
    let cut = (min - start).max(0.0);
    let kept = (start + len).min(max) - (start + cut);
    (kept > 0.0).then_some((cut, kept))
}

/// Where the grid starts and what is drawn on it this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Layout {
    /// Top-left corner of the first cell, in whole pixels.
    pub(crate) origin: [f32; 2],
    /// The surface size; rows and glyphs outside it are skipped.
    pub(crate) viewport: [f32; 2],
    pub(crate) metrics: CellMetrics,
    /// The display row and column at the top-left cell.
    pub(crate) scroll: Scroll,
    /// The cells under the cursor box: its display point and width. Glyphs
    /// there are drawn in `cursor_color` to show on the box.
    pub(crate) cursor: (DisplayPoint, usize),
    /// Colors by [`CellStyle`].
    pub(crate) color: [f32; 4],
    pub(crate) escape_color: [f32; 4],
    pub(crate) cursor_color: [f32; 4],
}

impl Layout {
    /// The top-left corner of a cell, in pixels. Cells left of or above the
    /// scroll position get negative coordinates.
    pub(crate) fn cell(&self, row: usize, column: usize) -> [f32; 2] {
        let column = column as i64 - self.scroll.left as i64;
        let row = row as i64 - self.scroll.top as i64;
        [
            self.origin[0] + column as f32 * self.metrics.width,
            self.origin[1] + row as f32 * self.metrics.height,
        ]
    }

    /// The rows at least partly inside the viewport, from the top one.
    pub(crate) fn visible_rows(&self) -> usize {
        let space = self.viewport[1] - self.origin[1];
        if space <= 0.0 {
            return 0;
        }
        (space / self.metrics.height).ceil() as usize
    }

    /// The columns at least partly inside the viewport, from the scroll
    /// position.
    pub(crate) fn visible_columns(&self) -> usize {
        let space = (self.viewport[0] - self.origin[0]).max(0.0);
        (space / self.metrics.width).ceil() as usize
    }

    /// The first column whose cells can reach the surface, past the margin
    /// left of the grid. Glyphs left of it cannot show.
    fn first_column(&self) -> usize {
        let margin = (self.origin[0] / self.metrics.width).ceil() as usize;
        self.scroll.left.saturating_sub(margin)
    }

    /// The columns of a row to shape: from the left edge rounded down to
    /// a multiple of [`WINDOW_STEP`], to past the right edge as it stands
    /// one step later. The range changes only when the left edge crosses
    /// a multiple of the step, so scrolling sideways shapes again once per
    /// step at most.
    pub(crate) fn shaped_columns(&self) -> Range<usize> {
        let base = self.scroll.left / WINDOW_STEP * WINDOW_STEP;
        let width = (self.visible_columns() + WINDOW_STEP).div_ceil(WINDOW_STEP) * WINDOW_STEP;
        base..base.saturating_add(width)
    }

    /// The quad for one glyph of `row`, cut to where it may paint, or `None`
    /// if nothing of it shows.
    ///
    /// A glyph paints only the cells of its cluster, and only its row. Above
    /// the row it also keeps [`CellMetrics::descent`] pixels: accents on
    /// capitals reach there, and letters can differ only there, such as `Ắ`
    /// and `Ẳ`. Below the row it keeps half that, in whole pixels: marks
    /// under a descender end there (ADR 0052).
    pub(crate) fn quad(
        &self,
        row: usize,
        glyph: &PlacedGlyph,
        entry: &AtlasGlyph,
    ) -> Option<GlyphInstance> {
        let CellMetrics {
            width,
            height,
            ascent,
            descent,
        } = self.metrics;
        let [cell_x, cell_y] = self.cell(row, glyph.column);
        let x = cell_x + glyph.offset[0] + entry.left as f32;
        let y = cell_y + ascent + glyph.offset[1] - entry.top as f32;
        let right = cell_x + glyph.cells as f32 * width;
        let (cut_x, w) = clip_axis(x, entry.size[0] as f32, cell_x, right)?;
        let bottom = cell_y + height + (descent / 2.0).round();
        let (cut_y, h) = clip_axis(y, entry.size[1] as f32, cell_y - descent, bottom)?;
        let [x, y] = [x + cut_x, y + cut_y];
        if x >= self.viewport[0] || y >= self.viewport[1] || x + w <= 0.0 || y + h <= 0.0 {
            return None;
        }
        let (cursor, cursor_cells) = self.cursor;
        let under_cursor = row == cursor.row
            && glyph.column < cursor.column + cursor_cells
            && cursor.column < glyph.column + glyph.cells as usize;
        Some(GlyphInstance {
            position: [x, y],
            size: [w, h],
            uv: [entry.uv[0] as f32 + cut_x, entry.uv[1] as f32 + cut_y],
            color: match glyph.style {
                _ if under_cursor => self.cursor_color,
                CellStyle::Text => self.color,
                CellStyle::Escape | CellStyle::Boxed => self.escape_color,
            },
            kind: match entry.format {
                GlyphFormat::Coverage => 0,
                GlyphFormat::Color => 1,
            },
        })
    }
}

/// Draws glyph quads in pass 2.
pub(crate) struct TextRenderer {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    /// The atlas generation `bind_group` was made for.
    bound_generation: u64,
    instance_buffer: wgpu::Buffer,
    /// Instances `instance_buffer` holds.
    capacity: usize,
    /// The most quads drawn in one frame, within the device's buffer limit.
    max_instances: usize,
    /// This frame's quads, reused across frames.
    instances: Vec<GlyphInstance>,
    pub(crate) atlas: GlyphAtlas,
}

impl TextRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: f32,
        height: f32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("text.wgsl"));
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Text Uniform Buffer"),
            contents: bytemuck::bytes_of(&Uniforms::new(width, height)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Text Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Text Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Text Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[GlyphInstance::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let atlas = GlyphAtlas::new(device);
        let bind_group = Self::bind(device, &bind_group_layout, &uniform_buffer, &atlas);
        Self {
            pipeline,
            bound_generation: atlas.generation(),
            uniform_buffer,
            bind_group_layout,
            bind_group,
            instance_buffer: Self::instance_buffer(device, INITIAL_INSTANCES),
            capacity: INITIAL_INSTANCES,
            max_instances: max_instances(device),
            instances: Vec::with_capacity(INITIAL_INSTANCES),
            atlas,
        }
    }

    fn bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniforms: &wgpu::Buffer,
        atlas: &GlyphAtlas,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Text Bind Group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(atlas.coverage_view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(atlas.color_view()),
                },
            ],
        })
    }

    fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Text Instance Buffer"),
            size: (capacity * std::mem::size_of::<GlyphInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Updates the orthographic projection for a new surface size.
    pub(crate) fn resize(&self, queue: &wgpu::Queue, width: f32, height: f32) {
        let uniforms = Uniforms::new(width, height);
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Builds this frame's quads from the visible rows of `display` and
    /// uploads them, with at most `budget` new glyphs. Must be called
    /// before the render pass begins. Returns the boxes to draw behind
    /// invisible characters, as `(row, column, cells)`, in `boxes`.
    #[allow(clippy::too_many_arguments, reason = "one frame's inputs")]
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        system: &mut dyn TextSystem,
        rows: &mut Rows,
        display: &DisplaySnapshot,
        layout: &Layout,
        budget: &mut RasterBudget,
        boxes: &mut Vec<(usize, usize, usize)>,
    ) {
        // A frame that fills the atlas clears it and is built once more,
        // so a full atlas never leaves glyphs invisible for good.
        for attempt in 0..2 {
            self.build(device, queue, system, rows, display, layout, budget, boxes);
            if !self.atlas.take_full() {
                break;
            }
            if attempt == 0 {
                log::debug!("glyph atlas is full; clearing it");
                self.atlas.clear();
            }
        }
        self.atlas.flush(queue);
        if self.atlas.generation() != self.bound_generation {
            self.bind_group = Self::bind(
                device,
                &self.bind_group_layout,
                &self.uniform_buffer,
                &self.atlas,
            );
            self.bound_generation = self.atlas.generation();
        }
        if self.instances.len() > self.capacity {
            self.capacity = self
                .instances
                .len()
                .next_power_of_two()
                .min(self.max_instances);
            self.instance_buffer = Self::instance_buffer(device, self.capacity);
        }
        if !self.instances.is_empty() {
            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&self.instances),
            );
        }
    }

    /// Fills `self.instances` with the quads of the visible glyphs.
    #[allow(clippy::too_many_arguments, reason = "one frame's inputs")]
    fn build(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        system: &mut dyn TextSystem,
        rows: &mut Rows,
        display: &DisplaySnapshot,
        layout: &Layout,
        budget: &mut RasterBudget,
        boxes: &mut Vec<(usize, usize, usize)>,
    ) {
        self.instances.clear();
        boxes.clear();
        let columns = layout.shaped_columns();
        let top = layout.scroll.top;
        let visible = top..(top + layout.visible_rows()).min(display.row_count());
        // Only glyphs that can show are looked up, so nothing off screen is
        // rasterized: a glyph paints only its own cells.
        let left = layout.first_column();
        let right = layout.scroll.left + layout.visible_columns();
        for row in visible.clone() {
            let shaped = rows.row(display, row, columns.clone(), system);
            boxes.extend(
                shaped
                    .boxes
                    .iter()
                    .map(|&(column, cells)| (row, column, cells)),
            );
            let from = shaped
                .glyphs
                .partition_point(|g| g.column + (g.cells as usize) <= left);
            for glyph in &shaped.glyphs[from..] {
                if glyph.column >= right {
                    break;
                }
                if self.instances.len() >= self.max_instances {
                    return;
                }
                let Some(entry) = self.atlas.get(device, queue, system, glyph.key, budget) else {
                    continue;
                };
                if let Some(quad) = layout.quad(row, glyph, &entry) {
                    self.instances.push(quad);
                }
            }
        }
        rows.evict(visible.start.saturating_sub(visible.len())..visible.end + visible.len());
    }

    /// Records the glyph quads into `pass`.
    pub(crate) fn render(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.instances.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        pass.draw(0..4, 0..self.instances.len() as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oso_platform::text::{FontId, GlyphId, GlyphKey};

    const WHITE: [f32; 4] = [1.0; 4];
    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    const GRAY: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

    fn layout() -> Layout {
        Layout {
            origin: [8.0, 8.0],
            viewport: [200.0, 100.0],
            metrics: CellMetrics {
                width: 10.0,
                height: 20.0,
                ascent: 16.0,
                descent: 4.0,
            },
            scroll: Scroll::default(),
            cursor: (DisplayPoint::new(0, 3), 1),
            color: WHITE,
            escape_color: GRAY,
            cursor_color: BLACK,
        }
    }

    fn glyph(column: usize, cells: u32) -> PlacedGlyph {
        PlacedGlyph {
            key: GlyphKey {
                font: FontId(0),
                glyph: GlyphId(1),
            },
            column,
            cells,
            offset: [0.0, 0.0],
            style: CellStyle::Text,
        }
    }

    fn entry() -> AtlasGlyph {
        AtlasGlyph {
            format: GlyphFormat::Coverage,
            uv: [5, 7],
            size: [8, 12],
            left: 1,
            top: 12,
        }
    }
    #[test]
    fn cells_are_column_times_width_from_the_scroll_position() {
        let mut l = layout();
        assert_eq!(l.cell(0, 0), [8.0, 8.0]);
        assert_eq!(l.cell(0, 3), [38.0, 8.0]);
        assert_eq!(l.cell(2, 1), [18.0, 48.0]);
        l.scroll = Scroll { top: 10, left: 20 };
        assert_eq!(l.cell(10, 20), [8.0, 8.0]);
        assert_eq!(l.cell(11, 19), [-2.0, 28.0]);
        // Far columns keep whole-pixel precision.
        l.scroll.left = 20_000_000;
        assert_eq!(l.cell(10, 20_000_001), [18.0, 8.0]);
    }

    #[test]
    fn glyph_sits_on_the_baseline() {
        let q = layout().quad(0, &glyph(1, 1), &entry()).unwrap();
        // Cell (0, 1) is at (18, 8); the baseline is 16 px down, and the
        // bitmap's top is 12 px above it.
        assert_eq!(q.position, [19.0, 12.0]);
        assert_eq!(q.size, [8.0, 12.0]);
        assert_eq!(q.uv, [5.0, 7.0]);
        assert_eq!(q.kind, 0);
    }

    #[test]
    fn full_width_glyph_after_it_starts_two_cells_later() {
        let l = layout();
        let wide = l.quad(0, &glyph(0, 2), &entry()).unwrap();
        let next = l.quad(0, &glyph(2, 1), &entry()).unwrap();
        assert_eq!(next.position[0] - wide.position[0], 2.0 * l.metrics.width);
    }

    #[test]
    fn glyphs_under_the_cursor_box_use_the_cursor_color() {
        let mut l = layout();
        assert_eq!(l.quad(0, &glyph(3, 1), &entry()).unwrap().color, BLACK);
        assert_eq!(l.quad(0, &glyph(2, 1), &entry()).unwrap().color, WHITE);
        assert_eq!(l.quad(1, &glyph(3, 1), &entry()).unwrap().color, WHITE);
        // A full-width glyph in columns 2 and 3 is under a cursor at 3.
        assert_eq!(l.quad(0, &glyph(2, 2), &entry()).unwrap().color, BLACK);
        // A cursor on a 4-cell escape covers all of its label.
        l.cursor = (DisplayPoint::new(0, 4), 4);
        let escape = PlacedGlyph {
            style: CellStyle::Escape,
            ..glyph(7, 1)
        };
        assert_eq!(l.quad(0, &escape, &entry()).unwrap().color, BLACK);
        assert_eq!(l.quad(0, &glyph(8, 1), &entry()).unwrap().color, WHITE);
        l.cursor = (DisplayPoint::new(5, 0), 1);
        assert_eq!(l.quad(0, &escape, &entry()).unwrap().color, GRAY);
    }

    #[test]
    fn color_glyphs_are_drawn_as_color() {
        let color = AtlasGlyph {
            format: GlyphFormat::Color,
            ..entry()
        };
        assert_eq!(layout().quad(0, &glyph(0, 2), &color).unwrap().kind, 1);
    }

    #[test]
    fn off_screen_glyphs_are_skipped() {
        let l = layout();
        assert!(l.quad(0, &glyph(100, 1), &entry()).is_none());
        assert!(l.quad(10, &glyph(0, 1), &entry()).is_none());
    }

    /// A bitmap of `size` at `left` from the pen and `top` above the baseline.
    fn bitmap(left: i32, top: i32, size: [u32; 2]) -> AtlasGlyph {
        AtlasGlyph {
            size,
            left,
            top,
            ..entry()
        }
    }

    #[test]
    fn a_glyph_wider_than_its_cell_is_cut_on_both_sides() {
        // Cell (0, 1) spans x 18..28. The bitmap spans x 8..38.
        let q = layout()
            .quad(0, &glyph(1, 1), &bitmap(-10, 12, [30, 12]))
            .unwrap();
        assert_eq!(q.position, [18.0, 12.0]);
        assert_eq!(q.size, [10.0, 12.0]);
        // The pixels that stay show the texels they showed before.
        assert_eq!(q.uv, [15.0, 7.0]);
    }

    #[test]
    fn a_glyph_keeps_all_the_cells_of_its_cluster() {
        let l = layout();
        // A full-width glyph that fills its two cells exactly.
        let wide = l.quad(0, &glyph(1, 2), &bitmap(0, 12, [20, 12])).unwrap();
        assert_eq!(wide.position, [18.0, 12.0]);
        assert_eq!(wide.size, [20.0, 12.0]);
        assert_eq!(wide.uv, [5.0, 7.0]);
        // A ligature over three clusters is cut only at the end of the third.
        let ligature = l.quad(0, &glyph(1, 3), &bitmap(0, 12, [35, 12])).unwrap();
        assert_eq!(ligature.size, [30.0, 12.0]);
    }

    #[test]
    fn marks_above_the_row_keep_only_the_descent() {
        // Row 1 starts at y 28, with its baseline at 44 and a descent of 4.
        // The bitmap's top is 30 px above the baseline, at y 14.
        let q = layout()
            .quad(1, &glyph(1, 1), &bitmap(0, 30, [8, 30]))
            .unwrap();
        assert_eq!(q.position, [18.0, 24.0], "4 px above the row");
        assert_eq!(q.size, [8.0, 20.0]);
        assert_eq!(q.uv, [5.0, 17.0]);
    }

    #[test]
    fn marks_below_the_row_keep_half_the_descent() {
        // Row 1 ends at y 48. The bitmap starts 2 px above the baseline, at
        // y 42, and is 20 px tall.
        let q = layout()
            .quad(1, &glyph(1, 1), &bitmap(0, 2, [8, 20]))
            .unwrap();
        assert_eq!(q.position, [18.0, 42.0]);
        assert_eq!(q.size, [8.0, 8.0], "2 px below the row");
        assert_eq!(q.uv, [5.0, 7.0], "the top of the bitmap stays");
    }

    #[test]
    fn the_band_below_the_row_follows_the_metrics_in_whole_pixels() {
        let mut l = layout();
        // As at a higher scale factor: half of 7 is rounded to 4.
        l.metrics = CellMetrics {
            width: 20.0,
            height: 40.0,
            ascent: 33.0,
            descent: 7.0,
        };
        // Row 1 spans y 48..88, with its baseline at 81. The bitmap starts at
        // y 79 and is 30 px tall, so it is kept down to y 92.
        let q = l.quad(1, &glyph(1, 1), &bitmap(0, 2, [8, 30])).unwrap();
        assert_eq!(q.position, [28.0, 79.0]);
        assert_eq!(q.size, [8.0, 13.0]);
    }

    #[test]
    fn a_glyph_entirely_outside_its_cells_is_not_drawn() {
        let l = layout();
        // Left and right of cell (0, 1), which spans x 18..28.
        assert!(l.quad(0, &glyph(1, 1), &bitmap(-8, 12, [8, 12])).is_none());
        assert!(l.quad(0, &glyph(1, 1), &bitmap(10, 12, [8, 12])).is_none());
        // Row 1 keeps y 24..50: this ends at y 24, and this starts at y 50.
        assert!(l.quad(1, &glyph(1, 1), &bitmap(0, 24, [8, 4])).is_none());
        assert!(l.quad(1, &glyph(1, 1), &bitmap(0, -6, [8, 4])).is_none());
    }

    #[test]
    fn a_cut_glyph_under_the_cursor_box_keeps_the_cursor_color() {
        let q = layout()
            .quad(0, &glyph(3, 1), &bitmap(-4, 12, [20, 12]))
            .unwrap();
        assert_eq!(q.size, [10.0, 12.0]);
        assert_eq!(q.color, BLACK);
    }

    #[test]
    fn glyphs_stay_in_their_cells_when_scrolled() {
        let mut l = layout();
        l.scroll = Scroll { top: 10, left: 20 };
        // Cell (11, 19) spans x -2..8 and y 28..48: it shows in the margin.
        let q = l
            .quad(11, &glyph(19, 1), &bitmap(-5, 12, [20, 12]))
            .unwrap();
        assert_eq!(q.position, [-2.0, 32.0]);
        assert_eq!(q.size, [10.0, 12.0]);
        assert_eq!(q.uv, [10.0, 7.0]);
        // Cell (11, 18) spans x -12..-2, so nothing of it is on the surface
        // even if its bitmap reaches past the cell.
        assert!(
            l.quad(11, &glyph(18, 1), &bitmap(0, 12, [30, 12]))
                .is_none()
        );
    }

    proptest::proptest! {
        /// Whatever a font's bitmap looks like, what is drawn is exactly the
        /// part of it in the cluster's cells and the row, with the descent
        /// above and half of it below, and each pixel shows the texel it would
        /// show uncut.
        #[test]
        fn prop_quads_are_the_part_of_the_bitmap_in_their_cells(
            (row, column, cells) in (0usize..4, 0usize..12, 1u32..4),
            (left, top) in (-200i32..200, -200i32..200),
            (width, height) in (0u32..400, 0u32..400),
            (offset_x, offset_y) in (-50i32..50, -50i32..50),
        ) {
            let l = layout();
            let placed = PlacedGlyph {
                offset: [offset_x as f32, offset_y as f32],
                ..glyph(column, cells)
            };
            let bitmap = bitmap(left, top, [width, height]);

            // The same rectangles in integers: the bitmap, and where it may
            // paint. The metrics of `layout` are 10 x 20 with a descent of 4.
            let (cell_x, cell_y) = (8 + 10 * column as i32, 8 + 20 * row as i32);
            let x0 = cell_x + offset_x + left;
            let y0 = cell_y + 16 + offset_y - top;
            let from_x = x0.max(cell_x);
            let to_x = (x0 + width as i32).min(cell_x + 10 * cells as i32);
            let from_y = y0.max(cell_y - 4);
            let to_y = (y0 + height as i32).min(cell_y + 22);
            let shows = from_x < to_x && from_y < to_y;
            let on_surface = from_x < 200 && from_y < 100 && to_x > 0 && to_y > 0;

            let quad = l.quad(row, &placed, &bitmap);
            proptest::prop_assert_eq!(quad.is_some(), shows && on_surface);
            if let Some(q) = quad {
                proptest::prop_assert_eq!(q.position, [from_x as f32, from_y as f32]);
                proptest::prop_assert_eq!(q.size, [(to_x - from_x) as f32, (to_y - from_y) as f32]);
                proptest::prop_assert_eq!(
                    q.uv,
                    [
                        (bitmap.uv[0] as i32 + from_x - x0) as f32,
                        (bitmap.uv[1] as i32 + from_y - y0) as f32,
                    ]
                );
            }
        }
    }

    #[test]
    fn only_rows_and_columns_in_the_viewport_are_visible() {
        let l = layout();
        // (200 - 8) / 10 = 19.2, so 20 columns show.
        assert_eq!(l.visible_columns(), 20);
        // (100 - 8) / 20 = 4.6, so 5 rows are at least partly visible.
        assert_eq!(l.visible_rows(), 5);
        let tiny = Layout {
            viewport: [200.0, 4.0],
            ..l
        };
        assert_eq!(tiny.visible_rows(), 0);
    }

    #[test]
    fn the_first_column_drawn_is_the_last_that_reaches_the_surface() {
        let mut l = layout();
        assert_eq!(l.first_column(), 0);
        // The margin of 8 px shows part of the column before the scroll
        // position. The one before that is off the surface.
        l.scroll.left = 20;
        assert_eq!(l.first_column(), 19);
        l.origin = [28.0, 8.0];
        assert_eq!(l.first_column(), 17, "a margin of 3 cells");
        l.scroll.left = 1;
        assert_eq!(l.first_column(), 0);
    }

    #[test]
    fn shaped_columns_cover_the_view_and_move_in_steps() {
        let mut l = layout();
        assert_eq!(l.shaped_columns(), 0..128);
        l.scroll.left = 63;
        assert_eq!(l.shaped_columns(), 0..128);
        l.scroll.left = 64;
        assert_eq!(l.shaped_columns(), 64..192);
        l.scroll.left = 200;
        assert_eq!(l.shaped_columns(), 192..320);
        // Every left edge until the next step is still covered.
        l.scroll.left = 255;
        assert_eq!(l.shaped_columns(), 192..320);
        assert!(255 + l.visible_columns() <= 320);
    }
}
