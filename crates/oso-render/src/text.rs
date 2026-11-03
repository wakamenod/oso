//! Pass 2: text drawn as glyph quads on the cell grid, from the atlas
//! (ADR 0005, ADR 0036).
//!
//! Rows are shaped when they first become visible and kept until their text
//! or the font changes. Glyphs are rasterized once, into the atlas. A frame
//! only turns the visible rows' cached glyphs into quads.

use bytemuck::{Pod, Zeroable};
use oso_platform::text::{CellMetrics, GlyphFormat, ShapedGlyph, TextSystem};
use wgpu::util::DeviceExt;

use crate::atlas::{AtlasGlyph, GlyphAtlas};
use crate::shape::Uniforms;

/// Instances the buffer holds before it first grows.
const INITIAL_INSTANCES: usize = 1024;

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
    /// The cell under the cursor, as `(row, column)`. Its glyph is drawn in
    /// `cursor_color` to show on the box cursor.
    pub(crate) cursor: (u32, u32),
    pub(crate) color: [f32; 4],
    pub(crate) cursor_color: [f32; 4],
}

impl Layout {
    /// The top-left corner of a cell, in pixels.
    pub(crate) fn cell(&self, row: u32, column: u32) -> [f32; 2] {
        [
            self.origin[0] + column as f32 * self.metrics.width,
            self.origin[1] + row as f32 * self.metrics.height,
        ]
    }

    /// The rows that are at least partly inside the viewport.
    pub(crate) fn visible_rows(&self, rows: usize) -> usize {
        let space = self.viewport[1] - self.origin[1];
        if space <= 0.0 {
            return 0;
        }
        let visible = (space / self.metrics.height).ceil() as usize;
        visible.min(rows)
    }

    /// The quad for one glyph of `row`, or `None` if it is off screen.
    pub(crate) fn quad(
        &self,
        row: u32,
        glyph: &ShapedGlyph,
        entry: &AtlasGlyph,
    ) -> Option<GlyphInstance> {
        let [cell_x, cell_y] = self.cell(row, glyph.column);
        let x = cell_x + glyph.offset[0] + entry.left as f32;
        let y = cell_y + self.metrics.ascent + glyph.offset[1] - entry.top as f32;
        let [w, h] = [entry.size[0] as f32, entry.size[1] as f32];
        if x >= self.viewport[0] || y >= self.viewport[1] || x + w <= 0.0 || y + h <= 0.0 {
            return None;
        }
        let (cursor_row, cursor_column) = self.cursor;
        let under_cursor = row == cursor_row
            && (glyph.column..glyph.column + u32::from(glyph.cells)).contains(&cursor_column);
        Some(GlyphInstance {
            position: [x, y],
            size: [w, h],
            uv: [entry.uv[0] as f32, entry.uv[1] as f32],
            color: if under_cursor {
                self.cursor_color
            } else {
                self.color
            },
            kind: match entry.format {
                GlyphFormat::Coverage => 0,
                GlyphFormat::Color => 1,
            },
        })
    }
}

/// Rows of text and their shaped glyphs, shaped on first use.
#[derive(Default)]
pub(crate) struct Rows {
    text: Vec<String>,
    shaped: Vec<Option<Vec<ShapedGlyph>>>,
}

impl Rows {
    /// Replaces the text, keeping the shaping of rows whose text is the
    /// same.
    pub(crate) fn set<S: AsRef<str>>(&mut self, rows: &[S]) {
        self.text.truncate(rows.len());
        self.shaped.truncate(rows.len());
        for (i, row) in rows.iter().enumerate() {
            let row = row.as_ref();
            match self.text.get_mut(i) {
                Some(text) if text == row => {}
                Some(text) => {
                    row.clone_into(text);
                    self.shaped[i] = None;
                }
                None => {
                    self.text.push(row.to_owned());
                    self.shaped.push(None);
                }
            }
        }
    }

    /// Forgets every row's shaping, for when the font changes.
    pub(crate) fn invalidate(&mut self) {
        self.shaped.iter_mut().for_each(|s| *s = None);
    }

    pub(crate) fn len(&self) -> usize {
        self.text.len()
    }

    /// The cluster that covers `column` of `row`, as its first column and
    /// its cells, so that a box cursor covers a whole full-width character.
    /// One cell past the end of the text, or in a row not shaped yet.
    pub(crate) fn cluster_at(&self, row: usize, column: u32) -> (u32, u32) {
        let glyphs = self
            .shaped
            .get(row)
            .and_then(Option::as_deref)
            .unwrap_or(&[]);
        // Glyphs are in column order.
        let i = glyphs.partition_point(|g| g.column + u32::from(g.cells) <= column);
        match glyphs.get(i) {
            Some(g) if g.column <= column => (g.column, u32::from(g.cells)),
            _ => (column, 1),
        }
    }

    /// The glyphs of row `i`, shaping it if needed.
    fn shaped(&mut self, i: usize, system: &mut dyn TextSystem) -> &[ShapedGlyph] {
        self.shaped[i].get_or_insert_with(|| {
            let mut glyphs = Vec::new();
            system.shape(&self.text[i], &mut glyphs);
            glyphs
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

    /// Builds this frame's quads from the visible rows and uploads them.
    /// Must be called before the render pass begins.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        system: &mut dyn TextSystem,
        rows: &mut Rows,
        layout: &Layout,
    ) {
        self.instances.clear();
        for i in 0..layout.visible_rows(rows.len()) {
            for glyph in rows.shaped(i, system) {
                let Some(entry) = self.atlas.get(device, queue, system, glyph.key) else {
                    continue;
                };
                if let Some(quad) = layout.quad(i as u32, glyph, &entry) {
                    self.instances.push(quad);
                }
            }
        }
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
            self.capacity = self.instances.len().next_power_of_two();
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
            cursor: (0, 3),
            color: WHITE,
            cursor_color: BLACK,
        }
    }

    fn glyph(column: u32, cells: u8) -> ShapedGlyph {
        ShapedGlyph {
            key: GlyphKey {
                font: FontId(0),
                glyph: GlyphId(1),
            },
            column,
            cells,
            cluster: 0,
            offset: [0.0, 0.0],
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
    fn cells_are_column_times_width() {
        let l = layout();
        assert_eq!(l.cell(0, 0), [8.0, 8.0]);
        assert_eq!(l.cell(0, 3), [38.0, 8.0]);
        assert_eq!(l.cell(2, 1), [18.0, 48.0]);
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
    fn glyph_under_the_cursor_uses_the_cursor_color() {
        let l = layout();
        assert_eq!(l.quad(0, &glyph(3, 1), &entry()).unwrap().color, BLACK);
        assert_eq!(l.quad(0, &glyph(2, 1), &entry()).unwrap().color, WHITE);
        assert_eq!(l.quad(1, &glyph(3, 1), &entry()).unwrap().color, WHITE);
        // A full-width glyph in columns 2 and 3 is under a cursor at 3.
        assert_eq!(l.quad(0, &glyph(2, 2), &entry()).unwrap().color, BLACK);
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
    fn only_rows_in_the_viewport_are_visible() {
        let l = layout();
        // (100 - 8) / 20 = 4.6, so 5 rows are at least partly visible.
        assert_eq!(l.visible_rows(100), 5);
        assert_eq!(l.visible_rows(2), 2);
        let tiny = Layout {
            viewport: [200.0, 4.0],
            ..l
        };
        assert_eq!(tiny.visible_rows(100), 0);
    }

    #[test]
    fn cursor_covers_the_whole_cluster() {
        let mut rows = Rows::default();
        rows.set(&["a日b"]);
        rows.shaped[0] = Some(vec![glyph(0, 1), glyph(1, 2), glyph(3, 1)]);
        assert_eq!(rows.cluster_at(0, 0), (0, 1));
        assert_eq!(rows.cluster_at(0, 1), (1, 2));
        assert_eq!(rows.cluster_at(0, 2), (1, 2), "right half of 日");
        assert_eq!(rows.cluster_at(0, 3), (3, 1));
        assert_eq!(rows.cluster_at(0, 9), (9, 1), "past the end");
        assert_eq!(rows.cluster_at(5, 2), (2, 1), "no such row");
    }

    #[test]
    fn rows_keep_shaping_of_unchanged_text() {
        let mut rows = Rows::default();
        rows.set(&["a", "b"]);
        rows.shaped = vec![Some(vec![glyph(0, 1)]), Some(vec![glyph(0, 1)])];
        rows.set(&["a", "c", "d"]);
        assert!(rows.shaped[0].is_some(), "unchanged row kept");
        assert!(rows.shaped[1].is_none(), "changed row reshaped");
        assert!(rows.shaped[2].is_none(), "new row shaped");
        rows.set(&["a"]);
        assert_eq!(rows.len(), 1);
        rows.invalidate();
        assert!(rows.shaped[0].is_none());
    }
}
