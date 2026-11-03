//! The cell grid: how many cells each grapheme cluster takes (ADR 0005,
//! ADR 0035).
//!
//! - A cluster takes the East Asian Width of its text: 1 cell for half
//!   width, 2 for full width, with emoji sequences counted as one 2-cell
//!   glyph. Ambiguous-width characters take 1 cell.
//! - Every cluster takes at least 1 cell and at most 2, so zero-width
//!   characters such as ZWSP or bidi controls stay visible and the cursor can
//!   stop on them.
//! - Control characters are shown as escapes, as in Emacs: caret notation in
//!   2 cells for C0 and DEL (`^@`, `^M`, `^?`), and an octal escape in 4
//!   cells for C1 (`\200`). A lone `\r` is a control character.
//! - A tab advances to the next tab stop. `\n` and `\r\n` end the row and
//!   take no cells.

use std::num::NonZeroU32;

use unicode_segmentation::GraphemeCursor;
use unicode_width::UnicodeWidthStr;

/// What a grapheme cluster is on the cell grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CellKind {
    /// Text drawn in 1 or 2 cells.
    Text,
    /// A tab, filling the cells up to the next tab stop.
    Tab,
    /// A control character, drawn as its [`control_escape`].
    Control(char),
    /// `\n` or `\r\n`. Ends the row and takes no cells.
    LineBreak,
}

/// Classifies a grapheme cluster and gives its width in cells. A tab's width
/// depends on its column and is given as 0 here.
pub fn classify(cluster: &str) -> (CellKind, usize) {
    match cluster {
        "\n" | "\r\n" => return (CellKind::LineBreak, 0),
        "\t" => return (CellKind::Tab, 0),
        _ => {}
    }
    let mut chars = cluster.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && c.is_control()
    {
        let cells = if u32::from(c) < 0x80 { 2 } else { 4 };
        return (CellKind::Control(c), cells);
    }
    (CellKind::Text, cluster.width().clamp(1, 2))
}

/// The cells one ASCII byte takes, for the fast path. `\r\n` is handled by
/// the caller.
fn classify_ascii(byte: u8) -> (CellKind, usize) {
    match byte {
        b'\n' => (CellKind::LineBreak, 0),
        b'\t' => (CellKind::Tab, 0),
        0x20..=0x7e => (CellKind::Text, 1),
        _ => (CellKind::Control(char::from(byte)), 2),
    }
}

/// The text a control character is drawn as: `^@` to `^_` and `^?` for C0
/// and DEL, `\200` to `\237` for C1. `None` for other characters.
pub fn control_escape(c: char) -> Option<ControlEscape> {
    let code = u32::from(c);
    let mut bytes = [0; 4];
    let len = match code {
        0x00..=0x1f | 0x7f => {
            bytes[0] = b'^';
            bytes[1] = (code as u8) ^ 0x40;
            2
        }
        0x80..=0x9f => {
            bytes[0] = b'\\';
            bytes[1] = b'0' + (code >> 6) as u8;
            bytes[2] = b'0' + ((code >> 3) & 7) as u8;
            bytes[3] = b'0' + (code & 7) as u8;
            4
        }
        _ => return None,
    };
    Some(ControlEscape { bytes, len })
}

/// The escape text of a control character. See [`control_escape`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlEscape {
    bytes: [u8; 4],
    len: usize,
}

impl ControlEscape {
    /// The escape as text, for example `^M`.
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or_default()
    }
}

/// The distance between tab stops, in cells: from 1 to [`TabWidth::MAX`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TabWidth(NonZeroU32);

impl TabWidth {
    /// The widest tab width; wider values are clamped to it.
    pub const MAX: u32 = 256;

    /// A tab width of `cells`, clamped to 1 to [`TabWidth::MAX`].
    pub fn new(cells: u32) -> Self {
        Self(NonZeroU32::new(cells.clamp(1, Self::MAX)).unwrap_or(NonZeroU32::MIN))
    }

    /// The width in cells.
    pub fn get(self) -> u32 {
        self.0.get()
    }

    /// The column of the first tab stop after `column`.
    pub(crate) fn next_stop(self, column: usize) -> usize {
        let width = self.0.get() as usize;
        (column / width).saturating_add(1).saturating_mul(width)
    }
}

impl Default for TabWidth {
    /// 8 cells, as in Emacs.
    fn default() -> Self {
        Self::new(8)
    }
}

/// The grapheme clusters of a text and their byte offsets. The text must
/// start on a cluster boundary.
///
/// An ASCII character followed by another one is a cluster of its own
/// (except `\r\n`): no ASCII character extends or prepends to a cluster.
/// Only clusters that touch non-ASCII text go through full segmentation.
pub(crate) struct Clusters<'a> {
    text: &'a str,
    position: usize,
}

impl<'a> Clusters<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self { text, position: 0 }
    }
}

impl<'a> Iterator for Clusters<'a> {
    /// The offset in the text, the cluster, its kind and its cells.
    type Item = (usize, &'a str, CellKind, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let (text, start) = (self.text, self.position);
        let bytes = text.as_bytes();
        let byte = *bytes.get(start)?;
        let next = bytes.get(start + 1).copied();
        let (end, kind, cells) = if byte == b'\r' && next == Some(b'\n') {
            (start + 2, CellKind::LineBreak, 0)
        } else if byte.is_ascii() && next.is_none_or(|next| next.is_ascii()) {
            let (kind, cells) = classify_ascii(byte);
            (start + 1, kind, cells)
        } else {
            let mut cursor = GraphemeCursor::new(start, text.len(), true);
            let end = match cursor.next_boundary(text, 0) {
                Ok(Some(end)) => end,
                _ => text[start..]
                    .char_indices()
                    .nth(1)
                    .map_or(text.len(), |(index, _)| start + index),
            };
            let (kind, cells) = classify(&text[start..end]);
            (end, kind, cells)
        };
        self.position = end;
        Some((start, &text[start..end], kind, cells))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(cluster: &str) -> usize {
        classify(cluster).1
    }

    #[test]
    fn widths_follow_east_asian_width() {
        assert_eq!(cells("a"), 1);
        assert_eq!(cells("ｱ"), 1, "half-width katakana");
        assert_eq!(cells("日"), 2);
        assert_eq!(cells("Ａ"), 2, "full-width Latin");
        assert_eq!(cells("○"), 1, "ambiguous width takes 1 cell");
        assert_eq!(cells("e\u{301}"), 1, "combining mark");
        assert_eq!(cells("👍"), 2);
        assert_eq!(cells("👍🏽"), 2, "modifier sequence");
        assert_eq!(cells("👨\u{200d}👩\u{200d}👧"), 2, "ZWJ sequence");
        assert_eq!(cells("❤\u{fe0f}"), 2, "emoji presentation");
        assert_eq!(cells("🇯🇵"), 2, "flag");
    }

    #[test]
    fn invisible_clusters_take_one_cell() {
        assert_eq!(cells("\u{200b}"), 1, "ZWSP");
        assert_eq!(cells("\u{202e}"), 1, "bidi override");
        assert_eq!(cells("\u{301}"), 1, "lone combining mark");
    }

    #[test]
    fn controls_are_escaped() {
        assert_eq!(classify("\r"), (CellKind::Control('\r'), 2));
        assert_eq!(classify("\0"), (CellKind::Control('\0'), 2));
        assert_eq!(classify("\u{7f}"), (CellKind::Control('\u{7f}'), 2));
        assert_eq!(classify("\u{85}"), (CellKind::Control('\u{85}'), 4));
        assert_eq!(classify("\t"), (CellKind::Tab, 0));
        assert_eq!(classify("\r\n"), (CellKind::LineBreak, 0));
        let escape = |c| control_escape(c).map(|escape| escape.as_str().to_owned());
        assert_eq!(escape('\0').as_deref(), Some("^@"));
        assert_eq!(escape('\r').as_deref(), Some("^M"));
        assert_eq!(escape('\u{1b}').as_deref(), Some("^["));
        assert_eq!(escape('\u{7f}').as_deref(), Some("^?"));
        assert_eq!(escape('\u{80}').as_deref(), Some("\\200"));
        assert_eq!(escape('\u{9f}').as_deref(), Some("\\237"));
        assert_eq!(escape('a'), None);
        for code in (0..0x20).chain(0x7f..0xa0) {
            let c = char::from_u32(code).unwrap_or_default();
            let width = classify(c.encode_utf8(&mut [0; 4])).1;
            if c != '\t' && c != '\n' {
                assert_eq!(escape(c).map(|text| text.len()), Some(width));
            }
        }
    }

    #[test]
    fn fast_path_matches_segmentation() {
        use unicode_segmentation::UnicodeSegmentation;
        let text: String = (0u8..0x80).map(char::from).collect::<String>()
            + "a\r\nb\r\r\ne\u{301}x日本\u{301}\r\u{301}🇯🇵🇯🇵a\u{200d}👍👨\u{200d}👩\n";
        let fast: Vec<_> = Clusters::new(&text).collect();
        let slow: Vec<_> = text
            .grapheme_indices(true)
            .map(|(offset, cluster)| {
                let (kind, cells) = classify(cluster);
                (offset, cluster, kind, cells)
            })
            .collect();
        assert_eq!(fast, slow);
    }

    #[test]
    fn tab_widths_are_clamped() {
        assert_eq!(TabWidth::new(0).get(), 1);
        assert_eq!(TabWidth::new(u32::MAX).get(), TabWidth::MAX);
        assert_eq!(TabWidth::default().get(), 8);
        let four = TabWidth::new(4);
        assert_eq!(four.next_stop(0), 4);
        assert_eq!(four.next_stop(3), 4);
        assert_eq!(four.next_stop(4), 8);
        assert_eq!(four.next_stop(usize::MAX), usize::MAX);
    }
}
