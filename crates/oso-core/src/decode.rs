//! Decoding a file into the rope's UTF-8, piece by piece as it is read
//! (ADR 0022, ADR 0040).
//!
//! Must not depend on: `rope`, `display_map` or file I/O; the caller hands it
//! the bytes.
//!
//! - Each invalid byte becomes one U+FFFD in the text, and its value is kept
//!   in [`InvalidBytes`], so the display can show it as `\xNN` and the
//!   buffer is never saved.
//! - A UTF-8 sequence split between two reads is joined, not reported as
//!   invalid; only a sequence still incomplete at the end of the file is.
//! - A UTF-8 BOM at the start is stripped and remembered.
//! - The line ending is the first line break's, even if it is split
//!   between reads. Line breaks stay as they are in the text.
//! - A NUL byte in the first 8 KB makes the file binary.

use std::ops::Range;
use std::sync::Arc;

use crate::sum_tree::{Bias, Cursor, Dimension, Item, SumTree, Summary};

/// How many bytes at the start of a file are searched for a NUL byte.
pub(crate) const BINARY_SNIFF_BYTES: u64 = 8 * 1024;

/// The UTF-8 byte order mark, which a file may start with and the rope never holds.
pub const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The text an invalid byte becomes in the rope.
pub(crate) const REPLACEMENT: char = '\u{FFFD}';

/// A file's line ending: the first line break's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LineEnding {
    /// `\n`, also for a file without a line break.
    #[default]
    Lf,
    /// `\r\n`.
    Crlf,
}

impl LineEnding {
    /// `"lf"` or `"crlf"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "lf",
            Self::Crlf => "crlf",
        }
    }

    /// The line break itself: `"\n"` or `"\r\n"`.
    pub fn text(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

/// What the decoder learned about a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileInfo {
    /// The file started with a UTF-8 BOM, which is not in the text.
    pub bom: bool,
    /// The first line break's ending.
    pub line_ending: LineEnding,
    /// A NUL byte is in the first 8 KB.
    pub binary: bool,
}

/// The text decoded from one piece of a file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Decoded {
    /// Valid UTF-8, with a U+FFFD for each invalid byte.
    pub(crate) text: String,
    /// The invalid bytes: the offset of each one's U+FFFD in `text`, and
    /// the byte.
    pub(crate) invalid: Vec<(usize, u8)>,
}

impl Decoded {
    /// Empties the text and the invalid bytes, keeping their capacity.
    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.invalid.clear();
    }

    fn push_str(&mut self, text: &str) {
        self.text.push_str(text);
    }

    fn push_invalid(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.invalid.push((self.text.len(), byte));
            self.text.push(REPLACEMENT);
        }
    }
}

/// A streaming UTF-8 decoder. Feed it the file's bytes in any pieces with
/// [`Decoder::decode`], then call [`Decoder::finish`]. The concatenated
/// output is the same however the file was split.
#[derive(Debug, Default)]
pub(crate) struct Decoder {
    /// Bytes not decoded yet: the start of the file while the BOM is
    /// undecided, or an incomplete UTF-8 sequence. At most 3 bytes.
    carry: Vec<u8>,
    bom_decided: bool,
    info: FileInfo,
    line_ending_found: bool,
    /// The last byte seen was `\r`, for a `\r\n` split between pieces.
    after_cr: bool,
    /// Bytes fed so far.
    fed: u64,
}

impl Decoder {
    /// A decoder at the start of a file.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// What is known about the file so far.
    pub(crate) fn info(&self) -> FileInfo {
        self.info
    }

    /// Decodes the next piece of the file, appending to `out`. A sequence
    /// left incomplete at the end of `input` waits for the next piece.
    pub(crate) fn decode(&mut self, input: &[u8], out: &mut Decoded) {
        self.inspect(input);
        let mut input = input;
        if !self.bom_decided {
            while !input.is_empty() && self.carry.len() < BOM.len() {
                self.carry.push(input[0]);
                input = &input[1..];
                if !BOM.starts_with(&self.carry) {
                    break;
                }
            }
            if self.carry.len() < BOM.len() && BOM.starts_with(&self.carry) {
                // Still a prefix of the BOM: wait for more.
                return;
            }
            self.decide_bom();
        }
        self.decode_carry(&mut input, out);
        if self.carry.is_empty() {
            self.decode_slice(input, out);
        }
    }

    /// Decodes what is left at the end of the file: bytes of an incomplete
    /// sequence are invalid.
    pub(crate) fn finish(&mut self, out: &mut Decoded) {
        self.decode_carry(&mut &[][..], out);
        out.push_invalid(&self.carry);
        self.carry.clear();
    }

    fn decide_bom(&mut self) {
        self.bom_decided = true;
        if self.carry == BOM {
            self.info.bom = true;
            self.carry.clear();
        }
    }

    /// Notes the line ending and NUL bytes in the raw input.
    fn inspect(&mut self, input: &[u8]) {
        if self.fed < BINARY_SNIFF_BYTES {
            let sniff = (BINARY_SNIFF_BYTES - self.fed).min(input.len() as u64) as usize;
            self.info.binary |= input[..sniff].contains(&0);
        }
        self.fed = self.fed.saturating_add(input.len() as u64);
        if self.line_ending_found {
            return;
        }
        if let Some(newline) = input.iter().position(|&byte| byte == b'\n') {
            let cr = match newline {
                0 => self.after_cr,
                _ => input[newline - 1] == b'\r',
            };
            self.line_ending_found = true;
            self.info.line_ending = if cr { LineEnding::Crlf } else { LineEnding::Lf };
        } else if let Some(&last) = input.last() {
            self.after_cr = last == b'\r';
        }
    }

    /// Decodes the carried bytes, taking bytes from `input` while they end
    /// in an incomplete sequence. Leaves `carry` empty, or incomplete with
    /// `input` used up.
    fn decode_carry(&mut self, input: &mut &[u8], out: &mut Decoded) {
        while !self.carry.is_empty() {
            match std::str::from_utf8(&self.carry) {
                Ok(text) => {
                    out.push_str(text);
                    self.carry.clear();
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.carry[..valid]).unwrap_or_default());
                    match e.error_len() {
                        Some(len) => {
                            out.push_invalid(&self.carry[valid..valid + len]);
                            self.carry.drain(..valid + len);
                        }
                        None => {
                            self.carry.drain(..valid);
                            let Some((&byte, rest)) = input.split_first() else {
                                return;
                            };
                            self.carry.push(byte);
                            *input = rest;
                        }
                    }
                }
            }
        }
    }

    /// Decodes `input`, carrying an incomplete sequence at its end.
    fn decode_slice(&mut self, input: &[u8], out: &mut Decoded) {
        let mut chunks = input.utf8_chunks().peekable();
        while let Some(chunk) = chunks.next() {
            out.push_str(chunk.valid());
            let invalid = chunk.invalid();
            // Only the last piece can be the start of a sequence that the
            // next input completes.
            let incomplete = chunks.peek().is_none()
                && !invalid.is_empty()
                && std::str::from_utf8(invalid).is_err_and(|e| e.error_len().is_none());
            if incomplete {
                self.carry.extend_from_slice(invalid);
            } else {
                out.push_invalid(invalid);
            }
        }
    }
}

/// The invalid bytes of a buffer: where their U+FFFD is in the rope, and
/// their values. Cloning is O(1); lookups take O(log n).
///
/// Bytes are appended in order as a file loads. Buffers that have any are
/// read-only, so their offsets never shift.
#[derive(Clone, Default)]
pub struct InvalidBytes {
    tree: SumTree<Block>,
}

impl std::fmt::Debug for InvalidBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvalidBytes")
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

/// The widest range of offsets one block covers: entries store their
/// offset from the block's start in 24 bits.
const MAX_BLOCK_SPAN: usize = 1 << 24;

/// Invalid bytes appended together. Each entry packs the offset from
/// `start` (24 bits) and the byte (8 bits): 4 bytes per invalid byte.
#[derive(Clone, Debug)]
struct Block {
    start: usize,
    entries: Arc<[u32]>,
    summary: BlockSummary,
}

impl Block {
    fn entry(&self, index: usize) -> (usize, u8) {
        let entry = self.entries[index];
        (self.start + (entry >> 8) as usize, entry as u8)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BlockSummary {
    count: usize,
    /// The end of the last U+FFFD.
    end: usize,
}

impl Summary for BlockSummary {
    fn add_summary(&mut self, other: &Self) {
        self.count += other.count;
        self.end = self.end.max(other.end);
    }
}

impl Item for Block {
    type Summary = BlockSummary;

    fn summary(&self) -> BlockSummary {
        self.summary
    }
}

/// The end of the blocks so far: what blocks are sought by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct End(usize);

impl Dimension<BlockSummary> for End {
    fn add_summary(&mut self, summary: &BlockSummary) {
        self.0 = self.0.max(summary.end);
    }
}

impl InvalidBytes {
    /// No invalid bytes.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of invalid bytes.
    pub fn len(&self) -> usize {
        self.tree.summary().count
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Appends invalid bytes at `base` plus each entry's offset. The
    /// offsets must ascend and come after every byte already held.
    pub(crate) fn extend(&mut self, base: usize, entries: &[(usize, u8)]) {
        let mut rest = entries;
        while let Some(&(first, _)) = rest.first() {
            let start = base + first;
            let len = rest.partition_point(|&(offset, _)| offset - first < MAX_BLOCK_SPAN);
            let (block, tail) = rest.split_at(len);
            rest = tail;
            let entries: Arc<[u32]> = block
                .iter()
                .map(|&(offset, byte)| (((offset - first) as u32) << 8) | u32::from(byte))
                .collect();
            let last = block.last().map_or(first, |&(offset, _)| offset);
            self.tree.push(Block {
                start,
                summary: BlockSummary {
                    count: entries.len(),
                    end: base + last + REPLACEMENT.len_utf8(),
                },
                entries,
            });
        }
    }

    /// The invalid byte whose U+FFFD starts at `offset`, if any.
    pub fn get(&self, offset: usize) -> Option<u8> {
        self.range(offset..offset + 1).next().map(|(_, byte)| byte)
    }

    /// The invalid bytes whose U+FFFD starts in `range`, in order.
    pub fn range(&self, range: Range<usize>) -> InvalidIter<'_> {
        if self.is_empty() || range.is_empty() {
            return InvalidIter {
                cursor: None,
                index: 0,
                end: 0,
            };
        }
        let mut cursor = self.tree.cursor::<End>();
        cursor.seek(&End(range.start), Bias::Right);
        let index = cursor.item().map_or(0, |block| {
            let from = range.start.saturating_sub(block.start);
            block
                .entries
                .partition_point(|&entry| ((entry >> 8) as usize) < from)
        });
        InvalidIter {
            cursor: Some(cursor),
            index,
            end: range.end,
        }
    }
}

/// Iterates over invalid bytes in a range. See [`InvalidBytes::range`].
pub struct InvalidIter<'a> {
    cursor: Option<Cursor<'a, Block, End>>,
    /// The next entry in the current block.
    index: usize,
    end: usize,
}

impl Iterator for InvalidIter<'_> {
    /// The offset of the byte's U+FFFD, and the byte.
    type Item = (usize, u8);

    fn next(&mut self) -> Option<(usize, u8)> {
        let cursor = self.cursor.as_mut()?;
        loop {
            let block = cursor.item()?;
            if self.index < block.entries.len() {
                let (offset, byte) = block.entry(self.index);
                if offset >= self.end {
                    self.cursor = None;
                    return None;
                }
                self.index += 1;
                return Some((offset, byte));
            }
            cursor.next();
            self.index = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The whole file decoded at once, by a simple reference: per byte,
    /// with `std`'s validation.
    fn reference(bytes: &[u8]) -> (Decoded, FileInfo) {
        let mut info = FileInfo {
            binary: bytes[..bytes.len().min(BINARY_SNIFF_BYTES as usize)].contains(&0),
            ..FileInfo::default()
        };
        let mut rest = bytes;
        if rest.starts_with(BOM) {
            info.bom = true;
            rest = &rest[3..];
        }
        if let Some(newline) = bytes.iter().position(|&b| b == b'\n')
            && newline > 0
            && bytes[newline - 1] == b'\r'
        {
            info.line_ending = LineEnding::Crlf;
        }
        let mut out = Decoded::default();
        while !rest.is_empty() {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&rest[..valid]).unwrap());
                    let len = e.error_len().unwrap_or(rest.len() - valid);
                    out.push_invalid(&rest[valid..valid + len]);
                    rest = &rest[valid + len..];
                }
            }
        }
        (out, info)
    }

    /// Decodes `bytes` split at `splits`.
    fn decode_split(bytes: &[u8], splits: &[usize]) -> (Decoded, FileInfo) {
        let mut decoder = Decoder::new();
        let mut whole = Decoded::default();
        let mut piece = Decoded::default();
        let mut base = 0;
        let mut from = 0;
        let mut splits: Vec<usize> = splits.iter().map(|&s| s.min(bytes.len())).collect();
        splits.sort_unstable();
        splits.push(bytes.len());
        let mut feed = |piece: &mut Decoded, whole: &mut Decoded| {
            whole.text.push_str(&piece.text);
            whole
                .invalid
                .extend(piece.invalid.iter().map(|&(o, b)| (base + o, b)));
            base += piece.text.len();
            piece.clear();
        };
        for split in splits {
            decoder.decode(&bytes[from..split], &mut piece);
            feed(&mut piece, &mut whole);
            from = split;
        }
        decoder.finish(&mut piece);
        feed(&mut piece, &mut whole);
        (whole, decoder.info())
    }

    fn check(bytes: &[u8], splits: &[usize]) {
        let expected = reference(bytes);
        assert_eq!(
            decode_split(bytes, splits),
            expected,
            "{bytes:x?} split at {splits:?}"
        );
    }

    #[test]
    fn valid_text_passes_through() {
        let text = "abc 日本語 🍣\r\nx\u{301}\n";
        let (out, info) = decode_split(text.as_bytes(), &[1, 5, 6, 7, 14]);
        assert_eq!(out.text, text);
        assert!(out.invalid.is_empty());
        assert_eq!(
            info,
            FileInfo {
                bom: false,
                line_ending: LineEnding::Crlf,
                binary: false
            }
        );
    }

    #[test]
    fn a_sequence_split_between_reads_is_joined() {
        let bytes = "日".as_bytes();
        for split in 0..=bytes.len() {
            let (out, _) = decode_split(bytes, &[split]);
            assert_eq!(out.text, "日", "split at {split}");
        }
        let (out, _) = decode_split("🍣".as_bytes(), &[1, 2, 3]);
        assert_eq!(out.text, "🍣");
    }

    #[test]
    fn line_endings_have_names() {
        assert_eq!(LineEnding::Lf.as_str(), "lf");
        assert_eq!(LineEnding::Crlf.as_str(), "crlf");
    }

    #[test]
    fn each_invalid_byte_becomes_one_replacement() {
        let (out, _) = decode_split(b"a\xff\xfeb\xe3\x81c", &[]);
        assert_eq!(out.text, "a\u{fffd}\u{fffd}b\u{fffd}\u{fffd}c");
        assert_eq!(out.invalid, [(1, 0xff), (4, 0xfe), (8, 0xe3), (11, 0x81)]);
    }

    #[test]
    fn a_sequence_incomplete_at_the_end_is_invalid() {
        for split in 0..=3 {
            let (out, _) = decode_split(b"a\xe3\x81", &[split]);
            assert_eq!(out.text, "a\u{fffd}\u{fffd}", "split at {split}");
            assert_eq!(out.invalid, [(1, 0xe3), (4, 0x81)]);
        }
    }

    #[test]
    fn the_bom_is_stripped_and_remembered() {
        let bytes = b"\xef\xbb\xbfhi";
        for split in 0..=bytes.len() {
            let (out, info) = decode_split(bytes, &[split]);
            assert_eq!(out.text, "hi");
            assert!(info.bom);
        }
        // A BOM elsewhere is text (U+FEFF), and a partial one is invalid.
        let (out, info) = decode_split(b"a\xef\xbb\xbf", &[]);
        assert_eq!(out.text, "a\u{feff}");
        assert!(!info.bom);
        let (out, info) = decode_split(b"\xef\xbb", &[1]);
        assert_eq!(out.text, "\u{fffd}\u{fffd}");
        assert!(!info.bom);
    }

    #[test]
    fn crlf_split_between_reads_is_detected() {
        let (_, info) = decode_split(b"ab\r\ncd\n", &[3]);
        assert_eq!(info.line_ending, LineEnding::Crlf);
        let (_, info) = decode_split(b"ab\ncd\r\n", &[3]);
        assert_eq!(info.line_ending, LineEnding::Lf, "the first one decides");
        let (_, info) = decode_split(b"a\rb", &[2]);
        assert_eq!(info.line_ending, LineEnding::Lf, "a lone CR is no break");
    }

    #[test]
    fn a_nul_in_the_first_8_kb_makes_a_binary_file() {
        let mut bytes = vec![b'a'; 9000];
        bytes[8191] = 0;
        assert!(decode_split(&bytes, &[8000]).1.binary);
        bytes[8191] = b'a';
        bytes[8192] = 0;
        assert!(!decode_split(&bytes, &[8000]).1.binary);
        // The NUL itself is text: the display shows it as `^@`.
        assert_eq!(decode_split(b"a\0b", &[]).0.text, "a\0b");
    }

    #[test]
    fn invalid_bytes_are_found_by_offset() {
        let mut invalid = InvalidBytes::new();
        assert_eq!(invalid.range(0..100).next(), None);
        invalid.extend(10, &[(0, 0x80), (3, 0x81)]);
        invalid.extend(100, &[(5, 0xff)]);
        assert_eq!(invalid.len(), 3);
        assert_eq!(invalid.get(10), Some(0x80));
        assert_eq!(invalid.get(13), Some(0x81));
        assert_eq!(invalid.get(11), None);
        assert_eq!(invalid.get(105), Some(0xff));
        assert_eq!(
            invalid.range(11..106).collect::<Vec<_>>(),
            [(13, 0x81), (105, 0xff)]
        );
        assert_eq!(invalid.range(14..105).collect::<Vec<_>>(), []);
        assert_eq!(invalid.range(0..usize::MAX).count(), 3);
    }

    #[test]
    fn blocks_split_where_offsets_exceed_24_bits() {
        let mut invalid = InvalidBytes::new();
        let far = MAX_BLOCK_SPAN * 2 + 7;
        invalid.extend(3, &[(0, 0x80), (MAX_BLOCK_SPAN, 0x81), (far, 0x82)]);
        assert_eq!(invalid.get(3), Some(0x80));
        assert_eq!(invalid.get(3 + MAX_BLOCK_SPAN), Some(0x81));
        assert_eq!(invalid.get(3 + far), Some(0x82));
        assert_eq!(invalid.range(4..usize::MAX).count(), 2);
    }

    /// Bytes that exercise every case: ASCII, line breaks, multi-byte
    /// sequences, their pieces, a BOM and NUL.
    fn bytes_strategy() -> impl Strategy<Value = Vec<u8>> {
        let piece = prop_oneof![
            Just(b"a".to_vec()),
            Just(b"\n".to_vec()),
            Just(b"\r\n".to_vec()),
            Just(b"\r".to_vec()),
            Just(b"\0".to_vec()),
            Just("日".as_bytes().to_vec()),
            Just("🍣".as_bytes().to_vec()),
            Just(BOM.to_vec()),
            Just(b"\xe3\x81".to_vec()),
            Just(b"\xf0\x9f".to_vec()),
            Just(b"\xc0\xaf".to_vec()),
            Just(b"\xed\xa0\x80".to_vec()),
            (0x80u8..=0xff).prop_map(|b| vec![b]),
        ];
        prop::collection::vec(piece, 0..40).prop_map(|pieces| pieces.concat())
    }

    proptest! {
        /// However the file is split, the output matches decoding it whole.
        #[test]
        fn prop_splits_do_not_change_the_output(
            bytes in bytes_strategy(),
            splits in prop::collection::vec(0usize..200, 0..12),
        ) {
            check(&bytes, &splits);
            // Every byte on its own is the hardest split.
            let every: Vec<usize> = (0..bytes.len()).collect();
            check(&bytes, &every);
        }

        /// The side table finds exactly the invalid bytes of the output.
        #[test]
        fn prop_the_side_table_matches_the_output(bytes in bytes_strategy(), start in 0usize..60, len in 0usize..60) {
            let (out, _) = decode_split(&bytes, &[]);
            let mut invalid = InvalidBytes::new();
            invalid.extend(0, &out.invalid);
            prop_assert_eq!(invalid.len(), out.invalid.len());
            let expected: Vec<_> = out.invalid.iter().copied()
                .filter(|&(o, _)| (start..start + len).contains(&o)).collect();
            prop_assert_eq!(invalid.range(start..start + len).collect::<Vec<_>>(), expected);
            for &(offset, _) in &out.invalid {
                prop_assert_eq!(&out.text[offset..offset + 3], "\u{fffd}");
            }
        }
    }
}
