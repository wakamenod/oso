//! A side's runs laid over a section's lines: each line's runs cut to its text, a BOM, CR LF
//! and invalid bytes counted as the section counts them, and the diff face kept where a side
//! has no runs yet.
//!
//! Must not depend on: threads or the syntax worker; these give highlights by hand.

use oso_core::display_map::TabWidth;
use oso_core::load::decode_all;
use oso_core::review::Side;
use oso_core::rope::Rope;
use oso_core::syntax::{Highlight, Highlights};

use super::project::{Projected, merged, project};
use super::section::Slot;

/// `text` decoded as a side is, with `marks` as (start, len, highlight) runs.
fn side(text: &[u8], marks: &[(usize, usize, Highlight)]) -> (Rope, Highlights) {
    let rope = decode_all(text, TabWidth::default()).text().clone();
    let mut highlights = Highlights::new(rope.len());
    for (start, len, highlight) in marks {
        highlights.replace(*start..start + len, [(*len, Some(*highlight))]);
    }
    (rope, highlights)
}

fn runs(projected: &Projected, number: u32) -> Vec<(u32, Option<Highlight>)> {
    projected
        .line(number)
        .map(<[_]>::to_vec)
        .unwrap_or_default()
}

#[test]
fn each_line_takes_its_own_runs_cut_to_its_text() {
    // "fn a() {}\nlet b = 1;\n": `fn` and `let` keywords, `1` a constant.
    let (rope, highlights) = side(
        b"fn a() {}\nlet b = 1;\n",
        &[
            (0, 2, Highlight::Keyword),
            (10, 3, Highlight::Keyword),
            (18, 1, Highlight::Constant),
        ],
    );
    let projected = project(&rope, false, &highlights, [1, 2], true);
    assert_eq!(
        runs(&projected, 1),
        [(2, Some(Highlight::Keyword)), (7, None)]
    );
    assert_eq!(
        runs(&projected, 2),
        [
            (3, Some(Highlight::Keyword)),
            (5, None),
            (1, Some(Highlight::Constant)),
            (1, None)
        ]
    );
    assert!(projected.line(3).is_none());
    assert!(projected.holds_all([1, 2].into_iter()));
    assert!(!projected.holds_all([1, 3].into_iter()));
    assert_eq!(projected.runs(), 6);
}

#[test]
fn a_bom_cr_lf_and_invalid_bytes_count_as_the_section_counts_them() {
    // The document's text has no BOM; the section's first line does.
    let bytes = b"\xEF\xBB\xBFfn\r\n\xFFx\r\n";
    let (rope, highlights) = side(bytes, &[(0, 2, Highlight::Keyword)]);
    let projected = project(&rope, true, &highlights, [1, 2], true);
    assert_eq!(
        runs(&projected, 1),
        [(3, None), (2, Some(Highlight::Keyword))],
        "the BOM plain, the CR left out"
    );
    // The invalid byte is a U+FFFD of three, then `x`.
    assert_eq!(runs(&projected, 2), [(4, None)]);
}

#[test]
fn lines_past_the_end_out_of_order_or_numbered_zero_get_nothing() {
    let (rope, highlights) = side(b"a\nb\n", &[]);
    let projected = project(&rope, false, &highlights, [0, 2, 1, 2, 9], true);
    assert!(projected.line(0).is_none());
    assert!(projected.line(1).is_none(), "out of order");
    assert_eq!(runs(&projected, 2), [(1, None)]);
    assert!(projected.line(9).is_none());
}

fn slot(side: Side, number: u32, at: usize, len: usize) -> Slot {
    Slot {
        side,
        number,
        at,
        len,
    }
}

/// A section's line `+fn a`, its own runs its diff face, its text after its sign.
fn painted(sides: [Option<&Projected>; 2]) -> Vec<(usize, Option<Highlight>)> {
    merged(
        &[(6, Some(Highlight::DiffAdded))],
        &[slot(Side::New, 1, 1, 4)],
        sides,
    )
}

#[test]
fn a_sections_runs_around_and_between_its_slots_stay_as_they_were() {
    let (rope, highlights) = side(
        b"fn\nlet\n",
        &[(0, 2, Highlight::Keyword), (3, 3, Highlight::Keyword)],
    );
    let done = project(&rope, false, &highlights, [1, 2], true);
    assert_eq!(
        runs(&done, 2),
        [(3, Some(Highlight::Keyword))],
        "never merged across lines"
    );
    // A header, `+fn`, a note, `+let` and a newline, each line's runs as a section has them.
    let base = [
        (3, Some(Highlight::DiffFileHeader)),
        (3, Some(Highlight::DiffAdded)),
        (2, Some(Highlight::Outdated)),
        (4, Some(Highlight::DiffAdded)),
        (1, None),
    ];
    let slots = [slot(Side::New, 1, 4, 2), slot(Side::New, 2, 9, 3)];
    assert_eq!(
        merged(&base, &slots, [None, Some(&done)]),
        [
            (3, Some(Highlight::DiffFileHeader)),
            (1, Some(Highlight::DiffAdded)),
            (2, Some(Highlight::Keyword)),
            (2, Some(Highlight::Outdated)),
            (1, Some(Highlight::DiffAdded)),
            (3, Some(Highlight::Keyword)),
            (1, None),
        ]
    );
    assert_eq!(
        merged(&base, &slots, [None, None]),
        base,
        "no runs: as it was"
    );
}

#[test]
fn a_side_highlighted_whole_draws_plain_text_in_the_texts_colour() {
    let (rope, highlights) = side(b"fn a\n", &[(0, 2, Highlight::Keyword)]);
    let done = project(&rope, false, &highlights, [1], true);
    assert_eq!(
        painted([None, Some(&done)]),
        [
            (1, Some(Highlight::DiffAdded)),
            (2, Some(Highlight::Keyword)),
            (2, None),
            (1, Some(Highlight::DiffAdded)),
        ],
        "the sign keeps its face"
    );
}

#[test]
fn until_a_side_is_whole_what_has_no_run_keeps_its_diff_face() {
    let (rope, highlights) = side(b"fn a\n", &[(0, 2, Highlight::Keyword)]);
    let partial = project(&rope, false, &highlights, [1], false);
    assert_eq!(
        painted([None, Some(&partial)]),
        [
            (1, Some(Highlight::DiffAdded)),
            (2, Some(Highlight::Keyword)),
            (3, Some(Highlight::DiffAdded)),
        ]
    );
}

#[test]
fn a_line_with_no_runs_goes_back_to_its_diff_face_and_short_runs_end_plain() {
    // The old side's runs only: the new line keeps its face.
    let (rope, highlights) = side(b"fn a\n", &[(0, 2, Highlight::Keyword)]);
    let old = project(&rope, false, &highlights, [1], true);
    assert_eq!(
        painted([Some(&old), None]),
        [(6, Some(Highlight::DiffAdded))]
    );
    // A side's line shorter than the section's text: the rest plain.
    let (rope, highlights) = side(b"fn\n", &[(0, 2, Highlight::Keyword)]);
    let short = project(&rope, false, &highlights, [1], true);
    assert_eq!(
        painted([None, Some(&short)]),
        [
            (1, Some(Highlight::DiffAdded)),
            (2, Some(Highlight::Keyword)),
            (2, None),
            (1, Some(Highlight::DiffAdded)),
        ]
    );
}
