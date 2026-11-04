//! Frame capture for the dev control socket's `screenshot` (ADR 0016), and
//! the GPU in use, for benchmarks (ADR 0044). Dev builds only.
//!
//! Must not depend on: winit, PNG encoding or the control socket; it hands
//! back raw pixels, and the tip crate does the rest.
//!
//! The UI thread only records and submits the copy. Waiting for the GPU and
//! reading the pixels back happen in [`Capture::wait`], on another thread.

use std::fmt;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::atlas::RasterBudget;
use crate::{RenderState, TextView};

/// How long [`Capture::wait`] waits for the GPU.
const GPU_TIMEOUT: Duration = Duration::from_secs(5);

/// How often [`Capture::wait`] polls the device.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Bytes per pixel of the formats a capture supports.
const BYTES_PER_PIXEL: u32 = 4;

/// An image in 8-bit RGBA, rows top to bottom, with no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// A frame copy submitted to the GPU and not read back yet. `Send`, so it
/// can be waited on away from the UI thread.
pub struct Capture {
    device: wgpu::Device,
    buffer: wgpu::Buffer,
    mapped: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    layout: Layout,
}

/// Why a capture failed.
#[derive(Debug)]
pub enum CaptureError {
    /// The surface uses a pixel format that capture cannot convert, named
    /// here.
    UnsupportedFormat(String),
    /// The GPU did not finish in time, or reported an error.
    Gpu(String),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat(format) => {
                write!(f, "cannot capture the surface format {format}")
            }
            Self::Gpu(e) => write!(f, "capture failed on the GPU: {e}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// How the copied pixels are laid out in the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    width: u32,
    height: u32,
    /// Bytes per row in the buffer, padded to wgpu's copy alignment.
    padded_row: u32,
    /// Whether the pixels are BGRA rather than RGBA.
    bgra: bool,
}

impl Layout {
    /// The layout for a `width` x `height` copy, or `None` for a format
    /// capture cannot convert.
    fn new(format: wgpu::TextureFormat, width: u32, height: u32) -> Option<Self> {
        use wgpu::TextureFormat as F;
        let bgra = match format {
            F::Rgba8Unorm | F::Rgba8UnormSrgb => false,
            F::Bgra8Unorm | F::Bgra8UnormSrgb => true,
            _ => return None,
        };
        let row = width * BYTES_PER_PIXEL;
        let padded_row =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        Some(Self {
            width,
            height,
            padded_row,
            bgra,
        })
    }

    fn buffer_size(self) -> u64 {
        u64::from(self.padded_row) * u64::from(self.height)
    }

    /// Strips the row padding from `data` and converts it to RGBA.
    fn to_rgba(self, data: &[u8]) -> Vec<u8> {
        let row = (self.width * BYTES_PER_PIXEL) as usize;
        let mut rgba = Vec::with_capacity(row * self.height as usize);
        for padded in data.chunks_exact(self.padded_row as usize) {
            rgba.extend_from_slice(&padded[..row]);
        }
        if self.bgra {
            for pixel in rgba.chunks_exact_mut(BYTES_PER_PIXEL as usize) {
                pixel.swap(0, 2);
            }
        }
        rgba
    }
}

impl RenderState {
    /// Draws the current frame again into an offscreen texture of the
    /// surface's size and format, and submits a copy of it to a buffer.
    /// This does not wait for the GPU; call [`Capture::wait`] on another
    /// thread for the pixels.
    /// Every glyph of the frame is rasterized, so the image is complete.
    pub fn capture(&mut self, text: &TextView) -> Result<Capture, CaptureError> {
        let (width, height) = (self.config.width, self.config.height);
        let format = self.config.format;
        let layout = Layout::new(format, width, height)
            .ok_or_else(|| CaptureError::UnsupportedFormat(format!("{format:?}")))?;

        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture readback"),
            size: layout.buffer_size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("capture"),
            });
        self.prepare_frame(text, &mut RasterBudget::unlimited());
        self.encode_passes(&mut encoder, &view);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(layout.padded_row),
                    rows_per_image: Some(height),
                },
            },
            size,
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        let (sender, mapped) = mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = sender.send(result);
        });
        Ok(Capture {
            device: self.device.clone(),
            buffer,
            mapped,
            layout,
        })
    }
}

impl Capture {
    /// Waits for the GPU and reads the frame back. Blocks: call it away from
    /// the UI thread.
    pub fn wait(self) -> Result<Image, CaptureError> {
        // `PollType::Wait` would hold the device's locks for the whole GPU
        // wait, stalling the UI thread's submit and present. Short
        // non-blocking polls hold them only for microseconds.
        let deadline = Instant::now() + GPU_TIMEOUT;
        let mapped = loop {
            self.device
                .poll(wgpu::PollType::Poll)
                .map_err(|e| CaptureError::Gpu(e.to_string()))?;
            match self.mapped.recv_timeout(POLL_INTERVAL) {
                Ok(result) => break result,
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                Err(e) => return Err(CaptureError::Gpu(e.to_string())),
            }
        };
        mapped.map_err(|e| CaptureError::Gpu(e.to_string()))?;
        let rgba = self.layout.to_rgba(&self.buffer.get_mapped_range(..));
        self.buffer.unmap();
        Ok(Image {
            width: self.layout.width,
            height: self.layout.height,
            rgba,
        })
    }
}

/// The GPU in use, for benchmarks to tell hardware from software rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuInfo {
    /// The adapter's name.
    pub name: String,
    /// The kind of adapter: `integrated`, `discrete`, `virtual`, `cpu` or
    /// `other`.
    pub kind: &'static str,
}

impl RenderState {
    /// The GPU in use.
    pub fn gpu_info(&self) -> GpuInfo {
        GpuInfo {
            name: self.adapter.name.clone(),
            kind: match self.adapter.device_type {
                wgpu::DeviceType::IntegratedGpu => "integrated",
                wgpu::DeviceType::DiscreteGpu => "discrete",
                wgpu::DeviceType::VirtualGpu => "virtual",
                wgpu::DeviceType::Cpu => "cpu",
                wgpu::DeviceType::Other => "other",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Layout;
    use wgpu::TextureFormat as F;

    #[test]
    fn rows_are_padded_to_the_copy_alignment() {
        let layout = Layout::new(F::Rgba8UnormSrgb, 3, 2).unwrap();
        assert_eq!(layout.padded_row, 256);
        assert_eq!(layout.buffer_size(), 512);
        assert_eq!(Layout::new(F::Rgba8Unorm, 64, 1).unwrap().padded_row, 256);
        assert_eq!(Layout::new(F::Rgba8Unorm, 65, 1).unwrap().padded_row, 512);
    }

    #[test]
    fn other_formats_are_refused() {
        assert_eq!(Layout::new(F::Rgba16Float, 1, 1), None);
        assert_eq!(Layout::new(F::Rgb10a2Unorm, 1, 1), None);
    }

    /// A 2x2 buffer with a pixel `[i, 10 + i, 20 + i, 255]` per position,
    /// padded as wgpu lays it out.
    fn padded_2x2(layout: Layout) -> Vec<u8> {
        let mut data = vec![0xEE; layout.buffer_size() as usize];
        for y in 0..2u8 {
            for x in 0..2u8 {
                let i = y * 2 + x;
                let at = usize::from(y) * layout.padded_row as usize + usize::from(x) * 4;
                data[at..at + 4].copy_from_slice(&[i, 10 + i, 20 + i, 255]);
            }
        }
        data
    }

    #[test]
    fn strips_padding_from_rgba() {
        let layout = Layout::new(F::Rgba8UnormSrgb, 2, 2).unwrap();
        let rgba = layout.to_rgba(&padded_2x2(layout));
        assert_eq!(
            rgba,
            [
                0, 10, 20, 255, 1, 11, 21, 255, //
                2, 12, 22, 255, 3, 13, 23, 255,
            ]
        );
    }

    #[test]
    fn swaps_bgra_to_rgba() {
        let layout = Layout::new(F::Bgra8UnormSrgb, 2, 2).unwrap();
        let rgba = layout.to_rgba(&padded_2x2(layout));
        assert_eq!(
            rgba,
            [
                20, 10, 0, 255, 21, 11, 1, 255, //
                22, 12, 2, 255, 23, 13, 3, 255,
            ]
        );
    }
}
