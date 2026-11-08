//! Building a buffer's text and display model from a file's bytes, as they
//! are read (ADR 0022, ADR 0040).
//!
//! Must not depend on: file I/O or threads, which the host owns, or `buffer`
//! and what sits above it.
//!
//! A [`Builder`] runs on the loading thread: it decodes each piece, appends
//! it to the rope and brings the display map along, so the UI thread never
//! does O(n) work. [`Builder::snapshot`] is O(1) and is what the UI shows
//! while the rest loads.

use crate::decode::{Decoded, Decoder, FileInfo};
use crate::display_map::{DisplayMap, TabWidth};
use crate::rope::Rope;
use crate::sum_tree::Bias;

/// How far a file has loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Bytes read from the file.
    pub read: u64,
    /// The file's size when loading started, if known.
    pub total: Option<u64>,
}

/// The buffer content loaded so far. Every part is an O(1) snapshot that
/// later appends do not change. Only a [`Builder`] makes one, so its text
/// and display map always agree.
#[derive(Clone)]
pub struct LoadSnapshot {
    pub(crate) text: Rope,
    /// Also holds the invalid bytes.
    pub(crate) display: DisplayMap,
    pub(crate) info: FileInfo,
    pub(crate) progress: Progress,
    pub(crate) done: bool,
}

impl LoadSnapshot {
    /// The text loaded so far.
    pub fn text(&self) -> &Rope {
        &self.text
    }

    /// How far the file has loaded.
    pub fn progress(&self) -> Progress {
        self.progress
    }

    /// Whether the whole file is in.
    pub fn done(&self) -> bool {
        self.done
    }
}

/// Decodes a file piece by piece into a rope, its display map and its
/// invalid bytes.
pub struct Builder {
    decoder: Decoder,
    text: Rope,
    display: DisplayMap,
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
        if !self.decoded.invalid.is_empty() {
            self.display.extend_invalid(base, &self.decoded.invalid);
        }
        self.display.sync(&self.text, &edit);
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
            info: self.decoder.info(),
            progress: self.progress,
            done: self.done,
        }
    }
}

/// Where a file read again differs from the text before it (ADR 0083): bytes
/// `start..old_end` of the old text became `start..new_end` of the new. Each end is on a
/// character boundary in both texts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Change {
    pub(crate) start: usize,
    pub(crate) old_end: usize,
    pub(crate) new_end: usize,
}

/// A file read again in full, with where it differs from the text it is to replace.
#[derive(Clone)]
pub struct Reloaded {
    pub(crate) snapshot: LoadSnapshot,
    /// `None` if the text is the same.
    pub(crate) change: Option<Change>,
}

impl Reloaded {
    /// Whether the text differs from the one it was compared with.
    pub fn is_changed(&self) -> bool {
        self.change.is_some()
    }
}

impl LoadSnapshot {
    /// Compares this whole text with `old`, which it is to replace. O(n), so a loading
    /// thread does it, not the UI.
    pub fn compared_with(self, old: &Rope) -> Reloaded {
        let change = changed_span(old, &self.text);
        Reloaded {
            snapshot: self,
            change,
        }
    }
}

/// The one span outside which `old` and `new` are the same, or `None` if they are equal.
fn changed_span(old: &Rope, new: &Rope) -> Option<Change> {
    let prefix = old.common_prefix(new);
    if prefix == old.len() && prefix == new.len() {
        return None;
    }
    // The bytes before `prefix` are the same, so a character starts at the same place in both;
    // the one at `prefix` differs, and may be a character's middle in one of them.
    let start = old
        .clip_offset(prefix, Bias::Left)
        .min(new.clip_offset(prefix, Bias::Left));
    let limit = old.len().min(new.len()) - start;
    let suffix = old.common_suffix(new, limit);
    // The bytes from here on are the same, so a character starts at the same distance from
    // the end in both.
    let old_end = old.clip_offset(old.len() - suffix, Bias::Right);
    let new_end = new.len() - (old.len() - old_end);
    Some(Change {
        start,
        old_end,
        new_end,
    })
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
    fn a_snapshot_reports_what_has_loaded() {
        let mut builder = Builder::new(Some(6), TabWidth::default());
        assert!(builder.is_empty());
        assert_eq!(builder.len(), 0);
        builder.push(b"abc");
        assert!(!builder.is_empty());
        assert_eq!(builder.len(), 3);
        let partial = builder.snapshot();
        assert_eq!(partial.text().to_string(), "abc");
        assert_eq!(
            partial.progress(),
            Progress {
                read: 3,
                total: Some(6)
            }
        );
        assert!(!partial.done());
        builder.finish();
        assert!(builder.snapshot().done());
    }

    #[test]
    fn a_snapshot_shares_the_invalid_bytes_of_its_map() {
        let display = decode_all(b"a\xffb", TabWidth::default())
            .display
            .snapshot();
        assert_eq!(display.invalid_bytes().get(1), Some(0xff));
    }

    #[test]
    fn a_row_read_from_a_column_past_an_invalid_byte_keeps_the_next_one() {
        let display = decode_all(b"a\xffb\xfec", TabWidth::default())
            .display
            .snapshot();
        let kinds = |column| -> Vec<CellKind> {
            display
                .clusters(DisplayPoint::new(0, column))
                .map(|cluster| cluster.kind)
                .collect()
        };
        assert_eq!(
            kinds(5),
            [CellKind::Text, CellKind::InvalidByte(0xfe), CellKind::Text]
        );
        assert_eq!(kinds(6), [CellKind::InvalidByte(0xfe), CellKind::Text]);
    }

    #[test]
    fn invalid_bytes_after_a_line_break_in_a_later_chunk_are_measured() {
        // `TabSummary::from_text` measures only a chunk's last line, at its
        // offset in the buffer, so a wrong base would miss the 4-cell escape.
        let pad = "a".repeat(crate::rope::MAX_CHUNK_BYTES * 3);
        let bytes = [pad.as_bytes(), b"\n\xffb"].concat();
        let display = decode_all(&bytes, TabWidth::default()).display.snapshot();
        assert_eq!(display.row_cells(1), 5);
    }

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
        assert_eq!(snapshot.display.invalid_bytes().len(), 3);
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

    /// Bytes mixing text, line breaks, controls and invalid sequences.
    fn bytes() -> impl proptest::strategy::Strategy<Value = Vec<u8>> {
        use proptest::prelude::*;
        let piece = prop_oneof![
            Just(b"a".to_vec()),
            Just(b"\t".to_vec()),
            Just(b"\r\n".to_vec()),
            Just(b"\n".to_vec()),
            Just(b"\r".to_vec()),
            Just("日\u{301}".as_bytes().to_vec()),
            Just("\u{600}".as_bytes().to_vec()),
            Just("\u{202e}".as_bytes().to_vec()),
            Just("👨\u{200d}👩".as_bytes().to_vec()),
            Just("\u{fffd}".as_bytes().to_vec()),
            any::<u8>().prop_map(|b| vec![b]),
        ];
        prop::collection::vec(piece, 0..60).prop_map(|pieces| pieces.concat())
    }

    proptest::proptest! {
        /// Any bytes, split anywhere, build the same buffer as decoding
        /// them whole, and every cursor stop maps back to itself.
        #[test]
        fn prop_pieces_match_the_whole(bytes in bytes(), splits in proptest::collection::vec(0usize..300, 0..8)) {
            let whole = decode_all(&bytes, TabWidth::default());
            let mut builder = Builder::new(None, TabWidth::default());
            let mut splits: Vec<usize> = splits.into_iter().map(|s| s.min(bytes.len())).collect();
            splits.sort_unstable();
            let mut from = 0;
            for split in splits.into_iter().chain([bytes.len()]) {
                builder.push(&bytes[from..split]);
                from = split;
            }
            builder.finish();
            let pieces = builder.snapshot();
            proptest::prop_assert_eq!(pieces.text.to_string(), whole.text.to_string());
            proptest::prop_assert_eq!(
                pieces.display.invalid_bytes().len(),
                whole.display.invalid_bytes().len()
            );
            let (a, b) = (whole.display.snapshot(), pieces.display.snapshot());
            proptest::prop_assert_eq!(a.max_point(), b.max_point());
            // Every cluster start maps to a point and back.
            for row in 0..b.row_count() {
                for cluster in b.clusters(DisplayPoint::new(row, 0)) {
                    let point = b.offset_to_display_point(cluster.offset);
                    proptest::prop_assert_eq!(point, a.offset_to_display_point(cluster.offset));
                    proptest::prop_assert_eq!(point.column, cluster.column);
                    proptest::prop_assert_eq!(b.display_point_to_offset(point, Bias::Left), cluster.offset);
                }
            }
        }
    }

    fn change(old: &str, new: &str) -> Option<Change> {
        let snapshot = decode_all(new.as_bytes(), TabWidth::default());
        snapshot.compared_with(&Rope::from(old)).change
    }

    #[test]
    fn a_text_read_again_says_where_it_changed() {
        assert_eq!(change("same\n", "same\n"), None);
        assert_eq!(change("", ""), None);
        let at = |start, old_end, new_end| {
            Some(Change {
                start,
                old_end,
                new_end,
            })
        };
        assert_eq!(change("abcdef", "abXYef"), at(2, 4, 4));
        assert_eq!(change("abc", "abcd"), at(3, 3, 4), "appended");
        assert_eq!(change("abcd", "abc"), at(3, 4, 3), "cut");
        assert_eq!(change("", "new"), at(0, 0, 3));
        assert_eq!(change("old", ""), at(0, 3, 0));
        // The shared part never counts twice: "aa" in "aaa" is one insertion.
        assert_eq!(change("aa", "aaa"), at(2, 2, 3));
    }

    #[test]
    fn a_change_starts_and_ends_on_characters() {
        // é and è share their first byte, and their last bytes differ.
        assert_eq!(
            change("aé", "aè"),
            Some(Change {
                start: 1,
                old_end: 3,
                new_end: 3
            })
        );
        // ü and ö share their first byte; the suffix "x" is the same.
        assert_eq!(
            change("üx", "öx"),
            Some(Change {
                start: 0,
                old_end: 2,
                new_end: 2
            })
        );
    }

    proptest::proptest! {
        #[test]
        fn the_change_rebuilds_the_new_text(old in "[aé\\n🍣]{0,80}", new in "[aé\\n🍣]{0,80}") {
            match change(&old, &new) {
                None => proptest::prop_assert_eq!(&old, &new),
                Some(c) => {
                    proptest::prop_assert!(c.start <= c.old_end && c.start <= c.new_end);
                    proptest::prop_assert!(old.is_char_boundary(c.start) && new.is_char_boundary(c.start));
                    proptest::prop_assert!(old.is_char_boundary(c.old_end) && new.is_char_boundary(c.new_end));
                    proptest::prop_assert_eq!(&old[..c.start], &new[..c.start]);
                    proptest::prop_assert_eq!(&old[c.old_end..], &new[c.new_end..]);
                }
            }
        }
    }
}
