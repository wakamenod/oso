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

/// The single wgpu backend used on this OS (ADR 0003). There is no GL fallback.
pub(crate) fn platform_backend() -> wgpu::Backends {
    if cfg!(target_os = "macos") {
        wgpu::Backends::METAL
    } else if cfg!(target_os = "windows") {
        wgpu::Backends::DX12
    } else {
        wgpu::Backends::VULKAN
    }
}

/// What to do after asking the surface for the next texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceAction {
    /// Draw and present the frame.
    Draw,
    /// Draw and present the frame, then configure the surface again.
    DrawThenReconfigure,
    /// Configure the surface again and retry the frame.
    Reconfigure,
    /// Create the surface again, configure it and retry the frame.
    Recreate,
    /// Skip this frame and retry it.
    Retry,
    /// Skip this frame. A later event requests the next one.
    Skip,
    /// Skip this frame and report an error.
    Fail,
}

/// Frames retried in a row before giving up until the next event.
const MAX_RETRIES: u8 = 3;

/// Counts one more frame that failed to draw. Returns the new count and
/// whether to request another redraw at once.
///
/// Under `ControlFlow::Wait` a retry that never succeeds would redraw as fast
/// as the event loop runs, so retries stop after `MAX_RETRIES`. The next
/// event (input, resize, visibility) starts a new attempt.
pub(crate) fn count_failed_frame(failed_frames: u8) -> (u8, bool) {
    let failed_frames = failed_frames.saturating_add(1);
    (failed_frames, failed_frames <= MAX_RETRIES)
}

/// Maps the result of `Surface::get_current_texture` to the recovery that
/// wgpu documents for it.
pub(crate) fn surface_action(status: &wgpu::CurrentSurfaceTexture) -> SurfaceAction {
    use wgpu::CurrentSurfaceTexture as S;
    match status {
        S::Success(_) => SurfaceAction::Draw,
        S::Suboptimal(_) => SurfaceAction::DrawThenReconfigure,
        S::Outdated => SurfaceAction::Reconfigure,
        S::Lost => SurfaceAction::Recreate,
        S::Timeout => SurfaceAction::Retry,
        // Occluded windows get `WindowEvent::Occluded(false)` when visible again.
        S::Occluded => SurfaceAction::Skip,
        S::Validation => SurfaceAction::Fail,
    }
}

/// A frame that could not be drawn. None of these are fatal: the frame is
/// skipped, and the next requested frame tries again.
#[derive(Debug)]
pub enum RenderError {
    /// Acquiring the surface texture raised a validation error.
    SurfaceValidation,
    /// The surface still failed after `MAX_RETRIES` retries in a row.
    RetriesExhausted,
    /// Creating the surface again after it was lost failed.
    RecreateSurface(wgpu::CreateSurfaceError),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SurfaceValidation => {
                f.write_str("validation error acquiring the surface texture")
            }
            Self::RetriesExhausted => write!(
                f,
                "surface did not recover after {MAX_RETRIES} retries; waiting for the next event"
            ),
            Self::RecreateSurface(e) => write!(f, "could not recreate the lost surface: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Manages the wgpu device, queue, and surface; issues frame draws.
///
/// Render pass layout (§5.6 — separated pass design):
///   Pass 1: ShapeRenderer  — clear background + cursor rectangle
///   Pass 2: TextRenderer   — (added in a future milestone)
pub struct RenderState {
    pub window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    shape_renderer: ShapeRenderer,
    /// Frames in a row that failed to draw; see `count_failed_frame`.
    failed_frames: u8,
}

impl RenderState {
    pub async fn new(window: Window) -> Self {
        let window = Arc::new(window);
        let size = window.inner_size();

        // 1. Create a wgpu instance for this OS's backend.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: platform_backend(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: Default::default(),
            display: None,
        });

        // 2. Create a surface tied to the window.
        let surface = instance.create_surface(window.clone()).unwrap();

        // 3. Request a physical GPU adapter. An editor draws very little, and
        //    low power keeps dual-GPU machines on the integrated GPU.
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
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

        // Fifo is the only present mode every backend must support, and it
        // never tears. Frames are drawn on demand, so vsync costs nothing while
        // idle. A frame latency of 1 keeps at most one frame queued, aiming at
        // the p99 <= 8 ms key-to-photon budget. A second frame within one
        // refresh may still wait for vblank. Once ADR 0025 measures this,
        // Mailbox, Immediate or a latency of 2 can be revisited.
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &config);

        let info = adapter.get_info();
        log::debug!(
            "wgpu backend {:?}, adapter {:?} ({:?}), present mode {:?}, frame latency {}",
            info.backend,
            info.name,
            info.device_type,
            config.present_mode,
            config.desired_maximum_frame_latency,
        );

        // 6. Initialize the shape renderer.
        let shape_renderer = ShapeRenderer::new(
            &device,
            surface_format,
            size.width as f32,
            size.height as f32,
        );

        Self {
            window,
            instance,
            surface,
            device,
            queue,
            config,
            shape_renderer,
            failed_frames: 0,
        }
    }

    /// Reconfigures the surface and updates the orthographic matrix on window resize.
    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if self.configure(new_size) {
            self.window.request_redraw();
        }
    }

    /// Configures the surface and the orthographic matrix for `size`.
    /// Returns false, and does nothing, for a zero-sized (minimized) window.
    fn configure(&mut self, size: PhysicalSize<u32>) -> bool {
        if size.width == 0 || size.height == 0 {
            return false;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.shape_renderer
            .resize(&self.queue, size.width as f32, size.height as f32);
        true
    }

    /// Recovers from a surface that gave no texture, and requests a redraw
    /// to retry the frame unless retries are exhausted.
    fn recover(&mut self, action: SurfaceAction) -> Result<(), RenderError> {
        match action {
            SurfaceAction::Reconfigure => {
                self.configure(self.window.inner_size());
            }
            SurfaceAction::Recreate => {
                self.surface = self
                    .instance
                    .create_surface(self.window.clone())
                    .map_err(RenderError::RecreateSurface)?;
                self.surface.configure(&self.device, &self.config);
            }
            SurfaceAction::Retry => {}
            SurfaceAction::Fail => return Err(RenderError::SurfaceValidation),
            SurfaceAction::Skip | SurfaceAction::Draw | SurfaceAction::DrawThenReconfigure => {
                return Ok(());
            }
        }
        let retry;
        (self.failed_frames, retry) = count_failed_frame(self.failed_frames);
        if retry {
            self.window.request_redraw();
            Ok(())
        } else if self.failed_frames == MAX_RETRIES + 1 {
            Err(RenderError::RetriesExhausted)
        } else {
            Ok(())
        }
    }

    /// Renders a single frame.
    ///
    /// Converting `Cursor.position` to pixel coordinates is the renderer's
    /// responsibility (§7.1); the `Cursor` struct itself stays free of pixel data.
    pub fn render(&mut self, cursor: &Cursor) -> Result<(), RenderError> {
        // Translate cursor.position → screen X via the pure coordinate function.
        let cursor_x = cursor_pixel_x(cursor.position, CHAR_WIDTH_PX, MARGIN_X);
        let cursor_y = MARGIN_Y;

        // Acquire the next surface texture, or recover as wgpu documents.
        let current = self.surface.get_current_texture();
        let action = surface_action(&current);
        let output = match current {
            wgpu::CurrentSurfaceTexture::Success(tex)
            | wgpu::CurrentSurfaceTexture::Suboptimal(tex) => tex,
            other => {
                log::debug!("no surface texture: {other:?}");
                return self.recover(action);
            }
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

        self.failed_frames = 0;

        // Configure only after presenting: no surface texture may be alive.
        // Only a size change is fixed here; reconfiguring with the same size
        // would cost a GPU wait on every frame without clearing the status.
        if action == SurfaceAction::DrawThenReconfigure {
            let size = self.window.inner_size();
            if size.width != self.config.width || size.height != self.config.height {
                self.configure(size);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        count_failed_frame, cursor_pixel_x, platform_backend, surface_action, SurfaceAction,
        MAX_RETRIES,
    };
    use proptest::prelude::*;
    use wgpu::CurrentSurfaceTexture as S;

    // --- Surface status mapping ---
    // `Success` and `Suboptimal` carry a texture and need a GPU to construct.

    #[test]
    fn outdated_reconfigures() {
        assert_eq!(surface_action(&S::Outdated), SurfaceAction::Reconfigure);
    }

    #[test]
    fn lost_recreates_the_surface() {
        assert_eq!(surface_action(&S::Lost), SurfaceAction::Recreate);
    }

    #[test]
    fn timeout_retries_the_frame() {
        assert_eq!(surface_action(&S::Timeout), SurfaceAction::Retry);
    }

    #[test]
    fn occluded_skips_the_frame() {
        assert_eq!(surface_action(&S::Occluded), SurfaceAction::Skip);
    }

    #[test]
    fn validation_fails_the_frame() {
        assert_eq!(surface_action(&S::Validation), SurfaceAction::Fail);
    }

    // --- Retry limit ---

    #[test]
    fn retries_stop_after_the_limit() {
        let mut failed = 0;
        let mut retries = 0;
        for _ in 0..100 {
            let retry;
            (failed, retry) = count_failed_frame(failed);
            retries += u32::from(retry);
        }
        assert_eq!(retries, u32::from(MAX_RETRIES));
    }

    #[test]
    fn failed_frame_count_saturates() {
        assert_eq!(count_failed_frame(u8::MAX), (u8::MAX, false));
    }

    // --- Backend selection ---

    #[test]
    fn exactly_one_backend_and_never_gl() {
        let backends = platform_backend();
        assert_eq!(backends.iter().count(), 1, "{backends:?}");
        assert!(!backends.contains(wgpu::Backends::GL));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_uses_metal() {
        assert_eq!(platform_backend(), wgpu::Backends::METAL);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_uses_dx12() {
        assert_eq!(platform_backend(), wgpu::Backends::DX12);
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn other_systems_use_vulkan() {
        assert_eq!(platform_backend(), wgpu::Backends::VULKAN);
    }

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
