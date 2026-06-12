use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

// Cursor size constants.
pub const CURSOR_W: f32 = 2.0;
pub const CURSOR_H: f32 = 20.0;

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

// ---- Uniforms (orthographic projection matrix) -----------------------------

/// CPU-side mirror of the WGSL `Uniforms` struct.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
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
    fn new(width: f32, height: f32) -> Self {
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
    index_buffer: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

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

        // Vertex buffer pre-allocated for 4 vertices (one quad).
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shape Vertex Buffer"),
            size: (std::mem::size_of::<Vertex>() * 4) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Index buffer: 2 triangles (6 indices, never changes).
        //
        //  3 --- 2
        //  |  \  |
        //  0 --- 1
        //
        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Shape Index Buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Self {
            pipeline,
            vertex_buffer,
            index_buffer,
            uniform_buffer,
            bind_group,
        }
    }

    /// Updates the orthographic projection matrix when the window is resized.
    pub fn resize(&self, queue: &wgpu::Queue, width: f32, height: f32) {
        let uniforms = Uniforms::new(width, height);
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Writes the cursor quad vertices to the vertex buffer.
    /// Must be called before the render pass begins.
    pub fn prepare(&self, queue: &wgpu::Queue, cursor_x: f32, cursor_y: f32) {
        // Near-white cursor color.
        let color = [0.90_f32, 0.90, 0.92, 1.0];

        //  3(x, y+h) --- 2(x+w, y+h)
        //  |                         |
        //  0(x,   y) --- 1(x+w,   y)
        let x = cursor_x;
        let y = cursor_y;
        let w = CURSOR_W;
        let h = CURSOR_H;

        let vertices = [
            Vertex {
                position: [x, y],
                color,
            },
            Vertex {
                position: [x + w, y],
                color,
            },
            Vertex {
                position: [x + w, y + h],
                color,
            },
            Vertex {
                position: [x, y + h],
                color,
            },
        ];

        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&vertices));
    }

    /// Records the cursor rectangle draw commands into the given render pass.
    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..6, 0, 0..1);
    }
}
