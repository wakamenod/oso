//! Pass 2: text drawn as glyph quads on the cell grid, from the atlas
//! (ADR 0005, ADR 0036).
//!
//! - Only visible rows are drawn, and of each only the visible columns,
//!   in steps of [`WINDOW_STEP`] columns, shaped through the cache in
//!   `rows`. A 10 MB line costs what a screen of it does.
//! - Glyphs are rasterized once, into the atlas, within a per-frame budget.
//!
//! The GPU side of text lives here; shaping and grid placement do not.

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use oso_core::display_map::{DisplayPoint, DisplaySnapshot};
use oso_core::window::Scroll;
use oso_platform::text::{CellMetrics, GlyphFormat, TextSystem};

use wgpu::util::DeviceExt;

use crate::atlas::{AtlasGlyph, GlyphAtlas, RasterBudget};
use crate::grid::{PlacedGlyph, Style};
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
    /// Colors by [`Style`].
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

    /// The columns at least partly inside the viewport, plus one for glyphs
    /// that overhang their cell to the left.
    pub(crate) fn visible_columns(&self) -> usize {
        let space = (self.viewport[0] - self.origin[0]).max(0.0);
        (space / self.metrics.width).ceil() as usize + 1
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

    /// The quad for one glyph of `row`, or `None` if it is off screen.
    pub(crate) fn quad(
        &self,
        row: usize,
        glyph: &PlacedGlyph,
        entry: &AtlasGlyph,
    ) -> Option<GlyphInstance> {
        let [cell_x, cell_y] = self.cell(row, glyph.column);
        let x = cell_x + glyph.offset[0] + entry.left as f32;
        let y = cell_y + self.metrics.ascent + glyph.offset[1] - entry.top as f32;
        let [w, h] = [entry.size[0] as f32, entry.size[1] as f32];
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
            uv: [entry.uv[0] as f32, entry.uv[1] as f32],
            color: match glyph.style {
                _ if under_cursor => self.cursor_color,
                Style::Text => self.color,
                Style::Escape | Style::Boxed => self.escape_color,
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
        for row in visible.clone() {
            let shaped = rows.row(display, row, columns.clone(), system);
            boxes.extend(
                shaped
                    .boxes
                    .iter()
                    .map(|&(column, cells)| (row, column, cells)),
            );
            // Only glyphs that can show are looked up, so nothing off
            // screen is rasterized.
            let left = layout.scroll.left.saturating_sub(WINDOW_STEP);
            let right = layout.scroll.left + layout.visible_columns();
            let from = shaped
                .glyphs
                .partition_point(|g| g.column + (g.cells as usize) < left);
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
            style: Style::Text,
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
            style: Style::Escape,
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

    #[test]
    fn only_rows_and_columns_in_the_viewport_are_visible() {
        let l = layout();
        // (200 - 8) / 10 = 19.2, so 20 columns show, plus one for overhang.
        assert_eq!(l.visible_columns(), 21);
        // (100 - 8) / 20 = 4.6, so 5 rows are at least partly visible.
        assert_eq!(l.visible_rows(), 5);
        let tiny = Layout {
            viewport: [200.0, 4.0],
            ..l
        };
        assert_eq!(tiny.visible_rows(), 0);
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
