//! Tests of the reports a session keeps: replaced by the next, forgotten when empty, and the
//! oldest of a document not open going first past each bound.
//!
//! Must not depend on: the session's loop or a process.

use oso_core::diagnostic::{Severity, Tags};

use std::path::{Path, PathBuf};

use super::*;
use crate::limits::DIAGNOSTICS_MAX;
use crate::position::Position;
use crate::protocol::Reported;

fn report(uri: &str, count: usize, message_bytes: usize) -> Published {
    Published {
        uri: uri.to_owned(),
        version: None,
        diagnostics: (0..count)
            .map(|_| Reported {
                start: Position::new(0, 0),
                end: Position::new(0, 1),
                severity: Severity::Error,
                tags: Tags::default(),
                source: String::new(),
                code: String::new(),
                message: "m".repeat(message_bytes),
            })
            .collect(),
    }
}

#[test]
fn a_report_replaces_its_document_s_last_and_an_empty_one_forgets_it() {
    let mut store = Store::default();
    store.put(
        PathBuf::from("file:///w/a.rs"),
        report("file:///w/a.rs", 2, 10),
        |_| false,
    );
    store.put(
        PathBuf::from("file:///w/a.rs"),
        report("file:///w/a.rs", 1, 10),
        |_| false,
    );
    assert_eq!(store.len(), 1);
    assert_eq!(
        store
            .get(Path::new("file:///w/a.rs"))
            .unwrap()
            .diagnostics
            .len(),
        1
    );
    assert_eq!(
        store.bytes(),
        report("file:///w/a.rs", 1, 10).bytes() + 2 * "file:///w/a.rs".len()
    );
    store.put(
        PathBuf::from("file:///w/a.rs"),
        report("file:///w/a.rs", 0, 0),
        |_| false,
    );
    assert_eq!((store.len(), store.bytes()), (0, 0));
    store.put(
        PathBuf::from("file:///w/b.rs"),
        report("file:///w/b.rs", 1, 1),
        |_| false,
    );
    store.clear();
    assert!(store.get(Path::new("file:///w/b.rs")).is_none());
}

#[test]
fn past_the_documents_bound_the_oldest_not_open_goes_first() {
    let mut store = Store::default();
    let uri = |n: usize| format!("file:///w/{n}.rs");
    let open = |kept: &Path| kept == Path::new("file:///w/0.rs");
    for n in 0..DIAGNOSED_DOCUMENTS_MAX {
        store.put(PathBuf::from(&uri(n)), report(&uri(n), 1, 0), open);
    }
    assert_eq!(store.len(), DIAGNOSED_DOCUMENTS_MAX);
    store.put(
        PathBuf::from(&uri(DIAGNOSED_DOCUMENTS_MAX)),
        report(&uri(DIAGNOSED_DOCUMENTS_MAX), 1, 0),
        open,
    );
    assert_eq!(store.len(), DIAGNOSED_DOCUMENTS_MAX);
    assert!(
        store.get(Path::new(&uri(0))).is_some(),
        "an open document's stays"
    );
    assert!(
        store.get(Path::new(&uri(1))).is_none(),
        "the oldest unopened went"
    );
    assert!(
        store
            .get(Path::new(&uri(DIAGNOSED_DOCUMENTS_MAX)))
            .is_some()
    );
}

#[test]
fn past_the_bytes_bound_the_oldest_go_and_the_newest_stays() {
    let mut store = Store::default();
    // Some 16 MiB each: five pass the bound.
    let large = |n: usize| report(&format!("file:///w/{n}.rs"), DIAGNOSTICS_MAX, 8 << 10);
    for n in 0..5 {
        store.put(PathBuf::from(format!("/w/{n}.rs")), large(n), |_| true);
        assert!(store.bytes() <= DIAGNOSTICS_BYTES_MAX, "{}", store.bytes());
    }
    assert!(store.get(Path::new("/w/4.rs")).is_some());
    assert!(
        store.get(Path::new("/w/0.rs")).is_none(),
        "all open: the oldest of any"
    );
    assert!(store.len() < 5);
}
