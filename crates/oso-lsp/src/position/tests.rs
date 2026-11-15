//! Positions against a naive scan of the text in both encodings, each clamp with a test, the
//! encoding chosen, and a fuzz target of positions anywhere, far past the text included.
//!
//! Must not depend on: `framing`, `json` or `uri`; the scan here is this crate's own.
//!
//! The fuzz target runs in `just ci` with proptest's default cases. For a long run:
//! `PROPTEST_CASES=200000 cargo test -p oso-lsp --release position::tests::fuzz`.

use oso_core::rope::Rope;
use proptest::prelude::*;

use super::{Encoding, Position, from_offset, to_offset, to_range};

const BOTH: [Encoding; 2] = [Encoding::Utf8, Encoding::Utf16];

fn width(character: char, encoding: Encoding) -> usize {
    match encoding {
        Encoding::Utf8 => character.len_utf8(),
        Encoding::Utf16 => character.len_utf16(),
    }
}

/// The start of each line, by a scan for `\n`.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(index, _)| index + 1));
    starts
}

/// Where line `row` ends, before `\n` or `\r\n`.
fn line_end(text: &str, start: usize) -> usize {
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |index| start + index);
    if end > start && text.as_bytes()[end - 1] == b'\r' && end < text.len() {
        end - 1
    } else {
        end
    }
}

fn naive_to_offset(text: &str, position: Position, encoding: Encoding) -> usize {
    let starts = line_starts(text);
    let Some(&start) = starts.get(position.line as usize) else {
        return text.len();
    };
    let end = line_end(text, start);
    let mut counted = 0;
    for (index, character) in text[start..end].char_indices() {
        let next = counted + width(character, encoding);
        if (position.character as usize) < next {
            return start + index;
        }
        counted = next;
    }
    end
}

fn naive_from_offset(text: &str, offset: usize, encoding: Encoding) -> Position {
    let offset = text.floor_char_boundary(offset);
    let before = &text[..offset];
    let start = before.rfind('\n').map_or(0, |index| index + 1);
    let character: usize = text[start..offset]
        .chars()
        .map(|c| width(c, encoding))
        .sum();
    Position::new(before.matches('\n').count() as u32, character as u32)
}

#[test]
fn the_encoding_is_utf8_only_when_chosen() {
    assert_eq!(Encoding::OFFERED, ["utf-8", "utf-16"]);
    assert_eq!(Encoding::chosen(None), Some(Encoding::Utf16));
    assert_eq!(Encoding::chosen(Some("utf-16")), Some(Encoding::Utf16));
    assert_eq!(Encoding::chosen(Some("utf-8")), Some(Encoding::Utf8));
    assert_eq!(Encoding::chosen(Some("utf-32")), None);
    assert_eq!(Encoding::chosen(Some("UTF-8")), None);
}

#[test]
fn columns_count_in_the_encoding() {
    let rope = Rope::from("a😀b\n日x");
    assert_eq!(to_offset(&rope, Position::new(0, 3), Encoding::Utf16), 5);
    assert_eq!(to_offset(&rope, Position::new(0, 5), Encoding::Utf8), 5);
    assert_eq!(to_offset(&rope, Position::new(1, 1), Encoding::Utf16), 10);
    assert_eq!(to_offset(&rope, Position::new(1, 3), Encoding::Utf8), 10);
    assert_eq!(from_offset(&rope, 5, Encoding::Utf16), Position::new(0, 3));
    assert_eq!(from_offset(&rope, 5, Encoding::Utf8), Position::new(0, 5));
    assert_eq!(from_offset(&rope, 10, Encoding::Utf16), Position::new(1, 1));
}

#[test]
fn positions_past_the_text_clamp() {
    let rope = Rope::from("ab\r\ncd\n");
    for encoding in BOTH {
        // A line past the end: the end.
        assert_eq!(to_offset(&rope, Position::new(3, 0), encoding), 7);
        assert_eq!(
            to_offset(&rope, Position::new(u32::MAX, u32::MAX), encoding),
            7
        );
        // A column past its line: the line's end, before `\r\n`.
        assert_eq!(to_offset(&rope, Position::new(0, 2), encoding), 2);
        assert_eq!(to_offset(&rope, Position::new(0, 3), encoding), 2);
        assert_eq!(to_offset(&rope, Position::new(0, u32::MAX), encoding), 2);
        assert_eq!(to_offset(&rope, Position::new(1, 9), encoding), 6);
        // The empty last line.
        assert_eq!(to_offset(&rope, Position::new(2, 0), encoding), 7);
        assert_eq!(to_offset(&rope, Position::new(2, 5), encoding), 7);
    }
    assert_eq!(
        from_offset(&rope, usize::MAX, Encoding::Utf16),
        Position::new(2, 0)
    );
}

#[test]
fn columns_inside_a_character_go_to_its_start() {
    let rope = Rope::from("x😀日");
    // Half a surrogate pair.
    assert_eq!(to_offset(&rope, Position::new(0, 2), Encoding::Utf16), 1);
    // Inside UTF-8 bytes.
    for column in 1..5 {
        assert_eq!(
            to_offset(&rope, Position::new(0, column), Encoding::Utf8),
            1
        );
    }
    assert_eq!(to_offset(&rope, Position::new(0, 6), Encoding::Utf8), 5);
    // An offset inside a character reads as its start.
    assert_eq!(from_offset(&rope, 3, Encoding::Utf16), Position::new(0, 1));
}

#[test]
fn a_lone_carriage_return_is_a_character() {
    let rope = Rope::from("a\rb\nc");
    assert_eq!(to_offset(&rope, Position::new(0, 2), Encoding::Utf16), 2);
    assert_eq!(to_offset(&rope, Position::new(1, 0), Encoding::Utf16), 4);
    assert_eq!(from_offset(&rope, 2, Encoding::Utf8), Position::new(0, 2));
}

#[test]
fn ranges_are_ordered_and_clamped() {
    let rope = Rope::from("abc\ndef");
    let (a, b) = (Position::new(1, 1), Position::new(0, 9));
    assert_eq!(to_range(&rope, a, b, Encoding::Utf16), 3..5);
    assert_eq!(to_range(&rope, b, a, Encoding::Utf16), 3..5);
}

#[test]
fn long_lines_convert_across_chunks() {
    let text = "a😀日".repeat(20_000);
    let rope = Rope::from(text.as_str());
    for (offset, _) in text.char_indices().step_by(997) {
        for encoding in BOTH {
            let position = from_offset(&rope, offset, encoding);
            assert_eq!(position, naive_from_offset(&text, offset, encoding));
            assert_eq!(to_offset(&rope, position, encoding), offset);
        }
    }
}

/// Text from pieces with every line ending, wide, astral and combining characters.
///
/// Up to a few hundred bytes, or now and then a few thousand, so that line breaks and astral
/// characters fall across the rope's chunks of about 1 KiB.
fn text() -> impl Strategy<Value = String> {
    let atoms = [
        "a",
        " ",
        "\t",
        "\n",
        "\r\n",
        "\r",
        "é",
        "日本",
        "😀",
        "👨\u{200d}👩",
        "e\u{301}",
        "\u{10ffff}",
    ];
    let pieces = prop::sample::select(atoms.to_vec());
    prop_oneof![
        3 => prop::collection::vec(pieces.clone(), 0..120),
        1 => prop::collection::vec(pieces, 400..1500),
    ]
    .prop_map(|atoms| atoms.concat())
}

fn encoding() -> impl Strategy<Value = Encoding> {
    prop::sample::select(BOTH.to_vec())
}

proptest! {
    #[test]
    fn conversions_match_a_naive_scan(text in text(), offset in 0usize..6000, line in 0u32..600, character in 0u32..60, encoding in encoding()) {
        let rope = Rope::from(text.as_str());
        let position = Position::new(line, character);
        prop_assert_eq!(to_offset(&rope, position, encoding), naive_to_offset(&text, position, encoding));
        prop_assert_eq!(from_offset(&rope, offset, encoding), naive_from_offset(&text, offset, encoding));
    }

    #[test]
    fn offsets_round_trip(text in text(), offset in 0usize..6000, encoding in encoding()) {
        let rope = Rope::from(text.as_str());
        let offset = text.floor_char_boundary(offset);
        // Between `\r` and `\n` is past the line's end, which clamps before the `\r`.
        let inside_crlf = text[..offset].ends_with('\r') && text[offset..].starts_with('\n');
        prop_assume!(!inside_crlf);
        prop_assert_eq!(to_offset(&rope, from_offset(&rope, offset, encoding), encoding), offset);
    }

    /// Positions anywhere: the offset is always in the text, on a character's boundary, and
    /// never inside a line break.
    #[test]
    fn fuzz(text in text(), line in any::<u32>(), character in any::<u32>(), small in (0u32..600, 0u32..30), encoding in encoding()) {
        let rope = Rope::from(text.as_str());
        for position in [Position::new(line, character), Position::new(small.0, small.1), Position::new(small.0, character)] {
            let offset = to_offset(&rope, position, encoding);
            prop_assert!(offset <= text.len());
            prop_assert!(text.is_char_boundary(offset));
            prop_assert!(!(text[..offset].ends_with('\r') && text[offset..].starts_with('\n')));
        }
    }
}
