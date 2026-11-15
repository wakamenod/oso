//! Tests of directory listings in the state a restart hands over (ADR 0134).
//!
//! Must not depend on: a window or the file system; a state is a plain value.

use std::path::PathBuf;

use serde_json::json;

use super::export_value as export;
use super::parse;
use super::tests::{absolute, visiting, with};

/// A directory listing goes by its directory, order, details and line, and its window shows
/// it again (ADR 0134).
#[test]
fn a_listing_is_handed_over_to_be_listed_again() {
    let directory = absolute("/p/src");
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let id = editor.add_listing(PathBuf::from(&directory));
    editor.show(id);
    let state = export(&editor, &[], None, None).unwrap();
    let parsed = parse(&state).unwrap();
    let listing = parsed
        .buffers
        .iter()
        .find_map(|buffer| buffer.listing.clone())
        .unwrap();
    assert_eq!(listing.directory, PathBuf::from(&directory));
    assert!(listing.details);
    assert!(!listing.by_date);
    let shown = &parsed.buffers[parsed.window().shown];
    assert_eq!(shown.listing.as_ref(), Some(&listing));
}

#[test]
fn a_listing_with_a_path_is_refused() {
    let state = with(|state| {
        let buffers = state["buffers"].as_array_mut().unwrap();
        let file = buffers
            .iter_mut()
            .find(|buffer| !buffer["path"].is_null())
            .unwrap();
        file["listing"] = json!({
            "directory": absolute("/p"), "details": true, "by_date": false, "line": 0
        });
    });
    assert!(parse(&state).is_err());
}
