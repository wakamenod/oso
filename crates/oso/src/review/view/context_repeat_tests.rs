//! A stretch asked again before the lines it asked for show: a key pressed again reads the
//! rows the next text replaces, and the view takes the stretch as the next lines that way.
//!
//! Must not depend on: `app`, `dispatch` or winit; these drive the view's state.

use std::ops::Range;
use std::sync::Arc;

use super::super::{Carrying, Tell};
use super::tests::{built_lines, numbered, reading_of};

/// The lines of the file whose `changed` lines of `count` changed once the view took the
/// stretches `asked`, one after another, each as a key asks for one.
fn lines_after(count: u32, changed: &[u32], asked: &[Range<u32>]) -> Vec<String> {
    let reading = Arc::new(reading_of(
        numbered(count, &[]),
        numbered(count, changed),
        b"a",
    ));
    let mut told = vec![
        Tell::Listed {
            entries: vec![reading.entry.clone()].into(),
            more: 0,
            cut: false,
        },
        Tell::File {
            reading: Arc::clone(&reading),
            carry: Carrying::Nothing,
        },
    ];
    told.extend(asked.iter().map(|range| Tell::Context {
        path: b"a.txt"[..].into(),
        show: Some(range.clone()),
    }));
    built_lines(told)
}

fn shows(lines: &[String], n: u32) -> bool {
    lines.iter().any(|line| *line == format!(" l{n}"))
}

#[test]
fn a_stretch_asked_again_before_its_lines_show_shows_the_next_ones_below() {
    // After the hunk at 30, lines 31 to 33 show and 34 on hide: `]` asks for 34 to 53.
    let once = lines_after(100, &[30], std::slice::from_ref(&(34..54)));
    assert!(shows(&once, 53) && !shows(&once, 54));
    let twice = lines_after(100, &[30], &[34..54, 34..54]);
    assert!(shows(&twice, 73) && !shows(&twice, 74), "{twice:?}");
    let thrice = lines_after(100, &[30], &[34..54, 34..54, 34..54]);
    assert!(shows(&thrice, 93) && !shows(&thrice, 94));
}

#[test]
fn a_stretch_asked_again_at_a_gaps_bottom_shows_the_next_ones_above() {
    // Before the hunk at 60, lines 57 to 59 show and 1 to 56 hide: `[` asks for 37 to 56.
    let twice = lines_after(100, &[60], &[37..57, 37..57]);
    assert!(shows(&twice, 17) && !shows(&twice, 16), "{twice:?}");
    let thrice = lines_after(100, &[60], &[37..57, 37..57, 37..57]);
    // The gap above the hunk is gone: the file's header, then its first line.
    assert_eq!(thrice[2], " l1", "{thrice:?}");
}

#[test]
fn a_stretch_shown_with_nothing_hidden_beside_it_changes_nothing() {
    // The two hunks' context meets: every line shows.
    let plain = lines_after(12, &[2, 9], &[]);
    assert_eq!(
        lines_after(12, &[2, 9], std::slice::from_ref(&(4..6))),
        plain
    );
}

#[test]
fn a_whole_gap_asked_twice_shows_its_next_lines() {
    // After the hunk at 3, lines 7 on hide: `=` asks for its first 10,000.
    let twice = lines_after(25_010, &[3], &[7..10_007, 7..10_007]);
    assert!(shows(&twice, 20_006) && !shows(&twice, 20_007));
}
