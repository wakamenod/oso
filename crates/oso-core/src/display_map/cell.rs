//! The cell grid: how many cells each grapheme cluster takes (ADR 0005,
//! ADR 0035).
//!
//! - A cluster takes the East Asian Width of its text: 1 cell for half
//!   width, 2 for full width, with emoji sequences counted as one 2-cell
//!   glyph. Ambiguous-width characters take 1 cell.
//! - Every other cluster takes at least 1 cell and at most 2.
//! - Control characters are shown as escapes, as in Emacs: caret notation in
//!   2 cells for C0 and DEL (`^@`, `^M`, `^?`), and an octal escape in 4
//!   cells for C1 (`\200`). A lone `\r` is a control character.
//! - Bidi controls and invisible characters are shown as their code point in
//!   a box (`U+202E`), never invisibly (`ui.md`, Trojan Source). The box
//!   takes one cell per character of its label. Such a character is split
//!   out of the cluster it would extend, except where it is part of an emoji
//!   or a script (see `is_hidden_extend`), so `a` followed by tag
//!   characters or a ZWNJ shows every hidden code point.
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
    /// A control character, drawn as its [`escape`].
    Control(char),
    /// A bidi control or invisible character, drawn as its [`escape`] in a
    /// box.
    Invisible(char),
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
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_control() {
            let cells = if u32::from(c) < 0x80 { 2 } else { 4 };
            return (CellKind::Control(c), cells);
        }
        if is_invisible(c) {
            let cells = escape(c).map_or(1, |escape| escape.len);
            return (CellKind::Invisible(c), cells);
        }
    }
    (CellKind::Text, cluster.width().clamp(1, 2))
}

/// Whether `c` is drawn as a boxed code point: a bidi control, a
/// default-ignorable code point (Unicode `DerivedCoreProperties.txt`), or a
/// line or paragraph separator. The Hangul jamo fillers U+115F and U+1160
/// are left out, because they compose syllables.
pub fn is_invisible(c: char) -> bool {
    matches!(
        u32::from(c),
        0x00ad
            | 0x034f
            | 0x061c
            | 0x17b4..=0x17b5
            | 0x180b..=0x180f
            | 0x200b..=0x200f
            | 0x2028..=0x202e
            | 0x2060..=0x206f
            | 0x3164
            | 0xfe00..=0xfe0f
            | 0xfeff
            | 0xffa0
            | 0xfff0..=0xfffb
            | 0x1bca0..=0x1bca3
            | 0x1d173..=0x1d17a
            | 0xe0000..=0xe0fff
    )
}

/// Whether the invisible character `c`, extending a cluster that starts with
/// `base`, would be hidden in it. Allowed are ZWJ, ZWNJ and variation
/// selectors after non-ASCII text (emoji sequences, scripts that need them),
/// VS15 and VS16 after a keycap base (`1️⃣`), and tag characters in a
/// U+1F3F4 flag sequence. Everything else is split out and boxed.
pub(crate) fn is_hidden_extend(c: char, base: char) -> bool {
    if !is_invisible(c) {
        return false;
    }
    let allowed = match u32::from(c) {
        0xe0020..=0xe007f => base == '\u{1f3f4}',
        0xfe0e | 0xfe0f => !base.is_ascii() || matches!(base, '0'..='9' | '#' | '*'),
        _ => !base.is_ascii(),
    };
    !allowed
}

/// The length of the first display cluster in a grapheme cluster: the whole
/// cluster, or the part before an invisible character it would hide, or the
/// invisible character itself.
pub(crate) fn first_display_cluster(cluster: &str) -> usize {
    let mut chars = cluster.char_indices();
    let Some((_, base)) = chars.next() else {
        return 0;
    };
    if is_invisible(base) {
        return base.len_utf8();
    }
    chars
        .find(|&(_, c)| is_hidden_extend(c, base))
        .map_or(cluster.len(), |(index, _)| index)
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

/// The text a control or invisible character is drawn as: `^@` to `^_`
/// and `^?` for C0 and DEL, `\200` to `\237` for C1, and the code point
/// (`U+202E`) for an [invisible](is_invisible) character. `None` for other
/// characters.
pub fn escape(c: char) -> Option<Escape> {
    let code = u32::from(c);
    let mut bytes = [0; 8];
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
        _ if is_invisible(c) => {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            let digits = if code > 0xffff { 5 } else { 4 };
            bytes[0] = b'U';
            bytes[1] = b'+';
            for index in 0..digits {
                let shift = 4 * (digits - 1 - index);
                bytes[2 + index] = HEX[((code >> shift) & 0xf) as usize];
            }
            2 + digits
        }
        _ => return None,
    };
    Some(Escape { bytes, len })
}

/// The escape text of a control or invisible character. See [`escape`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Escape {
    bytes: [u8; 8],
    len: usize,
}

impl Escape {
    /// The escape as text, for example `^M` or `U+202E`.
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
    pub fn next_stop(self, column: usize) -> usize {
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

/// The display clusters of a text and their byte offsets: grapheme clusters,
/// with hidden invisible characters split out (see
/// [`first_display_cluster`]). The text must start on a cluster boundary.
///
/// An ASCII character followed by another one is a cluster of its own
/// (except `\r\n`): no ASCII character extends or prepends to a cluster.
/// Only clusters that touch non-ASCII text go through full segmentation,
/// with one cursor for the whole text, so that runs of regional indicators
/// (flags) are not rescanned for every cluster.
pub struct Clusters<'a> {
    text: &'a str,
    position: usize,
    cursor: GraphemeCursor,
}

impl<'a> Clusters<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            position: 0,
            cursor: GraphemeCursor::new(0, text.len(), true),
        }
    }

    /// The end of the grapheme cluster that starts (or continues) at
    /// `start`.
    fn cluster_end(&mut self, start: usize) -> usize {
        if self.cursor.cur_cursor() != start {
            self.cursor.set_cursor(start);
        }
        match self.cursor.next_boundary(self.text, 0) {
            Ok(Some(end)) => end,
            _ => {
                let next_char = self.text[start..].chars().next().map_or(1, char::len_utf8);
                let end = start + next_char;
                self.cursor.set_cursor(end);
                end
            }
        }
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
            let cluster_end = self.cluster_end(start);
            let end = start + first_display_cluster(&text[start..cluster_end]);
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
    fn invisible_characters_are_boxed_code_points() {
        assert_eq!(classify("\u{202e}"), (CellKind::Invisible('\u{202e}'), 6));
        assert_eq!(classify("\u{200b}"), (CellKind::Invisible('\u{200b}'), 6));
        assert_eq!(classify("\u{feff}"), (CellKind::Invisible('\u{feff}'), 6));
        assert_eq!(classify("\u{2028}"), (CellKind::Invisible('\u{2028}'), 6));
        assert_eq!(classify("\u{e0041}"), (CellKind::Invisible('\u{e0041}'), 7));
        assert_eq!(cells("\u{301}"), 1, "a lone combining mark is text");
        let label = |c| escape(c).map(|escape| escape.as_str().to_owned());
        assert_eq!(label('\u{202e}').as_deref(), Some("U+202E"));
        assert_eq!(label('\u{2066}').as_deref(), Some("U+2066"));
        assert_eq!(label('\u{e0041}').as_deref(), Some("U+E0041"));
    }

    fn display_clusters(text: &str) -> Vec<&str> {
        Clusters::new(text)
            .map(|(_, cluster, _, _)| cluster)
            .collect()
    }

    #[test]
    fn hidden_extenders_are_split_out() {
        assert_eq!(
            display_clusters("a\u{e0041}\u{e0042}b"),
            ["a", "\u{e0041}", "\u{e0042}", "b"],
            "ASCII smuggling with tag characters"
        );
        assert_eq!(
            display_clusters("isAdmin\u{200c}").last(),
            Some(&"\u{200c}")
        );
        assert_eq!(
            display_clusters("x\u{200d}\u{301}"),
            ["x", "\u{200d}", "\u{301}"]
        );
        // Legitimate sequences stay whole.
        for whole in [
            "👨\u{200d}👩\u{200d}👧",
            "❤\u{fe0f}",
            "1\u{fe0f}\u{20e3}",
            "\u{1f3f4}\u{e0067}\u{e0062}\u{e0065}\u{e006e}\u{e0067}\u{e007f}",
            "\u{6cc}\u{200c}",
            "葛\u{e0100}",
        ] {
            assert_eq!(display_clusters(whole), [whole]);
        }
    }

    #[test]
    fn flag_runs_pair_up() {
        let flags = "🇯🇵".repeat(300);
        assert!(
            display_clusters(&flags)
                .iter()
                .all(|cluster| *cluster == "🇯🇵")
        );
        let odd = "a🇯".to_owned() + &"🇯🇵".repeat(3);
        assert_eq!(display_clusters(&odd)[1..3], ["🇯🇯", "🇵🇯"]);
    }

    #[test]
    fn controls_are_escaped() {
        assert_eq!(classify("\r"), (CellKind::Control('\r'), 2));
        assert_eq!(classify("\0"), (CellKind::Control('\0'), 2));
        assert_eq!(classify("\u{7f}"), (CellKind::Control('\u{7f}'), 2));
        assert_eq!(classify("\u{85}"), (CellKind::Control('\u{85}'), 4));
        assert_eq!(classify("\t"), (CellKind::Tab, 0));
        assert_eq!(classify("\r\n"), (CellKind::LineBreak, 0));
        let escape = |c| escape(c).map(|escape| escape.as_str().to_owned());
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
            + "a\r\nb\r\r\ne\u{301}x日本\u{301}\r\u{301}🇯🇵🇯🇵b\u{300}👍👨\u{200d}👩\n";
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
