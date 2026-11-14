//! A listing's order (ADR 0134): `.` and `..` first, then directories and the links that lead
//! to one, then the rest, as `--group-directories-first`; in each, by name ignoring ASCII case
//! and then by bytes, or by time, newest first.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the file system.

use std::cmp::Ordering;

use super::Sort;
use super::build::Entry;

/// Orders `entries`, which hold neither `.` nor `..`.
pub(super) fn sort(entries: &mut [Entry], sort: Sort) {
    entries.sort_unstable_by(|a, b| {
        let groups = b.enters().cmp(&a.enters());
        let within = match sort {
            Sort::Name => by_name(a, b),
            // Newest first, and an unknown time last.
            Sort::Time => b.modified.cmp(&a.modified).then_with(|| by_name(a, b)),
        };
        groups.then(within)
    });
}

/// By name ignoring ASCII case, then by bytes, so that the order is total.
fn by_name(a: &Entry, b: &Entry) -> Ordering {
    let (a, b) = (a.name.as_encoded_bytes(), b.name.as_encoded_bytes());
    let folded = a
        .iter()
        .map(u8::to_ascii_lowercase)
        .cmp(b.iter().map(u8::to_ascii_lowercase));
    folded.then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use oso_platform::file_meta::EntryKind;

    use super::super::build::{Entry, Leads};
    use super::{Sort, sort};

    fn entry(name: &str, kind: EntryKind, age: u64) -> Entry {
        Entry {
            name: name.into(),
            kind,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - age)),
            ..Entry::default()
        }
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.to_str().unwrap()).collect()
    }

    #[test]
    fn directories_come_first_then_names_ignoring_case() {
        let mut entries = vec![
            entry("b.md", EntryKind::File, 0),
            entry("Zeta", EntryKind::Directory, 0),
            entry("A.md", EntryKind::File, 0),
            entry("a.md", EntryKind::File, 0),
            entry(".env", EntryKind::File, 0),
            entry("alpha", EntryKind::Directory, 0),
        ];
        let mut to_dir = entry("link", EntryKind::Link, 0);
        to_dir.leads = Leads::Directory;
        entries.push(to_dir);
        sort(&mut entries, Sort::Name);
        assert_eq!(
            names(&entries),
            ["alpha", "link", "Zeta", ".env", "A.md", "a.md", "b.md"]
        );
    }

    #[test]
    fn by_time_the_newest_comes_first() {
        let mut entries = vec![
            entry("old", EntryKind::File, 300),
            entry("new", EntryKind::File, 1),
            entry("dir", EntryKind::Directory, 500),
            entry("mid", EntryKind::File, 100),
        ];
        entries[3].modified = None;
        sort(&mut entries, Sort::Time);
        assert_eq!(names(&entries), ["dir", "new", "old", "mid"]);
    }
}
