mod shape;
use shape::ShapeRenderer;

use std::sync::Arc;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::editor::cursor::Cursor;

// Top-left margin (will become the line-number gutter offset in the future).
const MARGIN_X: f32 = 8.0;
const MARGIN_Y: f32 = 8.0;

// Pixel width per cursor position unit (placeholder until cosmic-text is integrated).
// Validation test per §7.3: each key press shifts the cursor by +10 px.
const CHAR_WIDTH_PX: f32 = 10.0;

/// Calculates the pixel X coordinate of the cursor from its logical position.
///
/// This is a pure function with no GPU dependency, enabling straightforward
/// unit testing independent of the wgpu context.
///
/// # Arguments
/// - `position`   – byte offset of the cursor in the text buffer
/// - `char_width` – pixel width per character position (current placeholder: 10.0)
/// - `margin_x`   – left margin in pixels (current placeholder: 8.0)
pub(crate) fn cursor_pixel_x(position: usize, char_width: f32, margin_x: f32) -> f32 {
    position as f32 * char_width + margin_x
}

/// Manages the wgpu device, queue, and surface; issues frame draws.
///
/// Render pass layout (§5.6 — separated pass design):
///   Pass 1: ShapeRenderer  — clear background + cursor rectangle
///   Pass 2: TextRenderer   — (added in a future milestone)
pub struct RenderState {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    shape_renderer: ShapeRenderer,
}

impl RenderState {
    pub async fn new(window: Window) -> Self {
        let window = Arc::new(window);
        let size = window.inner_size();

        // 1. Create a wgpu instance (abstracts Vulkan, Metal, DX12, etc.).
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: Default::default(),
            display: None,
        });

        // 2. Create a surface tied to the window.
        let surface = instance.create_surface(window.clone()).unwrap();

        // 3. Request a physical GPU adapter.
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .unwrap();

        // 4. Request a logical device and a command queue.
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .unwrap();

        // 5. Configure the surface (format, dimensions, present mode, etc.).
        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: surface_caps.present_modes[0],
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // 6. Initialize the shape renderer.
        let shape_renderer = ShapeRenderer::new(
            &device,
            surface_format,
            size.width as f32,
            size.height as f32,
        );

        Self {
            window,
            surface,
            device,
            queue,
            config,
            shape_renderer,
        }
    }

    /// Reconfigures the surface and updates the orthographic matrix on window resize.
    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
            self.shape_renderer
                .resize(&self.queue, new_size.width as f32, new_size.height as f32);
            self.window.request_redraw();
        }
    }

    /// Renders a single frame.
    ///
    /// Converting `Cursor.position` to pixel coordinates is the renderer's
    /// responsibility (§7.1); the `Cursor` struct itself stays free of pixel data.
    pub fn render(&mut self, cursor: &Cursor) -> Result<(), String> {
        // Translate cursor.position → screen X via the pure coordinate function.
        let cursor_x = cursor_pixel_x(cursor.position, CHAR_WIDTH_PX, MARGIN_X);
        let cursor_y = MARGIN_Y;

        // Acquire the next surface texture.
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex) => tex,
            wgpu::CurrentSurfaceTexture::Suboptimal(tex) => tex,
            other => return Err(format!("{other:?}")),
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        // --- Pass 1: clear background + cursor rectangle (ShapeRenderer) ---
        self.shape_renderer.prepare(&self.queue, cursor_x, cursor_y);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Pass 1: Shape"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Clear with the editor background color.
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.07,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            self.shape_renderer.render(&mut pass);
        }

        // --- Pass 2+: text rendering (added in a future milestone) ---

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::cursor_pixel_x;
    use proptest::prelude::*;

    // --- Deterministic unit tests ---

    #[test]
    fn pixel_x_at_position_zero_equals_margin() {
        assert_eq!(cursor_pixel_x(0, 10.0, 8.0), 8.0);
    }

    #[test]
    fn pixel_x_at_position_one() {
        assert_eq!(cursor_pixel_x(1, 10.0, 8.0), 18.0);
    }

    #[test]
    fn pixel_x_at_position_ten() {
        assert_eq!(cursor_pixel_x(10, 10.0, 8.0), 108.0);
    }

    #[test]
    fn pixel_x_scales_linearly() {
        let base = cursor_pixel_x(0, 10.0, 8.0);
        let next = cursor_pixel_x(1, 10.0, 8.0);
        assert_eq!(next - base, 10.0);
    }

    // --- Property-based tests (proptest) ---

    proptest! {
        /// The pixel X coordinate must always be >= margin_x for any position.
        #[test]
        fn prop_pixel_x_never_less_than_margin(pos in 0usize..10_000usize) {
            let margin = 8.0_f32;
            let x = cursor_pixel_x(pos, 10.0, margin);
            prop_assert!(x >= margin, "x={x} was less than margin={margin}");
        }

        /// Pixel X must increase monotonically as position increases.
        #[test]
        fn prop_pixel_x_is_monotone(a in 0usize..10_000usize, b in 0usize..10_000usize) {
            let xa = cursor_pixel_x(a, 10.0, 8.0);
            let xb = cursor_pixel_x(b, 10.0, 8.0);
            if a < b {
                prop_assert!(xa < xb);
            } else if a > b {
                prop_assert!(xa > xb);
            } else {
                prop_assert_eq!(xa, xb);
            }
        }
    }
}
