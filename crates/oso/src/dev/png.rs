//! A minimal PNG encoder for screenshots: 8-bit RGBA, one IDAT chunk,
//! deflate through `miniz_oxide`.

use oso_render::Image;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// Bit depth 8, color type 6 (RGBA), default compression and filter
/// methods, no interlace.
const IHDR_TAIL: [u8; 5] = [8, 6, 0, 0, 0];

/// The deflate level: fast enough for a full frame, and flat editor colors
/// compress well anyway.
const LEVEL: u8 = 6;

/// Encodes `image` as a PNG file.
pub fn encode(image: &Image) -> Vec<u8> {
    let row = image.width as usize * 4;
    // Each row starts with its filter type; 0 leaves the bytes as they are.
    let mut raw = Vec::with_capacity((row + 1) * image.height as usize);
    for pixels in image.rgba.chunks_exact(row.max(1)) {
        raw.push(0);
        raw.extend_from_slice(pixels);
    }
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, LEVEL);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&image.width.to_be_bytes());
    ihdr.extend_from_slice(&image.height.to_be_bytes());
    ihdr.extend_from_slice(&IHDR_TAIL);

    let mut png = Vec::with_capacity(compressed.len() + 64);
    png.extend_from_slice(&SIGNATURE);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &compressed);
    chunk(&mut png, b"IEND", &[]);
    png
}

/// Appends one chunk: length, type, data and the CRC of type and data.
fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    let len = u32::try_from(data.len()).expect("a PNG chunk is smaller than 4 GiB");
    png.extend_from_slice(&len.to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let crc = crc32_update(crc32_update(!0, kind), data);
    png.extend_from_slice(&(!crc).to_be_bytes());
}

/// CRC-32 (ISO 3309, as PNG uses), one table entry per byte value.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
};

/// Continues a CRC over `bytes`. Start from `!0` and invert the result.
fn crc32_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc = CRC_TABLE[((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::{SIGNATURE, crc32_update, encode};
    use oso_render::Image;

    fn crc32(bytes: &[u8]) -> u32 {
        !crc32_update(!0, bytes)
    }

    /// Splits a PNG into its chunks, checking the signature and every CRC.
    fn chunks(png: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        assert_eq!(png[..8], SIGNATURE);
        let mut rest = &png[8..];
        let mut chunks = Vec::new();
        while !rest.is_empty() {
            let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = rest[4..8].try_into().unwrap();
            let data = rest[8..8 + len].to_vec();
            let crc = u32::from_be_bytes(rest[8 + len..12 + len].try_into().unwrap());
            assert_eq!(crc, crc32(&rest[4..8 + len]), "CRC of {kind:?}");
            chunks.push((kind, data));
            rest = &rest[12 + len..];
        }
        chunks
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn encodes_header_pixels_and_end() {
        let image = Image {
            width: 2,
            height: 3,
            rgba: (0..24).collect(),
        };
        let chunks = chunks(&encode(&image));
        let kinds: Vec<&[u8; 4]> = chunks.iter().map(|(kind, _)| kind).collect();
        assert_eq!(kinds, [b"IHDR", b"IDAT", b"IEND"]);

        let ihdr = &chunks[0].1;
        assert_eq!(ihdr[..4], 2u32.to_be_bytes());
        assert_eq!(ihdr[4..8], 3u32.to_be_bytes());
        assert_eq!(ihdr[8..], [8, 6, 0, 0, 0]);

        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&chunks[1].1).unwrap();
        let mut expected = Vec::new();
        for row in image.rgba.chunks(8) {
            expected.push(0);
            expected.extend_from_slice(row);
        }
        assert_eq!(raw, expected);
        assert!(chunks[2].1.is_empty());
    }

    #[test]
    fn flat_frames_compress_well() {
        let image = Image {
            width: 800,
            height: 600,
            rgba: [13, 13, 18, 255].repeat(800 * 600),
        };
        let png = encode(&image);
        assert!(png.len() < 20_000, "{} bytes", png.len());
    }
}
