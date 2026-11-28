//! Scenario tests of `M-z` and `zap-up-to-char`, with their keys and the character their
//! prompt reads, through the [`Dispatcher`] (ADR 0190, step 8).
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::command::Reach;
use oso_core::editor::Editor;
use oso_core::kill::Piece;

use super::Given;
use super::motion_tests::{at, cursors, send, text};
use crate::commands::kept_messages;

/// The newest kill's pieces.
fn newest(editor: &Editor) -> Vec<String> {
    editor
        .kill_ring()
        .get(0)
        .map(|entry| entry.pieces().iter().map(Piece::text).collect())
        .unwrap_or_default()
}

fn last_message(editor: &Editor) -> Option<String> {
    kept_messages(editor).pop()
}

#[test]
fn m_z_reads_a_character_and_kills_up_to_and_through_it_at_every_cursor() {
    let (mut d, mut editor) = at("one, two\nthree, four\n", &[0, 9]);
    send(&mut d, &mut editor, "M-z");
    let prompt = editor.prompt().expect("M-z reads a character");
    assert_eq!(prompt.label(), "Zap to char:");
    send(&mut d, &mut editor, ",");
    assert!(editor.prompt().is_none(), "one character ends it");
    assert_eq!(text(&editor), " two\n four\n");
    assert_eq!(cursors(&editor), [0, 5]);
    assert_eq!(newest(&editor), ["one,", "three,"]);
}

#[test]
fn a_count_goes_to_the_nth_and_a_negative_one_goes_back() {
    let (mut d, mut editor) = at("a-b-c-d\n", &[0]);
    send(&mut d, &mut editor, "M-2 M-z -");
    assert_eq!(text(&editor), "c-d\n");
    let (mut d, mut editor) = at("a-b-c-d\n", &[7]);
    send(&mut d, &mut editor, "M-- M-z -");
    assert_eq!(text(&editor), "a-b-c\n");
    assert_eq!(cursors(&editor), [5]);
    send(&mut d, &mut editor, "M-- M-2 M-z -");
    assert_eq!(text(&editor), "a\n");
    assert_eq!(
        newest(&editor),
        ["-b-c-d"],
        "a kill after a kill goes in front"
    );
}

#[test]
fn a_character_not_found_kills_nothing_there_and_says_so_after_the_others() {
    let (mut d, mut editor) = at("ab;\ncd\n", &[0, 4]);
    send(&mut d, &mut editor, "M-z ;");
    assert_eq!(text(&editor), "\ncd\n", "the first cursor found one");
    assert_eq!(
        last_message(&editor).as_deref(),
        Some("Search failed: \";\"")
    );
}

#[test]
fn a_lowercase_character_matches_either_case_and_an_uppercase_one_only_itself() {
    let (mut d, mut editor) = at("xAyaz\n", &[0]);
    send(&mut d, &mut editor, "M-z a");
    assert_eq!(text(&editor), "yaz\n");
    let (mut d, mut editor) = at("xayAz\n", &[0]);
    send(&mut d, &mut editor, "M-z A");
    assert_eq!(text(&editor), "z\n");
}

#[test]
fn a_cluster_is_killed_whole() {
    let (mut d, mut editor) = at("ae\u{301}b\n", &[0]);
    send(&mut d, &mut editor, "M-z e");
    assert_eq!(
        text(&editor),
        "b\n",
        "the combining mark goes with its letter"
    );
}

#[test]
fn zap_up_to_char_stops_before_it_and_passes_the_one_at_point() {
    let (mut d, mut editor) = at("x1x2x\n", &[0]);
    send(&mut d, &mut editor, "M-x");
    d.insert_text(&mut editor, "zap-up-to-char");
    send(&mut d, &mut editor, "RET");
    assert_eq!(
        editor.prompt().map(|p| p.label().to_owned()).as_deref(),
        Some("Zap up to char:")
    );
    send(&mut d, &mut editor, "x");
    assert_eq!(text(&editor), "x2x\n");
    assert_eq!(cursors(&editor), [0]);
    let (mut d, mut editor) = at("x1x2x\n", &[5]);
    send(&mut d, &mut editor, "M-- M-x");
    d.insert_text(&mut editor, "zap-up-to-char");
    send(&mut d, &mut editor, "RET");
    d.insert_text(&mut editor, "x");
    assert_eq!(text(&editor), "x1x\n", "back, after the one before");
}

#[test]
fn ret_on_an_empty_prompt_and_c_g_zap_nothing() {
    let (mut d, mut editor) = at("ab\n", &[0]);
    send(&mut d, &mut editor, "M-z RET");
    assert_eq!(text(&editor), "ab\n");
    assert!(editor.prompt().is_none());
    send(&mut d, &mut editor, "M-z C-g");
    assert_eq!(text(&editor), "ab\n");
    assert!(editor.prompt().is_none());
}

#[test]
fn the_agent_gives_the_character_as_text_and_its_zap_is_a_change() {
    let (mut d, mut editor) = at("one, two\n", &[0]);
    let refused = d.run_for_agent(&mut editor, "zap-to-char", Given::Text(","), Reach::Look);
    assert!(refused.is_err(), "a change, not a look");
    let (_, ran) = d
        .run_for_agent(&mut editor, "zap-to-char", Given::Text(","), Reach::Change)
        .unwrap();
    assert_eq!(ran, Ok(()));
    assert_eq!(text(&editor), " two\n");
    for given in [
        Given::Text("ab"),
        Given::Text(""),
        Given::Count(2),
        Given::Nothing,
    ] {
        let refused = d.run_for_agent(&mut editor, "zap-to-char", given, Reach::Change);
        assert!(refused.is_err(), "{given:?}");
    }
    assert_eq!(text(&editor), " two\n");
}

#[test]
fn a_zap_that_would_read_past_its_budget_kills_nothing_and_says_so() {
    let long = format!("{}x\n", "a".repeat(5 * 1024 * 1024));
    for (keys, at_) in [("M-z x", 0), ("M-- M-z y", long.len() - 1)] {
        let (mut d, mut editor) = at(&long, &[at_]);
        let started = std::time::Instant::now();
        send(&mut d, &mut editor, keys);
        let took = started.elapsed();
        assert_eq!(editor.current().1.text().len(), long.len(), "{keys}");
        assert_eq!(
            last_message(&editor).as_deref(),
            Some("Count too large"),
            "{keys}"
        );
        // A debug build; a release one reads its 4 MiB in a few milliseconds.
        assert!(took < std::time::Duration::from_secs(2), "{keys}: {took:?}");
    }
}

#[test]
fn several_cursors_share_one_budget() {
    let long = format!("{}\n", "a".repeat(3 * 1024 * 1024));
    let (mut d, mut editor) = at(&format!("{long}{long}x\n"), &[0, long.len()]);
    send(&mut d, &mut editor, "M-z x");
    assert_eq!(last_message(&editor).as_deref(), Some("Count too large"));
}

#[test]
fn a_character_yanked_from_a_secret_zaps_nothing() {
    use oso_core::buffer::Buffer;
    use oso_core::kill::Direction;
    let (mut d, mut editor) = at("one Sx two\n", &[0]);
    let notes = editor.window().buffer();
    let mut secret = Buffer::visiting(std::path::PathBuf::from("/p/.env"));
    secret.apply_load(oso_core::load::decode_all(
        b"S3CRET\n",
        oso_core::display_map::TabWidth::default(),
    ));
    secret.judge_secret(None);
    let secret = editor.add_buffer(secret);
    editor.show(secret);
    editor
        .kill(std::iter::once(0..6), Direction::Forward, true)
        .unwrap();
    editor.note_command_ended(false);
    editor.show(notes);
    let kills = editor.kill_ring().len();
    send(&mut d, &mut editor, "M-z");
    send(&mut d, &mut editor, "C-y");
    if d.take_clipboard_request().is_some() {
        d.clipboard_answered(&mut editor, &oso_core::kill::FromClipboard::Own);
    }
    assert!(editor.prompt().is_none());
    assert_eq!(text(&editor), "one Sx two\n", "no edit");
    assert_eq!(editor.kill_ring().len(), kills, "no new kill");
    let said = last_message(&editor).unwrap_or_default();
    assert!(!said.contains('S'), "{said}");
}

#[test]
fn a_folded_match_of_another_length_is_killed_to_its_own_end() {
    // U+2C65 is three bytes and lowercases nothing; U+023A, two bytes, lowercases to it.
    let (mut d, mut editor) = at("a\u{23a}bc\n", &[0]);
    send(&mut d, &mut editor, "M-z \u{2c65}");
    assert_eq!(text(&editor), "bc\n");
    let (mut d, mut editor) = at("a\u{23a}bc\n", &[4]);
    send(&mut d, &mut editor, "M-- M-x");
    d.insert_text(&mut editor, "zap-up-to-char");
    send(&mut d, &mut editor, "RET");
    d.insert_text(&mut editor, "\u{2c65}");
    assert_eq!(
        text(&editor),
        "a\u{23a}c\n",
        "after the match, whatever its length"
    );
}
