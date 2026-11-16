//! Tests of a followed buffer's diagnostics through edits, undo, loads, unfollowing and secrets
//! (ADR 0138, step 6).
//!
//! Must not depend on: `window`, `editor` or `command`.

#![expect(
    clippy::single_range_in_vec_init,
    reason = "a set of one range is what most of these check"
)]

use crate::buffer::{Buffer, Replacement};
use crate::diagnostic::{Diagnostics, New, Severity, Tags};
use crate::load::Builder;
use crate::undo::{Direction, Opening};

fn edit(buffer: &mut Buffer, range: std::ops::Range<usize>, text: &str) {
    let staged = buffer
        .stage_edits(&[Replacement { range, text }])
        .unwrap()
        .unwrap();
    buffer.commit_edit(staged, Opening::New(None), None);
}

fn report(ranges: &[std::ops::Range<usize>], len: usize) -> Diagnostics {
    Diagnostics::new(
        "fake",
        ranges.iter().map(|range| New {
            range: range.clone(),
            severity: Severity::Error,
            tags: Tags::default(),
            source: "",
            code: "",
            message: "m",
        }),
        len,
    )
}

fn ranges(buffer: &Buffer) -> Option<Vec<std::ops::Range<usize>>> {
    buffer
        .diagnostics()
        .map(|set| set.iter().map(|diagnostic| diagnostic.range()).collect())
}

fn followed(text: &str) -> Buffer {
    let mut buffer = Buffer::from_text("t", text);
    buffer.set_followed(true);
    buffer
}

#[test]
fn a_report_for_an_older_revision_follows_the_edits_since_and_each_edit_after() {
    let mut buffer = followed("let x = 1;\n");
    let reported = buffer.revision();
    edit(&mut buffer, 0..0, "// a\n");
    assert!(buffer.take_diagnostics(reported, report(&[4..5], 11)));
    assert_eq!(ranges(&buffer), Some(vec![9..10]));
    edit(&mut buffer, 0..5, "");
    assert_eq!(ranges(&buffer), Some(vec![4..5]));
    buffer.cross_whole(Direction::Undo).unwrap();
    assert_eq!(ranges(&buffer), Some(vec![9..10]), "an undo is a batch too");
}

#[test]
fn a_buffer_no_server_follows_takes_none_and_unfollowing_drops_them() {
    let mut plain = Buffer::from_text("t", "abc");
    assert!(!plain.take_diagnostics(plain.revision(), report(&[0..1], 3)));
    assert_eq!(ranges(&plain), None);
    let mut buffer = followed("abc");
    assert!(buffer.take_diagnostics(buffer.revision(), report(&[0..1], 3)));
    buffer.set_followed(false);
    assert_eq!(ranges(&buffer), None);
    buffer.set_followed(true);
    assert_eq!(ranges(&buffer), None, "they do not come back");
}

#[test]
fn a_report_the_log_no_longer_reaches_or_that_does_not_fit_is_dropped() {
    let mut buffer = followed("abc");
    let old = buffer.revision();
    assert!(buffer.take_diagnostics(old, report(&[0..1], 3)));
    for _ in 0..70 {
        edit(&mut buffer, 0..0, "x");
    }
    assert!(!buffer.take_diagnostics(old, report(&[1..2], 3)));
    assert_eq!(ranges(&buffer), Some(vec![70..71]), "the old set followed");
    let now = buffer.revision();
    assert!(!buffer.take_diagnostics(now, report(&[1..200], 300)));
}

#[test]
fn a_load_and_a_secret_verdict_drop_them() {
    let mut buffer = followed("abc");
    assert!(buffer.take_diagnostics(buffer.revision(), report(&[0..1], 3)));
    let mut builder = Builder::new(None, crate::display_map::TabWidth::default());
    builder.push(b"abcd");
    builder.finish();
    buffer.apply_load(builder.snapshot());
    assert_eq!(ranges(&buffer), None);

    let mut secret = followed("abc");
    assert!(secret.take_diagnostics(secret.revision(), report(&[0..1], 3)));
    assert!(secret.treat_as_secret());
    assert_eq!(ranges(&secret), None);
    assert!(!secret.take_diagnostics(secret.revision(), report(&[0..1], 3)));
}
