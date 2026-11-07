//! Splitting a row by the direction of its scripts, for the CoreText text
//! system (ADR 0066, ADR 0068).
//!
//! Must not depend on: `control`, `oso-core`, the GPU or CoreText.
//!
//! Each run of one direction would be a line of its own. Instead the runs of
//! a direction are joined into one line, each after a ZWNJ, which keeps letters
//! from joining and ligatures from forming across it, so a row is at most two
//! lines however often it changes direction.

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
pub(super) fn extends_a_letter(c: char) -> bool {
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
