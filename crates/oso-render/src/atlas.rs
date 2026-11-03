//! The glyph atlas: rasterized glyphs cached in GPU textures (ADR 0005,
//! ADR 0036).
//!
//! - One texture per [`GlyphFormat`]: one byte of coverage per pixel for
//!   text, premultiplied RGBA for color emoji.
//! - Glyphs are packed into shelves (rows) and never move. A full texture
//!   grows to twice its size, keeping its contents, so a glyph is
//!   rasterized once for as long as the font stays loaded.

use std::collections::HashMap;

use oso_platform::text::{GlyphBitmap, GlyphFormat, GlyphKey, TextSystem};

/// The side of a new atlas texture, in pixels.
const INITIAL_SIDE: u32 = 512;

/// The largest atlas side, also bounded by the device limit.
const MAX_SIDE: u32 = 8192;

/// One row of glyphs in a texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shelf {
    y: u32,
    height: u32,
    /// Where the next glyph on this shelf goes.
    x: u32,
}

/// Packs rectangles into shelves: rows as tall as their tallest glyph,
/// filled left to right. Glyphs of a font have similar heights, so little
/// space is wasted, and allocation is a short scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShelfPacker {
    width: u32,
    height: u32,
    shelves: Vec<Shelf>,
    /// The top of the space below the last shelf.
    next_y: u32,
}

impl ShelfPacker {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            shelves: Vec::new(),
            next_y: 0,
        }
    }

    /// The top-left corner of a free `width` x `height` area, or `None` if
    /// there is none.
    pub(crate) fn allocate(&mut self, width: u32, height: u32) -> Option<[u32; 2]> {
        if width == 0 || height == 0 || width > self.width {
            return None;
        }
        // A shelf at least as tall, but not much taller, with room left.
        // Sizes are bounded by the atlas side, so none of this overflows.
        let tallest = height.saturating_add(height / 4 + 1);
        let fits =
            |s: &Shelf| s.height >= height && s.height <= tallest && width <= self.width - s.x;
        if let Some(shelf) = self.shelves.iter_mut().find(|s| fits(s)) {
            let corner = [shelf.x, shelf.y];
            shelf.x += width;
            return Some(corner);
        }
        if height > self.height - self.next_y {
            return None;
        }
        let corner = [0, self.next_y];
        self.shelves.push(Shelf {
            y: self.next_y,
            height,
            x: width,
        });
        self.next_y += height;
        Some(corner)
    }

    /// Makes the area larger. Areas already allocated keep their place.
    pub(crate) fn grow(&mut self, width: u32, height: u32) {
        self.width = self.width.max(width);
        self.height = self.height.max(height);
    }

    /// Frees every area.
    pub(crate) fn clear(&mut self) {
        self.shelves.clear();
        self.next_y = 0;
    }
}

/// Where a cached glyph is, and how to place it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AtlasGlyph {
    pub(crate) format: GlyphFormat,
    /// Top-left corner in the texture, in pixels.
    pub(crate) uv: [u32; 2],
    pub(crate) size: [u32; 2],
    /// As in [`GlyphBitmap`]: from the pen to the left edge, and from the
    /// baseline up to the top edge.
    pub(crate) left: i32,
    pub(crate) top: i32,
}

/// One texture of the atlas and its packer.
struct Page {
    format: GlyphFormat,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    packer: ShelfPacker,
    side: u32,
}

impl Page {
    fn new(device: &wgpu::Device, format: GlyphFormat, side: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(match format {
                GlyphFormat::Coverage => "Glyph Atlas (coverage)",
                GlyphFormat::Color => "Glyph Atlas (color)",
            }),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: texture_format(format),
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            format,
            texture,
            view,
            packer: ShelfPacker::new(side, side),
            side,
        }
    }

    /// Doubles the texture's size and copies the old contents over.
    /// Returns false at the size limit.
    fn grow(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, max_side: u32) -> bool {
        let side = self.side.saturating_mul(2).min(max_side);
        if side <= self.side {
            return false;
        }
        let new = Self::new(device, self.format, side);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Glyph Atlas Grow"),
        });
        // Glyph uploads queued before this submit land in the old texture
        // first, so they are copied too.
        encoder.copy_texture_to_texture(
            self.texture.as_image_copy(),
            new.texture.as_image_copy(),
            wgpu::Extent3d {
                width: self.side,
                height: self.side,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(encoder.finish()));
        log::debug!(
            "glyph atlas ({:?}) grew from {} to {side} px",
            self.format,
            self.side
        );
        self.texture = new.texture;
        self.view = new.view;
        self.side = side;
        self.packer.grow(side, side);
        true
    }

    fn upload(&self, queue: &wgpu::Queue, uv: [u32; 2], bitmap: &GlyphBitmap) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: uv[0],
                    y: uv[1],
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &bitmap.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bitmap.width * bitmap.format.bytes_per_pixel() as u32),
                rows_per_image: Some(bitmap.height),
            },
            wgpu::Extent3d {
                width: bitmap.width,
                height: bitmap.height,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// Whether `bitmap` has the data its size says and fits in an atlas page.
fn valid(bitmap: &GlyphBitmap, max_side: u32) -> bool {
    let len = (bitmap.width as usize)
        .checked_mul(bitmap.height as usize)
        .and_then(|n| n.checked_mul(bitmap.format.bytes_per_pixel()));
    bitmap.width <= max_side && bitmap.height <= max_side && len == Some(bitmap.data.len())
}

/// The texture format a glyph format is stored in.
fn texture_format(format: GlyphFormat) -> wgpu::TextureFormat {
    match format {
        GlyphFormat::Coverage => wgpu::TextureFormat::R8Unorm,
        // CoreGraphics draws emoji in sRGB.
        GlyphFormat::Color => wgpu::TextureFormat::Rgba8UnormSrgb,
    }
}

/// Rasterized glyphs in GPU textures, keyed by [`GlyphKey`].
pub(crate) struct GlyphAtlas {
    coverage: Page,
    color: Page,
    /// `None` for glyphs that draw nothing (spaces) or did not fit, so they
    /// are not rasterized again.
    glyphs: HashMap<GlyphKey, Option<AtlasGlyph>>,
    max_side: u32,
    /// Bumped whenever a texture is replaced, so bind groups are rebuilt.
    generation: u64,
    /// Glyphs rasterized since the atlas was created, for tests and logs.
    rasterized: u64,
    /// Whether a glyph did not fit since the last [`Self::take_full`].
    full: bool,
}

impl GlyphAtlas {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let max_side = device.limits().max_texture_dimension_2d.min(MAX_SIDE);
        let side = INITIAL_SIDE.min(max_side);
        Self {
            coverage: Page::new(device, GlyphFormat::Coverage, side),
            color: Page::new(device, GlyphFormat::Color, side),
            glyphs: HashMap::new(),
            max_side,
            generation: 0,
            rasterized: 0,
            full: false,
        }
    }

    pub(crate) fn coverage_view(&self) -> &wgpu::TextureView {
        &self.coverage.view
    }

    pub(crate) fn color_view(&self) -> &wgpu::TextureView {
        &self.color.view
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn rasterized(&self) -> u64 {
        self.rasterized
    }

    /// Whether a glyph did not fit, even at the largest size, since the last
    /// call. The caller clears the atlas and draws the frame again, so a
    /// full atlas never leaves glyphs invisible for good.
    pub(crate) fn take_full(&mut self) -> bool {
        std::mem::take(&mut self.full)
    }

    /// Forgets every glyph, for when the text system's fonts change. The
    /// textures are kept and refilled.
    pub(crate) fn clear(&mut self) {
        self.glyphs.clear();
        self.coverage.packer.clear();
        self.color.packer.clear();
    }

    /// The cached glyph for `key`, rasterizing and uploading it on first
    /// use. `None` if it draws nothing.
    pub(crate) fn get(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        system: &mut dyn TextSystem,
        key: GlyphKey,
    ) -> Option<AtlasGlyph> {
        if let Some(&cached) = self.glyphs.get(&key) {
            return cached;
        }
        let bitmap = system.rasterize(key);
        self.rasterized += 1;
        let entry = match bitmap {
            Some(bitmap) if valid(&bitmap, self.max_side) => {
                match self.insert(device, queue, &bitmap) {
                    Some(entry) => Some(entry),
                    // Not cached: it may fit once the atlas is cleared.
                    None => {
                        self.full = true;
                        return None;
                    }
                }
            }
            // Empty, or a bitmap the atlas cannot hold.
            _ => None,
        };
        self.glyphs.insert(key, entry);
        entry
    }

    fn insert(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bitmap: &GlyphBitmap,
    ) -> Option<AtlasGlyph> {
        let max_side = self.max_side;
        let page = match bitmap.format {
            GlyphFormat::Coverage => &mut self.coverage,
            GlyphFormat::Color => &mut self.color,
        };
        let mut grew = false;
        let uv = loop {
            if let Some(uv) = page.packer.allocate(bitmap.width, bitmap.height) {
                break Some(uv);
            }
            if !page.grow(device, queue, max_side) {
                log::warn!("glyph atlas ({:?}) is full; glyph not drawn", bitmap.format);
                break None;
            }
            grew = true;
        };
        if let Some(uv) = uv {
            page.upload(queue, uv, bitmap);
        }
        if grew {
            self.generation += 1;
        }
        let uv = uv?;
        Some(AtlasGlyph {
            format: bitmap.format,
            uv,
            size: [bitmap.width, bitmap.height],
            left: bitmap.left,
            top: bitmap.top,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ShelfPacker, valid};
    use oso_platform::text::{GlyphBitmap, GlyphFormat};
    use proptest::prelude::*;

    fn overlaps(a: ([u32; 2], [u32; 2]), b: ([u32; 2], [u32; 2])) -> bool {
        let ((ax, ay), (aw, ah)) = ((a.0[0], a.0[1]), (a.1[0], a.1[1]));
        let ((bx, by), (bw, bh)) = ((b.0[0], b.0[1]), (b.1[0], b.1[1]));
        ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
    }

    #[test]
    fn first_glyph_goes_top_left() {
        let mut p = ShelfPacker::new(64, 64);
        assert_eq!(p.allocate(10, 12), Some([0, 0]));
    }

    #[test]
    fn similar_glyphs_share_a_shelf() {
        let mut p = ShelfPacker::new(64, 64);
        assert_eq!(p.allocate(10, 12), Some([0, 0]));
        assert_eq!(p.allocate(10, 11), Some([10, 0]));
        assert_eq!(p.allocate(10, 12), Some([20, 0]));
    }

    #[test]
    fn much_shorter_glyph_starts_a_new_shelf() {
        let mut p = ShelfPacker::new(64, 64);
        p.allocate(10, 30);
        assert_eq!(p.allocate(10, 5), Some([0, 30]));
    }

    #[test]
    fn full_shelf_starts_a_new_one() {
        let mut p = ShelfPacker::new(32, 64);
        p.allocate(20, 10);
        assert_eq!(p.allocate(20, 10), Some([0, 10]));
    }

    #[test]
    fn full_atlas_refuses() {
        let mut p = ShelfPacker::new(16, 16);
        assert!(p.allocate(16, 16).is_some());
        assert_eq!(p.allocate(1, 1), None);
        assert_eq!(p.allocate(17, 1), None, "wider than the atlas");
        assert_eq!(p.allocate(0, 4), None, "empty");
    }

    #[test]
    fn growing_keeps_allocations_and_adds_room() {
        let mut p = ShelfPacker::new(16, 16);
        assert_eq!(p.allocate(16, 16), Some([0, 0]));
        assert_eq!(p.allocate(16, 16), None);
        p.grow(32, 32);
        let next = p.allocate(16, 16).unwrap();
        assert!(!overlaps(([0, 0], [16, 16]), (next, [16, 16])));
    }

    #[test]
    fn bitmaps_must_match_their_size() {
        let bitmap = |width, height, format, len| GlyphBitmap {
            width,
            height,
            left: 0,
            top: 0,
            format,
            data: vec![0; len],
        };
        assert!(valid(&bitmap(3, 2, GlyphFormat::Coverage, 6), 64));
        assert!(valid(&bitmap(3, 2, GlyphFormat::Color, 24), 64));
        assert!(!valid(&bitmap(3, 2, GlyphFormat::Color, 6), 64));
        assert!(!valid(&bitmap(3, 2, GlyphFormat::Coverage, 7), 64));
        assert!(!valid(&bitmap(65, 1, GlyphFormat::Coverage, 65), 64));
        assert!(!valid(
            &bitmap(u32::MAX, u32::MAX, GlyphFormat::Color, 0),
            64
        ));
    }

    #[test]
    fn huge_requests_are_refused_without_overflow() {
        let mut p = ShelfPacker::new(64, 64);
        p.allocate(10, 10);
        assert_eq!(p.allocate(10, u32::MAX), None);
        assert_eq!(p.allocate(u32::MAX, 10), None);
        assert!(p.allocate(10, 10).is_some());
    }

    #[test]
    fn clear_frees_everything() {
        let mut p = ShelfPacker::new(16, 16);
        p.allocate(16, 16);
        p.clear();
        assert_eq!(p.allocate(16, 16), Some([0, 0]));
    }

    proptest! {
        /// Whatever the sizes, allocations stay inside the atlas and never
        /// overlap, also across a grow.
        #[test]
        fn allocations_never_overlap_and_stay_inside(
            sizes in prop::collection::vec((1u32..40, 1u32..40), 1..300),
            grow_at in 0usize..300,
        ) {
            let mut p = ShelfPacker::new(128, 128);
            let mut side = 128;
            let mut placed: Vec<([u32; 2], [u32; 2])> = Vec::new();
            for (i, &(w, h)) in sizes.iter().enumerate() {
                if i == grow_at {
                    side = 256;
                    p.grow(side, side);
                }
                let Some(corner) = p.allocate(w, h) else {
                    continue;
                };
                prop_assert!(corner[0] + w <= side && corner[1] + h <= side);
                for &other in &placed {
                    prop_assert!(!overlaps((corner, [w, h]), other));
                }
                placed.push((corner, [w, h]));
            }
        }
    }
}
