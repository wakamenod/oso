//! Text-grid snapshots of soft wrap (ADR 0100, ADR 0104): wrapped paragraphs of Japanese and
//! English, the mark and a space that hangs in its column, a region and a match over a break,
//! a preedit at a row's end, and line numbers on a wrapped line's first row only.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::display_map::Preedit;
use oso_core::key::KeySequence;

use super::super::Dispatcher;
use super::{assert_frames, check, check_editor, open, text_grid};

/// The keys that turn wrapping on (ADR 0100).
const WRAP: &str = "C-x x t";

#[test]
fn a_paragraph_of_japanese_wraps_with_kinsoku_and_the_mark() {
    // 21 columns: 20 of text and the mark's. No row starts with `、` or `。`.
    let text = "日本語の文章を折り返して表示します。句読点は行頭に来ません、そして括弧（かっこ）も考えます。\n次の行\n";
    check("wrap_japanese", text, (6, 21), &["", WRAP, "C-e", "C-n"]);
}

#[test]
fn a_paragraph_of_english_wraps_after_spaces_and_motion_stays_on_its_line() {
    let text = "The quick brown fox jumps over the lazy dog and keeps on running far away.\nnext\n";
    check(
        "wrap_english",
        text,
        (6, 21),
        &[WRAP, "M-f M-f M-f M-f M-f", "C-e", "C-n"],
    );
}

#[test]
fn a_space_that_hangs_takes_the_marks_cell_and_the_cursor_shows_there() {
    // 9 columns: `aaaaaaaa` fills the row's 8, and its space hangs in the mark's column.
    let text = "aaaaaaaa bbbb cccc\n";
    let steps = [WRAP, "C-u 8 C-f", "C-f", "C-x x t"];
    check("wrap_hanging_space", text, (4, 9), &steps);
}

#[test]
fn a_region_and_a_match_over_a_break_fill_to_the_rows_text_width() {
    let text = "aaaa bbbb cccc dddd\nend\n";
    let steps = [
        WRAP,
        "C-f C-f C-SPC C-u 6 C-f",
        "C-u 8 C-f",
        "C-g C-a",
        "C-s a SPC b",
    ];
    check("wrap_region_and_match", text, (4, 9), &steps);
}

#[test]
fn a_preedit_at_a_rows_end_wraps_as_if_it_were_typed() {
    let text = "aaaa bbb\nnext\n";
    let mut editor = open(text.as_bytes(), (4, 9));
    let mut dispatcher = Dispatcher::new();
    let keys: KeySequence = format!("{WRAP} C-e").parse().unwrap();
    dispatcher.handle_keys(&mut editor, keys.0);
    let mut frames = Vec::new();
    let mut show = |editor: &mut oso_core::editor::Editor, step: &str, composed: &str| {
        editor.set_preedit(Some(Preedit::new(composed.into(), None)));
        frames.push(format!("{step}\n{}", text_grid(editor)));
    };
    show(&mut editor, "a word that fits", "x");
    show(&mut editor, "a word past the edge", "xyz");
    show(&mut editor, "longer than a row", "にほんごにゅうりょく");
    editor.set_preedit(None);
    frames.push(format!("cleared\n{}", text_grid(&editor)));
    assert_frames("wrap_preedit_at_the_edge", text.as_bytes(), &frames);
}

#[test]
fn a_closing_mark_typed_at_a_rows_start_pulls_the_row_before() {
    // Four wide characters fill a row of 8; `。` may not start the next, so `え` comes down
    // with it as it will when committed.
    let text = "あいうえおか\n";
    let mut editor = open(text.as_bytes(), (3, 9));
    let mut dispatcher = Dispatcher::new();
    let keys: KeySequence = format!("{WRAP} C-u 4 C-f").parse().unwrap();
    dispatcher.handle_keys(&mut editor, keys.0);
    let before = format!("before\n{}", text_grid(&editor));
    editor.set_preedit(Some(Preedit::new("。".into(), None)));
    let composing = format!("composing\n{}", text_grid(&editor));
    assert_frames(
        "wrap_preedit_kinsoku",
        text.as_bytes(),
        &[before, composing],
    );
}

#[test]
fn line_numbers_are_on_a_lines_first_row_only() {
    let text = "short\na line long enough to wrap twice in the window\nlast\n";
    let mut editor = open(text.as_bytes(), (6, 24));
    editor.set_visual_aids(oso_core::editor::VisualAids {
        line_numbers: true,
        ..editor.visual_aids()
    });
    check_editor(
        "wrap_line_numbers",
        text.as_bytes(),
        editor,
        &[WRAP, "C-n C-e"],
    );
}
