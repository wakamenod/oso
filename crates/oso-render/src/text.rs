//! Pass 2: text drawn as glyph quads on the cell grid, from the atlas
//! (ADR 0005, ADR 0036).
//!
//! Rows come from the display model (`oso_core::display_map`):
//!
//! - Only visible rows are drawn, and of each only the visible columns,
//!   widened to [`WINDOW_STEP`] columns on each side so that small
//!   sideways scrolls reuse the shaping. A 10 MB line costs what a screen
//!   of it does.
//! - Shaping is cached by buffer line (display row) and the columns
//!   shaped, and checked against the buffer offsets those columns cover,
//!   so scrolling reshapes only rows that come into view.
//! - Control characters, invalid bytes and invisible characters are never
//!   shaped. Their escapes from the display model (`^M`, `\xFF`,
//!   `U+202E`) are shaped instead, as plain ASCII labels (security.md,
//!   T3), and invisible characters get a box.
//! - Glyphs are rasterized once, into the atlas, within a per-frame budget.

use std::collections::HashMap;
use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use oso_core::buffer::BufferId;
use oso_core::display_map::{CellKind, DisplayPoint, DisplaySnapshot};
use oso_core::sum_tree::Bias;
use oso_core::window::Scroll;
use oso_platform::text::{CellMetrics, GlyphFormat, ShapedGlyph, TextSystem};

use wgpu::util::DeviceExt;

use crate::atlas::{AtlasGlyph, GlyphAtlas, RasterBudget};
use crate::grid::{Grapheme, PlacedGlyph, Placer, Style};
use crate::shape::Uniforms;

/// Columns shaped beyond the visible ones on each side, and the step the
/// shaped window moves by.
pub(crate) const WINDOW_STEP: usize = 64;

/// The most characters of one cluster passed to the shaper: enough for
/// the longest emoji sequences and Indic clusters. A cluster of a base and
/// thousands of combining marks takes one cell; shaping all of it on the
/// UI thread would let a small file stall a frame.
const MAX_CLUSTER_CHARS: usize = 16;

/// The characters shaped per row, per column shaped. Real text has one or
/// two; past this budget, clusters are shaped by their first character
/// only, so hostile text costs a bounded amount per row.
const CHARS_PER_COLUMN: usize = 4;

/// `text` cut to its first `chars` characters.
fn clip_cluster(text: &str, chars: usize) -> &str {
    match text.char_indices().nth(chars) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

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

/// One display row's shaped columns.
#[derive(Debug, Default)]
pub(crate) struct ShapedRow {
    /// The columns shaped.
    columns: Range<usize>,
    /// The buffer offsets those columns cover. Text loaded or edited there
    /// changes them, which makes the row shape again.
    offsets: Range<usize>,
    /// The buffer revision the offsets were checked at. While it holds,
    /// the row is reused without looking at the buffer.
    revision: u64,
    /// Sorted by column.
    pub(crate) glyphs: Vec<PlacedGlyph>,
    /// Boxed invisible characters: their first column and cells.
    pub(crate) boxes: Vec<(usize, usize)>,
}

/// Shaped rows of one buffer, by display row: the shaping cache.
#[derive(Default)]
pub(crate) struct Rows {
    buffer: Option<BufferId>,
    /// The revision of the buffer this frame shows.
    revision: u64,
    rows: HashMap<usize, ShapedRow>,
    /// Scratch space for shaping a row.
    text: String,
    graphemes: Vec<Grapheme>,
    glyphs: Vec<ShapedGlyph>,
    placer: Placer,
    /// Rows shaped since the cache was created, for tests and logs.
    shaped: u64,
}

impl Rows {
    /// Forgets every row's shaping, for when the font changes.
    pub(crate) fn invalidate(&mut self) {
        self.rows.clear();
    }

    /// Starts a frame of `buffer` at `revision`: another buffer's rows are
    /// dropped.
    pub(crate) fn begin(&mut self, buffer: BufferId, revision: u64) {
        if self.buffer != Some(buffer) {
            self.buffer = Some(buffer);
            self.rows.clear();
        }
        self.revision = revision;
    }

    /// Drops cached rows outside `keep` once there are many more of them
    /// than a screen.
    pub(crate) fn evict(&mut self, keep: Range<usize>) {
        if self.rows.len() > 2 * keep.len() + 64 {
            self.rows.retain(|row, _| keep.contains(row));
        }
    }

    /// Rows shaped so far.
    pub(crate) fn shaped_count(&self) -> u64 {
        self.shaped
    }

    /// The shaped `columns` of `row`, shaping them if the cache has none
    /// for the text there now.
    pub(crate) fn row(
        &mut self,
        display: &DisplaySnapshot,
        row: usize,
        columns: Range<usize>,
        system: &mut dyn TextSystem,
    ) -> &ShapedRow {
        let revision = self.revision;
        let unchanged = self
            .rows
            .get(&row)
            .is_some_and(|shaped| shaped.revision == revision && shaped.columns == columns);
        if unchanged {
            return &self.rows[&row];
        }
        let offsets = display
            .display_point_to_offset(DisplayPoint::new(row, columns.start), Bias::Left)
            ..display.display_point_to_offset(DisplayPoint::new(row, columns.end), Bias::Right);
        let fresh = self
            .rows
            .get(&row)
            .is_some_and(|shaped| shaped.columns == columns && shaped.offsets == offsets);
        if !fresh {
            let mut shaped = self.rows.remove(&row).unwrap_or_default();
            self.shape(display, row, &columns, system, &mut shaped);
            shaped.columns = columns;
            shaped.offsets = offsets;
            self.rows.insert(row, shaped);
        }
        let shaped = self.rows.get_mut(&row).expect("inserted above");
        shaped.revision = revision;
        shaped
    }

    /// Shapes the clusters of `row` that start in `columns` into `out`.
    fn shape(
        &mut self,
        display: &DisplaySnapshot,
        row: usize,
        columns: &Range<usize>,
        system: &mut dyn TextSystem,
        out: &mut ShapedRow,
    ) {
        self.shaped += 1;
        self.text.clear();
        self.graphemes.clear();
        out.boxes.clear();
        let budget = columns.len().saturating_mul(CHARS_PER_COLUMN);
        for cluster in display.clusters(DisplayPoint::new(row, columns.start)) {
            if cluster.column >= columns.end {
                break;
            }
            let mut push = |text: &mut String, piece: &str, column, cells, style| {
                self.graphemes.push(Grapheme {
                    byte: text.len() as u32,
                    column,
                    cells,
                    style,
                });
                text.push_str(piece);
            };
            let cells = u32::try_from(cluster.cells).unwrap_or(u32::MAX);
            let chars = if self.text.len() < budget {
                MAX_CLUSTER_CHARS
            } else {
                1
            };
            match cluster.kind {
                CellKind::Text => push(
                    &mut self.text,
                    clip_cluster(cluster.text, chars),
                    cluster.column,
                    cells,
                    Style::Text,
                ),
                // A space keeps ligatures from forming across the tab, and
                // draws nothing.
                CellKind::Tab => push(&mut self.text, " ", cluster.column, cells, Style::Text),
                CellKind::Control(_) | CellKind::Invisible(_) | CellKind::InvalidByte(_) => {
                    let style = match cluster.kind {
                        CellKind::Invisible(_) => {
                            out.boxes.push((cluster.column, cluster.cells));
                            Style::Boxed
                        }
                        _ => Style::Escape,
                    };
                    let escape = cluster.kind.escape();
                    let label = escape.as_ref().map_or("?", |escape| escape.as_str());
                    for (i, c) in label.char_indices() {
                        let mut buf = [0; 4];
                        push(
                            &mut self.text,
                            c.encode_utf8(&mut buf),
                            cluster.column + i,
                            1,
                            style,
                        );
                    }
                }
                CellKind::LineBreak => break,
            }
        }
        system.shape(&self.text, &mut self.glyphs);
        self.placer.place(
            &self.graphemes,
            &self.glyphs,
            system.metrics().width,
            &mut out.glyphs,
        );
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
    use oso_core::buffer::Buffer;
    use oso_platform::text::{FontId, GlyphBitmap, GlyphId, GlyphKey};

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

    /// A text system with one glyph per character, one cell wide, that
    /// records what it shaped.
    #[derive(Default)]
    struct Recorder {
        shaped: Vec<String>,
    }

    impl TextSystem for Recorder {
        fn metrics(&self) -> CellMetrics {
            layout().metrics
        }

        fn shape(&mut self, text: &str, out: &mut Vec<ShapedGlyph>) {
            self.shaped.push(text.to_owned());
            out.clear();
            for (i, (byte, _)) in text.char_indices().enumerate() {
                out.push(ShapedGlyph {
                    key: GlyphKey {
                        font: FontId(0),
                        glyph: GlyphId(i as u16),
                    },
                    cluster: byte as u32,
                    x: i as f32 * 10.0,
                    y: 0.0,
                    advance: 10.0,
                });
            }
        }

        fn rasterize(&mut self, _key: GlyphKey) -> Option<GlyphBitmap> {
            None
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

    #[test]
    fn escapes_are_shaped_as_labels_never_as_raw_characters() {
        let buffer = Buffer::from_bytes("t", b"a\xe2\x80\xaeb\r\x07\xff\tc");
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let shaped = rows.row(&buffer.display(), 0, 0..128, &mut system);
        assert_eq!(system.shaped, ["aU+202Eb^M^G\\xFF c"]);
        assert_eq!(shaped.boxes, [(1, 6)]);
        let styles: Vec<_> = shaped.glyphs.iter().map(|g| (g.column, g.style)).collect();
        assert_eq!(styles[0], (0, Style::Text));
        assert_eq!(styles[1], (1, Style::Boxed));
        assert_eq!(styles[7], (7, Style::Text));
        assert_eq!(styles[8], (8, Style::Escape));
        assert_eq!(styles[12], (12, Style::Escape), "the invalid byte");
        assert_eq!(styles[16], (16, Style::Text), "the tab");
        assert_eq!(styles[17], (24, Style::Text), "after the tab stop");
    }

    #[test]
    fn rows_are_cached_by_line_and_reshaped_when_their_text_changes() {
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let buffer = Buffer::from_text("t", "one\ntwo\nthree");
        let display = buffer.display();
        let id = oso_core::editor::Editor::default().window().buffer();
        rows.begin(id, 1);
        rows.row(&display, 0, 0..128, &mut system);
        rows.row(&display, 1, 0..128, &mut system);
        rows.row(&display, 0, 0..128, &mut system);
        assert_eq!(rows.shaped_count(), 2, "row 0 came from the cache");
        // More text on row 2, as while loading, reshapes only row 2.
        rows.row(&display, 2, 0..128, &mut system);
        // The buffer changed: rows are checked against their offsets.
        rows.begin(id, 2);
        let longer = Buffer::from_text("t", "one\ntwo\nthree four");
        rows.row(&longer.display(), 2, 0..128, &mut system);
        rows.row(&longer.display(), 1, 0..128, &mut system);
        assert_eq!(rows.shaped_count(), 4);
        assert_eq!(system.shaped.last().unwrap(), "three four");
        // Other columns shape again.
        rows.row(&longer.display(), 1, 64..192, &mut system);
        assert_eq!(rows.shaped_count(), 5);
    }

    #[test]
    fn huge_clusters_are_cut_before_shaping() {
        let zalgo = format!("e{}", "\u{301}".repeat(100_000));
        let text = format!("{zalgo}\n{zalgo}");
        let buffer = Buffer::from_text("t", &text);
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        rows.row(&buffer.display(), 0, 0..256, &mut system);
        let shaped = system.shaped[0].chars().count();
        // The rope splits the cluster into 1 KB pieces of one cell each,
        // and each piece is cut to 32 characters.
        assert!(
            shaped <= 256 * (CHARS_PER_COLUMN + 1),
            "{shaped} characters shaped"
        );
    }

    #[test]
    fn a_long_line_shapes_only_the_columns_asked_for() {
        let line = "あ".repeat(1_000_000);
        let buffer = Buffer::from_text("t", &line);
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let shaped = rows.row(&buffer.display(), 0, 640..768, &mut system);
        assert_eq!(system.shaped[0].chars().count(), 64);
        assert_eq!(shaped.glyphs[0].column, 640);
    }

    proptest::proptest! {
        /// Whatever the file holds, the shaper never sees a control
        /// character or a bidi control (T3 in security.md).
        #[test]
        fn prop_the_shaper_never_sees_controls(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..200),
                                               extra in proptest::collection::vec(0usize..8, 0..20)) {
            let mut bytes = bytes;
            let specials = ["\u{202e}", "\u{2066}", "\u{200f}", "\u{61c}", "\r", "\u{85}", "\u{7f}", "👨\u{200d}👩"];
            for i in extra {
                bytes.extend_from_slice(specials[i].as_bytes());
            }
            let buffer = Buffer::from_bytes("t", &bytes);
            let display = buffer.display();
            let mut rows = Rows::default();
            let mut system = Recorder::default();
            for row in 0..display.row_count() {
                rows.row(&display, row, 0..256, &mut system);
            }
            for shaped in &system.shaped {
                for c in shaped.chars() {
                    let bidi = matches!(u32::from(c), 0x61c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069);
                    proptest::prop_assert!(!c.is_control() && !bidi, "{c:?} in {shaped:?}");
                }
            }
        }
    }
}
