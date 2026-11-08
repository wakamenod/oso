//! Painting a frame: its [`Scene`] uploaded and recorded into one render pass,
//! with one draw per layer per pipeline (ADR 0061). It knows no surface, so a
//! screenshot takes the same path as the window.
//!
//! Must not depend on: `capture`, winit or a font backend.

use oso_platform::text::{GlyphKey, TextSystem};

use crate::atlas::{AtlasGlyph, GlyphAtlas, RasterBudget};
use crate::pane::{Face, Pane};
use crate::scene::{GlyphSource, Limits, Scene};
use crate::shape::{MAX_RECTS, ShapeRenderer};
use crate::text::TextRenderer;

/// The real atlas, with what it needs to add a glyph this frame.
struct Atlas<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    atlas: &'a mut GlyphAtlas,
    budget: &'a mut RasterBudget,
}

impl GlyphSource for Atlas<'_> {
    fn get(&mut self, system: &mut dyn TextSystem, key: GlyphKey) -> Option<AtlasGlyph> {
        self.atlas
            .get(self.device, self.queue, system, key, self.budget)
    }

    fn take_full(&mut self) -> bool {
        self.atlas.take_full()
    }

    fn clear(&mut self) {
        self.atlas.clear();
    }
}

/// The pipelines and what the frame draws with them.
pub(crate) struct Painter {
    shape: ShapeRenderer,
    text: TextRenderer,
    scene: Scene,
}

impl Painter {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: f32,
        height: f32,
    ) -> Self {
        let shape = ShapeRenderer::new(device, format, width, height);
        let text = TextRenderer::new(device, format, width, height);
        let scene = Scene::new(Limits {
            rects: MAX_RECTS,
            quads: text.max_instances(),
        });
        Self { shape, text, scene }
    }

    /// Updates the orthographic projection for a new surface size.
    pub(crate) fn resize(&self, queue: &wgpu::Queue, width: f32, height: f32) {
        self.shape.resize(queue, width, height);
        self.text.resize(queue, width, height);
    }

    /// Drops cached shaping and glyphs, for when the font changes.
    pub(crate) fn invalidate(&mut self) {
        self.text.atlas.clear();
        self.scene.invalidate();
    }

    /// Shapes, rasterizes and uploads what one frame of `panes` draws, with at
    /// most `budget` new glyphs.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        system: &mut dyn TextSystem,
        panes: &[Pane<'_>],
        budget: &mut RasterBudget,
    ) {
        let rasterized = self.text.atlas.rasterized();
        let shaped = self.scene.shaped_count();
        let started = std::time::Instant::now();
        let mut atlas = Atlas {
            device,
            queue,
            atlas: &mut self.text.atlas,
            budget,
        };
        self.scene.build(panes, system, &mut atlas);
        let new_glyphs = self.text.atlas.rasterized() - rasterized;
        // A pane gone from the list took its count along.
        let new_rows = self.scene.shaped_count().saturating_sub(shaped);
        if new_glyphs > 0 || new_rows > 0 {
            log::debug!(
                "shaped {new_rows} rows and rasterized {new_glyphs} glyphs in {:?}",
                started.elapsed()
            );
        }
        self.text.prepare(device, queue, self.scene.quads());
        self.shape.prepare(device, queue, self.scene.rects());
    }

    /// Records the prepared frame into `view`.
    pub(crate) fn encode(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        let [r, g, b, a] = Face::Text.background().map(f64::from);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Panes"),
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
        for layer in self.scene.layers() {
            self.shape.render(&mut pass, layer.rects.clone());
            self.text.render(&mut pass, layer.quads.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    //! The same frames on a real GPU, in real pixels. Run with `just pixel-tests`:
    //! the rest of the crate's tests need no GPU. The glyphs come from a fake text
    //! system, so these assert the same on an OS with no text backend.

    use super::*;
    use crate::pane::{Layer, PaneContent, PaneId, Span};
    use oso_core::frame::CellRect;
    use oso_platform::text::{CellMetrics, FontId, GlyphBitmap, GlyphFormat, GlyphId, ShapedGlyph};

    const WIDTH: u32 = 128;
    const HEIGHT: u32 = 128;

    /// One glyph per character. Its bitmap fills its cell, 6 px above it and
    /// 8 px below it, past the bands of ADR 0052 on both sides.
    struct Blocks;

    impl TextSystem for Blocks {
        fn metrics(&self) -> CellMetrics {
            CellMetrics {
                width: 10.0,
                height: 20.0,
                ascent: 16.0,
                descent: 4.0,
            }
        }

        fn shape(&mut self, text: &str, out: &mut Vec<ShapedGlyph>) {
            out.clear();
            for (i, (byte, _)) in text.char_indices().enumerate() {
                out.push(ShapedGlyph {
                    key: GlyphKey {
                        font: FontId(0),
                        glyph: GlyphId(1),
                    },
                    cluster: byte as u32,
                    x: i as f32 * 10.0,
                    y: 0.0,
                    advance: 10.0,
                });
            }
        }

        fn rasterize(&mut self, _key: GlyphKey) -> Option<GlyphBitmap> {
            Some(GlyphBitmap {
                width: 10,
                height: 34,
                left: 0,
                top: 22,
                format: GlyphFormat::Coverage,
                data: vec![255; 340],
            })
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            std::thread::yield_now();
        }
    }

    /// Draws `panes` on a real device, and gives back RGBA pixels.
    fn draw(panes: &[Pane<'_>]) -> Vec<u8> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: Default::default(),
            display: None,
        });
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("this test needs a GPU");
        let (device, queue) =
            block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut painter = Painter::new(&device, format, WIDTH as f32, HEIGHT as f32);
        let mut budget = RasterBudget::new(64);
        painter.prepare(&device, &queue, &mut Blocks, panes, &mut budget);

        let size = wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let bytes_per_row = 4 * WIDTH.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT / 4);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(bytes_per_row * HEIGHT),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        let view = texture.create_view(&Default::default());
        painter.encode(&mut encoder, &view);
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(HEIGHT),
                },
            },
            size,
        );
        queue.submit(std::iter::once(encoder.finish()));
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let mapped = buffer.get_mapped_range(..);
        mapped
            .chunks_exact(bytes_per_row as usize)
            .flat_map(|row| row[..WIDTH as usize * 4].iter().copied())
            .collect()
    }

    fn red(pixels: &[u8], x: u32, y: u32) -> u8 {
        pixels[((y * WIDTH + x) * 4) as usize]
    }

    /// The red of a color drawn to an 8-bit surface.
    fn eight_bit(color: [f32; 4]) -> i32 {
        (color[0] * 255.0).round() as i32
    }

    fn assert_is(pixels: &[u8], (x, y): (u32, u32), color: [f32; 4], what: &str) {
        let (got, want) = (i32::from(red(pixels, x, y)), eight_bit(color));
        assert!(
            (got - want).abs() <= 1,
            "{what} at ({x}, {y}): {got}, not {want}"
        );
    }

    fn pane<'a>(
        id: u32,
        (row, column, rows, columns): (usize, usize, usize, usize),
        layer: u8,
        background: Option<Face>,
        lines: &'a [&'a [Span<'a>]],
    ) -> Pane<'a> {
        Pane {
            id: PaneId(id),
            region: CellRect {
                row,
                column,
                rows,
                columns,
            },
            layer: Layer(layer),
            background,
            border: None,
            content: PaneContent::Lines(lines),
        }
    }

    const INK: fn() -> [f32; 4] = || Face::Text.foreground();
    const PAPER: fn() -> [f32; 4] = || Face::Text.background();

    #[test]
    #[ignore = "needs a GPU"]
    fn an_opaque_pane_in_front_hides_the_text_under_it() {
        let text = [Span {
            text: "xxxxxxxxxx",
            face: Face::Text,
        }];
        let lines: [&[Span<'_>]; 4] = [&text, &text, &text, &text];
        // The grid starts at (8, 8) with cells of 10 x 20 px. A pane over rows
        // 1 and 2, columns 2 to 5, listed before the text it covers.
        let panes = [
            pane(1, (1, 2, 2, 4), 1, Some(Face::Text), &[]),
            pane(0, (0, 0, 4, 10), 0, None, &lines),
        ];
        let pixels = draw(&panes);
        assert_is(&pixels, (12, 18), INK(), "text beside the pane");
        assert_is(&pixels, (72, 38), INK(), "text beside the pane");
        assert_is(&pixels, (12, 38), INK(), "text on the row of the pane");
        assert_is(&pixels, (32, 38), PAPER(), "text under the pane");
        assert_is(&pixels, (67, 67), PAPER(), "the pane's last pixel");
        assert_is(&pixels, (68, 67), INK(), "the first pixel past it");
    }

    #[test]
    #[ignore = "needs a GPU"]
    fn a_panes_text_goes_over_its_own_background() {
        let word = [Span {
            text: "xx",
            face: Face::Text,
        }];
        let lines: [&[Span<'_>]; 1] = [&word];
        let panes = [pane(0, (1, 1, 1, 4), 0, Some(Face::Boxed), &lines)];
        let pixels = draw(&panes);
        assert_is(&pixels, (22, 38), INK(), "a glyph on the background");
        let fill = Face::Boxed.background();
        assert_is(&pixels, (52, 38), fill, "the background past the glyphs");
        assert_is(&pixels, (58, 67), PAPER(), "below the pane");
    }

    #[test]
    #[ignore = "needs a GPU"]
    fn both_layers_draw_their_own_ranges() {
        let word = [Span {
            text: "xx",
            face: Face::Text,
        }];
        let lines: [&[Span<'_>]; 1] = [&word];
        // Built front to back, the back layer's rectangles and quads start past
        // the front's in the buffers. The front pane is on rows 1 and 2.
        let panes = [
            pane(0, (0, 0, 4, 10), 0, Some(Face::Boxed), &lines),
            pane(1, (1, 1, 2, 4), 1, Some(Face::Text), &lines),
        ];
        let pixels = draw(&panes);
        let fill = Face::Boxed.background();
        assert_is(&pixels, (12, 18), INK(), "the back pane's text");
        assert_is(&pixels, (52, 18), fill, "the back pane's background");
        assert_is(&pixels, (22, 38), INK(), "the front pane's text");
        assert_is(&pixels, (52, 38), PAPER(), "the front pane's background");
        assert_is(
            &pixels,
            (72, 38),
            fill,
            "the back pane beside the front one",
        );
    }

    #[test]
    #[ignore = "needs a GPU"]
    fn the_frames_top_row_keeps_its_band_and_no_more() {
        let word = [Span {
            text: "x",
            face: Face::Text,
        }];
        let lines: [&[Span<'_>]; 1] = [&word];
        let panes = [pane(0, (0, 0, 1, 4), 0, None, &lines)];
        let pixels = draw(&panes);
        // Row 0 starts at y 8, the band at y 4 and the bitmap at y 2.
        assert_is(&pixels, (12, 4), INK(), "the band in the top margin");
        assert_is(&pixels, (12, 3), PAPER(), "the bitmap above the band");
        assert_is(&pixels, (7, 12), PAPER(), "the left margin");
    }

    #[test]
    #[ignore = "needs a GPU"]
    fn a_glyph_stays_in_its_pane() {
        let word = [Span {
            text: "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            face: Face::Text,
        }];
        let lines: [&[Span<'_>]; 1] = [&word];
        // Row 1 of the frame, 4 columns: x 8..48 and y 28..48.
        let panes = [pane(0, (1, 0, 1, 4), 0, None, &lines)];
        let pixels = draw(&panes);
        assert_is(&pixels, (8, 28), INK(), "the first pixel in the pane");
        assert_is(&pixels, (47, 47), INK(), "the last pixel in the pane");
        assert_is(&pixels, (7, 38), PAPER(), "the left margin");
        assert_is(&pixels, (48, 38), PAPER(), "past the right edge");
        assert_is(&pixels, (20, 27), INK(), "the band above a tiled row");
        assert_is(&pixels, (20, 23), PAPER(), "above the band");
        assert_is(&pixels, (20, 48), PAPER(), "below the pane");

        let panes = [pane(0, (1, 0, 2, 4), 0, None, &lines)];
        let pixels = draw(&panes);
        assert_is(&pixels, (20, 28), INK(), "the first row of a taller pane");
        assert_is(&pixels, (20, 27), PAPER(), "above a taller pane");

        let panes = [pane(0, (1, 0, 1, 4), 1, None, &lines)];
        let pixels = draw(&panes);
        assert_is(&pixels, (20, 27), PAPER(), "above a row in a higher layer");
    }
}
