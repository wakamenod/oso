//! Tests of selecting the word at each cursor, as the keys that add cursors do with no region
//! shown (ADR 0091), within one command's reading shared by the cursors (ADR 0101).
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use super::tests::{at, editor, regions};
use crate::selection::MAX_CURSORS;

#[test]
fn selecting_the_words_at_several_cursors_shares_one_commands_reading() {
    // Words longer than a stop reads, one a line: the cursors share what one command at
    // several reads (ADR 0101), so the first get a region and those it runs out on none.
    let line = format!("{}\n", "x".repeat(200_000));
    let starts: Vec<usize> = (0..6).map(|index| index * line.len() + 1).collect();
    let mut editor = editor(&line.repeat(6));
    at(&mut editor, &starts);
    let primary = editor.select_words().unwrap();
    assert!(primary.start < primary.end);
    let (regions, _) = regions(&editor);
    let selected: Vec<bool> = regions
        .iter()
        .map(|(mark, cursor)| mark != cursor)
        .collect();
    assert!(selected[0] && !selected[5], "{selected:?}");
    assert!(
        selected.windows(2).all(|pair| pair[0] || !pair[1]),
        "in order"
    );
    assert!(editor.window().selections.mark_active().is_active());
}

#[test]
fn the_most_cursors_a_window_may_have_all_select_their_words() {
    // Each cursor's seeks to where it starts come apart from the bytes they share.
    let text = "word ".repeat(MAX_CURSORS);
    let starts: Vec<usize> = (0..MAX_CURSORS).map(|index| index * 5 + 1).collect();
    let mut editor = editor(&text);
    at(&mut editor, &starts);
    assert_eq!(editor.select_words().unwrap(), 0..4);
    let (regions, _) = regions(&editor);
    let words: Vec<(usize, usize)> = (0..MAX_CURSORS)
        .map(|index| (index * 5, index * 5 + 4))
        .collect();
    assert_eq!(regions, words);
}
