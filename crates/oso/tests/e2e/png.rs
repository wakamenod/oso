//! Reads the PNG files `screenshot` writes, and tells what they show apart from
//! the background (ADR 0049).
//!
//! Must not depend on: the app or the socket; it turns a file into pixels. It
//! reads what a dev build's encoder writes and refuses anything else.

use std::fs;
use std::path::Path;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// Bit depth 8, color type 6 (RGBA), default compression and filter methods,
/// no interlace.
const IHDR_TAIL: [u8; 5] = [8, 6, 0, 0, 0];

/// An image in 8-bit RGBA, rows top to bottom.
pub struct Png {
    pub width: u32,
    pub height: u32,
    rgba: Vec<u8>,
}

/// Where the topmost pixels that are not the background are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Foreground {
    pub pixels: usize,
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

impl Png {
    /// Reads a file: its chunks in order, and every row of pixels.
    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = fs::read(path).map_err(|e| format!("cannot read it: {e}"))?;
        Self::parse(&bytes)
    }

    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut rest = bytes
            .strip_prefix(&SIGNATURE)
            .ok_or("it has no PNG signature")?;
        let mut size = None;
        let mut compressed = Vec::new();
        loop {
            let Chunk {
                kind,
                data,
                rest: after,
            } = next_chunk(rest)?;
            rest = after;
            match &kind {
                b"IHDR" => {
                    let (sides, tail) = data.split_at_checked(8).ok_or("a short IHDR")?;
                    if tail != IHDR_TAIL {
                        return Err(format!("not 8-bit RGBA: {tail:?}"));
                    }
                    let side = |at: usize| {
                        u32::from_be_bytes([sides[at], sides[at + 1], sides[at + 2], sides[at + 3]])
                    };
                    size = Some((side(0), side(4)));
                }
                b"IDAT" => compressed.extend_from_slice(data),
                b"IEND" => break,
                other => return Err(format!("an unexpected chunk {other:?}")),
            }
        }
        let (width, height) = size.ok_or("no IHDR")?;
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&compressed)
            .map_err(|e| format!("cannot inflate it: {e:?}"))?;
        Ok(Self {
            width,
            height,
            rgba: unfilter(&raw, width, height)?,
        })
    }

    /// How many pixels in `columns` of `rows` differ from the background, the bottom right
    /// corner's colour as for [`Png::foreground`].
    pub fn drawn_in(&self, columns: std::ops::Range<u32>, rows: std::ops::Range<u32>) -> usize {
        let Some(background) = self.rgba.last_chunk::<4>() else {
            return 0;
        };
        let stride = self.width as usize * 4;
        rows.filter(|&y| y < self.height)
            .flat_map(|y| {
                columns
                    .clone()
                    .filter(|&x| x < self.width)
                    .map(move |x| (x, y))
            })
            .filter(|&(x, y)| {
                let at = y as usize * stride + x as usize * 4;
                self.rgba.get(at..at + 4) != Some(&background[..])
            })
            .count()
    }

    /// How many pixels in `rows` differ from `other`'s, which has the same size.
    pub fn differs_in(&self, other: &Self, rows: std::ops::Range<u32>) -> usize {
        assert_eq!((self.width, self.height), (other.width, other.height));
        let stride = self.width as usize * 4;
        let (start, end) = (rows.start.min(self.height), rows.end.min(self.height));
        let bytes = start as usize * stride..end as usize * stride;
        let (a, b) = (&self.rgba[bytes.clone()], &other.rgba[bytes]);
        let pixels = a.chunks_exact(4).zip(b.chunks_exact(4));
        pixels.filter(|(a, b)| a != b).count()
    }

    /// What differs from the background in the topmost rows that hold any, down to
    /// the first row that holds none. So the modeline's bar and text, below a gap of
    /// empty rows, do not count. The background is taken to be the bottom right
    /// corner: nothing is drawn there unless the window is full to its last cell.
    pub fn foreground(&self) -> Option<Foreground> {
        let background = self.rgba.last_chunk::<4>()?;
        let stride = self.width as usize * 4;
        let drawn = |row: &[u8]| row.chunks_exact(4).any(|pixel| pixel != background);
        let mut rows = self.rgba.chunks_exact(stride);
        let skipped = rows.by_ref().take_while(|row| !drawn(row)).count();
        let held = 1 + rows.take_while(|row| drawn(row)).count();
        let end = (skipped + held).saturating_mul(stride).min(self.rgba.len());
        let top = &self.rgba[..end];
        let mut found: Option<Foreground> = None;
        for (index, pixel) in top.chunks_exact(4).enumerate() {
            if pixel == background {
                continue;
            }
            let index = u32::try_from(index).expect("a screenshot has few pixels");
            let (x, y) = (index % self.width, index / self.width);
            found = Some(found.map_or(
                Foreground {
                    pixels: 1,
                    left: x,
                    top: y,
                    width: 1,
                    height: 1,
                },
                |seen| {
                    let left = seen.left.min(x);
                    let top = seen.top.min(y);
                    Foreground {
                        pixels: seen.pixels + 1,
                        left,
                        top,
                        width: (seen.left + seen.width).max(x + 1) - left,
                        height: (seen.top + seen.height).max(y + 1) - top,
                    }
                },
            ));
        }
        found
    }
}

/// One chunk of a file, and what follows it.
struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
    rest: &'a [u8],
}

/// The first chunk in `bytes`. The CRC is left to the encoder's own tests.
fn next_chunk(bytes: &[u8]) -> Result<Chunk<'_>, String> {
    let (length, rest) = bytes.split_first_chunk::<4>().ok_or("no more chunks")?;
    let length = u32::from_be_bytes(*length) as usize;
    let (kind, rest) = rest.split_first_chunk::<4>().ok_or("a cut-off chunk")?;
    let data = rest.get(..length).ok_or("a cut-off chunk")?;
    let rest = rest.get(length + 4..).ok_or("a cut-off chunk")?;
    Ok(Chunk {
        kind: *kind,
        data,
        rest,
    })
}

/// The pixels of `raw`, whose rows each start with their filter type. The
/// encoder only uses type 0, which leaves them as they are.
fn unfilter(raw: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let row = width as usize * 4;
    if width == 0 || height == 0 {
        return Err("an empty image".to_owned());
    }
    if raw.len() != (row + 1) * height as usize {
        return Err(format!(
            "{} bytes of pixels for {width} by {height}",
            raw.len()
        ));
    }
    let mut rgba = Vec::with_capacity(row * height as usize);
    for line in raw.chunks_exact(row + 1) {
        let (filter, pixels) = line.split_first().ok_or("an empty row")?;
        if *filter != 0 {
            return Err(format!("a row with filter type {filter}"));
        }
        rgba.extend_from_slice(pixels);
    }
    Ok(rgba)
}
