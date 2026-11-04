//! The wgpu rendering pipeline (ADR 0004).
//!
//! Must not depend on: winit or `oso`. The caller owns the window, passes it
//! in as a [`wgpu::DisplayAndWindowHandle`] and its size as plain pixels, and
//! requests redraws when told to through [`Redraw`].

mod atlas;
#[cfg(feature = "dev")]
mod capture;
mod grid;
mod rows;
mod shape;
mod text;
use atlas::RasterBudget;
#[cfg(feature = "dev")]
pub use capture::{Capture, CaptureError, Image};
use rows::Rows;
use shape::{Rect, ShapeRenderer};
use text::{Layout, TextRenderer};

use std::sync::Arc;

use oso_core::buffer::BufferId;
use oso_core::display_map::{DisplayPoint, DisplaySnapshot};
use oso_core::motion;
use oso_core::window::Scroll;
use oso_platform::text::{CellMetrics, TextSystem};

// Top-left margin, in physical pixels (will become the line-number gutter
// offset in the future).
const MARGIN_X: f32 = 8.0;
const MARGIN_Y: f32 = 8.0;

// Colors in linear RGB, until the theme of ADR 0027.
const BACKGROUND: [f32; 4] = [0.05, 0.05, 0.07, 1.0];
const FOREGROUND: [f32; 4] = [0.90, 0.90, 0.92, 1.0];
const CURSOR: [f32; 4] = [0.90, 0.90, 0.92, 1.0];
/// Escapes (`^M`, `\xFF`) and boxed code points (`U+202E`), so they are
/// not mistaken for text that reads the same.
const ESCAPE: [f32; 4] = [0.85, 0.45, 0.20, 1.0];
const ESCAPE_BOX: [f32; 4] = [0.22, 0.10, 0.05, 1.0];

/// New glyphs a frame rasterizes at most. Each takes tens of
/// microseconds, so a screen of new CJK text fills in over a few frames
/// instead of stalling one (ADR 0040).
const GLYPHS_PER_FRAME: usize = 64;

/// What to draw: a window's view of a buffer.
pub struct TextView {
    /// The buffer's display model.
    pub display: DisplaySnapshot,
    /// The buffer, so that cached shaping of another one is not reused.
    pub buffer: BufferId,
    /// The buffer's revision: while it holds, cached rows are reused
    /// without looking at the buffer.
    pub revision: u64,
    /// The display row and column at the top left.
    pub scroll: Scroll,
    /// Where the cursor box goes.
    pub cursor: DisplayPoint,
}

/// The single wgpu backend used on this OS (ADR 0003). There is no GL fallback.
///
/// These conditions match the per-target wgpu features in this crate's
/// `Cargo.toml`, so this is the only backend compiled in.
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
/// wgpu documents for it. `None` means there is no surface, because creating
/// it again failed, so it is created again.
pub(crate) fn surface_action(status: Option<&wgpu::CurrentSurfaceTexture>) -> SurfaceAction {
    use wgpu::CurrentSurfaceTexture as S;
    let Some(status) = status else {
        return SurfaceAction::Recreate;
    };
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

/// Whether the caller should request another frame from its window.
#[must_use = "the caller must request a redraw when one is needed"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redraw {
    /// Request a redraw: the view changed, or a failed frame is retried.
    Needed,
    /// Draw nothing until the next event.
    NotNeeded,
}

/// Manages the wgpu device, queue, and surface; issues frame draws.
///
/// Render passes (architecture.md, "GPU rendering"):
///   Pass 1: ShapeRenderer — clear background + cursor box
///   Pass 2: TextRenderer  — glyph quads from the atlas
pub struct RenderState {
    /// The window the surface draws to, kept to create the surface again
    /// after it is lost.
    target: Arc<dyn wgpu::DisplayAndWindowHandle>,
    instance: wgpu::Instance,
    /// `None` only after creating a lost surface again failed.
    surface: Option<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    shape_renderer: ShapeRenderer,
    text_renderer: TextRenderer,
    /// Shapes and rasterizes text; see `oso_platform::text`.
    text_system: Box<dyn TextSystem>,
    /// Shaped rows of the buffer shown, by display row.
    rows: Rows,
    /// Boxes behind invisible characters this frame, and the rectangles
    /// drawn in pass 1; reused across frames.
    boxes: Vec<(usize, usize, usize)>,
    rects: Vec<Rect>,
    /// Frames in a row that failed to draw; see `count_failed_frame`.
    failed_frames: u8,
    /// Frames presented so far.
    presented: u64,
}

impl RenderState {
    /// Sets up the GPU for `target`, whose drawable area is `width` x
    /// `height` physical pixels. Text is drawn with `text_system`, whose
    /// fonts are loaded for the window's scale factor.
    pub async fn new(
        target: Arc<dyn wgpu::DisplayAndWindowHandle>,
        width: u32,
        height: u32,
        text_system: Box<dyn TextSystem>,
    ) -> Self {
        // 1. Create a wgpu instance for this OS's backend.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: platform_backend(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: Default::default(),
            display: None,
        });

        // 2. Create a surface tied to the window.
        let surface = instance.create_surface(target.clone()).unwrap();

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
            width: width.max(1),
            height: height.max(1),
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

        // 6. Initialize the shape and text renderers.
        let shape_renderer =
            ShapeRenderer::new(&device, surface_format, width as f32, height as f32);
        let text_renderer = TextRenderer::new(&device, surface_format, width as f32, height as f32);

        Self {
            target,
            instance,
            surface: Some(surface),
            device,
            queue,
            config,
            shape_renderer,
            text_renderer,
            text_system,
            rows: Rows::default(),
            boxes: Vec::new(),
            rects: Vec::new(),
            failed_frames: 0,
            presented: 0,
        }
    }

    /// The cell grid text is drawn on, in physical pixels.
    pub fn cell_metrics(&self) -> CellMetrics {
        self.text_system.metrics()
    }

    /// Replaces the text system, for example after the scale factor
    /// changed. Cached shaping and glyphs are dropped and redone on the
    /// next frame.
    pub fn set_text_system(&mut self, text_system: Box<dyn TextSystem>) -> Redraw {
        self.text_system = text_system;
        self.text_renderer.atlas.clear();
        self.rows.invalidate();
        Redraw::Needed
    }

    /// The rows and columns of text the surface shows in full, each at
    /// least 1: the window size that scrolling works with.
    pub fn text_area(&self) -> (usize, usize) {
        let metrics = self.text_system.metrics();
        let rows = (self.config.height as f32 - MARGIN_Y) / metrics.height;
        let columns = (self.config.width as f32 - MARGIN_X) / metrics.width;
        (rows.max(1.0) as usize, columns.max(1.0) as usize)
    }

    /// How many frames have been presented. A frame skipped because the
    /// window is hidden does not count.
    pub fn presented_frames(&self) -> u64 {
        self.presented
    }

    /// The size of the surface in physical pixels, as last configured.
    pub fn surface_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Reconfigures the surface and updates the orthographic matrix on window
    /// resize. The new size is in physical pixels.
    pub fn resize(&mut self, width: u32, height: u32) -> Redraw {
        if self.configure(width, height) {
            Redraw::Needed
        } else {
            Redraw::NotNeeded
        }
    }

    /// Configures the surface and the orthographic matrix for the given size.
    /// Returns false, and does nothing, for a zero-sized (minimized) window.
    fn configure(&mut self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        self.config.width = width;
        self.config.height = height;
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        self.shape_renderer
            .resize(&self.queue, width as f32, height as f32);
        self.text_renderer
            .resize(&self.queue, width as f32, height as f32);
        true
    }

    /// Recovers from a surface that gave no texture, and asks for a redraw
    /// to retry the frame unless retries are exhausted. `window_size` is as
    /// in [`Self::render`].
    fn recover(
        &mut self,
        action: SurfaceAction,
        window_size: &dyn Fn() -> (u32, u32),
    ) -> Result<Redraw, RenderError> {
        match action {
            SurfaceAction::Reconfigure => {
                let (width, height) = window_size();
                self.configure(width, height);
            }
            SurfaceAction::Recreate => {
                // Drop the old surface first. DXGI allows one swapchain per
                // window and Vulkan fails with NATIVE_WINDOW_IN_USE, and the
                // failed configure would panic in wgpu's default error handler.
                self.surface = None;
                let surface = self
                    .instance
                    .create_surface(self.target.clone())
                    .map_err(RenderError::RecreateSurface)?;
                surface.configure(&self.device, &self.config);
                self.surface = Some(surface);
            }
            SurfaceAction::Retry => {}
            SurfaceAction::Fail => return Err(RenderError::SurfaceValidation),
            SurfaceAction::Skip | SurfaceAction::Draw | SurfaceAction::DrawThenReconfigure => {
                return Ok(Redraw::NotNeeded);
            }
        }
        let retry;
        (self.failed_frames, retry) = count_failed_frame(self.failed_frames);
        if retry {
            Ok(Redraw::Needed)
        } else if self.failed_frames == MAX_RETRIES + 1 {
            Err(RenderError::RetriesExhausted)
        } else {
            Ok(Redraw::NotNeeded)
        }
    }

    /// Renders a single frame.
    ///
    /// `window_size` returns the window's current size in physical pixels.
    /// It is called only when the surface must be configured again, since
    /// asking the window can be a blocking round trip (for example to the X
    /// server).
    ///
    /// Returns [`Redraw::Needed`] when glyphs were left for the next frame
    /// to rasterize.
    pub fn render(
        &mut self,
        view: &TextView,
        window_size: &dyn Fn() -> (u32, u32),
    ) -> Result<Redraw, RenderError> {
        // The CPU work (shaping, rasterizing, building quads) comes first,
        // so it overlaps the wait for a drawable instead of adding to it.
        let started = std::time::Instant::now();
        let mut budget = RasterBudget::new(GLYPHS_PER_FRAME);
        self.prepare_frame(view, &mut budget);
        log::trace!("frame prepared in {:?}", started.elapsed());

        // Acquire the next surface texture, or recover as wgpu documents.
        let current = self
            .surface
            .as_ref()
            .map(wgpu::Surface::get_current_texture);
        let action = surface_action(current.as_ref());
        let output = match current {
            Some(
                wgpu::CurrentSurfaceTexture::Success(tex)
                | wgpu::CurrentSurfaceTexture::Suboptimal(tex),
            ) => tex,
            other => {
                log::debug!("no surface texture: {other:?}");
                return self.recover(action, window_size);
            }
        };
        let target = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        self.encode_passes(&mut encoder, &target);

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        log::trace!("frame presented {:?} after it started", started.elapsed());

        self.failed_frames = 0;
        self.presented += 1;

        // Configure only after presenting: no surface texture may be alive.
        // Only a size change is fixed here; reconfiguring with the same size
        // would cost a GPU wait on every frame without clearing the status.
        if action == SurfaceAction::DrawThenReconfigure {
            let (width, height) = window_size();
            if width != self.config.width || height != self.config.height {
                self.configure(width, height);
            }
        }

        Ok(if budget.exhausted {
            Redraw::Needed
        } else {
            Redraw::NotNeeded
        })
    }

    /// Shapes, rasterizes and uploads what one frame of `text` draws,
    /// with at most `budget` new glyphs. Touches the queue, not the surface.
    fn prepare_frame(&mut self, text: &TextView, budget: &mut RasterBudget) {
        let metrics = self.text_system.metrics();
        let display = &text.display;
        // The box cursor covers the whole character under it, two cells for
        // a full-width one (ADR 0026).
        let cursor_cells = motion::cursor_cells(display, text.cursor);
        let layout = Layout {
            origin: [MARGIN_X, MARGIN_Y],
            viewport: [self.config.width as f32, self.config.height as f32],
            metrics,
            scroll: text.scroll,
            cursor: (text.cursor, cursor_cells),
            color: FOREGROUND,
            escape_color: ESCAPE,
            cursor_color: BACKGROUND,
        };

        let rasterized = self.text_renderer.atlas.rasterized();
        let shaped = self.rows.shaped_count();
        let started = std::time::Instant::now();
        self.rows.begin(text.buffer, text.revision);
        self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut *self.text_system,
            &mut self.rows,
            display,
            &layout,
            budget,
            &mut self.boxes,
        );
        let new_glyphs = self.text_renderer.atlas.rasterized() - rasterized;
        let new_rows = self.rows.shaped_count() - shaped;
        if new_glyphs > 0 || new_rows > 0 {
            log::debug!(
                "shaped {new_rows} rows and rasterized {new_glyphs} glyphs in {:?}",
                started.elapsed()
            );
        }

        self.rects.clear();
        for &(row, column, cells) in &self.boxes {
            self.rects.push(Rect {
                position: layout.cell(row, column),
                size: [metrics.width * cells as f32, metrics.height],
                color: ESCAPE_BOX,
            });
        }
        self.rects.push(Rect {
            position: layout.cell(text.cursor.row, text.cursor.column),
            size: [metrics.width * cursor_cells as f32, metrics.height],
            color: CURSOR,
        });
        self.shape_renderer
            .prepare(&self.device, &self.queue, &self.rects);
    }

    /// Records the passes of the prepared frame into `view`.
    fn encode_passes(&mut self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        // --- Pass 1: clear background, boxes and the cursor (ShapeRenderer) ---
        let [r, g, b, a] = BACKGROUND.map(f64::from);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Pass 1: Shape"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Clear with the editor background color.
                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        self.shape_renderer.render(&mut pass);
        drop(pass);

        // --- Pass 2: glyphs from the atlas (TextRenderer) ---
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Pass 2: Text"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        self.text_renderer.render(&mut pass);
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_RETRIES, SurfaceAction, count_failed_frame, platform_backend, surface_action};
    use wgpu::CurrentSurfaceTexture as S;

    // --- Surface status mapping ---
    // `Success` and `Suboptimal` carry a texture and need a GPU to construct.

    #[test]
    fn outdated_reconfigures() {
        assert_eq!(
            surface_action(Some(&S::Outdated)),
            SurfaceAction::Reconfigure
        );
    }

    #[test]
    fn missing_surface_is_created_again() {
        assert_eq!(surface_action(None), SurfaceAction::Recreate);
    }

    #[test]
    fn lost_recreates_the_surface() {
        assert_eq!(surface_action(Some(&S::Lost)), SurfaceAction::Recreate);
    }

    #[test]
    fn timeout_retries_the_frame() {
        assert_eq!(surface_action(Some(&S::Timeout)), SurfaceAction::Retry);
    }

    #[test]
    fn occluded_skips_the_frame() {
        assert_eq!(surface_action(Some(&S::Occluded)), SurfaceAction::Skip);
    }

    #[test]
    fn validation_fails_the_frame() {
        assert_eq!(surface_action(Some(&S::Validation)), SurfaceAction::Fail);
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
    fn only_the_platform_backend_is_compiled_in() {
        assert_eq!(
            wgpu::Instance::enabled_backend_features(),
            platform_backend()
        );
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
}
