//! Scenario tests of kills, yanks, typing over the region and the clipboard's answer,
//! with their keys, through the [`Dispatcher`] (ADR 0037, ADR 0074).
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with
//! no window. The clipboard's answer is given here, as the host would.

use std::path::PathBuf;

use oso_core::buffer::{Buffer, Verdict};
use oso_core::editor::Editor;
use oso_core::kill::{Fingerprint, FromClipboard, Piece};
use oso_core::rope::Rope;
use oso_core::selection::MarkActive;

use super::Dispatcher;
use super::motion_tests::{at, cursors, send, text};
use crate::commands::kept_messages;

/// The newest entry's pieces.
fn newest(editor: &Editor) -> Vec<String> {
    editor
        .kill_ring()
        .get(0)
        .map(|entry| entry.pieces().iter().map(Piece::text).collect())
        .unwrap_or_default()
}

/// `C-y`, and the clipboard's answer.
fn yank(d: &mut Dispatcher, editor: &mut Editor, answer: &FromClipboard) {
    send(d, editor, "C-y");
    assert!(d.take_clipboard_request(), "a yank reads the clipboard");
    d.clipboard_answered(editor, answer);
}

fn last_message(editor: &Editor) -> Option<String> {
    kept_messages(editor).pop()
}

fn from_app(text: &str, concealed: bool, hash: u64) -> FromClipboard {
    FromClipboard::Text {
        text: Rope::from(text),
        concealed,
        fingerprint: Fingerprint {
            len: text.len(),
            hash,
        },
    }
}

#[test]
fn kills_in_a_row_append_and_a_yank_puts_them_back() {
    let (mut d, mut editor) = at("one two three\n", &[0]);
    send(&mut d, &mut editor, "M-d M-d");
    assert_eq!(text(&editor), " three\n");
    assert_eq!(newest(&editor), ["one two"]);
    assert_eq!(editor.kill_ring().len(), 1);

    send(&mut d, &mut editor, "C-e");
    yank(&mut d, &mut editor, &FromClipboard::Own);
    assert_eq!(text(&editor), " threeone two\n");
    let primary = *editor.window().selections.primary();
    assert_eq!(
        (primary.cursor, primary.mark),
        (13, Some(6)),
        "the mark at the start"
    );
    assert_eq!(
        editor.window().selections.mark_active(),
        MarkActive::Inactive
    );

    send(&mut d, &mut editor, "C-z");
    assert_eq!(text(&editor), " three\n", "one undo step");
}

#[test]
fn a_kill_after_another_command_starts_an_entry() {
    let (mut d, mut editor) = at("one two three\n", &[0]);
    send(&mut d, &mut editor, "M-d C-f M-d");
    assert_eq!(editor.kill_ring().len(), 2);
    assert_eq!(newest(&editor), ["two"]);

    let (mut d, mut editor) = at("one two three\n", &[0]);
    send(&mut d, &mut editor, "M-d C-g M-d");
    assert_eq!(editor.kill_ring().len(), 2, "C-g ends the run");

    let (mut d, mut editor) = at("one two three\n", &[0]);
    send(&mut d, &mut editor, "M-d C-u 1 M-d");
    assert_eq!(editor.kill_ring().len(), 1, "a prefix argument does not");
}

#[test]
fn backward_kills_go_before() {
    let (mut d, mut editor) = at("one two three\n", &[13]);
    send(&mut d, &mut editor, "M-DEL M-DEL");
    assert_eq!(newest(&editor), ["two three"]);
    send(&mut d, &mut editor, "M-DEL C-y");
    assert_eq!(newest(&editor), ["one two three"]);
}

#[test]
fn kill_line_takes_the_rest_the_break_or_the_whole_line() {
    let (mut d, mut editor) = at("ab cd\nef\n", &[3]);
    send(&mut d, &mut editor, "C-k");
    assert_eq!(
        (text(&editor).as_str(), newest(&editor)),
        ("ab \nef\n", vec!["cd".into()])
    );
    send(&mut d, &mut editor, "C-k");
    assert_eq!(text(&editor), "ab ef\n", "at the end, the break");
    assert_eq!(newest(&editor), ["cd\n"], "appended");

    let (mut d, mut editor) = at("ab\ncd\nef\n", &[3]);
    send(&mut d, &mut editor, "C-k");
    assert_eq!(text(&editor), "ab\nef\n", "at the start, the whole line");
    let (mut d, mut editor) = at("ab  \ncd\n", &[2]);
    send(&mut d, &mut editor, "C-k");
    assert_eq!(text(&editor), "abcd\n", "only blanks after it");
}

#[test]
fn kill_line_with_a_count() {
    let (mut d, mut editor) = at("a\nb\nc\nd\n", &[2]);
    send(&mut d, &mut editor, "C-u 2 C-k");
    assert_eq!(text(&editor), "a\nd\n");
    let (mut d, mut editor) = at("ab\ncd\n", &[4]);
    send(&mut d, &mut editor, "C-u 0 C-k");
    assert_eq!(
        (text(&editor).as_str(), newest(&editor)),
        ("ab\nd\n", vec!["c".into()])
    );
    let (mut d, mut editor) = at("ab\ncd\n", &[4]);
    send(&mut d, &mut editor, "M-- C-k");
    assert_eq!(text(&editor), "d\n", "back to the line before");
}

#[test]
fn kill_line_at_the_end_signals() {
    let (mut d, mut editor) = at("ab", &[2]);
    send(&mut d, &mut editor, "C-k");
    assert_eq!(last_message(&editor).as_deref(), Some("End of buffer"));
    assert!(editor.kill_ring().is_empty());
}

#[test]
fn c_w_kills_the_region_or_else_the_word_before() {
    let (mut d, mut editor) = at("one two three\n", &[4]);
    send(&mut d, &mut editor, "C-SPC M-f C-w");
    assert_eq!(
        (text(&editor).as_str(), newest(&editor)),
        ("one  three\n", vec!["two".into()])
    );
    let (mut d, mut editor) = at("one two three\n", &[7]);
    send(&mut d, &mut editor, "C-w");
    assert_eq!(text(&editor), "one  three\n");
    assert_eq!(newest(&editor), ["two"]);
}

#[test]
fn m_w_copies_and_hides_the_region() {
    let (mut d, mut editor) = at("one two\n", &[0]);
    send(&mut d, &mut editor, "C-SPC M-f M-w");
    assert_eq!(text(&editor), "one two\n");
    assert_eq!(newest(&editor), ["one"]);
    assert_eq!(
        editor.window().selections.mark_active(),
        MarkActive::Inactive
    );
    send(&mut d, &mut editor, "M-f M-w");
    assert_eq!(
        newest(&editor),
        ["one two"],
        "to the mark, with the region hidden"
    );

    let (mut d, mut editor) = at("one\n", &[0]);
    send(&mut d, &mut editor, "M-w");
    assert_eq!(
        last_message(&editor).as_deref(),
        Some("The mark is not set now, so there is no region")
    );
}

#[test]
fn a_read_only_buffer_copies_and_signals() {
    // An invalid byte makes a buffer read-only.
    let mut editor = Editor::with_buffer(Buffer::from_bytes("t", b"ab\xE9\n"));
    let mut d = Dispatcher::new();
    send(&mut d, &mut editor, "C-k");
    assert_eq!(
        last_message(&editor).as_deref(),
        Some("Buffer is read-only")
    );
    assert_eq!(newest(&editor), ["ab\u{FFFD}\n"]);
}

#[test]
fn typing_and_yanking_replace_the_region_and_del_deletes_it() {
    let (mut d, mut editor) = at("one two\n", &[0]);
    send(&mut d, &mut editor, "C-SPC M-f x");
    assert_eq!(text(&editor), "x two\n");
    assert!(editor.kill_ring().is_empty(), "replaced text is not killed");

    let (mut d, mut editor) = at("one two\n", &[0]);
    send(&mut d, &mut editor, "C-SPC M-f DEL");
    assert_eq!(text(&editor), " two\n");
    send(&mut d, &mut editor, "C-e C-SPC DEL");
    assert_eq!(
        text(&editor),
        " tw\n",
        "an empty region is none: a character"
    );

    let (mut d, mut editor) = at("one two\n", &[4]);
    send(&mut d, &mut editor, "M-d C-a C-SPC M-f");
    yank(&mut d, &mut editor, &FromClipboard::Own);
    assert_eq!(text(&editor), "two \n");
}

#[test]
fn each_cursor_kills_and_yanks_its_own_piece() {
    let (mut d, mut editor) = at("ab cd\nef gh\n", &[0, 6]);
    send(&mut d, &mut editor, "M-d");
    assert_eq!(text(&editor), " cd\n gh\n");
    assert_eq!(newest(&editor), ["ab", "ef"]);
    send(&mut d, &mut editor, "C-e");
    yank(&mut d, &mut editor, &FromClipboard::Own);
    assert_eq!(text(&editor), " cdab\n ghef\n");
}

#[test]
fn another_number_of_cursors_gets_the_pieces_joined() {
    let (mut d, mut editor) = at("ab cd\nef gh\n", &[0, 6]);
    send(&mut d, &mut editor, "M-d M->");
    assert_eq!(cursors(&editor), [8], "one cursor left");
    yank(&mut d, &mut editor, &FromClipboard::Own);
    assert_eq!(text(&editor), " cd\n gh\nab\nef");
}

#[test]
fn overlapping_kills_take_their_union_once() {
    let (mut d, mut editor) = at("hello world\n", &[0, 2]);
    send(&mut d, &mut editor, "M-d");
    assert_eq!(text(&editor), " world\n");
    assert_eq!(
        newest(&editor),
        ["hello"],
        "one piece for the cursors that met"
    );
    assert_eq!(cursors(&editor), [0]);
}

#[test]
fn text_from_another_app_is_yanked_and_kept() {
    let (mut d, mut editor) = at("x\n", &[1]);
    yank(&mut d, &mut editor, &from_app("pasted", false, 1));
    assert_eq!(text(&editor), "xpasted\n");
    assert_eq!(newest(&editor), ["pasted"]);
    assert!(!editor.kill_ring().get(0).unwrap().is_secret());
}

#[test]
fn the_same_text_from_another_app_is_kept_once() {
    let (mut d, mut editor) = at("\n", &[0]);
    yank(&mut d, &mut editor, &from_app("p", false, 1));
    yank(&mut d, &mut editor, &from_app("p", false, 1));
    assert_eq!(text(&editor), "pp\n");
    assert_eq!(editor.kill_ring().len(), 1);
}

#[test]
fn keys_after_a_yank_wait_for_it() {
    let (mut d, mut editor) = at("\n", &[0]);
    send(&mut d, &mut editor, "C-y a C-f");
    assert_eq!(text(&editor), "\n", "nothing yet");
    assert!(d.waits_for_clipboard());
    assert!(d.take_clipboard_request());
    assert!(!d.take_clipboard_request(), "asked once");
    d.clipboard_answered(&mut editor, &from_app("y", false, 1));
    assert_eq!(text(&editor), "ya\n");
    assert!(!d.waits_for_clipboard());
}

#[test]
fn a_yank_with_no_answer_takes_the_newest_kill_and_says_so() {
    let (mut d, mut editor) = at("ab\n", &[0]);
    send(&mut d, &mut editor, "M-d C-e");
    yank(&mut d, &mut editor, &FromClipboard::NoAnswer);
    assert_eq!(text(&editor), "ab\n");
    assert_eq!(
        last_message(&editor).as_deref(),
        Some("Clipboard not read; yanked the newest kill")
    );
    let (mut d, mut editor) = at("\n", &[0]);
    yank(&mut d, &mut editor, &FromClipboard::Nothing);
    assert_eq!(last_message(&editor).as_deref(), Some("Kill ring is empty"));
}

#[test]
fn a_numeric_prefix_yanks_from_the_ring_without_the_clipboard() {
    let (mut d, mut editor) = at("one two\n", &[0]);
    send(&mut d, &mut editor, "M-d C-f M-d C-u 2 C-y");
    assert!(!d.waits_for_clipboard());
    assert_eq!(text(&editor), " one\n");

    let (mut d, mut editor) = at("ab\n", &[0]);
    send(&mut d, &mut editor, "M-d C-u C-y");
    assert!(d.take_clipboard_request(), "a plain C-u still reads");
    d.clipboard_answered(&mut editor, &FromClipboard::Own);
    assert_eq!(
        editor.window().selections.primary().cursor,
        0,
        "the cursor at the start"
    );
}

/// An editor showing an editable buffer that may hold secrets, with `text`, and an
/// ordinary one to yank into.
fn secret_and_notes(text: &str) -> (Dispatcher, Editor, oso_core::buffer::BufferId) {
    let path = PathBuf::from("/test/.env");
    let mut secret = Buffer::visiting(path.clone());
    secret.judge_secret(Some(&[path]));
    secret.finish_empty();
    let mut editor = Editor::with_buffer(secret);
    let mut d = Dispatcher::new();
    d.insert_text(&mut editor, text);
    send(&mut d, &mut editor, "C-a");
    let notes = editor.add_buffer(Buffer::from_text("notes", ""));
    (d, editor, notes)
}

#[test]
fn a_kill_from_a_secret_buffer_is_secret_and_makes_its_yank_target_secret() {
    let (mut d, mut editor, notes) = secret_and_notes("TOKEN=abc");
    send(&mut d, &mut editor, "C-k");
    let write = editor.take_clipboard_write().unwrap();
    assert!(write.secret, "concealed on the clipboard");
    assert_eq!(write.text(), "TOKEN=abc");
    editor.note_clipboard_written(write.id, Fingerprint { len: 9, hash: 42 });

    editor.show(notes);
    yank(&mut d, &mut editor, &FromClipboard::Own);
    assert_eq!(text(&editor), "TOKEN=abc");
    assert_eq!(editor.current().1.verdict(), Verdict::Yanked);
}

#[test]
fn text_from_another_app_counts_as_secret_when_it_matches_or_is_concealed() {
    let (mut d, mut editor, notes) = secret_and_notes("TOKEN=abc");
    send(&mut d, &mut editor, "C-k");
    let write = editor.take_clipboard_write().unwrap();
    editor.note_clipboard_written(write.id, Fingerprint { len: 9, hash: 42 });
    editor.show(notes);
    // Copied out through another app and back: the same fingerprint.
    yank(&mut d, &mut editor, &from_app("TOKEN=abc", false, 42));
    assert_eq!(editor.current().1.verdict(), Verdict::Yanked);

    let (mut d, mut editor) = at("\n", &[0]);
    yank(&mut d, &mut editor, &from_app("pw", true, 7));
    assert_eq!(
        editor.current().1.verdict(),
        Verdict::Yanked,
        "a password manager's"
    );
    let (mut d, mut editor) = at("\n", &[0]);
    yank(&mut d, &mut editor, &from_app("pw", false, 7));
    assert_eq!(editor.current().1.verdict(), Verdict::Ordinary);
}

#[test]
fn an_entry_too_large_for_the_clipboard_stays_in_the_ring() {
    let big = "x".repeat(oso_core::kill::MAX_CLIPBOARD + 1);
    let (mut d, mut editor) = at(&big, &[0]);
    send(&mut d, &mut editor, "C-k");
    assert!(editor.take_clipboard_write().is_none());
    assert_eq!(
        last_message(&editor).as_deref(),
        Some("Not copied to the clipboard: too large")
    );
    assert_eq!(editor.kill_ring().get(0).unwrap().len(), big.len());
}

#[test]
fn a_yank_over_its_bound_inserts_nothing() {
    // 2 MiB at each of 9 cursors is over the 16 MiB a yank may insert.
    let line = "x".repeat(2 * 1024 * 1024);
    let content = format!("{line}\n{}", "\n".repeat(9));
    let (mut d, mut editor) = at(&content, &[0]);
    send(&mut d, &mut editor, "C-k");
    let starts: Vec<usize> = (1..=9).collect();
    super::motion_tests::place_at(&mut editor, &starts);
    let before = text(&editor);
    yank(&mut d, &mut editor, &FromClipboard::Own);
    assert_eq!(
        last_message(&editor).as_deref(),
        Some("Too much text to yank")
    );
    assert_eq!(text(&editor), before);
}
