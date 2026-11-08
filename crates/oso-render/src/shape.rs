//! Filled rectangles under the text: panes' backgrounds, the boxes of invisible
//! characters and the cursor box. Also rounded boxes with an outline and a soft shadow,
//! such as a float's panel, in a second pipeline of one instanced quad each (ADR 0075).
//!
//! Must not depend on: `atlas`, `grid`, `text` or anything else about text; it
//! knows pixels and colors.

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use oso_core::frame::{CellRect, PixelGrid};
use wgpu::util::DeviceExt;

use crate::pane::{Face, PANEL, Panel};

// ---- Vertex ----------------------------------------------------------------

/// Per-vertex data sent to the GPU: position + color.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

impl Vertex {
    /// Returns the wgpu vertex buffer layout descriptor for this type.
    fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                // location(0): position vec2<f32>
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                // location(1): color vec4<f32>
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        }
    }
}

/// A rounded box as the GPU takes it: seven `vec4`s (see `rounded.wgsl`).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct Rounded {
    /// x, y, width and height, in pixels.
    pub(crate) rect: [f32; 4],
    /// Top left, top right, bottom right and bottom left.
    pub(crate) radii: [f32; 4],
    /// Linear RGBA, straight alpha.
    pub(crate) fill: [f32; 4],
    /// The outline's color. It is drawn outside the box.
    pub(crate) outline: [f32; 4],
    pub(crate) shadow: [f32; 4],
    /// The outline's width, the shadow's offset down, its sigma, and how far past the box
    /// the quad reaches.
    pub(crate) params: [f32; 4],
    /// Nothing is drawn outside this: left, top, right and bottom.
    pub(crate) clip: [f32; 4],
}

impl Rounded {
    fn desc() -> wgpu::VertexBufferLayout<'static> {
        const ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4,
            4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Rounded>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRIBUTES,
        }
    }
}

/// The most rounded boxes in one frame: a float needs four.
pub(crate) const MAX_ROUNDED: usize = 64;

// ---- Uniforms (orthographic projection matrix) -----------------------------

/// CPU-side mirror of the WGSL `Uniforms` struct.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(crate) struct Uniforms {
    /// Column-major 4×4 orthographic projection matrix.
    ortho: [[f32; 4]; 4],
}

impl Uniforms {
    /// Builds an orthographic matrix that maps pixel coordinates
    /// [0, width] × [0, height] to wgpu NDC.
    ///
    /// X: [0, w] → [-1,  1]   (x_ndc =  2/w * x - 1)
    /// Y: [0, h] → [ 1, -1]   (y_ndc = -2/h * y + 1)  ← Y-axis flipped (down = positive)
    ///
    /// Column-major layout:
    ///   col 0: [2/w,    0,  0, 0]
    ///   col 1: [  0, -2/h,  0, 0]
    ///   col 2: [  0,    0,  1, 0]
    ///   col 3: [ -1,    1,  0, 1]
    pub(crate) fn new(width: f32, height: f32) -> Self {
        let w = width;
        let h = height;
        Self {
            ortho: [
                [2.0 / w, 0.0, 0.0, 0.0],  // col 0
                [0.0, -2.0 / h, 0.0, 0.0], // col 1
                [0.0, 0.0, 1.0, 0.0],      // col 2
                [-1.0, 1.0, 0.0, 1.0],     // col 3
            ],
        }
    }
}

// ---- ShapeRenderer ---------------------------------------------------------

/// Renders solid shapes (cursor rectangle, selection highlight, etc.) using
/// raw wgpu quads. Vello is intentionally not used (see §5.5).
pub struct ShapeRenderer {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    /// Rectangles `vertex_buffer` holds.
    capacity: usize,
    /// This frame's vertices, reused across frames.
    vertices: Vec<Vertex>,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    rounded_pipeline: wgpu::RenderPipeline,
    /// Holds [`MAX_ROUNDED`] boxes, written only in a frame that has some.
    rounded_buffer: wgpu::Buffer,
}

/// A filled rectangle: its top-left corner, size and color, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub(crate) position: [f32; 2],
    pub(crate) size: [f32; 2],
    pub(crate) color: [f32; 4],
}

/// Vertices per rectangle: two triangles.
const RECT_VERTICES: usize = 6;

/// Rectangles the vertex buffer holds before it first grows.
const INITIAL_RECTS: usize = 64;

/// The most rectangles in one frame: a screen of boxed characters needs
/// far fewer.
pub(crate) const MAX_RECTS: usize = 1 << 14;

impl ShapeRenderer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: f32,
        height: f32,
    ) -> Self {
        // Compile the WGSL shader module.
        let shader = device.create_shader_module(wgpu::include_wgsl!("cursor.wgsl"));

        // Uniform buffer (orthographic projection matrix).
        let uniforms = Uniforms::new(width, height);
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Shape Uniform Buffer"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Bind group layout.
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shape Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // Bind group.
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Shape Bind Group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Pipeline layout.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Shape Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // Render pipeline.
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Shape Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let rounded_pipeline = Self::rounded_pipeline(device, format, &pipeline_layout);
        Self {
            pipeline,
            rounded_pipeline,
            rounded_buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Rounded Instance Buffer"),
                size: (std::mem::size_of::<Rounded>() * MAX_ROUNDED) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            vertex_buffer: Self::vertex_buffer(device, INITIAL_RECTS),
            capacity: INITIAL_RECTS,
            vertices: Vec::with_capacity(INITIAL_RECTS * RECT_VERTICES),
            uniform_buffer,
            bind_group,
        }
    }

    /// The pipeline of rounded boxes: instanced quads, premultiplied (ADR 0075).
    fn rounded_pipeline(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        layout: &wgpu::PipelineLayout,
    ) -> wgpu::RenderPipeline {
        let rounded_shader = device.create_shader_module(wgpu::include_wgsl!("rounded.wgsl"));
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Rounded Pipeline"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &rounded_shader,
                entry_point: Some("vs_main"),
                buffers: &[Rounded::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &rounded_shader,
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
        })
    }

    fn vertex_buffer(device: &wgpu::Device, rects: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shape Vertex Buffer"),
            size: (std::mem::size_of::<Vertex>() * RECT_VERTICES * rects) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Updates the orthographic projection matrix when the window is resized.
    pub fn resize(&self, queue: &wgpu::Queue, width: f32, height: f32) {
        let uniforms = Uniforms::new(width, height);
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Uploads this frame's rectangles, at most [`MAX_RECTS`] of them. Must be
    /// called before the render pass begins.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rects: &[Rect],
        rounded: &[Rounded],
    ) {
        // A frame with no float writes nothing here.
        if !rounded.is_empty() {
            let rounded = &rounded[..rounded.len().min(MAX_ROUNDED)];
            queue.write_buffer(&self.rounded_buffer, 0, bytemuck::cast_slice(rounded));
        }
        self.vertices.clear();
        for rect in rects {
            let ([x, y], [w, h], color) = (rect.position, rect.size, rect.color);
            let corner = |position| Vertex { position, color };
            //  (x, y) ---- (x+w, y)
            //    |      /     |
            //  (x, y+h) -- (x+w, y+h)
            self.vertices.extend([
                corner([x, y]),
                corner([x + w, y]),
                corner([x, y + h]),
                corner([x + w, y]),
                corner([x + w, y + h]),
                corner([x, y + h]),
            ]);
        }
        if rects.len() > self.capacity {
            self.capacity = rects.len().next_power_of_two().min(MAX_RECTS);
            self.vertex_buffer = Self::vertex_buffer(device, self.capacity);
        }
        if !self.vertices.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
        }
    }

    /// Records the rectangles in `range`, in order, into the given render pass.
    pub(crate) fn render(&self, pass: &mut wgpu::RenderPass<'_>, range: Range<u32>) {
        if range.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        let vertices = RECT_VERTICES as u32;
        pass.draw(range.start * vertices..range.end * vertices, 0..1);
    }

    /// Records the rounded boxes in `range`, in order: one quad each.
    pub(crate) fn render_rounded(&self, pass: &mut wgpu::RenderPass<'_>, range: Range<u32>) {
        if range.is_empty() {
            return;
        }
        pass.set_pipeline(&self.rounded_pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.rounded_buffer.slice(..));
        pass.draw(0..4, range);
    }
}

// ---- A float's panel ------------------------------------------------------

/// One point in pixels: the cell's height over 16, so that a panel follows the font's size
/// as well as the display's scale.
fn point(cell: [f32; 2]) -> f32 {
    cell[1] / 16.0
}

/// A float's panel, its field and its keys as rounded boxes, cut to `region`, and on the
/// frame's top row also to the margin above it, which the panel covers up to the surface's
/// edge (ADR 0075). Cells are `cell` pixels. Its colors and sizes are read from the faces
/// and [`PANEL`] now, as a theme will switch them.
pub(crate) fn panel(
    panel: &Panel<'_>,
    region: CellRect,
    cell: [f32; 2],
) -> impl Iterator<Item = Rounded> {
    let grid = PixelGrid::new(cell[0], cell[1]);
    let pt = point(cell);
    let style = PANEL;
    let mut clip = grid.rect(region);
    if region.row == 0 {
        clip.height += clip.y;
        clip.y = 0.0;
    }
    let clip = [clip.x, clip.y, clip.x + clip.width, clip.y + clip.height];
    let mut fill = grid.rect(panel.cells);
    let attached = panel.cells.row == 0;
    if attached {
        fill.height += fill.y;
        fill.y = 0.0;
    }
    let radius = style.radius * pt;
    let top = if attached { 0.0 } else { radius };
    let hairline = (style.hairline * pt).round().max(1.0);
    let sigma = style.shadow_sigma * pt;
    let down = style.shadow_down * pt;
    let body = Rounded {
        rect: [fill.x, fill.y, fill.width, fill.height],
        radii: [top, top, radius, radius],
        fill: Face::Float.background(),
        outline: Face::FloatBorder.background(),
        shadow: style.shadow,
        params: [hairline, down, sigma, 3.0 * sigma + down + hairline + 1.0],
        clip,
    };
    let field = panel
        .field
        .filter(|field| field.rows > 0 && field.columns > 0)
        .map(|field| {
            let cells = grid.rect(field);
            let outline = (style.field_outline * pt).round().max(1.0);
            let [across, up] = style
                .field_reach
                .map(|reach| (reach * pt).round() - outline);
            Rounded {
                rect: [
                    cells.x - across,
                    cells.y - up,
                    cells.width + 2.0 * across,
                    cells.height + 2.0 * up,
                ],
                radii: [style.field_radius * pt; 4],
                fill: Face::Field.background(),
                outline: Face::Field.foreground(),
                shadow: [0.0; 4],
                params: [outline, 0.0, 0.0, outline + 1.0],
                clip,
            }
        });
    let keys = panel
        .keys
        .iter()
        .filter(|key| key.rows > 0 && key.columns > 0)
        .map(move |key| {
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
                params: [0.0, 0.0, 0.0, 1.0],
                clip,
            }
        });
    std::iter::once(body).chain(field).chain(keys)
}
