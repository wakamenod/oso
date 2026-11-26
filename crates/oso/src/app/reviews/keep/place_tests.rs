//! Where a saved cursor goes back (ADR 0122, step 6; ADR 0184): to its line once the text shows
//! it, not to its file's row while the file's lines are not read yet.
//!
//! Must not depend on: the app's threads, git or files.

use std::sync::Arc;

use oso_core::review::{RowKind, Rows, Section, Side};

use super::{Spot, spot};
use crate::review::saved::Place;

/// The rows of a review of `a.txt`: its title, then the file's row, and with `read` its lines,
/// old line 75 and new line 75.
fn rows(read: bool) -> Rows {
    let mut title = Section::new(Box::default());
    title.push(RowKind::Note, 1);
    let mut file = Section::new(b"a.txt".as_slice().into());
    file.push(RowKind::File, 1);
    if read {
        file.push(RowKind::Hunk { hunk: 0 }, 1);
        let lines = |side| RowKind::Lines {
            hunk: 0,
            side,
            first: 75,
        };
        file.push(lines(Side::Old), 1);
        file.push(lines(Side::New), 1);
    }
    Rows::new(vec![Arc::new(title), Arc::new(file)])
}

fn place(line: Option<(Side, u32)>) -> Place {
    Place {
        path: b"a.txt".as_slice().into(),
        line,
        above: Some(6),
    }
}

#[test]
fn a_place_on_a_line_waits_for_its_files_lines_and_then_goes_to_that_line() {
    let on_line = place(Some((Side::Old, 75)));
    // The first text shows the file's row before its lines are read: not there yet.
    assert_eq!(spot(&rows(false), &on_line, false), Spot::Wait);
    // Its lines in: old line 75 is the review's fourth line.
    assert_eq!(spot(&rows(true), &on_line, false), Spot::At(3));
    // Every file read and the line not shown: the file's row.
    assert_eq!(spot(&rows(false), &on_line, true), Spot::At(1));
    // A place on the file's row goes there at once.
    assert_eq!(spot(&rows(false), &place(None), false), Spot::At(1));
    // A file the review does not show: gone once every file was read, else not yet.
    let elsewhere = Place {
        path: b"b.txt".as_slice().into(),
        ..place(None)
    };
    assert_eq!(spot(&rows(true), &elsewhere, false), Spot::Wait);
    assert_eq!(spot(&rows(true), &elsewhere, true), Spot::Gone);
}
