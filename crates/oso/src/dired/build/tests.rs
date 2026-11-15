//! Tests of a listing's text, highlights and rows.
//!
//! Must not depend on: the file system.

use std::time::{Duration, SystemTime};

use oso_core::syntax::Highlight;
use oso_platform::file_meta::EntryKind;

use std::collections::HashSet;

use super::{Entry, Leads, Shape, build};

/// 2026-11-15 09:12:00 UTC.
fn at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_794_733_920)
}

fn entry(name: &[u8], kind: EntryKind, mode: u32, len: u64) -> Entry {
    Entry {
        name: unsafe_free_name(name),
        kind,
        mode,
        mode_text: super::super::rows::mode_text(mode),
        len,
        modified: Some(at()),
        ..Entry::default()
    }
}

#[cfg(unix)]
fn unsafe_free_name(name: &[u8]) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(name.to_vec())
}

#[cfg(not(unix))]
fn unsafe_free_name(name: &[u8]) -> std::ffi::OsString {
    String::from_utf8_lossy(name).into_owned().into()
}

fn entries() -> Vec<Entry> {
    let mut link = entry(b"latest", EntryKind::Link, 0o120_755, 8);
    link.target = Some("oso-core".into());
    link.leads = Leads::Directory;
    let mut broken = entry(b"gone", EntryKind::Link, 0o120_755, 7);
    broken.target = Some("nowhere".into());
    broken.leads = Leads::Missing;
    vec![
        entry(b".", EntryKind::Directory, 0o040_755, 0),
        entry(b"..", EntryKind::Directory, 0o040_755, 0),
        entry(b"oso", EntryKind::Directory, 0o040_755, 128),
        link,
        broken,
        entry(b"run.sh", EntryKind::File, 0o100_755, 300),
        entry(b"AGENTS.md", EntryKind::File, 0o100_644, 4096),
        entry(b"pipe", EntryKind::Other, 0o010_600, 0),
    ]
}

fn shown(details: bool, entries: &[Entry]) -> (String, super::Shown) {
    let refs: Vec<&Entry> = entries.iter().collect();
    let marked = HashSet::new();
    let shape = Shape {
        header: b"~/crates/",
        notes: &[],
        details,
        marked: &marked,
    };
    let shown = build(&shape, &refs, at(), &mut |_| 0);
    (shown.text.text().to_string(), shown)
}

/// Unix's mode column; Windows has its own test below.
#[cfg(unix)]
#[test]
fn rows_read_as_ls_l_writes_them() {
    let (text, shown) = shown(true, &entries());
    let expected = "  ~/crates/:\n\
        \x20 drwxr-xr-x     -  Nov 15 09:12  .\n\
        \x20 drwxr-xr-x     -  Nov 15 09:12  ..\n\
        \x20 drwxr-xr-x     -  Nov 15 09:12  oso\n\
        \x20 lrwxr-xr-x     8  Nov 15 09:12  latest -> oso-core\n\
        \x20 lrwxr-xr-x     7  Nov 15 09:12  gone -> nowhere\n\
        \x20 -rwxr-xr-x   300  Nov 15 09:12  run.sh\n\
        \x20 -rw-r--r--  4.0K  Nov 15 09:12  AGENTS.md\n\
        \x20 prw-------     0  Nov 15 09:12  pipe\n";
    assert_eq!(text, expected);
    assert_eq!(shown.header_lines, 1);
    assert_eq!(
        shown.name_column,
        text.lines().nth(1).unwrap().find('.').unwrap()
    );
    let names: Vec<_> = shown
        .rows
        .iter()
        .map(|row| row.name.to_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            ".",
            "..",
            "oso",
            "latest",
            "gone",
            "run.sh",
            "AGENTS.md",
            "pipe"
        ]
    );
    let enters: Vec<_> = shown.rows.iter().map(|row| row.enters).collect();
    assert_eq!(enters, [true, true, true, true, false, false, false, false]);
}

#[test]
fn every_name_starts_at_one_column() {
    let (text, shown) = shown(true, &entries());
    for (line, row) in text.lines().skip(1).zip(shown.rows.iter()) {
        let name = row.name.to_str().unwrap();
        assert_eq!(
            &line[shown.name_column..shown.name_column + name.len()],
            name
        );
    }
    let (text, shown) = self::shown(false, &entries());
    assert_eq!(shown.name_column, 2);
    assert_eq!(text.lines().nth(4).unwrap(), "  latest");
}

/// Unix's mode column; Windows has its own test below.
#[cfg(unix)]
#[test]
fn each_part_has_its_face() {
    let (text, shown) = shown(true, &entries());
    let face_at = |needle: &str| shown.highlights.at(text.find(needle).unwrap());
    assert_eq!(face_at("~/crates"), Some(Highlight::DiredHeader));
    assert_eq!(face_at("drwx"), Some(Highlight::DiredDetails));
    assert_eq!(face_at("oso\n"), Some(Highlight::DiredDirectory));
    assert_eq!(face_at("latest"), Some(Highlight::DiredSymlink));
    assert_eq!(face_at("gone"), Some(Highlight::DiredBrokenSymlink));
    assert_eq!(face_at("run.sh"), Some(Highlight::DiredExecutable));
    assert_eq!(face_at("AGENTS"), None);
    assert_eq!(face_at("pipe"), Some(Highlight::DiredSpecial));
    assert_eq!(face_at(" -> oso-core"), None);
    assert_eq!(shown.highlights.len(), text.len());
}

/// Unix's mode column; Windows has its own test below.
#[cfg(unix)]
#[test]
fn owners_show_only_when_someone_else_owns_a_row() {
    let mut rows = entries();
    rows[6].owner = Some(("root".into(), "wheel".into()));
    let (text, _) = shown(true, &rows);
    assert!(text.contains("-rw-r--r--  root wheel  4.0K"), "{text}");
    assert!(
        text.contains(&format!("-rwxr-xr-x{} 300", " ".repeat(14))),
        "{text}"
    );
}

#[test]
fn a_name_with_invalid_bytes_or_a_line_break_keeps_its_row() {
    let rows = vec![
        entry(b"a\xffb", EntryKind::File, 0o100_644, 1),
        entry(b"two\nlines", EntryKind::Directory, 0o040_755, 0),
    ];
    let (text, shown) = shown(true, &rows);
    assert_eq!(text.lines().count(), 3);
    assert!(text.lines().nth(2).unwrap().ends_with("two\\nlines"));
    assert_eq!(text.matches('\u{FFFD}').count(), 1);
    // The face follows the decoded text: the U+FFFD's three bytes are the name's.
    let at = text.find("two").unwrap();
    assert_eq!(shown.highlights.at(at), Some(Highlight::DiredDirectory));
    assert_eq!(shown.rows[1].name, unsafe_free_name(b"two\nlines"));
}

#[test]
fn notes_go_under_the_header() {
    let notes = vec!["reading… 3 entries so far".to_owned()];
    let entries = entries();
    let refs: Vec<&Entry> = entries.iter().collect();
    let marked = HashSet::new();
    let shape = Shape {
        header: b"/tmp/",
        notes: &notes,
        details: true,
        marked: &marked,
    };
    let shown = build(&shape, &refs, at(), &mut |_| 0);
    assert_eq!(shown.header_lines, 2);
    let text = shown.text.text().to_string();
    assert_eq!(text.lines().nth(1).unwrap(), "  reading… 3 entries so far");
}

/// Windows has no mode bits: its column is PowerShell's `Mode`, and a link shows as one from its
/// attributes, as reading a directory there gives them.
#[cfg(windows)]
#[test]
fn windows_rows_show_their_attributes() {
    let mut directory = entry(b"oso", EntryKind::Directory, 0, 0);
    directory.mode_text = "d----".to_owned();
    let mut link = entry(b"latest", EntryKind::Link, 0, 0);
    link.mode_text = "la---".to_owned();
    link.target = Some("oso".into());
    link.leads = Leads::Directory;
    let mut file = entry(b"AGENTS.md", EntryKind::File, 0, 4096);
    file.mode_text = "-a---".to_owned();
    let rows = [directory, link, file];
    let (text, shown) = shown(true, &rows);
    let lines: Vec<&str> = text.lines().skip(1).collect();
    assert!(lines[0].starts_with("  d----"), "{text}");
    assert!(lines[1].starts_with("  la---"), "{text}");
    assert!(lines[1].ends_with("latest -> oso"), "{text}");
    assert!(lines[2].starts_with("  -a---"), "{text}");
    let face_at = |needle: &str| shown.highlights.at(text.find(needle).unwrap());
    assert_eq!(face_at("latest"), Some(Highlight::DiredSymlink));
    assert_eq!(face_at("oso\n"), Some(Highlight::DiredDirectory));
}

#[test]
fn a_marked_row_shows_its_mark_and_its_name_in_the_marked_face() {
    let rows = entries();
    let refs: Vec<&Entry> = rows.iter().collect();
    let marked: HashSet<std::ffi::OsString> = ["run.sh".into(), ".".into()].into_iter().collect();
    let shape = Shape {
        header: b"/tmp/",
        notes: &[],
        details: false,
        marked: &marked,
    };
    let shown = build(&shape, &refs, at(), &mut |_| 0);
    let text = shown.text.text().to_string();
    assert!(text.contains("\n* run.sh\n"), "{text}");
    // `.` is never marked.
    assert!(text.contains("\n  .\n"), "{text}");
    assert_eq!(shown.marked, 1);
    let at = text.find("* run").unwrap();
    assert_eq!(shown.highlights.at(at), Some(Highlight::DiredMark));
    assert_eq!(shown.highlights.at(at + 2), Some(Highlight::DiredMarked));
    let row = &shown.rows[5];
    assert_eq!(row.face, Some(Highlight::DiredExecutable));
    assert_eq!(row.name_len, "run.sh".len());
}
