//! Building a buffer's text and display model from a file's bytes, as they
//! are read (ADR 0022, ADR 0037).
//!
//! A [`Builder`] runs on the loading thread: it decodes each piece, appends
//! it to the rope and brings the display map along, so the UI thread never
//! does O(n) work. [`Builder::snapshot`] is O(1) and is what the UI shows
//! while the rest loads.

use crate::decode::{Decoded, Decoder, FileInfo, InvalidBytes};
use crate::display_map::{DisplayMap, TabWidth};
use crate::rope::Rope;

/// How far a file has loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Bytes read from the file.
    pub read: u64,
    /// The file's size when loading started, if known.
    pub total: Option<u64>,
}

/// The buffer content loaded so far. Every part is an O(1) snapshot that
/// later appends do not change.
#[derive(Clone)]
pub struct LoadSnapshot {
    pub text: Rope,
    pub display: DisplayMap,
    pub invalid: InvalidBytes,
    pub info: FileInfo,
    pub progress: Progress,
    /// The whole file is in.
    pub done: bool,
}

/// Decodes a file piece by piece into a rope, its display map and its
/// invalid bytes.
pub struct Builder {
    decoder: Decoder,
    text: Rope,
    display: DisplayMap,
    invalid: InvalidBytes,
    /// Scratch space for each piece.
    decoded: Decoded,
    progress: Progress,
    done: bool,
}

impl Builder {
    /// A builder for a file of `total` bytes, if known, displayed with
    /// `tab_width`.
    pub fn new(total: Option<u64>, tab_width: TabWidth) -> Self {
        let text = Rope::new();
        Self {
            decoder: Decoder::new(),
            display: DisplayMap::new(&text, tab_width),
            text,
            invalid: InvalidBytes::new(),
            decoded: Decoded::default(),
            progress: Progress { read: 0, total },
            done: false,
        }
    }

    /// Appends the next piece of the file. O(log n) plus the piece's size.
    pub fn push(&mut self, bytes: &[u8]) {
        self.progress.read = self.progress.read.saturating_add(bytes.len() as u64);
        self.decoder.decode(bytes, &mut self.decoded);
        self.append();
    }

    /// Ends the file: an incomplete sequence at its end becomes invalid
    /// bytes.
    pub fn finish(&mut self) {
        self.decoder.finish(&mut self.decoded);
        self.append();
        self.done = true;
    }

    fn append(&mut self) {
        if self.decoded.text.is_empty() {
            return;
        }
        let base = self.text.len();
        let edit = self.text.push_str(&self.decoded.text);
        if self.decoded.invalid.is_empty() {
            self.display.sync(&self.text, &edit);
        } else {
            self.invalid.extend(base, &self.decoded.invalid);
            self.display
                .sync_with_invalid(&self.text, &self.invalid, &edit);
        }
        self.decoded.clear();
    }

    /// The text loaded so far, in bytes.
    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// Whether no text has loaded yet.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The content loaded so far. O(1).
    pub fn snapshot(&self) -> LoadSnapshot {
        LoadSnapshot {
            text: self.text.clone(),
            display: self.display.clone(),
            invalid: self.invalid.clone(),
            info: self.decoder.info(),
            progress: self.progress,
            done: self.done,
        }
    }
}

/// Decodes all of `bytes` at once, as a file of that content would load.
pub fn decode_all(bytes: &[u8], tab_width: TabWidth) -> LoadSnapshot {
    let mut builder = Builder::new(Some(bytes.len() as u64), tab_width);
    builder.push(bytes);
    builder.finish();
    builder.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::LineEnding;
    use crate::display_map::{CellKind, DisplayPoint};
    use crate::sum_tree::Bias;

    #[test]
    fn pieces_build_the_same_buffer_as_the_whole() {
        let bytes = "line 1\r\n日本語\t👍\r\nx\u{301}\n".as_bytes().repeat(200);
        let whole = decode_all(&bytes, TabWidth::default());
        let mut builder = Builder::new(None, TabWidth::default());
        for piece in bytes.chunks(7) {
            builder.push(piece);
        }
        assert!(!builder.snapshot().done);
        builder.finish();
        let pieces = builder.snapshot();
        assert!(pieces.done);
        assert_eq!(pieces.text.to_string(), whole.text.to_string());
        assert_eq!(pieces.info.line_ending, LineEnding::Crlf);
        assert_eq!(pieces.progress.read, bytes.len() as u64);
        let (a, b) = (whole.display.snapshot(), pieces.display.snapshot());
        assert_eq!(a.max_point(), b.max_point());
        for offset in (0..whole.text.len()).step_by(13) {
            assert_eq!(
                a.offset_to_display_point(offset),
                b.offset_to_display_point(offset)
            );
        }
    }

    #[test]
    fn invalid_bytes_show_as_escapes_in_four_cells() {
        let snapshot = decode_all(b"a\xffb\n\xe3\x81", TabWidth::default());
        let display = snapshot.display.snapshot();
        assert_eq!(snapshot.invalid.len(), 3);
        assert_eq!(display.row_cells(0), 1 + 4 + 1);
        assert_eq!(display.row_cells(1), 8);
        let kinds: Vec<_> = display
            .clusters(DisplayPoint::new(0, 0))
            .map(|c| (c.column, c.cells, c.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                (0, 1, CellKind::Text),
                (1, 4, CellKind::InvalidByte(0xff)),
                (5, 1, CellKind::Text),
            ]
        );
        assert_eq!(
            CellKind::InvalidByte(0xff).escape().unwrap().as_str(),
            "\\xFF"
        );
        // A real U+FFFD in the file is text in one cell.
        let real = decode_all("\u{fffd}".as_bytes(), TabWidth::default());
        assert_eq!(real.display.snapshot().row_cells(0), 1);
        // Positions inside an escape map to its start or end.
        let offset = display.display_point_to_offset(DisplayPoint::new(0, 3), Bias::Right);
        assert_eq!(offset, 4);
    }

    #[test]
    fn an_invalid_byte_never_joins_a_cluster() {
        // A combining mark after an invalid byte, and a prepended
        // character (U+0600) before one, would otherwise join its U+FFFD.
        let snapshot = decode_all("\u{600}".as_bytes(), TabWidth::default());
        let alone = snapshot.display.snapshot().row_cells(0);
        let mut bytes = "\u{600}".as_bytes().to_vec();
        bytes.extend_from_slice(b"\xff");
        bytes.extend_from_slice("\u{301}".as_bytes());
        let snapshot = decode_all(&bytes, TabWidth::default());
        let display = snapshot.display.snapshot();
        let kinds: Vec<_> = display
            .clusters(DisplayPoint::new(0, 0))
            .map(|c| c.kind)
            .collect();
        assert_eq!(
            kinds,
            [CellKind::Text, CellKind::InvalidByte(0xff), CellKind::Text]
        );
        assert_eq!(display.row_cells(0), alone + 4 + 1);
    }
}
