//! Scenario tests of the expression commands with their keys, through the [`Dispatcher`]
//! (ADR 0190, step 2): every key, Shift, counts, a repeated `C-M-SPC` and kills that append.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::kill::Piece;
use oso_core::selection::MarkActive;

use super::motion_tests::{at, cursors, send, text};

fn newest(editor: &oso_core::editor::Editor) -> Vec<String> {
    editor
        .kill_ring()
        .get(0)
        .map(|entry| entry.pieces().iter().map(Piece::text).collect())
        .unwrap_or_default()
}

#[test]
fn every_motion_key_runs_its_command() {
    // Each key, the text, where the cursor starts and where it goes.
    let cases: &[(&str, &str, usize, usize)] = &[
        ("C-M-f", "(a) b", 0, 3),
        ("C-M-<right>", "(a) b", 0, 3),
        ("C-M-b", "(a) b", 3, 0),
        ("C-M-<left>", "(a) b", 3, 0),
        ("C-M-n", "a (b) c", 0, 5),
        ("C-M-p", "a (b) c", 7, 2),
        ("C-M-u", "(a b)", 2, 0),
        ("C-M-<up>", "(a b)", 2, 0),
        ("C-M-d", "a (b)", 0, 3),
        ("C-M-<down>", "a (b)", 0, 3),
    ];
    for &(keys, line, from, to) in cases {
        let (mut d, mut editor) = at(line, &[from]);
        send(&mut d, &mut editor, keys);
        assert_eq!(cursors(&editor), [to], "{keys}");
    }
}

#[test]
fn counts_and_negative_counts_reach_the_motions() {
    let (mut d, mut editor) = at("a b c d", &[0]);
    send(&mut d, &mut editor, "C-u 3 C-M-f");
    assert_eq!(cursors(&editor), [5]);
    send(&mut d, &mut editor, "M-- C-M-f");
    assert_eq!(cursors(&editor), [4]);
    send(&mut d, &mut editor, "M-2 C-M-b");
    assert_eq!(cursors(&editor), [0]);
}

#[test]
fn shift_selects_and_the_next_motion_without_it_lets_go() {
    let (mut d, mut editor) = at("(a) (b) c", &[0]);
    send(&mut d, &mut editor, "C-M-F C-M-F");
    let selections = &editor.window().selections;
    assert_eq!(selections.mark_active(), MarkActive::Shifted);
    assert_eq!(selections.primary().range(), 0..7);
    send(&mut d, &mut editor, "C-M-S-<left>");
    assert_eq!(editor.window().selections.primary().range(), 0..4);
    send(&mut d, &mut editor, "C-M-f");
    assert_eq!(
        editor.window().selections.mark_active(),
        MarkActive::Inactive
    );
}

#[test]
fn c_m_k_and_c_m_del_kill_and_append() {
    let (mut d, mut editor) = at("(a) (b) c", &[0]);
    send(&mut d, &mut editor, "C-M-k C-M-k");
    assert_eq!(text(&editor), " c");
    assert_eq!(newest(&editor), ["(a) (b)"]);
    assert_eq!(editor.kill_ring().len(), 1);

    let (mut d, mut editor) = at("x a b", &[5]);
    send(&mut d, &mut editor, "C-M-DEL C-M-DEL");
    assert_eq!(text(&editor), "x ");
    assert_eq!(newest(&editor), ["a b"], "a backward kill goes first");
    assert_eq!(editor.kill_ring().len(), 1);
}

#[test]
fn c_m_spc_marks_and_repeated_marks_more() {
    for key in ["C-M-SPC", "C-M-@"] {
        let (mut d, mut editor) = at("a b c d", &[0]);
        send(&mut d, &mut editor, key);
        let primary = *editor.window().selections.primary();
        assert_eq!((primary.cursor, primary.mark), (0, Some(1)), "{key}");
        send(&mut d, &mut editor, &format!("{key} {key}"));
        let selections = &editor.window().selections;
        assert_eq!(selections.primary().mark, Some(5), "{key}");
        assert_eq!(selections.mark_active(), MarkActive::Active);
    }
}

#[test]
fn c_m_t_transposes() {
    let (mut d, mut editor) = at("(a) b", &[3]);
    send(&mut d, &mut editor, "C-M-t");
    assert_eq!(text(&editor), "b (a)");
    assert_eq!(cursors(&editor), [5]);
}
