//! What of a block is read (ADR 0182, "Reading"): what starts a view's wait again, what cannot be
//! read, and what was read against a model of its bytes.
//!
//! Must not depend on: the app or the clock; the tests give what the window shows, and the times.

use serde_json::json;

use super::*;

const T: Duration = Duration::from_millis(1);

/// A block of `lines` lines of 4 bytes, and what of it was read: nothing.
fn reading(lines: usize) -> Reading {
    let command = vec!["a"; lines].join("\n");
    let later = Instant::now() + Duration::from_secs(60);
    let block = Block::call("Bash", Some(&json!({"command": command})), later);
    assert_eq!(block.body.text.len(), 4 * lines);
    Reading::new(&block)
}

/// A view 80 cells wide of the lines `lines`.
fn view(lines: Range<usize>) -> View {
    View {
        window: None,
        columns: 80,
        wraps: true,
        attended: true,
        rows: (lines.enumerate())
            .map(|(at, line)| Row {
                start: 4 * line,
                end: 4 * line + 4,
                at,
                cells: 3,
            })
            .collect(),
    }
}

/// Looks at `view` until it is read, from `now` on.
fn read(reading: &mut Reading, view: &View, now: Instant) -> Instant {
    reading.look(view, true, now);
    let now = now + READ_AFTER;
    reading.look(view, false, now);
    now
}

#[test]
fn a_row_is_read_once_it_was_in_view_for_long_enough() {
    let now = Instant::now();
    let mut reading = reading(4);
    assert_eq!((reading.lines(), reading.unread()), ((0, 4), Some(0)));
    // Not presented.
    reading.look(&view(0..2), false, now);
    reading.look(&view(0..2), false, now + READ_AFTER);
    assert_eq!(reading.due(), None);
    assert!(reading.look(&view(0..2), true, now));
    assert_eq!(reading.due(), Some(now + READ_AFTER));
    assert!(!reading.look(&view(0..2), true, now + READ_AFTER - T));
    assert_eq!(reading.lines(), (0, 4));
    assert!(!reading.may_ask() && !reading.unread_in_view());
    assert!(reading.look(&view(0..2), false, now + READ_AFTER));
    assert_eq!((reading.lines(), reading.unread()), ((2, 4), Some(8)));
    assert_eq!(reading.due(), None);
    assert!(reading.may_ask());
    // The rest, in another view.
    let now = now + READ_AFTER;
    assert!(reading.look(&view(1..4), true, now));
    assert!(reading.unread_in_view() && reading.last_in_view());
    assert!(!reading.all_read());
    assert!(reading.look(&view(1..4), false, now + READ_AFTER));
    assert!(reading.all_read());
    assert_eq!((reading.lines(), reading.unread()), ((4, 4), None));
    // It stays read, its last row out of view.
    assert!(reading.look(&view(0..2), true, now + READ_AFTER));
    assert!(reading.all_read() && !reading.last_in_view());
    // Its end in view again allows once a frame of it was presented.
    reading.look(&view(1..4), false, now + READ_AFTER);
    assert!(reading.last_in_view() && !reading.allows());
    reading.look(&view(1..4), true, now + READ_AFTER);
    assert!(reading.allows());
}

#[test]
fn a_view_that_changes_starts_the_wait_again() {
    let start = Instant::now();
    let one = view(0..2);
    let mut others = vec![view(1..3)];
    others.push(View {
        columns: 79,
        ..one.clone()
    });
    others.push(View {
        wraps: false,
        ..one.clone()
    });
    others.push(View {
        attended: false,
        ..one.clone()
    });
    let mut moved = one.clone();
    moved.rows[0].at = 5;
    others.push(moved);
    let mut cut = one.clone();
    cut.rows[1].cells = 81;
    others.push(cut);
    for other in others {
        for presented in [false, true] {
            let mut reading = reading(4);
            reading.look(&one, true, start);
            reading.look(&other, presented, start + READ_AFTER - T);
            let back = start + READ_AFTER;
            reading.look(&one, true, back);
            assert_eq!(reading.lines(), (0, 4), "{other:?}");
            reading.look(&one, true, back + READ_AFTER - T);
            assert_eq!(reading.lines(), (0, 4), "{other:?}");
            reading.look(&one, true, back + READ_AFTER);
            assert_eq!(reading.lines(), (2, 4), "{other:?}");
        }
    }
}

#[test]
fn what_cannot_be_read_is_not() {
    let now = Instant::now();
    for spoil in [
        (|view: &mut View| view.wraps = false) as fn(&mut View),
        |view| view.attended = false,
        // Laid out wider than the window, as a wrap not made yet is.
        |view| view.rows[1].cells = 81,
        |view| view.rows[1].end = 4,
        |view| {
            view.rows.remove(1);
        },
    ] {
        let mut reading = reading(2);
        let mut view = view(0..2);
        spoil(&mut view);
        let now = read(&mut reading, &view, now);
        read(&mut reading, &view, now);
        assert!(!reading.all_read());
        assert!(view.attended || !reading.may_ask());
        // What is in view may be so where no one attends to it.
        assert!(!view.attended || !reading.unread_in_view());
        assert!(!view.attended || !reading.allows());
        assert!(reading.unread().is_some());
        assert_eq!(reading.due(), None);
    }
    // Nothing of it in view is none of it seen.
    let mut reading = reading(2);
    read(&mut reading, &view(0..0), now);
    assert!(!reading.may_ask());
}

#[test]
fn a_block_that_does_not_show_all_is_never_all_read() {
    let now = Instant::now();
    let mut lines = reading(3);
    // The last line says that two are not shown.
    lines.hidden = Hidden::Lines(2);
    read(&mut lines, &view(1..3), now);
    assert_eq!(lines.lines(), (1, 4));
    read(&mut lines, &view(0..3), now + READ_AFTER);
    assert_eq!(lines.lines(), (2, 4));
    assert!(lines.shown_read() && !lines.all_read());
    let mut none = reading(1);
    none.hidden = Hidden::All;
    read(&mut none, &view(0..1), now);
    assert!(none.shown_read() && !none.all_read());
    assert_eq!(none.lines(), (0, 0));
}

mod any {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        /// What was read is the bytes of the views that stood, as a set of bytes has them, at any
        /// width: never less after, and in as few ranges as they make.
        #[test]
        fn what_was_read_is_what_stood_in_view(
            lines in 1usize..20,
            steps in prop::collection::vec((0usize..90, 1usize..12, 1usize..6, 0u64..900, any::<bool>()), 0..40),
        ) {
            let mut reading = reading(lines);
            let len = reading.len();
            let mut model = vec![false; len];
            let mut held: Option<(View, Instant)> = None;
            let mut now = Instant::now();
            for (from, rows, bytes, wait, presented) in steps {
                let from = from % len;
                let mut shown = view(0..0);
                for at in 0..rows {
                    let start = (from + at * bytes).min(len);
                    let end = (start + bytes).min(len);
                    if start < end {
                        shown.rows.push(Row { start, end, at, cells: 3 });
                    }
                }
                reading.look(&shown, presented, now);
                if held.as_ref().is_none_or(|(view, _)| *view != shown) {
                    held = presented.then(|| (shown.clone(), now));
                }
                now += Duration::from_millis(wait);
                let before = reading.clone();
                reading.look(&shown, false, now);
                if let Some((view, since)) = &held && now - *since >= READ_AFTER {
                    for row in &view.rows {
                        model[row.start..row.end].fill(true);
                    }
                }
                prop_assert!(before.read.iter().all(|bytes| reading.covers(bytes)));
                // Apart, in order, and the same bytes.
                let mut bytes = vec![false; len];
                for pair in reading.read.windows(2) {
                    prop_assert!(pair[0].end < pair[1].start);
                }
                for read in &reading.read {
                    prop_assert!(read.start < read.end);
                    bytes[read.clone()].fill(true);
                }
                prop_assert_eq!(&bytes, &model);
                prop_assert_eq!(reading.unread(), model.iter().position(|read| !read));
                prop_assert_eq!(reading.all_read(), model.iter().all(|read| *read));
                let whole = model.chunks(4).filter(|line| line.iter().all(|read| *read)).count();
                prop_assert_eq!(reading.lines(), (whole, lines));
            }
        }
    }
}
