//! Tests of the styles a row's text takes: what is shaped in which font, when it is shaped
//! again, and that no style moves a cell.
//!
//! Must not depend on: the GPU or `text`; a fake text system shapes, but for the tests of the
//! real fonts, on the OS that has them.

use std::ops::Range;

use oso_core::buffer::Buffer;
use oso_core::display_map::Preedit;
use oso_core::syntax::{Highlight, Highlights};
use oso_platform::text::FontStyle::{self, Bold, Italic, Regular};

use super::MAX_STRETCHES;
use crate::pane::Face;
use crate::rows::tests::Recorder;
use crate::rows::{Rows, ShapedRow};

/// `text`'s highlights: `runs` as ranges with what each is, the rest plain.
fn highlights(text: &str, runs: &[(Range<usize>, Highlight)]) -> Highlights {
    let mut highlights = Highlights::new(text.len());
    for (range, highlight) in runs {
        highlights.replace(range.clone(), [(range.len(), Some(*highlight))]);
    }
    highlights
}

/// One buffer's line 0, drawn again and again in one row cache.
struct Setup {
    rows: Rows,
    buffer: Buffer,
    revision: u64,
    system: Recorder,
}

impl Setup {
    fn new(text: &str) -> Self {
        Self {
            rows: Rows::default(),
            buffer: Buffer::from_text("t", text),
            revision: 1,
            system: Recorder::default(),
        }
    }

    /// The row in the styles of `styles`, if any.
    fn row(&mut self, styles: Option<&Highlights>) -> &ShapedRow {
        let id = oso_core::editor::Editor::default().window().buffer();
        self.rows.begin(id, self.revision);
        let display = self.buffer.display();
        let line = (0, 0..128);
        self.rows.view_row(
            &display,
            line,
            (None, None),
            (None, styles),
            &mut self.system,
        )
    }

    /// What drawing the row shaped, and each in which style.
    fn shaped(&mut self, styles: Option<&Highlights>) -> Vec<(String, FontStyle)> {
        self.row(styles);
        let shaped = std::mem::take(&mut self.system.shaped);
        shaped
            .into_iter()
            .zip(std::mem::take(&mut self.system.styles))
            .collect()
    }
}

/// `pieces` as [`Setup::shaped`] gives them.
fn pieces(pieces: &[(&str, FontStyle)]) -> Vec<(String, FontStyle)> {
    (pieces.iter())
        .map(|(text, style)| ((*text).to_owned(), *style))
        .collect()
}

#[test]
fn each_stretch_of_a_style_is_shaped_in_its_font() {
    let text = "a bold 値 italic code";
    let runs = [
        (2..6, Highlight::SessionStrong),
        (7..10, Highlight::SessionHeading),
        (11..17, Highlight::SessionEmphasis),
        (18..22, Highlight::SessionStrongCode),
    ];
    let styles = highlights(text, &runs);
    let mut setup = Setup::new(text);
    let want = pieces(&[
        ("a ", Regular),
        ("bold", Bold),
        (" ", Regular),
        ("値", Bold),
        (" ", Regular),
        ("italic", Italic),
        (" ", Regular),
        ("code", Bold),
    ]);
    let got = setup.shaped(Some(&styles));
    // A piece of the same text and style is shaped once.
    let mut once = Vec::new();
    for piece in want {
        if !once.contains(&piece) {
            once.push(piece);
        }
    }
    assert_eq!(got, once);
    assert_eq!(setup.shaped(Some(&styles)), [], "and once only");
}

#[test]
fn a_row_of_one_style_is_one_piece() {
    let text = "a heading";
    let styles = highlights(text, &[(0..9, Highlight::SessionHeading)]);
    let mut setup = Setup::new(text);
    assert_eq!(setup.shaped(Some(&styles)), pieces(&[(text, Bold)]));
}

#[test]
fn a_view_of_no_styles_is_shaped_as_one_regular_piece_whatever_its_highlights() {
    let text = "a bold word";
    let mut setup = Setup::new(text);
    assert_eq!(setup.shaped(None), pieces(&[(text, Regular)]));
    // A transcript's rows in classes with no style are as they were too.
    let runs = highlights(
        text,
        &[(2..6, Highlight::Keyword), (7..11, Highlight::Link)],
    );
    assert_eq!(setup.shaped(Some(&runs)), []);
}

#[test]
fn highlights_that_come_late_shape_the_row_again_and_others_do_not() {
    let text = "a bold word";
    let none = highlights(text, &[]);
    let bold = highlights(text, &[(2..6, Highlight::SessionStrong)]);
    let more = highlights(text, &[(2..7, Highlight::SessionStrong)]);
    let coloured = highlights(
        text,
        &[
            (0..1, Highlight::Keyword),
            (2..6, Highlight::SessionStrong),
            (7..11, Highlight::SessionCode),
        ],
    );
    let mut setup = Setup::new(text);
    setup.shaped(Some(&none));
    // At the same revision, in and out of the style, and as its end moves.
    let want = pieces(&[("a ", Regular), ("bold", Bold), (" word", Regular)]);
    assert_eq!(setup.shaped(Some(&bold)), want);
    assert_eq!(setup.shaped(Some(&coloured)), [], "colours shape nothing");
    assert_eq!(setup.shaped(Some(&more)).len(), 2, "its end moved");
    let glyphs = |row: &ShapedRow| row.glyphs.iter().map(|g| g.key.font.0).collect::<Vec<_>>();
    assert!(glyphs(setup.row(Some(&none))).iter().all(|font| *font == 0));
    // With font-lock off, the view has no highlights.
    assert!(glyphs(setup.row(Some(&bold))).contains(&1));
    assert!(glyphs(setup.row(None)).iter().all(|font| *font == 0));
    assert!(glyphs(setup.row(Some(&bold))).contains(&1));
}

#[test]
fn a_style_goes_with_its_text_through_a_change() {
    let mut setup = Setup::new("a bold word");
    let bold = highlights("a bold word", &[(2..6, Highlight::SessionStrong)]);
    setup.shaped(Some(&bold));
    // The same text at another revision, in other styles.
    setup.revision += 1;
    let italic = highlights("a bold word", &[(2..6, Highlight::SessionEmphasis)]);
    let want = pieces(&[("a ", Regular), ("bold", Italic), (" word", Regular)]);
    assert_eq!(setup.shaped(Some(&italic)), want);
    setup.revision += 1;
    assert_eq!(setup.shaped(Some(&italic)), []);
}

#[test]
fn a_links_cells_are_underlined_and_it_is_shaped_regular() {
    let text = "a 値 link 値, b";
    let start = 2;
    let end = text.find(',').unwrap();
    let styles = highlights(text, &[(start..end, Highlight::SessionLink)]);
    let mut setup = Setup::new(text);
    assert_eq!(setup.row(Some(&styles)).underlines(), [(2, 10)]);
    assert_eq!(setup.system.styles, [Regular]);
    assert!(setup.row(None).underlines().is_empty());
    // Nor is the wrap mark after it.
    let display = setup.buffer.display();
    let line = (0, 0..5);
    let (marks, look) = ((Some(5), None), (None, Some(&styles)));
    let mut rows = Rows::default();
    let row = rows.view_row(&display, line, marks, look, &mut setup.system);
    assert_eq!(row.underlines(), [(2, 3)]);
}

#[test]
fn escapes_boxes_whitespace_and_a_row_with_a_preedit_are_regular_in_a_style() {
    let text = "a\u{1}\u{202e} b";
    let styles = highlights(text, &[(0..text.len(), Highlight::SessionHeading)]);
    let mut setup = Setup::new(text);
    setup.rows.set_whitespace(true);
    for (piece, style) in setup.shaped(Some(&styles)) {
        let bold = matches!(piece.as_str(), "a" | "b" | " b" | " ");
        assert_eq!(style == Bold, bold, "{piece:?} {style:?}");
    }
    let buffer = Buffer::from_text("t", "ab");
    let styles = highlights("ab", &[(0..2, Highlight::SessionHeading)]);
    let preedit = Preedit::new("か".to_string(), None);
    let mut system = Recorder::default();
    let mut rows = Rows::default();
    rows.view_row(
        &buffer.display(),
        (0, 0..128),
        (Some(4), None),
        (Some((1, &preedit)), Some(&styles)),
        &mut system,
    );
    assert_eq!(system.shaped, ["a", "か", "b", "\u{21a9}"]);
    assert_eq!(system.styles, [Regular; 4], "no row composed in has styles");
}

#[test]
fn a_cluster_takes_the_style_of_its_first_byte_and_is_never_cut() {
    // The highlight ends inside the cluster, after the letter.
    let text = "e\u{301}x";
    let styles = highlights(text, &[(0..1, Highlight::SessionStrong)]);
    let mut setup = Setup::new(text);
    let want = pieces(&[("e\u{301}", Bold), ("x", Regular)]);
    assert_eq!(setup.shaped(Some(&styles)), want);
    assert_eq!(setup.shaped(Some(&styles)), []);
}

#[test]
fn a_row_has_a_bounded_number_of_styled_stretches() {
    let text = "ab".repeat(60);
    let runs: Vec<_> = (0..60)
        .map(|n| (2 * n..2 * n + 1, Highlight::SessionStrong))
        .collect();
    let styles = highlights(&text, &runs);
    let mut setup = Setup::new(&text);
    let row = setup.row(Some(&styles));
    let bold = row.glyphs.iter().filter(|g| g.key.font.0 == 1).count();
    assert_eq!(bold, MAX_STRETCHES);
    assert_eq!(row.glyphs.len(), text.len(), "the rest is drawn, regular");
    assert!(row.breaks.len() <= 2 * MAX_STRETCHES + 1);
}

#[test]
fn a_short_piece_is_kept_by_its_style_too() {
    let text = "aaa";
    let runs = [
        (0..1, Highlight::SessionStrong),
        (1..2, Highlight::SessionEmphasis),
    ];
    let mut setup = Setup::new(text);
    let row = setup.row(Some(&highlights(text, &runs)));
    let fonts: Vec<_> = row.glyphs.iter().map(|g| g.key.font.0).collect();
    assert_eq!(fonts, [1, 2, 0]);
    assert_eq!(setup.system.shaped, ["a"; 3]);
}

#[test]
fn a_styled_row_is_reused_without_its_text_while_the_revision_and_its_styles_hold() {
    // Another text at the same revision shows whether the row was read.
    let (text, other) = ("a bold word and more", "x xxxx xxxx xxx xxxx");
    let styles = highlights(text, &[(2..14, Highlight::SessionStrong)]);
    let (buffer, changed) = (Buffer::from_text("t", text), Buffer::from_text("t", other));
    let id = oso_core::editor::Editor::default().window().buffer();
    let mut rows = Rows::default();
    let mut system = Recorder::default();
    let mut draw = |buffer: &Buffer, columns: Range<usize>, styles: &Highlights| -> String {
        rows.begin(id, 1);
        let (marks, look) = ((None, None), (None, Some(styles)));
        let row = rows.view_row(&buffer.display(), (0, columns), marks, look, &mut system);
        (row.glyphs.iter())
            .map(|g| char::from_u32(u32::from(g.key.glyph.0)).unwrap())
            .collect()
    };
    // Whole, and a row its line goes on in, which starts inside the style.
    for columns in [0..20, 4..20] {
        let want = text[columns.clone()].to_owned();
        assert_eq!(draw(&buffer, columns.clone(), &styles), want);
        assert_eq!(draw(&changed, columns.clone(), &styles), want, "not read");
        let late = highlights(text, &[(2..13, Highlight::SessionStrong)]);
        assert_eq!(
            draw(&changed, columns, &late),
            other[20 - want.len()..],
            "read"
        );
    }
}

#[test]
fn a_row_has_a_bounded_number_of_underlines() {
    let text = "ab".repeat(60);
    let runs: Vec<_> = (0..60)
        .map(|n| (2 * n..2 * n + 1, Highlight::SessionLink))
        .collect();
    let styles = highlights(&text, &runs);
    let mut setup = Setup::new(&text);
    assert_eq!(setup.row(Some(&styles)).underlines().len(), MAX_STRETCHES);
}

#[test]
fn a_line_or_another_buffer_after_a_styled_row_is_regular() {
    let text = "bold";
    let styles = highlights(
        text,
        &[
            (0..4, Highlight::SessionLink),
            (1..3, Highlight::SessionStrong),
        ],
    );
    let mut setup = Setup::new(text);
    assert!(!setup.row(Some(&styles)).underlines().is_empty());
    setup.rows.begin_lines();
    let spans = [crate::pane::Span {
        text,
        face: Face::Text,
    }];
    setup.system = Recorder::default();
    let row = setup.rows.line(0, &spans, 40, &mut setup.system);
    assert!(row.underlines().is_empty());
    assert_eq!(setup.system.styles, [Regular]);
}

/// One step of what a window may do with a row of a transcript.
#[derive(Clone, Debug)]
enum Step {
    /// Other highlights: each stretch's bytes and class.
    Highlights(Vec<(usize, Highlight)>),
    /// Another text, at another revision.
    Text(String),
    /// Where the row starts and ends in the line, and its place.
    Columns(usize, usize, usize),
    /// With styles, or with none, as font-lock goes.
    Styled(bool),
    /// A change elsewhere in the buffer.
    Revision,
}

fn step() -> impl proptest::strategy::Strategy<Value = Step> {
    use proptest::prelude::*;
    let class = prop::sample::select(vec![
        Highlight::SessionStrong,
        Highlight::SessionEmphasis,
        Highlight::SessionLink,
        Highlight::SessionHeading,
        Highlight::Keyword,
    ]);
    let part = prop::sample::select(vec![
        "a", "bc", "検", "e\u{301}", "🍣", "\u{1}", "\u{202e}", " ",
    ]);
    prop_oneof![
        3 => prop::collection::vec((1usize..6, class), 0..12).prop_map(Step::Highlights),
        2 => prop::collection::vec(part, 0..24).prop_map(|parts| Step::Text(parts.concat())),
        2 => (0usize..12, 1usize..40, 0usize..3).prop_map(|(a, b, c)| Step::Columns(a, b, c)),
        1 => any::<bool>().prop_map(Step::Styled),
        1 => Just(Step::Revision),
    ]
}

proptest::proptest! {
    /// Whatever happens to a row, it is drawn as one read and shaped anew is.
    #[test]
    fn a_kept_row_is_the_row_drawn_anew(steps in proptest::collection::vec(step(), 1..40)) {
        let id = oso_core::editor::Editor::default().window().buffer();
        let (mut kept, mut system) = (Rows::default(), Recorder::default());
        let (mut text, mut revision, mut styled) = ("a検bc e\u{301}x".to_owned(), 1, true);
        let (mut runs, mut columns, mut slot) = (Vec::new(), 0..40, 0);
        for step in steps {
            match step {
                Step::Highlights(to) => runs = to,
                Step::Text(to) => {
                    text = to;
                    revision += 1;
                }
                Step::Columns(start, cells, to) => (columns, slot) = (start..start + cells, to),
                Step::Styled(to) => styled = to,
                Step::Revision => revision += 1,
            }
            let mut styles = Highlights::new(text.len());
            let mut at = 0;
            for (len, highlight) in &runs {
                let end = (at + len).min(text.len());
                styles.replace(at..end, [(end - at, Some(*highlight))]);
                at = end;
            }
            let buffer = Buffer::from_text("t", &text);
            let display = buffer.display();
            let mut draw = |rows: &mut Rows| {
                rows.begin(id, revision);
                rows.will_draw([(0, columns.start)]);
                let (marks, look) = ((None, Some(slot)), (None, styled.then_some(&styles)));
                let row = rows.view_row(&display, (0, columns.clone()), marks, look, &mut system);
                let glyphs: Vec<_> = row.glyphs.clone();
                (glyphs, row.boxes.clone(), row.underlines().to_vec())
            };
            proptest::prop_assert_eq!(draw(&mut kept), draw(&mut Rows::default()));
        }
    }
}

/// What a script's text is drawn in must be its cells: Latin, Japanese, emoji, combining marks,
/// a table's rules, and letters of other scripts.
#[cfg(target_os = "macos")]
mod fonts {
    use std::collections::BTreeSet;

    use oso_core::display_map::text_clusters;
    use oso_platform::text::{FontRequest, load};
    use proptest::prelude::*;

    use super::*;

    const PARTS: [&str; 18] = [
        "a",
        "M",
        "fi",
        "->",
        "_",
        " ",
        "検証",
        "カ",
        "ｶ",
        "한",
        "🍣",
        "👨\u{200d}👩\u{200d}👧",
        "❤\u{fe0f}",
        "e\u{301}",
        "か\u{3099}",
        "│─",
        "\u{5d1}\u{5bc}",
        "\u{633}\u{644}",
    ];

    /// The cells each cluster of `row` paints: its columns and how many.
    fn cells(row: &ShapedRow) -> BTreeSet<(usize, u32)> {
        row.glyphs.iter().map(|g| (g.column, g.cells)).collect()
    }

    fn check(text: &str, stretches: &[(usize, Highlight)]) {
        let request = FontRequest {
            families: &["Menlo"],
            size_pt: 13.0,
        };
        let mut system = load(&request, 2.0).expect("Menlo ships with macOS");
        let buffer = Buffer::from_text("t", text);
        let display = buffer.display();
        let mut styles = Highlights::new(text.len());
        let mut at = 0;
        for (len, highlight) in stretches {
            let end = text.ceil_char_boundary((at + len).min(text.len()));
            styles.replace(at..end, [(end - at, Some(*highlight))]);
            at = end;
        }
        let mut row = |styles: Option<&Highlights>| {
            let line = (0, 0..256);
            let (marks, look) = ((None, None), (None, styles));
            cells(Rows::default().view_row(&display, line, marks, look, &mut *system))
        };
        let (regular, styled) = (row(None), row(Some(&styles)));
        // Where each cluster is, as the display model says, whatever the font.
        let clusters: BTreeSet<(usize, usize)> = (text_clusters(text, 0))
            .map(|cluster| (cluster.column, cluster.cells))
            .collect();
        for drawn in [&regular, &styled] {
            for &(column, cells) in drawn {
                let mut end = column;
                while end < column + cells as usize {
                    let cluster = clusters.iter().find(|(at, _)| *at == end);
                    end += cluster.unwrap_or_else(|| panic!("{text:?} {column}")).1;
                }
                assert_eq!(end, column + cells as usize, "{text:?} {column}");
            }
        }
        let cover = |drawn: &BTreeSet<(usize, u32)>| {
            (drawn.iter())
                .flat_map(|&(column, cells)| column..column + cells as usize)
                .collect::<BTreeSet<usize>>()
        };
        assert_eq!(cover(&styled), cover(&regular), "{text:?}");
    }

    #[test]
    fn every_kind_of_text_keeps_its_cells_in_every_style() {
        for highlight in [
            Highlight::SessionStrong,
            Highlight::SessionEmphasis,
            Highlight::SessionLink,
        ] {
            for part in PARTS {
                let text = format!("a{part}{part}b");
                check(&text, &[(text.len(), highlight)]);
                check(&text, &[(1, highlight)]);
                check(&text, &[(1, Highlight::Keyword), (1, highlight)]);
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn any_text_keeps_its_cells_in_any_styles(
            parts in prop::collection::vec(prop::sample::select(PARTS.to_vec()), 1..30),
            stretches in prop::collection::vec(
                (1usize..9, prop::sample::select(vec![
                    Highlight::SessionStrong,
                    Highlight::SessionEmphasis,
                    Highlight::SessionLink,
                    Highlight::Keyword,
                ])),
                0..20,
            ),
        ) {
            check(&parts.concat(), &stretches);
        }
    }
}
