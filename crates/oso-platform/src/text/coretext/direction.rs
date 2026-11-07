//! Splitting a row by the direction of its scripts, for the CoreText text
//! system (ADR 0066, ADR 0068).
//!
//! Must not depend on: `control`, `oso-core`, the GPU or CoreText.
//!
//! A row is split between graphemes. Right-to-left scripts are shaped at
//! embedding level 1, since their marks and joining need it, and nothing there
//! has a mirrored form. Other graphemes are at level 0, but for a space or
//! punctuation with no mirrored form, which stays in the run it is in.
//!
//! The runs of one direction are joined into one line, each after a ZWNJ, which
//! keeps letters from joining and ligatures from forming across it, so a row is
//! at most two lines however often it changes direction. The ZWNJ is oso's own
//! and maps to no byte of the row, so it draws nothing.

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

use super::super::ShapedGlyph;

/// What a UTF-16 offset of a separator between runs maps to: no byte of a row.
pub(super) const SEPARATOR: u32 = u32::MAX;

/// Maps the UTF-16 offsets of `text`, which starts `start` bytes into the row,
/// to its byte offsets in the row.
pub(super) fn push_bytes(bytes: &mut Vec<u32>, text: &str, start: usize) {
    for (byte, c) in text.char_indices() {
        let byte = u32::try_from(start + byte).unwrap_or(SEPARATOR - 1);
        bytes.extend(std::iter::repeat_n(byte, c.len_utf16()));
    }
}

/// A run of graphemes of one direction in a row.
#[derive(Debug, Clone, Copy)]
pub(super) struct DirectionRun {
    pub(super) start: usize,
    pub(super) right_to_left: bool,
    /// The leftmost pen of its glyphs and their total advance, as shaped.
    x: f32,
    advance: f32,
}

/// Fills `runs` with the runs of one direction of `text`, which has text.
pub(super) fn split(text: &str, runs: &mut Vec<DirectionRun>) {
    runs.clear();
    let mut current = false;
    for (byte, grapheme) in text.grapheme_indices(true) {
        let right_to_left = grapheme_direction(grapheme).unwrap_or(current);
        if right_to_left != current || byte == 0 {
            runs.push(DirectionRun {
                start: byte,
                right_to_left,
                x: f32::INFINITY,
                advance: 0.0,
            });
            current = right_to_left;
        }
    }
}

/// Puts the runs of `text` in one direction into `line`, each after a ZWNJ but
/// the first, and maps its UTF-16 offsets to the row's bytes in `bytes`.
pub(super) fn join(
    text: &str,
    runs: &[DirectionRun],
    right_to_left: bool,
    line: &mut String,
    bytes: &mut Vec<u32>,
) {
    line.clear();
    bytes.clear();
    let mut ends = runs.iter().map(|run| run.start).skip(1);
    for run in runs {
        let end = ends.next().unwrap_or(text.len());
        if run.right_to_left != right_to_left {
            continue;
        }
        if !line.is_empty() {
            line.push('\u{200c}');
            bytes.push(SEPARATOR);
        }
        push_bytes(bytes, &text[run.start..end], run.start);
        line.push_str(&text[run.start..end]);
    }
}

/// Moves the glyphs of each of `runs` so that the runs follow one another in
/// the order of the text, as if each had been shaped on its own.
pub(super) fn line_up(runs: &mut [DirectionRun], out: &mut [ShapedGlyph]) {
    let run_of = |runs: &[DirectionRun], cluster: u32| {
        runs.partition_point(|run| run.start as u32 <= cluster)
            .saturating_sub(1)
    };
    for glyph in out.iter() {
        let index = run_of(runs, glyph.cluster);
        let run = &mut runs[index];
        run.x = run.x.min(glyph.x);
        run.advance += glyph.advance;
    }
    let mut pen = 0.0;
    for run in runs.iter_mut() {
        let x = if run.x.is_finite() { run.x } else { 0.0 };
        run.x = pen - x;
        pen += run.advance;
    }
    for glyph in out.iter_mut() {
        glyph.x += runs[run_of(runs, glyph.cluster)].x;
    }
}

/// Whether `c` is in a block of a right-to-left script. Its letters need
/// embedding level 1 to shape as they should. None of these characters has a
/// mirrored form (`Bidi_Mirrored` in Unicode 17), so that level cannot show one.
pub(super) fn is_right_to_left(c: char) -> bool {
    matches!(
        u32::from(c),
        0x590..=0x8FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF
    )
}

/// Whether `c` extends the grapheme of a letter before it: a mark or a joiner,
/// by the same table as the display model's. None of these has a mirrored form.
fn extends_a_letter(c: char) -> bool {
    let mut bytes = [b'a'; 5];
    let len = 1 + c.encode_utf8(&mut bytes[1..]).len();
    let Ok(text) = std::str::from_utf8(&bytes[..len]) else {
        return false;
    };
    GraphemeCursor::new(1, len, true).is_boundary(text, 0) == Ok(false)
}

/// The direction of a grapheme, right to left or not: `None` for one that stays
/// in the run it is in, as a space or ASCII punctuation with no mirrored form
/// does. Right to left needs all of it to be: a prepended character can come
/// before a character with a mirrored form, which then keeps level 0.
pub(super) fn grapheme_direction(grapheme: &str) -> Option<bool> {
    let mut chars = grapheme.chars();
    let first = chars.next()?;
    if chars.as_str().is_empty() {
        let neutral = first == ' '
            || first.is_ascii_punctuation()
                && !matches!(first, '(' | ')' | '<' | '>' | '[' | ']' | '{' | '}');
        return if neutral {
            None
        } else {
            Some(is_right_to_left(first))
        };
    }
    let mut right_to_left = false;
    for c in grapheme.chars() {
        if is_right_to_left(c) {
            right_to_left = true;
        } else if !extends_a_letter(c) {
            return Some(false);
        }
    }
    Some(right_to_left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_is_split_where_its_direction_changes() {
        let mut runs = Vec::new();
        let text = "a \u{5d0}, \u{5d1}(b) \u{5d2}";
        split(text, &mut runs);
        let starts: Vec<_> = runs.iter().map(|r| (r.start, r.right_to_left)).collect();
        // The space and comma stay in the run they are in, the bracket does not.
        assert_eq!(starts, [(0, false), (2, true), (8, false), (12, true)]);
        let (mut line, mut bytes) = (String::new(), Vec::new());
        join(text, &runs, true, &mut line, &mut bytes);
        assert_eq!(line, "\u{5d0}, \u{5d1}\u{200c}\u{5d2}");
        assert_eq!(bytes, [2, 4, 5, 6, SEPARATOR, 12]);
        join(text, &runs, false, &mut line, &mut bytes);
        assert_eq!(line, "a \u{200c}(b) ");
        assert_eq!(bytes, [0, 1, SEPARATOR, 8, 9, 10, 11]);
    }

    #[test]
    fn runs_line_up_in_the_order_of_the_text() {
        let glyph = |cluster: u32, x: f32| ShapedGlyph {
            key: super::super::super::GlyphKey {
                font: super::super::super::FontId(0),
                glyph: super::super::super::GlyphId(0),
            },
            cluster,
            x,
            y: 0.0,
            advance: 8.0,
        };
        let mut runs = Vec::new();
        split("ab\u{5d0}\u{5d1}c", &mut runs);
        // Each direction's line starts at 0, the right-to-left one reversed.
        let mut out = [
            glyph(0, 0.0),
            glyph(1, 8.0),
            glyph(6, 24.0),
            glyph(4, 0.0),
            glyph(2, 8.0),
        ];
        line_up(&mut runs, &mut out);
        let pens: Vec<_> = out.iter().map(|g| (g.cluster, g.x)).collect();
        assert_eq!(pens, [(0, 0.0), (1, 8.0), (6, 32.0), (4, 16.0), (2, 24.0)]);
    }
    #[test]
    fn a_prepended_character_takes_the_direction_of_what_follows() {
        assert_eq!(grapheme_direction("\u{6dd}\u{661}"), Some(true));
        assert_eq!(grapheme_direction("\u{600}\u{5d0}"), Some(true));
        assert_eq!(grapheme_direction("\u{600}("), Some(false));
        assert_eq!(grapheme_direction("\u{600}a"), Some(false));
    }

    #[test]
    fn a_grapheme_of_one_character_has_its_direction_unless_it_stays_in_the_run() {
        assert_eq!(grapheme_direction("a"), Some(false));
        assert_eq!(grapheme_direction("\u{65e5}"), Some(false));
        assert_eq!(grapheme_direction("("), Some(false));
        assert_eq!(grapheme_direction("\u{5d0}"), Some(true));
        assert_eq!(grapheme_direction(" "), None);
        assert_eq!(grapheme_direction(","), None);
    }

    /// Every character with `Bidi_Mirrored=Y` in `UnicodeData.txt` of Unicode 17, as
    /// inclusive ranges.
    const MIRRORED: [(u32, u32); 114] = [
        (0x28, 0x29),
        (0x3C, 0x3C),
        (0x3E, 0x3E),
        (0x5B, 0x5B),
        (0x5D, 0x5D),
        (0x7B, 0x7B),
        (0x7D, 0x7D),
        (0xAB, 0xAB),
        (0xBB, 0xBB),
        (0xF3A, 0xF3D),
        (0x169B, 0x169C),
        (0x2039, 0x203A),
        (0x2045, 0x2046),
        (0x207D, 0x207E),
        (0x208D, 0x208E),
        (0x2140, 0x2140),
        (0x2201, 0x2204),
        (0x2208, 0x220D),
        (0x2211, 0x2211),
        (0x2215, 0x2216),
        (0x221A, 0x221D),
        (0x221F, 0x2222),
        (0x2224, 0x2224),
        (0x2226, 0x2226),
        (0x222B, 0x2233),
        (0x2239, 0x2239),
        (0x223B, 0x224C),
        (0x2252, 0x2255),
        (0x225F, 0x2260),
        (0x2262, 0x2262),
        (0x2264, 0x226B),
        (0x226D, 0x228C),
        (0x228F, 0x2292),
        (0x2298, 0x2298),
        (0x22A2, 0x22A3),
        (0x22A6, 0x22B8),
        (0x22BE, 0x22BF),
        (0x22C9, 0x22CD),
        (0x22D0, 0x22D1),
        (0x22D6, 0x22ED),
        (0x22F0, 0x22FF),
        (0x2308, 0x230B),
        (0x2320, 0x2321),
        (0x2329, 0x232A),
        (0x2768, 0x2775),
        (0x27C0, 0x27C0),
        (0x27C3, 0x27C6),
        (0x27C8, 0x27C9),
        (0x27CB, 0x27CD),
        (0x27D3, 0x27D6),
        (0x27DC, 0x27DE),
        (0x27E2, 0x27EF),
        (0x2983, 0x2998),
        (0x299B, 0x29A0),
        (0x29A2, 0x29AF),
        (0x29B8, 0x29B8),
        (0x29C0, 0x29C5),
        (0x29C9, 0x29C9),
        (0x29CE, 0x29D2),
        (0x29D4, 0x29D5),
        (0x29D8, 0x29DC),
        (0x29E1, 0x29E1),
        (0x29E3, 0x29E5),
        (0x29E8, 0x29E9),
        (0x29F4, 0x29F9),
        (0x29FC, 0x29FD),
        (0x2A0A, 0x2A1C),
        (0x2A1E, 0x2A21),
        (0x2A24, 0x2A24),
        (0x2A26, 0x2A26),
        (0x2A29, 0x2A29),
        (0x2A2B, 0x2A2E),
        (0x2A34, 0x2A35),
        (0x2A3C, 0x2A3E),
        (0x2A57, 0x2A58),
        (0x2A64, 0x2A65),
        (0x2A6A, 0x2A6D),
        (0x2A6F, 0x2A70),
        (0x2A73, 0x2A74),
        (0x2A79, 0x2AA3),
        (0x2AA6, 0x2AAD),
        (0x2AAF, 0x2AD6),
        (0x2ADC, 0x2ADC),
        (0x2ADE, 0x2ADE),
        (0x2AE2, 0x2AE6),
        (0x2AEC, 0x2AEE),
        (0x2AF3, 0x2AF3),
        (0x2AF7, 0x2AFB),
        (0x2AFD, 0x2AFD),
        (0x2BFE, 0x2BFE),
        (0x2E02, 0x2E05),
        (0x2E09, 0x2E0A),
        (0x2E0C, 0x2E0D),
        (0x2E1C, 0x2E1D),
        (0x2E20, 0x2E29),
        (0x2E55, 0x2E5C),
        (0x3008, 0x3011),
        (0x3014, 0x301B),
        (0xFE59, 0xFE5E),
        (0xFE64, 0xFE65),
        (0xFF08, 0xFF09),
        (0xFF1C, 0xFF1C),
        (0xFF1E, 0xFF1E),
        (0xFF3B, 0xFF3B),
        (0xFF3D, 0xFF3D),
        (0xFF5B, 0xFF5B),
        (0xFF5D, 0xFF5D),
        (0xFF5F, 0xFF60),
        (0xFF62, 0xFF63),
        (0x1D6DB, 0x1D6DB),
        (0x1D715, 0x1D715),
        (0x1D74F, 0x1D74F),
        (0x1D789, 0x1D789),
        (0x1D7C3, 0x1D7C3),
    ];

    #[test]
    fn no_character_with_a_mirrored_form_can_reach_level_1() {
        assert_eq!(MIRRORED.iter().map(|(a, b)| b - a + 1).sum::<u32>(), 554);
        let mirrored = MIRRORED
            .iter()
            .flat_map(|&(a, b)| a..=b)
            .filter_map(char::from_u32);
        for c in mirrored {
            assert!(!is_right_to_left(c), "{c:?}");
            assert!(!extends_a_letter(c), "{c:?}");
            assert_eq!(
                grapheme_direction(c.encode_utf8(&mut [0; 4])),
                Some(false),
                "{c:?}"
            );
        }
    }

    #[test]
    fn the_tables_are_of_the_unicode_version_checked() {
        // On a new version, check `MIRRORED` and the ranges of `is_right_to_left` again.
        assert_eq!(unicode_segmentation::UNICODE_VERSION, (17, 0, 0));
    }

    #[test]
    fn only_characters_with_no_mirrored_form_can_share_a_right_to_left_run() {
        // The closest characters with a mirrored form to the ranges, and ones
        // that look as if they had one.
        for c in [
            '(',
            ')',
            '<',
            '>',
            '[',
            ']',
            '{',
            '}',
            '\u{ab}',
            '\u{bb}',
            '\u{2039}',
            '\u{2264}',
            '\u{fe59}',
            '\u{fe5e}',
            '\u{fe64}',
            '\u{fe65}',
            '\u{ff08}',
            '\u{ff09}',
            '\u{1d6db}',
            '\u{1d7c3}',
        ] {
            assert!(!is_right_to_left(c), "{c:?}");
            assert_eq!(grapheme_direction(&c.to_string()), Some(false), "{c:?}");
        }
        assert!(is_right_to_left('\u{fd3e}') && is_right_to_left('\u{fd3f}'));
        // The edges of the ranges.
        for c in [
            '\u{58f}',
            '\u{900}',
            '\u{fb1c}',
            '\u{fe6f}',
            '\u{ff00}',
            '\u{1e7ff}',
            '\u{1f000}',
        ] {
            assert!(!is_right_to_left(c), "{c:?}");
        }
        for c in [
            '\u{590}',
            '\u{8ff}',
            '\u{fb1d}',
            '\u{fe70}',
            '\u{10800}',
            '\u{1efff}',
        ] {
            assert!(is_right_to_left(c), "{c:?}");
        }
    }
}
