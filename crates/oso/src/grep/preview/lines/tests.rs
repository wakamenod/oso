//! Tests of reading the lines a list of a server's places shows (ADR 0138, step 8b): several
//! lines in one pass, the last with no break, the bounds, a file that may hold secrets, a link
//! on the way, and a root that holds the home directory.
//!
//! Must not depend on: `app`, winit or the UI thread; these read files they make in a scratch
//! directory.

use std::io::Cursor;
use std::path::Path;

use super::super::super::tests::{Scratch, leads};
use super::{LINE_KEPT, RowLines, read_lines, scan};

fn text(found: &[(usize, Vec<u8>)]) -> Vec<(usize, String)> {
    (found.iter())
        .map(|(line, bytes)| (*line, String::from_utf8_lossy(bytes).into_owned()))
        .collect()
}

fn lines_of(bytes: &[u8], lines: &[usize], budget: &mut u64) -> Vec<(usize, String)> {
    text(&scan(Cursor::new(bytes.to_vec()), lines, budget, &|| true).unwrap())
}

#[test]
fn the_lines_asked_for_are_found_in_one_pass_the_last_with_no_break() {
    let mut budget = u64::MAX;
    let found = lines_of(b"one\ntwo\nthree\nfour", &[1, 3, 4], &mut budget);
    assert_eq!(
        found,
        [(1, "one".into()), (3, "three".into()), (4, "four".into())]
    );
    assert_eq!(budget, u64::MAX - 18, "what was read comes off the budget");
    let found = lines_of(b"a\nb\n", &[2, 2, 1, 9], &mut u64::MAX.clone());
    assert_eq!(
        found,
        [(2, "b".into())],
        "past the end, twice or out of order: once"
    );
}

#[test]
fn a_line_across_parts_is_found_and_kept_to_its_bound() {
    let mut bytes = vec![b'\n'; 70_000];
    bytes.extend(std::iter::repeat_n(b'x', 100_000));
    bytes.push(b'\n');
    let found = lines_of(&bytes, &[70_001], &mut u64::MAX.clone());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, 70_001);
    assert_eq!(found[0].1.len(), LINE_KEPT);
}

#[test]
fn nothing_past_the_budget_is_read() {
    let mut budget = 6;
    let found = lines_of(b"ab\ncd\nef\n", &[1, 3], &mut budget);
    assert_eq!(found, [(1, "ab".into())]);
    assert_eq!(budget, 0);
}

#[test]
fn a_stale_read_stops() {
    let found = scan(Cursor::new(b"a\n".to_vec()), &[1], &mut 10, &|| false);
    assert!(found.is_none());
}

#[test]
fn a_file_is_reached_as_the_preview_reaches_it_and_one_that_may_hold_secrets_never() {
    let scratch = Scratch::new("row-lines");
    scratch.file("src/a.rs", "fn a() {}\nfn b() {}\n");
    scratch.file(".env", "TOKEN=1\n");
    let root = scratch.0.as_path();
    let leads = leads(root);
    let read = |path: &str, home: Option<&Path>| {
        read_lines(
            (root, leads.as_deref()),
            Path::new(path),
            &[2],
            home,
            &mut u64::MAX.clone(),
            &|| true,
        )
    };
    assert_eq!(
        read("src/a.rs", None),
        Some(RowLines::Read(vec![(2, b"fn b() {}".to_vec())]))
    );
    assert_eq!(read(".env", None), Some(RowLines::Withheld));
    assert_eq!(read("src/missing.rs", None), Some(RowLines::Unreadable));
    assert_eq!(read("../a.rs", None), Some(RowLines::Unreadable));
    assert_eq!(
        read("src/a.rs", Some(root)),
        Some(RowLines::Unreadable),
        "never under a root that is the home directory"
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("src"), root.join("linked")).unwrap();
        assert_eq!(read("linked/a.rs", None), Some(RowLines::Unreadable));
    }
}
