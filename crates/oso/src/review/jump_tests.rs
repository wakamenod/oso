//! Carrying a line of the reviewed new side to the file's text now, the work tree's file read
//! for it, and lines taken across a reading's hunks.
//!
//! Must not depend on: `app` or git; these carry lines through texts made here, and read files
//! of a temporary directory.

use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_core::rope::Rope;
use oso_platform::file_meta::Directory;
use proptest::prelude::*;

use oso_core::review::{Hunk, Side};

use super::{Ask, Carried, Jump, Landed, across, answer, carry};
use crate::review::tests::scratch_dir;

fn later() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

fn lines(n: usize) -> Vec<u8> {
    (1..=n)
        .flat_map(|k| format!("line {k}\n").into_bytes())
        .collect()
}

#[test]
fn a_text_as_it_was_read_keeps_every_line() {
    let new = lines(5);
    assert_eq!(carry(&new, &new, 3, later()), (3, Carried::Kept));
}

#[test]
fn lines_put_in_above_move_the_line_down_and_those_below_leave_it() {
    let new = lines(5);
    let now = [b"a\nb\n".as_slice(), &new, b"tail\n"].concat();
    assert_eq!(carry(&new, &now, 3, later()), (5, Carried::Kept));
    // A line put in front of the very line moves it on too.
    let mut now = lines(2);
    now.extend_from_slice(b"inserted\n");
    now.extend_from_slice(&new[lines(2).len()..]);
    assert_eq!(carry(&new, &now, 3, later()), (4, Carried::Kept));
}

#[test]
fn lines_taken_out_above_move_the_line_up() {
    let new = lines(6);
    let now = new[lines(2).len()..].to_vec();
    assert_eq!(carry(&new, &now, 5, later()), (3, Carried::Kept));
}

#[test]
fn a_line_changed_since_goes_to_the_start_of_what_replaced_it() {
    let new = lines(6);
    let now = b"line 1\nline 2\nX\nY\nZ\nline 6\n";
    // Lines 3 to 5 became X, Y, Z: line 4 goes to X's line.
    assert_eq!(carry(&new, now, 4, later()), (3, Carried::Changed));
}

#[test]
fn a_line_taken_out_goes_to_the_line_after_it_and_the_end_clamps() {
    let new = lines(4);
    let now = b"line 1\nline 2\nline 3\n";
    // Line 4 was the last, and is gone: the last line there is.
    assert_eq!(carry(&new, now, 4, later()), (3, Carried::Changed));
    assert_eq!(carry(&new, b"", 2, later()), (1, Carried::Changed));
}

#[test]
fn past_the_deadline_or_the_line_limit_the_line_is_not_carried() {
    let new = lines(5);
    let now = lines(6);
    let (line, carried) = carry(&new, &now, 3, Instant::now());
    assert_eq!(line, 3);
    assert!(matches!(carried, Carried::Not(why) if why.contains("in time")));
    let long = vec![b'\n'; crate::review::diff::MAX_LINES + 1];
    let (line, carried) = carry(&new, &long, 3, later());
    assert_eq!(line, 3);
    assert!(matches!(carried, Carried::Not(why) if why.contains("too long")));
}

fn line_of(jumped: super::Jumped) -> Result<(u32, Carried), String> {
    match jumped.landed {
        Ok(Landed::Line { line, carried }) => Ok((line, carried)),
        Ok(Landed::Side(_)) => Err("a side".into()),
        Err(why) => Err(why),
    }
}

fn carry_jump(path: &[u8], new: &[u8], line: u32, now: Option<Rope>) -> Jump {
    Jump {
        ticket: 7,
        ask: Ask::Carry {
            path: path.into(),
            new: Arc::from(new),
            line,
            now,
        },
    }
}

#[test]
fn the_open_buffer_s_text_is_carried_to_and_no_file_is_read() {
    let new = lines(3);
    let now = Rope::from("added\nline 1\nline 2\nline 3\n");
    // No top: the file could not be read, and need not be.
    let jumped = answer(carry_jump(b"gone/a.rs", &new, 2, Some(now)), None);
    assert_eq!(jumped.ticket, 7);
    assert_eq!(line_of(jumped), Ok((3, Carried::Kept)));
}

#[test]
fn the_work_tree_s_file_is_read_with_no_link_followed_nor_a_file_of_two_names() {
    let dir = scratch_dir("jump-file");
    let root = dir.as_path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), b"top\nline 1\nline 2\n").unwrap();
    let top = Directory::open(root).unwrap();
    let new = lines(2);
    let found = answer(carry_jump(b"src/a.rs", &new, 2, None), Some(&top));
    assert_eq!(line_of(found), Ok((3, Carried::Kept)));
    let gone = answer(carry_jump(b"src/b.rs", &new, 2, None), Some(&top));
    assert!(
        line_of(gone)
            .unwrap_err()
            .contains("no longer in the work tree")
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("src"), root.join("linked")).unwrap();
        std::os::unix::fs::symlink(root.join("src/a.rs"), root.join("b.rs")).unwrap();
        std::fs::hard_link(root.join("src/a.rs"), root.join("twice.rs")).unwrap();
        // Through a linked directory: nothing is there, as no link is followed.
        let through = answer(carry_jump(b"linked/a.rs", &new, 2, None), Some(&top));
        assert!(
            line_of(through)
                .unwrap_err()
                .contains("no longer in the work tree")
        );
        let link = answer(carry_jump(b"b.rs", &new, 2, None), Some(&top));
        assert!(line_of(link).unwrap_err().contains("link"));
        let shared = answer(carry_jump(b"twice.rs", &new, 2, None), Some(&top));
        assert!(line_of(shared).unwrap_err().contains("more than one name"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_side_is_built_whole_with_its_invalid_bytes_kept() {
    let jump = Jump {
        ticket: 1,
        ask: Ask::Side {
            bytes: Arc::from(&b"fn a() {}\n\xff\n"[..]),
        },
    };
    let Ok(Landed::Side(text)) = answer(jump, None).landed else {
        panic!("a side's text");
    };
    assert!(text.is_changed());
}

proptest! {
    #[test]
    fn a_line_kept_lands_on_its_own_text(
        new in proptest::collection::vec(prop::sample::select(&b"abc\n"[..]), 0..80),
        now in proptest::collection::vec(prop::sample::select(&b"abc\n"[..]), 0..80),
        pick in 1u32..30,
    ) {
        let count = crate::review::diff::line_count(&new);
        prop_assume!(count > 0);
        let line = pick.min(u32::try_from(count).unwrap());
        let (landed, carried) = carry(&new, &now, line, later());
        let now_lines = crate::review::diff::lines(&now);
        prop_assert!(landed >= 1);
        prop_assert!(landed as usize <= now_lines.len().max(1));
        if carried == Carried::Kept && !now_lines.is_empty() {
            // A line kept is the same text in the text now.
            let new_lines = crate::review::diff::lines(&new);
            prop_assert_eq!(new_lines[line as usize - 1], now_lines[landed as usize - 1]);
        }
    }
}

#[test]
fn an_open_buffer_past_a_side_s_limit_is_not_copied() {
    let big = "x\n".repeat(usize::try_from(crate::review::MAX_BLOB).unwrap() / 2 + 1);
    let jumped = answer(
        carry_jump(b"a.rs", b"x\n", 1, Some(Rope::from(big.as_str()))),
        None,
    );
    let (line, carried) = line_of(jumped).unwrap();
    assert_eq!(line, 1);
    assert!(matches!(carried, Carried::Not(why) if why.contains("too large")));
}

fn hunk(old_start: u32, new_start: u32, removed: usize, added: usize) -> Hunk {
    let lines = |n: usize| (0..n).map(|_| b"x\n".as_slice().into()).collect();
    Hunk {
        old_start,
        new_start,
        removed: lines(removed),
        added: lines(added),
    }
}

/// Old `a b c d e f` into new `a B1 B2 c d x e`: `b` into two lines, `x` put in before `e`,
/// `f` taken out.
fn hunks() -> Vec<Hunk> {
    vec![hunk(2, 2, 1, 2), hunk(5, 6, 0, 1), hunk(6, 8, 1, 0)]
}

#[test]
fn a_removed_line_goes_to_where_it_was_removed_and_kept_ones_move_with_the_hunks() {
    let hunks = hunks();
    // `a` is before every hunk.
    assert_eq!(across(&hunks, Side::Old, 1), 1);
    // `b` became `B1 B2`: where it was removed is their first.
    assert_eq!(across(&hunks, Side::Old, 2), 2);
    // `c` and `d` moved one on.
    assert_eq!(across(&hunks, Side::Old, 3), 4);
    assert_eq!(across(&hunks, Side::Old, 4), 5);
    // `e` moved past `x` too.
    assert_eq!(across(&hunks, Side::Old, 5), 7);
    // `f` was taken out at the end: the line after the new side's last.
    assert_eq!(across(&hunks, Side::Old, 6), 8);
}

#[test]
fn an_added_line_goes_to_where_it_was_put_in_on_the_old_side() {
    let hunks = hunks();
    assert_eq!(across(&hunks, Side::New, 3), 2, "B2 is where b was");
    assert_eq!(across(&hunks, Side::New, 4), 3, "c");
    assert_eq!(across(&hunks, Side::New, 6), 5, "x goes in front of e");
    assert_eq!(across(&hunks, Side::New, 7), 5, "e");
    assert_eq!(across(&[], Side::New, 9), 9);
}
