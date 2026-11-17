//! A reject's edits: a changed, a put-in and a taken-out hunk reverted, several at once, undone
//! back to the new side, and refused where the lines moved.
//!
//! Must not depend on: the app's threads, git or files; the edits are made on a text here.

use oso_core::review::Hunk;
use oso_core::rope::Rope;

use super::revert_edits;
use crate::review::stage::CHANGED;

fn hunk(old_start: u32, new_start: u32, removed: &[&str], added: &[&str]) -> Hunk {
    let lines = |lines: &[&str]| lines.iter().map(|line| line.as_bytes().into()).collect();
    Hunk {
        old_start,
        new_start,
        removed: lines(removed),
        added: lines(added),
    }
}

/// `text` with `hunks` reverted, or undone with `undone`.
fn reverted(text: &str, hunks: &[Hunk], undone: bool) -> Result<String, &'static str> {
    let edits = revert_edits(&Rope::from(text), hunks, undone)?;
    let mut out = text.to_owned();
    // Descending, as the editor applies them.
    for (range, put) in edits {
        out.replace_range(range, &put);
    }
    Ok(out)
}

/// The old side `a b c d e f`, changed into the new side: `b` into `B1 B2`, `x` put in after
/// `d`, and `f` taken out.
fn sides() -> (&'static str, &'static str, Vec<Hunk>) {
    let old = "a\nb\nc\nd\ne\nf\n";
    let new = "a\nB1\nB2\nc\nd\nx\ne\n";
    let hunks = vec![
        hunk(2, 2, &["b\n"], &["B1\n", "B2\n"]),
        hunk(5, 6, &[], &["x\n"]),
        hunk(6, 8, &["f\n"], &[]),
    ];
    (old, new, hunks)
}

#[test]
fn each_kind_of_hunk_reverts_alone_and_all_at_once() {
    let (old, new, hunks) = sides();
    assert_eq!(
        reverted(new, &hunks[..1], false).unwrap(),
        "a\nb\nc\nd\nx\ne\n"
    );
    assert_eq!(
        reverted(new, &hunks[1..2], false).unwrap(),
        "a\nB1\nB2\nc\nd\ne\n"
    );
    assert_eq!(
        reverted(new, &hunks[2..], false).unwrap(),
        "a\nB1\nB2\nc\nd\nx\ne\nf\n"
    );
    assert_eq!(reverted(new, &hunks, false).unwrap(), old);
}

#[test]
fn undone_puts_the_new_side_back_where_the_revert_left_the_old() {
    let (old, new, hunks) = sides();
    assert_eq!(reverted(old, &hunks, true).unwrap(), new);
    // One hunk alone, reverted then undone.
    let one = reverted(new, &hunks[1..2], false).unwrap();
    assert_eq!(reverted(&one, &hunks[1..2], true).unwrap(), new);
}

#[test]
fn a_hunk_whose_lines_moved_is_refused_whole() {
    let (_, new, hunks) = sides();
    let moved = format!("top\n{new}");
    assert_eq!(reverted(&moved, &hunks, false), Err(CHANGED));
    let short = "a\nB1\n";
    assert_eq!(reverted(short, &hunks[..1], false), Err(CHANGED));
}
